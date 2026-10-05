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
    static LAST: spin::Mutex<&'static str> = spin::Mutex::new("");
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
/// and the type database (`/system/types`, through the directory index) — and prints ONE line:
///
/// `:: BOOT80: mount_ms=<n> mount_blocks=<n> mount_cmds=<n> blocks_read=<n> cmds=<n> ms=<n> users_store=<dat|none|nomount> stage=<s> types=<n> ra_window=<n> -> PASS|FAIL ::`
///
/// Bound (B350): `blocks_read <= 64` for the resolve and `ms <= 2000` for mount + resolve. The cold
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
    #[cfg(feature = "login")]
    let (store, stage) = (crate::fs::users::boot80_store_probe(), crate::fs::users::stage_name());
    #[cfg(not(feature = "login"))]
    let (store, stage) = ("none", "no-login");
    let types = crate::fs::unafs::with_unafs(|fs| {
        fs.resolve_path(crate::fs::assoc::TYPES_DIR).and_then(|id| fs.ls(id)).map(|v| v.len()).unwrap_or(0)
    })
    .unwrap_or(0);
    let rms = crate::arch::ms().saturating_sub(t1);
    let rio = io().since(io1);
    let total = mount_ms + rms;
    let ok = mounted && rio.blocks_read() <= 64 && total <= 2000;
    serial_println!(
        ":: BOOT80: mount_ms={} mount_blocks={} mount_cmds={} blocks_read={} cmds={} ms={} users_store={} stage={} types={} ra_window={} -> {} ::",
        mount_ms, mio.blocks_read(), mio.cmds(), rio.blocks_read(), rio.cmds(), total, store, stage, types,
        crate::fs::unafs::ra_window_bound(), if ok { "PASS" } else { "FAIL" }
    );
}
