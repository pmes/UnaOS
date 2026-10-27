// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Images (ISO 32000-1 §8.9): image XObjects and inline images (§8.9.7, the abbreviations rebuilt as a stream so
//! `filters::decode` is the one decoder) to a premultiplied svg_core `Pixmap`. DCTDecode goes through
//! `pixel_core::jpeg`; raw samples at 1/2/4/8/16 bits per component with /Decode through the colour space; /ImageMask
//! stencils (painted later in the fill colour); /SMask as alpha. CCITTFax and JBIG2 decode to 1-bit rows through
//! `filters::bilevel` and take the stencil or bpc-1 sample path (PDFFAX SR331; a feature their decoders refuse is
//! refused by its name). JPX is refused by name (the caller draws a grey box; PDF-JPX owed). Explicit and colour-key
//! /Mask and /Matte are owed (PDF-IMGMASK).

use alloc::vec;
use alloc::vec::Vec;

use svg_core::raster::Pixmap;

use super::colour::ColourSpace;
use crate::doc::Document;
use crate::filters::{self, Decoded};
use crate::object::{Dict, Object, Stream};

/// A decoded image.
pub enum Img {
    /// Colour samples (premultiplied), drawn as they are; `smooth` = /Interpolate.
    Pix { pix: Pixmap, smooth: bool },
    /// A stencil: only the alpha channel matters; painted in the fill colour.
    Stencil { pix: Pixmap, smooth: bool },
    /// An image codec this core does not decode, by its filter name.
    Refused(&'static str),
}

/// The inline-image abbreviations (§8.9.7, Table 93) as their full keys.
fn full_key(k: &[u8]) -> &[u8] {
    match k {
        b"BPC" => b"BitsPerComponent",
        b"CS" => b"ColorSpace",
        b"D" => b"Decode",
        b"DP" => b"DecodeParms",
        b"F" => b"Filter",
        b"H" => b"Height",
        b"IM" => b"ImageMask",
        b"I" => b"Interpolate",
        b"W" => b"Width",
        k => k,
    }
}

/// An inline image: its dictionary (abbreviated keys) and its data.
pub fn inline(d: &Dict, data: &[u8], res: &Dict, doc: &Document) -> Option<Img> {
    let mut dict = Dict::new();
    for (k, v) in d.iter() {
        dict.insert(full_key(k).to_vec(), v.clone());
    }
    decode(&Stream { dict, data: data.to_vec() }, res, doc, 0)
}

/// An image XObject.
pub fn xobject(s: &Stream, res: &Dict, doc: &Document) -> Option<Img> {
    decode(s, res, doc, 0)
}

fn decode(s: &Stream, res: &Dict, doc: &Document, depth: usize) -> Option<Img> {
    let d = &s.dict;
    let get = |k: &[u8]| d.get(k).map(|o| doc.get(o));
    let int = |k: &[u8]| get(k).and_then(|o| o.as_int());
    let w = int(b"Width")?;
    let h = int(b"Height")?;
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) * 4 > crate::MAX_DECODED as u64 {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let smooth = matches!(get(b"Interpolate"), Some(Object::Bool(true)));
    let stencil = matches!(get(b"ImageMask"), Some(Object::Bool(true)));
    let dec = doc.decode(s).ok()?;
    let (dec, bits1) = match bilevel_of(dec, doc, w, h) { Ok(v) => v, Err(why) => return Some(Img::Refused(why)) };
    let mut pix = if let Some(f) = &dec.image_filter {
        match f.as_slice() {
            b"DCTDecode" => {
                // The JPEG's own size wins over /Width /Height (the dictionary is sometimes stale).
                let img = pixel_core::jpeg::decode(&dec.data).ok()?;
                let mut p = Pixmap::new(img.width as usize, img.height as usize);
                p.data.copy_from_slice(&img.rgba);
                p
            }
            // CCITTFax and JBIG2 became 1-bit rows above; JPX is owed (PDF-JPX: mq.rs is its first piece).
            _ => return Some(Img::Refused("JPXDecode")),
        }
    } else if stencil {
        let decode = nums(doc, get(b"Decode")).unwrap_or_else(|| vec![0.0, 1.0]);
        let paint_on = if decode.first().copied().unwrap_or(0.0) > 0.5 { 1 } else { 0 };
        let row = w.div_ceil(8);
        let mut p = Pixmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let byte = dec.data.get(y * row + x / 8).copied().unwrap_or(0);
                let bit = (byte >> (7 - x % 8)) & 1;
                if bit == paint_on {
                    p.data[(y * w + x) * 4 + 3] = 255;
                }
            }
        }
        return Some(Img::Stencil { pix: p, smooth });
    } else {
        let bpc = if bits1 { 1 } else { int(b"BitsPerComponent").unwrap_or(8) as u32 };
        if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
            return None;
        }
        let cs = ColourSpace::parse(&get(b"ColorSpace")?, res, doc)?;
        raw(&dec.data, w, h, bpc, &cs, nums(doc, get(b"Decode")))?
    };
    // /SMask: a DeviceGray image whose samples are this image's alpha, resampled (nearest) to its size.
    if depth == 0 {
        if let Some(Object::Stream(sm)) = get(b"SMask") {
            let mut sd = sm.dict.clone();
            sd.insert(b"ColorSpace".to_vec(), Object::Name(b"DeviceGray".to_vec()));
            if let Some(Img::Pix { pix: m, .. }) = decode(&Stream { dict: sd, data: sm.data.clone() }, res, doc, depth + 1) {
                apply_alpha(&mut pix, &m);
            }
        }
    }
    Some(Img::Pix { pix, smooth })
}

fn nums(doc: &Document, o: Option<Object>) -> Option<Vec<f64>> {
    let o = o?;
    o.as_array()?.iter().map(|x| doc.get(x).as_f64()).collect()
}

/// Raw samples to an opaque pixmap.
fn raw(data: &[u8], w: usize, h: usize, bpc: u32, cs: &ColourSpace, decode: Option<Vec<f64>>) -> Option<Pixmap> {
    let n = cs.n();
    if n == 0 {
        return None;
    }
    let decode = decode.filter(|d| d.len() >= 2 * n).unwrap_or_else(|| cs.default_decode(bpc));
    let maxv = ((1u32 << bpc) - 1) as f64;
    let row = (w * n * bpc as usize).div_ceil(8);
    let sample = |y: usize, i: usize| -> u32 {
        let bit = i * bpc as usize;
        let at = y * row + bit / 8;
        match bpc {
            16 => (data.get(at).copied().unwrap_or(0) as u32) << 8 | data.get(at + 1).copied().unwrap_or(0) as u32,
            8 => data.get(at).copied().unwrap_or(0) as u32,
            _ => (data.get(at).copied().unwrap_or(0) as u32 >> (8 - bpc as usize - bit % 8)) & ((1 << bpc) - 1),
        }
    };
    let comp = |k: usize, s: u32| decode[2 * k] + s as f64 * (decode[2 * k + 1] - decode[2 * k]) / maxv;
    let mut p = Pixmap::new(w, h);
    // One component at up to 8 bits: a lookup table of every sample value.
    let lut: Option<Vec<[f32; 3]>> = (n == 1 && bpc <= 8).then(|| (0..=maxv as u32).map(|s| cs.to_rgb(&[comp(0, s)])).collect());
    let mut comps = vec![0.0; n];
    for y in 0..h {
        for x in 0..w {
            let rgb = match &lut {
                Some(t) => t[sample(y, x) as usize],
                None => {
                    for (k, c) in comps.iter_mut().enumerate() {
                        *c = comp(k, sample(y, x * n + k));
                    }
                    cs.to_rgb(&comps)
                }
            };
            let o = (y * w + x) * 4;
            p.data[o] = (rgb[0] * 255.0 + 0.5) as u8;
            p.data[o + 1] = (rgb[1] * 255.0 + 0.5) as u8;
            p.data[o + 2] = (rgb[2] * 255.0 + 0.5) as u8;
            p.data[o + 3] = 255;
        }
    }
    Some(p)
}

/// Multiply an opaque pixmap by a grey mask (its red channel), nearest-resampled; the result stays premultiplied.
fn apply_alpha(pix: &mut Pixmap, m: &Pixmap) {
    for y in 0..pix.h {
        let my = (y * m.h / pix.h).min(m.h - 1);
        for x in 0..pix.w {
            let mx = (x * m.w / pix.w).min(m.w - 1);
            let a = m.data[(my * m.w + mx) * 4] as u32;
            let o = (y * pix.w + x) * 4;
            for c in &mut pix.data[o..o + 4] {
                *c = svg_core::raster::div255(*c as u32 * a) as u8;
            }
        }
    }
}

/// CCITTFaxDecode / JBIG2Decode (PDFFAX SR331): their rows through `filters::bilevel`, flagged as 1 bit per sample;
/// any other stream passes through. A refusal carries the decoder's feature name, else the filter's.
fn bilevel_of(dec: Decoded, doc: &Document, w: usize, h: usize) -> Result<(Decoded, bool), &'static str> {
    let name = match dec.image_filter.as_deref() {
        Some(b"CCITTFaxDecode") => "CCITTFaxDecode",
        Some(b"JBIG2Decode") => "JBIG2Decode",
        _ => return Ok((dec, false)),
    };
    match filters::bilevel(&dec, &|o| doc.get(o), w, h) {
        Ok(data) => Ok((Decoded { data, image_filter: None, image_parms: None }, true)),
        Err(crate::Error::Unsupported(why)) => Err(why),
        Err(_) => Err(name),
    }
}
