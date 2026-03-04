use anyhow::{Context, Result};
use bandy::{SMessage, BandyMember};
#[cfg(feature = "gtk4")]
use gtk4::prelude::*;
#[cfg(feature = "gtk4")]
use gtk4::{Align, Box, Label, Orientation, Widget};

// Gix (Gitoxide) Imports
use gix::{discover};
use gix::object::tree::diff::Change;
use gix::object::tree::diff::Action;

#[cfg(feature = "gtk4")]
pub fn create_view() -> Widget {
    let vaire_box = Box::new(Orientation::Vertical, 10);
    vaire_box.set_valign(Align::Center);

    let label_text = match Vaire::look() {
        Ok(status) => format!(
            "Branch: {}\nCommit: {}\nDirty: {}",
            status.branch, status.commit, status.is_dirty
        ),
        Err(_) => "No Git Repository Detected".to_string(),
    };

    vaire_box.append(&Label::new(Some(&label_text)));
    vaire_box.upcast::<Widget>()
}

pub struct Vaire;

#[derive(Debug)]
pub struct GitStatus {
    pub branch: String,
    pub commit: String,
    pub is_dirty: bool,
}

impl Vaire {
    /// The High Loom: Inspects the repository using direct memory access (gix).
    pub fn look() -> Result<GitStatus> {
        // 1. OPEN THE REPOSITORY (Finds .git automatically walking up)
        let repo = discover(".").context("No repository found")?;
        let head = repo.head()?;

        let branch = head.referent_name().map(|n| n.as_bstr().to_string()).unwrap_or_else(|| "DETACHED".to_string());

        let commit_id = head.id().context("Head has no commit")?;
        let commit = commit_id.to_hex().to_string().chars().take(7).collect();

        // Dirty Check Stub
        let is_dirty = false;

        Ok(GitStatus {
            branch,
            commit,
            is_dirty,
        })
    }

    /// Handles an incoming SMessage.
    pub fn handle_message(msg: &SMessage) -> Option<SMessage> {
        match msg {
            SMessage::GetDiff { commit_a, commit_b } => {
                match Self::get_diff(commit_a, commit_b) {
                    Ok(diff) => Some(SMessage::DiffPayload { diff }),
                    Err(e) => Some(SMessage::Log {
                        level: "ERROR".to_string(),
                        source: "Vaire".to_string(),
                        content: format!("Diff failed: {}", e),
                    }),
                }
            }
            _ => None,
        }
    }

    /// Generates a unified diff between two commits using pure-Rust gix.
    fn get_diff(rev_a: &str, rev_b: &str) -> Result<String> {
        let repo = discover(".")?;

        // Resolve revisions to Objects -> Trees
        let a = repo.rev_parse_single(rev_a.as_bytes())?;
        let b = repo.rev_parse_single(rev_b.as_bytes())?;

        let tree_a = a.object()?.peel_to_tree()?;
        let tree_b = b.object()?.peel_to_tree()?;

        let mut diff_payload = String::with_capacity(1024);

        // Execute the pure-Rust tree diff provided by `gix`.
        // We use tree_a.changes().for_each_to_obtain_tree(&tree_b, ...)
        tree_a.changes()?
            .for_each_to_obtain_tree(&tree_b, |change| {
                match change {
                    Change::Addition { location, .. } => {
                        diff_payload.push_str(&format!("+ Added: {:?}\n", location));
                    }
                    Change::Deletion { location, .. } => {
                        diff_payload.push_str(&format!("- Deleted: {:?}\n", location));
                    }
                    Change::Modification { location, .. } => {
                        diff_payload.push_str(&format!("~ Modified: {:?}\n", location));
                    }
                    Change::Rewrite { location, .. } => {
                        diff_payload.push_str(&format!("* Rewritten: {:?}\n", location));
                    }
                }
                Ok::<_, anyhow::Error>(Action::Continue(()))
            })?;

        if diff_payload.is_empty() {
            diff_payload.push_str("No changes detected.");
        }

        Ok(diff_payload)
    }
}
