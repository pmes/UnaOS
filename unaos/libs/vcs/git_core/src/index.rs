// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The index ("dircache", `gitformat-index(5)`): `DIRC`, version 2/3/4, entries with their stat
//! data, mode, id, flags (assume-valid, extended, stage, name length) and v3 extended flags
//! (skip-worktree, intent-to-add); v2/v3 names NUL-padded to 8 bytes, v4 names prefix-compressed
//! against the previous entry. Extensions are parsed where git defines them (`TREE` cache tree,
//! `REUC` resolve-undo, `EOIE`, `IEOT`), and EVERY extension — known or not — is kept as bytes and
//! written back unchanged, so `parse → serialize` is byte-identical. Split index (`link`) entries
//! live in another file and are refused by name.

use alloc::vec::Vec;

use crate::hash::{HashKind, ObjectId};
use crate::{Error, Result};

/// Flag bits.
pub mod flags {
    /// CE_VALID (assume-unchanged).
    pub const ASSUME_VALID: u16 = 0x8000;
    /// An extended-flags word follows (v3+).
    pub const EXTENDED: u16 = 0x4000;
    /// Stage mask.
    pub const STAGE_MASK: u16 = 0x3000;
    /// Name length mask.
    pub const NAME_MASK: u16 = 0x0fff;
    /// Extended: skip-worktree.
    pub const SKIP_WORKTREE: u16 = 0x4000;
    /// Extended: intent-to-add.
    pub const INTENT_TO_ADD: u16 = 0x2000;
}

/// One index entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Entry {
    /// ctime seconds.
    pub ctime_s: u32,
    /// ctime nanoseconds.
    pub ctime_ns: u32,
    /// mtime seconds.
    pub mtime_s: u32,
    /// mtime nanoseconds.
    pub mtime_ns: u32,
    /// Device.
    pub dev: u32,
    /// Inode.
    pub ino: u32,
    /// Mode (100644, 100755, 120000, 160000).
    pub mode: u32,
    /// Owner.
    pub uid: u32,
    /// Group.
    pub gid: u32,
    /// File size (truncated to 32 bits).
    pub size: u32,
    /// Blob id.
    pub id: Option<ObjectId>,
    /// The 16-bit flags word as stored (name length bits included).
    pub flags: u16,
    /// v3 extended flags.
    pub ext_flags: u16,
    /// Path.
    pub path: Vec<u8>,
}

impl Entry {
    /// Merge stage (0 = normal).
    pub fn stage(&self) -> u8 {
        ((self.flags & flags::STAGE_MASK) >> 12) as u8
    }
    /// The object id (entries always carry one; `None` only on a default-constructed entry).
    pub fn oid(&self) -> ObjectId {
        self.id.expect("index entry without id")
    }
    /// Recompute the name-length bits and the extended bit.
    pub fn fix_flags(&mut self) {
        let nl = self.path.len().min(0xfff) as u16;
        self.flags = (self.flags & !(flags::NAME_MASK | flags::EXTENDED)) | nl | if self.ext_flags != 0 { flags::EXTENDED } else { 0 };
    }
}

/// An extension block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    /// Signature.
    pub sig: [u8; 4],
    /// Payload.
    pub data: Vec<u8>,
}

/// One node of the `TREE` (cache-tree) extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheTree {
    /// Path component (empty for the root).
    pub name: Vec<u8>,
    /// Entries covered, or -1 when invalidated.
    pub entry_count: i64,
    /// Number of subtrees.
    pub subtrees: u32,
    /// The tree id (valid nodes only).
    pub id: Option<ObjectId>,
}

/// The index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// 2, 3 or 4.
    pub version: u32,
    /// Entries sorted by (path, stage).
    pub entries: Vec<Entry>,
    /// Extensions in file order.
    pub extensions: Vec<Extension>,
    /// Object format.
    pub hash: HashKind,
}

fn be32(d: &[u8], i: usize) -> u32 {
    u32::from_be_bytes(d[i..i + 4].try_into().unwrap())
}

fn read_varint(d: &[u8], i: &mut usize) -> Result<u64> {
    let mut c = *d.get(*i).ok_or(Error::Corrupt("index: truncated varint"))?;
    *i += 1;
    let mut v = (c & 127) as u64;
    while c & 128 != 0 {
        c = *d.get(*i).ok_or(Error::Corrupt("index: truncated varint"))?;
        *i += 1;
        v = v.checked_add(1).and_then(|v| v.checked_mul(128)).ok_or(Error::Corrupt("index: varint overflow"))? + (c & 127) as u64;
    }
    Ok(v)
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    let mut buf = [0u8; 16];
    let mut pos = buf.len() - 1;
    buf[pos] = (v & 127) as u8;
    v >>= 7;
    while v != 0 {
        v -= 1;
        pos -= 1;
        buf[pos] = 128 | (v & 127) as u8;
        v >>= 7;
    }
    out.extend_from_slice(&buf[pos..]);
}

impl Index {
    /// An empty v2 index.
    pub fn new(hk: HashKind) -> Self {
        Index { version: 2, entries: Vec::new(), extensions: Vec::new(), hash: hk }
    }

    /// Parse (the trailing checksum is verified unless it is all zeros, as `index.skipHash` writes).
    pub fn parse(hk: HashKind, d: &[u8]) -> Result<Self> {
        let n = hk.len();
        if d.len() < 12 + n || &d[..4] != b"DIRC" {
            return Err(Error::Corrupt("index: bad signature"));
        }
        let version = be32(d, 4);
        if !(2..=4).contains(&version) {
            return Err(Error::Unsupported("index version"));
        }
        let end = d.len() - n;
        let trailer = &d[end..];
        if trailer.iter().any(|&b| b != 0) && hk.digest(&d[..end]).as_bytes() != trailer {
            return Err(Error::Corrupt("index: checksum mismatch"));
        }
        let count = be32(d, 8) as usize;
        let mut entries = Vec::with_capacity(count.min(1 << 20));
        let mut i = 12;
        let mut prev: Vec<u8> = Vec::new();
        for _ in 0..count {
            let start = i;
            if i + 40 + n + 2 > end {
                return Err(Error::Corrupt("index: truncated entry"));
            }
            let mut e = Entry {
                ctime_s: be32(d, i),
                ctime_ns: be32(d, i + 4),
                mtime_s: be32(d, i + 8),
                mtime_ns: be32(d, i + 12),
                dev: be32(d, i + 16),
                ino: be32(d, i + 20),
                mode: be32(d, i + 24),
                uid: be32(d, i + 28),
                gid: be32(d, i + 32),
                size: be32(d, i + 36),
                id: Some(ObjectId::from_bytes(hk, &d[i + 40..i + 40 + n])),
                flags: u16::from_be_bytes([d[i + 40 + n], d[i + 41 + n]]),
                ext_flags: 0,
                path: Vec::new(),
            };
            i += 42 + n;
            if e.flags & flags::EXTENDED != 0 {
                if version < 3 {
                    return Err(Error::Corrupt("index: extended flags in a v2 index"));
                }
                if i + 2 > end {
                    return Err(Error::Corrupt("index: truncated extended flags"));
                }
                e.ext_flags = u16::from_be_bytes([d[i], d[i + 1]]);
                i += 2;
            }
            if version == 4 {
                let strip = read_varint(d, &mut i)? as usize;
                if i > end {
                    return Err(Error::Corrupt("index: v4 prefix runs into the checksum"));
                }
                if strip > prev.len() {
                    return Err(Error::Corrupt("index: v4 prefix longer than the previous name"));
                }
                let nul = d[i..end].iter().position(|&b| b == 0).ok_or(Error::Corrupt("index: unterminated name"))? + i;
                let mut p = prev[..prev.len() - strip].to_vec();
                p.extend_from_slice(&d[i..nul]);
                e.path = p;
                i = nul + 1;
            } else {
                let nl = (e.flags & flags::NAME_MASK) as usize;
                let nul = if nl < 0xfff {
                    if i + nl >= end || d[i + nl] != 0 {
                        return Err(Error::Corrupt("index: name length disagrees"));
                    }
                    i + nl
                } else {
                    d[i..end].iter().position(|&b| b == 0).ok_or(Error::Corrupt("index: unterminated name"))? + i
                };
                e.path = d[i..nul].to_vec();
                let len = (nul - start + 8) & !7;
                i = start + len;
                if i > end {
                    return Err(Error::Corrupt("index: padding beyond end"));
                }
            }
            prev = e.path.clone();
            entries.push(e);
        }
        let mut extensions = Vec::new();
        while i + 8 <= end {
            let sig: [u8; 4] = d[i..i + 4].try_into().unwrap();
            let len = be32(d, i + 4) as usize;
            if i + 8 + len > end {
                return Err(Error::Corrupt("index: extension beyond end"));
            }
            if &sig == b"link" {
                return Err(Error::Unsupported("split index (link extension)"));
            }
            if sig[0] < b'A' || sig[0] > b'Z' {
                return Err(Error::Unsupported("required index extension"));
            }
            extensions.push(Extension { sig, data: d[i + 8..i + 8 + len].to_vec() });
            i += 8 + len;
        }
        if i != end {
            return Err(Error::Corrupt("index: trailing bytes before the checksum"));
        }
        Ok(Index { version, entries, extensions, hash: hk })
    }

    /// Serialize (entries in their current order, extensions verbatim).
    pub fn serialize(&self) -> Vec<u8> {
        let n = self.hash.len();
        let mut o = Vec::with_capacity(12 + self.entries.len() * 80);
        o.extend_from_slice(b"DIRC");
        o.extend_from_slice(&self.version.to_be_bytes());
        o.extend_from_slice(&(self.entries.len() as u32).to_be_bytes());
        let mut prev: &[u8] = b"";
        for e in &self.entries {
            let start = o.len();
            for v in [e.ctime_s, e.ctime_ns, e.mtime_s, e.mtime_ns, e.dev, e.ino, e.mode, e.uid, e.gid, e.size] {
                o.extend_from_slice(&v.to_be_bytes());
            }
            o.extend_from_slice(e.oid().as_bytes());
            o.extend_from_slice(&e.flags.to_be_bytes());
            if e.flags & flags::EXTENDED != 0 {
                o.extend_from_slice(&e.ext_flags.to_be_bytes());
            }
            if self.version == 4 {
                let common = prev.iter().zip(&e.path).take_while(|(a, b)| a == b).count();
                write_varint(&mut o, (prev.len() - common) as u64);
                o.extend_from_slice(&e.path[common..]);
                o.push(0);
            } else {
                o.extend_from_slice(&e.path);
                let len = (o.len() - start + 8) & !7;
                while o.len() < start + len {
                    o.push(0);
                }
            }
            prev = &e.path;
        }
        let _ = n;
        for x in &self.extensions {
            o.extend_from_slice(&x.sig);
            o.extend_from_slice(&(x.data.len() as u32).to_be_bytes());
            o.extend_from_slice(&x.data);
        }
        let h = self.hash.digest(&o);
        o.extend_from_slice(h.as_bytes());
        o
    }

    /// Sort entries into index order (path bytes, then stage).
    pub fn sort(&mut self) {
        self.entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.stage().cmp(&b.stage())));
    }

    /// Find a stage-0 entry by path.
    pub fn find(&self, path: &[u8]) -> Option<&Entry> {
        self.entries
            .binary_search_by(|e| e.path.as_slice().cmp(path).then(e.stage().cmp(&0)))
            .ok()
            .map(|i| &self.entries[i])
    }

    /// The extension with signature `sig`.
    pub fn extension(&self, sig: &[u8; 4]) -> Option<&Extension> {
        self.extensions.iter().find(|x| &x.sig == sig)
    }

    /// Drop the extensions that describe the entries (cache tree, EOIE/IEOT offsets, untracked
    /// cache) — what must happen once entries change.
    pub fn invalidate_derived(&mut self) {
        self.extensions.retain(|x| !matches!(&x.sig, b"TREE" | b"EOIE" | b"IEOT" | b"UNTR" | b"FSMN"));
    }

    /// Parse the `TREE` extension (pre-order).
    pub fn cache_tree(&self) -> Result<Vec<CacheTree>> {
        let Some(x) = self.extension(b"TREE") else { return Ok(Vec::new()) };
        let d = &x.data;
        let mut v = Vec::new();
        let mut i = 0;
        while i < d.len() {
            let nul = d[i..].iter().position(|&b| b == 0).ok_or(Error::Corrupt("TREE: name"))? + i;
            let name = d[i..nul].to_vec();
            let sp = d[nul..].iter().position(|&b| b == b' ').ok_or(Error::Corrupt("TREE: count"))? + nul;
            let nl = d[sp..].iter().position(|&b| b == b'\n').ok_or(Error::Corrupt("TREE: subtrees"))? + sp;
            let cnt = core::str::from_utf8(&d[nul + 1..sp]).ok().and_then(|s| s.parse::<i64>().ok()).ok_or(Error::Corrupt("TREE: count"))?;
            let sub = core::str::from_utf8(&d[sp + 1..nl]).ok().and_then(|s| s.parse::<u32>().ok()).ok_or(Error::Corrupt("TREE: subtrees"))?;
            i = nl + 1;
            let id = if cnt >= 0 {
                let n = self.hash.len();
                if i + n > d.len() {
                    return Err(Error::Corrupt("TREE: id"));
                }
                let id = ObjectId::from_bytes(self.hash, &d[i..i + n]);
                i += n;
                Some(id)
            } else {
                None
            };
            v.push(CacheTree { name, entry_count: cnt, subtrees: sub, id });
        }
        Ok(v)
    }
}
