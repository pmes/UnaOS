// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core
//!
//! `kvstore_core` — UnaOS's ordered key-value store (AETHERSTORAGE, LEDGER SR94). Aether's IndexedDB, Cache API
//! index, service-worker registry and quota flags sit on it; the image's Aether links the same crate later.
//!
//! # Model
//! A [`Store`] is one database: a set of named **trees** (each an ordered map of byte keys to byte values)
//! plus a **meta** map. It lives in memory and in exactly ONE file, a sorted log:
//!
//! ```text
//! file   := MAGIC record*
//! record := len:u32le crc32:u32le payload[len]          (crc over payload)
//! payload:= op*                                         (one committed transaction, applied whole or not at all)
//! op     := 1 tree | 2 tree | 3 tree key val | 4 tree key | 5 tree lo hi | 6 tree | 7 key val | 8 key
//!           (create, drop, put, del, del-range [lo, hi), clear, meta-set, meta-del; strings/bytes are u32le len + bytes)
//! ```
//!
//! The log IS the write-ahead journal: a commit appends one record (the backing makes the append durable —
//! `DirBacking` fsyncs) and only then is the transaction reported committed. Opening replays records in order
//! and stops at the first short or checksum-failing record (a torn tail from a crash mid-append); the next
//! write compacts first, so garbage never sits between good records. When the log grows past twice the live
//! data (and [`COMPACT_FLOOR`]), [`Store::compact`] writes a snapshot — MAGIC + one record recreating every
//! tree — through [`Backing::replace`], which is atomic (write a temp file, rename).
//!
//! # Transactions
//! [`Store::begin`] opens a [`Txn`]; every mutation applies to memory AT ONCE (so reads inside the transaction
//! see its writes) and records both the redo op and an undo entry. [`Store::commit`] appends the redo ops as one
//! record; [`Store::rollback`] replays the undo entries backwards. Several transactions may be open at once as
//! long as their caller keeps their tree sets disjoint (IndexedDB's scheduler does: a readwrite transaction never
//! runs beside an overlapping one), so no reader ever sees another transaction's uncommitted data.
//!
//! # Keys
//! [`key`] is the Indexed DB 3.0 key model (§2.6: number < date < string < binary < array) with an
//! order-preserving, prefix-free byte encoding, so a tree's byte order IS the IndexedDB key order and an index
//! entry `enc(indexKey) ++ enc(primaryKey)` sorts by index key, then primary key.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

pub mod key;
#[cfg(any(feature = "std", test))]
pub mod dir;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ops::Bound;

/// The first eight bytes of every store file.
pub const MAGIC: &[u8; 8] = b"UNAKV1\0\0";
/// Below this many log bytes a store never compacts.
pub const COMPACT_FLOOR: usize = 64 * 1024;
/// Largest record a replay accepts (a corrupt length beyond this ends the replay as a torn tail).
pub const MAX_RECORD: usize = 1 << 30;

/// What can go wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The backing refused (the message is the backing's).
    Io(String),
    /// The file is not a store (bad magic).
    NotAStore,
    /// The named tree does not exist.
    NoTree(String),
    /// The transaction was already committed or rolled back.
    Finished,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Io(m) => write!(f, "I/O error: {m}"),
            Error::NotAStore => f.write_str("not a kvstore file"),
            Error::NoTree(t) => write!(f, "no tree named {t:?}"),
            Error::Finished => f.write_str("transaction already finished"),
        }
    }
}

/// The caller's file I/O: named byte files in one place.
pub trait Backing {
    /// The whole file, or `None` when it does not exist.
    fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, Error>;
    /// Appends to the file (creating it), durably: when this returns `Ok` the bytes survive a crash.
    fn append(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error>;
    /// Replaces the file's contents atomically: a crash leaves the old file or the new one, never a mix.
    fn replace(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error>;
    /// Removes the file (absent is not an error).
    fn remove(&mut self, name: &str) -> Result<(), Error>;
    /// The names of the files present.
    fn list(&mut self) -> Result<Vec<String>, Error>;
}

/// An in-memory [`Backing`] (tests; the image before it has a volume to write to).
#[derive(Debug, Default, Clone)]
pub struct MemBacking {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Backing for MemBacking {
    fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.files.get(name).cloned())
    }
    fn append(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        self.files.entry(name.to_string()).or_default().extend_from_slice(bytes);
        Ok(())
    }
    fn replace(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        self.files.insert(name.to_string(), bytes.to_vec());
        Ok(())
    }
    fn remove(&mut self, name: &str) -> Result<(), Error> {
        self.files.remove(name);
        Ok(())
    }
    fn list(&mut self) -> Result<Vec<String>, Error> {
        Ok(self.files.keys().cloned().collect())
    }
}

// ------------------------------------------------------------------------------------------------ crc32

/// CRC-32 (IEEE 802.3, reflected, polynomial 0xEDB88320) — the record checksum.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c: u32 = !0;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

// ------------------------------------------------------------------------------------------------ ops

/// One redo operation (the log's vocabulary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    CreateTree(String),
    DropTree(String),
    Put(String, Vec<u8>, Vec<u8>),
    Del(String, Vec<u8>),
    /// Removes every key in `[lo, hi)`; an empty `hi` means "to the end".
    DelRange(String, Vec<u8>, Vec<u8>),
    Clear(String),
    MetaSet(Vec<u8>, Vec<u8>),
    MetaDel(Vec<u8>),
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

fn encode_ops(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        match op {
            Op::CreateTree(t) => {
                out.push(1);
                put_bytes(&mut out, t.as_bytes());
            }
            Op::DropTree(t) => {
                out.push(2);
                put_bytes(&mut out, t.as_bytes());
            }
            Op::Put(t, k, v) => {
                out.push(3);
                put_bytes(&mut out, t.as_bytes());
                put_bytes(&mut out, k);
                put_bytes(&mut out, v);
            }
            Op::Del(t, k) => {
                out.push(4);
                put_bytes(&mut out, t.as_bytes());
                put_bytes(&mut out, k);
            }
            Op::DelRange(t, lo, hi) => {
                out.push(5);
                put_bytes(&mut out, t.as_bytes());
                put_bytes(&mut out, lo);
                put_bytes(&mut out, hi);
            }
            Op::Clear(t) => {
                out.push(6);
                put_bytes(&mut out, t.as_bytes());
            }
            Op::MetaSet(k, v) => {
                out.push(7);
                put_bytes(&mut out, k);
                put_bytes(&mut out, v);
            }
            Op::MetaDel(k) => {
                out.push(8);
                put_bytes(&mut out, k);
            }
        }
    }
    out
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let l = self.b.get(self.i..self.i + 4)?;
        let n = u32::from_le_bytes([l[0], l[1], l[2], l[3]]) as usize;
        self.i += 4;
        let v = self.b.get(self.i..self.i.checked_add(n)?)?.to_vec();
        self.i += n;
        Some(v)
    }
    fn string(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?).ok()
    }
}

fn decode_ops(payload: &[u8]) -> Option<Vec<Op>> {
    let mut r = Reader { b: payload, i: 0 };
    let mut ops = Vec::new();
    while r.i < payload.len() {
        let op = match r.u8()? {
            1 => Op::CreateTree(r.string()?),
            2 => Op::DropTree(r.string()?),
            3 => Op::Put(r.string()?, r.bytes()?, r.bytes()?),
            4 => Op::Del(r.string()?, r.bytes()?),
            5 => Op::DelRange(r.string()?, r.bytes()?, r.bytes()?),
            6 => Op::Clear(r.string()?),
            7 => Op::MetaSet(r.bytes()?, r.bytes()?),
            8 => Op::MetaDel(r.bytes()?),
            _ => return None,
        };
        ops.push(op);
    }
    Some(ops)
}

/// Frames a payload as one record.
fn record(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Splits a store file into its committed transactions. Answers the op lists and the byte length of the
/// valid prefix (shorter than the file when the tail is torn).
pub fn replay(file: &[u8]) -> Result<(Vec<Vec<Op>>, usize), Error> {
    if file.len() < MAGIC.len() || &file[..MAGIC.len()] != MAGIC {
        return Err(Error::NotAStore);
    }
    let mut i = MAGIC.len();
    let mut out = Vec::new();
    while i + 8 <= file.len() {
        let n = u32::from_le_bytes([file[i], file[i + 1], file[i + 2], file[i + 3]]) as usize;
        let crc = u32::from_le_bytes([file[i + 4], file[i + 5], file[i + 6], file[i + 7]]);
        if n > MAX_RECORD || i + 8 + n > file.len() {
            break;
        }
        let payload = &file[i + 8..i + 8 + n];
        if crc32(payload) != crc {
            break;
        }
        let Some(ops) = decode_ops(payload) else { break };
        out.push(ops);
        i += 8 + n;
    }
    Ok((out, i))
}

// ------------------------------------------------------------------------------------------------ store

type Tree = BTreeMap<Vec<u8>, Vec<u8>>;

/// How to put back what one mutation changed.
#[derive(Debug, Clone)]
enum Undo {
    /// The tree did not exist: remove it.
    RemoveTree(String),
    /// The tree existed with these contents: restore them.
    RestoreTree(String, Tree),
    /// The key held this value (`None` = was absent).
    Key(String, Vec<u8>, Option<Vec<u8>>),
    /// These keys were removed from the tree.
    Keys(String, Vec<(Vec<u8>, Vec<u8>)>),
    Meta(Vec<u8>, Option<Vec<u8>>),
}

/// An open transaction: the redo ops for the log and the undo entries for a rollback.
#[derive(Debug, Default)]
pub struct Txn {
    ops: Vec<Op>,
    undo: Vec<Undo>,
    done: bool,
}

impl Txn {
    /// Whether the transaction changed anything.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// One database: its trees and meta in memory, its log through `B`.
pub struct Store<B: Backing> {
    backing: B,
    name: String,
    trees: BTreeMap<String, Tree>,
    meta: Tree,
    /// Bytes in the log file (valid prefix).
    log_bytes: usize,
    /// The file has a torn tail past `log_bytes`: compact before the next append.
    torn: bool,
}

fn tree_bytes(t: &Tree) -> usize {
    t.iter().map(|(k, v)| k.len() + v.len() + 9).sum()
}

impl<B: Backing> Store<B> {
    /// Opens (or creates empty) the store `name` on `backing`, replaying its log.
    pub fn open(mut backing: B, name: &str) -> Result<Self, Error> {
        let file = backing.read(name)?;
        let mut s = Store { backing, name: name.to_string(), trees: BTreeMap::new(), meta: Tree::new(), log_bytes: 0, torn: false };
        match file {
            None => {}
            Some(f) if f.is_empty() => {}
            Some(f) => {
                let (txns, valid) = replay(&f)?;
                for ops in &txns {
                    for op in ops {
                        s.apply(op, None);
                    }
                }
                s.log_bytes = valid;
                s.torn = valid < f.len();
            }
        }
        Ok(s)
    }

    /// Deletes the store `name` from `backing`.
    pub fn destroy(backing: &mut B, name: &str) -> Result<(), Error> {
        backing.remove(name)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn backing(&mut self) -> &mut B {
        &mut self.backing
    }

    /// Consumes the store, answering its backing.
    pub fn into_backing(self) -> B {
        self.backing
    }

    /// Bytes of the log file (what a quota estimate counts).
    pub fn log_bytes(&self) -> usize {
        self.log_bytes
    }

    /// Bytes of live data (keys + values + framing).
    pub fn live_bytes(&self) -> usize {
        self.trees.iter().map(|(n, t)| n.len() + 5 + tree_bytes(t)).sum::<usize>() + tree_bytes(&self.meta)
    }

    // ---- reads

    pub fn has_tree(&self, tree: &str) -> bool {
        self.trees.contains_key(tree)
    }

    pub fn tree_names(&self) -> Vec<String> {
        self.trees.keys().cloned().collect()
    }

    pub fn get(&self, tree: &str, key: &[u8]) -> Option<&[u8]> {
        self.trees.get(tree)?.get(key).map(Vec::as_slice)
    }

    /// The entries of `tree` within the bounds, in key order (`rev` = descending).
    pub fn range<'a>(&'a self, tree: &str, lo: Bound<&[u8]>, hi: Bound<&[u8]>, rev: bool) -> Vec<(&'a [u8], &'a [u8])> {
        let Some(t) = self.trees.get(tree) else { return Vec::new() };
        if let (Bound::Included(a) | Bound::Excluded(a), Bound::Included(b) | Bound::Excluded(b)) = (lo, hi) {
            if a > b || (a == b && !(matches!(lo, Bound::Included(_)) && matches!(hi, Bound::Included(_)))) {
                return Vec::new();
            }
        }
        let it = t.range::<[u8], _>((lo, hi)).map(|(k, v)| (k.as_slice(), v.as_slice()));
        if rev { it.rev().collect() } else { it.collect() }
    }

    /// The first entry of `tree` within the bounds (`rev` = the last).
    pub fn first<'a>(&'a self, tree: &str, lo: Bound<&[u8]>, hi: Bound<&[u8]>, rev: bool) -> Option<(&'a [u8], &'a [u8])> {
        let t = self.trees.get(tree)?;
        if let (Bound::Included(a) | Bound::Excluded(a), Bound::Included(b) | Bound::Excluded(b)) = (lo, hi) {
            if a > b || (a == b && !(matches!(lo, Bound::Included(_)) && matches!(hi, Bound::Included(_)))) {
                return None;
            }
        }
        let mut it = t.range::<[u8], _>((lo, hi));
        let e = if rev { it.next_back() } else { it.next() };
        e.map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    pub fn len(&self, tree: &str) -> usize {
        self.trees.get(tree).map(BTreeMap::len).unwrap_or(0)
    }

    pub fn meta(&self, key: &[u8]) -> Option<&[u8]> {
        self.meta.get(key).map(Vec::as_slice)
    }

    // ---- transactions

    pub fn begin(&self) -> Txn {
        Txn::default()
    }

    fn mutate(&mut self, txn: &mut Txn, op: Op) -> Result<(), Error> {
        if txn.done {
            return Err(Error::Finished);
        }
        match &op {
            Op::Put(t, ..) | Op::Del(t, _) | Op::DelRange(t, ..) | Op::Clear(t) | Op::DropTree(t) if !self.trees.contains_key(t) => {
                return Err(Error::NoTree(t.clone()));
            }
            _ => {}
        }
        self.apply(&op, Some(&mut txn.undo));
        txn.ops.push(op);
        Ok(())
    }

    pub fn create_tree(&mut self, txn: &mut Txn, tree: &str) -> Result<(), Error> {
        if self.trees.contains_key(tree) {
            return Ok(());
        }
        self.mutate(txn, Op::CreateTree(tree.to_string()))
    }
    pub fn drop_tree(&mut self, txn: &mut Txn, tree: &str) -> Result<(), Error> {
        self.mutate(txn, Op::DropTree(tree.to_string()))
    }
    /// Renames a tree (as drop + create + puts, so the log stays in the base vocabulary).
    pub fn rename_tree(&mut self, txn: &mut Txn, from: &str, to: &str) -> Result<(), Error> {
        let entries: Vec<(Vec<u8>, Vec<u8>)> =
            self.trees.get(from).ok_or_else(|| Error::NoTree(from.to_string()))?.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        self.create_tree(txn, to)?;
        for (k, v) in entries {
            self.put(txn, to, &k, &v)?;
        }
        self.drop_tree(txn, from)
    }
    pub fn put(&mut self, txn: &mut Txn, tree: &str, key: &[u8], val: &[u8]) -> Result<(), Error> {
        self.mutate(txn, Op::Put(tree.to_string(), key.to_vec(), val.to_vec()))
    }
    pub fn del(&mut self, txn: &mut Txn, tree: &str, key: &[u8]) -> Result<(), Error> {
        self.mutate(txn, Op::Del(tree.to_string(), key.to_vec()))
    }
    /// Removes `[lo, hi)` (`hi` empty = to the end).
    pub fn del_range(&mut self, txn: &mut Txn, tree: &str, lo: &[u8], hi: &[u8]) -> Result<(), Error> {
        self.mutate(txn, Op::DelRange(tree.to_string(), lo.to_vec(), hi.to_vec()))
    }
    pub fn clear(&mut self, txn: &mut Txn, tree: &str) -> Result<(), Error> {
        self.mutate(txn, Op::Clear(tree.to_string()))
    }
    pub fn set_meta(&mut self, txn: &mut Txn, key: &[u8], val: &[u8]) -> Result<(), Error> {
        self.mutate(txn, Op::MetaSet(key.to_vec(), val.to_vec()))
    }
    pub fn del_meta(&mut self, txn: &mut Txn, key: &[u8]) -> Result<(), Error> {
        self.mutate(txn, Op::MetaDel(key.to_vec()))
    }

    /// Makes the transaction durable: one record appended (compacting first when the log has a torn tail
    /// or has outgrown its live data). On an I/O error the transaction is rolled back and the error answered.
    pub fn commit(&mut self, mut txn: Txn) -> Result<(), Error> {
        if txn.done {
            return Err(Error::Finished);
        }
        txn.done = true;
        if txn.ops.is_empty() {
            return Ok(());
        }
        if self.torn || self.log_bytes == 0 {
            // A fresh file, or one whose torn tail must not sit before the new record: the snapshot already
            // contains this transaction's writes (they are in memory).
            if let Err(e) = self.compact() {
                txn.done = false;
                self.rollback(txn);
                return Err(e);
            }
            return Ok(());
        }
        let rec = record(&encode_ops(&txn.ops));
        if let Err(e) = self.backing.append(&self.name, &rec) {
            txn.done = false;
            self.rollback(txn);
            return Err(e);
        }
        self.log_bytes += rec.len();
        let live = self.live_bytes();
        if self.log_bytes > COMPACT_FLOOR && self.log_bytes > 2 * live {
            let _ = self.compact();
        }
        Ok(())
    }

    /// Puts back everything the transaction changed, newest first.
    pub fn rollback(&mut self, mut txn: Txn) {
        if txn.done {
            return;
        }
        txn.done = true;
        while let Some(u) = txn.undo.pop() {
            match u {
                Undo::RemoveTree(t) => {
                    self.trees.remove(&t);
                }
                Undo::RestoreTree(t, tree) => {
                    self.trees.insert(t, tree);
                }
                Undo::Key(t, k, old) => {
                    if let Some(tr) = self.trees.get_mut(&t) {
                        match old {
                            Some(v) => {
                                tr.insert(k, v);
                            }
                            None => {
                                tr.remove(&k);
                            }
                        }
                    }
                }
                Undo::Keys(t, kvs) => {
                    if let Some(tr) = self.trees.get_mut(&t) {
                        tr.extend(kvs);
                    }
                }
                Undo::Meta(k, old) => match old {
                    Some(v) => {
                        self.meta.insert(k, v);
                    }
                    None => {
                        self.meta.remove(&k);
                    }
                },
            }
        }
    }

    /// Rewrites the file as one snapshot record (atomically, through [`Backing::replace`]).
    pub fn compact(&mut self) -> Result<(), Error> {
        let mut ops = Vec::new();
        for (name, t) in &self.trees {
            ops.push(Op::CreateTree(name.clone()));
            for (k, v) in t {
                ops.push(Op::Put(name.clone(), k.clone(), v.clone()));
            }
        }
        for (k, v) in &self.meta {
            ops.push(Op::MetaSet(k.clone(), v.clone()));
        }
        let mut file = MAGIC.to_vec();
        if !ops.is_empty() {
            file.extend_from_slice(&record(&encode_ops(&ops)));
        }
        self.backing.replace(&self.name, &file)?;
        self.log_bytes = file.len();
        self.torn = false;
        Ok(())
    }

    /// Applies one op to memory, recording its undo when `undo` is given.
    fn apply(&mut self, op: &Op, undo: Option<&mut Vec<Undo>>) {
        let u = match op {
            Op::CreateTree(t) => {
                if self.trees.contains_key(t) {
                    None
                } else {
                    self.trees.insert(t.clone(), Tree::new());
                    Some(Undo::RemoveTree(t.clone()))
                }
            }
            Op::DropTree(t) => self.trees.remove(t).map(|old| Undo::RestoreTree(t.clone(), old)),
            Op::Put(t, k, v) => {
                let tr = self.trees.entry(t.clone()).or_default();
                let old = tr.insert(k.clone(), v.clone());
                Some(Undo::Key(t.clone(), k.clone(), old))
            }
            Op::Del(t, k) => {
                let tr = self.trees.entry(t.clone()).or_default();
                tr.remove(k).map(|old| Undo::Key(t.clone(), k.clone(), Some(old)))
            }
            Op::DelRange(t, lo, hi) => {
                let tr = self.trees.entry(t.clone()).or_default();
                let keys: Vec<Vec<u8>> = if hi.is_empty() {
                    tr.range::<[u8], _>((Bound::Included(lo.as_slice()), Bound::Unbounded)).map(|(k, _)| k.clone()).collect()
                } else if lo.as_slice() < hi.as_slice() {
                    tr.range::<[u8], _>((Bound::Included(lo.as_slice()), Bound::Excluded(hi.as_slice()))).map(|(k, _)| k.clone()).collect()
                } else {
                    Vec::new()
                };
                let mut removed = Vec::with_capacity(keys.len());
                for k in keys {
                    if let Some(v) = tr.remove(&k) {
                        removed.push((k, v));
                    }
                }
                (!removed.is_empty()).then(|| Undo::Keys(t.clone(), removed))
            }
            Op::Clear(t) => {
                let tr = self.trees.entry(t.clone()).or_default();
                let old = core::mem::take(tr);
                (!old.is_empty()).then(|| Undo::Keys(t.clone(), old.into_iter().collect()))
            }
            Op::MetaSet(k, v) => {
                let old = self.meta.insert(k.clone(), v.clone());
                Some(Undo::Meta(k.clone(), old))
            }
            Op::MetaDel(k) => self.meta.remove(k).map(|old| Undo::Meta(k.clone(), Some(old))),
        };
        if let (Some(list), Some(u)) = (undo, u) {
            list.push(u);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn b(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    #[test]
    fn crc32_kat() {
        // The check value of CRC-32/ISO-HDLC.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn commit_replays_and_rollback_restores() {
        let mut s = Store::open(MemBacking::default(), "db").unwrap();
        let mut t = s.begin();
        s.create_tree(&mut t, "a").unwrap();
        s.put(&mut t, "a", b"k1", b"v1").unwrap();
        s.put(&mut t, "a", b"k2", b"v2").unwrap();
        s.set_meta(&mut t, b"version", b"1").unwrap();
        s.commit(t).unwrap();

        let mut t = s.begin();
        s.put(&mut t, "a", b"k1", b"changed").unwrap();
        s.del(&mut t, "a", b"k2").unwrap();
        s.create_tree(&mut t, "b").unwrap();
        assert_eq!(s.get("a", b"k1"), Some(&b"changed"[..]));
        s.rollback(t);
        assert_eq!(s.get("a", b"k1"), Some(&b"v1"[..]));
        assert_eq!(s.get("a", b"k2"), Some(&b"v2"[..]));
        assert!(!s.has_tree("b"));

        let mut t = s.begin();
        s.del_range(&mut t, "a", b"k1", b"k2").unwrap();
        s.commit(t).unwrap();
        let back = s.into_backing();
        let s2 = Store::open(back, "db").unwrap();
        assert_eq!(s2.get("a", b"k1"), None);
        assert_eq!(s2.get("a", b"k2"), Some(&b"v2"[..]));
        assert_eq!(s2.meta(b"version"), Some(&b"1"[..]));
    }

    #[test]
    fn torn_tail_is_dropped_then_compacted_away() {
        let mut s = Store::open(MemBacking::default(), "db").unwrap();
        let mut t = s.begin();
        s.create_tree(&mut t, "a").unwrap();
        s.put(&mut t, "a", b"x", b"1").unwrap();
        s.commit(t).unwrap();
        let mut t = s.begin();
        s.put(&mut t, "a", b"y", b"2").unwrap();
        s.commit(t).unwrap();
        let mut back = s.into_backing();
        // A crash mid-append: half of a third record.
        let rec = record(&encode_ops(&[Op::Put(String::from("a"), b("z"), b("3"))]));
        back.append("db", &rec[..rec.len() / 2]).unwrap();
        let mut s = Store::open(back, "db").unwrap();
        assert_eq!(s.get("a", b"y"), Some(&b"2"[..]));
        assert_eq!(s.get("a", b"z"), None);
        let mut t = s.begin();
        s.put(&mut t, "a", b"w", b"4").unwrap();
        s.commit(t).unwrap();
        let mut back = s.into_backing();
        let file = back.read("db").unwrap().unwrap();
        let (_, valid) = replay(&file).unwrap();
        assert_eq!(valid, file.len(), "no garbage left between records");
        let s = Store::open(back, "db").unwrap();
        assert_eq!(s.range("a", Bound::Unbounded, Bound::Unbounded, false).len(), 3);
    }

    #[test]
    fn ranges_are_ordered_and_bounded() {
        let mut s = Store::open(MemBacking::default(), "db").unwrap();
        let mut t = s.begin();
        s.create_tree(&mut t, "t").unwrap();
        for k in ["a", "b", "c", "d"] {
            s.put(&mut t, "t", k.as_bytes(), b"").unwrap();
        }
        s.commit(t).unwrap();
        let keys = |v: Vec<(&[u8], &[u8])>| v.into_iter().map(|(k, _)| k.to_vec()).collect::<Vec<_>>();
        assert_eq!(keys(s.range("t", Bound::Excluded(b"a"), Bound::Included(b"c"), false)), vec![b("b"), b("c")]);
        assert_eq!(keys(s.range("t", Bound::Unbounded, Bound::Excluded(b"c"), true)), vec![b("b"), b("a")]);
        assert!(s.range("t", Bound::Excluded(b"c"), Bound::Excluded(b"c"), false).is_empty());
        assert!(s.range("t", Bound::Included(b"d"), Bound::Included(b"a"), false).is_empty());
    }

    #[test]
    fn dir_backing_over_a_temp_dir() {
        let dir = std::env::temp_dir().join(alloc::format!("kvstore_core_kat_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut s = Store::open(dir::DirBacking::new(&dir).unwrap(), "one.kv").unwrap();
            let mut t = s.begin();
            s.create_tree(&mut t, "s").unwrap();
            for i in 0u32..2000 {
                s.put(&mut t, "s", &i.to_be_bytes(), &[7u8; 40]).unwrap();
            }
            s.commit(t).unwrap();
            for i in 0u32..2000 {
                let mut t = s.begin();
                s.put(&mut t, "s", &i.to_be_bytes(), &[9u8; 40]).unwrap();
                s.commit(t).unwrap();
            }
            assert!(s.log_bytes() < 4 * s.live_bytes() + COMPACT_FLOOR, "compaction bounds the log");
        }
        let s = Store::open(dir::DirBacking::new(&dir).unwrap(), "one.kv").unwrap();
        assert_eq!(s.len("s"), 2000);
        assert_eq!(s.get("s", &1999u32.to_be_bytes()), Some(&[9u8; 40][..]));
        let mut d = dir::DirBacking::new(&dir).unwrap();
        assert_eq!(d.list().unwrap(), vec![String::from("one.kv")]);
        Store::<dir::DirBacking>::destroy(&mut d, "one.kv").unwrap();
        assert!(d.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
