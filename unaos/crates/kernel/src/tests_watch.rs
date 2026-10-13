//! CHARTER: Kernel — kernel-by-ruling (B532 QUEUEWEDGE: a running `tests` verb's task watched from a sibling core — its last stamp, its core, the LOCKREG lock it spins on — named on the wire and the panel; seam: kernel-only)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//!
//! TESTWATCH (rmbp-ledger B532 QUEUEWEDGE, item 2). Flight 30 boot 1 (`f30-boot1.log`, 19:16:55Z): `tests queuesleep`
//! ran on the `shell-job` task (`[shelltask] line verb=tests job=1 cpu=2`), and 15 s later the wire went silent with
//! no witness row — the deadman, RENDERWEDGE's render-watch (it reads an UNROUTED key's age; every door key had been
//! routed) and SESSIONWEDGE's PUMPWEDGE (a stale pump stamp, painted) had nothing to say about a TEST's task. A test
//! that can wedge the kernel ships with its own witness: this file.
//!
//! * [`arm`] / [`disarm`] — `shelltask.rs`'s job arms a slot around every `tests` line it dispatches (and QUEUESLEEP's
//!   own window task arms one for itself); [`stamp`] — a watched task says it made progress (`shelltask::out`, every
//!   transcript line; QUEUESLEEP's window, every 100 ms).
//! * The `tests-watch` task, spawned on the first arm on a SIBLING core of the arming task's
//!   (`sched::sibling_online_cpu`), looks every [`LOOK_MS`] and never waits: for each armed slot, which core runs the
//!   task now (`sched::current_task_id`), the LOCKREG lock it spins on (`sync::waits_on`) and the UnaFS mount's owner
//!   (`lockowner::held_by`). A task seen OFF every core and not spinning on a registered lock has given its core back —
//!   that, or an explicit [`stamp`], moves its `last=`.
//! * Every [`LINE_MS`] while a slot is armed: `[tests] watch verb=<name> task=<tid>@cpu<n> last=<ms> age_ms=<n>
//!   holders=<lockreg summary> state=<run|spin|off>` (`cpu-` when never seen on a core).
//! * Once per wedge, when `age_ms` reaches [`WEDGE_MS`] (the task held its core, or spun on a lock, that long without a
//!   stamp): `:: TESTWATCH: verb=<name> -> WEDGED age_ms=<n> holder=<site> :: task=<tid>@cpu<n> watch_cpu=<n>`, LOCKREG's
//!   own `[lock] task=<tid> holds=[…] waits=…` line (`sync::name_task`), and the same words painted straight onto the
//!   panel (SESSIONWEDGE's PUMPWEDGE band shape, theme `WEDGE_BAND`/`WEDGE_INK`, one band above it) every second while
//!   the wedge lasts — the rMBP's wire rides the usb-pump, the panel does not. `[tests] watch cleared verb=<name>` when
//!   it moves again. `holder=` is `<lock file>:<tid>@cpu<n>` (LOCKREG), `unafs:<tid>@cpu<n>` (the mount's owner when it
//!   is not the watched task), or `own-core@cpu<n>` (running, in no registered wait: a masked spin or a loop).
//!
//! HONEST LIMITS. A task parked forever (a semaphore never posted) is OFF every core and so reads as giving its core
//! back: its line keeps printing with `state=off` and a moving `last=`; this watch calls a wedge only what holds a core
//! or a registered lock. Sampling every 250 ms can miss a task that runs briefly between two looks — that is the case
//! [`stamp`] covers. A `tests` line dispatched INLINE on the render task (a console with no shell window and no door)
//! is not armed. x86 `wc` only (the `shell-job` seam is); `lib.rs` compiles this file nowhere else.

use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize};

/// Watched tasks at once (the shell job's `tests` line, QUEUESLEEP's window, one spare each).
const SLOTS: usize = 4;
/// A verb name's bytes kept for the line.
const NAME_MAX: usize = 24;
/// The look's period (SPLASHSTALL's / RENDERWEDGE's 250 ms).
pub const LOOK_MS: u64 = 250;
/// The `[tests] watch` line's period while a slot is armed.
pub const LINE_MS: u64 = 5_000;
/// No stamp for this long while the task holds a core or spins on a lock: WEDGED.
pub const WEDGE_MS: u64 = 10_000;
/// While a wedge lasts, the panel band is repainted this often (a live compositor may paint over it).
const PAINT_EVERY_MS: u64 = 1_000;

const NO_CPU: usize = usize::MAX;

static TID: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static LAST: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static SEEN_CPU: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(NO_CPU) }; SLOTS];
static NEXT_LINE: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static FIRED: [AtomicBool; SLOTS] = [const { AtomicBool::new(false) }; SLOTS];
static NEXT_PAINT: AtomicU64 = AtomicU64::new(0);
static NAMES: crate::sync::Mutex<[([u8; NAME_MAX], usize); SLOTS]> = crate::sync::Mutex::new([([0; NAME_MAX], 0); SLOTS]);
static WATCHING: AtomicBool = AtomicBool::new(false);
static WATCH_CPU: AtomicUsize = AtomicUsize::new(NO_CPU);
static WEDGES: AtomicU32 = AtomicU32::new(0);
static PAINTED: AtomicBool = AtomicBool::new(false);

fn this_cpu() -> usize {
    crate::arch::percpu::this_cpu().cpu_index as usize
}

fn my_tid() -> u64 {
    match crate::sync::here_tid() {
        0 => crate::arch::sched::current_id().unwrap_or(0),
        t => t,
    }
}

/// The verb's name as the line prints it: the fixture word of a `tests <name>` line (`all` for a bare `tests`), else
/// the line's first word. Pure.
pub fn verb_name(line: &str) -> &str {
    let mut w = line.split_whitespace();
    match w.next() {
        Some("tests") => w.next().unwrap_or("all"),
        Some(v) => v,
        None => "-",
    }
}

/// Watch the calling task while it runs `what` (a `tests` line, or a fixture's own window). Returns at once; the
/// first arm spawns the watch on a sibling core. A full table drops the arm (the line says nothing then).
pub fn arm(what: &str) {
    let tid = my_tid();
    if tid == 0 {
        return;
    }
    let now = crate::arch::ms().max(1);
    let name = verb_name(what);
    for i in 0..SLOTS {
        if TID[i].compare_exchange(0, tid, AcqRel, Relaxed).is_ok() || TID[i].load(Acquire) == tid {
            {
                let mut n = NAMES.lock();
                let b = name.as_bytes();
                let mut k = b.len().min(NAME_MAX);
                while k > 0 && !name.is_char_boundary(k) {
                    k -= 1;
                }
                n[i].0[..k].copy_from_slice(&b[..k]);
                n[i].1 = k;
            }
            LAST[i].store(now, Release);
            SEEN_CPU[i].store(this_cpu(), Relaxed);
            NEXT_LINE[i].store(now + LINE_MS, Relaxed);
            FIRED[i].store(false, Relaxed);
            break;
        }
    }
    if !WATCHING.swap(true, AcqRel) {
        let cpu = crate::arch::sched::sibling_online_cpu(this_cpu());
        WATCH_CPU.store(cpu, Relaxed);
        crate::arch::sched::spawn("tests-watch", watch_task, 0, cpu, crate::arch::sched::PRIO_HIGH);
        serial_println!("[tests] watch armed cpu={} line_ms={} wedge_ms={} (TESTWATCH B532)", cpu, LINE_MS, WEDGE_MS);
    }
}

/// The calling task's watch ends (its `tests` line returned). A slot another task armed is untouched.
pub fn disarm() {
    disarm_task(my_tid());
}

/// Task `tid`'s watch ends (the shell's abort reaped it).
pub fn disarm_task(tid: u64) {
    if tid == 0 {
        return;
    }
    for i in 0..SLOTS {
        if TID[i].load(Acquire) == tid {
            if FIRED[i].swap(false, AcqRel) {
                serial_println!("[tests] watch cleared task={} (the watched verb returned)", tid);
                band_clear();
            }
            TID[i].store(0, Release);
        }
    }
}

/// A watched task made progress (atomics only; a task that is not watched costs one scan of four words).
#[inline]
pub fn stamp() {
    if !WATCHING.load(Relaxed) {
        return;
    }
    let tid = my_tid();
    if tid == 0 {
        return;
    }
    for i in 0..SLOTS {
        if TID[i].load(Relaxed) == tid {
            LAST[i].store(crate::arch::ms().max(1), Relaxed);
        }
    }
}

fn watch_task(_arg: usize) {
    loop {
        crate::arch::sched::sleep_ms(LOOK_MS);
        look(crate::arch::ms());
    }
}

/// The core `tid` runs on now, if any.
fn on_cpu(tid: u64) -> Option<usize> {
    (0..crate::arch::gdt::MAX_CPUS).find(|&c| crate::arch::sched::current_task_id(c) == Some(tid))
}

/// The UnaFS mount's owner, when one is recorded (it runs masked, so it is some core's current task).
fn unafs_owner() -> Option<(u64, usize)> {
    for c in 0..crate::arch::gdt::MAX_CPUS {
        if let Some(t) = crate::arch::sched::current_task_id(c) {
            if crate::lockowner::held_by(crate::lockowner::UNAFS, t) {
                return Some((t, c));
            }
        }
    }
    None
}

/// What the watched task waits on / what holds the machine's one masked lock: LOCKREG first.
fn holders(out: &mut Line, tid: u64, waits: Option<(&'static str, u32, u32)>) {
    use core::fmt::Write;
    let mut any = false;
    if let Some((site, ht, hc)) = waits {
        let _ = write!(out, "{}:{}@cpu{}", site, ht, hc);
        any = true;
    }
    if let Some((t, c)) = unafs_owner() {
        if any {
            let _ = out.write_str(",");
        }
        let _ = write!(out, "unafs:{}@cpu{}{}", t, c, if t == tid { "(self)" } else { "" });
        any = true;
    }
    if !any {
        let _ = out.write_str("none");
    }
}

/// The WEDGED line's one holder word.
fn holder_word(out: &mut Line, tid: u64, cpu: Option<usize>, waits: Option<(&'static str, u32, u32)>) {
    use core::fmt::Write;
    if let Some((site, ht, hc)) = waits {
        let _ = write!(out, "{}:{}@cpu{}", site, ht, hc);
    } else if let Some((t, c)) = unafs_owner().filter(|&(t, _)| t != tid) {
        let _ = write!(out, "unafs:{}@cpu{}", t, c);
    } else if let Some(c) = cpu {
        let _ = write!(out, "own-core@cpu{}", c);
    } else {
        let _ = out.write_str("-");
    }
}

struct CpuWord(usize);
impl core::fmt::Display for CpuWord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == NO_CPU { f.write_str("cpu-") } else { write!(f, "cpu{}", self.0) }
    }
}

/// One look over every armed slot. Never waits (the names are read with `try_lock`; a busy table skips the look).
fn look(now: u64) {
    let names = match NAMES.try_lock() {
        Some(g) => *g,
        None => return,
    };
    for i in 0..SLOTS {
        let tid = TID[i].load(Acquire);
        if tid == 0 {
            continue;
        }
        let cpu = on_cpu(tid);
        let waits = crate::sync::waits_on(tid as u32);
        if let Some(c) = cpu {
            SEEN_CPU[i].store(c, Relaxed);
        }
        if cpu.is_none() && waits.is_none() {
            LAST[i].store(now.max(1), Relaxed); // off every core, in no registered wait: it gave its core back
        }
        let last = LAST[i].load(Relaxed);
        let age = now.saturating_sub(last);
        let name = core::str::from_utf8(&names[i].0[..names[i].1]).unwrap_or("?");
        let state = if waits.is_some() { "spin" } else if cpu.is_some() { "run" } else { "off" };
        let seen = SEEN_CPU[i].load(Relaxed);
        if now >= NEXT_LINE[i].load(Relaxed) {
            NEXT_LINE[i].store(now + LINE_MS, Relaxed);
            let mut h = Line::new();
            holders(&mut h, tid, waits);
            serial_println!("[tests] watch verb={} task={}@{} last={} age_ms={} holders={} state={}", name, tid, CpuWord(seen), last, age, h.as_str(), state);
        }
        if age >= WEDGE_MS && (cpu.is_some() || waits.is_some()) {
            if !FIRED[i].swap(true, AcqRel) {
                WEDGES.fetch_add(1, Relaxed);
                let mut h = Line::new();
                holder_word(&mut h, tid, cpu, waits);
                serial_println!(
                    ":: TESTWATCH: verb={} -> WEDGED age_ms={} holder={} :: task={}@{} watch_cpu={} wedges={} wedge_ms={}",
                    name, age, h.as_str(), tid, CpuWord(seen), WATCH_CPU.load(Relaxed) as i64, WEDGES.load(Relaxed), WEDGE_MS
                );
                crate::sync::name_task(tid, "TESTWATCH: the running test's task, wedged"); // LOCKREG: its holds and the lock it spins on
                band(name, age, h.as_str());
                NEXT_PAINT.store(now + PAINT_EVERY_MS, Relaxed);
            } else if now >= NEXT_PAINT.load(Relaxed) {
                NEXT_PAINT.store(now + PAINT_EVERY_MS, Relaxed);
                let mut h = Line::new();
                holder_word(&mut h, tid, cpu, waits);
                band(name, age, h.as_str());
            }
        } else if age < WEDGE_MS && FIRED[i].swap(false, AcqRel) {
            serial_println!("[tests] watch cleared verb={} task={} (it moved again) wedges={}", name, tid, WEDGES.load(Relaxed));
            band_clear();
        }
    }
}

/// The panel's words for a wedge: `TESTWATCH: verb=<name> -> WEDGED age_ms=<n> holder=<site>`.
fn band(name: &str, age_ms: u64, holder: &str) {
    use core::fmt::Write;
    let mut ln = Line::new();
    let _ = write!(ln, ":: TESTWATCH: verb={} -> WEDGED age_ms={} holder={} ::", name, age_ms, holder);
    paint(ln.as_str());
    PAINTED.store(true, Relaxed);
}

fn band_clear() {
    if PAINTED.swap(false, Relaxed) {
        paint("");
    }
}

/// The panel line, straight onto the framebuffer — no compositor, no pump, no heap (SESSIONWEDGE's `paint`, one band
/// higher so a PUMPWEDGE band and this one never cover each other). A busy `WRITER` skips this paint; empty clears.
fn paint(text: &str) {
    let Some(fbg) = crate::video::WRITER.try_lock() else { return };
    let fb = *fbg;
    drop(fbg);
    if !fb.is_ready() {
        return;
    }
    let i = fb.info();
    let ch = crate::video::font::Face::Chrome.cell_h();
    let band_h = ch * 2;
    if i.width == 0 || i.height < band_h * 6 {
        return;
    }
    let y0 = i.height - band_h * 5;
    fb.fill_rect(0, y0, i.width, band_h, BAND);
    if !text.is_empty() {
        let bytes = text.as_bytes();
        let face = crate::video::text::Face::Chrome;
        let n = crate::video::text::fit(bytes, false, face, i.width);
        let x0 = (i.width - crate::video::text::advance(&bytes[..n], false, face).min(i.width)) / 2;
        let ty = (y0 + (band_h - ch) / 2) as isize;
        let (w, h) = (i.width, i.height);
        let _ = crate::video::text::draw_with(&bytes[..n], false, face, x0, ty, w - x0, INK, &mut |px, py, a| {
            if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h && a >= 96 {
                fb.put_pixel(px as usize, py as usize, INK);
            }
        });
    }
    fb.flush_rect(0, y0, i.width, band_h);
}

const BAND: u32 = crate::video::theme::WEDGE_BAND;
const INK: u32 = crate::video::theme::WEDGE_INK;

/// A bounded, allocation-free line (SESSIONWEDGE's `Line`): what does not fit is dropped.
struct Line {
    b: [u8; 160],
    n: usize,
}
impl Line {
    const fn new() -> Self {
        Line { b: [0; 160], n: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("-")
    }
}
impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let mut take = core::cmp::min(self.b.len() - self.n, s.len());
        while take > 0 && !s.is_char_boundary(take) {
            take -= 1;
        }
        self.b[self.n..self.n + take].copy_from_slice(&s.as_bytes()[..take]);
        self.n += take;
        Ok(())
    }
}
