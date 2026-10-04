#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VEINCORE M3 (rmbp-ledger B304; docs/dev/evidence/rmbp-1004/VEINCORE.md): VEIN.BIN — Vein on UnaOS.
// ROADMAP §3b "port the bus, not the binary": a program on UnaOS is a handler speaking verbs on the bus.
// This one OWNS the chat verbs (BANDY3 registration) and answers them through `vein_core` — the SAME
// codec and providers the host Vein links, so the wire cannot drift between the rings.
//
// What it does, in order:
//   1. asks Principia `vein.provider` (BUS_VERB_PREF_GET, fulfilled in-kernel by PREFS): `"echo"` |
//      `"relay"`; unset / anything else = echo.
//   2. registers ChatSend / ChatReply / ChatCancel / ChatStatus (130..=133) with BUS_VERB_REGISTER.
//   3. prints `:: VEIN: registered=4 provider=<p> served=0 frames=0 -> PASS ::` and serves forever:
//      * ChatSend   — Echo: the answer streamed as ChatReply frames on the caller's corr (BUS_STATUS_MORE,
//                     then the final status-0 frame). Relay: one `[vein-relay] REQ <conv> <base64>` line
//                     on the console; the answer comes back later as verb-131 frames (below).
//      * ChatReply  — accepted from the KERNEL principal only (the `vein rsp` shell line, injected):
//                     forwarded to the waiting caller as its next ChatReply frame.
//      * ChatCancel — ends the conv's in-flight relay with -ECANCELED; answers 0.
//      * ChatStatus — `ready, provider, model`.
//
// THE PRINCIPAL RULE (BANDY3): the relayed header names the caller; this program never acts as it.
//
// ---------------------------------------------------------------------------------------------
// Syscall stubs — the user-pulse stubs verbatim (see that crate for the register-clobber contract).
// ---------------------------------------------------------------------------------------------
use una_abi::{
    BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_KIND_REQUEST, BUS_MAGIC, BUS_STATUS_MORE, BUS_VERB_CHAT_CANCEL,
    BUS_VERB_CHAT_REPLY, BUS_VERB_CHAT_SEND, BUS_VERB_CHAT_STATUS, BUS_VERB_PREF_GET, BUS_VERB_REGISTER, BUS_VERSION,
    EACCES, EAGAIN, ECANCELED, EINVAL, ENOENT, SYS_EXIT, SYS_MRECV, SYS_MSEND, SYS_WRITE, SYS_YIELD,
};
use vein_core::provider::{choose, Begin, Choice, Echo, Provider, ProviderIo, Relay, METAL_CHUNK};
use vein_core::wire::{status_request_ok, ChatCancel, ChatReply, ChatSend, ChatStatus, REPLY_HDR};

#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
mod sysabi {
    #[inline(always)]
    pub unsafe fn sys0(n: u64) -> u64 {
        let mut r: u64;
        unsafe { core::arch::asm!("svc #0", out("x0") r, in("x8") n, options(nostack)) };
        r
    }
    #[inline(always)]
    pub unsafe fn sys1(n: u64, a0: u64) -> u64 {
        let mut r: u64;
        unsafe { core::arch::asm!("svc #0", inout("x0") a0 => r, in("x8") n, options(nostack)) };
        r
    }
    #[inline(always)]
    pub unsafe fn sys2(n: u64, a0: u64, a1: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "svc #0",
                inout("x0") a0 => r,
                in("x1") a1,
                in("x8") n,
                options(nostack),
            )
        };
        r
    }
    #[inline(always)]
    pub unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "svc #0",
                inout("x0") a0 => r,
                in("x1") a1,
                in("x2") a2,
                in("x8") n,
                options(nostack),
            )
        };
        r
    }
}

/// x86_64 — U1b B1: the kernel's `sysretq` tail zeroes rdi/rsi/rdx/r8/r9/r10 before returning to ring
/// 3, on EVERY syscall, unconditionally, regardless of how many arguments that syscall took. So every
/// stub below — regardless of its own arity — must declare all six of those registers as NOT
/// surviving the `syscall` instruction: `inlateout(reg) a => _` for whichever ones happen to carry
/// that stub's own arguments, `lateout(reg) _` for the rest. Naming only the registers a given stub
/// passes (and leaving the others unnamed) is the arity mistake this block used to make: the
/// compiler is then free to believe an unnamed register — say `rdx` in a 2-argument stub — survives
/// the call and reuse it, and it does not; it reads back zero. A bare `in(reg)` is worse still,
/// promising rustc the register still holds its value afterward — a promise the kernel breaks — and
/// that is undefined behaviour that surfaces only on real hardware, never in QEMU or `check`.
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
mod sysabi {
    #[inline(always)]
    pub unsafe fn sys0(n: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                lateout("rdi") _,
                lateout("rsi") _,
                lateout("rdx") _,
                lateout("rcx") _,
                lateout("r11") _,
                lateout("r8") _,
                lateout("r9") _,
                lateout("r10") _,
                options(nostack),
            )
        };
        r
    }
    #[inline(always)]
    pub unsafe fn sys1(n: u64, a0: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                inlateout("rdi") a0 => _,
                lateout("rsi") _,
                lateout("rdx") _,
                lateout("rcx") _,
                lateout("r11") _,
                lateout("r8") _,
                lateout("r9") _,
                lateout("r10") _,
                options(nostack),
            )
        };
        r
    }
    #[inline(always)]
    pub unsafe fn sys2(n: u64, a0: u64, a1: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                inlateout("rdi") a0 => _,
                inlateout("rsi") a1 => _,
                lateout("rdx") _,
                lateout("rcx") _,
                lateout("r11") _,
                lateout("r8") _,
                lateout("r9") _,
                lateout("r10") _,
                options(nostack),
            )
        };
        r
    }
    #[inline(always)]
    pub unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                inlateout("rdi") a0 => _,
                inlateout("rsi") a1 => _,
                inlateout("rdx") a2 => _,
                lateout("rcx") _,
                lateout("r11") _,
                lateout("r8") _,
                lateout("r9") _,
                lateout("r10") _,
                options(nostack),
            )
        };
        r
    }
}

#[allow(unused_imports)]
use sysabi::{sys0, sys1, sys2, sys3};

fn write_bytes(b: &[u8]) {
    unsafe { sys3(SYS_WRITE, 1, b.as_ptr() as u64, b.len() as u64) };
}
fn exit(code: i32) -> ! {
    unsafe { sys1(SYS_EXIT, code as u64) };
    loop {
        core::hint::spin_loop();
    }
}

/// The PRIN_KERNEL_REPLY kind: every kernel reply, and every kernel-INJECTED request (`vein rsp`).
const PRIN_KERNEL: u8 = 4;
/// TX: one header + one ChatReply of METAL_CHUNK text (the largest frame this program sends).
const TX_LEN: usize = BUS_HDR_LEN + REPLY_HDR + METAL_CHUNK;

// Frame buffers live in .bss (a whole-frame receive buffer has no business on the stack).
static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
static mut TX: [u8; TX_LEN] = [0; TX_LEN];
static mut CHUNK: [u8; METAL_CHUNK] = [0; METAL_CHUNK];

#[allow(static_mut_refs)]
fn rx() -> &'static mut [u8; BUS_FRAME_MAX] {
    unsafe { &mut RX }
}
#[allow(static_mut_refs)]
fn tx() -> &'static mut [u8; TX_LEN] {
    unsafe { &mut TX }
}
#[allow(static_mut_refs)]
fn chunk() -> &'static mut [u8; METAL_CHUNK] {
    unsafe { &mut CHUNK }
}

/// Write a header into TX (principal zero — the kernel stamps); the body is already at TX[52..52+len].
fn seal(kind: u8, verb: u8, corr: u32, status: i32, len: usize) -> usize {
    let t = tx();
    let len = if status != 0 && status != BUS_STATUS_MORE { 0 } else { len };
    t[0..4].copy_from_slice(&BUS_MAGIC);
    t[4] = BUS_VERSION;
    t[5] = kind;
    t[6] = verb;
    t[7] = 0;
    t[8..12].copy_from_slice(&corr.to_le_bytes());
    t[12..16].copy_from_slice(&status.to_le_bytes());
    t[16..48].fill(0);
    t[48..52].copy_from_slice(&(len as u32).to_le_bytes());
    BUS_HDR_LEN + len
}

/// Send TX[..n]; a full mailbox (-EAGAIN) is retried after a yield — the caller's 16-deep mailbox is the
/// flow control of a long answer. Bounded: a caller that never drains costs a bounded spin, then -EAGAIN.
fn send(n: usize) -> i64 {
    let mut tries = 0u32;
    loop {
        let r = unsafe { sys2(SYS_MSEND, tx().as_ptr() as u64, n as u64) as i64 };
        if r != EAGAIN || tries >= 20_000 {
            return r;
        }
        tries += 1;
        unsafe { sys0(SYS_YIELD) };
    }
}
fn recv() -> i64 {
    unsafe { sys2(SYS_MRECV, rx().as_mut_ptr() as u64, BUS_FRAME_MAX as u64) as i64 }
}

/// Reply on `corr` with `status` and `body` (copied into TX).
fn reply(verb: u8, corr: u32, status: i32, body: &[u8]) -> i64 {
    let n = core::cmp::min(body.len(), TX_LEN - BUS_HDR_LEN);
    tx()[BUS_HDR_LEN..BUS_HDR_LEN + n].copy_from_slice(&body[..n]);
    let n = seal(BUS_KIND_REPLY, verb, corr, status, n);
    send(n)
}

struct Hdr {
    kind: u8,
    verb: u8,
    corr: u32,
    status: i32,
    body_len: usize,
}
fn hdr(n: i64) -> Option<Hdr> {
    let r = rx();
    if n < BUS_HDR_LEN as i64 || r[0..4] != BUS_MAGIC {
        return None;
    }
    let u = |o: usize| u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
    let body_len = core::cmp::min(u(48) as usize, BUS_FRAME_MAX - BUS_HDR_LEN);
    Some(Hdr { kind: r[5], verb: r[6], corr: u(8), status: u(12) as i32, body_len })
}

/// Ask: send a request and wait for its reply. Returns (status, body length in RX).
fn ask(verb: u8, corr: u32, body: &[u8]) -> (i64, usize) {
    tx()[BUS_HDR_LEN..BUS_HDR_LEN + body.len()].copy_from_slice(body);
    let n = seal(BUS_KIND_REQUEST, verb, corr, 0, body.len());
    let s = send(n);
    if s != 0 {
        return (s, 0);
    }
    match hdr(recv()) {
        Some(h) if h.kind == BUS_KIND_REPLY && h.corr == corr => (h.status as i64, h.body_len),
        _ => (-5, 0),
    }
}

/// The ProviderIo of one answer: each ChatReply becomes a REPLY frame on the caller's corr.
struct Io {
    corr: u32,
    frames: u32,
}
impl ProviderIo for Io {
    fn reply(&mut self, r: &ChatReply<'_>) -> bool {
        let ok = match r.encode(&mut tx()[BUS_HDR_LEN..]) {
            Ok(n) => {
                let n = seal(BUS_KIND_REPLY, BUS_VERB_CHAT_SEND, self.corr, r.status(), n);
                send(n) == 0
            }
            Err(_) => false,
        };
        self.frames += ok as u32;
        ok
    }
    fn line(&mut self, bytes: &[u8]) {
        write_bytes(bytes);
    }
}

struct Line {
    b: [u8; 120],
    n: usize,
}
impl Line {
    fn new() -> Self {
        Line { b: [0; 120], n: 0 }
    }
    fn put(&mut self, s: &[u8]) -> &mut Self {
        for &c in s {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }
    fn dec(&mut self, v: i64) -> &mut Self {
        if v < 0 {
            self.put(b"-");
        }
        let mut d = [0u8; 10];
        let s = vein_core::wire::dec_u32(v.unsigned_abs() as u32, &mut d);
        let mut t = [0u8; 10];
        let k = s.len();
        t[..k].copy_from_slice(s);
        self.put(&t[..k])
    }
    fn flush(&self) {
        write_bytes(&self.b[..self.n]);
    }
}

/// The one in-flight relay (v1: one at a time): (conv, the caller's corr).
static mut PEND: Option<(u32, u32)> = None;

#[allow(static_mut_refs)]
fn pend() -> &'static mut Option<(u32, u32)> {
    unsafe { &mut PEND }
}

/// Serve the request sitting in RX. Returns (status answered, frames sent, conv).
fn serve(h: &Hdr, choice: Choice) -> (i32, u32, u32) {
    let r = rx();
    let body = &r[BUS_HDR_LEN..BUS_HDR_LEN + h.body_len];
    match h.verb {
        BUS_VERB_CHAT_SEND => {
            let Ok(s) = ChatSend::decode(body) else {
                reply(h.verb, h.corr, EINVAL as i32, &[]);
                return (EINVAL as i32, 0, 0);
            };
            let mut io = Io { corr: h.corr, frames: 0 };
            let begun = match choice {
                Choice::Echo => Echo { chunk: chunk() }.begin(s.conv, s.text, &mut io),
                Choice::Relay if pend().is_some() => Begin::Err(EAGAIN as i32), // v1: one relay in flight
                Choice::Relay => Relay.begin(s.conv, s.text, &mut io),
            };
            match begun {
                Begin::Done { .. } => (0, io.frames, s.conv),
                Begin::Pending => {
                    *pend() = Some((s.conv, h.corr));
                    (BUS_STATUS_MORE, 0, s.conv)
                }
                Begin::Err(e) => {
                    reply(h.verb, h.corr, e, &[]);
                    (e, io.frames, s.conv)
                }
            }
        }
        BUS_VERB_CHAT_REPLY => {
            // Only the KERNEL may hand VEIN.BIN a relay answer (the injected `vein rsp` line).
            if r[16] != PRIN_KERNEL || r[17..48].iter().any(|&b| b != 0) {
                reply(h.verb, h.corr, EACCES as i32, &[]);
                return (EACCES as i32, 0, 0);
            }
            match (ChatReply::decode(body), *pend()) {
                (Ok(c), Some((conv, corr))) if c.conv == conv => {
                    let mut io = Io { corr, frames: 0 };
                    io.reply(&c);
                    if c.done {
                        *pend() = None;
                    }
                    (c.status(), io.frames, conv)
                }
                _ => (ENOENT as i32, 0, 0), // nothing waiting for it: dropped (corr 0 — no reply owed)
            }
        }
        BUS_VERB_CHAT_CANCEL => match ChatCancel::decode(body) {
            Ok(c) => {
                if let Some((conv, corr)) = *pend() {
                    if conv == c.conv {
                        *pend() = None;
                        reply(BUS_VERB_CHAT_SEND, corr, ECANCELED as i32, &[]);
                    }
                }
                reply(h.verb, h.corr, 0, &[]);
                (0, 0, c.conv)
            }
            Err(_) => {
                reply(h.verb, h.corr, EINVAL as i32, &[]);
                (EINVAL as i32, 0, 0)
            }
        },
        BUS_VERB_CHAT_STATUS if status_request_ok(body) => {
            let (p, m) = match choice {
                Choice::Echo => ("echo", "reverse"),
                Choice::Relay => (Relay.name(), Relay.model()),
            };
            let mut b = [0u8; 40];
            let n = ChatStatus { ready: true, provider: p.as_bytes(), model: m.as_bytes() }.encode(&mut b).unwrap_or(0);
            reply(h.verb, h.corr, 0, &b[..n]);
            (0, 1, 0)
        }
        _ => {
            reply(h.verb, h.corr, EINVAL as i32, &[]);
            (EINVAL as i32, 0, 0)
        }
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    // 1. Principia's choice (PREFS answers in-kernel; a TOML string comes back quoted).
    let (ps, plen) = ask(BUS_VERB_PREF_GET, 1, b"vein.provider");
    let choice = choose(if ps == 0 { Some(&rx()[BUS_HDR_LEN..BUS_HDR_LEN + plen]) } else { None });
    let pname: &[u8] = match choice {
        Choice::Echo => b"echo",
        Choice::Relay => b"relay",
    };
    // 2. own the four chat verbs.
    let (register, _) = ask(BUS_VERB_REGISTER, 2, &[BUS_VERB_CHAT_SEND, BUS_VERB_CHAT_REPLY, BUS_VERB_CHAT_CANCEL, BUS_VERB_CHAT_STATUS]);
    let mut l = Line::new();
    l.put(b":: VEIN: registered=").dec(if register == 0 { 4 } else { register }).put(b" provider=").put(pname).put(b" served=0 frames=0");
    l.put(if register == 0 { b" -> PASS ::\n".as_slice() } else { b" -> FAIL ::\n".as_slice() });
    l.flush();
    if register != 0 {
        exit(2); // nothing registered (knob off: the tag is -EINVAL; another owner: -EEXIST)
    }
    // 3. serve forever. A REPLY landing here is stale and dropped.
    let (mut served, mut frames) = (0u32, 0u32);
    loop {
        let Some(h) = hdr(recv()) else { continue };
        if h.kind != BUS_KIND_REQUEST {
            continue;
        }
        let (status, f, conv) = serve(&h, choice);
        served += 1;
        frames += f;
        let mut l = Line::new();
        l.put(b":: VEIN: served=").dec(served as i64).put(b" frames=").dec(frames as i64).put(b" verb=").dec(h.verb as i64);
        l.put(b" conv=").dec(conv as i64).put(b" status=").dec(status as i64).put(b" ::\n");
        l.flush();
        let _ = h.status;
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(3)
}
