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

//! CRYSTAL MENU — the SHARD menu, UnaOS's first live menu, hung off the brand crystal.
//!
//! # What this is, and how it differs from the menu PROTOCOL
//!
//! The design ledger at the foot of [`super::menubar`] specifies a renderer-agnostic menu PROTOCOL:
//! apps publish a menu tree as bandy messages, picks come back principal-stamped, and no renderer is
//! privileged. That is a later, larger arc. **This module is not that.** It is the SYSTEM menu — the
//! one macOS hangs off its apple and UnaOS hangs off its crystal — and it is KERNEL-OWNED end to end:
//! no app publishes it, no principal but the kernel authors it, and the items act on the machine, not
//! on a focused program. Because the publisher and the actor are both the kernel, this menu needs
//! none of the protocol's registry, wire encoding, or cross-principal pick delivery. It follows the
//! protocol's *shape* only where it fits: a fixed TREE of items addressed by identity, a PICK that
//! fires a named action, and a witness that makes every item falsifiable.
//!
//! # The items — Peter's naming, LOCKED (2026-08-11)
//!
//! The machine is a **SHARD**; the bus carries **Shard Messages**. The menu reads:
//!
//! | # | item              | action                                                          |
//! |---|-------------------|-----------------------------------------------------------------|
//! | 0 | About This Shard  | prints the Shard identity + version to the log (REAL)           |
//! | 1 | — (separator)     | —                                                               |
//! | 2 | Sleep             | honest STUB — prints `unimplemented: Sleep` (no ACPI S3 path)    |
//! | 3 | Restart           | honest STUB — prints `unimplemented: Restart` (no reboot path)   |
//! | 4 | Shut Down         | x86: REAL — [`crate::arch::acpi_power::poweroff`] (ACPI S5). aarch64: honest STUB |
//!
//! PI-DESK — **the RENDER and the HIT-TEST cross to aarch64 whole; two of the four ACTIONS do not,
//! and the menu says so on the wire rather than pretending.** `About` is REAL on both arches. On the
//! Pi, `Restart` and `Shut Down` join `Sleep` as honest stubs, each printing a line that names the
//! missing mechanism by name (PSCI `SYSTEM_RESET` / `SYSTEM_OFF` need an EL3 secure monitor, and Pi 4
//! bare-metal runs at EL2 with nothing behind the `smc`; the real Pi wiring is the BCM2711
//! watchdog/`PM_RSTS` block, which is a driver arc). See [`fire`] for the full ledger.
//!
//! **A stub is not a no-op and not a fake success.** Sleep and Restart RENDER as pickable items — a
//! menu item that could not be shown pickable would be unfalsifiable — and their pick prints one
//! honest line naming the verb and that it is unimplemented. Peter sees on glass/serial that the item
//! is a stub, not that the menu is broken.
//!
//! # Destructive-action discipline
//!
//! Shut Down and Restart end or interrupt the session. Per the safety model a destructive action must
//! be either confirmed or clearly deliberate; **a menu pick IS deliberate** — the operator opened the
//! menu, moved to the item, and pressed it — so no second "really?" affordance is added this arc. What
//! is guaranteed instead is that the destructive path fires on a PICK and on nothing else: opening the
//! menu, hovering (there is no hover this arc), or dismissing never reaches [`fire`]. And the one
//! action that halts the machine — Shut Down — is fired by exactly one caller, the live press in
//! [`press_at`]; the fixture proves Shut Down's ROUTING (that a press on its row resolves to
//! [`Verb::ShutDown`]) through the pure resolver [`item_at`] and NEVER drives a press at its
//! coordinates, so no gate can power the machine off. See [`selftest`].
//!
//! # How it composites — a transient SURFACE, not a strip tenant
//!
//! The dropdown is painted through [`super::strip::paint`] and erased through [`super::strip::erase_rect`]
//! — the same front-buffer row-run discipline every non-compositor painter uses — from [`compose`],
//! which [`super::strip::compose_all`] calls at the composite tail beside the dock and the bar. It is
//! deliberately **not** a `strip::TENANTS` entry: a registered strip consumes a PERMANENT occlusion
//! slot, and this surface is modal and momentary, not standing furniture.
//!
//! # MENU-OCC — a first-class occluder while open, without a tenancy
//!
//! Being transient does NOT mean unprotected. While the menu is open it is topmost by construction —
//! [`super::wm::composite_once`] paints [`compose`] at the pass TAIL, after every window — so a window
//! whose blit crosses the dropdown must WITHHOLD its columns or it overwrites the menu mid-frame (the
//! Boot C defect, "menubar menu gets overwritten"). Two clip paths carry the menu, on opposite sides
//! of the compositor:
//!
//! * **The DESKTOP present** ([`super::screen::present_background`]) subtracts [`open_rect`] directly,
//!   so the shell's whole-panel `clear_screen` cannot flush the menu.
//! * **The WINDOW blit** ([`super::wm::occ_clip`]) now pushes [`open_rect`] into every window's clip,
//!   through a DEDICATED transient occluder slot (`wm::MENU_OCC_MAX`) rather than a strip tenancy — so
//!   no permanent slot is spent and `wm::FURNITURE_MAX == strip::STRIP_MAX` stays true, while a window
//!   moved or dragged under the open menu is clipped against it.
//!
//! On DISMISS, [`repaint_vacated`] gives the erased rect the `damage_intersecting` + full-present
//! treatment [`super::wm::reclaim`] gives a vacated window box, so the windows that had been withholding
//! those rows repaint them cleanly instead of leaving a `DESKTOP_BG` hole.
//!
//! # It is only reachable when the bar is enabled
//!
//! The crystal is part of the menu bar, which is DEFAULT OFF ([`super::menubar`]). With the bar off
//! there is no crystal to click and [`press_at`] returns `false` for every point. Disabling the bar
//! while the menu is open tears the menu down — [`super::menubar::set_enabled`] calls [`dismiss`] — so
//! the surface never outlives the mark it hangs from.

use super::{menubar, strip, theme, wm};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

// ---------------------------------------------------------------------------
// The menu model — the SHARD tree
// ---------------------------------------------------------------------------

/// A menu verb: what a pick DOES. Kernel-authored, so this is a closed set, not a wire tag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// About This Shard — prints the Shard identity + version. REAL.
    About,
    /// Sleep — ACPI S3 suspend. STUB: no S3 path exists; the pick prints `unimplemented`.
    Sleep,
    /// Restart — a warm reboot. STUB: no reboot path exists; the pick prints `unimplemented`.
    Restart,
    /// Shut Down — ACPI S5 soft-off. REAL, and the one action that halts the machine.
    ShutDown,
}

impl Verb {
    /// The verb's name for the witness line.
    const fn name(self) -> &'static str {
        match self {
            Verb::About => "About",
            Verb::Sleep => "Sleep",
            Verb::Restart => "Restart",
            Verb::ShutDown => "ShutDown",
        }
    }

    /// `true` when the verb is BACKED by a real action, `false` when it is an honest stub. On the
    /// witness so a capture reads `action=real` or `action=stub` beside the pick.
    const fn real(self) -> bool {
        matches!(self, Verb::About | Verb::ShutDown)
    }

    /// The verb's stable ordinal for the witness (`u8`), independent of its row index.
    const fn ord(self) -> u8 {
        match self {
            Verb::About => 0,
            Verb::Sleep => 1,
            Verb::Restart => 2,
            Verb::ShutDown => 3,
        }
    }
}

/// One row of the menu: an item with a label and verb, or a separator (`verb == None`).
struct Row {
    label: &'static str,
    verb: Option<Verb>,
}

/// **The SHARD tree.** Order is Peter's, LOCKED: About first, a separator, then the power verbs.
const ROWS: [Row; 5] = [
    Row { label: "About This Shard", verb: Some(Verb::About) },
    Row { label: "", verb: None },
    Row { label: "Sleep", verb: Some(Verb::Sleep) },
    Row { label: "Restart", verb: Some(Verb::Restart) },
    Row { label: "Shut Down", verb: Some(Verb::ShutDown) },
];

// ---------------------------------------------------------------------------
// Metrics — all derived, none guessed
// ---------------------------------------------------------------------------

/// The glyph advance and cell height the menu draws text in — [`wm::TITLE_CELL_W`] /
/// [`wm::TITLE_CELL_H`], the same face metrics the bar caption and the dock tiles resolve to, so a
/// face change moves all of them together. FONT (GR27): the shared anti-aliased face's cell is not
/// square, so the two axes are named separately (the old square `CELL` and its `SCALE` are retired
/// with the 1-bit bitmap they described).
const CELL_W: usize = wm::TITLE_CELL_W;
const CELL_H: usize = wm::TITLE_CELL_H;

/// FONT-METRIC — the atlas those metrics come from, named once so the layout constants and the
/// glyph call can never disagree about which face the menu is drawing.
const FACE: super::font::Face = super::font::Face::Chrome;

/// An item row's height, px: the glyph cell plus 8 px of clearance split top and bottom, so the
/// glyph sits with 4 px of air above and below. DERIVED from the cell, so when FONT-METRIC moved
/// the chrome face from 16 to 20 px the row moved from 24 to 28 with it and the air stayed 4 —
/// which is the whole point of writing it as an expression. Bounded, not pinned, below.
const ITEM_H: usize = CELL_H + 8;

/// A separator row's height, px — a thin band carrying one keyline, centred.
const SEP_H: usize = 7;

/// The menu's 1 px keyline border, all four sides — the same [`theme::FRAME_LINE`] the window frame
/// and the bar keyline use.
const BORDER: usize = 1;

/// The horizontal inset of item text from the menu's inner edge — one [`strip::PAD`], the kit's gap.
const PADX: usize = strip::PAD;

/// The longest item label in glyphs, walked at compile time so the menu width is a function of the
/// tree rather than a magic number that a relabelled item could silently overflow.
const fn max_label_glyphs() -> usize {
    let mut m = 0;
    let mut i = 0;
    while i < ROWS.len() {
        let l = ROWS[i].label.len();
        if ROWS[i].verb.is_some() && l > m {
            m = l;
        }
        i += 1;
    }
    m
}

/// The menu's total height, walked at compile time from the row heights plus the two borders.
const fn menu_height() -> usize {
    let mut h = 2 * BORDER;
    let mut i = 0;
    while i < ROWS.len() {
        h += if ROWS[i].verb.is_some() { ITEM_H } else { SEP_H };
        i += 1;
    }
    h
}

/// The menu's width, px: both borders, both insets, and the widest label.
const MENU_W: usize = 2 * BORDER + 2 * PADX + max_label_glyphs() * CELL_W;

/// The menu's height, px.
const MENU_H: usize = menu_height();

/// How many pickable ITEMS the tree carries (separators excluded) — for the witness `items=` term.
const fn item_count() -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < ROWS.len() {
        if ROWS[i].verb.is_some() {
            n += 1;
        }
        i += 1;
    }
    n
}
const ITEM_COUNT: usize = item_count();

const _: () = {
    // The row height must clear the glyph it centres, or the label is cut.
    assert!(ITEM_H >= CELL_H);
    // FONT-METRIC — was `ITEM_H == 24`, a pin on the 16 px face's arithmetic that a face change is
    // SUPPOSED to move. What actually has to hold is the clearance the row was designed around: 4 px
    // of air above and below the cell, exactly, whatever the cell is.
    assert!(ITEM_H == CELL_H + 8 && (ITEM_H - CELL_H) % 2 == 0);
    // The separator band must hold its keyline with air either side.
    assert!(SEP_H >= 3);
    // A menu with no pickable item would be a surface with nothing to pick.
    assert!(ITEM_COUNT >= 1);
    // The menu must fit the strip painter's scratch, exactly as any strip must.
    assert!(MENU_W <= strip::MAX_STRIP_W);
    // The widest label must be representable — a sanity floor, not a real bound.
    assert!(max_label_glyphs() >= 1);
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Is the menu open? The whole of the modal state. `false` at boot and after every dismissal.
static OPEN: AtomicBool = AtomicBool::new(false);

/// What the dropdown last put on the panel — one signature, one packed rect. The strip primitive's
/// damage slot, so the menu repaints only when it opens, moves, or closes.
static SLOT: strip::Slot = strip::Slot::new();

/// The cost ledger, on the dock's and bar's precedent: NOT `witness`-gated, because the metal image
/// is built without `witness` and a cost claim absent from it is not a claim.
static LEDGER: strip::Ledger = strip::Ledger::new();

/// Falsifiable counters. `opens`/`dismisses` bracket the modal life; `picks` counts fired actions and
/// `last_verb` (a [`Verb::ord`], or `0xFF` for none) names the last one — so a capture reads the flow
/// without a debugger.
static OPENS: AtomicU64 = AtomicU64::new(0);
static DISMISSES: AtomicU64 = AtomicU64::new(0);
static PICKS: AtomicU64 = AtomicU64::new(0);
static LAST_VERB: AtomicU8 = AtomicU8::new(0xFF);

/// CLICK-BAND — **what the LAST consumed press did.** [`press_at`] answers the router `true`, and a
/// caller that could not say WHAT the menu did with the press could name the band and nothing else —
/// which is exactly the reading gap PA41's "the crystal ignores clicks" round was built on. Written by
/// every consuming arm of [`press_at`], read by [`last_press_outcome`] immediately after the call on
/// the same task; no cross-core reader.
static PRESS_OUTCOME: AtomicU8 = AtomicU8::new(0);
const OUT_OPEN: u8 = 1;
const OUT_PICK: u8 = 2;
const OUT_KEPT: u8 = 3;
const OUT_DISMISS: u8 = 4;
// ARC D M1 — **THE CLICK-HOLD DUPLICATE GUARD IS GONE FROM THIS FILE.** `ACTED_GEN`, `OUT_DUP` and
// the `dup-hold` outcome word existed because the input path could deliver TWO press edges for one
// physical click: `ehci::note_buttons`'s quiet-gap arm manufactured a press out of a drifting hold,
// and the x86 router's stale-latch arm routed it. This menu was the only press target where that was
// visible — every other one is idempotent, while a TOGGLE opened and immediately closed, "1 click to
// activate and 1 to open" (bench, 2026-08-18).
//
// The producer now guarantees one press edge per gesture (motion discriminates a drift from a
// re-landed finger, and a recovered press synthesises its own missing release first), so a second
// edge inside one hold cannot be produced and this guard could never fire again. A guard that cannot
// fire is not protection, it is a claim that the bug is still there — so it comes out with the bug.

/// CLOBBER-REPAIR (PA41) — passes in which a window the compositor painted had intersected the rows
/// the OPEN dropdown last painted. The bar's and the dock's counter, on the same terms; `clob=` on the
/// `[crystal]` ledger line is the falsifier for "the menu survives a window blit crossing it".
static CLOBBERS: AtomicU64 = AtomicU64::new(0);

/// CLICK-BAND — the last consumed press's outcome, as the witness word.
pub fn last_press_outcome() -> &'static str {
    match PRESS_OUTCOME.load(Ordering::Relaxed) {
        OUT_OPEN => "open",
        OUT_PICK => "pick",
        OUT_KEPT => "kept-open",
        OUT_DISMISS => "dismiss",
        _ => "none",
    }
}


// ---------------------------------------------------------------------------
// Geometry — the dropdown rect, and the row layout inside it
// ---------------------------------------------------------------------------

/// **The dropdown's rect on a `pw` x `ph` panel, or `None`.** Anchored under the crystal: its left
/// edge aligns with the crystal's left (as macOS drops its apple menu from the logo's left), its top
/// is the bar's bottom edge, and it is clamped so it never runs off the right or bottom of the panel.
///
/// `None` when the bar is absent (so there is no crystal to hang from) or the panel is too small to
/// hold the menu below the bar at all — the same decline-rather-than-squeeze rule the strip
/// constructors follow.
fn menu_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    let (cx, _cy, _cw, _ch) = menubar::crystal_box_abs(pw, ph)?;
    let (_bx, by, _bw, bh) = menubar::strip_rect(pw, ph)?;
    let my = by + bh; // flush under the bar
    if MENU_W > pw || my + MENU_H > ph {
        return None; // panel cannot host the menu below the bar
    }
    // Clamp x so the right edge stays on the panel; the left never goes past 0 because the crystal is
    // one PAD in from the left already.
    let mx = if cx + MENU_W > pw { pw - MENU_W } else { cx };
    Some((mx, my, MENU_W, MENU_H))
}

/// MENUFIT — **the dropdown's LIVE extent: its rect while the menu is open, `None` while it is not.**
///
/// The one accessor every other writer asks "where is the SHARD menu". The dropdown is a TRANSIENT
/// surface and deliberately not a [`strip::TENANTS`] member (it takes no occlusion slot), so
/// `strip::rects` does not report it and the desktop layer's whole-panel writes — `console::draw`
/// opens every repaint with a `clear_screen` — flushed the shell's background straight over an open
/// menu. `Screen::present_background` now subtracts this rect alongside the strips, which closes it.
///
/// Published from [`menu_rect`], the same function [`compose`] paints from and [`press_at`] hit-tests
/// against, for the reason the SHELLDESK review gave when it recorded this defect: re-deriving the
/// menu's geometry in `screen.rs` is precisely the drift the registry exists to prevent, so the fix
/// is an accessor here, not a second copy there.
///
/// `None` while closed, so a boot that never opens the menu pays one relaxed load per desktop
/// present and computes no geometry at all. It reports the rect from the instant [`open`] flips
/// `OPEN`, which can be one composite before [`compose`] has PAINTED those pixels — the same bounded
/// residual the strips carry, and bounded the same way: the desktop withholds rows the menu is about
/// to own rather than rows it has stopped owning, and `compose` runs on the next composite.
pub fn open_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    if !OPEN.load(Ordering::Relaxed) {
        return None;
    }
    menu_rect(pw, ph)
}

/// MENU-DRIVE / REVIEW — **does the SHARD menu owe a paint or an erase that only a composite can
/// discharge?** The third term in [`super::wm::composite`]'s "is anything OWED" tests, beside
/// `any_damaged` (a dirty window) and `deferred_owed` (a queued erase box).
///
/// Two relaxed loads, no lock — the same shape and cost as `deferred_owed`, and for the same reason:
/// it runs on the re-run loop and the lost-wakeup gate of every present, on every core.
///
/// ### The gap it closes, and why the in-place retry could not
///
/// [`open`]/[`dismiss`] flip `OPEN` and then call `wm::composite()` themselves, with one verified
/// retry. That retry recovers the case where the FIRST pass declined but the holder released before
/// the second — but if a holder keeps `COMP_GATE` for milliseconds (a `[wc-d] verify` on a witness
/// build holds it over a second), BOTH the pass and its retry decline, `COMP_PENDING` is published,
/// and the holder runs its re-run loop. That loop is the guaranteed painter for a dirty window or a
/// queued erase — but an open menu is NEITHER, so before this term the loop's `if !dmg && !owed`
/// broke and the menu stayed open-in-state, invisible on glass: the exact pre-fix Boot B signature
/// (`crystal_press=open`, no `[crystal]` rollup), reachable by Boot B's own gesture — close the last
/// window (that close is the gate holder), then immediately press the crystal.
///
/// The condition is the paint contract [`compose`] itself acts on, read the other way round:
///  * **OPEN and the slot is EMPTY** — the dropdown is in state but not yet on the panel: a PAINT is
///    owed.
///  * **CLOSED and the slot is NON-EMPTY** — the dropdown was dismissed but its pixels are still on
///    the panel: an ERASE is owed.
/// Either way the next `compose` discharges it; this makes the gate holder run that `compose`.
pub fn paint_owed() -> bool {
    let open = OPEN.load(Ordering::Relaxed);
    let owns_pixels = SLOT.packed() != 0;
    open != owns_pixels
}

/// MENU-DRIVE / REVIEW — count a composite the gate holder ran ONLY because the SHARD menu owed a
/// paint or erase, and name the first one on the wire. The `wcg::erase_wakeup_rescue` precedent: a
/// rescue is the mechanism working (not a FORBID), but it must be shown REACHABLE — a boot with
/// `menu_rescues=0` never exercised the holder-paints-the-menu path, and one with `>0` is a boot
/// where the pre-fix in-place retry would have stranded the menu.
#[cfg(feature = "witness")]
static MENU_RESCUES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "witness")]
pub fn menu_wakeup_rescue() {
    let n = MENU_RESCUES.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 {
        serial_println!("[crystal] rollup scope=wakeup menu_rescues={} -> RESCUED", n);
    }
}
#[cfg(not(feature = "witness"))]
#[inline]
pub fn menu_wakeup_rescue() {}

/// The top of row `idx` as an offset from the menu's top edge, in px — border plus every earlier row.
fn row_top(idx: usize) -> usize {
    let mut y = BORDER;
    let mut i = 0;
    while i < idx {
        y += if ROWS[i].verb.is_some() { ITEM_H } else { SEP_H };
        i += 1;
    }
    y
}

/// Which row, if any, the menu-local vertical offset `ly` falls inside. Borders and out-of-range
/// answer `None`.
fn row_at(ly: usize) -> Option<usize> {
    if ly < BORDER || ly >= MENU_H - BORDER {
        return None;
    }
    for i in 0..ROWS.len() {
        let top = row_top(i);
        let h = if ROWS[i].verb.is_some() { ITEM_H } else { SEP_H };
        if ly >= top && ly < top + h {
            return Some(i);
        }
    }
    None
}

/// **The pure hit resolver: which VERB does panel point `(px, py)` land on, if any.**
///
/// The whole of the routing decision, factored out with NO side effect precisely so the fixture can
/// prove that a press on the Shut Down row resolves to [`Verb::ShutDown`] WITHOUT firing it. A press
/// inside the menu but on a separator, a border, or the inner padding answers `None`; a press outside
/// the menu answers `None`.
fn item_at(r: strip::Rect, px: usize, py: usize) -> Option<Verb> {
    let (mx, my, mw, mh) = r;
    if px < mx || px >= mx + mw || py < my || py >= my + mh {
        return None;
    }
    let row = row_at(py - my)?;
    ROWS[row].verb
}

/// Does panel point `(px, py)` fall anywhere inside the menu rect (border included)?
fn menu_contains(r: strip::Rect, px: usize, py: usize) -> bool {
    let (mx, my, mw, mh) = r;
    px >= mx && px < mx + mw && py >= my && py < my + mh
}

// ---------------------------------------------------------------------------
// Open / dismiss / pick
// ---------------------------------------------------------------------------

/// Open the menu. Idempotent: opening an open menu is a no-op that neither re-counts nor repaints.
/// Fixture-direct callers come through here; the live press arm calls [`open_via`] so the witness
/// names WHERE in the press cell the click landed.
fn open(pw: usize, ph: usize) {
    open_via(pw, ph, "fixture-direct");
}

/// [`open`], with the press's provenance on the wire. FITTS-CORNER: the open line's `via=` word says
/// whether the consumed press hit the painted glyph itself (`crystal-glyph`), the widened corner
/// cell around it (`corner-zone` — the new pixels this arc claims, so a capture can tell a flick
/// into the corner from an aimed click), or no press at all (`fixture-direct`).
fn open_via(pw: usize, ph: usize, via: &str) {
    if OPEN.swap(true, Ordering::AcqRel) {
        return;
    }
    OPENS.fetch_add(1, Ordering::Relaxed);
    let (mw, mh, mx, my) = menu_rect(pw, ph).map(|(x, y, w, h)| (w, h, x, y)).unwrap_or((0, 0, 0, 0));
    serial_println!(
        ":: SHARD-MENU: crystal_press=open via={} menu={}x{}+{}+{} items={} ::",
        via, mw, mh, mx, my, ITEM_COUNT
    );
    // MENU-DRIVE (x86 trunk 122ed63e, ported; PA41 on the Pi) — **an open menu must DRIVE the pass
    // that paints it.** [`compose`] runs only from `strip::compose_all` at the tail of
    // `wm::composite_once`, and every OTHER state-changing gesture (a close, a drag, a zoom, a
    // minimise, a dock raise) runs a composite itself — this one did not. On the emptied late-boot
    // desktop nothing else composites (the render lane blocks on its channel, the backdrop's timer
    // flush carries no damage, `wm::service_damage` returns with no damaged row), so the operator's
    // presses opened the menu IN STATE while the glass never changed: on x86 that read as
    // `crystal_press=open` with no `[crystal]` rollup for the next 30 s; on the Pi, where the mouse
    // itself drives composites, it read as a menu that appeared only if you kept moving the pointer.
    // Task context only (the click router, `key_escape`, `set_enabled(false)` and the fixtures), so
    // the composite is taken directly.
    super::wm::composite();
    // BELT-AND-BRACES retry, and it is NOT dead weight on aarch64 even though `wm::composite` has no
    // decline path there (it IS `composite_once`): `strip::paint` can still decline the pass on a
    // contended SCRATCH or a surface that is not yet `word4`, and then the slot is untouched and the
    // menu is open with nothing on the glass. [`paint_owed`] is the same test `compose` acts on, so a
    // retry runs iff the first pass did not land. On x86 this also closes the narrow window where the
    // pass was declined into a concurrent `COMP_GATE` holder that has since released.
    if paint_owed() {
        super::wm::composite();
    }
}

/// Dismiss the menu. Idempotent. The next [`compose`] erases whatever rect the dropdown owned — the
/// strip primitive's vacate rule — so dismissal leaves the panel exactly as it was before the open.
/// `reason` names WHY on the witness (`outside` / `pick` / `escape` / `bar-off`) so a capture can tell
/// a cancel from a selection.
fn dismiss(reason: &str) {
    if !OPEN.swap(false, Ordering::AcqRel) {
        return;
    }
    DISMISSES.fetch_add(1, Ordering::Relaxed);
    serial_println!(":: SHARD-MENU: crystal_press=dismiss reason={} ::", reason);
    // MENU-DRIVE — the mirrored half of [`open`]'s rule. The erase ([`compose`]'s closed path, which
    // also hands the vacated rows back to their owners through [`repaint_vacated`]) runs only from a
    // composite, and on a static desktop no other pass is coming — an on-glass dropdown would outlive
    // its Escape. A dismiss of a menu that never painted erases nothing (the slot is clear) and the
    // pass is one bounded walk.
    super::wm::composite();
    if paint_owed() {
        super::wm::composite();
    }
}

/// **Public dismissal**, for [`super::menubar::set_enabled`]: turning the bar off must tear the menu
/// down, or the dropdown would outlive the crystal it hangs from with nothing left to dismiss it.
pub fn dismiss_for_bar_off() {
    dismiss("bar-off");
}

/// **Fire a verb's action.** THE ONE PLACE an action happens, and the only caller of the halting
/// Shut Down path.
///
/// - `About`    — prints the Shard identity + version (REAL).
/// - `Sleep`    — honest STUB: prints `unimplemented: Sleep`, no side effect.
/// - `Restart`  — honest STUB: prints `unimplemented: Restart`, no side effect.
/// - `ShutDown` — on x86, REAL: [`crate::arch::acpi_power::poweroff`], which does not return.
///
/// ⛔ The fixture must NEVER call this with [`Verb::ShutDown`], and does not: it drives picks only for
/// the safe verbs and proves Shut Down's routing through [`item_at`].
///
/// # PI-DESK — what Restart and Shut Down are on the Pi, and why they are NOT wired
///
/// The render and the hit-test cross to aarch64 whole; the ACTIONS do not, and the honest answer is
/// stated on the wire rather than faked. `acpi_power::poweroff` is an x86 path (ACPI S5 through the
/// PM1 control block) and has no aarch64 twin. The aarch64 twin *of the family* is PSCI —
/// `SYSTEM_OFF` (0x8400_0008) and `SYSTEM_RESET` (0x8400_0009) through an `smc` — and this kernel does
/// carry PSCI, but only in `arch/aarch64/smpprobe.rs`, only `CPU_ON`/`AFFINITY_INFO`/`FEATURES`, and
/// only on the **Tegra** path where an EL3 secure monitor answers. `SYSTEM_OFF` is queried by
/// `PSCI_FEATURES` there and, in that file's own words, *"never invoked"*.
///
/// The Pi 4 is the case that settles it: bare-metal BCM2711 boots to EL2 with **no EL3 firmware
/// behind it**, so an `smc` is not a power call — it is an unhandled exception. There is nothing to
/// route to. So both verbs print an `unimplemented:` line naming what is missing, exactly as `Sleep`
/// already did on x86, and the menu remains fully live: it opens, it hit-tests, it dismisses, and
/// `About` is REAL on both arches. Wiring these means a Pi power path (the watchdog/PM block for
/// reset, `PM_RSTS` for halt) — a driver arc, not a menu arc, and named as the follow-up rather than
/// smuggled in here.
fn fire(verb: Verb) {
    match verb {
        Verb::About => {
            // A minimal, honest About: the Shard identity and the kernel crate version, to the log.
            // A crispy About *panel* (a small centred surface with the crystal mark and these facts)
            // is the natural next step and is DESIGNED at the foot of this file; this arc prints it.
            serial_println!(
                ":: SHARD: about name=UnaOS shard=this version={} ::",
                env!("CARGO_PKG_VERSION")
            );
        }
        Verb::Sleep => {
            serial_println!(":: SHARD: unimplemented: Sleep (no ACPI S3 suspend path) ::");
        }
        Verb::Restart => {
            #[cfg(target_arch = "x86_64")]
            serial_println!(":: SHARD: unimplemented: Restart (no reboot path) ::");
            // PI-DESK — the same honest line, naming the aarch64 reason rather than the x86 one. See
            // this function's header: PSCI SYSTEM_RESET needs an EL3 monitor the Pi does not have.
            #[cfg(target_arch = "aarch64")]
            serial_println!(
                ":: SHARD: unimplemented: Restart (no PSCI SYSTEM_RESET — Pi 4 bare-metal runs at EL2 with no secure monitor; a BCM2711 watchdog reset is the wiring this needs) ::"
            );
        }
        Verb::ShutDown => {
            // Deliberate and destructive: the operator opened the menu and pressed Shut Down. Say so
            // once, then hand off to the real ACPI S5 soft-off, which either powers the machine off
            // mid-instruction or parks in `hlt` with its own witness. Never returns.
            #[cfg(target_arch = "x86_64")]
            {
                serial_println!(":: SHARD: shut down — entering ACPI S5 soft-off ::");
                crate::arch::acpi_power::poweroff();
            }
            // PI-DESK — NOT wired, and it returns. `smpprobe.rs` carries PSCI SYSTEM_OFF as a
            // FEATURES query it deliberately never invokes, and on the Pi there is no EL3 behind the
            // `smc` to answer it at all. A verb that cannot act says so and leaves the machine
            // running; it does not park the operator's desktop in a `wfi` to look decisive.
            #[cfg(target_arch = "aarch64")]
            serial_println!(
                ":: SHARD: unimplemented: Shut Down (no PSCI SYSTEM_OFF — Pi 4 bare-metal runs at EL2 with no secure monitor; PM_RSTS/watchdog halt is the wiring this needs) ::"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The click arm — reached from `wc_click_route_at`, ahead of the dock and window arms
// ---------------------------------------------------------------------------

/// **Route a PRESS at panel `(x, y)` to the crystal menu.** Returns `true` iff the press was CONSUMED
/// by the menu (opened it, dismissed it, or picked an item), in which case the caller drops the
/// matching release, exactly as the dock and close arms do.
///
/// The rule:
///  * **menu OPEN, press on an item** — pick it: fire the action and dismiss. Consumed.
///  * **menu OPEN, press inside on a separator/border/padding** — consumed, menu stays open.
///  * **menu OPEN, press outside** — dismiss (a first click outside a menu closes it). Consumed.
///  * **menu CLOSED, press in the corner cell** — open. Consumed. FITTS-CORNER: the cell is the
///    bar's whole upper-left corner ([`menubar::crystal_corner_abs`]), not just the painted glyph.
///  * **menu CLOSED, press elsewhere** — not ours; `false`, so the dock and window arms get their say.
///
/// Judged before the dock and window arms because an open dropdown is a modal surface composited on
/// top of everything; when closed, the only points it claims lie inside the bar's corner cell, which
/// the bar owns anyway (the bar composites above the windows).
pub fn press_at(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 {
        return false;
    }
    let (px, py) = (x as usize, y as usize);
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            return false;
        }
        (fb.width(), fb.height())
    };

    // ARC D M1 — every press that reaches this function is a press. The duplicate guard that used
    // to stand here (a release-generation comparison against the live hold) is gone: the input
    // producer no longer emits two edges for one click. See the section above the constants.
    if OPEN.load(Ordering::Acquire) {
        if let Some(r) = menu_rect(pw, ph) {
            if menu_contains(r, px, py) {
                return match item_at(r, px, py) {
                    Some(verb) => {
                        PICKS.fetch_add(1, Ordering::Relaxed);
                        LAST_VERB.store(verb.ord(), Ordering::Relaxed);
                        PRESS_OUTCOME.store(OUT_PICK, Ordering::Relaxed);
                        serial_println!(
                            ":: SHARD-MENU: crystal_pick verb={} action={} ::",
                            verb.name(),
                            if verb.real() { "real" } else { "stub" }
                        );
                        dismiss("pick");
                        fire(verb); // ShutDown does not return; every other verb does
                        true
                    }
                    // Inside the menu but not on an item (separator / border / padding): swallow the
                    // press and keep the menu open, as a real menu does.
                    None => {
                        PRESS_OUTCOME.store(OUT_KEPT, Ordering::Relaxed);
                        true
                    }
                };
            }
        }
        // A press anywhere outside the open menu dismisses it, and the click is spent doing so.
        PRESS_OUTCOME.store(OUT_DISMISS, Ordering::Relaxed);
        dismiss("outside");
        return true;
    }

    // Closed: the press cell we own is the bar's whole upper-left corner — FITTS-CORNER
    // ([`menubar::crystal_corner_abs`]): the crystal's full left slot (glyph plus both PAD margins)
    // by the bar's full height, anchored at the bar's origin so panel pixel (0,0) opens the menu
    // with zero aim. Every pixel of the cell is bar chrome composited above the windows, so this
    // claims nothing a window's own chrome could own; the DROPDOWN still anchors to
    // `crystal_box_abs`, the painted glyph, unchanged.
    if let Some((zx, zy, zw, zh)) = menubar::crystal_corner_abs(pw, ph) {
        if px >= zx && px < zx + zw && py >= zy && py < zy + zh {
            // The witness's `via=` word: on the painted glyph itself, or in the widened cell.
            let on_glyph = menubar::crystal_box_abs(pw, ph)
                .map(|(cx, cy, cw, ch)| px >= cx && px < cx + cw && py >= cy && py < cy + ch)
                .unwrap_or(false);
            PRESS_OUTCOME.store(OUT_OPEN, Ordering::Relaxed);
            open_via(pw, ph, if on_glyph { "crystal-glyph" } else { "corner-zone" });
            return true;
        }
    }
    false
}

/// **The Escape arm**, for `wc_route_event`: a bare `Esc` (0x1b) while the menu is open dismisses it
/// and is consumed. Returns `false` for every other event and for `Esc` when the menu is closed, so
/// Escape reaches an app normally when no menu is up.
///
/// Only the press edge is taken. The matching `KeyUp(0x1b)` — arriving with the menu already closed —
/// flows on untouched: no drain acts on a lone key release, so consuming it would buy nothing and
/// would need a scrap of state to remember the press. The Tab seam swallows its release because it
/// pairs with a focus move; a dismissal has no such pairing.
pub fn key_escape(ev: crate::pal::Event) -> bool {
    const K_ESC: u8 = 0x1b;
    match ev {
        crate::pal::Event::Key(K_ESC) if OPEN.load(Ordering::Acquire) => {
            dismiss("escape");
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// The composite seam — a transient surface through the strip painter
// ---------------------------------------------------------------------------

/// **MENU-OCC dismiss repaint — hand a just-vacated dropdown rect back to its OWNERS.**
///
/// The open menu is an [`wm::occ_clip`] citizen (§`MENU_OCC_MAX`): every window under it WITHHELD the
/// dropdown's rows while it was open, so [`strip::erase_rect`]'s `DESKTOP_BG` does not restore those
/// windows or the desktop content beneath — it stamps a HOLE over them, exactly the "menu-dismiss hole"
/// the SHELLDESK review recorded (a window would show desktop background until it next repainted on its
/// own). [`wm::damage_intersecting`] marks every window overlapping the vacated rect dirty and
/// [`super::screen::request_full_present`] asks the desktop layer to repaint its own rows — the same
/// `damage_intersecting` + full-present pair [`wm::reclaim`] gives a vacated WINDOW box, driven here by
/// the dismissal.
///
/// Runs from [`compose`] at the composite TAIL, where `composite_inner` has already returned and the
/// window-table lock is released, so the damage lock is taken cleanly. The residual is one frame of
/// `DESKTOP_BG` before the owners repaint — the bounded vacate residual the strips already carry, in
/// the safe direction (a hole that fills, never stale menu pixels that linger).
fn repaint_vacated(r: strip::Rect) {
    let (x, y, w, h) = r;
    if w == 0 || h == 0 {
        return;
    }
    wm::damage_intersecting(x, y, w, h);
    super::screen::request_full_present();
}

/// **Paint or erase the dropdown.** Called from [`super::strip::compose_all`] at the composite tail.
///
/// Returns `true` iff it painted or erased (the caller then owes the sprite a `Repaint`, as every
/// strip does). Damage-driven: an open menu whose rect is unchanged repaints nothing; a dismissal
/// erases the owned rect exactly once and clears the slot.
///
/// The disabled/closed path is the first line — one relaxed load and one packed check — so a boot
/// that never opens the menu pays two atomics per composite for its existence and touches no lock, no
/// framebuffer, and no allocation.
pub fn compose() -> bool {
    if !OPEN.load(Ordering::Relaxed) {
        // Closed. Owe the pixels of a menu just dismissed, once — and hand them back to their OWNERS.
        //
        // CRYSTAL-DISMISS (metal boot 8 review) — the ERASE lands before the slot is cleared, not
        // after. `strip::erase_rect` can DECLINE (a contended scratch under storm load, a surface
        // that is not ready), and clearing first turned that decline into a silent loss: the slot
        // read empty, [`paint_owed`] answered "nothing owed", and the dismissed dropdown's pixels
        // stood on the glass with no pass ever coming back for them — the state machine right and
        // the glass wrong, the mirror image of the boot-8 leak. Keeping the slot until the erase
        // has PAINTED keeps the debt visible: `paint_owed` stays true, and the gate holder's re-run
        // loop / the next composite retries the erase.
        if SLOT.packed() != 0 {
            let r = SLOT.rect();
            if !strip::erase_rect(r) {
                return false; // erase declined: still owed, still in the slot — the next pass retries
            }
            SLOT.clear();
            repaint_vacated(r);
            return true;
        }
        return false;
    }

    let t0 = crate::arch::now_cycles();
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
            return false;
        }
        (fb.width(), fb.height())
    };
    let rect = menu_rect(pw, ph);
    LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
    LEDGER.tick(
        "crystal",
        format_args!(
            "state=open opens={} dismisses={} picks={} clob={}",
            OPENS.load(Ordering::Relaxed),
            DISMISSES.load(Ordering::Relaxed),
            PICKS.load(Ordering::Relaxed),
            CLOBBERS.load(Ordering::Relaxed)
        ),
    );

    // The menu cannot be placed on this panel (too small) — erase anything we owed and stand down.
    // CRYSTAL-DISMISS — erase-then-clear, as on the dismissed path above and for the same reason: a
    // declined erase must stay owed, not be silently forgotten with its pixels still on the glass.
    let Some(r) = rect else {
        if SLOT.packed() != 0 {
            let old = SLOT.rect();
            if !strip::erase_rect(old) {
                return false;
            }
            SLOT.clear();
            repaint_vacated(old);
            return true;
        }
        return false;
    };

    // CLOBBER-REPAIR (PA41) — the dropdown's signature is a pure function of its RECT, so a window
    // that painted over the open menu changes nothing this test can see and the menu would stay
    // half-overwritten until it was dismissed. `wm::occ_clip` withholds the menu's columns from a
    // window blit on x86 and is `x86_64`-only, so on the Pi that protection does not exist at all and
    // this is the only thing standing between an open menu and a window's pixels. The dock's WCK5
    // condition, asked the dock's way: one bounded table scan, and only while the menu is OPEN — the
    // closed path returned above without reaching it.
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let (_, clobbered) = wm::dock_scan(&mut rows, SLOT.rect());
    if clobbered {
        CLOBBERS.fetch_add(1, Ordering::Relaxed);
    }
    let sig = strip::seal(strip::fnv1a_u64(strip::FNV_BASIS, strip::pack_rect(Some(r))));
    if sig == SLOT.sig() && SLOT.packed() == strip::pack_rect(Some(r)) && !clobbered {
        return false;
    }

    // Vacate the old rect first if the menu moved (a panel resize under an open menu), so the two
    // never race to own an overlapping pixel — the strip primitive's rule.
    let old = SLOT.packed();
    let new = strip::pack_rect(Some(r));
    if old != 0 && old != new {
        strip::erase_rect(strip::unpack_rect(old));
    }

    let t1 = crate::arch::now_cycles();
    if !strip::paint("crystal", r, |out, j| compose_row(out, r, j)) {
        return false;
    }
    LEDGER.paint(crate::arch::now_cycles().saturating_sub(t1), (r.2 * r.3) as u64);
    SLOT.store(sig, Some(r));
    true
}

/// Compose panel row `j` (0..MENU_H) of the dropdown into `out[0..MENU_W]` as logical colours.
///
/// A field pass — the menu face, the keyline border, and per-row content — then the item labels
/// overlaid by index. The dock's and bar's shape: an overlay keeps the inner loop a handful of
/// integer compares rather than a scan over every label at every pixel.
fn compose_row(out: &mut [u32], r: strip::Rect, j: usize) {
    let (_mx, _my, w, h) = r;
    // MENU-UNDER load-bearing palette fact (review, 2026-08-18): no colour this function emits may
    // be pure white 0x00FF_FFFF — that is the sprite's FILL, and `cursor::undraw_locked`'s colour
    // guard is what stops a stale save-under from stamping over a menu that opened mid-pass
    // (the open-direction residual the MENU-UNDER decline cannot see). CHROME_FACE, FRAME_LINE and
    // the blended label text all differ from FILL today; a future theme edit that puts 0xFFFFFF in
    // the dropdown (BEVEL_LIGHT / GLOSS_HIGHLIGHT are both pure white) re-opens a one-pixel stamp.

    // The whole-row base: keyline on the top and bottom border rows, menu face elsewhere.
    let base = if j < BORDER || j + 1 > h - BORDER {
        theme::FRAME_LINE
    } else {
        theme::CHROME_FACE
    };
    for i in 0..w {
        out[i] = base;
    }
    // The two side borders, on every row.
    for i in 0..w {
        if i < BORDER || i + BORDER >= w {
            out[i] = theme::FRAME_LINE;
        }
    }
    if j < BORDER || j + 1 > h - BORDER {
        return; // a pure border row — nothing else on it
    }

    let Some(row) = row_at(j) else {
        return;
    };
    let top = row_top(row);

    match ROWS[row].verb {
        // A separator: one keyline centred in the band, inset from the side borders by PADX.
        None => {
            if j == top + SEP_H / 2 {
                for i in (BORDER + PADX)..(w - BORDER - PADX) {
                    out[i] = theme::FRAME_LINE;
                }
            }
        }
        // An item: its label, vertically centred in the row, at PADX from the inner edge.
        Some(_) => {
            let vpad = (ITEM_H - CELL_H) / 2;
            let gtop = top + vpad;
            if j < gtop || j >= gtop + CELL_H {
                return;
            }
            let sy = j - gtop;
            // FONT (GR27) — the shared anti-aliased face, blended over the row fill the loop above
            // painted (RAM scratch — the blend's read is cached). Regular weight: menu items are
            // body text, not a caption.
            let label = ROWS[row].label.as_bytes();
            super::font::draw_row(out, w, label, BORDER + PADX, sy, theme::TITLE_TEXT_ACTIVE, false, FACE);
        }
    }
}

/// The crystal menu's ledger line, on the dock's/bar's terms plus this surface's own tail.
pub fn rollup(scope: &str) {
    LEDGER.rollup(
        "crystal",
        scope,
        format_args!(
            "opens={} dismisses={} picks={} clob={} last_verb={}",
            OPENS.load(Ordering::Relaxed),
            DISMISSES.load(Ordering::Relaxed),
            PICKS.load(Ordering::Relaxed),
            CLOBBERS.load(Ordering::Relaxed),
            LAST_VERB.load(Ordering::Relaxed)
        ),
    );
}

// ---------------------------------------------------------------------------
// Witness
// ---------------------------------------------------------------------------

/// CRYSTAL-MENU fixture — **the SHARD menu opens on the crystal, resolves every item, fires a pick,
/// dismisses three ways, and NEVER powers the machine off.**
///
/// Each leg can FAIL on its own, and the fixture restores the bar and the menu to their prior state
/// before it returns.
///
/// 1. **closed by default** — `OPEN` is observed `false` before the fixture touches it.
/// 2. **a crystal press OPENS** — with the bar enabled, a press at the crystal box centre opens the
///    menu and [`menu_rect`] is placeable. And FITTS-CORNER: a press at panel pixel `(0,0)` — the
///    zero-aim corner flick, a MISS before the corner cell existed — opens it too.
/// 3. **every item resolves to its verb** — [`item_at`] at each item row's centre answers About,
///    Sleep, Restart, Shut Down in tree order. This is the leg that proves Shut Down is REACHABLE as
///    a pick, and it does so WITHOUT firing it — the whole guard.
/// 4. **a SAFE pick fires and dismisses** — a press on the About row increments `picks`, prints the
///    identity, and leaves the menu closed. Sleep and Restart are driven too, so the wire carries
///    their honest `unimplemented` lines. Shut Down is NEVER driven.
/// 5. **click-outside dismisses** — reopened, a press well outside the menu closes it and is consumed.
/// 6. **Escape dismisses** — reopened, [`key_escape`] on `Esc` closes it and is consumed.
///
/// ⛔ **The Shut Down guard is structural, not a comment.** This function calls [`item_at`] at the
/// Shut Down row (a pure resolver) but never [`press_at`] there, and [`fire`] reaches
/// `acpi_power::poweroff` only from the live press. A gate that ran this fixture and lost the serial
/// tail would be the signature of that guard having failed — so the PASS line printing AFTER all legs
/// is itself the proof the machine stayed up.
#[cfg(feature = "witness")]
pub fn selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        (fb.width(), fb.height())
    };
    let saved_bar = menubar::enabled();

    // Leg 1 — closed by default, observed before any mutation.
    let start_closed = !OPEN.load(Ordering::Acquire);

    // The menu only exists when the bar is present; enable it for the battery.
    menubar::set_enabled(true);
    let cbox = menubar::crystal_box_abs(pw, ph);

    // Leg 2 — a crystal press opens.
    let opened = match cbox {
        Some((cx, cy, cw, ch)) => {
            let hit = press_at((cx + cw / 2) as i32, (cy + ch / 2) as i32);
            hit && OPEN.load(Ordering::Acquire)
        }
        None => false,
    };
    // Leg 2b — FITTS-CORNER: panel pixel (0,0), the zero-aim flick target, opens too. This is the
    // corner principle asserted at its extreme point, not a sample inside the glyph: (0,0) was a MISS
    // before the corner cell existed. Dismiss first so the press is judged by the CLOSED arm; the
    // reopened menu is exactly the state leg 3 wants.
    dismiss("corner-leg");
    let corner_ok = press_at(0, 0) && OPEN.load(Ordering::Acquire);

    let mr = menu_rect(pw, ph);
    let placed = mr.is_some();

    // Leg 3 — every item resolves to its verb (Shut Down included, via the PURE resolver only).
    let mut resolve_ok = placed;
    if let Some(r) = mr {
        for row in 0..ROWS.len() {
            if let Some(want) = ROWS[row].verb {
                let cy = r.1 + row_top(row) + ITEM_H / 2;
                let cx = r.0 + r.2 / 2;
                if item_at(r, cx, cy) != Some(want) {
                    resolve_ok = false;
                }
            }
        }
    }

    // Leg 4 — a SAFE pick fires and dismisses. About first (real), then Sleep and Restart (stubs) so
    // the wire carries their honest lines. Shut Down is NEVER driven.
    let picks_before = PICKS.load(Ordering::Relaxed);
    let pick_safe = |verb: Verb| -> bool {
        // Reopen and press the row that carries `verb`.
        open(pw, ph);
        let Some(r) = menu_rect(pw, ph) else { return false };
        let mut ok = false;
        for row in 0..ROWS.len() {
            if ROWS[row].verb == Some(verb) {
                let cy = r.1 + row_top(row) + ITEM_H / 2;
                let cx = r.0 + r.2 / 2;
                let consumed = press_at(cx as i32, cy as i32);
                ok = consumed && !OPEN.load(Ordering::Acquire);
            }
        }
        ok
    };
    let about_fired = pick_safe(Verb::About);
    let sleep_fired = pick_safe(Verb::Sleep);
    let restart_fired = pick_safe(Verb::Restart);
    let picks_counted = PICKS.load(Ordering::Relaxed) == picks_before + 3;
    let pick_ok = about_fired && sleep_fired && restart_fired && picks_counted;

    // Leg 5 — click-outside dismisses.
    open(pw, ph);
    // A point guaranteed outside the menu: the panel's far bottom-right corner.
    let outside_consumed = press_at((pw - 1) as i32, (ph - 1) as i32);
    let outside_ok = outside_consumed && !OPEN.load(Ordering::Acquire);

    // Leg 6 — Escape dismisses.
    open(pw, ph);
    let esc_consumed = key_escape(crate::pal::Event::Key(0x1b));
    let esc_ok = esc_consumed && !OPEN.load(Ordering::Acquire);

    // Leg 7 — MENU-OCC: the open dropdown is a first-class OCCLUDER. A window whose blit crosses it
    // must have the menu's columns WITHHELD, or the dropdown is overwritten mid-frame (Boot C,
    // operator: "menubar menu gets overwritten"). The occlusion is the present's own arithmetic —
    // [`wm::occ_menu_probe`] delegates to the proven [`wm::occ_bar_probe`], run against a synthetic
    // window that crosses the OPEN menu: the PROTECTED walk withholds the menu's columns (`px_prot>0`)
    // and the FAULT walk (clip empty) collapses to `px_fault==0`, so the leg is falsifiable rather
    // than trusted. Meaningful only on x86 + `wc`, where [`wm::occ_clip`] pushes the menu; off that
    // arch there is no window-blit occlusion path to protect and the leg trivially holds.
    open(pw, ph);
    let menu_occ_ok;
    #[cfg(target_arch = "x86_64")]
    {
        menu_occ_ok = match menu_rect(pw, ph) {
            Some((mx, my, mw, mh)) => {
                // A window box crossing the whole dropdown: its columns, spanning past it top and bottom.
                let win = (mx, my.saturating_sub(20), mw, mh + 40);
                let p = wm::occ_menu_probe((mx, my, mw, mh), win);
                let leg = p.pop_prot > 0 && p.px_prot > 0 && p.px_fault == 0;
                serial_println!(
                    ":: MENU-OCC: menu={}x{}+{}+{} win={}x{}+{}+{} occ={} occ_px={} fault_px={} :: {} ::",
                    mw, mh, mx, my, win.2, win.3, win.0, win.1,
                    p.pop_prot, p.px_prot, p.px_fault,
                    if leg { "PASS" } else { "FAIL" }
                );
                leg
            }
            None => false,
        };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        menu_occ_ok = true;
    }
    dismiss("menu-occ");

    // Restore: ensure closed, and put the bar back the way we found it.
    dismiss("selftest");
    menubar::set_enabled(saved_bar);

    let (rw, rh, rx, ry) = mr.map(|(x, y, w, h)| (w, h, x, y)).unwrap_or((0, 0, 0, 0));
    let ok = start_closed
        && opened
        && corner_ok
        && placed
        && resolve_ok
        && pick_ok
        && outside_ok
        && esc_ok
        && menu_occ_ok;
    serial_println!(
        ":: CRYSTAL-MENU: menu={}x{}+{}+{} panel={}x{} items={} start_closed={} opened={} \
         corner={} placed={} resolve={} pick={} outside={} escape={} menu_occ={} :: {} ::",
        rw, rh, rx, ry, pw, ph, ITEM_COUNT,
        start_closed, opened, corner_ok, placed, resolve_ok, pick_ok, outside_ok, esc_ok, menu_occ_ok,
        if ok { "PASS" } else { "FAIL" }
    );
    rollup("selftest");
}

/// SHARD-PRESS fixture (PA41) — **a press on the crystal, through the LIVE furniture router, puts the
/// dropdown ON THE GLASS; the next press takes it off again.**
///
/// # Why this exists, and why [`selftest`] could not answer it
///
/// [`selftest`] calls [`press_at`] DIRECTLY, so it proves the hit-test and the modal state machine and
/// nothing about either seam that matters on metal: not the router arm that reaches the menu, and not
/// the paint. On the Pi, PA41's operator pressed the crystal and saw nothing happen, and the only
/// witness terms available said `crystal_press=open` (the state DID change) beside a `[menubar]` line
/// whose `press=` word was a stale hardcode. Both halves of that gap are closed here:
///
/// 1. **the press is ROUTED** — driven through [`strip::press_route`], the ONE shared furniture router
///    both `arch/aarch64/syscall.rs::wc_click_route` and x86's `wc_click_route_at` call ahead of every
///    window arm. What is not covered is per-arch and named rather than implied: the button-mask edge
///    detection, the press-target latch and the input rings all sit ABOVE this seam.
/// 2. **and the menu is PAINTED** — `SLOT` non-empty, i.e. [`compose`] actually ran and landed pixels.
///    This is the leg that reds without MENU-DRIVE: before [`open`] drove its own composite, the state
///    flipped and the slot stayed empty, because on a quiet desktop no other pass was coming.
/// 3. **a press outside DISMISSES, and the ERASE lands** — the mirrored claim, `SLOT` back to empty.
///    Leg 3 is also what keeps the fixture side-effect-free: while the menu is open the crystal
///    consumes EVERY point, so the press that would otherwise reach the dock or a window is spent on
///    the dismissal. It is skipped entirely if leg 1 did not open, so a declined crystal can never
///    turn this fixture into an unsolicited dock press.
///
/// The bar is restored to whatever state it arrived in and the menu is left closed.
#[cfg(feature = "witness")]
pub fn routed_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        (fb.width(), fb.height())
    };
    let saved_bar = menubar::enabled();
    dismiss("routed-selftest"); // idempotent: a menu left open by anything above must not skew leg 1
    menubar::set_enabled(true);

    // Legs 1-2 — the crystal, through the router, and the paint that press owes.
    let (routed, open_word, opened, painted) = match menubar::crystal_box_abs(pw, ph) {
        Some((cx, cy, cw, ch)) => {
            let hit = strip::press_route((cx + cw / 2) as i32, (cy + ch / 2) as i32);
            (hit, last_press_outcome(), OPEN.load(Ordering::Acquire), SLOT.packed() != 0)
        }
        None => (false, "no-crystal", false, false),
    };
    let (mw, mh, mx, my) = menu_rect(pw, ph).map(|(x, y, w, h)| (w, h, x, y)).unwrap_or((0, 0, 0, 0));

    // Leg 3 — a press outside the OPEN menu dismisses it, and the erase lands. Only reachable when
    // leg 1 opened; otherwise this point belongs to the dock and the fixture declines to press it.
    let (dismissed_hit, dismiss_word, closed, erased) = if opened {
        let hit = strip::press_route((pw - 1) as i32, (ph - 1) as i32);
        (hit, last_press_outcome(), !OPEN.load(Ordering::Acquire), SLOT.packed() == 0)
    } else {
        (false, "not-open", false, false)
    };

    dismiss("routed-selftest");
    menubar::set_enabled(saved_bar);

    let ok = routed
        && open_word == "open"
        && opened
        && painted
        && dismissed_hit
        && dismiss_word == "dismiss"
        && closed
        && erased;
    serial_println!(
        ":: SHARD-PRESS: menu={}x{}+{}+{} panel={}x{} routed={}({}) opened={} painted={} \
         dismissed={}({}) closed={} erased={} :: {} ::",
        mw, mh, mx, my, pw, ph,
        routed, open_word, opened, painted,
        dismissed_hit, dismiss_word, closed, erased,
        if ok { "PASS" } else { "FAIL" }
    );
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// DESIGNED, NOT BUILT — the rest of the SHARD menu, recorded beside the first cut.
//
// ── A HOVER HIGHLIGHT ────────────────────────────────────────────────────────────────────────────
// This arc's menu fires on the PRESS edge and draws no hovered row: reaching a hover highlight means
// tracking pointer MOTION over the open menu, which is the drag-motion path (`wc_route_tail` ->
// `wc_drag_motion`) — a lane this arc does not touch. The next arc adds a `HOVER: AtomicU8` (the
// highlighted row index, or NONE), folds it into the damage signature so only the two changed rows
// repaint, and paints the hovered ITEM row in `theme::ACCENT` with `theme::BEVEL_LIGHT` ink. The
// press then fires the hovered row rather than the row under the cursor, matching press-drag-release
// menu behaviour.
//
// ── THE ABOUT PANEL ──────────────────────────────────────────────────────────────────────────────
// About prints the identity this arc. The crispy About PANEL is a second transient surface, minted the
// same way this dropdown is (a `strip::paint` rect, its own `Slot`, its own dismiss-on-click), centred
// on the panel: the brand crystal drawn large (reuse `menubar`'s `crystal_facet`), the name "UnaOS",
// the Shard version, and the build. It is a SECOND modal, so it wants the same press-outside/Escape
// dismissal this menu has — which is the argument for factoring open/dismiss/compose into a tiny modal
// surface primitive that both the menu and the About panel are tenants of, rather than copying them.
//
// ── SLEEP AND RESTART, MADE REAL ─────────────────────────────────────────────────────────────────
// Both are honest stubs today because neither op exists. RESTART is the nearer one: a warm reboot via
// the 8042 pulse (`0xFE` -> port `0x64`) or the PCI reset control register (`0xCF9`), each a real,
// small, discoverable action in the `acpi_power` idiom — refuse rather than guess, one witness line,
// fall back to a triple-fault only as a last resort. SLEEP is ACPI S3 (suspend-to-RAM), which needs
// the DSDT `\_S3_` package read the exact way `scan_s5` reads `\_S5_`, plus a resume vector and a
// register save/restore the kernel has no machinery for yet — a genuine subsystem, correctly a stub
// until that machinery exists. When each lands, its `Verb::_::real()` flips to `true` and its `fire`
// arm calls the op; the menu, the routing, and this fixture are unchanged.
// ═════════════════════════════════════════════════════════════════════════════════════════════════
