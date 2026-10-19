// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The SBC encoder (A2DP, rmbp-ledger B739). ONE file with no `crate::` path: `sbc_core` declares it as `pub mod sbc`
//! and the kernel's A2DP sink (`drivers/bt/avdtp.rs`) includes the same file by `#[path]`.
//!
//! Per frame (A2DP specification, the SBC chapter):
//! 1. ANALYSIS, per block and channel: the history X (10 x subbands samples) shifts by `subbands`, the new samples
//!    enter oldest-at-X[M-1]; Z = C x X (C = the specification's prototype, 80 taps for 8 subbands, 40 for 4);
//!    Y[i] = sum_j Z[i + 2Mj] (j = 0..4); S[k] = sum_i cos((k + 1/2)(i - M/2)pi/M) Y[i]. Fixed point: C in Q24, the
//!    cosine matrix in Q15, i64 accumulation, S kept in Q12 (|S| <= 2.5 x 2^15 x 2^12 < 2^31 for any 16-bit input:
//!    the prototype's absolute sum is 2.52).
//! 2. SCALE FACTORS: per channel and subband the smallest k (0..15) with max|S| < 2^(k+1).
//! 3. JOINT STEREO: subband sb (all but the last) is sent as M = (L+R)/2, S = (L-R)/2 when sf(M) + sf(S) <
//!    sf(L) + sf(R); the join bit says so.
//! 4. BIT ALLOCATION ([`bit_alloc`]): the specification's procedure word for word — bitneed (SNR = the scale factor;
//!    LOUDNESS = scale factor - offset, halved when positive, -5 for a zero scale factor), the bitslice descent, the
//!    two distribution passes; joint over both channels in STEREO/JOINT_STEREO, per channel in MONO/DUAL_CHANNEL.
//! 5. QUANTIZATION: q = floor((S / 2^(sf+1) + 1) x (2^bits - 1) / 2).
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

/// Fractional bits of the subband samples the analysis produces.
const SB_FRAC: u32 = 12;

/// The analysis filter for one block of one channel: `inp` = the `nsb` new samples, oldest first.
fn analyze(x: &mut [i32; 80], inp: &[i32], nsb: usize, s: &mut [i32; 8]) {
    let l = 10 * nsb;
    for i in (nsb..l).rev() {
        x[i] = x[i - nsb];
    }
    for i in 0..nsb {
        x[nsb - 1 - i] = inp[i];
    }
    let (c, m): (&[i32], &[i32]) = if nsb == 8 { (&C8, &M8) } else { (&C4, &M4) };
    let mut y = [0i64; 16];
    for i in 0..2 * nsb {
        let mut acc = 0i64;
        for j in 0..5 {
            let k = i + j * 2 * nsb;
            acc += c[k] as i64 * x[k] as i64;
        }
        y[i] = acc;
    }
    for k in 0..nsb {
        let mut acc = 0i64;
        for i in 0..2 * nsb {
            acc += m[k * 2 * nsb + i] as i64 * y[i];
        }
        s[k] = (acc >> (24 + 15 - SB_FRAC)) as i32;
    }
}

/// The smallest k in 0..=15 with |v| < 2^(k+1) for every v the subband carried (`maxabs` in Q12).
fn scf_of(maxabs: i64) -> u8 {
    let mut k = 0u8;
    while k < 15 && maxabs >= 1i64 << (k as u32 + 1 + SB_FRAC) {
        k += 1;
    }
    k
}

fn quantize(v: i32, scf: u8, bits: u8) -> u32 {
    let levels = (1i64 << bits) - 1;
    let sh = scf as u32 + 1 + SB_FRAC;
    let q = ((v as i64 + (1i64 << sh)) * levels) >> (sh + 1);
    q.clamp(0, levels) as u32
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

/// An SBC encoder: the configuration and each channel's analysis history.
#[derive(Clone)]
pub struct Encoder {
    cfg: Config,
    x: [[i32; 80]; 2],
}

impl Encoder {
    pub const fn new(cfg: Config) -> Encoder {
        Encoder { cfg, x: [[0; 80]; 2] }
    }

    pub const fn config(&self) -> &Config {
        &self.cfg
    }

    /// Forget the history (a new stream).
    pub fn reset(&mut self) {
        self.x = [[0; 80]; 2];
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
        // 1. Analysis: [block][channel][subband], Q12.
        let mut sbs = [[[0i32; 8]; 2]; 16];
        for blk in 0..nbl {
            for ch in 0..nch {
                let mut inp = [0i32; 8];
                for i in 0..nsb {
                    inp[i] = pcm[(blk * nsb + i) * nch + ch] as i32;
                }
                analyze(&mut self.x[ch], &inp[..nsb], nsb, &mut sbs[blk][ch]);
            }
        }
        // 2. Scale factors.
        let maxabs = |sbs: &[[[i32; 8]; 2]; 16], ch: usize, sb: usize| (0..nbl).map(|b| (sbs[b][ch][sb] as i64).abs()).max().unwrap_or(0);
        let mut scf = [[0u8; 8]; 2];
        for ch in 0..nch {
            for sb in 0..nsb {
                scf[ch][sb] = scf_of(maxabs(&sbs, ch, sb));
            }
        }
        // 3. Joint stereo, every subband but the last.
        let mut join = [false; 8];
        if cfg.mode == Mode::Joint {
            for sb in 0..nsb - 1 {
                let (mut mm, mut ms) = (0i64, 0i64);
                for b in 0..nbl {
                    let (l, r) = (sbs[b][0][sb] as i64, sbs[b][1][sb] as i64);
                    mm = mm.max(((l + r) >> 1).abs());
                    ms = ms.max(((l - r) >> 1).abs());
                }
                let (sm, ss) = (scf_of(mm), scf_of(ms));
                if (sm as u32 + ss as u32) < (scf[0][sb] as u32 + scf[1][sb] as u32) {
                    join[sb] = true;
                    for b in 0..nbl {
                        let (l, r) = (sbs[b][0][sb], sbs[b][1][sb]);
                        sbs[b][0][sb] = ((l as i64 + r as i64) >> 1) as i32;
                        sbs[b][1][sb] = ((l as i64 - r as i64) >> 1) as i32;
                    }
                    scf[0][sb] = sm;
                    scf[1][sb] = ss;
                }
            }
        }
        // 4. Bit allocation.
        let mut bits = [[0u8; 8]; 2];
        bit_alloc(&cfg, &scf, &mut bits);
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
/// * a tone frame's CRC byte equals the CRC recomputed over its own coverage, and the frame is `frame_len()` long.
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
    Ok(())
}

// ── The specification's tables in fixed point, generated once from the printed values: C = round(coefficient x 2^24)
// (the printed table carries its sign pattern: C[i] = h[i] x (-1)^floor(i / 2M) for the symmetric lowpass h); the
// cosine matrices = round(cos((k + 1/2)(i - M/2) pi / M) x 2^15), row k. A float model of this filter bank with the
// specification's synthesis (window -M x C) reconstructs noise at 64 dB (4 subbands) / 66 dB (8 subbands). ──────

/// The 8-subband prototype (80 taps), Q24.
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
pub(crate) const C4: [i32; 40] = [
    0, 9002, 25030, 45864, 64378, 65298, 31303, -51340,
    183103, 342901, 484455, 540125, 434140, 102885, -483548, -1302690,
    2274878, 3271353, 4137877, 4728293, 4937792, 4728293, 4137877, 3271353,
    -2274878, -1302690, -483548, 102885, 434140, 540125, 484455, 342901,
    -183103, -51340, 31303, 65298, 64378, 45864, 25030, 9002,
];
/// The 8-subband analysis matrix, Q15.
pub(crate) const M8: [i32; 128] = [
    23170, 27246, 30274, 32138, 32768, 32138, 30274, 27246, 23170, 18205, 12540, 6393, 0, -6393, -12540, -18205,
    -23170, -6393, 12540, 27246, 32768, 27246, 12540, -6393, -23170, -32138, -30274, -18205, 0, 18205, 30274, 32138,
    -23170, -32138, -12540, 18205, 32768, 18205, -12540, -32138, -23170, 6393, 30274, 27246, 0, -27246, -30274, -6393,
    23170, -18205, -30274, 6393, 32768, 6393, -30274, -18205, 23170, 27246, -12540, -32138, 0, 32138, 12540, -27246,
    23170, 18205, -30274, -6393, 32768, -6393, -30274, 18205, 23170, -27246, -12540, 32138, 0, -32138, 12540, 27246,
    -23170, 32138, -12540, -18205, 32768, -18205, -12540, 32138, -23170, -6393, 30274, -27246, 0, 27246, -30274, 6393,
    -23170, 6393, 12540, -27246, 32768, -27246, 12540, 6393, -23170, 32138, -30274, 18205, 0, -18205, 30274, -32138,
    23170, -27246, 30274, -32138, 32768, -32138, 30274, -27246, 23170, -18205, 12540, -6393, 0, 6393, -12540, 18205,
];
/// The 4-subband analysis matrix, Q15.
pub(crate) const M4: [i32; 32] = [
    23170, 30274, 32768, 30274, 23170, 12540, 0, -12540,
    -23170, 12540, 32768, 12540, -23170, -30274, 0, 30274,
    -23170, -12540, 32768, -12540, -23170, 30274, 0, -30274,
    23170, -30274, 32768, -30274, 23170, -12540, 0, 12540,
];
