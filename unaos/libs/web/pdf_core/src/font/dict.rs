// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The font dictionary (ISO 32000-1 §9.5-§9.10): [`PdfFont::load`] reads a font resource into everything glyph
//! drawing and text extraction need. It never fails: a part that does not read is left at its default (no program ->
//! the [`Fallback`] face; no widths -> the program's advances), because a viewer draws what it can.

use alloc::string::String;
use alloc::vec::Vec;

use super::cmap::CMap;
use super::encoding::{Base, Encoding};
use super::{Fallback, Matrix};
use crate::object::{Dict, Object};
use crate::Document;
use font_core::type1::Type1Font;

/// The font's /Subtype family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontKind {
    /// /Type1 and /MMType1 (whatever program they embed: Type 1, CFF, or none).
    Type1,
    TrueType,
    /// /Type0 with a CIDFontType0 (CFF) or CIDFontType2 (TrueType) descendant.
    Type0,
    Type3,
}

/// Where the glyph outlines come from.
#[derive(Clone, Debug)]
pub enum Program<'f> {
    /// An sfnt (TrueType, or OpenType with a `CFF ` table): `/FontFile2`, `/FontFile3 /OpenType`.
    Sfnt(Vec<u8>),
    /// A bare CFF: `/FontFile3 /Type1C` or `/CIDFontType0C`.
    Cff(Vec<u8>),
    /// A Type 1 program: `/FontFile`.
    Type1(Type1Font),
    /// No embedded program: a caller-supplied face; `bold` instances its wght axis at 700.
    Fallback { data: &'f [u8], bold: bool },
    /// Nothing to draw with (no program, no fallback): glyphs advance but have no outline.
    None,
}

/// Glyph widths (glyph space, thousandths of text space for every kind but Type 3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Widths {
    /// Simple fonts: /FirstChar and /Widths.
    pub first: u32,
    pub widths: Vec<f64>,
    /// /MissingWidth, or a CID font's /DW (default 1000).
    pub missing: f64,
    /// CID fonts: /W as (first CID, last CID, width).
    pub cid: Vec<(u32, u32, f64)>,
    /// The dictionary carried /Widths (or /W): without it the program's own advances are used.
    pub present: bool,
}

/// CIDFontType2's /CIDToGIDMap.
#[derive(Clone, Debug, PartialEq)]
pub enum CidToGid {
    Identity,
    Map(Vec<u16>),
}

/// A Type 3 font's glyph procedures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Type3 {
    pub char_procs: Dict,
    pub resources: Dict,
}

/// A font resource, read.
#[derive(Clone, Debug)]
pub struct PdfFont<'f> {
    pub kind: FontKind,
    /// /BaseFont without a subset tag (`ABCDEF+`).
    pub base_font: String,
    pub program: Program<'f>,
    /// /FontDescriptor /Flags (§9.8.2).
    pub flags: u32,
    /// Simple fonts: the resolved code -> glyph name table (None: a symbolic font with no encoding).
    pub encoding: Option<Encoding>,
    pub widths: Widths,
    pub to_unicode: Option<CMap>,
    /// Type 0: the encoding CMap.
    pub cmap: Option<CMap>,
    pub cid_to_gid: CidToGid,
    /// Glyph space -> text space at size 1 (Type 3's /FontMatrix; 0.001 for the others' widths).
    pub font_matrix: Matrix,
    pub type3: Option<Type3>,
    /// CFF programs: (glyph name, gid) sorted by name, and (CID, gid) sorted by CID for CID-keyed CFF.
    pub cff_names: Vec<(String, u16)>,
    pub cff_cids: Vec<(u16, u16)>,
}

/// Flags (§9.8.2 Table 123).
pub const FLAG_FIXED_PITCH: u32 = 1 << 0;
pub const FLAG_SERIF: u32 = 1 << 1;
pub const FLAG_SYMBOLIC: u32 = 1 << 2;
pub const FLAG_FORCE_BOLD: u32 = 1 << 18;

fn get_dict(doc: &Document, d: &Dict, key: &[u8]) -> Option<Dict> {
    match doc.get(d.get(key)?) {
        Object::Dict(x) => Some(x),
        Object::Stream(s) => Some(s.dict),
        _ => None,
    }
}

fn num(doc: &Document, o: &Object) -> Option<f64> {
    doc.get(o).as_f64()
}

fn array(doc: &Document, d: &Dict, key: &[u8]) -> Vec<Object> {
    match doc.get(d.get(key).unwrap_or(&Object::Null)) {
        Object::Array(a) => a,
        _ => Vec::new(),
    }
}

fn stream_data(doc: &Document, o: Option<&Object>) -> Option<(Dict, Vec<u8>)> {
    match doc.get(o?) {
        Object::Stream(s) => {
            let data = doc.decode(&s).ok()?.data;
            Some((s.dict, data))
        }
        _ => None,
    }
}

fn matrix(doc: &Document, d: &Dict, key: &[u8]) -> Option<Matrix> {
    let a = array(doc, d, key);
    if a.len() != 6 {
        return None;
    }
    let mut m = [0.0; 6];
    for (v, o) in m.iter_mut().zip(&a) {
        *v = num(doc, o)?;
    }
    Some(m)
}

/// The /W array (§9.7.4.3): `c [w1 w2 ...]` and `c_first c_last w`.
fn cid_widths(doc: &Document, a: &[Object]) -> Vec<(u32, u32, f64)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < a.len() {
        let Some(first) = num(doc, &a[i]).map(|v| v as u32) else { break };
        match doc.get(a.get(i + 1).unwrap_or(&Object::Null)) {
            Object::Array(ws) => {
                for (k, w) in ws.iter().enumerate() {
                    if let Some(w) = num(doc, w) {
                        out.push((first + k as u32, first + k as u32, w));
                    }
                }
                i += 2;
            }
            o => {
                let (Some(last), Some(w)) = (o.as_f64(), a.get(i + 2).and_then(|x| num(doc, x))) else { break };
                out.push((first, last as u32, w));
                i += 3;
            }
        }
    }
    out
}

/// The font a name / flags call for among the fallback faces: (bytes, bold).
fn fallback<'f>(name: &str, flags: u32, weight: f64, fb: &Fallback<'f>) -> Program<'f> {
    let lower: String = name.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mono = lower.contains("courier") || lower.contains("mono") || flags & FLAG_FIXED_PITCH != 0;
    let serif = lower.contains("times") || lower.contains("serif") || (flags & FLAG_SERIF != 0 && !lower.contains("sans"));
    let bold = lower.contains("bold") || lower.contains("black") || lower.contains("heavy") || weight >= 600.0 || flags & FLAG_FORCE_BOLD != 0;
    let data = if mono { fb.mono.or(fb.sans) } else if serif { fb.serif.or(fb.sans) } else { fb.sans };
    match data {
        Some(data) => Program::Fallback { data, bold },
        None => Program::None,
    }
}

impl<'f> PdfFont<'f> {
    /// Read a font dictionary.
    pub fn load(doc: &Document, d: &Dict, fb: &Fallback<'f>) -> PdfFont<'f> {
        let kind = match d.name(b"Subtype") {
            Some(b"TrueType") => FontKind::TrueType,
            Some(b"Type0") => FontKind::Type0,
            Some(b"Type3") => FontKind::Type3,
            _ => FontKind::Type1,
        };
        let raw_name = d.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
        let base_font = match raw_name.split_once('+') {
            Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => String::from(rest),
            _ => raw_name,
        };
        let mut f = PdfFont {
            kind,
            base_font,
            program: Program::None,
            flags: 0,
            encoding: None,
            widths: Widths { missing: 0.0, ..Widths::default() },
            to_unicode: stream_data(doc, d.get(b"ToUnicode")).map(|(_, data)| CMap::parse(&data)),
            cmap: None,
            cid_to_gid: CidToGid::Identity,
            font_matrix: [0.001, 0.0, 0.0, 0.001, 0.0, 0.0],
            type3: None,
            cff_names: Vec::new(),
            cff_cids: Vec::new(),
        };
        // The dictionary that carries the descriptor and widths: the descendant CIDFont for Type 0.
        let desc_font = if kind == FontKind::Type0 {
            f.cmap = Some(match doc.get(d.get(b"Encoding").unwrap_or(&Object::Null)) {
                Object::Name(n) => CMap::predefined(&n),
                Object::Stream(s) => doc.decode(&s).map(|x| CMap::parse(&x.data)).unwrap_or_else(|_| CMap::identity(false)),
                _ => CMap::identity(false),
            });
            let kids = array(doc, d, b"DescendantFonts");
            match kids.first().map(|k| doc.get(k)) {
                Some(Object::Dict(k)) => k,
                _ => Dict::new(),
            }
        } else {
            d.clone()
        };
        let descriptor = get_dict(doc, &desc_font, b"FontDescriptor").unwrap_or_default();
        f.flags = descriptor.int(b"Flags").and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
        let weight = descriptor.get(b"FontWeight").and_then(|o| num(doc, o)).unwrap_or(0.0);

        // Widths.
        if kind == FontKind::Type0 {
            let w = array(doc, &desc_font, b"W");
            f.widths.present = !w.is_empty();
            f.widths.cid = cid_widths(doc, &w);
            f.widths.missing = desc_font.get(b"DW").and_then(|o| num(doc, o)).unwrap_or(1000.0);
            if let Some((_, data)) = stream_data(doc, desc_font.get(b"CIDToGIDMap")) {
                f.cid_to_gid = CidToGid::Map(data.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect());
            }
        } else {
            let w = array(doc, d, b"Widths");
            f.widths.present = !w.is_empty();
            f.widths.first = d.int(b"FirstChar").and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
            f.widths.widths = w.iter().map(|o| num(doc, o).unwrap_or(0.0)).collect();
            f.widths.missing = descriptor.get(b"MissingWidth").and_then(|o| num(doc, o)).unwrap_or(0.0);
        }

        // Type 3: no program; glyph procedures and the font matrix.
        if kind == FontKind::Type3 {
            if let Some(m) = matrix(doc, d, b"FontMatrix") {
                f.font_matrix = m;
            }
            f.type3 = Some(Type3 {
                char_procs: get_dict(doc, d, b"CharProcs").unwrap_or_default(),
                resources: get_dict(doc, d, b"Resources").unwrap_or_default(),
            });
            let mut enc = Encoding::empty();
            if let Some(e) = get_dict(doc, d, b"Encoding") {
                enc.apply_differences(doc, &array(doc, &e, b"Differences"));
            }
            f.encoding = Some(enc);
            return f;
        }

        // The program.
        if let Some((_, data)) = stream_data(doc, descriptor.get(b"FontFile2")) {
            f.program = Program::Sfnt(data);
        } else if let Some((sd, data)) = stream_data(doc, descriptor.get(b"FontFile3")) {
            f.program = match sd.name(b"Subtype") {
                Some(b"OpenType") => Program::Sfnt(data),
                _ => Program::Cff(data),
            };
        } else if let Some(Object::Stream(s)) = descriptor.get(b"FontFile").map(|o| doc.get(o)) {
            if let Some(t1) = super::type1::load(doc, &s) {
                f.program = Program::Type1(t1);
            }
        }
        // A program font_core will not read draws with the fallback face, like an unembedded font.
        let readable = match &f.program {
            Program::Sfnt(b) => font_core::Font::parse(b).is_ok(),
            Program::Cff(b) => font_core::cff::Cff::parse(b, 0).is_some(),
            Program::Type1(_) => true,
            _ => false,
        };
        if !readable {
            f.program = fallback(&f.base_font, f.flags, weight, fb);
        }
        f.index_cff();

        // Simple fonts: the encoding (§9.6.6).
        if kind != FontKind::Type0 {
            let symbolic = f.flags & FLAG_SYMBOLIC != 0 || matches!(f.base_font.as_str(), "Symbol" | "ZapfDingbats");
            let builtin = || -> Option<Encoding> {
                match &f.program {
                    Program::Type1(t1) => Some(super::type1::builtin_encoding(t1)),
                    // CFF built-in encodings (Top DICT op 16) are owed: Standard for a non-symbolic font.
                    _ if symbolic => None,
                    _ => Some(Encoding::from_base(Base::Standard)),
                }
            };
            f.encoding = match doc.get(d.get(b"Encoding").unwrap_or(&Object::Null)) {
                Object::Name(n) => Base::from_name(&n).map(Encoding::from_base).or_else(builtin),
                Object::Dict(e) => {
                    let mut enc = match e.name(b"BaseEncoding").and_then(Base::from_name) {
                        Some(b) => Some(Encoding::from_base(b)),
                        None => builtin(),
                    }
                    .unwrap_or_else(Encoding::empty);
                    enc.apply_differences(doc, &array(doc, &e, b"Differences"));
                    Some(enc)
                }
                _ => builtin(),
            };
        }
        f
    }

    /// For a CFF program (bare or inside an OpenType sfnt): the name -> gid and CID -> gid indexes.
    fn index_cff(&mut self) {
        let index = |c: &font_core::cff::Cff<'_>, names: &mut Vec<(String, u16)>, cids: &mut Vec<(u16, u16)>, cid_keyed: bool| {
            for g in 0..c.glyph_count().min(65535) as u16 {
                if cid_keyed {
                    if let Some(cid) = c.glyph_sid(g) {
                        cids.push((cid, g));
                    }
                } else if let Some(n) = c.glyph_name(g) {
                    names.push((n, g));
                }
            }
        };
        let (mut names, mut cids) = (Vec::new(), Vec::new());
        let cid_keyed = self.kind == FontKind::Type0;
        match &self.program {
            Program::Cff(b) => {
                if let Some(c) = font_core::cff::Cff::parse(b, 0) {
                    index(&c, &mut names, &mut cids, cid_keyed);
                }
            }
            Program::Sfnt(b) => {
                if let Ok(font) = font_core::Font::parse(b) {
                    if let font_core::Outlines::Cff(c) = font.outlines {
                        index(&c, &mut names, &mut cids, cid_keyed);
                    }
                }
            }
            _ => {}
        }
        names.sort();
        names.dedup_by(|a, b| a.0 == b.0);
        cids.sort();
        cids.dedup_by(|a, b| a.0 == b.0);
        self.cff_names = names;
        self.cff_cids = cids;
    }

    /// The width of `code` (Type 0: of its CID) in glyph space, when the dictionary says (`None`: use the program's).
    pub fn dict_width(&self, code: u32, cid: u32) -> Option<f64> {
        let w = &self.widths;
        if !w.present {
            return None;
        }
        if self.kind == FontKind::Type0 {
            return Some(w.cid.iter().rev().find(|r| r.0 <= cid && cid <= r.1).map(|r| r.2).unwrap_or(w.missing));
        }
        let i = code.checked_sub(w.first).map(|i| i as usize);
        Some(i.and_then(|i| w.widths.get(i).copied()).unwrap_or(w.missing))
    }

    /// The Unicode text of `code` for extraction: /ToUnicode, else the encoding's glyph name through the AGL.
    pub fn unicode(&self, code: u32) -> Option<String> {
        if let Some(u) = self.to_unicode.as_ref().and_then(|m| m.unicode(code)) {
            return Some(u);
        }
        let name = self.encoding.as_ref()?.name(u8::try_from(code).ok()?)?;
        super::encoding::agl(name)
    }
}
