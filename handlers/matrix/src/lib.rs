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

use bandy::{MatrixEvent, SMessage, Synapse};
use std::path::{Path, PathBuf};

// True DAG Lexical Scanner
// J21 PATHFINDER: Explicitly replacing the J37 flat scanner with a true lexical topology engine.
// DO NOT DELETE: This powers the core Spatial Code Map (Matrix DAG).
use std::collections::HashMap;

pub struct MatrixScanner;

impl MatrixScanner {

    pub fn build_genesis_tree(dir: &Path, absolute_root: &Path) -> Vec<bandy::state::TopologyNode> {
        let mut nodes = Vec::new();

        let Ok(entries) = std::fs::read_dir(dir) else {
            return nodes;
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();

                if file_name == "target" || file_name == ".git" || file_name == "node_modules" {
                    continue;
                }

                if path.is_dir() {
                    dirs.push((path, file_name));
                } else {
                    files.push((path, file_name));
                }
            }
        }

        dirs.sort_by(|a, b| a.1.cmp(&b.1));
        files.sort_by(|a, b| a.1.cmp(&b.1));

        for (path, file_name) in dirs {
            let relative_path = path.strip_prefix(absolute_root).unwrap_or(&path).to_path_buf();
            let id = relative_path.to_string_lossy().into_owned();
            let children = Self::build_genesis_tree(&path, absolute_root);
            nodes.push(bandy::state::TopologyNode {
                id,
                label: file_name,
                children,
                is_expanded: false,
            });
        }

        for (path, file_name) in files {
            let relative_path = path.strip_prefix(absolute_root).unwrap_or(&path).to_path_buf();
            let id = relative_path.to_string_lossy().into_owned();
            nodes.push(bandy::state::TopologyNode {
                id,
                label: file_name,
                children: Vec::new(),
                is_expanded: false,
            });
        }

        nodes
    }

    /// J21 PATHFINDER: Core method for the Zero-Redundancy Indexed Dictionary DAG Scanner.
    pub fn map_topology(paths: &[std::path::PathBuf], absolute_workspace_root: &Path) -> Result<String, String> {
        // Dictionary Engine
        let mut dict_map: HashMap<String, usize> = HashMap::new();
        let mut dict_list: Vec<String> = Vec::new();

        // Edge connections: "NodeID:DepID,DepID|NodeID:DepID"
        let mut topology_edges: Vec<String> = Vec::new();

        let mut processed_any = false;

        for path in paths {
            if path.is_file() {
                Self::scan_file(path, absolute_workspace_root, &mut dict_map, &mut dict_list, &mut topology_edges);
                processed_any = true;
            } else if path.is_dir() {
                Self::scan_directory(path, absolute_workspace_root, &mut dict_map, &mut dict_list, &mut topology_edges);
                processed_any = true;
            } else {
                log::warn!("[MATRIX] Target is neither a file nor a directory: {:?}", path);
            }
        }

        if !processed_any {
            return Err("No valid targets were provided.".to_string());
        }

        // AI-Readable Serialization Format (`DICTIONARY$TOPOLOGY`)
        let dict_str = dict_list.join(",");
        let edges_str = topology_edges.join("|");

        let compressed_payload = format!("{}${}", dict_str, edges_str);
        Ok(compressed_payload)
    }

    fn get_or_insert_id(token: &str, dict_map: &mut HashMap<String, usize>, dict_list: &mut Vec<String>) -> usize {
        if let Some(&id) = dict_map.get(token) {
            id
        } else {
            let id = dict_list.len();
            dict_map.insert(token.to_string(), id);
            dict_list.push(token.to_string());
            id
        }
    }

    fn scan_directory(
        dir: &Path,
        absolute_workspace_root: &Path,
        dict_map: &mut HashMap<String, usize>,
        dict_list: &mut Vec<String>,
        topology_edges: &mut Vec<String>,
    ) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    Self::scan_directory(&p, absolute_workspace_root, dict_map, dict_list, topology_edges);
                } else if p.is_file() {
                    Self::scan_file(&p, absolute_workspace_root, dict_map, dict_list, topology_edges);
                }
            }
        }
    }

    fn strip_comments(content: &str) -> String {
        let mut result = String::with_capacity(content.len());
        let mut chars = content.chars().peekable();

        let mut in_line_comment = false;
        let mut in_block_comment = false;

        while let Some(c) = chars.next() {
            if in_line_comment {
                if c == '\n' {
                    in_line_comment = false;
                    result.push('\n');
                }
                continue;
            }
            if in_block_comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next(); // Consume '/'
                    in_block_comment = false;
                }
                continue;
            }

            if c == '/' {
                if let Some(&next_c) = chars.peek() {
                    if next_c == '/' {
                        chars.next(); // Consume second '/'
                        in_line_comment = true;
                        continue;
                    } else if next_c == '*' {
                        chars.next(); // Consume '*'
                        in_block_comment = true;
                        continue;
                    }
                }
            }

            result.push(c);
        }

        result
    }

    fn scan_file(
        file_path: &Path,
        absolute_workspace_root: &Path,
        dict_map: &mut HashMap<String, usize>,
        dict_list: &mut Vec<String>,
        topology_edges: &mut Vec<String>,
    ) {
        if file_path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }

        let relative_path = file_path.strip_prefix(absolute_workspace_root).unwrap_or(file_path).to_path_buf();
        let node_name = relative_path.to_string_lossy().into_owned();
        let node_id = Self::get_or_insert_id(&node_name, dict_map, dict_list);

        if let Ok(raw_contents) = std::fs::read_to_string(file_path) {
            // Lexical Extraction: Strip comments and replace multi-line breaks
            let no_comments = Self::strip_comments(&raw_contents);
            let single_line_content = no_comments.replace('\n', " ").replace('\r', " ");

            let mut local_deps = Vec::new();

            // Very simple tokenization by semicolons
            let statements = single_line_content.split(';');

            for stmt in statements {
                let trimmed = stmt.trim();

                if trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if let Some(mod_name) = parts.last() {
                        let clean_name = mod_name.trim();
                        let dep_id = Self::get_or_insert_id(clean_name, dict_map, dict_list);
                        local_deps.push(dep_id);
                    }
                }

                if trimmed.starts_with("use ") || trimmed.starts_with("pub use ") {
                    // Find the start of the path
                    let prefix_len = if trimmed.starts_with("use ") { 4 } else { 8 };
                    let path_str = &trimmed[prefix_len..].trim();

                    // Handle brackets: std::sync::{Arc, Mutex}
                    if let Some(brace_start) = path_str.find('{') {
                        if let Some(brace_end) = path_str.find('}') {
                            let base_path = &path_str[..brace_start].trim_end_matches("::");
                            let inside_braces = &path_str[brace_start + 1..brace_end];

                            for item in inside_braces.split(',') {
                                let item = item.trim();
                                if !item.is_empty() {
                                    let full_path = if base_path.is_empty() {
                                        item.to_string()
                                    } else {
                                        format!("{}::{}", base_path, item)
                                    };
                                    let dep_id = Self::get_or_insert_id(&full_path, dict_map, dict_list);
                                    local_deps.push(dep_id);
                                }
                            }
                        }
                    } else {
                        // Standard single use
                        let dep_id = Self::get_or_insert_id(path_str, dict_map, dict_list);
                        local_deps.push(dep_id);
                    }
                }
            }

            if !local_deps.is_empty() {
                let dep_strs: Vec<String> = local_deps.iter().map(|id| id.to_string()).collect();
                topology_edges.push(format!("{}:{}", node_id, dep_strs.join(",")));
            }
        }
    }
}



/// The Asynchronous Logic Kernel for the Matrix
pub async fn ignite(synapse: Synapse, absolute_workspace_root: std::sync::Arc<PathBuf>) {
    let mut rx = synapse.subscribe();
    println!("[MATRIX] Spatial Anchor Established via Brain Loop: {:?}", absolute_workspace_root);

    loop {
        match rx.recv().await {
            Ok(SMessage::Matrix(MatrixEvent::FocusSector(relative_targets_str))) => {
                println!("[MATRIX] Analyzing Sectors: {}", relative_targets_str);

                // J21 PATHFINDER: Enable Multi-Sector Bundling
                // Split the incoming space-separated targets and map them to absolute paths.
                let absolute_targets: Vec<std::path::PathBuf> = relative_targets_str
                    .split_whitespace()
                    .map(|target| absolute_workspace_root.join(target))
                    .collect();

                if let Ok(compressed_payload) = MatrixScanner::map_topology(&absolute_targets, &absolute_workspace_root) {
                    // J21 PATHFINDER: Fire the True DAG directly to `vein` via `IngestTopology`.
                    // This raw data structure fuels the instant UI payload mutation.
                    let _ = synapse.fire_async(SMessage::Matrix(MatrixEvent::IngestTopology { payload: compressed_payload })).await;
                }
            }
            Ok(_) => {}
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("lagged") {
                    log::warn!("[MATRIX] Event loop lagging: {}", err_msg);
                } else {
                    log::info!("[MATRIX] Synapse channel closed or error. Terminating loop.");
                    break;
                }
            }
        }
    }
}
