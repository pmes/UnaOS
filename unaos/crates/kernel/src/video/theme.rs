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
//! `kits/crispy/theme.json` @ branch `us-crispy`, commit `08b42ede`. The host-side
//! reader is `libs/quartzite/src/theme.rs` at the same commit.
//!
//! **Shared-source law.** Both arches (aarch64 Pi 4 and x86_64) source chrome and
//! desktop constants from *this* table, which in turn mirrors *that* json. No
//! per-arch invented numbers, ever. If a value needs to change, it changes in the
//! kit json first and is re-lifted here.
//!
//! **Taste gate is OPEN.** These values are provisional-but-current: the visual
//! verdict has not been taken. A verdict change edits THIS FILE ONLY — every
//! consumer reads the names, never the literals.
//!
//! # Wiring status
//!
//! Nothing consumes this table yet. Lifting the data and wiring the compositor are
//! deliberately separate arcs; the wiring arc (`wm.rs`, `screen.rs`, fbcon) follows.
//! Until then this module is byte-inert: all `const`, no statics, no code.
//!
//! # Representation
//!
//! Colours are packed `0x00RRGGBB` — the json palette carries **no per-colour alpha**
//! (every role is a 3-element sRGB triple), so the top byte is zero rather than an
//! invented opaque `0xFF`. The gloss layer is the one place alpha appears, and it
//! appears there as *separate scalar fields*, lifted below as `u8` 0..=255.
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
//! that exact rule, in f32, from the json value quoted in its provenance comment, so
//! a kernel-drawn pixel and a quartzite-drawn pixel agree bit for bit. No float math
//! survives into the kernel: the rounding happened here, at authoring time.

// ---------------------------------------------------------------------------
// Palette — 18 roles, packed 0x00RRGGBB.
// ---------------------------------------------------------------------------

/// `palette.chrome_face` = `[0.839, 0.835, 0.827]` — the window body fill.
pub const CHROME_FACE: u32 = 0x00D6_D5D3;

/// `palette.bevel_light` = `[0.976, 0.973, 0.965]` — top/left bevel edge.
pub const BEVEL_LIGHT: u32 = 0x00F9_F8F6;

/// `palette.bevel_shadow` = `[0.518, 0.514, 0.506]` — bottom/right bevel edge.
pub const BEVEL_SHADOW: u32 = 0x0084_8381;

/// `palette.frame_line` = `[0.31, 0.306, 0.302]` — the outer keyline of the frame.
pub const FRAME_LINE: u32 = 0x004F_4E4D;

/// `palette.title_active_top` = `[0.925, 0.929, 0.937]` — focused title gradient, top stop.
pub const TITLE_ACTIVE_TOP: u32 = 0x00EC_EDEF;

/// `palette.title_active_bottom` = `[0.741, 0.749, 0.769]` — focused title gradient, bottom stop.
pub const TITLE_ACTIVE_BOTTOM: u32 = 0x00BD_BFC4;

/// `palette.title_inactive_top` = `[0.898, 0.894, 0.89]` — unfocused title gradient, top stop.
pub const TITLE_INACTIVE_TOP: u32 = 0x00E5_E4E3;

/// `palette.title_inactive_bottom` = `[0.831, 0.827, 0.824]` — unfocused title gradient, bottom stop.
pub const TITLE_INACTIVE_BOTTOM: u32 = 0x00D4_D3D2;

/// `palette.title_text_active` = `[0.129, 0.133, 0.145]` — focused title caption ink.
pub const TITLE_TEXT_ACTIVE: u32 = 0x0021_2225;

/// `palette.title_text_inactive` = `[0.451, 0.451, 0.455]` — unfocused title caption ink.
pub const TITLE_TEXT_INACTIVE: u32 = 0x0073_7374;

/// `palette.button_face` = `[0.898, 0.894, 0.886]` — resting button face.
pub const BUTTON_FACE: u32 = 0x00E5_E4E2;

/// `palette.button_face_pressed` = `[0.706, 0.702, 0.694]` — pressed button face.
pub const BUTTON_FACE_PRESSED: u32 = 0x00B4_B3B1;

/// `palette.button_text` = `[0.114, 0.114, 0.118]` — button label ink.
pub const BUTTON_TEXT: u32 = 0x001D_1D1E;

/// `palette.content_fill` = `[0.96, 0.949, 0.918]` — content-region base fill.
pub const CONTENT_FILL: u32 = 0x00F5_F2EA;

/// `palette.content_text` = `[0.102, 0.098, 0.094]` — content-region ink.
pub const CONTENT_TEXT: u32 = 0x001A_1918;

/// `palette.scroll_track` = `[0.784, 0.78, 0.773]` — scrollbar trough.
pub const SCROLL_TRACK: u32 = 0x00C8_C7C5;

/// `palette.scroll_thumb` = `[0.878, 0.875, 0.867]` — scrollbar thumb.
pub const SCROLL_THUMB: u32 = 0x00E0_DFDD;

/// `palette.accent` = `[0.278, 0.404, 0.596]` — selection / focus accent.
pub const ACCENT: u32 = 0x0047_6798;

// ---------------------------------------------------------------------------
// Gloss — `palette.gloss`. A white highlight applied with a two-stop alpha
// falloff. The alphas are the json's own scalars, lifted through the same
// `to_u8` rule so the compositor can work in integer alpha.
// ---------------------------------------------------------------------------

/// `palette.gloss.highlight` = `[1.0, 1.0, 1.0]` — the gloss colour.
pub const GLOSS_HIGHLIGHT: u32 = 0x00FF_FFFF;

/// `palette.gloss.top_alpha` = `0.34` — gloss opacity at the top edge (0..=255).
pub const GLOSS_TOP_ALPHA: u8 = 87;

/// `palette.gloss.falloff` = `0.55` — gloss falloff shape parameter, as 0..=255.
///
/// The host stores this as a unit scalar and clamps it to `[0.01, 1.0]` before use;
/// the fixed-point form here is the same number, ready for integer interpolation.
pub const GLOSS_FALLOFF: u8 = 140;

/// `palette.gloss.bottom_alpha` = `0.06` — gloss opacity at the bottom edge (0..=255).
pub const GLOSS_BOTTOM_ALPHA: u8 = 15;

// ---------------------------------------------------------------------------
// Metrics — `metrics.*`, all integral in the json, all pixels unless noted.
// ---------------------------------------------------------------------------

/// `metrics.frame` = `8` — frame thickness, px.
pub const FRAME: usize = 8;

/// `metrics.bevel` = `2` — bevel thickness, px.
pub const BEVEL: usize = 2;

/// `metrics.title_height` = `28` — title bar height, px.
pub const TITLE_HEIGHT: usize = 28;

/// `metrics.corner_radius` = `6` — radius of the two *top* corners, px.
pub const CORNER_RADIUS: usize = 6;

/// `metrics.scrollbar_width` = `16` — scrollbar width, px.
pub const SCROLLBAR_WIDTH: usize = 16;

/// `metrics.button_height` = `26` — button height, px.
pub const BUTTON_HEIGHT: usize = 26;

/// `metrics.button_pad_x` = `14` — horizontal padding inside a button, px.
pub const BUTTON_PAD_X: usize = 14;

/// `metrics.gap` = `10` — standard gap between controls, px.
pub const GAP: usize = 10;

/// `metrics.control_box` = `14` — side of a square title-bar control box, px.
pub const CONTROL_BOX: usize = 14;

/// `metrics.text_px` = `15` — nominal text size, px.
pub const TEXT_PX: usize = 15;

/// `metrics.line_height_pct` = `155` — line height as a percent of `TEXT_PX`.
pub const LINE_HEIGHT_PCT: usize = 155;

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
    // `corner_radius` may legitimately be 0 (square head), so it is only bounded.
};

/// Relationships the json's own numbers imply, and that the chrome geometry relies on.
const _: () = {
    // The bevel is drawn inside the frame.
    assert!(BEVEL < FRAME);
    // The rounded head must fit inside the title bar.
    assert!(CORNER_RADIUS < TITLE_HEIGHT);
    // Title-bar controls must fit inside the title bar.
    assert!(CONTROL_BOX < TITLE_HEIGHT);
    // A line of text is taller than the glyph box.
    assert!(LINE_HEIGHT_PCT > 100);
};

/// Every colour is a packed `0x00RRGGBB`: the alpha byte is zero, because the json
/// palette carries no per-colour alpha. If a future kit adds alpha, this block is
/// the tripwire that says so.
const _: () = {
    const ROLES: [u32; 19] = [
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
    // The gloss must be lighter than what it glosses, or it is not a highlight.
    assert!(GLOSS_HIGHLIGHT != CHROME_FACE);
    // The gloss fades downward.
    assert!(GLOSS_TOP_ALPHA > GLOSS_BOTTOM_ALPHA);
};
