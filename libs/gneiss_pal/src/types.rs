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

use crate::shard::{Shard, ShardStatus};
use std::path::PathBuf;

// --- PURE LOGIC TYPES ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WolfpackState {
    Idle,
    Dreaming,
    Fabricating,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreFlightPayload {
    pub system: String,
    pub directives: String,
    pub engrams: String,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct HistoryItem {
    pub sender: String,
    pub content: String,
    pub timestamp: String,
    pub is_chat: bool,
}

#[derive(Debug, Clone)]
pub enum GuiUpdate {
    HistoryBatch(Vec<HistoryItem>),
    ShardStatusChanged { id: String, status: ShardStatus },
    ConsoleLog(String),
    ClearConsole,
    AppendInput(String),
    EditorLoad(String),
    SidebarStatus(WolfpackState), // The Pulse
    Spectrum(Vec<f32>),
    TokenUsage(i32, i32, i32), // Prompt, Candidates, Total
    ActiveDirective(String),
    ReviewPayload(PreFlightPayload), // The Interceptor
    SynapseError(String), // Discrete failure signal
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

#[derive(Debug)]
pub enum Event {
    Input {
        target: String,
        text: String,
    },
    TemplateAction(usize),
    NavSelect(usize),
    DockAction(usize),
    // REMOVED: TextBufferUpdate (GTK Dependency)
    UploadRequest,         // Kept for compatibility
    FileSelected(PathBuf), // File Upload Selection
    ToggleSidebar,
    LoadHistory,           // Fetch history when scrolling to top
    // --- ELESSAR EVENTS ---
    MatrixFileClick(PathBuf), // File Tree Click
    AuleIgnite,               // Forge Action
    Timer,                    // For heartbeat

    // --- VEIN EVENTS (Node Management) ---
    CreateNode {
        model: String,
        history: bool,
        temperature: f64,
        system_prompt: String,
    },
    NodeAction {
        action: String, // "exec", "arch", "debug", "una"
        active: bool,
    },
    ComplexInput {
        target: String,
        subject: String,
        body: String,
        point_break: bool,
        action: String, // "exec", "arch", "debug", "una"
    },
    ShardSelect(String),
    DispatchPayload(String), // The Interceptor
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

pub trait AppHandler: 'static {
    fn handle_event(&mut self, event: Event);
    fn view(&self) -> DashboardState;
}
