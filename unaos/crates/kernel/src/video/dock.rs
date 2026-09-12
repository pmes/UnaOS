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

//! DOCK — the bottom strip, and the guarantee that **every window has a way back**.
//!
//! # Why it exists
//!
//! Peter's ruling, white board Q10, 2026-08-09, verbatim:
//!
//! > *"i guess mac has had the dock forever so we should have a doc and all macos like experience.
//! > remember we are trying to make mac users comfortable with unaos/crispy we are pretending to be
//! > a normal OS. crispy is meant to be an amalgamation of macos over the years"*
//!
//! and the standing performance priority from the same board: *"just make the os high performance
//! if looks a little off we will change it"*.
//!
//! # What it is, and what it deliberately is NOT
//!
//! It is a **window switcher**. One tile per live window — *including the windows that are not on
//! the panel* — with a press that raises and un-hides the window it names. That is the load-bearing
//! job: [`super::wm`] expresses "minimised" as a POSITION (a row whose `z` is below
//! [`super::wm::shell_z`] does not composite at all, see `wm::above_shell`), and until this module
//! there was no gesture that could bring such a row back except `<TAB>`. A window the operator can
//! send away and cannot call back is a window they have lost.
//!
//! It **carries no app grid.** `docs/dev/OS/ARCHITECTURE.md` is explicit that UnaOS avoids
//! fixed-feature apps, and the standing instruction is not to build them. What it does carry, since
//! R49/R50 (2026-09-12), is a fixed set of PINNED APPS — Quarry (the Finder), the console and the
//! shell — whose tiles launch a fresh instance through each app's own mint seam; see APPPIN below.
//!
//! # Two panels now, and gated on each (PI-DESK)
//!
//! `#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature =
//! "desktop_firmware")))]` at the `mod` declaration in [`super`]. The composite seam in `wm::composite_once`
//! and the press seam in each arch's `syscall.rs` carry the same gate, so a knob-off build of EITHER
//! arch has neither — and a knob-off `kernel8.img` is byte-identical to the pre-arc image, measured.
//!
//! **The dock on the Pi is the dock, not a port of it.** One file, one tile model, one press rule.
//! What crossing cost was a single seam — [`focus_set`]/[`focus_get`] below, because the keyboard
//! designation lives in the per-arch syscall layer where the input rings are — and one line of the
//! router, which both arches reach through ONE shared entry point in [`super::strip`] rather than
//! through two copies of the ordering rule.
//!
//! Peter's Q10 ruling travels with it unchanged: **a window switcher with pinned apps, no app
//! grid.** The pinned set is the same on every board, and each tile launches through the seam the
//! app's own boot bring-up used — there is no dock-private launch path on either arch.
//!
//! # Materials and metrics — every value is a Crispy role or a Crispy metric
//!
//! **No new theme role was needed and none was invented.** The strip is the same object as the
//! window chrome and is machined from the same material:
//!
//! | element | colour | metric |
//! |---|---|---|
//! | strip face | [`theme::CHROME_FACE`] under [`ceramic::shade`] | height `2*GAP + BUTTON_HEIGHT` |
//! | strip keyline | [`theme::FRAME_LINE`] | 1 px, radius [`theme::CORNER_RADIUS`] |
//! | strip top bevel | [`theme::BEVEL_LIGHT`] | [`theme::BEVEL`] px |
//! | tile face | [`theme::BUTTON_FACE`] / [`theme::BUTTON_FACE_PRESSED`] under [`ceramic::shade_gain`] at [`ceramic::CONTROL_GAIN_Q16`] | [`theme::BUTTON_HEIGHT`] x auto, radius [`theme::WIDGET_RADIUS`] |
//! | caption ink | [`theme::TITLE_TEXT_ACTIVE`] / [`theme::TITLE_TEXT_INACTIVE`] | [`super::wm::TITLE_CELL`] |
//! | running indicator | [`theme::ACCENT`] (on the panel) / [`theme::SCROLL_THUMB`] (minimised) | `theme::GAP / 2` |
//! | padding, gaps, bottom margin | — | [`theme::GAP`] |
//!
//! The two DERIVED numbers, both stated here rather than buried: the indicator's diameter is
//! `theme::GAP / 2` (half the padding band the pip sits in — the dot is a status pip, not a
//! control, and must not read as one; see [`IND_D`] for why KNURL moved it off `CONTROL_BOX`);
//! and the tile takes the material at [`ceramic::CONTROL_GAIN_Q16`] rather than full gain, for
//! ceramic's own stated reason — a tile is a small saturated object and the grain competes with it.
//! Neither is dressed up as a kit citation. `kits/crispy/` is not in this repo and nothing here
//! pretends to have read it.
//!
//! # STRIPFACTOR — the dock is now TENANT #1 of [`super::strip`], and unchanged by being one
//!
//! Peter's 2026-08-11 direction (*"UnaOS is a spatial game-engine OS … we will not always have a
//! menu bar"*) made the kernel's contribution the STRIP MECHANISM rather than any particular strip.
//! Everything below that was general — edge-anchored geometry with floors, the staged row-run
//! painter, the vacated-pixel erase, the damage slot, the cost ledger, the rounded-corner and disc
//! arithmetic, and occlusion citizenship — moved to [`super::strip`] and is shared with
//! [`super::menubar`]. What stayed here is everything a DOCK is and a strip is not: the tile model,
//! the tile arithmetic, the caption budget, the running pip, and the raise-and-unhide press.
//!
//! **Nothing about the dock's behaviour moved with it.** The tile geometry, the damage conditions,
//! the paint order, the colours, and the press routing are the same code reading the same constants;
//! the primitive received the machinery verbatim rather than a reimplementation of it. Two things
//! about the ARTIFACT did change, and are disclosed rather than implied away:
//!
//!  * the not-word4 decline line is emitted by the primitive for every tenant, so it reads
//!    `[strip] decline reason=not-word4` where it read `[dock] decline reason=not-word4`;
//!  * [`super::strip::MAX_STRIP_W`] is 4096, not this module's old 2048, because a FLUSH tenant is
//!    the panel's full width and the bench panels reach 2880. The scratch is 32 KiB of `.bss`, up
//!    from 16. The dock's own `const` proof that its worst-case layout fits is unchanged and still
//!    checked below.
//!
//! # Performance — damage-driven, and measured
//!
//! **The dock does not repaint per frame.** [`compose`] runs at the tail of every composite pass and
//! repaints only when one of two things is true:
//!
//!  1. the tile model CHANGED — a different window set, a different caption, a different
//!     visible/focused/pressed state. Reduced to one `u64` FNV-1a signature, so the test is an
//!     integer compare against the last painted state, not a redraw;
//!  2. the pass PAINTED OVER the strip — a damaged, visible window whose outer box intersects the
//!     dock rect. That question is answered inside `wm`'s own table scan
//!     ([`super::wm::dock_scan`]), so it costs no second lock and no second walk.
//!
//! A quiet desktop therefore pays exactly: one `wm::dock_scan` (a bounded `MAX_WINDOWS` row scan,
//! the same shape as `focus_ring`), one signature hash over at most 12 short rows, one compare, and
//! a return. No framebuffer read, no framebuffer write, no allocation.
//!
//! The cost of both halves is COUNTED, in cycles, and put on the wire by [`rollup`] — `scan_cyc` is
//! what every pass pays, `paint_cyc` what a repaint costs — so "what did the dock cost per
//! composite" is a number in the capture rather than an estimate.
//!
//! # Front-buffer discipline (WC-H / WC-K / WC-L)
//!
//! The standing law in this subsystem is that nothing writes the live scan-out per-pixel: a painter
//! composes in CACHED RAM and copies out as contiguous row runs. The dock honours it. Each panel row
//! of the strip is composed into a cached scratch row and copied out with one `FrameBuffer::blit`
//! (a row of the strip is contiguous), and the whole strip is cleaned once with `flush_rect`. That
//! is `wm::stage_fill`'s shape at one-row granularity; the scratch is 2 x 6.4 KiB of `.bss` rather
//! than a whole-strip buffer, which is what keeps it affordable.
//!
//! The sprite is bracketed the way every other non-compositor painter in this subsystem brackets it
//! ([`super::wm::erase`]'s rule): [`compose`] takes the arrow off the panel with `cursor::undraw()`
//! BEFORE the first byte lands, and reports `true` so its caller upgrades the pass's cursor tail to
//! `Repaint`. A dock repaint therefore can never leave the sprite's save-under holding dock pixels.
//!
//! # Hit-testing follows drawing BY CONSTRUCTION
//!
//! There is exactly ONE tile-geometry accessor, [`Layout`], and both the painter ([`paint`]) and the
//! click router ([`press_at`]) obtain their rectangles from it. There is no second copy of the tile
//! arithmetic to drift — the law crispywire established after `controls` and `paint_window` disagreed
//! by one `GAP`. [`selftest`] asserts the two agree by driving a synthetic press at a tile centre
//! that [`Layout`] itself computed and checking WHICH window came back.
//!
//! # APPPIN — the console and the shell are APPS with PERMANENT tiles (R49 / R50, 2026-09-12)
//!
//! Peter, verbatim in RULINGS.md R49: *"console should be an app that is pinned to the taskbar not
//! this mystery thing that appears"* · *"same with shell"*; R50: *"quarry is our finder."*
//!
//! The model is macOS's. A pinned app's tile is ALWAYS on the strip. While the app has a live window
//! the tile IS that window's row (pip lit; a press raises it through the ordinary kernel-owner arm).
//! While it has none, the tile is a synthetic PIN in the same slot ([`SHELL_PIN_ID`],
//! [`CONSOLE_PIN_ID`], [`QUARRY_PIN_ID`]; pip in the minimised ink), and a press on it LAUNCHES a
//! fresh instance. Closing a window — the red disc, the app menu's Quit, `wc_close_furniture` — is a
//! QUIT: `wm::close` frees the row, and the body that owns the instance's state (its surface store,
//! its `Console`, its `Screen`/`TargetPal`) tears that state down on its next pass when it sees the
//! row gone. Nothing is kept back for a "reopen": there is no reopen route, no re-mint over old
//! pixels, no latch that remembers a dead id. A relaunch mints a NEW window with a NEW id and
//! generation through the SAME function the boot used for the first one — `open_shell_window` /
//! `tegra_shell_window_open` for the shell, `fbcon::panel_console_window_open` for the console —
//! exactly as QUARRY-LAUNCH mints a program's window through the seam `bg` takes.
//!
//! What this module owns of that: the tiles ([`pin_console`], [`pin_shell`], [`pin_quarry`]); the
//! press ([`press_at`] POSTS a launch for the app the tile names — [`PinnedApp`], [`take_launch`]);
//! the console's launch service ([`console_launch_service`], arch-neutral because the console's mint
//! seam is `fbcon`'s); and the ACCOUNTING every launch and quit reports through ([`app_launched`],
//! [`app_quit`]), so one wire grammar covers all three render bodies:
//!
//! ```text
//! [dock] press at (x,y) tile=t/n app=shell -> launch
//! [dock] launch app=shell by=x86_render_service win=3 gen=2 tries=1 -> LAUNCHED
//! [dock] quit app=shell by=x86_render_service win=3 gen=2 -> TORN-DOWN
//! ```
//!
//! Why the press POSTS rather than mints: it runs inside the click router, which may neither take a
//! blocking panel lock nor allocate a multi-megabyte surface (LOCKFIX `7847ceea`); Quarry's tile
//! defers the same way (`quarry::request_open` → `quarry::service`). A posted launch is a queue of
//! one per app, not a state machine: it carries no window id and no "was open".
//!
//! Quarry is the Finder (R50): its tile is permanent for the same reason, its module owns its request
//! seam and its window, and this arc changed nothing there.
//!
//! The pinned tiles' occlusion accounting — `wm::dock_tiles` counting every pin the painter paints,
//! through the one [`pins_applied`] fold — is the `4c6ca42d` fix and still holds.

use super::{ceramic, strip, theme, wm};

// ---------------------------------------------------------------------------
// PI-DESK — the ONE arch seam this module carries
// ---------------------------------------------------------------------------
//
// A tile press hands the KEYBOARD to the window it raised, and the keyboard designation lives in the
// per-arch syscall layer (`USER_INPUT_ACTIVE`) because that is where the input rings are. Every other
// line of this file is arithmetic over `wm` and the materials, and is arch-neutral by construction;
// these two functions are the whole of what is not. They are seamed HERE, at the one call site each,
// rather than by inventing a `crate::arch::user_input_*` re-export — the same shape `wm` uses for its
// `desktop_uefi` reach, and it keeps the fact that the two arches have separate input tables visible instead of
// papering over it.
//
// The contract is identical on both sides: `0` means the SHELL (a kernel-owned row has no input ring,
// so the router's furniture rule hands the keyboard to the shell rather than to a program that cannot
// read it), and any other value is an owning ASID.
//
// ⚠ **THERE ARE THREE STATES HERE, NOT TWO — and writing only two is what broke the build (SMALLS3).**
// The seam was written as an exhaustive `x86_64` / `aarch64` pair on the unstated premise that every
// build of one of those arches HAS a `syscall` module. On aarch64 that is false: `arch/aarch64/mod.rs`
// gates `pub mod syscall;` behind `any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0")` — the input rings
// belong to the EL0 layer, and a build without it has no ring table to designate into. So this module's
// own gate (`desktop_firmware`) armed with NONE of those three features named the module path in a
// build where the module does not exist, and both functions failed E0433. No coverage leg reached the
// combination (`arm-pi` always carries `baremetal`, `arm-tegra-desk` always carries `tegra_el0`), which
// is how it shipped.
//
// The third arm is a NO-OP, and that is the correct semantics rather than a placeholder: with no input
// ring table there is no keyboard destination to move, and the read side's `0` is not a stand-in — `0`
// IS the contract's value for "the shell has it", which is exactly true when no program can be handed
// a keystroke. The same shape and the same reasoning are already in `wm::vugmin_publish` (per-arch
// dispatch arms, each naming the feature that owns that arch's info page, plus a fall-through that
// consumes the argument) and in `wm::repaint`'s `focused` (`0` where the router is not compiled), so
// this follows the file next door instead of inventing a third convention.
//
// The two live arms are UNCHANGED in every configuration that compiled before: the conjunct added here
// is the module's own gate, verbatim, so the emitted code on `arm-pi`, `arm-tegra-desk` and every x86
// build is byte-identical.

// The conjunct is spelled out twice below rather than aliased — `cfg` attributes cannot be given a
// name — and the two copies must stay identical to EACH OTHER. `arch/aarch64/mod.rs` now gates
// `pub mod syscall;` behind `aarch64_el0`, which BOTH longhand terms imply, so the spelling here can
// only under-claim the module's presence, never name a module that is absent. A narrower copy
// silently stops designating focus on a board that has a ring table; a wider one is the E0433
// above, back again.

/// Designate `asid` as the keyboard's destination (`0` = the shell).
#[inline]
fn focus_set(asid: u64) {
    #[cfg(target_arch = "x86_64")]
    crate::arch::x86_64::syscall::user_input_set_active(asid);
    #[cfg(all(
        target_arch = "aarch64",
        any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0") // EL0-NAMING: POSITIVE LEG PAIRED WITH A NEGATED TWIN — KEPT LONGHAND ON PURPOSE. The negated three-state twin below must stay longhand (Cargo feature implication is ONE-WAY: `baremetal`/`tegra_el0` imply `aarch64_el0`, not the reverse, so `not(aarch64_el0)` would diverge from that predicate for anyone who enabled `aarch64_el0` ALONE), and this pair's correctness rests on the two spellings staying byte-identical — renaming this leg alone would split them. So it follows its twin, not the rename.
    ))]
    crate::arch::aarch64::syscall::user_input_set_active(asid);
    // No ring table in this build: nothing to designate into, and the argument still has to be consumed.
    #[cfg(not(any(
        target_arch = "x86_64",
        all(
            target_arch = "aarch64",
            any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0") // EL0-NAMING: NEGATED/RUNTIME — KEPT LONGHAND ON PURPOSE. Cargo feature implication is ONE-WAY: `baremetal`/`tegra_el0` imply `aarch64_el0`, not the reverse, so `not(aarch64_el0)` would diverge from this predicate for anyone who enabled `aarch64_el0` ALONE. No gate leg builds that combination, which is the trap — a byte-identity check over the legs would PASS while the hazard shipped. Positive sites are safe because implication runs their way; these are not.
        )
    )))]
    let _ = asid;
}

/// The ASID the keyboard is currently designated to (`0` = the shell) — the read side of
/// [`focus_set`], used by the fixture to save and restore the standing designation.
#[cfg(feature = "witness")]
#[inline]
fn focus_get() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::syscall::user_input_active()
    }
    #[cfg(all(
        target_arch = "aarch64",
        any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0") // EL0-NAMING: POSITIVE LEG PAIRED WITH A NEGATED TWIN — KEPT LONGHAND ON PURPOSE. The negated three-state twin below must stay longhand (Cargo feature implication is ONE-WAY: `baremetal`/`tegra_el0` imply `aarch64_el0`, not the reverse, so `not(aarch64_el0)` would diverge from that predicate for anyone who enabled `aarch64_el0` ALONE), and this pair's correctness rests on the two spellings staying byte-identical — renaming this leg alone would split them. So it follows its twin, not the rename.
    ))]
    {
        crate::arch::aarch64::syscall::user_input_active()
    }
    // No ring table in this build: `0` is the contract's own value for "the shell holds it", which is
    // what "no program can be handed a keystroke" means. The save/restore the fixture does around this
    // is then a well-typed round trip of the shell designation, not a lie about a table that is absent.
    #[cfg(not(any(
        target_arch = "x86_64",
        all(
            target_arch = "aarch64",
            any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0") // EL0-NAMING: NEGATED/RUNTIME — KEPT LONGHAND ON PURPOSE. Cargo feature implication is ONE-WAY: `baremetal`/`tegra_el0` imply `aarch64_el0`, not the reverse, so `not(aarch64_el0)` would diverge from this predicate for anyone who enabled `aarch64_el0` ALONE. No gate leg builds that combination, which is the trap — a byte-identity check over the legs would PASS while the hazard shipped. Positive sites are safe because implication runs their way; these are not.
        )
    )))]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// Metrics — every one a `theme` name, or derived from one with the derivation
// written out. Nothing here is a bare literal with a look chosen for it.
// ---------------------------------------------------------------------------

/// Padding inside the strip, gap between tiles, and the strip's margin off the panel's bottom edge —
/// all [`theme::GAP`], the kit's one "standard gap between controls", by way of the primitive's
/// [`strip::PAD`] so a strip's margin and a tenant's padding cannot drift apart.
const PAD: usize = strip::PAD;

/// A tile's height — [`theme::BUTTON_HEIGHT`]. A dock tile IS a button by the kit's own taxonomy: a
/// raised control with a label that does something when pressed.
const TILE_H: usize = theme::BUTTON_HEIGHT;

/// A tile's corner radius — [`theme::WIDGET_RADIUS`], the kit's radius "for widgets (buttons and
/// other raised controls)".
const TILE_R: usize = theme::WIDGET_RADIUS;

/// The strip's corner radius — [`theme::CORNER_RADIUS`], the same radius the window head is cut with,
/// so the dock reads as the same fabrication as the chrome.
const STRIP_R: usize = theme::CORNER_RADIUS;

/// The strip's height: a tile with the standard gap above and below it.
///
/// `pub` since STRIPFACTOR: [`super::menubar`]'s floor is derived from it (the bar must not crowd the
/// dock off a short panel), and a second copy of this arithmetic there is exactly the drift the
/// single-accessor law forbids.
pub const STRIP_H: usize = TILE_H + 2 * PAD;

/// The running indicator's diameter, px.
///
/// DERIVED, and stated as derived: `theme::GAP / 2` — half the padding band the pip sits in.
///
/// ⚠ **RE-DERIVED, same VALUE (6 px), by KNURL.** It was `theme::CONTROL_BOX / 2`, on the argument
/// that *"a pip the size of a control READS as a control, and a dot the operator tries to click is
/// worse than no dot"*. Peter's size ruling then took `CONTROL_BOX` from 12 to 24, which broke that
/// derivation in both directions at once: arithmetically it violated the `IND_D < PAD` assertion
/// below (12 is not less than `GAP` = 12, a BUILD failure), and semantically it produced a pip of
/// exactly the OLD control's diameter — i.e. the one outcome the halving existed to prevent.
///
/// So the pip is now derived from the band that actually bounds it, `PAD` = `theme::GAP`, which is
/// the constraint the assertion states and is independent of how large a control disc becomes. The
/// rendered pip is unchanged at 6 px; only its provenance moved. This is the sole line of the dock
/// module the size ruling touched, and it was touched because the alternative was a red tree.
const IND_D: usize = theme::GAP / 2;

/// The glyph advance and cell height the caption is drawn at — [`wm::TITLE_CELL_W`] /
/// [`wm::TITLE_CELL_H`], the shared anti-aliased face's own metrics, exactly as the window caption
/// resolves them. One definition, so a face change moves the window caption and the dock caption
/// together. FONT (GR27): the cell stopped being square with the 1-bit bitmap's retirement, so the
/// two axes are named separately — widths budget in `CELL_W`, vertical centring in `CELL_H`.
const CELL_W: usize = wm::TITLE_CELL_W;
const CELL_H: usize = wm::TITLE_CELL_H;

/// FONT-METRIC — the atlas those metrics come from, named once so the tile's width budget and its
/// glyph call can never disagree about which face the caption is drawn in.
const FACE: super::font::Face = super::font::Face::Chrome;

/// The longest caption a tile will ever show, in glyphs. Bounded by [`wm::MAX_TITLE`]; capped at 8
/// because a dock is a row of many small things and a tile wide enough for a whole 16-byte title
/// would let four windows fill a 1920 panel.
const LABEL_MAX: usize = 8;

/// The widest strip the primitive will compose — [`strip::MAX_STRIP_W`], shared with every tenant.
///
/// The dock's own worst case is unchanged and is still what the `const` proof below checks: a full
/// table of [`wm::MAX_WINDOWS`] tiles at [`LABEL_MAX`] glyphs is
/// `2*PAD + 12*(2*PAD + 8*CELL_W) + 11*PAD` px — comfortably inside the shared bound at the
/// face's 7 px advance. STRIPFACTOR raised the
/// shared bound to 4096 for the flush tenants; the dock neither needs nor is affected by the extra
/// width, and it no longer owns the scratch that provides it.
const MAX_STRIP_W: usize = strip::MAX_STRIP_W;

/// The layout cannot ask for a strip the scratch cannot hold. A `const` proof rather than a runtime
/// clamp, so a future `LABEL_MAX` or `MAX_WINDOWS` raise fails the BUILD.
const _: () = {
    assert!(2 * PAD + wm::MAX_WINDOWS * (2 * PAD + LABEL_MAX * CELL_W) + (wm::MAX_WINDOWS - 1) * PAD
        <= MAX_STRIP_W);
    // The caption must fit inside the tile it is centred in, or there is nothing to draw.
    assert!(CELL_H <= TILE_H);
    // The indicator must fit in the padding band below the tile.
    assert!(IND_D < PAD);
    // Both of a tile's corners must fit within its own height — the kit asserts this for buttons and
    // a tile IS a button; restated here because the tile is the object being cut.
    assert!(2 * TILE_R <= TILE_H);
    assert!(2 * STRIP_R <= STRIP_H);
    assert!(LABEL_MAX <= wm::MAX_TITLE);
};

// ---------------------------------------------------------------------------
// Layout — THE ONE geometry accessor. Painter and router both read it.
// ---------------------------------------------------------------------------

/// The dock's geometry for a given tile count and panel.
///
/// **The single source of tile arithmetic.** [`paint`] draws from it and [`press_at`] routes from it;
/// neither computes a rectangle of its own. Copy this arithmetic into a second place and the painter
/// and the router can disagree, which is exactly the defect crispywire convicted in `wm` (`controls`
/// and `paint_window` kept separate copies and differed by one `GAP`, so the threshold admitted
/// strips the painter then gave a zero-glyph budget).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The strip's outer box on the panel.
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    /// Tiles across.
    pub n: usize,
    /// One tile's width.
    pub tile_w: usize,
    /// Caption budget in glyphs, chosen so the strip fits the panel.
    pub glyphs: usize,
}

impl Layout {
    /// The layout for `n` tiles on a `pw` x `ph` panel, or `None` when there is nothing to draw
    /// (no tiles) or nowhere to draw it (a panel too narrow for even one-glyph tiles, or too short
    /// for the strip and its margin).
    ///
    /// The caption budget is chosen by trying [`LABEL_MAX`] glyphs and stepping down until the whole
    /// strip fits between two [`PAD`] margins — auto-sizing to the CONTENTS, with the panel as the
    /// only cap. Deterministic, integer, at most eight iterations.
    pub fn for_panel(n: usize, pw: usize, ph: usize) -> Option<Layout> {
        if n == 0 || n > wm::MAX_WINDOWS {
            return None;
        }
        let mut glyphs = LABEL_MAX;
        loop {
            let tile_w = 2 * PAD + glyphs * CELL_W;
            let w = 2 * PAD + n * tile_w + (n - 1) * PAD;
            // STRIPFACTOR — the anchoring, the margin and BOTH floors are the primitive's
            // `frame_centred`: `ph < STRIP_H + 2*PAD`, `w + 2*PAD > pw` and `w > MAX_STRIP_W` were
            // three separate tests here and are the same three there, in the same order, against the
            // same constants. The step-down loop stays, because auto-sizing the caption to the panel
            // is the DOCK's arithmetic, not any strip's.
            if let Some((x, y, w, h)) = strip::frame_centred(strip::Edge::Bottom, w, STRIP_H, pw, ph)
            {
                return Some(Layout { x, y, w, h, n, tile_w, glyphs });
            }
            if glyphs == 1 {
                return None; // even one glyph per tile will not fit: draw no dock at all.
            }
            glyphs -= 1;
        }
    }

    /// Tile `i`'s box on the panel, or `None` for an index past the tile count.
    #[inline]
    pub fn tile(&self, i: usize) -> Option<(usize, usize, usize, usize)> {
        if i >= self.n {
            return None;
        }
        Some((
            self.x + PAD + i * (self.tile_w + PAD),
            self.y + PAD,
            self.tile_w,
            TILE_H,
        ))
    }

    /// The tile index containing the panel point, or `None`. The ROUTER's whole geometry question,
    /// answered from the same fields the painter draws from.
    ///
    /// A point inside the STRIP but between two tiles answers `None` for the index while the caller's
    /// `contains` still answers `true` — the press is consumed by the dock (it landed on the dock)
    /// and raises nothing, which is what a press on the dock's own background should do.
    #[inline]
    pub fn tile_at(&self, px: usize, py: usize) -> Option<usize> {
        for i in 0..self.n {
            let (tx, ty, tw, th) = self.tile(i)?;
            if px >= tx && px < tx + tw && py >= ty && py < ty + th {
                return Some(i);
            }
        }
        None
    }

    /// Does the strip contain this panel point? Rounded corners included: a press on a cut corner is
    /// a press on whatever is behind the dock, exactly as `wm::hit_test` treats a window's cut head
    /// corners.
    #[inline]
    pub fn contains(&self, px: usize, py: usize) -> bool {
        strip::contains(self.rect(), STRIP_R, px, py)
    }

    /// The strip as a plain rect, for the damage question.
    #[inline]
    pub fn rect(&self) -> (usize, usize, usize, usize) {
        (self.x, self.y, self.w, self.h)
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// APPPIN — the pinned shell tile's synthetic window id.
///
/// Real ids are `1..=wm::MAX_WINDOWS` and `wm::WIN_NONE` is 0 — which is also [`PRESSED`]'s idle
/// value, so a pinned tile carrying `WIN_NONE` would paint PRESSED on every quiet pass. `u32::MAX`
/// collides with neither and can never name a live row, which is the point: a press that resolves to
/// this id has nothing to raise and is a LAUNCH of the shell app instead.
const SHELL_PIN_ID: wm::WinId = wm::WinId::MAX;

/// QUARRY-PIN — the pinned file-manager tile's synthetic window id. One below [`SHELL_PIN_ID`] on the
/// same argument: real ids are `1..=wm::MAX_WINDOWS` and `wm::WIN_NONE` is 0, so neither sentinel can
/// ever name a live row or collide with [`PRESSED`]'s idle value. Declared in both feature polarities
/// (a `const` costs nothing) so the press arm below reads the same either way.
const QUARRY_PIN_ID: wm::WinId = wm::WinId::MAX - 1; const PULSE_PIN_ID: wm::WinId = wm::WinId::MAX - 2; // A30 — the pulse instrument's pinned reopen id, one below quarry's on the same argument (real ids are 1..=wm::MAX_WINDOWS, WIN_NONE is 0, so no sentinel can name a live row or collide with PRESSED's idle value). Folded, not added — PARITY.md §5.3.

// ---------------------------------------------------------------------------
// APPPIN — the pinned apps and their launch requests (R49 / R50, 2026-09-12)
// ---------------------------------------------------------------------------

/// APPPIN — the pinned apps this module launches and accounts for.
///
/// Quarry is the third pinned tile and the Finder (R50) — its module owns its request seam
/// (`quarry::request_open` / `quarry::service`) and its window, so it is not in this enum; nor is
/// the pulse instrument's tile (`pulsewin::arm`). The two here are the apps whose windows are minted
/// by a RENDER BODY that owns their state, which is why their launch is posted here and drained
/// there, and why the accounting that names both halves lives in one place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PinnedApp {
    /// The console — the kernel log in a window. Mint seam `fbcon::panel_console_window_open`.
    Console,
    /// The shell — a `wm::KERNEL_OWNER_DESKTOP` window over the render body's own `Console`.
    /// Mint seam `open_shell_window` (x86, Pi) / `tegra_shell_window_open` (the cascaded scene).
    Shell,
}

impl PinnedApp {
    /// The request bit in [`LAUNCH_OWED`].
    fn bit(self) -> u32 {
        match self {
            PinnedApp::Console => 1,
            PinnedApp::Shell => 2,
        }
    }
    /// The accounting slot in [`APP_LAUNCHES`] / [`APP_QUITS`].
    fn slot(self) -> usize {
        match self {
            PinnedApp::Console => 0,
            PinnedApp::Shell => 1,
        }
    }
    /// The app's name on the wire — also its tile caption and, per R36, its window title's stem.
    pub fn word(self) -> &'static str {
        match self {
            PinnedApp::Console => "console",
            PinnedApp::Shell => "shell",
        }
    }
}

/// APPPIN — launches posted by a tile press and not yet drained: one bit per app.
///
/// A queue of ONE per app, deliberately: two presses before the owning body's next pass are one
/// launch, not two (one live instance per pinned app — the x86 render service owns exactly one shell
/// tuple, the console has exactly one route). It remembers nothing about any window: there is no id
/// in it, no generation, no "was open" — the reopen latches this replaced (`SHELL_REOPEN`,
/// `CONSOLE_REOPEN`, SO1/SO10/SO13/SO17) are gone, not renamed.
static LAUNCH_OWED: AtomicU32 = AtomicU32::new(0);

/// APPPIN — post a launch for `app`. Called by [`press_at`] on the app's pin tile, inside the click
/// router, which may neither allocate a surface nor take a panel lock (LOCKFIX `7847ceea`) — so the
/// mint waits for the owning body's next pass, the same deferral Quarry's tile takes.
fn post_launch(app: PinnedApp) {
    LAUNCH_OWED.fetch_or(app.bit(), Ordering::AcqRel);
}

/// APPPIN — take (and clear) a posted launch for `app`. One `AcqRel` RMW on a quiet pass.
///
/// Drained by the body that owns the app's instance: the console by [`console_launch_service`]
/// (every render body calls it, it is arch-neutral); the shell by the render body that owns the
/// shell tuple (`x86_render_service`, the Pi `render_service` mint arm, the cascaded scene's console
/// pump), each of which mints through the SAME function its own bring-up used.
pub fn take_launch(app: PinnedApp) -> bool {
    LAUNCH_OWED.fetch_and(!app.bit(), Ordering::AcqRel) & app.bit() != 0
}

/// SHELLPIN — append the PERMANENT shell tile to a scanned model, iff no live row already carries
/// `wm::KERNEL_OWNER_DESKTOP` (one live shell window max — a live shell's REAL tile is its raise
/// route and a second tile would be a second shell). Returns the new count. Applied by every reader
/// of the model — [`compose`], [`press_at`], [`strip_rect`] — so painter, router and occlusion
/// registry cannot disagree about the tile count.
fn pin_shell(rows: &mut [wm::DockEntry; wm::MAX_WINDOWS], n: usize) -> usize {
    // PINCOUNT — the condition lives ONCE (`pin_shell_wanted`, this file's tail); this is its only
    // mutator, and `pins_applied` folds the same predicate for the two count-only readers.
    if !pin_shell_wanted(n, &|o| rows[..n].iter().any(|r| r.owner_asid == o)) {
        return n;
    }
    let mut e = wm::DockEntry::empty();
    e.id = SHELL_PIN_ID;
    e.owner_asid = wm::KERNEL_OWNER_DESKTOP;
    e.title[..5].copy_from_slice(b"shell");
    e.title_len = 5;
    // `visible: false` — the closed shell is OFF the panel, so the pip takes the minimised ink
    // (`theme::SCROLL_THUMB`), which is the honest state and the same read a parked window gives.
    e.visible = false;
    e.focused = false;
    rows[n] = e;
    n + 1
}

/// QUARRY-PIN — the FILE MANAGER's permanent tile, and the dock's first PINNED APP.
///
/// Peter's direction, 2026-08-17: Quarry is *"pinned to the left side of the taskbar/dock so it opens
/// like Mac's Finder"*. So this one PREPENDS rather than appends — Finder is the leftmost tile on
/// macOS and the ask names that position specifically — and it is applied after [`pin_shell`] so the
/// settled strip reads `[quarry] [live windows…] [shell]`, which is the macOS order (leftmost pinned
/// app, switcher in the middle, the permanent tail on the right).
///
/// ### This does not make the dock a launcher
///
/// The module header's ruling stands and is not being quietly reversed: there is exactly one launch
/// path in this kernel (the shell's program source / `bg`) and this adds no second one. Quarry is not
/// a program — it is a KERNEL-OWNED window, the same class of furniture as the console window, and
/// this tile is its REOPEN route for exactly the reason [`pin_shell`] is the shell's: a window an
/// operator can close and cannot call back is a window they have lost. When Quarry is LIVE its real
/// row is dock-addressable (kernel owners are), so the model is unchanged and no pin is added — the
/// pin exists only while it is closed, precisely as the shell's does.
///
/// Returns the new count. Applied by every reader of the model — [`compose`], [`press_at`],
/// [`strip_rect`] — so painter, router and occlusion registry cannot disagree about the tile count.
#[cfg(feature = "quarry")]
fn pin_quarry(rows: &mut [wm::DockEntry; wm::MAX_WINDOWS], n: usize) -> usize {
    // PINCOUNT — the condition lives ONCE (`pin_quarry_wanted`, this file's tail); this is its only
    // mutator, and `pins_applied` folds the same predicate for the two count-only readers.
    if !pin_quarry_wanted(n, &|o| rows[..n].iter().any(|r| r.owner_asid == o)) {
        return n;
    }
    let mut e = wm::DockEntry::empty();
    e.id = QUARRY_PIN_ID;
    e.owner_asid = crate::video::quarry::OWNER;
    e.title[..6].copy_from_slice(b"quarry");
    e.title_len = 6;
    // Closed => OFF the panel, so the pip takes the minimised ink, the same honest read a parked
    // window gives. `pin_shell`'s rule, unchanged.
    e.visible = false;
    e.focused = false;
    // Prepend: shift the scanned rows right by one and take slot 0. Bounded by `n < MAX_WINDOWS`
    // above, so the shift can never write past the array.
    rows.copy_within(0..n, 1);
    rows[0] = e;
    n + 1
}

#[cfg(not(feature = "quarry"))]
#[inline(always)]
fn pin_quarry(_rows: &mut [wm::DockEntry; wm::MAX_WINDOWS], n: usize) -> usize {
    n
}

/// STRIPFACTOR — the tenant's registry hook: **the dock's rect on this panel, or `None`.**
///
/// Registered as [`strip::TENANTS`]`[strip::DOCK_SLOT]`, so this is what `wm::erase_clip` reads. It
/// is [`Layout::for_panel`] and nothing else — the dock's one source of tile arithmetic, reached
/// through the tile count [`wm::dock_scan`] reports — which is the same expression the erase clip
/// used to inline. The dock is unconditionally PRESENT (there is no disable for it: it is the
/// console's only way back), so `None` here means only that the panel cannot host it.
pub fn strip_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    let mut tiles = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    // A zero rect asks the damage question nothing; only the tile count is wanted here.
    let (n, _) = wm::dock_scan(&mut tiles, (0, 0, 0, 0));
    // PINCOUNT — the registry must report the strip the painter will paint, pinned tiles included,
    // and it wants the COUNT and nothing else — so it asks for the count rather than assembling the
    // pinned model. `pins_applied` is the ONE definition of that arithmetic: the same four pins, in
    // the same order, under the same per-pin `n < MAX_WINDOWS` cap `compose`'s chain applies.
    let n = pins_applied(n, |o| tiles[..n].iter().any(|r| r.owner_asid == o));
    Layout::for_panel(n, pw, ph).map(|l| l.rect())
}

/// What the dock last put on the panel — signature and rect, in the primitive's [`strip::Slot`].
/// The rect is read by [`compose`] to ask `wm` the damage question BEFORE it knows this pass's
/// layout; a stale rect is safe, because a layout change is also a signature change and repaints
/// regardless.
static SLOT: strip::Slot = strip::Slot::new();

/// The window id whose tile is held down, or `wm::WIN_NONE`. Cleared by the raise that follows.
static PRESSED: AtomicU32 = AtomicU32::new(wm::WIN_NONE);

/// Cost ledger — the primitive's [`strip::Ledger`]. Not `witness`-gated: the metal image is built
/// WITHOUT `witness`, and a performance claim that is absent from the only artifact that matters is
/// not a claim.
static LEDGER: strip::Ledger = strip::Ledger::new();

/// The dock's own vocabulary, appended to the ledger's common terms so the line is unchanged.
static PRESSES_N: AtomicU64 = AtomicU64::new(0);
static RAISES: AtomicU64 = AtomicU64::new(0);
static UNHIDES: AtomicU64 = AtomicU64::new(0);

/// CLICK-BAND — what the LAST consumed press did, for the router's `band=dock` witness line (the
/// crystal's `PRESS_OUTCOME` twin; see `crystal.rs`). Written by every consuming arm of
/// [`press_at`], read by [`last_press_outcome`] immediately after the call on the same task.
static PRESS_OUTCOME: AtomicU64 = AtomicU64::new(0);
const DOCK_OUT_BACKGROUND: u64 = 1;
const DOCK_OUT_LAUNCH_SHELL: u64 = 2;
const DOCK_OUT_RAISE: u64 = 3; const DOCK_OUT_LAUNCH_CONSOLE: u64 = 4; // APPPIN — one outcome word per pinned app, so the router's `band=dock` witness tells a console launch from a shell launch.

/// CLICK-BAND — the last consumed press's outcome, as the witness word.
pub fn last_press_outcome() -> &'static str {
    match PRESS_OUTCOME.load(Ordering::Relaxed) {
        DOCK_OUT_BACKGROUND => "background",
        DOCK_OUT_LAUNCH_SHELL => "launch-shell", DOCK_OUT_LAUNCH_CONSOLE => "launch-console",
        DOCK_OUT_RAISE => "raise",
        _ => "none",
    }
}
/// WCK5 — **passes in which a window had painted over the strip.** The repaint this arc is removing.
///
/// `paints` conflates the two damage conditions: a MODEL change (a window opened, closed, was renamed
/// or changed focus — a repaint the dock owes and always will) and a CLOBBER (a window blit published
/// over the strip, so the strip has to put itself back). During a sustained drag the model does not
/// change at all, so before WCK5 every paint of a drag was a clobber and the strip was being redrawn
/// at motion rate — Peter's "it goes away dragging any window", from the panel's own ledger.
///
/// `occclip_dock_px` proves the withholding on a `witness` build; this proves the CONSEQUENCE on the
/// metal image, which is built without `witness` and is the only artifact the symptom was ever seen
/// on. Deliberately counted at the damage question rather than at the paint, so it stays readable
/// when a model change and a clobber coincide.
static CLOBBERS: AtomicU64 = AtomicU64::new(0);

/// The dock's tail on every ledger line: the WCK5 clobber count, presses, and what they did. A macro
/// rather than a function because `format_args!` borrows its arguments and the result cannot outlive
/// the call.
///
/// STRIPFACTOR × WCK5 — `clob=` is dock-specific (WCK5's "a window painted over the strip" counter)
/// and rides the tail rather than the primitive's common terms, because the menu bar has no such
/// counter and a shared field would print a meaningless `clob=0` for it. It moved from between
/// `paints=` and `rate=` (WCK5's inline `serial_println!`) to the tail when the rollup folded onto
/// `strip::Ledger`; no spec pins its position, so the reconciliation is a field reorder the analyzer
/// and the FORBIDs (which match `clob=` anywhere) do not see.
macro_rules! dock_tail {
    () => {
        format_args!(
            "clob={} presses={} raises={} unhides={}",
            CLOBBERS.load(Ordering::Relaxed),
            PRESSES_N.load(Ordering::Relaxed),
            RAISES.load(Ordering::Relaxed),
            UNHIDES.load(Ordering::Relaxed)
        )
    };
}

/// FNV-1a 64 over the tile model — the whole "has anything changed?" test, reduced to one integer.
///
/// Everything the painter reads goes in: the window id, the owner, the caption bytes, the
/// visible/focused bits, the pressed id, and the layout (which folds in the panel geometry and the
/// glyph budget). A field the painter uses and this hash omits is a field whose change would leave a
/// stale strip on the panel, so the two lists are the same list on purpose.
fn signature(e: &[wm::DockEntry], l: &Layout, pressed: u32) -> u64 {
    // STRIPFACTOR — the same FNV-1a 64, from the primitive, over the same fields in the same order.
    let mut h = strip::FNV_BASIS;
    for v in [
        l.x as u64, l.y as u64, l.w as u64, l.h as u64,
        l.n as u64, l.tile_w as u64, l.glyphs as u64, pressed as u64,
    ] {
        h = strip::fnv1a_u64(h, v);
    }
    for r in e {
        for k in 0..4 {
            h = strip::fnv1a(h, ((r.id >> (k * 8)) & 0xFF) as u8);
        }
        h = strip::fnv1a_u64(h, r.owner_asid);
        h = strip::fnv1a(h, r.visible as u8);
        h = strip::fnv1a(h, r.focused as u8);
        h = strip::fnv1a(h, r.title_len as u8);
        for &b in r.title[..r.title_len.min(wm::MAX_TITLE)].iter() {
            h = strip::fnv1a(h, b);
        }
    }
    // A zero signature means "nothing painted"; fold it away so a real model can never collide with
    // the empty state.
    strip::seal(h)
}

// ---------------------------------------------------------------------------
// The composite seam
// ---------------------------------------------------------------------------

/// **The dock's whole per-composite cost.** Called from `wm::composite_once` at the tail of every
/// pass, AFTER the window loop and BEFORE the cursor tail.
///
/// Returns `true` iff it painted, in which case the caller owes the sprite a `Repaint` tail — this
/// function has already taken the arrow off the panel.
///
/// The quiet path is one `wm::dock_scan`, one hash and a compare. See the module header.
pub fn compose() -> bool {
    let t0 = crate::arch::now_cycles();
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    // Ask `wm` for the tile model AND the damage question in ONE table scan: "were any of the
    // windows that intersect the strip I last painted damaged in the pass that just ran?"
    let (n, clobbered) = wm::dock_scan(&mut rows, SLOT.rect());
    // SHELLPIN — the permanent shell tile, appended before the signature so a shell close (the row
    // vanishing, the pin appearing) is a MODEL change and repaints on its own.
    let n = pin_console(&mut rows, n); let n = pin_shell(&mut rows, n); // CONSOLEPIN — the CONSOLE window's own reopen tile, applied FIRST so the settled strip reads `[quarry] [live rows…] [console] [shell] [pulse]`: the console's pin sits where its live row sat, immediately left of the permanent shell tail. Permanent since APPPIN (R49): the console is a pinned app and its tile is always on the strip. Folded, not added — PARITY.md §5.3.
    // QUARRY-PIN — after the shell pin, and PREPENDING (see `pin_quarry`): the settled strip is
    // `[quarry] [live windows…] [shell]`, the macOS order Peter's direction names.
    let n = pin_quarry(&mut rows, n); let n = pin_pulse(&mut rows, n); settle(&mut rows, n, true); // A30 — the pulse instrument's reopen tile, APPENDED after the shell's so the settled strip reads quarry, live rows, shell, pulse. A no-op unless `pulsewin::ever_armed()`, i.e. on every desktop that never had the window. Folded, not added — PARITY.md §5.3. DOCKID — the model is SETTLED here and only here: reconcile the tile registry against the assembled model (the one mutating call in the block, so the registry has exactly one writer), then sort into strip order. Ordering is what render11's defect was — `[dock] press … tile=4/5 pulse=pin` and `tile=4/5 win=8` one pixel apart, because reopening the furniture teleported win 8's tile from index 1 to index 4. See the DOCKID block at this file's tail. ⚠ FOLDED onto this line rather than added below it — PARITY.md §5.3.
    // WCK5 — one relaxed add on the pass that was clobbered, and nothing at all on the quiet pass.
    if clobbered {
        CLOBBERS.fetch_add(1, Ordering::Relaxed);
    }
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
            return false;
        }
        (fb.width(), fb.height())
    };
    let layout = Layout::for_panel(n, pw, ph);
    let pressed = PRESSED.load(Ordering::Acquire);
    let sig = match layout {
        Some(l) => signature(&rows[..n], &l, pressed),
        None => 0,
    };
    let painted_sig = SLOT.sig();
    LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));

    // The ledger reaches the METAL image, or it is not a performance claim.
    //
    // `rollup` is not `witness`-gated, but a function with no caller outside `witness` is not in the
    // artifact either — the linker drops it and the claim evaporates exactly where it matters. So the
    // live emitter is HERE, on the path every pass takes, rate-limited by the primitive's `tick`.
    // Cost on a pass that does not print: one relaxed load and a compare.
    LEDGER.tick("dock", dock_tail!());

    // The two damage conditions, and nothing else. Note the ordering: a signature that MATCHES and a
    // pass that did not touch the strip is the common case and returns here having read no pixel.
    if sig == painted_sig && !clobbered {
        return false;
    }
    // THE STRIP OWES ITS OWN VACATED PIXELS. `wm::erase` cleans the boxes of WINDOWS; the dock is not
    // a window and no other painter knows its rect, so a strip that shrinks (a window closed, so the
    // tiles are fewer and the strip is narrower) or goes away entirely (the last window closed) would
    // leave its old ends standing on the panel until something else happened to paint over them. The
    // rule is the one `wm::close` follows: erase what you vacate, in the same pass, through the same
    // staged path.
    let old = SLOT.packed();
    let new = strip::pack_rect(layout.map(|l| l.rect()));
    let vacated = if old != 0 && old != new { Some(strip::unpack_rect(old)) } else { None };

    let Some(l) = layout else {
        SLOT.clear();
        return match vacated {
            // TEARSCOPE — `strip::vacate` IS `strip::erase_rect` plus the census; it returns exactly
            // what the erase returned and this arm behaves as it did. `owed=false`: the slot was
            // cleared above, so a declined erase is a span nothing comes back for.
            Some(v) => strip::vacate("dock", v, None, false),
            None => false,
        };
    };

    let t1 = crate::arch::now_cycles();
    if let Some(v) = vacated {
        // Erase FIRST, then paint: the new strip lands on top of the cleaned area, so the two never
        // race to own an overlapping pixel and the panel never shows a half-erased strip.
        //
        // TEARSCOPE — accounted, not changed. `owed=false` because `SLOT.store` below re-publishes
        // this tenant's rect whatever the erase returned, so a decline here strands the ENDS this
        // centred, tile-sized strip just stopped owning. That is the span Peter watched.
        strip::vacate("dock", v, Some(l.rect()), false);
    }
    if !paint(&l, &rows[..n], pressed) {
        return false;
    }
    LEDGER.paint(
        crate::arch::now_cycles().saturating_sub(t1),
        (l.w * l.h) as u64,
    );
    SLOT.store(sig, Some(l.rect()));
    true
}

/// Paint the strip. Returns `false` without touching the panel if it could not (no scratch, or a
/// surface whose layout the row-run path does not cover).
///
/// The one framebuffer writer in this module, and it writes the way the subsystem's law requires:
/// compose a row in cached RAM, copy it out with one `blit`, clean the whole rect once at the end.
fn paint(l: &Layout, rows: &[wm::DockEntry], pressed: u32) -> bool {
    // STRIPFACTOR — the whole body moved to `strip::paint`: the readiness and word4 checks, the
    // bounds check, the scratch `try_lock`, the cursor bracket, the per-row encode memo, the row
    // `blit` and the single `flush_rect`. This function is now the dock's row composer and nothing
    // else, which is exactly the split that lets a second tenant exist.
    strip::paint("dock", l.rect(), |out, j| compose_row(out, l, rows, pressed, j))
}

/// Compose panel row `j` of the strip into `out[0..l.w]` as logical `0x00RRGGBB` colours.
///
/// Two halves, in this order: a field pass (strip face, keyline, bevel, tile faces, the indicator
/// pips, the cut corners), then the caption glyphs overlaid by index. The glyphs are an overlay
/// rather than a per-pixel test so the inner loop stays a handful of integer compares instead of a
/// scan over every tile's caption at every pixel.
///
/// **The curvature is paid for only where there IS curvature.** The first cut ran `corner_cut` — and
/// through [`edge_ring`], four more of them — at EVERY pixel of the strip, which measured at ~95
/// cycles a pixel and made the dock the most expensive thing in the pass by an order of magnitude. A
/// rounded rectangle is straight everywhere except inside `r` of a corner, so the row is laid down as
/// flat spans first and the two `STRIP_R`-wide end bands are patched per-pixel, and only on the
/// `2*STRIP_R` rows that have a corner in them at all. Same pixels, same shape; the shape test runs
/// `4 * (2*STRIP_R)^2` times per repaint instead of `5 * w * h`.
fn compose_row(out: &mut [u32], l: &Layout, rows: &[wm::DockEntry], pressed: u32, j: usize) {
    // The strip's material is anchored to the STRIP, not to the panel: index ceramic by the row's
    // offset inside the box, exactly as the window chrome indexes it by the row's offset inside the
    // window. The grain then belongs to the object.
    let face = ceramic::shade(theme::CHROME_FACE, j);
    let line = ceramic::shade(theme::FRAME_LINE, j);
    // The row's interior colour: the top bevel hairline for the first `BEVEL` rows under the keyline,
    // the chrome face everywhere else.
    let fill = if j >= 1 && j < 1 + theme::BEVEL { theme::BEVEL_LIGHT } else { face };
    if j == 0 || j + 1 == l.h {
        for i in 0..l.w {
            out[i] = line;
        }
    } else {
        out[0] = line;
        out[l.w - 1] = line;
        for i in 1..l.w - 1 {
            out[i] = fill;
        }
    }
    // The corner bands — the only pixels whose membership is in question.
    if j < STRIP_R || j + STRIP_R >= l.h {
        for i in (0..STRIP_R).chain(l.w - STRIP_R..l.w) {
            if strip::corner_cut(i, j, l.w, l.h, STRIP_R) {
                // The pixels the painter cuts out of the corners are filled with the DESKTOP, exactly
                // as `wm::paint_window` fills a window's cut head corners. Same rule, same colour,
                // and `Layout::contains` declines the same pixels so a press there falls through.
                out[i] = wm::DESKTOP_BG;
            } else if strip::edge_ring(i, j, l.w, l.h, STRIP_R) {
                out[i] = line;
            }
        }
    }
    // Tiles.
    for (t, r) in rows.iter().enumerate().take(l.n) {
        let Some((tx, ty, tw, th)) = l.tile(t) else { continue };
        let (bx, by) = (tx - l.x, ty - l.y);
        // The indicator band: the PAD below the tile, inside the strip.
        if j >= by + th && j < by + th + PAD {
            let d = IND_D;
            let px0 = bx + tw / 2 - d / 2;
            let py0 = by + th + (PAD - d) / 2;
            let ink = if r.visible { theme::ACCENT } else { theme::SCROLL_THUMB };
            for i in px0..(px0 + d).min(l.w) {
                if strip::in_disc(i, j, px0, py0, d) {
                    out[i] = ink;
                }
            }
            continue;
        }
        if j < by || j >= by + th {
            continue;
        }
        // The tile face — the material at the CONTROL gain (see the module header).
        let base = if r.id == pressed {
            theme::BUTTON_FACE_PRESSED
        } else {
            theme::BUTTON_FACE
        };
        let tface = ceramic::shade_gain(base, j, ceramic::CONTROL_GAIN_Q16);
        // Same span-then-patch shape as the strip: the tile's straight middle is a flat run, and the
        // two `TILE_R` end bands are tested per-pixel only on the rows that actually have a corner.
        // A cut tile corner shows the STRIP's face — the tile is a slab lying ON the strip, so what a
        // cut corner reveals is what is behind it, not the desktop.
        let (lo, hi) = (bx.min(l.w), (bx + tw).min(l.w));
        let corner_row = (j - by) < TILE_R || (j - by) + TILE_R >= th;
        if !corner_row {
            for i in lo..hi {
                out[i] = tface;
            }
        } else {
            let mid0 = (bx + TILE_R).min(hi);
            let mid1 = (bx + tw - TILE_R).max(mid0).min(hi);
            for i in lo..mid0 {
                if !strip::corner_cut(i - bx, j - by, tw, th, TILE_R) {
                    out[i] = tface;
                }
            }
            for i in mid0..mid1 {
                out[i] = tface;
            }
            for i in mid1..hi {
                if !strip::corner_cut(i - bx, j - by, tw, th, TILE_R) {
                    out[i] = tface;
                }
            }
        }
        // The caption, overlaid. Vertically centred in the tile, left-padded by one `PAD`, and
        // truncated to the layout's glyph budget — the budget the layout SIZED the tile from, so the
        // text can never overrun the box it is in.
        let ty0 = by + (th - CELL_H) / 2;
        if j < ty0 || j >= ty0 + CELL_H {
            continue;
        }
        let sy = j - ty0;
        let ink = if r.focused {
            theme::TITLE_TEXT_ACTIVE
        } else {
            theme::TITLE_TEXT_INACTIVE
        };
        // FONT (GR27) — the shared anti-aliased face, alpha-composited over the tile face the row
        // loop above just painted (a RAM scratch row, so the blend's read is cached). Regular
        // weight: a dock label is a secondary surface beside the caption and the bar.
        let cols = l.glyphs.min(r.title_len);
        super::font::draw_row(out, l.w, &r.title[..cols], bx + PAD, sy, ink, false, FACE);
    }
}

// STRIPFACTOR — `edge_ring` moved to `strip::edge_ring`, which takes the radius as an argument
// rather than closing over this module's `STRIP_R`. Same four probes, same order, same result for
// the dock's radius; a flush tenant passes 0 and pays one compare.

// ---------------------------------------------------------------------------
// The press seam
// ---------------------------------------------------------------------------

/// **Route a press at a panel point.** Returns `true` iff the DOCK consumed it.
///
/// Called from the head of `wc_click_route_at`'s press edge, ahead of every window arm, because the
/// dock is composited ON TOP of the window layer: a point the dock covers is a point the operator can
/// see the dock at, and `wm::hit_test` — which knows nothing of this strip — would otherwise hand it
/// to the window underneath. That is the fall-through this ordering forbids.
///
/// On a TILE it does exactly what the router's own raise arm does, through the same two primitives
/// and in the same order (`user_input_set_active`, then `wm::focus_changed`) — no second focus
/// mechanism is invented here. `focus_changed` is what makes this a RESTORE: it takes a fresh `z` off
/// the same monotonic allocator for every window the owner has, which lifts a row that was sitting
/// below `SHELL_Z` back over the shell, and it publishes the owner's UNHIDE to the syscall layer,
/// which is the wake edge a parked vug needs. One gesture, both halves.
///
/// A kernel-owned row (the panel console) hands the KEYBOARD to the shell instead of to a program
/// with no input ring — the rule the router's furniture arm already states.
pub fn press_at(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 {
        return false;
    }
    let (px, py) = (x as usize, y as usize);
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
    // SHELLPIN — the router routes over the same pinned model the painter drew.
    let n = pin_console(&mut rows, n); let n = pin_shell(&mut rows, n); // CONSOLEPIN — the CONSOLE window's own reopen tile, applied FIRST so the settled strip reads `[quarry] [live rows…] [console] [shell] [pulse]`: the console's pin sits where its live row sat, immediately left of the permanent shell tail. Permanent since APPPIN (R49): the console is a pinned app and its tile is always on the strip. Folded, not added — PARITY.md §5.3.
    // QUARRY-PIN — after the shell pin, and PREPENDING (see `pin_quarry`): the settled strip is
    // `[quarry] [live windows…] [shell]`, the macOS order Peter's direction names.
    let n = pin_quarry(&mut rows, n); let n = pin_pulse(&mut rows, n); settle(&mut rows, n, false); // A30 — the pulse instrument's reopen tile, APPENDED after the shell's so the settled strip reads quarry, live rows, shell, pulse. A no-op unless `pulsewin::ever_armed()`, i.e. on every desktop that never had the window. Folded, not added — PARITY.md §5.3. DOCKID — the ROUTER routes over the order the painter painted. `reconciling=false`: this is the click path, and the registry's writer is `compose` alone (LOCKFIX's rule for this router — it allocates nothing and takes no panel lock), so this is a pure sort over the published ranks. ⚠ FOLDED onto this line — PARITY.md §5.3.
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            return false;
        }
        (fb.width(), fb.height())
    };
    let Some(l) = Layout::for_panel(n, pw, ph) else {
        return false;
    };
    if !l.contains(px, py) {
        return false;
    }
    PRESSES_N.fetch_add(1, Ordering::Relaxed);
    let Some(t) = l.tile_at(px, py) else {
        PRESS_OUTCOME.store(DOCK_OUT_BACKGROUND, Ordering::Relaxed);
        serial_println!("[dock] press at ({},{}) -> strip tiles={} raised=none", x, y, n);
        return true; // the dock's own background: consumed, raises nothing.
    };
    let r = rows[t];
    // APPPIN — a PIN tile names no row: nothing to raise, nothing to focus yet. POST a launch for the
    // app it names and consume the press; the body that owns that app's instance drains the post on
    // its next pass and mints a fresh window through the app's own mint seam (see the header). Focus
    // hand-back (`user_input_set_active(0)`) happens at the launch itself, keyed to the NEW window.
    // QUARRY-PIN — the file manager's tile names no row while it is closed. LATCH the open rather
    // than performing it: this runs inside a click router, and Quarry's open READS DIRECTORIES —
    // `crate::video::quarry::service()` drains the latch from the arch's input-drain task, which is
    // where a volume read belongs. Consumed either way, so the press never falls through to a window.
    #[cfg(feature = "quarry")]
    if r.id == QUARRY_PIN_ID {
        crate::video::quarry::request_open();
        serial_println!(
            "[dock] press at ({},{}) tile={}/{} quarry=pin -> open requested",
            x, y, t, n
        );
        return true;
    }
    if r.id == PULSE_PIN_ID { crate::video::pulsewin::arm(); serial_println!("[dock] press at ({},{}) tile={}/{} pulse=pin -> rearmed (render pass opens it)", x, y, t, n); return true; } // A30 — the pulse instrument's pinned tile. Unlike Quarry's this needs no latch and no deferral: `pulsewin::arm()` is two release stores and touches no device, and `service()`'s open arm on the next render pass is what actually mints the window — the same split that keeps the create on the compositor's own core (see that arm's readback). So the router re-arms and is done, and the press is consumed so it never falls through to a window beneath.
    let app = if r.id == CONSOLE_PIN_ID {
        Some((PinnedApp::Console, DOCK_OUT_LAUNCH_CONSOLE))
    } else if r.id == SHELL_PIN_ID {
        Some((PinnedApp::Shell, DOCK_OUT_LAUNCH_SHELL))
    } else {
        None
    };
    if let Some((app, out)) = app {
        PRESS_OUTCOME.store(out, Ordering::Relaxed);
        post_launch(app);
        serial_println!(
            "[dock] press at ({},{}) tile={}/{} app={} -> launch",
            x, y, t, n, app.word()
        );
        return true;
    }
    PRESS_OUTCOME.store(DOCK_OUT_RAISE, Ordering::Relaxed);
    let was_hidden = !r.visible;
    PRESSED.store(r.id, Ordering::Release);
    if crate::video::wm::is_kernel_owner(r.owner_asid) {
        focus_set(0);
    } else {
        focus_set(r.owner_asid);
    }
    let wgen = wm::winid_gen(r.id); // DOCKID — the OTHER half of the tile's identity, taken from the scan this press routed over and re-checked below: a close-and-recycle between the scan and the raise must not let this press land on the window that took the slot.
    wm::focus_changed(r.owner_asid);
    // DOCKID — and then THIS window, specifically. `focus_changed` is keyed by ASID and raises every
    // window the owner has, deliberately; a tile names ONE window, so with two windows under one owner
    // the topmost after the raise was whichever row sat later in the table, not the one pressed. See
    // `wm::raise_one`. Gated on the generation: a stale tile raises NOTHING rather than the wrong thing.
    let raised_one = wm::winid_gen(r.id) == wgen && wm::raise_one(r.id);
    PRESSED.store(wm::WIN_NONE, Ordering::Release);
    RAISES.fetch_add(1, Ordering::Relaxed);
    if was_hidden {
        UNHIDES.fetch_add(1, Ordering::Relaxed);
    }
    let now_visible = wm::info(r.id).map(|i| i.z > wm::shell_z()).unwrap_or(false);
    serial_println!(
        "[dock] press at ({},{}) tile={}/{} win={} owner={:#x} was_hidden={} -> raised={} unhid={}",
        x, y, t, n, r.id, r.owner_asid, was_hidden, now_visible,
        was_hidden && now_visible
    );
    // DOCKID — the IDENTITY witness, beside the geometry one above: which tile, which window, which
    // generation, and whether THAT window was brought forward. `raised=no` is the honest reading for a
    // tile whose window went away under the press, and it is a reading the older line cannot produce —
    // it reports `raised=` from `z > shell_z()`, which a DIFFERENT window in the same slot satisfies
    // just as well.
    serial_println!(
        "[dock] press tile={}/{} -> win={} gen={} raised={}",
        t, n, r.id, wgen,
        if raised_one { "yes" } else { "no" }
    );
    true
}

// ---------------------------------------------------------------------------
// Witness
// ---------------------------------------------------------------------------

/// **What the dock cost, and what it drew.** One bounded line; the caller decides how often.
///
/// Deliberately NOT `witness`-gated, on `ceramic::witness`'s precedent: the metal image is built
/// without `witness`, and a cost claim absent from that artifact is not a claim.
///
/// `scan_cyc` is what EVERY composite pass pays for the dock existing (the model scan, the hash and
/// the compare). `paint_cyc` is what a repaint costs, and `paints/passes` is the repaint RATE — the
/// number that says whether the strip is damage-driven or is quietly redrawing every frame. A dock
/// that repainted per frame would show `paints == passes`, so the claim is falsifiable from the wire.
pub fn rollup(scope: &str) {
    // STRIPFACTOR × WCK5 — the common terms come from the primitive's `Ledger`; the dock's own four
    // (WCK5's clobber count, presses, raises, unhides) are its tail. Every field WCK5's inline line
    // carried survives — `clob=` moved from between `paints=` and `rate=` to the tail, which no spec
    // pins.
    LEDGER.rollup("dock", scope, dock_tail!());
}


/// DOCK fixture — **a minimised window is restorable, and the tile that restores it is the tile the
/// painter drew.**
///
/// Five legs, each able to FAIL on its own:
///
/// ### CONSOLEWIN — the restored window is KERNEL FURNITURE, and that is what this fixture is for
/// The third row carries a reserved kernel owner rather than an ordinary ASID. It is the same row
/// legs 1-4 already minimised and brought back, so the shape of the fixture is unchanged; what
/// changes is that the claim now covers the one window whose reversibility has no other route.
/// `<TAB>` cannot restore a parked console — x86's `focus_ring_apps` filters the reserved band out
/// of the focus rotation — so the dock IS the console's way back, and `wm::minimise`'s standing
/// precondition ("a control that hides a window with no way back is worse than an inert one") rests
/// on this leg for every kernel row. `wm::dock_scan` has included kernel-owned rows since the module
/// landed; what did not exist was a leg that would notice if that stopped being true.
///
/// It also means the row must be parked EXPLICITLY (see the `wm::minimise` call below): furniture is
/// exempt from the shell raise, so `focus_changed(0)` no longer puts it under, and the fixture uses
/// the gesture the operator's minimise disc actually calls.
///
/// 1. **the model** — three windows are minted (three distinct owners), `focus_changed(0)` pushes
///    every row below the shell, two raises bring two of them back, and the third is minimised. The
///    dock must report exactly THREE tiles, of which exactly ONE is not visible. A dock that
///    enumerated only the windows on the panel would report two here, which is the whole defect this
///    module exists to prevent — a minimised window with no way back.
/// 2. **geometry agreement** — the press point is TILE `k`'s centre as [`Layout`] computes it, and
///    `Layout::tile_at` must answer `k` for it. Painter and router share the accessor, so this leg
///    fails only if the accessor is internally inconsistent.
/// 3. **the restore** — a synthetic press at the HIDDEN window's tile centre must be CONSUMED, and
///    the window must come back: `z > shell_z()` where it was `<` before. This is the load-bearing
///    claim.
/// 4. **specificity** — the press must have raised THAT window and not merely raised something. The
///    other hidden-then-raised rows are checked by identity, so an off-by-one in `tile_at` (the
///    classic painter/router drift) fails here rather than passing by luck.
/// 5. **the miss** — a press one pixel ABOVE the strip must NOT be consumed. Without this leg the
///    module could consume the whole panel and still pass legs 1-4; with it, "the dock does not
///    swallow the desktop" is checked rather than asserted.
///
/// Self-cleaning: the three rows are closed and the focus state is restored. Runs on the real panel,
/// so it belongs after every one-shot per-window latch — the same ordering rule
/// `wm::hittest_selftest` states at its own call site.
#[cfg(feature = "witness")]
pub fn selftest() {
    use core::sync::atomic::AtomicBool as OnceBool;
    static DONE: OnceBool = OnceBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    // STRIPFACTOR — the STRIP battery runs as one battery, and the menu bar's fixture is driven from
    // here rather than from its own call site.
    //
    // ⚠ Stated plainly, because it is a compromise and not a design: the natural home is beside
    // `dock::selftest`'s own invocation in `arch/x86_64/syscall.rs`, and that file is owned by a
    // concurrent arc and outside this arc's lane. Driving it here buys the identical preconditions —
    // the same `witness` gate, the same real panel, the same "after every one-shot per-window latch"
    // ordering — at the cost of coupling two fixtures that are not otherwise related.
    //
    // It runs BEFORE this fixture mints its rows, deliberately: the bar's legs are about the registry
    // and the panel's edges, not about the window table, and running first means they cannot be
    // perturbed by three synthetic windows — nor skipped by the `SKIP` return below if the table is
    // full.
    //
    // ⚠ **FOR THE INTEGRATOR — re-seat this in `arch/x86_64/syscall.rs` once that lane frees.** The
    // canonical home is line ~15477 there, immediately after `crate::video::dock::selftest();`, as
    // its own statement: `crate::video::menubar::selftest();`. It belongs beside the dock's call, not
    // nested inside the dock fixture — the two fixtures are unrelated and this nesting is a lane
    // compromise, not a design. When moved, DELETE this call and this comment block; nothing in either
    // fixture depends on the coupling, and `menubar::selftest`'s own one-shot `DONE` latch makes the
    // move idempotent (a double-drive from a botched move runs once, not twice).
    super::menubar::selftest();
    // STRIPVAC — the vacate handback's fixture, seated here on the identical compromise the block
    // above documents (same `witness` gate, same real panel, same "after every one-shot per-window
    // latch" ordering) and ahead of this fixture's own rows for the same reason: its subject is the
    // strip PRIMITIVE's census, not the window table. It scores under the catch-all census slot, so
    // it cannot move the `tenant=dock` counters the legs below drive. ⚠ Re-seat it in
    // `arch/x86_64/syscall.rs` beside `crate::video::dock::selftest();` when that lane frees, exactly
    // as the menu bar's call above is owed; its own `DONE` latch makes the move idempotent.
    super::strip::vacate_selftest();

    /// Three 8x8 ARGB8888 surfaces in rodata — read-only, because the compositor only reads.
    static SURF: [[u32; 64]; 3] = [[0x0020_40FF; 64], [0x0040_FF20; 64], [0x00FF_4020; 64]];
    /// CONSOLEWIN — the THIRD owner is kernel FURNITURE, and it is the row leg 3 restores.
    ///
    /// It was an ordinary ASID (`0xD0C3`). The change is what makes this fixture the x86 witness for
    /// the arc's reopen claim, and it costs nothing: `w[2]` is already the row the legs minimise and
    /// bring back, so the only thing that moves is WHICH owner is being proved restorable.
    ///
    /// Why it has to be this row and not an extra fourth one: the console's route back is the only
    /// one in the system that `<TAB>` cannot serve — x86's `focus_ring_apps` filters the reserved
    /// band out of the focus rotation, so a parked kernel row is not in the ring at all. The dock is
    /// therefore not a convenience for it, it is the whole of its reversibility, and
    /// `wm::minimise`'s precondition ("a control that hides a window with no way back is worse than
    /// an inert one") rests on THIS leg for furniture. `dock_scan` includes kernel-owned rows by
    /// design and has since the module landed; what was missing was a leg that would notice if that
    /// stopped being true.
    ///
    /// Deliberately NOT `KERNEL_OWNER_CONSOLE`: the real console row carries that owner on a live
    /// x86 boot, and a fixture sharing it would make `owner_hidden` and every owner-scoped raise
    /// ambiguous between the operator's console and a synthetic 8x8 square. Any value in the
    /// reserved band satisfies `is_kernel_owner`, which is the arm under test.
    const OWNER_FURNITURE: u64 = wm::KERNEL_OWNER_BASE + 0x50;
    const OWNERS: [u64; 3] = [0xD0C1, 0xD0C2, OWNER_FURNITURE];
    const NAMES: [&[u8]; 3] = [b"dockA", b"dockB", b"dockC"];

    let saved_focus = focus_get();
    let mut w = [wm::WIN_NONE; 3];
    for k in 0..3 {
        w[k] = wm::create(
            OWNERS[k],
            SURF[k].as_ptr() as usize,
            core::mem::size_of_val(&SURF[k]),
            8,
            8,
            // STRIDE IS IN BYTES, not pixels — `create_inner`'s extent contract is
            // `w * 4 <= stride` and `h * stride <= surf_len`. 8 px * 4 B = 32 B a row, 8 rows = the
            // whole 256-byte surface.
            32,
            NAMES[k],
        );
    }
    if w.iter().any(|&i| i == wm::WIN_NONE) {
        serial_println!(
            ":: DOCK: fixture — table full, wins={:?} :: SKIP ::",
            w
        );
        for &i in w.iter() {
            if i != wm::WIN_NONE {
                wm::close(i);
            }
        }
        return;
    }

    // Every row below the shell, then bring TWO of the three back: w[2] stays minimised.
    wm::focus_changed(0);
    wm::focus_changed(OWNERS[0]);
    wm::focus_changed(OWNERS[1]);
    // CONSOLEWIN — **and w[2] is parked EXPLICITLY, because the shell raise no longer parks it.**
    //
    // `w[2]` is kernel furniture now, and furniture is exempt from the incidental hide: a shell raise
    // sweeping past it leaves it composited, which is the whole of the CLOSEISO fix and is asserted
    // by `wm::closeiso_selftest` leg 1. So the `focus_changed(0)` above — which used to be what put
    // this row under — does nothing to it, and legs 1 and 3 would be reading a VISIBLE row and
    // proving nothing.
    //
    // The fix is not to weaken the exemption, it is to use the gesture the arc actually wired: an
    // operator pressing this window's own minimise disc. `wm::minimise` is what that disc calls, and
    // it is a DELIBERATE park, which `wm::above_shell` honours for furniture where it ignores the
    // shell raise. The outcome token is asserted rather than discarded — `parked` means the row went
    // down AND its owner is hidden, so a `declined` or an `already` from a future guard that started
    // refusing kernel rows again fails here loudly instead of leaving legs 1-4 to fail obscurely.
    let park = wm::minimise(w[2]);
    let park_ok = park == "parked";

    // Leg 1 — the model. Ours are the three rows we just made; the live console/desktop rows are in
    // the table too, so the assertions are made about OUR ids rather than about the total.
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
    // SHELLPIN — the fixture routes over the PINNED model, because the router does (the module's
    // founding law: painter and router share the accessor, and this fixture is what asserts it).
    // On a witness gate no KERNEL_OWNER_DESKTOP row exists (desktop_uefi never activates without a Kepler),
    // so the raw count is one tile short of the strip press_at lays out: every boundary shifts, the
    // probe centre lands off its tile — and can land ON the pin tile, latching a spurious reopen.
    let n = pin_console(&mut rows, n); let n = pin_shell(&mut rows, n); // CONSOLEPIN — the CONSOLE window's own reopen tile, applied FIRST so the settled strip reads `[quarry] [live rows…] [console] [shell] [pulse]`: the console's pin sits where its live row sat, immediately left of the permanent shell tail. Permanent since APPPIN (R49): the console is a pinned app and its tile is always on the strip. Folded, not added — PARITY.md §5.3.
    // QUARRY-PIN — after the shell pin, and PREPENDING (see `pin_quarry`): the settled strip is
    // `[quarry] [live windows…] [shell]`, the macOS order Peter's direction names.
    let n = pin_quarry(&mut rows, n); let n = pin_pulse(&mut rows, n); settle(&mut rows, n, false); // A30 — the pulse instrument's reopen tile, APPENDED after the shell's so the settled strip reads quarry, live rows, shell, pulse. A no-op unless `pulsewin::ever_armed()`, i.e. on every desktop that never had the window. Folded, not added — PARITY.md §5.3. DOCKID — the FIXTURE routes over the same order, or leg 2's `tile_at(centre of tile k) == Some(k)` would be checking a tile index the router never computes. ⚠ FOLDED onto this line — PARITY.md §5.3.
    let mine: [Option<usize>; 3] = [
        rows[..n].iter().position(|r| r.id == w[0]),
        rows[..n].iter().position(|r| r.id == w[1]),
        rows[..n].iter().position(|r| r.id == w[2]),
    ];
    let model_ok = mine.iter().all(|m| m.is_some())
        && mine[0].map(|i| rows[i].visible) == Some(true)
        && mine[1].map(|i| rows[i].visible) == Some(true)
        // THE leg: the minimised window is IN the model, and is marked as off the panel.
        && mine[2].map(|i| rows[i].visible) == Some(false);

    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        (fb.width(), fb.height())
    };
    let layout = Layout::for_panel(n, pw, ph);

    // Legs 2-5.
    let (geom_ok, restore_ok, specific_ok, miss_ok, probe) = match (layout, mine[2]) {
        (Some(l), Some(k)) => {
            let (tx, ty, tw, th) = l.tile(k).unwrap_or((0, 0, 0, 0));
            let (cx, cy) = (tx + tw / 2, ty + th / 2);
            // Leg 2 — the accessor agrees with itself: the centre of tile k IS in tile k.
            let geom = l.tile_at(cx, cy) == Some(k) && l.contains(cx, cy);
            // The window is below the shell before the press, or leg 3 proves nothing.
            let below_before = wm::info(w[2]).map(|i| i.z < wm::shell_z()).unwrap_or(false);
            // Leg 3 — the synthetic press.
            let consumed = press_at(cx as i32, cy as i32);
            let back = wm::info(w[2]).map(|i| i.z > wm::shell_z()).unwrap_or(false);
            let restore = consumed && below_before && back;
            // Leg 4 — it raised THAT window: w[2] is now the topmost of the three.
            let z = |id| wm::info(id).map(|i| i.z).unwrap_or(0);
            let specific = z(w[2]) > z(w[0]) && z(w[2]) > z(w[1]);
            // Leg 5 — one pixel above the strip is NOT the dock's.
            let miss = !press_at(l.x as i32 + 1, l.y as i32 - 1);
            (geom, restore, specific, miss, Some((cx, cy)))
        }
        _ => (false, false, false, false, None),
    };

    // Leg 6 — THE STRIP OWES ITS VACATED PIXELS. Closing three windows makes the dock narrower (or
    // removes it), and the rect it reports as painted must FOLLOW: a strip that shrank without
    // erasing what it vacated would still be claiming — and still be showing — the wide rect. This is
    // the defect the leg was written for, found by review rather than by the panel.
    let rect_before = SLOT.packed();
    for &i in w.iter() {
        wm::close(i);
    }
    let rect_after = SLOT.packed();
    let vacate_ok = rect_before == 0 || rect_after != rect_before;
    focus_set(saved_focus);
    wm::focus_changed(saved_focus);

    let ok = model_ok && geom_ok && restore_ok && specific_ok && miss_ok && vacate_ok && park_ok;
    let (px, py) = probe.unwrap_or((0, 0));
    let (lx, lw, lg) = layout.map(|l| (l.x, l.w, l.glyphs)).unwrap_or((0, 0, 0));
    if ok {
        serial_println!(
            ":: DOCK: strip tiles={} at x={} w={} glyphs={}, probe=({},{}) model={} geom={} restore={} specific={} miss={} vacate={} furniture park={}/{} :: PASS ::",
            n, lx, lw, lg, px, py, model_ok, geom_ok, restore_ok, specific_ok, miss_ok, vacate_ok,
            park, park_ok
        );
    } else {
        serial_println!(
            ":: DOCK: strip tiles={} at x={} w={} glyphs={}, probe=({},{}) model={} geom={} restore={} specific={} miss={} vacate={} furniture park={}/{} :: FAIL ::",
            n, lx, lw, lg, px, py, model_ok, geom_ok, restore_ok, specific_ok, miss_ok, vacate_ok,
            park, park_ok
        );
    }
    rollup("selftest"); dockid_selftest(); // DOCKID — the tile-IDENTITY battery, driven from here on `menubar::selftest`'s precedent (same `witness` gate, same real panel, same ordering) because this module's own call site is `arch/x86_64/syscall.rs`, outside this arc's lane. It runs LAST: it mints six rows of its own and closes them, and the legs above must not see them. Its own one-shot `DONE` latch makes a future move to the canonical call site idempotent. ⚠ FOLDED onto this line — PARITY.md §5.3.
}

// ------------------------------------------------------------------------------------------------
// A30 — the pulse instrument's pinned reopen tile (TAIL-APPENDED: nothing above this line moved, so
// knob-off panic `Location` line numbers are untouched; PARITY.md §5.3)
// ------------------------------------------------------------------------------------------------

/// A30-PIN — the PULSE instrument's permanent tile, on `pin_shell`'s and [`pin_quarry`]'s precedent.
///
/// **Why it has to exist.** `pulsewin::close` now clears the window's ARMED latch, which is A30's
/// actual fix: before it, `pulsewin::service`'s open arm re-opened the window on the very next render
/// pass and the operator's close disc did nothing that lasted (render7 caught it twice, and it is why
/// A18's cascade census counted three opens of one window). But a close that is final and has no way
/// back is the failure the shell's and Quarry's pins were minted to prevent — "a window an operator
/// can close and cannot call back is a window they have lost". So the close is final AGAINST THE
/// RENDER PASS and reversible BY THE OPERATOR, and this tile is the whole of the second half.
///
/// **This is not a launcher and it is not a new tile model.** Same rule as Quarry's pin: the pulse
/// window is kernel-owned furniture, its LIVE row is already dock-addressable (kernel owners are), so
/// this pin exists only while the window is closed and adds nothing while it is up. It APPENDS rather
/// than prepending — Quarry is the leftmost pinned app by Peter's direction and the shell is the
/// permanent tail, so the settled strip reads `quarry | live rows | shell | pulse`, with the
/// instrument at the far end where a status item belongs.
///
/// **It is a no-op on every board that has no pulse window.** The guard is
/// `pulsewin::ever_armed()` — a runtime fact, not a `cfg`. `pulsewin::arm()`'s only non-dock caller
/// is `desktop_firmware::activate`, the Pi/Orin seam, so an x86 `desktop_uefi` desktop never arms it
/// and this returns `n` untouched: the x86 dock's tile model, layout, damage signature and occlusion
/// clip are all byte-identical to what they were.
///
/// Returns the new count. Applied by every reader of the model — `compose`, [`press_at`],
/// [`strip_rect`], `selftest` — so painter, router, registry and self-test cannot disagree about the
/// tile count, which is the invariant `pin_shell`'s header states and `selftest` checks.
fn pin_pulse(rows: &mut [wm::DockEntry; wm::MAX_WINDOWS], n: usize) -> usize {
    // PINCOUNT — the condition lives ONCE (`pin_pulse_wanted`, this file's tail); this is its only
    // mutator, and `pins_applied` folds the same predicate for the two count-only readers. This one
    // takes NO row census: `ever_armed`/`is_open` are runtime cells, not a scan of the model.
    if !pin_pulse_wanted(n) {
        return n;
    }
    let mut e = wm::DockEntry::empty();
    e.id = PULSE_PIN_ID;
    e.owner_asid = crate::video::pulsewin::OWNER;
    e.title[..5].copy_from_slice(b"pulse");
    e.title_len = 5;
    // Closed => OFF the panel, so the pip takes the minimised ink — `pin_shell`'s rule, unchanged.
    e.visible = false;
    e.focused = false;
    rows[n] = e;
    n + 1
}

// ------------------------------------------------------------------------------------------------
// CONSOLEPIN / APPPIN — the CONSOLE app's permanent tile, the console's launch service, and the
// launch/quit accounting every pinned app reports through.
// ------------------------------------------------------------------------------------------------

/// CONSOLEPIN — the pinned console tile's synthetic window id. One below [`PULSE_PIN_ID`], on the
/// argument [`SHELL_PIN_ID`] states: real ids are `1..=wm::MAX_WINDOWS` and `wm::WIN_NONE` is 0, so
/// no sentinel in this descending run can ever name a live row or collide with [`PRESSED`]'s idle
/// value.
const CONSOLE_PIN_ID: wm::WinId = wm::WinId::MAX - 3;

/// APPPIN — **the console app's PERMANENT tile, on [`pin_shell`]'s and [`pin_quarry`]'s shape.**
///
/// The console window is kernel-owned furniture whose LIVE row is already dock-addressable
/// (`dock_scan` includes kernel owners), so this pin exists only while the window is CLOSED and the
/// model is unchanged while it is up. Applied FIRST — before `pin_shell` — so the settled strip reads
/// `[quarry] [live rows…] [console] [shell] [pulse]`: the pin lands where the live console row sat.
///
/// It is PERMANENT (R49). The "has a console window ever existed on this boot" latch that used to
/// gate it (`CONSOLE_WINDOWED`, SO13) is gone: a pinned app's tile is on the strip whether or not the
/// app has run yet, and pressing it launches the app — `fbcon::panel_console_window_open` is the
/// console's mint seam on every board that has a dock, and it names its own decline on the wire.
///
/// Returns the new count. Applied by every reader of the model — [`compose`], [`press_at`],
/// [`strip_rect`], [`selftest`] — so painter, router, occlusion registry and self-test cannot
/// disagree about the tile count, which is the invariant `pin_shell`'s header states.
fn pin_console(rows: &mut [wm::DockEntry; wm::MAX_WINDOWS], n: usize) -> usize {
    // PINCOUNT — the condition lives ONCE (`pin_console_wanted`, this file's tail); this is its only
    // mutator, and `pins_applied` folds the same predicate for the two count-only readers.
    if !pin_console_wanted(n, &|o| rows[..n].iter().any(|r| r.owner_asid == o)) {
        return n;
    }
    let mut e = wm::DockEntry::empty();
    e.id = CONSOLE_PIN_ID;
    e.owner_asid = wm::KERNEL_OWNER_CONSOLE;
    e.title[..7].copy_from_slice(b"console");
    e.title_len = 7;
    // Closed => OFF the panel, so the pip takes the minimised ink — `pin_shell`'s rule, unchanged.
    e.visible = false;
    e.focused = false;
    rows[n] = e;
    n + 1
}

/// APPPIN — a posted console launch that `fbcon` declined is retried on the next passes, this many
/// times, before it is dropped and reported. `panel_console_window_open` declines for TRANSIENT
/// reasons as well as permanent ones — `console-not-ready` and `install-contended` are both a
/// `FBCON.try_lock()` that lost to the console's own glyph painter, which on a board whose render
/// body shares a core with its console pump is a race the operator's press can lose (render13's
/// `route=declined` had no reason line in the capture; this is the shape that produces one). A
/// permanent decline (`alloc`, `geometry-unavailable`, `create-failed`) costs 32 quiet passes and then
/// one line naming the count; nothing is minted twice, because a success clears the counter.
const CONSOLE_LAUNCH_TRIES: u32 = 32;
static CONSOLE_LAUNCH_TRIED: AtomicU32 = AtomicU32::new(0);

/// APPPIN — **drain a posted console launch: mint the console window, or raise the live one.**
///
/// Called from a RENDER body, never from the click router: `fbcon::panel_console_window_open`
/// allocates a surface, takes the window TABLE and takes the panel locks (`WRITER`, `FBCON`), and the
/// input path is held to LOCKFIX `7847ceea` — *"nothing here takes a blocking panel lock"*. Every
/// render body calls this (x86, the Pi, the cascaded scene) through `main.rs`'s `console_launch_drain`
/// wrapper; it costs one relaxed RMW on a quiet pass.
///
/// Two arms, one live console window max:
///
///  * **already live** — the scan-to-press race (the window came back between the model the press
///    routed over and this pass). Raise it through the pair the router's furniture arm uses
///    (`focus_set(0)` then `wm::focus_changed`): a kernel-owned row has no input ring, so the
///    keyboard goes to the shell. Never a second window. Reported as `-> RAISED`.
///  * **closed** — LAUNCH through `fbcon::panel_console_window_open`, the ONE function that mints
///    this window anywhere in the tree (`desktop_uefi::activate` on x86, the cascade's
///    `desktop_firmware::activate` on aarch64 reach the same one). It allocates a fresh surface, mints
///    a fresh row (new id, new generation), re-installs the glyph route and repaints the retained
///    cell store (CONSOLETEXT, SO17) — the app's document survives a quit, the window does not.
///
/// Arch-neutral: this module's own gate (x86 + `wc` OR aarch64 + `desktop_firmware`) is exactly the
/// gate on every `fbcon` entry point it calls, so there is no `target_arch` test here and none is
/// needed — x86 has the same dock, the same pin and the same console window.
pub fn console_launch_service(who: &'static str) -> bool {
    if !take_launch(PinnedApp::Console) {
        return false;
    }
    if crate::video::fbcon::console_is_routed() {
        let id = crate::video::fbcon::console_win();
        CONSOLE_LAUNCH_TRIED.store(0, Ordering::Relaxed);
        focus_set(0);
        wm::focus_changed(wm::KERNEL_OWNER_CONSOLE);
        app_raised(PinnedApp::Console, id, who);
        return true;
    }
    let id = crate::video::fbcon::panel_console_window_open();
    if id == wm::WIN_NONE {
        let tries = CONSOLE_LAUNCH_TRIED.fetch_add(1, Ordering::Relaxed) + 1;
        if tries < CONSOLE_LAUNCH_TRIES {
            post_launch(PinnedApp::Console);
            return false;
        }
        CONSOLE_LAUNCH_TRIED.store(0, Ordering::Relaxed);
        serial_println!(
            "[dock] launch app=console by={} win=0 gen=0 tries={} -> DECLINE (fbcon named the reason on its `[wc-x] console-window DECLINE` line each try; the tile stays for the next press)",
            who, tries
        );
        return false;
    }
    let tries = CONSOLE_LAUNCH_TRIED.swap(0, Ordering::Relaxed) + 1;
    focus_set(0);
    wm::focus_changed(wm::KERNEL_OWNER_CONSOLE);
    app_launched(PinnedApp::Console, id, who, tries);
    true
}

// ------------------------------------------------------------------------------------------------
// APPPIN — the launch / quit accounting: one grammar for every pinned app and every render body.
// ------------------------------------------------------------------------------------------------

/// APPPIN — launches and quits per app, cumulative from boot. Read by [`apppin_selftest`] to wait on
/// the render body it drives (the body runs on its own core and services the post on its own pass),
/// and by nothing else; always compiled because the writers are the shipping launch and quit paths,
/// and a counter the fixture can read is the same counter a bench capture can reconstruct from the
/// lines below.
static APP_LAUNCHES: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static APP_QUITS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// APPPIN — **a pinned app's window was minted in answer to a launch.** Called by the body that
/// minted it, AFTER the row exists and the first frame is presented, with the id it minted, so
/// `win=`/`gen=` name a row `[wm] alloc` has already announced. `tries=` is the number of passes the
/// launch took (1 unless the mint declined transiently — see [`CONSOLE_LAUNCH_TRIES`]).
pub fn app_launched(app: PinnedApp, id: wm::WinId, by: &'static str, tries: u32) {
    serial_println!(
        "[dock] launch app={} by={} win={} gen={} tries={} -> LAUNCHED",
        app.word(),
        by,
        id,
        wm::winid_gen(id),
        tries
    );
    APP_LAUNCHES[app.slot()].fetch_add(1, Ordering::Release);
}

/// APPPIN — **a launch found the app already live and raised it instead** (the scan-to-press race).
/// Not a launch: the count does not move, so a fixture waiting on [`app_launches`] cannot mistake a
/// raise for a fresh instance.
pub fn app_raised(app: PinnedApp, id: wm::WinId, by: &'static str) {
    serial_println!(
        "[dock] launch app={} by={} win={} gen={} tries=0 -> RAISED",
        app.word(),
        by,
        id,
        wm::winid_gen(id)
    );
}

/// APPPIN — **a pinned app's window is gone and its owning body has torn the instance down.**
/// Called by that body on the pass it noticed the row missing (the disc, Quit or `wc_close_furniture`
/// freed it through `wm::close`; the body finds it gone, frees the surface store and drops the
/// `Console`). `win=`/`gen=` are the id and generation the body RECORDED at the mint — the row is
/// already freed, so they are not re-read from the table, where the slot may already be somebody
/// else's.
pub fn app_quit(app: PinnedApp, id: wm::WinId, generation: u32, by: &'static str) {
    serial_println!(
        "[dock] quit app={} by={} win={} gen={} -> TORN-DOWN",
        app.word(),
        by,
        id,
        generation
    );
    APP_QUITS[app.slot()].fetch_add(1, Ordering::Release);
}

/// APPPIN — launches of `app` since boot (see [`app_launched`]).
pub fn app_launches(app: PinnedApp) -> u64 {
    APP_LAUNCHES[app.slot()].load(Ordering::Acquire)
}

/// APPPIN — quits of `app` since boot (see [`app_quit`]).
pub fn app_quits(app: PinnedApp) -> u64 {
    APP_QUITS[app.slot()].load(Ordering::Acquire)
}

// ------------------------------------------------------------------------------------------------
// APPPIN — the fixture: press the shell tile, get a fresh window; close it, see the teardown and the
// tile still there; press again, get ANOTHER fresh window; and the sprite composes over it.
// ------------------------------------------------------------------------------------------------

/// APPPIN — **the pinned-app round trip, driven through the shipping seams end to end.**
///
/// Every leg goes through the code the operator's click takes: the press is [`press_at`] at the
/// pin tile's own centre (the [`Layout`] the painter uses), the launch is drained by the REAL render
/// body on ITS core and ITS pass (this fixture only waits — the shell tuple is that body's, and a
/// fixture that minted the row itself would prove its own arithmetic and nothing about the path the
/// tile takes), the quit is `wm::close` (the disc's and Quit's path), and the teardown is the body's
/// own — observed through [`app_quits`]. So a body that stopped draining the post, a launch that
/// re-minted an old row instead of a fresh one, a quit that left the tile behind or took it away,
/// and a relaunch whose window the sprite no longer composes over all read as `:: FAIL ::` here.
///
/// ### The sprite leg (the LOST POINTER, render13 boot 1)
///
/// After the old shell RE-MINT (`[realdesk] shell-remint … -> MINTED`, evidence
/// `docs/dev/evidence/orin27/render13-boot1-comp.log:604`) every `[comp2] rollup` read
/// `sprite_us=1` and `[cursor12] offer … -> nohit`: the sprite was on the panel but no composite
/// offered it to the re-minted window. The leg parks the REAL sprite at the relaunched window's
/// centre (dmgovlp's `set_abs` idiom, so the panel position is the shipping conversion), presents the
/// window six times, and reads `wm::cursor12_offer_counts`: the offer must have been PLANNED for a
/// window at least once and `nohit` must not have moved. A relaunch that minted a row the offer
/// cannot see fails this leg and only this leg.
///
/// ### What it deliberately does NOT do
///
/// It does not press the console tile: on this gate the console is the PANEL (no Kepler, no
/// `desktop_uefi::activate`), and launching it would route the boot log into a window for the rest
/// of the run — the console's launch service is the same code path in [`console_launch_service`] and
/// is proven on the bench, where the console IS a window. It closes every window it caused to exist
/// and waits for the quit, so the ladder after it sees the table it started with.
///
/// Verdict grammar (no numerics pinned but the counts the legs themselves fold):
/// ```text
/// :: APPPIN: press1=launch-shell launch1=3:2 quit1=torn-down tile=kept press2=launch-shell launch2=3:3 fresh=true sprite planned=6 nohit=0 cleanup=quit :: PASS ::
/// ```
#[cfg(feature = "witness")]
pub fn apppin_selftest() {
    use core::sync::atomic::AtomicBool as OnceBool;
    static DONE: OnceBool = OnceBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    /// The bound on each wait, in scheduler ticks (1 kHz on x86: 4 s). The render body's pulse is
    /// `X86_GUI_PULSE_MS` (250 ms) and a mint is one allocation, so a wait that reaches this is a
    /// body that is not draining, not a slow one.
    const WAIT_TICKS: u64 = 4000;
    /// Presents driven under the parked sprite. The first pass or two can be excluded by the WC-G
    /// probe / VERIFIED bit (`excl_probe`, `excl_unverified`) on a freshly minted window; six leaves
    /// room for both and still needs only ONE planned offer to pass.
    const SPRITE_PASSES: usize = 6;

    /// The live shell row — `(id, gen)` of the unique `KERNEL_OWNER_DESKTOP` row, from the same
    /// census the tiles are built from.
    fn shell_row() -> Option<(wm::WinId, u32)> {
        let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
        let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
        rows[..n]
            .iter()
            .find(|r| r.owner_asid == wm::KERNEL_OWNER_DESKTOP)
            .map(|r| (r.id, wm::winid_gen(r.id)))
    }
    /// Poll `pred` until it holds or the bound passes. Yields between polls: the body this waits on
    /// may share a core with the ladder.
    fn wait_until(pred: &mut dyn FnMut() -> bool) -> bool {
        let deadline = crate::arch::ticks() + WAIT_TICKS;
        loop {
            if pred() {
                return true;
            }
            if crate::arch::ticks() >= deadline {
                return false;
            }
            crate::arch::sched::yield_now();
        }
    }
    /// The settled strip model, exactly as `press_at` routes over it.
    fn strip_model(rows: &mut [wm::DockEntry; wm::MAX_WINDOWS]) -> usize {
        let (n, _) = wm::dock_scan(rows, (0, 0, 0, 0));
        let n = pin_console(rows, n);
        let n = pin_shell(rows, n);
        let n = pin_quarry(rows, n);
        let n = pin_pulse(rows, n);
        settle(rows, n, false);
        n
    }
    /// Is the shell PIN on the strip right now?
    fn shell_pin_present() -> bool {
        let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
        let n = strip_model(&mut rows);
        rows[..n].iter().any(|r| r.id == SHELL_PIN_ID)
    }
    /// Press the shell PIN at its own tile centre through the router's seam, and report the outcome
    /// word — or why no press could be made.
    fn press_shell_pin() -> &'static str {
        let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
        let n = strip_model(&mut rows);
        let Some(t) = rows[..n].iter().position(|r| r.id == SHELL_PIN_ID) else {
            return "no-pin";
        };
        let (pw, ph) = {
            let fb = *super::WRITER.lock();
            (fb.width(), fb.height())
        };
        let Some(l) = Layout::for_panel(n, pw, ph) else {
            return "no-layout";
        };
        let Some((tx, ty, tw, th)) = l.tile(t) else {
            return "no-tile";
        };
        if !press_at((tx + tw / 2) as i32, (ty + th / 2) as i32) {
            return "unconsumed";
        }
        last_press_outcome()
    }
    /// Wait for the body to answer a press with a FRESH launch (the count moves AND a row exists).
    fn await_launch(before: u64) -> Option<(wm::WinId, u32)> {
        if !wait_until(&mut || app_launches(PinnedApp::Shell) > before && shell_row().is_some()) {
            return None;
        }
        shell_row()
    }
    /// Quit through the disc's path and wait for the body's teardown.
    fn quit_and_await(id: wm::WinId) -> bool {
        let before = app_quits(PinnedApp::Shell);
        wm::close(id);
        wait_until(&mut || app_quits(PinnedApp::Shell) > before && shell_row().is_none())
    }
    /// The sprite leg: `(planned, nohit)` deltas over `SPRITE_PASSES` presents with the sprite parked
    /// at the window's centre.
    fn sprite_leg(id: wm::WinId) -> (u64, u64) {
        let Some(i) = wm::info(id) else {
            return (0, u64::MAX);
        };
        let (pw, ph) = {
            let fb = *super::WRITER.lock();
            (fb.width(), fb.height())
        };
        let (cx, cy) = (i.x + (i.w * i.scale) / 2, i.y + (i.h * i.scale) / 2);
        // `set_abs` takes HID space (0..=32767) — dmgovlp's and wmdirect's conversion, so the panel
        // position the sprite lands on is derived by the shipping code, not by this witness.
        let hid = |v: usize, span: usize| -> i32 {
            ((v as i64 * 32767) / (span as i64 - 1).max(1)) as i32
        };
        let (hx, hy) = (hid(cx, pw), hid(cy, ph));
        let (_, planned0, nohit0) = wm::cursor12_offer_counts();
        for _ in 0..SPRITE_PASSES {
            crate::pal::cursor::set_abs(hx, hy, pw as i32, ph as i32);
            super::cursor::ensure_drawn();
            let _ = wm::present_outcome_owned(id, wm::KERNEL_OWNER_DESKTOP);
        }
        let (_, planned1, nohit1) = wm::cursor12_offer_counts();
        (planned1.saturating_sub(planned0), nohit1.saturating_sub(nohit0))
    }

    // Leg 0 — the precondition: no shell instance is live. On this gate none can be (no desktop
    // takeover, so no bring-up mint), so a live one is a FAIL, not a SKIP: it would mean an earlier
    // fixture leaked a `KERNEL_OWNER_DESKTOP` row, and a skip here would hide that.
    let pre = shell_row();
    // Leg 1 — press the pin, get a fresh window from the real render body.
    let l0 = app_launches(PinnedApp::Shell);
    let press1 = if pre.is_none() { press_shell_pin() } else { "live-shell-precondition" };
    let win1 = if press1 == "launch-shell" { await_launch(l0) } else { None };
    // Leg 2 — quit it; the body tears the instance down; the tile is still there.
    let quit1 = match win1 {
        Some((id, _)) => quit_and_await(id),
        None => false,
    };
    let tile_kept = quit1 && shell_pin_present();
    // Leg 3 — press again: ANOTHER fresh window, never the old row.
    let l1 = app_launches(PinnedApp::Shell);
    let press2 = if tile_kept { press_shell_pin() } else { "skipped" };
    let win2 = if press2 == "launch-shell" { await_launch(l1) } else { None };
    let fresh = matches!((win1, win2), (Some(a), Some(b)) if a != b);
    // Leg 4 — the sprite composes over the relaunched window.
    let (planned, nohit) = match win2 {
        Some((id, _)) => sprite_leg(id),
        None => (0, u64::MAX),
    };
    // Leg 5 — leave the table as it was found.
    let cleanup = match win2 {
        Some((id, _)) => quit_and_await(id),
        None => false,
    };
    let ok = press1 == "launch-shell"
        && win1.is_some()
        && quit1
        && tile_kept
        && press2 == "launch-shell"
        && win2.is_some()
        && fresh
        && planned >= 1
        && nohit == 0
        && cleanup;
    let (w1, g1) = win1.unwrap_or((wm::WIN_NONE, 0));
    let (w2, g2) = win2.unwrap_or((wm::WIN_NONE, 0));
    serial_println!(
        ":: APPPIN: press1={} launch1={}:{} quit1={} tile={} press2={} launch2={}:{} fresh={} sprite planned={} nohit={} cleanup={} :: {} ::",
        press1,
        w1,
        g1,
        if quit1 { "torn-down" } else { "no-teardown" },
        if tile_kept { "kept" } else { "lost" },
        press2,
        w2,
        g2,
        fresh,
        planned,
        nohit,
        if cleanup { "quit" } else { "leaked" },
        if ok { "PASS" } else { "FAIL" }
    );
}

// ------------------------------------------------------------------------------------------------
// DOCKID — TILE IDENTITY: one tile per live window, keyed by (win id, generation), in a position
// that does not move under the operator's hand.  (TAIL-APPENDED: nothing above this line moved, so
// knob-off panic `Location` line numbers are untouched; PARITY.md §5.3)
// ------------------------------------------------------------------------------------------------
//
// # The defect, and where it actually was
//
// Peter, render11, verbatim: *"there's something weird going on with the opening and closing of
// windows who is who between what is open and what is showing in the taskbar. it's all crazy mixed
// up"*.
//
// The tile MODEL was never wrong. [`wm::dock_scan`] re-derives it from the window table on every
// pass, so the SET of tiles has always matched the set of live windows exactly. What was wrong is the
// ORDER, and the order is what an operator's hand knows a tile by. Two independent instabilities,
// both measured on `boot-render11-B-full.log`:
//
//  1. **A pin and its window occupy different positions.** `pin_quarry` PREPENDS, `pin_console`,
//     `pin_shell` and `pin_pulse` APPEND — but when those windows are LIVE their rows come from the
//     scan and sort by WINDOW ID, in the middle. So the settled strip `[quarry] [live rows…]
//     [console] [shell] [pulse]` that those four headers describe is only true while all four are
//     CLOSED. Opening one teleports its tile across the strip.
//
//  2. **Live rows are ordered by a RECYCLED SLOT ALIAS.** `create_inner` mints `id = slot + 1` and
//     takes the lowest free slot, so closing a low-id window and opening another puts the NEW window
//     in the MIDDLE of the strip and shifts every tile to its right by one.
//
// The wire shows both in four consecutive lines. With only win 8 (an app) live the strip was
// `[quarry] [win8] [console] [shell] [pulse]` and win 8's tile was index 1:
//
// ```text
// [dock] press at (792,1166)  tile=1/6 win=8 owner=0x4 …
// [dock] press at (974,1157)  tile=2/5 console=pin -> reopen requested
// [dock] press at (1077,1165) tile=3/5 shell=pin   -> reopen requested
// [dock] press at (1181,1162) tile=4/5 pulse=pin   -> rearmed
// [dock] press at (1182,1161) tile=4/5 win=8 owner=0x4 …      <-- ONE PIXEL later
// ```
//
// Reopening the three furniture windows moved win 8's tile from index 1 to index 4 without the
// operator touching it: two presses one pixel apart resolved to two different windows. That is the
// whole of "who is who … all crazy mixed up", and no amount of correctness in the tile SET fixes it.
//
// A third defect is in the press itself and is fixed with [`wm::raise_one`]: `press_at` raised by
// OWNER (`wm::focus_changed(r.owner_asid)`), which by design raises every window that owner has, so
// with two windows under one owner the tile pressed and the window that came forward were not the
// same window.
//
// # The model this installs
//
// A tile has an IDENTITY, and its identity fixes its position:
//
//  * **Furniture** (quarry, console, shell, pulse) is identified by its OWNER, and the pin and the
//    live row are THE SAME TILE. Its position is a constant ([`fixed_rank`]), so it is the same tile
//    in the same place whether the window is open or closed. This is what makes the four pin headers'
//    "settled strip" claim true in every state instead of only in the all-closed one.
//  * **Everything else** is identified by `(win id, generation)` — `wm::winid_gen`'s per-slot reuse
//    counter, which exists precisely so a capture "can tell the console that was win 1 from the
//    quarry that is win 1 now". Its position is its ARRIVAL RANK, allocated once when the tile is
//    created and held until the window closes. A recycled slot id therefore gets a NEW tile at the
//    END of the app run, never the closed window's old position.
//
// The registry is the dock's own belief about what is open, held ACROSS passes, so "the taskbar
// disagrees with the window manager" becomes a statement two lines of the same capture can settle
// ([`census`] beside `[wm] alloc`/`[wm] close`) instead of an inference from behaviour.
//
// # What did NOT change
//
// The tile SET is still `wm::dock_scan` plus the four pins, unchanged and in the same order of
// application — this block sorts what they produce and does not decide membership. `strip_rect` is
// untouched: the occlusion registry sizes the strip from the tile COUNT, which ordering cannot
// change. The signature, the damage conditions, the painter and the geometry are all unchanged.
//
// # Cost
//
// One extra bounded pass over at most `MAX_WINDOWS` model rows per composite (a key per row, each a
// linear probe of a 12-entry relaxed-atomic table) plus an insertion sort of at most 12 elements over
// precomputed keys. No lock, no allocation, no framebuffer access. The reconcile — which is the only
// mutating half, and the only one that can print — runs from [`compose`] alone on a shipped image
// (the `witness` fixture drives it too; see `reconcile`).

/// DOCKID — the registry's capacity. One entry per non-furniture window the table can hold; furniture
/// is never registered because its position is a constant rather than an arrival rank.
const MAX_TILES: usize = wm::MAX_WINDOWS;

/// DOCKID — the arrival counter. Monotonic, never reused, so a tile's rank is unique for the boot and
/// a window that closes can never hand its position to the window that recycles its slot.
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

/// DOCKID — registry column: the window id this slot's tile names, or `wm::WIN_NONE` for a free slot.
static TILE_ID: [AtomicU32; MAX_TILES] = [const { AtomicU32::new(wm::WIN_NONE) }; MAX_TILES];
/// DOCKID — registry column: the SLOT GENERATION the tile was created at. Half of the identity: a
/// tile whose id is live but whose generation has moved names a window that no longer exists.
static TILE_GEN: [AtomicU32; MAX_TILES] = [const { AtomicU32::new(0) }; MAX_TILES];
/// DOCKID — registry column: the owning ASID, for the remove witness (the row is gone by then, so it
/// cannot be asked).
static TILE_OWNER: [AtomicU64; MAX_TILES] = [const { AtomicU64::new(0) }; MAX_TILES];
/// DOCKID — registry column: the arrival rank, which IS the tile's position among the app tiles.
static TILE_SEQ: [AtomicU64; MAX_TILES] = [const { AtomicU64::new(0) }; MAX_TILES];

/// DOCKID — order key: the leftmost position, Quarry's, whether it is open or closed.
const RANK_QUARRY: u64 = 0;
/// DOCKID — order key base for the app run: `RANK_APPS + seq`, i.e. arrival order.
const RANK_APPS: u64 = 1;
/// DOCKID — order key base for a tile the registry has not seen yet (a window minted since the last
/// [`compose`]). It sorts after every registered app tile and before the permanent tail, by id, so a
/// press that lands between an alloc and the next composite still routes deterministically — and one
/// pass later the tile has its real rank and never moves again.
const RANK_UNSEEN: u64 = 0x4000_0000_0000_0000;
/// DOCKID — the permanent tail, in the order the pin headers name: console, shell, pulse.
const RANK_CONSOLE: u64 = u64::MAX - 2;
const RANK_SHELL: u64 = u64::MAX - 1;
const RANK_PULSE: u64 = u64::MAX;

/// DOCKID — **is this owner FURNITURE with a fixed position, and which?**
///
/// The four owners that have a pin. Their tile is the same tile open or closed, which is the whole
/// point: the pin is not a substitute for the window's tile, it IS the window's tile with the window
/// away. Any other owner — an app, or a kernel row with no pin such as the window menu — is ranked by
/// arrival instead.
fn fixed_rank(owner: u64) -> Option<u64> {
    #[cfg(feature = "quarry")]
    if owner == crate::video::quarry::OWNER {
        return Some(RANK_QUARRY);
    }
    match owner {
        wm::KERNEL_OWNER_CONSOLE => Some(RANK_CONSOLE),
        wm::KERNEL_OWNER_DESKTOP => Some(RANK_SHELL),
        _ if owner == crate::video::pulsewin::OWNER => Some(RANK_PULSE),
        _ => None,
    }
}

/// DOCKID — the registry slot holding `(id, gen)`, or `None`.
fn tile_slot(id: wm::WinId, wgen: u32) -> Option<usize> {
    (0..MAX_TILES).find(|&s| {
        TILE_ID[s].load(Ordering::Relaxed) == id && TILE_GEN[s].load(Ordering::Relaxed) == wgen
    })
}

/// DOCKID — **the order key for one model row.** Pure: it reads the registry and never writes it, so
/// [`press_at`] and `selftest` can order the model without racing [`compose`]'s reconcile.
fn order_key(e: &wm::DockEntry) -> u64 {
    if let Some(k) = fixed_rank(e.owner_asid) {
        return k;
    }
    match tile_slot(e.id, wm::winid_gen(e.id)) {
        Some(s) => RANK_APPS + TILE_SEQ[s].load(Ordering::Relaxed),
        None => RANK_UNSEEN + e.id as u64,
    }
}

/// DOCKID — **sort the assembled model into strip order.** The one place tile POSITION is decided;
/// [`compose`], [`press_at`] and `selftest` all call it on the same model, so painter, router and
/// fixture cannot disagree about which tile is where — the invariant `pin_shell`'s header states for
/// the tile COUNT, extended to the tile ORDER, which is the half the operator's hand actually uses.
///
/// Insertion sort over PRECOMPUTED keys: `n <= MAX_WINDOWS` (12), the keys are unique by construction
/// (a unique arrival rank, four distinct constants, or `RANK_UNSEEN + id`), and the sort is therefore
/// total and deterministic rather than merely stable.
fn order_model(rows: &mut [wm::DockEntry; wm::MAX_WINDOWS], n: usize) {
    let mut key = [0u64; wm::MAX_WINDOWS];
    for i in 0..n {
        key[i] = order_key(&rows[i]);
    }
    for i in 1..n {
        let mut j = i;
        while j > 0 && key[j - 1] > key[j] {
            rows.swap(j - 1, j);
            key.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// DOCKID — a tiny fixed formatting buffer, so [`census`] reaches the wire as ONE `serial_println!`.
/// A census built out of a run of `serial_print!`s could interleave with another core's line, and the
/// standing law in this tree is that the transport is held to a stricter standard than what it
/// reports on: a census that can be cut in half is not evidence.
struct Census {
    b: [u8; 224],
    n: usize,
}

impl core::fmt::Write for Census {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        Ok(())
    }
}

/// DOCKID — the word a PIN tile carries on the wire, or `None` for a real window id.
fn pin_word(id: wm::WinId) -> Option<&'static str> {
    match id {
        SHELL_PIN_ID => Some("shell"),
        QUARRY_PIN_ID => Some("quarry"),
        PULSE_PIN_ID => Some("pulse"),
        CONSOLE_PIN_ID => Some("console"),
        _ => None,
    }
}

/// DOCKID — **the taskbar's belief, on the wire, in strip order.**
///
/// `[dock] census tiles=3 win:gen=1:2,8:1,shell:pin` — every tile, left to right, named by the same
/// `(id, gen)` pair `[wm] alloc` and `[wm] close` name windows by. A capture can then settle "does the
/// dock agree with the window manager" by reading two lines, which is exactly what could not be done
/// on render11: the wm's belief was on the wire and the dock's was not.
///
/// Emitted only when the tile SET changed (see [`reconcile`]), so a quiet desktop prints nothing.
fn census(rows: &[wm::DockEntry], n: usize) {
    use core::fmt::Write;
    let mut c = Census { b: [0u8; 224], n: 0 };
    let _ = write!(c, "[dock] census tiles={} win:gen=", n);
    for (i, r) in rows[..n].iter().enumerate() {
        if i > 0 {
            let _ = write!(c, ",");
        }
        match pin_word(r.id) {
            Some(w) => {
                let _ = write!(c, "{}:pin", w);
            }
            None => {
                let _ = write!(c, "{}:{}", r.id, wm::winid_gen(r.id));
            }
        }
    }
    serial_println!("{}", core::str::from_utf8(&c.b[..c.n]).unwrap_or("[dock] census <unprintable>"));
}

/// DOCKID — **make the registry agree with the model, and say so on the wire.** Returns `true` iff
/// the tile set changed.
///
/// The only mutating half of this block. On a shipped image it runs from [`compose`] ALONE — the
/// pass-driven path — so the registry has exactly one writer and [`order_key`]'s readers never race a
/// partial update. ⚠ Stated exactly: the `witness` fixture [`dockid_selftest`] also drives it, which
/// is a second writer on a `witness` build and is why the claim above is scoped to the metal image
/// rather than made unconditionally. The fixture runs on the boot task and drives the composites it
/// races, so the two are serialised in practice; that is an argument, not a guarantee, and it buys a
/// fixture that asserts the registry rather than a mock of it.
///
/// Two arms, in this order:
///
///  * **retire** — a registered `(id, gen)` that is no longer in the model. `reason=reuse` when the
///    id is back in the model under a DIFFERENT generation (the slot was recycled by
///    `create_inner`), `reason=close` otherwise. The distinction is the one `wm::winid_gen` exists to
///    make and is why the tile cannot outlive its window: a recycled id retires the old tile and
///    admits a new one, rather than silently re-pointing a stale tile at a new window.
///  * **admit** — a model row with no registry entry takes a free slot and the next arrival rank.
///
/// Furniture is skipped by both arms: its rank is a constant, it has no arrival order, and its tile
/// is permanent by design (that is what a pin IS).
fn reconcile(rows: &[wm::DockEntry; wm::MAX_WINDOWS], n: usize) -> bool {
    let mut changed = false;
    for s in 0..MAX_TILES {
        let id = TILE_ID[s].load(Ordering::Relaxed);
        if id == wm::WIN_NONE {
            continue;
        }
        let tgen = TILE_GEN[s].load(Ordering::Relaxed);
        let live = rows[..n]
            .iter()
            .any(|r| r.id == id && fixed_rank(r.owner_asid).is_none() && wm::winid_gen(r.id) == tgen);
        if live {
            continue;
        }
        let reuse = rows[..n].iter().any(|r| r.id == id);
        serial_println!(
            "[dock] tile remove win={} gen={} owner={:#x} reason={}",
            id,
            tgen,
            TILE_OWNER[s].load(Ordering::Relaxed),
            if reuse { "reuse" } else { "close" }
        );
        TILE_ID[s].store(wm::WIN_NONE, Ordering::Relaxed);
        changed = true;
    }
    for r in rows[..n].iter() {
        if fixed_rank(r.owner_asid).is_some() {
            continue;
        }
        let wgen = wm::winid_gen(r.id);
        if tile_slot(r.id, wgen).is_some() {
            continue;
        }
        let Some(s) = (0..MAX_TILES).find(|&s| TILE_ID[s].load(Ordering::Relaxed) == wm::WIN_NONE)
        else {
            // The registry is exactly as large as the window table, so this is unreachable by
            // construction — and it is reported rather than assumed, because "unreachable by
            // construction" is the claim a future MAX_WINDOWS change would silently falsify.
            serial_println!("[dock] tile add win={} gen={} -> DECLINED (registry full)", r.id, wgen);
            continue;
        };
        let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
        TILE_GEN[s].store(wgen, Ordering::Relaxed);
        TILE_OWNER[s].store(r.owner_asid, Ordering::Relaxed);
        TILE_SEQ[s].store(seq, Ordering::Relaxed);
        TILE_ID[s].store(r.id, Ordering::Relaxed);
        serial_println!(
            "[dock] tile add win={} gen={} owner={:#x} seq={} label={}",
            r.id,
            wgen,
            r.owner_asid,
            seq,
            core::str::from_utf8(&r.title[..r.title_len.min(wm::MAX_TITLE)]).unwrap_or("?")
        );
        changed = true;
    }
    changed
}

/// DOCKID — the model assembly every reader shares: reconcile (compose only), then ORDER.
///
/// Split from the pin applications rather than folded into them so the three call sites read the same
/// two lines, and so `strip_rect` — which wants the tile COUNT and nothing else — is not made to pay
/// for an ordering it cannot use.
fn settle(rows: &mut [wm::DockEntry; wm::MAX_WINDOWS], n: usize, reconciling: bool) {
    if reconciling && reconcile(rows, n) {
        order_model(rows, n);
        census(rows, n);
        return;
    }
    order_model(rows, n);
}

/// DOCKID — **the fixture: a tile is the window it names, and it stays where the operator left it.**
///
/// Peter's report was about a SEQUENCE — open some windows, close one, open another — and no leg in
/// this module drove one. `selftest` mints three rows and never closes one mid-flight, so both halves
/// of the render11 defect were invisible to it: the tile-position instabilities only appear ACROSS a
/// close, and the wrong-window raise only appears with two windows under ONE owner.
///
/// Six legs. Legs 1-5 are red on the pre-DOCKID tree; leg 6 is red on the pre-PINCOUNT tree:
///
/// 1. **recycle** — the fixture's own precondition, asserted rather than assumed. Closing the MIDDLE
///    window and opening another must hand the new window the CLOSED one's id (`create_inner` takes
///    the lowest free slot), or legs 2 and 3 prove nothing about recycled ids. A boot where the table
///    happens not to recycle SKIPs rather than passing vacuously.
/// 2. **arrival order** — the new window's tile is to the RIGHT of the older survivor's. On the base
///    tree the model is ordered by WINDOW ID, so a recycled low id puts the newest window in the
///    MIDDLE of the strip and shifts every tile right of it — the second of the two instabilities in
///    this block's header, and the one an operator experiences as their tiles moving by themselves.
/// 3. **set** — the dock's registry and the window table agree: every non-furniture tile names a live
///    window at the generation the tile was created at, and every live dock-addressable non-furniture
///    window has exactly one tile. A tile that outlived its window or predates it fails here.
/// 4. **identity of the press** — two windows under ONE owner, and pressing the LOWER-id one's tile
///    must leave THAT window on top. On the base tree `press_at` raises by ASID alone, and
///    `focus_changed` hands out `z` in table order, so the higher-id sibling always ends in front —
///    the tile pressed and the window raised are not the same window. `wm::raise_one` is the fix and
///    this is its gate.
/// 5. **furniture is anchored** — the four pinned owners rank by [`fixed_rank`] and never by arrival,
///    so a furniture tile is in the same place whether its window is open or closed. Checked against
///    the ordered model rather than against the constants, so a future pin added to `settle` without
///    a rank fails here.
/// 6. **one count, five readers** — two halves. `count=` is the LIVE agreement: the two count-only
///    readers of the model, `wm::dock_tiles` (the occlusion clip's) and [`strip_rect`] (the tenant
///    registry's), report the tile count the pin CHAIN this fixture just ran produced. `pins=` is the
///    STRUCTURAL one: the chain and the [`pins_applied`] fold, over the same empty census, know about
///    the same pins. The second half exists because the first is only conviction-bearing where a
///    SECOND pin is live, and on the x86 `witness` desktop none can be (no `quarry` feature, console
///    routed, `pulsewin::ever_armed()` false) — so on this board `count=` is a regression guard and
///    `pins=` is the leg that convicts. See the PINCOUNT block at this file's tail.
///
/// Self-cleaning: every row it mints is closed and the focus owner is restored. Driven from the tail
/// of [`selftest`] on `menubar::selftest`'s precedent — same `witness` gate, same real panel, same
/// "after every one-shot per-window latch" ordering — because this module's call site lives in
/// `arch/x86_64/syscall.rs`, which is outside this arc's lane.
#[cfg(feature = "witness")]
pub fn dockid_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    /// Six 8x8 ARGB8888 surfaces in rodata — read-only, because the compositor only reads.
    static SURF: [[u32; 64]; 6] = [
        [0x0020_40FF; 64], [0x0040_FF20; 64], [0x00FF_4020; 64],
        [0x00FF_FF20; 64], [0x0020_FFFF; 64], [0x00FF_20FF; 64],
    ];
    /// Four distinct app owners for legs 1-3, then ONE owner shared by the last two rows for leg 4.
    /// Ordinary ASIDs, deliberately outside the reserved kernel band: [`fixed_rank`] must rank these
    /// by ARRIVAL, and a kernel-band owner would be ranked by constant and prove nothing.
    const OWNERS: [u64; 6] = [0xD1D1, 0xD1D2, 0xD1D3, 0xD1D4, 0xD1D5, 0xD1D5];
    const NAMES: [&[u8]; 6] = [b"idA", b"idB", b"idC", b"idD", b"idE", b"idF"];

    let mint = |k: usize| {
        wm::create(
            OWNERS[k],
            SURF[k].as_ptr() as usize,
            core::mem::size_of_val(&SURF[k]),
            8,
            8,
            // STRIDE IS IN BYTES — `create_inner`'s extent contract, exactly as `selftest` states it.
            32,
            NAMES[k],
        )
    };
    /// The ordered strip model, and the index of `id` in it — the router's own view, assembled
    /// EXACTLY the way [`press_at`] assembles it (scan, pins, `settle(.., false)`), so a leg cannot
    /// pass against an order the router never computes.
    ///
    /// ⚠ **It drives `wm::composite()` first and reconciles NOTHING itself, and that is the point.**
    /// The registry's writer on a shipped image is [`compose`]; if this fixture assembled the ranks
    /// itself it would prove its own arithmetic and nothing about the path the operator's clicks take.
    /// This cost one real defect before it was written this way: the three `settle` calls were folded
    /// onto their lines to the RIGHT of an existing `//`, so every one of them was commented out —
    /// `compose` and `press_at` shipped unfixed, `strings kernel.elf` found no `[dock] census` in the
    /// artifact, and a fixture that assembled its own model reported PASS over the top of it. With the
    /// reconcile left to `compose`, a dead fold empties the registry, every app tile falls back to
    /// `RANK_UNSEEN + id`, the strip returns to WINDOW-ID order, and legs 2 and 3 go red.
    fn strip_model(rows: &mut [wm::DockEntry; wm::MAX_WINDOWS]) -> usize {
        wm::composite();
        let (n, _) = wm::dock_scan(rows, (0, 0, 0, 0));
        let n = pin_console(rows, n);
        let n = pin_shell(rows, n);
        let n = pin_quarry(rows, n);
        let n = pin_pulse(rows, n);
        settle(rows, n, false);
        n
    }

    let saved_focus = focus_get();
    let mut w = [wm::WIN_NONE; 6];
    for k in 0..3 {
        w[k] = mint(k);
    }
    if w[..3].iter().any(|&i| i == wm::WIN_NONE) {
        for &i in w.iter() {
            if i != wm::WIN_NONE {
                wm::close(i);
            }
        }
        serial_println!(":: DOCKID: fixture — table full, wins={:?} :: SKIP ::", &w[..3]);
        return;
    }
    // Register the three arrivals before anything closes: the arrival ranks are what leg 2 reads.
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let _ = strip_model(&mut rows);

    // Close the MIDDLE window and open another. This is Peter's sequence, and `create_inner` hands
    // the new window the closed one's slot id.
    wm::close(w[1]);
    w[3] = mint(3);
    if w[3] == wm::WIN_NONE {
        for &i in [w[0], w[2]].iter() {
            wm::close(i);
        }
        serial_println!(":: DOCKID: fixture — table full at the reopen :: SKIP ::");
        return;
    }
    let recycle_ok = w[3] == w[1];

    let n = strip_model(&mut rows);
    let at = |rows: &[wm::DockEntry; wm::MAX_WINDOWS], n: usize, id: wm::WinId| {
        rows[..n].iter().position(|r| r.id == id)
    };
    // Leg 2 — arrival order: the survivor A, then the survivor C, then the NEW window D. On the base
    // tree D carries B's recycled id and lands between A and C.
    let order_ok = match (at(&rows, n, w[0]), at(&rows, n, w[2]), at(&rows, n, w[3])) {
        (Some(a), Some(c), Some(d)) => a < c && c < d,
        _ => false,
    };
    // Leg 3 — the registry and the table agree, in both directions.
    let mut set_ok = true;
    for s in 0..MAX_TILES {
        let id = TILE_ID[s].load(Ordering::Relaxed);
        if id == wm::WIN_NONE {
            continue;
        }
        if wm::winid_gen(id) != TILE_GEN[s].load(Ordering::Relaxed) || wm::info(id).is_none() {
            set_ok = false; // a tile that outlived its window, or names a recycled slot
        }
    }
    for r in rows[..n].iter() {
        if fixed_rank(r.owner_asid).is_some() {
            continue;
        }
        if tile_slot(r.id, wm::winid_gen(r.id)).is_none() {
            set_ok = false; // a live window with no tile
        }
    }
    // Leg 5 — furniture is anchored: every pinned owner in the model carries its constant rank, and
    // the model is sorted by it (so the tail really is the tail).
    let mut furniture_ok = true;
    let mut last = 0u64;
    for r in rows[..n].iter() {
        let k = order_key(r);
        if k < last {
            furniture_ok = false;
        }
        last = k;
        if pin_word(r.id).is_some() && fixed_rank(r.owner_asid).is_none() {
            furniture_ok = false; // a pin with no fixed rank would float on arrival order
        }
    }

    // Leg 6 — PINCOUNT: FOUR pins, ONE count, FIVE readers. The two COUNT-ONLY readers must report
    // the strip the painter will paint. `wm::dock_tiles` is the one that mirrored `pin_shell` alone
    // and therefore ran up to two tiles short whenever the console, Quarry or pulse pin was up — it
    // sizes `occ_clip`'s per-blit dock term and `erase_clip`'s strip rect, so short means a strip
    // tail nothing clips and a drag across it clobbers. `strip_rect` is the tenant-registry hook that
    // publishes the rect. Both are scored against `n`, the settled model this fixture assembled
    // through the mutating pin chain itself — so the leg compares the FOLD against the CHAIN, which
    // is the equivalence `pins_applied`'s header claims, rather than one copy against another.
    let tiles_probe = wm::dock_tiles_probe();
    let (cw, ch) = {
        let fb = *super::WRITER.lock();
        (fb.width(), fb.height())
    };
    let count_ok =
        tiles_probe == n && strip_rect(cw, ch) == Layout::for_panel(n, cw, ch).map(|l| l.rect());
    // ...and the half that does NOT depend on which pins happen to be up. The comparison above is
    // only conviction-bearing when a SECOND pin is live, and on the x86 `witness` desktop none can
    // be: there is no `quarry` feature in this leg, the console is routed (so its pin is suppressed
    // by design) and `pulsewin::ever_armed()` is false on every `desktop_uefi` boot. So the count
    // path is also gated STRUCTURALLY: run the mutating pin CHAIN over an empty census and the
    // `pins_applied` FOLD over the same census, and require the same answer. A pin added to the
    // chain without a `*_wanted` predicate in the fold — which is precisely what `wm::dock_tiles`
    // carried for three pins — makes the chain count higher than the fold and reds this leg on any
    // board. Local scratch, so nothing global moves: the one write in the chain is `pin_console`'s
    // `CONSOLE_WINDOWED` latch, which is idempotent and is the state `compose` already published.
    let mut probe = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let chain = {
        let m = pin_console(&mut probe, 0);
        let m = pin_shell(&mut probe, m);
        let m = pin_quarry(&mut probe, m);
        pin_pulse(&mut probe, m)
    };
    let chain_ok = chain == pins_applied(0, |_| false);

    // Leg 4 — the identity of the press, with TWO windows under ONE owner.
    w[4] = mint(4);
    w[5] = mint(5);
    let press_ok = if w[4] == wm::WIN_NONE || w[5] == wm::WIN_NONE {
        true // no room to run the leg; legs 1-3 and 5 still stand. Reported as `press=skip` below.
    } else {
        // Press the LOWER-id sibling's tile: `focus_changed`'s ASID raise walks the table in array
        // order, so on the base tree the HIGHER id always ends on top and this leg is deterministic.
        let (lo, hi) = if w[4] < w[5] { (w[4], w[5]) } else { (w[5], w[4]) };
        let n2 = strip_model(&mut rows);
        let (pw, ph) = {
            let fb = *super::WRITER.lock();
            (fb.width(), fb.height())
        };
        match (Layout::for_panel(n2, pw, ph), at(&rows, n2, lo)) {
            (Some(l), Some(k)) => match l.tile(k) {
                Some((tx, ty, tw, th)) => {
                    let consumed = press_at((tx + tw / 2) as i32, (ty + th / 2) as i32);
                    let z = |id| wm::info(id).map(|i| i.z).unwrap_or(0);
                    consumed && z(lo) > z(hi)
                }
                None => false,
            },
            _ => false,
        }
    };
    let press_ran = w[4] != wm::WIN_NONE && w[5] != wm::WIN_NONE;

    for &i in [w[0], w[2], w[3], w[4], w[5]].iter() {
        if i != wm::WIN_NONE {
            wm::close(i);
        }
    }
    focus_set(saved_focus);
    wm::focus_changed(saved_focus);

    let ok =
        recycle_ok && order_ok && set_ok && furniture_ok && count_ok && chain_ok && press_ok;
    serial_println!(
        ":: DOCKID: tiles={} closed=win{} reopened=win{} recycle={} order={} set={} furniture={} count={}/{} pins={}/{} press={} :: {} ::",
        n,
        w[1],
        w[3],
        recycle_ok,
        order_ok,
        set_ok,
        furniture_ok,
        count_ok,
        tiles_probe,
        chain_ok,
        chain,
        if press_ran { if press_ok { "yes" } else { "no" } } else { "skip" },
        if ok { "PASS" } else { "FAIL" }
    );
}

// ------------------------------------------------------------------------------------------------
// PINCOUNT — FOUR pins, ONE count, FIVE readers (TAIL-APPENDED: nothing above this line moved, so
// knob-off panic `Location` line numbers are untouched; PARITY.md §5.3)
// ------------------------------------------------------------------------------------------------
//
// # The defect
//
// This module grew four pins one at a time — [`pin_shell`] (appends), [`pin_quarry`] (prepends,
// `quarry`-gated), [`pin_pulse`] (appends, gated on the runtime `pulsewin::ever_armed()`) and
// [`pin_console`] (appends first, gated on the `CONSOLE_WINDOWED` latch) — and each one's header
// carries the same promise: *"applied by every reader of the model, so painter, router and occlusion
// registry cannot disagree about the tile count."*
//
// Four of the five readers kept it. [`compose`], [`press_at`], [`strip_rect`] and `dockid_selftest`
// all apply the pins. The FIFTH is `wm::dock_tiles`, which feeds `wm::occ_clip`'s per-window-blit
// dock term and `wm::erase_clip`'s strip rect, and it mirrored **`pin_shell` alone** — by hand, with
// its own transcription of that pin's condition and its own `+ 1`. So on any desktop where the
// console, Quarry or the pulse instrument is CLOSED, the occlusion clip was sized for a strip up to
// two tiles narrower than the one the painter drew, and a window dragged across the uncovered tail
// clobbered it until the next damage pass repainted the strip. Exactly the defect the `SHELLPIN`
// `+ 1` was added to fix, re-entered once per pin added after it.
//
// # The fix, and why it is not a fifth transcription
//
// [`pins_applied`] is the ONE definition of the pin arithmetic: the four conditions, in application
// order, under the same per-pin `n < MAX_WINDOWS` cap the mutating chain applies. Each pin's
// condition is stated ONCE, in a `*_wanted` predicate that the pin itself consults, so a pin and the
// count cannot drift — changing a pin's rule changes both, or compiles as neither.
//
// `wm::dock_tiles` calls it on the rows its caller already holds; [`strip_rect`], which wants the
// count and nothing else, calls it instead of assembling a model it then throws away. The other
// three readers need the pinned ROWS (to paint them, to route a press to them, to order them), so
// they keep the mutating chain — which is the same fold, and the predicates are shared.
//
// ## The lock hazard this shape exists to respect
//
// `wm::dock_tiles` may NOT call `wm::dock_scan`: its callers (`occ_clip` inside the blit loop, and
// `erase_clip`) are already holding the window TABLE, and `dock_scan` takes it. That is why the
// census is passed in as a CLOSURE over rows the caller has in hand rather than gathered here, and
// why [`pins_applied`] is pure — it reads runtime cells (`pulsewin::ever_armed`, `console_is_routed`,
// `CONSOLE_WINDOWED`) and writes none. The one write in the pin chain, `pin_console`'s latch of
// `CONSOLE_WINDOWED`, stays in `pin_console` and runs from `compose` alone.
//
// ## Why a fold over the bare scan equals the mutating chain
//
// The pins do not interact. Each tests a DISTINCT owner — `KERNEL_OWNER_CONSOLE`,
// `KERNEL_OWNER_DESKTOP`, `quarry::OWNER` — or, for pulse, no owner at all (`is_open()` is a runtime
// cell, not a row scan). A pin therefore cannot see, or be suppressed by, a row an earlier pin
// inserted, so applying the four to a bare `dock_scan` census gives the same count as applying them
// to the progressively pinned model. Only the per-pin cap is order-sensitive, and [`pins_applied`]
// applies it in the same order for the same reason.

/// PINCOUNT — the census a pin's condition asks its caller: **is there a live dock-addressable row
/// with this owner?** A closure rather than a slice because the two count-only readers hold
/// different things — `strip_rect` a scanned `DockEntry` model, `wm::dock_tiles` the raw window
/// table it may not re-scan under the lock its caller holds.
type Present<'a> = &'a dyn Fn(u64) -> bool;

/// PINCOUNT — [`pin_console`]'s condition, stated once. The `console_is_routed()` term is the LIVE
/// arm (a live console row is its own raise route and a second tile would be a second console).
/// APPPIN: no "has a console window ever existed" term — the console is a pinned app and its tile is
/// permanent, on every desktop, whether or not this boot has minted its window yet.
fn pin_console_wanted(n: usize, present: Present<'_>) -> bool {
    n < wm::MAX_WINDOWS
        && !crate::video::fbcon::console_is_routed()
        && !present(wm::KERNEL_OWNER_CONSOLE)
}

/// PINCOUNT — [`pin_shell`]'s condition, stated once: one live shell window max.
fn pin_shell_wanted(n: usize, present: Present<'_>) -> bool {
    n < wm::MAX_WINDOWS && !present(wm::KERNEL_OWNER_DESKTOP)
}

/// PINCOUNT — [`pin_quarry`]'s condition, stated once. `cfg`-gated in both polarities exactly as the
/// pin is, so a build without the file manager counts no tile for it and compiles no reference to it.
#[cfg(feature = "quarry")]
fn pin_quarry_wanted(n: usize, present: Present<'_>) -> bool {
    n < wm::MAX_WINDOWS && !present(crate::video::quarry::OWNER)
}

/// PINCOUNT — the erasing twin. No file manager, no tile, and the count is unchanged.
#[cfg(not(feature = "quarry"))]
#[inline(always)]
fn pin_quarry_wanted(_n: usize, _present: Present<'_>) -> bool {
    false
}

/// PINCOUNT — [`pin_pulse`]'s condition, stated once. It takes no census: the instrument's presence
/// is two runtime cells, and `ever_armed()` is false on every board that never had the window (every
/// x86 `desktop_uefi` desktop), which is what keeps this pin off those images entirely.
fn pin_pulse_wanted(n: usize) -> bool {
    n < wm::MAX_WINDOWS
        && crate::video::pulsewin::ever_armed()
        && !crate::video::pulsewin::is_open()
}

/// PINCOUNT — **how many tiles the four pins add to a census of `n` dock-addressable rows.**
///
/// The one place the pin arithmetic lives. Pure: no lock, no allocation, no store — see the block
/// header for why `wm::dock_tiles` could not have called `dock_scan` instead, and for why folding
/// over a bare census is equivalent to the mutating chain [`compose`] runs.
///
/// Never a hand-rolled `+ N`: a pin added to the chain above without a `*_wanted` predicate here is
/// a pin the occlusion clip cannot see, which is the defect this exists to close.
pub(super) fn pins_applied(n: usize, present: impl Fn(u64) -> bool) -> usize {
    let mut n = n;
    if pin_console_wanted(n, &present) {
        n += 1;
    }
    if pin_shell_wanted(n, &present) {
        n += 1;
    }
    if pin_quarry_wanted(n, &present) {
        n += 1;
    }
    if pin_pulse_wanted(n) {
        n += 1;
    }
    n
}
