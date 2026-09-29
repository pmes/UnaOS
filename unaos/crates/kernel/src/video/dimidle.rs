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
    if o != 0 { o } else { IDLE_MIN as u64 * 60_000 }
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
    #[cfg(feature = "witness")]
    fixture();
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
            if now < START_MS || blanked() { return; }
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
fn finish(ok: bool, swallowed: u32) {
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
