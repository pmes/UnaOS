use unafs::io::MappedFile;
use elessar::context::SkeletonGenerator;
use gneiss_pal::io::MemoryMappedRegion;
use bandy::SMessage;
use std::path::Path;
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
/// It scans the workspace and builds the context.
pub async fn run_indexer(root: std::path::PathBuf, tx: broadcast::Sender<SMessage>) {
    info!(":: CORTEX :: Indexing Workspace at {:?}", root);

    // Naive scan for .rs files in libs/ and handlers/
    // In a real implementation, we would use `elessar::context::WorkspaceIndexer`.
    // Let's use it!

    let mut indexer = elessar::context::WorkspaceIndexer::new();
    indexer.scan(&root);

    let mut total_skeletons = 0;

    for (crate_name, node) in indexer.nodes {
        // Iterate over source files in the crate
        // Assuming src/lib.rs or src/main.rs exists
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
                    Ok(_skeleton) => {
                        // In a real system, we would store this in the Vector DB.
                        // For now, we just count it.
                        total_skeletons += 1;
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

    let _ = tx.send(SMessage::Log {
        level: "INFO".to_string(),
        source: "Cortex".to_string(),
        content: format!("Workspace Indexed. Generated {} AST Skeletons.", total_skeletons),
    });
}
