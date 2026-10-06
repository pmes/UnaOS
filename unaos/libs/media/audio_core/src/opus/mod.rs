//! Opus, RFC 6716 with the RFC 8251 updates: the range decoder, SILK, CELT and the hybrid/transition logic
//! of the reference `opus_decoder.c`, fixed point and bit-exact with the normative reference decoder
//! (RFC 6716 §1 makes the reference implementation the specification); plus the Ogg Opus mapping
//! (RFC 7845: `OpusHead`, pre-skip, output gain, end trimming by granule position).
pub mod celt;
pub mod decoder;
pub mod range;
pub mod silk;

pub use decoder::OpusDecoder;

use crate::ogg::OggReader;
use crate::{Codec, Error, Format, Info, Pcm, Result, SeekPoint, Source};
use alloc::vec;
use alloc::vec::Vec;

/// RFC 7845 §5.1 identification header.
#[derive(Debug, Clone)]
pub struct OpusHead {
    pub channels: usize,
    pub pre_skip: u32,
    pub input_rate: u32,
    pub output_gain: i16,
    pub mapping_family: u8,
}

impl OpusHead {
    pub fn parse(d: &[u8]) -> Result<OpusHead> {
        if d.len() < 19 || &d[..8] != b"OpusHead" { return Err(Error::Invalid("OpusHead")); }
        if d[8] >> 4 != 0 { return Err(Error::Unsupported("OpusHead major version")); }
        let h = OpusHead {
            channels: d[9] as usize,
            pre_skip: u16::from_le_bytes([d[10], d[11]]) as u32,
            input_rate: u32::from_le_bytes([d[12], d[13], d[14], d[15]]),
            output_gain: i16::from_le_bytes([d[16], d[17]]),
            mapping_family: d[18],
        };
        if h.channels == 0 { return Err(Error::Invalid("OpusHead channel count")); }
        if h.mapping_family == 0 && h.channels > 2 { return Err(Error::Invalid("OpusHead family 0 with > 2 channels")); }
        if h.mapping_family != 0 { return Err(Error::Unsupported("Opus multistream (channel mapping family 1/255, owed)")); }
        Ok(h)
    }
}

/// An Ogg Opus stream (channel mapping family 0: mono/stereo).
pub struct OggOpus {
    r: OggReader,
    head: OpusHead,
    dec: OpusDecoder,
    skip: u64,
    /// Samples (per channel) handed out so far, after pre-skip.
    emitted: u64,
    /// Samples decoded including the pre-skip (to place the granule).
    decoded: u64,
    /// Granule of the first decoded sample (RFC 7845 §4.5: a stream may start at a granule > 0).
    granule_base: Option<i64>,
    buf: Vec<i16>,
    done: bool,
    /// SEEKTABLE2 (rmbp B469): the first audio page (after OpusHead/OpusTags, which end their pages).
    data_start: Option<u64>,
    /// The output length, once a seek has read the last page's granule.
    total: Option<u64>,
}

/// RFC 7845 §4.6: decode at least 80 ms (3840 samples at 48 kHz) before the target so the decoder converges.
pub const SEEK_PREROLL: u64 = 3840;

impl OggOpus {
    pub fn new(mut r: OggReader, first: &[u8]) -> Result<OggOpus> {
        let head = OpusHead::parse(first)?;
        // comment header
        let tags = r.next_packet()?.ok_or(Error::Eof)?;
        if !tags.data.starts_with(b"OpusTags") { return Err(Error::Invalid("OpusTags")); }
        let mut dec = OpusDecoder::new(head.channels);
        dec.decode_gain = head.output_gain as i32;
        let ch = head.channels;
        let data_start = r.at_page_boundary();
        Ok(OggOpus { r, skip: head.pre_skip as u64, head, dec, emitted: 0, decoded: 0, granule_base: None, buf: vec![0; 5760 * ch], done: false, data_start, total: None })
    }
}

impl Source for OggOpus {
    fn info(&self) -> Info {
        Info { rate: 48000, channels: self.head.channels as u16, bits: 16, frames: self.total, format: Format::Ogg, codec: Codec::Opus, float: false }
    }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        let ch = self.head.channels;
        loop {
            if self.done { return Ok(false); }
            let Some(p) = self.r.next_packet()? else { self.done = true; return Ok(false); };
            let n = match self.dec.decode(Some(&p.data), &mut self.buf, 5760) {
                Ok(n) => n,
                Err(_) => {
                    // a corrupt packet: conceal one 20 ms frame rather than stop
                    self.dec.decode(None, &mut self.buf, 960)?
                }
            };
            let mut start = 0usize;
            let mut end = n;
            self.decoded += n as u64;
            if self.skip > 0 {
                let s = (self.skip as usize).min(n);
                start = s;
                self.skip -= s as u64;
            }
            // the first page that completes a packet fixes the granule of sample 0 (RFC 7845 §4.5); a
            // later start is valid (a stream cut from a longer one) and only shifts the end limit
            if let (None, Some(g)) = (self.granule_base, p.granule) {
                self.granule_base = Some((g as i64 - self.decoded as i64).max(0));
            }
            // end trimming: the last page's granule bounds the total (RFC 7845 §4.4)
            if p.eos {
                if let Some(g) = p.granule {
                    let limit = g as i64 - self.granule_base.unwrap_or(0);
                    if limit < self.decoded as i64 {
                        let cut = (self.decoded as i64 - limit) as usize;
                        end = end.saturating_sub(cut).max(start);
                    }
                }
            }
            let frames = end - start;
            if frames == 0 { if p.eos { self.done = true; } continue; }
            pcm.set_int(ch, frames, 16);
            for c in 0..ch {
                for i in 0..frames { pcm.int[c][i] = self.buf[(start + i) * ch + c] as i32; }
            }
            self.emitted += frames as u64;
            if p.eos { self.done = true; }
            return Ok(true);
        }
    }
    /// SEEKTABLE2 (rmbp B469): the page granule bisection ([`OggReader::bisect`]) to the last page ending at or before
    /// the target granule less the 80 ms pre-roll; a fresh decoder starts with the packet after it, at that page's
    /// granule — known exactly, so `exact`; the pre-roll's PCM is the residual [`crate::Decoder::seek`] drops.
    fn seek(&mut self, t: u64) -> Result<Option<SeekPoint>> {
        let Some(start) = self.data_start else { return Ok(None) };
        if !self.r.can_seek() { return Ok(None); }
        if self.granule_base.is_none() {
            // the first audio page fixes where granule 0 lies (RFC 7845 §4.5): decode it once
            let mut scratch = Pcm::default();
            while self.granule_base.is_none() && !self.done { if !self.block(&mut scratch)? { break; } }
        }
        let base = self.granule_base.unwrap_or(0).max(0) as u64;
        let pre = self.head.pre_skip as u64;
        if let Some((_, g)) = self.r.bisect(start, u64::MAX)? { self.total = Some(g.saturating_sub(base + pre)); }
        let t = self.total.map_or(t, |n| t.min(n));
        let want = (t + pre + base).saturating_sub(SEEK_PREROLL);
        self.dec = OpusDecoder::new(self.head.channels);
        self.dec.decode_gain = self.head.output_gain as i32;
        self.done = false;
        if let Some((pb, g)) = self.r.bisect(start, want)?.filter(|&(_, g)| g >= base + pre) {
            self.r.resume(pb)?;
            self.skip = 0;
            self.decoded = g - base;
            self.emitted = g - base - pre;
            return Ok(Some(SeekPoint { byte: pb, sample: self.emitted, exact: true, table: "ogg", landed: self.emitted }));
        }
        // the target lies within the first 80 ms: from the top (the pre-skip is the convergence there)
        if !self.r.reposition(start)? { return Ok(None); }
        self.skip = pre;
        self.decoded = 0;
        self.emitted = 0;
        Ok(Some(SeekPoint { byte: start, sample: 0, exact: true, table: "ogg", landed: 0 }))
    }
}
