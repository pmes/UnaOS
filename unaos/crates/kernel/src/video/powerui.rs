// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// POWERMENU (R75) — the power UI's state: the two-step confirm behind the Crystal menu's Restart/Shut Down rows (M1),
// the battery panel's text (M2) and the low-battery NOTICE / optional clean shutdown (M3). The painting lives in
// `crystal.rs` (the panel is a second MODE of the crystal dropdown, so it inherits that surface's occlusion, erase and
// clobber-repair); this file is state and text only. Gate: same as `status`/`crystal`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

/// How long an armed Restart/Shut Down row waits for its second click.
pub const CONFIRM_MS: u64 = 5000;

// ── M1: the two-step confirm ───────────────────────────────────────────────────────────────────
static ARMED_VERB: AtomicU8 = AtomicU8::new(0); // 0 none, 1 restart, 2 shut down
static ARMED_AT: AtomicU64 = AtomicU64::new(0);

/// First call for `verb` (1 restart / 2 shut down) arms and returns `false`; a second within [`CONFIRM_MS`] disarms and
/// returns `true` (fire). Any other verb's click re-arms for that verb.
pub fn confirm(verb: u8) -> bool {
    let now = crate::arch::ms();
    let live = ARMED_VERB.load(Ordering::Acquire) == verb && now.wrapping_sub(ARMED_AT.load(Ordering::Relaxed)) < CONFIRM_MS;
    if live {
        ARMED_VERB.store(0, Ordering::Release);
        serial_println!(":: POWER-UI: confirmed verb={} ::", verb_name(verb));
        return true;
    }
    ARMED_AT.store(now, Ordering::Relaxed);
    ARMED_VERB.store(verb, Ordering::Release);
    serial_println!(":: POWER-UI: armed verb={} window_ms={} ::", verb_name(verb), CONFIRM_MS);
    false
}

fn verb_name(v: u8) -> &'static str { if v == 1 { "restart" } else { "shutdown" } }

/// The armed verb (1/2) while its window is live, else 0. Mixed into the crystal's paint signature so the label change repaints.
pub fn armed_code() -> u8 {
    let v = ARMED_VERB.load(Ordering::Acquire);
    if v != 0 && crate::arch::ms().wrapping_sub(ARMED_AT.load(Ordering::Relaxed)) < CONFIRM_MS { v } else { 0 }
}

/// The override label for the Restart (1) / Shut Down (2) row while armed.
pub fn armed_label(verb: u8) -> Option<&'static str> {
    if armed_code() == verb { Some(if verb == 1 { "Click again to restart" } else { "Click again to shut down" }) } else { None }
}

/// The widest armed label, in glyphs (the menu width must clear it).
pub const ARMED_LABEL_MAX: usize = 24;

/// Disarm (menu dismissed).
pub fn disarm() { ARMED_VERB.store(0, Ordering::Release); }

// ── M2: the battery panel ──────────────────────────────────────────────────────────────────────
static PANEL: AtomicBool = AtomicBool::new(false);
static TEXT: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());
/// Panel width in glyphs.
pub const PANEL_GLYPHS: usize = 34;

pub fn panel_open() -> bool { PANEL.load(Ordering::Relaxed) }
pub fn panel_clear() { PANEL.store(false, Ordering::Release); }
pub fn panel_set() { PANEL.store(true, Ordering::Release); }
pub fn panel_rows() -> usize { TEXT.lock().len() }

/// Copy line `i` into `out`; returns its length (0 past the end).
pub fn panel_line(i: usize, out: &mut [u8]) -> usize {
    let t = TEXT.lock();
    let Some(s) = t.get(i) else { return 0 };
    let n = s.len().min(out.len());
    out[..n].copy_from_slice(&s.as_bytes()[..n]);
    n
}

/// Build the panel/verb lines from the live reading (`None` reading -> one honest line). Pure of the clock apart from the
/// reading's own age; the SMC extras are read only where an SMC exists.
pub fn battery_lines(reading: Option<crate::video::status::Battery>) -> Vec<String> {
    let mut v = Vec::new();
    let Some(b) = reading else {
        v.push(String::from("No battery on this board"));
        return v;
    };
    #[allow(unused_mut)]
    let (mut cycles, mut design, mut full, mut rem): (Option<u16>, Option<u16>, Option<u16>, Option<u16>) = (None, None, None, None);
    #[cfg(all(target_arch = "x86_64", feature = "smc"))]
    { let e = crate::drivers::smc::battery::extras(); cycles = e.0; design = e.1; full = e.2; rem = e.3; }
    v.push(format!("Battery: {}%", b.percent));
    v.push(String::from(if b.charging { "State: charging" } else { "State: discharging" }));
    match (b.charging, b.minutes) {
        (true, Some(m)) => v.push(format!("Time to full: {} min", m)),
        (false, _) if b.ma < 0 && rem.is_some() => v.push(format!("Time remaining: {} min", rem.unwrap() as u32 * 60 / (-(b.ma as i32)) as u32)),
        _ => v.push(String::from("Time remaining: n/a")),
    }
    v.push(format!("{} mV  {} mA", b.mv, b.ma));
    match cycles { Some(c) => v.push(format!("Cycle count: {}", c)), None => v.push(String::from("Cycle count: n/a")) }
    match (full, design) {
        (Some(f), Some(d)) if d > 0 => v.push(format!("Health: {}% ({}/{} mAh)", f as u32 * 100 / d as u32, f, d)),
        _ => v.push(String::from("Health: n/a")),
    }
    v
}

/// Fill the panel text from the live model.
pub fn panel_fill() -> usize {
    let lines = battery_lines(crate::video::status::reading().map(|(b, _)| b));
    let n = lines.len();
    *TEXT.lock() = lines;
    n
}

/// The `battery` shell verb: the same lines the panel shows.
pub fn shell_verb(console: &mut crate::console::Console) {
    let lines = battery_lines(crate::video::status::reading().map(|(b, _)| b));
    for l in lines.iter() { console.println(l); }
    serial_println!(":: BATTERY: lines={} first={} ::", lines.len(), lines.first().map(|s| s.as_str()).unwrap_or(""));
}

// ── M3: low-battery notice and optional clean shutdown ─────────────────────────────────────────
static DONE_MASK: AtomicU8 = AtomicU8::new(0); // bit0 = 10 % notice shown, bit1 = 5 % notice shown
static SHUT_REQ: AtomicBool = AtomicBool::new(false);

/// Pure threshold logic over a caller-owned mask: returns 10 or 5 when a NOTICE is newly owed (0 otherwise). Discharging only; dropping
/// straight below 5 owes only the 5 notice (and marks both).
pub fn threshold_hit(pct: u16, charging: bool, done: &mut u8) -> u8 {
    if charging { return 0; }
    if pct <= 5 && *done & 2 == 0 { *done |= 3; return 5; }
    if pct <= 10 && *done & 1 == 0 { *done |= 1; return 10; }
    0
}

#[cfg(feature = "lowbat_shutdown")]
const fn parse_pct(s: Option<&str>) -> u16 {
    let Some(s) = s else { return 0 };
    let b = s.as_bytes();
    let (mut i, mut n) = (0, 0u16);
    while i < b.len() { if b[i] >= b'0' && b[i] <= b'9' { n = n * 10 + (b[i] - b'0') as u16; } i += 1; }
    n
}
/// `UNAOS_LOWBAT_SHUTDOWN=<pct>` — 0 = off (the default). PREFS (B300): this is now only the DEFAULT; the
/// preference `system.power.lowbat_shutdown_pct` (Principia's store) overrides it at run time.
#[cfg(feature = "lowbat_shutdown")]
pub const LOWBAT_SHUTDOWN_PCT: u16 = parse_pct(option_env!("UNAOS_LOWBAT_SHUTDOWN"));

/// The shutdown percent in force: `system.power.lowbat_shutdown_pct` (0..=100) when set, else the build
/// default. Read with `prefs::peek_int` (a try-lock: this runs on the device-service path).
#[cfg(feature = "lowbat_shutdown")]
pub fn lowbat_shutdown_pct() -> u16 {
    crate::prefs::peek_int(crate::prefs::key::LOWBAT_PCT, 0, 100).map(|x| x as u16).unwrap_or(LOWBAT_SHUTDOWN_PCT)
}

fn post_notice(level: u8, pct: u16) {
    #[cfg(feature = "login")]
    crate::video::crystal::login::notice_show(b"Low Battery", format!("Battery at {}% ({} threshold)\nPlug in the charger", pct, level).as_bytes());
    serial_println!(":: POWER-UI: lowbat notice level={} pct={} ::", level, pct);
}

/// Called from the device-service task beside `status::poll` (never from a composite).
pub fn lowbat_service() {
    let Some((b, _)) = crate::video::status::reading() else { return };
    if crate::video::status::reading_is_fixture() { return; } // TESTFIX3: a fixture's injected percent is never the battery — no notice, no mask latch, no shutdown
    let mut m = DONE_MASK.load(Ordering::Relaxed);
    let hit = threshold_hit(b.percent, b.charging, &mut m);
    if hit != 0 { DONE_MASK.store(m, Ordering::Relaxed); post_notice(hit, b.percent); }
    #[cfg(feature = "lowbat_shutdown")]
    let pct = lowbat_shutdown_pct();
    #[cfg(feature = "lowbat_shutdown")]
    if pct > 0 && !b.charging && b.percent <= pct && !SHUT_REQ.swap(true, Ordering::AcqRel) {
        serial_println!(":: LOWBAT-SHUTDOWN armed: pct={} <= {} — clean shutdown ::", b.percent, pct);
        crate::power::shutdown();
    }
    let _ = &SHUT_REQ;
}

// ── fixture (`tests power`) ────────────────────────────────────────────────────────────────────
/// Battery panel open/close through the real crystal path on a forced reading, the NOTICE threshold logic with forced percents.
/// No real shutdown: the confirm is exercised, never fired.
pub fn selftest() {
    // notice logic: 12 -> none; 10 -> 10; 9 -> none (once); 4 -> 5; 3 -> none; charging at 2 -> none.
    let mut m = 0u8;
    let notice_ok = threshold_hit(12, false, &mut m) == 0 && threshold_hit(10, false, &mut m) == 10 && threshold_hit(9, false, &mut m) == 0
        && threshold_hit(4, false, &mut m) == 5 && threshold_hit(3, false, &mut m) == 0 && { let mut c = 0u8; threshold_hit(2, true, &mut c) == 0 && c == 0 }
        && { let mut d = 0u8; threshold_hit(3, false, &mut d) == 5 && threshold_hit(8, false, &mut d) == 0 };
    // confirm: first click arms, second fires; a different verb re-arms.
    let confirm_ok = !confirm(2) && armed_label(2).is_some() && armed_label(1).is_none() && confirm(2) && !confirm(1) && { disarm(); armed_code() == 0 };
    // panel: forced model reading -> open -> rows drawn -> close.
    let panel_ok = confirm_ok && crate::video::crystal::power_panel_selftest();
    serial_println!(":: POWER-UI: panel_ok={} notice_ok={} -> {} ::", panel_ok, notice_ok, if panel_ok && notice_ok { "PASS" } else { "FAIL" });
}
