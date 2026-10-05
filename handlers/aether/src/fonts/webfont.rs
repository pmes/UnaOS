//! `@font-face` (css-fonts-4 §4, AETHERFONT M1): author faces, matched before installed families.
//!
//! - The rules come from css_core's parse (`CssRule::FontFace`: `font-family`, `src`, `font-weight`,
//!   `font-style`, `font-stretch`, `unicode-range` descriptors).
//! - `src` is tried in order (§4.3): `local(name)` against installed faces' full and PostScript names;
//!   `url(…)` from `data:` (decoded here), `file:` (read), or `http(s):` bytes the page loader fetched through
//!   http_core ([`store_bytes`]; `net::fetch_page` collects and fetches them beside the images). A
//!   `format()` hint naming a format font_core cannot read is skipped without fetching.
//! - Font data: sfnt (TrueType/OpenType/collections) as is; WOFF 1.0 (W3C WOFF File Format 1.0) unpacked
//!   here, zlib tables inflated by UnaOS's own `pixel_core::inflate`. WOFF 2.0 needs Brotli and the glyf
//!   transform — not implemented, so such a face is ledgered and the next source is tried.
//! - Selection inside an author family is css-fonts-4 §5.2 over the descriptor ranges, with synthesis as
//!   for installed faces; a face whose `unicode-range` excludes every character of the text is still the
//!   family's face for metrics (Blink keeps the first face of the segmented family as primary).

use super::db::{match_style, synthesis, Slant, Style};
use super::Face;
use font_core::Font;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// One registered author face.
#[derive(Clone)]
pub struct WebFace {
    pub family: String,
    pub face: &'static Face,
    /// `font-weight` range (a single value is min = max).
    pub weight: (f32, f32),
    pub slant: Slant,
    pub stretch: (f32, f32),
    pub unicode_range: Vec<(u32, u32)>,
    /// The rule's identity (family + descriptors + chosen source), so re-applying a sheet is a no-op.
    key: String,
}

struct Registry {
    faces: Vec<WebFace>,
    /// Font bytes fetched by the page loader, by absolute URL.
    bytes: HashMap<String, std::sync::Arc<Vec<u8>>>,
    /// Decoded data: URLs and files, by URL (sfnt bytes leaked once).
    loaded: HashMap<String, Option<&'static [u8]>>,
}

fn reg() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry { faces: Vec::new(), bytes: HashMap::new(), loaded: HashMap::new() }))
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

/// Bumped whenever the author face set changes: font stacks and shaping caches key on it.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Hands the bytes of a fetched font URL to the registry (the page loader calls this).
pub fn store_bytes(url: &str, bytes: Vec<u8>) {
    if let Ok(mut r) = reg().lock() {
        r.bytes.insert(url.to_string(), std::sync::Arc::new(bytes));
    }
}

/// Drops every author face (a new document).
pub fn clear() {
    if let Ok(mut r) = reg().lock() {
        if !r.faces.is_empty() {
            r.faces.clear();
            GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The best author face of `family` for `want` (None when no `@font-face` declares that family).
pub fn select(family: &str, want: Style) -> Option<&'static Face> {
    let r = reg().lock().ok()?;
    let cands: Vec<&WebFace> = r.faces.iter().filter(|f| f.family.eq_ignore_ascii_case(family)).collect();
    if cands.is_empty() {
        return None;
    }
    // §5.2 over ranges: a face whose range contains the wanted value matches it exactly; otherwise the
    // range's nearest end stands for it.
    let clamp = |v: f32, (lo, hi): (f32, f32)| v.clamp(lo, hi);
    let styles: Vec<Style> = cands
        .iter()
        .map(|f| Style { weight: clamp(want.weight, f.weight), slant: f.slant, stretch: clamp(want.stretch, f.stretch) })
        .collect();
    let k = match_style(&styles, want)?;
    let face = cands[k].face;
    let (b, o) = synthesis(Style { weight: styles[k].weight, ..face.style }, want);
    if b == face.synth_bold && o == face.synth_oblique {
        return Some(face);
    }
    Some(synth_variant(face, b, o))
}

fn synth_variant(face: &'static Face, bold: bool, oblique: bool) -> &'static Face {
    static V: OnceLock<Mutex<HashMap<(u32, bool, bool), &'static Face>>> = OnceLock::new();
    let m = V.get_or_init(|| Mutex::new(HashMap::new()));
    let mut m = m.lock().unwrap();
    *m.entry((face.id, bold, oblique)).or_insert_with(|| {
        Box::leak(Box::new(Face {
            id: super::next_face_id(),
            font: face.font,
            synth_bold: bold,
            synth_oblique: oblique,
            family: face.family.clone(),
            style: face.style,
        }))
    })
}

/// Formats font_core reads (css-fonts-4 §4.3 `<font-format>`); anything else is skipped unfetched.
fn format_supported(fmt: Option<&str>) -> bool {
    match fmt.map(|f| f.to_ascii_lowercase()) {
        None => true,
        Some(f) => matches!(f.as_str(), "truetype" | "opentype" | "woff" | "collection" | "truetype-variations" | "opentype-variations"),
    }
}

struct SliceSrc<'a>(&'a [u8], usize);
impl pixel_core::inflate::ByteSource for SliceSrc<'_> {
    fn next(&mut self) -> Option<u8> {
        let b = self.0.get(self.1).copied();
        self.1 += 1;
        b
    }
}
struct VecSink(Vec<u8>, usize);
impl pixel_core::inflate::Sink for VecSink {
    fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.0.len() >= self.1 {
            return Err(());
        }
        self.0.push(byte);
        Ok(())
    }
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// WOFF 1.0 → sfnt (W3C "WOFF File Format 1.0" §3–§5): the table directory is rebuilt with 4-byte aligned
/// tables; a table whose compLength < origLength is a zlib stream of origLength bytes.
pub fn woff1_to_sfnt(d: &[u8]) -> Option<Vec<u8>> {
    if be32(d, 0)? != 0x774F_4646 {
        return None;
    }
    let flavor = be32(d, 4)?;
    let num = be16(d, 12)? as usize;
    let total = be32(d, 16)? as usize;
    if num == 0 || num > 4096 || total > 64 << 20 {
        return None;
    }
    let mut tables: Vec<([u8; 4], Vec<u8>, u32)> = Vec::with_capacity(num);
    for i in 0..num {
        let e = 44 + 20 * i;
        let tag: [u8; 4] = d.get(e..e + 4)?.try_into().ok()?;
        let (off, comp, orig, cksum) = (be32(d, e + 4)? as usize, be32(d, e + 8)? as usize, be32(d, e + 12)? as usize, be32(d, e + 16)?);
        let raw = d.get(off..off.checked_add(comp)?)?;
        let data = if comp < orig {
            let mut sink = VecSink(Vec::with_capacity(orig), orig);
            pixel_core::inflate::zlib_inflate(&mut SliceSrc(raw, 0), &mut sink).ok()?;
            if sink.0.len() != orig {
                return None;
            }
            sink.0
        } else if comp == orig {
            raw.to_vec()
        } else {
            return None;
        };
        tables.push((tag, data, cksum));
    }
    tables.sort_by_key(|t| t.0);
    let mut sel = 0u16;
    while (2u32 << sel) <= num as u32 {
        sel += 1;
    }
    let search = (1u16 << sel) * 16;
    let mut out = Vec::with_capacity(total.max(12 + 16 * num));
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num as u16).to_be_bytes());
    out.extend_from_slice(&search.to_be_bytes());
    out.extend_from_slice(&sel.to_be_bytes());
    out.extend_from_slice(&((num as u16) * 16 - search).to_be_bytes());
    let mut off = 12 + 16 * num;
    let mut body = Vec::new();
    for (tag, data, ck) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&ck.to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        off = 12 + 16 * num + body.len();
    }
    out.extend_from_slice(&body);
    Some(out)
}

/// Decodes a font resource to sfnt bytes the process keeps (None when unreadable).
fn sfnt_of(bytes: &[u8]) -> Option<&'static [u8]> {
    let data = if bytes.starts_with(b"wOFF") {
        woff1_to_sfnt(bytes)?
    } else if bytes.starts_with(b"wOF2") {
        crate::ledger::record_css("font-face-woff2-unsupported");
        return None;
    } else {
        bytes.to_vec()
    };
    Font::parse(&data).ok()?;
    Some(Box::leak(data.into_boxed_slice()))
}

fn data_url_bytes(url: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let (meta, payload) = url.strip_prefix("data:")?.split_once(',')?;
    if meta.contains(";base64") {
        let clean: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD.decode(clean).ok()
    } else {
        Some(percent_decode_bytes(payload))
    }
}

/// The sfnt bytes of a `url()` source, if available now.
fn url_sfnt(url: &str) -> Option<&'static [u8]> {
    {
        let r = reg().lock().ok()?;
        if let Some(x) = r.loaded.get(url) {
            return *x;
        }
    }
    let raw: Option<Vec<u8>> = if url.starts_with("data:") {
        data_url_bytes(url)
    } else if let Some(p) = url.strip_prefix("file://") {
        std::fs::read(percent_decode(p)).ok()
    } else {
        reg().lock().ok()?.bytes.get(url).map(|b| b.as_ref().clone())
    };
    let s = raw.as_deref().and_then(sfnt_of);
    if raw.is_some() {
        reg().lock().ok()?.loaded.insert(url.to_string(), s);
    }
    s
}

fn percent_decode_bytes(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn percent_decode(s: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(s)).into_owned()
}

/// `local(name)`: an installed face whose full or PostScript name is `name` (css-fonts-4 §4.3).
fn local_face(name: &str) -> Option<&'static Face> {
    let d = super::db::db();
    let info = d.faces.iter().find(|f| f.full_names.iter().any(|n| n.eq_ignore_ascii_case(name)))?;
    super::load_face(info)
}

/// Parses a `font-weight` descriptor: `normal`/`bold`/number, or a range of two.
fn weight_desc(v: &str) -> (f32, f32) {
    let one = |s: &str| match s {
        "normal" => Some(400.0),
        "bold" => Some(700.0),
        n => n.parse::<f32>().ok().filter(|w| (1.0..=1000.0).contains(w)),
    };
    let parts: Vec<&str> = v.split_whitespace().collect();
    match parts.as_slice() {
        [a] => one(a).map(|w| (w, w)),
        [a, b] => one(a).zip(one(b)).map(|(a, b)| (a.min(b), a.max(b))),
        _ => None,
    }
    .unwrap_or((400.0, 400.0))
}

fn stretch_desc(v: &str) -> (f32, f32) {
    let one = |s: &str| -> Option<f32> {
        Some(match s {
            "normal" => 100.0,
            "ultra-condensed" => 50.0,
            "extra-condensed" => 62.5,
            "condensed" => 75.0,
            "semi-condensed" => 87.5,
            "semi-expanded" => 112.5,
            "expanded" => 125.0,
            "extra-expanded" => 150.0,
            "ultra-expanded" => 200.0,
            p => p.strip_suffix('%')?.parse().ok()?,
        })
    };
    let parts: Vec<&str> = v.split_whitespace().collect();
    match parts.as_slice() {
        [a] => one(a).map(|w| (w, w)),
        [a, b] => one(a).zip(one(b)).map(|(a, b)| (a.min(b), a.max(b))),
        _ => None,
    }
    .unwrap_or((100.0, 100.0))
}

/// Registers the `@font-face` rules of parsed sheets (document order). URLs resolve against `base`.
/// Returns how many faces are registered after the call.
pub fn apply_rules(sheets: &[css_core::stylesheet::Stylesheet], base: &str) -> usize {
    use css_core::stylesheet::{CssRule, FontSource};
    let text = |v: Option<&[css_core::CV]>| -> String {
        v.map(|v| css_core::serialize::to_css(v).trim().to_ascii_lowercase()).unwrap_or_default()
    };
    let mut wanted: Vec<WebFace> = Vec::new();
    fn walk<'a>(rules: &'a [CssRule], out: &mut Vec<&'a css_core::stylesheet::FontFace>) {
        for r in rules {
            match r {
                CssRule::FontFace(f) => out.push(f),
                CssRule::Media(_, inner) | CssRule::Supports(_, inner) | CssRule::LayerBlock(_, inner) => walk(inner, out),
                _ => {}
            }
        }
    }
    let mut rules = Vec::new();
    for s in sheets {
        walk(&s.rules, &mut rules);
    }
    for ff in rules {
        let Some(family) = ff.family() else { continue };
        let weight = weight_desc(&text(ff.descriptor("font-weight")));
        let style_s = text(ff.descriptor("font-style"));
        let slant = if style_s.starts_with("italic") {
            Slant::Italic
        } else if style_s.starts_with("oblique") {
            Slant::Oblique
        } else {
            Slant::Normal
        };
        let stretch = stretch_desc(&text(ff.descriptor("font-stretch")));
        let unicode_range: Vec<(u32, u32)> = ff
            .descriptor("unicode-range")
            .map(|v| css_core::urange::parse_urange_list(&css_core::serialize::to_css(v)).into_iter().flatten().collect())
            .unwrap_or_default();
        let mut chosen: Option<(&'static Face, String)> = None;
        for src in ff.sources() {
            match src {
                FontSource::Local(name) => {
                    if let Some(f) = local_face(&name) {
                        chosen = Some((f, format!("local({name})")));
                        break;
                    }
                }
                FontSource::Url { url, format } => {
                    if !format_supported(format.as_deref()) {
                        continue;
                    }
                    let abs = crate::images::resolve(base, &url);
                    if let Some(bytes) = url_sfnt(&abs) {
                        if let Ok(font) = Font::parse(bytes) {
                            let face: &'static Face = Box::leak(Box::new(Face {
                                id: super::next_face_id(),
                                font,
                                synth_bold: false,
                                synth_oblique: false,
                                family: family.clone(),
                                style: super::db::style_of(&font),
                            }));
                            chosen = Some((face, abs));
                            break;
                        }
                    } else {
                        crate::ledger::record_css(&format!("font-face-src-unavailable:{}", &abs[..abs.len().min(48)]));
                    }
                }
            }
        }
        let Some((face, src_key)) = chosen else { continue };
        let key = format!("{family}|{weight:?}|{slant:?}|{stretch:?}|{unicode_range:?}|{src_key}");
        wanted.push(WebFace { family, face, weight, slant, stretch, unicode_range, key });
    }
    let mut r = reg().lock().unwrap();
    let same = r.faces.len() == wanted.len() && r.faces.iter().zip(&wanted).all(|(a, b)| a.key == b.key);
    if !same {
        // keep the already-loaded Face of an unchanged rule (ids stay stable for the caches)
        let old: HashMap<String, &'static Face> = r.faces.iter().map(|f| (f.key.clone(), f.face)).collect();
        for w in wanted.iter_mut() {
            if let Some(f) = old.get(&w.key) {
                w.face = f;
            }
        }
        r.faces = wanted;
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    r.faces.len()
}

/// The `url()` sources of every `@font-face` in a CSS text, resolved against `base` (for the loader).
pub fn font_urls(css: &str, base: &str) -> Vec<String> {
    use css_core::stylesheet::{CssRule, FontSource};
    let sheet = css_core::stylesheet::parse_stylesheet(css);
    let mut out = Vec::new();
    for r in &sheet.rules {
        if let CssRule::FontFace(ff) = r {
            for s in ff.sources() {
                if let FontSource::Url { url, format } = s {
                    if format_supported(format.as_deref()) && !url.starts_with("data:") {
                        let abs = crate::images::resolve(base, &url);
                        if (abs.starts_with("http://") || abs.starts_with("https://")) && !out.contains(&abs) {
                            out.push(abs);
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WOFF 1.0 file built here from a real sfnt (each table zlib-compressed by… nothing: stored raw,
    /// compLength = origLength) round-trips to a parseable font; the decoder's zlib path is covered by
    /// `woff1_zlib_table` with a stored-block deflate stream.
    #[test]
    fn woff1_roundtrip() {
        let Ok(ttf) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf") else { return };
        let f = Font::parse(&ttf).unwrap();
        let tags: Vec<[u8; 4]> = f.table_tags().collect();
        let n = tags.len();
        let mut woff = vec![0u8; 44 + 20 * n];
        woff[0..4].copy_from_slice(b"wOFF");
        woff[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        woff[12..14].copy_from_slice(&(n as u16).to_be_bytes());
        woff[16..20].copy_from_slice(&(ttf.len() as u32 + 4096).to_be_bytes());
        for (i, t) in tags.iter().enumerate() {
            let data = f.table(t).unwrap();
            let off = woff.len();
            let e = 44 + 20 * i;
            woff[e..e + 4].copy_from_slice(t);
            woff[e + 4..e + 8].copy_from_slice(&(off as u32).to_be_bytes());
            woff[e + 8..e + 12].copy_from_slice(&(data.len() as u32).to_be_bytes());
            woff[e + 12..e + 16].copy_from_slice(&(data.len() as u32).to_be_bytes());
            woff.extend_from_slice(data);
            while woff.len() % 4 != 0 {
                woff.push(0);
            }
        }
        let sfnt = woff1_to_sfnt(&woff).expect("unpacks");
        let g = Font::parse(&sfnt).expect("parses");
        assert_eq!(g.num_glyphs, f.num_glyphs);
        assert_eq!(g.glyph_index('A'), f.glyph_index('A'));
        assert_eq!(font_core::name::family(&g).as_deref(), Some("DejaVu Sans"));
    }

    #[test]
    fn woff1_zlib_table() {
        // zlib stream with one stored block holding "abcd": 78 01 | 01 04 00 fb ff 'abcd' | adler32
        let payload = b"abcd";
        let mut z = vec![0x78, 0x01, 0x01, 0x04, 0x00, 0xfb, 0xff];
        z.extend_from_slice(payload);
        let (mut a, mut b) = (1u32, 0u32);
        for &x in payload {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut sink = VecSink(Vec::new(), 4);
        pixel_core::inflate::zlib_inflate(&mut SliceSrc(&z, 0), &mut sink).unwrap();
        assert_eq!(sink.0, payload);
    }

    #[test]
    fn descriptors_kat() {
        assert_eq!(weight_desc("bold"), (700.0, 700.0));
        assert_eq!(weight_desc("300 600"), (300.0, 600.0));
        assert_eq!(stretch_desc("condensed"), (75.0, 75.0));
        assert_eq!(stretch_desc("50% 200%"), (50.0, 200.0));
        assert!(format_supported(Some("woff")) && !format_supported(Some("woff2")) && !format_supported(Some("embedded-opentype")));
    }
}
