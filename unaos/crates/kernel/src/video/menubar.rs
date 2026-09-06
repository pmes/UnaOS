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

//! MENUBAR — the top strip. **Tenant #2 of [`super::strip`], and DEFAULT OFF.**
//!
//! # What this module is, and the direction that shaped it
//!
//! Peter's first direction was verbatim *"can't mimic macOS without that"*. His second, the same
//! week, is the one this file is built to: **UnaOS is a spatial game-engine OS**, the desktop is one
//! shell running on it, and *"we will not always have a menu bar"*.
//!
//! Both are honoured by making the bar a **tenant**, not a feature:
//!
//!  * every mechanism it uses — geometry with floors, the staged row-run painter, the damage slot,
//!    occlusion citizenship, the cost ledger — lives in [`super::strip`] and is shared with
//!    [`super::dock`]. This module contributes a rect, a row composer, and a model. Nothing else.
//!  * it is **absent by default**. [`ENABLED`] starts `false`, [`strip_rect`] answers `None`, and
//!    [`compose`] returns on three relaxed atomics (the enable load, the off-pass counter, the
//!    owed-pixels check) before reading a pixel, a lock, or the clock. A shell that wants it turns it
//!    on; a game shell never does and pays those three relaxed atomics per composite for its
//!    existence — no lock, no framebuffer touch, no allocation.
//!  * **deleting this file is a supported operation.** It costs one registry entry in
//!    `strip::TENANTS`, one line in `strip::compose_all`, and one `mod` declaration. The primitive
//!    does not know what a menu is, and `wm` knows only that some tenant occupies some rect.
//!
//! # What it draws, this arc
//!
//! Inert chrome, a brand mark, and two facts:
//!
//! | element | source | colour |
//! |---|---|---|
//! | strip face | — | [`theme::CHROME_FACE`] under [`ceramic::shade`] |
//! | bottom keyline | — | [`theme::FRAME_LINE`], 1 px |
//! | top bevel | — | [`theme::BEVEL_LIGHT`], [`theme::BEVEL`] px |
//! | CRYSTAL, leftmost | UnaOS's brand mark (Peter, *"instead of an apple do a small crystal"*) | the kit's blue gem ramp, [`theme::CONTROL_CLOSE`]/`_MID`/`_ZOOM` |
//! | title, left of crystal | the FOCUSED window's caption, via `wm::dock_scan` | [`theme::TITLE_TEXT_ACTIVE`] |
//! | clock, right | [`crate::clock::try_unix_now`], UTC `HH:MM` | [`theme::TITLE_TEXT_INACTIVE`] |
//!
//! **The bar's one press target is the CRYSTAL.** This module still registers nothing with the click
//! router itself; the SHARD menu ([`super::crystal`]) claims the crystal's corner cell
//! ([`crystal_corner_abs`] — FITTS-CORNER, the bar's whole upper-left corner, not just the glyph)
//! through the ONE shared furniture router [`strip::press_route`], which both arch routers call
//! ahead of every window arm.
//! Every other point on the bar falls through to whatever is behind it. The witness line says so
//! (`press=crystal`). It read `press=inert` from the arc when the bar had no press seam at all, and
//! that stale word survived onto the Pi, where PA41's operator read it off a metal capture as "press
//! routing latched off" — during an investigation whose real defect was that nothing COMPOSITED the
//! menu the press had already opened. A witness term must track the code it describes. Opening APP
//! menus is still the protocol arc's job; the design ledger at the foot of this file is what it will
//! be built from.
//!
//! **CLOBBER-REPAIR (PA41).** The bar asks [`wm::dock_scan`] a second question beside the model — *did
//! a window the compositor just painted intersect the rows I last painted?* — and repaints when the
//! answer is yes even though its signature is unchanged. That is [`super::dock`]'s WCK5 condition,
//! generalised to tenant #2, and it is a BELT on both arches. `clob=` on the ledger line is the
//! falsifier, and `CLOBBERS` is ungated, so that field is on the wire from a Pi/Orin capture too.
//!
//! ⚠ **The braces-and-belt claim that stood here was WRONG, and correcting it is why this paragraph
//! is long.** It read: *"on aarch64 it is not a belt: `wm::occ_clip` is `x86_64`-only, so a window
//! blit crossing the top strip is NOT withheld there"*. Neither half survives reading the code it
//! cites. `wm::occ_clip` carries NO arch gate at all (`video/wm.rs`, the `fn occ_clip` definition —
//! `#[allow(unused_variables, unused_mut)]` and nothing else), and its FURNITURE arm — the one that
//! pushes THIS bar's rect into every window's clip — rides
//! `any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "pidesk"))`,
//! which is the same dual gate this module's own declaration rides in `video/mod.rs`. So on a
//! `pidesk` aarch64 boot the bar IS in the clip and a crossing blit's columns ARE withheld, on the
//! identical `boxes_overlap` + `OccClip::push` + `span_occ` path x86 takes. `wm.rs`'s own ledger says
//! so twice — *"`occ_clip`'s WINDOW half-space runs on aarch64"* — and the arm's comment names the
//! Pi explicitly.
//!
//! **What IS x86-only is the PROOF, not the protection**, and that distinction is the entire content
//! of the correction: `OB_BOX`/`OB_N` — the statics behind `occclip_bar=`/`occclip_bar_px=` on
//! `[drag-occ]` — are `all(feature = "witness", target_arch = "x86_64")`. A Pi/Orin capture's silence
//! in those fields is therefore an ABSENT INSTRUMENT, never a zero, and an aarch64 regression in this
//! clip would read GREEN because there is no field left to fall. That is what the
//! `:: MENUBAR-OCC-PAR:` line exists to say out loud; the leg is [`occpar_once`], on the COMPOSE path.
//!
//! **PAR-MENUBAR (rmbp arc 7) corrects two things this paragraph itself got wrong.** It said the
//! `MENUBAR-OCC` probe at the foot of [`selftest`] is x86-gated — it was, by a bare
//! `#[cfg(target_arch = "x86_64")]` that has since been replaced by the [`occ_bar_reading`] dispatch;
//! what keeps that leg x86 now is REACHABILITY (`selftest` has no aarch64 caller), which is a
//! different claim with a different fix. And it pointed at [`selftest`] for the parity leg, which
//! never lived there — the whole reason ORIN-VPAR put it on the compose path is that `selftest`
//! cannot run on the Pi. The `orinvpar` knob is no longer part of it either: gating the aarch64 half
//! of a proof behind a DEFAULT-OFF knob left the green-reading regression intact on every default
//! boot, so the leg rides `witness` alone now, exactly as its x86 counterpart does.
//!
//! The stale sentence is recorded rather than silently deleted because of what it would have COST:
//! a reader taking it at face value would have concluded the aarch64 blit path has no clip term and
//! gated their own work off that — manufacturing the very drift the citation described. A drifted
//! citation is worse than the drift, because it is read as current.
//!
//! # The crystal — UnaOS's mark, where macOS puts its apple
//!
//! A small faceted gem at the left edge, `CRYSTAL_W`x`CRYSTAL_H` (16x22), sized from
//! [`theme::CONTROL_BOX`] so it reads as the same size-family as the window's traffic-light controls.
//! It is drawn from the kit's OWN blue accent ramp — three facets lit from the top-right, the
//! high-contrast seam down the crown reading as a facet edge — reusing three lifted roles `theme.rs`
//! records as having no consumer since the controls went semantic. No palette is invented. It appears
//! only when the bar is enabled (the bar is a default-off tenant; the crystal is part of it), and its
//! geometry is on the witness (`crystal=WxH+X+Y`) so a capture can confirm the mark is DRAWN.
//!
//! # Density, not decoration — why the bar earns its 34 rows
//!
//! The taste law for this surface is DENSITY: one strip serving every client beats per-window chrome.
//! So the bar is **flush** — corner to corner at `y = 0`, no margin, no centring, no rounded slab
//! floating in a gutter. It is [`theme::TITLE_HEIGHT`] tall, the same 34 px a window's own title bar
//! is, because it does the same job for the focused window and a second height would be a second
//! number for one idea. Nothing is padded to look roomy; the two insets are one [`strip::PAD`] each,
//! the kit's standard gap.
//!
//! # The clock is HONEST, or it is absent
//!
//! `clock::try_unix_now()` returns `None` until the civil clock has been anchored this boot (an SNTP sync
//! or an operator `setdate`). A bar that showed `00:00` or `--:--` for that state would be furniture
//! asserting a fact it does not have. So the clock is simply **not drawn** while unsynced, the title
//! keeps the width, and the witness says `clock=unsynced`. It is **UTC**: the kernel carries no
//! timezone, and rendering local time would require inventing one.
//!
//! # PI-DESK — compiled on the Pi, and STILL DEFAULT OFF there
//!
//! The `mod` gate is now `any(all(x86_64, wc), all(aarch64, desktop_firmware))`, so this tenant exists on the
//! BCM2711 panel too. Nothing about its POLICY moved with it. Peter's direction — *"we will not
//! always have a menu bar"* — is a runtime statement, and `ENABLED` still starts `false` on both
//! arches: a Pi boot with `UNAOS_PIDESK=1` gets the dock strip and the SHARD menu machinery and NO
//! top strip until something asks for one. Compiling a tenant is not enabling it, and this is the
//! tenant that exists to keep that distinction visible.

use super::{ceramic, strip, theme, wm};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// The bar's height — [`theme::TITLE_HEIGHT`]. The same strip a window head is, for the same job.
const BAR_H: usize = theme::TITLE_HEIGHT;

/// The glyph advance and cell height text is drawn at — [`wm::TITLE_CELL_W`]/[`wm::TITLE_CELL_H`],
/// the same face metrics the window caption and the dock tile resolve to. One definition, so a
/// face change moves all three together. FONT (GR27): the cell stopped being square when the
/// 1-bit bitmap gave way to the shared anti-aliased face, so the two axes are named separately.
const CELL_W: usize = wm::TITLE_CELL_W;
const CELL_H: usize = wm::TITLE_CELL_H;

/// FONT-METRIC — the atlas those metrics come from. Named once so the cell constants above and the
/// glyph calls below can never disagree about which face the bar is drawing.
const FACE: super::font::Face = super::font::Face::Chrome;

/// The clock's rendered width in glyphs: `HH:MM`.
const CLOCK_GLYPHS: usize = 5;

/// The longest title the bar will show, in glyphs — the whole stored caption. Unlike the dock (which
/// caps at 8 because it draws many tiles across one strip), the bar draws exactly ONE caption and has
/// the panel's full width for it, so there is no reason to truncate below what `wm` stores.
const TITLE_GLYPHS: usize = wm::MAX_TITLE;

// ---------------------------------------------------------------------------
// The brand CRYSTAL — the leftmost mark, where macOS puts its logo.
//
// Peter's ruling, 2026-08-11: *"instead of an apple do a small crystal"*. UnaOS's identity is
// crystal: the whole handler set is crystal-named (geode, obsidian, quartzite, euclase, zircon,
// mica), so the brand mark is a faceted gem, not a fruit.
//
// It is drawn from the kit's OWN blue gem ramp — `theme::CONTROL_CLOSE`/`_MID`/`_ZOOM`, the accent
// hue from darkest to lightest — which `theme.rs` records as having NO consumer since `paint_window`
// moved to the semantic traffic-light set. The crystal gives those three lifted roles a purpose
// again; no palette is invented (the shared-source law). Its size is `theme::CONTROL_BOX`-derived so
// the mark reads as the same size-family as the window controls.
// ---------------------------------------------------------------------------

/// The crystal's height, px — a hair inside the control disc's `theme::CONTROL_BOX` (24) footprint,
/// so the brand mark and the window's traffic-light controls read as one size family. `-2` gives the
/// gem a touch more clearance in the bar than the disc has in a title bar (the disc's is the "tight"
/// 5 px `theme.rs` flags; the crystal's is 6).
const CRYSTAL_H: usize = theme::CONTROL_BOX - 2;

/// The crystal's width, px — two-thirds of the control-disc diameter. A gem reads TALLER than wide, so
/// the mark is narrower than the round controls at the same height family; `2/3` of 24 is 16.
const CRYSTAL_W: usize = theme::CONTROL_BOX * 2 / 3;

/// The crown's height, px — the upper region above the girdle, drawn as the two bright table facets;
/// the pavilion (the rest) tapers to the point. Two-fifths, the classic brilliant-cut proportion.
const CRYSTAL_CROWN_H: usize = CRYSTAL_H * 2 / 5;

/// The title's left inset when the crystal is present: past the crystal and one more gap. macOS puts
/// the logo leftmost and the app menus to its right; the caption takes that same slot here.
const TITLE_X0: usize = strip::PAD + CRYSTAL_W + strip::PAD;

/// WINMENU (R21) — **the caption's SLOT, which is fixed-width, and where the window's menus begin.**
///
/// The caption is drawn at [`TITLE_X0`] and is between 0 and [`wm::MAX_TITLE`] glyphs long. If the
/// menu titles began after the caption's RENDERED width they would slide left and right every time
/// the focused window changed — and a press would then be judged against a layout the operator was
/// not looking at when they aimed. So the caption gets a slot of its full stored width plus one
/// glyph of gap, and the menus start at a constant offset whatever is in it. macOS's bar has the same
/// property for the same reason (its app name is bold and its menus do not reflow under it); the
/// difference is only that this kernel's caption is bounded, so the slot can be a `const`.
const MENUS_X0: usize = TITLE_X0 + (wm::MAX_TITLE + 1) * CELL_W;

/// The panel height below which the bar declines.
///
/// DERIVED, and stated: the bar must not crowd the dock off the panel. The dock's own floor is
/// `STRIP_H + 2*PAD` (its [`super::dock::STRIP_H`] plus a margin above and below), and the bar takes
/// `BAR_H` off the top before the dock ever gets to lay out. So a panel that can host both is at
/// least their sum, and one that cannot host both hosts the DOCK — the dock is the console's only way
/// back and the bar is a convenience, so the bar is the one that yields.
const FLOOR_H: usize = BAR_H + super::dock::STRIP_H + 2 * strip::PAD;

/// The panel width below which the bar declines: both insets, the clock, and at least one glyph of
/// title. Below it the bar would be a strip with nothing legible on it.
///
/// The `+ CRYSTAL_W` term is the brand mark's own left slot: the crystal pushes the title inset right,
/// so the floor that guarantees "crystal, one title glyph, and the clock all fit" grows by exactly the
/// crystal's width. Still far below every suite panel (160 vs 640/1280/1920), so no gate declines.
const FLOOR_W: usize = 4 * strip::PAD + CRYSTAL_W + (CLOCK_GLYPHS + 1) * CELL_W;

const _: () = {
    // The caption must fit inside the bar it is centred in, or there is nothing to draw.
    assert!(CELL_H <= BAR_H);
    // The bevel is drawn under the bar's top edge and must not reach its keyline.
    assert!(theme::BEVEL < BAR_H);
    // A bar that declines on every panel this kernel drives would be an inert file pretending to be a
    // feature. QEMU's raspi4b 640x480 is the smallest panel in the suites; the floors must admit it.
    assert!(FLOOR_W <= 640);
    assert!(FLOOR_H <= 480);
    // The title must be representable in what `wm` actually stores.
    assert!(TITLE_GLYPHS <= wm::MAX_TITLE);

    // The crystal must fit inside the bar with a bevel of clearance each side — the disc's own floor.
    assert!(CRYSTAL_H + 2 * theme::BEVEL <= BAR_H);
    // A gem needs a non-degenerate silhouette: a crown above the girdle and a pavilion below it, both
    // with real height, and a width the facet split can halve.
    assert!(CRYSTAL_CROWN_H > 0);
    assert!(CRYSTAL_CROWN_H < CRYSTAL_H);
    assert!(CRYSTAL_W >= 4);
    // The crystal and the clock must not collide on the SMALLEST panel the bar draws on. The crystal
    // occupies `[PAD, PAD + CRYSTAL_W)` and the clock `[FLOOR_W - PAD - CLOCK_GLYPHS*CELL_W, FLOOR_W - PAD)`;
    // this is the gap between them staying positive, so a future metric change that would overlap them
    // fails the BUILD rather than painting the gem over the time.
    assert!(strip::PAD + CRYSTAL_W < FLOOR_W - strip::PAD - CLOCK_GLYPHS * CELL_W);
    // The title, shifted past the crystal, must still leave room for at least one glyph before the
    // clock on the floor panel.
    assert!(TITLE_X0 + CELL_W <= FLOOR_W - strip::PAD - CLOCK_GLYPHS * CELL_W);
    // FITTS-CORNER: the press cell (`crystal_corner_abs`, TITLE_X0 wide by the bar's height) must
    // CONTAIN the painted glyph box, or a press on the visible mark could miss its own menu. The
    // horizontal half is the load-bearing one; vertical containment is the bevel assert above.
    assert!(strip::PAD + CRYSTAL_W <= TITLE_X0);
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// **The bar is off until something turns it on.** The whole of "absent by default".
///
/// Reading this flag is the FIRST thing an absent bar costs per composite pass — [`compose`]'s first
/// line and [`strip_rect`]'s. The full off-path is three relaxed atomics (this load, the off-pass
/// counter, the owed-pixels check); stated as three rather than dressed up as one, because a claim of
/// "one atomic" would be as false as a claim of zero, and the number that matters is that nothing
/// with a lock, a framebuffer touch, or an allocation runs.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// What the bar last put on the panel.
static SLOT: strip::Slot = strip::Slot::new();

/// The bar's cost ledger — see [`strip::Ledger`]. Not `witness`-gated: the metal image is built
/// without `witness` and a cost claim absent from it is not a claim.
static LEDGER: strip::Ledger = strip::Ledger::new();

/// How many times [`set_enabled`] actually changed the state, and how many composites ran while the
/// bar was off. The second is the falsifiable half of "absent costs nothing": a boot whose bar is
/// never enabled prints `off_passes` equal to the composite count and `paints=0`.
static TOGGLES: AtomicU64 = AtomicU64::new(0);
static OFF_PASSES: AtomicU64 = AtomicU64::new(0);

/// CLOBBER-REPAIR (PA41) — **passes in which a window the compositor painted had intersected the rows
/// the bar last painted.** [`super::dock`]'s WCK5 counter, generalised to tenant #2.
///
/// It is a SEPARATE reading from `paints`, which conflates the two damage conditions: a MODEL change
/// (the focused window's caption or the clock changed — a repaint the bar owes and always will) and a
/// CLOBBER (a window blit published over the strip and destroyed it). A boot with `clob=0` never met
/// the condition; a boot with `clob>0` is one where, before this arc, the bar was left standing with
/// a window's pixels in it and NOTHING in its own damage test able to see them — the bar's signature
/// is a function of the model and the rect, and a clobber changes neither.
static CLOBBERS: AtomicU64 = AtomicU64::new(0);

// ═══ ORIN-VPAR / MENUBAR-OCC-PAR — THE aarch64 HALF OF THIS BAR'S OCCLUSION PROOF ════════════════
//
// THE GAP, stated exactly, because it is a PROOF gap and not a PROTECTION gap and the two are
// constantly confused (this module's own header asserted the wrong one until this arc; see the
// correction there). `wm::occ_clip`'s furniture arm — the one that pushes THIS bar's rect into every
// window's clip — rides `any(all(x86_64, wc), all(aarch64, pidesk))`, which is this module's own
// declaration gate. So on a `pidesk` aarch64 boot the bar IS in the clip and a crossing blit's
// columns ARE withheld, by the same `boxes_overlap` + `OccClip::push` + `span_occ` path x86 takes.
//
// What does NOT exist on this arch is every way of SEEING that: `OB_N`/`OB_BOX` — the statics behind
// `occclip_bar=`/`occclip_bar_px=` — and `wm::occ_bar_probe` are `all(witness, x86_64)`. An aarch64
// regression in that clip therefore reads GREEN, because there is no field left to fall.
//
// ⚠ **WHY THIS DECLINES INSTEAD OF PORTING.** Closing it properly means widening ONE gate in
// `video/wm.rs` (`occ_bar_probe`'s, and with it `OB_N`/`OB_BOX` and the `[drag-occ]` fields that read
// them). `wm.rs` is another seat's lane on this arc, so this code does not reach for it — and it does
// NOT re-derive the span walk locally either: `OccClip::push`/`prepare`, `OccRows::spans` and
// `span_occ` are private to `wm`, and a second local definition of "occluded" is exactly the drift
// `wm::occ_menu_probe` was written to delegate AWAY from. It would yield a number that agrees with
// the present only by coincidence. A wrong port costs a flight; a named decline costs a line.
//
// ⚠ **WHY IT LIVES ON THE COMPOSE PATH AND NOT IN [`selftest`].** [`selftest`] is UNREACHABLE on
// aarch64: its only caller chain is `arch/x86_64/syscall.rs` → `dock::selftest` → `menubar::selftest`,
// so an aarch64 image contains no `:: MENUBAR:` string at all (checked against the built artifact,
// not inferred). `video/pidesk.rs` — which runs `crystal::routed_selftest` for exactly this reason,
// and whose comment records that this whole fixture family is x86-invoked — is another seat's file on
// this arc, so the reachable in-lane site is [`compose`], which `strip::compose_all` drives every
// pass. Being on the compose path also makes this STRICTLY better than a fixture would have been: it
// touches [`ENABLED`] not at all, where the x86 `MENUBAR-OCC` leg must toggle and restore it.
//
// BOTH OUTCOMES REACH THE WIRE, which is the point: [`occpar_once`] answers when the bar is live, and
// [`occpar_never_enabled`] answers when it never turns on — a bar that is off all boot would
// otherwise leave the question unasked, and an unasked question looks exactly like a passing one.
//
// ═══ PAR-MENUBAR (rmbp arc 7) — THE LEG ABOVE WAS ITSELF ARCH-DRIFTED, AND IS NOT ANY MORE ═══════
//
// ORIN-VPAR landed this family as `all(target_arch = "aarch64", feature = "witness", feature =
// "orinvpar")`. That closed the READING gap by naming it — and opened a new one measuring it: the
// drift detector (`parity-arch-gates.sh`) scores drift in BOTH directions, and menubar.rs went from
// ONE unpaired x86-only gate to THREE (one x86-only at [`selftest`]'s foot, two aarch64-only here).
// An instrument that exists on one chip and not the other IS the defect this instrument reports, and
// it does not stop being that defect because it is pointing the other way.
//
// Two things changed, and both are the same change:
//
// 1. **The arch term is gone.** These items ride `feature = "witness"` alone, exactly as the x86
//    `MENUBAR-OCC` leg does, so the line is emitted from BOTH chips and a capture from either is read
//    the same way. The one place the arch genuinely differs — whether `wm::occ_bar_probe` resolves —
//    is now a two-arm per-arch DISPATCH, [`occ_bar_reading`], which is what a real hardware
//    difference is supposed to look like. `arch=` on the line says which chip answered.
//
// 2. **The `orinvpar` term is gone too**, which matters more than it looks. `orinvpar` is DEFAULT
//    OFF, so on every default Pi/Orin witness boot the parity line was not in the image at all and an
//    aarch64 regression in this clip still read GREEN — the exact condition the leg was written to
//    end, surviving one knob further down. Its x86 counterpart needs no knob; neither does this. The
//    knob itself is untouched and still has consumers in `video/screen.rs`.
//
// What is still owed, and is NOT in this file: `wm::occ_bar_probe` and `OB_N`/`OB_PX`/`OB_BOX` are
// `all(feature = "witness", target_arch = "x86_64")`, and `video/wm.rs` is another executor's lane on
// this arc. Widening that one gate is the whole remaining port, and the diff is handed to the seat
// rather than applied here. When it lands, [`occ_bar_reading`]'s aarch64 arm is deleted and the arm
// above it loses its `target_arch` term — nothing else in this file moves.

/// MENUBAR-OCC-PAR — the chip that answered, for the wire. `cfg!` rather than a `#[cfg]` attribute
/// pair: this is one string, and per-arch attributes here would be arch drift inside the instrument
/// that exists to report arch drift.
#[cfg(feature = "witness")]
const PAR_ARCH: &str = if cfg!(target_arch = "x86_64") {
    "x86_64"
} else if cfg!(target_arch = "aarch64") {
    "aarch64"
} else {
    "other"
};

/// MENUBAR-OCC-PAR — one-shot latch for the `:: MENUBAR-OCC-PAR:` line. Steady state after it has
/// spent itself is one relaxed load per composite.
#[cfg(feature = "witness")]
static OCCPAR_DONE: AtomicBool = AtomicBool::new(false);

/// MENUBAR-OCC-PAR — off-passes to wait before declaring the bar never-enabled. A composite-rate
/// counter, so this is a few seconds of a live panel: long enough that a shell enabling the bar during
/// startup wins the race and gets the informative line, short enough that a capture never has to
/// wonder.
#[cfg(feature = "witness")]
const OCCPAR_DEADLINE_OFF_PASSES: u64 = 600;

/// MENUBAR-OCC-PAR — **the READING, one body.** Delegates to [`wm::occ_bar_probe`] — the present's
/// own span walk, never a copy of it — on BOTH arches since the probe's gate was widened to
/// "witness plus the menubar exists" (OCC-BAR-PAR, the fold that closed this file's own ledger
/// note about the pending two-arm dispatch). The `Option` stays: a future arch without the
/// instrument answers `None` here rather than growing a new cfg at every caller.
#[cfg(feature = "witness")]
fn occ_bar_reading(bar: strip::Rect, win: strip::Rect) -> Option<(u64, u64, u64, u64)> {
    let p = wm::occ_bar_probe(bar, win);
    Some((p.pop_prot, p.px_prot, p.pop_fault, p.px_fault))
}

/// MENUBAR-OCC-PAR — **the clip's PRECONDITION, measured, from this file alone, on both arches.**
///
/// Returns `(bar, win, crossed, at_risk_px)`. `win` is the x86 `MENUBAR-OCC` leg's synthetic window
/// box VERBATIM — at the top edge, four bar-heights tall (a real dragged window is taller than the
/// strip), half the panel wide and inset so it fits the smallest suite panel (640x480) as well as the
/// bench's — so both arches ask about the same geometry. `crossed` is the half-open overlap test on
/// both axes, i.e. `wm::boxes_overlap` inlined because that helper is private to `wm` and this is one
/// comparison, not a second definition of anything.
///
/// **It is ONE function called from BOTH legs** — [`occpar_once`] on the compose path and the
/// `MENUBAR-OCC` leg at the foot of [`selftest`] — where before it was the same arithmetic written
/// twice. Two copies of "the box that crosses the bar" is two things that can drift, and a parity
/// witness whose geometry has drifted from the fixture it claims parity with is worse than none.
///
/// `at_risk_px` is deliberately NOT named `occclip_bar_px`. It is the intersection's area: an upper
/// bound on what the clip withholds, computed HERE, and not the present's own arithmetic. Keeping the
/// names apart is what stops a later reader folding it into the x86 field and declaring a parity that
/// does not exist. (That distinction is ORIN-VPAR's and is kept verbatim.)
#[cfg(feature = "witness")]
fn occ_precondition(
    pw: usize,
    ph: usize,
    rect: Option<strip::Rect>,
) -> (strip::Rect, strip::Rect, bool, u64) {
    // `strip::Rect` IS `(x, y, w, h)`.
    let bar = rect.unwrap_or((0, 0, 0, 0));
    let win = (pw / 4, 0usize, pw / 2, (BAR_H * 4).min(ph));
    let crossed = bar.2 != 0
        && bar.3 != 0
        && win.2 != 0
        && bar.0 < win.0 + win.2
        && win.0 < bar.0 + bar.2
        && bar.1 < win.1 + win.3
        && win.1 < bar.1 + bar.3;
    // Saturating throughout: an instrument may not be the thing that overflows.
    let at_risk_px: u64 = if crossed {
        let w = (bar.0 + bar.2).min(win.0 + win.2).saturating_sub(bar.0.max(win.0));
        let h = (bar.1 + bar.3).min(win.1 + win.3).saturating_sub(bar.1.max(win.1));
        (w as u64).saturating_mul(h as u64)
    } else {
        0
    };
    (bar, win, crossed, at_risk_px)
}

/// MENUBAR-OCC-PAR — the answer when the bar IS live: the clip's precondition, measured, plus the
/// reading where one exists.
///
/// Everything upstream of the span walk is in this file and is checked here — the bar is enabled,
/// [`geometry`] answers a real rect on this panel, a realistic dragged-window box CROSSES it, and the
/// bar pixels AT RISK on that crossing are counted from the intersection. So `crossed=true
/// at_risk_px>0` establishes that the clip's precondition is live on this chip and that anything
/// missing is the READING — not the geometry and not the arming.
///
/// The verdict follows what the chip could answer. Where [`occ_bar_reading`] returns a reading it is
/// the probe's own verdict — `pop>0 && px_prot>0` fired, and `pop_fault>0 && px_fault==0` reproduces
/// the degenerate `x86-witness.spec`'s `occclip_bar=N>0 occclip_bar_px=0` FORBID trips on. Where it
/// returns none the verdict is `DECLINED` even though every precondition holds, because the leg cannot
/// answer the question it exists to ask; `pre_ok=` carries the half that IS proven, so the decline is
/// not information-free.
#[cfg(feature = "witness")]
fn occpar_once(pw: usize, ph: usize, rect: Option<strip::Rect>) {
    if OCCPAR_DONE.load(Ordering::Relaxed) {
        return;
    }
    // No geometry yet — try again next pass rather than report a zero rect as a finding.
    if rect.is_none() {
        return;
    }
    if OCCPAR_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let (bar, win, crossed, at_risk_px) = occ_precondition(pw, ph, rect);
    let pre_ok = crossed && at_risk_px > 0;
    let clob = CLOBBERS.load(Ordering::Relaxed);
    match occ_bar_reading(bar, win) {
        Some((pop, px_prot, pop_fault, px_fault)) => {
            let ok = pre_ok && pop > 0 && px_prot > 0 && pop_fault > 0 && px_fault == 0;
            serial_println!(
                ":: MENUBAR-OCC-PAR: arch={} bar_enabled=true clip_arm=present \
                 clip_gate=x86+wc|aarch64+desktop_firmware probe=present blocked_on=none \
                 occclip_bar={} occclip_bar_px={} forbid_bar={} forbid_bar_px={} \
                 bar={}x{}+{}+{} panel={}x{} win={}x{}+{}+{} crossed={} at_risk_px={} clob={} \
                 pre_ok={} :: {} ::",
                PAR_ARCH,
                pop, px_prot, pop_fault, px_fault,
                bar.2, bar.3, bar.0, bar.1,
                pw, ph,
                win.2, win.3, win.0, win.1,
                crossed, at_risk_px, clob, pre_ok,
                if ok { "PASS" } else { "FAIL" }
            );
        }
        // The aarch64 outcome. The blocking symbol and its exact gate go ON THE WIRE, so a Pi/Orin
        // capture says "this instrument is absent, and here is what would restore it" rather than
        // going quiet — the difference between a blind instrument and a reported one.
        None => {
            serial_println!(
                ":: MENUBAR-OCC-PAR: arch={} bar_enabled=true clip_arm=present \
                 clip_gate=x86+wc|aarch64+desktop_firmware probe=absent \
                 blocked_on=wm::occ_bar_probe blocker_gate=witness+x86_64 occclip_bar=absent \
                 occclip_bar_px=absent forbid_bar=absent forbid_bar_px=absent \
                 bar={}x{}+{}+{} panel={}x{} win={}x{}+{}+{} crossed={} at_risk_px={} clob={} \
                 pre_ok={} :: DECLINED ::",
                PAR_ARCH,
                bar.2, bar.3, bar.0, bar.1,
                pw, ph,
                win.2, win.3, win.0, win.1,
                crossed, at_risk_px, clob, pre_ok
            );
        }
    }
}

/// MENUBAR-OCC-PAR — the answer when the bar never turns on. Latches the same one-shot, so a shell
/// that enables the bar later gets silence here rather than a contradicting second line.
///
/// Without it a bar that is off for the whole boot leaves the question unasked, and an unasked
/// question looks exactly like a passing one on a capture.
#[cfg(feature = "witness")]
fn occpar_never_enabled(off_passes: u64) {
    if OCCPAR_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    serial_println!(
        ":: MENUBAR-OCC-PAR: arch={} bar_enabled=false clip_arm=present \
         clip_gate=x86+wc|aarch64+desktop_firmware probe={} occclip_bar=unasked \
         occclip_bar_px=unasked bar=none off_passes={} reason=bar_never_enabled_this_boot \
         :: DECLINED ::",
        PAR_ARCH,
        if cfg!(target_arch = "x86_64") { "present" } else { "absent" },
        off_passes
    );
}

/// SHELLDESK — **the COMPILED default, latched at the first mutation.**
///
/// [`selftest`]'s leg 1 ("absent by default") read [`ENABLED`] live, and its own comment named the
/// condition that would break it: *"if a future shell enables the bar before the battery runs, this
/// leg reds"*. The desktop shell now DOES enable the bar at desktop-ready (`desktop_uefi::activate`), so a
/// live read would report the shell's decision instead of the build's default and the leg would
/// answer a question nobody asked.
///
/// The claim the leg exists to defend is about the ARTIFACT — *the bar ships off; something has to
/// turn it on* — so what is observed is the value [`ENABLED`] held the first time anything wrote it.
/// `0` = never written (read the flag live, which is then still the initialiser), `1` = the default
/// was OFF, `2` = the default was ON. It stays falsifiable in exactly the way the leg's own fault
/// injection proved: building with `ENABLED` initialised to `true` makes the first `set_enabled`
/// latch `2`, and the leg reds — whether or not a shell has since turned it off again.
static DEFAULT_LATCH: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// SHELLDESK — was the bar OFF in the artifact? See [`DEFAULT_LATCH`].
#[cfg(feature = "witness")]
fn default_was_off() -> bool {
    match DEFAULT_LATCH.load(Ordering::Relaxed) {
        0 => !ENABLED.load(Ordering::Relaxed),
        1 => true,
        _ => false,
    }
}

/// Turn the bar on or off at runtime. Returns the previous state.
///
/// Turning it OFF is a full teardown: the next [`compose`] erases whatever rect the bar owned and
/// clears the slot, so the panel is left exactly as it was before the bar existed. That is what makes
/// the tenancy reversible rather than a one-way commitment, and it is the leg the fixture proves.
///
/// There is deliberately **no build knob**. A cargo feature would put the bar's presence in the
/// artifact rather than in the running system, and a spatial shell deciding at runtime that it wants
/// a HUD band is the case this primitive exists for.
pub fn set_enabled(on: bool) -> bool {
    let was = ENABLED.swap(on, Ordering::AcqRel);
    // SHELLDESK — latch the COMPILED default before the first write can hide it. `compare_exchange`
    // on the "never written" state, so only the first caller wins and a later toggle cannot rewrite
    // history. See [`DEFAULT_LATCH`].
    let _ = DEFAULT_LATCH.compare_exchange(
        0,
        if was { 2 } else { 1 },
        Ordering::AcqRel,
        Ordering::Relaxed,
    );
    if was != on {
        TOGGLES.fetch_add(1, Ordering::Relaxed);
        // CRYSTAL — turning the bar OFF must tear down the SHARD menu, or its dropdown would outlive
        // the crystal it hangs from with nothing left on the bar to dismiss it. Turning the bar ON
        // does not open it; a menu is opened by a press, never by a toggle.
        if !on {
            super::crystal::dismiss_for_bar_off();
        }
    }
    was
}

/// Is the bar on?
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// BRINGUP-PAINT (PA41) — **has the bar actually PUT PIXELS on the panel?** One relaxed packed load.
///
/// [`enabled`] is a statement of intent and [`strip_rect`] a statement of geometry; neither says a
/// composite ever ran, and the difference is the whole of the bring-up defect. From the instant the
/// bar is enabled `screen::present_background` subtracts its rect, so those rows stop being desktop
/// pixels and only a composite can fill them — and a composite whose [`strip::paint`] declined
/// (contended scratch, a surface not yet `word4`) leaves the bar enabled, its rows withheld, and
/// nothing on the glass. `super::desktop_firmware::activate` reads this back after its enable-seam composite
/// instead of assuming the composite painted, on the same "read the fact, do not infer it from your
/// own control flow" rule that seam already applies to `fbcon::console_is_routed`.
#[inline]
pub fn owns_pixels() -> bool {
    SLOT.packed() != 0
}

// ---------------------------------------------------------------------------
// Geometry — THE ONE accessor, shared by the painter, the registry and `wm`
// ---------------------------------------------------------------------------

/// **The bar's rect on a `pw` x `ph` panel, or `None`.** Registered in [`strip::TENANTS`], so this is
/// also what `wm::erase_clip` reads — there is no second copy of the bar's rectangle anywhere.
///
/// `None` for either of the two reasons a strip can be absent, and they are not distinguished here on
/// purpose: the clip's question is only *"do you own pixels"*.
///
///  * **disabled** — the default state;
///  * **the panel cannot host it** — shorter than [`FLOOR_H`] or narrower than [`FLOOR_W`].
pub fn strip_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    geometry(pw, ph)
}

/// The geometry alone, with the enable gate removed — so the floor can be tested for a panel the
/// fixture is not running on, and so a caller asking *"would it fit"* is not answered *"it is off"*.
pub fn geometry(pw: usize, ph: usize) -> Option<strip::Rect> {
    if pw < FLOOR_W {
        return None;
    }
    strip::frame_flush(strip::Edge::Top, BAR_H, FLOOR_H, pw, ph)
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

/// Everything the painter reads, gathered once per pass.
#[derive(Clone, Copy)]
struct Model {
    title: [u8; wm::MAX_TITLE],
    title_len: usize,
    /// `HH:MM`, or `None` while the civil clock has never been anchored this boot.
    clock: Option<[u8; CLOCK_GLYPHS]>,
    /// WINMENU (R21) — **the window whose menus this bar is showing**: the FRONTMOST VISIBLE row that
    /// has published a tree, or [`wm::WIN_NONE`].
    ///
    /// ⚠ It is deliberately NOT the same reduction as the caption's, and the difference is a fact
    /// about this kernel rather than a taste call. The caption's `focused` flag is an OWNER-ASID
    /// match, and the click router hands SHELL focus (asid `0`) to a press on kernel furniture
    /// (`is_kernel_owner`) — so the first publisher this arc has, [`super::pulsewin`], can never be
    /// `focused` by that test and an owner-keyed menu selection would show nothing for the one window
    /// it exists to serve. "Frontmost visible publisher" is what an operator means by *the window in
    /// front*, is computed from the same single [`wm::dock_scan`] the caption already runs, and is
    /// stated on the wire (`[winmenu] publish owner=`) so the two readings can be compared rather
    /// than confused.
    menu_owner: wm::WinId,
    /// WINMENU — the title boxes, laid out once per compose and handed to the row painter. Filled in
    /// by [`compose`] after the rect is settled, because the layout is a function of the bar rect.
    menus: super::winmenu::BarSnapshot,
}

impl Model {
    fn empty() -> Model {
        Model {
            title: [0; wm::MAX_TITLE],
            title_len: 0,
            clock: None,
            menu_owner: wm::WIN_NONE,
            menus: super::winmenu::BarSnapshot::empty(),
        }
    }

    /// The FOCUSED window's caption plus the wall clock.
    ///
    /// `wm::dock_scan` is the only public route to a caption — `wm::info` deliberately does not carry
    /// one — and its `focused` flag is an OWNER match, so several rows of one ASID all report
    /// `focused`. The bar wants the TOPMOST of them, which is why the scan is reduced by `z` through
    /// `wm::info` rather than by taking the first hit: taking the first would name whichever row has
    /// the lowest window id, which is not the window the operator is looking at.
    /// CLOBBER-REPAIR (PA41) — `painted` is the rect the bar last put on the panel ([`SLOT`]`.rect()`),
    /// and the second half of the returned pair answers [`wm::dock_scan`]'s damage question about it:
    /// *did any row the compositor is painting this pass intersect those pixels?* A zero-extent rect
    /// asks nothing and always answers `false`, which is what the bar passed before this arc — the
    /// model was taken and the damage answer thrown away, so the one writer that could destroy the bar
    /// without changing its signature was the one thing the bar never looked at.
    fn read(painted: (usize, usize, usize, usize)) -> (Model, bool) {
        let mut m = Model::empty();
        let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
        let (n, clobbered) = wm::dock_scan(&mut rows, painted);
        let mut best_z = 0u32;
        // WINMENU — the SECOND reduction, over the SAME scan: the frontmost visible PUBLISHER. It is
        // separate from the caption's because `focused` is an owner match and kernel furniture takes
        // shell focus; see [`Model::menu_owner`]. `winmenu::has_tree` is lock-free
        // (`WINMENU_MAX` relaxed loads, short-circuited to nothing when nothing has published), so
        // this costs a boot with no menus one atomic and no second table walk.
        let (mut menu_z, mut menu_owner) = (0u32, wm::WIN_NONE);
        for r in rows[..n].iter() {
            // PANEL V-4 — `wm::info` is LAZY, below both guards. It takes `wm::TABLE` with IRQs
            // masked (`wm::table`, and `wm.rs`'s standing rule about that critical section), so
            // hoisting it above the guards turned one masked acquisition per bar compose into up to
            // `MAX_WINDOWS` of them, on every composite, on every gated build — a regression against
            // trunk, and it silently falsified the claim three lines up: the `has_tree`
            // short-circuit cannot save a boot with no menus if `z` is computed before it is asked.
            let publisher = r.visible && super::winmenu::has_tree(r.id);
            if !publisher && (!r.focused || !r.visible) {
                continue;
            }
            let z = wm::info(r.id).map(|i| i.z).unwrap_or(0);
            if publisher && (menu_owner == wm::WIN_NONE || z >= menu_z) {
                menu_z = z;
                menu_owner = r.id;
            }
            if !r.focused || !r.visible {
                continue;
            }
            if z >= best_z {
                best_z = z;
                m.title_len = r.title_len.min(wm::MAX_TITLE);
                m.title = r.title;
            }
        }
        m.menu_owner = menu_owner;
        m.clock = clock_hhmm();
        (m, clobbered)
    }

    /// The model reduced to one integer — the whole "has anything changed?" test.
    ///
    /// Everything the painter reads goes in, and the layout with it (which folds in the panel
    /// geometry). A field the painter uses and this hash omits is a field whose change would leave a
    /// stale bar on the panel, so the two lists are the same list on purpose.
    fn signature(&self, r: strip::Rect) -> u64 {
        let mut h = strip::FNV_BASIS;
        for v in [r.0 as u64, r.1 as u64, r.2 as u64, r.3 as u64] {
            h = strip::fnv1a_u64(h, v);
        }
        h = strip::fnv1a(h, self.title_len as u8);
        for &b in self.title[..self.title_len].iter() {
            h = strip::fnv1a(h, b);
        }
        match self.clock {
            Some(c) => {
                h = strip::fnv1a(h, 1);
                for &b in c.iter() {
                    h = strip::fnv1a(h, b);
                }
            }
            None => h = strip::fnv1a(h, 0),
        }
        // WINMENU — the title boxes are part of what the painter reads, so they are part of the
        // "has anything changed?" test. A title that appears, moves, is relabelled or OPENS must
        // repaint the bar; without this fold the bar would keep a stale set of menus on the glass for
        // as long as the caption and the clock happened not to move, which on a quiet desktop is
        // minutes. (The two lists are the same list on purpose — this function's own rule.)
        h = strip::fnv1a_u64(h, self.menus.signature());
        strip::seal(h)
    }
}

/// UTC `HH:MM` from the shared civil clock, or `None` while it has never been anchored.
///
/// Minute granularity is the whole reason the bar is cheap: the signature changes once a minute, so
/// the clock costs one repaint every 60 s rather than one per frame. Seconds would make the bar the
/// most expensive thing in the composite and would be a worse clock — nothing on a menu bar is read
/// to the second.
fn clock_hhmm() -> Option<[u8; CLOCK_GLYPHS]> {
    // `try_unix_now`, never `unix_now`: this runs at the composite tail and the primitive's rule is
    // that nothing on that path spins on a lock. A contended anchor (a concurrent SNTP sync or
    // `setdate`) yields `None`, the bar draws no clock this pass and repaints on the next — the same
    // decline-and-retry shape `strip::paint` uses for its scratch. `None` reads `unsynced`, which is
    // also what a never-anchored clock reads, so a momentary contention is indistinguishable from
    // "no time yet" and neither fabricates one.
    let secs = crate::clock::try_unix_now()?;
    let (_, _, _, h, m, _) = crate::clock::civil_from_unix(secs);
    let d = |v: u32, k: u32| b'0' + ((v / k) % 10) as u8;
    Some([d(h, 10), d(h, 1), b':', d(m, 10), d(m, 1)])
}

// ---------------------------------------------------------------------------
// The composite seam
// ---------------------------------------------------------------------------

/// **The bar's whole per-composite cost.** Called from `strip::compose_all` at the tail of every
/// pass, after the window loop and before the cursor tail.
///
/// Returns `true` iff it painted, in which case the caller owes the sprite a `Repaint` — [`strip::paint`]
/// has already taken the arrow off the panel.
///
/// **The disabled path is the first line**, before the cycle counter, before the framebuffer lock,
/// before the clock's lock and before the window table: three relaxed atomics — the enable load, the
/// off-pass counter, and the owed-pixels check — and a return. A bar that is off does not scan, does
/// not hash, and cannot repaint. (The owed-pixels load is `SLOT.packed()`, which is `0` on every
/// pass of a bar that was never on, so the erase branch is not taken; a bar that was turned OFF pays
/// one erase, once, and then reads `0` too.)
pub fn compose() -> bool {
    if !ENABLED.load(Ordering::Relaxed) {
        let off = OFF_PASSES.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        // MENUBAR-OCC-PAR — the NEVER-ENABLED outcome. See [`occpar_once`]: a bar that is off all
        // boot would otherwise leave the parity question unanswered, and an unanswered question and a
        // passing one look identical on a capture. PAR-MENUBAR: no arch term — the x86 leg of this
        // proof carries no knob and no arch gate either, and an instrument present on one chip only
        // is the defect this instrument reports.
        #[cfg(feature = "witness")]
        if off == OCCPAR_DEADLINE_OFF_PASSES {
            occpar_never_enabled(off);
        }
        #[cfg(not(feature = "witness"))]
        let _ = off;
        // A bar that was turned OFF still owes the pixels it owned. One packed load answers it, and
        // the erase runs exactly once — the slot is cleared by it.
        if SLOT.packed() != 0 {
            let r = SLOT.rect();
            SLOT.clear();
            return strip::erase_rect(r);
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
    let rect = geometry(pw, ph);
    // MENUBAR-OCC-PAR — the FIRED-precondition outcome, one-shot, on the only site of this path that
    // has BOTH the panel and a settled rect. Here rather than in [`selftest`] because on aarch64
    // [`selftest`] is UNREACHABLE: its only caller chain is `arch/x86_64/syscall.rs` →
    // `dock::selftest` → there, so an aarch64 image carries no `:: MENUBAR:` string at all (verified
    // on the built artifact, not assumed). A parity witness placed in a fixture that cannot run would
    // have been the very defect this arc was sent to remove.
    //
    // PAR-MENUBAR: it runs on x86 too, and that is the point. The x86 pass carries the READING and
    // the aarch64 pass carries `probe=absent blocked_on=`, so the two captures differ by one field
    // instead of by a missing paragraph, and the missing field NAMES what would restore it.
    #[cfg(feature = "witness")]
    occpar_once(pw, ph, rect);
    // CLOBBER-REPAIR (PA41) — the model AND the damage question, from the ONE table scan `dock_scan`
    // already ran for the caption. The rect asked about is what the bar last PAINTED, never what it is
    // about to paint: the question is whether those pixels survived.
    let (mut model, clobbered) = if rect.is_some() {
        Model::read(SLOT.rect())
    } else {
        (Model::empty(), false)
    };
    // WINMENU (R21) — publish which window's menus the bar is showing, then take the title layout
    // for it. ORDER matters: `set_bar_owner` is what an input-path press reads to find out whose menu
    // it hit (so no press ever takes the window table's lock for that), and it dismisses an open
    // dropdown whose window has just stopped being frontmost — which must happen BEFORE the snapshot
    // is taken, or this pass would lay out a title box for a menu that is about to be torn down.
    super::winmenu::set_bar_owner(model.menu_owner);
    model.menus = match rect {
        Some(_) => super::winmenu::bar_boxes(pw, ph),
        None => super::winmenu::BarSnapshot::empty(),
    };
    // PANEL V-3 — the winmenu registry was CONTENDED taking that snapshot, so it lays out no titles
    // for a reason that has nothing to do with what is published. Painting it would repaint the bar
    // BLANK for one frame and change its signature to match, so the flicker would be recorded as the
    // truth. DECLINE the pass instead — `strip::paint`'s own rule — and re-ask next composite.
    if model.menus.busy {
        return false;
    }
    if clobbered {
        CLOBBERS.fetch_add(1, Ordering::Relaxed);
    }
    let sig = match rect {
        Some(r) => model.signature(r),
        None => 0,
    };
    LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
    LEDGER.tick("menubar", format_args!("press=crystal crystal={}x{} clob={} toggles={} off_passes={}",
        CRYSTAL_W, CRYSTAL_H, CLOBBERS.load(Ordering::Relaxed),
        TOGGLES.load(Ordering::Relaxed), OFF_PASSES.load(Ordering::Relaxed)));

    // The damage conditions, in the order the dock states them: a signature that MATCHES and a pass
    // that did not touch the strip is the common case and returns here having read no pixel.
    if sig == SLOT.sig() && SLOT.packed() == strip::pack_rect(rect) && !clobbered {
        return false;
    }

    // THE STRIP OWES ITS OWN VACATED PIXELS — `strip::erase_rect`'s rule. A bar that goes away (the
    // panel shrank below the floor) or moves would otherwise leave its old rows standing.
    let old = SLOT.packed();
    let new = strip::pack_rect(rect);
    let vacated = if old != 0 && old != new { Some(strip::unpack_rect(old)) } else { None };

    let Some(r) = rect else {
        SLOT.clear();
        return match vacated {
            Some(v) => strip::erase_rect(v),
            None => false,
        };
    };

    let t1 = crate::arch::now_cycles();
    if let Some(v) = vacated {
        // Erase FIRST, then paint, so the two never race to own an overlapping pixel.
        strip::erase_rect(v);
    }
    if !strip::paint("menubar", r, |out, j| compose_row(out, &model, r, j)) {
        return false;
    }
    LEDGER.paint(crate::arch::now_cycles().saturating_sub(t1), (r.2 * r.3) as u64);
    SLOT.store(sig, Some(r));
    true
}

/// The crystal's box-relative top-left in the bar: one [`strip::PAD`] from the left, centred
/// vertically. THE ONE offset both the painter and [`crystal_box`] read, so the mark the fixture
/// witnesses is the mark the painter drew.
#[inline]
fn crystal_offset(h: usize) -> (usize, usize) {
    (strip::PAD, (h - CRYSTAL_H) / 2)
}

/// The crystal's rect on the PANEL, for the witness — `(x, y, w, h)`, absolute. A function of the bar
/// rect and nothing else, so `crystal=WxH+X+Y` on the fixture line is falsifiable against where the
/// painter put it.
fn crystal_box(r: strip::Rect) -> (usize, usize, usize, usize) {
    let (rx, ry, _w, h) = r;
    let (ox, oy) = crystal_offset(h);
    (rx + ox, ry + oy, CRYSTAL_W, CRYSTAL_H)
}

/// **The crystal's absolute rect on a `pw` x `ph` panel, or `None`** when the bar is absent (disabled
/// or the panel cannot host it). THE accessor the SHARD menu ([`super::crystal`]) reads to hit-test a
/// press on the mark and to anchor its dropdown — a function of [`strip_rect`] and [`crystal_box`]
/// alone, so the box the menu opens from is the box the painter drew.
pub fn crystal_box_abs(pw: usize, ph: usize) -> Option<strip::Rect> {
    strip_rect(pw, ph).map(crystal_box)
}

/// FITTS-CORNER — **the crystal's PRESS cell: the bar's whole upper-left corner, or `None`.**
///
/// Peter, at the Orin bench (2026-08-25): *"the crystal menu took too much exact aim to open — the
/// ENTIRE upper left corner where it lives should open the menu, not clicking directly on the
/// crystal."* The glyph box is 16x22 with a [`strip::PAD`] inset on every side — a Fitts target that
/// demands aim at exactly the place aim should be free: a screen corner is the one target a flick
/// reaches with none, because the edges stop the pointer. So the PRESS target and the PAINT box part
/// ways here: [`crystal_box_abs`] stays the painter's and the dropdown-anchor's truth, and this cell
/// is what the click router hits against.
///
/// Derived, not hardcoded: anchored at the bar rect's own origin (the true panel corner — the bar is
/// `frame_flush(Top)`, so `(0,0)` is inside by construction), spanning the crystal's whole left slot
/// — [`TITLE_X0`] wide (`PAD + CRYSTAL_W + PAD`, everything left of the title's inset, i.e. the glyph
/// cell with both of its margins) by the bar's full height. Every pixel of the cell is a pixel the
/// BAR paints and composites above the windows, so widening the press target to it steals nothing: a
/// window dragged under the corner is under the bar there, and a press on visible bar chrome routing
/// to bar furniture is the fixed-furniture rule, not an exception to it.
///
/// `None` exactly when [`crystal_box_abs`] is `None` (bar disabled, or the panel cannot host it), so
/// a press with no bar still falls through to the arms below.
pub fn crystal_corner_abs(pw: usize, ph: usize) -> Option<strip::Rect> {
    let (bx, by, _bw, bh) = strip_rect(pw, ph)?;
    Some((bx, by, TITLE_X0, bh))
}

/// WINMENU (R21) — **where the focused window's menu titles begin**, as an offset from the bar's own
/// origin. See [`MENUS_X0`]: a fixed slot, so the titles do not slide when the caption changes.
#[inline]
pub fn menus_x0() -> usize {
    MENUS_X0
}

/// WINMENU (R21) — **the panel-absolute x the menu titles must stop before**: the clock's left edge,
/// less one [`strip::PAD`].
///
/// Derived from the SAME two terms [`compose_row`] draws the clock at ([`CLOCK_GLYPHS`] and
/// [`strip::PAD`]) rather than restated, so a title can never be laid out under the time. A bar too
/// narrow to hold the clock at all answers its own left edge, which lays out no titles.
pub fn menus_right_limit(bar: strip::Rect) -> usize {
    let (bx, _by, bw, _bh) = bar;
    let need = 2 * strip::PAD + CLOCK_GLYPHS * CELL_W;
    bx + bw.saturating_sub(need)
}

/// **THE ONE transient-dropdown accessor** — the rect of whichever menu is currently down, or `None`.
///
/// `wm::occ_clip`, `wm::composite_inner`'s sprite arm and `screen::present_background` each ask "where
/// is the open dropdown" exactly once, and before R21 each asked [`super::crystal::open_rect`] by
/// name. There are now TWO surfaces that can be that dropdown — the SHARD menu and a window menu —
/// and `wm::MENU_OCC_MAX` reserves capacity for ONE.
///
/// That budget is not a bet. The two are mutually exclusive BY CONSTRUCTION: `winmenu::press_at` runs
/// first in [`strip::press_route`] and consumes every press while a window menu is down (so the
/// crystal's closed-corner arm is unreachable), and its own closed arm declines every point while
/// [`super::crystal::is_open`] (so the crystal keeps its dismiss-outside press). `winmenu`'s header
/// states that argument where it is enforced. This function is the reading of it: at most one arm
/// can answer `Some`, so the three sites keep asking one question and `MENU_OCC_MAX` stays 1.
pub fn open_dropdown_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    super::crystal::open_rect(pw, ph).or_else(|| super::winmenu::open_rect(pw, ph))
}

/// Half the crystal's silhouette width at box-relative row `v`, in px.
///
/// A brilliant-cut gem: the CROWN (rows `< CRYSTAL_CROWN_H`) widens from a narrow table at the top to
/// the full girdle; the PAVILION (the rest) tapers from the girdle to the point at the bottom. Integer
/// interpolation, no float — the same discipline the disc and knurl idioms use.
#[inline]
fn crystal_half(v: usize) -> usize {
    let table = CRYSTAL_W / 4;
    let girdle = CRYSTAL_W / 2;
    if v < CRYSTAL_CROWN_H {
        table + (girdle - table) * v / CRYSTAL_CROWN_H
    } else {
        let pav = CRYSTAL_H - CRYSTAL_CROWN_H;
        girdle * (CRYSTAL_H - v) / pav
    }
}

/// The crystal's facet colour at box-relative `(u, v)`, or `None` outside the silhouette.
///
/// Three facets from the kit's blue gem ramp, lit from the top-right: the crown's RIGHT face catches
/// the light ([`theme::CONTROL_ZOOM`], lightest), its LEFT face is in shadow
/// ([`theme::CONTROL_CLOSE`], darkest), and the pavilion below the girdle is the medium tone
/// ([`theme::CONTROL_MID`]). The high-contrast CLOSE|ZOOM seam down the centre reads as the crown's
/// facet edge; the colour change at the girdle reads as the girdle line. Two facet lines and a table,
/// dense and minimal — a crystal at 16x22.
#[inline]
fn crystal_facet(u: usize, v: usize) -> Option<u32> {
    if v >= CRYSTAL_H || u >= CRYSTAL_W {
        return None;
    }
    let cx = CRYSTAL_W / 2;
    let half = crystal_half(v);
    let du = if u >= cx { u - cx } else { cx - u };
    if du > half {
        return None;
    }
    Some(if v < CRYSTAL_CROWN_H {
        if u < cx { theme::CONTROL_CLOSE } else { theme::CONTROL_ZOOM }
    } else {
        theme::CONTROL_MID
    })
}

/// Compose panel row `j` of the bar into `out[0..w]` as logical `0x00RRGGBB` colours.
///
/// A field pass (face, bevel, keyline), then the brand CRYSTAL overlaid at the left edge, then the two
/// texts overlaid by index — the dock's shape, and for its reason: an overlay keeps the inner loop a
/// handful of integer compares instead of a scan over every caption at every pixel. The bar is FLUSH,
/// so there is no corner arithmetic at all.
fn compose_row(out: &mut [u32], m: &Model, r: strip::Rect, j: usize) {
    let (_, _, w, h) = r;
    // The material is anchored to the STRIP, not to the panel: index ceramic by the row's offset
    // inside the box, exactly as the window chrome indexes it by the row's offset inside the window.
    let face = ceramic::shade(theme::CHROME_FACE, j);
    let fill = if j < theme::BEVEL {
        theme::BEVEL_LIGHT
    } else if j + 1 == h {
        // The bar's ONE keyline: the bottom edge, where it meets the desktop. The other three edges
        // are the panel's, and a keyline there would be a line drawn against nothing.
        ceramic::shade(theme::FRAME_LINE, j)
    } else {
        face
    };
    for i in 0..w {
        out[i] = fill;
    }

    // The brand CRYSTAL, overlaid at the left. Drawn BEFORE the text early-return because the gem
    // spans more rows than the glyph cell does — it is centred in the whole bar, not the text band.
    let (cx0, cy0) = crystal_offset(h);
    if j >= cy0 && j < cy0 + CRYSTAL_H {
        let v = j - cy0;
        for u in 0..CRYSTAL_W {
            if let Some(c) = crystal_facet(u, v) {
                let i = cx0 + u;
                if i < w {
                    out[i] = c;
                }
            }
        }
    }

    // The two texts share a baseline: vertically centred in the bar.
    let ty0 = (h - CELL_H) / 2;

    // WINMENU (R21) — **the focused window's MENU TITLES, in the bar.** Peter: *"menus belong in the
    // menu bar"*. Overlaid here, in the bar's own single paint, rather than composited as a second
    // surface on top of it — a title is bar chrome, and a strip that had to be repainted every time a
    // title lit would be two damage models for one row of pixels.
    //
    // It is BEFORE the text-band early-return because an OPEN title's box is filled for the bar's
    // whole height (the dropdown reads as hanging from a lit title, not floating under a flat strip),
    // and that fill lands on rows the caption never touches. `winmenu::draw_bar_row` is handed the
    // band (`ty0`) rather than recomputing it, so a title's baseline cannot drift from the caption's.
    super::winmenu::draw_bar_row(out, w, &m.menus, j, ty0);

    if j < ty0 || j >= ty0 + CELL_H {
        return;
    }
    let sy = j - ty0;

    // Title, at [`TITLE_X0`] — past the crystal, as macOS puts its app menus to the right of the logo.
    // The focused window's caption; nothing when nothing is focused, which is an empty bar rather than
    // a placeholder.
    //
    // FONT (GR27) — the shared anti-aliased face, alpha-composited over the row the bar already
    // painted (a RAM scratch row — the read the blend does is cached, never a panel mapping).
    // BOLD, the weight macOS gives the menu bar's app name; the clock stays regular, the same
    // primary/secondary split the two inks already draw.
    let cols = m.title_len.min(TITLE_GLYPHS);
    super::font::draw_row(out, w, &m.title[..cols], TITLE_X0, sy, theme::TITLE_TEXT_ACTIVE, true, FACE);

    // Clock, right, at one PAD from the far edge. Secondary ink: the title is what the operator is
    // reading, the clock is what they glance at.
    if let Some(c) = m.clock {
        let cw = CLOCK_GLYPHS * CELL_W;
        if w > cw + strip::PAD {
            super::font::draw_row(out, w, &c, w - strip::PAD - cw, sy, theme::TITLE_TEXT_INACTIVE, false, FACE);
        }
    }
}

/// The bar's ledger line. Same terms as the dock's, plus the two that make "absent costs nothing"
/// falsifiable: `off_passes` is how many composites ran with the bar disabled, and a boot that never
/// enables it prints `paints=0` beside a nonzero `off_passes`.
pub fn rollup(scope: &str) {
    LEDGER.rollup(
        "menubar",
        scope,
        format_args!(
            "press=crystal crystal={}x{} clob={} toggles={} off_passes={}",
            CRYSTAL_W,
            CRYSTAL_H,
            CLOBBERS.load(Ordering::Relaxed),
            TOGGLES.load(Ordering::Relaxed),
            OFF_PASSES.load(Ordering::Relaxed)
        ),
    );
}

// ---------------------------------------------------------------------------
// Witness
// ---------------------------------------------------------------------------

/// MENUBAR fixture — **the bar is absent by default, present when asked, and leaves no trace when
/// dismissed.**
///
/// Six legs, each able to FAIL on its own. The fixture restores the bar to DISABLED before it
/// returns, so it cannot leave the panel in a state the rest of the boot did not ask for.
///
/// 1. **absent by default** — the ARTIFACT ships with the bar off ([`DEFAULT_LATCH`], observed at the
///    first write rather than live, since the desktop shell enables the bar at desktop-ready), and a
///    disabled bar's [`strip_rect`] is `None`. A bar that defaulted on would fail here, which is the
///    whole of the direction's "absent by default".
/// 2. **absent costs nothing on the CLIP** — with the bar off, `strip::rects` reports exactly the
///    strips that are actually on the glass, and the menubar slot is `None`. This is the leg that
///    says a disabled tenant consumes no occlusion capacity: it is not merely uncounted, its slot in
///    the registry's output is empty.
/// 3. **present when enabled, and FLUSH** — after `set_enabled(true)` the rect is `(0, 0, pw, BAR_H)`.
///    A centred or inset bar fails here.
/// 4. **registry membership** — `strip::rects` now carries the bar at the menubar slot, and the
///    present count went UP by exactly one. A tenant that painted but never entered the clip would
///    pass leg 3 and fail here, and it is the failure the WCK4 review named: a strip absent from the
///    clip is a strip the erase publishes over while every witness reads CLEAN.
/// 5. **the floor declines, both ways** — [`geometry`] answers `None` for a panel below [`FLOOR_H`]
///    and `Some` for one above it. Driven with synthetic panel sizes so the leg holds on whatever
///    panel the fixture is actually running on. A floor that admitted every panel would fail the
///    first half; a floor that declined every panel would fail the second, so neither direction can
///    pass by accident.
/// 6. **dismissal is complete** — `set_enabled(false)` followed by a compose leaves the slot cleared,
///    i.e. the bar erased what it owned. A tenant that could be turned on but not off would leave a
///    strip on the glass that nothing on the panel could account for.
/// 7. **the crystal is drawn, and inside the bar** — with the bar enabled, [`crystal_box`] is
///    `CRYSTAL_W`x`CRYSTAL_H` and sits wholly within the bar rect (left of the title, top and bottom
///    clear of the edges). A brand mark that could not be shown drawn would be unfalsifiable; this leg
///    is the geometry a metal capture and the fixture both read as `crystal=WxH+X+Y`.
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
    let saved = enabled();

    let mut slots = [None; strip::STRIP_MAX];

    // Legs 1-2 — absent by DEFAULT, and the word "default" is load-bearing.
    //
    // ⛔ **The first cut of this leg was VACUOUS and a fault injection caught it.** It called
    // `set_enabled(false)` and then asserted the bar was off, which is a tautology: it proved the
    // setter works, not that the flag STARTS off. Building with `ENABLED` initialised to `true` left
    // this fixture printing `default_off=true … PASS` while `[drag-occ]` was reporting `bars=2/2
    // bar=1280` — the bar on the glass and in the erase clip, with its own witness calling it absent.
    //
    // So the flag is observed BEFORE anything in this function mutates it. That reading is the static
    // initialiser's value because this fixture is the first code to touch it: it runs once, from the
    // x86 selftest block, and nothing enables the bar on any boot path. `initial=` goes on the line so
    // a reader sees what was observed rather than trusting that claim — if a future shell enables the
    // bar before the battery runs, this leg reds and the line says why.
    // SHELLDESK — the default is read from the LATCH, not from the live flag. The desktop shell
    // enables the bar at desktop-ready (`desktop_uefi::activate`), which on a Kepler boot happens long before
    // this battery runs; a live read would then report the SHELL's decision and call the artifact's
    // default a defect. `initial=` on the line is still the live flag, so a reader sees both facts.
    // The fault injection the original leg was hardened by still reds this: `ENABLED` initialised to
    // `true` latches "default was ON" at the first write. And the "absent" half is now ESTABLISHED
    // rather than assumed — the fixture turns the bar off itself, which is what makes legs 1-2
    // meaningful on a boot that arrives here with the bar already on.
    let initial = enabled();
    set_enabled(false);
    let default_off = default_was_off() && strip_rect(pw, ph).is_none();
    let n_off = strip::rects(pw, ph, &mut slots);
    let clip_clean = slots[strip::MENUBAR_SLOT].is_none();

    // Legs 3-4 — present when enabled, and in the clip.
    set_enabled(true);
    let r = strip_rect(pw, ph);
    let flush = r == Some((0, 0, pw, BAR_H));
    let n_on = strip::rects(pw, ph, &mut slots);
    let member = slots[strip::MENUBAR_SLOT] == r && r.is_some() && n_on == n_off + 1;

    // Leg 5 — the floor declines, both ways. Synthetic panels, so the leg does not depend on the one
    // the fixture happens to be running on.
    let floor_declines = geometry(pw.max(FLOOR_W), FLOOR_H - 1).is_none()
        && geometry(FLOOR_W - 1, ph.max(FLOOR_H)).is_none();
    let floor_admits = geometry(FLOOR_W, FLOOR_H).is_some();

    // Leg 6 — dismissal is complete. Paint once with the bar on, then turn it off and compose again;
    // the slot must come back to "nothing painted".
    let _ = compose();
    let owned = SLOT.packed();
    set_enabled(false);
    let _ = compose();
    let dismissed = SLOT.packed() == 0;

    // Leg 7 — the crystal is drawn, and inside the bar. `r` is the enabled rect (from leg 3); the
    // crystal box must be the compiled size and sit wholly within it, left of the title inset.
    let (cbx, cby, cbw, cbh) = r.map(crystal_box).unwrap_or((0, 0, 0, 0));
    let crystal_ok = match r {
        Some((brx, bry, brw, brh)) => {
            cbw == CRYSTAL_W
                && cbh == CRYSTAL_H
                && cbx >= brx
                && cbx + cbw <= brx + brw
                && cby >= bry
                && cby + cbh <= bry + brh
                && cbx + cbw <= brx + TITLE_X0 // left of where the title begins
        }
        None => false,
    };

    set_enabled(saved);

    let clock = match clock_hhmm() {
        Some(_) => "set",
        None => "unsynced",
    };
    let ok = default_off && clip_clean && flush && member && floor_declines && floor_admits
        && dismissed && crystal_ok;
    let (rx, ry, rw, rh) = r.unwrap_or((0, 0, 0, 0));
    serial_println!(
        ":: MENUBAR: bar={}x{}+{}+{} panel={}x{} floor={}x{} strips={}->{} owned={:#x} clock={} \
         crystal={}x{}+{}+{} initial={} press=crystal default_off={} clip_clean={} flush={} \
         member={} floor={}/{} dismissed={} crystal_ok={} :: {} ::",
        rw, rh, rx, ry, pw, ph, FLOOR_W, FLOOR_H, n_off, n_on, owned, clock,
        cbw, cbh, cbx, cby, initial,
        default_off, clip_clean, flush, member, floor_declines, floor_admits, dismissed, crystal_ok,
        if ok { "PASS" } else { "FAIL" }
    );

    // ─── MENUBAR-OCC — the bar's WINDOW-BLIT occlusion protection, PROVEN able to fire ───────────────
    //
    // `wm::occ_clip` pushes this bar into every window's clip and the present folds the withheld pixels
    // into `occclip_bar=`/`occclip_bar_px=` on `[drag-occ]`, FORBIDden degenerate by `x86-witness.spec`
    // (`occclip_bar=N>0 occclip_bar_px=0`). That push is code-correct — the same `span_occ` formula the
    // proven DOCK path uses — but the FORBID is VACUOUS on every boot: the bar is DEFAULT OFF, so no
    // gate ever drags a window across the top strip and `occclip_bar` never leaves 0. "A witness that
    // cannot fail is a defect", so this leg FIRES it, exactly as WCK5 fired the dock's (engine.md
    // §WCK5): it ENABLES the bar, drives a synthetic window box across the top strip, and reads the
    // pair `wm::occ_clip`'s own primitives produce for it.
    //
    // Two runs, from [`wm::occ_bar_probe`]:
    //   * PROTECTED — the bar pushed into the clip, the span walk withholds its columns: `px_prot > 0`,
    //     the window's chrome kept off the strip. This is the fired witness.
    //   * FAULT — the strip still counted in the population but the clip walked empty (the span walk
    //     publishes its columns): `px_fault == 0` with `pop_fault > 0`, the exact
    //     `occclip_bar=N>0 occclip_bar_px=0` state the FORBID trips on — proven non-vacuous rather than
    //     trusted, WCK5's reverted probe computed instead of pinned.
    //
    // Its own enable→probe→restore cycle: the bar is turned ON here (the main fixture already restored
    // it to `saved` above) and put back to `saved` at the end, so the standing state — DEFAULT OFF on
    // a gate boot, ON once the desktop shell has asked for it — is exactly what it was on entry.
    // ⚠ **PAR-MENUBAR: THE BARE `#[cfg(target_arch = "x86_64")]` THAT STOOD HERE IS GONE**, and what
    // it was actually protecting is worth naming, because it was read as the opposite of what it said.
    // The block is arch-neutral except for ONE symbol — `wm::occ_bar_probe`, which is
    // `all(feature = "witness", target_arch = "x86_64")`. The gate existed to stop an aarch64 build
    // failing to RESOLVE that name; written as a bare arch gate it made the PROTECTION look x86-only
    // when only the READING is, which is the exact confusion this module's header correction was about.
    // It is now the per-arch dispatch [`occ_bar_reading`], and the geometry comes from
    // [`occ_precondition`] — the SAME function [`occpar_once`] uses — so the synthetic window box and
    // the crossing test have one definition in this file instead of two that could drift apart.
    //
    // This leg still only RUNS on x86, and that is a REACHABILITY fact, not an arch gate: [`selftest`]'s
    // only caller chain is `arch/x86_64/syscall.rs` → `dock::selftest` → here. That is why the aarch64
    // half of this proof lives on the compose path in [`occpar_once`] rather than in an arm added here.
    {
        set_enabled(true);
        let bar_rect = strip_rect(pw, ph);
        let (bar, win, crossed, _at_risk_px) = occ_precondition(pw, ph, bar_rect);
        let reading = occ_bar_reading(bar, win);
        // Restore THE STATE THE BOOT ARRIVED IN before the verdict, so `restored` reads the standing
        // state the rest of the boot depends on rather than the probe's transient enable.
        //
        // SHELLDESK — that state is `saved`, not `false`. It was an unconditional `set_enabled(false)`
        // when the bar had no owner; now the desktop shell turns it on at desktop-ready, and a fixture
        // that ended by switching the operator's menu bar off would have taken the bar off the glass
        // for the rest of a metal boot — a witness with a side effect on the thing it witnesses. On a
        // gate boot (no Kepler takeover, nothing enables the bar) `saved` IS `false` and this is the
        // line it always was.
        set_enabled(saved);
        let restored = enabled() == saved;

        match reading {
            Some((pop, px_prot, pop_fault, px_fault)) => {
                let fired = pop > 0 && px_prot > 0;
                let forbid_trips = pop_fault > 0 && px_fault == 0;
                let occ_ok = crossed && fired && forbid_trips && restored;
                serial_println!(
                    ":: MENUBAR-OCC: bar_enabled=true crossed={} occclip_bar={} occclip_bar_px={} \
                     forbid_bar={} forbid_bar_px={} forbid_trips_when_removed={} restored={} :: {} ::",
                    crossed, pop, px_prot, pop_fault, px_fault, forbid_trips, restored,
                    if occ_ok { "PASS" } else { "FAIL" }
                );
            }
            // Unreachable today — [`selftest`] has no aarch64 caller — but it is the honest shape
            // rather than a `0` a capture could not tell from a pass, and it is what this leg becomes
            // the moment a caller exists on that arch. The compose-path `:: MENUBAR-OCC-PAR:` line is
            // the reading a Pi/Orin boot actually gets today.
            None => {
                serial_println!(
                    ":: MENUBAR-OCC: bar_enabled=true crossed={} occclip_bar=absent \
                     occclip_bar_px=absent forbid_bar=absent forbid_bar_px=absent \
                     forbid_trips_when_removed=absent restored={} \
                     blocked_on=wm::occ_bar_probe :: DECLINED ::",
                    crossed, restored
                );
            }
        }
    }

    rollup("selftest");
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// THE MENU PROTOCOL — DESIGN LEDGER. **No implementation in this arc.**
//
// This block is the arc's centre of gravity and the specification the first protocol arc is built
// from. It is recorded here, beside the first renderer, and in `docs/dev/OS/08_VIDEO/engine.md`
// §MENUP. Nothing below is wired; every name is a proposal with its slot named so the next arc mints
// numbers instead of choosing them.
//
// ── THE LAW: NO RENDERER IS PRIVILEGED ───────────────────────────────────────────────────────────
//
// An app publishes a menu TREE. It does not publish a bar, a strip, a popup, or a coordinate. What
// draws the tree is a renderer, and the protocol names none:
//
//   * a desktop shell draws it as this module's strip plus a drop-down;
//   * a spatial/game shell draws it as a radial menu at the cursor, or an in-world panel on a
//     surface, or a wrist HUD;
//   * a headless session enumerates it into a command palette and never draws anything;
//   * an accessibility client speaks it.
//
// The load-bearing consequence: **the tree carries no geometry.** No x, no y, no width, no ordering
// hint expressed in pixels. An item has an identity, a label, a state, and children. A renderer that
// needed a coordinate from the publisher would be a renderer the protocol privileged.
//
// The menu bar is therefore TWO separable things, and both are deletable: a TENANT of
// `super::strip` (this file's top half) and a CLIENT of the protocol (unwritten). Deleting the bar
// touches neither the primitive nor the protocol.
//
// ── THE SHAPE: PUBLISH ON FOCUS, PICK BY IDENTITY ────────────────────────────────────────────────
//
// 1. An app publishes its tree once, when it gains focus or when its menus change. Not per frame,
//    not per open — a menu tree is a slowly-changing declaration, and the renderer caches it.
// 2. The kernel REGISTRY holds at most one live tree per principal, keyed exactly as `wm` keys
//    windows: by `owner_asid`. A second publish replaces the first; a dead principal's tree is
//    reaped where its windows are reaped (`close_owner`).
// 3. A renderer reads the registry for whichever principal it is presenting.
// 4. A pick is delivered to the TREE'S OWNER as an input-like event carrying the item id the
//    publisher itself chose.
//
// ⛔ **Picks are addressed by OWNERSHIP, not by focus, and this is the design's sharpest edge.**
// The existing input path (`user_input_push`, addressed via `user_input_set_active`) delivers to
// whichever slot holds focus. A menu pick must not: a renderer that draws another principal's menu
// (a shell drawing the focused app's menus is exactly that) would otherwise deliver the pick to
// itself. The pick goes to the principal whose tree contains the item. The first protocol arc's
// central fixture is this exact discrimination — see FALSIFICATION below.
//
// ── ABI ADDITIONS, EVERY ONE NAMED WITH ITS SLOT ─────────────────────────────────────────────────
//
// The metal half rides the ALREADY-FROZEN bus rather than minting a new stamping path, and that is a
// security decision, not a convenience: `SYS_MSEND`'s frame carries a 32-byte principal at bytes
// `16..48` that the KERNEL stamps, with a caller-supplied principal rejected `-EINVAL` and never
// overwritten (BANDY-STAMP, docs/SECURITY.md:325). ROADMAP §3b's law — *"every message carries the
// caller's principal … principal-stamping is not retrofittable"* — is thereby satisfied BY
// CONSTRUCTION, with no new code that could get it wrong.
//
// `una-abi/src/lib.rs`, bus verb tags (`BUS_VERB_*`, next free tag is 7):
//
//   BUS_VERB_MENU_PUBLISH = 7   body = an encoded tree; replaces this principal's registry entry.
//   BUS_VERB_MENU_CLEAR   = 8   empty body; drops this principal's entry.
//   BUS_VERB_MENU_GET     = 9   body = the target principal; REPLY body = that principal's tree.
//                               The renderer's read. Reply stamped `PRIN_KERNEL_REPLY` as every
//                               kernel reply already is.
//
// `una-abi/src/lib.rs`, input event types (`INPUT_EV_*`, next free tag is 6, an 8-bit field):
//
//   INPUT_EV_MENU_PICK    = 6   payload[31:0] = the publisher's own item id.
//                               Bit 63 stays clear, so it can never be read as `-errno` —
//                               the invariant `input_ev_pack`'s const-assert already pins.
//
// The wire encoding of a tree, new consts in the same crate (it is ABI: ring 3 writes these bytes):
//
//   MENU_WIRE_VERSION  = 1      a bump is a protocol break — rule on it, never bump it silently,
//                               exactly as `BUS_VERSION` is governed.
//   MENU_LABEL_MAX     = 24     bytes, ASCII, not NUL-terminated — `wm::MAX_TITLE`'s discipline.
//   MENU_DEPTH_MAX     = 2      a menu and its submenus. Deeper is a UI, not a menu, and an
//                               unbounded depth is an unbounded kernel walk.
//   MENU_ITEMS_MAX     = 64     per tree, total across all depths.
//   MENU_FLAG_DISABLED = 1<<0   MENU_FLAG_SEPARATOR = 1<<1
//   MENU_FLAG_CHECKED  = 1<<2   MENU_FLAG_SUBMENU   = 1<<3
//
// One item on the wire is fixed-width: `id: u32`, `parent: u32`, `flags: u32`, `label_len: u8`,
// `label: [u8; MENU_LABEL_MAX]`. Fixed-width because a kernel walking variable-length records from
// ring 3 is a parser, and the whole tree must fit `BUS_BODY_MAX = 4096` — 64 items at 40 bytes is
// 2560, so the caps and the frame agree with room, and the `const` assertion that says so belongs
// beside them.
//
// **Nothing is widened.** `una-abi`'s own rule (lib.rs:243-250): an old ring-3 stub declares unused
// argument registers as clobbers it never writes, so widening a live verb in place would have the
// kernel read junk. New tags only. No new SYSCALL number is needed at all — the menu rides
// `SYS_MSEND`/`SYS_MRECV`, which is why this design adds three verb tags and one event type rather
// than three syscalls.
//
// The HOST half, `libs/bandy/src/signals.rs`, in the UI events block (`:181-217`) beside
// `DockAction`/`NavSelect`:
//
//   SMessage::MenuPublish { principal: String, version: u32, items: Vec<MenuItem> }
//   SMessage::MenuCleared { principal: String }
//   SMessage::MenuQuery   { principal: String }        // late-joining renderer; the
//   SMessage::MenuIs      { principal: String, items: Vec<MenuItem> }   // PrefGet→PrefValueIs idiom
//   SMessage::MenuPick    { principal: String, item_id: u32 }
//
// plus a `MenuItem` serde struct and one golden KAT each in `libs/bandy/tests/smessage_kats.rs` —
// additive on an externally-tagged enum, so existing goldens are untouched.
//
// ⛔ **DISCLOSED GAP, not designed away: the host bus has no principal and no addressing.** `Synapse`
// is one flat tokio broadcast channel; every subscriber sees every message and there is no envelope.
// So `principal: String` on those variants is SELF-ASSERTED and is NOT a security boundary — it is a
// correlation key. On metal the same field is kernel-stamped and IS one. Any statement that the host
// path enforces the ROADMAP's message-security law would be false today, and the first protocol arc
// must either scope itself to metal or land the envelope first. Recorded because ROADMAP §3b says
// stamping is not retrofittable, and this is precisely the seam where retrofitting would be needed.
//
// ── WHO OWNS MENU STATE ──────────────────────────────────────────────────────────────────────────
//
// **The kernel, beside the window table — and NO new handler is invented.** The tree was read for
// this: `handlers/` has 20 crates and NOT ONE of them owns window, shell, or desktop state. `junct`
// is the messaging aggregator, not a window manager; `midden` is the shell's command parser, whose
// `no_std` core the kernel shell already calls. On metal, the thing that already knows which
// principal owns which surface and reaps both when it dies is `video/wm.rs`. A menu registry keyed by
// `owner_asid` is that table's neighbour, sharing its lifetime rules and its reaping — not a new app.
//
// On the host, the honest answer is that nothing owns it yet, and the first arc should NOT mint a
// handler to fix that. `libs/quartzite` is where host window state lives and is the natural renderer;
// the registry stays kernel-side and the host reads it.
//
// ── FALSIFICATION FOR THE FIRST PROTOCOL ARC ─────────────────────────────────────────────────────
//
// Four legs, each able to fail on its own, and the first is the one the whole design turns on:
//
//   1. ⛔ **A pick reaches the TREE'S OWNER, not the focused slot.** Principal A publishes a tree and
//      principal B holds focus. A pick on one of A's items must arrive in A's input ring and B's ring
//      must be EMPTY. A focus-addressed delivery — the obvious implementation, since that is what
//      every existing input event does — fails this leg and passes every other one.
//   2. **The principal is the kernel's, not the caller's.** A `MENU_PUBLISH` frame arriving with a
//      nonzero principal field is rejected `-EINVAL` and the registry is unchanged; BANDY-STAMP's
//      existing equivalence witness extended to the new verb.
//   3. **The caps are refusals, not truncations.** A tree of `MENU_ITEMS_MAX + 1` items, a label of
//      `MENU_LABEL_MAX + 1` bytes, and a depth of `MENU_DEPTH_MAX + 1` are each refused whole. A
//      truncating registry would publish a menu whose items the app did not author, which is worse
//      than no menu.
//   4. **Reaping.** A principal whose windows are closed by `close_owner` has no registry entry
//      afterwards, and a `MENU_GET` for it answers empty rather than a dead tree.
//
// And the renderer-agnosticism claim is falsifiable too, by construction rather than by fixture:
// **the first protocol arc must land with NO renderer at all** — registry, publish, get, pick,
// fixtures — and this module unchanged. If the protocol cannot be proven working without a bar
// drawing it, it was not renderer-agnostic.
// ═════════════════════════════════════════════════════════════════════════════════════════════════
