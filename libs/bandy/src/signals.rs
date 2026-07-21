// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::ontology::WeightedSkeleton;
use crate::state::DispatchRecord;

/// SMessage (The Shard Message).
/// The atomic unit of truth in UnaOS.
/// This Enum defines the limits of what can be said between processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SMessage {
    StateInvalidated,
    // --- SYSTEM HEARTBEAT ---
    Ping,
    Kill(String),
    Log {
        level: String,
        source: String,
        content: String,
    },
    /// One beat of the system monitor: per-core CPU load fractions
    /// (`0.0..=1.0`, one entry per core). Fired by the `pulse` vessel's
    /// sampler seam (`PulseSource`); a future UnaOS-kernel telemetry feed
    /// replaces the *source*, not this message.
    CorePulse {
        loads: Vec<f32>,
    },

    // --- EUCLASE (The Visual Cortex) ---
    EuclaseResize(u32, u32),
    VugPulse,

    // --- RESONANCE (The Voice) ---
    AudioChunk {
        source_id: String,
        samples: Vec<f32>,
        sample_rate: u32,
    },
    Spectrum {
        magnitude: Vec<f32>,
    },

    // --- VEIN / LUMEN (The Mind) ---
    UserPrompt(String),
    AiToken(String),
    AnalyzeContext {
        id: String,
        content: String,
    },
    NetworkLog(String),
    NetworkState(String),
    // Vaire / Git Integration
    GetDiff {
        commit_a: String,
        commit_b: String,
    },
    DiffPayload {
        diff: String,
    },
    // Context Telemetry (Lumen HUD)
    /// NON-WIRE EXCEPTION: `WeightedSkeleton.content` is `#[serde(skip)]` by
    /// design (in-process `Arc<String>` only — see
    /// [`crate::ontology::WeightedSkeleton`]). This variant serializes, but
    /// LOSSILY: the skeleton content is dropped on serialize and comes back
    /// `Default`-empty on deserialize, so it never crosses a process boundary.
    /// Inter-process telemetry is deferred to `unafs` shared memory. Frozen by
    /// the `context_telemetry_*` proofs in `tests/smessage_kats.rs`.
    ContextTelemetry {
        skeletons: Vec<WeightedSkeleton>,
    },

    // --- UNAFS / MATRIX (The Memory) ---
    FileEvent {
        path: String,
        event: String,
    },

    // --- AMBER BYTES (The Storage Rune) ---
    StorageQuery {
        receipt_id: u64,
        embedding: Vec<f32>,
    },
    StorageQueryResult {
        receipt_id: u64,
        memories: Vec<String>,
        directives: Vec<String>,
        engrams: Vec<String>,
        chrono: Vec<String>,
    },
    StorageSave {
        receipt_id: u64,
        sender: String,
        content: String,
        timestamp: String,
        embedding: Vec<f32>,
        memory_type: String,
    },
    StorageSaveResult {
        receipt_id: u64,
        success: bool,
        error: Option<String>,
    },
    StorageLoadPaged {
        receipt_id: u64,
        offset: usize,
        limit: usize,
    },
    StorageLoadPagedResult {
        receipt_id: u64,
        records: Vec<DispatchRecord>,
    },

    // --- AETHER (The Browser) ---
    OpenDocument { url: String },
    SurfaceBlit { url: String, width: u32, height: u32, pixels: Vec<u8> },

    // --- EDITOR (The Code Pane) ---
    /// Load a document into the active editor pane. Fired when a file is
    /// selected for editing; the macOS `MacOSSpline` router pushes `content`
    /// into the editor `NSTextView` via `setString`. `path`/`language` let the
    /// view label + (later) syntax-highlight the buffer.
    EditorLoad {
        path: Option<String>,
        content: String,
        language: String,
    },
    /// View → brain: the editor buffer changed (fired by the macOS
    /// `EditorDelegate`'s `textDidChange`). Carries the full current buffer so
    /// the brain can hold the live document without a separate read-back.
    EditorEdited {
        content: String,
    },
    /// View → brain: the user asked to save the active editor buffer (Cmd+S /
    /// menu Save). The brain owns the actual write (path + persistence); this is
    /// just the request signal.
    EditorSaveRequest,

    // --- CONSOLE (The Bottom Pane) ---
    /// Brain → view: append one line to the read-only console output pane.
    ConsoleAppend(String),
    /// View → brain: the user submitted a line in the console input field
    /// (Enter). The brain routes/executes it (e.g. into `midden`).
    ConsoleInput(String),

    // --- MIDDEN (The Terminal) ---
    NoOp,
    TerminalOutput(String),
    TerminalError(String),
    FileSystemEvent(String),
    TriggerUpload(PathBuf),

    // --- PRINCIPIA (The Basal Ganglia) ---
    Principia(PrincipiaCommand),

    // --- MATRIX (The Spatial Cortex) ---
    Matrix(MatrixEvent),

    // --- UI EVENTS (Migrated from gneiss_pal::types::Event) ---
    Input {
        target: String,
        text: String,
    },
    TemplateAction(usize),
    NavSelect(usize),
    DockAction(usize),
    UploadRequest,
    FileSelected(PathBuf),
    ToggleSidebar,
    LoadHistory { offset: usize },
    UpdateMatrixSelection(Vec<String>),
    MatrixFileClick(PathBuf),
    AuleIgnite,
    Timer,
    CreateNode {
        model: String,
        history: bool,
        temperature: f64,
        system_prompt: String,
    },
    NodeAction {
        action: String,
        active: bool,
    },
    ComplexInput {
        target: String,
        subject: String,
        body: String,
        point_break: bool,
        action: String,
    },
    ShardSelect(String),
    DispatchPayload(String),
    ToggleMatrixNode(String),
    UiReady,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrincipiaCommand {
    SetSystemRoot(PathBuf),
    SystemRootChanged(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MatrixEvent {
    /// Matrix broadcasts the entire topological map of the OS
    IngestTopology {
        ui_dag: String,
        semantic_dag: String,
    },
    /// Surgically appends extracted symbols to an existing node's children
    GraftTopology {
        target_id: String,
        payload: String,
    },
    /// Vein asks Matrix to focus on a specific sector (e.g., "euclase")
    FocusSector(String),
    /// Matrix returns the raw context of that sector
    SectorFocused { target: String, context: String },
    /// Matrix UI fires this when a spatial node is activated
    NodeSelected(PathBuf),
    /// Broadcasts an updated, flattened structural topology back to the UI
    TopologyMutated(Vec<(String, String, usize)>),
}

/// The trait that defines a "Nerve Ending" in the system.
pub trait BandyMember {
    fn publish(&self, topic: &str, msg: SMessage) -> anyhow::Result<()>;
}
