// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Glyph drawing's font half (ISO 32000-1 §9.4.4, §9.6.6, §9.7): [`GlyphSink::show`] turns a font resource and the
//! bytes of a shown string into the string's outline in TEXT SPACE and its horizontal displacement `tx`. The
//! renderer (PDF2) applies Tm x CTM, paints per the render mode Tr and advances Tm by `tx`; Type 3 glyphs come back
//! as CharProcs ([`Type3Glyph`]) for its content interpreter. One position loop ([`GlyphSink::layout`]) serves both
//! drawing and text extraction.
//!
//! Codes: one byte for simple fonts, the encoding CMap's codespace for Type 0. Code -> glyph: TrueType by the cmap
//! rules of §9.6.6.4 (non-symbolic: name -> AGL -> (3,1), else (1,0) by the name's MacRoman code, else the `post`
//! name; symbolic: (3,0) at code, 0xF000 / 0xF100 / 0xF200 + code, else (1,0) at code); CFF by glyph name through
//! the charset (CID-keyed: CID -> gid through the charset); Type 1 by name; CIDFontType2 by /CIDToGIDMap; a fallback
//! face by the Unicode the name (or /ToUnicode) gives.

use alloc::string::String;
use alloc::vec::Vec;

use font_core::cff::Cff;
use font_core::type1::Type1Font;
use font_core::{OutlineSink, Path};

use super::dict::{CidToGid, FontKind, PdfFont, Program, FLAG_SYMBOLIC};
use super::encoding::{agl, mac_roman_code};
use super::{Fallback, Matrix, apply, mul};
use crate::object::{Dict, Object};
use crate::Document;

/// The text state a shown string needs (§9.3): Tfs, Tc, Tw, Th (1.0 = 100 %), Trise. Tm, the CTM and Tr are the
/// renderer's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextParams {
    pub size: f64,
    pub char_spacing: f64,
    pub word_spacing: f64,
    pub h_scale: f64,
    pub rise: f64,
}

impl Default for TextParams {
    fn default() -> Self {
        TextParams { size: 0.0, char_spacing: 0.0, word_spacing: 0.0, h_scale: 1.0, rise: 0.0 }
    }
}

/// A Type 3 glyph: its CharProc (decoded content), the font's /Resources (empty: the page's apply), and `m`, glyph
/// space -> text space (FontMatrix x the glyph's placement). The renderer runs `proc_` under `m` x Tm x CTM.
#[derive(Clone, Debug, PartialEq)]
pub struct Type3Glyph {
    pub proc_: Vec<u8>,
    pub resources: Dict,
    pub m: Matrix,
}

/// What a shown string draws: every glyph's outline in text space, the displacement `tx` (text space units, Tc / Tw /
/// Th applied), and the Type 3 glyphs to run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub advance: f64,
    pub outline: Path,
    pub type3: Vec<Type3Glyph>,
}

/// A glyph reference into the font's program.
#[derive(Clone, Debug, PartialEq)]
pub enum Glyph {
    Gid(u16),
    /// A Type 1 glyph name.
    Name(String),
    /// A Type 3 CharProc name.
    Proc(Vec<u8>),
    /// Nothing to draw (still advances).
    None,
}

/// One glyph of a shown string.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub code: u32,
    /// Type 0: the CID; simple fonts: the code.
    pub cid: u32,
    /// The text-space x of the glyph origin (the sum of the previous glyphs' tx).
    pub pen: f64,
    /// w0, text space at size 1.
    pub width: f64,
    /// This glyph's tx.
    pub advance: f64,
    pub glyph: Glyph,
}

/// The program, parsed for one string (font_core borrows the bytes; parsing is the table directory only).
enum Face<'a> {
    Sfnt(font_core::Font<'a>),
    Cff(Cff<'a>),
    Type1(&'a Type1Font),
    None,
}

impl Face<'_> {
    /// Glyph units -> glyph text space (1 = one text unit at size 1).
    fn matrix(&self) -> Matrix {
        match self {
            Face::Sfnt(f) => {
                let s = 1.0 / f.units_per_em.max(1) as f64;
                [s, 0.0, 0.0, s, 0.0, 0.0]
            }
            Face::Cff(c) => {
                let s = c.font_matrix_scale as f64;
                [s, 0.0, 0.0, s, 0.0, 0.0]
            }
            Face::Type1(t) => t.font_matrix.map(|v| v as f64),
            Face::None => [0.001, 0.0, 0.0, 0.001, 0.0, 0.0],
        }
    }
}

fn face<'a>(p: &'a Program<'_>) -> Face<'a> {
    match p {
        Program::Sfnt(b) => font_core::Font::parse(b).map(Face::Sfnt).unwrap_or(Face::None),
        Program::Cff(b) => Cff::parse(b, 0).map(Face::Cff).unwrap_or(Face::None),
        Program::Type1(t) => Face::Type1(t),
        Program::Fallback { data, bold } => match font_core::Font::parse(data) {
            Ok(f) if *bold && f.has_axis(*b"wght") => Face::Sfnt(f.with_variations(&[(*b"wght", 700.0)])),
            Ok(f) => Face::Sfnt(f),
            Err(_) => Face::None,
        },
        Program::None => Face::None,
    }
}

struct NullSink;
impl OutlineSink for NullSink {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

/// An outline sink that maps glyph units through `m` into a text-space [`Path`].
struct Xf<'a> {
    m: Matrix,
    out: &'a mut Path,
}

impl Xf<'_> {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        let (a, b) = apply(&self.m, x as f64, y as f64);
        (a as f32, b as f32)
    }
}

impl OutlineSink for Xf<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (a, b) = self.p(x, y);
        self.out.move_to(a, b);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (a, b) = self.p(x, y);
        self.out.line_to(a, b);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x, y);
        self.out.quad_to(a, b, c, d);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x2, y2);
        let (e, f) = self.p(x, y);
        self.out.curve_to(a, b, c, d, e, f);
    }
    fn close(&mut self) {
        self.out.close();
    }
}

fn subtable<'a>(f: &font_core::Font<'a>, platform: u16, encoding: u16) -> Option<font_core::cmap::CmapSubtable<'a>> {
    let c = f.cmap()?;
    (0..c.len()).filter_map(|i| c.subtable(i)).find(|s| s.platform_id == platform && s.encoding_id == encoding)
}

fn nz(g: Option<u16>) -> Option<u16> {
    g.filter(|&g| g != 0)
}

/// §9.6.6.4: a simple TrueType font's code -> gid.
fn truetype_gid(font: &PdfFont<'_>, f: &font_core::Font<'_>, code: u32, name: Option<&str>) -> Option<u16> {
    let symbolic = font.flags & FLAG_SYMBOLIC != 0 || font.encoding.is_none();
    if let (false, Some(n)) = (symbolic, name) {
        if let Some(s) = subtable(f, 3, 1) {
            if let Some(g) = nz(agl(n).and_then(|t| t.chars().next()).and_then(|u| s.glyph(u as u32))) {
                return Some(g);
            }
        }
        if let Some(s) = subtable(f, 1, 0) {
            if let Some(g) = nz(mac_roman_code(n).and_then(|c| s.glyph(c as u32))) {
                return Some(g);
            }
        }
        if let Some(g) = (0..f.num_glyphs).find(|&g| f.glyph_name(g).as_deref() == Some(n)) {
            return Some(g);
        }
    }
    if let Some(s) = subtable(f, 3, 0) {
        for base in [0u32, 0xF000, 0xF100, 0xF200] {
            if let Some(g) = nz(s.glyph(base + code)) {
                return Some(g);
            }
        }
    }
    for (p, e) in [(1, 0), (3, 1)] {
        if let Some(g) = subtable(f, p, e).and_then(|s| nz(s.glyph(code))) {
            return Some(g);
        }
    }
    None
}

/// The character a fallback face draws for `code`: the encoding's name through the AGL, else /ToUnicode, else the
/// code itself when printable ASCII.
fn fallback_char(font: &PdfFont<'_>, code: u32) -> Option<char> {
    if font.kind != FontKind::Type0 {
        if let Some(c) = font.encoding.as_ref().and_then(|e| e.name(code as u8)).and_then(agl).and_then(|t| t.chars().next()) {
            return Some(c);
        }
    }
    if let Some(c) = font.to_unicode.as_ref().and_then(|m| m.unicode(code)).and_then(|t| t.chars().next()) {
        return Some(c);
    }
    (font.kind != FontKind::Type0 && (32..127).contains(&code)).then(|| code as u8 as char)
}

fn resolve(font: &PdfFont<'_>, face: &Face<'_>, code: u32, cid: u32) -> Glyph {
    let name = || font.encoding.as_ref().and_then(|e| e.name(code as u8));
    if font.kind == FontKind::Type3 {
        return match name() {
            Some(n) if font.type3.as_ref().is_some_and(|t| t.char_procs.contains(n.as_bytes())) => Glyph::Proc(n.as_bytes().to_vec()),
            _ => Glyph::None,
        };
    }
    let gid = |g: Option<u16>| g.map(Glyph::Gid).unwrap_or(Glyph::None);
    if let Program::Fallback { .. } = font.program {
        return match face {
            Face::Sfnt(f) => gid(fallback_char(font, code).map(|c| f.glyph_index(c)).filter(|&g| g != 0)),
            _ => Glyph::None,
        };
    }
    let cff_cid = |cid: u32| -> Option<u16> {
        if font.cff_cids.is_empty() {
            return u16::try_from(cid).ok();
        }
        let c = u16::try_from(cid).ok()?;
        font.cff_cids.binary_search_by_key(&c, |e| e.0).ok().map(|i| font.cff_cids[i].1)
    };
    let cff_name = |n: &str| font.cff_names.binary_search_by(|e| e.0.as_str().cmp(n)).ok().map(|i| font.cff_names[i].1);
    match (font.kind, face) {
        (FontKind::Type0, Face::Sfnt(f)) => {
            if matches!(f.outlines, font_core::Outlines::Cff(_)) {
                return gid(cff_cid(cid));
            }
            gid(match &font.cid_to_gid {
                CidToGid::Identity => u16::try_from(cid).ok(),
                CidToGid::Map(m) => m.get(cid as usize).copied(),
            })
        }
        (FontKind::Type0, Face::Cff(_)) => gid(cff_cid(cid)),
        (_, Face::Type1(t)) => match name() {
            Some(n) if t.has_glyph(n) => Glyph::Name(String::from(n)),
            _ => Glyph::None,
        },
        (_, Face::Cff(_)) => gid(name().and_then(cff_name)),
        (_, Face::Sfnt(f)) => {
            if matches!(f.outlines, font_core::Outlines::Cff(_)) {
                if let Some(g) = name().and_then(cff_name) {
                    return Glyph::Gid(g);
                }
            }
            gid(truetype_gid(font, f, code, name()))
        }
        _ => Glyph::None,
    }
}

/// w0 from the program when the dictionary gives no width (text space at size 1).
fn program_width(face: &Face<'_>, g: &Glyph) -> f64 {
    let m = face.matrix();
    match (face, g) {
        (Face::Sfnt(f), Glyph::Gid(g)) => f.advance(*g) as f64 * m[0],
        (Face::Cff(c), Glyph::Gid(g)) => c.outline(*g, &mut NullSink).unwrap_or(0.0) as f64 * m[0],
        (Face::Type1(t), Glyph::Name(n)) => t.outline(n, &mut NullSink).unwrap_or(0.0) as f64 * m[0],
        _ => 0.0,
    }
}

/// The font-resource cache and the glyph pipeline. One per page render (or extraction); it owns its fonts.
pub struct GlyphSink<'d, 'f> {
    doc: &'d Document,
    fallback: Fallback<'f>,
    /// Keyed by the font dictionary's value (the renderer hands the resolved dictionary).
    fonts: Vec<(Dict, PdfFont<'f>)>,
}

impl<'d, 'f> GlyphSink<'d, 'f> {
    pub fn new(doc: &'d Document, fallback: Fallback<'f>) -> Self {
        GlyphSink { doc, fallback, fonts: Vec::new() }
    }

    fn index(&mut self, d: &Dict) -> usize {
        if let Some(i) = self.fonts.iter().position(|(k, _)| k == d) {
            return i;
        }
        let f = PdfFont::load(self.doc, d, &self.fallback);
        self.fonts.push((d.clone(), f));
        self.fonts.len() - 1
    }

    /// The font a dictionary reads as (loaded once).
    pub fn font(&mut self, d: &Dict) -> &PdfFont<'f> {
        let i = self.index(d);
        &self.fonts[i].1
    }

    /// The glyph positions of a shown string and its total tx.
    pub fn layout(&mut self, d: &Dict, bytes: &[u8], p: &TextParams) -> (Vec<Placed>, f64) {
        let i = self.index(d);
        let font = &self.fonts[i].1;
        let face = face(&font.program);
        layout_in(font, &face, bytes, p)
    }

    /// The displacement alone (for a caller that draws nothing).
    pub fn displacement(&mut self, d: &Dict, bytes: &[u8], p: &TextParams) -> f64 {
        self.layout(d, bytes, p).1
    }

    /// Draw a shown string: its outline in text space, its tx, its Type 3 glyphs.
    pub fn show(&mut self, d: &Dict, bytes: &[u8], p: &TextParams) -> Shown {
        let i = self.index(d);
        let doc = self.doc;
        let font = &self.fonts[i].1;
        let face = face(&font.program);
        let (placed, advance) = layout_in(font, &face, bytes, p);
        let gm = face.matrix();
        let mut out = Shown { advance, ..Shown::default() };
        for g in &placed {
            let place: Matrix = [p.size * p.h_scale, 0.0, 0.0, p.size, g.pen, p.rise];
            match (&g.glyph, &face) {
                (Glyph::Proc(name), _) => {
                    let Some(t3) = font.type3.as_ref() else { continue };
                    let Some(Object::Stream(s)) = t3.char_procs.get(name).map(|o| doc.get(o)) else { continue };
                    let Ok(data) = doc.decode(&s) else { continue };
                    out.type3.push(Type3Glyph { proc_: data.data, resources: t3.resources.clone(), m: mul(&font.font_matrix, &place) });
                }
                (Glyph::Gid(gid), Face::Sfnt(f)) => {
                    f.outline(*gid, &mut Xf { m: mul(&gm, &place), out: &mut out.outline });
                }
                (Glyph::Gid(gid), Face::Cff(c)) => {
                    c.outline(*gid, &mut Xf { m: mul(&gm, &place), out: &mut out.outline });
                }
                (Glyph::Name(n), Face::Type1(t)) => {
                    t.outline(n, &mut Xf { m: mul(&gm, &place), out: &mut out.outline });
                }
                _ => {}
            }
        }
        out
    }
}

fn layout_in(font: &PdfFont<'_>, face: &Face<'_>, bytes: &[u8], p: &TextParams) -> (Vec<Placed>, f64) {
    let codes: Vec<(u32, usize)> = match (&font.kind, &font.cmap) {
        (FontKind::Type0, Some(m)) => m.codes(bytes),
        _ => bytes.iter().map(|&b| (b as u32, 1)).collect(),
    };
    let mut pen = 0.0;
    let mut out = Vec::with_capacity(codes.len());
    for (code, len) in codes {
        let cid = match (&font.kind, &font.cmap) {
            (FontKind::Type0, Some(m)) => m.cid(code),
            _ => code,
        };
        let glyph = resolve(font, face, code, cid);
        let width = match font.dict_width(code, cid) {
            Some(w) if font.kind == FontKind::Type3 => w * font.font_matrix[0],
            Some(w) => w / 1000.0,
            None if font.kind == FontKind::Type3 => 0.0,
            None => program_width(face, &glyph),
        };
        let ws = if code == 32 && len == 1 { p.word_spacing } else { 0.0 };
        let advance = (width * p.size + p.char_spacing + ws) * p.h_scale;
        out.push(Placed { code, cid, pen, width, advance, glyph });
        pen += advance;
    }
    (out, pen)
}
