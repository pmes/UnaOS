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
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub label: String,
    pub children: Vec<TreeNode>,
    pub is_expanded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExpandableList {
    pub roots: Vec<TreeNode>,
}

impl ExpandableList {
    pub fn flatten(&self) -> Vec<(&TreeNode, usize)> {
        let mut result = Vec::new();
        for root in &self.roots {
            self.flatten_recursive(root, 0, &mut result);
        }
        result
    }

    fn flatten_recursive<'a>(&'a self, node: &'a TreeNode, depth: usize, result: &mut Vec<(&'a TreeNode, usize)>) {
        result.push((node, depth));
        if node.is_expanded {
            for child in &node.children {
                self.flatten_recursive(child, depth + 1, result);
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SelectionState {
    pub selected_ids: HashSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatrixTetra {
    pub tree: ExpandableList,
    pub selection: SelectionState,
}

impl Default for MatrixTetra {
    fn default() -> Self {
        let tree = ExpandableList {
            roots: vec![
                TreeNode {
                    id: "unaos_core".to_string(),
                    label: "UnaOS Core".to_string(),
                    is_expanded: true,
                    children: vec![
                        TreeNode {
                            id: "kernel".to_string(),
                            label: "Kernel".to_string(),
                            is_expanded: false,
                            children: vec![],
                        },
                        TreeNode {
                            id: "dmz".to_string(),
                            label: "DMZ".to_string(),
                            is_expanded: false,
                            children: vec![],
                        },
                    ],
                },
                TreeNode {
                    id: "embassies".to_string(),
                    label: "Embassies".to_string(),
                    is_expanded: false,
                    children: vec![
                        TreeNode {
                            id: "gtk".to_string(),
                            label: "GTK".to_string(),
                            is_expanded: false,
                            children: vec![],
                        },
                        TreeNode {
                            id: "qt".to_string(),
                            label: "Qt".to_string(),
                            is_expanded: false,
                            children: vec![],
                        },
                    ],
                },
            ],
        };

        Self {
            tree,
            selection: SelectionState::default(),
        }
    }
}


#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScrollAnchor {
    Top,
    Bottom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScrollBehavior {
    AutoScroll,
    Manual,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamAlign {
    Start,
    End,
    Center,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamTetra {
    pub input_anchor: ScrollAnchor,
    pub scroll_behavior: ScrollBehavior,
    pub alignment: StreamAlign,
}

impl Default for StreamTetra {
    fn default() -> Self {
        Self {
            input_anchor: ScrollAnchor::Bottom,
            scroll_behavior: ScrollBehavior::AutoScroll,
            alignment: StreamAlign::Start,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TetraNode {
    Matrix(MatrixTetra), // MatrixTetra (Sidebar)
    Stream(StreamTetra), // Structuring Comms
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
            left_pane: TetraNode::Matrix(MatrixTetra::default()),
            right_pane: TetraNode::Stream(StreamTetra::default()),
            split_ratio: 0.25,
        }
    }
}