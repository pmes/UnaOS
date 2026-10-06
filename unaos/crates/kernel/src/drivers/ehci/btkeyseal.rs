// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Holocron — shared-core (the kernel is a CLIENT of Holocron's verbs; the seal is holocron_core's, run by APPS/HOLOCRON.ELF)
//!
//! BTKEYSEAL (rmbp-ledger B446, ARCHREVIEW F2) — the Bluetooth bond store's link keys as Holocron records.
//!
//! A BR/EDR link key is a pairing secret: whoever reads it impersonates the keyboard. It is Holocron's
//! (CODEX §2, R79), so the bond store never writes it: it ASKS Holocron — SecretPut / SecretGet /
//! SecretDelete / SecretList (verbs 145 / 144 / 147 / 146) on the record `bt/<addr12>`, kind `bt.linkkey`,
//! data the 16 key bytes. The body is built by `holocron_core::wire::Request` (the one codec both rings
//! link); the frame is relayed by `bus_route` from the kernel-client row (`prefs_client::relay_tag`)
//! stamped with the SESSION USER'S principal `user:<name>#<uid>` — the owner Holocron compares, built here
//! from the session table, never claimed. HOLOCRON.ELF seals under the user's ring (Argon2id ring key, a
//! fresh HKDF file key and ChaCha20-Poly1305 per write) and stores where its `PathStore` stores.
//!
//! Nothing here keeps a key: a request's body is wiped by `Request`'s `Drop`, the frame by `relay_tag`.
//! Key bytes never reach serial. Design: docs/dev/evidence/rmbp-1005/btkeyseal.md.

use alloc::vec::Vec;

/// Holocron's namespace for Bluetooth bonds.
pub const NS: &str = "bt";
/// The record kind.
pub const KIND: &str = una_abi::attr_keys::BT_LINKKEY;
/// How long one storage pass waits for HOLOCRON.ELF's answer.
pub const WAIT_MS: u64 = 250;

/// What Holocron answered.
pub enum Answer {
    /// OK, with the reply body (a Get's key bytes; caller wipes).
    Ok(Vec<u8>),
    /// `NOT_FOUND`: no such record.
    NotFound,
    /// Holocron answered with another status (locked, no ring, denied, corrupt, ...): its name.
    Refused(&'static str),
    /// Nobody to ask (no session, no HOLOCRON.ELF registered, the relay full or timed out): why.
    Unavailable(&'static str),
}

impl Answer {
    /// The short reason word the witnesses print.
    pub fn reason(&self) -> &'static str {
        match self {
            Answer::Ok(_) => "ok",
            Answer::NotFound => "not-found",
            Answer::Refused(r) | Answer::Unavailable(r) => r,
        }
    }
}

/// Seal `key` as `bt/<addr12>`.
pub fn put(addr12: &str, label: &str, key: &[u8; 16]) -> Answer {
    #[cfg(all(feature = "lumen", feature = "busreg", feature = "login"))]
    {
        let mut lb = alloc::string::String::from(label);
        lb.truncate(64);
        let req = holocron_core::wire::Request::Put { ns: NS.into(), name: addr12.into(), kind: KIND.into(), label: lb, data: key.to_vec() };
        return call(&req);
    }
    #[cfg(not(all(feature = "lumen", feature = "busreg", feature = "login")))]
    {
        let _ = (addr12, label, key);
        Answer::Unavailable("no-holocron-in-image")
    }
}

/// Open `bt/<addr12>`: the 16 key bytes, or why not.
pub fn get(addr12: &str) -> Result<[u8; 16], Answer> {
    #[cfg(all(feature = "lumen", feature = "busreg", feature = "login"))]
    {
        let req = holocron_core::wire::Request::Get { ns: NS.into(), name: addr12.into() };
        return match call(&req) {
            Answer::Ok(mut b) => {
                let r = if b.len() == 16 {
                    let mut k = [0u8; 16];
                    k.copy_from_slice(&b);
                    Ok(k)
                } else {
                    Err(Answer::Refused("bad-length"))
                };
                holocron_core::zero::wipe(&mut b);
                r
            }
            other => Err(other),
        };
    }
    #[cfg(not(all(feature = "lumen", feature = "busreg", feature = "login")))]
    {
        let _ = addr12;
        Err(Answer::Unavailable("no-holocron-in-image"))
    }
}

/// Delete `bt/<addr12>` (a `bt forget`).
pub fn delete(addr12: &str) -> Answer {
    #[cfg(all(feature = "lumen", feature = "busreg", feature = "login"))]
    {
        let req = holocron_core::wire::Request::Delete { ns: NS.into(), name: addr12.into() };
        return call(&req);
    }
    #[cfg(not(all(feature = "lumen", feature = "busreg", feature = "login")))]
    {
        let _ = addr12;
        Answer::Unavailable("no-holocron-in-image")
    }
}

/// How many records the `bt` namespace holds (SecretList), or why not.
pub fn count() -> Result<usize, Answer> {
    #[cfg(all(feature = "lumen", feature = "busreg", feature = "login"))]
    {
        let req = holocron_core::wire::Request::List { ns: NS.into() };
        return match call(&req) {
            Answer::Ok(b) => holocron_core::wire::decode_list(&b).map(|v| v.len()).ok_or(Answer::Refused("bad-list")),
            other => Err(other),
        };
    }
    #[cfg(not(all(feature = "lumen", feature = "busreg", feature = "login")))]
    Err(Answer::Unavailable("no-holocron-in-image"))
}

/// The record name and a Put body round-trip through the one codec (`tests btkeyseal`'s codec leg).
pub fn codec_ok() -> bool {
    #[cfg(feature = "lumen")]
    {
        use holocron_core::wire::Request;
        let req = Request::Put { ns: NS.into(), name: "a1b2c3d4e5f6".into(), kind: KIND.into(), label: "fixture".into(), data: alloc::vec![0x5a; 16] };
        let body = req.encode_body();
        let back = Request::decode(req.verb(), &body);
        return holocron_core::name::valid(NS) && holocron_core::name::valid("a1b2c3d4e5f6") && back.as_ref() == Ok(&req);
    }
    #[cfg(not(feature = "lumen"))]
    false
}

/// The session user's principal record (kind 5, `user:<name>#<uid>`), the stamp HOLOCRON.ELF compares.
#[cfg(all(feature = "lumen", feature = "busreg", feature = "login"))]
fn principal() -> Option<[u8; 32]> {
    let mut nm = [0u8; crate::fs::users::NAME_MAX];
    let n = crate::fs::users::whoami(&mut nm)?;
    let name = core::str::from_utf8(&nm[..n]).ok()?;
    let uid = crate::fs::users::id_of(&nm[..n])?;
    let s = holocron_core::wire::user_principal(name, uid);
    if s.len() > 30 {
        return None;
    }
    let mut p = [0u8; 32];
    p[0] = 5; // PRIN_USER
    p[1] = s.len() as u8;
    p[2..2 + s.len()].copy_from_slice(s.as_bytes());
    Some(p)
}

#[cfg(all(feature = "lumen", feature = "busreg", feature = "login"))]
fn call(req: &holocron_core::wire::Request) -> Answer {
    use holocron_core::wire::status as st;
    let Some(prin) = principal() else { return Answer::Unavailable("no-session") };
    let mut body = req.encode_body();
    let r = crate::prefs_client::relay_tag(req.verb(), prin, &body, WAIT_MS);
    holocron_core::zero::wipe(&mut body);
    match r {
        Ok((0, b)) => Answer::Ok(b),
        Ok((s, _)) => match s {
            st::NOT_FOUND => Answer::NotFound,
            st::LOCKED => Answer::Refused("locked"),
            st::NO_RING => Answer::Refused("no-ring"),
            st::DENIED => Answer::Refused("denied"),
            st::CORRUPT => Answer::Refused("corrupt"),
            st::IO => Answer::Refused("io"),
            _ => Answer::Refused("refused"),
        },
        Err(-2) => Answer::Unavailable("no-holocron"),
        Err(-11) => Answer::Unavailable("busy"),
        Err(-110) => Answer::Unavailable("timeout"),
        Err(_) => Answer::Unavailable("relay-refused"),
    }
}
