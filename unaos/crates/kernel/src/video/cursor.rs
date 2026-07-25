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
//! The sprite's box is at most one glyph cell plus one shadow block — 36 px square at the 4× cap —
//! and only the pixels the arrow actually PAINTS are saved (~50 of them at scale 1), never the whole
//! box. Saving those and putting them back is a few dozen words of copy; the alternative (marking the
//! box damaged and driving a desktop + window recomposite per pointer report) would run a composite
//! pass at HID report rate, ~125 Hz, for a sprite that moved three pixels. The save-under is the
//! smallest correct form for this present path — and under WC-E it runs on every desktop flush too,
//! which is why the mask, not the box, is what gets read back.
//!
//! **The race, and the two things that close it.** The front framebuffer has no single owner: the
//! console's `Screen::flush`, the compositor (on any core, from syscall context) and this module all
//! write to it. Under WC-E the compositor repaints the window layer on every desktop flush, so a
//! window can land on top of a drawn sprite routinely, not exceptionally — and a naive restore would
//! then stamp PRE-window pixels back INTO that window's rect, possibly inside a rect
//! `wm::verify_window` is about to read. So the restore is (1) **colour-guarded** — a pixel is put
//! back only if the panel still holds the exact colour the sprite painted there — and (2) **repaired**
//! — every window the restored rect overlaps is marked damaged, so the next composite redraws it from
//! its source surface. Neither alone is sufficient; together the restore cannot leave a window's rect
//! wrong for longer than one frame. See [`undraw_locked`] and [`repair`].
//!
//! Atomicity is the other half: every entry point holds the sprite lock across its whole
//! restore → save → draw sequence, so two cores cannot interleave into "save captured the arrow".
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

/// 8×8 arrow mask, MSB = leftmost pixel. The same SHAPE `pal::cursor` draws, so the pointer is
/// recognisably the same arrow on the full-screen demo paths and on the desktop — but not the same
/// SIZE: `pal::cursor` magnifies by `scale + 1` (a deliberate step above the text scale on a
/// full-screen demo), this module by `scale`, which makes the desktop sprite exactly one glyph cell.
/// The two never coexist on a panel — the demos own the screen while they run — so the difference is
/// a size change across modes, not two cursors at once.
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
///
/// **One lock, held across a whole operation.** Every public entry point takes this mutex ONCE and
/// holds it for the entire restore → save → draw sequence. An earlier cut had `repaint` call
/// `undraw` (which took and released the lock) and then re-acquire it for the save; in that gap
/// another core could draw the sprite, the save would capture THE ARROW as "what was underneath",
/// and the next undraw would stamp a white arrow permanently into the desktop or a window. The
/// private `*_locked` helpers exist so the outer call can keep the guard.
struct Sprite {
    /// Whether the sprite is currently ON the panel (and `saved` holds what it covered).
    drawn: bool,
    /// Origin and extent of the drawn box, in panel pixels (clipped to the panel, never shifted).
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    /// Block scale the box was drawn at — the mask is recomputed from it on restore.
    s: usize,
    /// The original pixel under each pixel the sprite PAINTED, in the scan order
    /// [`for_each_sprite_pixel`] walks. Only painted pixels are saved: the rest of the box was never
    /// modified, so restoring it would be a write — and a race — with nothing to fix.
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
    s: 0,
    saved: [0; MAX_PIX],
    unsupported: false,
});

/// CURSOR-1 witness latch — `[cursor] armed` prints once, at the first draw.
static ARMED: AtomicBool = AtomicBool::new(false);

/// One-shot latch for the unsupported-panel line, so it is printed outside the sprite lock.
static UNSUPPORTED_REPORTED: AtomicBool = AtomicBool::new(false);

/// Whether the system cursor has ever been drawn (i.e. a pointer device has reported).
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Block magnification, derived from the panel — THE METRICS RULE. The arrow is 8 blocks square (one
/// glyph cell, `cell_w` × `cell_h`); the shadow adds one more block in each direction.
fn block_scale(fb: &FrameBuffer) -> usize {
    crate::ui::Metrics::for_height(fb.info().height).scale
}

/// The colour the sprite paints at box-relative `(col, row)`, or `None` where it paints nothing.
///
/// Each pixel is answered ONCE, with its FINAL colour: the fill is tested first, so a pixel both the
/// shadow and the fill cover reads as `FILL` — which is what ends up on the panel, and therefore what
/// a restore must match against. That single-answer property is why save and restore can walk the
/// same scan order and pair up entry for entry with no per-pixel bookkeeping.
fn sprite_color(s: usize, col: usize, row: usize) -> Option<u32> {
    let hit = |c: usize, r: usize| -> bool {
        c < crate::ui::BASE_CELL * s
            && r < crate::ui::BASE_CELL * s
            && ARROW[r / s] & (0x80 >> (c / s)) != 0
    };
    if hit(col, row) {
        Some(FILL)
    } else if col >= s && row >= s && hit(col - s, row - s) {
        Some(SHADOW)
    } else {
        None
    }
}

/// Walk every pixel the sprite paints, in a fixed scan order, calling `f(x, y, colour, index)`.
fn for_each_sprite_pixel(
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    s: usize,
    mut f: impl FnMut(usize, usize, u32, usize),
) {
    let mut i = 0usize;
    for row in 0..bh {
        for col in 0..bw {
            if let Some(color) = sprite_color(s, col, row) {
                f(bx + col, by + row, color, i);
                i += 1;
            }
        }
    }
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

/// Take the sprite off the panel, with the lock already held. Returns the rect that was restored, for
/// the caller to hand to [`repair`] once the lock is released.
///
/// **The restore is colour-guarded, and that guard is half the fix for the stale-restore hazard.**
/// Between our draw and this restore another painter may have written into the sprite's pixels —
/// under WC-E that is routine, not hypothetical: every desktop flush repaints the window layer.
/// Writing the saved pixel back blindly would stamp PRE-window content into a window's rect, inside a
/// rect `wm::verify_window` may still be about to read — a `[wc-d] -> FAIL`, which the Pi spec
/// FORBIDs, and which nothing would repair on its own, since a composite repaints damaged windows and
/// not arbitrary rows. So each pixel is restored only if the framebuffer still holds the exact colour
/// the sprite painted there; anything else means another painter has taken that pixel and owns it.
///
/// The residual hole is narrow and named: a painter whose new content happens to be exactly `FILL` or
/// `SHADOW` at one of our pixels is indistinguishable from our own sprite, and that pixel would be
/// restored to stale content. [`repair`] closes it.
fn undraw_locked(sp: &mut Sprite) -> Option<(usize, usize, usize, usize)> {
    if !sp.drawn {
        return None;
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        // Unreachable in practice: `drawn` is only ever set by `draw_locked`, which returns early
        // unless the framebuffer is ready, and the framebuffer is initialised once and never torn
        // down. Handled anyway so the state can never become "drawn, with no way to undraw"; the
        // saved patch is dropped because there is no surface left to restore it into.
        sp.drawn = false;
        return None;
    }
    let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
    let saved = &sp.saved;
    for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
        if i < saved.len() && fb.read_pixel(x, y) == Some(color) {
            fb.put_pixel(x, y, saved[i]);
        }
    });
    flush_box(&fb, by, bh);
    sp.drawn = false;
    Some((bx, by, bw, bh))
}

/// Restore the pixels the sprite is covering and forget them. A no-op when the sprite is not on the
/// panel, so every painter may call it unconditionally.
///
/// Called by [`super::wm::composite`], `wm`'s desktop erase, and the render task around its
/// `Screen::flush` — i.e. by everything that writes to the front framebuffer.
pub fn undraw() {
    let restored = {
        let mut sp = SPRITE.lock();
        undraw_locked(&mut sp)
    };
    repair(restored);
}

/// Hand a restored rect back to the compositor: every window it overlaps is marked damaged, so the
/// next composite redraws that window from its source surface and discards anything the restore may
/// have put there. This is the other half of the stale-restore fix — it is what stops a restore
/// inside a composite bracket from leaving a verified rect wrong for longer than one frame.
///
/// **Marks only — it never calls `composite`.** `composite` brackets itself with this module, so
/// compositing from here would recurse. Under WC-E the repair is serviced within one desktop frame
/// (`Screen::flush` → `wm::repaint` → `composite`, ~20 fps on the bench); without WC-E, at the next
/// present of any window.
///
/// **Lock order, stated once: `SPRITE` → `TABLE`, never the reverse.** This runs with the sprite lock
/// RELEASED, and nothing in `wm` calls into this module while holding the window table — both
/// `composite`'s bracket and `erase`'s undraw run outside it. Any future caller must keep that order.
fn repair(restored: Option<(usize, usize, usize, usize)>) {
    if let Some((x, y, w, h)) = restored {
        super::wm::damage_intersecting(x, y, w, h);
    }
}

/// Draw the system cursor at the pointer's current position, saving what it covers first.
///
/// Undraws any previous sprite under the SAME lock acquisition, so the whole restore → save → draw
/// sequence is atomic against another core doing the same thing. Silently does nothing while the
/// pointer is hidden ([`crate::pal::cursor::visible`]): before the first pointer report of the boot,
/// and again ~1.5 s after the last one.
pub fn repaint() {
    let mut armed_at: Option<(i32, i32)> = None;
    let mut unsupported_now = false;
    let restored = {
        let mut sp = SPRITE.lock();
        let restored = undraw_locked(&mut sp);
        if crate::pal::cursor::visible() && !sp.unsupported {
            match draw_locked(&mut sp) {
                Ok(pos) => armed_at = pos,
                Err(()) => unsupported_now = true,
            }
        }
        restored
    };
    // Serial output happens with the sprite lock RELEASED: on a build where fbcon is still attached
    // `serial_println!` paints the framebuffer mirror, which is another writer to the panel — one
    // that would otherwise run with the sprite on it, and under our own lock.
    if unsupported_now && !UNSUPPORTED_REPORTED.swap(true, Ordering::Relaxed) {
        serial_println!("[cursor] disabled: panel format has no read-back inverse");
    }
    if let Some((px, py)) = armed_at {
        serial_println!("[cursor] armed x={} y={}", px, py);
    }
    repair(restored);
}

/// Save-under and draw, with the lock held. `Ok(Some(pos))` when this was the first draw of the boot
/// (the caller prints the witness), `Ok(None)` on any later draw, `Err(())` when the panel format has
/// no read-back inverse and the cursor must be disabled for the boot.
fn draw_locked(sp: &mut Sprite) -> Result<Option<(i32, i32)>, ()> {
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return Ok(None);
    }
    let info = fb.info();
    let s = block_scale(&fb);
    let side = (crate::ui::BASE_CELL + 1) * s;
    if s == 0 || side > MAX_SIDE || info.width == 0 || info.height == 0 {
        return Ok(None);
    }

    // The hot spot IS the box origin — the arrow's tip is its top-left pixel. The box is CLIPPED to
    // the panel, never shifted: shifting it inward would move the drawn tip away from
    // `pal::cursor::pos`, which is what `click1_dispatch` hit-tests, so a click near the right or
    // bottom edge would land up to `side - 1` px from the arrow the operator aimed with. Clipping
    // keeps the tip exactly on the hot spot and simply draws less of the tail. (`pal::cursor` clamps
    // the position to the panel, so the origin is always on-screen and the clipped box is never
    // empty.)
    let (px, py) = crate::pal::cursor::pos(info.width as i32, info.height as i32);
    let bx = (px.max(0) as usize).min(info.width.saturating_sub(1));
    let by = (py.max(0) as usize).min(info.height.saturating_sub(1));
    let bw = side.min(info.width - bx);
    let bh = side.min(info.height - by);

    // Save-under, PAINTED PIXELS ONLY (~50 reads at scale 1, against 1296 for the whole box). The
    // per-frame cost matters here because WC-E composites on every desktop flush, which brackets this
    // module ~20 times a second on the bench.
    let mut failed = false;
    {
        let saved = &mut sp.saved;
        for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, _c, i| {
            if failed || i >= saved.len() {
                return;
            }
            match fb.read_pixel(x, y) {
                Some(orig) => saved[i] = orig,
                None => failed = true,
            }
        });
    }
    if failed {
        // A single unreadable pixel disables the cursor for the rest of the boot rather than leaving
        // a patch on the panel that cannot be put back.
        sp.unsupported = true;
        return Err(());
    }
    sp.bx = bx;
    sp.by = by;
    sp.bw = bw;
    sp.bh = bh;
    sp.s = s;
    sp.drawn = true;

    for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, _i| {
        fb.put_pixel(x, y, color);
    });
    flush_box(&fb, by, bh);

    // CURSOR-1 witness: once, at the first draw of the boot. Input-driven by construction (nothing
    // reaches here before a pointer report), so quiet boot is preserved and the QEMU gate — which has
    // no HID pointer — never prints it. Emitted by the caller, outside the lock.
    if !ARMED.swap(true, Ordering::Relaxed) {
        return Ok(Some((px, py)));
    }
    Ok(None)
}
