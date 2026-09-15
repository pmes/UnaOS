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
    kdisp_phase!("panel_console_resume"); #[cfg(all(feature = "nvidia-kepler", feature = "beam"))] beam_probe(bar0); // BEAMX86 (rmbp A5) — the beam source's ONE call site, folded onto this line so the knob-off image cannot shift. Strictly AFTER the console resume (the head is repointed, the pattern cleared, the surface settled, so the raster this samples is the one presents will be ordered against) and strictly BEFORE the compositor activation below, so the first window present is already bracketed. Read-only and bounded: 4 heads x 45 ms = 180 ms, every head sampled even after one ARMS, because a NONE that names only the chosen head is not diagnosable from a flight log and the per-head census is what makes it so. Negligible beside the 1.12 s x5 `fb-draw hold` this same function already spends.

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


