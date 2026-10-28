//! The font database (AETHERFONT, SR61): every outline face under the host's font directories, read with
//! `font_core` (sfnt + `name` + `OS/2`), indexed by family name, and the CSS Fonts 4 face selection over it.
//!
//! - Discovery: fontconfig's `<dir>`s ([`crate::fontconfig::Config`]) walked for `.ttf/.otf/.ttc/.otc`; each
//!   face of a collection is a record. Bitmap-only faces (no `glyf`/`CFF`, e.g. CBDT emoji) and Type 1 files
//!   are not readable by font_core and are skipped.
//! - Family lookup (Chromium on Linux, `FontCache::GetFontPlatformData` → Skia's fontconfig interface): an
//!   installed family of that name; else, for the names Skia lets fontconfig substitute freely (`sans`,
//!   `serif`, `monospace`), fontconfig's first installed alias; else a metric-compatible alias only
//!   (Skia's `FontEquivClass`: Arial ≡ Liberation Sans ≡ Arimo, Times New Roman ≡ Liberation Serif ≡ Tinos,
//!   …), found through fontconfig's alias expansion. Any other substitute is refused, so the next family
//!   of the CSS list is tried.
//! - Generic families map through ONE per-OS table, [`generics_for`] (AETHER-MACGENERICS, SR418): on Linux
//!   Chromium's Linux defaults (`standard`/`serif` "Times New Roman", `sans-serif` "Arial", `monospace`
//!   "Monospace", `cursive` "Comic Sans MS", `fantasy` "Impact", `system-ui` "sans"), on macOS Chromium-mac's
//!   (Times, Helvetica, Courier, Apple Chancery, Papyrus, the SF system face), each a candidate list tried
//!   through [`FontDb::resolve_family`].
//! - Face selection inside a family: css-fonts-4 §5.2 (font-stretch, then font-style, then font-weight),
//!   then synthesis as Blink decides it: bold when the wanted weight exceeds the face's by more than 200,
//!   oblique when italic/oblique is wanted and the face is upright.
//! - Per-character system fallback: the faces in the order fontconfig sorts `sans-serif` (its alias rules
//!   applied in configuration order), which is the list `gfx::GetFontForCharacter` walks for a character
//!   none of the CSS families covers.

use crate::fontconfig::{family_eq, Config};
use font_core::Font;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Indexes every face of one font file from its headers only (AETHER-FONTDB-LAZY (SR419): [`faces_in_header`]);
/// the full bytes load on first use through [`crate::load_face`].
pub fn faces_in_file(path: &Path) -> Vec<FaceInfo> {
    faces_in_header(path).0
}

/// Every face of a font file held whole in memory.
pub fn faces_in_bytes(data: &[u8], path: &Path) -> Vec<FaceInfo> {
    let mut out = Vec::new();
    for i in 0..Font::face_count(data).min(64) {
        let Ok(f) = Font::parse_face(data, i) else { continue };
        if matches!(f.outlines, font_core::Outlines::None) {
            continue;
        }
        out.extend(face_record(&f, path, i));
    }
    out
}

/// The index record of one parsed face (AETHER-FONTDB-LAZY (SR419): the ONE builder both the whole-file and the
/// header path use): family names (IDs 1 and 16), full and PostScript names, the selection style.
fn face_record(f: &Font, path: &Path, index: u32) -> Option<FaceInfo> {
    let families = font_core::name::family_names(f);
    if families.is_empty() {
        return None;
    }
    let mut full_names = font_core::name::values(f, font_core::name::FULL_NAME);
    full_names.extend(font_core::name::values(f, font_core::name::POSTSCRIPT));
    Some(FaceInfo { path: path.to_path_buf(), index, families, full_names, style: style_of(f) })
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
        let narrower = idx
            .iter()
            .map(|&i| cands[i].stretch)
            .filter(|&s| s <= w)
            .fold(None, |a: Option<f32>, s| Some(a.map_or(s, |a| a.max(s))));
        let wider = idx
            .iter()
            .map(|&i| cands[i].stretch)
            .filter(|&s| s > w)
            .fold(None, |a: Option<f32>, s| Some(a.map_or(s, |a| a.min(s))));
        if w <= 100.0 {
            narrower.or(wider).unwrap()
        } else {
            wider.or(narrower).unwrap()
        }
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
    /// AETHER-FONTDB-LAZY (SR419): the bytes `discover` read to build the index (0 for [`FontDb::from_faces`]).
    bytes_read: u64,
    /// AETHERFONTDBFALLBACK (SR425): each face's `cmap` table alone, read on first need by [`FontDb::covers`]
    /// (None = file unreadable, empty = no cmap), and the bytes those bounded reads took.
    cmaps: Vec<OnceLock<Option<Box<[u8]>>>>,
    scan_bytes: AtomicU64,
}

impl FontDb {
    /// Builds the database from fontconfig's directories (once per process: [`db`]).
    /// AETHER-FONTDB-LAZY (SR419): each file is indexed from its headers ([`faces_in_header`]), never read whole.
    pub fn discover(config: Config) -> FontDb {
        let t0 = std::time::Instant::now();
        let mut files = Vec::new();
        for d in &config.dirs {
            walk_dir(d, &mut files, 0);
        }
        files.dedup();
        let mut faces = Vec::new();
        let mut bytes_read = 0u64;
        for f in &files {
            let (v, n) = faces_in_header(f);
            faces.extend(v);
            bytes_read += n;
        }
        eprintln!(
            "[fonts] discover files={} bytes_read={} faces={} ms={}",
            files.len(),
            bytes_read / 1024,
            faces.len(),
            t0.elapsed().as_millis()
        );
        let (cmaps, scan_bytes) = ((0..faces.len()).map(|_| OnceLock::new()).collect(), AtomicU64::new(0)); // AETHERFONTDBFALLBACK (SR425)
        FontDb { faces, config, resolved: Mutex::new(HashMap::new()), fallback: OnceLock::new(), bytes_read, cmaps, scan_bytes }
    }

    /// A database over explicit faces (tests).
    pub fn from_faces(faces: Vec<FaceInfo>, config: Config) -> FontDb {
        let (cmaps, scan_bytes) = ((0..faces.len()).map(|_| OnceLock::new()).collect(), AtomicU64::new(0)); // AETHERFONTDBFALLBACK (SR425)
        FontDb { faces, config, resolved: Mutex::new(HashMap::new()), fallback: OnceLock::new(), bytes_read: 0, cmaps, scan_bytes }
    }

    /// AETHER-FONTDB-LAZY (SR419): the bytes [`FontDb::discover`] read to index the faces.
    pub fn discover_bytes(&self) -> u64 {
        self.bytes_read
    }

    /// The faces of an installed family (exact name, fontconfig comparison).
    pub fn family_faces(&self, family: &str) -> Vec<usize> {
        (0..self.faces.len()).filter(|&i| self.faces[i].families.iter().any(|f| family_eq(f, family))).collect()
    }

    fn installed(&self, family: &str) -> Option<String> {
        self.faces.iter().find_map(|f| f.families.iter().find(|n| family_eq(n, family)).cloned())
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
            let (t0, b0) = (std::time::Instant::now(), self.scan_bytes()); // AETHERFONTDBFALLBACK (SR425)
            let mut scanned = 0usize;
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
                    let st =
                        (f.style.weight - 400.0).abs() as usize + if f.style.slant == Slant::Normal { 0 } else { 1000 };
                    (r, if r == usize::MAX { scanned += 1; latin_rank(self, i) } else { 0 }, st, i)
                })
                .collect();
            idx.sort();
            let kb = (self.scan_bytes() - b0) / 1024;
            eprintln!("[fonts] fallback faces={scanned} bytes_read={kb} ms={}", t0.elapsed().as_millis());
            idx.into_iter().map(|t| t.3).collect()
        })
    }
}

/// 0 when the face's cmap covers a–z (fontconfig's `en` orthography), else 1; 2 when its file is unreadable.
/// AETHERFONTDBFALLBACK (SR425): read through [`FontDb::covers`] (the cmap table alone), never a whole load.
fn latin_rank(db: &FontDb, i: usize) -> usize {
    match ('a'..='z').map(|c| db.covers(i, c)).try_fold(true, |all, v| v.map(|v| all && v)) {
        None => 2,
        Some(true) => 0,
        Some(false) => 1,
    }
}

/// The process-wide database, discovered on first use.
pub fn db() -> &'static FontDb {
    static DB: OnceLock<FontDb> = OnceLock::new();
    DB.get_or_init(|| FontDb::discover(Config::system()))
}

// AETHER-FONTDB-LAZY (SR419): discovery from the headers. A face file is indexed from its table directory and the
// five tables a record needs (`head`, `maxp`, `hhea`, `name`, `OS/2`), each read by offset; font_core parses them
// from a small sfnt assembled of those tables, so the name / OS/2 / head parsers stay font_core's own. The full
// bytes load on first use (`crate::load_face` -> `file_bytes`).

/// A table larger than this is treated as absent (a `name` or `OS/2` table is a few KB).
const HEADER_TABLE_CAP: u32 = 4 << 20;

/// Bounded reads of one font file, counting the bytes read.
struct HeaderReader {
    file: std::fs::File,
    len: u64,
    read: u64,
}

impl HeaderReader {
    fn open(path: &Path) -> Option<HeaderReader> {
        let file = std::fs::File::open(path).ok()?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        Some(HeaderReader { file, len, read: 0 })
    }

    fn at(&mut self, off: u64, len: usize) -> Option<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        if off.checked_add(len as u64)? > self.len {
            return None;
        }
        self.file.seek(SeekFrom::Start(off)).ok()?;
        let mut v = vec![0u8; len];
        self.file.read_exact(&mut v).ok()?;
        self.read += len as u64;
        Some(v)
    }

    /// The table-directory offset of every face of the file (a TTC's offset table, at most 64, else the one sfnt
    /// at 0). AETHERFONTDBFALLBACK (SR425): this, [`Self::directory`], [`Self::entry`] and [`Self::table`] are
    /// SR419's reads lifted out of `faces_in_header` / `face_from_header` so the fallback scan's cmap read
    /// ([`face_cmap`]) goes through the same ONE bounded reader.
    fn face_dirs(&mut self) -> Vec<u32> {
        match self.at(0, 12) {
            Some(h) if be32(&h, 0) == Some(0x7474_6366) => {
                let n = be32(&h, 8).unwrap_or(0).min(64) as usize;
                self.at(12, 4 * n).map(|o| (0..n).filter_map(|i| be32(&o, 4 * i)).collect()).unwrap_or_default()
            }
            Some(_) => vec![0],
            None => Vec::new(),
        }
    }

    /// The face directory at `dir`: its sfnt version and its 16-byte table records.
    fn directory(&mut self, dir: u64) -> Option<(u32, Vec<u8>)> {
        let hdr = self.at(dir, 12)?;
        let version = be32(&hdr, 0)?;
        let num_tables = be16(&hdr, 4)? as usize;
        Some((version, self.at(dir + 12, 16 * num_tables)?))
    }

    /// (offset, length) of the first record of `tag` whose range lies in the file, as `Font::table` finds it.
    fn entry(&self, recs: &[u8], tag: &[u8; 4]) -> Option<(u32, u32)> {
        let e = recs.chunks_exact(16).find(|e| &e[..4] == tag)?;
        let (off, len) = (be32(e, 8)?, be32(e, 12)?);
        (off as u64 + len as u64 <= self.len).then_some((off, len))
    }

    /// A table's bytes by `tag`, read by offset (absent, past the file or over [`HEADER_TABLE_CAP`]: None).
    fn table(&mut self, recs: &[u8], tag: &[u8; 4]) -> Option<Vec<u8>> {
        let (off, len) = self.entry(recs, tag)?;
        if len > HEADER_TABLE_CAP {
            return None;
        }
        self.at(off as u64, len as usize)
    }
}

fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Every face of one font file, indexed from its headers, and the bytes read to do it. A face is skipped exactly
/// where `Font::parse_face` and the outline check of [`faces_in_bytes`] skip it, except that the outline tables'
/// CONTENT is not read: a face whose `CFF ` or glyph data is malformed is indexed and fails at `load_face`.
pub fn faces_in_header(path: &Path) -> (Vec<FaceInfo>, u64) {
    let Some(mut r) = HeaderReader::open(path) else { return (Vec::new(), 0) };
    let dirs = r.face_dirs();
    let mut out = Vec::new();
    for (i, &dir) in dirs.iter().enumerate() {
        if let Some(face) = face_from_header(&mut r, path, dir as u64, i as u32) {
            out.push(face);
        }
    }
    (out, r.read)
}

/// One face of a file from its table directory at `dir`.
fn face_from_header(r: &mut HeaderReader, path: &Path, dir: u64, index: u32) -> Option<FaceInfo> {
    let (version, recs) = r.directory(dir)?;
    let entry = |r: &HeaderReader, tag: &[u8; 4]| r.entry(&recs, tag);
    let head = r.table(&recs, b"head")?;
    let maxp = r.table(&recs, b"maxp")?;
    let mut hhea = r.table(&recs, b"hhea")?;
    let name = r.table(&recs, b"name");
    let os2 = r.table(&recs, b"OS/2");
    // The skips `parse_face` makes beyond what it checks itself below: `hmtx` must hold numberOfHMetrics entries;
    // outlines are `glyf` + a long-enough `loca`, else `CFF ` (font.rs's outline rule).
    let num_glyphs = be16(&maxp, 4)? as usize;
    let num_hmetrics = be16(&hhea, 34)? as usize;
    if num_hmetrics == 0 || (entry(r, b"hmtx").map_or(0, |e| e.1) as usize) < 4 * num_hmetrics {
        return None;
    }
    let long = be16(&head, 50)? as i16 == 1;
    let outlines = match (entry(r, b"glyf"), entry(r, b"loca")) {
        (Some(_), Some(loca)) => loca.1 as usize >= (num_glyphs + 1) * if long { 4 } else { 2 },
        _ => entry(r, b"CFF ").is_some(),
    };
    if !outlines {
        return None;
    }
    // The header-only sfnt: the real tables, `hhea` claiming one metric and a 4-byte `hmtx` to hold it.
    hhea[34..36].copy_from_slice(&1u16.to_be_bytes());
    let hmtx = [0u8; 4];
    let mut tables: Vec<(&[u8; 4], &[u8])> =
        vec![(b"head", head.as_slice()), (b"maxp", maxp.as_slice()), (b"hhea", hhea.as_slice()), (b"hmtx", &hmtx[..])];
    if let Some(n) = &name {
        tables.push((b"name", n.as_slice()));
    }
    if let Some(o) = &os2 {
        tables.push((b"OS/2", o.as_slice()));
    }
    let sfnt = header_sfnt(version, &tables);
    let f = Font::parse_face(&sfnt, 0).ok()?;
    face_record(&f, path, index)
}

/// An sfnt of `tables` (offset table, directory, 4-aligned table data; checksums and the binary-search fields are
/// zero: `Font::parse_face` reads neither).
fn header_sfnt(version: u32, tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let n = tables.len();
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&[0u8; 6]);
    let mut off = 12 + 16 * n;
    for (tag, data) in tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        off += data.len().next_multiple_of(4);
    }
    for (_, data) in tables {
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    out
}

// AETHERFONTDBFALLBACK (SR425): the per-character fallback's coverage from each face's cmap table alone, read
// through SR419's bounded reader; the whole file loads (`crate::load_face` -> `file_bytes`) only for the face picked.

/// Face `index`'s `cmap` table alone (its directory, then the table by offset; empty when absent, past the file or
/// over [`HEADER_TABLE_CAP`]), and the bytes read; None when the file or the face's directory is unreadable.
pub fn face_cmap(path: &Path, index: u32) -> Option<(Vec<u8>, u64)> {
    let mut r = HeaderReader::open(path)?;
    let dir = *r.face_dirs().get(index as usize)?;
    let (_, recs) = r.directory(dir as u64)?;
    let cmap = r.table(&recs, b"cmap").unwrap_or_default();
    Some((cmap, r.read))
}

impl FontDb {
    /// The face's cmap table bytes (empty when it has none), read once with [`face_cmap`].
    fn cmap_of(&self, i: usize) -> Option<std::borrow::Cow<'_, [u8]>> {
        let read = || -> Option<Box<[u8]>> {
            let f = self.faces.get(i)?;
            let (t, n) = face_cmap(&f.path, f.index)?;
            self.scan_bytes.fetch_add(n, Ordering::Relaxed);
            Some(t.into_boxed_slice())
        };
        match self.cmaps.get(i) {
            Some(cell) => cell.get_or_init(read).as_deref().map(std::borrow::Cow::Borrowed),
            None => read().map(|b| std::borrow::Cow::Owned(b.into_vec())),
        }
    }

    /// Whether face `i` maps `c` (as `Font::glyph_index(c) != 0` would on the whole face); None when its file
    /// is unreadable.
    pub fn covers(&self, i: usize, c: char) -> Option<bool> {
        let cmap = self.cmap_of(i)?;
        let st = font_core::cmap::Cmap::parse(&cmap).and_then(|t| t.best());
        Some(st.is_some_and(|st| st.unicode_glyph(c) != 0))
    }

    /// The bytes the bounded cmap reads have taken so far.
    pub fn scan_bytes(&self) -> u64 {
        self.scan_bytes.load(Ordering::Relaxed)
    }
}

// AETHER-MACGENERICS (SR418): the CSS generic families, ONE table per OS, keyed by the generic's CSS keyword.
// Linux: Chromium Linux's WebFontFamilySettings names, one candidate each (byte-identical to the names Aether
// handed the resolver before this table). macOS: Chromium-mac's (locale_settings_mac.grd: Times, Helvetica,
// Courier, Apple Chancery, Papyrus; system-ui the CoreText UI face, SFNS.ttf; ui-monospace SF Mono, then Menlo),
// each followed by the faces a stock Mac also carries. A stock Mac has no fonts.conf, so no alias rescues a name.
const LINUX_GENERICS: &[(&str, &[&str])] = &[
    ("serif", &["Times New Roman"]),
    ("sans-serif", &["Arial"]),
    ("monospace", &["Monospace"]),
    ("cursive", &["Comic Sans MS"]),
    ("fantasy", &["Impact"]),
    ("system-ui", &["sans"]),
];

const MACOS_GENERICS: &[(&str, &[&str])] = &[
    ("serif", &["Times", "Times New Roman"]),
    ("sans-serif", &["Helvetica", "Arial"]),
    ("monospace", &["Courier", "Menlo", "Courier New"]),
    ("cursive", &["Apple Chancery", "Comic Sans MS"]),
    ("fantasy", &["Papyrus", "Impact"]),
    ("system-ui", &["System Font", ".SF NS", "SF Pro", "Helvetica Neue", "Helvetica"]),
    // reached once Aether parses the ui-* generics (AETHER-UIGENERICS, owed)
    ("ui-monospace", &["SF Mono", ".SF NS Mono", "Menlo"]),
];

/// The generic-family table of an OS: (CSS keyword, candidate family names in preference order).
pub fn generics_for(macos: bool) -> &'static [(&'static str, &'static [&'static str])] {
    if macos { MACOS_GENERICS } else { LINUX_GENERICS }
}

impl FontDb {
    /// The family name a generic keyword hands the resolver on that OS: the first candidate
    /// [`FontDb::resolve_family`] answers, else the first candidate (a miss stays a miss downstream); a keyword
    /// the table has no row for is returned as itself (resolved as an ordinary family name).
    pub fn generic_in(&self, macos: bool, keyword: &'static str) -> &'static str {
        let Some((_, cands)) = generics_for(macos).iter().find(|(k, _)| k.eq_ignore_ascii_case(keyword)) else {
            return keyword;
        };
        cands.iter().copied().find(|c| self.resolve_family(c).is_some()).unwrap_or(cands[0])
    }
}

/// The family name a generic keyword resolves through on this host ([`FontDb::generic_in`] over the process
/// database). The first call logs the picks once:
/// `[fonts] generics os=<linux|macos> sans=<face|none> serif=<face|none> mono=<face|none> ui=<face|none>`.
pub fn generic_family(keyword: &'static str) -> &'static str {
    let d = db();
    let macos = cfg!(target_os = "macos");
    static LOG: std::sync::Once = std::sync::Once::new();
    LOG.call_once(|| {
        let pick = |k: &'static str| d.resolve_family(d.generic_in(macos, k)).unwrap_or_else(|| "none".to_string());
        eprintln!(
            "[fonts] generics os={} sans={} serif={} mono={} ui={}",
            if macos { "macos" } else { "linux" },
            pick("sans-serif"),
            pick("serif"),
            pick("monospace"),
            pick("system-ui")
        );
    });
    d.generic_in(macos, keyword)
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
        let fam =
            [st(300.0, n, 100.0), st(400.0, n, 100.0), st(500.0, n, 100.0), st(700.0, n, 100.0), st(900.0, n, 100.0)];
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
        let Ok(out) = std::process::Command::new("fc-match")
            .args(["-s", "--format", "%{file}#%{index}\\n", "sans-serif"])
            .output()
        else {
            return;
        };
        let want: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| db.faces.iter().any(|f| format!("{}#{}", f.path.display(), f.index) == *l))
            .map(str::to_string)
            .collect();
        let ours: Vec<String> = db
            .fallback_order()
            .iter()
            .map(|&i| format!("{}#{}", db.faces[i].path.display(), db.faces[i].index))
            .collect();
        let n = 6.min(want.len());
        let fam = |s: &str| {
            db.faces
                .iter()
                .find(|f| format!("{}#{}", f.path.display(), f.index) == s)
                .map(|f| f.families[0].clone())
                .unwrap_or_default()
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

    // AETHER-FONTDB-LAZY (SR419): header-only discovery.
    fn same_records(a: &[FaceInfo], b: &[FaceInfo]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| {
                x.path == y.path
                    && x.index == y.index
                    && x.families == y.families
                    && x.full_names == y.full_names
                    && x.style == y.style
            })
    }

    /// One small real face: indexed from far fewer bytes than the file, the same record as the whole-file read,
    /// and it still shapes a glyph once `load_face` reads it.
    #[test]
    fn fontdb_lazy_fixture_kat() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fonts/jetbrains-mono");
        let file = dir.join("JetBrainsMono-Variable.ttf");
        let size = std::fs::metadata(&file).expect("fixture font").len();
        let db = FontDb::discover(Config { dirs: vec![dir], ..Default::default() });
        assert_eq!(db.faces.len(), 1, "{:?}", db.faces);
        assert!(db.faces[0].families.iter().any(|f| f == "JetBrains Mono"), "{:?}", db.faces[0].families);
        let n = db.discover_bytes();
        assert!(n > 0 && n < size, "bytes_read {n} vs file {size}");
        let whole = faces_in_bytes(&std::fs::read(&file).unwrap(), &file);
        assert!(same_records(&db.faces, &whole), "{:?} vs {:?}", db.faces, whole);
        let face = crate::load_face(&db.faces[0]).expect("the lazy load");
        let g = face.glyph_for_char('A').expect("a glyph for A");
        assert!(face.font.glyph_path(g).is_some_and(|p| !p.is_empty()), "A has an outline");
    }

    /// Every face file of the host (skipped without /usr/share/fonts): the header index equals the whole-file one.
    #[test]
    fn fontdb_lazy_matches_whole_read() {
        let mut files = Vec::new();
        walk_dir(Path::new("/usr/share/fonts"), &mut files, 0);
        let (mut header_bytes, mut whole_bytes) = (0u64, 0u64);
        for p in &files {
            let Ok(data) = std::fs::read(p) else { continue };
            let (head, n) = faces_in_header(p);
            let whole = faces_in_bytes(&data, p);
            assert!(same_records(&head, &whole), "{}: {:?} vs {:?}", p.display(), head, whole);
            header_bytes += n;
            whole_bytes += data.len() as u64;
        }
        assert!(header_bytes * 10 <= whole_bytes, "header reads {header_bytes} of {whole_bytes}");
    }

    // AETHER-MACGENERICS (SR418): fake databases (no font files are read: resolution looks at family names only).
    fn fake_db(families: &[&str], config: Config) -> FontDb {
        let faces = families
            .iter()
            .map(|f| FaceInfo {
                path: PathBuf::from(format!("/fake/{f}.ttf")),
                index: 0,
                families: vec![f.to_string()],
                full_names: Vec::new(),
                style: Style::default(),
            })
            .collect();
        FontDb::from_faces(faces, config)
    }

    /// The macOS table on a stock Mac (no fonts.conf, so no aliases): Chromium-mac's picks, the next candidate
    /// when one is absent, and a miss stays a miss.
    #[test]
    fn macgenerics_macos_table_kat() {
        let d = fake_db(&["Helvetica", "Times", "Menlo"], Config::default());
        assert_eq!(d.generic_in(true, "sans-serif"), "Helvetica");
        assert_eq!(d.generic_in(true, "serif"), "Times");
        assert_eq!(d.generic_in(true, "monospace"), "Menlo", "no Courier: Menlo");
        assert_eq!(d.generic_in(true, "system-ui"), "Helvetica", "no SF face, no Helvetica Neue");
        assert_eq!(d.generic_in(true, "ui-monospace"), "Menlo");
        assert_eq!(d.generic_in(true, "cursive"), "Apple Chancery", "none installed: the first candidate");
        assert_eq!(d.resolve_family(d.generic_in(true, "cursive")), None, "a miss stays a miss");
        assert_eq!(d.generic_in(true, "math"), "math", "no row: the keyword itself");
        let d = fake_db(&["Helvetica", "Times", "Menlo", "Courier", "System Font"], Config::default());
        assert_eq!(d.generic_in(true, "monospace"), "Courier");
        assert_eq!(d.generic_in(true, "system-ui"), "System Font");
        assert_eq!(d.resolve_family(d.generic_in(true, "SANS-SERIF")).as_deref(), Some("Helvetica"));
    }

    /// The Linux table is today's six names exactly, so every Linux pick resolves as before.
    #[test]
    fn macgenerics_linux_unchanged_kat() {
        let today: &[(&'static str, &'static str)] = &[
            ("serif", "Times New Roman"),
            ("sans-serif", "Arial"),
            ("monospace", "Monospace"),
            ("cursive", "Comic Sans MS"),
            ("fantasy", "Impact"),
            ("system-ui", "sans"),
        ];
        let t = generics_for(false);
        assert_eq!(t.len(), today.len());
        for ((k, cands), (tk, tn)) in t.iter().zip(today) {
            assert_eq!((*k, *cands), (*tk, &[*tn][..]));
        }
        let mut c = Config::default();
        c.read_str(
            r#"<fontconfig>
  <alias><family>sans-serif</family><prefer><family>DejaVu Sans</family></prefer></alias>
  <alias><family>sans</family><prefer><family>DejaVu Sans</family></prefer></alias>
  <alias><family>serif</family><prefer><family>DejaVu Serif</family></prefer></alias>
  <alias><family>monospace</family><prefer><family>DejaVu Sans Mono</family></prefer></alias>
</fontconfig>"#,
            Path::new("/etc/fonts"),
        );
        let d = fake_db(&["DejaVu Sans", "DejaVu Serif", "DejaVu Sans Mono"], c);
        for (k, n) in today {
            assert_eq!(d.generic_in(false, *k), *n, "{k}: today's name");
        }
        assert_eq!(d.resolve_family(d.generic_in(false, "monospace")).as_deref(), Some("DejaVu Sans Mono"));
        assert_eq!(d.resolve_family(d.generic_in(false, "system-ui")).as_deref(), Some("DejaVu Sans"));
        assert_eq!(d.resolve_family(d.generic_in(false, "sans-serif")), None, "Arial: no metric-compatible face");
        assert_eq!(d.generic_in(false, "ui-monospace"), "ui-monospace", "no Linux row");
    }

    /// AETHERFONTDBFALLBACK (SR425) oracle: the fallback order with the Latin rank read from each WHOLE file
    /// (the pre-SR425 shape), parsed with font_core and dropped (no leak).
    fn whole_order(db: &FontDb) -> Vec<usize> {
        let expanded = db.config.expand(&["sans-serif"]);
        let mut last: Option<(PathBuf, Option<Vec<u8>>)> = None;
        let mut idx: Vec<(usize, usize, usize, usize)> = Vec::new();
        for (i, f) in db.faces.iter().enumerate() {
            let r = f
                .families
                .iter()
                .filter_map(|n| expanded.iter().position(|e| family_eq(e, n)))
                .min()
                .unwrap_or(usize::MAX);
            let st = (f.style.weight - 400.0).abs() as usize + if f.style.slant == Slant::Normal { 0 } else { 1000 };
            let lr = if r == usize::MAX {
                if last.as_ref().is_none_or(|(p, _)| *p != f.path) {
                    last = Some((f.path.clone(), std::fs::read(&f.path).ok()));
                }
                let data = last.as_ref().and_then(|(_, d)| d.as_deref());
                match data.and_then(|d| Font::parse_face(d, f.index).ok()) {
                    None => 2,
                    Some(face) if ('a'..='z').all(|c| face.glyph_index(c) != 0) => 0,
                    Some(_) => 1,
                }
            } else {
                0
            };
            idx.push((r, lr, st, i));
        }
        idx.sort();
        idx.into_iter().map(|t| t.3).collect()
    }

    /// AETHERFONTDBFALLBACK (SR425): over the repo's fixture fonts with no fonts.conf aliases (the stock-Mac
    /// case: every face unranked, so every face takes the Latin rank), the bounded cmap read gives the same
    /// fallback order as the whole-file read, reads fewer bytes than the files hold, and answers coverage as
    /// the whole parsed face does (the Symbol U+F000 rule included).
    #[test]
    fn fallback_bounded_rank_kat() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dirs = vec![root.join("fonts"), root.join("unaos/libs/text/font_core/tests/data")];
        let db = FontDb::discover(Config { dirs, ..Config::default() });
        assert!(db.faces.len() >= 3, "the fixture faces (Inter, JetBrains Mono, Noto Sans) are discovered");
        let order = db.fallback_order().to_vec();
        assert_eq!(order, whole_order(&db), "the bounded rank keeps the whole-read fallback order");
        let mut paths: Vec<&PathBuf> = db.faces.iter().map(|f| &f.path).collect();
        paths.sort();
        paths.dedup();
        let total: u64 = paths.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        let read = db.scan_bytes();
        assert!(read > 0 && read < total, "bytes_read {read} must be > 0 and < the files' {total}");
        let chars = ['a', 'Z', '\u{e9}', '\u{416}', '\u{4e2d}', '\u{2603}', '\u{1f600}', '\u{f041}'];
        for (i, f) in db.faces.iter().enumerate() {
            let data = std::fs::read(&f.path).unwrap();
            let face = Font::parse_face(&data, f.index).unwrap();
            for c in chars {
                let want = face.glyph_index(c) != 0;
                assert_eq!(db.covers(i, c), Some(want), "{}#{} U+{:04X}", f.path.display(), f.index, c as u32);
            }
        }
    }

    /// AETHERFONTDBFALLBACK (SR425): on this host's installed set the bounded rank keeps the whole-read order.
    #[test]
    fn fallback_order_unchanged_on_host() {
        let db = db();
        if db.faces.is_empty() {
            return;
        }
        assert_eq!(db.fallback_order(), &whole_order(db)[..], "the host's fallback order");
    }
}
