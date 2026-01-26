use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Pile {
    pub created_at: SystemTime,
    pub entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub is_file: bool,
    pub size: u64,
    pub tags: Vec<String>,
}

impl Pile {
    pub fn new() -> Self {
        Self {
            created_at: SystemTime::now(),
            entries: Vec::new(),
        }
    }
}
