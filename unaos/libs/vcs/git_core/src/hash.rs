// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Object ids. Git's default object format is SHA-1 (20 bytes); `extensions.objectFormat = sha256`
//! selects SHA-256 (32 bytes) per `hash-function-transition`. An id is the digest of
//! `"<type> <decimal length>\0" + payload`.

use core::fmt;

use crypto_core::{Sha1, Sha256};

/// The object format (hash algorithm) of a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum HashKind {
    /// SHA-1, 20-byte ids (the default object format).
    #[default]
    Sha1,
    /// SHA-256, 32-byte ids.
    Sha256,
}

impl HashKind {
    /// Raw id length in bytes.
    pub const fn len(self) -> usize {
        match self {
            HashKind::Sha1 => 20,
            HashKind::Sha256 => 32,
        }
    }
    /// Hex id length.
    pub const fn hex_len(self) -> usize {
        self.len() * 2
    }
    /// The `extensions.objectFormat` / protocol `object-format` name.
    pub const fn name(self) -> &'static str {
        match self {
            HashKind::Sha1 => "sha1",
            HashKind::Sha256 => "sha256",
        }
    }
    /// Parse an object-format name.
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "sha1" => Some(HashKind::Sha1),
            "sha256" => Some(HashKind::Sha256),
            _ => None,
        }
    }
    /// The all-zero id of this format.
    pub const fn null(self) -> ObjectId {
        ObjectId { kind: self, bytes: [0; 32] }
    }
    /// The pack/idx format version number used in multi-pack-index and commit-graph headers.
    pub const fn oid_version(self) -> u8 {
        match self {
            HashKind::Sha1 => 1,
            HashKind::Sha256 => 2,
        }
    }
    /// A fresh streaming hasher.
    pub fn hasher(self) -> Hasher {
        match self {
            HashKind::Sha1 => Hasher::Sha1(Sha1::new()),
            HashKind::Sha256 => Hasher::Sha256(Sha256::new()),
        }
    }
    /// One-shot digest of `data` as an id.
    pub fn digest(self, data: &[u8]) -> ObjectId {
        let mut h = self.hasher();
        h.update(data);
        h.finish()
    }
}

/// A streaming hasher in either format.
#[derive(Clone)]
pub enum Hasher {
    /// SHA-1.
    Sha1(Sha1),
    /// SHA-256.
    Sha256(Sha256),
}

impl Hasher {
    /// Absorb bytes.
    pub fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Sha1(h) => h.update(data),
            Hasher::Sha256(h) => h.update(data),
        }
    }
    /// The id.
    pub fn finish(self) -> ObjectId {
        match self {
            Hasher::Sha1(h) => ObjectId::from_bytes(HashKind::Sha1, &h.finalize()),
            Hasher::Sha256(h) => ObjectId::from_bytes(HashKind::Sha256, &h.finalize()),
        }
    }
}

/// An object id in either format. Stored in a fixed 32-byte buffer; only `kind.len()` bytes count.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId {
    kind: HashKind,
    bytes: [u8; 32],
}

impl ObjectId {
    /// From raw bytes (`b.len()` must be `kind.len()`; extra bytes are ignored, short panics).
    pub fn from_bytes(kind: HashKind, b: &[u8]) -> Self {
        let mut bytes = [0u8; 32];
        bytes[..kind.len()].copy_from_slice(&b[..kind.len()]);
        ObjectId { kind, bytes }
    }
    /// From raw bytes, inferring the kind from the length (20 or 32).
    pub fn from_raw(b: &[u8]) -> Option<Self> {
        match b.len() {
            20 => Some(Self::from_bytes(HashKind::Sha1, b)),
            32 => Some(Self::from_bytes(HashKind::Sha256, b)),
            _ => None,
        }
    }
    /// Parse a full hex id of the given kind (lower or upper case).
    pub fn from_hex_kind(kind: HashKind, s: &[u8]) -> Option<Self> {
        if s.len() != kind.hex_len() {
            return None;
        }
        let mut bytes = [0u8; 32];
        for i in 0..kind.len() {
            bytes[i] = (unhex(s[2 * i])? << 4) | unhex(s[2 * i + 1])?;
        }
        Some(ObjectId { kind, bytes })
    }
    /// Parse a full hex id, inferring the kind from the length (40 or 64).
    pub fn from_hex(s: &[u8]) -> Option<Self> {
        match s.len() {
            40 => Self::from_hex_kind(HashKind::Sha1, s),
            64 => Self::from_hex_kind(HashKind::Sha256, s),
            _ => None,
        }
    }
    /// The format.
    pub fn kind(&self) -> HashKind {
        self.kind
    }
    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.kind.len()]
    }
    /// Is this the all-zero id?
    pub fn is_null(&self) -> bool {
        self.as_bytes().iter().all(|&b| b == 0)
    }
    /// Hex into a caller buffer (64 bytes holds either format); returns the used prefix.
    pub fn hex_into<'a>(&self, out: &'a mut [u8; 64]) -> &'a [u8] {
        for (i, b) in self.as_bytes().iter().enumerate() {
            out[2 * i] = HEX[(b >> 4) as usize];
            out[2 * i + 1] = HEX[(b & 15) as usize];
        }
        &out[..self.kind.hex_len()]
    }
    /// Lower-case hex string.
    pub fn to_hex(&self) -> alloc::string::String {
        let mut buf = [0u8; 64];
        let h = self.hex_into(&mut buf);
        alloc::string::String::from_utf8(h.to_vec()).unwrap()
    }
    /// Does this id start with the hex prefix `p` (any length, lower/upper)?
    pub fn starts_with_hex(&self, p: &[u8]) -> bool {
        if p.len() > self.kind.hex_len() {
            return false;
        }
        let mut buf = [0u8; 64];
        let h = self.hex_into(&mut buf);
        h.iter().zip(p).all(|(a, b)| *a == b.to_ascii_lowercase())
    }
    /// The first byte (fan-out tables index on it).
    pub fn first_byte(&self) -> u8 {
        self.bytes[0]
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn unhex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; 64];
        let h = self.hex_into(&mut buf);
        f.write_str(core::str::from_utf8(h).unwrap())
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_blob_ids() {
        // `git hash-object -t blob /dev/null` in both formats.
        assert_eq!(HashKind::Sha1.digest(b"blob 0\0").to_hex(), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(
            HashKind::Sha256.digest(b"blob 0\0").to_hex(),
            "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"
        );
        let id = ObjectId::from_hex(b"E69DE29BB2D1D6434B8B29AE775AD8C2E48C5391").unwrap();
        assert!(id.starts_with_hex(b"e69de"));
    }
}
