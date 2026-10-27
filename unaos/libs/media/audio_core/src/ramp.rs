// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core. THE ONE GAIN CURVE (NLE6MIXAUDIO, rmbp-ledger B953): the in-run interpolation of a
//! fade or crossfade envelope and the clip gain, called by Stria's render (`handlers/stria/src/nle.rs` `mix_into`, Q16
//! envelopes) and by the kernel mixer's per-frame gain (`unaos/crates/kernel/src/audio/mixer.rs` `gain_at`, Q8 gains
//! fed by the Player's EDL mode, `unaos/crates/kernel/src/video/vplay.rs`). The edge values are timeline_core's
//! (`schedule::AudioRun` layers' `env`, NLE3b / NLE4); between two edges the curve is LINEAR (equal-power is owed).

/// The envelope at frame `j` of a run of `n` frames that starts at `e0` and ends (at its end edge) at `e1`: linear,
/// integer, truncating toward zero (Stria's arithmetic since NLE4, moved here). `n == 0` holds `e0`.
#[inline]
pub fn at(e0: i64, e1: i64, j: i64, n: i64) -> i64 {
    if n > 0 { e0 + (e1 - e0) * j / n } else { e0 }
}

/// A clip gain in centi-dB as a Q16 factor (0 cdB = 65536 exactly), rounded: `round(10^(cdb / 2000) * 65536)`.
pub fn gain_q16(cdb: i32) -> i64 {
    if cdb == 0 {
        return 65536;
    }
    crate::math::round(crate::math::pow(10.0, cdb as f64 / 2000.0) * 65536.0) as i64
}

/// A layer's mixer gain (Q8, 256 = unity) at one envelope edge: the clip gain (Q16) x the envelope (Q16), clamped.
pub fn layer_q8(gain_cdb: i32, env: u32) -> u16 {
    ((gain_q16(gain_cdb) * env as i64) >> 24).clamp(0, u16::MAX as i64) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_is_linear_and_truncates() {
        assert_eq!(at(0, 65536, 0, 4), 0);
        assert_eq!(at(0, 65536, 2, 4), 32768);
        assert_eq!(at(256, 0, 1, 3), 171); // 256 + (-256 / 3) = 256 - 85
        assert_eq!(at(7, 9, 5, 0), 7);
    }

    #[test]
    fn gain_q16_matches_std() {
        for cdb in [-9600, -2400, -600, -1, 1, 600, 1200] {
            let want = (10f64.powf(cdb as f64 / 2000.0) * 65536.0).round() as i64;
            assert_eq!(gain_q16(cdb), want, "cdb {cdb}");
        }
        assert_eq!(gain_q16(0), 65536);
        assert_eq!(layer_q8(0, 65536), 256);
        assert_eq!(layer_q8(0, 0), 0);
    }

    /// The kernel `tests nle` fixture's audio dissolve (player.rs `nle_fx::ENV_KAT`): reel B (a +-8000 square, period
    /// 160) rising over reel A at its handle (period 48, from source tick 23), a 4-tick dissolve at tick 10, 25 fps,
    /// 48 kHz mono; each source x its Q8 ramp (`at`), `>> 8` and summed as the mixer's `mix_add`, the master unity
    /// (HDA). The RMS of mixer periods 41, 43 .. 55 counted from the first audible one (tick 10 = period 40).
    #[test]
    fn kernel_fixture_envelope() {
        const ENV_KAT: [u32; 8] = [7304, 6503, 5936, 5672, 5754, 6168, 6854, 7741];
        let sq = |i: i64, half: i64| if (i / half) % 2 == 0 { 8000i64 } else { -8000 };
        let (tick, cut, n) = (1920i64, 19_200i64, 4 * 1920i64);
        let mut got = [0u32; 8];
        for (q, p) in (41..56).step_by(2).enumerate() {
            let mut s = 0i64;
            for f in 480 * p..480 * p + 480 {
                let k = f - cut;
                let (top, und) = (sq(k, 80), sq(23 * tick + k, 24));
                let acc = ((top * at(0, 256, k, n)) >> 8).clamp(-32768, 32767);
                let acc = (acc + ((und * at(256, 0, k, n)) >> 8)).clamp(-32768, 32767);
                s += 2 * acc * acc;
            }
            got[q] = ((s / 960) as u64).isqrt() as u32;
        }
        assert_eq!(got, ENV_KAT);
    }
}
