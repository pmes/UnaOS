// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HOLOCRON2 M2 (rmbp-ledger B355): a ring-3 program asks Holocron for a credential FIRST — `SecretGet`
//! (144) over the BANDY bus, framed by `holocron_core::frame`, decided by `holocron_core::keysource::decide`
//! (the rule the host's `handlers/vein` applies) — and falls back to its old source (the key file) only when
//! Holocron has no such secret or no Holocron runs. LOCKED / DENIED / CORRUPT refuse, naming the fix: a key
//! that IS in the ring is never replaced by a stale file copy.
//!
//! "No fulfiller" and "no such secret" are the same errno on the wire (-ENOENT, the same decision); the
//! caller still wants to SAY which, so a `Status` (151) probe goes first: the kernel's own -ENOENT (or a
//! refused send: a kernel without `busreg`) means no Holocron.

use crate::sys::sys;
use holocron_core::frame;
use holocron_core::keysource::{self, Answer, KeySource};
use holocron_core::wire::{self, status, Request};
use holocron_core::zero::SecretBytes;
use una_abi::{BUS_FRAME_MAX, SYS_MRECV, SYS_MSEND};

static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
static mut CORR: u32 = 0x484F_0000;

/// One request, one reply: `(status, body)`, or `Err` when the bus refused the send.
fn call(r: &Request) -> Result<(i32, alloc::vec::Vec<u8>), i64> {
    let rx = unsafe { &mut *core::ptr::addr_of_mut!(RX) };
    let corr = unsafe {
        CORR = CORR.wrapping_add(1);
        CORR
    };
    let mut body = r.encode_body();
    let f = frame::request(r.verb(), corr, &body);
    holocron_core::zero::wipe(&mut body);
    let Some(mut f) = f else { return Err(una_abi::EINVAL) };
    let s = sys(SYS_MSEND, f.as_ptr() as u64, f.len() as u64, 0, 0);
    holocron_core::zero::wipe(&mut f);
    if s != 0 {
        return Err(s);
    }
    for _ in 0..8 {
        let n = sys(SYS_MRECV, rx.as_mut_ptr() as u64, rx.len() as u64, 0, 0);
        if n < 0 {
            return Err(n);
        }
        let Some(p) = frame::parse(&rx[..n as usize]) else { continue };
        if p.kind == frame::KIND_REPLY && p.corr == corr {
            let out = (p.status, p.body.to_vec());
            rx[..n as usize].fill(0);
            return Ok(out);
        }
    }
    Err(una_abi::EIO)
}

/// Where the key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyFrom {
    /// From Holocron: this many bytes in the caller's buffer.
    Holocron(usize),
    /// Use the old source; the reason names why (`no-fulfiller` / `not-found`).
    Fallback(&'static str),
    /// Use nothing; the reason names the fix.
    Refuse(&'static str),
}

/// Ask Holocron for `ns/name` into `out`, by the consumer rule.
pub fn key(ns: &str, name: &str, out: &mut [u8]) -> KeyFrom {
    match call(&Request::Status) {
        Err(_) => return KeyFrom::Fallback("no-fulfiller"),
        Ok((st, _)) if st == una_abi::ENOENT as i32 => return KeyFrom::Fallback("no-fulfiller"),
        Ok(_) => {}
    }
    let a = match call(&Request::Get { ns: ns.into(), name: name.into() }) {
        Ok((status::OK, b)) => Answer::Found(SecretBytes::new(b)),
        Ok((st, _)) => Answer::Status(st),
        Err(_) => Answer::Unavailable,
    };
    let why = match &a {
        Answer::Status(status::NOT_FOUND) => "not-found",
        _ => "no-fulfiller",
    };
    match keysource::decide(a) {
        KeySource::Holocron(b) => {
            let k = b.expose();
            if k.len() > out.len() {
                return KeyFrom::Refuse("the Holocron key is longer than this program's key buffer");
            }
            out[..k.len()].copy_from_slice(k);
            KeyFrom::Holocron(k.len())
        }
        KeySource::Fallback => KeyFrom::Fallback(why),
        KeySource::Refuse(r) => KeyFrom::Refuse(r),
    }
}

/// Vein's Claude API key (`vein/claude.api_key`).
pub fn claude_key(out: &mut [u8]) -> KeyFrom {
    key(keysource::VEIN_NS, keysource::CLAUDE_API_KEY, out)
}

const _: () = assert!(wire::VERB_STATUS == una_abi::BUS_VERB_HOLOCRON_LAST && wire::VERB_SECRET_GET == una_abi::BUS_VERB_HOLOCRON_FIRST);
