#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ABIELF (rmbp-ledger B693, BP-bar-13): ABIPROBE.ELF — an ELF built at una-abi vN runs unchanged on later builds.
//
// This program is built ONCE per ABI version and its bytes are COMMITTED (unaos/fixtures/ABIPROBE-v<N>.ELF, with
// the .sha256 beside it). The builder stages those bytes as APPS/ABIPROBE.ELF; `tests abielf` runs them. Nothing
// here is rebuilt by a later tree, so a later kernel that re-means a number, moves an argument or drops a verb
// answers this binary wrongly and the fixture reds. That is the gate.
//
// One call per stable family, each judged against its CONTRACT (una-abi's doc on the number), not against what
// the environment happens to hold (a volume with no settings file, a headless panel, an unset preference):
//
//   file   1 SYS_OPEN  `ABIPROBE.CFG` O_RW|O_CREAT  2 SYS_WRITE `abi=<N>\n`  3 SYS_READ  4 SYS_CLOSE
//          — the program's own settings file. A refused open (no such name on this volume) leaves the three
//          handle verbs to answer on a handle never held: WRITE/READ `-EACCES`, CLOSE `-EBADF` (their contract).
//   window 5 SYS_WIN_CREATE 64x48  6 SYS_WIN_PRESENT (0 or 1, BP-ring3-9)  7 SYS_WIN_CLOSE
//          — x86 reserves 32 (`-ENOSYS`, una-abi's matrix; slot teardown closes the window): 0 or -ENOSYS.
//   bus    8 SYS_MSEND BUS_VERB_ATTR_GET `/apps/ABIPROBE.ELF` attr_keys::VERSION  9 SYS_MRECV its reply
//   input 10 SYS_INPUT_POLL — the non-blocking (zero-timeout) read of this process's ring: an event or -EAGAIN.
//   prefs 11 SYS_MSEND BUS_VERB_PREF_GET `system.pointer.speed` (the verb `prefs_client` speaks) 12 SYS_MRECV
//
// "Answered" (`answered`): a success value, or a negative errno that is NOT -ENOSYS (the number left the table),
// -EFAULT or -EINVAL (a well-formed call misread: the argument layout moved). A bus reply must echo the verb and
// the corr; its status is judged the same way.
//
// Wire: one detail line `ABIPROBE: open=<rc> write=<rc> ... prefs=<status>` then
// `:: ABIPROBE: abi=<N> syscalls=<n> ok=<n> -> PASS|FAIL ::`.
// Exit status: 1<<24 (the probe ran) | abi<<16 | syscalls<<8 | ok — `tests abielf` judges from it.
use una_abi::{
    ABI_VERSION, BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_KIND_REQUEST, BUS_MAGIC, BUS_VERB_ATTR_GET,
    BUS_VERB_PREF_GET, BUS_VERSION, EACCES, EAGAIN, EBADF, EFAULT, EINVAL, ENOSYS, O_CREAT, O_RW, SYS_CLOSE, SYS_EXIT,
    SYS_INPUT_POLL, SYS_MRECV, SYS_MSEND, SYS_OPEN, SYS_READ, SYS_WIN_CLOSE, SYS_WIN_CREATE, SYS_WIN_PRESENT, SYS_WRITE,
};

/// x86_64: the kernel's `sysretq` tail scrubs rdi/rsi/rdx/r8/r9/r10 on every return and `syscall` destroys rcx/r11,
/// so every stub declares all eight whatever its arity (una-abi's calling-convention note; the user-prefs stubs).
#[inline(always)]
unsafe fn sys0(n: u64) -> i64 {
    let r: u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") n => r, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _,
            lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _, lateout("r10") _, options(nostack))
    };
    r as i64
}
#[inline(always)]
unsafe fn sys1(n: u64, a0: u64) -> i64 {
    let r: u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") n => r, inlateout("rdi") a0 => _, lateout("rsi") _,
            lateout("rdx") _, lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _,
            lateout("r10") _, options(nostack))
    };
    r as i64
}
#[inline(always)]
unsafe fn sys2(n: u64, a0: u64, a1: u64) -> i64 {
    let r: u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") n => r, inlateout("rdi") a0 => _, inlateout("rsi") a1 => _,
            lateout("rdx") _, lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _,
            lateout("r10") _, options(nostack))
    };
    r as i64
}
#[inline(always)]
unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let r: u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") n => r, inlateout("rdi") a0 => _, inlateout("rsi") a1 => _,
            inlateout("rdx") a2 => _, lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _,
            lateout("r10") _, options(nostack))
    };
    r as i64
}

fn exit(code: u64) -> ! {
    unsafe { sys1(SYS_EXIT, code) };
    loop {
        core::hint::spin_loop();
    }
}

/// The console handle (index 1, endowed at launch — the CONSOLE_FD every program writes its line to).
const CONSOLE: u64 = 1;
/// A handle index no process holds (out of every row's range): the handle verbs' refusal contract.
const NOT_HELD: u64 = 0xFFFF;
/// The program's own settings file (8.3, the SYS_OPEN name bound).
const CFG: &[u8] = b"ABIPROBE.CFG";
const ATTR_PATH: &[u8] = b"/apps/ABIPROBE.ELF";
const ATTR_KEY: &[u8] = una_abi::attr_keys::VERSION.as_bytes();
const PREF_KEY: &[u8] = b"system.pointer.speed";

/// A contract answer: success, or an errno that is not "no such verb" and not "your arguments are malformed".
fn answered(rc: i64) -> bool {
    rc >= 0 || (rc >= -4095 && rc != ENOSYS && rc != EFAULT && rc != EINVAL)
}

/// ONE frame buffer in .bss: requests are built in it and replies land in it (strictly half-duplex).
static mut FRAME: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];

#[allow(static_mut_refs)]
fn frame() -> &'static mut [u8; BUS_FRAME_MAX] {
    unsafe { &mut FRAME }
}

/// Send one v1 REQUEST (status 0, principal zero — the kernel stamps) and receive its reply.
/// Returns `(msend rc, mrecv rc, reply status)`; the status is meaningful only when the reply echoed verb + corr.
fn ask(verb: u8, corr: u32, body: &[u8]) -> (i64, i64, Option<i64>) {
    let f = frame();
    let bl = body.len().min(BUS_FRAME_MAX - BUS_HDR_LEN);
    f[0..4].copy_from_slice(&BUS_MAGIC);
    f[4] = BUS_VERSION;
    f[5] = BUS_KIND_REQUEST;
    f[6] = verb;
    f[7] = 0;
    f[8..12].copy_from_slice(&corr.to_le_bytes());
    f[12..48].fill(0);
    f[48..52].copy_from_slice(&(bl as u32).to_le_bytes());
    f[BUS_HDR_LEN..BUS_HDR_LEN + bl].copy_from_slice(&body[..bl]);
    let s = unsafe { sys2(SYS_MSEND, f.as_ptr() as u64, (BUS_HDR_LEN + bl) as u64) };
    if s != 0 {
        return (s, i64::MIN, None);
    }
    // flags 0: the original two-argument contract (block until the reply; the kernel fulfils before MSEND returns).
    let n = unsafe { sys3(SYS_MRECV, f.as_mut_ptr() as u64, BUS_FRAME_MAX as u64, 0) };
    let u = |o: usize| u32::from_le_bytes([f[o], f[o + 1], f[o + 2], f[o + 3]]);
    let echo = n >= BUS_HDR_LEN as i64 && f[0..4] == BUS_MAGIC && f[5] == BUS_KIND_REPLY && f[6] == verb && u(8) == corr;
    (s, n, if echo { Some(u(12) as i32 as i64) } else { None })
}

struct Line {
    b: [u8; 256],
    n: usize,
}
impl Line {
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
        loop {
            i -= 1;
            d[i] = b'0' + (u % 10) as u8;
            u /= 10;
            if u == 0 {
                break;
            }
        }
        self.put(&d[i..]);
    }
    fn kv(&mut self, k: &[u8], v: i64) {
        self.put(b" ");
        self.put(k);
        self.put(b"=");
        self.dec(v);
    }
    fn flush(&mut self) {
        unsafe { sys3(SYS_WRITE, CONSOLE, self.b.as_ptr() as u64, self.n as u64) };
        self.n = 0;
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let mut ok = 0u64;
    let mut n = 0u64;
    let mut tick = |good: bool| {
        n += 1;
        ok += good as u64;
    };
    let mut l = Line { b: [0; 256], n: 0 };
    l.put(b"ABIPROBE:");

    // --- file: open / write / read / close on the program's own settings file ---
    let open = unsafe { sys3(SYS_OPEN, CFG.as_ptr() as u64, CFG.len() as u64, O_RW | O_CREAT) };
    tick(answered(open));
    let held = open >= 0;
    let h = if held { open as u64 } else { NOT_HELD };
    let mut v = [b'a', b'b', b'i', b'=', b'0' + (ABI_VERSION % 10) as u8, b'\n'];
    let write = unsafe { sys3(SYS_WRITE, h, v.as_ptr() as u64, v.len() as u64) };
    tick(if held { answered(write) } else { write == EACCES });
    let read = unsafe { sys3(SYS_READ, h, v.as_mut_ptr() as u64, v.len() as u64) };
    tick(if held { answered(read) } else { read == EACCES });
    let close = unsafe { sys1(SYS_CLOSE, h) };
    tick(if held { close == 0 } else { close == EBADF });
    l.kv(b"open", open);
    l.kv(b"write", write);
    l.kv(b"read", read);
    l.kv(b"close", close);

    // --- window: create / present / close ---
    let win = unsafe { sys2(SYS_WIN_CREATE, 64, 48) };
    tick(answered(win));
    let w = if win >= 0 { win as u64 } else { NOT_HELD };
    let present = unsafe { sys1(SYS_WIN_PRESENT, w) };
    tick(if win >= 0 { present == 0 || present == 1 } else { answered(present) });
    let wclose = unsafe { sys1(SYS_WIN_CLOSE, w) };
    tick(wclose == 0 || wclose == ENOSYS || (win < 0 && answered(wclose)));
    l.kv(b"win", win);
    l.kv(b"present", present);
    l.kv(b"wclose", wclose);

    // --- bus: one GET verb round trip (ATTR_GET: [path_len u16][key_len u16][path][key]) ---
    let mut body = [0u8; 4 + 18 + 11];
    body[0..2].copy_from_slice(&(ATTR_PATH.len() as u16).to_le_bytes());
    body[2..4].copy_from_slice(&(ATTR_KEY.len() as u16).to_le_bytes());
    body[4..4 + ATTR_PATH.len()].copy_from_slice(ATTR_PATH);
    body[4 + ATTR_PATH.len()..].copy_from_slice(ATTR_KEY);
    let (s, r, st) = ask(BUS_VERB_ATTR_GET, 0xAB1, &body);
    tick(s == 0);
    tick(s == 0 && st.is_some_and(answered));
    l.kv(b"msend", s);
    l.kv(b"mrecv", r);
    l.kv(b"attr", st.unwrap_or(i64::MIN));

    // --- input: the zero-timeout read of this process's ring ---
    let input = unsafe { sys0(SYS_INPUT_POLL) };
    tick(input >= 0 || input == EAGAIN);
    l.kv(b"input", input);

    // --- prefs: PREF_GET, the verb the kernel's prefs_client speaks to Principia's store ---
    let (s, r, st) = ask(BUS_VERB_PREF_GET, 0xAB2, PREF_KEY);
    tick(s == 0);
    tick(s == 0 && st.is_some_and(answered));
    l.kv(b"pmsend", s);
    l.kv(b"pmrecv", r);
    l.kv(b"prefs", st.unwrap_or(i64::MIN));
    l.put(b"\n");
    l.flush();

    l.put(b":: ABIPROBE:");
    l.kv(b"abi", ABI_VERSION as i64);
    l.kv(b"syscalls", n as i64);
    l.kv(b"ok", ok as i64);
    l.put(if ok == n { b" -> PASS ::\n".as_slice() } else { b" -> FAIL ::\n".as_slice() });
    l.flush();
    exit(1 << 24 | (ABI_VERSION as u64 & 0xFF) << 16 | (n & 0xFF) << 8 | (ok & 0xFF))
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(0xF0)
}

/// EXECNAME (B322, R82): a console program (its window is a probe, not a face) — a bare `abiprobe` runs in the
/// foreground like `run`, and APPLAUNCH's windowed pick never chooses it.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(0);
