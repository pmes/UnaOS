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
