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
    /// The file currently open in the IDE/Editor.
    pub focused_file: Option<PathBuf>,
    /// Files currently being mutated in the UnaFS Write-Ahead Log.
    /// Key: File Path, Value: Intensity (0.0 - 1.0).
    pub wal_activity: HashMap<PathBuf, f32>,
    /// Files with recent changes in the Git Index.
    /// Key: File Path, Value: Intensity (0.0 - 1.0).
    pub git_activity: HashMap<PathBuf, f32>,
}

impl GravityWell {
    /// Initialize a new, empty Gravity Well.
    pub fn new() -> Self {
        Self {
            focused_file: None,
            wal_activity: HashMap::new(),
            git_activity: HashMap::new(),
        }
    }

    /// Update the primary focus point.
    pub fn set_focus(&mut self, path: PathBuf) {
        self.focused_file = Some(path);
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

            // 1. Primary Weight (1.0): The Focused File
            // If this file is what the user is looking at, it is paramount.
            if let Some(focus) = &self.focused_file {
                if path == focus {
                    score += 1.0;
                }
            }

            // 2. Secondary Weight (0.8): WAL Activity
            // If the user is typing in this file (even if not focused, e.g. multi-pane),
            // it is highly relevant.
            if let Some(val) = self.wal_activity.get(path) {
                score += 0.8 * val;
            }

            // 3. Tertiary Weight (0.5): Git Activity
            // If the file changed recently, it's likely part of the current task.
            if let Some(val) = self.git_activity.get(path) {
                score += 0.5 * val;
            }

            // We only care about things with SOME gravity.
            // A score of 0.0 means it's just background noise.
            if score > 0.0 {
                results.push(WeightedSkeleton {
                    path: path.clone(),
                    score,
                    content: content.clone(), // Zero-copy clone of the Arc
                });
            }
        }

        // Sort by score descending (Highest Gravity first).
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // The Can-Am Rule: Do not overwhelm the driver.
        // We only return the Top 5 most relevant items.
        results.into_iter().take(5).collect()
    }
}
