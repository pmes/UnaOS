// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — shared-core
//!
//! TRASH (R75) — a desktop Trash. `/home/<user>/.Trash/` is created on first use through the mount
//! table under the user's namespace (DIRNS); `trash` MOVES an entry there with the same `rename`
//! the shell's `mv` uses (LFNMV2). Quarry's menu, the Dock's Trash tile (DOCK2), the Trash query
//! folder (QUERYFOLDER) and the shell's `trash` verb call these same bodies.
//!
//! TRASHCORE (B449, ARCHREVIEW F5, R79): every Trash RULE lives in `trash_core`
//! (`unaos/libs/fs/trash_core`), the `no_std` core Matrix's `VaultTrash` links too — what a trashed
//! object is (TRASHTIME B308: `una:trash-origin`/`una:trash-time`/`una:trash-by` on a volume with
//! object ids, found by `TRASH_QUERY` scoped to the Trash, restored by id; the FAT `.Trash/.index`
//! fallback), the store choice, refusals, collision names (TESTFIX3's 8.3 form), restore and empty.
//! This file is only the core's I/O over the [`MountTable`] ([`Mt`]), the session user, the clock,
//! the shell verb and the `tests trash` fixture.
//!
//! Each op prints `[trash] store=attrs|index op=trash|restore|empty path= ok= reason= core=trash_core`.

use crate::fs::vfs::{AttrValue, MountTable, NodeKind, KERNEL_PRINCIPAL};
use alloc::string::String;
use alloc::vec::Vec;
use trash_core::{Attr, Ctx, Node, TrashFs, ATTR_KEY_TRASH_ORIGIN, INDEX, TRASH_QUERY};
pub use trash_core::{Entry, Store};

const P: &str = KERNEL_PRINCIPAL;
/// The kernel stack bounds how deep `empty` descends.
const MAX_TREE_DEPTH: usize = 6;

fn mt() -> MountTable {
    crate::shell::vfs_mount_table()
}

fn err_s(e: crate::fs::vfs::VfsError) -> String {
    alloc::format!("{:?}", e)
}

fn session_user() -> Option<String> {
    #[cfg(all(target_arch = "x86_64", feature = "login"))]
    {
        let mut b = [0u8; 32];
        let n = crate::arch::x86_64::syscall::session_name(&mut b)?;
        return core::str::from_utf8(&b[..n]).ok().map(String::from);
    }
    #[allow(unreachable_code)]
    None
}

/// `/home/<user>` (or `/home` with no session).
pub fn home_base() -> String {
    match session_user() {
        Some(u) => alloc::format!("/home/{}", u),
        None => String::from("/home"),
    }
}

/// The Trash folder path.
pub fn trash_dir() -> String {
    trash_core::trash_dir(&home_base())
}

fn index_path() -> String {
    trash_core::join(&trash_dir(), INDEX)
}

use trash_core::{join, leaf, parent};

/// `trash_core`'s I/O seam over the kernel mount table (FAT and UnaFS homes alike).
pub struct Mt(pub MountTable);

impl Mt {
    fn read_all(&self, p: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut off = 0u64;
        while let Ok(c) = self.0.read(p, off, 16 * 1024) {
            if c.is_empty() {
                break;
            }
            off += c.len() as u64;
            out.extend_from_slice(&c);
        }
        out
    }
}

impl TrashFs for Mt {
    fn stat(&mut self, path: &str) -> Option<Node> {
        self.0.stat(path).ok().map(|st| Node { dir: matches!(st.kind, NodeKind::Dir), id: st.id })
    }
    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        self.0.create(path, NodeKind::Dir, P).map(|_| ()).map_err(err_s)
    }
    fn rename(&mut self, src: &str, dst: &str) -> Result<(), String> {
        let before = crate::fs::fat::sector_write_count();
        let r = self.0.rename(src, dst, P);
        if crate::fs::fat::is_long_name(leaf(dst)) {
            serial_println!("[fs] mv {} -> {} lfn=1 ok={} sectors_written={}", src, dst, r.is_ok(), crate::fs::fat::sector_write_count().wrapping_sub(before));
        }
        r.map_err(err_s)
    }
    fn set_attr(&mut self, path: &str, key: &str, v: Attr) -> Result<(), String> {
        let v = match v {
            Attr::Str(s) => AttrValue::Str(s),
            Attr::Int(i) => AttrValue::Int(i),
        };
        self.0.set_attr(path, key, v, P).map_err(err_s)
    }
    fn get_attr(&mut self, path: &str, key: &str) -> Option<Attr> {
        match self.0.get_attr(path, key, P) {
            Ok(AttrValue::Str(s)) => Some(Attr::Str(s)),
            Ok(AttrValue::Int(i)) => Some(Attr::Int(i)),
            _ => None,
        }
    }
    fn remove_attr(&mut self, path: &str, key: &str) {
        let _ = self.0.remove_attr(path, key, P);
    }
    fn query(&mut self, q: &str) -> Vec<(u64, String)> {
        self.0.query(q, P).unwrap_or_default()
    }
    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        Ok(self.0.read_dir(dir).map_err(err_s)?.into_iter().map(|e| e.name).collect())
    }
    fn unlink(&mut self, path: &str) -> Result<(), String> {
        self.0.unlink(path, P).map_err(err_s)
    }
    fn rmdir(&mut self, path: &str) -> Result<(), String> {
        self.0.remove_dir(path, P).map_err(err_s)
    }
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        self.0.stat(path).ok()?;
        Some(self.read_all(path))
    }
    fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        if self.0.stat(path).is_ok() {
            self.0.unlink(path, P).map_err(err_s)?;
        }
        self.0.create(path, NodeKind::File, P).map_err(err_s)?;
        if !bytes.is_empty() {
            self.0.write(path, 0, bytes, P).map_err(err_s)?;
        }
        Ok(())
    }
    fn append_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let off = match self.0.stat(path) {
            Ok(st) => st.size,
            Err(_) => {
                self.0.create(path, NodeKind::File, P).map_err(err_s)?;
                0
            }
        };
        self.0.write(path, off, bytes, P).map(|_| ()).map_err(err_s)
    }
    fn case_insensitive(&self) -> bool {
        true // FAT homes fold case; the kernel has always compared Trash names without it
    }
    fn short_names(&self, leaf: &str) -> bool {
        !crate::fs::fat::is_long_name(leaf) // TESTFIX3: keep a collision name 8.3 when the leaf is
    }
    fn max_depth(&self) -> usize {
        MAX_TREE_DEPTH
    }
}

/// THE one choice (TRASHTIME, now `trash_core::store`): attributes where the home volume gives
/// objects an identity, else the `.index` fallback.
pub fn store() -> Store {
    trash_core::store(&mut Mt(mt()), &home_base())
}

fn log(op: &str, path: &str, r: &Result<(), String>) {
    let s = store().name();
    match r {
        Ok(()) => serial_println!("[trash] store={} op={} path={} ok=1 reason=- core=trash_core", s, op, path),
        Err(e) => serial_println!("[trash] store={} op={} path={} ok=0 reason={} core=trash_core", s, op, path, e),
    }
}

/// What the Trash holds. Empty when there is nothing.
pub fn entries() -> Vec<Entry> {
    trash_core::entries(&mut Mt(mt()), &home_base())
}

/// How many items the Trash holds.
pub fn count() -> usize {
    entries().len()
}

/// Move `path` into the Trash. Returns the trashed name.
pub fn trash(path: &str) -> Result<String, String> {
    let home = home_base();
    let user = session_user().unwrap_or_else(|| String::from(KERNEL_PRINCIPAL));
    let cx = Ctx { home: &home, user: &user, now: crate::clock::unix_now().unwrap_or(0) };
    let r = trash_core::trash(&mut Mt(mt()), &cx, path);
    log("trash", path, &r.clone().map(|_| ()));
    r
}

/// Move the trashed `name` back to its original path.
pub fn restore(name: &str) -> Result<String, String> {
    let r = trash_core::restore(&mut Mt(mt()), &home_base(), name);
    log("restore", name, &r.clone().map(|_| ()));
    r
}

/// Unlink everything in the Trash. Returns how many items were emptied.
pub fn empty() -> Result<usize, String> {
    let td = trash_dir();
    let r = trash_core::empty(&mut Mt(mt()), &home_base());
    log("empty", &td, &r.clone().map(|_| ()));
    r
}

/// The shell verb: `trash <path>` · `trash list` · `trash restore <name>` · `trash empty`.
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    match args.first().copied() {
        None => console.println("usage: trash <path> | trash list | trash restore <name> | trash empty"),
        Some("list") => {
            let es = entries();
            for e in &es {
                console.println(&alloc::format!("{}  <- {}  @{}", e.name, e.orig, e.when));
            }
            console.println(&alloc::format!("{} item(s) in {} (store={})", es.len(), trash_dir(), store().name()));
        }
        Some("restore") => match args.get(1) {
            Some(n) => match restore(n) {
                Ok(p) => console.println(&alloc::format!("restored {}", p)),
                Err(e) => console.println(&alloc::format!("trash: restore refused ({})", e)),
            },
            None => console.println("usage: trash restore <name>"),
        },
        Some("empty") => match empty() {
            Ok(n) => console.println(&alloc::format!("emptied {} item(s)", n)),
            Err(e) => console.println(&alloc::format!("trash: empty failed ({})", e)),
        },
        Some(p) => {
            let full = if p.starts_with('/') { String::from(p) } else { join(&home_base(), p) };
            match trash(&full) {
                Ok(n) => console.println(&alloc::format!("trashed {} as {}", full, n)),
                Err(e) => console.println(&alloc::format!("trash: refused ({})", e)),
            }
        }
    }
}

fn listed(t: &MountTable, dir: &str, name: &str) -> bool {
    t.read_dir(dir).map(|v| v.iter().any(|e| e.name.eq_ignore_ascii_case(name))).unwrap_or(false)
}

/// `:: TRASH: store=<attrs|index> trashed= restored= emptied= query_ok= -> PASS ::` — create a
/// scratch file under home, trash it, (attrs: rename it inside the Trash,) restore it, trash it
/// again, empty; the listing and the store (the query by inode id, or the `.index`) are verified at
/// each step. Runs the UNAFSTIME fixture first (same home, same registration).
pub fn selftest() {
    let t = mt();
    let base = home_base();
    if t.stat(&base).is_err() && t.create(&base, NodeKind::Dir, P).is_err() {
        serial_println!(":: TRASH: base={} reason=no-home -> SKIP ::", base);
        return;
    }
    crate::fs::unafstime::selftest(&base);
    let s = store();
    let td = trash_dir();
    let name = "TRASHFX.TXT";
    let moved = "TRASHFX2.TXT";
    let path = join(&base, name);
    let _ = empty(); // a clean slate (also creates nothing when absent)
    if t.stat(&path).is_ok() {
        let _ = t.unlink(&path, P);
    }
    let _ = t.create(&path, NodeKind::File, P).and_then(|_| t.write(&path, 0, b"trash me", P));
    let id0 = t.stat(&path).ok().and_then(|st| st.id);
    let (mut trashed, mut restored, mut emptied, mut query_ok) = (0u32, 0u32, 0u32, 0u32);
    // trash
    let mut tn = trash(&path);
    if let Ok(n) = &tn {
        let es = entries();
        let shape = !listed(&t, &base, name) && listed(&t, &td, n) && es.len() == 1 && es[0].orig == path && es[0].name == *n;
        let store_ok = match s {
            // found by the query, as the SAME object, and no index file written
            Store::Attrs => es.first().and_then(|e| e.id) == id0 && id0.is_some() && t.stat(&index_path()).is_err(),
            Store::Index => true,
        };
        if shape && store_ok {
            trashed += 1;
            query_ok += 1;
        }
    }
    // attrs: a rename INSIDE the Trash keeps the object restorable (identity, not name)
    if s == Store::Attrs {
        if let Ok(n) = &tn {
            let ok = t.rename(&join(&td, n), &join(&td, moved), P).is_ok()
                && entries().iter().any(|e| e.name.eq_ignore_ascii_case(moved) && e.id == id0);
            if ok {
                tn = Ok(String::from(moved));
            } else {
                query_ok = 0;
            }
        }
    }
    // restore
    if let Ok(n) = &tn {
        let rs = restore(n);
        let back = rs.is_ok() && listed(&t, &base, name) && !listed(&t, &td, n) && entries().is_empty() && t.read(&path, 0, 16).map(|b| b == b"trash me").unwrap_or(false);
        let same = match s {
            Store::Attrs => t.stat(&path).ok().and_then(|st| st.id) == id0 && t.get_attr(&path, ATTR_KEY_TRASH_ORIGIN, P).is_err(),
            Store::Index => true,
        };
        if back && same {
            restored += 1;
        }
    }
    // trash again, then empty
    let tn2 = trash(&path);
    if tn2.is_ok() && count() == 1 {
        trashed += 1;
    }
    let em = empty();
    if let (Ok(n), Ok(n2)) = (&em, &tn2) {
        if *n == 1 && !listed(&t, &td, n2) && !listed(&t, &base, name) && count() == 0 {
            emptied += 1;
            query_ok += 1;
        }
    }
    // a collision gets ~1
    let _ = t.create(&path, NodeKind::File, P);
    let c1 = trash(&path);
    let _ = t.create(&path, NodeKind::File, P);
    let c2 = trash(&path);
    let coll = matches!((&c1, &c2), (Ok(a), Ok(b)) if a != b && b.contains("~1")); // TESTFIX3: `TRASHF~1.TXT` on FAT
    let _ = empty();
    let none_left = match s {
        Store::Attrs => !t.query(TRASH_QUERY, P).unwrap_or_default().iter().any(|(_, p)| parent(p).eq_ignore_ascii_case(&td)),
        Store::Index => count() == 0,
    };
    if none_left {
        query_ok += 1;
    }
    let pass = trashed == 2 && restored == 1 && emptied == 1 && query_ok == 3 && coll;
    serial_println!(
        ":: TRASH: store={} trashed={} restored={} emptied={} query_ok={} -> {} ::",
        s.name(), trashed, restored, emptied, query_ok, if pass { "PASS" } else { "FAIL" }
    );
}
