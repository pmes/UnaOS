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
//!
//! SMALLFIX8 (rmbp-ledger B516) — flight 28's `HIDSTALL … hid_stall_s=1 dock_timeouts=1 … hold_worst_ms=483
//! hold_worst=usb-pump:flight-recorder -> FAIL`. A masked hold on the pump core starves the hid pump for its length: the
//! census names it, so it is not an unexplained stall and a dock press it overlapped is not a lost release. Every
//! holder over [`HOLD_OVER_MS`] also lands in a small ring of `(end_ms, hold_ms)` ([`held_between`]), and:
//! * a stall second whose gap a named hold in the last [`HELD_WINDOW_MS`] covers (`hold + HID_GAP_STALL_MS > gap`) is
//!   counted `held_stall_s=`, not `hid_stall_s=` — the verdict reads the unexplained seconds; the hold itself stays on
//!   the census (`holders=`, `hold_worst=`) and on the stall line;
//! * the Dock's release wait, from the press's delivery `t0`, is [`dock_release_wait_ms`]: wall time minus the pump's
//!   own holds inside it; a timeout whose last [`DOCK_RELEASE_TIMEOUT_MS`] held the pump core for a frame or more is
//!   `dock_held=`, never `dock_timeouts=` (a lost release), and says so: `[hid] dock timeout held_ms=<n> -> pump-held`;
//! * a timed-out press's release, when it does come, is timed press-to-release (`timeout_release_ms=`): a long click
//!   reads over 400, a lost release reads 0.

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
/// SMALLFIX8 (B516): stall seconds a named pump-core hold covered; dock timeouts that the pump's own hold explains.
static HID_STALL_HELD_S: AtomicU32 = AtomicU32::new(0);
static DOCK_HELD: AtomicU32 = AtomicU32::new(0);
/// SMALLFIX8: the worst UNEXPLAINED stall second's gap (ms) — what `hid_stall_s=` counts, by size.
static STALL_GAP_MAX_MS: AtomicU64 = AtomicU64::new(0);
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
        // SMALLFIX8 (B516): a gap a named pump-core hold covers is the holder's (the census names it), not a lost pass.
        let now = crate::arch::ms();
        if held_between(now.saturating_sub(HELD_WINDOW_MS), now) + HID_GAP_STALL_MS > hid_gap_ms {
            HID_STALL_HELD_S.fetch_add(1, Relaxed);
        } else {
            HID_STALL_S.fetch_add(1, Relaxed);
            STALL_GAP_MAX_MS.fetch_max(hid_gap_ms, Relaxed);
        }
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

/// One UnaFS transaction attempt began at `t0_us` ([`now_us`]) and its IRQ-masked span just ended; the site is `with_unafs`'s caller (HIDSTALLMASKSCOPE B1019: both `#[track_caller]`).
#[track_caller] pub fn note_masked_since(t0_us: u64) {
    note_masked(now_us().saturating_sub(t0_us) / 1000, core::panic::Location::caller());
}

/// One UnaFS transaction attempt's IRQ-masked span (ms), at `site`; a new max names its holder ([`masked_into`], tail).
pub fn note_masked(ms: u64, site: &'static core::panic::Location<'static>) {
    masked_into(&SEC_MASKED_MS, &SEC_MASKED_WHO, ms, site);
    masked_into(&MASKED_MAX_MS, &MASKED_MAX_WHO, ms, site);
    SCOPE_MASKED_MS.fetch_max(ms, Relaxed); masked_into(&SCOPE_BY_MS, &SCOPE_MASKED_WHO, ms, site); MASK_SITES.note(ms, site); // REGISTRYCHUNK (B508): the scoped max ([`scope_masked_take`]); MASKCENSUS (B1028): its holder on the printing line's cadence ([`scope_masked_by`]) and every span's site ([`MaskSites`]), tail
}

/// The second's longest masked UnaFS span and its holder, taken (reset) at `lag`'s roll (who first: a winner between the two is the next second's).
pub fn take_sec_masked() -> (u64, MaskedBy) {
    let by = SEC_MASKED_WHO.read(true); (SEC_MASKED_MS.swap(0, Relaxed), by)
}

/// A pin press timed out into its launch. SMALLFIX8 (B516): if the pump core was held for a frame or more inside the
/// wait (the timeout runs on the `usb-pump`, the release is decoded by the `hid-pump` on the same core), the timeout is
/// the holder's — `dock_held=` — never a lost release; either way the press's release, when it comes, is timed.
pub fn note_dock_timeout() {
    let now = crate::arch::ms();
    let held = held_between(now.saturating_sub(DOCK_RELEASE_TIMEOUT_MS), now);
    let pump_held = held >= HID_GAP_STALL_MS; let undecoded = !pump_held && release_undecoded(now); // SMALLFIX10 (B539 item 4): the decoder never pushed this press's release (file tail)
    if pump_held { DOCK_HELD.fetch_add(1, Relaxed); } else if undecoded { DOCK_UNDECODED.fetch_add(1, Relaxed); } else { DOCK_TIMEOUTS.fetch_add(1, Relaxed); }
    TIMEOUT_RELEASE_OWED.store(true, Relaxed);
    serial_println!(
        "[hid] dock timeout held_ms={} window_ms={} worst={} decoded={} -> {} (SMALLFIX8: a pump-core hold is the holder's, not a lost release; SMALLFIX10: undecoded = the pad sent no release report, the stream's loss, not the router's)",
        held, DOCK_RELEASE_TIMEOUT_MS, HolderName(session_worst().0), (!undecoded) as u8, if pump_held { "pump-held" } else if undecoded { "undecoded" } else { "unrouted" }
    );
}

/// The verdict (`hid_stall_s` = the UNEXPLAINED stall seconds; SMALLFIX8 counts held ones apart). Pure.
pub fn verdict(halted: u32, hid_stall_s: u32, keyq_ms: u64) -> bool {
    halted == 0 && hid_stall_s == 0 && keyq_ms <= KEY_QUEUE_BOUND_MS
}

/// Register `tests hidstall` (and HIDSTALL2's `tests inputstall`) once.
pub fn ensure() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::tests::register("hidstall", selftest); #[cfg(all(target_arch = "x86_64", feature = "wc"))] crate::video::renderwedge::ensure_tests(); // RENDERWEDGE (B520): `tests renderwedge` (same-line fold)
        crate::tests::register("inputstall", crate::video::lag::inputstall_selftest);
        crate::tests::register("queuesleep", crate::queuesleep::selftest); // QUEUESLEEP (B512, R103 §2): the idle window and one key, timed
    }
}

fn selftest() {
    let h = HALTED.load(Relaxed);
    let s = HID_STALL_S.load(Relaxed);
    let k = KEYQ_MAX_MS.load(Relaxed);
    serial_println!(
        ":: HIDSTALL: halted_at_login={} recovered={} proxy_retired={} hid_stall_s={} key_queue_max_ms={} dock_timeouts={} masked_max_ms={} masked_by={} -> {} :: hid_gap_bound_ms={} key_bound_ms={} release_timeout_ms={} holders={} hold_worst_ms={} hold_worst={} releases_waited={} lane_max_ms={} pump_cpu={} held_stall_s={} stall_gap_max_ms={} dock_held={} timeout_release_ms={} dock_undecoded={} fr={}",
        h, RECOVERED.load(Relaxed), PROXY_RETIRED.load(Relaxed), s, k, DOCK_TIMEOUTS.load(Relaxed), MASKED_MAX_MS.load(Relaxed), masked_by(),
        if verdict(h, s, k) { "PASS" } else { "FAIL" }, HID_GAP_STALL_MS, KEY_QUEUE_BOUND_MS, DOCK_RELEASE_TIMEOUT_MS,
        holders_seen(), session_worst().1, HolderName(session_worst().0), RELEASES_WAITED.load(Relaxed), LANE_MAX_MS.load(Relaxed), CpuWord(pump_cpu()),
        HID_STALL_HELD_S.load(Relaxed), STALL_GAP_MAX_MS.load(Relaxed), DOCK_HELD.load(Relaxed), TIMEOUT_RELEASE_MS.load(Relaxed), DOCK_UNDECODED.load(Relaxed), FrPlace
    ); mask_sites_say(); maskscope_kat(); // HIDSTALLMASKSCOPE (B1019): the masked-max holder KAT, `:: MASKSCOPE:` (LINE-NEUTRAL fold, code first; body at the tail); MASKCENSUS (B1028): the boot site table's ONE line first
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
pub fn note_hold(hold_ms: u64, name: Option<&'static str>, site: usize, step: u8) { let step = fr_step(step); // PUMPHOLE2 (B610): the `flight-recorder` step is charged to the recorder only while its work runs on this task (LINE-NEUTRAL fold, code first)
    if hold_ms <= HOLD_OVER_MS {
        return;
    }
    let at = crate::arch::ms(); // SMALLFIX8 (B516): the hold's end, into the ring `held_between` reads (overflow holds too)
    let hslot = HELD_IX.fetch_add(1, Relaxed) % HELD_RING_N; HELD_WHO[hslot].store(0, Relaxed); HELD_RING[hslot].store(at << 20 | hold_ms.min(HELD_MS_MASK), Relaxed); // POINTERFEEL (B535): the slot's holder is stamped below once known (0 = unnamed/overflow)
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
    h.n_min.fetch_add(1, Relaxed); HELD_WHO[hslot].store(ix as u8 + 1, Relaxed); // POINTERFEEL (B535): which holder this ringed hold was
    h.worst_min.fetch_max(hold_ms, Relaxed);
    h.n_all.fetch_add(1, Relaxed);
    h.worst_all.fetch_max(hold_ms, Relaxed);
    SEC_HOLD.fetch_max(hold_ms << 8 | (ix as u64 + 1), Relaxed);
}

// ── SMALLFIX8 (B516): when the pump core was held, for the stall second and the Dock's release wait ─────────────────

/// The last holds over [`HOLD_OVER_MS`] on the pump core, packed `end_ms << 20 | hold_ms` (hold capped at
/// [`HELD_MS_MASK`]); one writer (the pump core's tick ISR), readers sum overlaps. Eight is two seconds of the worst
/// cadence flight 28 read (12 holds a minute) many times over; an older hold has left every window that reads it.
const HELD_RING_N: usize = 8;
const HELD_MS_MASK: u64 = (1 << 20) - 1;
static HELD_RING: [AtomicU64; HELD_RING_N] = [const { AtomicU64::new(0) }; HELD_RING_N];
static HELD_IX: AtomicUsize = AtomicUsize::new(0);
/// A stall second is checked against the holds that ended in this window: the gap and the hold that caused it can be
/// rolled into adjacent seconds (the hold is charged on the first tick after it, the gap at the pass after it).
pub const HELD_WINDOW_MS: u64 = 2_000;

/// Milliseconds of `[from, to]` the pump core spent held (sum of each ringed hold's overlap with the interval).
pub fn held_between(from: u64, to: u64) -> u64 {
    let mut sum = 0u64;
    for e in HELD_RING.iter() {
        let w = e.load(Relaxed);
        if w == 0 {
            continue;
        }
        let (end, ms) = (w >> 20, w & HELD_MS_MASK);
        let (a, b) = (end.saturating_sub(ms).max(from), end.min(to));
        sum += b.saturating_sub(a);
    }
    sum
}

/// The Dock's release wait for a press DELIVERED to it at `t0` (its `PIN_T0`), at `now`: wall time minus the pump
/// core's own holds inside it — the release is decoded by the `hid-pump` on that core, so a held core is no time the
/// release could have arrived in. The timeout fires when THIS reaches [`DOCK_RELEASE_TIMEOUT_MS`].
pub fn dock_release_wait_ms(t0: u64, now: u64) -> u64 {
    now.saturating_sub(t0).saturating_sub(held_between(t0, now))
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
            write!(f, ":{}", crate::video::lag::PUMP_NAMES.get(step as usize).copied().unwrap_or(if step == P_FLIGHT_REST { "flight-step:bootwit" } else { "-" }))?; // PUMPHOLE2 (B610): the step's remainder named apart
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
/// SMALLFIX8 (B516): a press timed out and its release has not been seen yet; the latest such press-to-release (ms).
static TIMEOUT_RELEASE_OWED: AtomicBool = AtomicBool::new(false);
static TIMEOUT_RELEASE_MS: AtomicU64 = AtomicU64::new(0);
static LANE_MAX_MS: AtomicU64 = AtomicU64::new(0);

/// A decoder pushed a primary button edge into the pal ring (EHCI trackpad arms: `note_buttons`' answer).
#[inline]
pub fn note_button_edge(press: bool, release: bool) {
    if press {
        PRESS_PUSH_MS.store(crate::arch::ms().max(1), Relaxed);
    } else if release {
        let now = crate::arch::ms().max(1);
        RELEASE_PUSH_MS.store(now, Relaxed);
        if TIMEOUT_RELEASE_OWED.load(Relaxed) && TIMEOUT_RELEASE_OWED.swap(false, Relaxed) {
            let p = PRESS_PUSH_MS.load(Relaxed);
            if p != 0 { TIMEOUT_RELEASE_MS.store(now.saturating_sub(p), Relaxed); } // SMALLFIX8: the timed-out press's click length
        }
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

// ── POINTERFEEL (B535): which holder held the pump core over an interval ──────────────────────────────────────────────

/// The holder index (`+ 1`, 0 = unnamed) of each [`HELD_RING`] slot, written by [`note_hold`] beside the slot.
static HELD_WHO: [AtomicU8; HELD_RING_N] = [const { AtomicU8::new(0) }; HELD_RING_N];

/// Over `[from, to]`: the ringed hold with the largest overlap — `(overlap ms, holder index)`, `(0, usize::MAX)` when
/// the pump core was not held in the interval (or the hold was unnamed). `pointerfeel` names a report gap and the
/// login screen's longest HID gap with it.
pub fn holder_between(from: u64, to: u64) -> (u64, usize) {
    let mut best = (0u64, usize::MAX);
    for (i, e) in HELD_RING.iter().enumerate() {
        let w = e.load(Relaxed);
        if w == 0 {
            continue;
        }
        let (end, ms) = (w >> 20, w & HELD_MS_MASK);
        let ov = end.min(to).saturating_sub(end.saturating_sub(ms).max(from));
        let who = HELD_WHO[i].load(Relaxed);
        if ov > best.0 && who != 0 {
            best = (ov, who as usize - 1);
        }
    }
    best
}

// ── SMALLFIX10 (rmbp-ledger B539 item 4): a timeout whose release the decoder never pushed ─────────────────────────────
//
// TAIL-APPENDED (this file is in every image; nothing above it moves). Flight 31 boot 1: two pin presses timed out
// (`[hid] dock timeout held_ms=0 … -> no-release`, then `[dock] … release=timeout after_ms=400 pend=0 lane_ms=0`) and
// the verdict read `dock_timeouts=1 … dock_held=0 timeout_release_ms=0`. SMALLFIX8's rule did not hold because its
// premise did not: the wait held the pump core 0 ms (`held_ms=0`), nothing was queued (`pend=0`), and `lane_ms=0` is
// `release_lane_ms`'s `r < p` arm — NO release edge was pushed after the press. `timeout_release_ms=0` 127 s later says
// none came at all, and the wire agrees: no `[ptr] fine` and no `[tp] mt` line from 02:09:38 to 02:12:13 — the pad sent
// no report after the click's down report. The release was lost UPSTREAM of every router (the pad's lift report never
// reached the decoder: POINTERFEEL's report stream), and the Dock's timeout did its job (`-> launched`). Such a timeout
// is now counted `dock_undecoded=`, apart from `dock_timeouts=` (a release the decoder pushed and no router delivered
// in the window — the kernel's own loss, still the red one) and `dock_held=` (SMALLFIX8's pump hold).

/// SMALLFIX10: pin timeouts whose release the decoder never pushed (the input stream's loss).
static DOCK_UNDECODED: AtomicU32 = AtomicU32::new(0);

/// SMALLFIX10: at a timeout at `now`, did the decoder push a press recently and NO release since it? Only a decoder that
/// stamps its edges ([`note_button_edge`], the EHCI trackpad arms) can answer; a press it did not stamp (an xHCI or boot
/// mouse) leaves `p` stale or 0 and reads `false`, so the timeout stays `dock_timeouts=` — the conservative side.
fn release_undecoded(now: u64) -> bool {
    let (p, r) = (PRESS_PUSH_MS.load(Relaxed), RELEASE_PUSH_MS.load(Relaxed));
    p != 0 && r < p && now.saturating_sub(p) <= DOCK_RELEASE_TIMEOUT_MS + HELD_WINDOW_MS
}

// ── PUMPHOLE (rmbp-ledger B541) — the HIDSTALL line says where the flight recorder's body runs. FILE-TAIL append. ────────
// Flight 32 failed this verdict with `hold_worst=usb-pump:flight-recorder:209` while `tests pointerfeel` said `fr=inline`:
// the body's hand to its `flightrec` worker sat below the global-slot gate, which an internal-card-only boot never passes
// (flight_recorder.rs, the PUMPHOLE fold at the gate). ` fr=worker:c<n> fr_worst_ms=<n>` (or `fr=inline`, `-` off x86)
// now rides the end of the line, so a capture reads the recorder's placement beside the holder census it explains.
struct FrPlace;
impl core::fmt::Display for FrPlace {
    #[cfg(target_arch = "x86_64")]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (c, w) = crate::flight_recorder::worker_state();
        if c == usize::MAX { f.write_str("inline") } else { write!(f, "worker:c{} fr_worst_ms={}", c, w) }
    }
    #[cfg(not(target_arch = "x86_64"))]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("-")
    }
}

// ── PUMPHOLE2 (rmbp-ledger B610) — a hold in the pump's `flight-recorder` step is the recorder's only while the recorder's
// work runs on the pump. FILE-TAIL append. ────────
// Flight 33: `[hid] stall census holder=usb-pump:flight-recorder n=42 worst_ms=141` (all before login) beside
// `fr=worker:c2 fr_worst_ms=1`. The step id is "the step after the pump's last mark" (video/lag.rs `pump_next`), and the
// segment between `pump_seg(P_CONSOLE)` and `pump_seg(P_FLIGHT)` runs `flight_recorder::service()` AND
// `bootwit::service()` (main.rs usb_pump, SELFDIAG). Since PUMPHOLE2 the recorder's gate is one load on the pump once its
// worker is coming or running, so a masked hold in that segment is charged to `flight-recorder` only when
// `flight_recorder::on_caller_now()` says the recorder's work (an inline body, or the hand's spawn) is running on this
// very task; otherwise it is `usb-pump:flight-step:bootwit`. Off x86 there is no recorder: the step id is kept.

/// The step id a hold is charged under (`P_FLIGHT_REST` = the `flight-recorder` segment with the recorder not running).
const P_FLIGHT_REST: u8 = 0xFE;

/// Remap `step` for the attribution. ISR context: atomics only (`on_caller_now` is two relaxed loads).
#[inline]
fn fr_step(step: u8) -> u8 {
    #[cfg(target_arch = "x86_64")]
    if step == crate::video::lag::P_FLIGHT && !crate::flight_recorder::on_caller_now() {
        return P_FLIGHT_REST;
    }
    step
}

// ── HIDSTALLMASKSCOPE (rmbp-ledger B1019) — every masked maximum names its holder. FILE-TAIL append. ────────────────────
// Flight 38: `:: HIDSTALL: … masked_max_ms=2940` and CLOCKCORE's 2939 were boot-long maxima with no holder; LOGINSTALL
// (B1008) found task 67 by hand from the lock registry and the `[tests] watch` lines. The holder of a masked UnaFS span is
// the CURRENT task by construction (MOUNT is already released at the note), so a max records what LOCKREG's
// `pump_core_hold` records for a pump-core hold: `sched::current_name()` and a `&'static Location` — here `with_unafs`'s
// caller (`with_unafs` and `note_masked_since` are `#[track_caller]`). The record is written ONLY on the winning branch of
// the max's own `fetch_max`; two near-equal maxima racing may leave the earlier one's name (the try-lock below skips, it
// never spins): benign, said. `MaskedBy` prints `HolderName`'s shape (`<task>@<file>:<line>`, `?` no task, `-` none);
// it does not share `HolderName::fmt`'s body because that body is line-pinned above.

/// One masked maximum's holder: the task name (`&'static str` as pointer + length, 0 = none) and the site (a
/// `&'static Location`, 0 = none). `lock` is a TRY-lock over the three words: a writer or reader that finds it taken
/// skips (safe in a masked span and in an ISR — clockcore's `isr_tick` writes one).
pub struct MaskWho {
    lock: AtomicBool,
    name_p: AtomicUsize,
    name_l: AtomicUsize,
    site: AtomicUsize,
}

impl MaskWho {
    pub const fn new() -> Self {
        MaskWho { lock: AtomicBool::new(false), name_p: AtomicUsize::new(0), name_l: AtomicUsize::new(0), site: AtomicUsize::new(0) }
    }

    /// The max this record names was just won by `name` at `site` (call it only on the winning `fetch_max` branch).
    pub fn note(&self, name: Option<&'static str>, site: &'static core::panic::Location<'static>) {
        if self.lock.compare_exchange(false, true, Acquire, Relaxed).is_err() {
            return; // a near-equal max is being named this instant: its name stands (benign)
        }
        let (np, nl) = name.map(|n| (n.as_ptr() as usize, n.len())).unwrap_or((0, 0));
        self.name_p.store(np, Relaxed);
        self.name_l.store(nl, Relaxed);
        self.site.store(site as *const core::panic::Location<'static> as usize, Relaxed);
        self.lock.store(false, Release);
    }

    /// A consistent snapshot (`clear` = reset it too, for a max that is taken); a busy record reads as none.
    pub fn read(&self, clear: bool) -> MaskedBy {
        if self.lock.compare_exchange(false, true, Acquire, Relaxed).is_err() {
            return MaskedBy { name_p: 0, name_l: 0, site: 0 };
        }
        let by = MaskedBy { name_p: self.name_p.load(Relaxed), name_l: self.name_l.load(Relaxed), site: self.site.load(Relaxed) };
        if clear {
            self.name_p.store(0, Relaxed);
            self.name_l.store(0, Relaxed);
            self.site.store(0, Relaxed);
        }
        self.lock.store(false, Release);
        by
    }
}

/// A [`MaskWho`] snapshot: `masked_by=<task>@<file>:<line>` (`?` for no task, `-` for no record).
#[derive(Clone, Copy)] pub struct MaskedBy { // MASKCENSUS (B1028): Copy (three plain usizes) so login's `Done` (Copy) carries one; same-line token
    name_p: usize,
    name_l: usize,
    site: usize,
}

impl MaskedBy {
    /// The holder's task name, if one was recorded.
    pub fn name(&self) -> Option<&'static str> {
        if self.name_p == 0 {
            return None;
        }
        // SAFETY: `name_p`/`name_l` are a task's `&'static str` name, stored whole under the record's lock.
        Some(unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(self.name_p as *const u8, self.name_l)) })
    }

    /// The holder's site, if one was recorded.
    pub fn site(&self) -> Option<&'static core::panic::Location<'static>> {
        // SAFETY: `site` is a `&'static Location`, stored whole under the record's lock.
        if self.site == 0 { None } else { Some(unsafe { &*(self.site as *const core::panic::Location<'static>) }) }
    }
}

impl core::fmt::Display for MaskedBy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Some(l) = self.site() else { return f.write_str("-") };
        f.write_str(self.name().unwrap_or("?"))?;
        let file = l.file();
        write!(f, "@{}:{}", file.rsplit('/').next().unwrap_or(file), l.line())
    }
}

/// The second's and the boot's masked-max holders ([`note_masked`]).
static SEC_MASKED_WHO: MaskWho = MaskWho::new();
static MASKED_MAX_WHO: MaskWho = MaskWho::new();

/// Fold `ms` at `site` into `max`; when it is a new max, `who` names the current task and `site`. True on a new max.
fn masked_into(max: &AtomicU64, who: &MaskWho, ms: u64, site: &'static core::panic::Location<'static>) -> bool {
    let won = max.fetch_max(ms, Relaxed) < ms;
    if won {
        who.note(crate::arch::sched::current_name(), site);
    }
    won
}

/// The boot-long masked max's holder (the HIDSTALL verdict's `masked_by=`).
pub fn masked_by() -> MaskedBy {
    MASKED_MAX_WHO.read(false)
}

/// `tests hidstall`'s KAT: a 30 ms masked span held by the task running `tests`, recorded through the SAME
/// [`masked_into`] / [`MaskWho`] path on the KAT's OWN pair (a fixture's hold never feeds the boot maxima, B1008), must
/// name that task and the KAT's own call site through one `#[track_caller]` level (as `with_unafs` -> `note_masked_since`).
/// `:: MASKSCOPE: who=<task>@hidstall.rs:<n> ms=<n> -> PASS|FAIL :: task= site_ok= boot_by=`.
fn maskscope_kat() {
    static KAT_MAX: AtomicU64 = AtomicU64::new(0);
    static KAT_WHO: MaskWho = MaskWho::new();
    #[cfg(target_arch = "x86_64")]
    {
        if crate::arch::x86_64::apic::tsc_hz() < 1_000_000 {
            serial_println!(":: MASKSCOPE: who=- ms=0 -> SKIP reason=no-tsc-us ::"); // `now_us` would be the tick clock, frozen under the mask
            return;
        }
    }
    KAT_MAX.store(0, Relaxed);
    let _ = KAT_WHO.read(true);
    let me = crate::arch::sched::current_name();
    let (line, ms) = (line!(), kat_hold(&KAT_MAX, &KAT_WHO, 30));
    let by = KAT_WHO.read(false);
    let site_ok = by.site().is_some_and(|l| l.file() == file!() && l.line() == line);
    let (sites, top_ok, sums_ok, roll_ok) = masksites_kat(); let wrap = wrapsite_kat(); // MASKCENSUS (B1028): the site table's leg, on its own instance (tail); MASKWRITESITE (B1037): the third site, THROUGH the write wrapper (tail)
    let pass = me.is_some() && by.name() == me && site_ok && ms >= 29 && KAT_MAX.load(Relaxed) == ms && sites == 2 && top_ok && sums_ok && roll_ok && wrap != Some(false);
    serial_println!(
        ":: MASKSCOPE: who={} ms={} -> {} :: task={} site_ok={} sites={} top_ok={} sums_ok={} roll_ok={} wrapsite_ok={} boot_by={}",
        by, ms, if pass { "PASS" } else { "FAIL" }, me.unwrap_or("?"), site_ok as u8, sites, top_ok as u8, sums_ok as u8, roll_ok as u8, match wrap { Some(true) => "1", Some(false) => "0", None => "-" }, masked_by()
    );
    crate::tests::tally(pass);
}

/// Hold interrupts masked for `ms` on this task, then record the span at the CALLER's site into `max`/`who`.
#[track_caller]
fn kat_hold(max: &AtomicU64, who: &MaskWho, ms: u64) -> u64 {
    let site = core::panic::Location::caller();
    let t0 = now_us();
    crate::arch::without_interrupts(|| {
        while now_us().saturating_sub(t0) < ms * 1000 {
            core::hint::spin_loop();
        }
    });
    let span = now_us().saturating_sub(t0) / 1000;
    masked_into(max, who, span, site);
    span
}

// ── DESKTOPHIDGAP (rmbp-ledger B1020) — the census's core FOLLOWS the hid pump. FILE-TAIL append. ─────────────────────────
// Flight 38: every `[lag] stall … stage=hid` second after 20:45:00 read `hid_gap ≈ masked_ms + 10..15` and `holder=-`, and
// HIDSTALL said `pump_cpu=7 … fr=worker:c2 fr_worst_ms=1331`. `tests gatesteal2` declared c7 dead and
// `sched::rehome_dead_core` moved its sleepers — the `hid-pump` among them — to c2, the worker core the `flightrec` worker had
// been placed on (once, "apart from the hid pump" with pump_cpu=7). [`PUMP_CPU`] was written once at the spawn, so the tick
// census ([`note_hold`] via `sync::pump_core_hold`), the tickless exclusion and every worker's `!= pump_cpu()` placement kept
// reading the corpse's number while the flight recorder's masked UnaFS spans starved the pass on c2, charged to no holder.
// The hid pump now publishes its own core every pass (SESSIONWEDGE's B657 shape for the usb-pump: "the re-home MOVES the
// pump"), and says a move once.

/// DESKTOPHIDGAP (B1020): the `hid-pump` task's pass, on its own core — re-stamp [`PUMP_CPU`] (one writer: this task) and
/// print one line when it changed from a set value. Task context only (never the ISR).
#[cfg(target_arch = "x86_64")]
pub fn pump_here() {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    let was = PUMP_CPU.swap(cpu, Relaxed);
    if was != cpu && was != usize::MAX {
        serial_println!(
            "[hid] pump core moved from=c{} to=c{} (DESKTOPHIDGAP B1020: the census, the tickless exclusion and every worker's \"apart from the hid pump\" rule follow the hid pump; GATESTEAL's re-home moves a dead core's sleepers)",
            was, cpu
        );
    }
}

// ── MASKCENSUS (rmbp-ledger B1028) — every masked span's SITE, counted; the scoped max's holder. FILE-TAIL append. ──────────
// HIDSTALLMASKSCOPE (B1019) named only the MAX; flight 38's `hid_stall_s=5` came from many 65-129 ms spans, not one.
// [`MaskSites`] counts every span [`note_masked`] sees by its site (`with_unafs`'s `#[track_caller]` caller): per second for
// the `[lag] stall` line's `masked_sites=` (top-3 by the second's sum) and boot-long for `[hidstall] masked sites` after the
// HIDSTALL verdict (top-5 by sum; the verdict itself is at its 25-field cap). A key is claimed once by CAS (0 -> the
// `&'static Location`) and never freed; the counters are independent atomics, so there is no lock and nothing is skipped — a
// reader may see a span's `n` before its `sum` (one span skew, benign). A full table counts `other`. Cost per span: <= 16
// relaxed loads, one CAS on a site's first span, five RMW atomics; per second, one 16-slot scan in `lag`'s roll.
// [`SiteTop`] is the ONE top-k formatter (clockcore's `[clockcore] masked sites` line offers its SEEN rows to it).
// The SCOPE holder: `scope_masked_take`'s max is reset per transaction (assoc's group sizing needs that), so the holder rides
// a second max on the PRINTING line's cadence, [`scope_masked_by`] — the third [`MaskWho`]. The scope is global (any core):
// an overlapping login submit and registry build share it, as `SCOPE_MASKED_MS` already does.

const MASK_SITES_N: usize = 16;
const SITE_TOP_N: usize = 5;

/// A fixed 16-site table of masked spans: key = a `&'static Location` (0 = free), boot-long (n, sum, max) and the
/// second's (n, sum), plus the overflow row `other`.
pub(crate) struct MaskSites {
    site: [AtomicUsize; MASK_SITES_N],
    n: [AtomicU32; MASK_SITES_N],
    sum: [AtomicU64; MASK_SITES_N],
    max: [AtomicU64; MASK_SITES_N],
    sec_n: [AtomicU32; MASK_SITES_N],
    sec_sum: [AtomicU64; MASK_SITES_N],
    other_n: AtomicU32,
    other_sum: AtomicU64,
    sec_other_n: AtomicU32,
    sec_other_sum: AtomicU64,
}

impl MaskSites {
    pub(crate) const fn new() -> Self {
        MaskSites {
            site: [const { AtomicUsize::new(0) }; MASK_SITES_N],
            n: [const { AtomicU32::new(0) }; MASK_SITES_N],
            sum: [const { AtomicU64::new(0) }; MASK_SITES_N],
            max: [const { AtomicU64::new(0) }; MASK_SITES_N],
            sec_n: [const { AtomicU32::new(0) }; MASK_SITES_N],
            sec_sum: [const { AtomicU64::new(0) }; MASK_SITES_N],
            other_n: AtomicU32::new(0),
            other_sum: AtomicU64::new(0),
            sec_other_n: AtomicU32::new(0),
            sec_other_sum: AtomicU64::new(0),
        }
    }

    /// The slot keyed `p`, claimed by CAS when first seen (`None` = the table is full).
    fn slot(&self, p: usize) -> Option<usize> {
        for (i, k) in self.site.iter().enumerate() {
            let cur = k.load(Acquire);
            if cur == p {
                return Some(i);
            }
            if cur == 0 {
                match k.compare_exchange(0, p, core::sync::atomic::Ordering::AcqRel, Acquire) {
                    Ok(_) => return Some(i),
                    Err(now) if now == p => return Some(i),
                    Err(_) => {}
                }
            }
        }
        None
    }

    /// One masked span of `ms` at `site`. Atomics only (any core, right after the span).
    pub(crate) fn note(&self, ms: u64, site: &'static core::panic::Location<'static>) {
        match self.slot(site as *const core::panic::Location<'static> as usize) {
            Some(i) => {
                self.n[i].fetch_add(1, Relaxed);
                self.sum[i].fetch_add(ms, Relaxed);
                self.max[i].fetch_max(ms, Relaxed);
                self.sec_n[i].fetch_add(1, Relaxed);
                self.sec_sum[i].fetch_add(ms, Relaxed);
            }
            None => {
                self.other_n.fetch_add(1, Relaxed);
                self.other_sum.fetch_add(ms, Relaxed);
                self.sec_other_n.fetch_add(1, Relaxed);
                self.sec_other_sum.fetch_add(ms, Relaxed);
            }
        }
    }

    /// The second's top-3 by sum, its counters taken (reset) — `lag`'s roll, every second.
    pub(crate) fn take_sec(&self) -> SiteTop {
        let mut t = SiteTop::new(3, false);
        for (i, k) in self.site.iter().enumerate() {
            let p = k.load(Acquire);
            if p == 0 {
                continue;
            }
            let n = self.sec_n[i].swap(0, Relaxed);
            let sum = self.sec_sum[i].swap(0, Relaxed);
            t.offer(p, n, sum, 0);
        }
        t.set_other(self.sec_other_n.swap(0, Relaxed), self.sec_other_sum.swap(0, Relaxed));
        t
    }

    /// The boot-long top-5 by sum, with each site's max.
    pub(crate) fn boot(&self) -> SiteTop {
        let mut t = SiteTop::new(SITE_TOP_N, true);
        for (i, k) in self.site.iter().enumerate() {
            let p = k.load(Acquire);
            if p != 0 {
                t.offer(p, self.n[i].load(Relaxed), self.sum[i].load(Relaxed), self.max[i].load(Relaxed));
            }
        }
        t.set_other(self.other_n.load(Relaxed), self.other_sum.load(Relaxed));
        t
    }

    /// Empty the table (the KAT's own instance only: no writer runs beside it).
    fn clear(&self) {
        for i in 0..MASK_SITES_N {
            self.site[i].store(0, Relaxed);
            self.n[i].store(0, Relaxed);
            self.sum[i].store(0, Relaxed);
            self.max[i].store(0, Relaxed);
            self.sec_n[i].store(0, Relaxed);
            self.sec_sum[i].store(0, Relaxed);
        }
        self.other_n.store(0, Relaxed);
        self.other_sum.store(0, Relaxed);
        self.sec_other_n.store(0, Relaxed);
        self.sec_other_sum.store(0, Relaxed);
    }
}

/// The top sites by sum: `<file>:<line>:<n>x<sum_ms>[/<max_ms>],…` (`-` for none). Entries are
/// `(&'static Location as usize, n, sum_ms, max_ms)`, sorted by sum (the earlier offer first on a tie).
#[derive(Clone, Copy)]
pub struct SiteTop {
    e: [(usize, u32, u64, u64); SITE_TOP_N],
    cap: usize,
    with_max: bool,
    k: u32,
    other: (u32, u64),
}

impl SiteTop {
    /// Keep the top `cap` (at most 5); `with_max` prints `/<max_ms>` per site.
    pub const fn new(cap: usize, with_max: bool) -> Self {
        SiteTop { e: [(0, 0, 0, 0); SITE_TOP_N], cap: if cap < SITE_TOP_N { cap } else { SITE_TOP_N }, with_max, k: 0, other: (0, 0) }
    }

    /// Offer one site (`site` = a `&'static Location` as usize); a site with no span is not counted.
    pub fn offer(&mut self, site: usize, n: u32, sum_ms: u64, max_ms: u64) {
        if site == 0 || n == 0 {
            return;
        }
        self.k += 1;
        let mut i = self.cap;
        while i > 0 && (self.e[i - 1].0 == 0 || self.e[i - 1].2 < sum_ms) {
            i -= 1;
        }
        if i >= self.cap {
            return;
        }
        let mut j = self.cap - 1;
        while j > i {
            self.e[j] = self.e[j - 1];
            j -= 1;
        }
        self.e[i] = (site, n, sum_ms, max_ms);
    }

    /// The overflow row (spans whose site found the table full).
    pub fn set_other(&mut self, n: u32, sum_ms: u64) {
        self.other = (n, sum_ms);
    }

    /// `(n, sum_ms)` of the overflow row.
    pub fn other(&self) -> (u32, u64) {
        self.other
    }

    /// Sites with a span (all offered, not only the kept top).
    pub fn sites(&self) -> u32 {
        self.k
    }

    /// The kept entry `i` (`(0, 0, 0, 0)` past the end).
    pub fn entry(&self, i: usize) -> (usize, u32, u64, u64) {
        if i < self.cap { self.e[i] } else { (0, 0, 0, 0) }
    }

    /// The kept entry for `site`: `(n, sum_ms, max_ms)`.
    pub fn find(&self, site: usize) -> Option<(u32, u64, u64)> {
        self.e[..self.cap].iter().find(|e| e.0 == site && site != 0).map(|e| (e.1, e.2, e.3))
    }

    /// Printed only when `on` (else `-`): the stall line's `masked_sites=` below `STALL_MS` of masked time.
    pub fn when(mut self, on: bool) -> Self {
        if !on {
            self.e = [(0, 0, 0, 0); SITE_TOP_N];
            self.k = 0;
        }
        self
    }
}

impl core::fmt::Display for SiteTop {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut said = false;
        for &(p, n, sum, max) in self.e[..self.cap].iter() {
            if p == 0 {
                continue;
            }
            // SAFETY: every key is a `&'static Location` (from `#[track_caller]` / `Location::caller`), stored whole.
            let l: &'static core::panic::Location<'static> = unsafe { &*(p as *const core::panic::Location<'static>) };
            let file = l.file();
            if said {
                f.write_str(",")?;
            }
            write!(f, "{}:{}:{}x{}", file.rsplit('/').next().unwrap_or(file), l.line(), n, sum)?;
            if self.with_max {
                write!(f, "/{}", max)?;
            }
            said = true;
        }
        if !said {
            f.write_str("-")?;
        }
        Ok(())
    }
}

/// The boot's masked-span site table ([`note_masked`]).
static MASK_SITES: MaskSites = MaskSites::new();

/// The second's top-3 masked sites, taken at `lag`'s roll (every second, printed or not).
pub fn take_sec_sites() -> SiteTop {
    MASK_SITES.take_sec()
}

/// The SCOPE holder: the largest scoped span since the last [`scope_masked_by`], and who held it.
static SCOPE_BY_MS: AtomicU64 = AtomicU64::new(0);
static SCOPE_MASKED_WHO: MaskWho = MaskWho::new();

/// The holder of the largest masked UnaFS span (any core) since the previous call, reset (who first: a winner between
/// the two is the next window's). A reader calls it once where its printed max's window opens and once where it closes.
pub fn scope_masked_by() -> MaskedBy {
    let by = SCOPE_MASKED_WHO.read(true);
    SCOPE_BY_MS.swap(0, Relaxed);
    by
}

/// `tests hidstall`: the boot site table, ONE line after the HIDSTALL verdict.
fn mask_sites_say() {
    let t = MASK_SITES.boot();
    let (on, os) = t.other();
    serial_println!(
        "[hidstall] masked sites n={} other={}x{} top={} (MASKCENSUS B1028: every masked UnaFS span by its with_unafs caller, boot-long, <n>x<sum_ms>/<max_ms>, top-5 by sum)",
        t.sites(), on, os, t
    );
}

/// The MASKSCOPE leg's site check, on the KAT's OWN table (a fixture never feeds the boot table, B1008): two SYNTHETIC
/// spans' worth of sites (A: one 20 ms span; B: 4 + 5 ms) — no extra IRQ mask, `kat_hold` already drives the masked path.
/// `(sites, top_ok, sums_ok, roll_ok)`: top = A then B by sum; sums = each site's (n, sum, max); roll = the per-second
/// take returns both and a second take is empty while the boot counts stay.
fn masksites_kat() -> (u32, bool, bool, bool) {
    static KAT_SITES: MaskSites = MaskSites::new();
    KAT_SITES.clear();
    let a: &'static core::panic::Location<'static> = core::panic::Location::caller();
    let b: &'static core::panic::Location<'static> = core::panic::Location::caller();
    KAT_SITES.note(20, a);
    KAT_SITES.note(4, b);
    KAT_SITES.note(5, b);
    let (pa, pb) = (a as *const core::panic::Location<'static> as usize, b as *const core::panic::Location<'static> as usize);
    let boot = KAT_SITES.boot();
    let top_ok = pa != pb && boot.entry(0).0 == pa && boot.entry(1).0 == pb;
    let sums_ok = boot.find(pa) == Some((1, 20, 20)) && boot.find(pb) == Some((2, 9, 5));
    let (s1, s2) = (KAT_SITES.take_sec(), KAT_SITES.take_sec());
    let roll_ok = s1.sites() == 2 && s1.entry(0).0 == pa && s1.find(pb).map(|e| (e.0, e.1)) == Some((2, 9)) && s2.sites() == 0 && KAT_SITES.boot().sites() == 2;
    (boot.sites(), top_ok, sums_ok, roll_ok)
}

// ── MASKWRITESITE (rmbp-ledger B1037) — a masked span's SITE is the op's caller, not a wrapper's line. FILE-TAIL append. ─────
// MASKCENSUS (B1028) keys every span on `with_unafs`'s `#[track_caller]` caller, but splitwrite's `with_unafs_write` and
// `run` (`fs/unafs.rs`) carried no token, so every split write op keyed on `run`'s two `with_unafs` lines (HOLD 1, HOLD 2),
// and the in-file op wrappers (`native_acl_write/read/clear/list`, `snapshot_read`, `versions`, `read_version`,
// `restore_version`, `bind_root_on`) on their own call lines. Each now carries `#[track_caller]` (a same-line token): a VFS
// write op names its own `fs/vfs.rs` line. The split read's chain is NOT tokened: its lines name the hold phase (owed).

/// `tests hidstall`'s MASKSCOPE leg, third site: ONE no-op write op through `with_unafs_write` (x86 `wc`: splitwrite's
/// wrapper -> `run` -> `with_unafs`; elsewhere the alias, `with_unafs` itself) must key the BOOT table on the call's line.
/// A REAL ~0 ms masked span (it wins no max: `masked_into` takes only a strictly greater span); it claims one boot slot,
/// keyed here. `Some(true)` = the key at the call's line rose; `None` = no span was taken (an `Err`: no volume, a Busy
/// ticket) or the table is full (`other` rose); `Some(false)` = a span ran and keyed elsewhere (e.g. `run`'s line).
fn wrapsite_kat() -> Option<bool> {
    #[cfg(not(any(target_arch = "aarch64", feature = "unafs")))] { None } // MASKWRITESITE (B1037) seat fix: no UnaFS in this image (fs/mod.rs:43 gate), no wrapper to walk — the KAT reads `-`
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))] {
        let other0 = MASK_SITES.other_n.load(Relaxed);
        let n0 = site_n(line!() + 1); // the call's line: the NEXT line
        let (line, r) = (line!(), crate::fs::unafs::with_unafs_write(|_| ()));
        let rose = site_n(line) > n0;
        let full = MASK_SITES.other_n.load(Relaxed) != other0;
        if rose { Some(true) } else if r.is_err() || full { None } else { Some(false) }
    }
}

/// The boot table's `n` for the site keyed at `hidstall.rs:<line>` (0 = no such key).
fn site_n(line: u32) -> u32 {
    for (i, k) in MASK_SITES.site.iter().enumerate() {
        let p = k.load(Acquire);
        if p == 0 {
            continue;
        }
        // SAFETY: every key is a `&'static Location` (from `#[track_caller]` / `Location::caller`), stored whole by `MaskSites::note`.
        let l: &'static core::panic::Location<'static> = unsafe { &*(p as *const core::panic::Location<'static>) };
        if l.file() == file!() && l.line() == line {
            return MASK_SITES.n[i].load(Relaxed);
        }
    }
    0
}
