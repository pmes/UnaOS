// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! STRETCH (AETHERMEDIA2, LEDGER SR134) — time-stretch with the pitch kept, by WSOLA (waveform
//! similarity overlap-add; Verhelst & Roelands, ICASSP 1993), for HTMLMediaElement.playbackRate.
//!
//! Output is built from Hann-windowed frames of `N` input frames (≈ 20 ms) overlap-added at a
//! fixed synthesis hop `N/2` (a periodic Hann at 50 % overlap sums to exactly 1). The analysis
//! position advances `rate × N/2` per output hop; around it, within ±`S` (≈ 8 ms), the segment is
//! chosen whose first half best matches the NATURAL continuation of the previous segment (the
//! input right after it), by cross-correlation of the channel sum — so every overlap joins two
//! stretches of signal that were in phase, and no pitch moves. The search is coarse (step 4 over
//! the window, every second sample) and then refined (±3 around the best, every sample).
//!
//! `no_std` + `alloc`; interleaved `f32` in and out at the same rate and channel count.

use alloc::vec::Vec;

/// The rates WSOLA serves (HTML leaves the range to the UA; this is Chromium's audible range).
pub const MIN_RATE: f64 = 0.25;
pub const MAX_RATE: f64 = 4.0;

pub struct Wsola {
    ch: usize,
    /// Frame length and synthesis hop, in frames.
    n: usize,
    hs: usize,
    /// Search half-width, in frames.
    search: usize,
    rate: f64,
    win: Vec<f32>,
    /// Interleaved input; `input[0]` is absolute frame `base`.
    input: Vec<f32>,
    base: u64,
    /// Nominal analysis position of the next frame (absolute frames).
    pos: f64,
    /// Absolute start of the previous chosen segment.
    prev: Option<u64>,
    /// The overlap-add accumulator, `n` frames.
    ola: Vec<f32>,
}

impl Wsola {
    pub fn new(sample_rate: u32, channels: u16) -> Wsola {
        let sr = sample_rate.max(8000) as usize;
        let n = ((sr / 50) & !1).max(64); // ≈ 20 ms, even
        let hs = n / 2;
        let search = (sr / 125).max(8); // ≈ 8 ms
        let mut win = Vec::with_capacity(n);
        for i in 0..n {
            // periodic Hann: sums to 1 at hop n/2
            let x = 2.0 * core::f64::consts::PI * i as f64 / n as f64;
            win.push((0.5 - 0.5 * crate::math::cos(x)) as f32);
        }
        let ch = channels.max(1) as usize;
        Wsola { ch, n, hs, search, rate: 1.0, win, input: Vec::new(), base: 0, pos: 0.0, prev: None, ola: alloc::vec![0.0; n * ch] }
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Set the rate (clamped to [`MIN_RATE`]..=[`MAX_RATE`]); takes effect at the next hop.
    pub fn set_rate(&mut self, rate: f64) {
        self.rate = if rate.is_finite() { rate.clamp(MIN_RATE, MAX_RATE) } else { 1.0 };
    }

    /// Drop all state (a seek): the next input starts a fresh stream.
    pub fn reset(&mut self) {
        self.input.clear();
        self.base = 0;
        self.pos = 0.0;
        self.prev = None;
        for x in self.ola.iter_mut() {
            *x = 0.0;
        }
    }

    /// Input frames held.
    pub fn held(&self) -> usize {
        self.input.len() / self.ch
    }

    /// Queue interleaved input.
    pub fn push(&mut self, input: &[f32]) {
        let whole = input.len() - input.len() % self.ch;
        self.input.extend_from_slice(&input[..whole]);
    }

    /// Mono sample (channel sum) at absolute frame `f`.
    fn mono(&self, f: u64) -> f32 {
        let i = (f - self.base) as usize * self.ch;
        self.input[i..i + self.ch].iter().sum()
    }

    fn corr(&self, a: u64, b: u64, len: usize, step: usize) -> f32 {
        let mut s = 0.0f32;
        let mut k = 0;
        while k < len {
            s += self.mono(a + k as u64) * self.mono(b + k as u64);
            k += step;
        }
        s
    }

    /// Produce every output hop the queued input allows; returns the frames appended to `out`.
    pub fn pull(&mut self, out: &mut Vec<f32>) -> usize {
        let mut made = 0;
        loop {
            let end = self.base + self.held() as u64;
            let nominal = libm_round(self.pos).max(0.0) as u64;
            let chosen = match self.prev {
                None => {
                    if nominal + self.n as u64 > end {
                        break;
                    }
                    nominal.max(self.base)
                }
                Some(prev) => {
                    let natural = prev + self.hs as u64;
                    let lo = nominal.saturating_sub(self.search as u64).max(self.base);
                    let hi = nominal + self.search as u64;
                    if hi + self.n as u64 > end || natural + self.hs as u64 > end {
                        break;
                    }
                    let mut best = (lo, f32::MIN);
                    let mut q = lo;
                    while q <= hi {
                        let c = self.corr(q, natural, self.hs, 2);
                        if c > best.1 {
                            best = (q, c);
                        }
                        q += 4;
                    }
                    let (c0, _) = best;
                    let mut fine = (c0, f32::MIN);
                    for q in c0.saturating_sub(3).max(lo)..=(c0 + 3).min(hi) {
                        let c = self.corr(q, natural, self.hs, 1);
                        if c > fine.1 {
                            fine = (q, c);
                        }
                    }
                    fine.0
                }
            };
            // overlap-add the windowed segment, emit one hop
            let off = (chosen - self.base) as usize * self.ch;
            for i in 0..self.n {
                let w = self.win[i];
                for c in 0..self.ch {
                    self.ola[i * self.ch + c] += self.input[off + i * self.ch + c] * w;
                }
            }
            out.extend_from_slice(&self.ola[..self.hs * self.ch]);
            self.ola.drain(..self.hs * self.ch);
            self.ola.resize(self.n * self.ch, 0.0);
            made += self.hs;
            self.prev = Some(chosen);
            self.pos += self.hs as f64 * self.rate;
            // keep what a later search or continuation can still reach
            let keep = (libm_round(self.pos).max(0.0) as u64).saturating_sub(self.search as u64 + 4).min(chosen + self.hs as u64);
            if keep > self.base {
                let drop = ((keep - self.base) as usize).min(self.held());
                self.input.drain(..drop * self.ch);
                self.base += drop as u64;
            }
        }
        made
    }
}

fn libm_round(x: f64) -> f64 {
    crate::math::round(x)
}
