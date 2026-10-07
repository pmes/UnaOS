// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B485 HIDSTALL, R103: the pointer's halt, stall and dock release, counted)
//!
//! HIDSTALL (rmbp-ledger B485) — flight 26 read `STOP-NOTE … addr=5|6 class=xact-err-burn` at session open,
//! `[lag] stall … stage=hid hid_gap_ms=49 pump=desktop-app pump_ms=2xx` every 2 s, a 3–4 s key queue and a dock
//! press whose release never launched. The causes, as the wire names them (docs/dev/evidence/rmbp-1005/hidstall.md):
//!
//! * the halted endpoints are the BT HID PROXY (`05ac:820a`/`820b`), burned the moment BTHID's HCI Reset takes the
//!   radio — they carry no input; a halted INPUT endpoint is now cleared and re-armed (`[hid] recovered …`);
//! * the 49 ms gap is the Dock's 2 s Trash poll — a UnaFS query, and every UnaFS transaction is IRQ-masked, so the
//!   `hid-pump` task on the service core cannot run for its length; the Trash is now read on a change only;
//! * a pin press arms a launch-on-release; no release within [`DOCK_RELEASE_TIMEOUT_MS`] launches it.
//!
//! This file holds the arc's counters (atomics, every arch) and `tests hidstall` (R80: behind the verb, nothing at
//! boot): `:: HIDSTALL: halted_at_login=<n> recovered=<n> proxy_retired=<n> hid_stall_s=<n> key_queue_max_ms=<n>
//! dock_timeouts=<n> masked_max_ms=<n> -> PASS|FAIL ::` — PASS iff no input endpoint stayed halted, no desktop second
//! had a HID gap at or over [`HID_GAP_STALL_MS`], and no key sat in the queue over the INPUTSTALL bound.
//!
//! HIDSTALL2 (rmbp-ledger B509, R103 §2) — what flight 27 left (docs/dev/evidence/rmbp-1005/hidstall2.md):
//!
//! * THE HOLDER CENSUS. A masked span on the hid pump's core freezes that core's LAPIC tick; the first tick after it is
//!   taken the instant interrupts come back, ON the task that held them. LOCKREG's per-core tick (`sync::isr_tick`)
//!   measures the tick-to-tick gap on the pump core and, past [`HOLD_OVER_MS`], charges it here to the holder: the task,
//!   the `usb-pump` step in flight ([`crate::video::lag::pump_step_now`]) and the outermost lock the task still holds
//!   (LOCKREG's acquire site — the caller's own function). ISR-side: atomics only, one writer (the pump core). The stall
//!   line carries the second's worst as `holder=<name> hold_ms=<n>`; once a minute at the desktop
//!   `[hid] stall census holder=<name> n=<n> worst_ms=<n> session_n=<n> session_worst_ms=<n>` per holder seen, then
//!   `[hid] stall census holders=<k> worst_ms=<n> worst=<name> pump_cpu=<c> -> CLEAR|HELD` (CLEAR = no hold over 4 ms).
//! * THE RELEASE LANE. The Dock's release timeout runs on the `usb-pump` (`lp_service`), the release is ROUTED by the
//!   render task; flight 27's `release=timeout after_ms=400` fired while the render lane was busy (`comp=2816`) — the
//!   release was in the pal ring, not lost. The decoders stamp each button edge here ([`note_button_edge`]) so the Dock
//!   can wait for a queued release and say how long it sat (`lane_ms=`).
//! * THE TRACKPAD ON THE WIRE. [`tp_wire_due`] paces `[tp] mt` to one serial line per [`TP_WIRE_MS`] while a finger is
//!   down, so the pointer route never goes dark (QUIETBOOT moved every other `[tp]` line to the bootlog).
//! * `tests inputstall` — the live minute's INPUTSTALL reading, on demand (R80).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};

/// A pin press with no release (and no travel) for this long launches (DOCKRELEASE).
pub const DOCK_RELEASE_TIMEOUT_MS: u64 = 400;
/// A desktop second whose longest gap between two HID passes reached this is a HID stall second (one 60 Hz frame:
/// the pass cadence is one 1 ms tick, so a frame without a pass is a pointer the glass sees stutter).
pub const HID_GAP_STALL_MS: u64 = 16;
/// The key-queue bound the fixture reads (INPUTSTALL's `bound=`).
pub const KEY_QUEUE_BOUND_MS: u64 = 50; // = `video::lag::STALL_MS`

static HALTED: AtomicU32 = AtomicU32::new(0);
static RECOVERED: AtomicU32 = AtomicU32::new(0);
static PROXY_RETIRED: AtomicU32 = AtomicU32::new(0);
static HID_STALL_S: AtomicU32 = AtomicU32::new(0);
static KEYQ_MAX_MS: AtomicU64 = AtomicU64::new(0);
static DOCK_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static MASKED_MAX_MS: AtomicU64 = AtomicU64::new(0);
static SEC_MASKED_MS: AtomicU64 = AtomicU64::new(0);
/// Per interrupt-endpoint index: when it halted (ms), for the recovery's `after_ms=`.
static HALT_AT: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];

/// Is a halted interrupt endpoint the BT HID proxy (BTPROXY): burned by transaction errors, never a report, a boot
/// keyboard/mouse packet size (the internal keyboard is mps 10), on a controller that claimed a BT radio. Pure.
pub fn is_proxy(class: &str, reports: u32, mps: u16, radio: bool) -> bool {
    radio && class == "xact-err-burn" && reports == 0 && mps <= 8
}

/// May an `xact-err-burn` halt be cleared? Yes for an endpoint that has carried input (a burst of bus errors on a live
/// device, not a device saying no); never for the proxy (its EP0 would only time out under the HID lock). Pure.
pub fn xact_recoverable(class: &str, proxy: bool, reports: u32) -> bool {
    class == "xact-err-burn" && !proxy && reports > 0
}

/// The service walk saw endpoint `ep_i` halt.
pub fn note_halt(ep_i: usize) {
    if let Some(a) = HALT_AT.get(ep_i) {
        a.store(crate::arch::ms().max(1), Relaxed);
    }
}

/// An endpoint was retired: the proxy is counted apart (one line says what it is); any other is a halt that stayed.
pub fn note_retired(ctl: usize, addr: u8, ep: u8, kind: &str, proxy: bool, reports: u32) {
    if proxy {
        PROXY_RETIRED.fetch_add(1, Relaxed);
        serial_println!(
            "[hid] proxy-retired ctl={} addr={} ep=IN{} kind={} reports={} why=bt-hci-owns-radio (the BT HID proxy; input rides the internal keyboard and trackpad)",
            ctl, addr, ep, kind, reports
        );
    } else {
        HALTED.fetch_add(1, Relaxed);
        serial_println!("[hid] halted ctl={} addr={} ep=IN{} kind={} reports={} -> retired (HIDSTALL: not recovered)", ctl, addr, ep, kind, reports);
    }
}

/// A halted endpoint was cleared (ClearFeature ENDPOINT_HALT) and re-armed.
pub fn note_recovered(ctl: usize, addr: u8, ep: u8, ep_i: usize) {
    RECOVERED.fetch_add(1, Relaxed);
    let t = HALT_AT.get(ep_i).map(|a| a.swap(0, Relaxed)).unwrap_or(0);
    let after = if t == 0 { 0 } else { crate::arch::ms().saturating_sub(t) };
    serial_println!("[hid] recovered ctl={} addr={} ep=IN{} after_ms={}", ctl, addr, ep, after);
}

/// `lag`'s second roll at the desktop: the second's longest HID gap and the minute's key-queue max so far.
pub fn note_sec(hid_gap_ms: u64, keyq_ms: u64) {
    if hid_gap_ms >= HID_GAP_STALL_MS {
        HID_STALL_S.fetch_add(1, Relaxed);
    }
    KEYQ_MAX_MS.fetch_max(keyq_ms, Relaxed);
}

/// Microseconds on a clock that runs through a masked span (the TSC on x86; the tick clock elsewhere).
pub fn now_us() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let hz = crate::arch::x86_64::apic::tsc_hz();
        if hz >= 1_000_000 {
            return crate::arch::now_cycles() / (hz / 1_000_000);
        }
    }
    crate::arch::ms().saturating_mul(1000)
}

/// One UnaFS transaction attempt began at `t0_us` ([`now_us`]) and its IRQ-masked span just ended.
pub fn note_masked_since(t0_us: u64) {
    note_masked(now_us().saturating_sub(t0_us) / 1000);
}

/// One UnaFS transaction attempt's IRQ-masked span (ms).
pub fn note_masked(ms: u64) {
    SEC_MASKED_MS.fetch_max(ms, Relaxed);
    MASKED_MAX_MS.fetch_max(ms, Relaxed);
    SCOPE_MASKED_MS.fetch_max(ms, Relaxed); // REGISTRYCHUNK (B508): the scoped max ([`scope_masked_take`])
}

/// The second's longest masked UnaFS span, taken (reset) at `lag`'s roll.
pub fn take_sec_masked() -> u64 {
    SEC_MASKED_MS.swap(0, Relaxed)
}

/// A pin press timed out into its launch.
pub fn note_dock_timeout() {
    DOCK_TIMEOUTS.fetch_add(1, Relaxed);
}

/// The verdict. Pure.
pub fn verdict(halted: u32, hid_stall_s: u32, keyq_ms: u64) -> bool {
    halted == 0 && hid_stall_s == 0 && keyq_ms <= KEY_QUEUE_BOUND_MS
}

/// Register `tests hidstall` (and HIDSTALL2's `tests inputstall`) once.
pub fn ensure() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::tests::register("hidstall", selftest);
        crate::tests::register("inputstall", crate::video::lag::inputstall_selftest);
    }
}

fn selftest() {
    let h = HALTED.load(Relaxed);
    let s = HID_STALL_S.load(Relaxed);
    let k = KEYQ_MAX_MS.load(Relaxed);
    serial_println!(
        ":: HIDSTALL: halted_at_login={} recovered={} proxy_retired={} hid_stall_s={} key_queue_max_ms={} dock_timeouts={} masked_max_ms={} -> {} :: hid_gap_bound_ms={} key_bound_ms={} release_timeout_ms={} holders={} hold_worst_ms={} hold_worst={} releases_waited={} lane_max_ms={} pump_cpu={}",
        h, RECOVERED.load(Relaxed), PROXY_RETIRED.load(Relaxed), s, k, DOCK_TIMEOUTS.load(Relaxed), MASKED_MAX_MS.load(Relaxed),
        if verdict(h, s, k) { "PASS" } else { "FAIL" }, HID_GAP_STALL_MS, KEY_QUEUE_BOUND_MS, DOCK_RELEASE_TIMEOUT_MS,
        holders_seen(), session_worst().1, HolderName(session_worst().0), RELEASES_WAITED.load(Relaxed), LANE_MAX_MS.load(Relaxed), CpuWord(pump_cpu())
    );
}

/// REGISTRYCHUNK (rmbp-ledger B508): the longest masked UnaFS span on ANY core since the last take — a step reads
/// it around itself (the registry build: one take before, one per object) to say its `masked_ms=`.
static SCOPE_MASKED_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The scoped masked max since the last take (ms), reset.
pub fn scope_masked_take() -> u64 {
    SCOPE_MASKED_MS.swap(0, Relaxed)
}

// ── HIDSTALL2 (B509): the hid pump's core, and who held it ─────────────────────────────────────────────────────────────

/// A masked hold on the pump core longer than this is a holder the census names (R103 §2: the brief's 4 ms — a quarter
/// of [`HID_GAP_STALL_MS`], so four such holds cannot hide inside one stall frame).
pub const HOLD_OVER_MS: u64 = 4;
/// One `[tp] mt` serial line per this many ms while a finger is down (the brief's 10 s).
pub const TP_WIRE_MS: u64 = 10_000;
/// A pin press whose release is IN THE LANE (pushed, not yet routed) waits for it up to this long from the press, then
/// the timeout takes it — a bound on an advisory counter (`pal::release_edge_pending` ages out, it is never exact).
pub const DOCK_RELEASE_LANE_CAP_MS: u64 = 2_000;

static PUMP_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);

/// `ehci::hid_task_start`: the `hid-pump` task lives on `cpu`.
pub fn note_pump_cpu(cpu: usize) {
    PUMP_CPU.store(cpu, Relaxed);
}

/// The hid pump's core (`usize::MAX` = not started).
#[inline]
pub fn pump_cpu() -> usize {
    PUMP_CPU.load(Relaxed)
}

struct CpuWord(usize);
impl core::fmt::Display for CpuWord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == usize::MAX { f.write_str("-") } else { write!(f, "{}", self.0) }
    }
}

const HOLDERS: usize = 16;

/// One holder: the task name (`&'static str` as pointer + length), the outermost held lock's acquire site (a
/// `&'static Location`, 0 = none), the `usb-pump` step in flight (0 = not the pump), and its counts. Written by the pump
/// core's tick ISR only (one writer); `ready` publishes the key.
struct Holder {
    ready: AtomicBool,
    name_p: AtomicUsize,
    name_l: AtomicUsize,
    site: AtomicUsize,
    step: AtomicU8,
    n_min: AtomicU32,
    worst_min: AtomicU64,
    n_all: AtomicU32,
    worst_all: AtomicU64,
}

impl Holder {
    const fn new() -> Self {
        Holder {
            ready: AtomicBool::new(false),
            name_p: AtomicUsize::new(0),
            name_l: AtomicUsize::new(0),
            site: AtomicUsize::new(0),
            step: AtomicU8::new(0),
            n_min: AtomicU32::new(0),
            worst_min: AtomicU64::new(0),
            n_all: AtomicU32::new(0),
            worst_all: AtomicU64::new(0),
        }
    }
}

static HOLD: [Holder; HOLDERS] = [const { Holder::new() }; HOLDERS];
/// Holds that found the table full (counted, never dropped silently).
static HOLD_OVERFLOW: AtomicU32 = AtomicU32::new(0);
/// The second's worst hold on the pump core, packed `ms << 8 | (index + 1)`; taken at `lag`'s roll.
static SEC_HOLD: AtomicU64 = AtomicU64::new(0);

/// The pump core's tick ISR (LOCKREG `sync::isr_tick`): the tick before this one was `hold_ms` late — interrupts were
/// masked on this core by `name` (the current task), inside pump step `step` (0 = not the pump), holding `site`
/// (LOCKREG's outermost acquire site, 0 = none). ISR context: atomics only.
pub fn note_hold(hold_ms: u64, name: Option<&'static str>, site: usize, step: u8) {
    if hold_ms <= HOLD_OVER_MS {
        return;
    }
    let (np, nl) = name.map(|n| (n.as_ptr() as usize, n.len())).unwrap_or((0, 0));
    let mut ix = usize::MAX;
    for (i, h) in HOLD.iter().enumerate() {
        if !h.ready.load(Acquire) {
            h.name_p.store(np, Relaxed);
            h.name_l.store(nl, Relaxed);
            h.site.store(site, Relaxed);
            h.step.store(step, Relaxed);
            h.ready.store(true, Release);
            ix = i;
            break;
        }
        if h.name_p.load(Relaxed) == np && h.site.load(Relaxed) == site && h.step.load(Relaxed) == step {
            ix = i;
            break;
        }
    }
    let Some(h) = HOLD.get(ix) else {
        HOLD_OVERFLOW.fetch_add(1, Relaxed);
        return;
    };
    h.n_min.fetch_add(1, Relaxed);
    h.worst_min.fetch_max(hold_ms, Relaxed);
    h.n_all.fetch_add(1, Relaxed);
    h.worst_all.fetch_max(hold_ms, Relaxed);
    SEC_HOLD.fetch_max(hold_ms << 8 | (ix as u64 + 1), Relaxed);
}

/// The second's worst pump-core hold, taken (reset) at `lag`'s roll: `(ms, holder index or usize::MAX)`.
pub fn take_sec_hold() -> (u64, usize) {
    let w = SEC_HOLD.swap(0, Relaxed);
    if w == 0 { (0, usize::MAX) } else { (w >> 8, (w & 0xff) as usize - 1) }
}

/// A holder's name: `<task>[:<pump step>][@<file>:<line>]` (`-` for none, `?` for an unnamed task).
pub struct HolderName(pub usize);
impl core::fmt::Display for HolderName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Some(h) = HOLD.get(self.0).filter(|h| h.ready.load(Acquire)) else { return f.write_str("-") };
        let (np, nl) = (h.name_p.load(Relaxed), h.name_l.load(Relaxed));
        if np == 0 {
            f.write_str("?")?;
        } else {
            // SAFETY: `np`/`nl` are a task's `&'static str` name, stored whole by `note_hold`.
            let name = unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(np as *const u8, nl)) };
            f.write_str(name)?;
        }
        let step = h.step.load(Relaxed);
        if step != 0 {
            write!(f, ":{}", crate::video::lag::PUMP_NAMES.get(step as usize).copied().unwrap_or("-"))?;
        }
        let site = h.site.load(Relaxed);
        if site != 0 {
            // SAFETY: `site` is a `&'static Location` (LOCKREG's acquire site), stored whole by `note_hold`.
            let l: &'static core::panic::Location<'static> = unsafe { &*(site as *const core::panic::Location<'static>) };
            let file = l.file();
            write!(f, "@{}:{}", file.rsplit('/').next().unwrap_or(file), l.line())?;
        }
        Ok(())
    }
}

/// Holders named this session.
fn holders_seen() -> usize {
    HOLD.iter().filter(|h| h.ready.load(Acquire)).count()
}

/// The session's worst holder: `(index, ms)` (`usize::MAX` for none).
fn session_worst() -> (usize, u64) {
    let mut best = (usize::MAX, 0u64);
    for (i, h) in HOLD.iter().enumerate() {
        let w = h.worst_all.load(Relaxed);
        if h.ready.load(Acquire) && w > best.1 {
            best = (i, w);
        }
    }
    best
}

/// `lag`'s minute roll at the desktop (an unmasked context): one line per holder seen in the minute (at most 8; the rest
/// counted on the summary), then the summary — always, so a quiet desktop reads `holders=0 -> CLEAR` on the wire.
pub fn census_minute() {
    let (mut k, mut said, mut worst, mut wix) = (0usize, 0usize, 0u64, usize::MAX);
    for (i, h) in HOLD.iter().enumerate() {
        if !h.ready.load(Acquire) {
            continue;
        }
        let n = h.n_min.swap(0, Relaxed);
        let w = h.worst_min.swap(0, Relaxed);
        if n == 0 {
            continue;
        }
        k += 1;
        if w > worst {
            worst = w;
            wix = i;
        }
        if said < 8 {
            said += 1;
            serial_println!(
                "[hid] stall census holder={} n={} worst_ms={} session_n={} session_worst_ms={}",
                HolderName(i), n, w, h.n_all.load(Relaxed), h.worst_all.load(Relaxed)
            );
        }
    }
    serial_println!(
        "[hid] stall census holders={} worst_ms={} worst={} unnamed={} overflow={} pump_cpu={} over_ms={} -> {}",
        k, worst, HolderName(wix), k - said, HOLD_OVERFLOW.load(Relaxed), CpuWord(pump_cpu()), HOLD_OVER_MS,
        if k == 0 { "CLEAR" } else { "HELD" }
    );
}

// ── HIDSTALL2 (B509): the release lane ─────────────────────────────────────────────────────────────────────────────────

static PRESS_PUSH_MS: AtomicU64 = AtomicU64::new(0);
static RELEASE_PUSH_MS: AtomicU64 = AtomicU64::new(0);
static RELEASES_WAITED: AtomicU32 = AtomicU32::new(0);
static LANE_MAX_MS: AtomicU64 = AtomicU64::new(0);

/// A decoder pushed a primary button edge into the pal ring (EHCI trackpad arms: `note_buttons`' answer).
#[inline]
pub fn note_button_edge(press: bool, release: bool) {
    if press {
        PRESS_PUSH_MS.store(crate::arch::ms().max(1), Relaxed);
    } else if release {
        RELEASE_PUSH_MS.store(crate::arch::ms().max(1), Relaxed);
    }
}

/// How long the latest release has sat in the lane at `now` (0 = no release pushed since the latest press).
pub fn release_lane_ms(now: u64) -> u64 {
    let (p, r) = (PRESS_PUSH_MS.load(Relaxed), RELEASE_PUSH_MS.load(Relaxed));
    if r == 0 || r < p { 0 } else { now.saturating_sub(r) }
}

/// The Dock waited for a release that was in the lane (it launched on the release, not the timeout); `lane_ms` = how long
/// the release sat between its push and its route.
pub fn note_release_waited(lane_ms: u64) {
    RELEASES_WAITED.fetch_add(1, Relaxed);
    LANE_MAX_MS.fetch_max(lane_ms, Relaxed);
}

// ── HIDSTALL2 (B509): the trackpad on the wire ─────────────────────────────────────────────────────────────────────────

static TP_WIRE_LAST: AtomicU64 = AtomicU64::new(0);

/// Is a `[tp] mt` serial line due (one per [`TP_WIRE_MS`], the first at once)? Claims the slot when it says yes.
pub fn tp_wire_due(now: u64) -> bool {
    let last = TP_WIRE_LAST.load(Relaxed);
    (last == 0 || now.saturating_sub(last) >= TP_WIRE_MS) && TP_WIRE_LAST.compare_exchange(last, now.max(1), Relaxed, Relaxed).is_ok()
}
