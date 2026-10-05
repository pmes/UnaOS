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
//! thread (PLAYBACK, LEDGER SR26: Stria's media audio rides resonance; AUDIOTRACK, SR45: stereo,
//! flush and pause).
//!
//! The producer side ([`StreamFeed`]) lives with the player: it pushes interleaved `f32` frames
//! (mono, or stereo L/R) at the *source* rate into a lock-free ring. The node, on the audio
//! thread, pulls them and converts to the graph rate by linear interpolation (per channel, one
//! shared integer phase), and counts every source FRAME it consumed in an atomic — the number the
//! player's audio master clock is built on ("media time = what the device actually took", not
//! what was queued). An empty ring plays silence and does not count (a starved device stalls the
//! clock instead of letting video run ahead).
//!
//! Two controls cross the thread boundary without a lock:
//! * [`StreamFeed::flush`] — everything pushed so far is discarded unplayed and uncounted (a seek:
//!   the stale ring PLAYBACK named would otherwise play, and offset the clock, after the jump);
//! * [`StreamFeed::set_paused`] — the node emits silence and takes nothing (the clock freezes
//!   with it), and resumes from the same sample.

use crate::core::{AudioNode, GraphContext};
use crate::{BLOCK_SIZE, Sample};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What both sides share.
struct Shared {
    /// Source frames the audio thread has played.
    consumed: AtomicU64,
    /// Discard every sample with a push index below this (set by `flush`).
    discard_until: AtomicU64,
    paused: AtomicBool,
}

/// The audio-thread side.
pub struct StreamSource {
    rx: HeapCons<f32>,
    channels: usize,
    src_rate: u64,
    /// Read position between `prev` (0) and `cur` (`out_rate`), in units of 1/`out_rate` of a
    /// source sample: exact integer phase, so equal rates are bit-exact and no drift accumulates.
    pos: u64,
    prev: [f32; 2],
    cur: [f32; 2],
    /// Samples popped from the ring since creation (the push index of the next one).
    popped: u64,
    shared: Arc<Shared>,
    primed: bool,
}

/// The player side: push frames, read how many the device has taken, flush, pause.
pub struct StreamFeed {
    tx: HeapProd<f32>,
    shared: Arc<Shared>,
    /// Samples pushed since creation.
    pushed: u64,
    pub src_rate: u32,
    pub channels: u16,
}

/// A connected MONO pair with room for `capacity` samples.
pub fn stream_pair(src_rate: u32, capacity: usize) -> (StreamSource, StreamFeed) {
    stream_pair_channels(src_rate, 1, capacity)
}

/// A connected pair of 1 or 2 channels (interleaved L/R) with room for `capacity` frames.
pub fn stream_pair_channels(src_rate: u32, channels: u16, capacity: usize) -> (StreamSource, StreamFeed) {
    let ch = channels.clamp(1, 2) as usize;
    let (tx, rx) = HeapRb::<f32>::new(capacity.max(1) * ch).split();
    let shared = Arc::new(Shared { consumed: AtomicU64::new(0), discard_until: AtomicU64::new(0), paused: AtomicBool::new(false) });
    (
        StreamSource { rx, channels: ch, src_rate: src_rate.max(1) as u64, pos: 0, prev: [0.0; 2], cur: [0.0; 2], popped: 0, shared: shared.clone(), primed: false },
        StreamFeed { tx, shared, pushed: 0, src_rate, channels: ch as u16 },
    )
}

impl StreamFeed {
    /// Queue as many whole frames of the interleaved `samples` as fit; returns how many
    /// SAMPLES were taken (always a multiple of the channel count).
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let ch = self.channels as usize;
        let room = self.tx.vacant_len() / ch * ch;
        let n = (samples.len() / ch * ch).min(room);
        let took = self.tx.push_slice(&samples[..n]);
        self.pushed += took as u64;
        took
    }
    /// Free room in the ring, in samples.
    pub fn room(&self) -> usize {
        self.tx.vacant_len()
    }
    /// Source frames the audio thread has consumed since the pair was made.
    pub fn consumed(&self) -> u64 {
        self.shared.consumed.load(Ordering::Acquire)
    }
    /// Drop everything queued so far: the audio thread discards it unplayed and uncounted.
    /// Frames pushed after this call play normally.
    pub fn flush(&mut self) {
        self.shared.discard_until.store(self.pushed, Ordering::Release);
    }
    /// Pause (silence, nothing taken, the count frozen) or resume.
    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
    }
}

impl StreamSource {
    /// Discard what a flush asked to drop; true when anything was dropped.
    fn drop_flushed(&mut self) -> bool {
        let until = self.shared.discard_until.load(Ordering::Acquire);
        let mut any = false;
        while self.popped < until {
            if self.rx.try_pop().is_none() {
                break;
            }
            self.popped += 1;
            any = true;
        }
        any
    }

    /// Pull one frame into `cur`.
    fn pull(&mut self) -> bool {
        if self.rx.occupied_len() < self.channels {
            return false;
        }
        self.prev = self.cur;
        for c in 0..self.channels {
            self.cur[c] = self.rx.try_pop().unwrap_or(0.0);
        }
        if self.channels == 1 {
            self.cur[1] = self.cur[0];
        }
        self.popped += self.channels as u64;
        self.shared.consumed.fetch_add(1, Ordering::AcqRel);
        true
    }
}

impl AudioNode for StreamSource {
    fn channels(&self) -> usize {
        self.channels
    }

    fn process(&mut self, _inputs: &[&[Sample; BLOCK_SIZE]], outputs: &mut [&mut [Sample; BLOCK_SIZE]], context: &GraphContext) {
        if outputs.is_empty() {
            return;
        }
        if self.drop_flushed() {
            // a jump in the stream: restart from the next sample, no ramp across the cut
            self.primed = false;
        }
        let paused = self.shared.paused.load(Ordering::Acquire);
        let out_rate = (context.sample_rate.round() as u64).max(1);
        let stereo_out = outputs.len() >= 2;
        for i in 0..BLOCK_SIZE {
            let mut frame = [0.0f64; 2];
            let mut sounding = !paused;
            if sounding && !self.primed {
                // The first sample is played as is (no ramp from an invented zero).
                if self.pull() {
                    self.prev = self.cur;
                    self.primed = true;
                    self.pos = out_rate;
                } else {
                    sounding = false;
                }
            } else if sounding {
                self.pos += self.src_rate;
                while self.pos > out_rate {
                    if self.pull() {
                        self.pos -= out_rate;
                    } else {
                        // Output silence until data returns; the next sample re-primes.
                        self.primed = false;
                        sounding = false;
                        break;
                    }
                }
            }
            if sounding {
                let frac = self.pos as f64 / out_rate as f64;
                for c in 0..2 {
                    frame[c] = self.prev[c] as f64 + (self.cur[c] as f64 - self.prev[c] as f64) * frac;
                }
            }
            if stereo_out {
                outputs[0][i] = frame[0] as Sample;
                outputs[1][i] = frame[1] as Sample;
            } else {
                outputs[0][i] = ((frame[0] + frame[1]) * 0.5) as Sample;
            }
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
    fn block2(node: &mut StreamSource, rate: f64) -> ([Sample; BLOCK_SIZE], [Sample; BLOCK_SIZE]) {
        let (mut l, mut r) = ([0.0; BLOCK_SIZE], [0.0; BLOCK_SIZE]);
        {
            let mut outs = [&mut l, &mut r];
            node.process(&[], &mut outs, &ctx(rate));
        }
        (l, r)
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

    #[test]
    fn stereo_keeps_left_and_right_bit_exact_and_counts_frames() {
        let (mut src, mut feed) = stream_pair_channels(48_000, 2, 512);
        assert_eq!(src.channels(), 2);
        let lr: Vec<f32> = (0..128).flat_map(|i| [i as f32 / 128.0, -(i as f32) / 256.0]).collect();
        // a trailing half frame is refused, never split
        let mut odd = lr.clone();
        odd.push(9.0);
        assert_eq!(feed.push(&odd), 256);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for _ in 0..2 {
            let (a, b) = block2(&mut src, 48_000.0);
            l.extend(a.iter().map(|&x| x as f32));
            r.extend(b.iter().map(|&x| x as f32));
        }
        assert_eq!(l, lr.iter().step_by(2).copied().collect::<Vec<_>>());
        assert_eq!(r, lr.iter().skip(1).step_by(2).copied().collect::<Vec<_>>());
        assert_eq!(feed.consumed(), 128, "frames, not samples");
        // a stereo node asked for one output downmixes
        feed.push(&[0.5, -0.25, 0.5, -0.25]);
        let m = block(&mut src, 48_000.0);
        assert_eq!(m[0], 0.125);
    }

    #[test]
    fn flush_discards_unplayed_and_uncounted_then_plays_new() {
        let (mut src, mut feed) = stream_pair_channels(48_000, 2, 1024);
        feed.push(&vec![0.75f32; 2 * 300]);
        let (a, _) = block2(&mut src, 48_000.0);
        assert!(a.iter().all(|&x| x == 0.75));
        assert_eq!(feed.consumed(), 64);
        feed.flush();
        let fresh: Vec<f32> = (0..64).flat_map(|i| [i as f32, i as f32 + 0.5]).collect();
        feed.push(&fresh);
        let (l, r) = block2(&mut src, 48_000.0);
        assert_eq!(l[0], 0.0, "the first sample after the flush is the new stream's first");
        assert_eq!(r[0], 0.5);
        assert!(l.iter().enumerate().all(|(i, &x)| x == i as f64));
        assert_eq!(feed.consumed(), 128, "the 236 flushed frames were never counted");
    }

    #[test]
    fn pause_is_silent_takes_nothing_and_resumes_in_place() {
        let (mut src, mut feed) = stream_pair(48_000, 1024);
        let ramp: Vec<f32> = (0..200).map(|i| i as f32).collect();
        feed.push(&ramp);
        let a = block(&mut src, 48_000.0);
        assert_eq!(a[63], 63.0);
        feed.set_paused(true);
        assert!(block(&mut src, 48_000.0).iter().all(|&x| x == 0.0));
        assert_eq!(feed.consumed(), 64);
        feed.set_paused(false);
        let b = block(&mut src, 48_000.0);
        assert_eq!(b[0], 64.0);
        assert_eq!(feed.consumed(), 128);
    }

    #[test]
    fn graph_carries_a_stereo_node_and_copies_mono_ones() {
        let (src, mut feed) = stream_pair_channels(48_000, 2, 256);
        feed.push(&(0..64).flat_map(|i| [i as f32, -(i as f32)]).collect::<Vec<_>>());
        let mut g = crate::AudioGraph::new(48_000.0);
        g.add_node(Box::new(src));
        let (l, r) = g.process_stereo();
        assert!(l.iter().enumerate().all(|(i, &x)| x == i as f64));
        assert!(r.iter().enumerate().all(|(i, &x)| x == -(i as f64)));
        let mut m = crate::AudioGraph::new(48_000.0);
        m.add_node(Box::new(crate::SineOscillator::new(440.0)));
        let (l, r) = m.process_stereo();
        assert_eq!(l, r);
        assert!(l[1] != 0.0);
    }
}
