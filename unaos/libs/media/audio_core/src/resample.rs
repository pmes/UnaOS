// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RESAMPLE (NLE4, LEDGER SR203) — a LINEAR sample-rate converter, position-addressed, for Stria's offline editor
//! render (`stria::nle`), which mixes sources of any rate at the project's.
//!
//! CHARTER: Stria — shared-core (BP-handlers-4): the converter both rings can link; no I/O, `no_std` + `alloc`.
//!
//! LINEAR interpolation ONLY: output frame `k` at `to` Hz is the source at `k * from / to`, interpolated between its
//! two neighbouring source frames. It is cheap and exact in phase (an integer phase in units of `1 / to` of a source
//! frame, so equal rates are bit-exact and nothing drifts), but it is not a quality path: it aliases on the way down
//! and dulls the top octave on the way up. The windowed-sinc converter that replaces it in the render is owed as
//! NLE4b; do not take this for it. Interleaved `f32`, any channel count.

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
}
