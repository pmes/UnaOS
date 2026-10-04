//! AVIF container: ISOBMFF boxes (ISO/IEC 14496-12) carrying a HEIF image item
//! (ISO/IEC 23008-12) of type `av01` (AV1 Image File Format, AOMedia v1.1).
//!
//! What is read: `ftyp` (brand check), `meta` → `hdlr` (`pict`), `pitm` (primary item), `iinf`/`infe`
//! (item types), `iloc` (versions 0–2, construction methods 0 = file and 1 = `idat`), `iprp` →
//! `ipco` (`ispe`, `pixi`, `av1C`, `colr` nclx/ICC, `irot`, `imir`, `clap`) and `ipma` (association),
//! and the item's bytes, which are a low-overhead AV1 OBU stream (§5.2 of the AV1 spec).
//! Not read (owed): `grid`/`iovl` derived items, alpha (`auxl`) and gain-map items, `idat`-less
//! multi-extent edge cases beyond concatenation.

use crate::obu::{Obu, split_obus};
use crate::{Error, Result};
use alloc::vec::Vec;

/// `av1C` (AV1CodecConfigurationRecord).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Av1Config {
    pub seq_profile: u8,
    pub seq_level_idx_0: u8,
    pub seq_tier_0: u8,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub monochrome: bool,
    pub chroma_subsampling_x: bool,
    pub chroma_subsampling_y: bool,
    pub chroma_sample_position: u8,
    /// configOBUs (normally the sequence header).
    pub config_obus: Vec<u8>,
}

/// `colr` of type `nclx`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Nclx {
    pub colour_primaries: u16,
    pub transfer_characteristics: u16,
    pub matrix_coefficients: u16,
    pub full_range: bool,
}

/// The primary image item of an AVIF file: its AV1 payload and the properties that matter for
/// presenting it.
#[derive(Debug, Clone, Default)]
pub struct Obus {
    /// The item's bytes: a low-overhead AV1 OBU stream.
    pub data: Vec<u8>,
    /// `ispe` image spatial extents.
    pub width: u32,
    pub height: u32,
    /// `pixi` bits per channel.
    pub bits_per_channel: Vec<u8>,
    pub av1c: Option<Av1Config>,
    pub nclx: Option<Nclx>,
    pub has_icc: bool,
    /// `irot` angle (×90° anticlockwise), `imir` axis, `clap` present: parsed, applied by nobody yet.
    pub irot: Option<u8>,
    pub imir: Option<u8>,
    pub has_clap: bool,
}

impl Obus {
    /// The OBUs of the payload.
    pub fn obus(&self) -> Result<Vec<Obu<'_>>> {
        split_obus(&self.data)
    }
}

struct BoxHdr {
    typ: [u8; 4],
    body: usize, // offset of body
    end: usize,
}

fn be16(d: &[u8], o: usize) -> Result<u32> {
    d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as u32).ok_or(Error::Truncated)
}
fn be32(d: &[u8], o: usize) -> Result<u32> {
    d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or(Error::Truncated)
}
fn be_n(d: &[u8], o: usize, n: usize) -> Result<u64> {
    let mut v = 0u64;
    for i in 0..n {
        v = (v << 8) | *d.get(o + i).ok_or(Error::Truncated)? as u64;
    }
    Ok(v)
}

fn read_box(d: &[u8], o: usize, end: usize) -> Result<BoxHdr> {
    let size = be32(d, o)? as u64;
    let typ: [u8; 4] = d.get(o + 4..o + 8).ok_or(Error::Truncated)?.try_into().unwrap();
    let (size, hdr) = match size {
        1 => (be_n(d, o + 8, 8)?, 16usize),
        0 => ((end - o) as u64, 8usize),
        s => (s, 8usize),
    };
    let bend = o as u64 + size;
    if size < hdr as u64 || bend > end as u64 {
        return Err(Error::Invalid("box size"));
    }
    Ok(BoxHdr { typ, body: o + hdr, end: bend as usize })
}

fn children(d: &[u8], start: usize, end: usize) -> Result<Vec<BoxHdr>> {
    let mut v = Vec::new();
    let mut o = start;
    while o + 8 <= end {
        let b = read_box(d, o, end)?;
        o = b.end;
        v.push(b);
    }
    Ok(v)
}

struct Extent {
    offset: u64,
    length: u64,
}
struct ItemLoc {
    id: u32,
    construction_method: u8,
    base_offset: u64,
    extents: Vec<Extent>,
}

/// Extract the primary `av01` item of an AVIF file.
pub fn avif_payload(file: &[u8]) -> Result<Obus> {
    let top = children(file, 0, file.len())?;
    let ftyp = top.iter().find(|b| &b.typ == b"ftyp").ok_or(Error::Invalid("no ftyp"))?;
    {
        let mut ok = false;
        let mut o = ftyp.body;
        while o + 4 <= ftyp.end {
            let brand = &file[o..o + 4];
            if brand == b"avif" || brand == b"avis" || brand == b"mif1" || brand == b"miaf" {
                ok = true;
            }
            o += if o == ftyp.body { 8 } else { 4 }; // skip minor_version after major_brand
        }
        if !ok {
            return Err(Error::Invalid("ftyp: not an AVIF/HEIF brand"));
        }
    }
    let meta = top.iter().find(|b| &b.typ == b"meta").ok_or(Error::Invalid("no meta"))?;
    // meta is a FullBox: 4 bytes version/flags.
    let mboxes = children(file, meta.body + 4, meta.end)?;
    let mut primary: Option<u32> = None;
    let mut item_types: Vec<(u32, [u8; 4])> = Vec::new();
    let mut locs: Vec<ItemLoc> = Vec::new();
    let mut idat: Option<(usize, usize)> = None;
    let mut props: Vec<(usize, usize, [u8; 4])> = Vec::new(); // (body, end, type), 1-based index = pos+1
    let mut assoc: Vec<(u32, Vec<u16>)> = Vec::new();
    for b in &mboxes {
        match &b.typ {
            b"hdlr" => {
                let h = file.get(b.body + 8..b.body + 12).ok_or(Error::Truncated)?;
                if h != b"pict" {
                    return Err(Error::Invalid("hdlr is not pict"));
                }
            }
            b"pitm" => {
                let v = file[b.body];
                primary = Some(if v == 0 { be16(file, b.body + 4)? } else { be32(file, b.body + 4)? });
            }
            b"iinf" => {
                let v = file[b.body];
                let first = if v == 0 { b.body + 6 } else { b.body + 8 };
                for infe in children(file, first, b.end)? {
                    if &infe.typ != b"infe" {
                        continue;
                    }
                    let v = file[infe.body];
                    if v < 2 {
                        continue;
                    }
                    let (id, o) = if v == 2 { (be16(file, infe.body + 4)?, infe.body + 6) } else { (be32(file, infe.body + 4)?, infe.body + 8) };
                    let t: [u8; 4] = file.get(o + 2..o + 6).ok_or(Error::Truncated)?.try_into().unwrap();
                    item_types.push((id, t));
                }
            }
            b"iloc" => {
                let v = file[b.body];
                let mut o = b.body + 4;
                let a = file[o];
                let c = file[o + 1];
                let offset_size = (a >> 4) as usize;
                let length_size = (a & 15) as usize;
                let base_offset_size = (c >> 4) as usize;
                let index_size = if v == 1 || v == 2 { (c & 15) as usize } else { 0 };
                o += 2;
                let count = if v < 2 { let n = be16(file, o)?; o += 2; n } else { let n = be32(file, o)?; o += 4; n };
                for _ in 0..count {
                    let id = if v < 2 { let n = be16(file, o)?; o += 2; n } else { let n = be32(file, o)?; o += 4; n };
                    let mut cm = 0u8;
                    if v == 1 || v == 2 {
                        cm = (be16(file, o)? & 15) as u8;
                        o += 2;
                    }
                    let _dri = be16(file, o)?;
                    o += 2;
                    let base_offset = be_n(file, o, base_offset_size)?;
                    o += base_offset_size;
                    let ec = be16(file, o)?;
                    o += 2;
                    let mut extents = Vec::new();
                    for _ in 0..ec {
                        o += index_size;
                        let offset = be_n(file, o, offset_size)?;
                        o += offset_size;
                        let length = be_n(file, o, length_size)?;
                        o += length_size;
                        extents.push(Extent { offset, length });
                    }
                    locs.push(ItemLoc { id, construction_method: cm, base_offset, extents });
                }
            }
            b"idat" => idat = Some((b.body, b.end)),
            b"iprp" => {
                for pb in children(file, b.body, b.end)? {
                    if &pb.typ == b"ipco" {
                        for p in children(file, pb.body, pb.end)? {
                            props.push((p.body, p.end, p.typ));
                        }
                    } else if &pb.typ == b"ipma" {
                        let v = file[pb.body];
                        let flags = be_n(file, pb.body + 1, 3)?;
                        let mut o = pb.body + 4;
                        let n = be32(file, o)?;
                        o += 4;
                        for _ in 0..n {
                            let id = if v < 1 { let x = be16(file, o)?; o += 2; x } else { let x = be32(file, o)?; o += 4; x };
                            let cnt = file[o] as usize;
                            o += 1;
                            let mut list = Vec::new();
                            for _ in 0..cnt {
                                if flags & 1 != 0 {
                                    list.push((be16(file, o)? & 0x7fff) as u16);
                                    o += 2;
                                } else {
                                    list.push((file[o] & 0x7f) as u16);
                                    o += 1;
                                }
                            }
                            assoc.push((id, list));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let pid = primary.ok_or(Error::Invalid("no pitm"))?;
    let ptype = item_types.iter().find(|(id, _)| *id == pid).map(|x| x.1).ok_or(Error::Invalid("primary item has no infe"))?;
    if &ptype == b"grid" {
        return Err(Error::Unsupported("AVIF grid item"));
    }
    if &ptype != b"av01" {
        return Err(Error::Invalid("primary item is not av01"));
    }
    let loc = locs.iter().find(|l| l.id == pid).ok_or(Error::Invalid("primary item has no iloc"))?;
    let mut data = Vec::new();
    for e in &loc.extents {
        let (base, limit) = match loc.construction_method {
            0 => (0usize, file.len()),
            1 => idat.ok_or(Error::Invalid("iloc method 1 without idat"))?,
            _ => return Err(Error::Unsupported("iloc construction_method 2")),
        };
        let start = base as u64 + loc.base_offset + e.offset;
        let len = if e.length == 0 { limit as u64 - start } else { e.length };
        let end = start + len;
        if end > limit as u64 {
            return Err(Error::Truncated);
        }
        data.extend_from_slice(&file[start as usize..end as usize]);
    }
    let mut out = Obus { data, ..Default::default() };
    if let Some((_, list)) = assoc.iter().find(|(id, _)| *id == pid) {
        for &idx in list {
            if idx == 0 || idx as usize > props.len() {
                continue;
            }
            let (body, end, typ) = props[idx as usize - 1];
            match &typ {
                b"ispe" => {
                    out.width = be32(file, body + 4)?;
                    out.height = be32(file, body + 8)?;
                }
                b"pixi" => {
                    let n = file[body + 4] as usize;
                    out.bits_per_channel = file.get(body + 5..body + 5 + n).ok_or(Error::Truncated)?.to_vec();
                }
                b"av1C" => {
                    let b = file.get(body..end).ok_or(Error::Truncated)?;
                    if b.len() < 4 || b[0] & 0x7f != 1 {
                        return Err(Error::Invalid("av1C"));
                    }
                    out.av1c = Some(Av1Config {
                        seq_profile: b[1] >> 5,
                        seq_level_idx_0: b[1] & 31,
                        seq_tier_0: b[2] >> 7,
                        high_bitdepth: b[2] & 0x40 != 0,
                        twelve_bit: b[2] & 0x20 != 0,
                        monochrome: b[2] & 0x10 != 0,
                        chroma_subsampling_x: b[2] & 0x08 != 0,
                        chroma_subsampling_y: b[2] & 0x04 != 0,
                        chroma_sample_position: b[2] & 3,
                        config_obus: b[4..].to_vec(),
                    });
                }
                b"colr" => {
                    let t = file.get(body..body + 4).ok_or(Error::Truncated)?;
                    if t == b"nclx" {
                        out.nclx = Some(Nclx {
                            colour_primaries: be16(file, body + 4)? as u16,
                            transfer_characteristics: be16(file, body + 6)? as u16,
                            matrix_coefficients: be16(file, body + 8)? as u16,
                            full_range: file.get(body + 10).ok_or(Error::Truncated)? & 0x80 != 0,
                        });
                    } else if t == b"prof" || t == b"rICC" {
                        out.has_icc = true;
                    }
                }
                b"irot" => out.irot = Some(file[body] & 3),
                b"imir" => out.imir = Some(file[body] & 1),
                b"clap" => out.has_clap = true,
                _ => {}
            }
        }
    }
    Ok(out)
}
