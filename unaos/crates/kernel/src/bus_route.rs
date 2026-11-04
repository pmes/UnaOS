//! CHARTER: Kernel — fulfiller
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BANDY3 (ROADMAP §3b, audit B289): FULFILLER REGISTRATION ON THE WIRE — a ring-3 program owns a verb.
//
// The design of record is docs/dev/evidence/rmbp-1001/BANDY3.md §M1. In one paragraph: the verb space
// splits at 128 (`una_abi::BUS_VERB_FULFIL_MIN`). Tags below it are the KERNEL's — a REGISTER of one is
// `-EEXIST`, kernel fulfilment wins, ls/cat/cp/write/rm/mv are untouched. A row registers up to 8 tags
// at or above it. A caller's REQUEST for a registered tag is re-built by the kernel into the
// fulfiller's mailbox with `corr` = a kernel relay id and `principal` = the CALLER's kernel stamp; the
// fulfiller answers with a REPLY frame echoing that relay id, which the kernel re-builds into the
// caller's mailbox with the caller's own `corr` and the RESERVED KERNEL principal. No registration ->
// `-ENOENT` to the caller; a full mailbox or table -> `-EAGAIN`; the fulfiller exits -> every caller it
// owed gets `-ECONNRESET`. Nothing here ever blocks: every refusal is an errno.
//
// THE PRINCIPAL RULE. The fulfiller SEES the caller's principal in the relayed header; it never ACTS
// as the caller. Whatever it touches, it touches through ordinary syscalls under its OWN grants — a
// fulfiller is a service, and authorisation stays per-object in the kernel.
//
// ARCH-NEUTRAL. The two arches key their mailboxes differently (aarch64: ASID + ASID_GEN; x86: HANDLES
// row + SLOT_GEN) and own them privately; each hands this module an `Ops` table of three functions
// over its own mailbox, defined at the tail of its syscall.rs. Everything else — the tables, the relay,
// the reply, the exit sweep, the codec KATs, the `tests bandy3` fixture's verdict — lives here once.
//
// AI-OFF: nothing registers by default. The module only exists under feature `busreg`
// (`UNAOS_BUSREG=1`); off, `bus::verb_valid` refuses tags 127 and 128..=255 exactly as before.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicU32, Ordering};

use una_abi::{BUS_REG_MAX_PER_ROW, BUS_VERB_FULFIL_MIN, BUS_VERB_REGISTER};

/// Total registrations across all rows.
pub const REG_CAP: usize = 32;
/// Total relays in flight.
pub const PEND_CAP: usize = 32;
/// Relays in flight per CALLER row — one caller cannot starve the table for the rest.
pub const PEND_MAX_PER_CALLER: usize = 8;

const EINVAL: i64 = -22;
const ENOENT: i64 = -2;
const EAGAIN: i64 = -11;
const EEXIST: i64 = una_abi::EEXIST;
const ENOSPC: i64 = una_abi::ENOSPC;
const ECONNRESET: i64 = una_abi::ECONNRESET;

/// The reserved KERNEL reply principal kind (aarch64 PRIN_KERNEL_REPLY / x86 BUSX_PRIN_KERNEL_REPLY).
const PRIN_KERNEL_REPLY: u8 = 4;
/// The reserved launcher-minted kind, used for x86's `(row, SLOT_GEN)` identity on a relayed header.
const PRIN_KERNEL_PID: u8 = 3;

/// One arch's mailbox, as this module needs it. `row` is the arch's mailbox key (ASID / HANDLES row).
pub struct Ops {
    /// Enqueue `frame` for `row`'s CURRENT tenant and wake its MRECV. `false` = full.
    pub push: fn(row: usize, frame: Box<[u8]>) -> bool,
    /// Room for one more frame in `row`'s mailbox.
    pub has_room: fn(row: usize) -> bool,
    /// `row`'s current generation (bumped on teardown).
    pub rgen: fn(row: usize) -> u64,
}

#[derive(Clone, Copy)]
struct Reg {
    verb: u8,
    row: usize,
    rgen: u64,
}

#[derive(Clone, Copy)]
struct Pend {
    relay: u32,
    verb: u8,
    caller_row: usize,
    caller_gen: u64,
    caller_corr: u32,
    ful_row: usize,
    ful_gen: u64,
}

static REGS: crate::sync::Mutex<[Option<Reg>; REG_CAP]> = crate::sync::Mutex::new([None; REG_CAP]);
static PEND: crate::sync::Mutex<[Option<Pend>; PEND_CAP]> = crate::sync::Mutex::new([None; PEND_CAP]);
static NEXT_RELAY: AtomicU32 = AtomicU32::new(1);

/// Witness counters (monotonic since boot).
pub static REGISTERED: AtomicU32 = AtomicU32::new(0);
pub static RELAYED: AtomicU32 = AtomicU32::new(0);
pub static REPLIED: AtomicU32 = AtomicU32::new(0);
pub static ORPHAN: AtomicU32 = AtomicU32::new(0);
/// Non-final (BUS_STATUS_MORE) frames of multi-frame answers relayed.
pub static MORE: AtomicU32 = AtomicU32::new(0);

/// What the dispatcher does with a REQUEST after `route_request`.
pub enum Route {
    /// A kernel-owned verb: fall through to the in-kernel fulfiller, unchanged.
    Kernel,
    /// Answer the caller now with this status (empty body) — the REGISTER verdict, `-ENOENT`, `-EAGAIN`.
    Reply(i64),
    /// Relayed to a fulfiller: `SYS_MSEND` returns 0 and the answer arrives later.
    Relayed,
}

#[inline]
fn locked<T>(f: impl FnOnce() -> T) -> T {
    crate::arch::without_interrupts(f)
}

/// The x86 relayed-header identity: kind 3 (launcher-minted), value `row:<r>/gen:<g>` — x86's U6
/// identity IS `(row, SLOT_GEN)` (BUSX86 note 1); it has no `PrincipalRecord` to project.
pub fn row_principal(row: usize, rgen: u64) -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0] = PRIN_KERNEL_PID;
    let mut n = 0usize;
    let put = |b: &[u8], p: &mut [u8; 32], n: &mut usize| {
        for &c in b {
            if *n < 30 {
                p[2 + *n] = c;
                *n += 1;
            }
        }
    };
    let mut dec = [0u8; 20];
    put(b"row:", &mut p, &mut n);
    let d = fmt_dec(row as u64, &mut dec);
    put(d, &mut p, &mut n);
    put(b"/gen:", &mut p, &mut n);
    let mut dec2 = [0u8; 20];
    let d2 = fmt_dec(rgen, &mut dec2);
    put(d2, &mut p, &mut n);
    p[1] = n as u8;
    p
}

fn fmt_dec(mut v: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    if v == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    &buf[i..]
}

fn kernel_reply_principal() -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0] = PRIN_KERNEL_REPLY;
    p
}

/// The live fulfiller of `verb`, if any (a registration whose row generation has moved on is dead).
fn lookup(ops: &Ops, verb: u8) -> Option<(usize, u64)> {
    let hit = locked(|| REGS.lock().iter().flatten().find(|r| r.verb == verb).copied());
    match hit {
        Some(r) if (ops.rgen)(r.row) == r.rgen => Some((r.row, r.rgen)),
        _ => None,
    }
}

/// REGISTER: all-or-nothing. Returns the reply status.
pub fn register(ops: &Ops, row: usize, body: &[u8]) -> i64 {
    if body.is_empty() || body.len() > BUS_REG_MAX_PER_ROW {
        return EINVAL;
    }
    for (i, &v) in body.iter().enumerate() {
        if body[..i].contains(&v) {
            return EINVAL;
        }
    }
    if body.iter().any(|&v| v < BUS_VERB_FULFIL_MIN) {
        return EEXIST; // the kernel already fulfils it — kernel fulfilment wins, in v1 always
    }
    if let Some(st) = tagid_register_gate(ops, row, body) { return st; } // BUSREG-TAG-IDENTITY (B1157): a private tag is registered only by the image the kernel names for it (file tail)
    let rgen = (ops.rgen)(row);
    locked(|| {
        let mut t = REGS.lock();
        // Reap dead registrations (a row whose generation moved on). `ops.rgen` is an atomic load — no lock.
        for s in t.iter_mut() {
            if s.is_some_and(|r| (ops.rgen)(r.row) != r.rgen) {
                *s = None;
            }
        }
        let mut new = 0usize;
        for &v in body {
            match t.iter().flatten().find(|r| r.verb == v) {
                Some(r) if r.row == row && r.rgen == rgen => {} // idempotent
                Some(_) => return EEXIST,
                None => new += 1,
            }
        }
        let held = t.iter().flatten().filter(|r| r.row == row && r.rgen == rgen).count();
        if held + new > BUS_REG_MAX_PER_ROW {
            return ENOSPC;
        }
        if t.iter().filter(|s| s.is_none()).count() < new {
            return ENOSPC;
        }
        for &v in body {
            if t.iter().flatten().any(|r| r.verb == v) {
                continue;
            }
            if let Some(s) = t.iter_mut().find(|s| s.is_none()) {
                *s = Some(Reg { verb: v, row, rgen });
            }
        }
        REGISTERED.fetch_add(new as u32, Ordering::Relaxed);
        0
    })
}

/// The REQUEST-side hook. `caller_prin` is the CALLER's kernel stamp (the 32-byte wire image).
pub fn route_request(ops: &Ops, caller_row: usize, caller_gen: u64, caller_prin: [u8; 32], hdr: &crate::bus::BusHdr, body: &[u8]) -> Route {
    if hdr.verb == BUS_VERB_REGISTER {
        return Route::Reply(register(ops, caller_row, body));
    }
    if hdr.verb < BUS_VERB_FULFIL_MIN {
        return Route::Kernel;
    }
    if let Some(st) = pref_gate(hdr.verb, caller_row) {
        return Route::Reply(st); // PREFSCAP (B454): the deputy door is shut
    }
    let Some((ful_row, ful_gen)) = lookup(ops, hdr.verb) else {
        return Route::Reply(ENOENT); // no fulfiller: an answer, never a hang
    };
    if let Some(st) = tagid_relay_gate(hdr.verb, ful_row, ful_gen) { return Route::Reply(st); } // BUSREG-TAG-IDENTITY (B1157): no private body reaches a holder not named as the fulfiller (file tail)
    if !(ops.has_room)(ful_row) {
        return Route::Reply(EAGAIN);
    }
    // Reserve the pending slot (per-caller cap; a slot whose caller is dead may be reclaimed).
    let mut relay = NEXT_RELAY.fetch_add(1, Ordering::Relaxed);
    if relay == 0 {
        relay = NEXT_RELAY.fetch_add(1, Ordering::Relaxed);
    }
    let pend = Pend { relay, verb: hdr.verb, caller_row, caller_gen, caller_corr: hdr.corr, ful_row, ful_gen };
    let ok = locked(|| {
        let mut t = PEND.lock();
        let mine = t.iter().flatten().filter(|p| p.caller_row == caller_row && p.caller_gen == caller_gen).count();
        if mine >= PEND_MAX_PER_CALLER {
            return false;
        }
        // A free slot, else one whose CALLER is gone (its answer would be discarded anyway).
        let idx = t.iter().position(|s| s.is_none()).or_else(|| t.iter().position(|s| s.is_some_and(|p| (ops.rgen)(p.caller_row) != p.caller_gen)));
        match idx {
            Some(i) => {
                t[i] = Some(pend);
                true
            }
            None => false,
        }
    });
    if !ok {
        return Route::Reply(EAGAIN);
    }
    let mut frame = alloc::vec![0u8; crate::bus::BUS_HDR_LEN + body.len()];
    let h = crate::bus::BusHdr {
        kind: crate::bus::BUS_KIND_REQUEST,
        verb: hdr.verb,
        corr: relay,
        status: 0,
        principal: caller_prin,
        body_len: body.len() as u32,
    };
    crate::bus::hdr_write(&h, &mut frame);
    frame[crate::bus::BUS_HDR_LEN..].copy_from_slice(body);
    if !(ops.push)(ful_row, frame.into_boxed_slice()) {
        take_pending(|p| p.relay == relay);
        return Route::Reply(EAGAIN);
    }
    RELAYED.fetch_add(1, Ordering::Relaxed);
    Route::Relayed
}

fn take_pending(pred: impl Fn(&Pend) -> bool) -> Option<Pend> {
    locked(|| {
        let mut t = PEND.lock();
        for s in t.iter_mut() {
            if let Some(p) = s {
                if pred(p) {
                    let out = *p;
                    *s = None;
                    return Some(out);
                }
            }
        }
        None
    })
}

fn deliver(ops: &Ops, p: &Pend, status: i32, body: &[u8]) -> bool {
    let body: &[u8] = if status == 0 || status == una_abi::BUS_STATUS_MORE { body } else { &[] };
    let mut frame = alloc::vec![0u8; crate::bus::BUS_HDR_LEN + body.len()];
    crate::bus::build_reply(p.verb, p.caller_corr, status, kernel_reply_principal(), body, &mut frame);
    if p.caller_row == crate::prefs_client::KCLIENT_ROW { return crate::prefs_client::inbox_push(frame.into_boxed_slice()); } // SETTINGSBUS (B337): the kernel's preference client is a caller row of its own
    (ops.push)(p.caller_row, frame.into_boxed_slice())
}

/// The REPLY-side hook: a ring-3 REPLY frame is legal only as a fulfiller's answer to a relay it holds.
/// Returns the `SYS_MSEND` return value for the FULFILLER.
pub fn fulfiller_reply(ops: &Ops, row: usize, rgen: u64, hdr: &crate::bus::BusHdr, frame: &[u8]) -> i64 {
    if hdr.principal != [0u8; 32] {
        return EINVAL; // the kernel stamps — a fulfiller's claim is refused, never overwritten
    }
    let relay = hdr.corr;
    let verb = hdr.verb;
    let found = locked(|| PEND.lock().iter().flatten().find(|p| p.relay == relay && p.ful_row == row && p.ful_gen == rgen && p.verb == verb).copied());
    let Some(p) = found else {
        return ENOENT; // not a relay this row holds — no reply to an arbitrary caller can be forged
    };
    if (ops.rgen)(p.caller_row) != p.caller_gen {
        take_pending(|q| q.relay == relay); // the caller is gone; the answer has nowhere to go
        return 0;
    }
    if !(if p.caller_row == crate::prefs_client::KCLIENT_ROW { crate::prefs_client::inbox_has_room() } else { (ops.has_room)(p.caller_row) }) {
        return EAGAIN; // pending kept — the fulfiller may retry
    }
    // The `more` flag (VEINCORE B304, kept by LUMENAPP B323 as bus mechanism). A BUS_STATUS_MORE frame is
    // one non-final frame of a multi-frame answer: delivered WITH its body, the pending entry LEFT OPEN for the next; status 0 / an errno closes.
    if hdr.status == una_abi::BUS_STATUS_MORE {
        if !deliver(ops, &p, hdr.status, &frame[crate::bus::BUS_HDR_LEN..]) {
            return EAGAIN;
        }
        MORE.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    if take_pending(|q| q.relay == relay).is_none() {
        return ENOENT; // raced the fulfiller's own exit sweep
    }
    if !deliver(ops, &p, hdr.status, &frame[crate::bus::BUS_HDR_LEN..]) {
        return EAGAIN;
    }
    REPLIED.fetch_add(1, Ordering::Relaxed);
    0
}

/// Row teardown: drop the row's registrations; every caller it still owed an answer gets `-ECONNRESET`.
pub fn on_exit(ops: &Ops, row: usize) {
    locked(|| {
        for s in REGS.lock().iter_mut() {
            if s.is_some_and(|r| r.row == row) {
                *s = None;
            }
        }
    });
    while let Some(p) = take_pending(|p| p.ful_row == row) {
        if (ops.rgen)(p.caller_row) == p.caller_gen && deliver(ops, &p, ECONNRESET as i32, &[]) {
            ORPHAN.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// BANDY3-CODEC: the new frames' goldens (self-authored at this commit — the compat anchor for the
// register request, the relayed request carrying a stamped principal, and the fulfiller's reply).
// ---------------------------------------------------------------------------------------------

/// REGISTER request, corr = 1, body = [PREF_GET, PREF_LIST].
const GOLDEN_REQ_REGISTER: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, 0x01, 0x01, 127, 0x00, // magic, v1, REQUEST, verb REGISTER, rsvd
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // corr = 1, status = 0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // principal zero (kernel stamps)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x02, 0x00, 0x00, 0x00, // body_len = 2
    128, 129, // PREF_GET, PREF_LIST
];

/// RELAYED PrefGet request as the fulfiller receives it: corr = relay id 5, principal = the CALLER's
/// stamp (aarch64 PROGRAM_NAME kind 1, "prog:MIDDEN.BIN"), body = "ui.theme".
const GOLDEN_RELAYED: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, 0x01, 0x01, 128, 0x00,
    0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // corr = relay 5
    0x01, 15, b'p', b'r', b'o', b'g', b':', b'M', b'I', b'D', b'D', b'E', b'N', b'.', b'B', b'I', // kind 1, len 15
    b'N', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x08, 0x00, 0x00, 0x00,
    b'u', b'i', b'.', b't', b'h', b'e', b'm', b'e',
];

/// The fulfiller's REPLY to relay 5: status 0, principal zero (the kernel re-stamps), body "dark".
const GOLDEN_FUL_REPLY: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, 0x01, 0x02, 128, 0x00,
    0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x04, 0x00, 0x00, 0x00,
    b'd', b'a', b'r', b'k',
];

/// The codec KATs: 4 bits (register golden, relayed golden, fulfiller-reply golden, registrable range
/// admitted / kernel range unchanged). Returns the bit mask.
pub fn codec_kats() -> u32 {
    let mut w = 0u32;
    let mut f = [0u8; 128];
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 1, &[una_abi::BUS_VERB_R3PREF_GET, una_abi::BUS_VERB_R3PREF_LIST], &mut f);
    if &f[..n] == GOLDEN_REQ_REGISTER && matches!(crate::bus::frame_parse(&f[..n]), Ok(h) if h.verb == BUS_VERB_REGISTER && crate::bus::request_validate(&h).is_ok()) {
        w |= 1 << 0;
    }
    let mut prin = [0u8; 32];
    prin[0] = 1;
    prin[1] = 15;
    prin[2..17].copy_from_slice(b"prog:MIDDEN.BIN");
    let h = crate::bus::BusHdr { kind: crate::bus::BUS_KIND_REQUEST, verb: una_abi::BUS_VERB_R3PREF_GET, corr: 5, status: 0, principal: prin, body_len: 8 };
    let mut g = [0u8; 128];
    crate::bus::hdr_write(&h, &mut g);
    g[52..60].copy_from_slice(b"ui.theme");
    // A relayed frame PARSES (the fulfiller's view) but would be refused as a ring-3 REQUEST (stamped).
    if &g[..60] == GOLDEN_RELAYED && matches!(crate::bus::frame_parse(&g[..60]), Ok(p) if p.principal == prin && crate::bus::request_validate(&p).is_err()) {
        w |= 1 << 1;
    }
    let mut r = [0u8; 128];
    let rn = crate::bus::build_reply(una_abi::BUS_VERB_R3PREF_GET, 5, 0, [0u8; 32], b"dark", &mut r);
    if &r[..rn] == GOLDEN_FUL_REPLY && matches!(crate::bus::frame_parse(&r[..rn]), Ok(p) if p.kind == crate::bus::BUS_KIND_REPLY && p.corr == 5) {
        w |= 1 << 2;
    }
    // TESTFIX3: the "unassigned kernel tag" probe was 11, which the merge9 fold gave to ATTR_SET (11..=15) —
    // the bit failed on the metal (`w=0x1f7/0x1ff`). Probe 126, the unassigned tag under REGISTER, and hold
    // the kernel-owned tags (ATTR_SET, PREF_GET) valid so the kernel range is pinned, not guessed.
    if crate::bus::verb_valid(128) && crate::bus::verb_valid(255) && crate::bus::verb_valid(BUS_VERB_REGISTER) && !crate::bus::verb_valid(126) && !crate::bus::verb_valid(0)
        && crate::bus::verb_valid(una_abi::BUS_VERB_ATTR_SET) && crate::bus::verb_valid(una_abi::BUS_VERB_PREF_GET) {
        w |= 1 << 3;
    }
    w
}

/// Arch hooks the `tests bandy3` fixture drives: the PRODUCTION msend body under a scratch identity,
/// a non-blocking pop, a mailbox clear, and the arch's teardown sweep. Defined per arch.
pub struct Fixture {
    pub ops: &'static Ops,
    /// `sys_msend_for` / `busx_msend_for` under scratch `row`'s identity (principal = `prin`'s source).
    pub send: fn(row: usize, frame: &[u8]) -> i64,
    pub pop: fn(row: usize) -> Option<Box<[u8]>>,
    pub clear: fn(row: usize),
    /// The principal wire image the arch stamps for scratch `row` (what the fulfiller must see).
    pub stamp: fn(row: usize) -> [u8; 32],
    /// Two scratch rows (caller, fulfiller) — no live tenant at fixture time.
    pub rows: (usize, usize),
}

/// `tests bandy3`: the witness line. Drives the production relay through the arch's own msend path.
pub fn selftest(fx: &Fixture) {
    let (a, f) = fx.rows;
    // The scratch rows must hold no registration (a live fulfiller there is never disturbed) — SKIP.
    let busy = locked(|| REGS.lock().iter().flatten().any(|r| (r.row == a || r.row == f) && (fx.ops.rgen)(r.row) == r.rgen));
    if busy {
        serial_println!(":: BANDY3: scratch rows {}/{} hold a live registration — fixture SKIP ::", a, f);
        return;
    }
    // The verb pair: the top two registrable tags, always OPEN. BUSREG-TAG-IDENTITY (B1157): Principia's
    // GET/LIST are held only by a row named PREFS, and the scratch fulfiller is unnamed — its identity legs
    // are `tagid_selftest`'s, below; a running PREFS.ELF is never displaced either way.
    let (vg, vl) = (254u8, 255u8);
    (fx.clear)(a);
    (fx.clear)(f);
    let mut w = codec_kats(); // bits 0..3
    let parse = |b: &[u8]| crate::bus::frame_parse(b).ok();
    let mut buf = [0u8; 128];

    // bit4: no fulfiller -> the caller gets a KERNEL reply -ENOENT (an answer, not a hang).
    let n = crate::bus::build_request(vg, 41, b"ui.theme", &mut buf);
    let probe = (fx.send)(a, &buf[..n]);
    if probe == 0 && matches!((fx.pop)(a).as_deref().and_then(parse), Some(h) if h.status == ENOENT as i32 && h.corr == 41 && h.principal[0] == PRIN_KERNEL_REPLY) {
        w |= 1 << 4;
    }
    // bit5: a kernel tag is refused -EEXIST; GET+LIST register 0; another row cannot take them.
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 2, &[crate::bus::BUS_VERB_LS], &mut buf);
    let k = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == EEXIST as i32);
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 3, &[vg, vl], &mut buf);
    let r = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 3);
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 4, &[vg], &mut buf);
    let taken = (fx.send)(a, &buf[..n]) == 0 && matches!((fx.pop)(a).as_deref().and_then(parse), Some(h) if h.status == EEXIST as i32);
    if k && r && taken {
        w |= 1 << 5;
    }
    // bit6: the caller's PrefGet reaches the fulfiller with the CALLER's stamp and a relay corr.
    let n = crate::bus::build_request(vg, 42, b"ui.theme", &mut buf);
    let sent = (fx.send)(a, &buf[..n]) == 0 && (fx.pop)(a).is_none();
    let relayed = (fx.pop)(f);
    let rh = relayed.as_deref().and_then(parse);
    let relay_id = rh.map(|h| h.corr).unwrap_or(0);
    if sent && matches!(rh, Some(h) if h.kind == crate::bus::BUS_KIND_REQUEST && h.verb == vg && h.principal == (fx.stamp)(a) && h.principal != [0u8; 32] && h.corr != 42)
        && relayed.as_deref().map(|b| &b[crate::bus::BUS_HDR_LEN..]) == Some(b"ui.theme".as_slice())
    {
        w |= 1 << 6;
    }
    // bit7: a forged answer (unknown relay id; or from the wrong row) is -ENOENT; the real one lands at
    // the caller KERNEL-stamped with the caller's own corr and the fulfiller's body.
    let mut rb = [0u8; 128];
    let rn = crate::bus::build_reply(vg, relay_id.wrapping_add(1000), 0, [0u8; 32], b"x", &mut rb);
    let forged_id = (fx.send)(f, &rb[..rn]) == ENOENT;
    let rn = crate::bus::build_reply(vg, relay_id, 0, [0u8; 32], b"dark", &mut rb);
    let forged_row = (fx.send)(a, &rb[..rn]) == ENOENT;
    let answered = (fx.send)(f, &rb[..rn]) == 0;
    let back = (fx.pop)(a);
    if forged_id && forged_row && answered && matches!(back.as_deref().and_then(parse), Some(h) if h.kind == crate::bus::BUS_KIND_REPLY && h.corr == 42 && h.status == 0 && h.principal[0] == PRIN_KERNEL_REPLY && h.principal[1..].iter().all(|&b| b == 0))
        && back.as_deref().map(|b| &b[crate::bus::BUS_HDR_LEN..]) == Some(b"dark".as_slice())
    {
        w |= 1 << 7;
    }
    // bit8: the fulfiller exits with a request pending -> the caller gets -ECONNRESET; the verb is free.
    let n = crate::bus::build_request(vl, 43, b"", &mut buf);
    let pend = (fx.send)(a, &buf[..n]) == 0 && (fx.pop)(f).is_some();
    let o0 = ORPHAN.load(Ordering::Relaxed);
    on_exit(fx.ops, f);
    let reset = matches!((fx.pop)(a).as_deref().and_then(parse), Some(h) if h.status == ECONNRESET as i32 && h.corr == 43 && h.body_len == 0);
    let n = crate::bus::build_request(vl, 44, b"", &mut buf);
    let freed = (fx.send)(a, &buf[..n]) == 0 && matches!((fx.pop)(a).as_deref().and_then(parse), Some(h) if h.status == ENOENT as i32);
    if pend && reset && freed && ORPHAN.load(Ordering::Relaxed) == o0 + 1 {
        w |= 1 << 8;
    }
    (fx.clear)(a);
    (fx.clear)(f);
    on_exit(fx.ops, a);
    const ALL: u32 = (1 << 9) - 1;
    let (rg, rl, rp, or) = (REGISTERED.load(Ordering::Relaxed), RELAYED.load(Ordering::Relaxed), REPLIED.load(Ordering::Relaxed), ORPHAN.load(Ordering::Relaxed));
    if w == ALL {
        serial_println!(":: BANDY3: registered={} relayed={} replied={} orphan={} -> PASS ::", rg, rl, rp, or);
    } else {
        serial_println!(":: BANDY3: registered={} relayed={} replied={} orphan={} w={:#05x}/{:#05x} verbs={}/{} -> FAIL ::", rg, rl, rp, or, w, ALL, vg, vl);
    }
    tagid_selftest(fx); // BUSREG-TAG-IDENTITY (B1157): the identity legs, after the BANDY3 line
}

/// HOLOCRON2 (rmbp-ledger B355): does `verb` have a registration right now? (Generation not checked: a row
/// that died since is reaped at the next REGISTER; `tests holocron` and the login launch only read it.)
pub fn registered(verb: u8) -> bool {
    locked(|| REGS.lock().iter().flatten().any(|r| r.verb == verb))
}

// PREFSCAP (rmbp-ledger B454, SECREVIEW F4): `R3PREF_SET` (130) exists so the kernel's preference client
// (Settings, `prefs_client`) reaches Principia's ring-3 fulfiller, which forwards it to `PREF_SET` under
// ITS OWN name — a confused deputy for any other caller. A ring-3 program writes through `PREF_SET` (17)
// itself, under its own stamped name (`prefs::bus_fulfil_from`); 130 from any row but the client is -EACCES.
const EACCES: i64 = -13;

/// The PREF tags' caller gate: `Some(status)` = answered here, never relayed.
pub fn pref_gate(verb: u8, caller_row: usize) -> Option<i64> {
    (verb == una_abi::BUS_VERB_R3PREF_SET && caller_row != crate::prefs_client::KCLIENT_ROW).then_some(EACCES)
}

#[allow(dead_code)] // a no-witness build has no caller
/// `tests prefscap`' deputy leg: a ring-3 row's `R3PREF_SET` is refused, the kernel client's is routed.
pub fn r3pref_set_refused() -> bool {
    pref_gate(una_abi::BUS_VERB_R3PREF_SET, 0) == Some(EACCES)
        && pref_gate(una_abi::BUS_VERB_R3PREF_SET, crate::prefs_client::KCLIENT_ROW).is_none()
        && pref_gate(una_abi::BUS_VERB_R3PREF_GET, 0).is_none()
}
/// RINGLOGIN (rmbp-ledger B465): the row registered for `verb` right now (SYS_RINGKEY hands the login's ring
/// key only to the process holding Holocron's verbs). Tail append.
pub fn fulfiller_row(verb: u8) -> Option<(usize, u64)> {
    locked(|| REGS.lock().iter().flatten().find(|r| r.verb == verb).map(|r| (r.row, r.rgen)))
        .filter(|&(r, _)| matches!(tag_id(verb, r, &mut [0u8; crate::video::wm::MAX_TITLE]), TagId::Open | TagId::Fulfiller)) // BUSREG-TAG-IDENTITY (B1157): a private tag's holder counts only when NAMED its fulfiller (SYS_RINGKEY's trust, B1155's judge)
}

// =================================================================================================
// BUSREG-TAG-IDENTITY (rmbp-ledger B1157) — TAIL-APPENDED. REGISTER was first-come for every tag, so a
// program started before login could claim Principia's 128..=130 or Holocron's 144..=151 and read every
// relayed body and the session principal in the relayed header. A tag the kernel relays PRIVATE bodies on is
// now held only by the image the kernel names for it (`TAG_IMAGE`), judged on the ONE program name the kernel
// already trusts: the launcher-armed name of the row's wm owner (`wm::app_name_of`, PREFSCAP's key — never the
// body, never the principal, no capability table). The launcher arms AFTER the spawn returns (WINTITLE-LATE,
// wm.rs `app_name_arm`), and PREFS.ELF registers as its second syscall, so the rule is applied twice: REGISTER
// refuses a row NAMED as another program (-EACCES); the RELAY — by then the name is armed — refuses any holder
// not named as the fulfiller (named otherwise, or still unnamed), drops its registration and answers the caller
// -ENOENT, so no body ever reaches it. `fulfiller_row` (SYS_RINGKEY's trust, B1155's judge) takes the same
// filter. Every other tag stays open. Residual (docs/SECURITY.md): the name is trusted by spelling, as
// PREFSCAP's is. Witness: `tests bandy3` → `:: BUSREG-TAGID: … -> PASS ::`.
// =================================================================================================
/// (first tag, last tag, the image the kernel names as that range's fulfiller).
const TAG_IMAGE: &[(u8, u8, &str)] = &[
    (una_abi::BUS_VERB_R3PREF_GET, una_abi::BUS_VERB_R3PREF_SET, "/apps/PREFS.ELF"),
    (una_abi::BUS_VERB_HOLOCRON_FIRST, una_abi::BUS_VERB_HOLOCRON_LAST, "/apps/HOLOCRON.ELF"),
];

/// Private-tag REGISTERs refused, and registrations dropped at the relay or evicted (monotonic since boot).
pub static TAGID_REFUSED: AtomicU32 = AtomicU32::new(0);
pub static TAGID_DROPPED: AtomicU32 = AtomicU32::new(0);

/// The image the kernel names for `verb`, `None` = an open tag.
fn tag_image(verb: u8) -> Option<&'static str> {
    TAG_IMAGE.iter().find(|&&(lo, hi, _)| (lo..=hi).contains(&verb)).map(|&(_, _, img)| img)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TagId {
    /// Not a private tag: first-come, as before.
    Open,
    /// The row is named as the tag's fulfiller.
    Fulfiller,
    /// The row is named as another program.
    Other,
    /// No name armed for the row (yet).
    Unnamed,
}

/// `row`'s launcher-armed program name, lowercased into `buf` (`""` = none armed). The wm owner is
/// `owner_of_launch(row)` — x86 row + 1, aarch64 the ASID — the key `prefs::bus_fulfil_from` uses.
fn row_name(row: usize, buf: &mut [u8; crate::video::wm::MAX_TITLE]) -> &str {
    let n = crate::video::wm::app_name_of(crate::video::wm::owner_of_launch(row as u64), buf);
    buf[..n].make_ascii_lowercase();
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// Who `row` is for `verb`. Takes no lock but wm's leaf `APP_NAMES` — never called under `REGS`.
fn tag_id(verb: u8, row: usize, buf: &mut [u8; crate::video::wm::MAX_TITLE]) -> TagId {
    let Some(img) = tag_image(verb) else { return TagId::Open };
    let name = row_name(row, buf);
    if name.is_empty() {
        TagId::Unnamed
    } else if name.eq_ignore_ascii_case(crate::video::wm::program_name(img)) {
        TagId::Fulfiller
    } else {
        TagId::Other
    }
}

/// Remove exactly the registration `(verb, row, rgen)` that was judged (a re-registration since is untouched).
fn drop_reg(verb: u8, row: usize, rgen: u64) {
    locked(|| {
        for s in REGS.lock().iter_mut() {
            if s.is_some_and(|r| r.verb == verb && r.row == row && r.rgen == rgen) {
                *s = None;
            }
        }
    });
}

/// `register`'s gate: a row named as another program registers no private tag (`Some(-EACCES)`). A private
/// tag held by a row NAMED as another program (a squatter that registered while still unnamed) is evicted, so
/// the real fulfiller's REGISTER is not answered -EEXIST by it.
fn tagid_register_gate(ops: &Ops, row: usize, body: &[u8]) -> Option<i64> {
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    for &v in body {
        if tag_id(v, row, &mut nb) == TagId::Other {
            serial_println!("[busreg] register tag={} by={} refused: not the fulfiller", v, row_name(row, &mut nb));
            TAGID_REFUSED.fetch_add(1, Ordering::Relaxed);
            return Some(EACCES);
        }
    }
    for &v in body {
        if let Some((hr, hg)) = lookup(ops, v) {
            if hr != row && tag_id(v, hr, &mut nb) == TagId::Other {
                serial_println!("[busreg] evict tag={} row={} by={}: not the fulfiller", v, hr, row_name(hr, &mut nb));
                drop_reg(v, hr, hg);
                TAGID_DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    None
}

/// `route_request`'s gate: a private tag's request is relayed only to a holder NAMED as its fulfiller; any
/// other holder loses the registration and the caller is answered `-ENOENT` (no frame reaches the holder).
fn tagid_relay_gate(verb: u8, ful_row: usize, ful_gen: u64) -> Option<i64> {
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    let id = tag_id(verb, ful_row, &mut nb);
    if matches!(id, TagId::Open | TagId::Fulfiller) {
        return None;
    }
    let name = if id == TagId::Unnamed { "unnamed" } else { row_name(ful_row, &mut nb) };
    serial_println!("[busreg] relay tag={} to=row:{} by={} refused: not the fulfiller — registration dropped", verb, ful_row, name);
    drop_reg(verb, ful_row, ful_gen);
    TAGID_DROPPED.fetch_add(1, Ordering::Relaxed);
    Some(ENOENT)
}

/// `tests bandy3`' identity legs, on the fixture's scratch rows (caller `a`, fulfiller `f`):
/// L1 `f` named PROBE registers 130 -> -EACCES; L2 `f` unnamed registers 128 -> 0, the caller's GET 128 ->
/// KERNEL -ENOENT, `f`'s mailbox stays empty, 128 is dropped; L3 `f` named PREFS registers 128+130 -> 0, the
/// caller's GET 128 reaches `f` with its body, `fulfiller_row(130)` = `f`. SKIP when a live row holds one of
/// Principia's tags (a running PREFS.ELF is never disturbed) or `f`'s wm owner already carries a name.
fn tagid_selftest(fx: &Fixture) {
    let (a, f) = fx.rows;
    let owner_f = crate::video::wm::owner_of_launch(f as u64);
    let (get, set) = (una_abi::BUS_VERB_R3PREF_GET, una_abi::BUS_VERB_R3PREF_SET);
    if (get..=set).any(|v| lookup(fx.ops, v).is_some()) {
        serial_println!(":: BUSREG-TAGID: SKIP (a live row holds Principia's tags) ::");
        return;
    }
    if crate::video::wm::app_name_armed(owner_f) {
        serial_println!(":: BUSREG-TAGID: SKIP (scratch row {} carries a name) ::", f);
        return;
    }
    (fx.clear)(a);
    (fx.clear)(f);
    let parse = |b: &[u8]| crate::bus::frame_parse(b).ok();
    let mut buf = [0u8; 128];
    let mut w = 0u32;

    // L1: a row named as another program is refused at REGISTER.
    let _ = crate::video::wm::app_name_arm(owner_f, "/apps/PROBE.ELF");
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 61, &[set], &mut buf);
    let refused = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == EACCES as i32 && h.corr == 61);
    if refused && lookup(fx.ops, set).is_none() {
        w |= 1 << 0;
    }
    // L2: an unnamed row (the WINTITLE-LATE window) registers, but its first relay drops it — no body.
    crate::video::wm::app_name_forget(owner_f);
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 62, &[get], &mut buf);
    let admitted = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 62);
    let n = crate::bus::build_request(get, 63, b"ui.theme", &mut buf);
    let sent = (fx.send)(a, &buf[..n]) == 0;
    let enoent = matches!((fx.pop)(a).as_deref().and_then(parse), Some(h) if h.status == ENOENT as i32 && h.corr == 63 && h.principal[0] == PRIN_KERNEL_REPLY);
    let unseen = (fx.pop)(f).is_none();
    if admitted && sent && enoent && unseen && lookup(fx.ops, get).is_none() {
        w |= 1 << 1;
    }
    // L3: the row named PREFS registers and the relay reaches it with the caller's body.
    let _ = crate::video::wm::app_name_arm(owner_f, "/apps/PREFS.ELF");
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 64, &[get, set], &mut buf);
    let ok = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 64);
    let n = crate::bus::build_request(get, 65, b"ui.theme", &mut buf);
    let sent = (fx.send)(a, &buf[..n]) == 0 && (fx.pop)(a).is_none();
    let relayed = (fx.pop)(f);
    let reached = matches!(relayed.as_deref().and_then(parse), Some(h) if h.kind == crate::bus::BUS_KIND_REQUEST && h.verb == get && h.corr != 65)
        && relayed.as_deref().map(|b| &b[crate::bus::BUS_HDR_LEN..]) == Some(b"ui.theme".as_slice());
    let holder = matches!(fulfiller_row(set), Some((r, _)) if r == f);
    if ok && sent && reached && holder {
        w |= 1 << 2;
    }
    on_exit(fx.ops, f);
    (fx.clear)(a);
    (fx.clear)(f);
    on_exit(fx.ops, a);
    crate::video::wm::app_name_forget(owner_f);
    const ALL: u32 = (1 << 3) - 1;
    let (rf, dr) = (TAGID_REFUSED.load(Ordering::Relaxed), TAGID_DROPPED.load(Ordering::Relaxed));
    if w == ALL {
        serial_println!(":: BUSREG-TAGID: probe=refused unnamed=dropped prefs=admitted relayed=1 refused={} dropped={} -> PASS ::", rf, dr);
    } else {
        serial_println!(
            ":: BUSREG-TAGID: probe={} unnamed={} prefs={} relayed={} refused={} dropped={} w={:#x}/{:#x} -> FAIL ::",
            if w & 1 != 0 { "refused" } else { "OPEN" },
            if w & 2 != 0 { "dropped" } else { "KEPT" },
            if w & 4 != 0 { "admitted" } else { "REFUSED" },
            (w >> 2) & 1, rf, dr, w, ALL
        );
    }
}

// =================================================================================================
// PREFSELF-RELAUNCH (rmbp-ledger B1158) — TAIL-APPENDED. At the login commit (`fs::users::login`, after the
// session is stamped and before the login items and Holocron start) every live row holding a relayed-private
// tag that is NOT a program of the new session (a PREFS.ELF or HOLOCRON.ELF launched before login) is ENDED,
// never re-stamped (THE PRINCIPAL RULE above). The end is SECLOGIN M3's own road per row (`end_bus_row` on each
// arch: `wm::close_owner` + `bg_kill` + the gone re-read); the row's teardown runs `on_exit`, which releases its
// tags and answers its relays -ECONNRESET, so the session's own launch registers cleanly.
// =================================================================================================

/// The relayed-private tags: Principia's three (128-130) and Holocron's range (144-151). BUSREG-TAG-IDENTITY
/// (B1157) owns the table; until it folds this one line is the list (the seat swaps in B1157's table).
#[cfg(feature = "login")]
fn relayed_private(v: u8) -> bool {
    matches!(v, una_abi::BUS_VERB_R3PREF_GET | una_abi::BUS_VERB_R3PREF_LIST | una_abi::BUS_VERB_R3PREF_SET) || (una_abi::BUS_VERB_HOLOCRON_FIRST..=una_abi::BUS_VERB_HOLOCRON_LAST).contains(&v)
}

/// End every live relayed-private fulfiller row that is not a program of the open session (B1155's judge,
/// `pref_row_in_session`). REGS is copied out under its lock and released before any arch call. Returns how
/// many rows ended (the kill confirmed).
#[cfg(feature = "login")]
pub fn end_prelogin_fulfillers() -> usize {
    let ops = crate::arch::syscall::busreg_ops();
    let mut rows = [(usize::MAX, 0u64, 0u32); REG_CAP];
    let mut n = 0usize;
    locked(|| {
        for r in REGS.lock().iter().flatten() {
            if !relayed_private(r.verb) || (ops.rgen)(r.row) != r.rgen {
                continue;
            }
            if let Some(e) = rows[..n].iter_mut().find(|e| e.0 == r.row && e.1 == r.rgen) {
                e.2 += 1;
            } else if n < REG_CAP {
                rows[n] = (r.row, r.rgen, 1);
                n += 1;
            }
        }
    });
    let mut ended = 0usize;
    for &(row, rgen, tags) in &rows[..n] {
        if crate::arch::syscall::pref_row_in_session(row) {
            continue;
        }
        match crate::arch::syscall::end_bus_row(row, rgen) {
            Some((pid, windows, settle, true)) => {
                ended += 1;
                serial_println!("[prefsbus] fulfiller row={} ended at login: pre-login pid={} tags={} windows={} kill=\"{}\"", row, pid, tags, windows, settle);
            }
            Some((pid, windows, settle, false)) => {
                serial_println!("[prefsbus] fulfiller row={} NOT ended at login: pre-login pid={} tags={} windows={} kill=\"{}\"", row, pid, tags, windows, settle);
            }
            None => serial_println!("[prefsbus] fulfiller row={} not ended at login: pre-login tags={} no live process", row, tags),
        }
    }
    ended
}
