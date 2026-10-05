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
    if fold_take(&lb.b[..lb.n]) { return; } line_note(&lb.b[..lb.n]); #[cfg(feature = "selfdiag")] crate::bootwit::note(&lb.b[..lb.n]); // SELFDIAG M1 (B324): the boot-log tap. QUIETBOOT M4: per-tag tally until the `:: BOOT:` line (same-line fold).
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
    let now = crate::arch::ms(); if !crate::census::on(crate::census::SERIAL) { return; } // QUIETBOOT (R80): a census, OFF until `census start`.
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
    line_watch_check(text); // LUMENCRASH M3 (B326): a fixture may be waiting for this line (file tail)
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

// ── QUIETBOOT M4 (R80, rmbp-ledger B311) — the per-tag tally of the lines printed BEFORE `:: BOOT:` ──
//
// `tests quietboot` names the loudest tags when the boot is over its bound. A tag is the first word
// after `:: ` (up to `:`, ` ` or `=`) or a leading `[tag]`, cut to 8 bytes and packed into one `u64`
// (0 = empty slot). 48 slots, linear probe, CAS on the key — lock-free, so the emit path stays safe from
// an ISR; a full table counts the line in `TAG_OTHER`. Closed (one relaxed load per line) once the
// boot line has printed.
const TAG_SLOTS: usize = 128; // QUIETBOOT2 (B325): 48 -> 128 — boot 20 filled 48 slots before SMC-SCOUT/gen7 spoke, so its `top=` named neither.
static TAG_KEY: [AtomicU64; TAG_SLOTS] = [const { AtomicU64::new(0) }; TAG_SLOTS];
static TAG_N: [AtomicU64; TAG_SLOTS] = [const { AtomicU64::new(0) }; TAG_SLOTS];
static TAG_OTHER: AtomicU64 = AtomicU64::new(0);
static TAG_CLOSED: AtomicBool = AtomicBool::new(false);

fn tag_key(b: &[u8]) -> u64 {
    let (start, stop): (usize, &[u8]) = if b.starts_with(b":: ") { (3, b": =\n") } else if b.first() == Some(&b'[') { (0, b"]\n") } else { (0, b": =\n") };
    let mut k = 0u64;
    let mut i = 0usize;
    while i < 8 && start + i < b.len() {
        let c = b[start + i];
        if stop.contains(&c) { if c == b']' && i < 8 { k |= (c as u64) << (8 * i); } break; }
        k |= (c as u64) << (8 * i);
        i += 1;
    }
    k
}

fn tag_note(b: &[u8]) {
    if TAG_CLOSED.load(Relaxed) { return; }
    let k = tag_key(b);
    if k == 0 { TAG_OTHER.fetch_add(1, Relaxed); return; }
    let h = (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 57) as usize % TAG_SLOTS;
    for j in 0..TAG_SLOTS {
        let s = (h + j) % TAG_SLOTS;
        let cur = TAG_KEY[s].load(Relaxed);
        if cur == k || (cur == 0 && TAG_KEY[s].compare_exchange(0, k, Relaxed, Relaxed).map_or_else(|v| v == k, |_| true)) {
            TAG_N[s].fetch_add(1, Relaxed);
            return;
        }
    }
    TAG_OTHER.fetch_add(1, Relaxed);
}

/// Stop tallying (the `:: BOOT:` line calls this right after it reads `LINES`).
pub fn tag_close() { TAG_CLOSED.store(true, Relaxed); }

/// The `n` loudest tags before the boot line, as `(tag bytes, count)` — the tag is up to 8 ASCII bytes.
pub fn tag_top(out: &mut [([u8; 8], u64)]) -> usize {
    let mut taken = [false; TAG_SLOTS];
    let mut w = 0usize;
    while w < out.len() {
        let mut best: Option<usize> = None;
        for s in 0..TAG_SLOTS {
            if taken[s] || TAG_KEY[s].load(Relaxed) == 0 { continue; }
            if best.is_none_or(|b| TAG_N[s].load(Relaxed) > TAG_N[b].load(Relaxed)) { best = Some(s); }
        }
        let Some(s) = best else { break };
        taken[s] = true;
        out[w] = (TAG_KEY[s].load(Relaxed).to_le_bytes(), TAG_N[s].load(Relaxed));
        w += 1;
    }
    w
}

// ── LUMENCRASH M3 (rmbp-ledger B326) — a one-shot watch on ring-3 console lines, for a kernel fixture ──
// `tests lumen` spawns the real LUMEN.ELF and needs to know that the program reached its first wire line. A ring-3
// program's console bytes all pass through `emit_user` (SYS_WRITE on a Console handle), so the watch sits at its
// head: armed with a `'static` prefix BEFORE the spawn (the program may speak before the spawner learns its slot),
// it latches `line_watch_hit` on the first write that begins with the prefix. Two atomics publish the prefix (len
// first, then the pointer with Release); a disarmed watch costs one Acquire load per ring-3 write.
static WATCH_PTR: AtomicU64 = AtomicU64::new(0);
static WATCH_LEN: AtomicU64 = AtomicU64::new(0);
static WATCH_HIT: AtomicBool = AtomicBool::new(false);

/// Arm the watch for writes that begin with `prefix` (clears a previous hit).
pub fn line_watch_arm(prefix: &'static str) {
    WATCH_HIT.store(false, core::sync::atomic::Ordering::Release);
    WATCH_LEN.store(prefix.len() as u64, Relaxed);
    WATCH_PTR.store(prefix.as_ptr() as u64, core::sync::atomic::Ordering::Release);
}

/// Disarm the watch (the hit latch keeps its value until the next arm).
pub fn line_watch_disarm() {
    WATCH_PTR.store(0, core::sync::atomic::Ordering::Release);
}

/// True once a watched write was seen since the last arm.
pub fn line_watch_hit() -> bool {
    WATCH_HIT.load(core::sync::atomic::Ordering::Acquire)
}

fn line_watch_check(text: &str) {
    let p = WATCH_PTR.load(core::sync::atomic::Ordering::Acquire);
    if p == 0 {
        return;
    }
    let n = WATCH_LEN.load(Relaxed) as usize;
    // SAFETY: `p`/`n` came from a `&'static str` in `line_watch_arm`, published len-then-pointer.
    let pre = unsafe { core::slice::from_raw_parts(p as *const u8, n) };
    if text.as_bytes().starts_with(pre) {
        WATCH_HIT.store(true, core::sync::atomic::Ordering::Release);
    }
}

// ── QUIETBOOT3 (rmbp-ledger B352, R80) — a LINE is what reaches a newline ─────────────────────────────────────
// Boot 21 read `lines=1327` against a wire that carried 1144 lines to `:: BOOT:`: every `serial_print!` FRAGMENT
// was counted as a line (the iGPU ladder's EDID dump is 8 rows x 17 `serial_print!` calls; `top=` even named a
// tag `00` — the EDID's zero bytes). Now `LINES` counts an emit that ENDS in `\n` (every `serial_println!`, and
// the fragment that closes a `serial_print!` line), and the per-tag tally keys on the fragment that OPENS a
// line. `MID` is "the last emit left a line open"; cross-core interleave of fragments can mis-key a tag, never
// double-count a line.
static MID: AtomicBool = AtomicBool::new(false);

fn line_note(b: &[u8]) {
    let ends = b.last() == Some(&b'\n');
    let opens = !MID.swap(!ends, Relaxed);
    if ends { LINES.fetch_add(1, Relaxed); }
    if opens { tag_note(b); }
    if TAIL_ARM.load(Relaxed) { tail_note(b); } crate::pwwire::wire_note(b); // CONSOLEFIX M3 (B365): the `tests pwwire` tap
}

// ── QUIETBOOT3 (B352) — THE GLASS SAYS WHAT THE WIRE SAYS ─────────────────────────────────────────────────────
// While `tests` runs a fixture, every kernel line that carries a verdict arrow (`-> PASS`, `-> FAIL…`, `-> SKIP…`)
// leaves its TAIL here — the text after the LAST `-> ` up to the line's closing ` ::`. `tests` prints that tail on
// the console (`nethang -> PASS`), so the glass reads the wire's own word, never a paraphrase of a tally.
static TAIL_ARM: AtomicBool = AtomicBool::new(false);
const TAIL_MAX: usize = 120;
static TAIL: spin::Mutex<([u8; TAIL_MAX], usize)> = spin::Mutex::new(([0; TAIL_MAX], 0));

/// Arm the recorder for one fixture (clears the last tail).
pub fn tail_arm() {
    if let Some(mut t) = TAIL.try_lock() { t.1 = 0; }
    TAIL_ARM.store(true, Relaxed);
}

/// Disarm and take the last verdict tail the fixture printed (`None` = it printed no verdict line).
pub fn tail_take() -> Option<alloc::string::String> {
    TAIL_ARM.store(false, Relaxed);
    let t = TAIL.lock();
    if t.1 == 0 { return None; }
    core::str::from_utf8(&t.0[..t.1]).ok().map(alloc::string::String::from)
}

/// The tail of one verdict line: after the last `-> `, through the first ` ::` (or the line's end), trimmed.
/// Only `PASS` / `FAIL` / `SKIP` tails count — a `-> gsi=22` route line is not a verdict.
pub fn verdict_tail(line: &str) -> Option<&str> {
    let at = line.rfind("-> ")?;
    let rest = &line[at + 3..];
    let rest = rest.split(" ::").next().unwrap_or(rest).trim_end_matches(['\n', '\r', ' ']);
    if rest.starts_with("PASS") || rest.starts_with("FAIL") || rest.starts_with("SKIP") { Some(rest) } else { None }
}

fn tail_note(b: &[u8]) {
    let Ok(s) = core::str::from_utf8(b) else { return };
    let Some(v) = verdict_tail(s) else { return };
    if let Some(mut t) = TAIL.try_lock() {
        let mut k = v.len().min(TAIL_MAX);
        while k > 0 && !v.is_char_boundary(k) { k -= 1; }
        t.0[..k].copy_from_slice(&v.as_bytes()[..k]);
        t.1 = k;
    }
}

// ── GLASSLAG M4 (rmbp-ledger B370) — THE KEPLER TAKEOVER'S PROSE, FOLDED ──────────────────────────────────────────
// Flight 22 read `:: QUIETBOOT: lines=384 bound=250 … top=[kepler:263,…] -> FAIL` with the nine recon knobs off:
// `kepler::init` alone printed 263 `:: kepler: <sub> …` lines (128 `ucode-post`, 40 `FENCE`, the falcon ports,
// the beacons, recon, bind, witness …) before the boot line. They are the takeover's own narration, read on a
// bench session, not lines the boot needs to decide anything (R80). While a FOLD is open (`fold_open`, around
// the `kepler::init` call in `arch/x86_64/pci.rs`), a line opening with the fold's prefix does not reach the
// wire: it is tallied by FAMILY (the sub-tag's first word, lowercased, cut at `-` `/` `_` `[`), its text kept
// for that family's `last=`, and the whole line stored verbatim. `fold_close` prints at most 18 rollups:
//
// `:: KEPLER: fold family=<f> n=<count> last=<the family's last line> ::`  (the 16 loudest families)
// `:: KEPLER: fold rest=[<f>:<n>,…] ::`                                   (any beyond 16)
// `:: KEPLER: fold lines=<n> families=<n> kept=<n> dropped=<n> replay=tests keplerlog ::`
//
// `tests keplerlog` prints every folded line verbatim, then `:: KEPLERLOG: lines= kept= dropped= -> PASS ::`.
// A `census` build (`UNAOS_CENSUS=1`, the QEMU lanes) never folds. A contended fold store prints the line as
// before — the fold can lose a tally, never a line.

const FOLD_FAMS: usize = 32;
const FOLD_FAM_LEN: usize = 16;
const FOLD_LAST: usize = 160;
const FOLD_KEEP: usize = 48 * 1024;
const FOLD_SHOWN: usize = 16;

static FOLD_ON: AtomicBool = AtomicBool::new(false);
/// A folded fragment left its line open (a `serial_print!` piece): its continuation folds too.
static FOLD_MID: AtomicBool = AtomicBool::new(false);
static FOLD_PREFIX: spin::Mutex<&'static [u8]> = spin::Mutex::new(b"");

struct FoldStore {
    fam: [[u8; FOLD_FAM_LEN]; FOLD_FAMS],
    fam_len: [u8; FOLD_FAMS],
    n: [u32; FOLD_FAMS],
    last: [[u8; FOLD_LAST]; FOLD_FAMS],
    last_len: [u8; FOLD_FAMS],
    fams: usize,
    other: u32,
    lines: u32,
    keep: [u8; FOLD_KEEP],
    kept_len: usize,
    kept: u32,
    dropped: u32,
}

static FOLD: spin::Mutex<FoldStore> = spin::Mutex::new(FoldStore {
    fam: [[0; FOLD_FAM_LEN]; FOLD_FAMS],
    fam_len: [0; FOLD_FAMS],
    n: [0; FOLD_FAMS],
    last: [[0; FOLD_LAST]; FOLD_FAMS],
    last_len: [0; FOLD_FAMS],
    fams: 0,
    other: 0,
    lines: 0,
    keep: [0; FOLD_KEEP],
    kept_len: 0,
    kept: 0,
    dropped: 0,
});

/// The family of a folded line's body (the text after the prefix): its first word, lowercased, cut at the
/// first `-` `/` `_` `[` `=` `(` — `ucode-post` and `ucode` are one family, `WITNESS` and `witness-rematch` one. Pure.
pub fn fold_family(body: &[u8], out: &mut [u8; FOLD_FAM_LEN]) -> usize {
    let mut n = 0;
    for &c in body {
        if matches!(c, b' ' | b'\n' | b'-' | b'/' | b'_' | b'[' | b'=' | b'(' | b':') || n == FOLD_FAM_LEN { break; }
        out[n] = c.to_ascii_lowercase();
        n += 1;
    }
    n
}

/// Open a fold: until [`fold_close`], lines opening with `prefix` are tallied, not printed.
pub fn fold_open(prefix: &'static [u8]) {
    if cfg!(feature = "census") { return; }
    *FOLD_PREFIX.lock() = prefix;
    FOLD_ON.store(true, core::sync::atomic::Ordering::Release);
}

fn fold_take(b: &[u8]) -> bool {
    if !FOLD_ON.load(Relaxed) { return false; }
    let mid = FOLD_MID.load(Relaxed);
    let Some(pre) = FOLD_PREFIX.try_lock().map(|p| *p) else { return false };
    if !mid && !b.starts_with(pre) { return false; }
    let Some(mut f) = FOLD.try_lock() else { return false };
    let ends = b.last() == Some(&b'\n');
    FOLD_MID.store(!ends, Relaxed);
    // Verbatim, for `tests keplerlog`.
    if f.kept_len + b.len() <= FOLD_KEEP {
        let at = f.kept_len;
        f.keep[at..at + b.len()].copy_from_slice(b);
        f.kept_len += b.len();
        if ends { f.kept += 1; }
    } else if ends {
        f.dropped += 1;
    }
    if mid { return true; } // a continuation: tallied with the fragment that opened it
    f.lines += 1;
    let body = &b[pre.len()..];
    let mut key = [0u8; FOLD_FAM_LEN];
    let kn = fold_family(body, &mut key);
    let slot = (0..f.fams).find(|&i| f.fam_len[i] as usize == kn && f.fam[i][..kn] == key[..kn]);
    let i = match slot {
        Some(i) => i,
        None if f.fams < FOLD_FAMS => {
            let i = f.fams;
            f.fams += 1;
            f.fam[i] = key;
            f.fam_len[i] = kn as u8;
            i
        }
        None => { f.other += 1; return true; }
    };
    f.n[i] += 1;
    let mut text = body;
    while let Some((&c, rest)) = text.split_last() { if c == b'\n' || c == b' ' || c == b':' { text = rest; } else { break; } }
    let k = text.len().min(FOLD_LAST);
    f.last[i][..k].copy_from_slice(&text[..k]);
    f.last_len[i] = k as u8;
    true
}

/// Close the fold and print its rollups (at most 18 lines); registers `tests keplerlog`.
pub fn fold_close() {
    if !FOLD_ON.swap(false, core::sync::atomic::Ordering::AcqRel) { return; }
    FOLD_MID.store(false, Relaxed);
    let mut order = [0usize; FOLD_FAMS];
    let (fams, lines, other, kept, dropped) = {
        let f = FOLD.lock();
        for (i, o) in order.iter_mut().enumerate() { *o = i; }
        let fams = f.fams;
        order[..fams].sort_unstable_by(|a, b| f.n[*b].cmp(&f.n[*a]));
        for &i in order[..fams.min(FOLD_SHOWN)].iter() {
            let name = core::str::from_utf8(&f.fam[i][..f.fam_len[i] as usize]).unwrap_or("?");
            let last = core::str::from_utf8(&f.last[i][..f.last_len[i] as usize]).unwrap_or("?");
            serial_println!(":: KEPLER: fold family={} n={} last={} ::", name, f.n[i], last);
        }
        if fams > FOLD_SHOWN || f.other > 0 {
            let mut s = alloc::string::String::new();
            for &i in order[FOLD_SHOWN.min(fams)..fams].iter() {
                if !s.is_empty() { s.push(','); }
                s.push_str(core::str::from_utf8(&f.fam[i][..f.fam_len[i] as usize]).unwrap_or("?"));
                s.push_str(&alloc::format!(":{}", f.n[i]));
            }
            if f.other > 0 { if !s.is_empty() { s.push(','); } s.push_str(&alloc::format!("other:{}", f.other)); }
            serial_println!(":: KEPLER: fold rest=[{}] ::", s);
        }
        (fams, f.lines, f.other, f.kept, f.dropped)
    };
    let _ = other;
    serial_println!(":: KEPLER: fold lines={} families={} kept={} dropped={} replay=tests keplerlog ::", lines, fams, kept, dropped);
    crate::tests::register("keplerlog", keplerlog_replay);
}

/// `tests keplerlog`: every line the Kepler fold took off the boot, verbatim.
pub fn keplerlog_replay() {
    let (lines, kept, dropped) = {
        let f = FOLD.lock();
        let (lines, kept, dropped) = (f.lines, f.kept, f.dropped);
        let text = &f.keep[..f.kept_len];
        let mut owned = alloc::vec::Vec::with_capacity(text.len());
        owned.extend_from_slice(text);
        drop(f);
        for l in owned.split(|c| *c == b'\n') {
            if l.is_empty() { continue; }
            serial_println!("{}", core::str::from_utf8(l).unwrap_or("[keplerlog] utf8?"));
        }
        (lines, kept, dropped)
    };
    serial_println!(":: KEPLERLOG: lines={} kept={} dropped={} -> PASS ::", lines, kept, dropped);
}
