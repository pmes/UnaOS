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

use crate::console::Console;
use crate::pal::TargetPal;

/// The verbs whose line runs on the shell task: console-only verbs that can run long (fixtures, disk, network,
/// diagnostics). Not here, and so on the render task: `clear` and `history` (the view's own state), `selftest`/`tste`
/// (the pager draws), `login`/`logout`/`passwd`/`adduser`/`deluser` (prompts), the window and glass verbs (`view`,
/// `edit`, `dialog`, `activity`, `settings`, `screenshot`, `shot`, `wallpaper`, `top`, `batmon`), the process verbs
/// (`run`, `bg`, `storm`, `jobs`, `kill`, a bare program name) and the power verbs.
pub const ROUTED: &[&str] = &[
    "tests", "census", "prof", "play", "wifi", "linux", "src",
    "ls", "dir", "cat", "type", "head", "tail", "find", "du", "stat", "hexdump", "grep", "wc", "df",
    "touch", "append", "rm", "del", "mkdir", "md", "rmdir", "rd", "cp", "copy", "mv", "move", "ren", "rename",
    "sync", "write", "dd", "fdisk", "lsusb", "setfattr", "getfattr", "query", "file", "assoc", "snap",
    "ifconfig", "ping", "arp", "nc", "curl", "fetch", "dns", "ps", "dmesg", "uptime", "sleep", "burst", "simmer",
];

/// `true` when `line`'s verb runs on the shell task (a `<verb> --help` / `help` line is answered inline either way).
pub fn routed(line: &str) -> bool {
    match line.split_whitespace().next() {
        Some(v) => ROUTED.contains(&v),
        None => false,
    }
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub use imp::{ensure_tests, out, service, submit};

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod imp {
    use super::{routed, Console, TargetPal};
    use alloc::collections::VecDeque;
    use alloc::string::String;
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

    struct Queue {
        busy: bool,
        lines: VecDeque<String>,
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
        crate::arch::sched::spawn_stack("shell-job", job, 0, cpu, crate::arch::sched::PRIO_NORMAL, STACK);
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
            q.lines.push_back(String::from(line));
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
        q.lines.push_back(String::from(line));
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
    fn job(_arg: usize) {
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
            let line = {
                let mut q = QUEUE.lock();
                match q.lines.front() {
                    Some(l) if routed(l) => q.lines.pop_front(),
                    _ => {
                        q.busy = false;
                        None
                    }
                }
            };
            let Some(line) = line else { break };
            let (t0, l0, d0) = (crate::arch::ms(), JOB_LINES.load(Relaxed), DROPPED.load(Relaxed));
            let _ = crate::origin::with(crate::origin::Origin::Door, || crate::shell::dispatch_command(&line, &mut con, &mut pal));
            serial_println!("[shelltask] done verb={} tid={} ms={} lines={} dropped={}", verb(&line), tid,
                crate::arch::ms().saturating_sub(t0), JOB_LINES.load(Relaxed).wrapping_sub(l0), DROPPED.load(Relaxed).wrapping_sub(d0));
        }
        PENDING.store(true, Release);
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
                    Some(l) => {
                        let r = routed(&l);
                        if r {
                            q.busy = true;
                            q.user = console.session.username.clone();
                            q.hist = console.session.history.clone();
                            q.lines.push_front(l.clone());
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
        if idle && !out_left && placed == 0 {
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
