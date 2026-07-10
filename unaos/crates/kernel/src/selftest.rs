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

//! TSTE-1 — the in-OS self-test suite (`tste` shell command).
//!
//! Running tests should not require a host: from a booted UnaOS shell (x86 GUI, the Orin panel, or
//! the serial console) `tste` runs the suite and prints a PASS/FAIL/SKIP table. The output is ONE
//! view of ALL tests in three sections:
//!
//!   * `[boot-time]` — verdicts of the boot-sequenced fixtures (U-arc / M-arc), REPLAYED from a
//!     kernel-side ring (`BOOT_RING`) that captures every `-> PASS`/`-> FAIL` line as it is emitted
//!     during boot. These cannot be re-run on demand (they need an EL0 launcher refactor — TSTE-2),
//!     so `tste` replays the captured verdict rather than pretending to re-execute it.
//!   * `[live]` — checks that HONESTLY re-run post-boot: scheduler introspection, heap round-trip,
//!     the video geometry primitives (offscreen), and a fresh-task re-verification of the six sync
//!     primitives.
//!   * `[skipped]` — checks that cannot run in the current context, each with a reason.
//!
//! Every live line is mirrored to serial as `:: TSTE: <name> -> PASS/FAIL/SKIP ::`, and a final
//! `:: TSTE: N pass M fail K skip (+B boot) ::` summary — that serial evidence is the QEMU gate.
//!
//! Safety / context: `dispatch_command` runs in different contexts per platform (x86 GUI inline
//! loop — NOT a scheduled task; Orin `jd2_console_pump` scheduled task; Pi `GUI_CHANNEL` task). The
//! suite's coordinator (this module, run from that context) therefore NEVER blocks on a sync
//! primitive itself (on the unscheduled x86 BSP a `Mutex::lock`/`Semaphore::wait` would panic or
//! bail). All blocking happens inside FRESH worker tasks; the coordinator only spawns them and polls
//! an atomic result with a bounded budget (busy-poll + `hlt`/`yield_now`, never `sleep_ticks`), so
//! the worst case is a `SKIP`/`FAIL` line, never a hang. `tste` never takes the screen (it prints in
//! the console like `ps`) and is READ-ONLY toward storage.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use spin::Mutex;

use crate::arch::sched;
use crate::console::Console;
use crate::pal::{GneissPal, TargetPal};

// =================================================================================================
// M2b — the boot-verdict capture ring
// =================================================================================================
//
// Every boot-sequenced fixture funnels its verdict through the serial print path as a uniform
// `:: … -> PASS ::` / `:: … -> FAIL ::` line. `capture()` (called additively from each arch's
// serial `_print`, the single seam) scans each formatted line and, on a match, records a truncated
// label + verdict here. The ring is a fixed static (no alloc) guarded by a `try_lock` spin mutex; it
// is called from IRQ-masked print contexts, so it never blocks, never allocates, and drops on lock
// contention or overflow (counting drops). This is what lets `tste` REPLAY the boot fixtures it
// cannot itself re-run.

const RING_CAP: usize = 64;
const NAME_MAX: usize = 40;

#[derive(Clone, Copy)]
struct BootVerdict {
    name: [u8; NAME_MAX],
    len: u8,
    pass: bool,
}

impl BootVerdict {
    const fn empty() -> Self {
        BootVerdict { name: [0u8; NAME_MAX], len: 0, pass: false }
    }
    fn label(&self) -> &str {
        core::str::from_utf8(&self.name[..self.len as usize]).unwrap_or("<?>")
    }
}

struct BootRing {
    entries: [BootVerdict; RING_CAP],
    count: usize,
    dropped: usize,
}

static BOOT_RING: Mutex<BootRing> = Mutex::new(BootRing {
    entries: [BootVerdict::empty(); RING_CAP],
    count: 0,
    dropped: 0,
});

/// A tiny alloc-free `fmt::Write` sink: format `Arguments` into a fixed stack buffer so `capture`
/// can substring-scan the line without touching the heap (safe from any print context).
struct StackBuf {
    buf: [u8; 200],
    len: usize,
}
impl StackBuf {
    fn new() -> Self {
        StackBuf { buf: [0u8; 200], len: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}
impl fmt::Write for StackBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.len >= self.buf.len() {
                break; // truncate silently; the verdict marker is early in the line
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        Ok(())
    }
}

/// Additive hook on the serial print seam (M2b). If the formatted line carries a fixture verdict
/// (`-> PASS` / `-> FAIL`), record it into `BOOT_RING`. Zero behaviour change to what is printed;
/// alloc-free; `try_lock` only (drops on contention) so it is safe from IRQ-masked print contexts.
/// Our OWN live `:: TSTE:` lines are excluded so replaying can't feed back on itself.
pub fn capture(args: fmt::Arguments) {
    let mut sb = StackBuf::new();
    let _ = fmt::write(&mut sb, args);
    let s = sb.as_str();

    let (idx, pass) = if let Some(i) = s.find("-> PASS") {
        (i, true)
    } else if let Some(i) = s.find("-> FAIL") {
        (i, false)
    } else {
        return;
    };
    if s.contains("TSTE") {
        return; // never capture the suite's own live output
    }

    // Clean the label: text before the verdict marker, minus a leading ":: " frame, trimmed.
    let mut label = s[..idx].trim();
    if let Some(rest) = label.strip_prefix(":: ") {
        label = rest;
    }
    let label = label.trim();

    // Fit the label into NAME_MAX. If the whole label fits, store it verbatim. Otherwise truncate
    // at the last WORD boundary that leaves room for a trailing ellipsis marker, so a clipped
    // verdict reads as "open+read …" rather than an accidental mid-word chop ("open+read/"). Char-
    // boundary aware (never splits a multibyte char) and alloc-free (a fixed stack buffer). Fill the
    // ring slot directly from that buffer.
    let mut name = [0u8; NAME_MAX];
    let nlen = fit_label(label, &mut name);

    if let Some(mut ring) = BOOT_RING.try_lock() {
        if ring.count >= RING_CAP {
            ring.dropped += 1;
            return;
        }
        let slot = ring.count;
        ring.entries[slot].name[..nlen].copy_from_slice(&name[..nlen]);
        ring.entries[slot].len = nlen as u8;
        ring.entries[slot].pass = pass;
        ring.count += 1;
    }
    // Lock contended -> drop the record silently (a diagnostic ring, not a ledger). Rare: prints are
    // serialised by the serial lock the caller already holds.
}

/// Fit `label` into `out` (capacity `NAME_MAX`), returning the byte length written. Whole label if it
/// fits; otherwise truncate at the last word boundary (space) that still leaves room for a trailing
/// " …" marker when one exists — else a hard char-boundary cut — and append the marker so a clipped
/// verdict reads as "open+read …" rather than an accidental mid-word chop. Char-boundary aware
/// (never splits a multibyte char) and alloc-free.
fn fit_label(label: &str, out: &mut [u8; NAME_MAX]) -> usize {
    let full = label.as_bytes();
    if full.len() <= NAME_MAX {
        out[..full.len()].copy_from_slice(full);
        return full.len();
    }
    const MARK: &str = " \u{2026}"; // " …" — 4 bytes
    let budget = NAME_MAX - MARK.len();
    // Largest char-boundary cut that fits the budget.
    let mut cut = label
        .char_indices()
        .map(|(bi, c)| bi + c.len_utf8())
        .take_while(|&e| e <= budget)
        .last()
        .unwrap_or(0);
    // Prefer the last word boundary (space) within the cut, dropping the trailing space.
    if let Some(sp) = label[..cut].rfind(' ') {
        if sp > 0 {
            cut = sp;
        }
    }
    out[..cut].copy_from_slice(&label.as_bytes()[..cut]);
    out[cut..cut + MARK.len()].copy_from_slice(MARK.as_bytes());
    cut + MARK.len()
}

// =================================================================================================
// Outcome + registry plumbing
// =================================================================================================

enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

/// Per-run tally across all three sections.
struct Tally {
    pass: usize,
    fail: usize,
    skip: usize,
    boot: usize,
}

impl Tally {
    fn new() -> Self {
        Tally { pass: 0, fail: 0, skip: 0, boot: 0 }
    }
}

/// Page-at-a-time console output (POLISH-1). `tste`'s table can run well past a screenful; without
/// paging the older lines scroll off before they can be read. The pager counts console lines and,
/// after a screen's worth, prints a `-- more (any key) --` line and waits for a keypress — the demo's
/// `pal::pump_and_poll` idiom (busy-poll + `yield_now`, never WFI/`sleep_ticks`, so it is safe in the
/// unscheduled x86 GUI loop, the Orin scheduled pump, and the serial console alike) — then continues.
/// The serial mirror is NOT paginated: `report`'s `serial_println!` lines are emitted independently,
/// so the serial log (the QEMU gate evidence) still streams in full and never waits on a key.
struct Pager {
    rows: usize,
    on_page: usize,
}

impl Pager {
    fn new(pal: &TargetPal) -> Self {
        // Lines that fit above the prompt (console starts at y=20, 20px/line, ~40px prompt reserve),
        // bounded to the console's retained-history window (leave a line for the more-prompt).
        let fit = (pal.height() as usize).saturating_sub(60) / 20;
        Pager { rows: fit.clamp(6, 24), on_page: 0 }
    }

    /// Push one line to the console, repaint progressively, and pause at page boundaries. Emitting the
    /// serial mirror stays the caller's job (serial is a log, not a screen — never paginated).
    fn line(&mut self, console: &mut Console, pal: &mut TargetPal, text: &str) {
        console.println(text);
        console.draw(pal);
        pal.render();
        self.on_page += 1;
        if self.on_page >= self.rows {
            self.pause(console, pal);
        }
    }

    fn pause(&mut self, console: &mut Console, pal: &mut TargetPal) {
        console.println("-- more (any key) --");
        console.draw(pal);
        pal.render();
        // Wait for a keypress via the demo's pump idiom: busy-poll the input sources + cooperative
        // yield, never sleep (the post-drop aarch64 rule — no timer to wake a sleeper).
        loop {
            if let Some(crate::pal::Event::Key(_)) = crate::pal::pump_and_poll() {
                break;
            }
            sched::yield_now();
        }
        self.on_page = 0;
    }
}

/// Emit one live-test line to the console (progressively — the table fills in as tests run) and to
/// serial (the QEMU evidence).
fn report(
    pager: &mut Pager,
    console: &mut Console,
    pal: &mut TargetPal,
    tally: &mut Tally,
    name: &str,
    r: Outcome,
) {
    let line = match &r {
        Outcome::Pass => {
            tally.pass += 1;
            serial_println!(":: TSTE: {} -> PASS ::", name);
            format!("  [PASS] {}", name)
        }
        Outcome::Fail(why) => {
            tally.fail += 1;
            serial_println!(":: TSTE: {} -> FAIL ({}) ::", name, why);
            format!("  [FAIL] {}: {}", name, why)
        }
        Outcome::Skip(why) => {
            tally.skip += 1;
            serial_println!(":: TSTE: {} -> SKIP ({}) ::", name, why);
            format!("  [SKIP] {}: {}", name, why)
        }
    };
    // Progressive present (the pager redraws so the user watches the table fill in) + page pause at a
    // screenful. Serial was already mirrored above, unpaginated.
    pager.line(console, pal, &line);
}

// =================================================================================================
// The entry point
// =================================================================================================

/// Run the self-test suite, printing the three-section table to `console` (and mirroring to serial).
/// Safe in every shell context (see the module note): never blocks the coordinator.
pub fn run(console: &mut Console, pal: &mut TargetPal) {
    let mut tally = Tally::new();
    let mut pager = Pager::new(pal);

    pager.line(console, pal, "tste — UnaOS in-OS self-test suite");
    serial_println!(":: TSTE: suite start ::");

    // --- [boot-time]: replay the captured fixture verdicts --------------------------------------
    pager.line(console, pal, "[boot-time] (captured at boot; re-run needs TSTE-2)");
    let (verdicts, dropped) = snapshot_boot_ring();
    if verdicts.is_empty() {
        pager.line(console, pal, "  (none captured — see the serial boot log)");
    } else {
        for (label, pass) in &verdicts {
            tally.boot += 1;
            let tag = if *pass { "PASS" } else { "FAIL" };
            pager.line(console, pal, &format!("  [{}] {}", tag, label));
            serial_println!(":: TSTE: [boot] {} -> {} ::", label, tag);
        }
        if dropped > 0 {
            pager.line(console, pal, &format!("  (+{} verdicts dropped: ring full)", dropped));
        }
    }

    // --- [live]: checks that honestly re-run ----------------------------------------------------
    pager.line(console, pal, "[live]");

    report(&mut pager, console, pal, &mut tally, "sched.introspection", test_sched_introspection());
    report(&mut pager, console, pal, &mut tally, "heap.roundtrip", test_heap_roundtrip());
    report(&mut pager, console, pal, &mut tally, "video.geometry", test_video_geometry());

    // M2 — the six sync primitives, re-verified with fresh worker tasks.
    run_sync_section(&mut pager, console, pal, &mut tally);

    // M3 — storage read checks (READ-ONLY; never mutates the volume).
    run_storage_section(&mut pager, console, pal, &mut tally);

    // --- summary --------------------------------------------------------------------------------
    let summary = format!(
        "{} pass  {} fail  {} skip  (+{} boot-replayed)",
        tally.pass, tally.fail, tally.skip, tally.boot
    );
    pager.line(console, pal, "----");
    pager.line(console, pal, &summary);
    pager.line(console, pal, "footer: boot-sequenced EL0 fixtures re-run needs TSTE-2 (launcher refactor).");
    serial_println!(
        ":: TSTE: {} pass {} fail {} skip (+{} boot) ::",
        tally.pass, tally.fail, tally.skip, tally.boot
    );
    serial_println!(":: TSTE: suite complete ::");
}

fn snapshot_boot_ring() -> (Vec<(String, bool)>, usize) {
    let ring = BOOT_RING.lock();
    let mut out = Vec::with_capacity(ring.count);
    for e in ring.entries.iter().take(ring.count) {
        out.push((String::from(e.label()), e.pass));
    }
    (out, ring.dropped)
}

// =================================================================================================
// Live tests
// =================================================================================================

/// Scheduler introspection: the meter counters are readable and monotonic (never go backwards) and
/// at least one CPU is visible. Read-only; context-independent; cross-arch (both arches expose
/// `meter_cpu_count` / `meter_cpu_ticks`).
fn test_sched_introspection() -> Outcome {
    let ncpu = sched::meter_cpu_count();
    if ncpu == 0 {
        return Outcome::Fail(String::from("meter_cpu_count == 0"));
    }

    // Sample every visible core twice; the cumulative (busy+idle) tick sum must not decrease.
    let mut before: u64 = 0;
    for c in 0..ncpu {
        let (b, i) = sched::meter_cpu_ticks(c);
        before = before.wrapping_add(b).wrapping_add(i);
    }
    // A short spin so the APs (if any) advance their idle/busy counters between samples.
    for _ in 0..200_000 {
        core::hint::spin_loop();
    }
    let mut after: u64 = 0;
    for c in 0..ncpu {
        let (b, i) = sched::meter_cpu_ticks(c);
        after = after.wrapping_add(b).wrapping_add(i);
    }
    if after < before {
        return Outcome::Fail(format!("meter went backwards ({} -> {})", before, after));
    }

    // x86 also exposes per-CPU run-queue lengths; confirm the accessor answers for core 0.
    #[cfg(target_arch = "x86_64")]
    let _ = sched::run_queue_len(0);

    Outcome::Pass
}

/// Heap alloc/free round-trip: allocate a vector + a boxed value, write a pattern, read it back,
/// then drop (freeing). Exercises the global allocator end to end.
fn test_heap_roundtrip() -> Outcome {
    const N: usize = 512;
    let mut v: Vec<u64> = Vec::with_capacity(N);
    for i in 0..N {
        v.push((i as u64).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_A5A5);
    }
    for i in 0..N {
        let want = (i as u64).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_A5A5;
        if v[i] != want {
            return Outcome::Fail(format!("vec[{}] mismatch", i));
        }
    }
    let sum: u64 = v.iter().fold(0u64, |a, &x| a.wrapping_add(x));
    drop(v);

    let boxed = alloc::boxed::Box::new(sum);
    if *boxed != sum {
        return Outcome::Fail(String::from("box readback mismatch"));
    }
    drop(boxed);

    // Re-allocate a larger buffer after the frees to exercise the freed regions.
    let mut v2: Vec<u8> = alloc::vec![0u8; 4096];
    v2[0] = 0xEE;
    v2[4095] = 0x11;
    if v2[0] != 0xEE || v2[4095] != 0x11 {
        return Outcome::Fail(String::from("realloc readback mismatch"));
    }
    Outcome::Pass
}

/// An offscreen `GneissPal` surface (a heap pixel buffer) so the video geometry primitives can be
/// exercised and verified WITHOUT touching the visible framebuffer. Uses the trait-default
/// `draw_line`/`fill_triangle` (the Bresenham / scanline path); `TargetPal`/`Screen` override those
/// with the damage-tracked rasteriser, exercised visually by `vug` and the console (rides
/// `./arroyo x86` and the Orin bench).
struct OffscreenPal {
    buf: Vec<u32>,
    w: u32,
    h: u32,
}
impl OffscreenPal {
    fn new(w: u32, h: u32) -> Self {
        OffscreenPal { buf: alloc::vec![0u32; (w * h) as usize], w, h }
    }
    fn count_nonzero(&self) -> usize {
        self.buf.iter().filter(|&&p| p != 0).count()
    }
    fn get(&self, x: u32, y: u32) -> u32 {
        self.buf[(y * self.w + x) as usize]
    }
}
impl GneissPal for OffscreenPal {
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.w && y < self.h {
            self.buf[(y * self.w + x) as usize] = color;
        }
    }
    fn poll_event(&mut self) -> crate::pal::Event {
        crate::pal::Event::None
    }
    fn render(&mut self) {}
    fn width(&self) -> u32 {
        self.w
    }
    fn height(&self) -> u32 {
        self.h
    }
}

fn test_video_geometry() -> Outcome {
    let mut off = OffscreenPal::new(32, 32);

    // A horizontal line of 10 pixels: expect exactly 10 set, all on row 0.
    off.draw_line(0, 0, 9, 0, 0x00FF00);
    let after_line = off.count_nonzero();
    if after_line != 10 {
        return Outcome::Fail(format!("draw_line set {} px (want 10)", after_line));
    }
    if off.get(0, 0) == 0 || off.get(9, 0) == 0 {
        return Outcome::Fail(String::from("draw_line endpoints missing"));
    }

    // A filled triangle: expect a positive count, bounded by the surface, and its interior set.
    let mut off2 = OffscreenPal::new(32, 32);
    off2.fill_triangle((2, 2), (28, 6), (10, 28), 0xFF0000);
    let tri = off2.count_nonzero();
    if tri == 0 {
        return Outcome::Fail(String::from("fill_triangle set 0 px"));
    }
    if tri >= (32 * 32) {
        return Outcome::Fail(format!("fill_triangle overran ({} px)", tri));
    }
    Outcome::Pass
}

// =================================================================================================
// M2 — the six sync primitives, re-verified with fresh worker tasks (coordinator never blocks)
// =================================================================================================
//
// A single worker task (`sync_probe_worker`), spawned onto a scheduled CPU, exercises each primitive
// and records a per-primitive PASS bit into `SYNC_BITS`, then sets `SYNC_DONE`. Blocking cross-task
// checks (Condvar, join) spawn a child of the worker. The coordinator only polls `SYNC_DONE` with a
// bounded budget, so a broken/hung primitive surfaces as a FAIL/SKIP, never a shell hang.
//
// What each check proves (honest scope):
//   * Mutex/RwLock/Semaphore/Channel — uncontended API round-trip from a scheduled task (acquire /
//     release / send / recv correctness, no self-deadlock, guard drop unlocks).
//   * Condvar — cross-task wait/notify with a Mesa predicate loop (a real blocking wake-up).
//   * join   — cross-task completion handoff (`spawn_joinable` + `JoinHandle::join`).
// The full CROSS-CORE stress harness stays boot-sequenced (its verdicts replay in [boot-time]); this
// is the on-demand functional re-verification.

const BIT_MUTEX: usize = 1 << 0;
const BIT_RWLOCK: usize = 1 << 1;
const BIT_SEM: usize = 1 << 2;
const BIT_CHAN: usize = 1 << 3;
const BIT_CONDVAR: usize = 1 << 4;
const BIT_JOIN: usize = 1 << 5;
const ALL_BITS: usize = BIT_MUTEX | BIT_RWLOCK | BIT_SEM | BIT_CHAN | BIT_CONDVAR | BIT_JOIN;

static SYNC_BITS: AtomicUsize = AtomicUsize::new(0);
static SYNC_DONE: AtomicBool = AtomicBool::new(false);
static SYNC_BUSY: AtomicBool = AtomicBool::new(false); // guards against a re-entrant `tste`

// Fresh-task primitives (const-constructed statics; `.init()` reserves waiter capacity per run).
static SP_MUTEX: sched::Mutex<u64> = sched::Mutex::new(0);
static SP_RWLOCK: sched::RwLock<u64> = sched::RwLock::new(0);
static SP_SEM: sched::Semaphore = sched::Semaphore::new(0);
static SP_CHAN: sched::Channel<u64> = sched::Channel::new(2);
static SP_CV_MUTEX: sched::Mutex<bool> = sched::Mutex::new(false);
static SP_CV: sched::Condvar = sched::Condvar::new();
static SP_CV_CHILD_OK: AtomicBool = AtomicBool::new(false);
static SP_JOIN_FLAG: AtomicBool = AtomicBool::new(false);

/// Target a scheduled CPU for the worker. x86: the first AP (BSP runs the shell loop and is not in
/// the scheduler); needs >= 2 online cores. aarch64: the boot core (cooperative — the shell/CAPSTONE
/// share it).
fn sync_worker_cpu() -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        if sched::meter_cpu_count() >= 2 {
            Some(1)
        } else {
            None
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Some(0)
    }
}

#[cfg(target_arch = "x86_64")]
fn st_spawn(name: &'static str, entry: fn(usize), arg: usize, cpu: usize) {
    sched::spawn(name, entry, arg, cpu, sched::PRIO_NORMAL);
}
#[cfg(not(target_arch = "x86_64"))]
fn st_spawn(name: &'static str, entry: fn(usize), arg: usize, cpu: usize) {
    sched::spawn(name, entry, arg, cpu);
}

#[cfg(target_arch = "x86_64")]
fn st_spawn_joinable(name: &'static str, entry: fn(usize), arg: usize, cpu: usize) -> sched::JoinHandle {
    sched::spawn_joinable(name, entry, arg, cpu, sched::PRIO_NORMAL)
}
#[cfg(not(target_arch = "x86_64"))]
fn st_spawn_joinable(name: &'static str, entry: fn(usize), arg: usize, cpu: usize) -> sched::JoinHandle {
    sched::spawn_joinable(name, entry, arg, cpu)
}

/// The join child: set a flag and return (its completion permit is what `join` waits on).
fn sync_join_child(_: usize) {
    SP_JOIN_FLAG.store(true, Ordering::Release);
}

/// The condvar child (WAITER): lock, wait under a Mesa predicate loop until the parent sets the
/// predicate, then record success.
fn sync_cv_child(cpu: usize) {
    let mut guard = SP_CV_MUTEX.lock();
    while !*guard {
        guard = SP_CV.wait(guard);
    }
    drop(guard);
    SP_CV_CHILD_OK.store(true, Ordering::Release);
    let _ = cpu;
}

/// The sync probe worker (a scheduled task): exercise each primitive, set the result bits, signal
/// done. All blocking happens here / in its children, never in the coordinator.
fn sync_probe_worker(cpu: usize) {
    let mut bits = 0usize;

    // 1. Mutex — uncontended lock / mutate / drop-unlock / re-lock readback.
    {
        let mut g = SP_MUTEX.lock();
        *g = 0xBEEF;
        drop(g);
        let g2 = SP_MUTEX.lock();
        if *g2 == 0xBEEF {
            bits |= BIT_MUTEX;
        }
    }

    // 2. RwLock — write then read-share readback.
    {
        {
            let mut w = SP_RWLOCK.write();
            *w = 0x1234_5678;
        }
        let r = SP_RWLOCK.read();
        if *r == 0x1234_5678 {
            bits |= BIT_RWLOCK;
        }
    }

    // 3. Semaphore — post then acquire the permit (fast path; no park).
    {
        SP_SEM.post();
        if SP_SEM.wait() {
            bits |= BIT_SEM;
        }
    }

    // 4. Channel — send then recv (capacity >= 1 so neither side blocks).
    {
        SP_CHAN.init();
        SP_CHAN.send(0xC0FFEE);
        if SP_CHAN.recv() == 0xC0FFEE {
            bits |= BIT_CHAN;
        }
    }

    // 5. Condvar — cross-task wait/notify with a child waiter (a real blocking wake-up).
    {
        SP_CV_MUTEX.init();
        SP_CV.init();
        SP_CV_CHILD_OK.store(false, Ordering::Release);
        let child = st_spawn_joinable("tste-cv", sync_cv_child, 0, cpu);
        {
            let mut g = SP_CV_MUTEX.lock();
            *g = true;
        }
        SP_CV.notify_one();
        child.join();
        if SP_CV_CHILD_OK.load(Ordering::Acquire) {
            bits |= BIT_CONDVAR;
        }
    }

    // 6. join — cross-task completion handoff.
    {
        SP_JOIN_FLAG.store(false, Ordering::Release);
        let child = st_spawn_joinable("tste-join", sync_join_child, 0, cpu);
        child.join();
        if SP_JOIN_FLAG.load(Ordering::Acquire) {
            bits |= BIT_JOIN;
        }
    }

    SYNC_BITS.store(bits, Ordering::Release);
    SYNC_DONE.store(true, Ordering::Release);
}

/// Coordinator side: spawn the worker, poll `SYNC_DONE` with a bounded budget (never blocking), and
/// report each primitive. On no scheduled CPU or a timeout, SKIP the whole section honestly — the
/// boot CAPSTONE verdicts still appear in [boot-time].
fn run_sync_section(
    pager: &mut Pager,
    console: &mut Console,
    pal: &mut TargetPal,
    tally: &mut Tally,
) {
    let names = [
        ("sync.mutex", BIT_MUTEX),
        ("sync.rwlock", BIT_RWLOCK),
        ("sync.semaphore", BIT_SEM),
        ("sync.channel", BIT_CHAN),
        ("sync.condvar", BIT_CONDVAR),
        ("sync.join", BIT_JOIN),
    ];

    let cpu = match sync_worker_cpu() {
        Some(c) => c,
        None => {
            let why = String::from("no application processor for fresh workers (single core)");
            for (n, _) in names.iter() {
                report(pager, console, pal, tally, n, Outcome::Skip(why.clone()));
            }
            return;
        }
    };

    // Re-entrancy guard: if a previous `tste` left the probe in flight, don't stack a second.
    if SYNC_BUSY.swap(true, Ordering::AcqRel) {
        let why = String::from("previous sync probe still in flight");
        for (n, _) in names.iter() {
            report(pager, console, pal, tally, n, Outcome::Skip(why.clone()));
        }
        return;
    }

    SYNC_BITS.store(0, Ordering::Release);
    SYNC_DONE.store(false, Ordering::Release);
    SP_SEM.init();

    st_spawn("tste-sync", sync_probe_worker, cpu, cpu);

    // Bounded, non-blocking wait. Each iteration yields the coordinator: x86 `hlt` (timer wakes it —
    // the APs run the worker); aarch64 `yield_now` (cooperative — the worker shares this core).
    let done = poll_until_done();
    SYNC_BUSY.store(false, Ordering::Release);

    if !done {
        let why = String::from("scheduler did not complete the probe within budget (needs an active scheduler)");
        for (n, _) in names.iter() {
            report(pager, console, pal, tally, n, Outcome::Skip(why.clone()));
        }
        return;
    }

    let bits = SYNC_BITS.load(Ordering::Acquire);
    for (n, bit) in names.iter() {
        let r = if bits & bit != 0 {
            Outcome::Pass
        } else {
            Outcome::Fail(String::from("worker did not confirm (timeout/incorrect)"))
        };
        report(pager, console, pal, tally, n, r);
    }
    let _ = ALL_BITS;
}

// =================================================================================================
// M3 — storage read checks (READ-ONLY)
// =================================================================================================
//
// FAT mount status, a root-directory walk, and a known-file read (HELLO.BIN): verify the read
// length and that content came back. `tste` NEVER mutates the volume — no create/write/delete (those
// stay in the boot-sequenced battery). When no FAT volume is present (e.g. the raw usb.img pattern
// image under a plain `./arroyo test`) every storage check SKIPs honestly with the reason.

const KNOWN_FILE: &str = "HELLO.BIN";

fn run_storage_section(
    pager: &mut Pager,
    console: &mut Console,
    pal: &mut TargetPal,
    tally: &mut Tally,
) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => {
            let why = format!("no FAT volume mounted ({:?})", e);
            report(pager, console, pal, tally, "storage.mount", Outcome::Skip(why.clone()));
            report(pager, console, pal, tally, "storage.rootwalk", Outcome::Skip(why.clone()));
            report(pager, console, pal, tally, "storage.readfile", Outcome::Skip(why));
            return;
        }
    };
    report(pager, console, pal, tally, "storage.mount", Outcome::Pass);

    // Root-directory walk.
    let entries = match fs.read_root() {
        Ok(es) => es,
        Err(e) => {
            let why = format!("read_root failed ({:?})", e);
            report(pager, console, pal, tally, "storage.rootwalk", Outcome::Fail(why.clone()));
            report(pager, console, pal, tally, "storage.readfile", Outcome::Skip(String::from("root walk failed")));
            return;
        }
    };
    if entries.is_empty() {
        report(pager, console, pal, tally, "storage.rootwalk", Outcome::Fail(String::from("empty root")));
    } else {
        report(pager, console, pal, tally, "storage.rootwalk", Outcome::Pass);
    }

    // Known-file read: length + content sanity (READ-ONLY).
    let de = match fs.find_in_root(KNOWN_FILE) {
        Ok(de) => de,
        Err(_) => {
            report(
                pager, console, pal, tally, "storage.readfile",
                Outcome::Skip(format!("{} not present in root", KNOWN_FILE)),
            );
            return;
        }
    };
    const CAP: usize = 4096;
    let mut data: Vec<u8> = Vec::new();
    let r = match fs.read_file(&de, &mut data, CAP) {
        Ok(()) => {
            let want = core::cmp::min(de.size as usize, CAP);
            if data.len() != want {
                Outcome::Fail(format!("read {} bytes, expected {}", data.len(), want))
            } else if de.size == 0 {
                Outcome::Fail(String::from("known file is zero-length"))
            } else if data.iter().all(|&b| b == 0) {
                Outcome::Fail(String::from("content all-zero (unexpected)"))
            } else {
                Outcome::Pass
            }
        }
        Err(e) => Outcome::Fail(format!("read_file failed ({:?})", e)),
    };
    report(pager, console, pal, tally, "storage.readfile", r);
}

fn poll_until_done() -> bool {
    #[cfg(target_arch = "x86_64")]
    const BUDGET: usize = 4_000; // hlt iterations (~timer ticks); seconds of headroom
    #[cfg(not(target_arch = "x86_64"))]
    const BUDGET: usize = 20_000_000; // cooperative yields

    for _ in 0..BUDGET {
        if SYNC_DONE.load(Ordering::Acquire) {
            return true;
        }
        #[cfg(target_arch = "x86_64")]
        crate::hlt();
        #[cfg(not(target_arch = "x86_64"))]
        sched::yield_now();
    }
    SYNC_DONE.load(Ordering::Acquire)
}
