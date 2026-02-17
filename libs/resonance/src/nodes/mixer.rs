use crate::core::{AudioNode, GraphContext};
use crate::{BLOCK_SIZE, Sample};

/// A Summing Mixer node.
///
/// Sums all connected inputs to the output.
#[derive(Debug, Clone, Default)]
pub struct Mixer;

impl Mixer {
    pub fn new() -> Self {
        Self
    }
}

impl AudioNode for Mixer {
    fn process(
        &mut self,
        inputs: &[&[Sample; BLOCK_SIZE]],
        outputs: &mut [&mut [Sample; BLOCK_SIZE]],
        _context: &GraphContext,
    ) {
        // Must have at least one output buffer.
        if outputs.is_empty() {
            return;
        }
        let out = &mut outputs[0];

        match inputs.len() {
            0 => {
                // 0 Inputs: Output Silence.
                out.fill(0.0);
            }
            1 => {
                // 1 Input: Copy input to output.
                out.copy_from_slice(inputs[0]);
            }
            _ => {
                // 2+ Inputs: Accumulate.
                // Start by copying the first input to avoid zeroing.
                out.copy_from_slice(inputs[0]);

                // Add subsequent inputs.
                for input in inputs.iter().skip(1) {
                    for i in 0..BLOCK_SIZE {
                        out[i] += input[i];
                    }
                }
            }
        }
    }
}
