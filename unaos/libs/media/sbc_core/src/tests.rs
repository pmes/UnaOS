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
