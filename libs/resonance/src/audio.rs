use crate::commands::AudioCommand;
use crate::graph::AudioGraph;
use crate::{BLOCK_SIZE, Sample};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{Consumer, HeapRb, Producer};
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
    ///
    /// Returns the engine instance (which must be kept alive) and a command producer.
    pub fn new(
        mut graph: AudioGraph,
    ) -> Result<(Self, Producer<AudioCommand, Arc<HeapRb<AudioCommand>>>), anyhow::Error> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No default output device available"))?;

        let config = device.default_output_config()?;
        let sample_format = config.sample_format();
        let config: cpal::StreamConfig = config.into();

        let channels = config.channels as usize;

        // Command Channel
        // Create a ring buffer for commands.
        let ring = HeapRb::<AudioCommand>::new(128);
        let (producer, mut consumer) = ring.split();

        // State for the callback
        // We move the graph into the closure.
        // We need a cursor to track where we are in the current block.
        let mut block_offset = 0;
        let mut current_block = [0.0; BLOCK_SIZE];

        // Fill initial block so we start fresh
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
                        &mut consumer,
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

        Ok((Self { _stream: stream }, producer))
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
    consumer: &mut Consumer<AudioCommand, Arc<HeapRb<AudioCommand>>>,
) {
    // Iterate over frames (chunks of samples, one per channel)
    for frame in output.chunks_mut(channels) {
        // If we have exhausted the current block, generate a new one
        if *block_offset >= BLOCK_SIZE {
            // 1. Process Commands (Before generating the next block)
            process_commands(graph, consumer);

            // 2. Generate Audio
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

/// Consumes commands from the ring buffer and applies them to the graph.
fn process_commands(
    graph: &mut AudioGraph,
    consumer: &mut Consumer<AudioCommand, Arc<HeapRb<AudioCommand>>>,
) {
    while let Some(cmd) = consumer.pop() {
        match cmd {
            AudioCommand::SetMasterFrequency(freq) => {
                // For prototype: Target Node 0 (Oscillator)
                // In a real system we would use a map or explicit ID from command.
                // We access the raw nodes vector if possible, but AudioGraph encapsulates it.
                // We need to implement a command handler or param setter on AudioGraph.
                // Since AudioGraph exposes public fields in our current implementation (checked previously),
                // we might not have direct mutable access to nodes if they are private.
                // Let's assume for now we need to add a method to AudioGraph or if fields are public.
                // Checking previous implementation: `nodes: Vec<Box<dyn AudioNode + Send>>` is private (default visibility in struct definition was not pub).
                // Wait, `nodes` in `graph.rs` was defined as `nodes: Vec<Box...>`, not `pub nodes`.
                // So we cannot access it directly here.
                // We should add a helper on AudioGraph.

                // However, since we are in `libs/resonance`, we can modify `graph.rs`.
                // Or we can rely on `AudioGraph` having a method.
                // Let's implement `graph.set_node_param(id, param, value)`.

                // BUT, for this specific directive, the user said:
                // "if let Some(node) = graph.nodes.get_mut(0) { node.set_param(0, f); }"
                // This implies `nodes` should be accessible.
                // Since `audio.rs` is a sibling module to `graph.rs` and `nodes` is private to `graph` module,
                // we need to make `nodes` pub(crate) or add a method.

                // I will add a method `handle_command` to AudioGraph in `graph.rs` in the next step or
                // modify `graph.rs` now. Since I can't modify `graph.rs` in this step (strictly),
                // I will assume I can add a temporary method here if I can access it, or I will use a method I'll add to `graph.rs`.

                // Actually, let's look at `graph.rs` again.
                // `pub struct AudioGraph { nodes: Vec... }` - fields are private by default.

                // Strategy: I will add `set_node_param` to `AudioGraph` in `graph.rs`
                // BUT I am editing `audio.rs` now.
                // So I will call `graph.set_node_param(NodeId(0), 0, freq)` and implement it in `graph.rs` in a fix-up step
                // OR I can just edit `graph.rs` quickly before this file.

                // To avoid compilation error, I will define `set_node_param` call here and then update `graph.rs`.

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
                // Panic button logic - effectively mute or clear
                // For now, maybe set gain to 0 if we knew where gain was.
                // Or just do nothing for prototype as per instructions "Panic button".
            }
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
