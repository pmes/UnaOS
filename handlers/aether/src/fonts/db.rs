//! The font database (AETHERFONT, SR61): every outline face under the host's font directories, read with
//! `font_core` (sfnt + `name` + `OS/2`), indexed by family name, and the CSS Fonts 4 face selection over it.
//!
//! - Discovery: fontconfig's `<dir>`s ([`super::fontconfig::Config`]) walked for `.ttf/.otf/.ttc/.otc`; each
//!   face of a collection is a record. Bitmap-only faces (no `glyf`/`CFF`, e.g. CBDT emoji) and Type 1 files
//!   are not readable by font_core and are skipped.
//! - Family lookup (Chromium on Linux, `FontCache::GetFontPlatformData` → Skia's fontconfig interface): an
//!   installed family of that name; else, for the names Skia lets fontconfig substitute freely (`sans`,
//!   `serif`, `monospace`), fontconfig's first installed alias; else a metric-compatible alias only
//!   (Skia's `FontEquivClass`: Arial ≡ Liberation Sans ≡ Arimo, Times New Roman ≡ Liberation Serif ≡ Tinos,
//!   …), found through fontconfig's alias expansion. Any other substitute is refused, so the next family
//!   of the CSS list is tried.
//! - Generic families map to Chromium's Linux defaults (`standard`/`serif` "Times New Roman",
//!   `sans-serif` "Arial", `monospace` "Monospace", `cursive` "Comic Sans MS", `fantasy` "Impact").
//! - Face selection inside a family: css-fonts-4 §5.2 (font-stretch, then font-style, then font-weight),
//!   then synthesis as Blink decides it: bold when the wanted weight exceeds the face's by more than 200,
//!   oblique when italic/oblique is wanted and the face is upright.
//! - Per-character system fallback: the faces in the order fontconfig sorts `sans-serif` (its alias rules
//!   applied in configuration order), which is the list `gfx::GetFontForCharacter` walks for a character
//!   none of the CSS families covers.

use super::fontconfig::{family_eq, Config};
use font_core::Font;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// font-style, as matched (css-fonts-4 §2.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Slant {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// The selection-relevant facts of one face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub weight: f32,
    pub slant: Slant,
    /// font-stretch as a percentage (usWidthClass 1..9 → 50..200 %).
    pub stretch: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style { weight: 400.0, slant: Slant::Normal, stretch: 100.0 }
    }
}

/// One discovered face.
#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub path: PathBuf,
    pub index: u32,
    /// Family names (name IDs 1 and 16, every language), English first.
    pub families: Vec<String>,
    /// Full names (4) and PostScript names (6), for `@font-face local()`.
    pub full_names: Vec<String>,
    pub style: Style,
}

/// usWidthClass → percentage (OpenType OS/2).
pub fn width_class_percent(w: u16) -> f32 {
    match w {
        1 => 50.0,
        2 => 62.5,
        3 => 75.0,
        4 => 87.5,
        6 => 112.5,
        7 => 125.0,
        8 => 150.0,
        9 => 200.0,
        _ => 100.0,
    }
}

/// The selection style of a parsed face: OS/2 weight/width; italic from fsSelection bit 0 (or head.macStyle
/// bit 1), oblique from fsSelection bit 9.
pub fn style_of(f: &Font) -> Style {
    let (weight, stretch, sel) = match f.os2 {
        Some(o) => (o.weight_class, o.width_class, o.fs_selection),
        None => (if f.mac_style & 1 != 0 { 700 } else { 400 }, 5, 0),
    };
    let italic = sel & 1 != 0 || f.mac_style & 2 != 0;
    let slant = if sel & (1 << 9) != 0 {
        Slant::Oblique
    } else if italic {
        Slant::Italic
    } else {
        Slant::Normal
    };
    // Some legacy fonts store weight classes 1..9.
    let weight = if (1..=9).contains(&weight) { weight * 100 } else { weight.clamp(1, 1000) };
    Style { weight: weight as f32, slant, stretch: width_class_percent(stretch) }
}

/// Reads every face of one font file.
pub fn faces_in_file(path: &Path) -> Vec<FaceInfo> {
    let Ok(data) = std::fs::read(path) else { return Vec::new() };
    faces_in_bytes(&data, path)
}

pub fn faces_in_bytes(data: &[u8], path: &Path) -> Vec<FaceInfo> {
    let mut out = Vec::new();
    for i in 0..Font::face_count(data).min(64) {
        let Ok(f) = Font::parse_face(data, i) else { continue };
        if matches!(f.outlines, font_core::Outlines::None) {
            continue;
        }
        let families = font_core::name::family_names(&f);
        if families.is_empty() {
            continue;
        }
        let mut full_names = font_core::name::values(&f, font_core::name::FULL_NAME);
        full_names.extend(font_core::name::values(&f, font_core::name::POSTSCRIPT));
        out.push(FaceInfo { path: path.to_path_buf(), index: i, families, full_names, style: style_of(&f) });
    }
    out
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 16 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk_dir(&p, out, depth + 1);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc"))
        {
            out.push(p);
        }
    }
}

/// css-fonts-4 §5.2 step 4: the index of the face that best matches `want` (None when `cands` is empty).
pub fn match_style(cands: &[Style], want: Style) -> Option<usize> {
    if cands.is_empty() {
        return None;
    }
    let mut idx: Vec<usize> = (0..cands.len()).collect();
    // font-stretch: ≤ 100% prefers narrower (closest first), then wider; > 100% the reverse.
    let pick_stretch = |idx: &[usize]| -> f32 {
        let w = want.stretch;
        let narrower = idx.iter().map(|&i| cands[i].stretch).filter(|&s| s <= w).fold(None, |a: Option<f32>, s| Some(a.map_or(s, |a| a.max(s))));
        let wider = idx.iter().map(|&i| cands[i].stretch).filter(|&s| s > w).fold(None, |a: Option<f32>, s| Some(a.map_or(s, |a| a.min(s))));
        if w <= 100.0 { narrower.or(wider).unwrap() } else { wider.or(narrower).unwrap() }
    };
    let s = pick_stretch(&idx);
    idx.retain(|&i| cands[i].stretch == s);
    // font-style: italic → italic, oblique, normal; oblique → oblique, italic, normal; normal → normal,
    // oblique, italic.
    let order: [Slant; 3] = match want.slant {
        Slant::Italic => [Slant::Italic, Slant::Oblique, Slant::Normal],
        Slant::Oblique => [Slant::Oblique, Slant::Italic, Slant::Normal],
        Slant::Normal => [Slant::Normal, Slant::Oblique, Slant::Italic],
    };
    if let Some(sl) = order.iter().find(|sl| idx.iter().any(|&i| cands[i].slant == **sl)) {
        idx.retain(|&i| cands[i].slant == *sl);
    }
    // font-weight.
    let w = want.weight;
    let ws: Vec<(usize, f32)> = idx.iter().map(|&i| (i, cands[i].weight)).collect();
    if let Some(&(i, _)) = ws.iter().find(|(_, x)| *x == w) {
        return Some(i);
    }
    let below = |lim: f32| ws.iter().filter(|(_, x)| *x < lim).max_by(|a, b| a.1.total_cmp(&b.1)).map(|p| p.0);
    let above = |lim: f32| ws.iter().filter(|(_, x)| *x > lim).min_by(|a, b| a.1.total_cmp(&b.1)).map(|p| p.0);
    let r = if (400.0..=500.0).contains(&w) {
        ws.iter()
            .filter(|(_, x)| *x > w && *x <= 500.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|p| p.0)
            .or_else(|| below(w))
            .or_else(|| above(500.0))
    } else if w < 400.0 {
        below(w).or_else(|| above(w))
    } else {
        above(w).or_else(|| below(w))
    };
    r.or(Some(idx[0]))
}

/// Blink's synthesis decision for a chosen face: (synthetic bold, synthetic oblique).
pub fn synthesis(face: Style, want: Style) -> (bool, bool) {
    let bold = want.weight > face.weight + 200.0;
    let oblique = want.slant != Slant::Normal && face.slant == Slant::Normal;
    (bold, oblique)
}

/// Skia's `FontEquivClass` (SkFontConfigInterface_direct.cpp): families whose metrics are interchangeable,
/// so a fontconfig substitute within the class is accepted.
const EQUIV: &[&[&str]] = &[
    &["Arial", "Arimo", "Liberation Sans"],
    &["Times New Roman", "Tinos", "Liberation Serif"],
    &["Courier New", "Cousine", "Liberation Mono"],
    &["Symbol", "Symbol Neu"],
    &["MS PGothic", "ＭＳ Ｐゴシック", "Noto Sans CJK JP", "IPAPGothic", "MotoyaG04Gothic"],
    &["MS Gothic", "ＭＳ ゴシック", "Noto Sans Mono CJK JP", "IPAGothic", "MotoyaG04GothicMono"],
    &["MS PMincho", "ＭＳ Ｐ明朝", "IPAPMincho", "MotoyaG04Mincho"],
    &["MS Mincho", "ＭＳ 明朝", "IPAMincho", "MotoyaG04MinchoMono"],
    &["SimSun", "宋体", "Song ASC"],
    &["NSimSun", "新宋体", "Song ASC"],
    &["SimHei", "黑体", "Noto Sans CJK SC", "Source Han Sans SC"],
    &["PMingLiU", "新細明體", "AR PL UMing TW"],
    &["MingLiU", "細明體", "AR PL UMing TW MBE"],
    &["Cambria", "Caladea"],
    &["Calibri", "Carlito"],
];

fn equivalent(a: &str, b: &str) -> bool {
    EQUIV.iter().any(|class| class.iter().any(|x| family_eq(x, a)) && class.iter().any(|x| family_eq(x, b)))
}

/// A family name Skia lets fontconfig substitute for freely (`IsFallbackFontAllowed`).
fn substitutable(name: &str) -> bool {
    ["sans", "serif", "monospace"].iter().any(|g| name.eq_ignore_ascii_case(g))
}

/// The database: faces, config, and the derived orders.
pub struct FontDb {
    pub faces: Vec<FaceInfo>,
    pub config: Config,
    /// Family-name resolution cache: lower-cased CSS name → canonical installed family (or none).
    resolved: Mutex<HashMap<String, Option<String>>>,
    /// The per-character fallback order (face indices): fontconfig's sort of `sans-serif`.
    fallback: OnceLock<Vec<usize>>,
}

impl FontDb {
    /// Builds the database from fontconfig's directories (once per process: [`db`]).
    pub fn discover(config: Config) -> FontDb {
        let mut files = Vec::new();
        for d in &config.dirs {
            walk_dir(d, &mut files, 0);
        }
        files.dedup();
        let mut faces = Vec::new();
        for f in &files {
            faces.extend(faces_in_file(f));
        }
        FontDb { faces, config, resolved: Mutex::new(HashMap::new()), fallback: OnceLock::new() }
    }

    /// A database over explicit faces (tests).
    pub fn from_faces(faces: Vec<FaceInfo>, config: Config) -> FontDb {
        FontDb { faces, config, resolved: Mutex::new(HashMap::new()), fallback: OnceLock::new() }
    }

    /// The faces of an installed family (exact name, fontconfig comparison).
    pub fn family_faces(&self, family: &str) -> Vec<usize> {
        (0..self.faces.len()).filter(|&i| self.faces[i].families.iter().any(|f| family_eq(f, family))).collect()
    }

    fn installed(&self, family: &str) -> Option<String> {
        self.faces
            .iter()
            .find_map(|f| f.families.iter().find(|n| family_eq(n, family)).cloned())
    }

    /// The installed family a CSS family NAME resolves to (see the module comment), or None when Chromium
    /// would move on to the next family of the list.
    pub fn resolve_family(&self, name: &str) -> Option<String> {
        let key = name.to_lowercase();
        if let Some(r) = self.resolved.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return r;
        }
        let r = self.installed(name).or_else(|| {
            let expanded = self.config.expand(&[name]);
            expanded.iter().find_map(|cand| {
                let inst = self.installed(cand)?;
                (substitutable(name) || equivalent(name, &inst)).then_some(inst)
            })
        });
        if let Ok(mut m) = self.resolved.lock() {
            m.insert(key, r.clone());
        }
        r
    }

    /// The best face of `family` for `want`, with its synthesis flags.
    pub fn select(&self, family: &str, want: Style) -> Option<(usize, bool, bool)> {
        let idx = self.family_faces(family);
        let styles: Vec<Style> = idx.iter().map(|&i| self.faces[i].style).collect();
        let k = match_style(&styles, want)?;
        let (b, o) = synthesis(styles[k], want);
        Some((idx[k], b, o))
    }

    /// Faces in fontconfig's `sans-serif` sort order: by the first family of the alias-expanded pattern each
    /// face answers to; faces answering to none follow, those covering basic Latin (fontconfig's `en` lang
    /// test) first, then in discovery order.
    pub fn fallback_order(&self) -> &[usize] {
        self.fallback.get_or_init(|| {
            let expanded = self.config.expand(&["sans-serif"]);
            let rank = |f: &FaceInfo| -> usize {
                f.families
                    .iter()
                    .filter_map(|n| expanded.iter().position(|e| family_eq(e, n)))
                    .min()
                    .unwrap_or(usize::MAX)
            };
            let mut idx: Vec<(usize, usize, usize, usize)> = self
                .faces
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let r = rank(f);
                    // inside one family the regular face first (the pattern asks for weight 400, roman)
                    let st = (f.style.weight - 400.0).abs() as usize + if f.style.slant == Slant::Normal { 0 } else { 1000 };
                    (r, if r == usize::MAX { latin_rank(f) } else { 0 }, st, i)
                })
                .collect();
            idx.sort();
            idx.into_iter().map(|t| t.3).collect()
        })
    }
}

/// 0 when the face's file covers a–z (fontconfig's `en` orthography), else 1.
fn latin_rank(f: &FaceInfo) -> usize {
    let Some(face) = super::load_face(f) else { return 2 };
    if ('a'..='z').all(|c| face.font.glyph_index(c) != 0) { 0 } else { 1 }
}

/// The process-wide database, discovered on first use.
pub fn db() -> &'static FontDb {
    static DB: OnceLock<FontDb> = OnceLock::new();
    DB.get_or_init(|| FontDb::discover(Config::system()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(weight: f32, slant: Slant, stretch: f32) -> Style {
        Style { weight, slant, stretch }
    }

    /// css-fonts-4 §5.2 known answers.
    #[test]
    fn match_style_kat() {
        let n = Slant::Normal;
        let fam = [st(300.0, n, 100.0), st(400.0, n, 100.0), st(500.0, n, 100.0), st(700.0, n, 100.0), st(900.0, n, 100.0)];
        let pick = |w: f32| fam[match_style(&fam, st(w, n, 100.0)).unwrap()].weight;
        assert_eq!(pick(400.0), 400.0);
        assert_eq!(pick(450.0), 500.0, "400..500: heavier up to 500 first");
        assert_eq!(pick(600.0), 700.0, "> 500: heavier first");
        assert_eq!(pick(950.0), 900.0, "then lighter");
        assert_eq!(pick(350.0), 300.0, "< 400: lighter first");
        assert_eq!(pick(200.0), 300.0, "then heavier");
        let two = [st(300.0, n, 100.0), st(700.0, n, 100.0)];
        assert_eq!(two[match_style(&two, st(450.0, n, 100.0)).unwrap()].weight, 300.0, "450: lighter before > 500");
        // style: italic falls back to oblique before normal; normal prefers oblique over italic
        let sl = [st(400.0, Slant::Normal, 100.0), st(400.0, Slant::Oblique, 100.0), st(400.0, Slant::Italic, 100.0)];
        assert_eq!(sl[match_style(&sl[..2], st(400.0, Slant::Italic, 100.0)).unwrap()].slant, Slant::Oblique);
        let oi = [sl[1], sl[2]];
        assert_eq!(oi[match_style(&oi, st(400.0, Slant::Normal, 100.0)).unwrap()].slant, Slant::Oblique);
        // stretch is decided before style and weight: ≤ 100% narrower first, > 100% wider first
        let sw = [st(400.0, n, 75.0), st(700.0, n, 100.0), st(400.0, n, 125.0)];
        assert_eq!(sw[match_style(&sw, st(400.0, n, 87.5)).unwrap()].stretch, 75.0);
        assert_eq!(sw[match_style(&sw, st(400.0, n, 112.5)).unwrap()].stretch, 125.0);
        assert_eq!(sw[match_style(&sw, st(400.0, n, 100.0)).unwrap()].weight, 700.0, "the only 100% face");
        // Blink's synthesis
        assert_eq!(synthesis(st(400.0, n, 100.0), st(700.0, Slant::Italic, 100.0)), (true, true));
        assert_eq!(synthesis(st(500.0, n, 100.0), st(700.0, n, 100.0)), (false, false), "700 is not > 500 + 200");
        assert_eq!(synthesis(st(400.0, Slant::Italic, 100.0), st(400.0, Slant::Oblique, 100.0)), (false, false));
    }

    #[test]
    fn style_of_dejavu() {
        let Ok(d) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf") else { return };
        let f = Font::parse(&d).unwrap();
        assert_eq!(style_of(&f), st(700.0, Slant::Normal, 100.0));
        let Ok(d) = std::fs::read("/usr/share/fonts/truetype/liberation/LiberationSerif-Italic.ttf") else { return };
        assert_eq!(style_of(&Font::parse(&d).unwrap()).slant, Slant::Italic);
    }

    /// Family resolution as Chromium does it on this host, and the fallback order against `fc-match -s`.
    #[test]
    fn resolution_and_fallback_vs_fontconfig() {
        let db = db();
        if db.faces.is_empty() {
            return;
        }
        let has = |f: &str| db.installed(f).is_some();
        if has("Liberation Sans") {
            assert_eq!(db.resolve_family("Arial").as_deref(), Some("Liberation Sans"), "metric-compatible");
            assert_eq!(db.resolve_family("arial").as_deref(), Some("Liberation Sans"));
        }
        if has("Liberation Serif") {
            assert_eq!(db.resolve_family("Times New Roman").as_deref(), Some("Liberation Serif"));
        }
        if has("DejaVu Sans Mono") {
            assert_eq!(db.resolve_family("Monospace").as_deref(), Some("DejaVu Sans Mono"), "fontconfig's monospace");
        }
        assert_eq!(db.resolve_family("Helvetica Neue"), None, "a non-equivalent substitute is refused");
        assert_eq!(db.resolve_family("NoSuchFamily"), None);
        // fc-match -s sans-serif is the oracle for the fallback order (skipped without fontconfig's tools).
        let Ok(out) = std::process::Command::new("fc-match").args(["-s", "--format", "%{file}#%{index}\\n", "sans-serif"]).output()
        else {
            return;
        };
        let want: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| db.faces.iter().any(|f| format!("{}#{}", f.path.display(), f.index) == *l))
            .map(str::to_string)
            .collect();
        let ours: Vec<String> =
            db.fallback_order().iter().map(|&i| format!("{}#{}", db.faces[i].path.display(), db.faces[i].index)).collect();
        let n = 6.min(want.len());
        let fam = |s: &str| {
            db.faces.iter().find(|f| format!("{}#{}", f.path.display(), f.index) == s).map(|f| f.families[0].clone()).unwrap_or_default()
        };
        let wf: Vec<String> = want.iter().take(n).map(|s| fam(s)).collect();
        let mut of: Vec<String> = Vec::new();
        for s in &ours {
            let f = fam(s);
            if of.last() != Some(&f) {
                of.push(f);
            }
        }
        let mut wd: Vec<String> = Vec::new();
        for f in wf {
            if wd.last() != Some(&f) {
                wd.push(f);
            }
        }
        assert_eq!(&of[..wd.len()], &wd[..], "family order of the first {n} fc-match -s sans-serif faces");
    }
}
