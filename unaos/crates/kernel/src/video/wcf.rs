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
//! WC-D proved a window's pixels are byte-correct in RAM. WC-E fixed a real two-writer ordering bug.
//! The bench panel still garbles the 128x128 crystal. Content correct + writers ordered + garble
//! surviving leaves the scan-out side standing, and it is the side both prior witnesses are
//! constitutionally unable to see: they address the framebuffer through `info.stride`, the same number
//! the blit writes through, so they agree with the blit whatever the display pipe is programmed with.
//!
//! ## What counts as independent, and what does not
//!
//! The first cut of this module was **tautological**, and the reason is worth keeping written down.
//! `mailbox::init_framebuffer` defines `info.stride = pitch / 4`. So `stride * bpp == pitch`
//! identically, for every possible firmware — comparing them compares a number to itself. Re-asking
//! `GET_PITCH` does not help either: it is the same firmware answering the same question, so a second
//! agreeing reply adds no information about what the hardware is *doing*. Any witness built on the
//! pitch reply can only ever confirm the pitch reply.
//!
//! Two things here are genuinely independent of it:
//!
//! 1. **A row step derived from GEOMETRY, not from the pitch reply.** `virt_w * 4` is what one row of
//!    the reported virtual width occupies. It equals the pitch exactly when the firmware allocates
//!    unpadded, and diverges — by the padding — when it does not. `rowstep_match` is therefore a real
//!    question with a reachable `false`, which `stride * bpp == pitch` never was. The right-hand twin
//!    block is addressed with it, so when the firmware pads, the two blocks are written to genuinely
//!    different addresses and the panel shows it.
//! 2. **A marker whose photographed slope IS the hardware's row step.** [`ramp`] writes one two-pixel
//!    mark per row by pure byte arithmetic, stepping `k_row + 4` bytes each time. If the HVS advances
//!    a row by exactly `k_row`, the marks photograph as a diagonal moving exactly one pixel right per
//!    row. If the hardware's real step is `k_row + d`, the marks drift by `d/4` pixels per row and the
//!    diagonal visibly bends. This is the only construct here that can observe *"the HVS steps
//!    differently than the firmware reported"* — every number on the serial line, ours and the
//!    firmware's alike, is downstream of the firmware's own claim. It is measured off the photograph.
//!
//! ## The twin probe
//!
//! One known 16x16 pattern rendered twice, at the bench's 4x upscale, side by side:
//!
//! - **left** through `FrameBuffer::put_pixel` (indexed by `info.stride`) — byte-for-byte the
//!   addressing of `wm::draw_window`'s inner loop;
//! - **right** through raw stores whose row step is `virt_w * 4`, touching neither `info.stride` nor
//!   the pitch reply.
//!
//! Both are cross-read through the other path's addressing. On a padded framebuffer the two disagree
//! and say so as a count; on the panel the same probe reads as a photo — **left garbled / right clean
//! ⇒ the blit path; both garbled ⇒ HVS or pitch.** The direct path stores three bytes, exactly like
//! `put_pixel`, so the two blocks cannot differ for the non-addressing reason of one of them having
//! zeroed the fourth byte under an alpha mode that reads it.
//!
//! A comparison that cannot be made is counted as `skipped`, never silently dropped: a `comp_bad=0`
//! reached by skipping every pixel would read as agreement, which is the one way a witness can lie.
//! `PASS` requires `skipped == 0` **and** `checked` equal to the full expected count.
//!
//! ## Scope and safety
//!
//! `witness`-gated and aarch64-only: knob-off, this module does not compile and the Pi media are
//! byte-identical. It runs only from composite passes that already drew something (so it adds no work
//! to an idle desktop), only when no live window overlaps its region, and repaints so a later flush
//! cannot erase it before the panel is photographed.
//!
//! **Every line is one-shot, and a retryable condition never prints `SKIP`.** The probe runs from a
//! path that executes hundreds of times per boot, so an unlatched print is a flood and a `SKIP` emitted
//! before a later `PASS` is a false red against the spec's `FORBID`. See [`WITNESSED`] for the
//! terminal-vs-retryable split: terminal causes latch a one-shot `SKIP`, an overlapping window emits a
//! one-shot `DEFER` and keeps retrying, and the geometry report latches independently of both.
//!
//! Firmware is not re-queried from here: the property mailbox uses one static buffer with no lock and
//! is safe only during single-core boot, while this runs from composite (post-SMP, syscall context).
//! The firmware read happens where it is safe — at framebuffer bring-up — and is *recorded*.

use core::sync::atomic::{AtomicBool, Ordering};

use unaos_boot_info::PixelFormat;

use super::FrameBuffer;
use crate::arch::aarch64::mailbox;

/// Reference pattern edge, in SOURCE pixels.
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
/// Rows spanned by the slope marker. Each row moves the mark one nominal pixel right, so this is also
/// its horizontal travel; 256 is enough for a padding of a single pixel to accumulate into an
/// unmistakable bend (256 rows x 1 px = a quarter of the bench panel's width), and small enough to fit
/// beside the twins on a 640x480 gate panel.
const RAMP_ROWS: usize = 256;
/// Width the marker reserves: its travel plus the two-pixel mark itself.
const RAMP_W: usize = RAMP_ROWS + 8;

/// Total comparisons the cross-read must make (both blocks, every destination pixel).
const EXPECTED: usize = BLK * BLK * SCALE * SCALE * 2;

/// The reference pattern: a distinct colour per source pixel, so NO shift of the image — by a row, a
/// column, or a sub-pixel byte — can map onto itself. A flat or striped pattern would survive some
/// phase errors unchanged and quietly pass; this one cannot. R and G ramp on the two axes (a shear
/// tilts the ramp visibly), B alternates per pixel at the highest frequency the grid allows.
#[inline]
fn reference(x: usize, y: usize) -> u32 {
    let r = ((x * 15 + 16) & 0xFF) as u32;
    let g = ((y * 15 + 16) & 0xFF) as u32;
    let b = if (x ^ y) & 1 == 0 { 0xF0u32 } else { 0x20u32 };
    (r << 16) | (g << 8) | b
}

/// The three colour bytes in the order this surface stores them — `put_pixel`'s encoding, reproduced
/// so the direct path differs from the compositor path in ADDRESSING ONLY. `None` for any layout
/// without a full 4-byte RGB pixel; the probe skips rather than invent one.
#[inline]
fn bytes_of(fmt: PixelFormat, color: u32) -> Option<[u8; 3]> {
    let r = ((color >> 16) & 0xFF) as u8;
    let g = ((color >> 8) & 0xFF) as u8;
    let b = (color & 0xFF) as u8;
    match fmt {
        PixelFormat::Rgb => Some([r, g, b]),
        PixelFormat::Bgr => Some([b, g, r]),
        _ => None,
    }
}

/// Inverse of [`bytes_of`].
#[inline]
fn color_of(fmt: PixelFormat, b: [u8; 3]) -> Option<u32> {
    match fmt {
        PixelFormat::Rgb => Some(((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32),
        PixelFormat::Bgr => Some(((b[2] as u32) << 16) | ((b[1] as u32) << 8) | b[0] as u32),
        _ => None,
    }
}

/// Store one pixel as THREE bytes at a raw framebuffer offset — `put_pixel`'s write, addressed by
/// byte arithmetic instead of by `info.stride`. Leaves the fourth byte exactly as `put_pixel` does
/// (untouched), so an alpha mode that reads it cannot make the two paths differ for a reason that has
/// nothing to do with addressing. Returns whether the store landed.
#[inline]
fn store3(base: usize, len: usize, off: usize, px: [u8; 3]) -> bool {
    if off + 3 > len {
        return false;
    }
    // SAFETY: `off + 3 <= len`, and `base..base+len` is the mapped framebuffer.
    unsafe {
        let p = (base + off) as *mut u8;
        p.write(px[0]);
        p.add(1).write(px[1]);
        p.add(2).write(px[2]);
    }
    true
}

/// Read one pixel's three colour bytes back from a raw framebuffer offset. `None` when out of range —
/// the caller must COUNT that, never treat it as agreement.
#[inline]
fn load3(base: usize, len: usize, off: usize) -> Option<[u8; 3]> {
    if off + 3 > len {
        return None;
    }
    // SAFETY: bounds-checked above; volatile so the loads are not folded with the stores above.
    unsafe {
        let p = (base + off) as *const u8;
        Some([
            core::ptr::read_volatile(p),
            core::ptr::read_volatile(p.add(1)),
            core::ptr::read_volatile(p.add(2)),
        ])
    }
}

/// Whether the twin probe has said its final word — a verdict, or a TERMINAL skip.
///
/// Three latches, not one, because the three lines have three different lifetimes and conflating them
/// is how a diagnostic starts lying. The distinction that matters is **terminal vs retryable**:
///
/// - *Terminal* — no framebuffer, no recorded firmware truth, a layout with no 4-byte RGB pixel, a
///   panel too small. Nothing later in the boot changes any of these, so `SKIP` is the final answer:
///   print it once and latch.
/// - *Retryable* — a live window is sitting over the strip. That is a property of this instant, not of
///   the boot; the next composite may well be clear. Printing `SKIP` here would be wrong twice over: a
///   run that later PASSes would carry both lines, tripping the spec's unconditional
///   `FORBID … -> SKIP` on a perfectly good boot, and a window that stays put (a full-screen compat
///   surface never moves) would emit one line per composite pass, unbounded. So the retryable case
///   emits a one-shot `DEFER` token the spec does not forbid, then stays silent and keeps trying.
static WITNESSED: AtomicBool = AtomicBool::new(false);

/// Whether the one-shot `DEFER` token has been emitted. Separate from [`WITNESSED`]: deferring must not
/// prevent the verdict that a later, clear pass can still produce.
static DEFERRED: AtomicBool = AtomicBool::new(false);

/// Whether the `[wc-f] scanout` line has been printed. Separate again: the geometry report answers a
/// question that has nothing to do with whether the strip happens to be clear this instant, so it must
/// fire on the first pass regardless — and exactly once, however many passes defer afterwards.
static GEOM_WITNESSED: AtomicBool = AtomicBool::new(false);

/// The panel rectangles WC-F paints into: the twin blocks, then the slope marker. `None` when the
/// panel is too small to hold them. The caller (`wm::composite_inner`) uses these to refuse to run the
/// probe while a live window overlaps — the probe paints last and would otherwise corrupt what a
/// window is showing.
/// Both marks live in the BOTTOM strip, side by side — twins hard right, marker hard left — rather
/// than stacked. Stacking them put the marker's 256 rows into the middle of the panel, where on the
/// 640x480 default gate geometry `wm::place` lands its first window: the overlap test below would then
/// refuse to run the probe on every ordinary boot, and a witness that habitually skips is no witness.
/// Side by side, both clear the window flow at 640x480 and at 1920x1200 alike.
pub fn reserved(pw: usize, ph: usize) -> Option<[(usize, usize, usize, usize); 2]> {
    let need_w = RAMP_W + (2 * SIDE + GAP) + 3 * MARGIN;
    let need_h = RAMP_ROWS.max(SIDE) + 2 * MARGIN;
    if pw < need_w || ph < need_h {
        return None;
    }
    let ty = ph - MARGIN - SIDE;
    let tx = pw - MARGIN - (2 * SIDE + GAP);
    let ry = ph - MARGIN - RAMP_ROWS;
    Some([(tx, ty, 2 * SIDE + GAP, SIDE), (MARGIN, ry, RAMP_W, RAMP_ROWS)])
}

/// The whole WC-F probe.
///
/// `region_clear` is the caller's assurance that no live window overlaps [`reserved`]; `false` skips
/// the paint entirely rather than scribble over a window's content.
pub fn run(fb: &FrameBuffer, region_clear: bool) {
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);
    let bpp = info.bytes_per_pixel;
    let base = fb.base_addr();
    let len = fb.len();
    let truth = mailbox::scanout_truth();

    if !GEOM_WITNESSED.swap(true, Ordering::Relaxed) {
        report_geometry(&truth, base, len, pw, ph, info.stride, bpp);
    }

    // The row step the RIGHT-hand block is addressed with: one row of the firmware's reported virtual
    // width. Independent of the pitch reply by construction — it equals the pitch exactly when the
    // allocation is unpadded, and differs by the padding when it is not.
    let geom_row = truth.virt_w as usize * 4;
    let k_row = info.stride * bpp;
    let boxes = reserved(pw, ph);
    let fmt_ok = bytes_of(info.pixel_format, 0).is_some();
    let paintable =
        base != 0 && truth.valid && bpp == 4 && fmt_ok && geom_row != 0 && boxes.is_some();

    // Terminal: nothing later in this boot can change any of these. Say so once and stop.
    if !paintable {
        if !WITNESSED.swap(true, Ordering::Relaxed) {
            serial_println!(
                "[wc-f] twin -> SKIP (panel={}x{} bpp={} fmt_ok={} truth_valid={} geom_row={} fits={})",
                pw, ph, bpp, fmt_ok, truth.valid, geom_row, boxes.is_some()
            );
        }
        return;
    }
    // Retryable: a window is over the strip right now. One token, then silence — and no latch, so a
    // later clear pass still produces the verdict.
    if !region_clear {
        if !DEFERRED.swap(true, Ordering::Relaxed) {
            serial_println!(
                "[wc-f] twin -> DEFER (a live window overlaps the probe strip; retrying every composite until it clears)"
            );
        }
        return;
    }
    let boxes = match boxes {
        Some(b) => b,
        None => return,
    };
    let (lx, ly, _, _) = boxes[0];
    let (rax, ray, _, _) = boxes[1];
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

    // --- RIGHT: the same pattern, stepped by the GEOMETRY row, straight into the mapping.
    //
    // In the DIVERGENT case this block deliberately leaves the reserved rectangle, and that is the
    // point rather than a bug: `pad > 0` means `geom_row < k_row`, so each successive row lands at a
    // lower offset than the compositor would have used and the block drifts UPWARD out of the bottom
    // strip. Clamping it back would erase the very displacement the probe exists to make visible. What
    // that costs is the overlap guard's coverage — a divergent boot can scribble this block over a
    // window higher up the panel. Accepted, and bounded three ways: it only happens when the firmware
    // pads (`pad=` says so on the scanout line, which prints first), it is a `witness` build only, and
    // `direct_lands_row=` below states where the block's first row actually came out under the kernel's
    // row step, so the operator knows which part of the panel to photograph and which garble is ours.
    let mut direct_lost = 0usize;
    for row in 0..BLK {
        for col in 0..BLK {
            let px = bytes_of(info.pixel_format, reference(col, row)).unwrap_or([0; 3]);
            for sy in 0..SCALE {
                let dy = ly + row * SCALE + sy;
                for sx in 0..SCALE {
                    let off = dy * geom_row + (rx + col * SCALE + sx) * 4;
                    if !store3(base, len, off, px) {
                        direct_lost += 1;
                    }
                }
            }
        }
    }

    // --- The slope marker, whose PHOTOGRAPHED slope is the hardware's row step.
    let ramp_lost = ramp(base, len, info.pixel_format, k_row, rax, ray);

    // Push the touched regions out to the memory the HVS scans. The ranges are cleaned — and later
    // invalidated — SEPARATELY, never as one convex hull: when `geom_row > k_row` the hull spans rows
    // between them that this probe never wrote, and a bare invalidate over those would discard
    // somebody else's still-dirty lines, manufacturing the very garble we are hunting.
    let (twin_lo, twin_hi) = (ly, (ly + SIDE).min(ph));
    let (ramp_lo, ramp_hi) = (ray, (ray + RAMP_ROWS).min(ph));
    let k_twin = (twin_lo * k_row, (twin_hi * k_row).min(len));
    let g_twin = (twin_lo * geom_row, ((twin_hi + 1) * geom_row).min(len));
    let k_ramp = (ramp_lo * k_row, ((ramp_hi + 1) * k_row).min(len));
    let ranges = [k_twin, g_twin, k_ramp];
    for &(lo, hi) in ranges.iter() {
        if hi > lo {
            crate::arch::flush_framebuffer_range(base + lo, hi - lo);
        }
    }

    // CLAIM the readback half — atomically, not with the load/store pair this used to be. Composite
    // runs on any core, so two could otherwise clear a `load` that had not yet been `store`d and both
    // proceed: duplicate `twin`/`ramp` lines, and worse, one core's bare `DC IVAC` discarding the
    // other's not-yet-cleaned stores and reporting the loss as a FAIL. A spurious FAIL on metal is the
    // most expensive thing a diagnostic can produce. Same swap discipline as the other three latches.
    //
    // Painting above stays multiply-entrant, and safely so: every painter writes the same deterministic
    // bytes, and the winner cleans before it invalidates, so a loser's discarded lines are identical to
    // what is already in RAM.
    if WITNESSED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    // Read back from RAM, not from our own dirty lines — WC-D's `bad_ram` discipline. Same ranges,
    // same separation, same reason.
    for &(lo, hi) in ranges.iter() {
        if hi > lo {
            crate::arch::cache::invalidate_range(base + lo, hi - lo);
        }
    }

    // The cross-read. Each block is checked through the OTHER path's addressing, so the two counts are
    // zero exactly when `k_row` and `geom_row` describe the same rows.
    let mut checked = 0usize;
    let mut skipped = 0usize;
    let mut comp_bad = 0usize; // left block (compositor-written) read at the GEOMETRY row step
    let mut direct_bad = 0usize; // right block (directly written) read at info.stride
    let mut first_bad = (0usize, 0usize, 0u32, 0u32);
    for row in 0..BLK {
        for col in 0..BLK {
            let want = reference(col, row);
            for sy in 0..SCALE {
                let dy = ly + row * SCALE + sy;
                for sx in 0..SCALE {
                    // left, at the geometry row step
                    let x = lx + col * SCALE + sx;
                    match load3(base, len, dy * geom_row + x * 4)
                        .and_then(|b| color_of(info.pixel_format, b))
                    {
                        Some(got) => {
                            checked += 1;
                            if got != want {
                                if comp_bad + direct_bad == 0 {
                                    first_bad = (x, dy, got, want);
                                }
                                comp_bad += 1;
                            }
                        }
                        // A comparison that could not be made. Counted, never dropped: a `comp_bad=0`
                        // arrived at by skipping every pixel would read exactly like agreement.
                        None => skipped += 1,
                    }
                    // right, at info.stride
                    let x = rx + col * SCALE + sx;
                    match fb.read_pixel(x, dy) {
                        Some(got) => {
                            checked += 1;
                            if got != want {
                                if comp_bad + direct_bad == 0 {
                                    first_bad = (x, dy, got, want);
                                }
                                direct_bad += 1;
                            }
                        }
                        None => skipped += 1,
                    }
                }
            }
        }
    }

    let ok =
        comp_bad == 0 && direct_bad == 0 && skipped == 0 && direct_lost == 0 && checked == EXPECTED;
    serial_println!(
        "[wc-f] twin left=comp@stride({},{}) right=direct@geomrow({},{}) direct_lands=({},{}) blk={}x{} scale={}x panel={}x{} k_row={}B geom_row={}B alloc_pitch={}B checked={}/{} skipped={} lost={} comp_bad={} direct_bad={} first=({},{}) got={:#08x} want={:#08x} -> {}",
        lx, ly, rx, ly,
        // Where the right block's first row actually comes out under the kernel's row step. On a
        // padded boot the byte offset the direct path computed is not a whole number of kernel rows,
        // so the block lands both higher AND shifted sideways; the column is the remainder. Both
        // components, or the operator is hunting half-located.
        ((ly * geom_row) % k_row.max(1)) / 4, (ly * geom_row) / k_row.max(1),
        BLK, BLK, SCALE, pw, ph,
        k_row, geom_row, truth.alloc_pitch,
        checked, EXPECTED, skipped, direct_lost, comp_bad, direct_bad,
        first_bad.0, first_bad.1, first_bad.2, first_bad.3,
        if ok { "PASS" } else { "FAIL" }
    );
    serial_println!(
        "[wc-f] ramp origin=({},{}) rows={} byte_step={}B nominal=1px/row marks={} lost={} :: photograph the slope — a bend of d/4 px per row means the HVS row step is k_row+d, whatever the firmware reported",
        rax, ray, RAMP_ROWS, k_row + 4, RAMP_ROWS * 2, ramp_lost
    );
    // No latch here: the `compare_exchange` above already claimed it, which is what makes this half
    // single-entrant rather than merely once-reported.
}

/// The slope marker. One two-pixel mark per row, placed by PURE BYTE ARITHMETIC: each successive mark
/// sits `k_row + 4` bytes after the last. Under the kernel's assumed row step that is "one row down,
/// one pixel right", so the marks photograph as an exact 1 px/row diagonal. If the hardware's real row
/// step is `k_row + d`, each mark lands `d/4` pixels further left than intended and the diagonal bends
/// — the slope on the photograph is a direct measurement of the step the HVS actually uses, which no
/// number on the serial wire can be, since every one of those is the firmware describing itself.
///
/// Ticked white every 16th mark so rows can be counted off the photograph.
///
/// Returns the number of marks that did NOT land (offset past the mapping). The whole value of this
/// construct is what it looks like on a photograph, so "is the marker actually on the panel?" has to be
/// answerable from the log — otherwise an operator photographs a blank strip and cannot tell a clean
/// scan-out from a marker that was never drawn.
fn ramp(base: usize, len: usize, fmt: PixelFormat, k_row: usize, x0: usize, y0: usize) -> usize {
    let start = y0 * k_row + x0 * 4;
    let mut lost = 0usize;
    for i in 0..RAMP_ROWS {
        let color =
            if i % 16 == 0 { 0x00FF_FFFF } else { 0x0000_FF80 | ((i as u32 & 0xFF) << 16) };
        let px = match bytes_of(fmt, color) {
            Some(p) => p,
            None => return lost + (RAMP_ROWS - i) * 2,
        };
        let off = start + i * (k_row + 4);
        if !store3(base, len, off, px) {
            lost += 1;
        }
        if !store3(base, len, off + 4, px) {
            lost += 1;
        }
    }
    lost
}

/// The geometry half: the live `FrameBuffer` beside the firmware's recorded answers.
///
/// Note what is and is not claimed here. `stride * bpp == pitch` is an IDENTITY of `init_framebuffer`
/// (`stride = pitch / 4`), not an observation, so it is not reported as a check — it can never be
/// false, and a witness that "passes" it is decoration. The load-bearing field is `rowstep_match`:
/// `stride * bpp` against `virt_w * 4`, a row step derived from the reported GEOMETRY rather than from
/// the pitch reply. It goes false exactly when the firmware pads a row — which is the padding the
/// compositor would then be ignoring.
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
    let geom_row = truth.virt_w as usize * 4;
    // The mapping we store into IS the buffer the firmware allocated.
    let base_match = base as u64 == truth.alloc_base;
    // THE row-step question, asked against geometry rather than against the pitch reply.
    let rowstep_match = geom_row != 0 && k_row == geom_row;
    // How many bytes per row the firmware allocated beyond the visible pixels. Padding the compositor
    // does not know about is a row-phase garble, stated as a number.
    let pad = (truth.alloc_pitch as usize).saturating_sub(geom_row);
    // The allocation's pitch survives an independent, query-only re-read. Weak on its own (the same
    // firmware answering the same question) but a divergence would be decisive.
    let pitch_match = truth.get_pitch == truth.alloc_pitch;
    // The geometry `wm::place` lays windows out in is the mode being scanned.
    let panel_match = pw == truth.virt_w as usize && ph == truth.virt_h as usize;
    // The mapping covers every row the pipe scans.
    let fits = len >= truth.alloc_pitch as usize * truth.virt_h as usize;
    let ok = base_match && rowstep_match && pad == 0 && pitch_match && panel_match && fits;
    serial_println!(
        "[wc-f] scanout kernel base={:#x} len={} panel={}x{} stride={}px bpp={} k_row={}B :: firmware base={:#x} size={} alloc_pitch={}B get_pitch={}B geom_row={}B pad={}B virt={}x{} phys={}x{} off=({},{}) depth={} order={} alpha={} :: base_match={} rowstep_match={} pitch_match={} panel_match={} fits={} -> {}",
        base, len, pw, ph, stride, bpp, k_row,
        truth.alloc_base, truth.alloc_size, truth.alloc_pitch, truth.get_pitch, geom_row, pad,
        truth.virt_w, truth.virt_h, truth.phys_w, truth.phys_h, truth.off_x, truth.off_y,
        truth.depth, truth.order, truth.alpha,
        base_match, rowstep_match, pitch_match, panel_match, fits,
        if ok { "PASS" } else { "FAIL" }
    );
}

// ---------------------------------------------------------------------------
// CHROME-TRUTH — the glass-readback chrome witness
// ---------------------------------------------------------------------------

/// CHROME-TRUTH — has the readback spoken yet? Latched only by a pass that actually found chrome.
static CHROME_TRUTH_DONE: AtomicBool = AtomicBool::new(false);

/// CHROME-TRUTH — read the PANEL back at computed chrome coordinates and print want-vs-got.
///
/// ## The gap this closes
///
/// The Pi bench desktop already prints a full theme census (`[crispy] theme=us-crispy-modern …
/// title_h=34 frame=5 face=0xececee desktop=0x2d2b55`), and WC-D already proves each window's
/// CONTENT reached the scan-out buffer byte for byte. Neither instrument says one word about the
/// CHROME. `[crispy]` reports the constants the painter was COMPILED with — it fires from
/// `wm::paint_window` before a single chrome pixel is stored, and would print the identical line if
/// every chrome write below it were deleted. WC-D verifies the window's CONTENT rectangle against
/// its source surface, which by construction excludes the frame, the title strip and the discs:
/// chrome has no source surface to be compared against.
///
/// So "the wire says crispy" and "the glass shows crispy" have been, until now, two claims with no
/// wire between them — which is exactly the gap an operator standing at the bench saying *"I see no
/// crispy anything"* falls into. This closes it: after the first composite that drew a real window,
/// read the framebuffer back at coordinates DERIVED from the same geometry `paint_window` used, and
/// compare each against a colour recomputed by calling the same `theme::` / `ceramic::` /
/// `wm::title_row_color` the painter called. A `HIT` is the theme on glass. A `MISS` is the theme and
/// the glass disagreeing, with the coordinate and both colours named so it is diagnosable rather
/// than merely reported. This witness carries NO second copy of the kit's numbers, so it cannot
/// drift from the theme and it cannot pass by agreeing with itself.
///
/// ## What each probe pins, and why these five
///
/// | probe | coordinate | what a MISS would mean |
/// |---|---|---|
/// | `keyline_top` | box top edge, mid-width | the outer keyline (`theme::FRAME_LINE`, UNMACHINED — an exact constant) never landed |
/// | `bevel_light` | one `theme::BEVEL` in | the frame's hairline structure is absent |
/// | `title_top` | strip row 0, right end | the title gradient's top stop under `ceramic::shade` |
/// | `title_bot` | strip row `TITLE_H-1` | the gradient's bottom stop — the pair pins the whole ramp |
/// | `face_left` | left frame, below the strip | the machined `CHROME_FACE` surface itself |
///
/// The title probes take the strip's RIGHT end (`bw - BORDER - 2`) rather than its middle, because
/// the control cluster is LEFT-aligned and the caption follows it: mid-strip is where a glyph lives.
/// The face probe takes `bx + BORDER - 2`, inside the frame and clear of both the keyline (`kw` =
/// `theme::BEVEL` wide) and the bevel beside it.
///
/// A sixth reading, `content`, carries NO expectation and is reported as `APP`: those pixels belong
/// to a ring-3 surface and the kernel has no business predicting them. It is printed anyway because
/// "is there anything in the well at all" is the other half of what an operator is looking at.
///
/// ## The desktop reading is REPORTED BUT NOT JUDGED
///
/// On x86 the panel-wide `DESKTOP_BG` clear is `wcx`'s DESKTOP-CLEAR. **The Pi now has its twin —
/// `pidesk::activate`'s PIDESK DESKTOP-CLEAR, written because this probe named the gap** (bench
/// capture PA41 boot2: `want=0x2d2b55 got=0x1e1e1e -> NOCLEAR`, i.e. `video::PANEL_BG` still on the
/// glass at desktop-ready). It is `feature = "pidesk"` and one-shot at bring-up, so the promise is
/// conditional in two ways this witness must not paper over: knob-off there is still no clear, and
/// a clear at bring-up is not a claim about a pixel some LATER writer repaints — the Pi's
/// `render_service` still owns the backdrop (PARITY §6.1), so a probe that latches late reads
/// whatever the shell left. `wm::erase` paints `DESKTOP_BG` into window BOXES and `Screen` holds it
/// in the desktop LAYER, but neither promises a panel pixel outside every box. A witness that
/// called any of that a `MISS` would be convicting the chrome for a contract it never made. It
/// prints `NOCLEAR` instead — a distinct token, outside the verdict's arithmetic, naming which of
/// the three the boot is in. The probe point is CHOSEN by testing candidates against every row's outer box,
/// compat rows included (a compat row can own the whole panel and legitimately paint anything
/// there); when no candidate is clear it says so rather than asserting a colour it cannot justify.
///
/// ## The amplitude numbers — the line that answers the operator, not the gate
///
/// `HIT` on all five would still leave *"I see no crispy anything"* unexplained, because the honest
/// answer is not that the theme is missing but that this theme, on this panel, carries almost no
/// visible signal. So the verdict states each material's amplitude AS A NUMBER OF 8-BIT LEVELS,
/// measured rather than asserted:
///
/// - `title_grad` — `|title_top - title_bot|` READ FROM GLASS, widest channel. The kit's stops are
///   `0xEEEEF1`→`0xE3E3E8` focused and `0xF4F4F5`→`0xEFEFF1` not, i.e. ~11 and ~5 levels across the
///   whole 34-row strip.
/// - `ceramic_pp` — peak-to-peak of `ceramic::shade(CHROME_FACE, ·)` over the material's whole
///   128-row tile: the largest difference two chrome rows of one window can EVER show.
///   `GRAIN_AMP_Q16 + CURVE_AMP_Q16` = 1310/65536 = 2.0 % of a channel.
/// - `knurl_pp` — the same for the disc milling, `AMP_A + AMP_B` = 1311/65536, also 2.0 %.
/// - `paper_pp` — the same for the kit's PAPER material, which is a real texture rather than a
///   modulation, and is the loudest thing the kit has.
/// - `paper_on` — **can any paper pixel exist in this image at all?** `paper::fill_rect` has exactly
///   ONE call site in the tree, `video::instgui::repaint`, and `instgui` is `x86_64 + wc + instgui`.
///   The `paper` module itself is arch-neutral, so its constants and selftest literals ARE in the Pi
///   image and the census still names it — which is precisely how the wire can say "paper" while no
///   paper pixel has ever existed on this panel. `cfg!` of the consumer's own gate is the whole
///   truth of it, so that is what is printed.
///
/// Those five numbers are the deliverable. A boot that changes the kit's contrast, or wires paper
/// onto a Pi content surface, moves them; nobody has to take a claim about the look on trust again.
///
/// ## Cost, safety, and why it cannot perturb what it measures
///
/// One-shot for the boot, reads 6 pixels per window plus one, and WRITES NOTHING — unlike the twin
/// probe above it leaves no marks, so it cannot disturb a later photograph or a later WC-D
/// reference. It is called from the tail of `wm::composite_inner`, inside the same `drawn > 0` block
/// WC-F runs in and immediately after it, with the table guard already dropped and `rows` a
/// snapshot — the rule `[wc-c]` and WC-F already print under. Reads go through
/// `FrameBuffer::read_pixel` (volatile, and the exact inverse of `put_pixel`'s encoding, so the
/// `Bgr` layout the Pi firmware reports is decoded by the same match that encoded it); the scanlines
/// are invalidated first, on `wm::verify_window`'s pattern, so a read cannot be answered from a
/// dirty line the blit left in cache.
///
/// It does NOT latch on a pass that found no chrome-bearing row — the desktop takes seconds to reach
/// its steady population, and a witness that spent its one shot on the first pass to draw anything
/// would report on a panel nobody is looking at. Such a pass returns silently; a `SKIP` per pass
/// would be the flood the one-shot exists to avoid.
pub fn chrome_truth(fb: &FrameBuffer, rows: &[super::wm::Window], order: &[usize], focus: u64) {
    use super::wm::{outer_box, title_row_color, BORDER, DESKTOP_BG, TITLE_H};
    if CHROME_TRUTH_DONE.load(Ordering::Relaxed) {
        return;
    }
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);
    let row_bytes = info.stride * info.bytes_per_pixel;
    let (mut hits, mut probes, mut wins) = (0usize, 0usize, 0usize);
    let mut grad_max = 0u32;

    for &i in order.iter() {
        let Some(r) = rows.get(i) else { continue };
        // Compat rows have no chrome at all (`paint_window` guards every chrome write with
        // `!r.compat`), so probing one would report a MISS for a window that is CORRECTLY bare.
        if !r.used || r.compat || r.w == 0 || r.h == 0 || r.scale == 0 {
            continue;
        }
        let (bx, by, bw, bh) = outer_box(r);
        // Every probe below indexes at most `BORDER + TITLE_H + 8` rows down and `bw - BORDER - 2`
        // across; a box that cannot hold them is skipped rather than probed off its own edge.
        if bw < 2 * BORDER + 4 || bh < BORDER + TITLE_H + 10 || bx + bw > pw || by + bh > ph {
            continue;
        }
        wins += 1;
        // FOCUS — the same predicate `composite_inner` hands `draw_window`, so the gradient
        // recomputed here is the one that was painted.
        let foc = focus != 0 && focus == r.owner_asid;

        // Discard, never clean, over exactly the scanlines about to be read. Without it a read could
        // be answered from the line the blit left dirty, which would make this witness agree with
        // the compositor's INTENT instead of with the panel.
        crate::arch::cache::invalidate_range(fb.base_addr() + by * row_bytes, bh * row_bytes);

        let strip_x = bx + bw - BORDER - 2;
        let face_row = BORDER + TITLE_H + 8;
        let want: [(&str, usize, usize, u32); 5] = [
            ("keyline_top", bx + bw / 2, by, super::theme::FRAME_LINE),
            (
                "bevel_light",
                bx + bw / 2,
                by + super::theme::BEVEL,
                super::theme::BEVEL_LIGHT,
            ),
            (
                "title_top",
                strip_x,
                by + BORDER,
                super::ceramic::shade(title_row_color(0, TITLE_H, foc), BORDER),
            ),
            (
                "title_bot",
                strip_x,
                by + BORDER + TITLE_H - 1,
                super::ceramic::shade(
                    title_row_color(TITLE_H - 1, TITLE_H, foc),
                    BORDER + TITLE_H - 1,
                ),
            ),
            (
                "face_left",
                bx + BORDER - 2,
                by + face_row,
                super::ceramic::shade(super::theme::CHROME_FACE, face_row),
            ),
        ];
        let (mut got_top, mut got_bot) = (0u32, 0u32);
        for (n, (name, x, y, w)) in want.iter().enumerate() {
            let got = fb.read_pixel(*x, *y);
            probes += 1;
            let ok = got == Some(*w);
            if ok {
                hits += 1;
            }
            if n == 2 {
                got_top = got.unwrap_or(0);
            }
            if n == 3 {
                got_bot = got.unwrap_or(0);
            }
            serial_println!(
                "[chrome-truth] win={} box=({},{},{}x{}) foc={} pt={} at=({},{}) want={:#08x} got={:#08x} -> {}",
                r.id, bx, by, bw, bh,
                if foc { "yes" } else { "no" },
                name, x, y, w,
                got.unwrap_or(0),
                if ok { "HIT" } else { "MISS" }
            );
        }
        // The content well: an EL0 surface, so no expectation is possible and none is invented.
        let cpt = (bx + BORDER + 4, by + BORDER + TITLE_H + 4);
        serial_println!(
            "[chrome-truth] win={} pt=content at=({},{}) want=app got={:#08x} -> APP",
            r.id,
            cpt.0,
            cpt.1,
            fb.read_pixel(cpt.0, cpt.1).unwrap_or(0)
        );
        // The gradient the operator's eye is actually offered, measured off glass rather than
        // derived from the constants: the widest per-channel difference between the strip's first
        // and last row.
        for shift in [16u32, 8, 0] {
            grad_max = grad_max.max(((got_top >> shift) & 0xFF).abs_diff((got_bot >> shift) & 0xFF));
        }
    }

    // Nothing chrome-bearing on the panel yet. DO NOT latch — see the header.
    if wins == 0 {
        return;
    }

    let mut desk: Option<(usize, usize)> = None;
    if pw > 4 && ph > 4 {
        for cand in [
            (pw - 2, ph - 2),
            (pw - 2, 2),
            (2, ph - 2),
            (pw / 2, ph - 2),
            (pw - 2, ph / 2),
        ] {
            let covered = rows.iter().filter(|r| r.used).any(|r| {
                let (bx, by, bw, bh) = outer_box(r);
                cand.0 >= bx && cand.0 < bx + bw && cand.1 >= by && cand.1 < by + bh
            });
            if !covered {
                desk = Some(cand);
                break;
            }
        }
    }
    match desk {
        Some((dx, dy)) => {
            crate::arch::cache::invalidate_range(fb.base_addr() + dy * row_bytes, row_bytes);
            let got = fb.read_pixel(dx, dy);
            // NOT counted in `hits`/`probes`, and NOT a `FAIL` — see the header's DESKTOP note.
            let ok = got == Some(DESKTOP_BG);
            serial_println!(
                "[chrome-truth] pt=desktop at=({},{}) want={:#08x} got={:#08x} -> {}",
                dx, dy, DESKTOP_BG, got.unwrap_or(0),
                if ok { "HIT" } else if cfg!(feature = "pidesk") {
                    "NOCLEAR (pidesk cleared the panel at bring-up; a later writer owns this pixel — PARITY 6.1)"
                } else {
                    "NOCLEAR (no panel-wide DESKTOP_BG clear in this build; it is pidesk-gated on aarch64)"
                }
            );
        }
        None => serial_println!(
            "[chrome-truth] pt=desktop at=none want={:#08x} got=none -> SKIP (every candidate under a window box)",
            DESKTOP_BG
        ),
    }

    let face = super::theme::CHROME_FACE;
    let ceramic_pp = pp(super::ceramic::TILE_H, |k| super::ceramic::shade(face, k));
    let knurl_pp = pp(super::knurl::TILE_W * super::knurl::TILE_H, |k| {
        super::knurl::shade(face, k % super::knurl::TILE_W, k / super::knurl::TILE_W)
    });
    let paper_tile = super::paper::tile();
    let paper_pp = pp(paper_tile.len(), |k| paper_tile[k]);
    // The ONLY consumer of `paper::fill_rect` is `video::instgui::repaint`, under exactly this gate.
    let paper_on = cfg!(all(target_arch = "x86_64", feature = "wc", feature = "instgui"));
    serial_println!(
        "[chrome-truth] verdict wins={} hits={}/{} title_grad={} ceramic_pp={} knurl_pp={} paper_pp={} paper_on={} panel={}x{} -> {}",
        wins, hits, probes, grad_max, ceramic_pp, knurl_pp, paper_pp,
        if paper_on { "yes" } else { "no" },
        pw, ph,
        if hits == probes { "PASS" } else { "FAIL" }
    );
    // Latched only HERE — after a pass that found chrome to read and said what it read.
    CHROME_TRUTH_DONE.store(true, Ordering::Relaxed);
}

/// CHROME-TRUTH — peak-to-peak of `f` over `n` samples, in 8-bit levels, widest channel.
///
/// One helper for all three materials so the number means the same thing in each column of the
/// verdict: the largest difference the material can put between two of its own pixels. `ceramic` is
/// indexed by row, `knurl` by a flattened tile coordinate, `paper` by tile pixel — the shapes differ,
/// the question does not.
fn pp(n: usize, f: impl Fn(usize) -> u32) -> u32 {
    let mut best = 0u32;
    for shift in [16u32, 8, 0] {
        let (mut lo, mut hi) = (255u32, 0u32);
        for k in 0..n {
            let c = (f(k) >> shift) & 0xFF;
            lo = lo.min(c);
            hi = hi.max(c);
        }
        best = best.max(hi.saturating_sub(lo));
    }
    best
}
