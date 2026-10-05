// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The object model (`gitformat-*`, `git-cat-file(1)`): four kinds, each stored as
//! `"<kind> <len>\0" + payload` and named by the digest of exactly those bytes.
//!
//! Round-trip contract: `parse(x).serialize() == x` for every object git writes. Commits and tags
//! are kept as an ORDERED header list plus the message (so `gpgsig`, `mergetag`, `encoding` and any
//! header this crate has never heard of survive untouched); trees as their entry list.

use alloc::vec::Vec;

use crate::hash::{HashKind, ObjectId};
use crate::{Error, Result};

/// The four object kinds, numbered as the pack format numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// A commit.
    Commit = 1,
    /// A tree.
    Tree = 2,
    /// A blob.
    Blob = 3,
    /// An annotated tag.
    Tag = 4,
}

impl Kind {
    /// The header name.
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Commit => "commit",
            Kind::Tree => "tree",
            Kind::Blob => "blob",
            Kind::Tag => "tag",
        }
    }
    /// Parse a header name.
    pub fn from_name(s: &[u8]) -> Option<Self> {
        match s {
            b"commit" => Some(Kind::Commit),
            b"tree" => Some(Kind::Tree),
            b"blob" => Some(Kind::Blob),
            b"tag" => Some(Kind::Tag),
            _ => None,
        }
    }
    /// From the 3-bit pack type (1..=4).
    pub fn from_pack(t: u8) -> Option<Self> {
        match t {
            1 => Some(Kind::Commit),
            2 => Some(Kind::Tree),
            3 => Some(Kind::Blob),
            4 => Some(Kind::Tag),
            _ => None,
        }
    }
}

/// `"<kind> <len>\0"`.
pub fn header(kind: Kind, len: usize) -> Vec<u8> {
    let mut h = Vec::with_capacity(24);
    h.extend_from_slice(kind.name().as_bytes());
    h.push(b' ');
    push_decimal(&mut h, len as u64);
    h.push(0);
    h
}

/// The id of an object of `kind` with `payload`.
pub fn hash_object(hk: HashKind, kind: Kind, payload: &[u8]) -> ObjectId {
    let mut h = hk.hasher();
    h.update(&header(kind, payload.len()));
    h.update(payload);
    h.finish()
}

/// Parse `"<kind> <len>\0"` at the start of `raw`: (kind, payload length, header length).
pub fn parse_header(raw: &[u8]) -> Result<(Kind, usize, usize)> {
    let sp = raw.iter().position(|&b| b == b' ').ok_or(Error::Corrupt("object header: no space"))?;
    let kind = Kind::from_name(&raw[..sp]).ok_or(Error::Corrupt("object header: unknown kind"))?;
    let nul = raw[sp..].iter().position(|&b| b == 0).ok_or(Error::Corrupt("object header: no NUL"))? + sp;
    let digits = &raw[sp + 1..nul];
    if digits.is_empty() || digits.len() > 19 || (digits.len() > 1 && digits[0] == b'0') {
        return Err(Error::Corrupt("object header: bad length"));
    }
    let mut len: u64 = 0;
    for &d in digits {
        if !d.is_ascii_digit() {
            return Err(Error::Corrupt("object header: bad length"));
        }
        len = len * 10 + (d - b'0') as u64;
    }
    Ok((kind, len as usize, nul + 1))
}

pub(crate) fn push_decimal(out: &mut Vec<u8>, mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[i..]);
}

// ---------------------------------------------------------------------------------------------
// Trees
// ---------------------------------------------------------------------------------------------

/// Tree entry modes git writes.
pub mod mode {
    /// A subdirectory.
    pub const TREE: u32 = 0o040000;
    /// A regular file.
    pub const BLOB: u32 = 0o100644;
    /// An executable file.
    pub const BLOB_EXEC: u32 = 0o100755;
    /// A symbolic link (blob holds the target).
    pub const LINK: u32 = 0o120000;
    /// A submodule commit.
    pub const COMMIT: u32 = 0o160000;
    /// The object-type bits.
    pub const TYPE_MASK: u32 = 0o170000;
}

/// One tree entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The mode (octal on disk, no leading zero for trees: `40000`).
    pub mode: u32,
    /// The entry name (no `/`).
    pub name: Vec<u8>,
    /// The referenced object.
    pub id: ObjectId,
}

impl TreeEntry {
    /// Is this a subtree?
    pub fn is_tree(&self) -> bool {
        self.mode & mode::TYPE_MASK == mode::TREE
    }
    /// Is this a submodule (gitlink)?
    pub fn is_gitlink(&self) -> bool {
        self.mode & mode::TYPE_MASK == mode::COMMIT
    }
    /// The kind of object it references.
    pub fn kind(&self) -> Kind {
        if self.is_tree() {
            Kind::Tree
        } else if self.is_gitlink() {
            Kind::Commit
        } else {
            Kind::Blob
        }
    }
}

/// git's tree order (`base_name_compare`): bytewise, with a tree's name compared as if it ended
/// in `/`.
pub fn tree_order(a_name: &[u8], a_tree: bool, b_name: &[u8], b_tree: bool) -> core::cmp::Ordering {
    let n = a_name.len().min(b_name.len());
    match a_name[..n].cmp(&b_name[..n]) {
        core::cmp::Ordering::Equal => {}
        o => return o,
    }
    let ca = a_name.get(n).copied().unwrap_or(if a_tree { b'/' } else { 0 });
    let cb = b_name.get(n).copied().unwrap_or(if b_tree { b'/' } else { 0 });
    ca.cmp(&cb)
}

/// A tree: its entries in stored order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    /// Entries.
    pub entries: Vec<TreeEntry>,
}

impl Tree {
    /// Parse a tree payload whose ids are `hk`.
    pub fn parse(hk: HashKind, data: &[u8]) -> Result<Self> {
        let mut entries = Vec::new();
        let mut i = 0;
        let n = hk.len();
        while i < data.len() {
            let sp = data[i..].iter().position(|&b| b == b' ').ok_or(Error::Corrupt("tree: no space"))? + i;
            let mut m: u32 = 0;
            if sp == i || sp - i > 7 {
                return Err(Error::Corrupt("tree: bad mode"));
            }
            for &d in &data[i..sp] {
                if !(b'0'..=b'7').contains(&d) {
                    return Err(Error::Corrupt("tree: bad mode"));
                }
                m = m * 8 + (d - b'0') as u32;
            }
            let nul = data[sp..].iter().position(|&b| b == 0).ok_or(Error::Corrupt("tree: no NUL"))? + sp;
            if nul + 1 + n > data.len() {
                return Err(Error::Corrupt("tree: truncated id"));
            }
            let name = data[sp + 1..nul].to_vec();
            if name.is_empty() {
                return Err(Error::Corrupt("tree: empty name"));
            }
            let id = ObjectId::from_bytes(hk, &data[nul + 1..nul + 1 + n]);
            entries.push(TreeEntry { mode: m, name, id });
            i = nul + 1 + n;
        }
        Ok(Tree { entries })
    }

    /// Serialize (entries in their current order — call [`Tree::sort`] first when building).
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for e in &self.entries {
            push_octal(&mut out, e.mode);
            out.push(b' ');
            out.extend_from_slice(&e.name);
            out.push(0);
            out.extend_from_slice(e.id.as_bytes());
        }
        out
    }

    /// Sort into git's tree order.
    pub fn sort(&mut self) {
        self.entries.sort_by(|a, b| tree_order(&a.name, a.is_tree(), &b.name, b.is_tree()));
    }

    /// Look an entry up by name.
    pub fn find(&self, name: &[u8]) -> Option<&TreeEntry> {
        self.entries.iter().find(|e| e.name == name)
    }
}

fn push_octal(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 12];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v & 7) as u8;
        v >>= 3;
        if v == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[i..]);
}

// ---------------------------------------------------------------------------------------------
// Signatures
// ---------------------------------------------------------------------------------------------

/// An identity line: `Name <email> <unix seconds> <+|-HHMM>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The name.
    pub name: Vec<u8>,
    /// The email (between the angle brackets).
    pub email: Vec<u8>,
    /// Seconds since the epoch.
    pub time: i64,
    /// UTC offset in minutes.
    pub offset: i32,
    /// `-0000` (offset unknown) rather than `+0000`.
    pub negative_zero: bool,
}

impl Signature {
    /// Build one.
    pub fn new(name: &[u8], email: &[u8], time: i64, offset_minutes: i32) -> Self {
        Signature { name: name.to_vec(), email: email.to_vec(), time, offset: offset_minutes, negative_zero: false }
    }

    /// Parse the value of an `author`/`committer`/`tagger` header.
    pub fn parse(v: &[u8]) -> Result<Self> {
        let lt = v.iter().position(|&b| b == b'<').ok_or(Error::Corrupt("ident: no <"))?;
        let gt = v[lt..].iter().position(|&b| b == b'>').ok_or(Error::Corrupt("ident: no >"))? + lt;
        let mut name = &v[..lt];
        while let [rest @ .., b' '] = name {
            name = rest;
        }
        let email = &v[lt + 1..gt];
        let rest = &v[gt + 1..];
        let rest = trim_ascii(rest);
        let mut parts = rest.split(|&b| b == b' ').filter(|p| !p.is_empty());
        let t = parts.next().ok_or(Error::Corrupt("ident: no time"))?;
        let tz = parts.next().ok_or(Error::Corrupt("ident: no tz"))?;
        let time = parse_i64(t).ok_or(Error::Corrupt("ident: bad time"))?;
        if tz.len() != 5 || !(tz[0] == b'+' || tz[0] == b'-') || !tz[1..].iter().all(u8::is_ascii_digit) {
            return Err(Error::Corrupt("ident: bad tz"));
        }
        let hh = ((tz[1] - b'0') * 10 + (tz[2] - b'0')) as i32;
        let mm = ((tz[3] - b'0') * 10 + (tz[4] - b'0')) as i32;
        let mut offset = hh * 60 + mm;
        if tz[0] == b'-' {
            offset = -offset;
        }
        Ok(Signature {
            name: name.to_vec(),
            email: email.to_vec(),
            time,
            offset,
            negative_zero: tz[0] == b'-' && offset == 0,
        })
    }

    /// Serialize exactly as git writes it.
    pub fn serialize(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&self.name);
        o.extend_from_slice(b" <");
        o.extend_from_slice(&self.email);
        o.extend_from_slice(b"> ");
        if self.time < 0 {
            o.push(b'-');
            push_decimal(&mut o, self.time.unsigned_abs());
        } else {
            push_decimal(&mut o, self.time as u64);
        }
        o.push(b' ');
        o.extend_from_slice(&self.tz_bytes());
        o
    }

    /// `+HHMM` / `-HHMM`.
    pub fn tz_bytes(&self) -> [u8; 5] {
        let neg = self.offset < 0 || self.negative_zero;
        let a = self.offset.unsigned_abs();
        let (h, m) = (a / 60, a % 60);
        [if neg { b'-' } else { b'+' }, b'0' + (h / 10) as u8, b'0' + (h % 10) as u8, b'0' + (m / 10) as u8, b'0' + (m % 10) as u8]
    }
}

fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while let [b' ' | b'\t' | b'\n', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' ' | b'\t' | b'\n'] = s {
        s = rest;
    }
    s
}

fn parse_i64(s: &[u8]) -> Option<i64> {
    let (neg, d) = match s.first()? {
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    if d.is_empty() || d.len() > 18 {
        return None;
    }
    let mut v: i64 = 0;
    for &c in d {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as i64;
    }
    Some(if neg { -v } else { v })
}

// ---------------------------------------------------------------------------------------------
// Commits and tags: ordered headers + message
// ---------------------------------------------------------------------------------------------

/// The shared shape of commits and tags: `key SP value LF` headers (a value continues on lines
/// that begin with a space), a blank line, the message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Headed {
    /// Headers in stored order; multi-line values hold their `\n`s (continuation spaces removed).
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    /// The message (everything after the blank line), `None` when the object has no blank line.
    pub message: Option<Vec<u8>>,
}

impl Headed {
    /// Parse.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut headers: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut i = 0;
        while i < data.len() {
            if data[i] == b'\n' {
                return Ok(Headed { headers, message: Some(data[i + 1..].to_vec()) });
            }
            let eol = data[i..].iter().position(|&b| b == b'\n').map(|p| p + i);
            let line = &data[i..eol.unwrap_or(data.len())];
            if line[0] == b' ' {
                let last = headers.last_mut().ok_or(Error::Corrupt("headers: continuation first"))?;
                last.1.push(b'\n');
                last.1.extend_from_slice(&line[1..]);
            } else {
                let sp = line.iter().position(|&b| b == b' ');
                match sp {
                    Some(sp) => headers.push((line[..sp].to_vec(), line[sp + 1..].to_vec())),
                    None => return Err(Error::Corrupt("headers: line without space")),
                }
            }
            match eol {
                Some(e) => i = e + 1,
                None => return Err(Error::Corrupt("headers: unterminated")),
            }
        }
        Ok(Headed { headers, message: None })
    }

    /// Serialize.
    pub fn serialize(&self) -> Vec<u8> {
        let mut o = Vec::new();
        for (k, v) in &self.headers {
            o.extend_from_slice(k);
            o.push(b' ');
            for &b in v {
                o.push(b);
                if b == b'\n' {
                    o.push(b' ');
                }
            }
            o.push(b'\n');
        }
        if let Some(m) = &self.message {
            o.push(b'\n');
            o.extend_from_slice(m);
        }
        o
    }

    /// The first value of header `key`.
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.headers.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_slice())
    }

    /// Every value of header `key`.
    pub fn get_all<'a>(&'a self, key: &'a [u8]) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.headers.iter().filter(move |(k, _)| k == key).map(|(_, v)| v.as_slice())
    }
}

/// A commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Its headers and message.
    pub raw: Headed,
    /// The id format (to parse `tree`/`parent`).
    pub hash: HashKind,
}

impl Commit {
    /// Parse a commit payload.
    pub fn parse(hk: HashKind, data: &[u8]) -> Result<Self> {
        let raw = Headed::parse(data)?;
        let c = Commit { raw, hash: hk };
        c.tree().ok_or(Error::Corrupt("commit: no tree"))?;
        Ok(c)
    }

    /// Build a new commit exactly as `git commit-tree` lays it out: tree, parents, author,
    /// committer, [encoding], blank line, message.
    pub fn new(tree: ObjectId, parents: &[ObjectId], author: &Signature, committer: &Signature, message: &[u8]) -> Self {
        let mut headers = Vec::new();
        headers.push((b"tree".to_vec(), tree.to_hex().into_bytes()));
        for p in parents {
            headers.push((b"parent".to_vec(), p.to_hex().into_bytes()));
        }
        headers.push((b"author".to_vec(), author.serialize()));
        headers.push((b"committer".to_vec(), committer.serialize()));
        Commit { raw: Headed { headers, message: Some(message.to_vec()) }, hash: tree.kind() }
    }

    /// Serialize.
    pub fn serialize(&self) -> Vec<u8> {
        self.raw.serialize()
    }

    /// The root tree.
    pub fn tree(&self) -> Option<ObjectId> {
        ObjectId::from_hex_kind(self.hash, self.raw.get(b"tree")?)
    }
    /// The parents in order.
    pub fn parents(&self) -> Vec<ObjectId> {
        self.raw.get_all(b"parent").filter_map(|v| ObjectId::from_hex_kind(self.hash, v)).collect()
    }
    /// The author.
    pub fn author(&self) -> Option<Signature> {
        Signature::parse(self.raw.get(b"author")?).ok()
    }
    /// The committer.
    pub fn committer(&self) -> Option<Signature> {
        Signature::parse(self.raw.get(b"committer")?).ok()
    }
    /// The message.
    pub fn message(&self) -> &[u8] {
        self.raw.message.as_deref().unwrap_or(b"")
    }
    /// The first line of the message (the subject).
    pub fn summary(&self) -> &[u8] {
        let m = self.message();
        &m[..m.iter().position(|&b| b == b'\n').unwrap_or(m.len())]
    }
}

/// An annotated tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Its headers and message.
    pub raw: Headed,
    /// The id format.
    pub hash: HashKind,
}

impl Tag {
    /// Parse a tag payload.
    pub fn parse(hk: HashKind, data: &[u8]) -> Result<Self> {
        let raw = Headed::parse(data)?;
        let t = Tag { raw, hash: hk };
        t.target().ok_or(Error::Corrupt("tag: no object"))?;
        t.target_kind().ok_or(Error::Corrupt("tag: bad type"))?;
        Ok(t)
    }
    /// Build a tag as `git tag -a` lays it out.
    pub fn new(target: ObjectId, kind: Kind, name: &[u8], tagger: &Signature, message: &[u8]) -> Self {
        let headers = alloc::vec![
            (b"object".to_vec(), target.to_hex().into_bytes()),
            (b"type".to_vec(), kind.name().as_bytes().to_vec()),
            (b"tag".to_vec(), name.to_vec()),
            (b"tagger".to_vec(), tagger.serialize()),
        ];
        Tag { raw: Headed { headers, message: Some(message.to_vec()) }, hash: target.kind() }
    }
    /// Serialize.
    pub fn serialize(&self) -> Vec<u8> {
        self.raw.serialize()
    }
    /// The tagged object.
    pub fn target(&self) -> Option<ObjectId> {
        ObjectId::from_hex_kind(self.hash, self.raw.get(b"object")?)
    }
    /// The tagged object's kind.
    pub fn target_kind(&self) -> Option<Kind> {
        Kind::from_name(self.raw.get(b"type")?)
    }
    /// The tag name.
    pub fn name(&self) -> Option<&[u8]> {
        self.raw.get(b"tag")
    }
    /// The tagger.
    pub fn tagger(&self) -> Option<Signature> {
        Signature::parse(self.raw.get(b"tagger")?).ok()
    }
}

/// A parsed object of any kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Object {
    /// Blob bytes.
    Blob(Vec<u8>),
    /// A tree.
    Tree(Tree),
    /// A commit.
    Commit(Commit),
    /// A tag.
    Tag(Tag),
}

impl Object {
    /// Parse a payload of `kind`.
    pub fn parse(hk: HashKind, kind: Kind, data: &[u8]) -> Result<Self> {
        Ok(match kind {
            Kind::Blob => Object::Blob(data.to_vec()),
            Kind::Tree => Object::Tree(Tree::parse(hk, data)?),
            Kind::Commit => Object::Commit(Commit::parse(hk, data)?),
            Kind::Tag => Object::Tag(Tag::parse(hk, data)?),
        })
    }
    /// Serialize the payload.
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            Object::Blob(b) => b.clone(),
            Object::Tree(t) => t.serialize(),
            Object::Commit(c) => c.serialize(),
            Object::Tag(t) => t.serialize(),
        }
    }
    /// The kind.
    pub fn kind(&self) -> Kind {
        match self {
            Object::Blob(_) => Kind::Blob,
            Object::Tree(_) => Kind::Tree,
            Object::Commit(_) => Kind::Commit,
            Object::Tag(_) => Kind::Tag,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_roundtrip_with_gpgsig() {
        let raw = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nauthor A U Thor <a@x> 1700000000 +0130\ncommitter C O Mitter <c@x> 1700000001 -0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n abc\n -----END PGP SIGNATURE-----\nx-unknown v\n\nsubject\n\nbody\n";
        let c = Commit::parse(HashKind::Sha1, raw).unwrap();
        assert_eq!(c.serialize(), raw.to_vec());
        assert_eq!(c.summary(), b"subject");
        let a = c.author().unwrap();
        assert_eq!(a.offset, 90);
        assert_eq!(a.serialize(), b"A U Thor <a@x> 1700000000 +0130".to_vec());
        let cm = c.committer().unwrap();
        assert!(cm.negative_zero);
        assert_eq!(&cm.tz_bytes(), b"-0000");
        assert_eq!(c.raw.get(b"gpgsig").unwrap(), b"-----BEGIN PGP SIGNATURE-----\n\nabc\n-----END PGP SIGNATURE-----");
    }

    #[test]
    fn empty_tree_id() {
        assert_eq!(hash_object(HashKind::Sha1, Kind::Tree, b"").to_hex(), "4b825dc642cb6eb9a060e54bf8d69288fbee4904");
    }

    #[test]
    fn tree_order_slash() {
        use core::cmp::Ordering::*;
        assert_eq!(tree_order(b"a", true, b"a.c", false), Greater); // "a/" > "a."
        assert_eq!(tree_order(b"a", false, b"a.c", false), Less);
        assert_eq!(tree_order(b"a-b", false, b"a", true), Less); // '-' < '/'
    }
}
