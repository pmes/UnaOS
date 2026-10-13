// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! PALETTECORE (rmbp-ledger B559) — the ONE table of UnaOS colours, linked by both rings.
//!
//! CHARTER: Principia owns the appearance choice (`system.appearance.mode` / `.accent` / `.highlight`,
//! `prefs_core::appearance`); this crate owns only the values that choice indexes. Seam: `shared-core`.
//!
//! Before this crate the kernel's `video/theme.rs` held the palette and each of the four ring-3 programs
//! (`crates/user-{aether,una,midden,lumen}/src/theme.rs`) held a COPY of the rows it read (B552's owed). Now the
//! kernel re-exports every name below from `video/theme.rs` (consumers keep reading `theme::CHROME_FACE`,
//! `theme::LIGHT`, `theme::fixture::…`), the programs map their own roles over [`Look`] / [`Theme`], and this file
//! is the only place a colour literal lives (`unaos/scripts/appearance-check.py` scans the kernel's `theme.rs` and
//! the four programs' `theme.rs` for zero).
//!
//! The values are unchanged from `video/theme.rs` at 98d9f842, byte for byte; their provenance (the kit json
//! `kits/crispy/theme.json` @ `us-crispy-modern` `0787ba9f`, the pinned rounding rule, Peter's rulings on the
//! semantic controls) is the module header and the per-role docs in `crates/kernel/src/video/theme.rs`, which keep
//! naming them. Colours are packed `0x00RRGGBB` (the kit carries no per-colour alpha).

#![no_std]
#![forbid(unsafe_code)]

// ---------------------------------------------------------------------------------------------------------------
// The kit roles — `palette.*` @ `0787ba9f` (21 lifted, 3 DERIVED by ruling, the gloss colour).
// ---------------------------------------------------------------------------------------------------------------

/// `palette.chrome_face` = `[0.925, 0.925, 0.933]` — the window body fill.
pub const CHROME_FACE: u32 = 0x00EC_ECEE;
/// `palette.bevel_light` = `[1.0, 1.0, 1.0]` — top/left bevel edge.
pub const BEVEL_LIGHT: u32 = 0x00FF_FFFF;
/// `palette.bevel_shadow` = `[0.667, 0.667, 0.686]` — bottom/right bevel edge.
pub const BEVEL_SHADOW: u32 = 0x00AA_AAAF;
/// `palette.frame_line` = `[0.706, 0.706, 0.725]` — the outer keyline of the frame.
pub const FRAME_LINE: u32 = 0x00B4_B4B9;
/// `palette.title_active_top` = `[0.933, 0.933, 0.945]` — focused title gradient, top stop.
pub const TITLE_ACTIVE_TOP: u32 = 0x00EE_EEF1;
/// `palette.title_active_bottom` = `[0.89, 0.89, 0.91]` — focused title gradient, bottom stop.
pub const TITLE_ACTIVE_BOTTOM: u32 = 0x00E3_E3E8;
/// `palette.title_inactive_top` = `[0.957, 0.957, 0.961]` — unfocused title gradient, top stop.
pub const TITLE_INACTIVE_TOP: u32 = 0x00F4_F4F5;
/// `palette.title_inactive_bottom` = `[0.937, 0.937, 0.945]` — unfocused title gradient, bottom stop.
pub const TITLE_INACTIVE_BOTTOM: u32 = 0x00EF_EFF1;
/// `palette.title_text_active` = `[0.145, 0.149, 0.161]` — focused title caption ink.
pub const TITLE_TEXT_ACTIVE: u32 = 0x0025_2629;
/// `palette.title_text_inactive` = `[0.478, 0.478, 0.494]` — unfocused title caption ink.
pub const TITLE_TEXT_INACTIVE: u32 = 0x007A_7A7E;
/// `palette.button_face` = `[0.973, 0.973, 0.98]` — resting button face.
pub const BUTTON_FACE: u32 = 0x00F8_F8FA;
/// `palette.button_face_pressed` = `[0.878, 0.878, 0.89]` — pressed button face.
pub const BUTTON_FACE_PRESSED: u32 = 0x00E0_E0E3;
/// `palette.button_text` = `[0.125, 0.129, 0.141]` — button label ink.
pub const BUTTON_TEXT: u32 = 0x0020_2124;
/// `palette.content_fill` = `[0.96, 0.949, 0.918]` — content-region base fill (= `content_surface.Paper.base_rgb`).
pub const CONTENT_FILL: u32 = 0x00F5_F2EA;
/// `palette.content_text` = `[0.129, 0.125, 0.118]` — content-region ink.
pub const CONTENT_TEXT: u32 = 0x0021_201E;
/// `palette.scroll_track` = `[0.949, 0.949, 0.957]` — scrollbar trough.
pub const SCROLL_TRACK: u32 = 0x00F2_F2F4;
/// `palette.scroll_thumb` = `[0.796, 0.796, 0.812]` — scrollbar thumb.
pub const SCROLL_THUMB: u32 = 0x00CB_CBCF;
/// `palette.accent` = `[0.29, 0.451, 0.667]` — selection / focus accent ([`ACCENTS`]`[0]`).
pub const ACCENT: u32 = 0x004A_73AA;
/// `palette.control_close` = `[0.239, 0.373, 0.573]` — the kit's close control fill (the blue ramp, kept as record).
pub const CONTROL_CLOSE: u32 = 0x003D_5F92;
/// `palette.control_mid` = `[0.404, 0.549, 0.729]` — the kit's middle control fill.
pub const CONTROL_MID: u32 = 0x0067_8CBA;
/// `palette.control_zoom` = `[0.573, 0.667, 0.788]` — the kit's zoom control fill.
pub const CONTROL_ZOOM: u32 = 0x0092_AAC9;
/// DERIVED (Peter, 2026-08-09, *"same color as mac"*) — the macOS close hue `#FF5F57`; pending re-lift as `palette.ctrl_close`.
pub const CTRL_CLOSE: u32 = 0x00FF_5F57;
/// DERIVED (same ruling) — the macOS minimise hue `#FEBC2E`; pending re-lift as `palette.ctrl_min`.
pub const CTRL_MIN: u32 = 0x00FE_BC2E;
/// DERIVED (same ruling) — the macOS zoom hue `#28C840`; pending re-lift as `palette.ctrl_zoom`.
pub const CTRL_ZOOM: u32 = 0x0028_C840;
/// `palette.gloss.highlight` = `[1.0, 1.0, 1.0]` — the gloss colour (numerically = [`BEVEL_LIGHT`], a separate role).
pub const GLOSS_HIGHLIGHT: u32 = 0x00FF_FFFF;

// ---------------------------------------------------------------------------------------------------------------
// TERMCOLOR (R75) — the terminal's 16-colour palette.
// ---------------------------------------------------------------------------------------------------------------

/// The console's default text colour (what an attribute-less cell paints).
pub const TERM_FG: u32 = 0x00AA_AAAA;
/// The prompt's colour on the Moonstone console ground — the accent, lifted so it reads on dark.
pub const TERM_ACCENT: u32 = 0x007F_B2F0;
/// ANSI 0..7 (normal) then 8..15 (bright), tuned for the dark `DESKTOP_BG` ground.
pub const ANSI16: [u32; 16] = [
    0x0000_0000, 0x00CD_3131, 0x000D_BC79, 0x00E5_E510, 0x0024_72C8, 0x00BC_3FBC, 0x0011_A8CD, 0x00E5_E5E5,
    0x0066_6666, 0x00F1_4C4C, 0x0023_D18B, 0x00F5_F543, 0x003B_8EEA, 0x00D6_70D6, 0x0029_B8DB, 0x00FF_FFFF,
];

// ---------------------------------------------------------------------------------------------------------------
// APPEARANCE (rmbp-ledger B408) — the system palette as TOKENS: the LIGHT and DARK rows and the eight accents.
// ---------------------------------------------------------------------------------------------------------------

/// One palette token. Light = the kit (crispy) values above, unchanged; Dark = ours (neutral greys, the same accent).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tok {
    WindowBg, BevelLight, BevelShadow, FrameLine, TitleActiveTop, TitleActiveBottom, TitleInactiveTop, TitleInactiveBottom,
    TitleTextActive, TitleTextInactive, ButtonFace, ButtonFacePressed, ButtonText, ContentFill, ContentText, ScrollTrack,
    ScrollThumb, ControlClose, ControlMid, ControlZoom, CtrlClose, CtrlMin, CtrlZoom, GlossHighlight, FieldBg, ErrorText,
    MeterHigh, MeterMid, MeterLow, SyntaxCode, SyntaxNum, SyntaxLit,
}

/// Tokens in a set (the `tokens=` of the kernel's appearance witness).
pub const TOKENS: usize = 32;
/// The token names, [`Tok`] order — the names the appearance witness and Tabula's theme map spell.
pub const TOKEN_NAMES: [&str; TOKENS] = [
    "WindowBg", "BevelLight", "BevelShadow", "FrameLine", "TitleActiveTop", "TitleActiveBottom", "TitleInactiveTop", "TitleInactiveBottom",
    "TitleTextActive", "TitleTextInactive", "ButtonFace", "ButtonFacePressed", "ButtonText", "ContentFill", "ContentText", "ScrollTrack",
    "ScrollThumb", "ControlClose", "ControlMid", "ControlZoom", "CtrlClose", "CtrlMin", "CtrlZoom", "GlossHighlight", "FieldBg", "ErrorText",
    "MeterHigh", "MeterMid", "MeterLow", "SyntaxCode", "SyntaxNum", "SyntaxLit",
];

/// The LIGHT set: the kit's crispy roles as they are, plus the roles the audit lifted out of the painters.
pub const LIGHT: [u32; TOKENS] = [
    CHROME_FACE, BEVEL_LIGHT, BEVEL_SHADOW, FRAME_LINE, TITLE_ACTIVE_TOP, TITLE_ACTIVE_BOTTOM, TITLE_INACTIVE_TOP, TITLE_INACTIVE_BOTTOM,
    TITLE_TEXT_ACTIVE, TITLE_TEXT_INACTIVE, BUTTON_FACE, BUTTON_FACE_PRESSED, BUTTON_TEXT, CONTENT_FILL, CONTENT_TEXT, SCROLL_TRACK,
    SCROLL_THUMB, CONTROL_CLOSE, CONTROL_MID, CONTROL_ZOOM, CTRL_CLOSE, CTRL_MIN, CTRL_ZOOM, GLOSS_HIGHLIGHT,
    0x00FF_FFFF, // FieldBg — Quarry's rename / new-folder field
    0x00A0_2020, // ErrorText — Facet's failure line
    0x00C8_4B3C, 0x00D9_A22E, 0x0043_A05A, // MeterHigh / Mid / Low — Activity's load ink
    0x0020_8040, 0x00B0_5000, 0x0080_30A0, // SyntaxCode / Num / Lit — FileView's tints
];

/// The DARK set: ours — neutral greys, the window controls' semantic hues kept, inks lifted for a dark ground.
pub const DARK: [u32; TOKENS] = [
    0x002A_2A2D, 0x003A_3A3E, 0x0016_1618, 0x004A_4A50, 0x003C_3C41, 0x0034_3438, 0x0030_3033, 0x002B_2B2E,
    0x00E8_E8EC, 0x008E_8E94, 0x0048_484D, 0x005C_5C62, 0x00EE_EEF1, 0x001E_1E20, 0x00E6_E4DF, 0x0032_3236,
    0x0060_6066, CONTROL_CLOSE, CONTROL_MID, CONTROL_ZOOM, CTRL_CLOSE, CTRL_MIN, CTRL_ZOOM, GLOSS_HIGHLIGHT,
    0x002C_2C2F, 0x00E5_7373,
    0x00D9_604F, 0x00E0_AE45, 0x0052_B86A,
    0x006C_C88A, 0x00E3_9A55, 0x00C3_8BE0,
];

/// The eight accents (`prefs_core::appearance::ACCENTS` order), ours. Index 0 is the kit's own [`ACCENT`].
pub const ACCENTS: [u32; 8] = [ACCENT, 0x002E_8C8C, 0x005B_8C3A, 0x00C0_8A1E, 0x00C0_603A, 0x00B0_4A6A, 0x007A_5AB0, 0x006E_6E73];
/// Their names, spelled as `prefs_core::appearance::ACCENTS` stores them (this crate has no deps; the kernel, which
/// links both, asserts the two lists equal).
pub const ACCENT_NAMES: [&str; 8] = ["crispy", "teal", "moss", "amber", "clay", "rose", "violet", "slate"];

// ---------------------------------------------------------------------------------------------------------------
// The fixed-look rows — the same in Light and Dark (the console is dark in both; the pointer is un-themed, PA38;
// Pulse and Facet's media ground are dark instruments; BEZEL B405, PANICSCREEN B406, SESSIONWEDGE B525, PLAYER B419).
// ---------------------------------------------------------------------------------------------------------------

pub const DESKTOP_BG: u32 = 0x002D_2B55;
pub const PANEL_BG: u32 = 0x001E_1E1E;
pub const CONSOLE_BG: u32 = 0x0000_0000;
pub const CONSOLE_TEXT: u32 = 0x00C0_C0C0;
pub const CONSOLE_BRIGHT: u32 = 0x00FF_FFFF;
pub const PANIC_BG: u32 = 0x0030_0000;
pub const BEZEL_FACE: u32 = 0x0026_2629;
pub const BEZEL_EDGE: u32 = 0x003C_3C40;
pub const BEZEL_INK: u32 = 0x00EE_EEF0;
pub const BEZEL_DIM: u32 = 0x004A_4A50;
pub const PANIC_INK: u32 = 0x00EC_ECEE;
pub const PANIC_INK_SMALL: u32 = 0x00A0_A0A6;
pub const WEDGE_BAND: u32 = 0x0060_1010;
pub const WEDGE_INK: u32 = 0x00FF_F0E0;
pub const PLAYER_DIM_TEXT: u32 = 0x0070_6E6A;
pub const PLAYER_KNOB_EDGE: u32 = 0x00FF_FFFF;
pub const POINTER_FILL: u32 = 0x00FF_FFFF;
pub const POINTER_SHADOW: u32 = 0x0010_1014;
pub const PULSE_BG: u32 = 0x0010_0E16;
pub const PULSE_METER: u32 = 0x009B_59B6;
pub const PULSE_LABEL: u32 = 0x008A_8296;
pub const MEDIA_BG: u32 = 0x0020_2020;
pub const MEDIA_MATTE: u32 = 0x0010_1010;
pub const MEDIA_CAPTION: u32 = 0x00F0_F0F0;
pub const SNAP_INK: u32 = 0x004A_90D9;
pub const SHOT_BORDER: u32 = 0x00FF_FFFF;
/// An opaque black pixel (a decoded image's empty canvas).
pub const OPAQUE_BLACK: u32 = 0xFF00_0000;

/// The packed-ARGB form of an `0x00RRGGBB` role (Facet's surfaces and the ring-3 programs' blits carry alpha).
#[inline]
pub const fn opaque(c: u32) -> u32 {
    0xFF00_0000 | c
}

/// `a` and `b` mixed `num/den` of the way to `b`, per channel (`den` > 0, `num` <= `den`).
#[inline]
pub const fn mix(a: u32, b: u32, num: u32, den: u32) -> u32 {
    let mut out = 0;
    let mut s = 0;
    while s < 24 {
        let (x, y) = ((a >> s) & 0xFF, (b >> s) & 0xFF);
        out |= ((x * (den - num) + y * num) / den) << s;
        s += 8;
    }
    out
}

/// The [`ACCENTS`] index of an accent name (ASCII case-insensitive), `None` for an unknown word.
pub fn accent_by_name(w: &[u8]) -> Option<u8> {
    ACCENT_NAMES.iter().position(|n| n.as_bytes().eq_ignore_ascii_case(w)).map(|i| i as u8)
}

// ---------------------------------------------------------------------------------------------------------------
// The look and the role struct.
// ---------------------------------------------------------------------------------------------------------------

/// The appearance as Principia stores it, resolved: the set, the accent, the highlight (indices into [`ACCENTS`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Look {
    pub dark: bool,
    pub accent: u8,
    pub hl: u8,
}

impl Look {
    /// The store's defaults (`appearance.mode = light`, `accent = crispy`, `highlight = accent`).
    pub const DEFAULT: Look = Look { dark: false, accent: 0, hl: 0 };
    /// The live value of token `t` in this look's set.
    #[inline]
    pub const fn tok(self, t: Tok) -> u32 {
        if self.dark { DARK[t as usize] } else { LIGHT[t as usize] }
    }
    /// ControlAccent — `system.appearance.accent`.
    #[inline]
    pub const fn accent(self) -> u32 {
        ACCENTS[self.accent as usize % 8]
    }
    /// The highlight colour at full strength — `system.appearance.highlight`.
    #[inline]
    pub const fn highlight(self) -> u32 {
        ACCENTS[self.hl as usize % 8]
    }
    /// Selection — the highlight mixed toward the content ground so the ink stays legible (5/8 Light, 3/8 Dark).
    #[inline]
    pub const fn selection(self) -> u32 {
        let (num, den) = if self.dark { (3, 8) } else { (5, 8) };
        mix(self.highlight(), self.tok(Tok::ContentFill), num, den)
    }
    pub const fn mode_name(self) -> &'static str {
        if self.dark { "dark" } else { "light" }
    }
    pub const fn accent_name(self) -> &'static str {
        ACCENT_NAMES[self.accent as usize % 8]
    }
}

/// The role struct: every token of a look resolved once, plus the chosen colours. A painter maps its own roles over
/// it (`Theme::of(look).tok(Tok::FrameLine)`, `.accent`, `.selection`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    pub look: Look,
    /// [`LIGHT`] or [`DARK`], by the look's set.
    pub rows: [u32; TOKENS],
    pub accent: u32,
    pub highlight: u32,
    pub selection: u32,
}

impl Theme {
    pub const fn of(look: Look) -> Theme {
        Theme {
            look,
            rows: if look.dark { DARK } else { LIGHT },
            accent: look.accent(),
            highlight: look.highlight(),
            selection: look.selection(),
        }
    }
    #[inline]
    pub const fn tok(&self, t: Tok) -> u32 {
        self.rows[t as usize]
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The kernel's named groups (moved verbatim from `video/theme.rs`; the kernel re-exports each module by name).
// ---------------------------------------------------------------------------------------------------------------

/// SELFTEST VECTORS — pixel values the painters' selftests feed in or expect back (golden rows of the materials,
/// synthetic window surfaces). Not chrome; here so the glass's sources name no literal (the audit's rule).
pub mod fixture {
    pub const PROBE_RGB: u32 = 0x0012_3456;
    pub const BLACK: u32 = 0x0000_0000;
    pub const WHITE: u32 = 0x00FF_FFFF;
    pub const INK: u32 = 0x0020_2020;
    pub const MAGENTA: u32 = 0x00FF_00FF;
    pub const OPAQUE_GREEN: u32 = 0xFF00_FF00;
    pub const RGB_RED: u32 = 0x00FF_0000;
    pub const RGB_GREEN: u32 = 0x0000_FF00;
    pub const RGB_BLUE: u32 = 0x0000_00FF;
    pub const RAMP_BASE: u32 = 0x0000_FF80;
    pub const BLUE: u32 = 0x0020_40FF;
    pub const GREEN: u32 = 0x0040_FF20;
    pub const RED: u32 = 0x00FF_4020;
    pub const YELLOW: u32 = 0x00FF_FF20;
    pub const CYAN: u32 = 0x0020_FFFF;
    pub const PINK: u32 = 0x00FF_20FF;
    pub const SKY: u32 = 0x0030_90F0;
    pub const LEAF: u32 = 0x0040_A060;
    pub const DUSK: u32 = 0x0033_3355;
    pub const STEEL: u32 = 0x0030_70A0;
    pub const ROSE: u32 = 0x00FF_2020;
    pub const LIME: u32 = 0x0020_FF20;
    pub const JADE: u32 = 0x0020_C080;
    pub const TEST_COLOR: u32 = 0x0011_2233;
    pub const SENTINEL: u32 = 0x00AB_CDEF;
    /// The kit's paper `base_rgb` under the pinned rounding — `paper.rs`'s tripwire against `CONTENT_FILL`.
    pub const PAPER_BASE: u32 = 0x00F5_F2EA;
    /// `ceramic.rs` leg 4: `CHROME_FACE` under the material, rows 0..8.
    pub const CERAMIC_REF8: [u32; 8] = [
        0x00EC_ECEE, 0x00EB_EBED, 0x00EC_ECEE, 0x00EB_EBED, //
        0x00EC_ECEE, 0x00EA_EAEC, 0x00EA_EAEC, 0x00EA_EAEC,
    ];
    /// `knurl.rs` leg 4: the three control roles at node / apex / groove / cancel.
    pub const KNURL_REF: [[u32; 4]; 3] = [
        [0x00FF_5F57, 0x00FF_6058, 0x00F9_5D55, 0x00FF_5F57],
        [0x00FE_BC2E, 0x00FF_BF2E, 0x00F8_B82D, 0x00FE_BC2E],
        [0x0028_C840, 0x0028_CC41, 0x0027_C33E, 0x0028_C840],
    ];
    /// `paper.rs` leg 2: the reference corner.
    pub const PAPER_REF4: [u32; 16] = [
        0x00F7_F4EC, 0x00F7_F4EC, 0x00F7_F4EC, 0x00F7_F4EC, //
        0x00F7_F4EC, 0x00F7_F4EC, 0x00F7_F4EC, 0x00F7_F4EC, //
        0x00F3_F0E9, 0x00F4_F1E9, 0x00F4_F1E9, 0x00F4_F1E9, //
        0x00F3_F0E9, 0x00F4_F1E9, 0x00F4_F1E9, 0x00F4_F1E9,
    ];
}

/// PAINTERSCOPE (rmbp-ledger B482) — the kernel painters OUTSIDE `video/` (`unaos/scripts/appearance.painters`):
/// one look each (the console, the status bands, the boot splash, the Orin probes). Values are the literals they
/// replaced, unchanged.
pub mod painter {
    /// `console.rs` — the scrollback page band and its thumb.
    pub const CONSOLE_BAND: u32 = 0x003A_3868;
    /// `ui_status.rs` — a core meter's unfilled segment, its idle breath, its parked dash.
    pub const METER_DIM: u32 = 0x002A_2432;
    pub const METER_BREATH: u32 = 0x005F_4E86;
    pub const METER_PARKED: u32 = 0x003A_3550;
    /// `ui_status.rs` — the status strip (ground, aqua text) and the pulse instrument panel under it.
    pub const STATUS_STRIP_BG: u32 = 0x001B_1A3A;
    pub const STATUS_STRIP_FG: u32 = 0x007B_D0E0;
    pub const PULSE_PANEL_BG: u32 = 0x000E_0D22;
    pub const PULSE_PANEL_FG: u32 = 0x009F_B4C8;
    /// `ui_status.rs` — the instrument LED ramp, green through amber to red.
    pub const LED_GREEN: u32 = 0x002E_CC71;
    pub const LED_AMBER: u32 = 0x00F1_C40F;
    pub const LED_RED: u32 = 0x00E7_4C3C;
    /// `splash.rs` — the boot splash: backdrop, facet edge, inner facet, the white beam, the edge glint.
    pub const SPLASH_BG: u32 = 0x0006_0608;
    pub const SPLASH_EDGE: u32 = 0x004A_4658;
    pub const SPLASH_FACET: u32 = 0x002C_2936;
    pub const SPLASH_BEAM: u32 = 0x00F2_F2EE;
    pub const SPLASH_GLINT: u32 = 0x00FF_FFFF;
    /// `splash.rs` — the refracted spectrum, red to violet (one per ray sample).
    pub const SPLASH_SPECTRUM: [u32; 9] = [
        0x00E8_1414, 0x00F0_5810, 0x00F8_9008, 0x00F0_D010, 0x0060_D818, //
        0x0018_C860, 0x0018_B8C8, 0x002E_58E8, 0x0090_28D8,
    ];
    /// `splash.rs` — the takeover caption band and its ink.
    pub const SPLASH_CAPTION_BG: u32 = 0x0010_1418;
    pub const SPLASH_CAPTION_INK: u32 = 0x00E8_E8F0;
    /// `display_tegra.rs` ORIN-RASTGLASS — `rast_demo`'s `RW_CLEAR` (0x10, 0x10, 0x18) as the packed word.
    pub const RAST_PAPER: u32 = 0x0010_1018;
}

/// The pure primaries a panel bring-up paints to prove the pixel format (`display_tegra.rs`'s bars and quadrant
/// card): in the named order the format is right, red/blue swapped names it wrong. Not a theme — a test card.
pub mod testcard {
    pub const BLACK: u32 = 0x0000_0000;
    pub const BLUE: u32 = 0x0000_00FF;
    pub const GREEN: u32 = 0x0000_FF00;
    pub const CYAN: u32 = 0x0000_FFFF;
    pub const RED: u32 = 0x00FF_0000;
    pub const MAGENTA: u32 = 0x00FF_00FF;
    pub const YELLOW: u32 = 0x00FF_FF00;
    pub const WHITE: u32 = 0x00FF_FFFF;
}

/// AETHER.ELF — the page's own initial colours: CSS's initial `color` (black), the canvas when neither the root nor
/// the body sets a background (white, CSS2 §14.2) and the HTML UA sheet's link blue. They are the PAGE'S, the same
/// in Light and Dark (content, not chrome — the idea of the fixed-look rows above).
pub mod page {
    pub const PAGE_INK: u32 = 0x0000_0000;
    pub const PAGE_BG: u32 = 0x00FF_FFFF;
    pub const PAGE_LINK: u32 = 0x0000_00EE;
}

// ---------------------------------------------------------------------------------------------------------------
// The table's shape, checked at compile time (the kernel's `video/theme.rs` keeps the kit-role distinctness block).
// ---------------------------------------------------------------------------------------------------------------

const _: () = {
    let mut i = 0;
    while i < TOKENS {
        assert!(LIGHT[i] <= 0x00FF_FFFF && DARK[i] <= 0x00FF_FFFF);
        i += 1;
    }
    assert!(Tok::SyntaxLit as usize + 1 == TOKENS);
    assert!(ACCENTS.len() == ACCENT_NAMES.len());
    // The role struct agrees with the look it was built from (the two paths the rings take to one colour).
    let t = Theme::of(Look { dark: true, accent: 3, hl: 5 });
    assert!(t.tok(Tok::FrameLine) == DARK[Tok::FrameLine as usize]);
    assert!(t.accent == ACCENTS[3] && t.highlight == ACCENTS[5]);
    assert!(t.selection == mix(ACCENTS[5], DARK[Tok::ContentFill as usize], 3, 8));
};

/// SOFTY (B629): the Softy look's colours — every one Softy draws with, named once here (PALETTECORE B559: a colour
/// literal lives in this table and nowhere else); `video::theme::softy` and `video::softy::palette` re-export them.
pub mod softy {
    /// The taskbar's and the focused window head's surface (a soft, slightly cool light grey).
    pub const SURFACE: u32 = 0xF3F3F3;
    /// An unfocused window head: one step lighter, so focus reads at a glance.
    pub const SURFACE_IDLE: u32 = 0xFAFAFA;
    /// The Start menu's and the calendar's body.
    pub const FLYOUT: u32 = 0xF9F9F9;
    /// The Start menu's footer band (the user and the power button).
    pub const FLYOUT_FOOT: u32 = 0xEFEFEF;
    /// Primary ink.
    pub const TEXT: u32 = 0x202020;
    /// Secondary ink (placeholders, the date line, section labels' detail).
    pub const TEXT_2: u32 = 0x5C5C5C;
    /// An unfocused window's caption.
    pub const TEXT_IDLE: u32 = 0x8C8C8C;
    /// THE accent (the brief's value; SOFTY.md notes it is the seat's to align with DESIGN-LANGUAGE).
    pub const ACCENT: u32 = 0x0067C0;
    /// Ink on the accent.
    pub const ON_ACCENT: u32 = 0xFFFFFF;
    /// The hairline every surface edge carries (the flyouts' outline, the taskbar's top edge, dividers).
    pub const STROKE: u32 = 0x000000;
    /// An unfocused window's 1 px border.
    pub const EDGE_IDLE: u32 = 0xC8C8C8;
    /// A hovered control: white over the surface …
    pub const HOVER: u32 = 0xFFFFFF;
    /// … and pressed: a touch of ink.
    pub const PRESS: u32 = 0x000000;
    /// The close button under the pointer, and its glyph.
    pub const CLOSE_HOT: u32 = 0xC42B1C;
    pub const CLOSE_INK: u32 = 0xFFFFFF;
    /// A text field's face, its outline, and the darker underline that marks it as editable.
    pub const FIELD: u32 = 0xFFFFFF;
    pub const FIELD_LINE: u32 = 0x8A8A8A;
    /// The running-app pip under a taskbar tile (idle; the focused app's pip is the accent).
    pub const PIP: u32 = 0x8C8C8C;
    /// An app's monogram tile colour when the registrar draws no icon for it: one calm hue per table app
    /// (`dock::DP_PINS` order), then a hash pick for a window no pin names.
    pub const APP_HUES: [u32; 11] = [
    0x2E2E33, // console
    0x3B3F46, // shell
    0xD99A1E, // quarry
    0x2F9461, // activity
    0x6E6E78, // settings
    0x2B78C9, // editor
    0xC9822B, // lumen
    0x4C64D6, // aether
    0x7F58C7, // midden
    0x138C88, // una
    0xC9465C, // stria
    ];
}
