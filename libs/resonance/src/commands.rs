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

/// Commands sent from the UI thread to the Audio Engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioCommand {
    /// Update a specific parameter on a specific node.
    ///
    /// # Arguments
    /// * `node_id` - The index of the node in the graph.
    /// * `param_id` - The parameter ID (node-specific).
    /// * `value` - The new value.
    SetParam {
        node_id: usize,
        param_id: usize,
        value: f64,
    },

    /// Stop the audio engine immediately (panic button).
    Stop,

    /// Update the master frequency (assumes Node 0 is an oscillator).
    /// For the prototype: Just change the oscillator pitch.
    SetMasterFrequency(f64),
}
