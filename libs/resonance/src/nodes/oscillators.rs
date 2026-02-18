use crate::core::{AudioNode, GraphContext};
use crate::{BLOCK_SIZE, Sample};
use std::f64::consts::TAU;

/// A simple sine wave oscillator with optional frequency modulation.
#[derive(Debug, Clone)]
pub struct SineOscillator {
    /// The base frequency in Hz.
    pub frequency: Sample,
    /// The current phase (0.0 to 1.0).
    pub phase: Sample,
}

impl SineOscillator {
    /// Creates a new sine oscillator with the given frequency.
    pub fn new(frequency: Sample) -> Self {
        Self {
            frequency,
            phase: 0.0,
        }
    }
}

impl AudioNode for SineOscillator {
    fn process(
        &mut self,
        inputs: &[&[Sample; BLOCK_SIZE]],
        outputs: &mut [&mut [Sample; BLOCK_SIZE]],
        context: &GraphContext,
    ) {
        // We need at least one output buffer to write to.
        if outputs.is_empty() {
            return;
        }

        let out = &mut outputs[0];

        // FM Synthesis Check: Look at inputs[0].
        let fm_input = if !inputs.is_empty() {
            Some(inputs[0])
        } else {
            None
        };

        for i in 0..BLOCK_SIZE {
            // The modulation value for this sample
            let modulation = if let Some(fm) = fm_input { fm[i] } else { 0.0 };

            // Calculate the sine value: sin(TAU * self.phase).
            out[i] = (self.phase * TAU).sin();

            // The Advance: Increment phase by (frequency + modulation) / sample_rate.
            // We use multiplication by inverse sample rate for performance.
            let increment = (self.frequency + modulation) * context.inv_sample_rate;
            self.phase += increment;

            // The Wrap: If phase exceeds 1.0, wrap it back down.
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            } else if self.phase < 0.0 {
                // Handle negative frequency/modulation cases just in case
                self.phase += 1.0;
            }
        }
    }

    fn set_param(&mut self, id: usize, value: f64) {
        match id {
            // Param 0: Frequency
            0 => self.frequency = value,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::GraphContext;

    #[test]
    fn test_sine_oscillator_basic() {
        let mut osc = SineOscillator::new(440.0);
        let context = GraphContext::new(44100.0);
        let mut output = [0.0; BLOCK_SIZE];
        let mut outputs = [&mut output];
        let inputs: &[&[Sample; BLOCK_SIZE]] = &[];

        osc.process(inputs, &mut outputs, &context);

        // Check first sample is 0.0 (phase 0)
        assert!((outputs[0][0] - 0.0).abs() < 1e-6);
        // Check subsequent samples are within -1.0 to 1.0
        for i in 0..BLOCK_SIZE {
            assert!(outputs[0][i] >= -1.0 && outputs[0][i] <= 1.0);
        }
    }

    #[test]
    fn test_sine_oscillator_fm() {
        let mut osc = SineOscillator::new(440.0);
        let context = GraphContext::new(44100.0);
        let mut output = [0.0; BLOCK_SIZE];
        let mut outputs = [&mut output];
        let mod_input = [10.0; BLOCK_SIZE]; // Constant 10Hz modulation
        let inputs: &[&[Sample; BLOCK_SIZE]] = &[&mod_input];

        osc.process(inputs, &mut outputs, &context);

        // Just ensure it runs and produces output
        for i in 0..BLOCK_SIZE {
            assert!(outputs[0][i] >= -1.0 && outputs[0][i] <= 1.0);
        }
    }

    #[test]
    fn test_sine_oscillator_set_param() {
        let mut osc = SineOscillator::new(440.0);
        assert_eq!(osc.frequency, 440.0);
        osc.set_param(0, 880.0);
        assert_eq!(osc.frequency, 880.0);
    }
}
