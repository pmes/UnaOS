// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// kepler_display.rs — Kepler (GF119+) PDISPLAY read-only trace + display takeover
//
// Cleanroom source of record: envytools/rnndb/display/g80_pdisplay.xml,
//                              envytools/rnndb/display/nv_evo.xml
//
// This module is gated on `nvidia-kepler`. It performs:
//   1. Read-only PDISPLAY head state decode (scanout address, timing, underflow).
//   2. Boot-time trace population (`kdisp_trace_0`).
//   3. Display takeover (behind `nvidia-kepler-takeover` feature).
//
// Standing rules: `:: kdisp:` log prefix; cleanroom only; no blind writes to
// uncited register addresses; bounded polls.

use super::detect::GpuInfo;
use super::kepler::{mmio_read, mmio_write, regs, VramAllocator};

/// Sentinel value for a read that returned the BAD-read pattern (BAR0 unmapped
/// or device-absent 0xFFFFFFFF) or literal zero from a register we expected to
/// be populated.  Written into `kdisp_trace_0` slots so the land-review can
/// distinguish "zero because the mirror hypothesis is wrong" from "zero because
/// we never wrote the slot".
const SENTINEL: u32 = 0xDEAD_0000;

/// Read-only PDISPLAY decode and optional display takeover.
///
/// Phase 1 (always): Reads all four GK104 heads via two candidate MMIO mirror
/// layouts and HEAD_STAT, emitting `:: kdisp:` trace rows.  Populates the
/// 7-slot `kdisp_trace_0` array for the boot-info ABI.
///
/// Phase 2 (behind `nvidia-kepler-takeover`): If a matching head is found,
/// performs the pull-5 repoint-the-surface experiment (0x6101E0 only).
pub unsafe fn takeover_display(
    gpu: &GpuInfo,
    bar0: usize,
    _allocator: &mut VramAllocator,
    kdisp_trace: &mut [u32; 7],
) -> Option<usize> {
    serial_println!(":: kdisp: begin-trace ::");

    // Note: Five early return paths exist in this function. If an early return
    // fires, the inner phase sum will not match the outer kdisp_takeover delta.
    let mut t_last = crate::arch::ms();
    macro_rules! kdisp_phase {
        ($name:expr) => {
            let t_now = crate::arch::ms();
            serial_println!(":: kdisp: inner phase={} d={} ::", $name, t_now.wrapping_sub(t_last));
            t_last = t_now;
        }
    }

    // ── PDISPLAY CAPS (0x610000) — version/class sanity check ──────────
    let caps = mmio_read(bar0, regs::NV_PDISPLAY_BASE + 0x0000);
    let version = caps & 0xFFFF;
    let class_id = (caps >> 16) & 0xFFFF;
    serial_println!(":: kdisp: caps version={:04X} class={:04X} ::", version, class_id);
    // GK107 should report VERSION=0x0210, CLASS=0x917D (GK104_DISPLAY_MASTER).

    // ── Locate GOP FB ──────────────────────────────────────────────────
    let gop_fb_phys = match crate::video::fbcon::current_base() {
        Some(base) => base,
        None => {
            serial_println!(":: kdisp: takeover-abort no-gop ::");
            *kdisp_trace = [SENTINEL; 7];
            return None;
        }
    };

    let bar1_reg = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
    let mut vram_base = (bar1_reg & 0xFFFFFFF0) as usize;
    if (bar1_reg & 0x04) != 0 {
        let bar1_high = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18);
        vram_base |= (bar1_high as usize) << 32;
    }

    if gop_fb_phys < vram_base as u64 {
        serial_println!(":: kdisp: takeover-abort gop-not-in-vram {:X} ::", gop_fb_phys);
        *kdisp_trace = [SENTINEL; 7];
        return None;
    }
    let gop_vram_offset = (gop_fb_phys - vram_base as u64) as usize;
    let expected_addr = (gop_vram_offset >> 8) as u32;
    let expected_phys = (gop_fb_phys >> 8) as u32;
    serial_println!(":: kdisp: gop phys={:X} vram_off={:X} expected_addr={:08X} ::",
        gop_fb_phys, gop_vram_offset, expected_addr);

    // ── Per-head read-only scan ────────────────────────────────────────
    // Two candidate MMIO mirror layouts for the scanout surface address:
    //
    // Candidate A ("EVO core shadow"): The existing code probed
    //   NV_PDISPLAY_BASE + 0x400 + head*0x300 + 0x60
    //   which treats the PDISPLAY MMIO space as a direct mirror of the
    //   NV_EVO_CORE pushbuffer method layout (HEAD at +0x400, stride 0x300,
    //   G80_EVO_FB_SETTINGS at +0x60 → OFFSET_ORIGIN at +0x0).
    //   Citation: nv_evo.xml lines 846–848 (HEAD array, GF119+) and
    //   nv_evo.xml lines 155–157 (G80_EVO_FB_SETTINGS in NV_EVO_CORE at +0x400).
    //   HOWEVER: These are method offsets, not necessarily MMIO-readable.
    //   If the hw does not mirror armed method state here, these read as 0.
    //
    // Candidate B ("HEAD_VAL"): Pre-GF119 layout at
    //   NV_PDISPLAY_BASE + 0xA00 + head*0x540
    //   with FB_POS at +0x128, FB_SIZE at +0x118.
    //   Citation: g80_pdisplay.xml lines 371–408 (HEAD_VAL, G80:GF119).
    //   The XML marks this G80:GF119 only, but on GK107 the MMIO space may
    //   still respond (undocumented holdover).  If wrong, reads as 0.
    //
    // Per amendment A1: dump BOTH as separate labeled rows so a zero row
    // refutes that mirror hypothesis instead of the whole decode.

    let mut found_head: Option<usize> = None;
    let mut matched_addr: u32 = 0;
    let mut matched_size: u32 = 0;
    let mut matched_storage: u32 = 0;

    for head in 0..4usize {
        // ── Candidate A: EVO core shadow ──
        let evo_base = regs::NV_PDISPLAY_BASE + 0x400 + (head * 0x300) + 0x60;
        let evo_addr    = mmio_read(bar0, evo_base + 0x0);  // OFFSET_ORIGIN
        let evo_size    = mmio_read(bar0, evo_base + 0x8);  // SIZE
        let evo_storage = mmio_read(bar0, evo_base + 0xC);  // STORAGE
        serial_println!(":: kdisp: head[{}] evo addr={:08X} size={:08X} storage={:08X} ::",
            head, evo_addr, evo_size, evo_storage);

        // ── Candidate B: HEAD_VAL (pre-GF119 layout) ──
        let hv_base = regs::NV_PDISPLAY_BASE + 0xA00 + (head * 0x540);
        let hv_fb_pos  = mmio_read(bar0, hv_base + 0x128); // FB_POS
        let hv_fb_size = mmio_read(bar0, hv_base + 0x118); // FB_SIZE
        let hv_fb_pitch = mmio_read(bar0, hv_base + 0x120); // FB_PITCH
        serial_println!(":: kdisp: head[{}] hv  fb_pos={:08X} fb_size={:08X} fb_pitch={:08X} ::",
            head, hv_fb_pos, hv_fb_size, hv_fb_pitch);

        // ── HEAD_STAT (always valid per rnndb, stride 0x800, GK104 length 4) ──
        // g80_pdisplay.xml line 647: offset 0x6000, stride 0x800, length 4 (GK104-)
        let hs_base = regs::NV_PDISPLAY_BASE + 0x6000 + (head * 0x800);
        let underflow   = mmio_read(bar0, hs_base + 0x308); // REPORT_UNDERFLOW
        let vert        = mmio_read(bar0, hs_base + 0x340); // VERT (vline[15:0], vblank_count[31:16])
        let horz        = mmio_read(bar0, hs_base + 0x344); // HORZ (hline[15:0])
        serial_println!(":: kdisp: head[{}] stat underflow={:08X} vert={:08X} horz={:08X} ::",
            head, underflow, vert, horz);

        // ── Match logic: try candidate A first, fall back to B ──
        let (addr, size, storage, label) = if is_live(evo_addr) {
            (evo_addr, evo_size, evo_storage, "evo")
        } else if is_live(hv_fb_pos) {
            (hv_fb_pos, hv_fb_size, 0u32, "hv")
        } else {
            serial_println!(":: kdisp: head[{}] skip — no live candidate ::", head);
            continue;
        };

        if !is_live(size) {
            serial_println!(":: kdisp: head[{}] skip — size sentinel {:08X} ::", head, size);
            continue;
        }

        if addr == expected_addr || addr == expected_phys {
            serial_println!(":: kdisp: head[{}] MATCH via {} addr={:08X} ::", head, label, addr);
            found_head = Some(head);
            matched_addr = addr;
            matched_size = size;
            matched_storage = storage;
            // Continue scanning remaining heads for the trace dump.
        }
    }

    // ── Populate kdisp_trace_0 ─────────────────────────────────────────
    // Slot layout:
    //   [0] CAPS (version | class<<16)
    //   [1] matched_head (0xFFFF if none)
    //   [2] matched_addr (OFFSET_ORIGIN readback)
    //   [3] matched_size
    //   [4] HEAD_STAT.REPORT_UNDERFLOW for matched head (or SENTINEL)
    //   [5] HEAD_STAT.VERT for matched head (or SENTINEL)
    //   [6] HEAD_STAT.HORZ for matched head (or SENTINEL)
    kdisp_trace[0] = caps;
    if let Some(h) = found_head {
        kdisp_trace[1] = h as u32;
        kdisp_trace[2] = matched_addr;
        kdisp_trace[3] = matched_size;
        let hs = regs::NV_PDISPLAY_BASE + 0x6000 + (h * 0x800);
        kdisp_trace[4] = mmio_read(bar0, hs + 0x308);
        kdisp_trace[5] = mmio_read(bar0, hs + 0x340);
        kdisp_trace[6] = mmio_read(bar0, hs + 0x344);
    } else {
        kdisp_trace[1] = 0xFFFF;
        for s in kdisp_trace[2..].iter_mut() { *s = SENTINEL; }
    }
    serial_println!(":: kdisp: trace [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
        kdisp_trace[0], kdisp_trace[1], kdisp_trace[2], kdisp_trace[3],
        kdisp_trace[4], kdisp_trace[5], kdisp_trace[6]);

    // ── Phase 1.5: EVO Core Read-Out (Pull 4) ──────────────────────────
    // Milestone 1: Dense core-channel window
    for pass in 0..2 {
        let mut rows = 0;
        for addr in (0x610480..=0x6104FC).step_by(4) {
            let val = mmio_read(bar0, addr);
            serial_println!(":: kdisp: evo-core pass{} off={:03X} val={:08X} ::", pass, addr - 0x610480, val);
            rows += 1;
        }
        serial_println!(":: kdisp: evo-core pass{} done rows={} ::", pass, rows);

        if pass == 0 {
            for _ in 0..2_000_000 { core::hint::spin_loop(); }
        }
    }
    kdisp_phase!("evo_core_passes");

    // Milestone 2: Known-value scan
    let mut hits = 0;
    for addr in (0x610000..=0x613FFC).step_by(4) {
        let val = mmio_read(bar0, addr);
        
        let keyname = match val {
            0x00000200 => "0x200",
            0x00020000 => "0x20000",
            0x90020000 => "0x90020000",
            0x00002D00 => "pitch2880",
            0x013C6800 => "fbsize",
            0x07380BAF | 0x0BAF0738 => "raster",
            _ if (val & 0xFFF00000) == 0x90000000 => "barshape",
            _ if (val & 0xFFFF) == 0x0B40 || (val >> 16) == 0x0B40 => "w2880",
            _ if (val & 0xFFFF) == 0x0708 || (val >> 16) == 0x0708 => "h1800",
            _ => "",
        };
        
        if !keyname.is_empty() {
            hits += 1;
            if hits <= 64 {
                serial_println!(":: kdisp: evo-scan hit off={:06X} val={:08X} key={} ::", addr, val, keyname);
            }
        }
    }
    let capped = if hits > 64 { "true" } else { "false" };
    serial_println!(":: kdisp: evo-scan done range=610000-613FFC hits={} capped={} ::", hits, capped);
    kdisp_phase!("evo_scan");

    // ── Phase 2: Assembly Write + UPDATE Latch (Pull 11) ────────────────────
    if !cfg!(feature = "nvidia-kepler-takeover") {
        serial_println!(":: kdisp: trace-only — takeover feature not set ::");
        return None;
    }

    let gop_info = match crate::video::fbcon::current_info() {
        Some(info) => info,
        None => {
            serial_println!(":: kdisp: takeover-abort no-gop-info ::");
            return None;
        }
    };
    let expected_width = gop_info.width as u32;
    let expected_height = gop_info.height as u32;
    let expected_pitch = expected_width * 4;
    
    let fbcon_stride = gop_info.stride as u32;
    let fbcon_bpp = gop_info.bytes_per_pixel as u32;
    let fbcon_row_bytes = fbcon_stride * fbcon_bpp;
    serial_println!(":: kdisp: fbcon-view base={:016X} stride_px={} bpp={} w={} h={} row_bytes={} ::",
        crate::video::fbcon::current_base().unwrap_or(0), fbcon_stride, fbcon_bpp, expected_width, expected_height, fbcon_row_bytes);
    
    let hw_pitch = 16384;
    serial_println!(":: kdisp: fbcon-vs-hw row_bytes={} hw_pitch={} match={} ::",
        fbcon_row_bytes, hw_pitch, fbcon_row_bytes == hw_pitch);
    let fb_size = (expected_width * expected_height * 4) as usize;

    let bar1 = vram_base;
    let dst = (bar1 + gop_vram_offset) as *mut u8;
    
    serial_println!(":: kdisp: surf2 geom w={} h={} pitch={} ::", expected_width, expected_height, expected_pitch);
    
    // (GOB constants removed — s25 mirror decode proved the scanout is
    // LINEAR pitch 0x4000; block-linear road retired.)
    
    // 2. Pre-state
    let asm_reg = 0x640460;
    let armed_reg = 0x6101E0;
    let shadow_reg = 0x61D1E0;
    let update_reg = 0x640080;
    
    let pre_asm = mmio_read(bar0, asm_reg);
    let pre_armed = mmio_read(bar0, armed_reg);
    let pre_shadow = mmio_read(bar0, shadow_reg);
    serial_println!(":: kdisp: latch pre asm={:08X} armed={:08X} shadow={:08X} ::", pre_asm, pre_armed, pre_shadow);

    // ── SHUTRESTORE (R19) — the deleted display rungs, back behind their own knobs ──────────
    // RULINGS R19: a rung that FAILED once keeps its CODE and its KNOB, because many boots later a
    // later path can turn out to need the earlier one OPEN. Three display rungs had been deleted
    // outright (docs/dev/OS/08_VIDEO/SHUTOUT-REGISTER.md §7); each is restored below as a
    // self-contained, reversible probe behind a DEFAULT-OFF feature of its own. `takeover_display`'s
    // control flow is unchanged — none of them returns, none of them is on the boot path without its
    // knob, and with every knob off not one of these lines exists.
    #[cfg(feature = "nvidia-kepler-repoint")]
    repoint_surface(bar0, found_head.unwrap_or(0), gop_vram_offset);
    #[cfg(feature = "nvidia-kepler-latcharm")]
    latch_arm_update(bar0, asm_reg, armed_reg, shadow_reg, update_reg, pre_asm);
    #[cfg(feature = "nvidia-kepler-pitchladder")]
    pitch_ladders(bar0, bar1, asm_reg, armed_reg, shadow_reg, update_reg, pre_asm, expected_height);

    // --- Pull 15: Mirror Surface Params (Recon) ---
    let run_recon = false;
    if run_recon {
        // Pass 1: Dense Dump
        for offset in (0x400..=0x5FC).step_by(4) {
            let val = mmio_read(bar0, 0x640000 + offset);
            let abs = if val == 0xFFFFFFFF || (val & 0xFFFF0000) == 0xBAD00000 { " ABSENT?" } else { "" };
            serial_println!(":: kdisp: mirror-sp off={:03X} val={:08X}{} ::", offset, val, abs);
        }

        // Settle
        for _ in 0..1_500_000 { core::hint::spin_loop(); }

        // Pass 2: Volatility Check
        for offset in (0x400..=0x5FC).step_by(4) {
            let val = mmio_read(bar0, 0x640000 + offset);
            let abs = if val == 0xFFFFFFFF || (val & 0xFFFF0000) == 0xBAD00000 { " ABSENT?" } else { "" };
            serial_println!(":: kdisp: mirror-sp2 off={:03X} val={:08X}{} ::", offset, val, abs);
        }

        // Pass 3: Cross-Check Candidates
        let ptr_val = mmio_read(bar0, 0x640460);
        serial_println!(":: kdisp: mirror-sp ptr-slot val={:08X} expect=00090000-ish (fw surface ptr>>8?) ::", ptr_val);

        for offset in (0x400..=0x5FC).step_by(4) {
            let val = mmio_read(bar0, 0x640000 + offset);
            if val == 0xFFFFFFFF || (val & 0xFFFF0000) == 0xBAD00000 || val == 0 { continue; }
            
            let mut kind = "";
            if val == 11520 || val == 46080 || val == 720 || val == 180 || val == 192 || val == 256 || val == (11520<<8) {
                kind = "pitch";
            } else if (val & 0xFFFF) == 2880 || (val >> 16) == 2880 || (val & 0xFFFF) == 1800 || (val >> 16) == 1800 {
                kind = "wh";
            } else if val < 0x100 {
                kind = "blockmode";
            }
            
            if !kind.is_empty() {
                serial_println!(":: kdisp: mirror-sp cand off={:03X} val={:08X} kind={} ::", offset, val, kind);
            }
        }
    }
    
    let do_takeover = true;
    if !do_takeover {
        return None;
    }
    // --- End Pull 15 ---

    let pitch_bytes = 16384;
    let total_bytes = expected_height * pitch_bytes;

    // Step 1: Prepare surf2 (linear fill, placement-model probe)

    kdisp_phase!("pre_blit_recon");

    for y in 0..expected_height {
        let band_idx = y / 16;
        // Band 0 gets a colour no other band uses, so "our row 0" is
        // identifiable in the photo without decoding the barcode.
        let band_color = if band_idx == 0 { 0xFFFF8000 } else { match band_idx % 8 {
            0 => 0xFFFF0000,
            1 => 0xFF00FF00,
            2 => 0xFF0000FF,
            3 => 0xFFFFFF00,
            4 => 0xFF00FFFF,
            5 => 0xFFFF00FF,
            6 => 0xFFFFFFFF,
            _ => 0xFF404040,
        } };
        
        let row_base = y * pitch_bytes;
        
        for x in 0..(pitch_bytes / 4) {
            let diag_x = (y * 2880) / expected_height;

            let final_color = if x >= expected_width {
                0xFF000000 // BLACK padding (real bytes the hw scans)
            } else if y < 4 || y >= expected_height - 4 {
                0xFFFFFFFF // FIDUCIAL: surface top/bottom edges, unmistakable
            } else if x < 16 {
                0xFFFFFFFF // WHITE left-edge alignment marker
            } else if x < 32 {
                0xFF000000 // BLACK spacer
            } else if x < 144 {
                // 7-bit barcode of band_idx, 16 px cells with a 4 px gutter so
                // adjacent equal bits stay countable (without the gutter,
                // 0b1110000 reads as one 48 px run and aliases to 0b1100000).
                let cell = (x - 32) % 16;
                if cell >= 12 {
                    0xFF000000 // gutter
                } else {
                    let bit_idx = 6 - ((x - 32) / 16);
                    if (band_idx >> bit_idx) & 1 == 1 {
                        0xFFFFFFFF // WHITE
                    } else {
                        0xFF000000 // BLACK
                    }
                }
            } else if x < 160 {
                0xFF000000 // BLACK spacer
            } else if x >= diag_x && x < diag_x + 16 {
                0xFFFFFFFF // diagonal ramp
            } else {
                band_color
            };
            
            let target_byte_addr = row_base + (x * 4);
            let target_ptr = dst.add(target_byte_addr as usize) as *mut u32;
            core::ptr::write_volatile(target_ptr, final_color);
        }
    }
    kdisp_phase!("blit");

    serial_println!(":: kdisp: fb-draw base={:08X} pitch={} rows={} bytes={:08X} ::", gop_vram_offset, pitch_bytes, expected_height, total_bytes);

    // Overlap check (intentional for fb-draw)
    let gop_bytes = (expected_height * pitch_bytes) as usize;
    let surf2_bytes = total_bytes as usize;
    // We now draw AT the GOP base by design, so "do we overlap" is trivially
    // yes and no longer informative. The live question is whether our extent
    // exactly covers the scanned surface — a mismatch means rows are missing
    // off the bottom or we are writing past the FB into allocator territory.
    serial_println!(":: kdisp: fb-draw cover={} ours={:08X}+{:08X} gop={:08X}+{:08X} ::",
        if surf2_bytes == gop_bytes { "exact" } else { "SIZE-MISMATCH" },
        gop_vram_offset, total_bytes, gop_vram_offset, gop_bytes);

    // SHUTRESTORE (R19) — the `gop-overlap` detector, restored behind its own knob. This is the
    // probe that FOUND the confound of the whole s18–s26 campaign (a pattern painted into the
    // surface the firmware was already scanning proves nothing about the latch), and the commit
    // that replaced it with the `cover=` line above deleted it rather than keeping it. The
    // cover check answers a different question — extent, not intersection — so the two are not
    // substitutes: point the rung at a surf2 that is NOT the GOP base and `gop-overlap` is the
    // only line that can say the photo is void. Default OFF => neither the call nor the detector
    // exists in an unarmed build.
    #[cfg(feature = "nvidia-kepler-gopoverlap")]
    gop_overlap_probe(gop_vram_offset, surf2_bytes, gop_vram_offset, gop_bytes);

    // 1.12 s hold (standing length — Peter's camera calibration, s21)
    // Predictions with hold off: kepler=1521 -> ~400 ms, gui=3408 -> ~2290 ms — which would be the largest single boot win left on the machine.
    #[cfg(feature = "nvidia-kepler-kdisp-hold")]
    {
        serial_println!(":: kdisp: fb-draw hold begin (photo A — full panel calibration) ::");
        for t in 1..=5 {
            for _ in 0..60_000_000 { core::hint::spin_loop(); }
            serial_println!(":: kdisp: fb-draw hold t={}/5 (1.12s total) ::", t);
            // Dump on the FIRST and LAST tick
            if t == 1 || t == 5 {
                serial_println!(":: kdisp: fb-draw reg-dump t={} ptr={:08X} ptr_hi={:08X} size={:08X} store={:08X} fmt={:08X} ::",
                    t,
                    mmio_read(bar0, 0x640460),
                    mmio_read(bar0, 0x640464),
                    mmio_read(bar0, 0x640468),
                    mmio_read(bar0, 0x64046C),
                    mmio_read(bar0, 0x640470));
                serial_println!(":: kdisp: fb-draw reg-dump t={} armed={:08X} shadow={:08X} ::",
                    t, mmio_read(bar0, armed_reg), mmio_read(bar0, shadow_reg));
                for off in (0x4B8..=0x4C8).step_by(4) {
                    serial_println!(":: kdisp: fb-draw reg-dump off={:03X} val={:08X} ::", off, mmio_read(bar0, 0x640000 + off));
                }
                // Which head is actually live: the one whose vline/vblank advances.
                for h in 0..4usize {
                    let vert = mmio_read(bar0, 0x610000 + 0x6000 + h * 0x800 + 0x340);
                    serial_println!(":: kdisp: fb-draw head-stat t={} h={} vert={:08X} ::", t, h, vert);
                }
            }
        }
        serial_println!(":: kdisp: fb-draw hold end ::");
    }

    #[cfg(feature = "nvidia-kepler-kdisp-hold")]
    let hold_state = "ON";
    #[cfg(not(feature = "nvidia-kepler-kdisp-hold"))]
    let hold_state = "OFF";

    kdisp_phase!("nvidia_kepler_kdisp_hold");
    serial_println!(":: kdisp: inner phase kdisp_hold cfg_hold={} ::", hold_state);

    // Pull 20: Draw console-like glyph blocks using the true 16384 pitch
    for y in 64..72 {
        let row_base = y * pitch_bytes;
        for x in 0..(pitch_bytes / 4) {
            if (x >= 64 && x < 72) || (x >= 80 && x < 88) || (x >= 96 && x < 104) {
                let target_byte_addr = row_base + (x * 4);
                let target_ptr = dst.add(target_byte_addr as usize) as *mut u32;
                core::ptr::write_volatile(target_ptr, 0xFFFFFFFF);
            }
        }
    }
    serial_println!(":: kdisp: fbcon-probe drawn rows=8 ::");

    serial_println!(":: kdisp: fb-draw done ::");
    kdisp_phase!("glyph_draw");

    // CONSOLE-ON-PANEL seam. The calibration pattern above has been drawn, held for its 5 s photo
    // window and probed; the surface is now free. Hand it to the kernel console: fbcon clears the
    // pattern, switches to a legible glyph cell and starts mirroring serial output onto the panel
    // (see `fbcon::panel_console_resume` for why it was painting nothing before). Everything above
    // this line — the draw, the hold, the register dumps — is untouched.
    let repainted = crate::video::fbcon::panel_console_resume();
    serial_println!(":: kdisp: console-repaint rows={} ::", repainted);
    kdisp_phase!("panel_console_resume"); #[cfg(all(feature = "nvidia-kepler", feature = "beam"))] beam_probe(bar0); #[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))] kdhead_probe(bar0, gop_vram_offset, expected_width, expected_height, fbcon_row_bytes); // BEAMX86 (rmbp A5) — the beam source's ONE call site, folded onto this line so the knob-off image cannot shift. Strictly AFTER the console resume (the head is repointed, the pattern cleared, the surface settled, so the raster this samples is the one presents will be ordered against) and strictly BEFORE the compositor activation below, so the first window present is already bracketed. Read-only and bounded: 4 heads x 45 ms = 180 ms, every head sampled even after one ARMS, because a NONE that names only the chosen head is not diagnosable from a flight log and the per-head census is what makes it so. Negligible beside the 1.12 s x5 `fb-draw hold` this same function already spends. ── KDHEAD (register §1 rung KD14) rides the SAME line, for the same reason and in the same shape: its call sits immediately after beam_probe's because the control bracket it scores every head against IS the census beam_probe just published — the same sample, never a second one — and folding it here means a build without `nvidia-kepler-kdhead` keeps this function's line numbering byte-for-byte. Read-only and bounded: 5 candidate blocks x 4 heads x <=4 words, read twice with one settle spin per block; writes=0. Without `beam` the rung still runs its stride census and prints `bracket=absent`, and every decode is withheld — see the tail block for why a bracketless capture may never be read as a statement about a head.

    // WC-X86 seam. Strictly AFTER the console resume above, and for the same reason the console
    // resume is strictly after the calibration draw: this is the first line at which the panel is
    // settled — the scan-out has been repointed, the pattern cleared, the console re-homed on the
    // real surface. Activating the compositor any earlier would put windows on a surface the
    // takeover is about to repoint, and the takeover would win silently. Knob-gated (`UNAOS_WC=1`),
    // so this call site does not exist in a default build.
    #[cfg(feature = "wc")]
    crate::video::desktop_uefi::activate();

    #[cfg(feature = "wc")]
    let desktop_uefi_state = "ON";
    #[cfg(not(feature = "wc"))]
    let desktop_uefi_state = "OFF";

    kdisp_phase!("wcx_activate");
    serial_println!(":: kdisp: inner phase wcx_activate cfg_wc={} ::", desktop_uefi_state);

    let _ = t_last;
    // Completed fb-draw cycle: return the gop pointer so the late recap
    // (kepler.rs, printed inside the FTDI-ring window) can prove this leg ran.
    Some(gop_vram_offset)
}

// ══ SHUTRESTORE (R19) — the display rungs whose code had been deleted ════════════════════════════
//
// Restored from our OWN git history, unchanged in substance, into today's file structure. Each one
// lives behind a DEFAULT-OFF feature of its own inside the existing `nvidia-kepler` cfg region, so
// an unarmed build is byte-identical by construction and no rung reaches the boot path unasked.
// Witness tokens are VERBATIM — `repoint`, `asm-stuck`, `armed-followed`, `lin-step`, `bwpg-step`,
// `gop-overlap` — because the register, the specs and every past capture key on those exact strings.
//
// | rung | knob | feature | restored from |
// | KD6 `repoint` 0x6101E0        | UNAOS_KEPLER_REPOINT      | nvidia-kepler-repoint      | 896faee0 |
// | KD7 latch arm + UPDATE        | UNAOS_KEPLER_LATCH_ARM    | nvidia-kepler-latcharm     | bfeedd94 |
// | display parameter ladders     | UNAOS_KEPLER_PITCH_LADDER | nvidia-kepler-pitchladder  | eee60395 / 04b494be |
// | `gop-overlap` detector        | UNAOS_KEPLER_GOP_OVERLAP  | nvidia-kepler-gopoverlap   | bfeedd94 |

/// KD6 — the pull-5 repoint-the-surface experiment (0x6101E0 only), restored from 896faee0.
///
/// Refuted at s15 with the `rb-stuck` verdict; the deleting commit ("kepler-display pull 7 assembly
/// write and UPDATE latch") gave no reason beyond having moved on, so R19 says KEEP. It matters now
/// because 0x6101E0 is known to be the ARMED-STATE WITNESS: a later arc wants to read it, and the
/// one piece of code that ever wrote it and put it back was gone.
///
/// Fully reversible: the original value is read first, restored last, and the restore is READ BACK
/// (a restore that is written but never read is a success echo that cannot fail).
#[cfg(feature = "nvidia-kepler-repoint")]
unsafe fn repoint_surface(bar0: usize, head: usize, gop_vram_offset: usize) {
    let repoint_reg = regs::NV_PDISPLAY_BASE + 0x01E0; // 0x6101E0
    let orig_ptr = mmio_read(bar0, repoint_reg);
    let hs_base = regs::NV_PDISPLAY_BASE + 0x6000 + (head * 0x800);
    let pre_vert = mmio_read(bar0, hs_base + 0x340);
    let pre_horz = mmio_read(bar0, hs_base + 0x344);
    serial_println!(":: kdisp: repoint pre 6101E0={:08X} stat vert={:08X} horz={:08X} ::", orig_ptr, pre_vert, pre_horz);

    if orig_ptr == 0xFFFFFFFF || (orig_ptr & 0xFFF00000) == 0xBAD00000 {
        serial_println!(":: kdisp: repoint ABSENT/POISON rb={:08X} — no write attempted ::", orig_ptr);
        return;
    }

    // The rung's write. `gop_vram_offset >> 8` is the same 256-byte-granular surface pointer the
    // s15 run used (it hard-coded 0x00016000, the then-current GOP offset); deriving it keeps the
    // experiment pointed at a real surface on a machine whose GOP has moved.
    let new_ptr = (gop_vram_offset >> 8) as u32;
    mmio_write(bar0, repoint_reg, new_ptr);
    let rb = mmio_read(bar0, repoint_reg);
    serial_println!(":: kdisp: repoint wrote={:08X} rb={:08X} ::", new_ptr, rb);

    // Bounded panel window (~5 s) — the camera length Peter calibrated at s21.
    for t in 1..=5 {
        for _ in 0..60_000_000 { core::hint::spin_loop(); }
        let vert = mmio_read(bar0, hs_base + 0x340);
        let horz = mmio_read(bar0, hs_base + 0x344);
        serial_println!(":: kdisp: repoint hold t={}s stat vert={:08X} horz={:08X} ::", t, vert, horz);
    }

    mmio_write(bar0, repoint_reg, orig_ptr);
    let rb_restored = mmio_read(bar0, repoint_reg);
    serial_println!(":: kdisp: repoint restored rb={:08X} ::", rb_restored);
}

/// KD7 — the EVO assembly write + UPDATE latch, restored from bfeedd94.
///
/// SHUTOUT-REGISTER §7 is explicit that this rung is BLOCKED ON §2 (no pushbuffer), not refuted:
/// "when a pushbuffer exists this is the first thing to re-run". Its write half was the one piece
/// of the apparatus that vanished — today's tree still declares `update_reg` and never uses it.
///
/// One deliberate difference from the s28 original, stated rather than hidden: the original
/// `return None`d out of `takeover_display` when the assembly register refused the write. A rung
/// must not change the takeover's contract, so this restore LOGS the same verdict and returns
/// normally; the caller's control flow is identical with the knob on or off.
#[cfg(feature = "nvidia-kepler-latcharm")]
unsafe fn latch_arm_update(
    bar0: usize,
    asm_reg: usize,
    armed_reg: usize,
    shadow_reg: usize,
    update_reg: usize,
    pre_asm: u32,
) {
    let new_ptr = pre_asm;

    // Step 2: Latch Sequence
    mmio_write(bar0, asm_reg, new_ptr);
    let rb_asm = mmio_read(bar0, asm_reg);

    if rb_asm != new_ptr {
        serial_println!(":: kdisp: latch skip — asm rb={:08X} want={:08X} ::", rb_asm, new_ptr);
        let final_asm = mmio_read(bar0, asm_reg);
        let final_armed = mmio_read(bar0, armed_reg);
        let final_shadow = mmio_read(bar0, shadow_reg);
        serial_println!(":: kdisp: latch restored asm={:08X} armed={:08X} shadow={:08X} ::", final_asm, final_armed, final_shadow);
        serial_println!(":: kdisp: latch verdict asm-stuck=n armed-followed=n ::");
        return;
    }

    mmio_write(bar0, update_reg, 0x00000000);

    // 5 s hold (standing length — Peter's camera calibration, s21)
    serial_println!(":: kdisp: pm-step hold begin (photo B — post-latch) ::");
    for t in 1..=5 {
        for _ in 0..60_000_000 { core::hint::spin_loop(); }
        serial_println!(":: kdisp: pm-step hold t={}s ::", t);
        // Dump on the FIRST and LAST tick: a latch that reverts mid-hold is
        // invisible to a single sample.
        if t == 1 || t == 5 {
            serial_println!(":: kdisp: pm-step reg-dump t={} ptr={:08X} ptr_hi={:08X} size={:08X} store={:08X} fmt={:08X} ::",
                t,
                mmio_read(bar0, 0x640460),
                mmio_read(bar0, 0x640464),
                mmio_read(bar0, 0x640468),
                mmio_read(bar0, 0x64046C),
                mmio_read(bar0, 0x640470));
            // Armed/shadow readouts separate "armed a truncated value" from
            // "UPDATE never propagated" — currently byte-identical states.
            serial_println!(":: kdisp: pm-step reg-dump t={} armed={:08X} shadow={:08X} ::",
                t, mmio_read(bar0, armed_reg), mmio_read(bar0, shadow_reg));
            for off in (0x4B8..=0x4C8).step_by(4) {
                serial_println!(":: kdisp: pm-step reg-dump off={:03X} val={:08X} ::", off, mmio_read(bar0, 0x640000 + off));
            }
            // Which head is actually live: the one whose vline/vblank advances.
            for h in 0..4usize {
                let vert = mmio_read(bar0, 0x610000 + 0x6000 + h * 0x800 + 0x340);
                serial_println!(":: kdisp: pm-step head-stat t={} h={} vert={:08X} ::", t, h, vert);
            }
        }
    }
    serial_println!(":: kdisp: pm-step hold end ::");

    // Step 3: Restore
    mmio_write(bar0, asm_reg, pre_asm);
    mmio_write(bar0, update_reg, 0x00000000);

    // 1 s recovery gap
    for _ in 0..15_000_000 { core::hint::spin_loop(); }
    serial_println!(":: kdisp: pm-step done ::");

    serial_println!(":: kdisp: latch verdict asm-stuck=y ::");
}

/// The display parameter ladders — `lin-step` (pull 17, eee60395) and `bwpg-step` (pull 14,
/// 04b494be), restored together because they are one experiment in two coordinate systems: does
/// the head read the surface LINEAR at pitch 0x4000, or block-linear at some (block-width,
/// pitch-in-gobs) pair?
///
/// SHUTOUT-REGISTER §7: "superseded by KD8, and correctly so — but 'superseded' is a different word
/// from 'deleted'." KD8 answers the question for the surface the firmware handed us; it says
/// nothing about a surface we allocate ourselves, which is what the copy-engine arc will need.
#[cfg(feature = "nvidia-kepler-pitchladder")]
unsafe fn pitch_ladders(
    bar0: usize,
    bar1: usize,
    asm_reg: usize,
    armed_reg: usize,
    shadow_reg: usize,
    update_reg: usize,
    pre_asm: u32,
    expected_height: u32,
) {
    // The scratch surface both ladders paint into — deliberately NOT the GOP base, which is the
    // whole point of the `gop-overlap` detector above.
    let surf2_offset: usize = 0x1600000;
    let dst = (bar1 + surf2_offset) as *mut u8;
    let new_ptr = (surf2_offset >> 8) as u32;

    let latch_and_hold = |label: &str, total_bytes: usize| -> bool {
        mmio_write(bar0, asm_reg, new_ptr);
        let rb_asm = mmio_read(bar0, asm_reg);
        if rb_asm != new_ptr {
            serial_println!(":: kdisp: latch skip — asm rb unchanged ::");
            let final_asm = mmio_read(bar0, asm_reg);
            let final_armed = mmio_read(bar0, armed_reg);
            let final_shadow = mmio_read(bar0, shadow_reg);
            serial_println!(":: kdisp: latch restored asm={:08X} armed={:08X} shadow={:08X} ::", final_asm, final_armed, final_shadow);
            serial_println!(":: kdisp: latch verdict asm-stuck=n armed-followed=n ::");
            return false;
        }
        mmio_write(bar0, update_reg, 0x00000000);
        // 5 s per hold — the bench needs camera time between cycles (Peter, s21 prep).
        for t in 1..=5 {
            for _ in 0..60_000_000 { core::hint::spin_loop(); }
            serial_println!(":: kdisp: {} hold t={}s ::", label, t);
        }
        // Restore, then a 1 s recovery gap.
        mmio_write(bar0, asm_reg, pre_asm);
        mmio_write(bar0, update_reg, 0x00000000);
        for _ in 0..15_000_000 { core::hint::spin_loop(); }
        serial_println!(":: kdisp: {} done bytes={:08X} ::", label, total_bytes);
        true
    };

    // ── Rung A: `lin-step` — LINEAR fill at pitch 0x4000 (pull 17) ──────────────────────────
    let pitch_bytes: usize = 16384;
    let total_bytes = expected_height as usize * pitch_bytes;
    for y in 0..expected_height as usize {
        let block_color: u32 = match (y / 64) % 8 {
            0 => 0xFFFF0000, // RED
            1 => 0xFF00FF00, // GREEN
            2 => 0xFF0000FF, // BLUE
            3 => 0xFFFFFF00, // YELLOW
            4 => 0xFF00FFFF, // CYAN
            5 => 0xFFFF00FF, // MAGENTA
            6 => 0xFFFFFFFF, // WHITE
            _ => 0xFF404040, // GRAY
        };
        let row_color = if y % 64 == 0 { 0xFF000000u32 } else { block_color };
        let row_base = y * pitch_bytes;
        for x in 0..(pitch_bytes / 4) {
            let final_color: u32 = if x >= 2880 {
                0xFF000000 // BLACK padding
            } else if x < 256 {
                0xFFFFFFFF // WHITE left-edge ruler
            } else if x < 264 {
                0xFF000000 // BLACK spacer
            } else {
                row_color
            };
            let target_ptr = dst.add(row_base + (x * 4)) as *mut u32;
            core::ptr::write_volatile(target_ptr, final_color);
        }
    }
    serial_println!(":: kdisp: lin-step pitch=4000 fill done bytes={:08X} ::", total_bytes);
    latch_and_hold("lin-step pitch=4000", total_bytes);

    // ── Rung B: `bwpg-step` — block-linear, the block-width vs pitch-in-gobs matrix (pull 14) ──
    let gob_width_bytes: usize = 64;
    let gob_height: usize = 8;
    let gob_size_bytes: usize = 512;
    let bh: usize = 4;
    let cycles: [(usize, usize); 4] = [(2, 192), (2, 256), (4, 192), (4, 256)];

    for &(bw, pg) in cycles.iter() {
        let blocks_per_row = pg / bw;
        let padded_width_px = pg * 16;
        let gob_rows = (expected_height as usize + gob_height - 1) / gob_height;
        let num_block_rows = (gob_rows + bh - 1) / bh;
        let total_bytes = num_block_rows * bw * bh * blocks_per_row * gob_size_bytes;

        for y in 0..expected_height as usize {
            let block_color: u32 = match (y / 64) % 8 {
                0 => 0xFFFF0000,
                1 => 0xFF00FF00,
                2 => 0xFF0000FF,
                3 => 0xFFFFFF00,
                4 => 0xFF00FFFF,
                5 => 0xFFFF00FF,
                6 => 0xFFFFFFFF,
                _ => 0xFF404040,
            };
            let row_color = if y % 64 == 0 { 0xFF000000u32 } else { block_color };

            let gob_y = y / gob_height;
            let inner_y = y % gob_height;
            let blk_y = gob_y / bh;
            let gob_inner_y = gob_y % bh;

            for x in 0..padded_width_px {
                let final_color: u32 = if x >= 2880 {
                    0xFF000000
                } else if x < 256 {
                    0xFFFFFFFF
                } else if x < 264 {
                    0xFF000000
                } else {
                    row_color
                };

                let px_byte_x = x * 4;
                let gob_x = px_byte_x / gob_width_bytes;
                let inner_x = px_byte_x % gob_width_bytes;

                let blk_col = gob_x / bw;
                let gob_inner_x = gob_x % bw;

                let blk_index = (blk_y * blocks_per_row) + blk_col;
                let gob_inner_index = (gob_inner_y * bw) + gob_inner_x;

                let target_byte_addr = (blk_index * bw * bh * gob_size_bytes)
                                     + (gob_inner_index * gob_size_bytes)
                                     + (inner_y * gob_width_bytes)
                                     + inner_x;

                let target_ptr = dst.add(target_byte_addr) as *mut u32;
                core::ptr::write_volatile(target_ptr, final_color);
            }
        }
        serial_println!(":: kdisp: bwpg-step bw={} bh=4 pg={} fill done bytes={:08X} ::", bw, pg, total_bytes);
        // The hold label carries the cycle so a capture can tell the four apart.
        mmio_write(bar0, asm_reg, new_ptr);
        let rb_asm = mmio_read(bar0, asm_reg);
        if rb_asm != new_ptr {
            serial_println!(":: kdisp: latch skip — asm rb unchanged ::");
            let final_asm = mmio_read(bar0, asm_reg);
            let final_armed = mmio_read(bar0, armed_reg);
            let final_shadow = mmio_read(bar0, shadow_reg);
            serial_println!(":: kdisp: latch restored asm={:08X} armed={:08X} shadow={:08X} ::", final_asm, final_armed, final_shadow);
            serial_println!(":: kdisp: latch verdict asm-stuck=n armed-followed=n ::");
            return;
        }
        mmio_write(bar0, update_reg, 0x00000000);
        for t in 1..=5 {
            for _ in 0..60_000_000 { core::hint::spin_loop(); }
            serial_println!(":: kdisp: bwpg-step bw={} bh=4 pg={} hold t={}s ::", bw, pg, t);
        }
        mmio_write(bar0, asm_reg, pre_asm);
        mmio_write(bar0, update_reg, 0x00000000);
        for _ in 0..15_000_000 { core::hint::spin_loop(); }
        serial_println!(":: kdisp: bwpg-step bw={} bh=4 pg={} done ::", bw, pg);
    }

    serial_println!(":: kdisp: latch verdict asm-stuck=y ::");
}

/// The `gop-overlap` detector (restored from bfeedd94) — the probe that FOUND the confound of the
/// whole s18–s26 campaign.
///
/// If the scratch surface we paint intersects the firmware's GOP framebuffer, a photo of our
/// pattern proves nothing about the latch: we simply painted into the surface already being
/// scanned. It LOGS and never aborts — an abort would kill the sitting, and a void result that is
/// named is worth more than a boot that stopped.
#[cfg(feature = "nvidia-kepler-gopoverlap")]
fn gop_overlap_probe(surf2_offset: usize, surf2_bytes: usize, gop_offset: usize, gop_bytes: usize) {
    let overlap = surf2_offset < gop_offset + gop_bytes && gop_offset < surf2_offset + surf2_bytes;
    serial_println!(":: kdisp: fb-draw gop-overlap={} surf2={:08X}+{:08X} gop={:08X}+{:08X} ::",
        if overlap { "YES-RESULT-VOID" } else { "no" },
        surf2_offset, surf2_bytes, gop_offset, gop_bytes);
}

/// Returns false for zero, 0xFFFFFFFF, and the 0xBAD0xxxx pattern that our
/// BAR0 reads return when the target register is unmapped.
#[inline]
fn is_live(val: u32) -> bool {
    val != 0 && val != 0xFFFFFFFF && (val & 0xFFF00000) != 0xBAD00000
}

// ── BEAMX86 (rmbp A5) — the x86 beam source ───────────────────────────────────────────────────────
//
// THE DEFECT. `video/beam.rs` is the tearing FIX and it is already written: every panel present is
// bracketed against the raster position, and `[wc-h]/[wc-k]/[strip] torn=` becomes the OBSERVED beam
// crossing instead of the duration predicate that read 0 while the panel tore. The whole mechanism
// hangs off ONE arch hook, `crate::arch::scanout_beam() -> Option<(vline, vtotal)>`, and on x86 that
// hook was a constant `None` — so on the rMBP the bracket folded to a no-op and the shell window
// tore under `storm` (`[wc-h] win=2 torn=111 banded=13085`, flight 6 boot 1). The Orin answers the
// same hook from `display_tegra::beam_probe`; this is its x86 twin, and it is deliberately the SAME
// SHAPE — probe once at boot, validate BEHAVIOURALLY, publish through a single gate word, and read
// one register per call at runtime.
//
// THE REGISTER, and its cleanroom provenance. `HEAD_STAT` — envytools/rnndb/display/g80_pdisplay.xml
// line 647: offset 0x6000, stride 0x800, length 4 (GK104-). `+0x340` is VERT, `vline[15:0]` and
// `vblank_count[31:16]`; `+0x344` is HORZ. The read-only decode above (`:: kdisp: head[h] stat`)
// already reads exactly this word on all four heads, and the `fb-draw head-stat` dump at :436 exists
// because the LIVE head is the one whose vline/vblank ADVANCES — which is the test this probe
// automates. Register names and offsets from rnndb only; no nouveau code was read or transcribed.
//
// WHERE `vtotal` COMES FROM, and why it is SAMPLED. rnndb cites no lines-per-frame register in this
// bank, and the only raster-shaped word this file knows about is a VALUE match in the known-value
// scan above (`0x07380BAF | 0x0BAF0738 => "raster"`) at an offset that is discovered at runtime, not
// cited — reading it would be a blind read from an uncited address, which this module's standing
// rules forbid. So `vtotal` is MEASURED the way the Orin's probe measures it: sample VERT across at
// least two frames and take `max(vline) + 1`. The head's EVO `SIZE` readback is recorded on the same
// witness line so the flight can cross-check the two (they must agree to within the vblank rows —
// SIZE is the ACTIVE raster, `vtotal` the total, so `vtotal >= size_half` is the expected relation,
// never equality). A zero-compare is never a verdict: the probe ARMS only if the line counter was
// seen to ADVANCE and `vblank_count` was seen to CHANGE, so two identical samples give NONE, not
// `vtotal = 1`.
//
// KNOB-OFF. Every item below is `all(nvidia-kepler, beam)`-gated and APPENDED AT THE FILE TAIL, so
// neither `./arroyo knoboff beam` nor `./arroyo knoboff nvidia-kepler` can see a line shift from it;
// the single call site is folded onto an existing statement line inside `takeover_display`.

/// BEAMX86 — absolute VA of the LIVE head's `HEAD_STAT.VERT` word, or 0 until (unless) the probe
/// ARMED one this boot. This is the ONLY gate `scanout_beam` needs, and it is published LAST.
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
static BEAM_VERT_VA: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// BEAMX86 — lines per frame as the probe measured it (the highest vline seen, plus one).
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
static BEAM_VTOTAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// BEAMX86 — the probe runs ONCE per boot, verdict or no verdict. A second call is a no-op and
/// prints nothing (the witness line is the single-shot record of what this boot found).
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
static BEAM_PROBED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// BEAMX86 — `HEAD_STAT` base, PDISPLAY-relative (g80_pdisplay.xml:647).
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
const BEAM_OFF_HEAD_STAT: usize = 0x6000;
/// BEAMX86 — per-head stride of the `HEAD_STAT` bank.
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
const BEAM_HEAD_STRIDE: usize = 0x800;
/// BEAMX86 — `VERT` within a head's `HEAD_STAT`: `vline[15:0]`, `vblank_count[31:16]`.
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
const BEAM_OFF_VERT: usize = 0x340;
/// BEAMX86 — GK104 head count, the same four this file's read-only decode already walks.
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
const BEAM_HEADS: usize = 4;
/// BEAMX86 — sampling window per head, in ms: 2.7 frames at 60 Hz, so a raster wraps at least twice.
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
const BEAM_SAMPLE_MS: u64 = 45;
/// BEAMX86 — hard iteration cap per head. `crate::arch::ms()` is the APIC tick; if it were ever
/// stopped when this ran, a pure time-budget loop would spin forever inside the takeover. The cap
/// makes the probe BOUNDED by construction and the `samples=` field says which bound ended it.
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
const BEAM_SPIN_CAP: u32 = 8_000_000;

/// BEAMX86 — find and ARM the Kepler head's raster-position register. Called ONCE from
/// `takeover_display`, after `panel_console_resume` (the head is repointed, the pattern cleared and
/// the console re-homed, so the head is known-good and scanning) and BEFORE the compositor is
/// activated, so the very first present is already bracketed. READ-ONLY: every access below is
/// `mmio_read`; there is no `mmio_write` in this function. Prints exactly one `:: BEAMX86:` line.
///
/// # Safety
/// `bar0` must be the mapped BAR0 base this module's other reads already use.
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
pub unsafe fn beam_probe(bar0: usize) {
    use core::sync::atomic::Ordering;
    if BEAM_PROBED.swap(true, Ordering::AcqRel) {
        return;
    }

    // The panel's row count, for the sanity test below. `None` (no GOP info) drops the test rather
    // than the probe: a raster that advances and wraps is still a raster.
    let panel_h = crate::video::fbcon::current_info()
        .map(|i| i.height as u32)
        .unwrap_or(0);

    // Per-head census, kept so the ONE witness line can say what every head did — a NONE that names
    // only the chosen head is not diagnosable from a flight log.
    let mut c_adv = [0u32; BEAM_HEADS];
    let mut c_vbd = [0u32; BEAM_HEADS];
    let mut chosen: Option<(usize, u32, u32, u32, u32, u32, u32)> = None; // head, vtotal, samples, vbd, max, adv, evo_size

    for head in 0..BEAM_HEADS {
        let vert_off = regs::NV_PDISPLAY_BASE + BEAM_OFF_HEAD_STAT + head * BEAM_HEAD_STRIDE + BEAM_OFF_VERT;
        let first = mmio_read(bar0, vert_off);
        // NOT `is_live`: a VERT of literal 0 is a legal reading (line 0 of frame 0). Only the
        // unmapped patterns disqualify a head before it is sampled.
        if first == 0xFFFF_FFFF || (first & 0xFFF0_0000) == 0xBAD0_0000 {
            continue;
        }
        let mut prev = first & 0xFFFF;
        let mut max = prev;
        let mut vb_last = (first >> 16) & 0xFFFF;
        let (mut adv, mut vbd, mut samples, mut spins) = (0u32, 0u32, 0u32, 0u32);
        let t0 = crate::arch::ms();
        loop {
            if crate::arch::ms().wrapping_sub(t0) > BEAM_SAMPLE_MS {
                break;
            }
            spins = spins.saturating_add(1);
            if spins >= BEAM_SPIN_CAP {
                break;
            }
            let w = mmio_read(bar0, vert_off);
            samples = samples.saturating_add(1);
            let v = w & 0xFFFF;
            let vb = (w >> 16) & 0xFFFF;
            if vb != vb_last {
                vbd = vbd.saturating_add(1);
                vb_last = vb;
            }
            if v > prev {
                adv = adv.saturating_add(1);
            }
            if v > max {
                max = v;
            }
            prev = v;
            core::hint::spin_loop();
        }
        c_adv[head] = adv;
        c_vbd[head] = vbd;
        if chosen.is_some() {
            continue;
        }
        let vtotal = max.saturating_add(1);
        // THE VERDICT TEST. The line counter must have been seen to climb (`adv`), the frame counter
        // must have been seen to tick at least twice (`vbd` — two frames is the brief's floor and is
        // what makes `max` a whole-frame maximum rather than a partial sweep), the derived total must
        // be a real number of lines, and — when a panel height is known — it must cover the panel and
        // not exceed four times it. Any one of these failing leaves this head unarmed.
        if adv > 0
            && vbd >= 2
            && vtotal > 1
            && (panel_h == 0 || (vtotal >= panel_h && max < panel_h.saturating_mul(4)))
        {
            // EVO core SIZE for this head — the same candidate-A word the read-only decode above
            // reads (`evo_base + 0x8`), recorded for the register-vs-sampled cross-check.
            let evo_size = mmio_read(bar0, regs::NV_PDISPLAY_BASE + 0x400 + head * 0x300 + 0x60 + 0x8);
            chosen = Some((head, vtotal, samples, vbd, max, adv, evo_size));
        }
    }

    // KDHEAD (register §1 rung KD14) borrows THIS census rather than taking a second 4x45 ms sample:
    // its decode must be bracketed by the same reading the beam gate armed on, or a disagreement
    // between the two would be unattributable. One cfg'd statement, stores only, no device access.
    #[cfg(feature = "nvidia-kepler-kdhead")]
    kdhead_publish_census(&c_adv, &c_vbd);

    if let Some((head, vtotal, samples, vbd, max, adv, evo_size)) = chosen {
        BEAM_VTOTAL.store(vtotal, Ordering::Relaxed);
        // Published LAST, with Release: it is the gate `scanout_beam` reads, and a reader that sees
        // a non-zero address must also see the vtotal above.
        BEAM_VERT_VA.store(bar0 + regs::NV_PDISPLAY_BASE + BEAM_OFF_HEAD_STAT + head * BEAM_HEAD_STRIDE + BEAM_OFF_VERT, Ordering::Release);
        serial_println!(
            ":: BEAMX86: head={} vtotal={} samples={} vblank_delta={} -> ARMED :: max_vline={} adv={} panel_h={} evo_size={:08X} size_hi={} size_lo={} sample_ms={} census_adv=[{},{},{},{}] census_vbd=[{},{},{},{}] — vtotal is SAMPLED (max vline + 1 over {} ms, rnndb cites no lines-per-frame register in HEAD_STAT); evo_size is the ACTIVE raster readback, so vtotal >= the matching half by the vblank rows is the expected relation, never equality. arch::scanout_beam() now answers and every panel present is bracketed. READ-ONLY: writes=0 ::",
            head, vtotal, samples, vbd, max, adv, panel_h, evo_size,
            (evo_size >> 16) & 0xFFFF, evo_size & 0xFFFF, BEAM_SAMPLE_MS,
            c_adv[0], c_adv[1], c_adv[2], c_adv[3],
            c_vbd[0], c_vbd[1], c_vbd[2], c_vbd[3],
            BEAM_SAMPLE_MS,
        );
    } else {
        serial_println!(
            ":: BEAMX86: head=none vtotal=0 samples=0 vblank_delta=0 -> NONE :: panel_h={} sample_ms={} census_adv=[{},{},{},{}] census_vbd=[{},{},{},{}] — no head's VERT behaved as a raster (the line counter must CLIMB and vblank_count must tick twice inside {} ms; two identical samples is NONE, never vtotal=1). arch::scanout_beam() stays None for this boot, no present is held, and torn= stays the duration predicate. READ-ONLY: writes=0 ::",
            panel_h, BEAM_SAMPLE_MS,
            c_adv[0], c_adv[1], c_adv[2], c_adv[3],
            c_vbd[0], c_vbd[1], c_vbd[2], c_vbd[3],
            BEAM_SAMPLE_MS,
        );
    }
}

/// BEAMX86 — the raster position, `(vline, lines_per_frame)`, or `None` until (unless) `beam_probe`
/// ARMED a head this boot. ONE `read_volatile`, no lock, no print, no allocation: this is called
/// from the compositor's IRQ-masked present path at polling rate.
#[cfg(all(feature = "nvidia-kepler", feature = "beam"))]
#[inline]
pub fn scanout_beam() -> Option<(u32, u32)> {
    use core::sync::atomic::Ordering;
    let va = BEAM_VERT_VA.load(Ordering::Acquire);
    if va == 0 {
        return None;
    }
    let vt = BEAM_VTOTAL.load(Ordering::Relaxed);
    // SAFETY: `va` was computed from the BAR0 base this module's reads already use and published by
    // `beam_probe` only after that very word was read repeatedly and behaved as a raster counter;
    // this is a read.
    let v = (unsafe { core::ptr::read_volatile(va as *const u32) }) & 0xFFFF;
    Some((v.min(vt.saturating_sub(1)), vt))
}



// ── KDHEAD (rmbp, shut-out register §1 rung KD14) — the per-head decode, BRACKETED ────────────────
//
// THE RUNG THE REGISTER ASKED FOR. `SHUTOUT-REGISTER.md` §1 records KD3 `head-raw` as **shut-out**,
// and its "what would change the verdict" names exactly one thing: *"re-run the per-head decode with
// KD4's HEAD_STAT as the bracket and the per-head stride re-derived for the 917D class. The rung
// failed because it had no control read, not because the heads are dead; KD4 proves head 0 scans."*
// This is that rung. It is READ-ONLY (`writes=0`), it runs behind a knob of its own that is DEFAULT
// OFF, and it answers in three parts that are deliberately separable on the wire.
//
// PART 1 — THE CONTROL BRACKET, AND WHY IT IS BORROWED RATHER THAN TAKEN AGAIN. KD3's whole defect
// is that four byte-identical reads were scored as "the heads are dead" with nothing in the same
// capture saying whether any head was scanning at all. KD4 (`head[0] stat underflow=0
// vert=0x0493048A`, **[METAL s11]**) is that missing reading, and BEAMX86 already automates it:
// `beam_probe` samples `HEAD_STAT.VERT` on all four heads for 45 ms each and keeps a per-head census
// of how often the line counter climbed (`adv`) and how often `vblank_count` ticked (`vbd`). Taking
// that sample a SECOND time here would cost another 180 ms inside the takeover AND — worse — would
// be a different sample than the one the beam gate armed on, so a disagreement between the two would
// be unattributable. So BEAMX86 publishes its census and this rung READS it. A head with `adv > 0 &&
// vbd >= 2` is LIVE; anything else is DARK, and a decode read against a DARK head is scored
// `DARK-NOT-SCORED`, never "wrong" — that is R19's rule applied to this rung's own output.
//   ⚠ The bracket therefore needs `UNAOS_BEAM=1` on the same boot. Without it the rung still runs
//   its stride census (part 2, which needs no bracket) and prints `bracket=absent` with every decode
//   line scored `NO-BRACKET-NOT-SCORED`. A capture with no bracket may never be read as a statement
//   about the hardware.
//
// PART 2 — THE STRIDE, RE-DERIVED RATHER THAN ASSUMED, AND THE COUNTER TRAP. Sitting #4 read
// `head-raw addr=00000001 size=078004FE storage=0A0006A8` byte-identical on all four heads at
// `0x616100 + head*0x800` and concluded "the stride is collapsing" (**[METAL s4]**). That inference
// is testable directly: read the SAME word at head 0 and heads 1..3 and ask whether they DIFFER. If
// four heads at a claimed stride hand back one value, the stride does not separate heads for that
// block and every per-head number taken from it is one head's number printed four times.
//   The trap in that test, and the reason this rung reads every block TWICE with a settle between:
//   **a counter defeats it in the wrong direction.** `HEAD_STAT.VERT`/`HORZ` and the frame counter at
//   `+0x314` (**[METAL s13]**) change between two reads microseconds apart, so four reads of ONE
//   collapsed register return four different values and score as `heads_distinct=4/4` — a stride
//   that is wrong reading as a stride that works. So each probe word is read at every head, settled,
//   and read again; a word that MOVED is marked volatile and is EXCLUDED from the distinctness
//   tuple. Only words that held still at every head vote. `stable=` on the block line says how many
//   of the block's probe words survived that filter, and a block with `stable=0` scores
//   `heads_distinct=?` rather than a number.
//
// PART 3 — THE DECODE, ONLY WHERE IT IS EARNED. A block is decoded only if its stride actually
// separated heads AND the bracket says the head is live. The decoded fields are the mode geometry,
// the surface address and the pitch, each sliced the way a source this bench can cite slices it, and
// compared against what the takeover already inherited from the firmware — the GOP framebuffer's
// VRAM offset, its width/height, and `stride*bpp`. `-> AGREE` on the live head pins the decode BY
// OBSERVATION and re-opens KD3; `-> DISAGREE` names the first field that differs, which is a finding
// about the slicing and not about the silicon.
//
// CITATION CLASSES (falcon_microcode_spec.md §0.1) ride every offset in the tables below and are
// printed on the wire beside the block they belong to: **[TREE]** = this tree already reads the
// address; **[METAL sN]** = observed on this bench in sitting N; **[EXT]** = envytools/rnndb names a
// register this bench has also observed; **[UNPINNED]** = external or inferred with no observation
// on THIS part — probe-only, never a basis for a write. This rung writes nothing at all, so no
// UNPINNED claim is ever acted on: it is read, printed, and labelled.
//
// KNOB-OFF. Every item below is `all(nvidia-kepler, nvidia-kepler-kdhead)`-gated and APPENDED AT THE
// FILE TAIL, below BEAMX86's own tail block, so no knob-off line number moves; the single call site
// is folded onto the existing statement line inside `takeover_display` that already carries
// BEAMX86's. The census publication is one cfg'd statement INSIDE `beam_probe`, which does not exist
// in a build without `beam`.

/// KDHEAD — GK104 head count, the same four the read-only decode and BEAMX86 walk.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
const KDHEAD_HEADS: usize = 4;
/// KDHEAD — maximum probe words per block (the table's widest row uses all four).
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
const KDHEAD_PROBES: usize = 4;
/// KDHEAD — settle between the two census passes, so a counter has time to move and be caught.
/// Same magnitude as the `mirror-sp` volatility check this file already uses (`run_recon` pass 2).
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
const KDHEAD_SETTLE_SPINS: u32 = 1_500_000;

/// KDHEAD — the rung runs ONCE per boot. `takeover_display` has a known double-invocation
/// (**[METAL s15]**, "full ladder ran twice"), and two copies of this census in one capture would
/// invite a reader to diff them as if they were a control.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
static KDHEAD_RAN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// KDHEAD — BEAMX86's per-head `adv` census, published by `beam_probe` so this rung can bracket its
/// decode against the SAME sample the beam gate armed on instead of taking a second one.
#[cfg(all(feature = "nvidia-kepler", feature = "beam", feature = "nvidia-kepler-kdhead"))]
static KDHEAD_CENSUS_ADV: [core::sync::atomic::AtomicU32; KDHEAD_HEADS] = [
    core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0),
];
/// KDHEAD — BEAMX86's per-head `vblank_delta` census. See `KDHEAD_CENSUS_ADV`.
#[cfg(all(feature = "nvidia-kepler", feature = "beam", feature = "nvidia-kepler-kdhead"))]
static KDHEAD_CENSUS_VBD: [core::sync::atomic::AtomicU32; KDHEAD_HEADS] = [
    core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0),
];
/// KDHEAD — published LAST, with Release: a reader that sees `true` also sees both arrays above.
/// `false` means `beam_probe` never ran this boot, which is a DIFFERENT condition from "it ran and
/// found nothing" and the bracket line says which.
#[cfg(all(feature = "nvidia-kepler", feature = "beam", feature = "nvidia-kepler-kdhead"))]
static KDHEAD_CENSUS_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// KDHEAD — BEAMX86 hands its per-head census over. Called from the tail of `beam_probe`'s sampling
/// loop, with the arrays that boot's ONE sample produced. Pure stores; no read of the device.
#[cfg(all(feature = "nvidia-kepler", feature = "beam", feature = "nvidia-kepler-kdhead"))]
fn kdhead_publish_census(adv: &[u32; BEAM_HEADS], vbd: &[u32; BEAM_HEADS]) {
    use core::sync::atomic::Ordering;
    for h in 0..KDHEAD_HEADS {
        KDHEAD_CENSUS_ADV[h].store(adv[h], Ordering::Relaxed);
        KDHEAD_CENSUS_VBD[h].store(vbd[h], Ordering::Relaxed);
    }
    KDHEAD_CENSUS_DONE.store(true, Ordering::Release);
}

/// KDHEAD — the control bracket, or `None` when BEAMX86 did not sample this boot.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead", feature = "beam"))]
fn kdhead_bracket() -> Option<([u32; KDHEAD_HEADS], [u32; KDHEAD_HEADS])> {
    use core::sync::atomic::Ordering;
    if !KDHEAD_CENSUS_DONE.load(Ordering::Acquire) {
        return None;
    }
    let mut adv = [0u32; KDHEAD_HEADS];
    let mut vbd = [0u32; KDHEAD_HEADS];
    for h in 0..KDHEAD_HEADS {
        adv[h] = KDHEAD_CENSUS_ADV[h].load(Ordering::Relaxed);
        vbd[h] = KDHEAD_CENSUS_VBD[h].load(Ordering::Relaxed);
    }
    Some((adv, vbd))
}

/// KDHEAD — no BEAMX86 in this build, so there is no bracket and the rung says so rather than
/// inventing one. The decode half is withheld; the stride census still runs.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead", not(feature = "beam")))]
fn kdhead_bracket() -> Option<([u32; KDHEAD_HEADS], [u32; KDHEAD_HEADS])> {
    None
}

/// KDHEAD — one candidate per-head register block: where head 0's record starts, what stride the
/// source claims separates the heads, which words inside the record to score, and the citation class
/// of every one of those numbers.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
struct KdheadBlock {
    /// Short name, printed in `block=`.
    name: &'static str,
    /// BAR0 offset of head 0's record (absolute, the way `mmio_read` takes it).
    base: usize,
    /// Claimed per-head stride.
    stride: usize,
    /// Sub-offsets read at every head. Only the first `nprobe` entries are used.
    probe: [usize; KDHEAD_PROBES],
    /// How many entries of `probe` are live.
    nprobe: usize,
    /// Citation class of the base, the stride and the probe offsets, printed on the block line.
    cite: &'static str,
}

/// KDHEAD — the candidate blocks, every one of them named by the tree or by a sitting. Nothing here
/// is invented: `headstat` and `armed100` are the block KD3 and KD4 both read (the same bank, 0x100
/// apart), `evocore` and `headval` are this file's candidate A and candidate B, and `mirror` is the
/// EVO core-channel method mirror that KD8 decoded on metal.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
const KDHEAD_BLOCKS: [KdheadBlock; 5] = [
    // The bank KD4 proved: `HEAD_STAT`, offset 0x6000 from PDISPLAY, stride 0x800, length 4.
    // Probe words are chosen to be STABLE ones — the counters in this bank (+0x340 VERT, +0x344
    // HORZ, +0x314 the frame counter) are deliberately NOT probed, because four reads of one
    // collapsed counter differ and would score as a working stride. +0x308 is REPORT_UNDERFLOW,
    // read by KD4 every boot; +0x30C/+0x310 are the stable head-0-only config words s12 and s13
    // both saw hold their value across passes; +0x34C is the mode-timing word s13 decoded as
    // vtotal=0x738 | htotal=0xBAF, with head 1 holding a near-reset 0x00050008.
    KdheadBlock {
        name: "headstat", base: 0x61_6000, stride: 0x800,
        probe: [0x308, 0x30C, 0x310, 0x34C], nprobe: 4,
        cite: "base+stride=[EXT g80_pdisplay.xml:647 HEAD_STAT off=0x6000 stride=0x800 len=4 GK104-] and [TREE kepler_display.rs head-stat reads]; +0x308 [METAL s11 KD4]; +0x30C/+0x310 [METAL s12+s13 stable head-0-only config]; +0x34C [METAL s13 vtotal|htotal 0x07380BAF]",
    },
    // The sub-window sitting #4 read as the "ARMED block" and scored byte-identical on four heads.
    // Same bank as `headstat`, 0x100 in; carried as its OWN row because KD3's verdict is recorded
    // against these three offsets at this base and the register's re-run must address them by name.
    KdheadBlock {
        name: "armed100", base: 0x61_6100, stride: 0x800,
        probe: [0x000, 0x008, 0x00C, 0x000], nprobe: 3,
        cite: "base+stride+offsets [METAL s4 head-raw addr=00000001 size=078004FE storage=0A0006A8, byte-identical x4 — the reading this rung re-takes]; the ADDR/SIZE/STORAGE field roles at these offsets are [UNPINNED] (s4: 'addr=0x00000001 is not address-shaped')",
    },
    // Candidate A, this file's own: the EVO core method layout read as if PDISPLAY mirrored it —
    // HEAD at +0x400, stride 0x300, FB_SETTINGS at +0x60, OFFSET_ORIGIN/SIZE/STORAGE at +0x0/+0x8/
    // +0xC. BEAMX86 reads `+0x8` of head h's record for its `evo_size=` cross-check.
    KdheadBlock {
        name: "evocore", base: 0x61_0460, stride: 0x300,
        probe: [0x000, 0x008, 0x00C, 0x000], nprobe: 3,
        cite: "[TREE kepler_display.rs candidate A + BEAMX86 evo_size]; layout [EXT nv_evo.xml HEAD array GF119+, G80_EVO_FB_SETTINGS at +0x60 in NV_EVO_CORE]; that PDISPLAY MMIO MIRRORS those method offsets is [UNPINNED] and was read all-zero on four heads at [METAL s11]",
    },
    // Candidate B, this file's own: the pre-GF119 HEAD_VAL layout. rnndb marks it G80:GF119, so on
    // GK107 the whole block is UNPINNED; s11 read it all-zero on four heads.
    KdheadBlock {
        name: "headval", base: 0x61_0A00, stride: 0x540,
        probe: [0x118, 0x120, 0x128, 0x000], nprobe: 3,
        cite: "[TREE kepler_display.rs candidate B]; [EXT g80_pdisplay.xml:371-408 HEAD_VAL FB_SIZE+0x118 FB_PITCH+0x120 FB_POS+0x128] but marked G80:GF119, so on GK107 base/stride/fields are [UNPINNED]; read all-zero on four heads at [METAL s11]",
    },
    // The EVO core-channel METHOD MIRROR at 0x640000 — the one block on this part whose fields were
    // decoded on metal. s16 found head 0's record at +0x400 (0x640420 = the 0x07380BAF raster
    // totals, 0x640460 = 0x200 = the GOP surface >>8); s25 (KD8 `mirror-sp`) read 0x640468 =
    // 07080B40 (h1800 w2880) and 0x64046C = 01004000 (bit24 LAYOUT=PITCH/LINEAR, pitch 0x4000 =
    // 16384 B/row). The STRIDE between heads is the core-channel method stride 0x300, which is
    // [UNPINNED] here — s16/s25 only ever read head 0's record, and testing it is this rung's job.
    KdheadBlock {
        name: "mirror", base: 0x64_0400, stride: 0x300,
        probe: [0x020, 0x060, 0x068, 0x06C], nprobe: 4,
        cite: "head-0 record +0x20/+0x60 [METAL s16], +0x68/+0x6C [METAL s25 KD8 SET_STORAGE bit24 LAYOUT=1 PITCH(LINEAR) pitch=0x4000]; [TREE kepler_display.rs fb-draw reg-dump reads 0x640460..0x640470]; the per-head stride 0x300 is [UNPINNED] — every sitting read head 0's record only",
    },
];

/// KDHEAD — what a block's probe words decode to, when this bench can cite a slicing for them.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
struct KdheadGeom {
    width: u32,
    height: u32,
    /// Surface address as a VRAM byte offset, when the block holds one.
    surface: Option<u64>,
    /// Row pitch in bytes, when the block holds one.
    pitch: Option<u32>,
    /// How the three above were sliced out of the raw words, with the class of each slicing.
    cite: &'static str,
}

/// KDHEAD — slice a block's probe words into geometry / surface / pitch. `None` means this bench
/// cites NO surface-or-geometry slicing for that block, which is a statement about our sources and
/// is printed as such — never as a failed read.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
fn kdhead_decode(name: &str, w: &[u32; KDHEAD_PROBES]) -> Option<KdheadGeom> {
    match name {
        // s25's decode, field for field, on whichever head's record we are standing in.
        "mirror" => Some(KdheadGeom {
            width: w[2] & 0xFFFF,
            height: (w[2] >> 16) & 0xFFFF,
            surface: Some((w[1] as u64) << 8),
            pitch: Some(w[3] & 0xFFFF),
            cite: "geom=+0x68 lo16 x hi16 [METAL s25 07080B40 = h1800 w2880]; surface=+0x60 <<8 [METAL s16 0x200 = GOP vram_off 0x20000]; pitch=+0x6C & 0xFFFF [METAL s25 01004000 -> 0x4000]; that the field is exactly 16 bits wide is [UNPINNED]",
        }),
        // Candidate A's own slicing, as this file has always read it.
        "evocore" => Some(KdheadGeom {
            width: w[1] & 0xFFFF,
            height: (w[1] >> 16) & 0xFFFF,
            surface: Some((w[0] as u64) << 8),
            pitch: Some(w[2] & 0xFFFF),
            cite: "geom=+0x08 lo16 x hi16, surface=+0x00 <<8, pitch=+0x0C & 0xFFFF — all [UNPINNED]: [EXT nv_evo.xml] gives the METHOD field order, and no sitting has read a non-zero word here to pin it",
        }),
        // s4's own reading of its own three words, kept in s4's shape so the re-run is comparable.
        "armed100" => Some(KdheadGeom {
            width: w[1] & 0xFFFF,
            height: (w[1] >> 16) & 0xFFFF,
            surface: Some((w[0] as u64) << 8),
            pitch: Some(w[2] & 0xFFFF),
            cite: "geom=+0x08 lo16 x hi16 [METAL s4 read 078004FE here and called 0x0780=1920 'display geometry, just sliced wrong']; surface=+0x00 <<8 and pitch=+0x0C & 0xFFFF are [UNPINNED] (s4: addr=00000001 is not address-shaped)",
        }),
        // HEAD_VAL carries a size and a pitch but no scanout address in the three probed words.
        "headval" => Some(KdheadGeom {
            width: w[0] & 0xFFFF,
            height: (w[0] >> 16) & 0xFFFF,
            surface: None,
            pitch: Some(w[1] & 0xFFFF),
            cite: "geom=FB_SIZE lo16 x hi16, pitch=FB_PITCH & 0xFFFF [EXT g80_pdisplay.xml HEAD_VAL] but G80:GF119-marked, so [UNPINNED] on GK107; FB_POS is a position, not a surface address, so surface=n/a by construction",
        }),
        // s13 settled this one, and the answer was negative: the surface address is not exposed in
        // this bank, and +0x34C holds raster TOTALS, not the active geometry. Saying so is the
        // honest output; inventing a slicing would be the KD3 error again.
        _ => None,
    }
}

/// KDHEAD — is this word a read that came back at all? `0xFFFFFFFF` is an unmapped BAR and the
/// `0xBADxxxxx` family is the GK107's nonexistent-PRI-register signature. A literal zero is a
/// LEGAL reading and is never treated as absent here — KD3's error was in the other direction.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
fn kdhead_answered(val: u32) -> bool {
    val != 0xFFFF_FFFF && (val & 0xFFF0_0000) != 0xBAD0_0000
}

/// KDHEAD — re-run KD3's per-head decode with KD4's control bracket and the stride measured instead
/// of assumed. Called ONCE from `takeover_display`, immediately after `beam_probe` so the bracket it
/// borrows is already published, and before the compositor activation.
///
/// **READ-ONLY: every device access below is `mmio_read`; there is no `mmio_write`, no
/// `write_volatile` and no allocation in this function.** It prints `:: KDHEAD:` lines and nothing
/// else changes.
///
/// # Safety
/// `bar0` must be the mapped BAR0 base this module's other reads already use.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
pub unsafe fn kdhead_probe(
    bar0: usize,
    gop_vram_offset: usize,
    gop_w: u32,
    gop_h: u32,
    gop_pitch: u32,
) {
    use core::sync::atomic::Ordering;
    if KDHEAD_RAN.swap(true, Ordering::AcqRel) {
        return;
    }

    // ── The control bracket, borrowed from BEAMX86's one sample ────────────────────────────────
    let bracket = kdhead_bracket();
    let mut live = [false; KDHEAD_HEADS];
    match bracket {
        Some((adv, vbd)) => {
            for h in 0..KDHEAD_HEADS {
                // BEAMX86's own liveness test, verbatim in substance: the line counter must have
                // been seen to CLIMB and the frame counter to tick at least twice inside its 45 ms
                // window. The vtotal sanity BEAMX86 additionally applies is about ARMING a beam
                // source, not about whether a head scans, so it is deliberately not repeated here.
                live[h] = adv[h] > 0 && vbd[h] >= 2;
            }
            serial_println!(
                ":: KDHEAD: bracket source=beamx86-census live=[{},{},{},{}] :: census_adv=[{},{},{},{}] census_vbd=[{},{},{},{}] — KD4's control read ([METAL s11] head[0] stat vert=0x0493048A, heads 1-3 zero), taken ONCE by beam_probe over 4 heads x 45 ms and REUSED here rather than re-sampled, so the bracket and the beam gate cannot disagree. live = adv>0 AND vbd>=2. A head that is not live is DARK, and a decode read against it is scored DARK-NOT-SCORED, never wrong (R19) ::",
                if live[0] { "y" } else { "n" }, if live[1] { "y" } else { "n" },
                if live[2] { "y" } else { "n" }, if live[3] { "y" } else { "n" },
                adv[0], adv[1], adv[2], adv[3], vbd[0], vbd[1], vbd[2], vbd[3],
            );
        }
        None => {
            serial_println!(
                ":: KDHEAD: bracket source=absent live=[?,?,?,?] :: reason=no-beamx86-census-this-boot — either `beam` is not in this build or beam_probe did not run. The stride census below STILL RUNS and needs no bracket; every decode is WITHHELD and scored NO-BRACKET-NOT-SCORED. Nothing in this capture may be read as a statement about a head being dead — that is precisely KD3's shut-out condition. Fly with UNAOS_BEAM=1 ::"
            );
        }
    }

    serial_println!(
        ":: KDHEAD: gop w={} h={} vram_off={:08X} pitch={} :: the takeover's inherited truth, every decode below is compared against exactly these four numbers ([TREE] fbcon::current_info + the BAR1-relative offset this function already computed) ::",
        gop_w, gop_h, gop_vram_offset, gop_pitch
    );

    let mut n_separated = 0u32;
    let mut n_decoded = 0u32;
    let mut n_agree = 0u32;
    let mut n_disagree = 0u32;

    for blk in KDHEAD_BLOCKS.iter() {
        // ── Two passes with a settle, so a counter is caught and disqualified ──────────────────
        // Slots past `nprobe` are never read from the device and carry this module's SENTINEL, so
        // the dumped rows below can never be misread as a probe that returned zero.
        let mut pass0 = [[SENTINEL; KDHEAD_PROBES]; KDHEAD_HEADS];
        let mut pass1 = [[SENTINEL; KDHEAD_PROBES]; KDHEAD_HEADS];
        for h in 0..KDHEAD_HEADS {
            for p in 0..blk.nprobe {
                pass0[h][p] = mmio_read(bar0, blk.base + h * blk.stride + blk.probe[p]);
            }
        }
        for _ in 0..KDHEAD_SETTLE_SPINS {
            core::hint::spin_loop();
        }
        for h in 0..KDHEAD_HEADS {
            for p in 0..blk.nprobe {
                pass1[h][p] = mmio_read(bar0, blk.base + h * blk.stride + blk.probe[p]);
            }
        }

        // A probe word votes on the stride only if it held still at EVERY head.
        let mut stable = [false; KDHEAD_PROBES];
        let mut n_stable = 0usize;
        for p in 0..blk.nprobe {
            let mut held = true;
            for h in 0..KDHEAD_HEADS {
                if pass0[h][p] != pass1[h][p] {
                    held = false;
                }
            }
            stable[p] = held;
            if held {
                n_stable += 1;
            }
        }

        // How many heads ANSWERED at all (a literal zero answers; 0xFFFFFFFF and 0xBADxxxxx do not).
        let mut n_readable = 0u32;
        for h in 0..KDHEAD_HEADS {
            let mut any = false;
            for p in 0..blk.nprobe {
                if kdhead_answered(pass0[h][p]) {
                    any = true;
                }
            }
            if any {
                n_readable += 1;
            }
        }

        // Distinctness over the stable words only.
        let mut distinct = 0u32;
        if n_stable > 0 {
            for h in 0..KDHEAD_HEADS {
                let mut dup = false;
                for g in 0..h {
                    let mut same = true;
                    for p in 0..blk.nprobe {
                        if stable[p] && pass0[h][p] != pass0[g][p] {
                            same = false;
                        }
                    }
                    if same {
                        dup = true;
                    }
                }
                if !dup {
                    distinct += 1;
                }
            }
        }

        for h in 0..KDHEAD_HEADS {
            serial_println!(
                ":: KDHEAD: block={} head={} at=0x{:06X} w=[{:08X},{:08X},{:08X},{:08X}] again=[{:08X},{:08X},{:08X},{:08X}] nprobe={} ::",
                blk.name, h, blk.base + h * blk.stride,
                pass0[h][0], pass0[h][1], pass0[h][2], pass0[h][3],
                pass1[h][0], pass1[h][1], pass1[h][2], pass1[h][3],
                blk.nprobe,
            );
        }

        let separated = n_stable > 0 && distinct >= 2;
        if separated {
            n_separated += 1;
        }
        serial_println!(
            ":: KDHEAD: block={} base=0x{:06X} stride=0x{:X} heads_distinct={}/4 :: stable={}/{} readable={}/4 verdict={} cite={} — heads_distinct counts DISTINCT stable-word tuples; a counter is excluded by the two-pass filter so a collapsed stride cannot masquerade as four different heads, and stable=0 means no word in this block held still and the count is not a stride reading at all ::",
            blk.name, blk.base, blk.stride, distinct,
            n_stable, blk.nprobe, n_readable,
            if n_stable == 0 { "UNSCORABLE-all-words-volatile" }
            else if n_readable == 0 { "UNREADABLE-no-head-answered" }
            else if distinct == 1 { "COLLAPSED-stride-does-not-separate-heads-here (the s4 reading, re-taken)" }
            else { "SEPARATES-heads" },
            blk.cite,
        );

        // ── Part 3: decode, only where the stride earned it and the bracket allows ─────────────
        if !separated {
            serial_println!(
                ":: KDHEAD: block={} decode=skipped reason={} :: a decode taken from a block whose stride does not separate the heads is one head's number printed four times, which is exactly KD3's shut-out condition; the block's code and knob are KEPT and the reading above is the record (R19) ::",
                blk.name,
                if n_stable == 0 { "all-words-volatile" } else if distinct == 1 { "stride-collapsed" } else { "no-head-answered" },
            );
            continue;
        }

        for h in 0..KDHEAD_HEADS {
            let geom = match kdhead_decode(blk.name, &pass0[h]) {
                Some(g) => g,
                None => {
                    if h == 0 {
                        serial_println!(
                            ":: KDHEAD: block={} decode=none reason=no-cited-slicing :: this bench cites no surface-or-geometry field slicing for this block — [METAL s13] settled it in the negative ('the scanout surface ADDRESS is not exposed anywhere in these head-block windows') and +0x34C is raster TOTALS (vtotal|htotal), not the active geometry. Printing a slicing we cannot cite would be KD3's error a second time ::",
                            blk.name,
                        );
                    }
                    break;
                }
            };

            let mut answered = false;
            let mut nonzero = false;
            for p in 0..blk.nprobe {
                if kdhead_answered(pass0[h][p]) {
                    answered = true;
                    if pass0[h][p] != 0 {
                        nonzero = true;
                    }
                }
            }

            let live_tok = match bracket {
                None => "unknown",
                Some(_) if live[h] => "yes",
                Some(_) => "no",
            };

            let surf = geom.surface.unwrap_or(0);
            let pitch = geom.pitch.unwrap_or(0);

            let verdict = if bracket.is_none() {
                "NO-BRACKET-NOT-SCORED"
            } else if !live[h] {
                "DARK-NOT-SCORED"
            } else if !answered || !nonzero {
                "UNREADABLE"
            } else if geom.width == gop_w
                && geom.height == gop_h
                && geom.surface.map_or(true, |s| s == gop_vram_offset as u64)
                && geom.pitch.map_or(true, |p| p == gop_pitch)
            {
                n_agree += 1;
                "AGREE"
            } else {
                n_disagree += 1;
                "DISAGREE"
            };
            if verdict == "AGREE" || verdict == "DISAGREE" || verdict == "UNREADABLE" {
                n_decoded += 1;
            }

            let mismatch = if verdict != "DISAGREE" {
                "none"
            } else if geom.width != gop_w || geom.height != gop_h {
                "geom"
            } else if geom.surface.map_or(false, |s| s != gop_vram_offset as u64) {
                "surface"
            } else {
                "pitch"
            };

            serial_println!(
                ":: KDHEAD: head={} live={} geom={}x{} surface=0x{:X} pitch={} vs gop={}x{} 0x{:X} {} -> {} :: block={} mismatch={} surface_present={} pitch_present={} slicing={} — AGREE on a live head pins this decode BY OBSERVATION and re-opens KD3; DISAGREE names the field and is a finding about the slicing, not about the silicon; DARK-NOT-SCORED means the bracket says this head does not scan and the read is not evidence either way (R19) ::",
                h, live_tok, geom.width, geom.height, surf, pitch,
                gop_w, gop_h, gop_vram_offset, gop_pitch, verdict,
                blk.name, mismatch,
                if geom.surface.is_some() { "y" } else { "n" },
                if geom.pitch.is_some() { "y" } else { "n" },
                geom.cite,
            );
        }
    }

    serial_println!(
        ":: KDHEAD: end rung=KD14 bracket={} blocks={} separated={} decoded={} agree={} disagree={} writes=0 :: DEPENDS ON KD4 (the control read, [METAL s11]) and on BEAMX86's sampler for taking it; separated= is how many candidate blocks had a stride that actually distinguishes heads, which is the question s4 answered by inference and this rung answers by measurement. agree>0 on a live head re-opens KD3; agree=0 disagree>0 says which field of which slicing is wrong; separated=0 says every candidate stride collapses and the per-head decode has no block left to stand on — and NONE of those three is a statement that a head is dead, which the bracket line above settles independently ::",
        match bracket { Some(_) => "beamx86-census", None => "absent" },
        KDHEAD_BLOCKS.len(), n_separated, n_decoded, n_agree, n_disagree,
    );
}

/// KDHEAD — the `headstat` row's base must be the very address KD4 reads, or the rung is bracketing
/// one bank and decoding another. Checked at compile time rather than asserted in a comment.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
const _: () = assert!(KDHEAD_BLOCKS[0].base == regs::NV_PDISPLAY_BASE + 0x6000);
/// KDHEAD — and `armed100` must be that same bank 0x100 in, which is where s4 stood.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
const _: () = assert!(KDHEAD_BLOCKS[1].base == KDHEAD_BLOCKS[0].base + 0x100);
/// KDHEAD — `evocore`'s base must be the same word BEAMX86 cross-checks its sampled vtotal against
/// (`NV_PDISPLAY_BASE + 0x400 + head*0x300 + 0x60`), so the two rungs read one block, not two.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-kdhead"))]
const _: () = assert!(KDHEAD_BLOCKS[2].base == regs::NV_PDISPLAY_BASE + 0x400 + 0x60);
