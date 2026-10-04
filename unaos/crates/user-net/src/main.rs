#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// NETRING3 M3 (rmbp-ledger B306): NET.ELF — a ring-3 TCP client on the rMBP, the first rung of Vein on
// the metal reaching `https://api.anthropic.com`.
//
//   1. entropy  — SYS_GETRANDOM(32): the kernel DRBG answers 32 bytes (the TLS client's RNG to be).
//   2. resolve  — SYS_RESOLVE("api.anthropic.com"): the kernel resolver (DHCP-leased DNS).
//   3. connect  — SYS_SOCKET(AF_INET, STREAM) + SYS_CONNECT :80, polled (EINPROGRESS -> sleep, re-call).
//   4. http     — `GET / HTTP/1.0` through the blocking adapter; prints the status line.
//   5. tls      — v1: `tls=skip reason=window`. embedded-tls 0.19 builds for this target (see
//                 ../tls-spike) but needs about 100 KiB of code and record buffers; the ring-3 window is
//                 16 KiB. The adapter below is the transport it will run over, unchanged.
//
// One witness line: `:: NETRING3: resolve=<ip> connect=<0|err> http=<status> tls=<skip|...> -> PASS|SKIP|FAIL ::`.
// SKIP (never FAIL) when the resolve finds no link or no answer: a dark NIC is not a broken program.
// There is no argv for ring-3 programs yet, so the host is fixed (OWED: `NET.ELF tls <host>`).
//
// ---------------------------------------------------------------------------------------------
// Syscall stubs — the user-prefs stubs verbatim (see user-pulse for the register-clobber contract).
// ---------------------------------------------------------------------------------------------
use una_abi::{
    EAGAIN, EINPROGRESS, ENODEV, ENOENT, RESOLVE_OUT_LEN, SYS_CLOSE, SYS_CONNECT, SYS_EXIT, SYS_GETRANDOM,
    SYS_RESOLVE, SYS_SEND, SYS_SLEEP_MS, SYS_SOCKET, SYS_SOCK_RECV, SYS_WRITE,
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

const HOST: &[u8] = b"api.anthropic.com";
const PORT: u16 = 80;
/// Connect polls (x 50 ms) before giving up: 10 s.
const CONNECT_TRIES: u32 = 200;
/// Idle polls (x 10 ms) a blocking read/write waits for progress: 10 s.
const IO_IDLE_TRIES: u32 = 1000;

fn write_bytes(b: &[u8]) {
    unsafe { sys3(SYS_WRITE, 1, b.as_ptr() as u64, b.len() as u64) };
}
fn sleep_ms(ms: u64) {
    unsafe { sys1(SYS_SLEEP_MS, ms) };
}
fn exit(code: i32) -> ! {
    unsafe { sys1(SYS_EXIT, code as u64) };
    loop {
        core::hint::spin_loop();
    }
}

/// A tiny line builder — no `core::fmt` (it would not fit the window's code page budget).
struct Line {
    buf: [u8; 192],
    len: usize,
}
impl Line {
    fn new() -> Self {
        Line { buf: [0; 192], len: 0 }
    }
    fn s(&mut self, b: &[u8]) -> &mut Self {
        for &c in b {
            if self.len < self.buf.len() {
                self.buf[self.len] = c;
                self.len += 1;
            }
        }
        self
    }
    fn u(&mut self, mut v: u64) -> &mut Self {
        let mut t = [0u8; 20];
        let mut i = t.len();
        loop {
            i -= 1;
            t[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&t[i..])
    }
    fn i(&mut self, v: i64) -> &mut Self {
        if v < 0 {
            self.s(b"-").u(v.unsigned_abs())
        } else {
            self.u(v as u64)
        }
    }
    fn ip(&mut self, ip: &[u8]) -> &mut Self {
        self.u(ip[0] as u64).s(b".").u(ip[1] as u64).s(b".").u(ip[2] as u64).s(b".").u(ip[3] as u64)
    }
    fn print(&mut self) {
        self.s(b"\n");
        write_bytes(&self.buf[..self.len]);
    }
}
// ---------------------------------------------------------------------------------------------
// The blocking transport adapter. The kernel's socket verbs are NON-BLOCKING (the IF-masked handler
// never blocks): SYS_SOCK_RECV answers -EAGAIN when nothing is queued and 0 at clean end-of-stream,
// SYS_SEND may queue fewer bytes than asked or answer -EAGAIN when the tx ring is full. A TLS client
// (embedded-tls's `blocking::TlsConnection`) wants the embedded-io 0.7 contract instead: `read` blocks
// until at least one byte and returns 0 ONLY at end-of-stream; `write` returns at least one byte or an
// error. This type IS that contract over the syscalls (EAGAIN -> sleep 10 ms and re-call, bounded by
// IO_IDLE_TRIES so a dead peer is an error, not a hang); `impl embedded_io::{Read, Write}` is a two-line
// forward once the crate is admitted.
// ---------------------------------------------------------------------------------------------
struct Sock {
    h: u64,
}
impl Sock {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, i64> {
        for _ in 0..IO_IDLE_TRIES {
            let r = unsafe { sys3(SYS_SOCK_RECV, self.h, buf.as_mut_ptr() as u64, buf.len() as u64) } as i64;
            if r == EAGAIN {
                sleep_ms(10);
                continue;
            }
            return if r < 0 { Err(r) } else { Ok(r as usize) };
        }
        Err(EAGAIN)
    }
    fn write(&mut self, buf: &[u8]) -> Result<usize, i64> {
        for _ in 0..IO_IDLE_TRIES {
            let r = unsafe { sys3(SYS_SEND, self.h, buf.as_ptr() as u64, buf.len() as u64) } as i64;
            if r == EAGAIN || r == 0 {
                sleep_ms(10);
                continue;
            }
            return if r < 0 { Err(r) } else { Ok(r as usize) };
        }
        Err(EAGAIN)
    }
    fn write_all(&mut self, mut buf: &[u8]) -> Result<(), i64> {
        while !buf.is_empty() {
            let n = self.write(buf)?;
            buf = &buf[n..];
        }
        Ok(())
    }
    fn close(self) {
        unsafe { sys1(SYS_CLOSE, self.h) };
    }
}

/// The verdict line. `resolve`/`connect`/`http` are pre-rendered cells.
fn verdict(l: &mut Line, tail: &[u8]) -> ! {
    l.s(b" tls=skip reason=window -> ").s(tail).s(b" ::").print();
    exit(0)
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    // 1. entropy
    let mut seed = [0u8; 32];
    let got = unsafe { sys2(SYS_GETRANDOM, seed.as_mut_ptr() as u64, seed.len() as u64) } as i64;
    let mut l = Line::new();
    l.s(b":: NETRING3: rand=").i(got);
    if got != 32 || seed == [0u8; 32] {
        l.s(b" resolve=skip connect=skip http=skip");
        verdict(&mut l, b"FAIL reason=getrandom");
    }

    // 2. resolve
    let mut out = [0u8; RESOLVE_OUT_LEN];
    let r = unsafe { sys3(SYS_RESOLVE, HOST.as_ptr() as u64, HOST.len() as u64, out.as_mut_ptr() as u64) } as i64;
    l.s(b" resolve=");
    if r != 0 {
        l.s(b"err").i(r).s(b" connect=skip http=skip");
        if r == ENODEV {
            verdict(&mut l, b"SKIP reason=no-link");
        }
        if r == ENOENT {
            verdict(&mut l, b"SKIP reason=no-answer");
        }
        verdict(&mut l, b"FAIL reason=resolve");
    }
    l.ip(&out[..4]);

    // 3. connect
    let h = unsafe { sys3(SYS_SOCKET, 2, 1, 0) } as i64;
    if h < 0 {
        l.s(b" connect=sock").i(h).s(b" http=skip");
        verdict(&mut l, b"FAIL reason=socket");
    }
    let mut sock = Sock { h: h as u64 };
    let pb = PORT.to_le_bytes();
    let hdr = [out[0], out[1], out[2], out[3], pb[0], pb[1], 0, 0];
    let mut c = EINPROGRESS;
    for _ in 0..CONNECT_TRIES {
        c = unsafe { sys3(SYS_CONNECT, sock.h, hdr.as_ptr() as u64, hdr.len() as u64) } as i64;
        if c != EINPROGRESS {
            break;
        }
        sleep_ms(50);
    }
    l.s(b" connect=").i(c);
    if c != 0 {
        sock.close();
        l.s(b" http=skip");
        verdict(&mut l, b"FAIL reason=connect");
    }

    // 4. http
    let req: &[u8] = b"GET / HTTP/1.0\r\nHost: api.anthropic.com\r\nUser-Agent: UnaOS-NET/1\r\nConnection: close\r\n\r\n";
    if let Err(e) = sock.write_all(req) {
        sock.close();
        l.s(b" http=send").i(e);
        verdict(&mut l, b"FAIL reason=send");
    }
    let mut head = [0u8; 256];
    let mut n = 0usize;
    while n < head.len() && !head[..n].contains(&b'\n') {
        match sock.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => break,
        }
    }
    sock.close();
    let line_end = head[..n].iter().position(|&b| b == b'\r' || b == b'\n').unwrap_or(n);
    let status = &head[..line_end];
    Line::new().s(b"NET: ").s(status).print();
    // "HTTP/1.x NNN ..."
    let code_ok = status.len() >= 12 && status.starts_with(b"HTTP/1.") && status[9..12].iter().all(|b| b.is_ascii_digit());
    l.s(b" http=");
    if !code_ok {
        l.s(b"bad");
        verdict(&mut l, b"FAIL reason=status-line");
    }
    l.s(&status[9..12]);

    // 5. tls (v1: the window refuses it; see the header)
    verdict(&mut l, b"PASS")
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(3)
}

/// EXECNAME (B322, R82): this program's launch declaration — a console program (no SYS_WIN_CREATE): a bare `net` runs in the foreground like `run`.
/// Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(0);
