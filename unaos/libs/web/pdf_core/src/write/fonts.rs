// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDF-WRITE-FONTS (LEDGER SR338): the ONE TrueType subsetter and the Type0 /
//! CIDFontType2 objects the [`super::Builder`] embeds for [`super::PageBuilder::text`]. The face is read only through
//! font_core (`Font::parse`, `table`, `glyph_index`, `advance`, `Glyf::glyph_data` / `components`, `name::values`); this
//! file only WRITES sfnt tables. The program is compressed only through builder.rs's `flate` (pixel_core).
//!
//! * Glyph ids are renumbered at FIRST USE in `PageBuilder::text` (new 0 = .notdef), so the content stream's 2-byte CIDs
//!   are final when written and /CIDToGIDMap is /Identity; composite components are pulled in at `finish`, after the
//!   used ids.
//! * The subset keeps head / hhea / maxp / hmtx / loca / glyf / cmap / post (and cvt / fpgm / prep verbatim); every other
//!   table is dropped, so a variable face embeds its default instance. Checksums and `head.checkSumAdjustment` are
//!   recomputed.
//! * CFF and OpenType-CFF faces are refused by name (PDF-WRITE-CFF is owed).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

use font_core::glyf::{self, Component, Glyf};
use font_core::{Font, Outlines};

use super::builder::flate;
use crate::Error;
use crate::object::{Dict, Object, Ref, Stream};

/// One loaded face of one [`super::Builder`] (`Builder::font`): shared by every page that shows text in it, embedded
/// once per document as a subset of the glyphs the pages used. A handle belongs to the Builder that made it.
#[derive(Clone)]
pub struct FontHandle(pub(crate) Rc<RefCell<FontState>>);

impl fmt::Debug for FontHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let st = self.0.borrow();
        write!(f, "FontHandle(F{}, {} glyphs used)", st.id, st.gids.len() - 1)
    }
}

/// The face bytes and the renumbering so far.
pub(crate) struct FontState {
    /// The resource name is `/F{id}`.
    pub(crate) id: u32,
    data: Vec<u8>,
    /// New gid -> source gid; `gids[0]` is .notdef.
    gids: Vec<u16>,
    /// Source gid -> new gid.
    map: BTreeMap<u16, u16>,
    /// New gid -> the first character shown with it (the /ToUnicode map).
    uni: BTreeMap<u16, char>,
}

const CFF_REFUSED: &str = "PDF-WRITE-CFF: CFF / OpenType-CFF font (only TrueType glyf faces are subset)";

/// Read a face for embedding: TrueType (`glyf`) only.
pub(crate) fn load(id: u32, data: &[u8]) -> Result<FontState, Error> {
    if data.starts_with(b"OTTO") {
        return Err(Error::Unsupported(CFF_REFUSED));
    }
    let f = Font::parse(data).map_err(|_| Error::Unsupported("PDF-WRITE-FONTS: font_core cannot parse the face"))?;
    if f.table(b"CFF ").is_some() || f.table(b"CFF2").is_some() {
        return Err(Error::Unsupported(CFF_REFUSED));
    }
    if !matches!(f.outlines, Outlines::Glyf(_)) {
        return Err(Error::Unsupported("PDF-WRITE-FONTS: a face without glyf outlines"));
    }
    Ok(FontState { id, data: data.to_vec(), gids: alloc::vec![0], map: BTreeMap::new(), uni: BTreeMap::new() })
}

impl FontState {
    /// `s` as 2-byte big-endian CIDs, one glyph per character through font_core's cmap lookup; a character the face
    /// does not map is CID 0 (.notdef).
    pub(crate) fn encode(&mut self, s: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 * s.len());
        let Ok(f) = Font::parse(&self.data) else { return out };
        for c in s.chars() {
            let old = f.glyph_index(c);
            let new = if old == 0 {
                0
            } else if let Some(&n) = self.map.get(&old) {
                n
            } else if self.gids.len() < 0xFFFF {
                let n = self.gids.len() as u16;
                self.gids.push(old);
                self.map.insert(old, n);
                n
            } else {
                0
            };
            if new != 0 {
                self.uni.entry(new).or_insert(c);
            }
            out.extend_from_slice(&new.to_be_bytes());
        }
        out
    }

    /// The used ids plus every composite component they reach (appended in discovery order): new -> source, and
    /// source -> new.
    fn closure(&self, g: &Glyf<'_>, num_glyphs: u16) -> (Vec<u16>, BTreeMap<u16, u16>) {
        let mut order = self.gids.clone();
        let mut map = self.map.clone();
        map.insert(0, 0);
        let mut i = 0;
        while i < order.len() {
            for c in g.components(order[i]).unwrap_or_default() {
                if c.glyph < num_glyphs && !map.contains_key(&c.glyph) && order.len() < 0xFFFF {
                    map.insert(c.glyph, order.len() as u16);
                    order.push(c.glyph);
                }
            }
            i += 1;
        }
        (order, map)
    }

    /// The subset TrueType program and its new -> source glyph order.
    fn subset(&self, f: &Font<'_>) -> (Vec<u8>, Vec<u16>) {
        let Outlines::Glyf(g) = f.outlines else { return (Vec::new(), Vec::new()) };
        let (order, map) = self.closure(&g, f.num_glyphs);
        let n = order.len() as u16;
        let mut glyf_t = Vec::new();
        let mut loca = Vec::with_capacity(4 * (order.len() + 1));
        let mut hmtx = Vec::with_capacity(4 * order.len());
        for &old in &order {
            loca.extend_from_slice(&(glyf_t.len() as u32).to_be_bytes());
            let data = g.glyph_data(old).unwrap_or(&[]);
            match g.components(old) {
                Some(cs) if !cs.is_empty() => glyf_t.extend_from_slice(&renumber_components(data, &cs, &map)),
                _ => glyf_t.extend_from_slice(data),
            }
            while glyf_t.len() % 4 != 0 {
                glyf_t.push(0);
            }
            hmtx.extend_from_slice(&f.advance(old).to_be_bytes());
            hmtx.extend_from_slice(&f.lsb(old).to_be_bytes());
        }
        loca.extend_from_slice(&(glyf_t.len() as u32).to_be_bytes());
        let mut head = f.table(b"head").unwrap_or(&[]).to_vec();
        if head.len() >= 52 {
            head[8..12].copy_from_slice(&[0; 4]); // checkSumAdjustment, set over the whole file below
            head[50..52].copy_from_slice(&1u16.to_be_bytes()); // indexToLocFormat: long
        }
        let mut hhea = f.table(b"hhea").unwrap_or(&[]).to_vec();
        if hhea.len() >= 36 {
            hhea[34..36].copy_from_slice(&n.to_be_bytes());
        }
        let mut maxp = f.table(b"maxp").unwrap_or(&[]).to_vec();
        if maxp.len() >= 6 {
            maxp[4..6].copy_from_slice(&n.to_be_bytes());
        }
        let mut post = match f.table(b"post") {
            Some(p) if p.len() >= 32 => p[..32].to_vec(),
            _ => alloc::vec![0; 32],
        };
        post[..4].copy_from_slice(&0x0003_0000u32.to_be_bytes()); // version 3.0: no glyph names
        let mut tables: Vec<([u8; 4], Vec<u8>)> = alloc::vec![
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"maxp", maxp),
            (*b"hmtx", hmtx),
            (*b"loca", loca),
            (*b"glyf", glyf_t),
            (*b"cmap", self.cmap_table()),
            (*b"post", post),
        ];
        for tag in [b"cvt ", b"fpgm", b"prep"] {
            if let Some(t) = f.table(tag) {
                tables.push((*tag, t.to_vec()));
            }
        }
        (sfnt(tables), order)
    }

    /// One (3, 1) format 4 subtable: each used BMP character -> its new gid, one segment per character.
    fn cmap_table(&self) -> Vec<u8> {
        let mut pairs: Vec<(u16, u16)> = self.uni.iter().filter(|&(_, &c)| (c as u32) < 0xFFFF).map(|(&g, &c)| (c as u32 as u16, g)).collect();
        pairs.sort_unstable();
        pairs.dedup_by_key(|p| p.0);
        pairs.push((0xFFFF, 0));
        let segs = pairs.len() as u16;
        let (search, selector) = search_fields(segs, 2);
        let mut st = Vec::new();
        for v in [4u16, 16 + 8 * segs, 0, 2 * segs, search, selector, 2 * segs - search] {
            st.extend_from_slice(&v.to_be_bytes());
        }
        for &(c, _) in &pairs {
            st.extend_from_slice(&c.to_be_bytes()); // endCode
        }
        st.extend_from_slice(&[0, 0]); // reservedPad
        for &(c, _) in &pairs {
            st.extend_from_slice(&c.to_be_bytes()); // startCode
        }
        for &(c, g) in &pairs {
            let delta = if c == 0xFFFF { 1 } else { g.wrapping_sub(c) };
            st.extend_from_slice(&delta.to_be_bytes()); // idDelta
        }
        for _ in &pairs {
            st.extend_from_slice(&[0, 0]); // idRangeOffset
        }
        let mut t = Vec::with_capacity(12 + st.len());
        for v in [0u16, 1, 3, 1] {
            t.extend_from_slice(&v.to_be_bytes());
        }
        t.extend_from_slice(&12u32.to_be_bytes());
        t.extend_from_slice(&st);
        t
    }

    /// The /ToUnicode CMap: new gid (= CID) -> the character, bfchar blocks of at most 100, UTF-16BE.
    fn to_unicode(&self) -> Vec<u8> {
        let mut s = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let pairs: Vec<(&u16, &char)> = self.uni.iter().collect();
        for block in pairs.chunks(100) {
            s.push_str(&format!("{} beginbfchar\n", block.len()));
            for (g, c) in block {
                let mut units = [0u16; 2];
                let hex: String = c.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
                s.push_str(&format!("<{:04X}> <{hex}>\n", g));
            }
            s.push_str("endbfchar\n");
        }
        s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        s.into_bytes()
    }
}

/// A composite glyph's bytes with each component's glyph index renumbered. The record offsets follow from font_core's
/// parsed flags (`Glyf::components`): 4 bytes of flags and index, the args (2 or 4), the transform (0, 2, 4 or 8).
fn renumber_components(data: &[u8], comps: &[Component], map: &BTreeMap<u16, u16>) -> Vec<u8> {
    let mut out = data.to_vec();
    let mut at = 10;
    for c in comps {
        let gi = at + 2;
        if gi + 2 <= out.len() {
            let new = map.get(&c.glyph).copied().unwrap_or(0);
            out[gi..gi + 2].copy_from_slice(&new.to_be_bytes());
        }
        let args = if c.flags & glyf::ARG_1_AND_2_ARE_WORDS != 0 { 4 } else { 2 };
        let xform = if c.flags & glyf::WE_HAVE_A_SCALE != 0 {
            2
        } else if c.flags & glyf::WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            4
        } else if c.flags & glyf::WE_HAVE_A_TWO_BY_TWO != 0 {
            8
        } else {
            0
        };
        at += 4 + args + xform;
    }
    out
}

/// (searchRange, entrySelector) for `n` entries of `unit` bytes: the largest power of two <= n, times `unit`.
fn search_fields(n: u16, unit: u16) -> (u16, u16) {
    let mut p = 1u16;
    let mut e = 0u16;
    while p.saturating_mul(2) <= n {
        p *= 2;
        e += 1;
    }
    (p.saturating_mul(unit), e)
}

/// The sum of a table's big-endian u32 words, zero-padded to 4 bytes.
fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |s, w| {
        let mut b = [0u8; 4];
        b[..w.len()].copy_from_slice(w);
        s.wrapping_add(u32::from_be_bytes(b))
    })
}

/// An sfnt file (version 1.0): the directory sorted by tag, each table 4-byte aligned with its checksum, and
/// `head.checkSumAdjustment = 0xB1B0AFBA - sum(file)`.
fn sfnt(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by(|a, b| a.0.cmp(&b.0));
    let n = tables.len() as u16;
    let (search, selector) = search_fields(n, 16);
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for v in [n, search, selector, 16 * n - search] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut off = 12 + 16 * tables.len();
    let mut head_at = None;
    for (tag, t) in &tables {
        if tag == b"head" {
            head_at = Some(off);
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(t).to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(t.len() as u32).to_be_bytes());
        off += t.len().div_ceil(4) * 4;
    }
    for (_, t) in &tables {
        out.extend_from_slice(t);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    if let Some(h) = head_at {
        if h + 12 <= out.len() {
            let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
            out[h + 8..h + 12].copy_from_slice(&adj.to_be_bytes());
        }
    }
    out
}

fn name(n: &[u8]) -> Object {
    Object::Name(n.to_vec())
}

/// A stream, FlateDecode through pixel_core when the compressor accepts it.
fn flate_stream(mut d: Dict, raw: Vec<u8>) -> Stream {
    match flate(&raw) {
        Some(z) => {
            d.insert(b"Filter".to_vec(), name(b"FlateDecode"));
            Stream { dict: d, data: z }
        }
        None => Stream { dict: d, data: raw },
    }
}

/// The six-letter subset tag (§9.6.4): an FNV-1a of the glyph order mapped to A-Z, so the output is deterministic.
fn subset_tag(order: &[u16]) -> String {
    let mut h = order.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &g| {
        let h = (h ^ (g >> 8) as u64).wrapping_mul(0x0100_0000_01b3);
        (h ^ (g & 0xFF) as u64).wrapping_mul(0x0100_0000_01b3)
    });
    let mut s = String::new();
    for _ in 0..6 {
        s.push((b'A' + (h % 26) as u8) as char);
        h /= 26;
    }
    s
}

/// The face's PostScript name with only the bytes a PDF name may carry unescaped, else `F{id}`.
fn ps_name(f: &Font<'_>, id: u32) -> String {
    let raw = font_core::name::values(f, font_core::name::POSTSCRIPT).into_iter().next().unwrap_or_default();
    let clean: String = raw.chars().filter(|&c| c.is_ascii_graphic() && !"[](){}<>/%#".contains(c)).collect();
    if clean.is_empty() { format!("F{id}") } else { clean }
}

/// The font's five objects at `base`..`base + 4`: the Type0 font (referenced from the pages), its CIDFontType2, the
/// FontDescriptor, the FontFile2 program and the /ToUnicode CMap.
pub(crate) fn objects(st: &FontState, base: u32) -> Vec<(Ref, Object)> {
    let r = |k: u32| Ref { num: base + k, generation: 0 };
    let Ok(f) = Font::parse(&st.data) else { return Vec::new() };
    let (program, order) = st.subset(&f);
    let upem = f.units_per_em.max(1) as f64;
    let scale = |v: f64| Object::Int(svg_core::fmath::floor(v * 1000.0 / upem + 0.5) as i64);
    let base_font = format!("{}+{}", subset_tag(&order), ps_name(&f, st.id)).into_bytes();

    let mut t0 = Dict::new();
    t0.insert(b"Type".to_vec(), name(b"Font"));
    t0.insert(b"Subtype".to_vec(), name(b"Type0"));
    t0.insert(b"BaseFont".to_vec(), Object::Name(base_font.clone()));
    t0.insert(b"Encoding".to_vec(), name(b"Identity-H"));
    t0.insert(b"DescendantFonts".to_vec(), Object::Array(alloc::vec![Object::Ref(r(1))]));
    t0.insert(b"ToUnicode".to_vec(), Object::Ref(r(4)));

    let mut csi = Dict::new();
    csi.insert(b"Registry".to_vec(), Object::String(b"Adobe".to_vec()));
    csi.insert(b"Ordering".to_vec(), Object::String(b"Identity".to_vec()));
    csi.insert(b"Supplement".to_vec(), Object::Int(0));
    let widths: Vec<Object> = order.iter().map(|&g| Object::Real(f.advance(g) as f64 * 1000.0 / upem)).collect();
    let mut cid = Dict::new();
    cid.insert(b"Type".to_vec(), name(b"Font"));
    cid.insert(b"Subtype".to_vec(), name(b"CIDFontType2"));
    cid.insert(b"BaseFont".to_vec(), Object::Name(base_font.clone()));
    cid.insert(b"CIDSystemInfo".to_vec(), Object::Dict(csi));
    cid.insert(b"FontDescriptor".to_vec(), Object::Ref(r(2)));
    cid.insert(b"DW".to_vec(), Object::Int(1000));
    cid.insert(b"W".to_vec(), Object::Array(alloc::vec![Object::Int(0), Object::Array(widths)]));
    cid.insert(b"CIDToGIDMap".to_vec(), name(b"Identity"));

    let (italic, fixed) = f.post.map(|p| (p.italic_angle, p.is_fixed_pitch)).unwrap_or((0.0, false));
    let flags = 4 | if fixed { 1 } else { 0 } | if italic != 0.0 { 64 } else { 0 };
    let cap = f.os2.and_then(|o| o.cap_height).unwrap_or(f.ascender);
    let mut desc = Dict::new();
    desc.insert(b"Type".to_vec(), name(b"FontDescriptor"));
    desc.insert(b"FontName".to_vec(), Object::Name(base_font));
    desc.insert(b"Flags".to_vec(), Object::Int(flags));
    desc.insert(b"FontBBox".to_vec(), Object::Array(f.head_bbox.iter().map(|&v| scale(v as f64)).collect()));
    desc.insert(b"ItalicAngle".to_vec(), Object::Real(italic as f64));
    desc.insert(b"Ascent".to_vec(), scale(f.ascender as f64));
    desc.insert(b"Descent".to_vec(), scale(f.descender as f64));
    desc.insert(b"CapHeight".to_vec(), scale(cap as f64));
    desc.insert(b"StemV".to_vec(), Object::Int(80)); // no sfnt field carries it: the conventional estimate
    desc.insert(b"FontFile2".to_vec(), Object::Ref(r(3)));

    let mut ff = Dict::new();
    ff.insert(b"Length1".to_vec(), Object::Int(program.len() as i64));
    alloc::vec![
        (r(0), Object::Dict(t0)),
        (r(1), Object::Dict(cid)),
        (r(2), Object::Dict(desc)),
        (r(3), Object::Stream(flate_stream(ff, program))),
        (r(4), Object::Stream(flate_stream(Dict::new(), st.to_unicode()))),
    ]
}
