use gtk4::prelude::*;
use gtk4::{Align, Box, Label, Orientation, Widget};
use anyhow::{Context, Result};
use std::process::Command;
use std::str;

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
    /// The Torch: Illuminates the current repository state.
    pub fn look() -> Result<GitStatus> {
        // 1. CHECK BRANCH
        let branch_out = Command::new("git")
            .args(&["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .context("Failed to check git branch")?;

        if !branch_out.status.success() {
            // We are likely not in a git repo, or it's too dark (no commits yet).
            return Ok(GitStatus {
                branch: "VOID".to_string(),
                commit: "0000000".to_string(),
                is_dirty: false,
            });
        }
        let branch = str::from_utf8(&branch_out.stdout)?.trim().to_string();

        // 2. CHECK COMMIT HASH (Short)
        let commit_out = Command::new("git")
            .args(&["rev-parse", "--short", "HEAD"])
            .output()?;
        let commit = str::from_utf8(&commit_out.stdout)?.trim().to_string();

        // 3. CHECK DIRTY STATE (Porcelain)
        // If this returns output, the repo is dirty.
        let status_out = Command::new("git")
            .args(&["status", "--porcelain"])
            .output()?;
        let is_dirty = !status_out.stdout.is_empty();

        Ok(GitStatus {
            branch,
            commit,
            is_dirty,
        })
    }
}
