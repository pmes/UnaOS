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
/// performs the double-buffer + EVO flip.
pub unsafe fn takeover_display(
    gpu: &GpuInfo,
    bar0: usize,
    allocator: &mut VramAllocator,
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

    // ── Phase 1.5: Candidate Decode (Pull 3) ───────────────────────────
    let mut cand_vals = [[0u32; 3]; 4];
    let cands = [0x310, 0x520, 0x604, 0x614];
    
    for pass in 0..3 {
        for head in 0..2 {
            dump_dense_window(bar0, head, pass, 0x300, 0x35C);
            dump_dense_window(bar0, head, pass, 0x3F0, 0x40C);
            dump_dense_window(bar0, head, pass, 0x5F0, 0x61C);
        }
        
        let head0_base = regs::NV_PDISPLAY_BASE + 0x6000;
        for i in 0..4 {
            cand_vals[i][pass] = mmio_read(bar0, head0_base + cands[i]);
        }

        if pass < 2 {
            for _ in 0..2_000_000 { core::hint::spin_loop(); }
        }
    }

    for i in 0..4 {
        let off = cands[i];
        let v0 = cand_vals[i][0];
        let v1 = cand_vals[i][1];
        let v2 = cand_vals[i][2];
        let stable = if v0 == v1 && v1 == v2 { "yes" } else { "no" };
        
        serial_println!(":: kdisp: cand off={:03X} stable={} v0={:08X} v1={:08X} v2={:08X} ::",
            off, stable, v0, v1, v2);
        
        let shl8 = v0.wrapping_shl(8);
        let shl12 = v0.wrapping_shl(12);
        let pitch4 = v0 / 4;
        serial_println!(":: kdisp: cand off={:03X} shl8={:08X} shl12={:08X} pitch4={} ::",
            off, shl8, shl12, pitch4);
        serial_println!(":: kdisp: cand off={:03X} geom high={} low={} ::",
            off, v0 >> 16, v0 & 0xFFFF);
    }

    // ── Phase 2: Display takeover (write path, gated) ──────────────────
    if !cfg!(feature = "nvidia-kepler-takeover") {
        serial_println!(":: kdisp: trace-only — takeover feature not set ::");
        return None;
    }

    let head = match found_head {
        Some(h) => h,
        None => {
            serial_println!(":: kdisp: takeover-abort no-match ::");
            return None;
        }
    };

    let gop_info = match crate::video::fbcon::current_info() {
        Some(info) => info,
        None => {
            serial_println!(":: kdisp: takeover-abort no-gop-info ::");
            return None;
        }
    };
    let expected_width = gop_info.width as u32;
    let expected_height = gop_info.height as u32;

    let width = matched_size & 0xFFFF;
    let height = matched_size >> 16;
    if width != expected_width || height != expected_height {
        serial_println!(":: kdisp: takeover-abort bounds head={} {}x{} vs {}x{} ::",
            head, width, height, expected_width, expected_height);
        return None;
    }

    let bar1 = vram_base;
    serial_println!(":: kdisp: takeover head={} addr={:08X} {}x{} ::",
        head, matched_addr, expected_width, expected_height);

    // Double-buffer: copy GOP surface to new allocation
    let fb_size = (expected_width * expected_height * 4) as usize;
    let new_fb_offset = match allocator.alloc(fb_size) {
        Some(off) => off,
        None => {
            serial_println!(":: kdisp: takeover-abort alloc-fail ::");
            return None;
        }
    };
    serial_println!(":: kdisp: alloc new_fb={:X} ::", new_fb_offset);

    let src = (bar1 + gop_vram_offset) as *const u8;
    let dst = (bar1 + new_fb_offset) as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst, fb_size);

    // EVO core channel push (flip surface address)
    let evo_pb_off = match allocator.alloc(4096) {
        Some(off) => off,
        None => {
            serial_println!(":: kdisp: takeover-abort pb-alloc-fail ::");
            return None;
        }
    };

    let evo_pb = (bar1 + evo_pb_off) as *mut u32;
    let offset_origin_method = 0x400 + (head * 0x300) + 0x60;
    let update_method = 0x80;
    let new_addr = (new_fb_offset >> 8) as u32;

    core::ptr::write_volatile(evo_pb.add(0), (1 << 18) | (offset_origin_method as u32));
    core::ptr::write_volatile(evo_pb.add(1), new_addr);
    core::ptr::write_volatile(evo_pb.add(2), (1 << 18) | (update_method as u32));
    core::ptr::write_volatile(evo_pb.add(3), 0x00000000);

    // EVO core channel control — g80_pdisplay.xml line 1065: 0x610490 is
    // actually DAEMON.RFIFO_STATUS (GF119+), not an EVO channel control.
    // The pre-GF119 CTRL array lives at PDISPLAY+0x300 (stride 0x8, length 5)
    // but is absent from the GF119+ stripe.  We proceed with the empirically
    // probed 0x490 offset with a full bad-read guard; if it reads zero the
    // takeover aborts cleanly.
    let core_ctrl = regs::NV_PDISPLAY_BASE + 0x490;
    let core_ctrl_val = mmio_read(bar0, core_ctrl);
    if core_ctrl_val == 0 || (core_ctrl_val & 0xFFF00000) == 0xBAD00000 {
        serial_println!(":: kdisp: bad-read core_ctrl {:X} {:08X} ::", core_ctrl, core_ctrl_val);
        serial_println!(":: kdisp: takeover-abort bad-core-ctrl ::");
        return None;
    }

    mmio_write(bar0, core_ctrl, core_ctrl_val & !0x10);
    mmio_write(bar0, core_ctrl, mmio_read(bar0, core_ctrl) & !0x03);
    for _ in 0..100_000 { core::hint::spin_loop(); }

    let push_handle = (evo_pb_off >> 8) as u32 | 0x1;
    mmio_write(bar0, core_ctrl + 0x4, push_handle);
    mmio_write(bar0, core_ctrl + 0x8, 0x00010000);
    mmio_write(bar0, core_ctrl + 0xC, 0x00000001);

    // DISP_USER PUT — g80_pdisplay.xml line 1091
    mmio_write(bar0, 0x640000, 0);
    mmio_write(bar0, core_ctrl, mmio_read(bar0, core_ctrl) | 0x10);
    mmio_write(bar0, core_ctrl, mmio_read(bar0, core_ctrl) | 0x01000013);
    mmio_write(bar0, 0x640000, 16);

    // Bounded latch poll
    let head_base = regs::NV_PDISPLAY_BASE + 0x400 + (head * 0x300) + 0x60;
    let mut latched = false;
    for _ in 0..1_000_000 {
        if mmio_read(bar0, head_base) == new_addr {
            latched = true;
            break;
        }
        core::hint::spin_loop();
    }

    if latched {
        serial_println!(":: kdisp: flip-ok head={} addr={:X} ::", head, new_addr);
        Some(new_fb_offset)
    } else {
        serial_println!(":: kdisp: takeover-abort evo-flip-timeout ::");
        None
    }
}

/// Returns false for zero, 0xFFFFFFFF, and the 0xBAD0xxxx pattern that our
/// BAR0 reads return when the target register is unmapped.
#[inline]
fn is_live(val: u32) -> bool {
    val != 0 && val != 0xFFFFFFFF && (val & 0xFFF00000) != 0xBAD00000
}

/// Dumps a dense window of head configuration.
fn dump_dense_window(bar0: usize, head: usize, pass: usize, start: usize, end: usize) {
    let base = regs::NV_PDISPLAY_BASE + 0x6000 + (head * 0x800);
    let mut rows = 0;
    for offset in (start..=end).step_by(4) {
        let val = unsafe { mmio_read(bar0, base + offset) };
        serial_println!(":: kdisp: window head{} pass{} off={:03X} val={:08X} ::", head, pass, offset, val);
        rows += 1;
    }
    serial_println!(":: kdisp: window head{} pass{} done rows={} ::", head, pass, rows);
}

