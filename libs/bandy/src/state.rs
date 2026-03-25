// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use std::collections::{HashMap, VecDeque, HashSet};
use serde::{Deserialize, Serialize};

pub const MAX_STATE_CAPACITY: usize = 1000;

// --- SHARD DOMAIN ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShardRole {
    Root,    // Una-Prime (The Command Deck)
    Builder, // S9 (CI/CD)
    Storage, // The Mule (Big Data)
    Kernel,  // Hardware Debugging
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShardStatus {
    Online,   // Green
    OnCall,   // Teal
    Active,   // Seafoam
    Thinking, // Purple
    Paused,   // Yellow
    Error,    // Red
    Offline,  // Grey
}

#[derive(Debug, Clone)]
pub struct Shard {
    pub id: String,
    pub name: String,
    pub role: ShardRole,
    pub status: ShardStatus,
    pub cpu_load: u8, // Percentage 0-100
    pub children: Vec<Shard>,
}

#[derive(Debug, Clone)]
pub struct Heartbeat {
    pub id: String,
    pub status: ShardStatus,
    pub cpu_load: u8,
}

impl Shard {
    pub fn new(id: &str, name: &str, role: ShardRole) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            role,
            status: ShardStatus::Offline,
            cpu_load: 0,
            children: Vec::new(),
        }
    }
}


// --- PURE LOGIC TYPES ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WolfpackState {
    Idle,
    Dreaming,
    Fabricating,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PreFlightPayload {
    pub system: String,
    pub directives: String,
    pub engrams: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Default)]
pub struct HistoryItem {
    pub sender: String,
    pub content: String,
    pub timestamp: String,
    pub is_chat: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewMode {
    Comms,
    Wolfpack,
}

impl Default for ViewMode {
    fn default() -> Self {
        ViewMode::Comms
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SidebarPosition {
    Left,
    Right,
}

impl Default for SidebarPosition {
    fn default() -> Self {
        SidebarPosition::Right
    }
}

#[derive(Clone, Debug)]
pub struct DashboardState {
    pub mode: ViewMode,
    pub nav_items: Vec<String>,
    pub active_nav_index: usize,
    pub console_output: String,
    pub actions: Vec<String>,
    pub sidebar_position: SidebarPosition,
    pub dock_actions: Vec<String>,
    pub shard_tree: Vec<Shard>,
    pub sidebar_collapsed: bool,
}

impl Default for DashboardState {
    fn default() -> Self {
        DashboardState {
            mode: ViewMode::Comms,
            nav_items: Vec::new(),
            active_nav_index: 0,
            console_output: String::new(),
            actions: Vec::new(),
            sidebar_position: SidebarPosition::default(),
            dock_actions: Vec::new(),
            shard_tree: Vec::new(),
            sidebar_collapsed: false,
        }
    }
}

// --- THE CENTRAL NERVOUS SYSTEM STATE ---

#[derive(Debug, Clone)]
pub struct AppState {
    // The active timeline of thoughts/memories
    pub history: VecDeque<HistoryItem>,
    pub history_seq: usize,

    // Telemetry and diagnostics
    pub console_logs: VecDeque<String>,
    pub console_seq: usize,
    pub token_usage: (i32, i32, i32), // (Prompt, Response, Total)

    // UI Status Flags
    pub is_computing: bool,
    pub is_indexing: bool,

    // Current input state
    pub active_input_buffer: String,

    // Specific payloads previously in GuiUpdate
    pub active_directive: String,
    pub review_payload: Option<PreFlightPayload>,
    pub spectrum: Vec<f32>,
    pub sidebar_status: WolfpackState,
    pub editor_load: String,
    pub synapse_error: Option<String>,

    // Status mapping for Shards
    pub shard_statuses: HashMap<String, ShardStatus>,

    pub live_context: Vec<crate::WeightedSkeleton>,

    // The active spatial map (Matrix DAG topology)
    pub matrix_topology: String,

    // The JIT multi-selection list from the Matrix tree
    pub active_matrix_selection: Vec<String>,

    // The absolute workspace root anchor (J21 "Pathfinder" Directive)
    // Cached immutably and passed by reference to achieve zero-latency resolution
    pub absolute_workspace_root: std::sync::Arc<std::path::PathBuf>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            history: VecDeque::new(),
            history_seq: 0,
            console_logs: VecDeque::new(),
            console_seq: 0,
            token_usage: (0, 0, 0),
            is_computing: false,
            is_indexing: false,
            active_input_buffer: String::new(),
            active_directive: String::new(),
            review_payload: None,
            spectrum: Vec::new(),
            sidebar_status: WolfpackState::Idle,
            editor_load: String::new(),
            synapse_error: None,
            shard_statuses: HashMap::new(),
            live_context: Vec::new(),
            matrix_topology: String::new(),
            active_matrix_selection: Vec::new(),
            absolute_workspace_root: std::sync::Arc::new(
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            ),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopologyNode {
    pub id: String,
    pub label: String,
    pub children: Vec<TopologyNode>,
    pub is_expanded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExpandableList {
    pub roots: Vec<TopologyNode>,
}

impl ExpandableList {
    pub fn flatten(&self) -> Vec<(&TopologyNode, usize)> {
        let mut result = Vec::new();
        for root in &self.roots {
            self.flatten_recursive(root, 0, &mut result);
        }
        result
    }

    fn flatten_recursive<'a>(&'a self, node: &'a TopologyNode, depth: usize, result: &mut Vec<(&'a TopologyNode, usize)>) {
        result.push((node, depth));
        if node.is_expanded {
            for child in &node.children {
                self.flatten_recursive(child, depth + 1, result);
            }
        }
    }

    pub fn toggle_node(&mut self, node_id: &str) -> bool {
        for root in &mut self.roots {
            if Self::toggle_node_recursive(root, node_id) {
                return true;
            }
        }
        false
    }

    fn toggle_node_recursive(node: &mut TopologyNode, node_id: &str) -> bool {
        if node.id == node_id {
            node.is_expanded = !node.is_expanded;
            return true;
        }

        for child in &mut node.children {
            if Self::toggle_node_recursive(child, node_id) {
                return true;
            }
        }

        false
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SelectionState {
    pub selected_ids: HashSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopologyState {
    pub tree: ExpandableList,
    pub selection: SelectionState,
}

impl Default for TopologyState {
    fn default() -> Self {
        let tree = ExpandableList {
            roots: vec![
                TopologyNode {
                    id: "unaos_core".to_string(),
                    label: "UnaOS Core".to_string(),
                    is_expanded: true,
                    children: vec![
                        TopologyNode {
                            id: "kernel".to_string(),
                            label: "Kernel".to_string(),
                            is_expanded: false,
                            children: vec![],
                        },
                        TopologyNode {
                            id: "dmz".to_string(),
                            label: "DMZ".to_string(),
                            is_expanded: false,
                            children: vec![],
                        },
                    ],
                },
                TopologyNode {
                    id: "embassies".to_string(),
                    label: "Embassies".to_string(),
                    is_expanded: false,
                    children: vec![
                        TopologyNode {
                            id: "gtk".to_string(),
                            label: "GTK".to_string(),
                            is_expanded: false,
                            children: vec![],
                        },
                        TopologyNode {
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

impl TopologyState {
    pub fn new(roots: Vec<TopologyNode>) -> Self {
        Self {
            tree: ExpandableList { roots },
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
pub struct StreamState {
    pub input_anchor: ScrollAnchor,
    pub scroll_behavior: ScrollBehavior,
    pub alignment: StreamAlign,
}

impl Default for StreamState {
    fn default() -> Self {
        Self {
            input_anchor: ScrollAnchor::Bottom,
            scroll_behavior: ScrollBehavior::AutoScroll,
            alignment: StreamAlign::Start,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ViewEntity {
    Topology(TopologyState),
    Stream(StreamState),
    Empty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub left_pane: ViewEntity,
    pub right_pane: ViewEntity,
    pub split_ratio: f32,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            left_pane: ViewEntity::Topology(TopologyState::default()),
            right_pane: ViewEntity::Stream(StreamState::default()),
            split_ratio: 0.25,
        }
    }
}
