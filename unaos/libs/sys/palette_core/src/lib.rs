// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! PALETTECORE (rmbp-ledger B559) — the ONE table of UnaOS colours, linked by both rings.
//!
//! CHARTER: Principia owns the appearance choice (`system.appearance.mode` / `.accent` / `.highlight`,
//! `prefs_core::appearance`); this crate owns only the values that choice indexes. Seam: `shared-core`.
//!
//! Before this crate the kernel's `video/theme.rs` held the palette and each of the four ring-3 programs
//! (`crates/user-{aether,una,midden,lumen}/src/theme.rs`) held a COPY of the rows it read (B552's owed). Now the
//! kernel re-exports every name below from `video/theme.rs` (consumers keep reading `theme::SURFACE_WINDOW`,
//! `theme::LIGHT`, `theme::fixture::…`), the programs map their own roles over [`Look`] / [`Theme`], and this file
//! is the only place a colour literal lives (`unaos/scripts/appearance-check.py` scans the kernel's `theme.rs` and
//! the four programs' `theme.rs` for zero).
//!
//! DESIGNAPPLY (rmbp-ledger B639, R120–R122): the chrome rows are DESIGN-LANGUAGE §2.3's semantic tokens (named
//! by job, generated in OKLCH, checked against WCAG 2.x), which replaced the kit's material names (bevel, gloss,
//! the two-stop title gradient) in one commit; the eight accents carry §2.3's fill / hover / pressed / ink, and the
//! default keeps its name `crispy` (R121). The provenance of every value is §2.3's table
//! (`docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md`) and `docs/dev/evidence/designapply/DESIGNAPPLY.md`.
//! Colours are packed `0x00RRGGBB`; alphas are separate scalars (of 255).

#![no_std]
#![forbid(unsafe_code)]

// ---------------------------------------------------------------------------------------------------------------
// DESIGNAPPLY (rmbp-ledger B639, R121) — the SEMANTIC tokens of DESIGN-LANGUAGE §2.3, named by job, generated in
// OKLCH. They REPLACE the kit's material names (`BevelLight`, `BevelShadow`, `GlossHighlight`, the two-stop title
// gradient …) that drew an Aqua-era bevelled window. Every value below is §2.3's sRGB hex, as the table gives it;
// neutrals carry a trace of the accent hue (OKLCH chroma 0.006 at h 265). Light and Dark are NOT inversions: in
// Dark, elevation is a LIGHTER surface (window < raised < overlay), because shadows are hard to see there.
// ---------------------------------------------------------------------------------------------------------------

/// `surface.window` — window body, toolbars, title bar.
pub const SURFACE_WINDOW: (u32, u32) = (0x00F5_F7FB, 0x001D_1E21);
/// `surface.sidebar` — sidebars (Settings, Quarry, Aether's side tabs).
pub const SURFACE_SIDEBAR: (u32, u32) = (0x00EB_EDF1, 0x0025_2629);
/// `surface.content` — documents, web pages, list views, fields.
pub const SURFACE_CONTENT: (u32, u32) = (0x00FF_FFFF, 0x0016_171A);
/// `surface.raised` — grouped-list cards, the segmented thumb, a knob, a resting button.
pub const SURFACE_RAISED: (u32, u32) = (0x00FF_FFFF, 0x0029_2A2D);
/// `surface.overlay` — menus, popovers, toasts, dialogs.
pub const SURFACE_OVERLAY: (u32, u32) = (0x00FA_FCFF, 0x002F_3033);
/// `surface.sunken` — the segmented track, the omnibox at rest, a slider's track, a pressed button.
pub const SURFACE_SUNKEN: (u32, u32) = (0x00EE_F0F4, 0x0012_1417);
/// `line.subtle` — separators, card borders.
pub const LINE_SUBTLE: (u32, u32) = (0x00DC_DEE2, 0x0034_3539);
/// `line.strong` — the window outline (Light, at 60 %), inactive control discs.
pub const LINE_STRONG: (u32, u32) = (0x00BC_BEC2, 0x004B_4D50);
/// `line.control` — check box, radio, field and toggle-off borders: >= 3:1 against every surface.
pub const LINE_CONTROL: (u32, u32) = (0x0084_868A, 0x007E_8084);
/// `text.primary` — body text: >= 14.7:1 (Light), >= 11.7:1 (Dark) on every surface.
pub const TEXT_PRIMARY: (u32, u32) = (0x0019_1B1D, 0x00F0_F2F6);
/// `text.secondary` — subtitles, shortcuts: >= 5.8:1 / >= 6.1:1.
pub const TEXT_SECONDARY: (u32, u32) = (0x0059_5B5E, 0x00AF_B1B5);
/// `text.tertiary` — placeholders, timestamps, an unfocused window's title: >= 4.5:1 on every surface.
pub const TEXT_TERTIARY: (u32, u32) = (0x0067_696C, 0x0096_989C);
/// `text.disabled` — disabled labels (exempt from WCAG 1.4.3).
pub const TEXT_DISABLED: (u32, u32) = (0x00A3_A5A8, 0x005C_5E61);

/// The window-control discs (Una): OKLCH (0.66 0.19 28), (0.83 0.15 85), (0.74 0.17 150) — OUR values, not the
/// macOS hexes the kit's DERIVED `CTRL_*` were (§2.3: the red-amber-green convention is free to follow; the exact
/// values are made ours). The same in Light and Dark.
pub const CTRL_CLOSE: u32 = 0x00F0_584B;
pub const CTRL_MIN: u32 = 0x00F4_BF3F;
pub const CTRL_ZOOM: u32 = 0x0047_C86E;
/// DESIGN-LANGUAGE §3.7 (MENUSCRAFT B642): a kit sheet never paints PURE WHITE; the seal reads for it and steps
/// to [`OFF_WHITE`]. The core holds both so no painter or fixture carries the literal (the appearance gate).
pub const PURE_WHITE: u32 = 0x00FF_FFFF;
/// The one-step-off white the seal substitutes for [`PURE_WHITE`].
pub const OFF_WHITE: u32 = 0x00FE_FEFE;

/// `status.*` — FIXED, not accent-derived: they say the same thing in every theme. Each fill takes a white label
/// (>= 4.5:1); the ink (status as text or icon on a surface) is the fill in Light and §2.3's lifted ink in Dark.
pub const STATUS_POSITIVE: u32 = 0x0032_851B;
pub const STATUS_WARNING: u32 = 0x00A2_6708;
pub const STATUS_CRITICAL: u32 = 0x00BC_4945;
pub const STATUS_INFO: u32 = 0x0015_77C8;
pub const STATUS_INK_DARK: [u32; 4] = [0x0089_CC7B, 0x00E8_AA4E, 0x00FF_958D, 0x0073_BDFF];

/// `scrim` — black behind system-modal dialogs, at 30 % (Light) / 50 % (Dark), of 255.
pub const SCRIM_ALPHA: (u32, u32) = (77, 128);
/// `rim.dark` — white at 8 % (of 255): the 1-device-px inner rim on dark windows and overlays.
pub const RIM_DARK_ALPHA: u32 = 20;
/// `rim.dark`'s colour (white; the alpha above).
pub const RIM_DARK: u32 = 0x00FF_FFFF;
/// White — the label on an accent or status fill (every fill is solved for >= 4.6:1 white text).
pub const ON_FILL: u32 = 0x00FF_FFFF;

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
// DESIGNAPPLY (B639): the rows are §2.3's semantic set (above) plus the content inks the painters lifted
// (syntax), and the status inks.
// ---------------------------------------------------------------------------------------------------------------

/// One palette token, named by its JOB (DESIGN-LANGUAGE §2.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tok {
    SurfaceWindow, SurfaceSidebar, SurfaceContent, SurfaceRaised, SurfaceOverlay, SurfaceSunken,
    LineSubtle, LineStrong, LineControl,
    TextPrimary, TextSecondary, TextTertiary, TextDisabled,
    CtrlClose, CtrlMin, CtrlZoom,
    StatusPositive, StatusWarning, StatusCritical, StatusInfo,
    InkPositive, InkWarning, InkCritical, InkInfo,
    SyntaxCode, SyntaxNum, SyntaxLit,
}

/// DESIGNAPPLY (B639): the retired material names, kept ONLY as aliases for the one ring-3 consumer outside this
/// arc's lane (`crates/user-aether/src/theme.rs`, the cloud's), each pointing at the token that now does its job.
/// The kernel and the other three programs read the semantic names; the aether lane repoints and deletes these.
#[allow(non_upper_case_globals)]
impl Tok {
    pub const WindowBg: Tok = Tok::SurfaceWindow;
    pub const TitleActiveBottom: Tok = Tok::SurfaceWindow;
    pub const FrameLine: Tok = Tok::LineSubtle;
    pub const ContentFill: Tok = Tok::SurfaceContent;
    pub const ContentText: Tok = Tok::TextPrimary;
    pub const TitleTextInactive: Tok = Tok::TextSecondary;
    pub const FieldBg: Tok = Tok::SurfaceContent;
    pub const ErrorText: Tok = Tok::InkCritical;
}

/// Tokens in a set (the `tokens=` of the kernel's appearance witness).
pub const TOKENS: usize = 27;
/// The token names, [`Tok`] order, spelled as DESIGN-LANGUAGE §2.3 spells them.
pub const TOKEN_NAMES: [&str; TOKENS] = [
    "surface.window", "surface.sidebar", "surface.content", "surface.raised", "surface.overlay", "surface.sunken",
    "line.subtle", "line.strong", "line.control",
    "text.primary", "text.secondary", "text.tertiary", "text.disabled",
    "ctrl.close", "ctrl.min", "ctrl.zoom",
    "status.positive", "status.warning", "status.critical", "status.info",
    "ink.positive", "ink.warning", "ink.critical", "ink.info",
    "syntax.code", "syntax.num", "syntax.lit",
];

/// FileView's / Lumen's syntax tints (content inks, not chrome), Light then Dark.
pub const SYNTAX: [(u32, u32); 3] = [(0x0020_8040, 0x006C_C88A), (0x00B0_5000, 0x00E3_9A55), (0x0080_30A0, 0x00C3_8BE0)];

/// The LIGHT set (§2.3's Light column).
pub const LIGHT: [u32; TOKENS] = [
    SURFACE_WINDOW.0, SURFACE_SIDEBAR.0, SURFACE_CONTENT.0, SURFACE_RAISED.0, SURFACE_OVERLAY.0, SURFACE_SUNKEN.0,
    LINE_SUBTLE.0, LINE_STRONG.0, LINE_CONTROL.0,
    TEXT_PRIMARY.0, TEXT_SECONDARY.0, TEXT_TERTIARY.0, TEXT_DISABLED.0,
    CTRL_CLOSE, CTRL_MIN, CTRL_ZOOM,
    STATUS_POSITIVE, STATUS_WARNING, STATUS_CRITICAL, STATUS_INFO,
    STATUS_POSITIVE, STATUS_WARNING, STATUS_CRITICAL, STATUS_INFO, // the inks: the fills themselves on a light surface
    SYNTAX[0].0, SYNTAX[1].0, SYNTAX[2].0,
];

/// The DARK set (§2.3's Dark column).
pub const DARK: [u32; TOKENS] = [
    SURFACE_WINDOW.1, SURFACE_SIDEBAR.1, SURFACE_CONTENT.1, SURFACE_RAISED.1, SURFACE_OVERLAY.1, SURFACE_SUNKEN.1,
    LINE_SUBTLE.1, LINE_STRONG.1, LINE_CONTROL.1,
    TEXT_PRIMARY.1, TEXT_SECONDARY.1, TEXT_TERTIARY.1, TEXT_DISABLED.1,
    CTRL_CLOSE, CTRL_MIN, CTRL_ZOOM,
    STATUS_POSITIVE, STATUS_WARNING, STATUS_CRITICAL, STATUS_INFO,
    STATUS_INK_DARK[0], STATUS_INK_DARK[1], STATUS_INK_DARK[2], STATUS_INK_DARK[3],
    SYNTAX[0].1, SYNTAX[1].1, SYNTAX[2].1,
];

/// THE ACCENT RULE (§2.3). The eight accents (`prefs_core::appearance::ACCENTS` order) share chroma 0.16 (slate
/// 0.035, teal gamut-limited to 0.09); each `accent.fill` is the hue at the highest OKLCH lightness where white text
/// on it is >= 4.6:1, so switching accent never changes perceived weight. `crispy` KEEPS its name (R121: "crispy
/// stays") and takes §2.3's default periwinkle (h 268).
pub const ACCENTS: [u32; 8] = [0x004E_6DD3, 0x0013_8282, 0x0032_851B, 0x00A2_6708, 0x00C4_4F21, 0x00C4_486D, 0x008A_5FC9, 0x0065_7689];
/// `accent.hover` — `accent.fill` at L - 0.04.
pub const ACCENT_HOVER: [u32; 8] = [0x0043_61C5, 0x000A_7575, 0x0024_7905, 0x0092_5D0B, 0x00B6_420F, 0x00B6_3C62, 0x007E_53BC, 0x005A_6B7D];
/// `accent.pressed` — `accent.fill` at L - 0.08.
pub const ACCENT_PRESSED: [u32; 8] = [0x0039_55B8, 0x0001_6869, 0x001D_6C00, 0x0084_5300, 0x00A6_3801, 0x00A8_2E57, 0x0073_47AF, 0x004F_5F71];
/// `accent.ink` on a Light surface — L = min(L_fill, 0.50).
pub const ACCENT_INK_LIGHT: [u32; 8] = [0x003D_5ABD, 0x0001_7272, 0x0022_7702, 0x0089_5709, 0x00A8_3903, 0x00A8_2E57, 0x0073_47AF, 0x0054_6577];
/// `accent.ink` on a Dark surface — L = 0.80.
pub const ACCENT_INK_DARK: [u32; 8] = [0x00A2_BCFF, 0x0010_D8D8, 0x0083_D670, 0x00FE_A92F, 0x00FD_A485, 0x00FF_9CB3, 0x00C9_ADFD, 0x00AD_C0D4];
/// Their names, spelled as `prefs_core::appearance::ACCENTS` stores them (this crate has no deps; the kernel, which
/// links both, asserts the two lists equal).
pub const ACCENT_NAMES: [&str; 8] = ["crispy", "teal", "moss", "amber", "clay", "rose", "violet", "slate"];
/// The default accent's fill (`crispy`, [`ACCENTS`]`[0]`).
pub const ACCENT: u32 = ACCENTS[0];

/// The retiring MATERIALS' own base tones (DESIGN-LANGUAGE §9 "chrome materials to retire": `ceramic`, `knurl`,
/// `paper`). Not tokens — no painter's role reads them; they are the inputs the material ENGINES were written and
/// golden-tested against, kept so `paper.rs` (whose procedural engine survives for the wallpaper) keeps its base
/// until the craft arc that retires it. Values are the kit's (`kits/crispy/theme.json` @ `0787ba9f`), unchanged.
pub mod material {
    /// `palette.content_fill` = `content_surface.Paper.base_rgb` `[0.96, 0.949, 0.918]` — the paper's base.
    pub const PAPER: u32 = 0x00F5_F2EA;
    /// `palette.chrome_face` `[0.925, 0.925, 0.933]` — what `ceramic` was machined over.
    pub const CERAMIC: u32 = 0x00EC_ECEE;
    /// The three disc hues `knurl` was milled into (the kit era's DERIVED macOS hexes), golden-test inputs only.
    pub const KNURL_IN: [u32; 3] = [0x00FF_5F57, 0x00FE_BC2E, 0x0028_C840];
}

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
    /// `accent.fill` — `system.appearance.accent` (the primary button, the selected row, toggle-on, slider fill).
    #[inline]
    pub const fn accent(self) -> u32 {
        ACCENTS[self.accent as usize % 8]
    }
    /// `accent.hover`.
    #[inline]
    pub const fn accent_hover(self) -> u32 {
        ACCENT_HOVER[self.accent as usize % 8]
    }
    /// `accent.pressed`.
    #[inline]
    pub const fn accent_pressed(self) -> u32 {
        ACCENT_PRESSED[self.accent as usize % 8]
    }
    /// `accent.ink` — the accent as text or icon on this set's surfaces.
    #[inline]
    pub const fn accent_ink(self) -> u32 {
        if self.dark { ACCENT_INK_DARK[self.accent as usize % 8] } else { ACCENT_INK_LIGHT[self.accent as usize % 8] }
    }
    /// `focus.ring` — `accent.fill` in Light (>= 4.0:1 on the window and sidebar), `accent.ink` in Dark (>= 8.8:1).
    #[inline]
    pub const fn focus_ring(self) -> u32 {
        if self.dark { ACCENT_INK_DARK[self.accent as usize % 8] } else { ACCENTS[self.accent as usize % 8] }
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
        mix(self.highlight(), self.tok(Tok::SurfaceContent), num, den)
    }
    pub const fn mode_name(self) -> &'static str {
        if self.dark { "dark" } else { "light" }
    }
    pub const fn accent_name(self) -> &'static str {
        ACCENT_NAMES[self.accent as usize % 8]
    }
}

/// The role struct: every token of a look resolved once, plus the chosen colours. A painter maps its own roles over
/// it (`Theme::of(look).tok(Tok::LineSubtle)`, `.accent`, `.selection`).
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
    /// The kit's paper `base_rgb` under the pinned rounding — `paper.rs`'s tripwire against [`super::material::PAPER`].
    pub const PAPER_BASE: u32 = 0x00F5_F2EA;
    /// `ceramic.rs` leg 4: [`super::material::CERAMIC`] under the material, rows 0..8.
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
    assert!(t.tok(Tok::LineSubtle) == DARK[Tok::LineSubtle as usize]);
    assert!(t.accent == ACCENTS[3] && t.highlight == ACCENTS[5]);
    assert!(t.selection == mix(ACCENTS[5], DARK[Tok::SurfaceContent as usize], 3, 8));
    // DESIGNAPPLY (B639): §2.3's shape — Dark elevation by lightness (window < raised < overlay, per channel sum),
    // the text ramp darkest-first in Light and lightest-first in Dark, the accent ladder fill > hover > pressed in
    // lightness, the three discs three different hues, and the retired-name aliases pointing at real tokens.
    const fn lum(c: u32) -> u32 {
        ((c >> 16) & 255) + ((c >> 8) & 255) + (c & 255)
    }
    assert!(lum(DARK[Tok::SurfaceWindow as usize]) < lum(DARK[Tok::SurfaceRaised as usize]));
    assert!(lum(DARK[Tok::SurfaceRaised as usize]) < lum(DARK[Tok::SurfaceOverlay as usize]));
    assert!(lum(LIGHT[Tok::TextPrimary as usize]) < lum(LIGHT[Tok::TextSecondary as usize]));
    assert!(lum(LIGHT[Tok::TextSecondary as usize]) < lum(LIGHT[Tok::TextTertiary as usize]));
    assert!(lum(DARK[Tok::TextPrimary as usize]) > lum(DARK[Tok::TextSecondary as usize]));
    assert!(lum(DARK[Tok::TextSecondary as usize]) > lum(DARK[Tok::TextTertiary as usize]));
    let mut i = 0;
    while i < 8 {
        assert!(lum(ACCENTS[i]) > lum(ACCENT_HOVER[i]) && lum(ACCENT_HOVER[i]) > lum(ACCENT_PRESSED[i]));
        assert!(lum(ACCENT_INK_DARK[i]) > lum(ACCENTS[i]));
        i += 1;
    }
    assert!(CTRL_CLOSE != CTRL_MIN && CTRL_MIN != CTRL_ZOOM && CTRL_CLOSE != CTRL_ZOOM);
    assert!(Tok::WindowBg as usize == Tok::SurfaceWindow as usize && Tok::ErrorText as usize == Tok::InkCritical as usize);
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

    /// SOFTYDARK (rmbp-ledger B694) — Softy's OWN Dark set, selected by `system.appearance.mode` exactly as the Una
    /// look's Dark column is (`video::softy::palette` picks per paint through `theme::is_dark`). Windows 11's dark
    /// comfort, our values: a near-black taskbar under the VEIL, near-black heads, a lighter flyout (dark elevation is
    /// LIGHTER, DESIGN-LANGUAGE §2.3), light ink, white hairlines at a whisper; the accent stays the accent table's.
    ///
    /// DERIVATION (palette_core::contrast's method, rerunnable): every value is OKLCH (L, 0, -) — chroma 0, like the
    /// Light table above — → OKLab → linear sRGB (Ottosson) → gamma-encoded, 8 bits. The GROUNDS are chosen on a
    /// ramp: FIELD 0.200 → 161616 (the sunken search well), SURFACE_IDLE 0.220 → 1B1B1B, SURFACE 0.240 → 1F1F1F,
    /// FLYOUT_FOOT 0.255 → 232323, FLYOUT 0.285 → 2A2A2A. The INKS are SOLVED: each is the LOWEST L (0.001 steps)
    /// whose WCAG 2.x ratio to the lightest of the five grounds (FLYOUT) reaches its target: TEXT pinned at L 0.960 →
    /// F2F2F2 (12.82); TEXT_2 6:1 (the Light ramp's 5.8, made a floor) → 0.730 A8A8A8 (6.03); TEXT_IDLE 4.5:1 → 0.656
    /// 919191 (4.55); FIELD_LINE 3:1 ([S39]) → 0.554 737373 (3.02); EDGE_IDLE takes the Light table's own measure,
    /// its border against its idle head (C8C8C8 on FAFAFA, 1.60:1) → 0.362 3E3E3E (1.61 on 1B1B1B). The const block
    /// in `contrast` re-derives every floor from the hex at build time.
    pub mod dark {
        /// The taskbar (under the VEIL at [`VEIL_A`]) and the focused window's head.
        pub const SURFACE: u32 = 0x1F1F1F;
        /// An unfocused head: one step DOWN (it recedes; dark elevation rises toward light).
        pub const SURFACE_IDLE: u32 = 0x1B1B1B;
        /// The Start menu's, the calendar's and quick settings' body (an overlay: lighter).
        pub const FLYOUT: u32 = 0x2A2A2A;
        /// The Start menu's footer band.
        pub const FLYOUT_FOOT: u32 = 0x232323;
        /// Primary ink.
        pub const TEXT: u32 = 0xF2F2F2;
        /// Secondary ink.
        pub const TEXT_2: u32 = 0xA8A8A8;
        /// An unfocused window's caption.
        pub const TEXT_IDLE: u32 = 0x919191;
        /// The hairline: white, at the dark alpha (`rim.dark`'s idea: edges survive a dark wallpaper).
        pub const STROKE: u32 = 0xFFFFFF;
        /// An unfocused window's 1 px border.
        pub const EDGE_IDLE: u32 = 0x3E3E3E;
        /// A hovered control's wash and a pressed one's: white at a whisper (the dark state layers).
        pub const HOVER: u32 = 0xFFFFFF;
        pub const PRESS: u32 = 0xFFFFFF;
        /// The close button under the pointer: the same red in both modes (status colours never change by theme).
        pub const CLOSE_HOT: u32 = super::CLOSE_HOT;
        /// The search field's face (sunken, below every ground) and its editable underline.
        pub const FIELD: u32 = 0x161616;
        pub const FIELD_LINE: u32 = 0x737373;
        /// The idle running pip.
        pub const PIP: u32 = TEXT_IDLE;
        /// The VEIL's tint for the taskbar: `SURFACE` at 78 % over the blurred wallpaper (DESIGN-LANGUAGE §2.4, Dark).
        pub const VEIL_A: u8 = 199;
    }
}

/// STARFIELD (rmbp-ledger B661, R128) — the generated default wallpaper's palette: dark matter on ULTRA BLACK, the
/// lensed stars, the zircon and the beam (`docs/dev/OS/05_USER_EXPERIENCE/STARFIELD.md` §3). `(Light, Dark)` pairs;
/// the Dark set is the one the scene is for, the Light set is the same generator, quieter. Appended at the crate's
/// tail so no line above moves.
pub mod starfield {
    /// The ground. Dark: ultra black, never `#000000` (DESIGN-LANGUAGE §2.3 keeps pure black for the console and the
    /// media grounds; the blue survives two overlapping window shadows, so DESKTOPCRAFT's `black=` holds).
    pub const VOID: (u32, u32) = (0x00E8_EAF4, 0x0004_0612);
    /// The dark-matter haze and the faintest strokes (deep blue-violet).
    pub const MATTER_DEEP: (u32, u32) = (0x00D3_D6EE, 0x0016_0F44);
    /// The strokes at middle density (violet-blue).
    pub const MATTER_MID: (u32, u32) = (0x00A3_AEE2, 0x0030_47C4);
    /// The densest strokes and the streams' travelling glints (faint cyan; darker ink on Light).
    pub const MATTER_HI: (u32, u32) = (0x0071_83CC, 0x0086_E2F2);
    /// A star (blue-white; a muted dot on Light).
    pub const STAR: (u32, u32) = (0x0088_93C8, 0x00DC_E6FF);
    /// A warm bright star (the Starry Night yellow).
    pub const STAR_WARM: (u32, u32) = (0x00C2_9A68, 0x00FF_E2B0);
    /// The zircon's shadowed facets, its lit facets, its highlight and rim.
    pub const ZIRCON_DEEP: (u32, u32) = (0x0025_5682, 0x0004_2440);
    pub const ZIRCON_MID: (u32, u32) = (0x0056_9ED4, 0x0018_86C4);
    pub const ZIRCON_LIGHT: (u32, u32) = (0x00F2_FAFF, 0x00BE_F6FF);
    /// The zircon's fire (dispersion): a warm and a cool flash.
    pub const FIRE_WARM: (u32, u32) = (0x00EE_AE6A, 0x00FF_B45A);
    pub const FIRE_COOL: (u32, u32) = (0x00A2_86E6, 0x00B0_7CFF);
    /// The beam's core and its glow.
    pub const BEAM_CORE: (u32, u32) = (0x00FA_FCFF, 0x00F2_FAFF);
    pub const BEAM_GLOW: (u32, u32) = (0x00B4_D2FA, 0x0060_B0FF);
}

/// CONTRAST (rmbp-ledger B664) — the INCREASE-CONTRAST variant of the token table (Settings → Accessibility,
/// `system.accessibility.increase_contrast`): the SAME 27 tokens, the six surfaces, the discs, status, inks and
/// syntax UNCHANGED, the text ramp and the three lines pushed toward the ends of the OKLCH lightness range so every
/// text token meets DESIGN-LANGUAGE §2.3's contrast floor for this set, WCAG 1.4.6's enhanced 7:1, against EVERY
/// surface of its set. Appended at the crate's tail so no line above moves.
///
/// DERIVATION (§10's method, rerunnable): a token is OKLCH (L, 0.006, 265) — §2.3's neutral trace of the accent hue —
/// → OKLab → linear sRGB (Ottosson's matrices) → gamma-encoded and rounded to 8 bits; WCAG 2.x relative luminance
/// `Y = 0.2126 R + 0.7152 G + 0.0722 B` of the linearised channels (`c <= 0.04045 ? c/12.92 : ((c+0.055)/1.055)^2.4`)
/// and the ratio `(Y_hi + 0.05) / (Y_lo + 0.05)`, never rounded. In Light, L is the HIGHEST lightness (in 0.001 steps)
/// whose ratio to the LIGHTEST-ink-hostile surface (the minimum over all six) reaches the target; in Dark, the
/// LOWEST. Targets: `text.primary` is pinned at the ends (L 0.13 Light, 0.985 Dark); `text.secondary` 9:1 (so the
/// ramp still steps above the floor); `text.tertiary` 7:1 (the floor); `text.disabled` 4.5:1 (no longer exempt-dim);
/// `line.subtle` 3:1 (§2.3's control boundary, [S39] — "`line.*` to `line.control`"); `line.strong` 4.5:1;
/// `line.control` 6:1. Results (L → hex, minimum ratio over the six surfaces):
///
/// | token | Light | Dark |
/// |---|---|---|
/// | text.primary | 0.130 → 06070A, 17.19 | 0.985 → F8FAFE, 12.63 |
/// | text.secondary | 0.367 → 3D3F42, 9.01 | 0.875 → D4D6DA, 9.07 |
/// | text.tertiary | 0.427 → 4D4F52, 7.01 | 0.797 → BBBDC1, 7.01 |
/// | text.disabled | 0.529 → 6A6B6F, 4.54 | 0.675 → 95979B, 4.51 |
/// | line.subtle | 0.628 → 87888C, 3.02 | 0.575 → 77797C, 3.02 |
/// | line.strong | 0.529 → 6A6B6F, 4.54 | 0.675 → 95979B, 4.51 |
/// | line.control | 0.461 → 57585C, 6.06 | 0.753 → ADAFB3, 6.00 |
///
/// SOFTY's Light table is its own (chroma 0, `softy::*`), so its pairs are solved the same way against Softy's five
/// grounds (SURFACE, SURFACE_IDLE, FLYOUT, FLYOUT_FOOT, FIELD): TEXT L 0.13 → 070707 (17.52), TEXT_2 9:1 → 404040
/// (9.02), TEXT_IDLE 7:1 → 505050 (7.01), EDGE_IDLE 3:1 → 8A8A8A (3.00), FIELD_LINE 4.5:1 → 6C6C6C (4.57); the idle
/// pip takes TEXT_IDLE. Softy's Dark is its own set (`super::softy::dark`, SOFTYDARK B694), its twins solved against
/// its five Dark grounds (`softy::dark` here). The const blocks at the end of this module re-derive every ratio from
/// the hex at compile time ([`ratio_x100`]) and fail the build below a floor.
pub mod contrast {
    use super::{Tok, TOKENS};

    /// `text.primary` (Light, Dark).
    pub const TEXT_PRIMARY: (u32, u32) = (0x0006_070A, 0x00F8_FAFE);
    /// `text.secondary` — 9:1 on every surface.
    pub const TEXT_SECONDARY: (u32, u32) = (0x003D_3F42, 0x00D4_D6DA);
    /// `text.tertiary` — the 7:1 floor on every surface.
    pub const TEXT_TERTIARY: (u32, u32) = (0x004D_4F52, 0x00BB_BDC1);
    /// `text.disabled` — 4.5:1 on every surface.
    pub const TEXT_DISABLED: (u32, u32) = (0x006A_6B6F, 0x0095_979B);
    /// `line.subtle` — 3:1 on every surface.
    pub const LINE_SUBTLE: (u32, u32) = (0x0087_888C, 0x0077_797C);
    /// `line.strong` — 4.5:1 on every surface.
    pub const LINE_STRONG: (u32, u32) = (0x006A_6B6F, 0x0095_979B);
    /// `line.control` — 6:1 on every surface.
    pub const LINE_CONTROL: (u32, u32) = (0x0057_585C, 0x00AD_AFB3);

    /// The base set with the seven pushed tokens replaced (`dark` picks the column).
    const fn push(base: [u32; TOKENS], dark: bool) -> [u32; TOKENS] {
        const fn c(p: (u32, u32), dark: bool) -> u32 {
            if dark { p.1 } else { p.0 }
        }
        let mut r = base;
        r[Tok::LineSubtle as usize] = c(LINE_SUBTLE, dark);
        r[Tok::LineStrong as usize] = c(LINE_STRONG, dark);
        r[Tok::LineControl as usize] = c(LINE_CONTROL, dark);
        r[Tok::TextPrimary as usize] = c(TEXT_PRIMARY, dark);
        r[Tok::TextSecondary as usize] = c(TEXT_SECONDARY, dark);
        r[Tok::TextTertiary as usize] = c(TEXT_TERTIARY, dark);
        r[Tok::TextDisabled as usize] = c(TEXT_DISABLED, dark);
        r
    }

    /// The LIGHT set under increased contrast.
    pub const LIGHT: [u32; TOKENS] = push(super::LIGHT, false);
    /// The DARK set under increased contrast.
    pub const DARK: [u32; TOKENS] = push(super::DARK, true);

    /// The set a look resolves through: `dark` picks the column, `on` the contrast variant.
    #[inline]
    pub const fn rows(dark: bool, on: bool) -> &'static [u32; TOKENS] {
        match (dark, on) {
            (false, false) => &super::LIGHT,
            (true, false) => &super::DARK,
            (false, true) => &LIGHT,
            (true, true) => &DARK,
        }
    }

    /// SOFTY's own high-contrast pairs (its Light table; see the derivation above).
    pub mod softy {
        pub const TEXT: u32 = 0x070707;
        pub const TEXT_2: u32 = 0x404040;
        pub const TEXT_IDLE: u32 = 0x505050;
        pub const EDGE_IDLE: u32 = 0x8A8A8A;
        pub const FIELD_LINE: u32 = 0x6C6C6C;
        pub const PIP: u32 = TEXT_IDLE;

        /// SOFTYDARK (B694): the Dark twins, solved the same way against Softy's five DARK grounds (the LOWEST L
        /// reaching the target, chroma 0): TEXT pinned at L 0.985 → FAFAFA (13.75), TEXT_2 9:1 → 0.847 CDCDCD (9.02),
        /// TEXT_IDLE 7:1 → 0.772 B5B5B5 (7.00), EDGE_IDLE 3:1 → 0.554 737373 (3.02), FIELD_LINE 4.5:1 → 0.656 919191
        /// (4.55); the idle pip takes TEXT_IDLE.
        pub mod dark {
            pub const TEXT: u32 = 0xFAFAFA;
            pub const TEXT_2: u32 = 0xCDCDCD;
            pub const TEXT_IDLE: u32 = 0xB5B5B5;
            pub const EDGE_IDLE: u32 = 0x737373;
            pub const FIELD_LINE: u32 = 0x919191;
            pub const PIP: u32 = TEXT_IDLE;
        }
    }

    /// One 8-bit sRGB channel, linearised (WCAG 2.x). `x^2.4 = x^2 * (x^2)^(1/5)`; the fifth root by Newton from 1.0
    /// (the root lies in (0.27, 1] here, so 40 steps are far past convergence). No libm: `no_std`, const.
    const fn lin(c: u32) -> f64 {
        let v = c as f64 / 255.0;
        if v <= 0.04045 {
            return v / 12.92;
        }
        let x = (v + 0.055) / 1.055;
        let a = x * x;
        let mut y = 1.0;
        let mut i = 0;
        while i < 40 {
            let y4 = y * y * y * y;
            y -= (y4 * y - a) / (5.0 * y4);
            i += 1;
        }
        a * y
    }

    /// WCAG 2.x relative luminance of a packed `0x00RRGGBB`.
    pub const fn luminance(c: u32) -> f64 {
        0.2126 * lin((c >> 16) & 255) + 0.7152 * lin((c >> 8) & 255) + 0.0722 * lin(c & 255)
    }

    /// The contrast ratio of two colours x100, floored (thresholds are never rounded up: 6.999 is 699).
    pub const fn ratio_x100(a: u32, b: u32) -> u32 {
        let (x, y) = (luminance(a), luminance(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        ((hi + 0.05) / (lo + 0.05) * 100.0) as u32
    }

    /// The six surfaces, [`Tok`] order (the grounds every text token is held against).
    pub const SURFACES: [Tok; 6] =
        [Tok::SurfaceWindow, Tok::SurfaceSidebar, Tok::SurfaceContent, Tok::SurfaceRaised, Tok::SurfaceOverlay, Tok::SurfaceSunken];

    /// The lowest ratio (x100) of token `t` against the six surfaces of `set`.
    pub const fn min_on_surfaces(set: &[u32; TOKENS], t: Tok) -> u32 {
        let mut m = u32::MAX;
        let mut i = 0;
        while i < SURFACES.len() {
            let r = ratio_x100(set[t as usize], set[SURFACES[i] as usize]);
            if r < m {
                m = r;
            }
            i += 1;
        }
        m
    }

    /// Softy's five Light grounds (the text pairs are held against each).
    pub const SOFTY_GROUNDS: [u32; 5] =
        [super::softy::SURFACE, super::softy::SURFACE_IDLE, super::softy::FLYOUT, super::softy::FLYOUT_FOOT, super::softy::FIELD];

    /// The lowest ratio (x100) of `ink` against Softy's five Light grounds.
    pub const fn softy_min(ink: u32) -> u32 {
        let mut m = u32::MAX;
        let mut i = 0;
        while i < SOFTY_GROUNDS.len() {
            let r = ratio_x100(ink, SOFTY_GROUNDS[i]);
            if r < m {
                m = r;
            }
            i += 1;
        }
        m
    }

    /// SOFTYDARK (B694): Softy's five DARK grounds.
    pub const SOFTY_DARK_GROUNDS: [u32; 5] = [
        super::softy::dark::SURFACE,
        super::softy::dark::SURFACE_IDLE,
        super::softy::dark::FLYOUT,
        super::softy::dark::FLYOUT_FOOT,
        super::softy::dark::FIELD,
    ];

    /// The lowest ratio (x100) of `ink` against Softy's five Dark grounds.
    pub const fn softy_dark_min(ink: u32) -> u32 {
        let mut m = u32::MAX;
        let mut i = 0;
        while i < SOFTY_DARK_GROUNDS.len() {
            let r = ratio_x100(ink, SOFTY_DARK_GROUNDS[i]);
            if r < m {
                m = r;
            }
            i += 1;
        }
        m
    }

    // SOFTYDARK (B694): Softy's Dark set and its twins, re-derived from the hex — the floors, the elevations and the
    // ramp's order. A value edited below its target fails the build.
    const _: () = {
        use super::softy::dark as d;
        assert!(softy_dark_min(d::TEXT) >= 1200 && softy_dark_min(d::TEXT_2) >= 600 && softy_dark_min(d::TEXT_IDLE) >= 450);
        assert!(softy_dark_min(d::FIELD_LINE) >= 300 && ratio_x100(d::EDGE_IDLE, d::SURFACE_IDLE) >= 160);
        assert!(softy_dark_min(softy::dark::TEXT) >= 700 && softy_dark_min(softy::dark::TEXT_2) >= 900 && softy_dark_min(softy::dark::TEXT_IDLE) >= 700);
        assert!(softy_dark_min(softy::dark::EDGE_IDLE) >= 300 && softy_dark_min(softy::dark::FIELD_LINE) >= 450);
        assert!(softy_dark_min(softy::dark::TEXT_2) >= softy_dark_min(d::TEXT_2) && softy_dark_min(softy::dark::TEXT_IDLE) >= softy_dark_min(d::TEXT_IDLE));
        assert!(ratio_x100(d::CLOSE_HOT, super::softy::CLOSE_INK) >= 450);
        // Dark elevation rises toward light: field < idle head < taskbar < footer < flyout.
        assert!(luminance(d::FIELD) < luminance(d::SURFACE_IDLE) && luminance(d::SURFACE_IDLE) < luminance(d::SURFACE));
        assert!(luminance(d::SURFACE) < luminance(d::FLYOUT_FOOT) && luminance(d::FLYOUT_FOOT) < luminance(d::FLYOUT));
        // The ramp keeps its order, lightest-first.
        assert!(luminance(d::TEXT) > luminance(d::TEXT_2) && luminance(d::TEXT_2) > luminance(d::TEXT_IDLE));
        assert!(luminance(softy::dark::TEXT) > luminance(softy::dark::TEXT_2) && luminance(softy::dark::TEXT_2) > luminance(softy::dark::TEXT_IDLE));
    };

    // The floors, re-derived from the hex at compile time: a value edited below its target fails the build.
    const _: () = {
        let sets = [&LIGHT, &DARK];
        let mut s = 0;
        while s < 2 {
            let set = sets[s];
            // The surfaces, discs, status, inks and syntax are the base set's: only the seven pushed tokens move.
            let base = if s == 0 { &super::LIGHT } else { &super::DARK };
            let mut i = 0;
            while i < TOKENS {
                let pushed = i >= Tok::LineSubtle as usize && i <= Tok::TextDisabled as usize;
                assert!(pushed || set[i] == base[i]);
                assert!(set[i] <= 0x00FF_FFFF);
                i += 1;
            }
            assert!(min_on_surfaces(set, Tok::TextPrimary) >= 700);
            assert!(min_on_surfaces(set, Tok::TextSecondary) >= 900);
            assert!(min_on_surfaces(set, Tok::TextTertiary) >= 700);
            assert!(min_on_surfaces(set, Tok::TextDisabled) >= 450);
            assert!(min_on_surfaces(set, Tok::LineSubtle) >= 300);
            assert!(min_on_surfaces(set, Tok::LineStrong) >= 450);
            assert!(min_on_surfaces(set, Tok::LineControl) >= 600);
            // Every pushed token is at least as far from the surfaces as its base value (contrast only rises).
            let mut t = Tok::LineSubtle as usize;
            while t <= Tok::TextDisabled as usize {
                let tok = [Tok::LineSubtle, Tok::LineStrong, Tok::LineControl, Tok::TextPrimary, Tok::TextSecondary, Tok::TextTertiary, Tok::TextDisabled][t - Tok::LineSubtle as usize];
                assert!(min_on_surfaces(set, tok) >= min_on_surfaces(base, tok));
                t += 1;
            }
            s += 1;
        }
        assert!(softy_min(softy::TEXT) >= 700 && softy_min(softy::TEXT_2) >= 900 && softy_min(softy::TEXT_IDLE) >= 700);
        assert!(softy_min(softy::EDGE_IDLE) >= 300 && softy_min(softy::FIELD_LINE) >= 450);
        assert!(softy_min(softy::TEXT_2) >= softy_min(super::softy::TEXT_2) && softy_min(softy::TEXT_IDLE) >= softy_min(super::softy::TEXT_IDLE));
        // The ramp keeps its order (darkest-first in Light, lightest-first in Dark).
        assert!(luminance(LIGHT[Tok::TextPrimary as usize]) < luminance(LIGHT[Tok::TextSecondary as usize]));
        assert!(luminance(LIGHT[Tok::TextSecondary as usize]) < luminance(LIGHT[Tok::TextTertiary as usize]));
        assert!(luminance(DARK[Tok::TextPrimary as usize]) > luminance(DARK[Tok::TextSecondary as usize]));
        assert!(luminance(DARK[Tok::TextSecondary as usize]) > luminance(DARK[Tok::TextTertiary as usize]));
        // The method agrees with §2.3's published figure for the base set (text.primary on white, Light: >= 14.7).
        assert!(ratio_x100(super::TEXT_PRIMARY.0, super::SURFACE_CONTENT.0) >= 1470);
    };
}
