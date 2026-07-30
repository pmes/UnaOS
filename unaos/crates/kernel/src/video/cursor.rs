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
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
    /// CURSOR-11 — painted-pixel indices the open pass DEFERRED handing back: the arrow is still on
    /// the panel at them, and a painter in this pass may be about to overwrite them.
    ///
    /// This is the third pixel class, and it is what removes the blink over a PRESENTING window.
    /// `off` says "handed back, `saved[i]` is stale, someone owes this pixel a redraw"; `pend` says
    /// "NOT handed back — the arrow is on glass and `saved[i]` is still true of the panel — but the
    /// pass's tail owes it a VERDICT before it may be trusted again". The two are disjoint by
    /// construction ([`defer_within`] skips anything already `off`, and [`settle_pending_locked`]
    /// skips anything that became `off` under it).
    ///
    /// The tail ([`adopt_overlay`]) resolves every bit, and only ever one of three ways:
    ///  * the pixel rode a window's staged present (`Overlay::covered`) — `saved[i]` takes the
    ///    LAYER's content, which is exactly what the freshly presented rows now hold beneath the
    ///    arrow, and the bit clears with no panel write at all;
    ///  * the panel still holds our colour — nobody painted there, `saved[i]` was never invalidated,
    ///    and the bit clears with no panel write either;
    ///  * the panel holds something else — a painter took the pixel, so `saved[i]` is re-taken from
    ///    the FINISHED front and the arrow is put back over it.
    ///
    /// Meaningful only while `drawn`, like `off`, and reset by every full [`undraw_locked`] /
    /// [`draw_locked`] cycle for the same reason.
    pend: Bits,
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
    pend: Bits::EMPTY,
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

// ---- CURSOR-6 — the LIVE BOX, and the painters that meet it -------------------------------------
//
// P65v2 established that every CURSOR-5 mechanism is silent on metal while the symptom survives, so
// the next mechanism is not in the paths those counters watch. The one fact none of them can observe
// is the one the panel shows: **a painter wrote front-buffer pixels that the sprite was occupying,
// and the sprite module never found out**. Every existing counter is taken from inside the sprite's
// own bookkeeping (plans, sessions, generations, coverage bits); an overwrite by a painter that never
// consulted the module leaves that bookkeeping perfectly self-consistent and the arrow off the panel.
// `COHERENT` and "spotty" are not in contradiction, which is exactly why P65v2 read as a dead end.
//
// Measuring it needs the sprite's box readable from inside `wm`'s `BlitGuard` window and from the
// desktop's row loop — neither of which may take `SPRITE` (the guard's drain barrier spins IRQ-masked
// until every registered blit retires, and a blocking sprite lock there is exactly the wait F4's
// termination argument excludes). So the box is MIRRORED into relaxed atomics beside `EPOCH`, on the
// same discipline and with the same honesty about what a lock-free read can promise: the answer may
// be one motion stale, which degrades a counter's precision and nothing else. Nothing in the
// mechanism reads it — it is diagnostic only, and no pixel decision is taken from it.

/// CURSOR-6 — is the sprite on the panel right now? Mirrors [`Sprite::drawn`].
static LIVE_ON: AtomicBool = AtomicBool::new(false);
/// CURSOR-6 — the mirrored panel box (`bx`, `by`, `bw`, `bh`).
static LIVE_BOX: [AtomicUsize; 4] =
    [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

/// CURSOR-6 — publish the sprite's box for the lock-free readers. Called wherever [`Sprite::drawn`]
/// becomes true, so the mirror cannot drift from the field.
fn publish_box(sp: &Sprite) {
    // Down first, ALWAYS. The four coordinates are four separate stores, so a reader that caught
    // them mid-update could assemble a box that never existed — new `bx` with old `bw`. Clearing the
    // flag first makes that window unobservable rather than merely unlikely, and it costs one
    // relaxed store on a path that already does five. The invariant is then unconditional: a `Some`
    // answer from `live_box_relaxed` is always a box some sprite really occupied.
    LIVE_ON.store(false, Ordering::Relaxed);
    LIVE_BOX[0].store(sp.bx, Ordering::Relaxed);
    LIVE_BOX[1].store(sp.by, Ordering::Relaxed);
    LIVE_BOX[2].store(sp.bw, Ordering::Relaxed);
    LIVE_BOX[3].store(sp.bh, Ordering::Relaxed);
    LIVE_ON.store(true, Ordering::Release);
}

/// CURSOR-6 — the sprite has left the panel entirely (a FULL undraw). A masked undraw deliberately
/// does NOT retract: part of the sprite is still up, and a reader that concluded "no sprite" would
/// undercount exactly the straddle case this arc is trying to see.
fn retract_box() {
    LIVE_ON.store(false, Ordering::Release);
}

/// CURSOR-6 — the sprite's panel box without taking the sprite lock, for painters inside `wm`'s blit
/// guard and the desktop's row loop. `None` means the sprite is provably not on the panel; `Some` is
/// an advisory box that may be one pointer report stale.
pub fn live_box_relaxed() -> Option<(usize, usize, usize, usize)> {
    if !LIVE_ON.load(Ordering::Acquire) {
        return None;
    }
    let b = (
        LIVE_BOX[0].load(Ordering::Relaxed),
        LIVE_BOX[1].load(Ordering::Relaxed),
        LIVE_BOX[2].load(Ordering::Relaxed),
        LIVE_BOX[3].load(Ordering::Relaxed),
    );
    if b.2 == 0 || b.3 == 0 { None } else { Some(b) }
}

/// CURSOR-6 — a window present whose front-buffer row blit covered part of the live sprite box while
/// the pass held **no overlay plan at all**, so nothing had handed those pixels back first. This is
/// the spotty mechanism stated as a measurement: they held the arrow before the blit and hold window
/// content after it, no path tells the sprite module, and nothing repaints them until the next
/// pointer report.
#[cfg(feature = "witness")]
static C6_PRESENT_OVER: AtomicU64 = AtomicU64::new(0);

/// CURSOR-6 — the DENOMINATOR for [`C6_PRESENT_OVER`]: presents that landed on the live sprite box
/// while the pass DID hold a plan. A plan in hand means `composite_inner`'s masked undraw ran over
/// every window that meets the sprite (the paint set is built independently of the per-window
/// `may_overlay` exclusion), so those pixels were handed back before the blit and the tail owes them
/// a repaint. Healthy, and the number that makes the defect count legible: `over=0 masked=0` proves
/// nothing, and the verdict says so rather than leaving a reader to notice.
#[cfg(feature = "witness")]
static C6_PRESENT_MASKED: AtomicU64 = AtomicU64::new(0);

/// CURSOR-6 — desktop presents (`Screen::present_background`) whose surviving damage spans met the
/// live sprite box. `Screen::flush` brackets that call with [`undraw`]/[`repaint`], so this must
/// be 0; a non-zero count means the desktop reached the panel through a path that skipped the
/// bracket, and the desktop is erasing the arrow ~20 times a second.
///
/// **FLICKER-3 gave this counter a second job and did not change its expected reading.** The
/// bracket is now taken only when the present's damage meets the sprite's box, so this is also the
/// designated detector for a WRONG SKIP: `Screen::damage_meets` tests a superset of the spans this
/// function copies, so a skip can never let one of them reach the arrow, and the first thing that
/// would refute that argument is this counter going non-zero.
#[cfg(feature = "witness")]
static C6_DESKTOP_OVER: AtomicU64 = AtomicU64::new(0);

/// FLICKER-3 — desktop presents that reached the flush gate at all, and the subset that SKIPPED the
/// bracket because this present's damage provably could not touch the live sprite.
///
/// ### Why there are two counters and not one
/// A bare `flush_skip=` cannot falsify anything, and this counter pair exists because `[cursor12]`
/// already taught the project that lesson at the cost of three sittings. `flush_skip=0` is what a
/// HEALTHY gate reads when it has simply not run yet (no flush since boot), and it is also what a
/// gate reads that is wired up but structurally never able to skip — the two are indistinguishable
/// from the skip count alone. `flush_gate=` is the denominator that separates them:
///
/// * `flush_gate=0` — no desktop present has happened in this window. **No verdict**; the
///   instrument is unarmed, exactly as `[cursor12] passes=0` is unarmed.
/// * `flush_gate>0, flush_skip=0` — the gate ran and never once found a present whose damage
///   missed the arrow. That is a real reading, and on x86 it is the expected one under the
///   full-screen `vug` presenter, which erases the pointer's previous footprint into the BACK
///   buffer every frame (`vug.rs`, the VUG-FPS dirty-rect phase) and so damages the sprite's own
///   box at frame rate by construction.
/// * `flush_skip` climbing with `flush_gate` — the bracket is being paid only when it buys
///   something, which is the whole of this fix.
///
/// The falsifier is already on the same line and needs no new term: a skip that should NOT have been
/// taken means the desktop blitted over a live arrow, and that is precisely what
/// [`note_desktop_over_sprite`] counts. `flush_skip>0` with `desktop_over=0` is the gate working;
/// `desktop_over>0` is the gate wrong, and the `UNBRACKETED` verdict already says so.
#[cfg(feature = "witness")]
static C6_FLUSH_GATE: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "witness")]
static C6_FLUSH_SKIP: AtomicU64 = AtomicU64::new(0);

/// CURSOR-6 — a [`compose_into`] that declined because the session no longer described THIS plan.
/// CURSOR-4 left this exit silent, so it was the one decline class absent from `offers - taken`.
#[cfg(feature = "witness")]
static C6_MISMATCH: AtomicU64 = AtomicU64::new(0);

/// CURSOR-6 — an [`overlay_uncover`] that could not apply its clear. See [`note_uncover_lost`].
#[cfg(feature = "witness")]
static C6_UNCOVER_LOST: AtomicU64 = AtomicU64::new(0);

/// CURSOR-6 — a coverage clear that was DROPPED, so the open session must not be trusted.
///
/// ### The hole, and why CURSOR-4's justification for dropping it does not hold
/// [`overlay_uncover`] `try_lock`s and returns silently on contention, on the documented argument
/// that "the only writer that could be holding it is another pass, which has already declined to
/// share the session — its coverage is not ours to correct". That is not the only writer.
/// [`overlay_open`] and [`adopt_overlay`] both take `OVERLAY` with a blocking `lock()` from outside
/// the guard, and another core runs one of those on every composite; a pass PROBING for the session
/// it is about to be refused holds the lock for exactly as long as it takes to observe `ov.session`.
/// So a clear can be lost while OUR session is the live one.
///
/// What that costs is not a missed optimisation. The window has just painted its own content over
/// pixels a LOWER window's `compose_into` claimed, and the coverage bit for those pixels is still
/// set. [`adopt_overlay`] then installs the lower layer's save-under for them and clears their `off`
/// bit — the module now believes the arrow is on the panel where this window's content is (**the
/// arrow is missing**), and the next undraw's colour guard sees the lower layer's saved value and
/// writes it back into the upper window's rect (**a stale patch inside a live window**). Both P65
/// symptoms, from one dropped `try_lock`.
///
/// The fix is the fallback the arc already trusts everywhere else: mark the session untrustworthy and
/// let the tail take the whole-sprite refresh. One relaxed store inside the guard, consumed and
/// cleared by `adopt_overlay` under the sprite lock. It cannot deadlock (no lock is taken).
///
/// **The residual, stated rather than implied: the leak is bounded at one pass, not zero.** Only the
/// session owner can set this (the flag is written from `draw_window`, which sees a plan only on a
/// pass that opened a session) and only `adopt_overlay` clears it, so the ordinary path sets and
/// retires it within the same pass. A pass that sets the flag and then settles on a `Repaint` tail
/// instead of `Adopt` — the tail is chosen from `session`, so this needs the session to have been
/// lost between the set and the tail — leaves the flag standing for the NEXT session to consume.
/// That next pass takes a `refresh_locked` it did not itself earn: one whole-sprite repaint, one
/// pass late, and then the flag is clear again. It cannot accumulate (an `AtomicBool`, not a count)
/// and it cannot persist (the next adopt always swaps it down). A spurious repaint is the same cost
/// this fix pays deliberately everywhere else, so the bound is the honest ceiling and not a hole.
static UNCOVER_LOST: AtomicBool = AtomicBool::new(false);

/// CURSOR-6 — record a dropped coverage clear. Called from `wm::draw_window`, inside the blit guard:
/// one relaxed store, no lock, no allocation, no serial.
pub fn note_uncover_lost() {
    UNCOVER_LOST.store(true, Ordering::Relaxed);
    #[cfg(feature = "witness")]
    C6_UNCOVER_LOST.fetch_add(1, Ordering::Relaxed);
}

/// CURSOR-7 — the arc's mechanism, in one bit: a window present has just written front-buffer pixels
/// that the live sprite was occupying, and NOTHING in that pass owes them a repaint.
///
/// ### Why the repair is at the pass TAIL and not before the blit
/// The obvious shape — hide the sprite before the blit, put it back after — is not available where the
/// blit happens. `draw_window`/`stage_window` run inside `wm`'s `BlitGuard` window, and F4's drain
/// barrier spins IRQ-masked and unpreemptible until every registered blit retires; a blocking `SPRITE`
/// acquisition there is exactly the wait its termination argument excludes (docs/dev/OS/08_VIDEO
/// §WEDGE-1's audited-exception list). CURSOR-3/4/5 already spend their whole design on that
/// constraint: the only sprite work admissible inside the guard is `OVERLAY.try_lock` and relaxed
/// atomics. So this flag is a relaxed store inside the guard, and the *repair* is `cursor::repaint()`
/// run from `wm::composite`, outside the guard, on the same footing as the `Repaint` tail that has
/// existed since WC-I. No new lock, no new lock ORDER, and nothing added to the drain's wait set.
///
/// ### What that leaves, stated rather than implied
/// The sprite is absent from the panel for the interval between the offending blit and the tail — the
/// length of the rest of the window loop, bounded by one composite pass. That is a strictly shorter
/// absence than the defect it replaces, which was UNBOUNDED: before CURSOR-7 the pass's tail was
/// `Untouched` (`ensure_drawn`, a no-op while `sp.drawn`), so the module believed the arrow was on a
/// panel that no longer held it and nothing repainted it until the pointer moved again. "Composite the
/// sprite on top after the blit" is what this is; it is not a claim to have made the present atomic
/// with the sprite. The path that IS atomic — `compose_into`, the sprite riding the present inside the
/// layer — is unchanged and still carries the bracketed case.
///
/// ### Global, coalescing, and consumed by whichever tail gets there first
/// One `AtomicBool` for the whole system, not one per pass: the sprite is global, the repair is
/// whole-sprite, and a pass on core B repairing an arrow core A trampled is the correct outcome, not a
/// crossed wire. Coalescing means N unbracketed presents cost at most one repaint per tail, which is
/// the point — the fix must not reinstate the per-present duty cycle WC-I and CURSOR-4 removed.
///
/// `Release`/`Acquire`: the store publishes "the panel no longer holds the arrow where the module
/// thinks it does", and what must not be reordered against it is the present's own pixel writes.
///
/// ### CURSOR-8 — the flag is a REQUEST, and [`take_present_dirty`] is now the thing that judges it
/// CURSOR-7 shipped with the arming and the granting fused: every consumer that saw the flag up ran a
/// whole-sprite [`repaint`]. Its own author wrote the failure condition down (engine.md §CURSOR-7):
/// *if `repaint=` starts tracking the PRESENT rate rather than the MOTION rate, the fix has become the
/// churn WC-I removed and belongs behind a rate limit.* P69 is that condition, met — see
/// [`REPAIR_MIN_MS`] for the loop it closes. The flag still means exactly what it meant; the decision
/// to spend a repaint on it moved into the consumer, where a clock and the sprite's own generation are
/// readable and the blit guard is gone.
static PRESENT_DIRTY: AtomicBool = AtomicBool::new(false);

/// CURSOR-9 — has any painter written front-buffer pixels inside the live sprite box since the sprite
/// was last drawn?
///
/// ### The leg of the P69 loop CURSOR-8 did not reach
/// CURSOR-8 rate-limited the loop's REPAIR leg (`PRESENT_DIRTY` → [`take_present_dirty`] → a tail
/// [`repaint`]). It left the loop's other leg untouched, and that leg is not driven by presents at all:
///
/// 1. a pointer report calls [`repaint`], which ends in [`repair`];
/// 2. [`repair`] calls `wm::damage_intersecting` over the whole sprite box, UNCONDITIONALLY —
///    so every window under the pointer is marked damaged 125 times a second;
/// 3. the next composite (any vug's present, or `Screen::flush` → `wm::service_damage`, whose own
///    contract is "service the damage `cursor::repair` left") therefore re-blits that window;
/// 4. that pass takes the cursor bracket (`composite_inner`'s `undraw_within*`), so the arrow comes
///    OFF the panel before `paint_window` composes the window off-screen and stays off until the row
///    blit or the pass tail puts it back — milliseconds, at composite cadence.
///
/// Nothing in that chain needs the window to be presenting anything of its own: the damage is
/// manufactured by the cursor. **That is why a PARKED vug flickers exactly like a running one, and why
/// the bare desktop is clean** — over the desktop step 2 marks no window, so step 3 never happens and
/// `composite_inner` never brackets the sprite at all (WC-I).
///
/// ### Why the flag is the exact predicate rather than a rate limit
/// [`repair`] exists for ONE residual, named at [`undraw_locked`]: the colour guard cannot tell our own
/// `FILL`/`SHADOW` from a painter's content that happens to equal it, so a restore can put a stale
/// pixel inside a window's rect. That residual requires **some painter to have written into the sprite
/// box between our draw and this undraw**. `wm::draw_window` already computes that predicate for every
/// window present it makes (`live_box_relaxed()` + `boxes_overlap`, already un-gated mechanism), so the
/// answer is available for one relaxed store on a path that has just blitted a window's worth of rows.
/// A pointer report over a window nobody has painted since the arrow went down provably cannot have
/// produced a stale restore, and damaging that window buys nothing but a turn of the loop above.
///
/// This is a narrowing of WHEN the repair is requested, not a weakening of what it does: every path
/// that can trample the sprite still arms it, and an armed request is serviced exactly as before.
///
/// ### The painters this does NOT hear from, stated rather than implied
/// * `Screen::present_background` — the desktop's flush is bracketed by the render task
///   (`cursor::undraw` → `pal.render` → `cursor::repaint`), so the sprite is provably not on the panel
///   while it runs and there is nothing for a repair to mend. A broken bracket is what
///   `note_desktop_over_sprite` is the designated detector for, and it must be 0.
/// * `wm::erase` / `drain_deferred` — both undraw FIRST and then run their own `damage_intersecting`
///   over the boxes they painted, so the repair they owe does not come from here.
/// * WC-F's ground-truth probe — paints the front OUTSIDE any window box, after the pass. It arms this
///   flag explicitly from `composite_inner`'s `reserved_hit` branch, so that path is unchanged.
/// * `fbcon`'s serial mirror on a `UNAOS_BOOTLOG` build. It does not arm, so a console line painted
///   into the sprite box would no longer provoke the repair. Named as a residual: that build already
///   has a second unsynchronised front-buffer writer, and the sprite's colour guard is what stands
///   between it and a stale patch either way.
///
/// `Release`/`Acquire`, on the same argument as [`PRESENT_DIRTY`]: the store publishes "the panel under
/// the arrow is not what the module saved", and what must not be reordered against it is the painter's
/// own pixel writes.
static TOUCHED_SINCE_DRAW: AtomicBool = AtomicBool::new(false);

/// CURSOR-9 — arm the repair for a painter that reaches the front buffer outside `draw_window`'s
/// present path. Called from `composite_inner`'s WC-F `reserved_hit` branch, which paints the front at
/// the tail of the pass and outside every window box.
pub fn note_sprite_touched() {
    TOUCHED_SINCE_DRAW.store(true, Ordering::Release);
}

/// CURSOR-8 — [`live_epoch`] as it stood when [`PRESENT_DIRTY`] was last armed.
///
/// The cheap half of "were the sprite's pixels ACTUALLY disturbed since the last repair", and the
/// only half that is affordable here. The expensive half — compare the panel against `Sprite::saved`
/// across the whole box — costs a read-back of exactly the pixels the repair would rewrite, so it IS
/// the repair, and it would need `SPRITE` held while the front is read. What a generation comparison
/// buys instead, for one `Acquire` load and no lock: every full undraw and every draw bumps the epoch
/// ([`bump_epoch`]), so `live_epoch() != PRESENT_DIRTY_EPOCH` at consume time means a complete
/// restore → save → draw cycle has run since the offending present — from the HID router's
/// [`repaint`], from `wm::erase`, from the deferred-erase drain — and that cycle has already
/// re-established the arrow and its save-under from the finished front. The request is stale, and
/// granting it would buy nothing but another turn of the loop.
///
/// Written BEFORE the flag and read AFTER it, so a consumer that observes the flag up observes an
/// epoch no NEWER than the arming. An epoch one arming stale can only make this test conservative: it
/// declines to suppress and falls through to the rate floor, which is the safe direction.
static PRESENT_DIRTY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// CURSOR-8 — the monotonic counter reading at the last GRANTED repair, in [`mono_now_hz`]'s units.
/// `0` means "none yet this boot", which the elapsed test reads as overdue.
static LAST_REPAIR_TICKS: AtomicU64 = AtomicU64::new(0);

/// CURSOR-8 — the floor between two granted tail repairs, in milliseconds.
///
/// ### The loop this number breaks
/// P69: "mouse worse than ever over a vug — unusable", and "keystrokes typed into the background
/// console flash the vug". Both are one positive feedback loop, and CURSOR-7 closed it:
///
/// 1. a vug presents (~50 fps each, several of them, from several cores at once);
/// 2. the present meets the live sprite box while the presenting pass held no bracket of its own —
///    which in the VUGPAR steady state is the COMMON case, not a rare one: a masked undraw on core A
///    deliberately does not retract [`LIVE_ON`] (part of the sprite is still up, and a reader that
///    concluded "no sprite" would undercount the straddle case CURSOR-6 exists to see), while core B's
///    own `sprite_plan()` came back empty — so B is unbracketed over a box that is still advertised;
/// 3. so the present arms [`PRESENT_DIRTY`], and B's tail runs a whole-sprite [`repaint`];
/// 4. [`repaint`] ends in [`repair`], which calls `wm::damage_intersecting` over the restored rect —
///    marking every window UNDER the sprite damaged, by design (the colour guard can leave stale
///    pixels inside a window's rect and only a redraw from the app's surface mends them);
/// 5. the next composite therefore re-blits that whole window from its surface — and that present
///    meets the sprite again, at (2).
///
/// The cursor is unusable because it spends its life mid-restore, which is exactly the duty cycle WC-I
/// and CURSOR-3 were built to remove; the vug flashes because every turn of the loop re-blits it
/// whole. The keystroke report is the SECOND-ORDER view of the same loop: a keystroke echoes to the
/// console, the console is the DESKTOP surface, and `Screen::flush` ends in `wm::service_damage` —
/// which exists to service exactly the damage `cursor::repair` leaves behind, and so cashes the queued
/// sprite damage into a full re-blit of the focused vug, one flash per keypress. No console pixel ever
/// lands on the vug (WC-I's `occluders` subtraction sees to that, and `desktop_over` is the counter
/// that would say otherwise); the keystroke supplies only the CADENCE that pays out damage the repair
/// storm had already queued.
///
/// ### Why 8 ms and not a frame
/// The bound has to be the MOTION rate, because motion is the only thing a whole-sprite repaint can
/// legitimately track: a 125 Hz HID mouse reports every 8 ms, and the router's own repaint on that
/// path is a cost the design already accepts. So a parked sprite over a churning vug repairs at 125 Hz
/// worst case instead of at aggregate present rate, the loop's gain drops below one, and nothing a
/// pointer report would have repainted anyway is delayed by more than one report period. A frame-rate
/// bound (16 ms) would leave a visibly late arrow on a fast drag; a present-rate bound is the thing
/// being removed.
const REPAIR_MIN_MS: u64 = 8;

/// CURSOR-8 — the monotonic counter and its frequency, or `None` where this machine has no
/// trustworthy one.
///
/// A LANE-LOCAL mirror of the private `clock::monotonic()` seam, and named as one: `clock` exposes
/// whole `uptime_secs()` and raw `mono_ticks()`, but no MILLISECOND reading outside the `logts`
/// feature, and adding one is an edit to a shared kernel-core file this arc's brief does not cover.
/// Both arms call the same public arch accessors `clock::monotonic()` calls, so the two cannot
/// disagree about the timebase. **Fold this into `clock` as a `pub fn mono_ms()` when a session owns
/// that file** — it is duplication, it is flagged as duplication, and it is two arms to delete.
#[cfg(target_arch = "aarch64")]
fn mono_now_hz() -> Option<(u64, u64)> {
    let hz = crate::arch::timer::cntfrq();
    if hz == 0 {
        return None; // defensive: a zero CNTFRQ would make the division meaningless
    }
    Some((crate::arch::now_cycles(), hz))
}

#[cfg(target_arch = "x86_64")]
fn mono_now_hz() -> Option<(u64, u64)> {
    // The invariant-TSC bit is `clock`'s honesty contract for a WALL clock. Here the only question is
    // whether an 8 ms interval can be measured at all, and a calibrated frequency is the whole of it.
    let hz = crate::arch::apic::tsc_hz();
    if hz == 0 {
        return None; // calibration never ran or was rejected
    }
    Some((crate::arch::now_cycles(), hz))
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn mono_now_hz() -> Option<(u64, u64)> {
    None
}

/// CURSOR-7 — tail repairs actually taken. `repaired <= present_over` by construction (the flag
/// coalesces), so this counts REPAIR PASSES, never pixels or presents; see [`cursor6_rollup`].
///
/// CURSOR-8 — and it now counts GRANTED repairs only: a pass that found the flag up and declined it
/// (stale generation, or inside the rate floor) does NOT increment this. That is what keeps
/// `[cursor6] repaired=` the answer to "how often did the panel actually get a fresh arrow" rather
/// than "how often was one asked for" — the ask is `present_over`, and the gap between the two is the
/// whole of `[cursor8]`.
#[cfg(feature = "witness")]
static C7_REPAIRED: AtomicU64 = AtomicU64::new(0);

/// CURSOR-8 — repair requests observed by a consumer: the denominator of the `[cursor8]` rollup.
/// Distinct from `present_over`, which counts ARMINGS: the flag coalesces, so several armings between
/// two tails are one request here, and `requests <= present_over` always.
#[cfg(feature = "witness")]
static C8_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// CURSOR-8 — requests declined because [`live_epoch`] had moved since the arming: a full sprite cycle
/// already re-established the arrow, so the repair had nothing left to do.
#[cfg(feature = "witness")]
static C8_SUPPRESSED_STALE: AtomicU64 = AtomicU64::new(0);

/// CURSOR-8 — requests declined because the last granted repair was less than [`REPAIR_MIN_MS`] ago.
/// The request is NOT dropped: the flag is re-armed, and the first pass past the floor takes it. So
/// this counts DEFERRALS, never losses, and it is the number that says the storm was actually caught.
#[cfg(feature = "witness")]
static C8_SUPPRESSED_RATE: AtomicU64 = AtomicU64::new(0);

/// CURSOR-8 — repairs granted on a machine with no readable monotonic counter, where the floor cannot
/// be applied at all and CURSOR-7's unbounded behaviour stands. Must be 0 on the Pi (CNTFRQ is
/// architectural) and on any x86 past APIC calibration; non-zero says the rate limit is not RUNNING,
/// which is a different reading from "not needed".
#[cfg(feature = "witness")]
static C8_UNCLOCKED: AtomicU64 = AtomicU64::new(0);

/// CURSOR-10 — panel bytes this module has handed to `DC CVAC`, across every flush path it owns.
/// Reported as KiB in `[cursor8]`'s line because that is the rollup the pointer-report cost already
/// lives in. On the bench the number to read is its RATE against `[cursor6] present_over`: before
/// CURSOR-10 a pointer report cost two full-scanline sweeps of the sprite's height (~553 KB at
/// 1920x1200), after it one column-bounded sweep of the union (~9 KB), so a run whose motion profile
/// is unchanged should show this fall by roughly the panel-width-to-sprite-width ratio. QEMU raspi4b
/// has no HID pointer, so on the gate it counts only the compositor-driven paths and cannot witness
/// the win.
#[cfg(feature = "witness")]
static C10_FLUSH_BYTES: AtomicU64 = AtomicU64::new(0);

// ---- CURSOR-11 witnesses ------------------------------------------------------------------------

/// CURSOR-11 — composite passes that took the COMPOSE-THROUGH shape: the session was opened and the
/// arrow was left on glass for the whole pass. One per [`defer_within`] call.
#[cfg(feature = "witness")]
static C11_PASSES: AtomicU64 = AtomicU64::new(0);

/// CURSOR-11 — composite passes that still took a real BRACKET, i.e. the arrow left the glass. Three
/// sources, all pre-existing and all deliberately kept: the WC-F reserved-box full undraw, the
/// sessionless masked undraw ([`undraw_within_nosession`]), and an [`adopt_overlay`] whose session
/// came back incoherent and fell to `refresh_locked`. The ratio against `passes` is the arc's verdict
/// on the bench: the P73 blink is `bracketed` dominating a pointer that sits over a presenting vug.
#[cfg(feature = "witness")]
static C11_BRACKETED: AtomicU64 = AtomicU64::new(0);

/// CURSOR-11 — sprite pixels left on glass across a present that could have overwritten them. The
/// denominator for the two settlements below; `installed + redrawn <= deferred`, and the difference is
/// the pixels no painter in the pass actually touched (the common case, settled by reading the front
/// and writing nothing).
#[cfg(feature = "witness")]
static C11_PIX_DEFERRED: AtomicU64 = AtomicU64::new(0);

/// CURSOR-11 — deferred pixels a staged present carried through, whose save-under the tail installed
/// from the BACK LAYER. **The arrow never left the panel at these pixels**, which is the whole of the
/// fix; they are the number to read on the bench.
#[cfg(feature = "witness")]
static C11_PIX_INSTALLED: AtomicU64 = AtomicU64::new(0);

/// CURSOR-11 — deferred pixels the tail found holding someone else's colour: a painter in the pass
/// took them without composing the sprite in, so the tail re-saved from the finished front and put the
/// arrow back. These pixels DID blink, for the length of the pass, exactly as they did before this
/// arc. Not a fault — it is what the direct path, the instrument exclusions and the straddle cost —
/// but a large ratio against `installed` means the compose-through is not reaching the pixels the
/// pointer is actually over, and the reading to take to `[cursor3] rollup`'s decline breakdown.
#[cfg(feature = "witness")]
static C11_PIX_REDRAWN: AtomicU64 = AtomicU64::new(0);

/// CURSOR-11 — a pass that took a real bracket rather than the deferral, counted from `wm`'s two
/// decline arms. The third source (an incoherent tail) is counted inside [`adopt_overlay`].
#[cfg(feature = "witness")]
pub fn note_bracketed_pass() {
    C11_BRACKETED.fetch_add(1, Ordering::Relaxed);
}

/// CURSOR-11 — the compose-through rollup, printed immediately after `[cursor8]`'s because it is the
/// same question one level up: `[cursor8]` says what it cost to REBUILD the arrow, this says how often
/// the arrow had to come down at all.
///
/// * **`passes` / `bracketed`** — composite passes that left the arrow on glass, against those that
///   took it off. Before this arc every pass over a window was `bracketed` by construction.
/// * **`px_deferred`** — sprite pixels carried through a present without a handback.
/// * **`px_installed`** — of those, the ones a staged present delivered with the arrow already in the
///   rows, whose save-under came from the layer. **The arrow never left the panel here.**
/// * **`px_redrawn`** — of those, the ones a painter took anyway (direct path, instrument exclusion,
///   straddle): re-saved from the finished front and redrawn by the tail. These blinked.
/// * The remainder, `px_deferred - px_installed - px_redrawn`, is pixels nothing in the pass touched:
///   settled by one front read and no write at all.
///
/// `UNWITNESSED` on QEMU raspi4b by construction — no HID pointer report means the sprite is never
/// drawn, `sprite_plan()` is always `None`, no session is ever opened and every counter here is 0. The
/// gate proves NO-REGRESSION only; the verdict that carries the fix is `px_installed` dominating
/// `px_redrawn` on an attended bench boot with the pointer parked over a presenting vug.
#[cfg(feature = "witness")]
pub fn cursor11_rollup(scope: &str) {
    let passes = C11_PASSES.load(Ordering::Relaxed);
    let bracketed = C11_BRACKETED.load(Ordering::Relaxed);
    let deferred = C11_PIX_DEFERRED.load(Ordering::Relaxed);
    let installed = C11_PIX_INSTALLED.load(Ordering::Relaxed);
    let redrawn = C11_PIX_REDRAWN.load(Ordering::Relaxed);
    let verdict = if passes == 0 && bracketed == 0 {
        "UNWITNESSED"
    } else if deferred == 0 {
        "NO-DEFERRAL"
    } else if installed >= redrawn {
        "THROUGH"
    } else {
        "BRACKETED"
    };
    serial_println!(
        "[cursor11] compose-through scope={} passes={} bracketed={} px_deferred={} px_installed={} px_redrawn={} -> {}",
        scope, passes, bracketed, deferred, installed, redrawn, verdict
    );
}

/// CURSOR-6/7 — count a window present that landed on the live sprite box, and, when nothing in that
/// pass owes the sprite a repaint, ARM the tail repair.
///
/// `bracketed` is the pass's `disturbed`: this pass took the sprite down (fully or masked) and its
/// tail is therefore `Repaint` or `Adopt`, both of which put the arrow back. CURSOR-6 asked the
/// narrower question `cur.is_some()` — whether an overlay PLAN was in hand — which counted the
/// sessionless masked-undraw path (`overlay_open` refused by a concurrent pass, the VUGPAR steady
/// state) as unbracketed although its `undraw_within_nosession` had handed the pixels back and its
/// `Repaint` tail owed them a repaint. That misclassification is a large part of P67v2's
/// `present_over=9/s`, and correcting it is not a softening: the class that remains is exactly the one
/// with no repair behind it, which is the one this arc closes.
///
/// No longer witness-gated — the flag is now MECHANISM. The counters stay gated; the store and the
/// `live_box_relaxed` test that precedes it are two relaxed atomics on a path that already blits a
/// window's worth of rows.
pub fn note_present_over_sprite(bracketed: bool) {
    // CURSOR-9 — BOTH arms. This is not the repair REQUEST (that is `PRESENT_DIRTY`, below, and it is
    // deliberately the unbracketed case only); it is the answer to "could a painter have taken one of
    // our pixels since the arrow went down", which a bracketed present answers just as affirmatively
    // as an unbracketed one. See `TOUCHED_SINCE_DRAW`.
    TOUCHED_SINCE_DRAW.store(true, Ordering::Release);
    if !bracketed {
        // CURSOR-8 — the generation FIRST, the flag second, and both plain stores. The consumer reads
        // them the other way round (flag, then epoch), so an observer that sees the flag up cannot see
        // an epoch newer than this arming; it can see an OLDER one if a second core armed in between,
        // and that direction is harmless (it declines to suppress, and the rate floor still applies).
        // One extra relaxed store on a path that has just blitted a window's worth of rows, and still
        // nothing here that the drain barrier could ever wait on.
        PRESENT_DIRTY_EPOCH.store(live_epoch(), Ordering::Relaxed);
        PRESENT_DIRTY.store(true, Ordering::Release);
    }
    #[cfg(feature = "witness")]
    if bracketed {
        C6_PRESENT_MASKED.fetch_add(1, Ordering::Relaxed);
    } else {
        C6_PRESENT_OVER.fetch_add(1, Ordering::Relaxed);
    }
}

/// CURSOR-7 — consume the tail-repair flag. `true` obliges the caller to run [`repaint`] once the
/// pass's own tail has been settled.
///
/// A swap, not a load: the flag is a request for ONE whole-sprite repaint, and leaving it standing
/// would make every later pass pay for a present that has already been repaired. Called from
/// `wm::composite` only, outside the `BlitGuard` window.
///
/// ### CURSOR-8 — and it is a request to be JUDGED, not a command
/// CURSOR-7 granted every request, so `repaint=` tracked the aggregate PRESENT rate; P69 is what that
/// costs on a panel with several presenting vugs (see [`REPAIR_MIN_MS`] for the loop, both of P69's
/// symptoms, and why the bound is the HID report period). Two tests stand between the request and the
/// repaint, cheapest first, and both are lock-free — this runs outside the `BlitGuard` window, but it
/// also runs on the tail of every composite on the machine, so it stays off `SPRITE` on cost grounds
/// alone:
///
/// * **stale** — the generation moved since the arming, so a full restore → save → draw cycle has
///   already put the arrow back from the finished front. Nothing to repair. This is the "was the
///   sprite actually disturbed" test in the only form that is affordable here; see
///   [`PRESENT_DIRTY_EPOCH`] for why the pixel-exact form is not.
/// * **rate** — the last granted repair was less than [`REPAIR_MIN_MS`] ago. **The request is re-armed
///   rather than dropped**: deferring a repair by up to 8 ms is a bounded latency, dropping one is the
///   unbounded absence CURSOR-7 exists to close, and the two must not be confused. The first pass past
///   the floor takes it, and a pass will come — every present runs a tail, and `service_damage` runs on
///   the desktop's cadence even when no window presents at all.
///
/// The re-arm is a plain `store(true)`, not a compare-exchange: a concurrent arming that raced it can
/// only be asking for the same thing, and the epoch it stored is at worst one arming stale — which
/// [`PRESENT_DIRTY_EPOCH`] documents as the conservative direction. Nothing here spins, and nothing
/// here can fail to make progress: every path is a bounded number of atomic operations.
pub fn take_present_dirty() -> bool {
    if !PRESENT_DIRTY.swap(false, Ordering::AcqRel) {
        return false;
    }
    #[cfg(feature = "witness")]
    C8_REQUESTS.fetch_add(1, Ordering::Relaxed);

    // Read AFTER the flag — see `note_present_over_sprite` for the pairing.
    if live_epoch() != PRESENT_DIRTY_EPOCH.load(Ordering::Relaxed) {
        #[cfg(feature = "witness")]
        C8_SUPPRESSED_STALE.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    let (now, hz) = match mono_now_hz() {
        Some(t) => t,
        None => {
            // No measurable interval on this machine: grant, and SAY SO. Declining instead would
            // reinstate CURSOR-6's unbounded absence on a machine whose only fault is an uncalibrated
            // counter, and granting silently would let `[cursor8]` read as paced when it is not.
            #[cfg(feature = "witness")]
            {
                C8_UNCLOCKED.fetch_add(1, Ordering::Relaxed);
                C7_REPAIRED.fetch_add(1, Ordering::Relaxed);
            }
            return true;
        }
    };
    let floor = hz.saturating_mul(REPAIR_MIN_MS) / 1000;
    let last = LAST_REPAIR_TICKS.load(Ordering::Relaxed);
    // `wrapping_sub` on a free-running counter, then an unsigned compare: a counter that wrapped (or a
    // `last` written by a core whose reading ran marginally ahead) yields a huge elapsed and grants,
    // which is the safe direction — the failure mode of this limiter must be an extra repaint, never a
    // withheld one.
    if last != 0 && now.wrapping_sub(last) < floor {
        #[cfg(feature = "witness")]
        C8_SUPPRESSED_RATE.fetch_add(1, Ordering::Relaxed);
        PRESENT_DIRTY.store(true, Ordering::Release);
        return false;
    }
    LAST_REPAIR_TICKS.store(now, Ordering::Relaxed);
    #[cfg(feature = "witness")]
    C7_REPAIRED.fetch_add(1, Ordering::Relaxed);
    true
}

/// CURSOR-8 — the arc's one-line rollup: how many tail-repair requests were made, how many were
/// GRANTED, and which test declined the rest.
///
/// Printed after `[cursor6]`'s, because it is that line's footnote: `[cursor6] present_over=` is the
/// ask and `repaired=` is the grant, and this says what happened in between. The reading that matters
/// on the bench is `suppressed_rate` climbing while `repairs` stays near the motion rate — that is the
/// present-rate storm being caught. `requests > 0` with `suppressed_rate == 0` and a large `repairs`
/// is CURSOR-7's behaviour, i.e. the limiter is not biting and the floor may be too low.
///
/// * **`requests`** — flag consumptions, not armings. The flag coalesces, so this is `<= present_over`
///   by construction and the ratio between them is how much the coalescing is already absorbing.
/// * **`repairs`** — granted, and identical to `[cursor6] repaired=`. Both are the same counter, on
///   purpose: two lines that disagreed about how often the arrow was rebuilt would be worse than one.
/// * **`suppressed_stale`** — a full sprite cycle beat us to it. Load, not damage.
/// * **`suppressed_rate`** — inside the floor, DEFERRED (re-armed), not lost.
/// * **`unclocked`** — granted without a floor because no monotonic counter was readable. Must be 0
///   on both bench platforms; non-zero means the limiter is not running.
/// * **`flush_kb`** — CURSOR-10: panel KiB this module has cleaned to the PoC, over every flush path
///   it owns. A cost, not a fault, and the only number that reads the coalesce on the bench: it
///   should fall by roughly the panel-width-to-sprite-width ratio against an unchanged motion
///   profile. It does not enter the verdict.
///
/// `UNWITNESSED` where no request ever arrived, which on QEMU raspi4b is every boot: no HID pointer
/// report means the sprite is never drawn, `live_box_relaxed()` is always `None`, and nothing can arm.
/// The gate therefore proves no-regression only, and the verdict says so rather than reading `PACED`
/// vacuously.
#[cfg(feature = "witness")]
pub fn cursor8_rollup(scope: &str) {
    let requests = C8_REQUESTS.load(Ordering::Relaxed);
    let repairs = C7_REPAIRED.load(Ordering::Relaxed);
    let stale = C8_SUPPRESSED_STALE.load(Ordering::Relaxed);
    let rate = C8_SUPPRESSED_RATE.load(Ordering::Relaxed);
    let unclocked = C8_UNCLOCKED.load(Ordering::Relaxed);
    let flush_kb = C10_FLUSH_BYTES.load(Ordering::Relaxed) / 1024;
    let verdict = if unclocked > 0 {
        "UNCLOCKED"
    } else if requests == 0 {
        "UNWITNESSED"
    } else if rate > 0 {
        "LIMITED"
    } else {
        "PACED"
    };
    serial_println!(
        "[cursor8] repair rate scope={} requests={} repairs={} suppressed_stale={} suppressed_rate={} unclocked={} floor_ms={} flush_kb={} -> {}",
        scope, requests, repairs, stale, rate, unclocked, REPAIR_MIN_MS, flush_kb, verdict
    );
    // CURSOR-11 — and how often the arrow had to come down at all, which is the question the three
    // lines above all presuppose an answer to. Same scope, same block, one call site.
    cursor11_rollup(scope);
}

/// CURSOR-6 — count a desktop present that landed on the live sprite (from `Screen::present_background`).
#[cfg(feature = "witness")]
pub(super) fn note_desktop_over_sprite() {
    C6_DESKTOP_OVER.fetch_add(1, Ordering::Relaxed);
}

/// FLICKER-3 — record one pass through `Screen::flush`'s bracket gate, and whether it skipped.
///
/// Called once per flush, AFTER the present, from `Screen::flush` and nowhere else — outside every
/// lock this module owns, so it is a pair of relaxed adds on a path that has just done a blit.
#[cfg(feature = "witness")]
pub(super) fn note_flush_gate(skipped: bool) {
    C6_FLUSH_GATE.fetch_add(1, Ordering::Relaxed);
    if skipped {
        C6_FLUSH_SKIP.fetch_add(1, Ordering::Relaxed);
    }
}

/// CURSOR-6 — the arc's rollup, printed by `wm` immediately after `[cursor5]`'s.
///
/// Each line item answers a question `[cursor5]`'s `COHERENT` could not, which is the whole point of
/// the arc:
///  * `present_over` / `masked` — a window present took the arrow's pixels. `masked` is the healthy
///    case (the pass bracketed the sprite first and its tail owes them a repaint) and is the
///    denominator; `present_over` is the same event with no bracket of the pass's own.
///    **CURSOR-7 changed what both mean, in two ways, and the change is deliberate.** First, the
///    split is now taken on the pass's `disturbed` rather than on `cur.is_some()`, so the sessionless
///    masked undraw — which DOES hand the pixels back and DOES take a `Repaint` tail — counts as
///    `masked` instead of as a defect. Second, `present_over` is no longer an unrepaired overwrite:
///    each one arms `PRESENT_DIRTY` and is settled by a tail repaint. It is now a COST counter (how
///    often the sprite has to be re-established from the finished front) rather than a damage counter,
///    and `repaired` is what says the mechanism ran.
///  * `repaired` — CURSOR-7 tail repairs taken. The flag COALESCES, so this counts repair PASSES:
///    `repaired <= present_over` always, and a ratio well under 1 means several presents per pass met
///    the arrow and were settled together, which is the fix working as intended rather than a gap.
///    `present_over > 0` with `repaired == 0` is the one reading that means the mechanism is NOT
///    running — that is a regression in `wm::composite`'s tail, not load.
///    **CURSOR-8 — `repaired` is now GRANTED repairs.** A request the rate floor deferred or the
///    generation test found stale does not count here, so the ratio `repaired / present_over` is no
///    longer only the coalescing: it is the coalescing AND the limiter, and `[cursor8]` immediately
///    below splits the two. `present_over > 0 && repaired == 0` therefore has a second, benign
///    reading it did not have before — every request so far fell inside the floor — and `[cursor8]`
///    is what tells the two apart. `OVERWRITTEN` is kept as the verdict because the panel outcome is
///    the same either way for the length of that window, and a reader who sees it should look at the
///    next line rather than at `wm::composite`.
///  * `desktop_over` — the same question for the desktop layer. Must be 0 (the render task brackets
///    its flush); non-zero is a broken bracket, not load.
///  * `mismatch` — [`compose_into`] declines where the open session did not describe the plan; the
///    last decline class that `offers - taken` could not account for.
///  * `uncover_lost` — dropped coverage clears, each absorbed by a whole-sprite refresh rather than
///    left to corrupt a session. **Printed against `planned`, because the fix has a PRICE and the
///    bench must be able to read it.** Each one costs the pass a `refresh_locked` — the whole-sprite
///    duty cycle CURSOR-4 and CURSOR-5 spent two arcs removing — so `uncover_lost/planned` is the
///    fraction of sessions paying it. A few per thousand is the absorbed race the fix is for; a
///    large fraction under VUGPAR would mean the `try_lock` is contended often enough that the
///    correct answer is to make `overlay_uncover` not need the lock, not to keep paying refreshes.
///
/// QEMU raspi4b delivers no HID pointer report, so the sprite is never drawn, `live_box_relaxed()` is
/// always `None`, and every counter here is 0 — reported as `UNWITNESSED`, never as `CLEAN`.
#[cfg(feature = "witness")]
pub fn cursor6_rollup(scope: &str, planned: u64) {
    let over = C6_PRESENT_OVER.load(Ordering::Relaxed);
    let masked = C6_PRESENT_MASKED.load(Ordering::Relaxed);
    let desktop = C6_DESKTOP_OVER.load(Ordering::Relaxed);
    let mismatch = C6_MISMATCH.load(Ordering::Relaxed);
    let lost = C6_UNCOVER_LOST.load(Ordering::Relaxed);
    let repaired = C7_REPAIRED.load(Ordering::Relaxed);
    // FLICKER-3 — the gate's own reading, appended to this line rather than given one of its own:
    // `desktop_over` is its falsifier and the two have to be read together or not at all.
    let fskip = C6_FLUSH_SKIP.load(Ordering::Relaxed);
    let fgate = C6_FLUSH_GATE.load(Ordering::Relaxed);
    // `desktop_over` first: of the three it is the one that would mean a BROKEN bracket rather than
    // a missing one, so a reader who sees it should look there before anything else. It is a
    // VERDICT term and no longer a gate FORBID — see the spec comment: this counter is
    // over-count-biased by construction (the publish is deliberately early), and a desktop flush
    // that races an arriving arrow is a real, healthy, transient overlap. Failing a metal boot on it
    // would be a false red, and a false red costs Peter a bench sitting chasing nothing.
    //
    // `UNWITNESSED` covers both "no pointer ever existed" (QEMU) and "no present ever met the
    // sprite": `over == masked == 0` means the mechanism was never exercised, and `INTACT` there
    // would be a vacuous pass.
    //
    // CURSOR-7 — `OVERWRITTEN` no longer means "presents met the arrow"; it means "presents met the
    // arrow AND NOTHING REPAIRED IT", which is now a regression in `wm::composite`'s tail rather than
    // the steady state P67v2 measured. The repaired case gets its own word instead of being folded
    // into `INTACT`: an arrow that is re-established once per pass is genuinely better than one that
    // was never disturbed, and saying so is what keeps the next bench reading honest.
    let verdict = if desktop > 0 {
        "UNBRACKETED"
    } else if !armed() || (over == 0 && masked == 0) {
        "UNWITNESSED"
    } else if over > 0 && repaired == 0 {
        "OVERWRITTEN"
    } else if over > 0 {
        "REPAIRED"
    } else {
        "INTACT"
    };
    serial_println!(
        "[cursor6] rollup scope={} present_over={} masked={} repaired={} desktop_over={} mismatch={} uncover_lost={}/{} flush_skip={}/{} -> {}",
        scope, over, masked, repaired, desktop, mismatch, lost, planned, fskip, fgate, verdict
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

/// CURSOR-WCR — one panel pixel read back as ONE bus transaction, where the layout allows it.
///
/// ### Why the read side needed its own primitive
/// Every pixel walk in this module that touches the FRONT buffer is a read-back: the colour guard in
/// [`undraw_locked`] asks "is our own colour still there", and the save-under in [`draw_locked`] asks
/// "what is underneath". [`FrameBuffer::read_pixel`] answers both by decoding the layout per call and
/// issuing THREE separate `read_volatile`s, one per colour byte. On a cached surface that costs
/// nothing measurable. On the rMBP it does: the panel is WC-mapped (`x86 fb-wc: retyped 15 leaf(s) WC
/// (PAT PA4)`), WC memory is UNCACHEABLE for reads, and an uncached byte load is a full round trip to
/// the aperture that no cache line amortises. So a `refresh_locked` pair — undraw then draw, one per
/// pointer report — pays six aperture round trips per painted pixel, at the trackpad's ~125 Hz.
///
/// Where the layout is a full 4-byte pixel (every x86 panel this kernel has met: `bytes_per_pixel`
/// 4, `Rgb` or `Bgr`), the three colour bytes are three quarters of ONE aligned 32-bit word, and one
/// aligned load returns all of them in one transaction. That is a 3:1 cut in aperture traffic on
/// exactly the two walks the pointer path runs per report, and it is the read-side twin of the 3:1
/// [`FrameBuffer::put_raw4`] already takes on the write side.
///
/// ### What is NOT changed
/// The values. This is `read_pixel`'s contract, byte for byte: the same bounds tests (off-panel and
/// past the mapped length both answer `None`), the same `0x00RRGGBB` inverse of `put_pixel`'s
/// encoding, and `None` on any layout without one — `U8` averages and is not invertible, so it takes
/// the slow path and refuses there exactly as before. The load stays `volatile`, so it is still not
/// hoisted across or folded with the stores the same walk performs.
///
/// The decision is made ONCE per walk ([`PanelRead::of`]), not once per pixel: the per-pixel cost is
/// a bounds test, a load and a shift. The fast path additionally requires a 4-byte-aligned base, so
/// the load can never be an unaligned access even in principle; every framebuffer this kernel maps is
/// page-aligned, and one that somehow is not simply falls back.
///
/// ### Arch scope
/// x86_64 only, and deliberately: WC read cost is what motivates it, `put_raw4`/`encode4` are already
/// x86-gated for the same reason, and the aarch64 panel is cached RAM where the three byte loads are
/// three L1 hits. On aarch64 `of` never reports a fast path and every call is `read_pixel` verbatim.
#[derive(Clone, Copy)]
struct PanelRead {
    fb: FrameBuffer,
    /// `Some((base, row_bytes, bgr))` when the aligned-word path applies; `None` = defer to
    /// [`FrameBuffer::read_pixel`].
    fast: Option<(usize, usize, bool)>,
}

impl PanelRead {
    /// Decide the read path for one pixel walk over `fb`.
    fn of(fb: &FrameBuffer) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let info = fb.info();
            let base = fb.base_addr();
            let bgr = match info.pixel_format {
                unaos_boot_info::PixelFormat::Bgr => true,
                unaos_boot_info::PixelFormat::Rgb => false,
                _ => return Self { fb: *fb, fast: None },
            };
            if base != 0 && base % 4 == 0 && info.bytes_per_pixel == 4 {
                return Self { fb: *fb, fast: Some((base, info.stride * 4, bgr)) };
            }
        }
        Self { fb: *fb, fast: None }
    }

    /// Read one pixel as `0x00RRGGBB`. Identical in value to `fb.read_pixel(x, y)`.
    #[inline]
    fn pixel(&self, x: usize, y: usize) -> Option<u32> {
        if let Some((base, row_bytes, bgr)) = self.fast {
            let info = self.fb.info();
            if x >= info.width || y >= info.height {
                return None;
            }
            let offset = y * row_bytes + x * 4;
            if offset + 4 > self.fb.len() {
                return None;
            }
            // SAFETY: bounds-checked against the mapped length above, and 4-byte aligned (the base
            // is, and the offset is a multiple of 4). Volatile for the same reason `read_pixel`'s
            // byte loads are: this walk's own stores must not be folded into it.
            let v = unsafe { core::ptr::read_volatile((base + offset) as *const u32) };
            // `put_pixel` lays the bytes down as [b,g,r] (Bgr) or [r,g,b] (Rgb); a little-endian word
            // load returns them in that order from the low byte up. Bgr is therefore the identity
            // under a 24-bit mask, and Rgb is that with the outer two bytes exchanged.
            return Some(if bgr {
                v & 0x00FF_FFFF
            } else {
                ((v & 0xFF) << 16) | (v & 0xFF00) | ((v >> 16) & 0xFF)
            });
        }
        self.fb.read_pixel(x, y)
    }
}

/// Clean the panel rows the box spans, so the non-coherent HVS sees the change. Whole scanlines at
/// the panel's stride — the same discipline `wm::draw_window` and `Screen::flush` use.
///
/// Kept as-is for the standalone entry points ([`undraw`], [`ensure_drawn`], the masked paths), whose
/// other callers are unchanged by CURSOR-10. The [`refresh_locked`] pair goes through
/// [`FlushUnion`]/[`flush_rect`] instead.
fn flush_box(fb: &FrameBuffer, y: usize, h: usize) {
    let info = fb.info();
    let row_bytes = info.stride * info.bytes_per_pixel;
    let y0 = y.min(info.height);
    let y1 = (y + h).min(info.height);
    if y1 > y0 {
        #[cfg(feature = "witness")]
        C10_FLUSH_BYTES.fetch_add(((y1 - y0) * row_bytes) as u64, Ordering::Relaxed);
        fb.flush_range(y0 * row_bytes, (y1 - y0) * row_bytes);
    }
}

/// CURSOR-10 — clean only the COLUMNS the box spans, row by row, instead of whole scanlines.
///
/// The sprite is ~36 px wide on a 1920 px panel, so a whole-scanline sweep cleans ~53x the bytes the
/// sprite could possibly have dirtied. `flush_range` takes a byte range, not a rect, so a
/// column-bounded clean is `h` calls rather than one — and `h` is the sprite's height (~36), not the
/// panel's.
///
/// **Cache-line alignment is not a correctness condition here, and that is worth saying explicitly.**
/// `cache::clean_range` rounds the start DOWN to the D-cache line (64 B on the A72) and iterates to
/// the end, so a rect whose columns do not sit on a line boundary simply cleans a few neighbouring
/// pixels as well. `DC CVAC` writes a dirty line back; it never invalidates and never discards, so
/// cleaning a byte we did not write can only publish that byte's newest value — including another
/// core's, which is that core's own data and which it will clean again anyway. There is no rect
/// whose flush this can get wrong; the only cost of misalignment is a few extra bytes.
///
/// Falls back to the contiguous whole-span form once the columns cover most of the row: each
/// `flush_range` ends in its own `dsb sy`, and past that point `h` barriers cost more than the bytes
/// they save.
fn flush_rect(fb: &FrameBuffer, x: usize, y: usize, w: usize, h: usize) {
    let info = fb.info();
    let row_bytes = info.stride * info.bytes_per_pixel;
    let bpp = info.bytes_per_pixel;
    let y0 = y.min(info.height);
    let y1 = (y + h).min(info.height);
    let x0 = x.min(info.width);
    let x1 = (x + w).min(info.width);
    if y1 <= y0 || x1 <= x0 || bpp == 0 {
        return;
    }
    // Columns cover >= 3/4 of the row (the 4 and the 3 are that fraction, not a pixel size).
    if (x1 - x0) * 4 >= info.width * 3 {
        flush_box(fb, y0, y1 - y0);
        return;
    }
    let span = (x1 - x0) * bpp;
    #[cfg(feature = "witness")]
    C10_FLUSH_BYTES.fetch_add(((y1 - y0) * span) as u64, Ordering::Relaxed);
    for row in y0..y1 {
        fb.flush_range(row * row_bytes + x0 * bpp, span);
    }
}

/// CURSOR-10 — a deferred flush: the bounding box of every rect a pass has dirtied but not yet
/// cleaned.
///
/// [`refresh_locked`] is `undraw_locked` then `draw_locked` under ONE lock, and each used to end in
/// its own whole-scanline `flush_box`. That is two full sweeps per HID report (~553 KB on the bench
/// panel, ~69 MB/s at 125 Hz), and — worse than the bytes — the first sweep PUBLISHES the panel with
/// the arrow taken down and not yet put back. Handing both rects to one union flushed after both
/// pixel phases have run removes the intermediate publication entirely and halves the sweep even
/// before [`flush_rect`]'s column bound is applied.
///
/// Union rather than two rects, because the two boxes are the same sprite one report apart: at 125 Hz
/// they overlap or nearly do, and the union's column span is the sprite's width plus the motion
/// delta. The degenerate case is a teleporting pointer, where the union spans the panel width and
/// [`flush_rect`] falls back to whole scanlines — i.e. one full-scanline sweep, still exactly half of
/// what this path did before, never more.
///
/// The saved-under logic is untouched: nothing here reads or writes `saved`, `off`, or the epoch, and
/// the flush is a cache maintenance operation on pixels already written. The [wc-d] argument is
/// unaffected — it turns on WHAT is in RAM when the compositor verifies, and every byte either phase
/// wrote is still cleaned, once, before the lock is released.
#[derive(Clone, Copy, Default)]
struct FlushUnion(Option<(usize, usize, usize, usize)>);

impl FlushUnion {
    /// Grow the union to include `(x, y, w, h)`.
    fn add(&mut self, x: usize, y: usize, w: usize, h: usize) {
        self.0 = Some(match self.0 {
            None => (x, y, w, h),
            Some((ox, oy, ow, oh)) => {
                let x0 = ox.min(x);
                let y0 = oy.min(y);
                let x1 = (ox + ow).max(x + w);
                let y1 = (oy + oh).max(y + h);
                (x0, y0, x1 - x0, y1 - y0)
            }
        });
    }

    /// Clean everything accumulated, once. A no-op when no phase dirtied anything.
    fn flush(self, fb: &FrameBuffer) {
        if let Some((x, y, w, h)) = self.0 {
            flush_rect(fb, x, y, w, h);
        }
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
///
/// CURSOR-10 — `pend` is the caller's deferred flush. `None` means "clean my rows yourself before
/// returning", which is what every standalone caller wants; `Some` defers the clean to the caller,
/// which is sound ONLY because the caller still holds the sprite lock and cleans before releasing it.
fn undraw_locked(
    sp: &mut Sprite,
    pend: Option<&mut FlushUnion>,
) -> Option<(usize, usize, usize, usize)> {
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
        // CURSOR-11 — a deferred verdict is meaningless once the sprite is down; see `Sprite::pend`.
        sp.pend.reset();
        bump_epoch(sp);
        retract_box();
        return None;
    }
    // CURSOR-9 — consumed HERE, not in `repair`: `repaint` is undraw-then-draw under one acquisition,
    // and `draw_locked` clears the flag, so a consumer downstream of the draw would never see the
    // disturbance the undraw it is repairing was answering. Swap, so one painter's trample buys one
    // repair rather than every later report's.
    let touched = TOUCHED_SINCE_DRAW.swap(false, Ordering::AcqRel);
    let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
    let off = sp.off;
    let saved = &sp.saved;
    // CURSOR-WCR — the colour guard below is one FRONT-buffer read per painted pixel, and this walk
    // is one of the two `refresh_locked` runs per pointer report. Decide the read path once.
    let rd = PanelRead::of(&fb);
    for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
        // CURSOR-4: a pixel already handed back by a masked undraw is not ours and `saved[i]` is
        // stale for it. Restoring it here would stamp pre-pass content over whatever the compositor
        // has since put there — the exact stale-restore hazard the colour guard exists to prevent,
        // except that the colour guard cannot see it (the compositor may legitimately have painted
        // `FILL` there, and more importantly the guard would pass on any pixel the pass left alone).
        if off.get(i) {
            return;
        }
        if i < saved.len() && rd.pixel(x, y) == Some(color) {
            fb.put_pixel(x, y, saved[i]);
        }
    });
    match pend {
        Some(p) => p.add(bx, by, bw, bh),
        None => flush_box(&fb, by, bh),
    }
    sp.drawn = false;
    sp.off.reset();
    // CURSOR-11 — every pending verdict is answered by this restore: the colour guard above already
    // asked each pixel the question `settle_pending_locked` would have, and the sprite is off the
    // panel now, so no bit may survive into the next draw. See `Sprite::pend`.
    sp.pend.reset();
    bump_epoch(sp);
    // CURSOR-6 — retract AFTER the restore, mirroring `draw_locked`'s publish-before-paint for the
    // same reason: the window in which the mirror disagrees with the panel must always be the one
    // where it claims a sprite that is not there, never the one where it denies a sprite that is.
    retract_box();
    // CURSOR-9 — the rect is the REPAIR REQUEST, and it is owed only where a painter could have taken
    // one of our pixels. `None` here means "restored, provably exactly, nothing to mend"; the pixels
    // went back either way.
    if touched { Some((bx, by, bw, bh)) } else { None }
}

/// Restore the pixels the sprite is covering and forget them. A no-op when the sprite is not on the
/// panel, so every painter may call it unconditionally.
///
/// Called by [`super::wm::composite`], `wm`'s desktop erase, and `Screen::flush` around its desktop
/// blit — i.e. by everything that writes to the front framebuffer.
///
/// CURSOR-13 — that last caller used to be the RENDER TASK, wrapping the whole of `Screen::flush`,
/// which put the flush's window composite inside the bracket too and left [`sprite_plan`] answering
/// `None` on every one of those passes. The bracket now belongs to `Screen::flush` and covers only
/// `present_background`; the composite that follows it runs with the sprite on the panel.
pub fn undraw() {
    let restored = {
        let mut sp = SPRITE.lock();
        undraw_locked(&mut sp, None)
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
///
/// **CURSOR-9 — `None` now has two meanings, and both are "nothing to mend".** It has always meant
/// "the sprite was not on the panel"; it now also means "the sprite was restored, and no painter has
/// written inside its box since it was drawn, so the colour guard's residual cannot have bitten".
/// See [`TOUCHED_SINCE_DRAW`] for the predicate and for the painters it does not hear from.
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
///
/// ### CURSOR-11 — this has no caller any more, and is kept deliberately
/// The session-owning pass was its only one, and it now takes [`defer_within`] instead: the handback
/// is what the arrow's absence from the panel COSTS, and for pixels a staged present is going to
/// carry it is a cost with nothing bought. What survives is the ARGUMENT above, unchanged and still
/// load-bearing — it is what [`undraw_within_nosession`] (the sessionless pass, which has no coverage
/// to install and so cannot defer) and [`settle_pending_locked`] (the same question, asked one pass
/// later against a finished front) both rest on. It is retained as the reference statement of that
/// argument and as the always-correct fallback should a future pass need a handback it can pay for.
#[allow(dead_code)]
pub fn undraw_within(boxes: &[(usize, usize, usize, usize)]) {
    let restored = {
        let mut sp = SPRITE.lock();
        undraw_within_locked(&mut sp, boxes).0
    };
    repair(restored);
}

/// CURSOR-11 — the compose-through entry point: leave the arrow ON GLASS and record that this pass's
/// tail owes those pixels a verdict.
///
/// ### The blink this removes (P73)
/// CURSOR-9 stopped the cursor manufacturing damage for QUIET windows; over a PRESENTING one the
/// arrow still blinked at present rate, together with the vug's own fps overlay text. The mechanism
/// is [`undraw_within`] itself. A pass that owns the overlay session hands back every sprite pixel
/// inside its paint set BEFORE the compose, then `compose_into` paints the arrow into the staged rows
/// and the row blit puts it back — so between the mask and the blit sits a whole off-screen compose,
/// and the panel publishes an arrow-less box for exactly that interval. Once per present. That is a
/// duty cycle, not a race, and no care inside the bracket shortens it — which is precisely the
/// argument CURSOR-3 made about WC-I's bracket, applied one level down to the mask that replaced it.
///
/// The undraw was never needed for the pixels the pass is going to COMPOSE. Its whole justification
/// is "a pixel a painter is about to overwrite must be handed back before the overwrite, or
/// `saved[i]` goes stale" — and a pixel that rides the window's staged present is not overwritten
/// behind our back at all: the rows that land on it already contain the arrow, and
/// [`adopt_overlay`] installs the LAYER's pixel as its save-under. Handing it back first buys
/// nothing and costs the blink.
///
/// So this call writes NO framebuffer pixels, takes no `WRITER` lock, bumps no generation and asks
/// for no [`repair`]. It marks [`Sprite::pend`]. Everything else is deferred to the tail, where the
/// front buffer is FINAL and the answer per pixel is knowable — see [`settle_pending_locked`] for
/// the two non-composed outcomes and [`Sprite::pend`] for all three.
///
/// **Only the session owner may call this.** A pass that lost `overlay_open` has no coverage to
/// install and no `compose_into` in its future, so for it the deferral would be a promise nothing
/// keeps: it stays on [`undraw_within_nosession`], unchanged. The WC-F reserved-hit path stays on
/// the full [`undraw`], unchanged. Both still arm CURSOR-9's repair machinery exactly as before.
///
/// `boxes` is the same conservative paint set [`undraw_within`] takes, and is used for the same
/// reason: a pixel outside all of them is a pixel no painter in this pass can reach, so it needs
/// neither a handback nor a verdict and is left entirely alone.
///
/// Returns how many pixels were newly deferred, for the witness.
pub fn defer_within(boxes: &[(usize, usize, usize, usize)]) -> usize {
    let mut sp = SPRITE.lock();
    if !sp.drawn {
        return 0;
    }
    let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
    let off = sp.off;
    let mut pend = sp.pend;
    let mut n = 0usize;
    for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, _c, i| {
        // A pixel an earlier masked undraw already handed back is `off`, and `off` and `pend` are
        // disjoint: that pixel's `saved` is stale and only a redraw can settle it, which is
        // `redraw_off_locked`'s job and not this one's.
        if off.get(i) || pend.get(i) || !in_any(boxes, x, y) {
            return;
        }
        pend.set(i);
        n += 1;
    });
    sp.pend = pend;
    #[cfg(feature = "witness")]
    {
        C11_PASSES.fetch_add(1, Ordering::Relaxed);
        C11_PIX_DEFERRED.fetch_add(n as u64, Ordering::Relaxed);
    }
    n
}

/// CURSOR-11 — settle every [`Sprite::pend`] bit the pass's coverage install did not already claim.
///
/// ### Ordering, and why it is load-bearing (the [wc-d] argument)
/// This runs from [`adopt_overlay`], i.e. with the pass finished, the `BlitGuard` dropped and every
/// window's rows on the panel — and it runs AFTER the coverage install, never before. Both halves of
/// that ordering carry weight:
///
/// * **After the pass.** The front buffer is final here, so `read_pixel` answers the only question
///   that matters — "did a painter in this pass take this pixel?" — with no race left in it. Asking
///   mid-pass would read a pixel some later window is still going to overwrite, and would install a
///   save-under describing content the panel no longer holds. That is the stale-save that stamps a
///   white arrow into a window's rect, which is the failure mode every arc since CURSOR-3 has been
///   arranged around.
/// * **After the install.** [`adopt_overlay`] first writes `ov.saved[i]` — the BACK LAYER's pixel —
///   into `sp.saved[i]` for every covered index and clears their `pend` bit. Those pixels are exactly
///   the ones a staged present delivered with the arrow already in them, so the front now holds our
///   `FILL` there. If this function ran first it would read that `FILL`, see `now == color`, and
///   conclude "nobody painted here" — which is true and useless, because the pixel UNDER the arrow
///   changed: the window presented new content beneath it. `saved[i]` would keep the pre-present
///   pixel, and the next real undraw would restore last frame's window content into a live window.
///   The install is what makes the save-under coherent for that class, and it must therefore retire
///   the bit before this pass sees it. The two sets are disjoint after the install by construction.
///
/// The colour guard is [`undraw_locked`]'s, used in the opposite direction. `now == color` means the
/// arrow survived the pass untouched, so `saved[i]` is still true of the panel and there is nothing
/// to do — no read, no write, no flush, which is the common case and the whole point. `now != color`
/// means a painter took the pixel: `saved[i]` is re-taken from the finished front (which is that
/// painter's content, provably not our own fill — we can see it is not our colour) and the arrow is
/// put back on top.
///
/// The guard's residual is the same one it has always had and is closed the same way: a painter whose
/// content happens to equal `FILL` or `SHADOW` at one of our pixels reads as "untouched" and keeps a
/// stale save. `note_present_over_sprite` armed `TOUCHED_SINCE_DRAW` for every present over the
/// sprite box in this pass, so the next full undraw's [`repair`] damages the windows involved and the
/// composite after it repaints them from source. CURSOR-9's machinery is untouched and is exactly
/// what covers this.
fn settle_pending_locked(sp: &mut Sprite, unsupported_now: &mut bool) {
    let mut any = false;
    for w in 0..MASK_WORDS {
        if sp.pend.0[w] != 0 {
            any = true;
            break;
        }
    }
    if !any {
        return;
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        // Nothing can be read or written; drop the promises rather than carry them into the next
        // pass, where the geometry they index may already be gone.
        sp.pend.reset();
        return;
    }
    let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
    let deferred = sp.pend;
    let off = sp.off;
    let mut wrote = false;
    let mut failed = false;
    // CURSOR-WCR — one FRONT read per deferred pixel; decide the read path once for the walk.
    let rd = PanelRead::of(&fb);
    {
        let saved = &mut sp.saved;
        for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
            if !deferred.get(i) || off.get(i) || i >= saved.len() || failed {
                return;
            }
            match rd.pixel(x, y) {
                // Untouched: the arrow is still on glass and `saved[i]` still describes what is
                // under it. This is the pixel class the arc exists to produce.
                Some(now) if now == color => {}
                Some(now) => {
                    saved[i] = now;
                    fb.put_pixel(x, y, color);
                    wrote = true;
                    #[cfg(feature = "witness")]
                    C11_PIX_REDRAWN.fetch_add(1, Ordering::Relaxed);
                }
                // Fail-closed, exactly as `draw_locked` and `redraw_off_locked` do: a panel with no
                // read-back inverse gets no cursor rather than an unrestorable patch.
                None => failed = true,
            }
        });
    }
    sp.pend.reset();
    if failed {
        sp.unsupported = true;
        *unsupported_now = true;
        return;
    }
    if wrote {
        flush_box(&fb, by, bh);
    }
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
        return (undraw_locked(sp, None), 0);
    }
    // CURSOR-9 — same consumption as the full undraw, and for the same reason. Taken AFTER the
    // `is_ready` delegation above, which consumes it itself.
    let touched = TOUCHED_SINCE_DRAW.swap(false, Ordering::AcqRel);
    let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
    let mut off = sp.off;
    let mut handed_back = 0usize;
    // CURSOR-WCR — the masked undraw's colour guard is the same FRONT read; same one-off decision.
    let rd = PanelRead::of(&fb);
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
            if i < saved.len() && rd.pixel(x, y) == Some(color) {
                fb.put_pixel(x, y, saved[i]);
            }
            off.set(i);
        });
    }
    sp.off = off;
    flush_box(&fb, by, bh);
    (if touched { Some((bx, by, bw, bh)) } else { None }, handed_back)
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
        match draw_locked(&mut sp, None) {
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
    // CURSOR-10 — both phases defer their clean into one union, flushed below.
    let mut pend = FlushUnion::default();
    let restored = undraw_locked(sp, Some(&mut pend));
    if crate::pal::cursor::visible() && !sp.unsupported {
        match draw_locked(sp, Some(&mut pend)) {
            Ok(pos) => *armed_at = pos,
            Err(()) => *unsupported_now = true,
        }
    }
    // Unconditional, and after BOTH phases: every pixel write above is complete, and the restore owes
    // RAM its clean even on the paths where the draw declined or failed (hidden pointer, unsupported
    // panel, unreadable pixel). The lock is still held, so nothing has observed the union half-built.
    let fb = *super::WRITER.lock();
    pend.flush(&fb);
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
    /// CURSOR-6 — the open session did not describe this plan (another pass owns it, or ours was
    /// closed under us). A fifth decline class, and the last silent one.
    pub mismatch: bool,
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
/// `try_lock` only: this runs inside the `BlitGuard` window.
///
/// CURSOR-6 — returns whether the clear was APPLIED. CURSOR-4 discarded a contended clear silently,
/// on an argument about who can hold the lock that does not survive inspection; see [`UNCOVER_LOST`]
/// for the interleave and for what a dropped clear costs the panel. A `false` answer obliges the
/// caller to invalidate the session rather than to shrug.
///
/// A session that does not match this plan returns `true`: there is nothing of OURS to clear, and the
/// coverage belongs to a pass whose bookkeeping is not ours to correct — the original argument, which
/// is sound for this case and only for this case.
#[must_use]
pub fn overlay_uncover(plan: &Plan, bx: usize, by: usize, bw: usize, bh: usize) -> bool {
    let mut ov = match OVERLAY.try_lock() {
        Some(g) => g,
        None => return false,
    };
    if !overlay_matches(&ov, plan) {
        return true;
    }
    let boxes = [(bx, by, bw, bh)];
    let mut covered = ov.covered;
    for_each_sprite_pixel(plan.bx, plan.by, plan.bw, plan.bh, plan.s, |x, y, _c, i| {
        if in_any(&boxes, x, y) {
            covered.clear(i);
        }
    });
    ov.covered = covered;
    true
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
        // CURSOR-6 — counted. This was the one decline exit CURSOR-4 left silent, so a reader
        // reconciling the breakdown against `offers - taken` found a gap with no name on it.
        #[cfg(feature = "witness")]
        C6_MISMATCH.fetch_add(1, Ordering::Relaxed);
        out.mismatch = true;
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
        return Composed { taken: 0, missed, locked: false, stale: false, mismatch: false };
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
    // CURSOR-6 — swap, not load: the flag is per-pass state and the tail is the one place that can
    // retire it. Reading it without clearing would make one dropped clear poison every later pass;
    // clearing it without reading would lose the only evidence the pass produced.
    let uncover_lost = UNCOVER_LOST.swap(false, Ordering::Relaxed);
    let restored = {
        let mut sp = SPRITE.lock();
        // CURSOR-4 — the session closes here, unconditionally, whatever the pass managed. `composite`
        // routes every exit through its tail, so this is the one place that can release it, and a
        // session that outlived its pass would lock the mechanism out for the rest of the boot.
        let coherent = {
            let mut ov = OVERLAY.lock();
            // CURSOR-5's coherence question, computed on its OWN terms and nothing else: does the
            // open session still describe the sprite that exists? This is what `adopt_incoh` counts,
            // and it must stay answerable independently of anything CURSOR-6 added.
            let c5_coherent = ov.session
                && sp.drawn
                && ov.epoch == sp.epoch
                && (ov.bx, ov.by, ov.bw, ov.bh, ov.s) == (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
            // CURSOR-6 — `!uncover_lost` is a SECOND, independent reason to decline the install. A
            // dropped coverage clear means at least one `covered` bit describes a pixel some window
            // has since overwritten, and the install below would hand that pixel's stale save to the
            // module AND clear its `off` bit. Neither the generation nor the geometry moved, so
            // `c5_coherent` cannot see it. Declining routes the pass to `refresh_locked`, which
            // re-establishes the whole sprite from the finished front buffer — CURSOR-3's fallback,
            // always available and always correct.
            //
            // The two are AND-ed for the DECISION and counted SEPARATELY for the evidence. The first
            // cut suppressed `adopt_incoh` whenever a lost clear coincided, which would have hidden
            // a real CURSOR-5 incoherence behind a CURSOR-6 one — a silent counter, which is exactly
            // the failure mode that made P65v2 unreadable.
            let coherent = c5_coherent && !uncover_lost;
            if coherent {
                // Install the layer-derived save-under for every pixel a present delivered. Those
                // pixels are on the panel already — this is bookkeeping, not painting, which is what
                // makes the composed path cheaper than the bracket as well as steadier.
                let (bx, by, bw, bh, s) = (sp.bx, sp.by, sp.bw, sp.bh, sp.s);
                let covered = ov.covered;
                let src = &ov.saved;
                let mut off = sp.off;
                // CURSOR-11 — the install RETIRES the pending verdict for every pixel it claims, and
                // it does so here rather than leaving it to `settle_pending_locked` because these are
                // the pixels whose save-under the layer, and only the layer, can supply. The panel
                // holds our `FILL` at each of them now (the row copies delivered it), so a front read
                // would answer "untouched" and keep the PRE-PRESENT pixel as the save-under — the
                // stale save that stamps last frame's window content back into a live window. See
                // `settle_pending_locked` for why this ordering is the whole coherence argument.
                let mut pend = sp.pend;
                let dst = &mut sp.saved;
                for_each_sprite_pixel(bx, by, bw, bh, s, |_x, _y, _c, i| {
                    if covered.get(i) && i < dst.len() && i < src.len() {
                        dst[i] = src[i];
                        off.clear(i);
                        if pend.get(i) {
                            pend.clear(i);
                            #[cfg(feature = "witness")]
                            C11_PIX_INSTALLED.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
                sp.off = off;
                sp.pend = pend;
            }
            ov.session = false;
            ov.covered.reset();
            // Counted on CURSOR-5's OWN predicate, so a lost clear can neither create nor mask an
            // `adopt_incoh`. The two mechanisms overlap freely and each is counted where it belongs
            // (`uncover_lost` is bumped at `note_uncover_lost`); a pass that hit both appears in
            // both, which is the truth and is what lets a bench reader add them up.
            #[cfg(feature = "witness")]
            if !c5_coherent {
                C5_ADOPT_INCOH.fetch_add(1, Ordering::Relaxed);
            }
            coherent
        };
        // The sprite's own generation moved under us (another core ran a full `repaint` mid-pass), or
        // the pointer has moved since the plan was taken, or it has gone invisible: none of the
        // per-pixel state describes the panel any more, so fall back to the whole-sprite refresh.
        // `undraw_locked` skips the off-panel pixels rather than stamping their stale saves, and
        // `draw_locked` then re-establishes the sprite entirely from the finished front buffer.
        // CURSOR-11 — the fallback below answers the PENDING class too, and answers it correctly
        // without knowing about it: `undraw_locked`'s colour guard asks each pixel exactly the
        // question `settle_pending_locked` asks (does the panel still hold our colour?), declines the
        // pixels a painter took, and `draw_locked` re-saves every one of them from the finished
        // front. Both reset `pend`, so no verdict survives the pass either way. An incoherent tail is
        // therefore a BRACKETED pass — the arrow does leave the glass for the length of one refresh —
        // which is the cost this fallback has always had and what `[cursor11] bracketed=` measures.
        if coherent
            && want == Some((sp.bx, sp.by))
            && crate::pal::cursor::visible()
            && !sp.unsupported
        {
            // CURSOR-11 — the deferred class, settled against the FINISHED front. Runs before
            // `redraw_off_locked` only for tidiness (the two sets are disjoint: `pend` and `off`
            // never overlap), and after the coverage install for a reason that is not tidiness at
            // all — see `settle_pending_locked`.
            settle_pending_locked(&mut sp, &mut unsupported_now);
            // The straddle remainder: pixels the masked undraw took down that no layer delivered —
            // over bare desktop, over a window that declined the overlay, or over a window this pass
            // never drew. The front buffer is finished now, so a save-and-draw here has exactly
            // `draw_locked`'s provenance, and it touches strictly FEWER front pixels than CURSOR-3's
            // bracket, which repainted the whole sprite.
            redraw_off_locked(&mut sp, &mut unsupported_now);
            None
        } else {
            #[cfg(feature = "witness")]
            C11_BRACKETED.fetch_add(1, Ordering::Relaxed);
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
    // CURSOR-WCR — the off-pixel settle is a FRONT save-under read per pixel; same one-off decision.
    let rd = PanelRead::of(&fb);
    {
        let saved = &mut sp.saved;
        for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
            if !off.get(i) || failed || i >= saved.len() {
                return;
            }
            match rd.pixel(x, y) {
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
///
/// CURSOR-10 — `pend` as in [`undraw_locked`]: `Some` defers this draw's clean to the caller, so the
/// undraw's restore and the draw's arrow reach RAM in ONE sweep and the panel is never published with
/// the arrow missing.
fn draw_locked(
    sp: &mut Sprite,
    pend: Option<&mut FlushUnion>,
) -> Result<Option<(i32, i32)>, ()> {
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

    // Save-under, PAINTED PIXELS ONLY (36 reads at scale 1, 144 at the bench panel's scale 2, against
    // 1296 for the whole box). The per-frame cost matters here because WC-E composites on every
    // desktop flush, which brackets this module ~20 times a second on the bench — and, since
    // CURSOR-X86, `pal::cursor`'s repaint choke point runs it once per HID report at ~125 Hz.
    //
    // CURSOR-WCR — and on an x86 WC panel each of those reads is an aperture round trip, so the walk
    // takes the aligned-word path: one transaction per pixel instead of three. See `PanelRead`.
    let mut failed = false;
    let rd = PanelRead::of(&fb);
    {
        let saved = &mut sp.saved;
        for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, i| {
            if failed || i >= saved.len() {
                return;
            }
            match rd.pixel(x, y) {
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
    // CURSOR-11 — and nothing is pending either: every `saved` entry above was just taken from the
    // finished front, which is the strongest form of the verdict a pending bit was waiting for.
    sp.pend.reset();
    bump_epoch(sp);
    // CURSOR-6 — publish BEFORE the pixels go down, not after: a lock-free reader that saw "no
    // sprite" while the arrow was already on the panel would UNDERCOUNT the very overwrite the mirror
    // exists to detect, and an early publish can only over-count (a painter charged for a pixel the
    // arrow had not reached yet). Bias the diagnostic towards false positives, never false negatives.
    //
    // CURSOR-9 — and the disturbance flag is cleared HERE, before the pixels go down, for the same
    // bias: a painter that tramples the arrow while this loop is still running arms it again and is
    // charged for a sprite it may only partly have met, which costs one spurious repair. Clearing
    // after the paint would instead swallow that painter's arming, which is the direction that leaves
    // a stale pixel inside a window's rect.
    TOUCHED_SINCE_DRAW.store(false, Ordering::Release);
    publish_box(sp);

    for_each_sprite_pixel(bx, by, bw, bh, s, |x, y, color, _i| {
        fb.put_pixel(x, y, color);
    });
    match pend {
        Some(p) => p.add(bx, by, bw, bh),
        None => flush_box(&fb, by, bh),
    }

    // CURSOR-1 witness: once, at the first draw of the boot. Input-driven by construction (nothing
    // reaches here before a pointer report), so quiet boot is preserved and the QEMU gate — which has
    // no HID pointer — never prints it. Emitted by the caller, outside the lock.
    if !ARMED.swap(true, Ordering::Relaxed) {
        return Ok(Some((px, py)));
    }
    Ok(None)
}
