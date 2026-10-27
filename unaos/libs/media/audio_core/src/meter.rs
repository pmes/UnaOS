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
