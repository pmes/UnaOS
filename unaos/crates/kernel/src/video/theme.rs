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
//! numbers come from. A later verdict change edits THIS FILE ONLY — every consumer
//! reads the names, never the literals.
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
//! integer scale of the 8-px bitmap cell toward it (`wm::TITLE_SCALE` = 2, so 16 px drawn for the
//! 15 px asked). [`LINE_HEIGHT_PCT`] remains the one metric with no consumer — window captions are
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
//!  * **`video/cursor.rs`** — the pointer sprite's `FILL` and `SHADOW` (lines 110/112).
//!
//! See `docs/dev/OS/08_VIDEO/engine.md` §CRISPYWIRE.
//!
//! A verdict change still edits THIS FILE ONLY: every consumer reads the names.
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
// the json triple at `us-crispy-modern` `0787ba9f`.
// ---------------------------------------------------------------------------

/// `palette.chrome_face` = `[0.925, 0.925, 0.933]` @ `0787ba9f` — the window body fill.
pub const CHROME_FACE: u32 = 0x00EC_ECEE;

/// `palette.bevel_light` = `[1.0, 1.0, 1.0]` @ `0787ba9f` — top/left bevel edge.
pub const BEVEL_LIGHT: u32 = 0x00FF_FFFF;

/// `palette.bevel_shadow` = `[0.667, 0.667, 0.686]` @ `0787ba9f` — bottom/right bevel edge.
pub const BEVEL_SHADOW: u32 = 0x00AA_AAAF;

/// `palette.frame_line` = `[0.706, 0.706, 0.725]` @ `0787ba9f` — the outer keyline of the frame.
pub const FRAME_LINE: u32 = 0x00B4_B4B9;

/// `palette.title_active_top` = `[0.933, 0.933, 0.945]` @ `0787ba9f` — focused title gradient, top stop.
pub const TITLE_ACTIVE_TOP: u32 = 0x00EE_EEF1;

/// `palette.title_active_bottom` = `[0.89, 0.89, 0.91]` @ `0787ba9f` — focused title gradient, bottom stop.
pub const TITLE_ACTIVE_BOTTOM: u32 = 0x00E3_E3E8;

/// `palette.title_inactive_top` = `[0.957, 0.957, 0.961]` @ `0787ba9f` — unfocused title gradient, top stop.
pub const TITLE_INACTIVE_TOP: u32 = 0x00F4_F4F5;

/// `palette.title_inactive_bottom` = `[0.937, 0.937, 0.945]` @ `0787ba9f` — unfocused title gradient, bottom stop.
pub const TITLE_INACTIVE_BOTTOM: u32 = 0x00EF_EFF1;

/// `palette.title_text_active` = `[0.145, 0.149, 0.161]` @ `0787ba9f` — focused title caption ink.
pub const TITLE_TEXT_ACTIVE: u32 = 0x0025_2629;

/// `palette.title_text_inactive` = `[0.478, 0.478, 0.494]` @ `0787ba9f` — unfocused title caption ink.
pub const TITLE_TEXT_INACTIVE: u32 = 0x007A_7A7E;

/// `palette.button_face` = `[0.973, 0.973, 0.98]` @ `0787ba9f` — resting button face.
pub const BUTTON_FACE: u32 = 0x00F8_F8FA;

/// `palette.button_face_pressed` = `[0.878, 0.878, 0.89]` @ `0787ba9f` — pressed button face.
pub const BUTTON_FACE_PRESSED: u32 = 0x00E0_E0E3;

/// `palette.button_text` = `[0.125, 0.129, 0.141]` @ `0787ba9f` — button label ink.
pub const BUTTON_TEXT: u32 = 0x0020_2124;

/// `palette.content_fill` = `[0.96, 0.949, 0.918]` @ `0787ba9f` — content-region base fill.
///
/// Unchanged across the CRISPY-PI-2 re-lift: iteration 3 kept the paper tone, and
/// `content_surface.Paper.base_rgb` still agrees with it by construction.
pub const CONTENT_FILL: u32 = 0x00F5_F2EA;

/// `palette.content_text` = `[0.129, 0.125, 0.118]` @ `0787ba9f` — content-region ink.
pub const CONTENT_TEXT: u32 = 0x0021_201E;

/// `palette.scroll_track` = `[0.949, 0.949, 0.957]` @ `0787ba9f` — scrollbar trough.
pub const SCROLL_TRACK: u32 = 0x00F2_F2F4;

/// `palette.scroll_thumb` = `[0.796, 0.796, 0.812]` @ `0787ba9f` — scrollbar thumb.
pub const SCROLL_THUMB: u32 = 0x00CB_CBCF;

/// `palette.accent` = `[0.29, 0.451, 0.667]` @ `0787ba9f` — selection / focus accent.
pub const ACCENT: u32 = 0x004A_73AA;

// ---------------------------------------------------------------------------
// Title-bar controls. Iteration 3 replaces the single square `CONTROL_BOX` role
// with three *circular* controls, each with its own fill: a left-to-right ramp of
// the accent hue from darkest (close) to lightest (zoom). The geometry is still
// one number — `CONTROL_BOX` below, now read as a circle's diameter.
// ---------------------------------------------------------------------------

/// `palette.control_close` = `[0.239, 0.373, 0.573]` @ `0787ba9f` — close control fill.
pub const CONTROL_CLOSE: u32 = 0x003D_5F92;

/// `palette.control_mid` = `[0.404, 0.549, 0.729]` @ `0787ba9f` — middle (minimise) control fill.
pub const CONTROL_MID: u32 = 0x0067_8CBA;

/// `palette.control_zoom` = `[0.573, 0.667, 0.788]` @ `0787ba9f` — zoom control fill.
pub const CONTROL_ZOOM: u32 = 0x0092_AAC9;

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
pub const CTRL_CLOSE: u32 = 0x00FF_5F57;

/// DERIVED (Peter's ruling, 2026-08-09, *"same color as mac"*) — MINIMISE control
/// fill: the macOS standard minimise hue, `#FEBC2E`, hue 43°. Supersedes this table's
/// earlier muted derivation `#C89C52`. Pending re-lift as `palette.ctrl_min`.
pub const CTRL_MIN: u32 = 0x00FE_BC2E;

/// DERIVED (Peter's ruling, 2026-08-09, *"same color as mac"*) — ZOOM control fill:
/// the macOS standard zoom hue, `#28C840`, hue 128°. Supersedes this table's earlier
/// muted derivation `#5E9468`. Pending re-lift as `palette.ctrl_zoom`.
pub const CTRL_ZOOM: u32 = 0x0028_C840;

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
pub const GLOSS_HIGHLIGHT: u32 = 0x00FF_FFFF;

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
pub const FRAME: usize = 5;

/// `metrics.bevel` = `1` @ `0787ba9f` — bevel thickness, px. Iteration 3 makes the
/// bevel a true hairline.
pub const BEVEL: usize = 1;

/// `metrics.title_height` = `34` @ `0787ba9f` — title bar height, px.
pub const TITLE_HEIGHT: usize = 34;

/// `metrics.corner_radius` = `12` @ `0787ba9f` — radius of the two *top* corners, px.
pub const CORNER_RADIUS: usize = 12;

/// `metrics.widget_radius` = `8` @ `0787ba9f` — corner radius for widgets (buttons and
/// other raised controls), px. New in iteration 3.
pub const WIDGET_RADIUS: usize = 8;

/// `metrics.well_radius` = `15` @ `0787ba9f` — corner radius for wells (recessed
/// regions, e.g. a scroll trough or a content sink), px. New in iteration 3.
pub const WELL_RADIUS: usize = 15;

/// `metrics.scrollbar_width` = `12` @ `0787ba9f` — scrollbar width, px.
pub const SCROLLBAR_WIDTH: usize = 12;

/// `metrics.button_height` = `28` @ `0787ba9f` — button height, px.
pub const BUTTON_HEIGHT: usize = 28;

/// `metrics.button_pad_x` = `18` @ `0787ba9f` — horizontal padding inside a button, px.
pub const BUTTON_PAD_X: usize = 18;

/// `metrics.gap` = `12` @ `0787ba9f` — standard gap between controls, px.
pub const GAP: usize = 12;

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
pub const CONTROL_BOX: usize = 24;

/// Radius of a circular title-bar control, px — `CONTROL_BOX / 2`, derived here rather
/// than lifted, because the json expresses the control's size only as `control_box`.
pub const CONTROL_RADIUS: usize = CONTROL_BOX / 2;

/// `metrics.text_px` = `15` @ `0787ba9f` — nominal text size, px.
pub const TEXT_PX: usize = 15;

/// `metrics.line_height_pct` = `165` @ `0787ba9f` — line height as a percent of `TEXT_PX`.
pub const LINE_HEIGHT_PCT: usize = 165;

// ---------------------------------------------------------------------------
// Compile-time sanity. Every assertion below is a `const` evaluation: it costs
// nothing at runtime and emits no code or data. These check the *shape* the json
// asserts (metrics positive, roles the kit declares distinct staying distinct),
// so a bad re-lift fails the build rather than the panel.
// ---------------------------------------------------------------------------

/// Metrics that must be strictly positive for any chrome to be drawable.
const _: () = {
    assert!(FRAME > 0);
    assert!(BEVEL > 0);
    assert!(TITLE_HEIGHT > 0);
    assert!(SCROLLBAR_WIDTH > 0);
    assert!(BUTTON_HEIGHT > 0);
    assert!(BUTTON_PAD_X > 0);
    assert!(GAP > 0);
    assert!(CONTROL_BOX > 0);
    assert!(TEXT_PX > 0);
    assert!(LINE_HEIGHT_PCT > 0);
    // The three radii (`corner_radius`, `widget_radius`, `well_radius`) may each
    // legitimately be 0 (a square head, a square widget, a square well), so they are
    // bounded below rather than required positive.
};

/// Relationships the json's own numbers imply, and that the chrome geometry relies on.
const _: () = {
    // The bevel is drawn inside the frame.
    assert!(BEVEL < FRAME);
    // The rounded head must fit inside the title bar: 12 < 34.
    assert!(CORNER_RADIUS < TITLE_HEIGHT);
    // Title-bar controls must fit inside the title bar: 24 < 34 since the size ruling.
    assert!(CONTROL_BOX < TITLE_HEIGHT);
    // …and must leave a real clearance band, not merely fit. `(34 - 24)/2` = 5 px each side, which
    // is tight (see the note on `CONTROL_BOX`: raising `TITLE_HEIGHT` is a proposed taste-gate
    // question). One bevel of clearance is the floor below which the disc would touch the frame.
    assert!(TITLE_HEIGHT >= CONTROL_BOX + 2 * BEVEL);
    // A circular control needs a non-degenerate radius, or it cannot be drawn round.
    assert!(CONTROL_RADIUS > 0);
    // Both of a widget's corners must fit within its own height: 2*8 <= 28.
    assert!(2 * WIDGET_RADIUS <= BUTTON_HEIGHT);
    // A well's corner is a chrome-scale radius, not a window-scale one.
    assert!(WELL_RADIUS < TITLE_HEIGHT);
    // A line of text is taller than the glyph box.
    assert!(LINE_HEIGHT_PCT > 100);
};

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
        assert!(ROLES[i] <= 0x00FF_FFFF);
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
