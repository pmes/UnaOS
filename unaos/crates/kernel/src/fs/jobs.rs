// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Mica — shared-core
//!
//! UNAOSVOLUME (rmbp-ledger B427) — **the jobs on the UnaOS volume, as the kernel reads them.** Peter, 2026-10-06:
//! "we could use the image we write to our boot disk as the working drive so whenever UnaOS is ready to run on its
//! own everything is already in place".
//!
//! The store is `/jobs` on the UnaFS root (the card build puts it there with `mica jobs build --into`, the same
//! builder as `./arroyo jobs-image`); its layout and attribute names are `jobs_core`'s, the ONE core Mica
//! (`handlers/mica`, CODEX §2's Ledger, the only writer) links too. The kernel is a READER this arc: Quarry lists
//! `/jobs/...` as ordinary folders with ATTRCOLUMNS' columns (`una:view`, written by the builder), and
//! `/jobs/queries/Open jobs` is QUERYFOLDER's saved-query shape — no second store, no second query engine.
//!
//! [`owe`] runs at `login ok` (R93: what the session has is said when it opens); the count is a SCAN, not an inline
//! read (JOBSCAN, B497 — flight 27: the inline read on the render task held the glass 17 s, one IRQ-masked UnaFS
//! transaction per directory with an inode read per row, 10.9 s masked): on x86 it runs on the `jobs-scan` task on a
//! worker core (never the render core), AFTER the bar is painted (`desktopbuild::settled`), one directory blob per
//! chunk ([`MountTable::read_dir_kinds`] — names and kinds, no inode reads), a 1 ms sleep between chunks. Then the
//! worker builds the FILETYPES registry the login owed (`assoc::build_owed`), off the device-service pass. Lines:
//!   `[jobs] scan pending at=login on=<worker|inline> [cpu=<c>]`
//!   `[jobs] scan chunk=<n>/<m> records=<r> ms=<ms>` (one per directory)
//!   `[jobs] volume=/jobs records=<n> claims=<n> ledger=<n> queue=<n> queries=<n> on=<worker|inline> chunks=<m>
//!    max_chunk_ms=<ms> waited_ms=<bar wait> ms=<total>` (or `volume=absent records=0`).
//! [`counts`] is `None` (pending) until the scan lands. `tests jobscan` (R80: only when asked):
//!   `:: JOBSCAN: on=<worker|inline> chunks=<n> max_chunk_ms=<ms> bar_ms=<ms> -> PASS|FAIL ::`.

use crate::fs::vfs::{MountTable, NodeKind};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::{AcqRel, Acquire, Relaxed, Release}};

/// 0 = never asked, 1 = pending (the scan is running), 2 = landed.
static STATE: AtomicU8 = AtomicU8::new(0);
static CLAIMS: AtomicU32 = AtomicU32::new(0);
static LEDGER: AtomicU32 = AtomicU32::new(0);
static QUEUE: AtomicU32 = AtomicU32::new(0);
static QUERIES: AtomicU32 = AtomicU32::new(0);
static CHUNKS: AtomicU32 = AtomicU32::new(0);
static MAX_CHUNK_MS: AtomicU64 = AtomicU64::new(0);
static ON_WORKER: AtomicBool = AtomicBool::new(false);
/// How long the worker waits for the bar before scanning anyway.
const BAR_WAIT_MS: u64 = 10_000;

/// What `/jobs` holds, once the scan has landed.
#[derive(Clone, Copy)]
pub struct Counts {
    pub records: u32,
    pub claims: u32,
    pub ledger: u32,
    pub queue: u32,
    pub queries: u32,
}

/// The counts, or `None` while the scan is pending (or was never asked). No UI reads them yet (the numbers live on
/// the wire line; Quarry lists `/jobs/...` directly) — this is the read a surface takes.
pub fn counts() -> Option<Counts> {
    if STATE.load(Acquire) != 2 {
        return None;
    }
    let (c, l, q) = (CLAIMS.load(Relaxed), LEDGER.load(Relaxed), QUEUE.load(Relaxed));
    Some(Counts { records: c + l + q, claims: c, ledger: l, queue: q, queries: QUERIES.load(Relaxed) })
}

/// `idle` · `pending` · `landed`.
pub fn state() -> &'static str {
    match STATE.load(Acquire) {
        1 => "pending",
        2 => "landed",
        _ => "idle",
    }
}

fn sub(name: &str) -> String {
    format!("{}/{}", jobs_core::ROOT, name)
}

/// `login ok`: the scan is owed. On x86 it runs on the `jobs-scan` worker task; elsewhere inline (still chunked).
pub fn owe() {
    ensure_tests();
    if STATE.swap(1, AcqRel) == 1 {
        return; // a scan is already in flight
    }
    #[cfg(target_arch = "x86_64")]
    if let Some(cpu) = crate::arch::smp::worker_cpu(0) {
        crate::fs::assoc::take_to_worker();
        ON_WORKER.store(true, Relaxed);
        serial_println!("[jobs] scan pending at=login on=worker cpu={}", cpu);
        crate::arch::sched::spawn("jobs-scan", worker, 0, cpu, crate::arch::sched::PRIO_NORMAL);
        return;
    }
    ON_WORKER.store(false, Relaxed);
    serial_println!("[jobs] scan pending at=login on=inline");
    scan(0);
}

/// Is the bar painted (the desktop built and its first paint read back)? `true` where no desktop object exists.
fn bar_settled() -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        crate::video::desktopbuild::settled()
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        true
    }
}

/// `jobs-scan`: wait (bounded) for the bar, scan, then build the registry the login owed. Exits when done.
#[cfg(target_arch = "x86_64")]
fn worker(_: usize) {
    let t0 = crate::arch::ms();
    while !bar_settled() && crate::arch::ms().saturating_sub(t0) < BAR_WAIT_MS {
        crate::arch::sched::sleep_ms(10);
    }
    scan(crate::arch::ms().saturating_sub(t0));
    crate::fs::assoc::build_owed(); // FILETYPES (B423): the owed registry, here — not on the pass that pumps hid and paints the bar
}

/// One chunk: one directory blob (one UnaFS transaction). `(files, sub-folders, ms)`.
fn chunk(mt: &MountTable, dir: &str, nested: bool) -> (u32, Vec<String>, u64) {
    let c0 = crate::arch::ms();
    let mut files = 0u32;
    let mut subs = Vec::new();
    if let Ok(list) = mt.read_dir_kinds(dir) {
        for (name, kind) in list {
            match kind {
                NodeKind::File => files += 1,
                NodeKind::Dir if nested => subs.push(format!("{}/{}", dir, name)),
                NodeKind::Dir => {}
            }
        }
    }
    (files, subs, crate::arch::ms().saturating_sub(c0))
}

/// Between chunks: the core is unmasked and the scheduler runs the rest (the hid pump, the IPIs).
fn breathe() {
    #[cfg(target_arch = "x86_64")]
    if ON_WORKER.load(Relaxed) {
        crate::arch::sched::sleep_ms(1);
    }
}

fn scan(waited_ms: u64) {
    let t0 = crate::arch::ms();
    let mt = crate::shell::vfs_mount_table();
    match mt.stat(jobs_core::ROOT) {
        Ok(s) if matches!(s.kind, NodeKind::Dir) => {}
        _ => {
            for a in [&CLAIMS, &LEDGER, &QUEUE, &QUERIES, &CHUNKS] {
                a.store(0, Relaxed);
            }
            STATE.store(2, Release);
            serial_println!("[jobs] volume=absent records=0");
            return;
        }
    }
    // The four top folders first (m is known once their sub-folders are), then ledger/queue's sub-folders.
    let top: [(&str, bool, &AtomicU32); 4] =
        [(jobs_core::STATUS_DIR, false, &CLAIMS), (jobs_core::LEDGER_DIR, true, &LEDGER), (jobs_core::QUEUE_DIR, true, &QUEUE), (jobs_core::QUERIES_DIR, false, &QUERIES)];
    let mut tally = [0u32; 4];
    let mut said: Vec<(u32, u64)> = Vec::new();
    let mut rest: Vec<(usize, String)> = Vec::new();
    let mut max_ms = 0u64;
    for (i, (name, nested, _)) in top.iter().enumerate() {
        let (f, subs, ms) = chunk(&mt, &sub(name), *nested);
        tally[i] += f;
        max_ms = max_ms.max(ms);
        said.push((f, ms));
        rest.extend(subs.into_iter().map(|p| (i, p)));
        breathe();
    }
    let m = said.len() + rest.len();
    for (n, (f, ms)) in said.iter().enumerate() {
        serial_println!("[jobs] scan chunk={}/{} records={} ms={}", n + 1, m, f, ms);
    }
    for (k, (i, path)) in rest.iter().enumerate() {
        let (f, _, ms) = chunk(&mt, path, false);
        tally[*i] += f;
        max_ms = max_ms.max(ms);
        serial_println!("[jobs] scan chunk={}/{} records={} ms={}", said.len() + k + 1, m, f, ms);
        breathe();
    }
    for (i, (_, _, a)) in top.iter().enumerate() {
        a.store(tally[i], Relaxed);
    }
    CHUNKS.store(m as u32, Relaxed);
    MAX_CHUNK_MS.store(max_ms, Relaxed);
    STATE.store(2, Release);
    serial_println!(
        "[jobs] volume={} records={} claims={} ledger={} queue={} queries={} on={} chunks={} max_chunk_ms={} waited_ms={} ms={}",
        jobs_core::ROOT,
        tally[0] + tally[1] + tally[2],
        tally[0],
        tally[1],
        tally[2],
        tally[3],
        if ON_WORKER.load(Relaxed) { "worker" } else { "inline" },
        m,
        max_ms,
        waited_ms,
        crate::arch::ms().saturating_sub(t0)
    );
}

/// Register `tests jobscan` once (from [`owe`]).
fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, AcqRel) {
        crate::tests::register("jobscan", selftest);
    }
}

/// `tests jobscan` (R80: only when asked) — reads what the login's scan recorded; runs nothing.
fn selftest() {
    let landed = STATE.load(Acquire) == 2;
    let on = if ON_WORKER.load(Relaxed) { "worker" } else { "inline" };
    let (chunks, max_ms) = (CHUNKS.load(Relaxed), MAX_CHUNK_MS.load(Relaxed));
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    let bar_ms = crate::video::desktopbuild::last_bar_ms();
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    let bar_ms: Option<u64> = None;
    let bar_ok = bar_ms.map_or(true, |b| b <= 200);
    let pass = landed && on == "worker" && max_ms <= 50 && bar_ok;
    let bar = bar_ms.map_or(String::from("none"), |b| format!("{}", b));
    serial_println!(
        ":: JOBSCAN: on={} chunks={} max_chunk_ms={} bar_ms={} -> {} :: state={}",
        on, chunks, max_ms, bar, if pass { "PASS" } else { "FAIL" }, state()
    );
}
