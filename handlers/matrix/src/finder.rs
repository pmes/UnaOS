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

//! The Finder capability — matrix as a file browser.
//!
//! Where [`crate::MatrixScanner::build_genesis_tree`] produces the RECURSIVE
//! code-topology DAG (pruning empty dirs, dropping symlinks, `.rs`-aware), the
//! Finder is a NAVIGABLE CURSOR: it lists ONE directory's immediate children
//! (empty dirs included — a Finder shows what is there) and executes the Finder
//! verbs (open, new folder, rename, copy, move, delete). It never re-scans the
//! whole tree to move one level; a per-directory `read_dir` is the right weight
//! for a cursor, and it deliberately does NOT prune, because pruning is a
//! code-map concern, not a file-manager one.
//!
//! ## Security posture
//!
//! Every path is anchored under the workspace root and resolved with
//! [`Finder::resolve`], which:
//!   * rejects `..` traversal and absolute escapes (Denied), and
//!   * never follows a symlink component (the genesis scan's symlink law holds
//!     for navigation and ops too).
//!
//! Every verb is principal-attributed on the bus (`Origin`), so an in-kernel
//! fulfilment would run with the invoker's grants (ROADMAP message-security
//! law). Writes that a read-only volume refuses surface as
//! [`FsOutcome::Denied`] — the loud FAT-verb-style refusal — never a silent
//! no-op. Deletes are reversible: they move to a workspace `.una-trash/` rather
//! than hard-deleting, honoring the destructive-action discipline.

use std::path::{Path, PathBuf};

use bandy::state::{BrowseEntry, BrowseKind, BrowseListing, FsOutcome, FsVerb};
use bandy::MatrixEvent;
use bandy::Origin;

/// Directory names the Finder never lists — the genesis build-noise set plus
/// the Finder's own trash bin.
const EXCLUDED: &[&str] = &["target", ".git", "node_modules", TRASH_DIR];

/// Workspace-relative name of the reversible trash bin.
const TRASH_DIR: &str = ".una-trash";

/// A file-browser cursor anchored at an absolute workspace root. All paths in
/// and out of the Finder are workspace-relative (`""` = the root itself).
pub struct Finder {
    root: PathBuf,
}

fn deny(reason: impl Into<String>) -> FsOutcome {
    FsOutcome::Denied { reason: reason.into() }
}

/// Map a filesystem error onto an outcome. A read-only volume / permission
/// denial becomes the LOUD `Denied` refusal (the FAT-verb posture on the host);
/// everything else is a genuine `Error`.
fn io_outcome(e: &std::io::Error, verb: &str) -> FsOutcome {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        return deny(format!("read-only or permission-denied volume: {verb} refused ({e})"));
    }
    // EROFS (30) can arrive with an unmapped ErrorKind on some platforms.
    #[cfg(unix)]
    if e.raw_os_error() == Some(30) {
        return deny(format!("read-only volume: {verb} refused (EROFS)"));
    }
    FsOutcome::Error { message: format!("{verb}: {e}") }
}

/// Validate a bare filename (rename target / new folder name): no separators,
/// no traversal, non-empty.
fn bare_name(name: &str) -> Result<&str, FsOutcome> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." || n.contains('/') || n.contains('\\') || n.contains('\0') {
        return Err(deny(format!("invalid name: {name:?}")));
    }
    Ok(n)
}

impl Finder {
    /// Anchor a Finder at an absolute workspace root.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The absolute root this Finder is anchored at.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a workspace-relative path to an absolute one, enforcing the
    /// sandbox: no `..`/absolute escape, no symlink component ever followed.
    /// Intermediate components must already exist (a symlink can only be
    /// detected on a real path), so this is for EXISTING targets and existing
    /// parent directories — a create validates its parent with `resolve` and
    /// appends a `bare_name`.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, FsOutcome> {
        let rel = rel.trim_start_matches('/');
        let mut acc = self.root.clone();
        if rel.is_empty() {
            return Ok(acc);
        }
        for comp in rel.split('/') {
            if comp.is_empty() || comp == "." {
                continue;
            }
            if comp == ".." {
                return Err(deny("path escapes the workspace root"));
            }
            acc.push(comp);
            // Symlink guard: never follow a link component (the genesis law,
            // extended to navigation and ops).
            if let Ok(meta) = std::fs::symlink_metadata(&acc) {
                if meta.file_type().is_symlink() {
                    return Err(deny("path crosses a symlink (never followed)"));
                }
            }
        }
        Ok(acc)
    }

    /// Workspace-relative form of an absolute path under the root.
    fn rel_of(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.root)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/")
    }

    // --- NAVIGATION ---------------------------------------------------------

    /// List one directory's immediate children as a browse-view [`BrowseListing`].
    /// Dirs first, then files, each alphabetical. Build-noise and the trash bin
    /// are excluded; symlinks are shown (flagged) but classified without being
    /// followed.
    pub fn list(&self, rel: &str) -> Result<BrowseListing, FsOutcome> {
        let dir = self.resolve(rel)?;
        let meta = std::fs::symlink_metadata(&dir).map_err(|e| io_outcome(&e, "list"))?;
        if !meta.file_type().is_dir() {
            return Err(deny("not a directory"));
        }

        let mut dirs: Vec<BrowseEntry> = Vec::new();
        let mut files: Vec<BrowseEntry> = Vec::new();

        let read = std::fs::read_dir(&dir).map_err(|e| io_outcome(&e, "list"))?;
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if EXCLUDED.contains(&name.as_str()) {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            let is_symlink = ft.is_symlink();
            let abs = entry.path();
            let rel_path = self.rel_of(&abs);

            if is_symlink {
                // Classify the link by ITS OWN type — never the target's.
                files.push(BrowseEntry {
                    path: rel_path,
                    name,
                    kind: BrowseKind::Other,
                    size: 0,
                    is_symlink: true,
                });
            } else if ft.is_dir() {
                dirs.push(BrowseEntry { path: rel_path, name, kind: BrowseKind::Dir, size: 0, is_symlink: false });
            } else if ft.is_file() {
                // `entry.metadata()` does not traverse symlinks; here the entry
                // is a plain file anyway.
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push(BrowseEntry { path: rel_path, name, kind: BrowseKind::File, size, is_symlink: false });
            } else {
                files.push(BrowseEntry { path: rel_path, name, kind: BrowseKind::Other, size: 0, is_symlink: false });
            }
        }

        dirs.sort_by(|a, b| a.name.cmp(&b.name));
        files.sort_by(|a, b| a.name.cmp(&b.name));
        dirs.append(&mut files);

        let rel_norm = self.rel_of(&dir);
        let parent = if rel_norm.is_empty() {
            None
        } else {
            Some(
                rel_norm
                    .rsplit_once('/')
                    .map(|(p, _)| p.to_string())
                    .unwrap_or_default(),
            )
        };

        // Breadcrumbs: root first, then each descended segment with its rel path.
        let mut breadcrumbs = vec![(String::new(), String::new())];
        if !rel_norm.is_empty() {
            let mut acc = String::new();
            for seg in rel_norm.split('/') {
                acc = if acc.is_empty() { seg.to_string() } else { format!("{acc}/{seg}") };
                breadcrumbs.push((seg.to_string(), acc.clone()));
            }
        }

        Ok(BrowseListing { path: rel_norm, parent, breadcrumbs, entries: dirs })
    }

    // --- FILE OPERATIONS ----------------------------------------------------

    /// Resolve, validate as a file, and answer `Ok` — the vessel routes the
    /// path to the editor. Opening a directory is refused (navigate instead).
    pub fn open(&self, rel: &str) -> FsOutcome {
        let abs = match self.resolve(rel) {
            Ok(p) => p,
            Err(o) => return o,
        };
        match std::fs::symlink_metadata(&abs) {
            Ok(m) if m.file_type().is_file() => FsOutcome::Ok { path: self.rel_of(&abs) },
            Ok(_) => deny("open targets a file; use BrowseTo to open a directory"),
            Err(e) => io_outcome(&e, "open"),
        }
    }

    /// Create a new directory `name` inside `parent_rel`.
    pub fn new_folder(&self, parent_rel: &str, name: &str) -> FsOutcome {
        let parent = match self.resolve(parent_rel) {
            Ok(p) => p,
            Err(o) => return o,
        };
        if !parent.is_dir() {
            return deny("parent is not a directory");
        }
        let name = match bare_name(name) {
            Ok(n) => n,
            Err(o) => return o,
        };
        let target = parent.join(name);
        if target.exists() {
            return deny(format!("already exists: {name}"));
        }
        match std::fs::create_dir(&target) {
            Ok(()) => FsOutcome::Ok { path: self.rel_of(&target) },
            Err(e) => io_outcome(&e, "new_folder"),
        }
    }

    /// Rename `path_rel` to the bare name `new_name` within the same parent.
    pub fn rename(&self, path_rel: &str, new_name: &str) -> FsOutcome {
        let src = match self.resolve(path_rel) {
            Ok(p) => p,
            Err(o) => return o,
        };
        if src == self.root {
            return deny("cannot rename the workspace root");
        }
        if !src.exists() {
            return deny(format!("no such path: {path_rel}"));
        }
        let name = match bare_name(new_name) {
            Ok(n) => n,
            Err(o) => return o,
        };
        let Some(parent) = src.parent() else {
            return deny("cannot rename the workspace root");
        };
        let dst = parent.join(name);
        if dst.exists() {
            return deny(format!("target already exists: {name}"));
        }
        match std::fs::rename(&src, &dst) {
            Ok(()) => FsOutcome::Ok { path: self.rel_of(&dst) },
            Err(e) => io_outcome(&e, "rename"),
        }
    }

    /// Copy `src_rel` into the directory `dst_dir_rel`.
    pub fn copy(&self, src_rel: &str, dst_dir_rel: &str) -> FsOutcome {
        let src = match self.resolve(src_rel) {
            Ok(p) => p,
            Err(o) => return o,
        };
        let dst_dir = match self.resolve(dst_dir_rel) {
            Ok(p) => p,
            Err(o) => return o,
        };
        if src == self.root {
            return deny("cannot copy the workspace root");
        }
        if !src.exists() {
            return deny(format!("no such path: {src_rel}"));
        }
        if !dst_dir.is_dir() {
            return deny("destination is not a directory");
        }
        // Refuse copying a directory into itself or a descendant.
        if dst_dir == src || dst_dir.starts_with(&src) {
            return deny("cannot copy a directory into itself");
        }
        let Some(name) = src.file_name() else {
            return deny("source has no file name");
        };
        let dst = dst_dir.join(name);
        if dst.exists() {
            return deny(format!("target already exists: {}", name.to_string_lossy()));
        }
        let res = if src.is_dir() {
            copy_dir_recursive(&src, &dst)
        } else {
            std::fs::copy(&src, &dst).map(|_| ())
        };
        match res {
            Ok(()) => FsOutcome::Ok { path: self.rel_of(&dst) },
            Err(e) => io_outcome(&e, "copy"),
        }
    }

    /// Move `src_rel` into the directory `dst_dir_rel`.
    pub fn mv(&self, src_rel: &str, dst_dir_rel: &str) -> FsOutcome {
        let src = match self.resolve(src_rel) {
            Ok(p) => p,
            Err(o) => return o,
        };
        let dst_dir = match self.resolve(dst_dir_rel) {
            Ok(p) => p,
            Err(o) => return o,
        };
        if src == self.root {
            return deny("cannot move the workspace root");
        }
        if !src.exists() {
            return deny(format!("no such path: {src_rel}"));
        }
        if !dst_dir.is_dir() {
            return deny("destination is not a directory");
        }
        if dst_dir == src || dst_dir.starts_with(&src) {
            return deny("cannot move a directory into itself");
        }
        let Some(name) = src.file_name() else {
            return deny("source has no file name");
        };
        let dst = dst_dir.join(name);
        if dst.exists() {
            return deny(format!("target already exists: {}", name.to_string_lossy()));
        }
        match std::fs::rename(&src, &dst) {
            Ok(()) => FsOutcome::Ok { path: self.rel_of(&dst) },
            Err(e) => io_outcome(&e, "move"),
        }
    }

    /// Delete `path_rel`. Requires `confirmed` (else `NeedsConfirm`). A
    /// confirmed delete is REVERSIBLE: it moves the target into the workspace
    /// `.una-trash/`, never hard-deleting.
    pub fn delete(&self, path_rel: &str, confirmed: bool) -> FsOutcome {
        if !confirmed {
            return FsOutcome::NeedsConfirm;
        }
        let src = match self.resolve(path_rel) {
            Ok(p) => p,
            Err(o) => return o,
        };
        if src == self.root {
            return deny("cannot delete the workspace root");
        }
        if !src.exists() {
            return deny(format!("no such path: {path_rel}"));
        }
        let trash = self.root.join(TRASH_DIR);
        if let Err(e) = std::fs::create_dir_all(&trash) {
            return io_outcome(&e, "delete");
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "item".to_string());
        let dst = trash.join(format!("{stamp}-{base}"));
        match std::fs::rename(&src, &dst) {
            Ok(()) => FsOutcome::Ok { path: self.rel_of(&dst) },
            Err(e) => io_outcome(&e, "delete"),
        }
    }

    /// Run one verb by tag. `arg` is the second operand (new name / dest dir).
    pub fn run_verb(&self, verb: FsVerb, path: &str, arg: Option<&str>, confirmed: bool) -> FsOutcome {
        match verb {
            FsVerb::Open => self.open(path),
            FsVerb::NewFolder => match arg {
                Some(name) => self.new_folder(path, name),
                None => deny("new_folder requires a name"),
            },
            FsVerb::Rename => match arg {
                Some(name) => self.rename(path, name),
                None => deny("rename requires a new name"),
            },
            FsVerb::Copy => match arg {
                Some(dst) => self.copy(path, dst),
                None => deny("copy requires a destination directory"),
            },
            FsVerb::Move => match arg {
                Some(dst) => self.mv(path, dst),
                None => deny("move requires a destination directory"),
            },
            FsVerb::Delete => self.delete(path, confirmed),
        }
    }

    /// The directory whose listing should refresh after a successful `verb`.
    fn refresh_dir(&self, verb: FsVerb, path: &str, arg: Option<&str>) -> String {
        match verb {
            // NewFolder's `path` IS the parent directory.
            FsVerb::NewFolder => path.to_string(),
            // Copy/Move land in the destination directory (`arg`).
            FsVerb::Copy | FsVerb::Move => arg.unwrap_or("").to_string(),
            // Rename/Delete change the target's own parent.
            FsVerb::Rename | FsVerb::Delete => parent_rel(path),
            FsVerb::Open => path.to_string(),
        }
    }

    /// Translate a Finder request event into the events to publish. Returns
    /// empty for events this handler does not own. This is the pure seam the
    /// async `ignite` loop fires onto the bus, and the unit tests drive
    /// directly.
    pub fn dispatch(&self, event: &MatrixEvent) -> Vec<MatrixEvent> {
        match event {
            MatrixEvent::BrowseTo { principal, path } => match self.list(path) {
                Ok(listing) => vec![MatrixEvent::DirListed(listing)],
                Err(outcome) => vec![MatrixEvent::FsOpResult {
                    principal: principal.clone(),
                    verb: FsVerb::Open,
                    path: path.clone(),
                    outcome,
                }],
            },
            MatrixEvent::FileOp { principal, verb, path, arg, confirmed } => {
                let outcome = self.run_verb(*verb, path, arg.as_deref(), *confirmed);
                let mut out = vec![MatrixEvent::FsOpResult {
                    principal: principal.clone(),
                    verb: *verb,
                    path: path.clone(),
                    outcome: outcome.clone(),
                }];
                // A successful mutation refreshes the affected directory so the
                // browse view stays live without the UI re-requesting it.
                if verb.is_write() {
                    if let FsOutcome::Ok { .. } = outcome {
                        let dir = self.refresh_dir(*verb, path, arg.as_deref());
                        if let Ok(listing) = self.list(&dir) {
                            out.push(MatrixEvent::DirListed(listing));
                        }
                    }
                }
                out
            }
            _ => Vec::new(),
        }
    }
}

/// Is `event` a Finder request this handler should service?
pub fn is_finder_request(event: &MatrixEvent) -> bool {
    matches!(event, MatrixEvent::BrowseTo { .. } | MatrixEvent::FileOp { .. })
}

/// Convenience: the principal an event carries (for logging/attribution).
pub fn event_principal(event: &MatrixEvent) -> Option<&Origin> {
    match event {
        MatrixEvent::BrowseTo { principal, .. }
        | MatrixEvent::FileOp { principal, .. }
        | MatrixEvent::FsOpResult { principal, .. } => Some(principal),
        _ => None,
    }
}

/// Workspace-relative parent of a `/`-joined relative path (`""` for a
/// top-level entry or the root itself).
fn parent_rel(rel: &str) -> String {
    rel.trim_end_matches('/')
        .rsplit_once('/')
        .map(|(p, _)| p.to_string())
        .unwrap_or_default()
}

/// Recursively copy `src` dir to `dst` (created fresh). Symlinks encountered
/// inside are skipped (never followed), consistent with the Finder's law.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
