// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use std::collections::{HashMap, VecDeque, HashSet};
use serde::{Deserialize, Serialize};
pub use crate::ontology::{Origin, Shard, ShardStatus, WeightedSkeleton};

pub const MAX_STATE_CAPACITY: usize = 1000;

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

#[derive(Debug, Clone)]
pub struct HistoryItem {
    pub origin: Origin,
    pub display_name: Option<String>,
    pub content: String,
    pub timestamp: String,
    pub is_chat: bool,
}

impl Default for HistoryItem {
    fn default() -> Self {
        Self {
            origin: Origin::System("UnaOS".to_string()),
            display_name: None,
            content: String::new(),
            timestamp: String::new(),
            is_chat: false,
        }
    }
}

/// DispatchRecord
/// Represents a semantic memory entry.
/// Shared between Vein and Amber Bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchRecord {
    pub id: String,
    pub origin: Origin,
    pub display_name: Option<String>,
    pub subject: String,
    pub timestamp: String,
    pub content: String,
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

    pub live_context: Vec<WeightedSkeleton>,

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

/// Backing state for an editable text pane (the "Editor" ViewEntity).
/// `path` is the file the buffer was loaded from (None = scratch buffer),
/// `content` the current buffer text, `language` a syntax hint (e.g. "rust",
/// "plaintext"). Serializable so it can ride the workspace snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditorState {
    pub path: Option<String>,
    pub content: String,
    pub language: String,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            path: None,
            content: String::new(),
            language: "plaintext".to_string(),
        }
    }
}

/// One log record as the Console viewer holds it: the same triple the
/// `SMessage::Log` producer message carries, plus a monotonic `seq` the
/// scrollback ring assigns on ingest so a view can anchor/dedupe without
/// comparing content. `level` is the severity tag (e.g. "info"/"warn"/"error"),
/// `source` the emitting subsystem (the facet axis), `content` the text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub seq: u64,
    pub level: String,
    pub source: String,
    pub content: String,
}

/// Which log stream the Console viewer shows. On HOST every record arrives as
/// an `SMessage::Log` on the bus, so a source is a FILTER over that one feed,
/// not a second transport. On METAL the same selector chooses the kernel
/// `TERM_RING` feed (see `unaos/crates/kernel/src/termring.rs`); the ring-3 app
/// is unchanged — only the ingest edge swaps. `All` = every record;
/// `Subsystem(name)` = only records whose `source` equals `name`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LogSource {
    All,
    Subsystem(String),
}

impl Default for LogSource {
    fn default() -> Self {
        LogSource::All
    }
}

/// Backing state for the Console log viewer — the macOS `Console.app`
/// equivalent, owned by the `comscan` handler. This is the DESIGNED render seam
/// for a future `ViewEntity::Console(LogViewState)`; it is a standalone struct
/// today because `ViewEntity` is matched exhaustively in the Qt/GTK tetra
/// bridge (`libs/quartzite/src/tetra.rs`), so wiring the variant belongs with
/// the vessel widget that renders it, not with this handler arc.
///
/// `lines` is the already-filtered scrollback snapshot the view draws;
/// `filter`/`source` are the active query; `paused` is scroll-lock (the tail is
/// frozen — the ring keeps ingesting, the view stops moving); `dropped` is the
/// count of records the bounded ring has evicted since boot, so a truncated
/// view is VISIBLY truncated (the same counted-loss honesty `termring` keeps).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LogViewState {
    pub lines: Vec<LogLine>,
    pub filter: String,
    pub source: LogSource,
    pub paused: bool,
    pub dropped: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ViewEntity {
    Topology(TopologyState),
    Stream(StreamState),
    Editor(EditorState),
    Empty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub left_pane: ViewEntity,
    pub right_pane: ViewEntity,
    pub split_ratio: f32,
    /// Optional bottom pane, stacked *under* the right pane in a vertical split.
    /// `None` = no bottom pane (the historical two-pane layout). `Some(_)` asks
    /// the platform backend to build the bottom **console** pane (read-only
    /// output + one-line input); the macOS `MacOSSpline` reads this at bootstrap.
    /// Reuses `ViewEntity` for forward-compatibility; today any `Some` requests
    /// the console.
    pub bottom_pane: Option<ViewEntity>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            left_pane: ViewEntity::Topology(TopologyState::default()),
            right_pane: ViewEntity::Stream(StreamState::default()),
            split_ratio: 0.25,
            bottom_pane: None,
        }
    }
}
