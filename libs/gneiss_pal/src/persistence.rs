use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedMessage {
    pub role: String, // "user" or "model"
    pub content: String,
}

#[derive(Clone)]
pub struct BrainManager {
    file_path: PathBuf,
}

impl BrainManager {
    pub fn new() -> Self {
        // MANUAL OVERRIDE: Force ~/.local/share/unaos/vein
        // We ask for the base data directory (usually ~/.local/share)
        // and manually append our specific hierarchy.
        let base_dirs = BaseDirs::new().expect("Could not determine base directories");

        let data_dir = base_dirs
            .data_local_dir()
            .join("unaos") // The Organization
            .join("vein"); // The App

        // Create the directory tree
        fs::create_dir_all(&data_dir).expect("Failed to create data directory");

        Self {
            file_path: data_dir.join("history.json"),
        }
    }

    pub fn save(&self, messages: &[SavedMessage]) {
        if let Ok(json) = serde_json::to_string_pretty(messages) {
            let _ = fs::write(&self.file_path, json);
        }
    }

    pub fn load(&self) -> Vec<SavedMessage> {
        if !self.file_path.exists() {
            return vec![];
        }
        if let Ok(data) = fs::read_to_string(&self.file_path) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            vec![]
        }
    }
}
