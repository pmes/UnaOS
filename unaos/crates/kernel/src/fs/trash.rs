// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! TRASH (R75) — a desktop Trash. `/home/<user>/.Trash/` is created on first use through the mount
//! table under the user's namespace (DIRNS); `trash` MOVES an entry there with the same `rename`
//! the shell's `mv` uses (LFNMV2; a collision appends `~1`, `~2`). Quarry's menu and the shell's
//! `trash` verb call these same bodies.
//!
//! TRASHTIME (B308, audit B288/B294): WHAT a trashed object is depends on the volume, chosen in ONE
//! function, [`store`]:
//! * **attrs** — a volume with object ids (UnaFS): the object itself carries `una:trash-origin`
//!   (the original absolute path), `una:trash-time` (unix seconds, Int) and `una:trash-by` (the
//!   session user), key literals from `una-abi` (Matrix's host Finder stamps the same ones). The
//!   rename into `.Trash/` is a rekey, so the inode id survives; the listing is
//!   `query("una:trash-origin != \"\"")` scoped to the Trash's direct children; restore moves the
//!   id's CURRENT path back (a renamed trashed item still restores); empty drops the keys with the
//!   unlink. No index file is written.
//! * **index** — the FAT FALLBACK (no attributes): `.Trash/.index`, one
//!   `original-path<TAB>trashed-name<TAB>unix-time` line per item (appended on trash, rewritten on
//!   restore/empty).
//!
//! Each op prints `[trash] store=attrs|index op=trash|restore|empty path= ok= reason=`.

use crate::fs::vfs::{AttrValue, MountTable, NodeKind, KERNEL_PRINCIPAL};
use alloc::string::String;
use alloc::vec::Vec;
use una_abi::{ATTR_KEY_TRASH_BY, ATTR_KEY_TRASH_ORIGIN, ATTR_KEY_TRASH_TIME, TRASH_DIR_NAME, TRASH_QUERY};

const P: &str = KERNEL_PRINCIPAL;
const INDEX: &str = ".index";
const DIRNAME: &str = TRASH_DIR_NAME;
const MAX_TREE_DEPTH: usize = 6;
const KEYS: [&str; 3] = [ATTR_KEY_TRASH_ORIGIN, ATTR_KEY_TRASH_TIME, ATTR_KEY_TRASH_BY];

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
    alloc::format!("{}/{}", home_base(), DIRNAME)
}

fn index_path() -> String {
    alloc::format!("{}/{}", trash_dir(), INDEX)
}

fn join(d: &str, n: &str) -> String {
    if d.ends_with('/') { alloc::format!("{}{}", d, n) } else { alloc::format!("{}/{}", d, n) }
}

fn leaf(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn parent(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) => "/",
        Some(i) => &p[..i],
        None => "/",
    }
}

fn starts_ci(path: &str, prefix: &str) -> bool {
    path.len() >= prefix.len()
        && path.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
        && (path.len() == prefix.len() || path.as_bytes()[prefix.len()] == b'/')
}

/// Where a trashed object's record lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    /// On the object: `una:trash-*` attributes (a volume with object ids — UnaFS).
    Attrs,
    /// The FAT fallback: `.Trash/.index`.
    Index,
}

impl Store {
    pub fn name(self) -> &'static str {
        match self {
            Store::Attrs => "attrs",
            Store::Index => "index",
        }
    }
}

/// THE one choice (TRASHTIME): attributes where the home volume gives objects an identity, else the
/// `.index` fallback.
pub fn store() -> Store {
    match mt().stat(&home_base()) {
        Ok(st) if st.id.is_some() => Store::Attrs,
        _ => Store::Index,
    }
}

/// One trashed item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub orig: String,
    pub name: String,
    pub when: u64,
    /// The object's inode id on an attribute store (`None` on the index fallback).
    pub id: Option<u64>,
}

fn log(op: &str, path: &str, r: &Result<(), String>) {
    let s = store().name();
    match r {
        Ok(()) => serial_println!("[trash] store={} op={} path={} ok=1 reason=-", s, op, path),
        Err(e) => serial_println!("[trash] store={} op={} path={} ok=0 reason={}", s, op, path, e),
    }
}

/// Create `.Trash` on first use.
fn ensure_dir(t: &MountTable) -> Result<(), String> {
    let d = trash_dir();
    match t.stat(&d) {
        Ok(st) if matches!(st.kind, NodeKind::Dir) => Ok(()),
        Ok(_) => Err(String::from(".Trash is a file")),
        Err(_) => {
            let h = home_base();
            if t.stat(&h).is_err() {
                let _ = t.create(&h, NodeKind::Dir, P);
            }
            t.create(&d, NodeKind::Dir, P).map(|_| ()).map_err(err_s)
        }
    }
}

fn read_all(t: &MountTable, p: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut off = 0u64;
    while let Ok(c) = t.read(p, off, 16 * 1024) {
        if c.is_empty() {
            break;
        }
        off += c.len() as u64;
        out.extend_from_slice(&c);
    }
    out
}

fn parse(text: &str) -> Vec<Entry> {
    let mut v = Vec::new();
    for l in text.lines() {
        let mut it = l.split('\t');
        if let (Some(o), Some(n), Some(w)) = (it.next(), it.next(), it.next()) {
            if !o.is_empty() && !n.is_empty() {
                v.push(Entry { orig: String::from(o), name: String::from(n), when: w.trim().parse().unwrap_or(0), id: None });
            }
        }
    }
    v
}

/// The attribute store's listing: the query, scoped to the direct children of this user's Trash.
fn attr_entries(t: &MountTable) -> Vec<Entry> {
    let td = trash_dir();
    let mut v = Vec::new();
    for (id, path) in t.query(TRASH_QUERY, P).unwrap_or_default() {
        if !parent(&path).eq_ignore_ascii_case(&td) {
            continue; // another user's Trash, or a stamped object moved out by hand
        }
        let orig = match t.get_attr(&path, ATTR_KEY_TRASH_ORIGIN, P) {
            Ok(AttrValue::Str(s)) => s,
            _ => continue,
        };
        let when = match t.get_attr(&path, ATTR_KEY_TRASH_TIME, P) {
            Ok(AttrValue::Int(i)) if i > 0 => i as u64,
            _ => 0,
        };
        v.push(Entry { orig, name: String::from(leaf(&path)), when, id: Some(id) });
    }
    v
}

/// What the Trash holds. Empty when there is nothing.
pub fn entries() -> Vec<Entry> {
    let t = mt();
    match store() {
        Store::Attrs => attr_entries(&t),
        Store::Index => {
            let b = read_all(&t, &index_path());
            parse(core::str::from_utf8(&b).unwrap_or(""))
        }
    }
}

/// How many items the Trash holds.
pub fn count() -> usize {
    entries().len()
}

fn write_index(t: &MountTable, es: &[Entry]) -> Result<(), String> {
    let ip = index_path();
    if t.stat(&ip).is_ok() {
        t.unlink(&ip, P).map_err(err_s)?;
    }
    t.create(&ip, NodeKind::File, P).map_err(err_s)?;
    let mut s = String::new();
    for e in es {
        s.push_str(&alloc::format!("{}\t{}\t{}\n", e.orig, e.name, e.when));
    }
    if !s.is_empty() {
        t.write(&ip, 0, s.as_bytes(), P).map_err(err_s)?;
    }
    Ok(())
}

fn append_index(t: &MountTable, e: &Entry) -> Result<(), String> {
    let ip = index_path();
    let off = match t.stat(&ip) {
        Ok(st) => st.size,
        Err(_) => {
            t.create(&ip, NodeKind::File, P).map_err(err_s)?;
            0
        }
    };
    let line = alloc::format!("{}\t{}\t{}\n", e.orig, e.name, e.when);
    t.write(&ip, off, line.as_bytes(), P).map(|_| ()).map_err(err_s)
}

fn move_logged(t: &MountTable, src: &str, dst: &str) -> Result<(), String> {
    let before = crate::fs::fat::sector_write_count();
    let r = t.rename(src, dst, P);
    if crate::fs::fat::is_long_name(leaf(dst)) {
        serial_println!("[fs] mv {} -> {} lfn=1 ok={} sectors_written={}", src, dst, r.is_ok(), crate::fs::fat::sector_write_count().wrapping_sub(before));
    }
    r.map_err(err_s)
}

/// Drop the three trash keys from the object at `path` (absent keys are not an error).
fn strip_keys(t: &MountTable, path: &str) {
    for k in KEYS {
        let _ = t.remove_attr(path, k, P);
    }
}

/// Stamp the three trash keys on the object at `path`.
fn stamp_keys(t: &MountTable, path: &str, when: u64) -> Result<(), String> {
    let by = session_user().unwrap_or_else(|| String::from(KERNEL_PRINCIPAL));
    t.set_attr(path, ATTR_KEY_TRASH_ORIGIN, AttrValue::Str(String::from(path)), P).map_err(err_s)?;
    t.set_attr(path, ATTR_KEY_TRASH_TIME, AttrValue::Int(when as i64), P).map_err(err_s)?;
    t.set_attr(path, ATTR_KEY_TRASH_BY, AttrValue::Str(by), P).map_err(err_s)
}

/// Move `path` into the Trash. Returns the trashed name.
pub fn trash(path: &str) -> Result<String, String> {
    let r = trash_inner(path);
    log("trash", path, &r.clone().map(|_| ()));
    r
}

fn trash_inner(path: &str) -> Result<String, String> {
    let base = home_base();
    if path.split('/').any(|c| c == "..") {
        return Err(String::from("a path with .. leaves the home folder"));
    }
    if !starts_ci(path, &base) {
        return Err(alloc::format!("files are trashed only under {}", base));
    }
    if path.len() == base.len() {
        return Err(String::from("the home folder itself cannot be trashed"));
    }
    let td = trash_dir();
    if starts_ci(path, &td) {
        return Err(String::from("already in the trash"));
    }
    if leaf(path).contains('\t') || leaf(path).contains('\n') {
        return Err(String::from("bad-name"));
    }
    let t = mt();
    t.stat(path).map_err(err_s)?;
    ensure_dir(&t)?;
    let mut name = String::from(leaf(path));
    let mut n = 0u32;
    while t.stat(&join(&td, &name)).is_ok() {
        n += 1;
        if n > 99 {
            return Err(String::from("too-many-collisions"));
        }
        name = collision_name(leaf(path), n); // TESTFIX3: 8.3-safe on FAT (see collision_name)
    }
    let when = crate::clock::unix_now().unwrap_or(0);
    let dst = join(&td, &name);
    match store() {
        Store::Attrs => {
            // The record goes ON the object first, then the rekeying rename: a failure between the
            // two leaves a stamped object outside the Trash, which the scoped query ignores and the
            // strip below removes.
            if let Err(why) = stamp_keys(&t, path, when) {
                strip_keys(&t, path);
                return Err(alloc::format!("attrs: {}", why));
            }
            if let Err(why) = move_logged(&t, path, &dst) {
                strip_keys(&t, path);
                return Err(why);
            }
        }
        Store::Index => {
            move_logged(&t, path, &dst)?;
            let e = Entry { orig: String::from(path), name: name.clone(), when, id: None };
            if let Err(why) = append_index(&t, &e) {
                let _ = move_logged(&t, &dst, path); // never leave an unindexed item
                return Err(alloc::format!("index: {}", why));
            }
        }
    }
    Ok(name)
}

/// Move the trashed `name` back to its original path.
pub fn restore(name: &str) -> Result<String, String> {
    let r = restore_inner(name);
    log("restore", name, &r.clone().map(|_| ()));
    r
}

fn restore_inner(name: &str) -> Result<String, String> {
    let t = mt();
    let mut es = entries();
    let i = es.iter().position(|e| e.name.eq_ignore_ascii_case(name)).ok_or_else(|| String::from("not-in-trash"))?;
    let orig = es[i].orig.clone();
    let pd = parent(&orig);
    match t.stat(pd) {
        Ok(st) if matches!(st.kind, NodeKind::Dir) => {}
        _ => return Err(alloc::format!("the original folder {} is gone", pd)),
    }
    if t.stat(&orig).is_ok() {
        return Err(alloc::format!("{} already exists", orig));
    }
    match es[i].id {
        Some(id) => {
            // The id's CURRENT path, from the query that just named it (renamed items included).
            let cur = join(&trash_dir(), &es[i].name);
            if t.stat(&cur).ok().and_then(|s| s.id) != Some(id) {
                return Err(String::from("trashed object moved"));
            }
            move_logged(&t, &cur, &orig)?;
            strip_keys(&t, &orig);
        }
        None => {
            move_logged(&t, &join(&trash_dir(), &es[i].name), &orig)?;
            es.remove(i);
            write_index(&t, &es)?;
        }
    }
    Ok(orig)
}

fn delete_tree(t: &MountTable, path: &str, depth: usize) -> Result<(), String> {
    let st = t.stat(path).map_err(err_s)?;
    if matches!(st.kind, NodeKind::File) {
        return t.unlink(path, P).map_err(err_s);
    }
    if depth > MAX_TREE_DEPTH {
        return Err(String::from("too-deep"));
    }
    for e in t.read_dir(path).map_err(err_s)? {
        if e.name == "." || e.name == ".." {
            continue;
        }
        delete_tree(t, &join(path, &e.name), depth + 1)?;
    }
    t.remove_dir(path, P).map_err(err_s)
}

/// Unlink everything in the Trash. Returns how many items were emptied.
pub fn empty() -> Result<usize, String> {
    let td = trash_dir();
    let r = empty_inner(&td);
    log("empty", &td, &r.clone().map(|_| ()));
    r
}

fn empty_inner(td: &str) -> Result<usize, String> {
    let t = mt();
    if t.stat(td).is_err() {
        return Ok(0);
    }
    let s = store();
    let mut n = 0usize;
    for e in t.read_dir(td).map_err(err_s)? {
        if e.name == "." || e.name == ".." || e.name.eq_ignore_ascii_case(INDEX) {
            continue;
        }
        let p = join(td, &e.name);
        if s == Store::Attrs {
            strip_keys(&t, &p); // the keys leave the attribute index with the object
        }
        delete_tree(&t, &p, 0)?;
        n += 1;
    }
    if s == Store::Index {
        write_index(&t, &[])?;
    }
    Ok(n)
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

/// TESTFIX3 (FLIGHT 19 `tests trash`): the `n`th collision name for `leaf`. The old `<leaf>~<n>`
/// (`TRASHFX.TXT~1`) is not an 8.3 name, and FAT's cross-directory `move_entry` refuses a non-8.3
/// destination (`[fs] mv … lfn=1 ok=false` -> `reason=Unsupported`), so the SECOND trash of a name
/// failed on the metal's FAT home. When `leaf` is itself 8.3 the suffix goes inside the stem
/// (`TRASHF~1.TXT`, the stem cut to keep 8 bytes) so the move stays representable; a leaf that is
/// already long keeps `<leaf>~<n>` (the volume either takes long names or refused the leaf already).
fn collision_name(leaf: &str, n: u32) -> String {
    let suffix = alloc::format!("~{}", n);
    if crate::fs::fat::is_long_name(leaf) {
        return alloc::format!("{}{}", leaf, suffix);
    }
    let (stem, ext) = match leaf.find('.') {
        Some(i) => (&leaf[..i], &leaf[i..]),
        None => (leaf, ""),
    };
    let keep = stem.len().min(8usize.saturating_sub(suffix.len()));
    alloc::format!("{}{}{}", &stem[..keep], suffix, ext)
}
