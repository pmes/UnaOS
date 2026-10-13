// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAIDE2 (LEDGER SR111): the workspace's git state, READ-ONLY, through Vairë's core — `git_core` (GITCORE SR59,
// CHARTER: Vaire — shared-core; no_std + alloc, its `std` Repository NOT enabled). The core does the format work:
// the index (`index::Index::parse`), a blob's id (`object::hash_object`), a loose object (`loose::decode`), a pack
// entry with its delta chain (`pack::Idx::lookup` + `pack::Pack::object_at`), gitignore matching (`ignore::Ignore`)
// and git's line diff (`diff::xdiff`). This file brings only the I/O, over the ring-3 file surface (fs.rs: SYS_STAT,
// SYS_PATH_READ, PATH_R_LIST). Nothing here writes under `.git`.
//
// DISCOVERY. `<workspace root>/.git` only — a directory, or a worktree link file (`gitdir: <p>`, then `commondir`
// for the shared object store). The object format is the common config's `objectformat` (sha1 unless sha256). The
// index is read whole (INDEX_MAX); a repository with no index yet reads as an empty one. info/exclude and the root
// `.gitignore` are the ignore lists.
//
// MARKS (`mark`): a FILE is `M` when it is in the index and its size, or the blob id of its bytes (files past
// HASH_MAX are judged by size alone), differs from the entry; `?` when it is not in the index and not ignored. A
// FOLDER is `?` when no index path lies beneath it and it is not ignored. Gitlinks and symlinks stay unmarked.
//
// STATUS (`status`): the workspace walked over PATH_R_LIST (WALK_MAX files, DEPTH_MAX levels — past either the walk
// is `truncated` and names no deletions), untracked folders collapsed to `dir/` as git prints them.
//
// DIFF (`diff`): the index blob (loose, else a pack under objects/pack — the idx and the pack read whole, PACK_MAX
// each, past it refused by name) against the bytes on the volume, Myers with git's indent heuristic and 3 lines of
// context; an untracked file diffs against nothing (`--- /dev/null`), a binary side prints git's one line.

use crate::fs::{errno_word, list, read_file, resolve, stat};
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use git_core::diff::xdiff;
use git_core::ignore::{Ignore, PatternList};
use git_core::index::Index;
use git_core::object::{self, Kind};
use git_core::pack::{Idx, Pack};
use git_core::{HashKind, ObjectId};

const INDEX_MAX: usize = 4 << 20;
/// Files larger than this are judged by size alone (a hash would read them whole).
const HASH_MAX: usize = 4 << 20;
const OBJ_MAX: usize = 16 << 20;
const PACK_MAX: usize = 24 << 20;
const WALK_MAX: usize = 4000;
const DEPTH_MAX: usize = 12;
const MODE_LINK: u32 = 0o120000;
const MODE_GITLINK: u32 = 0o160000;

fn join(d: &str, n: &str) -> String {
    if d.ends_with('/') {
        alloc::format!("{}{}", d, n)
    } else {
        alloc::format!("{}/{}", d, n)
    }
}

/// The first line of a small text file, trimmed.
fn first_line(path: &str) -> Option<String> {
    let b = read_file(path, 4096, false).ok()?;
    let t = String::from_utf8_lossy(&b).into_owned();
    t.lines().next().map(|l| String::from(l.trim())).filter(|l| !l.is_empty())
}

pub struct Repo {
    /// The workspace root (normal, absolute) the repository's paths are relative to.
    pub root: String,
    pub gitdir: String,
    objects: String,
    pub hk: HashKind,
    index: Index,
    ignore: Ignore,
}

#[derive(Default)]
pub struct Status {
    pub modified: Vec<String>,
    pub untracked: Vec<String>,
    pub deleted: Vec<String>,
    pub truncated: bool,
}

/// The repository at `<root>/.git`: `Ok(None)` when there is none, `Err(why)` when it cannot be read.
pub fn open(root: &str) -> Result<Option<Repo>, String> {
    let root = resolve("/", root);
    let dotgit = join(&root, ".git");
    let Ok(st) = stat(&dotgit) else { return Ok(None) };
    let gitdir = if st.kind == 1 {
        dotgit
    } else {
        let l = first_line(&dotgit).ok_or_else(|| String::from(".git file unreadable"))?;
        let p = l.strip_prefix("gitdir:").ok_or_else(|| String::from(".git file without gitdir:"))?;
        resolve(&root, p.trim())
    };
    let common = first_line(&join(&gitdir, "commondir")).map_or_else(|| gitdir.clone(), |c| resolve(&gitdir, &c));
    let cfg = read_file(&join(&common, "config"), 64 * 1024, false).unwrap_or_default();
    let flat: Vec<u8> = cfg.iter().filter(|b| !b.is_ascii_whitespace()).map(|b| b.to_ascii_lowercase()).collect();
    let hk = if flat.windows(19).any(|w| w == b"objectformat=sha256") { HashKind::Sha256 } else { HashKind::Sha1 };
    let index = match read_file(&join(&gitdir, "index"), INDEX_MAX, true) {
        Ok(b) => Index::parse(hk, &b).map_err(|e| alloc::format!("index: {}", e))?,
        Err(una_abi::ENOENT) => Index::new(hk),
        Err(e) => return Err(alloc::format!("index: {} {}", errno_word(e), e)),
    };
    let mut ignore = Ignore::default();
    for p in [join(&join(&common, "info"), "exclude"), join(&root, ".gitignore")] {
        if let Ok(t) = read_file(&p, 256 * 1024, false) {
            ignore.lists.push(PatternList::parse(&t, b""));
        }
    }
    Ok(Some(Repo { root, gitdir, objects: join(&common, "objects"), hk, index, ignore }))
}

impl Repo {
    pub fn entries(&self) -> usize {
        self.index.entries.len()
    }
    pub fn hk_name(&self) -> &'static str {
        match self.hk {
            HashKind::Sha1 => "sha1",
            HashKind::Sha256 => "sha256",
        }
    }

    /// `path` relative to the root, or `None` outside it (the root itself is `""`).
    fn rel<'a>(&self, path: &'a str) -> Option<&'a str> {
        if self.root == "/" {
            return path.strip_prefix('/');
        }
        let r = path.strip_prefix(self.root.as_str())?;
        if r.is_empty() {
            Some(r)
        } else {
            r.strip_prefix('/')
        }
    }

    fn ignored(&self, rel: &str, dir: bool) -> bool {
        self.ignore.is_excluded_with_parents(rel.as_bytes(), dir)
    }

    /// Does any index path lie beneath folder `rel`?
    fn tracked_under(&self, rel: &str) -> bool {
        let mut pre = Vec::from(rel.as_bytes());
        pre.push(b'/');
        let i = self.index.entries.partition_point(|e| e.path.as_slice() < pre.as_slice());
        self.index.entries.get(i).is_some_and(|e| e.path.starts_with(&pre))
    }

    /// The tree row's mark for `path`: `M`, `?`, or a space.
    pub fn mark(&self, path: &str, dir: bool) -> u8 {
        let Some(rel) = self.rel(path) else { return b' ' };
        if rel.is_empty() || rel == ".git" || rel.starts_with(".git/") {
            return b' ';
        }
        if dir {
            return if self.tracked_under(rel) || self.ignored(rel, true) { b' ' } else { b'?' };
        }
        let Some(e) = self.index.find(rel.as_bytes()) else {
            return if self.ignored(rel, false) { b' ' } else { b'?' };
        };
        let Some(id) = e.id else { return b' ' };
        if e.mode == MODE_LINK || e.mode == MODE_GITLINK {
            return b' ';
        }
        let Ok(st) = stat(path) else { return b' ' };
        if st.size as u32 != e.size {
            return b'M';
        }
        if st.size as usize > HASH_MAX {
            return b' ';
        }
        match read_file(path, HASH_MAX, true) {
            Ok(bytes) if object::hash_object(self.hk, Kind::Blob, &bytes) != id => b'M',
            _ => b' ',
        }
    }

    /// The whole workspace's state (bounded walk).
    pub fn status(&self) -> Status {
        let mut st = Status::default();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut stack: Vec<(String, usize)> = alloc::vec![(self.root.clone(), 0)];
        let mut files = 0usize;
        'walk: while let Some((dir, depth)) = stack.pop() {
            let Ok(names) = list(&dir) else { continue };
            for n in names {
                let isdir = n.ends_with('/');
                let name = n.trim_end_matches('/');
                if name.is_empty() || name == "." || name == ".." || name == ".git" {
                    continue;
                }
                let p = join(&dir, name);
                let Some(rel) = self.rel(&p).map(String::from) else { continue };
                if self.ignored(&rel, isdir) {
                    continue;
                }
                if isdir {
                    if !self.tracked_under(&rel) {
                        st.untracked.push(alloc::format!("{}/", rel));
                    } else if depth + 1 < DEPTH_MAX {
                        stack.push((p, depth + 1));
                    } else {
                        st.truncated = true;
                    }
                    continue;
                }
                files += 1;
                if files > WALK_MAX {
                    st.truncated = true;
                    break 'walk;
                }
                match self.mark(&p, false) {
                    b'M' => st.modified.push(rel.clone()),
                    b'?' => st.untracked.push(rel.clone()),
                    _ => {}
                }
                seen.insert(rel);
            }
        }
        if !st.truncated {
            for e in self.index.entries.iter().filter(|e| e.stage() == 0 && e.mode != MODE_GITLINK) {
                let p = String::from_utf8_lossy(&e.path).into_owned();
                if !seen.contains(&p) {
                    st.deleted.push(p);
                }
            }
        }
        st.modified.sort();
        st.untracked.sort();
        st
    }

    /// An object by id: loose first, then every pack whose idx names it.
    fn read_object(&self, id: &ObjectId) -> Result<(Kind, Vec<u8>), String> {
        let hex = id.to_hex();
        let p = alloc::format!("{}/{}/{}", self.objects, &hex[..2], &hex[2..]);
        match read_file(&p, OBJ_MAX, true) {
            Ok(f) => return git_core::loose::decode(&f, OBJ_MAX).map_err(|e| alloc::format!("loose {}: {}", &hex[..7], e)),
            Err(una_abi::ENOENT) => {}
            Err(e) => return Err(alloc::format!("loose {}: {} {}", &hex[..7], errno_word(e), e)),
        }
        let packdir = join(&self.objects, "pack");
        let names = list(&packdir).unwrap_or_default();
        for n in names.iter().filter(|n| n.ends_with(".idx")) {
            let ip = join(&packdir, n);
            let Ok(ib) = read_file(&ip, PACK_MAX, true) else { continue };
            let Ok(idx) = Idx::parse(&ib, self.hk, false) else { continue };
            let Some(off) = idx.lookup(id) else { continue };
            let pp = alloc::format!("{}.pack", &ip[..ip.len() - 4]);
            let pb = read_file(&pp, PACK_MAX, true).map_err(|e| alloc::format!("pack {}: {} {} (bound {} MiB)", n, errno_word(e), e, PACK_MAX >> 20))?;
            let pack = Pack::parse(&pb, self.hk, false).map_err(|e| alloc::format!("pack {}: {}", n, e))?;
            return pack.object_at(off, &|i| idx.lookup(i), &mut |_| None).map_err(|e| alloc::format!("pack {}: {}", n, e));
        }
        Err(alloc::format!("object {} not found (loose or packed)", &hex[..7]))
    }

    /// `git diff -- <path>` (the index against the volume): the unified text, empty when nothing differs.
    pub fn diff(&self, path: &str) -> Result<Vec<u8>, String> {
        let rel = self.rel(path).filter(|r| !r.is_empty()).ok_or_else(|| String::from("not inside the repository"))?;
        let new = read_file(path, OBJ_MAX, true).map_err(|e| alloc::format!("{} {}", errno_word(e), e))?;
        let (old, tracked) = match self.index.find(rel.as_bytes()).and_then(|e| e.id) {
            Some(id) => match self.read_object(&id)? {
                (Kind::Blob, d) => (d, true),
                _ => return Err(String::from("the index entry is not a blob")),
            },
            None => (Vec::new(), false),
        };
        let mut out = Vec::new();
        if old == new {
            return Ok(out);
        }
        let a = if tracked { alloc::format!("a/{}", rel) } else { String::from("/dev/null") };
        if git_core::diff::is_binary(&old) || git_core::diff::is_binary(&new) {
            out.extend_from_slice(alloc::format!("Binary files {} and b/{} differ\n", a, rel).as_bytes());
            return Ok(out);
        }
        let env = xdiff::diff(&old, &new, xdiff::Algorithm::Myers, true);
        let script = xdiff::build_script(&env);
        out.extend_from_slice(alloc::format!("--- {}\n+++ b/{}\n", a, rel).as_bytes());
        xdiff::emit(&env, &script, &xdiff::EmitOptions::default(), &mut out);
        Ok(out)
    }
}
