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
use crate::{Codec, Error, Format, Info, Pcm, Result, Source};
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
}

impl OggOpus {
    pub fn new(mut r: OggReader, first: &[u8]) -> Result<OggOpus> {
        let head = OpusHead::parse(first)?;
        // comment header
        let tags = r.next_packet()?.ok_or(Error::Eof)?;
        if !tags.data.starts_with(b"OpusTags") { return Err(Error::Invalid("OpusTags")); }
        let mut dec = OpusDecoder::new(head.channels);
        dec.decode_gain = head.output_gain as i32;
        let ch = head.channels;
        Ok(OggOpus { r, skip: head.pre_skip as u64, head, dec, emitted: 0, decoded: 0, granule_base: None, buf: vec![0; 5760 * ch], done: false })
    }
}

impl Source for OggOpus {
    fn info(&self) -> Info {
        Info { rate: 48000, channels: self.head.channels as u16, bits: 16, frames: None, format: Format::Ogg, codec: Codec::Opus, float: false }
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
}
