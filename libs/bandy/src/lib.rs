pub mod synapse;
pub mod telemetry;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use synapse::Synapse;

/// SMessage (The Shard Message).
/// The atomic unit of truth in UnaOS.
/// This Enum defines the limits of what can be said between processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SMessage {
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

    // --- UNAFS / MATRIX (The Memory) ---
    FileEvent {
        path: String,
        event: String,
    },

    // --- MIDDEN (The Terminal) ---
    NoOp,
    TerminalOutput(String),
    TerminalError(String),
    FileSystemEvent(String),

    // --- PRINCIPIA (The Basal Ganglia) ---
    Principia(PrincipiaCommand),

    // --- MATRIX (The Spatial Cortex) ---
    Matrix(MatrixEvent),
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
        nodes: Vec<SpatialNode>,
        edges: Vec<SpatialEdge>,
    },
    /// Vein asks Matrix to focus on a specific sector (e.g., "euclase")
    FocusSector(String),
    /// Matrix returns the raw context of that sector
    SectorFocused { target: String, context: String },
    /// Matrix UI fires this when a spatial node is activated
    NodeSelected(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialNode {
    pub id: String,
    pub kind: String, // "crate", "struct", "fn"
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialEdge {
    pub from: String,
    pub to: String,
    pub relation: String, // "imports", "implements", "calls"
}

/// The trait that defines a "Nerve Ending" in the system.
pub trait BandyMember {
    fn publish(&self, topic: &str, msg: SMessage) -> anyhow::Result<()>;
}
