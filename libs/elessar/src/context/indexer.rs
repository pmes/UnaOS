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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

// We don't need unafs here. We just need standard paths.
// The indexer builds the graph of crates.

/// A lightweight representation of a crate in the workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateNode {
    pub name: String,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

/// The Workspace Indexer.
///
/// It scans the workspace for `Cargo.toml` files and builds a DAG of dependencies.
pub struct WorkspaceIndexer {
    pub nodes: HashMap<String, CrateNode>,
}

impl WorkspaceIndexer {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    /// Recursively scans a directory for crates.
    pub fn scan(&mut self, root: &Path) {
        let mut visited = HashSet::new();
        self.scan_recursive(root, &mut visited);
    }

    fn scan_recursive(&mut self, dir: &Path, visited: &mut HashSet<PathBuf>) {
        if visited.contains(dir) {
            return;
        }
        visited.insert(dir.to_path_buf());

        // Check if this is a crate (has Cargo.toml)
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                if let Some(node) = self.parse_cargo_toml(&content, dir) {
                    self.nodes.insert(node.name.clone(), node);
                }
            }
        }

        // Recurse into subdirectories
        // Avoid target, .git, node_modules
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name == "target" || name == ".git" || name == "node_modules" {
                        continue;
                    }
                    self.scan_recursive(&path, visited);
                }
            }
        }
    }

    fn parse_cargo_toml(&self, content: &str, path: &Path) -> Option<CrateNode> {
        // We do a naive parse to avoid pulling in toml crate dependency if we want strict minimalism,
        // BUT the user approved toml/parsing logic implicitly via "DAG/RAG Indexer".
        // Wait, I didn't add `toml` crate to Cargo.toml.
        // Can I do it with string matching? Yes, Can-Am style.
        // Or I can add `toml` if needed.
        // Let's try string matching for efficiency and zero deps.

        let mut name = String::new();
        let mut dependencies = Vec::new();
        let mut in_package = false;
        let mut in_deps = false;

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("[package]") {
                in_package = true;
                in_deps = false;
            } else if line.starts_with("[dependencies]") {
                in_package = false;
                in_deps = true;
            } else if line.starts_with("[") {
                in_package = false;
                in_deps = false;
            }

            if in_package && line.starts_with("name =") {
                if let Some(val) = line.split('=').nth(1) {
                    name = val.trim().trim_matches('"').to_string();
                }
            }

            if in_deps && !line.starts_with("#") && !line.is_empty() {
                // dep = ...
                if let Some(dep_name) = line.split('=').nth(0) {
                     let clean = dep_name.trim();
                     if !clean.is_empty() {
                         dependencies.push(clean.to_string());
                     }
                }
            }
        }

        if !name.is_empty() {
            Some(CrateNode {
                name,
                path: path.to_path_buf(),
                dependencies,
            })
        } else {
            None
        }
    }
}
