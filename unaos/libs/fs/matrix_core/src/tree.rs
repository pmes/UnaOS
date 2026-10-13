// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The workspace tree model: the genesis tree, the Finder's one-level listing, the sandboxed resolver, the
//! listing frame and the bare-name rule. Every rule here is the host handler's (handlers/matrix: lib.rs
//! `build_genesis_tree`, finder.rs `list` / `resolve` / `bare_name` / `parent_rel`), moved so both rings run it.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{Ignore, Kind, Volume, join};

/// One genesis-tree node. `id` is the path in the volume's namespace (workspace-relative on the host),
/// `label` the entry name, `dir` true for a directory node (which always has children: empty branches are
/// pruned).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub dir: bool,
    pub children: Vec<Node>,
}

/// One row of the Finder's listing: `path` is `join(dir, name)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub name: String,
    pub kind: Kind,
    pub size: u64,
}

/// The genesis tree under `dir`, ids in the volume's namespace (`join(dir, name)`). See [`genesis_ids`].
pub fn genesis<V: Volume>(vol: &mut V, dir: &str, ignore: &Ignore) -> Vec<Node> {
    genesis_ids(vol, dir, dir, ignore)
}

/// The genesis tree: list `dir` on `vol`, naming each node `join(id_base, name)` — the host's
/// `build_genesis_tree`, rule for rule:
///
/// * ignored entries (the build-noise set by default) are skipped, by name, before anything else;
/// * a symlink is NEVER followed (skipped — the all-asset scan broadens exposure to cycles), nor is a device,
///   socket or fifo (`Kind::Other`);
/// * a directory is walked first and kept only if it holds something (a branch with no leaves is pruned);
/// * a directory the volume refuses to list is an empty branch (pruned), the root's refusal an empty tree;
/// * order: directories first, then files, each by name in byte order (`String`'s `Ord`).
pub fn genesis_ids<V: Volume>(vol: &mut V, dir: &str, id_base: &str, ignore: &Ignore) -> Vec<Node> {
    let Ok(entries) = vol.list(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<Node> = Vec::new();
    let mut files: Vec<Node> = Vec::new();
    for e in entries {
        if e.name.is_empty() || e.name == "." || e.name == ".." {
            continue;
        }
        let path = join(dir, &e.name);
        if ignore.ignored(&path, &e.name, e.kind == Kind::Dir) {
            continue;
        }
        let id = join(id_base, &e.name);
        match e.kind {
            Kind::Dir => {
                let children = genesis_ids(vol, &path, &id, ignore);
                if !children.is_empty() {
                    dirs.push(Node { id, label: e.name, dir: true, children });
                }
            }
            Kind::File => files.push(Node { id, label: e.name, dir: false, children: Vec::new() }),
            Kind::Symlink | Kind::Other => {}
        }
    }
    dirs.sort_by(|a, b| a.label.cmp(&b.label));
    files.sort_by(|a, b| a.label.cmp(&b.label));
    dirs.append(&mut files);
    dirs
}

/// The Finder's listing of ONE directory: every child the ignore policy keeps (empty directories included —
/// a Finder shows what is there; symlinks shown as `Kind::Symlink`, never followed), directories first, then
/// everything else, each by name in byte order. The tree pane on the image lists with this too.
pub fn list_dir<V: Volume>(vol: &mut V, dir: &str, ignore: &Ignore) -> Result<Vec<Item>, V::Error> {
    let entries = vol.list(dir)?;
    let mut dirs: Vec<Item> = Vec::new();
    let mut rest: Vec<Item> = Vec::new();
    for e in entries {
        if e.name.is_empty() || e.name == "." || e.name == ".." {
            continue;
        }
        let path = join(dir, &e.name);
        if ignore.ignored(&path, &e.name, e.kind == Kind::Dir) {
            continue;
        }
        let size = if e.kind == Kind::File { e.size } else { 0 };
        let item = Item { path, name: e.name, kind: e.kind, size };
        if item.kind == Kind::Dir { dirs.push(item) } else { rest.push(item) }
    }
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    rest.sort_by(|a, b| a.name.cmp(&b.name));
    dirs.append(&mut rest);
    Ok(dirs)
}

/// The refusal reasons [`resolve`] gives, verbatim (the host's `FsOutcome::Denied` reasons).
pub const DENY_ABSOLUTE: &str = "absolute paths are not root-relative — refused";
pub const DENY_ESCAPE: &str = "path escapes the workspace root";
pub const DENY_SYMLINK: &str = "path crosses a symlink (never followed)";

/// Resolve a workspace-relative path to its normal form (`a/b`, `""` = the root), enforcing the sandbox: an
/// absolute path is refused (a caller error, never silently re-anchored), any `..` component is refused,
/// `""` and `.` components fold away, and no component that EXISTS may be a symlink (checked on the volume,
/// component by component — a missing component is not a refusal, so a create can validate its parent).
pub fn resolve<V: Volume>(vol: &mut V, rel: &str) -> Result<String, &'static str> {
    if rel.starts_with('/') {
        return Err(DENY_ABSOLUTE);
    }
    let mut acc = String::new();
    for comp in rel.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(DENY_ESCAPE);
        }
        acc = join(&acc, comp);
        if let Some(st) = vol.stat(&acc) {
            if st.kind == Kind::Symlink {
                return Err(DENY_SYMLINK);
            }
        }
    }
    Ok(acc)
}

/// The parent of a `/`-joined relative path (`""` for a top-level entry or the root itself).
pub fn parent_rel(rel: &str) -> String {
    rel.trim_end_matches('/').rsplit_once('/').map(|(p, _)| String::from(p)).unwrap_or_default()
}

/// A listing's breadcrumbs for normal relative path `rel`: `("", "")` for the root, then each descended
/// segment with its relative path. Returns `(parent, crumbs)`; `parent` is `None` at the root.
pub fn breadcrumbs(rel: &str) -> (Option<String>, Vec<(String, String)>) {
    let parent = if rel.is_empty() { None } else { Some(rel.rsplit_once('/').map(|(p, _)| String::from(p)).unwrap_or_default()) };
    let mut crumbs = alloc::vec![(String::new(), String::new())];
    if !rel.is_empty() {
        let mut acc = String::new();
        for seg in rel.split('/') {
            acc = join(&acc, seg);
            crumbs.push((String::from(seg), acc.clone()));
        }
    }
    (parent, crumbs)
}

/// A bare file name (a rename target, a new folder's name): trimmed, non-empty, not `.` / `..`, no `/`,
/// `\` or NUL. `None` when refused.
pub fn bare_name(name: &str) -> Option<&str> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." || n.contains('/') || n.contains('\\') || n.contains('\0') {
        return None;
    }
    Some(n)
}
