//! CHARTER: Kernel — wm (B520 RENDERWEDGE: the render task's waits bounded and named, its watch on a sibling core)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//!
//! RENDERWEDGE (rmbp-ledger B520). Flight 28 (`f28-boot1.log`), from `tests quarry2` at 12:27:53Z to the end of the
//! log: the render task never routed another event (`handler=- render_ms=0.0`, no `:: LAG:`, no `[serialdoor]`)
//! while EHCI keys were still decoded; the `queue=` ages are a stuck key's age at sparse rolls. The window:
//!
//! * 12:27:55 `[trash] store=attrs op=trash path=/home/una/Q2NOTE.MD ok=1` — quarry2's disk leg (`leg_disk`, on
//!   the `shell-job` task) trashed its scratch file. Its next three calls are `trash::entries()` (a `TRASH_QUERY`
//!   over UnaFS — the FIRST non-empty attrs-Trash query of the boot; the Dock's two reads at 12:18 were `full=0`),
//!   `compute_meta` on the Trash and `trash::restore`. No `restore` line, no `:: QUARRY2:`, no `TESTS: ran` ever.
//! * The same `trash()` bumped `TRASH_GEN`; the Dock spawned its `dock-trash` reader (`trash::count()` → the same
//!   query). Its `[dock] trash state` line never printed either.
//! * From that second to the end, EVERY stall line reads `masked_ms=0` (86–270 in the seconds before). `masked_ms`
//!   is the longest IRQ-masked `with_unafs` attempt that ENDED in the second — so for nine minutes no UnaFS attempt
//!   ended anywhere: one began and never released `unafs::MOUNT` (fs/unafs.rs:567, taken at :794 inside
//!   `without_interrupts`, an unbounded masked spin for every later taker). LOCKREG said nothing because its 16
//!   scan lines (`LINE_CAP`) were spent by 12:22:36.
//! * The render task's last finished step was `console-launch` (12:27:55, 457 ms) — the step QUEUESLEEP's cause.md
//!   saw wait 36 s behind a masked UnaFS transaction at 12:23:14 (`handler=console-launch masked_ms=36303`).
//!
//! THE WEDGE: `unafs-mount` — the render task waiting, unbounded and masked, on the UnaFS mount lock that quarry2's
//! disk-leg Trash round trip (or the Dock reader it set off) held and never let go.
//!
//! This file:
//! 1. [`wait_unlocked`] — the render task's acquire of a named lock is BOUNDED: past its bound it prints
//!    `[render] wait-timeout what=<name> ms=<n> holder=<h>` and the caller continues with a refusal (the UnaFS
//!    acquire answers `MountError::Busy`, the honest "nothing was done, retry"). Off the render task it is a no-op.
//! 2. [`run_bounded`] — a fixture leg on its own task with a bounded wait (quarry2's disk leg): on timeout the
//!    caller is told, the leg is abandoned on its task and LOCKREG names what it holds and waits on.
//! 3. The `render-watch` task on a sibling core (SPLASHSTALL's `bootwatch` shape, B510): fed by ONE same-line fold
//!    on lag.rs's `let stall =` line (every second's roll), it prints, once per wedge,
//!    `:: RENDERWEDGE: age_ms=<n> last_handler=<name> holder=<name> -> FAIL ::` when a key or press has waited in the
//!    queue for 5 s and the render task did nothing in the second that saw it (no route, no handler, no composite),
//!    then LOCKREG's own view of the render task (`[lock] task=<tid> holds=[…] waits=<site>`); `-> PASS` once at
//!    each session end (the login generation moves) when none occurred.
//! 4. `tests renderwedge` — quarry2's shape with its precondition deliberately unmet (the lock its leg needs is held
//!    by a holder that does not let go): the bounded job times out, the bounded wait prints its timeout line, the
//!    watch prints its line (`-> SEEN :: fixture=1`, the same formatter — `-> FAIL` would be tallied as this
//!    fixture's own failure by the verdict tap), the holder lets go and the abandoned leg finishes and cleans up.
//!
//! HONEST LIMITS. The watch reads the roll: with no roll (no route and no pass anywhere) a stale suspicion waits for
//! the next one, and a key that sat ≥ 50 ms in a second the render task spent entirely idle starts the clock even
//! if it is routed a moment later (cleared at the next roll that sees the render task work). `holder=` is the
//! UnaFS mount's owner (STACKGUARD2's `lockowner`, scanned over every core's current task — a masked holder cannot
//! be descheduled, so it IS current) or the fixture's holder; for any other lock the `[lock] task=` line that
//! follows names the wait site, and `untracked` is printed — LOCKREG's holder-by-address lookup is private to
//! sync.rs (owed: a `pub fn waits_on(tid)`, STOP-reported). x86 `wc` only; elsewhere every entry is inline/no-op.

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub use imp::{ensure_tests, roll, run_bounded, wait_unlocked, W_UNAFS};

/// A bounded job's outcome.
pub enum Bounded<R> {
    /// The job ran to its end inside the bound.
    Done(R),
    /// The bound passed; the job is abandoned on its own task.
    Timeout(Timeout),
}

/// What a timed-out bounded job left behind: the bound, its task and the UnaFS holder at the timeout.
pub struct Timeout {
    pub ms: u64,
    pub job_tid: u64,
    pub holder: Holder,
}

/// A holder, printed `<lock>:<label>:<tid>@cpu<n>` (LOCKREG's `by=<tid>@cpu<n>` spelling), or `untracked`.
#[derive(Clone, Copy)]
pub struct Holder {
    pub lock: &'static str,
    pub label: &'static str,
    pub tid: u64,
    pub cpu: usize,
}

impl Holder {
    pub const NONE: Holder = Holder { lock: "-", label: "-", tid: 0, cpu: 0 };
}

#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
impl core::fmt::Display for Holder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.tid == 0 {
            return f.write_str("untracked");
        }
        write!(f, "{}:{}:{}@cpu{}", self.lock, self.label, self.tid, self.cpu)
    }
}

#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn run_bounded<R: Send + 'static>(_what: &'static str, f: fn() -> R, _bound_ms: u64) -> Bounded<R> {
    Bounded::Done(f())
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod imp {
    use super::{Bounded, Holder, Timeout};
    use alloc::sync::Arc;
    use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize};

    /// A key/press waiting this long with the render task idle is a wedge.
    pub const WEDGE_MS: u64 = 5_000;
    /// A queued age below this at a roll is ordinary latency, not a suspicion.
    const SUSPECT_US: u64 = 50_000;
    /// The watch's period (SPLASHSTALL's 250 ms).
    const WATCH_MS: u64 = 250;
    /// The render task's bound on the UnaFS mount: twice the longest legitimate masked span flight 28 printed
    /// (`[filetypes] masked_ms=666`, LOGINRENDER's window), rounded up.
    pub const MOUNT_BOUND_MS: u64 = 1_500;
    /// A bounded job's stack (the `shell-job`'s, SHELLTASK: a fixture that went deep on the render stack).
    const JOB_STACK: usize = 64 * 1024;

    /// The named waits ([`wait_unlocked`]'s `what`): index 0 = none.
    pub const WAIT_NAMES: [&str; 3] = ["-", "unafs-mount", "scratch"];
    pub const W_UNAFS: u8 = 1;
    pub const W_SCRATCH: u8 = 2;

    // ── what the roll tells us (lag.rs's one fold) ─────────────────────────────────────────────────────────────
    /// When the oldest unrouted key/press was pushed (arch ms), as the last suspicious roll computed it; 0 = none.
    static QUEUE_SINCE_MS: AtomicU64 = AtomicU64::new(0);
    /// The render task's last named step (index into `lag::SEG_NAMES`).
    static LAST_HANDLER: AtomicU8 = AtomicU8::new(0);
    /// The render task as a roll or a wait on it saw it (0 = not yet).
    static RENDER_TID: AtomicU64 = AtomicU64::new(0);
    static RENDER_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
    // ── the render task's named wait in flight ─────────────────────────────────────────────────────────────────
    static WAIT_WHAT: AtomicU8 = AtomicU8::new(0);
    static WAIT_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
    // ── the watch ──────────────────────────────────────────────────────────────────────────────────────────────
    static WATCHING: AtomicBool = AtomicBool::new(false);
    static FIRED: AtomicBool = AtomicBool::new(false);
    static WEDGES: AtomicU32 = AtomicU32::new(0);
    static SESSION: AtomicU32 = AtomicU32::new(u32::MAX);
    static WATCH_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
    // ── the fixture's feed (same detector, its own clock and holder) ───────────────────────────────────────────
    static FIX_SINCE_MS: AtomicU64 = AtomicU64::new(0);
    static FIX_FIRES: AtomicU32 = AtomicU32::new(0);
    static FIX_AGE_MS: AtomicU64 = AtomicU64::new(0);
    static FIX_HOLDER_TID: AtomicU64 = AtomicU64::new(0);
    static FIX_HOLDER_CPU: AtomicUsize = AtomicUsize::new(0);
    // ── the last bounded job, for naming a holder ──────────────────────────────────────────────────────────────
    static JOB: crate::sync::Mutex<(u64, &'static str)> = crate::sync::Mutex::new((0, "-"));

    fn on_render() -> bool {
        matches!(crate::arch::sched::current_name(), Some(n) if n.starts_with("render"))
    }

    fn this_cpu() -> usize {
        crate::arch::percpu::this_cpu().cpu_index as usize
    }

    fn note_render() {
        if let Some(t) = crate::arch::sched::current_id() {
            RENDER_TID.store(t, Relaxed);
            RENDER_CPU.store(this_cpu(), Relaxed);
        }
    }

    /// A tid's label: the shell job, the last bounded job, the render task.
    fn label_of(tid: u64) -> &'static str {
        if tid != 0 && tid == crate::shelltask::job_tid() {
            return "shell-job";
        }
        if tid != 0 && tid == RENDER_TID.load(Relaxed) {
            return "render";
        }
        if let Some(j) = JOB.try_lock() {
            if j.0 == tid && tid != 0 {
                return j.1;
            }
        }
        "task"
    }

    /// The UnaFS mount's holder: the core whose CURRENT task STACKGUARD2's `lockowner` records as the mount's owner
    /// (the transaction runs masked, so its holder cannot be off-core). `Holder::NONE` when nobody is recorded.
    pub fn unafs_holder() -> Holder {
        for c in 0..crate::arch::gdt::MAX_CPUS {
            if let Some(t) = crate::arch::sched::current_task_id(c) {
                if crate::lockowner::held_by(crate::lockowner::UNAFS, t) {
                    return Holder { lock: "unafs", label: label_of(t), tid: t, cpu: c };
                }
            }
        }
        Holder::NONE
    }

    fn now_us() -> u64 {
        crate::hidstall::now_us() // TSC: advances while this core is masked (the UnaFS acquire is)
    }

    /// The render task's acquire of `m`: spin until it is free, at most the bound for `what`. `true` = free (take
    /// it now); `false` = the bound passed — `[render] wait-timeout what= ms= holder=` is printed and the caller
    /// refuses. Not the render task: `true` at once (every other caller keeps the old unbounded acquire).
    #[inline]
    pub fn wait_unlocked<T: ?Sized>(m: &crate::sync::Mutex<T>, what: u8) -> bool {
        if !m.is_locked() || !on_render() {
            return true;
        }
        wait_free(m, what, MOUNT_BOUND_MS, false)
    }

    /// The bounded spin itself (`force`: the fixture runs it off the render task).
    #[inline(never)]
    fn wait_free<T: ?Sized>(m: &crate::sync::Mutex<T>, what: u8, bound_ms: u64, force: bool) -> bool {
        if !force {
            note_render();
        }
        let t0 = now_us();
        if !force {
            WAIT_WHAT.store(what, Relaxed);
        }
        let ok = loop {
            if !m.is_locked() {
                break true;
            }
            let ms = now_us().saturating_sub(t0) / 1000;
            if ms >= bound_ms {
                break false;
            }
            core::hint::spin_loop();
        };
        if !force {
            WAIT_WHAT.store(0, Relaxed);
        }
        if !ok {
            WAIT_TIMEOUTS.fetch_add(1, Relaxed);
            let h = if what == W_UNAFS { unafs_holder() } else { fix_holder() };
            serial_println!(
                "[render] wait-timeout what={} ms={} holder={} on={} (RENDERWEDGE: the render task continues; the caller refuses)",
                WAIT_NAMES[what as usize % WAIT_NAMES.len()], now_us().saturating_sub(t0) / 1000, h, if force { "fixture" } else { "render" }
            );
        }
        ok
    }

    fn fix_holder() -> Holder {
        match FIX_HOLDER_TID.load(Relaxed) {
            0 => Holder::NONE,
            t => Holder { lock: "scratch", label: "rw-holder", tid: t, cpu: FIX_HOLDER_CPU.load(Relaxed) },
        }
    }

    // ── run_bounded ────────────────────────────────────────────────────────────────────────────────────────────

    struct Shared<R> {
        f: fn() -> R,
        out: crate::sync::Mutex<Option<R>>,
        done: AtomicBool,
        abandoned: AtomicBool,
        tid: AtomicU64,
        what: &'static str,
        t0: u64,
    }

    fn tramp<R: Send + 'static>(arg: usize) {
        // SAFETY: `arg` is the `Arc::into_raw` [`run_bounded`] handed this task — one strong count, ours.
        let s: Arc<Shared<R>> = unsafe { Arc::from_raw(arg as *const Shared<R>) };
        s.tid.store(crate::sync::here_tid(), Release);
        let r = (s.f)();
        *s.out.lock() = Some(r);
        s.done.store(true, Release);
        if s.abandoned.load(Acquire) {
            serial_println!("[renderwedge] late what={} ms={} (the abandoned job reached its end; its result is dropped)", s.what, crate::arch::ms().saturating_sub(s.t0));
        }
    }

    /// The job's core: a worker-pool core that is not the caller's and not the BSP (`shelltask::job_cpu`'s rule).
    fn job_cpu() -> usize {
        let here = this_cpu();
        for n in 0..crate::arch::gdt::MAX_CPUS {
            match crate::arch::smp::worker_cpu(n) {
                Some(c) if c != here && c != 0 => return c,
                Some(_) => continue,
                None => break,
            }
        }
        crate::arch::sched::CPU_AUTO
    }

    /// Run `f` on its own task and wait at most `bound_ms` for it. The caller must be a task that may sleep (a
    /// `tests` fixture on the shell job), never the render task. On timeout the job keeps its task — nothing can
    /// pull it out of a spin — and LOCKREG names what it holds and waits on (`[lock] task=<tid> …`).
    pub fn run_bounded<R: Send + 'static>(what: &'static str, f: fn() -> R, bound_ms: u64) -> Bounded<R> {
        let s = Arc::new(Shared { f, out: crate::sync::Mutex::new(None), done: AtomicBool::new(false), abandoned: AtomicBool::new(false), tid: AtomicU64::new(0), what, t0: crate::arch::ms() });
        let raw = Arc::into_raw(s.clone()) as usize;
        crate::arch::sched::spawn_stack(what, tramp::<R>, raw, job_cpu(), crate::arch::sched::PRIO_NORMAL, JOB_STACK);
        let t0 = crate::arch::ms();
        loop {
            if s.done.load(Acquire) {
                if let Some(r) = s.out.lock().take() {
                    return Bounded::Done(r);
                }
            }
            let ms = crate::arch::ms().saturating_sub(t0);
            let tid = s.tid.load(Acquire);
            if tid != 0 {
                if let Some(mut j) = JOB.try_lock() {
                    *j = (tid, what);
                }
            }
            if ms >= bound_ms {
                s.abandoned.store(true, Release);
                let holder = unafs_holder();
                serial_println!("[renderwedge] job-timeout what={} ms={} job={} holder={} -> abandoned (the caller continues)", what, ms, tid, holder);
                crate::sync::name_task(tid, what); // LOCKREG: what the abandoned job holds and spins on
                return Bounded::Timeout(Timeout { ms, job_tid: tid, holder });
            }
            crate::arch::sched::sleep_ms(5);
        }
    }

    // ── the roll (lag.rs, `let stall =`, every second) ─────────────────────────────────────────────────────────

    /// One second's roll: `queue_us` the oldest unrouted key/press's age, `render_us` the render task's longest
    /// route/handler/composite in the second, `handler` its worst named step. Atomics, then (once) the spawn.
    pub fn roll(queue_us: u64, render_us: u64, handler: &'static str, now_ms: u64) {
        if let Some(i) = crate::video::lag::SEG_NAMES.iter().position(|n| *n == handler) {
            if i != 0 {
                LAST_HANDLER.store(i as u8, Relaxed);
            }
        }
        let here = on_render();
        if here {
            note_render();
        }
        if render_us > 0 || here {
            QUEUE_SINCE_MS.store(0, Relaxed); // the render task worked this second: it is not wedged
        } else if queue_us >= SUSPECT_US {
            let since = now_ms.saturating_sub(queue_us / 1000).max(1);
            let _ = QUEUE_SINCE_MS.compare_exchange(0, since, Relaxed, Relaxed);
        }
        if !WATCHING.load(Relaxed) && crate::boot::phase() == crate::boot::Phase::Desktop {
            watch_start();
        }
    }

    fn watch_start() {
        if WATCHING.swap(true, AcqRel) {
            return;
        }
        // A sibling core: never the render task's (it is the one that may not come back). `sibling_online_cpu`
        // falls back to the caller's core when it is alone.
        let rc = RENDER_CPU.load(Relaxed);
        let cpu = crate::arch::sched::sibling_online_cpu(if rc == usize::MAX { this_cpu() } else { rc });
        WATCH_CPU.store(cpu, Relaxed);
        SESSION.store(crate::prefs::session_gen(), Relaxed);
        crate::arch::sched::spawn("render-watch", watch_task, 0, cpu, crate::arch::sched::PRIO_HIGH);
        serial_println!("[render] watch armed cpu={} render_cpu={} wedge_ms={} (RENDERWEDGE B520)", cpu, if rc == usize::MAX { -1 } else { rc as i64 }, WEDGE_MS);
    }

    fn watch_task(_arg: usize) {
        loop {
            crate::arch::sched::sleep_ms(WATCH_MS);
            look(crate::arch::ms());
        }
    }

    /// The watch line — ONE formatter for the live wedge and the fixture.
    fn say(age_ms: u64, last: &str, holder: Holder, fixture: bool) {
        serial_println!(
            ":: RENDERWEDGE: age_ms={} last_handler={} holder={} -> {} :: wait={} render={}@cpu{} watch_cpu={} wedges={}{}",
            age_ms, last, holder, if fixture { "SEEN" } else { "FAIL" },
            WAIT_NAMES[WAIT_WHAT.load(Relaxed) as usize % WAIT_NAMES.len()],
            RENDER_TID.load(Relaxed), RENDER_CPU.load(Relaxed) as i64, WATCH_CPU.load(Relaxed), WEDGES.load(Relaxed),
            if fixture { " fixture=1" } else { "" }
        );
    }

    /// One look. Never waits.
    fn look(now: u64) {
        let since = QUEUE_SINCE_MS.load(Relaxed);
        if since != 0 && now.saturating_sub(since) >= WEDGE_MS {
            if !FIRED.swap(true, AcqRel) {
                WEDGES.fetch_add(1, Relaxed);
                let w = WAIT_WHAT.load(Relaxed);
                let holder = if w == W_UNAFS || w == 0 { unafs_holder() } else { Holder::NONE };
                let h = LAST_HANDLER.load(Relaxed) as usize;
                say(now.saturating_sub(since), crate::video::lag::SEG_NAMES.get(h).copied().unwrap_or("-"), holder, false);
                let rt = RENDER_TID.load(Relaxed);
                if rt != 0 {
                    crate::sync::name_task(rt, "RENDERWEDGE: the render task, wedged"); // LOCKREG: its holds and the lock it spins on
                }
            }
        } else if since == 0 && FIRED.swap(false, AcqRel) {
            serial_println!("[render] wedge cleared (the render task worked again) wedges={}", WEDGES.load(Relaxed));
        }
        let fs = FIX_SINCE_MS.load(Relaxed);
        if fs != 0 && now.saturating_sub(fs) >= WEDGE_MS && FIX_SINCE_MS.compare_exchange(fs, 0, AcqRel, Relaxed).is_ok() {
            FIX_AGE_MS.store(now.saturating_sub(fs), Relaxed);
            say(now.saturating_sub(fs), "fixture", fix_holder(), true);
            FIX_FIRES.fetch_add(1, Release);
        }
        // A session ends when the login generation moves (login and logout both move it).
        let g = crate::prefs::session_gen();
        let s = SESSION.load(Relaxed);
        if s != u32::MAX && g != s && SESSION.compare_exchange(s, g, AcqRel, Relaxed).is_ok() {
            let n = WEDGES.swap(0, Relaxed);
            serial_println!(
                ":: RENDERWEDGE: wedges={} wait_timeouts={} session={} -> {} :: wedge_ms={} watch_cpu={}",
                n, WAIT_TIMEOUTS.load(Relaxed), s, if n == 0 { "PASS" } else { "FAIL" }, WEDGE_MS, WATCH_CPU.load(Relaxed)
            );
        }
    }

    // ── `tests renderwedge` ────────────────────────────────────────────────────────────────────────────────────

    /// The lock quarry2's shape needs, held by a holder that does not let go (the unmet precondition).
    static SCRATCH: crate::sync::Mutex<u32> = crate::sync::Mutex::new(0);
    static RELEASE: AtomicBool = AtomicBool::new(false);
    static HELD: AtomicBool = AtomicBool::new(false);
    static LEG_CLEANED: AtomicBool = AtomicBool::new(false);
    /// The holder lets go on its own after this, whatever the fixture did (the fixture cannot wedge the machine).
    const HOLD_MAX_MS: u64 = 12_000;
    const LEG_BOUND_MS: u64 = 1_500;
    const WAIT_BOUND_MS: u64 = 300;

    /// `tests renderwedge` registration, once (never at boot, R80).
    pub fn ensure_tests() {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, AcqRel) {
            crate::tests::register("renderwedge", selftest);
        }
    }

    fn holder_task(_arg: usize) {
        let t0 = crate::arch::ms();
        let mut g = SCRATCH.lock();
        FIX_HOLDER_TID.store(crate::sync::here_tid(), Relaxed);
        FIX_HOLDER_CPU.store(this_cpu(), Relaxed);
        HELD.store(true, Release);
        while !RELEASE.load(Acquire) && crate::arch::ms().saturating_sub(t0) < HOLD_MAX_MS {
            crate::arch::sched::sleep_ms(10);
        }
        *g += 1;
        drop(g);
        HELD.store(false, Release);
    }

    /// The leg's scratch, released on every path (quarry2's `Scratch` shape): its drop is the cleanup.
    struct LegScratch;
    impl Drop for LegScratch {
        fn drop(&mut self) {
            LEG_CLEANED.store(true, Release);
        }
    }

    /// quarry2's disk leg, reduced to its shape: take the scratch, then the lock the volume's answer needs.
    fn leg() -> u32 {
        let _s = LegScratch;
        let g = SCRATCH.lock();
        *g
    }

    fn wait_for(bound_ms: u64, mut f: impl FnMut() -> bool) -> bool {
        let t0 = crate::arch::ms();
        loop {
            if f() {
                return true;
            }
            if crate::arch::ms().saturating_sub(t0) >= bound_ms {
                return false;
            }
            crate::arch::sched::sleep_ms(10);
        }
    }

    /// `:: RENDERWEDGE: fixture=1 job=timeout:<ms> wait=timeout:<ms> watch=seen:<age> holder=<h> released=1
    /// cleaned=1 -> PASS ::`.
    fn selftest() {
        if !WATCHING.load(Acquire) {
            watch_start(); // the fixture proves the live watch task, so it must be running
        }
        RELEASE.store(false, Release);
        LEG_CLEANED.store(false, Release);
        FIX_HOLDER_TID.store(0, Relaxed);
        crate::arch::sched::spawn("rw-holder", holder_task, 0, job_cpu(), crate::arch::sched::PRIO_NORMAL);
        if !wait_for(1_000, || HELD.load(Acquire)) {
            RELEASE.store(true, Release);
            serial_println!(":: RENDERWEDGE: fixture=1 -> FAIL :: reason=holder-did-not-start");
            return;
        }
        let holder = fix_holder();
        // 1. quarry2's shape: the leg on a bounded job, its precondition (the lock) held.
        let job = match run_bounded("rw-leg", leg, LEG_BOUND_MS) {
            Bounded::Done(_) => None,
            Bounded::Timeout(t) => Some(t.ms),
        };
        // 2. the render task's bounded wait, the same spin (forced: the fixture is not the render task).
        let t0 = now_us();
        let waited = wait_free(&SCRATCH, W_SCRATCH, WAIT_BOUND_MS, true);
        let wait_ms = now_us().saturating_sub(t0) / 1000;
        // 3. the watch: the fixture's queue clock starts now; the live watch task must print within its bound.
        let f0 = FIX_FIRES.load(Acquire);
        FIX_SINCE_MS.store(crate::arch::ms().max(1), Release);
        let seen = wait_for(WEDGE_MS + 3_000, || FIX_FIRES.load(Acquire) != f0);
        FIX_SINCE_MS.store(0, Release);
        // 4. the holder lets go: the abandoned leg takes the lock, ends, and its scratch is released.
        RELEASE.store(true, Release);
        let released = wait_for(2_000, || !HELD.load(Acquire));
        let cleaned = wait_for(2_000, || LEG_CLEANED.load(Acquire));
        let ok = job.is_some() && !waited && wait_ms >= WAIT_BOUND_MS && seen && released && cleaned;
        serial_println!(
            ":: RENDERWEDGE: fixture=1 job={} wait={}:{} watch={}:{} holder={} released={} cleaned={} -> {} :: job_bound_ms={} wait_bound_ms={} wedge_ms={} watch_cpu={}",
            match job { Some(ms) => alloc::format!("timeout:{}", ms), None => alloc::string::String::from("ran(precondition-met?)") },
            if waited { "free" } else { "timeout" }, wait_ms,
            if seen { "seen" } else { "silent" }, FIX_AGE_MS.load(Relaxed),
            holder, released as u8, cleaned as u8, if ok { "PASS" } else { "FAIL" },
            LEG_BOUND_MS, WAIT_BOUND_MS, WEDGE_MS, WATCH_CPU.load(Relaxed)
        );
        FIX_HOLDER_TID.store(0, Relaxed);
    }
}
