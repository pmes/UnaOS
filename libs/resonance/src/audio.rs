use bandy::{BandyMember, SMessage};
use crate::commands::AudioCommand;
use crate::graph::AudioGraph;
use crate::{BLOCK_SIZE, Sample};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{Consumer, HeapRb, Producer};
use std::sync::Arc;

/// The engine that manages the audio driver and drives the graph.
pub struct AudioEngine {
    // The stream must be held to keep the audio running.
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub is_active: bool,
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
        let sample_rate = config.sample_rate().0;
        let config: cpal::StreamConfig = config.into();

        let channels = config.channels as usize;

        // Command Channel
        // Create a ring buffer for commands.
        let ring = HeapRb::<AudioCommand>::new(128);
        let (producer, mut consumer) = ring.split();

        // State for the callback
        // We move the graph into the closure.
        // We need a cursor to track where we are in the current block.
        let mut block_offset = BLOCK_SIZE;
        let mut current_block = [0.0; BLOCK_SIZE];

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

        Ok((
            Self {
                _stream: stream,
                sample_rate,
                is_active: true,
            },
            producer,
        ))
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
    _graph: &mut AudioGraph,
    consumer: &mut Consumer<AudioCommand, Arc<HeapRb<AudioCommand>>>,
) {
    while let Some(cmd) = consumer.pop() {
        match cmd {
            AudioCommand::SetMasterFrequency(_freq) => {
                // _graph.set_node_param(crate::NodeId(0), 0, _freq);
            }
            AudioCommand::SetParam {
                node_id: _node_id,
                param_id: _param_id,
                value: _value,
            } => {
                // _graph.set_node_param(crate::NodeId(_node_id), _param_id, _value);
            }
            AudioCommand::Stop => {
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
