use crate::core::{AudioNode, GraphContext};
use crate::{BLOCK_SIZE, Sample};

/// A Voltage Controlled Amplifier (VCA) node.
///
/// Inputs:
/// - 0: Audio Signal
/// - 1: Control Signal (Modulation) - Optional
#[derive(Debug, Clone)]
pub struct Gain {
    /// The base gain factor.
    pub base_gain: Sample,
}

impl Gain {
    /// Creates a new Gain node with the specified base gain.
    pub fn new(base_gain: Sample) -> Self {
        Self { base_gain }
    }
}

impl Default for Gain {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl AudioNode for Gain {
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
                // 1 Input (Signal Only): Output = Input * base_gain.
                let signal = inputs[0];
                for i in 0..BLOCK_SIZE {
                    out[i] = signal[i] * self.base_gain;
                }
            }
            _ => {
                // 2+ Inputs (Signal + Mod): Output = Input * (base_gain + Mod).
                let signal = inputs[0];
                let modulation = inputs[1];
                for i in 0..BLOCK_SIZE {
                    out[i] = signal[i] * (self.base_gain + modulation[i]);
                }
            }
        }
    }
}
