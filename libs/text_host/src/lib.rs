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

/// The bytes of a font file, read once and kept for the process. The first caller of a path reads it under the font-file lock: one read and one leaked slice per path (TEXTHOST-FILE-BYTES-RACE SR1013).
fn file_bytes(path: &std::path::Path) -> Option<&'static [u8]> {
    use crate::FONT_FILES as FILES; // TEXTHOST-FACE-BYTES-WITNESS (SR1012): the same file cache, hoisted to the file tail so `files_loaded_bytes` reads it
    let mut g = FILES.get_or_init(|| Mutex::new(HashMap::new())).lock().ok()?;
    if let Some(&b) = g.get(path) {
        return b;
    }
    *g.entry(path.to_path_buf()).or_insert_with(|| std::fs::read(path).ok().map(|v| &*Box::leak(v.into_boxed_slice()))) // TEXTHOST-FILE-BYTES-RACE (SR1013): ONE guard from the get through the vacant arm, so a racing first reader waits and takes the one slice
}

type LoadedMap = HashMap<FaceKey, &'static Face>;

/// The ONE loaded-face cache and its contract (TEXTHOSTFACECACHECONTRACT SR822): ONE `&'static Face` per [`FaceKey`]
/// (built by `face_key` alone) for the process's life. The first loader wins (`remember`'s vacant-entry insert, the
/// single guard of `load_face_instance` and `installed_twin`); an entry is never evicted, replaced or cleared, so a
/// reader may hold the reference forever and a caller may compare faces by pointer (`std::ptr::eq`); `Face.id` is
/// unique per key, which the glyph and shaping caches key on. Its writers: `load_face_synth`, `load_face_instance`,
/// `installed_twin`; its readers by id: `is_installed` and `installed_twin`, through `Loaded::by_id` (TEXTHOSTFACECACHEIDINDEX SR846).
fn faces_loaded() -> &'static Mutex<Loaded> {
    static F: OnceLock<Mutex<Loaded>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(Loaded::default()))
}

/// A fresh face id (for faces built outside [`load_face`], e.g. Aether's `@font-face` faces).
/// True for a face loaded from the host's font directories (not `@font-face`, not a fallback twin): these get
/// fontconfig's per-face hinting in [`raster`] (FONTHINT SR62); web fonts are drawn unhinted, as Chromium does.
pub fn is_installed(face: &Face) -> bool {
    faces_loaded().lock().map(|m| m.by_id.contains_key(&face.id)).unwrap_or(false)
}

pub fn next_face_id() -> u32 {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Loads a discovered face (no synthesis).
pub fn load_face(info: &FaceInfo) -> Option<&'static Face> {
    load_face_synth(info, false, false)
}

/// A discovered face with synthesis flags, the one object per key of the loaded-face cache (`faces_loaded`: the contract; key `face_key`, no coords).
pub fn load_face_synth(info: &FaceInfo, bold: bool, oblique: bool) -> Option<&'static Face> {
    if let Some(f) = cache_get(&*faces_loaded().lock().ok()?, &info.path, info.index, bold, oblique, &[]) {
        return Some(f);
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
    let face: &'static Face = remember(&mut *faces_loaded().lock().ok()?, face_key(info.path.clone(), info.index, bold, oblique, Vec::new()), face); // TEXTHOST-FALLBACKSYNTH-PTR-FLAKE (SR813): first writer wins, so the cache's one object per key holds under parallel first loads
    Some(face)
}

/// The face an installed family NAME gives for `want` (resolution as [`db::FontDb::resolve_family`], then
/// css-fonts-4 §5.2 selection and synthesis), or None when the family resolves to nothing.
pub fn installed_face_in(d: &db::FontDb, name: &str, want: Style) -> Option<&'static Face> {
    let fam = d.resolve_family(name)?;
    let (i, b, o) = d.select(&fam, want)?;
    load_face_styled(d.faces.get(i)?, b, o, want, None)
}

/// Platform fallback for one character (`gfx::GetFontForCharacter`): the first face in fontconfig's
/// `sans-serif` order whose cmap has it, as the style-matched face of its family; select's pick drawn under the fallback road's absolute synthesis rule ([`cover_face`], AETHERFALLBACKSYNTHSTYLEMATCH SR755).
pub fn platform_fallback_in(d: &db::FontDb, c: char, want: Style) -> Option<&'static Face> {
    for &i in d.fallback_order() {
        let info = &d.faces[i];
        if d.covers(i, c) != Some(true) {
            continue; // AETHERFONTDBFALLBACK (SR425): the bounded cmap read decides; only the face that has `c` loads whole
        }
        let Some(f) = load_face(info) else { continue };
        let picked = d
            .select(&info.families[0], want)
            .and_then(|(k, _, _)| cover_face(d.faces.get(k)?, want)) // AETHERFALLBACKSYNTHSTYLEMATCH (SR755): Chromium's fallback road synthesizes by the absolute rule, never select's +200
            .filter(|g| g.font.glyph_index(c) != 0);
        return picked.or_else(|| cover_face(info, want)).or(Some(f));
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
/// The cache's contract is `faces_loaded`'s (one object per `face_key`, first loader wins, never evicted).
/// Past [`INSTANCES_MAX`] instances of one (file, index, synthesis) a new coords tuple returns the nearest loaded one ([`snap_instance`], TEXTHOSTFACECACHEEVICTION SR1004).
pub fn load_face_instance(info: &FaceInfo, bold: bool, oblique: bool, axes: &[([u8; 4], f32)]) -> Option<&'static Face> {
    let base = load_face_synth(info, bold, oblique)?;
    let font = base.font.with_variations(axes);
    if !font.is_instanced() {
        return Some(base);
    }
    let coords: [i16; font_core::var::MAX_AXES] = std::array::from_fn(|i| font.coords().get(i).map_or(0, |c| (c * 16384.0).round() as i16));
    let mut m = faces_loaded().lock().ok()?;
    if let Some(f) = cache_get(&m, &info.path, info.index, bold, oblique, &coords[..font.coords().len()]) {
        return Some(f);
    }
    if let Some(f) = snap_instance(&m, &info.path, info.index, bold, oblique, &coords[..font.coords().len()]) { return Some(f); } // TEXTHOST-FACE-CACHE-EVICTION (SR1004): a full family snaps, never grows
    let face: &'static Face = Box::leak(Box::new(Face {
        id: next_face_id(),
        font,
        synth_bold: bold,
        synth_oblique: oblique,
        family: base.family.clone(),
        style: base.style,
    }));
    remember(&mut m, face_key(info.path.clone(), info.index, bold, oblique, coords[..font.coords().len()].to_vec()), face);
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

// ---- AETHERHOSTWGHTQUARTZITE (LEDGER SR716): text_host's own entry points on the instancing road (named tail block) ----

/// The face an installed family NAME gives for `want` in the process database ([`installed_face_in`] over
/// [`db::db`]): a variable face instanced at the weight through [`load_face_styled`] (AETHERHOSTWGHTQUARTZITE SR716).
pub fn installed_face(name: &str, want: Style) -> Option<&'static Face> {
    installed_face_in(db::db(), name, want)
}

/// Platform fallback for one character in the process database ([`platform_fallback_in`] over [`db::db`]), the
/// style-matched face instanced through [`load_face_styled`] (AETHERHOSTWGHTQUARTZITE SR716).
pub fn platform_fallback(c: char, want: Style) -> Option<&'static Face> {
    platform_fallback_in(db::db(), c, want)
}

#[cfg(test)]
mod hostwghtquartzite_tests {
    use super::*;

    /// A database over the one tree fixture face (`FontDb::from_faces`), and that face.
    fn fixture_db(rel: &str) -> (db::FontDb, FaceInfo) {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
        let info = db::faces_in_file(&p).into_iter().next().unwrap_or_else(|| panic!("{rel}: the tree's fixture face"));
        (db::FontDb::from_faces(vec![info.clone()], crate::fontconfig::Config::default()), info)
    }

    fn want(weight: f32) -> Style {
        Style { weight, ..Style::default() }
    }

    fn ink_h(f: &Face) -> u32 {
        let m = raster::rasterize(f, f.font.glyph_index('H'), 32.0, 0).unwrap();
        m.data.iter().map(|&v| v as u32).sum()
    }

    /// `installed_face_in` on the variable Inter: 400 is today's object, 600 a distinct instance with more ink in
    /// `H`, 700 the instance with no synthetic bold (the old road emboldened the default instance).
    #[test]
    fn hostwghtquartzite_installed_face_instances() {
        let (d, info) = fixture_db("fonts/inter/InterVariable.ttf");
        let fam = info.families[0].clone();
        let f400 = installed_face_in(&d, &fam, want(400.0)).unwrap();
        assert!(std::ptr::eq(f400, load_face(&info).unwrap()), "a default-weight request is today's object");
        let f600 = installed_face_in(&d, &fam, want(600.0)).unwrap();
        assert_ne!(f600.id, f400.id);
        assert!(f600.font.is_instanced() && !f600.synth_bold);
        assert!(ink_h(f600) > ink_h(f400), "600 inks more than 400");
        assert!(db::synthesis(info.style, want(700.0)).0, "the old road emboldened Inter at 700");
        let f700 = installed_face_in(&d, &fam, want(700.0)).unwrap();
        assert!(!f700.synth_bold);
        assert!(std::ptr::eq(f700, load_face_styled(&info, true, false, want(700.0), None).unwrap()));
    }

    /// `platform_fallback_in` gives the style-matched face as the instance; at 400 today's object.
    #[test]
    fn hostwghtquartzite_platform_fallback_instances() {
        let (d, info) = fixture_db("fonts/inter/InterVariable.ttf");
        let g = platform_fallback_in(&d, 'H', want(700.0)).unwrap();
        assert!(!g.synth_bold);
        assert_ne!(g.id, load_face(&info).unwrap().id);
        assert!(std::ptr::eq(g, load_face_styled(&info, true, false, want(700.0), None).unwrap()));
        assert!(std::ptr::eq(platform_fallback_in(&d, 'H', want(400.0)).unwrap(), load_face(&info).unwrap()));
    }

    /// A static face returns the very `load_face_synth` object the pre-SR716 road gave, at every weight.
    #[test]
    fn hostwghtquartzite_static_face_unchanged() {
        let (d, info) = fixture_db("unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf");
        let fam = info.families[0].clone();
        for w in [400.0, 700.0] {
            let (b, o) = db::synthesis(info.style, want(w));
            assert!(std::ptr::eq(installed_face_in(&d, &fam, want(w)).unwrap(), load_face_synth(&info, b, o).unwrap()), "{w}");
        }
    }

    /// The public names are the `_in` bodies over the process database.
    #[test]
    fn hostwghtquartzite_process_db_delegates() {
        if let Some(f) = installed_face("sans", want(700.0)) {
            assert!(std::ptr::eq(f, installed_face_in(db::db(), "sans", want(700.0)).unwrap()));
        }
        if let Some(f) = platform_fallback('H', want(700.0)) {
            assert!(std::ptr::eq(f, platform_fallback_in(db::db(), 'H', want(700.0)).unwrap()));
        }
    }
}

// ---- AETHERHOSTWGHTCOVERFACE (LEDGER SR725): the covering face on the instancing road (named tail block) ----

/// AETHERHOSTWGHTCOVERFACE (SR725): the face a covering road draws `want` with — the face [`platform_fallback_in`] and
/// Aether's `fallback_for` fall back to when the style-matched face lacks the character, and the last resort of
/// [`line::TextStyle::stack`] / Aether's `stack`. AETHERFALLBACKSYNTH (SR731): synthesis is decided against the covering
/// face's own style through [`db::synthesis_threshold`] at [`db::FC_BOLD_WEIGHT`] (SR745: Chromium's fallback emboldens at 600 or more a face below fontconfig bold / obliques a face lacking the asked weight /
/// slant); a variable face is instanced at the asked weight / stretch first, synthetic bold only where its `wght` axis
/// cannot reach ([`load_face_styled`]'s rule).
pub fn cover_face(info: &FaceInfo, want: Style) -> Option<&'static Face> {
    let (b, o) = db::synthesis_threshold(info.style, want, db::FC_BOLD_WEIGHT); load_face_styled(info, b, o, want, None)
}

#[cfg(test)]
mod hostwghtcoverface_tests {
    use super::*;

    fn tree_face(rel: &str) -> FaceInfo {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
        db::faces_in_file(&p).into_iter().next().unwrap_or_else(|| panic!("{rel}: the tree's fixture face"))
    }

    fn want(weight: f32) -> Style {
        Style { weight, ..Style::default() }
    }

    /// The style-matched face lacks `Ж` (the static fixture relabelled into Inter's family at 700; no tree fixture has
    /// U+4E00), so `platform_fallback_in` returns the covering Inter: at 700 the wght 700 instance (the pre-SR725 road
    /// gave its default instance), at 400 select's own Inter, today's object.
    #[test]
    fn hostwghtcoverface_platform_fallback_cover_instances() {
        let inter = tree_face("fonts/inter/InterVariable.ttf");
        let mut bold = tree_face("unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf");
        bold.families = inter.families.clone();
        bold.style = Style { weight: 700.0, ..inter.style };
        let d = db::FontDb::from_faces(vec![inter.clone(), bold.clone()], crate::fontconfig::Config::default());
        assert_eq!(d.select(&inter.families[0], want(700.0)).map(|k| k.0), Some(1), "700 selects the static face");
        assert_eq!(load_face(&bold).unwrap().font.glyph_index('Ж'), 0, "the selected face lacks the character");
        let g = platform_fallback_in(&d, 'Ж', want(700.0)).unwrap();
        assert_ne!(g.id, load_face(&inter).unwrap().id, "not the default instance");
        assert!(g.font.is_instanced() && !g.synth_bold);
        assert!(std::ptr::eq(g, cover_face(&inter, want(700.0)).unwrap()));
        assert!(std::ptr::eq(platform_fallback_in(&d, 'Ж', want(400.0)).unwrap(), load_face(&inter).unwrap()));
    }

    /// A static covering face is the very `load_face` object below 600 and synthetic bold / oblique from 600 (SR731 flipped this pin; SR745 moved 600: Blink's fallback road emboldens >= 600 on a face below fontconfig bold); a variable one asked for its
    /// default weight is too. This is the face line.rs `stack`'s last resort loads (that road reads the process
    /// `db::db()`, so the KAT is on the one fn it calls).
    #[test]
    fn hostwghtcoverface_static_and_default_unchanged() {
        let s = tree_face("unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf");
        let plain = load_face(&s).unwrap();
        for w in [400.0, 500.0] {
            assert!(std::ptr::eq(plain, cover_face(&s, want(w)).unwrap()), "{w}");
        }
        assert!(std::ptr::eq(load_face_synth(&s, true, false).unwrap(), cover_face(&s, want(600.0)).unwrap()), "600 on a 400 cover: synthetic bold (SR745, Blink's fallback threshold)");
        let italic = Style { slant: db::Slant::Italic, ..want(700.0) };
        assert!(std::ptr::eq(load_face_synth(&s, true, true).unwrap(), cover_face(&s, italic).unwrap()), "synthetic bold and oblique on a static covering face (SR731)");
        let inter = tree_face("fonts/inter/InterVariable.ttf");
        assert!(std::ptr::eq(load_face(&inter).unwrap(), cover_face(&inter, want(400.0)).unwrap()));
    }
}

// ---- AETHERFALLBACKSYNTH (LEDGER SR731): a covering face synthesised against its own style (named tail block) ----

#[cfg(test)]
mod fallbacksynth_tests {
    use super::*;

    fn tree_face(rel: &str) -> FaceInfo {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
        db::faces_in_file(&p).into_iter().next().unwrap_or_else(|| panic!("{rel}: the tree's fixture face"))
    }

    fn want(weight: f32) -> Style {
        Style { weight, ..Style::default() }
    }

    const STATIC: &str = "unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf";
    const INTER: &str = "fonts/inter/InterVariable.ttf";

    /// The static regular fixture (usWeightClass 400) as a covering face: at 600, 700 and 900 the synthetic-bold object
    /// (SR745 `db::synthesis_threshold`: >= 600 on a face below fontconfig bold; 600 moved from the plain set), at 400 and 500 the plain `load_face` object.
    #[test]
    fn fallbacksynth_static_cover_bold() {
        let s = tree_face(STATIC);
        let plain = load_face(&s).unwrap();
        for w in [600.0, 700.0, 900.0] {
            let f = cover_face(&s, want(w)).unwrap();
            assert!(f.synth_bold && !f.synth_oblique, "{w}");
            assert_ne!(f.id, plain.id, "{w}");
            assert!(std::ptr::eq(f, load_face_synth(&s, true, false).unwrap()), "{w}");
        }
        for w in [400.0, 500.0] {
            assert!(std::ptr::eq(plain, cover_face(&s, want(w)).unwrap()), "{w}");
        }
    }

    /// A roman static covering face under italic (and oblique) is synthetic oblique, not bold, at 400.
    #[test]
    fn fallbacksynth_static_cover_oblique() {
        let s = tree_face(STATIC);
        for slant in [db::Slant::Italic, db::Slant::Oblique] {
            let f = cover_face(&s, Style { slant, ..want(400.0) }).unwrap();
            assert!(f.synth_oblique && !f.synth_bold, "{slant:?}");
            assert!(std::ptr::eq(f, load_face_synth(&s, false, true).unwrap()), "{slant:?}");
        }
    }

    /// The variable Inter covering face at 700 is the wght 700 instance with no synthesis (unchanged by SR731); under
    /// italic at 400 it is synthetic oblique (InterVariable is roman: no ital / slnt axis) and not instanced.
    #[test]
    fn fallbacksynth_variable_cover_instanced() {
        let inter = tree_face(INTER);
        let b = cover_face(&inter, want(700.0)).unwrap();
        assert!(b.font.is_instanced() && !b.synth_bold && !b.synth_oblique);
        assert!(std::ptr::eq(b, load_face_instance(&inter, false, false, &[(*b"wght", 700.0)]).unwrap()));
        let i = cover_face(&inter, Style { slant: db::Slant::Italic, ..want(400.0) }).unwrap();
        assert!(i.synth_oblique && !i.synth_bold && !i.font.is_instanced());
    }

    /// `platform_fallback_in` onto the cover road with a static face: the style match at 700 (a face of the same family
    /// whose file does not exist) loads nothing, so the road falls to `cover_face` of the static fixture — the
    /// synthetic-bold object; at 400 select's own static face, the `load_face` object. (No static tree face covers a
    /// character the style-matched face lacks, so the unloadable match stands in for it.)
    #[test]
    fn fallbacksynth_platform_fallback_static_cover() {
        let mut s = tree_face(STATIC);
        s.families = vec!["SynthFam".to_string()];
        let mut x = s.clone();
        x.path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("no-such-face-sr731.ttf");
        x.style = Style { weight: 700.0, ..s.style };
        let d = db::FontDb::from_faces(vec![s.clone(), x], crate::fontconfig::Config::default());
        assert_eq!(d.select("SynthFam", want(700.0)).map(|k| k.0), Some(1), "700 selects the unloadable face");
        let g = platform_fallback_in(&d, 'H', want(700.0)).unwrap();
        assert!(g.synth_bold && !g.synth_oblique);
        assert!(std::ptr::eq(g, load_face_synth(&s, true, false).unwrap()));
        assert!(std::ptr::eq(platform_fallback_in(&d, 'H', want(400.0)).unwrap(), load_face(&s).unwrap()));
    }
}

// ---- AETHERFALLBACKSYNTHSTYLEMATCH (LEDGER SR755): the style-matched fallback face under the absolute rule (named tail block) ----

#[cfg(test)]
mod fallbacksynthstylematch_tests {
    use super::*;

    const STATIC: &str = "unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf";

    fn want(weight: f32) -> Style {
        Style { weight, ..Style::default() }
    }

    /// The static regular fixture as family "StyleFam" with one face per weight in `weights` (one file relabelled, so
    /// every face loads the same plain / synthetic objects; which face select picks is asserted on its index).
    fn family(weights: &[f32]) -> (FaceInfo, db::FontDb) {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(STATIC);
        let mut s = db::faces_in_file(&p).into_iter().next().expect("the tree's static fixture face");
        s.families = vec!["StyleFam".to_string()];
        let faces = weights.iter().map(|&w| FaceInfo { style: Style { weight: w, ..s.style }, ..s.clone() }).collect();
        (s, db::FontDb::from_faces(faces, crate::fontconfig::Config::default()))
    }

    /// Regular 400 + Bold 700 under fallback: select's pick is fontconfig's closest weight (600 and 900 the Bold, 500 the
    /// Regular) and none is synthesized, as Chromium (a bold face is never emboldened; 500 is below kBoldThreshold).
    #[test]
    fn fallbacksynthstylematch_two_weight_family() {
        let (s, d) = family(&[400.0, 700.0]);
        let plain = load_face(&s).unwrap();
        for (w, k) in [(600.0, 1), (500.0, 0), (900.0, 1)] {
            assert_eq!(d.select("StyleFam", want(w)).map(|p| p.0), Some(k), "{w}");
            let g = platform_fallback_in(&d, 'H', want(w)).unwrap();
            assert!(std::ptr::eq(g, plain) && !g.synth_bold && !g.synth_oblique, "{w}");
        }
    }

    /// A one-weight Regular family at 600 is synthetic bold on the fallback road (moved by SR755: select's +200 left it
    /// plain; Chromium's fontconfig regular face is not bold and 600 meets kBoldThreshold); at 500 plain.
    #[test]
    fn fallbacksynthstylematch_one_weight_regular() {
        let (s, d) = family(&[400.0]);
        let g = platform_fallback_in(&d, 'H', want(600.0)).unwrap();
        assert!(g.synth_bold && !g.synth_oblique);
        assert!(std::ptr::eq(g, load_face_synth(&s, true, false).unwrap()));
        assert!(std::ptr::eq(platform_fallback_in(&d, 'H', want(500.0)).unwrap(), load_face(&s).unwrap()));
    }

    /// Regular 400 + DemiBold 600 asked at 700: select picks the 600 face, and the fallback road emboldens it (moved by
    /// SR755: 700 is not more than 200 over 600; a 600 face is below fontconfig bold, so Chromium synthesizes).
    #[test]
    fn fallbacksynthstylematch_demibold_at_700() {
        let (s, d) = family(&[400.0, 600.0]);
        assert_eq!(d.select("StyleFam", want(700.0)).map(|p| p.0), Some(1));
        let g = platform_fallback_in(&d, 'H', want(700.0)).unwrap();
        assert!(g.synth_bold && std::ptr::eq(g, load_face_synth(&s, true, false).unwrap()));
    }

    /// The primary road (`installed_face_in`, select's +200) at 600 is unchanged: the two-weight family's Bold and the
    /// one-weight family's Regular, both plain.
    #[test]
    fn fallbacksynthstylematch_primary_road_unchanged() {
        for weights in [&[400.0, 700.0][..], &[400.0][..]] {
            let (s, d) = family(weights);
            let g = installed_face_in(&d, "StyleFam", want(600.0)).unwrap();
            assert!(std::ptr::eq(g, load_face(&s).unwrap()) && !g.synth_bold, "{weights:?}");
        }
    }
}

// ---- AETHERFONTSYNTHESISPROPERTY (LEDGER SR750): an installed face's twin with other synthesis flags (named tail block) ----

/// AETHERFONTSYNTHESISPROPERTY (SR750): the loaded-face-cache twin of an installed `face` with the synthesis flags
/// (`bold`, `oblique`): the same file, index and instance coords keyed with the new flags in the ONE loaded-face cache
/// ([`load_face_synth`] / [`load_face_instance`]'s), so it keeps fontconfig's per-face hinting ([`is_installed`]) and,
/// with both flags false, is the very object an unsynthesized selection loads; built from the same (instanced) font
/// when that key is not loaded yet. None when `face` is not an installed face (`@font-face`, a fallback twin).
/// The cache's contract is `faces_loaded`'s; the twin's key is `face_key` of the face's own key with the new flags.
pub fn installed_twin(face: &'static Face, bold: bool, oblique: bool) -> Option<&'static Face> {
    let mut m = faces_loaded().lock().ok()?;
    let (path, index, _, _, coords) = m.by_id.get(&face.id)?;
    if let Some(f) = cache_get(&m, path, *index, bold, oblique, coords) {
        return Some(f);
    }
    let twin: &'static Face = Box::leak(Box::new(Face {
        id: next_face_id(),
        font: face.font,
        synth_bold: bold,
        synth_oblique: oblique,
        family: face.family.clone(),
        style: face.style,
    }));
    let key = face_key(path.clone(), *index, bold, oblique, coords.clone());
    remember(&mut m, key, twin);
    Some(twin)
}

// ---- TEXTHOSTFALLBACKSYNTHPTRFLAKE (LEDGER SR813): the loaded-face cache's one object per key under parallel first loads (named tail block) ----

#[cfg(test)]
mod facecache_identity_tests {
    use super::*;

    /// Eight threads first-load ONE key at once (the static fixture through a path with a distinct `..` component, so
    /// the key is the same file's but no other test's: PathBuf equality keeps `..`): every caller gets the one cached
    /// object, which a later load returns and which is installed (before SR813 a racing `insert` replaced the first
    /// writer's object, the static-cover pointer pin's once-in-~30 red).
    #[test]
    fn facecache_concurrent_first_load_one_object() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../libs/..")
            .join("unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf");
        let info = db::faces_in_file(&p).into_iter().next().expect("the tree's static fixture face");
        let gate = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (info, gate) = (info.clone(), gate.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    load_face_synth(&info, true, true).map(|f| f as *const Face as usize)
                })
            })
            .collect();
        let got: Vec<usize> = threads.into_iter().map(|t| t.join().unwrap().expect("the fixture loads")).collect();
        let face = load_face_synth(&info, true, true).unwrap();
        assert!(got.iter().all(|&a| a == face as *const Face as usize), "one object per key: {got:?}");
        assert!(is_installed(face) && face.synth_bold && face.synth_oblique);
    }
}

// ---- TEXTHOSTFACECACHECONTRACT (LEDGER SR822): the loaded-face cache's key in one place and its contract pinned on all three roads (named tail block) ----

/// TEXTHOSTFACECACHECONTRACT (SR822): the loaded-face cache's key (`faces_loaded`, whose doc is the contract): the file,
/// the face index, synthetic bold, synthetic oblique, and the instance's F2Dot14 coords as font_core normalized them
/// (empty for the default instance). The three roads build the same shape, so it is one type.
pub(crate) type FaceKey = (PathBuf, u32, bool, bool, Vec<i16>);

/// TEXTHOSTFACECACHECONTRACT (SR822): the ONE builder of a [`FaceKey`]; `load_face_synth` (no coords),
/// `load_face_instance` (the instance's coords) and `installed_twin` (the face's own key with new flags) call it.
fn face_key(path: PathBuf, index: u32, bold: bool, oblique: bool, coords: Vec<i16>) -> FaceKey {
    (path, index, bold, oblique, coords)
}

#[cfg(test)]
mod facecache_contract_tests {
    use super::*;

    const STATIC: &str = "unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf";
    const INTER: &str = "fonts/inter/InterVariable.ttf";

    /// A tree fixture face through a path with a distinct `vessels/..` component, so every key this test loads is fresh
    /// to it (PathBuf equality keeps `..`).
    fn fresh_face(rel: &str) -> FaceInfo {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vessels/..").join(rel);
        db::faces_in_file(&p).into_iter().next().unwrap_or_else(|| panic!("{rel}: the tree's fixture face"))
    }

    fn addr(f: &Face) -> usize {
        f as *const Face as usize
    }

    /// The cache's contract on all three roads at once: two threads behind a Barrier each take the synth road
    /// (`load_face_synth`), the instance road (`load_face_instance` at wght 700) and the twin road (`installed_twin`)
    /// twice, and every call of a road returns ONE object, the cache's entry under `face_key`; where two roads' keys
    /// coincide they return the same object (SR750: an installed face twinned with its own flags is itself).
    #[test]
    fn facecache_contract_three_roads() {
        let (s, inter) = (fresh_face(STATIC), fresh_face(INTER));
        let w700 = [(*b"wght", 700.0)];
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let (s, inter, gate) = (s.clone(), inter.clone(), gate.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    let mut got = Vec::new();
                    for _ in 0..2 {
                        got.push(addr(load_face_synth(&s, true, false).expect("the static fixture loads")));
                        got.push(addr(load_face_instance(&inter, false, false, &w700).expect("Inter loads at 700")));
                        got.push(addr(installed_twin(load_face(&s).expect("the static fixture loads"), false, true).expect("an installed face twins")));
                    }
                    got
                })
            })
            .collect();
        let got: Vec<Vec<usize>> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        let plain = load_face(&s).unwrap();
        let synth = load_face_synth(&s, true, false).unwrap();
        let inst = load_face_instance(&inter, false, false, &w700).unwrap();
        let twin = installed_twin(plain, false, true).unwrap();
        for g in &got {
            for k in 0..2 {
                assert_eq!(g[3 * k], addr(synth), "synth road: one object per key");
                assert_eq!(g[3 * k + 1], addr(inst), "instance road: one object per key");
                assert_eq!(g[3 * k + 2], addr(twin), "twin road: one object per key");
            }
        }
        assert!(inst.font.is_instanced() && twin.synth_oblique && !twin.synth_bold);
        let coords: Vec<i16> = inst.font.coords().iter().map(|c| (c * 16384.0).round() as i16).collect();
        {
            let m = faces_loaded().lock().unwrap();
            let entry = |k: FaceKey| m.get(&k).map(|f| addr(f));
            assert_eq!(entry(face_key(s.path.clone(), s.index, true, false, Vec::new())), Some(addr(synth)), "face_key is the cache's key");
            assert_eq!(entry(face_key(inter.path.clone(), inter.index, false, false, coords)), Some(addr(inst)));
            assert_eq!(entry(face_key(s.path.clone(), s.index, false, true, Vec::new())), Some(addr(twin)));
        }
        assert!(std::ptr::eq(plain, installed_twin(plain, false, false).unwrap()), "a face twinned with its own flags is itself");
        assert!(std::ptr::eq(inst, installed_twin(inst, false, false).unwrap()), "an instance twinned with its own flags is itself");
        assert!(std::ptr::eq(synth, installed_twin(plain, true, false).unwrap()), "the twin road meets the synth road");
        let inst_bold = installed_twin(inst, true, false).unwrap();
        assert!(std::ptr::eq(inst_bold, load_face_instance(&inter, true, false, &w700).unwrap()), "the twin road meets the instance road");
        assert!(std::ptr::eq(synth, load_face_instance(&s, true, false, &w700).unwrap()), "a static face's instance road is the synth road");
        assert!(std::ptr::eq(load_face(&inter).unwrap(), load_face_instance(&inter, false, false, &[(*b"wght", 400.0)]).unwrap()), "the default instance is the synth road's");
    }
}

// ---- TEXTHOSTFACECACHEKEYVARIATION (LEDGER SR826): the loaded-face cache looked up by its key's borrowed parts (named tail block) ----

/// TEXTHOSTFACECACHEKEYVARIATION (SR826): a [`FaceKey`] seen as its parts, so the loaded-face cache (`faces_loaded`)
/// is looked up by borrowed parts and a hit builds no PathBuf and no Vec: the owned key and the borrowed tuple both
/// implement it and the owned key `Borrow`s as it; `Hash` / `Eq` on the trait object work on the parts tuple, which
/// hashes exactly what the owned tuple hashes (std's PathBuf hashes as its Path, a Vec as its slice), as `Borrow`
/// requires. The owned key is still built by `face_key` alone, on a miss.
pub(crate) trait FaceKeyParts {
    fn parts(&self) -> (&std::path::Path, u32, bool, bool, &[i16]);
}

impl FaceKeyParts for FaceKey {
    fn parts(&self) -> (&std::path::Path, u32, bool, bool, &[i16]) {
        (self.0.as_path(), self.1, self.2, self.3, &self.4)
    }
}

impl FaceKeyParts for (&std::path::Path, u32, bool, bool, &[i16]) {
    fn parts(&self) -> (&std::path::Path, u32, bool, bool, &[i16]) {
        *self
    }
}

impl<'a> std::borrow::Borrow<dyn FaceKeyParts + 'a> for FaceKey {
    fn borrow(&self) -> &(dyn FaceKeyParts + 'a) {
        self
    }
}

impl std::hash::Hash for dyn FaceKeyParts + '_ {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        std::hash::Hash::hash(&self.parts(), h)
    }
}

impl PartialEq for dyn FaceKeyParts + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.parts() == other.parts()
    }
}

impl Eq for dyn FaceKeyParts + '_ {}

/// TEXTHOSTFACECACHEKEYVARIATION (SR826): the ONE lookup of the loaded-face cache by a key's borrowed parts (the
/// three roads' hit path: `load_face_synth`, `load_face_instance`, `installed_twin`); the caller holds the guard.
fn cache_get(m: &LoadedMap, path: &std::path::Path, index: u32, bold: bool, oblique: bool, coords: &[i16]) -> Option<&'static Face> {
    let parts: &dyn FaceKeyParts = &(path, index, bold, oblique, coords);
    m.get(parts).copied()
}

#[cfg(test)]
mod facecache_key_variation_tests {
    use super::*;
    use std::hash::BuildHasher;

    /// The borrowed parts find exactly their owned key (`Borrow`'s contract: they hash as the owned key and compare
    /// equal only to it): keys differing in each field (path, index, bold, oblique, coords empty / one / MAX_AXES)
    /// are each found by their parts, and each one-field variation of the fullest key misses. Two keys share a
    /// bucket only if their hashes collide, which no test can force through std's RandomState: the bucket walk is
    /// std's, not ours. On the real cache, `cache_get` returns the object a `load_face_synth` first load cached.
    #[test]
    fn facecache_key_borrowed_lookup_kat() {
        let full: Vec<i16> = (0..font_core::var::MAX_AXES as i16).map(|k| k * 2048 - 8192).collect();
        let a = || PathBuf::from("/fonts/a.ttf");
        let keys: Vec<FaceKey> = vec![
            face_key(a(), 0, false, false, Vec::new()),
            face_key(a(), 1, false, false, Vec::new()),
            face_key(a(), 0, true, false, Vec::new()),
            face_key(a(), 0, false, true, Vec::new()),
            face_key(PathBuf::from("/fonts/b.ttf"), 0, false, false, Vec::new()),
            face_key(a(), 0, false, false, vec![16384]),
            face_key(a(), 0, false, false, full.clone()),
        ];
        let mut m: HashMap<FaceKey, usize> = HashMap::new();
        for (k, key) in keys.iter().enumerate() {
            m.insert(key.clone(), k);
        }
        assert_eq!(m.len(), keys.len(), "every key distinct");
        let s = m.hasher();
        for (k, key) in keys.iter().enumerate() {
            let parts: &dyn FaceKeyParts = &key.parts();
            assert_eq!(s.hash_one(key), s.hash_one(parts), "the parts hash as the owned key ({k})");
            assert_eq!(m.get(parts), Some(&k), "found by its borrowed parts ({k})");
        }
        let (p, i, b, o, c) = keys[6].parts();
        let mut other = c.to_vec();
        other[0] += 1;
        let misses: [(&std::path::Path, u32, bool, bool, &[i16]); 6] = [
            (std::path::Path::new("/fonts/c.ttf"), i, b, o, c),
            (p, i + 2, b, o, c),
            (p, i, !b, o, c),
            (p, i, b, !o, c),
            (p, i, b, o, &c[..c.len() - 1]),
            (p, i, b, o, &other[..]),
        ];
        for (n, q) in misses.iter().enumerate() {
            let parts: &dyn FaceKeyParts = q;
            assert_eq!(m.get(parts), None, "a one-field variation misses ({n})");
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../handlers/..")
            .join("unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf");
        let info = db::faces_in_file(&path).into_iter().next().expect("the tree's static fixture face");
        let face = load_face_synth(&info, false, true).expect("the fixture loads");
        let got = cache_get(&faces_loaded().lock().unwrap(), &info.path, info.index, false, true, &[]);
        assert!(got.is_some_and(|g| std::ptr::eq(g, face)), "cache_get is the cache's entry");
        assert!(std::ptr::eq(face, load_face_synth(&info, false, true).unwrap()), "the hit road returns the one object");
    }
}

// ---- TEXTHOSTFACECACHEIDINDEX (LEDGER SR846): the loaded-face cache's id index beside its map under its one lock (named tail block) ----

/// TEXTHOSTFACECACHEIDINDEX (SR846): the loaded-face cache (`faces_loaded`, whose doc is the contract) and its index by
/// `Face.id` under the ONE lock: `by_id` holds exactly the ids of `map`'s values, each to the key its face sits under (one
/// face per key, one fresh id per face), so `is_installed` and `installed_twin` read by id without scanning the map.
/// Reads go through the map (`Deref`, read-only); the ONE writer is [`remember`].
#[derive(Default)]
struct Loaded {
    map: LoadedMap,
    by_id: HashMap<u32, FaceKey>,
}

impl std::ops::Deref for Loaded {
    type Target = LoadedMap;
    fn deref(&self) -> &LoadedMap {
        &self.map
    }
}

/// TEXTHOSTFACECACHEIDINDEX (SR846): the ONE insert into the loaded-face cache (`load_face_synth`, `load_face_instance`,
/// `installed_twin` call it under their guard): the first writer wins (SR813: an occupied key returns its cached
/// object), and a vacant key writes the id index with the map entry; the key is cloned on this miss path only.
fn remember(l: &mut Loaded, key: FaceKey, face: &'static Face) -> &'static Face {
    match l.map.entry(key) {
        std::collections::hash_map::Entry::Occupied(e) => *e.get(),
        std::collections::hash_map::Entry::Vacant(v) => {
            l.by_id.insert(face.id, v.key().clone());
            *v.insert(face)
        }
    }
}

#[cfg(test)]
mod facecache_id_index_tests {
    use super::*;

    /// The pre-SR846 `is_installed`: a scan of the map's values by id (the oracle).
    fn scan_installed(face: &Face) -> bool {
        faces_loaded().lock().unwrap().values().any(|f| f.id == face.id)
    }

    /// The pre-SR846 `installed_twin` key search: a scan of the map's entries by id (the oracle).
    fn scan_key(face: &Face) -> Option<FaceKey> {
        faces_loaded().lock().unwrap().iter().find(|(_, f)| f.id == face.id).map(|(k, _)| k.clone())
    }

    fn addr(f: &Face) -> usize {
        f as *const Face as usize
    }

    /// K = 16 objects through the three roads (the static fixture's synth road x4 flag pairs, Inter plain and five
    /// instances, six twins), all keys fresh to this test (a distinct `tools/..` path component): for each, the index
    /// answers as the old scans did: `is_installed` == the value scan, `by_id` names the scanned key, and
    /// `installed_twin` under every flag pair is the map's entry under that key with those flags; the bold twin of the
    /// installed static face is the synth road's object; a face never inserted is not installed and has no twin; and
    /// under the lock the index is the map's bijection. Neither reader allocates (counted by reading: a `contains_key`;
    /// a `get` then `cache_get` by borrowed parts).
    #[test]
    fn facecache_id_index_matches_scan_kat() {
        let fresh = |rel: &str| {
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/..").join(rel);
            db::faces_in_file(&p).into_iter().next().unwrap_or_else(|| panic!("{rel}: the tree's fixture face"))
        };
        let s = fresh("unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf");
        let inter = fresh("fonts/inter/InterVariable.ttf");
        let flags = [(false, false), (true, false), (false, true), (true, true)];
        let mut faces: Vec<&'static Face> = flags.iter().map(|&(b, o)| load_face_synth(&s, b, o).expect("the static fixture loads")).collect();
        let inter_plain = load_face(&inter).expect("Inter loads");
        faces.push(inter_plain);
        let insts: Vec<&'static Face> = [300.0, 500.0, 600.0, 700.0, 800.0]
            .iter()
            .map(|&w| load_face_instance(&inter, false, false, &[(*b"wght", w)]).expect("Inter instances"))
            .collect();
        faces.extend(&insts);
        for &i in &insts {
            faces.push(installed_twin(i, false, true).expect("an installed instance twins"));
        }
        faces.push(installed_twin(inter_plain, true, false).expect("an installed face twins"));
        let distinct: std::collections::HashSet<usize> = faces.iter().map(|f| addr(f)).collect();
        assert_eq!(distinct.len(), 16, "K = 16 distinct objects");
        for &f in &faces {
            assert!(is_installed(f) && scan_installed(f), "installed through the index and the scan");
            let key = scan_key(f).expect("the scan finds every loaded face");
            assert_eq!(faces_loaded().lock().unwrap().by_id.get(&f.id), Some(&key), "by_id names the scanned key");
            for &(b, o) in &flags {
                let twin = installed_twin(f, b, o).expect("an installed face twins");
                let m = faces_loaded().lock().unwrap();
                let entry = cache_get(&m, &key.0, key.1, b, o, &key.4);
                assert!(entry.is_some_and(|e| std::ptr::eq(e, twin)), "installed_twin is the map's entry under the scanned key");
            }
        }
        assert!(std::ptr::eq(installed_twin(faces[0], true, false).unwrap(), load_face_synth(&s, true, false).unwrap()), "the bold twin through the index");
        let stray: &'static Face = Box::leak(Box::new(Face {
            id: next_face_id(),
            font: faces[0].font,
            synth_bold: false,
            synth_oblique: false,
            family: faces[0].family.clone(),
            style: faces[0].style,
        }));
        assert!(!is_installed(stray) && !scan_installed(stray), "a face never inserted is not installed");
        assert!(installed_twin(stray, true, false).is_none() && scan_key(stray).is_none(), "nor has it a twin");
        let m = faces_loaded().lock().unwrap();
        assert_eq!(m.by_id.len(), m.map.len(), "one index entry per map entry");
        for (id, key) in &m.by_id {
            assert_eq!(m.map.get(key).map(|f| f.id), Some(*id), "the index's key holds the face with its id");
        }
    }
}

// ---- TEXTHOSTFACECACHEEVICTION (LEDGER SR1004): the instance bound and its snap (named tail block) ----

/// TEXTHOSTFACECACHEEVICTION (SR1004): the most instances (non-empty coords) one (file, index, synthetic bold,
/// synthetic oblique) family holds in the loaded-face cache (`faces_loaded`); past it [`load_face_instance`] returns
/// the nearest loaded instance ([`snap_instance`]) instead of loading another. A BOUND, not an eviction (measured):
/// every [`Face`] and its file bytes are `Box::leak`ed (nothing to free), and `&'static Face` / `Face.id` are held
/// across frames (aether's `STACKS` / `FALLBACK` / `fallback_variant`, raster's `GLYPHS` / `HINTERS`), so dropping an
/// entry would break the cache's contract; a snap inserts nothing, so the contract and `by_id` stand. A Face is its
/// inline `Font` (borrowed tables, no owned buffer) plus a family String: <= 16 x ~2.2 KB per family.
pub const INSTANCES_MAX: usize = 16;

/// TEXTHOSTFACECACHEEVICTION (SR1004): the loaded instance a miss at `coords` snaps to once its family (`path`,
/// `index`, `bold`, `oblique`, non-empty coords; the default instance is neither counted nor a target) holds
/// [`INSTANCES_MAX`] entries: the least L1 distance over the F2Dot14 coords (a missing axis as 0), ties to the lower
/// `Face.id` (the oldest). None while the family has room. The caller holds the guard; one scan, on a miss only.
fn snap_instance(m: &LoadedMap, path: &std::path::Path, index: u32, bold: bool, oblique: bool, coords: &[i16]) -> Option<&'static Face> {
    let family = move || m.iter().filter(move |(k, _)| !k.4.is_empty() && k.0.as_path() == path && k.1 == index && k.2 == bold && k.3 == oblique);
    if family().count() < INSTANCES_MAX {
        return None;
    }
    let l1 = |c: &[i16]| -> i32 {
        (0..c.len().max(coords.len())).map(|i| (*c.get(i).unwrap_or(&0) as i32 - *coords.get(i).unwrap_or(&0) as i32).abs()).sum()
    };
    family().min_by_key(|(k, f)| (l1(&k.4), f.id)).map(|(_, f)| *f)
}

/// TEXTHOSTFACECACHEEVICTION (SR1004): how many faces the loaded-face cache holds (its map's len under the ONE lock),
/// the witness metric (owed TEXTHOST-FACE-BYTES-WITNESS puts it on stria-view / aether's witness).
pub fn faces_loaded_count() -> usize {
    faces_loaded().lock().map(|m| m.map.len()).unwrap_or(0)
}

#[cfg(test)]
mod facecache_eviction_tests {
    use super::*;

    fn addr(f: &Face) -> usize {
        f as *const Face as usize
    }

    /// The (path, index, no synthesis) instance family of `info` under the lock.
    fn family_len(info: &FaceInfo) -> usize {
        let m = faces_loaded().lock().unwrap();
        m.keys().filter(|k| !k.4.is_empty() && k.0 == info.path && k.1 == info.index && !k.2 && !k.3).count()
    }

    /// Inter (a distinct `docs/..` path component, so every key is this test's alone) at 20 distinct weights 100, 140,
    /// ... 860: the first 16 load 16 objects, the family stays at INSTANCES_MAX, 740..860 snap to the 700 object (the
    /// nearest: the axis map is monotonic), 341 to the 340 object, 900 (far, the axis max) to 700 (pinned), 340 again
    /// is a plain hit; a bold instance is a new family's object; the snap inserted nothing (`by_id` == map len);
    /// `faces_loaded_count` brackets the lock's len; a Face's inline size is bounded.
    #[test]
    fn facecache_instance_bound_snap_kat() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/..").join("fonts/inter/InterVariable.ttf");
        let inter = db::faces_in_file(&p).into_iter().next().expect("the tree's Inter face");
        let at = |w: f32| load_face_instance(&inter, false, false, &[(*b"wght", w)]).expect("Inter instances");
        let weights: Vec<f32> = (0..20).map(|i| 100.0 + 40.0 * i as f32).collect();
        let faces: Vec<&'static Face> = weights.iter().map(|&w| at(w)).collect();
        let first: std::collections::HashSet<usize> = faces[..16].iter().map(|f| addr(f)).collect();
        assert_eq!(first.len(), 16, "the first INSTANCES_MAX load distinct objects");
        assert_eq!(family_len(&inter), INSTANCES_MAX, "the family is capped");
        let w700 = faces[15];
        assert_eq!(weights[15], 700.0);
        for f in &faces[16..] {
            assert!(std::ptr::eq(*f, w700), "740..860 snap to the nearest loaded instance, 700");
        }
        assert!(std::ptr::eq(at(341.0), faces[6]), "341 snaps to the 340 object");
        assert_eq!(weights[6], 340.0);
        assert!(std::ptr::eq(at(900.0), w700), "a far request still snaps (pinned)");
        assert!(std::ptr::eq(at(340.0), faces[6]), "a loaded weight is a plain hit");
        assert_eq!(family_len(&inter), INSTANCES_MAX, "snaps load nothing");
        let bold = load_face_instance(&inter, true, false, &[(*b"wght", 900.0)]).expect("Inter instances bold");
        assert!(bold.synth_bold && !faces.iter().any(|f| std::ptr::eq(*f, bold)), "the bound is per synthesis family");
        {
            let m = faces_loaded().lock().unwrap();
            assert_eq!(m.by_id.len(), m.map.len(), "one index entry per map entry");
        }
        let a = faces_loaded_count();
        let len = faces_loaded().lock().unwrap().map.len();
        let b = faces_loaded_count();
        assert!(a <= len && len <= b && a > 0, "the count is the lock's len (the map never shrinks): {a} {len} {b}");
        assert!(std::mem::size_of::<Face>() <= 2048, "a Face is {} bytes inline", std::mem::size_of::<Face>());
    }
}

// ---- TEXTHOSTFACEBYTESWITNESS (LEDGER SR1012): the face cache's count x bytes and its report line (named tail block) ----

/// TEXTHOSTFACEBYTESWITNESS (SR1012): `file_bytes`'s cache of font files (one leaked slice per path, None for an
/// unreadable one), at the file tail so [`files_loaded_bytes`] reads it; `file_bytes` names it through a `use`.
type FontFiles = Mutex<HashMap<PathBuf, Option<&'static [u8]>>>;
static FONT_FILES: OnceLock<FontFiles> = OnceLock::new();

/// TEXTHOSTFACEBYTESWITNESS (SR1012): the loaded-face cache (`faces_loaded`) and the font-file cache, counted by
/// reading: `faces` entries, of which `instances` carry coords, in `families` instance families (the (file, index,
/// synthetic bold, synthetic oblique) unit [`INSTANCES_MAX`] bounds); `face_bytes` the bytes those entries own
/// ([`face_bytes`]); `files` font files read, `file_bytes` their leaked bytes (shared by every face of a file).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FacesReport {
    pub faces: usize,
    pub instances: usize,
    pub families: usize,
    pub face_bytes: usize,
    pub files: usize,
    pub file_bytes: usize,
}

impl FacesReport {
    /// The report as one line (the devtools Performance panel's): `fonts: 23 faces (16 instances, 1 families) in
    /// 51 KB + 1840 KB files (3)`, KB rounded to the nearest 1024 bytes.
    pub fn line(&self) -> String {
        let kb = |b: usize| (b + 512) / 1024;
        format!(
            "fonts: {} faces ({} instances, {} families) in {} KB + {} KB files ({})",
            self.faces,
            self.instances,
            self.families,
            kb(self.face_bytes),
            kb(self.file_bytes),
            self.files
        )
    }
}

/// TEXTHOSTFACEBYTESWITNESS (SR1012): the bytes one loaded-face cache entry owns, by reading: the leaked `Face`
/// (inline `Font`, which owns no buffer) and its family String's heap, the map's key (inline [`FaceKey`] + the
/// PathBuf's and the coords Vec's heap) and value pointer, and `by_id`'s clone of the key and its u32. The hash
/// tables' slot overhead and the allocator's rounding are not counted; the file bytes are [`files_loaded_bytes`]'s.
fn face_bytes(face: &Face, key: &FaceKey) -> usize {
    let key_bytes = std::mem::size_of::<FaceKey>() + key.0.capacity() + key.4.capacity() * std::mem::size_of::<i16>();
    std::mem::size_of::<Face>() + face.family.capacity() + 2 * key_bytes + std::mem::size_of::<&Face>() + std::mem::size_of::<u32>()
}

/// TEXTHOSTFACEBYTESWITNESS (SR1012): the face half of a [`FacesReport`] read off the held cache (the caller holds
/// `faces_loaded`'s guard); families are counted without a set (an instance key counts when no earlier instance
/// key shares its family). The file fields stay 0.
fn report_of(l: &Loaded) -> FacesReport {
    let inst = || l.map.keys().filter(|k| !k.4.is_empty());
    let families =
        inst().enumerate().filter(|(i, k)| !inst().take(*i).any(|j| j.0 == k.0 && j.1 == k.1 && j.2 == k.2 && j.3 == k.3)).count();
    FacesReport {
        faces: l.map.len(),
        instances: inst().count(),
        families,
        face_bytes: l.map.iter().map(|(k, f)| face_bytes(f, k)).sum(),
        files: 0,
        file_bytes: 0,
    }
}

/// TEXTHOSTFACEBYTESWITNESS (SR1012): (files read, their leaked bytes) off the held font-file cache.
fn files_of(m: &HashMap<PathBuf, Option<&'static [u8]>>) -> (usize, usize) {
    m.values().flatten().fold((0usize, 0usize), |(n, b), s| (n + 1, b + s.len()))
}

/// TEXTHOSTFACEBYTESWITNESS (SR1012): the bytes of every font file read and kept for the process (`file_bytes`).
pub fn files_loaded_bytes() -> usize {
    FONT_FILES.get().and_then(|m| m.lock().ok()).map_or(0, |m| files_of(&m).1)
}

/// TEXTHOSTFACEBYTESWITNESS (SR1012): the face cache's report: its face half under `faces_loaded`'s ONE lock, then
/// its file half under the font-file cache's lock (taken one after the other, never nested), by reading only.
pub fn faces_report() -> FacesReport {
    let mut r = faces_loaded().lock().map(|l| report_of(&l)).unwrap_or_default();
    let (files, file_bytes) = FONT_FILES.get().and_then(|m| m.lock().ok()).map_or((0, 0), |m| files_of(&m));
    r.files = files;
    r.file_bytes = file_bytes;
    r
}

#[cfg(test)]
mod facebytes_witness_tests {
    use super::*;

    /// Inter through a `fonts/..` path component (PathBuf equality keeps `..`, so these keys are this test's alone).
    fn inter() -> FaceInfo {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fonts/..").join("fonts/inter/InterVariable.ttf");
        db::faces_in_file(&p).into_iter().next().expect("the tree's Inter face")
    }

    /// K loads (the base, five weight instances, the synthetic-bold base: 7 keys of this path, 5 of them instances):
    /// on ONE guard the report's counts are the map's own, its bytes the sum of `face_bytes`, the instances within
    /// SR1004's bound; this path's 7 entries own between the by-length estimate x their count and 5 % over it; the
    /// public report's count is at least this test's keys (the map never shrinks).
    #[test]
    fn facebytes_witness_report_kat() {
        let info = inter();
        load_face(&info).expect("Inter loads");
        for w in [300.0, 500.0, 600.0, 700.0, 800.0] {
            load_face_instance(&info, false, false, &[(*b"wght", w)]).expect("Inter instances");
        }
        load_face_synth(&info, true, false).expect("Inter synthetic bold");
        {
            let g = faces_loaded().lock().unwrap();
            let r = report_of(&g);
            assert_eq!(r.faces, g.map.len());
            assert_eq!(r.instances, g.map.keys().filter(|k| !k.4.is_empty()).count());
            let fams: std::collections::HashSet<(PathBuf, u32, bool, bool)> =
                g.map.keys().filter(|k| !k.4.is_empty()).map(|k| (k.0.clone(), k.1, k.2, k.3)).collect();
            assert_eq!(r.families, fams.len());
            assert_eq!(r.face_bytes, g.map.iter().map(|(k, f)| face_bytes(f, k)).sum::<usize>());
            assert!(r.instances <= r.families * INSTANCES_MAX, "SR1004's bound: {r:?}");
            assert_eq!((r.files, r.file_bytes), (0, 0), "the face half carries no file bytes");
            let mine: Vec<(&FaceKey, &&'static Face)> = g.map.iter().filter(|(k, _)| k.0 == info.path).collect();
            assert_eq!(mine.len(), 7, "base + 5 instances + synthetic bold");
            assert_eq!(mine.iter().filter(|(k, _)| !k.4.is_empty()).count(), 5);
            let est: usize = mine
                .iter()
                .map(|(k, f)| {
                    std::mem::size_of::<Face>()
                        + f.family.len()
                        + 2 * (std::mem::size_of::<FaceKey>() + k.0.as_os_str().len() + k.4.len() * 2)
                        + std::mem::size_of::<&Face>()
                        + std::mem::size_of::<u32>()
                })
                .sum();
            let got: usize = mine.iter().map(|(k, f)| face_bytes(f, k)).sum();
            assert!(est <= got && got * 100 <= est * 105, "7 entries own {got} bytes, estimate {est}");
        }
        let r = faces_report();
        assert!(r.faces >= 7 && r.instances >= 5 && r.families >= 1 && r.face_bytes > 0, "{r:?}");
        assert!(r.files >= 1 && r.file_bytes > 0, "Inter's file is counted: {r:?}");
    }

    /// The font-file cache's half: on ONE guard `files_of` is the sum of the leaked slices' lengths and their count;
    /// Inter's entry is its file's length on disk; `files_loaded_bytes` holds at least it.
    #[test]
    fn facebytes_witness_files_kat() {
        let info = inter();
        load_face(&info).expect("Inter loads");
        let disk = std::fs::metadata(&info.path).expect("Inter on disk").len() as usize;
        {
            let g = FONT_FILES.get().expect("a file was read").lock().unwrap();
            let want = g.values().filter_map(|b| *b).fold((0usize, 0usize), |(n, s), b| (n + 1, s + b.len()));
            assert_eq!(files_of(&g), want);
            assert_eq!(g.get(&info.path).copied().flatten().map(|b| b.len()), Some(disk), "the leaked slice is the file");
        }
        assert!(files_loaded_bytes() >= disk);
    }

    /// The panel line's format, pinned on literal reports (KB to the nearest 1024 bytes).
    #[test]
    fn facebytes_witness_line_kat() {
        let r = FacesReport { faces: 23, instances: 16, families: 1, face_bytes: 52_000, files: 3, file_bytes: 1_884_160 };
        assert_eq!(r.line(), "fonts: 23 faces (16 instances, 1 families) in 51 KB + 1840 KB files (3)");
        assert_eq!(FacesReport::default().line(), "fonts: 0 faces (0 instances, 0 families) in 0 KB + 0 KB files (0)");
        let r = FacesReport { face_bytes: 511, file_bytes: 512, ..FacesReport::default() };
        assert_eq!(r.line(), "fonts: 0 faces (0 instances, 0 families) in 0 KB + 1 KB files (0)");
    }
}

// ---- TEXTHOSTFILEBYTESRACE (LEDGER SR1013): one first reader per font file under the font-file lock (named tail block) ----

#[cfg(test)]
mod filebytes_race_tests {
    use super::*;

    /// Inter through a `..` prefix of this block's own (PathBuf equality keeps `..`, so the key is cold for this test).
    fn inter_via(prefix: &str) -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(prefix).join("fonts/inter/InterVariable.ttf")
    }

    /// Eight threads first-read ONE cold path at once: the map holds one entry of the file's length, every thread got
    /// that very slice (address and length), so no copy lives outside the map and `files_of` counts the path once
    /// (before SR1013 each thread read and leaked its own copy and the last `insert` replaced the others).
    #[test]
    fn filebytes_race_cold_one_slice_kat() {
        let p = inter_via("../../unaos/..");
        let disk = std::fs::metadata(&p).expect("Inter on disk").len() as usize;
        let gate = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (p, gate) = (p.clone(), gate.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    file_bytes(&p).map(|b| (b.as_ptr() as usize, b.len()))
                })
            })
            .collect();
        let got: Vec<(usize, usize)> = threads.into_iter().map(|t| t.join().unwrap().expect("Inter reads")).collect();
        {
            let g = FONT_FILES.get().expect("a file was read").lock().unwrap();
            let kept = g.get(&p).copied().flatten().expect("one entry for the path");
            assert_eq!(kept.len(), disk, "the kept slice is the file");
            assert!(got.iter().all(|&a| a == (kept.as_ptr() as usize, disk)), "one slice for every first reader: {got:?}");
            let (n, bytes) = files_of(&g);
            assert!(n >= 1 && bytes >= disk);
        }
        assert!(files_loaded_bytes() >= disk);
    }

    /// A warm path is served from the map: the same slice (address and length) as the first read and as the entry.
    #[test]
    fn filebytes_race_warm_same_slice() {
        let p = inter_via("../text_host/../..");
        let a = file_bytes(&p).expect("Inter reads");
        let b = file_bytes(&p).expect("Inter warm");
        assert!(std::ptr::eq(a, b), "a warm call reads nothing new");
        let g = FONT_FILES.get().expect("a file was read").lock().unwrap();
        assert!(std::ptr::eq(g.get(&p).copied().flatten().expect("the entry"), a));
    }
}
