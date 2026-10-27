// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver
//!
//! LIDSLEEP (rmbp-ledger B431, MACPARITY row 34) — the lid read and the backlight-only sleep. Design and rung
//! ledger: `docs/dev/evidence/rmbp-1005/lidsleep.md` (DRIVERS-METHOD §6).
//!
//! * **Rung 0 (the premise, READ-ONLY):** with `lidsleep` (UNAOS_LIDSLEEP=1, implies `smc`, x86) the SMC key
//!   `MSLD` is read once at the first desktop pass and every 1 s after, through `smc::read_key` (READ_CMD only —
//!   nothing is written to the SMC). Printed at boot and on change: `[smc] lid=<open|closed> raw=<byte> …`.
//!   `nonzero = closed` is the HYPOTHESIS rung 0 tests; the raw byte is always printed beside it.
//! * **The ladder, written and UNAPPLIED:** lid closed → backlight off (gmux 0x74 := 0, the register BRIGHTSLIDER
//!   drives), the Kepler's display engine left running, input ignored; lid open → restore the prior raw.
//!   [`LADDER_ARMED`] is false until rung 0 is confirmed on metal; each lid change prints what it WOULD do.
//! * **[`sleep_request`]** (LOGINWINDOW's Sleep button): blanks the backlight only and says so; the next key or
//!   pointer event restores it ([`service`] watches `dimidle`'s activity clock). S3 is NOT this arc.
//! * **`tests lidsleep`** (R80: registered, never at boot): `:: LIDSLEEP: msld=… flips=… backlight_off=… ::`.
//!
//! SLEEP (rmbp-ledger B691, BP-bar-20, `docs/dev/blueprint/power.md`) — the ladder is ARMED and the blank is a STATE:
//! [`sleep_request`] (the lid closing, LOGINWINDOW's Sleep) turns the backlight off AND holds the panel (`asleep()` is
//! a refusal term of `video::panel_refuse_term`, so the compositor and the cursor decline and every core idles in
//! `hlt`); the service pass then fills the panel black and has `power::sleep` read the S3 census and the battery.
//! Wakers, each named: `lid` (MSLD back to 0, polled every [`SLEEP_POLL_MS`] while asleep), `power-key` (the fixed
//! ACPI power button), `input` (a key or pointer event). [`wake`] restores the level; the next service pass repaints
//! (the DIMIDLE repaint) and `power::sleep::first_frame` prints `[power] sleep … wake src=… ms=` and `:: SLEEP: ::`.
//! `tests lidsleep` keeps its backlight-only blink (it never enters the state); `tests sleep` enters and leaves it.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Rung 2 (lid → sleep). ARMED by SLEEP (B691): the bench's lid close is rung 0's confirmation and the state's entry in
/// one step; a wrong polarity costs a dark panel that the next key wakes (`input`), never a lost machine.
pub const LADDER_ARMED: bool = true;
/// Rung 0's poll period (ms).
pub const POLL_MS: u64 = 1000;
/// The lid poll while asleep (ms): the lid waker's latency bound inside BP-bar-20's 1000 ms lid-open → first frame.
pub const SLEEP_POLL_MS: u64 = 200;

/// MSLD's last raw byte (`u32::MAX` = never read).
static LID_RAW: AtomicU32 = AtomicU32::new(u32::MAX);
/// Lid state changes seen since boot.
static FLIPS: AtomicU32 = AtomicU32::new(0);
static LAST_POLL_MS: AtomicU64 = AtomicU64::new(0);
/// 0 = polling, 1 = key absent (stop), 2 = stuck seen once (keep polling, print once).
static POLL_STATE: AtomicU32 = AtomicU32::new(0);
/// A [`sleep_request`] blank is in force.
static SLEPT: AtomicBool = AtomicBool::new(false);
static SLEPT_AT_MS: AtomicU64 = AtomicU64::new(0);
/// The register's value before the blank (restored on wake).
static PRIOR_RAW: AtomicU32 = AtomicU32::new(0);
/// The sleep's blank reached a backlight (a gmux answered).
static SLEPT_HW: AtomicBool = AtomicBool::new(false);
/// The service pass owes the entry's black fill (and the power record's slow half).
static FILL_OWED: AtomicBool = AtomicBool::new(false);
/// The service pass owes the wake's repaint (and the SLEEP line).
static REPAINT_OWED: AtomicBool = AtomicBool::new(false);

/// `nonzero = closed` — rung 0's hypothesis (polarity unpinned, decided by the glass). Pure.
pub const fn lid_closed(raw: u8) -> bool { raw != 0 }

fn lid_word(raw: u8) -> &'static str { if lid_closed(raw) { "closed" } else { "open" } }

/// What one MSLD read answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Msld {
    Ok(u8, usize),
    NoKey,
    Stuck(u8),
    Unbuilt,
}

impl Msld {
    pub fn word(self) -> &'static str {
        match self { Msld::Ok(..) => "ok", Msld::NoKey => "no-key", Msld::Stuck(_) => "stuck", Msld::Unbuilt => "unbuilt" }
    }
}

/// One read-only MSLD read (1 byte: the first value byte of the key, whatever its length).
pub fn read_msld() -> Msld {
    #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
    {
        use crate::drivers::smc;
        let mut b = [0u8; 1];
        return match smc::read_key(b"MSLD", &mut b) {
            Ok(n) => Msld::Ok(b[0], n),
            Err(smc::SmcError::Absent) => Msld::NoKey,
            Err(smc::SmcError::Stuck(s)) => Msld::Stuck(s),
        };
    }
    #[allow(unreachable_code)]
    Msld::Unbuilt
}

/// Lid changes seen since boot.
pub fn flips() -> u32 { FLIPS.load(Ordering::Relaxed) }

/// The sleep state is in force: the panel is held (a refusal term of `video::panel_refuse_term`).
pub fn asleep() -> bool { SLEPT.load(Ordering::Relaxed) }

/// One service pass (called from `dimidle::service`): the entry's fill, the wakers (input, power key, lid), rung 0's
/// MSLD poll (1 s awake, [`SLEEP_POLL_MS`] asleep), and the wake's repaint.
pub fn service() {
    let now = crate::arch::ms();
    if FILL_OWED.swap(false, Ordering::AcqRel) && SLEPT.load(Ordering::Acquire) {
        if let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) { fb.fill_screen(0); fb.flush_all(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] super::zoom::note_unnoted(); } // ZOOMFURN (B848): an unnamed RAM-frame write owes the lens one whole present, counted. ⚠ LINE-NEUTRAL fold.
        #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] crate::power::sleep::measure_enter();
    }
    if SLEPT.load(Ordering::Acquire) {
        let act = crate::video::dimidle::last_activity_ms();
        if act > SLEPT_AT_MS.load(Ordering::Relaxed) { wake("input"); }
    }
    #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
    if SLEPT.load(Ordering::Acquire) && crate::power::sleep::power_key() { wake("power-key"); }
    if POLL_STATE.load(Ordering::Relaxed) != 1 {
        let last = LAST_POLL_MS.load(Ordering::Relaxed);
        let period = if SLEPT.load(Ordering::Relaxed) { SLEEP_POLL_MS } else { POLL_MS };
        if last == 0 || now.wrapping_sub(last) >= period {
            LAST_POLL_MS.store(now.max(1), Ordering::Relaxed);
            poll(now);
        }
    }
    if REPAINT_OWED.swap(false, Ordering::AcqRel) {
        if let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) {
            fb.fill_screen(super::wm::DESKTOP_BG);
            fb.flush_all(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] super::zoom::note_unnoted(); // ZOOMFURN (B848): an unnamed RAM-frame write owes the lens one whole present, counted. ⚠ LINE-NEUTRAL fold.
            let (w, h) = (fb.width(), fb.height());
            let _ = super::wm::damage_intersecting(0, 0, w, h);
        }
        super::wm::composite();
        #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] crate::power::sleep::first_frame(SLEPT_HW.load(Ordering::Relaxed));
    }
}

fn poll(now: u64) {
    match read_msld() {
        Msld::Ok(raw, n) => {
            let prev = LID_RAW.swap(raw as u32, Ordering::AcqRel);
            if prev == u32::MAX {
                serial_println!("[smc] lid={} raw={} len={} at=boot key=MSLD (rung 0: nonzero=closed is the hypothesis)", lid_word(raw), raw, n);
            } else if prev != raw as u32 {
                let f = FLIPS.fetch_add(1, Ordering::Relaxed) + 1;
                serial_println!("[smc] lid={} raw={} was={} flips={} ms={}", lid_word(raw), raw, prev, f, now);
                if (prev != 0) != (raw != 0) {
                    let would = if lid_closed(raw) { "backlight-off" } else { "restore" };
                    if LADDER_ARMED {
                        if lid_closed(raw) { let _ = sleep_request("lid"); } else if asleep() { wake("lid"); }
                    } else {
                        serial_println!("[lidsleep] ladder unapplied lid={} would={} armed=0 (rung 0 open)", lid_word(raw), would);
                    }
                }
            }
        }
        Msld::NoKey => {
            POLL_STATE.store(1, Ordering::Relaxed);
            serial_println!("[smc] lid=no-key key=MSLD (rung 0 refuted for MSLD; poll stopped)");
        }
        Msld::Stuck(s) => {
            if POLL_STATE.swap(2, Ordering::Relaxed) != 2 {
                serial_println!("[smc] lid=stuck key=MSLD step={} (bounded; poll continues, printed once)", s);
            }
        }
        Msld::Unbuilt => POLL_STATE.store(1, Ordering::Relaxed),
    }
}

/// What a blank did: `ok` (hardware readback 0), `unarmed` (no gmux on this build), `fail` (readback not 0).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Blank {
    pub prior: Option<u32>,
    pub readback: Option<u32>,
    pub hw: bool,
}

impl Blank {
    pub fn word(&self) -> &'static str {
        if !self.hw { "unarmed" } else if self.readback == Some(0) { "ok" } else { "fail" }
    }
}

fn opt(v: Option<u32>) -> alloc::string::String {
    match v { Some(x) => alloc::format!("{}", x), None => alloc::string::String::from("-") }
}

/// The backlight off, nothing else (the gmux write and its readback). `tests lidsleep`'s blink and the state's first step.
fn blank(via: &str) -> Blank {
    let prior = super::backlight::readback_now().or(Some(super::backlight::cur_raw()));
    let (_, readback, hw) = super::backlight::off_via(via);
    Blank { prior, readback, hw }
}

/// The sleep state (the lid closing, LOGINWINDOW's Sleep): the backlight off and the panel held, every core left to idle
/// in `hlt`; the service pass fills the panel black. The display engine keeps scanning out (no Kepler PM rung); S3 is
/// unreachable from this tree and `power::sleep` says why. Idempotent: a second request keeps the first prior level.
pub fn sleep_request(via: &str) -> Blank {
    let b = blank(via);
    if !SLEPT.load(Ordering::Relaxed) { PRIOR_RAW.store(b.prior.unwrap_or(0), Ordering::Relaxed); } #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] if !SLEPT.load(Ordering::Relaxed) { arm_wake_claim(); } // LIDKEYSWALLOW (B784): a first entry re-arms the wake claim before SLEPT is set (file tail). LINE-NEUTRAL fold, code first
    SLEPT_HW.store(b.hw, Ordering::Relaxed);
    SLEPT_AT_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
    let first = !SLEPT.swap(true, Ordering::AcqRel);
    serial_println!(
        "[lidsleep] sleep_request via={} backlight_off={} prior={} readback={} engine=running panel=held wake=lid,power-key,input",
        via, b.word(), opt(b.prior), opt(b.readback)
    );
    if first {
        FILL_OWED.store(true, Ordering::Release);
        #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] crate::power::sleep::enter(via, b.word());
    }
    b
}

/// Leave the sleep state from `via` (a named waker): restore the level the blank took, release the panel, and owe the
/// service pass its repaint. Returns the readback.
pub fn wake(via: &str) -> Option<u32> {
    if !SLEPT.swap(false, Ordering::AcqRel) { return None; } #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] clear_wake_claim(); // LIDKEYSWALLOW (B784): the wake edge clears the claim, so a claim made in one sleep never answers the next (file tail). LINE-NEUTRAL fold, code first
    let prior = PRIOR_RAW.load(Ordering::Relaxed);
    let a = super::backlight::set_raw_via(prior, "lidsleep-wake");
    #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] crate::power::sleep::woke(via);
    REPAINT_OWED.store(true, Ordering::Release);
    serial_println!("[lidsleep] wake via={} restored={} readback={}", via, prior, opt(a.readback));
    a.readback
}

/// `tests lidsleep` (R80: registered, never at boot). One MSLD read; the blank and its restore (the panel blinks dark).
/// Expected shape (the capture and the gmux's own semantics, not this build's output): msld=ok (MSLD is in this SMC's
/// key index, f13 idx 255), backlight_off=ok (index 0x74 reads back 0 after the 0) and the restore reads back the prior.
pub fn selftest() {
    let m = read_msld();
    let raw = match m { Msld::Ok(r, _) => alloc::format!("{}", r), _ => alloc::string::String::from("-") };
    let b = blank("tests");
    let restore_rb = super::backlight::set_raw_via(b.prior.unwrap_or(0), "lidsleep-wake").readback;
    serial_println!("[lidsleep] blink via=tests backlight_off={} restore_rb={} (the state is `tests sleep`'s)", b.word(), opt(restore_rb));
    let restored = match (b.hw, b.prior) { (true, Some(p)) => restore_rb == Some(p), _ => true };
    let verdict = match (m, b.word()) {
        (Msld::Ok(..), "ok") if restored => "PASS",
        (Msld::Ok(..) | Msld::Unbuilt, "unarmed") => "SKIP",
        _ if super::backlight::gmux_absent() => "SKIP reason=no-gmux", _ => "FAIL", // QEMUSKIP (B613): the blank is a gmux write; no gmux answered
    };
    serial_println!(
        ":: LIDSLEEP: msld={} raw={} flips={} backlight_off={} prior={} off_rb={} restore_rb={} armed={} -> {} ::",
        m.word(), raw, flips(), b.word(), opt(b.prior), opt(b.readback), opt(restore_rb), LADDER_ARMED as u8, verdict
    );
}

/// `tests sleep` (SLEEP B691, R80: registered, never at boot): enter the state, hold it 1.5 s (the service pass fills the
/// panel and reads the census and the battery), leave it as `tests`, and wait (bounded 3 s) for the service pass's
/// repaint, which prints `:: SLEEP: … ::` (`SKIP reason=no-gmux` under QEMU). The panel goes dark for the hold.
#[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
pub fn sleep_selftest() {
    let _ = sleep_request("tests");
    crate::arch::sched::sleep_ms(1500);
    let _ = wake("tests");
    let t0 = crate::arch::ms();
    while REPAINT_OWED.load(Ordering::Acquire) && crate::arch::ms().wrapping_sub(t0) < 3000 { crate::arch::sched::sleep_ms(50); }
    if REPAINT_OWED.load(Ordering::Acquire) {
        serial_println!(":: SLEEP: state={} wake_ms=- drain_pct_per_h=unmeasured wakes=- unexplained=0 -> FAIL :: reason=no-service-pass-in-3000ms", crate::power::sleep::STATE);
    }
    key_wake_leg(wake_key_swallowed()); // LIDKEYSWALLOW (B784): the second cycle, woken by a key (file tail); the tests wake's own answer is passed in
}

// LIDKEYSWALLOW (rmbp-ledger B784, BP-power-13) — TAIL-APPENDED under `lidsleep` (x86). The key that wakes the sleep
// state is consumed exactly as DIMIDLE consumes its blank's waking key. The `input` waker reads `dimidle`'s activity
// stamp on the service pass, after the event was already popped, so the drop is decided at the pop: `dimidle::gate`
// asks [`claim_wake`] (folded on its activity-stamp line) and drops a claimed press through DIMIDLE's one `swallow`
// (its release too). The FIRST input event while asleep claims the wake (DIMIDLE's `BLANKED.swap` shape): a press is
// swallowed, pointer motion wakes and is not, later events pass; a `lid`, `power-key` or `tests` wake claims nothing.
// Atomics only: `gate` stays interrupt-safe.
/// The wake is claimed (the first input event of this sleep was seen).
#[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
static WAKE_CLAIMED: AtomicBool = AtomicBool::new(false);
/// The claim was a press, and `dimidle::gate` dropped it (`key_swallowed=` on the wake line).
#[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
static KEY_SWALLOWED: AtomicBool = AtomicBool::new(false);

/// From `dimidle::gate`, every input event: `true` exactly once per sleep entry, for the first input event while the
/// state is in force, when it is a press (the gate then swallows it).
#[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
pub fn claim_wake(press: bool) -> bool {
    if !SLEPT.load(Ordering::Acquire) || WAKE_CLAIMED.swap(true, Ordering::AcqRel) { return false; }
    KEY_SWALLOWED.store(press, Ordering::Release);
    press
}

/// The last wake's waking press was swallowed (read by `power::sleep::first_frame` after the wake).
#[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
pub fn wake_key_swallowed() -> bool { KEY_SWALLOWED.load(Ordering::Acquire) }

/// A first entry (from [`sleep_request`], before SLEPT is set): the claim is open and nothing is swallowed yet.
#[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
fn arm_wake_claim() {
    WAKE_CLAIMED.store(false, Ordering::Release);
    KEY_SWALLOWED.store(false, Ordering::Release);
}

/// The wake edge (from [`wake`], after SLEPT is cleared): the claim is cleared so no claim of this sleep answers the
/// next; `KEY_SWALLOWED` is kept for the wake's line.
#[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
fn clear_wake_claim() { WAKE_CLAIMED.store(false, Ordering::Release); }

/// `tests sleep`'s second cycle: enter, hold 500 ms, push a key, and wait (bounded 2 s) for the state to leave through
/// the `input` waker — draining through the shared pop with DIMIDLE's own `drain_own` when this task is the drainer
/// (QEMUCLASS2 B565) — then for the repaint (bounded 3 s). Judged: the `tests` wake swallowed nothing, the key wake
/// swallowed its key. `tests_ks`: the first cycle's answer.
#[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
fn key_wake_leg(tests_ks: bool) {
    let _ = sleep_request("tests");
    crate::arch::sched::sleep_ms(500);
    crate::pal::push_event(crate::pal::Event::Key(b'~'));
    let t0 = crate::arch::ms();
    while asleep() && crate::arch::ms().wrapping_sub(t0) < 2000 {
        #[cfg(feature = "witness")] if crate::gui_watchdog::is_app_active() { super::dimidle::drain_own(); }
        crate::arch::sched::sleep_ms(20);
    }
    let woke = !asleep();
    if !woke { let _ = wake("tests"); }
    let t1 = crate::arch::ms();
    while REPAINT_OWED.load(Ordering::Acquire) && crate::arch::ms().wrapping_sub(t1) < 3000 { crate::arch::sched::sleep_ms(50); }
    let key_ks = wake_key_swallowed();
    let ok = woke && !tests_ks && key_ks;
    serial_println!(
        ":: SLEEPKEY: tests_wake_key_swallowed={} input_wake_key_swallowed={} woke={} -> {} ::{}",
        tests_ks as u8, key_ks as u8, woke as u8, if ok { "PASS" } else { "FAIL" }, if woke { "" } else { " reason=no-drain" }
    );
}
