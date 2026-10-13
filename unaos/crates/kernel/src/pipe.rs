//! CHARTER: Midden — fulfiller (CODEX §2: Midden "manages stdin/stdout"; the kernel carries a launched program's console bytes to the program that launched it)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RING3PIPE (rmbp-ledger B609) — the launcher's read of its child's console output (una-abi's RING3PIPE block).
//!
//! SR111 UNAIDE2 left it owed: a program UNA.ELF's Run verb or MIDDEN.ELF's `run` launches is DETACHED (the `bg`
//! body, `SYS_SPAWN_PATH`) and its output went only to the console, never back to the pane that asked. This module is
//! the arch-neutral table both halves meet in; the arch's syscall layer supplies the keys:
//!
//! * a KEY is the arch's per-address-space row (x86: the process slot `caller_row()`; aarch64: the ASID) and a
//!   GENERATION is that row's tenant generation (x86: `SLOT_GEN`, bumped at the top of the slot's teardown). A
//!   `(key, gen)` pair names one tenant for its whole life and never the next one, so liveness is a comparison,
//!   not a hook: the child is alive while its row's generation is still the one bound here.
//! * [`reserve`] (the launcher, before the spawn: a full table refuses the launch before a slot exists) →
//!   [`bind`] (inside the spawn, in the pre-run window, so the child's first write is captured) → [`set_pid`]
//!   (after the spawn: the pid names the pipe). A launch that fails after the reserve calls [`release`].
//! * [`feed`] is called by the arch's `SYS_WRITE` Console path with the writer's `(key, gen)`: one atomic load
//!   when no pipe is bound, else the matching ring takes what fits and counts the rest as dropped.
//! * [`read`] is `SYS_PIPE_READ`'s body: data (oldest first), else EOF once the child's generation moved (the pipe
//!   is freed by that answer), else empty. Only the launcher's `(key, gen)` reads; anyone else is refused.
//!
//! Wire: `[pipe] open pid=<p> reader=<k> child=<k>` at bind+pid, `[pipe] close pid=<p> bytes=<b> dropped=<d>` at EOF.
//! The ring's bytes are never printed here (the child's own write already went to the console).

use crate::sync::Mutex;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicUsize, Ordering};
use una_abi::{PIPE_MAX, PIPE_RING_BYTES};

/// Slot state: reserved by a launcher, not yet bound to a child.
const RESERVED: u8 = 1;
/// Slot state: bound to a child's `(key, gen)`; feeds land here.
const BOUND: u8 = 2;

struct Pipe {
    state: u8,
    reader: usize,
    reader_gen: u64,
    child: usize,
    child_gen: u64,
    pid: u64,
    ring: Box<[u8]>,
    head: usize,
    len: usize,
    total: u64,
    dropped: u64,
}

static PIPES: Mutex<[Option<Pipe>; PIPE_MAX]> = Mutex::new([const { None }; PIPE_MAX]);
/// Pipes in the BOUND state — `feed`'s fast path (every console write of every program reads it).
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// What [`read`] answers.
pub enum Read {
    /// `n` bytes copied into the caller's buffer.
    Data(usize),
    /// The ring is drained and the child has exited; the pipe is now closed. `(bytes carried, bytes dropped)`.
    Eof(u64, u64),
    /// Empty, the child still runs.
    Empty,
    /// The dropped-byte count (a zero-length read).
    Dropped(u64),
    /// No live pipe has that pid.
    NoPipe,
    /// The pipe is another program's.
    NotYours,
}

/// Reserve a pipe for the launcher `(reader, reader_gen)`. Pipes whose launcher is gone (`alive` says so) are
/// reclaimed first. `Err` names the refusal (the table is full).
pub fn reserve(reader: usize, reader_gen: u64, alive: impl Fn(usize, u64) -> bool) -> Result<usize, &'static str> {
    // The ring is allocated before the lock: the table lock never waits on the allocator.
    let ring = alloc::vec![0u8; PIPE_RING_BYTES].into_boxed_slice();
    let mut t = PIPES.lock();
    for s in t.iter_mut() {
        let dead = matches!(s, Some(p) if !alive(p.reader, p.reader_gen));
        if dead {
            if matches!(s, Some(p) if p.state == BOUND) {
                LIVE.fetch_sub(1, Ordering::AcqRel);
            }
            *s = None;
        }
    }
    let Some(i) = t.iter().position(|s| s.is_none()) else {
        return Err("pipe-table-full");
    };
    t[i] = Some(Pipe { state: RESERVED, reader, reader_gen, child: 0, child_gen: 0, pid: 0, ring, head: 0, len: 0, total: 0, dropped: 0 });
    Ok(i)
}

/// Bind reserved pipe `idx` to the child `(child, child_gen)` — called before the child can run.
pub fn bind(idx: usize, child: usize, child_gen: u64) {
    let mut t = PIPES.lock();
    if let Some(Some(p)) = t.get_mut(idx) {
        if p.state == RESERVED {
            p.child = child;
            p.child_gen = child_gen;
            p.state = BOUND;
            LIVE.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// Name bound pipe `idx` by the child's pid (after the spawn returned it).
pub fn set_pid(idx: usize, pid: u64) {
    let keys = {
        let mut t = PIPES.lock();
        match t.get_mut(idx) {
            Some(Some(p)) => {
                p.pid = pid;
                Some((p.reader, p.child))
            }
            _ => None,
        }
    };
    if let Some((reader, child)) = keys {
        serial_println!("[pipe] open pid={} reader={} child={} ring={}", pid, reader, child, PIPE_RING_BYTES);
    }
}

/// Drop pipe `idx` (a launch that failed after the reserve).
pub fn release(idx: usize) {
    let mut t = PIPES.lock();
    if let Some(s) = t.get_mut(idx) {
        if matches!(s, Some(p) if p.state == BOUND) {
            LIVE.fetch_sub(1, Ordering::AcqRel);
        }
        *s = None;
    }
}

/// The writer `(key, gen)` wrote `bytes` to its console: into its pipe's ring, if it has one.
pub fn feed(key: usize, tgen: u64, bytes: &[u8]) {
    if LIVE.load(Ordering::Acquire) == 0 || bytes.is_empty() {
        return;
    }
    let mut t = PIPES.lock();
    let Some(p) = t.iter_mut().flatten().find(|p| p.state == BOUND && p.child == key && p.child_gen == tgen) else {
        return;
    };
    let cap = p.ring.len();
    let n = bytes.len().min(cap - p.len);
    let mut at = (p.head + p.len) % cap;
    for &b in &bytes[..n] {
        p.ring[at] = b;
        at += 1;
        if at == cap {
            at = 0;
        }
    }
    p.len += n;
    p.total += n as u64;
    p.dropped += (bytes.len() - n) as u64;
}

/// `SYS_PIPE_READ`'s body for the caller `(reader, reader_gen)`: copy up to `out.len()` bytes of pipe `pid` into
/// `out`. `child_alive(key, gen)` is the arch's generation compare.
pub fn read(reader: usize, reader_gen: u64, pid: u64, out: &mut [u8], child_alive: impl Fn(usize, u64) -> bool) -> Read {
    if pid == 0 {
        return Read::NoPipe;
    }
    let mut t = PIPES.lock();
    let Some(slot) = t.iter_mut().find(|s| matches!(s, Some(p) if p.state == BOUND && p.pid == pid)) else {
        return Read::NoPipe;
    };
    let Some(p) = slot.as_mut() else {
        return Read::NoPipe;
    };
    if p.reader != reader || p.reader_gen != reader_gen {
        return Read::NotYours;
    }
    if out.is_empty() {
        return Read::Dropped(p.dropped);
    }
    if p.len > 0 {
        let cap = p.ring.len();
        let n = out.len().min(p.len);
        for o in out[..n].iter_mut() {
            *o = p.ring[p.head];
            p.head += 1;
            if p.head == cap {
                p.head = 0;
            }
        }
        p.len -= n;
        return Read::Data(n);
    }
    if child_alive(p.child, p.child_gen) {
        return Read::Empty;
    }
    let (total, dropped) = (p.total, p.dropped);
    *slot = None;
    LIVE.fetch_sub(1, Ordering::AcqRel);
    drop(t);
    serial_println!("[pipe] close pid={} bytes={} dropped={}", pid, total, dropped);
    Read::Eof(total, dropped)
}
