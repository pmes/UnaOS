// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The `UNARASTR` container, version 1, little-endian throughout.
//!
//! ```text
//! header (16 bytes): magic b"UNARASTR" | version u32 = 1 | chunk_count u32
//! chunk:             tag [u8; 4] | len u32 | payload[len]
//! META (one, first): width u32 | height u32 | layer_count u32 | per layer: name_len u32 | name UTF-8 |
//!                    w u32 | h u32 | ox i32 | oy i32 | opacity u8 | blend u8 | visible u8 | has_mask u8
//! LAYR (one per layer, in order): a colour-type-6 PNG (pixel_core `png::encode::encode_rgba` / `decode_png`)
//! MASK (one per masked layer): layer_index u32 | w u32 | h u32 | coverage, RAW (w * h bytes)
//! SELE (optional, one): kind u8 (0 rect, 1 ellipse, 2 polygon, 3 coverage) | rect / ellipse: 4 x i32 |
//!                    polygon: n u32, n x (i32, i32) | coverage: w u32 | h u32 | w * h bytes
//! ```
//!
//! Unknown tags are skipped. Mask coverage is raw in this version.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Error;
use crate::document::{BlendMode, Document, Layer, Mask};
use crate::selection::Selection;

pub const MAGIC: [u8; 8] = *b"UNARASTR";
pub const VERSION: u32 = 1;

const META: [u8; 4] = *b"META";
const LAYR: [u8; 4] = *b"LAYR";
const MASK: [u8; 4] = *b"MASK";
const SELE: [u8; 4] = *b"SELE";

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn puti(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn chunk(out: &mut Vec<u8>, tag: [u8; 4], payload: &[u8]) {
    out.extend_from_slice(&tag);
    put32(out, payload.len() as u32);
    out.extend_from_slice(payload);
}

fn put_mask(p: &mut Vec<u8>, m: &Mask) {
    put32(p, m.width);
    put32(p, m.height);
    p.extend_from_slice(&m.coverage);
}

/// The document as an `UNARASTR` file. It validates first; a layer pixel_core cannot encode is [`Error::PngEncode`].
pub fn encode(doc: &Document) -> Result<Vec<u8>, Error> {
    doc.validate()?;
    let mut chunks: Vec<([u8; 4], Vec<u8>)> = Vec::new();

    let mut meta = Vec::new();
    put32(&mut meta, doc.width);
    put32(&mut meta, doc.height);
    put32(&mut meta, doc.layers.len() as u32);
    for l in &doc.layers {
        put32(&mut meta, l.name.len() as u32);
        meta.extend_from_slice(l.name.as_bytes());
        put32(&mut meta, l.width);
        put32(&mut meta, l.height);
        puti(&mut meta, l.offset.0);
        puti(&mut meta, l.offset.1);
        meta.extend_from_slice(&[l.opacity, l.blend.to_byte(), l.visible as u8, l.mask.is_some() as u8]);
    }
    chunks.push((META, meta));

    for l in &doc.layers {
        let png = pixel_core::png::encode::encode_rgba(l.width, l.height, &l.pixels).map_err(|_| Error::PngEncode)?;
        chunks.push((LAYR, png));
    }
    for (i, l) in doc.layers.iter().enumerate() {
        if let Some(m) = &l.mask {
            let mut p = Vec::new();
            put32(&mut p, i as u32);
            put_mask(&mut p, m);
            chunks.push((MASK, p));
        }
    }
    if let Some(s) = &doc.selection {
        let mut p = Vec::new();
        match s {
            Selection::Rect { x, y, w, h } => {
                p.push(0);
                for v in [*x, *y, *w, *h] {
                    puti(&mut p, v);
                }
            }
            Selection::Ellipse { cx, cy, rx, ry } => {
                p.push(1);
                for v in [*cx, *cy, *rx, *ry] {
                    puti(&mut p, v);
                }
            }
            Selection::Polygon(pts) => {
                p.push(2);
                put32(&mut p, pts.len() as u32);
                for (x, y) in pts {
                    puti(&mut p, *x);
                    puti(&mut p, *y);
                }
            }
            Selection::Coverage(m) => {
                p.push(3);
                put_mask(&mut p, m);
            }
        }
        chunks.push((SELE, p));
    }

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    put32(&mut out, VERSION);
    put32(&mut out, chunks.len() as u32);
    for (tag, payload) in &chunks {
        chunk(&mut out, *tag, payload);
    }
    Ok(out)
}

/// A bounds-checked little-endian reader over one payload; running short is [`Error::Malformed`].
struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Malformed)?;
        let s = self.b.get(self.at..end).ok_or(Error::Malformed)?;
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Result<i32, Error> {
        Ok(self.u32()? as i32)
    }
    fn mask(&mut self) -> Result<Mask, Error> {
        let (w, h) = (self.u32()?, self.u32()?);
        if (w as u64) * (h as u64) > pixel_core::MAX_PIXELS {
            return Err(Error::TooLarge);
        }
        let coverage = self.take(w as usize * h as usize)?.to_vec();
        Ok(Mask { width: w, height: h, coverage })
    }
}

/// One layer's META record, before its pixels arrive.
struct LayerMeta {
    name: String,
    width: u32,
    height: u32,
    offset: (i32, i32),
    opacity: u8,
    blend: BlendMode,
    visible: bool,
    has_mask: bool,
}

/// Read an `UNARASTR` file back into a validated [`Document`].
pub fn decode(bytes: &[u8]) -> Result<Document, Error> {
    if bytes.len() < 16 {
        return Err(Error::Truncated);
    }
    if bytes[0..8] != MAGIC {
        return Err(Error::Magic);
    }
    let le = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    if le(8) != VERSION {
        return Err(Error::Version);
    }
    let count = le(12);

    let mut size: Option<(u32, u32)> = None;
    let mut metas: Vec<LayerMeta> = Vec::new();
    let mut layers: Vec<Layer> = Vec::new();
    let mut selection: Option<Selection> = None;
    let mut at = 16usize;
    for _ in 0..count {
        if bytes.len() - at < 8 {
            return Err(Error::Truncated);
        }
        let tag = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
        let len = le(at + 4) as usize;
        at += 8;
        if bytes.len() - at < len {
            return Err(Error::Truncated);
        }
        let payload = &bytes[at..at + len];
        at += len;
        let mut r = Rd { b: payload, at: 0 };
        match tag {
            META => {
                if size.is_some() {
                    return Err(Error::Malformed);
                }
                size = Some((r.u32()?, r.u32()?));
                let n = r.u32()?;
                for _ in 0..n {
                    let name_len = r.u32()? as usize;
                    let name = core::str::from_utf8(r.take(name_len)?).map_err(|_| Error::Malformed)?;
                    let (width, height) = (r.u32()?, r.u32()?);
                    let offset = (r.i32()?, r.i32()?);
                    let opacity = r.u8()?;
                    let blend = BlendMode::from_byte(r.u8()?).ok_or(Error::Malformed)?;
                    let visible = match r.u8()? {
                        0 => false,
                        1 => true,
                        _ => return Err(Error::Malformed),
                    };
                    let has_mask = r.u8()? != 0;
                    metas.push(LayerMeta { name: String::from(name), width, height, offset, opacity, blend, visible, has_mask });
                }
            }
            LAYR => {
                let m = metas.get(layers.len()).ok_or(Error::Malformed)?;
                let img = pixel_core::decode_png(payload).map_err(|_| Error::Png)?;
                if img.width != m.width || img.height != m.height {
                    return Err(Error::Malformed);
                }
                layers.push(Layer {
                    name: m.name.clone(),
                    pixels: img.rgba,
                    width: m.width,
                    height: m.height,
                    offset: m.offset,
                    opacity: m.opacity,
                    blend: m.blend,
                    visible: m.visible,
                    mask: None,
                });
            }
            MASK => {
                let index = r.u32()? as usize;
                let mask = r.mask()?;
                let l = layers.get_mut(index).ok_or(Error::Malformed)?;
                if !metas[index].has_mask || l.mask.is_some() {
                    return Err(Error::Malformed);
                }
                l.mask = Some(mask);
            }
            SELE => {
                if selection.is_some() {
                    return Err(Error::Malformed);
                }
                selection = Some(match r.u8()? {
                    0 => Selection::Rect { x: r.i32()?, y: r.i32()?, w: r.i32()?, h: r.i32()? },
                    1 => Selection::Ellipse { cx: r.i32()?, cy: r.i32()?, rx: r.i32()?, ry: r.i32()? },
                    2 => {
                        let n = r.u32()? as usize;
                        if n > payload.len() / 8 {
                            return Err(Error::Malformed);
                        }
                        let mut pts = Vec::with_capacity(n);
                        for _ in 0..n {
                            pts.push((r.i32()?, r.i32()?));
                        }
                        Selection::Polygon(pts)
                    }
                    3 => Selection::Coverage(r.mask()?),
                    _ => return Err(Error::Malformed),
                });
            }
            _ => {} // an unknown chunk: skipped (forward compatible)
        }
    }
    let (width, height) = size.ok_or(Error::Malformed)?;
    if layers.len() != metas.len() || metas.iter().zip(&layers).any(|(m, l)| m.has_mask != l.mask.is_some()) {
        return Err(Error::Malformed);
    }
    let doc = Document { width, height, layers, selection };
    doc.validate()?;
    Ok(doc)
}
