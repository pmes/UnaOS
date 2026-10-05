#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BANDY3 M3: PREFS.ELF — the first ring-3 FULFILLER (ROADMAP §3b principle 3: a command is addressable
// the same way whether fulfilled in-kernel, by a handler, or by a spawned vessel). Principia owns every
// preference decision (LAWS §Handler manifest); this is Principia's read side on the metal, as a ring-3
// program that OWNS two bus verbs instead of a second store inside the kernel.
//
// What it does, in order:
//   1. probe  — asks PrefGet BEFORE registering: with no fulfiller the kernel answers -ENOENT (no hang).
//   2. register PrefGet + PrefList (BUS_VERB_REGISTER) — 0.
//   3. register LS — refused -EEXIST: a kernel-owned verb cannot be taken over (kernel fulfilment wins).
//   4. self_get — asks PrefGet `ui.theme` of ITSELF through the kernel: the relayed request arrives in
//      its own mailbox carrying a KERNEL-stamped principal (never the caller's claim), it answers it, and
//      the answer comes back as a KERNEL-stamped reply with the original corr.
//   then prints ONE line and SERVES forever: every relayed request is answered from the table below.
//
// THE PRINCIPAL RULE. The relayed header names the CALLER. This program may read it (it prints it per
// request); it never acts AS the caller — any file it reads, it reads under its OWN grants.
//
// SETTINGSBUS (B337): it registers the write tag too (R3PREF_SET 130) and answers all three from the ONE
// store — Principia's TOML at `<home>/.config/unaos/preferences.toml`, which the kernel keeps through
// `prefs_core` — by asking the kernel's store verbs (PREF_GET/SET/LIST 16..=18) under its OWN principal. The
// kernel's Settings window is a bus client (`prefs_client.rs`) and reaches the store THROUGH this program
// while it runs. `TABLE` is the fallback for its own demo keys. PrefChanged to ring 3 is OWED.
//
// ---------------------------------------------------------------------------------------------
// Syscall stubs — the user-pulse stubs verbatim (see that crate for the register-clobber contract).
// ---------------------------------------------------------------------------------------------
use una_abi::{
    BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_KIND_REQUEST, BUS_MAGIC, BUS_VERB_LS, BUS_VERB_PREF_GET,
    BUS_VERB_PREF_LIST, BUS_VERB_PREF_SET, BUS_VERB_R3PREF_GET, BUS_VERB_R3PREF_LIST, BUS_VERB_R3PREF_SET, BUS_VERB_REGISTER,
    BUS_VERSION, ENOENT, SYS_EXIT, SYS_MRECV, SYS_MSEND, SYS_WRITE,
};

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

/// The v1 answer table — Principia's dotted keys, TOML scalar text. OWED: `prefs_core` replaces it.
const TABLE: &[(&[u8], &[u8])] = &[
    (b"ui.theme", b"dark"),
    (b"ui.font_scale", b"1"),
    (b"input.pointer_speed", b"5"),
    (b"power.idle_blank_secs", b"300"),
    (b"audio.volume", b"70"),
];

/// The PRIN_KERNEL_REPLY kind every kernel reply carries.
const PRIN_KERNEL_REPLY: u8 = 4;

// Frame buffers live in .bss (a whole-frame receive buffer has no business on the stack).
static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
static mut TX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];

#[allow(static_mut_refs)]
fn rx() -> &'static mut [u8; BUS_FRAME_MAX] {
    unsafe { &mut RX }
}
#[allow(static_mut_refs)]
fn tx() -> &'static mut [u8; BUS_FRAME_MAX] {
    unsafe { &mut TX }
}

/// Build a frame into TX; returns its length (body truncated to the TX buffer, never past it).
fn build(kind: u8, verb: u8, corr: u32, status: i32, body: &[u8]) -> usize {
    let t = tx();
    let body = if status != 0 { &[][..] } else { &body[..core::cmp::min(body.len(), t.len() - BUS_HDR_LEN)] };
    t[0..4].copy_from_slice(&BUS_MAGIC);
    t[4] = BUS_VERSION;
    t[5] = kind;
    t[6] = verb;
    t[7] = 0;
    t[8..12].copy_from_slice(&corr.to_le_bytes());
    t[12..16].copy_from_slice(&status.to_le_bytes());
    for b in &mut t[16..48] {
        *b = 0; // the kernel stamps; a caller-supplied principal is refused
    }
    t[48..52].copy_from_slice(&(body.len() as u32).to_le_bytes());
    t[BUS_HDR_LEN..BUS_HDR_LEN + body.len()].copy_from_slice(body);
    BUS_HDR_LEN + body.len()
}

fn send(n: usize) -> i64 {
    unsafe { sys2(SYS_MSEND, tx().as_ptr() as u64, n as u64) as i64 }
}
fn recv() -> i64 {
    unsafe { sys2(SYS_MRECV, rx().as_mut_ptr() as u64, BUS_FRAME_MAX as u64) as i64 }
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
    Some(Hdr { kind: r[5], verb: r[6], corr: u(8), status: u(12) as i32, body_len: u(48) as usize })
}

/// Ask: send a request and wait for the frame that comes back (status from the reply).
fn ask(verb: u8, corr: u32, body: &[u8]) -> i64 {
    let n = build(BUS_KIND_REQUEST, verb, corr, 0, body);
    let s = send(n);
    if s != 0 {
        return s;
    }
    match hdr(recv()) {
        Some(h) if h.kind == BUS_KIND_REPLY && h.corr == corr => h.status as i64,
        _ => -5,
    }
}

/// Answer the relayed request sitting in RX. Returns the status it answered with.
///
/// SETTINGSBUS (B337): Principia's answers come from the ONE store — each relayed request is asked of the
/// kernel's store verbs (PREF_GET / PREF_SET / PREF_LIST, 16..=18) from HERE, under THIS program's own
/// session principal (a fulfiller never acts as its caller). The fixed v1 `TABLE` stays as the fallback
/// for its own demo keys (an unset `ui.theme` still answers `dark`).
fn serve(h: &Hdr) -> i32 {
    // The relayed request is copied out of RX first: the forward below receives into RX.
    let mut req = [0u8; 1024];
    let rl = core::cmp::min(h.body_len, req.len());
    req[..rl].copy_from_slice(&rx()[BUS_HDR_LEN..BUS_HDR_LEN + rl]);
    let mut caller = [0u8; 32];
    caller.copy_from_slice(&rx()[16..48]);
    let body = &req[..rl];
    let mut len = 0usize;
    let status: i32 = match h.verb {
        BUS_VERB_R3PREF_GET => match forward(BUS_VERB_PREF_GET, body) {
            (0, n) => {
                len = n;
                0
            }
            _ => match TABLE.iter().find(|(k, _)| *k == body) {
                Some((_, v)) => {
                    outb()[..v.len()].copy_from_slice(v);
                    len = v.len();
                    0
                }
                None => ENOENT as i32,
            },
        },
        BUS_VERB_R3PREF_SET => forward(BUS_VERB_PREF_SET, body).0,
        BUS_VERB_R3PREF_LIST => {
            // A namespace (no dot) or empty asks the store; a dotted prefix is the v1 table's form.
            if !body.contains(&b'.') {
                if let (0, n) = forward(BUS_VERB_PREF_LIST, body) {
                    len = n;
                }
            }
            for (k, v) in TABLE.iter().filter(|(k, _)| k.starts_with(body)) {
                for part in [*k, b"=".as_slice(), *v, b"\n".as_slice()] {
                    if len + part.len() <= outb().len() {
                        outb()[len..len + part.len()].copy_from_slice(part);
                        len += part.len();
                    }
                }
            }
            0
        }
        _ => ENOENT as i32,
    };
    let r = &caller;
    // Show the stamped caller (principal value bytes, printable only) — visible, never borrowed.
    let mut line = Line::new();
    line.put(b":: PREFS: served verb=");
    line.dec(h.verb as i64);
    line.put(b" caller_kind=");
    line.dec(r[0] as i64);
    line.put(b" caller=");
    let plen = core::cmp::min(r[1] as usize, 30);
    for &c in &r[2..2 + plen] {
        line.put(&[if (0x21..0x7f).contains(&c) { c } else { b'.' }]);
    }
    line.put(b" status=");
    line.dec(status as i64);
    line.put(b" ::\n");
    let n = build(BUS_KIND_REPLY, h.verb, h.corr, status, &outb()[..len]);
    let _ = send(n);
    line.flush();
    status
}

static mut OUT: [u8; BUS_FRAME_MAX - BUS_HDR_LEN] = [0; BUS_FRAME_MAX - BUS_HDR_LEN];
#[allow(static_mut_refs)]
fn outb() -> &'static mut [u8; BUS_FRAME_MAX - BUS_HDR_LEN] {
    unsafe { &mut OUT }
}
static mut FWD_CORR: u32 = 1000;

/// SETTINGSBUS: ask the kernel's store verb `verb` with `body`; the reply body lands in OUT. Returns
/// `(status, body length)`. A relayed request that arrives while waiting is answered -EAGAIN (its caller
/// falls back to the kernel fulfiller), so the wait is never a deadlock.
#[allow(static_mut_refs)]
fn forward(verb: u8, body: &[u8]) -> (i32, usize) {
    let corr = unsafe {
        FWD_CORR = FWD_CORR.wrapping_add(1).max(1000);
        FWD_CORR
    };
    let n = build(BUS_KIND_REQUEST, verb, corr, 0, body);
    let s = send(n);
    if s != 0 {
        return (s as i32, 0);
    }
    loop {
        let Some(h) = hdr(recv()) else { return (-5, 0) };
        if h.kind == BUS_KIND_REPLY && h.corr == corr {
            let bl = core::cmp::min(h.body_len, outb().len());
            outb()[..bl].copy_from_slice(&rx()[BUS_HDR_LEN..BUS_HDR_LEN + bl]);
            return (h.status, if h.status == 0 { bl } else { 0 });
        }
        if h.kind == BUS_KIND_REQUEST {
            let m = build(BUS_KIND_REPLY, h.verb, h.corr, -11, &[]);
            let _ = send(m);
        }
    }
}

struct Line {
    b: [u8; 200],
    n: usize,
}
impl Line {
    fn new() -> Self {
        Line { b: [0; 200], n: 0 }
    }
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn dec(&mut self, v: i64) {
        if v < 0 {
            self.put(b"-");
        }
        let mut u = v.unsigned_abs();
        let mut d = [0u8; 20];
        let mut i = d.len();
        if u == 0 {
            i -= 1;
            d[i] = b'0';
        }
        while u > 0 {
            i -= 1;
            d[i] = b'0' + (u % 10) as u8;
            u /= 10;
        }
        self.put(&d[i..]);
    }
    fn flush(&self) {
        write_bytes(&self.b[..self.n]);
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    // 1. probe: no fulfiller yet -> -ENOENT, an answer and not a hang. (Knob off: the tag is refused at
    //    SYS_MSEND with -EINVAL, and the program says so and exits — nothing to serve.)
    let probe = ask(BUS_VERB_R3PREF_GET, 1, b"ui.theme");
    // 2. register Principia's two read verbs.
    let register = ask(BUS_VERB_REGISTER, 2, &[BUS_VERB_R3PREF_GET, BUS_VERB_R3PREF_LIST, BUS_VERB_R3PREF_SET]); // SETTINGSBUS: + the write tag
    // 3. a kernel-owned verb cannot be taken over.
    let kernel_tag = ask(BUS_VERB_REGISTER, 3, &[BUS_VERB_LS]);
    // 4. self_get: the relayed request arrives here with a kernel stamp; answer it; read the answer.
    let mut self_get: i64 = -5;
    let mut stamp_kernel = false;
    let mut stamped_caller = false;
    if register == 0 {
        let n = build(BUS_KIND_REQUEST, BUS_VERB_R3PREF_GET, 4, 0, b"ui.theme");
        if send(n) == 0 {
            if let Some(h) = hdr(recv()) {
                if h.kind == BUS_KIND_REQUEST && h.verb == BUS_VERB_R3PREF_GET {
                    stamped_caller = rx()[16] != 0; // the kernel wrote the caller's principal
                    serve(&h);
                    if let Some(r) = hdr(recv()) {
                        let x = rx();
                        stamp_kernel = r.kind == BUS_KIND_REPLY && r.corr == 4 && x[16] == PRIN_KERNEL_REPLY && x[17..48].iter().all(|&b| b == 0);
                        self_get = if r.status == 0 && &x[BUS_HDR_LEN..BUS_HDR_LEN + r.body_len] == b"dark" { 0 } else { r.status as i64 };
                    }
                }
            }
        }
    }
    let pass = probe == ENOENT && register == 0 && kernel_tag == una_abi::EEXIST && self_get == 0 && stamp_kernel && stamped_caller;
    let mut l = Line::new();
    l.put(b":: PREFS: probe=");
    l.dec(probe);
    l.put(b" register=");
    l.dec(register);
    l.put(b" kernel_tag=");
    l.dec(kernel_tag);
    l.put(b" self_get=");
    l.dec(self_get);
    l.put(if stamp_kernel && stamped_caller { b" stamp=kernel".as_slice() } else { b" stamp=bad".as_slice() });
    l.put(if pass { b" -> PASS ::\n".as_slice() } else { b" -> FAIL ::\n".as_slice() });
    l.flush();
    if register != 0 {
        exit(2); // nothing registered — nothing to serve
    }
    // Serve forever. A REPLY landing here is stale (nothing else is asked) and is dropped.
    loop {
        if let Some(h) = hdr(recv()) {
            if h.kind == BUS_KIND_REQUEST {
                serve(&h);
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(3)
}

/// EXECNAME (B322, R82): this program's launch declaration — it stays running as Principia's bus fulfiller, so a bare `prefs` detaches like `bg`.
/// Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_RESIDENT);
