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
