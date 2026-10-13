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

// PALETTECORE (B559): every Softy COLOUR literal lives in `palette_core::softy` (re-exported as `theme::softy`); this
// file keeps the dimensions and resolves the colours under their names, so Softy reads one module.
//
// SOFTY2 (B641): the colours are FUNCTIONS now, resolved per paint from the live appearance (`theme::is_dark`, the
// accent index): LIGHT is Softy's own table (`palette_core::softy`, unchanged) with the accent from the ACCENT TABLE
// (`theme::accent()` — `#0067C0` is gone); DARK is DESIGN-LANGUAGE §2.3's semantic table, read by its token NAMES
// through [`Sem`] / [`dark`] — the one seam DESIGNAPPLY (B639) repoints when its tokens land in `palette_core`.
pub use crate::video::theme::softy::{ON_ACCENT, CLOSE_INK, APP_HUES};
use crate::video::theme::softy as light;

/// DESIGN-LANGUAGE §2.3's semantic tokens Softy's dark look reads, by their table NAMES ([`Sem::name`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sem {
    SurfaceWindow,
    SurfaceSidebar,
    SurfaceContent,
    SurfaceRaised,
    SurfaceOverlay,
    SurfaceSunken,
    LineSubtle,
    LineStrong,
    LineControl,
    TextPrimary,
    TextSecondary,
    TextTertiary,
    TextDisabled,
    AccentFill,
    AccentHover,
    AccentPressed,
    AccentInk,
    StatusCritical,
    RimDark,
}

/// Every token [`dark`] answers, in the table's order (the fixture walks it).
pub const SEMS: [Sem; 19] = [
    Sem::SurfaceWindow, Sem::SurfaceSidebar, Sem::SurfaceContent, Sem::SurfaceRaised, Sem::SurfaceOverlay, Sem::SurfaceSunken,
    Sem::LineSubtle, Sem::LineStrong, Sem::LineControl, Sem::TextPrimary, Sem::TextSecondary, Sem::TextTertiary,
    Sem::TextDisabled, Sem::AccentFill, Sem::AccentHover, Sem::AccentPressed, Sem::AccentInk, Sem::StatusCritical, Sem::RimDark,
];

impl Sem {
    /// The token's name as DESIGN-LANGUAGE §2.3 spells it.
    pub const fn name(self) -> &'static str {
        match self {
            Sem::SurfaceWindow => "surface.window",
            Sem::SurfaceSidebar => "surface.sidebar",
            Sem::SurfaceContent => "surface.content",
            Sem::SurfaceRaised => "surface.raised",
            Sem::SurfaceOverlay => "surface.overlay",
            Sem::SurfaceSunken => "surface.sunken",
            Sem::LineSubtle => "line.subtle",
            Sem::LineStrong => "line.strong",
            Sem::LineControl => "line.control",
            Sem::TextPrimary => "text.primary",
            Sem::TextSecondary => "text.secondary",
            Sem::TextTertiary => "text.tertiary",
            Sem::TextDisabled => "text.disabled",
            Sem::AccentFill => "accent.fill",
            Sem::AccentHover => "accent.hover",
            Sem::AccentPressed => "accent.pressed",
            Sem::AccentInk => "accent.ink",
            Sem::StatusCritical => "status.critical",
            Sem::RimDark => "rim.dark",
        }
    }
}

/// **THE SEAM** — token `t`'s DARK value through the palette API that exists today: `palette_core`'s Dark set
/// (`theme::DARK[Tok::…]`, whose neutrals sit within a few steps of §2.3's: window `#1D1E21` ≈ ContentFill, raised
/// `#292A2D` ≈ FieldBg, overlay `#2F3033` ≈ TitleInactiveTop, line.subtle `#343539` ≈ TitleActiveBottom, line.strong
/// `#4B4D50` ≈ FrameLine, text.primary `#F0F2F6` ≈ ButtonText, text.disabled `#5C5E61` ≈ ButtonFacePressed) and the
/// accent table (`theme::accent()`, the chosen accent's fill; hover / pressed / ink derived the way §2.3 derives them,
/// toward black or white). When DESIGNAPPLY (B639) lands §2.3's tokens in `palette_core`, each arm becomes the token
/// of the same name and nothing else in Softy moves.
pub fn dark(t: Sem) -> u32 {
    use crate::video::theme::{Tok, DARK};
    let d = |k: Tok| DARK[k as usize];
    let acc = crate::video::theme::accent();
    match t {
        // Flight 35 (SOFTY dark=0/19): the seat's LOCAL WAVE 3 fold mapped the retired material tokens onto the
        // nearest semantic ones and made SurfaceWindow and SurfaceRaised the SAME token, so the dark elevations
        // no longer rose. DESIGNAPPLY's palette IS the §2.3 vocabulary Softy's Sem names: each reads its own.
        Sem::SurfaceWindow => d(Tok::SurfaceWindow),
        Sem::SurfaceSidebar => d(Tok::SurfaceSidebar),
        Sem::SurfaceContent => d(Tok::SurfaceContent),
        Sem::SurfaceSunken => d(Tok::SurfaceSunken),
        Sem::SurfaceRaised => d(Tok::SurfaceRaised),
        Sem::SurfaceOverlay => d(Tok::SurfaceOverlay),
        Sem::LineSubtle => d(Tok::LineSubtle),
        Sem::LineStrong => d(Tok::LineStrong),
        Sem::LineControl => d(Tok::LineControl),
        Sem::TextTertiary => d(Tok::TextTertiary),
        Sem::TextPrimary => d(Tok::TextPrimary),
        Sem::TextSecondary => d(Tok::TextSecondary),
        Sem::TextDisabled => d(Tok::TextDisabled),
        Sem::AccentFill => acc,
        Sem::AccentHover => palette_core::mix(acc, light::PRESS, 1, 12),
        Sem::AccentPressed => palette_core::mix(acc, light::PRESS, 1, 6),
        Sem::AccentInk => palette_core::mix(acc, ON_ACCENT, 2, 5),
        Sem::StatusCritical => light::CLOSE_HOT,
        Sem::RimDark => ON_ACCENT,
    }
}

/// Is the dark look live (`appearance.mode`, through the theme)?
#[inline]
pub fn is_dark() -> bool {
    crate::video::theme::is_dark()
}

#[inline]
fn pick(l: u32, t: Sem) -> u32 {
    if is_dark() { dark(t) } else { l }
}

// ── colour (SOFTY2: functions, light | dark) ───────────────────────────────────────────────────────────────────
// Uppercase like `theme::FRAME()`: each was a `const` of the same name before SOFTY2, and every painter calls it so.
#[allow(non_snake_case)] #[inline] pub fn SURFACE() -> u32 { pick(light::SURFACE, Sem::SurfaceWindow) }
#[allow(non_snake_case)] #[inline] pub fn SURFACE_IDLE() -> u32 { pick(light::SURFACE_IDLE, Sem::SurfaceSunken) }
#[allow(non_snake_case)] #[inline] pub fn FLYOUT() -> u32 { pick(light::FLYOUT, Sem::SurfaceOverlay) }
#[allow(non_snake_case)] #[inline] pub fn FLYOUT_FOOT() -> u32 { pick(light::FLYOUT_FOOT, Sem::SurfaceWindow) }
#[allow(non_snake_case)] #[inline] pub fn TEXT() -> u32 { pick(light::TEXT, Sem::TextPrimary) }
#[allow(non_snake_case)] #[inline] pub fn TEXT_2() -> u32 { pick(light::TEXT_2, Sem::TextSecondary) }
#[allow(non_snake_case)] #[inline] pub fn TEXT_IDLE() -> u32 { pick(light::TEXT_IDLE, Sem::TextTertiary) }
/// THE accent — the accent table's FILL for the chosen accent (`system.appearance.accent`), in both modes.
#[allow(non_snake_case)] #[inline] pub fn ACCENT() -> u32 { crate::video::theme::accent() }
/// The accent pressed (§2.3 `accent.pressed`).
#[allow(non_snake_case)] #[inline] pub fn ACCENT_PRESSED() -> u32 { dark(Sem::AccentPressed) }
/// The accent as an INK on a surface (a pip, the focused window's border): the fill in light, §2.3 `accent.ink` in dark.
#[allow(non_snake_case)] #[inline] pub fn ACCENT_INK() -> u32 { pick(ACCENT(), Sem::AccentInk) }
/// The hairline's colour: black in light; `rim.dark` (white) in dark.
#[allow(non_snake_case)] #[inline] pub fn STROKE() -> u32 { pick(light::STROKE, Sem::RimDark) }
#[allow(non_snake_case)] #[inline] pub fn EDGE_IDLE() -> u32 { pick(light::EDGE_IDLE, Sem::LineStrong) }
/// A hovered control's wash: white over the light surface; white at a whisper over the dark one.
#[allow(non_snake_case)] #[inline] pub fn HOVER() -> u32 { pick(light::HOVER, Sem::RimDark) }
/// A pressed control's wash: a touch of ink in light; a lighter whisper in dark.
#[allow(non_snake_case)] #[inline] pub fn PRESS() -> u32 { pick(light::PRESS, Sem::RimDark) }
#[allow(non_snake_case)] #[inline] pub fn CLOSE_HOT() -> u32 { pick(light::CLOSE_HOT, Sem::StatusCritical) }
/// The close button held down: the hot fill a step darker.
#[allow(non_snake_case)] #[inline] pub fn CLOSE_PRESSED() -> u32 { palette_core::mix(CLOSE_HOT(), light::PRESS, 1, 5) }
#[allow(non_snake_case)] #[inline] pub fn FIELD() -> u32 { pick(light::FIELD, Sem::SurfaceContent) }
#[allow(non_snake_case)] #[inline] pub fn FIELD_LINE() -> u32 { pick(light::FIELD_LINE, Sem::LineControl) }
#[allow(non_snake_case)] #[inline] pub fn PIP() -> u32 { pick(light::PIP, Sem::TextTertiary) }

/// The hairline's alpha (about 10 % in light; `rim.dark`'s 8 % in dark).
#[allow(non_snake_case)] #[inline] pub fn STROKE_A() -> u8 { if is_dark() { 20 } else { 26 } }
#[allow(non_snake_case)] #[inline] pub fn HOVER_A() -> u8 { if is_dark() { 18 } else { 190 } }
#[allow(non_snake_case)] #[inline] pub fn PRESS_A() -> u8 { if is_dark() { 10 } else { 14 } }
/// A hovered window button (minimise, maximise): ink at a whisper.
#[allow(non_snake_case)] #[inline] pub fn BTN_HOVER_A() -> u8 { if is_dark() { 22 } else { 18 } }
/// A pressed window button: twice the hover's wash.
#[allow(non_snake_case)] #[inline] pub fn BTN_PRESS_A() -> u8 { 2 * BTN_HOVER_A() }
/// What a window head's corner shows — the desktop (the compositor's own rule: corners are transparency to it).
pub const DESKTOP: u32 = crate::video::wm::DESKTOP_BG;

/// DESIGN-LANGUAGE §2.4 `elev.2` as two `0xAARRGGBB` shadow inks `(near, far)`: `0 0 2 pt` and `0 8 24 pt`, black at
/// 12 % / 18 % in light and 30 % / 45 % in dark. Black is `palette_core::softy::PRESS` (the table's own ink-black).
pub fn elev2() -> (u32, u32) {
    let (n, f) = if is_dark() { (77u32, 115u32) } else { (31, 46) };
    (light::PRESS | n << 24, light::PRESS | f << 24)
}


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

// ── SOFTY2 (B641): the quick-settings flyout (the tray's press) ────────────────────────────────────────────────
/// Its width, inner pad, one row's height.
pub const QS_W: usize = 360;
pub const QS_PAD: usize = 20;
pub const QS_ROW: usize = 52;
/// The volume slider: track height, knob diameter, the mute button's box.
pub const QS_TRACK_H: usize = 4;
pub const QS_KNOB: usize = 18;
pub const QS_BTN: usize = 36;
/// The status tile (Wi-Fi): its height and radius.
pub const QS_TILE_H: usize = 64;
pub const QS_TILE_R: usize = 6;
