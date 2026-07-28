//! Aether's event loop: a real timer queue and a bounded job executor.
//!
//! # Why this exists
//!
//! The first cut wired `setTimeout` straight onto boa's job queue and ignored
//! the delay: every callback became an immediate job, and
//! `SimpleJobExecutor::run_jobs` runs *until the queue is empty*. A page whose
//! timer callback re-arms `setTimeout` — every poll loop on the web — therefore
//! refilled the queue faster than the drain emptied it, and one `tick()` never
//! returned. The engine thread pegged, the shell's `select!` loop starved, and
//! navigation went dead (observed live on google search).
//!
//! The fix is structural, not a heuristic: timers no longer live on the job
//! queue at all.
//!
//! - The **callbacks** live in the JS global array `__timers`, mirrored by
//!   index (the same discipline as `__handlers`): a GC object must never sit in
//!   a Rust `thread_local`, or it outlives the boa context.
//! - The **schedule** lives in a Rust `thread_local` queue: `(id, slot, due_ms,
//!   interval)` records, ordered by a monotonic millisecond clock.
//! - [`fire_due_timers`] snapshots the due set **before running any callback**.
//!   A timer armed (or re-armed) by a callback lands in the queue *after* the
//!   snapshot was taken, so it cannot be fired by the tick that armed it. Each
//!   tick therefore fires at most one generation, and at most
//!   [`MAX_FIRES_PER_TICK`] of it. That is the boundedness guarantee, by
//!   construction.
//! - [`AetherJobExecutor`] bounds the *other* half: the microtask drain runs to
//!   completion like a browser's, but bails out with a ledger line rather than
//!   pegging the thread if a page's promise chain never settles.

use boa_engine::{
    Context, JsResult, JsValue, JsString,
    context::ContextBuilder,
    job::{GenericJob, Job, JobExecutor, PromiseJob, SimpleJobExecutor, TimeoutJob},
    context::time::JsInstant,
    native_function::NativeFunction,
};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

/// Most timers a single `tick()` will fire. A page that has more than this
/// many callbacks due in one frame is pathological; the overflow stays queued
/// and fires on the next tick, and the shortfall is ledgered.
pub const MAX_FIRES_PER_TICK: usize = 256;

/// Bounded boot drain: how many passes of "fire due timers, then drain
/// microtasks" a page load gets before first layout, and how far the clock
/// advances per pass. Mirrors `__drainRaf`'s pass model — a chain of
/// zero-delay boot timers gets this many generations, an infinite one is cut
/// off rather than hanging the load.
pub const BOOT_PASSES: u32 = 8;
pub const BOOT_PASS_MS: u64 = 16;

/// Safety valve on one microtask drain. Normal pages settle in tens of jobs;
/// this only catches a promise chain that re-queues itself forever, which
/// would otherwise peg the engine thread exactly like the timer bug did.
const MAX_JOBS_PER_DRAIN: usize = 50_000;
/// ...and a wall-clock companion, because 50k *slow* jobs peg the thread just
/// as well as infinitely many fast ones.
const MAX_DRAIN_MS: u128 = 500;

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

thread_local! {
    /// Monotonic origin for this thread. Timer due-times are milliseconds
    /// since this instant, plus OFFSET.
    static CLOCK_BASE: Instant = Instant::now();
    /// Virtual time added to the real elapsed time: the boot drain advances
    /// it a frame per pass, and tests drive it directly.
    static CLOCK_OFFSET: Cell<u64> = const { Cell::new(0) };
    /// When set, the clock reads exactly CLOCK_OFFSET and does not advance on
    /// its own — the deterministic seam tests use instead of sleeping.
    static CLOCK_FROZEN: Cell<bool> = const { Cell::new(false) };
}

/// Milliseconds on the engine's monotonic timer clock.
pub fn now_ms() -> u64 {
    let offset = CLOCK_OFFSET.with(Cell::get);
    if CLOCK_FROZEN.with(Cell::get) {
        offset
    } else {
        CLOCK_BASE.with(|b| b.elapsed().as_millis() as u64) + offset
    }
}

/// Moves the clock forward by `ms`. The boot drain uses this to give a page's
/// zero/short-delay timers a few frames of virtual life before first layout;
/// tests use it to reach a timer's due-time without sleeping.
pub fn advance_clock(ms: u64) {
    CLOCK_OFFSET.with(|o| o.set(o.get().saturating_add(ms)));
}

/// Stops the clock at its current reading. Only [`advance_clock`] moves it
/// after this — the deterministic seam for tests.
pub fn freeze_clock() {
    let now = now_ms();
    CLOCK_FROZEN.with(|f| f.set(true));
    CLOCK_OFFSET.with(|o| o.set(now));
}

// ---------------------------------------------------------------------------
// Timer queue
// ---------------------------------------------------------------------------

/// One armed timer. The callback is NOT here — it sits in the JS global
/// `__timers` array at `slot`, so no GC object crosses into Rust storage.
struct Timer {
    id: u32,
    slot: u32,
    due_ms: u64,
    /// `Some(period)` for `setInterval`, `None` for `setTimeout`.
    interval: Option<u64>,
}

struct TimerQueue {
    timers: Vec<Timer>,
    next_id: u32,
}

thread_local! {
    static TIMERS: RefCell<TimerQueue> = RefCell::new(TimerQueue { timers: Vec::new(), next_id: 1 });
}

/// Drops every armed timer and rewinds the clock. Called when a new page boots.
pub fn reset() {
    TIMERS.with(|q| {
        let mut q = q.borrow_mut();
        q.timers.clear();
        q.next_id = 1;
    });
    CLOCK_OFFSET.with(|o| o.set(0));
    CLOCK_FROZEN.with(|f| f.set(false));
}

/// Number of timers currently armed (diagnostics and tests).
pub fn armed_count() -> usize {
    TIMERS.with(|q| q.borrow().timers.len())
}

/// `__armTimer(slot, delay, repeat)` — records a timer whose callback the JS
/// side has already parked at `__timers[slot]`. Returns the timer id.
fn arm_timer(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let slot = args.first().cloned().unwrap_or_default().to_number(context)? as u32;
    let delay = args.get(1).cloned().unwrap_or_default().to_number(context)?;
    let repeat = args.get(2).map(JsValue::to_boolean).unwrap_or(false);
    // Non-finite/negative delays clamp to 0, exactly as the HTML spec says.
    // Intervals additionally clamp to 4ms, the spec's minimum — a 0ms interval
    // is a busy loop in any engine (ours would stay bounded per tick, but it
    // would still spin a frame's worth of work every frame).
    let mut ms = if delay.is_finite() && delay > 0.0 { delay as u64 } else { 0 };
    if repeat {
        ms = ms.max(4);
    }
    let id = TIMERS.with(|q| {
        let mut q = q.borrow_mut();
        let id = q.next_id;
        q.next_id = q.next_id.wrapping_add(1).max(1);
        q.timers.push(Timer {
            id,
            slot,
            due_ms: now_ms().saturating_add(ms),
            interval: if repeat { Some(ms) } else { None },
        });
        id
    });
    Ok(JsValue::new(id))
}

/// `__clearTimer(id)` — tombstones the Rust record and hands back the JS slot
/// so the prelude can null the callback out (both halves, or the array leaks).
/// Returns -1 for an unknown id.
fn clear_timer(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as u32;
    let slot = TIMERS.with(|q| {
        let mut q = q.borrow_mut();
        match q.timers.iter().position(|t| t.id == id) {
            Some(i) => Some(q.timers.remove(i).slot),
            None => None,
        }
    });
    Ok(JsValue::new(slot.map_or(-1i32, |s| s as i32)))
}

/// Fires every timer due at the current clock reading — and nothing else.
///
/// The due set is snapshotted while the queue lock is held, before a single
/// callback runs, so anything a callback arms waits for the next call. Returns
/// how many callbacks ran.
pub fn fire_due_timers(context: &mut Context) -> usize {
    // A poisoned engine runs no more script (see `js::guarded`); the queue is
    // left alone rather than half-consumed.
    if crate::js::engine_poisoned() {
        return 0;
    }
    // Resolve the dispatcher BEFORE the snapshot: if a page has clobbered
    // `__fireTimer` there is nothing to run, and the queue must be left
    // untouched rather than half-consumed.
    let Some(fire) = fire_fn(context) else { return 0 };
    let now = now_ms();
    let mut capped = false;
    // (due_ms, id, slot, oneshot) — snapshot taken before any JS runs.
    let mut due: Vec<(u64, u32, u32, bool)> = TIMERS.with(|q| {
        let mut q = q.borrow_mut();
        let mut due = Vec::new();
        for t in q.timers.iter_mut() {
            if t.due_ms > now {
                continue;
            }
            if due.len() >= MAX_FIRES_PER_TICK {
                capped = true;
                break;
            }
            due.push((t.due_ms, t.id, t.slot, t.interval.is_none()));
            match t.interval {
                // Re-arm from the fire time, not from the original due time:
                // a queue that fell behind must not then fire a burst of
                // back-dated repeats to "catch up".
                Some(period) => t.due_ms = now.saturating_add(period),
                // One-shot: mark for the sweep below (id 0 is never issued).
                None => t.id = 0,
            }
        }
        q.timers.retain(|t| t.id != 0);
        due
    });
    if capped {
        crate::ledger::record_js(&format!("timer-fire-cap:{}", MAX_FIRES_PER_TICK));
    }
    // Due order, ties by arming order — what a browser's timer task source does.
    due.sort_by_key(|(due_ms, id, _, _)| (*due_ms, *id));

    for (_, _, slot, oneshot) in &due {
        // Guarded per callback: a throw is ledgered and the generation
        // continues (what a browser's timer task source does), and a PANIC
        // out of boa poisons the engine and stops the generation instead of
        // ending the process (see `js::guarded`).
        let fired = crate::js::guarded("timer-callback", || {
            fire.call(
                &JsValue::undefined(),
                &[JsValue::new(*slot), JsValue::new(*oneshot)],
                context,
            )
        });
        match fired {
            Some(Err(e)) => {
                crate::ledger::record_js(&record_digest("timer-callback-threw", &e));
            }
            Some(Ok(_)) => {}
            None => break,
        }
    }
    due.len()
}

/// The prelude's `__fireTimer`, if the page still has one.
fn fire_fn(context: &mut Context) -> Option<boa_engine::JsObject> {
    let v = context
        .global_object()
        .get(JsString::from("__fireTimer"), context)
        .ok()?;
    v.as_callable()
}

/// The bounded boot drain: [`BOOT_PASSES`] passes of "advance a frame, fire the
/// timers now due, settle the microtasks they queued".
///
/// This is what replaces the old unbounded `run_jobs()` at load. A page that
/// arms `setTimeout(fn, 0)` during its scripts still runs it before first
/// layout (and short-delay chains still get several generations), but a boot
/// timer that re-arms itself forever costs `BOOT_PASSES` passes instead of the
/// whole load.
pub fn boot_drain(context: &mut Context) {
    for _ in 0..BOOT_PASSES {
        if crate::js::engine_poisoned() {
            break;
        }
        advance_clock(BOOT_PASS_MS);
        let fired = fire_due_timers(context);
        let _ = context.run_jobs();
        if fired == 0 && armed_count() == 0 {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Job executor
// ---------------------------------------------------------------------------

/// Boa's `SimpleJobExecutor` with a ceiling.
///
/// Semantics are otherwise the browser's: a drain runs promise jobs (and boa's
/// own generic/timeout jobs) until the queue is genuinely empty, so promise
/// chains complete within one `tick()`. The differences are deliberate:
///
/// - a job that throws is ledgered and the drain continues, instead of
///   discarding every remaining job (boa's `SimpleJobExecutor` clears its
///   queues on the first error — one rejected handler must not silently kill
///   the rest of the page's work);
/// - the drain stops after [`MAX_JOBS_PER_DRAIN`] jobs or [`MAX_DRAIN_MS`],
///   whichever comes first, so no page can hold the engine thread forever.
#[derive(Default)]
pub struct AetherJobExecutor {
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    timeout_jobs: RefCell<Vec<(JsInstant, TimeoutJob)>>,
    /// Boa's own executor, used only for the `NativeAsyncJob` tail (nothing in
    /// Aether enqueues one; boa internals may). Forwarding keeps the future
    /// machinery — and its dependency — where it already lives.
    async_fallback: RefCell<Option<Rc<SimpleJobExecutor>>>,
}

impl std::fmt::Debug for AetherJobExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AetherJobExecutor").finish_non_exhaustive()
    }
}

impl JobExecutor for AetherJobExecutor {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        match job {
            Job::PromiseJob(p) => self.promise_jobs.borrow_mut().push_back(p),
            Job::GenericJob(g) => self.generic_jobs.borrow_mut().push_back(g),
            Job::TimeoutJob(t) => {
                let due = context.clock().now() + t.timeout();
                self.timeout_jobs.borrow_mut().push((due, t));
            }
            Job::AsyncJob(a) => {
                let mut slot = self.async_fallback.borrow_mut();
                let inner = slot.get_or_insert_with(|| Rc::new(SimpleJobExecutor::new())).clone();
                drop(slot);
                inner.enqueue_job(Job::AsyncJob(a), context);
            }
            // `Job` is #[non_exhaustive]: a job kind we do not know how to run
            // is reported, never silently dropped.
            _ => crate::ledger::record_js("job-kind-unknown"),
        }
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        let started = Instant::now();
        let mut ran = 0usize;
        loop {
            // A poisoned engine drains nothing further. Queued jobs are left
            // in place and die with the context.
            if crate::js::engine_poisoned() {
                return Ok(());
            }
            // Async tail first: whatever it resolves becomes promise jobs on
            // this executor, which the loop below then drains.
            let pending_async = self.async_fallback.borrow().clone();
            if let Some(inner) = pending_async {
                match crate::js::guarded("async-job", || inner.run_jobs(context)) {
                    Some(Err(e)) => {
                        crate::ledger::record_js(&record_digest("async-job-error", &e));
                    }
                    Some(Ok(())) => {}
                    None => return Ok(()),
                }
            }

            // Timeout jobs boa itself scheduled (page timers do NOT come
            // through here — they live in the timer queue above).
            let now = context.clock().now();
            let ready: Vec<TimeoutJob> = {
                let mut jobs = self.timeout_jobs.borrow_mut();
                let mut ready = Vec::new();
                let mut keep = Vec::with_capacity(jobs.len());
                for (due, job) in jobs.drain(..) {
                    if job.is_cancelled() {
                        continue;
                    }
                    if due <= now {
                        ready.push(job);
                    } else {
                        keep.push((due, job));
                    }
                }
                *jobs = keep;
                ready
            };
            for job in ready {
                ran += 1;
                match crate::js::guarded("timeout-job", || job.call(context)) {
                    Some(Err(e)) => {
                        crate::ledger::record_js(&record_digest("timeout-job-error", &e));
                    }
                    Some(Ok(_)) => {}
                    None => return Ok(()),
                }
            }

            let next = {
                let job = self.promise_jobs.borrow_mut().pop_front();
                match job {
                    Some(p) => Some(Runnable::Promise(p)),
                    None => self.generic_jobs.borrow_mut().pop_front().map(Runnable::Generic),
                }
            };
            let Some(next) = next else { break };
            ran += 1;
            // The guarded call: this is where boa's `RuntimeLimit` panic
            // originates. A promise reaction whose handler trips the VM
            // ceiling reaches `new_promise_reaction_job`'s `to_opaque`, which
            // panics rather than rejecting. Catching it here poisons the
            // engine and returns; the page stops running JS and renders as it
            // stands, and the process lives.
            let result = crate::js::guarded("job", || match next {
                Runnable::Promise(p) => p.call(context),
                Runnable::Generic(g) => g.call(context),
            });
            match result {
                Some(Err(e)) => crate::ledger::record_js(&record_digest("job-error", &e)),
                Some(Ok(_)) => {}
                None => return Ok(()),
            }

            if ran >= MAX_JOBS_PER_DRAIN || (ran % 512 == 0 && started.elapsed().as_millis() > MAX_DRAIN_MS)
            {
                crate::ledger::record_js(&format!("job-drain-cap:{}", ran));
                break;
            }
            context.clear_kept_objects();
        }
        context.clear_kept_objects();
        Ok(())
    }
}

enum Runnable {
    Promise(PromiseJob),
    Generic(GenericJob),
}

fn record_digest(tag: &str, e: &boa_engine::JsError) -> String {
    let msg = e.to_string();
    format!("{}:{}", tag, &msg[..msg.len().min(64)])
}

// ---------------------------------------------------------------------------
// Context construction
// ---------------------------------------------------------------------------

pub fn create_context() -> Context {
    reset();
    let mut context = ContextBuilder::new()
        .job_executor(Rc::new(AetherJobExecutor::default()))
        .build()
        .unwrap();

    crate::js::clear_poison();
    // VM stack ceiling. boa's default is 10_240 value-stack slots, which real
    // minified bundles exceed: google search trips it on roughly two runs in
    // five (bisected — raising this alone clears it, raising the recursion
    // limit alone does not). Tripping it is not a graceful error, either:
    // `RuntimeLimit` has no opaque JS form, so boa PANICS the moment the error
    // reaches a promise reaction or a JS `catch` (see `js::guarded`). The
    // ceiling therefore has to sit well clear of what pages legitimately use.
    //
    // Nothing is lost by raising it. It bounds a heap `Vec<JsValue>`, not the
    // native stack, and it was never what bounds a runaway page here — the
    // timer snapshot, `MAX_FIRES_PER_TICK`, `MAX_JOBS_PER_DRAIN` and
    // `MAX_DRAIN_MS` do that, in wall-clock terms, and they degrade instead of
    // aborting. 1 Mi slots is ~16 MB at full extension and ~100x the default.
    //
    // The recursion limit stays at boa's 512: unlike the value stack it does
    // bound native re-entrancy (a native that calls back into JS recurses in
    // Rust), so it is the guard on the real thread stack. The loop-iteration
    // limit stays disabled (boa's default) — a long loop is bounded by the
    // drain's wall clock, and killing it here would only kill it by panic.
    context.runtime_limits_mut().set_stack_size_limit(1 << 20);

    context
        .register_global_callable(JsString::from("__armTimer"), 3, NativeFunction::from_fn_ptr(arm_timer))
        .unwrap();
    context
        .register_global_callable(JsString::from("__clearTimer"), 1, NativeFunction::from_fn_ptr(clear_timer))
        .unwrap();

    // The JS half of the timer queue. Callbacks live here, indexed by slot;
    // Rust holds only the slot number. Freed slots are recycled so a page that
    // arms timers forever does not grow the array forever.
    let _ = context.eval(boa_engine::Source::from_bytes(
        r#"
        globalThis.__timers = [];
        globalThis.__timerFree = [];
        globalThis.__armTimerJs = function (fn, delay, repeat, extra) {
            // setTimeout('code') is legal and still shows up in the wild.
            if (typeof fn === 'string') {
                var src = fn;
                fn = function () { (0, eval)(src); };
            }
            if (typeof fn !== 'function') { return 0; }
            var cb = (extra && extra.length)
                ? function () { return fn.apply(undefined, extra); }
                : fn;
            var slot = __timerFree.length ? __timerFree.pop() : __timers.length;
            __timers[slot] = cb;
            return __armTimer(slot, delay, repeat);
        };
        globalThis.__fireTimer = function (slot, oneshot) {
            var cb = __timers[slot];
            if (oneshot) {
                // Release before the call: a one-shot's record is already gone
                // from the queue, and holding the closure past its single use
                // keeps whatever it closed over alive.
                __timers[slot] = null;
                __timerFree.push(slot);
            }
            // No try/catch here on purpose: a throw propagates out to the
            // Rust caller, which ledgers it and moves to the next timer. A JS
            // `catch` would only swallow it silently — and would not even see
            // the error a runaway callback actually produces, since boa's
            // RuntimeLimit is uncatchable in JS by construction.
            if (typeof cb === 'function') { cb(); }
        };
        globalThis.setTimeout = function (fn, delay) {
            return __armTimerJs(fn, delay, false, Array.prototype.slice.call(arguments, 2));
        };
        globalThis.setInterval = function (fn, delay) {
            return __armTimerJs(fn, delay, true, Array.prototype.slice.call(arguments, 2));
        };
        globalThis.clearTimeout = function (id) {
            var slot = __clearTimer(+id || 0);
            if (slot >= 0) { __timers[slot] = null; __timerFree.push(slot); }
        };
        globalThis.clearInterval = globalThis.clearTimeout;
        "#,
    ));

    context
}
