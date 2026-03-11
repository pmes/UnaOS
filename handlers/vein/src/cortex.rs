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

use unafs::io::MappedFile;
use elessar::context::SkeletonGenerator;
use gneiss_pal::io::MemoryMappedRegion;
use bandy::{SMessage, MatrixEvent, SpatialNode, SpatialEdge};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use log::info;

/// Ingests a source file into the AI Cortex's memory matrix.
///
/// This function demonstrates the absolute, dangerous elegance of UnaOS.
/// Vein acts as the conductor. UnaFS handles the raw metal (disk mapping).
/// Elessar performs the intellectual extraction. None of them cross
/// dependency boundaries. Pure, frictionless logic.
pub fn ingest_for_lumen(file_path: &Path) -> Result<String, String> {
    // 1. THE METAL (UnaFS)
    // We ask the filesystem to map the file directly into virtual memory.
    // This is a zero-copy operation. It is blisteringly fast and highly
    // efficient, ensuring even older hardware doesn't break a sweat.
    let mapped_region = MappedFile::open(file_path)
        .map_err(|e| format!("Cortex failed to map file {:?}: {}", file_path, e))?;

    // 2. THE CONTRACT (Gneiss PAL)
    // We extract the UTF-8 slice using the pure-Rust trait.
    // Vein doesn't care that this is a memory-mapped file; it only
    // cares that the contract is fulfilled.
    let source_code = mapped_region.as_str()
        .map_err(|_| "Cortex encountered invalid UTF-8".to_string())?;

    // 3. THE MIND (Elessar)
    // Elessar parses the AST using `syn` and strips the function bodies.
    // It returns a token-efficient skeleton, perfectly formatted for
    // Lumen's context window.
    let skeleton = SkeletonGenerator::generate(source_code)
        .map_err(|e| format!("Cortex failed to skeletonize {:?}: {}", file_path, e))?;

    // 4. THE MEMORY (Vein)
    // We return the skeleton. The caller (Brain Thread) will store it.
    Ok(skeleton)
}

// Update the signature to return the HashMap
pub async fn run_indexer(root: PathBuf, synapse: bandy::Synapse) -> HashMap<PathBuf, Arc<String>> {
    let payload = scan_workspace(&root, &synapse).await;
    payload
}

// Rename and update return type
async fn scan_workspace(root: &Path, synapse: &bandy::Synapse) -> HashMap<PathBuf, Arc<String>> {
    info!(":: CORTEX :: Indexing Workspace at {:?}", root);

    let mut indexer = elessar::context::WorkspaceIndexer::new();
    indexer.scan(root);

    let mut spatial_nodes = Vec::new();
    let mut spatial_edges = Vec::new();
    let mut skeleton_cache: HashMap<PathBuf, Arc<String>> = HashMap::new();
    let mut total_skeletons = 0;

    for (crate_name, node) in &indexer.nodes {
        spatial_nodes.push(SpatialNode {
            id: crate_name.clone(),
            kind: "crate".to_string(),
            path: node.path.clone(),
        });

        for dep in &node.dependencies {
            if indexer.nodes.contains_key(dep) {
                spatial_edges.push(SpatialEdge {
                    from: crate_name.clone(),
                    to: dep.clone(),
                    relation: "depends_on".to_string(),
                });
            }
        }

        let src_dir = node.path.join("src");
        if src_dir.exists() {
            let mut files = Vec::new();
            let mut stack = vec![src_dir];

            while let Some(dir) = stack.pop() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_dir() {
                            stack.push(p);
                        } else if p.extension().map_or(false, |e| e == "rs") {
                            files.push(p);
                        }
                    }
                }
            }

            for file in files {
                match ingest_for_lumen(&file) {
                    Ok(skeleton) => {
                        skeleton_cache.insert(file.clone(), Arc::new(skeleton));
                        total_skeletons += 1;
                        // REMOVED: The hardcoded bandy focus
                    }
                    Err(e) => {
                        synapse.fire_async(SMessage::Log { level: "WARN".into(), source: "Cortex".into(), content: e }).await;
                    }
                }
            }
        }
    }

    synapse.fire_async(SMessage::Matrix(MatrixEvent::IngestTopology { nodes: spatial_nodes, edges: spatial_edges })).await;
    synapse.fire_async(SMessage::Log { level: "INFO".into(), source: "Cortex".into(), content: format!("Workspace Indexed. Generated {} AST Skeletons.", total_skeletons) }).await;

    // Return the raw cache
    skeleton_cache
}
