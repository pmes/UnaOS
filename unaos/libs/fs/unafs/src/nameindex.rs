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

//! CHARTER: Kernel — fs-core
//!
//! NAMEINDEX (rmbp-ledger B432) — the volume-wide NAME index, in the attribute
//! catalog's own two B+trees (R79: one store).
//!
//! * The fact (ORDERED tree only — no equality key, not in the record's
//!   `entries`): [`NAME_KEY`] (`una:fsname`) = the ASCII-lower-cased name, one fact
//!   per WORD START ([`name_keys`]: the leaf, then each suffix after `_ - .
//!   space`, at most [`NAME_WORDS_MAX`]). It is an index fact only — never an
//!   inode attribute; the exact name stays in the record (the inode's `name`,
//!   the directory entry). Only this core writes it: create (`add_entry`,
//!   `create_files_batch`), `rename`, `unlink`, `rmdir` and fsck's relink, all
//!   through `index_apply` (so every catalog-generation watcher sees it).
//! * Readiness: the marker fact [`NAME_MARK_KEY`] = [`NAME_INDEX_VERSION`] on
//!   the root inode. `format` stamps it (an empty v6+ volume is complete); a
//!   volume written before this arc is unmarked until
//!   [`name_index_build_step`](UnaFS::name_index_build_step) has visited every
//!   inode and [`name_index_mark`](UnaFS::name_index_mark) ran (the kernel's
//!   login task; the host's `unafs reindex`).
//! * The query: [`find_names`](UnaFS::find_names) is ONE ordered-tree range
//!   scan over `una:fsname`, every candidate verified against the real name.

use super::*;
use crate::index::{TAG_STRING, string_ord_open};

/// The name fact's key.
pub const NAME_KEY: &str = "una:fsname";
/// The readiness marker's key (on the root inode).
pub const NAME_MARK_KEY: &str = "una:fsname-index";
/// The marker's value: the index layout version.
pub const NAME_INDEX_VERSION: i64 = 1;
/// Word starts indexed per name (the leaf counts as one).
pub const NAME_WORDS_MAX: usize = 8;
/// Index keys one [`find_names`](UnaFS::find_names) visits at most.
pub const NAME_SCAN_MAX: usize = 4096;
/// The source word a reader prints (`src=`).
pub const NAME_SRC: &str = "una-name-index";

/// A word boundary in a name.
fn is_sep(c: u8) -> bool {
    matches!(c, b'_' | b'-' | b'.' | b' ')
}

/// The index strings of `name`: the lower-cased leaf, then the suffix at each
/// word start (after `_ - . space`), deduplicated, at most [`NAME_WORDS_MAX`].
pub fn name_keys(name: &str) -> Vec<String> {
    let low = name.to_ascii_lowercase();
    let b = low.as_bytes();
    let mut out: Vec<String> = Vec::new();
    if b.is_empty() {
        return out;
    }
    out.push(low.clone());
    for i in 1..b.len() {
        if out.len() >= NAME_WORDS_MAX {
            break;
        }
        if is_sep(b[i - 1]) && !is_sep(b[i]) && low.is_char_boundary(i) {
            let s = String::from(&low[i..]);
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// How well `prefix` names `leaf`: 2 = a prefix of the leaf, 1 = a prefix of a
/// word in it, `None` = no (ASCII case-insensitive). THE match rule — the
/// kernel's launcher and Quarry share it.
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
        if is_sep(lb[i - 1]) && at(i) {
            return Some(1);
        }
    }
    None
}

/// One name hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameHit {
    /// Absolute path on the volume.
    pub path: String,
    pub dir: bool,
    /// 2 = leaf prefix, 1 = word prefix.
    pub rank: u8,
}

/// A [`find_names`](UnaFS::find_names) answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NameFind {
    /// Best first: rank, then files before directories, then shallower, then path.
    pub hits: Vec<NameHit>,
    /// Index keys visited.
    pub scanned: usize,
    /// `true` = the volume's index is ready and answered; `false` = no index
    /// (pre-v6 volume, or not yet built) — `hits` is empty.
    pub indexed: bool,
}

/// What fsck found in the name index (a READY index only; an unmarked index
/// is not a fault — it is owed a build).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NameIndexCheck {
    pub ready: bool,
    /// Name facts the volume's names call for.
    pub expected: usize,
    /// `una:fsname` keys in the ordered tree.
    pub indexed: usize,
    /// Inodes with a name fact missing.
    pub missing: Vec<u64>,
    /// Inodes holding a name fact their name does not call for (or dead).
    pub stale: Vec<u64>,
}

fn name_hash() -> u64 {
    crate::hash::hash_bytes(NAME_KEY.as_bytes())
}

fn mark_hash() -> u64 {
    crate::hash::hash_bytes(NAME_MARK_KEY.as_bytes())
}

/// A name fact or the marker: ordered tree ONLY (no equality key, not counted in the catalog record's
/// `entries`, which stays the attribute count). `index_apply` asks.
pub(super) fn is_name_fact(f: &IndexFact) -> bool {
    f.key_hash == name_hash() || f.key_hash == mark_hash()
}

/// `[lo, hi)` over every `una:fsname` ordered key whose string starts with `p`
/// (`p` already lower-cased; capped at the key's string cap — a superset).
fn prefix_range(p: &str) -> (Vec<u8>, Vec<u8>) {
    let mut lo = Vec::with_capacity(16 + p.len());
    lo.extend_from_slice(&name_hash().to_be_bytes());
    lo.push(TAG_STRING);
    lo.extend_from_slice(&string_ord_open(p.as_bytes()));
    let mut hi = lo.clone();
    // Name bytes are UTF-8 (never 0xFF) and an escaped NUL is `00 FF`: every
    // key continuing `lo` sorts below `lo ‖ FF`.
    hi.push(0xFF);
    (lo, hi)
}

impl<D: BlockDevice> UnaFS<D> {
    /// The name facts inode `id` named `name` contributes (none on a pre-v6
    /// volume: the flat catalog is not range-scannable).
    pub(super) fn name_facts(&self, id: u64, name: &str) -> Vec<IndexFact> {
        if !self.superblock.indexed() {
            return Vec::new();
        }
        name_keys(name)
            .into_iter()
            .map(|k| IndexFact::new(NAME_KEY, &AttributeValue::String(k), id))
            .collect()
    }

    fn mark_fact(&self) -> IndexFact {
        IndexFact::new(NAME_MARK_KEY, &AttributeValue::Int(NAME_INDEX_VERSION), self.superblock.root_inode)
    }

    /// Stamp the readiness marker (in the caller's transaction).
    pub(super) fn name_index_mark_inner(&mut self) -> Result<(), FileSystemError> {
        if !self.superblock.indexed() {
            return Ok(());
        }
        let m = self.mark_fact();
        self.index_apply(&[], &[m])
    }

    /// Is this volume's name index complete (the marker is present)?
    pub fn name_index_ready(&mut self) -> Result<bool, FileSystemError> {
        let Some(rec) = self.catalog_record()? else {
            return Ok(false);
        };
        let Some(k) = self.mark_fact().ord_key() else {
            return Ok(false);
        };
        let mut store = ReadStore { device: &mut self.device };
        Ok(Btree::open(rec.ord_root, LexCmp).contains(&mut store, &k)?)
    }

    /// The leaf name of a named inode (`None` for the root, a system object or
    /// an unnamed one). A name too long for the inode trailer is read from the
    /// parent's listing.
    fn leaf_of(&mut self, inode: &Inode) -> Result<Option<String>, FileSystemError> {
        if inode.parent == 0 || matches!(inode.kind, FileKind::System) {
            return Ok(None);
        }
        if let Some(n) = &inode.name {
            return Ok(Some(n.clone()));
        }
        Ok(self.ls(inode.parent)?.into_iter().find(|e| e.inode_id == inode.id).map(|e| e.name))
    }

    /// One chunk of the migration build: index the names of inode ids
    /// `from .. from + max` and commit (with autocommit on). Idempotent — a
    /// fact already present is a no-op insert, so creates and renames that ran
    /// meanwhile (they maintain their own facts) are safe. Returns
    /// `(next id, names indexed, done)`. Does NOT mark: the caller runs
    /// [`name_index_mark`](Self::name_index_mark) once `done`.
    pub fn name_index_build_step(&mut self, from: u64, max: usize) -> Result<(u64, usize, bool), FileSystemError> {
        if !self.superblock.indexed() {
            return Ok((from, 0, true));
        }
        let end = core::cmp::min(self.imap.len() as u64, from.saturating_add(max as u64));
        let mut facts: Vec<IndexFact> = Vec::new();
        let mut names = 0usize;
        let mut id = from.max(1);
        while id < end {
            if self.imap.get(id as usize).copied().unwrap_or(0) != 0 {
                let inode = match self.read_inode(id) {
                    Ok(i) => i,
                    Err(FileSystemError::NotFound) => {
                        id += 1;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                if let Some(n) = self.leaf_of(&inode)? {
                    facts.extend(self.name_facts(id, &n));
                    names += 1;
                }
            }
            id += 1;
        }
        if !facts.is_empty() {
            self.index_apply(&[], &facts)?;
            self.maybe_commit()?;
        }
        Ok((end, names, end >= self.imap.len() as u64))
    }

    /// Stamp the readiness marker (one transaction).
    pub fn name_index_mark(&mut self) -> Result<(), FileSystemError> {
        self.name_index_mark_inner()?;
        self.maybe_commit()
    }

    /// Every `una:fsname` ordered key (the reindex drop and the fsck check).
    fn name_keys_on_disk(&mut self) -> Result<Vec<Vec<u8>>, FileSystemError> {
        let Some(rec) = self.catalog_record()? else {
            return Ok(Vec::new());
        };
        let kh = name_hash();
        let mut ord_lo = kh.to_be_bytes().to_vec();
        ord_lo.push(TAG_STRING);
        let mut ord_hi = kh.to_be_bytes().to_vec();
        ord_hi.push(TAG_STRING + 1);
        let mut store = ReadStore { device: &mut self.device };
        let ord: Vec<Vec<u8>> = Btree::open(rec.ord_root, LexCmp)
            .range(&mut store, Some(&ord_lo), Some(&ord_hi), false)?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        Ok(ord)
    }

    /// The `una:fsname` keys in the ordered tree (`tests nameindex`'s `names=`; leaf reads only).
    pub fn name_index_count(&mut self) -> Result<usize, FileSystemError> {
        Ok(self.name_keys_on_disk()?.len())
    }

    /// Host `unafs reindex` / fsck repair: drop every name fact and the marker,
    /// rebuild from every inode, mark — ONE transaction. Returns the names indexed.
    pub fn reindex_names(&mut self) -> Result<usize, FileSystemError> {
        if !self.superblock.indexed() {
            return Ok(0);
        }
        let was = self.autocommit;
        self.autocommit = false;
        let r = self.reindex_names_inner();
        self.autocommit = was;
        match r {
            Ok(n) => {
                self.commit()?;
                Ok(n)
            }
            Err(e) => {
                self.txn_unwind();
                Err(e)
            }
        }
    }

    /// Drop every name fact and the marker (one transaction): the volume then
    /// reads as un-indexed until a build marks it. Returns the facts dropped.
    pub fn name_index_drop(&mut self) -> Result<usize, FileSystemError> {
        let n = self.name_index_drop_inner()?;
        self.maybe_commit()?;
        Ok(n)
    }

    fn reindex_names_inner(&mut self) -> Result<usize, FileSystemError> {
        self.name_index_drop_inner()?;
        let mut next = 1u64;
        let mut names = 0usize;
        loop {
            let (n, k, done) = self.name_index_build_step(next, 4096)?;
            names += k;
            next = n;
            if done {
                break;
            }
        }
        self.name_index_mark_inner()?;
        Ok(names)
    }

    fn name_index_drop_inner(&mut self) -> Result<usize, FileSystemError> {
        if !self.superblock.indexed() {
            return Ok(0);
        }
        let ord = self.name_keys_on_disk()?;
        let mark = self.mark_fact();
        let rec = self.catalog_record()?.ok_or(FileSystemError::CorruptVolume("catalog record invalid"))?;
        let mut ordt = Btree::open(rec.ord_root, LexCmp);
        let written = {
            let mut store = DeviceStore::new(&mut self.device, &mut self.refmap);
            for k in &ord {
                ordt.remove(&mut store, k)?;
            }
            if let Some(k) = mark.ord_key() {
                ordt.remove(&mut store, &k)?;
            }
            store.written
        };
        self.count_index_writes(written);
        let new = CatalogRecord { eq_root: rec.eq_root, ord_root: ordt.root(), entries: rec.entries };
        if new != rec {
            self.rewrite_data_inner(self.superblock.catalog_inode, &new.to_bytes())?;
        }
        Ok(ord.len())
    }

    /// fsck's name-index check: the facts every named inode calls for against
    /// the ordered tree's `una:fsname` keys. Read-only.
    pub fn name_index_check(&mut self) -> Result<NameIndexCheck, FileSystemError> {
        let mut out = NameIndexCheck::default();
        if !self.superblock.indexed() || !self.name_index_ready()? {
            return Ok(out);
        }
        out.ready = true;
        let mut want: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut owner: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
        for id in 1..self.imap.len() as u64 {
            if self.imap.get(id as usize).copied().unwrap_or(0) == 0 {
                continue;
            }
            let inode = match self.read_inode(id) {
                Ok(i) => i,
                Err(FileSystemError::NotFound) => continue,
                Err(e) => return Err(e),
            };
            if let Some(n) = self.leaf_of(&inode)? {
                for f in self.name_facts(id, &n) {
                    if let Some(k) = f.ord_key() {
                        owner.insert(k.clone(), id);
                        want.insert(k);
                    }
                }
            }
        }
        let ord = self.name_keys_on_disk()?;
        out.expected = want.len();
        out.indexed = ord.len();
        let have: BTreeSet<Vec<u8>> = ord.into_iter().collect();
        let mut missing: BTreeSet<u64> = BTreeSet::new();
        for k in want.difference(&have) {
            missing.insert(owner[k]);
        }
        let mut stale: BTreeSet<u64> = BTreeSet::new();
        for k in have.difference(&want) {
            if let Some(id) = crate::index::inode_of_key(k) {
                stale.insert(id);
            }
        }
        out.missing = missing.into_iter().collect();
        out.stale = stale.into_iter().collect();
        Ok(out)
    }

    /// QUARRY3 (B413) + LAUNCHER (B417), ONE query: every object whose name
    /// `prefix` names (a prefix of the leaf, or of a word in it; ASCII
    /// case-insensitive), best first, at most `limit`. One ordered-tree range
    /// scan (at most [`NAME_SCAN_MAX`] keys); each candidate verified against
    /// the real name; paths by parent pointers. An unready volume answers
    /// `indexed: false` and no hits (the caller names the source).
    pub fn find_names(&mut self, prefix: &str, limit: usize) -> Result<NameFind, FileSystemError> {
        let mut out = NameFind::default();
        if !self.superblock.indexed() || !self.name_index_ready()? {
            return Ok(out);
        }
        out.indexed = true;
        let p = prefix.to_ascii_lowercase();
        if p.is_empty() || limit == 0 {
            return Ok(out);
        }
        let Some(rec) = self.catalog_record()? else {
            return Ok(out);
        };
        let (lo, hi) = prefix_range(&p);
        let mut ids: Vec<u64> = Vec::new();
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        {
            let mut store = ReadStore { device: &mut self.device };
            let t = Btree::open(rec.ord_root, LexCmp);
            let mut c = t.cursor_seek(&mut store, &lo)?;
            while let Some((k, _)) = c.current() {
                if k >= hi.as_slice() || out.scanned >= NAME_SCAN_MAX {
                    break;
                }
                out.scanned += 1;
                if let Some(id) = crate::index::inode_of_key(k)
                    && seen.insert(id)
                {
                    ids.push(id);
                }
                t.cursor_next(&mut store, &mut c)?;
            }
        }
        let mut best = 0usize; // rank-2 hits so far: enough of them ends the verify early
        for id in ids {
            if best >= limit || out.hits.len() >= limit.saturating_mul(4) {
                break;
            }
            let inode = match self.read_inode(id) {
                Ok(i) => i,
                Err(FileSystemError::NotFound) => continue, // a stale fact: the index costs reads, never answers
                Err(e) => return Err(e),
            };
            let Some(leaf) = self.leaf_of(&inode)? else { continue };
            let Some(rank) = name_match(&leaf, &p) else { continue };
            let path = self.path_of(id)?;
            if path.is_empty() {
                continue;
            }
            if rank == 2 {
                best += 1;
            }
            out.hits.push(NameHit { path, dir: inode.kind == FileKind::Directory, rank });
        }
        out.hits.sort_by(|a, b| {
            b.rank
                .cmp(&a.rank)
                .then(a.dir.cmp(&b.dir))
                .then(a.path.matches('/').count().cmp(&b.path.matches('/').count()))
                .then(a.path.cmp(&b.path))
        });
        out.hits.truncate(limit);
        Ok(out)
    }
}
