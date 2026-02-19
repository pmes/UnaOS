use gtk4::prelude::*;
use gtk4::{Align, Box, Label, Orientation, Widget};
use anyhow::{Context, Result};
use git2::{Repository, StatusOptions};

pub fn create_view() -> Widget {
    let vaire_box = Box::new(Orientation::Vertical, 10);
    vaire_box.set_valign(Align::Center);

    // In a real app, we would call Vaire::look() here and update the label.
    // For now, we keep the stub or maybe try to look?
    // Let's try to look and show it!

    let label_text = match Vaire::look() {
        Ok(status) => format!("Branch: {}\nCommit: {}\nDirty: {}", status.branch, status.commit, status.is_dirty),
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
    /// The High Loom: Inspects the repository using direct memory access (libgit2).
    pub fn look() -> Result<GitStatus> {
        // 1. OPEN THE REPOSITORY (Finds .git automatically walking up)
        let repo = Repository::open_from_env()
            .or_else(|_| Repository::discover("."))
            .context("No repository found")?;

        // 2. GET HEAD (Branch or Detached)
        let head = repo.head().context("Failed to get HEAD")?;

        let branch = if let Some(name) = head.shorthand() {
            name.to_string()
        } else {
            "DETACHED".to_string()
        };

        // 3. GET COMMIT HASH (OID)
        let commit = if let Some(target) = head.target() {
            let full = target.to_string();
            // Shorten to 7 chars for display
            full.chars().take(7).collect()
        } else {
            "0000000".to_string()
        };

        // 4. CHECK DIRTY STATE (The Matrix)
        // We scan for modified, added, or deleted files.
        let mut status_opts = StatusOptions::new();
        status_opts.include_untracked(true); // Show untracked files as dirty? Usually yes.

        let statuses = repo.statuses(Some(&mut status_opts))?;
        let is_dirty = !statuses.is_empty();

        Ok(GitStatus {
            branch,
            commit,
            is_dirty,
        })
    }
}
