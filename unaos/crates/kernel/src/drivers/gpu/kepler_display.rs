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
use super::kepler::{mmio_read, regs, VramAllocator};

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

    // ── Phase 2: Assembly-State Hunt (Pull 6) ──────────────────────────────
    if !cfg!(feature = "nvidia-kepler-takeover") {
        serial_println!(":: kdisp: trace-only — takeover feature not set ::");
        return None;
    }

    // Milestone 1: The gap region (0x610000..=0x6103FC)
    let mut m1_0x6101e0_val = 0;
    for pass in 0..2 {
        let mut rows = 0;
        for addr in (0x610000..=0x6103FC).step_by(4) {
            let val = mmio_read(bar0, addr);
            serial_println!(":: kdisp: gap pass{} off={:03X} val={:08X} ::", pass, addr - 0x610000, val);
            if pass == 0 && addr == (regs::NV_PDISPLAY_BASE + 0x01E0) {
                m1_0x6101e0_val = val;
            }
            rows += 1;
        }
        serial_println!(":: kdisp: gap pass{} done rows={} ::", pass, rows);

        if pass == 0 {
            for _ in 0..2_000_000 { core::hint::spin_loop(); }
        }
    }

    // Milestone 2: Widened known-value scan
    const MAX_HITS: usize = 64;
    let mut hit_addrs = [0usize; MAX_HITS];
    let mut hit_vals = [0u32; MAX_HITS];
    let mut hits = 0;

    let scan_ranges = [(0x614000, 0x61FFFC), (0x640000, 0x647FFC)];
    for (start, end) in scan_ranges.iter() {
        for addr in (*start..=*end).step_by(4) {
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
                if hits < MAX_HITS {
                    hit_addrs[hits] = addr;
                    hit_vals[hits] = val;
                    serial_println!(":: kdisp: evo-scan2 hit off={:06X} val={:08X} key={} ::", addr, val, keyname);
                }
                hits += 1;
            }
        }
    }
    let capped = if hits > MAX_HITS { "t" } else { "f" };
    serial_println!(":: kdisp: evo-scan2 done ranges=614000-61FFFC,640000-647FFC hits={} capped={} ::", hits, capped);

    // Bounded delay before M3
    for _ in 0..2_000_000 { core::hint::spin_loop(); }

    // Milestone 3: Armed-pair check
    let repoint_reg = regs::NV_PDISPLAY_BASE + 0x01E0; // 0x6101E0
    let m3_0x6101e0_val = mmio_read(bar0, repoint_reg);
    serial_println!(":: kdisp: pair off=6101E0 first={:08X} second={:08X} ::", m1_0x6101e0_val, m3_0x6101e0_val);

    let check_count = if hits > MAX_HITS { MAX_HITS } else { hits };
    for i in 0..check_count {
        let addr = hit_addrs[i];
        let first = hit_vals[i];
        let second = mmio_read(bar0, addr);
        serial_println!(":: kdisp: pair off={:06X} first={:08X} second={:08X} ::", addr, first, second);
    }

    None
}

/// Returns false for zero, 0xFFFFFFFF, and the 0xBAD0xxxx pattern that our
/// BAR0 reads return when the target register is unmapped.
#[inline]
fn is_live(val: u32) -> bool {
    val != 0 && val != 0xFFFFFFFF && (val & 0xFFF00000) != 0xBAD00000
}


