// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Loose objects (`objects/xx/yyyy…`): one zlib stream of `"<kind> <len>\0" + payload`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::deflate;
use crate::hash::{HashKind, ObjectId};
use crate::object::{self, Kind};
use crate::zlib;
use crate::{Error, Result};

/// The path of a loose object relative to `objects/`: `xx/yyyy…`.
pub fn path(id: &ObjectId) -> String {
    let h = id.to_hex();
    let mut p = String::with_capacity(h.len() + 1);
    p.push_str(&h[..2]);
    p.push('/');
    p.push_str(&h[2..]);
    p
}

/// Encode a loose object at `level`: (id, file bytes).
pub fn encode(hk: HashKind, kind: Kind, payload: &[u8], level: u8) -> (ObjectId, Vec<u8>) {
    let mut raw = object::header(kind, payload.len());
    raw.extend_from_slice(payload);
    let id = hk.digest(&raw);
    (id, deflate::zlib_compress(&raw, level))
}

/// Decode a loose object file: (kind, payload). `max` bounds the inflated size.
pub fn decode(file: &[u8], max: usize) -> Result<(Kind, Vec<u8>)> {
    let (raw, _) = zlib::inflate(file, 0, max)?;
    let (kind, len, hl) = object::parse_header(&raw)?;
    if raw.len() - hl != len {
        return Err(Error::Corrupt("loose object: length disagrees with header"));
    }
    Ok((kind, raw[hl..].to_vec()))
}

/// Decode and verify the id.
pub fn decode_verified(id: &ObjectId, file: &[u8], max: usize) -> Result<(Kind, Vec<u8>)> {
    let (kind, payload) = decode(file, max)?;
    if object::hash_object(id.kind(), kind, &payload) != *id {
        return Err(Error::HashMismatch);
    }
    Ok((kind, payload))
}
