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
        unread_handoff(row, &p); if (ops.rgen)(p.caller_row) == p.caller_gen && deliver(ops, &p, ECONNRESET as i32, &[]) { // RELAY-RESET-SET (B1169): a kernel-client SET whose frame the dying fulfiller never read is marked for `prefs_client` BEFORE the reset is delivered (file tail). LINE-NEUTRAL fold.
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
    tagid_selftest(fx); armearly_selftest(fx); unreadset_selftest(fx); /* RELAY-RESET-SET (B1169): the unread-mark legs (file tail). BUSREG-ARM-EARLY (B1164): the early-arm legs, after the BUSREG-TAGID line (file tail) */ armforget_selftest(fx); /* ARMFORGET-CHAIN (B1171): the teardown-forget legs (file tail) */ crate::prefs_client::queued_selftest(); /* RELAY-QUEUED-SET (B1175): the queued-set legs (prefs_client.rs tail) */ armlate_selftest(fx); /* ARMFORGET-LATEARM (B1178): the late-arm stamp legs (file tail) */ // BUSREG-TAG-IDENTITY (B1157): the identity legs, after the BANDY3 line
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
// filter. Every other tag stays open. The spelling residual is closed by BUSREG-TAG-PATH (B1163, tail below:
// the path the launcher loaded decides, the name only demotes). Witness: `tests bandy3` → `:: BUSREG-TAGID: … -> PASS ::`.
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
pub fn tag_image(verb: u8) -> Option<&'static str> {
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
    let n = crate::video::wm::app_name_of(crate::video::wm::owner_of_launch(row as u64), buf); let n = armearly_name(row, n, buf); // BUSREG-ARM-EARLY (B1164): no launcher-armed name yet -> the one the arch spawn armed at the mint, at the row's CURRENT generation (file tail)
    buf[..n].make_ascii_lowercase();
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// Who `row` is for `verb`. Takes no lock but wm's leaf `APP_NAMES` — never called under `REGS`.
fn tag_id(verb: u8, row: usize, buf: &mut [u8; crate::video::wm::MAX_TITLE]) -> TagId {
    let Some(img) = tag_image(verb) else { return TagId::Open }; if let Some(id) = tagpath_id(row, img, buf) { return id; } // BUSREG-TAG-PATH (B1163): the loaded /apps/ path decides, the name only demotes (file tail)
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
    // BUSREG-TAG-PATH (B1163) L4: named PREFS, but loaded from a home path -> refused at REGISTER.
    on_exit(fx.ops, f); (fx.clear)(a); // seat fold: L3's pending GET is reset into a's mailbox by this on_exit (found by B1164); drained so L5 reads its own reply
    (fx.clear)(f);
    let _ = crate::video::wm::app_name_arm(owner_f, "/home/una/PREFS.ELF");
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 66, &[set], &mut buf);
    let path_refused = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == EACCES as i32 && h.corr == 66);
    if path_refused && lookup(fx.ops, set).is_none() {
        w |= 1 << 3;
    }
    // L5: named PREFS by spelling alone (a relative literal arms no path) -> registers, but the relay drops it.
    let _ = crate::video::wm::app_name_arm(owner_f, "PREFS.ELF");
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 67, &[get], &mut buf);
    let admitted = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 67);
    let n = crate::bus::build_request(get, 68, b"ui.theme", &mut buf);
    let sent = (fx.send)(a, &buf[..n]) == 0;
    let enoent = matches!((fx.pop)(a).as_deref().and_then(parse), Some(h) if h.status == ENOENT as i32 && h.corr == 68 && h.principal[0] == PRIN_KERNEL_REPLY);
    let unseen = (fx.pop)(f).is_none();
    if admitted && sent && enoent && unseen && lookup(fx.ops, get).is_none() {
        w |= 1 << 4;
    }
    on_exit(fx.ops, f);
    (fx.clear)(a);
    (fx.clear)(f);
    on_exit(fx.ops, a);
    crate::video::wm::app_name_forget(owner_f);
    const ALL: u32 = (1 << 5) - 1;
    let (rf, dr) = (TAGID_REFUSED.load(Ordering::Relaxed), TAGID_DROPPED.load(Ordering::Relaxed));
    if w == ALL {
        serial_println!(":: BUSREG-TAGID: probe=refused unnamed=dropped prefs=admitted path=refused spelled=dropped relayed=1 refused={} dropped={} -> PASS ::", rf, dr);
    } else {
        serial_println!(
            ":: BUSREG-TAGID: probe={} unnamed={} prefs={} path={} spelled={} relayed={} refused={} dropped={} w={:#x}/{:#x} -> FAIL ::",
            if w & 1 != 0 { "refused" } else { "OPEN" },
            if w & 2 != 0 { "dropped" } else { "KEPT" },
            if w & 4 != 0 { "admitted" } else { "REFUSED" },
            if w & 8 != 0 { "refused" } else { "OPEN" },
            if w & 16 != 0 { "dropped" } else { "KEPT" },
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
    tag_image(v).is_some() // seat fold: B1158's inline list retired onto B1157's `TAG_IMAGE` (one table of the relayed-private tags)
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

// =================================================================================================
// BUSREG-TAG-PATH (rmbp-ledger B1163) — TAIL-APPENDED. B1157 judged a private tag's holder on the launcher-armed
// NAME, so an image called PREFS.ELF launched from any path was Principia's fulfiller. The launcher also knows
// the PATH it read the image from, and wm now keeps it beside the name (`wm::app_path_with`, armed by the same
// `app_name_arm` / `app_name_arm_launch` every launch road calls). The rule: a row with a path armed is the
// fulfiller iff that path IS `TAG_IMAGE`'s image — the `/apps/` prefix byte-exact (`/` is not a ROOTACL tree,
// so `/APPS/…` could be planted on a case-sensitive root), the leaf case-insensitive (only `kernel` and the
// administrator create anything under ROOTACL's `/apps`) — else it is `Other`. With no path armed, the
// fulfiller's NAME alone is `Unnamed` (registers — the `app_name_arm_launch` gap between name and path — and is
// dropped at the relay if still pathless): the spelling never promotes. Any other case is B1157's name logic.
// =================================================================================================

/// `tag_id`'s path key for `row` against the image `img`: `Some(verdict)` when the path decides (or the name
/// alone would have promoted), `None` = fall through to the name. Takes only wm's leaf locks.
fn tagpath_id(row: usize, img: &str, buf: &mut [u8; crate::video::wm::MAX_TITLE]) -> Option<TagId> {
    let owner = crate::video::wm::owner_of_launch(row as u64);
    if let Some(is) = crate::video::wm::app_path_with(owner, |p| path_names(p, img)).or_else(|| armearly_path(row, img)) { // BUSREG-ARM-EARLY (B1164): the path from APP_PATHS, else the early record at the row's current generation (file tail)
        return Some(if is { TagId::Fulfiller } else { TagId::Other });
    }
    let name = row_name(row, buf);
    (!name.is_empty() && name.eq_ignore_ascii_case(crate::video::wm::program_name(img))).then_some(TagId::Unnamed)
}

/// Is the loaded `path` the image `img` (`/apps/<LEAF>`)? The directory byte-exact, the leaf any case.
pub fn path_names(path: &str, img: &str) -> bool {
    const DIR: &str = "/apps/";
    match (path.strip_prefix(DIR), img.strip_prefix(DIR)) {
        (Some(leaf), Some(want)) => !leaf.is_empty() && leaf.eq_ignore_ascii_case(want),
        _ => false,
    }
}

// =================================================================================================
// BUSREG-ARM-EARLY (rmbp-ledger B1164) — TAIL-APPENDED. B1157 applied the private-tag check twice because the
// launcher arms a program's name only AFTER the spawn made the task runnable (WINTITLE-LATE), and PREFS.ELF
// registers as its second syscall. The arch spawn bodies now arm the kernel launcher's own argv[0] at the mint,
// before the task exists (x86 `spawn_user_image_bg_inner`, aarch64 `spawn_user_image_bg_piped`, one line each;
// the ring-3 SYS_SPAWN_PATH road passes its kernel-opened path, never its argv — BUSREG-ARM-EARLY-RING3 B1172), into wm's
// early record (`wm::app_name_arm_early`), stamped with the row's generation. `row_name` and `tagpath_id` read it
// only when no launcher-armed name / path exists, and only at the row's CURRENT generation, so a launched
// fulfiller is judged at REGISTER, a squatter early-named otherwise is refused there, and the relay gate stays
// the belt for a holder with neither. Window titles never see the early record (wm keeps it apart).
// Witness: `tests bandy3` → `:: BUSREG-ARMEARLY: … -> PASS ::`; every kernel-road launch says `[busreg] arm-early`.
// =================================================================================================

/// Early records armed since boot (monotonic).
pub static ARMEARLY_ARMED: AtomicU32 = AtomicU32::new(0);

/// The arch spawn body's arm: `path` is the early path the body's CALLER chose (a kernel road's argv[0], the ring-3
/// road's kernel-opened path; `None` = nothing armed); `ring3` labels the road only. Takes only wm's leaf lock.
pub fn arm_early(row: usize, path: Option<&str>, ring3: bool) {
    let Some(path) = path else { return }; // ARMFORGET-LATEARM (B1178) + ARMLATE-KNOBOFF (B1184): the mint note (path or none) is the arch spawn line's own `wm::app_mint_note`, just before this call, in every build
    let rg = (crate::arch::syscall::busreg_ops().rgen)(row);
    let armed = crate::video::wm::app_name_arm_early(crate::video::wm::owner_of_launch(row as u64), rg, path);
    if armed {
        ARMEARLY_ARMED.fetch_add(1, Ordering::Relaxed);
    }
    serial_println!(
        "[busreg] arm-early row={} gen={} name={} path={} armed={} road={}",
        row, rg, crate::video::wm::program_name(path), if path.starts_with('/') { path } else { "-" }, armed as u8, if ring3 { "ring3" } else { "kernel" }
    );
}

/// `row_name`'s fallback: `n` when a launcher-armed name exists, else the early name of `row`'s current tenant.
fn armearly_name(row: usize, n: usize, buf: &mut [u8; crate::video::wm::MAX_TITLE]) -> usize {
    if n != 0 {
        return n;
    }
    let rg = (crate::arch::syscall::busreg_ops().rgen)(row);
    crate::video::wm::app_name_early_of(crate::video::wm::owner_of_launch(row as u64), rg, buf)
}

/// `tagpath_id`'s fallback: is the early path of `row`'s current tenant the image `img`? `None` = no absolute one.
fn armearly_path(row: usize, img: &str) -> Option<bool> {
    let rg = (crate::arch::syscall::busreg_ops().rgen)(row);
    crate::video::wm::app_path_early_of(crate::video::wm::owner_of_launch(row as u64), rg, |p| path_names(p, img))
}

/// `tests bandy3`' early-arm legs, on the fixture's scratch rows (caller `a`, fulfiller `f`), with NO late arm:
/// E1 `f` early-armed `/apps/PROBE.ELF` registers 130 -> -EACCES (REGISTER alone decides); E2 `f` early-armed
/// `/apps/PREFS.ELF` registers 128+130 -> 0, the caller's GET 128 reaches `f` with its body, `fulfiller_row(130)`
/// = `f`; E3 an early record at a DEAD generation answers as unnamed: registers 128 -> 0, the GET -> KERNEL
/// -ENOENT, `f`'s mailbox empty, 128 dropped. SKIP as `tagid_selftest` does, and when `f` carries any name.
fn armearly_selftest(fx: &Fixture) {
    let (a, f) = fx.rows;
    let owner_f = crate::video::wm::owner_of_launch(f as u64);
    let (get, set) = (una_abi::BUS_VERB_R3PREF_GET, una_abi::BUS_VERB_R3PREF_SET);
    if (get..=set).any(|v| lookup(fx.ops, v).is_some()) {
        serial_println!(":: BUSREG-ARMEARLY: SKIP (a live row holds Principia's tags) ::");
        return;
    }
    let rg = (fx.ops.rgen)(f);
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    if crate::video::wm::app_name_armed(owner_f)
        || crate::video::wm::app_path_with(owner_f, |_| true).is_some()
        || crate::video::wm::app_name_early_of(owner_f, rg, &mut nb) != 0
    {
        serial_println!(":: BUSREG-ARMEARLY: SKIP (scratch row {} carries a name) ::", f);
        return;
    }
    (fx.clear)(a);
    (fx.clear)(f);
    let parse = |b: &[u8]| crate::bus::frame_parse(b).ok();
    let mut buf = [0u8; 128];
    let mut w = 0u32;

    // E1: early-named as another program (by its /apps/ path) -> refused at REGISTER, no late arm needed.
    arm_early(f, Some("/apps/PROBE.ELF"), false);
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 71, &[set], &mut buf);
    let refused = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == EACCES as i32 && h.corr == 71);
    if refused && lookup(fx.ops, set).is_none() {
        w |= 1 << 0;
    }
    // E2: early-armed as the fulfiller's image, still no late arm -> registered AND relayed (B1157 dropped it).
    arm_early(f, Some("/apps/PREFS.ELF"), false);
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 72, &[get, set], &mut buf);
    let ok = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 72);
    let n = crate::bus::build_request(get, 73, b"ui.theme", &mut buf);
    let sent = (fx.send)(a, &buf[..n]) == 0 && (fx.pop)(a).is_none();
    let relayed = (fx.pop)(f);
    let reached = matches!(relayed.as_deref().and_then(parse), Some(h) if h.kind == crate::bus::BUS_KIND_REQUEST && h.verb == get && h.corr != 73)
        && relayed.as_deref().map(|b| &b[crate::bus::BUS_HDR_LEN..]) == Some(b"ui.theme".as_slice());
    let holder = matches!(fulfiller_row(set), Some((r, _)) if r == f);
    if ok && sent && reached && holder {
        w |= 1 << 1;
    }
    on_exit(fx.ops, f);
    (fx.clear)(a);
    (fx.clear)(f);
    // E3: the same record at a generation that is not the row's current one -> unnamed: the relay drops it.
    let _ = crate::video::wm::app_name_arm_early(owner_f, rg.wrapping_add(1), "/apps/PREFS.ELF");
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 74, &[get], &mut buf);
    let admitted = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 74);
    let n = crate::bus::build_request(get, 75, b"ui.theme", &mut buf);
    let sent = (fx.send)(a, &buf[..n]) == 0;
    let enoent = matches!((fx.pop)(a).as_deref().and_then(parse), Some(h) if h.status == ENOENT as i32 && h.corr == 75 && h.principal[0] == PRIN_KERNEL_REPLY);
    let unseen = (fx.pop)(f).is_none();
    if admitted && sent && enoent && unseen && lookup(fx.ops, get).is_none() {
        w |= 1 << 2;
    }
    on_exit(fx.ops, f);
    (fx.clear)(a);
    (fx.clear)(f);
    // E4 (BUSREG-ARM-EARLY-RING3 B1172): the ring-3 road's arm (SYS_SPAWN_PATH's kernel-opened path) -> admitted
    // at REGISTER as the fulfiller, no late arm.
    arm_early(f, Some("/apps/PREFS.ELF"), true);
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 76, &[get, set], &mut buf);
    let r3 = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 76);
    if r3 && matches!(fulfiller_row(set), Some((r, _)) if r == f) {
        w |= 1 << 3;
    }
    on_exit(fx.ops, f);
    (fx.clear)(a);
    (fx.clear)(f);
    on_exit(fx.ops, a);
    crate::video::wm::app_name_early_forget(owner_f);
    const ALL: u32 = (1 << 4) - 1;
    let armed = ARMEARLY_ARMED.load(Ordering::Relaxed);
    if w == ALL {
        serial_println!(":: BUSREG-ARMEARLY: probe=refused-at-register prefs=relayed-early stale=dropped ring3=registered-early armed={} -> PASS ::", armed);
    } else {
        serial_println!(
            ":: BUSREG-ARMEARLY: probe={} prefs={} stale={} ring3={} armed={} w={:#x}/{:#x} -> FAIL ::",
            if w & 1 != 0 { "refused-at-register" } else { "OPEN" },
            if w & 2 != 0 { "relayed-early" } else { "DROPPED" },
            if w & 4 != 0 { "dropped" } else { "KEPT" },
            if w & 8 != 0 { "registered-early" } else { "REFUSED" },
            armed, w, ALL
        );
    }
}

// =================================================================================================
// BUSREG-ARM-EARLY-RING3 (rmbp-ledger B1172) — TAIL-APPENDED. The spawn bodies take the early path as an argument
// (x86 `spawn_user_image_bg_inner`, aarch64 `spawn_user_image_bg_piped`: `early`), chosen by each CALLER: a kernel
// road passes its own argv[0], SYS_SPAWN_PATH's `spawn_path_checked` the path the kernel opened, apptrust passed and
// read the image from (the string its late `app_name_arm` records), never ring-3 argv. The mint's as_row / ring3
// trust branch is retired; `arm_early`'s `ring3` labels the road only. Witness: `tests bandy3` E4 (`ring3=`).
// =================================================================================================

// RELAY-RESET-SET (rmbp-ledger B1169) — TAIL-APPENDED. RELAY-RESET-FALLBACK (B1165) left a kernel-client SET
// whose fulfiller dies mid-relay refused -ECONNRESET, because the holder may have committed it. Two truths sit
// under that reset and only the ARCH's mailbox clear (x86 `busx_mbox_clear`, aarch64 `bus_mbox_clear`, run on
// the teardown line after the generation bump and just before `on_exit`) can tell them apart: a relayed
// REQUEST still in a slot was never MRECV'd (UNREAD: the holder never saw it); a frame the holder popped is
// gone (READ: committed-or-not is unknowable). The clear reports each REQUEST frame it drops through
// `mbox_unread` (the relay id is the frame's `corr`, kernel-built by `route_request`, so ring 3 cannot forge
// it); `on_exit`'s sweep takes the mark per pending relay in `unread_handoff` and, for a kernel-client SET,
// hands the CLIENT's corr to `prefs_client::note_set_unread` BEFORE `deliver` queues the -ECONNRESET. The
// reply frame is unchanged for every caller (a status in it would be forgeable: `fulfiller_reply` passes a
// live holder's status verbatim). `UNREAD` is a LEAF lock (taken under the arch mailbox SpinMutex, IRQs
// masked, and in `on_exit` with PEND released); full = the oldest mark is overwritten, so a lost mark leaves
// the SET -ECONNRESET (fail-closed, B1165's behaviour).
// Witness: `tests bandy3` → `:: RELAY-RESET-SET: unread=marked read=unmarked handoff=once -> PASS ::`.
// =================================================================================================

/// (row, relay id) of relayed REQUEST frames a mailbox clear dropped unread; a ring, next slot in `.1`.
static UNREAD: crate::sync::Mutex<([(usize, u32); PEND_CAP], usize)> = crate::sync::Mutex::new(([(usize::MAX, 0u32); PEND_CAP], 0));

/// The arch mailbox clear's report: `frame` (a slot's frame, `None` = empty slot) is being dropped from
/// `row`'s mailbox without being read. A relayed REQUEST (a fulfiller tag, kernel-built) marks its relay id.
pub fn mbox_unread(row: usize, frame: Option<&[u8]>) {
    let Some(f) = frame else { return };
    let Ok(h) = crate::bus::frame_parse(f) else { return };
    if h.kind != crate::bus::BUS_KIND_REQUEST || h.verb < BUS_VERB_FULFIL_MIN || h.corr == 0 {
        return;
    }
    locked(|| {
        let mut u = UNREAD.lock();
        let i = u.1;
        u.0[i] = (row, h.corr);
        u.1 = (i + 1) % PEND_CAP;
    });
}

/// Take (clear) the unread mark of relay `relay` on fulfiller row `row`. `true` = the frame was never read.
fn take_unread(row: usize, relay: u32) -> bool {
    if relay == 0 {
        return false;
    }
    locked(|| {
        let mut u = UNREAD.lock();
        match u.0.iter().position(|&e| e == (row, relay)) {
            Some(i) => {
                u.0[i] = (usize::MAX, 0);
                true
            }
            None => false,
        }
    })
}

/// `on_exit`'s per-relay step: the mark is always consumed; a kernel-client SET (R3PREF_SET) whose frame was
/// unread is handed to `prefs_client` by its CLIENT corr, before the -ECONNRESET is delivered.
fn unread_handoff(row: usize, p: &Pend) {
    if take_unread(row, p.relay) && p.verb == una_abi::BUS_VERB_R3PREF_SET && p.caller_row == crate::prefs_client::KCLIENT_ROW {
        crate::prefs_client::note_set_unread(p.caller_corr);
    }
}

/// `tests bandy3`' unread-mark legs on the fixture's scratch rows (caller `a`, fulfiller `f`), open tag 254:
/// U1 a relayed request left in `f`'s mailbox and cleared is marked; U2 one `f` popped (read) and then cleared
/// is not; U3 `unread_handoff` of a kernel-client SET marks the client corr exactly once (a GET marks none).
fn unreadset_selftest(fx: &Fixture) {
    let (a, f) = fx.rows;
    let busy = locked(|| REGS.lock().iter().flatten().any(|r| (r.row == a || r.row == f) && (fx.ops.rgen)(r.row) == r.rgen));
    if busy {
        serial_println!(":: RELAY-RESET-SET: SKIP (scratch rows {}/{} hold a live registration) ::", a, f);
        return;
    }
    let vg = 254u8;
    let parse = |b: &[u8]| crate::bus::frame_parse(b).ok();
    let pend_relay = |c: u32| locked(|| PEND.lock().iter().flatten().find(|p| p.ful_row == f && p.caller_row == a && p.caller_corr == c).map(|p| p.relay));
    let mut buf = [0u8; 128];
    (fx.clear)(a);
    (fx.clear)(f);
    // U1: unread at the clear -> marked.
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 80, &[vg], &mut buf);
    let reg1 = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 80);
    let n = crate::bus::build_request(vg, 81, b"ui.theme", &mut buf);
    let sent1 = (fx.send)(a, &buf[..n]) == 0;
    let r1 = pend_relay(81);
    (fx.clear)(f);
    let u1 = reg1 && sent1 && r1.is_some_and(|r| take_unread(f, r));
    on_exit(fx.ops, f);
    (fx.clear)(a);
    // U2: read (popped) before the clear -> not marked.
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 82, &[vg], &mut buf);
    let reg2 = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 82);
    let n = crate::bus::build_request(vg, 83, b"ui.theme", &mut buf);
    let sent2 = (fx.send)(a, &buf[..n]) == 0;
    let r2 = pend_relay(83);
    let read2 = (fx.pop)(f).is_some();
    (fx.clear)(f);
    let u2 = reg2 && sent2 && read2 && r2.is_some_and(|r| !take_unread(f, r));
    on_exit(fx.ops, f);
    (fx.clear)(a);
    // U3: the handoff of a kernel-client SET marks the client corr once; a GET's mark is consumed, not handed.
    let cc = u32::MAX; // never a live client corr in practice (the client counts up from 1)
    let mut handoff = |verb: u8| {
        let mut relay = NEXT_RELAY.fetch_add(1, Ordering::Relaxed);
        if relay == 0 {
            relay = NEXT_RELAY.fetch_add(1, Ordering::Relaxed);
        }
        let n = crate::bus::build_request(verb, relay, b"", &mut buf);
        mbox_unread(f, Some(&buf[..n]));
        let p = Pend { relay, verb, caller_row: crate::prefs_client::KCLIENT_ROW, caller_gen: crate::prefs_client::KCLIENT_GEN, caller_corr: cc, ful_row: f, ful_gen: (fx.ops.rgen)(f) };
        unread_handoff(f, &p);
        !take_unread(f, relay)
    };
    let get_consumed = handoff(una_abi::BUS_VERB_R3PREF_GET);
    let get_none = !crate::prefs_client::set_unread_take(cc);
    let set_consumed = handoff(una_abi::BUS_VERB_R3PREF_SET);
    let once = crate::prefs_client::set_unread_take(cc) && !crate::prefs_client::set_unread_take(cc);
    let u3 = get_consumed && get_none && set_consumed && once;
    if u1 && u2 && u3 {
        serial_println!(":: RELAY-RESET-SET: unread=marked read=unmarked handoff=once -> PASS ::");
    } else {
        serial_println!(":: RELAY-RESET-SET: u1={} u2={} u3={} (reg={}/{} sent={}/{} read={}) -> FAIL ::", u1 as u8, u2 as u8, u3 as u8, reg1 as u8, reg2 as u8, sent1 as u8, sent2 as u8, read2 as u8);
    }
}

// ARMFORGET-TEARDOWN (rmbp-ledger B1171) — TAIL-APPENDED. Both arches forget a tenant's launcher records at the
// generation bump in `clear_handle_row` through ONE wm fn (`wm::app_records_teardown`), on the final release and
// before the slot is claimable, so a recycled owner launched by a road that arms nothing is judged Unnamed, never
// on its predecessor's name and path (which B1163's rule would PROMOTE). The leg drives that exact fn: the full
// `clear_handle_row` on a scratch row would close a live compat window (aarch64) and revoke focus.
// Witness: `tests bandy3` → `:: BUSREG-ARMFORGET: … -> PASS ::`.
// =================================================================================================

/// `tests bandy3`' teardown-forget legs, on the fixture's scratch rows (caller `a`, fulfiller `f`):
/// F1 `f` armed as the fulfiller (late name + path `/apps/PREFS.ELF` and the early record at its generation) is
/// `Fulfiller`; after `wm::app_records_teardown` all three records are gone and `f` is `Unnamed`; F2 the recycled,
/// unarmed `f` registers 128 -> 0 (REGISTER refuses only `Other`), the caller's GET 128 -> KERNEL -ENOENT, `f`'s
/// mailbox empty, 128 dropped. SKIP as `armearly_selftest` does.
fn armforget_selftest(fx: &Fixture) {
    let (a, f) = fx.rows;
    let owner_f = crate::video::wm::owner_of_launch(f as u64);
    let (get, set) = (una_abi::BUS_VERB_R3PREF_GET, una_abi::BUS_VERB_R3PREF_SET);
    if (get..=set).any(|v| lookup(fx.ops, v).is_some()) {
        serial_println!(":: BUSREG-ARMFORGET: SKIP (a live row holds Principia's tags) ::");
        return;
    }
    let rg = (fx.ops.rgen)(f);
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    if crate::video::wm::app_name_armed(owner_f)
        || crate::video::wm::app_path_with(owner_f, |_| true).is_some()
        || crate::video::wm::app_name_early_of(owner_f, rg, &mut nb) != 0
    {
        serial_println!(":: BUSREG-ARMFORGET: SKIP (scratch row {} carries a name) ::", f);
        return;
    }
    (fx.clear)(a);
    (fx.clear)(f);
    let parse = |b: &[u8]| crate::bus::frame_parse(b).ok();
    let mut buf = [0u8; 128];
    let mut w = 0u32;

    // F1: the predecessor's records, then the teardown's own forget.
    let _ = crate::video::wm::app_name_arm(owner_f, "/apps/PREFS.ELF");
    let _ = crate::video::wm::app_name_arm_early(owner_f, rg, "/apps/PREFS.ELF");
    let armed = tag_id(set, f, &mut nb) == TagId::Fulfiller;
    crate::video::wm::app_records_teardown(owner_f);
    let cleared = !crate::video::wm::app_name_armed(owner_f)
        && crate::video::wm::app_path_with(owner_f, |_| true).is_none()
        && crate::video::wm::app_name_early_of(owner_f, rg, &mut nb) == 0;
    let unnamed = tag_id(set, f, &mut nb) == TagId::Unnamed;
    if armed && cleared && unnamed {
        w |= 1 << 0;
    }
    // F2: the recycled owner, armed by nothing, registers but its first relay drops it — no body reaches it.
    let n = crate::bus::build_request(BUS_VERB_REGISTER, 81, &[get], &mut buf);
    let admitted = (fx.send)(f, &buf[..n]) == 0 && matches!((fx.pop)(f).as_deref().and_then(parse), Some(h) if h.status == 0 && h.corr == 81);
    let n = crate::bus::build_request(get, 82, b"ui.theme", &mut buf);
    let sent = (fx.send)(a, &buf[..n]) == 0;
    let enoent = matches!((fx.pop)(a).as_deref().and_then(parse), Some(h) if h.status == ENOENT as i32 && h.corr == 82 && h.principal[0] == PRIN_KERNEL_REPLY);
    let unseen = (fx.pop)(f).is_none();
    if admitted && sent && enoent && unseen && lookup(fx.ops, get).is_none() {
        w |= 1 << 1;
    }
    on_exit(fx.ops, f);
    (fx.clear)(a);
    (fx.clear)(f);
    on_exit(fx.ops, a);
    crate::video::wm::app_name_forget(owner_f);
    crate::video::wm::app_name_early_forget(owner_f);
    const ALL: u32 = (1 << 2) - 1;
    let forgot = crate::video::wm::ARMFORGET_CLEARED.load(Ordering::Relaxed);
    if w == ALL {
        serial_println!(":: BUSREG-ARMFORGET: armed=fulfiller cleared=name,path,early recycled=unnamed relay=dropped forgot={} -> PASS ::", forgot);
    } else {
        serial_println!(
            ":: BUSREG-ARMFORGET: armed={} cleared={} recycled={} relay={} forgot={} w={:#x}/{:#x} -> FAIL ::",
            if armed { "fulfiller" } else { "NOT-FULFILLER" },
            if cleared { "name,path,early" } else { "KEPT" },
            if unnamed { "unnamed" } else { "NAMED" },
            if w & 2 != 0 { "dropped" } else { "KEPT" },
            forgot, w, ALL
        );
    }
}

// =================================================================================================
// ARMFORGET-LATEARM (rmbp-ledger B1178) — TAIL-APPENDED. A child that exits and is torn down before its launcher's
// late `app_name_arm` left a name and path on a dead owner that no teardown forget (B1171) could reach, and the slot's
// next tenant inherited it (B1163's path rule PROMOTES it). Every mint now notes the row's generation in wm
// (`wm::app_mint_note` on the arch spawn line, every build — ARMLATE-KNOBOFF B1184 retired `armlate_mint` onto it), the late arm stamps name and path with it, and wm's
// readers answer only at the row's CURRENT generation (`wm::app_path_with`, `app_name_of`, `mint_title`), so
// `tag_id` judges a stale late record as absent. The fixture cannot bump a row's generation (its `clear` drains the
// mailbox only), so the leg stands in for the bump the way E3 (B1164) does: a mint note one generation back.
// Witness: `tests bandy3` → `:: BUSREG-ARMLATE: … -> PASS ::`.
// =================================================================================================

/// `tests bandy3`' late-arm legs, on the fixture's fulfiller row `f`: L1 `f`'s tenant minted one generation ago
/// (torn down since) is late-armed `/apps/PREFS.ELF` -> the record is present but stale: `tag_id` Unnamed,
/// `app_name_of` none, no path; L2 a mint at the current generation and the next arm -> `Fulfiller` (the stale stamp
/// replaced). SKIP as `armforget_selftest` does.
fn armlate_selftest(fx: &Fixture) {
    let (_, f) = fx.rows;
    let owner_f = crate::video::wm::owner_of_launch(f as u64);
    let (get, set) = (una_abi::BUS_VERB_R3PREF_GET, una_abi::BUS_VERB_R3PREF_SET);
    if (get..=set).any(|v| lookup(fx.ops, v).is_some()) {
        serial_println!(":: BUSREG-ARMLATE: SKIP (a live row holds Principia's tags) ::");
        return;
    }
    let rg = (fx.ops.rgen)(f);
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    if crate::video::wm::app_name_armed(owner_f)
        || crate::video::wm::app_path_with(owner_f, |_| true).is_some()
        || crate::video::wm::app_name_early_of(owner_f, rg, &mut nb) != 0
    {
        serial_println!(":: BUSREG-ARMLATE: SKIP (scratch row {} carries a name) ::", f);
        return;
    }
    let mut w = 0u32;

    // L1: minted at the generation before the current one (the teardown's bump came first), then the late arm.
    let prev = crate::video::wm::app_mint_note(owner_f, Some(rg.wrapping_sub(1)));
    let _ = crate::video::wm::app_name_arm(owner_f, "/apps/PREFS.ELF");
    let unnamed = tag_id(set, f, &mut nb) == TagId::Unnamed;
    let stale = unnamed
        && crate::video::wm::app_name_of(owner_f, &mut nb) == 0
        && crate::video::wm::app_path_with(owner_f, |_| true).is_none()
        && crate::video::wm::app_name_armed(owner_f);
    if stale {
        w |= 1 << 0;
    }
    // L2: the next tenant's mint at the current generation, then its arm -> the fulfiller again.
    let _ = crate::video::wm::app_mint_note(owner_f, Some(rg));
    let _ = crate::video::wm::app_name_arm(owner_f, "/apps/PREFS.ELF");
    let rearmed = tag_id(set, f, &mut nb) == TagId::Fulfiller && crate::video::wm::app_name_of(owner_f, &mut nb) != 0;
    if rearmed {
        w |= 1 << 1;
    }
    crate::video::wm::app_name_forget(owner_f);
    let _ = crate::video::wm::app_mint_note(owner_f, prev);
    const ALL: u32 = (1 << 2) - 1;
    if w == ALL {
        serial_println!(":: BUSREG-ARMLATE: late=stale-dropped rearm=fulfiller -> PASS ::");
    } else {
        serial_println!(
            ":: BUSREG-ARMLATE: late={} rearm={} w={:#x}/{:#x} -> FAIL ::",
            if w & 1 != 0 { "stale-dropped" } else { "KEPT" },
            if w & 2 != 0 { "fulfiller" } else { "NOT-FULFILLER" },
            w, ALL
        );
    }
}
