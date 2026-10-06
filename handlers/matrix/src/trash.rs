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

//! CHARTER: Matrix — shared-core
//!
//! TRASHTIME M3 (B308) + TRASHCORE (B449): the Trash on a UnaFS vault. Every Trash RULE — what a
//! trashed object is (`una:trash-origin` / `una:trash-time` / `una:trash-by`, literals once in
//! `una-abi`), the refusals, the collision name, stamp-then-rekey into `/home/<user>/.Trash/`, the
//! query scoped to that folder, restore by inode id, strip-then-unlink on empty — lives in
//! `trash_core`, the `no_std` core the kernel's `fs/trash.rs` links too. This file is only the core's
//! I/O over `UnaFS` ([`Vault`]) and the Finder's [`TrashStore`] verbs.

use trash_core::{Attr, Ctx, Node, TrashFs};
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

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `trash_core`'s I/O seam over a UnaFS vault, by absolute path.
pub struct Vault<'a, D: BlockDevice>(pub &'a mut UnaFS<D>);

impl<D: BlockDevice> Vault<'_, D> {
    fn id(&mut self, path: &str) -> Result<u64, String> {
        self.0.resolve_path(path).map_err(e)
    }
    fn at(&mut self, path: &str) -> Result<(u64, String), String> {
        Ok((self.id(trash_core::parent(path))?, trash_core::leaf(path).to_string()))
    }
}

impl<D: BlockDevice> TrashFs for Vault<'_, D> {
    fn stat(&mut self, path: &str) -> Option<Node> {
        let id = self.0.resolve_path(path).ok()?;
        let dir = self.0.read_inode(id).ok()?.kind == FileKind::Directory;
        Some(Node { dir, id: Some(id) })
    }
    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        let (pid, n) = self.at(path)?;
        self.0.mkdir(pid, n).map(|_| ()).map_err(e)
    }
    fn rename(&mut self, src: &str, dst: &str) -> Result<(), String> {
        let (sp, sn) = self.at(src)?;
        let (dp, dn) = self.at(dst)?;
        self.0.rename(sp, &sn, dp, &dn).map(|_| ()).map_err(e)
    }
    fn set_attr(&mut self, path: &str, key: &str, v: Attr) -> Result<(), String> {
        let id = self.id(path)?;
        let v = match v {
            Attr::Str(s) => AttributeValue::String(s),
            Attr::Int(i) => AttributeValue::Int(i),
        };
        self.0.set_attribute(id, key.into(), v).map(|_| ()).map_err(e)
    }
    fn get_attr(&mut self, path: &str, key: &str) -> Option<Attr> {
        let id = self.0.resolve_path(path).ok()?;
        match self.0.get_attribute(id, key) {
            Ok(Some(AttributeValue::String(s))) => Some(Attr::Str(s)),
            Ok(Some(AttributeValue::Int(i))) => Some(Attr::Int(i)),
            _ => None,
        }
    }
    fn remove_attr(&mut self, path: &str, key: &str) {
        if let Ok(id) = self.0.resolve_path(path) {
            let _ = self.0.remove_attribute(id, key);
        }
    }
    fn query(&mut self, q: &str) -> Vec<(u64, String)> {
        self.0.query(q).map(|v| v.into_iter().map(|h| (h.inode_id, h.path)).collect()).unwrap_or_default()
    }
    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        let id = self.id(dir)?;
        Ok(self.0.ls(id).map_err(e)?.into_iter().map(|c| c.name).collect())
    }
    fn unlink(&mut self, path: &str) -> Result<(), String> {
        let (pid, n) = self.at(path)?;
        self.0.unlink(pid, &n).map(|_| ()).map_err(e)
    }
    fn rmdir(&mut self, path: &str) -> Result<(), String> {
        let (pid, n) = self.at(path)?;
        self.0.rmdir(pid, &n).map(|_| ()).map_err(e)
    }
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        let id = self.0.resolve_path(path).ok()?;
        let size = self.0.read_inode(id).ok()?.size;
        self.0.read_data(id, 0, size).ok()
    }
    fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        if self.0.resolve_path(path).is_ok() {
            self.unlink(path)?;
        }
        self.append_file(path, bytes)
    }
    fn append_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let id = match self.0.resolve_path(path) {
            Ok(id) => id,
            Err(_) => {
                let (pid, n) = self.at(path)?;
                self.0.create_file(pid, n).map_err(e)?
            }
        };
        let off = self.0.read_inode(id).map_err(e)?.size;
        self.0.write_data(id, off, bytes).map_err(e)
    }
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
        trash_core::trash_dir(&self.home())
    }
}

impl<D: BlockDevice + Send> TrashStore for VaultTrash<D> {
    fn trash(&mut self, path: &str) -> Result<String, String> {
        let home = self.home();
        let cx = Ctx { home: &home, user: &self.user, now: now_secs() };
        trash_core::trash(&mut Vault(&mut self.fs), &cx, path)
    }

    fn items(&mut self) -> Result<Vec<TrashItem>, String> {
        let home = self.home();
        Ok(trash_core::entries(&mut Vault(&mut self.fs), &home)
            .into_iter()
            .map(|x| TrashItem { id: x.id.unwrap_or(0), name: x.name, origin: x.orig, when: x.when as i64, by: x.by })
            .collect())
    }

    fn restore(&mut self, name: &str) -> Result<String, String> {
        let home = self.home();
        trash_core::restore(&mut Vault(&mut self.fs), &home, name)
    }

    fn empty(&mut self) -> Result<usize, String> {
        let home = self.home();
        trash_core::empty(&mut Vault(&mut self.fs), &home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use una_abi::{ATTR_KEY_TRASH_BY, ATTR_KEY_TRASH_ORIGIN, ATTR_KEY_TRASH_TIME, TRASH_QUERY};
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
        assert_eq!(ATTR_KEY_TRASH_ORIGIN, una_abi::attr_keys::TRASH_ORIGIN); // ATTRKEYS (B452): the one registry
        assert_eq!(ATTR_KEY_TRASH_TIME, una_abi::attr_keys::TRASH_TIME);
        assert_eq!(ATTR_KEY_TRASH_BY, una_abi::attr_keys::TRASH_BY);
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
