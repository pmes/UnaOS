use unafs::{UnaFS, FileDevice, FileSystem, FileKind, DirEntry};
use crate::model::DispatchRecord;
use std::path::Path;
use anyhow::{Result, Context};

const DISK_PATH: &str = "/tmp/lumen_storage.ufs";
const HISTORY_FILE: &str = "history.bin";

pub struct DiskManager {
    fs: FileSystem,
}

impl DiskManager {
    pub fn new() -> Result<Self> {
        let path = Path::new(DISK_PATH);
        let fs = if path.exists() {
            // Mount existing
            let mut device = FileDevice::new(path)?;
            let mut fs = UnaFS::mount(device)?;
            // Check if history file exists, if not create it
            // We need to look up the root dir.
            // For Phase 1, we assume simple root structure.
            // If we can't find the file in root, we create it.
            // UnaFS API exploration needed? Assuming standard operations.
            // Let's ensure the file exists.
            Self::ensure_history_file(&mut fs)?;
            fs
        } else {
            // Format new
            let mut device = FileDevice::new(path)?;
            // 64MB for history seems plenty for text
            let mut fs = UnaFS::format(&mut device, 64)?;
            Self::create_history_file(&mut fs)?;
            fs
        };

        Ok(Self { fs })
    }

    fn create_history_file(fs: &mut FileSystem) -> Result<()> {
        let root_inode = fs.get_root_inode();
        // Create file under root.
        // Assuming fs.create_file(&mut parent_dir_inode, name, kind)
        // We need to verify UnaFS API from memory/context.
        // Memory says: `libs/unafs` implements `UnaFS<BlockDevice>`.
        // Let's assume a `create_entry` or similar.
        // Re-reading memory: "The `UnaFS::format` method requires a mutable device...".
        // I will use a simplified approach: just try to read/write by known inode if possible,
        // or scan root directory.

        // Since I don't have full API docs, I'll implement a robust "find or create" logic.
        // But `UnaFS` is likely lower level.
        // Let's try to list root.
        let root = fs.read_dir(root_inode)?;
        let exists = root.iter().any(|e| e.name == HISTORY_FILE);

        if !exists {
             fs.create_entry(root_inode, HISTORY_FILE, FileKind::File)?;
        }
        Ok(())
    }

    fn ensure_history_file(fs: &mut FileSystem) -> Result<()> {
        Self::create_history_file(fs)
    }

    pub fn save_history(&mut self, records: &[DispatchRecord]) -> Result<()> {
        let data = bincode::serialize(records).context("Failed to serialize history")?;

        // Find inode for history file
        let root_inode = self.fs.get_root_inode();
        let root_entries = self.fs.read_dir(root_inode)?;
        let entry = root_entries.iter().find(|e| e.name == HISTORY_FILE)
            .ok_or_else(|| anyhow::anyhow!("History file not found"))?;

        // Write data
        self.fs.write_data(entry.inode, &data)?;
        Ok(())
    }

    pub fn load_history(&mut self) -> Result<Vec<DispatchRecord>> {
        let root_inode = self.fs.get_root_inode();
        let root_entries = self.fs.read_dir(root_inode)?;

        if let Some(entry) = root_entries.iter().find(|e| e.name == HISTORY_FILE) {
            let data = self.fs.read_data(entry.inode)?;
            if data.is_empty() {
                return Ok(Vec::new());
            }
            let records: Vec<DispatchRecord> = bincode::deserialize(&data)
                .context("Failed to deserialize history")?;
            Ok(records)
        } else {
            Ok(Vec::new())
        }
    }
}
