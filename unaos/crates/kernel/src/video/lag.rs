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
}

impl Slot {
    const fn new() -> Self {
        Slot {
            stage: AtomicU8::new(IDLE),
            kind: AtomicU8::new(K_KEY),
            ring: AtomicU8::new(0),
            seq: AtomicU32::new(0),
            t: [const { AtomicU64::new(0) }; 6],
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
static SPAN: spin::Mutex<Span> = spin::Mutex::new(Span { n: 0, by_kind: [0; 4], ms: [0; SAMPLES], max_ms: 0, max_kind: K_KEY, stage_us: [0; 5] });
static TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static ORPHANS: AtomicU32 = AtomicU32::new(0);
static COALESCED: AtomicU32 = AtomicU32::new(0);
static LOST: AtomicU32 = AtomicU32::new(0);
static NEXT_ROLLUP_MS: AtomicU64 = AtomicU64::new(0);
/// A finished event whose line is owed (printed from an unmasked context): packed by `finish`.
static OWED: spin::Mutex<[Option<Done>; 4]> = spin::Mutex::new([None; 4]);

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
    let s = &SLOTS[c];
    if s.stage.compare_exchange(IDLE, ISR, AcqRel, Relaxed).is_ok() {
        s.kind.store(kind, Relaxed);
        s.ring.store(0, Relaxed);
        s.t[0].store(now_us(), Release);
    } else {
        COALESCED.fetch_add(1, Relaxed);
    }
}

/// The router's guard (`wc_route_event`): the event is off the ring (`queue` ends) and, when the guard
/// drops without a ring-3 delivery or a launch having claimed it, a kernel surface consumed it (`wm` and
/// `app` end together).
pub struct RouteGuard(Option<usize>);

pub fn route(ev: &Event) -> RouteGuard {
    if !ON {
        return RouteGuard(None);
    }
    let Some((c, _)) = class_of(ev) else {
        return RouteGuard(None);
    };
    if SLOTS[c].advance(ISR, QUEUE, now_us()) { RouteGuard(Some(c)) } else { RouteGuard(None) }
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        if let Some(c) = self.0 {
            let _ = SLOTS[c].advance(QUEUE, APP, now_us());
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
pub struct PassGuard;

pub fn pass() -> PassGuard {
    if ON {
        let now = now_us();
        for s in &SLOTS {
            if s.stage.load(Acquire) == APP && s.ring.load(Relaxed) == 0 {
                let _ = s.advance(APP, COMP, now);
            }
        }
    }
    PassGuard
}

impl Drop for PassGuard {
    fn drop(&mut self) {
        if !ON {
            return;
        }
        let now = now_us();
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
