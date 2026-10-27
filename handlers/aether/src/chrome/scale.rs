//! AETHERSCALE (SR190) — the header's METRICS as one rule of the chrome face: the face follows the display font size,
//! the face's line grows every header band by the same amount, and the menu is fitted inside the window. std-free
//! (core only), linked into AETHER.ELF's chrome by `#[path]` (R104) so the host test reads the bytes the image builds.
//!
//! THE RULE: face = [`FACE_BASE`] x css / default css; its LINE = round(face x [`LINE_RATIO`]); a band of base height
//! `b` is `grow(b) = b + max(0, line - LINE_BASE)` (LINE_BASE = line(FACE_BASE): the kernel's `TextMetrics::grow`
//! mirrored). At the default face every height is the base (TAB 26, BAR 30, CTL 22, MENU_ROW 22, STATUS 14).
//!
//! THE BOUND is the surface: the header and the status line leave at least one page line on a [`SURFACE_H`] window
//! across the whole font-size schema (9..=32); the menu's row is the one value the surface caps
//! ([`Metrics::menu`]), and its face is held to that row's line.

/// The chrome's body face at the default font size, px.
pub const FACE_BASE: f32 = 11.0;
/// A face's line over its size (the ratio the old FONT_MAX already used).
pub const LINE_RATIO: f32 = 1.25;
/// The default face's line, px (`line(FACE_BASE)`): bands grow by the line's excess over it.
pub const LINE_BASE: i32 = 14;
/// The base heights (px at the default face): the tab strip, the toolbar, a control square (the field's height), a
/// menu row, the status line.
pub const TAB_H: i32 = 26;
pub const BAR_H: i32 = 30;
pub const CTL: i32 = 22;
pub const MENU_ROW: i32 = 22;
pub const STATUS_H: i32 = 14;
/// The tallest window AETHER.ELF's surface holds (`main.rs MAX_H`, BP-video-8), and one page line at the default.
pub const SURFACE_H: i32 = 288;
pub const PAGE_LINE: i32 = 20;

/// The chrome face for font size `css` (px) when `default_css` is the default size.
pub fn face_for(css: i32, default_css: i32) -> f32 {
    FACE_BASE * css as f32 / default_css.max(1) as f32
}

/// A face's line, px (rounded half up; faces are positive).
pub fn line(face: f32) -> i32 {
    (face * LINE_RATIO + 0.5) as i32
}

/// A band of base height `base` at a face whose line is `line`.
pub fn grow(base: i32, line: i32) -> i32 {
    base + (line - LINE_BASE).max(0)
}

/// Every header height at one face, px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub line: i32,
    pub tab_h: i32,
    pub bar_h: i32,
    pub ctl: i32,
    pub menu_row: i32,
    pub status_h: i32,
}

/// The open menu's fit: its row height, its face, and its top edge (window px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Menu {
    pub row: i32,
    pub face: f32,
    pub y: i32,
}

impl Metrics {
    /// The metrics at body face `face`.
    pub fn at(face: f32) -> Metrics {
        let l = line(face);
        Metrics {
            line: l,
            tab_h: grow(TAB_H, l),
            bar_h: grow(BAR_H, l),
            ctl: grow(CTL, l),
            menu_row: grow(MENU_ROW, l),
            status_h: grow(STATUS_H, l),
        }
    }

    /// The header's height (tab strip + toolbar): the page starts below it in the top layout.
    pub fn chrome_h(&self) -> i32 {
        self.tab_h + self.bar_h
    }

    /// The page's height in a `win_h` window between the header and the status line.
    pub fn content_h(&self, win_h: i32) -> i32 {
        win_h - self.chrome_h() - self.status_h
    }

    /// The menu of `rows` rows (inner padding `pad`) at face `face`, fitted inside a `win_h` window: the row is the
    /// grown row, capped so every row fits (`(win_h - 4 - 2 pad) / rows`); the top sits under the page origin
    /// (`page_y + 2`), lifted so the menu ends 2 px above the window's foot, never above 2; the face is held to the
    /// row (`row / LINE_RATIO`) so a row never carries more than its line.
    pub fn menu(&self, face: f32, rows: i32, pad: i32, page_y: i32, win_h: i32) -> Menu {
        let rows = rows.max(1);
        let row = self.menu_row.min((win_h - 4 - 2 * pad) / rows).max(1);
        let h = rows * row + 2 * pad;
        let y = (page_y + 2).min(win_h - h - 2).max(2);
        Menu { row, face: face.min(row as f32 / LINE_RATIO), y }
    }
}
