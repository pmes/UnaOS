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
// FILE OPERATIONS — UNAIDE2 (LEDGER SR111). `n` new file, `f` new folder, `r` rename (the IDE asks the name in the
// editor's status line), Backspace/Delete trash. Only the doors ring 3 has are used: SYS_PATH_WRITE's three verbs
// (fs.rs: create-or-truncate, make-parents, unlink). New file = an empty PATH_W_TRUNC write; new folder = fs::mkdir
// (MKDIRS over `dir/.keep`, then its unlink); rename of a FILE = copy then unlink (`via=copy`: a new object — its id,
// attributes and grants do not follow); rename of a FOLDER is refused by name (no ring-3 rename door: SYS_RENAME is
// the FAT root-name verb, the mount table's rename has no syscall). TRASH is trash_core's rule (Matrix, B449), whose
// `store` RENAMES the object into <home>/.Trash — with no ring-3 rename it cannot run here, so a delete is REFUSED BY
// NAME, never replaced by a permanent unlink. After each op the tree is rebuilt with its open folders re-opened and
// the selection on the touched path. Wire: `:: UNA: tree op=<newfile|newdir|rename|trash> path=<p> [to=<p>
// via=copy] rc=<r> [why=<w>] ::`.
//
// MATRIXCORE (SR99, R104): the listing is Matrix's own — `matrix_core::list_dir`, the rule the host Finder lists
// with (the build-noise set `target` / `.git` / `node_modules` left out, directories first, then files, each by
// name in byte order), over `PathVolume`: the core's `Volume` seam on SYS_PATH_READ + PATH_R_LIST (list),
// SYS_STAT (stat) and SYS_PATH_READ (read). The tree keeps its lazy shape: one listing per opened directory.

use crate::fs::{errno_word, list, mkdir, read_file, resolve, stat, unlink, write_file, CAT_MAX};
use crate::Line;
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
    /// UNAIDE2: ask a name for this operation (the IDE's status-line prompt).
    Ask(Op),
    /// UNAIDE2: trash the selection (refused by name: `Tree::trash`).
    Trash,
}

/// UNAIDE2: the file operations that need a name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    NewFile,
    NewDir,
    Rename,
}

/// UNAIDE2: the largest file a rename copies.
const COPY_MAX: usize = 16 * 1024 * 1024;

/// UNAIDE2: a leaf name the ops accept — printable ASCII, no `/`, not `.`/`..`.
fn name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 255 && n != "." && n != ".." && n.bytes().all(|b| (0x20..=0x7e).contains(&b) && b != b'/')
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        alloc::format!("{}{}", dir, name)
    } else {
        alloc::format!("{}/{}", dir, name)
    }
}

/// The folder holding `path` (`/` for a root entry).
fn parent(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(i) => String::from(&path[..i]),
    }
}

/// `:: UNA: tree op=<op> path=<p> [to=<p> via=copy] rc=<r> [why=<w>] ::`
fn op_wire(op: &str, path: &str, to: Option<&str>, rc: i64, why: Option<&str>) {
    let mut l = Line::new(b":: UNA: tree op=");
    l.put(op.as_bytes()).put(b" path=").put(path.as_bytes());
    if let Some(t) = to {
        l.put(b" to=").put(t.as_bytes()).put(b" via=copy");
    }
    l.put(b" rc=").dec(rc);
    if let Some(w) = why {
        l.put(b" why=").put(w.as_bytes());
    }
    l.put(b" ::");
    l.wire();
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

    /// UNAIDE2: the directory an op lands in — the selected folder, the selected file's folder, else the root.
    pub fn target_dir(&self) -> String {
        match self.nodes.get(self.sel) {
            Some(n) if n.dir && !n.err => n.path.clone(),
            Some(n) if !n.err => parent(&n.path),
            _ => resolve("/", &self.root),
        }
    }

    /// UNAIDE2: the selected entry's path and whether it is a folder (refusal rows excluded).
    pub fn selected(&self) -> Option<(String, bool)> {
        self.nodes.get(self.sel).filter(|n| !n.err).map(|n| (n.path.clone(), n.dir))
    }

    /// UNAIDE2: list the root again, re-open every folder that was open, and select `select` when it is listed.
    pub fn rebuild(&mut self, select: Option<&str>) {
        let open: Vec<String> = self.nodes.iter().filter(|n| n.dir && n.open).map(|n| n.path.clone()).collect();
        let keep = self.nodes.get(self.sel).map(|n| n.path.clone());
        self.nodes = children(&resolve("/", &self.root), 0);
        let mut i = 0;
        while i < self.nodes.len() {
            if self.nodes[i].dir && !self.nodes[i].err && open.iter().any(|p| *p == self.nodes[i].path) {
                self.toggle(i);
            }
            i += 1;
        }
        let want = select.map(String::from).or(keep);
        self.sel = want.and_then(|w| self.nodes.iter().position(|n| n.path == w)).unwrap_or(0).min(self.nodes.len().saturating_sub(1));
        self.dirty = true;
    }

    /// UNAIDE2: an empty file `name` in `dir` (one PATH_W_TRUNC write with no data). The new path, or -errno.
    pub fn new_file(&mut self, dir: &str, name: &str) -> Result<String, i64> {
        if !name_ok(name) {
            return Err(una_abi::EINVAL);
        }
        let p = join(dir, name);
        let r = if stat(&p).is_ok() { Err(una_abi::EEXIST) } else { write_file(&p, &[]).map(|_| ()) };
        op_wire("newfile", &p, None, r.err().unwrap_or(0), None);
        r?;
        self.rebuild(Some(&p));
        Ok(p)
    }

    /// UNAIDE2: an empty folder `name` in `dir` (fs::mkdir). The new path, or -errno.
    pub fn new_dir(&mut self, dir: &str, name: &str) -> Result<String, i64> {
        if !name_ok(name) {
            return Err(una_abi::EINVAL);
        }
        let p = join(dir, name);
        let r = if stat(&p).is_ok() { Err(una_abi::EEXIST) } else { mkdir(&p) };
        op_wire("newdir", &p, None, r.err().unwrap_or(0), None);
        r?;
        self.rebuild(Some(&p));
        Ok(p)
    }

    /// UNAIDE2: rename the FILE `path` to `name` in its folder — copied whole, then the original unlinked (a new
    /// object: id, attributes and grants do not follow). A folder is refused (`ENOSYS`: no ring-3 rename door).
    /// `Ok(new path)`; `Err((errno, why))` — `why` names a refusal or a half-done copy.
    pub fn rename(&mut self, path: &str, name: &str) -> Result<String, (i64, Option<&'static str>)> {
        if !name_ok(name) {
            return Err((una_abi::EINVAL, None));
        }
        let to = join(&parent(path), name);
        let r = copy_rename(path, &to);
        let (rc, why) = match r {
            Ok(()) => (0, None),
            Err((e, w)) => (e, w),
        };
        op_wire("rename", path, Some(&to), rc, why);
        r?;
        self.rebuild(Some(&to));
        Ok(to)
    }

    /// UNAIDE2: trash the selection — REFUSED BY NAME. trash_core's `store` renames the object into <home>/.Trash and
    /// ring 3 has no path rename (SYS_PATH_WRITE: truncate / make-parents / unlink; SYS_RENAME: FAT root names), so
    /// the Trash rule cannot run here; a permanent unlink is not offered in its place. Returns the refusal errno.
    pub fn trash(&mut self) -> i64 {
        let p = self.selected().map_or_else(|| resolve("/", &self.root), |s| s.0);
        op_wire("trash", &p, None, una_abi::ENOSYS, Some("no-ring3-rename-door-for-trash_core"));
        una_abi::ENOSYS
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
            return match k {
                b'n' => Act::Ask(Op::NewFile),
                b'f' => Act::Ask(Op::NewDir),
                _ => Act::None,
            };
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
            b'n' => return Act::Ask(Op::NewFile),
            b'f' => return Act::Ask(Op::NewDir),
            b'r' if !self.nodes[self.sel].err => return Act::Ask(Op::Rename),
            0x08 | 0x7f => return Act::Trash,
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

/// UNAIDE2: the rename body — a FILE copied whole to `to`, then `path` unlinked. `why` names a refusal (a folder) or a
/// half-done copy (the original kept).
fn copy_rename(path: &str, to: &str) -> Result<(), (i64, Option<&'static str>)> {
    match stat(path) {
        Ok(s) if s.kind == 1 => return Err((una_abi::ENOSYS, Some("folder-no-ring3-rename-door"))),
        Ok(_) => {}
        Err(e) => return Err((e, None)),
    }
    if stat(to).is_ok() {
        return Err((una_abi::EEXIST, None));
    }
    let bytes = read_file(path, COPY_MAX, true).map_err(|e| (e, None))?;
    if let Err(e) = write_file(to, &bytes) {
        let _ = unlink(to);
        return Err((e, None));
    }
    unlink(path).map_err(|e| (e, Some("copied-original-kept")))
}
