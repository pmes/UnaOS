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

//! CURSOR-1 — the SYSTEM CURSOR: a compositor-drawn sprite in the real scan-out buffer.
//!
//! ### Why this is not `pal::cursor`
//! [`crate::pal::cursor`] owns the pointer's **state**: the shared hot-spot position (moved by
//! `move_rel` / `set_abs` from the event stream) and the auto-hide clock (`visible()`). It also
//! draws — but it draws through a [`GneissPal`](crate::pal::GneissPal), i.e. into whatever surface
//! the caller owns. On the full-screen demo paths that surface *is* the frame, so the sprite lands
//! on top of everything and the arrangement works.
//!
//! It does not work for the windowed desktop. The Pi render task draws into a [`Screen`] back
//! buffer and flushes damaged rectangles forward, while [`wm::composite`](super::wm::composite)
//! blits windows **directly into the front framebuffer**. A sprite drawn into the back buffer is
//! therefore not on top of anything the compositor painted — it is on top of the console only, and
//! only until the next window present. This module draws the sprite where "on top" is a fact rather
//! than a hope: into the front buffer, as the last painter of every pass.
//!
//! ### The contract, in three calls
//! * [`undraw`] — restore the pixels the sprite is covering and forget them. **Every** painter that
//!   is about to write to the front framebuffer calls this first.
//! * [`repaint`] — save what is under the hot spot and draw the sprite there. Called last, by the
//!   same painters, once their pixels are down.
//! * [`armed`] — whether the sprite has ever been drawn (the `[cursor]` witness's latch).
//!
//! `repaint` is `undraw`-then-draw, so the pair is idempotent and a painter that calls only
//! `repaint` is still correct; the separate `undraw` exists so the sprite is *off* the panel while
//! another painter (and, crucially, `wm::verify_window`) looks at those pixels.
//!
//! ### Damage: save-under, not a full recomposite
//! The sprite's box is at most one glyph cell plus one shadow block — 36 px square at the 4× cap.
//! Saving those pixels and putting them back is a few hundred words of copy; the alternative
//! (marking the box damaged and driving a desktop + window recomposite per pointer report) would
//! run a composite pass at HID report rate, ~125 Hz, for a sprite that moved three pixels. The
//! save-under is the smallest correct form for this present path.
//!
//! **The one race, stated plainly.** The front framebuffer has no single owner: the console's
//! `Screen::flush`, the compositor, and this module all write to it, and only the render task's own
//! sequence is ordered. If another core paints into the sprite's box between our draw and our
//! `undraw`, the restore puts back pixels that are no longer current — a stale patch at most one
//! cell across, repaired by that painter's next pass. This is the same non-exclusive-framebuffer
//! caveat `wm::verify_window` documents for its rect, and the reason `undraw` is called by the
//! painters rather than left to a timer.
//!
//! ### Checksum safety (CURSOR-1's hard requirement)
//! The sprite must not perturb `[wc-c]`'s per-window checksum, `[wc-d]`'s scan-out verdict, the
//! UVUG present checksum, or the `kernel8-test` capture. Two independent reasons it cannot:
//!
//! 1. **Ordering.** `wm::composite` calls [`undraw`] before it takes the window table lock and
//!    [`repaint`] only after the last `draw_window` / `verify_window` has returned. No verified
//!    pixel is ever read with the sprite on the panel. (`[wc-c]`'s checksum reads the *source*
//!    surface, which this module never touches at all.)
//! 2. **Arming.** The sprite is drawn only while [`crate::pal::cursor::visible()`] — i.e. only
//!    after a real pointer report has arrived. QEMU raspi4b delivers no HID pointer input, so on
//!    the gate this module writes zero pixels and prints nothing, for the whole boot.
//!
//! ### THE METRICS RULE
//! No pixel count is named here. The block scale is [`Metrics::scale`](crate::ui::Metrics::scale)
//! and the arrow is 8×8 blocks, so the sprite is **exactly one glyph cell** (`cell_w` × `cell_h`) —
//! the derivation `ui.rs` already states for the text cursor — plus a one-block drop shadow that
//! keeps it visible over light and dark content alike.

use super::FrameBuffer;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// 8×8 arrow mask, MSB = leftmost pixel. The same shape `pal::cursor` draws, so the pointer looks
/// the same on the full-screen demo paths and on the desktop.
const ARROW: [u8; 8] = [
    0b1000_0000,
    0b1100_0000,
    0b1110_0000,
    0b1111_0000,
    0b1111_1000,
    0b1111_1100,
    0b1101_1000,
    0b1000_1100,
];

/// Arrow fill.
const FILL: u32 = 0x00FF_FFFF;
/// Drop shadow, one block down-right of the fill.
const SHADOW: u32 = 0x0010_1014;

/// The sprite's box is `8 * scale` (one glyph cell) plus one `scale` block of shadow, so the
/// save-under buffer is sized for the scale cap. Derived, not chosen: `(8 + 1) * SCALE_MAX`.
const MAX_SIDE: usize = (crate::ui::BASE_CELL + 1) * crate::ui::SCALE_MAX;
const MAX_PIX: usize = MAX_SIDE * MAX_SIDE;

/// Sprite state. `drawn` is the only thing that decides whether `saved` means anything.
struct Sprite {
    /// Whether the sprite is currently ON the panel (and `saved` holds what it covered).
    drawn: bool,
    /// Origin and extent of the saved/drawn box, in panel pixels.
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    /// The pixels the sprite is covering, row-major over `bw` × `bh`.
    saved: [u32; MAX_PIX],
    /// Fail-closed latch: a panel whose format has no colour inverse (`read_pixel` returns `None`,
    /// e.g. the lossy `U8` layout) cannot be saved from, so the sprite is never drawn on it. Better
    /// no cursor than a trail of wrongly-restored pixels across the desktop.
    unsupported: bool,
}

static SPRITE: Mutex<Sprite> = Mutex::new(Sprite {
    drawn: false,
    bx: 0,
    by: 0,
    bw: 0,
    bh: 0,
    saved: [0; MAX_PIX],
    unsupported: false,
});

/// CURSOR-1 witness latch — `[cursor] armed` prints once, at the first draw.
static ARMED: AtomicBool = AtomicBool::new(false);

/// Whether the system cursor has ever been drawn (i.e. a pointer device has reported).
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Block magnification and the sprite's box, derived from the panel — THE METRICS RULE.
fn geometry(fb: &FrameBuffer) -> (usize, usize) {
    let s = crate::ui::Metrics::for_height(fb.info().height).scale;
    // 8 blocks of arrow + 1 block of shadow offset.
    (s, (crate::ui::BASE_CELL + 1) * s)
}

/// Clean the panel rows the box spans, so the non-coherent HVS sees the change. Whole scanlines at
/// the panel's stride — the same discipline `wm::draw_window` and `Screen::flush` use.
fn flush_box(fb: &FrameBuffer, y: usize, h: usize) {
    let info = fb.info();
    let row_bytes = info.stride * info.bytes_per_pixel;
    let y0 = y.min(info.height);
    let y1 = (y + h).min(info.height);
    if y1 > y0 {
        fb.flush_range(y0 * row_bytes, (y1 - y0) * row_bytes);
    }
}

/// Restore the pixels the sprite is covering and forget them. A no-op when the sprite is not on the
/// panel, so every painter may call it unconditionally.
///
/// Called by [`super::wm::composite`], [`super::wm`]'s desktop erase, and the render task around its
/// `Screen::flush` — i.e. by everything that writes to the front framebuffer.
pub fn undraw() {
    let mut sp = SPRITE.lock();
    if !sp.drawn {
        return;
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        sp.drawn = false;
        return;
    }
    for row in 0..sp.bh {
        for col in 0..sp.bw {
            fb.put_pixel(sp.bx + col, sp.by + row, sp.saved[row * sp.bw + col]);
        }
    }
    flush_box(&fb, sp.by, sp.bh);
    sp.drawn = false;
}

/// Draw the system cursor at the pointer's current position, saving what it covers first.
///
/// Undraws any previous sprite, so this is the single call a painter needs after its own pixels are
/// down. Silently does nothing while the pointer is hidden ([`crate::pal::cursor::visible`]): before
/// the first pointer report of the boot, and again ~1.5 s after the last one.
pub fn repaint() {
    undraw();
    if !crate::pal::cursor::visible() {
        return;
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return;
    }
    let info = fb.info();
    let (s, side) = geometry(&fb);
    if side == 0 || side > MAX_SIDE || info.width == 0 || info.height == 0 {
        return;
    }
    // Hot spot = the arrow's tip. Clamp the BOX to the panel so the sprite is never half off the
    // edge with a save-under that spans rows it never wrote.
    let (px, py) = crate::pal::cursor::pos(info.width as i32, info.height as i32);
    let bx = (px.max(0) as usize).min(info.width.saturating_sub(side));
    let by = (py.max(0) as usize).min(info.height.saturating_sub(side));
    if bx + side > info.width || by + side > info.height {
        return; // panel smaller than one sprite — nothing sane to draw
    }

    let mut sp = SPRITE.lock();
    if sp.unsupported {
        return;
    }
    // Save-under. A single unreadable pixel disables the cursor for the rest of the boot rather
    // than leaving a box we cannot put back.
    for row in 0..side {
        for col in 0..side {
            match fb.read_pixel(bx + col, by + row) {
                Some(px) => sp.saved[row * side + col] = px,
                None => {
                    sp.unsupported = true;
                    serial_println!("[cursor] disabled: panel format has no read-back inverse");
                    return;
                }
            }
        }
    }
    sp.bx = bx;
    sp.by = by;
    sp.bw = side;
    sp.bh = side;
    sp.drawn = true;

    // Shadow first, then the fill one block up-left of it: legible over dark and light alike.
    // Merged horizontal runs (≤ 2 rects per glyph row) rather than one fill per set bit.
    for &(ox, oy, color) in &[(s, s, SHADOW), (0, 0, FILL)] {
        for (row, bits) in ARROW.iter().enumerate() {
            let mut col = 0usize;
            while col < 8 {
                if bits & (0x80 >> col) != 0 {
                    let mut run = 1usize;
                    while col + run < 8 && bits & (0x80 >> (col + run)) != 0 {
                        run += 1;
                    }
                    fb.fill_rect(bx + col * s + ox, by + row * s + oy, run * s, s, color);
                    col += run;
                } else {
                    col += 1;
                }
            }
        }
    }
    flush_box(&fb, by, side);

    // CURSOR-1 witness: once, at the first draw of the boot. Input-driven by construction (nothing
    // reaches here before a pointer report), so quiet boot is preserved and the QEMU gate — which
    // has no HID pointer — never prints it.
    if !ARMED.swap(true, Ordering::Relaxed) {
        serial_println!("[cursor] armed x={} y={}", px, py);
    }
}
