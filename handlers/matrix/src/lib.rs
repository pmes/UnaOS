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

pub mod indexer;

pub enum ScanDepth {
    Interface,
    DeepAST,
}

pub struct MatrixScanner;

impl MatrixScanner {

    pub fn build_genesis_tree(dir: &Path, absolute_root: &Path) -> Vec<bandy::state::TopologyNode> {
        let mut nodes = Vec::new();

        let Ok(entries) = std::fs::read_dir(dir) else {
            return nodes;
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        // 1. First Pass: Collect valid files and calculate children for directories.
        // We only want to map spatial code logic. Configuration files and other noise are dropped.
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();

                if file_name == "target" || file_name == ".git" || file_name == "node_modules" {
                    continue;
                }

                if path.is_dir() {
                    // Recursively process the directory first to see if it holds any logic.
                    let children = Self::build_genesis_tree(&path, absolute_root);
                    // A branch with no leaves is dead weight. Prune it.
                    if !children.is_empty() {
                        dirs.push((path, file_name, children));
                    }
                } else if path.is_file() {
                    // J24.8 "Phil": Strictly isolate .rs files. Non-code files must vanish.
                    if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                        files.push((path, file_name));
                    }
                }
            }
        }

        // 2. Deterministic Sorting: Directories first, then Files (alphabetically).
        dirs.sort_by(|a, b| a.1.cmp(&b.1));
        files.sort_by(|a, b| a.1.cmp(&b.1));

        // 3. Construct the TopologyNodes.
        for (path, file_name, children) in dirs {
            let relative_path = path.strip_prefix(absolute_root).unwrap_or(&path).to_path_buf();
            let id = relative_path.to_string_lossy().into_owned();
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
    pub fn map_topology(
        paths: &[std::path::PathBuf],
        absolute_workspace_root: &Path,
        depth: ScanDepth,
    ) -> Result<(String, String), String> {
        // Dictionary Engine
        let mut dict_map: HashMap<String, usize> = HashMap::new();
        let mut dict_list: Vec<String> = Vec::new();

        // Edge connections: "NodeID:DepID,DepID|NodeID:DepID"
        let mut topology_edges: Vec<String> = Vec::new();

        let mut processed_any = false;

        let is_single_file = paths.len() == 1 && paths[0].is_file();

        for path in paths {
            if path.is_file() {
                Self::scan_file(
                    path,
                    absolute_workspace_root,
                    &mut dict_map,
                    &mut dict_list,
                    &mut topology_edges,
                    &depth,
                    is_single_file,
                );
                processed_any = true;
            } else if path.is_dir() {
                Self::scan_directory(
                    path,
                    absolute_workspace_root,
                    &mut dict_map,
                    &mut dict_list,
                    &mut topology_edges,
                    &depth,
                );
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

        // Semantic code topology logic
        let mut semantic_dag = String::from("--- SEMANTIC CODE TOPOLOGY ---\n");
        let mut edges_map: HashMap<usize, Vec<usize>> = HashMap::new();

        for edge in &topology_edges {
            if let Some((node_str, deps_str)) = edge.split_once(':') {
                if let Ok(node_id) = node_str.parse::<usize>() {
                    let deps: Vec<usize> = deps_str
                        .split(',')
                        .filter_map(|d| d.parse::<usize>().ok())
                        .collect();
                    edges_map.insert(node_id, deps);
                }
            }
        }

        for (id, node_name) in dict_list.iter().enumerate() {
            if let Some(deps) = edges_map.get(&id) {
                if !deps.is_empty() {
                    let dep_names: Vec<String> = deps.iter().map(|&d_id| {
                        dict_list.get(d_id).unwrap_or(&d_id.to_string()).clone()
                    }).collect();
                    semantic_dag.push_str(&format!("[{}] relies on: {}\n", node_name, dep_names.join(", ")));
                } else {
                    semantic_dag.push_str(&format!("[{}] operates independently.\n", node_name));
                }
            } else {
                semantic_dag.push_str(&format!("[{}] operates independently.\n", node_name));
            }
        }

        Ok((compressed_payload, semantic_dag))
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
        depth: &ScanDepth,
    ) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    Self::scan_directory(&p, absolute_workspace_root, dict_map, dict_list, topology_edges, depth);
                } else if p.is_file() {
                    Self::scan_file(&p, absolute_workspace_root, dict_map, dict_list, topology_edges, depth, false);
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

    fn expand_use_path(path: &str, results: &mut Vec<String>) {
        let path = path.trim();
        if path.is_empty() {
            return;
        }

        // Fast path for simple non-bracketed imports.
        if !path.contains('{') {
            // Remove " as alias" if present.
            let clean_path = if let Some(as_idx) = path.find(" as ") {
                path[..as_idx].trim()
            } else {
                path
            };
            if !clean_path.is_empty() {
                results.push(clean_path.to_string());
            }
            return;
        }

        let mut stack = Vec::new();
        let mut current_prefix = String::new();
        let mut current_token = String::new();

        let mut chars = path.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' => {
                    let prefix = current_token.trim().trim_end_matches("::").trim();
                    if !prefix.is_empty() {
                        let full_prefix = if current_prefix.is_empty() {
                            prefix.to_string()
                        } else {
                            format!("{}::{}", current_prefix, prefix)
                        };
                        stack.push(current_prefix.clone());
                        current_prefix = full_prefix;
                    } else {
                        stack.push(current_prefix.clone());
                    }
                    current_token.clear();
                }
                '}' => {
                    let token = current_token.trim();
                    if !token.is_empty() {
                        let clean_token = if let Some(as_idx) = token.find(" as ") {
                            token[..as_idx].trim()
                        } else {
                            token
                        };

                        let full_path = if clean_token == "self" {
                            current_prefix.clone()
                        } else if current_prefix.is_empty() {
                            clean_token.to_string()
                        } else {
                            format!("{}::{}", current_prefix, clean_token)
                        };
                        if !full_path.is_empty() {
                            results.push(full_path);
                        }
                    }

                    if let Some(prev_prefix) = stack.pop() {
                        current_prefix = prev_prefix;
                    }
                    current_token.clear();
                }
                ',' => {
                    let token = current_token.trim();
                    if !token.is_empty() {
                        let clean_token = if let Some(as_idx) = token.find(" as ") {
                            token[..as_idx].trim()
                        } else {
                            token
                        };

                        let full_path = if clean_token == "self" {
                            current_prefix.clone()
                        } else if current_prefix.is_empty() {
                            clean_token.to_string()
                        } else {
                            format!("{}::{}", current_prefix, clean_token)
                        };
                        if !full_path.is_empty() {
                            results.push(full_path);
                        }
                    }
                    current_token.clear();
                }
                _ => {
                    current_token.push(c);
                }
            }
        }

        // Handle any trailing token (though unusual in well-formed bracketed uses)
        let token = current_token.trim();
        if !token.is_empty() {
            let clean_token = if let Some(as_idx) = token.find(" as ") {
                token[..as_idx].trim()
            } else {
                token
            };

            let full_path = if clean_token == "self" {
                current_prefix.clone()
            } else if current_prefix.is_empty() {
                clean_token.to_string()
            } else {
                format!("{}::{}", current_prefix, clean_token)
            };
            if !full_path.is_empty() {
                results.push(full_path);
            }
        }
    }

    fn extract_deps_from_stmt(stmt: &str) -> Vec<String> {
        let mut deps = Vec::new();
        let stmt = stmt.trim();

        // 1. Handle visibility modifiers
        let mut content = stmt;
        if content.starts_with("pub") {
            content = &content[3..].trim_start();
            if content.starts_with('(') {
                // Skip past the matching closing parenthesis
                let mut depth = 0;
                let mut end_idx = 0;
                for (i, c) in content.char_indices() {
                    if c == '(' {
                        depth += 1;
                    } else if c == ')' {
                        depth -= 1;
                        if depth == 0 {
                            end_idx = i;
                            break;
                        }
                    }
                }
                if end_idx > 0 {
                    content = &content[end_idx + 1..].trim_start();
                }
            }
        }

        // 2. Parse mod or use
        if content.starts_with("mod ") {
            let mod_name = content[4..].trim();
            // In cases like `mod a { ... }`, we only care about the name before `{` if any,
            // though our split(';') might not give us blocks perfectly.
            // We assume standard `mod a;` since blocks wouldn't end in `;` without internal `;`.
            // Let's take the first token.
            let name = mod_name.split_whitespace().next().unwrap_or("").trim_end_matches('{').trim();
            if !name.is_empty() {
                deps.push(name.to_string());
            }
        } else if content.starts_with("use ") {
            let use_path = content[4..].trim();
            Self::expand_use_path(use_path, &mut deps);
        }

        deps
    }

    fn scan_file(
        file_path: &Path,
        absolute_workspace_root: &Path,
        dict_map: &mut HashMap<String, usize>,
        dict_list: &mut Vec<String>,
        topology_edges: &mut Vec<String>,
        depth: &ScanDepth,
        extract_symbols: bool,
    ) {
        if file_path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }

        let relative_path = file_path.strip_prefix(absolute_workspace_root).unwrap_or(file_path).to_path_buf();
        let node_name = relative_path.to_string_lossy().into_owned();
        let node_id = Self::get_or_insert_id(&node_name, dict_map, dict_list);

        if let Ok(raw_contents) = std::fs::read_to_string(file_path) {
            // Lexical Extraction: Strip comments
            let no_comments = Self::strip_comments(&raw_contents);

            let mut local_deps = Vec::new();

            if extract_symbols {
                // Line-by-line lexical pass to find file symbols.
                for line in no_comments.lines() {
                    let mut clean_line = line.trim();
                    while clean_line.starts_with("#[") || clean_line.starts_with("#![") {
                        if let Some(end_idx) = clean_line.find(']') {
                            clean_line = clean_line[end_idx + 1..].trim();
                        } else {
                            break;
                        }
                    }

                    // A basic zero-copy parsing to find our target keywords
                    let words: Vec<&str> = clean_line.split_whitespace().collect();
                    if words.is_empty() {
                        continue;
                    }

                    let mut is_pub = false;
                    let mut keyword_idx = 0;

                    if words[0] == "pub" {
                        is_pub = true;
                        keyword_idx = 1;
                        // Handle `pub (crate)` where there's a space
                        if words.len() > 1 && words[1].starts_with('(') {
                            keyword_idx = 2;
                        }
                    } else if words[0].starts_with("pub(") {
                        is_pub = true;
                        keyword_idx = 1;
                    }

                    // Skip intermediate modifiers like `async`, `const`, `unsafe`, `extern`, `default`
                    while keyword_idx < words.len() {
                        let w = words[keyword_idx];
                        if w == "async" || w == "const" || w == "unsafe" || w == "extern" || w == "default" {
                            keyword_idx += 1;
                        } else {
                            break;
                        }
                    }

                    if keyword_idx < words.len() {
                        let keyword = words[keyword_idx];

                        let is_target_symbol = match depth {
                            ScanDepth::Interface => {
                                is_pub && (keyword == "fn" || keyword == "struct" || keyword == "enum" || keyword == "trait")
                            }
                            ScanDepth::DeepAST => {
                                keyword == "fn" || keyword == "struct" || keyword == "enum" || keyword == "trait" || keyword == "impl"
                            }
                        };

                        if is_target_symbol {
                            if let Some(name) = words.get(keyword_idx + 1) {
                                // Extract the name, stopping at <, (, or {
                                let mut clean_name = *name;
                                if let Some(idx) = clean_name.find(|c| c == '<' || c == '(' || c == '{' || c == ':') {
                                    clean_name = &clean_name[..idx];
                                }

                                if !clean_name.is_empty() {
                                    let formatted_symbol = format!("{} {}", keyword, clean_name);
                                    let symbol_id = Self::get_or_insert_id(&formatted_symbol, dict_map, dict_list);
                                    local_deps.push(symbol_id);
                                }
                            }
                        }
                    }
                }
            }

            // Very simple tokenization by semicolons
            let statements = no_comments.split(';');

            for stmt in statements {
                // Skip empty or attribute-only lines simply by taking the non-attribute parts
                // but for now, extract_deps_from_stmt will handle valid keywords.
                // Note: We might have attributes like `#[cfg(test)] mod tests;`
                // Let's strip simple attributes that might prepend our statements.
                let mut clean_stmt = stmt.trim();
                while clean_stmt.starts_with("#[") || clean_stmt.starts_with("#![") {
                    if let Some(end_idx) = clean_stmt.find(']') {
                        clean_stmt = clean_stmt[end_idx + 1..].trim();
                    } else {
                        break;
                    }
                }

                // Also clean up multiline breaks (we didn't replace \n across the whole file anymore)
                let single_line_stmt = clean_stmt.replace('\n', " ").replace('\r', " ");

                let extracted_deps = Self::extract_deps_from_stmt(&single_line_stmt);
                for dep in extracted_deps {
                    let dep_id = Self::get_or_insert_id(&dep, dict_map, dict_list);
                    local_deps.push(dep_id);
                }
            }

            if !local_deps.is_empty() {
                // Deduplicate local_deps just in case
                local_deps.sort_unstable();
                local_deps.dedup();

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

                // Hardcode ScanDepth::Interface for now as per The Architect's instruction.
                if let Ok((compressed_payload, semantic_dag)) = MatrixScanner::map_topology(&absolute_targets, &absolute_workspace_root, ScanDepth::Interface) {
                    let is_single_file = absolute_targets.len() == 1 && absolute_targets[0].is_file();

                    if is_single_file {
                        let relative_path = absolute_targets[0].strip_prefix(&*absolute_workspace_root).unwrap_or(&absolute_targets[0]).to_path_buf();
                        let target_id = relative_path.to_string_lossy().into_owned();

                        // Graft for UI structure
                        let _ = synapse.fire_async(SMessage::Matrix(MatrixEvent::GraftTopology {
                            target_id: target_id.clone(),
                            payload: compressed_payload
                        })).await;

                        // Focus for LLM Context (Fixes missing DAG in single-file pre-flight)
                        let _ = synapse.fire_async(SMessage::Matrix(MatrixEvent::SectorFocused {
                            target: target_id,
                            context: semantic_dag
                        })).await;
                    } else {
                        // J21 PATHFINDER: Fire the True DAG directly to `vein` via `IngestTopology`.
                        // This raw data structure fuels the instant UI payload mutation.
                        let _ = synapse.fire_async(SMessage::Matrix(MatrixEvent::IngestTopology { ui_dag: compressed_payload, semantic_dag })).await;
                    }
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
