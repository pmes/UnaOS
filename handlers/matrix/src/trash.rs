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

//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! TRASHTIME M3 (B308): the Trash on a UnaFS vault, the host twin of the kernel's `fs/trash.rs`.
//!
//! A trashed object IS an object carrying three attributes, whose key literals live ONCE in
//! `una-abi` (linked by both rings): `una:trash-origin` (String, the original absolute vault path),
//! `una:trash-time` (Int, unix seconds) and `una:trash-by` (String, the user). Trashing stamps them
//! and renames the object into `/home/<user>/.Trash/` (a UnaFS rename is a rekey: same inode id);
//! the listing is `query("una:trash-origin != \"\"")` scoped to that folder's direct children;
//! restore finds the trashed name's inode id and moves the id's CURRENT path back to the origin;
//! empty drops the keys and unlinks. No index file, no second store: the kernel Finder (Quarry) and
//! this one agree on what a trashed file is because they read the same attributes.

use una_abi::{ATTR_KEY_TRASH_BY, ATTR_KEY_TRASH_ORIGIN, ATTR_KEY_TRASH_TIME, TRASH_DIR_NAME, TRASH_QUERY};
use unafs::{AttributeValue, BlockDevice, FileKind, UnaFS};

/// One trashed object, as the query finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashItem {
    pub id: u64,
    /// The name it carries inside the Trash now.
    pub name: String,
    /// `una:trash-origin`.
    pub origin: String,
    /// `una:trash-time`.
    pub when: i64,
    /// `una:trash-by`.
    pub by: String,
}

/// The verbs a Finder routes to whatever Trash it carries.
pub trait TrashStore: Send {
    fn trash(&mut self, path: &str) -> Result<String, String>;
    fn restore(&mut self, name: &str) -> Result<String, String>;
    fn empty(&mut self) -> Result<usize, String>;
    fn items(&mut self) -> Result<Vec<TrashItem>, String>;
}

/// The Trash of one user on one UnaFS vault.
pub struct VaultTrash<D: BlockDevice> {
    fs: UnaFS<D>,
    user: String,
}

fn e<E: core::fmt::Debug>(x: E) -> String {
    format!("{x:?}")
}

fn leaf(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn parent(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &p[..i],
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl<D: BlockDevice> VaultTrash<D> {
    pub fn new(fs: UnaFS<D>, user: &str) -> Self {
        Self { fs, user: user.to_string() }
    }

    /// The vault, for callers (and tests) that need the filesystem itself.
    pub fn fs(&mut self) -> &mut UnaFS<D> {
        &mut self.fs
    }

    pub fn home(&self) -> String {
        format!("/home/{}", self.user)
    }

    pub fn trash_dir(&self) -> String {
        format!("{}/{}", self.home(), TRASH_DIR_NAME)
    }

    fn ensure_dir(&mut self, path: &str) -> Result<u64, String> {
        if let Ok(id) = self.fs.resolve_path(path) {
            return Ok(id);
        }
        let pid = self.ensure_dir(parent(path))?;
        self.fs.mkdir(pid, leaf(path).to_string()).map_err(e)
    }

    fn str_attr(&mut self, id: u64, key: &str) -> String {
        match self.fs.get_attribute(id, key) {
            Ok(Some(AttributeValue::String(s))) => s,
            _ => String::new(),
        }
    }

    fn strip(&mut self, id: u64) {
        for k in [ATTR_KEY_TRASH_ORIGIN, ATTR_KEY_TRASH_TIME, ATTR_KEY_TRASH_BY] {
            let _ = self.fs.remove_attribute(id, k);
        }
    }

    fn remove_tree(&mut self, path: &str, depth: usize) -> Result<(), String> {
        if depth > 32 {
            return Err("too-deep".into());
        }
        let id = self.fs.resolve_path(path).map_err(e)?;
        let pid = self.fs.resolve_path(parent(path)).map_err(e)?;
        if self.fs.read_inode(id).map_err(e)?.kind == FileKind::Directory {
            for c in self.fs.ls(id).map_err(e)? {
                if c.name == "." || c.name == ".." {
                    continue;
                }
                self.remove_tree(&format!("{path}/{}", c.name), depth + 1)?;
            }
            self.fs.rmdir(pid, leaf(path)).map(|_| ()).map_err(e)
        } else {
            self.fs.unlink(pid, leaf(path)).map(|_| ()).map_err(e)
        }
    }
}

impl<D: BlockDevice + Send> TrashStore for VaultTrash<D> {
    fn trash(&mut self, path: &str) -> Result<String, String> {
        let home = self.home();
        let td = self.trash_dir();
        if path.split('/').any(|c| c == "..") || !path.starts_with(&format!("{home}/")) {
            return Err(format!("files are trashed only under {home}"));
        }
        if path == td || path.starts_with(&format!("{td}/")) {
            return Err("already in the trash".into());
        }
        let id = self.fs.resolve_path(path).map_err(|_| format!("no such path: {path}"))?;
        let tid = self.ensure_dir(&td)?;
        let mut name = leaf(path).to_string();
        let mut n = 0u32;
        while self.fs.resolve_path(&format!("{td}/{name}")).is_ok() {
            n += 1;
            if n > 99 {
                return Err("too-many-collisions".into());
            }
            name = format!("{}~{n}", leaf(path));
        }
        self.fs.set_attribute(id, ATTR_KEY_TRASH_ORIGIN.into(), AttributeValue::String(path.into())).map_err(e)?;
        self.fs.set_attribute(id, ATTR_KEY_TRASH_TIME.into(), AttributeValue::Int(now_secs())).map_err(e)?;
        self.fs.set_attribute(id, ATTR_KEY_TRASH_BY.into(), AttributeValue::String(self.user.clone())).map_err(e)?;
        let pid = self.fs.resolve_path(parent(path)).map_err(e)?;
        if let Err(why) = self.fs.rename(pid, leaf(path), tid, &name) {
            self.strip(id); // never leave a stamped object outside the Trash
            return Err(e(why));
        }
        Ok(name)
    }

    fn items(&mut self) -> Result<Vec<TrashItem>, String> {
        let td = self.trash_dir();
        let mut out = Vec::new();
        for h in self.fs.query(TRASH_QUERY).map_err(e)? {
            if parent(&h.path) != td {
                continue; // another user's Trash, or a stamped object that was moved out
            }
            let when = match self.fs.get_attribute(h.inode_id, ATTR_KEY_TRASH_TIME) {
                Ok(Some(AttributeValue::Int(i))) => i,
                _ => 0,
            };
            let origin = self.str_attr(h.inode_id, ATTR_KEY_TRASH_ORIGIN);
            let by = self.str_attr(h.inode_id, ATTR_KEY_TRASH_BY);
            out.push(TrashItem { id: h.inode_id, name: leaf(&h.path).to_string(), origin, when, by });
        }
        Ok(out)
    }

    fn restore(&mut self, name: &str) -> Result<String, String> {
        let it = self.items()?.into_iter().find(|i| i.name == name).ok_or("not-in-trash")?;
        // The id's CURRENT path (a renamed trashed item still restores).
        let cur = self.fs.path_of(it.id).map_err(e)?;
        let pd = parent(&it.origin).to_string();
        let did = match self.fs.resolve_path(&pd) {
            Ok(d) if self.fs.read_inode(d).map(|i| i.kind == FileKind::Directory).unwrap_or(false) => d,
            _ => return Err(format!("the original folder {pd} is gone")),
        };
        if self.fs.resolve_path(&it.origin).is_ok() {
            return Err(format!("{} already exists", it.origin));
        }
        let sid = self.fs.resolve_path(parent(&cur)).map_err(e)?;
        self.fs.rename(sid, leaf(&cur), did, leaf(&it.origin)).map_err(e)?;
        self.strip(it.id);
        Ok(it.origin)
    }

    fn empty(&mut self) -> Result<usize, String> {
        let td = self.trash_dir();
        let Ok(tid) = self.fs.resolve_path(&td) else { return Ok(0) };
        let mut n = 0;
        for c in self.fs.ls(tid).map_err(e)? {
            if c.name == "." || c.name == ".." {
                continue;
            }
            self.strip(c.inode_id); // the keys leave the index with the object
            self.remove_tree(&format!("{td}/{}", c.name), 0)?;
            n += 1;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unafs::MemDevice;

    fn vault() -> VaultTrash<MemDevice> {
        let mut fs = UnaFS::format(MemDevice::new(), 32).expect("format");
        let root = fs.resolve_path("/").unwrap();
        let h = fs.mkdir(root, "home".into()).unwrap();
        let u = fs.mkdir(h, "una".into()).unwrap();
        let f = fs.create_file(u, "note.txt".into()).unwrap();
        fs.write_data(f, 0, b"keep me").unwrap();
        VaultTrash::new(fs, "una")
    }

    #[test]
    fn keys_are_the_kernel_literals() {
        assert_eq!(ATTR_KEY_TRASH_ORIGIN, "una:trash-origin");
        assert_eq!(ATTR_KEY_TRASH_TIME, "una:trash-time");
        assert_eq!(ATTR_KEY_TRASH_BY, "una:trash-by");
    }

    #[test]
    fn trash_query_rename_restore_empty() {
        let mut v = vault();
        let id = v.fs().resolve_path("/home/una/note.txt").unwrap();
        let name = v.trash("/home/una/note.txt").unwrap();
        assert_eq!(name, "note.txt");
        // Same inode, now in the Trash, found by query with its stamps.
        let items = v.items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, id);
        assert_eq!(items[0].origin, "/home/una/note.txt");
        assert_eq!(items[0].by, "una");
        assert!(items[0].when > 0);
        assert!(v.fs().resolve_path("/home/una/note.txt").is_err());
        // Rename inside the Trash: still the same object, still restorable by its new name.
        let tid = v.fs().resolve_path("/home/una/.Trash").unwrap();
        v.fs().rename(tid, "note.txt", tid, "renamed.txt").unwrap();
        let items = v.items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "renamed.txt");
        assert_eq!(v.restore("renamed.txt").unwrap(), "/home/una/note.txt");
        assert_eq!(v.fs().resolve_path("/home/una/note.txt").unwrap(), id);
        assert!(v.items().unwrap().is_empty());
        assert_eq!(v.fs().get_attribute(id, ATTR_KEY_TRASH_ORIGIN).unwrap(), None);
        // Trash again, collide, empty: the query finds nothing afterwards.
        v.trash("/home/una/note.txt").unwrap();
        let u = v.fs().resolve_path("/home/una").unwrap();
        v.fs().create_file(u, "note.txt".into()).unwrap();
        assert_eq!(v.trash("/home/una/note.txt").unwrap(), "note.txt~1");
        assert_eq!(v.items().unwrap().len(), 2);
        assert_eq!(v.empty().unwrap(), 2);
        assert!(v.items().unwrap().is_empty());
        assert!(v.fs().query(TRASH_QUERY).unwrap().is_empty());
    }

    #[test]
    fn refusals() {
        let mut v = vault();
        assert!(v.trash("/etc/passwd").is_err());
        assert!(v.trash("/home/una/../x").is_err());
        assert!(v.trash("/home/una/missing").is_err());
        assert!(v.restore("nothing").is_err());
        assert_eq!(v.empty().unwrap(), 0);
    }
}
