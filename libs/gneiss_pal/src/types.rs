use gtk4::{TextBuffer, Adjustment, Widget, Window};
use std::path::PathBuf;
use crate::shard::{Shard, ShardStatus};
use std::sync::{Arc, Mutex};

// --- ELESSAR MUTATION (S40) ---
pub trait Spline: Send + Sync {
    fn bootstrap(&self, window: &Window) -> Widget;
    // We simplify handle_event to allow direct manipulation via interior mutability if needed,
    // or passing a channel sender. For now, basic signature.
    // However, since `AppHandler` is the main loop, Spline might need to hook into it.
    // Let's keep it simple: Bootstrap returns the Widget tree.
    // The Widget tree should contain its own signal handlers that communicate via the existing `Event` loop.
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WolfpackState {
    Idle,
    Dreaming,
    Fabricating,
}

#[derive(Debug, Clone)]
pub enum GuiUpdate {
    ShardStatusChanged { id: String, status: ShardStatus },
    ConsoleLog(String),
    SidebarStatus(WolfpackState), // The Pulse
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
    Input(String),
    TemplateAction(usize),
    NavSelect(usize),
    DockAction(usize),
    TextBufferUpdate(TextBuffer, Adjustment),
    UploadRequest, // Kept for compatibility
    FileSelected(PathBuf), // File Upload Selection
    ToggleSidebar,
    // --- ELESSAR EVENTS ---
    MatrixFileClick(PathBuf), // File Tree Click
    AuleIgnite, // Forge Action
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
