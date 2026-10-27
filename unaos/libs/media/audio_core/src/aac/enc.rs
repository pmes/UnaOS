// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core: the AAC-LC encoder (ISO/IEC 14496-3 §4), the decoder's tables and transform reused.
//!
//! NLE6EXPORTAAC (rmbp-ledger B960; handlers.md §5 NLE6's NLE6EXPORT-AAC). The editor's export (Stria's `nle export
//! --audio aac`, the kernel's `nle export --aac KBPS`) encodes its sound here; `no_std` + `alloc`, both rings link it.
//!
//! * ONE TRANSFORM: the forward MDCT is the windowed N-sample block folded into N/2 values and put through the
//!   decoder's OWN inverse transform (`crate::vorbis::Imdct::inverse`, the object `AacDecoder` builds): the DCT-IV is
//!   its own inverse, so the forward spectrum is read back from the middle half of that call's output. Scale: X = 2·Σ
//!   w·x·cos in i16 units, the pair of the decoder's 2/N/32768 synthesis (exact reconstruction, `aac_enc_sine_snr`).
//!   N is 2048 (long) or 256 (the eight short windows, AACENCSHORT LEDGER SR375): one fold, `Encoder::mdct`.
//! * ONE TABLE SET: the scalefactor and spectral Huffman codes are `super::tables`' `SF_CODE`/`SF_BITS`/`SPEC_CODES`/
//!   `SPEC_BITS`, the bands `SWB_1024`/`SWB_128`, the windows the decoder's `sine`, |q|^(4/3) its `pow43_table`.
//! * BLOCK SWITCHING (AACENCSHORT, LEDGER SR375): an attack detector (8 sub-blocks of 128 high-passed samples per block,
//!   an attack where one carries `ATTACK_RATIO` times the mean of the 8 before it and at least `ATTACK_MIN`) over a
//!   ONE-FRAME LOOKAHEAD (`encode` holds one input frame: API latency only, the stream's delay is still [`PRIMING`]) drives
//!   LONG → LONG_START → EIGHT_SHORT → LONG_STOP with sine shapes; the eight short windows are coded as eight groups
//!   (no grouping rule: AACENC-GROUPING owed). M/S (AACENCSHORT): a stereo frame is always a CPE with `common_window`
//!   1; a band goes M/S when its perceptual-entropy estimate is lower that way (`ms_decide`), ms_mask_present 0/1/2.
//!   Not built: TNS, PNS, intensity, SBR, a bit reservoir, one budget across the CPE (AACENC-CPEBUDGET).
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
/// window_sequence values (§4.6.11, the decoder's).
const ONLY_LONG: u8 = 0;
const LONG_START: u8 = 1;
const EIGHT_SHORT: u8 = 2;
const LONG_STOP: u8 = 3;
/// The attack detector (AACENCSHORT): a 128-sample sub-block's high-passed energy over the mean of the 8 before it, and
/// its absolute floor (i16² summed over the sub-block: an rms step of ~88 LSB).
const ATTACK_RATIO: f64 = 10.0;
const ATTACK_MIN: f64 = 1.0e6;

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

/// The bands a block codes (AACENCSHORT): long, one window of `SWB_1024` bands; EIGHT_SHORT, eight windows of `SWB_128`
/// bands. Band `i` is window `i / nb`, band `i % nb` — the decoder's scalefactor and spectral order with one window a group.
#[derive(Clone, Copy)]
struct Grid {
    short: bool,
    nw: usize,
    swb: &'static [u16],
    /// bands per window below the bitrate's cutoff
    nb: usize,
}

impl Grid {
    fn bands(&self) -> usize { self.nw * self.nb }
    /// Coefficient range of band `i` in the 1024-value block (short windows window-major, `w * 128 + k`).
    fn rng(&self, i: usize) -> (usize, usize) {
        let (w, b) = (i / self.nb, i % self.nb);
        let o = if self.short { w * 128 } else { 0 };
        (o + self.swb[b] as usize, o + self.swb[b + 1] as usize)
    }
}

/// One channel's coded frame.
struct Coded {
    max_sfb: usize,
    global_gain: i32,
    /// per window and band (`w * max_sfb + b`): codebook (0 = ZERO_HCB) and scalefactor
    cb: Vec<u8>,
    sf: Vec<i32>,
    q: Vec<i32>,
    bits: usize,
}

impl Coded {
    /// Re-strided to `m >= max_sfb` bands a window (a CPE's common max_sfb), the new bands ZERO_HCB.
    fn pad(&mut self, m: usize, nw: usize) {
        if m == self.max_sfb { return; }
        let mut cb = vec![0u8; nw * m];
        let mut sf = vec![0i32; nw * m];
        for w in 0..nw {
            for b in 0..self.max_sfb {
                cb[w * m + b] = self.cb[w * self.max_sfb + b];
                sf[w * m + b] = self.sf[w * self.max_sfb + b];
            }
        }
        self.cb = cb;
        self.sf = sf;
        self.max_sfb = m;
    }
}

/// The AAC-LC encoder: 1 or 2 channels at one of the 13 AAC rates, a target bitrate.
pub struct Encoder {
    rate: u32,
    channels: usize,
    sf_index: usize,
    bitrate: u32,
    mdct: Imdct,
    /// the 256-point transform of the short windows (AACENCSHORT; built as the decoder's `imdct_short`)
    mdct_s: Imdct,
    /// the sine window's rising half (1024), and the short one's (128)
    win: Vec<f32>,
    win_s: Vec<f32>,
    pow43: Vec<f32>,
    /// per channel: the previous block (the first half of the next MDCT input)
    hist: Vec<Vec<f32>>,
    /// interleaved input short of a frame
    pend: Vec<i16>,
    /// the frame coded next, held for the one-frame lookahead the block decision needs
    held: Option<Vec<i16>>,
    units: u64,
    /// bands at or above this one are never coded (the bitrate's bandwidth); the short windows' own count
    max_band: usize,
    max_band_s: usize,
    budget: usize,
    flushed: bool,
    /// the block coded next is EIGHT_SHORT (its attack seen); the last block's window_sequence
    short_next: bool,
    prev_seq: u8,
    /// per channel: the last 8 sub-block energies and the last sample (the detector's high-pass)
    det: Vec<([f64; 8], f32)>,
    /// block switching and M/S on (the in-module KATs turn them off for the long-only / L-R comparisons)
    blocks: bool,
    ms: bool,
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
        // the same cutoff in the short windows' 128 bins
        let bin_s = (cutoff as u64 * 2 * 128 / rate as u64) as u16;
        let swb_s = t::SWB_128[sf_index];
        let num_swb_s = t::NUM_SWB_128[sf_index] as usize;
        let max_band_s = (0..num_swb_s).take_while(|&b| swb_s[b] < bin_s).count();
        Ok(Encoder {
            rate,
            channels: ch,
            sf_index,
            bitrate,
            mdct: Imdct::new(2048, [2048, 2048]),
            mdct_s: Imdct::new(256, [256, 256]),
            win: sine(2048),
            win_s: sine(256),
            pow43: pow43_table(),
            hist: vec![vec![0.0; FRAME]; ch],
            pend: Vec::new(),
            held: None,
            units: 0,
            max_band,
            max_band_s,
            budget,
            flushed: false,
            short_next: false,
            prev_seq: ONLY_LONG,
            det: vec![([0f64; 8], 0f32); ch],
            blocks: true,
            ms: true,
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

    /// Interleaved PCM in; every whole frame out as an access unit, one frame behind the input (the lookahead the
    /// block decision needs; [`Encoder::flush`] returns the held frame).
    pub fn encode(&mut self, pcm: &[i16]) -> Vec<AccessUnit> {
        self.pend.extend_from_slice(pcm);
        let n = FRAME * self.channels;
        let mut out = Vec::new();
        while self.pend.len() >= n {
            let next: Vec<i16> = self.pend.drain(..n).collect();
            self.push(next, &mut out);
        }
        out
    }

    /// The end: the held frame, the partial frame padded with silence, then one silent frame that drains the overlap.
    /// Encoding `L` frames in all yields `ceil(L / 1024) + 1` units. A second call yields nothing.
    pub fn flush(&mut self) -> Vec<AccessUnit> {
        if self.flushed { return Vec::new(); }
        self.flushed = true;
        let n = FRAME * self.channels;
        let mut out = Vec::new();
        if !self.pend.is_empty() {
            let mut block = core::mem::take(&mut self.pend);
            block.resize(n, 0);
            self.push(block, &mut out);
        }
        self.push(vec![0i16; n], &mut out);
        if let Some(cur) = self.held.take() { out.push(self.frame(&cur, &vec![0i16; n])); }
        out
    }

    /// One input frame in: the held frame is coded with this one as its lookahead, this one is held. The first frame
    /// primes the detector over the silence before it (block 0 is never short).
    fn push(&mut self, next: Vec<i16>, out: &mut Vec<AccessUnit>) {
        match self.held.take() {
            None => {
                let ch = self.channels;
                let mut look = vec![0i16; FRAME * ch];
                look[FRAME / 2 * ch..].copy_from_slice(&next[..FRAME / 2 * ch]);
                let _ = self.attack(&look);
            }
            Some(cur) => out.push(self.frame(&cur, &next)),
        }
        self.held = Some(next);
    }

    /// The attack detector over one block's window (1024 interleaved frames, the block's middle: its last 512 input
    /// samples' predecessor half and the next frame's first 512): 8 sub-blocks of 128 first-difference samples per channel,
    /// an attack where a sub-block's energy is over `ATTACK_MIN` and `ATTACK_RATIO` times the mean of the 8 before it.
    fn attack(&mut self, win: &[i16]) -> bool {
        let ch = self.channels;
        let mut hit = false;
        for c in 0..ch {
            let (hist, last) = &mut self.det[c];
            for s in 0..8 {
                let mut e = 0f64;
                for n in 0..128 {
                    let x = win[(s * 128 + n) * ch + c] as f32;
                    let d = (x - *last) as f64;
                    e += d * d;
                    *last = x;
                }
                let mean = hist.iter().sum::<f64>() / 8.0;
                if e > ATTACK_MIN && e > ATTACK_RATIO * mean { hit = true; }
                hist.rotate_left(1);
                hist[7] = e;
            }
        }
        hit
    }

    /// One ICS grid per window_sequence.
    fn grid(&self, seq: u8) -> Grid {
        if seq == EIGHT_SHORT {
            Grid { short: true, nw: 8, swb: t::SWB_128[self.sf_index], nb: self.max_band_s }
        } else {
            Grid { short: false, nw: 1, swb: t::SWB_1024[self.sf_index], nb: self.max_band }
        }
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

    /// The forward MDCT of a windowed N-sample block (N/2 coefficients; N = 2048, or 256 for a short window): the fold,
    /// the decoder's inverse of that size, the middle half read back (v[j] = -y[3N/4 - 1 - j]), scaled 2·Σ.
    fn mdct(&self, z: &[f32]) -> Vec<f32> {
        let tr = if z.len() == 256 { &self.mdct_s } else { &self.mdct };
        let (n2, n4) = (z.len() / 2, z.len() / 4);
        let mut u = vec![0f32; n2];
        for m in 0..n4 { u[m] = -z[3 * n4 - 1 - m] - z[3 * n4 + m]; }
        for m in n4..n2 { u[m] = z[m - n4] - z[3 * n4 - 1 - m]; }
        let mut y = vec![0f32; z.len()];
        tr.inverse(&u, &mut y);
        (0..n2).map(|j| -2.0 * y[3 * n4 - 1 - j]).collect()
    }

    /// The analysis window of `seq` over a raw 2048-sample block (the decoder's `synth` mirrored, sine shapes) and its
    /// spectrum: 1024 long coefficients, or eight short windows of 128 (window-major).
    fn analyse(&self, seq: u8, raw: &[f32]) -> Vec<f32> {
        let (wl, ws) = (&self.win, &self.win_s);
        if seq == EIGHT_SHORT {
            let mut x = vec![0f32; 1024];
            let mut z = vec![0f32; 256];
            for w in 0..8 {
                let o = 448 + w * 128;
                for n in 0..128 {
                    z[n] = raw[o + n] * ws[n];
                    z[128 + n] = raw[o + 128 + n] * ws[127 - n];
                }
                x[w * 128..w * 128 + 128].copy_from_slice(&self.mdct(&z));
            }
            return x;
        }
        let mut z = vec![0f32; 2048];
        if seq == LONG_STOP {
            for n in 0..128 { z[448 + n] = raw[448 + n] * ws[n]; }
            for n in 576..1024 { z[n] = raw[n]; }
        } else {
            for n in 0..1024 { z[n] = raw[n] * wl[n]; }
        }
        if seq == LONG_START {
            for n in 1024..1472 { z[n] = raw[n]; }
            for n in 0..128 { z[1472 + n] = raw[1472 + n] * ws[127 - n]; }
        } else {
            for n in 0..1024 { z[1024 + n] = raw[1024 + n] * wl[1023 - n]; }
        }
        self.mdct(&z)
    }

    fn frame(&mut self, cur: &[i16], next: &[i16]) -> AccessUnit {
        let ch = self.channels;
        let n = FRAME * ch;
        // the NEXT block's attack window: this frame's last 512 samples and the next frame's first 512
        let mut look = Vec::with_capacity(n);
        look.extend_from_slice(&cur[n / 2..]);
        look.extend_from_slice(&next[..n / 2]);
        let attack = self.attack(&look) && self.blocks;
        // the sequence state machine (one decision for both channels: a common_window CPE needs it)
        let seq = if self.short_next || (self.prev_seq == EIGHT_SHORT && attack) {
            EIGHT_SHORT
        } else if attack {
            LONG_START
        } else if self.prev_seq == EIGHT_SHORT {
            LONG_STOP
        } else {
            ONLY_LONG
        };
        self.short_next = attack;
        self.prev_seq = seq;
        let g = self.grid(seq);
        let mut x: Vec<Vec<f32>> = Vec::with_capacity(ch);
        for c in 0..ch {
            let mut raw = vec![0f32; 2048];
            for i in 0..FRAME {
                raw[i] = self.hist[c][i];
                let s = cur[i * ch + c] as f32;
                raw[FRAME + i] = s;
                self.hist[c][i] = s;
            }
            x.push(self.analyse(seq, &raw));
        }
        let mut allowed: Vec<Vec<f64>> = x.iter().map(|s| self.allowed(s, &g)).collect();
        let ms = if ch == 2 && self.ms { ms_decide(&mut x, &mut allowed, &g) } else { vec![false; g.bands()] };
        let mut coded: Vec<Coded> = (0..ch).map(|c| self.code(&x[c], &g, &allowed[c])).collect();
        let mut w = Bits::new();
        if ch == 1 {
            w.put(3, super::decoder::ID_SCE);
            w.put(4, 0);
            write_ics(&mut w, &coded[0], &g, Some(seq));
        } else {
            let m = coded[0].max_sfb.max(coded[1].max_sfb);
            for c in coded.iter_mut() { c.pad(m, g.nw); }
            w.put(3, super::decoder::ID_CPE);
            w.put(4, 0);
            w.put(1, 1); // common_window: one ics_info, the M/S mask
            write_info(&mut w, seq, m);
            let mut used: Vec<bool> = Vec::with_capacity(g.nw * m);
            for win in 0..g.nw { for b in 0..m { used.push(ms[win * g.nb + b]); } }
            let present = if !used.iter().any(|&u| u) { 0 } else if used.iter().all(|&u| u) { 2 } else { 1 };
            w.put(2, present);
            if present == 1 { for &u in &used { w.put(1, u as u32); } }
            write_ics(&mut w, &coded[0], &g, None);
            write_ics(&mut w, &coded[1], &g, None);
        }
        w.put(3, super::decoder::ID_END);
        let au = AccessUnit { data: w.finish(), pts: self.units * FRAME as u64 };
        self.units += 1;
        au
    }

    /// Per band of the grid, the noise energy it may carry: its energy at the fixed SNR offset, floored at the absolute
    /// level and at the frame's mask floor (`MASK_DB` under its loudest band per coefficient).
    fn allowed(&self, x: &[f32], g: &Grid) -> Vec<f64> {
        let nb = g.bands();
        let energy: Vec<f64> = (0..nb).map(|i| { let (lo, hi) = g.rng(i); x[lo..hi].iter().map(|&v| v as f64 * v as f64).sum() }).collect();
        let floor = (0..nb).map(|i| { let (lo, hi) = g.rng(i); energy[i] / (hi - lo) as f64 }).fold(0f64, f64::max) * math::exp2(-MASK_DB / 10.0 * core::f64::consts::LOG2_10);
        (0..nb).map(|i| {
            let (lo, hi) = g.rng(i);
            (energy[i] * math::exp2(-SNR_DB / 10.0 * core::f64::consts::LOG2_10)).max((ATH.max(floor)) * (hi - lo) as f64)
        }).collect()
    }

    /// The two loops on one channel's spectrum over the grid, each band held under its `allowed` noise.
    fn code(&self, x: &[f32], g: &Grid, allowed: &[f64]) -> Coded {
        let nb = g.bands();
        let a: Vec<f32> = x.iter().map(|&v| math::pow(v.abs() as f64, 0.75) as f32).collect();
        // the OUTER loop: per band, the coarsest scalefactor within its threshold (None: the band is not coded)
        let mut base: Vec<Option<(i32, i32)>> = Vec::with_capacity(nb); // (sf, the finest sf without overflow)
        for i in 0..nb {
            let (lo, hi) = g.rng(i);
            let e: f64 = x[lo..hi].iter().map(|&v| v as f64 * v as f64).sum();
            let allowed = allowed[i];
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
            if self.plan(x, &a, &base, m, g, g.nb).bits <= self.budget { h = m; } else { l = m + 1; }
        }
        let mut c = self.plan(x, &a, &base, l, g, g.nb);
        // still over at the coarsest offset: drop the top bands (of every window) until it fits
        let mut top = g.nb;
        while c.bits > self.budget && top > 0 {
            top -= 1;
            c = self.plan(x, &a, &base, 60, g, top);
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

    /// Quantise every band at its base sf + `gofs` (never finer than its overflow limit), hold the scalefactor chain
    /// within ±60, pick the codebooks, count the bits. Bands `top..` of every window are not coded.
    fn plan(&self, x: &[f32], a: &[f32], base: &[Option<(i32, i32)>], gofs: i32, g: &Grid, top: usize) -> Coded {
        let nb = g.bands();
        let mut q = vec![0i32; 1024];
        let mut sf: Vec<i32> = vec![0; nb];
        let mut on = vec![false; nb];
        let quant = |q: &mut [i32], i: usize, s: i32| -> bool {
            let qs = math::exp2(-0.1875 * (s - 100) as f64) as f32;
            let (lo, hi) = g.rng(i);
            let mut any = false;
            for k in lo..hi {
                let m = ((a[k] * qs + 0.4054) as i32).min(MAX_Q);
                q[k] = if x[k] < 0.0 { -m } else { m };
                any |= m != 0;
            }
            any
        };
        for i in 0..nb {
            if i % g.nb >= top { continue; }
            if let Some((s, fine)) = base[i] {
                let s = (s + gofs).clamp(fine, 255);
                sf[i] = s;
                on[i] = quant(&mut q, i, s);
            }
        }
        // the chain: consecutive coded scalefactors differ by at most 60 — the finer band is coarsened (never finer, so
        // never an overflow); a band coarsened to nothing drops out and the chain is checked again
        loop {
            let idx: Vec<usize> = (0..nb).filter(|&i| on[i]).collect();
            let mut changed = false;
            for p in idx.windows(2) {
                let (i, j) = (p[0], p[1]);
                if sf[j] - sf[i] > 60 { sf[i] = sf[j] - 60; on[i] = quant(&mut q, i, sf[i]); changed = true; break; }
                if sf[i] - sf[j] > 60 { sf[j] = sf[i] - 60; on[j] = quant(&mut q, j, sf[j]); changed = true; break; }
            }
            if !changed { break; }
        }
        for i in 0..nb {
            if !on[i] { let (lo, hi) = g.rng(i); for k in lo..hi { q[k] = 0; } }
        }
        let max_sfb = (0..nb).filter(|&i| on[i]).map(|i| i % g.nb + 1).max().unwrap_or(0);
        let mut cb = vec![0u8; g.nw * max_sfb];
        let mut sfc = vec![0i32; g.nw * max_sfb];
        // global_gain, ics_info (short: 4-bit max_sfb + 7 grouping bits), pulse/tns/gain_control flags
        let mut bits = if g.short { 8 + 15 + 3 } else { 8 + 11 + 3 };
        for w in 0..g.nw {
            for b in 0..max_sfb {
                let i = w * g.nb + b;
                if on[i] {
                    let (lo, hi) = g.rng(i);
                    let (book, n) = best_book(&q[lo..hi]);
                    cb[w * max_sfb + b] = book;
                    sfc[w * max_sfb + b] = sf[i];
                    bits += n;
                }
            }
        }
        // section data: runs of one codebook per window (3-bit lengths, escape 7, short; 5-bit, escape 31, long)
        let (sb, esc) = if g.short { (3, 7) } else { (5, 31) };
        for w in 0..g.nw {
            let row = &cb[w * max_sfb..(w + 1) * max_sfb];
            let mut b = 0;
            while b < max_sfb {
                let mut e = b;
                while e < max_sfb && row[e] == row[b] { e += 1; }
                bits += 4 + sb * ((e - b) / esc + 1);
                b = e;
            }
        }
        let global_gain = cb.iter().position(|&c| c != 0).map(|j| sfc[j]).unwrap_or(100);
        let mut prev = global_gain;
        for j in 0..cb.len() {
            if cb[j] != 0 { bits += t::SF_BITS[(sfc[j] - prev + 60) as usize] as usize; prev = sfc[j]; }
        }
        Coded { max_sfb, global_gain, cb, sf: sfc, q, bits }
    }
}

/// M/S per band (AACENCSHORT): M = (L+R)/2, S = (L-R)/2 where the perceptual-entropy estimate is lower — the product
/// of max(E/T, 1) over the two channels, M/S with both thresholds min(T_L, T_R)/2 (noise_M + noise_S lands in each of L
/// and R). An M/S band's coefficients and thresholds are replaced in place; the mask (one flag per grid band) returned.
fn ms_decide(x: &mut [Vec<f32>], a: &mut [Vec<f64>], g: &Grid) -> Vec<bool> {
    let mut ms = vec![false; g.bands()];
    let (xl, xr) = x.split_at_mut(1);
    let (xl, xr) = (&mut xl[0], &mut xr[0]);
    let (al, ar) = a.split_at_mut(1);
    let (al, ar) = (&mut al[0], &mut ar[0]);
    let pe = |e: f64, t: f64| (e / t).max(1.0);
    for i in 0..g.bands() {
        let (lo, hi) = g.rng(i);
        let (mut el, mut er, mut em, mut es) = (0f64, 0f64, 0f64, 0f64);
        for k in lo..hi {
            let (l, r) = (xl[k] as f64, xr[k] as f64);
            let (m, s) = ((l + r) * 0.5, (l - r) * 0.5);
            el += l * l;
            er += r * r;
            em += m * m;
            es += s * s;
        }
        let tm = al[i].min(ar[i]) * 0.5;
        if tm <= 0.0 { continue; }
        if pe(em, tm) * pe(es, tm) < pe(el, al[i]) * pe(er, ar[i]) {
            ms[i] = true;
            for k in lo..hi {
                let (l, r) = (xl[k], xr[k]);
                xl[k] = (l + r) * 0.5;
                xr[k] = (l - r) * 0.5;
            }
            al[i] = tm;
            ar[i] = tm;
        }
    }
    ms
}

/// ics_info (§4.4.2.1): EIGHT_SHORT carries a 4-bit max_sfb and the 7 grouping bits (0: eight groups), the long
/// sequences a 6-bit max_sfb and predictor_data_present 0; the shape is sine.
fn write_info(w: &mut Bits, seq: u8, max_sfb: usize) {
    w.put(1, 0); // ics_reserved_bit
    w.put(2, seq as u32);
    w.put(1, 0); // window_shape: sine
    if seq == EIGHT_SHORT {
        w.put(4, max_sfb as u32);
        w.put(7, 0); // scale_factor_grouping: every window its own group (AACENC-GROUPING owed)
    } else {
        w.put(6, max_sfb as u32);
        w.put(1, 0); // predictor_data_present
    }
}

/// individual_channel_stream (§4.4.2.7): the ics_info when `info` (an SCE; a common_window CPE wrote it once), no pulse
/// / TNS / gain control; section data, scalefactors and spectral data per window (one window a group).
fn write_ics(w: &mut Bits, c: &Coded, g: &Grid, info: Option<u8>) {
    w.put(8, c.global_gain as u32);
    if let Some(seq) = info { write_info(w, seq, c.max_sfb); }
    let m = c.max_sfb;
    let (sb, esc) = if g.short { (3u32, 7usize) } else { (5u32, 31usize) };
    for win in 0..g.nw {
        let row = &c.cb[win * m..(win + 1) * m];
        let mut b = 0;
        while b < m {
            let mut e = b;
            while e < m && row[e] == row[b] { e += 1; }
            w.put(4, row[b] as u32);
            let mut len = e - b;
            while len >= esc { w.put(sb, esc as u32); len -= esc; }
            w.put(sb, len as u32);
            b = e;
        }
    }
    let mut prev = c.global_gain;
    for j in 0..c.cb.len() {
        if c.cb[j] != 0 {
            let i = (c.sf[j] - prev + 60) as usize;
            w.put(t::SF_BITS[i] as u32, t::SF_CODE[i]);
            prev = c.sf[j];
        }
    }
    w.put(1, 0); // pulse_data_present
    w.put(1, 0); // tns_data_present
    w.put(1, 0); // gain_control_data_present
    for win in 0..g.nw {
        let o = if g.short { win * 128 } else { 0 };
        for b in 0..m {
            let cb = c.cb[win * m + b];
            if cb != 0 { write_band(w, cb, &c.q[o + g.swb[b] as usize..o + g.swb[b + 1] as usize]); }
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

    // ---- AACENCSHORT (LEDGER SR375): block switching and M/S

    /// The unit's window_sequence off the wire: after id/tag, an SCE's global_gain (8) or a CPE's common_window (1),
    /// then ics_reserved_bit (1) and the 2-bit sequence.
    fn wire_seq(u: &AccessUnit) -> u32 {
        let mut r = BitReader::new(&u.data);
        let id = r.read(3).unwrap();
        r.read(4).unwrap();
        if id == crate::aac::decoder::ID_CPE { r.read(1).unwrap(); } else { r.read(8).unwrap(); }
        r.read(1).unwrap();
        r.read(2).unwrap()
    }

    /// A CPE unit's M/S mask off the wire: (ms_mask_present, bands flagged, bands in the mask = groups × max_sfb).
    fn wire_ms(u: &AccessUnit) -> (u32, usize, usize) {
        let mut r = BitReader::new(&u.data);
        assert_eq!(r.read(3).unwrap(), crate::aac::decoder::ID_CPE);
        r.read(4).unwrap();
        assert_eq!(r.read(1).unwrap(), 1, "common_window");
        r.read(1).unwrap();
        let seq = r.read(2).unwrap();
        r.read(1).unwrap();
        let (groups, max_sfb) = if seq == 2 { let m = r.read(4).unwrap() as usize; assert_eq!(r.read(7).unwrap(), 0); (8, m) } else { let m = r.read(6).unwrap() as usize; r.read(1).unwrap(); (1, m) };
        let present = r.read(2).unwrap();
        let n = groups * max_sfb;
        let on = match present { 1 => (0..n).filter(|_| r.read(1).unwrap() == 1).count(), 2 => n, _ => 0 };
        (present, on, n)
    }

    /// Castanet-like bursts on silence: 64 samples of a decaying 3 kHz tone (amplitude 16000) at each position; the
    /// second channel at half level.
    fn bursts(frames: usize, at: &[usize], ch: usize) -> Vec<i16> {
        let mut v = vec![0i16; frames * ch];
        for &p in at {
            for n in 0..64 {
                let s = 16000.0 * math::exp(-(n as f64) / 12.0) * math::sin(2.0 * core::f64::consts::PI * 3000.0 * n as f64 / 48000.0);
                for c in 0..ch { v[(p + n) * ch + c] = (s * if c == 0 { 1.0 } else { 0.5 }) as i16; }
            }
        }
        v
    }

    // floors under the measured numbers (LEDGER SR375): the bursts' SNR 15.5 / 16.1 dB at 128 kb/s stereo; the pre-echo
    // 10.1 dB lower with short windows (4.37e8 -> 4.27e7, mono 64 kb/s; SNR 2.6 -> 15.5 dB); M/S on 1025 of 1025 bands
    const SHORT_SNR_FLOOR: f64 = 12.0;
    const PRE_ECHO_DB: f64 = 6.0;
    const MS_BANDS_FLOOR: f64 = 0.9;

    const BURSTS: [usize; 3] = [10000, 20000, 30000];

    /// The 256-point fold (the short windows) against the definition, as `aac_enc_mdct_matches_direct` does for 2048.
    #[test]
    fn aac_enc_short_mdct_matches_direct() {
        let e = Encoder::new(48000, 1, 64000).unwrap();
        let z: Vec<f32> = (0..256).map(|n| (math::sin(n as f64 * 0.37) * 1000.0 + math::cos(n as f64 * 0.011) * 300.0) as f32).collect();
        let x = e.mdct(&z);
        assert_eq!(x.len(), 128);
        let mut worst = 0f64;
        let mut peak = 0f64;
        for k in 0..128 {
            let mut s = 0f64;
            for n in 0..256 {
                s += z[n] as f64 * math::cos(2.0 * core::f64::consts::PI / 256.0 * (n as f64 + 0.5 + 64.0) * (k as f64 + 0.5));
            }
            worst = worst.max((2.0 * s - x[k] as f64).abs());
            peak = peak.max((2.0 * s).abs());
        }
        assert!(worst <= peak * 1e-4, "worst {worst} peak {peak}");
    }

    /// The window sequence around three bursts, read off the wire and PINNED: block u's attack window is input
    /// [(u-1)·1024 + 512, u·1024 + 512), so a burst at p makes unit (p - 512) / 1024 + 1 EIGHT_SHORT, the one before
    /// LONG_START, the one after LONG_STOP; every other unit ONLY_LONG. The stream decodes through the decoder's short path.
    #[test]
    fn aac_enc_short_click_sequence() {
        let frames = 40960;
        let pcm = bursts(frames, &BURSTS, 2);
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        assert_eq!(units.len(), 41);
        for (i, u) in units.iter().enumerate() { assert_eq!(u.pts, i as u64 * 1024); }
        let seq: Vec<u32> = units.iter().map(wire_seq).collect();
        let mut want = vec![0u32; 41];
        for u in [9usize, 19, 28] { want[u] = 1; want[u + 1] = 2; want[u + 2] = 3; }
        assert_eq!(seq, want);
        let d = decode(&e, &units);
        for c in 0..2 {
            let (mut s, mut n) = (0f64, 0f64);
            for i in 0..frames {
                let x = pcm[i * 2 + c] as f64;
                let y = d[c][i + PRIMING as usize];
                s += x * x;
                n += (x - y) * (x - y);
            }
            let snr = 10.0 * math::log10(s / n.max(1e-9));
            std::println!("aac_enc_short_click_sequence: channel {c} SNR {snr:.1} dB");
            assert!(snr >= SHORT_SNR_FLOOR, "channel {c}: SNR {snr:.1} dB");
        }
    }

    /// Mono bursts at 64 kb/s, block switching on vs off: (the decoded energy in [p - 768, p - 64) summed over the
    /// bursts — the input is silence there, so all of it is pre-echo — and the whole signal's SNR).
    fn pre_echo(blocks: bool) -> (f64, f64) {
        let frames = 40960;
        let pcm = bursts(frames, &BURSTS, 1);
        let mut e = Encoder::new(48000, 1, 64000).unwrap();
        e.blocks = blocks;
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let d = decode(&e, &units);
        let mut pre = 0f64;
        for &p in &BURSTS { for i in p - 768..p - 64 { pre += d[0][i + PRIMING as usize] * d[0][i + PRIMING as usize]; } }
        let (mut s, mut n) = (0f64, 0f64);
        for i in 0..frames {
            let x = pcm[i] as f64;
            let y = d[0][i + PRIMING as usize];
            s += x * x;
            n += (x - y) * (x - y);
        }
        (pre, 10.0 * math::log10(s / n.max(1e-9)))
    }

    /// Short windows hold the bursts' quantisation noise near them: the pre-echo falls by at least PRE_ECHO_DB.
    #[test]
    fn aac_enc_short_pre_echo() {
        let (ps, ss) = pre_echo(true);
        let (pl, sl) = pre_echo(false);
        let db = 10.0 * math::log10(pl / ps.max(1e-9));
        std::println!("aac_enc_short_pre_echo: pre-echo long {pl:.3e} short {ps:.3e} ({db:.1} dB lower), SNR long {sl:.1} short {ss:.1} dB");
        assert!(db >= PRE_ECHO_DB, "pre-echo only {db:.1} dB lower");
    }

    /// L = a broadband noise, R = L + an independent ±8: (units, total bytes, per-channel SNR) with M/S on or off.
    fn correlated(ms: bool) -> (Vec<AccessUnit>, usize, [f64; 2]) {
        let frames = 24000;
        let (mut a, mut b) = (0x1234_5678u32, 0x9abc_def0u32);
        let mut pcm = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            a = a.wrapping_mul(1664525).wrapping_add(1013904223);
            b = b.wrapping_mul(22695477).wrapping_add(1);
            let l = ((a >> 8) % 12001) as i32 - 6000;
            let r = l + ((b >> 8) % 17) as i32 - 8;
            pcm.push(l as i16);
            pcm.push(r as i16);
        }
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        e.ms = ms;
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let bytes = units.iter().map(|u| u.data.len()).sum();
        let d = decode(&e, &units);
        let mut snr = [0f64; 2];
        for c in 0..2 {
            let (mut s, mut n) = (0f64, 0f64);
            for i in 2048..frames - 2048 {
                let x = pcm[i * 2 + c] as f64;
                let y = d[c][i + PRIMING as usize];
                s += x * x;
                n += (x - y) * (x - y);
            }
            snr[c] = 10.0 * math::log10(s / n.max(1e-9));
        }
        (units, bytes, snr)
    }

    /// A correlated pair (L ≈ R) takes M/S on most bands (read off the wire), costs fewer bytes than the independent
    /// L/R coding (S falls under its threshold and codes nothing) and decodes through the decoder's M/S path no worse.
    #[test]
    fn aac_enc_ms_correlated() {
        let (u_ms, b_ms, s_ms) = correlated(true);
        let (u_lr, b_lr, s_lr) = correlated(false);
        let (mut on, mut all) = (0usize, 0usize);
        for u in &u_ms { let (_, o, n) = wire_ms(u); on += o; all += n; }
        for u in &u_lr { assert_eq!(wire_ms(u).0, 0); }
        let frac = on as f64 / all.max(1) as f64;
        std::println!("aac_enc_ms_correlated: M/S on {on}/{all} bands ({:.1}%), bytes M/S {b_ms} L/R {b_lr}, SNR M/S {:.1}/{:.1} L/R {:.1}/{:.1} dB", frac * 100.0, s_ms[0], s_ms[1], s_lr[0], s_lr[1]);
        assert!(frac >= MS_BANDS_FLOOR, "M/S on {on}/{all}");
        assert!(b_ms < b_lr, "M/S {b_ms} bytes, L/R {b_lr}");
        for c in 0..2 { assert!(s_ms[c] >= s_lr[c] - 1.0, "channel {c}: M/S {:.1} dB, L/R {:.1} dB", s_ms[c], s_lr[c]); }
    }
}
