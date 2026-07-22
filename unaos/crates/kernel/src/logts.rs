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
//! ## The knob
//!
//! Gated entirely by the `logts` cargo feature (env `UNAOS_LOGTS=1`, wired in `arroyo` + the
//! builder). DEFAULT OFF: with the feature absent this module is not compiled and `_print` writes
//! the raw args, so the serial byte-stream is IDENTICAL to a plain build — the witness batteries and
//! mbench specs, which parse serial lines, are unaffected.
//!
//! ## The format (fixed-width, 12 columns)
//!
//! * pre-sync (no civil anchor yet): `[  12345ms] ` — monotonic milliseconds since boot,
//!   right-justified in 7 columns. Available from the very first print on aarch64 (CNTPCT is always
//!   live) and printed as `[      0ms] ` on x86 until the TSC is calibrated (honest zero, never a
//!   panic).
//! * post-sync (a civil anchor exists — SNTP on the pi, `setdate` on either arch): `[15:04:07Z] ` —
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
use core::sync::atomic::{AtomicBool, Ordering};

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
            let (_, _, _, h, m, s) = crate::clock::civil_from_unix(secs);
            let _ = write!(w, "[{:02}:{:02}:{:02}Z] ", h, m, s);
        }
        None => {
            let _ = write!(w, "[{:>7}ms] ", mono_ms.unwrap_or(0));
        }
    }
    w.len
}

/// A `fmt::Write` adapter that inserts a timestamp prefix at the start of every line and forwards
/// the raw bytes to `inner` (the arch UART writer). Wrap the UART writer with this in `_print` under
/// the `logts` feature; the raw path is used otherwise, keeping the default byte-identical.
pub struct PrefixWriter<'a, W: Write> {
    pub inner: &'a mut W,
}

impl<W: Write> Write for PrefixWriter<'_, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if AT_LINE_START.load(Ordering::Relaxed) {
                let mut pbuf = [0u8; 20];
                let n = render_prefix(&mut pbuf);
                // pbuf is pure ASCII by construction; the fallback keeps this infallible.
                let pfx = core::str::from_utf8(&pbuf[..n]).unwrap_or("");
                self.inner.write_str(pfx)?;
                AT_LINE_START.store(false, Ordering::Relaxed);
            }
            // Emit up to and including the next newline (which re-arms the line-start flag).
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // include the '\n'
                self.inner.write_str(&s[start..i])?;
                AT_LINE_START.store(true, Ordering::Relaxed);
            } else {
                self.inner.write_str(&s[start..i])?;
            }
        }
        Ok(())
    }
}
