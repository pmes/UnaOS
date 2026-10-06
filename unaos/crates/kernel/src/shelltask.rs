//! CHARTER: Kernel — kernel-by-ruling (B458 SHELLTASK: the shell's dispatch on its own task, the render task composes only — PERFREVIEW F1, R86/R88)
//!
//! SHELLTASK (rmbp-ledger B458). Flights 24/25: every typed `tests` verb ran `shell::dispatch_command` ON the render
//! task (`main.rs` `handle_key`), and the compositor, the cursor and every queued key waited for it — 21 `[lag] stall
//! … render=handler` seconds, 64.5 s in all. Design: docs/dev/evidence/rmbp-1005/shelltask.md.
//!
//! The shape is DECJOB's: a line whose verb is on [`ROUTED`] is handed to a `shell-job` kernel task (`spawn_stack`,
//! its own [`STACK`] under STACKGUARD2's guard, on a worker-pool core that is neither the render core nor the BSP;
//! every lock here is a `sync::Mutex`, so LOCKREG names it). The transcript is the seam: the task's `Console` is a
//! PRODUCER console (`Console::set_task_out`) whose `println` lands in [`OUT`]; the render pass ([`service`]) drains
//! it into the shell window's console with `try_lock` — it never waits on the shell — and paints. A line that needs
//! the view or the glass (not on [`ROUTED`]) runs on the render task as before. While a job runs, typed lines queue
//! in order; the task runs the routed ones, the render pass the rest when the task is idle.
//!
//! Wire: `[shelltask] line verb=<v> job=<n> cpu=<c> key_us=<n> queued=<q>`, `[shelltask] done verb=<v> tid=<t>
//! ms=<n> lines=<n>`; `tests shelltask` → `:: SHELLTASK: shell_task=<tid> render_stalls=0 key_us=<n> … ::`.
//!
//! SHELLTASK2 (B474, docs/dev/evidence/rmbp-1005/shelltask2.md): the job carries a TEARDOWN-1 kill switch; Ctrl-C /
//! ⌘. ([`interrupt`]) aborts it (a foreground program first: its own switch), the render pass frees what the dead
//! task held the STACKGUARD2 way and keeps the queued lines; DOCKPIN lines go through the task ([`submit_glass`]),
//! their window work POSTED back to the render pass; `run`/`bg`/`jobs`/`kill`/`storm` and a program word run here.
//! Wire: `[shelltask] abort verb=<v> tid=<t> after_ms=<n> locks_released=<n> queued_kept=<q> how=<task|child|abandoned>`.

use crate::console::Console;
use crate::pal::TargetPal;

/// The verbs whose line runs on the shell task: console-only verbs that can run long (fixtures, disk, network,
/// diagnostics). Not here, and so on the render task: `clear` and `history` (the view's own state), `selftest`/`tste`
/// (the pager draws), `login`/`logout`/`passwd`/`adduser`/`deluser` (prompts), the window and glass verbs (`view`,
/// `edit`, `dialog`, `activity`, `settings`, `screenshot`, `shot`, `wallpaper`, `top`, `batmon`), the process verbs
/// and the power verbs. SHELLTASK2 (B474): the process verbs (`run`, `bg`, `storm`, `jobs`, `kill`) and any word that is
/// not a verb (a bare program name, a program path — [`crate::shell::program_word`]) run on the task: their waits
/// (the foreground program, the kill confirm) are the task's own.
pub const ROUTED: &[&str] = &[
    "tests", "census", "prof", "play", "wifi", "linux", "src",
    "ls", "dir", "cat", "type", "head", "tail", "find", "du", "stat", "hexdump", "grep", "wc", "df",
    "touch", "append", "rm", "del", "mkdir", "md", "rmdir", "rd", "cp", "copy", "mv", "move", "ren", "rename",
    "sync", "write", "dd", "fdisk", "lsusb", "setfattr", "getfattr", "query", "file", "assoc", "snap",
    "ifconfig", "ping", "arp", "nc", "curl", "fetch", "dns", "ps", "dmesg", "uptime", "sleep", "burst", "simmer",
    "run", "bg", "jobs", "kill", "storm", // SHELLTASK2 (B474)
];

/// `true` when `line`'s verb runs on the shell task (a `<verb> --help` / `help` line is answered inline either way).
pub fn routed(line: &str) -> bool {
    match line.split_whitespace().next() {
        Some(v) => ROUTED.contains(&v) || crate::shell::program_word(v), // SHELLTASK2 (B474): a program word execs on the task
        None => false,
    }
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub use imp::{ensure_tests, interrupt, job_tid, out, pending, service, submit, submit_glass};

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod imp {
    use super::{routed, Console, TargetPal};
    use alloc::collections::VecDeque;
    use alloc::string::String;
    use alloc::sync::Arc;
    use crate::arch::sched::KillSwitch;
    use alloc::vec::Vec;
    use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

    /// The task's usable stack. The shell ran on the render task's 32 KiB (`RENDER_PATH_STACK_SIZE`); the task
    /// gets twice that, so a fixture that went deep on the render stack has headroom here.
    const STACK: usize = 64 * 1024;
    /// Typed lines held while a job runs.
    const QUEUE_CAP: usize = 16;
    /// Transcript lines held for the render pass; past it the producer waits (bounded), then drops oldest.
    const OUT_CAP: usize = 512;
    /// Lines the render pass places per pass (bounds a frame's work).
    const DRAIN_PER_PASS: usize = 256;
    /// How long a producer waits for the render pass to make room before it drops the oldest line (counted).
    const OUT_WAIT_MS: u64 = 50;

    /// A queued line; `glass` = a DOCKPIN launch (origin Glass; runs on the task, its window work posted back).
    struct Entry {
        line: String,
        glass: bool,
    }
    struct Queue {
        busy: bool,
        lines: VecDeque<Entry>,
        user: String,
        hist: Vec<String>,
    }
    static QUEUE: crate::sync::Mutex<Queue> =
        crate::sync::Mutex::new(Queue { busy: false, lines: VecDeque::new(), user: String::new(), hist: Vec::new() });
    /// The transcript: the task writes, the render pass paints.
    static OUT: crate::sync::Mutex<VecDeque<String>> = crate::sync::Mutex::new(VecDeque::new());
    /// Anything for the render pass to look at (a job live, a line queued, transcript waiting) — the idle pass is
    /// this one relaxed load.
    static PENDING: AtomicBool = AtomicBool::new(false);
    /// The last `shell-job` task's id (0 = never spawned).
    static TID: AtomicU64 = AtomicU64::new(0);
    /// The render task's id, as the render pass last saw it.
    static RENDER_TID: AtomicU64 = AtomicU64::new(0);
    /// Render passes through [`service`] (the render task is alive while a job runs).
    static PASSES: AtomicU64 = AtomicU64::new(0);
    /// The worst Enter-key cost on the render task for a handed-off line (µs).
    static KEY_US_MAX: AtomicU64 = AtomicU64::new(0);
    static JOBS: AtomicU32 = AtomicU32::new(0);
    static DROPPED: AtomicU32 = AtomicU32::new(0);
    static JOB_LINES: AtomicU32 = AtomicU32::new(0);
    // ── SHELLTASK2 (B474) ──
    /// The live job's generation + 1 (0 = no job). A job whose generation was retired (an abort) touches nothing.
    static LIVE: AtomicU32 = AtomicU32::new(0);
    static GEN: AtomicU32 = AtomicU32::new(0);
    /// The live job's kill switch (TEARDOWN-1).
    static KILL: crate::sync::Mutex<Option<Arc<KillSwitch>>> = crate::sync::Mutex::new(None);
    /// The verb the job is running (the abort line's `verb=`).
    static CUR_VERB: crate::sync::Mutex<String> = crate::sync::Mutex::new(String::new());
    /// An abort in flight: the dead task's id (0 = none), when Ctrl-C landed, the generation it retires.
    static ABORT_TID: AtomicU64 = AtomicU64::new(0);
    static ABORT_T0: AtomicU64 = AtomicU64::new(0);
    static ABORT_GEN: AtomicU32 = AtomicU32::new(0);
    static ABORT_VERB: crate::sync::Mutex<String> = crate::sync::Mutex::new(String::new());
    /// Window work a glass line posted back to the render pass.
    static POST: crate::sync::Mutex<VecDeque<String>> = crate::sync::Mutex::new(VecDeque::new());
    static DOCK_LINES: AtomicU32 = AtomicU32::new(0);
    static ABORTS: AtomicU32 = AtomicU32::new(0);
    /// How long the render pass waits for the scheduler to reap an aborted job before it abandons it.
    const ABORT_WAIT_MS: u64 = 2000;

    fn now_us() -> u64 {
        let hz = crate::arch::apic::tsc_hz();
        if hz >= 1_000_000 { crate::arch::now_cycles() / (hz / 1_000_000) } else { crate::arch::ms().saturating_mul(1000) }
    }

    fn verb(line: &str) -> &str {
        line.split_whitespace().next().unwrap_or("")
    }

    /// The job's core: the first worker-pool core that is not the caller's and not the BSP (`play-dec`'s rule);
    /// `CPU_AUTO` only when the pool has none.
    fn job_cpu() -> usize {
        let here = crate::arch::percpu::this_cpu().cpu_index as usize;
        for n in 0..crate::arch::gdt::MAX_CPUS {
            match crate::arch::smp::worker_cpu(n) {
                Some(c) if c != here && c != 0 => return c,
                Some(_) => continue,
                None => break,
            }
        }
        crate::arch::sched::CPU_AUTO
    }

    fn spawn(first: &str, key_us: u64, queued: usize) {
        let cpu = job_cpu();
        JOBS.fetch_add(1, Relaxed);
        let kill = Arc::new(KillSwitch::new());
        *KILL.lock() = Some(kill.clone());
        let jgen = GEN.load(Acquire);
        LIVE.store(jgen.wrapping_add(1), Release);
        let tid = crate::arch::sched::spawn_stack_killable("shell-job", job, jgen as usize, cpu, crate::arch::sched::PRIO_NORMAL, STACK, kill); // SHELLTASK2 (B474): killable
        TID.store(tid, Release);
        serial_println!("[shelltask] line verb={} job={} cpu={} key_us={} queued={}", verb(first), JOBS.load(Relaxed),
            if cpu == crate::arch::sched::CPU_AUTO { -1 } else { cpu as i64 }, key_us, queued);
    }

    /// `main.rs` `handle_key`, the CR arm (the render task): `true` when the shell task took the line — it runs
    /// there, or waits its turn behind the running job — and the caller only repaints. `false`: dispatch inline,
    /// as before. Only the shell WINDOW's console hands lines off; the backdrop console and every other surface
    /// keep the inline path.
    pub fn submit(line: &str, console: &mut Console) -> bool {
        if !console.is_in_window() {
            return false;
        }
        let t0 = now_us();
        let mut q = QUEUE.lock();
        if !q.busy && q.lines.is_empty() {
            if !routed(line) {
                return false;
            }
            q.busy = true;
            q.user = console.session.username.clone();
            q.hist = console.session.history.clone();
            q.lines.push_back(Entry { line: String::from(line), glass: false });
            drop(q);
            PENDING.store(true, Release);
            let key_us = now_us().saturating_sub(t0);
            KEY_US_MAX.fetch_max(key_us, Relaxed);
            spawn(line, key_us, 0);
            return true;
        }
        if q.lines.len() >= QUEUE_CAP {
            drop(q);
            console.println("shell: busy — the running command holds the shell; line not queued");
            return true;
        }
        q.lines.push_back(Entry { line: String::from(line), glass: false });
        let n = q.lines.len();
        drop(q);
        PENDING.store(true, Release);
        let key_us = now_us().saturating_sub(t0);
        KEY_US_MAX.fetch_max(key_us, Relaxed);
        serial_println!("[shelltask] queue verb={} queued={} key_us={} on={}", verb(line), n, key_us, if routed(line) { "task" } else { "render" });
        true
    }

    /// The task console's `println` (`Console::task_out`): append one transcript line. The producer is a task, so
    /// it may wait — bounded by [`OUT_WAIT_MS`] — for the render pass to make room; past that the oldest line goes,
    /// counted (`dropped=` on the `done` line).
    pub fn out(text: &str) {
        let t0 = crate::arch::ms();
        loop {
            {
                let mut o = OUT.lock();
                if o.len() < OUT_CAP || crate::arch::ms().saturating_sub(t0) >= OUT_WAIT_MS {
                    if o.len() >= OUT_CAP {
                        o.pop_front();
                        DROPPED.fetch_add(1, Relaxed);
                    }
                    o.push_back(String::from(text));
                    JOB_LINES.fetch_add(1, Relaxed);
                    PENDING.store(true, Release);
                    return;
                }
            }
            crate::arch::sched::sleep_ms(1);
        }
    }

    /// The `shell-job` task: run the routed lines at the head of the queue, in order, on a throwaway 16x16 pal (the
    /// `witness_capture` shape — a routed verb never draws); stop at an empty queue or a render-bound line.
    fn job(arg: usize) {
        let my_gen = arg as u32;
        let tid = crate::sync::here_tid();
        TID.store(tid, Release);
        let info = unaos_boot_info::FrameBufferInfo {
            width: 16, height: 16, stride: 16, bytes_per_pixel: 4,
            pixel_format: unaos_boot_info::PixelFormat::Bgr,
        };
        let mut store = alloc::vec![0u8; 16 * 16 * 4];
        let mut fb = crate::video::FrameBuffer::new();
        fb.init(store.as_mut_ptr() as usize, store.len(), info);
        let mut screen = crate::video::Screen::direct(fb);
        let mut pal = TargetPal { surface: &mut screen };
        let mut con = Console::new();
        con.set_task_out();
        con.mark_in_window();
        {
            let q = QUEUE.lock();
            con.session.username = q.user.clone();
            con.session.history = q.hist.clone();
        }
        loop {
            let entry = {
                let mut q = QUEUE.lock();
                if GEN.load(Acquire) != my_gen {
                    None // SHELLTASK2: this job was aborted and abandoned — the shell is someone else's now
                } else {
                    match q.lines.front() {
                        Some(e) if e.glass || routed(&e.line) => q.lines.pop_front(),
                        _ => {
                            q.busy = false;
                            None
                        }
                    }
                }
            };
            let Some(Entry { line, glass }) = entry else { break };
            if glass && !routed(&line) {
                // SHELLTASK2 (B474): a dock launch that opens a kernel window — the window is the render pass's.
                POST.lock().push_back(line.clone());
                PENDING.store(true, Release);
                serial_println!("[shelltask] post verb={} -> render (window)", verb(&line));
                continue;
            }
            if let Some(mut v) = CUR_VERB.try_lock() {
                v.clear();
                v.push_str(verb(&line));
            }
            let (t0, l0, d0) = (crate::arch::ms(), JOB_LINES.load(Relaxed), DROPPED.load(Relaxed));
            let origin = if glass { crate::origin::Origin::Glass } else { crate::origin::Origin::Door };
            let _ = crate::origin::with(origin, || crate::shell::dispatch_command(&line, &mut con, &mut pal));
            serial_println!("[shelltask] done verb={} tid={} ms={} lines={} dropped={}", verb(&line), tid,
                crate::arch::ms().saturating_sub(t0), JOB_LINES.load(Relaxed).wrapping_sub(l0), DROPPED.load(Relaxed).wrapping_sub(d0));
        }
        PENDING.store(true, Release);
        let _ = LIVE.compare_exchange(my_gen.wrapping_add(1), 0, AcqRel, Acquire);
        drop(store);
    }

    /// The render pass (`x86_render_service`, beside `S_SHELL`, while the shell window exists): place the
    /// transcript the task wrote, then — the task idle — start the next queued line (a routed one on a fresh task,
    /// a render-bound one inline, as `handle_key` would have). `true` when the shell window was repainted.
    pub fn service(console: &mut Console, pal: &mut TargetPal) -> bool {
        PASSES.fetch_add(1, Relaxed);
        if !PENDING.load(Acquire) {
            return false;
        }
        RENDER_TID.store(crate::sync::here_tid(), Relaxed);
        let mut placed = 0usize;
        if ABORT_TID.load(Acquire) != 0 {
            placed += abort_poll(console); // SHELLTASK2 (B474): an abort in flight — reaped? free its locks, free the shell
        }
        let post = POST.try_lock().and_then(|mut p| p.pop_front());
        if let Some(l) = post {
            serial_println!("[shelltask] render verb={} (posted window work)", verb(&l));
            let _ = crate::origin::with(crate::origin::Origin::Glass, || crate::shell::dispatch_command(&l, console, pal));
            console.drain_output();
            placed += 1;
        }
        let out_left;
        if let Some(mut o) = OUT.try_lock() {
            while placed < DRAIN_PER_PASS {
                match o.pop_front() {
                    Some(l) => {
                        console.place_from_task(&l);
                        placed += 1;
                    }
                    None => break,
                }
            }
            out_left = !o.is_empty();
        } else {
            out_left = true;
        }
        let mut idle = false;
        let mut next: Option<(String, bool)> = None;
        if let Some(mut q) = QUEUE.try_lock() {
            if !q.busy {
                match q.lines.pop_front() {
                    Some(e) => {
                        let r = e.glass || routed(&e.line);
                        let l = e.line.clone();
                        if r {
                            q.busy = true;
                            q.user = console.session.username.clone();
                            q.hist = console.session.history.clone();
                            q.lines.push_front(e);
                        }
                        next = Some((l, r));
                    }
                    None => idle = true,
                }
            }
        }
        match next {
            Some((l, true)) => spawn(&l, 0, 0),
            Some((l, false)) => {
                serial_println!("[shelltask] render verb={} (render-bound, after the job)", verb(&l));
                let _ = crate::origin::with(crate::origin::Origin::Door, || crate::shell::dispatch_command(&l, console, pal));
                console.drain_output();
                placed += 1;
            }
            None => {}
        }
        let post_left = POST.try_lock().map(|p| !p.is_empty()).unwrap_or(true);
        if idle && !out_left && !post_left && placed == 0 && ABORT_TID.load(Acquire) == 0 {
            PENDING.store(false, Release);
            // A producer that raced the clear re-arms on its own store; re-check the transcript once.
            if OUT.try_lock().map(|o| !o.is_empty()).unwrap_or(true) {
                PENDING.store(true, Release);
            }
        }
        if placed > 0 {
            console.draw(pal);
        }
        placed > 0
    }

    pub fn ensure_tests() {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, AcqRel) {
            crate::tests::register("shelltask", selftest);
            crate::tests::register("shelltask2", selftest2); // SHELLTASK2 (B474)
        }
    }

    /// `tests shelltask` (R80: on demand). It runs ON the shell task (a typed `tests` line is routed), so it holds
    /// its core for 1.5 s the way a long verb does, then waits out the next `[lag]` second: the render task must
    /// have kept passing (`render_passes` > 0) with no render-handler stall second (`render_stalls=0`), and the
    /// Enter key's cost on the render task must be under R86's 1 ms.
    fn selftest() {
        let tid = crate::sync::here_tid();
        let shell = TID.load(Acquire);
        if shell == 0 || tid != shell {
            serial_println!(":: SHELLTASK: shell_task=none here_tid={} -> SKIP reason=not-on-shell-task (type `tests shelltask` in the shell window) ::", tid);
            return;
        }
        let (s0, p0) = (crate::video::lag::handler_stalls(), PASSES.load(Relaxed));
        let t0 = crate::arch::ms();
        let mut spin = 0u64;
        while crate::arch::ms().saturating_sub(t0) < 1500 {
            spin = spin.wrapping_add(1);
            core::hint::spin_loop();
        }
        crate::arch::sched::sleep_ms(1200);
        let (s1, p1) = (crate::video::lag::handler_stalls(), PASSES.load(Relaxed));
        let render = RENDER_TID.load(Relaxed);
        let key_us = KEY_US_MAX.load(Relaxed);
        let stalls = s1.wrapping_sub(s0);
        let passes = p1.wrapping_sub(p0);
        let ok = stalls == 0 && passes > 0 && render != tid && key_us < 1000;
        serial_println!(
            ":: SHELLTASK: shell_task={} render_tid={} render_stalls={} render_passes={} key_us={} jobs={} spin={} -> {} ::",
            tid, render, stalls, passes, key_us, JOBS.load(Relaxed), spin, if ok { "PASS" } else { "FAIL" }
        );
    }

    // ══ SHELLTASK2 (rmbp-ledger B474) — the abort, the dock seam, `tests shelltask2` ══════════════════════

    /// Free what a dead (reaped) task held, the STACKGUARD2 order: the named three (`lockowner`: sink, UART, UnaFS
    /// mount) first, then every lock LOCKREG holds in its name. The count of locks given back.
    fn free_dead(tid: u64, why: &str) -> usize {
        let named = crate::arch::stackguard::release_held(tid).count_ones() as usize;
        named + crate::sync::release_task(tid, why)
    }

    /// Ctrl-C (`shellux` 0x03) / ⌘. (`Action::Interrupt`) on the render task: abort the shell window's running job.
    /// A foreground program first — its own kill switch, the job returns by itself; else the job task's switch,
    /// confirmed by [`abort_poll`] on the next passes. `false` when no job runs (the key cancels the line as before).
    pub fn interrupt(console: &mut Console) -> bool {
        if !console.is_in_window() || LIVE.load(Acquire) == 0 {
            return false;
        }
        console.println("^C");
        if ABORT_TID.load(Acquire) != 0 {
            return true; // already aborting
        }
        let tid = TID.load(Acquire);
        let v = CUR_VERB.try_lock().map(|v| v.clone()).unwrap_or_else(|| String::from("?"));
        if crate::arch::syscall::fg_interrupt() {
            let q = QUEUE.try_lock().map(|q| q.lines.len()).unwrap_or(0);
            serial_println!("[shelltask] abort verb={} tid={} after_ms=0 locks_released=0 queued_kept={} how=child", v, tid, q);
            ABORTS.fetch_add(1, Relaxed);
            return true;
        }
        let Some(k) = KILL.try_lock().and_then(|k| k.clone()) else { return true };
        if let Some(mut a) = ABORT_VERB.try_lock() {
            a.clear();
            a.push_str(&v);
        }
        ABORT_GEN.store(LIVE.load(Acquire).wrapping_sub(1), Release);
        ABORT_T0.store(crate::arch::ms(), Release);
        ABORT_TID.store(tid.max(1), Release);
        k.request();
        PENDING.store(true, Release);
        true
    }

    /// The render pass while an abort is in flight: reaped (or ended by itself) → free its locks, place what it
    /// wrote, retire its generation, keep the queued lines, free the shell; not reaped in [`ABORT_WAIT_MS`] (parked
    /// where no kill boundary reaches) → NAME it and abandon it (its kill stays armed). Lines placed.
    fn abort_poll(console: &mut Console) -> usize {
        let tid = ABORT_TID.load(Acquire);
        let jgen = ABORT_GEN.load(Acquire);
        let after = crate::arch::ms().saturating_sub(ABORT_T0.load(Acquire));
        let reaped = KILL.try_lock().and_then(|k| k.clone()).map(|k| k.is_reaped()).unwrap_or(false)
            || LIVE.load(Acquire) != jgen.wrapping_add(1);
        if !reaped && after < ABORT_WAIT_MS {
            return 0;
        }
        let (how, released) = if reaped { ("task", free_dead(tid, "shelltask abort")) } else {
            crate::sync::name_task(tid, "shelltask abort: not reaped");
            ("abandoned", 0)
        };
        let Some(mut q) = QUEUE.try_lock() else { return 0 }; // an abandoned job holds it this instant: next pass
        GEN.fetch_add(1, AcqRel);
        q.busy = false;
        let kept = q.lines.len();
        drop(q);
        let _ = LIVE.compare_exchange(jgen.wrapping_add(1), 0, AcqRel, Acquire);
        if let Some(mut k) = KILL.try_lock() {
            *k = None;
        }
        let mut placed = 0usize;
        if let Some(mut o) = OUT.try_lock() {
            while let Some(l) = o.pop_front() {
                console.place_from_task(&l);
                placed += 1;
            }
        }
        let v = ABORT_VERB.try_lock().map(|v| v.clone()).unwrap_or_default();
        ABORT_TID.store(0, Release);
        ABORTS.fetch_add(1, Relaxed);
        serial_println!("[shelltask] abort verb={} tid={} after_ms={} locks_released={} queued_kept={} how={}", v, tid, after, released, kept, how);
        PENDING.store(true, Release);
        placed + 1
    }

    /// `main.rs`'s DOCKPIN arm (render task): a pinned app's verb line (or Quarry's `run <path>`) goes through the
    /// shell task's queue, origin Glass. A program runs on the task; a kernel-window verb is posted back to the
    /// render pass by the task. `false`: not taken (no shell window, queue full) — the caller dispatches inline.
    pub fn submit_glass(line: &str, console: &mut Console) -> bool {
        if !console.is_in_window() {
            return false;
        }
        let mut q = QUEUE.lock();
        if q.lines.len() >= QUEUE_CAP {
            return false;
        }
        let idle = !q.busy && q.lines.is_empty();
        q.lines.push_back(Entry { line: String::from(line), glass: true });
        let n = q.lines.len();
        if idle {
            q.busy = true;
            q.user = console.session.username.clone();
            q.hist = console.session.history.clone();
        }
        drop(q);
        PENDING.store(true, Release);
        DOCK_LINES.fetch_add(1, Relaxed);
        serial_println!("[shelltask] dockpin verb={} -> task queued={}", verb(line), n);
        if idle {
            spawn(line, 0, 0);
        }
        true
    }

    static FIX_LOCK: crate::sync::Mutex<u32> = crate::sync::Mutex::new(0);
    static FIX_HELD: AtomicBool = AtomicBool::new(false);

    /// The fixture's wedge: take a lock and never let it go (the DECJOBHANG shape — a job that never returns).
    fn wedge(_arg: usize) {
        let g = FIX_LOCK.lock();
        FIX_HELD.store(true, Release);
        let mut n = 0u64;
        loop {
            n = n.wrapping_add(1);
            core::hint::spin_loop();
            if n == u64::MAX {
                break;
            }
        }
        drop(g);
    }

    /// `tests shelltask2` (R80: on demand). A scratch `shell-job`-shaped task (killable, own stack, a worker core)
    /// takes a lock and spins; the fixture aborts it through the same switch and the same [`free_dead`] the
    /// render pass uses, then reads: reaped (`abort=ok`), the lock free (`locks=0`), the shell queue untouched
    /// (`queued_kept`), the dock arm and `run` routed to the task.
    fn selftest2() {
        FIX_HELD.store(false, Release);
        let before = QUEUE.lock().lines.len();
        let kill = Arc::new(KillSwitch::new());
        let tid = crate::arch::sched::spawn_stack_killable("shelltask2-wedge", wedge, 0, job_cpu(), crate::arch::sched::PRIO_NORMAL, 16 * 1024, kill.clone());
        let t0 = crate::arch::ms();
        while !FIX_HELD.load(Acquire) && crate::arch::ms().saturating_sub(t0) < 1000 {
            crate::arch::sched::sleep_ms(2);
        }
        let held = FIX_HELD.load(Acquire);
        let ta = crate::arch::ms();
        kill.request();
        while !kill.is_reaped() && crate::arch::ms().saturating_sub(ta) < ABORT_WAIT_MS {
            crate::arch::sched::sleep_ms(2);
        }
        let reaped = kill.is_reaped();
        let after = crate::arch::ms().saturating_sub(ta);
        let released = if reaped { free_dead(tid, "tests shelltask2") } else { 0 };
        let locks = if FIX_LOCK.try_lock().is_some() { 0 } else { 1 };
        let kept = QUEUE.lock().lines.len();
        serial_println!("[shelltask] abort verb=wedge tid={} after_ms={} locks_released={} queued_kept={} how=fixture", tid, after, released, kept);
        let run_task = routed("run /apps/ELFHELLO.ELF") && routed("/apps/LUMEN.ELF");
        let ok = held && reaped && released >= 1 && locks == 0 && kept == before && run_task;
        serial_println!(
            ":: SHELLTASK2: abort={} locks={} queued_kept={} dockpin=task dock_lines={} run={} aborts={} -> {} ::",
            if reaped { "ok" } else if !held { "no-wedge" } else { "not-reaped" }, locks, kept, DOCK_LINES.load(Relaxed),
            if run_task { "task" } else { "render" }, ABORTS.load(Relaxed), if ok { "PASS" } else { "FAIL" }
        );
    }

    // ── DOORHEADLESS (B487) — the door's headless console reads the seam ──
    /// Anything for a render pass to place or start (the idle pass is this load).
    pub fn pending() -> bool {
        PENDING.load(Acquire)
    }
    /// The last `shell-job` task's id (0 = never spawned).
    pub fn job_tid() -> u64 {
        TID.load(Acquire)
    }
}

/// Every other build: the shell stays on the caller (no shell window to hand off from).
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn submit(_line: &str, _console: &mut Console) -> bool {
    false
}

#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn out(text: &str) {
    let _ = text;
}

#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn service(_console: &mut Console, _pal: &mut TargetPal) -> bool {
    false
}

#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn ensure_tests() {}

#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn interrupt(_console: &mut Console) -> bool {
    false
}

#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn submit_glass(_line: &str, _console: &mut Console) -> bool {
    false
}
