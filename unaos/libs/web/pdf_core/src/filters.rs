// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The standard stream filters (ISO 32000-1 §7.4).
//!
//! FlateDecode is pixel_core's inflater (`zlib_inflate`, and `inflate_raw` when the zlib header is refused — the
//! http_core rule); this module adds no DEFLATE decoder of its own. Real files carry Flate streams whose Adler-32 is
//! wrong or missing, or that stop short: the bytes already produced are kept, as every reader does. The predictors
//! (§7.4.4.4): PNG 10-15 runs on pixel_core's `png::unfilter_rows` (PIXEL-UNFILTER SR316; the PDF lenient rules —
//! an unknown filter type reads as None, a short last row is kept — stay here); TIFF 2 lives here (pixel_core has none).
//! The image codecs (DCT, JPX, CCITTFax, JBIG2) stop the chain: decoding stops at the first one and the result
//! carries the raw bytes and the filter's name (PDF2 decodes DCT through pixel_core::jpeg); [`bilevel`] then decodes
//! CCITTFax ([`ccitt`]) and JBIG2 ([`jbig2`], over the one MQ coder [`mq`]) to 1-bit rows (PDFFAX SR331). JPX decodes
//! through [`jpx`] (PDFJPX SR335, over the same [`mq`]), called by `render::image`.

use alloc::vec;
use alloc::vec::Vec;

use pixel_core::inflate::{self, ByteSource, InflateError, Sink};

use crate::object::{Dict, Object, Stream};
use crate::{Error, MAX_DECODED};

pub mod ccitt;
pub mod jbig2;
pub mod jpx;
pub mod mq;

/// A decoded stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub data: Vec<u8>,
    /// The image filter decoding stopped at (`DCTDecode`, `JPXDecode`, `CCITTFaxDecode`, `JBIG2Decode`): `data` is
    /// still in that codec.
    pub image_filter: Option<Vec<u8>>,
    /// That filter's /DecodeParms, when it had any.
    pub image_parms: Option<Dict>,
}

/// Undo a stream's /Filter chain. `resolve` dereferences an indirect /Filter, /DecodeParms or parameter value.
pub fn decode(stream: &Stream, resolve: &dyn Fn(&Object) -> Object) -> Result<Decoded, Error> {
    let filter = resolve(stream.dict.get(b"Filter").unwrap_or(&Object::Null));
    let parms = resolve(stream.dict.get(b"DecodeParms").unwrap_or(&Object::Null));
    let names: Vec<Vec<u8>> = match &filter {
        Object::Name(n) => vec![n.clone()],
        Object::Array(a) => a.iter().filter_map(|o| resolve(o).as_name().map(|n| n.to_vec())).collect(),
        _ => Vec::new(),
    };
    let parm_of = |i: usize| -> Option<Dict> {
        let p = match &parms {
            Object::Array(a) => a.get(i).map(resolve),
            o if i == 0 => Some(o.clone()),
            _ => None,
        };
        p.and_then(|p| p.as_dict().cloned()).map(|d| {
            let mut out = Dict::new();
            for (k, v) in d.iter() {
                out.insert(k.to_vec(), resolve(v));
            }
            out
        })
    };
    let mut cur = stream.data.clone();
    for (i, name) in names.iter().enumerate() {
        let parm = parm_of(i);
        cur = match name.as_slice() {
            b"FlateDecode" | b"Fl" => predict_with(flate(&cur)?, parm.as_ref())?,
            b"LZWDecode" | b"LZW" => {
                let early = parm.as_ref().and_then(|p| p.int(b"EarlyChange")).unwrap_or(1) != 0;
                predict_with(lzw(&cur, early)?, parm.as_ref())?
            }
            b"ASCIIHexDecode" | b"AHx" => ascii_hex(&cur)?,
            b"ASCII85Decode" | b"A85" => ascii85(&cur)?,
            b"RunLengthDecode" | b"RL" => run_length(&cur)?,
            b"DCTDecode" | b"DCT" | b"JPXDecode" | b"CCITTFaxDecode" | b"CCF" | b"JBIG2Decode" => {
                let full: &[u8] = match name.as_slice() {
                    b"DCT" => b"DCTDecode",
                    b"CCF" => b"CCITTFaxDecode",
                    n => n,
                };
                return Ok(Decoded { data: cur, image_filter: Some(full.to_vec()), image_parms: parm });
            }
            b"Crypt" => cur, // PDFCRYPT (SR315): the security handler decrypted this stream on resolve (crypt.rs)
            _ => return Err(Error::Unsupported("stream filter")),
        };
        if cur.len() > MAX_DECODED {
            return Err(Error::TooLarge);
        }
    }
    Ok(Decoded { data: cur, image_filter: None, image_parms: None })
}

struct SliceSrc<'a> {
    b: &'a [u8],
    i: usize,
}

impl ByteSource for SliceSrc<'_> {
    fn next(&mut self) -> Option<u8> {
        let v = self.b.get(self.i).copied();
        if v.is_some() {
            self.i += 1;
        }
        v
    }
}

struct CapSink<'a> {
    out: &'a mut Vec<u8>,
}

impl Sink for CapSink<'_> {
    fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.out.len() >= MAX_DECODED {
            return Err(());
        }
        self.out.push(byte);
        Ok(())
    }
}

fn settle(r: Result<(), InflateError>, out: Vec<u8>) -> Result<Vec<u8>, Error> {
    match r {
        Ok(()) => Ok(out),
        // The data is whole; only the trailer is wrong or missing, or the stream stops short: keep what was produced.
        Err(InflateError::AdlerMismatch) | Err(InflateError::TruncatedInput) => Ok(out),
        Err(InflateError::SinkRejected) => Err(Error::TooLarge),
        Err(e) => Err(Error::Inflate(inflate::inflate_reason(e))),
    }
}

/// FlateDecode without predictor: zlib (RFC 1950), or a bare DEFLATE stream when the zlib header is refused.
pub fn flate(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let r = inflate::zlib_inflate(&mut SliceSrc { b: data, i: 0 }, &mut CapSink { out: &mut out }).map(|_| ());
    if r == Err(InflateError::BadZlibHeader) {
        let mut raw = Vec::new();
        let r = inflate::inflate_raw(&mut SliceSrc { b: data, i: 0 }, &mut CapSink { out: &mut raw }).map(|_| ());
        return settle(r, raw);
    }
    settle(r, out)
}

fn predict_with(data: Vec<u8>, parm: Option<&Dict>) -> Result<Vec<u8>, Error> {
    let Some(p) = parm else { return Ok(data) };
    let predictor = p.int(b"Predictor").unwrap_or(1);
    if predictor <= 1 {
        return Ok(data);
    }
    let colors = p.int(b"Colors").unwrap_or(1);
    let bpc = p.int(b"BitsPerComponent").unwrap_or(8);
    let columns = p.int(b"Columns").unwrap_or(1);
    if !(1..=32).contains(&colors) || !matches!(bpc, 1 | 2 | 4 | 8 | 16) || !(1..=1 << 24).contains(&columns) {
        return Err(Error::Syntax("predictor parameters", 0));
    }
    predict(&data, predictor, colors as usize, bpc as usize, columns as usize)
}

/// §7.4.4.4: undo predictor `predictor` (1 none, 2 TIFF, 10-15 PNG with a per-row filter-type byte).
pub fn predict(data: &[u8], predictor: i64, colors: usize, bpc: usize, columns: usize) -> Result<Vec<u8>, Error> {
    let row = (colors * bpc * columns).div_ceil(8);
    match predictor {
        1 => Ok(data.to_vec()),
        2 => Ok(tiff(data, row, colors, bpc)),
        10..=15 => png(data, row, (colors * bpc).div_ceil(8).max(1)),
        _ => Err(Error::Unsupported("predictor")),
    }
}

fn tiff(data: &[u8], row: usize, colors: usize, bpc: usize) -> Vec<u8> {
    let mut out = data.to_vec();
    if row == 0 {
        return out;
    }
    for line in out.chunks_mut(row) {
        match bpc {
            8 => {
                for i in colors..line.len() {
                    line[i] = line[i].wrapping_add(line[i - colors]);
                }
            }
            16 => {
                let step = colors * 2;
                let mut i = step;
                while i + 1 < line.len() {
                    let a = u16::from_be_bytes([line[i], line[i + 1]]);
                    let b = u16::from_be_bytes([line[i - step], line[i - step + 1]]);
                    let v = a.wrapping_add(b).to_be_bytes();
                    line[i] = v[0];
                    line[i + 1] = v[1];
                    i += 2;
                }
            }
            _ => {
                // 1, 2 or 4 bits per sample, packed MSB-first.
                let mask = (1u16 << bpc) - 1;
                let samples = line.len() * 8 / bpc;
                let get = |l: &[u8], s: usize| -> u16 {
                    let bit = s * bpc;
                    ((l[bit / 8] >> (8 - bpc - bit % 8)) as u16) & mask
                };
                for s in colors..samples {
                    let v = (get(line, s) + get(line, s - colors)) & mask;
                    let bit = s * bpc;
                    let shift = 8 - bpc - bit % 8;
                    line[bit / 8] = (line[bit / 8] & !((mask as u8) << shift)) | ((v as u8) << shift);
                }
            }
        }
    }
    out
}

/// PNG predictors 10-15 through pixel_core's ONE unfilter. PDF policy around it: an unknown filter type reads as None
/// (remapped to 0 first), a short trailing row is unfiltered and kept (the shared fn's last-row rule), and the
/// filter-type bytes are dropped from the output.
fn png(data: &[u8], row: usize, bpp: usize) -> Result<Vec<u8>, Error> {
    let stride = row + 1;
    let mut buf = data.to_vec();
    for i in (0..buf.len()).step_by(stride) {
        if buf[i] > 4 {
            buf[i] = 0;
        }
    }
    pixel_core::png::unfilter_rows(bpp, stride, &mut buf).map_err(|_| Error::Unsupported("predictor"))?;
    let mut w = 0usize;
    for i in 0..buf.len() {
        if i % stride != 0 {
            buf[w] = buf[i];
            w += 1;
        }
    }
    buf.truncate(w);
    Ok(buf)
}

/// §7.4.4.2 LZWDecode: MSB-first codes of 9 to 12 bits, 256 = clear, 257 = end of data. With `early` the code width
/// grows one code early (the default /EarlyChange 1).
pub fn lzw(data: &[u8], early: bool) -> Result<Vec<u8>, Error> {
    #[derive(Clone, Copy)]
    struct E {
        prefix: u16,
        byte: u8,
        first: u8,
        len: u32,
    }
    const NONE: u16 = u16::MAX;
    let e = early as u32;
    let mut table: Vec<E> = (0..258u32)
        .map(|i| E { prefix: NONE, byte: i as u8, first: i as u8, len: if i < 256 { 1 } else { 0 } })
        .collect();
    let mut out: Vec<u8> = Vec::new();
    let (mut acc, mut nbits, mut pos) = (0u32, 0u32, 0usize);
    let mut width = 9u32;
    let mut prev: Option<u16> = None;
    loop {
        while nbits < width {
            let Some(&b) = data.get(pos) else { return Ok(out) };
            pos += 1;
            acc = (acc << 8) | b as u32;
            nbits += 8;
        }
        let code = (acc >> (nbits - width)) & ((1 << width) - 1);
        nbits -= width;
        acc &= (1u32 << nbits) - 1;
        if code == 256 {
            table.truncate(258);
            width = 9;
            prev = None;
            continue;
        }
        if code == 257 {
            break;
        }
        let next = table.len() as u32;
        let entry = if code < next && table[code as usize].len > 0 {
            table[code as usize]
        } else if code == next && prev.is_some() {
            // KwKwK: the code being defined, = prev + prev's first byte.
            let p = table[prev.unwrap() as usize];
            E { prefix: prev.unwrap(), byte: p.first, first: p.first, len: p.len + 1 }
        } else {
            return Err(Error::Syntax("lzw code out of range", pos));
        };
        if let Some(p) = prev {
            if next < 4096 {
                let pe = table[p as usize];
                table.push(E { prefix: p, byte: entry.first, first: pe.first, len: pe.len + 1 });
            }
        }
        // Emit the sequence by walking its prefix chain backwards.
        let start = out.len();
        let len = entry.len as usize;
        if start + len > MAX_DECODED {
            return Err(Error::TooLarge);
        }
        out.resize(start + len, 0);
        let mut c = if code < next { code as u16 } else { (table.len() - 1) as u16 };
        for k in (0..len).rev() {
            let t = table[c as usize];
            out[start + k] = t.byte;
            c = t.prefix;
        }
        prev = Some(if code < next { code as u16 } else { (table.len() - 1) as u16 });
        let n = table.len() as u32 + e;
        width = if n >= 2048 { 12 } else if n >= 1024 { 11 } else if n >= 512 { 10 } else { 9 };
    }
    Ok(out)
}

/// §7.4.2 ASCIIHexDecode: `>` ends the data, white space is skipped, an odd final digit is followed by a 0.
pub fn ascii_hex(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut hi: Option<u8> = None;
    for (i, &c) in data.iter().enumerate() {
        let v = match c {
            b'>' => break,
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            c if crate::lexer::is_white(c) => continue,
            _ => return Err(Error::Syntax("ASCIIHexDecode digit", i)),
        };
        match hi.take() {
            Some(h) => out.push(h << 4 | v),
            None => hi = Some(v),
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    Ok(out)
}

/// §7.4.3 ASCII85Decode: `z` is four zero bytes, `~>` ends the data, a final partial group of n characters is n-1
/// bytes. A leading `<~` is tolerated.
pub fn ascii85(data: &[u8]) -> Result<Vec<u8>, Error> {
    let body = if data.starts_with(b"<~") { &data[2..] } else { data };
    let mut out = Vec::with_capacity(body.len() * 4 / 5);
    let mut group = [0u8; 5];
    let mut n = 0usize;
    for (i, &c) in body.iter().enumerate() {
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                group[n] = c - b'!';
                n += 1;
                if n == 5 {
                    let v = group.iter().fold(0u64, |acc, &d| acc * 85 + d as u64);
                    if v > u32::MAX as u64 {
                        return Err(Error::Syntax("ASCII85Decode group overflow", i));
                    }
                    out.extend_from_slice(&(v as u32).to_be_bytes());
                    n = 0;
                }
            }
            c if crate::lexer::is_white(c) => {}
            _ => return Err(Error::Syntax("ASCII85Decode character", i)),
        }
    }
    if n > 1 {
        for slot in group.iter_mut().skip(n) {
            *slot = 84;
        }
        let v = group.iter().fold(0u64, |acc, &d| acc * 85 + d as u64).min(u32::MAX as u64);
        out.extend_from_slice(&(v as u32).to_be_bytes()[..n - 1]);
    }
    Ok(out)
}

/// §7.4.5 RunLengthDecode: length byte 0-127 copies the next n+1 bytes, 129-255 repeats the next byte 257-n times,
/// 128 ends the data.
pub fn run_length(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&l) = data.get(i) {
        i += 1;
        if l == 128 {
            break;
        }
        if l < 128 {
            let n = l as usize + 1;
            let end = (i + n).min(data.len());
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else {
            let Some(&b) = data.get(i) else { break };
            i += 1;
            out.extend(core::iter::repeat_n(b, 257 - l as usize));
        }
        if out.len() > MAX_DECODED {
            return Err(Error::TooLarge);
        }
    }
    Ok(out)
}

/// CCITTFaxDecode and JBIG2Decode (PDFFAX SR331): `dec` stopped at one of them (its `image_filter`); its bytes become
/// packed 1-bit rows `w` wide (`w.div_ceil(8)` bytes a row), `h` rows, cropped or white-padded, in the PDF sample
/// sense (§7.4.6-7.4.7: black is 0, or 1 under CCITT /BlackIs1 true). The /JBIG2Globals stream is undone by
/// [`decode`] itself; `resolve` dereferences its entries.
pub fn bilevel(dec: &Decoded, resolve: &dyn Fn(&Object) -> Object, w: usize, h: usize) -> Result<Vec<u8>, Error> {
    let parms = dec.image_parms.as_ref();
    let int = |k: &[u8]| parms.and_then(|p| p.get(k)).map(resolve).and_then(|o| o.as_int());
    let flag = |k: &[u8], dflt: bool| match parms.and_then(|p| p.get(k)).map(resolve) {
        Some(Object::Bool(b)) => b,
        _ => dflt,
    };
    let (bits, bw, white) = match dec.image_filter.as_deref() {
        Some(b"CCITTFaxDecode") => {
            let p = ccitt::Params {
                k: int(b"K").unwrap_or(0),
                columns: int(b"Columns").filter(|&c| c > 0).map_or(1728, |c| c as usize),
                rows: int(b"Rows").filter(|&r| r > 0).map_or(0, |r| r as usize),
                black_is_1: flag(b"BlackIs1", false),
                byte_align: flag(b"EncodedByteAlign", false),
                end_of_line: flag(b"EndOfLine", false),
                end_of_block: flag(b"EndOfBlock", true),
            };
            (ccitt::decode(&dec.data, &p)?, p.columns, !p.black_is_1)
        }
        Some(b"JBIG2Decode") => {
            let globals = match parms.and_then(|p| p.get(b"JBIG2Globals")).map(resolve) {
                Some(Object::Stream(g)) => Some(decode(&g, resolve)?.data),
                _ => None,
            };
            let bm = jbig2::decode(&dec.data, globals.as_deref())?;
            (bm.pack(true), bm.w, true)
        }
        _ => return Err(Error::Unsupported("image codec")),
    };
    let (src, dst) = (bw.div_ceil(8), w.div_ceil(8));
    if dst.saturating_mul(h) > MAX_DECODED {
        return Err(Error::TooLarge);
    }
    let mut out = vec![if white { 0xFF } else { 0 }; dst * h];
    let have = if src == 0 { 0 } else { (bits.len() / src).min(h) };
    for y in 0..have {
        let from = &bits[y * src..(y + 1) * src];
        if bw == w {
            out[y * dst..(y + 1) * dst].copy_from_slice(from);
            continue;
        }
        for x in 0..w.min(bw) {
            let m = 0x80u8 >> (x % 8);
            let o = &mut out[y * dst + x / 8];
            if from[x / 8] & m != 0 { *o |= m } else { *o &= !m }
        }
    }
    Ok(out)
}
