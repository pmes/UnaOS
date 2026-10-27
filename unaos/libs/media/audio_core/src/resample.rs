// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RESAMPLE (NLE4, LEDGER SR203; NLE4B, SR222) — sample-rate converters, position-addressed, for Stria's offline
//! editor render (`stria::nle`), which mixes sources of any rate at the project's.
//!
//! CHARTER: Stria — shared-core (BP-handlers-4): the converter both rings can link; no I/O, `no_std` + `alloc`.
//!
//! Output frame `k` at `to` Hz is the source at `k * from / to`, an exact integer phase ([`linear_pos`]: units of
//! `1 / to` of a source frame, so equal rates are bit-exact and nothing drifts). Two paths, interleaved `f32`, any
//! channel count:
//! * [`Sinc`] — the QUALITY path (NLE4B, the render's): a Kaiser-windowed sinc (beta [`BETA`], [`TAPS`] taps, more going
//!   down, cutoff [`CUTOFF`] of the lower rate), its table of [`PHASES`] + 1 rows built at construction and linearly
//!   interpolated between rows; position-addressed ([`Sinc::at`]) or streamed over caller buffers ([`Sinc::process`]).
//! * [`linear`] — the CHEAP path: interpolation between the two neighbouring source frames. It aliases on the way down
//!   and dulls the top octave on the way up.

use alloc::vec::Vec;

/// Where output frame `k` (at `to` Hz) falls in a stream at `from` Hz: `(source frame, phase, to)`, the position being
/// `source frame + phase / to` (`0 <= phase < to`). `k` and the source frame count from the same origin.
pub fn linear_pos(k: i64, from: u32, to: u32) -> (i64, u32, u32) {
    let to = to.max(1);
    let p = k as i128 * from as i128;
    let (i, f) = (p.div_euclid(to as i128), p.rem_euclid(to as i128));
    (i as i64, f as u32, to)
}

/// Output frames `out_first..out_first + n` at `to` Hz of the interleaved `ch`-channel stream at `from` Hz whose frame
/// `src_first` is `src[0]`, by LINEAR interpolation (see the module docs). A neighbour past either end of `src` holds
/// the nearest frame `src` has; an empty `src` gives silence.
pub fn linear(src: &[f32], src_first: i64, ch: usize, from: u32, to: u32, out_first: i64, n: usize) -> Vec<f32> {
    let ch = ch.max(1);
    let frames = (src.len() / ch) as i64;
    let mut out = Vec::with_capacity(n * ch);
    if frames == 0 {
        out.resize(n * ch, 0.0);
        return out;
    }
    let at = |i: i64, c: usize| src[((i - src_first).clamp(0, frames - 1) as usize) * ch + c];
    for k in 0..n as i64 {
        let (i, f, d) = linear_pos(out_first + k, from, to);
        for c in 0..ch {
            let a = at(i, c);
            if f == 0 {
                out.push(a);
            } else {
                let b = at(i + 1, c);
                out.push(a + (b - a) * (f as f32 / d as f32));
            }
        }
    }
    out
}

// ---- NLE4B (LEDGER SR222): the windowed-sinc QUALITY path ----

/// Taps per phase at equal or rising rates; going down they scale by `from / to` (even, at most [`MAX_TAPS`]).
pub const TAPS: usize = 32;
/// The most taps a falling ratio grows to.
pub const MAX_TAPS: usize = 512;
/// Rows of the sinc table across one source frame; a phase between two rows is linearly interpolated.
pub const PHASES: usize = 64;
/// The Kaiser window's beta (sidelobes near -80 dB).
pub const BETA: f64 = 8.0;
/// The cutoff, as a fraction of the LOWER of the two rates (0.9 of its Nyquist).
pub const CUTOFF: f64 = 0.45;

/// A Kaiser-windowed sinc converter from `from` Hz to `to` Hz, interleaved `f32`, `ch` channels. Output frame `k` is the
/// band-limited source at `k * from / to` (the exact integer phase of [`linear_pos`]), computed by ONE kernel that both
/// the position-addressed [`Sinc::at`] and the streaming [`Sinc::process`] call. Equal rates copy (bit-exact).
#[derive(Clone, Debug)]
pub struct Sinc {
    /// The ratio as given, and reduced by its gcd (the phase arithmetic runs on the reduced pair).
    rates: (u32, u32),
    from: u32,
    to: u32,
    ch: usize,
    /// Taps per phase (even; 1 at equal rates) and `(PHASES + 1) * taps` coefficients, row `r` at phase `r / PHASES`.
    taps: usize,
    table: Vec<f32>,
    /// The streaming form's state: source frames from `hist_at` on, the next output frame, the frames fed, and (after
    /// `flush`) the real stream length.
    hist: Vec<f32>,
    hist_at: i64,
    next: i64,
    fed: i64,
    end: Option<i64>,
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

impl Sinc {
    /// The converter and its table (built here: `(PHASES + 1) * taps` evaluations of the windowed sinc).
    pub fn new(from: u32, to: u32, ch: usize) -> Sinc {
        let (from, to, ch) = (from.max(1), to.max(1), ch.max(1));
        let g = gcd(from, to);
        let (rf, rt) = (from / g, to / g);
        let (taps, table) = if rf == rt {
            (1, alloc::vec![1.0f32; PHASES + 1])
        } else {
            let mut t = TAPS;
            if rf > rt {
                t = (TAPS as u64 * rf as u64).div_ceil(rt as u64) as usize;
                t = (t + (t & 1)).min(MAX_TAPS);
            }
            (t, table(t, CUTOFF * from.min(to) as f64 / from as f64))
        };
        let mut s = Sinc { rates: (from, to), from: rf, to: rt, ch, taps, table, hist: Vec::new(), hist_at: 0, next: 0, fed: 0, end: None };
        s.reset();
        s
    }
    /// The rates as given to [`Sinc::new`].
    pub fn rates(&self) -> (u32, u32) {
        self.rates
    }
    /// Taps per output frame.
    pub fn taps(&self) -> usize {
        self.taps
    }
    /// The group delay of the streaming form, in source frames: [`Sinc::process`] holds this many frames past an
    /// output's position before it yields it. [`Sinc::at`] is zero-phase (centred on `k * from / to`).
    pub fn delay(&self) -> usize {
        self.taps / 2
    }
    /// Source frames the window reaches BEFORE a position (`taps / 2 - 1`; 0 at equal rates).
    fn back(&self) -> i64 {
        ((self.taps - 1) / 2) as i64
    }
    /// Back to the stream's start (the history pre-rolled with silence).
    pub fn reset(&mut self) {
        let b = self.back();
        self.hist.clear();
        self.hist.resize(b as usize * self.ch, 0.0);
        self.hist_at = -b;
        self.next = 0;
        self.fed = 0;
        self.end = None;
    }
    /// Output frames `out_first..out_first + n` of the interleaved stream whose frame `src_first` is `src[0]`, the same
    /// addressing as [`linear`] (a neighbour past either end of `src` holds the nearest frame; an empty `src` is
    /// silence). Its window reaches [`Sinc::delay`]` - 1` frames before and `delay` frames after each position.
    pub fn at(&self, src: &[f32], src_first: i64, out_first: i64, n: usize) -> Vec<f32> {
        let mut out = alloc::vec![0.0f32; n * self.ch];
        if src.len() < self.ch {
            return out;
        }
        for (k, o) in out.chunks_mut(self.ch).enumerate() {
            let (i, f, _) = linear_pos(out_first + k as i64, self.from, self.to);
            self.frame_at(src, src_first, i, f, o);
        }
        out
    }
    /// Feed `input` (whole interleaved frames) and write as many output frames into `output` as are ready and fit:
    /// `(source frames consumed, output frames produced)`. All input is taken (held as history); outputs that did not
    /// fit come on the next call. Two calls over a split stream give the same frames as one over the whole.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> (usize, usize) {
        let frames = input.len() / self.ch;
        self.hist.extend_from_slice(&input[..frames * self.ch]);
        self.fed += frames as i64;
        (frames, self.produce(output))
    }
    /// End of stream: the window's lookahead is silence, and the outputs run to `ceil(fed * to / from)` frames in all
    /// (the count [`linear`] gives over the same span). Call again while it returns a full `output`.
    pub fn flush(&mut self, output: &mut [f32]) -> usize {
        if self.end.is_none() {
            self.end = Some(self.fed);
            let pad = self.delay().max(1) * self.ch;
            self.hist.resize(self.hist.len() + pad, 0.0);
            self.fed += self.delay().max(1) as i64;
        }
        self.produce(output)
    }
    fn produce(&mut self, output: &mut [f32]) -> usize {
        let (ch, h) = (self.ch, self.delay() as i64);
        let room = output.len() / ch;
        let mut made = 0;
        while made < room {
            let (i, f, _) = linear_pos(self.next, self.from, self.to);
            if let Some(end) = self.end {
                if i >= end {
                    break;
                }
            }
            if i + h >= self.fed {
                break;
            }
            let o = &mut output[made * ch..(made + 1) * ch];
            o.fill(0.0);
            self.frame_at(&self.hist, self.hist_at, i, f, o);
            self.next += 1;
            made += 1;
        }
        let (keep, _, _) = linear_pos(self.next, self.from, self.to);
        let drop = ((keep - self.back() - self.hist_at).max(0) as usize * ch).min(self.hist.len());
        self.hist.drain(..drop);
        self.hist_at += (drop / ch) as i64;
        made
    }
    /// THE kernel: one output frame at source position `i + f / to` into `out` (zeroed by the caller), over source
    /// frames `i - back ..= i + delay` of `src` (frame `src_first` at `src[0]`; past either end holds).
    fn frame_at(&self, src: &[f32], src_first: i64, i: i64, f: u32, out: &mut [f32]) {
        let (ch, t) = (self.ch, self.taps);
        let frames = (src.len() / ch) as i64;
        let p = f as u64 * PHASES as u64;
        let (r, w) = ((p / self.to as u64) as usize, (p % self.to as u64) as f32 / self.to as f32);
        let (a, b) = (&self.table[r * t..(r + 1) * t], &self.table[(r + 1) * t..(r + 2) * t]);
        let first = i - self.back();
        for j in 0..t {
            let c = a[j] + (b[j] - a[j]) * w;
            let at = ((first + j as i64 - src_first).clamp(0, frames - 1) as usize) * ch;
            for (o, x) in out.iter_mut().zip(&src[at..at + ch]) {
                *o += c * x;
            }
        }
    }
}

/// The `(PHASES + 1) x taps` table at cutoff `fc` cycles per source frame: row `r`, tap `j` is the windowed sinc at
/// distance `d = j - (taps / 2 - 1) - r / PHASES` source frames, `2 fc sinc(2 fc d) I0(BETA sqrt(1 - (d / h)^2)) /
/// I0(BETA)`, each row scaled to a unity sum (DC passes at every phase).
fn table(taps: usize, fc: f64) -> Vec<f32> {
    use crate::math::{PI, bessel_i0, sin, sqrt};
    let h = (taps / 2) as f64;
    let norm = bessel_i0(BETA);
    let mut out = Vec::with_capacity((PHASES + 1) * taps);
    let mut row = alloc::vec![0.0f64; taps];
    for r in 0..=PHASES {
        let frac = r as f64 / PHASES as f64;
        let mut sum = 0.0;
        for (j, v) in row.iter_mut().enumerate() {
            let d = j as f64 - (h - 1.0) - frac;
            let x = 2.0 * fc * d;
            let s = if x == 0.0 { 1.0 } else { sin(PI * x) / (PI * x) };
            let q = d / h;
            *v = 2.0 * fc * s * bessel_i0(BETA * sqrt((1.0 - q * q).max(0.0))) / norm;
            sum += *v;
        }
        out.extend(row.iter().map(|v| (v / sum) as f32));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_rates_are_bit_exact() {
        let src: Vec<f32> = (0..64).map(|i| (i as f32 * 0.37).sin()).collect();
        assert_eq!(linear(&src, 0, 2, 48_000, 48_000, 0, 32), src);
        assert_eq!(linear(&src, 10, 1, 44_100, 44_100, 12, 4), src[2..6].to_vec());
    }

    #[test]
    fn doubling_the_rate_puts_midpoints_between() {
        let src = [0.0f32, 1.0, 3.0, 7.0];
        assert_eq!(linear(&src, 0, 1, 24_000, 48_000, 0, 6), [0.0, 0.5, 1.0, 2.0, 3.0, 5.0]);
    }

    #[test]
    fn forty_eight_k_to_forty_four_one_k() {
        // one second of 48 kHz yields 44 100 frames; output frame 441 sits exactly on source frame 480
        let src: Vec<f32> = (0..48_000).map(|i| i as f32).collect();
        let out = linear(&src, 0, 1, 48_000, 44_100, 0, 44_100);
        assert_eq!(out.len(), 44_100);
        assert_eq!(linear_pos(441, 48_000, 44_100), (480, 0, 44_100));
        assert_eq!(out[441], 480.0);
        // between: frame 1 is source 1 + 3900/44100
        assert!((out[1] - (1.0 + 3_900.0 / 44_100.0)).abs() < 1e-6);
    }

    // ---- NLE4B (SR222): the windowed sinc ----

    /// A sine of `hz` at `rate` Hz, amplitude 1, `n` frames (the angle reduced exactly in integers).
    fn tone(hz: u64, rate: u64, n: usize) -> Vec<f32> {
        use crate::math::{PI, sin};
        (0..n as u64).map(|i| sin(2.0 * PI * ((hz * i) % rate) as f64 / rate as f64) as f32).collect()
    }

    /// |DFT| of `x` at `hz`, `x` taken at `rate` Hz (bin-centred: `x.len()` is the rate).
    fn bin(x: &[f32], hz: u64, rate: u64) -> f64 {
        use crate::math::{PI, sincos, sqrt};
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let (s, c) = sincos(2.0 * PI * ((hz * i as u64) % rate) as f64 / rate as f64);
            re += v as f64 * c;
            im -= v as f64 * s;
        }
        sqrt(re * re + im * im)
    }

    fn db(x: f64) -> f64 {
        20.0 * crate::math::log10(x)
    }

    /// Image rejection (dB) of a `hz` tone taken 48 kHz -> 44.1 kHz by `conv`: its bin over the worse image bin
    /// (`48k -+ hz` folded at 44.1 kHz), over exactly one second of interior output (output frames 1000..45100).
    fn rejection(hz: u64, conv: impl Fn(&[f32]) -> Vec<f32>) -> f64 {
        let out = conv(&tone(hz, 48_000, 52_800));
        assert_eq!(out.len(), 44_100);
        let fold = |f: u64| if f % 44_100 > 22_050 { 44_100 - f % 44_100 } else { f % 44_100 };
        let worst = bin(&out, fold(48_000 - hz), 44_100).max(bin(&out, fold(48_000 + hz), 44_100));
        db(bin(&out, hz, 44_100) / worst)
    }

    #[test]
    fn sinc_equal_rates_are_bit_exact() {
        let src: Vec<f32> = (0..64).map(|i| (i as f32 * 0.37).sin()).collect();
        let s = Sinc::new(48_000, 48_000, 2);
        assert_eq!(s.at(&src, 0, 0, 32), src);
        assert_eq!(Sinc::new(44_100, 44_100, 1).at(&src, 10, 12, 4), src[2..6].to_vec());
        let mut st = Sinc::new(48_000, 48_000, 2);
        let mut out = alloc::vec![0.0f32; 64];
        let (took, made) = st.process(&src, &mut out);
        assert_eq!((took, made), (32, 32));
        assert_eq!(out, src);
    }

    #[test]
    fn sinc_rejects_the_images_going_down() {
        let sinc = Sinc::new(48_000, 44_100, 1);
        let r1 = rejection(1_000, |s| sinc.at(s, 0, 1_000, 44_100));
        let r10 = rejection(10_000, |s| sinc.at(s, 0, 1_000, 44_100));
        let l10 = rejection(10_000, |s| linear(s, 0, 1, 48_000, 44_100, 1_000, 44_100));
        std::println!("sinc taps={} 1k={r1:.1} dB 10k={r10:.1} dB; linear 10k={l10:.1} dB", sinc.taps());
        assert!(r1 >= 60.0 && r10 >= 60.0, "sinc {r1} {r10}");
        assert!(l10 < 40.0, "the control: linear {l10}");
    }

    #[test]
    fn sinc_keeps_15k_going_up() {
        // output frames 1000..49000 at 48 kHz: one second, bin-centred
        let src = tone(15_000, 44_100, 52_920);
        let gain = |out: Vec<f32>| db(bin(&out, 15_000, 48_000) / 24_000.0);
        let s = gain(Sinc::new(44_100, 48_000, 1).at(&src, 0, 1_000, 48_000));
        let l = gain(linear(&src, 0, 1, 44_100, 48_000, 1_000, 48_000));
        std::println!("15k up: sinc {s:.3} dB linear {l:.3} dB");
        assert!(s.abs() <= 0.5, "sinc {s}");
        assert!(l < -1.0, "the control: linear {l}");
    }

    #[test]
    fn sinc_counts_like_linear_and_holds_the_anchor() {
        let src: Vec<f32> = (0..48_000).map(|i| (i % 1_000) as f32).collect();
        let mut st = Sinc::new(48_000, 44_100, 1);
        let mut out = alloc::vec![0.0f32; 50_000];
        let (_, made) = st.process(&src, &mut out);
        let made = made + st.flush(&mut out[made..]);
        assert!((made as i64 - linear(&src, 0, 1, 48_000, 44_100, 0, 44_100).len() as i64).abs() <= 1, "{made}");
        assert_eq!(made, 44_100);
        // output frame 441 sits exactly on source frame 480: phase 0 of a ramp passes
        assert!((st_at(&src, 441) - 480.0).abs() < 1e-3, "{}", st_at(&src, 441));
    }

    fn st_at(src: &[f32], k: i64) -> f32 {
        Sinc::new(48_000, 44_100, 1).at(src, 0, k, 1)[0]
    }

    #[test]
    fn sinc_blocks_join_bit_exact() {
        let src: Vec<f32> = (0..9_001).map(|i| ((i * 7_919) % 2_003) as f32 / 1_001.0 - 1.0).collect();
        let ch = 1;
        for (from, to) in [(48_000, 44_100), (44_100, 48_000), (32_000, 48_000)] {
            let whole = {
                let mut s = Sinc::new(from, to, ch);
                let mut out = alloc::vec![0.0f32; 20_000];
                let (_, a) = s.process(&src, &mut out);
                let b = s.flush(&mut out[a..]);
                out.truncate(a + b);
                out
            };
            let mut s = Sinc::new(from, to, ch);
            let mut parts = Vec::new();
            let mut buf = [0.0f32; 333];
            for chunk in [&src[..4_321], &src[4_321..]] {
                let (_, mut n) = s.process(chunk, &mut buf);
                loop {
                    parts.extend_from_slice(&buf[..n]);
                    if n < buf.len() {
                        break;
                    }
                    n = s.process(&[], &mut buf).1;
                }
            }
            loop {
                let n = s.flush(&mut buf);
                parts.extend_from_slice(&buf[..n]);
                if n < buf.len() {
                    break;
                }
            }
            assert_eq!(parts, whole, "{from}->{to}");
            // the position-addressed form: two blocks == one
            let r = Sinc::new(from, to, ch);
            let mut two = r.at(&src, 0, 0, 3_000);
            two.extend(r.at(&src, 0, 3_000, 2_000));
            assert_eq!(two, r.at(&src, 0, 0, 5_000), "{from}->{to}");
        }
    }
}

// ---- NLE4BKERNEL (rmbp-ledger B896): the streaming form over interleaved i16, for the kernel's mixer ----
// The ONE i16 <-> f32 conversion of the kernel's converters: the mixer's source ingest (any rate -> 48 kHz) and its A2DP
// output (48 -> 44.1 kHz) call these, never `process` with their own conversion. Alloc-free: the samples cross in
// stack chunks of `I16_CHUNK` samples (so at most `I16_CHUNK` channels); an output rounds half away from zero and
// saturates to i16 (never wraps).

/// Samples per stack chunk of [`Sinc::process_i16`] / [`Sinc::flush_i16`] (also their channel limit).
pub const I16_CHUNK: usize = 64;

impl Sinc {
    /// [`Sinc::process`] over interleaved i16: all of `input`'s whole frames are taken, and as many ready output frames
    /// as fit are written into `output` (`(source frames consumed, output frames produced)`); outputs that did not fit
    /// come on the next call (`input` may then be empty). Bit-for-bit the f32 form's frames, rounded.
    pub fn process_i16(&mut self, input: &[i16], output: &mut [i16]) -> (usize, usize) {
        let ch = self.ch;
        let step = (I16_CHUNK / ch).max(1);
        let (frames, room_all) = (input.len() / ch, output.len() / ch);
        let (mut fi, mut fo) = ([0.0f32; I16_CHUNK], [0.0f32; I16_CHUNK]);
        let (mut took, mut made) = (0usize, 0usize);
        loop {
            let n = (frames - took).min(step);
            for (d, s) in fi.iter_mut().zip(&input[took * ch..(took + n) * ch]) {
                *d = *s as f32;
            }
            let room = (room_all - made).min(step);
            let (t, m) = self.process(&fi[..n * ch], &mut fo[..room * ch]);
            for (d, s) in output[made * ch..(made + m) * ch].iter_mut().zip(&fo[..m * ch]) {
                *d = round_i16(*s);
            }
            took += t;
            made += m;
            if took == frames && m < room.max(1) {
                break;
            }
        }
        (took, made)
    }

    /// [`Sinc::flush`] over interleaved i16: the stream's tail (the lookahead read as silence) into `output`; call again
    /// while it returns a full `output`.
    pub fn flush_i16(&mut self, output: &mut [i16]) -> usize {
        let ch = self.ch;
        let step = (I16_CHUNK / ch).max(1);
        let room_all = output.len() / ch;
        let mut fo = [0.0f32; I16_CHUNK];
        let mut made = 0usize;
        while made < room_all {
            let room = (room_all - made).min(step);
            let m = self.flush(&mut fo[..room * ch]);
            for (d, s) in output[made * ch..(made + m) * ch].iter_mut().zip(&fo[..m * ch]) {
                *d = round_i16(*s);
            }
            made += m;
            if m < room {
                break;
            }
        }
        made
    }
}

/// An f32 sample (i16 scale) to i16: rounded half away from zero, saturated (`as` saturates; NaN is 0).
fn round_i16(v: f32) -> i16 {
    (if v >= 0.0 { v + 0.5 } else { v - 0.5 }) as i16
}

#[cfg(test)]
mod kernel_i16 {
    use super::*;

    /// A deterministic i16 signal (a scrambled ramp at about half scale).
    fn noise(n: usize, ch: usize) -> Vec<i16> {
        (0..n * ch).map(|i| (((i as i64 * 7_919) % 32_749) - 16_374) as i16).collect()
    }

    /// The whole-stream f32 form, rounded: the reference the i16 form must equal.
    fn whole_f32(from: u32, to: u32, ch: usize, src: &[i16]) -> Vec<i16> {
        let mut s = Sinc::new(from, to, ch);
        let fin: Vec<f32> = src.iter().map(|&v| v as f32).collect();
        let mut out = alloc::vec![0.0f32; (src.len() / ch * to as usize / from as usize + 64) * ch];
        let (_, a) = s.process(&fin, &mut out);
        let b = s.flush(&mut out[a * ch..]);
        out[..(a + b) * ch].iter().map(|&v| round_i16(v)).collect()
    }

    #[test]
    fn i16_equal_rates_are_bit_exact() {
        let src = noise(1_000, 2);
        let mut s = Sinc::new(48_000, 48_000, 2);
        let mut out = alloc::vec![0i16; 2_000];
        assert_eq!(s.process_i16(&src, &mut out), (1_000, 1_000));
        assert_eq!(out, src);
    }

    #[test]
    fn i16_frame_calls_equal_the_whole_f32_stream() {
        // the mixer's ingest shape: ONE source frame per call (mono and stereo), then the flush at end of stream
        for (from, to, ch) in [(44_100u32, 48_000u32, 1usize), (44_100, 48_000, 2), (22_050, 48_000, 1), (48_000, 44_100, 2)] {
            let src = noise(3_001, ch);
            let mut s = Sinc::new(from, to, ch);
            let (mut got, mut buf) = (Vec::new(), [0i16; 16]);
            for f in src.chunks_exact(ch) {
                let (took, made) = s.process_i16(f, &mut buf[..8 * ch]);
                assert_eq!(took, 1);
                got.extend_from_slice(&buf[..made * ch]);
            }
            loop {
                let n = s.flush_i16(&mut buf[..8 * ch]);
                got.extend_from_slice(&buf[..n * ch]);
                if n < 8 {
                    break;
                }
            }
            assert_eq!(got, whole_f32(from, to, ch, &src), "{from}->{to} ch={ch}");
        }
    }

    #[test]
    fn i16_saturates_never_wraps() {
        // a full-scale square overshoots (Gibbs) through the sinc: the i16 form clamps where the f32 form exceeds i16
        let src: Vec<i16> = (0..4_410).map(|i| if (i / 50) % 2 == 0 { i16::MAX } else { i16::MIN }).collect();
        let reference = {
            let mut s = Sinc::new(44_100, 48_000, 1);
            let fin: Vec<f32> = src.iter().map(|&v| v as f32).collect();
            let mut out = alloc::vec![0.0f32; 5_000];
            let (_, a) = s.process(&fin, &mut out);
            out.truncate(a);
            out
        };
        assert!(reference.iter().any(|&v| v > 32_767.0) && reference.iter().any(|&v| v < -32_768.0), "the control: no overshoot");
        let mut s = Sinc::new(44_100, 48_000, 1);
        let mut out = alloc::vec![0i16; 5_000];
        let (_, made) = s.process_i16(&src, &mut out);
        assert_eq!(made, reference.len());
        for (o, r) in out[..made].iter().zip(&reference) {
            if *r > 32_767.0 {
                assert_eq!(*o, i16::MAX);
            } else if *r < -32_768.0 {
                assert_eq!(*o, i16::MIN);
            } else {
                assert!((*o as f32 - r).abs() <= 0.5, "{o} vs {r}");
            }
        }
    }

    #[test]
    fn i16_bt_periods_count() {
        // the mixer's A2DP output: 18 periods of 480 stereo frames at 48 kHz, then the flush at release = 7938 at 44.1 kHz
        let src = noise(8_640, 2);
        let mut s = Sinc::new(48_000, 44_100, 2);
        let (mut made, mut buf) = (0usize, [0i16; 2 * 512]);
        for p in src.chunks(480 * 2) {
            let (took, m) = s.process_i16(p, &mut buf);
            assert_eq!((took, m < 512), (480, true));
            made += m;
        }
        loop {
            let n = s.flush_i16(&mut buf);
            made += n;
            if n < 512 {
                break;
            }
        }
        assert_eq!(made, 7_938);
    }
}
