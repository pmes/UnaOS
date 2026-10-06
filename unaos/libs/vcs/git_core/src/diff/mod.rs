// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `git diff <tree-ish> <tree-ish>`: the tree walk, rename detection, the patch and `--stat`
//! renderings. The line engine is [`xdiff`]; everything here reproduces git's observable output
//! (`git-diff(1)`, `diff-format`, `diffcore(7)`): the extended header lines, abbreviated index
//! lines (unique prefixes, `core.abbrev` auto length), C-quoted paths, the trailing TAB on
//! `---`/`+++` names with spaces, binary detection (NUL in the first 8000 bytes or `-diff`),
//! type changes split into delete + create, submodule pseudo-content, rename detection with git's
//! scoring (exact by id, then unique basenames at the 75% bar, then the similarity matrix with
//! four candidates per destination, 50% minimum, span-hash similarity in 64-byte/line chunks),
//! and the diffstat layout (80 columns, name truncation with `...`, graph scaling, `{a => b}`).

pub mod xdiff;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::hash::ObjectId;
use crate::object::{self, mode, Kind, Tree};
use crate::{Error, Result};

pub use xdiff::Algorithm;

/// Where objects come from (a repository, a pack, a test fixture).
pub trait ObjectSource {
    /// Read an object.
    fn read_object(&self, id: &ObjectId) -> Result<(Kind, Vec<u8>)>;
    /// The shortest unique hex prefix of `id` with at least `min` digits.
    fn unique_abbrev(&self, id: &ObjectId, min: usize) -> usize {
        let _ = id;
        min
    }
    /// The default abbreviation length (`core.abbrev` / auto).
    fn default_abbrev(&self) -> usize {
        7
    }
}

/// One side of a file pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    /// Path from the root.
    pub path: Vec<u8>,
    /// Mode.
    pub mode: u32,
    /// Blob (or gitlink commit) id.
    pub id: ObjectId,
}

/// A file pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    /// Pre-image (`None` = created).
    pub old: Option<FileSpec>,
    /// Post-image (`None` = deleted).
    pub new: Option<FileSpec>,
    /// Rename score (0..=60000) when this is a rename.
    pub rename_score: Option<u32>,
}

/// git's `MAX_SCORE`.
pub const MAX_SCORE: u32 = 60000;
/// Default `-M` threshold (50%).
pub const DEFAULT_RENAME_SCORE: u32 = 30000;

fn read_tree(src: &dyn ObjectSource, id: &ObjectId) -> Result<Tree> {
    let (k, d) = src.read_object(id)?;
    if k != Kind::Tree {
        return Err(Error::Corrupt("diff: expected a tree"));
    }
    Tree::parse(id.kind(), &d)
}

fn join(base: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(base.len() + name.len() + 1);
    p.extend_from_slice(base);
    if !base.is_empty() {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

fn add_all(src: &dyn ObjectSource, base: &[u8], t: &Tree, added: bool, out: &mut Vec<Pair>) -> Result<()> {
    for e in &t.entries {
        let p = join(base, &e.name);
        if e.is_tree() {
            let sub = read_tree(src, &e.id)?;
            add_all(src, &p, &sub, added, out)?;
        } else {
            let f = FileSpec { path: p, mode: e.mode, id: e.id };
            out.push(if added { Pair { old: None, new: Some(f), rename_score: None } } else { Pair { old: Some(f), new: None, rename_score: None } });
        }
    }
    Ok(())
}

/// The raw changes between two trees (`None` = the empty tree), in git's tree order.
pub fn tree_changes(src: &dyn ObjectSource, a: Option<&ObjectId>, b: Option<&ObjectId>) -> Result<Vec<Pair>> {
    let ta = match a {
        Some(id) => read_tree(src, id)?,
        None => Tree::default(),
    };
    let tb = match b {
        Some(id) => read_tree(src, id)?,
        None => Tree::default(),
    };
    let mut out = Vec::new();
    walk(src, b"", &ta, &tb, &mut out)?;
    Ok(out)
}

fn walk(src: &dyn ObjectSource, base: &[u8], a: &Tree, b: &Tree, out: &mut Vec<Pair>) -> Result<()> {
    let (mut i, mut j) = (0, 0);
    while i < a.entries.len() || j < b.entries.len() {
        let ord = match (a.entries.get(i), b.entries.get(j)) {
            (Some(x), Some(y)) => object::tree_order(&x.name, x.is_tree(), &y.name, y.is_tree()),
            (Some(_), None) => core::cmp::Ordering::Less,
            _ => core::cmp::Ordering::Greater,
        };
        // Same name but one is a tree: tree_order separates them ("a" < "a/"); git's tree walk
        // compares names alone and then treats a file<->dir switch as delete + add.
        let same_name = match (a.entries.get(i), b.entries.get(j)) {
            (Some(x), Some(y)) => x.name == y.name,
            _ => false,
        };
        if same_name {
            let x = &a.entries[i];
            let y = &b.entries[j];
            i += 1;
            j += 1;
            let p = join(base, &x.name);
            match (x.is_tree(), y.is_tree()) {
                (true, true) => {
                    if x.id != y.id {
                        walk(src, &p, &read_tree(src, &x.id)?, &read_tree(src, &y.id)?, out)?;
                    }
                }
                (false, false) => {
                    if x.id != y.id || x.mode != y.mode {
                        out.push(Pair {
                            old: Some(FileSpec { path: p.clone(), mode: x.mode, id: x.id }),
                            new: Some(FileSpec { path: p, mode: y.mode, id: y.id }),
                            rename_score: None,
                        });
                    }
                }
                (false, true) => {
                    out.push(Pair { old: Some(FileSpec { path: p.clone(), mode: x.mode, id: x.id }), new: None, rename_score: None });
                    add_all(src, &p, &read_tree(src, &y.id)?, true, out)?;
                }
                (true, false) => {
                    add_all(src, &p, &read_tree(src, &x.id)?, false, out)?;
                    out.push(Pair { old: None, new: Some(FileSpec { path: p, mode: y.mode, id: y.id }), rename_score: None });
                }
            }
            continue;
        }
        if ord == core::cmp::Ordering::Less {
            let x = &a.entries[i];
            i += 1;
            let p = join(base, &x.name);
            if x.is_tree() {
                add_all(src, &p, &read_tree(src, &x.id)?, false, out)?;
            } else {
                out.push(Pair { old: Some(FileSpec { path: p, mode: x.mode, id: x.id }), new: None, rename_score: None });
            }
        } else {
            let y = &b.entries[j];
            j += 1;
            let p = join(base, &y.name);
            if y.is_tree() {
                add_all(src, &p, &read_tree(src, &y.id)?, true, out)?;
            } else {
                out.push(Pair { old: None, new: Some(FileSpec { path: p, mode: y.mode, id: y.id }), rename_score: None });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Content helpers
// ---------------------------------------------------------------------------------------------

fn is_reg(m: u32) -> bool {
    m & mode::TYPE_MASK == 0o100000
}

/// The bytes a file side diffs as: the blob, or for a gitlink `Subproject commit <hex>\n`.
pub fn content(src: &dyn ObjectSource, f: &FileSpec) -> Result<Vec<u8>> {
    if f.mode & mode::TYPE_MASK == mode::COMMIT {
        let mut v = b"Subproject commit ".to_vec();
        v.extend_from_slice(f.id.to_hex().as_bytes());
        v.push(b'\n');
        return Ok(v);
    }
    let (k, d) = src.read_object(&f.id)?;
    if k != Kind::Blob {
        return Err(Error::Corrupt("diff: expected a blob"));
    }
    Ok(d)
}

/// git's `buffer_is_binary`: a NUL in the first 8000 bytes.
pub fn is_binary(d: &[u8]) -> bool {
    d[..d.len().min(8000)].contains(&0)
}

// ---------------------------------------------------------------------------------------------
// Rename detection
// ---------------------------------------------------------------------------------------------

const HASHBASE: u32 = 107927;

/// The span-hash multiset of a buffer: chunks end at `\n` or after 64 bytes; CR of CRLF is skipped
/// in text. Returned as (hash, byte count) sorted by hash.
pub fn span_hashes(d: &[u8], is_text: bool) -> Vec<(u32, u32)> {
    let mut m: BTreeMap<u32, u32> = BTreeMap::new();
    let (mut a1, mut a2) = (0u32, 0u32);
    let mut n = 0u32;
    let mut i = 0;
    while i < d.len() {
        let c = d[i] as u32;
        i += 1;
        if is_text && c == b'\r' as u32 && i < d.len() && d[i] == b'\n' {
            continue;
        }
        let old1 = a1;
        a1 = (a1 << 7) ^ (a2 >> 25);
        a2 = (a2 << 7) ^ (old1 >> 25);
        a1 = a1.wrapping_add(c);
        n += 1;
        if n < 64 && c != b'\n' as u32 {
            continue;
        }
        let h = a1.wrapping_add(a2.wrapping_mul(0x61)) % HASHBASE;
        *m.entry(h).or_insert(0) += n;
        n = 0;
        a1 = 0;
        a2 = 0;
    }
    if n > 0 {
        let h = a1.wrapping_add(a2.wrapping_mul(0x61)) % HASHBASE;
        *m.entry(h).or_insert(0) += n;
    }
    m.into_iter().collect()
}

fn copied(s: &[(u32, u32)], d: &[(u32, u32)]) -> u64 {
    let (mut i, mut j) = (0, 0);
    let mut sc = 0u64;
    while i < s.len() && j < d.len() {
        match s[i].0.cmp(&d[j].0) {
            core::cmp::Ordering::Less => i += 1,
            core::cmp::Ordering::Greater => j += 1,
            core::cmp::Ordering::Equal => {
                sc += s[i].1.min(d[j].1) as u64;
                i += 1;
                j += 1;
            }
        }
    }
    sc
}

struct Cand<'a> {
    spec: &'a FileSpec,
    data: Option<Vec<u8>>,
    spans: Option<Vec<(u32, u32)>>,
}

impl Cand<'_> {
    fn load(&mut self, src: &dyn ObjectSource) -> Result<()> {
        if self.data.is_none() {
            self.data = Some(content(src, self.spec)?);
        }
        Ok(())
    }
    fn size(&mut self, src: &dyn ObjectSource) -> Result<u64> {
        self.load(src)?;
        Ok(self.data.as_ref().unwrap().len() as u64)
    }
    fn spans(&mut self, src: &dyn ObjectSource) -> Result<&[(u32, u32)]> {
        self.load(src)?;
        if self.spans.is_none() {
            let d = self.data.as_ref().unwrap();
            self.spans = Some(span_hashes(d, !is_binary(d)));
        }
        Ok(self.spans.as_deref().unwrap())
    }
}

fn estimate(src: &dyn ObjectSource, a: &mut Cand<'_>, b: &mut Cand<'_>, min_score: u32) -> Result<u32> {
    if !is_reg(a.spec.mode) || !is_reg(b.spec.mode) {
        return Ok(0);
    }
    let (sa, sb) = (a.size(src)?, b.size(src)?);
    let max = sa.max(sb);
    let base = sa.min(sb);
    let delta = max - base;
    if max * ((MAX_SCORE - min_score) as u64) < delta * MAX_SCORE as u64 {
        return Ok(0);
    }
    let sc = copied(a.spans(src)?, b.spans(src)?);
    if sb == 0 {
        return Ok(0);
    }
    Ok((sc * MAX_SCORE as u64 / max) as u32)
}

fn basename(p: &[u8]) -> &[u8] {
    p.iter().rposition(|&c| c == b'/').map(|i| &p[i + 1..]).unwrap_or(p)
}

/// Rename options.
#[derive(Debug, Clone, Copy)]
pub struct RenameOptions {
    /// Minimum score (`-M<n>`), out of [`MAX_SCORE`].
    pub min_score: u32,
    /// `diff.renameLimit` (sources × destinations ≤ limit²).
    pub limit: usize,
}

impl Default for RenameOptions {
    fn default() -> Self {
        RenameOptions { min_score: DEFAULT_RENAME_SCORE, limit: 1000 }
    }
}

/// Pair deletions with creations (diffcore-rename without copies), rewriting the queue the way
/// git does: a rename takes the destination's position, its deletion disappears.
pub fn detect_renames(src: &dyn ObjectSource, pairs: Vec<Pair>, opt: &RenameOptions) -> Result<Vec<Pair>> {
    let srcs: Vec<usize> = (0..pairs.len()).filter(|&i| pairs[i].old.is_some() && pairs[i].new.is_none()).collect();
    let dsts: Vec<usize> = (0..pairs.len()).filter(|&i| pairs[i].old.is_none() && pairs[i].new.is_some()).collect();
    if srcs.is_empty() || dsts.is_empty() {
        return Ok(pairs);
    }
    let mut used = alloc::vec![false; srcs.len()];
    let mut dst_rename: Vec<Option<(usize, u32)>> = alloc::vec![None; dsts.len()];
    // 1. exact renames
    for (di, &d) in dsts.iter().enumerate() {
        let t = pairs[d].new.as_ref().unwrap();
        let mut best: Option<(usize, i32)> = None;
        for (si, &s) in srcs.iter().enumerate() {
            let o = pairs[s].old.as_ref().unwrap();
            if o.id != t.id {
                continue;
            }
            if (!is_reg(o.mode) || !is_reg(t.mode)) && o.mode != t.mode {
                continue;
            }
            if used[si] {
                continue;
            }
            let score = 1 + (basename(&o.path) == basename(&t.path)) as i32;
            if best.map_or(true, |(_, b)| score > b) {
                best = Some((si, score));
                if score == 2 {
                    break;
                }
            }
        }
        if let Some((si, _)) = best {
            used[si] = true;
            dst_rename[di] = Some((si, MAX_SCORE));
        }
    }
    let mut cands_s: Vec<Cand<'_>> = srcs.iter().map(|&s| Cand { spec: pairs[s].old.as_ref().unwrap(), data: None, spans: None }).collect();
    let mut cands_d: Vec<Cand<'_>> = dsts.iter().map(|&d| Cand { spec: pairs[d].new.as_ref().unwrap(), data: None, spans: None }).collect();
    // 2. unique basenames at the higher bar
    let min_base = opt.min_score + (MAX_SCORE - opt.min_score) / 2;
    {
        let mut sb: BTreeMap<&[u8], isize> = BTreeMap::new();
        for (si, c) in cands_s.iter().enumerate() {
            if used[si] {
                continue;
            }
            let b = basename(&c.spec.path);
            sb.entry(b).and_modify(|v| *v = -1).or_insert(si as isize);
        }
        let mut db: BTreeMap<&[u8], isize> = BTreeMap::new();
        for (di, c) in cands_d.iter().enumerate() {
            if dst_rename[di].is_some() {
                continue;
            }
            let b = basename(&c.spec.path);
            db.entry(b).and_modify(|v| *v = -1).or_insert(di as isize);
        }
        let srcs_order: Vec<(usize, Vec<u8>)> = cands_s.iter().enumerate().filter(|(si, _)| !used[*si]).map(|(si, c)| (si, basename(&c.spec.path).to_vec())).collect();
        for (si, b) in srcs_order {
            if sb.get(b.as_slice()).copied() != Some(si as isize) {
                continue;
            }
            let Some(&di) = db.get(b.as_slice()) else { continue };
            if di < 0 {
                continue;
            }
            let di = di as usize;
            if dst_rename[di].is_some() {
                continue;
            }
            let score = estimate(src, &mut cands_s[si], &mut cands_d[di], min_base)?;
            if score < min_base {
                continue;
            }
            used[si] = true;
            dst_rename[di] = Some((si, score));
        }
    }
    // 3. the similarity matrix
    let rem_s: Vec<usize> = (0..srcs.len()).filter(|&i| !used[i]).collect();
    let rem_d: Vec<usize> = (0..dsts.len()).filter(|&i| dst_rename[i].is_none()).collect();
    if !rem_s.is_empty() && !rem_d.is_empty() && (rem_s.len() as u64) * (rem_d.len() as u64) <= (opt.limit as u64) * (opt.limit as u64) {
        #[derive(Clone, Copy)]
        struct Score {
            score: u32,
            name: bool,
            dst: isize,
            src: usize,
        }
        let cmp = |a: &Score, b: &Score| -> i64 {
            if a.score == b.score {
                (b.name as i64) - (a.name as i64)
            } else {
                b.score as i64 - a.score as i64
            }
        };
        let mut mx: Vec<Score> = Vec::with_capacity(rem_d.len() * 4);
        for &di in &rem_d {
            let mut m = [Score { score: 0, name: false, dst: -1, src: 0 }; 4];
            for &si in &rem_s {
                let score = estimate(src, &mut cands_s[si], &mut cands_d[di], opt.min_score)?;
                let this = Score { score, name: basename(&cands_s[si].spec.path) == basename(&cands_d[di].spec.path), dst: di as isize, src: si };
                let mut worst = 0;
                for k in 1..4 {
                    if cmp(&m[k], &m[worst]) > 0 {
                        worst = k;
                    }
                }
                if cmp(&m[worst], &this) > 0 {
                    m[worst] = this;
                }
            }
            mx.extend_from_slice(&m);
            cands_d[di].data = None;
        }
        mx.sort_by(|a, b| cmp(a, b).cmp(&0)); // stable
        for s in &mx {
            if s.dst < 0 || s.score < opt.min_score {
                break;
            }
            let di = s.dst as usize;
            if dst_rename[di].is_some() || used[s.src] {
                continue;
            }
            used[s.src] = true;
            dst_rename[di] = Some((s.src, s.score));
        }
    }
    // Rewrite the queue.
    let mut src_pos: BTreeMap<usize, usize> = BTreeMap::new();
    for (si, &s) in srcs.iter().enumerate() {
        src_pos.insert(s, si);
    }
    let mut dst_pos: BTreeMap<usize, usize> = BTreeMap::new();
    for (di, &d) in dsts.iter().enumerate() {
        dst_pos.insert(d, di);
    }
    let mut out = Vec::with_capacity(pairs.len());
    for (i, p) in pairs.iter().enumerate() {
        if let Some(&di) = dst_pos.get(&i) {
            if let Some((si, score)) = dst_rename[di] {
                out.push(Pair { old: pairs[srcs[si]].old.clone(), new: p.new.clone(), rename_score: Some(score) });
                continue;
            }
        }
        if let Some(&si) = src_pos.get(&i) {
            if used[si] {
                continue;
            }
        }
        out.push(p.clone());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// Does git's `core.quotePath` quoting apply to this path?
pub fn needs_quote(p: &[u8]) -> bool {
    p.iter().any(|&c| c < 0x20 || c == b'"' || c == b'\\' || c >= 0x80 || c == 0x7f)
}

/// C-style quote (`"..."` with `\t \n \" \\` and octal escapes), only when needed.
pub fn quote_path(prefix: &[u8], p: &[u8]) -> Vec<u8> {
    let mut whole = prefix.to_vec();
    whole.extend_from_slice(p);
    if !needs_quote(p) {
        return whole;
    }
    let mut o = alloc::vec![b'"'];
    for &c in &whole {
        match c {
            0x07 => o.extend_from_slice(b"\\a"),
            0x08 => o.extend_from_slice(b"\\b"),
            b'\t' => o.extend_from_slice(b"\\t"),
            b'\n' => o.extend_from_slice(b"\\n"),
            0x0b => o.extend_from_slice(b"\\v"),
            0x0c => o.extend_from_slice(b"\\f"),
            b'\r' => o.extend_from_slice(b"\\r"),
            b'"' => o.extend_from_slice(b"\\\""),
            b'\\' => o.extend_from_slice(b"\\\\"),
            c if c < 0x20 || c >= 0x7f => {
                o.push(b'\\');
                o.push(b'0' + (c >> 6));
                o.push(b'0' + ((c >> 3) & 7));
                o.push(b'0' + (c & 7));
            }
            c => o.push(c),
        }
    }
    o.push(b'"');
    o
}

fn octal6(m: u32) -> Vec<u8> {
    let mut v = alloc::vec![b'0'; 6];
    let mut x = m;
    for i in (0..6).rev() {
        v[i] = b'0' + (x & 7) as u8;
        x >>= 3;
    }
    v
}

fn abbrev(src: &dyn ObjectSource, id: &ObjectId) -> Vec<u8> {
    let n = src.unique_abbrev(id, src.default_abbrev()).min(id.kind().hex_len());
    id.to_hex().as_bytes()[..n].to_vec()
}

/// Patch options.
#[derive(Debug, Clone, Copy, Default)]
pub struct PatchOptions {
    /// Line algorithm.
    pub algorithm: Algorithm,
    /// Hunk layout.
    pub emit: xdiff::EmitOptions,
    /// Indent heuristic (default on).
    pub no_indent_heuristic: bool,
}

/// Render the pairs as `git diff` does.
pub fn patch(src: &dyn ObjectSource, pairs: &[Pair], opt: &PatchOptions, out: &mut Vec<u8>) -> Result<()> {
    for p in pairs {
        match (&p.old, &p.new) {
            (Some(o), Some(n)) if p.rename_score.is_none() && (o.mode & mode::TYPE_MASK) != (n.mode & mode::TYPE_MASK) => {
                // type change: split into deletion + creation
                file_patch(src, Some(o), None, None, opt, out)?;
                file_patch(src, None, Some(n), None, opt, out)?;
            }
            _ => file_patch(src, p.old.as_ref(), p.new.as_ref(), p.rename_score, opt, out)?,
        }
    }
    Ok(())
}

fn file_patch(src: &dyn ObjectSource, old: Option<&FileSpec>, new: Option<&FileSpec>, score: Option<u32>, opt: &PatchOptions, out: &mut Vec<u8>) -> Result<()> {
    let name_a = old.or(new).unwrap().path.clone();
    let name_b = new.or(old).unwrap().path.clone();
    out.extend_from_slice(b"diff --git ");
    out.extend_from_slice(&quote_path(b"a/", &name_a));
    out.push(b' ');
    out.extend_from_slice(&quote_path(b"b/", &name_b));
    out.push(b'\n');
    match (old, new) {
        (None, Some(n)) => {
            out.extend_from_slice(b"new file mode ");
            out.extend_from_slice(&octal6(n.mode));
            out.push(b'\n');
        }
        (Some(o), None) => {
            out.extend_from_slice(b"deleted file mode ");
            out.extend_from_slice(&octal6(o.mode));
            out.push(b'\n');
        }
        (Some(o), Some(n)) if o.mode != n.mode => {
            out.extend_from_slice(b"old mode ");
            out.extend_from_slice(&octal6(o.mode));
            out.extend_from_slice(b"\nnew mode ");
            out.extend_from_slice(&octal6(n.mode));
            out.push(b'\n');
        }
        _ => {}
    }
    if let Some(s) = score {
        out.extend_from_slice(b"similarity index ");
        crate::object::push_decimal(out, (s as u64) * 100 / MAX_SCORE as u64);
        out.extend_from_slice(b"%\nrename from ");
        out.extend_from_slice(&quote_path(b"", &name_a));
        out.extend_from_slice(b"\nrename to ");
        out.extend_from_slice(&quote_path(b"", &name_b));
        out.push(b'\n');
    }
    let hk = old.or(new).unwrap().id.kind();
    let oid_a = old.map(|f| f.id).unwrap_or(hk.null());
    let oid_b = new.map(|f| f.id).unwrap_or(hk.null());
    if oid_a != oid_b {
        out.extend_from_slice(b"index ");
        out.extend_from_slice(&abbrev(src, &oid_a));
        out.extend_from_slice(b"..");
        out.extend_from_slice(&abbrev(src, &oid_b));
        if let (Some(o), Some(n)) = (old, new) {
            if o.mode == n.mode {
                out.push(b' ');
                out.extend_from_slice(&octal6(o.mode));
            }
        }
        out.push(b'\n');
    } else {
        return Ok(()); // pure rename or mode change: no body
    }
    let da = match old {
        Some(f) => content(src, f)?,
        None => Vec::new(),
    };
    let db = match new {
        Some(f) => content(src, f)?,
        None => Vec::new(),
    };
    let lbl_a = if old.is_some() { quote_path(b"a/", &name_a) } else { b"/dev/null".to_vec() };
    let lbl_b = if new.is_some() { quote_path(b"b/", &name_b) } else { b"/dev/null".to_vec() };
    if is_binary(&da) || is_binary(&db) {
        out.extend_from_slice(b"Binary files ");
        out.extend_from_slice(&lbl_a);
        out.extend_from_slice(b" and ");
        out.extend_from_slice(&lbl_b);
        out.extend_from_slice(b" differ\n");
        return Ok(());
    }
    let env = xdiff::diff(&da, &db, opt.algorithm, !opt.no_indent_heuristic);
    let script = xdiff::build_script(&env);
    if script.is_empty() {
        return Ok(());
    }
    let tab_a = if old.is_some() && name_a.contains(&b' ') { "\t" } else { "" };
    let tab_b = if new.is_some() && name_b.contains(&b' ') { "\t" } else { "" };
    out.extend_from_slice(b"--- ");
    out.extend_from_slice(&lbl_a);
    out.extend_from_slice(tab_a.as_bytes());
    out.extend_from_slice(b"\n+++ ");
    out.extend_from_slice(&lbl_b);
    out.extend_from_slice(tab_b.as_bytes());
    out.push(b'\n');
    xdiff::emit(&env, &script, &opt.emit, out);
    Ok(())
}

/// git's `pprint_rename`: `a/{old => new}/c`.
pub fn pprint_rename(a: &[u8], b: &[u8]) -> Vec<u8> {
    if needs_quote(a) || needs_quote(b) {
        let mut o = quote_path(b"", a);
        o.extend_from_slice(b" => ");
        o.extend_from_slice(&quote_path(b"", b));
        return o;
    }
    let (la, lb) = (a.len(), b.len());
    let mut pfx = 0;
    let mut i = 0;
    while i < la && i < lb && a[i] == b[i] {
        if a[i] == b'/' {
            pfx = i + 1;
        }
        i += 1;
    }
    // Common suffix, letting the scan see the prefix's trailing slash.
    let adj = if pfx > 0 { 1 } else { 0 };
    let mut sfx = 0;
    let (mut oa, mut ob) = (la as isize, lb as isize); // index of the char compared (la = NUL)
    let at = |s: &[u8], k: isize| if k as usize == s.len() { 0u8 } else { s[k as usize] };
    while (pfx as isize - adj) <= oa && (pfx as isize - adj) <= ob && at(a, oa) == at(b, ob) {
        if at(a, oa) == b'/' {
            sfx = la - oa as usize;
        }
        oa -= 1;
        ob -= 1;
    }
    let amid = la as isize - pfx as isize - sfx as isize;
    let bmid = lb as isize - pfx as isize - sfx as isize;
    let (amid, bmid) = (amid.max(0) as usize, bmid.max(0) as usize);
    let mut o = Vec::new();
    if pfx + sfx > 0 {
        o.extend_from_slice(&a[..pfx]);
        o.push(b'{');
    }
    o.extend_from_slice(&a[pfx..pfx + amid]);
    o.extend_from_slice(b" => ");
    o.extend_from_slice(&b[pfx..pfx + bmid]);
    if pfx + sfx > 0 {
        o.push(b'}');
        o.extend_from_slice(&a[la - sfx..]);
    }
    o
}

struct StatFile {
    name: Vec<u8>,
    added: u64,
    deleted: u64,
    binary: bool,
}

/// `git diff --stat` at `width` columns (git uses the terminal width, 80 when not a terminal).
pub fn stat(src: &dyn ObjectSource, pairs: &[Pair], opt: &PatchOptions, width: usize, out: &mut Vec<u8>) -> Result<()> {
    let mut files: Vec<StatFile> = Vec::new();
    for p in pairs {
        let name = match (&p.old, &p.new, p.rename_score) {
            (Some(o), Some(n), Some(_)) => pprint_rename(&o.path, &n.path),
            _ => quote_path(b"", &p.new.as_ref().or(p.old.as_ref()).unwrap().path),
        };
        let da = match &p.old {
            Some(f) => content(src, f)?,
            None => Vec::new(),
        };
        let db = match &p.new {
            Some(f) => content(src, f)?,
            None => Vec::new(),
        };
        let same = p.old.as_ref().map(|f| f.id) == p.new.as_ref().map(|f| f.id);
        if is_binary(&da) || is_binary(&db) {
            let (a, d) = if same { (0, 0) } else { (db.len() as u64, da.len() as u64) };
            files.push(StatFile { name, added: a, deleted: d, binary: true });
        } else {
            let (a, d) = if same {
                (0, 0)
            } else {
                let env = xdiff::diff(&da, &db, opt.algorithm, !opt.no_indent_heuristic);
                xdiff::counts(&env)
            };
            files.push(StatFile { name, added: a, deleted: d, binary: false });
        }
    }
    let dw = |v: u64| -> usize {
        let mut w = 1;
        let mut x = v;
        while x >= 10 {
            x /= 10;
            w += 1;
        }
        w
    };
    let mut max_change = 0u64;
    let mut max_len = 0usize;
    let mut bin_width = 0usize;
    let mut number_width = 0usize;
    for f in &files {
        let len = utf8_width(&f.name);
        if max_len < len {
            max_len = len;
        }
        if f.binary {
            let w = 14 + dw(f.added) + dw(f.deleted);
            if bin_width < w {
                bin_width = w;
            }
            number_width = 3;
            continue;
        }
        if max_change < f.added + f.deleted {
            max_change = f.added + f.deleted;
        }
    }
    let mut width = width as i64;
    number_width = number_width.max(dw(max_change));
    let nw = number_width as i64;
    if width < 16 + 6 + nw {
        width = 16 + 6 + nw;
    }
    let mut graph_width: i64 = if max_change as i64 + 4 > bin_width as i64 { max_change as i64 } else { bin_width as i64 - 4 };
    let mut name_width: i64 = max_len as i64;
    if name_width + nw + 6 + graph_width > width {
        if graph_width > width * 3 / 8 - nw - 6 {
            graph_width = width * 3 / 8 - nw - 6;
            if graph_width < 6 {
                graph_width = 6;
            }
        }
        if name_width > width - nw - 6 - graph_width {
            name_width = width - nw - 6 - graph_width;
        } else {
            graph_width = width - nw - 6 - name_width;
        }
    }
    let (mut adds, mut dels) = (0u64, 0u64);
    for f in &files {
        let mut prefix: &[u8] = b"";
        let mut name: &[u8] = &f.name;
        let mut len = name_width;
        let mut name_len = utf8_width(name) as i64;
        if name_width < name_len {
            prefix = b"...";
            len -= 3;
            if len < 0 {
                len = 0;
            }
            while name_len > len {
                let (w, adv) = utf8_char(name);
                name_len -= w as i64;
                name = &name[adv..];
            }
            if let Some(s) = name.iter().position(|&c| c == b'/') {
                name = &name[s..];
            }
        }
        let padding = (len - utf8_width(name) as i64).max(0) as usize;
        out.push(b' ');
        out.extend_from_slice(prefix);
        out.extend_from_slice(name);
        out.extend(core::iter::repeat_n(b' ', padding));
        out.extend_from_slice(b" | ");
        if f.binary {
            out.extend(core::iter::repeat_n(b' ', number_width.saturating_sub(3)));
            out.extend_from_slice(b"Bin");
            if f.added == 0 && f.deleted == 0 {
                out.push(b'\n');
                continue;
            }
            out.push(b' ');
            crate::object::push_decimal(out, f.deleted);
            out.extend_from_slice(b" -> ");
            crate::object::push_decimal(out, f.added);
            out.extend_from_slice(b" bytes\n");
            continue;
        }
        adds += f.added;
        dels += f.deleted;
        let (mut add, mut del) = (f.added as i64, f.deleted as i64);
        if graph_width <= max_change as i64 {
            let scale = |it: i64| -> i64 { if it == 0 { 0 } else { 1 + it * (graph_width - 1) / max_change as i64 } };
            let mut total = scale(add + del);
            if total < 2 && add != 0 && del != 0 {
                total = 2;
            }
            if add < del {
                add = scale(add);
                del = total - add;
            } else {
                del = scale(del);
                add = total - del;
            }
        }
        let tot = f.added + f.deleted;
        let s = alloc::format!("{:>w$}", tot, w = number_width);
        out.extend_from_slice(s.as_bytes());
        if tot != 0 {
            out.push(b' ');
        }
        out.extend(core::iter::repeat_n(b'+', add.max(0) as usize));
        out.extend(core::iter::repeat_n(b'-', del.max(0) as usize));
        out.push(b'\n');
    }
    let n = files.len();
    let mut s = String::new();
    if n == 0 {
        // git prints NOTHING for an empty `--stat` (a merge whose tree equals its first parent; flight-26 fold
        // 07e39937 on this repo): no summary line, not " 0 files changed".
    } else {
        s.push_str(&alloc::format!(" {} file{} changed", n, if n == 1 { "" } else { "s" }));
        if adds != 0 || dels == 0 {
            s.push_str(&alloc::format!(", {} insertion{}(+)", adds, if adds == 1 { "" } else { "s" }));
        }
        if dels != 0 || adds == 0 {
            s.push_str(&alloc::format!(", {} deletion{}(-)", dels, if dels == 1 { "" } else { "s" }));
        }
        s.push('\n');
    }
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

/// Display width of a UTF-8 string (East Asian wide characters count 2; invalid bytes 1).
fn utf8_width(s: &[u8]) -> usize {
    let mut w = 0;
    let mut i = 0;
    while i < s.len() {
        let (cw, adv) = utf8_char(&s[i..]);
        w += cw;
        i += adv;
    }
    w
}

fn utf8_char(s: &[u8]) -> (usize, usize) {
    let c = s[0];
    let (len, init) = match c {
        0x00..=0x7f => return (1, 1),
        0xc0..=0xdf => (2, (c & 0x1f) as u32),
        0xe0..=0xef => (3, (c & 0x0f) as u32),
        0xf0..=0xf7 => (4, (c & 0x07) as u32),
        _ => return (1, 1),
    };
    if s.len() < len || !s[1..len].iter().all(|&b| b & 0xc0 == 0x80) {
        return (1, 1);
    }
    let mut cp = init;
    for &b in &s[1..len] {
        cp = (cp << 6) | (b & 0x3f) as u32;
    }
    let wide = matches!(cp, 0x1100..=0x115f | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe4f | 0xff00..=0xff60 | 0xffe0..=0xffe6 | 0x1f300..=0x1f64f | 0x1f900..=0x1f9ff | 0x20000..=0x3fffd);
    (if wide { 2 } else { 1 }, len)
}
