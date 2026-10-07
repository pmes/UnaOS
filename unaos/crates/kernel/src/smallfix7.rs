// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B501 SMALLFIX7: flight 27's small reds, each cause named on the wire)
//!
//! SMALLFIX7 (rmbp-ledger B501) — `tests smallfix7` (R80: behind the `tests` verb, nothing at boot) lists what it
//! checked, each item read from the live state the fix touches:
//!
//! * **deferred** (item 2) — the `TESTS: deferred=` announce printed this boot (`ladder_arm` signals the desktop
//!   source BOOTVERDICTS stranded); **verdicts** — verdict lines counted before it, by tag;
//! * **word** (item 2) — `tests::boot_word` reads PASS/FAIL now that a `tests` run has closed the boot's window;
//! * **ownrun** (item 3) — `defer_fast` bodies run only for their own fixture (this one is running, `prtscrdir` is not);
//! * **prtscr** (item 3) — `tests prtscr` is registered;
//! * **ring** (item 4) — the lines an unopened console's prefill would replay (`LOGINFURN ring_tail_lines=`);
//! * **preheap** (item 5) — `midden_core::fixtures::admit` before the heap parks without calling the table's push;
//! * **saves** (item 1, x86 wc) — window frames written this boot (the WINMEMORY store's trigger is move-end/close).
//!
//! `:: SMALLFIX7: deferred=<announced|MISSING> verdicts=<n>:<tags> word=<ok|FAIL> ownrun=<ok|FAIL> prtscr=<registered|MISSING>
//! ring=<n> preheap=<parked|FAIL> saves=<n|-> -> PASS|FAIL ::`

/// Register `tests smallfix7` (and `tests prtscr`, latched with `dir_fixture`'s own arm) once.
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("smallfix7", selftest);
        crate::video::prtscr::prtscr_fixture_arm();
    }
}

fn selftest() {
    use midden_core::fixtures::{admit, Admit, Stash};
    let announced = crate::tests::announced();
    let (n, tags) = crate::tests::boot_verdicts();
    let word = crate::tests::boot_word(true) == "PASS" && crate::tests::boot_word(false) == "FAIL";
    let ownrun = crate::tests::running_is("smallfix7") && !crate::tests::running_is("prtscrdir");
    let prtscr = crate::tests::registered("prtscr");
    let ring = crate::loginfurn::ring_tail_lines();
    let mut st: Stash<u8, 1> = Stash::new();
    let mut pushed = 0u32;
    let preheap = admit(&mut st, false, 1, |_| pushed += 1) == Admit::Parked && pushed == 0;
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    let saves = alloc::format!("{}", crate::video::wm::winmemory::saves());
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    let saves = alloc::string::String::from("-");
    let pass = announced && n == 0 && word && ownrun && prtscr && ring > 0 && preheap;
    let v = |b: bool, y: &'static str, f: &'static str| if b { y } else { f };
    serial_println!(
        ":: SMALLFIX7: deferred={} verdicts={}:{} word={} ownrun={} prtscr={} ring={} preheap={} saves={} -> {} ::",
        v(announced, "announced", "MISSING"), n, tags, v(word, "ok", "FAIL"), v(ownrun, "ok", "FAIL"),
        v(prtscr, "registered", "MISSING"), ring, v(preheap, "parked", "FAIL"), saves, v(pass, "PASS", "FAIL")
    );
}
