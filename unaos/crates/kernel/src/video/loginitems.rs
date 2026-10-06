// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — shared-core
//!
//! LOGIN ITEMS (PREFSUI, rmbp-ledger B389, R91: "auto launched items should be in the prefs panel"; flight 25:
//! "there's no login items list in prefs"). What launches itself at login is a USER PREFERENCE —
//! `system.login.items` in Principia's store, a comma-joined list of program names, EMPTY by default (R88). The
//! list rules (parse, render, toggle, move) are `prefs_core::login`, the shared core both rings link; this module
//! keeps the session's copy, writes it over the bus (`prefs_client`, SETTINGSBUS) and launches it.
//!
//! * Editors: the Settings window's **Login Items** tab (add from the installed programs, remove, move up/down)
//!   and the dock tile menu's **Open at Login** row — both edit the cache and latch ONE store write, drained by
//!   [`service`] (never a bus write inside the click router).
//!   `[settings] login_items op=<op> name=<n> items=<list|none> via=<settings|dock>`.
//! * The launch: `users::login` posts [`post_login`] at `login ok`; [`service`] (the settings service pass) reads
//!   the user's list once the desktop is built (`users::furniture_held()` false) and launches each item through the
//!   dock's launch seams (`dock::launch_named` — the same posts a tile press makes), in order.
//!   `[login] items n=<n> launched=<list or none>`.
//! * The installed programs a list may name are the dock's app table (`dock::installed_names`).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static ITEMS: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());
/// The session user the cache was read for (empty = not read).
static LOADED_FOR: spin::Mutex<String> = spin::Mutex::new(String::new());
static SAVE_OWED: AtomicBool = AtomicBool::new(false);
static LAUNCH_OWED: AtomicBool = AtomicBool::new(false);
/// Items launched at the last login (the witness reads it).
static LAUNCHED_N: AtomicU32 = AtomicU32::new(0);

/// `users::login`: the session opened — read this user's list and launch it once the desktop is built.
pub fn post_login() {
    LOADED_FOR.lock().clear();
    LAUNCH_OWED.store(true, Ordering::Release);
}

/// The programs a login item may name: the dock's app table (what this build can launch).
pub fn installed() -> Vec<&'static str> {
    crate::video::dock::installed_names()
}

/// The session's list (a copy; empty before the first read or with no session).
pub fn items() -> Vec<String> {
    ITEMS.lock().clone()
}

/// Is `name` a login item? Never blocks (the dock menu asks while painting): unknown = `false`.
pub fn contains(name: &str) -> bool {
    ITEMS.try_lock().map(|l| prefs_core::login::contains(&l, name)).unwrap_or(false)
}

fn list_word(l: &[String]) -> String {
    if l.is_empty() { String::from("none") } else { prefs_core::login::render(l) }
}

/// One edit of the list: `op` ∈ add | remove | toggle | up | down. Latches the store write. Returns whether the
/// list changed (toggle: and the new membership is in the line).
pub fn edit(op: &str, name: &str, via: &str) -> bool {
    let changed = {
        let mut l = ITEMS.lock();
        let before = l.clone();
        match op {
            "add" => { if !prefs_core::login::contains(&l, name) { prefs_core::login::toggle(&mut l, name); } }
            "remove" => { prefs_core::login::remove(&mut l, name); }
            "toggle" => { prefs_core::login::toggle(&mut l, name); }
            "up" | "down" => {
                if let Some(i) = l.iter().position(|x| x.eq_ignore_ascii_case(name)) {
                    prefs_core::login::move_by(&mut l, i, if op == "up" { -1 } else { 1 });
                }
            }
            _ => {}
        }
        let ch = *l != before;
        serial_println!("[settings] login_items op={} name={} items={} via={} changed={}", op, name, list_word(&l), via, ch as u8);
        ch
    };
    if changed { SAVE_OWED.store(true, Ordering::Release); }
    changed
}

/// The settings service pass: read the session's list (once per login), drain an owed store write, and run the
/// login's launch once the desktop is built.
pub fn service() {
    let mut wb = [0u8; crate::prefs::WHO_BUF]; // PERFREVIEW F3 (B443): compared on the stack; a String only when the session changed
    let Some(u) = crate::prefs::user_name_in(&mut wb) else { return };
    let fresh = { let g = LOADED_FOR.lock(); *g != u };
    if fresh {
        let l = crate::prefs_client::sys_text(crate::prefs::key::LOGIN_ITEMS).map(|t| prefs_core::login::parse(&t)).unwrap_or_default();
        *ITEMS.lock() = l;
        *LOADED_FOR.lock() = String::from(u);
        SAVE_OWED.store(false, Ordering::Release);
    }
    if SAVE_OWED.swap(false, Ordering::AcqRel) {
        let v = prefs_core::login::render(&ITEMS.lock());
        if let Err(e) = crate::prefs_client::pref_set(crate::prefs::NS, crate::prefs::key::LOGIN_ITEMS, crate::prefs::PrefValue::Str(v)) {
            serial_println!("[settings] login_items save failed status={}", e);
        }
    }
    #[cfg(feature = "login")]
    let built = !crate::fs::users::furniture_held();
    #[cfg(not(feature = "login"))]
    let built = true;
    if built && LAUNCH_OWED.swap(false, Ordering::AcqRel) {
        launch();
    }
}

/// Launch every item, in order, through the dock's seams. `[login] items n=<n> launched=<list or none>`.
fn launch() {
    let l = items();
    let mut done: Vec<&str> = Vec::new();
    let mut refused = String::new();
    let mut shell_for_verbs = false;
    for n in l.iter() {
        match crate::video::dock::launch_named(n) {
            Some(how) => {
                serial_println!("[login] item launch name={} how={} via=login-items", n, how);
                if how == "verb-posted" { shell_for_verbs = true; }
                done.push(n.as_str());
            }
            None => { if !refused.is_empty() { refused.push(','); } refused.push_str(n); }
        }
    }
    // PREFSUI M6: LOGINFURN's credit — one window per launched item, plus the shell a ring-3 item's verb runs in.
    crate::loginfurn::login_items_posted(done.len() as u32 + shell_for_verbs as u32);
    LAUNCHED_N.store(done.len() as u32, Ordering::Relaxed);
    let w = if done.is_empty() { String::from("none") } else { done.join(",") };
    serial_println!("[login] items n={} launched={}{}{} (R91: Settings > Login Items)", l.len(), w, if refused.is_empty() { "" } else { " unknown=" }, refused);
}

/// Items launched at the last login.
pub fn launched() -> u32 {
    LAUNCHED_N.load(Ordering::Relaxed)
}

/// `tests settings` leg: toggle a program in and out of the list through [`edit`] and the shared rules; the list
/// ends as it began. Returns the list's length after the round trip, or `None` on a mismatch.
#[cfg(feature = "witness")]
pub fn selftest() -> Option<usize> {
    let before = items();
    let name = *installed().iter().find(|n| !prefs_core::login::contains(&before, n))?;
    let added = edit("add", name, "selftest") && contains(name);
    let removed = edit("remove", name, "selftest") && !contains(name);
    let same = items() == before;
    (added && removed && same).then_some(before.len())
}
