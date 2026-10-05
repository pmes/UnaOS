//! The Opus decoder proper (`src/opus_decoder.c`, `src/opus.c`; RFC 6716 §3, §4.5): TOC and frame
//! packing (codes 0–3, padding), SILK/hybrid/CELT dispatch, mode transitions with the 5 ms redundant CELT
//! frames and the smooth cross-fades, packet-loss concealment through both layers, and the final range.
//! Fixed point, 48 kHz output, bit-exact with the reference's `opus_decode`.
use super::celt::fixed::{mult16_16, mult16_16_q15};
use super::celt::tables::WINDOW120;
use super::celt::CeltDecoder;
use super::range::RangeDecoder;
use super::silk::{DecControl, SilkDecoder};
use crate::{Error, Result};
use alloc::vec;

pub const MODE_SILK_ONLY: i32 = 1000;
pub const MODE_HYBRID: i32 = 1001;
pub const MODE_CELT_ONLY: i32 = 1002;
pub const BW_NB: i32 = 1101;
pub const BW_MB: i32 = 1102;
pub const BW_WB: i32 = 1103;
pub const BW_SWB: i32 = 1104;
pub const BW_FB: i32 = 1105;

const FS: i32 = 48000;

pub fn packet_mode(toc: u8) -> i32 {
    if toc & 0x80 != 0 { MODE_CELT_ONLY } else if toc & 0x60 == 0x60 { MODE_HYBRID } else { MODE_SILK_ONLY }
}
pub fn packet_bandwidth(toc: u8) -> i32 {
    if toc & 0x80 != 0 {
        let bw = BW_MB + ((toc >> 5) & 0x3) as i32;
        if bw == BW_MB { BW_NB } else { bw }
    } else if toc & 0x60 == 0x60 {
        if toc & 0x10 != 0 { BW_FB } else { BW_SWB }
    } else {
        BW_NB + ((toc >> 5) & 0x3) as i32
    }
}
pub fn packet_samples_per_frame(toc: u8, fs: i32) -> i32 {
    if toc & 0x80 != 0 {
        (fs << ((toc >> 3) & 0x3)) / 400
    } else if toc & 0x60 == 0x60 {
        if toc & 0x08 != 0 { fs / 50 } else { fs / 100 }
    } else {
        let a = ((toc >> 3) & 0x3) as i32;
        if a == 3 { fs * 60 / 1000 } else { (fs << a) / 100 }
    }
}
pub fn packet_channels(toc: u8) -> usize { if toc & 0x4 != 0 { 2 } else { 1 } }

fn parse_size(d: &[u8]) -> Option<(usize, usize)> {
    if d.is_empty() { None } else if d[0] < 252 { Some((d[0] as usize, 1)) } else if d.len() < 2 { None } else { Some((4 * d[1] as usize + d[0] as usize, 2)) }
}

/// `opus_packet_parse` (not self-delimited): (toc, payload offset, frame sizes).
pub fn parse_packet(data: &[u8]) -> Result<(u8, usize, alloc::vec::Vec<usize>)> {
    if data.is_empty() { return Err(Error::Invalid("empty Opus packet")); }
    let framesize = packet_samples_per_frame(data[0], 48000) as usize;
    let toc = data[0];
    let mut p = 1usize;
    let mut len = data.len() - 1;
    let mut sizes = alloc::vec::Vec::new();
    let last_size;
    let count;
    match toc & 3 {
        0 => { count = 1; last_size = len; }
        1 => {
            count = 2;
            if len & 1 != 0 { return Err(Error::Invalid("Opus code-1 odd length")); }
            last_size = len / 2;
            sizes.push(last_size);
        }
        2 => {
            count = 2;
            let (s, b) = parse_size(&data[p..]).ok_or(Error::Invalid("Opus frame size"))?;
            len -= b;
            if s > len { return Err(Error::Invalid("Opus frame size")); }
            p += b;
            sizes.push(s);
            last_size = len - s;
        }
        _ => {
            if len < 1 { return Err(Error::Invalid("Opus code-3 header")); }
            let ch = data[p];
            p += 1;
            count = (ch & 0x3F) as usize;
            if count == 0 || framesize * count > 5760 { return Err(Error::Invalid("Opus frame count")); }
            len -= 1;
            if ch & 0x40 != 0 {
                loop {
                    if len == 0 { return Err(Error::Invalid("Opus padding")); }
                    let pv = data[p] as usize;
                    p += 1;
                    len -= 1;
                    let tmp = if pv == 255 { 254 } else { pv };
                    if tmp > len { return Err(Error::Invalid("Opus padding")); }
                    len -= tmp;
                    if pv != 255 { break; }
                }
            }
            let cbr = ch & 0x80 == 0;
            if !cbr {
                let mut ls = len as isize;
                for _ in 0..count - 1 {
                    let (s, b) = parse_size(&data[p..p + len]).ok_or(Error::Invalid("Opus frame size"))?;
                    len -= b;
                    if s > len { return Err(Error::Invalid("Opus frame size")); }
                    p += b;
                    ls -= (b + s) as isize;
                    sizes.push(s);
                }
                if ls < 0 { return Err(Error::Invalid("Opus frame sizes")); }
                last_size = ls as usize;
            } else {
                last_size = len / count;
                if last_size * count != len { return Err(Error::Invalid("Opus CBR length")); }
                for _ in 0..count - 1 { sizes.push(last_size); }
            }
        }
    }
    if last_size > 1275 { return Err(Error::Invalid("Opus frame too long")); }
    sizes.push(last_size);
    let _ = count;
    Ok((toc, p, sizes))
}

pub struct OpusDecoder {
    pub channels: usize,
    silk: SilkDecoder,
    celt: CeltDecoder,
    ctl: DecControl,
    pub decode_gain: i32,
    stream_channels: usize,
    bandwidth: i32,
    mode: i32,
    prev_mode: i32,
    frame_size: i32,
    prev_redundancy: bool,
    pub last_packet_duration: i32,
    pub range_final: u32,
}

fn smooth_fade(in1: &[i16], in2: &[i16], out: &mut [i16], overlap: usize, channels: usize) {
    for c in 0..channels {
        for i in 0..overlap {
            let w = mult16_16_q15(WINDOW120[i] as i32, WINDOW120[i] as i32);
            out[i * channels + c] = ((mult16_16(w, in2[i * channels + c] as i32) + mult16_16(32767 - w, in1[i * channels + c] as i32)) >> 15) as i16;
        }
    }
}

impl OpusDecoder {
    pub fn new(channels: usize) -> OpusDecoder {
        let mut ctl = DecControl::default();
        ctl.api_sample_rate = FS;
        ctl.n_channels_api = channels;
        OpusDecoder {
            channels,
            silk: SilkDecoder::new(),
            celt: CeltDecoder::new(channels),
            ctl,
            decode_gain: 0,
            stream_channels: channels,
            bandwidth: 0,
            mode: 0,
            prev_mode: 0,
            frame_size: FS / 400,
            prev_redundancy: false,
            last_packet_duration: 0,
            range_final: 0,
        }
    }

    pub fn reset(&mut self) {
        self.celt.reset();
        self.silk.reset();
        self.stream_channels = self.channels;
        self.bandwidth = 0;
        self.mode = 0;
        self.prev_mode = 0;
        self.frame_size = FS / 400;
        self.prev_redundancy = false;
        self.last_packet_duration = 0;
        self.range_final = 0;
    }

    fn decode_frame(&mut self, data: Option<&[u8]>, pcm: &mut [i16], frame_size: i32, decode_fec: bool) -> Result<i32> {
        let ch = self.channels;
        let f20 = FS / 50;
        let f10 = f20 >> 1;
        let f5 = f10 >> 1;
        let f2_5 = f5 >> 1;
        if frame_size < f2_5 { return Err(Error::Invalid("Opus buffer too small")); }
        let mut frame_size = frame_size.min(FS / 25 * 3);
        let mut data = data;
        let mut len = data.map(|d| d.len()).unwrap_or(0) as i32;
        if len <= 1 {
            data = None;
            frame_size = frame_size.min(self.frame_size);
        }
        let mut audiosize;
        let mode;
        let bandwidth;
        if data.is_some() {
            audiosize = self.frame_size;
            mode = self.mode;
            bandwidth = self.bandwidth;
        } else {
            audiosize = frame_size;
            mode = if self.prev_redundancy { MODE_CELT_ONLY } else { self.prev_mode };
            bandwidth = 0;
            if mode == 0 {
                for v in pcm[..(audiosize as usize) * ch].iter_mut() { *v = 0; }
                return Ok(audiosize);
            }
            if audiosize > f20 {
                let mut off = 0usize;
                loop {
                    let ret = self.decode_frame(None, &mut pcm[off..], audiosize.min(f20), false)?;
                    off += ret as usize * ch;
                    audiosize -= ret;
                    if audiosize <= 0 { break; }
                }
                return Ok(frame_size);
            } else if audiosize < f20 {
                if audiosize > f10 { audiosize = f10; } else if mode != MODE_SILK_ONLY && audiosize > f5 && audiosize < f10 { audiosize = f5; }
            }
        }
        let empty: [u8; 0] = [];
        let dbytes: &[u8] = data.unwrap_or(&empty);
        let mut dec = RangeDecoder::new(dbytes);
        let celt_accum = mode != MODE_CELT_ONLY && frame_size >= f10;
        let mut transition = false;
        if data.is_some() && self.prev_mode > 0
            && ((mode == MODE_CELT_ONLY && self.prev_mode != MODE_CELT_ONLY && !self.prev_redundancy)
                || (mode != MODE_CELT_ONLY && self.prev_mode == MODE_CELT_ONLY))
        {
            transition = true;
        }
        let mut pcm_transition = vec![0i16; (f5 as usize) * ch];
        if transition && mode == MODE_CELT_ONLY {
            self.decode_frame(None, &mut pcm_transition, f5.min(audiosize), false)?;
        }
        if audiosize > frame_size { return Err(Error::Invalid("Opus buffer too small")); }
        frame_size = audiosize;
        let fsz = frame_size as usize;
        let mut pcm_silk = vec![0i16; if mode != MODE_CELT_ONLY && !celt_accum { (f10.max(frame_size) as usize) * ch } else { 0 }];
        if mode != MODE_CELT_ONLY {
            if self.prev_mode == MODE_CELT_ONLY { self.silk.reset(); }
            self.ctl.payload_size_ms = 10.max(1000 * audiosize / FS);
            if data.is_some() {
                self.ctl.n_channels_internal = self.stream_channels;
                self.ctl.internal_sample_rate = if mode == MODE_SILK_ONLY {
                    match bandwidth { BW_NB => 8000, BW_MB => 12000, _ => 16000 }
                } else { 16000 };
            }
            let lost_flag = if data.is_none() { 1 } else { 2 * decode_fec as i32 };
            let mut decoded = 0i32;
            let mut off = 0usize;
            loop {
                let first = decoded == 0;
                let target: &mut [i16] = if celt_accum { &mut pcm[off..] } else { &mut pcm_silk[off..] };
                let r = self.silk.decode(&mut self.ctl, lost_flag, first, &mut dec, target);
                let n = match r {
                    Ok(n) => n as i32,
                    Err(e) => {
                        if lost_flag != 0 {
                            for v in target[..fsz * ch].iter_mut() { *v = 0; }
                            frame_size
                        } else {
                            return Err(e);
                        }
                    }
                };
                off += n as usize * ch;
                decoded += n;
                if decoded >= frame_size { break; }
            }
        }
        let mut start_band = 0;
        let mut redundancy = false;
        let mut redundancy_bytes = 0i32;
        let mut celt_to_silk = false;
        if !decode_fec && mode != MODE_CELT_ONLY && data.is_some() && dec.tell() + 17 + 20 * (mode == MODE_HYBRID) as i32 <= 8 * len {
            redundancy = if mode == MODE_HYBRID { dec.bit_logp(12) } else { true };
            if redundancy {
                celt_to_silk = dec.bit_logp(1);
                redundancy_bytes = if mode == MODE_HYBRID { dec.dec_uint(256) as i32 + 2 } else { len - ((dec.tell() + 7) >> 3) };
                len -= redundancy_bytes;
                if len * 8 < dec.tell() {
                    len = 0;
                    redundancy_bytes = 0;
                    redundancy = false;
                }
                dec.storage = dec.storage.wrapping_sub(redundancy_bytes as u32);
            }
        }
        if mode != MODE_CELT_ONLY { start_band = 17; }
        if redundancy { transition = false; }
        if transition && mode != MODE_CELT_ONLY {
            self.decode_frame(None, &mut pcm_transition, f5.min(audiosize), false)?;
        }
        if bandwidth != 0 {
            self.celt.end = match bandwidth { BW_NB => 13, BW_MB | BW_WB => 17, BW_SWB => 19, _ => 21 };
        }
        self.celt.stream_channels = self.stream_channels;
        let mut redundant_audio = vec![0i16; if redundancy { (f5 as usize) * ch } else { 0 }];
        let mut redundant_rng = 0u32;
        let red_data: &[u8] = if redundancy { &dbytes[(len as usize).min(dbytes.len())..((len + redundancy_bytes) as usize).min(dbytes.len())] } else { &empty };
        if redundancy && celt_to_silk {
            self.celt.start = 0;
            let _ = self.celt.decode(Some(red_data), &mut redundant_audio, f5 as usize, None, false);
            redundant_rng = self.celt.rng;
        }
        self.celt.start = start_band;
        let celt_data = &dbytes[..(len.max(0) as usize).min(dbytes.len())];
        let mut celt_err = None;
        if mode != MODE_SILK_ONLY {
            let celt_frame_size = f20.min(frame_size);
            if mode != self.prev_mode && self.prev_mode > 0 && !self.prev_redundancy { self.celt.reset(); }
            let d = if decode_fec { None } else if data.is_some() { Some(celt_data) } else { None };
            if let Err(e) = self.celt.decode(d, pcm, celt_frame_size as usize, Some(&mut dec), celt_accum) { celt_err = Some(e); }
        } else {
            let silence = [0xFFu8, 0xFF];
            if !celt_accum { for v in pcm[..fsz * ch].iter_mut() { *v = 0; } }
            if self.prev_mode == MODE_HYBRID && !(redundancy && celt_to_silk && self.prev_redundancy) {
                self.celt.start = 0;
                let _ = self.celt.decode(Some(&silence), pcm, f2_5 as usize, None, celt_accum);
            }
        }
        if mode != MODE_CELT_ONLY && !celt_accum {
            for i in 0..fsz * ch { pcm[i] = (pcm[i] as i32 + pcm_silk[i] as i32).clamp(-32768, 32767) as i16; }
        }
        let f2 = f2_5 as usize;
        if redundancy && !celt_to_silk {
            self.celt.reset();
            self.celt.start = 0;
            let _ = self.celt.decode(Some(red_data), &mut redundant_audio, f5 as usize, None, false);
            redundant_rng = self.celt.rng;
            let o = ch * (fsz - f2);
            let a: alloc::vec::Vec<i16> = pcm[o..o + f2 * ch].to_vec();
            smooth_fade(&a, &redundant_audio[ch * f2..], &mut pcm[o..], f2, ch);
        }
        if redundancy && celt_to_silk && (self.prev_mode != MODE_SILK_ONLY || self.prev_redundancy) {
            for c in 0..ch { for i in 0..f2 { pcm[ch * i + c] = redundant_audio[ch * i + c]; } }
            let b: alloc::vec::Vec<i16> = pcm[ch * f2..ch * 2 * f2].to_vec();
            smooth_fade(&redundant_audio[ch * f2..], &b, &mut pcm[ch * f2..], f2, ch);
        }
        if transition {
            if audiosize >= f5 {
                pcm[..ch * f2].copy_from_slice(&pcm_transition[..ch * f2]);
                let b: alloc::vec::Vec<i16> = pcm[ch * f2..ch * 2 * f2].to_vec();
                smooth_fade(&pcm_transition[ch * f2..], &b, &mut pcm[ch * f2..], f2, ch);
            } else {
                let b: alloc::vec::Vec<i16> = pcm[..ch * f2].to_vec();
                smooth_fade(&pcm_transition, &b, pcm, f2, ch);
            }
        }
        if self.decode_gain != 0 {
            let gain = super::celt::fixed::celt_exp2(super::celt::fixed::mult16_16_p15(21771, self.decode_gain));
            for v in pcm[..fsz * ch].iter_mut() {
                let x = super::celt::fixed::mult16_32_p16(*v as i32, gain);
                *v = x.clamp(-32767, 32767) as i16;
            }
        }
        self.range_final = if len <= 1 { 0 } else { dec.rng ^ redundant_rng };
        self.prev_mode = mode;
        self.prev_redundancy = redundancy && !celt_to_silk;
        if let Some(e) = celt_err { return Err(e); }
        Ok(audiosize)
    }

    /// `opus_decode`: one packet (or `None` for a lost one, concealing `frame_size` samples) into interleaved
    /// 16-bit `pcm` (capacity `frame_size` per channel). Returns samples per channel.
    pub fn decode(&mut self, data: Option<&[u8]>, pcm: &mut [i16], frame_size: usize) -> Result<usize> {
        self.decode_ext(data, pcm, frame_size, false)
    }

    /// `opus_decode` with `decode_fec`: recover the lost previous frame from this packet's SILK LBRR data
    /// (in-band FEC), concealing whatever part of `frame_size` the FEC does not cover.
    pub fn decode_ext(&mut self, data: Option<&[u8]>, pcm: &mut [i16], frame_size: usize, decode_fec: bool) -> Result<usize> {
        let frame_size = frame_size as i32;
        let data = match data { Some(d) if !d.is_empty() => d, _ => {
            if frame_size % (FS / 400) != 0 { return Err(Error::Invalid("Opus PLC size")); }
            let mut count = 0i32;
            while count < frame_size {
                let r = self.decode_frame(None, &mut pcm[count as usize * self.channels..], frame_size - count, false)?;
                count += r;
            }
            self.last_packet_duration = count;
            return Ok(count as usize);
        } };
        let toc = data[0];
        let pmode = packet_mode(toc);
        let pbw = packet_bandwidth(toc);
        let pfs = packet_samples_per_frame(toc, FS);
        let psc = packet_channels(toc);
        let (_, off, sizes) = parse_packet(data)?;
        if decode_fec {
            if frame_size < pfs || pmode == MODE_CELT_ONLY || self.mode == MODE_CELT_ONLY {
                return self.decode_ext(None, pcm, frame_size as usize, false);
            }
            let duration_copy = self.last_packet_duration;
            if frame_size - pfs != 0 {
                if let Err(e) = self.decode_ext(None, pcm, (frame_size - pfs) as usize, false) {
                    self.last_packet_duration = duration_copy;
                    return Err(e);
                }
            }
            self.mode = pmode;
            self.bandwidth = pbw;
            self.frame_size = pfs;
            self.stream_channels = psc;
            let o = self.channels * (frame_size - pfs) as usize;
            self.decode_frame(Some(&data[off..off + sizes[0]]), &mut pcm[o..], pfs, true)?;
            self.last_packet_duration = frame_size;
            return Ok(frame_size as usize);
        }
        if sizes.len() as i32 * pfs > frame_size { return Err(Error::Invalid("Opus buffer too small")); }
        self.mode = pmode;
        self.bandwidth = pbw;
        self.frame_size = pfs;
        self.stream_channels = psc;
        let mut nb = 0i32;
        let mut p = off;
        for &s in sizes.iter() {
            let r = self.decode_frame(Some(&data[p..p + s]), &mut pcm[nb as usize * self.channels..], frame_size - nb, false)?;
            p += s;
            nb += r;
        }
        self.last_packet_duration = nb;
        Ok(nb as usize)
    }
}

/// `opus_packet_has_lbrr`: does this packet's first SILK frame carry in-band FEC data?
pub fn packet_has_lbrr(data: &[u8]) -> bool {
    if data.is_empty() || packet_mode(data[0]) == MODE_CELT_ONLY { return false; }
    let pfs = packet_samples_per_frame(data[0], 48000);
    let nb_frames = if pfs > 960 { pfs / 960 } else { 1 };
    let Ok((_, off, sizes)) = parse_packet(data) else { return false };
    if sizes[0] == 0 { return false; }
    let f0 = data[off] as i32;
    let mut lbrr = (f0 >> (7 - nb_frames)) & 1;
    if packet_channels(data[0]) == 2 { lbrr |= (f0 >> (6 - 2 * nb_frames)) & 1; }
    lbrr != 0
}
