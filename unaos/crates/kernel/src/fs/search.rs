//! CHARTER: Kernel — fs-core
//!
//! SEARCH (rmbp-ledger B417 LAUNCHER; QUARRY3 B413 joins) — **find files by name.** ONE function the
//! launcher and Quarry's search field share: [`by_name`]`(prefix, limit)`.
//!
//! The name index UnaFS keeps is the per-directory name B+tree (`ls`); no volume-wide name attribute is
//! indexed. So a search is a BOUNDED breadth-first walk of those trees through the VFS (`read_dir`, every
//! mounted volume, FAT included): the session home first, then `/`; `/volumes` (the same disks again) and
//! the type database (`/system/types`, objects not documents) are skipped. [`snapshot`] takes the walk once
//! (the launcher does it when it opens) and [`filter`] answers each keystroke from memory.
//!
//! Matching: the LEAF, case-insensitive — a prefix of the leaf, or of a word in it (`_ - . space` split).
use alloc::string::String;
use alloc::vec::Vec;

/// Entries a walk visits before it stops (the snapshot says `truncated`).
pub const SNAP_BUDGET: usize = 6000;
/// Directory depth below a root.
const MAX_DEPTH: usize = 10;

/// One name the walk found.
#[derive(Clone)]
pub struct Hit {
    pub path: String,
    pub dir: bool,
}

/// A walk's result: the names, and whether the budget cut it.
pub struct Snapshot {
    pub hits: Vec<Hit>,
    pub truncated: bool,
}

fn skip(path: &str) -> bool {
    matches!(path, "/volumes" | "/system/types" | "/proc" | "/dev")
}

fn join(dir: &str, name: &str) -> String {
    if dir == "/" { alloc::format!("/{}", name) } else { alloc::format!("{}/{}", dir, name) }
}

/// Walk the names under the session home, then `/`, breadth-first, at most `budget` entries.
pub fn snapshot(budget: usize) -> Snapshot {
    let mt = crate::shell::vfs_mount_table();
    let mut roots: Vec<String> = Vec::new();
    if let Some(h) = crate::prefs::home() {
        if !h.is_empty() {
            roots.push(h);
        }
    }
    roots.push(String::from("/"));
    let mut hits: Vec<Hit> = Vec::new();
    let mut seen_dirs: Vec<String> = Vec::new();
    let mut truncated = false;
    'roots: for r in roots.iter() {
        let mut q: alloc::collections::VecDeque<(String, usize)> = alloc::collections::VecDeque::new();
        q.push_back((r.clone(), 0));
        while let Some((dir, depth)) = q.pop_front() {
            if seen_dirs.iter().any(|d| *d == dir) {
                continue;
            }
            seen_dirs.push(dir.clone());
            let Ok(ents) = mt.read_dir(&dir) else { continue };
            for e in ents {
                if e.name == "." || e.name == ".." || e.name.is_empty() {
                    continue;
                }
                if hits.len() >= budget {
                    truncated = true;
                    break 'roots;
                }
                let p = join(&dir, &e.name);
                let is_dir = matches!(e.kind, crate::fs::vfs::NodeKind::Dir);
                if is_dir && !skip(&p) && depth + 1 < MAX_DEPTH {
                    q.push_back((p.clone(), depth + 1));
                }
                if !hits.iter().any(|h| h.path == p) {
                    hits.push(Hit { path: p, dir: is_dir });
                }
            }
        }
    }
    Snapshot { hits, truncated }
}

/// The leaf of `path`.
pub fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// How well `prefix` names `leaf`: 2 = a prefix of the leaf, 1 = a prefix of a word in it, `None` = no.
pub fn name_match(leaf: &str, prefix: &str) -> Option<u8> {
    if prefix.is_empty() {
        return None;
    }
    let lb = leaf.as_bytes();
    let pb = prefix.as_bytes();
    let at = |i: usize| lb.len() >= i + pb.len() && lb[i..i + pb.len()].eq_ignore_ascii_case(pb);
    if at(0) {
        return Some(2);
    }
    for i in 1..lb.len() {
        if matches!(lb[i - 1], b'_' | b'-' | b'.' | b' ') && at(i) {
            return Some(1);
        }
    }
    None
}

/// The snapshot's names matching `prefix`, best first (leaf prefix, then word prefix; files before
/// directories; shallower first — the walk's order), at most `limit`.
pub fn filter(snap: &[Hit], prefix: &str, limit: usize) -> Vec<Hit> {
    let mut scored: Vec<(u8, usize, &Hit)> = Vec::new();
    for (i, h) in snap.iter().enumerate() {
        if let Some(q) = name_match(leaf(&h.path), prefix) {
            let rank = q * 2 + (!h.dir) as u8;
            scored.push((rank, i, h));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, _, h)| h.clone()).collect()
}

/// **THE seam**: the paths whose leaf `prefix` names, best first, at most `limit` (one walk).
pub fn by_name(prefix: &str, limit: usize) -> Vec<String> {
    let s = snapshot(SNAP_BUDGET);
    filter(&s.hits, prefix, limit).into_iter().map(|h| h.path).collect()
}
