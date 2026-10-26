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

//! CHARTER: Kernel — shared-core
//!
//! KERNELFONT (rmbp-ledger B359) — `video::text`, the ONE text-drawing seam of the desktop.
//!
//! Every surface that puts a string on the glass calls here: the login screen, the menu bar, window
//! titles, the crystal menu, the dock, the window menus, Quarry, the file viewer and editor, Settings,
//! Activity, the console, the boot step label. The call shapes are `video::font`'s (`draw_text` for a
//! cached-RAM surface, `draw_row` for a strip painter's scanline, `draw_glyph_fb` for the console's cell,
//! `draw_with` for a painter that computes its own background), so a caller moved by changing the module
//! it names and, where it measured with `len * cell_w`, by asking [`advance`].
//!
//! THE ENGINE IS NOT HERE. What a string becomes in pixels — shaping (bidi, scripts, per-cluster fallback),
//! sizing from the cell, the byte-bounded glyph cache, quarter-pixel x origins, Skia analytic-AA coverage
//! and Skia's A8 pre-blend (contrast 0.2, gamma 1.2) — is `font_core::ui`, `no_std`, shared with the host
//! harness that measures it against Chromium. This module is the fulfiller: it reads the faces off
//! [`DIR`] once the volume is up ([`service`], chained from the desktop's service pass), owns the one
//! engine behind a lock, maps the [`Face`] a caller names to a role, size and baseline, and FALLS BACK to
//! `video::font`'s bitmap atlases — saying so on the wire — whenever no face is loaded (before the
//! volume, on a build without the desktop, when `DejaVuSans.ttf` is absent, under lock contention).
//!
//! Faces ([`Face`]):
//! * `Body` — the character grid (console, file viewer, editor, Quarry): DejaVu Sans Mono sized so its
//!   advance fits `font::CELL_W` and its line box `font::CELL_H`; the grid arithmetic of every caller holds.
//! * `Chrome` — captions, the menu bar, menus, dock tiles: the `system.display.font` family (DejaVu Sans by
//!   default, bold where the caller asks), proportional, its size DERIVED from the bar's cell as FONT-METRIC
//!   ruled for the bitmap face: the line box fits `font::CHROME_CELL_H` and the mean a..z advance of the bold
//!   face fits `font::CHROME_CELL_W`, so the `len * cell_w` budgets the bar, menus and dock were built on keep
//!   holding (14 px with DejaVu Sans Bold on the 9x20 cell).
//! * `Ui` — running text that is not a grid (the login form, Settings labels): the `system.display.font`
//!   family at `system.display.font_size` CSS px x the panel's ppi / 96 (EDID), capped by the body cell.
//! * `Grid` (KERNELFONT2, B363) — the CONSOLE's character cell: the body cell x `video::dpi`'s half-pixel scale
//!   (7x16 -> 18x40 at 2.5 on the 220/227-ppi rMBP panels), DejaVu Sans Mono at `system.display.font_size` CSS px
//!   x ppi / 96 capped by that cell — the face `font_size` takes effect on. Before the faces load the bitmap atlas
//!   is drawn at the scale's integer part, centred in the same cell, so the grid never changes under the console.
//! * `Title` (FONTSCRAFT, B640) — DESIGN-LANGUAGE §2.2's `type.title2` (Una 20/26 pt, Softy 20/28, `wght` 600,
//!   tracking -0.75 %) from `theme::type_role`, the title-size face SETTINGSCRAFT's kit lacked; its cell is the line
//!   height in px at the dpi scale.
//!
//! FONTSCRAFT (rmbp-ledger B640, R124): the faces are DESIGN-LANGUAGE §2.2's — **Inter** (variable: `wght` + `opsz`)
//! for every proportional role (`system.display.font = "sans"`, the schema default, IS Inter), **JetBrains Mono**
//! (variable `wght`) for the grid, and the fallback chain Inter → Noto Sans (variable) → DejaVu Sans → the bitmap
//! atlas. `font_core::ui` instances the variable faces per draw: `wght` 400, 600 where a caller asks bold (semibold,
//! not bold, below 20 pt), the type role's weight for `Title`; `opsz` = the size in pt (the engine's px per pt is
//! the dpi scale). A tree without the vendored faces (an older image) loads exactly KERNELFONT's DejaVu set in the
//! same roles. Witness: one `[font] face=… ` line after `[kfont] load`.

use super::font;
use super::framebuffer::FrameBuffer;

/// Where the builder stages the faces (as DATA, from the host's font packages — `builder/src/main.rs`).
pub const DIR: &str = "/system/fonts";
/// Where they are found when the UnaFS SSD is the root and the card (which the builder staged) is at `/boot`.
pub const DIR_CARD: &str = "/boot/system/fonts";

/// Which face a call draws with. `Body`/`Chrome` keep `video::font`'s cell metrics; `Ui` lays out on the
/// body cell's height with a proportional face.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Body,
    Chrome,
    Ui,
    /// KERNELFONT2: the console's dpi-scaled cell (see the module doc).
    Grid,
    /// FONTSCRAFT (B640): `type.title2` — the title-size face (see the module doc).
    Title,
}

impl Face {
    /// The bitmap atlas this face falls back to.
    #[inline(always)]
    pub const fn bitmap(self) -> font::Face {
        match self {
            Face::Chrome | Face::Title => font::Face::Chrome,
            _ => font::Face::Body,
        }
    }
    /// The layout cell's advance (the grid a caller places on; a proportional face measures with [`advance`]).
    #[inline(always)]
    pub fn cell_w(self) -> usize {
        match self {
            Face::Grid => grid_cell().0,
            Face::Chrome => chrome_cell().0, // UIMETRICS (B372): the chrome cell x the dpi scale
            Face::Ui => grid_cell().0, // UIMETRICS (B372): the native windows' text cell — the body cell x the dpi scale
            Face::Title => title_cell().0,
            _ => self.bitmap().cell_w(),
        }
    }
    /// The layout cell's height.
    #[inline(always)]
    pub fn cell_h(self) -> usize {
        match self {
            Face::Grid => grid_cell().1,
            Face::Chrome => chrome_cell().1,
            Face::Ui => grid_cell().1,
            Face::Title => title_cell().1,
            _ => self.bitmap().cell_h(),
        }
    }
    /// The face's family name for witnesses (`dejavu-sans` …) when a face is loaded, else the bitmap atlas's.
    pub fn name(self) -> &'static str {
        #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
        if let Some(n) = tt::with(|t| t.names[self.index()]) {
            return n;
        }
        self.bitmap().name()
    }
    const fn index(self) -> usize {
        match self {
            Face::Body => 0,
            Face::Chrome => 1,
            Face::Ui => 2,
            Face::Grid => 3,
            Face::Title => 4,
        }
    }
}

/// The face a surface is drawing with now, for witnesses: `dejavu-sans-17.00` style when a face is loaded,
/// else the bitmap atlas's `noto<raster>-aa`.
pub fn face_name(face: Face) -> alloc::string::String {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    if let Some(n) = tt::face_name(face) {
        return n;
    }
    alloc::string::String::from(face.bitmap().name())
}

/// A TrueType face is loaded and drawing.
pub fn active() -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    return tt::ready();
    #[allow(unreachable_code)]
    false
}

/// The desktop service pass: load the faces once the volume holds them; follow the font preferences.
pub fn service() {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    tt::service();
}

/// Blend `s` into a cached-RAM surface at `(x, y)` (the line's TOP), `video::font::draw_text`'s contract:
/// `stride` in `u32`s, `clip_w`/`clip_h` the surface's extents, a glyph that does not fit whole ENDS the
/// string, the destination is read. Returns the pen x after the last glyph drawn.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn draw_text(px: &mut [u32], stride: usize, clip_w: usize, clip_h: usize, x: usize, y: usize, s: &[u8], ink: u32, bold: bool, face: Face) -> usize {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    if let Some(p) = tt::draw_text(px, stride, clip_w, clip_h, x, y, s, ink, bold, face.index()) {
        return p;
    }
    if let Some((k, ox, oy)) = blowup(face) {
        // UIMETRICS: no face yet and a scaled cell — the atlas glyph at the scale's integer part, centred in the cell.
        let (cw, ch) = (face.cell_w(), face.cell_h());
        if y + ch > clip_h {
            return x;
        }
        let mut cx = x;
        for &b in s {
            if cx + cw > clip_w {
                break;
            }
            for (ry, row) in font::glyph(b, bold, face.bitmap()).iter().enumerate() {
                for (rx, &a) in row.iter().enumerate() {
                    if a == 0 {
                        continue;
                    }
                    for dy in 0..k {
                        let i = (y + oy + ry * k + dy) * stride + cx + ox + rx * k;
                        for dx in 0..k {
                            if let Some(p) = px.get_mut(i + dx) {
                                *p = font::blend(*p, ink, a);
                            }
                        }
                    }
                }
            }
            cx += cw;
        }
        return cx;
    }
    font::draw_text(px, stride, clip_w, clip_h, x, y, s, ink, bold, face.bitmap())
}

/// Blend scanline `sy` (`0..face.cell_h()`) of `s` into a strip painter's cached-RAM row at `x0`
/// (`video::font::draw_row`'s contract).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn draw_row(out: &mut [u32], w: usize, s: &[u8], x0: usize, sy: usize, ink: u32, bold: bool, face: Face) {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    if tt::draw_row(out, w, s, x0, sy, ink, bold, face.index()) {
        return;
    }
    if let Some((k, ox, oy)) = blowup(face) {
        // UIMETRICS: the scaled cell's row `sy` is atlas row `(sy - oy) / k`, each atlas pixel k wide.
        if sy < oy || (sy - oy) / k >= face.bitmap().cell_h() {
            return;
        }
        let (ry, cw) = ((sy - oy) / k, face.cell_w());
        for (n, &b) in s.iter().enumerate() {
            let cx = x0 + n * cw + ox;
            if let Some(row) = font::glyph(b, bold, face.bitmap()).get(ry) {
                for (rx, &a) in row.iter().enumerate() {
                    if a == 0 {
                        continue;
                    }
                    for dx in 0..k {
                        let i = cx + rx * k + dx;
                        if i < w && i < out.len() {
                            out[i] = font::blend(out[i], ink, a);
                        }
                    }
                }
            }
        }
        return;
    }
    font::draw_row(out, w, s, x0, sy, ink, bold, face.bitmap())
}

/// One character cell of a [`FrameBuffer`] at `(cx, cy)` against a KNOWN background (write-only;
/// `video::font::draw_glyph_fb`'s contract). Pixels stay inside the cell.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn draw_glyph_fb(fb: &FrameBuffer, ch: u8, cx: usize, cy: usize, ink: u32, bg: u32, bold: bool, face: Face) {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    if tt::draw_cell(ch, cx, cy, ink, bold, face.index(), &mut |x, y, a| fb.put_pixel(x, y, font::blend(bg, ink, a))) {
        return;
    }
    if let Some((k, ox, oy)) = blowup(face) {
        // KERNELFONT2 / UIMETRICS: no face yet — the atlas glyph at the scale's integer part, centred in the scaled cell.
        let (ox, oy) = (cx + ox, cy + oy);
        for (ry, row) in font::glyph(ch, bold, face.bitmap()).iter().enumerate() {
            for (rx, &a) in row.iter().enumerate() {
                if a != 0 {
                    let c = font::blend(bg, ink, a);
                    for dy in 0..k {
                        for dx in 0..k {
                            fb.put_pixel(ox + rx * k + dx, oy + ry * k + dy, c);
                        }
                    }
                }
            }
        }
        return;
    }
    font::draw_glyph_fb(fb, ch, cx, cy, ink, bg, bold, face.bitmap())
}

/// For painters that compute each pixel's background themselves (window captions, the boot label): every
/// covered pixel of `s` with its line top at `(x, y)` goes to `put(px, py, coverage)`; glyphs end at
/// `x + max_w` (all-or-nothing). Returns the pen x after the last glyph drawn.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn draw_with(s: &[u8], bold: bool, face: Face, x: usize, y: isize, max_w: usize, ink: u32, put: &mut dyn FnMut(isize, isize, u8)) -> usize {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    if let Some(p) = tt::draw_with(s, bold, face.index(), x, y, max_w, ink, put) {
        return p;
    }
    let (bf, cw) = (face.bitmap(), face.cell_w());
    let (k, ox, oy) = blowup(face).unwrap_or((1, 0, 0)); // UIMETRICS: the atlas glyph blown up inside a scaled cell
    let mut cx = x;
    for &b in s {
        if cx + cw > x + max_w {
            break;
        }
        for (ry, row) in font::glyph(b, bold, bf).iter().enumerate() {
            for (rx, &a) in row.iter().enumerate() {
                if a != 0 {
                    for dy in 0..k {
                        for dx in 0..k {
                            put((cx + ox + rx * k + dx) as isize, y + (oy + ry * k + dy) as isize, a);
                        }
                    }
                }
            }
        }
        cx += cw;
    }
    cx
}

/// UIMETRICS (B372): the chrome cell (captions, the bar, the crystal menu, dock labels) — the chrome atlas cell x
/// `video::dpi`'s scale, each side rounded up (9x20 -> 23x50 at 2.5). `wm::TITLE_CELL_W/H` are this.
pub fn chrome_cell() -> (usize, usize) {
    let s2 = crate::video::dpi::s2();
    (crate::video::dpi::px_at(font::CHROME_CELL_W, s2), crate::video::dpi::px_at(font::CHROME_CELL_H, s2))
}

/// FONTSCRAFT (B640): the title face's cell — the chrome cell's width (a proportional face measures with
/// [`advance`]) by `type.title2`'s line height in px at the dpi scale (26 pt -> 52 px at 2.0 in the Una look).
pub fn title_cell() -> (usize, usize) {
    let st = crate::video::theme::type_role(crate::video::theme::TypeRole::Title2, crate::video::look::is_softy());
    let s2 = crate::video::dpi::s2();
    (chrome_cell().0, crate::video::dpi::px_at(st.line_pt as usize, s2))
}

/// UIMETRICS: how the BITMAP fallback draws `face` inside its scaled cell — `(k, ox, oy)`: the atlas glyph
/// magnified by the scale's integer part `k`, offset to the cell's centre. `None` when the face's cell IS the
/// atlas cell (scale 1.0, or a face that does not scale): draw the atlas as it is.
fn blowup(face: Face) -> Option<(usize, usize, usize)> {
    let bf = face.bitmap();
    let (cw, ch) = (face.cell_w(), face.cell_h());
    if (cw, ch) == (bf.cell_w(), bf.cell_h()) {
        return None;
    }
    let k = (crate::video::dpi::s2() as usize / 2).max(1);
    Some((k, cw.saturating_sub(bf.cell_w() * k) / 2, ch.saturating_sub(bf.cell_h() * k) / 2))
}

/// The width `s` takes in `face`, in whole px (the shaped advance when a face is loaded, else
/// `len * cell_w`). What a caller that centres, right-aligns or places a caret measures with.
#[inline]
pub fn advance(s: &[u8], bold: bool, face: Face) -> usize {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    if let Some(w) = tt::advance(s, bold, face.index()) {
        return w;
    }
    s.len() * face.cell_w()
}

/// How many leading bytes of `s` fit in `max_w` px (a whole-glyph truncation for callers that cut).
pub fn fit(s: &[u8], bold: bool, face: Face, max_w: usize) -> usize {
    if advance(s, bold, face) <= max_w {
        return s.len();
    }
    let mut n = s.len();
    while n > 0 && advance(&s[..n], bold, face) > max_w {
        n -= 1;
    }
    n
}

/// KERNELFONT2 M4: bumped when the faces load and on every restyle (`system.display.font` / `font_size`); the
/// desktop service pass (`quarry::live::service`) compares it with what it last saw and has every kernel window that
/// caches its own pixels repaint once ([`epoch`]).
static EPOCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The face epoch (0 = no face has loaded yet).
pub fn epoch() -> u32 {
    EPOCH.load(core::sync::atomic::Ordering::Acquire)
}

/// KERNELFONT2: the console's cell — the body cell x `video::dpi`'s scale, each side rounded up to the pixel;
/// UIMETRICS (B372): and x `font_size / 13` when `system.display.font_size` is not the default, so the console
/// REGRIDS on a restyle (the face then fills the cell it is sized for instead of being capped by the 13-px one).
pub fn grid_cell() -> (usize, usize) {
    grid_cell_at(crate::video::dpi::scale_x2())
}

/// The CSS px the grid cell is sized for (0 = the default 13), written by the restyle under the engine's lock.
static GRID_CSS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// KERNELFONT2's default `font_size` — the size the 18x40 (at 2.5) cell was built for.
pub const GRID_BASE_CSS: u32 = 13;

fn grid_cell_at(s2: u32) -> (usize, usize) {
    let fs = match GRID_CSS.load(core::sync::atomic::Ordering::Relaxed) {
        0 => GRID_BASE_CSS,
        f => f,
    } as usize; grid_cell_for(fs as u32, s2) } fn grid_cell_for(css: u32, s2: u32) -> (usize, usize) { let fs = css as usize; // TEXTSCALE (B813): split IN PLACE — `grid_cell_for` is the one cell formula at any css (the text-size cap asks it at each candidate); LINE-NEUTRAL fold, code first
    if fs == GRID_BASE_CSS as usize {
        return (crate::video::dpi::px_at(font::CELL_W, s2), crate::video::dpi::px_at(font::CELL_H, s2));
    }
    let k = 2 * GRID_BASE_CSS as usize;
    ((font::CELL_W * s2 as usize * fs).div_ceil(k).max(1), (font::CELL_H * s2 as usize * fs).div_ceil(k).max(1))
}

/// KERNELFONT2: arm the console's grid on a framebuffer `fb_w` px wide — latches `video::dpi`'s scale (first call
/// wins), returns [`grid_cell`]. Called by `fbcon` where the console takes the face.
pub fn arm_grid(fb_w: usize) -> (usize, usize) {
    let s2 = crate::video::dpi::latch(fb_w);
    grid_cell_at(s2)
}

/// The engine half, compiled only where a desktop exists (no other image links `font_core`).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
mod tt {
    use super::font;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use font_core::ui::{device_px, edid_ppi, Engine, Role, Style, DEFAULT_CSS_PX};
    use crate::video::theme::{type_role, TypeRole};
    use font_core::Font;

    /// (file, wire name, role, bold). FONTSCRAFT (B640): Inter and JetBrains Mono are the Sans and Mono primaries;
    /// the fallback chain after them is this table's ORDER (GLYPHS, B676): Noto Sans, the Noto script faces, DejaVu. The DejaVu Sans / Sans Mono rows are the
    /// STAND-INS: when Inter (JetBrains Mono) did not load they take its role, as KERNELFONT had them, and a DejaVu
    /// mono face is read only then. A sans face (Inter or DejaVu Sans) is REQUIRED: without one the desktop stays on
    /// the bitmap face.
    pub const FACES: &[(&str, &str, Role, bool)] = &[
        ("InterVariable.ttf", "inter", Role::Sans, false),
        ("JetBrainsMono-Variable.ttf", "jetbrains-mono", Role::Mono, false),
        ("NotoSans-Variable.ttf", "noto-sans", Role::Fallback, false),
        ("NotoSansArabic-Regular.ttf", "noto-arabic", Role::Script, false),
        ("NotoSansHebrew-Regular.ttf", "noto-hebrew", Role::Script, false),
        ("NotoSansDevanagari-Regular.ttf", "noto-devanagari", Role::Script, false),
        ("NotoSansThai-Regular.ttf", "noto-thai", Role::Script, false),
        ("DejaVuSans.ttf", "dejavu-sans", Role::Fallback, false),
        ("DejaVuSans-Bold.ttf", "dejavu-sans-bold", Role::Fallback, true),
        ("DejaVuSansMono.ttf", "dejavu-mono", Role::Mono, false),
        ("DejaVuSansMono-Bold.ttf", "dejavu-mono-bold", Role::Mono, true),
        ("DejaVuSerif.ttf", "dejavu-serif", Role::Serif, false),
        ("DejaVuSerif-Bold.ttf", "dejavu-serif-bold", Role::Serif, true),
    ];

    /// `system.display.font` / `system.display.font_size` (Principia schema rows, KERNELFONT M3).
    const FONT_KEY: &str = "display.font";
    const FONT_SIZE_KEY: &str = "display.font_size";
    const RETRY_MS: u64 = 2000;
    const MAX_TRIES: u32 = 15;
    const PREFS_MS: u64 = 1000;
    /// A face file larger than this is refused (Inter's variable face is 880 KB, DejaVu Sans 760 KB).
    const FACE_MAX: usize = 4 * 1024 * 1024;
    /// Bounded wait for the engine lock before a call draws with the bitmap face instead.
    const SPIN: u32 = 1 << 14;

    pub struct Tt {
        pub eng: Engine<'static>,
        /// Per [`super::Face`] index: style and the baseline row inside the face's cell.
        pub styles: [(Style, f32); 5],
        pub names: [&'static str; 5],
        pub family: Role,
        pub css_px: i64, pub cap: i64, // TEXTSCALE (B813): the css size the panel holds (`textscale::cap_now`, computed outside this lock); the faces size from `css_px.min(cap)`. LINE-NEUTRAL fold
        pub ppi: u32,
        pub font_bytes: usize,
        /// UI faces (sans / mono / serif) that did not load — what `fallback=` names.
        pub missing: String,
        /// Script fallback faces (Noto) that did not load: those scripts draw as .notdef boxes.
        pub scripts_missing: String,
    }

    static TT: crate::sync::Mutex<Option<Tt>> = crate::sync::Mutex::new(None);
    static READY: AtomicBool = AtomicBool::new(false);
    static GAVE_UP: AtomicBool = AtomicBool::new(false);
    static TRIES: AtomicU32 = AtomicU32::new(0);
    static NEXT_TRY: AtomicU64 = AtomicU64::new(0);
    pub static NEXT_PREFS: AtomicU64 = AtomicU64::new(0);
    pub static CONTENDED: AtomicU64 = AtomicU64::new(0);

    pub fn ready() -> bool {
        READY.load(Ordering::Acquire)
    }

    /// Run `f` on the engine, or `None` (no face loaded, or the lock stayed busy for [`SPIN`] tries — that
    /// call then draws with the bitmap face; counted in [`CONTENDED`]).
    pub fn with<R>(f: impl FnOnce(&mut Tt) -> R) -> Option<R> {
        if !READY.load(Ordering::Acquire) {
            return None;
        }
        let mut n = 0;
        let mut g = loop {
            if let Some(g) = TT.try_lock() {
                break g;
            }
            n += 1;
            if n >= SPIN {
                CONTENDED.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            core::hint::spin_loop();
        };
        g.as_mut().map(f)
    }

    pub fn face_name(face: super::Face) -> Option<String> {
        with(|t| {
            let (st, _) = t.styles[face.index()];
            alloc::format!("{}-{:.2}", t.names[face.index()], st.size)
        })
    }

    /// The layout cell of face index `face` (KERNELFONT2: index 3 is the console's dpi-scaled grid).
    fn cell(face: usize) -> (usize, usize) {
        match face {
            1 => super::chrome_cell(), // UIMETRICS (B372)
            2 | 3 => super::grid_cell(), // UIMETRICS: Ui lays out on the dpi-scaled cell (the native windows)
            4 => super::title_cell(), // FONTSCRAFT (B640)
            _ => (font::CELL_W, font::CELL_H),
        }
    }

    fn style(t: &Tt, face: usize, bold: bool, s: &[u8]) -> (Style, f32) {
        let (mut st, bl) = t.styles[face];
        st.bold = bold; if super::tabular(s) { st.features |= font_core::ui::FEAT_TNUM; } // FONTS2 (B650): a run that OPENS with a figure is a readout (the clock, a meter, a percentage) and draws with tabular figures, so its width holds while its digits change (DESIGN-LANGUAGE §2.2: "Figures that change in place … use `tnum`"); LINE-NEUTRAL fold, code first
        (st, bl)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(px: &mut [u32], stride: usize, clip_w: usize, clip_h: usize, x: usize, y: usize, s: &[u8], ink: u32, bold: bool, face: usize) -> Option<usize> {
        with(|t| {
            let cell_h = cell(face).1;
            if y + cell_h > clip_h {
                return x;
            }
            let (st, bl) = style(t, face, bold, s);
            let pen = t.eng.draw_0rgb(px, stride, clip_w, clip_h, x as f32, y as f32 + bl, s, st, ink);
            x + font_core::fmath::ceil(pen) as usize
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_row(out: &mut [u32], w: usize, s: &[u8], x0: usize, sy: usize, ink: u32, bold: bool, face: usize) -> bool {
        with(|t| {
            let cell_h = cell(face).1;
            if sy >= cell_h {
                return;
            }
            let (st, bl) = style(t, face, bold, s);
            let row = sy as i32;
            t.eng.draw_with(s, st, x0 as f32, bl, None, Some(row), ink, &mut |gx, gy, a| {
                if gy == row && gx >= 0 && (gx as usize) < w && (gx as usize) < out.len() {
                    out[gx as usize] = font_core::ui::blend(out[gx as usize], ink, a);
                }
            });
        })
        .is_some()
    }

    pub fn draw_cell(ch: u8, cx: usize, cy: usize, ink: u32, bold: bool, face: usize, put: &mut dyn FnMut(usize, usize, u8)) -> bool {
        with(|t| {
            let (cw, chh) = cell(face);
            let (st, bl) = style(t, face, bold, &[]);
            let c = if (0x20..0x7f).contains(&ch) { ch as char } else { ' ' };
            let (x0, y0) = (cx as i32, cy as i32);
            let (x1, y1) = (x0 + cw as i32, y0 + chh as i32);
            t.eng.draw_char_with(c, st, cx as f32, cy as f32 + bl, ink, &mut |gx, gy, a| {
                if gx >= x0 && gx < x1 && gy >= y0 && gy < y1 {
                    put(gx as usize, gy as usize, a);
                }
            });
        })
        .is_some()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_with(s: &[u8], bold: bool, face: usize, x: usize, y: isize, max_w: usize, ink: u32, put: &mut dyn FnMut(isize, isize, u8)) -> Option<usize> {
        with(|t| {
            let (st, bl) = style(t, face, bold, s);
            let pen = t.eng.draw_with(s, st, x as f32, y as f32 + bl, Some((x + max_w) as f32), None, ink, &mut |gx, gy, a| put(gx as isize, gy as isize, a));
            x + font_core::fmath::ceil(pen) as usize
        })
    }

    pub fn advance(s: &[u8], bold: bool, face: usize) -> Option<usize> {
        with(|t| {
            let (st, _) = style(t, face, bold, s);
            font_core::fmath::ceil(t.eng.measure(s, st)) as usize
        })
    }

    /// The panel's ppi from its EDID (0 = no trustworthy EDID; sizes then take 96).
    pub fn panel_ppi() -> u32 {
        crate::video::edid_block().and_then(|b| edid_ppi(&b)).map_or(0, |(_, _, p)| p)
    }

    fn family_of(s: Option<String>) -> Role {
        match s.as_deref() {
            Some("serif") => Role::Serif,
            Some("mono") => Role::Mono,
            _ => Role::Sans,
        }
    }

    fn family_name(r: Role) -> &'static str {
        match r {
            Role::Serif => "serif",
            Role::Mono => "mono",
            _ => "sans",
        }
    }

    /// Size the three faces: Body fits the console cell (mono); Chrome fits the bar's cell; Ui is the
    /// preference (CSS px x ppi / 96), capped by the body cell.
    pub fn restyle(t: &mut Tt) {
        // FONTSCRAFT (B640): a variable face's `opsz` is the size in pt — device px over the dpi scale.
        let s2 = crate::video::dpi::s2().max(1);
        t.eng.set_px_per_pt(s2 as f32 / 2.0); t.eng.bold_weight = type_role(TypeRole::BodyStrong, crate::video::look::is_softy()).weight; // FONTKIT (B730): a `bold` run (the bar, menus, window titles) draws `type.body-strong`'s `wght` from theme's ONE table (600, the engine's former constant); LINE-NEUTRAL fold, code first
        let eng = &t.eng;
        let fam = if eng.has(t.family) { t.family } else { Role::Sans };
        let mono = if eng.has(Role::Mono) { Role::Mono } else { Role::Sans };
        let body = eng.fit_size(mono, Some(font::CELL_W as f32), font::CELL_H as f32).unwrap_or(11.0);
        let (ccw, cch) = super::chrome_cell(); // UIMETRICS (B372): the chrome face fits the dpi-scaled chrome cell
        let chrome = eng.fit_size_mean(fam, true, ccw as f32, cch as f32).unwrap_or(14.0);
        // KERNELFONT2: the console grid — mono at font_size x ppi / 96, capped by the dpi-scaled cell; UIMETRICS: the
        // cell itself follows font_size (the console regrids, `fbcon::regrid`, after this restyle lands).
        super::GRID_CSS.store(t.css_px.min(t.cap).clamp(1, 64) as u32, Ordering::Relaxed);
        let (gw, gh) = super::grid_cell();
        // UIMETRICS (B372): the UI face (the native kernel windows) is font_size x ppi / 96 capped by the dpi-scaled
        // cell's height — 40 px at 2.5, so the default 13 CSS px draws its 29.9 px (KERNELFONT's 13.5-px cap is gone).
        let ui_fit = eng.fit_size(fam, None, gh as f32).unwrap_or(12.0);
        let ui = device_px(t.css_px.min(t.cap) as f32, t.ppi).min(ui_fit).max(6.0);
        let grid_fit = eng.fit_size(mono, Some(gw as f32), gh as f32).unwrap_or(body);
        let grid = device_px(t.css_px.min(t.cap) as f32, t.ppi).min(grid_fit).max(6.0);
        let mk = |role: Role, size: f32, cell: usize| (Style::new(role, false, size), eng.baseline_in_cell(role, size, cell as f32));
        // FONTSCRAFT (B640): the title face — `type.title2` of the look on the glass, in px at the dpi scale.
        let t2 = type_role(TypeRole::Title2, crate::video::look::is_softy());
        let (title_px, title_line) = (t2.size_pt as f32 * s2 as f32 / 2.0, super::title_cell().1);
        let title = (
            Style::new(fam, true, title_px).with_axes(t2.axes().0, t2.axes().1).with_tracking(t2.tracking_em()), // FONTKIT (B730): the role's (wght, opsz) from theme's ONE table
            eng.baseline_in_line(fam, title_px, title_line as f32),
        );
        t.styles = [mk(mono, body, font::CELL_H), mk(fam, chrome, cch), mk(fam, ui, gh), mk(mono, grid, gh), title];
        let nm = |r: Role| eng.primary_name(r, false).unwrap_or("none");
        t.names = [nm(mono), nm(fam), nm(fam), nm(mono), nm(fam)]; let _ = crate::video::theme::text_restyled(crate::video::metrics::to_logical(gh), t.cap.clamp(0, 64) as u32); // TEXTSCALE (B813): publish the UI line (the capped grid cell) for the layouts that grow with it, under this lock and before the caller bumps `text::epoch`. LINE-NEUTRAL fold
    }

    pub fn read_face(mt: &crate::fs::vfs::MountTable, p: &str) -> Result<Vec<u8>, &'static str> {
        let st = mt.stat(p).map_err(|_| "absent")?;
        let n = st.size as usize;
        if n == 0 || n > FACE_MAX {
            return Err("size");
        }
        let mut v = Vec::new();
        v.try_reserve_exact(n).map_err(|_| "heap")?;
        while v.len() < n {
            let want = (n - v.len()).min(256 * 1024);
            let b = mt.read(p, v.len() as u64, want).map_err(|_| "read")?;
            if b.is_empty() {
                return Err("short");
            }
            v.extend_from_slice(&b[..b.len().min(n - v.len())]);
        }
        Ok(v)
    }

    pub fn service() {
        if READY.load(Ordering::Acquire) {
            follow_prefs();
            return;
        }
        if GAVE_UP.load(Ordering::Relaxed) {
            return;
        }
        let now = crate::arch::ms();
        if now < NEXT_TRY.load(Ordering::Relaxed) {
            return;
        }
        NEXT_TRY.store(now + RETRY_MS, Ordering::Relaxed);
        let n = TRIES.fetch_add(1, Ordering::Relaxed) + 1;
        let mt = crate::shell::vfs_mount_table();
        // FONTSCRAFT (B640): the volume holds Inter, or (an older image) DejaVu Sans.
        let has = |d: &str, f: &str| mt.stat(&alloc::format!("{}/{}", d, f)).is_ok();
        let Some(dir) = [super::DIR, super::DIR_CARD].into_iter().find(|d| has(d, "InterVariable.ttf") || has(d, "DejaVuSans.ttf")) else {
            if n >= MAX_TRIES {
                GAVE_UP.store(true, Ordering::Relaxed);
                serial_println!("[kfont] load faces=0/{} fallback=bitmap reason=absent path={}/InterVariable.ttf tries={} (KERNELFONT: the desktop stays on the noto bitmap atlases)", FACES.len(), super::DIR, n);
                serial_println!("[font] face=font8x8 reason=absent chain=font8x8 (FONTSCRAFT: no Inter, Noto Sans or DejaVu on the volume)");
            }
            return;
        };
        load(&mt, dir, now);
    }

    fn load(mt: &crate::fs::vfs::MountTable, dir: &str, t0: u64) {
        let cap = (crate::allocator::HEAP_SIZE / 128).clamp(512 * 1024, 2 * 1024 * 1024);
        let mut eng = Engine::new(cap);
        let mut missing = String::new();
        let mut scripts_missing = String::new();
        let mut bytes = 0usize;
        // FONTSCRAFT (B640): which primaries loaded decides the stand-ins' roles.
        let (mut have_inter, mut have_jb) = (false, false);
        for &(file, name, role0, bold) in FACES {
            let mut role = role0;
            if name.starts_with("dejavu-sans") && !have_inter {
                role = Role::Sans; // the KERNELFONT sans stands in for Inter
            }
            if name.starts_with("dejavu-mono") && have_jb {
                continue; // JetBrains Mono holds the grid: the DejaVu mono faces are not read
            }
            let p = alloc::format!("{}/{}", dir, file);
            let why = match read_face(mt, &p) {
                Ok(v) => {
                    let len = v.len();
                    let data: &'static [u8] = alloc::boxed::Box::leak(v.into_boxed_slice());
                    match Font::parse(data) {
                        Ok(f) => { #[cfg(feature = "selfdiag")] super::resident_note(&p, data); // LUMENFAST (B507): the parsed face's bytes, served to ring 3's SYS_PATH_READ of the same path; folded onto this line, code before comment, so no line below moves
                            have_inter |= name == "inter";
                            have_jb |= name == "jetbrains-mono";
                            eng.add_face(name, role, bold, f);
                            bytes += len;
                            None
                        }
                        Err(_) => Some("parse"),
                    }
                }
                Err(e) => Some(e),
            };
            if let Some(w) = why {
                let m = if role == Role::Script { &mut scripts_missing } else { &mut missing };
                if !m.is_empty() {
                    m.push('+');
                }
                m.push_str(name);
                serial_println!("[kfont] face {} {} reason={}", name, p, w);
            }
        }
        if !eng.has(Role::Sans) {
            GAVE_UP.store(true, Ordering::Relaxed);
            serial_println!("[kfont] load faces={}/{} fallback=bitmap reason=no-sans missing={}", eng.face_count(), FACES.len(), missing);
            return;
        }
        let ppi = match crate::video::dpi::ppi() {
            0 => panel_ppi(),
            p => p, // KERNELFONT2: the framebuffer's effective ppi (EDID native x fb width / native width)
        };
        let family = family_of(crate::prefs::text(FONT_KEY));
        let css_px = crate::prefs::int(FONT_SIZE_KEY, 9, 32).unwrap_or(DEFAULT_CSS_PX);
        let n = eng.face_count();
        let mut t = Tt { eng, styles: [(Style::new(Role::Sans, false, 12.0), 12.0); 5], names: ["", "", "", "", ""], family, css_px, cap: super::textscale::cap_now(), ppi, font_bytes: bytes, missing, scripts_missing };
        restyle(&mut t);
        let fallback = if t.missing.is_empty() { String::from("none") } else { t.missing.clone() };
        let s2 = crate::video::dpi::scale_x2();
        let (gw, gh) = super::grid_cell();
        let (pw, ph) = crate::video::panel_info_nonblocking().map_or((0, 0), |i| (i.width, i.height));
        serial_println!(
            "[kfont] load dir={} faces={}/{} fallback={} scripts_missing={} font_kib={} cache_kib={} ppi={} scale={} cell={}x{} grid={}x{} panel={}x{} font={} font_size={} body={}-{:.2} chrome={}-{:.2} ui={}-{:.2} console={}-{:.2} ms={}",
            dir, n, FACES.len(), fallback, if t.scripts_missing.is_empty() { "none" } else { t.scripts_missing.as_str() }, bytes / 1024, cap / 1024, ppi,
            crate::video::dpi::scale_str(s2), gw, gh, pw / gw.max(1), ph / gh.max(1), pw, ph,
            family_name(family), css_px, t.names[0], t.styles[0].0.size, t.names[1], t.styles[1].0.size, t.names[2], t.styles[2].0.size, t.names[3], t.styles[3].0.size,
            crate::arch::ms().saturating_sub(t0)
        );
        font_witness(&t, have_inter, have_jb); // FONTSCRAFT (B640): `[font] face=Inter …`
        crate::video::edidsrc::witness(ppi); // KFONTPPI (B382): edid_src, ppi, scale, cell, grid — one line
        *TT.lock() = Some(t);
        READY.store(true, Ordering::Release);
        regrid_console(); // UIMETRICS (B372): a non-default font_size at load regrids the console too
        super::EPOCH.fetch_add(1, Ordering::AcqRel); // KERNELFONT2 M4: the windows painted before this repaint once
        let _ = crate::video::wm::damage_intersecting(0, 0, 1 << 16, 1 << 16);
    }

    /// FONTSCRAFT (B640): the face the desktop draws with, its variable instance at the UI size, the mono face, the
    /// fallback chain as loaded and the title role — one line:
    /// `[font] face=Inter file=InterVariable.ttf axes=opsz:14-32,wght:100-900 ui=<px> wght=400 opsz=<pt> bold_wght=600
    /// mono=jetbrains-mono chain=inter>noto-sans>dejavu-sans>font8x8 title=<px>/<line>@600 look=una base_pt=13
    /// features=kern,liga,calt+tnum,zero,ss01,cv01-on-request atlases=per-role`.
    fn font_witness(t: &Tt, have_inter: bool, have_jb: bool) {
        let eng = &t.eng;
        let ui = t.styles[2].0;
        let face = if have_inter { "Inter" } else { "DejaVu" };
        let file = if have_inter { "InterVariable.ttf" } else { "DejaVuSans.ttf" };
        let inst = eng.primary_instance(&ui);
        let axes: String = inst
            .map(|f| {
                f.axes()
                    .iter()
                    .map(|a| alloc::format!("{}:{}-{}", core::str::from_utf8(&a.tag).unwrap_or("?"), a.min as i32, a.max as i32))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let val = |tag: &[u8; 4]| inst.and_then(|f| f.variation_value(*tag)).map_or(String::from("-"), |v| alloc::format!("{:.1}", v));
        let mut chain: Vec<&str> = Vec::new();
        for s in eng.slots() {
            if (s.role == Role::Sans || s.role == Role::Fallback) && !s.bold && !chain.contains(&s.name) {
                chain.push(s.name);
            }
        }
        chain.push("font8x8");
        let softy = crate::video::look::is_softy();
        let base = if softy { crate::video::theme::TYPE_BASE_PT.1 } else { crate::video::theme::TYPE_BASE_PT.0 };
        let (ts, tl) = (t.styles[4].0, super::title_cell().1);
        serial_println!(
            "[font] face={} file={} axes={} ui={:.2} wght={} opsz={} bold_wght={} mono={} chain={} title={:.1}/{}@{} look={} base_pt={} features=kern,liga,calt+tnum,zero,ss01,ss02,cv01,cv05,cv08,case-on-request atlases=per-role reason={}",
            face, file, if axes.is_empty() { "none" } else { axes.as_str() }, ui.size, val(b"wght"), val(b"opsz"), eng.bold_weight,
            eng.primary_name(Role::Mono, false).unwrap_or("none"), chain.join(">"), ts.size, tl, ts.weight,
            if softy { "softy" } else { "una" }, base,
            match (have_inter, have_jb) { (true, true) => "ok", (true, false) => "jetbrains-absent", (false, true) => "inter-absent", _ => "inter-absent+jetbrains-absent" }
        );
    }

    /// Re-size when `system.display.font` / `font_size` change (checked once a second off the paint paths).
    fn follow_prefs() {
        let now = crate::arch::ms();
        if now < NEXT_PREFS.load(Ordering::Relaxed) {
            return;
        }
        NEXT_PREFS.store(now + PREFS_MS, Ordering::Relaxed);
        let family = family_of(crate::prefs::text(FONT_KEY));
        let css_px = super::textscale::override_or(crate::prefs::int(FONT_SIZE_KEY, 9, 32).unwrap_or(DEFAULT_CSS_PX)); let cap = super::textscale::cap_now(); // TEXTSCALE (B813): the fixture's css override and the panel's cap, both read outside the lock. LINE-NEUTRAL fold
        let changed = with(|t| {
            if t.family == family && t.css_px == css_px && t.cap == cap {
                return None;
            }
            t.family = family;
            t.css_px = css_px; t.cap = cap;
            restyle(t);
            Some(alloc::format!("font={} font_size={} ppi={} chrome={}-{:.2} ui={}-{:.2} console={}-{:.2} metrics=row:{} bar:{} cap={} eff={}", family_name(family), css_px, t.ppi, t.names[1], t.styles[1].0.size, t.names[2], t.styles[2].0.size, t.names[3], t.styles[3].0.size, crate::video::settings::textscale::wire().0, crate::video::settings::textscale::wire().1, t.cap, t.css_px.min(t.cap)))
        })
        .flatten();
        if let Some(line) = changed {
            serial_println!("[kfont] restyle {}", line);
            regrid_console(); // UIMETRICS (B372): the console's cell follows font_size
            super::EPOCH.fetch_add(1, Ordering::AcqRel); // KERNELFONT2 M4
            let _ = crate::video::wm::damage_intersecting(0, 0, 1 << 16, 1 << 16);
        }
    }

    /// UIMETRICS (B372): re-derive the console's grid from the restyled cell and say so (no lock held here).
    /// PREFSUI (B389): the UI scale moved live (`dpi::set_live`) — the faces re-size to the scale's effective ppi,
    /// the console regrids, every window repaints once. `true` when the faces moved.
    pub fn rescale() -> bool {
        let ppi = crate::video::dpi::ppi(); let cap = super::textscale::cap_now(); // TEXTSCALE (B813): the scale moved, so the cap does; read outside the lock. LINE-NEUTRAL fold
        let moved = with(|t| {
            if ppi == 0 || t.ppi == ppi {
                return false;
            }
            t.ppi = ppi; t.cap = cap;
            restyle(t);
            true
        })
        .unwrap_or(false);
        if moved {
            serial_println!("[kfont] rescale ppi={} scale={}", ppi, crate::video::dpi::scale_str(crate::video::dpi::s2()));
            regrid_console();
            super::EPOCH.fetch_add(1, Ordering::AcqRel);
            let _ = crate::video::wm::damage_intersecting(0, 0, 1 << 16, 1 << 16);
        }
        moved
    }

    fn regrid_console() {
        if let Some((oc, or, nc, nr)) = crate::video::fbcon::regrid() {
            let (gw, gh) = super::grid_cell();
            serial_println!("[kfont] regrid console cell={}x{} grid={}x{} was={}x{}", gw, gh, nc, nr, oc, or);
        }
    }

    /// The figures `tests font` prints.
    pub fn stats() -> Option<(usize, font_core::ui::Stats, String, usize)> {
        with(|t| (t.eng.face_count(), t.eng.stats, t.missing.clone(), t.font_bytes))
    }
}

/// `tests font` (KERNELFONT M4): draw a fixed paragraph through the seam into a scratch surface, time
/// 1000 glyphs, check the coverage is anti-aliased and inked, and print the row's witness:
/// `:: KERNELFONT: faces=<n> cache_kib=<n> evictions=<n> fallback=none glyphs_drawn=<n> ms_per_1000=<n> -> PASS ::`.
pub fn fixture() {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        tt::service();
        const W: usize = 480;
        const H: usize = 64;
        let mut surf = alloc::vec![crate::video::theme::fixture::WHITE; W * H];
        let lines: [&[u8]; 4] = [b"The quick brown fox jumps over the lazy dog 0123456789", b"UnaOS - Log In - Password - Settings", "Ελληνικά Кириллица".as_bytes(), "مرحبا שלום नमस्ते สวัสดี".as_bytes()];
        let t0 = crate::arch::ms();
        let before = tt::stats().map_or(0, |s| s.1.glyphs_drawn);
        let mut drawn = 0u64;
        while drawn < 1000 {
            for (i, l) in lines.iter().enumerate() {
                surf.iter_mut().for_each(|p| *p = crate::video::theme::fixture::WHITE);
                let _ = draw_text(&mut surf, W, W, H, 4, 4 + (i % 2) * 20, l, crate::video::theme::fixture::INK, false, Face::Ui);
            }
            let now = tt::stats().map_or(0, |s| s.1.glyphs_drawn);
            if now == before {
                break; // the bitmap face: no engine counter moves
            }
            drawn = now - before;
        }
        let ms = crate::arch::ms().saturating_sub(t0);
        let (mut mid, mut ink) = (false, false);
        for &p in surf.iter() {
            let g = p & 0xFF;
            if g <= 0x60 {
                ink = true; // a stem: at or near the ink (0x20)
            } else if g != 0xFF {
                mid = true; // partial coverage: anti-aliased
            }
        }
        let ms_per_1000 = if drawn > 0 { ms * 1000 / drawn } else { 0 };
        match tt::stats() {
            Some((faces, st, missing, bytes)) => {
                let fallback = if missing.is_empty() { alloc::string::String::from("none") } else { missing };
                let ok = faces >= 1 && fallback == "none" && drawn >= 1000 && mid && ink;
                serial_println!(
                    "[kfont] fixture font_kib={} cache_bytes={} hits={} misses={} flushes={} runs_shaped={} contended={} aa_mid={} ink={} body={} chrome={} ui={} console={}",
                    bytes / 1024, st.cache_bytes, st.hits, st.misses, st.flushes, st.runs_shaped, tt::CONTENDED.load(core::sync::atomic::Ordering::Relaxed), mid as u8, ink as u8,
                    face_name(Face::Body), face_name(Face::Chrome), face_name(Face::Ui), face_name(Face::Grid)
                );
                serial_println!(
                    ":: KERNELFONT: faces={} cache_kib={} evictions={} fallback={} glyphs_drawn={} ms_per_1000={} -> {} ::",
                    faces, st.cap_bytes / 1024, st.evictions, fallback, st.glyphs_drawn, ms_per_1000, if ok { "PASS" } else { "FAIL" }
                );
            }
            None => serial_println!(
                ":: KERNELFONT: faces=0 cache_kib=0 evictions=0 fallback=bitmap glyphs_drawn=0 ms_per_1000=0 -> FAIL :: (no face loaded from {}: see the [kfont] load line)",
                DIR
            ),
        }
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    serial_println!(":: KERNELFONT: faces=0 cache_kib=0 evictions=0 fallback=bitmap glyphs_drawn=0 ms_per_1000=0 -> SKIP :: (no desktop on this build)");
}

/// Register `tests font` once (called from `tests::shell_verb`).
pub fn ensure_tests() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("font", fixture); #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] crate::tests::register("fonts", fonts2_fixture); #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] crate::tests::register("glyphs", glyphs_fixture); // GLYPHS (B676): `tests glyphs` — the four scripts, RTL, the pseudo-locale pass (LINE-NEUTRAL append, before this comment). FONTS2 (B650): `tests fonts` — tnum, MVAR, FeatureVariations, the vendored KAT; desktop builds only, body at the FILE TAIL (LINE-NEUTRAL append, before this comment)
    }
}

/// KERNELFONT2 (B363): the directory (`DIR` or `DIR_CARD`) holding face `file`, read whole and parsed by
/// `font_core` — what a ring-3 reader of the same volume (LUMEN.ELF) will find. `None` on a build without the
/// desktop engine, or when the face is absent or refused.
pub fn volume_face(file: &str) -> Option<&'static str> {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        let mt = crate::shell::vfs_mount_table();
        for d in [DIR, DIR_CARD] {
            let p = alloc::format!("{}/{}", d, file);
            if let Ok(v) = tt::read_face(&mt, &p) {
                if font_core::Font::parse(&v).is_ok() {
                    return Some(d);
                }
            }
        }
        None
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        let _ = file;
        None
    }
}

/// PREFSUI (B389, R93): the UI scale moved live — re-size the faces to it (no engine: nothing). `true` = moved.
pub fn rescale() -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    return tt::rescale();
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    false
}

// ── LUMENFAST (rmbp-ledger B507) — the faces the kernel holds, served to ring 3's reads of the same path ──────────
// Flight 27: LUMEN.ELF spent 12.5-15.7 s of its start (`font_ms`) reading the three DejaVu faces over
// `SYS_PATH_READ` in 32 KiB steps, each step a fresh mount table and a UnaFS walk — while the kernel had held the
// very same bytes since `[kfont] load` (KERNELFONT, the `Box::leak`ed buffers `font_core` parsed). `tt::load`
// notes each face it parsed here; `selfdiag::path_fulfil` (the kernel FULFILLING the volume's read verb) answers a
// read of that exact path from this copy. One store: the file as the kernel read it; no second font engine (ring 3
// still parses with `font_core`). A `SYS_PATH_WRITE` to a noted path forgets it (the next read goes to the volume).
// Witness, once per face on its first serve: `[kfont] resident path=<p> kib=<n> -> served-from-memory`.
// Gated on `selfdiag` (the fulfiller's own feature): a build without SYS_PATH_READ carries none of it.

#[cfg(feature = "selfdiag")]
struct Resident {
    path: alloc::string::String,
    data: &'static [u8],
    said: bool,
}

#[cfg(feature = "selfdiag")]
static RESIDENT: crate::sync::Mutex<alloc::vec::Vec<Resident>> = crate::sync::Mutex::new(alloc::vec::Vec::new());
#[cfg(feature = "selfdiag")]
static RESIDENT_SERVED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `tt::load`: face `path` parsed from `data` (the leaked buffer the engine keeps for the boot).
#[cfg(feature = "selfdiag")]
pub fn resident_note(path: &str, data: &'static [u8]) {
    let mut r = RESIDENT.lock();
    r.retain(|e| e.path != path);
    r.push(Resident { path: alloc::string::String::from(path), data, said: false });
}

/// A write reached `path`: the resident copy no longer stands for the file.
#[cfg(feature = "selfdiag")]
pub fn resident_forget(path: &str) {
    RESIDENT.lock().retain(|e| e.path != path);
}

/// `SYS_PATH_READ` of a resident face: up to `cap` bytes at `off` (empty at or past the end), or `None` when
/// the kernel holds no face at `path` (the volume answers).
#[cfg(feature = "selfdiag")]
pub fn resident_read(path: &str, off: u64, cap: usize) -> Option<alloc::vec::Vec<u8>> {
    let mut r = RESIDENT.lock();
    let e = r.iter_mut().find(|e| e.path == path)?;
    let len = e.data.len() as u64;
    let out = if off >= len || cap == 0 { alloc::vec::Vec::new() } else { e.data[off as usize..(off + cap as u64).min(len) as usize].to_vec() };
    RESIDENT_SERVED.fetch_add(out.len() as u64, core::sync::atomic::Ordering::Relaxed);
    if !e.said {
        e.said = true;
        serial_println!("[kfont] resident path={} kib={} -> served-from-memory", e.path, e.data.len() / 1024);
    }
    Some(out)
}

/// Bytes served from resident faces this boot (`tests lumenfast`).
#[cfg(feature = "selfdiag")]
pub fn resident_served() -> u64 {
    RESIDENT_SERVED.load(core::sync::atomic::Ordering::Relaxed)
}

// ------------------------------------------------------------------------------------------------ FONTS2 (B650)
// The file tail holds FONTS2's statements so that no line above moves (the knob-off image's panic locations are
// line numbers): the figures rule and `tests fonts`. Both exist only on a desktop build, like the engine they read.

/// FONTS2 (rmbp-ledger B650): the FIGURES RULE of the text seam — `true` when the run OPENS with a figure (after
/// leading spaces, an ASCII digit, or a `+` / `-` / `.` followed by one). Such a run is a readout whose digits change
/// in place — `HH:MM`, `97%`, `13/16`, `11 min`, `13 px`, `1h 2m 3s` — and draws with `tnum` (tabular figures), so
/// its width is the same for every value. `Untitled 2` or `Log In 3 times` open with a letter and keep the face's
/// proportional figures. Applied in `tt::style` for every string call (`draw_text`, `draw_row`, `draw_with`,
/// `advance`, so `fit`); `draw_cell` draws one glyph on a grid and has no run to classify.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn tabular(s: &[u8]) -> bool {
    let i = s.iter().position(|&c| c != b' ').unwrap_or(s.len());
    match s.get(i) {
        Some(c) if c.is_ascii_digit() => true,
        Some(b'+' | b'-' | b'.') => s.get(i + 1).is_some_and(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// FONTS2 (B650): the vendored faces' header facts the kernel checks its loaded slots against — (wire name,
/// numGlyphs, unitsPerEm, hhea ascender / descender / lineGap), fontTools 4.66.1's reading of the pinned files
/// (`unaos/firmware/fonts.pins`; the host KAT rows in `font_core/tests/data/kat_data.rs` carry the rest).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
const VENDORED_KAT: [(&str, u16, u16, [i16; 3]); 3] = [
    ("inter", 2937, 2048, [1984, -494, 0]),
    ("jetbrains-mono", 1743, 1000, [1020, -300, 0]),
    ("noto-sans", 3877, 1000, [1069, -293, 0]),
];

/// `tests fonts` (FONTS2, rmbp-ledger B650): the four things FONTSCRAFT left owed, measured through the seam —
/// * tnum: the clock (`Face::Chrome`, as the menu bar draws it) and five meter shapes (`Face::Ui`) have the same
///   advance for `1`s as for `0`s; CONTROL: `x 11111` vs `x 00000` (no leading figure, proportional) DIFFER when Inter
///   is the UI face (its proportional `1` is narrower; DejaVu's default figures are already tabular).
/// * mvar: the UI sans face instanced at `wght` 100 / 400 / 700 / 900 at the UI size keeps ONE line box, and when
///   the face carries `MVAR` its underline or x-height moves with the weight (Inter: `undo`/`unds`).
/// * gsub_fv: the FeatureVariations reader on `layout::FEATURE_VARIATIONS_PROBE` (`ss01` = lookup 0 at the default
///   instance, lookup 1 at axis-0 coordinate 0.5 and above, nothing selected below); the loaded faces' own record
///   count is printed (0: no vendored face carries FeatureVariations).
/// * kat: each vendored face loaded with the pinned header facts ([`VENDORED_KAT`]).
///
/// `:: FONTS2: tnum=ok(clock,meters=5) mvar=ok(instances=4) gsub_fv=ok(rules=1) kat=ok(vendored=3) -> PASS ::`.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn fonts2_fixture() {
    use alloc::string::String;
    use alloc::vec::Vec;
    use font_core::ui::{Role, Style};
    tt::service();
    if !tt::ready() {
        serial_println!(":: FONTS2: tnum=fail(no-face) mvar=fail(no-face) gsub_fv=fail(no-face) kat=fail(vendored=0) -> FAIL :: (no face loaded from {}: see the [kfont] load line)", DIR);
        return;
    }
    // (a) tnum.
    let same = |a: &[u8], b: &[u8], f: Face| advance(a, false, f) == advance(b, false, f);
    let clock = tabular(b"11:11") && same(b"11:11", b"00:00", Face::Chrome);
    let meters: [(&[u8], &[u8]); 5] = [(b"11%", b"00%"), (b"11/16", b"00/16"), (b"11 min", b"00 min"), (b"11 px", b"00 px"), (b"1h 11m 11s", b"0h 00m 00s")];
    let meters_ok = meters.iter().filter(|&&(a, b)| tabular(a) && same(a, b, Face::Ui)).count();
    let ui_name = Face::Ui.name();
    let control_differs = !same(b"x 11111", b"x 00000", Face::Ui);
    let control_ok = !tabular(b"x 11111") && (control_differs || ui_name != "inter");
    serial_println!(
        "[fonts2] tnum clock={} ({}px) meters={}/5 control={} ui_face={}",
        clock as u8, advance(b"00:00", false, Face::Chrome), meters_ok, if control_differs { "differs" } else { "same" }, ui_name
    );
    let tnum_ok = clock && meters_ok == 5 && control_ok;
    // (b) MVAR.
    let mv = tt::with(|t| {
        let base = Style::new(Role::Sans, false, t.styles[2].0.size);
        let insts: Vec<font_core::Font> = [100u16, 400, 700, 900].iter().filter_map(|&w| t.eng.primary_instance(&base.with_weight(w))).collect();
        let Some(reg) = insts.get(1).copied() else { return (0usize, false, String::from("no sans face")) };
        let one_line = insts.iter().all(|f| f.line_metrics() == reg.line_metrics());
        let moved = insts.iter().any(|f| f.underline() != reg.underline() || f.x_height() != reg.x_height());
        let tags: Vec<String> = reg.mvar.map(|m| m.tags()).unwrap_or_default().iter().map(|g| String::from(core::str::from_utf8(g).unwrap_or("?"))).collect();
        let per: Vec<String> = insts.iter().map(|f| alloc::format!("{:?}/{:?}", f.underline().unwrap_or((0, 0)), f.x_height().unwrap_or(0))).collect();
        let detail = alloc::format!(
            "tags={} line={:?} one_line={} underline/xh@100,400,700,900={}",
            if tags.is_empty() { String::from("none") } else { tags.join(",") }, reg.line_metrics(), one_line as u8, per.join(",")
        );
        (insts.len(), one_line && (reg.mvar.is_none() || moved), detail)
    });
    let (n_inst, mvar_ok, mv_detail) = mv.unwrap_or((0, false, String::from("engine busy")));
    serial_println!("[fonts2] mvar {}", mv_detail);
    let mvar_ok = mvar_ok && n_inst == 4;
    // (c) GSUB FeatureVariations — on the probe; the faces' own count for the record.
    let probe = font_core::layout::Gsub::parse(font_core::layout::FEATURE_VARIATIONS_PROBE);
    let (rules, fv_ok, fv_detail) = match probe {
        Some(g) => {
            let at = |c: f32| g.0.feature_lookups_at(&[*b"DFLT"], *b"ss01", &[c]);
            let (d, h, f) = (at(0.0), at(0.5), at(1.0));
            let below = g.0.selected_variation(&[0.25]);
            let ok = d.as_deref() == Some(&[0][..]) && h.as_deref() == Some(&[1][..]) && f.as_deref() == Some(&[1][..]) && below.is_none() && g.0.feature_variation_count() == 1;
            (g.0.feature_variation_count(), ok, alloc::format!("probe ss01@0={:?} @0.5={:?} @1.0={:?} selected@0.25={:?}", d, h, f, below))
        }
        None => (0, false, String::from("probe did not parse")),
    };
    let face_fv = tt::with(|t| t.eng.slots().iter().map(|s| s.font.gsub.map_or(0, |g| g.0.feature_variation_count()) + s.font.gpos.map_or(0, |g| g.0.feature_variation_count())).sum::<usize>()).unwrap_or(0);
    serial_println!("[fonts2] gsub_fv {} faces_records={} (the vendored faces' GSUB/GPOS are version 1.0: the reader is proven on the probe)", fv_detail, face_fv);
    // (d) the vendored faces against their pinned header facts.
    let kat = tt::with(|t| {
        VENDORED_KAT
            .iter()
            .map(|&(name, ng, upem, hhea)| {
                let hit = t.eng.slots().iter().find(|s| s.name == name);
                let ok = hit.is_some_and(|s| s.font.num_glyphs == ng && s.font.units_per_em == upem && [s.font.ascender, s.font.descender, s.font.line_gap] == hhea);
                (name, hit.is_some(), ok)
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let vendored = kat.iter().filter(|k| k.2).count();
    let kd: Vec<String> = kat.iter().map(|(n, present, ok)| alloc::format!("{}={}", n, if *ok { "ok" } else if *present { "differs" } else { "absent" })).collect();
    serial_println!("[fonts2] kat {}", kd.join(" "));
    let kat_ok = vendored == VENDORED_KAT.len();
    let word = |b: bool| if b { "ok" } else { "fail" };
    serial_println!(
        ":: FONTS2: tnum={}(clock,meters={}) mvar={}(instances={}) gsub_fv={}(rules={}) kat={}(vendored={}) -> {} ::",
        word(tnum_ok), meters_ok, word(mvar_ok), n_inst, word(fv_ok), rules, word(kat_ok), vendored,
        if tnum_ok && mvar_ok && fv_ok && kat_ok { "PASS" } else { "FAIL" }
    );
}

// ------------------------------------------------------------------------------------------------ GLYPHS (B676)
// The file tail again (FONTS2's rule: no line above moves). `tests glyphs` — i18n through the one text seam. Desktop
// builds only, like the engine it reads.

/// GLYPHS (rmbp-ledger B676): one line per script `tests glyphs` renders — Arabic (two words: an RTL paragraph),
/// Devanagari (opening on the conjunct KSSA, KA + VIRAMA + SSA), CJK (Han and Hiragana), Thai (above and below marks).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
const GLYPH_LINES: [(&str, &str); 4] = [
    ("arabic", "\u{645}\u{631}\u{62D}\u{628}\u{627} \u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645}"),
    ("devanagari", "\u{915}\u{94D}\u{937}\u{92E}\u{93E} \u{928}\u{92E}\u{938}\u{94D}\u{924}\u{947}"),
    ("cjk", "\u{4E2D}\u{6587} \u{65E5}\u{672C}\u{8A9E} \u{3072}\u{3089}\u{304C}\u{306A}"),
    ("thai", "\u{E2A}\u{E27}\u{E31}\u{E2A}\u{E14}\u{E35}\u{E0A}\u{E32}\u{E27}\u{E42}\u{E25}\u{E01}"),
];

/// GLYPHS (B676): the Devanagari conjunct the fixture asks the shaper to FORM (KA + VIRAMA + SSA -> one KSSA glyph in
/// Noto Sans Devanagari): fewer glyphs than code points, none of them `.notdef`.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
const GLYPH_CONJUNCT: &str = "\u{915}\u{94D}\u{937}";

/// GLYPHS (B676): the PSEUDO-LOCALE of a furniture label — its Latin letters swapped for accented forms (which keep
/// the letter's width class: a translation's diacritics) and 40 % more characters, rounded up, cycled from its own
/// letters (so the label's letter mix, and with it its mean advance, is kept): `Shut Down` -> `Shüt DöwñShüt`.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn pseudo_label(s: &str) -> alloc::string::String {
    let acc = |c: char| match c {
        'a' => 'á',
        'e' => 'é',
        'i' => 'í',
        'o' => 'ö',
        'u' => 'ü',
        'y' => 'ý',
        'c' => 'ç',
        'n' => 'ñ',
        'A' => 'Á',
        'E' => 'É',
        'I' => 'Í',
        'O' => 'Ö',
        'U' => 'Ü',
        'C' => 'Ç',
        'N' => 'Ñ',
        _ => c,
    };
    let base: alloc::vec::Vec<char> = s.chars().map(acc).collect();
    let letters: alloc::vec::Vec<char> = base.iter().copied().filter(|c| !c.is_whitespace()).collect();
    let mut out: alloc::string::String = base.iter().collect();
    if !letters.is_empty() {
        for i in 0..(base.len() * 2).div_ceil(5) {
            out.push(letters[i % letters.len()]);
        }
    }
    out
}

/// `tests glyphs` (GLYPHS, rmbp-ledger B676; DESIGN-LANGUAGE §2.2; BP-bar-22): i18n is first-class.
/// * The fallback chain resolves PER GLYPH RUN: the four [`GLYPH_LINES`] are drawn through the seam (`Face::Ui`) into
///   an offscreen surface (ink counted per line) and resolved glyph by glyph by `font_core::ui::Engine::resolve` — the
///   draw calls' own shaping path — naming the face each glyph came from; `.notdef` (glyph 0) is counted. A script no
///   face in the chain covers falls to the chain's terminal, the `font8x8` atlas — an ASCII atlas that has no glyph
///   for it either — and its word is `font8x8` with its `.notdef` count: measured, not hidden.
/// * Devanagari: [`GLYPH_CONJUNCT`] forms (fewer glyphs than code points). Arabic: the run advances right to left —
///   its source clusters DESCEND along the visual order (first > last, never rising).
/// * Pseudo-locale: every fixed-box furniture label (`crystal::label_boxes`, `softy::startmenu::label_boxes`) at
///   +40 % ([`pseudo_label`]) is measured with `advance` in its own face and weight against the box its painter
///   clips it to; a label wider than its box is a clip.
///
/// `:: GLYPHS: arabic=ok devanagari=ok cjk=ok thai=ok notdef=0 rtl=ok pseudo_clipped=0 -> PASS ::`.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn glyphs_fixture() {
    use alloc::string::String;
    use alloc::vec::Vec;
    tt::service();
    if !tt::ready() {
        serial_println!(":: GLYPHS: arabic=fail devanagari=fail cjk=fail thai=fail notdef=0 rtl=fail pseudo_clipped=0 -> FAIL :: (no face loaded from {}: see the [kfont] load line)", DIR);
        return;
    }
    let face = Face::Ui;
    let ch = face.cell_h().max(1);
    const W: usize = 1536;
    let h = GLYPH_LINES.len() * ch + 8;
    let (white, ink) = (crate::video::theme::fixture::WHITE, crate::video::theme::fixture::INK);
    let mut surf = alloc::vec![white; W * h];
    let mut pens = [0usize; 4];
    for (i, (_, line)) in GLYPH_LINES.iter().enumerate() {
        pens[i] = draw_text(&mut surf, W, W, h, 4, 4 + i * ch, line.as_bytes(), ink, false, face);
    }
    let inked = |i: usize| surf[(4 + i * ch) * W..((4 + (i + 1) * ch) * W).min(surf.len())].iter().filter(|&&p| p != white).count();
    let res = tt::with(|t| {
        let mut st = t.styles[face.index()].0;
        st.bold = false;
        let chain = t.eng.chain_names(&st);
        let names: Vec<&'static str> = t.eng.slots().iter().map(|s| s.name).collect();
        let lines: Vec<Vec<font_core::ui::Resolved>> = GLYPH_LINES.iter().map(|(_, l)| t.eng.resolve(l.as_bytes(), st)).collect();
        let conj = t.eng.resolve(GLYPH_CONJUNCT.as_bytes(), st);
        (chain, names, lines, conj, st.size)
    });
    let Some((chain, names, lines, conj, size)) = res else {
        serial_println!(":: GLYPHS: arabic=fail devanagari=fail cjk=fail thai=fail notdef=0 rtl=fail pseudo_clipped=0 -> FAIL :: (engine lock busy)");
        return;
    };
    serial_println!("[glyphs] chain={}>font8x8 face=ui size={:.2} surface={}x{}", chain.join(">"), size, W, h);
    let mut words = ["fail"; 4];
    let (mut notdef_all, mut rtl_ok) = (0usize, false);
    let conj_n = GLYPH_CONJUNCT.chars().count();
    let conj_ok = !conj.is_empty() && conj.len() < conj_n && conj.iter().all(|g| g.glyph != 0);
    for (i, ((key, line), r)) in GLYPH_LINES.iter().zip(lines.iter()).enumerate() {
        let notdef = r.iter().filter(|g| g.glyph == 0).count();
        notdef_all += notdef;
        let mut faces: Vec<&str> = Vec::new();
        for g in r.iter().filter(|g| g.glyph != 0) {
            let n = names.get(g.face as usize).copied().unwrap_or("?");
            if !faces.contains(&n) {
                faces.push(n);
            }
        }
        let cl: Vec<usize> = r.iter().map(|g| g.cluster).collect();
        let desc = cl.len() >= 2 && cl.windows(2).all(|p| p[0] >= p[1]) && cl[0] > cl[cl.len() - 1];
        let (px, adv) = (inked(i), pens[i].saturating_sub(4));
        let mut ok = !r.is_empty() && notdef == 0 && px > 0 && adv > 0;
        let mut extra = String::new();
        match *key {
            "arabic" => {
                rtl_ok = desc;
                ok &= desc;
                extra = alloc::format!(" clusters={}", if desc { "descending(rtl)" } else { "not-descending" });
            }
            "devanagari" => {
                ok &= conj_ok;
                extra = alloc::format!(" conjunct=KA+VIRAMA+SSA glyphs={}/{} formed={}", conj.len(), conj_n, conj_ok as u8);
            }
            _ => {}
        }
        words[i] = if ok { "ok" } else if notdef > 0 { "font8x8" } else { "fail" };
        serial_println!(
            "[glyphs] line script={} faces={} glyphs={} chars={} notdef={} ink_px={} adv_px={}{}{}",
            key, if faces.is_empty() { String::from("none") } else { faces.join("+") }, r.len(), line.chars().filter(|c| !c.is_whitespace()).count(), notdef, px, adv, extra,
            if notdef > 0 { " resolved=none(no face in the chain covers it; the terminal font8x8 is an ASCII atlas: drawn as .notdef)" } else { "" }
        );
    }
    // The pseudo-locale pass.
    let mut boxes: Vec<(&'static str, String, usize, bool, Face)> = Vec::new();
    crate::video::crystal::label_boxes(&mut boxes);
    crate::video::softy::startmenu::label_boxes(&mut boxes);
    let (mut clipped, mut worst) = (0usize, (0usize, "", String::new(), 0usize, 0usize));
    for (surface, label, bx, bold, f) in boxes.iter() {
        let p = pseudo_label(label);
        let (wp, w0) = (advance(p.as_bytes(), *bold, *f), advance(label.as_bytes(), *bold, *f));
        let per_mille = wp * 1000 / (*bx).max(1);
        if per_mille > worst.0 {
            worst = (per_mille, *surface, label.clone(), wp, *bx);
        }
        if wp > *bx {
            clipped += 1;
            serial_println!("[glyphs] pseudo clipped surface={} label=\"{}\" base_w={} pseudo_w={} box={}", surface, label, w0, wp, bx);
        }
    }
    serial_println!(
        "[glyphs] pseudo labels={} expand=+40% clipped={} worst={}:\"{}\" pseudo_w={} box={} fill={}.{}%",
        boxes.len(), clipped, worst.1, worst.2, worst.3, worst.4, worst.0 / 10, worst.0 % 10
    );
    let pass = words.iter().all(|&w| w == "ok") && notdef_all == 0 && rtl_ok && clipped == 0 && !boxes.is_empty();
    serial_println!(
        ":: GLYPHS: arabic={} devanagari={} cjk={} thai={} notdef={} rtl={} pseudo_clipped={} -> {} ::",
        words[0], words[1], words[2], words[3], notdef_all, if rtl_ok { "ok" } else { "fail" }, clipped, if pass { "PASS" } else { "FAIL" }
    );
}

// ------------------------------------------------------------------------------------------------ FONTKIT (B730)
// The file tail again (FONTS2's rule: no line above moves). The probe `tests fontkit` (`tests.rs` tail) reads: the
// type roles' axes from theme's ONE table, and the variable UI face rendering them. Desktop builds only.

/// FONTKIT (rmbp-ledger B730): what `tests fontkit` scores.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub struct FontkitProbe {
    /// Rows of `theme::TYPE_RAMP` whose `(wght, opsz)` hold in BOTH looks: a proportional row names `wght` 100..=900
    /// and `opsz` = its size in pt; the mono row names no axis.
    pub roles: usize,
    pub rows: usize,
    /// The primary UI sans face carries both `wght` and `opsz` (Inter Variable loaded).
    pub variable: bool,
    /// `Face::Title`'s live style carries `type.title2`'s axes (the pass-through in `tt::restyle`).
    pub title_ok: bool,
    /// Ink coverage (sum of A8 coverage) of `Title` at `type.title2`'s size: `wght` 600, then 400.
    pub ink_600: u64,
    pub ink_400: u64,
    /// The advance of `Title` at the same size and `wght` 400: `opsz` 14, then 28 (px).
    pub w_opsz14: f32,
    pub w_opsz28: f32,
    pub face: &'static str,
    pub size: f32,
}

/// FONTKIT (B730): measure the role table and the variable face through the one engine; `None` when no face loaded
/// (or the engine lock stayed busy).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn fontkit_probe() -> Option<FontkitProbe> {
    use crate::video::theme::{type_role, TypeRole, TYPE_RAMP};
    use font_core::ui::{Role, Style};
    let row_ok = |s: crate::video::theme::TypeStep| {
        if s.mono {
            s.axes() == (None, None)
        } else {
            s.axes() == (Some(s.weight), Some(s.size_pt as f32)) && (100..=900).contains(&s.weight)
        }
    };
    let roles = TYPE_RAMP.iter().filter(|r| row_ok(r.1) && row_ok(r.2)).count();
    tt::service();
    if !tt::ready() {
        return None;
    }
    let ink = crate::video::theme::fixture::INK;
    tt::with(|t| {
        let t2 = type_role(TypeRole::Title2, crate::video::look::is_softy());
        let live = t.styles[4].0;
        let title_ok = live.weight == t2.weight && live.opsz == t2.opsz as f32;
        let size = live.size;
        let base = Style::new(Role::Sans, false, size);
        let variable = t.eng.primary_instance(&base).is_some_and(|f| f.has_axis(*b"wght") && f.has_axis(*b"opsz"));
        let mut cover = |st: Style| {
            let mut sum = 0u64;
            t.eng.draw_with(b"Title", st, 0.0, size * 1.5, None, None, ink, &mut |_, _, a| sum += a as u64);
            sum
        };
        let ink_600 = cover(base.with_axes(Some(600), Some(t2.opsz as f32)));
        let ink_400 = cover(base.with_axes(Some(400), Some(t2.opsz as f32)));
        let w_opsz14 = t.eng.measure(b"Title", base.with_axes(Some(400), Some(14.0)));
        let w_opsz28 = t.eng.measure(b"Title", base.with_axes(Some(400), Some(28.0)));
        let face = t.eng.primary_name(Role::Sans, false).unwrap_or("none");
        FontkitProbe { roles, rows: TYPE_RAMP.len(), variable, title_ok, ink_600, ink_400, w_opsz14, w_opsz28, face, size }
    })
}

// ── TEXTSCALE (rmbp-ledger B813) — the largest text step capped to what the panel holds ───────────────────────────
// Flight 37: `font_size=26 … ui=inter-52.75` overwrote Settings' rows. The layouts now grow with the UI line
// (`theme::metrics`, published by `tt::restyle`), so a size whose grown Settings window cannot stand on the panel
// between the bar and the dock is CAPPED: the faces size from `min(font_size, cap)`. The cap is computed here, OUTSIDE
// the engine's lock (it reads the panel through `panel_info_nonblocking` and the furniture's reservations), at each
// candidate size through the one cell formula (`grid_cell_for`) and Settings' own height (`settings::win_h_at`).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub mod textscale {
    use core::sync::atomic::{AtomicI64, Ordering};

    /// The largest css size the preference takes (Principia's schema bound).
    const CSS_MAX: i64 = 32;
    /// `tests textscale`'s css size, 0 = the preference.
    static OVERRIDE: AtomicI64 = AtomicI64::new(0);
    /// The last cap computed (a refused panel read answers it, so the poll never flaps); no cap until the panel reads.
    static CAP: AtomicI64 = AtomicI64::new(CSS_MAX);

    /// The css size `follow_prefs` follows: the fixture's override, else the preference `css`.
    pub fn override_or(css: i64) -> i64 {
        match OVERRIDE.load(Ordering::Relaxed) {
            0 => css,
            o => o,
        }
    }

    /// The UI line (logical px) at css `css` and scale `s2` — the grid cell's height, as `restyle` publishes it.
    pub fn line_at(css: u32, s2: u32) -> usize {
        super::grid_cell_for(css, s2).1 * 2 / (s2.max(1) as usize)
    }

    /// The largest css size in 13..=32 whose grown Settings window (and its title band) stands between the bar and the
    /// dock of a `pw` x `ph` panel at scale `s2`; none -> the default 13. Below 13 nothing grows, so it never caps there.
    pub fn cap_for(pw: usize, ph: usize, s2: u32) -> i64 {
        let avail = ph.saturating_sub(crate::ui_status::top_chrome_h(pw, ph)).saturating_sub(crate::ui_status::chrome_h(ph));
        let chrome = crate::video::wm::TITLE_H() + 2 * crate::video::wm::BORDER();
        let wide = crate::video::metrics::size(crate::video::settings::WIN_W) + 2 * crate::video::wm::BORDER() <= pw;
        let fits = |css: i64| {
            let m = crate::video::theme::TextMetrics { line: line_at(css as u32, s2), cap_css: 0 };
            wide && crate::video::metrics::size(crate::video::settings::win_h_at(m)) + chrome <= avail
        };
        let base = super::GRID_BASE_CSS as i64;
        (base..=CSS_MAX).rev().find(|&c| fits(c)).unwrap_or(base)
    }

    /// The cap for the live panel (call it outside the engine's lock); a busy panel answers the last one.
    pub fn cap_now() -> i64 {
        match crate::video::panel_info_nonblocking() {
            Some(i) if i.width > 0 && i.height > 0 => {
                let c = cap_for(i.width, i.height, crate::video::dpi::s2());
                CAP.store(c, Ordering::Relaxed);
                c
            }
            _ => CAP.load(Ordering::Relaxed),
        }
    }

    /// `tests textscale`: follow css `css` (0 = back to the preference) through the real `follow_prefs` path and
    /// return `Some((cap, effective css))` once the engine holds it, `None` when no face is loaded or it never landed.
    #[cfg(feature = "witness")]
    pub fn step(css: i64) -> Option<(i64, i64)> {
        OVERRIDE.store(css, Ordering::Relaxed);
        for _ in 0..64 {
            super::tt::NEXT_PREFS.store(0, Ordering::Relaxed);
            super::tt::service();
            let want = override_or(crate::prefs::int("display.font_size", 9, 32).unwrap_or(font_core::ui::DEFAULT_CSS_PX));
            if let Some((have, cap)) = super::tt::with(|t| (t.css_px, t.cap)) {
                if have == want {
                    return Some((cap, have.min(cap)));
                }
            } else if !super::tt::ready() {
                return None;
            }
            crate::arch::sched::yield_now();
        }
        None
    }
}
