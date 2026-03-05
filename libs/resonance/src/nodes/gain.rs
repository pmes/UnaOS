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
