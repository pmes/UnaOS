// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Origin {
    LocalUser(String),
    Shard(String),
    System(String),
}

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
