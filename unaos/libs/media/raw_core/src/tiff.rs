// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The TIFF container (TIFF 6.0 §2): the header, IFDs, typed values — and the walk that finds a raw file's
//! three things (the raw strip, the embedded preview, the facts). Every read is checked against the slice.

use alloc::string::String;
use alloc::vec::Vec;

use crate::exif::{self, Facts};
use crate::{Error, MAX_DIM, MAX_IFDS, MAX_PIXELS};

/// TIFF tags this core reads (TIFF 6.0, TIFF-EP, EXIF 2.32, and Sony's private raw-IFD tags).
pub mod tag {
    pub const WIDTH: u16 = 256;
    pub const HEIGHT: u16 = 257;
    pub const BITS: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const MAKE: u16 = 271;
    pub const MODEL: u16 = 272;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const ORIENTATION: u16 = 274;
    pub const STRIP_BYTES: u16 = 279;
    pub const SUB_IFDS: u16 = 330;
    pub const JPEG_OFFSET: u16 = 513;
    pub const JPEG_LENGTH: u16 = 514;
    pub const CFA_DIM: u16 = 33421;
    pub const CFA_PATTERN: u16 = 33422;
    pub const EXIF_IFD: u16 = 34665;
    /// Sony: the cRAW tone curve's four knees (dcraw `sony_curve`).
    pub const SONY_CURVE: u16 = 28688;
    /// Sony: BlackLevel, four shorts (ARW 2.3.1 and later).
    pub const SONY_BLACK: u16 = 0x7310;
    /// Sony: WhiteLevel, one to three shorts.
    pub const SONY_WHITE: u16 = 0x787f;
    /// DNG ColorMatrix2: XYZ -> camera under the second calibration illuminant (D65), nine SRATIONALs (B724).
    pub const COLOR_MATRIX2: u16 = 50722;
    /// Canon CR2: the raw strip's slice table `[n, sw, slast]` (B724).
    pub const CR2_SLICE: u16 = 0xC640;
}

/// Which raw format a TIFF container is (B724): named by its header (`CR\x02` at byte 8: Canon CR2) or IFD0's Make.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fmt {
    #[default]
    Tiff,
    Arw,
    Cr2,
    Nef,
}

impl Fmt {
    /// The wire word: `tiff`, `arw`, `cr2`, `nef`.
    pub fn name(self) -> &'static str {
        match self {
            Fmt::Tiff => "tiff",
            Fmt::Arw => "arw",
            Fmt::Cr2 => "cr2",
            Fmt::Nef => "nef",
        }
    }
}

/// `PhotometricInterpretation` of a colour-filter mosaic (TIFF-EP).
pub const PHOTOMETRIC_CFA: u32 = 32803;

/// One IFD entry: tag, type, count and where its 4-byte value field sits.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    at: usize,
}

/// The byte-order-aware view of the file.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    pub b: &'a [u8],
    pub le: bool,
}

fn type_size(t: u16) -> Option<u64> {
    Some(match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

impl<'a> Reader<'a> {
    pub fn u16(&self, i: usize) -> Option<u16> {
        let s = self.b.get(i..i.checked_add(2)?)?;
        Some(if self.le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }
    pub fn u32(&self, i: usize) -> Option<u32> {
        let s = self.b.get(i..i.checked_add(4)?)?;
        let a = [s[0], s[1], s[2], s[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }

    /// The entries of the IFD at `off` and its next-IFD pointer.
    pub fn ifd(&self, off: usize) -> Result<(Vec<Entry>, u32), Error> {
        let n = self.u16(off).ok_or(Error::Truncated)? as usize;
        let end = off + 2 + n * 12;
        if end + 4 > self.b.len() {
            return Err(Error::Truncated);
        }
        let mut v = Vec::new();
        v.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
        for i in 0..n {
            let e = off + 2 + i * 12;
            v.push(Entry { tag: self.u16(e).unwrap_or(0), typ: self.u16(e + 2).unwrap_or(0), count: self.u32(e + 4).unwrap_or(0), at: e + 8 });
        }
        Ok((v, self.u32(end).unwrap_or(0)))
    }

    /// Where `e`'s value bytes are: inline in the entry (four bytes or fewer) or at the offset it holds.
    /// `None` when the type is unknown or the bytes lie outside the slice.
    pub fn value_span(&self, e: &Entry) -> Option<(usize, usize)> {
        let len = type_size(e.typ)?.checked_mul(e.count as u64)?;
        let start = if len <= 4 { e.at } else { self.u32(e.at)? as usize };
        let len = usize::try_from(len).ok()?;
        let end = start.checked_add(len)?;
        (end <= self.b.len()).then_some((start, len))
    }

    /// Element `i` of an integer-typed entry (BYTE, SHORT, LONG, IFD; UNDEFINED read as bytes).
    pub fn uint(&self, e: &Entry, i: u32) -> Option<u32> {
        if i >= e.count {
            return None;
        }
        let (s, _) = self.value_span(e)?;
        match e.typ {
            1 | 7 => self.b.get(s + i as usize).map(|&x| x as u32),
            3 => self.u16(s + 2 * i as usize).map(|x| x as u32),
            4 | 13 => self.u32(s + 4 * i as usize),
            _ => None,
        }
    }

    /// A RATIONAL (or SRATIONAL read as unsigned) entry's first value.
    pub fn rational(&self, e: &Entry) -> Option<(u32, u32)> {
        if !matches!(e.typ, 5 | 10) || e.count == 0 {
            return None;
        }
        let (s, _) = self.value_span(e)?;
        Some((self.u32(s)?, self.u32(s + 4)?))
    }

    /// An ASCII entry up to its first NUL, trimmed. Bytes outside ASCII are dropped.
    pub fn ascii(&self, e: &Entry) -> Option<String> {
        if e.typ != 2 {
            return None;
        }
        let (s, l) = self.value_span(e)?;
        let raw = &self.b[s..s + l];
        let raw = raw.split(|&c| c == 0).next().unwrap_or(&[]);
        let t: String = raw.iter().filter(|c| c.is_ascii() && !c.is_ascii_control()).map(|&c| c as char).collect();
        let t = String::from(t.trim());
        (!t.is_empty()).then_some(t)
    }
}

/// The raw strip: where the mosaic is and how it is coded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Strip {
    pub width: u32,
    pub height: u32,
    pub bits: u16,
    /// 1 = uncompressed (16-bit containers, or packed); 32767 = Sony (`bits` 8: cRAW); 6 = Canon CR2's lossless
    /// JPEG; 34713 = Nikon NEF's Huffman coding.
    pub compression: u16,
    /// File offset and byte length of the (contiguous) strip data.
    pub offset: u64,
    pub len: u64,
    /// The 2x2 colour-filter pattern, row-major: 0 red, 1 green, 2 blue (TIFF-EP CFAPattern).
    pub cfa: [u8; 4],
    /// Black and white levels in decoded sample units (0 = not named in the file).
    pub black: u16,
    pub white: u16,
    /// Sony's cRAW curve knees (tag 28688), when present.
    pub curve: Option<[u16; 4]>,
    /// The file's byte order (uncompressed samples are stored in it).
    pub le: bool,
    /// Canon CR2's slice table (tag 0xC640), when present (B724).
    pub slices: Option<[u16; 3]>,
    /// Nikon NEF's maker-note decoding data (tag 0x96), for `Compression 34713` (B724).
    pub nikon: Option<crate::nef::Meta>,
}

/// What [`parse`] learned from a raw file's head.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawInfo {
    pub le: bool,
    /// IFDs visited (IFD0's chain, SubIFDs, the EXIF IFD).
    pub ifds: u32,
    pub strip: Option<Strip>,
    /// The largest embedded JPEG: file offset and length.
    pub preview: Option<(u64, u64)>,
    pub orientation: u8,
    pub facts: Facts,
    /// The as-shot white balance (AsShotNeutral, else Sony's SR2 levels) — RAWVIEW (B711), [`crate::wb`].
    pub wb: Option<crate::wb::Wb>,
    /// The raw format (B724).
    pub fmt: Fmt,
    /// Camera RGB -> linear sRGB (D65), Q12, each row summing to 4096 — RAWCOLOR (B724), [`crate::matrix`].
    pub matrix: Option<[[i32; 3]; 3]>,
    pub matrix_source: Option<crate::matrix::MatrixSource>,
}

#[derive(Default)]
struct Ifd {
    width: u32,
    height: u32,
    bits: u16,
    compression: u16,
    photometric: u32,
    strip: Option<(u64, u64)>,
    strip_err: Option<Error>,
    jpeg: (Option<u32>, Option<u32>),
    cfa: Option<[u8; 4]>,
    curve: Option<[u16; 4]>,
    black: Option<u16>,
    white: Option<u16>,
    slices: Option<[u16; 3]>,
    cm2: Option<[(i32, i32); 9]>,
    exif: Option<u32>,
}

/// The strip extent named by StripOffsets/StripByteCounts: one span, or several that are contiguous.
fn strip_span(r: &Reader, off: &Entry, cnt: &Entry) -> Result<(u64, u64), Error> {
    if off.count == 0 || off.count != cnt.count || off.count > 65536 {
        return Err(Error::Malformed("strip tables"));
    }
    let first = r.uint(off, 0).ok_or(Error::Truncated)? as u64;
    let mut end = first;
    for i in 0..off.count {
        let (o, c) = (r.uint(off, i).ok_or(Error::Truncated)? as u64, r.uint(cnt, i).ok_or(Error::Truncated)? as u64);
        if o != end {
            return Err(Error::Unsupported("non-contiguous strips"));
        }
        end = o + c;
    }
    Ok((first, end - first))
}

fn read_ifd(r: &Reader, ents: &[Entry], info: &mut RawInfo, queue: &mut Vec<u32>, ifd0: bool) -> Ifd {
    let mut d = Ifd { bits: 0, ..Default::default() };
    let find = |t: u16| ents.iter().find(|e| e.tag == t);
    for e in ents {
        let u = |i| r.uint(e, i);
        match e.tag {
            tag::WIDTH => d.width = u(0).unwrap_or(0),
            tag::HEIGHT => d.height = u(0).unwrap_or(0),
            tag::BITS => d.bits = u(0).unwrap_or(0) as u16,
            tag::COMPRESSION => d.compression = u(0).unwrap_or(0) as u16,
            tag::PHOTOMETRIC => d.photometric = u(0).unwrap_or(0),
            tag::JPEG_OFFSET => d.jpeg.0 = u(0),
            tag::JPEG_LENGTH => d.jpeg.1 = u(0),
            tag::SUB_IFDS => {
                for i in 0..e.count.min(8) {
                    if let Some(o) = u(i) {
                        queue.push(o);
                    }
                }
            }
            tag::CFA_PATTERN if e.count == 4 => {
                let p = [u(0), u(1), u(2), u(3)];
                if p.iter().all(|c| matches!(c, Some(0..=2))) {
                    d.cfa = Some(p.map(|c| c.unwrap_or(1) as u8));
                }
            }
            tag::SONY_CURVE if e.count >= 4 => {
                d.curve = Some([0, 1, 2, 3].map(|i| u(i).map(|v| ((v >> 2) & 0xfff) as u16).unwrap_or(0)));
            }
            tag::SONY_BLACK => {
                let v: Vec<u32> = (0..e.count.min(4)).filter_map(|i| u(i)).collect();
                if !v.is_empty() {
                    d.black = Some((v.iter().sum::<u32>() / v.len() as u32) as u16);
                }
            }
            tag::SONY_WHITE => d.white = u(0).map(|v| v as u16),
            tag::CR2_SLICE if e.count >= 3 => {
                if let (Some(a), Some(b), Some(c)) = (u(0), u(1), u(2)) {
                    d.slices = Some([a as u16, b as u16, c as u16]);
                }
            }
            tag::COLOR_MATRIX2 if e.count == 9 && matches!(e.typ, 5 | 10) => {
                if let Some((s, _)) = r.value_span(e) {
                    let mut m = [(0i32, 0i32); 9];
                    let ok = (0..9).all(|i| match (r.u32(s + 8 * i), r.u32(s + 8 * i + 4)) {
                        (Some(n), Some(dd)) => {
                            m[i] = if e.typ == 10 { (n as i32, dd as i32) } else { (n.min(i32::MAX as u32) as i32, dd.min(i32::MAX as u32) as i32) };
                            true
                        }
                        _ => false,
                    });
                    if ok {
                        d.cm2 = Some(m);
                    }
                }
            }
            // RAWVIEW (B711): AsShotNeutral outranks Sony's levels wherever it is met.
            crate::wb::tag::AS_SHOT_NEUTRAL => {
                if let Some(w) = crate::wb::neutral(r, e).and_then(crate::wb::Wb::from_neutral) {
                    info.wb = Some(w);
                }
            }
            _ => {}
        }
        if ifd0 {
            match e.tag {
                tag::MAKE => info.facts.make = r.ascii(e),
                tag::MODEL => info.facts.model = r.ascii(e),
                tag::ORIENTATION => info.orientation = u(0).unwrap_or(1).clamp(1, 8) as u8,
                tag::EXIF_IFD => {
                    if let Some(o) = u(0) {
                        info.ifds += exif::read(r, o as usize, &mut info.facts) as u32;
                        d.exif = Some(o);
                    }
                }
                crate::wb::tag::DNG_PRIVATE if info.wb.is_none() => info.wb = crate::wb::sony_sr2(r, e),
                _ => {}
            }
        }
    }
    if let (Some(o), Some(c)) = (find(tag::STRIP_OFFSETS), find(tag::STRIP_BYTES)) {
        match strip_span(r, o, c) {
            Ok(s) => d.strip = Some(s),
            Err(e) => d.strip_err = Some(e),
        }
    }
    d
}

/// Parse a TIFF/raw file's head. `head` may be the whole file or just its first bytes: IFDs must lie inside it;
/// the strip and the preview are named (offset, length) and checked by whoever reads them. Pure.
pub fn parse(head: &[u8]) -> Result<RawInfo, Error> {
    if !crate::is_tiff(head) {
        return Err(if head.len() < 8 { Error::Truncated } else { Error::NotTiff });
    }
    let r = Reader { b: head, le: head[0] == b'I' };
    let mut info = RawInfo { le: r.le, orientation: 1, ..Default::default() };
    let ifd0 = r.u32(4).ok_or(Error::Truncated)?;
    if ifd0 < 8 {
        return Err(Error::Malformed("IFD0 offset"));
    }
    let mut queue: Vec<u32> = alloc::vec![ifd0];
    let mut seen: Vec<u32> = Vec::new();
    let mut best: Option<(Ifd, bool)> = None; // (ifd, is a CFA)
    let mut fence: Option<Error> = None;
    let mut chain_next = Some(ifd0);
    let mut qi = 0;
    let mut ifd0_dims: Option<(u32, u32)> = None;
    // RAWCOLOR (B724): a CR2 names itself in the header; its raw is IFD3 of IFD0's chain, its big JPEG IFD0's strip.
    let is_cr2 = head.get(8..11) == Some(b"CR\x02");
    let mut chain: Vec<u32> = Vec::new();
    let mut cm2: Option<[(i32, i32); 9]> = None;
    let mut exif_at: Option<u32> = None;
    while qi < queue.len() && seen.len() < MAX_IFDS {
        let off = queue[qi];
        qi += 1;
        if seen.contains(&off) {
            continue;
        }
        seen.push(off);
        let (ents, next) = match r.ifd(off as usize) {
            Ok(x) => x,
            Err(e) if off == ifd0 => return Err(e),
            Err(_) => continue,
        };
        info.ifds += 1;
        // IFD0's chain (IFD1, …) is followed; SubIFDs' own next pointers are not part of the walk.
        if chain_next == Some(off) {
            chain.push(off);
            chain_next = (next != 0).then_some(next);
            if let Some(n) = chain_next {
                queue.push(n);
            }
        }
        let mut d = read_ifd(&r, &ents, &mut info, &mut queue, off == ifd0);
        if off == ifd0 && d.width > 0 && d.height > 0 {
            ifd0_dims = Some((d.width, d.height));
        }
        if cm2.is_none() {
            cm2 = d.cm2;
        }
        if d.exif.is_some() {
            exif_at = d.exif;
        }
        let chain_idx = chain.iter().position(|&c| c == off);
        if is_cr2 && chain_idx == Some(0) && d.compression == 6 {
            // CR2's full-size JPEG is IFD0's strip, not a 513/514 pair.
            if let Some((o, l)) = d.strip {
                if l > 2 && info.preview.map(|(_, pl)| l > pl).unwrap_or(true) && head.get(o as usize..o as usize + 2).map(|s| s == [0xFF, 0xD8]).unwrap_or(true) {
                    info.preview = Some((o, l));
                }
            }
        }
        let cr2_raw = is_cr2 && d.compression == 6 && d.strip.is_some() && (d.slices.is_some() || chain_idx == Some(3));
        if cr2_raw && (d.width == 0 || d.height == 0) {
            // The geometry is the lossless JPEG's SOF3 — read here when the strip lies in the head, else by the decoder.
            if let Some((o, _)) = d.strip {
                if let Some(f) = head.get(o as usize..).and_then(|b| crate::cr2::frame(b).ok()) {
                    let (w, h) = f.dims();
                    (d.width, d.height, d.bits) = (w, h, f.bits as u16);
                }
            }
        }
        if let (Some(o), Some(l)) = d.jpeg {
            if l > 2 && info.preview.map(|(_, pl)| (l as u64) > pl).unwrap_or(true) && head.get(o as usize..o as usize + 2).map(|s| s == [0xFF, 0xD8]).unwrap_or(true) {
                info.preview = Some((o as u64, l as u64));
            }
        }
        let cfa = d.photometric == PHOTOMETRIC_CFA;
        let rawish = cfa || cr2_raw || (d.photometric == 0 && d.compression == 32767);
        if rawish && (d.strip.is_some() || d.strip_err.is_some()) {
            let px = d.width as u64 * d.height as u64;
            let unknown = cr2_raw && px == 0; // a CR2 whose strip is past the head: the decoder fences it
            if !unknown && (d.width == 0 || d.height == 0 || d.width > MAX_DIM || d.height > MAX_DIM || px > MAX_PIXELS) {
                fence = Some(Error::TooLarge);
                continue;
            }
            let pri = cfa || cr2_raw;
            let better = match &best {
                None => true,
                Some((b, bc)) => (pri && !bc) || (pri == *bc && px > b.width as u64 * b.height as u64),
            };
            if better {
                best = Some((d, pri));
            }
        }
    }
    if let Some((d, _)) = best {
        if let Some(e) = d.strip_err {
            return Err(e);
        }
        let (offset, len) = d.strip.unwrap_or((0, 0));
        info.strip = Some(Strip {
            width: d.width,
            height: d.height,
            bits: if d.bits == 0 { 16 } else { d.bits },
            compression: if d.compression == 0 { 1 } else { d.compression },
            offset,
            len,
            cfa: d.cfa.unwrap_or([0, 1, 1, 2]),
            black: d.black.unwrap_or(0),
            white: d.white.unwrap_or(0),
            curve: d.curve,
            le: r.le,
            slices: d.slices,
            nikon: None,
        });
        if d.width > 0 && d.height > 0 {
            info.facts.width = Some(d.width);
            info.facts.height = Some(d.height);
        }
    } else if let Some(e) = fence {
        return Err(e);
    } else if let Some((w, h)) = ifd0_dims {
        info.facts.width = Some(w);
        info.facts.height = Some(h);
    }
    // RAWCOLOR (B724): the format, Nikon's maker-note decoding data, the colour matrix.
    let make = info.facts.make.as_deref().map(str::trim).unwrap_or("");
    info.fmt = if is_cr2 {
        Fmt::Cr2
    } else if make.eq_ignore_ascii_case("SONY") {
        Fmt::Arw
    } else if make.len() >= 5 && make[..5].eq_ignore_ascii_case("NIKON") {
        Fmt::Nef
    } else {
        Fmt::Tiff
    };
    if let (Fmt::Nef, Some(s), Some(x)) = (info.fmt, info.strip.as_mut(), exif_at) {
        let (m, black) = crate::nef::maker(&r, x as usize, s.bits);
        if s.compression == 34713 {
            s.nikon = m;
        }
        if s.black == 0 {
            s.black = black.unwrap_or(0);
        }
    }
    if let Some(m) = cm2.as_ref().and_then(crate::matrix::from_rationals) {
        (info.matrix, info.matrix_source) = (Some(m), Some(crate::matrix::MatrixSource::Dng));
    } else if let Some(m) = crate::matrix::table(info.facts.make.as_deref(), info.facts.model.as_deref()).as_ref().and_then(crate::matrix::from_table) {
        (info.matrix, info.matrix_source) = (Some(m), Some(crate::matrix::MatrixSource::Table));
    }
    Ok(info)
}

impl RawInfo {
    /// The preview's byte range, checked against a file of `file_len` bytes.
    pub fn preview_in(&self, file_len: u64) -> Option<(u64, u64)> {
        self.preview.filter(|&(o, l)| o.checked_add(l).map(|e| e <= file_len).unwrap_or(false))
    }
}
