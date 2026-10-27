// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! JPXDecode (ISO 32000-1 §7.4.9; ITU-T T.800 | ISO/IEC 15444-1, JPEG 2000 Part 1) — PDFJPX SR335.
//!
//! CHARTER: Aether — shared-core. pdf_core's JPEG 2000 decoder. Tier-1 decodes through the ONE MQ coder (`mq.rs`:
//! T.800 Annex C is the coder of T.88 Annex E), never a second copy; `render::image` maps the samples this module
//! returns to colour through its one raw-sample path (§7.4.9: /ColorSpace wins, else the [`Space`] read here).
//!
//! Built: the JP2 boxes (signature, jp2h's colr: enumerated sRGB / greyscale / sYCC / CMYK; an ICC profile reads as
//! [`Space::Unknown`], i.e. by channel count) and raw codestreams; SIZ, COD, COC, QCD, QCC in the main and the
//! tile-part headers; tiles over any number of tile-parts; precincts; the LRCP, RLCP and RPCL progressions; packet
//! headers (tag trees, inclusion, zero bit-planes, pass counts, Lblock, SOP / EPH); EBCOT (Annex D) with selective
//! arithmetic bypass, context reset, termination on every pass, predictable termination and the segmentation
//! symbol; scalar dequantisation (E.1); the reversible 5/3 and irreversible 9/7 inverse wavelets (Annex F); the RCT
//! and ICT (Annex G; the ICT also undoes an sYCC colr); the DC level shift to 8-bit samples; subsampled components
//! replicated. Refused by name: palettes ("JPX palette"), Part 2 and HTJ2K codestreams, depths over 16 bits, CPRL /
//! PCRL progressions, POC, RGN, packed packet headers (PPM / PPT), vertical-causal contexts. A truncated stream keeps
//! what arrived. Everything is bounded (`MAX_DECODED`, and the caps below).

use alloc::vec;
use alloc::vec::Vec;

use super::mq::MqDecoder;
use crate::{Error, MAX_DECODED};

/// The colour space the JP2 colr box names: what §7.4.9 uses when the image dictionary has no /ColorSpace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// No colr box, an ICC profile or an enumeration this core does not map: read by the component count.
    Unknown,
}

/// A decoded image: `comps` 8-bit samples per pixel, interleaved, rows top to bottom (the codestream's own size).
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub comps: usize,
    pub space: Space,
    pub data: Vec<u8>,
}

/// Components per image.
pub const MAX_COMPS: usize = 16;
/// Tiles per image (Isot is 16 bits).
pub const MAX_TILES: usize = 65535;
/// Packets per tile.
pub const MAX_PACKETS: usize = 1 << 22;
/// Precincts, and code-blocks, per tile.
pub const MAX_BLOCKS: usize = 1 << 22;

const JP2_SIGNATURE: [u8; 12] = [0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];

fn syn(why: &'static str) -> Error {
    Error::Syntax(why, 0)
}

fn be16(d: &[u8], at: usize) -> Result<usize, Error> {
    match d.get(at..at + 2) {
        Some(b) => Ok((b[0] as usize) << 8 | b[1] as usize),
        None => Err(syn("JPX: short segment")),
    }
}

fn be32(d: &[u8], at: usize) -> Result<usize, Error> {
    match d.get(at..at + 4) {
        Some(b) => Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize),
        None => Err(syn("JPX: short segment")),
    }
}

/// ceil(x / 2^s).
fn cpow(x: usize, s: usize) -> usize {
    if s >= 63 { usize::from(x > 0) } else { ((x as u64 + (1u64 << s) - 1) >> s) as usize }
}

/// 2^e as an f32 (e clamped to the normal range).
fn pow2(e: i32) -> f32 {
    f32::from_bits(((e.clamp(-126, 127) + 127) as u32) << 23)
}

/// Decode a JPXDecode stream: a JP2 file (boxes) or a raw codestream.
pub fn decode(data: &[u8]) -> Result<Image, Error> {
    let (cs, space, ycc) = if data.starts_with(&JP2_SIGNATURE) { boxes(data)? } else { (data, Space::Unknown, false) };
    let mut img = codestream(cs)?;
    img.space = space;
    if ycc && img.comps >= 3 {
        sycc_to_rgb(&mut img);
    }
    Ok(img)
}

// ---------------------------------------------------------------------------------------------------------------
// The JP2 file format (Annex I)

/// The top-level box walk: (the codestream, the colr space, whether it is sYCC).
fn boxes(data: &[u8]) -> Result<(&[u8], Space, bool), Error> {
    let (mut space, mut ycc) = (Space::Unknown, false);
    let mut at = 0;
    while at + 8 <= data.len() {
        let (body, next) = one_box(data, at)?;
        match &data[at + 4..at + 8] {
            b"jp2h" => {
                let mut i = 0;
                while i + 8 <= body.len() {
                    let (b, n) = one_box(body, i)?;
                    match &body[i + 4..i + 8] {
                        // METH 1: an enumerated colour space; 2 / 3 (ICC) stay Unknown (PDF-ICC owed).
                        b"colr" if b.first() == Some(&1) => match be32(b, 3)? {
                            16 => space = Space::Rgb,
                            17 => space = Space::Gray,
                            18 => (space, ycc) = (Space::Rgb, true),
                            12 => space = Space::Cmyk,
                            _ => {}
                        },
                        b"pclr" => return Err(Error::Unsupported("JPX palette")),
                        _ => {}
                    }
                    i = n;
                }
            }
            b"jp2c" => return Ok((body, space, ycc)),
            _ => {}
        }
        at = next;
    }
    Err(syn("JPX: no jp2c box"))
}

/// One box at `at`: its body and the offset after it (LBox 1: an XLBox follows; 0: to the end). A box running past
/// the end of the data is cut there (a truncated file keeps what arrived).
fn one_box(d: &[u8], at: usize) -> Result<(&[u8], usize), Error> {
    let lbox = be32(d, at)?;
    let (start, end) = match lbox {
        0 => (at + 8, d.len()),
        1 => {
            let (hi, lo) = (be32(d, at + 8)?, be32(d, at + 12)?);
            if hi != 0 {
                return Err(Error::TooLarge);
            }
            if lo < 16 {
                return Err(syn("JPX: box length"));
            }
            (at + 16, at.saturating_add(lo))
        }
        n if n < 8 => return Err(syn("JPX: box length")),
        n => (at + 8, at.saturating_add(n)),
    };
    let end = end.min(d.len());
    if start > end {
        return Err(syn("JPX: box past the end"));
    }
    Ok((&d[start..end], end))
}

// ---------------------------------------------------------------------------------------------------------------
// The codestream headers (Annex A)

#[derive(Clone, Copy)]
struct Comp {
    depth: u32,
    dx: usize,
    dy: usize,
}

struct Siz {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    tw: usize,
    th: usize,
    tx0: usize,
    ty0: usize,
    ntx: usize,
    nty: usize,
    comps: Vec<Comp>,
}

/// A component's coding style (COD / COC SPcod).
#[derive(Clone)]
struct Cod {
    levels: usize,
    xcb: usize,
    ycb: usize,
    style: u8,
    reversible: bool,
    /// (PPx, PPy) per resolution level.
    pp: [(u8, u8); 33],
}

/// A component's quantisation (QCD / QCC): style, guard bits, (exponent, mantissa) per subband.
#[derive(Clone)]
struct Qcd {
    style: u8,
    guard: u32,
    steps: Vec<(u32, u32)>,
}

/// The coding parameters in force (main header, then a tile's own markers over a copy of it).
#[derive(Clone)]
struct Coding {
    prog: u8,
    layers: usize,
    mct: bool,
    sop: bool,
    eph: bool,
    cod: Vec<Cod>,
    qcd: Vec<Qcd>,
    coc_set: Vec<bool>,
    qcc_set: Vec<bool>,
}

impl Coding {
    fn new(nc: usize) -> Coding {
        let cod = Cod { levels: 0, xcb: 6, ycb: 6, style: 0, reversible: true, pp: [(15, 15); 33] };
        let qcd = Qcd { style: 0, guard: 1, steps: Vec::new() };
        Coding {
            prog: 0,
            layers: 1,
            mct: false,
            sop: false,
            eph: false,
            cod: vec![cod; nc],
            qcd: vec![qcd; nc],
            coc_set: vec![false; nc],
            qcc_set: vec![false; nc],
        }
    }

    /// A tile's starting copy: its COD / QCD override the main header's COC / QCC too (A.6 precedence).
    fn fresh(&self) -> Coding {
        let mut c = self.clone();
        c.coc_set.iter_mut().for_each(|b| *b = false);
        c.qcc_set.iter_mut().for_each(|b| *b = false);
        c
    }
}

fn siz(s: &[u8]) -> Result<Siz, Error> {
    let rsiz = be16(s, 0)?;
    if rsiz & 0x8000 != 0 {
        return Err(Error::Unsupported("JPX Part 2"));
    }
    if rsiz & 0x4000 != 0 {
        return Err(Error::Unsupported("JPX HTJ2K"));
    }
    let f = |i: usize| be32(s, 2 + 4 * i);
    let (x1, y1, x0, y0, tw, th, tx0, ty0) = (f(0)?, f(1)?, f(2)?, f(3)?, f(4)?, f(5)?, f(6)?, f(7)?);
    let nc = be16(s, 34)?;
    if nc == 0 || nc > MAX_COMPS {
        return Err(Error::Unsupported("JPX component count"));
    }
    let mut comps = Vec::with_capacity(nc);
    for c in 0..nc {
        let b = s.get(36 + 3 * c..39 + 3 * c).ok_or(syn("JPX: short SIZ"))?;
        let depth = (b[0] & 0x7F) as u32 + 1;
        if depth > 16 {
            return Err(Error::Unsupported("JPX precision"));
        }
        if b[1] == 0 || b[2] == 0 {
            return Err(syn("JPX: component subsampling"));
        }
        comps.push(Comp { depth, dx: b[1] as usize, dy: b[2] as usize });
    }
    if x1 <= x0 || y1 <= y0 || tw == 0 || th == 0 || tx0 > x0 || ty0 > y0 || tx0 + tw <= x0 || ty0 + th <= y0 {
        return Err(syn("JPX: SIZ geometry"));
    }
    let (w, h) = ((x1 - x0) as u64, (y1 - y0) as u64);
    if w > 1 << 24 || h > 1 << 24 || w * h * nc as u64 * 4 > MAX_DECODED as u64 {
        return Err(Error::TooLarge);
    }
    let (ntx, nty) = ((x1 - tx0).div_ceil(tw), (y1 - ty0).div_ceil(th));
    if ntx as u64 * nty as u64 > MAX_TILES as u64 {
        return Err(Error::TooLarge);
    }
    Ok(Siz { x0, y0, x1, y1, tw, th, tx0, ty0, ntx, nty, comps })
}

/// SPcod / SPcoc, with the Scod / Scoc precinct flag.
fn spcod(s: &[u8], scod: u8) -> Result<Cod, Error> {
    let b = s.get(..5).ok_or(syn("JPX: short COD"))?;
    let levels = b[0] as usize;
    let (xcb, ycb) = (b[1] as usize + 2, b[2] as usize + 2);
    let style = b[3];
    if levels > 32 {
        return Err(syn("JPX: decomposition levels"));
    }
    if xcb > 10 || ycb > 10 || xcb + ycb > 12 {
        return Err(syn("JPX: code-block size"));
    }
    if style & 0x08 != 0 {
        return Err(Error::Unsupported("JPX vertical causal"));
    }
    if style & 0xC0 != 0 {
        return Err(Error::Unsupported("JPX HTJ2K"));
    }
    let reversible = match b[4] {
        0 => false,
        1 => true,
        _ => return Err(Error::Unsupported("JPX Part 2")),
    };
    let mut pp = [(15u8, 15u8); 33];
    if scod & 1 != 0 {
        for (r, slot) in pp.iter_mut().enumerate().take(levels + 1) {
            let v = *s.get(5 + r).ok_or(syn("JPX: short COD"))?;
            *slot = (v & 15, v >> 4);
            if r > 0 && (slot.0 == 0 || slot.1 == 0) {
                return Err(syn("JPX: precinct size"));
            }
        }
    }
    Ok(Cod { levels, xcb, ycb, style, reversible, pp })
}

/// SQcd / SPqcd.
fn quant(s: &[u8]) -> Result<Qcd, Error> {
    let sq = *s.first().ok_or(syn("JPX: short QCD"))?;
    let (style, guard) = (sq & 0x1F, (sq >> 5) as u32);
    let rest = &s[1..];
    let steps = match style {
        0 => rest.iter().map(|&b| ((b >> 3) as u32, 0)).collect(),
        1 | 2 => {
            let n = if style == 1 { 1.min(rest.len() / 2) } else { rest.len() / 2 };
            (0..n).map(|i| {
                let v = (rest[2 * i] as u32) << 8 | rest[2 * i + 1] as u32;
                (v >> 11, v & 0x7FF)
            })
            .collect()
        }
        _ => return Err(syn("JPX: quantisation style")),
    };
    Ok(Qcd { style, guard, steps })
}

/// A COC / QCC component index (one byte below 257 components, else two) and the bytes after it.
fn comp_index(s: &[u8], nc: usize) -> Result<(usize, usize), Error> {
    let (c, k) = if nc < 257 { (*s.first().ok_or(syn("JPX: short segment"))? as usize, 1) } else { (be16(s, 0)?, 2) };
    if c >= nc {
        return Err(syn("JPX: component index"));
    }
    Ok((c, k))
}

/// Apply one header marker segment to the coding parameters.
fn marker(m: usize, s: &[u8], co: &mut Coding, nc: usize) -> Result<(), Error> {
    match m {
        0xFF52 => {
            let b = s.get(..5).ok_or(syn("JPX: short COD"))?;
            co.prog = match b[1] {
                p @ 0..=2 => p,
                3 | 4 => return Err(Error::Unsupported("JPX progression CPRL/PCRL")),
                _ => return Err(syn("JPX: progression order")),
            };
            co.layers = be16(b, 2)?;
            if co.layers == 0 {
                return Err(syn("JPX: no layers"));
            }
            co.mct = b[4] == 1;
            co.sop = b[0] & 2 != 0;
            co.eph = b[0] & 4 != 0;
            let cod = spcod(&s[5..], b[0])?;
            for c in 0..nc {
                if !co.coc_set[c] {
                    co.cod[c] = cod.clone();
                }
            }
        }
        0xFF53 => {
            let (c, k) = comp_index(s, nc)?;
            let scoc = *s.get(k).ok_or(syn("JPX: short COC"))?;
            co.cod[c] = spcod(&s[k + 1..], scoc)?;
            co.coc_set[c] = true;
        }
        0xFF5C => {
            let q = quant(s)?;
            for c in 0..nc {
                if !co.qcc_set[c] {
                    co.qcd[c] = q.clone();
                }
            }
        }
        0xFF5D => {
            let (c, k) = comp_index(s, nc)?;
            co.qcd[c] = quant(&s[k..])?;
            co.qcc_set[c] = true;
        }
        0xFF5E => return Err(Error::Unsupported("JPX region of interest")),
        0xFF5F => return Err(Error::Unsupported("JPX progression order change")),
        0xFF60 | 0xFF61 => return Err(Error::Unsupported("JPX packed packet headers")),
        // TLM, PLM, PLT, CRG, COM and anything else informational: skipped by its length.
        _ => {}
    }
    Ok(())
}

/// One marker segment at `at`: (marker, body, offset after it).
fn segment(cs: &[u8], at: usize) -> Result<(usize, &[u8], usize), Error> {
    let m = be16(cs, at)?;
    if m & 0xFF00 != 0xFF00 {
        return Err(syn("JPX: marker expected"));
    }
    let l = be16(cs, at + 2)?;
    let body = cs.get(at + 4..at + 2 + l.max(2)).ok_or(syn("JPX: short segment"))?;
    Ok((m, body, at + 2 + l.max(2)))
}

/// A component plane at its own resolution (pre-shift sample values).
struct Plane {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    data: Vec<f32>,
}

fn codestream(cs: &[u8]) -> Result<Image, Error> {
    if !cs.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        return Err(syn("JPX: no SOC / SIZ"));
    }
    let (_, sbody, mut at) = segment(cs, 2)?;
    let siz = siz(sbody)?;
    let nc = siz.comps.len();
    let mut main = Coding::new(nc);
    let (mut have_cod, mut have_qcd) = (false, false);
    loop {
        let (m, body, next) = segment(cs, at).map_err(|_| syn("JPX: main header"))?;
        if m == 0xFF90 {
            break;
        }
        have_cod |= m == 0xFF52;
        have_qcd |= m == 0xFF5C;
        marker(m, body, &mut main, nc)?;
        at = next;
    }
    if !have_cod || !have_qcd {
        return Err(syn("JPX: no COD / QCD"));
    }
    let ntiles = siz.ntx * siz.nty;
    let mut tiles: Vec<Option<(Coding, Vec<u8>)>> = (0..ntiles).map(|_| None).collect();
    // The tile-parts (A.4.2): SOT, the tile-part header, SOD, the data up to Psot (0: to the end).
    while at + 12 <= cs.len() && be16(cs, at)? == 0xFF90 {
        let lsot = be16(cs, at + 2)?;
        let isot = be16(cs, at + 4)?;
        let psot = be32(cs, at + 6)?;
        if isot >= ntiles {
            return Err(syn("JPX: tile index"));
        }
        let end = if psot == 0 {
            if cs.ends_with(&[0xFF, 0xD9]) { cs.len() - 2 } else { cs.len() }
        } else {
            at.saturating_add(psot).min(cs.len())
        };
        let slot = tiles[isot].get_or_insert_with(|| (main.fresh(), Vec::new()));
        let mut p = at + 2 + lsot;
        let mut data_at = None;
        while p + 2 <= end {
            if be16(cs, p)? == 0xFF93 {
                data_at = Some(p + 2);
                break;
            }
            let (m, body, next) = segment(cs, p)?;
            marker(m, body, &mut slot.0, nc)?;
            p = next;
        }
        if let Some(d) = data_at.filter(|&d| d <= end) {
            slot.1.extend_from_slice(&cs[d..end]);
        }
        if end <= at {
            break;
        }
        at = end;
    }
    let mut planes: Vec<Plane> = siz
        .comps
        .iter()
        .map(|c| {
            let (x0, y0) = (siz.x0.div_ceil(c.dx), siz.y0.div_ceil(c.dy));
            let (w, h) = (siz.x1.div_ceil(c.dx) - x0, siz.y1.div_ceil(c.dy) - y0);
            Plane { x0, y0, w, h, data: vec![0.0; w * h] }
        })
        .collect();
    for (t, slot) in tiles.iter().enumerate() {
        if let Some((co, body)) = slot {
            decode_tile(&siz, t, co, body, &mut planes)?;
        }
    }
    // DC level shift (G.1.2; a signed component is shifted into the same unsigned range), clamp, to 8 bits, and the
    // subsampled components replicated onto the image grid.
    let (w, h) = (siz.x1 - siz.x0, siz.y1 - siz.y0);
    let mut data = vec![0u8; w * h * nc];
    for (c, (comp, pl)) in siz.comps.iter().zip(&planes).enumerate() {
        if pl.w == 0 || pl.h == 0 {
            continue;
        }
        let maxv = ((1u32 << comp.depth) - 1) as f32;
        let shift = (1u32 << (comp.depth - 1)) as f32;
        let xs: Vec<usize> = (0..w).map(|x| ((siz.x0 + x) / comp.dx).clamp(pl.x0, pl.x0 + pl.w - 1) - pl.x0).collect();
        for y in 0..h {
            let py = ((siz.y0 + y) / comp.dy).clamp(pl.y0, pl.y0 + pl.h - 1) - pl.y0;
            let row = &pl.data[py * pl.w..(py + 1) * pl.w];
            for (x, &px) in xs.iter().enumerate() {
                let s = ((row[px] + shift).clamp(0.0, maxv) + 0.5) as u32;
                let v = match comp.depth {
                    8 => s,
                    d if d < 8 => (s * 255 + (maxv as u32) / 2) / maxv as u32,
                    d => s >> (d - 8),
                };
                data[(y * w + x) * nc + c] = v.min(255) as u8;
            }
        }
    }
    Ok(Image { w, h, comps: nc, space: Space::Unknown, data })
}

/// sYCC (colr enumeration 18) to sRGB on the 8-bit samples: the ICT equations about a 128 chroma midpoint.
fn sycc_to_rgb(img: &mut Image) {
    let n = img.comps;
    for px in img.data.chunks_exact_mut(n) {
        let (y, cb, cr) = (px[0] as f32, px[1] as f32 - 128.0, px[2] as f32 - 128.0);
        let rgb = [y + 1.402 * cr, y - 0.344_136 * cb - 0.714_136 * cr, y + 1.772 * cb];
        for (o, v) in px.iter_mut().zip(rgb) {
            *o = (v.clamp(0.0, 255.0) + 0.5) as u8;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tiles, resolutions, bands, precincts, code-blocks (Annex B)

struct Block {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    seen: bool,
    lblock: u32,
    zbp: u32,
    passes: u32,
    /// The codeword segments' bytes, by segment number (`seg_id`).
    segs: Vec<Vec<u8>>,
}

/// One band's share of one precinct: its code-blocks (raster order, `cw` wide) and their two tag trees.
struct PBand {
    cw: usize,
    blocks: Vec<Block>,
    incl: TagTree,
    zbp: TagTree,
}

struct Band {
    /// 0 LL, 1 HL, 2 LH, 3 HH.
    kind: u8,
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    mb: u32,
    delta: f32,
    coef: Vec<f32>,
}

struct Res {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    ppx: usize,
    ppy: usize,
    pw: usize,
    ph: usize,
    bands: Vec<Band>,
    /// [precinct][band].
    precs: Vec<Vec<PBand>>,
}

struct TComp {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    dx: usize,
    dy: usize,
    cod: Cod,
    res: Vec<Res>,
}

/// ceil((x - 2^(nb-1) * o) / 2^nb), the band origin of B-15.
fn band_coord(x: usize, nb: usize, o: usize) -> usize {
    let off = if o == 1 && nb > 0 { 1i64 << (nb - 1) } else { 0 };
    let a = x as i64 - off;
    let b = 1i64 << nb;
    (-((-a).div_euclid(b))).max(0) as usize
}

fn tcomp(cod: &Cod, qcd: &Qcd, comp: &Comp, rect: (usize, usize, usize, usize), count: &mut usize) -> Result<TComp, Error> {
    let (tx0, ty0, tx1, ty1) = rect;
    let (x0, y0, x1, y1) = (tx0.div_ceil(comp.dx), ty0.div_ceil(comp.dy), tx1.div_ceil(comp.dx), ty1.div_ceil(comp.dy));
    let nl = cod.levels;
    let mut res = Vec::with_capacity(nl + 1);
    for r in 0..=nl {
        let s = nl - r;
        let (rx0, ry0, rx1, ry1) = (cpow(x0, s), cpow(y0, s), cpow(x1, s), cpow(y1, s));
        let (ppx, ppy) = (cod.pp[r].0 as usize, cod.pp[r].1 as usize);
        let pw = if rx1 > rx0 { cpow(rx1, ppx) - (rx0 >> ppx) } else { 0 };
        let ph = if ry1 > ry0 { cpow(ry1, ppy) - (ry0 >> ppy) } else { 0 };
        *count = count.saturating_add(pw.saturating_mul(ph));
        if *count > MAX_BLOCKS {
            return Err(Error::TooLarge);
        }
        let kinds: &[u8] = if r == 0 { &[0] } else { &[1, 2, 3] };
        let nb = if r == 0 { nl } else { nl - r + 1 };
        let mut bands = Vec::with_capacity(kinds.len());
        for (bi, &kind) in kinds.iter().enumerate() {
            let (xo, yo) = match kind {
                1 => (1, 0),
                2 => (0, 1),
                3 => (1, 1),
                _ => (0, 0),
            };
            let (bx0, by0) = (band_coord(x0, nb, xo), band_coord(y0, nb, yo));
            let (bx1, by1) = (band_coord(x1, nb, xo).max(bx0), band_coord(y1, nb, yo).max(by0));
            // Quantisation (E.1): the subband's exponent and mantissa; the derived style scales the LL's (E-5).
            let sb = if r == 0 { 0 } else { 1 + 3 * (r - 1) + bi };
            let (eps, mu) = match qcd.style {
                1 => {
                    let (e0, m0) = *qcd.steps.first().ok_or(syn("JPX: short QCD"))?;
                    let e = e0 as i64 - nl as i64 + nb as i64;
                    if e < 0 {
                        return Err(syn("JPX: derived exponent"));
                    }
                    (e as u32, m0)
                }
                _ => *qcd.steps.get(sb).ok_or(syn("JPX: short QCD"))?,
            };
            if qcd.guard + eps == 0 {
                return Err(syn("JPX: no bit-planes"));
            }
            let mb = qcd.guard + eps - 1;
            if mb > 30 {
                return Err(Error::Unsupported("JPX bit depth"));
            }
            let gain = [0, 1, 1, 2][kind as usize];
            let delta = if cod.reversible {
                1.0
            } else {
                pow2(comp.depth as i32 + gain - eps as i32) * (1.0 + mu as f32 / 2048.0)
            };
            let (w, h) = (bx1 - bx0, by1 - by0);
            bands.push(Band { kind, x0: bx0, y0: by0, w, h, mb, delta, coef: vec![0.0; w * h] });
        }
        // Precincts in band coordinates (B.6): 2^PPx (r = 0) or 2^(PPx - 1); code-blocks on their own grid (B.7).
        let (ppxb, ppyb) = if r == 0 { (ppx, ppy) } else { (ppx - 1, ppy - 1) };
        let (xcb, ycb) = (cod.xcb.min(ppxb), cod.ycb.min(ppyb));
        let (pi0, pj0) = (rx0 >> ppx, ry0 >> ppy);
        let mut precs = Vec::with_capacity(pw * ph);
        for k in 0..pw * ph {
            let (i, j) = (k % pw, k / pw);
            let (cx0, cy0) = ((pi0 + i) << ppxb, (pj0 + j) << ppyb);
            let (cx1, cy1) = (cx0 + (1 << ppxb), cy0 + (1 << ppyb));
            let mut pbands = Vec::with_capacity(bands.len());
            for b in &bands {
                let (px0, py0) = (cx0.max(b.x0), cy0.max(b.y0));
                let (px1, py1) = (cx1.min(b.x0 + b.w), cy1.min(b.y0 + b.h));
                let (mut cw, mut ch, mut blocks) = (0, 0, Vec::new());
                if px0 < px1 && py0 < py1 {
                    let (gx0, gy0) = (px0 >> xcb, py0 >> ycb);
                    cw = cpow(px1, xcb) - gx0;
                    ch = cpow(py1, ycb) - gy0;
                    *count = count.saturating_add(cw * ch);
                    if *count > MAX_BLOCKS {
                        return Err(Error::TooLarge);
                    }
                    for gy in gy0..gy0 + ch {
                        for gx in gx0..gx0 + cw {
                            blocks.push(Block {
                                x0: (gx << xcb).max(px0),
                                y0: (gy << ycb).max(py0),
                                x1: ((gx + 1) << xcb).min(px1),
                                y1: ((gy + 1) << ycb).min(py1),
                                seen: false,
                                lblock: 3,
                                zbp: 0,
                                passes: 0,
                                segs: Vec::new(),
                            });
                        }
                    }
                }
                pbands.push(PBand { cw, blocks, incl: TagTree::new(cw, ch), zbp: TagTree::new(cw, ch) });
            }
            precs.push(pbands);
        }
        res.push(Res { x0: rx0, y0: ry0, x1: rx1, y1: ry1, ppx, ppy, pw, ph, bands, precs });
    }
    Ok(TComp { x0, y0, x1, y1, dx: comp.dx, dy: comp.dy, cod: cod.clone(), res })
}

fn decode_tile(siz: &Siz, t: usize, co: &Coding, body: &[u8], planes: &mut [Plane]) -> Result<(), Error> {
    let (p, q) = (t % siz.ntx, t / siz.ntx);
    let tx0 = (siz.tx0 + p * siz.tw).max(siz.x0);
    let ty0 = (siz.ty0 + q * siz.th).max(siz.y0);
    let tx1 = (siz.tx0 + (p + 1) * siz.tw).min(siz.x1);
    let ty1 = (siz.ty0 + (q + 1) * siz.th).min(siz.y1);
    let mut count = 0;
    let mut comps = Vec::with_capacity(siz.comps.len());
    for (c, comp) in siz.comps.iter().enumerate() {
        comps.push(tcomp(&co.cod[c], &co.qcd[c], comp, (tx0, ty0, tx1, ty1), &mut count)?);
    }
    // Tier-2: the packets in progression order; a packet that runs past the data ends the tile.
    let order = packets(co, &comps, tx0, ty0)?;
    let mut at = 0;
    for &(l, r, c, k) in &order {
        let style = comps[c].cod.style;
        match read_packet(body, at, co, l, &mut comps[c].res[r], k, style) {
            Some(next) => at = next,
            None => break,
        }
    }
    // Tier-1, dequantisation and the inverse wavelet per component.
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(comps.len());
    for tc in comps.iter_mut() {
        let (style, rev) = (tc.cod.style, tc.cod.reversible);
        for res in tc.res.iter_mut() {
            let Res { bands, precs, .. } = res;
            for pb in precs.iter() {
                for (bi, b) in pb.iter().enumerate() {
                    let band = &mut bands[bi];
                    for blk in &b.blocks {
                        t1_block(blk, band, style, rev);
                    }
                }
            }
        }
        out.push(idwt(tc));
    }
    // The component transform (Annex G) over the first three components.
    if co.mct && comps.len() >= 3 {
        let same = |a: &TComp, b: &TComp| (a.x0, a.y0, a.x1, a.y1) == (b.x0, b.y0, b.x1, b.y1);
        let rev = comps[0].cod.reversible;
        if !same(&comps[0], &comps[1])
            || !same(&comps[0], &comps[2])
            || comps[1].cod.reversible != rev
            || comps[2].cod.reversible != rev
        {
            return Err(Error::Unsupported("JPX component transform"));
        }
        let (a, rest) = out.split_at_mut(1);
        let (b, c) = rest.split_at_mut(1);
        for ((y0, y1), y2) in a[0].iter_mut().zip(b[0].iter_mut()).zip(c[0].iter_mut()) {
            let (v0, v1, v2) = (*y0, *y1, *y2);
            if rev {
                let g = v0 - ((v1 + v2) as i32 >> 2) as f32;
                (*y0, *y1, *y2) = (v2 + g, g, v1 + g);
            } else {
                (*y0, *y1, *y2) = (v0 + 1.402 * v2, v0 - 0.344_13 * v1 - 0.714_14 * v2, v0 + 1.772 * v1);
            }
        }
    }
    // Composition into the component planes.
    for (tc, (data, pl)) in comps.iter().zip(out.iter().zip(planes.iter_mut())) {
        let tw = tc.x1 - tc.x0;
        if tc.x0 < pl.x0 || tc.y0 < pl.y0 {
            continue;
        }
        for y in 0..tc.y1 - tc.y0 {
            let py = tc.y0 + y - pl.y0;
            let px = tc.x0 - pl.x0;
            if py >= pl.h || px + tw > pl.w {
                continue;
            }
            pl.data[py * pl.w + px..py * pl.w + px + tw].copy_from_slice(&data[y * tw..(y + 1) * tw]);
        }
    }
    Ok(())
}

/// The packet sequence (B.12.1): (layer, resolution, component, precinct).
fn packets(co: &Coding, comps: &[TComp], tx0: usize, ty0: usize) -> Result<Vec<(usize, usize, usize, usize)>, Error> {
    let nprec: usize = comps.iter().flat_map(|t| t.res.iter()).map(|r| r.pw * r.ph).sum();
    if (nprec as u64) * (co.layers as u64) > MAX_PACKETS as u64 {
        return Err(Error::TooLarge);
    }
    let maxr = comps.iter().map(|t| t.res.len()).max().unwrap_or(0);
    let mut out = Vec::with_capacity(nprec * co.layers);
    match co.prog {
        0 => {
            for l in 0..co.layers {
                for r in 0..maxr {
                    for (c, tc) in comps.iter().enumerate() {
                        if let Some(res) = tc.res.get(r) {
                            out.extend((0..res.pw * res.ph).map(|k| (l, r, c, k)));
                        }
                    }
                }
            }
        }
        1 => {
            for r in 0..maxr {
                for l in 0..co.layers {
                    for (c, tc) in comps.iter().enumerate() {
                        if let Some(res) = tc.res.get(r) {
                            out.extend((0..res.pw * res.ph).map(|k| (l, r, c, k)));
                        }
                    }
                }
            }
        }
        _ => {
            // RPCL (B.12.1.3): each precinct sits at the reference-grid point where the position loop first meets
            // it (the tile origin for a first precinct the grid does not align); sorting on (r, y, x, c) is that loop.
            let mut keys: Vec<(usize, u64, u64, usize, usize)> = Vec::with_capacity(nprec);
            for (c, tc) in comps.iter().enumerate() {
                let nl = tc.res.len() - 1;
                for (r, res) in tc.res.iter().enumerate() {
                    let s = (nl - r) as u32;
                    for k in 0..res.pw * res.ph {
                        let (i, j) = (k % res.pw, k / res.pw);
                        let pos = |i: usize, r0: usize, pp: usize, d: usize, t0: usize| -> u64 {
                            if i == 0 && r0 & ((1 << pp) - 1) != 0 {
                                t0 as u64
                            } else {
                                (d as u64) * ((((r0 >> pp) + i) as u64) << pp << s)
                            }
                        };
                        let x = pos(i, res.x0, res.ppx, tc.dx, tx0);
                        let y = pos(j, res.y0, res.ppy, tc.dy, ty0);
                        keys.push((r, y, x, c, k));
                    }
                }
            }
            keys.sort_unstable();
            for (r, _, _, c, k) in keys {
                out.extend((0..co.layers).map(|l| (l, r, c, k)));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------
// Tier-2: tag trees and packet headers (B.10)

/// A tag tree (B.10.2) over a `w` x `h` leaf grid.
struct TagTree {
    widths: Vec<usize>,
    offs: Vec<usize>,
    value: Vec<u32>,
    low: Vec<u32>,
}

impl TagTree {
    fn new(w: usize, h: usize) -> TagTree {
        let (mut widths, mut offs, mut total) = (Vec::new(), Vec::new(), 0);
        if w > 0 && h > 0 {
            let (mut a, mut b) = (w, h);
            loop {
                widths.push(a);
                offs.push(total);
                total += a * b;
                if a == 1 && b == 1 {
                    break;
                }
                a = a.div_ceil(2);
                b = b.div_ceil(2);
            }
        }
        TagTree { widths, offs, value: vec![u32::MAX; total], low: vec![0; total] }
    }

    /// Whether leaf (x, y)'s value is below `threshold`, reading the bits that decide it.
    fn decode(&mut self, bits: &mut HeaderBits, x: usize, y: usize, threshold: u32) -> bool {
        let mut path = [0usize; 48];
        let n = self.widths.len().min(48);
        if n == 0 {
            return false;
        }
        let (mut x, mut y) = (x, y);
        for (lv, slot) in path.iter_mut().enumerate().take(n) {
            *slot = self.offs[lv] + y * self.widths[lv] + x;
            x /= 2;
            y /= 2;
        }
        let mut low = 0;
        for &node in path[..n].iter().rev() {
            if low > self.low[node] {
                self.low[node] = low;
            } else {
                low = self.low[node];
            }
            while low < threshold && low < self.value[node] {
                if bits.bit() == 1 {
                    self.value[node] = low;
                } else {
                    low += 1;
                }
            }
            self.low[node] = low;
        }
        self.value[path[0]] < threshold
    }
}

/// The packet-header bit reader (B.10.1): MSB first; a byte after a 0xFF carries 7 bits.
struct HeaderBits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u8,
    ct: u32,
    over: bool,
}

impl<'a> HeaderBits<'a> {
    fn new(d: &'a [u8], pos: usize) -> Self {
        HeaderBits { d, pos, buf: 0, ct: 0, over: false }
    }

    fn bit(&mut self) -> u32 {
        if self.ct == 0 {
            let stuffed = self.buf == 0xFF;
            match self.d.get(self.pos) {
                Some(&b) => {
                    self.buf = b;
                    self.pos += 1;
                }
                None => {
                    self.buf = 0;
                    self.over = true;
                }
            }
            self.ct = if stuffed { 7 } else { 8 };
        }
        self.ct -= 1;
        ((self.buf >> self.ct) & 1) as u32
    }

    fn bits(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |v, _| v << 1 | self.bit())
    }

    /// The offset after the header: a last byte of 0xFF is followed by its stuffed byte.
    fn end(&self) -> usize {
        if self.buf == 0xFF { self.pos + 1 } else { self.pos }
    }
}

/// The number of coding passes (Table B.4).
fn num_passes(b: &mut HeaderBits) -> u32 {
    if b.bit() == 0 {
        return 1;
    }
    if b.bit() == 0 {
        return 2;
    }
    let v = b.bits(2);
    if v < 3 {
        return 3 + v;
    }
    let v = b.bits(5);
    if v < 31 {
        return 6 + v;
    }
    37 + b.bits(7)
}

/// The codeword segment pass `k` belongs to (D.4.1, Table D.9): one segment; each pass its own (TERMALL); or, under
/// BYPASS, passes 0-9 then per bit-plane a raw SP + MR segment and an MQ CU segment.
fn seg_id(style: u8, k: u32) -> usize {
    let k = k as usize;
    if style & 0x04 != 0 {
        k
    } else if style & 0x01 != 0 {
        if k < 10 { 0 } else { 1 + 2 * ((k - 10) / 3) + usize::from((k - 10) % 3 == 2) }
    } else {
        0
    }
}

/// Whether pass `k` is coded raw (BYPASS: the SP and MR passes after the first ten).
fn raw_pass(style: u8, k: u32) -> bool {
    style & 0x01 != 0 && k >= 10 && (k - 1) % 3 != 2
}

/// Read one packet at `at` (precinct `k` of `res`, layer `layer`): the offset after it, or None when the data ran out.
fn read_packet(d: &[u8], mut at: usize, co: &Coding, layer: usize, res: &mut Res, k: usize, style: u8) -> Option<usize> {
    if at >= d.len() {
        return None;
    }
    if co.sop && d.get(at..at + 2) == Some(&[0xFF, 0x91]) {
        at += 6;
    }
    let mut bits = HeaderBits::new(d, at);
    // (band, block, segment, length) of every contribution, in header order.
    let mut parts: Vec<(usize, usize, usize, usize)> = Vec::new();
    if bits.bit() == 1 {
        for (bi, pb) in res.precs.get_mut(k)?.iter_mut().enumerate() {
            let PBand { cw, blocks, incl, zbp } = pb;
            for (bk, blk) in blocks.iter_mut().enumerate() {
                let (bx, by) = (bk % *cw, bk / *cw);
                let included = if blk.seen { bits.bit() == 1 } else { incl.decode(&mut bits, bx, by, layer as u32 + 1) };
                if bits.over {
                    return None;
                }
                if !included {
                    continue;
                }
                if !blk.seen {
                    let mut i = 1;
                    while !zbp.decode(&mut bits, bx, by, i) {
                        i += 1;
                        if i > 64 || bits.over {
                            return None;
                        }
                    }
                    blk.zbp = i - 1;
                    blk.seen = true;
                }
                let np = num_passes(&mut bits);
                while bits.bit() == 1 {
                    blk.lblock += 1;
                    if blk.lblock > 32 || bits.over {
                        return None;
                    }
                }
                let end = blk.passes + np;
                if end > 3 * 31 {
                    return None;
                }
                let mut kp = blk.passes;
                while kp < end {
                    let seg = seg_id(style, kp);
                    let mut n = 0u32;
                    while kp < end && seg_id(style, kp) == seg {
                        kp += 1;
                        n += 1;
                    }
                    let nbits = blk.lblock + (31 - n.leading_zeros());
                    if nbits > 32 {
                        return None;
                    }
                    parts.push((bi, bk, seg, bits.bits(nbits) as usize));
                }
                blk.passes = end;
            }
        }
    }
    if bits.over {
        return None;
    }
    at = bits.end();
    if co.eph && d.get(at..at + 2) == Some(&[0xFF, 0x92]) {
        at += 2;
    }
    for (bi, bk, seg, len) in parts {
        let blk = &mut res.precs[k][bi].blocks[bk];
        while blk.segs.len() <= seg {
            blk.segs.push(Vec::new());
        }
        let have = d.len().saturating_sub(at).min(len);
        blk.segs[seg].extend_from_slice(&d[at..at + have]);
        if have < len {
            return None;
        }
        at += len;
    }
    Some(at)
}

// ---------------------------------------------------------------------------------------------------------------
// Tier-1: EBCOT (Annex D) over the ONE MQ decoder

/// The raw (bypass) bit reader (D.6): MSB first; a byte after a 0xFF carries 7 bits; past the end, 1 bits.
struct RawBits<'a> {
    d: &'a [u8],
    pos: usize,
    c: u8,
    ct: u32,
}

impl RawBits<'_> {
    fn bit(&mut self) -> u32 {
        if self.ct == 0 {
            let after_ff = self.c == 0xFF;
            match self.d.get(self.pos) {
                Some(&b) if !(after_ff && b > 0x8F) => {
                    self.c = b;
                    self.pos += 1;
                    self.ct = if after_ff { 7 } else { 8 };
                }
                _ => {
                    self.c = 0xFF;
                    self.ct = 8;
                }
            }
        }
        self.ct -= 1;
        ((self.c >> self.ct) & 1) as u32
    }
}

enum Coder<'a> {
    Mq(MqDecoder<'a>),
    Raw(RawBits<'a>),
}

impl Coder<'_> {
    fn decode(&mut self, cx: &mut u8) -> u32 {
        match self {
            Coder::Mq(m) => m.decode(cx) as u32,
            Coder::Raw(r) => r.bit(),
        }
    }
}

const SIG: u8 = 1;
const NEG: u8 = 2;
const VISIT: u8 = 4;
const REFINED: u8 = 8;

/// One code-block's tier-1 state, padded by one sample on every side.
struct T1 {
    w: usize,
    h: usize,
    s: usize,
    flags: Vec<u8>,
    mag: Vec<u32>,
    cx: [u8; 19],
}

impl T1 {
    fn new(w: usize, h: usize) -> T1 {
        let n = (w + 2) * (h + 2);
        let mut t = T1 { w, h, s: w + 2, flags: vec![0; n], mag: vec![0; n], cx: [0; 19] };
        t.reset();
        t
    }

    /// The initial states (D.7, Table D.7): UNIFORM at 46, run-length at 3, the all-zero ZC context at 4.
    fn reset(&mut self) {
        self.cx = [0; 19];
        self.cx[0] = 4 << 1;
        self.cx[17] = 3 << 1;
        self.cx[18] = 46 << 1;
    }

    fn at(&self, x: usize, y: usize) -> usize {
        (y + 1) * self.s + x + 1
    }

    fn sig(&self, i: usize) -> usize {
        (self.flags[i] & SIG) as usize
    }

    /// (horizontal, vertical, diagonal) significant neighbours.
    fn hvd(&self, i: usize) -> (usize, usize, usize) {
        let s = self.s;
        let h = self.sig(i - 1) + self.sig(i + 1);
        let v = self.sig(i - s) + self.sig(i + s);
        let d = self.sig(i - s - 1) + self.sig(i - s + 1) + self.sig(i + s - 1) + self.sig(i + s + 1);
        (h, v, d)
    }

    /// The zero-coding context (Table D.1) for band `kind`.
    fn zc(&self, i: usize, kind: u8) -> usize {
        let (h, v, d) = self.hvd(i);
        if kind == 3 {
            let hv = h + v;
            return match d {
                0 => hv.min(2),
                1 => 3 + hv.min(2),
                2 => if hv >= 1 { 7 } else { 6 },
                _ => 8,
            };
        }
        let (h, v) = if kind == 1 { (v, h) } else { (h, v) };
        match h {
            2 => 8,
            1 => if v >= 1 { 7 } else if d >= 1 { 6 } else { 5 },
            _ => match v {
                2 => 4,
                1 => 3,
                _ => d.min(2),
            },
        }
    }

    fn contrib(&self, i: usize) -> i32 {
        match self.flags[i] & (SIG | NEG) {
            0 | NEG => 0,
            SIG => 1,
            _ => -1,
        }
    }

    /// Decode the sign of sample `i` (Tables D.2 / D.3; raw under bypass) and make it significant at `plane`.
    fn sign(&mut self, c: &mut Coder, i: usize, plane: u32) {
        let neg = match c {
            Coder::Raw(r) => r.bit(),
            Coder::Mq(m) => {
                let h = (self.contrib(i - 1) + self.contrib(i + 1)).clamp(-1, 1);
                let v = (self.contrib(i - self.s) + self.contrib(i + self.s)).clamp(-1, 1);
                let (ctx, xor) = match (h, v) {
                    (1, 1) => (13, 0),
                    (1, 0) => (12, 0),
                    (1, _) => (11, 0),
                    (0, 1) => (10, 0),
                    (0, 0) => (9, 0),
                    (0, _) => (10, 1),
                    (_, 1) => (11, 1),
                    (_, 0) => (12, 1),
                    _ => (13, 1),
                };
                m.decode(&mut self.cx[ctx]) as u32 ^ xor
            }
        };
        self.mag[i] |= 1 << plane;
        self.flags[i] |= SIG | if neg == 1 { NEG } else { 0 };
    }

    /// Significance propagation (D.3.1).
    fn sig_pass(&mut self, c: &mut Coder, plane: u32, kind: u8) {
        for y0 in (0..self.h).step_by(4) {
            for x in 0..self.w {
                for y in y0..(y0 + 4).min(self.h) {
                    let i = self.at(x, y);
                    if self.flags[i] & SIG != 0 {
                        continue;
                    }
                    let ctx = self.zc(i, kind);
                    if ctx == 0 {
                        continue;
                    }
                    if c.decode(&mut self.cx[ctx]) == 1 {
                        self.sign(c, i, plane);
                    }
                    self.flags[i] |= VISIT;
                }
            }
        }
    }

    /// Magnitude refinement (D.3.3).
    fn ref_pass(&mut self, c: &mut Coder, plane: u32) {
        for y0 in (0..self.h).step_by(4) {
            for x in 0..self.w {
                for y in y0..(y0 + 4).min(self.h) {
                    let i = self.at(x, y);
                    if self.flags[i] & (SIG | VISIT) != SIG {
                        continue;
                    }
                    let ctx = if self.flags[i] & REFINED != 0 {
                        16
                    } else {
                        let (h, v, d) = self.hvd(i);
                        if h + v + d > 0 { 15 } else { 14 }
                    };
                    self.mag[i] |= c.decode(&mut self.cx[ctx]) << plane;
                    self.flags[i] |= REFINED;
                }
            }
        }
    }

    /// Cleanup (D.3.4), with the run-length mode on full four-sample stripe columns.
    fn cleanup(&mut self, c: &mut Coder, plane: u32, kind: u8) {
        for y0 in (0..self.h).step_by(4) {
            let yend = (y0 + 4).min(self.h);
            for x in 0..self.w {
                let mut y = y0;
                let run = yend - y0 == 4
                    && (y0..yend).all(|yy| {
                        let i = self.at(x, yy);
                        self.flags[i] & (SIG | VISIT) == 0 && self.zc(i, kind) == 0
                    });
                if run {
                    if c.decode(&mut self.cx[17]) == 0 {
                        continue;
                    }
                    let r = c.decode(&mut self.cx[18]) << 1 | c.decode(&mut self.cx[18]);
                    y = y0 + r as usize;
                    let i = self.at(x, y);
                    self.sign(c, i, plane);
                    y += 1;
                }
                while y < yend {
                    let i = self.at(x, y);
                    if self.flags[i] & (SIG | VISIT) == 0 {
                        let ctx = self.zc(i, kind);
                        if c.decode(&mut self.cx[ctx]) == 1 {
                            self.sign(c, i, plane);
                        }
                    }
                    y += 1;
                }
            }
        }
        self.flags.iter_mut().for_each(|f| *f &= !VISIT);
    }
}

/// Decode one code-block into its band's coefficients (dequantised: E.1.1 for 9/7, the integers for 5/3).
fn t1_block(blk: &Block, band: &mut Band, style: u8, reversible: bool) {
    if blk.passes == 0 || blk.zbp >= band.mb || blk.x1 <= blk.x0 || blk.y1 <= blk.y0 {
        return;
    }
    let planes = band.mb - blk.zbp;
    let npass = blk.passes.min(3 * planes - 2);
    let (w, h) = (blk.x1 - blk.x0, blk.y1 - blk.y0);
    let mut t = T1::new(w, h);
    let empty: &[u8] = &[];
    let mut coder = Coder::Raw(RawBits { d: empty, pos: 0, c: 0, ct: 0 });
    let mut seg = usize::MAX;
    let mut last = planes - 1;
    for k in 0..npass {
        let s = seg_id(style, k);
        if s != seg {
            seg = s;
            let data = blk.segs.get(s).map_or(empty, |v| v.as_slice());
            coder = if raw_pass(style, k) {
                Coder::Raw(RawBits { d: data, pos: 0, c: 0, ct: 0 })
            } else {
                Coder::Mq(MqDecoder::new(data))
            };
        }
        let (plane, pass) = if k == 0 { (planes - 1, 2) } else { (planes - 1 - (k + 2) / 3, (k - 1) % 3) };
        match pass {
            0 => t.sig_pass(&mut coder, plane, band.kind),
            1 => t.ref_pass(&mut coder, plane),
            _ => {
                t.cleanup(&mut coder, plane, band.kind);
                if style & 0x20 != 0 {
                    // The segmentation symbol (D.5): four UNIFORM decisions, read and dropped.
                    for _ in 0..4 {
                        coder.decode(&mut t.cx[18]);
                    }
                }
            }
        }
        if style & 0x02 != 0 {
            t.reset();
        }
        last = plane;
    }
    // Reconstruction at the midpoint of the planes not decoded (E.1.1.2, r = 1/2).
    let half = if last > 0 { 1u32 << (last - 1) } else { 0 };
    let (ox, oy) = (blk.x0 - band.x0, blk.y0 - band.y0);
    for y in 0..h {
        for x in 0..w {
            let i = t.at(x, y);
            let m = t.mag[i];
            if m == 0 {
                continue;
            }
            let q = (m | half) as f32;
            let v = if reversible { q } else { q * band.delta };
            let o = (oy + y) * band.w + ox + x;
            if let Some(slot) = band.coef.get_mut(o) {
                *slot = if t.flags[i] & NEG != 0 { -v } else { v };
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The inverse discrete wavelet transform (Annex F)

const ALPHA: f32 = -1.586_134_342_059_924;
const BETA: f32 = -0.052_980_118_572_961;
const GAMMA: f32 = 0.882_911_075_530_934;
const DELTA: f32 = 0.443_506_852_043_971;
const K: f32 = 1.230_174_104_914_001;

/// The periodic symmetric extension (F.3.7) of local index `j` into `0..n` (n >= 2).
fn mirror(j: isize, n: usize) -> usize {
    let period = 2 * (n as isize - 1);
    let mut j = j.rem_euclid(period);
    if j >= n as isize {
        j = period - j;
    }
    j as usize
}

/// 1D_SR (F.3.6): `x` holds the interleaved signal starting at global index `i0`.
fn sr1d(x: &mut [f32], i0: usize, reversible: bool) {
    let n = x.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        if i0 & 1 == 1 {
            x[0] /= 2.0;
        }
        return;
    }
    let even = i0 & 1;
    let odd = 1 - even;
    let nb = |x: &[f32], l: usize| x[mirror(l as isize - 1, n)] + x[mirror(l as isize + 1, n)];
    if reversible {
        // F-5: X(2n) = Y(2n) - floor((Y(2n-1) + Y(2n+1) + 2) / 4); X(2n+1) = Y(2n+1) + floor((X(2n) + X(2n+2)) / 2).
        for l in (even..n).step_by(2) {
            let s = nb(x, l);
            x[l] -= ((s + 2.0) as i32 >> 2) as f32;
        }
        for l in (odd..n).step_by(2) {
            let s = nb(x, l);
            x[l] += (s as i32 >> 1) as f32;
        }
    } else {
        // Table F.4: the scaling, then the four lifting steps undone in reverse order.
        for l in (even..n).step_by(2) {
            x[l] *= K;
        }
        for l in (odd..n).step_by(2) {
            x[l] /= K;
        }
        for (start, c) in [(even, DELTA), (odd, GAMMA), (even, BETA), (odd, ALPHA)] {
            for l in (start..n).step_by(2) {
                let s = nb(x, l);
                x[l] -= c * s;
            }
        }
    }
}

/// 2D_SR (F.3.2) over every resolution: interleave the four bands, then HOR_SR on the rows and VER_SR on the columns.
fn idwt(tc: &TComp) -> Vec<f32> {
    let rev = tc.cod.reversible;
    let mut a = tc.res[0].bands[0].coef.clone();
    let mut aw = tc.res[0].x1 - tc.res[0].x0;
    for r in 1..tc.res.len() {
        let (lo, rs) = (&tc.res[r - 1], &tc.res[r]);
        let (w, h) = (rs.x1 - rs.x0, rs.y1 - rs.y0);
        let mut out = vec![0.0f32; w * h];
        let get = |data: &[f32], bw: usize, bx0: usize, by0: usize, u: usize, v: usize| -> f32 {
            let (x, y) = ((u / 2).wrapping_sub(bx0), (v / 2).wrapping_sub(by0));
            if x < bw { data.get(y.wrapping_mul(bw).wrapping_add(x)).copied().unwrap_or(0.0) } else { 0.0 }
        };
        for v in 0..h {
            for u in 0..w {
                let (gu, gv) = (rs.x0 + u, rs.y0 + v);
                out[v * w + u] = match (gu & 1, gv & 1) {
                    (0, 0) => get(&a, aw, lo.x0, lo.y0, gu, gv),
                    (1, 0) => get(&rs.bands[0].coef, rs.bands[0].w, rs.bands[0].x0, rs.bands[0].y0, gu, gv),
                    (0, _) => get(&rs.bands[1].coef, rs.bands[1].w, rs.bands[1].x0, rs.bands[1].y0, gu, gv),
                    _ => get(&rs.bands[2].coef, rs.bands[2].w, rs.bands[2].x0, rs.bands[2].y0, gu, gv),
                };
            }
        }
        if w > 0 {
            for row in out.chunks_exact_mut(w) {
                sr1d(row, rs.x0, rev);
            }
        }
        let mut col = vec![0.0f32; h];
        for u in 0..w {
            for v in 0..h {
                col[v] = out[v * w + u];
            }
            sr1d(&mut col, rs.y0, rev);
            for v in 0..h {
                out[v * w + u] = col[v];
            }
        }
        a = out;
        aw = w;
    }
    a
}
