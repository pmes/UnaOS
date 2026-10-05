// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `git_core` — UnaOS's own git (GITCORE, LEDGER SR59).
//!
//! CHARTER: Vaire — shared-core. Vaire (CODEX §2, the Loom: repositories, mirrors, dev trees) owns
//! the repositories; this crate is the format arithmetic both rings can link. It holds no bus surface.
//!
//! Written from git's own format documentation (`gitformat-pack(5)`, `gitformat-index(5)`,
//! `gitformat-commit-graph`, `gitprotocol-v2(5)`, `gitprotocol-http(5)`, `gitprotocol-pack(5)`,
//! `git-config(1)`, `gitignore(5)`, `hash-function-transition`), RFC 1950/1951 for zlib/DEFLATE, and
//! for the diff engine the published algorithms (Myers 1986; histogram diff as JGit/xdiff define it)
//! with xdiff's observable output rules. Every module is proven against the `git` CLI as the oracle.
//!
//! | module | what |
//! |---|---|
//! | [`hash`] | object ids, SHA-1 and SHA-256 object formats |
//! | [`object`] | blob / tree / commit / tag parse + serialize (byte-identical round trip) |
//! | [`deflate`] | RFC 1951 ENCODER (stored / fixed / dynamic blocks) + zlib framing — first in the tree |
//! | [`zlib`] | inflate over pixel_core's decoder, with the consumed-byte count packs need |
//! | [`loose`] | loose object encode / decode |
//! | [`delta`] | git delta apply + encoder |
//! | [`pack`] | packfile v2 read / write, idx v2 read / write, index-pack, multi-pack-index read |
//! | [`refs`] | loose refs, packed-refs, symbolic refs, reflog |
//! | [`config`] | git-config syntax, includes, typed values |
//! | [`ignore`] | gitignore(5) / gitattributes(5) matching (wildmatch) |
//! | [`index`] | the index file v2 / v3 / v4, extensions (unknown preserved) |
//! | [`diff`] | Myers + histogram line diff, unified output, `--stat`, rename similarity |
//! | [`protocol`] | pkt-line, protocol v2 (ls-refs, fetch), receive-pack push, dumb HTTP |
//! | `repo` (std) | the on-disk repository |
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod hash;
pub mod object;
pub mod deflate;
pub mod zlib;
pub mod loose;
pub mod refs;
pub mod config;
pub mod ignore;

pub use hash::{HashKind, ObjectId};
pub use object::{Kind, Commit, Tag, Tree, TreeEntry, Signature};

/// The one error type: a named, falsifiable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Malformed input; the string names the structure and the rule broken.
    Corrupt(&'static str),
    /// A zlib stream failed to inflate.
    Inflate(&'static str),
    /// An object the operation needs is not present.
    Missing(ObjectId),
    /// The input uses a feature this crate does not implement (named).
    Unsupported(&'static str),
    /// A computed id did not match the expected one.
    HashMismatch,
    /// An I/O failure (std only), with the operation.
    Io(alloc::string::String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Corrupt(s) => write!(f, "corrupt: {s}"),
            Error::Inflate(s) => write!(f, "inflate: {s}"),
            Error::Missing(id) => write!(f, "missing object {id}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::HashMismatch => write!(f, "object hash mismatch"),
            Error::Io(s) => write!(f, "io: {s}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

/// Crate result.
pub type Result<T> = core::result::Result<T, Error>;
