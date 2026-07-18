// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// GUI-WITNESS M1: a small, lock-light boot-milestone recorder.
//
// The problem of record: a GUI (non-usbdebug) boot emits ZERO serial (cause under separate
// investigation) and detaches fbcon at the GUI handoff, so once the panel is live there is no
// witness surface at all — serial cannot witness its own silence. This module is that surface: a
// fixed-size ring of short `&'static` milestone tags, each stamped with `arch::ms()`, written from
// the existing milestone call sites (PORTSW flip, FTDI console up/failed, EHCI HID armed, block
// device up, GUI handoff). It is purely additive — recording a milestone changes no behaviour of
// the milestone itself — and the ring is readable AFTER the GUI owns the screen via the `bootlog`
// shell verb (see shell.rs), the operator's eyes at the bench.
//
// Heap-free by construction (a `static` with an inline array, like the FTDI capture ring): the
// earliest milestone (PORTSW) predates nothing here, but keeping it allocation-free means a record
// call is always safe regardless of heap/lock state and never itself perturbs a boot. Tags are
// `&'static str` so an entry is `Copy` and no per-entry buffer or formatting is needed.

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

/// Ring capacity. Far more than the handful of boot milestones we record; drop-oldest on overflow
/// keeps the LATEST milestones (the ones nearest a late failure) if a build ever records more.
const CAP: usize = 32;

/// One milestone: the wall-clock (`arch::ms()`) at which it was reached, and its short static tag.
type Entry = (u64, &'static str);

struct Ring {
    slots: [Entry; CAP],
    /// Index of the oldest valid entry.
    head: usize,
    /// Number of valid entries currently buffered (0..=CAP).
    len: usize,
}

impl Ring {
    const fn new() -> Self {
        Ring { slots: [(0, ""); CAP], head: 0, len: 0 }
    }

    fn record(&mut self, ms: u64, tag: &'static str) {
        if self.len == CAP {
            self.head = (self.head + 1) % CAP;
            self.len -= 1;
        }
        let tail = (self.head + self.len) % CAP;
        self.slots[tail] = (ms, tag);
        self.len += 1;
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring::new());

/// Record a boot milestone. `tag` is a short static identifier, e.g. `"portsw:flip"`,
/// `"ftdi:console-up"`, `"ftdi:failed"`, `"ehci:hid-armed"`, `"block:up"`, `"gui:handoff"`.
/// Lock-light: takes the ring mutex for a couple of array writes and releases it. Never blocks,
/// never allocates. Safe to call from any milestone site on any arch.
pub fn record(tag: &'static str) {
    let ms = crate::arch::ms();
    RING.lock().record(ms, tag);
    // QUIET-PANEL on-panel leg: paint the milestone line. On a quiet GUI boot these are the ONLY
    // lines the panel shows before the handoff; a no-op on headless/test builds and after the
    // handoff. Called OUTSIDE the ring lock (fbcon takes its own).
    crate::video::fbcon::milestone(ms, tag);
}

/// Copy the buffered milestones (oldest→newest) into `out`, returning how many were written.
/// Snapshots under the lock into the caller's buffer, then releases it BEFORE the caller prints —
/// so console/serial I/O never runs while the ring is locked.
pub fn snapshot(out: &mut [Entry]) -> usize {
    let r = RING.lock();
    let n = r.len.min(out.len());
    for i in 0..n {
        out[i] = r.slots[(r.head + i) % CAP];
    }
    n
}

/// The ring's fixed capacity — a caller sizes its snapshot buffer to this to see every milestone.
pub const fn capacity() -> usize {
    CAP
}

/// Last ring length surfaced by the serial-dump service below (starts sentinel so the first call
/// always prints, even with zero milestones, giving a stable "ring is empty" artifact if a build
/// records none).
static LAST_DUMPED_LEN: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Serial-dump SERVICE for the QEMU/serial proof path (M3). Called each iteration of the main loop
/// on a build where serial IS live (QEMU, or a usbdebug metal boot); it re-prints the full milestone
/// ring ONLY when the ring has grown since the last print. Because several milestones (FTDI console
/// up, block device up) are recorded from inside the main loop, a single one-shot dump would race
/// them — dumping on growth guarantees the LAST dump in the serial log is the complete ring, while
/// staying bounded (at most one print per recorded milestone). Additive: emits diagnostic lines
/// only. On the silent GUI metal boot this prints to a dead wire (harmless); the panel-side witness
/// there is the `bootlog` shell verb, which reads the same ring.
pub fn service_serial_dump() {
    let mut buf = [(0u64, ""); CAP];
    let n = snapshot(&mut buf);
    if LAST_DUMPED_LEN.swap(n, Ordering::AcqRel) == n {
        return;
    }
    serial_println!(":: BOOTLOG: {} milestone(s) recorded (GUI-WITNESS ring) ::", n);
    for (ms, tag) in &buf[..n] {
        serial_println!(":: BOOTLOG: [{:>8} ms] {} ::", ms, tag);
    }
}
