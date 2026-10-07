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
//!
//! SPLASHSTALL (rmbp-ledger B510) — every step under the held splash says when it BEGINS and when it
//! ENDS (`[boot] step=<name> begin at_ms=<n> budget_ms=<n>` / `[boot] step=<name> end ms=<n> budget_ms=<n>
//! over=<0|1> …`), and a watchdog that does NOT run on the stepping task (flight 26's two stalled boots held
//! "Starting" on the glass with the device-service pass — and with it the splash's own 5 s bound — never
//! coming back) says, every 5 s past a step's budget, `[boot] step=<name> OVER budget_ms=<n> elapsed_ms=<n>
//! last=<witness> cmds=<n>` on the wire AND on the panel under the splash word. See the block at this
//! file's tail and `docs/dev/evidence/rmbp-1005/splashstall.md`.

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
    scope_note(&SC_RD_CMDS, &SC_RD_SECTORS, sectors); // REGISTRYCHUNK (B508): the armed scope's own share
}

/// One write command of `sectors` 512 B sectors reached the medium.
#[inline]
pub fn note_write(sectors: u64) {
    WR_CMDS.fetch_add(1, Ordering::Relaxed);
    WR_SECTORS.fetch_add(sectors, Ordering::Relaxed);
    scope_note(&SC_WR_CMDS, &SC_WR_SECTORS, sectors); // REGISTRYCHUNK (B508)
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

/// Start step `name`; `label` is what the held splash says meanwhile ("Reading the volume").
///
/// SPLASHSTALL (B510): the step is entered in the live table the splash watchdog reads, and its begin is
/// said ONCE on the wire — `[boot] step=<name> begin at_ms=<n> budget_ms=<n>`. A step re-begun while it is
/// still live (a `Busy` store mount retried across passes, its `Step` dropped un-ended) keeps its FIRST
/// start: the boot has been waiting on it since then, and that is what its budget measures.
pub fn begin(name: &'static str, label: &'static str) -> Step {
    // Painted once per distinct label: a step retried across passes (a `Busy` store mount) does not
    // composite the glass on every pass.
    static LAST: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("");
    let fresh = { let mut l = LAST.lock(); if *l != label { *l = label; true } else { false } };
    if fresh {
        crate::splash::step_label(label);
    }
    let (t0, io0, new) = live_enter(name, label);
    if new {
        serial_println!("[boot] step={} begin at_ms={} budget_ms={}", name, t0, budget_of(name).0);
        watch_start();
    }
    Step { name, t0, io0 }
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
        let (budget, _) = budget_of(self.name);
        let over = live_leave(self.name, ms, budget); // SPLASHSTALL (B510): out of the watchdog's table, into the boot's tally
        serial_println!(
            "[boot] step={} end ms={} budget_ms={} over={} blocks_read={} blocks_written={} cmds={}{}{}",
            self.name, ms, budget, over, d.blocks_read(), d.blocks_written(), d.cmds(),
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

// ── REGISTRYCHUNK (rmbp-ledger B508) — one core's share of the medium counters ─────────────────────────────────
//
// The counters above are the CARD's: every core's commands. A step that shares the card with another (flight 27:
// the registry build beside the jobs scan) cannot name its own cost from them. A scope, armed by ONE task on ONE
// core, also counts what that core issues (x86: `percpu::this_cpu`; elsewhere every command, said `global`).
// One scope at a time; armed only after login (per-CPU state is up long before). Cost when disarmed: one load.

const SCOPE_OFF: usize = usize::MAX;
static SCOPE_CPU: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(SCOPE_OFF);
static SC_RD_CMDS: AtomicU64 = AtomicU64::new(0);
static SC_RD_SECTORS: AtomicU64 = AtomicU64::new(0);
static SC_WR_CMDS: AtomicU64 = AtomicU64::new(0);
static SC_WR_SECTORS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn scope_note(cmds: &AtomicU64, sectors: &AtomicU64, n: u64) {
    let s = SCOPE_CPU.load(Ordering::Relaxed);
    if s == SCOPE_OFF {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    if crate::arch::percpu::this_cpu().cpu_index as usize != s {
        return;
    }
    cmds.fetch_add(1, Ordering::Relaxed);
    sectors.fetch_add(n, Ordering::Relaxed);
}

/// Arm the scope on the calling core (`true` = armed; `false` = another scope holds it — use [`io`]).
pub fn scope_arm() -> bool {
    #[cfg(target_arch = "x86_64")]
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    #[cfg(not(target_arch = "x86_64"))]
    let cpu = 0usize;
    SCOPE_CPU.compare_exchange(SCOPE_OFF, cpu, Ordering::AcqRel, Ordering::Relaxed).is_ok()
}

/// Disarm the scope.
pub fn scope_disarm() {
    SCOPE_CPU.store(SCOPE_OFF, Ordering::Release);
}

/// The armed scope's counters now (deltas via [`Io::since`]).
pub fn scope_io() -> Io {
    Io {
        rd_cmds: SC_RD_CMDS.load(Ordering::Relaxed),
        rd_sectors: SC_RD_SECTORS.load(Ordering::Relaxed),
        wr_cmds: SC_WR_CMDS.load(Ordering::Relaxed),
        wr_sectors: SC_WR_SECTORS.load(Ordering::Relaxed),
    }
}

/// What the scope's counts mean on the wire: `core<n>` (x86, one core's commands) or `global` (the card's).
pub fn scope_word() -> alloc::string::String {
    let s = SCOPE_CPU.load(Ordering::Relaxed);
    if s == SCOPE_OFF || cfg!(not(target_arch = "x86_64")) {
        alloc::string::String::from("global")
    } else {
        alloc::format!("core{}", s)
    }
// ── SPLASHSTALL (rmbp-ledger B510) — the steps under the splash, their budgets, and the watchdog ─────────
// Flight 26 (FLIGHT26.md §2 LOADERSTALL, re-read by B490): two boots sat on the held splash's "Starting" for
// minutes with no wire. "Starting" was `stage-resolve`'s word, painted at ~13 s; the splash's own 5 s bound
// (`splash::hold_service`) is polled from the same device-service pass the step runs in, so a step that never
// returns also never lets the bound fire. Three things follow, all in this block:
//   1. every step under the splash is in [`LIVE`] from its begin line to its end line;
//   2. a watchdog that is NOT the stepping task (x86: its own kernel task on a sibling core, `watch_start`)
//      reads [`LIVE`] every [`WATCH_MS`] and, for a step past its budget, says
//      `[boot] step=<name> OVER budget_ms=<n> elapsed_ms=<n> last=<witness> cmds=<n>` on the wire and the
//      same facts on the panel under the splash word (`splash::over_label`), every [`OVER_EVERY_MS`] until
//      the step ends — the panel line is the fallback when the FTDI is not there (flight 26: no wire at all);
//   3. the boot's tally (`steps=<n> slowest=<name>:<ms> over=<n>`) rides `:: BOOT:` and `tests splash`.
// The watchdog takes nothing it can wait on: `LIVE` by `try_lock` (a busy table skips one 250 ms look), no
// allocation, the panel by `try_lock` in `splash::over_label`.

/// Each step's budget: the SLOWEST green time on flights 24–27's wire, times 3 (B510's rule). `src` names the
/// line it came from. Steps with no green time on any wire yet carry the held splash's own bound (5000 ms,
/// `splash::hold_service`) and say `hold-bound`: the next flight's `end ms=` is their first measurement.
pub const BUDGETS: [(&str, u64, &str); 5] = [
    ("store-wait", 5000, "hold-bound"),       // the splash up, the users store's volume not yet mountable — no wire line before B510
    ("users-load", 249, "f26:83x3"),          // f26-boot.log `[boot] step=users-load ms=83` (f24 83, f25 82, f27 81)
    ("root-mount", 3891, "f24:1297x3"),       // f24-boots.log `[boot] step=root-mount ms=1297` (f25 1281, f26 590, f27 192)
    ("stage-resolve", 927, "f27:309x3"),      // f27-boot1.log `[boot] step=stage-resolve ms=309` (f26 300, f24 200)
    ("first-screen", 5000, "hold-bound"),     // stage resolved -> the first screen painted (`splash::hold_release`) — no wire line before B510
];
/// A step not in [`BUDGETS`] gets the held splash's bound.
pub const DEFAULT_BUDGET_MS: u64 = 5000;
/// How often an over-budget step repeats its OVER line (wire and panel).
pub const OVER_EVERY_MS: u64 = 5000;
/// The watchdog's look period.
pub const WATCH_MS: u64 = 250;

/// `(budget_ms, src)` for `name`.
pub fn budget_of(name: &str) -> (u64, &'static str) {
    BUDGETS.iter().find(|b| b.0 == name).map(|b| (b.1, b.2)).unwrap_or((DEFAULT_BUDGET_MS, "default"))
}

/// One live step: what the watchdog needs to say about it without asking anyone.
#[derive(Clone, Copy)]
struct Live {
    name: &'static str,
    label: &'static str,
    t0: u64,
    io0: Io,
    budget: u64,
    last: &'static str,
    next_over: u64,
    overs: u32,
}
const LIVE_NONE: Live = Live { name: "", label: "", t0: 0, io0: Io { rd_cmds: 0, rd_sectors: 0, wr_cmds: 0, wr_sectors: 0 }, budget: 0, last: "", next_over: 0, overs: 0 };
const LIVE_CAP: usize = 4;
static LIVE: crate::sync::Mutex<[Live; LIVE_CAP]> = crate::sync::Mutex::new([LIVE_NONE; LIVE_CAP]);

static STEPS_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static OVER_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static SLOWEST: crate::sync::Mutex<(&'static str, u64)> = crate::sync::Mutex::new(("none", 0));

/// Enter `name` in the live table; `(t0, io0, new)`. A name already live keeps its first start.
fn live_enter(name: &'static str, label: &'static str) -> (u64, Io, bool) {
    let mut g = LIVE.lock();
    if let Some(l) = g.iter().find(|l| l.name == name) {
        return (l.t0, l.io0, false);
    }
    let (t0, io0) = (crate::arch::ms(), io());
    let budget = budget_of(name).0;
    if let Some(slot) = g.iter_mut().find(|l| l.name.is_empty()) {
        *slot = Live { name, label, t0, io0, budget, last: "begin", next_over: t0 + budget, overs: 0 };
    }
    (t0, io0, true)
}

/// Take `name` out of the live table and into the tally; returns `over` (1 when it ran past its budget).
fn live_leave(name: &'static str, ms: u64, budget: u64) -> u8 {
    use core::sync::atomic::Ordering::Relaxed;
    let overs = {
        let mut g = LIVE.lock();
        match g.iter_mut().find(|l| l.name == name) {
            Some(l) => { let o = l.overs; *l = LIVE_NONE; o }
            None => 0,
        }
    };
    STEPS_N.fetch_add(1, Relaxed);
    { let mut s = SLOWEST.lock(); if ms >= s.1 || s.0 == "none" { *s = (name, ms); } }
    let over = overs > 0 || ms > budget;
    if over && overs == 0 {
        OVER_N.fetch_add(1, Relaxed); // ran past its budget between two watchdog looks (or with no watchdog): still over
    }
    if overs > 0 {
        crate::splash::over_label(""); // the step ended: its OVER line leaves the panel
    }
    over as u8
}

/// The innermost live step's last witness — what the OVER line's `last=` names. Called from inside a step at its
/// sub-stages (`users::stage_resolve`: `store-read`, `publish`, `desktop-build`, …).
pub fn witness(what: &'static str) {
    let mut g = LIVE.lock();
    if let Some(l) = g.iter_mut().rev().find(|l| !l.name.is_empty()) {
        l.last = what;
    }
}

/// Begin `name` without holding its `Step` (a wait that ends somewhere else: `store-wait`, `first-screen`).
/// Only while the splash holds the glass — these steps are the splash's, and do not exist without it.
pub fn open(name: &'static str, label: &'static str) {
    if crate::splash::holding() {
        let _ = begin(name, label);
    }
}

/// End `name` if it is live (silent otherwise). `extra` is appended to its end line.
pub fn close(name: &'static str, extra: &str) {
    let found = LIVE.lock().iter().find(|l| l.name == name).map(|l| (l.t0, l.io0));
    if let Some((t0, io0)) = found {
        Step { name, t0, io0 }.end(extra);
    }
}

/// The splash released the glass (`splash::hold_release`): the waits that were the splash's end with it.
pub fn splash_released(by: &str) {
    if LIVE.lock().iter().all(|l| l.name.is_empty()) {
        return;
    }
    let extra = alloc::format!("by={}", by);
    close("store-wait", &extra);
    close("first-screen", &extra);
}

/// `(steps, slowest_name, slowest_ms, over)` so far.
pub fn tally() -> (u32, &'static str, u64, u32) {
    use core::sync::atomic::Ordering::Relaxed;
    let s = *SLOWEST.lock();
    (STEPS_N.load(Relaxed), s.0, s.1, OVER_N.load(Relaxed))
}

/// The tally as the `:: BOOT:` line carries it: `steps=<n> slowest=<name>:<ms> over=<n>`.
pub fn boot_fields() -> alloc::string::String {
    let (n, name, ms, over) = tally();
    alloc::format!("steps={} slowest={}:{} over={}", n, name, ms, over)
}

static WATCHING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static WATCH_KIND: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0); // 0 none, 1 task

/// Start the watchdog if it is not running, and register `tests splash` (once).
fn watch_start() {
    use core::sync::atomic::Ordering;
    static REG: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !REG.swap(true, Ordering::AcqRel) {
        crate::tests::register("splash", splash_selftest);
    }
    #[cfg(target_arch = "x86_64")]
    if !WATCHING.swap(true, Ordering::AcqRel) {
        // A sibling core: the stepping task is the one that may never come back, so the watchdog must not
        // share its core's queue. `sibling_online_cpu` falls back to the caller's core when it is alone.
        let me = crate::arch::percpu::this_cpu().cpu_index as usize;
        let cpu = crate::arch::sched::sibling_online_cpu(me);
        WATCH_KIND.store(1, Ordering::Relaxed);
        crate::arch::sched::spawn("bootwatch", watch_task, 0, cpu, crate::arch::sched::PRIO_HIGH);
    }
    // aarch64: no watchdog task yet (owed, B510's design note) — the begin/end lines and the tally still run.
}

#[cfg(target_arch = "x86_64")]
fn watch_task(_arg: usize) {
    loop {
        crate::arch::sched::sleep_ms(WATCH_MS);
        if !poll() {
            break;
        }
    }
    WATCHING.store(false, core::sync::atomic::Ordering::Release);
}

/// One watchdog look. Returns whether to keep watching (a step is live, or the splash still holds the glass).
/// Never waits: a busy table is a skipped look.
pub fn poll() -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    let now = crate::arch::ms();
    let mut due = [("", "", 0u64, 0u64, "", 0u64); LIVE_CAP];
    let mut n = 0usize;
    let any = {
        let Some(mut g) = LIVE.try_lock() else { return true };
        for l in g.iter_mut().filter(|l| !l.name.is_empty()) {
            let el = now.saturating_sub(l.t0);
            if el >= l.budget && now >= l.next_over {
                if l.overs == 0 {
                    OVER_N.fetch_add(1, Relaxed);
                }
                l.overs += 1;
                l.next_over = now + OVER_EVERY_MS;
                due[n] = (l.name, l.label, l.budget, el, l.last, io().since(l.io0).cmds());
                n += 1;
            }
        }
        g.iter().any(|l| !l.name.is_empty())
    };
    for d in &due[..n] {
        serial_println!("[boot] step={} OVER budget_ms={} elapsed_ms={} last={} cmds={}", d.0, d.2, d.3, d.4, d.5);
    }
    if n > 0 {
        // The panel's line: the innermost over step, in words under the splash's own word.
        let d = due[n - 1];
        let mut ln = PanelLine { b: [0; 160], n: 0 };
        use core::fmt::Write;
        let _ = write!(
            ln,
            "{} ({}) is taking {}.{} s, budget {}.{} s, last {}, {} commands",
            if d.1.is_empty() { d.0 } else { d.1 }, d.0, d.3 / 1000, (d.3 % 1000) / 100, d.2 / 1000, (d.2 % 1000) / 100, d.4, d.5
        );
        crate::splash::over_label(ln.as_str());
    }
    any || crate::splash::holding()
}

/// A bounded, allocation-free line (deadman's `Line` shape): what does not fit is dropped.
struct PanelLine {
    b: [u8; 160],
    n: usize,
}
impl PanelLine {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("boot step over budget")
    }
}
impl core::fmt::Write for PanelLine {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let take = core::cmp::min(self.b.len() - self.n, s.len());
        self.b[self.n..self.n + take].copy_from_slice(&s.as_bytes()[..take]);
        self.n += take;
        Ok(())
    }
}

/// `tests splash` (R80: a fixture runs when asked): the boot's steps under the splash, read back.
///
/// `:: SPLASH: steps=<n> slowest=<name>:<ms> over=<n> live=<n> watch=<task|none> -> PASS|FAIL|SKIP ::`
///
/// PASS: at least one step ran, none ran past its budget, none is still live. FAIL names the over count or the
/// live step. SKIP when no step ran (no store volume, a headless build).
pub fn splash_selftest() {
    let (n, name, ms, over) = tally();
    let live = LIVE.lock().iter().filter(|l| !l.name.is_empty()).count();
    let watch = if WATCH_KIND.load(core::sync::atomic::Ordering::Relaxed) == 1 { "task" } else { "none" };
    if n == 0 {
        serial_println!(":: SPLASH: steps=0 slowest=none over={} live={} watch={} -> SKIP reason=no-steps ::", over, live, watch);
        return;
    }
    let ok = over == 0 && live == 0;
    serial_println!(
        ":: SPLASH: steps={} slowest={}:{} over={} live={} watch={} -> {} ::",
        n, name, ms, over, live, watch, if ok { "PASS" } else { "FAIL" }
    );
}
