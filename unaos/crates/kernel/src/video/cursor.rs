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
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// CURSOR-4 — a per-painted-pixel bit set over [`for_each_sprite_pixel`]'s scan order.
///
/// The sprite is no longer an all-or-nothing object: after CURSOR-4 a pass may take PART of it off
/// the panel (the part the pass is about to paint over) and deliver PART of it through a window's
/// staged present. Both of those are per-pixel facts about a ~50–800 entry set, so they are stored
/// as bits rather than as rectangles — the union of "the boxes this pass will paint" is not a box,
/// and approximating it with one is what forced CURSOR-3's whole-box decline in the first place.
const MASK_WORDS: usize = MAX_PIX.div_ceil(64);

#[derive(Clone, Copy)]
struct Bits([u64; MASK_WORDS]);

impl Bits {
    const EMPTY: Bits = Bits([0; MASK_WORDS]);
    #[inline]
    fn get(&self, i: usize) -> bool {
        i < MAX_PIX && self.0[i / 64] & (1u64 << (i % 64)) != 0
    }
    #[inline]
    fn set(&mut self, i: usize) {
        if i < MAX_PIX {
            self.0[i / 64] |= 1u64 << (i % 64);
        }
    }
    #[inline]
    fn clear(&mut self, i: usize) {
        if i < MAX_PIX {
            self.0[i / 64] &= !(1u64 << (i % 64));
        }
    }
    fn reset(&mut self) {
        self.0 = [0; MASK_WORDS];
    }
}

/// CURSOR-4 — is panel point `(x, y)` inside any of `boxes`?
fn in_any(boxes: &[(usize, usize, usize, usize)], x: usize, y: usize) -> bool {
    boxes.iter().any(|&(bx, by, bw, bh)| x >= bx && y >= by && x < bx + bw && y < by + bh)
}


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
    /// CURSOR-4 — painted-pixel indices this sprite is NOT currently on the panel at, because a
    /// masked undraw ([`undraw_within`]) put the original back ahead of a compositor pass that is
    /// about to paint those pixels. `saved[i]` is STALE for exactly these indices: the pixel has
    /// been handed back, and whatever lands there next belongs to the painter, not to us.
    ///
    /// Meaningful only while `drawn`. Every full [`undraw_locked`] / [`draw_locked`] cycle resets it,
    /// so the two states cannot drift: `drawn && off.get(i)` is "hole", `drawn && !off.get(i)` is
    /// "ours, and `saved[i]` says what it covers".
    off: Bits,
    /// CURSOR-4 — bumped by every full undraw and every draw. An overlay plan carries the generation
    /// it was taken at, and a plan whose generation no longer matches describes a sprite that has
    /// since been taken down and put back somewhere else — so its layer-derived save-under must not
    /// be installed. This is what makes the split sprite safe against a concurrent [`repaint`]:
    /// the mismatch is detected rather than merged.
    epoch: u64,
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
    off: Bits::EMPTY,
    epoch: 0,
    unsupported: false,
});

/// CURSOR-5 — [`Sprite::epoch`], mirrored where a lock-free reader can see it.
///
/// ### The hole CURSOR-4 left, and why an atomic is the only shape that closes it
/// [`compose_into`] runs inside `wm`'s `BlitGuard` window, so it may not take `SPRITE` (F4's drain
/// barrier spins IRQ-masked until every registered blit retires, and a second blocking lock in that
/// wait set breaks its termination argument). It therefore validated the plan against [`OVERLAY`]'s
/// OWN copy of the geometry and generation — which is a copy of the plan, so the test could only ever
/// answer "is this the plan the session was opened with", never "does that plan still describe the
/// sprite".
///
/// Between `overlay_open` and `compose_into` the sprite can be taken off the panel by a full
/// [`undraw`], from three callers that owe nothing to the pass: `wm::erase` on another core,
/// `wm::drain_deferred` (which ran INSIDE the bracket until CURSOR-5 moved it ahead of it), and a
/// [`repaint`] from the render task. Every one of those bumps the generation. `compose_into` could
/// not see the bump, so it painted the sprite into the layer and the present put it on the panel —
/// while the sprite module believed itself off-panel. The next [`draw_locked`] then read the front
/// for its save-under, captured the overlay's own `FILL`, and the arrow stood in the window's rect
/// until something else damaged that window. **That is the P64 "flash in the vug display".**
///
/// One `u64` load is admissible everywhere a lock is not: no wait, and no entry in the drain
/// barrier's wait set.
///
/// **What this check is, stated honestly (lens SHOULD-FIX 3).** An earlier draft claimed "a stale
/// read is a conservative read". That is FALSE, and the falsity is worth naming rather than
/// softening: the failure mode is not "the reader sees a newer value than it should", it is "the
/// reader sees an OLD value that still equals `plan.epoch`" — which is precisely the retired plan the
/// check exists to reject, waved through. The generation check therefore **narrows** the window
/// between a retiring undraw and a compose; it does not close it. It cannot: any lock-free test of a
/// value that another core is concurrently changing has a residual, and this one is not permitted a
/// lock.
///
/// What closes the residual is the layer BEHIND it, which was always there and is what makes the
/// narrowing worth having rather than a false floor: `adopt_overlay` re-checks `ov.epoch == sp.epoch`
/// with the sprite lock HELD, so a compose that slipped through on a stale read is caught at the tail
/// and settled by a whole-sprite `refresh_locked` instead of by an install. The check turns a
/// certainty into a race, and the tail turns the race into a repaint. `C5_SELF_SAVE` is what would
/// show the residual actually biting.
///
/// **Release/Acquire, not Relaxed.** The store is what publishes "the sprite is no longer where your
/// plan says", and the thing a reader must not reorder against it is the undraw's PIXEL WRITES. A
/// relaxed pair leaves an acquiring reader free to observe the new generation while still seeing
/// pre-undraw pixels (or, worse, to observe the old generation after the pixels have moved) — so the
/// ordering is doing real work here even though the value itself is only advisory.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// CURSOR-5 — the sprite generation, readable without the sprite lock. Only [`compose_into`] needs
/// it; every other consumer holds `SPRITE` and reads the field directly. `Acquire`, paired with
/// [`bump_epoch`]'s `Release` — see [`EPOCH`] for what the ordering buys and what it does not.
fn live_epoch() -> u64 {
    EPOCH.load(Ordering::Acquire)
}

/// CURSOR-5 — advance the generation. The ONE place `epoch` moves, so the mirror cannot drift from
/// the field: a bump that forgot the atomic would leave `compose_into` trusting a retired plan, which
/// is the defect this arc exists to close.
fn bump_epoch(sp: &mut Sprite) {
    sp.epoch = sp.epoch.wrapping_add(1);
    EPOCH.store(sp.epoch, Ordering::Release);
}

// ---- CURSOR-5 witnesses ------------------------------------------------------------------------
//
// The residual has to be VISIBLE in replay or the next bench verdict is another adjective. Each
// counter names one mechanism, and each is zero on a boot where the mechanism did not run — which on
// QEMU raspi4b is all of them, since no HID pointer report ever arrives and the sprite is never
// drawn. The rollup says `UNWITNESSED` in exactly that case rather than `CLEAN`.

/// A [`compose_into`] that declined because the plan's generation no longer matched the live sprite —
/// mechanism A, caught rather than painted. Non-zero here means the interleave still HAPPENS and is
/// now being absorbed; it is a measure of contention, not of damage.
#[cfg(feature = "witness")]
static C5_STALE_COMPOSE: AtomicU64 = AtomicU64::new(0);

/// An [`adopt_overlay`] that found its session incoherent with the sprite (generation or geometry
/// moved mid-pass) and fell back to the whole-sprite refresh. Before CURSOR-5 this was the branch
/// that stamped the arrow; it is now the branch that merely costs a repaint.
#[cfg(feature = "witness")]
static C5_ADOPT_INCOH: AtomicU64 = AtomicU64::new(0);

/// A save-under that read back EXACTLY the colour the sprite paints at that pixel, while the module
/// believed the sprite was not on the panel there. An UPPER BOUND on self-capture, not a proof of it:
/// window content is free to contain `FILL`-white or `SHADOW`-dark pixels of its own, and this cannot
/// tell those apart from our own arrow. A boot where it stays 0 has provably not stamped; a boot
/// where it climbs in step with `stale_compose` is showing the mechanism still leaking.
#[cfg(feature = "witness")]
static C5_SELF_SAVE: AtomicU64 = AtomicU64::new(0);

/// Passes that lost the overlay session to another core and took a MASKED undraw anyway (CURSOR-5's
/// change of shape for the lock class — see [`undraw_within`]'s caller in `wm::composite_inner`).
/// Every one of these is a whole-sprite bracket that did not happen.
#[cfg(feature = "witness")]
static C5_MASKED_NOSESSION: AtomicU64 = AtomicU64::new(0);

/// WC-L's drain observed an OPEN overlay session when it was about to undraw. The direct detector for
/// the same-core half of mechanism A; it must be 0, because CURSOR-5 moved the drain ahead of the
/// bracket. A non-zero count means someone reordered `composite_inner` and re-opened the hole.
#[cfg(feature = "witness")]
static C5_DRAIN_INSESSION: AtomicU64 = AtomicU64::new(0);

/// CURSOR-5 — count a masked undraw taken without owning the session (called from `wm`).
#[cfg(feature = "witness")]
pub fn note_masked_nosession() {
    C5_MASKED_NOSESSION.fetch_add(1, Ordering::Relaxed);
}

/// CURSOR-5 — does the drain's own pass hold an overlay session right now? For WC-L's drain, which
/// must not take the sprite down inside one.
///
/// ### The invariant is PER-PASS, and the first cut tested it globally (lens SHOULD-FIX 2)
/// CURSOR-5's ordering fix says: *this* composite pass drains before *this* pass opens a session. It
/// says nothing whatever about another core, and another core legitimately holding a session while
/// this one drains is not a defect — it is the VUGPAR steady state, two cores compositing at once,
/// which is the load this whole arc is about. The first cut tested the GLOBAL session flag and
/// counted a contended `try_lock` as busy, so a perfectly healthy metal boot would have driven
/// `drain_insession` up, tripped the spec's `FORBID`, and reported `REGRESSED`. A false red on P65
/// costs Peter a bench boot to chase a bug that is not there, which is a worse failure than the
/// counter not existing.
///
/// So the test is scoped to the core that opened the session. A pass runs `composite_inner` on one
/// core and its drain is on that same core, so "the open session belongs to this core" is exactly
/// "this pass is mid-session" — the invariant, and nothing wider. Cross-core sessions are invisible
/// here by construction, which is correct: they are absorbed by [`compose_into`]'s generation check
/// and counted as `stale_compose`, where they read as load rather than as breakage.
///
/// `try_lock`, and a contended lock counts NOTHING: this core cannot be the one holding `OVERLAY`
/// (nothing on this path holds it across the drain), so contention proves the holder is someone
/// else — the case that must not count. Blocking would also put `OVERLAY` into a wait the drain does
/// not need.
#[cfg(feature = "witness")]
pub fn note_drain_undraw() {
    let mine = match OVERLAY.try_lock() {
        Some(g) => g.session && g.owner_cpu == crate::arch::sched::meter_current_cpu(),
        None => false,
    };
    if mine {
        C5_DRAIN_INSESSION.fetch_add(1, Ordering::Relaxed);
    }
}

/// CURSOR-5 — the arc's rollup, printed by `wm` immediately after `[cursor3]`'s so the decline
/// breakdown and the coherence residual read as one block.
#[cfg(feature = "witness")]
pub fn cursor5_rollup(scope: &str) {
    let stale = C5_STALE_COMPOSE.load(Ordering::Relaxed);
    let incoh = C5_ADOPT_INCOH.load(Ordering::Relaxed);
    let selfsave = C5_SELF_SAVE.load(Ordering::Relaxed);
    let masked = C5_MASKED_NOSESSION.load(Ordering::Relaxed);
    let drain = C5_DRAIN_INSESSION.load(Ordering::Relaxed);
    // `drain_insession` is the only line item that is a DEFECT rather than a cost: the reorder in
    // `composite_inner` makes it structurally impossible, so a non-zero count is a regression in the
    // ordering, not a symptom of load.
    let verdict = if drain > 0 {
        "REGRESSED"
    } else if !armed() {
        "UNWITNESSED"
    } else if selfsave > 0 {
        "RESIDUAL"
    } else {
        "COHERENT"
    };
    serial_println!(
        "[cursor5] rollup scope={} stale_compose={} adopt_incoh={} selfsave={} masked_nosession={} drain_insession={} -> {}",
        scope, stale, incoh, selfsave, masked, drain, verdict
    );
}

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
        sp.off.reset();
        bump_epoch(sp);
        return None;
    }
    let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
    let off = sp.off;
    let saved = &sp.saved;
    for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
        // CURSOR-4: a pixel already handed back by a masked undraw is not ours and `saved[i]` is
        // stale for it. Restoring it here would stamp pre-pass content over whatever the compositor
        // has since put there — the exact stale-restore hazard the colour guard exists to prevent,
        // except that the colour guard cannot see it (the compositor may legitimately have painted
        // `FILL` there, and more importantly the guard would pass on any pixel the pass left alone).
        if off.get(i) {
            return;
        }
        if i < saved.len() && fb.read_pixel(x, y) == Some(color) {
            fb.put_pixel(x, y, saved[i]);
        }
    });
    flush_box(&fb, by, bh);
    sp.drawn = false;
    sp.off.reset();
    bump_epoch(sp);
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

/// CURSOR-3 — the sprite's geometry as one snapshot, for a compositor that is going to paint it
/// itself. [`sprite_box`] answers "could this pass touch the sprite?"; this answers "and at exactly
/// what geometry, at what block scale" — atomically, under one acquisition, because a box taken at
/// one instant and a scale taken at another would not describe any sprite that ever existed.
///
/// A snapshot, never a handle, with the same degradation as [`sprite_box`]: the pointer can move the
/// moment the lock drops, and [`adopt_overlay`] re-tests the position before it trusts the plan.
#[derive(Clone, Copy)]
pub struct Plan {
    pub bx: usize,
    pub by: usize,
    pub bw: usize,
    pub bh: usize,
    pub s: usize,
    /// CURSOR-4 — the sprite generation this plan was taken at. Every consumer re-checks it, so a
    /// plan that outlived the sprite it describes is discarded rather than applied.
    pub epoch: u64,
}

/// CURSOR-3 — the sprite's current geometry, or `None` when it is not on the panel.
pub fn sprite_plan() -> Option<Plan> {
    let sp = SPRITE.lock();
    if sp.drawn {
        Some(Plan { bx: sp.bx, by: sp.by, bw: sp.bw, bh: sp.bh, s: sp.s, epoch: sp.epoch })
    } else {
        None
    }
}

/// CURSOR-4 — take off the panel ONLY the sprite pixels that fall inside `boxes`, and leave the rest
/// exactly where they are.
///
/// ### Why the bracket had to become partial
/// CURSOR-3's bracket is all-or-nothing: if any window overlaps the sprite's box, the WHOLE sprite
/// comes off the panel for the length of the pass. For a sprite resting ON a window's border that is
/// the worst of both worlds — the part hanging over the desktop is taken down even though nothing in
/// the pass is ever going to write there, and it is that part, blinking at present rate, that Peter
/// still sees at P62.
///
/// The undraw exists for one reason: a pixel some painter in this pass is about to overwrite must be
/// handed back BEFORE the overwrite, or the save-under would be stale and the restore would stamp
/// pre-pass content into a window's rect. That reason applies per PIXEL, not per sprite. `boxes` is
/// the compositor's conservative union of the extents it may paint (every live window above the
/// shell whose outer box meets the sprite), so a pixel outside all of them is a pixel no painter in
/// this pass can reach — and taking it down buys nothing and costs a visible hole.
///
/// Pixels that ARE handed back are recorded in [`Sprite::off`]; the pass's tail
/// ([`adopt_overlay`]) is what owes every one of them a pixel again, either from a window's staged
/// present or from a save-and-draw against the finished front buffer.
///
/// Returns the rect that was written, for [`repair`]. Conservative (the whole box) rather than the
/// touched subset — `repair` marks windows damaged, and a narrow rect would be a narrower repair.
pub fn undraw_within(boxes: &[(usize, usize, usize, usize)]) {
    let restored = {
        let mut sp = SPRITE.lock();
        undraw_within_locked(&mut sp, boxes).0
    };
    repair(restored);
}

/// CURSOR-5 — the masked undraw for a pass that does NOT own the overlay session, which is a
/// different operation from [`undraw_within`] however identical the pixel work looks.
///
/// ### The interleave this exists to close (lens MUST-FIX 1)
/// CURSOR-5's first cut let a session-less pass call [`undraw_within`] directly, on the argument that
/// the mask's justification (hand back what THIS pass may paint over) is independent of who owns the
/// session. The pixel argument is sound; the BOOKKEEPING one is not, and the gap reintroduced both
/// P64 symptoms on the new path:
///
/// * Pass A owns the session, composed sprite pixel `P` into its layer and presented it. `P` is on
///   the panel, `ov.saved[P]` holds the window content beneath it, `A`'s plan generation is `e`.
/// * Pass B, having lost the session, mask-undraws `P`. The colour guard passes (the panel really
///   does hold the sprite's colour at `P` — A just put it there), so B writes `sp.saved[P]` — B's
///   OWN save-under, captured before A's window ever composed — into A's window rect. A stale pixel,
///   inside a live window: **the flash**.
/// * `undraw_within_locked` does not bump the generation, so `sp.epoch` is still `e`. A's
///   `adopt_overlay` therefore finds its session COHERENT, installs `ov.saved[P]` over B's damage and
///   clears the off bit — the module now believes the sprite is on the panel at `P`, where B has just
///   painted something else. **The hole.**
///
/// CURSOR-3 and CURSOR-4 were immune only by accident: their decline branch took a FULL undraw, which
/// bumps the generation, so A's session went incoherent and fell back to a whole-sprite refresh.
///
/// ### The fix, and why the generation is the right lever
/// Any pixel actually handed back here is a pixel whose ownership has changed behind the session
/// owner's back, and the generation is precisely the channel that says "the sprite you planned
/// against is not the sprite that exists". Bumping it makes A's `compose_into` decline (`stale`),
/// A's `adopt_overlay` incoherent, and A's tail a whole-sprite `refresh_locked` — the exact,
/// already-proven fallback CURSOR-3's full undraw bought by accident, now bought on purpose and only
/// when a pixel really moved.
///
/// The bump is conditional on `handed_back > 0` rather than unconditional, and that is not an
/// optimisation: the overwhelmingly common case is a pass whose paint set does not meet the sprite at
/// all (or meets only pixels a previous mask already took), which changes nothing and must not
/// invalidate a healthy session. A pass that hands nothing back has not disturbed A's premise.
///
/// The alternative the lens offered — clearing the open session's `covered` bits for the masked
/// extents — was rejected as strictly weaker: it repairs the bookkeeping (the hole) but leaves the
/// stale pixel B already wrote inside A's window rect, since A would then repaint `P` from the front
/// where B's stale value is sitting. Only invalidating the generation reaches both.
pub fn undraw_within_nosession(boxes: &[(usize, usize, usize, usize)]) {
    let restored = {
        let mut sp = SPRITE.lock();
        let (restored, handed_back) = undraw_within_locked(&mut sp, boxes);
        if handed_back > 0 {
            bump_epoch(&mut sp);
        }
        restored
    };
    repair(restored);
}

/// Returns the rect to [`repair`], and how many painted pixels this call newly handed back — the
/// second is what [`undraw_within_nosession`] keys its generation bump on.
fn undraw_within_locked(
    sp: &mut Sprite,
    boxes: &[(usize, usize, usize, usize)],
) -> (Option<(usize, usize, usize, usize)>, usize) {
    if !sp.drawn {
        return (None, 0);
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        // The full undraw bumps the generation itself, so the caller's conditional bump would be
        // redundant — and harmless either way, since it reports zero pixels handed back.
        return (undraw_locked(sp), 0);
    }
    let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
    let mut off = sp.off;
    let mut handed_back = 0usize;
    {
        let saved = &sp.saved;
        for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
            if off.get(i) || !in_any(boxes, x, y) {
                return;
            }
            handed_back += 1;
            // Same colour guard as the full undraw, and for the same reason: a pixel another painter
            // has already taken is theirs, and putting our saved value back would be the stale
            // restore. The bit is set either way — guarded or not, this pixel is no longer ours.
            if i < saved.len() && fb.read_pixel(x, y) == Some(color) {
                fb.put_pixel(x, y, saved[i]);
            }
            off.set(i);
        });
    }
    sp.off = off;
    flush_box(&fb, by, bh);
    (Some((bx, by, bw, bh)), handed_back)
}

/// WC-I — the sprite's CURRENT panel box, or `None` when it is not on the panel.
///
/// The compositor uses it to decide whether a pass needs the cursor bracket at all. Before WC-I every
/// `composite` took the sprite off the panel and put it back, unconditionally — and `composite` runs
/// once per window present, from the presenting task's own core. With several high-rate windows the
/// sprite spent most of its life mid-restore, on one core or another, and the colour guard in
/// [`undraw_locked`] means a pixel another painter has taken is simply not put back: the panel showed
/// a sprite with holes in it that moved from report to report. That is the "spotty/flickery cursor"
/// of the P60 bench run, and it is a cost paid by every present regardless of where the pointer is.
///
/// A snapshot, never a handle: the sprite can move the instant the lock is dropped. The compositor
/// uses it only to answer "could this pass touch the sprite?", and a stale answer degrades to an
/// unnecessary bracket (the pre-WC-I behaviour), never to a missed one — a sprite that MOVED between
/// this read and the draw was moved by `repaint`, which redraws it on top afterwards.
pub fn sprite_box() -> Option<(usize, usize, usize, usize)> {
    sprite_plan().map(|p| (p.bx, p.by, p.bw, p.bh))
}

/// WC-I — put the sprite on the panel if it is not already there, and do nothing at all if it is.
///
/// The tail of a composite that did NOT disturb the sprite. [`repaint`] would restore-then-redraw at
/// the same position — a full save-under cycle per present, which is exactly the churn WC-I removes —
/// whereas this is one lock acquisition and a boolean test in the common case. The case where it does
/// work is the one that needs it: `wm::erase` takes the sprite down and leaves it to the composite
/// that follows to put it back.
pub fn ensure_drawn() {
    let mut armed_at: Option<(i32, i32)> = None;
    let mut unsupported_now = false;
    {
        let mut sp = SPRITE.lock();
        if sp.drawn || sp.unsupported || !crate::pal::cursor::visible() {
            return;
        }
        match draw_locked(&mut sp) {
            Ok(pos) => armed_at = pos,
            Err(()) => unsupported_now = true,
        }
    }
    if unsupported_now && !UNSUPPORTED_REPORTED.swap(true, Ordering::Relaxed) {
        serial_println!("[cursor] disabled: panel format has no read-back inverse");
    }
    if let Some((px, py)) = armed_at {
        serial_println!("[cursor] armed x={} y={}", px, py);
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
        refresh_locked(&mut sp, &mut armed_at, &mut unsupported_now)
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

/// The restore → save → draw sequence, with the lock held. The body [`repaint`] and
/// [`adopt_overlay`]'s fallback share, so there is exactly one implementation of "put the sprite
/// where the pointer is now" and both callers report the same three outputs (the restored rect for
/// [`repair`], the first-draw position for the witness, and the unsupported latch).
fn refresh_locked(
    sp: &mut Sprite,
    armed_at: &mut Option<(i32, i32)>,
    unsupported_now: &mut bool,
) -> Option<(usize, usize, usize, usize)> {
    let restored = undraw_locked(sp);
    if crate::pal::cursor::visible() && !sp.unsupported {
        match draw_locked(sp) {
            Ok(pos) => *armed_at = pos,
            Err(()) => *unsupported_now = true,
        }
    }
    restored
}

// ---- CURSOR-3: composite-through ---------------------------------------------------------------

/// CURSOR-3 — the sprite as the window back-layer left it: geometry, and what it covered THERE.
///
/// ### The case WC-I left open, and why it is structural
/// WC-I stopped `composite` from bracketing the sprite when the pointer is nowhere near a window.
/// Over a window the bracket is still taken — and it is taken once per present, ~60 times a second,
/// from the presenting task's core. Between the `undraw` and the `repaint` sits the whole of
/// `draw_window`: a full off-screen compose plus `bh` row copies, milliseconds during which the
/// sprite is simply NOT on the panel. That is a duty cycle, not a race, and no amount of care inside
/// the bracket shortens it. Peter's P61 verdict is exactly its shape: solid over the desktop (no
/// bracket at all after WC-I), spotty over a live vug (bracket per present).
///
/// ### The mechanism: ride the present instead of racing it
/// WC-H already composes each window into a cached-RAM back layer and presents it as contiguous
/// rows. If the sprite is painted INTO that layer after the window is composed and before the rows
/// are copied, then the cursor reaches the panel inside the same copies the window does — one
/// present, atomic to the same degree the window's own pixels are, with no undraw phase and no
/// interval in which those pixels are window-only. The flicker has nowhere left to live.
///
/// The save-under has to come from the layer for the same reason: the pixels the sprite hides are
/// the WINDOW's pixels, and at overlay time they exist only in the layer — the front still holds the
/// previous frame. Reading them from the front afterwards (what [`draw_locked`] does) would capture
/// the sprite's own `FILL`, and the next restore would stamp a white arrow permanently into the
/// window's rect.
///
/// ### CURSOR-4 — the straddling sprite, and the provenance argument that makes it sound
/// CURSOR-3 took the overlay only when the sprite's box lay ENTIRELY inside one window's clipped
/// outer box, and declined otherwise, because "which pixels came from the layer and which from the
/// front" is per-pixel bookkeeping whose failure mode is a white arrow stamped into a window. The
/// P62 wire says that decline is 42% of offers, and a pointer resting ON a window border straddles
/// on every single pass — so the decline is not a rare miss, it IS the flicker.
///
/// CURSOR-4 does the bookkeeping, explicitly, with each pixel's provenance named:
///
/// * **In-layer pixels** — saved from the BACK LAYER and painted into it, exactly as CURSOR-3 does.
///   The layer holds this window's freshly composed content and nothing else, so the save is the
///   window's own pixel and can never be the sprite's `FILL`: the layer is private to this pass, the
///   sprite has never been written to it before [`compose_into`] runs, and `paint_window` fills the
///   whole clipped box densely just above.
/// * **Out-of-layer pixels** — never read from the front HERE. They are left off the panel by the
///   masked undraw and settled by [`adopt_overlay`], AFTER every window in the pass has presented,
///   by reading the finished front buffer. That read is sound for the same reason [`draw_locked`]'s
///   is: at that instant the sprite is provably not on those pixels (the masked undraw took them
///   down and nothing has put them back), so the save-under cannot capture our own fill.
/// * **Pixels no painter in this pass can reach** — never taken down at all, so there is nothing to
///   save and nothing to restore. This is the class that carries most of the fix: a sprite hanging
///   off a window edge over bare desktop now simply stays on the panel through the present.
///
/// The dangerous middle case — a pixel one window composes and a HIGHER window then overwrites
/// directly — is closed by [`overlay_uncover`]: every window that paints its box without composing
/// the sprite into it clears the coverage bits inside that box, and windows are drawn back-to-front,
/// so the topmost painter of each pixel is the one whose verdict survives. A pixel that ends the
/// pass uncovered is a pixel the tail repaints from the front, which is CURSOR-3's bracket narrowed
/// to exactly the pixels that still need it.
///
/// ### One session per pass, and why a second concurrent pass declines instead of merging
/// Coverage accumulates across the several windows of ONE pass, so it cannot be a last-writer-wins
/// slot any more. `session` makes the accumulation single-owner: the pass that opens it owns the
/// overlay until its tail closes it, and a second `composite` on another core finds it busy, takes
/// no plan, and runs CURSOR-3's whole-sprite bracket. Merging two passes' coverage instead would let
/// pass B's reset erase pass A's bits, and pass A's tail would then "restore" pixels that already
/// carry the sprite from B's layer — reading `FILL` as the under-pixel and stamping the arrow.
struct Overlay {
    /// CURSOR-4 — a pass owns the overlay from [`overlay_open`] to [`adopt_overlay`].
    session: bool,
    /// CURSOR-5 — the core the owning pass runs on. Diagnostic only: it exists so
    /// [`note_drain_undraw`] can tell "this pass is mid-session" (a defect) from "another core is
    /// mid-session" (the VUGPAR steady state), which the session flag alone cannot. Nothing in the
    /// mechanism reads it.
    owner_cpu: usize,
    /// The sprite generation the session was opened at (see [`Sprite::epoch`]).
    epoch: u64,
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    s: usize,
    /// CURSOR-4 — painted-pixel indices whose `saved` entry has been filled FROM A BACK LAYER and
    /// whose pixel that layer's present has delivered to the panel.
    covered: Bits,
    /// What the BACK LAYER held under each covered pixel: the window's own composed content.
    saved: [u32; MAX_PIX],
}

/// CURSOR-4 — what one window's overlay offer did, for the compositor's witness.
#[derive(Clone, Copy, Default)]
pub struct Composed {
    /// Painted sprite pixels composed into this window's layer.
    pub taken: usize,
    /// Painted sprite pixels that fell outside this window's box — the straddle, now carried by the
    /// tail rather than costing the whole sprite its overlay.
    pub missed: usize,
    /// The plan lock was held by another core; this offer did nothing at all.
    pub locked: bool,
    /// CURSOR-5 — the plan's generation no longer matched the live sprite, so the sprite was NOT
    /// composed into this layer. See [`EPOCH`]: this is the flash mechanism, declined.
    pub stale: bool,
}

/// CURSOR-3 — the published plan. Separate from [`SPRITE`] on purpose: [`compose_into`] runs from
/// inside `wm`'s `BlitGuard` window, and F4's drain barrier spins IRQ-masked until every registered
/// blit retires. A `lock()` there would put a second blocking wait into that termination argument.
/// This one is only ever `try_lock`ed from that side — the same discipline, and for the same reason,
/// that WC-H's `STAGE` uses — so a contended pass simply declines the overlay and falls back to
/// WC-I's bracket. The SPRITE lock is never taken inside the guard at all.
///
/// **Lock order: `SPRITE` → `OVERLAY`.** [`adopt_overlay`] takes both, in that order, outside the
/// guard; [`compose_into`] takes only this one. No cycle exists and none may be introduced.
static OVERLAY: Mutex<Overlay> = Mutex::new(Overlay {
    session: false,
    owner_cpu: 0,
    epoch: 0,
    bx: 0,
    by: 0,
    bw: 0,
    bh: 0,
    s: 0,
    covered: Bits::EMPTY,
    saved: [0; MAX_PIX],
});

/// CURSOR-4 — open the pass's overlay session, or report that another pass owns it.
///
/// Called from `composite_inner` BEFORE the masked undraw and before the `BlitGuard` is registered,
/// so the decision "this pass will split the sprite" is made once and everything downstream follows
/// it. `false` means the caller must fall back to CURSOR-3's whole-sprite bracket: full undraw,
/// `Repaint` tail, no plan handed to any window.
///
/// `lock()` rather than `try_lock()` is admissible here for the same reason `sprite_plan()`'s is —
/// this runs outside the guard, so this lock is not in F4's drain wait set. Inside the guard the
/// overlay is only ever `try_lock`ed.
pub fn overlay_open(plan: &Plan) -> bool {
    let mut ov = OVERLAY.lock();
    if ov.session {
        return false;
    }
    ov.session = true;
    ov.owner_cpu = crate::arch::sched::meter_current_cpu();
    ov.epoch = plan.epoch;
    ov.bx = plan.bx;
    ov.by = plan.by;
    ov.bw = plan.bw;
    ov.bh = plan.bh;
    ov.s = plan.s;
    ov.covered.reset();
    true
}

/// CURSOR-4 — does `ov` still describe the sprite `plan` was taken from?
fn overlay_matches(ov: &Overlay, plan: &Plan) -> bool {
    ov.session
        && ov.epoch == plan.epoch
        && (ov.bx, ov.by, ov.bw, ov.bh, ov.s) == (plan.bx, plan.by, plan.bw, plan.bh, plan.s)
}

/// CURSOR-4 — this window painted its box and did NOT compose the sprite into it, so every sprite
/// pixel inside that box belongs to the window now. Clearing the coverage bits is what makes the
/// tail repaint them from the front instead of trusting a lower window's layer save that this
/// window's pixels have since overwritten.
///
/// Called for every drawn window that overlaps the sprite and did not take the overlay — the direct
/// (unstaged) path, a window an instrument excluded, a compat row, and a contended plan lock alike.
/// `try_lock` only: this runs inside the `BlitGuard` window. A contended clear is safe to skip
/// because the only writer that could be holding it is another pass, which has already declined to
/// share the session — its coverage is not ours to correct.
pub fn overlay_uncover(plan: &Plan, bx: usize, by: usize, bw: usize, bh: usize) {
    let mut ov = match OVERLAY.try_lock() {
        Some(g) => g,
        None => return,
    };
    if !overlay_matches(&ov, plan) {
        return;
    }
    let boxes = [(bx, by, bw, bh)];
    let mut covered = ov.covered;
    for_each_sprite_pixel(plan.bx, plan.by, plan.bw, plan.bh, plan.s, |x, y, _c, i| {
        if in_any(&boxes, x, y) {
            covered.clear(i);
        }
    });
    ov.covered = covered;
}

/// CURSOR-3 — paint the sprite into a staged back layer, saving what it covers FROM THAT LAYER.
///
/// `layer` is `wm`'s back buffer for one window; `(ox, oy)` is the panel coordinate its origin sits
/// at. `plan` is the geometry the compositor snapshotted (and undrew) before it registered the blit.
/// Returns `true` when the layer now carries the sprite and a plan has been published for
/// [`adopt_overlay`] to install; `false` means nothing was written and the caller owes the sprite its
/// ordinary [`repaint`].
///
/// Takes NO framebuffer lock and NOT the sprite lock — every input is either an argument or the
/// layer itself, which the caller already owns exclusively through WC-H's `STAGE` guard.
/// CURSOR-4 — now PARTIAL: the sprite is clipped to the layer, the intersecting pixels ride this
/// window's present, and the remainder is left to the tail. See [`Overlay`] for the provenance
/// argument each class rests on.
pub fn compose_into(layer: &FrameBuffer, ox: usize, oy: usize, plan: Plan) -> Composed {
    let mut out = Composed::default();
    if plan.s == 0 || plan.bw == 0 || plan.bh == 0 {
        return out;
    }
    let li = layer.info();
    // The layer's extent IS the window's clipped outer box, at panel origin `(ox, oy)`. A sprite
    // pixel is this window's iff it lands inside it.
    let inside = |x: usize, y: usize| -> Option<(usize, usize)> {
        if x < ox || y < oy {
            return None;
        }
        let (lx, ly) = (x - ox, y - oy);
        if lx < li.width && ly < li.height { Some((lx, ly)) } else { None }
    };
    let mut ov = match OVERLAY.try_lock() {
        Some(g) => g,
        None => {
            out.locked = true;
            return out;
        }
    };
    if !overlay_matches(&ov, &plan) {
        return out;
    }
    // CURSOR-5 — and does that plan still describe the SPRITE? `overlay_matches` compares the plan
    // against the session's copy OF THE PLAN, so it cannot answer this; [`EPOCH`] can, without a
    // lock. A mismatch means the sprite has been taken off the panel (or moved) since this pass
    // opened its session — `wm::erase` on another core, a render-task `repaint`, or, before CURSOR-5
    // reordered it, WC-L's drain from inside this very pass. Composing now would put the arrow on the
    // panel behind the module's back, and the next save-under would capture it: the P64 flash.
    //
    // Declining is total, not partial: nothing is written to the layer, and the coverage bits inside
    // this window's box are cleared so the tail repaints those pixels from the finished front. That
    // is CURSOR-3's fallback for one window, which is always available and always correct.
    if live_epoch() != plan.epoch {
        let mut covered = ov.covered;
        for_each_sprite_pixel(plan.bx, plan.by, plan.bw, plan.bh, plan.s, |x, y, _c, i| {
            if inside(x, y).is_some() {
                covered.clear(i);
            }
        });
        ov.covered = covered;
        #[cfg(feature = "witness")]
        C5_STALE_COMPOSE.fetch_add(1, Ordering::Relaxed);
        out.stale = true;
        return out;
    }

    // Pass one: save the layer's own pixel under every sprite pixel this window owns. Runs to
    // completion before a single pixel is written, exactly as `draw_locked` does, so a failure
    // partway leaves the window's composed content untouched.
    let mut failed = false;
    let mut mine = Bits::EMPTY;
    let mut taken = 0usize;
    let mut missed = 0usize;
    {
        let saved = &mut ov.saved;
        for_each_sprite_pixel(plan.bx, plan.by, plan.bw, plan.bh, plan.s, |x, y, _c, i| {
            let Some((lx, ly)) = inside(x, y) else {
                missed += 1;
                return;
            };
            if failed || i >= saved.len() {
                failed = true;
                return;
            }
            match layer.read_pixel(lx, ly) {
                Some(orig) => {
                    saved[i] = orig;
                    mine.set(i);
                    taken += 1;
                }
                None => failed = true,
            }
        });
    }
    if failed {
        // This window's box reverts to "the window's own pixels": nothing was written to the layer,
        // and the tail owes those sprite pixels a save-and-draw against the front.
        let mut covered = ov.covered;
        for_each_sprite_pixel(plan.bx, plan.by, plan.bw, plan.bh, plan.s, |x, y, _c, i| {
            if inside(x, y).is_some() {
                covered.clear(i);
            }
        });
        ov.covered = covered;
        return Composed { taken: 0, missed, locked: false, stale: false };
    }

    for_each_sprite_pixel(plan.bx, plan.by, plan.bw, plan.bh, plan.s, |x, y, color, _i| {
        if let Some((lx, ly)) = inside(x, y) {
            layer.put_pixel(lx, ly, color);
        }
    });
    // Accumulate, do not replace: several windows of one pass may each carry part of one sprite, and
    // back-to-front order means a later window's verdict for a shared pixel is the one that lands.
    for w in 0..MASK_WORDS {
        ov.covered.0[w] |= mine.0[w];
    }
    out.taken = taken;
    out.missed = missed;
    out
}

/// The panel origin the sprite WOULD be drawn at right now — [`draw_locked`]'s clip, without the
/// draw. Used to decide whether a published plan still describes where the pointer is.
fn current_origin() -> Option<(usize, usize)> {
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return None;
    }
    let info = fb.info();
    if info.width == 0 || info.height == 0 {
        return None;
    }
    let (px, py) = crate::pal::cursor::pos(info.width as i32, info.height as i32);
    Some((
        (px.max(0) as usize).min(info.width - 1),
        (py.max(0) as usize).min(info.height - 1),
    ))
}

/// CURSOR-3 — the tail of a composite that carried the sprite through a staged present.
///
/// The panel already holds the sprite: it arrived inside the window's row copies. What is missing is
/// the sprite MODULE's knowledge of that fact, and installing it is the whole of the common case —
/// no framebuffer write at all, which is what makes this cheaper than the bracket it replaces as
/// well as steadier.
///
/// Two things are then normalised, both under the same acquisition:
/// * **A sprite another core drew in the meantime** is taken down first. Its `saved` is front-derived
///   and correct for the pixels it covers, so restoring it before installing the plan leaves the
///   panel holding exactly one sprite — ours, the one the present put there.
/// * **A pointer that moved** since the plan was taken means the panel carries the sprite at the OLD
///   origin. Installing the plan first is what makes that recoverable: the module now knows those
///   pixels are the sprite's and what the window had under them, so [`refresh_locked`]'s undraw puts
///   the window's pixels back and redraws at the new origin — instead of a save-under capturing the
///   overlay's own `FILL` and stamping an arrow into the window for the rest of the boot.
pub fn adopt_overlay() {
    let mut armed_at: Option<(i32, i32)> = None;
    let mut unsupported_now = false;
    let want = current_origin();
    let restored = {
        let mut sp = SPRITE.lock();
        // CURSOR-4 — the session closes here, unconditionally, whatever the pass managed. `composite`
        // routes every exit through its tail, so this is the one place that can release it, and a
        // session that outlived its pass would lock the mechanism out for the rest of the boot.
        let coherent = {
            let mut ov = OVERLAY.lock();
            let coherent = ov.session
                && sp.drawn
                && ov.epoch == sp.epoch
                && (ov.bx, ov.by, ov.bw, ov.bh, ov.s) == (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
            if coherent {
                // Install the layer-derived save-under for every pixel a present delivered. Those
                // pixels are on the panel already — this is bookkeeping, not painting, which is what
                // makes the composed path cheaper than the bracket as well as steadier.
                let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
                let covered = ov.covered;
                let src = &ov.saved;
                let mut off = sp.off;
                let dst = &mut sp.saved;
                for_each_sprite_pixel(bx, by, bw, bh, s, |_x, _y, _c, i| {
                    if covered.get(i) && i < dst.len() && i < src.len() {
                        dst[i] = src[i];
                        off.clear(i);
                    }
                });
                sp.off = off;
            }
            ov.session = false;
            ov.covered.reset();
            #[cfg(feature = "witness")]
            if !coherent {
                C5_ADOPT_INCOH.fetch_add(1, Ordering::Relaxed);
            }
            coherent
        };
        // The sprite's own generation moved under us (another core ran a full `repaint` mid-pass), or
        // the pointer has moved since the plan was taken, or it has gone invisible: none of the
        // per-pixel state describes the panel any more, so fall back to the whole-sprite refresh.
        // `undraw_locked` skips the off-panel pixels rather than stamping their stale saves, and
        // `draw_locked` then re-establishes the sprite entirely from the finished front buffer.
        if coherent
            && want == Some((sp.bx, sp.by))
            && crate::pal::cursor::visible()
            && !sp.unsupported
        {
            // The straddle remainder: pixels the masked undraw took down that no layer delivered —
            // over bare desktop, over a window that declined the overlay, or over a window this pass
            // never drew. The front buffer is finished now, so a save-and-draw here has exactly
            // `draw_locked`'s provenance, and it touches strictly FEWER front pixels than CURSOR-3's
            // bracket, which repainted the whole sprite.
            redraw_off_locked(&mut sp, &mut unsupported_now);
            None
        } else {
            refresh_locked(&mut sp, &mut armed_at, &mut unsupported_now)
        }
    };
    if unsupported_now && !UNSUPPORTED_REPORTED.swap(true, Ordering::Relaxed) {
        serial_println!("[cursor] disabled: panel format has no read-back inverse");
    }
    if let Some((px, py)) = armed_at {
        serial_println!("[cursor] armed x={} y={}", px, py);
    }
    repair(restored);
}

/// CURSOR-4 — put back the sprite pixels that are still off the panel, saving each from the FRONT.
///
/// Runs only from [`adopt_overlay`]'s aligned branch, i.e. with the pass finished and the sprite's
/// geometry unchanged since the plan was taken. Every pixel it touches is one the masked undraw
/// handed back and nothing has re-delivered, so reading the front cannot read our own fill.
fn redraw_off_locked(sp: &mut Sprite, unsupported_now: &mut bool) {
    let mut any = false;
    for w in 0..MASK_WORDS {
        if sp.off.0[w] != 0 {
            any = true;
            break;
        }
    }
    if !any {
        return;
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return;
    }
    let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
    let mut off = sp.off;
    let mut failed = false;
    {
        let saved = &mut sp.saved;
        for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
            if !off.get(i) || failed || i >= saved.len() {
                return;
            }
            match fb.read_pixel(x, y) {
                Some(orig) => {
                    // CURSOR-5 — same detector as `draw_locked`'s, and the same argument: `off` says
                    // this pixel was handed back and nothing has re-delivered it, so our own fill has
                    // no business being here.
                    #[cfg(feature = "witness")]
                    if orig == color {
                        C5_SELF_SAVE.fetch_add(1, Ordering::Relaxed);
                    }
                    let _ = color;
                    saved[i] = orig;
                }
                None => failed = true,
            }
        });
    }
    if failed {
        // Same fail-closed rule as `draw_locked`: a panel we cannot read back from gets no cursor.
        // The pixels stay off (they hold the compositor's content, which is correct) and the next
        // `refresh_locked` will find `unsupported` and leave the sprite down for good.
        sp.unsupported = true;
        *unsupported_now = true;
        return;
    }
    for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
        if off.get(i) {
            fb.put_pixel(x, y, color);
            off.clear(i);
        }
    });
    sp.off = off;
    flush_box(&fb, by, bh);
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
        for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
            if failed || i >= saved.len() {
                return;
            }
            match fb.read_pixel(x, y) {
                Some(orig) => {
                    // CURSOR-5 — the self-capture signature. The sprite is provably not on the panel
                    // here (the undraw above took it down, or it was never up), so a pixel that
                    // already holds the exact colour we are about to paint is either window content
                    // that happens to match or an arrow some other writer put there behind our back.
                    // Counted, not acted on: we cannot tell the two apart, and refusing the save
                    // would leave the pixel unrestorable, which is strictly worse than one frame of
                    // white. The count is what makes the residual legible in replay.
                    #[cfg(feature = "witness")]
                    if orig == color {
                        C5_SELF_SAVE.fetch_add(1, Ordering::Relaxed);
                    }
                    let _ = color;
                    saved[i] = orig;
                }
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
    // CURSOR-4: a full draw re-establishes the whole sprite from the front, so nothing is off-panel
    // and every `saved` entry is fresh. The generation bump retires any overlay plan still in flight.
    sp.off.reset();
    bump_epoch(sp);

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
