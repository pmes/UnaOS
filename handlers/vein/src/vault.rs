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

//! The Semantic Vault — vein's durable engram store.
//!
//! CHARTER PROVENANCE: this is vein's own storage seam. Jules commit `3839cff`
//! (2026-03-11) extracted this DiskManager/durable-memory actor OUT of vein and
//! bolted it onto `amber_bytes`, derailing that handler from its "The Block"
//! forensic-recovery charter. The AMBER-CHARTER arc undoes that: the engram
//! save/query actor returns HOME to vein; amber_bytes recovers The Block.
//!
//! The fail-closed mount guard (AMBER-GUARD) moved with the actor and is
//! non-negotiable: an existing vault that cannot be mounted is left byte-
//! identical on disk for recovery — never truncated, never reformatted.

use anyhow::{Context, Result};
use bandy::state::DispatchRecord;
use bandy::{SMessage, Synapse};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::task;
use unafs::{AttributeValue, FileSystem, UnaFS, FileDevice};

/// The DiskManager is the synchronous guardian of the Semantic Vault.
///
/// ARCHITECTURAL NOTE (THE CAN-AM RULE):
/// This struct is strictly synchronous. It performs heavy, blocking I/O via UnaFS.
/// It MUST NEVER be called directly on the Tokio async reactor thread.
pub struct DiskManager {
    pub fs: FileSystem,
}

impl DiskManager {
    /// Open the vault at `path`: mount it if the file already exists, or
    /// create and format a fresh vault on true first run (no file present).
    ///
    /// FAIL-CLOSED GUARANTEE: if the vault file already exists but cannot be
    /// mounted (corruption, version skew, transient I/O), this returns the
    /// error and leaves the on-disk bytes untouched for recovery. It never
    /// truncates or reformats an existing file.
    pub fn new(path: &Path) -> Result<Self> {
        if path.exists() {
            // Existing vault: mount it or fail closed. Do NOT reformat.
            let device = FileDevice::open(path)
                .with_context(|| format!("failed to open existing vault at {}", path.display()))?;
            let fs = UnaFS::mount(device).with_context(|| {
                format!(
                    "refusing to reformat: existing vault at {} failed to mount \
                     (its bytes are left untouched for recovery)",
                    path.display()
                )
            })?;
            Ok(Self { fs })
        } else {
            // True first run: create and format a fresh vault. `create_new`
            // guarantees this can never truncate a file that appeared after
            // the exists() check above.
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .with_context(|| format!("failed to create fresh vault at {}", path.display()))?
                .set_len(64 * 1024 * 1024)?;
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

    pub fn load_paged_memories(&mut self, offset: usize, limit: usize) -> Result<Vec<DispatchRecord>> {
        // Retrieve all chat memories for UI startup
        let query_str = "type == \"chat\"";

        let mut inodes = self
            .fs
            .query(query_str)
            .map_err(|e| anyhow::anyhow!("Query failed: {:?}", e))?;

        // 1. Sort DESCENDING (newest first) to establish the pagination baseline
        inodes.sort_by_key(|(inode, _)| std::cmp::Reverse(inode.id));

        // 2. Slice the page
        let mut paged_inodes: Vec<_> = inodes.into_iter().skip(offset).take(limit).collect();

        // 3. Re-sort ASCENDING so the UI receives them in proper chronological order
        paged_inodes.sort_by_key(|(inode, _)| inode.id);

        let mut records = Vec::new();
        for (inode, _) in paged_inodes {
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

            let origin = if sender == "Architect" {
                bandy::ontology::Origin::LocalUser(sender.clone())
            } else if sender == "System" || sender == "UnaOS" {
                bandy::ontology::Origin::System(sender.clone())
            } else {
                bandy::ontology::Origin::Shard(sender.clone())
            };
            records.push(DispatchRecord {
                id: inode.id.to_string(),
                origin,
                display_name: Some(sender),
                subject: "Memory".to_string(),
                timestamp,
                content,
                is_chat: true,
            });
        }

        Ok(records)
    }
}

/// Ignite the Semantic Vault Storage Rune.
/// This Rune takes absolute and exclusive ownership of the UnaFS DiskManager.
/// It listens to the Synapse for incoming storage requests, executes the bare-metal I/O,
/// and fires the results back into the nervous system.
pub async fn ignite(vault_path: PathBuf, synapse: Synapse) {
    let mut rx = synapse.subscribe();
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
                ":: VEIN VAULT :: Fatal error: failed to mount UnaFS vault: {}",
                e
            );
            return;
        }
    };

    println!(
        ":: VEIN VAULT :: Storage Rune online, holding exclusive lock on Vault at {:?}",
        vault_path
    );

    // The Actor Loop
    loop {
        match rx.recv().await {
            Ok(msg) => match msg {
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
                SMessage::StorageLoadPaged { receipt_id, offset, limit } => {
                    let mut dm = disk_manager;
                    let (dm_returned, result) = task::spawn_blocking(move || {
                        let res = dm.load_paged_memories(offset, limit).unwrap_or_default();
                        (dm, res)
                    })
                    .await
                    .unwrap();

                    disk_manager = dm_returned;

                    synapse_clone
                        .fire_async(SMessage::StorageLoadPagedResult {
                            receipt_id,
                            records: result,
                        })
                        .await;
                }
                _ => {} // Ignore other messages
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                eprintln!("Vein Vault receiver lagged, dropping missed events.");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                println!(":: VEIN VAULT :: Synapse channel closed, terminating loop.");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const VAULT_LEN: u64 = 64 * 1024 * 1024;

    /// Fresh path: no file at the vault path -> a new vault is created,
    /// formatted, and immediately usable for writes and queries.
    #[test]
    fn fresh_path_creates_usable_vault() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("vault.unafs");
        assert!(!vault.exists());

        let mut dm = DiskManager::new(&vault).expect("first run must create a fresh vault");
        assert_eq!(fs::metadata(&vault).expect("vault file").len(), VAULT_LEN);

        dm.save_memory(
            "Architect",
            "first light",
            "2026-07-13T00:00:00Z",
            vec![0.5; 4],
            "engram",
        )
        .expect("fresh vault must accept writes");
        let engrams = dm
            .get_latest_engrams(1)
            .expect("fresh vault must answer queries");
        assert_eq!(engrams, vec!["first light".to_string()]);
    }

    /// Guard path: an existing file that fails to mount must return an error
    /// AND remain byte-identical on disk — never truncated, never reformatted.
    #[test]
    fn mount_failure_preserves_existing_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("vault.unafs");

        // Garbage large enough to reach the superblock parse (>= 1 block).
        let garbage: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
        fs::write(&vault, &garbage).expect("seed garbage vault");

        let result = DiskManager::new(&vault);
        assert!(
            result.is_err(),
            "mounting a corrupt existing vault must fail closed"
        );

        let after = fs::read(&vault).expect("vault file must still exist");
        assert_eq!(
            after, garbage,
            "existing vault bytes must be byte-identical after a failed mount"
        );
    }

    /// Guard path, sub-block variant: an existing file shorter than one block
    /// used to be silently reformatted by the old size gate; it must now fail
    /// closed with its bytes untouched.
    #[test]
    fn short_existing_file_is_not_reformatted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("vault.unafs");

        let stub = b"not a vault".to_vec();
        fs::write(&vault, &stub).expect("seed stub file");

        let result = DiskManager::new(&vault);
        assert!(
            result.is_err(),
            "an existing sub-block file must fail closed, not be reformatted"
        );

        let after = fs::read(&vault).expect("vault file must still exist");
        assert_eq!(
            after, stub,
            "stub bytes must be untouched after a failed mount"
        );
    }

    /// Happy reopen: a valid vault written by one DiskManager reopens via
    /// DiskManager::new with its data intact.
    #[test]
    fn valid_vault_reopens_with_data_intact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("vault.unafs");

        {
            let mut dm = DiskManager::new(&vault).expect("create fresh vault");
            dm.save_memory(
                "Architect",
                "remember me",
                "2026-07-13T00:00:00Z",
                vec![0.25; 4],
                "engram",
            )
            .expect("write memory");
        }

        let mut dm = DiskManager::new(&vault).expect("valid existing vault must remount");
        let engrams = dm
            .get_latest_engrams(1)
            .expect("reopened vault must answer queries");
        assert_eq!(engrams, vec!["remember me".to_string()]);
    }
}
