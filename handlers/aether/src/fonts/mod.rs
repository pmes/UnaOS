//! Aether's text stack (AETHERFONT, SR61): font discovery, CSS font matching, shaping, measuring and glyph
//! rasterization, all on UnaOS's own `font_core` (FONTCORE SR48 + FONTBIDI SR56) — no font-kit, no
//! FreeType, no fontconfig library.
//!
//! - [`db`]: the host's faces (fontconfig's directories read as paths), family resolution as Chromium does it
//!   on Linux, css-fonts-4 §5.2 face selection, synthesis, and the per-character fallback order.
//! - [`webfont`]: `@font-face` faces (css-fonts-4 §4), fetched through the page's loader, matched before
//!   installed families.
//! - [`FontSel`]: what a text run asks for (family list id, weight, style, stretch); [`face`] its primary
//!   face, [`run_faces`] the fallback stack a text needs.
//! - [`shape`]: `font_core::shape_fallback` (bidi UAX #9, per-cluster fallback, GSUB/GPOS, kerning,
//!   ligatures) behind a cache, and the [`shape::Advancer`] the line breaker measures with.
//! - [`raster`]: glyph coverage from font_core's rasterizer in Skia's analytic-AA mode with Skia's A8
//!   pre-blend, quarter-pixel x origins, synthetic bold (FreeType's embolden, Skia's strength) and oblique
//!   (Skia's skew of −1/4).
//! - [`lines`]: the css-text-3 line breaker shared by measurer and painter.

pub mod lines;
pub mod shape;
pub mod webfont;

// QUARTZFONT (SR64): discovery, the face database, the loaded-face store and the rasterizer moved to the
// shared `libs/text_host` crate (quartzite's chrome is their second user); re-exported at their old paths.
pub use text_host::{db, fontconfig, raster};
pub use text_host::{line_metrics, load_face, load_face_synth, next_face_id, Face, Metrics};
use text_host::needs_glyph;

use db::{Slant, Style};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// What a text run asks for. `family` is a family-LIST id ([`family_list`]); the first four are fixed:
/// [`SANS`], [`SERIF`], [`MONO`] (each exactly that one generic) and [`STANDARD`] (the initial value,
/// Chromium's standard font "Times New Roman").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontSel {
    pub family: u16,
    pub weight: u16,
    pub italic: bool,
    /// font-stretch in percent.
    pub stretch: u16,
}

impl Default for FontSel {
    fn default() -> Self {
        FontSel { family: STANDARD, weight: 400, italic: false, stretch: 100 }
    }
}

impl FontSel {
    pub fn new(family: u16, weight: u16, italic: bool) -> Self {
        FontSel { family, weight, italic, stretch: 100 }
    }
    pub fn style(&self) -> Style {
        Style {
            weight: self.weight as f32,
            slant: if self.italic { Slant::Italic } else { Slant::Normal },
            stretch: self.stretch as f32,
        }
    }
}

pub const SANS: u16 = 0;
pub const SERIF: u16 = 1;
pub const MONO: u16 = 2;
pub const STANDARD: u16 = 3;

/// css-fonts-4 §4.2 `<generic-family>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Generic {
    Serif,
    SansSerif,
    Monospace,
    Cursive,
    Fantasy,
    SystemUi,
}

impl Generic {
    pub fn keyword(self) -> &'static str {
        match self {
            Generic::Serif => "serif",
            Generic::SansSerif => "sans-serif",
            Generic::Monospace => "monospace",
            Generic::Cursive => "cursive",
            Generic::Fantasy => "fantasy",
            Generic::SystemUi => "system-ui",
        }
    }
    /// The family Chromium's Linux defaults name for the generic (`WebFontFamilySettings`); `system-ui` is
    /// the desktop's font, fontconfig's `sans`.
    pub fn family_name(self) -> &'static str {
        match self {
            Generic::Serif => "Times New Roman",
            Generic::SansSerif => "Arial",
            Generic::Monospace => "Monospace",
            Generic::Cursive => "Comic Sans MS",
            Generic::Fantasy => "Impact",
            Generic::SystemUi => "sans",
        }
    }
}

/// One entry of a `font-family` list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Generic(Generic),
    Named(String),
}

/// Parses a `font-family` value (css-fonts-4 §2.1): comma-separated `<family-name>` (a string, or a
/// sequence of identifiers joined by single spaces) or `<generic-family>` keywords. None when invalid.
pub fn parse_family_list(value: &str) -> Option<Vec<Family>> {
    let mut out = Vec::new();
    for item in split_commas(value) {
        let item = item.trim();
        if item.is_empty() {
            return None;
        }
        if let Some(q) = item.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let inner = item.strip_prefix(q)?.strip_suffix(q)?;
            out.push(Family::Named(unescape_css(inner)));
            continue;
        }
        let words: Vec<String> = item.split_whitespace().map(unescape_css).collect();
        if words.len() == 1 {
            let g = match words[0].to_ascii_lowercase().as_str() {
                "serif" => Some(Generic::Serif),
                "sans-serif" => Some(Generic::SansSerif),
                "monospace" => Some(Generic::Monospace),
                "cursive" => Some(Generic::Cursive),
                "fantasy" => Some(Generic::Fantasy),
                "system-ui" => Some(Generic::SystemUi),
                // CSS-wide keywords cannot be family names
                "inherit" | "initial" | "unset" | "revert" | "revert-layer" | "default" => return None,
                _ => None,
            };
            if let Some(g) = g {
                out.push(Family::Generic(g));
                continue;
            }
        }
        out.push(Family::Named(words.join(" ")));
    }
    (!out.is_empty()).then_some(out)
}

fn split_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut q, mut start) = (None, 0);
    let b: Vec<(usize, char)> = s.char_indices().collect();
    let mut k = 0;
    while k < b.len() {
        let (i, c) = b[k];
        match (q, c) {
            (_, '\\') => k += 1,
            (None, '"' | '\'') => q = Some(c),
            (Some(o), c) if c == o => q = None,
            (None, ',') => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        k += 1;
    }
    out.push(&s[start..]);
    out
}

fn unescape_css(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && it.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
            hex.push(it.next().unwrap());
        }
        if hex.is_empty() {
            if let Some(n) = it.next() {
                out.push(n);
            }
        } else {
            if it.peek().is_some_and(|c| c.is_whitespace()) {
                it.next();
            }
            out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('\u{FFFD}'));
        }
    }
    out
}

/// Chromium's computed-value serialization of a family list (`ComputedStyleUtils::ValueForFontFamily`):
/// generics as keywords; a name unquoted when it is one CSS identifier, else a quoted string.
pub fn serialize_family_list(list: &[Family]) -> String {
    list.iter()
        .map(|f| match f {
            Family::Generic(g) => g.keyword().to_string(),
            Family::Named(n) => {
                let ident = !n.is_empty()
                    && !n.starts_with(|c: char| c.is_ascii_digit())
                    && !n.starts_with("--")
                    && !(n.starts_with('-') && n[1..].starts_with(|c: char| c.is_ascii_digit()))
                    && n.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii());
                if ident {
                    n.clone()
                } else {
                    format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\""))
                }
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

struct Registry {
    lists: Vec<Vec<Family>>,
    index: HashMap<Vec<Family>, u16>,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| {
        let lists = vec![
            vec![Family::Generic(Generic::SansSerif)],
            vec![Family::Generic(Generic::Serif)],
            vec![Family::Generic(Generic::Monospace)],
            vec![Family::Named("Times New Roman".into())],
        ];
        let index = lists.iter().enumerate().map(|(i, l)| (l.clone(), i as u16)).collect();
        Mutex::new(Registry { lists, index })
    })
}

/// The id of a family list (interned; the fixed ids for the single generics and the standard font).
pub fn intern_family_list(list: Vec<Family>) -> u16 {
    let mut r = registry().lock().unwrap();
    if let Some(&i) = r.index.get(&list) {
        return i;
    }
    if r.lists.len() >= u16::MAX as usize {
        return SANS;
    }
    let i = r.lists.len() as u16;
    r.index.insert(list.clone(), i);
    r.lists.push(list);
    i
}

/// The family list of an id.
pub fn family_list(id: u16) -> Vec<Family> {
    let r = registry().lock().unwrap();
    r.lists.get(id as usize).cloned().unwrap_or_else(|| r.lists[0].clone())
}

type FaceKey = (usize, bool, bool);

fn db_face(k: FaceKey) -> Option<&'static Face> {
    let d = db::db();
    load_face_synth(d.faces.get(k.0)?, k.1, k.2)
}

/// The face a family NAME gives for `want`: an `@font-face` family first (css-fonts-4 §4.1: author faces
/// shadow installed families of the same name), else the installed family it resolves to.
fn named_face(name: &str, want: Style) -> Option<&'static Face> {
    if let Some(f) = webfont::select(name, want) {
        return Some(f);
    }
    let d = db::db();
    let fam = d.resolve_family(name)?;
    db_face(d.select(&fam, want)?)
}

type StackCache = (u64, HashMap<FontSel, std::rc::Rc<Vec<&'static Face>>>);
type FallbackCache = (u64, HashMap<(char, u16, bool), Option<&'static Face>>);

thread_local! {
    static STACKS: std::cell::RefCell<StackCache> = std::cell::RefCell::new((0, HashMap::new()));
    static FALLBACK: std::cell::RefCell<FallbackCache> = std::cell::RefCell::new((0, HashMap::new()));
}

/// The faces of a selection's family list, in list order, each family once (absent families skipped),
/// then the standard font — Blink's `FontFallbackList` order before platform fallback.
pub fn stack(sel: &FontSel) -> std::rc::Rc<Vec<&'static Face>> {
    let generation = webfont::generation();
    if let Some(s) = STACKS.with(|c| {
        let c = c.borrow();
        (c.0 == generation).then(|| c.1.get(sel).cloned()).flatten()
    }) {
        return s;
    }
    let want = sel.style();
    let mut faces: Vec<&'static Face> = Vec::new();
    fn push(faces: &mut Vec<&'static Face>, f: Option<&'static Face>) {
        if let Some(f) = f {
            if !faces.iter().any(|x| x.id == f.id) {
                faces.push(f);
            }
        }
    }
    for fam in family_list(sel.family) {
        match fam {
            Family::Generic(g) => push(&mut faces, named_face(g.family_name(), want)),
            Family::Named(n) => {
                // an author family contributes every face of its unicode-range subsets
                let web = webfont::select_all(&n, want);
                if web.is_empty() {
                    push(&mut faces, named_face(&n, want));
                }
                for f in web {
                    push(&mut faces, Some(f));
                }
            }
        }
    }
    push(&mut faces, named_face(Generic::Serif.family_name(), want));
    if faces.is_empty() {
        // Blink's last resort (FontCache::GetLastResortFallbackFont): "Sans", then "Arial".
        push(&mut faces, named_face("sans", want));
        push(&mut faces, named_face("Arial", want));
    }
    if faces.is_empty() {
        let d = db::db();
        push(&mut faces, d.fallback_order().first().and_then(|&i| load_face(&d.faces[i])));
    }
    let rc = std::rc::Rc::new(faces);
    STACKS.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != generation {
            *c = (generation, HashMap::new());
        }
        c.1.insert(*sel, rc.clone());
    });
    rc
}

/// The primary face of a selection (its first available family).
pub fn face(sel: &FontSel) -> Option<&'static Face> {
    stack(sel).first().copied()
}

/// Platform fallback for one character (`gfx::GetFontForCharacter`): the first face in fontconfig's
/// `sans-serif` order whose cmap has it, as the style-matched face of its family (synthesized when the
/// family lacks the weight/slant).
pub fn fallback_for(c: char, sel: &FontSel) -> Option<&'static Face> {
    let key = (c, sel.weight, sel.italic);
    let generation = webfont::generation();
    if let Some(r) = FALLBACK.with(|m| {
        let m = m.borrow();
        (m.0 == generation).then(|| m.1.get(&key).copied()).flatten()
    }) {
        return r;
    }
    let d = db::db();
    let mut found = None;
    for &i in d.fallback_order() {
        let info = &d.faces[i];
        let Some(f) = load_face(info) else { continue };
        if f.font.glyph_index(c) == 0 {
            continue;
        }
        let fam = info.families[0].clone();
        found = d.select(&fam, sel.style()).and_then(db_face).filter(|g| g.font.glyph_index(c) != 0).or(Some(f));
        break;
    }
    // FONTHINT (SR62): Chromium draws a platform-fallback face with the default render params (hinting off
    // under --font-render-hinting=none), not the face's fontconfig hintstyle — measured: Loma reached as the
    // Thai fallback of `sans-serif` renders unhinted, the same face named directly renders hintslight. A distinct
    // face id (not among the installed faces) carries that through the glyph caches.
    let found = found.map(fallback_variant);
    FALLBACK.with(|m| {
        let mut m = m.borrow_mut();
        if m.0 != generation {
            *m = (generation, HashMap::new());
        }
        m.1.insert(key, found);
    });
    found
}

/// The fallback-use twin of an installed face: same font and synthesis, its own id, never hinted.
fn fallback_variant(face: &'static Face) -> &'static Face {
    static V: OnceLock<Mutex<HashMap<u32, &'static Face>>> = OnceLock::new();
    let m = V.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut m) = m.lock() else { return face };
    m.entry(face.id).or_insert_with(|| {
        Box::leak(Box::new(Face {
            id: next_face_id(),
            font: face.font,
            synth_bold: face.synth_bold,
            synth_oblique: face.synth_oblique,
            family: face.family.clone(),
            style: face.style,
        }))
    })
}

/// The face list `text` is shaped over: the selection's stack, then (in fallback order) a platform face for
/// each character no face of the stack has.
pub fn run_faces(sel: &FontSel, text: &str) -> Vec<&'static Face> {
    let st = stack(sel);
    let mut faces: Vec<&'static Face> = st.iter().copied().collect();
    let n = faces.len();
    for c in text.chars() {
        if !needs_glyph(c) || faces.iter().any(|f| f.font.glyph_index(c) != 0) {
            continue;
        }
        if let Some(f) = fallback_for(c, sel) {
            if !faces[n..].iter().any(|x| x.id == f.id) {
                faces.push(f);
            }
        }
    }
    faces
}

/// The used line height: `mult` x size, `-mult` px when negative (a length), or (0 = `normal`) the rounded
/// metrics sum.
pub fn line_height(font: &Face, size: f32, mult: f32) -> f32 {
    if mult < 0.0 {
        -mult
    } else if mult > 0.0 {
        size * mult
    } else {
        let (a, d, g) = line_metrics(font, size);
        a + d + g
    }
}

/// Offset from a line box's top to its baseline: the half-leading model of CSS 2.2 §10.8.1 — the glyph area
/// (A + D) is centred in the line box, so (line-height - (A + D)) / 2 sits above the ascent.
pub fn baseline_offset(font: &Face, size: f32, mult: f32) -> f32 {
    let (a, d, _) = line_metrics(font, size);
    let lh = line_height(font, size, mult);
    ((lh - (a + d)) / 2.0).floor() + a
}

/// Text-decoration geometry at a size, in px: the underline's top below the baseline, the line-through's
/// top above it, and the shared thickness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecoMetrics {
    pub underline_offset: f32,
    pub line_through_offset: f32,
    pub thickness: f32,
}

/// css-text-decor-3/4 §2–3 as Blink paints `text-decoration-thickness: auto` and `text-underline-position:
/// auto` (measured against Chromium on 6 faces × 12 sizes, AETHERFONT.md "decorations"): the thickness is
/// max(1, size/10) — the face's `post` underline metrics are not consulted — painted as whole rows (floor);
/// the underline's top sits max(1, ⌈thickness/2⌉) below the baseline; the line-through's top is
/// round(−ascent/3 − thickness/2) from the baseline, with the ascent in whole pixels.
pub fn decoration_metrics(face: &Face, size: f32) -> DecoMetrics {
    let th = (size / 10.0).max(1.0);
    let thickness = th.floor().max(1.0);
    let underline_offset = (th / 2.0).ceil().max(1.0);
    let (asc, _, _) = line_metrics(face, size);
    let lt_top = (-(asc / 3.0) - th / 2.0 + 0.5).floor();
    DecoMetrics { underline_offset, line_through_offset: -lt_top, thickness }
}

/// Advance of U+0020 in px (its advance in the face).
pub fn space_advance(font: &Face, size: f32) -> f32 {
    font.glyph_for_char(' ').map(|g| font.advance_em(g) * size).filter(|w| *w > 0.0).unwrap_or(size * 0.25)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chromium's decoration rows (measured; AETHERFONT.md): Liberation Sans 16/28/48 px.
    #[test]
    fn decoration_metrics_kat() {
        let Some(f) = face(&FontSel::new(SANS, 400, false)) else { return };
        if f.family != "Liberation Sans" {
            return;
        }
        let m = |s: f32| {
            let d = decoration_metrics(f, s);
            (d.underline_offset, d.thickness, -d.line_through_offset)
        };
        assert_eq!(m(16.0), (1.0, 1.0, -5.0));
        assert_eq!(m(28.0), (2.0, 2.0, -10.0));
        assert_eq!(m(48.0), (3.0, 4.0, -17.0));
        assert_eq!(m(10.0), (1.0, 1.0, -3.0));
    }

    #[test]
    fn family_list_parse_and_serialize_kat() {
        let l = parse_family_list(r#""Helvetica Neue",  Arial , sans-serif"#).unwrap();
        assert_eq!(
            l,
            vec![Family::Named("Helvetica Neue".into()), Family::Named("Arial".into()), Family::Generic(Generic::SansSerif)]
        );
        assert_eq!(serialize_family_list(&l), r#""Helvetica Neue", Arial, sans-serif"#);
        // identifier sequences collapse to single spaces; a quoted generic is a name
        let l = parse_family_list("Times   New Roman, 'serif', monospace").unwrap();
        assert_eq!(l[0], Family::Named("Times New Roman".into()));
        assert_eq!(l[1], Family::Named("serif".into()));
        assert_eq!(l[2], Family::Generic(Generic::Monospace));
        assert_eq!(serialize_family_list(&[Family::Named("Times New Roman".into())]), "\"Times New Roman\"");
        assert!(parse_family_list("inherit").is_none());
        assert!(parse_family_list("a,,b").is_none());
        assert_eq!(intern_family_list(vec![Family::Generic(Generic::Monospace)]), MONO);
        let id = intern_family_list(vec![Family::Generic(Generic::Monospace), Family::Generic(Generic::Monospace)]);
        assert!(id > STANDARD, "`monospace, monospace` is not the single generic");
    }

    #[test]
    fn stacks_resolve_like_chromium() {
        if db::db().faces.is_empty() {
            return;
        }
        let fam = |id: u16| face(&FontSel::new(id, 400, false)).map(|f| f.family.clone());
        if db::db().resolve_family("Liberation Serif").is_some() {
            assert_eq!(fam(STANDARD).as_deref(), Some("Liberation Serif"));
            assert_eq!(fam(SERIF).as_deref(), Some("Liberation Serif"));
            let unknown = intern_family_list(vec![Family::Named("No Such Face".into())]);
            assert_eq!(fam(unknown).as_deref(), Some("Liberation Serif"), "no family matched: the standard font");
        }
        if db::db().resolve_family("Liberation Sans").is_some() {
            assert_eq!(fam(SANS).as_deref(), Some("Liberation Sans"));
            let bold = face(&FontSel::new(SANS, 700, false)).unwrap();
            assert_eq!((bold.style.weight, bold.synth_bold), (700.0, false));
        }
        // synthesis where the family has no bold/italic face (WenQuanYi Zen Hei, OS/2 weight 500): Blink
        // emboldens only when the wanted weight exceeds the face's by more than 200
        if db::db().resolve_family("WenQuanYi Zen Hei").is_some() {
            let wqy = intern_family_list(vec![Family::Named("WenQuanYi Zen Hei".into())]);
            let f = face(&FontSel::new(wqy, 700, true)).unwrap();
            assert!(!f.synth_bold && f.synth_oblique, "{f:?}");
            let f = face(&FontSel::new(wqy, 900, false)).unwrap();
            assert!(f.synth_bold && !f.synth_oblique, "{f:?}");
            // CJK falls back to it from a Latin face
            let r = run_faces(&FontSel::new(SANS, 400, false), "a世");
            assert!(r.iter().any(|f| f.family == "WenQuanYi Zen Hei"), "{r:?}");
        }
    }
}
