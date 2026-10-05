//! Persistent per-origin storage for `localStorage` (a JSON file per sanitized origin under the
//! platform data directory). The page-facing `localStorage` is the in-memory Web Storage of the
//! script platform prelude; this store is the persistence layer a later arc wires behind it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct LocalStorage {
    data: HashMap<String, String>,
    path: PathBuf,
}

impl LocalStorage {
    /// Opens (or creates empty) the store at `path`.
    pub fn new(path: PathBuf) -> Self {
        let data = fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();
        Self { data, path }
    }

    /// The store file for `origin` (sanitized to a file name) under the platform data directory.
    pub fn for_origin(origin: &str) -> Self {
        let sanitized = origin.replace(|c: char| !c.is_alphanumeric(), "_");
        let dir = directories::ProjectDirs::from("", "UnaOS", "Aether")
            .map(|p| p.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/tmp/aether_storage"));
        let _ = fs::create_dir_all(&dir);
        Self::new(dir.join(format!("{sanitized}.json")))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get_item(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(String::as_str)
    }

    pub fn set_item(&mut self, key: &str, value: &str) {
        self.data.insert(key.to_string(), value.to_string());
        self.save();
    }

    pub fn remove_item(&mut self, key: &str) {
        self.data.remove(key);
        self.save();
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.save();
    }

    fn save(&self) {
        if let Ok(content) = serde_json::to_string(&self.data) {
            let _ = fs::write(&self.path, content);
        }
    }
}
