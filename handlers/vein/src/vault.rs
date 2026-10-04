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

/// EMBED (B317): the model that made a memory's vector (`<provider>/<model>`).
pub const ATTR_EMBED_MODEL: &str = "una:embed-model";
/// EMBED (B317): that vector's width.
pub const ATTR_EMBED_DIMS: &str = "una:embed-dims";
/// The memory types the vault holds vectors for.
pub const MEMORY_TYPES: [&str; 3] = ["chat", "directive", "engram"];

/// The `una:embed-model` tag on an inode, if it has one.
pub fn embed_tag(inode: &unafs::Inode) -> Option<&str> {
    match inode.attributes.get(ATTR_EMBED_MODEL) {
        Some(AttributeValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

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
        embed_model: &str,
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

        // EMBED (B317): a vector is written only when there is one (recall off writes none), and it
        // carries the model that made it so vectors from different models never compare.
        self.write_vector(inode_id, embedding, embed_model)?;

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

    /// EMBED (B317): store `embedding` + `una:embed-model` + `una:embed-dims` on `inode_id`.
    /// An empty vector writes nothing.
    pub fn write_vector(&mut self, inode_id: u64, embedding: Vec<f32>, embed_model: &str) -> Result<()> {
        if embedding.is_empty() {
            return Ok(());
        }
        let dims = embedding.len() as i64;
        // Save embedding separately to handle potentially large attributes safely
        self.fs
            .set_attribute(inode_id, "embedding".to_string(), AttributeValue::Vector(embedding))
            .context("Failed to save embedding")?;
        self.fs
            .set_attribute(inode_id, ATTR_EMBED_MODEL.to_string(), AttributeValue::String(embed_model.to_string()))
            .context("Failed to save embed model")?;
        self.fs
            .set_attribute(inode_id, ATTR_EMBED_DIMS.to_string(), AttributeValue::Int(dims))
            .context("Failed to save embed dims")?;
        Ok(())
    }

    /// EMBED (B317): every memory (chat, directive, engram) whose vector was not made by
    /// `embed_model` — tagged with another model, untagged (pre-B317), or stored with no vector.
    /// Ascending `(inode id, data size)`.
    pub fn stale_memories(&mut self, embed_model: &str) -> Result<Vec<(u64, u64)>> {
        let mut ids = Vec::new();
        for t in MEMORY_TYPES {
            let hits = self
                .fs
                .query_inodes(&format!("type == \"{t}\""))
                .map_err(|e| anyhow::anyhow!("Query failed: {:?}", e))?;
            ids.extend(hits.into_iter().filter(|(i, _)| embed_tag(i) != Some(embed_model)).map(|(i, _)| (i.id, i.size)));
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    /// EMBED (B317): up to `limit` stale memories with their content, and the stale total.
    pub fn reembed_batch(&mut self, embed_model: &str, limit: usize) -> Result<(Vec<(u64, String)>, usize)> {
        let stale = self.stale_memories(embed_model)?;
        let total = stale.len();
        let items = stale
            .into_iter()
            .take(limit)
            .map(|(id, size)| (id, String::from_utf8_lossy(&self.fs.read_data(id, 0, size).unwrap_or_default()).into_owned()))
            .collect();
        Ok((items, total))
    }

    /// EMBED (B317): write a re-embed batch; answers `(written, still stale)`.
    pub fn write_vectors(&mut self, embed_model: &str, vectors: Vec<(u64, Vec<f32>)>) -> Result<(usize, usize)> {
        let mut written = 0;
        for (id, v) in vectors {
            if v.is_empty() {
                continue;
            }
            self.write_vector(id, v, embed_model)?;
            written += 1;
        }
        Ok((written, self.stale_memories(embed_model)?.len()))
    }

    pub fn search_memories(
        &mut self,
        embedding: &[f32],
        threshold: f32,
        memory_type: &str,
        embed_model: &str,
    ) -> Result<Vec<String>> {
        // EMBED (B317): no vector, no comparison (recall off).
        if embedding.is_empty() {
            return Ok(Vec::new());
        }
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
            .query_inodes(&query_str)
            .map_err(|e| anyhow::anyhow!("Query failed: {:?}", e))?;

        // EMBED (B317): only vectors made by the same model compare. An empty tag (an older sender)
        // keeps the pre-B317 behaviour.
        if !embed_model.is_empty() {
            inodes.retain(|(inode, _)| embed_tag(inode) == Some(embed_model));
        }

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
            .query_inodes(query_str)
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
            .query_inodes(query_str)
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
                    embed_model,
                } => {
                    let mut dm = disk_manager;
                    let emb = embedding.clone();
                    let (dm_returned, result) = task::spawn_blocking(move || {
                        let m = embed_model.as_str();
                        let chat_mem = dm.search_memories(&emb, 0.45, "chat", m).unwrap_or_default();
                        let directive_mem = dm
                            .search_memories(&emb, 0.45, "directive", m)
                            .unwrap_or_default();
                        let engram_mem =
                            dm.search_memories(&emb, 0.45, "engram", m).unwrap_or_default();
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
                    embed_model,
                } => {
                    let mut dm = disk_manager;
                    let (dm_returned, result) = task::spawn_blocking(move || {
                        let res = dm.save_memory(&sender, &content, &timestamp, embedding, &memory_type, &embed_model);
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
                // EMBED (B317): the re-embed walk — the vault hands out bounded batches of memories
                // whose vector was not made by the configured model, and writes the new vectors back.
                SMessage::ReEmbed { receipt_id, embed_model, limit } => {
                    let mut dm = disk_manager;
                    let (dm_returned, result) = task::spawn_blocking(move || {
                        let res = dm.reembed_batch(&embed_model, limit);
                        (dm, res)
                    })
                    .await
                    .unwrap();
                    disk_manager = dm_returned;
                    let (items, stale_total) = result.unwrap_or_default();
                    synapse_clone.fire_async(SMessage::ReEmbedBatch { receipt_id, items, stale_total }).await;
                }
                SMessage::ReEmbedWrite { receipt_id, embed_model, vectors } => {
                    let mut dm = disk_manager;
                    let (dm_returned, result) = task::spawn_blocking(move || {
                        let res = dm.write_vectors(&embed_model, vectors);
                        (dm, res)
                    })
                    .await
                    .unwrap();
                    disk_manager = dm_returned;
                    let (written, remaining, error) = match result {
                        Ok((w, r)) => (w, r, None),
                        Err(e) => (0, 0, Some(e.to_string())),
                    };
                    synapse_clone.fire_async(SMessage::ReEmbedDone { receipt_id, written, remaining, error }).await;
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
            "test/model",
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
                "test/model",
            )
            .expect("write memory");
        }

        let mut dm = DiskManager::new(&vault).expect("valid existing vault must remount");
        let engrams = dm
            .get_latest_engrams(1)
            .expect("reopened vault must answer queries");
        assert_eq!(engrams, vec!["remember me".to_string()]);
    }

    /// EMBED (B317): the model tag is stored beside each vector, vectors from another model never
    /// compare, a memory stored with recall off has no vector, and the re-embed walk finds and fixes
    /// both — bounded per batch.
    #[test]
    fn embed_model_tags_gate_recall_and_drive_reembed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut dm = DiskManager::new(&dir.path().join("vault.unafs")).expect("vault");
        let a = vec![1.0, 0.0, 0.0, 0.0];
        dm.save_memory("user", "alpha", "t", a.clone(), "chat", "local/m1").unwrap();
        dm.save_memory("user", "beta", "t", a.clone(), "chat", "gemini/m2").unwrap();
        dm.save_memory("user", "gamma", "t", Vec::new(), "chat", "").unwrap();

        // Tags on disk.
        let hits = dm.fs.query_inodes("type == \"chat\"").unwrap();
        let alpha = hits.iter().find(|(i, _)| i.size == 5).map(|(i, _)| i.clone()).unwrap();
        assert_eq!(embed_tag(&alpha), Some("local/m1"));
        assert_eq!(alpha.attributes.get(ATTR_EMBED_DIMS), Some(&AttributeValue::Int(4)));
        let gamma = hits.iter().find(|(i, _)| embed_tag(i).is_none()).map(|(i, _)| i.clone()).unwrap();
        assert!(!gamma.attributes.contains_key("embedding") && !gamma.large_attributes.contains_key("embedding"));

        // Only same-model vectors compare.
        assert_eq!(dm.search_memories(&a, 0.45, "chat", "local/m1").unwrap(), vec!["[user]: alpha".to_string()]);
        assert_eq!(dm.search_memories(&a, 0.45, "chat", "gemini/m2").unwrap(), vec!["[user]: beta".to_string()]);
        assert!(dm.search_memories(&[], 0.45, "chat", "local/m1").unwrap().is_empty());

        // Stale under local/m1: beta (other model) and gamma (no vector).
        let (batch, total) = dm.reembed_batch("local/m1", 1).unwrap();
        assert_eq!((batch.len(), total), (1, 2));
        assert_eq!(batch[0].1, "beta");
        let (written, remaining) = dm.write_vectors("local/m1", vec![(batch[0].0, vec![0.0, 1.0, 0.0, 0.0])]).unwrap();
        assert_eq!((written, remaining), (1, 1));
        let (batch, total) = dm.reembed_batch("local/m1", 8).unwrap();
        assert_eq!((batch.len(), total, batch[0].1.as_str()), (1, 1, "gamma"));
        let (_, remaining) = dm.write_vectors("local/m1", vec![(batch[0].0, vec![0.0, 0.0, 1.0, 0.0])]).unwrap();
        assert_eq!(remaining, 0);
        assert_eq!(dm.search_memories(&a, 0.45, "chat", "local/m1").unwrap().len(), 1);
        assert_eq!(dm.search_memories(&[0.0, 0.0, 1.0, 0.0], 0.45, "chat", "local/m1").unwrap(), vec!["[user]: gamma".to_string()]);
    }
}
