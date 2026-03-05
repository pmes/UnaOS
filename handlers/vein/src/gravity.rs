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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use bandy::WeightedSkeleton;

/// GravityWell
///
/// The Context Gravity Model.
/// This struct is responsible for calculating the "Gravitational Pull" of every file
/// in the workspace based on the user's current actions.
///
/// It aggregates signals from:
/// 1. The Focused File (Primary Weight)
/// 2. The Write-Ahead Log (Secondary Weight)
/// 3. Git Activity (Tertiary Weight)
pub struct GravityWell {
    pub focused_file: Option<PathBuf>,
    pub wal_activity: HashMap<PathBuf, f32>,
    pub git_activity: HashMap<PathBuf, f32>,
    pub prompt_keywords: Vec<String>, // <-- NEW
}

impl GravityWell {
    pub fn new() -> Self {
        Self {
            focused_file: None,
            wal_activity: HashMap::new(),
            git_activity: HashMap::new(),
            prompt_keywords: Vec::new(),
        }
    }

    pub fn set_focus(&mut self, path: PathBuf) {
        self.focused_file = Some(path);
    }

    // --- NEW: EXTRACT SEMANTIC KEYWORDS FROM PROMPT ---
    pub fn extract_keywords(&mut self, prompt: &str) {
        let stop_words = ["this", "that", "with", "from", "your", "what", "when", "where", "have", "will", "just", "like", "need", "make", "sure", "code", "file"];
        self.prompt_keywords = prompt
            // Split by non-alphanumeric EXCEPT underscores (preserves snake_case like telemetry_tx)
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| w.len() > 3) // Ignore tiny words
            .map(|w| w.to_lowercase())
            .filter(|w| !stop_words.contains(&w.as_str()))
            .collect();
    }

    /// Mark a file as active in the Write-Ahead Log.
    pub fn mark_wal_activity(&mut self, path: PathBuf) {
        // For now, we set max intensity.
        // In a future iteration, this could decay over time.
        self.wal_activity.insert(path, 1.0);
    }

    /// Mark a file as having recent git changes.
    pub fn mark_git_activity(&mut self, path: PathBuf) {
        self.git_activity.insert(path, 1.0);
    }

    /// Calculate the Gravitational Score for all known skeletons.
    /// Returns a sorted list of the most relevant skeletons.
    pub fn calculate_scores(&self, skeletons: &HashMap<PathBuf, Arc<String>>) -> Vec<WeightedSkeleton> {
        let mut results = Vec::new();

        for (path, content) in skeletons {
            let mut score = 0.0;
            let path_str = path.to_string_lossy().to_lowercase();

            // 1. Primary Weight
            if let Some(focus) = &self.focused_file {
                if path == focus { score += 1.0; }
            }

            // 2. Secondary Weights
            if let Some(val) = self.wal_activity.get(path) { score += 0.8 * val; }
            if let Some(val) = self.git_activity.get(path) { score += 0.5 * val; }

            // 3. NEW: Semantic Prompt Weight
            if !self.prompt_keywords.is_empty() {
                let content_lower = content.to_lowercase();
                for kw in &self.prompt_keywords {
                    if path_str.contains(kw) {
                        score += 0.6; // High gravity if the filename matches
                    } else if content_lower.contains(kw) {
                        score += 0.3; // Medium gravity if the code contains the keyword
                    }
                }
            }

            if score > 0.0 {
                results.push(WeightedSkeleton {
                    path: path.clone(),
                    score,
                    content: content.clone(),
                });
            }
        }

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.into_iter().take(5).collect()
    }
}
