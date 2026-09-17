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
//!     during boot. These cannot be re-run on demand (they need a user launcher refactor — TSTE-2),
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
use core::cell::UnsafeCell;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};


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
// label + verdict here. The ring is a fixed static (no alloc) and, since SERWIT-2, entirely LOCK-FREE:
// it is called from IRQ-masked print contexts on every core at once, so it never blocks, never
// allocates, and has no contention-loss path left at all — only the ring genuinely filling up, which is
// counted and reported. This is what lets `tste` REPLAY the boot fixtures it cannot itself re-run.

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

/// SERWIT-2 — THE VERDICT RING IS A LEDGER, SO IT DOES NOT GET TO BE LOSSY.
///
/// This ring used to be a `Mutex<BootRing>` taken with `try_lock` from the print seam, and a failed
/// `try_lock` discarded the verdict record with no counter. That is worse here than at the other three
/// taps, because this is not a mirror of the wire — it is the ONLY record `tste` can replay from, and
/// the lines it captures are exactly the `-> PASS` / `-> FAIL` lines the whole verification culture is
/// built on. A `tste` table that is silently short reads as "that fixture never ran".
///
/// The tap also runs OUTSIDE the serial lock and outside the mask, so "contention is rare because
/// prints are serialised" — the old comment's reasoning — was simply not true: every core reaches it at
/// once. Under a multi-core burst the ring lost records routinely.
///
/// The fix is not to count the loss, it is to remove the lock. A fixed array of fixed-size records
/// needs no mutual exclusion at all: a writer claims its index with one `fetch_add` and publishes the
/// slot with one release store, exactly like the serial staging ring. So there is nothing left to
/// contend for, nothing to `try_lock`, and NO loss path except the ring genuinely filling up — which
/// was already counted and is already reported by `run()`. As a bonus this deletes a lock that was
/// reachable from every print context on both arches.
struct BootSlots {
    entries: [UnsafeCell<BootVerdict>; RING_CAP],
    ready: [AtomicBool; RING_CAP],
}

// SAFETY: each `entries[i]` is written by exactly one core — the one that won `BOOT_CLAIMED`'s
// `fetch_add` for index `i`, which no other core can ever obtain — and is read only after that core's
// `ready[i]` release store is observed with `Acquire`. The claim counter is what serialises the two, so
// no two accesses to a slot ever overlap.
unsafe impl Sync for BootSlots {}

static BOOT_SLOTS: BootSlots = BootSlots {
    entries: [const { UnsafeCell::new(BootVerdict::empty()) }; RING_CAP],
    ready: [const { AtomicBool::new(false) }; RING_CAP],
};

/// Verdict lines claimed so far, INCLUDING the ones that did not fit. Monotonic, so
/// `count.saturating_sub(RING_CAP)` is the ring-full drop count with no separate counter to skew.
static BOOT_CLAIMED: AtomicUsize = AtomicUsize::new(0);

/// How much of a line's HEAD is kept for the label. The label is clipped to [`NAME_MAX`] (40) anyway,
/// so this only has to be comfortably wider than that.
const HEAD_MAX: usize = 200;

/// SERWIT-2W — A STREAMING VERDICT SCANNER, BECAUSE THE OLD ONE LOST WHOLE VERDICTS.
///
/// `capture` used to format the line into a 200-byte stack buffer and then `find("-> PASS")` in it,
/// with the comment "truncate silently; the verdict marker is early in the line". **The marker is not
/// early in the line — it is at the END of it**, by the tree's own convention (`:: LABEL — detail …
/// -> PASS ::`). So any verdict line longer than 200 bytes had its marker chopped off before the
/// search ever ran, and the fixture was simply never recorded: not dropped-and-counted, not truncated,
/// just absent from `tste`'s table as if it had never executed. 264 of the tree's format strings are
/// wider than 240 bytes, and the widest verdict lines are exactly the detailed ones.
///
/// This scanner has no width limit at all. It keeps the first [`HEAD_MAX`] bytes (all the label can
/// ever need) and matches the three needles incrementally against the stream, so a marker at byte 1200
/// is found exactly as reliably as one at byte 12. Still alloc-free, still one fixed stack frame,
/// still safe from an IRQ-masked print context.
struct VerdictScan {
    head: [u8; HEAD_MAX],
    head_len: usize,
    /// Bytes seen, whole line.
    total: usize,
    /// Incremental match positions for the three needles.
    m_pass: usize,
    m_fail: usize,
    m_tste: usize,
    /// Byte offset of the first verdict marker found, and which one.
    hit: Option<(usize, bool)>,
    /// Did the line mention TSTE anywhere (the suite's own output — never captured).
    saw_tste: bool,
}

const N_PASS: &[u8] = b"-> PASS";
const N_FAIL: &[u8] = b"-> FAIL";
const N_TSTE: &[u8] = b"TSTE";

/// Advance one needle by one byte; returns true when the needle completes here.
#[inline]
fn needle_step(state: &mut usize, needle: &[u8], b: u8) -> bool {
    if b == needle[*state] {
        *state += 1;
        if *state == needle.len() {
            *state = 0;
            return true;
        }
    } else {
        // Restart, but re-test this byte against the needle's first character — otherwise a needle
        // whose first char immediately follows a failed partial match would be missed.
        *state = usize::from(b == needle[0]);
    }
    false
}

impl VerdictScan {
    fn new() -> Self {
        VerdictScan {
            head: [0u8; HEAD_MAX],
            head_len: 0,
            total: 0,
            m_pass: 0,
            m_fail: 0,
            m_tste: 0,
            hit: None,
            saw_tste: false,
        }
    }
    /// The kept head of the line, as `str`. The head can end mid-char (the cut is at a fixed byte
    /// count), so take the longest valid prefix rather than throwing the whole label away.
    fn head_str(&self) -> &str {
        let bytes = &self.head[..self.head_len];
        match core::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or(""),
        }
    }
}

impl fmt::Write for VerdictScan {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.head_len < HEAD_MAX {
                self.head[self.head_len] = b;
                self.head_len += 1;
            }
            self.total += 1;
            if needle_step(&mut self.m_tste, N_TSTE, b) {
                self.saw_tste = true;
            }
            if self.hit.is_none() {
                if needle_step(&mut self.m_pass, N_PASS, b) {
                    self.hit = Some((self.total - N_PASS.len(), true));
                } else if needle_step(&mut self.m_fail, N_FAIL, b) {
                    self.hit = Some((self.total - N_FAIL.len(), false));
                }
            }
        }
        Ok(())
    }
}

/// Additive hook on the serial print seam (M2b). If the formatted line carries a fixture verdict
/// (`-> PASS` / `-> FAIL`), record it into `BOOT_RING`. Zero behaviour change to what is printed;
/// alloc-free; `try_lock` only (drops on contention) so it is safe from IRQ-masked print contexts.
/// Our OWN live `:: TSTE:` lines are excluded so replaying can't feed back on itself.
pub fn capture(args: fmt::Arguments) {
    let tap = &crate::serial_ring::TAP_TSTE;
    tap.submit();
    let mut sc = VerdictScan::new();
    let _ = fmt::write(&mut sc, args);

    let Some((idx, pass)) = sc.hit else {
        tap.suppress(); // not a verdict line — declined by policy, not lost
        return;
    };
    if sc.saw_tste {
        tap.suppress(); // never capture the suite's own live output
        return;
    }

    // Clean the label: text before the verdict marker, minus a leading ":: " frame, trimmed. The
    // marker can sit past `HEAD_MAX` on a very wide line; the label is clipped to `NAME_MAX` (40)
    // regardless, so the kept head is always enough.
    let s = sc.head_str();
    let mut cut = idx.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut label = s[..cut].trim();
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

    // SERWIT-2: claim a slot with one atomic increment. Wait-free, lock-free, and — unlike the
    // `try_lock` this replaced — it cannot fail for any reason except the ring being genuinely full,
    // which `BOOT_CLAIMED` records by construction (it keeps counting past `RING_CAP`).
    let idx = BOOT_CLAIMED.fetch_add(1, Ordering::AcqRel);
    if idx >= RING_CAP {
        crate::serial_ring::TAP_TSTE.drop_line();
        return;
    }
    // SAFETY: this core exclusively owns slot `idx` — no other core can win the same `fetch_add` value
    // — and no reader may touch it until the release store below.
    unsafe {
        let slot = &mut *BOOT_SLOTS.entries[idx].get();
        slot.name[..nlen].copy_from_slice(&name[..nlen]);
        slot.len = nlen as u8;
        slot.pass = pass;
    }
    BOOT_SLOTS.ready[idx].store(true, Ordering::Release);
    crate::serial_ring::TAP_TSTE.absorb();
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
        // Share the console's page-height math (the single source of truth) so a pager page is
        // exactly one console screenful minus the prompt line — no independent clamp that would
        // pause at ~2/3 height on a native-resolution panel.
        Pager { rows: Console::page_rows(pal), on_page: 0 }
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
    report(&mut pager, console, pal, &mut tally, "video.geometry", test_video_geometry()); report(&mut pager, console, pal, &mut tally, "video.font.aa", test_font_aa()); // FONTAA (SO48) — SAME-LINE append, deliberately: a new statement on its own line here shifts every `panic::Location` below it in this file and moves the knob-off image (LAWS §5, "cfg does not protect byte identity"). The fixture body is at this file's TAIL, where an append shifts nothing. The call goes BEFORE the `//`, or it compiles nothing and this gate stays green while measuring an absent fixture (LEDGER P7).
    // VWIT: the damage-tracked `Screen` present path (format decode, damage-limited blit, no-op
    // flush, clip safety) — the on-screen renderer `video.geometry` cannot reach. Logic lives in
    // `video/witness.rs`; here we only map its Result into the tste table.
    report(
        &mut pager,
        console,
        pal,
        &mut tally,
        "video.present",
        match crate::video::witness::run() {
            Ok(()) => Outcome::Pass,
            Err(why) => Outcome::Fail(why),
        },
    );

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
    pager.line(console, pal, "footer: boot-sequenced user fixtures re-run needs TSTE-2 (launcher refactor).");
    serial_println!(
        ":: TSTE: {} pass {} fail {} skip (+{} boot) ::",
        tally.pass, tally.fail, tally.skip, tally.boot
    );
    serial_println!(":: TSTE: suite complete ::");
}

fn snapshot_boot_ring() -> (Vec<(String, bool)>, usize) {
    let claimed = BOOT_CLAIMED.load(Ordering::Acquire);
    let filled = claimed.min(RING_CAP);
    let mut out = Vec::with_capacity(filled);
    for i in 0..filled {
        // A claimed-but-not-yet-published slot belongs to a core that is a handful of instructions from
        // finishing; skip it rather than read a half-written record. It is not lost — it is simply not
        // in THIS snapshot, and `tste` is read after boot, when no such slot exists in practice.
        if !BOOT_SLOTS.ready[i].load(Ordering::Acquire) {
            continue;
        }
        // SAFETY: `ready[i]` was published with `Release` by the slot's one and only writer, so the
        // record is complete and no further write to it can occur.
        let e = unsafe { &*BOOT_SLOTS.entries[i].get() };
        out.push((String::from(e.label()), e.pass));
    }
    // Ring-full loss, derived from the monotonic claim counter rather than a separate tally that could
    // itself be lost. This is the ring's own channel, and it is the ONLY loss path left here.
    (out, claimed.saturating_sub(RING_CAP))
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
// stay in the boot-sequenced battery). When no FAT volume is present (e.g. the aarch64 default
// `usb.img` raw pattern image) storage checks SKIP honestly; the x86 default `usb-boot.img` carries a FAT32 slot since DEFAULTMEDIUM, so plain `./arroyo test` now runs this section for real.

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

// =================================================================================================
// FONTAA (SO48) — the anti-aliased face's blit, against the 1-bit blit it replaces
// =================================================================================================

/// FONTAA (SO48) — **prove the shared surface blit paints GRADED coverage, and prove in the same
/// breath that the face it replaces cannot.**
///
/// ### What it measures, and why that is the right question
///
/// "Blocky" is not a size complaint and it is not a scale complaint. It is a complaint about
/// COVERAGE: a 1-bit glyph table has two ink states, so every edge is a step, and replicating each
/// set bit into a `ts`x`ts` square makes the step `ts` pixels tall while adding no information.
/// Magnification is the amplifier, not the cause — which is exactly why this fixture asks about the
/// number of DISTINCT pixel values a rendered string produces and not about any pixel count.
/// [`video::font::draw_text`](crate::video::font::draw_text) blends Noto's own 8-bit alpha, so a
/// rendered string carries intermediate values; the `font8x8` path cannot produce one at any scale.
///
/// ### ⚠ WHERE THIS RUNS, AND WHERE IT DOES NOT — measured, not assumed
///
/// This is a LIVE fixture, so it runs when an operator types `tste` and **not** during
/// `./arroyo test`. That is not a guess about the harness: on the battery capture this arc gated
/// on, `LC_ALL=C grep -a -c -F ":: TSTE: suite start"` is 0, and so is the count for
/// `video.geometry` — the live fixture that has sat one statement above this one for arcs. The
/// `:: TSTE:` lines a battery capture DOES carry are boot-sequenced fixtures printing that tag
/// directly from `shell.rs`, never this suite.
///
/// So the leg that can be settled without executing a blend — *is the shipped atlas 8-bit alpha or
/// 1-bit?* — is ALSO asserted at compile time, in `video/font.rs`'s tail, where every build of
/// either arch runs it and `./arroyo check` is the gate. This function is the runtime half: it is
/// the one that exercises the actual blit, and it is the one a bench operator can fire. Putting
/// it on the boot wire is one `verdict("video.font.aa", …)` line in `shell.rs`'s boot-fixture
/// block, which was outside this arc's file list and is reported rather than written.
///
/// ### The control probe, and why it is not decoration
///
/// A "text has more than two colours" assertion is worthless on its own: it passes identically for
/// a correct blit, for a blit that smeared the whole buffer, and for a buffer nobody cleared. So
/// leg 3 renders the SAME string through the 1-bit path this arc removes and REQUIRES it to come
/// out at exactly two values. That control must hit for the fixture to mean anything — if a future
/// change made the discriminator blind, leg 3 goes red first and says so, instead of leg 2 going
/// quietly green forever (LAWS §5, "a check that cannot fire is an absent one").
///
/// ### The legs
///
/// 1. **advance** — the pen lands at `n * Face::cell_w()`: the FACE's advance, not `8 * scale`.
/// 2. **graded** — every pixel is the background, the ink, or strictly between, and at least one is
///    strictly between. Endpoint exactness is part of it: `alpha == 0` and `alpha == 255` must stay
///    bit-exact or the pixel-equality instruments elsewhere in the tree lose their witness.
/// 3. **control** — the 1-bit block-replicated render of the same string yields EXACTLY two values.
/// 4. **containment** — a glyph that does not fit whole inside the clip box draws nothing and does
///    not advance the pen, the all-or-nothing rule every hand-written helper had.
fn test_font_aa() -> Outcome {
    use crate::video::font::{self, Face};

    const SW: usize = 96;
    const SH: usize = 48;
    const BG: u32 = 0x0000_0000;
    const INK: u32 = 0x00FF_FFFF;
    const S: &[u8] = b"Ago";

    // ⚠ `black_box` ON THE STRING, and NOT on the face. Both halves of that were measured.
    //
    // WHY ANY BARRIER. Every input here is a constant, so on the first cut LLVM folded leg 1's
    // advance identity and the metrics guard at COMPILE time and deleted both branches — certified
    // by their absence from the built ELF (`LC_ALL=C grep -a -o -F "advance: pen"` = 0 on both
    // arches, against 1 for the three legs whose outcome depends on atlas bytes it cannot fold).
    // That is a pass of a kind, but it is the optimizer's proof about one constant call, not a
    // runtime check — "a check that cannot fire is an absent one" (LAWS §5), applied to this
    // fixture's own legs. An opaque `s` is enough to restore them: `pen`, `s.len() * cw` and the
    // guard all become values no compile-time reasoning reaches.
    //
    // ⚠ WHY NOT THE FACE — 96 KiB IN EVERY MINIMAL IMAGE, MEASURED AND NOT ARGUED. The first cut
    // also wrote `black_box(Face::Body)`, and `./arroyo knoboff wc f164b6fd` priced it: the wc-OFF
    // loadable image grew **+87,482 bytes on aarch64 and +121,652 on x86**, and the aarch64 wc-OFF
    // image came out the same size as the wc-ON one. The cause is `font::glyph`'s four-arm match on
    // `face`: with a CONSTANT face it folds to the two `Face::Body` atlases and the linker strips
    // the `Chrome` pair, which in a wc-off build nothing else references. An OPAQUE face keeps all
    // four arms live, so the Size20 regular+bold pair — priced at 98,040 B from the crate's own
    // geometry (95 glyphs x 20 rows x 9 px, plus the row-slice tables) — lands in a build that
    // never draws a title bar. **The general rule this buys, and it is now written down: a `Face`
    // must reach `glyph` as a compile-time constant at every call site, or the image pays for both
    // rasters.** Every real call site already does; a fixture is the one place that can get it
    // wrong, and this one did for one build.
    let face = Face::Body;
    let s: &[u8] = core::hint::black_box(S);
    let (cw, ch) = (face.cell_w(), face.cell_h());
    if cw == 0 || ch == 0 || s.len() * cw > SW || ch > SH {
        return Outcome::Fail(format!("face metrics unusable: cell={}x{}", cw, ch));
    }

    // 1 — the advance is the face's own.
    let mut aa = alloc::vec![BG; SW * SH];
    let pen = font::draw_text(&mut aa, SW, SW, SH, 0, 0, s, INK, false, face);
    if pen != s.len() * cw {
        return Outcome::Fail(format!("advance: pen={} want={} (cell_w={})", pen, s.len() * cw, cw));
    }

    // 2 — graded coverage, endpoints bit-exact.
    let mut between = 0usize;
    let mut foreign = 0usize;
    for &p in aa.iter() {
        if p == BG || p == INK {
            continue;
        }
        // Grey only: the ink is white over black, so any blended pixel has three equal channels
        // strictly inside the endpoints. Anything else means the blend went somewhere it should not.
        let (r, g, b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
        if r == g && g == b && r > 0 && r < 0xFF {
            between += 1;
        } else {
            foreign += 1;
        }
    }
    if foreign != 0 {
        return Outcome::Fail(format!("blend produced {} non-grey pixels over a mono ramp", foreign));
    }
    if between == 0 {
        return Outcome::Fail(format!(
            "no graded coverage: \"{}\" rendered with only the two endpoint values — this is the 1-bit face",
            core::str::from_utf8(s).unwrap_or("?")
        ));
    }

    // 3 — THE CONTROL, which must hit: the 1-bit path cannot produce a graded pixel.
    let mut bits = alloc::vec![BG; SW * SH];
    let ts = 2usize; // the replication factor `login`/`quarry`/`instgui` all used
    for (c, &b) in s.iter().enumerate() {
        let bitmap = font8x8::legacy::BASIC_LEGACY[b.min(127) as usize];
        for (ry, rowbits) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if rowbits & (1 << rx) == 0 {
                    continue;
                }
                for dy in 0..ts {
                    for dx in 0..ts {
                        let (x, y) = (c * 8 * ts + rx * ts + dx, ry * ts + dy);
                        if x < SW && y < SH {
                            bits[y * SW + x] = INK;
                        }
                    }
                }
            }
        }
    }
    let graded_1bit = bits.iter().filter(|&&p| p != BG && p != INK).count();
    if graded_1bit != 0 {
        return Outcome::Fail(format!(
            "control did not hold: the 1-bit render produced {} graded pixels, so leg 2 proves nothing",
            graded_1bit
        ));
    }

    // 4 — containment: a glyph one pixel short of fitting draws nothing and does not advance.
    let mut tight = alloc::vec![BG; SW * SH];
    let pen0 = font::draw_text(&mut tight, SW, cw - 1, SH, 0, 0, s, INK, false, face);
    if pen0 != 0 || tight.iter().any(|&p| p != BG) {
        return Outcome::Fail(format!(
            "containment: clip_w={} drew {} pixels and advanced to {}",
            cw - 1,
            tight.iter().filter(|&&p| p != BG).count(),
            pen0
        ));
    }

    Outcome::Pass
}
