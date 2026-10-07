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
}

impl Face {
    /// The bitmap atlas this face falls back to.
    #[inline(always)]
    pub const fn bitmap(self) -> font::Face {
        match self {
            Face::Chrome => font::Face::Chrome,
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
    } as usize;
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
    use font_core::Font;

    /// (file, wire name, role, bold). The first is REQUIRED: without it the desktop stays on the bitmap face.
    pub const FACES: &[(&str, &str, Role, bool)] = &[
        ("DejaVuSans.ttf", "dejavu-sans", Role::Sans, false),
        ("DejaVuSans-Bold.ttf", "dejavu-sans-bold", Role::Sans, true),
        ("DejaVuSansMono.ttf", "dejavu-mono", Role::Mono, false),
        ("DejaVuSansMono-Bold.ttf", "dejavu-mono-bold", Role::Mono, true),
        ("DejaVuSerif.ttf", "dejavu-serif", Role::Serif, false),
        ("DejaVuSerif-Bold.ttf", "dejavu-serif-bold", Role::Serif, true),
        ("NotoSansArabic-Regular.ttf", "noto-arabic", Role::Script, false),
        ("NotoSansHebrew-Regular.ttf", "noto-hebrew", Role::Script, false),
        ("NotoSansDevanagari-Regular.ttf", "noto-devanagari", Role::Script, false),
        ("NotoSansThai-Regular.ttf", "noto-thai", Role::Script, false),
    ];

    /// `system.display.font` / `system.display.font_size` (Principia schema rows, KERNELFONT M3).
    const FONT_KEY: &str = "display.font";
    const FONT_SIZE_KEY: &str = "display.font_size";
    const RETRY_MS: u64 = 2000;
    const MAX_TRIES: u32 = 15;
    const PREFS_MS: u64 = 1000;
    /// A face file larger than this is refused (DejaVu Sans is 760 KB; the Noto script faces are smaller).
    const FACE_MAX: usize = 4 * 1024 * 1024;
    /// Bounded wait for the engine lock before a call draws with the bitmap face instead.
    const SPIN: u32 = 1 << 14;

    pub struct Tt {
        pub eng: Engine<'static>,
        /// Per [`super::Face`] index: style and the baseline row inside the face's cell.
        pub styles: [(Style, f32); 4],
        pub names: [&'static str; 4],
        pub family: Role,
        pub css_px: i64,
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
    static NEXT_PREFS: AtomicU64 = AtomicU64::new(0);
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
            _ => (font::CELL_W, font::CELL_H),
        }
    }

    fn style(t: &Tt, face: usize, bold: bool) -> (Style, f32) {
        let (mut st, bl) = t.styles[face];
        st.bold = bold;
        (st, bl)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(px: &mut [u32], stride: usize, clip_w: usize, clip_h: usize, x: usize, y: usize, s: &[u8], ink: u32, bold: bool, face: usize) -> Option<usize> {
        with(|t| {
            let cell_h = cell(face).1;
            if y + cell_h > clip_h {
                return x;
            }
            let (st, bl) = style(t, face, bold);
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
            let (st, bl) = style(t, face, bold);
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
            let (st, bl) = style(t, face, bold);
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
            let (st, bl) = style(t, face, bold);
            let pen = t.eng.draw_with(s, st, x as f32, y as f32 + bl, Some((x + max_w) as f32), None, ink, &mut |gx, gy, a| put(gx as isize, gy as isize, a));
            x + font_core::fmath::ceil(pen) as usize
        })
    }

    pub fn advance(s: &[u8], bold: bool, face: usize) -> Option<usize> {
        with(|t| {
            let (st, _) = style(t, face, bold);
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
        let eng = &t.eng;
        let fam = if eng.has(t.family) { t.family } else { Role::Sans };
        let mono = if eng.has(Role::Mono) { Role::Mono } else { Role::Sans };
        let body = eng.fit_size(mono, Some(font::CELL_W as f32), font::CELL_H as f32).unwrap_or(11.0);
        let (ccw, cch) = super::chrome_cell(); // UIMETRICS (B372): the chrome face fits the dpi-scaled chrome cell
        let chrome = eng.fit_size_mean(fam, true, ccw as f32, cch as f32).unwrap_or(14.0);
        // KERNELFONT2: the console grid — mono at font_size x ppi / 96, capped by the dpi-scaled cell; UIMETRICS: the
        // cell itself follows font_size (the console regrids, `fbcon::regrid`, after this restyle lands).
        super::GRID_CSS.store(t.css_px.clamp(1, 64) as u32, Ordering::Relaxed);
        let (gw, gh) = super::grid_cell();
        // UIMETRICS (B372): the UI face (the native kernel windows) is font_size x ppi / 96 capped by the dpi-scaled
        // cell's height — 40 px at 2.5, so the default 13 CSS px draws its 29.9 px (KERNELFONT's 13.5-px cap is gone).
        let ui_fit = eng.fit_size(fam, None, gh as f32).unwrap_or(12.0);
        let ui = device_px(t.css_px as f32, t.ppi).min(ui_fit).max(6.0);
        let grid_fit = eng.fit_size(mono, Some(gw as f32), gh as f32).unwrap_or(body);
        let grid = device_px(t.css_px as f32, t.ppi).min(grid_fit).max(6.0);
        let mk = |role: Role, size: f32, cell: usize| (Style { role, bold: false, size }, eng.baseline_in_cell(role, size, cell as f32));
        t.styles = [mk(mono, body, font::CELL_H), mk(fam, chrome, cch), mk(fam, ui, gh), mk(mono, grid, gh)];
        let nm = |r: Role| match r {
            Role::Mono => "dejavu-mono",
            Role::Serif => "dejavu-serif",
            _ => "dejavu-sans",
        };
        t.names = [nm(mono), nm(fam), nm(fam), nm(mono)];
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
        let Some(dir) = [super::DIR, super::DIR_CARD].into_iter().find(|d| mt.stat(&alloc::format!("{}/{}", d, FACES[0].0)).is_ok()) else {
            if n >= MAX_TRIES {
                GAVE_UP.store(true, Ordering::Relaxed);
                serial_println!("[kfont] load faces=0/{} fallback=bitmap reason=absent path={}/{} tries={} (KERNELFONT: the desktop stays on the noto bitmap atlases)", FACES.len(), super::DIR, FACES[0].0, n);
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
        for &(file, name, role, bold) in FACES {
            let p = alloc::format!("{}/{}", dir, file);
            let why = match read_face(mt, &p) {
                Ok(v) => {
                    let len = v.len();
                    let data: &'static [u8] = alloc::boxed::Box::leak(v.into_boxed_slice());
                    match Font::parse(data) {
                        Ok(f) => { #[cfg(feature = "selfdiag")] super::resident_note(&p, data); // LUMENFAST (B507): the parsed face's bytes, served to ring 3's SYS_PATH_READ of the same path; folded onto this line, code before comment, so no line below moves
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
        let mut t = Tt { eng, styles: [(Style { role: Role::Sans, bold: false, size: 12.0 }, 12.0); 4], names: ["", "", "", ""], family, css_px, ppi, font_bytes: bytes, missing, scripts_missing };
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
        crate::video::edidsrc::witness(ppi); // KFONTPPI (B382): edid_src, ppi, scale, cell, grid — one line
        *TT.lock() = Some(t);
        READY.store(true, Ordering::Release);
        regrid_console(); // UIMETRICS (B372): a non-default font_size at load regrids the console too
        super::EPOCH.fetch_add(1, Ordering::AcqRel); // KERNELFONT2 M4: the windows painted before this repaint once
        let _ = crate::video::wm::damage_intersecting(0, 0, 1 << 16, 1 << 16);
    }

    /// Re-size when `system.display.font` / `font_size` change (checked once a second off the paint paths).
    fn follow_prefs() {
        let now = crate::arch::ms();
        if now < NEXT_PREFS.load(Ordering::Relaxed) {
            return;
        }
        NEXT_PREFS.store(now + PREFS_MS, Ordering::Relaxed);
        let family = family_of(crate::prefs::text(FONT_KEY));
        let css_px = crate::prefs::int(FONT_SIZE_KEY, 9, 32).unwrap_or(DEFAULT_CSS_PX);
        let changed = with(|t| {
            if t.family == family && t.css_px == css_px {
                return None;
            }
            t.family = family;
            t.css_px = css_px;
            restyle(t);
            Some(alloc::format!("font={} font_size={} ppi={} chrome={}-{:.2} ui={}-{:.2} console={}-{:.2}", family_name(family), css_px, t.ppi, t.names[1], t.styles[1].0.size, t.names[2], t.styles[2].0.size, t.names[3], t.styles[3].0.size))
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
        let ppi = crate::video::dpi::ppi();
        let moved = with(|t| {
            if ppi == 0 || t.ppi == ppi {
                return false;
            }
            t.ppi = ppi;
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
        crate::tests::register("font", fixture);
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
