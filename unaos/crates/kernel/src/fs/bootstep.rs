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

//! CHARTER: Kernel — fs-core
//!
//! BOOT80 (rmbp-ledger B350) — the boot's own store-step lines. Boot 21 waited ~60 s between
//! `:: PREFS:` and `FIRSTBOOT` and printed nothing (R80 had taken the `[users] load` / `[assoc] seed`
//! witnesses, which were bootlog-only). Under R80 a line that says what the BOOT did is the boot's own
//! line, so each step the stage waits on prints ONE line, unconditionally:
//!
//! `[boot] step=<name> ms=<n> blocks_read=<n> blocks_written=<n> cmds=<n>`
//!
//! `blocks_*` are 4 KiB units (UnaFS's block) of what the step moved on the medium, `cmds` the block
//! commands it issued. Counted at the SDHC block entry points (`drivers/block.rs`, the card on the
//! rMBP); a build without `sdhcblk` counts nothing and says zeros. While a step runs its name is
//! painted on the held splash (`splash::step_label`), so a long step is never a silent glass.

use core::sync::atomic::{AtomicU64, Ordering};

static RD_CMDS: AtomicU64 = AtomicU64::new(0);
static RD_SECTORS: AtomicU64 = AtomicU64::new(0);
static WR_CMDS: AtomicU64 = AtomicU64::new(0);
static WR_SECTORS: AtomicU64 = AtomicU64::new(0);

/// One read command of `sectors` 512 B sectors reached the medium.
#[inline]
pub fn note_read(sectors: u64) {
    RD_CMDS.fetch_add(1, Ordering::Relaxed);
    RD_SECTORS.fetch_add(sectors, Ordering::Relaxed);
}

/// One write command of `sectors` 512 B sectors reached the medium.
#[inline]
pub fn note_write(sectors: u64) {
    WR_CMDS.fetch_add(1, Ordering::Relaxed);
    WR_SECTORS.fetch_add(sectors, Ordering::Relaxed);
}

/// A snapshot of the medium counters.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Io {
    pub rd_cmds: u64,
    pub rd_sectors: u64,
    pub wr_cmds: u64,
    pub wr_sectors: u64,
}

impl Io {
    /// The counters' growth from `earlier` to `self`.
    pub fn since(self, earlier: Io) -> Io {
        Io {
            rd_cmds: self.rd_cmds.wrapping_sub(earlier.rd_cmds),
            rd_sectors: self.rd_sectors.wrapping_sub(earlier.rd_sectors),
            wr_cmds: self.wr_cmds.wrapping_sub(earlier.wr_cmds),
            wr_sectors: self.wr_sectors.wrapping_sub(earlier.wr_sectors),
        }
    }
    /// 4 KiB blocks read (sectors rounded up to whole UnaFS blocks).
    pub fn blocks_read(self) -> u64 {
        self.rd_sectors.div_ceil(8)
    }
    /// 4 KiB blocks written.
    pub fn blocks_written(self) -> u64 {
        self.wr_sectors.div_ceil(8)
    }
    /// Block commands issued, both directions.
    pub fn cmds(self) -> u64 {
        self.rd_cmds + self.wr_cmds
    }
}

/// The counters now.
pub fn io() -> Io {
    Io {
        rd_cmds: RD_CMDS.load(Ordering::Relaxed),
        rd_sectors: RD_SECTORS.load(Ordering::Relaxed),
        wr_cmds: WR_CMDS.load(Ordering::Relaxed),
        wr_sectors: WR_SECTORS.load(Ordering::Relaxed),
    }
}

/// A step in flight: its name, its start and the counters at its start.
pub struct Step {
    name: &'static str,
    t0: u64,
    io0: Io,
}

/// Start step `name`; `label` is what the held splash says meanwhile ("Loading users").
pub fn begin(name: &'static str, label: &'static str) -> Step {
    // Painted once per distinct label: a step retried across passes (a `Busy` store mount) does not
    // composite the glass on every pass.
    static LAST: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("");
    let fresh = { let mut l = LAST.lock(); if *l != label { *l = label; true } else { false } };
    if fresh {
        crate::splash::step_label(label);
    }
    Step { name, t0: crate::arch::ms(), io0: io() }
}

impl Step {
    /// The elapsed ms and the medium traffic so far.
    pub fn delta(&self) -> (u64, Io) {
        (crate::arch::ms().saturating_sub(self.t0), io().since(self.io0))
    }

    /// End the step: its one line. `extra` is appended verbatim (empty for none).
    pub fn end(self, extra: &str) -> (u64, Io) {
        let (ms, d) = self.delta();
        note_span(self.name, self.t0, self.t0 + ms); // SMALLFIX6 (B495): the span, for the lag boot line's `overlaps=`
        serial_println!(
            "[boot] step={} ms={} blocks_read={} blocks_written={} cmds={}{}{}",
            self.name, ms, d.blocks_read(), d.blocks_written(), d.cmds(),
            if extra.is_empty() { "" } else { " " }, extra
        );
        (ms, d)
    }
}

/// BOOT80 M3 — `tests boot80`: the falsifier. Drops the one UnaFS mount and re-mounts it COLD from the
/// card (the refcount map, the inode map and the roots read again through the read-ahead window), then
/// resolves what the stage waits on — the users store (`USERS.DAT`, re-read from its volume), the stage,
/// and the type database (`/system/filetypes`, through the directory index) — and prints ONE line:
///
/// `:: BOOT80: mount_ms=<n> mount_blocks=<n> mount_cmds=<n> blocks_read=<n> cmds=<n> ms=<n> users_store=<dat|none|nomount> stage=<s> types=<n> ra_window=<n> -> PASS|FAIL ::`
///
/// Bound (B350, re-derived by SMALLFIX2 B391): the users-store leg `store_blocks <= 64`, the type-database leg
/// `types_blocks <= 2 x levels x ra-window blocks` (measured under the mount lock), and `ms <= 2000` for mount + resolve;
/// `foreign_wr` is the window's writes (the resolve writes nothing: non-zero names a concurrent writer). The cold
/// mount's own blocks are said separately: by format it reads the whole refcount map (128 leaves on the
/// card's 512 MiB volume), which BOOT80 makes cheap per command but does not shrink.
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
pub fn boot80_selftest() {
    let t0 = crate::arch::ms();
    let io0 = io();
    crate::fs::unafs::force_remount();
    let mounted = crate::fs::unafs::with_unafs(|fs| fs.root_generation()).is_ok();
    let mount_ms = crate::arch::ms().saturating_sub(t0);
    let mio = io().since(io0);
    let t1 = crate::arch::ms();
    let io1 = io();
    // SMALLFIX2 (rmbp-ledger B391): the users store (FAT, p1) is its own leg, measured on the global counters.
    #[cfg(feature = "login")]
    let store = crate::fs::users::boot80_store_probe();
    #[cfg(not(feature = "login"))]
    let store = "none";
    let sio = io().since(io1);
    #[cfg(feature = "login")]
    let stage = crate::fs::users::stage_name();
    #[cfg(not(feature = "login"))]
    let stage = "no-login";
    // The type database is measured INSIDE the one UnaFS mount lock: no other task's UnaFS traffic can land in this
    // window (flight 24's 168 blocks rode a screenshot capture writing beside it — the counters are global). Its
    // bound is the walk it does, not the number of types: `ls` reads ONE directory whatever its count (flight 25:
    // 26 types in 34 blocks) — an inode read and a directory read per level, each at most one read-ahead window.
    // SMALLFIX5 (B480) item 6: the leg is ASSOCSTAMP's own read — the directory resolved and its ONE stamp attribute
    // (what a login build reads when nothing changed), no longer an `ls` of every type. `types` is the stamp's count.
    let (have, tio) = crate::fs::unafs::with_unafs(|fs| {
        let i0 = io();
        let v = fs.resolve_path(crate::fs::assoc::TYPES_DIR).ok().and_then(|id| fs.get_attribute(id, crate::fs::assoc::STAMP_KEY).ok().flatten());
        let s = match v { Some(::unafs::inode::AttributeValue::String(s)) => Some(s), _ => None };
        (s, io().since(i0))
    })
    .unwrap_or((None, Io::default()));
    let want = crate::fs::assoc::stamp_now();
    let stamp_state = match &have { Some(h) if *h == want => "match", Some(_) => "miss", None => "none" };
    let types = have.as_deref().and_then(|h| h.rsplit("n=").next()).and_then(|n| n.trim().parse::<usize>().ok()).unwrap_or(0);
    let rms = crate::arch::ms().saturating_sub(t1);
    let rio = io().since(io1);
    let total = mount_ms + rms;
    let levels = 1 + crate::fs::assoc::TYPES_DIR.split('/').filter(|c| !c.is_empty()).count() as u64;
    let ra_blocks = crate::fs::unafs::ra_window_bound().div_ceil(8).max(1);
    let bound = 2 * levels * ra_blocks;
    const STORE_BOUND: u64 = 64; // B350's resolve bound, kept for the store leg it was measured on
    let ok = mounted && tio.blocks_read() <= bound && sio.blocks_read() <= STORE_BOUND && total <= 2000;
    serial_println!(
        ":: BOOT80: mount_ms={} mount_blocks={} mount_cmds={} blocks_read={} cmds={} ms={} users_store={} stage={} types={} ra_window={} store_blocks={} store_bound={} types_blocks={} bound={} from=walk{}x2xra{} foreign_wr={} types_from=stamp stamp={} -> {} ::",
        mount_ms, mio.blocks_read(), mio.cmds(), rio.blocks_read(), rio.cmds(), total, store, stage, types,
        crate::fs::unafs::ra_window_bound(), sio.blocks_read(), STORE_BOUND, tio.blocks_read(), bound, levels, ra_blocks,
        rio.blocks_written(), stamp_state, if ok { "PASS" } else { "FAIL" }
    );
}

// ── SMALLFIX6 (rmbp-ledger B495) — the boot's step spans ─────────────────────────────────────────────────
// Flight 26's `[lag] stall boot_suppressed=… worst_stage=render-handler worst_ms=6094` had no name: the render
// task made no route, pass or park for ~6.1 s and the wire could not say during what. Every ended step (and the
// `login ok` type-registry build) keeps its `[t0, t1]` here, the first [`SPAN_CAP`] of the boot, so `video::lag`
// can name the steps the worst handler interval overlapped. No I/O; a busy lock drops the span (never waits).

/// How many spans the boot keeps.
pub const SPAN_CAP: usize = 16;
static SPANS: crate::sync::Mutex<[(&'static str, u64, u64); SPAN_CAP]> = crate::sync::Mutex::new([("", 0, 0); SPAN_CAP]);

/// Keep `name`'s span `[t0_ms, t1_ms]` (kernel ms). Never waits.
pub fn note_span(name: &'static str, t0_ms: u64, t1_ms: u64) {
    if let Some(mut g) = SPANS.try_lock() {
        if let Some(slot) = g.iter_mut().find(|s| s.0.is_empty()) {
            *slot = (name, t0_ms, t1_ms);
        }
    }
}

/// The kept spans that intersect `[a_ms, b_ms]`, as `name:ms` comma-joined, or `none`. `-` when the lock is busy.
pub fn overlaps(a_ms: u64, b_ms: u64) -> alloc::string::String {
    let Some(g) = SPANS.try_lock() else { return alloc::string::String::from("-") };
    let mut out = alloc::string::String::new();
    for s in g.iter().filter(|s| !s.0.is_empty() && s.1 <= b_ms && s.2 >= a_ms) {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(&alloc::format!("{}:{}", s.0, s.2.saturating_sub(s.1)));
    }
    if out.is_empty() {
        out.push_str("none");
    }
    out
}

/// Every kept span, as [`overlaps`] prints them (`tests smallfix6`).
pub fn spans() -> alloc::string::String {
    overlaps(0, u64::MAX)
}
