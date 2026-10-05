// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The bus surface: verb tags, request bodies, reply statuses and bodies. ONE codec for every transport:
//! on the metal a body rides a BANDY v1 frame (52-byte header, kernel-stamped principal, verb in the
//! header, 4 KiB body ceiling — BANDY3); on the host it rides the Holocron Unix socket
//! (`[u32 len][u8 verb][body]` → `[u32 len][i32 status][body]`, principal from SO_PEERCRED). The
//! transport supplies the caller's principal; nothing in a body ever claims one.
//!
//! ## Verbs (BANDY3's registrable range, `>= BUS_VERB_FULFIL_MIN` = 128; eight = `BUS_REG_MAX_PER_ROW`)
//!
//! | tag | verb | request body | OK reply body |
//! |---|---|---|---|
//! | 144 | SecretGet | `str8 ns, str8 name` | the secret bytes |
//! | 145 | SecretPut | `str8 ns, str8 name, str8 kind, str8 label, bytes16 data` | empty |
//! | 146 | SecretList | `str8 ns` | repeated `str8 name, str8 kind, str8 label, i64 created` |
//! | 147 | SecretDelete | `str8 ns, str8 name` | empty |
//! | 148 | Unlock | `u8 flags (bit0 create), bytes16 password` | empty |
//! | 149 | Lock | empty | empty |
//! | 150 | Sign | `str8 key name (ns "ssh"), bytes16 data` | 64-byte Ed25519 signature |
//! | 151 | Status | empty | `u8 state (0 no ring, 1 locked, 2 unlocked), u8 suite, str8 owner` |
//!
//! `str8` = `u8 len, bytes`; `bytes16` = `u16 len (LE), bytes`. A body with trailing bytes is EINVAL.
//! An error reply is its status and NO body (the BANDY v1 frozen error-reply rule).

use alloc::string::String;
use alloc::vec::Vec;

/// SecretGet.
pub const VERB_SECRET_GET: u8 = 144;
/// SecretPut.
pub const VERB_SECRET_PUT: u8 = 145;
/// SecretList.
pub const VERB_SECRET_LIST: u8 = 146;
/// SecretDelete.
pub const VERB_SECRET_DELETE: u8 = 147;
/// Unlock.
pub const VERB_UNLOCK: u8 = 148;
/// Lock.
pub const VERB_LOCK: u8 = 149;
/// Sign.
pub const VERB_SIGN: u8 = 150;
/// Status.
pub const VERB_STATUS: u8 = 151;
/// Every Holocron verb — the body of HOLOCRON.ELF's BUS_VERB_REGISTER frame on the metal.
pub const VERBS: [u8; 8] = [
    VERB_SECRET_GET,
    VERB_SECRET_PUT,
    VERB_SECRET_LIST,
    VERB_SECRET_DELETE,
    VERB_UNLOCK,
    VERB_LOCK,
    VERB_SIGN,
    VERB_STATUS,
];
/// The BANDY v1 body ceiling.
pub const BODY_MAX: usize = 4096;

/// Reply statuses: 0 or a negative errno with una-abi's numbering.
pub mod status {
    /// Success.
    pub const OK: i32 = 0;
    /// EPERM: the password did not open the ring.
    pub const BAD_PASSWORD: i32 = -1;
    /// ENOENT: no such secret (the ONLY answer a consumer may fall back on).
    pub const NOT_FOUND: i32 = -2;
    /// EIO: the store failed, or the entropy source did (nothing was sealed or minted).
    pub const IO: i32 = -5;
    /// EAGAIN: unlock attempts are rate-limited; retry later.
    pub const RATE_LIMITED: i32 = -11;
    /// EACCES: the caller's principal is not the ring's owner.
    pub const DENIED: i32 = -13;
    /// EEXIST: create asked for, but a ring exists.
    pub const EXISTS: i32 = -17;
    /// ENODEV: no ring has been created for this user.
    pub const NO_RING: i32 = -19;
    /// EINVAL: malformed body, name or argument.
    pub const INVALID: i32 = -22;
    /// EFBIG: the secret exceeds `SECRET_MAX`.
    pub const TOO_BIG: i32 = -27;
    /// ENOLCK: the ring is locked.
    pub const LOCKED: i32 = -37;
    /// EBADMSG: a stored file failed to parse or authenticate.
    pub const CORRUPT: i32 = -74;
    /// ENOSYS: unknown verb.
    pub const NO_VERB: i32 = -38;
}

/// A decoded request (fields as in the module table).
#[allow(missing_docs)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// SecretGet.
    Get { ns: String, name: String },
    /// SecretPut.
    Put { ns: String, name: String, kind: String, label: String, data: Vec<u8> },
    /// SecretList.
    List { ns: String },
    /// SecretDelete.
    Delete { ns: String, name: String },
    /// Unlock (or create, with `create`).
    Unlock { create: bool, password: Vec<u8> },
    /// Lock.
    Lock,
    /// Sign with the Ed25519 key `ssh/<key>`.
    Sign { key: String, data: Vec<u8> },
    /// Status.
    Status,
}

impl Drop for Request {
    fn drop(&mut self) {
        match self {
            Request::Put { data, .. } | Request::Sign { data, .. } => crate::zero::wipe(data),
            Request::Unlock { password, .. } => crate::zero::wipe(password),
            _ => {}
        }
    }
}

fn put_str8(v: &mut Vec<u8>, s: &str) {
    v.push(s.len().min(255) as u8);
    v.extend_from_slice(&s.as_bytes()[..s.len().min(255)]);
}
fn put_bytes16(v: &mut Vec<u8>, b: &[u8]) {
    let n = b.len().min(u16::MAX as usize);
    v.extend_from_slice(&(n as u16).to_le_bytes());
    v.extend_from_slice(&b[..n]);
}

/// A bounds-checked body reader.
pub struct Reader<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> Reader<'a> {
    /// Read from `b`.
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, o: 0 }
    }
    /// One byte.
    pub fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.o)?;
        self.o += 1;
        Some(v)
    }
    /// `n` bytes.
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.o.checked_add(n)?;
        let s = self.b.get(self.o..end)?;
        self.o = end;
        Some(s)
    }
    /// A `str8` (UTF-8).
    pub fn str8(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        core::str::from_utf8(self.take(n)?).ok().map(String::from)
    }
    /// A `bytes16`.
    pub fn bytes16(&mut self) -> Option<Vec<u8>> {
        let n = u16::from_le_bytes([self.u8()?, self.u8()?]) as usize;
        self.take(n).map(<[u8]>::to_vec)
    }
    /// A little-endian i64.
    pub fn i64(&mut self) -> Option<i64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(i64::from_le_bytes(a))
    }
    /// True when every byte was consumed.
    pub fn done(&self) -> bool {
        self.o == self.b.len()
    }
}

impl Request {
    /// The verb tag.
    pub fn verb(&self) -> u8 {
        match self {
            Request::Get { .. } => VERB_SECRET_GET,
            Request::Put { .. } => VERB_SECRET_PUT,
            Request::List { .. } => VERB_SECRET_LIST,
            Request::Delete { .. } => VERB_SECRET_DELETE,
            Request::Unlock { .. } => VERB_UNLOCK,
            Request::Lock => VERB_LOCK,
            Request::Sign { .. } => VERB_SIGN,
            Request::Status => VERB_STATUS,
        }
    }

    /// Encode the body.
    pub fn encode_body(&self) -> Vec<u8> {
        let mut v = Vec::new();
        match self {
            Request::Get { ns, name } | Request::Delete { ns, name } => {
                put_str8(&mut v, ns);
                put_str8(&mut v, name);
            }
            Request::Put { ns, name, kind, label, data } => {
                put_str8(&mut v, ns);
                put_str8(&mut v, name);
                put_str8(&mut v, kind);
                put_str8(&mut v, label);
                put_bytes16(&mut v, data);
            }
            Request::List { ns } => put_str8(&mut v, ns),
            Request::Unlock { create, password } => {
                v.push(*create as u8);
                put_bytes16(&mut v, password);
            }
            Request::Lock | Request::Status => {}
            Request::Sign { key, data } => {
                put_str8(&mut v, key);
                put_bytes16(&mut v, data);
            }
        }
        v
    }

    /// Decode `body` for `verb`. `Err` carries the status to reply with.
    pub fn decode(verb: u8, body: &[u8]) -> Result<Request, i32> {
        if body.len() > BODY_MAX {
            return Err(status::INVALID);
        }
        let mut r = Reader::new(body);
        let inv = status::INVALID;
        let req = match verb {
            VERB_SECRET_GET => Request::Get { ns: r.str8().ok_or(inv)?, name: r.str8().ok_or(inv)? },
            VERB_SECRET_DELETE => Request::Delete { ns: r.str8().ok_or(inv)?, name: r.str8().ok_or(inv)? },
            VERB_SECRET_PUT => Request::Put {
                ns: r.str8().ok_or(inv)?,
                name: r.str8().ok_or(inv)?,
                kind: r.str8().ok_or(inv)?,
                label: r.str8().ok_or(inv)?,
                data: r.bytes16().ok_or(inv)?,
            },
            VERB_SECRET_LIST => Request::List { ns: r.str8().ok_or(inv)? },
            VERB_UNLOCK => {
                let flags = r.u8().ok_or(inv)?;
                if flags & !1 != 0 {
                    return Err(inv);
                }
                Request::Unlock { create: flags & 1 == 1, password: r.bytes16().ok_or(inv)? }
            }
            VERB_LOCK => Request::Lock,
            VERB_SIGN => Request::Sign { key: r.str8().ok_or(inv)?, data: r.bytes16().ok_or(inv)? },
            VERB_STATUS => Request::Status,
            _ => return Err(status::NO_VERB),
        };
        if !r.done() {
            return Err(inv);
        }
        Ok(req)
    }
}

/// A reply: a status and (only when OK) a body.
#[derive(Clone, PartialEq, Eq)]
pub struct Reply {
    /// 0 or a negative errno ([`status`]).
    pub status: i32,
    /// The body (empty on error).
    pub body: Vec<u8>,
}

impl core::fmt::Debug for Reply {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Reply {{ status: {}, body: <{} bytes> }}", self.status, self.body.len())
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        crate::zero::wipe(&mut self.body);
    }
}

impl Reply {
    /// An OK reply carrying `body`.
    pub fn ok(body: Vec<u8>) -> Self {
        Reply { status: status::OK, body }
    }
    /// An error reply: the status and no body.
    pub fn err(status: i32) -> Self {
        Reply { status, body: Vec::new() }
    }
}

/// One `SecretList` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListEntry {
    /// Secret name.
    pub name: String,
    /// Metadata.
    pub meta: crate::format::Meta,
}

/// Encode a `SecretList` reply body.
pub fn encode_list(entries: &[ListEntry]) -> Vec<u8> {
    let mut v = Vec::new();
    for e in entries {
        put_str8(&mut v, &e.name);
        put_str8(&mut v, &e.meta.kind);
        put_str8(&mut v, &e.meta.label);
        v.extend_from_slice(&e.meta.created.to_le_bytes());
    }
    v
}

/// Decode a `SecretList` reply body.
pub fn decode_list(body: &[u8]) -> Option<Vec<ListEntry>> {
    let mut r = Reader::new(body);
    let mut out = Vec::new();
    while !r.done() {
        let name = r.str8()?;
        let kind = r.str8()?;
        let label = r.str8()?;
        let created = r.i64()?;
        out.push(ListEntry { name, meta: crate::format::Meta { created, kind, label } });
    }
    Some(out)
}

/// `Status` reply: ring state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingState {
    /// No ring file.
    NoRing = 0,
    /// Ring file present, key not in memory.
    Locked = 1,
    /// Key in memory.
    Unlocked = 2,
}

/// Decode a `Status` reply body: `(state, suite, owner)`.
pub fn decode_status(body: &[u8]) -> Option<(RingState, u8, String)> {
    let mut r = Reader::new(body);
    let st = match r.u8()? {
        0 => RingState::NoRing,
        1 => RingState::Locked,
        2 => RingState::Unlocked,
        _ => return None,
    };
    let suite = r.u8()?;
    let owner = r.str8()?;
    r.done().then_some((st, suite, owner))
}

/// Encode a `Status` reply body.
pub fn encode_status(state: RingState, suite: u8, owner: &str) -> Vec<u8> {
    let mut v = alloc::vec![state as u8, suite];
    put_str8(&mut v, owner);
    v
}

// ---- principals --------------------------------------------------------------------------------------

/// BANDY v1 principal record kind for a human user (`PRIN_USER`, kernel LOGIN M1): value `user:<name>#<uid>`.
pub const PRIN_USER: u8 = 5;
/// Bytes of a BANDY v1 principal record: `kind u8, len u8, value [u8; 30]`.
pub const PRIN_RECORD_LEN: usize = 32;

/// Project a kernel-stamped principal record to the canonical string Holocron compares. ONLY a
/// `PRIN_USER` record projects; every other kind (a program digest, x86's `row:<r>/gen:<g>` relay stamp,
/// the kernel reply record, none) is `None` — fail-closed: such a caller is never the ring's owner.
pub fn principal_from_record(rec: &[u8]) -> Option<&str> {
    if rec.len() != PRIN_RECORD_LEN || rec[0] != PRIN_USER {
        return None;
    }
    let n = rec[1] as usize;
    if n == 0 || n > PRIN_RECORD_LEN - 2 {
        return None;
    }
    let s = core::str::from_utf8(&rec[2..2 + n]).ok()?;
    s.starts_with("user:").then_some(s)
}

/// The canonical user principal `user:<name>#<uid>` (SECLOGIN M2), as both arches' kernels mint it.
pub fn user_principal(name: &str, uid: u32) -> String {
    alloc::format!("user:{name}#{uid}")
}
