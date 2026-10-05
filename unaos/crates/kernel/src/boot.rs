//! CHARTER: Kernel — kernel-by-ruling (R86 INSTALLBARE: the boot's one phase gate, read by every starter)
//!
//! INSTALLBARE (rmbp-ledger B364, R86 under R77/R80). Peter, flight 22, at the password setter: "nothing should be
//! running! … THAT BLOCK OF MY INPUT SHOULD NOT BE HAPPENING!!!!" The wire under the setter: a shell window minted and
//! swept, the compositor's WC-D valve CLOSED for 29 027 ms with the first key landing the second it reopened, 39
//! `[net]` + 18 `[usbnet]` + bthid + wifi + `[status]` running; and at boot 2's LOGIN SCREEN `[login] installer:
//! stage=desktop`, STAT.ELF, hda, net — because the login screen was a flag (`users::BOOT2`) no starter read.
//!
//! ONE gate, [`phase`], answers for the whole boot:
//!
//! * [`Phase::Setter`] — the store is not read yet, or it says Installer / CreateUser (root's password, the first user);
//! * [`Phase::LoginScreen`] — the store has users (boot 2) and no session has opened since the boot;
//! * [`Phase::Desktop`] — otherwise; LATCHED by the first session open ([`session_opened`], from
//!   `login::close_into_session`) or the installer's own `user-created` advance ([`ignite`]), so a later Log Out
//!   does not stop what the desktop started.
//!
//! [`phase`] is PURE (atomics only): `wm::create_inner` and `wm::verify_reference` ask it under the window table.
//! [`services_up`] adds the ORDER of the first login (M3): the furniture is re-minted first, and the services
//! (net, usbnet, bt, wifi, hda, status, the ring-3 probes, the WC-D valve) open when the shell window has
//! launched ([`furniture_ready`]) or [`SERVICES_BOUND_MS`] after the ignition, whichever is first.
//! [`services_gate`] is the task-context form: it prints the ONE `[boot] services up` line and the
//! `:: BOOT: login->desktop=` measurement, and counts the starters it held.
//!
//! M2 (the keyboard path): [`key_stamp`] at the EHCI decode while a secret screen is up, [`key_taken`] in
//! `login::consume_key` → `[login] key latency ms=` per key; [`hid_pass`] measures the longest gap between two
//! `service_ehci_hid` passes while a screen is up (the starvation a queue latency cannot see).
//! `tests installbare` ([`installbare_selftest`]) reports the pre-Desktop census.
//!
//! A build without `login` has no installer: [`phase`] answers Desktop from the first instruction.
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

/// The boot's phase. See the module doc.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Setter,
    LoginScreen,
    Desktop,
}

impl Phase {
    pub fn word(self) -> &'static str {
        match self {
            Phase::Setter => "setter",
            Phase::LoginScreen => "login-screen",
            Phase::Desktop => "desktop",
        }
    }
}

/// The Desktop phase is latched (a session opened / the installer advanced).
static LATCH: AtomicBool = AtomicBool::new(false);
/// When the latch was taken by a login or an advance (`arch::ms`, ≥ 1); 0 = the boot never had a pre-Desktop phase
/// that a login ended (a Desktop machine from its first stage, or a non-`login` build).
static IGNITE_MS: AtomicU64 = AtomicU64::new(0);
/// The phase the ignition ended: 0 none, 1 setter, 2 login-screen.
static FROM: AtomicU8 = AtomicU8::new(0);
/// The shell window launched after the ignition (the furniture is up).
static READY_MS: AtomicU64 = AtomicU64::new(0);
/// The `[boot] services up` line has been printed.
static SERVICES_SAID: AtomicBool = AtomicBool::new(false);
/// M3 — the services wait at most this long after the ignition for the furniture to report ready.
pub const SERVICES_BOUND_MS: u64 = 1_500;

/// **THE GATE.** Pure: atomics only, safe under any lock and in any context.
pub fn phase() -> Phase {
    #[cfg(not(feature = "login"))]
    {
        Phase::Desktop
    }
    #[cfg(feature = "login")]
    {
        use crate::fs::users;
        if LATCH.load(Ordering::Acquire) {
            return Phase::Desktop;
        }
        if !users::stage_resolved() {
            return Phase::Setter;
        }
        match users::boot_stage() {
            users::BootStage::Installer | users::BootStage::CreateUser => Phase::Setter,
            users::BootStage::Desktop => {
                if users::boot2_resolved() {
                    Phase::LoginScreen
                } else {
                    Phase::Desktop
                }
            }
        }
    }
}

/// Is the boot at its Desktop?
pub fn desktop() -> bool {
    phase() == Phase::Desktop
}

/// May a SERVICE start (net, usbnet, bt, wifi, hda, status, the ring-3 probes, the WC-D valve)? Pure.
pub fn services_up() -> bool {
    if phase() != Phase::Desktop {
        return false;
    }
    let ig = IGNITE_MS.load(Ordering::Acquire);
    if ig == 0 {
        return true;
    }
    READY_MS.load(Ordering::Acquire) != 0 || crate::arch::ms() >= ig.saturating_add(SERVICES_BOUND_MS)
}

/// [`services_up`] for a starter in TASK context: the first `true` prints the `[boot] services up` line and the
/// `:: BOOT: login->desktop=` measurement; a `false` is counted as a held ask (by starter name).
pub fn services_gate(who: &'static str) -> bool {
    // A store that never answers: `desktop_allowed` owns the bounded wait and its `no-store` Desktop publish. Asked only
    // from the device-service starters (never from under a controller lock: `bt`, `usbnet`), while unresolved.
    #[cfg(feature = "login")]
    if !crate::fs::users::stage_resolved() && who != "bt" && who != "usbnet" {
        let _ = crate::fs::users::desktop_allowed();
    }
    if services_up() {
        if !SERVICES_SAID.swap(true, Ordering::AcqRel) {
            services_line(who);
        }
        return true;
    }
    note_held(who);
    false
}

fn services_line(who: &'static str) {
    let now = crate::arch::ms();
    let ig = IGNITE_MS.load(Ordering::Acquire);
    let ready = READY_MS.load(Ordering::Acquire);
    serial_println!("[boot] services up first={} at={}ms held_starters={} (R86: nothing but the setter / the login dialog ran before this)", who, now, held_count());
    if ig != 0 {
        let from = match FROM.load(Ordering::Relaxed) { 1 => "setter", 2 => "login-screen", _ => "none" };
        let why = if ready != 0 { "furniture-ready" } else { "bound" };
        serial_println!(
            ":: BOOT: phase=desktop from={} login->desktop={} services_after_ms={} why={} ::",
            from,
            if ready != 0 { alloc::format!("{}ms", ready.saturating_sub(ig)) } else { alloc::string::String::from("none") },
            now.saturating_sub(ig),
            why
        );
    }
}

/// M3 — the Desktop phase begins NOW (`why`: `session` from the first login, `user-created` from the installer).
/// Idempotent: the first caller latches, prints `[boot] phase=desktop`, and starts the services clock.
pub fn ignite(why: &'static str) {
    let before = phase();
    if LATCH.swap(true, Ordering::AcqRel) {
        return;
    }
    if before == Phase::Desktop {
        return; // the boot was already a Desktop (no pre-Desktop phase to end): nothing to order
    }
    FROM.store(if before == Phase::LoginScreen { 2 } else { 1 }, Ordering::Relaxed);
    IGNITE_MS.store(crate::arch::ms().max(1), Ordering::Release);
    serial_println!(
        "[boot] phase=desktop from={} why={} at={}ms pre_windows={} pre_starts={} pre_valve={} (R86: the furniture first, then the services)",
        before.word(), why, crate::arch::ms(), PRE_WINDOWS.load(Ordering::Relaxed), PRE_STARTS.load(Ordering::Relaxed), PRE_VALVE.load(Ordering::Relaxed)
    );
}

/// A session opened at a login screen / the create-user form (`login::close_into_session`).
pub fn session_opened() {
    ignite("session");
}

/// The shell window launched (`dock::app_launched`): after an ignition, the furniture is up and the services may go.
pub fn furniture_ready() {
    if IGNITE_MS.load(Ordering::Acquire) != 0 && READY_MS.load(Ordering::Acquire) == 0 {
        READY_MS.store(crate::arch::ms().max(1), Ordering::Release);
    }
}

// ── the pre-Desktop census (what `tests installbare` reads) ────────────────────────────────────────────────────

/// Windows minted (owner ≠ 0: not the login screen's own band) while the phase was not Desktop.
static PRE_WINDOWS: AtomicU32 = AtomicU32::new(0);
/// Service starts ([`note_start`]) while the phase was not Desktop.
static PRE_STARTS: AtomicU32 = AtomicU32::new(0);
/// WC-D valve episodes that closed while the phase was not Desktop.
static PRE_VALVE: AtomicU32 = AtomicU32::new(0);
/// The first pre-Desktop service start's name (for the FAIL line).
static FIRST_START: spin::Mutex<&'static str> = spin::Mutex::new("");
/// Distinct starters the gate held, and the total asks.
static HELD: spin::Mutex<[&'static str; 16]> = spin::Mutex::new([""; 16]);
static HELD_ASKS: AtomicU32 = AtomicU32::new(0);

/// `wm::create_inner`: a row is being minted. Pure apart from one atomic add.
pub fn note_window(owner: u64) {
    if owner != 0 && phase() != Phase::Desktop {
        PRE_WINDOWS.fetch_add(1, Ordering::Relaxed);
    }
}

/// A service BEGAN its work (the begin point, behind its gate): counted when the phase is not Desktop.
pub fn note_start(who: &'static str) {
    if phase() != Phase::Desktop {
        PRE_STARTS.fetch_add(1, Ordering::Relaxed);
        if let Some(mut f) = FIRST_START.try_lock() {
            if f.is_empty() {
                *f = who;
            }
        }
    }
}

/// `wm::wcdvalve_closed`: a valve episode closed.
pub fn note_valve() {
    if phase() != Phase::Desktop {
        PRE_VALVE.fetch_add(1, Ordering::Relaxed);
    }
}

fn note_held(who: &'static str) {
    HELD_ASKS.fetch_add(1, Ordering::Relaxed);
    if let Some(mut h) = HELD.try_lock() {
        if !h.iter().any(|w| *w == who) {
            if let Some(s) = h.iter_mut().find(|w| w.is_empty()) {
                *s = who;
            }
        }
    }
}

fn held_count() -> usize {
    HELD.try_lock().map(|h| h.iter().filter(|w| !w.is_empty()).count()).unwrap_or(0)
}

// ── M2: the keyboard path ──────────────────────────────────────────────────────────────────────────────────────

/// A secret screen (setter / create-user / login / alert) is on the glass (`login::open_as` / `take_down`).
static SCREEN_UP: AtomicBool = AtomicBool::new(false);
const KR: usize = 32;
static KEY_RING: [AtomicU64; KR] = [const { AtomicU64::new(0) }; KR];
static KEY_HEAD: AtomicU32 = AtomicU32::new(0);
static KEY_TAIL: AtomicU32 = AtomicU32::new(0);
static KEY_N: AtomicU32 = AtomicU32::new(0);
static KEY_MAX: AtomicU64 = AtomicU64::new(0);
/// Pre-Desktop key stats for the fixture: the first key's latency + 1 (0 = none), the max.
static PRE_FIRST_KEY: AtomicU64 = AtomicU64::new(0);
static PRE_KEY_MAX: AtomicU64 = AtomicU64::new(0);
static PRE_KEYS: AtomicU32 = AtomicU32::new(0);
static HID_LAST: AtomicU64 = AtomicU64::new(0);
static HID_GAP_MAX: AtomicU64 = AtomicU64::new(0);
/// M2 bound: the longest a key may take from the EHCI decode to the screen.
pub const KEY_BOUND_MS: u64 = 50;

/// `login`: the screen is up (`true`) / taken down (`false`). A fresh open starts a fresh key queue.
pub fn note_screen(up: bool) {
    if up {
        KEY_TAIL.store(KEY_HEAD.load(Ordering::Acquire), Ordering::Release);
    }
    SCREEN_UP.store(up, Ordering::Release);
}

/// Is a secret screen up (pure)?
pub fn screen_is_up() -> bool {
    SCREEN_UP.load(Ordering::Acquire)
}

/// The EHCI decode pushed a key press: stamp it while a screen is up.
pub fn key_stamp() {
    if !SCREEN_UP.load(Ordering::Acquire) {
        return;
    }
    let h = KEY_HEAD.fetch_add(1, Ordering::AcqRel);
    KEY_RING[h as usize % KR].store(crate::arch::ms().max(1), Ordering::Release);
    let t = KEY_TAIL.load(Ordering::Acquire);
    if h.wrapping_sub(t) >= KR as u32 {
        let _ = KEY_TAIL.compare_exchange(t, h.wrapping_sub(KR as u32 - 1), Ordering::AcqRel, Ordering::Relaxed);
    }
}

/// `login::consume_key` took a key: pop its stamp and print `[login] key latency ms=` (unstamped keys — a fixture's,
/// the serial door's — print nothing).
pub fn key_taken() {
    let stamp = loop {
        let t = KEY_TAIL.load(Ordering::Acquire);
        if t == KEY_HEAD.load(Ordering::Acquire) {
            return;
        }
        let v = KEY_RING[t as usize % KR].load(Ordering::Acquire);
        if KEY_TAIL.compare_exchange(t, t.wrapping_add(1), Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            break v;
        }
    };
    let ms = crate::arch::ms().saturating_sub(stamp);
    let n = KEY_N.fetch_add(1, Ordering::Relaxed) + 1;
    let max = KEY_MAX.fetch_max(ms, Ordering::Relaxed).max(ms);
    if phase() != Phase::Desktop {
        PRE_KEYS.fetch_add(1, Ordering::Relaxed);
        let _ = PRE_FIRST_KEY.compare_exchange(0, ms + 1, Ordering::AcqRel, Ordering::Relaxed);
        PRE_KEY_MAX.fetch_max(ms, Ordering::Relaxed);
    }
    serial_println!(
        "[login] key latency ms={} max={} n={} hid_gap_max_ms={} bound={} (R86: the screen's input is never blocked)",
        ms, max, n, HID_GAP_MAX.load(Ordering::Relaxed), KEY_BOUND_MS
    );
}

/// `service_ehci_hid` entry: the gap since the previous pass, counted while a screen is up before the Desktop.
pub fn hid_pass() {
    let now = crate::arch::ms();
    let last = HID_LAST.swap(now, Ordering::AcqRel);
    if last != 0 && SCREEN_UP.load(Ordering::Acquire) && phase() != Phase::Desktop {
        HID_GAP_MAX.fetch_max(now.saturating_sub(last), Ordering::Relaxed);
    }
}

// ── `tests installbare` ────────────────────────────────────────────────────────────────────────────────────────

/// Register `tests installbare` once (folded into `tests::ensure_shellux`).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("installbare", installbare_selftest);
    }
}

/// `tests installbare` — what this boot did BEFORE its Desktop: no service start, no window but the screen's,
/// no valve episode, and every key at the screen within [`KEY_BOUND_MS`].
/// `:: INSTALLBARE: phase=setter services=0 windows=0 valve=none first_key_ms=<n> … -> PASS ::`
pub fn installbare_selftest() {
    let from = match FROM.load(Ordering::Relaxed) {
        1 => "setter",
        2 => "login-screen",
        _ => {
            serial_println!(":: INSTALLBARE: phase=desktop services=- windows=- valve=- first_key_ms=- -> SKIP reason=no-pre-desktop-phase ::");
            return;
        }
    };
    let services = PRE_STARTS.load(Ordering::Relaxed);
    let windows = PRE_WINDOWS.load(Ordering::Relaxed);
    let valve = PRE_VALVE.load(Ordering::Relaxed);
    let keys = PRE_KEYS.load(Ordering::Relaxed);
    let first = PRE_FIRST_KEY.load(Ordering::Relaxed);
    let kmax = PRE_KEY_MAX.load(Ordering::Relaxed);
    let gap = HID_GAP_MAX.load(Ordering::Relaxed);
    let num = |v: u64| alloc::format!("{}", v);
    let pass = services == 0 && windows == 0 && valve == 0 && (keys == 0 || kmax <= KEY_BOUND_MS);
    if !pass {
        serial_println!(
            ":: INSTALLBARE: reason=services={} first_start={} windows={} valve={} key_max_ms={} bound={} ::",
            services, FIRST_START.try_lock().map(|f| *f).unwrap_or("?"), windows, valve, kmax, KEY_BOUND_MS
        );
    }
    serial_println!(
        ":: INSTALLBARE: phase={} services={} windows={} valve={} first_key_ms={} key_max_ms={} keys={} hid_gap_max_ms={} held={} -> {} ::",
        from,
        services,
        windows,
        if valve == 0 { alloc::string::String::from("none") } else { num(valve as u64) },
        if first == 0 { alloc::string::String::from("none") } else { num(first - 1) },
        if keys == 0 { alloc::string::String::from("none") } else { num(kmax) },
        keys,
        gap,
        held_count(),
        if pass { "PASS" } else { "FAIL" }
    );
}

// ── M4: GLASSEYES' bare scene ──────────────────────────────────────────────────────────────────────────────────

/// `shot setter` / `shot login` hold the furniture (bar, dock) while the bare scene is captured.
static SHOT_BARE: AtomicBool = AtomicBool::new(false);
pub fn shot_bare(on: bool) {
    SHOT_BARE.store(on, Ordering::Release);
}
pub fn shot_bare_held() -> bool {
    SHOT_BARE.load(Ordering::Acquire)
}
