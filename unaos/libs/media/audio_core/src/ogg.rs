//! The Ogg container, RFC 3533: page capture with resynchronisation (§6), the page CRC-32 (checked;
//! a page that fails it is dropped), lacing-value packet reassembly across pages (§5), granule positions
//! (the last packet completed on a page carries the page's granule), and logical-stream selection (the
//! first stream whose BOS page names a codec we decode; other multiplexed streams are skipped). A chained
//! stream (a second BOS after EOS) ends decoding at the chain boundary — OWED.
//! [`open`] looks into the first packet and returns the codec: FLAC-in-Ogg here, Opus and Vorbis in
//! their own modules.
use crate::crc::crc32_ogg;
use crate::flac::{FrameDecoder, StreamInfo};
use crate::io::ByteStream;
use crate::{Codec, Error, Format, Info, Pcm, Result, Source};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// One reassembled packet.
#[derive(Debug, Clone)]
pub struct Packet {
    pub data: Vec<u8>,
    /// The page's granule position, on the last packet that completes on that page.
    pub granule: Option<u64>,
    /// The packet is the last of the logical stream.
    pub eos: bool,
}

pub struct OggReader {
    s: ByteStream,
    serial: Option<u32>,
    ready: VecDeque<Packet>,
    partial: Vec<u8>,
    have_partial: bool,
    eos: bool,
    pub pages: u64,
    pub crc_failures: u64,
    pub last_granule: u64,
}

impl OggReader {
    pub fn new(s: ByteStream) -> OggReader {
        OggReader { s, serial: None, ready: VecDeque::new(), partial: Vec::new(), have_partial: false, eos: false, pages: 0, crc_failures: 0, last_granule: 0 }
    }
    /// Lock onto one logical stream (by serial number).
    pub fn set_serial(&mut self, serial: u32) { self.serial = Some(serial); }

    /// Read the next page; returns (serial, header_type, granule, segments-with-body) or None at EOF.
    fn page(&mut self) -> Result<Option<(u32, u8, u64, Vec<u8>, Vec<u8>)>> {
        loop {
            if self.s.fill(27)? < 27 { return Ok(None); }
            let d = self.s.data();
            if &d[0..4] != b"OggS" {
                let skip = d[1..].windows(4).position(|w| w == b"OggS").map(|p| p + 1).unwrap_or(d.len() - 3);
                self.s.consume(skip);
                continue;
            }
            if d[4] != 0 { self.s.consume(1); continue; }
            let nseg = d[26] as usize;
            if self.s.fill(27 + nseg)? < 27 + nseg { return Ok(None); }
            let lacing: Vec<u8> = self.s.data()[27..27 + nseg].to_vec();
            let body: usize = lacing.iter().map(|&l| l as usize).sum();
            let total = 27 + nseg + body;
            if self.s.fill(total)? < total { return Ok(None); }
            let d = self.s.data();
            let mut hdr = d[..total].to_vec();
            let crc = u32::from_le_bytes([hdr[22], hdr[23], hdr[24], hdr[25]]);
            hdr[22..26].copy_from_slice(&[0; 4]);
            if crc32_ogg(&hdr) != crc {
                self.crc_failures += 1;
                self.s.consume(1);
                continue;
            }
            let htype = d[5];
            let granule = u64::from_le_bytes(d[6..14].try_into().unwrap());
            let serial = u32::from_le_bytes(d[14..18].try_into().unwrap());
            let body = d[27 + nseg..total].to_vec();
            self.s.consume(total);
            self.pages += 1;
            return Ok(Some((serial, htype, granule, lacing, body)));
        }
    }

    /// The next packet of the selected logical stream (the first stream seen if none was selected).
    pub fn next_packet(&mut self) -> Result<Option<Packet>> {
        loop {
            if let Some(p) = self.ready.pop_front() { return Ok(Some(p)); }
            if self.eos { return Ok(None); }
            let Some((serial, htype, granule, lacing, body)) = self.page()? else {
                return Ok(None);
            };
            match self.serial {
                None => self.serial = Some(serial),
                Some(s) if s != serial => continue,
                _ => {}
            }
            let continued = htype & 1 != 0;
            if !continued && self.have_partial { self.partial.clear(); self.have_partial = false; } // lost continuation
            let mut skip_first = continued && !self.have_partial; // continuation of a packet we never saw
            let mut off = 0usize;
            let mut completed: Vec<Vec<u8>> = Vec::new();
            for &l in lacing.iter() {
                let seg = &body[off..off + l as usize];
                off += l as usize;
                if !skip_first { self.partial.extend_from_slice(seg); self.have_partial = true; }
                if l < 255 {
                    if !skip_first { completed.push(core::mem::take(&mut self.partial)); }
                    self.have_partial = false;
                    skip_first = false;
                }
            }
            let eos = htype & 4 != 0;
            let n = completed.len();
            for (i, data) in completed.into_iter().enumerate() {
                let last = i + 1 == n;
                self.ready.push_back(Packet { data, granule: if last && granule != u64::MAX { Some(granule) } else { None }, eos: last && eos });
            }
            if granule != u64::MAX && n > 0 { self.last_granule = granule; }
            if eos { self.eos = true; }
        }
    }
}

/// Look into the first packet of an Ogg stream and return the matching codec source.
pub fn open(s: ByteStream) -> Result<Box<dyn Source>> {
    let mut r = OggReader::new(s);
    let first = r.next_packet()?.ok_or(Error::Eof)?;
    let d = &first.data;
    if d.len() >= 51 && d[0] == 0x7F && &d[1..5] == b"FLAC" {
        return Ok(Box::new(OggFlac::new(r, &first.data)?));
    }
    if d.starts_with(b"OpusHead") { let d = first.data.clone(); return Ok(Box::new(crate::opus::OggOpus::new(r, &d)?)); }
    if d.len() >= 7 && d[0] == 1 && &d[1..7] == b"vorbis" { let d = first.data.clone(); return Ok(Box::new(crate::vorbis::OggVorbis::new(r, &d)?)); }
    Err(Error::Unsupported("Ogg stream of an unknown codec"))
}

/// FLAC in Ogg (the Xiph mapping: `0x7F "FLAC" 1.0`, header count, `fLaC`, STREAMINFO; one frame per packet).
pub struct OggFlac {
    r: OggReader,
    fd: FrameDecoder,
    done: bool,
    pub md5_ok: Option<bool>,
}

impl OggFlac {
    fn new(r: OggReader, first: &[u8]) -> Result<OggFlac> {
        if &first[9..13] != b"fLaC" || first[13] & 0x7F != 0 { return Err(Error::Invalid("Ogg FLAC first packet")); }
        let si = StreamInfo::parse(&first[17..])?;
        Ok(OggFlac { r, fd: FrameDecoder::new(si), done: false, md5_ok: None })
    }
}

impl Source for OggFlac {
    fn info(&self) -> Info {
        let si = &self.fd.si;
        Info { rate: si.rate, channels: si.channels as u16, bits: si.bps as u16, frames: if si.total > 0 { Some(si.total) } else { None }, format: Format::Ogg, codec: Codec::Flac, float: false }
    }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        if self.done { return Ok(false); }
        loop {
            let Some(p) = self.r.next_packet()? else {
                self.done = true;
                self.md5_ok = Some(self.fd.verify()?);
                return Ok(false);
            };
            if p.data.len() >= 2 && p.data[0] == 0xFF && p.data[1] & 0xFE == 0xF8 {
                self.fd.decode(&p.data, pcm)?;
                return Ok(true);
            }
            // a metadata packet (VORBIS_COMMENT, PADDING, ...): skip
        }
    }
}
