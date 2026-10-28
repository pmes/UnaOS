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
//!   Its i16 face ([`Sinc::process_i16`], RSFIXED SR317) runs the same stream in integers: a Q30 copy of the table and
//!   an i64 multiply-accumulate, no f32 in the loop (the kernel's soft-float target).
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
    /// RSFIXED (SR317): the same rows in Q30 (`1 << 30` = 1.0), read by the i16 form's integer kernel.
    fixed: Vec<i32>,
    /// The streaming form's state: source frames from `hist_at` on (`hist` for the f32 form, `hist16` for the i16
    /// form; a stream is fed by ONE form), the next output frame, the frames fed, and (after `flush`) the real length.
    hist: Vec<f32>,
    hist16: Vec<i16>,
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
        let (taps, (table, fixed)) = if rf == rt {
            (1, (alloc::vec![1.0f32; PHASES + 1], alloc::vec![1i32 << 30; PHASES + 1]))
        } else {
            let mut t = TAPS;
            if rf > rt {
                t = (TAPS as u64 * rf as u64).div_ceil(rt as u64) as usize;
                t = (t + (t & 1)).min(MAX_TAPS);
            }
            (t, table(t, CUTOFF * from.min(to) as f64 / from as f64))
        };
        let mut s = Sinc { rates: (from, to), from: rf, to: rt, ch, taps, table, fixed, hist: Vec::new(), hist16: Vec::new(), hist_at: 0, next: 0, fed: 0, end: None };
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
        self.hist16.clear();
        self.hist16.resize(b as usize * self.ch, 0);
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
        self.feed(input, output)
    }
    /// The ONE streaming feed, either form (RSFIXED SR317: generic over [`Form`]).
    fn feed<T: Form>(&mut self, input: &[T], output: &mut [T]) -> (usize, usize) {
        let (ch, frames) = (self.ch, input.len() / self.ch);
        T::hist(self).extend_from_slice(&input[..frames * ch]);
        self.fed += frames as i64;
        (frames, self.produce(output))
    }
    /// End of stream: the window's lookahead is silence, and the outputs run to `ceil(fed * to / from)` frames in all
    /// (the count [`linear`] gives over the same span). Call again while it returns a full `output`.
    pub fn flush(&mut self, output: &mut [f32]) -> usize {
        self.end(output)
    }
    /// The ONE end of stream, either form.
    fn end<T: Form>(&mut self, output: &mut [T]) -> usize {
        if self.end.is_none() {
            self.end = Some(self.fed);
            let pad = self.delay().max(1) * self.ch;
            let h = T::hist(self);
            let n = h.len() + pad;
            h.resize(n, T::ZERO);
            self.fed += self.delay().max(1) as i64;
        }
        self.produce(output)
    }
    fn produce<T: Form>(&mut self, output: &mut [T]) -> usize {
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
            o.fill(T::ZERO);
            T::frame(self, T::hist_ref(self), self.hist_at, i, f, o);
            self.next += 1;
            made += 1;
        }
        let (keep, _, _) = linear_pos(self.next, self.from, self.to);
        let drop = ((keep - self.back() - self.hist_at).max(0) as usize * ch).min(T::hist_ref(self).len());
        T::hist(self).drain(..drop);
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
/// I0(BETA)`, each row scaled to a unity sum (DC passes at every phase). Returned twice from the same f64 rows: as f32
/// and (RSFIXED SR317) as Q30 i32, rounded half away from zero.
fn table(taps: usize, fc: f64) -> (Vec<f32>, Vec<i32>) {
    use crate::math::{PI, bessel_i0, sin, sqrt};
    let h = (taps / 2) as f64;
    let norm = bessel_i0(BETA);
    let mut out = Vec::with_capacity((PHASES + 1) * taps);
    let mut q30 = Vec::with_capacity((PHASES + 1) * taps);
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
        q30.extend(row.iter().map(|v| {
            let q = v / sum * (1u64 << 30) as f64;
            (if q >= 0.0 { q + 0.5 } else { q - 0.5 }) as i32
        }));
    }
    (out, q30)
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
    pub(super) fn tone(hz: u64, rate: u64, n: usize) -> Vec<f32> {
        use crate::math::{PI, sin};
        (0..n as u64).map(|i| sin(2.0 * PI * ((hz * i) % rate) as f64 / rate as f64) as f32).collect()
    }

    /// |DFT| of `x` at `hz`, `x` taken at `rate` Hz (bin-centred: `x.len()` is the rate).
    pub(super) fn bin(x: &[f32], hz: u64, rate: u64) -> f64 {
        use crate::math::{PI, sincos, sqrt};
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let (s, c) = sincos(2.0 * PI * ((hz * i as u64) % rate) as f64 / rate as f64);
            re += v as f64 * c;
            im -= v as f64 * s;
        }
        sqrt(re * re + im * im)
    }

    pub(super) fn db(x: f64) -> f64 {
        20.0 * crate::math::log10(x)
    }

    /// Image rejection (dB) of a `hz` tone taken 48 kHz -> 44.1 kHz by `conv`: its bin over the worse image bin
    /// (`48k -+ hz` folded at 44.1 kHz), over exactly one second of interior output (output frames 1000..45100).
    pub(super) fn rejection(hz: u64, conv: impl Fn(&[f32]) -> Vec<f32>) -> f64 {
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
// The ONE i16 face of the kernel's converters: the mixer's source ingest (any rate -> 48 kHz) and its A2DP output
// (48 -> 44.1 kHz) call these. RSFIXED (SR317): they run the stream machine on an i16 history through `frame_q`, the
// Q30 integer kernel (no f32 anywhere, no chunking, any channel count); an output rounds half away from zero and
// saturates to i16 (never wraps), within 1 LSB of the f32 form's rounded frames.

impl Sinc {
    /// [`Sinc::process`] over interleaved i16: all of `input`'s whole frames are taken, and as many ready output frames
    /// as fit are written into `output` (`(source frames consumed, output frames produced)`); outputs that did not fit
    /// come on the next call (`input` may then be empty). Integer arithmetic; within 1 LSB of the f32 form, rounded.
    pub fn process_i16(&mut self, input: &[i16], output: &mut [i16]) -> (usize, usize) {
        self.feed(input, output)
    }

    /// [`Sinc::flush`] over interleaved i16: the stream's tail (the lookahead read as silence) into `output`; call again
    /// while it returns a full `output`.
    pub fn flush_i16(&mut self, output: &mut [i16]) -> usize {
        self.end(output)
    }
}

/// An f32 sample (i16 scale) to i16: rounded half away from zero, saturated (`as` saturates; NaN is 0). The KATs'
/// reference rounding of the f32 form.
#[cfg(test)]
fn round_i16(v: f32) -> i16 {
    (if v >= 0.0 { v + 0.5 } else { v - 0.5 }) as i16
}

// ---- RSFIXED (LEDGER SR317): the sample forms of the ONE stream machine, and the Q30 integer kernel ----

/// A sample form the stream machine (`feed` / `end` / `produce`) runs on: its silence, its history in the [`Sinc`],
/// and its kernel (`f32` -> [`Sinc::frame_at`], `i16` -> [`Sinc::frame_q`]).
trait Form: Copy {
    const ZERO: Self;
    fn hist(s: &mut Sinc) -> &mut Vec<Self>;
    fn hist_ref(s: &Sinc) -> &Vec<Self>;
    fn frame(s: &Sinc, src: &[Self], src_first: i64, i: i64, f: u32, out: &mut [Self]);
}

impl Form for f32 {
    const ZERO: f32 = 0.0;
    fn hist(s: &mut Sinc) -> &mut Vec<f32> {
        &mut s.hist
    }
    fn hist_ref(s: &Sinc) -> &Vec<f32> {
        &s.hist
    }
    fn frame(s: &Sinc, src: &[f32], src_first: i64, i: i64, f: u32, out: &mut [f32]) {
        s.frame_at(src, src_first, i, f, out)
    }
}

impl Form for i16 {
    const ZERO: i16 = 0;
    fn hist(s: &mut Sinc) -> &mut Vec<i16> {
        &mut s.hist16
    }
    fn hist_ref(s: &Sinc) -> &Vec<i16> {
        &s.hist16
    }
    fn frame(s: &Sinc, src: &[i16], src_first: i64, i: i64, f: u32, out: &mut [i16]) {
        s.frame_q(src, src_first, i, f, out)
    }
}

impl Sinc {
    /// [`Sinc::frame_at`] in integers: the same row pair, the weight between them in Q16, each tap's coefficient
    /// interpolated in Q30 once per frame, then per channel an i64 sum of sample x coefficient, rounded half away from
    /// zero at 2^30 and clamped to i16. (|coef| < 2 and `MAX_TAPS` taps: the sum stays under 2^55.)
    fn frame_q(&self, src: &[i16], src_first: i64, i: i64, f: u32, out: &mut [i16]) {
        let (ch, t) = (self.ch, self.taps);
        let frames = (src.len() / ch) as i64;
        let p = f as u64 * PHASES as u64;
        let (r, w) = ((p / self.to as u64) as usize, (((p % self.to as u64) << 16) / self.to as u64) as i64);
        let (a, b) = (&self.fixed[r * t..(r + 1) * t], &self.fixed[(r + 1) * t..(r + 2) * t]);
        let mut coef = [0i32; MAX_TAPS];
        for (c, (x, y)) in coef.iter_mut().zip(a.iter().zip(b)) {
            *c = *x + (((*y as i64 - *x as i64) * w) >> 16) as i32;
        }
        let first = i - self.back() - src_first;
        for (c, o) in out.iter_mut().enumerate() {
            let mut acc = 0i64;
            for (j, k) in coef[..t].iter().enumerate() {
                let at = (first + j as i64).clamp(0, frames - 1) as usize * ch + c;
                acc += *k as i64 * src[at] as i64;
            }
            let half = 1i64 << 29;
            let v = if acc >= 0 { (acc + half) >> 30 } else { -((half - acc) >> 30) };
            *o = v.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        }
    }
}

#[cfg(test)]
mod kernel_i16 {
    use super::*;

    /// A deterministic i16 signal (a scrambled ramp at about half scale).
    fn noise(n: usize, ch: usize) -> Vec<i16> {
        (0..n * ch).map(|i| (((i as i64 * 7_919) % 32_749) - 16_374) as i16).collect()
    }

    /// The whole-stream f32 form, rounded: the reference the i16 form stays within 1 LSB of (RSFIXED SR317).
    fn whole_f32(from: u32, to: u32, ch: usize, src: &[i16]) -> Vec<i16> {
        let mut s = Sinc::new(from, to, ch);
        let fin: Vec<f32> = src.iter().map(|&v| v as f32).collect();
        let mut out = alloc::vec![0.0f32; (src.len() / ch * to as usize / from as usize + 64) * ch];
        let (_, a) = s.process(&fin, &mut out);
        let b = s.flush(&mut out[a * ch..]);
        out[..(a + b) * ch].iter().map(|&v| round_i16(v)).collect()
    }

    /// The whole stream through the i16 (Q30 integer) form in one call, then its flush.
    fn whole_i16(from: u32, to: u32, ch: usize, src: &[i16]) -> Vec<i16> {
        let mut s = Sinc::new(from, to, ch);
        let mut out = alloc::vec![0i16; (src.len() / ch * to as usize / from as usize + 64) * ch];
        let (_, a) = s.process_i16(src, &mut out);
        let b = s.flush_i16(&mut out[a * ch..]);
        out.truncate((a + b) * ch);
        out
    }

    /// The largest |a - b| over two equal-length streams, and how many samples differ.
    fn diff(a: &[i16], b: &[i16]) -> (i32, usize) {
        assert_eq!(a.len(), b.len());
        a.iter().zip(b).fold((0, 0), |(m, n), (x, y)| {
            let d = (*x as i32 - *y as i32).abs();
            (m.max(d), n + (d != 0) as usize)
        })
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
            assert_eq!(got, whole_i16(from, to, ch, &src), "{from}->{to} ch={ch}");
            let (worst, _) = diff(&got, &whole_f32(from, to, ch, &src));
            assert!(worst <= 1, "{from}->{to} ch={ch}: {worst} LSB from the f32 form");
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
                assert!((*o as i32 - round_i16(*r) as i32).abs() <= 1, "{o} vs {r}");
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

    // ---- RSFIXED (SR317): the Q30 integer form against the f32 form ----

    use super::tests::{bin, db, rejection, tone};

    /// A `tone` at i16 scale (amplitude 32000), interleaved over `ch` equal channels.
    fn tone_i16(hz: u64, rate: u64, n: usize, ch: usize) -> Vec<i16> {
        tone(hz, rate, n).iter().flat_map(|v| core::iter::repeat(round_i16(v * 32_000.0)).take(ch)).collect()
    }

    #[test]
    fn q_sines_within_one_lsb() {
        for (from, to) in [(48_000u32, 44_100u32), (44_100, 48_000)] {
            for hz in [1_000u64, 10_000] {
                let src = tone_i16(hz, from as u64, 9_600, 2);
                let (worst, n) = diff(&whole_i16(from, to, 2, &src), &whole_f32(from, to, 2, &src));
                std::println!("q sines {from}->{to} {hz} Hz: max_diff={worst} differing={n}");
                assert!(worst <= 1, "{from}->{to} {hz} Hz: {worst}");
            }
        }
    }

    /// The interior frames `first..first + n` of a Q30 stream over `src` (amplitude 1), back at amplitude 1.
    fn q_stream(from: u32, to: u32, src: &[f32], first: usize, n: usize) -> Vec<f32> {
        let s: Vec<i16> = src.iter().map(|v| round_i16(v * 32_000.0)).collect();
        whole_i16(from, to, 1, &s)[first..first + n].iter().map(|&v| v as f32 / 32_000.0).collect()
    }

    #[test]
    fn q_rejects_images_going_down() {
        let r1 = rejection(1_000, |s| q_stream(48_000, 44_100, s, 1_000, 44_100));
        let r10 = rejection(10_000, |s| q_stream(48_000, 44_100, s, 1_000, 44_100));
        std::println!("q 1k={r1:.1} dB 10k={r10:.1} dB");
        assert!(r1 >= 60.0 && r10 >= 60.0, "q30 {r1} {r10}");
    }

    #[test]
    fn q_keeps_15k_going_up() {
        let out = q_stream(44_100, 48_000, &tone(15_000, 44_100, 52_920), 1_000, 48_000);
        let g = db(bin(&out, 15_000, 48_000) / 24_000.0);
        std::println!("q 15k up {g:.3} dB");
        assert!(g.abs() <= 0.5, "q30 {g}");
    }

    #[test]
    fn q_cost_per_frame() {
        // the host figure (run with --release for a meaningful number): ns per output frame, f32 `process` vs the
        // Q30 `process_i16`, one second of stereo each way; only the counts are asserted
        for (from, to) in [(44_100u32, 48_000u32), (48_000, 44_100)] {
            let src = tone_i16(1_000, from as u64, from as usize, 2);
            let fin: Vec<f32> = src.iter().map(|&v| v as f32).collect();
            let mut fo = alloc::vec![0.0f32; (to as usize + 64) * 2];
            let mut qo = alloc::vec![0i16; (to as usize + 64) * 2];
            let mut sf = Sinc::new(from, to, 2);
            let t0 = std::time::Instant::now();
            let (_, mf) = sf.process(&fin, &mut fo);
            let tf = t0.elapsed().as_nanos() as f64;
            let mut sq = Sinc::new(from, to, 2);
            let t1 = std::time::Instant::now();
            let (_, mq) = sq.process_i16(&src, &mut qo);
            let tq = t1.elapsed().as_nanos() as f64;
            assert_eq!(mf, mq);
            assert!(mq + sq.taps() >= to as usize, "{mq}");
            std::println!("cost {from}->{to} stereo taps={}: f32 {:.1} ns/frame q30 {:.1} ns/frame", sq.taps(), tf / mf as f64, tq / mq as f64);
        }
    }
}
