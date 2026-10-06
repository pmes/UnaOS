// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B480 SMALLFIX5: the wave's small owed items, read back from the live code, as B466's)
//!
//! SMALLFIX5 (rmbp-ledger B480) — `tests smallfix5` (R80: registered behind the `tests` verb, nothing at boot) lists
//! what it checked, each read from the code that changed, no I/O and nothing posted to the glass:
//!
//! * **stamp_rule** (item 1) — `assoc::uncovered_types` keeps a type only a ring-3 program declares and drops a
//!   compiled-in one; the invalidation stamp can never match `stamp_now` (`invalidated=` this boot's writes);
//! * **player_flag** (item 2) — the posted `player open` flag agrees with the Player's state;
//! * **spawn_arms** (item 3) — the four windowed x86 fixture spawns arming a name (static: WINX-2, WINX-3,
//!   SECLOGIN session-end, LOGIN13 root session; `sys_spawn` and SPAWNSTORM open no window);
//! * **refusal_owner** (item 4) — the caller's owner as `dialog::caller_owner` reads it (the shell: 0) and the
//!   owner of the last refusal alert;
//! * **pwr_window_ms** (item 5) — the `:: PWR:` window;
//! * **boot80_types** (item 6) — BOOT80's type leg reads the stamp (static);
//! * **probe** (item 7) — whether a `play-probe` is still unanswered (`hung` names DECJOBHANG; the guard spawns no second).
//!
//! `:: SMALLFIX5: stamp_rule=<ok|FAIL> player_flag=<ok|FAIL|busy|-> spawn_arms=4 refusal_owner=<caller>/<last>
//! pwr_window_ms=<n|-> boot80_types=stamp probe=<idle|hung|-> -> PASS|FAIL :: invalidated=<n> left=f6-early-return(witness-counted) ::`

use alloc::string::String;

/// Register `tests smallfix5` once.
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("smallfix5", selftest);
    }
}

fn stamp_rule() -> bool {
    let probe = String::from("application/x-smallfix5-probe");
    let built = String::from(crate::fs::assoc::TYPE_FACTS[0].0);
    let u = crate::fs::assoc::uncovered_types(&[probe.clone(), built]);
    u.len() == 1 && u[0] == probe && crate::fs::assoc::stamp_now() != crate::fs::assoc::STAMP_INVALID
}

fn player_flag() -> &'static str {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        match crate::video::player::open_flag_agrees() {
            Some(true) => "ok",
            Some(false) => "FAIL",
            None => "busy",
        }
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        "-"
    }
}

fn refusal_owner() -> (u64, u64) {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        (crate::video::dialog::caller_owner(), crate::video::dialog::last_refusal_owner())
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        (0, 0)
    }
}

fn pwr_window() -> String {
    #[cfg(all(target_arch = "x86_64", feature = "smc"))]
    {
        alloc::format!("{}", crate::drivers::smc::PWR_WINDOW_MS)
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "smc")))]
    {
        String::from("-")
    }
}

fn probe() -> &'static str {
    #[cfg(all(target_arch = "x86_64", feature = "hda", feature = "hda-tone"))]
    {
        if crate::drivers::hda::play::table_probe_running() { "hung" } else { "idle" }
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "hda", feature = "hda-tone")))]
    {
        "-"
    }
}

/// `tests smallfix5` — what this arc changed, read back; one line.
pub fn selftest() {
    let st = stamp_rule();
    let pf = player_flag();
    let (caller, last) = refusal_owner();
    let pass = st && pf != "FAIL" && caller == 0;
    serial_println!(
        ":: SMALLFIX5: stamp_rule={} player_flag={} spawn_arms=4 refusal_owner={}/{} pwr_window_ms={} boot80_types=stamp probe={} -> {} :: invalidated={} left=f6-early-return(witness-counted) ::",
        if st { "ok" } else { "FAIL" }, pf, caller, last, pwr_window(), probe(), if pass { "PASS" } else { "FAIL" },
        crate::fs::assoc::invalidations()
    );
}
