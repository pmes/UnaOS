// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! CHARTER: Kernel — kernel-by-ruling (FLIGHTRING B400, R88: the ONE ring of the boot's text both arches' serial seams feed; the console and `UNAOS.LOG` read it)
//!
//! FLIGHTRING (rmbp-ledger B400). The boot's text, kept in ONE ring fed from each arch's one serial print seam
//! (x86 `arch/x86_64/serial.rs::_print` through `flight_recorder::capture`; aarch64 `arch/aarch64/serial.rs::_print`).
//! Two halves, both always readable:
//!   * the PINNED head — the boot's first [`PINNED_CAP`] bytes, never evicted (what a post-mortem needs);
//!   * the ROLLING ring — the newest N bytes, always present (what a console opened late needs). A static
//!     [`BOOT_ROLL`] bootstrap rolls from the first byte; [`arm_once`] (the first main-loop service, heap up) moves it
//!     to a heap buffer sized from memory (heap free ÷ 64, floor [`FLOOR_ROLL`], ceiling [`CEIL_ROLL`]).
//!
//! Before this arc the x86 recorder kept the FIRST 256 KiB and stopped (flight 24 card 3:
//! `[console] prefill … ring_bytes=262144 ring_full=1` — the console showed the ring's end, not the wire's), and
//! aarch64 kept nothing (`ring=none`). Readers: [`tail`] (the console's prefill and scroll-back) and [`file_image`]
//! (`UNAOS.LOG`). Where the two halves do not join, the view carries a marker line between them.
//!
//! Capture is alloc-free and `try_lock` only (an IRQ-masked print context may arrive here); a contended line is
//! staged lock-free and folded in by the next holder, in order — the SERWIT-2 shape the x86 recorder already had.

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

/// The pinned head: the boot's first 64 KiB.
pub const PINNED_CAP: usize = 64 * 1024;
/// The static bootstrap of the rolling ring (rolls from the first byte; the heap ring replaces it at arming).
pub const BOOT_ROLL: usize = 192 * 1024;
/// The rolling ring's floor once armed.
pub const FLOOR_ROLL: usize = 1024 * 1024;
/// The rolling ring's ceiling (a reader walks it under the lock; keep that walk bounded).
pub const CEIL_ROLL: usize = 4 * 1024 * 1024;

/// The ring's indices, shared by the live ring and a test's heap ring (the same code writes both).
#[derive(Clone, Copy)]
pub struct Core {
    head_len: usize,
    /// Next write index into the rolling buffer.
    wpos: usize,
    /// Valid bytes in the rolling buffer (`<=` its length).
    held: usize,
    /// Bytes ever appended (the absolute offset of the rolling buffer's newest byte + 1).
    total: u64,
}

impl Core {
    pub const fn new() -> Self {
        Core { head_len: 0, wpos: 0, held: 0, total: 0 }
    }

    /// Append to both halves: the head until it is full, the rolling ring always (oldest bytes overwritten).
    pub fn append(&mut self, head: &mut [u8], roll: &mut [u8], bytes: &[u8]) {
        if self.head_len < head.len() {
            let n = core::cmp::min(head.len() - self.head_len, bytes.len());
            head[self.head_len..self.head_len + n].copy_from_slice(&bytes[..n]);
            self.head_len += n;
        }
        let cap = roll.len();
        self.total = self.total.wrapping_add(bytes.len() as u64);
        if cap == 0 {
            return;
        }
        if bytes.len() >= cap {
            roll.copy_from_slice(&bytes[bytes.len() - cap..]);
            self.wpos = 0;
            self.held = cap;
            return;
        }
        let n = bytes.len();
        let first = core::cmp::min(cap - self.wpos, n);
        roll[self.wpos..self.wpos + first].copy_from_slice(&bytes[..first]);
        roll[..n - first].copy_from_slice(&bytes[first..]);
        self.wpos = (self.wpos + n) % cap;
        self.held = core::cmp::min(self.held + n, cap);
    }

    /// The rolling buffer's contents oldest-first, as (at most) two slices.
    fn roll_segs<'a>(&self, roll: &'a [u8]) -> (&'a [u8], &'a [u8]) {
        let cap = roll.len();
        if cap == 0 || self.held == 0 {
            return (&[], &[]);
        }
        let start = (self.wpos + cap - self.held) % cap;
        let a_len = core::cmp::min(self.held, cap - start);
        (&roll[start..start + a_len], &roll[..self.held - a_len])
    }

    /// Absolute offset of the rolling buffer's oldest byte.
    fn roll_from(&self) -> u64 {
        self.total - self.held as u64
    }

    /// The pinned head and the rolling part meet (nothing rolled out between them).
    pub fn joined(&self) -> bool {
        self.roll_from() <= self.head_len as u64
    }

    /// The rolling ring has overwritten its oldest bytes at least once.
    pub fn wrapped(&self, roll_cap: usize) -> bool {
        self.total > roll_cap as u64
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// Re-home the rolling part into `to` (oldest-first). `to` must be at least `held` long.
    fn rehome(&mut self, from: &[u8], to: &mut [u8]) {
        let (a, b) = self.roll_segs(from);
        to[..a.len()].copy_from_slice(a);
        to[a.len()..a.len() + b.len()].copy_from_slice(b);
        self.held = a.len() + b.len();
        self.wpos = self.held % to.len().max(1);
    }

    /// The readable view: head, (marker), rolling — with the overlap removed when they join, and the rolling part's
    /// partial first line dropped when they do not. `marker` is the caller's storage for the marker line.
    fn view<'a>(&self, head: &'a [u8], roll: &'a [u8], marker: &'a mut alloc::string::String) -> View<'a> {
        let head = &head[..self.head_len];
        let (mut a, mut b) = self.roll_segs(roll);
        let mut segs: [&'a [u8]; 4] = [head, &[], &[], &[]];
        if self.joined() {
            // Skip the bytes the head already holds.
            let mut skip = (self.head_len as u64 - self.roll_from()) as usize;
            let s = core::cmp::min(skip, a.len());
            a = &a[s..];
            skip -= s;
            let s = core::cmp::min(skip, b.len());
            b = &b[s..];
        } else {
            // Drop the partial line the rolling part starts with.
            if let Some(i) = a.iter().position(|&c| c == b'\n') {
                a = &a[i + 1..];
            } else if let Some(i) = b.iter().position(|&c| c == b'\n') {
                a = &[];
                b = &b[i + 1..];
            } else {
                a = &[];
                b = &[];
            }
            let gap = self.roll_from() - self.head_len as u64 + (self.held - a.len() - b.len()) as u64;
            let nl = if head.last().map_or(true, |&c| c == b'\n') { "" } else { "\n" };
            *marker = alloc::format!(
                "{}:: FLIGHTRING: ---- pinned head ends ({} KiB); {} byte(s) rolled out; the rolling ring begins ---- ::\n",
                nl,
                PINNED_CAP / 1024,
                gap
            );
            segs[1] = marker.as_bytes();
        }
        segs[2] = a;
        segs[3] = b;
        View { segs }
    }
}

/// The view as up to four slices read as one byte string.
struct View<'a> {
    segs: [&'a [u8]; 4],
}

impl View<'_> {
    fn len(&self) -> usize {
        self.segs.iter().map(|s| s.len()).sum()
    }
    fn at(&self, mut i: usize) -> u8 {
        for s in self.segs.iter() {
            if i < s.len() {
                return s[i];
            }
            i -= s.len();
        }
        0
    }
    fn copy(&self, from: usize, to: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(to - from);
        let mut base = 0usize;
        for s in self.segs.iter() {
            let (lo, hi) = (from.max(base), to.min(base + s.len()));
            if lo < hi {
                out.extend_from_slice(&s[lo - base..hi - base]);
            }
            base += s.len();
        }
        out
    }
    /// End of the last whole line.
    fn end(&self) -> usize {
        let mut i = self.len();
        while i > 0 && self.at(i - 1) != b'\n' {
            i -= 1;
        }
        i
    }
    /// The start of the line that ends at `pos` (exclusive; `pos` follows a `\n`), or `None` at the view's start.
    fn line_start(&self, pos: usize) -> Option<usize> {
        if pos == 0 {
            return None;
        }
        let mut i = pos - 1;
        while i > 0 && self.at(i - 1) != b'\n' {
            i -= 1;
        }
        Some(i)
    }
    /// `max_lines` whole lines ending `back` lines before the end. Returns (from, to, lines, back_used, at_top).
    fn tail_span(&self, max_lines: usize, back: usize) -> (usize, usize, usize, usize, bool) {
        let mut to = self.end();
        let mut used = 0usize;
        while used < back {
            match self.line_start(to) {
                Some(s) => {
                    to = s;
                    used += 1;
                }
                None => break,
            }
        }
        let mut from = to;
        let mut lines = 0usize;
        let mut top = false;
        while lines < max_lines {
            match self.line_start(from) {
                Some(s) => {
                    from = s;
                    lines += 1;
                }
                None => {
                    top = true;
                    break;
                }
            }
        }
        if from == 0 {
            top = true;
        }
        (from, to, lines, used, top)
    }
}

/// What a reader got: the lines' bytes plus the ring's shape.
pub struct Tail {
    pub bytes: Vec<u8>,
    pub lines: usize,
    /// Lines actually skipped from the end (`<=` the `back` asked for).
    pub back: usize,
    /// The span reaches the view's first line.
    pub at_top: bool,
    pub total: u64,
    pub wrapped: bool,
    pub joined: bool,
    pub rolling_kib: usize,
}

/// A heap ring of the live shape (the test writes it past its capacity without touching the boot's log).
pub struct HeapRing {
    pub core: Core,
    pub head: Vec<u8>,
    pub roll: Vec<u8>,
}

impl HeapRing {
    pub fn new(head: usize, roll: usize) -> Option<Self> {
        let mut h = Vec::new();
        let mut r = Vec::new();
        h.try_reserve_exact(head).ok()?;
        r.try_reserve_exact(roll).ok()?;
        h.resize(head, 0);
        r.resize(roll, 0);
        Some(HeapRing { core: Core::new(), head: h, roll: r })
    }
    pub fn append(&mut self, bytes: &[u8]) {
        self.core.append(&mut self.head, &mut self.roll, bytes);
    }
    pub fn tail(&self, max_lines: usize, back: usize) -> Tail {
        tail_of(&self.core, &self.head, &self.roll, max_lines, back)
    }
}

fn tail_of(core: &Core, head: &[u8], roll: &[u8], max_lines: usize, back: usize) -> Tail {
    let mut marker = alloc::string::String::new();
    let v = core.view(head, roll, &mut marker);
    let (from, to, lines, used, top) = v.tail_span(max_lines, back);
    Tail {
        bytes: v.copy(from, to),
        lines,
        back: used,
        at_top: top,
        total: core.total(),
        wrapped: core.wrapped(roll.len()),
        joined: core.joined(),
        rolling_kib: roll.len() / 1024,
    }
}

// ── The live ring ───────────────────────────────────────────────────────────────────────────────────────────────

struct Live {
    core: Core,
    head: [u8; PINNED_CAP],
    boot: [u8; BOOT_ROLL],
    big: Option<&'static mut [u8]>,
}

impl Live {
    fn append(&mut self, bytes: &[u8]) {
        let roll: &mut [u8] = match self.big.as_deref_mut() {
            Some(b) => b,
            None => &mut self.boot,
        };
        self.core.append(&mut self.head, roll, bytes);
    }
    fn roll(&self) -> &[u8] {
        match self.big.as_deref() {
            Some(b) => b,
            None => &self.boot,
        }
    }
}

impl fmt::Write for Live {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.append(s.as_bytes());
        Ok(())
    }
}

static RING: Mutex<Live> = Mutex::new(Live {
    core: Core::new(),
    head: [0u8; PINNED_CAP],
    boot: [0u8; BOOT_ROLL],
    big: None,
});

/// SERWIT-2 staging ring (64 × 240 B): a contended line waits here, lock-free, for the next holder.
static STAGE: crate::serial_ring::LineRing<64, 240> = crate::serial_ring::LineRing::new();

/// Lines staged but not yet folded into the ring.
pub fn staged_in_flight() -> u64 {
    STAGE.in_flight()
}

/// The serial seam's tap: alloc-free, `try_lock` only, never blocks; a contended line is staged, not lost.
pub fn capture(args: fmt::Arguments) {
    let tap = &crate::serial_ring::TAP_FLIGHTREC;
    tap.submit();
    if let Some(mut ring) = RING.try_lock() {
        drain_staged(&mut ring);
        let _ = ring_write(&mut ring, args);
        tap.absorb();
        return;
    }
    match STAGE.stage(args) {
        crate::serial_ring::Staged::Whole => {
            tap.note_staged();
            return;
        }
        crate::serial_ring::Staged::Truncated => {
            tap.note_staged();
            tap.tear();
            return;
        }
        crate::serial_ring::Staged::Full => {}
    }
    if let Some(mut ring) = RING.try_lock() {
        drain_staged(&mut ring);
        let _ = ring_write(&mut ring, args);
        tap.absorb();
        return;
    }
    tap.drop_line();
}

#[cfg(feature = "logts")]
static LINE_START: AtomicBool = AtomicBool::new(true);

/// CLOCK-2b: timestamp-prefixed under `logts`, as the FTDI capture is.
fn ring_write(ring: &mut Live, args: fmt::Arguments) -> fmt::Result {
    #[cfg(feature = "logts")]
    {
        use core::fmt::Write;
        crate::logts::TapPrefixWriter { inner: ring, state: &LINE_START }.write_fmt(args)
    }
    #[cfg(not(feature = "logts"))]
    fmt::write(ring, args)
}

/// Fold the staged lines in, in order. Caller holds `RING`. Pure memcpy.
fn drain_staged(ring: &mut Live) {
    #[cfg(feature = "logts")]
    let mut last_byte: Option<u8> = None;
    let n = STAGE.drain(|s| {
        ring.append(s.as_bytes());
        #[cfg(feature = "logts")]
        {
            last_byte = s.as_bytes().last().copied().or(last_byte);
        }
    });
    crate::serial_ring::TAP_FLIGHTREC.absorb_n(n);
    #[cfg(feature = "logts")]
    if let Some(b) = last_byte {
        LINE_START.store(b == b'\n', Ordering::Relaxed);
    }
}

/// Bytes ever captured (`None` while contended).
pub fn total() -> Option<u64> {
    RING.try_lock().map(|r| r.core.total())
}

/// The rolling ring's size in KiB (0 while contended).
pub fn rolling_kib() -> usize {
    RING.try_lock().map(|r| r.roll().len() / 1024).unwrap_or(0)
}

/// `max_lines` whole lines ending `back` lines before the newest (the console's prefill: `back = 0`). A few
/// `try_lock` retries; `None` when the ring stays contended.
pub fn tail(max_lines: usize, back: usize) -> Option<Tail> {
    for _ in 0..64 {
        if let Some(ring) = RING.try_lock() {
            return Some(tail_of(&ring.core, &ring.head, ring.roll(), max_lines, back));
        }
        core::hint::spin_loop();
    }
    None
}

/// The `UNAOS.LOG` body within `budget` bytes: the whole view when it fits, else the pinned head, a marker naming
/// what was left out, and the newest whole lines that fit. Returns the body and the captured total.
pub fn file_image(budget: usize) -> Option<(Vec<u8>, u64)> {
    let ring = RING.try_lock()?;
    let mut marker = alloc::string::String::new();
    let core = ring.core;
    let v = core.view(&ring.head, ring.roll(), &mut marker);
    let len = v.len();
    if len <= budget {
        return Some((v.copy(0, len), core.total()));
    }
    let head_len = v.segs[0].len();
    let note_room = 192usize;
    let room = budget.saturating_sub(head_len + note_room);
    // The newest `room` bytes, from a line start.
    let mut from = len - room;
    while from < len && v.at(from - 1) != b'\n' {
        from += 1;
    }
    let mut out = Vec::with_capacity(budget);
    out.extend_from_slice(v.segs[0]);
    if out.last().map_or(false, |&c| c != b'\n') {
        out.push(b'\n');
    }
    let skipped = core.total().saturating_sub((head_len + (len - from)) as u64);
    let note = alloc::format!(
        ":: FLIGHTRING: ---- pinned head ends ({} KiB); {} byte(s) of the ring left out of this file; the newest {} byte(s) follow ---- ::\n",
        PINNED_CAP / 1024,
        skipped,
        len - from
    );
    out.extend_from_slice(&note.as_bytes()[..note.len().min(note_room)]);
    out.extend_from_slice(&v.copy(from, len));
    Some((out, core.total()))
}

static ARMED: AtomicBool = AtomicBool::new(false);
static ARMED_KIB: AtomicU64 = AtomicU64::new(0);

/// Move the rolling ring onto the heap, sized from memory — once, from the first main-loop service (heap up,
/// unmasked, not a print context). One line either way.
pub fn arm_once() {
    if ARMED.swap(true, Ordering::AcqRel) {
        return;
    }
    let free = crate::allocator::heap_census(4096).free;
    let want = ((free / 64).clamp(FLOOR_ROLL, CEIL_ROLL)) & !0x3FF;
    let mut v: Vec<u8> = Vec::new();
    let buf: Option<&'static mut [u8]> = if v.try_reserve_exact(want).is_ok() {
        v.resize(want, 0);
        Some(alloc::boxed::Box::leak(v.into_boxed_slice()))
    } else {
        None
    };
    let (kib, total) = match buf {
        Some(b) => {
            let mut ring = RING.lock();
            let ring = &mut *ring;
            let from: &[u8] = &ring.boot;
            let mut core = ring.core;
            core.rehome(from, b);
            ring.core = core;
            ring.big = Some(b);
            (want / 1024, ring.core.total())
        }
        None => (BOOT_ROLL / 1024, total().unwrap_or(0)),
    };
    ARMED_KIB.store(kib as u64, Ordering::Relaxed);
    serial_println!(
        "[flight] ring rolling_kib={} pinned_head_kib={} heap_free_mib={} captured={}{} (FLIGHTRING B400: the newest {} KiB always held, the boot's first {} KiB pinned)",
        kib,
        PINNED_CAP / 1024,
        free >> 20,
        total,
        if kib == BOOT_ROLL / 1024 { " reason=heap-refused" } else { "" },
        kib,
        PINNED_CAP / 1024
    );
}
