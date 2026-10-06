//! CHARTER: Kernel — fs-core
//!
//! SEARCH (rmbp-ledger B417 LAUNCHER; QUARRY3 B413 joins) — **find files by name.** ONE function the
//! launcher and Quarry's search field share: [`by_name`]`(prefix, limit)`.
//!
//! NAMEINDEX (B432): on a native UnaFS `/` with its name index ready, [`query`] is ONE range scan of the
//! volume's `una:fsname` index (`UnaFS::find_names`, the shared core) per keystroke — no walk, no snapshot;
//! `src=una-name-index`. The type database (`/system/types`, objects not documents) and the skip list are
//! filtered from its hits. Otherwise (a FAT root, or a volume whose index the login task is still building)
//! the old BOUNDED breadth-first walk through the VFS stands: [`snapshot`] once, [`filter`] per keystroke;
//! `src=walk`.
//!
//! Matching: the LEAF, case-insensitive — a prefix of the leaf, or of a word in it (`_ - . space` split);
//! the core's `unafs::name_match` is the same rule.
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

/// The source word the index answers under (`src=`).
pub const SRC_INDEX: &str = "una-name-index";

/// NAMEINDEX (B432): the paths whose leaf `prefix` names, best first, at most `limit`, from UnaFS's name
/// index — `None` when `/` is not a native volume with a ready index (the caller walks).
pub fn query(prefix: &str, limit: usize) -> Option<Vec<Hit>> {
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        let mt = crate::shell::vfs_mount_table();
        if !mt.volume_name("/").map(|n| n == "native").unwrap_or(false) {
            return None;
        }
        let f = crate::fs::unafs::with_unafs(|fs| fs.find_names(prefix, limit + 16)).ok()?.ok()?;
        if !f.indexed {
            return None;
        }
        let hidden = |p: &str| ["/volumes", "/system/types", "/proc", "/dev"].iter().any(|s| p == *s || (p.starts_with(s) && p.as_bytes().get(s.len()) == Some(&b'/')));
        return Some(f.hits.into_iter().filter(|h| !hidden(&h.path)).take(limit).map(|h| Hit { path: h.path, dir: h.dir }).collect());
    }
    #[allow(unreachable_code)]
    {
        let _ = (prefix, limit);
        None
    }
}

/// Is the index the source right now (`/` native, index ready)?
pub fn indexed() -> bool {
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        let native = crate::shell::vfs_mount_table().volume_name("/").map(|n| n == "native").unwrap_or(false);
        return native && matches!(crate::fs::unafs::with_unafs(|fs| fs.name_index_ready()), Ok(Ok(true)));
    }
    #[allow(unreachable_code)]
    false
}

/// **THE seam**: the paths whose leaf `prefix` names, best first, at most `limit` — the name index's one
/// range scan, else (no index) one walk. Returns the source word too.
pub fn by_name_src(prefix: &str, limit: usize) -> (Vec<String>, &'static str) {
    if let Some(h) = query(prefix, limit) {
        return (h.into_iter().map(|h| h.path).collect(), SRC_INDEX);
    }
    let s = snapshot(SNAP_BUDGET);
    (filter(&s.hits, prefix, limit).into_iter().map(|h| h.path).collect(), "walk")
}

/// [`by_name_src`] without the source word.
pub fn by_name(prefix: &str, limit: usize) -> Vec<String> {
    by_name_src(prefix, limit).0
}
