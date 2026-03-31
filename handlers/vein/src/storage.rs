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

// use crate::model::DispatchRecord; // <-- EXCISED
use anyhow::{Context, Result};
use gneiss_pal::paths::UnaPaths;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use unafs::{AttributeValue, FileDevice, FileSystem, UnaFS};

// Define a local struct for memory loading since we can't depend on UI models
pub struct DispatchRecord {
    pub id: String,
    pub sender: String,
    pub subject: String,
    pub timestamp: String,
    pub content: String,
    pub is_chat: bool,
}

pub struct CortexStorage {
    base_dir: PathBuf,
}

impl CortexStorage {
    /// Initializes the Cortex Storage.
    /// It inherently trusts the Plexus Abstraction Layer.
    pub fn new() -> Self {
        let base_dir = UnaPaths::cortex();

        // Ensure our specific lobes exist.
        fs::create_dir_all(base_dir.join("models")).expect("Failed to form model lobe");
        fs::create_dir_all(base_dir.join("memories")).expect("Failed to form memory lobe");

        Self { base_dir }
    }

    #[inline]
    pub fn model_path(&self, model_name: &str) -> PathBuf {
        self.base_dir.join("models").join(model_name)
    }

    #[inline]
    pub fn memory_db(&self) -> PathBuf {
        self.base_dir.join("memories").join("vector.db")
    }
}

/// The DiskManager is the synchronous guardian of the Semantic Vault.
///
/// ARCHITECTURAL NOTE (THE CAN-AM RULE):
/// This struct is strictly synchronous. It performs heavy, blocking I/O via UnaFS.
/// It MUST NEVER be called directly on the Tokio async reactor thread.
/// The caller (e.g., `lib.rs`) must wrap instances of this in `Arc<std::sync::Mutex<DiskManager>>`
/// and offload all method calls to `tokio::task::spawn_blocking`.
pub struct DiskManager {
    pub fs: FileSystem,
}

impl DiskManager {
    pub fn new(path: &Path) -> Result<Self> {
        // Only mount if the file is at least 1 block long (4096 bytes)
        let is_valid_disk =
            path.exists() && std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) >= 4096;

        if is_valid_disk {
            let device = FileDevice::open(path)?;
            match UnaFS::mount(device) {
                Ok(fs) => Ok(Self { fs }),
                Err(e) => {
                    eprintln!(":: LIBRARIAN :: Mount failed ({}), reformatting...", e);
                    std::fs::File::create(path)?.set_len(64 * 1024 * 1024)?;
                    let device = FileDevice::open(path)?;
                    let fs = UnaFS::format(device, 64)?;
                    Ok(Self { fs })
                }
            }
        } else {
            std::fs::File::create(path)?.set_len(64 * 1024 * 1024)?;
            let device = FileDevice::open(path)?;
            let fs = UnaFS::format(device, 64)?;
            Ok(Self { fs })
        }
    }

    pub fn save_memory(
        &mut self,
        sender: &str,
        content: &str,
        timestamp: &str,
        embedding: Vec<f32>,
        memory_type: &str,
    ) -> Result<()> {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "type".to_string(),
            AttributeValue::String(memory_type.to_string()),
        );
        attrs.insert(
            "sender".to_string(),
            AttributeValue::String(sender.to_string()),
        );
        attrs.insert(
            "timestamp".to_string(),
            AttributeValue::String(timestamp.to_string()),
        );

        let inode_id = self
            .fs
            .create_inode(attrs)
            .context("Failed to create inode")?;
        self.fs
            .write_data(inode_id, 0, content.as_bytes())
            .context("Failed to write content")?;

        // Save embedding separately to handle potentially large attributes safely
        self.fs
            .set_attribute(
                inode_id,
                "embedding".to_string(),
                AttributeValue::Vector(embedding),
            )
            .context("Failed to save embedding")?;

        // CRITICAL FIX: The `create_inode` call does not update the catalog.
        // We MUST explicitly call `set_attribute` on "type" so the query engine
        // can find these records during `load_all_memories`.
        self.fs
            .set_attribute(
                inode_id,
                "type".to_string(),
                AttributeValue::String(memory_type.to_string()),
            )
            .context("Failed to catalog memory type")?;

        Ok(())
    }

    pub fn search_memories(&mut self, embedding: &[f32], threshold: f32, memory_type: &str) -> Result<Vec<String>> {
        // Query syntax: similarity(embedding, [0.1,0.2,...]) > 0.7
        let vec_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let query_str = format!("similarity(embedding, {}) > {} AND type == \"{}\"", vec_str, threshold, memory_type);

        let mut inodes = self
            .fs
            .query(&query_str)
            .map_err(|e| anyhow::anyhow!("Query failed: {:?}", e))?;

        // === THE NEUROSURGERY: ATTENTION SPAN ===
        // Sort by pure vector gravity (descending)
        // This permanently prevents 429 API Payload explosions.
        inodes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        inodes.truncate(3);

        let mut memories = Vec::new();

        for (inode, _) in inodes {
            let data = self
                .fs
                .read_data(inode.id, 0, inode.size)
                .unwrap_or_default();
            let content = String::from_utf8(data).unwrap_or_default();

            let sender = match inode.attributes.get("sender") {
                Some(AttributeValue::String(s)) => s.as_str(),
                _ => "Unknown",
            };

            // Format: [Sender]: Content
            memories.push(format!("[{}]: {}", sender, content));
        }

        Ok(memories)
    }

    pub fn get_latest_engrams(&mut self, limit: usize) -> Result<Vec<String>> {
        let query_str = "type == \"engram\"";

        let mut inodes = self
            .fs
            .query(query_str)
            .map_err(|e| anyhow::anyhow!("Query failed: {:?}", e))?;

        // Sort by ID descending (newest first)
        inodes.sort_by_key(|(inode, _)| std::cmp::Reverse(inode.id));
        inodes.truncate(limit);

        let mut memories = Vec::new();
        for (inode, _) in inodes {
            let data = self
                .fs
                .read_data(inode.id, 0, inode.size)
                .unwrap_or_default();
            let content = String::from_utf8(data).unwrap_or_default();
            memories.push(content);
        }

        Ok(memories)
    }

    pub fn load_all_memories(&mut self) -> Result<Vec<DispatchRecord>> {
        // Retrieve all chat memories for UI startup
        let query_str = "type == \"chat\"";

        let mut inodes = self
            .fs
            .query(query_str)
            .map_err(|e| anyhow::anyhow!("Query failed: {:?}", e))?;

        // Sort by ID (Creation Order)
        inodes.sort_by_key(|(inode, _)| inode.id);

        let mut records = Vec::new();
        for (inode, _) in inodes {
            let data = self
                .fs
                .read_data(inode.id, 0, inode.size)
                .unwrap_or_default();
            let content = String::from_utf8(data).unwrap_or_default();

            let sender = match inode.attributes.get("sender") {
                Some(AttributeValue::String(s)) => s.clone(),
                _ => "System".to_string(),
            };

            let timestamp = match inode.attributes.get("timestamp") {
                Some(AttributeValue::String(s)) => s.clone(),
                _ => "".to_string(),
            };

            records.push(DispatchRecord {
                id: inode.id.to_string(),
                sender,
                subject: "Memory".to_string(),
                timestamp,
                content,
                is_chat: true,
            });
        }

        Ok(records)
    }
}
