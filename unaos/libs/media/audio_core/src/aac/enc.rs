// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core: the AAC-LC encoder (ISO/IEC 14496-3 §4), the decoder's tables and transform reused.
//!
//! NLE6EXPORTAAC (rmbp-ledger B960; handlers.md §5 NLE6's NLE6EXPORT-AAC). The editor's export (Stria's `nle export
//! --audio aac`, the kernel's `nle export --aac KBPS`) encodes its sound here; `no_std` + `alloc`, both rings link it.
//!
//! * ONE TRANSFORM: the forward MDCT is the windowed 2048-sample block folded into 1024 values and put through the
//!   decoder's OWN inverse transform (`crate::vorbis::Imdct::inverse`, the object `AacDecoder` builds): the DCT-IV is
//!   its own inverse, so the forward spectrum is read back from the middle half of that call's output. Scale: X = 2·Σ
//!   w·x·cos in i16 units, the pair of the decoder's 2/N/32768 synthesis (exact reconstruction, `aac_enc_sine_snr`).
//! * ONE TABLE SET: the scalefactor and spectral Huffman codes are `super::tables`' `SF_CODE`/`SF_BITS`/`SPEC_CODES`/
//!   `SPEC_BITS`, the bands `SWB_1024`/`NUM_SWB_1024`, the window the decoder's `sine`, |q|^(4/3) its `pow43_table`.
//! * LONG WINDOWS ONLY: every block is ONLY_LONG with the sine shape. Short blocks / block switching are NOT built
//!   (AACENC-SHORT owed: a transient pre-echoes); neither are M/S (stereo is a CPE of two independent channels,
//!   `common_window` 0), TNS, PNS, intensity, SBR or a bit reservoir.
//! * THE QUANTISER (the two loops): a per-band threshold from the band's energy at a fixed SNR offset (floored at an
//!   absolute level and at the frame's mask floor, `MASK_DB` under its loudest band per coefficient, so a tone's window
//!   leakage is not coded at the tone's SNR; measured on `aac_enc_sine_snr`'s 1 kHz tone at 128 kb/s stereo, 1356 bits a
//!   channel: MASK_DB 40 -> 40.2 dB at 75 B/frame, 50 -> 49.6 dB at 155 B, 60 (taken) -> 56.8 dB at 247 B; with no floor
//!   14.4 dB, the leakage bands eating the budget) — the OUTER loop takes, per band, the coarsest scalefactor whose measured quantisation noise stays
//!   under it; the INNER loop shifts every band by one global offset, binary-searched so the frame's counted bits fit
//!   the per-channel budget (`bitrate · 1024 / rate / channels`); a band above the bitrate's cutoff or quantised to
//!   nothing is ZERO_HCB; consecutive scalefactors are held within ±60 by coarsening the finer band.
//! * OUTPUT: one raw_data_block per [`AccessUnit`] (MP4's sample), [`Encoder::adts`] frames one for a `.aac` stream,
//!   [`Encoder::asc`] is the AudioSpecificConfig. The first unit carries the 1024-sample MDCT priming ([`PRIMING`]): an
//!   MP4 writer trims it with an edit list (demux_core `build::mp4a_edit_list`).
use alloc::vec;
use alloc::vec::Vec;

use super::decoder::{pow43_table, sine};
use super::tables as t;
use crate::math;
use crate::vorbis::Imdct;
use crate::{Error, Result};

/// Samples per channel per access unit.
pub const FRAME: usize = 1024;
/// The decoder's output leads the input by one frame (the MDCT overlap): the samples an edit list skips.
pub const PRIMING: u32 = 1024;

/// One encoded raw_data_block and its first sample's position (in samples per channel, the priming included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessUnit {
    pub data: Vec<u8>,
    pub pts: u64,
}

/// Fixed signal-to-mask offset of the per-band threshold (dB) and its absolute floor (noise energy per coefficient,
/// i16 units: well under one LSB of white noise, which is ~4096 in this spectrum's scale).
const SNR_DB: f64 = 18.0;
const ATH: f64 = 1000.0;
/// The frame's mask floor: no band's noise need sit lower than this many dB under the loudest band (per coefficient).
const MASK_DB: f64 = 60.0;
/// The largest quantised magnitude the escape codebook carries (§4.6.1: 8191).
const MAX_Q: i32 = 8191;

/// The private MSB-first bit writer (BITWRITER-ONE owed: h264enc_core's lives in a crate this one does not link).
struct Bits {
    buf: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn new() -> Bits { Bits { buf: Vec::new(), acc: 0, n: 0 } }
    fn put(&mut self, n: u32, v: u32) {
        for i in (0..n).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 8 { self.buf.push(self.acc as u8); self.acc = 0; self.n = 0; }
        }
    }
    /// Zero-padded to the byte (the raw_data_block's own alignment).
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 { self.buf.push((self.acc << (8 - self.n)) as u8); }
        self.buf
    }
}

/// One channel's coded frame.
struct Coded {
    max_sfb: usize,
    global_gain: i32,
    /// per band (0..max_sfb): codebook (0 = ZERO_HCB) and scalefactor
    cb: Vec<u8>,
    sf: Vec<i32>,
    q: Vec<i32>,
    bits: usize,
}

/// The AAC-LC encoder: 1 or 2 channels at one of the 13 AAC rates, a target bitrate.
pub struct Encoder {
    rate: u32,
    channels: usize,
    sf_index: usize,
    bitrate: u32,
    mdct: Imdct,
    /// the sine window's rising half (1024)
    win: Vec<f32>,
    pow43: Vec<f32>,
    /// per channel: the previous block (the first half of the next MDCT input)
    hist: Vec<Vec<f32>>,
    /// interleaved input short of a frame
    pend: Vec<i16>,
    units: u64,
    /// bands at or above this one are never coded (the bitrate's bandwidth)
    max_band: usize,
    budget: usize,
    flushed: bool,
}

impl Encoder {
    /// `bitrate` in bits per second (the frame budget; there is no reservoir). Refused: a rate that is not an AAC
    /// sampling rate, more than two channels, a zero bitrate.
    pub fn new(rate: u32, channels: u16, bitrate: u32) -> Result<Encoder> {
        let sf_index = super::RATES.iter().position(|&r| r == rate).ok_or(Error::Unsupported("aac-enc: sample rate (not an AAC rate)"))?;
        if !(1..=2).contains(&channels) { return Err(Error::Unsupported("aac-enc: channels (mono or stereo only)")); }
        if bitrate == 0 { return Err(Error::Invalid("aac-enc: bitrate 0")); }
        let ch = channels as usize;
        let total = (bitrate as u64 * FRAME as u64 / rate as u64) as usize;
        // frame overhead: element id + tag (+ common_window), ID_END, the alignment
        let budget = (total.saturating_sub(3 + 4 + 1 + 3 + 7) / ch).clamp(64, 6144 - 16);
        let bpc = bitrate / channels as u32;
        let cutoff = match bpc { b if b >= 96_000 => 20_000, b if b >= 64_000 => 16_000, b if b >= 48_000 => 14_000, b if b >= 32_000 => 11_000, b if b >= 24_000 => 8_000, _ => 6_000 }.min(rate / 2);
        let bin = (cutoff as u64 * 2 * FRAME as u64 / rate as u64) as u16;
        let swb = t::SWB_1024[sf_index];
        let num_swb = t::NUM_SWB_1024[sf_index] as usize;
        let max_band = (0..num_swb).take_while(|&b| swb[b] < bin).count();
        Ok(Encoder {
            rate,
            channels: ch,
            sf_index,
            bitrate,
            mdct: Imdct::new(2048, [2048, 2048]),
            win: sine(2048),
            pow43: pow43_table(),
            hist: vec![vec![0.0; FRAME]; ch],
            pend: Vec::new(),
            units: 0,
            max_band,
            budget,
            flushed: false,
        })
    }

    pub fn rate(&self) -> u32 { self.rate }
    pub fn channels(&self) -> u16 { self.channels as u16 }
    pub fn bitrate(&self) -> u32 { self.bitrate }
    pub fn sf_index(&self) -> usize { self.sf_index }

    /// The AudioSpecificConfig (§1.6.2.1): AAC LC (2), the sampling frequency index, the channel configuration, a
    /// GASpecificConfig of 1024-sample frames, no core coder, no extension.
    pub fn asc(&self) -> [u8; 2] {
        let v = (2u16 << 11) | ((self.sf_index as u16) << 7) | ((self.channels as u16) << 3);
        v.to_be_bytes()
    }

    /// Interleaved PCM in; every whole frame out as an access unit.
    pub fn encode(&mut self, pcm: &[i16]) -> Vec<AccessUnit> {
        self.pend.extend_from_slice(pcm);
        let n = FRAME * self.channels;
        let mut out = Vec::new();
        while self.pend.len() >= n {
            let block: Vec<i16> = self.pend.drain(..n).collect();
            out.push(self.frame(&block));
        }
        out
    }

    /// The end: the partial frame padded with silence, then one silent frame that drains the overlap. Encoding
    /// `L` frames in all yields `ceil(L / 1024) + 1` units. A second call yields nothing.
    pub fn flush(&mut self) -> Vec<AccessUnit> {
        if self.flushed { return Vec::new(); }
        self.flushed = true;
        let n = FRAME * self.channels;
        let mut out = Vec::new();
        if !self.pend.is_empty() {
            let mut block = core::mem::take(&mut self.pend);
            block.resize(n, 0);
            out.push(self.frame(&block));
        }
        out.push(self.frame(&vec![0i16; n]));
        out
    }

    /// One unit as an ADTS frame (§1.A.2: 7-byte header, no CRC, buffer fullness 0x7FF, one raw block).
    pub fn adts(&self, au: &AccessUnit) -> Vec<u8> {
        let len = au.data.len() + 7;
        let cc = self.channels as u8;
        let mut h = vec![
            0xFF,
            0xF1,
            (1 << 6) | ((self.sf_index as u8) << 2) | ((cc >> 2) & 1),
            ((cc & 3) << 6) | ((len >> 11) & 3) as u8,
            ((len >> 3) & 0xFF) as u8,
            (((len & 7) << 5) as u8) | 0x1F,
            0xFC,
        ];
        h.extend_from_slice(&au.data);
        h
    }

    /// The forward MDCT of a windowed 2048-sample block (1024 coefficients): the fold, the decoder's inverse, the
    /// middle half read back (v[j] = -y[3N/4 - 1 - j]), scaled 2·Σ.
    fn mdct(&self, z: &[f32]) -> Vec<f32> {
        const N4: usize = 512;
        let mut u = vec![0f32; 1024];
        for m in 0..N4 { u[m] = -z[3 * N4 - 1 - m] - z[3 * N4 + m]; }
        for m in N4..1024 { u[m] = z[m - N4] - z[3 * N4 - 1 - m]; }
        let mut y = vec![0f32; 2048];
        self.mdct.inverse(&u, &mut y);
        (0..1024).map(|j| -2.0 * y[3 * N4 - 1 - j]).collect()
    }

    fn frame(&mut self, block: &[i16]) -> AccessUnit {
        let ch = self.channels;
        let mut coded = Vec::with_capacity(ch);
        for c in 0..ch {
            let mut z = vec![0f32; 2048];
            for n in 0..FRAME {
                z[n] = self.hist[c][n] * self.win[n];
                let s = block[n * ch + c] as f32;
                z[FRAME + n] = s * self.win[FRAME - 1 - n];
                self.hist[c][n] = s;
            }
            let x = self.mdct(&z);
            coded.push(self.code(&x));
        }
        let mut w = Bits::new();
        if ch == 1 {
            w.put(3, super::decoder::ID_SCE);
            w.put(4, 0);
            self.write_ics(&mut w, &coded[0]);
        } else {
            w.put(3, super::decoder::ID_CPE);
            w.put(4, 0);
            w.put(1, 0); // common_window: two independent channels (M/S owed)
            self.write_ics(&mut w, &coded[0]);
            self.write_ics(&mut w, &coded[1]);
        }
        w.put(3, super::decoder::ID_END);
        let au = AccessUnit { data: w.finish(), pts: self.units * FRAME as u64 };
        self.units += 1;
        au
    }

    /// The two loops on one channel's spectrum.
    fn code(&self, x: &[f32]) -> Coded {
        let swb = t::SWB_1024[self.sf_index];
        let nb = self.max_band;
        let a: Vec<f32> = x.iter().map(|&v| math::pow(v.abs() as f64, 0.75) as f32).collect();
        // the OUTER loop: per band, the coarsest scalefactor within its threshold (None: the band is not coded)
        let mut base: Vec<Option<(i32, i32)>> = Vec::with_capacity(nb); // (sf, the finest sf without overflow)
        let energy: Vec<f64> = (0..nb).map(|b| x[swb[b] as usize..swb[b + 1] as usize].iter().map(|&v| v as f64 * v as f64).sum()).collect();
        // the frame's mask floor: MASK_DB under its loudest band's energy per coefficient
        let floor = (0..nb).map(|b| energy[b] / (swb[b + 1] - swb[b]) as f64).fold(0f64, f64::max) * math::exp2(-MASK_DB / 10.0 * core::f64::consts::LOG2_10);
        for b in 0..nb {
            let (lo, hi) = (swb[b] as usize, swb[b + 1] as usize);
            let e = energy[b];
            let allowed = (e * math::exp2(-SNR_DB / 10.0 * core::f64::consts::LOG2_10)).max((ATH.max(floor)) * (hi - lo) as f64);
            if e <= allowed { base.push(None); continue; }
            // the finest legal sf: |q| <= MAX_Q (maxq falls as sf rises)
            let (mut l, mut h) = (0i32, 255i32);
            while l < h { let m = (l + h) / 2; if self.band(x, &a, lo, hi, m).1 <= MAX_Q { h = m; } else { l = m + 1; } }
            let fine = l;
            // the coarsest sf whose noise stays under the threshold
            let (mut l, mut h) = (fine, 255i32);
            while l < h { let m = (l + h + 1) / 2; if self.band(x, &a, lo, hi, m).0 <= allowed { l = m; } else { h = m - 1; } }
            base.push(Some((l, fine)));
        }
        // the INNER loop: the smallest global offset whose frame fits the budget
        let (mut l, mut h) = (-60i32, 60i32);
        while l < h {
            let m = l + (h - l) / 2;
            if self.plan(x, &a, &base, m, nb).bits <= self.budget { h = m; } else { l = m + 1; }
        }
        let mut c = self.plan(x, &a, &base, l, nb);
        // still over at the coarsest offset: drop the top bands until it fits
        let mut top = nb;
        while c.bits > self.budget && top > 0 {
            top -= 1;
            c = self.plan(x, &a, &base, 60, top);
        }
        c
    }

    /// Band `lo..hi` quantised at `sf`: (the noise energy, the largest |q|).
    fn band(&self, x: &[f32], a: &[f32], lo: usize, hi: usize, sf: i32) -> (f64, i32) {
        let qs = math::exp2(-0.1875 * (sf - 100) as f64) as f32;
        let ds = math::exp2(0.25 * (sf - 100) as f64) as f32;
        let mut noise = 0f64;
        let mut mq = 0i32;
        for k in lo..hi {
            let q = (a[k] * qs + 0.4054) as i32;
            mq = mq.max(q);
            let d = if q < 8192 { self.pow43[q as usize] * ds } else { 0.0 };
            let r = (x[k].abs() - d) as f64;
            noise += r * r;
        }
        (noise, mq)
    }

    /// Quantise every band at its base sf + `g` (never finer than its overflow limit), hold the scalefactor chain within
    /// ±60, pick the codebooks, count the bits. Bands `top..` are not coded.
    fn plan(&self, x: &[f32], a: &[f32], base: &[Option<(i32, i32)>], g: i32, top: usize) -> Coded {
        let swb = t::SWB_1024[self.sf_index];
        let mut q = vec![0i32; 1024];
        let mut sf: Vec<i32> = vec![0; top];
        let mut on = vec![false; top];
        let quant = |q: &mut [i32], b: usize, s: i32| -> bool {
            let qs = math::exp2(-0.1875 * (s - 100) as f64) as f32;
            let mut any = false;
            for k in swb[b] as usize..swb[b + 1] as usize {
                let m = ((a[k] * qs + 0.4054) as i32).min(MAX_Q);
                q[k] = if x[k] < 0.0 { -m } else { m };
                any |= m != 0;
            }
            any
        };
        for b in 0..top {
            if let Some((s, fine)) = base[b] {
                let s = (s + g).clamp(fine, 255);
                sf[b] = s;
                on[b] = quant(&mut q, b, s);
            }
        }
        // the chain: consecutive coded scalefactors differ by at most 60 — the finer band is coarsened (never finer, so
        // never an overflow); a band coarsened to nothing drops out and the chain is checked again
        loop {
            let idx: Vec<usize> = (0..top).filter(|&b| on[b]).collect();
            let mut changed = false;
            for p in idx.windows(2) {
                let (i, j) = (p[0], p[1]);
                if sf[j] - sf[i] > 60 { sf[i] = sf[j] - 60; on[i] = quant(&mut q, i, sf[i]); changed = true; break; }
                if sf[i] - sf[j] > 60 { sf[j] = sf[i] - 60; on[j] = quant(&mut q, j, sf[j]); changed = true; break; }
            }
            if !changed { break; }
        }
        for b in 0..top {
            if !on[b] { for k in swb[b] as usize..swb[b + 1] as usize { q[k] = 0; } }
        }
        let max_sfb = (0..top).rev().find(|&b| on[b]).map(|b| b + 1).unwrap_or(0);
        let mut cb = vec![0u8; max_sfb];
        let mut bits = 8 + 11 + 3; // global_gain, ics_info, pulse/tns/gain_control flags
        for b in 0..max_sfb {
            if on[b] {
                let (book, n) = best_book(&q[swb[b] as usize..swb[b + 1] as usize]);
                cb[b] = book;
                bits += n;
            }
        }
        // section data: runs of one codebook
        let mut b = 0;
        while b < max_sfb {
            let mut e = b;
            while e < max_sfb && cb[e] == cb[b] { e += 1; }
            bits += 4 + 5 * ((e - b) / 31 + 1);
            b = e;
        }
        let global_gain = (0..max_sfb).find(|&b| cb[b] != 0).map(|b| sf[b]).unwrap_or(100);
        let mut prev = global_gain;
        for b in 0..max_sfb {
            if cb[b] != 0 { bits += t::SF_BITS[(sf[b] - prev + 60) as usize] as usize; prev = sf[b]; }
        }
        sf.truncate(max_sfb);
        Coded { max_sfb, global_gain, cb, sf, q, bits }
    }

    /// individual_channel_stream (§4.4.2.7), ONLY_LONG, sine window, no pulse / TNS / gain control.
    fn write_ics(&self, w: &mut Bits, c: &Coded) {
        let swb = t::SWB_1024[self.sf_index];
        w.put(8, c.global_gain as u32);
        w.put(1, 0); // ics_reserved_bit
        w.put(2, 0); // ONLY_LONG_SEQUENCE
        w.put(1, 0); // window_shape: sine
        w.put(6, c.max_sfb as u32);
        w.put(1, 0); // predictor_data_present
        let mut b = 0;
        while b < c.max_sfb {
            let mut e = b;
            while e < c.max_sfb && c.cb[e] == c.cb[b] { e += 1; }
            w.put(4, c.cb[b] as u32);
            let mut len = e - b;
            while len >= 31 { w.put(5, 31); len -= 31; }
            w.put(5, len as u32);
            b = e;
        }
        let mut prev = c.global_gain;
        for b in 0..c.max_sfb {
            if c.cb[b] != 0 {
                let i = (c.sf[b] - prev + 60) as usize;
                w.put(t::SF_BITS[i] as u32, t::SF_CODE[i]);
                prev = c.sf[b];
            }
        }
        w.put(1, 0); // pulse_data_present
        w.put(1, 0); // tns_data_present
        w.put(1, 0); // gain_control_data_present
        for b in 0..c.max_sfb {
            if c.cb[b] != 0 { write_band(w, c.cb[b], &c.q[swb[b] as usize..swb[b + 1] as usize]); }
        }
    }
}

/// The largest magnitude each spectral codebook carries, whether it is signed (§4.6.3, Table 4.152).
fn book_range(cb: u8) -> (i32, bool) {
    match cb { 1 | 2 => (1, true), 3 | 4 => (2, false), 5 | 6 => (4, true), 7 | 8 => (7, false), 9 | 10 => (12, false), _ => (MAX_Q, false) }
}

/// The codeword index of a quad (books 1-4) or pair (5-11): the inverse of `decode_ics`'s unpacking.
fn index(cb: u8, v: &[i32]) -> usize {
    match cb {
        1 | 2 => (27 * (v[0] + 1) + 9 * (v[1] + 1) + 3 * (v[2] + 1) + (v[3] + 1)) as usize,
        3 | 4 => (27 * v[0].abs() + 9 * v[1].abs() + 3 * v[2].abs() + v[3].abs()) as usize,
        5 | 6 => (9 * (v[0] + 4) + (v[1] + 4)) as usize,
        _ => {
            let m = match cb { 7 | 8 => 8, 9 | 10 => 13, _ => 17 };
            (m * v[0].abs().min(16) + v[1].abs().min(16)) as usize
        }
    }
}

/// The escape's length for |v| >= 16: N ones, a zero, N + 4 bits.
fn esc_n(v: i32) -> u32 { (31 - (v as u32).leading_zeros()) - 4 }

/// Bits of `q` in codebook `cb` (codeword + sign bits + escapes).
fn book_bits(cb: u8, q: &[i32]) -> usize {
    let (_, signed) = book_range(cb);
    let step = if cb <= 4 { 4 } else { 2 };
    let lens = t::SPEC_BITS[cb as usize - 1];
    let mut n = 0usize;
    for v in q.chunks(step) {
        n += lens[index(cb, v)] as usize;
        if !signed { n += v.iter().filter(|&&x| x != 0).count(); }
        if cb == 11 { for &x in v { if x.abs() >= 16 { n += 2 * esc_n(x.abs()) as usize + 5; } } }
    }
    n
}

/// The cheapest codebook that carries the band: (book, bits).
fn best_book(q: &[i32]) -> (u8, usize) {
    let m = q.iter().map(|x| x.abs()).max().unwrap_or(0);
    if m == 0 { return (0, 0); }
    (1u8..=11).filter(|&cb| m <= book_range(cb).0).map(|cb| (cb, book_bits(cb, q))).min_by_key(|&(_, n)| n).unwrap_or((11, 0))
}

/// spectral_data for one band in codebook `cb` (§4.4.2.7, Table 4.50; the order `decode_ics` reads).
fn write_band(w: &mut Bits, cb: u8, q: &[i32]) {
    let (_, signed) = book_range(cb);
    let step = if cb <= 4 { 4 } else { 2 };
    let (codes, lens) = (t::SPEC_CODES[cb as usize - 1], t::SPEC_BITS[cb as usize - 1]);
    for v in q.chunks(step) {
        let i = index(cb, v);
        w.put(lens[i] as u32, codes[i] as u32);
        if !signed { for &x in v { if x != 0 { w.put(1, (x < 0) as u32); } } }
        if cb == 11 {
            for &x in v {
                let m = x.abs();
                if m >= 16 {
                    let n = esc_n(m);
                    for _ in 0..n { w.put(1, 1); }
                    w.put(1, 0);
                    w.put(n + 4, (m - (1 << (n + 4))) as u32);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aac::{AacDecoder, AdtsStream, Asc, Layout};
    use crate::bits::BitReader;

    /// Decode raw units through OUR decoder: per channel, i16-scaled f64.
    fn decode(e: &Encoder, units: &[AccessUnit]) -> Vec<Vec<f64>> {
        let mut d = AacDecoder::new(e.sf_index(), Layout::from_config(e.channels() as u8).unwrap()).unwrap();
        let mut out = vec![Vec::new(); e.channels() as usize];
        for u in units {
            let mut r = BitReader::new(&u.data);
            d.decode_block(&mut r, 0).unwrap();
            for (c, o) in out.iter_mut().enumerate() { o.extend(d.out[c].iter().map(|&v| v as f64 * 32768.0)); }
        }
        out
    }

    fn sine_pcm(rate: u32, frames: usize, hz: f64, amp: f64) -> Vec<i16> {
        let mut v = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let p = 2.0 * core::f64::consts::PI * hz * n as f64 / rate as f64;
            v.push((amp * math::sin(p)) as i16);
            v.push((amp * math::sin(p + 0.7)) as i16);
        }
        v
    }

    /// The forward transform (the fold over the decoder's `Imdct::inverse`) against the definition, N = 2048.
    #[test]
    fn aac_enc_mdct_matches_direct() {
        let e = Encoder::new(48000, 1, 64000).unwrap();
        let z: Vec<f32> = (0..2048).map(|n| (math::sin(n as f64 * 0.37) * 1000.0 + math::cos(n as f64 * 0.011) * 300.0) as f32).collect();
        let x = e.mdct(&z);
        let mut worst = 0f64;
        let mut peak = 0f64;
        for k in (0..1024).step_by(37) {
            let mut s = 0f64;
            for n in 0..2048 {
                s += z[n] as f64 * math::cos(2.0 * core::f64::consts::PI / 2048.0 * (n as f64 + 0.5 + 512.0) * (k as f64 + 0.5));
            }
            worst = worst.max((2.0 * s - x[k] as f64).abs());
            peak = peak.max((2.0 * s).abs());
        }
        assert!(worst <= peak * 1e-4, "worst {worst} peak {peak}");
    }

    /// The AudioSpecificConfig bytes, and our parser reads them back.
    #[test]
    fn aac_enc_asc_bytes() {
        let a = Encoder::new(48000, 2, 128000).unwrap().asc();
        assert_eq!(a, [0x11, 0x90]);
        let b = Encoder::new(44100, 1, 64000).unwrap().asc();
        assert_eq!(b, [0x12, 0x08]);
        let p = Asc::parse(&a).unwrap();
        assert_eq!((p.object_type, p.sf_index, p.channel_config, p.sbr), (2, 3, 2, false));
        assert!(Encoder::new(44000, 2, 128000).is_err());
        assert!(Encoder::new(48000, 3, 128000).is_err());
    }

    /// A 1 kHz sine at 128 kb/s stereo decodes through OUR decoder at >= 30 dB SNR, within the bitrate. Measured: 56.8 dB
    /// at MASK_DB 60 (49.6 at 50, 40.2 at 40, 14.4 with no mask floor; 34.0 / 54.3 dB at 3000 / 6000 bits a channel
    /// without it: the transform and bitstream are exact, the allocation is the psy model's).
    #[test]
    fn aac_enc_sine_snr() {
        let frames = 48000;
        let pcm = sine_pcm(48000, frames, 1000.0, 16000.0);
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        assert_eq!(units.len(), frames.div_ceil(1024) + 1);
        for (i, u) in units.iter().enumerate() { assert_eq!(u.pts, i as u64 * 1024); }
        let bytes: usize = units.iter().map(|u| u.data.len()).sum();
        let secs = units.len() as f64 * 1024.0 / 48000.0;
        assert!(bytes as f64 * 8.0 <= 128000.0 * secs * 1.05, "{bytes} bytes over {secs} s");
        let d = decode(&e, &units);
        for c in 0..2 {
            let (mut s, mut n) = (0f64, 0f64);
            for i in 2048..frames - 2048 {
                let x = pcm[i * 2 + c] as f64;
                let y = d[c][i + PRIMING as usize];
                s += x * x;
                n += (x - y) * (x - y);
            }
            let snr = 10.0 * math::log10(s / n.max(1e-9));
            assert!(snr >= 30.0, "channel {c}: SNR {snr:.1} dB");
        }
    }

    /// Silence costs a few bytes a frame (max_sfb 0 in both channels).
    #[test]
    fn aac_enc_silence_tiny() {
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        let mut units = e.encode(&vec![0i16; 4096 * 2]);
        units.extend(e.flush());
        assert_eq!(units.len(), 5);
        for u in &units { assert!(u.data.len() <= 8, "{} bytes", u.data.len()); }
        let d = decode(&e, &units);
        assert!(d.iter().all(|c| c.iter().all(|&v| v.abs() < 1.0)));
    }

    /// ADTS framing round trip: the frames through `AdtsStream` decode to what the raw units decode to.
    #[test]
    fn aac_enc_adts_round_trip() {
        use crate::Source;
        let pcm = sine_pcm(44100, 10000, 440.0, 8000.0);
        let mut e = Encoder::new(44100, 2, 96000).unwrap();
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let stream: Vec<u8> = units.iter().flat_map(|u| e.adts(u)).collect();
        let raw = decode(&e, &units);
        let bs = crate::io::ByteStream::new(alloc::boxed::Box::new(crate::io::VecReader::new(stream)));
        let mut s = AdtsStream::new(bs).unwrap();
        assert_eq!((s.info().rate, s.info().channels), (44100, 2));
        let mut got = vec![Vec::new(); 2];
        let mut pcm_b = crate::Pcm::default();
        while s.block(&mut pcm_b).unwrap() {
            for c in 0..2 { got[c].extend(pcm_b.flt[c][..pcm_b.frames].iter().map(|&v| v as f64 * 32768.0)); }
        }
        assert_eq!(got[0].len(), units.len() * 1024);
        for c in 0..2 { assert!(got[c].iter().zip(&raw[c]).all(|(a, b)| (a - b).abs() < 1e-3)); }
    }
}
