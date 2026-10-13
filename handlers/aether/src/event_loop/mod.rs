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
//!
//! The networking task source (AETHERNET, SR70) is `js::net`: body chunks, socket messages and
//! server-sent events queued by worker threads run as tasks from [`Engine::tick`], the boot drain and
//! [`settle`]. While network operations are in flight and nothing else is runnable, [`settle`] waits on
//! real time (the virtual clock advancing with it), so a page's socket or stream can deliver before the
//! budget runs out; with no operation in flight it behaves exactly as before.
//!
//! AETHERWORKERS (SR123): two more sources. A dedicated worker's messages ride the `js::broadcast` inbox (so
//! they run where every inbox message runs), and while one of the document's workers has work outstanding
//! (`workers::dedicated::busy`) [`settle`] waits on real time for it exactly as it does for network
//! operations. And the idle-callback task source (Cooperative Scheduling of Background Tasks §4):
//! `requestIdleCallback(cb, { timeout })` queues `cb`; [`fire_due_timers`] starts an IDLE PERIOD when its pass
//! fired nothing (no timer due, no browsing-context task) and the page's task queue is empty, with deadline =
//! min(now + [`FRAME_BUDGET_MS`], the next timer's due time), and runs the callbacks queued before the period
//! (a callback queued during it waits for the next) while the deadline holds; a callback whose `timeout`
//! passed runs at the next pass whether or not the loop is idle, with `didTimeout` true and no time
//! remaining. Each callback is its own task (`js::invoke_callback`: report, microtask checkpoint) and gets an
//! IdleDeadline whose `timeRemaining()` is `max(0, deadline − now)` on the page clock. Idle callbacks are NOT
//! counted in [`fire_due_timers`]'s return value: an idle period is idleness, so a callback that re-queues
//! itself every period cannot hold a virtual-time driver past its idle rounds.

use crate::js::Engine;
use js_core::vm::{CallCtx, JsResult, NativeFn, Prop, PropertyKey, Value, Vm, WC};
use std::cell::{Cell, RefCell};
use std::time::Instant;

/// Most timers a single tick will fire; the overflow stays queued and the shortfall is ledgered.
pub const MAX_FIRES_PER_TICK: usize = 256;

/// Bounded boot drain: passes of "advance a frame, run tasks, fire due timers" a page load gets before
/// first layout, and the clock advance per pass.
pub const BOOT_PASSES: u32 = 8;
pub const BOOT_PASS_MS: u64 = FRAME_BUDGET_MS;

/// One frame of the page clock (≈60 Hz): the longest idle period between frames, and the boot drain's pass.
pub const FRAME_BUDGET_MS: u64 = 16;

/// Real milliseconds [`settle`] sleeps per round while it waits on network operations in flight.
pub const NET_WAIT_MS: u64 = 2;

thread_local! {
    /// Monotonic origin for this thread.
    static CLOCK_BASE: Instant = Instant::now();
    /// Virtual time added to the real elapsed time (boot drain frames, tests); signed so a thaw can
    /// rebase the clock under the real one.
    static CLOCK_OFFSET: Cell<i64> = const { Cell::new(0) };
    /// When set, the clock reads exactly CLOCK_OFFSET and only [`advance_clock`] moves it.
    static CLOCK_FROZEN: Cell<bool> = const { Cell::new(false) };
    /// Mirror of the armed timer count (for diagnostics without the VM at hand).
    static ARMED: Cell<usize> = const { Cell::new(0) };
    /// The idle request callback list: (handle, timeout deadline on the page clock), in request order. The
    /// callbacks themselves live in the realm (the prelude's map), traced by its collector.
    static IDLE: RefCell<(u32, Vec<(u32, Option<u64>)>)> = const { RefCell::new((0, Vec::new())) };
}

/// Milliseconds on the page clock.
pub fn now_ms() -> u64 {
    let offset = CLOCK_OFFSET.with(Cell::get);
    if CLOCK_FROZEN.with(Cell::get) {
        offset.max(0) as u64
    } else {
        (CLOCK_BASE.with(|b| b.elapsed().as_millis() as i64) + offset).max(0) as u64
    }
}

/// Moves the clock forward by `ms`.
pub fn advance_clock(ms: u64) {
    CLOCK_OFFSET.with(|o| o.set(o.get().saturating_add(ms as i64)));
}

/// Stops the clock at its current reading; only [`advance_clock`] moves it after this.
pub fn freeze_clock() {
    let now = now_ms();
    CLOCK_FROZEN.with(|f| f.set(true));
    CLOCK_OFFSET.with(|o| o.set(now as i64));
}

/// Lets a frozen clock run again from its current reading (real time resumes from there).
pub fn thaw_clock() {
    if !CLOCK_FROZEN.with(Cell::get) {
        return;
    }
    let now = now_ms() as i64;
    CLOCK_FROZEN.with(|f| f.set(false));
    let elapsed = CLOCK_BASE.with(|b| b.elapsed().as_millis() as i64);
    CLOCK_OFFSET.with(|o| o.set(now - elapsed));
}

/// Whether the clock is frozen.
pub fn clock_frozen() -> bool {
    CLOCK_FROZEN.with(Cell::get)
}

/// Rewinds the clock for a new page (timers die with the previous page's VM; so does the previous
/// page's browsing-context tree — AETHERFRAMES).
pub fn reset() {
    CLOCK_OFFSET.with(|o| o.set(0));
    CLOCK_FROZEN.with(|f| f.set(false));
    ARMED.with(|a| a.set(0));
    IDLE.with(|q| *q.borrow_mut() = (0, Vec::new()));
    crate::js::window::frames::reset();
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
///
/// AETHERFRAMES (SR83): the browsing-context task source runs first (`js::window::frames::service`:
/// new iframes get their contexts, queued nested navigations load, removed iframes' contexts unload).
/// Every driver of the loop — the loader's parse-blocking spins and its "spin until nothing delays the
/// load event", the boot drain, `Engine::tick`, `settle` — reaches it here, so a page's `load` follows
/// its iframes' and a test's virtual-time settle sees them load. Its tasks count in the return value.
pub fn fire_due_timers(vm: &mut Vm) -> usize {
    if crate::js::engine_poisoned() {
        return 0;
    }
    let framed = crate::js::window::frames::service(vm);
    let now = now_ms() as f64;
    // (due, seq, id) snapshot taken before any callback runs.
    let mut due: Vec<(f64, u64, u32)> = vm.timers.iter().filter(|t| t.due <= now).map(|t| (t.due, t.seq, t.id)).collect();
    due.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    if due.len() > MAX_FIRES_PER_TICK {
        crate::ledger::record_js(&format!("timer-fire-cap:{MAX_FIRES_PER_TICK}"));
        due.truncate(MAX_FIRES_PER_TICK);
    }
    let mut ran = framed;
    let idle = due.is_empty() && framed == 0 && crate::js::page(|p| p.tasks.is_empty());
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
        let level = crate::js::timer_level(id, vm.timers.iter().any(|t| t.id == id));
        crate::js::set_timer_nesting(Some(level));
        match &callback {
            Value::String(src) => {
                let _ = crate::js::run_timer_source(vm, &src.to_rust());
            }
            f => {
                crate::js::invoke_callback(vm, "timer-callback", f, &args);
            }
        }
        crate::js::set_timer_nesting(None);
        if crate::js::engine_poisoned() {
            break;
        }
    }
    run_idle_callbacks(vm, idle);
    ran
}

/// The idle-callback pass (see the module docs): an idle period when `idle`, else only the timed-out
/// callbacks. Answers how many ran.
fn run_idle_callbacks(vm: &mut Vm, idle: bool) -> usize {
    let queued: Vec<(u32, Option<u64>)> = IDLE.with(|q| q.borrow().1.clone());
    if queued.is_empty() || crate::js::engine_poisoned() {
        return 0;
    }
    let now = now_ms();
    let mut deadline = now + FRAME_BUDGET_MS;
    if let Some(d) = next_due(vm) {
        deadline = deadline.min((d.max(now as f64)).ceil() as u64);
    }
    let g = vm.realm().global;
    let run = vm.get(g, &PropertyKey::from_str("__idle_run")).unwrap_or(Value::Undefined);
    if !vm.is_callable(&run) {
        return 0;
    }
    let mut ran = 0;
    for (id, timeout) in queued {
        let t = now_ms();
        let timed_out = timeout.is_some_and(|at| t >= at);
        if !timed_out && (!idle || t >= deadline) {
            continue;
        }
        // An earlier callback of this pass may have cancelled it.
        let still = IDLE.with(|q| {
            let mut q = q.borrow_mut();
            match q.1.iter().position(|(h, _)| *h == id) {
                Some(i) => {
                    q.1.remove(i);
                    true
                }
                None => false,
            }
        });
        if !still {
            continue;
        }
        let dl = if timed_out { t } else { deadline };
        crate::js::invoke_callback(vm, "idle-callback", &run, &[Value::Number(id as f64), Value::Number(dl as f64), Value::Bool(timed_out)]);
        ran += 1;
        if crate::js::engine_poisoned() {
            break;
        }
    }
    ran
}

/// `__ric(op, n)`: `add(timeout)` → handle; `cancel(handle)`; `now()` → the page clock (ms).
fn ric_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = vm.arg(ctx, 0);
    let op = vm.to_string(&op)?.to_rust();
    let n = vm.arg(ctx, 1);
    let n = if n.is_nullish() { 0.0 } else { vm.to_number(&n)? };
    match op.as_str() {
        "add" => {
            let timeout = (n.is_finite() && n > 0.0).then(|| now_ms() + n as u64);
            let id = IDLE.with(|q| {
                let mut q = q.borrow_mut();
                q.0 = q.0.wrapping_add(1).max(1);
                let id = q.0;
                q.1.push((id, timeout));
                id
            });
            Ok(Value::Number(id as f64))
        }
        "cancel" => {
            let id = if n.is_finite() && n >= 0.0 { n as u32 } else { 0 };
            IDLE.with(|q| q.borrow_mut().1.retain(|(h, _)| *h != id));
            Ok(Value::Undefined)
        }
        "now" => Ok(Value::Number(now_ms() as f64)),
        _ => vm.throw_type("__ric: unknown operation"),
    }
}

/// Number of idle callbacks queued.
pub fn idle_queued() -> usize {
    IDLE.with(|q| q.borrow().1.len())
}

/// Installs `requestIdleCallback` / `cancelIdleCallback` / `IdleDeadline` in the page realm (replacing the
/// timer-queue shim `api::platform` used to install).
pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__ric", 2, ric_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__ric"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(IDLE_PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("idle-prelude-failed:{}", crate::js::clip(&m, 64)));
        eprintln!("[aether] idle prelude failed: {m}");
    }
}

const IDLE_PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var CB = new Map(), D = new WeakMap(), TOKEN = {};
    class IdleDeadline {
        constructor(token, deadline, didTimeout) {
            if (token !== TOKEN) { throw new TypeError('Illegal constructor'); }
            D.set(this, { deadline: deadline, didTimeout: didTimeout });
        }
        timeRemaining() {
            var d = D.get(this);
            if (!d) { throw new TypeError('Illegal invocation'); }
            return Math.max(0, d.deadline - __ric('now'));
        }
        get didTimeout() {
            var d = D.get(this);
            if (!d) { throw new TypeError('Illegal invocation'); }
            return d.didTimeout;
        }
    }
    Object.defineProperty(IdleDeadline.prototype, Symbol.toStringTag, { value: 'IdleDeadline', configurable: true });
    hide('IdleDeadline', IdleDeadline);
    hide('requestIdleCallback', function requestIdleCallback(callback, options) {
        if (typeof callback !== 'function') {
            throw new TypeError("Failed to execute 'requestIdleCallback' on 'Window': parameter 1 is not of type 'IdleRequestCallback'.");
        }
        var t = options !== undefined && options !== null && options.timeout !== undefined ? Number(options.timeout) : 0;
        var id = __ric('add', t > 0 ? t : 0);
        CB.set(id, callback);
        return id;
    });
    hide('cancelIdleCallback', function cancelIdleCallback(handle) {
        var id = Number(handle);
        if (CB.delete(id)) { __ric('cancel', id); }
    });
    hide('__idle_run', function (id, deadline, didTimeout) {
        var cb = CB.get(id);
        CB.delete(id);
        if (typeof cb === 'function') { cb.call(globalThis, new IdleDeadline(TOKEN, deadline, didTimeout)); }
    });
})();
"#;

/// The bounded boot drain: [`BOOT_PASSES`] passes of "advance a frame, run the queued tasks, fire the
/// timers now due". A page's zero/short-delay boot timers still run before first layout; a boot timer
/// that re-arms itself forever costs the passes, not the load.
pub fn boot_drain(engine: &mut Engine) {
    for _ in 0..BOOT_PASSES {
        if crate::js::engine_poisoned() {
            break;
        }
        advance_clock(BOOT_PASS_MS);
        let tasks = engine.run_tasks() + engine.run_network_tasks();
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
        n += engine.run_network_tasks();
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
        if crate::js::net::active() || crate::workers::dedicated::busy(crate::js::broadcast::doc()) {
            // Network operations in flight, or a dedicated worker with work outstanding (AETHERWORKERS):
            // wait a little real time for them, the page clock keeping pace, so a timer due meanwhile still
            // fires in order.
            let t0 = Instant::now();
            std::thread::sleep(std::time::Duration::from_millis(NET_WAIT_MS));
            advance_clock((t0.elapsed().as_millis() as u64).max(1));
            idle_rounds = 0;
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
