// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! MATRIXCORE (LEDGER SR99) — Matrix's core: The Map, as bytes both rings link.
//!
//! CHARTER: Matrix owns The Map — files, the workspace indexer and finder (docs/CODEX.md §2); seam
//! `shared-core` (R104: this core is INSERTED beside the host face — `handlers/matrix` keeps its std volume,
//! its bus loop and the GTK vessels; UNA.ELF's tree pane links the same core over SYS_PATH_*).
//!
//! `no_std` + `alloc`, zero dependencies, no `unsafe`. What is here:
//!
//! * [`Volume`] — the one I/O seam (list one directory, stat one path without following a link, read one
//!   file), the way midden_core has one. [`MemVolume`] is the fixture implementation, present in the core
//!   (not behind `cfg(test)`) so a host KAT and a ring-3 witness prove the same walk.
//! * [`tree`] — the genesis tree (recursive, empty branches pruned, links never followed, dirs first then
//!   files in byte order), the Finder's one-level listing (empty dirs shown, links flagged), the sandboxed
//!   path resolver, the listing frame (parent, breadcrumbs) and the bare-name rule.
//! * [`ignore`] — the ignore rules: the build-noise set (`target`, `.git`, `node_modules`), hidden files,
//!   and a `.gitignore` subset.
//! * [`index`] — the token indexer: the literal-aware comment stripper, the statement splitter, the `use` /
//!   `mod` dependency extractor, the symbol pass, and the `DICTIONARY$EDGES` payload with its semantic DAG.
//! * [`query`] — the query model: find a name in a tree, ranked exact > prefix > substring > subsequence,
//!   ties kept in the tree's own (Finder) order.
//!
//! Paths inside the core are `/`-joined strings in the volume's own namespace: workspace-relative on the
//! host (`""` = the root), absolute on the image (`/home/<user>/…`). [`join`] is the one joiner.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod ignore;
pub mod index;
pub mod query;
pub mod tree;

pub use ignore::{BUILD_NOISE, Ignore};
pub use index::{Indexer, ScanDepth};
pub use query::{Hit, Rank, query};
pub use tree::{Item, Node, bare_name, breadcrumbs, genesis, genesis_ids, list_dir, parent_rel, resolve};

/// What a directory entry is, classified by ITS OWN type — a symlink is a `Symlink`, never its target's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Symlink,
    /// A device, socket, fifo — listed by the Finder, never walked or indexed.
    Other,
}

/// One child of a listed directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// Bytes for a `File`; 0 for everything else (and wherever the volume does not know it).
    pub size: u64,
}

/// One path's facts, read without following a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub size: u64,
}

/// The I/O seam. The host implements it over `std::fs` (handlers/matrix), the image over SYS_PATH_READ +
/// PATH_R_LIST and SYS_STAT (crates/user-una), the KATs over [`MemVolume`].
pub trait Volume {
    type Error;
    /// The immediate children of `dir`, in the volume's own order (the core sorts where an order is part of
    /// the contract). Never `.` or `..` (the core drops them if a volume yields them anyway).
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, Self::Error>;
    /// `path`'s facts without following a final link; `None` when it does not exist or cannot be read.
    fn stat(&mut self, path: &str) -> Option<Stat>;
    /// `path`'s whole contents; `None` when it cannot be read.
    fn read(&mut self, path: &str) -> Option<Vec<u8>>;
}

/// `name` inside `dir`: `""` + `a` = `a`, `/` + `a` = `/a`, `x/` + `a` = `x/a`.
pub fn join(dir: &str, name: &str) -> String {
    let d = dir.trim_end_matches('/');
    let mut out = String::with_capacity(d.len() + 1 + name.len());
    if d.is_empty() {
        if dir.starts_with('/') {
            out.push('/');
        }
    } else {
        out.push_str(d);
        out.push('/');
    }
    out.push_str(name);
    out
}

/// The last component of a `/`-joined path.
pub fn base_name(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    p.rsplit_once('/').map_or(p, |(_, b)| b)
}

/// The fixture [`Volume`]: a flat set of `(path, kind, contents)` rows in the volume's own namespace. A
/// directory a file's path implies need not be listed; `list` yields children in insertion order (the way a
/// directory's on-disk order is not sorted), so a KAT proves the core does its own ordering.
pub struct MemVolume {
    rows: Vec<(String, Kind, Vec<u8>)>,
}

impl MemVolume {
    pub fn new() -> MemVolume {
        MemVolume { rows: Vec::new() }
    }

    /// Add a file with `contents` (its parent directories are implied).
    pub fn file(mut self, path: &str, contents: &[u8]) -> MemVolume {
        self.rows.push((String::from(path.trim_matches('/')), Kind::File, contents.to_vec()));
        self
    }

    /// Add a directory (needed only for an EMPTY one; parents of files are implied).
    pub fn dir(mut self, path: &str) -> MemVolume {
        self.rows.push((String::from(path.trim_matches('/')), Kind::Dir, Vec::new()));
        self
    }

    /// Add a symlink (never followed by the core; its target is not modelled).
    pub fn link(mut self, path: &str) -> MemVolume {
        self.rows.push((String::from(path.trim_matches('/')), Kind::Symlink, Vec::new()));
        self
    }

    fn kind_of(&self, path: &str) -> Option<(Kind, u64)> {
        let p = path.trim_matches('/');
        if p.is_empty() {
            return Some((Kind::Dir, 0));
        }
        for (rp, k, c) in &self.rows {
            if rp == p {
                return Some((*k, if *k == Kind::File { c.len() as u64 } else { 0 }));
            }
        }
        // An implied directory: some row lives under it.
        let mut pre = String::from(p);
        pre.push('/');
        if self.rows.iter().any(|(rp, _, _)| rp.starts_with(pre.as_str())) {
            return Some((Kind::Dir, 0));
        }
        None
    }
}

impl Default for MemVolume {
    fn default() -> Self {
        Self::new()
    }
}

impl Volume for MemVolume {
    /// The only refusal a fixture has: the directory is not there (or is not a directory).
    type Error = ();

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, ()> {
        match self.kind_of(dir) {
            Some((Kind::Dir, _)) => {}
            _ => return Err(()),
        }
        let d = dir.trim_matches('/');
        let mut out: Vec<Entry> = Vec::new();
        for (rp, _, _) in &self.rows {
            let rest = if d.is_empty() {
                rp.as_str()
            } else {
                match rp.strip_prefix(d).and_then(|r| r.strip_prefix('/')) {
                    Some(r) => r,
                    None => continue,
                }
            };
            if rest.is_empty() {
                continue;
            }
            let name = rest.split('/').next().unwrap_or(rest);
            if out.iter().any(|e| e.name == name) {
                continue;
            }
            let child = join(d, name);
            let (kind, size) = self.kind_of(&child).unwrap_or((Kind::Other, 0));
            out.push(Entry { name: String::from(name), kind, size });
        }
        Ok(out)
    }

    fn stat(&mut self, path: &str) -> Option<Stat> {
        self.kind_of(path).map(|(kind, size)| Stat { kind, size })
    }

    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        let p = path.trim_matches('/');
        self.rows.iter().find(|(rp, k, _)| rp == p && *k == Kind::File).map(|(_, _, c)| c.clone())
    }
}
