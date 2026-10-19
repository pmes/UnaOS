// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Host tests: the KAT both rings run, then a ROUND TRIP — frames from [`Encoder`] read back by a decoder written here
//! from the specification's decoder side (frame parse, the same normative bit allocation, dequantization
//! sf x ((2q + 1) / levels - 1), joint-stereo M+S / M-S, the synthesis filter with window -M x C) and compared with
//! the input after the filter bank's delay: the SNR says the frames carry the audio, bit for bit where a sink reads it.

use super::sbc::*;

#[test]
fn kat_passes() {
    assert_eq!(kat(), Ok(()));
}

struct BitR<'a> {
    b: &'a [u8],
    pos: usize,
}

impl BitR<'_> {
    fn get(&mut self, n: u8) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let bit = (self.b[self.pos / 8] >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        v
    }
}

/// The decoder: one frame -> `samples` PCM frames per channel (f64), synthesis state in `v`.
fn decode(frame: &[u8], v: &mut [Vec<f64>; 2], out: &mut [Vec<f64>; 2]) -> usize {
    let mut r = BitR { b: frame, pos: 0 };
    assert_eq!(r.get(8), 0x9C);
    let h = r.get(8) as u8;
    let freq = [Freq::F16000, Freq::F32000, Freq::F44100, Freq::F48000][(h >> 6) as usize];
    let blocks = [4u8, 8, 12, 16][((h >> 4) & 3) as usize];
    let mode = [Mode::Mono, Mode::Dual, Mode::Stereo, Mode::Joint][((h >> 2) & 3) as usize];
    let alloc = if (h >> 1) & 1 == 1 { Alloc::Snr } else { Alloc::Loudness };
    let subbands = if h & 1 == 1 { 8u8 } else { 4 };
    let bitpool = r.get(8) as u8;
    let cfg = Config { freq, blocks, mode, alloc, subbands, bitpool };
    let crc = r.get(8) as u8;
    let (nch, nsb, nbl) = (cfg.channels(), subbands as usize, blocks as usize);
    let mut join = [false; 8];
    if mode == Mode::Joint {
        for sb in 0..nsb {
            join[sb] = r.get(1) == 1;
        }
    }
    let mut scf = [[0u8; 8]; 2];
    for ch in 0..nch {
        for sb in 0..nsb {
            scf[ch][sb] = r.get(4) as u8;
        }
    }
    assert_eq!(crc, frame_crc(frame, r.pos), "crc");
    let mut bits = [[0u8; 8]; 2];
    bit_alloc(&cfg, &scf, &mut bits);
    let mut sb_s = vec![[[0f64; 8]; 2]; nbl];
    for blk in 0..nbl {
        for ch in 0..nch {
            for sb in 0..nsb {
                let b = bits[ch][sb];
                if b > 0 {
                    let q = r.get(b) as f64;
                    let levels = ((1u32 << b) - 1) as f64;
                    let sf = (1u32 << (scf[ch][sb] as u32 + 1)) as f64;
                    sb_s[blk][ch][sb] = sf * ((q * 2.0 + 1.0) / levels - 1.0);
                }
            }
        }
        for sb in 0..nsb {
            if join[sb] {
                let (m, s) = (sb_s[blk][0][sb], sb_s[blk][1][sb]);
                sb_s[blk][0][sb] = m + s;
                sb_s[blk][1][sb] = m - s;
            }
        }
    }
    assert!(r.pos <= cfg.frame_len() * 8);
    let c: Vec<f64> = if nsb == 8 { C8.iter().map(|&x| x as f64 / (1 << 24) as f64).collect() } else { C4.iter().map(|&x| x as f64 / (1 << 24) as f64).collect() };
    let m = nsb;
    for blk in 0..nbl {
        for ch in 0..nch {
            let vv = &mut v[ch];
            vv.rotate_right(2 * m);
            for k in 0..2 * m {
                vv[k] = (0..m).map(|i| ((i as f64 + 0.5) * (k as f64 + (m / 2) as f64) * std::f64::consts::PI / m as f64).cos() * sb_s[blk][ch][i]).sum();
            }
            let mut u = vec![0f64; 10 * m];
            for i in 0..5 {
                for j in 0..m {
                    u[i * 2 * m + j] = vv[i * 4 * m + j];
                    u[i * 2 * m + j + m] = vv[i * 4 * m + j + 3 * m];
                }
            }
            let w: Vec<f64> = (0..10 * m).map(|i| u[i] * c[i] * -(m as f64)).collect();
            for j in 0..m {
                out[ch].push((0..10).map(|i| w[j + m * i]).sum());
            }
        }
    }
    cfg.frame_len()
}

/// Best SNR (dB) of `y` against `x` over delays 0..300, the gain fitted.
fn snr(x: &[f64], y: &[f64]) -> (f64, usize) {
    let mut best = (f64::MIN, 0);
    for d in 0..300 {
        let n = x.len().min(y.len() - d) - 400;
        let (mut xy, mut yy, mut xx) = (0.0, 0.0, 0.0);
        for i in 200..n {
            xy += x[i] * y[i + d];
            yy += y[i + d] * y[i + d];
            xx += x[i] * x[i];
        }
        if yy == 0.0 {
            continue;
        }
        let g = xy / yy;
        let mut e = 0.0;
        for i in 200..n {
            let t = x[i] - g * y[i + d];
            e += t * t;
        }
        let s = 10.0 * (xx / e.max(1e-9)).log10();
        if s > best.0 {
            best = (s, d);
        }
    }
    best
}

fn round_trip(cfg: Config, tones: [(f64, f64); 2], frames: usize) -> [f64; 2] {
    let nch = cfg.channels();
    let spf = cfg.samples();
    let fs = cfg.freq.hz() as f64;
    let total = spf * frames;
    let mut x = [vec![0f64; total], vec![0f64; total]];
    let mut pcm = vec![0i16; total * nch];
    for n in 0..total {
        for ch in 0..nch {
            let (hz, amp) = tones[ch];
            let s = (amp * (2.0 * std::f64::consts::PI * hz * n as f64 / fs).sin()).round();
            x[ch][n] = s;
            pcm[n * nch + ch] = s as i16;
        }
    }
    let mut enc = Encoder::new(cfg);
    let mut v = [vec![0f64; 20 * cfg.subbands as usize], vec![0f64; 20 * cfg.subbands as usize]];
    let mut y = [Vec::new(), Vec::new()];
    let mut frame = vec![0u8; cfg.frame_len()];
    for f in 0..frames {
        let n = enc.encode(&pcm[f * spf * nch..(f + 1) * spf * nch], &mut frame);
        assert_eq!(n, cfg.frame_len());
        assert_eq!(&frame[..3], &cfg.header());
        decode(&frame, &mut v, &mut y);
    }
    let mut r = [0.0; 2];
    for ch in 0..nch {
        r[ch] = snr(&x[ch], &y[ch]).0;
    }
    r
}

#[test]
fn round_trip_joint_stereo_hq() {
    let r = round_trip(Config::A2DP_HQ, [(1000.0, 12000.0), (2500.0, 8000.0)], 120);
    assert!(r[0] > 30.0 && r[1] > 30.0, "joint stereo bitpool 53 SNR {:?}", r);
}

#[test]
fn round_trip_joint_stereo_correlated() {
    // The same tone in both channels: the low subbands join (M carries it, S is near zero).
    let r = round_trip(Config { bitpool: 35, ..Config::A2DP_HQ }, [(440.0, 10000.0), (440.0, 10000.0)], 120);
    assert!(r[0] > 25.0 && r[1] > 25.0, "joint stereo bitpool 35 SNR {:?}", r);
}

#[test]
fn round_trip_other_modes() {
    let mono4 = Config { mode: Mode::Mono, subbands: 4, blocks: 8, bitpool: 31, ..Config::A2DP_HQ };
    let r = round_trip(mono4, [(1000.0, 12000.0), (0.0, 0.0)], 300);
    assert!(r[0] > 25.0, "mono 4 subbands SNR {:?}", r);
    for mode in [Mode::Stereo, Mode::Dual] {
        let cfg = Config { mode, alloc: Alloc::Snr, bitpool: if mode == Mode::Dual { 32 } else { 53 }, ..Config::A2DP_HQ };
        let r = round_trip(cfg, [(1000.0, 12000.0), (3000.0, 6000.0)], 120);
        assert!(r[0] > 25.0 && r[1] > 25.0, "{:?} SNR {:?}", mode, r);
    }
}

/// A2DPKNOB (rmbp-ledger B750): the A2DP specification's recommended SBC encoder settings, the 48 kHz column (the
/// 44.1 kHz column is in [`kat`], which the kernel runs too): (channel mode, bitpool) -> frame length in bytes and bit
/// rate in kb/s — middle quality mono 18 -> 44 / 132 and joint stereo 33 -> 79 / 237, high quality mono 29 -> 66 / 198
/// and joint stereo 51 -> 115 / 345 (16 blocks, 8 subbands, loudness). Each length is also the SBC frame_length
/// formula worked by hand here, and the header byte is the frame syntax's bit fields. NOT BYTE-EXACT: no
/// SIG-published SBC frame's bytes are in the tree or on the bench, and none are invented; a byte-exact KAT
/// against a published frame stays owed.
#[test]
fn a2dp_recommended_set_48k() {
    let set = |mode, bitpool| Config { freq: Freq::F48000, mode, bitpool, ..Config::A2DP_HQ };
    for (cfg, len, kbps) in [(set(Mode::Mono, 18), 44, 132), (set(Mode::Joint, 33), 79, 237), (set(Mode::Mono, 29), 66, 198), (set(Mode::Joint, 51), 115, 345)] {
        // frame_length = 4 + (4 x subbands x channels) / 8 + ceil(data bits / 8); data = blocks x channels x bitpool
        // (mono), subbands + blocks x bitpool (joint stereo).
        let data = if cfg.mode == Mode::Joint { 8 + 16 * cfg.bitpool as usize } else { 16 * cfg.bitpool as usize };
        let by_hand = 4 + (4 * 8 * cfg.channels()) / 8 + data.div_ceil(8);
        assert_eq!((cfg.frame_len(), by_hand), (len, len), "{:?} bitpool {} frame length", cfg.mode, cfg.bitpool);
        assert_eq!((cfg.bit_rate() + 500) / 1000, kbps, "{:?} bitpool {} bit rate", cfg.mode, cfg.bitpool);
    }
    // 48 kHz = 0b11, 16 blocks = 0b11, joint stereo = 0b11, loudness = 0, 8 subbands = 1.
    let hq = set(Mode::Joint, 51);
    assert_eq!(hq.header(), [0x9C, 0b1111_1101, 51]);
    let mut enc = Encoder::new(hq);
    let mut f = [0u8; 128];
    assert_eq!(enc.encode(&[0i16; 256], &mut f), 115);
    assert_eq!(f[..3], [0x9C, 0xFD, 0x33]);
}

// ── SBCFRAME (LEDGER SR163): an INDEPENDENT integer decoder and the specification's bit allocation re-typed beside
// the encoder's. Nothing below calls the encoder's allocation, CRC, frame-length or quantizer code: the frame is
// parsed bit by bit from the SBC frame syntax (syncword, sampling_frequency 2, blocks 2, channel_mode 2,
// allocation_method 1, subbands 1, bitpool 8, crc_check 8, join[subbands] in JOINT_STEREO, scale_factor 4 bits per
// channel and subband, then audio_sample[block][channel][subband] in bits[ch][sb] bits, padding to the byte), the CRC is
// re-run bit-serially over the syntax's coverage (header byte, bitpool, join..last scale factor; x^8+x^4+x^3+x^2+1,
// init 0x0F), the allocation is [`spec_alloc`] (the specification's pseudo-code, mono/dual per channel and
// stereo/joint over both channels written as separate passes, `max_bitneed` starting at 0 as printed), dequantization
// and synthesis are integer only (Q12 subband samples, the matrixing N[k][i] = cos((i + 1/2)(k + M/2) pi / M) from the
// exact cos(n pi / 32) table below, the window D = -M x C). LIMIT, said rather than hidden: the specification's printed
// prototype table is not in the tree or on the bench (no A2DP/SBC document offline), so the synthesis window is the
// crate's own `C8`/`C4` (already the encoder's analysis prototype); every other number here is the frame syntax, a
// cosine, or an outside Python transcription of the allocation pseudo-code (the three documented cases).

/// cos(n pi / 32) x 2^15, n = 0..=16 (exact to the nearest integer; a cosine, not a specification table).
const COS32: [i64; 17] = [32768, 32610, 32138, 31357, 30274, 28899, 27246, 25330, 23170, 20788, 18205, 15447, 12540, 9512, 6393, 3212, 0];

/// cos(n pi / 32) in Q15 for any integer n (period 64, cos(2 pi - x) = cos x, cos(pi - x) = -cos x).
fn cos32(n: i64) -> i64 {
    let n = n.rem_euclid(64);
    let n = if n > 32 { 64 - n } else { n } as usize;
    if n <= 16 { COS32[n] } else { -COS32[32 - n] }
}

/// The specification's loudness offsets, typed again here: [sampling_frequency][subband].
const SPEC_OFFSET4: [[i32; 4]; 4] = [[-1, 0, 0, 0], [-2, 0, 0, 1], [-2, 0, 0, 1], [-2, 0, 0, 1]];
const SPEC_OFFSET8: [[i32; 8]; 4] = [[-2, 0, 0, 0, 0, 0, 0, 1], [-3, 0, 0, 0, 0, 0, 1, 2], [-4, 0, 0, 0, 0, 0, 1, 2], [-4, 0, 0, 0, 0, 0, 1, 2]];

/// The specification's bit allocation for the channels in `chs` sharing one bitpool, as its pseudo-code is printed.
fn spec_alloc_pass(fs: usize, snr: bool, nsb: usize, bitpool: i32, scf: &[[u8; 8]; 2], chs: &[usize], bits: &mut [[u8; 8]; 2]) {
    let mut bitneed = [[0i32; 8]; 2];
    for &ch in chs {
        for sb in 0..nsb {
            let s = scf[ch][sb] as i32;
            bitneed[ch][sb] = if snr {
                s
            } else if s == 0 {
                -5
            } else {
                let loudness = s - if nsb == 4 { SPEC_OFFSET4[fs][sb] } else { SPEC_OFFSET8[fs][sb] };
                if loudness > 0 { loudness / 2 } else { loudness }
            };
        }
    }
    let mut max_bitneed = 0;
    for &ch in chs {
        for sb in 0..nsb {
            if bitneed[ch][sb] > max_bitneed {
                max_bitneed = bitneed[ch][sb];
            }
        }
    }
    let (mut bitcount, mut slicecount, mut bitslice) = (0i32, 0i32, max_bitneed + 1);
    let mut guard = 0;
    loop {
        bitslice -= 1;
        bitcount += slicecount;
        slicecount = 0;
        for &ch in chs {
            for sb in 0..nsb {
                if bitneed[ch][sb] > bitslice + 1 && bitneed[ch][sb] < bitslice + 16 {
                    slicecount += 1;
                } else if bitneed[ch][sb] == bitslice + 1 {
                    slicecount += 2;
                }
            }
        }
        guard += 1;
        assert!(guard < 1000, "bitpool beyond the allocation's capacity");
        if bitcount + slicecount >= bitpool {
            break;
        }
    }
    if bitcount + slicecount == bitpool {
        bitcount += slicecount;
        bitslice -= 1;
    }
    for &ch in chs {
        for sb in 0..nsb {
            bits[ch][sb] = if bitneed[ch][sb] < bitslice + 2 { 0 } else { (bitneed[ch][sb] - bitslice).min(16) as u8 };
        }
    }
    let last = chs.len() - 1;
    let (mut ci, mut sb) = (0usize, 0usize);
    while bitcount < bitpool && sb < nsb {
        let ch = chs[ci];
        if bits[ch][sb] >= 2 && bits[ch][sb] < 16 {
            bits[ch][sb] += 1;
            bitcount += 1;
        } else if bitneed[ch][sb] == bitslice + 1 && bitpool > bitcount + 1 {
            bits[ch][sb] = 2;
            bitcount += 2;
        }
        if ci == last {
            ci = 0;
            sb += 1;
        } else {
            ci += 1;
        }
    }
    let (mut ci, mut sb) = (0usize, 0usize);
    while bitcount < bitpool && sb < nsb {
        let ch = chs[ci];
        if bits[ch][sb] < 16 {
            bits[ch][sb] += 1;
            bitcount += 1;
        }
        if ci == last {
            ci = 0;
            sb += 1;
        } else {
            ci += 1;
        }
    }
}

/// The specification's allocation by channel mode: MONO and DUAL_CHANNEL each channel alone with the whole bitpool,
/// STEREO and JOINT_STEREO both channels over one bitpool. `mode` and `fs` are the header's 2-bit codes.
fn spec_alloc(fs: u8, mode: u8, snr: bool, nsb: usize, bitpool: u8, scf: &[[u8; 8]; 2]) -> [[u8; 8]; 2] {
    let mut bits = [[0u8; 8]; 2];
    let (fs, bp) = (fs as usize, bitpool as i32);
    match mode {
        0 => spec_alloc_pass(fs, snr, nsb, bp, scf, &[0], &mut bits),
        1 => {
            spec_alloc_pass(fs, snr, nsb, bp, scf, &[0], &mut bits);
            spec_alloc_pass(fs, snr, nsb, bp, scf, &[1], &mut bits);
        }
        _ => spec_alloc_pass(fs, snr, nsb, bp, scf, &[0, 1], &mut bits),
    }
    bits
}

/// The three documented cases: (sampling_frequency, channel mode, allocation, subbands, blocks, bitpool) -> bits.
/// The expected arrays were computed when this test was written by an outside Python transcription of the
/// specification's allocation pseudo-code (not by this crate): blocks do not enter the allocation, they are named so
/// the case is a whole frame configuration.
#[test]
fn spec_alloc_three_documented_cases() {
    struct Case {
        cfg: Config,
        scf: [[u8; 8]; 2],
        bits: [[u8; 8]; 2],
    }
    let cases = [
        // 1. 44.1 kHz JOINT_STEREO LOUDNESS, 8 subbands, 16 blocks, bitpool 53 (the A2DP high-quality set).
        Case { cfg: Config::A2DP_HQ, scf: [[8, 7, 6, 5, 4, 3, 1, 0], [7, 7, 5, 4, 3, 2, 0, 0]], bits: [[8, 5, 5, 4, 4, 2, 0, 0], [7, 5, 4, 4, 3, 2, 0, 0]] },
        // 2. 48 kHz MONO SNR, 4 subbands, 8 blocks, bitpool 31.
        Case {
            cfg: Config { freq: Freq::F48000, blocks: 8, mode: Mode::Mono, alloc: Alloc::Snr, subbands: 4, bitpool: 31 },
            scf: [[9, 6, 3, 0, 0, 0, 0, 0], [0; 8]],
            bits: [[13, 9, 6, 3, 0, 0, 0, 0], [0; 8]],
        },
        // 3. 16 kHz DUAL_CHANNEL LOUDNESS, 8 subbands, 4 blocks, bitpool 18 — the right channel silent (every scale
        //    factor 0, bitneed -5), so its bits come from the slice descent and the two distribution passes alone.
        Case {
            cfg: Config { freq: Freq::F16000, blocks: 4, mode: Mode::Dual, alloc: Alloc::Loudness, subbands: 8, bitpool: 18 },
            scf: [[5, 4, 4, 3, 2, 2, 1, 1], [0; 8]],
            bits: [[5, 4, 3, 2, 2, 2, 0, 0], [3, 3, 2, 2, 2, 2, 2, 2]],
        },
    ];
    for (n, c) in cases.iter().enumerate() {
        assert!(c.cfg.valid(), "case {}", n + 1);
        let spec = spec_alloc(c.cfg.freq.code(), c.cfg.mode.code(), c.cfg.alloc == Alloc::Snr, c.cfg.subbands as usize, c.cfg.bitpool, &c.scf);
        assert_eq!(spec, c.bits, "case {}: the re-typed specification allocation", n + 1);
        let mut enc = [[0u8; 8]; 2];
        bit_alloc(&c.cfg, &c.scf, &mut enc);
        assert_eq!(enc, c.bits, "case {}: the encoder's bit_alloc", n + 1);
    }
}

/// The encoder's `bit_alloc` equals the re-typed specification allocation over every sampling frequency, channel
/// mode, allocation method, subband count and legal bitpool, on pseudo-random scale factors (a fixed LCG).
#[test]
fn spec_alloc_equals_bit_alloc_sweep() {
    let mut seed = 0x5BC0_2026u32;
    let mut next = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (seed >> 16) & 0x7FFF
    };
    let mut checked = 0u32;
    for freq in [Freq::F16000, Freq::F32000, Freq::F44100, Freq::F48000] {
        for mode in [Mode::Mono, Mode::Dual, Mode::Stereo, Mode::Joint] {
            for alloc in [Alloc::Loudness, Alloc::Snr] {
                for subbands in [4u8, 8] {
                    let max = if matches!(mode, Mode::Mono | Mode::Dual) { 16 * subbands as u32 } else { 32 * subbands as u32 };
                    for bitpool in (2..=max.min(250)).step_by(3) {
                        let cfg = Config { freq, blocks: 16, mode, alloc, subbands, bitpool: bitpool as u8 };
                        for _ in 0..4 {
                            let mut scf = [[0u8; 8]; 2];
                            let quiet = next() % 4 == 0;
                            for ch in 0..2 {
                                for sb in 0..subbands as usize {
                                    scf[ch][sb] = if quiet { (next() % 3) as u8 } else { (next() % 16) as u8 };
                                }
                            }
                            let spec = spec_alloc(freq.code(), mode.code(), alloc == Alloc::Snr, subbands as usize, cfg.bitpool, &scf);
                            let mut enc = [[0u8; 8]; 2];
                            bit_alloc(&cfg, &scf, &mut enc);
                            assert_eq!(enc, spec, "{:?} scf {:?}", cfg, scf);
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(checked > 5000, "{} cases", checked);
}

/// What the integer decoder read from one frame.
struct IntFrame {
    cfg: Config,
    join: [bool; 8],
    scf: [[u8; 8]; 2],
    bits: [[u8; 8]; 2],
    /// frame_length from the frame syntax's formula, worked here.
    len: usize,
    /// The bit position after the last audio sample.
    end: usize,
}

/// One bit of `f` at position `p`, MSB first.
fn bit_at(f: &[u8], p: usize) -> u32 {
    ((f[p / 8] >> (7 - p % 8)) & 1) as u32
}

fn bits_at(f: &[u8], pos: &mut usize, n: u8) -> u32 {
    let mut v = 0;
    for _ in 0..n {
        v = (v << 1) | bit_at(f, *pos);
        *pos += 1;
    }
    v
}

/// The integer synthesis state: V (20 x subbands values, Q12) per channel.
struct IntSynth {
    v: [[i64; 160]; 2],
}

/// Fractional bits of the decoder's subband samples.
const DQ_FRAC: u32 = 12;

/// Decode ONE frame with integers only: parse, CRC, allocate, dequantize, un-join, synthesize; PCM appended per channel.
fn int_decode(f: &[u8], st: &mut IntSynth, pcm: &mut [Vec<i16>; 2]) -> IntFrame {
    let mut p = 0usize;
    assert_eq!(bits_at(f, &mut p, 8), 0x9C, "syncword");
    let fs = bits_at(f, &mut p, 2) as u8;
    let nbl = [4usize, 8, 12, 16][bits_at(f, &mut p, 2) as usize];
    let mode = bits_at(f, &mut p, 2) as u8;
    let snr = bits_at(f, &mut p, 1) == 1;
    let nsb = if bits_at(f, &mut p, 1) == 1 { 8usize } else { 4 };
    let bitpool = bits_at(f, &mut p, 8) as u8;
    let crc = bits_at(f, &mut p, 8) as u8;
    let nch = if mode == 0 { 1 } else { 2 };
    let mut join = [false; 8];
    if mode == 3 {
        for sb in 0..nsb {
            join[sb] = bits_at(f, &mut p, 1) == 1;
        }
    }
    let mut scf = [[0u8; 8]; 2];
    for ch in 0..nch {
        for sb in 0..nsb {
            scf[ch][sb] = bits_at(f, &mut p, 4) as u8;
        }
    }
    // crc_check: bit-serial over the header byte, the bitpool, and join..the last scale factor.
    let mut c = 0x0Fu8;
    for q in (8..24).chain(32..p) {
        let fb = (c >> 7) as u32 ^ bit_at(f, q);
        c = (c << 1) ^ if fb == 1 { 0x1D } else { 0 };
    }
    assert_eq!(crc, c, "crc_check");
    let bits = spec_alloc(fs, mode, snr, nsb, bitpool, &scf);
    // frame_length = 4 + (4 x nrof_subbands x nrof_channels) / 8 + ceil(data / 8).
    let (nb, bp) = (nbl, bitpool as usize);
    let data = match mode {
        0 | 1 => nb * nch * bp,
        2 => nb * bp,
        _ => nsb + nb * bp,
    };
    let len = 4 + (4 * nsb * nch) / 8 + data.div_ceil(8);
    let m = nsb as i64;
    for _blk in 0..nbl {
        let mut s = [[0i64; 8]; 2];
        for ch in 0..nch {
            for sb in 0..nsb {
                let b = bits[ch][sb];
                if b > 0 {
                    let q = bits_at(f, &mut p, b) as i64;
                    let levels = (1i64 << b) - 1;
                    // sb_sample = scale_factor x ((2 x audio_sample + 1) / levels - 1), scale_factor = 2^(scf + 1).
                    s[ch][sb] = ((2 * q + 1 - levels) << (scf[ch][sb] as u32 + 1 + DQ_FRAC)) / levels;
                }
            }
        }
        for sb in 0..nsb {
            if join[sb] {
                let (mm, ss) = (s[0][sb], s[1][sb]);
                s[0][sb] = mm + ss;
                s[1][sb] = mm - ss;
            }
        }
        for ch in 0..nch {
            let v = &mut st.v[ch];
            let l = 20 * nsb;
            for i in (2 * nsb..l).rev() {
                v[i] = v[i - 2 * nsb];
            }
            // V[k] = sum_i N[k][i] S[i], N[k][i] = cos((2i + 1)(2k + M) pi / 4M) = cos(n pi / 32), n = (2i+1)(2k+M) 8/M.
            for k in 0..2 * nsb {
                let mut acc = 0i64;
                for i in 0..nsb {
                    acc += cos32((2 * i as i64 + 1) * (2 * k as i64 + m) * 8 / m) * s[ch][i];
                }
                v[k] = acc >> 15;
            }
            // U from V; W = D x U with D = -M x C (Q24); x[j] = sum_i W[j + M i].
            let cproto: &[i32] = if nsb == 8 { &C8 } else { &C4 };
            let mut w = [0i64; 80];
            for i in 0..5 {
                for j in 0..nsb {
                    let (u0, u1) = (v[i * 4 * nsb + j], v[i * 4 * nsb + 3 * nsb + j]);
                    w[i * 2 * nsb + j] = -m * cproto[i * 2 * nsb + j] as i64 * u0;
                    w[i * 2 * nsb + nsb + j] = -m * cproto[i * 2 * nsb + nsb + j] as i64 * u1;
                }
            }
            for j in 0..nsb {
                let mut x = 0i64;
                for i in 0..10 {
                    x += w[j + nsb * i];
                }
                let x = ((x >> 24) + (1 << (DQ_FRAC - 1))) >> DQ_FRAC;
                pcm[ch].push(x.clamp(i16::MIN as i64, i16::MAX as i64) as i16);
            }
        }
    }
    let freq = [Freq::F16000, Freq::F32000, Freq::F44100, Freq::F48000][fs as usize];
    let mode = [Mode::Mono, Mode::Dual, Mode::Stereo, Mode::Joint][mode as usize];
    let cfg = Config { freq, blocks: nbl as u8, mode, alloc: if snr { Alloc::Snr } else { Alloc::Loudness }, subbands: nsb as u8, bitpool };
    IntFrame { cfg, join, scf, bits, len, end: p }
}

/// The integer decoder reads every field the frame syntax defines: the configuration back, a silence frame's zero
/// scale factors and join bits, the CRC, frame_length, and the audio samples ending inside the last byte when the
/// allocation spent the whole bitpool.
#[test]
fn int_decoder_reads_the_frame_syntax() {
    let mut st = IntSynth { v: [[0; 160]; 2] };
    let mut pcm = [Vec::new(), Vec::new()];
    let mut f = [0u8; 600];
    let mut enc = Encoder::new(Config::A2DP_HQ);
    assert_eq!(enc.encode(&[0i16; 256], &mut f), 119);
    let d = int_decode(&f, &mut st, &mut pcm);
    assert_eq!((d.cfg, d.len, d.join, d.scf), (Config::A2DP_HQ, 119, [false; 8], [[0; 8]; 2]));
    assert!(pcm[0].iter().chain(pcm[1].iter()).all(|&s| s == 0), "silence decodes to silence");
    let cfgs = [
        Config::A2DP_HQ,
        Config { bitpool: 35, ..Config::A2DP_HQ },
        Config { freq: Freq::F48000, mode: Mode::Mono, bitpool: 29, ..Config::A2DP_HQ },
        Config { freq: Freq::F32000, blocks: 8, mode: Mode::Mono, alloc: Alloc::Snr, subbands: 4, bitpool: 31 },
        Config { freq: Freq::F16000, blocks: 12, mode: Mode::Dual, subbands: 4, bitpool: 20, ..Config::A2DP_HQ },
        Config { blocks: 4, mode: Mode::Stereo, alloc: Alloc::Snr, bitpool: 64, ..Config::A2DP_HQ },
    ];
    for cfg in cfgs {
        let (nch, spf) = (cfg.channels(), cfg.samples());
        let mut x = vec![0i16; spf * nch];
        for n in 0..spf {
            for ch in 0..nch {
                // A deterministic two-slope ramp per channel (integers): every subband carries something.
                x[n * nch + ch] = ((((n * (5 + 3 * ch)) % 64) as i32 - 32) * 600) as i16;
            }
        }
        let mut enc = Encoder::new(cfg);
        let mut st = IntSynth { v: [[0; 160]; 2] };
        let mut pcm = [Vec::new(), Vec::new()];
        for _ in 0..3 {
            for b in f.iter_mut() {
                *b = 0;
            }
            assert_eq!(enc.encode(&x, &mut f), cfg.frame_len());
            let d = int_decode(&f, &mut st, &mut pcm);
            assert_eq!(d.cfg, cfg);
            assert_eq!(d.len, cfg.frame_len(), "{:?}", cfg);
            assert!(d.scf[..nch].iter().any(|s| s.iter().any(|&k| k > 0)), "{:?}", cfg);
            let spent: usize = d.bits[..nch].iter().map(|r| r.iter().map(|&b| b as usize).sum::<usize>()).sum();
            let full = if matches!(cfg.mode, Mode::Dual) { 2 * cfg.bitpool as usize } else { cfg.bitpool as usize };
            assert!(d.end <= d.len * 8, "{:?}", cfg);
            if spent == full {
                assert!(d.len * 8 - d.end < 8, "{:?}: {} bits of padding", cfg, d.len * 8 - d.end);
            }
            if !matches!(cfg.mode, Mode::Joint) {
                assert_eq!(d.join, [false; 8]);
            }
        }
        assert_eq!(pcm[0].len(), 3 * spf);
    }
}

/// THE ROUND TRIP: a known 128-sample sine (one period = 128 samples, 344.5 Hz at 44.1 kHz; left at -6 dBFS, right
/// its third harmonic at half level) through the encoder and the integer decoder; the error energy after the filter
/// bank's delay (searched over 0..300 samples, no gain fitted) must sit 30 dB under the signal (25 dB for 4 subbands
/// at bitpool 31). The specification's own conformance bound is not in the tree; 30 / 25 dB are the floors the float
/// round trips above already hold.
#[test]
fn int_round_trip_sine_128() {
    let sine: Vec<i16> = (0..128).map(|n| (16384.0 * (2.0 * std::f64::consts::PI * n as f64 / 128.0).sin()).round() as i16).collect();
    let cases = [(Config::A2DP_HQ, 24usize, 1000i64), (Config { freq: Freq::F44100, blocks: 8, mode: Mode::Mono, alloc: Alloc::Snr, subbands: 4, bitpool: 31 }, 96, 316)];
    for (cfg, frames, ratio) in cases {
        let (nch, spf) = (cfg.channels(), cfg.samples());
        let total = spf * frames;
        let src = |ch: usize, n: usize| -> i16 { if ch == 0 { sine[n % 128] } else { sine[(3 * n) % 128] / 2 } };
        let mut enc = Encoder::new(cfg);
        let mut st = IntSynth { v: [[0; 160]; 2] };
        let mut pcm = [Vec::new(), Vec::new()];
        let mut f = vec![0u8; cfg.frame_len()];
        for fr in 0..frames {
            let mut x = vec![0i16; spf * nch];
            for n in 0..spf {
                for ch in 0..nch {
                    x[n * nch + ch] = src(ch, fr * spf + n);
                }
            }
            assert_eq!(enc.encode(&x, &mut f), cfg.frame_len());
            int_decode(&f, &mut st, &mut pcm);
        }
        for ch in 0..nch {
            let y = &pcm[ch];
            assert_eq!(y.len(), total);
            let (from, to) = (512, total - 300);
            let mut best = (i64::MAX, 0usize, 0i64);
            for d in 0..300 {
                let (mut sig, mut err) = (0i64, 0i64);
                for n in from..to {
                    let (a, b) = (src(ch, n) as i64, y[n + d] as i64);
                    sig += a * a;
                    err += (a - b) * (a - b);
                }
                if err < best.0 {
                    best = (err, d, sig);
                }
            }
            let (err, d, sig) = best;
            assert!(sig >= ratio * err, "{:?} ch {}: delay {} signal {} error {} (ratio {})", cfg.mode, ch, d, sig, err, sig / err.max(1));
            eprintln!("SBCFRAME int round trip {:?} {} subbands ch {}: delay {} SNR ratio {}", cfg.mode, cfg.subbands, ch, d, sig / err.max(1));
        }
    }
}
