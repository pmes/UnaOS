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
            let mut device = FileDevice::open(path)?;
            let mut fs = UnaFS::mount(device)?;
            Self::ensure_history_file(&mut fs)?;
            fs
        } else {
            // Format new
            let mut device = FileDevice::open(path)?;
            let mut fs = UnaFS::format(&mut device, 64)?;
            Self::create_history_file(&mut fs)?;
            fs
        };

        Ok(Self { fs })
    }

    fn create_history_file(fs: &mut FileSystem) -> Result<()> {
        let root_inode = fs.superblock.root_inode;

        let root = fs.ls(root_inode)?;
        let exists = root.iter().any(|e| e.name == HISTORY_FILE);

        if !exists {
             fs.create_file(root_inode, HISTORY_FILE.to_string())?;
        }
        Ok(())
    }

    fn ensure_history_file(fs: &mut FileSystem) -> Result<()> {
        Self::create_history_file(fs)
    }

    pub fn save_history(&mut self, records: &[DispatchRecord]) -> Result<()> {
        let data = bincode::serialize(records).context("Failed to serialize history")?;

        // Find inode for history file
        let root_inode = self.fs.superblock.root_inode;
        let root_entries = self.fs.ls(root_inode)?;
        let entry = root_entries.iter().find(|e| e.name == HISTORY_FILE)
            .ok_or_else(|| anyhow::anyhow!("History file not found"))?;

        // Write data
        self.fs.write_data(entry.inode, 0, &data)?;
        Ok(())
    }

    pub fn load_history(&mut self) -> Result<Vec<DispatchRecord>> {
        let root_inode = self.fs.superblock.root_inode;
        let root_entries = self.fs.ls(root_inode)?;

        if let Some(entry) = root_entries.iter().find(|e| e.name == HISTORY_FILE) {
            let inode_obj = self.fs.read_inode(entry.inode)?;
            if inode_obj.size == 0 {
                return Ok(Vec::new());
            }
            let data = self.fs.read_data(entry.inode, 0, inode_obj.size)?;
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
