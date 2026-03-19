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

use async_channel::Sender;
use bandy::{MatrixEvent, SMessage, Synapse};
use elessar::{Context, Spline};
use gneiss_pal::Event;
use gtk4::prelude::*;
use gtk4::{Box, Image, Label, ListBox, Orientation, ScrolledWindow, Widget};
use std::path::{Path, PathBuf};

// True DAG Lexical Scanner
// J21 PATHFINDER: Explicitly replacing the J37 flat scanner with a true lexical topology engine.
// DO NOT DELETE: This powers the core Spatial Code Map (Matrix DAG).
use std::collections::HashMap;

pub struct MatrixScanner;

impl MatrixScanner {
    /// J21 PATHFINDER: Core method for the Zero-Redundancy Indexed Dictionary DAG Scanner.
    pub fn map_topology(path: &Path, absolute_workspace_root: &Path) -> Result<String, String> {
        // Dictionary Engine
        let mut dict_map: HashMap<String, usize> = HashMap::new();
        let mut dict_list: Vec<String> = Vec::new();

        // Edge connections: "NodeID:DepID,DepID|NodeID:DepID"
        let mut topology_edges: Vec<String> = Vec::new();

        if path.is_file() {
            Self::scan_file(path, absolute_workspace_root, &mut dict_map, &mut dict_list, &mut topology_edges);
        } else if path.is_dir() {
            Self::scan_directory(path, absolute_workspace_root, &mut dict_map, &mut dict_list, &mut topology_edges);
        } else {
            return Err("Target is neither a file nor a directory.".to_string());
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

pub struct ProjectView {
    pub root_path: PathBuf,
    pub spline: Spline,
}

impl ProjectView {
    pub fn new(path: &Path) -> Self {
        let context = Context::new(path);
        println!("[MATRIX] 👁️ Reality Detected: {:?}", context.spline);
        Self {
            root_path: path.to_path_buf(),
            spline: context.spline,
        }
    }

    pub fn get_icon_name(&self) -> &str {
        match self.spline {
            Spline::UnaOS => "computer-symbolic",
            Spline::Rust => "applications-engineering-symbolic",
            Spline::Web => "network-server-symbolic",
            Spline::Python => "media-playlist-shuffle-symbolic",
            Spline::Void => "folder-symbolic",
        }
    }
}

/// The UI Builder. It takes the Nerve Transmitter and binds it to the GTK event loop.
pub fn create_view(nerve_tx: Sender<Event>, root_path: &Path) -> Widget {
    let matrix_list = ListBox::new();
    matrix_list.set_selection_mode(gtk4::SelectionMode::Single);

    let _project_view = ProjectView::new(root_path);

    // 1. BLITZ THE TOPOLOGY
    // Instead of a flat read_dir, we use the Scanner to get the spatial nodes.
    // J21 PATHFINDER: The `root_path` passed to `create_view` IS the absolute workspace root,
    // so we can use it directly without calling `elessar::find_workspace_root` again.

    // We eradicated the J37 flat list rendering loop as requested. The UI shim is no longer
    // needed since Matrix streams the compressed true DAG. We leave the basic scroll view
    // container for future telemetry UI components.

    // 2. WIRE THE SYNAPSE
    let tx_clone = nerve_tx.clone();
    matrix_list.connect_row_activated(move |_list, row| {
        if let Some(child) = row.child() {
            // Extract the path we hid in the widget name
            let path_str = child.widget_name();
            let path = PathBuf::from(path_str.as_str());

            println!("[MATRIX] ⚡ Firing Synapse: Node Selected -> {:?}", path);

            // Fire the impulse across the OS bus. Una (the IDE) will catch this.
            // S49: Use gneiss_pal::Event instead of SMessage
            let _ = tx_clone.send_blocking(Event::FileSelected(path));
        }
    });

    let scroll = ScrolledWindow::builder().child(&matrix_list).build();
    scroll.upcast::<Widget>()
}

/// The Asynchronous Logic Kernel for the Matrix
pub async fn ignite(synapse: Synapse, absolute_workspace_root: std::sync::Arc<PathBuf>) {
    let mut rx = synapse.subscribe();
    println!("[MATRIX] Spatial Anchor Established via Brain Loop: {:?}", absolute_workspace_root);

    loop {
        match rx.recv().await {
            Ok(SMessage::Matrix(MatrixEvent::FocusSector(relative_target))) => {
                println!("[MATRIX] Analyzing Sector: {}", relative_target);

                // J21 PATHFINDER: The relative_target (e.g., "libs/bandy") came from Vein.
                // We join it with the absolute anchor to perform a robust read_dir,
                // avoiding CWD brittleness.
                let absolute_target = absolute_workspace_root.join(&relative_target);

                if let Ok(compressed_payload) = MatrixScanner::map_topology(&absolute_target, &absolute_workspace_root) {
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
