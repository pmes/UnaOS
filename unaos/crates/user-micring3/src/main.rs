#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// MICRING3 (rmbp-ledger B736, BP-audio-12): MICRING3.ELF — the microphone from ring 3, the smallest client of una-abi's
// MICRING3 verbs (BUS_VERB_AUDIO_SOURCE_OPEN / READ / CLOSE, 40..42). `tests micring3` runs it three ways, by argv[1]:
//
//   (none)  OPEN (polling -EAGAIN every 10 ms while the grant is asked, 3 s at most), READ every 10 ms for 100 ms,
//           CLOSE, exit.
//   hold    OPEN, then READ every 10 ms until a READ answers -EPERM (the user revoked the grant: the source is closed)
//           or 5 s pass, then CLOSE (a revoked handle closes quietly), exit.
//   exit    OPEN, one READ, exit WITHOUT CLOSE — the kernel closes it at the program's end.
//
// Wire: `MICRING3: mode=<m> open=<rc> ch=<n> rate=<hz> read_bytes=<n> revoked=<0|1> close=<rc>`.
// Exit status: 1<<24 (it ran) | opened<<23 | revoked<<22 | close_ok<<21 | min(read_bytes, 0xFFFFF).
use una_abi::{
    audio_source_open_parse, BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_KIND_REQUEST, BUS_MAGIC,
    BUS_VERB_AUDIO_SOURCE_CLOSE, BUS_VERB_AUDIO_SOURCE_OPEN, BUS_VERB_AUDIO_SOURCE_READ, BUS_VERSION, EAGAIN, EIO,
    EPERM, SYS_EXIT, SYS_MRECV, SYS_MSEND, SYS_SLEEP_MS, SYS_WRITE,
};

/// x86_64: every stub declares all eight scrubbed/destroyed registers whatever its arity (the ABIPROBE stubs).
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

fn nap(ms: u64) {
    unsafe { sys1(SYS_SLEEP_MS, ms) };
}

/// The console handle (index 1, endowed at launch).
const CONSOLE: u64 = 1;

/// ONE frame buffer in .bss: requests are built in it and replies land in it (strictly half-duplex).
static mut FRAME: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];

#[allow(static_mut_refs)]
fn frame() -> &'static mut [u8; BUS_FRAME_MAX] {
    unsafe { &mut FRAME }
}

/// One request and its reply: `Ok(body_len)` (the body at `frame()[BUS_HDR_LEN..]`) or `Err(-errno)` — the reply's
/// status, or the bus's own failure.
fn ask(verb: u8, corr: u32, body: &[u8]) -> Result<usize, i64> {
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
        return Err(s);
    }
    for _ in 0..8 {
        let n = unsafe { sys3(SYS_MRECV, f.as_mut_ptr() as u64, BUS_FRAME_MAX as u64, 0) };
        if n < 0 {
            return Err(n);
        }
        let n = (n as usize).min(BUS_FRAME_MAX);
        let u = |o: usize| u32::from_le_bytes([f[o], f[o + 1], f[o + 2], f[o + 3]]);
        if n < BUS_HDR_LEN || f[0..4] != BUS_MAGIC || f[5] != BUS_KIND_REPLY || f[6] != verb || u(8) != corr {
            continue; // not this ask's reply
        }
        let status = u(12) as i32 as i64;
        if status != 0 {
            return Err(status);
        }
        return Ok((u(48) as usize).min(n - BUS_HDR_LEN));
    }
    Err(EIO)
}

struct Line {
    b: [u8; 192],
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
    fn kv(&mut self, k: &[u8], v: i64) {
        self.put(b" ");
        self.put(k);
        self.put(b"=");
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
    fn flush(&mut self) {
        unsafe { sys3(SYS_WRITE, CONSOLE, self.b.as_ptr() as u64, self.n as u64) };
        self.n = 0;
    }
}

/// argv[1] from the kernel-written args page (empty when absent).
fn mode() -> &'static [u8] {
    let va = una_abi::USER_ARGS_VA;
    // SAFETY: the launcher maps this process's 4 KiB args page (ring-3 read-only) at `USER_ARGS_VA`.
    let page = unsafe { core::slice::from_raw_parts(va as *const u8, 4096) };
    una_abi::Args::parse(page, va).and_then(|a| a.get(1)).unwrap_or(b"")
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let m = mode();
    let mut l = Line { b: [0; 192], n: 0 };
    l.put(b"MICRING3: mode=");
    l.put(if m.is_empty() { b"read".as_slice() } else { m });

    // OPEN, polling while the grant is asked (the user answers the ask on the glass).
    let mut open = EAGAIN;
    for _ in 0..300 {
        open = match ask(BUS_VERB_AUDIO_SOURCE_OPEN, 0x4D49_0001, &[]) {
            Ok(n) => match audio_source_open_parse(&frame()[BUS_HDR_LEN..BUS_HDR_LEN + n]) {
                Some((_, _, _, h)) => h as i64,
                None => EIO,
            },
            Err(e) => e,
        };
        if open != EAGAIN {
            break;
        }
        nap(10);
    }
    l.kv(b"open", open);
    if open <= 0 {
        l.put(b"\n");
        l.flush();
        exit(1 << 24);
    }
    let (ch, rate) = match audio_source_open_parse(&frame()[BUS_HDR_LEN..BUS_HDR_LEN + una_abi::AUDIO_SOURCE_OPEN_REPLY_LEN]) {
        Some((c, _, r, _)) => (c as i64, r as i64),
        None => (0, 0),
    };
    l.kv(b"ch", ch);
    l.kv(b"rate", rate);
    let h = (open as u32).to_le_bytes();

    // READ: 100 ms (10 x 10 ms), or until revoked (hold), or once (exit).
    let rounds = match m {
        b"hold" => 500,
        b"exit" => 1,
        _ => 10,
    };
    let mut bytes = 0u64;
    let mut revoked = false;
    for _ in 0..rounds {
        nap(10);
        match ask(BUS_VERB_AUDIO_SOURCE_READ, 0x4D49_0002, &h) {
            Ok(n) => bytes += n as u64,
            Err(e) if e == EPERM => {
                revoked = true;
                break;
            }
            Err(_) => break,
        }
    }
    l.kv(b"read_bytes", bytes as i64);
    l.kv(b"revoked", revoked as i64);
    let status = 1u64 << 24 | 1 << 23 | (revoked as u64) << 22 | bytes.min(0xF_FFFF);
    if m == b"exit" {
        l.put(b" close=none\n");
        l.flush();
        exit(status); // no CLOSE: the program's end closes it
    }
    let close = match ask(BUS_VERB_AUDIO_SOURCE_CLOSE, 0x4D49_0003, &h) {
        Ok(_) => 0,
        Err(e) => e,
    };
    l.kv(b"close", close);
    l.put(b"\n");
    l.flush();
    exit(status | ((close == 0) as u64) << 21)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(0xF0)
}

/// EXECNAME (B322, R82): a console program — a bare `micring3` runs in the foreground like `run`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(0);
