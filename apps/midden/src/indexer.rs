use crate::pile::{Pile, Entry};
use walkdir::WalkDir;
use anyhow::Result;
use std::path::Path;

pub struct Indexer;

impl Indexer {
    pub fn scan(root: &Path) -> Result<Pile> {
        let mut pile = Pile::new();

        println!("Midden is learning the strata at: {:?}", root);

        for entry in WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| !is_ignored(e.path())) // The Filter Logic
        {
            let metadata = entry.metadata()?;
            let path = entry.path().to_path_buf();

            // CLASSIFICATION LOGIC
            let mut tags = Vec::new();
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                match ext {
                    "rs" | "go" | "c" | "cpp" | "py" => tags.push("code".to_string()),
                    "md" | "txt" => tags.push("doc".to_string()),
                    "toml" | "json" | "yaml" | "yml" => tags.push("config".to_string()),
                    "lock" => tags.push("meta".to_string()),
                    _ => {},
                }
                // Specific Language Tags
                tags.push(ext.to_string());
            }

            pile.entries.push(Entry {
                path,
                is_file: metadata.is_file(),
                size: metadata.len(),
                tags,
            });
        }

        println!("Knowledge acquired. {} entries found.", pile.entries.len());
        Ok(pile)
    }
}

// Helper to ignore the noise
fn is_ignored(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == ".git" || s == "target" || s == "node_modules"
    })
}
