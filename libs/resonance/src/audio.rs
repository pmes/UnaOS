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

use crate::commands::AudioCommand;
use crate::graph::AudioGraph;
use crate::{BLOCK_SIZE, Sample};
use bandy::{BandyMember, SMessage};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{HeapProd, HeapRb, traits::*};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The engine that manages the audio driver and drives the graph.
pub struct AudioEngine {
    // The stream must be held to keep the audio running.
    _stream: cpal::Stream,
    pub sample_rate: u32,
    /// Shared liveness flag: true while the callback is producing the graph's
    /// signal. Flipped false by [`AudioCommand::Stop`] and by a stream error
    /// (so a dead device is visible), flipped true by [`ResonanceHandle::start`].
    active: Arc<AtomicBool>,
}

/// The control-thread face of a running [`AudioEngine`].
///
/// `AudioEngine::new` returns the command producer as an unnameable
/// `impl Producer` — a GUI struct cannot store that. `ResonanceHandle` is the
/// nameable owner: it wraps the producer plus the two atomics the callback
/// shares (liveness + output level), so a vessel can hold it in a field and
/// drive the engine from buttons, sliders, and cadence tasks.
///
/// All methods are real-time safe on the *audio* side: they only enqueue onto
/// the lock-free ring buffer or touch atomics; nothing here blocks the
/// callback.
pub struct ResonanceHandle {
    producer: HeapProd<AudioCommand>,
    active: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
}

impl ResonanceHandle {
    /// Set the master frequency in Hz (targets node 0 — see
    /// [`AudioCommand::SetMasterFrequency`]). Returns false if the command
    /// queue is full.
    pub fn set_frequency(&mut self, hz: f64) -> bool {
        self.producer
            .try_push(AudioCommand::SetMasterFrequency(hz))
            .is_ok()
    }

    /// Set an arbitrary node parameter. Returns false if the command queue is
    /// full.
    pub fn set_param(&mut self, node: usize, param: usize, value: f64) -> bool {
        self.producer
            .try_push(AudioCommand::SetParam {
                node_id: node,
                param_id: param,
                value,
            })
            .is_ok()
    }

    /// Stop the output: the callback drains this command, flips the shared
    /// liveness flag, and emits silence from the next block on. Returns false
    /// if the command queue is full.
    pub fn stop(&mut self) -> bool {
        self.producer.try_push(AudioCommand::Stop).is_ok()
    }

    /// Resume output after [`stop`](Self::stop). Sets the shared liveness
    /// flag directly (no queue round-trip); the callback picks it up at the
    /// next block boundary. Note this cannot revive a stream the *device*
    /// killed — `is_active` flipping false again right away is the signal.
    pub fn start(&self) {
        self.active.store(true, Ordering::Release);
    }

    /// Whether the callback is currently producing the graph's signal.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// The peak absolute sample value of the most recently generated block
    /// (`0.0` while stopped). Written once per block by the audio callback.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Acquire))
    }

    /// A cloneable, `Send` read-only probe of the engine's level + liveness,
    /// for cadence tasks that must not own the (single) command producer.
    pub fn meter(&self) -> ResonanceMeter {
        ResonanceMeter {
            active: self.active.clone(),
            level: self.level.clone(),
        }
    }
}

/// Read-only view of a running engine's output level and liveness.
/// See [`ResonanceHandle::meter`].
#[derive(Clone)]
pub struct ResonanceMeter {
    active: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
}

impl ResonanceMeter {
    /// See [`ResonanceHandle::level`].
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Acquire))
    }

    /// See [`ResonanceHandle::is_active`].
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

impl AudioEngine {
    /// Creates a new AudioEngine, moving the Graph into the audio thread.
    ///
    /// This function initializes the default host and output device, re-tunes
    /// the graph to the device's real sample rate (so a graph built against a
    /// default rate plays at true pitch), configures the stream, and starts
    /// the processing loop.
    ///
    /// Returns the engine instance (which must be kept alive) and the
    /// [`ResonanceHandle`] that controls it.
    pub fn new(mut graph: AudioGraph) -> Result<(Self, ResonanceHandle), anyhow::Error> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No default output device available"))?;

        let config = device.default_output_config()?;
        let sample_format = config.sample_format();
        let sample_rate = config.sample_rate();
        let config: cpal::StreamConfig = config.into();

        let channels = config.channels as usize;

        // Honesty first: the graph must run at the device's rate, not
        // whatever rate it was constructed with, or every oscillator plays
        // detuned by the ratio of the two.
        graph.set_sample_rate(sample_rate as Sample);

        // Command Channel
        // Create a ring buffer for commands.
        let ring = HeapRb::<AudioCommand>::new(128);
        let (producer, mut consumer) = ring.split();

        // Shared state between the callback, the engine, and the handle.
        let active = Arc::new(AtomicBool::new(true));
        let level = Arc::new(AtomicU32::new(0.0f32.to_bits()));

        // State for the callback
        // We move the graph into the closure.
        // We need a cursor to track where we are in the current block.
        let mut block_offset = BLOCK_SIZE;
        let mut current_block = [0.0; BLOCK_SIZE];

        let cb_active = active.clone();
        let cb_level = level.clone();

        // A stream error means the device died: say so AND flip the shared
        // flag so `is_active` readers actually find out.
        let err_active = active.clone();
        let err_fn = move |err| {
            eprintln!("an error occurred on stream: {}", err);
            err_active.store(false, Ordering::Release);
        };

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    write_output_f32(
                        data,
                        channels,
                        &mut graph,
                        &mut current_block,
                        &mut block_offset,
                        &mut consumer,
                        &cb_active,
                        &cb_level,
                    );
                },
                err_fn,
                None, // Timeout
            )?,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unsupported sample format: {:?}",
                    sample_format
                ));
            }
        };

        stream.play()?;

        Ok((
            Self {
                _stream: stream,
                sample_rate,
                active: active.clone(),
            },
            ResonanceHandle {
                producer,
                active,
                level,
            },
        ))
    }

    /// Whether the callback is currently producing the graph's signal (false
    /// after a [`AudioCommand::Stop`] or a stream error).
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Simulates processing a chunk of audio and broadcasting it.
    /// This is where the physics (Resonance) meets the wire (Bandy).
    pub fn process_frame(&self, raw_samples: Vec<f32>) -> anyhow::Result<()> {
        // 1. (Future) Apply DSP / Noise Reduction here.

        // 2. Wrap it in the Monolithic Enum.
        let msg = SMessage::AudioChunk {
            source_id: "mic_01".to_string(),
            samples: raw_samples, // In reality, this would be the processed buffer
            sample_rate: self.sample_rate,
        };

        // 3. Publish to the Nervous System.
        self.publish("system/audio/input", msg)?;

        Ok(())
    }
}

// Implement the Nervous System Interface
impl BandyMember for AudioEngine {
    fn publish(&self, topic: &str, msg: SMessage) -> anyhow::Result<()> {
        // TODO: This will eventually push to the specific Transport Layer (MQTT/ZMQ).
        // For now, we just acknowledge the data structure exists.
        println!("[BANDY] Publishing to '{}': {:?}", topic, msg);
        Ok(())
    }
}

/// Helper function to write f32 output.
/// Separated to keep the closure clean (and unit-testable without a device).
///
/// Real-time safe: no allocation, no locks — only the lock-free ring buffer
/// and two atomics.
#[allow(clippy::too_many_arguments)]
fn write_output_f32(
    output: &mut [f32],
    channels: usize,
    graph: &mut AudioGraph,
    current_block: &mut [Sample; BLOCK_SIZE],
    block_offset: &mut usize,
    consumer: &mut impl Consumer<Item = AudioCommand>,
    active: &AtomicBool,
    level: &AtomicU32,
) {
    // Iterate over frames (chunks of samples, one per channel)
    for frame in output.chunks_mut(channels) {
        // If we have exhausted the current block, generate a new one
        if *block_offset >= BLOCK_SIZE {
            // 1. Process Commands (Before generating the next block).
            // Commands drain even while stopped, so a queued Stop can't be
            // starved and parameter changes made during silence apply on
            // resume.
            process_commands(graph, consumer, active);

            // 2. Generate Audio — or silence, if stopped. The graph is not
            // processed while stopped, so its state (oscillator phase)
            // freezes until resume.
            if active.load(Ordering::Acquire) {
                let processed = graph.process();
                // Copy to our local cache because 'processed' is a reference to graph internal memory
                current_block.copy_from_slice(processed);
            } else {
                current_block.fill(0.0);
            }

            // 3. Publish the block peak — one atomic store per block — so
            // control threads can meter the output without touching the
            // graph.
            let mut peak: Sample = 0.0;
            for &s in current_block.iter() {
                let a = s.abs();
                if a > peak {
                    peak = a;
                }
            }
            level.store((peak as f32).to_bits(), Ordering::Release);

            *block_offset = 0;
        }

        // Get sample from current block
        // Convert f64 -> f32
        let sample = current_block[*block_offset] as f32;
        *block_offset += 1;

        // Write to all channels in the frame
        for sample_out in frame.iter_mut() {
            *sample_out = sample;
        }
    }
}

/// Consumes commands from the ring buffer and applies them to the graph.
///
/// `SetMasterFrequency` targets node 0, param 0. That assumption holds for
/// the fixed test graph (`create_test_graph` adds its oscillator first) and
/// any graph built the same way; graphs with a different layout should be
/// driven via `SetParam` instead.
fn process_commands(
    graph: &mut AudioGraph,
    consumer: &mut impl Consumer<Item = AudioCommand>,
    active: &AtomicBool,
) {
    while let Some(cmd) = consumer.try_pop() {
        match cmd {
            AudioCommand::SetMasterFrequency(freq) => {
                graph.set_node_param(crate::NodeId(0), 0, freq);
            }
            AudioCommand::SetParam {
                node_id,
                param_id,
                value,
            } => {
                graph.set_node_param(crate::NodeId(node_id), param_id, value);
            }
            AudioCommand::Stop => {
                active.store(false, Ordering::Release);
            }
        }
    }
}

/// Helper to create a test graph (Sine 440Hz -> Gain 0.1).
///
/// Node layout (relied on by `SetMasterFrequency` and the examples):
/// node 0 = oscillator, node 1 = gain.
///
/// Built at a default 44 100 Hz; `AudioEngine::new` re-tunes the graph to the
/// real device rate before it starts processing.
pub fn create_test_graph() -> AudioGraph {
    use crate::nodes::gain::Gain;
    use crate::nodes::oscillators::SineOscillator;

    // Default rate — corrected at engine start (see AudioEngine::new).
    let mut graph = AudioGraph::new(44100.0);

    // Osc: 440 Hz
    let osc = Box::new(SineOscillator::new(440.0));
    let osc_id = graph.add_node(osc);

    // Gain: 0.1 (Prevent blowing ears)
    let gain = Box::new(Gain::new(0.1));
    let gain_id = graph.add_node(gain);

    // Connect Osc -> Gain
    graph.connect(osc_id, gain_id, 0);

    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// A fresh command ring pair, matching the engine's construction.
    fn command_ring() -> (HeapProd<AudioCommand>, impl Consumer<Item = AudioCommand>) {
        HeapRb::<AudioCommand>::new(128).split()
    }

    /// The expected second sample of a fresh test graph: sin(TAU·f/sr)·gain
    /// (the first sample is sin(0) = 0, which proves nothing).
    fn expected_sample_1(freq: f64, sample_rate: f64, gain: f64) -> f64 {
        (TAU * freq / sample_rate).sin() * gain
    }

    #[test]
    fn set_master_frequency_reaches_the_graph() {
        let mut graph = create_test_graph();
        let (mut producer, mut consumer) = command_ring();
        let active = AtomicBool::new(true);

        producer
            .try_push(AudioCommand::SetMasterFrequency(880.0))
            .unwrap();
        process_commands(&mut graph, &mut consumer, &active);

        let out = graph.process();
        let expected = expected_sample_1(880.0, 44100.0, 0.1);
        assert!(
            (out[1] - expected).abs() < 1e-9,
            "frequency command must change the oscillator: expected {expected}, got {}",
            out[1]
        );
    }

    #[test]
    fn set_param_reaches_the_gain_node() {
        let mut graph = create_test_graph();
        let (mut producer, mut consumer) = command_ring();
        let active = AtomicBool::new(true);

        // Node 1 = gain, param 0 = base_gain (see create_test_graph).
        producer
            .try_push(AudioCommand::SetParam {
                node_id: 1,
                param_id: 0,
                value: 0.5,
            })
            .unwrap();
        process_commands(&mut graph, &mut consumer, &active);

        let out = graph.process();
        let expected = expected_sample_1(440.0, 44100.0, 0.5);
        assert!(
            (out[1] - expected).abs() < 1e-9,
            "gain command must change the gain node: expected {expected}, got {}",
            out[1]
        );
    }

    #[test]
    fn graph_retune_changes_pitch() {
        let mut graph = create_test_graph();
        graph.set_sample_rate(48000.0);

        let out = graph.process();
        let expected = expected_sample_1(440.0, 48000.0, 0.1);
        assert!(
            (out[1] - expected).abs() < 1e-9,
            "retuned graph must advance phase at the new rate: expected {expected}, got {}",
            out[1]
        );
    }

    #[test]
    fn stop_silences_output_and_deactivates() {
        let mut graph = create_test_graph();
        let (mut producer, mut consumer) = command_ring();
        let active = AtomicBool::new(true);
        let level = AtomicU32::new(0.5f32.to_bits());

        producer.try_push(AudioCommand::Stop).unwrap();

        // Two blocks of stereo frames, pre-filled with garbage the callback
        // must overwrite with silence.
        let mut output = [1.0f32; BLOCK_SIZE * 2 * 2];
        let mut current_block = [0.7; BLOCK_SIZE];
        let mut block_offset = BLOCK_SIZE; // force a fresh block immediately

        write_output_f32(
            &mut output,
            2,
            &mut graph,
            &mut current_block,
            &mut block_offset,
            &mut consumer,
            &active,
            &level,
        );

        assert!(!active.load(Ordering::Acquire), "Stop must clear the flag");
        assert!(
            output.iter().all(|&s| s == 0.0),
            "stopped engine must emit pure silence"
        );
        assert_eq!(
            f32::from_bits(level.load(Ordering::Acquire)),
            0.0,
            "level must read 0.0 while stopped"
        );
    }

    #[test]
    fn callback_reports_block_peak_level() {
        let mut graph = create_test_graph();
        let (_producer, mut consumer) = command_ring();
        let active = AtomicBool::new(true);
        let level = AtomicU32::new(0.0f32.to_bits());

        // One 64-sample block at 440 Hz / 44.1 kHz covers ~0.64 of a cycle,
        // so the in-block peak reaches the full amplitude (sin peaks at the
        // quarter cycle): 1.0 * gain 0.1.
        let mut output = [0.0f32; BLOCK_SIZE * 2];
        let mut current_block = [0.0; BLOCK_SIZE];
        let mut block_offset = BLOCK_SIZE;

        write_output_f32(
            &mut output,
            2,
            &mut graph,
            &mut current_block,
            &mut block_offset,
            &mut consumer,
            &active,
            &level,
        );

        let peak = f32::from_bits(level.load(Ordering::Acquire));
        assert!(
            (0.09..=0.101).contains(&peak),
            "block peak should be ~0.1 (gain-scaled sine peak), got {peak}"
        );
    }

    #[test]
    fn stopped_graph_resumes_after_start_flag() {
        let mut graph = create_test_graph();
        let (mut producer, mut consumer) = command_ring();
        let active = AtomicBool::new(true);
        let level = AtomicU32::new(0.0f32.to_bits());

        producer.try_push(AudioCommand::Stop).unwrap();

        let mut output = [0.0f32; BLOCK_SIZE * 2];
        let mut current_block = [0.0; BLOCK_SIZE];
        let mut block_offset = BLOCK_SIZE;

        // Block 1: consumed the Stop — silence.
        write_output_f32(
            &mut output,
            2,
            &mut graph,
            &mut current_block,
            &mut block_offset,
            &mut consumer,
            &active,
            &level,
        );
        assert!(output.iter().all(|&s| s == 0.0));

        // What ResonanceHandle::start does: flip the shared flag.
        active.store(true, Ordering::Release);

        // Block 2: signal is back.
        block_offset = BLOCK_SIZE;
        write_output_f32(
            &mut output,
            2,
            &mut graph,
            &mut current_block,
            &mut block_offset,
            &mut consumer,
            &active,
            &level,
        );
        assert!(
            output.iter().any(|&s| s != 0.0),
            "resumed engine must produce signal again"
        );
    }
}
