// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAMETAL (LEDGER SR90): the sidebar — the workspace as a file tree (CODEX §4: the Vaire/Matrix sidebar's slot;
// here only the tree, the Matrix's file view, over the ring-3 file surface). The root is the workspace the
// command line names (argv[1]) or the session home (/home/<user>, SYS_WHOAMI). A directory is listed with
// SYS_PATH_READ + PATH_R_LIST when it is opened (names ending in `/` are directories, the surface's own mark), so
// the tree costs one listing per expanded directory and nothing for the rest. A listing the ACL refuses (PATH_R_LIST
// is home-scoped) is a red row under that directory naming the errno, never an empty tree.
//
// KEYS (focused): Up/Down move the selection; Enter or Right opens a directory (Enter again, or Left, closes it) and
// on a file asks the IDE to open it in the editor (the same as Ctrl+O); Left on a closed entry jumps to its parent.
//
// MATRIXCORE (SR99, R104): the listing is Matrix's own — `matrix_core::list_dir`, the rule the host Finder lists
// with (the build-noise set `target` / `.git` / `node_modules` left out, directories first, then files, each by
// name in byte order), over `PathVolume`: the core's `Volume` seam on SYS_PATH_READ + PATH_R_LIST (list),
// SYS_STAT (stat) and SYS_PATH_READ (read). The tree keeps its lazy shape: one listing per opened directory.

use crate::fs::{errno_word, list, read_file, resolve, stat, CAT_MAX};
use alloc::string::String;
use alloc::vec::Vec;
use matrix_core::{Entry, Ignore, Kind, Stat, Volume};
use una_abi::{KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};

/// The core's `Volume` over the ring-3 file surface. Paths are absolute (`/home/<user>/…`). A listing's names
/// carry the surface's own directory mark (a trailing `/`); it gives no size or link facts, so entries are
/// `Dir` or `File` with size 0 (the surface does not expose links to ring 3). The error is the -errno.
pub struct PathVolume;

impl Volume for PathVolume {
    type Error = i64;

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, i64> {
        Ok(list(dir)?
            .into_iter()
            .map(|n| {
                let is_dir = n.ends_with('/');
                let name = String::from(n.trim_end_matches('/'));
                Entry { name, kind: if is_dir { Kind::Dir } else { Kind::File }, size: 0 }
            })
            .collect())
    }

    fn stat(&mut self, path: &str) -> Option<Stat> {
        stat(path).ok().map(|s| {
            let kind = if s.kind == 1 { Kind::Dir } else { Kind::File };
            Stat { kind, size: if kind == Kind::File { s.size } else { 0 } }
        })
    }

    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        read_file(path, CAT_MAX, true).ok()
    }
}

/// Directory depth the tree indents to (deeper entries are listed, flush at this indent).
const DEPTH_MAX: u8 = 8;

pub struct Node {
    pub path: String,
    pub name: String,
    pub depth: u8,
    pub dir: bool,
    pub open: bool,
    /// A refusal row (the listing under the entry above failed): `name` carries the errno word.
    pub err: bool,
}

pub struct Tree {
    pub root: String,
    pub nodes: Vec<Node>,
    pub sel: usize,
    pub top: usize,
    pub dirty: bool,
}

/// What a key asks of the IDE.
pub enum Act {
    None,
    Open(String),
}

/// `dir` (absolute, normal) listed by the core: `.`/`..` and the build noise dropped, directories first, then
/// files, each by name in byte order (the surface's names are ASCII 8.3 or UTF-8); `path` = `join(dir, name)`.
fn children(dir: &str, depth: u8) -> Vec<Node> {
    match matrix_core::list_dir(&mut PathVolume, dir, &Ignore::build_noise()) {
        Ok(items) => items
            .into_iter()
            .map(|it| Node { path: it.path, name: it.name, depth, dir: it.kind == Kind::Dir, open: false, err: false })
            .collect(),
        Err(e) => alloc::vec![Node {
            path: String::from(dir),
            name: alloc::format!("({} {})", errno_word(e), e),
            depth,
            dir: false,
            open: false,
            err: true,
        }],
    }
}

impl Tree {
    pub fn new(root: String) -> Tree {
        // The core joins paths without folding, so the root is listed in its normal form (`resolve` from `/`).
        let nodes = children(&resolve("/", &root), 0);
        Tree { root, nodes, sel: 0, top: 0, dirty: true }
    }

    /// Entries that name a file or directory (refusal rows excluded) — the start line's `entries=`.
    pub fn entries(&self) -> usize {
        self.nodes.iter().filter(|n| !n.err).count()
    }

    /// The selected FILE, if the selection is one.
    pub fn selected_file(&self) -> Option<&str> {
        self.nodes.get(self.sel).filter(|n| !n.dir && !n.err).map(|n| n.path.as_str())
    }

    fn toggle(&mut self, i: usize) {
        let (open, depth, path) = {
            let n = &self.nodes[i];
            (n.open, n.depth, n.path.clone())
        };
        if open {
            let mut end = i + 1;
            while end < self.nodes.len() && self.nodes[end].depth > depth {
                end += 1;
            }
            self.nodes.drain(i + 1..end);
            self.nodes[i].open = false;
        } else {
            let kids = children(&path, depth.saturating_add(1));
            let tail = self.nodes.split_off(i + 1);
            self.nodes.extend(kids);
            self.nodes.extend(tail);
            self.nodes[i].open = true;
        }
    }

    /// Row `r` of the visible list (a click), selected.
    pub fn select_row(&mut self, r: usize) {
        let i = self.top + r;
        if i < self.nodes.len() {
            self.sel = i;
            self.dirty = true;
        }
    }

    /// The indent, in cells, of entry `i`.
    pub fn indent(&self, i: usize) -> usize {
        self.nodes[i].depth.min(DEPTH_MAX) as usize
    }

    /// Keep the selection inside `rows` visible rows.
    pub fn scroll_to_sel(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + rows {
            self.top = self.sel + 1 - rows;
        }
    }

    pub fn key(&mut self, k: u8) -> Act {
        if self.nodes.is_empty() {
            return Act::None;
        }
        let mut act = Act::None;
        match k {
            KEY_UP => self.sel = self.sel.saturating_sub(1),
            KEY_DOWN => self.sel = (self.sel + 1).min(self.nodes.len() - 1),
            b'\n' | b'\r' | KEY_RIGHT => {
                let n = &self.nodes[self.sel];
                if n.err {
                    return act;
                }
                if n.dir {
                    if !(k == KEY_RIGHT && n.open) {
                        self.toggle(self.sel);
                    }
                } else {
                    act = Act::Open(n.path.clone());
                }
            }
            KEY_LEFT => {
                let n = &self.nodes[self.sel];
                if n.dir && n.open {
                    self.toggle(self.sel);
                } else if n.depth > 0 {
                    let d = n.depth;
                    if let Some(p) = (0..self.sel).rev().find(|&j| self.nodes[j].depth < d) {
                        self.sel = p;
                    }
                }
            }
            _ => return act,
        }
        self.dirty = true;
        act
    }
}
