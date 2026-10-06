//! CHARTER: Kernel — kernel-by-ruling (R92/R93 DESKTOPBUILT: the desktop is an object built at `login ok`, torn down at logout)
//!
//! DESKTOPBUILT (rmbp-ledger B387). Peter, flight 24: "i've been talking about not loading the desktop prior to
//! login. there should not have been a problem redrawing these because drawing them should not have been done in
//! the first place. / battery did not appear in menubar until i clicked on the crystal menu" (R93).
//!
//! Before a session there is NO desktop: [`built`] is false and `strip::compose_all` paints no furniture (it
//! vacates the bar's and the dock's pixels instead). [`build`] is asked at `login ok` (`login::close_into_session`)
//! and at a store-less Desktop stage (`users::stage_publish`); the build itself runs in [`service`] — on x86 the
//! device-service pass (`desktop_uefi::desktop_app_service`, the task that already sweeps the SMC; never the
//! key/click path, never masked), elsewhere inline. It waits (bounded) for the session's stage and the battery
//! source, turns the bar on, re-arms the wallpaper and composites ONCE, then reads back that the bar and the dock
//! own pixels. While built, a change of the battery item composites (the bar's damage follows its model).
//! [`teardown`] (Log Out) unbuilds it; the next login builds fresh. Nothing is held, owed or swept.
//!
//! Lines: `[desktop] built at=<login ok|stage> bar_ms=<n> battery=<painted|absent|unpainted> dock=<painted|unpainted> pins=<n>`
//! (every build), `:: DESKTOPBUILT: prebuilt=<0|1> built_at=<login|stage> bar_first_paint_ms=<n> battery=<0|1>
//! dock=<0|1> swept=<n> -> PASS|FAIL ::` (the boot's first build), `[desktop] torn down why=<w> bar_off=<0|1>`.
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};

static BUILT: AtomicBool = AtomicBool::new(false);
/// `ms().max(1)` of the build's ask (`login ok`), `0` = no build owed.
static OWED_AT: AtomicU64 = AtomicU64::new(0);
/// The build ran and its first paint is being read back: the ask's stamp, `0` = nothing pending.
static PAINT_AT: AtomicU64 = AtomicU64::new(0);
static AT_LOGIN: AtomicBool = AtomicBool::new(false);
static PREBUILT: AtomicBool = AtomicBool::new(false);
static FIRST_SAID: AtomicBool = AtomicBool::new(false);
/// The battery item the bar last composited with (`u32::MAX` = not built).
static LAST_BATT: AtomicU32 = AtomicU32::new(u32::MAX);
/// The battery source gets this long to answer before the bar paints without it.
const BATT_WAIT_MS: u64 = 500;
/// The first paint's read-back bound (a composite declined by a sibling's gate is re-driven inside it).
const PAINT_WAIT_MS: u64 = 500;

/// Is there a desktop? `strip::compose_all`'s gate.
pub fn built() -> bool {
    BUILT.load(Acquire)
}

/// Ask for the desktop (`why` = `login ok` or `stage`). Idempotent while one is built or owed.
pub fn build(why: &'static str) {
    if !BUILT.load(Acquire) && OWED_AT.compare_exchange(0, crate::arch::ms().max(1), AcqRel, Relaxed).is_ok() {
        AT_LOGIN.store(why == "login ok", Relaxed);
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    service(); // no device-service pass carries it off x86 (and no SMC battery to wait for): build inline; a later `build` retries
}

fn batt_key() -> u32 {
    match super::status::bar_item() {
        Some(b) => ((b.percent as u32) << 1) | b.charging as u32,
        None => u32::MAX - 1,
    }
}

/// The build, its first paint's read-back, and the battery follow. Cheap when idle: three relaxed loads.
pub fn service() {
    let paint_at = PAINT_AT.load(Acquire);
    if paint_at != 0 {
        let ms = crate::arch::ms().saturating_sub(paint_at);
        if super::menubar::owns_pixels() || ms >= PAINT_WAIT_MS {
            if PAINT_AT.compare_exchange(paint_at, 0, AcqRel, Relaxed).is_ok() {
                say(ms);
            }
        } else {
            super::wm::composite(); // a pass declined by a sibling core's gate: drive another, bounded by PAINT_WAIT_MS
        }
        return;
    }
    let at = OWED_AT.load(Acquire);
    if at == 0 {
        if BUILT.load(Relaxed) {
            let k = batt_key();
            if LAST_BATT.swap(k, Relaxed) != k {
                super::wm::composite(); // the bar's model changed (battery): repaint it now, not on the next press
            }
        }
        return;
    }
    #[cfg(feature = "login")]
    if crate::fs::users::furniture_held() {
        return; // the session's stage has not published yet (flight 24 card 1's race): stay owed
    }
    let waited = crate::arch::ms().saturating_sub(at);
    if super::status::source() == super::status::Source::Unresolved && waited < BATT_WAIT_MS && cfg!(all(target_arch = "x86_64", feature = "wc")) {
        return; // the first SMC sweep lands ~1 ms after the services open: the bar's first paint carries the battery
    }
    if OWED_AT.compare_exchange(at, 0, AcqRel, Relaxed).is_err() {
        return;
    }
    PREBUILT.store(super::menubar::owns_pixels() || super::dock::owns_pixels(), Relaxed);
    LAST_BATT.store(batt_key(), Relaxed);
    BUILT.store(true, Release);
    let _ = super::menubar::set_enabled(true);
    #[cfg(feature = "facet")]
    super::wallpaper::rearm(); // the session user's ~/Desktop/WALL.PNG, probed on this composite's flush
    PAINT_AT.store(at, Release);
    super::wm::composite();
    if super::menubar::owns_pixels() && PAINT_AT.compare_exchange(at, 0, AcqRel, Relaxed).is_ok() {
        say(crate::arch::ms().saturating_sub(at));
    }
}

fn say(bar_ms: u64) {
    let bar = super::menubar::owns_pixels();
    let has_batt = super::status::bar_item().is_some();
    let no_source = super::status::source() == super::status::Source::None;
    let battery = if bar && has_batt { "painted" } else if no_source { "absent" } else { "unpainted" };
    let dock = super::dock::owns_pixels();
    let at_login = AT_LOGIN.load(Relaxed);
    serial_println!(
        "[desktop] built at={} bar_ms={} battery={} dock={} pins={}",
        if at_login { "login ok" } else { "stage" }, bar_ms, battery, if dock { "painted" } else { "unpainted" }, super::dock::pins_pinned()
    );
    if !FIRST_SAID.swap(true, AcqRel) {
        let prebuilt = PREBUILT.load(Relaxed);
        #[cfg(feature = "login")]
        let swept = super::crystal::login::swept_n();
        #[cfg(not(feature = "login"))]
        let swept = 0u32;
        let pass = !prebuilt && swept == 0 && bar && bar_ms <= PAINT_WAIT_MS && (battery == "painted" || no_source) && dock;
        serial_println!(
            ":: DESKTOPBUILT: prebuilt={} built_at={} bar_first_paint_ms={} battery={} dock={} swept={} -> {} ::",
            prebuilt as u8, if at_login { "login" } else { "stage" }, bar_ms, (battery == "painted") as u8, dock as u8, swept, if pass { "PASS" } else { "FAIL" }
        );
    }
}

/// Log Out: the desktop object goes — the bar off, unbuilt, one composite vacates the bar's and the dock's pixels.
pub fn teardown(why: &'static str) {
    OWED_AT.store(0, Release);
    PAINT_AT.store(0, Release);
    LAST_BATT.store(u32::MAX, Relaxed);
    if !BUILT.swap(false, AcqRel) {
        return;
    }
    let off = super::menubar::set_enabled(false);
    super::wm::composite();
    serial_println!("[desktop] torn down why={} bar_off={}", why, off as u8);
}
