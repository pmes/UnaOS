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
