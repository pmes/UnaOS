use crate::gravity::GravityWell;
use unafs::io::MappedFile;
use elessar::context::SkeletonGenerator;
use gneiss_pal::io::MemoryMappedRegion;
use bandy::{SMessage, MatrixEvent, SpatialNode, SpatialEdge};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::broadcast;
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

/// The Background Indexer Task.
/// It scans the workspace, maps the topology, and builds the context.
pub async fn run_indexer(root: PathBuf, tx: broadcast::Sender<SMessage>, telemetry_tx: async_channel::Sender<SMessage>) {
    let payload = scan_and_score(&root, &tx).await;

    if !payload.is_empty() {
        info!(":: CORTEX :: Broadcasting Telemetry Payload ({} items)", payload.len());
        // We send the compiled telemetry across the thread boundary.
        // The payload contains Arc<String>, so no actual skeleton text is copied.
        // We use the High-Priority Telemetry Channel (Async) directly to the UI.
        let _ = telemetry_tx.send(SMessage::ContextTelemetry { skeletons: payload }).await;
    }
}

async fn scan_and_score(root: &Path, tx: &broadcast::Sender<SMessage>) -> Vec<bandy::WeightedSkeleton> {
    info!(":: CORTEX :: Indexing Workspace at {:?}", root);

    // 1. IGNITE THE INDEXER
    // We use Elessar to build a DAG of the workspace crates.
    let mut indexer = elessar::context::WorkspaceIndexer::new();
    indexer.scan(root);

    let mut spatial_nodes = Vec::new();
    let mut spatial_edges = Vec::new();
    let mut skeleton_cache: HashMap<PathBuf, Arc<String>> = HashMap::new();
    let mut gravity = GravityWell::new();
    let mut total_skeletons = 0;

    // 2. EXTRACT TOPOLOGY & INGEST SKELETONS
    for (crate_name, node) in &indexer.nodes {
        // Map the Crate to a Spatial Node for the Matrix UI
        spatial_nodes.push(SpatialNode {
            id: crate_name.clone(),
            kind: "crate".to_string(),
            path: node.path.clone(),
        });

        // Map the dependencies to Spatial Edges
        for dep in &node.dependencies {
            // We only map edges to crates that exist within our local workspace DAG
            if indexer.nodes.contains_key(dep) {
                spatial_edges.push(SpatialEdge {
                    from: crate_name.clone(),
                    to: dep.clone(),
                    relation: "depends_on".to_string(),
                });
            }
        }

        // Ingest the source files for this specific crate
        let src_dir = node.path.join("src");
        if src_dir.exists() {
            // Recursive walk to find all .rs files
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
                        // Store the skeleton in the memory cache (Zero-Copy Arc)
                        skeleton_cache.insert(file.clone(), Arc::new(skeleton));
                        total_skeletons += 1;

                        // Mock Heuristic for "Context Awareness":
                        if file.to_string_lossy().contains("libs/bandy/src/lib.rs") {
                             gravity.set_focus(file.clone());
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(SMessage::Log {
                            level: "WARN".to_string(),
                            source: "Cortex".to_string(),
                            content: e,
                        });
                    }
                }
            }
        }
    }

    // 3. BROADCAST THE TOPOLOGY
    // We fire the DAG across the nervous system so Matrix can render the 3D/List view immediately.
    let _ = tx.send(SMessage::Matrix(MatrixEvent::IngestTopology {
        nodes: spatial_nodes,
        edges: spatial_edges,
    }));

    let _ = tx.send(SMessage::Log {
        level: "INFO".to_string(),
        source: "Cortex".to_string(),
        content: format!("Workspace Indexed. Generated {} AST Skeletons.", total_skeletons),
    });

    // 4. COMPILE TELEMETRY
    // We calculate the gravitational pull of the current context.
    gravity.calculate_scores(&skeleton_cache)
}
