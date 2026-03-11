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

use anyhow::{Context, Result};
use bandy::{DispatchRecord, SMessage, Synapse};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::task;
use unafs::{AttributeValue, FileDevice, FileSystem, UnaFS};

/// The DiskManager is the synchronous guardian of the Semantic Vault.
///
/// ARCHITECTURAL NOTE (THE CAN-AM RULE):
/// This struct is strictly synchronous. It performs heavy, blocking I/O via UnaFS.
/// It MUST NEVER be called directly on the Tokio async reactor thread.
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

    pub fn search_memories(
        &mut self,
        embedding: &[f32],
        threshold: f32,
        memory_type: &str,
    ) -> Result<Vec<String>> {
        // Query syntax: similarity(embedding, [0.1,0.2,...]) > 0.7
        let vec_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let query_str = format!(
            "similarity(embedding, {}) > {} AND type == \"{}\"",
            vec_str, threshold, memory_type
        );

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

/// Ignite the Amber Bytes Storage Rune.
/// This Rune takes absolute and exclusive ownership of the UnaFS DiskManager.
/// It listens to the Synapse for incoming storage requests, executes the bare-metal I/O,
/// and fires the results back into the nervous system.
pub async fn ignite(vault_path: PathBuf, synapse: Synapse) {
    let rx = synapse.rx();
    let synapse_clone = synapse.clone();

    // Use spawn_blocking for initial mount to keep the reactor happy
    let vault_path_clone = vault_path.clone();
    let disk_manager_result = task::spawn_blocking(move || DiskManager::new(&vault_path_clone))
        .await
        .unwrap();

    let mut disk_manager = match disk_manager_result {
        Ok(dm) => dm,
        Err(e) => {
            eprintln!(
                ":: AMBER BYTES :: Fatal error: failed to mount UnaFS vault: {}",
                e
            );
            return;
        }
    };

    println!(
        ":: AMBER BYTES :: Storage Rune online, holding exclusive lock on Vault at {:?}",
        vault_path
    );

    // The Actor Loop
    loop {
        if let Ok(msg) = rx.recv().await {
            match msg {
                SMessage::StorageQuery {
                    receipt_id,
                    embedding,
                } => {
                    let mut dm = disk_manager;
                    let emb = embedding.clone();
                    let (dm_returned, result) = task::spawn_blocking(move || {
                        let chat_mem = dm.search_memories(&emb, 0.45, "chat").unwrap_or_default();
                        let directive_mem = dm
                            .search_memories(&emb, 0.45, "directive")
                            .unwrap_or_default();
                        let engram_mem =
                            dm.search_memories(&emb, 0.45, "engram").unwrap_or_default();
                        let chrono_mem = dm.get_latest_engrams(2).unwrap_or_default();
                        (dm, (chat_mem, directive_mem, engram_mem, chrono_mem))
                    })
                    .await
                    .unwrap();

                    disk_manager = dm_returned;
                    let (chat_mem, directive_mem, engram_mem, chrono_mem) = result;

                    synapse_clone
                        .fire_async(SMessage::StorageQueryResult {
                            receipt_id,
                            memories: chat_mem,
                            directives: directive_mem,
                            engrams: engram_mem,
                            chrono: chrono_mem,
                        })
                        .await;
                }
                SMessage::StorageSave {
                    receipt_id,
                    sender,
                    content,
                    timestamp,
                    embedding,
                    memory_type,
                } => {
                    let mut dm = disk_manager;
                    let (dm_returned, result) = task::spawn_blocking(move || {
                        let res =
                            dm.save_memory(&sender, &content, &timestamp, embedding, &memory_type);
                        (dm, res)
                    })
                    .await
                    .unwrap();

                    disk_manager = dm_returned;

                    match result {
                        Ok(_) => {
                            synapse_clone
                                .fire_async(SMessage::StorageSaveResult {
                                    receipt_id,
                                    success: true,
                                    error: None,
                                })
                                .await;
                        }
                        Err(e) => {
                            synapse_clone
                                .fire_async(SMessage::StorageSaveResult {
                                    receipt_id,
                                    success: false,
                                    error: Some(e.to_string()),
                                })
                                .await;
                        }
                    }
                }
                SMessage::StorageLoadAll { receipt_id } => {
                    let mut dm = disk_manager;
                    let (dm_returned, result) = task::spawn_blocking(move || {
                        let res = dm.load_all_memories().unwrap_or_default();
                        (dm, res)
                    })
                    .await
                    .unwrap();

                    disk_manager = dm_returned;

                    synapse_clone
                        .fire_async(SMessage::StorageLoadAllResult {
                            receipt_id,
                            records: result,
                        })
                        .await;
                }
                _ => {} // Ignore other messages
            }
        }
    }
}
