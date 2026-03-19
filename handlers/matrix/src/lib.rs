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
pub struct MatrixScanner;
impl MatrixScanner {
    pub fn map_topology(path: &Path, absolute_workspace_root: &Path) -> Result<MatrixEvent, String> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        // 1. INPUT BIFURCATION
        // Safely and accurately handle both directory and explicit file targets.
        if path.is_file() {
            Self::scan_file(path, absolute_workspace_root, &mut nodes, &mut edges);
        } else if path.is_dir() {
            Self::scan_directory(path, absolute_workspace_root, &mut nodes, &mut edges);
        } else {
            return Err("Target is neither a file nor a directory.".to_string());
        }

        Ok(MatrixEvent::IngestTopology { nodes, edges })
    }

    /// Recursively walks a directory, finding all .rs files and building the module tree.
    fn scan_directory(
        dir: &Path,
        absolute_workspace_root: &Path,
        nodes: &mut Vec<bandy::SpatialNode>,
        edges: &mut Vec<bandy::SpatialEdge>,
    ) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    Self::scan_directory(&p, absolute_workspace_root, nodes, edges);
                } else if p.is_file() {
                    Self::scan_file(&p, absolute_workspace_root, nodes, edges);
                }
            }
        }
    }

    /// Performs a zero-copy lexical scan of a .rs file to map structural topology.
    fn scan_file(
        file_path: &Path,
        absolute_workspace_root: &Path,
        nodes: &mut Vec<bandy::SpatialNode>,
        edges: &mut Vec<bandy::SpatialEdge>,
    ) {
        // Only process Rust source files
        if file_path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }

        // 1. CREATE NODE
        // J21 PATHFINDER: Store paths relative to the absolute root anchor to conserve memory.
        let relative_path = file_path.strip_prefix(absolute_workspace_root).unwrap_or(file_path).to_path_buf();
        let node_id = relative_path.to_string_lossy().into_owned();

        nodes.push(bandy::SpatialNode {
            id: node_id.clone(),
            kind: "module".to_string(),
            path: relative_path,
        });

        // 2. LEXICAL SCAN
        // Read file contents into a string and fast-scan for structural keywords.
        // We avoid external AST crates to maintain zero-latency core boot times.
        if let Ok(contents) = std::fs::read_to_string(file_path) {
            for line in contents.lines() {
                let trimmed = line.trim();

                // Detect module declarations (e.g., `mod foo;` or `pub mod bar;`)
                if trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    // Extract the module name, stripping the trailing semicolon
                    if let Some(mod_name) = parts.last() {
                        let clean_name = mod_name.trim_end_matches(';');
                        edges.push(bandy::SpatialEdge {
                            from: node_id.clone(),
                            to: clean_name.to_string(),
                            relation: "declares".to_string(),
                        });
                    }
                }

                // Detect use statements (e.g., `use std::path::Path;`)
                if trimmed.starts_with("use ") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() > 1 {
                        let clean_use = parts[1].trim_end_matches(';');
                        edges.push(bandy::SpatialEdge {
                            from: node_id.clone(),
                            to: clean_use.to_string(),
                            relation: "imports".to_string(),
                        });
                    }
                }
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

    if let Ok(MatrixEvent::IngestTopology { nodes, .. }) = MatrixScanner::map_topology(root_path, root_path) {
        // Filter to just the files (modules) for the visual list
        for node in nodes.into_iter().filter(|n| n.kind == "module") {
            let row = Box::new(Orientation::Horizontal, 10);
            row.set_margin_start(10);
            row.set_margin_end(10);
            row.set_margin_top(5);
            row.set_margin_bottom(5);

            row.append(&Image::from_icon_name("text-x-generic-symbolic"));

            let label = Label::new(Some(&node.id));
            label.set_hexpand(true);
            label.set_xalign(0.0);

            // HACK/ELEGANCE: Store the absolute path in the widget's internal name string.
            // This avoids complex GTK subclassing just to hold a PathBuf.
            row.set_widget_name(&node.path.to_string_lossy());

            row.append(&label);
            matrix_list.append(&row);
        }
    }

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

                if let Ok(MatrixEvent::IngestTopology { nodes, edges }) = MatrixScanner::map_topology(&absolute_target, &absolute_workspace_root) {
                    // J21 PATHFINDER: Fire the True DAG directly to `vein` via `IngestTopology`.
                    // This raw data structure fuels the instant UI payload mutation.
                    let _ = synapse.fire_async(SMessage::Matrix(MatrixEvent::IngestTopology { nodes, edges })).await;
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
