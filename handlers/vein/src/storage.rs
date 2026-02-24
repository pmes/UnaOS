use crate::model::DispatchRecord;
use anyhow::{Context, Result};
use gneiss_pal::paths::UnaPaths;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use unafs::{AttributeValue, FileDevice, FileSystem, UnaFS};
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
    ) -> Result<()> {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "type".to_string(),
            AttributeValue::String("chat".to_string()),
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

        Ok(())
    }

    pub fn search_memories(&mut self, embedding: &[f32], threshold: f32) -> Result<Vec<String>> {
        // Query syntax: similarity(embedding, [0.1,0.2,...]) > 0.7
        let vec_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let query_str = format!("similarity(embedding, {}) > {}", vec_str, threshold);

        let mut inodes = self
            .fs
            .query(&query_str)
            .map_err(|e| anyhow::anyhow!("Query failed: {:?}", e))?;

        // === THE NEUROSURGERY: ATTENTION SPAN ===
        // Sort by newest first, and strictly truncate to the top 3 results.
        // This permanently prevents 429 API Payload explosions.
        inodes.sort_by_key(|inode| std::cmp::Reverse(inode.id));
        inodes.truncate(3);

        let mut memories = Vec::new();

        for inode in inodes {
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

    pub fn load_all_memories(&mut self) -> Result<Vec<DispatchRecord>> {
        // Retrieve all chat memories for UI startup
        let query_str = "type == \"chat\"";

        let mut inodes = self
            .fs
            .query(query_str)
            .map_err(|e| anyhow::anyhow!("Query failed: {:?}", e))?;

        // Sort by ID (Creation Order)
        inodes.sort_by_key(|inode| inode.id);

        let mut records = Vec::new();
        for inode in inodes {
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
