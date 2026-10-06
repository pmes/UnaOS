// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AAC-LC (ISO/IEC 14496-3 subpart 4, the general audio coder; ISO/IEC 13818-7 ADTS), float.
//!
//! * [`decoder`] — raw_data_block: SCE/CPE/LFE/CCE/DSE/PCE/FIL, section and scalefactor data, pulse, TNS,
//!   spectral Huffman decoding, inverse quantisation, M/S, intensity stereo, PNS, filterbank.
//! * [`AdtsStream`] — ADTS framing (sync confirmed by the next header, CRC skipped, several raw blocks
//!   per frame), ID3v2 in front tolerated.
//! * [`AacSource`] — access units from MP4 ([`crate::container`], over `demux_core`) with an AudioSpecificConfig; edit-list /
//!   iTunSMPB gapless trimming.
//!
//! Output channel order is the WAVE order (FL FR FC LFE BL BR FLC FRC BC SL SR), mapped from the
//! channelConfiguration or the program_config_element. HE-AAC (SBR, PS) streams decode as their AAC-LC
//! core at the core sample rate; SBR/PS are owed.
pub mod decoder;
pub mod tables;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::bits::BitReader;
use crate::io::ByteStream;
use crate::{SeekPoint, Codec, Error, Format, Info, Pcm, Result, Source};
pub use decoder::{AacDecoder, Layout, Pce};

pub const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// The parts of an AudioSpecificConfig (§1.6.2.1) this decoder uses.
#[derive(Clone, Debug)]
pub struct Asc {
    pub object_type: u32,
    pub sf_index: usize,
    pub channel_config: u8,
    pub pce: Option<Pce>,
    /// Explicitly signalled SBR (object type 5/29 or the sync extension): the output rate would double.
    pub sbr: bool,
}

fn aot(r: &mut BitReader) -> Result<u32> {
    let a = r.read(5)?;
    Ok(if a == 31 { 32 + r.read(6)? } else { a })
}

fn sf_index(r: &mut BitReader) -> Result<usize> {
    let i = r.read(4)? as usize;
    if i == 15 {
        let rate = r.read(24)?;
        return RATES.iter().position(|&x| x == rate).ok_or(Error::Unsupported("aac: explicit sampling rate"));
    }
    if i > 12 { return Err(Error::Invalid("aac: sampling frequency index")); }
    Ok(i)
}

impl Asc {
    pub fn parse(d: &[u8]) -> Result<Asc> {
        let mut r = BitReader::new(d);
        let mut object_type = aot(&mut r)?;
        let sfi = sf_index(&mut r)?;
        let channel_config = r.read(4)? as u8;
        let mut sbr = false;
        if object_type == 5 || object_type == 29 {
            sbr = true;
            let _ext = sf_index(&mut r)?;
            object_type = aot(&mut r)?;
        }
        let mut pce = None;
        match object_type {
            1 | 2 | 3 | 4 | 6 | 7 => {
                // GASpecificConfig
                if r.bit()? { return Err(Error::Unsupported("aac: 960-sample frames")); }
                if r.bit()? { r.read(14)?; }
                let ext = r.bit()?;
                if channel_config == 0 { pce = Some(Pce::parse(&mut r, 0)?); }
                if object_type == 6 || object_type == 20 { r.read(3)?; }
                let _ = ext;
            }
            _ => return Err(Error::Unsupported("aac: object type (only AAC LC is decoded)")),
        }
        if object_type != 2 { return Err(Error::Unsupported("aac: object type (only AAC LC is decoded)")); }
        // backward-compatible SBR signalling (sync extension 0x2b7)
        if r.bits_left() >= 16 && r.peek(11) == 0x2b7 {
            r.read(11)?;
            if aot(&mut r)? == 5 && r.bit().unwrap_or(false) { sbr = true; }
        }
        Ok(Asc { object_type, sf_index: sfi, channel_config, pce, sbr })
    }
    pub fn layout(&self) -> Result<Layout> {
        if let Some(p) = &self.pce { return Ok(Layout::from_pce(p)); }
        Layout::from_config(self.channel_config).ok_or(Error::Unsupported("aac: channel configuration"))
    }
}

// ------------------------------------------------------------------ ADTS

#[derive(Clone, Copy, Debug)]
struct AdtsHeader {
    protection_absent: bool,
    profile: u8,
    sf_index: usize,
    channel_config: u8,
    frame_length: usize,
    blocks: usize,
}

fn adts_header(d: &[u8]) -> Option<AdtsHeader> {
    if d.len() < 7 || d[0] != 0xFF || d[1] & 0xF6 != 0xF0 { return None; }
    let sfi = ((d[2] >> 2) & 15) as usize;
    if sfi > 12 { return None; }
    let frame_length = (((d[3] & 3) as usize) << 11) | ((d[4] as usize) << 3) | (d[5] as usize >> 5);
    let protection_absent = d[1] & 1 != 0;
    if frame_length < if protection_absent { 7 } else { 9 } { return None; }
    Some(AdtsHeader {
        protection_absent,
        profile: d[2] >> 6,
        sf_index: sfi,
        channel_config: ((d[2] & 1) << 2) | (d[3] >> 6),
        frame_length,
        blocks: (d[6] & 3) as usize + 1,
    })
}

pub struct AdtsStream {
    s: ByteStream,
    dec: AacDecoder,
    info: Info,
    in_sync: bool,
    /// SEEKTABLE2 (rmbp B469): the first frame's offset, the decoder's parameters (a seek restarts it), and the
    /// header-walk index (every [`ADTS_STRIDE`]th frame's (byte, first sample)), built on the first seek.
    start: u64,
    sf_index: usize,
    layout: Layout,
    index: Option<Vec<(u64, u64)>>,
}

/// SEEKTABLE2: one index point per this many ADTS frames (a seek walks at most twice this many headers).
pub const ADTS_STRIDE: u64 = 16;

impl AdtsStream {
    pub fn new(mut s: ByteStream) -> Result<AdtsStream> {
        skip_id3(&mut s)?;
        let h = sync(&mut s, false)?.ok_or(Error::Invalid("aac: no ADTS frame"))?;
        if h.profile != 1 { return Err(Error::Unsupported("aac: ADTS profile (only AAC LC is decoded)")); }
        let layout = match Layout::from_config(h.channel_config) {
            Some(l) => l,
            None => {
                // channelConfiguration 0: the PCE is the first element of the first raw block
                let fr = s.need(h.frame_length)?.to_vec();
                let off = if h.protection_absent { 7 } else { 9 };
                let mut r = BitReader::new(&fr[off..]);
                if r.read(3)? != decoder::ID_PCE { return Err(Error::Unsupported("aac: ADTS channel configuration 0 without a PCE")); }
                Layout::from_pce(&Pce::parse(&mut r, 0)?)
            }
        };
        let ch = layout.channels as u16;
        let dec = AacDecoder::new(h.sf_index, layout.clone())?;
        let info = Info { rate: RATES[h.sf_index], channels: ch, bits: 0, frames: None, format: Format::Adts, codec: Codec::Aac, float: true };
        let start = s.offset();
        Ok(AdtsStream { s, dec, info, in_sync: true, start, sf_index: h.sf_index, layout, index: None })
    }
}

fn skip_id3(s: &mut ByteStream) -> Result<()> {
    loop {
        let d = s.need(10).map(|d| d.to_vec());
        let Ok(d) = d else { return Ok(()) };
        if &d[0..3] != b"ID3" { return Ok(()); }
        let size = ((d[6] as u64 & 127) << 21) | ((d[7] as u64 & 127) << 14) | ((d[8] as u64 & 127) << 7) | (d[9] as u64 & 127);
        let footer = if d[5] & 0x10 != 0 { 10 } else { 0 };
        s.skip(10 + size + footer)?;
    }
}

/// Find the next ADTS header. While in sync the header at the cursor is taken as is; out of sync, a
/// candidate counts only if the next header (or EOF) follows at its frame_length.
fn sync(s: &mut ByteStream, in_sync: bool) -> Result<Option<AdtsHeader>> {
    loop {
        if s.fill(7)? < 7 { return Ok(None); }
        if let Some(h) = adts_header(s.data()) {
            if in_sync { return Ok(Some(h)); }
            let have = s.fill(h.frame_length + 7)?;
            if have <= h.frame_length + 1 { return Ok(Some(h)); }
            if let Some(n) = adts_header(&s.data()[h.frame_length..]) {
                if n.sf_index == h.sf_index { return Ok(Some(h)); }
            }
        }
        s.consume(1);
    }
}

impl Source for AdtsStream {
    fn info(&self) -> Info { self.info }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        loop {
            let Some(h) = sync(&mut self.s, self.in_sync)? else { return Ok(false) };
            let Ok(fr) = self.s.need(h.frame_length).map(|d| d.to_vec()) else { return Ok(false) };
            self.s.consume(h.frame_length);
            let mut off = 7;
            if !h.protection_absent { off += if h.blocks > 1 { 2 * h.blocks } else { 2 }; }
            if off > fr.len() { self.in_sync = false; continue; }
            let body = &fr[off..];
            let mut r = BitReader::new(body);
            let ch = self.info.channels as usize;
            pcm.set_float(ch, 0);
            let mut ok_blocks = 0;
            for b in 0..h.blocks {
                let start = r.bit_pos();
                if self.dec.decode_block(&mut r, start).is_err() { break; }
                if !h.protection_absent && h.blocks > 1 { r.skip(16).ok(); }
                let n = pcm.frames;
                for c in 0..ch { pcm.flt[c].extend_from_slice(&self.dec.out[c]); }
                pcm.frames = n + 1024;
                ok_blocks += 1;
                let _ = b;
            }
            if ok_blocks == 0 {
                // a damaged frame: hand out silence of its length so timing holds, and resync
                self.in_sync = false;
                for c in 0..ch { pcm.flt[c].resize(1024 * h.blocks, 0.0); }
                pcm.frames = 1024 * h.blocks;
                return Ok(true);
            }
            self.in_sync = true;
            return Ok(true);
        }
    }
    /// SEEKTABLE2 (rmbp B469): the header-walk index to the frame holding the target, then one frame back — the
    /// pre-roll that primes the MDCT overlap (its PCM is the residual [`crate::Decoder::seek`] drops). The decoder
    /// restarts fresh there. Every frame's first sample is counted, so the landing is exact.
    fn seek(&mut self, t: u64) -> Result<Option<SeekPoint>> {
        if self.s.len().is_none() || !self.s.seekable() { return Ok(None); }
        if self.index.is_none() { self.index = Some(self.build_index()?); }
        let idx = self.index.as_deref().unwrap_or(&[]);
        let i = idx.partition_point(|e| e.1 <= t).saturating_sub(2);
        let Some(&(b0, s0)) = idx.get(i) else { return Ok(None) };
        if !self.s.seek(b0)? { return Ok(None); }
        let (mut cur, mut prev) = ((b0, s0), None);
        while let Some(h) = self.header_here()? {
            cur.0 = self.s.offset();
            let len = 1024 * h.blocks as u64;
            if cur.1 + len > t || !self.step(&h)? { break; }
            prev = Some(cur);
            cur.1 += len;
        }
        let (byte, sample) = prev.unwrap_or(cur);
        if !self.s.seek(byte)? { return Ok(None); }
        self.dec = AacDecoder::new(self.sf_index, self.layout.clone())?;
        self.in_sync = true;
        Ok(Some(SeekPoint { byte, sample, exact: true, table: "adts", landed: sample }))
    }
}

/// SEEKTABLE2 (rmbp B469): ADTS carries no timestamps, so its table is a header walk (no decode): each frame is
/// `1024 · number_of_raw_data_blocks` samples (ISO/IEC 13818-7 §6.2 / 14496-3 §1.A.2), found by `frame_length`
/// from the first frame — the same frames `block` hands out, a damaged one as silence of its length.
impl AdtsStream {
    /// The header at the cursor, stepping over bytes that are not one (as `block`'s in-sync scan does).
    fn header_here(&mut self) -> Result<Option<AdtsHeader>> {
        loop {
            if self.s.fill(7)? < 7 { return Ok(None); }
            if let Some(h) = adts_header(self.s.data()) { return Ok(Some(h)); }
            self.s.consume(1);
        }
    }
    /// Step over the frame at the cursor; `false` when it is cut short (the end: `block` stops there too).
    fn step(&mut self, h: &AdtsHeader) -> Result<bool> {
        if self.s.fill(h.frame_length)? < h.frame_length { return Ok(false); }
        self.s.consume(h.frame_length);
        Ok(true)
    }
    fn build_index(&mut self) -> Result<Vec<(u64, u64)>> {
        let mut idx = Vec::new();
        let (mut n, mut sample) = (0u64, 0u64);
        if !self.s.seek(self.start)? { return Ok(idx); }
        while let Some(h) = self.header_here()? {
            let at = self.s.offset();
            if !self.step(&h)? { break; }
            if n % ADTS_STRIDE == 0 { idx.push((at, sample)); }
            sample += 1024 * h.blocks as u64;
            n += 1;
        }
        self.info.frames = Some(sample);
        Ok(idx)
    }
}

// ------------------------------------------------------------------ AAC in MP4

/// Access units with an AudioSpecificConfig (MP4 `esds`), plus trimming.
pub struct AacSource {
    /// VPLAYAUDIO (rmbp B475): the file's bytes, shared with the demuxer that indexed them (no copy).
    data: alloc::sync::Arc<Vec<u8>>,
    units: Vec<(usize, usize)>,
    next: usize,
    dec: AacDecoder,
    info: Info,
    skip: u64,
    total: Option<u64>,
    emitted: u64,
    /// SEEKTABLE (rmbp B433): the gapless skip at the top, and the ASC (a seek restarts the decoder from it).
    skip0: u64,
    asc: Vec<u8>,
}

impl AacSource {
    pub fn new(data: impl Into<alloc::sync::Arc<Vec<u8>>>, units: Vec<(usize, usize)>, asc: &[u8], skip: u64, total: Option<u64>) -> Result<AacSource> {
        let data = data.into();
        let a = Asc::parse(asc)?;
        let layout = a.layout()?;
        let ch = layout.channels as u16;
        let dec = AacDecoder::new(a.sf_index, layout)?;
        let frames = total.or(Some((units.len() as u64 * 1024).saturating_sub(skip)));
        let info = Info { rate: RATES[a.sf_index], channels: ch, bits: 0, frames, format: Format::Mp4, codec: Codec::Aac, float: true };
        Ok(AacSource { data, units, next: 0, dec, info, skip, total, emitted: 0, skip0: skip, asc: asc.to_vec() })
    }
    pub fn boxed(self) -> Box<dyn Source> { Box::new(self) }
}

impl Source for AacSource {
    fn info(&self) -> Info { self.info }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        let ch = self.info.channels as usize;
        loop {
            if self.next >= self.units.len() { return Ok(false); }
            if let Some(t) = self.total { if self.emitted >= t { return Ok(false); } }
            let (off, len) = self.units[self.next];
            self.next += 1;
            let end = (off + len).min(self.data.len());
            let au = &self.data[off.min(end)..end];
            let mut r = BitReader::new(au);
            if self.dec.decode_block(&mut r, 0).is_err() {
                for o in self.dec.out.iter_mut() { o.iter_mut().for_each(|x| *x = 0.0); }
            }
            let mut start = 0usize;
            if self.skip > 0 { let s = (self.skip as usize).min(1024); start = s; self.skip -= s as u64; }
            let mut stop = 1024usize;
            if let Some(t) = self.total { stop = stop.min(start + (t - self.emitted) as usize); }
            if stop <= start { continue; }
            pcm.set_float(ch, stop - start);
            for c in 0..ch { pcm.flt[c].copy_from_slice(&self.dec.out[c][start..stop]); }
            self.emitted += (stop - start) as u64;
            return Ok(true);
        }
    }
    /// SEEKTABLE (rmbp B433): the MP4 sample table (`stsc`+`stco`/`co64`+`stsz`, or the fragments' `trun`s) is the
    /// index: access unit `k` starts at decoded sample `1024·k` (AAC-LC, ISO/IEC 14496-3 — the same times `stts`
    /// gives; `demux_core::Demuxer::seek_track` over the same file lands on the same unit). One unit of pre-roll
    /// primes the MDCT overlap; its PCM is dropped. Exact.
    fn seek(&mut self, t: u64) -> Result<Option<SeekPoint>> {
        let n = ((t + self.skip0) / 1024).min(self.units.len() as u64);
        let l = n.saturating_sub(1);
        let a = Asc::parse(&self.asc)?;
        self.dec = AacDecoder::new(a.sf_index, a.layout()?)?;
        self.next = l as usize;
        let d0 = (n * 1024).max(self.skip0);
        self.skip = d0 - l * 1024;
        let sample = d0 - self.skip0;
        self.emitted = sample;
        let byte = self.units.get(l as usize).map(|u| u.0 as u64).unwrap_or(self.data.len() as u64);
        Ok(Some(SeekPoint { byte, sample, exact: true, table: "mp4", landed: sample }))
    }
}
