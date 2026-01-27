use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use directories::ProjectDirs;

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
        let proj_dirs = ProjectDirs::from("org", "unaos", "vein")
            .expect("Could not determine home directory");
        let data_dir = proj_dirs.data_dir();
        fs::create_dir_all(data_dir).expect("Failed to create data directory");

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
