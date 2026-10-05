// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The on-disk formats, version 1. All integers little-endian. Every parser is fail-closed: an unknown
//! magic, version or suite, a length that does not add up, trailing bytes or a non-UTF-8 string refuse
//! the WHOLE file; nothing is partially adopted.
//!
//! ## A secret: `/home/<u>/.holocron/<ns>/<name>` (one secret per file)
//!
//! ```text
//! off len  field
//!   0   4  magic "HCRN"
//!   4   1  version = 1
//!   5   1  suite (0x01 Argon2id+HKDF-SHA-256+ChaCha20-Poly1305 · 0xFE TEST-INSECURE)
//!   6   2  header_len (= 62 + kind_len + label_len; the offset of the sealed body)
//!   8   4  kdf m_kib   ┐ the RING's Argon2id parameters, recorded so a file names
//!  12   4  kdf t       │ how its ring key was derived (audit, migration); must equal
//!  16   4  kdf p       ┘ the ring's at open
//!  20  16  salt        per-file HKDF salt: file key = HKDF(ring key, salt, "holocron/v1/secret")
//!  36  12  nonce       AEAD nonce
//!  48   8  created     i64 unix seconds          ┐ the metadata, mirrored as typed UnaFS
//!  56   4  sealed_len  ciphertext + 16-byte tag  │ attributes (`created` Int, `kind` Str,
//!  60   1  kind_len                              │ `label` Str) — clear, but AUTHENTICATED:
//!  61   1  label_len                             │ the whole header is AEAD associated data
//!  62   …  kind, label (UTF-8)                   ┘
//!   …   …  sealed body (sealed_len bytes, the file's last byte)
//! ```
//!
//! AEAD associated data = `header || ns || 0x00 || name`: a file renamed or moved to another namespace,
//! or a header edited (label, kind, created, params), fails to open.
//!
//! ## The ring: `/home/<u>/.holocron/.ring`
//!
//! ```text
//!   0   4  magic "HCRR"
//!   4   1  version = 1
//!   5   1  suite
//!   6   2  reserved = 0
//!   8  12  kdf m_kib, t, p   (Argon2id over the login password)
//!  20  16  salt              Argon2id salt
//!  36  12  nonce             verifier nonce
//!  48   1  owner_len
//!  49   …  owner             the owning principal, `user:<name>#<uid>`
//!   …  48  verifier          seal(HKDF(ring key, salt, "holocron/v1/verifier"), nonce,
//!                                 aad = ring header, VERIFIER_PLAINTEXT) — 32 + 16 bytes
//! ```

use crate::seal::{KdfParams, NONCE_LEN, SALT_LEN, TAG_LEN};
use alloc::string::String;
use alloc::vec::Vec;

/// Secret file magic.
pub const MAGIC_SECRET: [u8; 4] = *b"HCRN";
/// Ring file magic.
pub const MAGIC_RING: [u8; 4] = *b"HCRR";
/// The only version this code reads or writes.
pub const VERSION: u8 = 1;
/// Argon2id (RFC 9106) + HKDF-SHA-256 (RFC 5869) + ChaCha20-Poly1305 (RFC 8439); Ed25519 for keys.
pub const SUITE_ARGON2ID_CHACHA20POLY1305: u8 = 0x01;
/// The deliberately insecure test suite (`testseal`). Never accepted by the production sealer.
pub const SUITE_TEST_INSECURE: u8 = 0xFE;
/// Fixed part of a secret header.
pub const SECRET_FIXED: usize = 62;
/// Fixed part of a ring header (before the owner bytes).
pub const RING_FIXED: usize = 49;
/// Largest secret plaintext (it must fit one 4 KiB BANDY v1 bus body with its framing).
pub const SECRET_MAX: usize = 3072;
/// The 32 bytes the ring verifier seals.
pub const VERIFIER_PLAINTEXT: &[u8; 32] = b"HOLOCRON-RING-V1-VERIFIER-------";
/// Bytes of the sealed verifier.
pub const VERIFIER_SEALED: usize = 32 + TAG_LEN;
/// HKDF info for a secret's file key.
pub const INFO_SECRET: &[u8] = b"holocron/v1/secret";
/// HKDF info for the ring verifier key.
pub const INFO_VERIFIER: &[u8] = b"holocron/v1/verifier";

/// Why a file was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// Shorter than its fixed header, or its lengths do not add up, or bytes trail.
    Length,
    /// Wrong magic.
    Magic,
    /// A version this code does not read.
    Version,
    /// Kind or label is not UTF-8, or the owner is empty.
    Text,
    /// A reserved field is non-zero.
    Reserved,
}

/// A secret's metadata (clear, authenticated; mirrored as UnaFS typed attributes).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Meta {
    /// `created`: unix seconds.
    pub created: i64,
    /// `kind`: what the secret is (`api-key`, `password`, `ssh-ed25519`, ...). ≤ 255 bytes.
    pub kind: String,
    /// `label`: a human label. ≤ 255 bytes.
    pub label: String,
}

/// A parsed secret header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretHeader {
    /// Suite byte.
    pub suite: u8,
    /// The ring's KDF parameters.
    pub kdf: KdfParams,
    /// Per-file HKDF salt.
    pub salt: [u8; SALT_LEN],
    /// AEAD nonce.
    pub nonce: [u8; NONCE_LEN],
    /// Metadata.
    pub meta: Meta,
    /// Length of the sealed body (ciphertext + tag).
    pub sealed_len: u32,
}

fn put_u16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl SecretHeader {
    /// Header length on disk.
    pub fn len(&self) -> usize {
        SECRET_FIXED + self.meta.kind.len() + self.meta.label.len()
    }
    /// Never empty (kept for clippy's `len_without_is_empty`).
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Encode the header (kind and label must each be ≤ 255 bytes; [`crate::ring`] checks).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.len());
        v.extend_from_slice(&MAGIC_SECRET);
        v.push(VERSION);
        v.push(self.suite);
        put_u16(&mut v, self.len() as u16);
        put_u32(&mut v, self.kdf.m_kib);
        put_u32(&mut v, self.kdf.t);
        put_u32(&mut v, self.kdf.p);
        v.extend_from_slice(&self.salt);
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(&self.meta.created.to_le_bytes());
        put_u32(&mut v, self.sealed_len);
        v.push(self.meta.kind.len() as u8);
        v.push(self.meta.label.len() as u8);
        v.extend_from_slice(self.meta.kind.as_bytes());
        v.extend_from_slice(self.meta.label.as_bytes());
        v
    }
}

/// Parse a whole secret file: `(header, header bytes, sealed body)`.
pub fn parse_secret(b: &[u8]) -> Result<(SecretHeader, &[u8], &[u8]), FormatError> {
    if b.len() < SECRET_FIXED {
        return Err(FormatError::Length);
    }
    if b[0..4] != MAGIC_SECRET {
        return Err(FormatError::Magic);
    }
    if b[4] != VERSION {
        return Err(FormatError::Version);
    }
    let header_len = u16_at(b, 6) as usize;
    let kind_len = b[60] as usize;
    let label_len = b[61] as usize;
    if header_len != SECRET_FIXED + kind_len + label_len || b.len() < header_len {
        return Err(FormatError::Length);
    }
    let sealed_len = u32_at(b, 56);
    let sealed = &b[header_len..];
    if sealed.len() != sealed_len as usize || sealed.len() < TAG_LEN || sealed.len() > SECRET_MAX + TAG_LEN {
        return Err(FormatError::Length);
    }
    let kind = core::str::from_utf8(&b[SECRET_FIXED..SECRET_FIXED + kind_len]).map_err(|_| FormatError::Text)?;
    let label = core::str::from_utf8(&b[SECRET_FIXED + kind_len..header_len]).map_err(|_| FormatError::Text)?;
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&b[20..36]);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&b[36..48]);
    let mut created = [0u8; 8];
    created.copy_from_slice(&b[48..56]);
    let hdr = SecretHeader {
        suite: b[5],
        kdf: KdfParams { m_kib: u32_at(b, 8), t: u32_at(b, 12), p: u32_at(b, 16) },
        salt,
        nonce,
        meta: Meta { created: i64::from_le_bytes(created), kind: kind.into(), label: label.into() },
        sealed_len,
    };
    Ok((hdr, &b[..header_len], sealed))
}

/// The associated data a secret is sealed under: `header || ns || 0x00 || name`.
pub fn secret_aad(header: &[u8], ns: &str, name: &str) -> Vec<u8> {
    let mut a = Vec::with_capacity(header.len() + ns.len() + 1 + name.len());
    a.extend_from_slice(header);
    a.extend_from_slice(ns.as_bytes());
    a.push(0);
    a.extend_from_slice(name.as_bytes());
    a
}

/// A parsed ring header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingHeader {
    /// Suite byte.
    pub suite: u8,
    /// Argon2id parameters.
    pub kdf: KdfParams,
    /// Argon2id salt.
    pub salt: [u8; SALT_LEN],
    /// Verifier nonce.
    pub nonce: [u8; NONCE_LEN],
    /// Owning principal (`user:<name>#<uid>`), 1..=255 bytes.
    pub owner: String,
}

impl RingHeader {
    /// Encode the header (everything before the verifier).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(RING_FIXED + self.owner.len());
        v.extend_from_slice(&MAGIC_RING);
        v.push(VERSION);
        v.push(self.suite);
        put_u16(&mut v, 0);
        put_u32(&mut v, self.kdf.m_kib);
        put_u32(&mut v, self.kdf.t);
        put_u32(&mut v, self.kdf.p);
        v.extend_from_slice(&self.salt);
        v.extend_from_slice(&self.nonce);
        v.push(self.owner.len() as u8);
        v.extend_from_slice(self.owner.as_bytes());
        v
    }
}

/// Parse a whole ring file: `(header, header bytes, sealed verifier)`.
pub fn parse_ring(b: &[u8]) -> Result<(RingHeader, &[u8], &[u8]), FormatError> {
    if b.len() < RING_FIXED {
        return Err(FormatError::Length);
    }
    if b[0..4] != MAGIC_RING {
        return Err(FormatError::Magic);
    }
    if b[4] != VERSION {
        return Err(FormatError::Version);
    }
    if u16_at(b, 6) != 0 {
        return Err(FormatError::Reserved);
    }
    let owner_len = b[48] as usize;
    let hlen = RING_FIXED + owner_len;
    if owner_len == 0 {
        return Err(FormatError::Text);
    }
    if b.len() != hlen + VERIFIER_SEALED {
        return Err(FormatError::Length);
    }
    let owner = core::str::from_utf8(&b[RING_FIXED..hlen]).map_err(|_| FormatError::Text)?;
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&b[20..36]);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&b[36..48]);
    let hdr = RingHeader {
        suite: b[5],
        kdf: KdfParams { m_kib: u32_at(b, 8), t: u32_at(b, 12), p: u32_at(b, 16) },
        salt,
        nonce,
        owner: owner.into(),
    };
    Ok((hdr, &b[..hlen], &b[hlen..]))
}
