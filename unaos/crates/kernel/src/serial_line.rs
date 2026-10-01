// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! SERIALLOCK M1 — the LINE-level lock in front of `arch::serial::_print`.
//!
//! Boot 17 (`docs/dev/evidence/rmbp-0915/flight17/f17-boot1.log`) carried ~197 lines with two witnesses
//! merged (`...thr=streak<=1:: VUGART: frames=2 ...`). `_print` is atomic per sink call, but a line is
//! several calls (a `serial_print!` fragment, then the `serial_println!` that ends it), and the format
//! arguments are evaluated sink by sink. So the macros now (1) format the WHOLE line into a fixed
//! 2048-byte stack buffer (SERIAL2: was 512; x86 task stacks are 16 KiB, IST 8 KiB, and panic mode bypasses the buffer) first (truncated with `…`), then (2) hand that one `&str` to `_print` inside
//! ONE critical section (`LINE_BUSY`). Lines from different cores can no longer interleave.
//!
//! Context rule. A caller with interrupts MASKED (an ISR, the deadman tick, a `without_interrupts`
//! body) must never wait on a lock whose holder it may have interrupted: it spins a bounded
//! `MASKED_SPINS`, and on contention PARKS the line in a small deferred ring that the next UNMASKED
//! print drains first (`deferred=`). An unmasked caller spins up to `UNMASKED_SPINS`, then bypasses
//! the lock (counted `bypass=`) rather than deadlock on a nested print. Panic mode bypasses entirely.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// One line, including its `\n` and a possible `…` marker.
pub const LINE_MAX: usize = 2048;
const DEFER_SLOTS: usize = 8;
const MASKED_SPINS: u32 = 4096;
const UNMASKED_SPINS: u32 = 1 << 22;

static LINE_BUSY: AtomicBool = AtomicBool::new(false);
static LINES: AtomicU64 = AtomicU64::new(0);
static MERGED_FIXED: AtomicU64 = AtomicU64::new(0);
static TRUNC: AtomicU64 = AtomicU64::new(0);
static DEFERRED: AtomicU64 = AtomicU64::new(0);
static DEFER_LOST: AtomicU64 = AtomicU64::new(0);
static BYPASS: AtomicU64 = AtomicU64::new(0);
static SRC_EMIT: AtomicU64 = AtomicU64::new(0);
static SRC_USER: AtomicU64 = AtomicU64::new(0);
static TRUNC_ANNOUNCED: AtomicBool = AtomicBool::new(false);

struct Buf {
    b: [u8; LINE_MAX],
    n: usize,
    cut: bool,
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Reserve 4 bytes: the 3-byte `…` and the `\n`.
        let room = (LINE_MAX - 4).saturating_sub(self.n);
        let mut k = s.len().min(room);
        while k > 0 && !s.is_char_boundary(k) {
            k -= 1;
        }
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        if k < s.len() {
            self.cut = true;
        }
        Ok(())
    }
}

struct Deferred {
    buf: [[u8; LINE_MAX]; DEFER_SLOTS],
    len: [u16; DEFER_SLOTS],
    n: usize,
}
static DEFER: spin::Mutex<Deferred> =
    spin::Mutex::new(Deferred { buf: [[0; LINE_MAX]; DEFER_SLOTS], len: [0; DEFER_SLOTS], n: 0 });

#[inline]
fn irq_masked() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        !x86_64::instructions::interrupts::are_enabled()
    }
    #[cfg(target_arch = "aarch64")]
    {
        let daif: u64;
        unsafe { core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags)) };
        daif & 0x80 != 0
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        false
    }
}

#[inline]
fn forward(s: &str) {
    crate::arch::serial::_print(format_args!("{}", s));
}

/// The macros' single entry. `nl` is true for `serial_println!`.
#[doc(hidden)]
pub fn emit(args: fmt::Arguments, nl: bool) {
    emit_src(args, nl, false)
}

fn emit_src(args: fmt::Arguments, nl: bool, user: bool) {
    if crate::serial_ring::in_panic_mode() {
        // Last words: no lock, no buffer.
        crate::arch::serial::_print(format_args!("{}{}", args, if nl { "\n" } else { "" }));
        return;
    }
    let mut lb = Buf { b: [0; LINE_MAX], n: 0, cut: false };
    let _ = lb.write_fmt(args);
    if lb.cut {
        lb.b[lb.n..lb.n + 3].copy_from_slice("…".as_bytes());
        lb.n += 3;
        TRUNC.fetch_add(1, Relaxed);
    }
    if nl {
        lb.b[lb.n] = b'\n';
        lb.n += 1;
    }
    let line = core::str::from_utf8(&lb.b[..lb.n]).unwrap_or("[serial] utf8?\n");
    LINES.fetch_add(1, Relaxed);
    if user { SRC_USER.fetch_add(1, Relaxed); } else { SRC_EMIT.fetch_add(1, Relaxed); }

    let masked = irq_masked();
    let bound = if masked { MASKED_SPINS } else { UNMASKED_SPINS };
    let mut spins: u32 = 0;
    loop {
        if LINE_BUSY.compare_exchange(false, true, core::sync::atomic::Ordering::Acquire, Relaxed).is_ok() {
            if spins > 0 {
                MERGED_FIXED.fetch_add(1, Relaxed);
            }
            if !masked {
                drain_deferred();
            }
            forward(line);
            LINE_BUSY.store(false, core::sync::atomic::Ordering::Release);
            break;
        }
        spins += 1;
        if spins >= bound {
            if masked {
                park(line);
            } else {
                BYPASS.fetch_add(1, Relaxed);
                forward(line);
            }
            break;
        }
        core::hint::spin_loop();
    }
    if lb.cut && !masked && !TRUNC_ANNOUNCED.swap(true, Relaxed) {
        serial_println!("[serial] trunc: a line exceeded {} bytes and was cut with `…` (count in :: SERIAL:)", LINE_MAX);
    }
}

fn park(line: &str) {
    if let Some(mut d) = DEFER.try_lock() {
        if d.n < DEFER_SLOTS {
            let i = d.n;
            let k = line.len().min(LINE_MAX);
            d.buf[i][..k].copy_from_slice(&line.as_bytes()[..k]);
            d.len[i] = k as u16;
            d.n += 1;
            DEFERRED.fetch_add(1, Relaxed);
            return;
        }
    }
    DEFER_LOST.fetch_add(1, Relaxed);
}

/// Called by the lock holder, unmasked only: parked lines go out ahead of the holder's own.
fn drain_deferred() {
    if let Some(mut d) = DEFER.try_lock() {
        for i in 0..d.n {
            let k = d.len[i] as usize;
            if let Ok(s) = core::str::from_utf8(&d.buf[i][..k]) {
                forward(s);
            }
        }
        d.n = 0;
    }
}

/// `(lines, merged_fixed, trunc, deferred)` plus the two loss counters for the census.
pub fn census() -> (u64, u64, u64, u64, u64, u64) {
    (
        LINES.load(Relaxed),
        MERGED_FIXED.load(Relaxed),
        TRUNC.load(Relaxed),
        DEFERRED.load(Relaxed),
        DEFER_LOST.load(Relaxed),
        BYPASS.load(Relaxed),
    )
}

static NEXT_CENSUS_MS: AtomicU64 = AtomicU64::new(0);

/// M2 — the census witness, once per second, riding `serial_ring::mirror_service` (unmasked,
/// no locks held, reached on both arches). PASS = it printed; the numbers are for the reader.
pub fn census_poll() {
    let now = crate::arch::ms();
    if now < NEXT_CENSUS_MS.load(Relaxed) {
        return;
    }
    NEXT_CENSUS_MS.store(now.saturating_add(1000), Relaxed);
    let (l, m, t, d, dl, b) = census();
    let (e, u, raw, ring) = by_source();
    serial_println!(":: SERIAL: lines={} merged_fixed={} trunc={} deferred={} defer_lost={} bypass={} by_source=[emit:{},user:{},raw:{},ring:{}] -> PASS ::", l, m, t, d, dl, b, e, u, raw, ring);
}

/// SERIAL2 M1 — where the wire's lines came from: `emit` (kernel macros through the line lock), `user`
/// (ring-3 `SYS_WRITE` lines through [`emit_user`]), `raw` (`_print` submissions that did NOT come through
/// `emit`: direct callers and panic mode = SUBMITTED minus the two), `ring` (lines the FTDI capture tap
/// lost or tore — the sink-side damage the line lock cannot see; boot 18's 1136 merged lines were this
/// class, `[mirror] tste: 1101 line(s) dropped`, plus ring-3's own 192-byte `Buf` clipping the `\n`).
pub fn by_source() -> (u64, u64, u64, u64) {
    let e = SRC_EMIT.load(Relaxed);
    let u = SRC_USER.load(Relaxed);
    let sub = crate::serial_ring::SUBMITTED.load(Relaxed);
    let t = &crate::serial_ring::TAP_FTDI;
    (e, u, sub.saturating_sub(e + u), t.dropped.load(Relaxed) + t.torn.load(Relaxed))
}

const USER_SLOTS: usize = 16;
/// Per-process partial-line bound (SERIAL2 M3).
const USER_LINE_MAX: usize = 1536;
struct UserLine {
    b: [u8; USER_LINE_MAX],
    n: usize,
}
static USER_LINES: [spin::Mutex<UserLine>; USER_SLOTS] =
    [const { spin::Mutex::new(UserLine { b: [0; USER_LINE_MAX], n: 0 }) }; USER_SLOTS];

/// SERIAL2 M3 — the ring-3 console write path. `sys_write` used to hand each raw write to `serial_print!`;
/// a vug whose line exceeds one write (or two vugs' writes) could split a line across the lock. Now the
/// bytes are buffered per process (`row`) until a `\n` and each COMPLETE line goes out as ONE `emit`
/// (one critical section). A partial line is held up to [`USER_LINE_MAX`] bytes, then flushed as is. The
/// syscall runs IF-masked: the slot is `try_lock`ed and on contention the text goes out directly.
pub fn emit_user(row: usize, text: &str) {
    let mut guard = match USER_LINES.get(row).and_then(|m| m.try_lock()) {
        Some(g) => g,
        None => return emit_src(format_args!("{}", text), false, true),
    };
    let mut rest = text;
    while !rest.is_empty() {
        let (chunk, tail) = match rest.find('\n') {
            Some(i) => rest.split_at(i + 1),
            None => (rest, ""),
        };
        rest = tail;
        let complete = chunk.ends_with('\n');
        if guard.n + chunk.len() > USER_LINE_MAX {
            // Flush what is held, then fall through with the chunk on its own.
            if guard.n > 0 {
                let n = guard.n;
                if let Ok(h) = core::str::from_utf8(&guard.b[..n]) {
                    emit_src(format_args!("{}", h), false, true);
                }
                guard.n = 0;
            }
            if chunk.len() > USER_LINE_MAX {
                emit_src(format_args!("{}", chunk), false, true);
                continue;
            }
        }
        let n = guard.n;
        guard.b[n..n + chunk.len()].copy_from_slice(chunk.as_bytes());
        guard.n += chunk.len();
        if complete {
            let n = guard.n;
            if let Ok(l) = core::str::from_utf8(&guard.b[..n]) {
                emit_src(format_args!("{}", l), false, true);
            }
            guard.n = 0;
        }
    }
}
