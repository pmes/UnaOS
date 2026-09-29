//! R77 M3 — THE TEST SUITE IS A COMMAND.
//!
//! Every fixture that drives the desktop, the input router or the login screen used to ignite as a
//! boot side-effect, so a first boot with no root password ran the whole press battery beneath the
//! modal set-password screen and every press fixture went red (boot 16). A fixture now REGISTERS
//! itself here instead of running; the operator fires the registry from the desktop shell with
//! `tests` (all) or `tests <name>`, which tells a boot problem from a test problem.
//!
//! * Default: `register` stores the fixture; the boot prints ONE line
//!   `:: TESTS: deferred=<n> fire=tests ::` once every registering source has been through.
//! * `tests-at-boot` (`UNAOS_TESTS_AT_BOOT=1`, the QEMU lanes' default): `register` RUNS the fixture on
//!   the spot — the call site is the old call site, so the lanes see the old order — and the boot line
//!   reads `deferred=0`.
//! * `tests` / `tests <name>` / `tests list`: refused until `fs::users::desktop_allowed()` (the
//!   installer's gate — no test runs beneath the set-password or create-user screens).
//!
//! Pass/fail is the serial verdict tap's own count (`selftest::capture` calls [`tally`] for every
//! `-> PASS` / `-> FAIL` line), so the total is exactly what the fixtures printed.
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use crate::console::Console;

/// Registry capacity — a full table is loud (`:: TESTS: table full … -> FAIL ::`), never silent.
const CAP: usize = 48;

static TABLE: spin::Mutex<[Option<(&'static str, fn())>; CAP]> = spin::Mutex::new([None; CAP]);
static DEFERRED: AtomicUsize = AtomicUsize::new(0);
static AT_BOOT: AtomicUsize = AtomicUsize::new(0);
static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Registering sources whose completion the boot line waits for: the desktop battery (x86 witness
/// images) and the loginst chain. Other registrations (early input fixtures) precede both.
pub const SRC_DESK: u32 = 1;
pub const SRC_LOGIN: u32 = 2;
const REQUIRED: u32 = (if cfg!(all(target_arch = "x86_64", feature = "witness")) { SRC_DESK } else { 0 })
    | (if cfg!(feature = "loginst") { SRC_LOGIN } else { 0 });
static SOURCES_DONE: AtomicU32 = AtomicU32::new(0);
static ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// Called by the verdict tap for every fixture verdict line.
pub fn tally(pass: bool) {
    if pass { PASS.fetch_add(1, Ordering::Relaxed); } else { FAIL.fetch_add(1, Ordering::Relaxed); }
}

/// How many fixtures are parked behind the verb.
pub fn deferred_count() -> usize { DEFERRED.load(Ordering::Relaxed) }

/// Register a fixture. Deferred by default; run at once under `tests-at-boot`.
pub fn register(name: &'static str, f: fn()) {
    #[cfg(feature = "tests-at-boot")]
    {
        AT_BOOT.fetch_add(1, Ordering::Relaxed);
        f();
        let _ = name;
    }
    #[cfg(not(feature = "tests-at-boot"))]
    {
        let mut t = TABLE.lock();
        match t.iter_mut().find(|s| s.is_none()) {
            Some(slot) => {
                *slot = Some((name, f));
                DEFERRED.fetch_add(1, Ordering::Relaxed);
            }
            None => serial_println!(":: TESTS: table full (cap={}) — `{}` NOT registered -> FAIL ::", CAP, name),
        }
    }
}

/// A registering source has finished; once all required sources have, print the boot line ONCE.
pub fn source_done(bit: u32) {
    let done = SOURCES_DONE.fetch_or(bit, Ordering::AcqRel) | bit;
    if done & REQUIRED == REQUIRED && !ANNOUNCED.swap(true, Ordering::AcqRel) {
        serial_println!(":: TESTS: deferred={} fire=tests at_boot={} ::", deferred_count(), AT_BOOT.load(Ordering::Relaxed));
    }
}

/// Run one named fixture (`Some`) or all (`None`); returns how many ran.
pub fn run(name: Option<&str>) -> usize {
    if RUNNING.swap(true, Ordering::AcqRel) {
        serial_println!(":: TESTS: already running — refused ::");
        return 0;
    }
    let (p0, f0) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    let mut ran = 0usize;
    let mut i = 0usize;
    loop {
        // Copy the entry out so the table lock is NOT held across the fixture (fixtures print, spin, and may register nothing).
        let ent = { let t = TABLE.lock(); if i >= CAP { None } else { t[i] } };
        let Some((n, f)) = ent else { break };
        i += 1;
        if let Some(want) = name { if want != n { continue; } }
        serial_println!(":: TESTS: run {} ::", n);
        f();
        ran += 1;
    }
    let (p, f) = (PASS.load(Ordering::Relaxed).wrapping_sub(p0), FAIL.load(Ordering::Relaxed).wrapping_sub(f0));
    serial_println!(":: TESTS: ran={} pass={} fail={} ::", ran, p, f);
    RUNNING.store(false, Ordering::Release);
    ran
}

/// The `tests` shell verb: `tests` (all) · `tests <name>` · `tests list`.
pub fn shell_verb(args: &[&str], console: &mut Console) {
    #[cfg(feature = "login")]
    if !crate::fs::users::desktop_allowed() {
        console.println("tests: refused — finish first-boot setup (root password, then create a user) before the desktop suite runs");
        return;
    }
    if args.first().copied() == Some("list") {
        let t = TABLE.lock();
        for e in t.iter().flatten() { console.println(e.0); }
        console.println(&format!("{} deferred, {} ran at boot", deferred_count(), AT_BOOT.load(Ordering::Relaxed)));
        return;
    }
    let name = args.first().copied();
    let (p0, f0) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    let ran = run(name);
    let (p, f) = (PASS.load(Ordering::Relaxed).wrapping_sub(p0), FAIL.load(Ordering::Relaxed).wrapping_sub(f0));
    if ran == 0 && name.is_some() {
        console.println("tests: no such fixture (try `tests list`)");
    } else {
        console.println(&format!("tests: ran={} pass={} fail={}", ran, p, f));
    }
}
