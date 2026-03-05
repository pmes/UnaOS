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

pub mod synapse;
pub mod telemetry;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
pub use synapse::Synapse;

/// WeightedSkeleton
///
/// A struct representing a scored, prioritized code skeleton.
/// This is the payload for the Context Telemetry stream.
///
/// It wraps the raw `content` in an `Arc<String>` to allow zero-copy
/// passing between threads (e.g., from the Vein Cortex thread to the GTK Main Loop).
///
/// Note: The `content` field is skipped during serialization because `Arc`
/// pointers are only valid within the same process address space.
/// For future inter-process telemetry, we will rely on `unafs` shared memory paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedSkeleton {
    /// The file path of the skeleton source.
    pub path: PathBuf,
    /// The calculated relevance score (Gravity Model).
    pub score: f32,
    /// The raw skeleton content (Arc-wrapped for zero-copy thread transfer).
    #[serde(skip)]
    pub content: Arc<String>,
}

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
