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

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedMessage {
    pub role: String, // "user" or "model"
    pub content: String,
    pub timestamp: Option<String>,
}

#[derive(Clone)]
pub struct BrainManager {
    file_path: PathBuf,
}

impl BrainManager {
    pub fn new(file_path: PathBuf) -> Self {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create data directory");
        }

        Self { file_path }
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

    pub fn get_active_directive(&self) -> String {
        // Try to read 'directive.txt' in the same folder
        if let Some(parent) = self.file_path.parent() {
            let path = parent.join("directive.txt");
            if let Ok(content) = fs::read_to_string(path) {
                return content.trim().to_string();
            }
        }
        "Directive 055".to_string() // Default as per mission
    }
}
