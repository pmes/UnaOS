// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (R90)
//!
//! WINDOWCAP (rmbp-ledger B378, R90) — THE ONE DYNAMIC LIMIT ON RUNNING APPS.
//!
//! Peter, flight 23: "no hardcoding!!! there should be a dynamic limit to the number of apps that can
//! execute so that in case of emergency whatever is causing a bazzilion apps to spawn or whatever doesn't
//! take down the machine". The wire: `video::wm` held `MAX_WINDOWS = 12` fixed rows, the table filled
//! after `storm` + console + shell + STAT + lumen + quarry + settings, and every later open was refused
//! SILENTLY (`[facet] refuse … reason=no-window(create-failed)`, `[fileview] refuse … window create
//! failed`, `[login] screen open window=no`).
//!
//! This module is the limit and nothing else: no table, no store. `wm::create_inner` and the ring-3
//! `sys_spawn` ASK it; the tables stay theirs.
//!
//! ## The limit is derived, never written down — and memory is its only term (WINDOWCAP-2, WINDOWCAP3)
//! `windows = mem`, `procs = min(mem, windows)`:
//!   * `mem`  — half the kernel heap free at arming ÷ the per-row kernel cost ([`WIN_COST`], [`PROC_COST`]).
//!     A program's cost now carries its whole address-space record ([`SLOT_COST`]: page tables + backing),
//!     because WINDOWCAP3 (B399) moved that record from a `.bss` pool to the heap.
//!   * there is no `asids` term any more: the address-space pool (`USER_SLOTS` = 12 x86 / 8 aarch64), the
//!     process table and every per-slot sidecar are heap-grown (`procslot::SlotVec`); the only other bound
//!     is the slot TYPE (`procslot::SLOT_ID_MAX`, the futex key's tag byte), the `WinId` rule.
//!
//! There is no id-space term (the compositor table is a heap `Vec` with segment-doubling side storage,
//! `video::rowstore`; the only bound is the `WinId` type, `wm::WIN_ID_SENTINEL_FLOOR`) and no dock term
//! (a panel's width bounds what the dock SHOWS — it overflows the rest into a `+<k>` group — never what
//! may be OPEN). Only APP rows count (`wm`'s `dock_addressable`: used, not compat, owner ≠ 0); a system
//! row is never refused by the limit, which is what lets the notice open when the limit is hit.
//!
//! ## Wire
//! * `[wm] limit windows=<n> procs=<n> from=mem:<MiB> (R90)` — once, at arming (WINDOWCAP3: `asids` gone).
//! * `[wm] REFUSED create reason=limit|heap n=<n> (R90)` / `[wm] REFUSED spawn reason=limit n=<n> (R90)`.
//! * `:: WINDOWCAP: fixed_cap=none limit=<n> procs=<n> opens_refused=<k> -> PASS|FAIL ::` — at the desktop
//!   ignition (`boot::ignite`), an arming line (R80), not a test.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

/// The dock's pinned tiles (`dock::pins_applied`: shell, console, Quarry, pulse) — the live dock row
/// count is the app rows PLUS these.
pub const DOCK_PINS: usize = 4;

/// The largest window surface the ABI hands out (CRYSTAL-HD: 288x288 ARGB8888, both arches).
const SURFACE_MAX: usize = 288 * 288 * 4;
#[cfg(target_arch = "x86_64")]
const _: () = assert!(SURFACE_MAX == crate::arch::x86_64::memory::FB_WIN_SLOT_SIZE);

/// Per-window kernel heap cost: the pacer's shadow and the pass's mirror of the largest surface
/// (`wm::PACE_SHADOW` / `PACE_MIRROR`), plus a page for the row and its `rowstore` side state.
pub const WIN_COST: usize = 2 * SURFACE_MAX + 4096;

/// WINDOWCAP3 (B399): one address-space record — its page tables, args page and 1.3 MiB backing — which
/// the slot pool now takes from the heap on a slot's first claim (0 where this build has no ring 3).
#[cfg(target_arch = "x86_64")]
pub const SLOT_COST: usize = crate::arch::x86_64::memory::SLOT_RECORD_BYTES;
#[cfg(all(target_arch = "aarch64", any(feature = "baremetal", feature = "tegra_el0")))]
pub const SLOT_COST: usize = crate::arch::aarch64::uslots::SLOT_RECORD_BYTES;
#[cfg(not(any(target_arch = "x86_64", all(target_arch = "aarch64", any(feature = "baremetal", feature = "tegra_el0")))))]
pub const SLOT_COST: usize = 0;

/// Per-process kernel heap cost: a program owns a window and an address-space record, plus its kernel
/// task stacks and handle state.
pub const PROC_COST: usize = WIN_COST + SLOT_COST + 64 * 1024;

static ARMED: AtomicBool = AtomicBool::new(false);
static MEM_MIB: AtomicUsize = AtomicUsize::new(0);
static MEM_WIN: AtomicUsize = AtomicUsize::new(usize::MAX);
static MEM_PROC: AtomicUsize = AtomicUsize::new(usize::MAX);
static OPENS_REFUSED: AtomicU64 = AtomicU64::new(0);
static SPAWNS_REFUSED: AtomicU64 = AtomicU64::new(0);
/// The notice is posted once per burst: set when posted, cleared when an app row is next admitted.
static NOTICE_UP: AtomicBool = AtomicBool::new(false);
/// The ring-3 spawner's pause, doubled per refusal (50 ms .. 1.6 s), reset by an admitted spawn.
static SPAWN_BACKOFF_MS: AtomicU64 = AtomicU64::new(0);

/// Whether this build runs ring-3 programs at all (no process table, no process limit).
#[inline]
pub fn has_procs() -> bool {
    cfg!(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))
}

/// Arm the memory term once (idempotent) and print the limit line. Takes the heap lock (through
/// `allocator::heap_census`), so never call it from inside an allocation.
pub fn arm() {
    if ARMED.swap(true, Relaxed) {
        return;
    }
    let free = crate::allocator::heap_census(4096).free;
    let budget = free / 2;
    MEM_MIB.store(free >> 20, Relaxed);
    MEM_WIN.store((budget / WIN_COST).max(1), Relaxed);
    MEM_PROC.store((budget / PROC_COST).max(1), Relaxed);
    serial_println!(
        "[wm] limit windows={} procs={} from=mem:{} (R90)",
        win_limit(),
        proc_limit(),
        free >> 20
    );
}

fn fmt_usize(mut v: usize, buf: &mut [u8; 8]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 || i == 0 {
            break;
        }
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("?")
}

/// The live window limit: app rows `wm` admits — memory, and the `WinId` type. Arms on first use.
pub fn win_limit() -> usize {
    if !ARMED.load(Relaxed) {
        arm();
    }
    MEM_WIN.load(Relaxed).min(super::wm::WIN_ID_SENTINEL_FLOOR as usize - 1).max(1)
}

/// The live process limit: ring-3 programs `sys_spawn` admits (never more than the window limit — every
/// program must be able to own a window).
pub fn proc_limit() -> usize {
    if !has_procs() {
        return 0;
    }
    let win = win_limit(); // arms the memory term on first use
    MEM_PROC.load(Relaxed).min(win).min(crate::procslot::SLOT_ID_MAX).max(1) // WINDOWCAP3: memory, never a pool width
}

/// The LIVE dock row count — the app rows open now plus the pins — which every "can the dock host the
/// strip" check asks (`desktop_uefi`, `desktop_firmware`, `quarry::live`, `main`, `display_tegra`). Since
/// WINDOWCAP-2 the dock overflows into a `+<k>` group, so `for_panel` answers for ANY count a panel can
/// show one tile for; the panel arguments are kept for the callers' shape.
pub fn dock_rows(_pw: usize, _ph: usize) -> usize {
    super::wm::live_window_count() + DOCK_PINS
}

/// `wm::create_inner` refused an app row (`heap == true`: the table could not grow — the heap said no). Called AFTER
/// the table lock drops. One wire line per refusal (the first 16, then every 64th), one notice per burst.
pub fn note_open_refused(apps: usize, heap: bool) {
    let k = OPENS_REFUSED.fetch_add(1, Relaxed) + 1;
    if k <= 16 || k % 64 == 0 {
        serial_println!(
            "[wm] REFUSED create reason={} n={} refused={} (R90)",
            if heap { "heap" } else { "limit" },
            apps,
            k
        );
    }
    if !NOTICE_UP.swap(true, Relaxed) {
        post_notice(apps);
    }
}

/// An app row was admitted: the next refusal is a new burst and says so again.
#[inline]
pub fn note_open_admitted() {
    if NOTICE_UP.load(Relaxed) {
        NOTICE_UP.store(false, Relaxed);
    }
}

fn post_notice(n: usize) {
    #[cfg(feature = "login")]
    {
        let mut line = [0u8; 46];
        let mut len = 0usize;
        let mut push = |s: &[u8], len: &mut usize| {
            for &b in s {
                if *len < line.len() {
                    line[*len] = b;
                    *len += 1;
                }
            }
        };
        let mut nb = [0u8; 8];
        push(b"Too many windows open (", &mut len);
        push(fmt_usize(n, &mut nb).as_bytes(), &mut len);
        push(b") - close one", &mut len);
        crate::fs::users::screen_notice(b"Too many windows", &line[..len]);
    }
    #[cfg(not(feature = "login"))]
    let _ = n;
}

/// The ring-3 spawner hit the process limit: say it, then PAUSE THE SPAWNER (not the machine) — a
/// doubling sleep, 50 ms .. 1.6 s, reset by the next admitted spawn. Called from `sys_spawn` on the
/// spawning task, holding no lock.
pub fn note_spawn_refused(live: usize) {
    let k = SPAWNS_REFUSED.fetch_add(1, Relaxed) + 1;
    let prev = SPAWN_BACKOFF_MS.load(Relaxed);
    let ms = if prev == 0 { 50 } else { (prev * 2).min(1600) };
    SPAWN_BACKOFF_MS.store(ms, Relaxed);
    if k <= 16 || k % 64 == 0 {
        serial_println!(
            "[wm] REFUSED spawn reason=limit n={} refused={} paused_ms={} (R90)",
            live,
            k,
            ms
        );
    }
    #[cfg(target_arch = "x86_64")]
    crate::arch::x86_64::sched::sleep_ms(ms);
}

/// A spawn was admitted: the spawner's backoff resets.
#[inline]
pub fn note_spawn_admitted() {
    if SPAWN_BACKOFF_MS.load(Relaxed) != 0 {
        SPAWN_BACKOFF_MS.store(0, Relaxed);
    }
}

/// M4 — the witness, printed once at the desktop ignition. `fixed_cap=none`: the limit is memory's (no id
/// space, no dock width, no constant). PASS: the eleventh window fits (`limit >= 11`) and nothing has been
/// refused yet.
pub fn witness() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Relaxed) {
        return;
    }
    #[cfg(all(target_arch = "x86_64", feature = "witness"))]
    let _ = crate::tests::defer("spawnstorm", crate::arch::syscall::spawnstorm_selftest); // WINDOWCAP3 (B399): registers `tests spawnstorm` (R80: nothing runs at boot)
    let limit = win_limit();
    let refused = OPENS_REFUSED.load(Relaxed);
    let mem_bound = limit == MEM_WIN.load(Relaxed);
    serial_println!(
        ":: WINDOWCAP: fixed_cap={} limit={} procs={} from=mem:{} opens_refused={} -> {} ::",
        if mem_bound { "none" } else { "type:u32" },
        limit,
        proc_limit(),
        MEM_MIB.load(Relaxed),
        refused,
        if limit >= 11 && refused == 0 { "PASS" } else { "FAIL" }
    );
}
