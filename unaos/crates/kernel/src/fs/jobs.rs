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
//! [`announce`] runs at `login ok` (R93: what the session has is said when it opens) and prints ONE line — a count
//! of directory entries, nothing tested (R80):
//!   `[jobs] volume=/jobs records=<n> claims=<n> ledger=<n> queue=<n> queries=<n>` (or `volume=absent records=0`).

use crate::fs::vfs::{MountTable, NodeKind};
use alloc::format;
use alloc::string::String;

/// Files directly in `dir`, and (with `nested`) in its sub-folders one level down.
fn count(mt: &MountTable, dir: &str, nested: bool) -> usize {
    let Ok(list) = mt.read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in list {
        match e.kind {
            NodeKind::File => n += 1,
            NodeKind::Dir if nested => n += count(mt, &format!("{}/{}", dir, e.name), false),
            NodeKind::Dir => {}
        }
    }
    n
}

fn sub(name: &str) -> String {
    format!("{}/{}", jobs_core::ROOT, name)
}

/// The one login line: what the volume's `/jobs` holds.
pub fn announce() {
    let mt = crate::shell::vfs_mount_table();
    match mt.stat(jobs_core::ROOT) {
        Ok(s) if matches!(s.kind, NodeKind::Dir) => {}
        _ => {
            serial_println!("[jobs] volume=absent records=0");
            return;
        }
    }
    let claims = count(&mt, &sub(jobs_core::STATUS_DIR), false);
    let ledger = count(&mt, &sub(jobs_core::LEDGER_DIR), true);
    let queue = count(&mt, &sub(jobs_core::QUEUE_DIR), true);
    let queries = count(&mt, &sub(jobs_core::QUERIES_DIR), false);
    serial_println!(
        "[jobs] volume={} records={} claims={} ledger={} queue={} queries={}",
        jobs_core::ROOT,
        claims + ledger + queue,
        claims,
        ledger,
        queue,
        queries
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
