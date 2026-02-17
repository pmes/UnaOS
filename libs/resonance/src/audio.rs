use crate::graph::AudioGraph;
use crate::{BLOCK_SIZE, Sample};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// The engine that manages the audio driver and drives the graph.
pub struct AudioEngine {
    // The stream must be held to keep the audio running.
    _stream: cpal::Stream,
}

impl AudioEngine {
    /// Creates a new AudioEngine, moving the Graph into the audio thread.
    ///
    /// This function initializes the default host and output device, configures
    /// the stream, and starts the processing loop.
    pub fn new(mut graph: AudioGraph) -> Result<Self, anyhow::Error> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No default output device available"))?;

        let config = device.default_output_config()?;
        let sample_format = config.sample_format();
        let config: cpal::StreamConfig = config.into();

        // TODO: In a real app, we might want to verify sample rate matches graph context
        // or re-initialize graph with device sample rate.
        // For now, we assume the graph was created with a compatible rate or tolerates mismatch.

        let channels = config.channels as usize;

        // State for the callback
        // We move the graph into the closure.
        // We need a cursor to track where we are in the current block.
        let mut block_offset = 0;
        let mut current_block = [0.0; BLOCK_SIZE];

        // Fill initial block
        // We can't call process yet as we are outside, but let's prep the buffer.
        // Actually, let's just let the first iteration fill it.
        // To be safe, let's say block_offset = BLOCK_SIZE so it triggers immediately.
        block_offset = BLOCK_SIZE;

        let err_fn = |err| eprintln!("an error occurred on stream: {}", err);

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

        Ok(Self { _stream: stream })
    }
}

/// Helper function to write f32 output.
/// Separated to keep the closure clean.
fn write_output_f32(
    output: &mut [f32],
    channels: usize,
    graph: &mut AudioGraph,
    current_block: &mut [Sample; BLOCK_SIZE],
    block_offset: &mut usize,
) {
    // Iterate over frames (chunks of samples, one per channel)
    for frame in output.chunks_mut(channels) {
        // If we have exhausted the current block, generate a new one
        if *block_offset >= BLOCK_SIZE {
            let processed = graph.process();
            // Copy to our local cache because 'processed' is a reference to graph internal memory
            current_block.copy_from_slice(processed);
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

/// Helper to create a test graph (Sine 440Hz -> Gain 0.1).
pub fn create_test_graph() -> AudioGraph {
    use crate::nodes::gain::Gain;
    use crate::nodes::oscillators::SineOscillator;

    // Assume standard sample rate for now
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
