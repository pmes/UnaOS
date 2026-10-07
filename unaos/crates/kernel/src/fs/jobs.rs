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
        crate::tests::register("jobscan", selftest); crate::tests::register("jobsui", ui_selftest); // JOBSUI (B511): `tests jobsui`
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

// ── JOBSNEXT (rmbp-ledger B506): `jobs next [n] [track]` — the next wave on the glass ─────────────────────────────
// Peter, 2026-10-07: "why do i have to tell you that there's more to do after every single boot". The shell's
// `jobs next` reads `/jobs/owed` (the flight's §3, written by Mica) and the track's records through the VFS and
// prints `jobs_core::rank_next` — the SAME ranking `mica jobs next` prints on the host (R79: one ranker). A verb
// Peter types, never a boot step (R80).
//
// JOBSCAN (B497) measured what a whole-directory walk costs on this volume (1675 inode reads, 17 s, IRQ-masked), so
// the kernel never walks: it asks the volume's attribute index for exactly the records `rank_next` reads — every
// OPEN record (`job:status == "open"`: the queue candidates, the open ledger rows, the header's open count) and
// every record carrying a §3 name (`job:arc == "<NAME>"`: the running check and the cited row of a name with no
// open record) — kept to the track's queue and ledger folders. On that subset `rank_next` returns what it returns
// on the whole store. Each record is then its own short read (attributes, body), never one long transaction.
// Wire: `[jobs] next flight=f<n> track=rmbp owed=<k> open=<m> ranked=<n> gpu=kepler:<id>,intel:<id> read=<r> via=index ms=<t>`
// then one tab-separated row per rank (`<rank> <id> <name> <status> <ledger row> -`).

use crate::fs::vfs::AttrValue;
use alloc::string::ToString;
use alloc::vec::Vec;

/// The track the glass ranks when none is typed (the rMBP's own queue).
pub const NEXT_TRACK: &str = "rmbp";

/// `arch::ticks()` is milliseconds on x86 (the calibrated 1 kHz APIC); on aarch64 it is the ~250 Hz heartbeat.
#[cfg(target_arch = "x86_64")]
const TICK_WORD: &str = "ms";
#[cfg(not(target_arch = "x86_64"))]
const TICK_WORD: &str = "ticks";

/// One record off the volume by path (its `job:*` attributes and its body).
fn load_one(mt: &MountTable, path: &str) -> Option<jobs_core::Record> {
    let kv = mt.list_attrs(path, crate::fs::vfs::KERNEL_PRINCIPAL).ok()?;
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut seq = 0i64;
    for (k, v) in kv {
        match v {
            AttrValue::Int(i) if k == jobs_core::K_SEQ => seq = i,
            AttrValue::Str(s) if k.starts_with(jobs_core::job_ns()) => attrs.push((k, s)),
            _ => {}
        }
    }
    let size = mt.stat(path).map(|s| s.size as usize).unwrap_or(0);
    let body = mt.read(path, 0, size).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
    jobs_core::record_from_volume(&attrs, seq, body)
}

/// The fallback when the volume answers no queries: the track's two folders, walked.
fn walk(mt: &MountTable, dir: &str, paths: &mut Vec<String>) {
    let Ok(list) = mt.read_dir(dir) else { return };
    for e in list {
        if matches!(e.kind, NodeKind::File) {
            paths.push(format!("{}/{}", dir, e.name));
        }
    }
}

/// `/jobs/owed` back: (`f<n>`, names); empty when the card's volume predates it.
fn owed(mt: &MountTable) -> (String, Vec<String>) {
    let path = sub(jobs_core::OWED_FILE);
    let flight = match mt.get_attr(&path, jobs_core::K_FLIGHT, crate::fs::vfs::KERNEL_PRINCIPAL) {
        Ok(AttrValue::Str(s)) => s,
        _ => String::new(),
    };
    let size = mt.stat(&path).map(|s| s.size as usize).unwrap_or(0);
    let body = mt.read(&path, 0, size).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
    (flight, body.lines().filter(|l| !l.is_empty()).map(|l| l.to_string()).collect())
}

/// The ranked lines (header first), or the one absent line. Both the console and the serial wire get them.
pub fn next_lines(n: usize, track: &str) -> Vec<String> {
    let t0 = crate::arch::ticks();
    let mt = crate::shell::vfs_mount_table();
    if !matches!(mt.stat(jobs_core::ROOT), Ok(s) if matches!(s.kind, NodeKind::Dir)) {
        return alloc::vec![String::from("[jobs] next volume=absent ranked=0")];
    }
    let (flight, names) = owed(&mt);
    let q_dir = format!("{}/{}", sub(jobs_core::QUEUE_DIR), track);
    let l_dir = format!("{}/{}", sub(jobs_core::LEDGER_DIR), track);
    let mine = |p: &str| {
        p.rsplit_once('/').is_some_and(|(d, f)| !f.is_empty() && (d == q_dir.as_str() || d == l_dir.as_str()))
    };
    let mut paths: Vec<String> = Vec::new();
    let mut via = "index";
    let mut exprs = alloc::vec![format!("{} == \"open\"", jobs_core::K_STATUS)];
    for nm in &names {
        if !nm.contains('"') {
            exprs.push(format!("{} == \"{}\"", jobs_core::K_ARC, nm));
        }
    }
    for e in &exprs {
        match mt.query(e, crate::fs::vfs::KERNEL_PRINCIPAL) {
            Ok(hits) => {
                for (_, p) in hits {
                    if mine(&p) && !paths.contains(&p) {
                        paths.push(p);
                    }
                }
            }
            Err(_) => {
                via = "walk";
                break;
            }
        }
    }
    if via == "walk" {
        paths.clear();
        walk(&mt, &q_dir, &mut paths);
        walk(&mt, &l_dir, &mut paths);
    }
    let recs: Vec<jobs_core::Record> = paths.iter().filter_map(|p| load_one(&mt, p)).collect();
    let ranked = jobs_core::rank_next(&recs, &names, track, n);
    let head = jobs_core::next_header(&recs, &ranked, &flight, names.len(), track);
    let mut out = alloc::vec![format!("{} read={} via={} {}={}", head, recs.len(), via, TICK_WORD, crate::arch::ticks().saturating_sub(t0))];
    for (k, t) in ranked.iter().enumerate() {
        out.push(jobs_core::next_row(&recs, k + 1, t, ""));
    }
    out
}

// ── JOBSUI (rmbp-ledger B511) — the jobs on the glass ─────────────────────────────────────────────────────────────────
// Peter (2026-10-07): "what happened to the new jobs queue?" — the counts and the queue's open items, VISIBLE, not only
// on the wire. Mica (`handlers/mica`, CODEX §2's Ledger) owns the view's shape — `jobs_core`'s `counts_line` /
// `item_row` / `shown_open` / `last_flight`, which `mica jobs view` renders on the host; the kernel FULFILS the reads
// (R79): the four counts from the landed scan ([`counts`]), the items straight off `/jobs/queue/<track>` through the
// VFS. Surfaces: the `jobs` shell verb ([`shell_verb`]) and Quarry's status line under `/jobs` ([`status_text`]).
// While the scan is pending every surface says `jobs scanning…` (`scanning...` on the glass), never zeros.
// Wire: `[jobsui] verb=jobs counts=<landed|pending> items_shown=<n> read=<files> ms=<ms>`;
//       `[jobsui] quarry status=<pending|landed> text=<the line drawn>` (once per change);
//       `tests jobsui` → `:: JOBSUI: counts=ok pending_said=1 items_shown=<n> -> PASS :: …` (R80: only when asked).

/// Times a surface drew the pending line (the live half of `pending_said`).
static UI_PENDING_DRAWN: AtomicU32 = AtomicU32::new(0);
/// The last `jobs` verb's item count.
static UI_ITEMS_SHOWN: AtomicU32 = AtomicU32::new(0);
/// Files the item read examines at most (a queue whose first ranks are all closed cannot turn one verb into a scan).
const UI_READ_CAP: u32 = 160;
/// Bytes of an item's line read for its done mark and the flight it names.
const UI_BODY_BYTES: usize = 4096;

/// One open queue item, as the view shows it.
pub struct ViewItem {
    pub name: String,
    pub track: &'static str,
    pub status: String,
    pub flight: String,
}

/// The four counts in `jobs_core`'s view order (records, claims, ledger, queue); `None` while the scan is pending.
pub fn view_counts() -> Option<[u64; 4]> {
    counts().map(|c| [c.records as u64, c.claims as u64, c.ledger as u64, c.queue as u64])
}

/// The counts line a glass surface draws (`jobs records=… claims=… ledger=… queue=…`, or `jobs scanning...`).
pub fn status_text() -> String {
    let c = view_counts();
    if c.is_none() {
        UI_PENDING_DRAWN.fetch_add(1, Relaxed);
    }
    jobs_core::glass(&jobs_core::counts_line(c))
}

/// The queue's open items by rank (track by track in `jobs_core::QUEUES` order, each in its `job:seq` order), at most
/// `n`, optionally one `track` only. Reads `/jobs` through the VFS: one listing per track, then per item one attribute
/// read (its `job:status`) and, for an open one, its line's head. `(items, files examined)`.
pub fn open_items(n: usize, track: Option<&str>) -> (Vec<ViewItem>, u32) {
    let mt = crate::shell::vfs_mount_table();
    let mut out: Vec<ViewItem> = Vec::new();
    let mut read = 0u32;
    for (t, _) in jobs_core::QUEUES.iter() {
        if out.len() >= n || read >= UI_READ_CAP {
            break;
        }
        if track.is_some_and(|w| w != *t) {
            continue;
        }
        let dir = format!("{}/{}/{}", jobs_core::ROOT, jobs_core::QUEUE_DIR, t);
        let Ok(list) = mt.read_dir_kinds(&dir) else { continue };
        let mut names: Vec<String> = list.into_iter().filter(|(_, k)| matches!(k, NodeKind::File)).map(|(nm, _)| nm).collect();
        jobs_core::rank_order(&mut names);
        for nm in names {
            if out.len() >= n || read >= UI_READ_CAP {
                break;
            }
            read += 1;
            let path = format!("{}/{}", dir, nm);
            let Ok(attrs) = mt.list_attrs(&path, crate::fs::vfs::KERNEL_PRINCIPAL) else { continue };
            let attr = |k: &str| -> String {
                attrs.iter().find(|(a, _)| a == k).and_then(|(_, v)| match v {
                    crate::fs::vfs::AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }).unwrap_or_default()
            };
            let status = attr(jobs_core::K_STATUS);
            if status != "open" {
                continue;
            }
            let head = mt.read(&path, 0, UI_BODY_BYTES).unwrap_or_default();
            let body = match core::str::from_utf8(&head) {
                Ok(s) => s,
                Err(e) => core::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or(""),
            };
            if !jobs_core::shown_open(&status, body) {
                continue;
            }
            let flight = jobs_core::last_flight(&attr(jobs_core::K_FLIGHT), body);
            out.push(ViewItem { name: jobs_core::queue_name(&nm).1, track: *t, status, flight });
        }
    }
    (out, read)
}

/// `jobs [<track>|bg|next]` — the read: the counts, then the queue's top ten open items. Returns `true` when the
/// caller lists its background programs after (bare `jobs` keeps BGRUN-1's reaper; `jobs bg` is that list alone).
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) -> bool {
    let track = match args.first().copied() {
        None => None,
        Some("bg") => return true,
        Some("next") => {
            console.println("jobs next: the ranked next wave is JOBSNEXT's (rmbp-ledger B506), not in this build — `jobs` is the read");
            return false;
        }
        Some(w) if jobs_core::QUEUES.iter().any(|(t, _)| *t == w) => Some(w),
        Some(_) => {
            console.println("usage: jobs [rmbp|orin|pi|trunk|bg|next]   (the jobs on /jobs, then background programs)");
            return false;
        }
    };
    let t0 = crate::arch::ms();
    if state() == "idle" {
        owe(); // asked (R80): a session that never logged in scans now, the same chunked scan
    }
    let landed = counts().is_some();
    console.println(&status_text());
    let (items, read) = open_items(jobs_core::VIEW_ITEMS, track);
    if items.is_empty() {
        console.println("  no open queue items on /jobs");
    } else {
        console.println("  open queue items (rank = the queue's order):");
        for (i, it) in items.iter().enumerate() {
            console.println(&jobs_core::glass(&jobs_core::item_row(i + 1, &it.name, it.track, &it.status, &it.flight)));
        }
    }
    UI_ITEMS_SHOWN.store(items.len() as u32, Relaxed);
    serial_println!(
        "[jobsui] verb=jobs counts={} items_shown={} read={} ms={}",
        if landed { "landed" } else { "pending" },
        items.len(),
        read,
        crate::arch::ms().saturating_sub(t0)
    );
    if track.is_none() {
        console.println("background programs:");
    }
    track.is_none()
}

/// `tests jobsui` (R80: only when asked) — the counts as drawn, the pending line's words, the items the view shows.
fn ui_selftest() {
    let c = view_counts();
    let counts = match c {
        Some(v) => {
            let l = jobs_core::counts_line(Some(v));
            if v[0] == v[1] + v[2] + v[3] && l.contains(&format!("queue={}", v[3])) && !jobs_core::says_pending(&l) { "ok" } else { "FAIL" }
        }
        None => "pending",
    };
    // The pending line both surfaces draw (wire and glass spellings): the word, never a number.
    let pend = jobs_core::counts_line(None);
    let pending_said = jobs_core::says_pending(&pend) && jobs_core::says_pending(&jobs_core::glass(&pend));
    let (items, read) = open_items(jobs_core::VIEW_ITEMS, None);
    let queue = c.map_or(0, |v| v[3]);
    let pass = counts == "ok" && pending_said && (!items.is_empty() || queue == 0);
    serial_println!(
        ":: JOBSUI: counts={} pending_said={} items_shown={} -> {} :: state={} live_pending={} verb_items={} read={} first={}",
        counts,
        pending_said as u8,
        items.len(),
        if pass { "PASS" } else { "FAIL" },
        state(),
        UI_PENDING_DRAWN.load(Relaxed),
        UI_ITEMS_SHOWN.load(Relaxed),
        read,
        items.first().map_or("-", |i| i.name.as_str())
    );
}
