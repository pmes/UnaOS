// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! SELFDIAG's shared core (rmbp-ledger B324, R82: "the smart installer will need to use vein in order to
//! auto-diagnose ... you can make changes and reboot until the driver works"). Everything the diagnosis
//! program does that is not I/O, so the ring-3 program (`crates/user-diag` → `APPS/DIAG.ELF`) and the
//! kernel's `tests selfdiag` run ONE implementation:
//!
//! * [`witness`] — which serial lines are verdicts (`:: TAG: … -> PASS|FAIL|SKIP ::`, plus `:: BOOT:`), the
//!   tag, the verdict.
//! * [`owners`] — the build-generated table `TAG<TAB>repo path<TAB>line` (scripts/witness-owners.py).
//! * [`prompt`] — the Vein request text: the failing lines, each owner's section, the ask for one diff.
//! * [`diff`] — the unified-diff parser (fenced or bare), bounded, no allocator.
//! * [`apply`] — the hunk locator (old side exact, offset fuzz ±[`apply::FUZZ`] lines, refuse on mismatch)
//!   and the streaming emitter, both over a [`apply::ReadAt`] source, so a 2 MiB kernel file never has to
//!   fit in a ring-3 program's memory.
//! * [`record`] — `/var/log/diag.<n>.md` and the next boot's verdict section.
//! * [`fixture`] — the Echo provider's canned answer for the canned FAIL line (no key, no network).
#![no_std]
#![forbid(unsafe_code)]

pub mod apply;
pub mod diff;
pub mod fixture;
pub mod owners;
pub mod prompt;
pub mod record;
pub mod witness;

/// A bounded byte writer over a caller buffer: never panics, records overflow.
pub struct Out<'a> {
    buf: &'a mut [u8],
    n: usize,
    over: bool,
}

impl<'a> Out<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Out { buf, n: 0, over: false }
    }
    pub fn put(&mut self, b: &[u8]) -> &mut Self {
        if self.over || self.n + b.len() > self.buf.len() {
            self.over = true;
            return self;
        }
        self.buf[self.n..self.n + b.len()].copy_from_slice(b);
        self.n += b.len();
        self
    }
    pub fn s(&mut self, s: &str) -> &mut Self {
        self.put(s.as_bytes())
    }
    pub fn dec(&mut self, mut v: u64) -> &mut Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..]);
        self
    }
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    pub fn overflowed(&self) -> bool {
        self.over
    }
    /// The bytes written so far (a prefix when it overflowed).
    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.n]
    }
    /// The written length, or `None` if anything overflowed.
    pub fn done(self) -> Option<usize> {
        if self.over { None } else { Some(self.n) }
    }
}

/// Parse a decimal `u64` (ASCII digits only, at least one, no overflow).
pub fn parse_dec(b: &[u8]) -> Option<u64> {
    if b.is_empty() || b.len() > 19 {
        return None;
    }
    let mut v: u64 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as u64;
    }
    Some(v)
}

/// Lines of `text` without their `\n` (and without a trailing `\r`).
pub fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l))
}
