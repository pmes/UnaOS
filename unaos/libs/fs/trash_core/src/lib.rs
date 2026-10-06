// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — shared-core
//!
//! `trash_core` — the ONE Trash (TRASHCORE, rmbp-ledger B449; ARCHREVIEW F5; R79). Matrix
//! (`handlers/matrix/src/trash.rs`, `VaultTrash` over a host UnaFS vault) and the kernel
//! (`unaos/crates/kernel/src/fs/trash.rs`, over the mount table) both link this crate; neither keeps a
//! Trash rule of its own. Each brings only its I/O, as an implementation of [`TrashFs`].
//!
//! # What a trashed object is — chosen in ONE function, [`store`]
//! * **attrs** — the home volume gives objects an identity (UnaFS): the object carries
//!   [`ATTR_KEY_TRASH_ORIGIN`] (the original absolute path), [`ATTR_KEY_TRASH_TIME`] (unix seconds,
//!   Int) and [`ATTR_KEY_TRASH_BY`] (the user), and is renamed into `<home>/.Trash/` (a rekey: the id
//!   survives). The listing is [`TRASH_QUERY`] scoped to the Trash's direct children; restore moves the
//!   id's CURRENT path back (a renamed trashed item still restores); empty strips the keys, then
//!   unlinks. No index file.
//! * **index** — the FAT fallback (no attributes): `.Trash/.index`, one
//!   `original-path<TAB>trashed-name<TAB>unix-time` line per item.
//!
//! # The Trash query folder (QUERYFOLDER, B420)
//! A Finder showing a folder asks [`is_trash_folder`]; its "Where from" column is [`origins`] of
//! [`entries`].
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub use una_abi::{ATTR_KEY_TRASH_BY, ATTR_KEY_TRASH_ORIGIN, ATTR_KEY_TRASH_TIME, TRASH_DIR_NAME, TRASH_QUERY};

/// The FAT fallback's index file, inside the Trash.
pub const INDEX: &str = ".index";
/// The three keys a trashed object carries.
pub const KEYS: [&str; 3] = [ATTR_KEY_TRASH_ORIGIN, ATTR_KEY_TRASH_TIME, ATTR_KEY_TRASH_BY];

/// What `stat` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub dir: bool,
    /// The object id (inode) on a volume that has them; `None` on FAT.
    pub id: Option<u64>,
}

/// An attribute value as the Trash uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attr {
    Str(String),
    Int(i64),
}

/// The I/O seam: the volume the Trash lives on, by absolute path.
pub trait TrashFs {
    fn stat(&mut self, path: &str) -> Option<Node>;
    fn mkdir(&mut self, path: &str) -> Result<(), String>;
    fn rename(&mut self, src: &str, dst: &str) -> Result<(), String>;
    fn set_attr(&mut self, path: &str, key: &str, v: Attr) -> Result<(), String>;
    fn get_attr(&mut self, path: &str, key: &str) -> Option<Attr>;
    /// Absent keys are not an error.
    fn remove_attr(&mut self, path: &str, key: &str);
    /// `(object id, current absolute path)` of every match.
    fn query(&mut self, q: &str) -> Vec<(u64, String)>;
    /// The names in `dir` (`.`/`..` may appear; they are skipped).
    fn list(&mut self, dir: &str) -> Result<Vec<String>, String>;
    fn unlink(&mut self, path: &str) -> Result<(), String>;
    fn rmdir(&mut self, path: &str) -> Result<(), String>;
    /// The whole file, or `None` when it does not exist.
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>>;
    /// Replace the file's contents (create it when absent).
    fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), String>;
    /// Append to the file (create it when absent).
    fn append_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), String>;
    /// Names compare without case (FAT).
    fn case_insensitive(&self) -> bool {
        false
    }
    /// The volume can only move to 8.3 names AND `leaf` is one — so a collision suffix must go inside
    /// the stem (`TRASHF~1.TXT`) to stay representable. Default: names are free (`leaf~1`).
    fn short_names(&self, _leaf: &str) -> bool {
        false
    }
    /// How deep `empty` descends into a trashed folder (a kernel stack bounds it).
    fn max_depth(&self) -> usize {
        32
    }
}

/// Where a trashed object's record lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    /// On the object: the `una:trash-*` attributes.
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

/// One trashed item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `una:trash-origin` (or the index's first column).
    pub orig: String,
    /// The name it carries inside the Trash now.
    pub name: String,
    /// Unix seconds (0 when unknown).
    pub when: u64,
    /// `una:trash-by` (empty on the index fallback).
    pub by: String,
    /// The object id on an attribute store (`None` on the index fallback).
    pub id: Option<u64>,
}

/// Who trashes, where, and when — the facts the caller's ring knows.
#[derive(Clone, Copy, Debug)]
pub struct Ctx<'a> {
    /// `/home/<user>` (or `/home` with no session).
    pub home: &'a str,
    /// Stamped as `una:trash-by`.
    pub user: &'a str,
    /// Unix seconds.
    pub now: u64,
}

pub fn join(d: &str, n: &str) -> String {
    if d.ends_with('/') { format!("{d}{n}") } else { format!("{d}/{n}") }
}

pub fn leaf(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

pub fn parent(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &p[..i],
    }
}

fn eq(ci: bool, a: &str, b: &str) -> bool {
    if ci { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// `path` is `prefix` or below it (component-wise).
fn under(ci: bool, path: &str, prefix: &str) -> bool {
    path.len() >= prefix.len()
        && eq(ci, &path[..prefix.len()], prefix)
        && (path.len() == prefix.len() || path.as_bytes()[prefix.len()] == b'/')
}

/// The Trash folder of `home`.
pub fn trash_dir(home: &str) -> String {
    join(home, TRASH_DIR_NAME)
}

fn index_path(home: &str) -> String {
    join(&trash_dir(home), INDEX)
}

/// QUERYFOLDER: is `cwd` the Trash of `home`?
pub fn is_trash_folder(fs: &impl TrashFs, cwd: &str, home: &str) -> bool {
    eq(fs.case_insensitive(), cwd.trim_end_matches('/'), &trash_dir(home))
}

/// QUERYFOLDER: the Trash folder's `(name, origin)` pairs, for its "Where from" column.
pub fn origins(es: &[Entry]) -> Vec<(String, String)> {
    es.iter().map(|e| (e.name.clone(), e.orig.clone())).collect()
}

/// THE one choice: attributes where the home volume gives objects an identity, else the index.
pub fn store(fs: &mut impl TrashFs, home: &str) -> Store {
    match fs.stat(home) {
        Some(n) if n.id.is_some() => Store::Attrs,
        _ => Store::Index,
    }
}

/// The `n`th collision name for `leaf`: `<leaf>~<n>`, or inside an 8.3 stem (`TRASHF~1.TXT`) when the
/// volume can only move to short names (TESTFIX3: FAT's cross-directory move refuses a long name).
pub fn collision_name(leaf: &str, n: u32, short: bool) -> String {
    let suffix = format!("~{n}");
    if !short {
        return format!("{leaf}{suffix}");
    }
    let (stem, ext) = match leaf.find('.') {
        Some(i) => (&leaf[..i], &leaf[i..]),
        None => (leaf, ""),
    };
    let keep = stem.len().min(8usize.saturating_sub(suffix.len()));
    format!("{}{}{}", &stem[..keep], suffix, ext)
}

/// The index fallback's lines.
pub fn parse_index(text: &str) -> Vec<Entry> {
    let mut v = Vec::new();
    for l in text.lines() {
        let mut it = l.split('\t');
        if let (Some(o), Some(n), Some(w)) = (it.next(), it.next(), it.next()) {
            if !o.is_empty() && !n.is_empty() {
                v.push(Entry { orig: o.to_string(), name: n.to_string(), when: w.trim().parse().unwrap_or(0), by: String::new(), id: None });
            }
        }
    }
    v
}

pub fn render_index(es: &[Entry]) -> String {
    let mut s = String::new();
    for e in es {
        s.push_str(&format!("{}\t{}\t{}\n", e.orig, e.name, e.when));
    }
    s
}

/// What the Trash of `home` holds.
pub fn entries(fs: &mut impl TrashFs, home: &str) -> Vec<Entry> {
    match store(fs, home) {
        Store::Attrs => attr_entries(fs, home),
        Store::Index => {
            let b = fs.read_file(&index_path(home)).unwrap_or_default();
            parse_index(core::str::from_utf8(&b).unwrap_or(""))
        }
    }
}

fn attr_entries(fs: &mut impl TrashFs, home: &str) -> Vec<Entry> {
    let td = trash_dir(home);
    let ci = fs.case_insensitive();
    let mut v = Vec::new();
    for (id, path) in fs.query(TRASH_QUERY) {
        if !eq(ci, parent(&path), &td) {
            continue; // another user's Trash, or a stamped object moved out by hand
        }
        let orig = match fs.get_attr(&path, ATTR_KEY_TRASH_ORIGIN) {
            Some(Attr::Str(s)) if !s.is_empty() => s,
            _ => continue,
        };
        let when = match fs.get_attr(&path, ATTR_KEY_TRASH_TIME) {
            Some(Attr::Int(i)) if i > 0 => i as u64,
            _ => 0,
        };
        let by = match fs.get_attr(&path, ATTR_KEY_TRASH_BY) {
            Some(Attr::Str(s)) => s,
            _ => String::new(),
        };
        v.push(Entry { orig, name: leaf(&path).to_string(), when, by, id: Some(id) });
    }
    v
}

fn ensure_dir(fs: &mut impl TrashFs, path: &str, depth: usize) -> Result<(), String> {
    match fs.stat(path) {
        Some(n) if n.dir => Ok(()),
        Some(_) => Err(format!("{} is a file", leaf(path))),
        None if depth > 16 || path == "/" => Err(format!("cannot create {path}")),
        None => {
            ensure_dir(fs, parent(path), depth + 1)?;
            fs.mkdir(path)
        }
    }
}

fn strip(fs: &mut impl TrashFs, path: &str) {
    for k in KEYS {
        fs.remove_attr(path, k);
    }
}

fn stamp(fs: &mut impl TrashFs, path: &str, cx: &Ctx) -> Result<(), String> {
    fs.set_attr(path, ATTR_KEY_TRASH_ORIGIN, Attr::Str(path.to_string()))?;
    fs.set_attr(path, ATTR_KEY_TRASH_TIME, Attr::Int(cx.now as i64))?;
    fs.set_attr(path, ATTR_KEY_TRASH_BY, Attr::Str(cx.user.to_string()))
}

/// Move `path` into the Trash. Returns the trashed name.
pub fn trash(fs: &mut impl TrashFs, cx: &Ctx, path: &str) -> Result<String, String> {
    let ci = fs.case_insensitive();
    let home = cx.home;
    if path.split('/').any(|c| c == "..") {
        return Err("a path with .. leaves the home folder".into());
    }
    if !under(ci, path, home) {
        return Err(format!("files are trashed only under {home}"));
    }
    if path.trim_end_matches('/').len() == home.len() {
        return Err("the home folder itself cannot be trashed".into());
    }
    let td = trash_dir(home);
    if under(ci, path, &td) {
        return Err("already in the trash".into());
    }
    let lf = leaf(path);
    if lf.is_empty() || lf.contains('\t') || lf.contains('\n') {
        return Err("bad-name".into());
    }
    if fs.stat(path).is_none() {
        return Err(format!("no such path: {path}"));
    }
    ensure_dir(fs, &td, 0)?;
    let short = fs.short_names(lf);
    let mut name = lf.to_string();
    let mut n = 0u32;
    while fs.stat(&join(&td, &name)).is_some() {
        n += 1;
        if n > 99 {
            return Err("too-many-collisions".into());
        }
        name = collision_name(lf, n, short);
    }
    let dst = join(&td, &name);
    match store(fs, home) {
        Store::Attrs => {
            // The record goes ON the object first, then the rekeying rename: a failure between the two
            // leaves a stamped object outside the Trash, which the scoped query ignores and the strip
            // removes.
            if let Err(why) = stamp(fs, path, cx) {
                strip(fs, path);
                return Err(format!("attrs: {why}"));
            }
            if let Err(why) = fs.rename(path, &dst) {
                strip(fs, path);
                return Err(why);
            }
        }
        Store::Index => {
            fs.rename(path, &dst)?;
            let e = Entry { orig: path.to_string(), name: name.clone(), when: cx.now, by: String::new(), id: None };
            if let Err(why) = fs.append_file(&index_path(home), render_index(&[e]).as_bytes()) {
                let _ = fs.rename(&dst, path); // never leave an unindexed item
                return Err(format!("index: {why}"));
            }
        }
    }
    Ok(name)
}

/// Move the trashed `name` back to its original path. Returns that path.
pub fn restore(fs: &mut impl TrashFs, home: &str, name: &str) -> Result<String, String> {
    let ci = fs.case_insensitive();
    let mut es = entries(fs, home);
    let i = es.iter().position(|e| eq(ci, &e.name, name)).ok_or_else(|| String::from("not-in-trash"))?;
    let orig = es[i].orig.clone();
    let pd = parent(&orig);
    match fs.stat(pd) {
        Some(n) if n.dir => {}
        _ => return Err(format!("the original folder {pd} is gone")),
    }
    if fs.stat(&orig).is_some() {
        return Err(format!("{orig} already exists"));
    }
    let cur = join(&trash_dir(home), &es[i].name);
    match es[i].id {
        Some(id) => {
            // The id's CURRENT path, from the query that just named it (renamed items included).
            if fs.stat(&cur).and_then(|n| n.id) != Some(id) {
                return Err("trashed object moved".into());
            }
            fs.rename(&cur, &orig)?;
            strip(fs, &orig);
        }
        None => {
            fs.rename(&cur, &orig)?;
            es.remove(i);
            fs.write_file(&index_path(home), render_index(&es).as_bytes())?;
        }
    }
    Ok(orig)
}

fn delete_tree(fs: &mut impl TrashFs, path: &str, depth: usize) -> Result<(), String> {
    let n = fs.stat(path).ok_or_else(|| format!("no such path: {path}"))?;
    if !n.dir {
        return fs.unlink(path);
    }
    if depth > fs.max_depth() {
        return Err("too-deep".into());
    }
    for c in fs.list(path)? {
        if c == "." || c == ".." {
            continue;
        }
        delete_tree(fs, &join(path, &c), depth + 1)?;
    }
    fs.rmdir(path)
}

/// Unlink everything in the Trash of `home`. Returns how many items were emptied.
pub fn empty(fs: &mut impl TrashFs, home: &str) -> Result<usize, String> {
    let td = trash_dir(home);
    if fs.stat(&td).is_none() {
        return Ok(0);
    }
    let s = store(fs, home);
    let ci = fs.case_insensitive();
    let mut n = 0usize;
    for c in fs.list(&td)? {
        if c == "." || c == ".." || (s == Store::Index && eq(ci, &c, INDEX)) {
            continue;
        }
        let p = join(&td, &c);
        if s == Store::Attrs {
            strip(fs, &p); // the keys leave the attribute index with the object
        }
        delete_tree(fs, &p, 0)?;
        n += 1;
    }
    if s == Store::Index {
        fs.write_file(&index_path(home), b"")?;
    }
    Ok(n)
}
