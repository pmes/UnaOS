// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! GLASSLAG M1 (rmbp-ledger B370) — THE PER-EVENT LATENCY LINE. Flight 22: Peter at the glass, "delay in
//! interactivity", "delay in opening when clicked in taskbar", the lumen echo "with a delay" — and the wire
//! had no counterpart: `[wpace] rollup … -> FREE` says the compositor is not pacing, so the time is spent
//! somewhere else. This module times ONE event at a time per class (keyboard, pointer press) from the
//! input funnel to the glass, stamping the stages between, and names the stage that ate the time:
//!
//! `[lag] <what> ms=<total> queue=<ms> wm=<ms> app=<ms> comp=<ms> present=<ms> worst=<stage>`
//!
//! `<what>` is `key→echo` (a key press until the frame that shows its effect), `click→shown` (a press on a
//! window), `click→window-shown` (a press on a taskbar tile until the window it opens or raises is on the
//! glass) or `menu→open` (a press that opens the crystal, a window menu or a tile menu). The stages:
//!
//! * `queue`   — the event's push into `pal`'s ring (the decoder, ISR or poll) → the router took it off.
//! * `wm`      — the router took it → it was routed (a ring-3 app's input ring, a kernel surface, a launch).
//! * `app`     — routed → the app took it (ring 3: `sys_input_poll` read it; a kernel surface: the router
//!   returned; a launch: the window was minted or raised; a menu: it opened).
//! * `comp`    — the app took it → it drew (ring 3: the app's present syscall; kernel: the next composite pass).
//! * `present` — it drew → that composite pass ended (the frame is on the glass).
//!
//! Every 5 s with any event in the span: `:: LAG: n= key= click= launch= menu= p50= p95= max= max_kind=
//! worst_stage= stage_ms=[…] timeout= orphan= ::` — so the next flight names the stage. A span with no event
//! prints nothing (R80: nothing runs at boot but the boot; an idle machine stays quiet).
//!
//! An event that never reaches the glass within its bound (3 s, 10 s for a launch) prints
//! `[lag] <what> timeout ms=<age> reached=<stage>` when it got as far as being routed; one that was never
//! routed (a key with no taker) is counted as `orphan=` only. A key is printed per event only at
//! `ms >= 50` (typing would otherwise flood the wire); every key is in the rollup. No key VALUE is ever
//! printed (R65).
//!
//! # Honesty
//! * One slot per class: while an event is in flight, later events of its class are not timed (the OLDEST
//!   pending event is the one measured, the same proxy `rtwit` uses). Counted as `coalesced=`.
//! * Stage attribution follows FIFO order, not identity: the first key to reach the router after the timed
//!   key was pushed is taken to be that key. A ring-3 key's `app` stamp is exact (the ring sequence number).
//! * A kernel surface that draws inside the router is charged one extra composite pass.
//! * x86 + `wc` only (the hooks live in the x86 router). Elsewhere every entry point is a no-op.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering::{AcqRel, Acquire, Relaxed, Release}};

use crate::pal::Event;

/// The instrument is live only where its hooks are (the x86 router chain, the `wc` compositor).
const ON: bool = cfg!(all(target_arch = "x86_64", feature = "wc"));

// Stages (index into `t` is `stage - 1`; `t[5]` is the present stamp).
const IDLE: u8 = 0;
const ISR: u8 = 1;
const QUEUE: u8 = 2;
const WM: u8 = 3;
const APP: u8 = 4;
const COMP: u8 = 5;

// Kinds.
const K_KEY: u8 = 0;
const K_CLICK: u8 = 1;
const K_LAUNCH: u8 = 2;
const K_MENU: u8 = 3;

// Classes.
const C_KEY: usize = 0;
const C_PTR: usize = 1;

/// The stage names, in order (`d[i]` is the time spent in `STAGES[i]`).
pub const STAGES: [&str; 5] = ["queue", "wm", "app", "comp", "present"];
/// Rollup cadence.
pub const SPAN_MS: u64 = 5_000;
/// A key faster than this is in the rollup only.
pub const KEY_PRINT_MS: u64 = 50;
/// The bound before an in-flight event is called a timeout (a launch reads a program from the disk).
pub const TIMEOUT_MS: u64 = 3_000;
pub const LAUNCH_TIMEOUT_MS: u64 = 10_000;

struct Slot {
    stage: AtomicU8,
    kind: AtomicU8,
    /// 0 = a kernel surface; `n` = ring-3 input slot `n - 1`.
    ring: AtomicU8,
    /// The ring's tail just after this event (ring-3 only).
    seq: AtomicU32,
    t: [AtomicU64; 6],
    /// INPUTSTALL M1: a kernel surface's present REQUEST (`wm::present`) inside the `comp` stage — splits it
    /// into `draw=` (the handler painting) and `pre=` (the present path up to the pass). 0 = none.
    req: AtomicU64,
}

impl Slot {
    const fn new() -> Self {
        Slot {
            stage: AtomicU8::new(IDLE),
            kind: AtomicU8::new(K_KEY),
            ring: AtomicU8::new(0),
            seq: AtomicU32::new(0),
            t: [const { AtomicU64::new(0) }; 6],
            req: AtomicU64::new(0),
        }
    }
    /// Move `from` → `to`, stamping every stage passed through with `now`. False if the slot moved on.
    fn advance(&self, from: u8, to: u8, now: u64) -> bool {
        if self.stage.compare_exchange(from, to, AcqRel, Relaxed).is_err() {
            return false;
        }
        let mut s = from;
        while s < to {
            self.t[s as usize].store(now, Relaxed);
            s += 1;
        }
        true
    }
}

static SLOTS: [Slot; 2] = [Slot::new(), Slot::new()];

// ── the span (rollup) ──────────────────────────────────────────────────────────────────────────
const SAMPLES: usize = 64;
struct Span {
    n: u32,
    by_kind: [u32; 4],
    ms: [u32; SAMPLES],
    max_ms: u32,
    max_kind: u8,
    stage_us: [u64; 5],
}
static SPAN: crate::sync::Mutex<Span> = crate::sync::Mutex::new(Span { n: 0, by_kind: [0; 4], ms: [0; SAMPLES], max_ms: 0, max_kind: K_KEY, stage_us: [0; 5] });
static TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static ORPHANS: AtomicU32 = AtomicU32::new(0);
static COALESCED: AtomicU32 = AtomicU32::new(0);
static LOST: AtomicU32 = AtomicU32::new(0); #[cfg(all(feature = "ftdirx", feature = "wc", target_arch = "x86_64"))] static ROUTE_DOOR0: AtomicU64 = AtomicU64::new(0); // QUEUEWEDGE (B533 SMALLFIX9): the door's claimed-byte count at the route's entry; the guard's drop reads it again to know the route took a door byte
static NEXT_ROLLUP_MS: AtomicU64 = AtomicU64::new(0);
/// A finished event whose line is owed (printed from an unmasked context): packed by `finish`.
static OWED: crate::sync::Mutex<[Option<Done>; 4]> = crate::sync::Mutex::new([None; 4]);

#[derive(Clone, Copy)]
struct Done {
    kind: u8,
    total_us: u64,
    d: [u64; 5],
    timeout_at: Option<u8>,
}

#[inline]
fn now_us() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let hz = crate::arch::x86_64::apic::tsc_hz();
        if hz >= 1_000_000 {
            return crate::arch::now_cycles() / (hz / 1_000_000);
        }
    }
    crate::arch::ms().saturating_mul(1000)
}

#[inline]
fn class_of(ev: &Event) -> Option<(usize, u8)> {
    match ev {
        Event::Key(_) => Some((C_KEY, K_KEY)),
        Event::Button(m) if m & 0b11 != 0 => Some((C_PTR, K_CLICK)),
        _ => None,
    }
}

fn label(kind: u8) -> &'static str {
    match kind {
        K_KEY => "key→echo",
        K_CLICK => "click→shown",
        K_LAUNCH => "click→window-shown",
        _ => "menu→open",
    }
}

fn stage_name(stage: u8) -> &'static str {
    match stage {
        ISR => "isr",
        QUEUE => "queue",
        WM => "wm",
        APP => "app",
        COMP => "comp",
        _ => "idle",
    }
}

/// `pal`'s enqueue funnel: a key press or a primary-button press starts a timing if its class is free.
/// Masked context (the ring lock is held): atomics only, never a print.
pub fn on_enqueue(ev: &Event) {
    if !ON {
        return;
    }
    let Some((c, kind)) = class_of(ev) else { return };
    let now = now_us();
    pend_enqueued(now); // INPUTSTALL M3: one more key/press waits for the render task
    let s = &SLOTS[c];
    if s.stage.compare_exchange(IDLE, ISR, AcqRel, Relaxed).is_ok() {
        s.kind.store(kind, Relaxed);
        s.ring.store(0, Relaxed);
        s.req.store(0, Relaxed);
        s.t[0].store(now, Release);
    } else {
        COALESCED.fetch_add(1, Relaxed);
    }
}

/// The router's guard (`wc_route_event`): the event is off the ring (`queue` ends) and, when the guard
/// drops without a ring-3 delivery or a launch having claimed it, a kernel surface consumed it (`wm` and
/// `app` end together).
/// INPUTSTALL M1: `.1` is the route's entry stamp, `.2` whether it runs on the render task.
pub struct RouteGuard(Option<usize>, u64, bool);

pub fn route(ev: &Event) -> RouteGuard {
    if !ON {
        return RouteGuard(None, 0, false);
    }
    let now = now_us();
    let r = render_here(); qs_probe_route(ev); #[cfg(all(feature = "ftdirx", feature = "wc", target_arch = "x86_64"))] ROUTE_DOOR0.store(crate::drivers::xhci::ftdi::ftdirx::origin_census().0, Relaxed); // QUEUEWEDGE (B533): snapshot the door's claim count. QUEUESLEEP (B512): the fixture's probe key is timed to its route
    if r {
        render_entry(now); seg_anchor(now, true); // INPUTSTALL M1: the previous event's handler ended here (INPUTSTALL2 M1: the route's first door is timed from here)
    }
    let Some((c, _)) = class_of(ev) else {
        return RouteGuard(None, now, r);
    };
    pend_routed(); // INPUTSTALL M3: a key/press left the queue
    if SLOTS[c].advance(ISR, QUEUE, now) { RouteGuard(Some(c), now, r) } else { RouteGuard(None, now, r) }
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        let now = now_us();
        if let Some(c) = self.0 {
            let _ = SLOTS[c].advance(QUEUE, APP, now); #[cfg(all(feature = "ftdirx", feature = "wc", target_arch = "x86_64"))] if crate::drivers::xhci::ftdi::ftdirx::origin_census().0 != ROUTE_DOOR0.load(Relaxed) && crate::serialdoor::route_word() == "headless" && SLOTS[c].advance(APP, COMP, now) && SLOTS[c].stage.compare_exchange(COMP, IDLE, AcqRel, Relaxed).is_ok() { SLOTS[c].t[5].store(now, Relaxed); finish(&SLOTS[c], None); } // QUEUEWEDGE (B533 SMALLFIX9) — ⚠ SAME-LINE fold, line-NEUTRAL: a door byte routed to the HEADLESS console (no shell window) has nothing to draw, and no composite pass runs on an idle desktop, so its timing ends HERE at the route (comp = present = 0) instead of being charged to `comp` until the 3 s timeout
        }
        if self.2 {
            render_route_done(self.1, now); // INPUTSTALL M1
        }
        if ON {
            flush(crate::arch::ms());
        }
    }
}

/// The router delivered the event to ring-3 input slot `slot`; `seq` is that ring's tail after it.
pub fn routed_ring(ev: &Event, slot: usize, seq: u32) {
    if !ON {
        return;
    }
    let Some((c, _)) = class_of(ev) else { return };
    let s = &SLOTS[c];
    if s.stage.load(Acquire) == QUEUE {
        s.ring.store((slot as u8).wrapping_add(1), Relaxed);
        s.seq.store(seq, Relaxed);
        let _ = s.advance(QUEUE, WM, now_us());
    }
}

/// `sys_input_poll` consumed ring-3 slot `slot`'s event; `head` is the ring's head after it.
pub fn consumed_ring(slot: usize, head: u32) {
    if !ON {
        return;
    }
    for s in &SLOTS {
        if s.stage.load(Acquire) == WM && s.ring.load(Relaxed) == (slot as u8).wrapping_add(1)
            && head.wrapping_sub(s.seq.load(Relaxed)) as i32 >= 0
        {
            let _ = s.advance(WM, APP, now_us());
        }
    }
}

/// A ring-3 present syscall from slot `slot` (the app drew).
pub fn app_drew(slot: usize) {
    if !ON {
        return;
    }
    for s in &SLOTS {
        if s.stage.load(Acquire) == APP
            && (s.ring.load(Relaxed) == (slot as u8).wrapping_add(1) || s.kind.load(Relaxed) == K_LAUNCH)
        {
            let _ = s.advance(APP, COMP, now_us());
        }
    }
}

/// The dock took the press on a tile (a launch, an open request or a raise): the timing becomes
/// `click→window-shown` and its `wm` stage ends here.
pub fn launch_routed() {
    if !ON {
        return;
    }
    let s = &SLOTS[C_PTR];
    if s.stage.load(Acquire) == QUEUE {
        s.kind.store(K_LAUNCH, Relaxed);
        let _ = s.advance(QUEUE, WM, now_us());
    }
}

/// A window was minted (`wm::create_inner`) or raised (`wm::raise_one`): a launch's `app` stage ends.
pub fn window_shown() {
    if !ON {
        return;
    }
    let s = &SLOTS[C_PTR];
    if s.kind.load(Relaxed) == K_LAUNCH {
        let _ = s.advance(WM, APP, now_us());
    }
}

/// A menu opened (crystal, a window menu, a tile menu): the timing becomes `menu→open`, `app` ends here.
pub fn menu_opened() {
    if !ON {
        return;
    }
    let s = &SLOTS[C_PTR];
    let now = now_us();
    for from in [QUEUE, WM] {
        if s.stage.load(Acquire) == from {
            s.kind.store(K_MENU, Relaxed);
            if s.advance(from, APP, now) {
                return;
            }
        }
    }
}

/// The composite pass's guard (`wm::composite`): at entry a kernel-surface event that its consumer has
/// taken is drawing (`comp` ends); at exit every event that drew is on the glass (`present` ends).
/// INPUTSTALL M1: `.0` is the pass's entry stamp, `.1` whether it runs on the render task.
pub struct PassGuard(u64, bool);

pub fn pass() -> PassGuard {
    if ON {
        let now = now_us();
        crate::video::beam::pass_begin(); // INPUTSTALL M2: this pass's one frame of beam wait starts here
        let r = render_here();
        if r {
            render_entry(now);
        }
        for s in &SLOTS {
            if s.stage.load(Acquire) == APP && s.ring.load(Relaxed) == 0 {
                let _ = s.advance(APP, COMP, now);
            }
        }
        return PassGuard(now, r);
    }
    PassGuard(0, false)
}

impl Drop for PassGuard {
    fn drop(&mut self) {
        if !ON {
            return;
        }
        let now = now_us();
        sec_pass(self.0, now, self.1); // INPUTSTALL M1
        for s in &SLOTS {
            if s.stage.compare_exchange(COMP, IDLE, AcqRel, Relaxed).is_ok() {
                s.t[5].store(now, Relaxed);
                finish(s, None);
            }
        }
        if irqs_on() {
            flush(crate::arch::ms());
        }
    }
}

fn irqs_on() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        x86_64::instructions::interrupts::are_enabled()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// The per-stage split of a finished (or timed-out) slot. Pure over the stamps.
pub fn split(t: &[u64; 6], upto: usize) -> [u64; 5] {
    let mut d = [0u64; 5];
    let mut i = 0;
    while i < 5 && i < upto {
        d[i] = t[i + 1].saturating_sub(t[i]);
        i += 1;
    }
    d
}

/// The stage that took the longest. Pure.
pub fn worst(d: &[u64; 5]) -> usize {
    let mut w = 0;
    for i in 1..5 {
        if d[i] > d[w] {
            w = i;
        }
    }
    w
}

fn finish(s: &Slot, timeout_at: Option<u8>) {
    let mut t = [0u64; 6];
    for (i, x) in t.iter_mut().enumerate() {
        *x = s.t[i].load(Relaxed);
    }
    let kind = s.kind.load(Relaxed);
    let (d, total_us) = match timeout_at {
        None => (split(&t, 5), t[5].saturating_sub(t[0])),
        Some(st) => (split(&t, (st as usize).saturating_sub(1)), now_us().saturating_sub(t[0])),
    };
    if timeout_at.is_none() {
        let req = s.req.load(Relaxed);
        let split_comp = if s.ring.load(Relaxed) == 0 && req >= t[3] && req <= t[4] && req != 0 {
            Some((req - t[3], t[4] - req))
        } else {
            None
        };
        sec_event(kind, &d, split_comp); // INPUTSTALL M1
        if kind == K_KEY {
            crate::perf::note_key(total_us); // PERFREVIEW (B443)
        }
        match SPAN.try_lock() {
            Some(mut sp) => {
                let ms = (total_us / 1000) as u32;
                let i = sp.n as usize;
                if i < SAMPLES {
                    sp.ms[i] = ms;
                }
                sp.n += 1;
                sp.by_kind[kind as usize & 3] += 1;
                if ms >= sp.max_ms {
                    sp.max_ms = ms;
                    sp.max_kind = kind;
                }
                for k in 0..5 {
                    sp.stage_us[k] += d[k];
                }
            }
            None => {
                LOST.fetch_add(1, Relaxed);
            }
        }
    }
    let print = timeout_at.is_some() || kind != K_KEY || total_us >= KEY_PRINT_MS * 1000;
    if print {
        if let Some(mut o) = OWED.try_lock() {
            if let Some(slot) = o.iter_mut().find(|x| x.is_none()) {
                *slot = Some(Done { kind, total_us, d, timeout_at });
            }
        }
    }
}

fn ms_str(us: u64) -> alloc::string::String {
    alloc::format!("{}.{}", us / 1000, (us % 1000) / 100)
}

/// Print the owed per-event lines, expire stuck events, and roll the span up every [`SPAN_MS`].
/// Unmasked contexts only (the router guard, the composite guard with IRQs on).
fn flush(now_ms: u64) {
    sec_roll(now_ms); // INPUTSTALL M1/M4: the per-second stall line and the per-minute witness
    // Expiry.
    for s in &SLOTS {
        let st = s.stage.load(Acquire);
        if st == IDLE {
            continue;
        }
        let t0 = s.t[0].load(Relaxed);
        let bound = if s.kind.load(Relaxed) == K_LAUNCH { LAUNCH_TIMEOUT_MS } else { TIMEOUT_MS };
        if now_us().saturating_sub(t0) > bound * 1000 && s.stage.compare_exchange(st, IDLE, AcqRel, Relaxed).is_ok() {
            if st >= WM {
                TIMEOUTS.fetch_add(1, Relaxed);
                finish(s, Some(st));
            } else {
                ORPHANS.fetch_add(1, Relaxed);
            }
        }
    }
    // Owed lines.
    let owed = match OWED.try_lock() {
        Some(mut o) => {
            let c = *o;
            *o = [None; 4];
            c
        }
        None => [None; 4],
    };
    for d in owed.iter().flatten() {
        match d.timeout_at {
            None => serial_println!(
                "[lag] {} ms={} queue={} wm={} app={} comp={} present={} worst={}",
                label(d.kind), ms_str(d.total_us), ms_str(d.d[0]), ms_str(d.d[1]), ms_str(d.d[2]), ms_str(d.d[3]), ms_str(d.d[4]),
                STAGES[worst(&d.d)]
            ),
            Some(st) => serial_println!(
                "[lag] {} timeout ms={} reached={} queue={} wm={} app={}",
                label(d.kind), ms_str(d.total_us), stage_name(st), ms_str(d.d[0]), ms_str(d.d[1]), ms_str(d.d[2])
            ),
        }
    }
    // Rollup.
    let due = NEXT_ROLLUP_MS.load(Relaxed);
    if now_ms < due {
        return;
    }
    if NEXT_ROLLUP_MS.compare_exchange(due, now_ms + SPAN_MS, Relaxed, Relaxed).is_err() || due == 0 {
        return;
    }
    let (n, by, mut ms, max_ms, max_kind, stage_us) = match SPAN.try_lock() {
        Some(mut sp) => {
            let r = (sp.n, sp.by_kind, sp.ms, sp.max_ms, sp.max_kind, sp.stage_us);
            *sp = Span { n: 0, by_kind: [0; 4], ms: [0; SAMPLES], max_ms: 0, max_kind: K_KEY, stage_us: [0; 5] };
            r
        }
        None => return,
    };
    let to = TIMEOUTS.swap(0, Relaxed);
    let orph = ORPHANS.swap(0, Relaxed);
    let coal = COALESCED.swap(0, Relaxed);
    let lost = LOST.swap(0, Relaxed);
    if n == 0 && to == 0 {
        return;
    }
    let k = (n as usize).min(SAMPLES);
    let (p50, p95) = percentiles(&mut ms[..k]);
    serial_println!(
        ":: LAG: n={} key={} click={} launch={} menu={} p50={} p95={} max={} max_kind={} worst_stage={} stage_ms=[queue:{},wm:{},app:{},comp:{},present:{}] timeout={} orphan={} coalesced={} lost={} span={}s ::",
        n, by[K_KEY as usize], by[K_CLICK as usize], by[K_LAUNCH as usize], by[K_MENU as usize], p50, p95, max_ms, label(max_kind),
        if n == 0 { "none" } else { STAGES[worst(&stage_us)] },
        stage_us[0] / 1000, stage_us[1] / 1000, stage_us[2] / 1000, stage_us[3] / 1000, stage_us[4] / 1000,
        to, orph, coal, lost, SPAN_MS / 1000
    );
}

/// `(p50, p95)` of the span's samples (sorted in place). `(0, 0)` for none. Pure.
pub fn percentiles(v: &mut [u32]) -> (u32, u32) {
    if v.is_empty() {
        return (0, 0);
    }
    v.sort_unstable();
    let n = v.len();
    (v[(n - 1) / 2], v[((n * 95).div_ceil(100)).saturating_sub(1).min(n - 1)])
}

// ── INPUTSTALL (rmbp-ledger B375; R88 + VUGFITS, flight 23) ─────────────────────────────────────
//
// Flight 23 printed the three symptoms on three different lines that nobody could line up: `[lag]` (one
// event's stages), `:: BEAMHOLD:` (every 1024 bands) and the vug's `:: VUGART: … strand=` (ring 3, per
// rollup). This section puts them on ONE line per second, only for a second in which something exceeded
// [`STALL_MS`]:
//
// `[lag] stall at_ms= span_ms= stage=<queue|wm|app|comp|present|hid> stage_ms= queue= wm= app= comp=
//  present= draw= pre= render=<route|handler|composite> render_ms= passes= pass_ms_max= rows= full=
//  beam_ms= beam_max_ms= capped= yielded= valve= hid_gap_ms= strand=<stranded>/<frames>`
//
// * the five stage columns are the max over the events that finished in the second, AND the age of any
//   event still in flight at the roll (a keystroke sitting in the queue is charged while it sits);
// * `draw=`/`pre=` split a kernel surface's `comp` at its present REQUEST (`wm::present`): the handler
//   painting vs the present path up to the pass — `-` when the surface drew without asking;
// * `render=` is what the RENDER TASK (the single consumer of the input channel) spent longest on: routing
//   an event, running the consumer's handler after the route, or a composite pass (which includes the
//   re-runs it did for presents that folded behind it);
// * `passes=`/`pass_ms_max=` every composite pass on every core; `rows=`/`full=` the panel rows the beam
//   brackets covered and how many brackets were full-panel (≥ 90 % of the rows) — the dirty-rect area the
//   brief asked `comp=` to be measured against; `beam_ms=`/`beam_max_ms=`/`capped=` the beam spin;
// * `yielded=` the re-runs the render task declined because input was waiting (M3);
// * `valve=` the WC-D valve (`open`, `closed`, `none` where the valve is not built);
// * `hid_gap_ms=` the longest gap between two EHCI HID service passes (the pump sleeps one ~4 ms tick);
// * `strand=` the vug's own count, reported through `SYS_PROF(OP_NOTE)` once per frame.
//
// Once a minute in which any input event finished: `:: INPUTSTALL: key_queue_max_ms= comp_max_ms=
// hid_gap_max_ms= strand_pct= bound=50 -> PASS|FAIL ::` — PASS iff the three ms are ≤ 50 and
// `strand_pct` ≤ [`STRAND_PCT_BOUND`]. R80: a reading of the live path, never a test; an idle minute
// prints nothing.
//
// HONEST LIMIT: a press a kernel surface consumes WITHOUT drawing (the login's `control=none`) is still
// charged `comp` until the next unrelated pass — the instrument cannot know the handler chose not to draw.

/// A stage, render phase or HID gap at or over this is a stall second.
pub const STALL_MS: u64 = 50;
/// The witness's strand bound, in percent of the vug frames in the minute.
pub const STRAND_PCT_BOUND: u64 = 1;
const SEC_MS: u64 = 1_000;
const MIN_MS: u64 = 60_000;
/// Stall lines are bounded: the first 600, then one in 64.
const STALL_LINES_FREE: u32 = 600;

static SEC_T0_MS: AtomicU64 = AtomicU64::new(0);
static SEC_STAGE_US: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
static SEC_DRAW_US: AtomicU64 = AtomicU64::new(0);
static SEC_PRE_US: AtomicU64 = AtomicU64::new(0);
static SEC_SPLIT: AtomicU32 = AtomicU32::new(0);
static SEC_PASSES: AtomicU32 = AtomicU32::new(0);
static SEC_PASS_MAX_US: AtomicU64 = AtomicU64::new(0);
static SEC_R_ROUTE_US: AtomicU64 = AtomicU64::new(0);
static SEC_R_HANDLER_US: AtomicU64 = AtomicU64::new(0);
static SEC_R_COMP_US: AtomicU64 = AtomicU64::new(0);
static SEC_HID_MS: AtomicU64 = AtomicU64::new(0);
static SEC_FRAMES: AtomicU32 = AtomicU32::new(0);
static SEC_STRANDS: AtomicU32 = AtomicU32::new(0);
static SEC_YIELDS: AtomicU32 = AtomicU32::new(0);
static STALL_LINES: AtomicU32 = AtomicU32::new(0);

static MIN_T0_MS: AtomicU64 = AtomicU64::new(0);
static MIN_EVENTS: AtomicU32 = AtomicU32::new(0);
static MIN_KEYQ_US: AtomicU64 = AtomicU64::new(0);
static MIN_COMP_US: AtomicU64 = AtomicU64::new(0);
static MIN_HID_MS: AtomicU64 = AtomicU64::new(0);
static MIN_FRAMES: AtomicU32 = AtomicU32::new(0);
static MIN_STRANDS: AtomicU32 = AtomicU32::new(0);

/// The render task's last route exit (0 = none open): the start of the consumer's handler.
static RENDER_EXIT_US: AtomicU64 = AtomicU64::new(0);
/// M3: keys/presses pushed into `pal` and not yet routed, and when the last one was pushed.
static PEND: AtomicU32 = AtomicU32::new(0);
static PEND_LAST_US: AtomicU64 = AtomicU64::new(0);

/// Is this the render task — the one consumer of the GUI input channel (`render`, or its re-homed twin)?
#[inline]
fn render_here() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        matches!(crate::arch::sched::current_name(), Some(n) if n.starts_with("render"))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// The render task started something new (a route, a pass) or parked: the handler interval closes.
fn render_entry(now: u64) {
    let ex = RENDER_EXIT_US.swap(0, Relaxed); RENDER_PARKED.store(false, Relaxed); // QUEUESLEEP (B512): the render task is off the channel
    if ex != 0 && now > ex {
        SEC_R_HANDLER_US.fetch_max(now - ex, Relaxed);
        boot_handler_note(now - ex); crate::video::loginrender::turn(ex, now); // LOGINRENDER (B514): the turn, measured for `tests loginrender` (same-line fold). SMALLFIX6 (B495): the boot's longest handler interval and when it ended
    }
}

fn render_route_done(t0: u64, now: u64) {
    SEC_R_ROUTE_US.fetch_max(now.saturating_sub(t0), Relaxed);
    RENDER_EXIT_US.store(now, Relaxed);
}

/// The render task is about to park on the input channel (`gui_recv_blocking_x86`): whatever it was doing
/// since its last route ends here, and the park itself is not a stall.
pub fn render_idle() {
    if ON {
        render_entry(now_us());
        seg_anchor(0, false); RENDER_PARKED.store(true, Relaxed); // QUEUESLEEP (B512): parked on the channel. INPUTSTALL2 M1: the park is not a step
    }
}

fn sec_pass(t0: u64, now: u64, render: bool) {
    let d = now.saturating_sub(t0);
    SEC_PASSES.fetch_add(1, Relaxed);
    SEC_PASS_MAX_US.fetch_max(d, Relaxed);
    crate::perf::note_frame(d); crate::video::loginrender::pass(now); // LOGINRENDER (B514): a pass ended inside the turn or not (same-line fold). PERFREVIEW (B443)
    if render {
        SEC_R_COMP_US.fetch_max(d, Relaxed);
    }
}

fn sec_event(kind: u8, d: &[u64; 5], split: Option<(u64, u64)>) {
    for (k, v) in d.iter().enumerate() {
        SEC_STAGE_US[k].fetch_max(*v, Relaxed);
    }
    if let Some((a, b)) = split {
        SEC_DRAW_US.fetch_max(a, Relaxed);
        SEC_PRE_US.fetch_max(b, Relaxed);
        SEC_SPLIT.fetch_add(1, Relaxed);
    }
    MIN_EVENTS.fetch_add(1, Relaxed);
    if kind == K_KEY {
        MIN_KEYQ_US.fetch_max(d[0], Relaxed);
    }
    MIN_COMP_US.fetch_max(d[3], Relaxed); QS_QUEUE_US.fetch_max(d[0], Relaxed); // QUEUESLEEP (B512): the fixture window's queue max
}

/// `wm::present` — a kernel surface asked for its pass. The first request inside an event's `comp`
/// stage is the split point (`draw=` before it, `pre=` after it).
pub fn present_req() {
    if !ON {
        return;
    }
    let mut now = 0;
    for s in &SLOTS {
        if s.stage.load(Acquire) == APP && s.ring.load(Relaxed) == 0 {
            if now == 0 {
                now = now_us();
            }
            let _ = s.req.compare_exchange(0, now, Relaxed, Relaxed);
        }
    }
}

/// `boot::hid_pass` — the gap since the previous EHCI HID service pass, in ms.
pub fn hid_gap(ms: u64) {
    if ON {
        SEC_HID_MS.fetch_max(ms, Relaxed);
        MIN_HID_MS.fetch_max(ms, Relaxed);
    }
}

/// `SYS_PROF(OP_NOTE, kind, value)` — a program's own frame note. `NOTE_FRAME`: one presented frame,
/// `value != 0` when the frame stranded (the vug's `strand=`).
pub fn app_note(kind: u64, value: u64) {
    if !ON {
        return;
    }
    if kind == una_abi::prof::NOTE_FRAME {
        SEC_FRAMES.fetch_add(1, Relaxed);
        MIN_FRAMES.fetch_add(1, Relaxed);
        if value != 0 {
            SEC_STRANDS.fetch_add(1, Relaxed);
            MIN_STRANDS.fetch_add(1, Relaxed);
        }
    }
}

fn pend_enqueued(now: u64) {
    PEND.fetch_add(1, Relaxed);
    PEND_LAST_US.store(now, Relaxed);
}

fn pend_routed() {
    let mut v = PEND.load(Relaxed);
    while v != 0 {
        match PEND.compare_exchange_weak(v, v - 1, Relaxed, Relaxed) {
            Ok(_) => return,
            Err(now) => v = now,
        }
    }
}

/// INPUTSTALL M3 — **INPUT FIRST.** Asked by `wm::composite`'s gate holder before each re-run it would do
/// for presents that folded behind it: `true` when the holder is the RENDER TASK and a key or press is
/// waiting for it, i.e. the re-run would hold the operator's input behind somebody else's frame. The holder
/// then leaves the damage on the table (`COMP_PENDING` stays set) and returns to drain; the render task's own
/// burst present, or the folding program's next present, takes it. A count the queue lost track of (an event
/// popped by a full-screen loop's own pump, never routed) expires with the [`TIMEOUT_MS`] bound.
pub fn input_first() -> bool {
    if !ON || PEND.load(Relaxed) == 0 || !render_here() {
        return false;
    }
    let now = now_us();
    if now.saturating_sub(PEND_LAST_US.load(Relaxed)) > TIMEOUT_MS * 1000 {
        PEND.store(0, Relaxed);
        return false;
    }
    SEC_YIELDS.fetch_add(1, Relaxed);
    true
}

/// The WC-D valve, as the stall line names it.
fn valve_word() -> &'static str {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        crate::video::wm::inputstall_valve()
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        "none"
    }
}

/// Roll the second (and the minute). Unmasked contexts only — called from [`flush`].
fn sec_roll(now_ms: u64) {
    let t0 = SEC_T0_MS.load(Relaxed);
    if t0 == 0 {
        let _ = SEC_T0_MS.compare_exchange(0, now_ms, Relaxed, Relaxed);
        let _ = MIN_T0_MS.compare_exchange(0, now_ms, Relaxed, Relaxed);
        return;
    }
    if now_ms < t0 + SEC_MS || SEC_T0_MS.compare_exchange(t0, now_ms, Relaxed, Relaxed).is_err() {
        return;
    }
    let nu = now_us();
    let mut st = [0u64; 5];
    for (k, v) in st.iter_mut().enumerate() {
        *v = SEC_STAGE_US[k].swap(0, Relaxed);
    }
    // An event still in flight is charged its age in the stage it is in.
    for (c, s) in SLOTS.iter().enumerate() {
        let stg = s.stage.load(Acquire);
        if stg == IDLE {
            continue;
        }
        let i = (stg - 1) as usize;
        if i < 5 {
            let since = s.t[i].load(Relaxed);
            if since != 0 && nu > since {
                st[i] = st[i].max(nu - since); if i == 0 { QS_QUEUE_US.fetch_max(nu - since, Relaxed); } // QUEUESLEEP (B512): a key still in the queue counts while it sits
                if i == 0 && c == C_KEY {
                    MIN_KEYQ_US.fetch_max(nu - since, Relaxed);
                }
            }
        }
    }
    let draw = SEC_DRAW_US.swap(0, Relaxed);
    let pre = SEC_PRE_US.swap(0, Relaxed);
    let split = SEC_SPLIT.swap(0, Relaxed);
    let passes = SEC_PASSES.swap(0, Relaxed);
    let pass_max = SEC_PASS_MAX_US.swap(0, Relaxed);
    let r = [SEC_R_ROUTE_US.swap(0, Relaxed), SEC_R_HANDLER_US.swap(0, Relaxed), SEC_R_COMP_US.swap(0, Relaxed)];
    let hid = SEC_HID_MS.swap(0, Relaxed);
    let frames = SEC_FRAMES.swap(0, Relaxed);
    let strands = SEC_STRANDS.swap(0, Relaxed);
    let yields = SEC_YIELDS.swap(0, Relaxed);
    let beam = crate::video::beam::sec_take();
    let (seg_h, seg_hu, seg_p, seg_pu) = seg_take(); let masked = crate::hidstall::take_sec_masked(); // INPUTSTALL2 M1. HIDSTALL (B485): the second's longest IRQ-masked UnaFS span
    let mut w = worst(&st);
    let mut w_ms = st[w] / 1000;
    let mut w_name = STAGES[w];
    if hid > w_ms {
        w_ms = hid;
        w_name = "hid";
        w = 5;
    }
    let _ = w;
    let rw = if r[0] >= r[1] && r[0] >= r[2] { 0 } else if r[1] >= r[2] { 1 } else { 2 };
    let stall = w_ms >= STALL_MS || r[rw] / 1000 >= STALL_MS || seg_pu / 1000 >= STALL_MS; #[cfg(all(target_arch = "x86_64", feature = "wc"))] crate::video::renderwedge::roll(st[0], r[0].max(r[1]).max(r[2]), seg_h, now_ms); // RENDERWEDGE (B520): every roll feeds the render-watch (the queue age, the render task's work, its step), same-line fold. INPUTSTALL2 M1: a pump step that held the HID pass is a stall second
    // HIDSTALL2 (B509): `stage=` names what MADE the second a stall. Flight 27 printed `stage=hid stage_ms=2..8` 596 times
    // because the label was the largest of six small numbers while the trigger was the render handler (`render_ms=108`)
    // or a pump step; `stage=hid` now means the HID gap itself crossed the bound.
    if stall && w_ms < STALL_MS {
        if r[rw] / 1000 >= STALL_MS {
            w_name = ["render-route", "render-handler", "render-composite"][rw];
            w_ms = r[rw] / 1000;
        } else {
            w_name = "pump";
            w_ms = seg_pu / 1000;
        }
    }
    let (hold_ms, hold_ix) = crate::hidstall::take_sec_hold();
    // M4b (the seat, R86/QUIETBOOT): before `phase=desktop` a stall second is COUNTED, not printed; the first
    // roll at the desktop says the count in one line. R80: a measurement kept, not a test run.
    let desk = crate::boot::phase() == crate::boot::Phase::Desktop; if desk { crate::hidstall::note_sec(hid, MIN_KEYQ_US.load(Relaxed) / 1000); } // HIDSTALL (B485): the desktop's HID stall seconds and key-queue max
    if stall && !desk {
        BOOT_SUPPRESSED.fetch_add(1, Relaxed);
        let bw = w_ms.max(r[rw] / 1000);
        if bw >= BOOT_WORST_MS.load(Relaxed) {
            BOOT_WORST_MS.store(bw, Relaxed);
            BOOT_WORST_STAGE.store(if w_ms >= r[rw] / 1000 { stage_code(w_name) } else { 10 + rw as u8 }, Relaxed);
        }
    }
    if desk && !BOOT_SAID.swap(true, Relaxed) {
        let n = BOOT_SUPPRESSED.load(Relaxed);
        let (h_ms, h_end) = (BOOT_H_US.load(Relaxed) / 1000, BOOT_H_END_MS.load(Relaxed));
        serial_println!(
            "[lag] stall boot_suppressed={} worst_stage={} worst_ms={} handler_span_ms={}..{} overlaps={}",
            n, if n == 0 { "none" } else { stage_word(BOOT_WORST_STAGE.load(Relaxed)) }, BOOT_WORST_MS.load(Relaxed),
            h_end.saturating_sub(h_ms), h_end, crate::fs::bootstep::overlaps(h_end.saturating_sub(h_ms), h_end)
        );
    }
    if stall && desk {
        let n = STALL_LINES.fetch_add(1, Relaxed); if r[1] / 1000 >= STALL_MS { HANDLER_STALLS.fetch_add(1, Relaxed); } // SHELLTASK (B458): a render-handler stall second, counted
        if n < STALL_LINES_FREE || n % 64 == 0 {
            let (dw, pr) = if split == 0 {
                (alloc::string::String::from("-"), alloc::string::String::from("-"))
            } else {
                (ms_str(draw), ms_str(pre))
            };
            serial_println!(
                "[lag] stall at_ms={} span_ms={} stage={} stage_ms={} queue={} wm={} app={} comp={} present={} draw={} pre={} render={} render_ms={} passes={} pass_ms_max={} rows={} full={} beam_ms={} beam_max_ms={} capped={} yielded={} valve={} hid_gap_ms={} strand={}/{} handler={} handler_ms={} pump={} pump_ms={} masked_ms={} holder={} hold_ms={}{}{}",
                t0, now_ms.saturating_sub(t0), w_name, w_ms,
                ms_str(st[0]), ms_str(st[1]), ms_str(st[2]), ms_str(st[3]), ms_str(st[4]), dw, pr,
                ["route", "handler", "composite"][rw], ms_str(r[rw]),
                passes, ms_str(pass_max), beam[3], beam[4], beam[0] / 1000, ms_str(beam[1]), beam[5], yields,
                valve_word(), hid, strands, frames, seg_h, ms_str(seg_hu), seg_p, ms_str(seg_pu), masked,
                crate::hidstall::HolderName(hold_ix), hold_ms, QsField(w_name, masked), crate::video::loginrender::StallWhat(w_name == "render-handler", t0) // QUEUESLEEP (B512): ` pump_what= pump_wait=` when the stage is pump or queue; LOGINRENDER (B514): the trailing ` handler_what=<name>:<ms>` token stays LAST
            );
        }
    }
    // The minute.
    let m0 = MIN_T0_MS.load(Relaxed);
    if now_ms < m0 + MIN_MS || MIN_T0_MS.compare_exchange(m0, now_ms, Relaxed, Relaxed).is_err() {
        return;
    }
    if desk {
        crate::hidstall::census_minute(); // HIDSTALL2 (B509): who held the hid pump's core this minute, every minute
    }
    let ev = MIN_EVENTS.swap(0, Relaxed);
    let kq = MIN_KEYQ_US.swap(0, Relaxed) / 1000;
    let cm = MIN_COMP_US.swap(0, Relaxed) / 1000;
    let hg = MIN_HID_MS.swap(0, Relaxed);
    let fr = MIN_FRAMES.swap(0, Relaxed) as u64;
    let sd = MIN_STRANDS.swap(0, Relaxed) as u64;
    if ev == 0 {
        return;
    }
    let pct = strand_pct(sd, fr);
    serial_println!(
        ":: INPUTSTALL: key_queue_max_ms={} comp_max_ms={} hid_gap_max_ms={} strand_pct={} bound={} -> {} :: events={} frames={} strands={} strand_bound_pct={} span={}s",
        kq, cm, hg, pct, STALL_MS, crate::tests::boot_word(verdict(kq, cm, hg, pct)), // SMALLFIX7 (B501): the minute's line is a record before the deferral point
        ev, fr, sd, STRAND_PCT_BOUND, now_ms.saturating_sub(m0) / 1000
    );
}

/// HIDSTALL2 (B509): `tests inputstall` — the LIVE minute's INPUTSTALL reading so far (nothing is reset; the minute line
/// still prints at its roll). R80: registered by `hidstall::ensure`, run only by the verb.
pub fn inputstall_selftest() {
    if !ON {
        serial_println!(":: INPUTSTALL: -> SKIP :: reason=no-wc via=tests");
        return;
    }
    let ev = MIN_EVENTS.load(Relaxed);
    let kq = MIN_KEYQ_US.load(Relaxed) / 1000;
    let cm = MIN_COMP_US.load(Relaxed) / 1000;
    let hg = MIN_HID_MS.load(Relaxed);
    let fr = MIN_FRAMES.load(Relaxed) as u64;
    let sd = MIN_STRANDS.load(Relaxed) as u64;
    let pct = strand_pct(sd, fr);
    serial_println!(
        ":: INPUTSTALL: key_queue_max_ms={} comp_max_ms={} hid_gap_max_ms={} strand_pct={} bound={} -> {} :: events={} frames={} strands={} strand_bound_pct={} span={}s via=tests",
        kq, cm, hg, pct, STALL_MS, if verdict(kq, cm, hg, pct) { "PASS" } else { "FAIL" },
        ev, fr, sd, STRAND_PCT_BOUND, crate::arch::ms().saturating_sub(MIN_T0_MS.load(Relaxed)) / 1000
    );
}

/// Stranded frames as a whole percent of the frames noted (0 for none). Pure.
pub fn strand_pct(strands: u64, frames: u64) -> u64 {
    if frames == 0 { 0 } else { strands.saturating_mul(100) / frames }
}

/// The INPUTSTALL verdict over one minute's maxima. Pure.
pub fn verdict(key_queue_ms: u64, comp_ms: u64, hid_gap_ms: u64, strand_pct: u64) -> bool {
    key_queue_ms <= STALL_MS && comp_ms <= STALL_MS && hid_gap_ms <= STALL_MS && strand_pct <= STRAND_PCT_BOUND
}

// ── INPUTSTALL M4b (the seat): the bare boot stays quiet ─────────────────────────────────────────────
static BOOT_SUPPRESSED: AtomicU32 = AtomicU32::new(0);
static BOOT_WORST_MS: AtomicU64 = AtomicU64::new(0);
static BOOT_WORST_STAGE: AtomicU8 = AtomicU8::new(0);
static BOOT_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// A stall's stage as a byte: `0..5` the [`STAGES`], `5` the HID gap, `10..13` a render phase.
fn stage_code(name: &str) -> u8 {
    match STAGES.iter().position(|s| *s == name) {
        Some(i) => i as u8,
        None => match name {
            "render-route" => 10, // HIDSTALL2 (B509): the stall's trigger names, as `stage_word` reads them back
            "render-handler" => 11,
            "render-composite" => 12,
            "pump" => 13,
            _ => 5,
        },
    }
}

fn stage_word(code: u8) -> &'static str {
    match code {
        0..=4 => STAGES[code as usize],
        10 => "render-route",
        11 => "render-handler",
        12 => "render-composite",
        13 => "pump",
        _ => "hid",
    }
}

// ── INPUTSTALL2 M1 (rmbp-ledger B388): NAME the step ──────────────────────────────────────────────────
//
// Flight 24's chart said `render=handler render_ms=119.0` and `stage=hid hid_gap_ms=16016` and could not say
// WHICH handler or WHICH pump step. A step is named by a mark placed AFTER it: [`seg`] on the render task
// (the route doors, the drain tail, the drains, the panel render, the rollups) and [`pump_seg`] on the
// `usb-pump` loop. The interval since the previous mark is charged to the mark's name; the second's worst
// rides the stall line as `handler=<step> handler_ms=<n> pump=<step> pump_ms=<n>`. Max and name travel as
// ONE packed word (`us << 8 | id`) so a racing `fetch_max` can never pair one step's time with another's
// name. R80: a reading of the live path, never a test.

/// Render-task step names ([`seg`] ids index this).
pub const SEG_NAMES: [&str; 14] = [
    "-", "route-doors", "route-focus", "route-click", "route-ring", "drain-tail", "console-launch",
    "shell-launch", "cursor-instgui", "login-drain", "panel-render", "shell-present", "rollups-5s", "termsel",
];
pub const S_DOORS: u8 = 1;
pub const S_FOCUS: u8 = 2;
pub const S_CLICK: u8 = 3;
pub const S_RING: u8 = 4;
pub const S_DRAIN: u8 = 5;
pub const S_CONSOLE: u8 = 6;
pub const S_SHELL: u8 = 7;
pub const S_INSTGUI: u8 = 8;
pub const S_LOGIN: u8 = 9;
pub const S_RENDER: u8 = 10;
pub const S_PRESENT: u8 = 11;
pub const S_ROLLUPS: u8 = 12;
pub const S_TERMSEL: u8 = 13;

/// `usb-pump` step names ([`pump_seg`] ids index this).
pub const PUMP_NAMES: [&str; 17] = [
    "-", "xhci", "ehci-hid", "store-services", "prtscr", "selfhost", "desktop-app", "root-pass", "fatverb",
    "wifi", "bootlog", "console", "flight-recorder", "u-probes", "usb-summary", "net-hda", "typematic-smc",
];
pub const P_XHCI: u8 = 1;
pub const P_HID: u8 = 2;
pub const P_STORE: u8 = 3;
pub const P_PRTSCR: u8 = 4;
pub const P_SELFHOST: u8 = 5;
pub const P_DESKAPP: u8 = 6;
pub const P_ROOT: u8 = 7;
pub const P_FATVERB: u8 = 8;
pub const P_WIFI: u8 = 9;
pub const P_BOOTLOG: u8 = 10;
pub const P_CONSOLE: u8 = 11;
pub const P_FLIGHT: u8 = 12;
pub const P_UPROBES: u8 = 13;
pub const P_SUMMARY: u8 = 14;
pub const P_NET: u8 = 15;
pub const P_TYPEMATIC: u8 = 16;

/// The render task's last mark (0 = none open: parked, or not yet routed).
static SEG_LAST_US: AtomicU64 = AtomicU64::new(0);
/// The second's worst render step, packed `us << 8 | id`.
static SEC_SEG: AtomicU64 = AtomicU64::new(0);
/// The pump's last mark (the loop top re-arms it).
static PUMP_LAST_US: AtomicU64 = AtomicU64::new(0);
/// The second's worst pump step, packed `us << 8 | id`.
static SEC_PUMP: AtomicU64 = AtomicU64::new(0);

/// A route began on the render task (or the task is about to park: `open = false`).
fn seg_anchor(now: u64, open: bool) {
    SEG_LAST_US.store(if open { now } else { 0 }, Relaxed);
}

/// The render task finished step `id`: the interval since its previous mark is charged to `id`.
#[inline]
pub fn seg(id: u8) {
    if !ON || !render_here() {
        return;
    }
    let now = now_us();
    let last = SEG_LAST_US.swap(now, Relaxed); RENDER_LAST_SEG.store(id, Relaxed); // QUEUESLEEP (B512): the render task's last finished step
    if last != 0 && now > last {
        SEC_SEG.fetch_max(((now - last) << 8) | id as u64, Relaxed);
        if id == S_CONSOLE {
            crate::perf::note_svc(now - last); // PERFREVIEW (B443): the service chain's span
        }
    }
}

/// HIDSTALL2 (B509): the `usb-pump` step IN FLIGHT (its [`PUMP_NAMES`] id; 0 = between passes) — the loop's step order
/// is fixed (`main.rs`'s `usb_pump`), so the step after the last mark is the one running. Read by the pump core's tick
/// ISR to name a masked holder (`hidstall::note_hold`).
static PUMP_CUR: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// The `usb-pump` loop's step order (`main.rs`'s `usb_pump`, the marks in sequence). Pure.
pub fn pump_next(id: u8) -> u8 {
    match id {
        P_XHCI => P_HID,
        P_HID => P_TYPEMATIC,
        P_TYPEMATIC => P_STORE,
        P_STORE => P_PRTSCR,
        P_PRTSCR => P_SELFHOST,
        P_SELFHOST => P_DESKAPP,
        P_DESKAPP => P_ROOT,
        P_ROOT => P_FATVERB,
        P_FATVERB => P_WIFI,
        P_WIFI => P_BOOTLOG,
        P_BOOTLOG => P_CONSOLE,
        P_CONSOLE => P_FLIGHT,
        P_FLIGHT => P_UPROBES,
        P_UPROBES => P_SUMMARY,
        P_SUMMARY => P_NET,
        _ => 0,
    }
}

/// HIDSTALL2: the `usb-pump` step in flight (0 = none / not tracked).
#[inline]
pub fn pump_step_now() -> u8 {
    PUMP_CUR.load(Relaxed)
}

/// The `usb-pump` loop's top (after its nap): the first step's interval starts here.
#[inline]
pub fn pump_top() {
    if ON {
        let t = now_us(); PUMP_LAST_US.store(t, Relaxed); qs_nap_end(t); // QUEUESLEEP (B512): the nap that just ended, measured
        PUMP_CUR.store(P_XHCI, Relaxed);
    }
}

/// The `usb-pump` loop finished step `id`.
#[inline]
pub fn pump_seg(id: u8) {
    if !ON {
        return;
    }
    PUMP_CUR.store(pump_next(id), Relaxed);
    let now = now_us();
    let last = PUMP_LAST_US.swap(now, Relaxed); qs_step(id, now, last); // QUEUESLEEP (B512): what the input waited on during this step
    if last != 0 && now > last {
        SEC_PUMP.fetch_max(((now - last) << 8) | id as u64, Relaxed);
    }
}

/// Unpack `(name, us)` from a packed step word. Pure.
pub fn seg_unpack(w: u64, names: &[&'static str]) -> (&'static str, u64) {
    let id = (w & 0xff) as usize;
    (names.get(id).copied().unwrap_or("-"), w >> 8)
}

/// The second's two worst steps, taken (reset) at the roll: `(handler, handler_us, pump, pump_us)`.
fn seg_take() -> (&'static str, u64, &'static str, u64) {
    let (h, hu) = seg_unpack(SEC_SEG.swap(0, Relaxed), &SEG_NAMES);
    let (p, pu) = qs_roll(seg_unpack(SEC_PUMP.swap(0, Relaxed), &PUMP_NAMES)); // QUEUESLEEP (B512): an overslept nap is a pump stall too; the second's waits are taken here
    (h, hu, p, pu)
}

// SHELLTASK (rmbp-ledger B458) — the render-handler stall seconds at the desktop (`[lag] stall … render=handler`
// class: the render task held over STALL_MS inside a handler — the shell on the render task was the flown cause).
static HANDLER_STALLS: AtomicU32 = AtomicU32::new(0);
/// Render-handler stall seconds since boot (`tests shelltask` reads the delta across a running verb).
pub fn handler_stalls() -> u32 {
    HANDLER_STALLS.load(Relaxed)
}

// ── SMALLFIX6 (rmbp-ledger B495): NAME the boot's render-handler stall ───────────────────────────────────
// Flight 26: `worst_stage=render-handler worst_ms=6094|6086|6115` on all three boots, and nothing said during
// what. Boot 1 root-mount 590 + assoc-seed 5498 = 6088 and boot 3 552 + 5562 = 6114: the render task made no
// route/pass/park across `users::service`'s BOOT80 steps on the usb-pump (the flown tree wrote the type registry
// at boot) — it waited on the UnaFS volume those steps held. The boot line now carries the longest handler
// interval's span and the boot steps it overlapped (`fs::bootstep::overlaps`), so the next flight names it.
static BOOT_H_US: AtomicU64 = AtomicU64::new(0);
static BOOT_H_END_MS: AtomicU64 = AtomicU64::new(0);

/// Before the boot line is said: keep the longest render-handler interval and the kernel ms it ended at.
fn boot_handler_note(d_us: u64) {
    if !BOOT_SAID.load(Relaxed) && d_us > BOOT_H_US.load(Relaxed) {
        BOOT_H_US.store(d_us, Relaxed);
        BOOT_H_END_MS.store(crate::arch::ms(), Relaxed);
    }
}

/// `tests smallfix6`: the boot's longest render-handler interval `(ms, end_ms)`.
pub fn boot_handler_span() -> (u64, u64) {
    (BOOT_H_US.load(Relaxed) / 1000, BOOT_H_END_MS.load(Relaxed))
}

// ── QUEUESLEEP (rmbp-ledger B512, R103 §2): NAME what the input waited on ─────────────────────────────────────────────
//
// Flight 28, an idle desktop: `stage=pump stage_ms=28440 pump=selfhost`, `stage=queue stage_ms=3090` and `stage_ms=35413`,
// and `span_ms=24156` gaps — and the line said WHICH step held the loop but not what the INPUT was waiting on while it did.
// This section stamps every wait on the input path in one atomic each (HIDSTALL2's shape) and puts the answer on the
// stall line, only when the stage is `pump` or `queue`: ` pump_what=<fn> pump_wait=<word>`.
//
// * `pump` — the second's longest `usb-pump` interval: a step ([`pump_seg`]) or the loop's nap ([`pump_nap`] →
//   [`pump_top`], `pump_what=nap pump_wait=hlt` when the core did not give the loop back within the tick). For a step,
//   `pump_wait` is what the xHCI INPUT poll did meanwhile: `run` (it kept polling elsewhere — the step held only the
//   loop), `poll-gap` (no poll for an input interval, [`HID_INTERVAL_US`]) or `claim-busy` (the controller was on loan
//   for that long, every retry refused).
// * `queue` — a key or press is in the ring and not routed. `pump_what=input` with `pump_wait=app-gate` (a full-screen
//   app owns the ring), or `hlt` (the input task has not come back from its nap); `pump_what=render@<last step>` with
//   `channel-full` (the channel refused: the render task is not taking), `hlt` (parked on the channel, not woken),
//   `lock` (off the channel in the second an IRQ-masked UnaFS span passed the bound) or `handler` (off the channel,
//   inside the step after the one named).
//
// `tests queuesleep` (crate::queuesleep) reads the same stamps over a 30 s idle window. R80: the stamps are atomics on
// paths that already run; nothing here prints at boot.

/// `pump_wait=` words, indexed by the `W_*` codes.
pub const WAIT_NAMES: [&str; 9] = ["-", "hlt", "lock", "poll-gap", "claim-busy", "app-gate", "channel-full", "handler", "run"];
pub const W_NONE: u8 = 0;
pub const W_HLT: u8 = 1;
pub const W_LOCK: u8 = 2;
pub const W_POLLGAP: u8 = 3;
pub const W_CLAIM: u8 = 4;
pub const W_APPGATE: u8 = 5;
pub const W_CHANFULL: u8 = 6;
pub const W_HANDLER: u8 = 7;
pub const W_RUN: u8 = 8;
/// One HID input interval (a boot mouse's / keyboard's 8 ms `bInterval`): the longest the xHCI input poll may wait.
pub const HID_INTERVAL_US: u64 = 8_000;
/// The `usb-pump` loop's nap, as a step id (beyond [`PUMP_NAMES`]).
const STEP_NAP: u8 = 0xFE;
/// The input task's stamp older than this means it has not come back from its nap.
const INPUT_STALE_MS: u64 = 50;
/// The fixture's probe: a `KeyUp` with this payload (no key carries it; every consumer ignores a `KeyUp`).
pub const PROBE_KEY: u8 = 0;

static RENDER_PARKED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static RENDER_LAST_SEG: AtomicU8 = AtomicU8::new(0);
static NAP_T0_US: AtomicU64 = AtomicU64::new(0);
static XPOLL_LAST_US: AtomicU64 = AtomicU64::new(0);
static XBUSY_SINCE_US: AtomicU64 = AtomicU64::new(0);
/// The input task's last stamp: `ms << 8 | W_*`.
static INPUT_WAIT: AtomicU64 = AtomicU64::new(0);
/// The second's worst pump interval, packed by [`qs_pack`]; taken at the roll into [`ROLL_PWAIT`] for the line.
static SEC_PWAIT: AtomicU64 = AtomicU64::new(0);
static ROLL_PWAIT: AtomicU64 = AtomicU64::new(0);
// The fixture's window (reset by `qs_window_reset`).
static QS_PUMP: AtomicU64 = AtomicU64::new(0);
static QS_QUEUE_US: AtomicU64 = AtomicU64::new(0);
static QS_XGAP_US: AtomicU64 = AtomicU64::new(0);
static QS_BUSY: AtomicU32 = AtomicU32::new(0);
static QS_NAP_US: AtomicU64 = AtomicU64::new(0);
static PROBE_T0_US: AtomicU64 = AtomicU64::new(0);
static PROBE_DONE_US: AtomicU64 = AtomicU64::new(0);

/// `us << 16 | step << 8 | code` — the longest interval wins a `fetch_max`, and its step and wait travel with it. Pure.
pub fn qs_pack(us: u64, step: u8, code: u8) -> u64 {
    (us.min(u64::MAX >> 16) << 16) | ((step as u64) << 8) | code as u64
}

/// `(us, step, code)` from [`qs_pack`]. Pure.
pub fn qs_unpack(w: u64) -> (u64, u8, u8) {
    (w >> 16, ((w >> 8) & 0xff) as u8, (w & 0xff) as u8)
}

/// A pump step id as the line names it (`nap` for the loop's nap). Pure.
pub fn qs_step_name(step: u8) -> &'static str {
    if step == STEP_NAP { "nap" } else { PUMP_NAMES.get(step as usize).copied().unwrap_or("-") }
}

/// A `W_*` code's word. Pure.
pub fn wait_name(code: u8) -> &'static str {
    WAIT_NAMES.get(code as usize).copied().unwrap_or("-")
}

/// What a pump step's input poll did, from the poll gap and the Busy streak at the step's end. Pure.
pub fn qs_step_wait(xgap_us: u64, busy_us: u64) -> u8 {
    if xgap_us < HID_INTERVAL_US {
        W_RUN
    } else if busy_us >= HID_INTERVAL_US {
        W_CLAIM
    } else {
        W_POLLGAP
    }
}

/// Where a key sits when it is not routed, from the input task's and the render task's stamps. Pure.
/// `input` = the input task's last `W_*`, `stale` = that stamp is older than [`INPUT_STALE_MS`].
/// Returns `(render?, code)`: `false` names the input task, `true` the render task.
pub fn qs_queue_wait(input: u8, stale: bool, parked: bool, masked_ms: u64) -> (bool, u8) {
    if input == W_APPGATE {
        (false, W_APPGATE)
    } else if stale {
        (false, W_HLT)
    } else if input == W_CHANFULL {
        (true, W_CHANFULL)
    } else if parked {
        (true, W_HLT)
    } else if masked_ms >= STALL_MS {
        (true, W_LOCK)
    } else {
        (true, W_HANDLER)
    }
}

/// The `usb-pump` loop is about to nap (`main.rs`'s `usb_pump`, before `sleep_ticks(1)`).
#[inline]
pub fn pump_nap() {
    if ON {
        NAP_T0_US.store(now_us(), Relaxed);
    }
}

fn qs_nap_end(t: u64) {
    let t0 = NAP_T0_US.swap(0, Relaxed);
    if t0 != 0 && t > t0 {
        let w = qs_pack(t - t0, STEP_NAP, W_HLT);
        QS_NAP_US.fetch_max(t - t0, Relaxed);
        SEC_PWAIT.fetch_max(w, Relaxed);
        QS_PUMP.fetch_max(w, Relaxed);
    }
}

fn qs_step(id: u8, now: u64, last: u64) {
    if last == 0 || now <= last {
        return;
    }
    let xl = XPOLL_LAST_US.load(Relaxed);
    let xgap = if xl == 0 { 0 } else { now.saturating_sub(xl) };
    let bs = XBUSY_SINCE_US.load(Relaxed);
    let busy = if bs == 0 { 0 } else { now.saturating_sub(bs) };
    let w = qs_pack(now - last, id, qs_step_wait(xgap, busy));
    SEC_PWAIT.fetch_max(w, Relaxed);
    QS_PUMP.fetch_max(w, Relaxed);
}

/// A successful xHCI event poll (the `usb-pump`'s, the `xhci-poll` task's, a full-screen app's `pump_and_poll`).
#[inline]
pub fn xhci_polled() {
    if !ON {
        return;
    }
    let now = now_us();
    let prev = XPOLL_LAST_US.swap(now, Relaxed);
    if prev != 0 && now > prev {
        QS_XGAP_US.fetch_max(now - prev, Relaxed);
    }
    XBUSY_SINCE_US.store(0, Relaxed);
}

/// An xHCI claim refused `Busy` (the controller is on loan to another context).
#[inline]
pub fn xhci_busy() {
    if ON {
        let _ = XBUSY_SINCE_US.compare_exchange(0, now_us().max(1), Relaxed, Relaxed);
        QS_BUSY.fetch_add(1, Relaxed);
    }
}

/// The input task's state at the end of its pass (`main.rs`'s `input_service`): `W_APPGATE`, `W_CHANFULL` or `W_HLT`.
#[inline]
pub fn input_wait(code: u8) {
    if ON {
        INPUT_WAIT.store(crate::arch::ms() << 8 | code as u64, Relaxed);
    }
}

/// The roll's pump word: an overslept nap longer than every step is the second's pump interval. Takes the second's waits.
fn qs_roll(step: (&'static str, u64)) -> (&'static str, u64) {
    let w = SEC_PWAIT.swap(0, Relaxed);
    ROLL_PWAIT.store(w, Relaxed);
    let (us, s, _) = qs_unpack(w);
    if s == STEP_NAP && us > step.1 { ("nap", us) } else { step }
}

/// The queue's word now: `(what, wait)` — `input` or `render@<last step>`.
pub fn qs_queue_word(masked_ms: u64) -> (alloc::string::String, &'static str) {
    let iw = INPUT_WAIT.load(Relaxed);
    let stale = iw != 0 && crate::arch::ms().saturating_sub(iw >> 8) > INPUT_STALE_MS;
    let (render, code) = qs_queue_wait((iw & 0xff) as u8, stale, RENDER_PARKED.load(Relaxed), masked_ms);
    let what = if render {
        alloc::format!("render@{}", SEG_NAMES.get(RENDER_LAST_SEG.load(Relaxed) as usize).copied().unwrap_or("-"))
    } else {
        alloc::string::String::from("input")
    };
    (what, wait_name(code))
}

/// The stall line's QUEUESLEEP field: ` pump_what=<fn> pump_wait=<word>` for a `pump` or `queue` stage, empty otherwise.
struct QsField(&'static str, u64);
impl core::fmt::Display for QsField {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            "pump" => {
                let (_, s, c) = qs_unpack(ROLL_PWAIT.load(Relaxed));
                write!(f, " pump_what={} pump_wait={}", qs_step_name(s), wait_name(c))
            }
            "queue" => {
                let (what, wait) = qs_queue_word(self.1);
                write!(f, " pump_what={} pump_wait={}", what, wait)
            }
            _ => Ok(()),
        }
    }
}

/// `route`'s probe check: the fixture's `KeyUp(PROBE_KEY)` reached the router.
#[inline]
fn qs_probe_route(ev: &Event) {
    if matches!(ev, Event::KeyUp(k) if *k == PROBE_KEY) && PROBE_T0_US.load(Relaxed) != 0 {
        let _ = PROBE_DONE_US.compare_exchange(0, now_us().max(1), Relaxed, Relaxed);
    }
}

/// Is the instrument live (x86 + `wc`)?
pub fn qs_on() -> bool {
    ON
}

/// The fixture's window starts: every window maximum is reset.
pub fn qs_window_reset() {
    for a in [&QS_PUMP, &QS_QUEUE_US, &QS_XGAP_US, &QS_NAP_US] {
        a.store(0, Relaxed);
    }
    QS_BUSY.store(0, Relaxed);
}

/// The window so far: `(pump packed, queue_us, xhci poll gap max us, busy claims, nap max us)`.
pub fn qs_window() -> (u64, u64, u64, u32, u64) {
    (QS_PUMP.load(Relaxed), QS_QUEUE_US.load(Relaxed), QS_XGAP_US.load(Relaxed), QS_BUSY.load(Relaxed), QS_NAP_US.load(Relaxed))
}

/// The oldest key or press still in the queue right now, in us (0 = none) — the in-flight charge, without a roll.
pub fn qs_queue_now_us() -> u64 {
    let nu = now_us();
    let mut m = 0;
    for s in &SLOTS {
        if s.stage.load(Acquire) == ISR {
            let t = s.t[0].load(Relaxed);
            if t != 0 && nu > t {
                m = m.max(nu - t);
            }
        }
    }
    m
}

/// Arm the probe at `now_us`: the next `KeyUp(PROBE_KEY)` the router takes is timed from here.
pub fn qs_probe_arm() -> u64 {
    PROBE_DONE_US.store(0, Relaxed);
    let t = now_us().max(1);
    PROBE_T0_US.store(t, Relaxed);
    t
}

/// The probe's route stamp (0 = not yet); disarms when read non-zero.
pub fn qs_probe_done() -> u64 {
    let d = PROBE_DONE_US.load(Relaxed);
    if d != 0 {
        PROBE_T0_US.store(0, Relaxed);
    }
    d
}

/// The microsecond clock the stamps use.
pub fn qs_now_us() -> u64 {
    now_us()
}
