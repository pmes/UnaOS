//! DIMIDLE — idle screen blanking, both desktops (x86 `wc`, aarch64 `desktop_firmware`).
//!
//! After `UNAOS_IDLE_MIN` minutes (default 10, 0 = never) with no key or pointer event the panel goes
//! black and the compositor stops presenting: [`blanked`] is a third term of `video::panel_refuse_term`,
//! so `wm::composite` and the cursor sprite decline exactly as they do on a dying machine. Any input
//! event wakes it; the waking KEY/button/wheel is swallowed at `pal::pop_event` (the one drain every
//! consumer shares) so it never reaches a window, and [`service`] repaints (backdrop + all windows
//! damaged + composite). Pointer motion wakes but is not swallowed.
//!
//! No backlight step: the gmux code in this tree (drivers/gpu/igpu.rs) is a MUX switch only, with no
//! brightness port, so the blank is a black panel, not a dimmed backlight.
//!
//! Interrupt-safe: atomics only, no heap, no lock in [`gate`]. The paint work lives in [`service`],
//! called from `bootpace::service_dump` (every service lane, ungated).
use crate::pal::Event;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// SCREENLOCK M3: build-time `UNAOS_LOCK_ON_IDLE=1` — a wake from the idle blank opens the lock screen. Default off.
pub const LOCK_ON_IDLE: bool = match option_env!("UNAOS_LOCK_ON_IDLE") { Some(v) => v.len() == 1 && v.as_bytes()[0] == b'1', None => false };
/// Build-time idle minutes (`UNAOS_IDLE_MIN`), default 10, 0 = never.
pub const IDLE_MIN: u32 = parse_min(option_env!("UNAOS_IDLE_MIN"));
const IDLE_MIN_DEFAULT: u32 = 10;

/// Decimal 0..=1440 at compile time; anything else is the default (model: `hda::parse_amp`).
const fn parse_min(s: Option<&str>) -> u32 {
    let Some(s) = s else { return IDLE_MIN_DEFAULT };
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 4 { return IDLE_MIN_DEFAULT; }
    let mut v: u32 = 0; let mut i = 0;
    while i < b.len() { let c = b[i]; if c < b'0' || c > b'9' { return IDLE_MIN_DEFAULT; } v = v * 10 + (c - b'0') as u32; i += 1; }
    if v > 1440 { IDLE_MIN_DEFAULT } else { v }
}

static LAST_ACTIVITY_MS: AtomicU64 = AtomicU64::new(0);
static BLANKED: AtomicBool = AtomicBool::new(false);
static WAKE_OWED: AtomicBool = AtomicBool::new(false);
/// Test override of the threshold in ms (0 = use the build value).
static THRESH_OVERRIDE_MS: AtomicU64 = AtomicU64::new(0);
static BLANKED_AT_MS: AtomicU64 = AtomicU64::new(0);
static WOKE_AT_MS: AtomicU64 = AtomicU64::new(0);
static SWALLOWED: AtomicU32 = AtomicU32::new(0);
/// Key byte whose press was swallowed on wake; its release is swallowed once too.
static WAKE_KEY_UP: AtomicU32 = AtomicU32::new(0);

/// The panel is blanked: the compositor and cursor must not write it.
pub fn blanked() -> bool { BLANKED.load(Ordering::Relaxed) }

fn threshold_ms() -> u64 {
    let o = THRESH_OVERRIDE_MS.load(Ordering::Relaxed);
    if o != 0 { o } else { idle_min() as u64 * 60_000 }
}

/// The input seam, called from `pal::pop_event` for every popped event. Stamps the idle clock on any
/// key/pointer event, wakes a blanked panel, and returns `true` when the event must be dropped (the
/// waking key/button/wheel, and that key's release).
pub fn gate(ev: Event) -> bool {
    let (input, press) = match ev {
        Event::Key(_) | Event::Button(_) | Event::Wheel(_) => (true, true),
        Event::KeyUp(_) => (true, false),
        Event::Mouse { .. } | Event::MouseAbsolute { .. } => (true, false),
        _ => (false, false),
    };
    if !input { return false; }
    LAST_ACTIVITY_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
    if BLANKED.swap(false, Ordering::AcqRel) {
        WOKE_AT_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
        WAKE_OWED.store(true, Ordering::Release);
        if press {
            SWALLOWED.fetch_add(1, Ordering::Relaxed);
            if let Event::Key(b) = ev { WAKE_KEY_UP.store(b as u32 | 0x100, Ordering::Relaxed); }
            return true;
        }
        return false;
    }
    if let Event::KeyUp(b) = ev {
        let w = WAKE_KEY_UP.load(Ordering::Relaxed);
        if w == b as u32 | 0x100 { WAKE_KEY_UP.store(0, Ordering::Relaxed); return true; }
    }
    false
}

/// One service pass: blank on idle, repaint after a wake, run the fixture. Cheap when nothing is due.
pub fn service() {
    if WAKE_OWED.swap(false, Ordering::AcqRel) {
        if let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) {
            fb.fill_screen(super::wm::DESKTOP_BG);
            fb.flush_all();
            let (w, h) = (fb.width(), fb.height());
            let _ = super::wm::damage_intersecting(0, 0, w, h);
        }
        super::wm::composite();
        #[cfg(feature = "login")] if LOCK_ON_IDLE { let _ = super::crystal::login::lock(); } // SCREENLOCK M3 — the wake lands on the lock screen (`UNAOS_LOCK_ON_IDLE=1`); refused harmlessly with no session
    }
    let th = threshold_ms();
    if th != 0 && !BLANKED.load(Ordering::Relaxed) {
        let now = crate::arch::ms();
        if now.wrapping_sub(LAST_ACTIVITY_MS.load(Ordering::Relaxed)) >= th {
            if let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) {
                fb.fill_screen(0);
                fb.flush_all();
                BLANKED_AT_MS.store(now.max(1), Ordering::Relaxed);
                BLANKED.store(true, Ordering::Release);
            }
        }
    }
    super::lidsleep::service(); // LIDSLEEP (B431): rung 0's MSLD poll (knob) and the sleep blank's wake
    #[cfg(feature = "witness")]
    { dimidle_arm(); fixture(); }
}

/// The witness: a 1-second test threshold, wait for the blank, inject a key, check the wake and that
/// the key was dropped at the shared drain (so no window can have received it). A state machine
/// across passes, so the service lane never sleeps.
#[cfg(feature = "witness")]
fn fixture() {
    use core::sync::atomic::AtomicU8;
    static ST: AtomicU8 = AtomicU8::new(0);
    static T0: AtomicU64 = AtomicU64::new(0);
    static S0: AtomicU32 = AtomicU32::new(0);
    const START_MS: u64 = 12_000;
    let now = crate::arch::ms();
    match ST.load(Ordering::Relaxed) {
        0 => {
            if now < START_MS || blanked() || !FX_ARMED.load(Ordering::Acquire) { return; }
            S0.store(SWALLOWED.load(Ordering::Relaxed), Ordering::Relaxed);
            THRESH_OVERRIDE_MS.store(1000, Ordering::Relaxed);
            LAST_ACTIVITY_MS.store(now.max(1), Ordering::Relaxed);
            T0.store(now, Ordering::Relaxed);
            ST.store(1, Ordering::Relaxed);
        }
        1 => {
            if blanked() {
                // while blanked the compositor must be refusing on the idle term
                let refused = super::panel_refuse_term() == Some("idle-blank");
                if !refused { finish(false, 0); ST.store(3, Ordering::Relaxed); return; }
                crate::pal::push_event(Event::Key(b'~'));
                T0.store(now, Ordering::Relaxed);
                ST.store(2, Ordering::Relaxed);
            } else if now.wrapping_sub(T0.load(Ordering::Relaxed)) > 6000 {
                finish(false, 0); ST.store(3, Ordering::Relaxed);
            }
        }
        2 => {
            let swallowed = SWALLOWED.load(Ordering::Relaxed).wrapping_sub(S0.load(Ordering::Relaxed));
            if swallowed != 0 || now.wrapping_sub(T0.load(Ordering::Relaxed)) > 2000 {
                let ok = swallowed == 1 && !blanked();
                finish(ok, swallowed);
                ST.store(3, Ordering::Relaxed);
            }
        }
        _ => {}
    }
}

#[cfg(feature = "witness")]
fn finish(ok: bool, swallowed: u32) { FX_DONE.store(true, Ordering::Release);
    THRESH_OVERRIDE_MS.store(0, Ordering::Relaxed);
    LAST_ACTIVITY_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
    serial_println!(
        ":: DIMIDLE: idle_min={} blanked_at_ms={} woke_at_ms={} wake_key_swallowed={} -> {} ::",
        IDLE_MIN,
        BLANKED_AT_MS.load(Ordering::Relaxed),
        WOKE_AT_MS.load(Ordering::Relaxed),
        swallowed,
        if ok { "PASS" } else { "FAIL" }
    );
}

/// SETTINGS (R75): runtime idle minutes; `u32::MAX` = not set, use the build value `IDLE_MIN`.
static IDLE_MIN_RT: AtomicU32 = AtomicU32::new(u32::MAX);
/// The idle minutes in force (runtime value if set, else the build value). 0 = never.
pub fn idle_min() -> u32 { let v = IDLE_MIN_RT.load(Ordering::Relaxed); if v == u32::MAX { IDLE_MIN } else { v } }
/// SETTINGS: set the idle minutes at runtime (0 = never; capped at 1440) and restart the idle clock.
pub fn set_idle_min(n: u32) {
    IDLE_MIN_RT.store(n.min(1440), Ordering::Relaxed);
    LAST_ACTIVITY_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
}

/// LIDSLEEP (B431): the last key/pointer event's time (ms; 0 = none yet) — the sleep blank's wake reads it.
pub fn last_activity_ms() -> u64 { LAST_ACTIVITY_MS.load(Ordering::Relaxed) }

// BOOTVERDICTS (rmbp-ledger B472, R80) — TAIL-APPENDED. The DIMIDLE witness BLANKED THE PANEL at 12 s and
// INJECTED A KEY beneath the set-password screen (flight 25, f25-boots.log 442): a test at boot, and one that
// touches the very input R86 says is never blocked. The state machine now waits for `tests dimidle` to arm it.
#[cfg(feature = "witness")]
static FX_ARMED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "witness")]
static FX_DONE: AtomicBool = AtomicBool::new(false);

/// The service pass's one-shot: register `tests dimidle` (under `tests-at-boot` this arms it at once).
#[cfg(feature = "witness")]
fn dimidle_arm() {
    static REG: AtomicBool = AtomicBool::new(false);
    if !REG.swap(true, Ordering::AcqRel) {
        crate::tests::register("dimidle", selftest);
    }
}

/// `tests dimidle`: arm the 1 s blank-and-wake witness and wait, bounded, for its one line.
#[cfg(feature = "witness")]
pub fn selftest() {
    if FX_DONE.load(Ordering::Acquire) {
        serial_println!(":: DIMIDLE: skipped reason=once-per-boot ::");
        return;
    }
    FX_ARMED.store(true, Ordering::Release);
    if cfg!(feature = "tests-at-boot") { return; }
    let dl = crate::arch::ms() + 12_000;
    while !FX_DONE.load(Ordering::Acquire) && crate::arch::ms() < dl {
        crate::arch::sched::yield_now();
    }
    if !FX_DONE.load(Ordering::Acquire) {
        serial_println!(":: DIMIDLE: skipped reason=service-silent (armed; the line prints when the service lane runs) ::");
    }
}
