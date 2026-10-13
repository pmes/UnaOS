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

//! CRISPY-PI — the Crispy desktop theme as a kernel-side `const` table.
//!
//! # Source of record
//!
//! `kits/crispy/theme.json` @ branch `us-crispy-modern`, commit `0787ba9f`. The
//! host-side reader is `libs/quartzite/src/theme.rs` at the same commit.
//!
//! CRISPY-PI-2 re-lifted this table from that commit, replacing the provisional
//! `us-crispy` `08b42ede` values CRISPY-PI carried.
//!
//! **Shared-source law.** Both arches (aarch64 Pi 4 and x86_64) source chrome and
//! desktop constants from *this* table, which in turn mirrors *that* json. No
//! per-arch invented numbers, ever. If a value needs to change, it changes in the
//! kit json first and is re-lifted here.
//!
//! **Taste gate is CLOSED — APPROVED** (iteration 3, Peter, 2026-07-26). These are
//! no longer provisional: the visual verdict has been taken on the kit these
//! numbers come from. A later verdict change edits the VALUE in `unaos/libs/sys/palette_core` (PALETTECORE,
//! rmbp-ledger B559: the one no_std table both rings link) and its provenance HERE — every consumer, kernel or
//! ring 3, reads the names, never the literals. This file re-exports every colour; it spells none.
//!
//! # Wiring status
//!
//! **WIRED (CRISPYWIRE).** `video/wm.rs` consumes this table: every chrome colour and every
//! chrome metric the window compositor draws with resolves to a role or a metric below, on both
//! arches. `wm::TITLE_H` is [`TITLE_HEIGHT`], `wm::BORDER` is [`FRAME`], the tiler's gap is
//! [`GAP`]. The module is no longer byte-inert — that was a property of having no consumers, not
//! a goal.
//!
//! CRISPYWIRE-REVIEW adds [`TEXT_PX`] to that set: the window caption is drawn at the nearest
//! rasterized cell toward it — FONT (GR27): the shared anti-aliased face's 16 px cell
//! (`wm::TITLE_CELL_H`; it was `TITLE_SCALE = 2` over the 8-px bitmap before the face landed, the
//! same 16 px drawn for the 15 asked). [`LINE_HEIGHT_PCT`] remains the one metric with no consumer — window captions are
//! a single line, so there is no inter-line distance for it to set.
//!
//! ### Still UN-WIRED — invented colours, next arc's scope
//!
//! The claim above is scoped to the **window compositor's chrome**. These modules paint the same
//! glass from constants of their own, and this pass deliberately did not touch them (one arc, one
//! scope) — they are named here so the gap is disclosed rather than implied away:
//!
//!  * **`screen.rs` and fbcon** — untouched, and there is still **no desktop-background role in the
//!    kit** for them to read.
//!  * **`ui_status.rs`** — the bottom instrument strip: `METER_DIM`, `METER_BREATH`, `METER_PARKED`
//!    (lines 229/232/234) and the three VU colours `LED_GREEN` / `LED_AMBER` / `LED_RED` (727-729).
//!    The LED ramp is arguably an instrument rather than chrome, which is itself a taste question.
//!  * **`video/cursor.rs`** — the pointer sprite's `FILL`/`SHADOW` (110/112), duplicated verbatim in `pal.rs`'s `cursor` module (166-177).
//!    ⛔ THE POINTER IS UN-THEMED, NOT MERELY UN-WIRED, AND THAT IS A VERDICT RATHER THAN A TODO (PA38): `kits/crispy/theme.json` and
//!    `kit.json` define NO cursor/pointer/arrow role at all — `quartzite`'s `Palette`/`Metrics` carry none either (its `caret` is a
//!    text-layout pen advance, not a pointer) and the kit's clean-room line disclaims imported cursors outright. There is nothing to
//!    lift, so the 8x8 `ARROW` and its two colours stay INVENTED and no arc may quietly author a shape here: the pointer is a taste
//!    question that belongs to Peter and closes with a KIT REVISION, not a kernel edit.
//!
//! See `docs/dev/OS/08_VIDEO/engine.md` §CRISPYWIRE and §CURSOR-VANISH.
//!
//! A verdict change still edits ONE table (`palette_core`, the values) and this header (the provenance): every
//! consumer reads the names.
//!
//! # Representation
//!
//! Colours are packed `0x00RRGGBB` — the json palette carries **no per-colour alpha**
//! (every role is a 3-element sRGB triple), so the top byte is zero rather than an
//! invented opaque `0xFF`. The gloss layer is the one place alpha appears, and it
//! appears there as *separate scalar fields*, lifted below as **Q16 fixed point**
//! (`value * 65536`) rather than `u8` — see the fidelity note.
//!
//! # What is NOT lifted
//!
//! The json's `content_surface.Paper` block (`base_rgb`, `algo: "Laid"`,
//! `amplitude: 0.02`, `scale: 4.0`, `octaves: 3`, `seed: 4223012511`) is
//! **deliberately absent**, not an oversight. It is a *material* — quartzite's
//! `surface.rs` layer, a procedurally generated paper texture that a content region
//! composites under its content — not chrome, and not a constant: lifting it means
//! porting a multi-octave noise generator, which is a rasterizer concern. When it
//! lands it belongs beside the surface/material code (`docs/dev/OS/08_VIDEO/`
//! rasterizer lane), reading `base_rgb` from `CONTENT_FILL` here — the two agree by
//! construction, both being `[0.96, 0.949, 0.918]`. This module stays palette +
//! metrics only.
//!
//! # The pinned rounding rule
//!
//! quartzite's `theme.rs` converts a channel with:
//!
//! ```text
//! fn to_u8(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 }
//! ```
//!
//! i.e. **clamp to `[0,1]`, multiply by 255, add 0.5, then truncate toward zero**
//! (`as u8` on a non-negative `f32` truncates) — round-half-up on the f32 product,
//! evaluated in `f32` precision at every step. Every literal below was produced by
//! that exact rule, in f32, from the json value quoted in its provenance comment. No
//! float math survives into the kernel: the rounding happened here, at authoring time.
//!
//! # How far the fidelity claim reaches
//!
//! Bit-for-bit agreement with a quartzite-drawn pixel holds for **flat fills** — any
//! surface painted with a palette role at full opacity. It does **not** extend to the
//! gloss ramp, for two independent reasons:
//!
//!  1. the gloss scalars are quantized here, and a quantized endpoint is not the f32
//!     the host uses (`0.5` as `u8` would be `128`, i.e. `0.50196`);
//!  2. more fundamentally, the host does not round the *endpoints* at all — it
//!     interpolates in f32 across the ramp and rounds each *composited per-pixel*
//!     alpha. No table of endpoint constants can reproduce that by itself; matching
//!     it is a property of the interpolator the wiring arc writes.
//!
//! So the gloss scalars below are carried at **Q16** (`value * 65536`, same
//! round-half-up rule) rather than `u8`: Q16 makes the endpoint error ~4e-6 instead
//! of ~2e-3, which keeps the residual error entirely in the interpolator where it
//! belongs, and leaves no lossy `u8` sitting here as a trap for the wiring arc. The
//! gradient stops (`TITLE_*_TOP`/`BOTTOM`) are exact colours and are unaffected by
//! any of this; only the *interpolation between* them carries the same caveat.

// ---------------------------------------------------------------------------
// Palette — 21 roles, packed 0x00RRGGBB. Every provenance comment below quotes
// the json triple at `us-crispy-modern` `0787ba9f`; the value is `palette_core`'s (B559), re-exported by name.
// ---------------------------------------------------------------------------

/// `palette.chrome_face` = `[0.925, 0.925, 0.933]` @ `0787ba9f` — the window body fill.
pub use palette_core::CHROME_FACE;

/// `palette.bevel_light` = `[1.0, 1.0, 1.0]` @ `0787ba9f` — top/left bevel edge.
pub use palette_core::BEVEL_LIGHT;

/// `palette.bevel_shadow` = `[0.667, 0.667, 0.686]` @ `0787ba9f` — bottom/right bevel edge.
pub use palette_core::BEVEL_SHADOW;

/// `palette.frame_line` = `[0.706, 0.706, 0.725]` @ `0787ba9f` — the outer keyline of the frame.
pub use palette_core::FRAME_LINE;

/// `palette.title_active_top` = `[0.933, 0.933, 0.945]` @ `0787ba9f` — focused title gradient, top stop.
pub use palette_core::TITLE_ACTIVE_TOP;

/// `palette.title_active_bottom` = `[0.89, 0.89, 0.91]` @ `0787ba9f` — focused title gradient, bottom stop.
pub use palette_core::TITLE_ACTIVE_BOTTOM;

/// `palette.title_inactive_top` = `[0.957, 0.957, 0.961]` @ `0787ba9f` — unfocused title gradient, top stop.
pub use palette_core::TITLE_INACTIVE_TOP;

/// `palette.title_inactive_bottom` = `[0.937, 0.937, 0.945]` @ `0787ba9f` — unfocused title gradient, bottom stop.
pub use palette_core::TITLE_INACTIVE_BOTTOM;

/// `palette.title_text_active` = `[0.145, 0.149, 0.161]` @ `0787ba9f` — focused title caption ink.
pub use palette_core::TITLE_TEXT_ACTIVE;

/// `palette.title_text_inactive` = `[0.478, 0.478, 0.494]` @ `0787ba9f` — unfocused title caption ink.
pub use palette_core::TITLE_TEXT_INACTIVE;

/// `palette.button_face` = `[0.973, 0.973, 0.98]` @ `0787ba9f` — resting button face.
pub use palette_core::BUTTON_FACE;

/// `palette.button_face_pressed` = `[0.878, 0.878, 0.89]` @ `0787ba9f` — pressed button face.
pub use palette_core::BUTTON_FACE_PRESSED;

/// `palette.button_text` = `[0.125, 0.129, 0.141]` @ `0787ba9f` — button label ink.
pub use palette_core::BUTTON_TEXT;

/// `palette.content_fill` = `[0.96, 0.949, 0.918]` @ `0787ba9f` — content-region base fill.
///
/// Unchanged across the CRISPY-PI-2 re-lift: iteration 3 kept the paper tone, and
/// `content_surface.Paper.base_rgb` still agrees with it by construction.
pub use palette_core::CONTENT_FILL;

/// `palette.content_text` = `[0.129, 0.125, 0.118]` @ `0787ba9f` — content-region ink.
pub use palette_core::CONTENT_TEXT;

/// `palette.scroll_track` = `[0.949, 0.949, 0.957]` @ `0787ba9f` — scrollbar trough.
pub use palette_core::SCROLL_TRACK;

/// `palette.scroll_thumb` = `[0.796, 0.796, 0.812]` @ `0787ba9f` — scrollbar thumb.
pub use palette_core::SCROLL_THUMB;

/// `palette.accent` = `[0.29, 0.451, 0.667]` @ `0787ba9f` — selection / focus accent.
pub use palette_core::ACCENT;

// ---------------------------------------------------------------------------
// Title-bar controls. Iteration 3 replaces the single square `CONTROL_BOX` role
// with three *circular* controls, each with its own fill: a left-to-right ramp of
// the accent hue from darkest (close) to lightest (zoom). The geometry is still
// one number — `CONTROL_BOX` below, now read as a circle's diameter.
// ---------------------------------------------------------------------------

/// `palette.control_close` = `[0.239, 0.373, 0.573]` @ `0787ba9f` — close control fill.
pub use palette_core::CONTROL_CLOSE;

/// `palette.control_mid` = `[0.404, 0.549, 0.729]` @ `0787ba9f` — middle (minimise) control fill.
pub use palette_core::CONTROL_MID;

/// `palette.control_zoom` = `[0.573, 0.667, 0.788]` @ `0787ba9f` — zoom control fill.
pub use palette_core::CONTROL_ZOOM;

// ---------------------------------------------------------------------------
// SEMANTIC control fills — Peter's ruling, 2026-08-09 (white board Q9, answer
// "b on a red yellow green top left of window").
//
// ⛔ PROVENANCE, STATED PLAINLY: these three are DERIVED, not lifted. They do not
// appear in any `palette.*` entry of any kit revision, because `kits/crispy/` is
// not reachable from this repo (white-board Q4, the standing gap; the shared-source
// law currently rests on in-tree triangulation). They are entered here as the
// authored source of record and are PENDING RE-LIFT into
// `kits/crispy/theme.json` — as `palette.ctrl_close` / `_min` / `_zoom` — the
// moment that kit is reachable. No kit hash is cited above them, deliberately: a
// fabricated citation would be worse than the gap it papers over.
//
// ### The register is APPLE'S, by ruling — superseding this arc's own derivation
//
// These three were first entered as MUTED derivations (`#C25F55 / #C89C52 /
// #5E9468`): the macOS hues pulled down to the Scandinavian-minimal saturation the
// rest of this table sits in, on the argument that fully saturated signals fight the
// near-white chrome (`0xEEEEF1`) and the muted `ACCENT` (`0x4A73AA`).
//
// Peter is the taste gate and he overruled it, 2026-08-09, verbatim:
//
//     "same color as mac but knurled if possible to add more texture"
//
// So the values below are now the **macOS standard hues, unmodified**, and the
// texture that was to have come from restraint comes instead from the material:
// `video/knurl.rs` mills a diamond crosshatch into the discs at the same
// 2 %-of-a-channel budget `paper` and `ceramic` share. The earlier muted triple is
// recorded here rather than deleted, because the argument for it was recorded and a
// reversed decision should show its own reversal.
//
// The three blue `CONTROL_*` roles above are KEPT. They are the kit's own ramp and
// this arc has no authority to delete a lifted role; nothing in the tree consumes
// them since `paint_window` moved to the semantic set, so they stand as the record
// of what the kit says until the re-lift reconciles the two.
// ---------------------------------------------------------------------------

/// DERIVED (Peter's ruling, 2026-08-09, *"same color as mac"*) — CLOSE control fill:
/// the macOS standard close hue, `#FF5F57`, hue 3°. Supersedes this table's earlier
/// muted derivation `#C25F55`. Pending re-lift as `palette.ctrl_close`.
///
/// Its red channel is already `0xFF`, so `knurl`'s crest CLIPS on this role and the
/// crosshatch reads there as its trough alone — disclosed in `knurl.rs` and pinned by
/// that module's leg 4 rather than left as an unchecked note.
pub use palette_core::CTRL_CLOSE;

/// DERIVED (Peter's ruling, 2026-08-09, *"same color as mac"*) — MINIMISE control
/// fill: the macOS standard minimise hue, `#FEBC2E`, hue 43°. Supersedes this table's
/// earlier muted derivation `#C89C52`. Pending re-lift as `palette.ctrl_min`.
pub use palette_core::CTRL_MIN;

/// DERIVED (Peter's ruling, 2026-08-09, *"same color as mac"*) — ZOOM control fill:
/// the macOS standard zoom hue, `#28C840`, hue 128°. Supersedes this table's earlier
/// muted derivation `#5E9468`. Pending re-lift as `palette.ctrl_zoom`.
pub use palette_core::CTRL_ZOOM;

// ---------------------------------------------------------------------------
// Gloss — `palette.gloss`. A white highlight applied with a two-stop alpha
// falloff. The three scalars are unit values in the json; they are carried here as
// **Q16 fixed point** (`value * 65536`, same round-half-up rule) rather than `u8`,
// because they are ramp *endpoints* fed to an interpolator, not final pixel alphas.
// See the module header: the gloss ramp is explicitly outside the bit-for-bit
// claim, and Q16 keeps the endpoint error negligible so the only error left is the
// interpolator's own. Convert to 0..=255 at the point of use.
// ---------------------------------------------------------------------------

/// Fixed-point scale for the gloss scalars: `Q16_ONE` represents `1.0`.
pub const Q16_ONE: u32 = 65536;

/// `palette.gloss.highlight` = `[1.0, 1.0, 1.0]` @ `0787ba9f` — the gloss colour.
///
/// Unchanged across the CRISPY-PI-2 re-lift; note that iteration 3 also takes
/// `BEVEL_LIGHT` to pure white, so the two roles now coincide numerically. They stay
/// separate names because they are separate roles in the json and may diverge again.
pub use palette_core::GLOSS_HIGHLIGHT;

/// `palette.gloss.top_alpha` = `0.14` @ `0787ba9f` — gloss opacity at the top edge, Q16.
pub const GLOSS_TOP_ALPHA_Q16: u32 = 9175;

/// `palette.gloss.falloff` = `0.5` @ `0787ba9f` — gloss falloff shape parameter, Q16.
///
/// The host stores this as a unit scalar and clamps it to `[0.01, 1.0]` before use.
pub const GLOSS_FALLOFF_Q16: u32 = 32768;

/// `palette.gloss.bottom_alpha` = `0.0` @ `0787ba9f` — gloss opacity at the bottom edge, Q16.
///
/// Iteration 3 takes the ramp to fully transparent at the bottom: the gloss now dies
/// out entirely rather than leaving CRISPY-PI's `0.06` floor across the lower chrome.
pub const GLOSS_BOTTOM_ALPHA_Q16: u32 = 0;

// ---------------------------------------------------------------------------
// Metrics — `metrics.*`, all integral in the json, all pixels unless noted.
// ---------------------------------------------------------------------------

/// `metrics.frame` = `5` @ `0787ba9f` — frame thickness, px.
#[allow(non_snake_case)] #[inline] pub fn FRAME() -> usize { crate::ui::px(crate::ui::base::FRAME) } // UIMETRICS: was `const` 5 device px

/// `metrics.bevel` = `1` @ `0787ba9f` — bevel thickness, px. Iteration 3 makes the
/// bevel a true hairline.
#[allow(non_snake_case)] #[inline] pub fn BEVEL() -> usize { crate::ui::px(crate::ui::base::BEVEL) } // UIMETRICS: was `const` 1

/// `metrics.title_height` = `34` @ `0787ba9f` — title bar height, px.
#[allow(non_snake_case)] #[inline] pub fn TITLE_HEIGHT() -> usize { crate::ui::px(crate::ui::base::TITLE_HEIGHT) } // UIMETRICS: was `const` 34

/// `metrics.corner_radius` = `12` @ `0787ba9f` — radius of the two *top* corners, px.
#[allow(non_snake_case)] #[inline] pub fn CORNER_RADIUS() -> usize { crate::ui::px(crate::ui::base::CORNER_RADIUS) } // UIMETRICS: was `const` 12

/// `metrics.widget_radius` = `8` @ `0787ba9f` — corner radius for widgets (buttons and
/// other raised controls), px. New in iteration 3.
#[allow(non_snake_case)] #[inline] pub fn WIDGET_RADIUS() -> usize { crate::ui::px(crate::ui::base::WIDGET_RADIUS) } // UIMETRICS: was `const` 8

/// `metrics.well_radius` = `15` @ `0787ba9f` — corner radius for wells (recessed
/// regions, e.g. a scroll trough or a content sink), px. New in iteration 3.
#[allow(non_snake_case)] #[inline] pub fn WELL_RADIUS() -> usize { crate::ui::px(crate::ui::base::WELL_RADIUS) } // UIMETRICS: was `const` 15

/// `metrics.scrollbar_width` = `12` @ `0787ba9f` — scrollbar width, px.
#[allow(non_snake_case)] #[inline] pub fn SCROLLBAR_WIDTH() -> usize { crate::ui::px(crate::ui::base::SCROLLBAR_WIDTH) } // UIMETRICS: was `const` 12

/// `metrics.button_height` = `28` @ `0787ba9f` — button height, px.
#[allow(non_snake_case)] #[inline] pub fn BUTTON_HEIGHT() -> usize { crate::ui::px(crate::ui::base::BUTTON_HEIGHT) } // UIMETRICS: was `const` 28

/// `metrics.button_pad_x` = `18` @ `0787ba9f` — horizontal padding inside a button, px.
#[allow(non_snake_case)] #[inline] pub fn BUTTON_PAD_X() -> usize { crate::ui::px(crate::ui::base::BUTTON_PAD_X) } // UIMETRICS: was `const` 18

/// `metrics.gap` = `12` @ `0787ba9f` — standard gap between controls, px.
#[allow(non_snake_case)] #[inline] pub fn GAP() -> usize { crate::ui::px(crate::ui::base::GAP) } // UIMETRICS: was `const` 12

/// The title-bar control's extent, px — read as a DIAMETER, the controls being circles.
///
/// ⛔ **OVERRIDDEN, and no longer the kit's `metrics.control_box` = `12`.** Peter's ruling from
/// the bench, 2026-08-09, verbatim: *"window buttons are very small"*.
///
/// ### The arithmetic, so the number is derived rather than picked
///
/// macOS draws its traffic-light controls at about **12 points**. Peter's panel is a 2880x1800
/// 15" rMBP — a 2x Retina display — so a Mac renders those discs at about **24 device pixels**.
///
/// This metric is a **device-pixel** metric, and nothing magnifies it:
/// `wm::spawn_geometry` computes `w * scale + 2 * BORDER` and `h * scale + TITLE_H + 2 * BORDER`,
/// i.e. `wm::place_scale`'s integer upscale (and its `legibility_cap`) apply to the app's CONTENT
/// only — `BORDER`, `TITLE_HEIGHT` and this box are added unscaled, and `wm::paint_window` writes
/// the discs through `put_pixel` at panel coordinates. `UNAOS_FBW`/`UNAOS_FBH` change the panel
/// geometry the content scale is chosen against; they do not change a chrome metric either. So
/// `12` really was 12 device pixels on glass — **half** the physical size of the thing it is
/// imitating, which is the whole explanation for the verdict.
///
/// `24` is therefore the size that MATCHES the reference, not a size that was liked.
///
/// ### What moves with it, and what that cost
///
///  * `CONTROL_RADIUS` below: 6 -> 12.
///  * `wm::CTRL_RESERVE` = `3 * CONTROL_BOX + 4 * GAP`: 84 -> 120, and with it the strip width
///    `wm::controls` declines below (`2*FRAME + GAP + CTRL_RESERVE + TEXT_CELL` = 122 -> 158) and
///    the caption's left inset. Both are derived from this constant and moved by themselves; the
///    fixture surfaces that must clear the threshold are now sized from `wm::CLUSTER_MIN_SRC_W`
///    with a const-assert, so this metric can never again silently turn a close-control gate into
///    a SKIP.
///  * `wm::ctrl_glyph`'s three symbols are all expressed in terms of `d`, so they scale by
///    themselves — and they scale into BETTER proportions, not worse: the minimise bar is 2 px of
///    a 24-px disc (macOS-like) where it was 2 px of a 12-px disc (heavy).
///  * `video/knurl.rs`'s crosshatch becomes legible at all; see its legibility section.
///
/// ### ⚠ PROPOSED, NOT DECIDED — `TITLE_HEIGHT` is now tight
///
/// The disc is centred in the strip, so the clearance above and below it is
/// `(TITLE_HEIGHT - CONTROL_BOX) / 2` = `(34 - 24) / 2` = **5 px**. macOS gives its 24-px discs
/// about 16 px of clearance in a ~56-px strip. The chrome will read as CRAMPED. Raising
/// `TITLE_HEIGHT` to ~44 (clearance 10) or ~48 (clearance 12) is the fix, and it is a **taste-gate
/// question** — it changes every window's proportions and the caption's centring — so it is put on
/// the record here and NOT taken by this arc. The const-assert below still holds with room.
#[allow(non_snake_case)] #[inline] pub fn CONTROL_BOX() -> usize { crate::ui::px(crate::ui::base::CONTROL_BOX) } // UIMETRICS: was `const` 24 (the kit's 12 at 2x); the kit's 12 x the dpi scale now (seat, B372)

/// Radius of a circular title-bar control, px — `CONTROL_BOX / 2`, derived here rather
/// than lifted, because the json expresses the control's size only as `control_box`.
#[allow(non_snake_case)] #[inline] pub fn CONTROL_RADIUS() -> usize { CONTROL_BOX() / 2 } // UIMETRICS: was `const`

/// `metrics.text_px` = `15` @ `0787ba9f` — nominal text size, px.
#[allow(non_snake_case)] #[inline] pub fn TEXT_PX() -> usize { crate::ui::px(crate::ui::base::TEXT_PX) } // UIMETRICS: was `const` 15

/// `metrics.line_height_pct` = `165` @ `0787ba9f` — line height as a percent of `TEXT_PX`.
pub const LINE_HEIGHT_PCT: usize = 165;

// ---------------------------------------------------------------------------
// Compile-time sanity. Every assertion below is a `const` evaluation: it costs
// nothing at runtime and emits no code or data. These check the *shape* the json
// asserts (metrics positive, roles the kit declares distinct staying distinct),
// so a bad re-lift fails the build rather than the panel.
// ---------------------------------------------------------------------------

/// Metrics that must be strictly positive for any chrome to be drawable.
#[allow(dead_code)] pub(crate) fn uimetrics_sanity_positive(ck: &mut super::metrics::Sane) {
    ck.t(FRAME() > 0);
    ck.t(BEVEL() > 0);
    ck.t(TITLE_HEIGHT() > 0);
    ck.t(SCROLLBAR_WIDTH() > 0);
    ck.t(BUTTON_HEIGHT() > 0);
    ck.t(BUTTON_PAD_X() > 0);
    ck.t(GAP() > 0);
    ck.t(CONTROL_BOX() > 0);
    ck.t(TEXT_PX() > 0);
    const _: () = assert!(LINE_HEIGHT_PCT > 0);
    // The three radii (`corner_radius`, `widget_radius`, `well_radius`) may each
    // legitimately be 0 (a square head, a square widget, a square well), so they are
    // bounded below rather than required positive.
}

/// Relationships the json's own numbers imply, and that the chrome geometry relies on.
#[allow(dead_code)] pub(crate) fn uimetrics_sanity_relations(ck: &mut super::metrics::Sane) {
    // The bevel is drawn inside the frame.
    ck.t(BEVEL() < FRAME());
    // The rounded head must fit inside the title bar: 12 < 34.
    ck.t(CORNER_RADIUS() < TITLE_HEIGHT());
    // Title-bar controls must fit inside the title bar: 24 < 34 since the size ruling.
    ck.t(CONTROL_BOX() < TITLE_HEIGHT());
    // …and must leave a real clearance band, not merely fit. `(34 - 24)/2` = 5 px each side, which
    // is tight (see the note on `CONTROL_BOX`: raising `TITLE_HEIGHT` is a proposed taste-gate
    // question). One bevel of clearance is the floor below which the disc would touch the frame.
    ck.t(TITLE_HEIGHT() >= CONTROL_BOX() + 2 * BEVEL());
    // A circular control needs a non-degenerate radius, or it cannot be drawn round.
    ck.t(CONTROL_RADIUS() > 0);
    // Both of a widget's corners must fit within its own height: 2*8 <= 28.
    ck.t(2 * WIDGET_RADIUS() <= BUTTON_HEIGHT());
    // A well's corner is a chrome-scale radius, not a window-scale one.
    ck.t(WELL_RADIUS() < TITLE_HEIGHT());
    // A line of text is taller than the glyph box.
    const _: () = assert!(LINE_HEIGHT_PCT > 100);
}

/// Every colour is a packed `0x00RRGGBB`: the alpha byte is zero, because the json
/// palette carries no per-colour alpha. If a future kit adds alpha, this block is
/// the tripwire that says so.
const _: () = {
    const ROLES: [u32; 25] = [
        CTRL_CLOSE,
        CTRL_MIN,
        CTRL_ZOOM,
        CHROME_FACE,
        BEVEL_LIGHT,
        BEVEL_SHADOW,
        FRAME_LINE,
        TITLE_ACTIVE_TOP,
        TITLE_ACTIVE_BOTTOM,
        TITLE_INACTIVE_TOP,
        TITLE_INACTIVE_BOTTOM,
        TITLE_TEXT_ACTIVE,
        TITLE_TEXT_INACTIVE,
        BUTTON_FACE,
        BUTTON_FACE_PRESSED,
        BUTTON_TEXT,
        CONTENT_FILL,
        CONTENT_TEXT,
        SCROLL_TRACK,
        SCROLL_THUMB,
        ACCENT,
        CONTROL_CLOSE,
        CONTROL_MID,
        CONTROL_ZOOM,
        GLOSS_HIGHLIGHT,
    ];
    let mut i = 0;
    while i < ROLES.len() {
        assert!(ROLES[i] >> 24 == 0);
        i += 1;
    }
};

/// Roles the json gives distinct values must stay distinct after rounding — a
/// collapsed pair here means a bevel, a gradient, or a state change has gone
/// invisible.
const _: () = {
    assert!(BEVEL_LIGHT != CHROME_FACE);
    assert!(BEVEL_SHADOW != CHROME_FACE);
    assert!(BEVEL_LIGHT != BEVEL_SHADOW);
    assert!(FRAME_LINE != CHROME_FACE);
    assert!(TITLE_ACTIVE_TOP != TITLE_ACTIVE_BOTTOM);
    assert!(TITLE_INACTIVE_TOP != TITLE_INACTIVE_BOTTOM);
    assert!(TITLE_ACTIVE_TOP != TITLE_INACTIVE_TOP);
    assert!(TITLE_ACTIVE_BOTTOM != TITLE_INACTIVE_BOTTOM);
    assert!(TITLE_TEXT_ACTIVE != TITLE_TEXT_INACTIVE);
    assert!(BUTTON_FACE != BUTTON_FACE_PRESSED);
    assert!(BUTTON_TEXT != BUTTON_FACE);
    assert!(CONTENT_TEXT != CONTENT_FILL);
    assert!(SCROLL_THUMB != SCROLL_TRACK);
    assert!(ACCENT != CHROME_FACE);
    // The three circular controls are a ramp: each must be visibly its own step, and
    // each must stand off the title bar it sits on.
    assert!(CONTROL_CLOSE != CONTROL_MID);
    assert!(CONTROL_MID != CONTROL_ZOOM);
    assert!(CONTROL_CLOSE != CONTROL_ZOOM);
    assert!(CONTROL_CLOSE != TITLE_ACTIVE_TOP);
    assert!(CONTROL_MID != TITLE_ACTIVE_TOP);
    assert!(CONTROL_ZOOM != TITLE_ACTIVE_TOP);
    // The SEMANTIC set carries the same two obligations, and one more that the blue
    // ramp never had: three DIFFERENT HUES, not three steps of one. A pair that
    // collapsed here would put the destructive control and a harmless one in the
    // same colour, which is the exact failure this set exists to remove.
    assert!(CTRL_CLOSE != CTRL_MIN);
    assert!(CTRL_MIN != CTRL_ZOOM);
    assert!(CTRL_CLOSE != CTRL_ZOOM);
    assert!(CTRL_CLOSE != TITLE_ACTIVE_TOP);
    assert!(CTRL_MIN != TITLE_ACTIVE_TOP);
    assert!(CTRL_ZOOM != TITLE_ACTIVE_TOP);
    assert!(CTRL_CLOSE != TITLE_INACTIVE_TOP);
    assert!(CTRL_MIN != TITLE_INACTIVE_TOP);
    assert!(CTRL_ZOOM != TITLE_INACTIVE_TOP);
    // The gloss must be lighter than what it glosses, or it is not a highlight.
    assert!(GLOSS_HIGHLIGHT != CHROME_FACE);
    // The gloss fades downward. Iteration 3 takes the bottom stop to exactly 0, so
    // this is the assertion that the ramp still has a direction at all.
    assert!(GLOSS_TOP_ALPHA_Q16 > GLOSS_BOTTOM_ALPHA_Q16);
    //
    // NOTE: `BEVEL_LIGHT == GLOSS_HIGHLIGHT` at this commit (both pure white), so no
    // distinctness is asserted between them — the json gives them the same value.
};

/// The gloss scalars are unit values, so no Q16 form may exceed `1.0`, and the
/// falloff must be inside the `[0.01, 1.0]` band the host clamps it to.
///
/// `GLOSS_BOTTOM_ALPHA_Q16` is exactly `0` at this commit; a `<= Q16_ONE` bound on it
/// would be vacuous, and the direction assertion above is what actually constrains it.
const _: () = {
    assert!(GLOSS_TOP_ALPHA_Q16 <= Q16_ONE);
    assert!(GLOSS_FALLOFF_Q16 <= Q16_ONE);
    assert!(GLOSS_FALLOFF_Q16 >= Q16_ONE / 100);
};

// --- KEYMAP (R60, R61) — THE THEME OWNS ITS KEY BINDINGS ---------------------------------------
//
// Peter, 2026-09-22 (R60): *"we will be implementing a windows-esque them at some point so
// key-bindings shouldn't be hard coded."* A chord is a property of the THEME, exactly as the
// screenshot destination is (R60's other half, `video/prtscr.rs`'s `SCRSHOT-DESKTOP`). So the
// tables live HERE, beside the colours and the metrics, and `video/keymap.rs` holds only the
// mechanism that reads them.
//
// TAIL-APPENDED, never folded into the body above: this module is lexed into every image and a
// line inserted mid-file moves every `panic::Location` below it (LEDGER P7). Nothing follows this
// block, so nothing moves.

use super::keymap::{Action, Binding, Table, ALT, CMD, CTRL, SHIFT};

/// CRISPY's rows. PRECEDENCE ORDER — the resolver takes the first row whose usage edged and whose
/// roles are held, so the two-modifier chords are written above the one-modifier ones. Every row
/// is written in ROLE space (`CMD`), never in HID space; [`CRISPY_BINDINGS`] says what `CMD` is.
///
/// HID usages are Keyboard/Keypad page ids (HUT 1.12 §10) and match
/// `drivers::xhci::HID_SCANCODE_TO_ASCII`'s index: `a`=0x04, `c`=0x06, `q`=0x14, `v`=0x19,
/// `x`=0x1B, `3`=0x20, `4`=0x21, Escape = 0x29, Print Screen = 0x46, Home = 0x4A, End = 0x4D,
/// Right Arrow = 0x4F, Left Arrow = 0x50.
///
/// TERMSEL's rows (the terminal selection, `video/termsel.rs`, `clipboard.md` §7) are the Mac's
/// own: `⌘⇧←`/`⌘⇧→` to the line start/end — written ABOVE the bare `Shift+←`/`Shift+→`, which
/// would otherwise shadow them (the `no_shadow` check at the foot of this block refuses the other
/// order at compile time) — and `Shift+Home`/`Shift+End` as well, because the rMBP's internal
/// keyboard has no Home or End key but an external one on the same desktop does.
pub static CRISPY_ROWS: &[Binding] = &[ // SMALLFIX3 (B416): a slice — the count is derived (`.len()`), so an arc adding a row never edits this line
    // WINSNAP — window snapping. Written ABOVE every row on the arrow usages: `no_shadow` refuses the other order (bare `⌘←` is CursorLineStart).
    Binding { roles: CMD | ALT, usage: 0x50, action: Action::SnapLeft, token: "cmd-alt-left" }, Binding { roles: CTRL | CMD, usage: 0x14, action: Action::LockScreen, token: super::shortcuts::C_CTRL_CMD_Q }, Binding { roles: CMD, usage: 0x1A, action: Action::CloseWindow, token: super::shortcuts::C_CMD_W }, Binding { roles: CMD, usage: 0x0B, action: Action::HideApp, token: super::shortcuts::C_CMD_H }, Binding { roles: CMD, usage: 0x36, action: Action::OpenSettings, token: super::shortcuts::C_CMD_COMMA }, Binding { roles: CMD | ALT, usage: 0x29, action: Action::ForceQuit, token: super::shortcuts::C_CMD_ALT_ESC }, // APPMENU2 (B393) — the WM's system chords (Q W H comma, Opt-Esc; Ctrl-Cmd-Q locks). ABOVE the bare Esc row (`no_shadow`), Ctrl-Cmd-Q above Cmd-Q (whose row sits below Cmd-Shift-Q's LogOut).
    Binding { roles: CMD | ALT, usage: 0x4F, action: Action::SnapRight, token: "cmd-alt-right" },
    Binding { roles: CMD | ALT, usage: 0x52, action: Action::SnapZoom, token: "cmd-alt-up" },
    Binding { roles: CMD | ALT, usage: 0x51, action: Action::SnapRestore, token: "cmd-alt-down" },
    Binding { roles: CTRL | ALT, usage: 0x50, action: Action::SnapLeft, token: "ctrl-alt-left" },
    Binding { roles: CTRL | ALT, usage: 0x4F, action: Action::SnapRight, token: "ctrl-alt-right" },
    Binding { roles: CTRL | ALT, usage: 0x52, action: Action::SnapZoom, token: "ctrl-alt-up" },
    Binding { roles: CTRL | ALT, usage: 0x51, action: Action::SnapRestore, token: "ctrl-alt-down" },
    // WINRESIZE (R75) — Ctrl+arrow nudges the focused window, Ctrl+Shift+arrow resizes it. FIRST in the
    // table: the Shift row is above the Ctrl row on every usage (`no_shadow`), and both are above the
    // bare caret/select rows on the same usages, which they take over while Ctrl is held.
    Binding { roles: CTRL | SHIFT, usage: 0x50, action: Action::WinSizeLeft, token: "ctrl-shift-left" },
    Binding { roles: CTRL | SHIFT, usage: 0x4F, action: Action::WinSizeRight, token: "ctrl-shift-right" },
    Binding { roles: CTRL | SHIFT, usage: 0x52, action: Action::WinSizeUp, token: "ctrl-shift-up" },
    Binding { roles: CTRL | SHIFT, usage: 0x51, action: Action::WinSizeDown, token: "ctrl-shift-down" },
    Binding { roles: CTRL, usage: 0x50, action: Action::WinNudgeLeft, token: "ctrl-left" },
    Binding { roles: CTRL, usage: 0x4F, action: Action::WinNudgeRight, token: "ctrl-right" },
    Binding { roles: CTRL, usage: 0x52, action: Action::WinNudgeUp, token: "ctrl-up" },
    Binding { roles: CTRL, usage: 0x51, action: Action::WinNudgeDown, token: "ctrl-down" },
    // The two chords flight 11 proved on metal. Their tokens are the exact bytes the `[prtscr]`
    // witness has always carried, so a capture from before KEYMAP and one from after grep alike.
    Binding { roles: CMD | SHIFT, usage: 0x20, action: Action::Screenshot, token: super::shortcuts::C_CMD_SHIFT_3 },
    Binding { roles: CMD | SHIFT, usage: 0x21, action: Action::ScreenshotRegion, token: super::shortcuts::C_CMD_SHIFT_4 },
    Binding { roles: CMD | SHIFT, usage: 0x22, action: Action::ScreenshotWindow, token: "cmd-shift-5" }, // SHOTREGION M2
    // The SLOT (R60). LOGINFLOW may bind it; nothing else may, and nothing acts on it today.
    Binding { roles: CMD | SHIFT, usage: 0x14, action: Action::LogOut, token: "cmd-shift-q" }, Binding { roles: CMD, usage: 0x14, action: Action::QuitApp, token: super::shortcuts::C_CMD_Q }, // APPMENU2 — BELOW Cmd-Shift-Q (`no_shadow`)
    // SCROLLBACK (R75) — ABOVE every bare Home/End row (`no_shadow`). Cmd+Home/End and Ctrl+Home/End jump, Shift+PgUp/PgDn page.
    Binding { roles: SHIFT, usage: 0x4B, action: Action::ScrollPageUp, token: "shift-pgup" },
    Binding { roles: SHIFT, usage: 0x4E, action: Action::ScrollPageDown, token: "shift-pgdn" },
    Binding { roles: CMD, usage: 0x4A, action: Action::ScrollTop, token: "cmd-home" },
    Binding { roles: CMD, usage: 0x4D, action: Action::ScrollBottom, token: "cmd-end" },
    Binding { roles: CTRL, usage: 0x4A, action: Action::ScrollTop, token: "ctrl-home" },
    Binding { roles: CTRL, usage: 0x4D, action: Action::ScrollBottom, token: "ctrl-end" },
    // TERMSEL — to the line start / end, the Mac chords. ABOVE the bare Shift rows (precedence).
    Binding { roles: CMD | SHIFT, usage: 0x50, action: Action::SelectLineStart, token: "cmd-shift-left" },
    Binding { roles: CMD | SHIFT, usage: 0x4F, action: Action::SelectLineEnd, token: "cmd-shift-right" },
    // TERMSEL2 — the CARET to the line start / end: `⌘←`/`⌘→`, the Mac's chords (the rMBP has no
    // Home/End key). BELOW `⌘⇧←/→`, which they would otherwise shadow.
    Binding { roles: CMD, usage: 0x50, action: Action::CursorLineStart, token: "cmd-left" },
    Binding { roles: CMD, usage: 0x4F, action: Action::CursorLineEnd, token: "cmd-right" },
    // R61's four. Consumed by the terminal (`clipboard::terminal_action`, `clipboard.md` §3/§7):
    // Copy takes the selection or the whole line, Paste types, Cut removes the selection, Select
    // All selects the line.
    Binding { roles: CMD, usage: 0x06, action: Action::Copy, token: "cmd-c" },
    Binding { roles: CMD, usage: 0x19, action: Action::Paste, token: "cmd-v" },
    Binding { roles: CMD, usage: 0x1B, action: Action::Cut, token: "cmd-x" },
    Binding { roles: CMD, usage: 0x04, action: Action::SelectAll, token: "cmd-a" },
    // TERMSEL — one cell at a time, and the PC-keyboard line ends.
    Binding { roles: SHIFT, usage: 0x50, action: Action::SelectLeft, token: "shift-left" },
    Binding { roles: SHIFT, usage: 0x4F, action: Action::SelectRight, token: "shift-right" },
    Binding { roles: SHIFT, usage: 0x4A, action: Action::SelectLineStart, token: "shift-home" },
    Binding { roles: SHIFT, usage: 0x4D, action: Action::SelectLineEnd, token: "shift-end" },
    // TERMSEL2 — the CARET, bare keys. `roles: 0` matches with any modifier held, so each is written
    // BELOW every row on its usage (`no_shadow` refuses the other order). The arrow bytes are still
    // typed; Home and End type nothing.
    Binding { roles: 0, usage: 0x50, action: Action::CursorLeft, token: "left" },
    Binding { roles: 0, usage: 0x4F, action: Action::CursorRight, token: "right" },
    Binding { roles: 0, usage: 0x4A, action: Action::CursorLineStart, token: "home" },
    Binding { roles: 0, usage: 0x4D, action: Action::CursorLineEnd, token: "end" },
    // TERMSEL — Esc drops the selection. `roles: 0`, like Print Screen; the key is STILL TYPED
    // (0x1B reaches every key consumer as before), so a menu that dismisses on it is unaffected.
    Binding { roles: 0, usage: 0x29, action: Action::Deselect, token: "esc" },
    // WINCYCLE — Alt+Tab and the Mac's own ⌘Tab. Above the bare rows on usage 0x2B (none exist).
    Binding { roles: ALT, usage: 0x2B, action: Action::CycleWindow, token: super::shortcuts::C_ALT_TAB },
    Binding { roles: CMD, usage: 0x2B, action: Action::CycleWindow, token: super::shortcuts::C_CMD_TAB }, Binding { roles: CMD, usage: 0x10, action: Action::Minimize, token: super::shortcuts::C_CMD_M }, Binding { roles: CMD, usage: 0x35, action: Action::CycleApp, token: super::shortcuts::C_CMD_GRAVE }, Binding { roles: CMD, usage: 0x0E, action: Action::ClearView, token: super::shortcuts::C_CMD_K } /* LUMENBIN: ⌘K (HID 0x0E) */, Binding { roles: CMD, usage: 0x0C, action: Action::GetInfo, token: super::shortcuts::C_CMD_I } /* ATTRCOLUMNS: ⌘I (HID 0x0C) */, // WINDOWLIST — ⌘M minimise, ⌘` next window of the same app (CRISPY)
    // Print Screen. `roles: 0` — the key means capture whatever else is held, which is precisely
    // what the `0x46` edge did before it was a row. A theme that drops this row disarms the key.
    Binding { roles: 0, usage: 0x46, action: Action::Screenshot, token: super::shortcuts::C_PRINT_SCREEN },
    // BRIGHTKEYS: the Apple keyboard's F1/F2 are brightness down/up (HID 0x3A/0x3B). Consumed by
    // `video::brightkeys` at `pal::push_event`.
    Binding { roles: 0, usage: 0x3A, action: Action::BrightnessDown, token: "f1-brightness-down" },
    Binding { roles: 0, usage: 0x3B, action: Action::BrightnessUp, token: "f2-brightness-up" },
    // SCREENLOCK — ⌘L and Ctrl+Alt+L lock the session (usage 0x0F = L; no other row watches it).
    Binding { roles: CMD, usage: 0x0F, action: Action::LockScreen, token: super::shortcuts::C_CMD_L },
    Binding { roles: CTRL | ALT, usage: 0x0F, action: Action::LockScreen, token: super::shortcuts::C_CTRL_ALT_L },
    // SHORTCUTS — ⌘/ opens the help overlay (usage 0x38 = `/`; no other row watches it).
    Binding { roles: CMD, usage: 0x38, action: Action::ShowShortcuts, token: super::shortcuts::C_CMD_SLASH },
    // LAUNCHER (B417) — ⌘Space opens the Launcher (usage 0x2C = Space; no other row watches it).
    Binding { roles: CMD, usage: 0x2C, action: Action::Launcher, token: super::shortcuts::C_CMD_SPACE },
    // SHELLTASK2 (B474) — ⌘. interrupts the shell window's running command (usage 0x37 = `.`; no other row watches it).
    Binding { roles: CMD, usage: 0x37, action: Action::Interrupt, token: super::shortcuts::C_CMD_PERIOD },
];

/// **The desktop's live table.** `cmd_role` is `HID_MOD_GUI`, so every `CMD` above is the Command
/// key on the operator's Apple keyboard, left or right.
pub static CRISPY_BINDINGS: &Table = &Table {
    name: "crispy",
    cmd_role: crate::drivers::xhci::HID_MOD_GUI,
    rows: CRISPY_ROWS,
};

/// A PC-shaped table. IT EXISTS TO PROVE THE SEAM AND IS SELECTED BY NOTHING — compiled,
/// resolvable, and reached today only by `keymap::selftest`'s `pc_table_alt_c=` leg. A knob that
/// selects it is NOT this arc; when one is written it changes `keymap::active()` and nothing else.
///
/// The rows are the SAME rows in role space. What differs is one field — `cmd_role` — plus the
/// keyboard the operator is typing on: `Alt+C` is copy on a PC because R61 says the Command role
/// moves to Alt there, not because a second Copy row was written.
pub static PC_ROWS: &[Binding] = &[ // SMALLFIX3 (B416): a slice — the count is derived (`.len()`)
    // WINSNAP — window snapping. Written ABOVE every row on the arrow usages: `no_shadow` refuses the other order (bare `⌘←` is CursorLineStart).
    Binding { roles: CTRL | ALT, usage: 0x50, action: Action::SnapLeft, token: "ctrl-alt-left" },
    Binding { roles: CTRL | ALT, usage: 0x4F, action: Action::SnapRight, token: "ctrl-alt-right" },
    Binding { roles: CTRL | ALT, usage: 0x52, action: Action::SnapZoom, token: "ctrl-alt-up" },
    Binding { roles: CTRL | ALT, usage: 0x51, action: Action::SnapRestore, token: "ctrl-alt-down" },
    // WINRESIZE (R75) — Ctrl+arrow nudges the focused window, Ctrl+Shift+arrow resizes it. FIRST in the
    // table: the Shift row is above the Ctrl row on every usage (`no_shadow`), and both are above the
    // bare caret/select rows on the same usages, which they take over while Ctrl is held.
    Binding { roles: CTRL | SHIFT, usage: 0x50, action: Action::WinSizeLeft, token: "ctrl-shift-left" },
    Binding { roles: CTRL | SHIFT, usage: 0x4F, action: Action::WinSizeRight, token: "ctrl-shift-right" },
    Binding { roles: CTRL | SHIFT, usage: 0x52, action: Action::WinSizeUp, token: "ctrl-shift-up" },
    Binding { roles: CTRL | SHIFT, usage: 0x51, action: Action::WinSizeDown, token: "ctrl-shift-down" },
    Binding { roles: CTRL, usage: 0x50, action: Action::WinNudgeLeft, token: "ctrl-left" },
    Binding { roles: CTRL, usage: 0x4F, action: Action::WinNudgeRight, token: "ctrl-right" },
    Binding { roles: CTRL, usage: 0x52, action: Action::WinNudgeUp, token: "ctrl-up" },
    Binding { roles: CTRL, usage: 0x51, action: Action::WinNudgeDown, token: "ctrl-down" },
    // PrtSc. The SHIFTED row is written ABOVE the bare one, and it has to be: the bare row names
    // no roles, so it matches with Shift held too and would shadow the region chord entirely. That
    // is this table's one ordering hazard; the `const` block at the foot of this file checks it.
    Binding { roles: SHIFT, usage: 0x46, action: Action::ScreenshotRegion, token: "shift-prtsc" },
    Binding { roles: 0, usage: 0x46, action: Action::Screenshot, token: "prtsc" },
    Binding { roles: CMD, usage: 0x2B, action: Action::CycleWindow, token: super::shortcuts::C_ALT_TAB }, Binding { roles: CMD, usage: 0x10, action: Action::Minimize, token: super::shortcuts::C_CMD_M }, Binding { roles: CMD, usage: 0x35, action: Action::CycleApp, token: super::shortcuts::C_CMD_GRAVE }, Binding { roles: CMD, usage: 0x0E, action: Action::ClearView, token: super::shortcuts::C_CMD_K } /* LUMENBIN: ⌘K (HID 0x0E) */, Binding { roles: CMD, usage: 0x0C, action: Action::GetInfo, token: super::shortcuts::C_CMD_I } /* ATTRCOLUMNS: ⌘I (HID 0x0C) */, // WINCYCLE — Alt is the PC's cmd role; WINDOWLIST — Alt+M / Alt+` on the PC table
    Binding { roles: SHIFT, usage: 0x4B, action: Action::ScrollPageUp, token: "shift-pgup" }, // SCROLLBACK (R75) — above the bare Home/End rows
    Binding { roles: SHIFT, usage: 0x4E, action: Action::ScrollPageDown, token: "shift-pgdn" },
    Binding { roles: CTRL, usage: 0x4A, action: Action::ScrollTop, token: "ctrl-home" },
    Binding { roles: CTRL, usage: 0x4D, action: Action::ScrollBottom, token: "ctrl-end" },
    Binding { roles: CMD, usage: 0x06, action: Action::Copy, token: "alt-c" },
    Binding { roles: CMD, usage: 0x19, action: Action::Paste, token: "alt-v" },
    Binding { roles: CMD, usage: 0x1B, action: Action::Cut, token: "alt-x" },
    Binding { roles: CMD, usage: 0x04, action: Action::SelectAll, token: "alt-a" },
    // TERMSEL — a PC keyboard's selection chords. No `Alt+Shift+←` row: on a PC the line ends
    // are Home and End, which every PC keyboard has.
    Binding { roles: SHIFT, usage: 0x50, action: Action::SelectLeft, token: "shift-left" },
    Binding { roles: SHIFT, usage: 0x4F, action: Action::SelectRight, token: "shift-right" },
    Binding { roles: SHIFT, usage: 0x4A, action: Action::SelectLineStart, token: "shift-home" },
    Binding { roles: SHIFT, usage: 0x4D, action: Action::SelectLineEnd, token: "shift-end" },
    Binding { roles: 0, usage: 0x29, action: Action::Deselect, token: "esc" },
    // TERMSEL2 — the caret: arrows, Home, End. Below the Shift rows on the same usages.
    Binding { roles: 0, usage: 0x50, action: Action::CursorLeft, token: "left" },
    Binding { roles: 0, usage: 0x4F, action: Action::CursorRight, token: "right" },
    Binding { roles: 0, usage: 0x4A, action: Action::CursorLineStart, token: "home" },
    Binding { roles: 0, usage: 0x4D, action: Action::CursorLineEnd, token: "end" },
    Binding { roles: CTRL | ALT, usage: 0x0F, action: Action::LockScreen, token: super::shortcuts::C_CTRL_ALT_L }, // SCREENLOCK — Ctrl+Alt+L
];

/// The PC table. `cmd_role` is `HID_MOD_ALT` — R61's *"(alt-c on pc)"*, and the one field that
/// makes the whole difference.
pub static PC_BINDINGS: &Table = &Table {
    name: "pc",
    cmd_role: crate::drivers::xhci::HID_MOD_ALT,
    rows: PC_ROWS,
};

/// The tables' two structural contracts, checked at compile time beside the kit's own colour
/// assertions above.
const _: () = {
    // The two tables must DISAGREE about the physical Command key, or the role indirection is a
    // no-op wearing a name and `pc_table_alt_c=` in the fixture proves nothing.
    assert!(CRISPY_BINDINGS.cmd_role != PC_BINDINGS.cmd_role);
    // No row may shadow a later one (see `keymap::no_shadow`). This is what stops a bare-modifier
    // row being written above a chord that needs one — the failure that disarms a binding in
    // silence.
    assert!(super::keymap::no_shadow(CRISPY_ROWS));
    assert!(super::keymap::no_shadow(PC_ROWS));
};
// ══════════════ SCRSHOT-DESKTOP — WHERE A SCREEN CAPTURE LANDS (R60, 2026-09-22) ══════════════
//
// Appended at the file's TAIL, and that is not tidiness: KEYMAP is adding its bindings table to
// this same file in the same fold window, so a one-sided append is the only shape whose fold has
// no hand-merge in it. (The colour table above is a lifted `const` set; an insertion into its
// middle would also move every line the ledger rows cite positionally.)
//
// **WHY THE DESTINATION IS A PROPERTY OF THE THEME AND NOT OF `video/prtscr.rs`.**
// Peter, R60: *"is screenshot working? mac saves to desktop, correct? we should too, on this
// pioneer crispy theme anyway. we will be implementing a windows-esque them at some point so
// key-bindings shouldn't be hard coded."* The ruling makes one argument about two things — the
// CHORD and the DESTINATION are both facts about the desktop the user is looking at, not about the
// capture mechanism. A Mac saves to `~/Desktop`; a Windows-shaped theme saves to
// `%USERPROFILE%\Pictures\Screenshots`; the code that reads pixels and writes a PNG is the same on
// both and must not know which. So `prtscr` asks THIS table where the file goes, exactly as
// `wm.rs` asks it what colour a title bar is, and the second theme is one more `const` here.

/// SCRSHOT-DESKTOP (R60) — **the folder, inside the logged-in user's home, that a screen capture
/// lands in under the CRISPY theme.** The resolved destination is `/home/<name>/<CAPTURE_DIR>`.
///
/// `Desktop`, because that is where a Mac puts a screenshot and CRISPY is the Mac-shaped theme.
/// This supersedes the 2026-09-13 `Pictures/Screenshots` destination (PRTSCR-HOME), which was a
/// correct answer to a different question — *whose* folder — and never had a ruling on *which*.
///
/// ## Two constraints this value must satisfy, and they are not the same constraint
///
///  * **It must be a legal 8.3 SHORT NAME**, because `fs/fat.rs:118` writes 8.3 only and
///    `format_83` (`fs/fat.rs:325`) is the decider: base `1..=8` characters, no dot, every byte a
///    legal short-name byte. `Desktop` is SEVEN characters and clears that with a character to
///    spare — which is the whole reason this move needs no alias table entry where
///    `Screenshots` (eleven characters) needed `SCRSHOTS`. It is NOT const-asserted here on
///    purpose: the go-red for the capture-directory fixture is to point this constant at a name
///    `format_83` refuses, and a compile error is not a red RUN. `prtscr::dir_fixture` measures
///    it instead, on the wire, every boot.
///  * **It must be ONE component.** `prtscr::ensure_capture_dir` walks the home's components and
///    then appends this as a single leaf; a value with a `/` in it would be created as one
///    directory whose name contains a slash, which `format_83` also refuses — loudly, on the same
///    fixture line, rather than quietly.
///
/// What the operator sees on the medium is `DESKTOP`, uppercase, because `format_83` upcases every
/// byte it stores. That is not a presentation choice made anywhere in the kernel; it is what FAT
/// short names are. The lookup is case-insensitive AND matches a VFAT long name (`DirEntry::eq_name`,
/// `fs/fat.rs:190`), so a stick that already carries a `Desktop` folder made on a Mac is ADOPTED
/// with its own spelling and nothing new is written.
pub const CAPTURE_DIR: &str = "Desktop";

/// SCRSHOT-DESKTOP (R60) — the theme's own name, for the witness lines that report a themed
/// decision. One word, lower case, so `theme=crispy` reads the same in a log and in a spec pin.
///
/// It exists because a destination with no theme beside it on the wire is unreadable the day the
/// second theme lands: `dir=/home/una/Desktop` alone cannot be told from a Windows-shaped theme
/// that happens to agree. Every line that prints [`CAPTURE_DIR`]'s consequence prints this first.
pub const NAME: &str = "crispy";

// --- TERMCOLOR (R75) — the terminal's 16-colour palette --------------------------------------------
/// The console's default text colour (what an attribute-less cell paints).
pub use palette_core::TERM_FG;
/// The prompt's colour on the Moonstone console ground — the accent, lifted so it reads on dark.
pub use palette_core::TERM_ACCENT;
/// ANSI 0..7 (normal) then 8..15 (bright), tuned for the dark `0x2D2B55` ground.
pub use palette_core::ANSI16;
/// Named picks the shell's own output uses (`Console::style`).
pub const TERM_RED: u8 = 31;
pub const TERM_GREEN: u8 = 32;
pub const TERM_BLUE: u8 = 94;
pub const TERM_DIM: u8 = 90;

// ── APPEARANCE (rmbp-ledger B408, MACPARITY rows 21/22) — the system palette as TOKENS ─────────────────────────────
// The Mac's rule: every colour on the glass comes from the system's palette, never a literal, so Light / Dark, the
// accent and the highlight reach every control at once. The sets, the accents and the fixed-look rows below are
// `palette_core`'s (PALETTECORE B559 — the table the four ring-3 programs map their roles over too), re-exported
// by name. The kit roles above stay as the LIGHT set's provenance (and
// for the const contexts — selftest goldens, const asserts); every PAINTER reads the live value through the role
// functions below (`chrome_face()` … `accent()`), which index the active set. `unaos/scripts/appearance-check.py`
// certifies [`AUDIT_LITERALS_OUTSIDE`]: the colour literals under `video/` outside this file (before: 103).
// The preference is Principia's (`system.appearance.*`, `prefs_core::appearance`); `video::appearance` applies it.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering as AOrd};

/// The colour literals under `video/` outside this file and in the listed painters (`scripts/appearance.painters`) — certified by `unaos/scripts/appearance-check.py`.
pub const AUDIT_LITERALS_OUTSIDE: usize = 0;

/// The tokens, their names, the LIGHT and DARK sets and the eight accents (`prefs_core::appearance::ACCENTS` order)
/// — PALETTECORE (rmbp-ledger B559): the one table both rings link, re-exported so every consumer keeps its `theme::` path.
pub use palette_core::{Tok, ACCENTS, DARK, LIGHT, TOKENS, TOKEN_NAMES};

/// Colours that are the same in both sets (the console is dark in both; the pointer is un-themed, PA38; Pulse and
/// Facet's media ground are dark instruments) — consts, so the const contexts that read them keep compiling.
pub use palette_core::DESKTOP_BG;
pub use palette_core::PANEL_BG;
pub use palette_core::CONSOLE_BG;
pub use palette_core::CONSOLE_TEXT;
pub use palette_core::CONSOLE_BRIGHT;
pub use palette_core::PANIC_BG;
// BEZEL (B405) and PANICSCREEN (B406) are fixed-look overlays, the same in Light and Dark (APPEARANCE B408 moved
// their literals here at merge17 so `appearance-check.py` holds at literals_outside_theme=0).
pub use palette_core::BEZEL_FACE;
pub use palette_core::BEZEL_EDGE;
pub use palette_core::BEZEL_INK;
pub use palette_core::BEZEL_DIM;
pub use palette_core::PANIC_INK;
pub use palette_core::PANIC_INK_SMALL;
// SESSIONWEDGE (B525): the PUMPWEDGE band painted over the panel while the usb-pump is wedged (fixed-look, like PANIC).
pub use palette_core::WEDGE_BAND;
pub use palette_core::WEDGE_INK;
// PLAYER (B419): the player's dim caption and the scrubber knob's edge (moved here at merge17).
pub use palette_core::PLAYER_DIM_TEXT;
pub use palette_core::PLAYER_KNOB_EDGE;
pub use palette_core::POINTER_FILL;
pub use palette_core::POINTER_SHADOW;
pub use palette_core::PULSE_BG;
pub use palette_core::PULSE_METER;
pub use palette_core::PULSE_LABEL;
pub use palette_core::MEDIA_BG;
pub use palette_core::MEDIA_MATTE;
pub use palette_core::MEDIA_CAPTION;
pub use palette_core::SNAP_INK;
pub use palette_core::SHOT_BORDER;
/// An opaque black pixel (a decoded image's empty canvas).
pub use palette_core::OPAQUE_BLACK;

/// The packed-ARGB form of an `0x00RRGGBB` role (Facet's surfaces carry alpha).
pub use palette_core::opaque;

static DARK_ON: AtomicBool = AtomicBool::new(false);
static ACCENT_IX: AtomicU8 = AtomicU8::new(0);
static HL_IX: AtomicU8 = AtomicU8::new(0);
/// Bumped on every change of set, accent or highlight — `strip::seal` and Quarry's repaint pass read it.
static EPOCH: AtomicU32 = AtomicU32::new(0);

/// The live value of token `t`.
#[inline]
pub fn tok(t: Tok) -> u32 {
    if DARK_ON.load(AOrd::Relaxed) { DARK[t as usize] } else { LIGHT[t as usize] }
}

/// Is the Dark set live?
pub fn is_dark() -> bool {
    DARK_ON.load(AOrd::Relaxed)
}

/// The chosen accent's index / colour.
pub fn accent_index() -> usize {
    (ACCENT_IX.load(AOrd::Relaxed) as usize).min(ACCENTS.len() - 1)
}

/// The highlight's accent index.
pub fn highlight_index() -> usize {
    (HL_IX.load(AOrd::Relaxed) as usize).min(ACCENTS.len() - 1)
}

/// Change epoch (0 = never changed).
pub fn epoch() -> u32 {
    EPOCH.load(AOrd::Acquire)
}

/// Install a set / accent / highlight. Returns whether anything moved (the epoch is bumped only then).
pub fn set(dark: bool, accent: usize, highlight: usize) -> bool {
    let a = accent.min(ACCENTS.len() - 1) as u8;
    let h = highlight.min(ACCENTS.len() - 1) as u8;
    let moved = DARK_ON.swap(dark, AOrd::AcqRel) != dark;
    let moved = (ACCENT_IX.swap(a, AOrd::AcqRel) != a) | moved;
    let moved = (HL_IX.swap(h, AOrd::AcqRel) != h) | moved;
    if moved {
        EPOCH.fetch_add(1, AOrd::AcqRel);
    }
    moved
}

/// `a` and `b` mixed `num/den` of the way to `b`, per channel.
fn mix(a: u32, b: u32, num: u32, den: u32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s) & 0xFF, (b >> s) & 0xFF);
        ((x * (den - num) + y * num) / den) << s
    };
    ch(16) | ch(8) | ch(0)
}

#[inline] pub fn chrome_face() -> u32 { tok(Tok::WindowBg) }
#[inline] pub fn bevel_light() -> u32 { tok(Tok::BevelLight) }
#[inline] pub fn bevel_shadow() -> u32 { tok(Tok::BevelShadow) }
#[inline] pub fn frame_line() -> u32 { tok(Tok::FrameLine) }
#[inline] pub fn title_active_top() -> u32 { tok(Tok::TitleActiveTop) }
#[inline] pub fn title_active_bottom() -> u32 { tok(Tok::TitleActiveBottom) }
#[inline] pub fn title_inactive_top() -> u32 { tok(Tok::TitleInactiveTop) }
#[inline] pub fn title_inactive_bottom() -> u32 { tok(Tok::TitleInactiveBottom) }
#[inline] pub fn title_text_active() -> u32 { tok(Tok::TitleTextActive) }
#[inline] pub fn title_text_inactive() -> u32 { tok(Tok::TitleTextInactive) }
#[inline] pub fn button_face() -> u32 { tok(Tok::ButtonFace) }
#[inline] pub fn button_face_pressed() -> u32 { tok(Tok::ButtonFacePressed) }
#[inline] pub fn button_text() -> u32 { tok(Tok::ButtonText) }
#[inline] pub fn content_fill() -> u32 { tok(Tok::ContentFill) }
#[inline] pub fn content_text() -> u32 { tok(Tok::ContentText) }
#[inline] pub fn scroll_track() -> u32 { tok(Tok::ScrollTrack) }
#[inline] pub fn scroll_thumb() -> u32 { tok(Tok::ScrollThumb) }
#[inline] pub fn control_close() -> u32 { tok(Tok::ControlClose) }
#[inline] pub fn control_mid() -> u32 { tok(Tok::ControlMid) }
#[inline] pub fn control_zoom() -> u32 { tok(Tok::ControlZoom) }
#[inline] pub fn ctrl_close() -> u32 { tok(Tok::CtrlClose) }
#[inline] pub fn ctrl_min() -> u32 { tok(Tok::CtrlMin) }
#[inline] pub fn ctrl_zoom() -> u32 { tok(Tok::CtrlZoom) }
#[inline] pub fn gloss_highlight() -> u32 { tok(Tok::GlossHighlight) }
#[inline] pub fn field_bg() -> u32 { tok(Tok::FieldBg) }
#[inline] pub fn error_text() -> u32 { tok(Tok::ErrorText) }
#[inline] pub fn meter_high() -> u32 { tok(Tok::MeterHigh) }
#[inline] pub fn meter_mid() -> u32 { tok(Tok::MeterMid) }
#[inline] pub fn meter_low() -> u32 { tok(Tok::MeterLow) }
#[inline] pub fn syntax_code() -> u32 { tok(Tok::SyntaxCode) }
#[inline] pub fn syntax_num() -> u32 { tok(Tok::SyntaxNum) }
#[inline] pub fn syntax_lit() -> u32 { tok(Tok::SyntaxLit) }
/// ControlAccent — the default button, the selected segment, the focused control's ring, the slider knob, the
/// menu highlight: `system.appearance.accent`.
#[inline] pub fn accent() -> u32 { ACCENTS[accent_index()] }
/// Selection — text selection behind ink: `system.appearance.highlight`, mixed toward the content ground so the
/// ink stays legible in both sets (5/8 of the way to the ground in Light, 3/8 in Dark).
#[inline] pub fn selection() -> u32 {
    let (num, den) = if is_dark() { (3, 8) } else { (5, 8) };
    mix(ACCENTS[highlight_index()], content_fill(), num, den)
}

/// SELFTEST VECTORS — pixel values the painters' selftests feed in or expect back (golden rows of the materials,
/// synthetic window surfaces). Not chrome; here so the glass's sources name no literal (the audit's rule).
pub use palette_core::fixture;

const _: () = {
    let mut i = 0;
    while i < TOKENS {
        assert!(LIGHT[i] >> 24 == 0 && DARK[i] >> 24 == 0);
        i += 1;
    }
    assert!(LIGHT[Tok::ContentText as usize] != LIGHT[Tok::ContentFill as usize]);
    assert!(DARK[Tok::ContentText as usize] != DARK[Tok::ContentFill as usize]);
    assert!(DARK[Tok::WindowBg as usize] != LIGHT[Tok::WindowBg as usize]);
    assert!(Tok::SyntaxLit as usize + 1 == TOKENS);
    assert!(ACCENTS.len() == 8);
    // PALETTECORE (B559): the core spells the accent names (it has no deps); the kernel links both, so the two lists
    // are held equal here — a renamed accent in either fails the build, never the glass.
    const fn names_eq(a: &[&str], b: &[&str]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut i = 0;
        while i < a.len() {
            let (x, y) = (a[i].as_bytes(), b[i].as_bytes());
            if x.len() != y.len() {
                return false;
            }
            let mut j = 0;
            while j < x.len() {
                if x[j] != y[j] {
                    return false;
                }
                j += 1;
            }
            i += 1;
        }
        true
    }
    assert!(names_eq(&palette_core::ACCENT_NAMES, &prefs_core::appearance::ACCENTS));
};

// ── PAINTERSCOPE (rmbp-ledger B482) — the painters OUTSIDE video/ read their colours here ──────────────────────────
// `unaos/scripts/appearance.painters` lists every kernel file outside `video/` that puts a colour on the glass;
// `appearance-check.py` scans them with `video/` and counts toward [`AUDIT_LITERALS_OUTSIDE`]. Their roles live in
// `painter` (one look each: the console, the status bands, the boot splash, the Orin probes); the format-check
// primaries a panel bring-up paints live in `testcard`. Values are the literals they replaced, unchanged.
pub use palette_core::painter;

/// The pure primaries a panel bring-up paints to prove the pixel format (`display_tegra.rs`'s bars and quadrant
/// card): in the named order the format is right, red/blue swapped names it wrong. Not a theme — a test card.
pub use palette_core::testcard;

// ── CURSORSHAPE (rmbp-ledger B550) — the sprite table's inks, one (fill, shadow) pair per shape ────────────────────
// `video/cursor.rs`'s `SHAPES` table reads its colours HERE, indexed by `una_abi::CURSOR_*` (arrow, hand, text,
// crosshair, move, resize-ns, resize-ew, busy). No new literal: every pair is the two pointer colours above, so the
// pointer stays one look in Light and Dark (PA38's "un-themed" verdict is about the colours; this file is where a
// kit revision would change them, for every shape at once). The TEXT BEAM is the one inverted pair — dark ink with a
// light offset — because it stands over text: a white beam on a white page (Aether) is invisible and the offset
// keeps it readable on a dark editor (Una). Appended at the file tail: no line above moves.
/// The text beam's ink and its offset halo.
pub const POINTER_BEAM_FILL: u32 = POINTER_SHADOW;
pub const POINTER_BEAM_SHADOW: u32 = POINTER_FILL;
/// `(fill, shadow)` per `una_abi::CURSOR_*` index; the width is asserted against `CURSOR_SHAPES` in `video/cursor.rs`.
pub const POINTER_SHAPE_INK: [(u32, u32); 8] = [
    (POINTER_FILL, POINTER_SHADOW),           // arrow
    (POINTER_FILL, POINTER_SHADOW),           // hand
    (POINTER_BEAM_FILL, POINTER_BEAM_SHADOW), // text beam
    (POINTER_FILL, POINTER_SHADOW),           // crosshair
    (POINTER_FILL, POINTER_SHADOW),           // move
    (POINTER_FILL, POINTER_SHADOW),           // resize-ns
    (POINTER_FILL, POINTER_SHADOW),           // resize-ew
    (POINTER_FILL, POINTER_SHADOW),           // busy
];

// ── GLASSKIT (rmbp-ledger B628) — the drawing kit's roles (`video/draw.rs`), every one DERIVED from the set above ──
// No new colour: each role is a live token mixed, lifted or given an alpha, so Light/Dark and the accent move them
// with everything else. Alpha-bearing roles are straight `0xAARRGGBB` words (`draw::Ink::argb`). Appended at the file
// tail: no line above moves.
/// The hairline's opacity (of 255): an edge the eye reads as a line, not a border.
pub const HAIRLINE_ALPHA: u32 = 31;
/// A control's edge (a check box, a field, an unpressed button): one step firmer than a hairline.
pub const CONTROL_EDGE_ALPHA: u32 = 56;
/// The focus ring's opacity over the accent.
pub const FOCUS_RING_ALPHA: u32 = 120;
/// A soft (control-sized) shadow's peak opacity, Light / Dark.
pub const SHADOW_SOFT_ALPHA: (u32, u32) = (46, 110);
/// A window's shadow's peak opacity, Light / Dark.
pub const SHADOW_WINDOW_ALPHA: (u32, u32) = (72, 150);
/// How far a group box's fill is lifted above the window's face, per channel.
pub const GROUP_LIFT: u32 = 8;

/// `c` lifted `n` per channel toward white, saturating.
fn lift(c: u32, n: u32) -> u32 {
    let ch = |s: u32| (((c >> s) & 0xFF) + n).min(255) << s;
    ch(16) | ch(8) | ch(0)
}
/// The hairline: the content ink at [`HAIRLINE_ALPHA`].
#[inline] pub fn hairline() -> u32 { content_text() | (HAIRLINE_ALPHA << 24) }
/// A control's edge: the content ink at [`CONTROL_EDGE_ALPHA`].
#[inline] pub fn control_edge() -> u32 { content_text() | (CONTROL_EDGE_ALPHA << 24) }
/// The focus ring: the accent at [`FOCUS_RING_ALPHA`].
#[inline] pub fn focus_ring() -> u32 { accent() | (FOCUS_RING_ALPHA << 24) }
/// A knob's or a button's shadow (black, the set's soft peak).
#[inline] pub fn shadow_soft() -> u32 { (if is_dark() { SHADOW_SOFT_ALPHA.1 } else { SHADOW_SOFT_ALPHA.0 }) << 24 }
/// A window's drop shadow (black, the set's window peak).
#[inline] pub fn shadow_window() -> u32 { (if is_dark() { SHADOW_WINDOW_ALPHA.1 } else { SHADOW_WINDOW_ALPHA.0 }) << 24 }
/// An inset group box's fill: the window's face one step lighter.
#[inline] pub fn group_fill() -> u32 { lift(chrome_face(), GROUP_LIFT) }
/// An off switch's track, a slider's rail, a segmented control's well: the face a sixth of the way to the ink.
#[inline] pub fn control_track() -> u32 { mix(chrome_face(), content_text(), 1, 6) }
/// A knob, an unchecked box, a plain button's face: the field's white on Light, the light ink on Dark.
#[inline] pub fn knob_face() -> u32 { if is_dark() { content_text() } else { field_bg() } }
/// Ink drawn ON the accent (a check mark, a selected row's label, the default button's label).
#[inline] pub fn ink_on_accent() -> u32 { if is_dark() { content_text() } else { field_bg() } }
/// The secondary ink (a group's title, a caption): the content ink two fifths of the way to the face.
#[inline] pub fn ink_secondary() -> u32 { mix(content_text(), chrome_face(), 2, 5) }
/// SOFTY (B629): the Softy look's colours, re-exported by name from `palette_core::softy` (PALETTECORE B559: the
/// values live there); `video::softy::palette` re-exports them beside its dimensions.
pub use palette_core::softy;

// ── FONTSCRAFT (rmbp-ledger B640, R124): DESIGN-LANGUAGE §2.2's type ramp ─────────────────────────────────────────
// One row per role: (role, the Una step, the Softy step). Una's body is 13 pt (after the Mac), Softy's 14 pt (after
// Windows 11); the roles and weights are the same, only the base step differs. Line heights are multiples of 2 pt
// (of 4 pt from 20 pt up). Tracking is in basis points of the em (+0.5 % = 50). Sizes are pt: a device px count is
// `pt x video::dpi`'s scale. `video::text` draws a role with the UI face (Inter: `wght` = the row's weight, `opsz`
// = the size in pt) or, for `Mono`, JetBrains Mono. Never below 11 pt; semibold (600), not bold, below 20 pt.

/// A type role of DESIGN-LANGUAGE §2.2.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeRole {
    /// dock tooltips, badges, timestamps
    Caption,
    /// secondary row text, section headers (at 600)
    Footnote,
    /// every label, menu item, list row, field
    Body,
    /// window titles, the app name in the bar, the default button
    BodyStrong,
    /// toast title, dialog title (Una), group headers
    Headline,
    /// Settings pane title (compact), dialog title (Softy)
    Title3,
    /// Settings pane title
    Title2,
    /// About, onboarding
    Title1,
    /// login clock, hero numbers
    Display,
    /// console, code, Activity figures
    Mono,
}

/// One step of the ramp: size and line height in pt, `wght`, tracking in basis points of the em, mono face or not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TypeStep {
    pub size_pt: u8,
    pub line_pt: u8,
    pub weight: u16,
    pub tracking_bp: i16,
    pub mono: bool,
}

impl TypeStep {
    /// Tracking as an em fraction (what `font_core::ui::Style::tracking` takes).
    pub fn tracking_em(self) -> f32 {
        self.tracking_bp as f32 / 10_000.0
    }
}

const fn ts(size_pt: u8, line_pt: u8, weight: u16, tracking_bp: i16) -> TypeStep {
    TypeStep { size_pt, line_pt, weight, tracking_bp, mono: false }
}

/// The base step of each look: (Una's body, Softy's body), in pt.
pub const TYPE_BASE_PT: (u8, u8) = (13, 14);

/// DESIGN-LANGUAGE §2.2, row for row: (role, Una, Softy).
pub const TYPE_RAMP: [(TypeRole, TypeStep, TypeStep); 10] = [
    (TypeRole::Caption, ts(11, 14, 400, 50), ts(12, 16, 400, 50)),
    (TypeRole::Footnote, ts(12, 16, 400, 25), ts(12, 16, 400, 25)),
    (TypeRole::Body, ts(13, 18, 400, 0), ts(14, 20, 400, 0)),
    (TypeRole::BodyStrong, ts(13, 18, 600, 0), ts(14, 20, 600, 0)),
    (TypeRole::Headline, ts(15, 20, 600, -25), ts(16, 22, 600, -25)),
    (TypeRole::Title3, ts(17, 22, 600, -50), ts(18, 24, 600, -50)),
    (TypeRole::Title2, ts(20, 26, 600, -75), ts(20, 28, 600, -75)),
    (TypeRole::Title1, ts(26, 32, 700, -100), ts(28, 36, 700, -100)),
    (TypeRole::Display, ts(40, 48, 700, -150), ts(40, 52, 700, -150)),
    (TypeRole::Mono, TypeStep { mono: true, ..ts(12, 18, 400, 0) }, TypeStep { mono: true, ..ts(13, 20, 400, 0) }),
];

/// The step of `role` in the Una look (`softy == false`) or the Softy look.
pub fn type_role(role: TypeRole, softy: bool) -> TypeStep {
    let mut i = 0;
    while i < TYPE_RAMP.len() {
        if TYPE_RAMP[i].0 == role {
            return if softy { TYPE_RAMP[i].2 } else { TYPE_RAMP[i].1 };
        }
        i += 1;
    }
    TYPE_RAMP[2].1
}
