//! Aether's event loop: the page clock and the timer task source.
//!
//! The timers' callbacks and arguments live in the js_core VM (`vm.timers`, traced by its garbage
//! collector); the *schedule* is this module's: every due time is on the page clock below, and
//! [`fire_due_timers`] is the timer task source of HTML §8.6.
//!
//! Boundedness by construction (the incident that shaped this module: a `setTimeout` callback that
//! re-arms itself used to hold a tick forever):
//!
//! - [`fire_due_timers`] snapshots the due set **before running any callback**. A timer armed (or
//!   re-armed) by a callback is due after the snapshot was taken, so the tick that armed it cannot fire
//!   it: each tick fires at most one generation, and at most [`MAX_FIRES_PER_TICK`] of it.
//! - an interval re-arms from its fire time, so a queue that fell behind never bursts to catch up;
//! - every task runs under js_core's instruction budget (`js::TASK_BUDGET`), and the microtask
//!   checkpoint after it is bounded too.

use crate::js::Engine;
use js_core::vm::{Value, Vm};
use std::cell::Cell;
use std::time::Instant;

/// Most timers a single tick will fire; the overflow stays queued and the shortfall is ledgered.
pub const MAX_FIRES_PER_TICK: usize = 256;

/// Bounded boot drain: passes of "advance a frame, run tasks, fire due timers" a page load gets before
/// first layout, and the clock advance per pass.
pub const BOOT_PASSES: u32 = 8;
pub const BOOT_PASS_MS: u64 = 16;

thread_local! {
    /// Monotonic origin for this thread.
    static CLOCK_BASE: Instant = Instant::now();
    /// Virtual time added to the real elapsed time (boot drain frames, tests).
    static CLOCK_OFFSET: Cell<u64> = const { Cell::new(0) };
    /// When set, the clock reads exactly CLOCK_OFFSET and only [`advance_clock`] moves it.
    static CLOCK_FROZEN: Cell<bool> = const { Cell::new(false) };
    /// Mirror of the armed timer count (for diagnostics without the VM at hand).
    static ARMED: Cell<usize> = const { Cell::new(0) };
}

/// Milliseconds on the page clock.
pub fn now_ms() -> u64 {
    let offset = CLOCK_OFFSET.with(Cell::get);
    if CLOCK_FROZEN.with(Cell::get) {
        offset
    } else {
        CLOCK_BASE.with(|b| b.elapsed().as_millis() as u64) + offset
    }
}

/// Moves the clock forward by `ms`.
pub fn advance_clock(ms: u64) {
    CLOCK_OFFSET.with(|o| o.set(o.get().saturating_add(ms)));
}

/// Stops the clock at its current reading; only [`advance_clock`] moves it after this.
pub fn freeze_clock() {
    let now = now_ms();
    CLOCK_FROZEN.with(|f| f.set(true));
    CLOCK_OFFSET.with(|o| o.set(now));
}

/// Rewinds the clock for a new page (timers die with the previous page's VM).
pub fn reset() {
    CLOCK_OFFSET.with(|o| o.set(0));
    CLOCK_FROZEN.with(|f| f.set(false));
    ARMED.with(|a| a.set(0));
}

/// Number of timers currently armed.
pub fn armed_count() -> usize {
    ARMED.with(Cell::get)
}

pub fn set_armed(n: usize) {
    ARMED.with(|a| a.set(n));
}

/// The due time of the earliest armed timer (virtual-time drivers jump to it).
pub fn next_due(vm: &Vm) -> Option<f64> {
    vm.timers.iter().map(|t| t.due).fold(None, |a: Option<f64>, d| Some(a.map_or(d, |a| a.min(d))))
}

/// Fires every timer due at the current clock reading — and nothing else — each as its own task
/// (callback, then a microtask checkpoint). Returns how many ran.
pub fn fire_due_timers(vm: &mut Vm) -> usize {
    if crate::js::engine_poisoned() {
        return 0;
    }
    let now = now_ms() as f64;
    // (due, seq, id) snapshot taken before any callback runs.
    let mut due: Vec<(f64, u64, u32)> = vm.timers.iter().filter(|t| t.due <= now).map(|t| (t.due, t.seq, t.id)).collect();
    due.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    if due.len() > MAX_FIRES_PER_TICK {
        crate::ledger::record_js(&format!("timer-fire-cap:{MAX_FIRES_PER_TICK}"));
        due.truncate(MAX_FIRES_PER_TICK);
    }
    let mut ran = 0;
    for (_, _, id) in due {
        // A callback earlier in this generation may have cleared this one.
        let Some(i) = vm.timers.iter().position(|t| t.id == id) else { continue };
        let (callback, args) = match vm.timers[i].interval {
            Some(period) => {
                // Re-arm from the fire time: a fallen-behind interval fires once, not a burst.
                let seq = vm.timers[i].seq;
                vm.timers[i].due = now + period;
                vm.timers[i].seq = seq + (1u64 << 32);
                (vm.timers[i].callback.clone(), vm.timers[i].args.clone())
            }
            None => {
                let t = vm.timers.remove(i);
                (t.callback, t.args)
            }
        };
        set_armed(vm.timers.len());
        ran += 1;
        match &callback {
            Value::String(src) => {
                let _ = crate::js::run_timer_source(vm, &src.to_rust());
            }
            f => {
                crate::js::invoke_callback(vm, "timer-callback", f, &args);
            }
        }
        if crate::js::engine_poisoned() {
            break;
        }
    }
    ran
}

/// The bounded boot drain: [`BOOT_PASSES`] passes of "advance a frame, run the queued tasks, fire the
/// timers now due". A page's zero/short-delay boot timers still run before first layout; a boot timer
/// that re-arms itself forever costs the passes, not the load.
pub fn boot_drain(engine: &mut Engine) {
    for _ in 0..BOOT_PASSES {
        if crate::js::engine_poisoned() {
            break;
        }
        advance_clock(BOOT_PASS_MS);
        let tasks = engine.run_tasks();
        let fired = fire_due_timers(&mut engine.vm);
        engine.checkpoint();
        if tasks == 0 && fired == 0 && engine.vm.timers.is_empty() && !engine.has_tasks() {
            break;
        }
    }
}

/// Runs the page's event loop in virtual time until it is idle or `budget_ms` of page time has passed:
/// tasks, then timers in due order (the clock jumps to the next due time), then animation frames —
/// what Chromium's `--virtual-time-budget` does for `--dump-dom`.
pub fn settle(engine: &mut Engine, budget_ms: u64) {
    freeze_clock();
    let start = now_ms();
    let mut idle_rounds = 0;
    loop {
        if crate::js::engine_poisoned() || now_ms() > start + budget_ms {
            break;
        }
        let mut n = engine.run_tasks();
        n += fire_due_timers(&mut engine.vm);
        engine.checkpoint();
        n += engine.drain_raf_once();
        if n > 0 {
            idle_rounds = 0;
            continue;
        }
        if engine.has_tasks() {
            continue;
        }
        match next_due(&engine.vm) {
            Some(due) => {
                let now = now_ms();
                let target = due.ceil().max(now as f64) as u64;
                if target > start + budget_ms {
                    break;
                }
                advance_clock(target.saturating_sub(now).max(1));
            }
            None => {
                idle_rounds += 1;
                if idle_rounds > 2 {
                    break;
                }
                advance_clock(16);
            }
        }
    }
}
