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
pub mod db_cache;
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

type LoadedMap = HashMap<(PathBuf, u32, bool, bool, Vec<i16>), &'static Face>;

fn faces_loaded() -> &'static Mutex<LoadedMap> {
    static F: OnceLock<Mutex<LoadedMap>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A fresh face id (for faces built outside [`load_face`], e.g. Aether's `@font-face` faces).
/// True for a face loaded from the host's font directories (not `@font-face`, not a fallback twin): these get
/// fontconfig's per-face hinting in [`raster`] (FONTHINT SR62); web fonts are drawn unhinted, as Chromium does.
pub fn is_installed(face: &Face) -> bool {
    faces_loaded().lock().map(|m| m.values().any(|f| f.id == face.id)).unwrap_or(false)
}

pub fn next_face_id() -> u32 {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Loads a discovered face (no synthesis).
pub fn load_face(info: &FaceInfo) -> Option<&'static Face> {
    load_face_synth(info, false, false)
}

pub fn load_face_synth(info: &FaceInfo, bold: bool, oblique: bool) -> Option<&'static Face> {
    let key = (info.path.clone(), info.index, bold, oblique, Vec::new());
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
        if d.covers(i, c) != Some(true) {
            continue; // AETHERFONTDBFALLBACK (SR425): the bounded cmap read decides; only the face that has `c` loads whole
        }
        let Some(f) = load_face(info) else { continue };
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

// ---- AETHERHOSTWGHT (LEDGER SR690): a variable face instanced at the asked weight / width / optical size (named tail block) ----

/// AETHERHOSTWGHT (SR690): the user axis values a face is instanced at for `want` — `wght` from the weight, `wdth`
/// from the stretch, `opsz` from `size_px` when given — one entry per axis the face HAS, each clamped to the axis
/// range (the ELF's `Engine::instance` rule, font_core ui.rs; Chrome clamps the asked weight to the face's range).
/// Empty for a static face. font_core normalizes (`Font::with_variations`).
pub fn axes_for(font: &Font, want: Style, size_px: Option<f32>) -> Vec<([u8; 4], f32)> {
    font.axes()
        .iter()
        .filter_map(|a| {
            let v = match &a.tag {
                b"wght" => want.weight,
                b"wdth" => want.stretch,
                b"opsz" => size_px?,
                _ => return None,
            };
            v.is_finite().then(|| (a.tag, v.max(a.min).min(a.max)))
        })
        .collect()
}

/// AETHERHOSTWGHT (SR690): a discovered face at the instance `axes` (user values by tag), with the given synthesis.
/// A static face, or axes that land on the default instance, return the plain [`load_face_synth`] object itself;
/// otherwise one leaked [`Face`] per (file, index, synthesis, F2Dot14 coords as font_core normalized them), in the
/// ONE loaded-face cache, so two requests for the same weight share it and the raster caches (keyed on the id) see
/// a distinct face per instance.
pub fn load_face_instance(info: &FaceInfo, bold: bool, oblique: bool, axes: &[([u8; 4], f32)]) -> Option<&'static Face> {
    let base = load_face_synth(info, bold, oblique)?;
    let font = base.font.with_variations(axes);
    if !font.is_instanced() {
        return Some(base);
    }
    let coords: Vec<i16> = font.coords().iter().map(|c| (c * 16384.0).round() as i16).collect();
    let key = (info.path.clone(), info.index, bold, oblique, coords);
    let mut m = faces_loaded().lock().ok()?;
    if let Some(f) = m.get(&key) {
        return Some(*f);
    }
    let face: &'static Face = Box::leak(Box::new(Face {
        id: next_face_id(),
        font,
        synth_bold: bold,
        synth_oblique: oblique,
        family: base.family.clone(),
        style: base.style,
    }));
    m.insert(key, face);
    Some(face)
}

/// AETHERHOSTWGHT (SR690): the face a selection draws `want` with: `bold` / `oblique` are [`db::FontDb::select`]'s
/// synthesis flags; a variable face is instanced through [`axes_for`] and its synthetic bold re-decided by
/// [`db::synthesis`] against the INSTANCED weight (bold only where the `wght` axis cannot reach the asked weight).
pub fn load_face_styled(info: &FaceInfo, bold: bool, oblique: bool, want: Style, size_px: Option<f32>) -> Option<&'static Face> {
    let axes = axes_for(&load_face(info)?.font, want, size_px);
    let bold = match axes.iter().find(|a| &a.0 == b"wght") {
        Some(&(_, w)) => db::synthesis(Style { weight: w, ..info.style }, want).0,
        None => bold,
    };
    load_face_instance(info, bold, oblique, &axes)
}

#[cfg(test)]
mod hostwght_tests {
    use super::*;

    fn tree_face(rel: &str) -> FaceInfo {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
        db::faces_in_file(&p).into_iter().next().unwrap_or_else(|| panic!("{rel}: the tree's fixture face"))
    }

    fn want(weight: f32) -> Style {
        Style { weight, ..Style::default() }
    }

    /// Inter at wght 700 is a distinct face whose `H` differs from the default instance's and equals, bit for bit,
    /// font_core's own `instance(&[opsz 14, wght 700])` rasterized through the same raster (same id: same hinting).
    #[test]
    fn hostwght_instances_the_weight() {
        let info = tree_face("fonts/inter/InterVariable.ttf");
        let base = load_face(&info).unwrap();
        let inst = load_face_instance(&info, false, false, &[(*b"wght", 700.0)]).unwrap();
        assert_ne!(inst.id, base.id);
        assert!(std::ptr::eq(inst, load_face_instance(&info, false, false, &[(*b"wght", 700.0)]).unwrap()), "one Face per instance");
        let own = base.font.instance(&[14.0, 700.0]);
        assert_eq!(inst.font.coords(), own.coords());
        let reference = Face { id: inst.id, font: own, synth_bold: false, synth_oblique: false, family: inst.family.clone(), style: inst.style };
        let gid = base.font.glyph_index('H');
        let m = raster::rasterize(inst, gid, 32.0, 0).unwrap();
        let r = raster::rasterize(&reference, gid, 32.0, 0).unwrap();
        let d = raster::rasterize(base, gid, 32.0, 0).unwrap();
        assert_eq!((m.left, m.top, m.w, m.h, &m.data), (r.left, r.top, r.w, r.h, &r.data), "the instance is font_core's");
        assert_ne!((m.w, &m.data), (d.w, &d.data), "wght 700 is not the default instance");
    }

    /// A static face (and a variable face asked for its default) returns the very object `load_face_synth` gives.
    #[test]
    fn hostwght_static_face_unchanged() {
        let s = tree_face("unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf");
        let plain = load_face(&s).unwrap();
        assert!(std::ptr::eq(plain, load_face_instance(&s, false, false, &[(*b"wght", 700.0)]).unwrap()));
        assert!(std::ptr::eq(plain, load_face_styled(&s, false, false, want(700.0), Some(40.0)).unwrap()));
        let bold = load_face_synth(&s, true, false).unwrap();
        assert!(std::ptr::eq(bold, load_face_styled(&s, true, false, want(700.0), None).unwrap()), "a static face keeps select's embolden");
        let inter = tree_face("fonts/inter/InterVariable.ttf");
        assert!(std::ptr::eq(load_face(&inter).unwrap(), load_face_styled(&inter, false, false, want(400.0), None).unwrap()));
    }

    /// Blink's rule would embolden Inter (OS/2 400) at 700; the wght axis reaches 700, so the instance is not emboldened.
    #[test]
    fn hostwght_no_synthetic_bold_when_axis_reaches() {
        let inter = tree_face("fonts/inter/InterVariable.ttf");
        assert_eq!(db::synthesis(inter.style, want(700.0)), (true, false));
        let f = load_face_styled(&inter, true, false, want(700.0), None).unwrap();
        assert!(!f.synth_bold);
        assert_eq!(axes_for(&f.font, want(700.0), None), vec![(*b"wght", 700.0)]);
        let noto = tree_face("fonts/noto-sans/NotoSans-Variable.ttf");
        assert!(!load_face_styled(&noto, true, false, want(900.0), None).unwrap().synth_bold, "900 is within 200 of the axis max 700");
    }

    /// The asked values clamp to the axis range; opsz only when a size is given.
    #[test]
    fn hostwght_clamps_to_axis_range() {
        let inter = tree_face("fonts/inter/InterVariable.ttf");
        let font = load_face(&inter).unwrap().font;
        assert_eq!(axes_for(&font, want(950.0), None), vec![(*b"wght", 900.0)]);
        assert_eq!(axes_for(&font, want(50.0), None), vec![(*b"wght", 100.0)]);
        assert_eq!(axes_for(&font, want(400.0), Some(40.0)), vec![(*b"opsz", 32.0), (*b"wght", 400.0)]);
        assert!(axes_for(&font, want(400.0), Some(10.0)).contains(&(*b"opsz", 14.0)));
    }
}
