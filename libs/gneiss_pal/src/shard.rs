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

// libs/gneiss_pal/src/shard.rs
use serde::{Deserialize, Serialize};

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
