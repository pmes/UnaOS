// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The SBC encoder (A2DP, rmbp-ledger B739). ONE file with no `crate::` path: `sbc_core` declares it as `pub mod sbc`
//! and the kernel's A2DP sink (`drivers/bt/avdtp.rs`) includes the same file by `#[path]`.
//!
//! Per frame (A2DP specification, the SBC chapter; the arithmetic of steps 1-3 and 5 is Google libsbc's, A2DPKAT
//! rmbp-ledger B870 — github.com/google/libsbc @ 6e505650145c9973d08a0bdd5e5f5e1914305e40, `src/sbc.c`, Apache-2.0 —
//! so the frames are byte-exact against that reference, [`kat`]):
//! 1. ANALYSIS, per block and channel (libsbc `analyze_4` / `analyze_8`): the window (fixed 2.13, [`W4`] / [`W8`],
//!    the specification's prototype x 4 x 2^13, duplicated and transposed for a circular history of 10 blocks split by
//!    parity) gives the folded half-sums, rounded to i16; the DCT ([`COS8`] / [`COSMAT`], Q13) gives the subband
//!    samples, rounded to i16. Unity gain against the specification's synthesis.
//! 2. SCALE FACTORS: per channel and subband the highest set bit of the OR of every block's magnitude (a negative
//!    sample counts as its ones' complement), 0 for silence.
//! 3. JOINT STEREO: subband sb (all but the last) is sent as M = (L+R)/2, S = (L-R)/2 when the scale factors of the
//!    UNHALVED L+R and L-R sum below sf(L) + sf(R); those are the scale factors sent; the join bit says so.
//! 4. BIT ALLOCATION ([`bit_alloc`]): the specification's procedure word for word — bitneed (SNR = the scale factor;
//!    LOUDNESS = scale factor - offset, halved when positive, -5 for a zero scale factor), the bitslice descent, the
//!    two distribution passes; joint over both channels in STEREO/JOINT_STEREO, per channel in MONO/DUAL_CHANNEL
//!    (libsbc's `compute_nbits` is the same procedure).
//! 5. QUANTIZATION: q = (((s x (2^bits - 1)) >> (sf + 1)) + 2^bits - 1) >> 1 on the i16 subband sample.
//! 6. PACKING: syncword 0x9C, the header byte, bitpool, CRC, join bits, scale factors (4 bits), samples (block,
//!    channel, subband); the CRC-8 (x^8+x^4+x^3+x^2+1, init 0x0F) covers the header byte, the bitpool and every bit
//!    from the join field to the last scale factor.
//!
//! [`kat`] is the known-answer test both rings run (host `cargo test -p sbc_core`, kernel `tests a2dp`).
#![allow(clippy::needless_range_loop)]

/// The SBC syncword.
pub const SYNC: u8 = 0x9C;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Freq {
    F16000,
    F32000,
    F44100,
    F48000,
}

impl Freq {
    pub const fn code(self) -> u8 {
        match self {
            Freq::F16000 => 0,
            Freq::F32000 => 1,
            Freq::F44100 => 2,
            Freq::F48000 => 3,
        }
    }
    pub const fn hz(self) -> u32 {
        match self {
            Freq::F16000 => 16_000,
            Freq::F32000 => 32_000,
            Freq::F44100 => 44_100,
            Freq::F48000 => 48_000,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Mono,
    Dual,
    Stereo,
    Joint,
}

impl Mode {
    pub const fn code(self) -> u8 {
        match self {
            Mode::Mono => 0,
            Mode::Dual => 1,
            Mode::Stereo => 2,
            Mode::Joint => 3,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alloc {
    Loudness,
    Snr,
}

/// One SBC stream's parameters (what AVDTP's SET_CONFIGURATION carries, plus the bitpool in use).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    pub freq: Freq,
    /// 4, 8, 12 or 16.
    pub blocks: u8,
    pub mode: Mode,
    pub alloc: Alloc,
    /// 4 or 8.
    pub subbands: u8,
    pub bitpool: u8,
}

impl Config {
    /// The A2DP specification's recommended high-quality joint-stereo set at 44.1 kHz (bitpool 53, 119-byte frames).
    pub const A2DP_HQ: Config = Config { freq: Freq::F44100, blocks: 16, mode: Mode::Joint, alloc: Alloc::Loudness, subbands: 8, bitpool: 53 };

    pub const fn channels(&self) -> usize {
        match self.mode {
            Mode::Mono => 1,
            _ => 2,
        }
    }

    /// PCM frames (samples per channel) one SBC frame carries.
    pub const fn samples(&self) -> usize {
        self.blocks as usize * self.subbands as usize
    }

    /// Blocks and subbands are legal values and the bitpool is in 2..=16 x subbands (MONO, DUAL) or 32 x subbands.
    pub const fn valid(&self) -> bool {
        let b = matches!(self.blocks, 4 | 8 | 12 | 16);
        let s = matches!(self.subbands, 4 | 8);
        let max = match self.mode {
            Mode::Mono | Mode::Dual => 16 * self.subbands as u32,
            _ => 32 * self.subbands as u32,
        };
        b && s && self.bitpool >= 2 && (self.bitpool as u32) <= max
    }

    /// The specification's frame_length: 4 + (4 x subbands x channels) / 8 + ceil(the sample bits / 8).
    pub const fn frame_len(&self) -> usize {
        let (sb, nb, bp, ch) = (self.subbands as usize, self.blocks as usize, self.bitpool as usize, self.channels());
        let data = match self.mode {
            Mode::Mono | Mode::Dual => nb * ch * bp,
            Mode::Stereo => nb * bp,
            Mode::Joint => sb + nb * bp,
        };
        4 + (4 * sb * ch) / 8 + data.div_ceil(8)
    }

    /// Bits per second: 8 x frame_length x fs / (subbands x blocks).
    pub const fn bit_rate(&self) -> u32 {
        (8 * self.frame_len() as u64 * self.freq.hz() as u64 / self.samples() as u64) as u32
    }

    /// The header byte after the syncword: sampling_frequency(2) blocks(2) channel_mode(2) allocation_method(1)
    /// subbands(1).
    pub const fn header_byte(&self) -> u8 {
        let blk = match self.blocks {
            4 => 0,
            8 => 1,
            12 => 2,
            _ => 3,
        };
        let alloc = match self.alloc {
            Alloc::Loudness => 0,
            Alloc::Snr => 1,
        };
        (self.freq.code() << 6) | (blk << 4) | (self.mode.code() << 2) | (alloc << 1) | (self.subbands == 8) as u8
    }

    /// The three bytes before the CRC: syncword, header byte, bitpool.
    pub const fn header(&self) -> [u8; 3] {
        [SYNC, self.header_byte(), self.bitpool]
    }
}

/// The SBC CRC-8 over the first `bits` bits of `data`, MSB first: G(X) = X^8 + X^4 + X^3 + X^2 + 1, register
/// initialised to 0x0F, no final inversion.
pub fn crc8(data: &[u8], bits: usize) -> u8 {
    let mut c: u8 = 0x0F;
    for n in 0..bits.min(data.len() * 8) {
        let b = (data[n / 8] >> (7 - n % 8)) & 1;
        let top = c >> 7;
        c <<= 1;
        if top ^ b != 0 {
            c ^= 0x1D;
        }
    }
    c
}

/// Loudness offsets by sampling frequency (16, 32, 44.1, 48 kHz) and subband.
const OFFSET4: [[i8; 4]; 4] = [[-1, 0, 0, 0], [-2, 0, 0, 1], [-2, 0, 0, 1], [-2, 0, 0, 1]];
const OFFSET8: [[i8; 8]; 4] = [[-2, 0, 0, 0, 0, 0, 0, 1], [-3, 0, 0, 0, 0, 0, 1, 2], [-4, 0, 0, 0, 0, 0, 1, 2], [-4, 0, 0, 0, 0, 0, 1, 2]];

fn bitneed(cfg: &Config, scf: u8, sb: usize) -> i32 {
    match cfg.alloc {
        Alloc::Snr => scf as i32,
        Alloc::Loudness => {
            if scf == 0 {
                return -5;
            }
            let f = cfg.freq.code() as usize;
            let off = if cfg.subbands == 4 { OFFSET4[f][sb] } else { OFFSET8[f][sb] } as i32;
            let l = scf as i32 - off;
            if l > 0 { l / 2 } else { l }
        }
    }
}

/// The specification's bit allocation: `bits[ch][sb]` from the scale factors. Joint over both channels in STEREO and
/// JOINT_STEREO; each channel alone (with the whole bitpool) in MONO and DUAL_CHANNEL.
pub fn bit_alloc(cfg: &Config, scf: &[[u8; 8]; 2], bits: &mut [[u8; 8]; 2]) {
    *bits = [[0; 8]; 2];
    match cfg.mode {
        Mode::Mono => alloc_set(cfg, scf, bits, 0, 1),
        Mode::Dual => {
            alloc_set(cfg, scf, bits, 0, 1);
            alloc_set(cfg, scf, bits, 1, 1);
        }
        Mode::Stereo | Mode::Joint => alloc_set(cfg, scf, bits, 0, 2),
    }
}

fn alloc_set(cfg: &Config, scf: &[[u8; 8]; 2], bits: &mut [[u8; 8]; 2], c0: usize, nch: usize) {
    let nsb = (cfg.subbands as usize).min(8);
    let bp = cfg.bitpool as i32;
    let mut need = [[0i32; 8]; 2];
    let mut maxn = i32::MIN;
    for ch in c0..c0 + nch {
        for sb in 0..nsb {
            need[ch][sb] = bitneed(cfg, scf[ch][sb], sb);
            maxn = maxn.max(need[ch][sb]);
        }
    }
    let (mut bitcount, mut slicecount) = (0i32, 0i32);
    let mut bitslice = maxn + 1;
    // Each (ch, sb) contributes at most 16 bits over the descent, so a valid bitpool (<= 16 x subbands x channels)
    // ends it within 17 steps; the bound only guards an invalid configuration.
    for _ in 0..40 {
        bitslice -= 1;
        bitcount += slicecount;
        slicecount = 0;
        for ch in c0..c0 + nch {
            for sb in 0..nsb {
                let n = need[ch][sb];
                if n > bitslice + 1 && n < bitslice + 16 {
                    slicecount += 1;
                } else if n == bitslice + 1 {
                    slicecount += 2;
                }
            }
        }
        if bitcount + slicecount >= bp {
            break;
        }
    }
    if bitcount + slicecount == bp {
        bitcount += slicecount;
        bitslice -= 1;
    }
    for ch in c0..c0 + nch {
        for sb in 0..nsb {
            let n = need[ch][sb];
            bits[ch][sb] = if n < bitslice + 2 { 0 } else { (n - bitslice).min(16) as u8 };
        }
    }
    // The two distribution passes, in the specification's order (subband-major, channel 0 then 1).
    let (mut ch, mut sb) = (c0, 0usize);
    while bitcount < bp && sb < nsb {
        let b = bits[ch][sb];
        if (2..16).contains(&b) {
            bits[ch][sb] += 1;
            bitcount += 1;
        } else if need[ch][sb] == bitslice + 1 && bp > bitcount + 1 {
            bits[ch][sb] = 2;
            bitcount += 2;
        }
        if nch == 2 && ch == c0 {
            ch = c0 + 1;
        } else {
            ch = c0;
            sb += 1;
        }
    }
    let (mut ch, mut sb) = (c0, 0usize);
    while bitcount < bp && sb < nsb {
        if bits[ch][sb] < 16 {
            bits[ch][sb] += 1;
            bitcount += 1;
        }
        if nch == 2 && ch == c0 {
            ch = c0 + 1;
        } else {
            ch = c0;
            sb += 1;
        }
    }
}

/// i16 saturation (libsbc's `SBC_SAT16`).
fn sat16(v: i32) -> i32 {
    v.clamp(i16::MIN as i32, i16::MAX as i32)
}

/// One channel's analysis state (libsbc's `sbc_estate`): the block counter (0..=9), the PCM history split by block
/// parity in the window's row order, and the half-sums carried into the next block.
#[derive(Clone)]
struct Est {
    idx: usize,
    x: [[[i16; 5]; 8]; 2],
    y: [i32; 4],
}

impl Est {
    const ZERO: Est = Est { idx: 0, x: [[[0; 5]; 8]; 2], y: [0; 4] };
}

/// x[k] x w[k + idx] summed over k < 5 (libsbc's window rows read from `window[..][0] + idx`).
fn dot5(x: &[i16; 5], w: &[i16; 10], idx: usize) -> i32 {
    let mut a = 0i32;
    for k in 0..5 {
        a += x[k] as i32 * w[k + idx] as i32;
    }
    a
}

/// libsbc `analyze_4`: four new samples (oldest first) -> four subband samples.
fn analyze_4(st: &mut Est, inp: &[i16; 8], out: &mut [i32; 8]) {
    let (idx, odd) = (st.idx >> 1, st.idx & 1);
    let ii = if idx != 0 { 5 - idx } else { 0 };
    let x = &mut st.x[odd];
    x[0][ii] = inp[3];
    x[1][ii] = inp[1];
    x[2][ii] = inp[2];
    x[3][ii] = inp[0];
    let (w0, w1) = (&W4[0], &W4[1]);
    let y0 = dot5(&x[0], &w0[0], idx) + st.y[0];
    st.y[0] = dot5(&x[0], &w1[0], idx);
    let y1 = dot5(&x[2], &w0[2], idx) + dot5(&x[3], &w0[3], idx);
    let y2 = st.y[1];
    st.y[1] = dot5(&x[2], &w1[2], idx) - dot5(&x[3], &w1[3], idx);
    let y3 = dot5(&x[1], &w0[1], idx);
    let y = [y0, y1, y2, y3].map(|v| sat16((v + (1 << 14)) >> 15));
    st.idx = if st.idx < 9 { st.idx + 1 } else { 0 };
    let c = COS8;
    let s = [
        y[0] * c[2] + y[1] * c[1] + y[2] * c[3] + (y[3] << 13),
        -y[0] * c[2] + y[1] * c[3] - y[2] * c[1] + (y[3] << 13),
        -y[0] * c[2] - y[1] * c[3] + y[2] * c[1] + (y[3] << 13),
        y[0] * c[2] - y[1] * c[1] - y[2] * c[3] + (y[3] << 13),
    ];
    for k in 0..4 {
        out[k] = sat16((s[k] + (1 << 12)) >> 13);
    }
}

/// libsbc `analyze_8`: eight new samples (oldest first) -> eight subband samples.
fn analyze_8(st: &mut Est, inp: &[i16; 8], out: &mut [i32; 8]) {
    let (idx, odd) = (st.idx >> 1, st.idx & 1);
    let ii = if idx != 0 { 5 - idx } else { 0 };
    let x = &mut st.x[odd];
    x[0][ii] = inp[7];
    x[1][ii] = inp[3];
    x[2][ii] = inp[6];
    x[3][ii] = inp[0];
    x[4][ii] = inp[5];
    x[5][ii] = inp[1];
    x[6][ii] = inp[4];
    x[7][ii] = inp[2];
    let (w0, w1) = (&W8[0], &W8[1]);
    let y0 = dot5(&x[0], &w0[0], idx) + st.y[0];
    st.y[0] = dot5(&x[0], &w1[0], idx);
    let y1 = dot5(&x[2], &w0[2], idx) + dot5(&x[3], &w0[3], idx);
    let y4 = st.y[1];
    st.y[1] = dot5(&x[2], &w1[2], idx) - dot5(&x[3], &w1[3], idx);
    let y2 = dot5(&x[4], &w0[4], idx) + dot5(&x[5], &w0[5], idx);
    let y5 = st.y[2];
    st.y[2] = dot5(&x[4], &w1[4], idx) - dot5(&x[5], &w1[5], idx);
    let y3 = dot5(&x[6], &w0[6], idx) + dot5(&x[7], &w0[7], idx);
    let y6 = st.y[3];
    st.y[3] = dot5(&x[6], &w1[6], idx) - dot5(&x[7], &w1[7], idx);
    let y7 = dot5(&x[1], &w0[1], idx);
    let y = [y0, y1, y2, y3, y4, y5, y6, y7].map(|v| sat16((v + (1 << 14)) >> 15));
    st.idx = if st.idx < 9 { st.idx + 1 } else { 0 };
    for i in 0..8 {
        let mut s = 0i32;
        for j in 0..8 {
            s += y[j] * COSMAT[i][j];
        }
        out[i] = sat16((s + (1 << 12)) >> 13);
    }
}

/// libsbc's magnitude for the scale factor: a negative sample counts as its ones' complement.
fn mag(s: i32) -> u32 {
    (if s < 0 { !s } else { s }) as u32
}

/// The highest set bit of `m` (0 for 0): libsbc's scale factor of an OR of magnitudes.
fn msb(m: u32) -> u8 {
    if m == 0 { 0 } else { (31 - m.leading_zeros()) as u8 }
}

/// libsbc's quantizer: (((s x (2^bits - 1)) >> (sf + 1)) + 2^bits - 1) >> 1.
fn quantize(s: i32, scf: u8, bits: u8) -> u32 {
    let range = (1i64 << bits) - 1;
    ((((s as i64 * range) >> (scf as u32 + 1)) + range) >> 1) as u32
}

struct BitW<'a> {
    b: &'a mut [u8],
    pos: usize,
}

impl BitW<'_> {
    fn put(&mut self, v: u32, n: u8) {
        for i in (0..n as u32).rev() {
            if (v >> i) & 1 != 0 {
                if let Some(x) = self.b.get_mut(self.pos / 8) {
                    *x |= 0x80 >> (self.pos % 8);
                }
            }
            self.pos += 1;
        }
    }
}

/// An SBC encoder: the configuration and each channel's analysis state.
#[derive(Clone)]
pub struct Encoder {
    cfg: Config,
    est: [Est; 2],
}

impl Encoder {
    pub const fn new(cfg: Config) -> Encoder {
        Encoder { cfg, est: [Est::ZERO, Est::ZERO] }
    }

    pub const fn config(&self) -> &Config {
        &self.cfg
    }

    /// Forget the history (a new stream).
    pub fn reset(&mut self) {
        self.est = [Est::ZERO, Est::ZERO];
    }

    /// Encode ONE frame. `pcm` = `config().samples()` PCM frames, 16-bit, interleaved by channel (one channel in
    /// MONO). Returns the bytes written (`frame_len()`), or 0 when the configuration is invalid, `pcm` is short or
    /// `out` is smaller than a frame.
    pub fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> usize {
        let cfg = self.cfg;
        let (nch, nsb, nbl) = (cfg.channels(), cfg.subbands as usize, cfg.blocks as usize);
        let len = cfg.frame_len();
        if !cfg.valid() || pcm.len() < cfg.samples() * nch || out.len() < len {
            return 0;
        }
        // 1. Analysis: [block][channel][subband], each an i16 value.
        let mut sbs = [[[0i32; 8]; 2]; 16];
        for blk in 0..nbl {
            for ch in 0..nch {
                let mut inp = [0i16; 8];
                for i in 0..nsb {
                    inp[i] = pcm[(blk * nsb + i) * nch + ch];
                }
                if nsb == 8 {
                    analyze_8(&mut self.est[ch], &inp, &mut sbs[blk][ch]);
                } else {
                    analyze_4(&mut self.est[ch], &inp, &mut sbs[blk][ch]);
                }
            }
        }
        // 2. Scale factors, and 3. joint stereo's decision (every subband but the last).
        let mut scf = [[0u8; 8]; 2];
        let mut join = [false; 8];
        for sb in 0..nsb {
            let (mut m0, mut m1, mut j0, mut j1) = (0u32, 0u32, 0u32, 0u32);
            for b in 0..nbl {
                let (s0, s1) = (sbs[b][0][sb], sbs[b][1][sb]);
                m0 |= mag(s0);
                m1 |= mag(s1);
                j0 |= mag(s0 + s1);
                j1 |= mag(s0 - s1);
            }
            scf[0][sb] = msb(m0);
            scf[1][sb] = msb(m1);
            if cfg.mode == Mode::Joint && sb + 1 < nsb {
                let (js0, js1) = (msb(j0), msb(j1));
                if (js0 as u32 + js1 as u32) < (scf[0][sb] as u32 + scf[1][sb] as u32) {
                    join[sb] = true;
                    scf[0][sb] = js0;
                    scf[1][sb] = js1;
                }
            }
        }
        // 4. Bit allocation.
        let mut bits = [[0u8; 8]; 2];
        bit_alloc(&cfg, &scf, &mut bits);
        // 3. Joint stereo's coupling: M = (L+R)/2, S = (L-R)/2.
        for sb in 0..nsb {
            if join[sb] {
                for b in 0..nbl {
                    let (s0, s1) = (sbs[b][0][sb], sbs[b][1][sb]);
                    sbs[b][0][sb] = (s0 + s1) >> 1;
                    sbs[b][1][sb] = (s0 - s1) >> 1;
                }
            }
        }
        // 6. Packing (the CRC byte is written last).
        for b in out[..len].iter_mut() {
            *b = 0;
        }
        let mut w = BitW { b: &mut out[..len], pos: 0 };
        let h = cfg.header();
        w.put(h[0] as u32, 8);
        w.put(h[1] as u32, 8);
        w.put(h[2] as u32, 8);
        w.put(0, 8);
        if cfg.mode == Mode::Joint {
            for sb in 0..nsb {
                w.put(join[sb] as u32, 1);
            }
        }
        for ch in 0..nch {
            for sb in 0..nsb {
                w.put(scf[ch][sb] as u32, 4);
            }
        }
        let crc_end = w.pos;
        // 5. Quantization, block / channel / subband order.
        for blk in 0..nbl {
            for ch in 0..nch {
                for sb in 0..nsb {
                    let b = bits[ch][sb];
                    if b > 0 {
                        w.put(quantize(sbs[blk][ch][sb], scf[ch][sb], b), b);
                    }
                }
            }
        }
        out[3] = frame_crc(&out[..len], crc_end);
        len
    }
}

/// The CRC of a packed frame: the header byte, the bitpool, then bits 32..`crc_end` (join + scale factors).
pub fn frame_crc(frame: &[u8], crc_end: usize) -> u8 {
    let mut buf = [0u8; 2 + 9];
    if frame.len() < 4 || crc_end < 32 {
        return 0;
    }
    buf[0] = frame[1];
    buf[1] = frame[2];
    let tail = (crc_end - 32).min(72);
    let nbytes = tail.div_ceil(8);
    for i in 0..nbytes {
        buf[2 + i] = frame.get(4 + i).copied().unwrap_or(0);
    }
    crc8(&buf, 16 + tail)
}

/// The bit position where a frame's CRC coverage ends (after the last scale factor).
pub fn crc_end(cfg: &Config) -> usize {
    32 + if cfg.mode == Mode::Joint { cfg.subbands as usize } else { 0 } + 4 * cfg.subbands as usize * cfg.channels()
}

/// THE KNOWN-ANSWER TEST (host `cargo test -p sbc_core`, kernel `tests a2dp`). Every expected number is a constant of
/// the specification or a CRC computed outside this crate:
/// * frame length and bit rate of the A2DP specification's recommended sets at 44.1 kHz (8 subbands, 16 blocks,
///   loudness): mono bitpool 19 -> 46 bytes / 127 kb/s, mono 31 -> 70 / 193, joint stereo 35 -> 83 / 229, joint
///   stereo 53 -> 119 / 328;
/// * the header of the high-quality set: 9C BD 35;
/// * CRC-8 (x^8+x^4+x^3+x^2+1, init 0x0F) of "123456789" (72 bits) = 0x1E, of BD 35 (16 bits) = 0xB3, of the first
///   12 bits of A1 B2 = 0xBF (computed when this file was written by an independent bitwise reference — the
///   specification's shift-register loop in Python — not by this function);
/// * a digital-silence frame of the high-quality set: no subband joined, every scale factor 0, so its first twelve
///   bytes are 9C BD 35 7D 00 x 8 (0x7D = the CRC of BD 35 and 72 zero bits, by the same reference);
/// * a tone frame's CRC byte equals the CRC recomputed over its own coverage, and the frame is `frame_len()` long;
/// * A2DPKAT (rmbp-ledger B870): TWO WHOLE FRAMES per vector, byte for byte ([`KAT_JS44`], [`KAT_MONO16`]), computed
///   from Google libsbc's encoder (github.com/google/libsbc @ 6e505650145c9973d08a0bdd5e5f5e1914305e40, `src/sbc.c`
///   `sbc_encode`: `analyze_4`/`analyze_8`, `compute_scale_factors(_js)`, `compute_nbits`, `encode_frame`,
///   `compute_crc`) by a hand transcription whose every table was compared with that source (A2DPKAT's
///   `libsbc_ref.py`; regenerating them with libsbc's own `tools/esbc` is owed, KATBIN). The input is integers only
///   ([`kat_lcg`], [`kat_tri`]): s = s x 1664525 + 1013904223 (u32), a = (s >> 16) - 32768; tri(i) = the 16-step
///   triangle +-8192 (`t * 2048 - 8192` rising for t = i % 16 < 8, `(16 - t) * 2048 - 8192` falling).
///   - JOINT STEREO 44.1 kHz, 16 blocks, 8 subbands, loudness, bitpool 53 (`Config::A2DP_HQ`); seed 1, per frame
///     index i (0..256) draw a then b: L = tri(i) + (a >> 3), R = ((L x 7) >> 3) + (b >> 4); frames 0 and 1 (frame 0
///     joins subbands 0, 1; frame 1 joins 0, 1, 5). For `esbc -j -b 53` write these as a 44.1 kHz stereo WAV.
///   - MONO 16 kHz, 8 blocks, 4 subbands, SNR, bitpool 18; seed 2, per sample i (0..64): tri(i) + (a >> 3); frames 0
///     and 1 (`esbc -s 4 -B 8 -S -b 18` on a 16 kHz mono WAV).
///   A mismatch fails as `kat-js44-frame<n>` / `kat-mono16-frame<n>` (`tests a2dp`: `sbc_kat=FAIL`).
pub fn kat() -> Result<(), &'static str> {
    let set = |mode, bitpool| Config { mode, bitpool, ..Config::A2DP_HQ };
    for (cfg, len, kbps) in [(set(Mode::Mono, 19), 46, 127), (set(Mode::Mono, 31), 70, 193), (set(Mode::Joint, 35), 83, 229), (set(Mode::Joint, 53), 119, 328)] {
        if cfg.frame_len() != len {
            return Err("frame-length");
        }
        if (cfg.bit_rate() + 500) / 1000 != kbps {
            return Err("bit-rate");
        }
    }
    if Config::A2DP_HQ.header() != [0x9C, 0xBD, 0x35] {
        return Err("header");
    }
    if crc8(b"123456789", 72) != 0x1E || crc8(&[0xBD, 0x35], 16) != 0xB3 || crc8(&[0xA1, 0xB2], 12) != 0xBF {
        return Err("crc8");
    }
    let mut enc = Encoder::new(Config::A2DP_HQ);
    let pcm = [0i16; 256];
    let mut f = [0u8; 128];
    if enc.encode(&pcm, &mut f) != 119 {
        return Err("silence-length");
    }
    if f[..4] != [0x9C, 0xBD, 0x35, 0x7D] || f[4..12].iter().any(|&b| b != 0) {
        return Err("silence-frame");
    }
    // A tone: a 16-step triangle at -12 dBFS on the left, its inverse at half level on the right.
    let mut pcm = [0i16; 256];
    for i in 0..128 {
        let t = (i % 16) as i32;
        let tri = if t < 8 { t * 2048 - 8192 } else { (16 - t) * 2048 - 8192 };
        pcm[2 * i] = tri as i16;
        pcm[2 * i + 1] = (-tri / 2) as i16;
    }
    let mut enc = Encoder::new(Config::A2DP_HQ);
    for _ in 0..3 {
        if enc.encode(&pcm, &mut f) != 119 {
            return Err("tone-length");
        }
    }
    if f[..3] != [0x9C, 0xBD, 0x35] || f[3] != frame_crc(&f[..119], crc_end(&Config::A2DP_HQ)) {
        return Err("tone-crc");
    }
    if f[4..12].iter().all(|&b| b == 0) {
        return Err("tone-scale-factors");
    }
    // A2DPKAT (B870): the two cited vectors, two whole frames each.
    let mut pcm = [0i16; 512];
    let mut seed = 1u32;
    for i in 0..256 {
        let (a, b) = (kat_lcg(&mut seed), kat_lcg(&mut seed));
        let l = kat_tri(i) + (a >> 3);
        pcm[2 * i] = l as i16;
        pcm[2 * i + 1] = (((l * 7) >> 3) + (b >> 4)) as i16;
    }
    let mut enc = Encoder::new(Config::A2DP_HQ);
    for (n, want) in KAT_JS44.iter().enumerate() {
        if enc.encode(&pcm[n * 256..(n + 1) * 256], &mut f) != 119 || f[..119] != want[..] {
            return Err(["kat-js44-frame0", "kat-js44-frame1"][n]);
        }
    }
    let mono = Config { freq: Freq::F16000, blocks: 8, mode: Mode::Mono, alloc: Alloc::Snr, subbands: 4, bitpool: 18 };
    let mut pcm = [0i16; 64];
    let mut seed = 2u32;
    for i in 0..64 {
        pcm[i] = (kat_tri(i) + (kat_lcg(&mut seed) >> 3)) as i16;
    }
    let mut enc = Encoder::new(mono);
    for (n, want) in KAT_MONO16.iter().enumerate() {
        if enc.encode(&pcm[n * 32..(n + 1) * 32], &mut f) != 24 || f[..24] != want[..] {
            return Err(["kat-mono16-frame0", "kat-mono16-frame1"][n]);
        }
    }
    Ok(())
}

/// A2DPKAT: the KAT input's generator (Numerical Recipes' LCG): the next value in -32768..32768.
fn kat_lcg(s: &mut u32) -> i32 {
    *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*s >> 16) as i32 - 32768
}

/// A2DPKAT: the KAT input's 16-step triangle, -8192..=8192.
fn kat_tri(i: usize) -> i32 {
    let t = (i % 16) as i32;
    if t < 8 { t * 2048 - 8192 } else { (16 - t) * 2048 - 8192 }
}

// ── The specification's prototype in fixed point, generated once from the printed values: C = round(coefficient x
// 2^24) (the printed table carries its sign pattern: C[i] = h[i] x (-1)^floor(i / 2M) for the symmetric lowpass h). The
// encoder's analysis no longer uses it (A2DPKAT B870: libsbc's windows below); the host tests' decoder synthesizes with
// it (window -M x C), so it is compiled for tests only. ─────────────────────────────────────────────────────────

/// The 8-subband prototype (80 taps), Q24.
#[cfg(test)]
pub(crate) const C8: [i32; 80] = [
    0, 2627, 5759, 9305, 13823, 19125, 24770, 29926,
    33753, 35295, 33463, 27121, 15136, -3000, -27678, -58673,
    94951, 134711, 175464, 213863, 245829, 266834, 272141, 257000,
    217050, 148605, 49058, -82473, -245625, -438051, -655572, -892335,
    1140833, 1392253, 1637043, 1865571, 2068036, 2235806, 2361452, 2439237,
    2465497, 2439237, 2361452, 2235806, 2068036, 1865571, 1637043, 1392253,
    -1140833, -892335, -655572, -438051, -245625, -82473, 49058, 148605,
    217050, 257000, 272141, 266834, 245829, 213863, 175464, 134711,
    -94951, -58673, -27678, -3000, 15136, 27121, 33463, 35295,
    33753, 29926, 24770, 19125, 13823, 9305, 5759, 2627,
];
/// The 4-subband prototype (40 taps), Q24.
#[cfg(test)]
pub(crate) const C4: [i32; 40] = [
    0, 9002, 25030, 45864, 64378, 65298, 31303, -51340,
    183103, 342901, 484455, 540125, 434140, 102885, -483548, -1302690,
    2274878, 3271353, 4137877, 4728293, 4937792, 4728293, 4137877, 3271353,
    -2274878, -1302690, -483548, 102885, 434140, 540125, 484455, 342901,
    -183103, -51340, 31303, 65298, 64378, 45864, 25030, 9002,
];
// ── A2DPKAT (rmbp-ledger B870): Google libsbc's analysis tables (github.com/google/libsbc @ 6e505650, src/sbc.c,
// Copyright 2022 Google LLC, Apache-2.0), copied value for value (A2DPKAT's tablecheck.py compared them with that
// source), and the KAT's two cited vectors. ─────────────────────────────────────────────────────────────────────

/// libsbc's 4-subband window (fixed 2.13), duplicated and transposed for its circular history (rows 0, 2, 1, 3).
const W4: [[[i16; 10]; 4]; 2] = [
    [
        [0, 358, 4443, -4443, -358, 0, 358, 4443, -4443, -358], // 0
        [49, 946, 8082, -944, 61, 49, 946, 8082, -944, 61], // 2
        [18, 670, 6389, -2544, -100, 18, 670, 6389, -2544, -100], // 1
        [90, 1055, 9235, 201, 128, 90, 1055, 9235, 201, 128], // 3
    ],
    [
        [126, 848, 9644, 848, 126, 126, 848, 9644, 848, 126], // 0
        [61, -944, 8082, 946, 49, 61, -944, 8082, 946, 49], // 2
        [128, 201, 9235, 1055, 90, 128, 201, 9235, 1055, 90], // 1
        [-100, -2544, 6389, 670, 18, -100, -2544, 6389, 670, 18], // 3
    ],
];
/// libsbc's 8-subband window (fixed 2.13), rows 0, 4, 1, 7, 2, 6, 3, 5.
const W8: [[[i16; 10]; 8]; 2] = [
    [
        [0, 185, 2228, -2228, -185, 0, 185, 2228, -2228, -185], // 0
        [27, 480, 4039, -480, 30, 27, 480, 4039, -480, 30], // 4
        [5, 263, 2719, -1743, -115, 5, 263, 2719, -1743, -115], // 1
        [58, 502, 4764, 290, 69, 58, 502, 4764, 290, 69], // 7
        [11, 343, 3197, -1280, -54, 11, 343, 3197, -1280, -54], // 2
        [48, 532, 4612, 96, 65, 48, 532, 4612, 96, 65], // 6
        [18, 418, 3644, -856, -6, 18, 418, 3644, -856, -6], // 3
        [37, 521, 4367, -161, 53, 37, 521, 4367, -161, 53], // 5
    ],
    [
        [66, 424, 4815, 424, 66, 66, 424, 4815, 424, 66], // 0
        [30, -480, 4039, 480, 27, 30, -480, 4039, 480, 27], // 4
        [69, 290, 4764, 502, 58, 69, 290, 4764, 502, 58], // 1
        [-115, -1743, 2719, 263, 5, -115, -1743, 2719, 263, 5], // 7
        [65, 96, 4612, 532, 48, 65, 96, 4612, 532, 48], // 2
        [-54, -1280, 3197, 343, 11, -54, -1280, 3197, 343, 11], // 6
        [53, -161, 4367, 521, 37, 53, -161, 4367, 521, 37], // 3
        [-6, -856, 3644, 418, 18, -6, -856, 3644, 418, 18], // 5
    ],
];
/// libsbc's 4-subband DCT constants (Q13): cos(k pi / 8), k = 0..3.
const COS8: [i32; 4] = [8192, 7568, 5793, 3135];
/// libsbc's 8-subband DCT matrix (Q13) over its folded half-sums.
const COSMAT: [[i32; 8]; 8] = [
    [5793, 6811, 7568, 8035, 4551, 3135, 1598, 8192],
    [-5793, -1598, 3135, 6811, -8035, -7568, -4551, 8192],
    [-5793, -8035, -3135, 4551, 1598, 7568, 6811, 8192],
    [5793, -4551, -7568, 1598, 6811, -3135, -8035, 8192],
    [5793, 4551, -7568, -1598, -6811, -3135, 8035, 8192],
    [-5793, 8035, -3135, -4551, -1598, 7568, -6811, 8192],
    [-5793, 1598, 3135, -6811, 8035, -7568, 4551, 8192],
    [5793, -6811, 7568, -8035, -4551, 3135, -1598, 8192],
];
/// A2DPKAT (B870): libsbc @ 6e505650's joint-stereo 44.1 kHz bitpool 53 frames 0 and 1 of [`kat`]'s stereo input.
const KAT_JS44: [[u8; 119]; 2] = [
    [
        0x9C, 0xBD, 0x35, 0x6E, 0xC0, 0xDC, 0xAA, 0xAA, 0xAB, 0xA9, 0xAA, 0xAB, 0xAA, 0x7E, 0xF6, 0xDB,
        0x57, 0xDB, 0x6D, 0xAB, 0xF7, 0xB6, 0xDA, 0xBE, 0xDB, 0x6D, 0x5F, 0x3D, 0xB6, 0xD5, 0xE6, 0xDB,
        0x6A, 0xFA, 0x09, 0x46, 0xAF, 0xB4, 0xA3, 0x56, 0xCB, 0xB9, 0xBA, 0x69, 0x4C, 0xDB, 0x4A, 0x59,
        0x4A, 0x4C, 0x50, 0xA4, 0xE5, 0x9D, 0x25, 0xA8, 0x58, 0x76, 0xD4, 0x94, 0x1B, 0x1D, 0x71, 0x6B,
        0x6F, 0x38, 0x63, 0x39, 0x4B, 0x16, 0x9B, 0xA9, 0x85, 0x46, 0x36, 0xE0, 0xCC, 0xDA, 0x90, 0x96,
        0xC6, 0xA6, 0xC5, 0xD9, 0x4A, 0x81, 0x41, 0xC9, 0xB6, 0x31, 0xB2, 0xDB, 0x56, 0x13, 0x6E, 0xE5,
        0x71, 0x47, 0x29, 0x4D, 0x49, 0x71, 0xA4, 0x89, 0x4A, 0xD1, 0x84, 0xE4, 0x35, 0x61, 0x91, 0x98,
        0xB4, 0x57, 0x1B, 0x2A, 0xFA, 0x8A, 0x25,
    ],
    [
        0x9C, 0xBD, 0x35, 0xF2, 0xC4, 0xDC, 0xBA, 0xAB, 0xAA, 0xA9, 0xBA, 0xA9, 0xAA, 0x59, 0x55, 0x8A,
        0x2C, 0x11, 0x56, 0xAD, 0x07, 0x2B, 0x4A, 0x62, 0x49, 0xA6, 0x55, 0xD2, 0x9C, 0x74, 0xBC, 0x92,
        0xB5, 0x4D, 0xAC, 0x9B, 0x58, 0x5B, 0x49, 0x56, 0x51, 0xCA, 0x92, 0xC1, 0x2C, 0x4A, 0xD2, 0x5C,
        0x29, 0x02, 0x46, 0xDC, 0x55, 0xA4, 0x62, 0xD6, 0xAE, 0xB9, 0x6A, 0x94, 0x1E, 0xED, 0x34, 0x89,
        0x38, 0xA4, 0x67, 0x48, 0x91, 0x2B, 0xD2, 0x58, 0x8C, 0xF6, 0x2A, 0x4E, 0x21, 0x49, 0x35, 0x17,
        0xD6, 0x2C, 0x6A, 0xBB, 0x91, 0xAD, 0x39, 0xAC, 0x93, 0x98, 0xDA, 0x49, 0x56, 0x13, 0x68, 0xD4,
        0xB2, 0xDA, 0x62, 0xCC, 0x53, 0xA4, 0xC5, 0xF8, 0x92, 0x45, 0x84, 0xEB, 0x17, 0x49, 0xB9, 0x92,
        0xB4, 0x54, 0xF6, 0x42, 0x79, 0xB8, 0x92,
    ],
];
/// A2DPKAT (B870): libsbc @ 6e505650's mono 16 kHz 4-subband SNR bitpool 18 frames 0 and 1 of [`kat`]'s mono input.
const KAT_MONO16: [[u8; 24]; 2] = [
    [
        0x9C, 0x12, 0x12, 0xB2, 0xCB, 0xAB, 0x7D, 0xED, 0xDF, 0x7B, 0x77, 0x5C, 0xDD, 0xE9, 0x27, 0x30,
        0x6E, 0xE4, 0x5B, 0x6D, 0xEE, 0x99, 0x7C, 0x56,
    ],
    [
        0x9C, 0x12, 0x12, 0x57, 0xDA, 0xAA, 0x46, 0x6D, 0x28, 0x17, 0x09, 0xAE, 0xCD, 0x6D, 0x44, 0x54,
        0xB4, 0xE7, 0x0E, 0xCC, 0x2A, 0xF1, 0xA8, 0x93,
    ],
];
