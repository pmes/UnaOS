// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The `.geode` v1 codec (GEODEMETA SR148; GEODEMOD SR157). This file carries NO crate-level attribute: the host
//! crate's `lib.rs` is its root, and the kernel compiles it as a module by path (`fs/geodew.rs`).
//!
//! # The format (all integers little-endian, the §R1 codec of `docs/dev/OS/09_FILESYSTEM/unafs-records.md`)
//!
//! ```text
//! header   "GEODEAR1" (8) ‖ version u32 (= 1) ‖ entries u64
//! entry    path   String          relative, '/'-joined; no empty, "." or ".." component
//!          kind   FileKind §R4    0 File · 1 Directory (2 Symlink, 3 System: refused)
//!          ctime u64 ‖ mtime u64 ‖ atime u64      the §R7 meta trailer's three, unix seconds
//!          attrs  Map<String, AttributeValue §R5>  the inode's `attributes` ∪ its resolved
//!                                                   `large_attributes`, keys ascending
//!          data   len u64 ‖ bytes                   a File's contents; a Directory's is empty
//! trailer  "GEODEEND" (8) ‖ FNV-1a 64 (unafs::hash) of every byte before the trailer
//! ```
//!
//! Entries are in pre-order (a directory before what it holds), so an unpack creates each entry under a parent it
//! has already made. ONE encoder: [`header`], [`entry_head`] and the [`Encoder`] that hashes what it passes to a
//! [`Sink`]; [`write_archive`] is the whole-buffer form over the same calls. ONE decoder: the streaming [`Decoder`]
//! over a [`Src`] (bounded: [`PATH_MAX`], [`VALUE_MAX`], every count checked against the bytes left before any
//! allocation), the [`Checker`] rules, [`check`] (names only kept) and [`read_archive`] (the whole-buffer form).
//! A refusal is a word ([`Why`]) the kernel prints on its wire and the host crate wraps.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use unafs::codec::{Encode, Writer};
use unafs::hash::FnvHasher;
use unafs::{AttributeValue, FileKind};

/// The header magic.
pub const MAGIC: &[u8; 8] = b"GEODEAR1";
/// The trailer magic.
pub const END: &[u8; 8] = b"GEODEEND";
/// The format version this build writes and reads.
pub const VERSION: u32 = 1;
/// The header's length: magic, version, entry count.
pub const HEADER_LEN: u64 = 8 + 4 + 8;
/// The trailer's length: magic, FNV-1a 64.
pub const TRAILER_LEN: u64 = 8 + 8;
/// The fewest bytes one entry takes (empty path, no attributes, no data): path len 8 + kind 4 + times 24 + attr
/// count 8 + data len 8.
pub const MIN_ENTRY: u64 = 8 + 4 + 24 + 8 + 8;
/// One read's worth of archive bytes (the streamed decoder's buffer step; the kernel's chunk per mount hold).
pub const CHUNK: usize = 64 * 1024;
/// The longest path, or attribute key, a decode accepts.
pub const PATH_MAX: u64 = 4096;
/// The largest single attribute value a decode accepts.
pub const VALUE_MAX: u64 = 16 * 1024 * 1024;

/// Why an encode or decode refused: one word (the kernel's `refused:<why>`; the host crate's `Format` reason).
pub type Why = &'static str;

/// One archived entry, whole: the portable part of a UnaFS record.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Relative path under the packed directory, '/'-joined.
    pub path: String,
    /// `File` or `Directory`.
    pub kind: FileKind,
    pub ctime: u64,
    pub mtime: u64,
    pub atime: u64,
    /// Every attribute of the inode, small and spilled.
    pub attributes: BTreeMap<String, AttributeValue>,
    /// A file's contents (empty for a directory).
    pub data: Vec<u8>,
}

/// The parent of an archive path (`""` at the top level).
pub fn parent_of(p: &str) -> &str {
    p.rsplit_once('/').map(|(a, _)| a).unwrap_or("")
}

/// The last component of an archive path.
pub fn leaf(p: &str) -> &str {
    p.rsplit_once('/').map(|(_, l)| l).unwrap_or(p)
}

/// A path the archive may name: relative, '/'-joined, every component a real name.
pub fn path_ok(p: &str) -> bool {
    !p.is_empty() && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains('\0') && !c.contains('\\'))
}

// ---------------------------------------------------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------------------------------------------------

/// The header bytes for an archive of `entries` entries.
pub fn header(entries: u64) -> Vec<u8> {
    let mut w = Writer::new();
    w.raw(MAGIC);
    w.u32(VERSION);
    w.u64(entries);
    w.into_bytes()
}

/// One entry's head: every field up to and including the data length; the `data_len` bytes follow it.
pub fn entry_head(path: &str, kind: FileKind, ctime: u64, mtime: u64, atime: u64, attrs: &BTreeMap<String, AttributeValue>, data_len: u64) -> Vec<u8> {
    let mut w = Writer::new();
    w.str(path);
    kind.encode(&mut w);
    w.u64(ctime);
    w.u64(mtime);
    w.u64(atime);
    w.map(attrs);
    w.u64(data_len);
    w.into_bytes()
}

/// The encoded length of an attribute map (the §R1 `map`).
pub fn attrs_len(attrs: &BTreeMap<String, AttributeValue>) -> u64 {
    let mut w = Writer::new();
    w.map(attrs);
    w.into_bytes().len() as u64
}

/// The encoded length of one entry: its head (path of `path_len` bytes, an attribute map of `attrs_len` bytes) and
/// its `data_len` bytes. An archive's length is `HEADER_LEN + Σ entry_len + TRAILER_LEN`.
pub fn entry_len(path_len: usize, attrs_len: u64, data_len: u64) -> u64 {
    8 + path_len as u64 + 4 + 24 + attrs_len + 8 + data_len
}

/// Where an archive's bytes go (a file on a volume; a buffer).
pub trait Sink {
    fn put(&mut self, b: &[u8]) -> Result<(), Why>;
}

impl Sink for Vec<u8> {
    fn put(&mut self, b: &[u8]) -> Result<(), Why> {
        self.extend_from_slice(b);
        Ok(())
    }
}

/// The streaming encoder: every byte it passes to the sink is hashed and counted; [`Encoder::finish`] writes the
/// trailer. The caller puts, in order, each entry's [`entry_head`] and then exactly its data bytes.
pub struct Encoder<'a, S: Sink> {
    s: &'a mut S,
    h: FnvHasher,
    n: u64,
}

impl<'a, S: Sink> Encoder<'a, S> {
    /// Start an archive of `entries` entries: the header is written.
    pub fn new(s: &'a mut S, entries: u64) -> Result<Self, Why> {
        let mut e = Encoder { s, h: FnvHasher::new(), n: 0 };
        e.put(&header(entries))?;
        Ok(e)
    }
    /// Pass archive bytes (an entry head, or data) to the sink.
    pub fn put(&mut self, b: &[u8]) -> Result<(), Why> {
        self.h.write(b);
        self.n += b.len() as u64;
        self.s.put(b)
    }
    /// Write the trailer; the archive's whole length.
    pub fn finish(self) -> Result<u64, Why> {
        let sum = self.h.finish();
        self.s.put(END)?;
        self.s.put(&sum.to_le_bytes())?;
        Ok(self.n + TRAILER_LEN)
    }
}

/// Encode whole entries as a `.geode` v1 archive (the [`Encoder`] over a buffer).
pub fn write_archive(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    if let Ok(mut e) = Encoder::new(&mut out, entries.len() as u64) {
        for x in entries {
            let _ = e.put(&entry_head(&x.path, x.kind, x.ctime, x.mtime, x.atime, &x.attributes, x.data.len() as u64));
            let _ = e.put(&x.data);
        }
        let _ = e.finish();
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------------------------------------------------

/// Where an archive's bytes come from (a file on a volume; a buffer).
pub trait Src {
    fn len(&self) -> u64;
    fn read_at(&mut self, off: u64, n: usize) -> Result<Vec<u8>, Why>;
}

/// An archive held in RAM.
pub struct Bytes<'a>(pub &'a [u8]);

impl Src for Bytes<'_> {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&mut self, off: u64, n: usize) -> Result<Vec<u8>, Why> {
        let a = usize::try_from(off).map_err(|_| "truncated")?;
        a.checked_add(n).and_then(|b| self.0.get(a..b)).map(|s| s.to_vec()).ok_or("truncated")
    }
}

/// One entry's head: everything but its bytes, which follow (`data_len` of them).
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    pub path: String,
    pub kind: FileKind,
    pub ctime: u64,
    pub mtime: u64,
    pub atime: u64,
    pub attrs: BTreeMap<String, AttributeValue>,
    pub data_len: u64,
}

/// The streaming decoder over an archive's body (everything before the trailer): every byte read is hashed once,
/// at most [`CHUNK`] (or one field) buffered.
pub struct Decoder<'a, S: Src> {
    s: &'a mut S,
    off: u64,
    end: u64,
    buf: Vec<u8>,
    at: usize,
    h: FnvHasher,
    want: u64,
    count: u64,
}

impl<'a, S: Src> Decoder<'a, S> {
    /// Open an archive: the trailer's magic and sum, then the header (magic, version, an entry count that fits).
    pub fn open(s: &'a mut S) -> Result<Self, Why> {
        let len = s.len();
        if len < HEADER_LEN + TRAILER_LEN {
            return Err("short");
        }
        let t = s.read_at(len - TRAILER_LEN, TRAILER_LEN as usize)?;
        if t.len() != TRAILER_LEN as usize || &t[..8] != END {
            return Err("trailer");
        }
        let mut sum = [0u8; 8];
        sum.copy_from_slice(&t[8..]);
        let mut d = Decoder { s, off: 0, end: len - TRAILER_LEN, buf: Vec::new(), at: 0, h: FnvHasher::new(), want: u64::from_le_bytes(sum), count: 0 };
        if d.take(8)? != MAGIC {
            return Err("magic");
        }
        if d.u32()? != VERSION {
            return Err("version");
        }
        let count = d.u64()?;
        if count > d.remaining() / MIN_ENTRY {
            return Err("count");
        }
        d.count = count;
        Ok(d)
    }
    /// The header's entry count.
    pub fn count(&self) -> u64 {
        self.count
    }
    /// Body bytes not yet consumed.
    pub fn remaining(&self) -> u64 {
        (self.end - self.off) + (self.buf.len() - self.at) as u64
    }
    fn fill(&mut self, need: usize) -> Result<(), Why> {
        if self.buf.len() - self.at >= need {
            return Ok(());
        }
        self.buf.drain(..self.at);
        self.at = 0;
        while self.buf.len() < need {
            let left = self.end - self.off;
            if left == 0 {
                return Err("truncated");
            }
            let n = (CHUNK.max(need - self.buf.len()) as u64).min(left) as usize;
            let b = self.s.read_at(self.off, n)?;
            if b.len() != n {
                return Err("truncated");
            }
            self.h.write(&b);
            self.off += n as u64;
            self.buf.extend_from_slice(&b);
        }
        Ok(())
    }
    fn take(&mut self, n: u64) -> Result<&[u8], Why> {
        if n > self.remaining() {
            return Err("truncated");
        }
        self.fill(n as usize)?;
        let a = self.at;
        self.at += n as usize;
        Ok(&self.buf[a..a + n as usize])
    }
    fn u32(&mut self) -> Result<u32, Why> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, Why> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn string(&mut self, max: u64) -> Result<String, Why> {
        let n = self.u64()?;
        if n > max {
            return Err("too-long");
        }
        let b = self.take(n)?;
        core::str::from_utf8(b).map(String::from).map_err(|_| "utf8")
    }
    fn value(&mut self) -> Result<AttributeValue, Why> {
        Ok(match self.u32()? {
            0 => AttributeValue::Int(self.u64()? as i64),
            1 => AttributeValue::Float(f64::from_bits(self.u64()?)),
            2 => AttributeValue::String(self.string(VALUE_MAX)?),
            3 => {
                let n = self.u64()?;
                if n > VALUE_MAX {
                    return Err("too-long");
                }
                AttributeValue::Blob(self.take(n)?.to_vec())
            }
            4 => {
                let n = self.u64()?;
                if n.saturating_mul(4) > VALUE_MAX.min(self.remaining()) {
                    return Err("too-long");
                }
                let mut v = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    v.push(f32::from_bits(self.u32()?));
                }
                AttributeValue::Vector(v)
            }
            _ => return Err("format"),
        })
    }
    /// The next entry's head (its `data_len` bytes follow: [`Decoder::chunk`] or [`Decoder::skip`] them). A
    /// symlink or system record is refused (`kind`): the format does not carry them.
    pub fn head(&mut self) -> Result<Head, Why> {
        let path = self.string(PATH_MAX)?;
        let kind = match self.u32()? {
            0 => FileKind::File,
            1 => FileKind::Directory,
            2 | 3 => return Err("kind"),
            _ => return Err("format"),
        };
        let (ctime, mtime, atime) = (self.u64()?, self.u64()?, self.u64()?);
        let n = self.u64()?;
        if n.saturating_mul(8 + 12) > self.remaining() {
            return Err("count");
        }
        let mut attrs = BTreeMap::new();
        for _ in 0..n {
            let k = self.string(PATH_MAX)?;
            let v = self.value()?;
            attrs.insert(k, v);
        }
        let data_len = self.u64()?;
        if data_len > self.remaining() {
            return Err("truncated");
        }
        Ok(Head { path, kind, ctime, mtime, atime, attrs, data_len })
    }
    /// At most `max` bytes of the next `left` (a file's data, streamed).
    pub fn chunk(&mut self, left: u64, max: usize) -> Result<Vec<u8>, Why> {
        let n = left.min(max as u64);
        Ok(self.take(n)?.to_vec())
    }
    /// Read past `n` bytes (still hashed).
    pub fn skip(&mut self, mut n: u64) -> Result<(), Why> {
        while n > 0 {
            let k = n.min(CHUNK as u64);
            self.take(k)?;
            n -= k;
        }
        Ok(())
    }
    /// After the last entry: nothing may follow it, and the body's FNV-1a must be the trailer's.
    pub fn finish(self) -> Result<(), Why> {
        if self.remaining() != 0 {
            return Err("trailing");
        }
        if self.h.finish() != self.want {
            return Err("checksum");
        }
        Ok(())
    }
}

/// The archive's structural rules, entry by entry: every path's components, every parent an earlier directory
/// entry, no repeat, no data on a directory.
#[derive(Default)]
pub struct Checker {
    dirs: BTreeSet<String>,
    seen: BTreeSet<String>,
}

impl Checker {
    pub fn new() -> Self {
        Self::default()
    }
    /// Admit the next head; `true` when it is a top-level name (the ones a target must not already hold).
    pub fn admit(&mut self, h: &Head) -> Result<bool, Why> {
        if !path_ok(&h.path) {
            return Err("path");
        }
        if h.kind == FileKind::Directory && h.data_len != 0 {
            return Err("dirdata");
        }
        let parent = parent_of(&h.path);
        if !parent.is_empty() && !self.dirs.contains(parent) {
            return Err("parent");
        }
        if !self.seen.insert(h.path.clone()) {
            return Err("repeated");
        }
        if h.kind == FileKind::Directory {
            self.dirs.insert(h.path.clone());
        }
        Ok(parent.is_empty())
    }
}

/// What a checked archive holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub entries: u64,
    pub files: u32,
    pub data_bytes: u64,
    /// The top-level names, in archive order.
    pub tops: Vec<String>,
}

/// Check an archive whole without keeping its bytes (streamed; names only kept): the [`Decoder`]'s bounds, the
/// [`Checker`]'s rules, nothing after the last entry, the checksum.
pub fn check<S: Src>(src: &mut S) -> Result<Summary, Why> {
    let mut d = Decoder::open(src)?;
    let mut c = Checker::new();
    let mut sum = Summary { entries: d.count(), files: 0, data_bytes: 0, tops: Vec::new() };
    for _ in 0..sum.entries {
        let h = d.head()?;
        if c.admit(&h)? {
            sum.tops.push(h.path.clone());
        }
        if h.kind == FileKind::File {
            sum.files += 1;
            sum.data_bytes += h.data_len;
        }
        d.skip(h.data_len)?;
    }
    d.finish()?;
    Ok(sum)
}

/// Parse and check a whole archive held in RAM: the same decoder and rules as [`check`], every entry kept whole.
pub fn read_archive(bytes: &[u8]) -> Result<Vec<Entry>, Why> {
    let mut src = Bytes(bytes);
    let mut d = Decoder::open(&mut src)?;
    let mut c = Checker::new();
    let mut out = Vec::with_capacity(d.count() as usize);
    for _ in 0..d.count() {
        let h = d.head()?;
        c.admit(&h)?;
        let data = d.chunk(h.data_len, h.data_len as usize)?;
        out.push(Entry { path: h.path, kind: h.kind, ctime: h.ctime, mtime: h.mtime, atime: h.atime, attributes: h.attrs, data });
    }
    d.finish()?;
    Ok(out)
}
