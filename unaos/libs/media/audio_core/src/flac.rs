//! FLAC, RFC 9639: STREAMINFO (§8.2), the frame header with its UTF-8-coded number and CRC-8 (§9.1),
//! CONSTANT / VERBATIM / FIXED / LPC subframes with wasted bits (§9.2), Rice-partitioned residuals with
//! both parameter widths and the escape code (§9.2.7), the four channel assignments (§4.2), the frame
//! CRC-16 (§9.3) and — the correctness proof — the MD5 of the decoded stream checked against STREAMINFO
//! at the end of the stream (§8.2). 4–32-bit samples; the 33-bit side channel of 32-bit audio is carried
//! in `i64`. The same [`FrameDecoder`] serves native FLAC and the Ogg mapping (see [`crate::ogg`]).
use crate::bits::BitReader;
use crate::crc::{crc16, crc8};
use crate::io::ByteStream;
use crate::md5::Md5;
use crate::{Codec, Error, Format, Info, Pcm, Result, Source};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamInfo {
    pub min_block: u32,
    pub max_block: u32,
    pub min_frame: u32,
    pub max_frame: u32,
    pub rate: u32,
    pub channels: u32,
    pub bps: u32,
    pub total: u64,
    pub md5: [u8; 16],
}

impl StreamInfo {
    /// Parse the 34-byte STREAMINFO body (RFC 9639 §8.2).
    pub fn parse(b: &[u8]) -> Result<StreamInfo> {
        if b.len() < 34 { return Err(Error::Invalid("short STREAMINFO")); }
        let mut r = BitReader::new(b);
        let si = StreamInfo {
            min_block: r.read(16)?,
            max_block: r.read(16)?,
            min_frame: r.read(24)?,
            max_frame: r.read(24)?,
            rate: r.read(20)?,
            channels: r.read(3)? + 1,
            bps: r.read(5)? + 1,
            total: r.read64(36)?,
            md5: b[18..34].try_into().unwrap(),
        };
        if si.bps < 4 { return Err(Error::Invalid("STREAMINFO bits per sample < 4")); }
        Ok(si)
    }
}

/// Decodes single frames; owns the per-channel work buffers and the running MD5.
pub struct FrameDecoder {
    pub si: StreamInfo,
    work: Vec<Vec<i64>>,
    md5: Md5,
    md5_bytes: Vec<u8>,
    pub samples: u64,
    pub frames: u64,
}

fn utf8_number(r: &mut BitReader) -> Result<u64> {
    let b = r.read(8)?;
    let n = (b as u8).leading_ones();
    if n == 0 { return Ok(b as u64); }
    if n == 1 || n > 7 { return Err(Error::Invalid("frame number coding")); }
    let mut v = (b & (0x7F >> n)) as u64;
    for _ in 1..n {
        let c = r.read(8)?;
        if c & 0xC0 != 0x80 { return Err(Error::Invalid("frame number continuation")); }
        v = (v << 6) | (c & 0x3F) as u64;
    }
    Ok(v)
}

/// Header facts that decide where a frame belongs.
#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub block: usize,
    pub rate: u32,
    pub chan_code: u32,
    pub channels: usize,
    pub bps: u32,
    pub number: u64,
    pub variable: bool,
}

fn parse_header(r: &mut BitReader, d: &[u8], si: &StreamInfo) -> Result<FrameHeader> {
    if r.read(15)? != 0x7FFC { return Err(Error::Invalid("frame sync")); }
    let variable = r.bit()?;
    let bs = r.read(4)?;
    let sr = r.read(4)?;
    let chan_code = r.read(4)?;
    let ss = r.read(3)?;
    if r.bit()? { return Err(Error::Invalid("frame header reserved bit")); }
    let number = utf8_number(r)?;
    let block = match bs {
        0 => return Err(Error::Invalid("block size code 0")),
        1 => 192,
        2..=5 => 576 << (bs - 2),
        6 => r.read(8)? as usize + 1,
        7 => r.read(16)? as usize + 1,
        _ => 256 << (bs - 8),
    };
    let rate = match sr {
        0 => si.rate,
        1 => 88200,
        2 => 176400,
        3 => 192000,
        4 => 8000,
        5 => 16000,
        6 => 22050,
        7 => 24000,
        8 => 32000,
        9 => 44100,
        10 => 48000,
        11 => 96000,
        12 => r.read(8)? * 1000,
        13 => r.read(16)?,
        14 => r.read(16)? * 10,
        _ => return Err(Error::Invalid("sample rate code 15")),
    };
    let channels = match chan_code {
        0..=7 => chan_code as usize + 1,
        8..=10 => 2,
        _ => return Err(Error::Invalid("reserved channel assignment")),
    };
    let bps = match ss {
        0 => si.bps,
        1 => 8,
        2 => 12,
        3 => return Err(Error::Invalid("reserved sample size code")),
        4 => 16,
        5 => 20,
        6 => 24,
        _ => 32,
    };
    let hlen = r.bit_pos() / 8;
    let c = r.read(8)?;
    if crc8(&d[..hlen]) as u32 != c { return Err(Error::Checksum("FLAC frame header CRC-8")); }
    Ok(FrameHeader { block, rate, chan_code, channels, bps, number, variable })
}

fn residual(r: &mut BitReader, out: &mut [i64], order: usize) -> Result<()> {
    let n = out.len();
    let method = r.read(2)?;
    if method > 1 { return Err(Error::Invalid("reserved residual coding method")); }
    let (pbits, esc) = if method == 0 { (4, 15) } else { (5, 31) };
    let porder = r.read(4)?;
    let parts = 1usize << porder;
    if n % parts != 0 || (n >> porder) < order { return Err(Error::Invalid("residual partition order")); }
    let psize = n >> porder;
    let mut i = order;
    for p in 0..parts {
        let end = (p + 1) * psize;
        let k = r.read(pbits)?;
        if k == esc {
            let bits = r.read(5)?;
            while i < end { out[i] = r.signed(bits)?; i += 1; }
        } else {
            while i < end {
                let q = r.unary()? as u64;
                let v = (q << k) | r.read(k)? as u64;
                out[i] = ((v >> 1) as i64) ^ -((v & 1) as i64);
                i += 1;
            }
        }
    }
    Ok(())
}

fn subframe(r: &mut BitReader, out: &mut [i64], bps: u32) -> Result<()> {
    if r.bit()? { return Err(Error::Invalid("subframe padding bit")); }
    let t = r.read(6)?;
    let wasted = if r.bit()? { r.unary()? + 1 } else { 0 };
    if wasted >= bps { return Err(Error::Invalid("wasted bits >= sample size")); }
    let bps = bps - wasted;
    let n = out.len();
    match t {
        0 => {
            let v = r.signed(bps)?;
            out.iter_mut().for_each(|s| *s = v);
        }
        1 => {
            for s in out.iter_mut() { *s = r.signed(bps)?; }
        }
        8..=12 => {
            let order = (t - 8) as usize;
            if order > n { return Err(Error::Invalid("fixed order > block size")); }
            for s in out[..order].iter_mut() { *s = r.signed(bps)?; }
            residual(r, out, order)?;
            match order {
                0 => {}
                1 => for i in 1..n { out[i] += out[i - 1]; },
                2 => for i in 2..n { out[i] += 2 * out[i - 1] - out[i - 2]; },
                3 => for i in 3..n { out[i] += 3 * out[i - 1] - 3 * out[i - 2] + out[i - 3]; },
                _ => for i in 4..n { out[i] += 4 * out[i - 1] - 6 * out[i - 2] + 4 * out[i - 3] - out[i - 4]; },
            }
        }
        32..=63 => {
            let order = (t - 31) as usize;
            if order > n { return Err(Error::Invalid("LPC order > block size")); }
            for s in out[..order].iter_mut() { *s = r.signed(bps)?; }
            let prec = r.read(4)?;
            if prec == 15 { return Err(Error::Invalid("LPC precision 15")); }
            let prec = prec + 1;
            let shift = r.signed(5)?;
            if shift < 0 { return Err(Error::Invalid("negative LPC shift")); }
            let mut coef = [0i64; 32];
            for c in coef[..order].iter_mut() { *c = r.signed(prec)?; }
            residual(r, out, order)?;
            for i in order..n {
                let mut acc = 0i64;
                for j in 0..order { acc += coef[j] * out[i - 1 - j]; }
                out[i] += acc >> shift;
            }
        }
        _ => return Err(Error::Invalid("reserved subframe type")),
    }
    if wasted > 0 { out.iter_mut().for_each(|s| *s <<= wasted); }
    Ok(())
}

impl FrameDecoder {
    pub fn new(si: StreamInfo) -> FrameDecoder {
        FrameDecoder { si, work: Vec::new(), md5: Md5::new(), md5_bytes: Vec::new(), samples: 0, frames: 0 }
    }

    /// Decode one frame from the start of `d` into `pcm`. Returns the bytes the frame occupied.
    /// [`Error::Eof`] means `d` does not hold the whole frame yet.
    pub fn decode(&mut self, d: &[u8], pcm: &mut Pcm) -> Result<usize> {
        let mut r = BitReader::new(d);
        let h = parse_header(&mut r, d, &self.si)?;
        if self.si.channels != 0 && h.channels as u32 != self.si.channels { return Err(Error::Unsupported("FLAC channel count change mid-stream")); }
        if h.bps != self.si.bps { return Err(Error::Unsupported("FLAC sample size change mid-stream")); }
        if self.work.len() < h.channels { self.work.resize_with(h.channels, Vec::new); }
        for c in 0..h.channels {
            let side = matches!((h.chan_code, c), (8, 1) | (9, 0) | (10, 1));
            let w = &mut self.work[c];
            w.clear();
            w.resize(h.block, 0);
            subframe(&mut r, w, h.bps + side as u32)?;
        }
        r.align();
        let end = r.byte_pos();
        let c = r.read(16)?;
        if crc16(&d[..end]) as u32 != c { return Err(Error::Checksum("FLAC frame CRC-16")); }
        let n = h.block;
        match h.chan_code {
            8 => { let (a, b) = self.work.split_at_mut(1); for i in 0..n { b[0][i] = a[0][i] - b[0][i]; } }
            9 => { let (a, b) = self.work.split_at_mut(1); for i in 0..n { a[0][i] += b[0][i]; } }
            10 => {
                let (a, b) = self.work.split_at_mut(1);
                for i in 0..n {
                    let side = b[0][i];
                    let mid = (a[0][i] << 1) | (side & 1);
                    a[0][i] = (mid + side) >> 1;
                    b[0][i] = (mid - side) >> 1;
                }
            }
            _ => {}
        }
        pcm.set_int(h.channels, n, h.bps);
        let bytes = h.bps.div_ceil(8) as usize;
        self.md5_bytes.clear();
        self.md5_bytes.reserve(n * h.channels * bytes);
        for i in 0..n {
            for c in 0..h.channels {
                let v = self.work[c][i];
                pcm.int[c][i] = v as i32;
                self.md5_bytes.extend_from_slice(&v.to_le_bytes()[..bytes]);
            }
        }
        self.md5.update(&self.md5_bytes);
        self.samples += n as u64;
        self.frames += 1;
        Ok(end + 2)
    }

    /// Compare the running MD5 with STREAMINFO's (an all-zero signature means "not computed": skipped).
    pub fn verify(&mut self) -> Result<bool> {
        let m = core::mem::take(&mut self.md5).finish();
        if self.si.md5 == [0; 16] { return Ok(false); }
        if m != self.si.md5 { return Err(Error::Checksum("FLAC MD5 of the decoded stream")); }
        Ok(true)
    }
}

/// A frame header at the start of `d` whose CRC-8 verifies and which names its own rate and depth (so it
/// can stand without STREAMINFO) — how [`crate::sniff`] recognises a headerless FLAC stream.
pub fn probe_frame(d: &[u8]) -> Option<FrameHeader> {
    if d.len() < 2 || d[0] != 0xFF || d[1] & 0xFE != 0xF8 { return None; }
    let mut r = BitReader::new(d);
    let h = parse_header(&mut r, d, &StreamInfo::default()).ok()?;
    if h.rate == 0 || h.bps == 0 { return None; }
    Some(h)
}

/// Native FLAC (`fLaC` + metadata blocks + frames).
pub struct FlacDecoder {
    s: ByteStream,
    fd: FrameDecoder,
    done: bool,
    /// Set at end of stream: `Some(true)` the MD5 matched, `Some(false)` STREAMINFO carried no MD5.
    pub md5_ok: Option<bool>,
}

impl FlacDecoder {
    pub fn new(mut s: ByteStream) -> Result<FlacDecoder> {
        s.fill(64)?;
        if let Some(h) = probe_frame(s.data()) {
            // A headerless stream (frames only, no `fLaC`/STREAMINFO): the first frame header carries every
            // parameter a decode needs; there is no MD5 to verify and no total.
            let si = StreamInfo { rate: h.rate, channels: h.channels as u32, bps: h.bps, ..StreamInfo::default() };
            return Ok(FlacDecoder { s, fd: FrameDecoder::new(si), done: false, md5_ok: None });
        }
        if s.take(4)? != b"fLaC" { return Err(Error::Invalid("not fLaC")); }
        let mut si = None;
        loop {
            let h = s.take(4)?;
            let last = h[0] & 0x80 != 0;
            let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize;
            match h[0] & 0x7F {
                0 => si = Some(StreamInfo::parse(&s.take(len)?)?),
                127 => return Err(Error::Invalid("metadata block type 127")),
                _ => s.skip(len as u64)?,
            }
            if last { break; }
        }
        let si = si.ok_or(Error::Invalid("no STREAMINFO"))?;
        Ok(FlacDecoder { s, fd: FrameDecoder::new(si), done: false, md5_ok: None })
    }
    pub fn stream_info(&self) -> StreamInfo { self.fd.si }
}

impl Source for FlacDecoder {
    fn info(&self) -> Info {
        let si = &self.fd.si;
        Info { rate: si.rate, channels: si.channels as u16, bits: si.bps as u16, frames: if si.total > 0 { Some(si.total) } else { None }, format: Format::Flac, codec: Codec::Flac, float: false }
    }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        if self.done { return Ok(false); }
        let mut want = if self.fd.si.max_frame > 0 { self.fd.si.max_frame as usize + 16 } else { 1 << 16 };
        loop {
            let have = self.s.fill(want)?;
            if have < 2 {
                self.done = true;
                self.md5_ok = Some(self.fd.verify()?);
                return Ok(false);
            }
            let d = self.s.data();
            if !(d[0] == 0xFF && d[1] & 0xFE == 0xF8) {
                // Not at a sync code (trailing tag / junk): resynchronise.
                let skip = d[1..].windows(2).position(|w| w[0] == 0xFF && w[1] & 0xFE == 0xF8).map(|p| p + 1).unwrap_or(have - 1);
                self.s.consume(skip);
                continue;
            }
            match self.fd.decode(d, pcm) {
                Ok(n) => { self.s.consume(n); return Ok(true); }
                Err(Error::Eof) if have >= want => want *= 2,
                Err(Error::Eof) => {
                    // The stream ended inside a frame: truncated file. Verify nothing, stop.
                    self.done = true;
                    return Err(Error::Eof);
                }
                Err(e @ (Error::Checksum(_) | Error::Invalid(_))) if self.fd.frames == 0 && have > 2 => {
                    // A false sync before the first frame: step past it.
                    let _ = e;
                    self.s.consume(1);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Decode a native FLAC file completely; returns (info, interleaved samples at native depth, MD5 verdict).
pub fn decode_file(bytes: &[u8]) -> Result<(StreamInfo, Vec<i32>, Option<bool>)> {
    let s = ByteStream::new(alloc::boxed::Box::new(crate::VecReader::new(bytes.to_vec())));
    let mut d = FlacDecoder::new(s)?;
    let mut pcm = Pcm::default();
    let ch = d.fd.si.channels as usize;
    let mut out = vec![];
    while d.block(&mut pcm)? {
        for i in 0..pcm.frames { for c in 0..ch { out.push(pcm.int[c][i]); } }
    }
    Ok((d.fd.si, out, d.md5_ok))
}
