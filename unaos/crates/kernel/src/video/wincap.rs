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
//! ## The limit is derived, never written down
//! `windows = min(mem, dock, ids)`, `procs = min(asids, mem, windows)`:
//!   * `mem`  — half the kernel heap free at arming ÷ the per-row kernel cost ([`WIN_COST`], [`PROC_COST`]).
//!   * `dock` — the most APP tiles the dock can host on THIS panel beside its pins ([`DOCK_PINS`]),
//!     asked of `dock::Layout::for_panel` live, so the dock checks hold for every table state the limit
//!     admits (they read [`dock_rows`]).
//!   * `ids`  — the compositor's id space (`wm::MAX_WINDOWS`) less [`SYS_ROWS`] kept for the system's own
//!     rows (owner 0: the login screen, the notice that SAYS the limit, the compat row).
//!   * `asids`— the ring-3 process table (`arch::syscall::proc_table_rows`, itself the address-space pool
//!     less its 2-slot reserve).
//!
//! Only APP rows count (`wm`'s `dock_addressable`: used, not compat, owner ≠ 0); a system row is never
//! refused by the limit, which is what lets the notice open when the limit is hit.
//!
//! ## Wire
//! * `[wm] limit windows=<n> procs=<n> from=mem:<MiB>,asids:<n>,dock:<n>,ids:<n> (R90)` — once, at arming.
//! * `[wm] REFUSED create reason=limit|ids n=<n> (R90)` / `[wm] REFUSED spawn reason=limit n=<n> (R90)`.
//! * `:: WINDOWCAP: fixed_cap=<none|ids:N> limit=<n> procs=<n> opens_refused=<k> -> PASS|FAIL ::` — at the
//!   desktop ignition (`boot::ignite`), an arming line (R80), not a test.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

/// Id-space rows kept for the system's own (owner-0 / compat) rows: the login screen and its control
/// row, the notice, and the compat row. The limit counts app rows only, so these always find a slot.
pub const SYS_ROWS: usize = 4;

/// The dock's pinned tiles (`dock::pins_applied`: shell, console, Quarry, pulse) — the dock draws the
/// app rows PLUS these, so the dock term leaves room for all four.
pub const DOCK_PINS: usize = 4;

/// The largest window surface the ABI hands out (CRYSTAL-HD: 288x288 ARGB8888, both arches).
const SURFACE_MAX: usize = 288 * 288 * 4;
#[cfg(target_arch = "x86_64")]
const _: () = assert!(SURFACE_MAX == crate::arch::x86_64::memory::FB_WIN_SLOT_SIZE);

/// Per-window kernel heap cost: the pacer's shadow and the pass's mirror of the largest surface
/// (`wm::PACE_SHADOW` / `PACE_MIRROR`), plus a page for the row's side state.
pub const WIN_COST: usize = 2 * SURFACE_MAX + 4096;

/// Per-process kernel heap cost: a program owns a window, plus its kernel task stacks and handle state.
pub const PROC_COST: usize = WIN_COST + 64 * 1024;

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
/// The last measured dock term (`usize::MAX` until a panel answers).
static DOCK_LAST: AtomicUsize = AtomicUsize::new(usize::MAX);

/// The `ids` term.
#[inline]
pub fn ids_term() -> usize {
    super::wm::MAX_WINDOWS.saturating_sub(SYS_ROWS).max(1)
}

/// The `asids` term (0 where this build has no process table).
#[inline]
pub fn asids_term() -> usize {
    #[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
    {
        crate::arch::syscall::proc_table_rows()
    }
    #[cfg(not(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64")))]
    {
        0
    }
}

/// The `dock` term: the most app tiles the dock can host on the live panel beside its pins, or `None`
/// when no panel is attached (or no dock is built) — the term is then absent, not zero.
pub fn dock_term() -> Option<usize> {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        // `try_lock`, never `lock`: a caller may already hold `WRITER` (or another core's pass does);
        // then the last answer stands (`DOCK_LAST`, `usize::MAX` = never measured = absent).
        let fb = match super::WRITER.try_lock() {
            Some(g) => *g,
            None => {
                let last = DOCK_LAST.load(Relaxed);
                return if last == usize::MAX { None } else { Some(last) };
            }
        };
        if !fb.is_ready() {
            return None;
        }
        let info = fb.info();
        let d = dock_term_for(info.width, info.height);
        DOCK_LAST.store(d, Relaxed);
        Some(d)
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        None
    }
}

/// The `dock` term for an explicit panel — for callers that already hold the geometry (and may hold
/// `WRITER`, so must not re-take it).
pub fn dock_term_for(pw: usize, ph: usize) -> usize {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        let mut n = super::wm::MAX_WINDOWS;
        while n > DOCK_PINS {
            if super::dock::Layout::for_panel(n, pw, ph).is_some() {
                return n - DOCK_PINS;
            }
            n -= 1;
        }
        0
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        let _ = (pw, ph);
        super::wm::MAX_WINDOWS
    }
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
    let dock = dock_term();
    let mut dbuf = [0u8; 8];
    let dock_s = match dock {
        Some(d) => fmt_usize(d, &mut dbuf),
        None => "none",
    };
    serial_println!(
        "[wm] limit windows={} procs={} from=mem:{},asids:{},dock:{},ids:{} (R90)",
        win_limit(),
        proc_limit(),
        free >> 20,
        asids_term(),
        dock_s,
        ids_term()
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

/// The live window limit: app rows `wm` admits. Arms on first use.
pub fn win_limit() -> usize {
    if !ARMED.load(Relaxed) {
        arm();
    }
    let mut n = ids_term().min(MEM_WIN.load(Relaxed));
    if let Some(d) = dock_term() {
        n = n.min(d.max(1));
    }
    n.max(1)
}

/// The live process limit: ring-3 programs `sys_spawn` admits (never more than the window limit — every
/// program must be able to own a window).
pub fn proc_limit() -> usize {
    let a = asids_term();
    if a == 0 {
        return 0;
    }
    a.min(MEM_PROC.load(Relaxed)).min(win_limit()).max(1)
}

/// The dock row count every "can the dock host the full table" check asks, for the panel the caller
/// holds: the app limit on that panel plus the pins, never past the id space. Parametric on the LIVE
/// limit, so it holds for every table state `wm` admits. Takes no `WRITER` (the callers hold the panel).
pub fn dock_rows(pw: usize, ph: usize) -> usize {
    if !ARMED.load(Relaxed) {
        arm();
    }
    let d = dock_term_for(pw, ph);
    DOCK_LAST.store(d, Relaxed);
    let apps = ids_term().min(MEM_WIN.load(Relaxed)).min(d.max(1));
    (apps + DOCK_PINS).min(super::wm::MAX_WINDOWS)
}

/// `wm::create_inner` refused an app row (`ids == true`: the id space itself, not the limit). Called AFTER
/// the table lock drops. One wire line per refusal (the first 16, then every 64th), one notice per burst.
pub fn note_open_refused(apps: usize, ids: bool) {
    let k = OPENS_REFUSED.fetch_add(1, Relaxed) + 1;
    if k <= 16 || k % 64 == 0 {
        serial_println!(
            "[wm] REFUSED create reason={} n={} refused={} (R90)",
            if ids { "ids" } else { "limit" },
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

/// M4 — the witness, printed once at the desktop ignition. `fixed_cap` names the term that binds when
/// it is a compile-time width (`ids:N`) rather than the machine (`none`). PASS: the eleventh window fits
/// (`limit >= 11`) and nothing has been refused yet.
pub fn witness() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Relaxed) {
        return;
    }
    let limit = win_limit();
    let refused = OPENS_REFUSED.load(Relaxed);
    let ids_bind = limit == ids_term();
    let mut ib = [0u8; 8];
    let ids_s = fmt_usize(ids_term(), &mut ib);
    serial_println!(
        ":: WINDOWCAP: fixed_cap={}{} limit={} procs={} opens_refused={} -> {} ::",
        if ids_bind { "ids:" } else { "none" },
        if ids_bind { ids_s } else { "" },
        limit,
        proc_limit(),
        refused,
        if limit >= 11 && refused == 0 { "PASS" } else { "FAIL" }
    );
}
