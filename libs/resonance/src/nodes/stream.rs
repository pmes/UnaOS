// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! `StreamSource` — a graph node that plays a stream of decoded PCM pushed from outside the audio
//! thread (PLAYBACK, LEDGER SR26: Stria's media audio rides resonance).
//!
//! The producer side ([`StreamFeed`]) lives with the player: it pushes mono `f32` samples at the
//! *source* rate into a lock-free ring. The node, on the audio thread, pulls them and converts to
//! the graph rate by linear interpolation, and counts every source sample it consumed in an
//! atomic — the number the player's audio master clock is built on ("media time = what the
//! device actually took", not what was queued). An empty ring plays silence and does not count
//! (a starved device stalls the clock instead of letting video run ahead).

use crate::core::{AudioNode, GraphContext};
use crate::{BLOCK_SIZE, Sample};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The audio-thread side.
pub struct StreamSource {
    rx: HeapCons<f32>,
    src_rate: u64,
    /// Read position between `prev` (0) and `cur` (`out_rate`), in units of 1/`out_rate` of a
    /// source sample: exact integer phase, so equal rates are bit-exact and no drift accumulates.
    pos: u64,
    prev: f32,
    cur: f32,
    consumed: Arc<AtomicU64>,
    primed: bool,
}

/// The player side: push samples, read how many the device has taken.
pub struct StreamFeed {
    tx: HeapProd<f32>,
    consumed: Arc<AtomicU64>,
    pub src_rate: u32,
}

/// A connected pair with room for `capacity` samples.
pub fn stream_pair(src_rate: u32, capacity: usize) -> (StreamSource, StreamFeed) {
    let (tx, rx) = HeapRb::<f32>::new(capacity.max(1)).split();
    let consumed = Arc::new(AtomicU64::new(0));
    (
        StreamSource { rx, src_rate: src_rate.max(1) as u64, pos: 0, prev: 0.0, cur: 0.0, consumed: consumed.clone(), primed: false },
        StreamFeed { tx, consumed, src_rate },
    )
}

impl StreamFeed {
    /// Queue as many of `samples` as fit; returns how many were taken.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        self.tx.push_slice(samples)
    }
    /// Free room in the ring.
    pub fn room(&self) -> usize {
        self.tx.vacant_len()
    }
    /// Source samples the audio thread has consumed since the pair was made.
    pub fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Acquire)
    }
}

impl StreamSource {
    fn pull(&mut self) -> bool {
        match self.rx.try_pop() {
            Some(s) => {
                self.prev = self.cur;
                self.cur = s;
                self.consumed.fetch_add(1, Ordering::AcqRel);
                true
            }
            None => false,
        }
    }
}

impl AudioNode for StreamSource {
    fn process(&mut self, _inputs: &[&[Sample; BLOCK_SIZE]], outputs: &mut [&mut [Sample; BLOCK_SIZE]], context: &GraphContext) {
        if outputs.is_empty() {
            return;
        }
        let out_rate = (context.sample_rate.round() as u64).max(1);
        let out = &mut outputs[0];
        for o in out.iter_mut() {
            if !self.primed {
                // The first sample is played as is (no ramp from an invented zero).
                if self.pull() {
                    self.prev = self.cur;
                    self.primed = true;
                    self.pos = out_rate;
                } else {
                    *o = 0.0;
                    continue;
                }
            } else {
                self.pos += self.src_rate;
                let mut starved = false;
                while self.pos > out_rate {
                    if self.pull() {
                        self.pos -= out_rate;
                    } else {
                        starved = true;
                        break;
                    }
                }
                if starved {
                    // Output silence until data returns; the next sample re-primes.
                    self.primed = false;
                    *o = 0.0;
                    continue;
                }
            }
            let frac = self.pos as f64 / out_rate as f64;
            *o = (self.prev as f64 + (self.cur as f64 - self.prev as f64) * frac) as Sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(rate: f64) -> GraphContext {
        GraphContext { sample_rate: rate, inv_sample_rate: 1.0 / rate }
    }
    fn block(node: &mut StreamSource, rate: f64) -> [Sample; BLOCK_SIZE] {
        let mut buf = [0.0; BLOCK_SIZE];
        {
            let mut outs = [&mut buf];
            node.process(&[], &mut outs, &ctx(rate));
        }
        buf
    }

    #[test]
    fn same_rate_is_bit_exact_and_counted() {
        let (mut src, mut feed) = stream_pair(48_000, 1024);
        let ramp: Vec<f32> = (0..256).map(|i| i as f32 / 256.0).collect();
        assert_eq!(feed.push(&ramp), 256);
        let mut got = Vec::new();
        for _ in 0..4 {
            got.extend(block(&mut src, 48_000.0));
        }
        assert_eq!(got.iter().map(|&x| x as f32).collect::<Vec<_>>(), ramp);
        assert_eq!(feed.consumed(), 256);
        // Starved: silence, and the counter does not move.
        assert!(block(&mut src, 48_000.0).iter().all(|&x| x == 0.0));
        assert_eq!(feed.consumed(), 256);
    }

    #[test]
    fn upsampling_interpolates_and_counts_source_samples() {
        let (mut src, mut feed) = stream_pair(24_000, 1024);
        let ramp: Vec<f32> = (0..200).map(|i| i as f32).collect();
        feed.push(&ramp);
        let b = block(&mut src, 48_000.0);
        // Output n sits at source position n/2.
        for (n, &x) in b.iter().enumerate() {
            assert!((x - n as f64 / 2.0).abs() < 1e-9, "n={n} x={x}");
        }
        assert_eq!(feed.consumed(), 33, "positions 0..31.5 need source samples 0..=32");
    }
}
