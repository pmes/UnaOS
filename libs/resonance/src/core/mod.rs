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

    /// Set a node-specific parameter.
    ///
    /// # Arguments
    /// * `id` - The parameter ID (meaning defined by the implementation).
    /// * `value` - The new value.
    fn set_param(&mut self, _id: usize, _value: f64) {
        // Default implementation does nothing.
    }
}
