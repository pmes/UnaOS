// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HOLOCRON2 (rmbp-ledger B355): the BANDY v1 frame around a Holocron body — the metal transport's codec,
//! ONE copy, linked by HOLOCRON.ELF (the fulfiller) and every ring-3 client (LUMEN.ELF through
//! `vein_ring3::holocron`), and KAT'd on the host against the host daemon's own socket bytes
//! (`handlers/holocron/tests/m3_metal_frame.rs`).
//!
//! The header is una-abi's BUS v1 (`BUS_HDR_LEN` = 52, little-endian), restated here because this crate is
//! dependency-free beyond CRYPTOCORE (the host KAT pins the two against each other):
//!
//! | bytes | field |
//! |---|---|
//! | 0..4 | magic `UBS1` |
//! | 4 | version 1 |
//! | 5 | kind (1 request, 2 reply) |
//! | 6 | verb (144..=151, or 127 REGISTER) |
//! | 7 | reserved 0 |
//! | 8..12 | corr u32 |
//! | 12..16 | status i32 (0 on a request) |
//! | 16..48 | principal record (all-zero from ring 3; the kernel stamps the CALLER on a relayed request) |
//! | 48..52 | body_len u32 |
//!
//! A body is [`crate::wire`]'s; an error reply carries no body (the BANDY v1 frozen rule). The host socket
//! frames the SAME body as `[u32 len][u8 verb][body]` → `[u32 len][i32 status][body]`; only the envelope
//! differs, which is exactly what the host KAT proves.

use crate::wire::{BODY_MAX, PRIN_RECORD_LEN};
use alloc::vec::Vec;

/// `b"UBS1"`.
pub const MAGIC: [u8; 4] = *b"UBS1";
/// Wire version.
pub const VERSION: u8 = 1;
/// Header bytes.
pub const HDR_LEN: usize = 52;
/// Frame kind: request.
pub const KIND_REQUEST: u8 = 1;
/// Frame kind: reply.
pub const KIND_REPLY: u8 = 2;
/// The BANDY3 register verb (una-abi `BUS_VERB_REGISTER`).
pub const VERB_REGISTER: u8 = 127;
/// The whole-frame ceiling.
pub const FRAME_MAX: usize = HDR_LEN + BODY_MAX;

/// A parsed frame header plus a borrowed body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    /// 1 request, 2 reply.
    pub kind: u8,
    /// Verb tag.
    pub verb: u8,
    /// Correlation id (a relay id on a relayed request).
    pub corr: u32,
    /// Status (0 or a negative errno).
    pub status: i32,
    /// The 32-byte principal record.
    pub principal: &'a [u8],
    /// The body.
    pub body: &'a [u8],
}

fn build(kind: u8, verb: u8, corr: u32, status: i32, body: &[u8]) -> Vec<u8> {
    let body = if status != 0 || body.len() > BODY_MAX { &[][..] } else { body };
    let mut f = alloc::vec![0u8; HDR_LEN + body.len()];
    f[0..4].copy_from_slice(&MAGIC);
    f[4] = VERSION;
    f[5] = kind;
    f[6] = verb;
    f[8..12].copy_from_slice(&corr.to_le_bytes());
    f[12..16].copy_from_slice(&status.to_le_bytes());
    f[48..52].copy_from_slice(&(body.len() as u32).to_le_bytes());
    f[HDR_LEN..].copy_from_slice(body);
    f
}

/// A ring-3 REQUEST frame: status 0, principal zero (the kernel stamps the sender). `None` when the body is
/// over [`BODY_MAX`].
pub fn request(verb: u8, corr: u32, body: &[u8]) -> Option<Vec<u8>> {
    (body.len() <= BODY_MAX).then(|| build(KIND_REQUEST, verb, corr, 0, body))
}

/// The REGISTER request for `verbs` (HOLOCRON.ELF sends it with [`crate::wire::VERBS`]).
pub fn register(corr: u32, verbs: &[u8]) -> Vec<u8> {
    build(KIND_REQUEST, VERB_REGISTER, corr, 0, verbs)
}

/// A fulfiller's REPLY to relay `corr`: principal zero (the kernel re-stamps), body only when `status == 0`.
pub fn reply(verb: u8, corr: u32, status: i32, body: &[u8]) -> Vec<u8> {
    build(KIND_REPLY, verb, corr, status, body)
}

/// Parse a frame (either kind). Fail-closed: wrong magic/version, a nonzero reserved byte, a body length
/// that disagrees with the bytes, or a body over [`BODY_MAX`] is `None`.
pub fn parse(b: &[u8]) -> Option<Frame<'_>> {
    if b.len() < HDR_LEN || b[0..4] != MAGIC || b[4] != VERSION || b[7] != 0 {
        return None;
    }
    if b[5] != KIND_REQUEST && b[5] != KIND_REPLY {
        return None;
    }
    let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let blen = u(48) as usize;
    if blen > BODY_MAX || HDR_LEN + blen != b.len() {
        return None;
    }
    Some(Frame { kind: b[5], verb: b[6], corr: u(8), status: u(12) as i32, principal: &b[16..16 + PRIN_RECORD_LEN], body: &b[HDR_LEN..] })
}

/// The relayed request's caller as Holocron compares it (`None` = not a user: DENIED by the service).
pub fn caller<'a>(f: &Frame<'a>) -> Option<&'a str> {
    crate::wire::principal_from_record(f.principal)
}

/// The host socket's request bytes for the same body (`[u32 len][u8 verb][body]`) — what
/// `handlers/holocron`'s client writes; the KAT's bridge.
pub fn host_request(verb: u8, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + body.len());
    v.extend_from_slice(&((1 + body.len()) as u32).to_le_bytes());
    v.push(verb);
    v.extend_from_slice(body);
    v
}

/// Parse the host socket's reply bytes (`[u32 len][i32 status][body]`) into `(status, body)`.
pub fn host_reply(b: &[u8]) -> Option<(i32, &[u8])> {
    let n = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as usize;
    if n < 4 || b.len() != 4 + n {
        return None;
    }
    Some((i32::from_le_bytes(b[4..8].try_into().ok()?), &b[8..]))
}
