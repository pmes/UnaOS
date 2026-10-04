// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! TRASH (R75) — a desktop Trash. `/home/<user>/.Trash/` is created on first use through the mount
//! table under the user's namespace (DIRNS); `trash` MOVES an entry there with the same `rename`
//! the shell's `mv` uses (LFNMV2; a collision appends `~1`, `~2`); `.Trash/.index` is a text file of
//! `original-path<TAB>trashed-name<TAB>unix-time` lines (appended on trash, rewritten on restore/empty).
//! Quarry's menu and the shell's `trash` verb call these same bodies. Each op prints
//! `[trash] op=trash|restore|empty path= ok= reason=`.

use crate::fs::vfs::{MountTable, NodeKind, KERNEL_PRINCIPAL};
use alloc::string::String;
use alloc::vec::Vec;

const P: &str = KERNEL_PRINCIPAL;
const INDEX: &str = ".index";
const DIRNAME: &str = ".Trash";
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

/// One index line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub orig: String,
    pub name: String,
    pub when: u64,
}

fn log(op: &str, path: &str, r: &Result<(), String>) {
    match r {
        Ok(()) => serial_println!("[trash] op={} path={} ok=1 reason=-", op, path),
        Err(e) => serial_println!("[trash] op={} path={} ok=0 reason={}", op, path, e),
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
                v.push(Entry { orig: String::from(o), name: String::from(n), when: w.trim().parse().unwrap_or(0) });
            }
        }
    }
    v
}

/// The index, parsed. Empty when there is none.
pub fn entries() -> Vec<Entry> {
    let t = mt();
    let b = read_all(&t, &index_path());
    parse(core::str::from_utf8(&b).unwrap_or(""))
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
    move_logged(&t, path, &join(&td, &name))?;
    let e = Entry { orig: String::from(path), name: name.clone(), when: crate::clock::unix_now().unwrap_or(0) };
    if let Err(why) = append_index(&t, &e) {
        let _ = move_logged(&t, &join(&td, &name), path); // never leave an unindexed item
        return Err(alloc::format!("index: {}", why));
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
    move_logged(&t, &join(&trash_dir(), &es[i].name), &orig)?;
    es.remove(i);
    write_index(&t, &es)?;
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
    let mut n = 0usize;
    for e in t.read_dir(td).map_err(err_s)? {
        if e.name == "." || e.name == ".." || e.name.eq_ignore_ascii_case(INDEX) {
            continue;
        }
        delete_tree(&t, &join(td, &e.name), 0)?;
        n += 1;
    }
    write_index(&t, &[])?;
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
            console.println(&alloc::format!("{} item(s) in {}", es.len(), trash_dir()));
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

/// `:: TRASH: trashed= restored= emptied= index_ok= -> PASS ::` — create a scratch file under home,
/// trash it, restore it, trash it again, empty; the listing (and the index) is verified at each step.
pub fn selftest() {
    let t = mt();
    let base = home_base();
    if t.stat(&base).is_err() && t.create(&base, NodeKind::Dir, P).is_err() {
        serial_println!(":: TRASH: base={} reason=no-home -> SKIP ::", base);
        return;
    }
    let td = trash_dir();
    let name = "TRASHFX.TXT";
    let path = join(&base, name);
    let _ = empty(); // a clean slate (also creates nothing when absent)
    if t.stat(&path).is_ok() {
        let _ = t.unlink(&path, P);
    }
    let _ = t.create(&path, NodeKind::File, P).and_then(|_| t.write(&path, 0, b"trash me", P));
    let (mut trashed, mut restored, mut emptied, mut index_ok) = (0u32, 0u32, 0u32, 0u32);
    // trash
    let tn = trash(&path);
    if let Ok(n) = &tn {
        let es = entries();
        if !listed(&t, &base, name) && listed(&t, &td, n) && es.len() == 1 && es[0].orig == path && es[0].name == *n {
            trashed += 1;
            index_ok += 1;
        }
    }
    // restore
    if let Ok(n) = &tn {
        let rs = restore(n);
        if rs.is_ok() && listed(&t, &base, name) && !listed(&t, &td, n) && entries().is_empty() && t.read(&path, 0, 16).map(|b| b == b"trash me").unwrap_or(false) {
            restored += 1;
            index_ok += 1;
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
            index_ok += 1;
        }
    }
    // a collision gets ~1
    let _ = t.create(&path, NodeKind::File, P);
    let c1 = trash(&path);
    let _ = t.create(&path, NodeKind::File, P);
    let c2 = trash(&path);
    let coll = matches!((&c1, &c2), (Ok(a), Ok(b)) if a != b && b.contains("~1")); // TESTFIX3: `TRASHF~1.TXT` on FAT
    let _ = empty();
    let pass = trashed == 2 && restored == 1 && emptied == 1 && index_ok == 3 && coll;
    serial_println!(":: TRASH: trashed={} restored={} emptied={} index_ok={} -> {} ::", trashed, restored, emptied, index_ok, if pass { "PASS" } else { "FAIL" });
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
