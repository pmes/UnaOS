// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! References (`gitrepository-layout(5)`, `git-check-ref-format(1)`, `git-pack-refs(1)`,
//! `git-reflog(1)`): loose ref files, symbolic refs, the `packed-refs` file with its peeled lines,
//! reflog entries, and ref-name validation. Bytes in, bytes out — the std `repo` module does the
//! file I/O and lock-file dance.

use alloc::vec::Vec;

use crate::hash::{HashKind, ObjectId};
use crate::object::Signature;
use crate::{Error, Result};

/// The content of one loose ref file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefValue {
    /// An object id.
    Direct(ObjectId),
    /// `ref: <target>`.
    Symbolic(Vec<u8>),
}

impl RefValue {
    /// Parse a loose ref file (trailing whitespace ignored, as git does).
    pub fn parse(hk: HashKind, data: &[u8]) -> Result<Self> {
        let mut d = data;
        while let [rest @ .., b'\n' | b'\r' | b' ' | b'\t'] = d {
            d = rest;
        }
        if let Some(t) = d.strip_prefix(b"ref:") {
            let mut t = t;
            while let [b' ' | b'\t', rest @ ..] = t {
                t = rest;
            }
            return Ok(RefValue::Symbolic(t.to_vec()));
        }
        if d.len() < hk.hex_len() {
            return Err(Error::Corrupt("ref: short id"));
        }
        let id = ObjectId::from_hex_kind(hk, &d[..hk.hex_len()]).ok_or(Error::Corrupt("ref: bad hex"))?;
        if d.len() > hk.hex_len() && !d[hk.hex_len()].is_ascii_whitespace() {
            return Err(Error::Corrupt("ref: trailing garbage"));
        }
        Ok(RefValue::Direct(id))
    }

    /// Serialize as git writes it.
    pub fn serialize(&self) -> Vec<u8> {
        let mut o = Vec::new();
        match self {
            RefValue::Direct(id) => o.extend_from_slice(id.to_hex().as_bytes()),
            RefValue::Symbolic(t) => {
                o.extend_from_slice(b"ref: ");
                o.extend_from_slice(t);
            }
        }
        o.push(b'\n');
        o
    }
}

/// One `packed-refs` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedRef {
    /// Full ref name.
    pub name: Vec<u8>,
    /// Its value.
    pub id: ObjectId,
    /// The fully peeled object (annotated tags), from a `^` line.
    pub peeled: Option<ObjectId>,
}

/// The `packed-refs` file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackedRefs {
    /// The header traits (`peeled`, `fully-peeled`, `sorted`), when the header is present.
    pub traits: Option<Vec<Vec<u8>>>,
    /// Records in file order.
    pub refs: Vec<PackedRef>,
}

impl PackedRefs {
    /// Parse.
    pub fn parse(hk: HashKind, data: &[u8]) -> Result<Self> {
        let mut p = PackedRefs::default();
        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            if let Some(h) = line.strip_prefix(b"# pack-refs with:") {
                p.traits = Some(h.split(|&b| b == b' ').filter(|t| !t.is_empty()).map(|t| t.to_vec()).collect());
                continue;
            }
            if line[0] == b'#' {
                continue;
            }
            if line[0] == b'^' {
                let id = ObjectId::from_hex_kind(hk, &line[1..]).ok_or(Error::Corrupt("packed-refs: bad peeled line"))?;
                let last = p.refs.last_mut().ok_or(Error::Corrupt("packed-refs: peeled line first"))?;
                last.peeled = Some(id);
                continue;
            }
            let hl = hk.hex_len();
            if line.len() < hl + 2 || line[hl] != b' ' {
                return Err(Error::Corrupt("packed-refs: bad record"));
            }
            let id = ObjectId::from_hex_kind(hk, &line[..hl]).ok_or(Error::Corrupt("packed-refs: bad id"))?;
            p.refs.push(PackedRef { name: line[hl + 1..].to_vec(), id, peeled: None });
        }
        Ok(p)
    }

    /// Serialize as `git pack-refs` writes it (header `peeled fully-peeled sorted`, sorted by name).
    pub fn serialize(&self) -> Vec<u8> {
        let mut o = Vec::new();
        match &self.traits {
            Some(t) => {
                o.extend_from_slice(b"# pack-refs with:");
                for x in t {
                    o.push(b' ');
                    o.extend_from_slice(x);
                }
                o.extend_from_slice(b" \n");
            }
            None => {}
        }
        for r in &self.refs {
            o.extend_from_slice(r.id.to_hex().as_bytes());
            o.push(b' ');
            o.extend_from_slice(&r.name);
            o.push(b'\n');
            if let Some(p) = &r.peeled {
                o.push(b'^');
                o.extend_from_slice(p.to_hex().as_bytes());
                o.push(b'\n');
            }
        }
        o
    }

    /// The traits git 2.x writes.
    pub fn standard_traits() -> Vec<Vec<u8>> {
        alloc::vec![b"peeled".to_vec(), b"fully-peeled".to_vec(), b"sorted".to_vec()]
    }

    /// Look up by full name.
    pub fn find(&self, name: &[u8]) -> Option<&PackedRef> {
        if self.traits.as_ref().is_some_and(|t| t.iter().any(|x| x == b"sorted")) {
            self.refs.binary_search_by(|r| r.name.as_slice().cmp(name)).ok().map(|i| &self.refs[i])
        } else {
            self.refs.iter().find(|r| r.name == name)
        }
    }

    /// Insert or replace, keeping name order.
    pub fn upsert(&mut self, r: PackedRef) {
        match self.refs.binary_search_by(|x| x.name.cmp(&r.name)) {
            Ok(i) => self.refs[i] = r,
            Err(i) => self.refs.insert(i, r),
        }
    }

    /// Remove by name; true when present.
    pub fn remove(&mut self, name: &[u8]) -> bool {
        let n = self.refs.len();
        self.refs.retain(|r| r.name != name);
        n != self.refs.len()
    }
}

/// One reflog line: `<old> <new> <ident>\t<message>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    /// Previous value.
    pub old: ObjectId,
    /// New value.
    pub new: ObjectId,
    /// Who and when.
    pub who: Signature,
    /// The message (no newline).
    pub message: Vec<u8>,
}

impl ReflogEntry {
    /// Parse a reflog file into entries.
    pub fn parse_all(hk: HashKind, data: &[u8]) -> Result<Vec<Self>> {
        let mut v = Vec::new();
        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let hl = hk.hex_len();
            if line.len() < 2 * hl + 2 {
                return Err(Error::Corrupt("reflog: short line"));
            }
            let old = ObjectId::from_hex_kind(hk, &line[..hl]).ok_or(Error::Corrupt("reflog: old"))?;
            let new = ObjectId::from_hex_kind(hk, &line[hl + 1..2 * hl + 1]).ok_or(Error::Corrupt("reflog: new"))?;
            let rest = &line[2 * hl + 2..];
            let (ident, msg) = match rest.iter().position(|&b| b == b'\t') {
                Some(t) => (&rest[..t], &rest[t + 1..]),
                None => (rest, &b""[..]),
            };
            v.push(ReflogEntry { old, new, who: Signature::parse(ident)?, message: msg.to_vec() });
        }
        Ok(v)
    }

    /// Serialize one line (with its newline) exactly as git appends it.
    pub fn serialize(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(self.old.to_hex().as_bytes());
        o.push(b' ');
        o.extend_from_slice(self.new.to_hex().as_bytes());
        o.push(b' ');
        o.extend_from_slice(&self.who.serialize());
        o.push(b'\t');
        // git collapses the message to one line.
        for &b in &self.message {
            o.push(if b == b'\n' { b' ' } else { b });
        }
        o.push(b'\n');
        o
    }
}

/// `git check-ref-format` rules for a full ref name (`allow_onelevel` permits e.g. `HEAD`).
pub fn is_valid_name(name: &[u8], allow_onelevel: bool) -> bool {
    if name.is_empty() || name == b"@" || name.ends_with(b"/") || name.starts_with(b"/") || name.ends_with(b".") {
        return false;
    }
    if !allow_onelevel && !name.contains(&b'/') {
        return false;
    }
    if name.windows(2).any(|w| w == b".." || w == b"@{" || w == b"//") {
        return false;
    }
    for comp in name.split(|&b| b == b'/') {
        if comp.is_empty() || comp.starts_with(b".") || comp.ends_with(b".lock") {
            return false;
        }
    }
    !name.iter().any(|&b| b < 0x20 || b == 0x7f || matches!(b, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\'))
}

/// The dwim expansion order git uses for a short name (`git rev-parse` / `ref_rev_parse_rules`).
pub const RESOLVE_RULES: &[&str] = &["%s", "refs/%s", "refs/tags/%s", "refs/heads/%s", "refs/remotes/%s", "refs/remotes/%s/HEAD"];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_ref_format() {
        for ok in [&b"refs/heads/main"[..], b"refs/tags/v1.0", b"refs/heads/a-b_c"] {
            assert!(is_valid_name(ok, false), "{ok:?}");
        }
        for bad in [&b"refs/heads/a..b"[..], b"refs/heads/.x", b"refs/heads/x.lock", b"refs/heads/x ", b"refs//x", b"refs/x@{1", b"main", b"refs/heads/x."] {
            assert!(!is_valid_name(bad, false), "{bad:?}");
        }
        assert!(is_valid_name(b"HEAD", true));
    }
}
