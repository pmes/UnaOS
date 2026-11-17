// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — the one level meter (NLE4C, LEDGER SR301; shared-core, BP-handlers-14): peak with hold and decay,
//! RMS over a sliding 50 ms window, both in dBFS.
//!
//! A frame's peak is its largest `|x|` over the channels and its power the mean of the channels' squares (so a full-scale
//! sine reads 0 dBFS peak and -3.01 dB RMS on any channel count). The peak holds for [`HOLD_MS`] after it was set, then
//! falls [`DECAY_DB_PER_S`] (a per-sample factor: a straight line in dB); a louder sample re-arms it. Every reading is
//! clamped to [`FLOOR_DB`]. Time is counted in the samples pushed: a meter nothing is fed to stands still.

use alloc::vec;
use alloc::vec::Vec;

use crate::math;

/// The quietest reading (silence).
pub const FLOOR_DB: f64 = -96.0;
/// The RMS window.
pub const WINDOW_MS: u32 = 50;
/// How long a peak holds before it decays.
pub const HOLD_MS: u32 = 500;
/// The peak's fall once the hold has run out.
pub const DECAY_DB_PER_S: f64 = 20.0;

/// `lin` (full scale 1.0) in dBFS, clamped to [`FLOOR_DB`].
pub fn dbfs(lin: f64) -> f64 {
    if !(lin > 0.0) {
        return FLOOR_DB;
    }
    (20.0 * math::log10(lin)).max(FLOOR_DB)
}

/// One meter (see the module docs).
#[derive(Debug, Clone)]
pub struct Meter {
    rate: u32,
    /// The window's per-frame powers, a ring.
    ring: Vec<f64>,
    pos: usize,
    /// The held peak (linear).
    peak: f64,
    /// Frames of hold left.
    hold_left: usize,
    hold: usize,
    /// The per-frame decay factor.
    decay: f64,
}

impl Meter {
    pub fn new(rate: u32) -> Meter {
        let rate = rate.max(1);
        let win = ((rate as u64 * WINDOW_MS as u64 / 1000) as usize).max(1);
        let hold = (rate as u64 * HOLD_MS as u64 / 1000) as usize;
        let decay = math::pow(10.0, -DECAY_DB_PER_S / 20.0 / rate as f64);
        Meter { rate, ring: vec![0.0; win], pos: 0, peak: 0.0, hold_left: 0, hold, decay }
    }
    /// The rate this meter counts time at.
    pub fn rate(&self) -> u32 {
        self.rate
    }
    /// Feed interleaved samples of `ch` channels.
    pub fn push(&mut self, interleaved: &[f32], ch: usize) {
        let ch = ch.max(1);
        for frame in interleaved.chunks(ch) {
            let (mut p, mut s) = (0f64, 0f64);
            for &x in frame {
                let x = x as f64;
                p = p.max(x.abs());
                s += x * x;
            }
            self.ring[self.pos] = s / frame.len() as f64;
            self.pos = (self.pos + 1) % self.ring.len();
            if p >= self.peak {
                self.peak = p;
                self.hold_left = self.hold;
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.peak *= self.decay;
            }
        }
    }
    /// The held peak, dBFS.
    pub fn peak_db(&self) -> f64 {
        dbfs(self.peak)
    }
    /// The RMS over the last [`WINDOW_MS`] (frames not yet fed count as silence), dBFS.
    pub fn rms_db(&self) -> f64 {
        let mean = self.ring.iter().sum::<f64>() / self.ring.len() as f64;
        if !(mean > 0.0) {
            return FLOOR_DB;
        }
        (10.0 * math::log10(mean)).max(FLOOR_DB)
    }
    /// Back to silence.
    pub fn reset(&mut self) {
        self.ring.iter_mut().for_each(|s| *s = 0.0);
        (self.pos, self.peak, self.hold_left) = (0, 0.0, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, frames: usize) -> Vec<f32> {
        (0..frames).map(|i| math::sin(2.0 * math::PI * hz * i as f64 / rate as f64) as f32).collect()
    }

    /// A full-scale 1 kHz sine: peak 0 dBFS, RMS -3.01 dB (1/sqrt 2), mono and as identical stereo.
    #[test]
    fn meter_sine_peak_and_rms() {
        let s = sine(48_000, 1000.0, 4800);
        let mut m = Meter::new(48_000);
        m.push(&s, 1);
        assert!(m.peak_db().abs() < 0.01, "{}", m.peak_db());
        assert!((m.rms_db() + 3.0103).abs() < 0.01, "{}", m.rms_db());
        let st: Vec<f32> = s.iter().flat_map(|&x| [x, x]).collect();
        let mut m2 = Meter::new(48_000);
        m2.push(&st, 2);
        assert_eq!((m2.peak_db(), m2.rms_db()), (m.peak_db(), m.rms_db()));
    }

    /// Silence (and a fresh meter) reads the floor exactly.
    #[test]
    fn meter_silence_is_the_floor() {
        let mut m = Meter::new(44_100);
        assert_eq!((m.peak_db(), m.rms_db()), (FLOOR_DB, FLOOR_DB));
        m.push(&[0.0; 4410], 1);
        assert_eq!((m.peak_db(), m.rms_db()), (FLOOR_DB, FLOOR_DB));
        assert_eq!(dbfs(1e-9), FLOOR_DB);
    }

    /// A full-scale impulse holds 0 dBFS for HOLD_MS, then falls 20 dB in the next second; the RMS has left the window.
    #[test]
    fn meter_peak_holds_then_decays() {
        let rate = 48_000u32;
        let hold = (rate * HOLD_MS / 1000) as usize;
        let mut m = Meter::new(rate);
        m.push(&[1.0], 1);
        m.push(&vec![0.0; hold], 1);
        assert!(m.peak_db().abs() < 1e-9, "{}", m.peak_db());
        assert_eq!(m.rms_db(), FLOOR_DB);
        m.push(&vec![0.0; rate as usize], 1);
        assert!((m.peak_db() + 20.0).abs() < 0.01, "{}", m.peak_db());
        m.reset();
        assert_eq!(m.peak_db(), FLOOR_DB);
    }
}

// ---- STRIALOUDNESSREADOUT (LEDGER SR866): BS.1770 integrated loudness (named tail block) ----
//
// ITU-R BS.1770-4: each channel K-weighted (a high shelf, then the RLB high-pass), the channel-weighted mean square over
// 400 ms blocks at 75 % overlap (four 100 ms hops), each block's loudness `-0.691 + 10 log10(z)`; the absolute gate
// drops blocks at or under -70 LUFS, the relative gate those at or under (the loudness of the rest) - 10 LU, and the
// integrated loudness is that of the mean `z` of what both gates keep. The filters are designed at any rate by the
// bilinear transform from the analogue parameters that reproduce the standard's 48 kHz table. Streaming in [`Meter`]'s
// shape (one f64 per 100 ms is all it keeps), [`integrated_lufs`] the one pure fn over a buffer.

/// The reading when no block passes the absolute gate (silence, or less than one 400 ms block): the gate itself.
pub const LUFS_FLOOR: f64 = -70.0;
/// Hops per gating block (400 ms of 100 ms hops, 75 % overlap).
const LUFS_BLOCK_HOPS: usize = 4;

/// One biquad's coefficients `[b0, b1, b2, a1, a2]` (a0 = 1).
type Biquad = [f64; 5];

/// The K-weighting at `rate`: (stage 1 the high shelf, stage 2 the RLB high-pass).
fn k_weighting(rate: u32) -> (Biquad, Biquad) {
    let fs = rate.max(1) as f64;
    let tan = |x: f64| {
        let (s, c) = math::sincos(x);
        s / c
    };
    let (f0, g, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = tan(math::PI * f0 / fs);
    let vh = math::pow(10.0, g / 20.0);
    let vb = math::pow(vh, 0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = tan(math::PI * f0 / fs);
    let a0 = 1.0 + k / q + k * k;
    let rlb = [1.0, -2.0, 1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];
    (shelf, rlb)
}

/// One sample through a transposed direct-form-II biquad with state `s`.
fn biquad(c: &Biquad, x: f64, s: &mut [f64]) -> f64 {
    let y = c[0] * x + s[0];
    s[0] = c[1] * x - c[3] * y + s[1];
    s[1] = c[2] * x - c[4] * y;
    y
}

/// A mean square as loudness (LUFS).
fn lufs_of(z: f64) -> f64 {
    -0.691 + 10.0 * math::log10(z)
}

/// The BS.1770 integrated loudness meter (see the block comment above).
#[derive(Debug, Clone)]
pub struct Loudness {
    ch: usize,
    shelf: Biquad,
    rlb: Biquad,
    /// Per channel: the shelf's two state words, then the high-pass's.
    state: Vec<[f64; 4]>,
    /// The channel weights G (1.0; 5.1 as L R C LFE Ls Rs: 1, 1, 1, 0, 1.41, 1.41).
    weights: Vec<f64>,
    /// Frames per 100 ms hop.
    hop: usize,
    /// Frames into the open hop, and its weighted sum of squares.
    n: usize,
    acc: f64,
    /// Each closed hop's weighted sum of squares.
    hops: Vec<f64>,
    tp: Option<crate::resample::Sinc>, tp_buf: Vec<f32>, peak: f64, // STRIALOUDNESSNEXT (SR995): the true-peak oversampler, its output buffer, the largest |x| seen (file tail)
}

impl Loudness {
    pub fn new(rate: u32, ch: usize) -> Loudness {
        let ch = ch.max(1);
        let (shelf, rlb) = k_weighting(rate);
        let weights = if ch == 6 { vec![1.0, 1.0, 1.0, 0.0, 1.41, 1.41] } else { vec![1.0; ch] };
        let hop = ((rate.max(1) as usize + 5) / 10).max(1);
        Loudness { ch, shelf, rlb, state: vec![[0.0; 4]; ch], weights, hop, n: 0, acc: 0.0, hops: Vec::new(), tp: tp_oversampler(rate, ch), tp_buf: Vec::new(), peak: 0.0 } // STRIALOUDNESSNEXT (SR995): the true-peak state (file tail)
    }
    /// Feed interleaved samples of the meter's channel count (a trailing partial frame is ignored).
    pub fn push(&mut self, interleaved: &[f32]) { self.tp_push(interleaved); // STRIALOUDNESSNEXT (SR995): the true peak over the same samples (file tail)
        for frame in interleaved.chunks_exact(self.ch) {
            let mut sum = 0.0;
            for (i, &x) in frame.iter().enumerate() {
                let s = &mut self.state[i];
                let y = biquad(&self.shelf, x as f64, &mut s[0..2]);
                let y = biquad(&self.rlb, y, &mut s[2..4]);
                sum += self.weights[i] * y * y;
            }
            self.acc += sum;
            self.n += 1;
            if self.n == self.hop {
                self.hops.push(self.acc);
                (self.n, self.acc) = (0, 0.0);
            }
        }
    }
    /// The gated integrated loudness of everything pushed, LUFS; [`LUFS_FLOOR`] when no block passes the absolute gate.
    pub fn integrated(&self) -> f64 {
        if self.hops.len() < LUFS_BLOCK_HOPS {
            return LUFS_FLOOR;
        }
        let z = self.blocks(LUFS_BLOCK_HOPS); // STRIALOUDNESSNEXT (SR995): the one block builder (file tail)
        let mean_over = |gate: f64| gate_mean(&z, gate); // STRIALOUDNESSNEXT (SR995): the one gate (file tail)
        let Some(abs) = mean_over(LUFS_FLOOR) else { return LUFS_FLOOR };
        match mean_over(lufs_of(abs) - 10.0) {
            Some(m) => lufs_of(m),
            None => LUFS_FLOOR,
        }
    }
}

/// THE pure fn: the BS.1770 integrated loudness (LUFS) of `interleaved` (`ch` channels at `rate`).
pub fn integrated_lufs(interleaved: &[f32], ch: usize, rate: u32) -> f64 {
    let mut l = Loudness::new(rate, ch);
    l.push(interleaved);
    l.integrated()
}

#[cfg(test)]
mod strialoudnessreadout_tests {
    use super::*;

    /// `secs` of a `hz` sine whose peak is `dbfs` on each of `ch` identical channels.
    fn tone(rate: u32, ch: usize, hz: f64, dbfs: f64, secs: f64) -> Vec<f32> {
        let a = math::pow(10.0, dbfs / 20.0);
        let n = (rate as f64 * secs) as usize;
        (0..n).flat_map(|i| core::iter::repeat_n((a * math::sin(2.0 * math::PI * hz * i as f64 / rate as f64)) as f32, ch)).collect()
    }

    /// BS.1770-4 Tables 1 and 2: the 48 kHz coefficients, reproduced by the bilinear design.
    #[test]
    fn strialoudnessreadout_k_weighting_is_the_standards_48k_table() {
        let (shelf, rlb) = k_weighting(48_000);
        let want_shelf = [1.53512485958697, -2.69169618940638, 1.19839281085285, -1.69065929318241, 0.73248077421585];
        let want_rlb = [1.0, -2.0, 1.0, -1.99004745483398, 0.99007225036621];
        for (g, w) in shelf.iter().zip(want_shelf).chain(rlb.iter().zip(want_rlb)) {
            assert!((g - w).abs() < 1e-8, "{shelf:?} {rlb:?}");
        }
    }

    /// EBU Tech 3341's 997 Hz sine at -23 dBFS stereo reads -23.0 LUFS (and -33 dBFS -33.0), at 48 and 44.1 kHz.
    #[test]
    fn strialoudnessreadout_997hz_sine_reads_its_level() {
        let l = integrated_lufs(&tone(48_000, 2, 997.0, -23.0, 20.0), 2, 48_000);
        assert!((l + 23.0).abs() < 0.1, "{l}");
        let l33 = integrated_lufs(&tone(48_000, 2, 997.0, -33.0, 20.0), 2, 48_000);
        assert!((l33 + 33.0).abs() < 0.1, "{l33}");
        let l44 = integrated_lufs(&tone(44_100, 2, 997.0, -23.0, 20.0), 2, 44_100);
        assert!((l44 + 23.0).abs() < 0.1, "{l44}");
    }

    /// EBU Tech 3341 case 3: -36 / -23 / -36 dBFS for 10 / 60 / 10 s reads -23.0: the relative gate drops the quiet parts.
    #[test]
    fn strialoudnessreadout_relative_gate_drops_the_quiet_parts() {
        let mut s = tone(48_000, 2, 997.0, -36.0, 10.0);
        s.extend(tone(48_000, 2, 997.0, -23.0, 60.0));
        s.extend(tone(48_000, 2, 997.0, -36.0, 10.0));
        let l = integrated_lufs(&s, 2, 48_000);
        assert!((l + 23.0).abs() < 0.1, "{l}");
    }

    /// Silence, and a tone shorter than one 400 ms block, read the gate's floor exactly.
    #[test]
    fn strialoudnessreadout_silence_reads_the_floor() {
        assert_eq!(integrated_lufs(&vec![0.0; 2 * 48_000 * 2], 2, 48_000), LUFS_FLOOR);
        assert_eq!(integrated_lufs(&tone(48_000, 2, 997.0, -23.0, 0.3), 2, 48_000), LUFS_FLOOR);
        assert_eq!(integrated_lufs(&[], 2, 48_000), LUFS_FLOOR);
    }

    /// Pushing in odd chunks is one push: the meter streams.
    #[test]
    fn strialoudnessreadout_streaming_equals_one_push() {
        let s = tone(48_000, 2, 997.0, -20.0, 3.0);
        let mut m = Loudness::new(48_000, 2);
        for c in s.chunks(2 * 777) {
            m.push(c);
        }
        assert_eq!(m.integrated(), integrated_lufs(&s, 2, 48_000));
    }
}

// ---- STRIALOUDNESSNEXT (LEDGER SR995): loudness range and true peak beside the integrated measure (named tail block) ----
//
// EBU Tech 3342 loudness range: the short-term loudness of 3 s blocks (30 of the meter's 100 ms hops, so a block every
// 100 ms), the -70 LUFS absolute gate and a -20 LU relative gate (relative to the mean `z` of the blocks above the
// absolute gate: the integrated measure's gating, [`gate_mean`], with a wider relative gate), and the range is the
// 95th minus the 10th percentile of what both gates keep (0 LU with fewer than two). ITU-R BS.1770-4 Annex 2 true
// peak: each sample oversampled to at least 192 kHz (4x under 96 kHz, 2x under 192 kHz, none at or over) through THE
// windowed-sinc converter (`resample::Sinc`, its streaming form; no second interpolation filter), the largest `|y|`
// over the oversampled stream and the samples themselves (the converter holds its last `delay` frames; their own
// values still count), in dBTP through [`dbfs`]. Both read off the same `push`; K-weighting is not applied to either
// peak (the standard's true peak is unweighted); the range reuses the K-weighted hops the integrated measure keeps.

/// Hops per short-term block (3 s of 100 ms hops).
pub const LRA_BLOCK_HOPS: usize = 30;
/// The loudness range's relative gate, LU under the gated mean.
pub const LRA_RELATIVE_GATE: f64 = -20.0;

/// The mean of the block mean squares `z` whose loudness is above `gate` (LUFS); `None` when none is.
fn gate_mean(z: &[f64], gate: f64) -> Option<f64> {
    let (s, n) = z.iter().filter(|&&zj| lufs_of(zj) > gate).fold((0.0, 0usize), |(s, n), &zj| (s + zj, n + 1));
    (n > 0).then(|| s / n as f64)
}

/// The true-peak oversampler at `rate` (`None` at 192 kHz or over: the samples are the peak).
fn tp_oversampler(rate: u32, ch: usize) -> Option<crate::resample::Sinc> {
    let factor = if rate < 96_000 { 4 } else if rate < 192_000 { 2 } else { return None };
    Some(crate::resample::Sinc::new(rate, rate.max(1) * factor, ch))
}

impl Loudness {
    /// Each `n_hops`-hop block's mean square (the hops slide one at a time: 100 ms apart).
    fn blocks(&self, n_hops: usize) -> Vec<f64> {
        let per = (n_hops * self.hop) as f64;
        self.hops.windows(n_hops).map(|w| w.iter().sum::<f64>() / per).collect()
    }
    /// The EBU Tech 3342 loudness range of everything pushed, LU (0 when fewer than two short-term blocks pass the gates).
    pub fn range_lu(&self) -> f64 {
        let st = self.blocks(LRA_BLOCK_HOPS);
        let Some(abs) = gate_mean(&st, LUFS_FLOOR) else { return 0.0 };
        let gate = (lufs_of(abs) + LRA_RELATIVE_GATE).max(LUFS_FLOOR);
        let mut l: Vec<f64> = st.iter().map(|&z| lufs_of(z)).filter(|&lz| lz > gate).collect();
        if l.len() < 2 {
            return 0.0;
        }
        l.sort_unstable_by(f64::total_cmp);
        let at = |p: f64| l[((l.len() - 1) as f64 * p + 0.5) as usize];
        at(0.95) - at(0.10)
    }
    /// The BS.1770-4 true peak of everything pushed, dBTP ([`FLOOR_DB`] for silence).
    pub fn true_peak_db(&self) -> f64 {
        dbfs(self.peak)
    }
    /// Raise the peak over `interleaved`'s samples and their oversampled stream.
    fn tp_push(&mut self, interleaved: &[f32]) {
        let ch = self.ch;
        let x = &interleaved[..interleaved.len() / ch * ch];
        for &v in x {
            self.peak = self.peak.max(math::fabs(v as f64));
        }
        let Some(sinc) = self.tp.as_mut() else { return };
        let (from, to) = sinc.rates();
        let room = (x.len() / ch * (to / from.max(1)) as usize + sinc.taps() + 1) * ch;
        self.tp_buf.resize(room, 0.0);
        let (_, mut made) = sinc.process(x, &mut self.tp_buf);
        loop {
            for &y in &self.tp_buf[..made * ch] {
                self.peak = self.peak.max(math::fabs(y as f64));
            }
            if made * ch < room {
                break;
            }
            made = sinc.process(&[], &mut self.tp_buf).1;
        }
    }
}

#[cfg(test)]
mod strialoudnessnext_tests {
    use super::*;

    /// `secs` of a `hz` sine at `dbfs` peak, starting at `phase`, on each of `ch` identical channels.
    fn tone(rate: u32, ch: usize, hz: f64, dbfs: f64, phase: f64, secs: f64) -> Vec<f32> {
        let a = math::pow(10.0, dbfs / 20.0);
        let n = (rate as f64 * secs) as usize;
        (0..n).flat_map(|i| core::iter::repeat_n((a * math::sin(2.0 * math::PI * hz * i as f64 / rate as f64 + phase)) as f32, ch)).collect()
    }

    /// EBU Tech 3342 case 1: 20 s at -20 dBFS then 20 s at -30 dBFS (1 kHz stereo) reads 10 LU within 1; one level reads
    /// near 0; silence and less than one 3 s block read 0; the integrated measure is unchanged by the shared gate.
    #[test]
    fn strialoudnessnext_range_reads_the_step() {
        let mut s = tone(48_000, 2, 1000.0, -20.0, 0.0, 20.0);
        s.extend(tone(48_000, 2, 1000.0, -30.0, 0.0, 20.0));
        let mut m = Loudness::new(48_000, 2);
        m.push(&s);
        assert!((m.range_lu() - 10.0).abs() < 1.0, "{}", m.range_lu());
        let mut flat = Loudness::new(48_000, 2);
        flat.push(&tone(48_000, 2, 1000.0, -20.0, 0.0, 10.0));
        assert!(flat.range_lu() < 0.1, "{}", flat.range_lu());
        let mut quiet = Loudness::new(48_000, 2);
        quiet.push(&vec![0.0; 2 * 48_000 * 5]);
        assert_eq!(quiet.range_lu(), 0.0);
        let mut short = Loudness::new(48_000, 2);
        short.push(&tone(48_000, 2, 1000.0, -20.0, 0.0, 2.0));
        assert_eq!(short.range_lu(), 0.0);
        assert!((flat.integrated() + 20.0).abs() < 0.2, "{}", flat.integrated());
    }

    /// A 12 kHz sine at 48 kHz started at 45 degrees has every sample at 0.707 of its peak (-3 dB): the sample peak
    /// under-reads, the true peak reads the sine's own level within 0.5 dB; silence reads the floor.
    #[test]
    fn strialoudnessnext_true_peak_finds_the_intersample_peak() {
        let s = tone(48_000, 2, 12_000.0, -6.0, math::PI / 4.0, 1.0);
        let sample_peak = dbfs(s.iter().fold(0.0f64, |p, &v| p.max(math::fabs(v as f64))));
        assert!((sample_peak + 9.01).abs() < 0.1, "{sample_peak}");
        let mut m = Loudness::new(48_000, 2);
        for c in s.chunks(2 * 1001) {
            m.push(c);
        }
        assert!((m.true_peak_db() + 6.0).abs() < 0.5, "{}", m.true_peak_db());
        assert_eq!(Loudness::new(48_000, 2).true_peak_db(), FLOOR_DB);
        let mut hi = Loudness::new(192_000, 1);
        hi.push(&[0.5, -0.25]);
        assert!((hi.true_peak_db() - dbfs(0.5)).abs() < 1e-12);
    }
}
