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
        probe_reset();
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
        probe_pass();
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
    probe_pass();
    #[cfg(feature = "login")]
    if crate::fs::users::furniture_held() {
        return; // the session's stage has not published yet (flight 24 card 1's race): stay owed
    }
    let _ = HELD_AT.compare_exchange(0, crate::arch::ms().max(1), AcqRel, Relaxed); // KVMLAT2: the first pass that found the stage published
    let waited = crate::arch::ms().saturating_sub(at);
    if super::status::source() == super::status::Source::Unresolved && waited < BATT_WAIT_MS && cfg!(all(target_arch = "x86_64", feature = "wc")) {
        return; // the first SMC sweep lands ~1 ms after the services open: the bar's first paint carries the battery
    }
    if OWED_AT.compare_exchange(at, 0, AcqRel, Relaxed).is_err() {
        return;
    }
    probe_built(at); #[cfg(all(target_arch = "x86_64", feature = "wc"))] let t_build = crate::arch::ms(); // FIRSTLOGIN (B636): the build's own body (enable, wallpaper rearm, the composite) timed for the bar's steps row. LINE-NEUTRAL append, code first
    PREBUILT.store(super::menubar::owns_pixels() || super::dock::owns_pixels(), Relaxed);
    LAST_BATT.store(batt_key(), Relaxed);
    BUILT.store(true, Release);
    let _ = super::menubar::set_enabled(true);
    #[cfg(feature = "facet")]
    super::wallpaper::rearm(); // the session user's ~/Desktop/WALL.PNG, probed on this composite's flush
    PAINT_AT.store(at, Release);
    super::wm::composite(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] steps::built(t_build); // FIRSTLOGIN (B636). LINE-NEUTRAL append, code first
    if super::menubar::owns_pixels() && PAINT_AT.compare_exchange(at, 0, AcqRel, Relaxed).is_ok() {
        say(crate::arch::ms().saturating_sub(at));
    }
}

fn say(bar_ms: u64) {
    LAST_BAR_MS.store(bar_ms.saturating_add(1), Relaxed); let bar = super::menubar::owns_pixels();
    let has_batt = super::status::bar_item().is_some();
    let no_source = super::status::source() == super::status::Source::None;
    let battery = if bar && has_batt { "painted" } else if no_source { "absent" } else { "unpainted" };
    let dock = super::dock::owns_pixels();
    let at_login = AT_LOGIN.load(Relaxed);
    serial_println!(
        "[desktop] built at={} bar_ms={} battery={} dock={} pins={} held_ms={} batt_ms={} gap_max_ms={}",
        if at_login { "login ok" } else { "stage" }, bar_ms, battery, if dock { "painted" } else { "unpainted" }, super::dock::pins_pinned(),
        HELD_MS.load(Relaxed), BATT_MS.load(Relaxed), GAP_MAX_MS.load(Relaxed) // KVMLAT2 (B576): where the bar's span went
    ); #[cfg(all(target_arch = "x86_64", feature = "wc"))] steps::say(bar_ms); // FIRSTLOGIN (B636): `[desktop] bar steps=…`, only past STEPS_OVER_MS. LINE-NEUTRAL append, code first
    if !FIRST_SAID.swap(true, AcqRel) {
        let prebuilt = PREBUILT.load(Relaxed);
        #[cfg(feature = "login")]
        let swept = super::crystal::login::swept_n();
        #[cfg(not(feature = "login"))]
        let swept = 0u32;
        let pass = !prebuilt && swept == 0 && bar && bar_ms <= PAINT_WAIT_MS && (battery == "painted" || no_source) && dock;
        serial_println!(
            ":: DESKTOPBUILT: prebuilt={} built_at={} bar_first_paint_ms={} battery={} dock={} swept={} -> {} ::",
            prebuilt as u8, if at_login { "login" } else { "stage" }, bar_ms, (battery == "painted") as u8, dock as u8, swept, crate::tests::boot_word(pass) // SMALLFIX7 (B501): a record at boot
        );
    }
}

/// Log Out: the desktop object goes — the bar off, unbuilt, one composite vacates the bar's and the dock's pixels.
pub fn teardown(why: &'static str) {
    OWED_AT.store(0, Release);
    probe_reset();
    PAINT_AT.store(0, Release);
    LAST_BATT.store(u32::MAX, Relaxed);
    if !BUILT.swap(false, AcqRel) {
        return;
    }
    let off = super::menubar::set_enabled(false);
    super::wm::composite();
    serial_println!("[desktop] torn down why={} bar_off={}", why, off as u8);
}

// JOBSCAN (B497): the login's deferred work (the jobs scan, the owed filetypes registry) waits for the bar.
/// The last build's `bar_ms` + 1 (`0` = never said).
static LAST_BAR_MS: AtomicU64 = AtomicU64::new(0);

/// Is the desktop built and its first paint read back (nothing owed, nothing pending)?
pub fn settled() -> bool {
    BUILT.load(Acquire) && OWED_AT.load(Acquire) == 0 && PAINT_AT.load(Acquire) == 0
}

/// The last build's `bar_ms` (`None` = no build said yet).
pub fn last_bar_ms() -> Option<u64> {
    match LAST_BAR_MS.load(Relaxed) {
        0 => None,
        n => Some(n - 1),
    }
}

// KVMLAT2 (B576): THE BAR'S SPAN, SPLIT. Image 26's `[desktop] built at=login ok bar_ms=425` held a 12801 -> 12960 ms
// span the wire did not explain. The `[desktop] built` line now ends `held_ms=<ask -> the first pass that found the
// session's stage published (furniture_held)> batt_ms=<that pass -> the build (the battery source's bounded wait)>
// gap_max_ms=<the longest gap between two service passes from the ask to the line>` — bar_ms minus those two is the
// first paint's read-back; a large gap_max_ms names the pass that ran something else (the HDA walk, B576) between.
static HELD_AT: AtomicU64 = AtomicU64::new(0);
static LAST_PASS: AtomicU64 = AtomicU64::new(0);
static HELD_MS: AtomicU64 = AtomicU64::new(0);
static BATT_MS: AtomicU64 = AtomicU64::new(0);
static GAP_MAX_MS: AtomicU64 = AtomicU64::new(0);

/// A new ask (or a teardown): the span's readings start over.
fn probe_reset() { #[cfg(all(target_arch = "x86_64", feature = "wc"))] steps::reset(); // FIRSTLOGIN (B636). LINE-NEUTRAL append, code first
    for a in [&HELD_AT, &LAST_PASS, &HELD_MS, &BATT_MS, &GAP_MAX_MS] {
        a.store(0, Relaxed);
    }
}

/// A service pass while the build is owed or its paint pending: the gap since the previous one.
fn probe_pass() {
    let now = crate::arch::ms().max(1); #[cfg(all(target_arch = "x86_64", feature = "wc"))] steps::pass(); // FIRSTLOGIN (B636): the passes the span took. LINE-NEUTRAL append, code first
    let last = LAST_PASS.swap(now, Relaxed);
    if last != 0 {
        GAP_MAX_MS.fetch_max(now.saturating_sub(last), Relaxed);
    }
}

/// The build ran (ask stamp `at`): the stage wait and the battery wait, fixed.
fn probe_built(at: u64) {
    let (now, held_at) = (crate::arch::ms(), HELD_AT.load(Relaxed));
    let held_at = if held_at == 0 { now } else { held_at };
    HELD_MS.store(held_at.saturating_sub(at), Relaxed);
    BATT_MS.store(now.saturating_sub(held_at), Relaxed);
}

// FIRSTLOGIN (rmbp-ledger B636): THE BAR'S STEPS, NAMED — and the starter that ran inside them. Flight 34 boot 1 (a fresh
// card, the first login): `[desktop] built at=login ok bar_ms=341`; boot 2 (same card): `bar_ms=40`. The wire
// (f34-boot1.log 15:01:19Z) puts NO first-login work in that span: the home, the prefs, the settings, the Holocron ring
// and the KDF all print BEFORE `[login] session open` (they ran on the submit's worker), and the ask comes after
// `[filetypes] login=none … ms=11`. What sits between the ask and the paint is `[usbnet] bring-up try=0 of 3
// kind=ax88179 … ms=333 -> armed`: the dongle was plugged at boot 1 (unplugged at boot 2's login — its bring-up is
// 15:27:51Z there), its class bring-up is held by `boot::services_gate("usbnet")` until the session's ignition
// (drivers/xhci/usbnet.rs `bringup_pending`), and it then runs in the usb-pump's `xhci` step — the SAME loop whose
// `desktop-app` step runs [`service`] (main.rs, `lag::P_XHCI` before `lag::P_DESKAPP`), so the build's passes stopped
// for 333 ms and the battery's first poll (`[status] poll n=1`, the next line) came after it. KVMLAT2's HDA walk was
// the same shape (B576); this is its second tenant. So: (1) the row below says which step of the span the time went
// to and whether that step was a pump gap (another step of the loop running between two passes) rather than its own
// work; (2) [`starter_holds`] is KVMLAT2's `hda::bar_holds` made one call for every pump starter — a class bring-up
// asks it after its services gate and waits (bounded) while the desktop is owed. Row, said once per build and only
// when `bar_ms > STEPS_OVER_MS`: `[desktop] bar steps=stage:<n>,batt:<n>,build:<n>,paint:<n> bar_ms=<n>
// gap_max_ms=<n> passes=<n> worst=<step>:<ms> why=<pump-gap|own> held_starters=<a+b|none>` — stage = the ask to the
// first pass that found the session's stage published, batt = that pass to the build (the battery source's bounded
// wait), build = the build's own body (enable, wallpaper rearm, the composite), paint = the rest (the first paint's
// read-back); `why=pump-gap` when the longest gap between two passes is at least half the worst step.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod steps {
    use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::{AcqRel, Acquire, Relaxed}};

    /// The row prints only past this (boot 2's bar is 40 ms; the target is within 2x of it).
    pub const STEPS_OVER_MS: u64 = 100;
    /// The starters [`super::starter_holds`] knows by name (bit i of [`HELD`] / [`SAID`]); any other is `other`.
    const STARTERS: [&str; 5] = ["usbnet", "bt", "wifi", "hda", "other"];
    /// A starter waits at most this long from the first hold (= `fs::jobs::BAR_WAIT_MS`, KVMLAT2's HDA fallback,
    /// restated here because `fs::jobs` rides `unafs`): a desktop that never settles costs the bring-up that bound.
    const STARTER_WAIT_MS: u64 = 10_000;

    static BUILD_MS: AtomicU64 = AtomicU64::new(0);
    static PASSES: AtomicU32 = AtomicU32::new(0);
    /// Starters held behind the bar since the last ask (bits of [`STARTERS`]).
    static HELD: AtomicU32 = AtomicU32::new(0);
    /// Starters whose release line was said (once per boot each).
    static SAID: AtomicU32 = AtomicU32::new(0);
    /// The first hold's stamp (`ms().max(1)`, 0 = none yet): the bound runs from here.
    static HOLD_T0: AtomicU64 = AtomicU64::new(0);

    pub(super) fn reset() {
        BUILD_MS.store(0, Relaxed);
        PASSES.store(0, Relaxed);
        HELD.store(0, Relaxed);
    }

    pub(super) fn pass() {
        PASSES.fetch_add(1, Relaxed);
    }

    pub(super) fn built(t0: u64) {
        BUILD_MS.store(crate::arch::ms().saturating_sub(t0), Relaxed);
    }

    fn bit(who: &str) -> u32 {
        1 << STARTERS.iter().position(|s| *s == who).unwrap_or(STARTERS.len() - 1)
    }

    /// [`super::starter_holds`]'s body: `true` = hold `who` this pass.
    pub(super) fn holds(who: &'static str) -> bool {
        let now = crate::arch::ms().max(1);
        let t0 = match HOLD_T0.compare_exchange(0, now, AcqRel, Acquire) { Ok(_) => now, Err(v) => v };
        let waited = now.saturating_sub(t0);
        let settled = super::settled();
        if !settled && waited < STARTER_WAIT_MS {
            HELD.fetch_or(bit(who), Relaxed);
            return true;
        }
        if SAID.fetch_or(bit(who), AcqRel) & bit(who) == 0 {
            serial_println!("[desktop] starter after-bar who={} waited_ms={} why={}", who, waited, if settled { "settled" } else { "timeout" });
        }
        false
    }

    pub(super) fn say(bar_ms: u64) {
        if bar_ms <= STEPS_OVER_MS {
            return;
        }
        let (stage, batt, gap) = (super::HELD_MS.load(Relaxed), super::BATT_MS.load(Relaxed), super::GAP_MAX_MS.load(Relaxed));
        let build = BUILD_MS.load(Relaxed);
        let paint = bar_ms.saturating_sub(stage.saturating_add(batt).saturating_add(build));
        let (wn, wv) = [("stage", stage), ("batt", batt), ("build", build), ("paint", paint)].into_iter().fold(("-", 0u64), |a, p| if p.1 > a.1 { p } else { a });
        let why = if gap > 0 && gap.saturating_mul(2) >= wv { "pump-gap" } else { "own" };
        let held = HELD.load(Relaxed);
        let mut names = alloc::string::String::new();
        for (i, s) in STARTERS.iter().enumerate() {
            if held & (1 << i) != 0 {
                if !names.is_empty() {
                    names.push('+');
                }
                names.push_str(s);
            }
        }
        serial_println!(
            "[desktop] bar steps=stage:{},batt:{},build:{},paint:{} bar_ms={} gap_max_ms={} passes={} worst={}:{} why={} held_starters={}",
            stage, batt, build, paint, bar_ms, gap, PASSES.load(Relaxed), wn, wv, why, if names.is_empty() { "none" } else { names.as_str() }
        );
    }
}

/// FIRSTLOGIN (B636): **may a usb-pump starter `who` run its bring-up now?** `true` = not yet: the desktop is not
/// settled (its build owed or its first paint pending — or not asked yet: the services open at the ignition, a few ms
/// before `login::close_into_session` asks), bounded by `STARTER_WAIT_MS` from the first hold. Asked AFTER the
/// starter's `boot::services_gate`, from the pump (never under a controller lock). Said once per starter at release:
/// `[desktop] starter after-bar who=<w> waited_ms=<n> why=<settled|timeout>`; a held starter is named on the bar's
/// steps row (`held_starters=`).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn starter_holds(who: &'static str) -> bool {
    steps::holds(who)
}
