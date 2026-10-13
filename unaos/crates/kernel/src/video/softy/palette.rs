// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (SOFTY B629: the look's tokens — every Softy colour and dimension, named once)
//!
//! Every number Softy draws with lives here and nowhere else (SOFTY.md carries the same table for the seat's
//! DESIGN-LANGUAGE alignment). Colours are logical `0x00RRGGBB`; dimensions are LOGICAL px (96 ppi) and reach the
//! glass through [`px`] — the panel's UI scale (`ui::px`), so a 2x panel draws them twice as large.
//!
//! The tone: a light neutral surface, one dark ink, ONE accent. Alphas are straight `0..=255`.

/// A logical length at the panel's scale.
#[inline]
pub fn px(n: usize) -> usize {
    crate::ui::px(n)
}

// PALETTECORE (B559): every Softy COLOUR lives in `palette_core::softy` (re-exported as `theme::softy`); this file keeps
// the dimensions and re-exports the colours under their names, so Softy reads one module.
pub use crate::video::theme::softy::{SURFACE, SURFACE_IDLE, FLYOUT, FLYOUT_FOOT, TEXT, TEXT_2, TEXT_IDLE, ACCENT, ON_ACCENT, STROKE, EDGE_IDLE, HOVER, PRESS, CLOSE_HOT, CLOSE_INK, FIELD, FIELD_LINE, PIP, APP_HUES};

// ── colour ─────────────────────────────────────────────────────────────────────────────────────────────────────
/// The hairline's alpha (about 10 %).
pub const STROKE_A: u8 = 26;
pub const HOVER_A: u8 = 190;
pub const PRESS_A: u8 = 14;
/// A hovered window button (minimise, maximise): ink at a whisper.
pub const BTN_HOVER_A: u8 = 18;
/// What a window head's corner shows — the desktop (the compositor's own rule: corners are transparency to it).
pub const DESKTOP: u32 = crate::video::wm::DESKTOP_BG;


// ── the taskbar ────────────────────────────────────────────────────────────────────────────────────────────────
/// The band's height.
pub const BAR_H: usize = 48;
/// One app button.
pub const TILE_W: usize = 44;
pub const TILE_H: usize = 40;
pub const TILE_GAP: usize = 4;
pub const TILE_R: usize = 6;
/// The icon inside a tile.
pub const TILE_ICON: usize = 24;
/// The running pip: idle width, the focused app's width, its height, its gap above the band's bottom.
pub const PIP_W: usize = 6;
pub const PIP_W_ON: usize = 16;
pub const PIP_H: usize = 3;
pub const PIP_DY: usize = 4;
/// The most tiles the band lays out (pins plus unpinned running apps).
pub const TILES_MAX: usize = 16;
/// The tray: icon size, the group's inner pad, the clock's pad, the band's right margin.
pub const TRAY_ICON: usize = 16;
pub const TRAY_GAP: usize = 8;
pub const TRAY_PAD: usize = 10;
pub const EDGE_PAD: usize = 12;

// ── the Start menu ─────────────────────────────────────────────────────────────────────────────────────────────
pub const START_W: usize = 600;
pub const START_H: usize = 620;
/// Its float above the taskbar, its radius, its inner pad.
pub const FLY_GAP: usize = 12;
pub const FLY_R: usize = 8;
pub const START_PAD: usize = 32;
/// The search field.
pub const SEARCH_H: usize = 36;
/// The pinned grid.
pub const GRID_COLS: usize = 6;
pub const CELL_W: usize = 88;
pub const CELL_H: usize = 84;
pub const CELL_R: usize = 6;
pub const GRID_ICON: usize = 32;
/// The footer: height, avatar, the power button.
pub const FOOT_H: usize = 64;
pub const AVATAR: usize = 32;
pub const POWER_BTN: usize = 40;
/// The power menu's rows.
pub const POWER_ROW_H: usize = 36;
pub const POWER_W: usize = 180;

// ── the calendar flyout ────────────────────────────────────────────────────────────────────────────────────────
pub const CAL_W: usize = 336;
pub const CAL_PAD: usize = 20;
pub const CAL_CELL: usize = 40;

// ── window chrome ──────────────────────────────────────────────────────────────────────────────────────────────
/// One caption button (minimise, maximise, close): its width at 1x (the brief's 46).
pub const BTN_W: usize = 46;
/// The glyphs drawn in it: their box and stroke.
pub const GLYPH: usize = 10;
pub const GLYPH_T: usize = 1;
/// The head's corner radius.
pub const WIN_R: usize = 8;
/// The app icon at the head's left, and the gap before the caption.
pub const WIN_ICON: usize = 16;
pub const WIN_ICON_GAP: usize = 10;
