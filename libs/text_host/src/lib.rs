//! The host's font stack (QUARTZFONT, LEDGER SR64), on UnaOS's own `font_core` (FONTCORE SR48 + FONTBIDI SR56).
//!
//! Lifted from Aether's `fonts/` (AETHERFONT, SR61) when quartzite — the vessels' GUI toolkit, aether-shell's
//! chrome — became its second user. Aether keeps what is CSS-specific (family lists, `@font-face`, the line
//! breaker, its shaping cache); everything both need lives here:
//!
//! - [`fontconfig`]: fonts-conf(5) read as data (directories, includes, `<alias>`), no libfontconfig.
//! - [`db`]: every installed outline face, family resolution as Chromium does it on Linux, css-fonts-4 §5.2
//!   face selection and Blink's synthesis decision, fontconfig's `sans-serif` fallback order.
//! - [`Face`] / [`load_face`]: a loaded face, alive for the process.
//! - [`raster`]: glyph coverage from font_core's rasterizer in `RenderMode::SkiaAaa`, quarter-pixel x
//!   phases, Skia's A8 pre-blend, synthetic bold/oblique.
//! - [`line`]: one line of text in a named family list — faces, `shape_fallback`, placement, caret
//!   geometry and painting into an RGBA/BGRA buffer. What a toolkit's label or entry needs.

pub mod db;
pub mod fontconfig;
pub mod line;
pub mod raster;

/// The core underneath, for callers that need its UAX tables (graphemes, line breaks) directly.
pub use font_core;

use db::{FaceInfo, Style};
use font_core::Font;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// One loaded face, alive for the process (faces are font-global, not page-scoped).
pub struct Face {
    /// Unique per (file, face index, synthesis): the glyph and shaping caches key on it.
    pub id: u32,
    pub font: Font<'static>,
    pub synth_bold: bool,
    pub synth_oblique: bool,
    /// The family it was selected as (English name ID 1, or the `@font-face` family).
    pub family: String,
    pub style: Style,
}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Face#{}({} {:?}{}{})",
            self.id,
            self.family,
            self.style,
            if self.synth_bold { " +bold" } else { "" },
            if self.synth_oblique { " +oblique" } else { "" }
        )
    }
}

/// Font-unit metrics of a face, the way Skia reports them on Linux (`SkFontMetrics` from FreeType):
/// ascent/descent/line gap from `hhea` (OS/2 typo when USE_TYPO_METRICS), x-height and cap-height from
/// OS/2 (else measured from `x`/`H`), underline from `post`.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub units_per_em: u32,
    pub ascent: f32,
    /// Negative below the baseline.
    pub descent: f32,
    pub line_gap: f32,
    pub x_height: f32,
    pub cap_height: f32,
    /// Negative below the baseline.
    pub underline_position: f32,
    pub underline_thickness: f32,
}

impl Face {
    pub fn metrics(&self) -> Metrics {
        let f = &self.font;
        let (a, d, g) = f.line_metrics();
        let glyph_top =
            |c: char| -> f32 { f.glyph_path(f.glyph_index(c)).filter(|p| !p.is_empty()).map(|p| p.y_max).unwrap_or(0.0) };
        let x_height =
            f.os2.and_then(|o| o.x_height).filter(|&x| x > 0).map(|x| x as f32).unwrap_or_else(|| glyph_top('x'));
        let cap_height =
            f.os2.and_then(|o| o.cap_height).filter(|&x| x > 0).map(|x| x as f32).unwrap_or_else(|| glyph_top('H'));
        let (up, ut) = f.post.map(|p| (p.underline_position as f32, p.underline_thickness as f32)).unwrap_or((0.0, 0.0));
        Metrics {
            units_per_em: f.units_per_em.max(1) as u32,
            ascent: a as f32,
            descent: d as f32,
            line_gap: g as f32,
            x_height,
            cap_height,
            underline_position: up,
            underline_thickness: ut,
        }
    }
    /// The glyph for `c` (None for .notdef).
    pub fn glyph_for_char(&self, c: char) -> Option<u16> {
        let g = self.font.glyph_index(c);
        (g != 0).then_some(g)
    }
    /// Advance of glyph `g` in em.
    pub fn advance_em(&self, g: u16) -> f32 {
        self.font.advance(g) as f32 / self.font.units_per_em.max(1) as f32
    }
}

/// The bytes of a font file, read once and kept for the process.
fn file_bytes(path: &std::path::Path) -> Option<&'static [u8]> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, Option<&'static [u8]>>>> = OnceLock::new();
    let m = FILES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(b) = m.lock().ok()?.get(path) {
        return *b;
    }
    let b = std::fs::read(path).ok().map(|v| &*Box::leak(v.into_boxed_slice()));
    m.lock().ok()?.insert(path.to_path_buf(), b);
    b
}

type LoadedMap = HashMap<(PathBuf, u32, bool, bool), &'static Face>;

fn faces_loaded() -> &'static Mutex<LoadedMap> {
    static F: OnceLock<Mutex<LoadedMap>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A fresh face id (for faces built outside [`load_face`], e.g. Aether's `@font-face` faces).
pub fn next_face_id() -> u32 {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Loads a discovered face (no synthesis).
pub fn load_face(info: &FaceInfo) -> Option<&'static Face> {
    load_face_synth(info, false, false)
}

pub fn load_face_synth(info: &FaceInfo, bold: bool, oblique: bool) -> Option<&'static Face> {
    let key = (info.path.clone(), info.index, bold, oblique);
    if let Some(f) = faces_loaded().lock().ok()?.get(&key) {
        return Some(*f);
    }
    let bytes = file_bytes(&info.path)?;
    let font = Font::parse_face(bytes, info.index).ok()?;
    let face: &'static Face = Box::leak(Box::new(Face {
        id: next_face_id(),
        font,
        synth_bold: bold,
        synth_oblique: oblique,
        family: info.families.first().cloned().unwrap_or_default(),
        style: info.style,
    }));
    faces_loaded().lock().ok()?.insert(key, face);
    Some(face)
}

/// The face an installed family NAME gives for `want` (resolution as [`db::FontDb::resolve_family`], then
/// css-fonts-4 §5.2 selection and synthesis), or None when the family resolves to nothing.
pub fn installed_face(name: &str, want: Style) -> Option<&'static Face> {
    let d = db::db();
    let fam = d.resolve_family(name)?;
    let (i, b, o) = d.select(&fam, want)?;
    load_face_synth(d.faces.get(i)?, b, o)
}

/// Platform fallback for one character (`gfx::GetFontForCharacter`): the first face in fontconfig's
/// `sans-serif` order whose cmap has it, as the style-matched face of its family.
pub fn platform_fallback(c: char, want: Style) -> Option<&'static Face> {
    let d = db::db();
    for &i in d.fallback_order() {
        let info = &d.faces[i];
        let Some(f) = load_face(info) else { continue };
        if f.font.glyph_index(c) == 0 {
            continue;
        }
        let picked = d
            .select(&info.families[0], want)
            .and_then(|(k, b, o)| load_face_synth(d.faces.get(k)?, b, o))
            .filter(|g| g.font.glyph_index(c) != 0);
        return picked.or(Some(f));
    }
    None
}

/// Whether a character needs a glyph at all (controls, default-ignorables and spaces are drawn by none).
pub fn needs_glyph(c: char) -> bool {
    !(c.is_control() || c.is_whitespace() || font_core::ucd::is_default_ignorable(c))
}

/// Pixel line metrics of `font` at `size`: (ascent, descent, line gap), each rounded to whole pixels the
/// way Blink's SimpleFontData rounds them.
pub fn line_metrics(font: &Face, size: f32) -> (f32, f32, f32) {
    let m = font.metrics();
    let scale = size / m.units_per_em as f32;
    ((m.ascent * scale).round(), (-m.descent * scale).round(), (m.line_gap * scale).round())
}
