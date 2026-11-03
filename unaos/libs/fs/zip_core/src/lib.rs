// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Geode (CODEX §2, Archives), seam shared-core — the ZIP container both rings can link; Vug's mesh_core
//! reads and writes 3MF packages through it.
//!
//! VUG3MF (LEDGER SR486). The records are PKWARE's APPNOTE.TXT (6.3.x): the local file header (4.3.7, signature
//! `0x04034b50`, 30 bytes), the central directory file header (4.3.12, `0x02014b50`, 46 bytes) and the end of
//! central directory record (4.3.16, `0x06054b50`, 22 bytes plus a comment of at most 65 535 bytes). Methods 0
//! (stored) and 8 (deflated) only; ZIP64, encryption, other methods and spanned archives are refused by name.
//!
//! Nothing is a codec of its own: [`Entry::data`] inflates through `pixel_core::inflate::inflate_raw` and checks
//! `pixel_core::crc::crc32` (CRC-32/ISO-HDLC, APPNOTE 4.4.7); [`Writer`] deflates through
//! `git_core::deflate::deflate`. Pure: bytes in, a value or a named [`Refusal`] out; no clock (the writer's DOS
//! time is the fixed 1980-01-01 00:00, so its output is byte-deterministic).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod read;
mod write;

pub use read::{Archive, Entry, read};
pub use write::Writer;

/// The compression methods this core reads and writes (APPNOTE 4.4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Method 0.
    Stored,
    /// Method 8 (raw RFC 1951).
    Deflated,
}

impl Method {
    /// The APPNOTE method number.
    pub fn code(self) -> u16 {
        match self {
            Method::Stored => 0,
            Method::Deflated => 8,
        }
    }
}

/// Why an archive is refused (the first failure), each by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No end of central directory record in the last 22 + 65 535 bytes.
    NotZip,
    /// A record or an entry's data runs past the end of the buffer.
    Truncated { what: &'static str },
    /// A record whose signature is not the one its position requires.
    BadSignature { what: &'static str },
    /// A ZIP64 archive (0xFFFF / 0xFFFFFFFF fields or the ZIP64 locator); the writer: over 65 535 entries or 4 GiB.
    Zip64,
    /// An archive spanned over more than one disk.
    MultiDisk,
    /// An entry with the encryption flag (general purpose bit 0).
    Encrypted { name: alloc::string::String },
    /// An entry compressed by a method other than 0 or 8.
    Method { name: alloc::string::String, method: u16 },
    /// Central directory entry `index` has a name that is not UTF-8.
    BadName { index: u32 },
    /// The local header disagrees with the central directory (method or name).
    LocalMismatch { name: alloc::string::String },
    /// The deflated data is malformed (pixel_core's reason).
    Inflate { name: alloc::string::String, reason: &'static str },
    /// The data's length is not the size the central directory declares.
    SizeMismatch { name: alloc::string::String },
    /// The data's CRC-32 is not the one the central directory declares.
    CrcMismatch { name: alloc::string::String },
    /// The writer: a name longer than 65 535 bytes.
    NameTooLong,
}

pub(crate) const SIG_LOCAL: u32 = 0x0403_4b50;
pub(crate) const SIG_CENTRAL: u32 = 0x0201_4b50;
pub(crate) const SIG_EOCD: u32 = 0x0605_4b50;
pub(crate) const SIG_ZIP64_LOCATOR: u32 = 0x0706_4b50;
/// General purpose bit 0: the entry is encrypted.
pub(crate) const FLAG_ENCRYPTED: u16 = 1 << 0;
/// General purpose bit 11: the name is UTF-8 (APPNOTE appendix D).
pub(crate) const FLAG_UTF8: u16 = 1 << 11;
