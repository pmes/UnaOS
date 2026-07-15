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

//! Vaire — the Loom: the repo/workspace manager for UnaOS.
//!
//! Vaire manages a workspace as a set of **Bolts**: managed units, each with a
//! kind (a Git repository, a UnaFS vault, or a reserved NLE project). It
//! discovers/registers them in a manifest and reports each one's real status,
//! mapped to the **Crystal Color** vocabulary (Green / Amber / Red).
//!
//! This crate is the STATUS half of the Loom (per-unit truth). SYNC and SNAP
//! are prospective (see the README); UnaFS-native versioning is the Destiny.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bandy::SMessage;

/// The dev-tree Bolt (BOLT-1): STATUS / SNAP / SYNC of a source tree that must
/// exist coherently on two drives, without the live-copy flip (SWITCH is arc 2).
pub mod devtree;
#[cfg(feature = "gtk4")]
use gtk4::prelude::*;
#[cfg(feature = "gtk4")]
use gtk4::{Align, Box, Label, Orientation, Widget};

// Gix (Gitoxide) Imports
use gix::discover;
use gix::object::tree::diff::Action;
use gix::object::tree::diff::Change;

// UnaFS: the vault-probe rides its fail-closed mount check (read-only).
use unafs::{FileDevice, UnaFS};

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

#[derive(Debug, Clone)]
pub struct GitStatus {
    pub branch: String,
    pub commit: String,
    pub is_dirty: bool,
}

/// The "Crystal Color" of a Bolt — the God-view at-a-glance state.
///
/// Canon vocabulary (pre-drift Loom charter):
/// * **Green** — clean, synced, ready.
/// * **Amber** — local changes present.
/// * **Red** — detached HEAD, conflict, or an unreadable/absent unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrystalColor {
    Green,
    Amber,
    Red,
}

impl CrystalColor {
    /// Map a git status to a Crystal Color: detached HEAD is Red, a dirty
    /// working tree is Amber, otherwise Green.
    pub fn from_git(status: &GitStatus) -> Self {
        if status.branch == "DETACHED" {
            CrystalColor::Red
        } else if status.is_dirty {
            CrystalColor::Amber
        } else {
            CrystalColor::Green
        }
    }
}

/// The kind of managed unit a [`Bolt`] wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltKind {
    /// A Git repository (status via `gix`).
    GitRepo,
    /// A UnaFS semantic vault (status via the fail-closed read-only mount probe).
    Vault,
    /// Reserved: a non-linear-editing project. Registered but not yet
    /// status-aware (RITES-2+).
    NleProject,
}

/// A managed unit under the Loom's care — a declared path plus its kind.
#[derive(Debug, Clone)]
pub struct Bolt {
    pub name: String,
    pub path: PathBuf,
    pub kind: BoltKind,
}

/// The computed status of a single [`Bolt`].
#[derive(Debug, Clone)]
pub struct BoltStatus {
    pub name: String,
    pub kind: BoltKind,
    pub crystal: CrystalColor,
    /// Human-readable detail (branch@commit, mount state, or the failure).
    pub detail: String,
}

/// The Bolt manifest: the declarative registry of managed units.
///
/// Held in memory and driven programmatically; a hosting vessel supplies the
/// declaration. Registration is order-preserving; [`Self::status_all`] reports
/// every unit's live Crystal Color.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    bolts: Vec<Bolt>,
}

impl Manifest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a managed unit. Returns the assigned index in the manifest.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        kind: BoltKind,
    ) -> usize {
        self.bolts.push(Bolt {
            name: name.into(),
            path: path.into(),
            kind,
        });
        self.bolts.len() - 1
    }

    /// The registered units, in registration order.
    pub fn list(&self) -> &[Bolt] {
        &self.bolts
    }

    /// Live status of one unit, dispatched on its kind.
    pub fn status_of(&self, bolt: &Bolt) -> BoltStatus {
        let (crystal, detail) = match bolt.kind {
            BoltKind::GitRepo => match Vaire::look_at(&bolt.path) {
                Ok(git) => (
                    CrystalColor::from_git(&git),
                    format!(
                        "{} @ {}{}",
                        git.branch,
                        git.commit,
                        if git.is_dirty { " (dirty)" } else { "" }
                    ),
                ),
                Err(e) => (CrystalColor::Red, format!("not a readable repo: {e}")),
            },
            BoltKind::Vault => probe_vault(&bolt.path),
            BoltKind::NleProject => (
                CrystalColor::Red,
                "NLE project kind reserved (not status-aware until RITES-2+)".to_string(),
            ),
        };
        BoltStatus {
            name: bolt.name.clone(),
            kind: bolt.kind,
            crystal,
            detail,
        }
    }

    /// Live status of every registered unit, in registration order.
    pub fn status_all(&self) -> Vec<BoltStatus> {
        self.bolts.iter().map(|b| self.status_of(b)).collect()
    }
}

/// Probe a UnaFS vault's status, read-only, riding the fail-closed mount check.
///
/// * absent path → Red.
/// * present but unreadable/unmountable (corruption, version skew) → Red;
///   the on-disk bytes are never touched (the file is opened read-only).
/// * present and mounts → Green. Last-snapshot is not yet tracked (SNAP is
///   RITES-2), so it is reported as not-available.
fn probe_vault(path: &Path) -> (CrystalColor, String) {
    if !path.exists() {
        return (CrystalColor::Red, "vault absent".to_string());
    }
    let device = match FileDevice::open_read_only(path) {
        Ok(d) => d,
        Err(e) => return (CrystalColor::Red, format!("vault unreadable: {e}")),
    };
    match UnaFS::mount(device) {
        Ok(_) => (
            CrystalColor::Green,
            "mounted; last-snapshot: n/a (SNAP is RITES-2)".to_string(),
        ),
        Err(e) => (
            CrystalColor::Red,
            format!("present but failed to mount (fail-closed, bytes untouched): {e}"),
        ),
    }
}

impl Vaire {
    /// The High Loom: inspect the repository at (or above) the current
    /// directory and return its [`GitStatus`].
    pub fn look() -> Result<GitStatus> {
        Self::look_at(".")
    }

    /// Inspect the repository discovered at (or above) `path`. Path-parameterized
    /// so callers — and tests — need not depend on the process working directory.
    pub fn look_at<P: AsRef<Path>>(path: P) -> Result<GitStatus> {
        // 1. OPEN THE REPOSITORY (Finds .git automatically walking up)
        let repo = discover(path).context("No repository found")?;
        let head = repo.head()?;

        let branch = head
            .referent_name()
            .map(|n| n.as_bstr().to_string())
            .unwrap_or_else(|| "DETACHED".to_string());

        let commit_id = head.id().context("Head has no commit")?;
        let commit = commit_id.to_hex().to_string().chars().take(7).collect();

        // Real dirty check: gix compares the working tree against the index and
        // the index against HEAD's tree. (Untracked files do not flip this,
        // matching `git status` porcelain's tracked-change notion of "dirty".)
        // This replaces the historical `false` stub — a documented lie.
        let is_dirty = repo.is_dirty().context("dirty-state check failed")?;

        Ok(GitStatus {
            branch,
            commit,
            is_dirty,
        })
    }

    /// Handles an incoming SMessage.
    pub fn handle_message(msg: &SMessage) -> Option<SMessage> {
        match msg {
            SMessage::GetDiff { commit_a, commit_b } => match Self::get_diff(commit_a, commit_b) {
                Ok(diff) => Some(SMessage::DiffPayload { diff }),
                Err(e) => Some(SMessage::Log {
                    level: "ERROR".to_string(),
                    source: "Vaire".to_string(),
                    content: format!("Diff failed: {}", e),
                }),
            },
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
        tree_a
            .changes()?
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// Build a git repo fixture with one commit of `filename` -> `contents`.
    /// Uses the git CLI with a local, hermetic identity so it needs no global
    /// config and touches no shared state (everything lives under `dir`).
    fn init_repo(dir: &Path, filename: &str, contents: &str) {
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git must be available");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "loom@unaos.test"]);
        run(&["config", "user.name", "Loom Test"]);
        run(&["config", "commit.gpgsign", "false"]);
        fs::write(dir.join(filename), contents).expect("write fixture file");
        run(&["add", filename]);
        run(&["commit", "-q", "-m", "seed"]);
    }

    /// Format a small valid UnaFS vault at `path` (mirrors amber_bytes' first-run
    /// path: create, size, format), then drop it so the file is closed.
    fn make_vault(path: &Path) {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .expect("create vault file")
            .set_len(4 * 1024 * 1024)
            .expect("size vault");
        let device = FileDevice::open(path).expect("open vault device");
        let _fs = UnaFS::format(device, 4).expect("format vault");
    }

    #[test]
    fn clean_repo_reports_green_not_dirty() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repo(dir.path(), "a.txt", "hello\n");

        let status = Vaire::look_at(dir.path()).expect("look must succeed");
        assert!(
            status.branch.ends_with("main"),
            "branch was {:?}",
            status.branch
        );
        assert_eq!(status.commit.len(), 7, "short commit hash");
        assert!(!status.is_dirty, "a freshly committed repo must be clean");
        assert_eq!(CrystalColor::from_git(&status), CrystalColor::Green);
    }

    #[test]
    fn modified_tracked_file_reports_amber_dirty() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repo(dir.path(), "a.txt", "hello\n");

        // Mutate a TRACKED file — this is the case the old `false` stub lied about.
        fs::write(dir.path().join("a.txt"), "hello, world\n").expect("modify tracked file");

        let status = Vaire::look_at(dir.path()).expect("look must succeed");
        assert!(
            status.is_dirty,
            "a modified tracked file must report is_dirty = true (the fixed stub)"
        );
        assert_eq!(CrystalColor::from_git(&status), CrystalColor::Amber);
    }

    #[test]
    fn manifest_registers_and_lists_in_order() {
        let mut m = Manifest::new();
        let i0 = m.register("kernel", "/tmp/k", BoltKind::GitRepo);
        let i1 = m.register("vault", "/tmp/v.unafs", BoltKind::Vault);
        assert_eq!((i0, i1), (0, 1));
        assert_eq!(m.list().len(), 2);
        assert_eq!(m.list()[0].name, "kernel");
        assert_eq!(m.list()[1].kind, BoltKind::Vault);
    }

    #[test]
    fn manifest_status_all_maps_git_crystal() {
        let clean = tempfile::tempdir().expect("tempdir");
        init_repo(clean.path(), "a.txt", "x\n");
        let dirty = tempfile::tempdir().expect("tempdir");
        init_repo(dirty.path(), "a.txt", "x\n");
        fs::write(dirty.path().join("a.txt"), "y\n").expect("dirty it");

        let mut m = Manifest::new();
        m.register("clean", clean.path(), BoltKind::GitRepo);
        m.register("dirty", dirty.path(), BoltKind::GitRepo);

        let all = m.status_all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].crystal, CrystalColor::Green);
        assert_eq!(all[1].crystal, CrystalColor::Amber);
        assert!(all[1].detail.contains("dirty"));
    }

    #[test]
    fn vault_bolt_absent_is_red() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope.unafs");
        let mut m = Manifest::new();
        m.register("vault", &missing, BoltKind::Vault);
        let s = &m.status_all()[0];
        assert_eq!(s.crystal, CrystalColor::Red);
        assert!(s.detail.contains("absent"));
    }

    #[test]
    fn vault_bolt_mounts_green() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("v.unafs");
        make_vault(&vault);

        let mut m = Manifest::new();
        m.register("vault", &vault, BoltKind::Vault);
        let s = &m.status_all()[0];
        assert_eq!(s.crystal, CrystalColor::Green, "detail: {}", s.detail);
        assert!(s.detail.contains("mounted"));
    }

    #[test]
    fn vault_bolt_corrupt_is_red_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("corrupt.unafs");
        // Garbage large enough to reach the superblock parse (>= 1 block).
        let garbage: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
        fs::write(&vault, &garbage).expect("seed garbage");

        let mut m = Manifest::new();
        m.register("vault", &vault, BoltKind::Vault);
        let s = &m.status_all()[0];
        assert_eq!(s.crystal, CrystalColor::Red);

        // Fail-closed: the bytes must be byte-identical after the probe.
        let after = fs::read(&vault).expect("still exists");
        assert_eq!(after, garbage, "probe must not touch vault bytes");
    }

    #[test]
    fn detached_head_reports_red() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repo(dir.path(), "a.txt", "hello\n");
        let out = Command::new("git")
            .args(["checkout", "-q", "--detach"])
            .current_dir(dir.path())
            .output()
            .expect("git must be available");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

        let status = Vaire::look_at(dir.path()).expect("look must succeed");
        assert_eq!(status.branch, "DETACHED");
        assert_eq!(CrystalColor::from_git(&status), CrystalColor::Red);
    }
}
