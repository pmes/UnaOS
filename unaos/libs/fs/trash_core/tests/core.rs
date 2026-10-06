// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TRASHCORE (B449): the core's rules over an in-memory volume, both stores — attrs (ids, attributes,
// case-sensitive names: UnaFS) and index (no ids, case folding, 8.3 moves: FAT).

use std::collections::BTreeMap;
use trash_core::*;

#[derive(Clone, Default)]
struct N {
    dir: bool,
    id: u64,
    attrs: BTreeMap<String, Attr>,
    data: Vec<u8>,
}

struct Mem {
    ids: bool,
    ci: bool,
    next: u64,
    nodes: BTreeMap<String, N>,
}

impl Mem {
    fn new(ids: bool, ci: bool) -> Self {
        let mut m = Mem { ids, ci, next: 1, nodes: BTreeMap::new() };
        m.put("/", true);
        m.put("/home", true);
        m.put("/home/una", true);
        m
    }
    fn key(&self, p: &str) -> String {
        if self.ci { p.to_ascii_lowercase() } else { p.to_string() }
    }
    fn put(&mut self, p: &str, dir: bool) -> u64 {
        let id = self.next;
        self.next += 1;
        let k = self.key(p);
        self.nodes.insert(k, N { dir, id, ..Default::default() });
        id
    }
    fn file(&mut self, p: &str, body: &[u8]) -> u64 {
        let id = self.put(p, false);
        let k = self.key(p);
        self.nodes.get_mut(&k).unwrap().data = body.to_vec();
        id
    }
    fn has(&self, p: &str) -> bool {
        self.nodes.contains_key(&self.key(p))
    }
}

impl TrashFs for Mem {
    fn stat(&mut self, p: &str) -> Option<Node> {
        let ids = self.ids;
        self.nodes.get(&self.key(p)).map(|n| Node { dir: n.dir, id: ids.then_some(n.id) })
    }
    fn mkdir(&mut self, p: &str) -> Result<(), String> {
        if self.has(p) || !self.has(parent(p)) {
            return Err("mkdir".into());
        }
        self.put(p, true);
        Ok(())
    }
    fn rename(&mut self, s: &str, d: &str) -> Result<(), String> {
        let (ks, kd) = (self.key(s), self.key(d));
        if !self.has(s) || self.has(d) || !self.has(parent(d)) {
            return Err("rename".into());
        }
        let moved: Vec<String> = self.nodes.keys().filter(|k| **k == ks || k.starts_with(&format!("{ks}/"))).cloned().collect();
        for k in moved {
            let n = self.nodes.remove(&k).unwrap();
            self.nodes.insert(format!("{kd}{}", &k[ks.len()..]), n);
        }
        Ok(())
    }
    fn set_attr(&mut self, p: &str, k: &str, v: Attr) -> Result<(), String> {
        if !self.ids {
            return Err("Unsupported".into());
        }
        let key = self.key(p);
        self.nodes.get_mut(&key).ok_or("noent")?.attrs.insert(k.into(), v);
        Ok(())
    }
    fn get_attr(&mut self, p: &str, k: &str) -> Option<Attr> {
        self.nodes.get(&self.key(p))?.attrs.get(k).cloned()
    }
    fn remove_attr(&mut self, p: &str, k: &str) {
        let key = self.key(p);
        if let Some(n) = self.nodes.get_mut(&key) {
            n.attrs.remove(k);
        }
    }
    fn query(&mut self, q: &str) -> Vec<(u64, String)> {
        assert_eq!(q, TRASH_QUERY);
        self.nodes
            .iter()
            .filter(|(_, n)| matches!(n.attrs.get(ATTR_KEY_TRASH_ORIGIN), Some(Attr::Str(s)) if !s.is_empty()))
            .map(|(k, n)| (n.id, k.clone()))
            .collect()
    }
    fn list(&mut self, d: &str) -> Result<Vec<String>, String> {
        let kd = self.key(d);
        let mut v = vec![".".to_string(), "..".to_string()];
        for k in self.nodes.keys() {
            if k != "/" && parent(k) == kd {
                v.push(leaf(k).to_string());
            }
        }
        Ok(v)
    }
    fn unlink(&mut self, p: &str) -> Result<(), String> {
        let k = self.key(p);
        self.nodes.remove(&k).map(|_| ()).ok_or("unlink".into())
    }
    fn rmdir(&mut self, p: &str) -> Result<(), String> {
        if self.list(p)?.len() > 2 {
            return Err("not empty".into());
        }
        self.unlink(p)
    }
    fn read_file(&mut self, p: &str) -> Option<Vec<u8>> {
        self.nodes.get(&self.key(p)).map(|n| n.data.clone())
    }
    fn write_file(&mut self, p: &str, b: &[u8]) -> Result<(), String> {
        if !self.has(p) {
            self.put(p, false);
        }
        let k = self.key(p);
        self.nodes.get_mut(&k).unwrap().data = b.to_vec();
        Ok(())
    }
    fn append_file(&mut self, p: &str, b: &[u8]) -> Result<(), String> {
        if !self.has(p) {
            self.put(p, false);
        }
        let k = self.key(p);
        self.nodes.get_mut(&k).unwrap().data.extend_from_slice(b);
        Ok(())
    }
    fn case_insensitive(&self) -> bool {
        self.ci
    }
    fn short_names(&self, leaf: &str) -> bool {
        // FAT-like: a leaf whose stem fits 8 and ext fits 3 can only move to an 8.3 name.
        self.ci && { let (s, e) = leaf.split_once('.').unwrap_or((leaf, "")); s.len() <= 8 && e.len() <= 3 }
    }
    fn max_depth(&self) -> usize {
        if self.ci { 6 } else { 32 }
    }
}

const H: &str = "/home/una";
fn cx(now: u64) -> Ctx<'static> {
    Ctx { home: H, user: "una", now }
}

#[test]
fn keys_are_the_abi_literals() {
    use una_abi::attr_keys as k;
    assert_eq!(KEYS, [k::TRASH_ORIGIN, k::TRASH_TIME, k::TRASH_BY]);
    assert_eq!(trash_dir(H), "/home/una/.Trash");
}

#[test]
fn attrs_store_trash_rename_restore_empty() {
    let mut m = Mem::new(true, false);
    let id = m.file("/home/una/note.txt", b"keep me");
    assert_eq!(store(&mut m, H), Store::Attrs);
    assert_eq!(trash(&mut m, &cx(77), "/home/una/note.txt").unwrap(), "note.txt");
    let es = entries(&mut m, H);
    assert_eq!(es, vec![Entry { orig: "/home/una/note.txt".into(), name: "note.txt".into(), when: 77, by: "una".into(), id: Some(id) }]);
    assert!(!m.has("/home/una/note.txt") && !m.has("/home/una/.Trash/.index"), "no index on the attrs store");
    // QUERYFOLDER
    assert!(is_trash_folder(&m, "/home/una/.Trash/", H) && !is_trash_folder(&m, H, H));
    assert_eq!(origins(&es), vec![("note.txt".to_string(), "/home/una/note.txt".to_string())]);
    // a rename inside the Trash keeps it restorable (identity, not name)
    m.rename("/home/una/.Trash/note.txt", "/home/una/.Trash/renamed.txt").unwrap();
    assert_eq!(restore(&mut m, H, "renamed.txt").unwrap(), "/home/una/note.txt");
    assert_eq!(m.stat("/home/una/note.txt").unwrap().id, Some(id));
    assert_eq!(m.get_attr("/home/una/note.txt", ATTR_KEY_TRASH_ORIGIN), None);
    assert_eq!(m.read_file("/home/una/note.txt").unwrap(), b"keep me");
    assert!(entries(&mut m, H).is_empty());
    // collide, empty: the query finds nothing afterwards
    trash(&mut m, &cx(1), "/home/una/note.txt").unwrap();
    m.file("/home/una/note.txt", b"");
    assert_eq!(trash(&mut m, &cx(2), "/home/una/note.txt").unwrap(), "note.txt~1");
    assert_eq!(entries(&mut m, H).len(), 2);
    assert_eq!(empty(&mut m, H).unwrap(), 2);
    assert!(m.query(TRASH_QUERY).is_empty() && entries(&mut m, H).is_empty());
}

#[test]
fn attrs_store_scoped_to_this_users_trash() {
    let mut m = Mem::new(true, false);
    m.put("/home/bob", true);
    m.file("/home/bob/x", b"");
    trash(&mut m, &Ctx { home: "/home/bob", user: "bob", now: 5 }, "/home/bob/x").unwrap();
    assert!(entries(&mut m, H).is_empty());
    assert_eq!(entries(&mut m, "/home/bob").len(), 1);
}

#[test]
fn index_store_fat_shape() {
    let mut m = Mem::new(false, true);
    m.file("/home/una/TRASHFX.TXT", b"trash me");
    assert_eq!(store(&mut m, H), Store::Index);
    assert_eq!(trash(&mut m, &cx(9), "/home/una/TRASHFX.TXT").unwrap(), "TRASHFX.TXT");
    let idx = m.read_file("/home/una/.Trash/.index").unwrap();
    assert_eq!(idx, b"/home/una/TRASHFX.TXT\tTRASHFX.TXT\t9\n");
    // case folding on the restore name
    assert_eq!(restore(&mut m, H, "trashfx.txt").unwrap(), "/home/una/TRASHFX.TXT");
    assert!(entries(&mut m, H).is_empty());
    // the 8.3 collision name (TESTFIX3)
    trash(&mut m, &cx(1), "/home/una/TRASHFX.TXT").unwrap();
    m.file("/home/una/TRASHFX.TXT", b"");
    assert_eq!(trash(&mut m, &cx(2), "/home/una/TRASHFX.TXT").unwrap(), "TRASHF~1.TXT");
    assert_eq!(entries(&mut m, H).len(), 2);
    // empty skips (and rewrites) the index, deletes a folder tree
    m.put("/home/una/dir", true);
    m.file("/home/una/dir/a", b"");
    trash(&mut m, &cx(3), "/home/una/dir").unwrap();
    assert_eq!(empty(&mut m, H).unwrap(), 3);
    assert_eq!(m.read_file("/home/una/.Trash/.index").unwrap(), b"");
    assert_eq!(m.list("/home/una/.Trash").unwrap().len(), 3); // . .. .index
}

#[test]
fn collision_names() {
    assert_eq!(collision_name("note.txt", 1, false), "note.txt~1");
    assert_eq!(collision_name("TRASHFX.TXT", 1, true), "TRASHF~1.TXT");
    assert_eq!(collision_name("A", 12, true), "A~12");
    assert_eq!(collision_name("ABCDEFGH", 12, true), "ABCDE~12");
}

#[test]
fn index_round_trip_and_junk() {
    let es = parse_index("/a/b\tb\t3\n\nbad line\n/a/c\tc\tx\n");
    assert_eq!(es.len(), 2);
    assert_eq!(es[1].when, 0);
    assert_eq!(parse_index(&render_index(&es)), es);
}

#[test]
fn refusals() {
    let mut m = Mem::new(true, false);
    m.file("/home/una/a\tb", b"");
    assert!(trash(&mut m, &cx(1), "/etc/passwd").is_err());
    assert!(trash(&mut m, &cx(1), "/home/unabob/x").is_err());
    assert!(trash(&mut m, &cx(1), "/home/una/../x").is_err());
    assert!(trash(&mut m, &cx(1), "/home/una").is_err());
    assert!(trash(&mut m, &cx(1), "/home/una/missing").is_err());
    assert_eq!(trash(&mut m, &cx(1), "/home/una/a\tb").unwrap_err(), "bad-name");
    m.mkdir("/home/una/.Trash").unwrap();
    m.file("/home/una/.Trash/y", b"");
    assert_eq!(trash(&mut m, &cx(1), "/home/una/.Trash/y").unwrap_err(), "already in the trash");
    assert!(restore(&mut m, H, "nothing").is_err());
    assert_eq!(empty(&mut m, H).unwrap(), 1);
}

#[test]
fn restore_refuses_occupied_or_gone_origin() {
    let mut m = Mem::new(true, false);
    m.put("/home/una/d", true);
    m.file("/home/una/d/f", b"");
    m.file("/home/una/g", b"");
    trash(&mut m, &cx(1), "/home/una/d/f").unwrap();
    trash(&mut m, &cx(1), "/home/una/g").unwrap();
    m.file("/home/una/g", b"new");
    assert!(restore(&mut m, H, "g").unwrap_err().contains("already exists"));
    m.unlink("/home/una/d").unwrap();
    assert!(restore(&mut m, H, "f").unwrap_err().contains("is gone"));
}
