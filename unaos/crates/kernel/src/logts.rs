// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! CLOCK-2 — opt-in log timestamps on the kernel serial log (`logts` feature).
//!
//! Today serial lines carry no timestamps (a few subsystems hand-roll `[ms]` prefixes). This
//! module prefixes every serial LINE with a compact, fixed-width timestamp so a captured log is
//! self-dating. It is the named CLOCK-1 follow-up ("log-timestamp adoption"): the prefix reads the
//! shared `crate::clock` seam, so it works on both arches with no new plumbing.
//!
//! CLOCK-2b: the prefix covers the UART byte-stream AND the two capture taps (`ftdi::mirror`,
//! `flight_recorder::capture`) — see [`TapPrefixWriter`]. The taps run OUTSIDE the IRQ mask and
//! outside `SERIAL1`, which is still safe: the whole render path is lock-free plus one `try_lock`
//! (verified end to end, GR16 review). fbcon and the `tste` selftest ring stay unprefixed.
//!
//! ## The knob
//!
//! Gated entirely by the `logts` cargo feature (env `UNAOS_LOGTS=1`, wired in `arroyo` + the
//! builder). DEFAULT OFF: with the feature absent this module is not compiled and `_print` writes
//! the raw args, so the serial byte-stream is IDENTICAL to a plain build — the witness batteries and
//! mbench specs, which parse serial lines, are unaffected.
//!
//! ## The format (fixed-width, 12 columns)
//!
//! * pre-sync (no civil anchor yet): `[  12345ms] ` — monotonic milliseconds since KERNEL ENTRY
//!   (the bootpace `entry` stamp, so the column lines up with BPACE/GPACE `t=` figures),
//!   right-justified in 7 columns. Before the entry stamp exists or before the counter is
//!   calibrated the line prints `[      ?ms] ` — an explicit unknown, never a fabricated zero.
//! * post-sync (a civil anchor exists — SNTP on the pi, `date -s` on either arch): `[15:04:07Z] ` —
//!   UTC wall time HH:MM:SS. The prefix flips from the monotonic form to this the instant the anchor
//!   is planted.
//!
//! Both forms are 12 columns wide so the log stays column-aligned across the flip. (A run past
//! 9,999,999 ms ≈ 2.8 h widens the monotonic form; boot/test logs never reach that.)
//!
//! ## Safety in every print context
//!
//! `_print` already runs with interrupts masked while holding the single UART lock (aarch64
//! `SERIAL_PORT`, x86 `SERIAL1`). The line-start state below is a `Relaxed` atomic — NOT a lock — so
//! it is mutated only under that existing UART lock, adding no new lock and no interleaving beyond
//! what already exists. The prefix reads `clock::logts_now()`, which is lock-free for the monotonic
//! part and `try_lock`-only for the civil anchor (never blocks, never panics), so it is safe from
//! early boot (before clock init), from IRQ-masked handlers, and on every core.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

/// True while the next byte written to the UART begins a fresh line (so it should be prefixed).
/// Persists across `serial_print!` fragments that build one line piecewise, so a partial-line print
/// followed by its completion gets exactly ONE prefix. Only ever touched under the UART lock held by
/// `_print`, so `Relaxed` is sufficient and no new lock is introduced.
static AT_LINE_START: AtomicBool = AtomicBool::new(true);

/// A bounded, allocation-free byte sink for rendering the prefix.
struct FixedBuf<'a> {
    buf: &'a mut [u8],
    len: usize,
}
impl core::fmt::Write for FixedBuf<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let b = s.as_bytes();
        let end = core::cmp::min(self.buf.len(), self.len + b.len());
        let n = end - self.len;
        self.buf[self.len..end].copy_from_slice(&b[..n]);
        self.len = end;
        Ok(())
    }
}

/// Render the current line prefix into `buf` (>= 20 bytes), returning its length. Pure ASCII.
fn render_prefix(buf: &mut [u8]) -> usize {
    let (mono_ms, unix) = crate::clock::logts_now();
    let mut w = FixedBuf { buf, len: 0 };
    match unix {
        Some(secs) => {
            let (secs, _) = advance(secs); // BOOTVERDICTS2 (B616): a frozen civil clock advances on `arch::ms()`
            let (_, _, _, h, m, s) = crate::clock::civil_from_unix(secs);
            let _ = write!(w, "[{:02}:{:02}:{:02}Z] ", h, m, s);
        }
        None => {
            // `None` is "the emission time is not known" — before the bootpace `entry` stamp or
            // before TSC calibration. An explicit `?` keeps the null reading falsifiable: a boot
            // whose EVERY line reads `?` is a machine with no trustworthy counter, which a
            // fabricated `0ms` would render indistinguishable from a very fast boot.
            match mono_ms {
                Some(ms) => {
                    let _ = write!(w, "[{:>7}ms] ", ms);
                }
                None => {
                    let _ = write!(w, "[{:>7}ms] ", '?');
                }
            }
        }
    }
    w.len
}

/// The line-scan shared by every prefixing writer: insert a fresh prefix wherever `state` says a
/// line begins, forward the raw bytes to `inner`, and re-arm `state` at each `\n`.
///
/// `state` is per-SINK, not global. The UART, the FTDI capture ring and the flight recorder each
/// receive the same `Arguments` through separate calls, so a shared flag desynchronizes the moment a
/// `serial_print!` fragment (no trailing `\n`) leaves it `false` for the next sink's fresh line.
/// Each sink's flag is only mutated while that sink's own lock is held, so `Relaxed` still suffices.
fn write_prefixed<W: Write>(inner: &mut W, state: &AtomicBool, s: &str) -> fmt::Result {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if state.load(Ordering::Relaxed) {
            let mut pbuf = [0u8; 20];
            let n = render_prefix(&mut pbuf);
            // pbuf is pure ASCII by construction; the fallback keeps this infallible.
            let pfx = core::str::from_utf8(&pbuf[..n]).unwrap_or("");
            inner.write_str(pfx)?;
            state.store(false, Ordering::Relaxed);
        }
        // Emit up to and including the next newline (which re-arms the line-start flag).
        let start = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        if i < bytes.len() {
            i += 1; // include the '\n'
            inner.write_str(&s[start..i])?;
            state.store(true, Ordering::Relaxed);
        } else {
            inner.write_str(&s[start..i])?;
        }
    }
    Ok(())
}

/// A `fmt::Write` adapter that inserts a timestamp prefix at the start of every line and forwards
/// the raw bytes to `inner` (the arch UART writer). Wrap the UART writer with this in `_print` under
/// the `logts` feature; the raw path is used otherwise, keeping the default byte-identical.
pub struct PrefixWriter<'a, W: Write> {
    pub inner: &'a mut W,
}

impl<W: Write> Write for PrefixWriter<'_, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_prefixed(self.inner, &AT_LINE_START, s)
    }
}

/// CLOCK-2b — the same prefix for the capture TAPS (`ftdi::mirror`, `flight_recorder::capture`).
///
/// The UART wrap above never reaches a machine with no 16550: on the 2012 rMBP the `_print` UART
/// branch is DECLINED and the only transports a bench sitting has are the FTDI capture ring and
/// `UNAOS.LOG` — both of which received raw `args`, so an armed `logts` boot produced a capture with
/// zero timestamps on the one machine that needed them. Each tap wraps its own sink with its own
/// line-start flag (see [`write_prefixed`] for why the flag cannot be shared).
///
/// The flag is only mutated while the tap's ring lock is held. Lines that take a tap's contended
/// STAGE path are deliberately NOT prefixed: their bytes reach the ring at drain time, and a prefix
/// rendered then would stamp the drain, not the emission — a bare line in a `logts` capture reads as
/// "deferred under contention; its time lies between its prefixed neighbors", which is honest.
pub struct TapPrefixWriter<'a, W: Write> {
    pub inner: &'a mut W,
    pub state: &'static AtomicBool,
}

impl<W: Write> Write for TapPrefixWriter<'_, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_prefixed(self.inner, self.state, s)
    }
}

// ---------------------------------------------------------------------------------------------
// LOGWIT-1 — the tap prefix's own witness.
// ---------------------------------------------------------------------------------------------

/// LOGWIT-1: prove on the wire that [`TapPrefixWriter`] actually reached the FTDI capture.
///
/// CLOCK-2b's whole claim is about a stream nobody reads back: the 2012 rMBP has no 16550, so the
/// prefix that matters is the one in the cable's ring, and until now the only evidence that it landed
/// there was that the code compiled. Compiled is not reachable. This emits a nonce marker through the
/// ordinary `serial_println!` path, reads the capture ring back through
/// [`crate::drivers::xhci::ftdi::peek_recent`], and asserts the marker came back carrying a
/// well-formed 12-column prefix immediately before it.
///
/// **Controls.** A matcher that says PASS on anything proves nothing, so the same `judge` routine is
/// run first over two synthetic haystacks it MUST reject: an unprefixed copy of the very marker being
/// looked for, and a copy wearing a malformed 12-byte lookalike prefix. Either acceptance is a
/// distinct verdict — `FORBID`, not `FAIL` — because it does not mean the prefix is missing, it means
/// nothing this witness says about any boot can be believed.
///
/// **Retries.** `mirror` is `try_lock`-only: a contended print is STAGED, and staged lines are folded
/// into the ring deliberately BARE (a drain-time prefix would stamp the drain). So one bare readback is
/// not proof of a broken tap. Up to `ATTEMPTS` nonce markers are emitted and the witness passes on
/// the first that lands prefixed, reporting how many it took; only if EVERY attempt comes back bare or
/// unreadable does it FAIL — which is the honest reading, since a tap that never once takes the direct
/// path has no prefix on the bench's evidence either way.
#[cfg(all(feature = "witness", feature = "logts"))]
mod logwit {
    use super::FixedBuf;
    use core::fmt::Write;

    /// Markers emitted before the witness gives up. See the module item docs: a single bare readback is
    /// explainable by lock contention, a run of them is not.
    const ATTEMPTS: u32 = 4;

    /// Readback window. The marker is emitted immediately before the peek, so it sits at the ring's
    /// tail; 512 bytes is room for it plus anything another core interleaved, and is a stack size the
    /// tree already uses in boot-time fixtures.
    const WINDOW: usize = 512;

    /// What the 12 columns at a line's start were found to be.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        /// `[  12345ms] ` — monotonic ms since kernel entry.
        Mono,
        /// `[      ?ms] ` — the explicit unknown (no origin stamp / uncalibrated counter yet).
        Unknown,
        /// `[15:04:07Z] ` — a civil anchor exists.
        Civil,
    }

    impl Kind {
        fn name(self) -> &'static str {
            match self {
                Kind::Mono => "mono",
                Kind::Unknown => "unknown",
                Kind::Civil => "civil",
            }
        }
    }

    enum Verdict {
        /// The marker was found with a well-formed prefix in the 12 columns before it.
        Prefixed(Kind),
        /// The marker was found, but what precedes it on its line is not a well-formed prefix.
        Bare,
        /// The marker is not in the window, or its line start is not (its line was cut by the window
        /// edge, so the columns before it were never seen and cannot be judged either way).
        Absent,
    }

    /// Classify exactly 12 bytes as one of the three legal prefix forms, or reject them.
    ///
    /// Written against the forms `render_prefix` emits rather than against a loose "starts with `[`"
    /// test: the point of the check is that the WIDTH and the SHAPE survived, since a prefix that
    /// silently changed width is what would break every column-aligned reader of the log.
    fn classify(p: &[u8]) -> Option<Kind> {
        if p.len() != 12 || p[0] != b'[' || p[10] != b']' || p[11] != b' ' {
            return None;
        }
        // `[HH:MM:SSZ] `
        if p[9] == b'Z' {
            let ok = p[1].is_ascii_digit()
                && p[2].is_ascii_digit()
                && p[3] == b':'
                && p[4].is_ascii_digit()
                && p[5].is_ascii_digit()
                && p[6] == b':'
                && p[7].is_ascii_digit()
                && p[8].is_ascii_digit();
            return if ok { Some(Kind::Civil) } else { None };
        }
        // `[<7 cols>ms] ` — the 7 columns are right-justified, so leading spaces then the value.
        if &p[8..10] != b"ms" {
            return None;
        }
        let field = &p[1..8];
        let pad = field.iter().take_while(|&&b| b == b' ').count();
        let val = &field[pad..];
        if val.is_empty() {
            return None;
        }
        if val == b"?" {
            return Some(Kind::Unknown);
        }
        if val.iter().all(|b| b.is_ascii_digit()) {
            Some(Kind::Mono)
        } else {
            None
        }
    }

    /// Find the LAST occurrence of `needle` in `hay` and judge the 12 columns that precede it on its
    /// own line. Shared by the live readback and by both negative controls — a control that ran a
    /// different matcher would control nothing.
    fn judge(hay: &[u8], needle: &[u8]) -> Verdict {
        if needle.is_empty() || hay.len() < needle.len() {
            return Verdict::Absent;
        }
        let mut at = None;
        let mut i = 0;
        while i + needle.len() <= hay.len() {
            if &hay[i..i + needle.len()] == needle {
                at = Some(i);
            }
            i += 1;
        }
        let at = match at {
            Some(a) => a,
            None => return Verdict::Absent,
        };
        // The line start must be one we actually saw. Without a preceding newline inside the window
        // the columns before the marker are simply unknown, and calling that BARE would convict the
        // tap of a truncation the reader caused.
        let start = match hay[..at].iter().rposition(|&b| b == b'\n') {
            Some(nl) => nl + 1,
            None => return Verdict::Absent,
        };
        if at - start != 12 {
            return Verdict::Bare;
        }
        match classify(&hay[start..at]) {
            Some(k) => Verdict::Prefixed(k),
            None => Verdict::Bare,
        }
    }

    /// Render this attempt's nonce marker into `buf`, returning it as a `&str` (pure ASCII).
    fn marker<'a>(buf: &'a mut [u8], seq: u32) -> Option<&'a str> {
        let len = {
            let mut w = FixedBuf { buf, len: 0 };
            let _ = write!(w, ":: LOGWIT-1 probe seq={} ::", seq);
            w.len
        };
        core::str::from_utf8(&buf[..len]).ok()
    }

    pub fn run() {
        // --- negative controls, BEFORE anything is claimed --------------------------------------
        // Control A: the exact marker text, unprefixed. This is the shape fbcon and the staged-line
        // path legitimately produce, and it must not satisfy the matcher.
        // Control B: the same line wearing a 12-byte lookalike that is not one of the legal forms.
        let ctrl_needle = b":: LOGWIT-1 control ::";
        let bare = b"\n:: LOGWIT-1 control ::\n";
        let malformed = b"\n[ 12x45ms] :: LOGWIT-1 control ::\n";
        let bare_ok = matches!(judge(bare, ctrl_needle), Verdict::Bare);
        let malformed_ok = matches!(judge(malformed, ctrl_needle), Verdict::Bare);
        if !bare_ok || !malformed_ok {
            serial_println!(
                ":: LOGWIT-1: matcher ACCEPTED a control it must reject (unprefixed_rejected={} malformed_rejected={}) -> FORBID (matcher vacuous — no CLOCK-2b prefix claim on this boot is evidence) ::",
                bare_ok,
                malformed_ok
            );
            return;
        }

        // --- the live readback -------------------------------------------------------------------
        let mut bare_seen = 0u32;
        let mut absent_seen = 0u32;
        let mut unread = 0u32;
        for seq in 1..=ATTEMPTS {
            let mut nbuf = [0u8; 48];
            let needle = match marker(&mut nbuf, seq) {
                Some(s) => s,
                None => continue,
            };
            // Emit it exactly as it was rendered, through the ordinary path — the tap under test is
            // reached from `_print`, so anything that bypassed `serial_println!` would test nothing.
            serial_println!("{}", needle);

            let mut win = [0u8; WINDOW];
            let n = match crate::drivers::xhci::ftdi::peek_recent(&mut win) {
                Some(n) => n,
                None => {
                    unread += 1;
                    continue;
                }
            };
            match judge(&win[..n], needle.as_bytes()) {
                Verdict::Prefixed(k) => {
                    serial_println!(
                        ":: LOGWIT-1: tap prefix reached the FTDI capture — marker seq={} read back with a well-formed 12-col prefix (kind={}), unprefixed + malformed controls rejected, attempts={} bare={} absent={} unread={} -> PASS ::",
                        seq,
                        k.name(),
                        seq,
                        bare_seen,
                        absent_seen,
                        unread
                    );
                    return;
                }
                Verdict::Bare => bare_seen += 1,
                Verdict::Absent => absent_seen += 1,
            }
        }

        serial_println!(
            ":: LOGWIT-1: no marker came back prefixed from the FTDI capture in {} attempt(s) (bare={} absent={} ring-unreadable={}) — the CLOCK-2b tap prefix is NOT on the bench's evidence stream -> FAIL ::",
            ATTEMPTS,
            bare_seen,
            absent_seen,
            unread
        );
    }
}

/// LOGWIT-1 entry point — see [`logwit`].
#[cfg(all(feature = "witness", feature = "logts"))]
pub fn logwit1() {
    logwit::run();
}

// ---------------------------------------------------------------------------------------------
// BOOTVERDICTS2 (rmbp-ledger B616) — the stamp advances when the civil clock does not.
// ---------------------------------------------------------------------------------------------
//
// KVMLAT/UIREDS26 (B564): on the image-26 KVM boot every line read `[07:13:52Z]` (the RTC anchor) until the SNTP
// sync, then `[07:14:21Z]` for six minutes (docs/dev/evidence/qemu-image26/serial.log, 965 onward). The civil clock
// extends its anchor by `clock::monotonic()`, which on x86 is the INVARIANT TSC only (`clock.rs` `monotonic`, the
// CLOCK-X1 gate on `apic::tsc_invariant`); KVM's default vCPU advertises none (`[clock] source=apic … invariant=0`,
// serial.log:3), so `monotonic()` is `None`, `logts_now`'s elapsed is 0 and every stamp is the anchor's own second.
// The kernel's millisecond clock (`arch::ms()`: CLOCKCORE's tick on that machine) runs on regardless. Where the
// civil clock has no counter, the prefix extends the anchor itself: it latches the anchor's second with `arch::ms()`
// at the first line that shows it (a frozen clock repeats its anchor's second exactly, so a different second IS a
// new anchor) and adds the whole seconds since. The latch lands at the first print after the anchor, so the stamp
// can lag the anchor instant by the gap to that print (the SNTP sync prints its own line at once). The civil clock
// itself (`unix_now`, the menu bar, `time`, FAT mtimes) is unchanged here — `clock.rs` is that fix (B618).

/// The anchor second the fallback last latched (`u64::MAX` = none yet), and `arch::ms()` at that latch.
static LATCH_UNIX: AtomicU64 = AtomicU64::new(u64::MAX);
static LATCH_MS: AtomicU64 = AtomicU64::new(0);

/// Which clock moved the stamp: the civil clock's own counter, or the fallback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StampSrc {
    /// `clock::mono_ticks()` is `Some`: `logts_now` already extrapolated the anchor (aarch64 CNTPCT, x86 invariant TSC or the calibrated ms tick — CLOCKX86 B618).
    Counter,
    /// The civil clock has no counter: the anchor's second plus whole `arch::ms()` seconds since its latch.
    ArchMs,
}

impl StampSrc {
    fn name(self) -> &'static str {
        match self {
            StampSrc::Counter => "counter",
            StampSrc::ArchMs => "arch-ms",
        }
    }
}

/// `unix` is `logts_now`'s civil second. Lock-free (atomics, one `arch::ms()` read, the CPUID+atomic of
/// `mono_ticks`), so it is safe on every print path the prefix is.
fn advance(unix: u64) -> (u64, StampSrc) {
    if crate::clock::mono_ticks().is_some() {
        return (unix, StampSrc::Counter);
    }
    let now = crate::arch::ms();
    if LATCH_UNIX.load(Ordering::Acquire) != unix {
        LATCH_MS.store(now, Ordering::Relaxed);
        LATCH_UNIX.store(unix, Ordering::Release);
        return (unix, StampSrc::ArchMs);
    }
    (unix.saturating_add(now.saturating_sub(LATCH_MS.load(Ordering::Relaxed)) / 1000), StampSrc::ArchMs)
}

/// How long the probe watches one anchor before it says whether the stamp moved.
const PROBE_SPAN_MS: u64 = 3000;
/// 0 = no sample, 1 = sampled (watching), 2 = said for the anchor in `P_BASE`.
static PROBE: AtomicU8 = AtomicU8::new(0);
static P_BASE: AtomicU64 = AtomicU64::new(0);
static P_MS: AtomicU64 = AtomicU64::new(0);
static P_STAMP: AtomicU64 = AtomicU64::new(0);

/// The probe that proves the stamp moves: once per civil anchor (the RTC at boot, then each SNTP sync or `date -s`),
/// sample the stamp, wait [`PROBE_SPAN_MS`] of `arch::ms()`, sample again, and say
/// `[logts] source=<counter|arch-ms> anchor=<rtc|sntp|manual> advancing=<0|1> span_ms=<n> moved_s=<n>`.
/// `advancing=0` is the frozen stamp B564 saw. No verdict word: it is a boot line, not a test (R80).
/// Called from `serial_ring::mirror_service` (IRQs on, no lock held, not a print context), on both arches.
pub fn probe_poll() {
    let Some((base, source)) = crate::clock::raw_anchor() else { return };
    let Some(unix) = crate::clock::logts_now().1 else { return }; // the anchor lock was contended: next pass
    let (stamp, src) = advance(unix);
    let now = crate::arch::ms();
    let st = PROBE.load(Ordering::Relaxed);
    if st == 0 || P_BASE.load(Ordering::Relaxed) != base {
        P_BASE.store(base, Ordering::Relaxed);
        P_MS.store(now, Ordering::Relaxed);
        P_STAMP.store(stamp, Ordering::Relaxed);
        PROBE.store(1, Ordering::Relaxed);
        return;
    }
    let span = now.saturating_sub(P_MS.load(Ordering::Relaxed));
    if st != 1 || span < PROBE_SPAN_MS {
        return;
    }
    PROBE.store(2, Ordering::Relaxed);
    let moved = stamp.saturating_sub(P_STAMP.load(Ordering::Relaxed));
    let anchor = match source {
        crate::clock::ClockSource::Unset => "unset",
        crate::clock::ClockSource::Manual => "manual",
        crate::clock::ClockSource::Sntp { .. } => "sntp",
        crate::clock::ClockSource::Rtc => "rtc",
    };
    serial_println!(
        "[logts] source={} anchor={} advancing={} span_ms={} moved_s={}",
        src.name(),
        anchor,
        (moved > 0) as u8,
        span,
        moved
    );
}
