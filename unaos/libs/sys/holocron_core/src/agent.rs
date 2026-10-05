// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The SSH-agent shape: Holocron's Ed25519 keys presented over the SSH agent protocol, so `ssh`,
//! `ssh-add` and `git` use the ring as their agent (`SSH_AUTH_SOCK`).
//!
//! Specifications: draft-ietf-sshm-ssh-agent (formerly draft-miller-ssh-agent; the protocol OpenSSH
//! speaks) §3 framing, §4.4 REQUEST_IDENTITIES, §4.5 SIGN_REQUEST, §4.6 LOCK/UNLOCK; RFC 4251 §5 data
//! types (`uint32`, `string`); RFC 8709 §4 the `ssh-ed25519` public key blob and §6 the signature blob.
//!
//! ```text
//! message        = uint32 length, byte type, contents          (length counts type + contents)
//! 11 REQUEST_IDENTITIES  ()                     → 12 IDENTITIES_ANSWER uint32 n, n × (string blob, string comment)
//! 13 SIGN_REQUEST        string blob, string data, uint32 flags → 14 SIGN_RESPONSE string signature
//! 22 LOCK                string passphrase      → 6 SUCCESS | 5 FAILURE   (Holocron: Lock)
//! 23 UNLOCK              string passphrase      → 6 SUCCESS | 5 FAILURE   (Holocron: Unlock, rate-limited)
//! anything else (ADD_IDENTITY 17, REMOVE 18/19, EXTENSION 27 …) → 5 FAILURE: keys enter the ring
//! through SecretPut, never through the agent socket.
//! blob (RFC 8709 §4)      = string "ssh-ed25519", string A (32)
//! signature (RFC 8709 §6) = string "ssh-ed25519", string R||S (64)
//! ```

use crate::seal::{Entropy, Sealer, Signer};
use crate::service::{Holocron, Store};
use crate::wire::{self, status};
use alloc::string::String;
use alloc::vec::Vec;

/// SSH_AGENT_FAILURE.
pub const FAILURE: u8 = 5;
/// SSH_AGENT_SUCCESS.
pub const SUCCESS: u8 = 6;
/// SSH_AGENTC_REQUEST_IDENTITIES.
pub const REQUEST_IDENTITIES: u8 = 11;
/// SSH_AGENT_IDENTITIES_ANSWER.
pub const IDENTITIES_ANSWER: u8 = 12;
/// SSH_AGENTC_SIGN_REQUEST.
pub const SIGN_REQUEST: u8 = 13;
/// SSH_AGENT_SIGN_RESPONSE.
pub const SIGN_RESPONSE: u8 = 14;
/// SSH_AGENTC_LOCK.
pub const LOCK: u8 = 22;
/// SSH_AGENTC_UNLOCK.
pub const UNLOCK: u8 = 23;
/// The largest message accepted (OpenSSH's `AGENT_MAX_LEN`, 256 KiB).
pub const MAX_LEN: usize = 256 * 1024;
/// The key/signature algorithm name.
pub const ED25519: &str = "ssh-ed25519";
/// Suffix appended to a comment when the signer is the test signer.
pub const TEST_SUFFIX: &str = " [TEST-INSECURE]";

fn put_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn put_string(v: &mut Vec<u8>, s: &[u8]) {
    put_u32(v, s.len() as u32);
    v.extend_from_slice(s);
}

/// An RFC 4251 reader (big-endian).
struct R<'a> {
    b: &'a [u8],
    o: usize,
}
impl<'a> R<'a> {
    fn u32(&mut self) -> Option<u32> {
        let s = self.b.get(self.o..self.o.checked_add(4)?)?;
        self.o += 4;
        Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn string(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        let end = self.o.checked_add(n)?;
        let s = self.b.get(self.o..end)?;
        self.o = end;
        Some(s)
    }
    fn done(&self) -> bool {
        self.o == self.b.len()
    }
}

/// Prefix `msg` (type + contents) with its uint32 length.
pub fn frame(msg: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + msg.len());
    put_u32(&mut v, msg.len() as u32);
    v.extend_from_slice(msg);
    v
}

/// The RFC 8709 §4 public key blob.
pub fn ed25519_blob(public: &[u8; 32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(51);
    put_string(&mut v, ED25519.as_bytes());
    put_string(&mut v, public);
    v
}

/// Parse an RFC 8709 §4 blob.
pub fn parse_ed25519_blob(blob: &[u8]) -> Option<[u8; 32]> {
    let mut r = R { b: blob, o: 0 };
    if r.string()? != ED25519.as_bytes() {
        return None;
    }
    let a = r.string()?;
    if a.len() != 32 || !r.done() {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(a);
    Some(out)
}

/// The RFC 8709 §6 signature blob.
pub fn ed25519_signature(sig: &[u8; 64]) -> Vec<u8> {
    let mut v = Vec::with_capacity(83);
    put_string(&mut v, ED25519.as_bytes());
    put_string(&mut v, sig);
    v
}

/// A parsed request.
#[allow(missing_docs)]
#[derive(Debug, PartialEq, Eq)]
pub enum AgentRequest<'a> {
    /// 11.
    RequestIdentities,
    /// 13.
    Sign { blob: &'a [u8], data: &'a [u8], flags: u32 },
    /// 22.
    Lock { passphrase: &'a [u8] },
    /// 23.
    Unlock { passphrase: &'a [u8] },
    /// Any other type, or a malformed body.
    Unsupported(u8),
}

/// Parse one message body (type + contents, the length already stripped).
pub fn parse_request(msg: &[u8]) -> AgentRequest<'_> {
    let Some((&ty, rest)) = msg.split_first() else { return AgentRequest::Unsupported(0) };
    let mut r = R { b: rest, o: 0 };
    let parsed = match ty {
        REQUEST_IDENTITIES => r.done().then_some(AgentRequest::RequestIdentities),
        SIGN_REQUEST => (|| {
            let blob = r.string()?;
            let data = r.string()?;
            let flags = r.u32()?;
            r.done().then_some(AgentRequest::Sign { blob, data, flags })
        })(),
        LOCK => r.string().filter(|_| r.done()).map(|p| AgentRequest::Lock { passphrase: p }),
        UNLOCK => r.string().filter(|_| r.done()).map(|p| AgentRequest::Unlock { passphrase: p }),
        _ => None,
    };
    parsed.unwrap_or(AgentRequest::Unsupported(ty))
}

/// IDENTITIES_ANSWER for `(blob, comment)` pairs.
pub fn identities_answer(ids: &[(Vec<u8>, String)]) -> Vec<u8> {
    let mut v = alloc::vec![IDENTITIES_ANSWER];
    put_u32(&mut v, ids.len() as u32);
    for (blob, comment) in ids {
        put_string(&mut v, blob);
        put_string(&mut v, comment.as_bytes());
    }
    v
}

/// SIGN_RESPONSE carrying the RFC 8709 signature blob.
pub fn sign_response(sig: &[u8; 64]) -> Vec<u8> {
    let mut v = alloc::vec![SIGN_RESPONSE];
    put_string(&mut v, &ed25519_signature(sig));
    v
}

/// Answer one agent message for `caller` (the transport-stamped principal). Returns the reply message
/// (type + contents) — the caller frames it. Every path goes through the same owner check as the bus.
pub fn handle<S: Sealer, G: Signer, T: Store, E: Entropy>(
    h: &mut Holocron<S, G, T, E>,
    caller: Option<&str>,
    msg: &[u8],
    now_ms: u64,
    now_unix: i64,
) -> Vec<u8> {
    let fail = alloc::vec![FAILURE];
    match parse_request(msg) {
        AgentRequest::RequestIdentities => match h.identities(caller) {
            Ok(ids) => {
                let suffix = if h.signer_is_real() { "" } else { TEST_SUFFIX };
                let pairs: Vec<(Vec<u8>, String)> = ids
                    .iter()
                    .map(|i| (ed25519_blob(&i.public), alloc::format!("{}{}", i.label, suffix)))
                    .collect();
                identities_answer(&pairs)
            }
            Err(_) => fail,
        },
        AgentRequest::Sign { blob, data, .. } => {
            let Some(public) = parse_ed25519_blob(blob) else { return fail };
            match h.sign_by_public(caller, &public, data) {
                Ok(sig) => sign_response(&sig),
                Err(_) => fail,
            }
        }
        AgentRequest::Lock { .. } => {
            let r = h.handle(caller, wire::VERB_LOCK, &[], now_ms, now_unix);
            if r.status == status::OK { alloc::vec![SUCCESS] } else { fail }
        }
        AgentRequest::Unlock { passphrase } => {
            let body = wire::Request::Unlock { create: false, password: passphrase.to_vec() }.encode_body();
            let mut body = body;
            let r = h.handle(caller, wire::VERB_UNLOCK, &body, now_ms, now_unix);
            crate::zero::wipe(&mut body);
            if r.status == status::OK { alloc::vec![SUCCESS] } else { fail }
        }
        AgentRequest::Unsupported(_) => fail,
    }
}
