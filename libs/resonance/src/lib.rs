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

pub type Sample = f64;
pub const BLOCK_SIZE: usize = 64;

pub mod audio;
pub mod commands;
pub mod core;
pub mod dsp;
pub mod graph;
pub mod nodes;

pub use audio::{AudioEngine, create_test_graph};
pub use commands::AudioCommand;
pub use core::{AudioNode, GraphContext};
pub use graph::{AudioGraph, NodeId};
pub use nodes::gain::Gain;
pub use nodes::mixer::Mixer;
pub use nodes::oscillators::SineOscillator;
