//! The font collection `<text>` draws from: faces are registered as bytes (a `no_std` caller hands them over;
//! with the `std` feature [`FontSet::system`] reads /usr/share/fonts), identified by the OpenType `name` table
//! (typographic family, ID 16, else family, ID 1), `OS/2` weight class and italic bits; selection follows
//! the CSS Fonts 4 §5.2 font matching algorithm for `font-style` then `font-weight`, with generic families
//! mapped to configurable names.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use font_core::Font;

#[derive(Clone, Debug)]
pub struct FontFace {
    pub data: alloc::sync::Arc<Vec<u8>>,
    pub index: u32,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

impl FontFace {
    pub fn font(&self) -> Option<Font<'_>> {
        Font::parse_face(&self.data, self.index).ok()
    }
}

#[derive(Clone, Debug)]
pub struct FontSet {
    pub faces: Vec<FontFace>,
    pub serif: String,
    pub sans_serif: String,
    pub monospace: String,
    pub cursive: String,
    pub fantasy: String,
    /// Used when no listed family matches (the UA default font).
    pub default_family: String,
}

impl Default for FontSet {
    fn default() -> Self {
        FontSet {
            faces: Vec::new(),
            serif: "Times New Roman".into(),
            sans_serif: "Arial".into(),
            monospace: "Courier New".into(),
            cursive: "Comic Sans MS".into(),
            fantasy: "Impact".into(),
            default_family: "Times New Roman".into(),
        }
    }
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

/// Family name from the `name` table: ID 16 preferred over ID 1; Windows Unicode (English) preferred.
pub fn family_name(font: &Font) -> Option<String> {
    let t = font.table(b"name")?;
    let count = be16(t, 2)? as usize;
    let storage = be16(t, 4)? as usize;
    let mut best: Option<(u32, String)> = None;
    for i in 0..count {
        let r = 6 + i * 12;
        let (pid, eid, lid, nid, len, off) = (be16(t, r)?, be16(t, r + 2)?, be16(t, r + 4)?, be16(t, r + 6)?, be16(t, r + 8)? as usize, be16(t, r + 10)? as usize);
        if nid != 1 && nid != 16 {
            continue;
        }
        let Some(bytes) = t.get(storage + off..storage + off + len) else { continue };
        let s = match pid {
            0 | 3 => {
                if pid == 3 && eid != 1 && eid != 10 && eid != 0 {
                    continue;
                }
                let u: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                char::decode_utf16(u).map(|c| c.unwrap_or('\u{fffd}')).collect::<String>()
            }
            1 => bytes.iter().map(|&b| if b < 128 { b as char } else { '\u{fffd}' }).collect(),
            _ => continue,
        };
        let score = (if nid == 16 { 100 } else { 0 }) + (if pid == 3 { 10 } else { 0 }) + (if lid == 0x409 || pid != 3 { 5 } else { 0 });
        if best.as_ref().map(|b| score > b.0).unwrap_or(true) {
            best = Some((score, s));
        }
    }
    best.map(|b| b.1)
}

impl FontSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register every face in a font file (TrueType, OpenType/CFF, or a collection). Returns the face count.
    pub fn add(&mut self, data: Vec<u8>) -> usize {
        let data = alloc::sync::Arc::new(data);
        let n = Font::face_count(&data).max(1);
        let mut added = 0;
        for i in 0..n {
            let Ok(f) = Font::parse_face(&data, i) else { continue };
            let Some(family) = family_name(&f) else { continue };
            let (weight, italic) = match f.os2 {
                Some(o) => (o.weight_class.clamp(1, 1000), o.fs_selection & 1 != 0 || f.mac_style & 2 != 0),
                None => (if f.mac_style & 1 != 0 { 700 } else { 400 }, f.mac_style & 2 != 0),
            };
            self.faces.push(FontFace { data: data.clone(), index: i, family, weight, italic });
            added += 1;
        }
        added
    }

    /// Load every .ttf/.otf/.ttc under the given directories (host only).
    #[cfg(feature = "std")]
    pub fn add_dirs(&mut self, dirs: &[&str]) {
        extern crate std;
        fn walk(p: &std::path::Path, set: &mut FontSet, depth: usize) {
            if depth > 6 {
                return;
            }
            let Ok(rd) = std::fs::read_dir(p) else { return };
            let mut entries: Vec<std::path::PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            entries.sort();
            for e in entries {
                if e.is_dir() {
                    walk(&e, set, depth + 1);
                } else if let Some(ext) = e.extension().and_then(|x| x.to_str()) {
                    if matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc") {
                        if let Ok(d) = std::fs::read(&e) {
                            set.add(d);
                        }
                    }
                }
            }
        }
        for d in dirs {
            walk(std::path::Path::new(d), self, 0);
        }
    }

    /// The system fonts (host only), with generic families mapped to DejaVu where present.
    #[cfg(feature = "std")]
    pub fn system() -> Self {
        let mut s = FontSet::new();
        s.add_dirs(&["/usr/share/fonts", "/usr/local/share/fonts"]);
        let has = |s: &FontSet, f: &str| s.faces.iter().any(|x| x.family.eq_ignore_ascii_case(f));
        for (slot, cands) in [(0, ["Times New Roman", "Liberation Serif", "DejaVu Serif"]), (1, ["Arial", "Liberation Sans", "DejaVu Sans"]), (2, ["Courier New", "Liberation Mono", "DejaVu Sans Mono"])] {
            if let Some(c) = cands.iter().find(|c| has(&s, c)) {
                match slot {
                    0 => s.serif = c.to_string(),
                    1 => s.sans_serif = c.to_string(),
                    _ => s.monospace = c.to_string(),
                }
            }
        }
        s.default_family = s.serif.clone();
        s
    }

    fn generic(&self, name: &str) -> Option<&str> {
        Some(match name.to_ascii_lowercase().as_str() {
            "serif" => &self.serif,
            "sans-serif" => &self.sans_serif,
            "monospace" => &self.monospace,
            "cursive" => &self.cursive,
            "fantasy" => &self.fantasy,
            _ => return None,
        })
    }

    /// Best face of one family for (weight, italic) per CSS Fonts 4 §5.2.
    fn best_in_family(&self, family: &str, weight: u16, italic: bool) -> Option<usize> {
        let cands: Vec<usize> = (0..self.faces.len()).filter(|&i| self.faces[i].family.eq_ignore_ascii_case(family)).collect();
        if cands.is_empty() {
            return None;
        }
        let style_ok: Vec<usize> = cands.iter().copied().filter(|&i| self.faces[i].italic == italic).collect();
        let pool = if style_ok.is_empty() { cands } else { style_ok };
        let w = weight as i32;
        let key = |i: usize| -> (i32, i32) {
            let fw = self.faces[i].weight as i32;
            // Lower key wins.
            if (400..=500).contains(&w) {
                if fw >= w && fw <= 500 {
                    (0, fw - w)
                } else if fw < w {
                    (1, w - fw)
                } else {
                    (2, fw - w)
                }
            } else if w < 400 {
                if fw <= w { (0, w - fw) } else { (1, fw - w) }
            } else if fw >= w {
                (0, fw - w)
            } else {
                (1, w - fw)
            }
        };
        pool.into_iter().min_by_key(|&i| key(i))
    }

    /// Resolve a `font-family` list.
    pub fn select(&self, families: &str, weight: u16, italic: bool) -> Option<usize> {
        for f in families.split(',') {
            let f = f.trim().trim_matches(|c| c == '"' || c == '\'').trim();
            if f.is_empty() {
                continue;
            }
            let name = self.generic(f).unwrap_or(f);
            if let Some(i) = self.best_in_family(name, weight, italic) {
                return Some(i);
            }
        }
        self.best_in_family(&self.default_family.clone(), weight, italic).or(if self.faces.is_empty() { None } else { Some(0) })
    }

    /// A face (other than `skip`) with a glyph for `c`, preferring the style requested.
    pub fn fallback_for(&self, c: char, weight: u16, italic: bool, skip: usize) -> Option<usize> {
        let mut best: Option<(i32, usize)> = None;
        for (i, f) in self.faces.iter().enumerate() {
            if i == skip {
                continue;
            }
            let Some(font) = f.font() else { continue };
            if font.glyph_index(c) == 0 {
                continue;
            }
            let score = (f.italic != italic) as i32 * 1000 + (f.weight as i32 - weight as i32).abs();
            if best.map(|b| score < b.0).unwrap_or(true) {
                best = Some((score, i));
            }
        }
        best.map(|b| b.1)
    }
}
