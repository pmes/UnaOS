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
//! | title, right of crystal | the FOCUSED window's caption, via `wm::dock_scan` | [`theme::TITLE_TEXT_ACTIVE`] |
//! | battery, right of the menus and LEFT of the clock | MENUSTAT — [`super::status::bar_item`], the desktop STATUS MODEL. An outlined cell filled in proportion to the charge, a lightning mark while current flows in, and the percent right-aligned in a fixed slot. ABSENT entirely when the board has no battery source, which is QEMU, the Pi and the Orin — and absent because it was MEASURED absent, not because it was compiled out | outline + percent [`theme::TITLE_TEXT_INACTIVE`] (the clock's ink — the status area is glanced at); fill [`theme::TITLE_TEXT_ACTIVE`]; the bolt is the INVERSE of whatever is under it |
//! | clock, right | [`crate::clock::try_unix_now`], UTC `HH:MM` | [`theme::TITLE_TEXT_INACTIVE`] |
//!
//! **The bar's one press target is the CRYSTAL.** This module still registers nothing with the click
//! router itself; the SHARD menu ([`super::crystal`]) claims the crystal's corner cell
//! ([`crystal_corner_abs`] — FITTS-CORNER, the bar's whole upper-LEFT corner, not just the glyph)
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
//! A small faceted gem at the panel's TOP-LEFT, one [`strip::PAD`] in from the left edge — exactly
//! where macOS puts its apple, which is what the crystal IS (Peter, re-ruling 2026-09-06, orin 17:
//! the crystal is the Mac menu, top-left). **The MARK's inset is not the gap Peter named** — his
//! render9 sentence (2026-09-07) settles which of the two things moves: *"the drop down part of the
//! crystal menu is the only fucking thing that ever needed to move ... yet here it is jammed right up
//! to the edge"*. So the glyph keeps its PAD, exactly as it always had, and only the DROPDOWN goes
//! flush to `x = 0` ([`super::crystal`]'s `menu_rect`). `CRYSTAL_W`x`CRYSTAL_H` (16x22), sized from
//! [`theme::CONTROL_BOX`] so it reads as the same size-family as the window's traffic-light controls.
//! It is drawn from the kit's OWN blue accent ramp — three facets lit from the top-right, the
//! high-contrast seam down the crown reading as a facet edge — reusing three lifted roles `theme.rs`
//! records as having no consumer since the controls went semantic. No palette is invented. It appears
//! only when the bar is enabled (the bar is a default-off tenant; the crystal is part of it). Its GEOMETRY
//! is `crystal=WxH+X+Y`; whether it is DRAWN is read off the PANEL (`gem_px=`/`sym=`, CRYSTAL2 B194).
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
//! or an operator `date -s`). A bar that showed `00:00` or `--:--` for that state would be furniture
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

/// SO2 — **the caption's WEIGHT.** The bar draws its app name bold (macOS's rule, recorded at the
/// `draw_row` call in [`compose_row`]) and its clock regular. It is a `const` rather than a literal
/// at the call site because [`super::winmenu`] now draws with it too: R21's menus are the bar's own
/// text, and Peter's SO2 reading of `render7` — *"misplaced and different font from main app menu
/// item font"* — was exactly this, a window menu set in the bar's face at the bar's cell but NOT at
/// the bar's weight, so the drop-down read as a different typeface from the name it hangs under.
const BOLD: bool = true;

/// SO2 — **THE BAR'S TYPE, exported as one set.** `winmenu` took its face and cell from
/// [`super::crystal`]'s dropdown constants, which happen to resolve to the same atlas — so the two
/// agreed by coincidence, through a third party, and the one attribute that did NOT come along was
/// the weight. These four name the bar's own type so a client draws the BAR'S text by construction,
/// and [`BAR_FONT_NAME`] is what the `[winmenu] open … font=` witness prints, so a capture states
/// which type the drop-down was set in instead of leaving it to be inferred from pixels.
pub const BAR_FACE: super::font::Face = FACE;
/// SO2 — the bar's glyph advance, px. See [`BAR_FACE`].
pub const BAR_CELL_W: usize = CELL_W;
/// SO2 — the bar's glyph cell height, px. See [`BAR_FACE`].
pub const BAR_CELL_H: usize = CELL_H;
/// SO2 — the bar's text weight. See [`BAR_FACE`].
pub const BAR_BOLD: bool = BOLD;
/// SO2 — the bar's type, named for the wire. See [`BAR_FACE`].
pub const BAR_FONT_NAME: &str = "chrome20-bold";

/// The clock's rendered width in glyphs: `HH:MM`.
const CLOCK_GLYPHS: usize = 5;

/// The longest title the bar will show, in glyphs — the whole stored caption. Unlike the dock (which
/// caps at 8 because it draws many tiles across one strip), the bar draws exactly ONE caption and has
/// the panel's full width for it, so there is no reason to truncate below what `wm` stores.
const TITLE_GLYPHS: usize = wm::MAX_TITLE;

// ---------------------------------------------------------------------------
// MENUSTAT — the STATUS AREA, right end, beside the clock.
//
// Peter's direction for this surface is the Mac clone (LAWS §6, R25): the right end of the bar
// carries the status items, and the bar has drawn the UTC clock there since it was written. The
// first item beside it is the BATTERY, and it goes to the clock's LEFT, which is where a Mac puts
// it.
//
// ⛔ **THE FLOORS DO NOT MOVE, AND THAT IS A CONSTRAINT AND NOT AN OMISSION.** [`FLOOR_W`] reserves
// the crystal's slot, one title glyph and the clock. Adding the item's width to it would take the
// BAR off every panel between the old floor and the new one — on machines that may have no battery
// at all — and would move the `floor=` term on `:: MENUBAR:`, which is a menubar line a spec can
// pin. So the item is laid out INSIDE the existing floors by [`batt_slot`], from the same two terms
// the clock is placed by, and is simply not drawn on a panel that cannot seat it beside the clock
// with a title glyph left over. Same terms, same PAD, no new floor.
//
// The crystal's position is R25's and is unreachable from here BY CONSTRUCTION: every number in
// this block is measured from the bar's RIGHT edge inward, and `TITLE_X0`/`CRYSTAL_SLOT` appear in
// [`batt_slot`] only as the floor the item must not cross.
// ---------------------------------------------------------------------------

/// The battery cell's body width, px — [`theme::CONTROL_BOX`], so the item reads as the same
/// size-family as the crystal and the window controls. No new metric is invented; the kit's one
/// control dimension is the source for all three.
const BATT_BODY_W: usize = theme::CONTROL_BOX;
/// The cell's body height, px — half the control box. A battery reads WIDER than tall, which is
/// [`CRYSTAL_W`]'s proportion rule applied to a lying-down shape.
const BATT_BODY_H: usize = theme::CONTROL_BOX / 2;
/// The positive terminal's nub, px — the columns past the body's right edge that make the outline
/// read as a battery rather than as a progress bar.
const BATT_CAP_W: usize = 2;
/// The nub's height, px — a third of the body, centred on it.
const BATT_CAP_H: usize = BATT_BODY_H / 3;
/// The whole drawn glyph's width, px: body plus nub.
const BATT_GLYPH_W: usize = BATT_BODY_W + BATT_CAP_W;
/// The percent text's slot, in glyphs — `100%` is the longest string it can hold, and the text is
/// RIGHT-ALIGNED inside it (`  5%`, ` 82%`, `100%`). The fixed slot is why the glyph's x never moves
/// when the charge crosses 10 % or 100 %: a status item that shifted its own parts every time a
/// digit appeared would be MENUOWN's variable gap, one surface over.
const BATT_PCT_GLYPHS: usize = 4;
/// The gap between the glyph and its percent, px — half a [`strip::PAD`]. They are ONE item, so the
/// space inside it must read as smaller than the PAD separating the item from the clock.
const BATT_GAP: usize = strip::PAD / 2;
/// The whole item's width, px. The number the layout is built from and the number the ledger records.
const BATT_ITEM_W: usize = BATT_GLYPH_W + BATT_GAP + BATT_PCT_GLYPHS * CELL_W;

/// The charging mark's box, px.
const BOLT_W: usize = 5;
/// See [`BOLT_W`].
const BOLT_H: usize = 8;
/// The charging mark — a lightning bolt, 5x8, MSB leftmost. Drawn as the INVERSE of whatever is
/// under it (see [`draw_battery_glyph`]), which is the one rule that keeps it legible at 3 % and at
/// 97 % without inventing a third ink for it.
const BOLT: [u8; BOLT_H] = [
    0b00011, //
    0b00110, //
    0b01100, //
    0b11111, //
    0b00110, //
    0b01100, //
    0b11000, //
    0b10000, //
];

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

/// **The crystal's LEFT SLOT**, and the whole of the geometry rule this constant carries.
///
/// Peter, re-ruling 2026-09-06 (orin 17): *the crystal is the Mac menu, top-left.* The mark's home is
/// the panel's upper-LEFT corner, always — it is UnaOS's apple, and an apple menu that migrates is
/// not an apple menu.
///
/// **Which of the two things carries the inset is settled, and it is the GLYPH** (Peter, render9,
/// 2026-09-07): *"the drop down part of the crystal menu is the only fucking thing that ever needed
/// to move. how i never once said to move the fucking crystal yet here it is jammed right up to the
/// edge."* Three readings are on the record and only the last is the rule; the first two are kept so
/// the next reader does not re-derive either:
///
/// 1. render7's layout — glyph at one [`strip::PAD`], dropdown INHERITING that inset at
///    `menu=170x121+12+34`. The 12 px Peter named was the dropdown's, not the glyph's.
/// 2. `bb513370` MISREAD "close the gap" as "move to the other edge" and sent the whole group flush
///    right (rejected on render8, re-ruled R25).
/// 3. `1046f81c` then closed the gap by moving BOTH to `x = 0` — which took the glyph off its inset
///    as collateral. Rejected on render9: the glyph was never the thing with the gap.
///
/// So the glyph keeps its ONE outside PAD (state 1's mark, unchanged since it was first drawn) and
/// the DROPDOWN alone goes flush — it is decoupled from the glyph in [`super::crystal`]'s
/// `menu_rect` rather than derived from it, which is what makes the two positions independently
/// settable at all. This slot is therefore the glyph's outside PAD, the glyph, and one PAD of
/// clearance on its INNER (right) side separating the mark from the caption beside it.
const CRYSTAL_SLOT: usize = strip::PAD + CRYSTAL_W + strip::PAD;

/// The title's left inset: past the crystal's whole slot, which is [`CRYSTAL_SLOT`] — the glyph's
/// outside PAD, the glyph, and its one PAD of inner clearance. macOS puts the apple leftmost and the
/// app menus to its right; the caption takes that same slot here.
const TITLE_X0: usize = CRYSTAL_SLOT;

/// WINMENU (R21) — **where the window's menus begin WHEN THE BAR NAMES NO APP.**
///
/// MENUOWN (Peter, 2026-09-07: *"spaced incorrectly"*). This was a FIXED COLUMN —
/// `TITLE_X0 + (MAX_TITLE + 1) * CELL_W`, which on the bench's panel is 181 px — and the menus were
/// laid out there whatever the caption's rendered width. `render9` measured the consequence: `View`
/// began at x=187 under `console` (ink ending at 89) and at x=187 under `quarry` (ink ending at 81),
/// so the visible gap between the app name and its first menu was 97 px in one frame and 105 in the
/// other. A gap that changes size with the length of the word before it is the definition of
/// spaced wrong, and macOS has no such property: an app's menus sit one fixed gap after its name.
///
/// **The argument that produced the fixed column was sound, and was answered elsewhere.** It read:
/// *"if the menu titles began after the caption's RENDERED width they would slide left and right
/// every time the focused window changed — and a press would then be judged against a layout the
/// operator was not looking at when they aimed"*. But the titles ALREADY move under the operator on
/// every focus change, because which titles exist at all is a property of the focused window; a
/// stationary column bought no stability, it only bought a variable gap. The press/paint agreement
/// the argument actually wanted is a different property, and `bar_boxes` has it by construction:
/// one layout feeds the painter, the hit test and the dropdown anchor.
///
/// So the menus now follow the APP TITLE BOX ([`super::winmenu::bar_boxes`]), and this constant is
/// what is left of the old one: the anchor for a bar with no caption to follow. It is [`TITLE_X0`] —
/// where the name WOULD have been — so a window with menus and no name puts them where a named
/// window's name starts, rather than 153 px into empty chrome.
const MENUS_X0: usize = TITLE_X0;

/// The panel height below which the bar declines.
///
/// DERIVED, and stated: the bar must not crowd the dock off the panel. The dock's own floor is
/// `STRIP_H + 2*PAD` (its [`super::dock::STRIP_H`] plus a margin above and below), and the bar takes
/// `BAR_H` off the top before the dock ever gets to lay out. So a panel that can host both is at
/// least their sum, and one that cannot host both hosts the DOCK — the dock is the console's only way
/// back and the bar is a convenience, so the bar is the one that yields.
const FLOOR_H: usize = BAR_H + super::dock::STRIP_H + 2 * strip::PAD;

/// The panel width below which the bar declines: the insets, the clock, the crystal's slot, and at
/// least one glyph of title. Below it the bar would be a strip with nothing legible on it.
///
/// The `+ CRYSTAL_SLOT` term is the brand mark's own left slot: the crystal pushes the title inset
/// right, so the floor that guarantees "crystal, one title glyph, and the clock all fit" grows by the
/// slot. The slot already carries the glyph's outside PAD (render9's ruling), so the two PADs written
/// separately here are the title-to-clock gap and the clock's own right inset. Still far below every
/// suite panel, so no gate declines — [`FLOOR_W`] is asserted `<= 640` below, which is the claim that
/// matters.
const FLOOR_W: usize = 2 * strip::PAD + CRYSTAL_SLOT + (CLOCK_GLYPHS + 1) * CELL_W;

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
    // owns `[PAD, PAD + CRYSTAL_W)` — one PAD in from the left, render9's ruling — and the clock
    // `[FLOOR_W - PAD - CLOCK_GLYPHS*CELL_W, FLOOR_W - PAD)`; this is the gap between them staying
    // positive, so a future metric change that would overlap them fails the BUILD rather than painting
    // the gem over the time.
    assert!(strip::PAD + CRYSTAL_W < FLOOR_W - strip::PAD - CLOCK_GLYPHS * CELL_W);
    // The title, shifted past the crystal's slot, must still leave room for at least one glyph before
    // the clock on the floor panel.
    assert!(TITLE_X0 + CELL_W <= FLOOR_W - strip::PAD - CLOCK_GLYPHS * CELL_W);
    // FITTS-CORNER: the press cell (`crystal_corner_abs`, CRYSTAL_SLOT wide by the bar's height, at the
    // bar's LEFT end) must CONTAIN the painted glyph box, or a press on the visible mark could miss its
    // own menu. The horizontal half is the load-bearing one; vertical containment is the bevel assert.
    // The cell starts at 0 and the glyph at PAD, so the glyph's RIGHT edge is what must fit.
    assert!(strip::PAD + CRYSTAL_W <= CRYSTAL_SLOT);

    // MENUSTAT — the status item's silhouette must be drawable inside the bar it sits in, and the
    // charging mark inside the cell it sits in. These are build-time because they are statements
    // about CONSTANTS: a metric change that would draw the cell through the bar's keyline, or the
    // bolt through the cell's outline, fails the build rather than painting it once on the glass.
    assert!(BATT_BODY_H + 2 * theme::BEVEL <= BAR_H);
    assert!(BATT_BODY_H >= 4); // an outline, a fill row and an outline
    assert!(BATT_CAP_H > 0 && BATT_CAP_H < BATT_BODY_H);
    // The bolt lives INSIDE the 1-px outline on all four sides, so it can never erase the cell's
    // own edge — the falsifier for the "inverse of whatever is under it" rule, which is only safe
    // while "under it" is fill or face and never outline.
    assert!(BOLT_H + 2 <= BATT_BODY_H);
    assert!(BOLT_W + 2 <= BATT_BODY_W);
    // `100%` must fit the slot the percent is right-aligned in, or a full pack would be truncated.
    assert!(BATT_PCT_GLYPHS >= 4);
    // ⛔ THE FLOORS ARE NOT WIDENED: the item is NOT part of `FLOOR_W`. This asserts the consequence
    // that matters — on the floor panel the bar still seats the crystal, a title glyph and the clock
    // WITHOUT the item, which is the state `batt_slot` declines into rather than shrinking anything.
    // If a future edit folds `BATT_ITEM_W` into `FLOOR_W`, this assert stops meaning what it says and
    // the reader is sent to `batt_slot` to find out which rule changed.
    assert!(FLOOR_W == 2 * strip::PAD + CRYSTAL_SLOT + (CLOCK_GLYPHS + 1) * CELL_W);
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
        // MENUFIRST — stamp the FIRST off→on edge, so `after_enable_ms` is a measurement and not the
        // seam's opinion of its own control flow. `compare_exchange` on the never-written state: only
        // the first edge wins, so the fixture's four later toggles cannot rewrite the shell's. One
        // counter read; see [`ENABLED_AT_CYC`] for why it is cycles and not ms.
        if on {
            let now = crate::arch::now_cycles();
            let _ = ENABLED_AT_CYC.compare_exchange(CYC_NONE, now, Ordering::AcqRel, Ordering::Relaxed);
        }
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
    /// WINMENU (R21) — **the window whose menus this bar is showing**, or [`wm::WIN_NONE`].
    ///
    /// MENUOWN (Peter, 2026-09-07): **it is the row the CAPTION names**, when that row has published
    /// a tree. The bar shows ONE app's menus and names ONE app, and on a Mac those are the same app;
    /// a bar that names `console` over `pulse`'s `View` is showing two apps at once.
    ///
    /// ⚠ **It used to be "the frontmost VISIBLE PUBLISHER", which ignored focus entirely, and the
    /// premise that justified that is FALSE.** The claim recorded here was: *"the caption's `focused`
    /// flag is an OWNER-ASID match, and the click router hands SHELL focus (asid `0`) to a press on
    /// kernel furniture (`is_kernel_owner`) — so [`super::pulsewin`] can never be `focused` by that
    /// test"*. It conflates the two things the router's furniture arm does on ONE line
    /// (`arch/aarch64/syscall.rs`, the SHELLWIN-PI arm): `user_input_set_active(0)` hands the
    /// KEYBOARD to the shell, and `wm::focus_changed(owner)` — called with the furniture's OWN owner
    /// — sets `FOCUS_ASID`, which is what `dock_scan`'s `focused` reads. Kernel furniture therefore
    /// DOES take focus by that test, and `render9` proves it from the glass: the caption read
    /// `console` in one frame and `quarry` in the next, which is only reachable if `FOCUS_ASID` had
    /// become `KERNEL_OWNER_CONSOLE` and then quarry's `OWNER`. Under the old reduction that same
    /// pair of frames carried pulse's `View` in the bar the whole time, because pulse was the only
    /// publisher and "frontmost publisher" cannot see a window that is in front of it without menus.
    ///
    /// Keeping the two reductions apart cost exactly that defect, so they are now ONE reading:
    /// [`cap_owner`](Self::cap_owner), filtered by whether it publishes.
    menu_owner: wm::WinId,
    /// SO3 — **the window the CAPTION names**, i.e. the row [`title`](Self::title) was taken from:
    /// the frontmost FOCUSED VISIBLE row. The app menu belongs to the name the operator is reading,
    /// so `Quit` reaps THIS row.
    ///
    /// MENUOWN — it is now also what [`menu_owner`](Self::menu_owner) is derived from, so the bar
    /// cannot name one app and drop another's menus. The two fields remain separate because the
    /// second is this one FILTERED by `winmenu::has_tree`: a focused window with no menus of its own
    /// still has a name, and still gets its `Quit`.
    cap_owner: wm::WinId,
    /// WINMENU — the title boxes, laid out once per compose and handed to the row painter. Filled in
    /// by [`compose`] after the rect is settled, because the layout is a function of the bar rect.
    menus: super::winmenu::BarSnapshot,
    /// MENUSTAT — **the battery status item**, or `None` when this board has no battery source
    /// (QEMU, the Pi, the Orin) or the held reading has gone stale. Taken from [`super::status`],
    /// which is the whole reason this field is not a driver call: `drivers::smc` is x86-only and
    /// this file is compiled on the Pi.
    ///
    /// ⛔ **It is [`super::status::BarItem`] and not [`super::status::Battery`], and the difference
    /// is the repaint discipline.** Only what is DRAWN — the percent and the charge state — is in
    /// the model, so only what is drawn is in [`signature`](Self::signature). The minutes, the
    /// current, the voltage and the age are on the wire and nowhere near the damage test, which is
    /// what keeps the bar's `paints=` flat while the meter ticks at 10 s.
    batt: Option<super::status::BarItem>,
}

impl Model {
    fn empty() -> Model {
        Model {
            title: [0; wm::MAX_TITLE],
            title_len: 0,
            clock: None,
            menu_owner: wm::WIN_NONE,
            cap_owner: wm::WIN_NONE,
            menus: super::winmenu::BarSnapshot::empty(),
            batt: None,
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
        // MENUOWN — ONE reduction, not two. The frontmost FOCUSED VISIBLE row is the app the bar
        // names, and it is now also the app whose menus the bar shows: `menu_owner` is taken from
        // `cap_owner` below rather than computed here from a separate "frontmost publisher" walk.
        // See [`Model::menu_owner`] for the premise that walk rested on and why it is false.
        //
        // PANEL V-4 — `wm::info` is LAZY, below the guard. It takes `wm::TABLE` with IRQs masked
        // (`wm::table`, and `wm.rs`'s standing rule about that critical section), so hoisting it
        // above the guard turned one masked acquisition per bar compose into up to `MAX_WINDOWS` of
        // them, on every composite, on every gated build. The guard is now STRICTLY tighter than it
        // was — the old `publisher ||` arm let every published row past it — so this reduction takes
        // no more of those acquisitions than before, and on a desktop with a background publisher it
        // takes fewer.
        for r in rows[..n].iter() {
            if !r.focused || !r.visible {
                continue;
            }
            let z = wm::info(r.id).map(|i| i.z).unwrap_or(0);
            if z >= best_z {
                best_z = z;
                m.title_len = r.title_len.min(wm::MAX_TITLE);
                m.title = r.title;
                m.cap_owner = r.id; // SO3 — the app menu's owner is the row the caption came from
            }
        }
        // MENUOWN — the bar shows the FOCUSED app's menus, and no one else's. `winmenu::has_tree` is
        // lock-free (`WINMENU_MAX` relaxed loads, short-circuited to nothing when nothing has
        // published), and it is asked ONCE here rather than once per row, so this is cheaper than
        // the walk it replaces as well as correct. A focused window that published nothing answers
        // `WIN_NONE`: the bar keeps its app-menu box (SO3 gives every window a name-menu) and lays
        // out no tenant titles, which is exactly what a Mac shows for an app with no menus of its
        // own — and is what `render9` should have shown while `console` and `quarry` held focus.
        m.menu_owner = if super::winmenu::has_tree(m.cap_owner) { m.cap_owner } else { wm::WIN_NONE };
        // MENUSTAT — the battery item, gathered with the rest of the model and from the same rule:
        // everything the painter reads is read ONCE, here. It is three relaxed atomic loads and a
        // clock read (`super::status::bar_item`) — no lock, no port, no allocation — which is the
        // only shape allowed at this seam: `compose` runs masked inside `strip::compose_all`, and
        // the SMC transaction that produced this number ran ten seconds ago on the device-service
        // task (`super::status::poll`). A bar that read the SMC here would wait on six bounded
        // handshakes with interrupts off.
        m.batt = super::status::bar_item();
        m.clock = clock_hhmm(); #[cfg(feature = "sntp6")] if m.clock.is_none() { barclock_note(None); } // SNTP-NET6, folded LINE-NEUTRAL (code before the comment, LEDGER P7): the UNSYNCED half of the bar's clock witness, latched to one line per boot at the file tail. It is reported from the MODEL, not the painter, because the painter's clock branch never runs when there is nothing to draw — which is precisely the state this half exists to say aloud.
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
        // MENUSTAT — **the status item's DRAWN state, and nothing else.** This is the whole of the
        // repaint discipline the brief asked to be measured, and it is enforced by what the model
        // carries rather than by care here: `batt` is a `BarItem`, so the only things that CAN be
        // folded are the percent and the charge state. The minutes, the current, the voltage and
        // the age are not in this struct, so a 10 s poll that moves them cannot reach this hash and
        // cannot cost a repaint — while a percent tick or a bolt appearing must, because the glass
        // would otherwise be wrong. (The fixture's `jitter_paint`/`change_paint` legs measure both
        // halves against `compose`'s own return value.)
        match self.batt {
            Some(b) => {
                h = strip::fnv1a(h, 1);
                h = strip::fnv1a(h, b.percent as u8);
                h = strip::fnv1a(h, b.charging as u8);
            }
            None => h = strip::fnv1a(h, 0),
        }
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
    // `date -s`) yields `None`, the bar draws no clock this pass and repaints on the next — the same
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
            // TEARSCOPE — `strip::vacate` IS `strip::erase_rect` plus the census; same return, same
            // behaviour. `new=None` (the bar is going away, so all of `r` is uncovered) and
            // `owed=false` (the slot is already cleared, so a decline is forgotten).
            return strip::vacate("menubar", r, None, false);
        }
        return false;
    }
    let t0 = crate::arch::now_cycles();
    let (pw, ph) = {
        // LOCKFIX B1 — WAS `let fb = *super::WRITER.lock();` plus a separate `is_ready` arm; the `dock::compose` twin verbatim, and the same reason: this is the menubar's PRESENT TAIL, it runs MASKED inside `wcg`'s composite chain, and the WEDGE-8 rule forbids waiting on a lock from a context that cannot be preempted. `panel_snapshot` is LOCKFIX's PAINT-path door — still BLOCKING when interrupts are enabled, so the uncontended path is unchanged — and `.filter(is_ready)` folds the old readiness arm into the same `else`, so a held panel declines exactly the way an unready surface always has (ledger the pass, return `false`; the bar's signature stays unmatched and the next composite repaints it). No counter, no new print: LAWS §1(e). ⚠ LINE-NEUTRAL: 5 lines out, 5 in.
        let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) else {
            LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
            return false;
        };
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
    // SO3 (Peter's ruling) — publish the CAPTION and the row it names, so the bar's app title is a
    // menu title like any other. Lock-free stores only; like `set_bar_owner` above this runs INSIDE
    // `strip::compose_all`, so an owner change here clears menu STATE and never drives a composite
    // (PANEL V-1's rule, and `winmenu::compose` discharges the erase later in this same pass).
    super::winmenu::set_app_window(model.cap_owner, &model.title[..model.title_len]);
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
    // MENUOWN — **WHOSE menus the bar is showing, and WHERE it put them.** See [`MENUROW_KEY`].
    menurow_witness(&model); #[cfg(feature = "witness")] persist_model(&mut model); // CRYSTAL2 (B194) — `crystal_persist_selftest`'s synthetic model, AFTER the menu row is announced and BEFORE the signature, so the bar's real damage test decides the repaint. Inert (one relaxed load) unless that fixture is running. ⚠ SAME-LINE fold, line-neutral (B94).
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
    // MENUSTAT — the status area's own line, beside the cost ledger's and on its cadence. A SIBLING
    // line rather than terms appended to the one above, for the reason PTRINSTALL records at
    // `main.rs`: the ledger's tail is the tenant's COST vocabulary and the status item is not a
    // cost, and a reader grepping `[menubar] battery` should get one line per interval with only
    // the battery on it.
    battery_witness();

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
            // TEARSCOPE — accounted, not changed. `owed=false`: the slot is cleared above.
            Some(v) => strip::vacate("menubar", v, None, false),
            None => false,
        };
    };

    let t1 = crate::arch::now_cycles();
    if let Some(v) = vacated {
        // Erase FIRST, then paint, so the two never race to own an overlapping pixel.
        //
        // TEARSCOPE — accounted, not changed. `owed=false` because `SLOT.store` below re-publishes
        // this tenant's rect whatever the erase returned. The bar is `frame_flush(Top)` and so is
        // full-panel-width, which means a content change alone does not move it; this arm is reached
        // when the RECT itself changes (a panel resize, or the bar crossing its floor).
        strip::vacate("menubar", v, Some(r), false);
    }
    if !strip::paint("menubar", r, |out, j| compose_row(out, &model, r, j)) {
        return false;
    }
    LEDGER.paint(crate::arch::now_cycles().saturating_sub(t1), (r.2 * r.3) as u64);
    SLOT.store(sig, Some(r));
    // MENUFIRST — the FIRST paint, once per boot, read off the paint that just landed. Sited AFTER
    // `SLOT.store` so the line can never describe a paint `strip::paint` declined (the `return false`
    // two statements up is the decline arm, and it is above this). One-shot inside the witness.
    #[cfg(feature = "witness")]
    firstpaint_witness(&model, r);
    true
}

/// MENUOWN — the last item row this bar announced, as [`super::winmenu::BarSnapshot::owner_key`].
/// `0` is "never announced", so the FIRST composed row always speaks: a boot whose bar never says
/// whose menus it is showing must be distinguishable from one whose row never changed.
static MENUROW_KEY: AtomicU64 = AtomicU64::new(0);

/// MENUOWN — **the bar's ownership and layout, ON THE WIRE, on change.**
///
/// Peter, at the bench on `render9` (2026-09-07): *"pulse view menu item still showing across all
/// apps and spaced incorrectly"*. Both halves of that reading were GLASS-ONLY facts. The bar's
/// existing instruments could not have caught either: `[winmenu] publish owner=` fires once when a
/// tenant registers and never again, the `[menubar]` cost ledger carries `press=`/`clob=`/`toggles=`
/// and no layout at all, and `bar_owner=` on the `[winmenu]` rollup is a bare id with nothing to
/// compare it to. So a flight could show `View` sitting under the word `console` at a column the
/// caption never moved, and every line on the wire read green. This is the line that makes both
/// falsifiable:
///
///  * `cap_owner=`/`cap=` — the app the bar NAMES. `menu_owner=` — the app whose menus it SHOWS.
///    **They must be the same window**, and a capture where they differ is defect one, stated.
///  * `items=` — every box's label at its panel-absolute column. A tenant title whose `x` does not
///    move when `cap=` changes length is defect two, stated.
///
/// UNGATED, deliberately. The metal image is built without `witness` and Peter's captures come off
/// metal; an instrument that is absent from the artifact he flies is not an instrument. It is
/// affordable because it is EDGE-TRIGGERED on [`MENUROW_KEY`] — one relaxed load per composite in
/// the steady state, and a line only when the row genuinely moved.
fn menurow_witness(m: &Model) {
    let key = m.menus.owner_key(m.cap_owner);
    if MENUROW_KEY.swap(key, Ordering::Relaxed) == key {
        return;
    }
    serial_println!(
        "[menubar] menus cap_owner={} cap={} menu_owner={} boxes={} items={}",
        m.cap_owner,
        core::str::from_utf8(&m.title[..m.title_len]).unwrap_or("?"),
        m.menu_owner,
        m.menus.n,
        m.menus.items()
    );
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// MENUSTAT — THE STATUS AREA ON THE WIRE
// ═════════════════════════════════════════════════════════════════════════════════════════════════

/// The cycle stamp of the last `[menubar] battery` line, `0` = never.
static BATT_WIRE_LAST: AtomicU64 = AtomicU64::new(0);
/// Whether the ABSENT statement has been made. Once a boot: "this machine has no battery" is a
/// fixed fact, and a fixed fact repeated every five seconds is the SO30 defect that ate 36 % of a
/// boot's wire.
static BATT_ABSENT_SAID: AtomicBool = AtomicBool::new(false);
/// The status line's period. Five seconds, which is [`strip::Ledger`]'s `ROLLUP_PERIOD_US` and
/// `wm`'s `WCN_ROLLUP_MS` — restated here because that constant is private to `strip` and `strip`
/// is not this lane's file. It is the same number for the same reason: a capture should carry the
/// bar's cost line and the bar's status line in one interval so they can be read side by side.
const BATT_WIRE_US: u64 = 5_000_000;

/// `mins=` — the minutes-to-full field, or `-` when the SMC offers no estimate (which it does not
/// while discharging). A `Display` adapter rather than two near-identical `serial_println!` arms,
/// so the line's format string exists exactly once and the two states cannot drift apart.
struct Mins(Option<u16>);

impl core::fmt::Display for Mins {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(m) => write!(f, "{}", m),
            None => f.write_str("-"),
        }
    }
}

/// MENUSTAT — **the status area's reading, ON THE WIRE.**
///
/// UNGATED, for `menurow_witness`'s reason and it is the same reason: the metal image is built
/// WITHOUT `witness`, Peter's captures come off metal, and an instrument absent from the artifact he
/// flies is not an instrument. It is affordable because it is rate-limited to one line per
/// [`BATT_WIRE_US`] — one relaxed load per composite in the steady state.
///
/// Two shapes, and the second is the one a gate boot prints:
///
///  * `[menubar] battery pct=82 charging=y mins=106 age_s=3 src=smc mv=12311 ma=1014 polls=4/4` —
///    a live reading. `age_s` is how long ago the poll that produced it landed, which is the
///    falsifier for the BATMON-HOLD rule at this layer: a held reading is visible AS held instead of
///    looking fresh. `polls=answers/attempts` separates "the service never ran" from "it ran and the
///    SMC said nothing".
///  * `[menubar] battery absent src=none` — ONCE. This is what QEMU prints, and printing it is the
///    point: `isa-applesmc` answers `REV`/`OSK0` and carries no battery key, so the absence is
///    MEASURED and says so, rather than the item being compiled out and the boot being silent about
///    a question it did answer.
///
/// An `Unresolved` source says NOTHING. The desktop service has not swept yet, and a line claiming
/// absence before anything asked would be the same defect in the other direction.
fn battery_witness() {
    match super::status::reading() {
        Some((b, age_ms)) => {
            let now = crate::arch::now_cycles();
            let last = BATT_WIRE_LAST.load(Ordering::Relaxed);
            if last != 0 && strip::cycles_to_us(now.saturating_sub(last)) < BATT_WIRE_US {
                return;
            }
            // The same `compare_exchange` `Ledger::tick` uses, for its reason: two cores must not
            // print one interval twice, and a loser simply skips.
            if BATT_WIRE_LAST
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
            {
                return;
            }
            let (polls, answers) = super::status::counts();
            serial_println!(
                "[menubar] battery pct={} charging={} mins={} age_s={} src={} mv={} ma={} polls={}/{}",
                b.percent,
                if b.charging { "y" } else { "n" },
                Mins(b.minutes),
                age_ms / 1000,
                super::status::source().as_str(),
                b.mv,
                b.ma,
                answers,
                polls
            );
        }
        None => {
            let src = super::status::source();
            if src == super::status::Source::Unresolved {
                return; // nothing has asked yet — see the doc comment
            }
            if BATT_ABSENT_SAID.swap(true, Ordering::Relaxed) {
                return;
            }
            // `src=none` on a board with no battery source, which is every gate boot. It can also
            // read `src=smc` — a source that answered and has since gone past `status::STALE_MS`,
            // i.e. a pack removed or an SMC that stopped answering for a minute. Both are "the item
            // is not on the glass", and the term says which.
            serial_println!("[menubar] battery absent src={}", src.as_str());
        }
    }
}

// ---------------------------------------------------------------------------
// MENUFIRST — the bar's FIRST PAINT, on the wire
// ---------------------------------------------------------------------------

/// MENUFIRST — the CYCLE stamp of the first off→on edge, or [`CYC_NONE`] while the bar has never
/// been on.
///
/// Cycles and not milliseconds, and that is forced rather than preferred: [`crate::clock`]'s
/// `logts_now` — the obvious source, since it is what stamps the `[ NNNNNms]` line prefix — is
/// `#[cfg(feature = "logts")]`, and the x86 `wc` gate builds without `logts`. Reaching for it put an
/// `E0425` in front of this arc's first gate run. `arch::now_cycles()` is what this file already
/// times its ledger with, [`strip::cycles_to_us`] is `pub` and is the SAME conversion the strip's own
/// `age_ms=` term uses, and neither takes a lock — which also makes this strictly better than
/// `logts_now` on a path that runs masked at the composite tail, since that function's civil half
/// does a `try_lock` this reading has no use for.
///
/// Latched by `compare_exchange` on the never-written state, so the FIXTURE's toggles (legs 3-4 and
/// [`battery_selftest`] — five on flight 11's wire, `toggles=5`) cannot rewrite the shell's edge, and
/// re-armable for the fixture alone through [`firstpaint_rearm`].
static ENABLED_AT_CYC: AtomicU64 = AtomicU64::new(CYC_NONE);

/// MENUFIRST — "the bar has never been enabled". `u64::MAX` rather than `0`, because a counter
/// reading of `0` is reachable at the very first tick and a sentinel a real measurement can collide
/// with is not a sentinel; `u64::MAX` cycles is not reachable on any machine this boots on.
const CYC_NONE: u64 = u64::MAX;

/// MENUFIRST — **the SPEAK one-shot: one line per boot (SO30), and NOTHING EVER RESETS IT.**
///
/// It is deliberately not part of what [`firstpaint_rearm`] takes and [`firstpaint_restore`] puts
/// back. The fixture re-arms the RECORDER so it can measure an edge of its own; if it could re-arm
/// this too, a later genuine composite would print a second `[menubar] first-paint` — and on the
/// board where that matters (metal, where the shell's seam spoke twenty seconds earlier) the second
/// line would carry the fixture's synthetic numbers under the boot's name. Keeping the speak latch
/// out of the fixture's reach is what makes "once per boot" true by construction instead of by the
/// fixture remembering to restore it.
static FIRSTPAINT_SAID: AtomicBool = AtomicBool::new(false);

/// MENUFIRST — the reading the witness published, for the fixture to read BACK rather than recompute:
/// `after_enable_ms` in the low 32 bits, the completeness mask in bits 32-34, `crystal=drawn` in bit
/// 35, and bit 63 set once anything has been recorded. Reading it back is what stops the fixture's
/// bound leg re-deriving the very claim it exists to check.
static FIRSTPAINT_READING: AtomicU64 = AtomicU64::new(0);

/// MENUFIRST — bit 63 of [`FIRSTPAINT_READING`]: "a first paint has been recorded this boot".
const FP_VALID: u64 = 1 << 63;

/// MENUFIRST — **one composite pass, in ms**: the bound the fixture holds `after_enable_ms` to.
///
/// `16_667` is the frame the compositor paces against, and it is on the wire on every `[strip]
/// rollup` line of flight 11 as `frame_us=16667`. The bound is TWO of them, not one: the pass the
/// enable lands in and the pass that paints it — `desktop_uefi::activate` calls `wm::composite()` on
/// the line after `set_enabled(true)`, so one frame of slack is structural and a third would be the
/// bar being late. **33 ms**, by integer division — `2 * 16_667 / 1000` truncates 33.334 to 33, and
/// the truncation is in the SAFE direction (a tighter bound cannot hide a late paint). Flight 11
/// spent **1** of them (`[27616ms] menubar ENABLED` → `[27617ms] [strip] rollup tenant=menubar …
/// paints=1`).
///
/// ⚠ The `16_667` is a THIRD copy of `wcg`'s original (`strip.rs` already carries the second, with
/// the same note): both are private to their modules, so this is a restatement, not a re-derivation,
/// and it is only ever compared against — it paces nothing.
const ONE_COMPOSITE_MS: u64 = 2 * 16_667 / 1000;

/// MENUFIRST — **the go-red's injected lateness, and it is the brief's own number.**
///
/// 5051 ms is the gap this arc was sent to close, read off two `[menubar] live` rollups — 27616 ms
/// `paints=0` and 32667 ms `paints=1` — that are the SAME PAINT reported twice on the ledger's ~5 s
/// cadence. It is named here rather than typed into [`firstpaint_selftest`] so the fixture's
/// injection and this explanation cannot drift apart.
#[cfg(feature = "witness")]
const RED_INJECT_MS: u64 = 5051;

/// MENUFIRST — a boot-time reading in ms, or `?` when the counter has no origin yet.
///
/// The `?` is [`crate::logts`]'s own discipline, imported deliberately: a boot with no trustworthy
/// counter must not be made indistinguishable from an instantaneous one by a fabricated `0`.
struct Ms(Option<u64>);

impl core::fmt::Display for Ms {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{}", v),
            None => write!(f, "?"),
        }
    }
}

/// MENUFIRST — **ms since the `entry` stamp**, or `None` before that stamp exists.
///
/// The origin is [`crate::bootpace::origin_cycles`], which is the SAME origin `logts` subtracts for
/// the `[ NNNNNms]` line prefix — its doc comment states the rule this obeys: *an out-of-module
/// timestamp must subtract the LEDGER's origin, not invent its own*, because the raw x86 TSC counts
/// from processor RESET and an unsubtracted reading disagrees with every BPACE/GPACE `t=` by the
/// firmware duration. So on any build that carries `logts` — which is every flown image — `at=` is
/// directly comparable to the prefix on its own line, and on the `logts`-less gate it is still the
/// same origin every other pace figure uses.
///
/// `origin == 0` is "no `entry` stamp yet", and it answers `None` rather than `0` for the reason
/// [`Ms`] states. Lock-free: one counter read, one relaxed load, one multiply.
#[inline]
fn now_ms() -> Option<u64> {
    let origin = crate::bootpace::origin_cycles();
    if origin == 0 {
        return None; // no `entry` stamp — "since entry" does not exist to measure
    }
    Some(strip::cycles_to_us(crate::arch::now_cycles().saturating_sub(origin)) / 1000)
}

/// MENUFIRST — **is the model COMPLETE, and if not, what is missing?**
///
/// Returns `(mask, what)`, one bit per element the bar draws from a source that can be UN-ASKED at
/// the enable seam, named in draw order. `0` is `complete`.
///
/// ⛔ **The crystal is not in this mask, and that is the finding.** [`crystal_facet`] and
/// [`crystal_half`] read nothing but `const` geometry and three `const` inks (`theme.rs:198,201,204`
/// — `CONTROL_CLOSE`/`CONTROL_MID`/`CONTROL_ZOOM` are `pub const u32`), and `ceramic::shade` is a
/// `spin::Once` table generated on first use. There is no theme, face or kit the mark waits on, so
/// the mark cannot be half-drawn by TIMING: at the enable seam it is the same 16x22 gem it is at
/// minute ten. What flight 11 put on the glass was a COMPLETE crystal on an EMPTY bar, and the three
/// bits below are the emptiness.
///
/// * **caption** — `cap_owner == WIN_NONE`: no focused visible row, so no app name and no app menu.
///   Flight 11 at the seam: `[27616ms] [menubar] menus cap_owner=0 cap= menu_owner=0 boxes=0
///   items=none`. The row that would have filled it was `/STAT.ELF`, 15.5 s away behind `[43069ms]
///   [wc-x] desktop-app HOLD-EXPIRED reason=dmg-refuse-unsettled … waited=15005ms`.
/// * **clock** — [`clock_hhmm`] answered `None`: the civil clock has never been anchored, or was
///   contended. Either way the bar draws no clock this pass.
/// * **batt** — `status::source() == Unresolved`, which [`battery_witness`] states in words as
///   *"nothing has asked yet"*. This is NOT "this board has no battery": flight 11 emitted no
///   `[menubar] battery` line in 2.2 MB of wire, so on an rmbp WITH a pack the source never resolved
///   all boot and the item was absent for a reason nothing on the glass could distinguish from
///   "absent by design".
fn model_terms(m: &Model) -> (u64, &'static str) {
    let mut mask = 0u64;
    if m.cap_owner == wm::WIN_NONE {
        mask |= 1;
    }
    if m.clock.is_none() {
        mask |= 2;
    }
    if super::status::source() == super::status::Source::Unresolved {
        mask |= 4;
    }
    let what = match mask {
        0 => "",
        1 => "caption",
        2 => "clock",
        3 => "caption+clock",
        4 => "batt",
        5 => "caption+batt",
        6 => "clock+batt",
        _ => "caption+clock+batt",
    };
    (mask, what)
}

/// MENUFIRST — **the bar's first paint, once per boot, with what was in the model when it landed.**
///
/// Peter, flight 11: *"startup is still slow and shows a broken crystal"*. The capture answers half
/// of that on its own, and the half it answers is the half that was read backwards — so this line
/// exists to make the OTHER half readable without 2.2 MB of wire and a cycle count.
///
/// **There is no 5051 ms first-paint gap.** `[menubar] live` is a ROLLUP on a ~5 s cadence, and
/// [`compose`] emits it through `LEDGER.tick` ABOVE the damage test and the paint below it — so the
/// `paints=0` at 27616 ms and the `paints=1` at 32667 ms are the same single paint reported twice.
/// The capture settles it arithmetically: that paint reads `paint=1130360cyc/419us px/paint=97920`
/// at 32667 ms, at 37866 ms and again at 43053 ms — three reports, identical to the cycle, where a
/// second paint would have added to the total. The bar's first paint landed at 27617 ms, **1 ms
/// after the enable**, and the strip says the same on its own line: `[27617ms] [strip] rollup
/// tenant=menubar … paints=1 paint_px=97920 … -> CLEAN` (97920 = 2880 x 34 — the whole band).
///
/// So `after_enable_ms` is the number that was missing, and it is not the one that was looked for.
/// What was wrong on flight 11 is `model=`: the band that stood on the glass from 27617 ms to
/// 43364 ms — 15746 ms, and the strip states that too (`emit=2 age_ms=15746`) — was the bar's face,
/// its one keyline and the crystal, and NOTHING ELSE. No caption, no menu titles, no battery item.
/// A 2880-px grey strip with a single 16x22 gem at `x=12`, held for a quarter of a minute because
/// nothing in the model changed and so nothing owed a repaint.
///
/// Emitted from the PAINT SITE and not from the seam, because `[wc-x] menubar PAINTED` is the
/// shell's claim about its own control flow, and this is the bar's reading of what it drew.
#[cfg(feature = "witness")]
fn firstpaint_witness(m: &Model, r: strip::Rect) {
    // The RECORDER's one-shot is the reading's own valid bit, not the speak latch: one relaxed-ordered
    // load on a path that runs a handful of times per boot (`paints=3` on flight 11's fixture line),
    // and it keeps the measurement and the line independently one-shot. Everything below — the clock
    // read, the model reduction, the store — is paid once.
    if FIRSTPAINT_READING.load(Ordering::Acquire) & FP_VALID != 0 {
        return;
    }
    let at = now_ms();
    let enabled_at = ENABLED_AT_CYC.load(Ordering::Acquire);
    // The INTERVAL is measured in the counter directly rather than as a difference of two `at=`
    // readings: `at=` is truncated to whole ms, so subtracting two of them would quantise a
    // sub-millisecond first paint to `0` or `1` depending on where the truncation fell. Flight 11's
    // enable→paint was 1 ms of wall time and this is the arithmetic that can say so.
    let after = if enabled_at == CYC_NONE {
        None
    } else {
        Some(strip::cycles_to_us(crate::arch::now_cycles().saturating_sub(enabled_at)) / 1000)
    };
    let (mask, what) = model_terms(m);
    // CRYSTAL2 (B194) — the mark is DRAWN iff the PANEL holds it: every silhouette pixel in its ink, no
    // gem ink outside it, mirror-symmetric. This was `r.3 >= CRYSTAL_H && …` — the rect's geometry, true
    // on any 2880x34 bar whether or not a pixel of the gem reached the glass. See [`crystal_readback`].
    let gem = crystal_readback(r); let crystal = matches!(gem, Some((m, w, 0, true)) if m == w && w > 0);
    FIRSTPAINT_READING.store(
        FP_VALID
            | (mask << 32)
            | ((crystal as u64) << 35) | (gem_class(gem) << 36) // CRYSTAL2 — bits 36-37: the readback's word
            | after.unwrap_or(u32::MAX as u64).min(u32::MAX as u64),
        Ordering::Release,
    );
    // SPEAK iff nothing has spoken this boot — see [`FIRSTPAINT_SAID`], which the fixture cannot
    // re-arm. The reading above is stored FIRST and unconditionally, because the fixture's go-red
    // exists to be read back; what it must never do is put a second line on the wire carrying a
    // deliberately-wrong number, which would read exactly like the defect it is proving the fixture
    // can see. On metal the shell's seam has already spoken and the fixture is silent; on QEMU
    // `desktop_uefi::activate` never runs (no Kepler — `x86-wc.spec`'s own SCOPE note), so the
    // fixture's driven pass IS that boot's first paint and it speaks. One rule, both boards.
    if FIRSTPAINT_SAID.swap(true, Ordering::AcqRel) {
        return;
    }
    serial_println!(
        "[menubar] first-paint at={} after_enable_ms={} model={}{} crystal={} rect={}x{}+{}+{} gem_px={}/{} stray={} sym={}",
        Ms(at),
        Ms(after),
        if mask == 0 { "complete" } else { "partial:" },
        what,
        gem_word(gem),
        r.2, r.3, r.0, r.1, gem.map_or(0, |g| g.0), gem.map_or(0, |g| g.1), gem.map_or(0, |g| g.2), gem.map_or(false, |g| g.3)
    );
}

/// MENUFIRST — the fixture's re-arm. **Witness builds only, and [`firstpaint_selftest`] is its one
/// caller.**
///
/// Legs 3-4 of [`selftest`] and [`battery_selftest`] toggle the bar, so a fixture that wants to
/// MEASURE an enable→paint edge must be able to take one of its own without the shell's edge being
/// the thing it measures. Returns the previous `(enabled_at, reading)` so the fixture can put the
/// boot's real numbers back: a fixture that left its own synthetic edge latched would make every
/// later reader of this witness read the fixture instead of the boot.
///
/// ⛔ **The SPEAK latch is NOT in here.** See [`FIRSTPAINT_SAID`] — re-arming it is the one thing
/// that could put a second line on the wire, so the fixture is not given the ability.
#[cfg(feature = "witness")]
fn firstpaint_rearm() -> (u64, u64) {
    (
        ENABLED_AT_CYC.swap(CYC_NONE, Ordering::AcqRel),
        FIRSTPAINT_READING.swap(0, Ordering::AcqRel),
    )
}

/// MENUFIRST — put back what [`firstpaint_rearm`] took, so the fixture's edge does not outlive it.
#[cfg(feature = "witness")]
fn firstpaint_restore(prev: (u64, u64)) {
    ENABLED_AT_CYC.store(prev.0, Ordering::Release);
    FIRSTPAINT_READING.store(prev.1, Ordering::Release);
}

/// The crystal's box-relative top-left in the bar: **one [`strip::PAD`] from the left**, centred
/// vertically. THE ONE offset both the painter and [`crystal_box`] read, so the mark the fixture
/// witnesses is the mark the painter drew.
///
/// ⛔ **This PAD is not the gap, and it is not spendable.** `1046f81c` set this term to `0` while
/// closing the DROPDOWN's inset, which moved the mark onto the panel edge as collateral; Peter
/// rejected that on render9 (*"how i never once said to move the fucking crystal yet here it is
/// jammed right up to the edge"*). The mark's position is the one thing in this file that has never
/// been asked to change: it is the Mac apple's position, top-left and INSET. The dropdown's `x` is a
/// separate number, computed in [`super::crystal`]'s `menu_rect` and no longer derived from this one
/// — see [`CRYSTAL_SLOT`] for the three-state history.
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
/// ways here: [`crystal_box_abs`] stays the painter's truth, and this cell is what the click router
/// hits against. **This is also why the glyph's own PAD costs nothing in reach** — the corner pixel
/// opens the menu whether or not the mark is drawn on it.
///
/// Derived, not hardcoded: anchored at the bar rect's own origin — the true panel corner, since the
/// bar is `frame_flush(Top)`, so `(0,0)` is inside it by construction — and spanning the crystal's
/// whole left slot, [`CRYSTAL_SLOT`] wide (`PAD + CRYSTAL_W + PAD`: the glyph with both of its
/// margins, i.e. everything left of the title's inset) by the bar's full height. `bb513370`
/// mirrored this cell to the upper-RIGHT; the 2026-09-06 re-ruling puts it back, because the crystal
/// is the Mac menu and the Mac menu is top-left. Every pixel of the cell is a pixel the
/// BAR paints and composites above the windows, so widening the press target to it steals nothing: a
/// window dragged under the corner is under the bar there, and a press on visible bar chrome routing
/// to bar furniture is the fixed-furniture rule, not an exception to it.
///
/// `None` exactly when [`crystal_box_abs`] is `None` (bar disabled, or the panel cannot host it), so
/// a press with no bar still falls through to the arms below.
pub fn crystal_corner_abs(pw: usize, ph: usize) -> Option<strip::Rect> {
    let (bx, by, bw, bh) = strip_rect(pw, ph)?;
    Some((bx, by, CRYSTAL_SLOT.min(bw), bh))
}

/// WINMENU (R21) / MENUOWN — **where the focused window's menu titles begin when the bar names no
/// app**, as an offset from the bar's own origin. See [`MENUS_X0`].
///
/// This is the FALLBACK anchor, not the usual one: with a caption on the bar the titles follow the
/// app title box, one box gap after the name, and never a fixed column.
#[inline]
pub fn menus_x0() -> usize {
    MENUS_X0
}

/// SO3 — **where the CAPTION's glyphs begin**, as an offset from the bar's own origin: [`TITLE_X0`],
/// the same constant [`compose_row`] hands `draw_row`.
///
/// Peter's ruling (SO3, 2026-09-06): *every* app window's name in the bar must open a menu carrying
/// at least **Quit**. The name is drawn HERE and nowhere else, so this is the anchor the app-menu
/// title box is laid out around ([`super::winmenu::bar_boxes`]) — one constant, one accessor, so the
/// box a press lands in is the box the caption's glyphs were drawn in. Exported rather than
/// duplicated for the reason `crystal_box_abs` records for the brand mark.
#[inline]
pub fn caption_x0() -> usize {
    TITLE_X0
}

/// WINMENU (R21) — **the panel-absolute x the menu titles must stop before**: the clock's left edge,
/// less one [`strip::PAD`].
///
/// Derived from the SAME two terms [`compose_row`] draws the clock at ([`CLOCK_GLYPHS`] and
/// [`strip::PAD`]) rather than restated, so a title can never be laid out under the time. The crystal
/// does not appear here: it holds the bar's LEFT corner (2026-09-06 re-ruling), so it bounds where the
/// titles BEGIN ([`MENUS_X0`], via [`TITLE_X0`]) and not where they must stop. A bar too narrow to hold
/// the clock at all answers its own left edge, which lays out no titles.
pub fn menus_right_limit(bar: strip::Rect) -> usize {
    let (bx, _by, bw, _bh) = bar;
    let need = 2 * strip::PAD + CLOCK_GLYPHS * CELL_W;
    // MENUSTAT — the limit is the STATUS AREA's left edge, not the clock's. This function's own
    // rule is that "a title can never be laid out under the time"; the battery item is the same
    // claim about the same rows, and the moment a second item exists the clock stops being the
    // leftmost thing out here. Derived from [`batt_slot`] — the SAME function the painter places
    // the item with — rather than from a restated arithmetic, so a title box and the cell it must
    // not sit under have one definition between them.
    //
    // It is a function of RUNTIME state (is there an item), which is new for this accessor and is
    // accounted for: `Model::signature` folds the item's presence, so an item appearing or going
    // absent repaints the bar and re-lays the titles in the same pass.
    if super::status::bar_item().is_some() {
        if let Some(x0) = batt_slot(bw) {
            return bx + x0.saturating_sub(strip::PAD);
        }
    }
    bx + bw.saturating_sub(need)
}

/// **The CLOCK's left inset inside a bar `w` px wide**, or `None` when the bar is too narrow to
/// draw one — [`compose_row`]'s own test, lifted so it has ONE definition.
///
/// MENUSTAT lifted it because three sites now need the same number and two of them are new: the
/// painter (which always had it inline), [`batt_slot`] (which places the status item one PAD to its
/// left) and [`battery_selftest`]'s layout leg (which asserts the clock's rect does not move when
/// the item appears). Three copies of `w - PAD - CLOCK_GLYPHS * CELL_W` is how the clock ends up in
/// two places at once.
#[inline]
fn clock_slot(w: usize) -> Option<usize> {
    let cw = CLOCK_GLYPHS * CELL_W;
    if w > cw + strip::PAD { Some(w - strip::PAD - cw) } else { None }
}

/// The clock's rect on the PANEL — `(x, y, w, h)`, absolute — or `None` when no clock is drawn.
///
/// For the fixture: the falsifier for *"the status item did not move the clock"*. It is a function
/// of the bar rect and [`clock_slot`] alone and mentions the battery nowhere, which is the reason
/// the claim holds — but a claim that holds BY CONSTRUCTION still has to be readable off a capture,
/// and this is what puts it there.
#[cfg(feature = "witness")]
fn clock_rect(r: strip::Rect) -> Option<(usize, usize, usize, usize)> {
    let (rx, ry, w, h) = r;
    let x0 = clock_slot(w)?;
    Some((rx + x0, ry + (h - CELL_H) / 2, CLOCK_GLYPHS * CELL_W, CELL_H))
}

/// MENUSTAT — **the battery item's left inset inside a bar `w` px wide**, or `None` when the bar
/// cannot seat it.
///
/// One [`strip::PAD`] left of the clock — the same gap the clock keeps from the bar's right edge,
/// so the two items in the status area are spaced by the kit's one number and not by a second one
/// invented for this row.
///
/// ⛔ **The decline is the floor rule, and it is why [`FLOOR_W`] did not have to move.** The item
/// yields on a panel that cannot seat it: not enough width for the clock at all, not enough to put
/// a PAD and the item to its left, or not enough left over for the caption's first glyph past
/// [`TITLE_X0`]. On the floor panel the bar therefore still draws exactly what it drew before this
/// arc — crystal, a title glyph, the clock — and the item is the thing that is absent, which is the
/// same yielding discipline `FLOOR_H` already states between the bar and the dock.
///
/// It does NOT ask whether there IS a battery: this is geometry. [`compose_row`] and
/// [`menus_right_limit`] pair it with [`super::status::bar_item`], so "no source" and "no room" stay
/// two separate facts with two separate answers.
fn batt_slot(w: usize) -> Option<usize> {
    let x0 = clock_slot(w)?.checked_sub(strip::PAD + BATT_ITEM_W)?;
    if x0 < TITLE_X0 + CELL_W {
        return None;
    }
    Some(x0)
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
/// ([`theme::CONTROL_MID`]). The CLOSE|ZOOM seam on the box's centre line reads as the crown's facet
/// edge. CRYSTAL2 (B194): centred on the box's HALF-pixel line, so the gem is mirror-symmetric (it was
/// centred on column 8 of 16: the girdle's right tip clipped by the box, the crown split 4 dark to 5 lit).
#[inline]
fn crystal_facet(u: usize, v: usize) -> Option<u32> {
    if v >= CRYSTAL_H || u >= CRYSTAL_W {
        return None;
    }
    let cx = CRYSTAL_W / 2; // the facet seam: columns `< cx` are the shadowed face, `>= cx` the lit one
    let half = crystal_half(v);
    let d2 = (2 * u + 1).abs_diff(CRYSTAL_W); // CRYSTAL2 — distance from the box's centre line, in HALF px
    if d2 > (2 * half).saturating_sub(1).max(1) {
        return None;
    }
    Some(if v < CRYSTAL_CROWN_H {
        if u < cx { theme::CONTROL_CLOSE } else { theme::CONTROL_ZOOM }
    } else {
        theme::CONTROL_MID
    })
}

/// MENUSTAT — compose the battery GLYPH into row `j` of the bar: an outlined cell whose fill is
/// proportional to the charge, with a lightning mark when current is flowing in.
///
/// Called from [`compose_row`] ABOVE the text-band early return, for the crystal's reason: the cell
/// is centred in the whole bar and spans rows the glyph band never touches. The percent TEXT is
/// drawn in the band, beside the clock, from the same [`batt_slot`].
///
/// # The inks, and why no palette is invented
///
/// The status area is SECONDARY chrome — the clock is already drawn in
/// [`theme::TITLE_TEXT_INACTIVE`] on exactly that argument ("the title is what the operator is
/// reading, the clock is what they glance at"), and the battery is the same kind of fact. So the
/// outline and the percent take that ink, and the FILL takes [`theme::TITLE_TEXT_ACTIVE`] — the
/// bar's primary — because the fill is the one part of the item that carries the measurement. Two
/// roles the bar already draws with; nothing new.
///
/// # ⛔ The bolt is the INVERSE of what is under it
///
/// A charging mark in one fixed ink is legible over exactly one of the two backgrounds it can land
/// on. At 97 % the bolt sits on FILL (dark); at 3 % it sits on the bar's FACE showing through the
/// empty cell (light). So the bolt is drawn in [`theme::BEVEL_LIGHT`] over filled columns and in
/// the fill ink over empty ones — one rule, both cases, no third colour. The build-time asserts on
/// [`BOLT_W`]/[`BOLT_H`] keep it strictly inside the outline, which is what makes "whatever is under
/// it" mean fill-or-face and never the cell's own edge.
fn draw_battery_glyph(out: &mut [u32], w: usize, h: usize, j: usize, x0: usize, it: super::status::BarItem) {
    let by0 = (h - BATT_BODY_H) / 2;
    if j < by0 || j >= by0 + BATT_BODY_H {
        return;
    }
    let v = j - by0;
    let outline = theme::TITLE_TEXT_INACTIVE;
    let ink = theme::TITLE_TEXT_ACTIVE;
    // The fill, in columns of the cell's INTERIOR (the outline takes one column each side). Integer
    // arithmetic, no float — the crystal's discipline. `percent` is already clamped at the decode
    // (`status::decode`), so this can never exceed the interior.
    let inner = BATT_BODY_W - 2;
    let fill = inner * (it.percent.min(100) as usize) / 100;
    let edge_row = v == 0 || v + 1 == BATT_BODY_H;
    for u in 0..BATT_BODY_W {
        let c = if edge_row || u == 0 || u + 1 == BATT_BODY_W {
            outline
        } else if u - 1 < fill {
            ink
        } else {
            continue; // the empty part of the cell keeps the bar's own face
        };
        let i = x0 + u;
        if i < w {
            out[i] = c;
        }
    }
    // The positive terminal's nub, centred on the body.
    let ny = (BATT_BODY_H - BATT_CAP_H) / 2;
    if v >= ny && v < ny + BATT_CAP_H {
        for u in 0..BATT_CAP_W {
            let i = x0 + BATT_BODY_W + u;
            if i < w {
                out[i] = outline;
            }
        }
    }
    if !it.charging {
        return;
    }
    let bx = (BATT_BODY_W - BOLT_W) / 2;
    let byy = (BATT_BODY_H - BOLT_H) / 2;
    if v < byy || v >= byy + BOLT_H {
        return;
    }
    let row = BOLT[v - byy];
    for u in 0..BOLT_W {
        if row & (1 << (BOLT_W - 1 - u)) == 0 {
            continue;
        }
        let cell = bx + u;
        let i = x0 + cell;
        if i < w {
            out[i] = if cell - 1 < fill { theme::BEVEL_LIGHT } else { ink };
        }
    }
}

/// Compose panel row `j` of the bar into `out[0..w]` as logical `0x00RRGGBB` colours.
///
/// A field pass (face, bevel, keyline), then the brand CRYSTAL overlaid at its left inset, then the two
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

    // The brand CRYSTAL, overlaid at its one-PAD left inset ([`crystal_offset`]). Drawn BEFORE the
    // text early-return because the gem spans more rows than the glyph cell does — it is centred in
    // the whole bar, not the band.
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

    // MENUSTAT — the battery CELL, at its slot left of the clock. Before the text-band return for
    // the crystal's reason: the cell is centred in the bar, not in the glyph band. Two `Option`s and
    // no item is drawn unless BOTH answer — `m.batt` is *does this board have a battery*
    // ([`super::status`]'s measured answer, `None` on QEMU, the Pi and the Orin) and [`batt_slot`]
    // is *can this panel seat it*. Keeping them apart is what makes "absent" one fact with two
    // distinguishable causes instead of one silent blank.
    if let (Some(it), Some(bx0)) = (m.batt, batt_slot(w)) {
        draw_battery_glyph(out, w, h, j, bx0, it);
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
    //
    // SO3 — the caption is the APP MENU's title, so when that menu is down it takes the lit-title
    // ink `winmenu::draw_bar_row` gives every other open title. The box fill under it was already
    // laid by that call (it runs above the band return, and box 0 is the caption's); this is the
    // other half of the same convention, kept HERE because the bar owns the caption's glyphs and
    // `draw_bar_row` deliberately does not redraw them.
    let cols = m.title_len.min(TITLE_GLYPHS);
    let cap_ink = if m.menus.app_open() { theme::BEVEL_LIGHT } else { theme::TITLE_TEXT_ACTIVE };
    super::font::draw_row(out, w, &m.title[..cols], TITLE_X0, sy, cap_ink, BOLD, FACE);

    // MENUSTAT — the battery's PERCENT, right-aligned in its fixed [`BATT_PCT_GLYPHS`] slot so the
    // cell beside it never moves when a digit appears or goes. Secondary ink, the clock's: the
    // whole status area is glanced at, not read. Drawn from the SAME [`batt_slot`] the cell was, so
    // the two halves of one item cannot drift apart.
    if let (Some(it), Some(bx0)) = (m.batt, batt_slot(w)) {
        let mut pct = [b' '; BATT_PCT_GLYPHS];
        let p = it.percent.min(100);
        pct[BATT_PCT_GLYPHS - 1] = b'%';
        if p >= 100 {
            pct[BATT_PCT_GLYPHS - 4] = b'1';
            pct[BATT_PCT_GLYPHS - 3] = b'0';
            pct[BATT_PCT_GLYPHS - 2] = b'0';
        } else if p >= 10 {
            pct[BATT_PCT_GLYPHS - 3] = b'0' + (p / 10) as u8;
            pct[BATT_PCT_GLYPHS - 2] = b'0' + (p % 10) as u8;
        } else {
            pct[BATT_PCT_GLYPHS - 2] = b'0' + p as u8;
        }
        let tx = bx0 + BATT_GLYPH_W + BATT_GAP;
        super::font::draw_row(out, w, &pct, tx, sy, theme::TITLE_TEXT_INACTIVE, false, FACE);
    }

    // Clock, right, at one PAD from the far edge — the crystal holds the LEFT corner, so nothing of
    // the brand sits out here. Secondary ink: the title is what the operator is reading, the clock is
    // what they glance at.
    // MENUSTAT — the two-line test that stood here (`let cw = …; if w > cw + strip::PAD`) is now
    // [`clock_slot`], because the status item has to be placed one PAD to the LEFT of exactly this
    // x and a second copy of `w - PAD - CLOCK_GLYPHS * CELL_W` is how a clock ends up in two places
    // at once. Same arithmetic, same guard, one definition — and it is the definition the fixture's
    // `clock=` term and [`batt_slot`] both read.
    if let (Some(c), Some(cx)) = (m.clock, clock_slot(w)) {
        super::font::draw_row(out, w, &c, cx, sy, theme::TITLE_TEXT_INACTIVE, false, FACE); #[cfg(feature = "sntp6")] barclock_note(Some((cx, ty0, CLOCK_GLYPHS * CELL_W, CELL_H))); // SNTP-NET6, folded LINE-NEUTRAL (code before the comment, LEDGER P7): the SET half, reported from the one place that knows the clock's DRAWN rect. `compose_row` runs once per row per pass, so this call is on the compositor cadence and the latch at the file tail — not this site — is what makes it one line per boot (SO30).
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
    // crystal box must be the compiled size, sit wholly within it, and sit at EXACTLY one
    // [`strip::PAD`] from its left edge — render9's falsifier at the paint end. The equality is the
    // load-bearing half in BOTH directions: a larger x is the pre-`bb513370` drift creeping back, and
    // `cbx == brx` is `1046f81c`'s regression, the mark jammed onto the panel edge. The DROPDOWN's own
    // x is not tested here — it is a separate number now, witnessed by `crystal.rs`.
    let (cbx, cby, cbw, cbh) = r.map(crystal_box).unwrap_or((0, 0, 0, 0));
    let crystal_ok = match r {
        Some((brx, bry, brw, brh)) => {
            cbw == CRYSTAL_W
                && cbh == CRYSTAL_H
                && cbx >= brx
                && cbx + cbw <= brx + brw
                && cby >= bry
                && cby + cbh <= bry + brh
                && cbx == brx + strip::PAD // one PAD in from the bar's LEFT edge, never flush
                && cbx + cbw <= brx + TITLE_X0 // and left of where the title begins
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

    // MENUSTAT — the status area's own fixture, LAST and in its own function. Last because it
    // drives `compose` with injected readings and must not perturb the census legs above; its own
    // function because it is a separate claim with a separate verdict line, and a leg folded into
    // `:: MENUBAR:` would have made a decode defect read as a geometry failure.
    battery_selftest(pw, ph);

    // MENUFIRST — the enable→first-paint edge, AFTER the battery fixture and for the same two
    // reasons it is after the census legs: it drives `compose` (so it must not perturb a leg that
    // reads the model), and its claim — *the first paint lands inside one composite pass of the
    // enable* — deserves its own verdict line rather than a term folded into `:: MENUBAR:`.
    firstpaint_selftest(pw, ph); crystal_persist_selftest(pw, ph); // CRYSTAL2 (B194) — the crystal on EVERY paint, read off the panel; after MENUFIRST for its reason (it drives `compose`). ⚠ SAME-LINE fold (B94).

    rollup("selftest");
}

/// MENUSTAT fixture — **the battery item decodes flight 11's bytes, does not move the clock, and
/// does not repaint the bar when nothing it draws has changed.**
///
/// Six legs, each able to fail on its own, and the first two are a pair: a PASS leg that can only be
/// trusted because the RED leg beside it proves the decoder is what produced it.
///
/// 1. **the decode** — the nine byte strings `:: SMC-SCOUT:` printed at 25627–25629 ms on flight 11
///    (`BNum=[01] BRSC=[00 52] B0St=[00 80] B0AC=[03 f6] B0AV=[30 17] B0TF=[00 6a]`, with
///    `B0FC=[26 ea]`/`B0RM=[1f e0]` as the corroboration) fed to [`super::status::decode`] verbatim
///    must yield **82 %, charging, 106 min, 12311 mV, +1014 mA**. Hardware facts, off the wire, used
///    as such.
/// 2. ⛔ **the GO-RED** — the same bytes through the same decoder with its byte order swapped
///    ([`super::status::set_byte_swap`], the injection inside `be16`). It must NOT yield 82 %. This
///    is what stops leg 1 being a tautology: without it the leg would pass on a decoder that ignored
///    its arguments and returned the constants this comment names.
/// 3. **absence decodes as absence** — no keys at all (QEMU's `isa-applesmc`) and `BNum=[00]` (a
///    machine whose SMC reports zero packs) both answer `None`. The honest empty state is reachable,
///    not merely intended.
/// 4. ⛔ **the CLOCK DOES NOT MOVE** — [`clock_rect`] is identical with the item injected present
///    and injected absent, and so are [`caption_x0`] and [`geometry`]. This is the leg the brief
///    named: the crystal's position is R25's, the caption's inset is SO3's, and the clock's is this
///    file's, and a status item that paid for its slot out of any of them would be taking a rule's
///    pixels to draw a convenience.
/// 5. **the item is seated where the layout says** — [`batt_slot`] is one PAD left of the clock and
///    clear of the caption's first glyph. The numbers go on the line so a capture and the ledger row
///    can be checked against each other.
/// 6. ⛔ **the repaint discipline, MEASURED** — `compose()` returns `true` iff it painted, so the
///    claim is read directly off the composite rather than inferred. A reading whose minutes,
///    current and voltage move but whose percent and charge state do not must paint NOTHING
///    (`jitter_paint=false`); a percent change must paint (`change_paint=true`). The positive half
///    is a control: without it a bar that had stopped painting altogether would pass the first half.
///
/// The model's state is snapshotted and restored, so a boot whose SMC had already answered gets its
/// reading back and the fixture cannot leave a synthetic battery on the operator's glass.
#[cfg(feature = "witness")]
pub fn battery_selftest(pw: usize, ph: usize) {
    use super::status;
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    // Flight 11, 2026-09-.. at 25627–25629 ms — the bytes, not a transcription of their meaning.
    const F11_BNUM: Option<u8> = Some(0x01);
    const F11_BRSC: Option<[u8; 2]> = Some([0x00, 0x52]);
    const F11_B0ST: Option<[u8; 2]> = Some([0x00, 0x80]);
    const F11_B0AC: Option<[u8; 2]> = Some([0x03, 0xf6]);
    const F11_B0AV: Option<[u8; 2]> = Some([0x30, 0x17]);
    const F11_B0TF: Option<[u8; 2]> = Some([0x00, 0x6a]);
    let want = status::Battery { percent: 82, charging: true, minutes: Some(106), ma: 1014, mv: 12311 };

    // Leg 1 — the decode.
    let got = status::decode(F11_BNUM, F11_BRSC, F11_B0ST, F11_B0AC, F11_B0AV, F11_B0TF);
    let decode_ok = got == Some(want);
    let (gp, gc, gm, gv, ga) = match got {
        Some(b) => (b.percent, b.charging, b.minutes.unwrap_or(0), b.mv, b.ma),
        None => (0, false, 0, 0, 0),
    };

    // Leg 2 — the go-red, through the SAME call with the fault armed.
    status::set_byte_swap(true);
    let red = status::decode(F11_BNUM, F11_BRSC, F11_B0ST, F11_B0AC, F11_B0AV, F11_B0TF);
    status::set_byte_swap(false);
    let red_pct = red.map(|b| b.percent).unwrap_or(0);
    let gone_red = red != Some(want) && red_pct != want.percent;

    // Leg 3 — absence.
    let absent_ok = status::decode(None, None, None, None, None, None).is_none()
        && status::decode(Some(0), F11_BRSC, F11_B0ST, F11_B0AC, F11_B0AV, F11_B0TF).is_none()
        // a partial sweep is not a reading: no percent, no item (the caller HOLDS instead)
        && status::decode(F11_BNUM, None, F11_B0ST, F11_B0AC, F11_B0AV, F11_B0TF).is_none();

    // Legs 4-6 need the bar present and the model drivable. Everything below restores what it found.
    let saved_model = status::snapshot_state();
    let saved_en = enabled();
    set_enabled(true);
    let r = strip_rect(pw, ph);

    // Leg 4 — the clock, the caption and the floors, with the item absent and present.
    status::inject(None);
    let (clk_a, cap_a, geo_a) = (r.and_then(clock_rect), caption_x0(), geometry(pw, ph));
    status::inject(Some(want));
    let (clk_b, cap_b, geo_b) = (r.and_then(clock_rect), caption_x0(), geometry(pw, ph));
    let layout_ok = clk_a == clk_b && cap_a == cap_b && geo_a == geo_b && clk_b.is_some();

    // Leg 5 — where the item sits, from the same accessors the painter uses.
    let bw = r.map(|(_, _, w, _)| w).unwrap_or(0);
    let slot = batt_slot(bw);
    let clock_x = clock_slot(bw).unwrap_or(0);
    let seat_ok = match slot {
        Some(x0) => x0 + BATT_ITEM_W + strip::PAD == clock_x && x0 >= TITLE_X0 + CELL_W,
        None => false,
    };

    // Leg 6 — the repaint discipline. `compose` returns `true` iff it painted.
    let _ = compose(); // settle: this pass owes the item it has just been given
    let _ = compose(); // and the signature is now stored
    status::inject(Some(status::Battery { percent: 82, charging: true, minutes: Some(99), ma: 1041, mv: 12290 }));
    let jitter_paint = compose();
    status::inject(Some(status::Battery { percent: 83, ..want }));
    let mut change_paint = false;
    for _ in 0..4 {
        // A pass can decline for reasons that have nothing to do with damage (a contended panel, a
        // busy winmenu registry — both return `false` and re-ask next composite), so the POSITIVE
        // control is given the retries the negative one must not have.
        if compose() {
            change_paint = true;
            break;
        }
    }
    let damage_ok = !jitter_paint && change_paint;

    status::restore_state(saved_model);
    set_enabled(saved_en);

    let (cx, cy, cw2, ch) = clk_b.unwrap_or((0, 0, 0, 0));
    let ok = decode_ok && gone_red && absent_ok && layout_ok && seat_ok && damage_ok;
    serial_println!(
        ":: MENUBATT: pct={} charging={} mins={} mv={} ma={} b0st=0x{:02x}{:02x} \
         item_w={} glyph={}x{} gap={} pct_glyphs={} batt_x={} clock={}x{}+{}+{} caption_x0={} \
         floor={}x{} red_pct={} decode_ok={} gone_red={} absent_ok={} layout_ok={} seat_ok={} \
         jitter_paint={} change_paint={} damage_ok={} :: {} ::",
        gp,
        if gc { "y" } else { "n" },
        gm,
        gv,
        ga,
        F11_B0ST.unwrap_or([0, 0])[0],
        F11_B0ST.unwrap_or([0, 0])[1],
        BATT_ITEM_W,
        BATT_GLYPH_W,
        BATT_BODY_H,
        BATT_GAP,
        BATT_PCT_GLYPHS,
        match slot { Some(x) => x as i64, None => -1 },
        cw2, ch, cx, cy,
        cap_b,
        FLOOR_W, FLOOR_H,
        red_pct,
        decode_ok, gone_red, absent_ok, layout_ok, seat_ok,
        jitter_paint, change_paint, damage_ok,
        if ok { "PASS" } else { "FAIL" }
    );
}

/// MENUFIRST fixture — **the bar's first paint lands inside ONE COMPOSITE PASS of the enable, it is
/// RECORDED, and the recorder can say a late one is late.**
///
/// Four legs, and the fourth is what makes the first three worth reading.
///
/// 1. **the EDGE is stamped by the enable, not by the paint** — [`ENABLED_AT_CYC`] is `CYC_NONE` with
///    the bar off and a real reading after `set_enabled(true)`. Without this the bound below would be
///    measuring a number the paint site wrote about itself.
/// 2. **a paint lands, and the recorder holds it** — `compose()` returns `true` (it is the composite
///    that says whether it painted, so the claim is read off the pass rather than inferred) and
///    [`FIRSTPAINT_READING`] carries [`FP_VALID`]. The retries the positive control is given are
///    [`battery_selftest`]'s leg 6 rule: a pass can decline for a contended panel or a busy winmenu
///    registry, which has nothing to do with damage.
/// 3. **the BOUND** — `after_enable_ms <= `[`ONE_COMPOSITE_MS`]. This is the brief's number, and on
///    flight 11 the boot itself already met it at 1 ms; the leg exists so a future seam that defers
///    the enable-seam composite (the `[wc-x] menubar PAINTED` line is the shell's CLAIM, and the
///    x86 seam does not read `owns_pixels()` back the way `desktop_firmware`'s twin does) is caught
///    here instead of on a flight.
/// 4. ⛔ **the GO-RED, through the same recorder and the same predicate** — the edge is pushed back
///    by **5051 ms**, which is not an arbitrary number: it is the exact gap the brief read off two
///    `[menubar] live` rollups (27616 ms `paints=0`, 32667 ms `paints=1`) that are the SAME PAINT
///    reported twice on the ledger's ~5 s cadence. A bar whose first paint really had landed 5051 ms
///    after its enable is the defect; this leg manufactures it and requires the bound to answer
///    `false`. Without it leg 3 would pass on a test that ignored its argument — and, since the
///    number it injects is the one the arc was sent to chase, a green leg 4 is also the standing
///    statement that the recorder would have caught that gap if it had been real.
///
/// The injection RECORDS but does not SPEAK: [`FIRSTPAINT_SAID`] is the speak latch and the
/// fixture is deliberately not given the ability to re-arm it, so the second and later paints this
/// function drives are silent whatever order they run in. Everything the fixture DOES touch — the
/// edge, the reading and the enable flag — is put back, so the boot's own `[menubar] first-paint`
/// line is the one a capture reads and this fixture is invisible in it.
#[cfg(feature = "witness")]
pub fn firstpaint_selftest(pw: usize, ph: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let saved_en = enabled();
    let rect = geometry(pw, ph);

    // Leg 1 — the edge. Taken on the fixture's OWN off→on transition, with the recorder re-armed so
    // the stamp under test is this one and not a shell seam that ran twenty seconds ago.
    let prev = firstpaint_rearm();
    set_enabled(false);
    let _ = compose(); // discharge the vacate, so the slot is clear and the next enable owes a paint
    let unstamped = ENABLED_AT_CYC.load(Ordering::Acquire) == CYC_NONE;
    set_enabled(true);
    let stamped = ENABLED_AT_CYC.load(Ordering::Acquire) != CYC_NONE;

    // Leg 2 — the paint. CRYSTAL2 (B194): SETTLED ON THE RECORDER, not four back-to-back `compose()`
    // tries. Flight 12 declined all four (and leg 4's four) on a refused leaf lock — `decl_lock` 0 -> 16
    // across the fixture, zero `[menubar]` paints landed — and the verdict then printed the ZEROS of an
    // empty reading as `model=complete crystal=absent`: FIXTURE_FLAKES Class 6. An unlanded paint is
    // now a stated SKIP with its settle figures on the line, never a FAIL and never a fake reading.
    let settle_a = paint_settle(|| FIRSTPAINT_READING.load(Ordering::Acquire) & FP_VALID != 0);
    let painted = settle_a.0; // a paint LANDED and was recorded — a sibling core's pass counts too
    // (`recorded` below stays its own field: it is the READING's valid bit, `painted` is the settle's.)
    let reading = FIRSTPAINT_READING.load(Ordering::Acquire);
    let recorded = reading & FP_VALID != 0;
    let after = reading & 0xFFFF_FFFF;
    let mask = (reading >> 32) & 0x7;
    let crystal = (reading >> 35) & 1 == 1;

    // Leg 3 — the bound.
    let bounded = recorded && after <= ONE_COMPOSITE_MS;

    // Leg 4 — ⛔ the GO-RED. Same recorder, same predicate, the edge pushed back by the 5051 ms the
    // rollup cadence looked like. The toggle is what makes the next `compose` owe a paint: turning
    // the bar off clears the slot, so the pass after the re-enable has no stored signature to match.
    let _ = firstpaint_rearm();
    set_enabled(false);
    let _ = compose();
    set_enabled(true);
    let real_edge = ENABLED_AT_CYC.load(Ordering::Acquire);
    // 5051 ms expressed in CYCLES, self-calibrated through the SAME `strip::cycles_to_us` the reading
    // is taken with — so the injection is stated in the units the bound is stated in rather than in a
    // guessed TSC rate, and it stays 5051 ms on a machine of any clock. `probe_us.max(1)` is the
    // uncalibrated-counter guard `cycles_to_us` itself carries; the product below is ~5.3e12 at a
    // 1 GHz-scale rate and cannot overflow `u64`.
    let probe_cyc: u64 = 1 << 20;
    let probe_us = strip::cycles_to_us(probe_cyc).max(1);
    let inject_cyc = (RED_INJECT_MS * 1000).saturating_mul(probe_cyc) / probe_us;
    ENABLED_AT_CYC.store(real_edge.saturating_sub(inject_cyc), Ordering::Release);
    // CRYSTAL2 — the same settle on the same recorder, for leg 2's reason. The injected edge above is
    // what the recorder measures from, whichever core's pass lands the paint.
    let settle_b = paint_settle(|| FIRSTPAINT_READING.load(Ordering::Acquire) & FP_VALID != 0);
    let red_painted = settle_b.0;
    // A leg-4 paint that never lands leaves `gone_red=false` for the same foreign reason leg 2's would,
    // so the verdict is SKIP when EITHER settle ran out: the control is unproven, not refuted. A landed
    // paint that reads wrong (late, unrecorded, gem not on the panel) is still a FAIL.
    let red_reading = FIRSTPAINT_READING.load(Ordering::Acquire);
    let red_after = red_reading & 0xFFFF_FFFF;
    let gone_red = red_painted && red_reading & FP_VALID != 0 && !(red_after <= ONE_COMPOSITE_MS);

    set_enabled(false);
    let _ = compose(); // hand the band back before the boot's own state is restored
    firstpaint_restore(prev);
    set_enabled(saved_en);

    let (rw, rh) = rect.map(|(_, _, w, h)| (w, h)).unwrap_or((0, 0));
    let ok = unstamped && stamped && painted && recorded && bounded && gone_red && crystal; let gemc = (reading >> 36) & 3;
    serial_println!(
        ":: MENUFIRST: after_enable_ms={} bound_ms={} model={}{} crystal={} bar={}x{} \
         gem={}x{}+{}+{} red_after_enable_ms={} settle_tries={}+{} settle_us={}+{} decl_lock={} unstamped={} stamped={} painted={} recorded={} \
         bounded={} gone_red={} :: {} ::",
        after,
        ONE_COMPOSITE_MS,
        if !recorded { "unread" } else if mask == 0 { "complete" } else { "partial:" },
        match mask {
            0 => "",
            1 => "caption",
            2 => "clock",
            3 => "caption+clock",
            4 => "batt",
            5 => "caption+batt",
            6 => "clock+batt",
            _ => "caption+clock+batt",
        },
        if recorded { GEM_WORDS[gemc as usize] } else { "unread" },
        rw, rh,
        CRYSTAL_W, CRYSTAL_H, crystal_offset(rh.max(CRYSTAL_H)).0, crystal_offset(rh.max(CRYSTAL_H)).1,
        red_after, settle_a.1, settle_b.1, settle_a.2, settle_b.2, settle_a.3 + settle_b.3,
        unstamped, stamped, painted, recorded, bounded, gone_red,
        if !painted || !red_painted { "SKIP" } else if ok { "PASS" } else { "FAIL" }
    );
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

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// SNTP-NET6 — the bar's CLOCK WITNESS. Appended at the FILE TAIL, so not one `panic::Location`
// above it moves and the knob-off image is byte-identical (LAWS §5, byte identity is measured).
// ═════════════════════════════════════════════════════════════════════════════════════════════════
//
// WHY THIS EXISTS AT ALL. The bar has drawn a clock in its upper right since it was written, and on
// this arch nobody has ever seen it, because nothing on the board ever anchored the civil clock —
// `clock::try_unix_now()` was `None` on every pass of every boot, so the honest branch at
// `clock_hhmm` fired every time and the bar drew nothing. SNTP-NET6 makes the OTHER branch reachable
// on aarch64 for the first time, and a branch nothing can observe is a branch nobody can score: the
// existing `clock={set|unsynced}` term lives on the `:: MENUBAR:` census line, which is reached only
// from the x86 `dock::selftest`. This module's own comments at :400 and :938 state the invariant that
// an aarch64 image contains NO `:: MENUBAR:` string at all, and that invariant is verified against
// the built artifact. So on the Orin we were blind, and adding a second `:: MENUBAR:` string would
// have paid for sight by breaking the thing that made the x86 leg checkable.
//
// Hence a NEW, BOUNDED family: `:: BARCLOCK:`. Two states, each reported AT MOST ONCE per boot, so
// the ceiling is two lines for the life of the machine. That bound is the whole design — `compose_row`
// runs once per row of every composite, and a per-pass line here is SO30 exactly, the defect that ate
// 36% of a boot's wire. The latch below, not the call sites, is what enforces it.
//
// THE HONESTY RULE IS ASSERTED, NOT ASSUMED. The bar must draw NO clock while unsynced AND the title
// must keep its width. The second half is a STATIC fact and is asserted as one: `TITLE_X0` and
// `TITLE_GLYPHS` are `const`s that do not mention the clock, so no clock state can move the caption —
// the `const _` below says so in a form the compiler checks on every build, which is stronger than a
// runtime probe that only covers the states a given boot happened to reach. The first half is
// asserted at runtime by `net_sntp_client::fixture()` leg 0x20 (anchored => `try_unix_now` is `Some`,
// cleared => `None`), which is the exact predicate `clock_hhmm` reads.

/// The bar's clock states, latched. Bit 0 = `unsynced` reported, bit 1 = `set` reported. Relaxed is
/// sufficient: the only thing ordered against this is whether a line has already been printed, and a
/// doubled line under an improbable race is a cosmetic cost, never a wrong fact.
#[cfg(feature = "sntp6")]
static BARCLOCK_SEEN: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// The title's geometry does not mention the clock, so the clock's state cannot move the caption.
/// Compile-time, because that is what the claim actually is.
#[cfg(feature = "sntp6")]
const _: () = {
    // The caption starts at a constant that does not mention the clock, and it is allowed the whole
    // stored title either way — so no clock state can move or shorten it.
    assert!(TITLE_GLYPHS <= wm::MAX_TITLE);
    assert!(TITLE_X0 > 0);
    // And the clock's slot is RESERVED in the width floor whether or not a clock is drawn, which is
    // the mechanism that keeps the above true: an unsynced bar does not hand the title that space.
    assert!(FLOOR_W >= (CLOCK_GLYPHS + 1) * CELL_W);
};

/// Report the bar's clock state ONCE per state per boot. `Some((x, y, w, h))` is the rect the clock
/// was just drawn into (panel-relative, the bar sits at `y = 0`); `None` is the unsynced state, in
/// which nothing was drawn.
///
/// Called from two folded sites — the model (`None`) and the painter (`Some`) — and from nowhere
/// else. Takes no lock: it reads the already-computed state its caller holds, so it cannot spin
/// inside a composite, which is the rule `clock::try_unix_now` exists to honour.
#[cfg(feature = "sntp6")]
fn barclock_note(rect: Option<(usize, usize, usize, usize)>) {
    use core::sync::atomic::Ordering as O;
    let bit = if rect.is_some() { 0b10 } else { 0b01 };
    if BARCLOCK_SEEN.fetch_or(bit, O::Relaxed) & bit != 0 {
        return; // this state has already been said aloud this boot
    }
    match rect {
        Some((x, y, w, h)) => {
            let mut iso = [0u8; 24];
            let n = crate::clock::try_unix_now().map(|s| crate::clock::render_iso8601(s, &mut iso));
            serial_println!(
                ":: BARCLOCK: clock=set rect={}x{}+{}+{} glyphs={} iso={} title_glyphs={} face={} — the bar drew a clock for the first time this boot ::",
                w, h, x, y, CLOCK_GLYPHS,
                match n { Some(k) => core::str::from_utf8(&iso[..k]).unwrap_or("????"), None => "(contended)" },
                TITLE_GLYPHS, BAR_FONT_NAME
            );
        }
        None => serial_println!(
            ":: BARCLOCK: clock=unsynced rect=none glyphs=0 title_glyphs={} — no civil anchor this boot, so the bar draws NO clock and the title keeps its width ::",
            TITLE_GLYPHS
        ),
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// CRYSTAL2 (rmbp-ledger B194) — THE CRYSTAL READ OFF THE PANEL, ON EVERY PAINT, AND A FIXTURE PAINT
// THAT SETTLES INSTEAD OF GIVING UP AFTER FOUR TRIES. Tail-appended so no panic `Location` above
// moves (B94); every item is `witness`-gated, the image Peter flies is a witness build.
//
// Flight 12 put two readings of the same mark on one wire. `[menubar] first-paint … crystal=drawn`
// at 6699 ms was the RECT's geometry (`r.3 >= CRYSTAL_H && …`), true on any 2880x34 bar whether or
// not a pixel of the gem reached the glass. `:: MENUFIRST: … crystal=absent … recorded=false` at
// 48497 ms was bit 35 of a reading that was never written — the same zero that printed
// `model=complete` and `after_enable_ms=0` — because all eight `compose()` tries declined on a
// refused leaf lock (`[strip] rollup tenant=menubar … decl_lock=16`, 0 at 48481 ms, 16 at 53483 ms and
// never again all boot) and zero `[menubar]` paints landed across the whole fixture (`paints=3`
// before and after). Neither line said anything about the glass. Both now read the PANEL.
// ═════════════════════════════════════════════════════════════════════════════════════════════════

/// CRYSTAL2 — the settle budget, ms. DOCKID2's `DOCKID_FOLD_WAIT_MS` (`video/dock.rs`), restated
/// because that constant is private to its module: the same Class 6 cure, the same bound.
#[cfg(feature = "witness")]
const SETTLE_BUDGET_MS: u64 = 250;

/// CRYSTAL2 — spins between two settle tries: DOCKID2's `DOCKID_FOLD_SPIN_MAX`, restated likewise.
#[cfg(feature = "witness")]
const SETTLE_SPIN: u32 = 4096;

/// CRYSTAL2 — **drive [`compose`] until `done` answers `true`, or [`SETTLE_BUDGET_MS`] runs out.**
///
/// Returns `(landed, tries, waited_us, decl_lock)` — the last being how many LEAF-LOCK declines the
/// menubar's strip census counted while this ran ([`strip::bar_decl_lock`]), so an unlanded settle
/// names its cause on the line instead of leaving it to a rollup five seconds later.
///
/// `done` is a PREDICATE ON PUBLISHED STATE, never `compose()`'s return value: a sibling core's
/// composite paints through this same `compose`, and a paint it lands is as much the bar's as one
/// this fixture drove — FIXTURE_FLAKES Class 6's rule (score what the publisher published, and say
/// so when it published nothing). Timed on the cycle counter through [`strip::cycles_to_us`], not on
/// `arch::ms()`: the tick needs interrupts, and a settle that cannot see time pass cannot end.
#[cfg(feature = "witness")]
fn paint_settle(done: impl Fn() -> bool) -> (bool, u32, u64, u64) {
    let t0 = crate::arch::now_cycles();
    let d0 = strip::bar_decl_lock("menubar");
    let waited = || strip::cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
    let mut tries = 0u32;
    let landed = loop {
        if done() {
            break true;
        }
        if waited() >= SETTLE_BUDGET_MS * 1000 {
            break false;
        }
        tries += 1;
        let _ = compose();
        if done() {
            break true;
        }
        for _ in 0..SETTLE_SPIN {
            core::hint::spin_loop();
        }
    };
    (landed, tries, waited(), strip::bar_decl_lock("menubar").saturating_sub(d0))
}

/// CRYSTAL2 — the readback's words, indexed by [`gem_class`]: the panel lock was refused, every
/// silhouette pixel in its ink with none outside and the mark mirror-symmetric, some but not all of
/// that, none of it.
#[cfg(feature = "witness")]
const GEM_WORDS: [&str; 4] = ["unread", "drawn", "broken", "absent"];

/// CRYSTAL2 — classify a [`crystal_readback`] into an index of [`GEM_WORDS`].
#[cfg(feature = "witness")]
fn gem_class(g: Option<(u32, u32, u32, bool)>) -> u64 {
    match g {
        None => 0,
        Some((m, w, 0, true)) if m == w && w > 0 => 1,
        Some((0, _, 0, _)) => 3,
        Some(_) => 2,
    }
}

/// CRYSTAL2 — [`gem_class`] as its word, for the `[menubar] first-paint` line.
#[cfg(feature = "witness")]
fn gem_word(g: Option<(u32, u32, u32, bool)>) -> &'static str {
    GEM_WORDS[gem_class(g) as usize]
}

/// CRYSTAL2 — **the crystal's 16x22 box read back off the PANEL**: `(matched, want, stray, sym)`, or
/// `None` when the panel lock was refused (masked and contended — [`super::panel_snapshot`]'s rule).
///
/// * `want` — silhouette pixels ([`crystal_facet`] answers `Some`); `matched` — those whose panel
///   pixel IS that ink. `matched == want` is "every pixel of the gem reached the glass".
/// * `stray` — pixels OUTSIDE the silhouette carrying one of the three gem inks: a mark drawn at the
///   wrong offset, or smeared, reads `stray > 0` even when every silhouette pixel happens to match.
/// * `sym` — the box is mirror-symmetric about its vertical centre line (shape, and the crown's
///   shadowed face opposite its lit one). Read off the PANEL, so it holds [`crystal_facet`] itself to
///   account, which `matched` cannot: `matched` compares the glass with the painter's own function.
///
/// 352 reads of the scan-out surface, ~1 µs each on the rMBP's write-combined aperture
/// (`framebuffer.rs`'s cost note), paid once per boot by the first-paint witness and once per paint
/// by [`crystal_persist_selftest`] — never on a steady-state composite.
#[cfg(feature = "witness")]
fn crystal_readback(r: strip::Rect) -> Option<(u32, u32, u32, bool)> {
    let fb = super::panel_snapshot()?;
    let (bx, by, _, _) = crystal_box(r);
    let (mut matched, mut want, mut stray, mut sym) = (0u32, 0u32, 0u32, true);
    for v in 0..CRYSTAL_H {
        // 0 = not a gem ink, 1 = CLOSE (shadowed crown face), 2 = MID (pavilion), 3 = ZOOM (lit face).
        let mut cls = [0u8; CRYSTAL_W];
        for u in 0..CRYSTAL_W {
            let px = fb.read_pixel(bx + u, by + v);
            cls[u] = match px {
                Some(theme::CONTROL_CLOSE) => 1,
                Some(theme::CONTROL_MID) => 2,
                Some(theme::CONTROL_ZOOM) => 3,
                _ => 0,
            };
            match crystal_facet(u, v) {
                Some(c) => {
                    want += 1;
                    if px == Some(c) {
                        matched += 1;
                    }
                }
                None => {
                    if cls[u] != 0 {
                        stray += 1;
                    }
                }
            }
        }
        // The mirror of a shadowed face is the lit one; the pavilion and the bar's face mirror to
        // themselves.
        for u in 0..CRYSTAL_W / 2 {
            if cls[CRYSTAL_W - 1 - u] != [0u8, 3, 2, 1][cls[u] as usize] {
                sym = false;
            }
        }
    }
    Some((matched, want, stray, sym))
}

/// CRYSTAL2 — which synthetic model [`crystal_persist_selftest`] has [`compose`] paint: `0` none (the
/// live model, always, outside the fixture), `1` the flight-12 first paint's model (no caption, no
/// clock, no battery), `2` a COMPLETE one, `3` the complete one a clock minute later.
#[cfg(feature = "witness")]
static PERSIST_MODEL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// CRYSTAL2 — the synthetic model for [`PERSIST_MODEL`] kind `k`. Every field is set, so its
/// [`Model::signature`] is a function of the kind and the rect alone and the fixture can compute the
/// signature a landed paint must leave in [`SLOT`].
#[cfg(feature = "witness")]
fn persist_build(kind: u8) -> Model {
    let mut m = Model::empty();
    if kind >= 2 {
        const CAP: &[u8] = b"Persist";
        m.title[..CAP.len()].copy_from_slice(CAP);
        m.title_len = CAP.len();
        m.clock = Some(if kind == 2 { *b"12:34" } else { *b"12:35" });
        m.batt = Some(super::status::BarItem { percent: 82, charging: true });
    }
    m
}

/// CRYSTAL2 — [`compose`]'s hook: replace the pass's model while the fixture holds a kind. One
/// relaxed load when it does not.
#[cfg(feature = "witness")]
#[inline]
fn persist_model(m: &mut Model) {
    let k = PERSIST_MODEL.load(Ordering::Relaxed);
    if k != 0 {
        *m = persist_build(k);
    }
}

/// CRYSTAL2 — what a model DRAWS, named in [`model_terms`]'s words but read off the model itself
/// (that function's `batt` term is the status SOURCE, which a synthetic model does not move).
#[cfg(feature = "witness")]
fn drawn_terms(m: &Model) -> &'static str {
    match (m.title_len > 0, m.clock.is_some(), m.batt.is_some()) {
        (true, true, true) => "complete",
        (false, false, false) => "partial:caption+clock+batt",
        _ => "partial:other",
    }
}

/// CRYSTAL2 fixture — **the crystal is on the glass after EVERY paint of the bar, whatever the bar's
/// model: a partial bar, then a complete one, then that complete one a clock minute later.**
///
/// The question flight 12 left open: `[menubar] first-paint` spoke on a PARTIAL bar
/// (`model=partial:caption+clock+batt`) and nothing spoke for the later, fuller ones — so a gem
/// drawn on the first paint and omitted by a later one would be a broken crystal the wire never saw.
/// The code answers it (`strip::paint` rewrites every row of the rect, and [`compose_row`] draws the
/// gem before any model-dependent branch), and both flights' censuses agree (`paint_px` is exactly
/// `paints x 97920` on all 17 `[strip] rollup tenant=menubar` lines of flights 11 and 12). This makes
/// that a MEASUREMENT: each paint is driven through the real [`compose`] and the real damage test
/// with the model swapped by [`persist_model`], settled on [`SLOT`] holding that model's signature
/// (a sibling core's paint counts), and then the gem's box is read back off the panel.
///
/// `-> PASS` iff all three paints landed and every one read `drawn` ([`GEM_WORDS`]). `-> SKIP` when a
/// paint did not land inside the budget or the panel lock was refused — Class 6: stated, never a
/// FAIL. GO-RED (a source mutation, B194): skip the gem in [`compose_row`] when the model has a clock
/// — i.e. on the second and third paints — and the line reads `present=1/3 … -> FAIL`.
///
/// Restores what it touched: the override is cleared, the band is handed back with the bar OFF, and
/// the enable flag is put back — so on a metal boot the next composite repaints the LIVE model.
#[cfg(feature = "witness")]
pub fn crystal_persist_selftest(pw: usize, ph: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let saved_en = enabled();
    // The synthetic clock is not the civil clock: keep `:: BARCLOCK:` from announcing it.
    #[cfg(feature = "sntp6")]
    let saved_clock_said = BARCLOCK_SEEN.fetch_or(0b11, Ordering::Relaxed);
    // Clear the slot first, so paint 1 is a PAINT and not the live bar already matching kind 1.
    set_enabled(false);
    let _ = compose();
    set_enabled(true);
    let rect = strip_rect(pw, ph);
    const KINDS: [u8; 3] = [1, 2, 3];
    let mut words = ["-"; 3];
    let mut tries = [0u32; 3];
    let (mut landed, mut present, mut matched, mut want, mut stray) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let (mut sym, mut unread, mut decl) = (true, 0u32, 0u64);
    if let Some(r) = rect {
        for (i, &k) in KINDS.iter().enumerate() {
            let m = persist_build(k);
            words[i] = drawn_terms(&m);
            let want_sig = m.signature(r);
            PERSIST_MODEL.store(k, Ordering::Release);
            let s = paint_settle(|| SLOT.sig() == want_sig && SLOT.packed() == strip::pack_rect(Some(r)));
            tries[i] = s.1;
            decl += s.3;
            if !s.0 {
                continue;
            }
            landed += 1;
            let g = crystal_readback(r);
            match g {
                Some((mt, w, st, y)) => {
                    matched += mt;
                    want += w;
                    stray += st;
                    sym &= y;
                }
                None => unread += 1,
            }
            if gem_class(g) == 1 {
                present += 1;
            }
        }
    }
    PERSIST_MODEL.store(0, Ordering::Release);
    set_enabled(false);
    let _ = compose(); // hand the band back; the next enabled pass repaints the LIVE model
    set_enabled(saved_en);
    #[cfg(feature = "sntp6")]
    BARCLOCK_SEEN.store(saved_clock_said, Ordering::Relaxed);
    let n = KINDS.len() as u32;
    let verdict = if rect.is_none() || landed < n || unread > 0 {
        "SKIP"
    } else if present == n {
        "PASS"
    } else {
        "FAIL"
    };
    serial_println!(
        "[menubar] crystal-persist paints={} present={}/{} models={},{},{} gem_px={}/{} stray={} sym={} \
         tries={},{},{} decl_lock={} unread={} budget_ms={} -> {}",
        landed, present, landed, words[0], words[1], words[2], matched, want, stray, sym,
        tries[0], tries[1], tries[2], decl, unread, SETTLE_BUDGET_MS, verdict
    );
}
