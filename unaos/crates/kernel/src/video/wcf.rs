// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! WC-F — an INDEPENDENT read of what the HVS actually scans.
//!
//! ## The blind spot this exists to break
//!
//! WC-D proved a window's pixels are byte-correct in RAM (`bad_cache=0 bad_ram=0` on every window at
//! bench geometry). WC-E fixed a real two-writer ordering bug. The bench panel still garbles the
//! 128x128 crystal. Content correct + writers ordered + garble surviving leaves exactly one class of
//! suspect standing, and it is the class BOTH prior witnesses are constitutionally unable to see:
//! they address the framebuffer through `info.stride`, the same number the blit writes through, so
//! they agree with the blit no matter what geometry the display pipe is programmed with. A witness
//! that asks our numbers can never falsify our numbers.
//!
//! This module asks two questions neither prior arc could:
//!
//! 1. **Does the LIVE `FrameBuffer` describe the surface the firmware allocated?** WC-E printed the
//!    firmware's geometry at bring-up and stopped there. The `FrameBuffer` the compositor addresses
//!    through is a separate object built from that reply — its `base`, `len` and `stride` can each
//!    diverge (a remap, a clamp, a rounding, an override) with nothing downstream able to notice.
//!    [`ScanoutTruth`](crate::arch::aarch64::mailbox::ScanoutTruth) is put beside the live handle on
//!    one line, with the identities named: `base_match`, `rowbytes_match`, `pitch_match`, `fits`.
//!
//! 2. **Is the defect in the blit path or in the scan-out?** The twin-pattern probe below renders the
//!    SAME known 16x16 pattern twice, at the same 4x upscale the bench window gets, side by side:
//!    - the LEFT block through the compositor's own primitive (`FrameBuffer::put_pixel`, indexed by
//!      `info.stride`) — the exact addressing `wm::draw_window` uses;
//!    - the RIGHT block through raw stores computed from the FIRMWARE'S PITCH, touching neither
//!      `info.stride` nor `put_pixel`.
//!
//!    If the two numbers agree, the two blocks are byte-identical and land aligned on the panel. If
//!    they disagree, the compositor's block shears against the direct block by exactly the phase
//!    error, and a bench photo reads the verdict off the panel without a serial cable:
//!    **left garbled / right clean ⇒ the blit path's addressing; both garbled ⇒ HVS or pitch.**
//!
//! The verdict is also readable on the wire alone, which is the point of doing it as a cross-read:
//! each block is read back through the OTHER path's addressing and compared to the reference. A
//! nonzero `comp_bad` (left block, read at firmware pitch) or `direct_bad` (right block, read at
//! `info.stride`) is the two paths disagreeing, stated as a count, no photo needed.
//!
//! ## Scope and safety
//!
//! `witness`-gated and aarch64-only: knob-off, this module does not compile and the Pi media are
//! byte-identical. The probe paints into an unused strip at the bottom-right of the panel, is
//! redrawn by every composite pass (so a later desktop flush cannot erase it before the operator
//! photographs it), and prints exactly once. Every path through [`run`] emits a line — PASS, FAIL or
//! `-> SKIP` with a reason — so a missing verdict is never ambiguous.
//!
//! Firmware is not re-queried from here: the property mailbox uses one static buffer with no lock and
//! is safe only during single-core boot, while this runs from composite (post-SMP, syscall context).
//! The firmware read therefore happens where it is safe — at framebuffer bring-up, in
//! `mailbox::witness_fb_geometry` — and is *recorded* for this module to compare against.

use core::sync::atomic::{AtomicBool, Ordering};

use unaos_boot_info::PixelFormat;

use super::FrameBuffer;
use crate::arch::aarch64::mailbox;

/// Reference pattern edge, in SOURCE pixels. 16x16 matches the granularity a crystal's content has.
const BLK: usize = 16;
/// Upscale. 4x is what `wm::place` gives a 128x128 window on the 1920x1200 bench panel (WC-SCALE's
/// legibility ceiling), so the probe exercises the bench's blit, not a 1x special case.
const SCALE: usize = 4;
/// Destination edge of one block, in panel pixels.
const SIDE: usize = BLK * SCALE;
/// Gap between the twins — wide enough that a shear is obvious, narrow enough for one photo.
const GAP: usize = 16;
/// Distance kept from the panel edges.
const MARGIN: usize = 16;

/// The reference pattern: a distinct colour per source pixel, so NO shift of the image — by a row, a
/// column, or a sub-pixel byte — can map onto itself. A flat or striped pattern would survive some
/// phase errors unchanged and quietly pass; this one cannot. R and G ramp on the two axes (a shear
/// tilts the ramp visibly), B alternates per pixel at the highest frequency the grid allows (a
/// one-pixel phase error inverts it across the whole block).
#[inline]
fn reference(x: usize, y: usize) -> u32 {
    let r = ((x * 15 + 16) & 0xFF) as u32;
    let g = ((y * 15 + 16) & 0xFF) as u32;
    let b = if (x ^ y) & 1 == 0 { 0xF0u32 } else { 0x20u32 };
    (r << 16) | (g << 8) | b
}

/// Encode `0x00RRGGBB` into the little-endian 4-byte word this surface stores. `None` for any layout
/// without a full 4-byte pixel — the direct path must reproduce `put_pixel`'s encoding exactly or the
/// comparison would measure our own encoder instead of the addressing under test.
#[inline]
fn encode(fmt: PixelFormat, color: u32) -> Option<u32> {
    let r = (color >> 16) & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = color & 0xFF;
    match fmt {
        PixelFormat::Rgb => Some(r | (g << 8) | (b << 16)),
        PixelFormat::Bgr => Some(b | (g << 8) | (r << 16)),
        _ => None,
    }
}

/// Inverse of [`encode`].
#[inline]
fn decode(fmt: PixelFormat, raw: u32) -> Option<u32> {
    let (a, b, c) = (raw & 0xFF, (raw >> 8) & 0xFF, (raw >> 16) & 0xFF);
    match fmt {
        PixelFormat::Rgb => Some((a << 16) | (b << 8) | c),
        PixelFormat::Bgr => Some((c << 16) | (b << 8) | a),
        _ => None,
    }
}

/// Whether the one-shot verdict has been printed.
static WITNESSED: AtomicBool = AtomicBool::new(false);

/// The whole WC-F probe. Called at the tail of every composite pass: the twins are repainted each
/// time (so nothing that runs after a composite can erase them before the panel is photographed) and
/// the verdict is printed on the first pass only.
pub fn run(fb: &FrameBuffer) {
    let first = !WITNESSED.swap(true, Ordering::Relaxed);
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);
    let bpp = info.bytes_per_pixel;
    let base = fb.base_addr();
    let len = fb.len();

    let truth = mailbox::scanout_truth();
    if first {
        report_geometry(&truth, base, len, pw, ph, info.stride, bpp);
    }

    // The direct path needs a byte pitch from OUTSIDE the kernel's own bookkeeping. `get_pitch` is
    // the independently-read one; fall back to the allocation's only if that query failed.
    let fw_pitch = if truth.get_pitch != 0 { truth.get_pitch } else { truth.alloc_pitch } as usize;

    let need_w = 2 * SIDE + GAP + 2 * MARGIN;
    let need_h = SIDE + 2 * MARGIN;
    let encodable = bpp == 4 && encode(info.pixel_format, 0).is_some();
    if base == 0 || !truth.valid || fw_pitch == 0 || !encodable || pw < need_w || ph < need_h {
        if first {
            serial_println!(
                "[wc-f] twin -> SKIP (panel={}x{} need={}x{} bpp={} fw_pitch={} truth_valid={} encodable={})",
                pw, ph, need_w, need_h, bpp, fw_pitch, truth.valid, encodable
            );
        }
        return;
    }

    // Bottom-right, clear of the desktop's chrome and of where `wm::place` puts windows.
    let ly = ph - MARGIN - SIDE;
    let lx = pw - MARGIN - (2 * SIDE + GAP);
    let rx = lx + SIDE + GAP;

    // --- LEFT: the compositor's own addressing. Byte-for-byte the inner loop of `wm::draw_window`.
    for row in 0..BLK {
        for col in 0..BLK {
            let px = reference(col, row);
            for sy in 0..SCALE {
                for sx in 0..SCALE {
                    fb.put_pixel(lx + col * SCALE + sx, ly + row * SCALE + sy, px);
                }
            }
        }
    }

    // --- RIGHT: the same pattern, addressed at the firmware's pitch, straight into the mapping.
    let raw_of = |c: u32| encode(info.pixel_format, c).unwrap_or(0);
    for row in 0..BLK {
        for col in 0..BLK {
            let raw = raw_of(reference(col, row));
            for sy in 0..SCALE {
                let dy = ly + row * SCALE + sy;
                for sx in 0..SCALE {
                    let dx = rx + col * SCALE + sx;
                    let off = dy * fw_pitch + dx * 4;
                    // Bounds-checked against the MAPPED length, never against the geometry under
                    // test: a wrong pitch must show up as a wrong picture, not as a fault.
                    if off + 4 > len {
                        continue;
                    }
                    // SAFETY: `off + 4 <= len`, and `base..base+len` is the mapped framebuffer.
                    unsafe { core::ptr::write_volatile((base + off) as *mut u32, raw) };
                }
            }
        }
    }

    // Push both blocks out to the memory the HVS scans. Rows are cleaned by BOTH pitches, because
    // which one describes the real rows is precisely the open question.
    let rows_lo = ly;
    let rows_hi = (ly + SIDE).min(ph);
    let k_row = info.stride * bpp;
    fb.flush_range(rows_lo * k_row, (rows_hi - rows_lo) * k_row);
    let fw_lo = rows_lo * fw_pitch;
    let fw_hi = ((rows_hi * fw_pitch) + fw_pitch).min(len);
    if fw_hi > fw_lo {
        crate::arch::flush_framebuffer_range(base + fw_lo, fw_hi - fw_lo);
    }

    if !first {
        return;
    }

    // Read back from RAM, not from our own dirty lines — the same discipline WC-D's `bad_ram` pass
    // uses, and the only way the read can disagree with the write.
    let lo = fw_lo.min(rows_lo * k_row);
    let hi = fw_hi.max(rows_hi * k_row).min(len);
    if hi > lo {
        crate::arch::cache::invalidate_range(base + lo, hi - lo);
    }

    // The cross-read. Each block is checked through the OTHER path's addressing; agreement of the
    // two pitches is exactly the condition under which both counts are zero.
    let mut checked = 0usize;
    let mut comp_bad = 0usize; // left block (compositor-written) read at FIRMWARE pitch
    let mut direct_bad = 0usize; // right block (directly written) read at info.stride
    let mut first_bad = (0usize, 0usize, 0u32, 0u32);
    for row in 0..BLK {
        for col in 0..BLK {
            let want = reference(col, row);
            for sy in 0..SCALE {
                let dy = ly + row * SCALE + sy;
                for sx in 0..SCALE {
                    checked += 1;
                    // left, at firmware pitch
                    let off = dy * fw_pitch + (lx + col * SCALE + sx) * 4;
                    if off + 4 <= len {
                        // SAFETY: bounds-checked above; volatile so the load is not folded with the
                        // stores this function just issued.
                        let raw = unsafe { core::ptr::read_volatile((base + off) as *const u32) };
                        let got = decode(info.pixel_format, raw).unwrap_or(0);
                        if got != want {
                            if comp_bad == 0 && direct_bad == 0 {
                                first_bad = (lx + col * SCALE + sx, dy, got, want);
                            }
                            comp_bad += 1;
                        }
                    }
                    // right, at info.stride
                    if let Some(got) = fb.read_pixel(rx + col * SCALE + sx, dy) {
                        if got != want {
                            if comp_bad == 0 && direct_bad == 0 {
                                first_bad = (rx + col * SCALE + sx, dy, got, want);
                            }
                            direct_bad += 1;
                        }
                    }
                }
            }
        }
    }

    let ok = comp_bad == 0 && direct_bad == 0;
    serial_println!(
        "[wc-f] twin left=comp@stride({},{}) right=direct@pitch({},{}) blk={}x{} scale={}x panel={}x{} k_row={}B fw_pitch={}B checked={} comp_bad={} direct_bad={} first=({},{}) got={:#08x} want={:#08x} -> {}",
        lx, ly, rx, ly, BLK, BLK, SCALE, pw, ph, k_row, fw_pitch,
        checked, comp_bad, direct_bad,
        first_bad.0, first_bad.1, first_bad.2, first_bad.3,
        if ok { "PASS" } else { "FAIL" }
    );
}

/// The geometry half: the live `FrameBuffer` beside the firmware's recorded answers, with every
/// identity a scan-out defect breaks named and evaluated, so the verdict survives without a photo.
fn report_geometry(
    truth: &mailbox::ScanoutTruth,
    base: usize,
    len: usize,
    pw: usize,
    ph: usize,
    stride: usize,
    bpp: usize,
) {
    if !truth.valid {
        serial_println!(
            "[wc-f] scanout -> SKIP (no firmware ground truth recorded; kernel base={:#x} len={} panel={}x{} stride={}px)",
            base, len, pw, ph, stride
        );
        return;
    }
    let k_row = stride * bpp;
    // `base_match`: the mapping the compositor stores into is the buffer the firmware allocated.
    let base_match = base as u64 == truth.alloc_base;
    // `rowbytes_match`: THE identity under suspicion. The compositor advances one row by
    // `stride * bpp` bytes; the HVS advances by the firmware's pitch. Any difference shifts every
    // row's phase against the one above it — the signature of the bench garble.
    let rowbytes_match = truth.get_pitch != 0 && k_row == truth.get_pitch as usize;
    // `pitch_match`: the allocation's pitch and an independently-queried pitch are the same number.
    let pitch_match = truth.get_pitch == truth.alloc_pitch;
    // `panel_match`: the geometry the compositor lays windows out in is the mode being scanned.
    let panel_match = pw == truth.virt_w as usize && ph == truth.virt_h as usize;
    // `fits`: the mapping we write through covers every row the pipe scans.
    let fits = len >= truth.get_pitch.max(truth.alloc_pitch) as usize * truth.virt_h as usize;
    let ok = base_match && rowbytes_match && pitch_match && panel_match && fits;
    serial_println!(
        "[wc-f] scanout kernel base={:#x} len={} panel={}x{} stride={}px bpp={} row_bytes={}B :: firmware base={:#x} size={} alloc_pitch={}B get_pitch={}B virt={}x{} phys={}x{} off=({},{}) depth={} order={} alpha={} :: base_match={} rowbytes_match={} pitch_match={} panel_match={} fits={} -> {}",
        base, len, pw, ph, stride, bpp, k_row,
        truth.alloc_base, truth.alloc_size, truth.alloc_pitch, truth.get_pitch,
        truth.virt_w, truth.virt_h, truth.phys_w, truth.phys_h, truth.off_x, truth.off_y,
        truth.depth, truth.order, truth.alpha,
        base_match, rowbytes_match, pitch_match, panel_match, fits,
        if ok { "PASS" } else { "FAIL" }
    );
}
