// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core (demux_core: containers in, compressed payloads out; it decodes nothing).
//!
//! HEVCHEIC (LEDGER SR220): the HEIF image-item reader (ISO/IEC 23008-12, the High Efficiency Image File Format) —
//! the container of every HEIC still. It reads the boxes through demux_core's one ISOBMFF reader ([`crate::mp4::boxes`]
//! and the bounds-checked [`crate::read::Reader`]) and is codec-blind: an item is its type, its properties and its
//! byte extents; what decodes the bytes (hevc_core for `hvc1`, av1_core for `av01`) is the caller's.
//!
//! Read: `ftyp` (brands, [`is_heic`]); `meta` (full box) → `hdlr` (must be `pict`), `pitm` (v0 / v1), `iinf` (v0 / v1)
//! → `infe` v2 / v3 (item type), `iloc` v0 / v1 / v2 (offset / length / base-offset / index sizes; construction method
//! 0 = file and 1 = `idat`; method 2 = item and an external data reference are refused by name when the item's bytes
//! are asked for), `iref` v0 / v1 (`dimg`, `thmb`, `cdsc`, ...), `idat`, `iprp` → `ipco` (1-based property list) and
//! `ipma` (v0 16-bit / v1 32-bit item ids; flags bit 0 → 15-bit indices); the properties `hvcC` (kept raw), `ispe`,
//! `irot`, `imir`, `clap`, `colr` (`nclx` read; `prof` / `rICC` noted); the `grid` derived item's `ImageGrid` body
//! ([`parse_grid`]) with its tiles in `dimg` order.
//!
//! HEIFONE (LEDGER SR303): the same reader serves AVIF (AV1 Image File Format, the HEIF profile of AOMedia's AVIF
//! v1.1): an `av01` item ([`ItemKind::Av01`]) with its `av1C` property kept raw and `pixi` (bits per channel); grids of
//! `av01` tiles read as grids of `hvc1` tiles do. [`is_avif`] tells an AVIF still from HEIC.
//!
//! [`build`] is the minimal writer the KATs (here, in gneiss_pal's `dsp::image` and in Facet) make HEIF files with.

use alloc::vec::Vec;

use crate::Error;
use crate::mp4::{BoxHeader, body, boxes, child, children, full};

/// What an item is (`infe` item_type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// An HEVC-coded image (`hvc1`): its bytes are one length-prefixed access unit, its `hvcC` the parameter sets.
    Hvc1,
    /// An AV1-coded image (`av01`, HEIFONE SR303): its bytes are a low-overhead AV1 OBU stream, its `av1C` the record.
    Av01,
    /// A derived image grid (`grid`): its bytes are an `ImageGrid` ([`parse_grid`]), its tiles the `dimg` references.
    Grid,
    /// Exif metadata (`Exif`).
    Exif,
    /// Any other item type (`jpeg`, `iovl`, `mime`, ...), named by its four-character code.
    Other([u8; 4]),
}

/// `clap` — the clean aperture (ISO/IEC 14496-12 12.1.4): rationals as (numerator, denominator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clap {
    pub width: (u32, u32),
    pub height: (u32, u32),
    pub horiz_off: (i32, u32),
    pub vert_off: (i32, u32),
}

impl Clap {
    /// The crop rectangle `(x, y, w, h)` inside a `w` x `h` image: width and height floored to whole pixels, the
    /// centre offset by `horiz_off` / `vert_off` from the image's centre, so `x = horiz_off + (w - cw) / 2` (floored).
    /// `None` when a denominator is zero, the aperture is empty, or it falls outside the image.
    pub fn rect(&self, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        if self.width.1 == 0 || self.height.1 == 0 || self.horiz_off.1 == 0 || self.vert_off.1 == 0 {
            return None;
        }
        let cw = self.width.0 / self.width.1;
        let ch = self.height.0 / self.height.1;
        if cw == 0 || ch == 0 || cw > w || ch > h {
            return None;
        }
        let edge = |off: (i32, u32), full: u32, c: u32| -> Option<u32> {
            let d = off.1 as i64;
            let v = (2 * off.0 as i64 + (full - c) as i64 * d).div_euclid(2 * d);
            (v >= 0 && v as u64 + c as u64 <= full as u64).then_some(v as u32)
        };
        Some((edge(self.horiz_off, w, cw)?, edge(self.vert_off, h, ch)?, cw, ch))
    }
}

/// `colr` of type `nclx` (ISO/IEC 23091-2 code points).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nclx {
    pub colour_primaries: u16,
    pub transfer_characteristics: u16,
    pub matrix_coefficients: u16,
    pub full_range: bool,
}

/// A transformative property, in the order `ipma` associates it (HEIF 6.5.1: applied in that order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    Clap(Clap),
    /// `irot`: angle x 90 degrees anticlockwise (0..=3).
    Irot(u8),
    /// `imir`: axis 0 = vertical (left-right flip), 1 = horizontal (top-bottom flip).
    Imir(u8),
}

/// One item and everything the file says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: u32,
    pub kind: ItemKind,
    /// The `hvcC` property body (an HEVCDecoderConfigurationRecord), raw.
    pub hvcc: Option<Vec<u8>>,
    /// The `av1C` property body (an AV1CodecConfigurationRecord), raw (HEIFONE SR303).
    pub av1c: Option<Vec<u8>>,
    /// `pixi`: bits per channel, one entry per channel.
    pub pixi: Option<Vec<u8>>,
    /// `ispe` (width, height).
    pub ispe: Option<(u32, u32)>,
    pub nclx: Option<Nclx>,
    /// A `colr` of type `prof` / `rICC` is associated (the profile itself is not read).
    pub icc: bool,
    /// `clap` / `irot` / `imir` in association order.
    pub transforms: Vec<Transform>,
    /// The item's bytes as absolute `(offset, length)` ranges in the file, or why they cannot be located.
    pub extents: Result<Vec<(usize, usize)>, Error>,
    /// `dimg` references from this item (a grid's tiles, row-major).
    pub dimg: Vec<u32>,
}

/// One `iref` reference: `from` refers to every id in `to` with the type `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub kind: [u8; 4],
    pub from: u32,
    pub to: Vec<u32>,
}

/// A parsed HEIF file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heif {
    /// `ftyp` major brand then the compatible brands.
    pub brands: Vec<[u8; 4]>,
    /// `pitm`: the primary item's id (present in `items`).
    pub primary: u32,
    pub items: Vec<Item>,
    pub refs: Vec<Reference>,
}

impl Heif {
    pub fn item(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The item's bytes: its extents concatenated.
    pub fn data(&self, bytes: &[u8], item: &Item) -> Result<Vec<u8>, Error> {
        let ext = item.extents.as_ref().map_err(|e| e.clone())?;
        let mut out = Vec::new();
        for &(o, n) in ext {
            out.extend_from_slice(bytes.get(o..o.checked_add(n).ok_or(Error::Truncated)?).ok_or(Error::Truncated)?);
        }
        Ok(out)
    }
}

/// `ImageGrid` (HEIF 6.6.2.3.2): the tiles are `rows` x `columns` images of one size laid row-major, the output is the
/// top-left `width` x `height` of that canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub rows: u32,
    pub columns: u32,
    pub width: u32,
    pub height: u32,
}

/// The `ftyp` brands (major, then compatible; the minor version skipped), or `None` when the file does not open with
/// an `ftyp` box.
pub fn brands(bytes: &[u8]) -> Option<Vec<[u8; 4]>> {
    if bytes.len() < 8 || &bytes[4..8] != b"ftyp" {
        return None;
    }
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let first = *boxes(bytes, 0, size.min(bytes.len())).ok()?.first()?;
    let b = &bytes[first.body..first.end];
    if b.len() < 8 {
        return None;
    }
    let mut out = Vec::new();
    out.push([b[0], b[1], b[2], b[3]]);
    for c in b[8..].chunks_exact(4) {
        out.push([c[0], c[1], c[2], c[3]]);
    }
    Some(out)
}

/// An AVIF still (HEIFONE SR303): brand `avif` among the major / compatible brands. An `avis` image sequence without
/// the `avif` brand is not a still and answers `false`.
pub fn is_avif(bytes: &[u8]) -> bool {
    brands(bytes).is_some_and(|b| b.iter().any(|x| x == b"avif"))
}

/// An HEIF still that may hold HEVC images: brand `heic` or `heix`, or `mif1` without `avif` / `avis` (an AVIF file
/// also carries `mif1`). Image sequences (`hevc`, `heis`, `msf1` alone) are not stills and answer `false`.
pub fn is_heic(bytes: &[u8]) -> bool {
    let Some(b) = brands(bytes) else { return false };
    let has = |s: &[u8; 4]| b.iter().any(|x| x == s);
    has(b"heic") || has(b"heix") || (has(b"mif1") && !has(b"avif") && !has(b"avis"))
}

fn id(r: &mut crate::read::Reader, wide: bool) -> Result<u32, Error> {
    if wide { r.u32() } else { r.u16().map(u32::from) }
}

fn usize_of(v: u64) -> Result<usize, Error> {
    usize::try_from(v).map_err(|_| Error::Invalid("heif: offset does not fit"))
}

/// Parse the `meta` box of an HEIF file.
pub fn parse_heif(bytes: &[u8]) -> Result<Heif, Error> {
    let top = boxes(bytes, 0, bytes.len())?;
    if top.first().map(|b| &b.kind) != Some(b"ftyp") {
        return Err(Error::UnknownFormat);
    }
    let brands = brands(bytes).ok_or(Error::Invalid("heif: ftyp"))?;
    let meta = child(&top, b"meta").ok_or(Error::Invalid("heif: no meta box"))?;
    if meta.end - meta.body < 4 {
        return Err(Error::Truncated);
    }
    let mb = boxes(bytes, meta.body + 4, meta.end)?;

    let hdlr = child(&mb, b"hdlr").ok_or(Error::Invalid("heif: meta has no hdlr"))?;
    let mut r = body(bytes, hdlr);
    full(&mut r)?;
    r.skip(4)?;
    if &r.fourcc()? != b"pict" {
        return Err(Error::Unsupported("heif: meta handler is not pict"));
    }

    let pitm = child(&mb, b"pitm").ok_or(Error::Invalid("heif: no pitm (primary item)"))?;
    let mut r = body(bytes, pitm);
    let (v, _) = full(&mut r)?;
    let primary = id(&mut r, v != 0)?;

    // iinf → infe: the item list.
    let iinf = child(&mb, b"iinf").ok_or(Error::Invalid("heif: no iinf"))?;
    let mut r = body(bytes, iinf);
    let (v, _) = full(&mut r)?;
    id(&mut r, v != 0)?; // entry_count: the infe boxes that follow are the truth
    let mut items: Vec<Item> = Vec::new();
    for infe in boxes(bytes, iinf.body + r.pos(), iinf.end)?.iter().filter(|b| &b.kind == b"infe") {
        let mut r = body(bytes, infe);
        let (v, _) = full(&mut r)?;
        if v < 2 {
            continue; // infe v0 / v1 carry no item_type: not an image item
        }
        let item_id = id(&mut r, v >= 3)?;
        r.skip(2)?; // item_protection_index
        let kind = match &r.fourcc()? {
            b"hvc1" => ItemKind::Hvc1,
            b"av01" => ItemKind::Av01,
            b"grid" => ItemKind::Grid,
            b"Exif" => ItemKind::Exif,
            k => ItemKind::Other(*k),
        };
        items.push(Item {
            id: item_id,
            kind,
            hvcc: None,
            av1c: None,
            pixi: None,
            ispe: None,
            nclx: None,
            icc: false,
            transforms: Vec::new(),
            extents: Err(Error::Invalid("heif: item has no iloc entry")),
            dimg: Vec::new(),
        });
    }

    // idat: construction method 1's base.
    let idat = child(&mb, b"idat").map(|b| (b.body, b.end - b.body));

    // iloc: where each item's bytes are.
    if let Some(iloc) = child(&mb, b"iloc") {
        let mut r = body(bytes, iloc);
        let (v, _) = full(&mut r)?;
        if v > 2 {
            return Err(Error::Unsupported("heif: iloc version above 2"));
        }
        let a = r.u8()?;
        let b = r.u8()?;
        let (off_size, len_size, base_size) = ((a >> 4) as usize, (a & 15) as usize, (b >> 4) as usize);
        let index_size = if v >= 1 { (b & 15) as usize } else { 0 };
        let count = id(&mut r, v == 2)?;
        for _ in 0..count {
            let item_id = id(&mut r, v == 2)?;
            let method = if v >= 1 { r.u16()? & 15 } else { 0 };
            let data_ref = r.u16()?;
            let base = r.uint_n(base_size)?;
            let n = r.u16()?;
            let mut raw = Vec::new();
            for _ in 0..n {
                if index_size > 0 {
                    r.uint_n(index_size)?;
                }
                raw.push((r.uint_n(off_size)?, r.uint_n(len_size)?));
            }
            let extents = locate(bytes.len(), idat, method, data_ref, base, &raw);
            if let Some(it) = items.iter_mut().find(|i| i.id == item_id) {
                it.extents = extents;
            }
        }
    }

    // iref: dimg (grid tiles), thmb, cdsc, ...
    let mut refs = Vec::new();
    if let Some(iref) = child(&mb, b"iref") {
        let mut r = body(bytes, iref);
        let (v, _) = full(&mut r)?;
        for rb in boxes(bytes, iref.body + r.pos(), iref.end)? {
            let mut r = body(bytes, &rb);
            let from = id(&mut r, v != 0)?;
            let n = r.u16()?;
            let mut to = Vec::new();
            for _ in 0..n {
                to.push(id(&mut r, v != 0)?);
            }
            if &rb.kind == b"dimg" {
                if let Some(it) = items.iter_mut().find(|i| i.id == from) {
                    it.dimg.extend_from_slice(&to);
                }
            }
            refs.push(Reference { kind: rb.kind, from, to });
        }
    }

    // iprp: ipco properties, ipma associations.
    if let Some(iprp) = child(&mb, b"iprp") {
        let ib = children(bytes, iprp)?;
        let props: Vec<BoxHeader> = match child(&ib, b"ipco") {
            Some(ipco) => children(bytes, ipco)?,
            None => Vec::new(),
        };
        for ipma in ib.iter().filter(|b| &b.kind == b"ipma") {
            let mut r = body(bytes, ipma);
            let (v, flags) = full(&mut r)?;
            let n = r.u32()?;
            for _ in 0..n {
                let item_id = id(&mut r, v >= 1)?;
                let k = r.u8()?;
                for _ in 0..k {
                    let idx = if flags & 1 != 0 { (r.u16()? & 0x7fff) as usize } else { (r.u8()? & 0x7f) as usize };
                    if idx == 0 {
                        continue;
                    }
                    let p = props.get(idx - 1).ok_or(Error::Invalid("heif: ipma index past ipco"))?;
                    if let Some(it) = items.iter_mut().find(|i| i.id == item_id) {
                        associate(bytes, p, it)?;
                    }
                }
            }
        }
    }

    if !items.iter().any(|i| i.id == primary) {
        return Err(Error::Invalid("heif: primary item missing from iinf"));
    }
    Ok(Heif { brands, primary, items, refs })
}

/// Resolve an `iloc` entry to absolute ranges (`len == 0` = to the end of the file / of `idat`).
fn locate(
    file_len: usize,
    idat: Option<(usize, usize)>,
    method: u16,
    data_ref: u16,
    base: u64,
    raw: &[(u64, u64)],
) -> Result<Vec<(usize, usize)>, Error> {
    if data_ref != 0 {
        return Err(Error::Unsupported("heif: item data in another file (data_reference_index)"));
    }
    let (origin, limit) = match method {
        0 => (0usize, file_len),
        1 => idat.ok_or(Error::Invalid("heif: iloc construction_method 1 without idat"))?,
        _ => return Err(Error::Unsupported("heif: iloc construction_method 2 (item)")),
    };
    let mut out = Vec::new();
    for &(off, len) in raw {
        let start = usize_of(base.checked_add(off).ok_or(Error::Truncated)?)?;
        if start > limit {
            return Err(Error::Truncated);
        }
        let len = if len == 0 { limit - start } else { usize_of(len)? };
        if start.checked_add(len).ok_or(Error::Truncated)? > limit {
            return Err(Error::Truncated);
        }
        out.push((origin + start, len));
    }
    Ok(out)
}

fn associate(bytes: &[u8], p: &BoxHeader, it: &mut Item) -> Result<(), Error> {
    let mut r = body(bytes, p);
    match &p.kind {
        b"hvcC" => it.hvcc = Some(bytes[p.body..p.end].to_vec()),
        b"av1C" => it.av1c = Some(bytes[p.body..p.end].to_vec()),
        b"pixi" => {
            full(&mut r)?;
            let n = r.u8()? as usize;
            let mut bits = Vec::with_capacity(n);
            for _ in 0..n {
                bits.push(r.u8()?);
            }
            it.pixi = Some(bits);
        }
        b"ispe" => {
            full(&mut r)?;
            it.ispe = Some((r.u32()?, r.u32()?));
        }
        b"irot" => it.transforms.push(Transform::Irot(r.u8()? & 3)),
        b"imir" => it.transforms.push(Transform::Imir(r.u8()? & 1)),
        b"clap" => {
            let mut v = [0u32; 8];
            for x in v.iter_mut() {
                *x = r.u32()?;
            }
            it.transforms.push(Transform::Clap(Clap {
                width: (v[0], v[1]),
                height: (v[2], v[3]),
                horiz_off: (v[4] as i32, v[5]),
                vert_off: (v[6] as i32, v[7]),
            }));
        }
        b"colr" => match &r.fourcc()? {
            b"nclx" => {
                it.nclx = Some(Nclx {
                    colour_primaries: r.u16()?,
                    transfer_characteristics: r.u16()?,
                    matrix_coefficients: r.u16()?,
                    full_range: r.u8()? & 0x80 != 0,
                })
            }
            b"prof" | b"rICC" => it.icc = true,
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

/// Parse an `ImageGrid` item body.
pub fn parse_grid(data: &[u8]) -> Result<Grid, Error> {
    let mut r = crate::read::Reader::new(data);
    if r.u8()? != 0 {
        return Err(Error::Unsupported("heif: ImageGrid version above 0"));
    }
    let flags = r.u8()?;
    let rows = r.u8()? as u32 + 1;
    let columns = r.u8()? as u32 + 1;
    let (width, height) = if flags & 1 != 0 { (r.u32()?, r.u32()?) } else { (r.u16()? as u32, r.u16()? as u32) };
    Ok(Grid { rows, columns, width, height })
}

/// The minimal HEIF writer the KATs build files with: `ftyp` (heic, mif1), `meta` (`hdlr` pict, `pitm`, `iinf` with
/// `infe` v2, `iref` `dimg`, `iprp` with one `ipco` entry per item property and `ipma` v0, `idat`, `iloc` v1 with
/// 32-bit offsets and lengths), `mdat`. Items are numbered 1..=n in the order given. [`build::avif`] writes the same
/// file under the AVIF brands (`avif`, `mif1`, `miaf`).
pub mod build {
    use alloc::vec::Vec;

    /// A box.
    pub fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(body.len() + 8);
        v.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    /// A full box (version, 24-bit flags).
    pub fn full_bx(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut b = Vec::with_capacity(body.len() + 4);
        b.push(version);
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        b.extend_from_slice(body);
        bx(kind, &b)
    }

    /// An HEVCDecoderConfigurationRecord (ISO/IEC 14496-15 8.3.3.1): Main profile, level 2, 8-bit 4:2:0, 4-byte NAL
    /// lengths, one array per run of NAL units of one type (each unit without start code or length).
    pub fn hvcc_record(units: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[1, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 60, 0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 0x0F]);
        let ty = |u: &[u8]| u.first().map_or(0, |b| (b >> 1) & 0x3f);
        let mut runs: Vec<(u8, Vec<&[u8]>)> = Vec::new();
        for &u in units {
            match runs.last_mut() {
                Some((t, list)) if *t == ty(u) => list.push(u),
                _ => runs.push((ty(u), alloc::vec![u])),
            }
        }
        v.push(runs.len() as u8);
        for (t, list) in runs {
            v.push(0x80 | t);
            v.extend_from_slice(&(list.len() as u16).to_be_bytes());
            for u in list {
                v.extend_from_slice(&(u.len() as u16).to_be_bytes());
                v.extend_from_slice(u);
            }
        }
        v
    }

    pub fn hvcc(record: &[u8]) -> Vec<u8> {
        bx(b"hvcC", record)
    }
    /// `av1C` around an AV1CodecConfigurationRecord body.
    pub fn av1c(record: &[u8]) -> Vec<u8> {
        bx(b"av1C", record)
    }
    /// `pixi` with one bit depth per channel.
    pub fn pixi(bits: &[u8]) -> Vec<u8> {
        let mut b = alloc::vec![bits.len() as u8];
        b.extend_from_slice(bits);
        full_bx(b"pixi", 0, 0, &b)
    }
    pub fn ispe(w: u32, h: u32) -> Vec<u8> {
        let mut b = w.to_be_bytes().to_vec();
        b.extend_from_slice(&h.to_be_bytes());
        full_bx(b"ispe", 0, 0, &b)
    }
    pub fn irot(angle: u8) -> Vec<u8> {
        bx(b"irot", &[angle & 3])
    }
    pub fn imir(axis: u8) -> Vec<u8> {
        bx(b"imir", &[axis & 1])
    }
    /// `clap` from (num, den) pairs: width, height, horizontal offset, vertical offset.
    pub fn clap(w: (u32, u32), h: (u32, u32), ho: (i32, u32), vo: (i32, u32)) -> Vec<u8> {
        let mut b = Vec::new();
        for x in [w.0, w.1, h.0, h.1, ho.0 as u32, ho.1, vo.0 as u32, vo.1] {
            b.extend_from_slice(&x.to_be_bytes());
        }
        bx(b"clap", &b)
    }
    pub fn nclx(primaries: u16, transfer: u16, matrix: u16, full_range: bool) -> Vec<u8> {
        let mut b = b"nclx".to_vec();
        for x in [primaries, transfer, matrix] {
            b.extend_from_slice(&x.to_be_bytes());
        }
        b.push(if full_range { 0x80 } else { 0 });
        bx(b"colr", &b)
    }
    /// An `ImageGrid` body with 16-bit output dimensions (32-bit when either exceeds 65535).
    pub fn grid_data(rows: u8, columns: u8, width: u32, height: u32) -> Vec<u8> {
        let wide = width > 0xFFFF || height > 0xFFFF;
        let mut v = alloc::vec![0, wide as u8, rows - 1, columns - 1];
        if wide {
            v.extend_from_slice(&width.to_be_bytes());
            v.extend_from_slice(&height.to_be_bytes());
        } else {
            v.extend_from_slice(&(width as u16).to_be_bytes());
            v.extend_from_slice(&(height as u16).to_be_bytes());
        }
        v
    }

    /// One item to write.
    #[derive(Clone, Debug, Default)]
    pub struct ItemSpec {
        pub kind: [u8; 4],
        pub data: Vec<u8>,
        /// Whole property boxes, associated in this order.
        pub props: Vec<Vec<u8>>,
        /// `dimg` references to other items (1-based ids).
        pub dimg: Vec<u32>,
        /// iloc construction_method: 0 = the bytes go to `mdat`, 1 = to `idat`; any other value is written as-is
        /// (with the bytes in `mdat`) so a reader's refusal can be exercised.
        pub construction: u8,
    }

    /// A HEIF file of `items` (ids 1..=n) whose primary item is `primary`.
    pub fn still(primary: u32, items: &[ItemSpec]) -> Vec<u8> {
        still_with_handler(primary, items, b"pict")
    }

    /// [`still`] with another `hdlr` handler type (a reader must refuse anything but `pict`).
    pub fn still_with_handler(primary: u32, items: &[ItemSpec], handler: &[u8; 4]) -> Vec<u8> {
        file(b"heic\0\0\0\0mif1heic", primary, items, handler)
    }

    /// An AVIF still (HEIFONE SR303): [`still`] under the brands `avif` (major), `mif1`, `miaf`.
    pub fn avif(primary: u32, items: &[ItemSpec]) -> Vec<u8> {
        file(b"avif\0\0\0\0avifmif1miaf", primary, items, b"pict")
    }

    fn file(ftyp: &[u8], primary: u32, items: &[ItemSpec], handler: &[u8; 4]) -> Vec<u8> {
        let ftyp = bx(b"ftyp", ftyp);
        let meta0 = meta(primary, items, handler, 0);
        let mdat_body = ftyp.len() + meta0.len() + 8;
        let meta = meta(primary, items, handler, mdat_body);
        let mut out = ftyp;
        out.extend_from_slice(&meta);
        let mut mdat = Vec::new();
        for it in items.iter().filter(|i| i.construction != 1) {
            mdat.extend_from_slice(&it.data);
        }
        out.extend_from_slice(&bx(b"mdat", &mdat));
        out
    }

    fn meta(primary: u32, items: &[ItemSpec], handler: &[u8; 4], mdat_body: usize) -> Vec<u8> {
        let mut m = Vec::new();
        let mut h = alloc::vec![0u8; 4];
        h.extend_from_slice(handler);
        h.extend_from_slice(&[0u8; 13]);
        m.extend(full_bx(b"hdlr", 0, 0, &h));
        m.extend(full_bx(b"pitm", 0, 0, &(primary as u16).to_be_bytes()));
        let mut iinf = (items.len() as u16).to_be_bytes().to_vec();
        for (i, it) in items.iter().enumerate() {
            let mut e = ((i + 1) as u16).to_be_bytes().to_vec();
            e.extend_from_slice(&[0, 0]);
            e.extend_from_slice(&it.kind);
            e.push(0);
            iinf.extend(full_bx(b"infe", 2, 0, &e));
        }
        m.extend(full_bx(b"iinf", 0, 0, &iinf));
        let mut iref = Vec::new();
        for (i, it) in items.iter().enumerate().filter(|(_, it)| !it.dimg.is_empty()) {
            let mut d = ((i + 1) as u16).to_be_bytes().to_vec();
            d.extend_from_slice(&(it.dimg.len() as u16).to_be_bytes());
            for &t in &it.dimg {
                d.extend_from_slice(&(t as u16).to_be_bytes());
            }
            iref.extend(bx(b"dimg", &d));
        }
        if !iref.is_empty() {
            m.extend(full_bx(b"iref", 0, 0, &iref));
        }
        let mut ipco = Vec::new();
        let mut ipma = (items.len() as u32).to_be_bytes().to_vec();
        let mut next = 1u8;
        for (i, it) in items.iter().enumerate() {
            ipma.extend_from_slice(&((i + 1) as u16).to_be_bytes());
            ipma.push(it.props.len() as u8);
            for p in &it.props {
                ipco.extend_from_slice(p);
                ipma.push(next);
                next += 1;
            }
        }
        let mut iprp = bx(b"ipco", &ipco);
        iprp.extend(full_bx(b"ipma", 0, 0, &ipma));
        m.extend(bx(b"iprp", &iprp));
        let idat: Vec<u8> = items.iter().filter(|i| i.construction == 1).flat_map(|i| i.data.iter().copied()).collect();
        if !idat.is_empty() {
            m.extend(bx(b"idat", &idat));
        }
        let mut iloc = alloc::vec![0x44, 0x00];
        iloc.extend_from_slice(&(items.len() as u16).to_be_bytes());
        let (mut in_mdat, mut in_idat) = (0usize, 0usize);
        for (i, it) in items.iter().enumerate() {
            iloc.extend_from_slice(&((i + 1) as u16).to_be_bytes());
            iloc.extend_from_slice(&(it.construction as u16).to_be_bytes());
            iloc.extend_from_slice(&[0, 0, 0, 1]);
            let off = if it.construction == 1 {
                in_idat += it.data.len();
                in_idat - it.data.len()
            } else {
                in_mdat += it.data.len();
                mdat_body + in_mdat - it.data.len()
            };
            iloc.extend_from_slice(&(off as u32).to_be_bytes());
            iloc.extend_from_slice(&(it.data.len() as u32).to_be_bytes());
        }
        m.extend(full_bx(b"iloc", 1, 0, &iloc));
        full_bx(b"meta", 0, 0, &m)
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;
    use alloc::vec;

    fn hvc1(data: &[u8], props: Vec<Vec<u8>>) -> ItemSpec {
        ItemSpec { kind: *b"hvc1", data: data.to_vec(), props, ..Default::default() }
    }

    #[test]
    fn one_hvc1_item_reads_back() {
        let rec = hvcc_record(&[&[0x40, 0x01, 0xAA], &[0x42, 0x01, 0xBB], &[0x44, 0x01, 0xCC]]);
        let f = still(1, &[hvc1(b"\0\0\0\x02AU", vec![hvcc(&rec), ispe(64, 48), nclx(1, 13, 6, true)])]);
        assert!(is_heic(&f));
        let h = parse_heif(&f).unwrap();
        assert_eq!(h.primary, 1);
        assert_eq!(&h.brands[..], &[*b"heic", *b"mif1", *b"heic"]);
        let it = h.item(1).unwrap();
        assert_eq!(it.kind, ItemKind::Hvc1);
        assert_eq!(it.hvcc.as_deref(), Some(&rec[..]));
        assert_eq!(rec[22], 3, "three arrays: VPS, SPS, PPS");
        assert_eq!(it.ispe, Some((64, 48)));
        assert_eq!(it.nclx, Some(Nclx { colour_primaries: 1, transfer_characteristics: 13, matrix_coefficients: 6, full_range: true }));
        assert!(it.transforms.is_empty());
        assert_eq!(h.data(&f, it).unwrap(), b"\0\0\0\x02AU");
        // The extent lies inside mdat, after ftyp and meta.
        let (o, n) = it.extents.as_ref().unwrap()[0];
        assert_eq!((&f[o - 8 + 4..o], n), (&b"mdat"[..], 6));
    }

    #[test]
    fn a_2x2_grid_names_its_tiles_in_order() {
        let mut items = vec![ItemSpec {
            kind: *b"grid",
            data: grid_data(2, 2, 120, 90),
            props: vec![ispe(120, 90), clap((100, 1), (80, 1), (-2, 1), (0, 1)), irot(1)],
            dimg: vec![2, 3, 4, 5],
            construction: 1,
        }];
        for t in 0..4u8 {
            items.push(hvc1(&[0, 0, 0, 1, t], vec![ispe(64, 48)]));
        }
        let f = still(1, &items);
        let h = parse_heif(&f).unwrap();
        let g = h.item(1).unwrap();
        assert_eq!(g.kind, ItemKind::Grid);
        assert_eq!(g.dimg, vec![2, 3, 4, 5]);
        assert_eq!(h.refs, vec![Reference { kind: *b"dimg", from: 1, to: vec![2, 3, 4, 5] }]);
        let grid = parse_grid(&h.data(&f, g).unwrap()).unwrap();
        assert_eq!(grid, Grid { rows: 2, columns: 2, width: 120, height: 90 });
        // The grid's bytes came from idat (construction method 1), the tiles' from mdat.
        let idat = f.windows(4).position(|w| w == b"idat").unwrap() + 4;
        assert_eq!(g.extents.as_ref().unwrap()[0], (idat, 8));
        for t in 0..4u32 {
            assert_eq!(h.data(&f, h.item(t + 2).unwrap()).unwrap(), vec![0, 0, 0, 1, t as u8]);
        }
        let c = Clap { width: (100, 1), height: (80, 1), horiz_off: (-2, 1), vert_off: (0, 1) };
        assert_eq!(g.transforms, vec![Transform::Clap(c), Transform::Irot(1)]);
        // x = -2 + (120 - 100) / 2 = 8, y = (90 - 80) / 2 = 5.
        assert_eq!(c.rect(120, 90), Some((8, 5, 100, 80)));
        assert_eq!(parse_grid(&grid_data(1, 3, 70000, 2)).unwrap(), Grid { rows: 1, columns: 3, width: 70000, height: 2 });
    }

    #[test]
    fn clap_rect_rounds_and_bounds() {
        let c = |w, h, ho, vo| Clap { width: w, height: h, horiz_off: ho, vert_off: vo };
        assert_eq!(c((56, 1), (40, 1), (0, 1), (0, 1)).rect(64, 48), Some((4, 4, 56, 40)));
        // Half-pixel offsets floor: x = -1/2 + 4 = 3.5 -> 3.
        assert_eq!(c((112, 2), (40, 1), (-1, 2), (1, 2)).rect(64, 48), Some((3, 4, 56, 40)));
        assert_eq!(c((56, 1), (40, 1), (-5, 1), (0, 1)).rect(64, 48), None, "left of the image");
        assert_eq!(c((65, 1), (40, 1), (0, 1), (0, 1)).rect(64, 48), None, "wider than the image");
        assert_eq!(c((56, 0), (40, 1), (0, 1), (0, 1)).rect(64, 48), None);
    }

    #[test]
    fn refusals_are_named() {
        let one = [hvc1(b"x", vec![])];
        let mut ftyp_only = vec![0, 0, 0, 0x18];
        ftyp_only.extend(b"ftypheic\0\0\0\0mif1heic");
        assert_eq!(parse_heif(&ftyp_only), Err(Error::Invalid("heif: no meta box")));
        assert_eq!(parse_heif(&still_with_handler(1, &one, b"vide")), Err(Error::Unsupported("heif: meta handler is not pict")));
        assert_eq!(parse_heif(&still(2, &one)), Err(Error::Invalid("heif: primary item missing from iinf")));
        assert_eq!(parse_heif(b"\0\0\0\x08moov"), Err(Error::UnknownFormat));
        let f = still(1, &[ItemSpec { construction: 2, ..hvc1(b"x", vec![]) }]);
        let h = parse_heif(&f).unwrap();
        assert_eq!(h.data(&f, h.item(1).unwrap()), Err(Error::Unsupported("heif: iloc construction_method 2 (item)")));
        // Cut inside meta: the meta box claims more than the file holds.
        let f = still(1, &one);
        let meta = f.windows(4).position(|w| w == b"meta").unwrap();
        assert!(parse_heif(&f[..meta + 40]).is_err());
        // Every prefix is an Err or a parse, never a panic.
        for n in 0..f.len() {
            let _ = parse_heif(&f[..n]);
        }
    }

    #[test]
    fn heic_brands_are_told_from_avif_and_sequences() {
        let ftyp = |brands: &[u8]| {
            let mut v = ((brands.len() + 12) as u32).to_be_bytes().to_vec();
            v.extend(b"ftyp");
            v.extend(&brands[..4]);
            v.extend([0, 0, 0, 0]);
            v.extend(&brands[4..]);
            v
        };
        assert!(is_heic(&ftyp(b"heicmif1heic")));
        assert!(is_heic(&ftyp(b"mif1mif1")));
        assert!(is_heic(&ftyp(b"heixmif1")));
        assert!(!is_heic(&ftyp(b"avifmif1miaf")));
        assert!(!is_heic(&ftyp(b"mif1avif")));
        assert!(!is_heic(&ftyp(b"heismsf1hevc")));
        assert!(!is_heic(&ftyp(b"isommp41")));
        assert!(!is_heic(b""));
        assert!(!is_heic(b"\0\0\0\x20ftypheic"), "ftyp claiming more than the file");
        // HEIFONE (SR303): AVIF stills are told apart; an avis-only sequence is not a still.
        assert!(is_avif(&ftyp(b"avifmif1miaf")));
        assert!(is_avif(&ftyp(b"mif1avif")));
        assert!(!is_avif(&ftyp(b"avismsf1")));
        assert!(!is_avif(&ftyp(b"heicmif1heic")));
        assert!(!is_avif(b""));
    }

    #[test]
    fn an_av01_item_reads_back_with_av1c_and_pixi() {
        let rec = [0x81, 0x20, 0x00, 0x00];
        let item = ItemSpec {
            kind: *b"av01",
            data: b"\x12\x00OBU".to_vec(),
            props: vec![ispe(1, 1), pixi(&[8, 8, 8]), av1c(&rec), nclx(1, 13, 6, true), irot(3)],
            ..Default::default()
        };
        let f = avif(1, &[item]);
        assert!(is_avif(&f) && !is_heic(&f));
        let h = parse_heif(&f).unwrap();
        assert_eq!(&h.brands[..], &[*b"avif", *b"avif", *b"mif1", *b"miaf"]);
        let it = h.item(1).unwrap();
        assert_eq!(it.kind, ItemKind::Av01);
        assert_eq!(it.av1c.as_deref(), Some(&rec[..]));
        assert_eq!(it.pixi.as_deref(), Some(&[8u8, 8, 8][..]));
        assert_eq!((it.hvcc.as_deref(), it.ispe), (None, Some((1, 1))));
        assert_eq!(it.nclx.map(|n| n.matrix_coefficients), Some(6));
        assert_eq!(it.transforms, vec![Transform::Irot(3)]);
        assert_eq!(h.data(&f, it).unwrap(), b"\x12\x00OBU");
    }
}
