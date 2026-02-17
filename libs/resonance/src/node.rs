use crate::{BLOCK_SIZE, Sample};

/// Context passed to every node during processing.
#[derive(Debug, Clone, Copy)]
pub struct GraphContext {
    pub sample_rate: Sample,
    pub inv_sample_rate: Sample,
}

impl GraphContext {
    pub fn new(sample_rate: Sample) -> Self {
        Self {
            sample_rate,
            inv_sample_rate: 1.0 / sample_rate,
        }
    }
}

/// The contract for all audio processing nodes.
pub trait AudioNode {
    /// Process a block of audio.
    ///
    /// # Arguments
    ///
    /// * `inputs` - A slice of references to input buffers. Each buffer is a fixed-size array of `BLOCK_SIZE` samples.
    /// * `outputs` - A mutable slice of mutable references to output buffers. Each buffer is a fixed-size array of `BLOCK_SIZE` samples.
    /// * `context` - The global graph context (sample rate, etc.).
    fn process(
        &mut self,
        inputs: &[&[Sample; BLOCK_SIZE]],
        outputs: &mut [&mut [Sample; BLOCK_SIZE]],
        context: &GraphContext,
    );
}
