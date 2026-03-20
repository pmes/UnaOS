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

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TetraNode {
    Matrix, // Future MatrixTetra (Sidebar)
    Stream, // Future StreamTetra (Comms)
    Empty,  // Placeholder
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceTetra {
    pub left_pane: TetraNode,
    pub right_pane: TetraNode,
    pub split_ratio: f32,
}

impl Default for WorkspaceTetra {
    fn default() -> Self {
        Self {
            left_pane: TetraNode::Matrix,
            right_pane: TetraNode::Stream,
            split_ratio: 0.25,
        }
    }
}
