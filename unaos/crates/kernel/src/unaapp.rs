//! CHARTER: Una — vessel over shared-core edit_core (Tabula) + midden_core (Midden); ring-3 app, MIDDENTWIN shape
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! UNAMETAL (LEDGER SR90, R104) — `tests una`: the ring-3 IDE, driven and read back.
//!
//! `APPS/UNA.ELF` (crates/user-una) is the Una IDE's UnaOS-native face: one window split three ways — a file tree,
//! an editor over edit_core, a console that is MIDDEN.ELF's loop over midden_core. This fixture is `middenapp.rs`'s
//! shape: find the image on the volume, run the production validator (`arch::x86_64::elf::validate_elf`, the one
//! `bg` runs) in the ELF-window model, check the windowed app note (`midden_core::app_note_flags`), SPAWN it the way
//! `bg` does (`spawn_user_image_bg`), wait for `:: UNA: start`, then drive it the way the input router does —
//! events into its ring by owner (`user_input_push_owner`):
//!
//!   1. a KEY_UP of `3` with no press before it — exactly what the HID decoder delivers for Ctrl+3 (the press of a
//!      Ctrl+digit is suppressed, its release is not; see user-una's FOCUS note): the console takes the focus
//!      (the program starts with the editor focused, so a line that reaches the console proves the chord);
//!   2. `help⏎` as KEY_DOWNs, waited on the console's `:: UNA: plan=` line;
//!   3. Ctrl+Q (KEY_DOWN 0x11): the program quits with its packed EXIT STATUS, read back through `bg_poll`.
//!
//! PASS = the loader takes the image in the ELF model with the windowed note, the program starts, reports three
//! panes, its window, ONE console line planned whose kind is Say and verb `help`, and more console rows than the
//! core's ring-3 `help` text alone (so the help drew in the console pane). The single-producer note is
//! middenapp.rs's: do not type into the new window during `tests una` on the bench.
//!
//! WITNESS. `:: UNAAPP: image=/apps/UNA.ELF window=<elf|fixed|bad> panes=<n> lines=<n> -> PASS|FAIL|SKIP ::`
//! preceded by `[unaapp] …` (pid, slot, timings, the decoded status) and, on FAIL/SKIP, `:: UNAAPP: reason=… ::`.
//! No `UNA.ELF` on the volume is a SKIP (the builder stages it; a bare kernel has none).

use crate::arch::x86_64::syscall::{self as sc, BgPoll};

const IMAGE: &str = "/apps/UNA.ELF";
const START: &str = ":: UNA: start";
const PLANNED: &str = ":: UNA: plan=";
/// The window, one mono face off the volume and the workspace's first listing come before the start line.
const START_MS: u64 = 20_000;
const LINE_MS: u64 = 5_000;
const EXIT_MS: u64 = 5_000;
const PANES: u32 = 3;

/// Register `tests una` once (`tests::ensure_shellux`'s line; `tests-at-boot` runs it there).
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("una", selftest);
    }
}

/// user-una's `status` read back: `(rows, cmds, kind, verb, panes, window, face)` — rows `[9:0]`, cmds `[15:10]`,
/// kind `[17:16]`, verb code `[25:18]`, panes `[27:26]`, window `[28]`, face `[29]`. Kept in step with
/// crates/user-una/src/main.rs `status` by hand (the program links no kernel crate, the kernel no program).
const fn status_parse(s: i32) -> (u32, u32, u32, u8, u32, bool, bool) {
    let s = s as u32;
    (s & 0x3FF, (s >> 10) & 0x3F, (s >> 16) & 3, ((s >> 18) & 0xFF) as u8, (s >> 26) & 3, (s >> 28) & 1 != 0, (s >> 29) & 1 != 0)
}

fn verdict(window: &str, panes: u32, lines: u32, v: &str) {
    serial_println!(":: UNAAPP: image={} window={} panes={} lines={} -> {} ::", IMAGE, window, panes, lines, v);
}

fn why(r: &str) {
    serial_println!(":: UNAAPP: reason={} ::", r);
}

/// Wait until the watched line is seen, the task ends, or `ms` pass. `Ok(ms taken)` on the line.
fn wait_line(pid: u64, ms: u64) -> Result<u64, &'static str> {
    let t0 = crate::arch::ticks();
    while crate::arch::ticks() < t0 + ms {
        if crate::serial_line::line_watch_hit() {
            return Ok(crate::arch::ticks().saturating_sub(t0));
        }
        match sc::bg_poll(pid, false) {
            BgPoll::Running => {}
            BgPoll::Faulted => return Err("faulted"),
            BgPoll::Exited(_) => return Err("exited-early"),
            _ => return Err("gone"),
        }
        crate::arch::sched::yield_now();
    }
    Err("timeout")
}

/// Push one event into the ring of the process in `slot`.
fn push(slot: u64, ty: u64, b: u8) -> bool {
    sc::user_input_push_owner(slot + 1, una_abi::input_ev_pack(ty, b as u64))
}

/// Type `s` into the ring, one KEY_DOWN per byte (`\n` is Enter, as the router sends it).
fn type_line(slot: u64, s: &[u8]) -> bool {
    let mut ok = true;
    for &b in s {
        ok &= push(slot, una_abi::INPUT_EV_KEY_DOWN, b);
    }
    ok
}

pub fn selftest() {
    let Ok(fs) = crate::fs::rootdisk::program_source() else {
        why("no-program-volume");
        return verdict("bad", 0, 0, "SKIP");
    };
    let cap = sc::user_image_cap();
    let img = match fs.find_app("UNA.ELF") {
        Ok(de) if de.size != 0 && de.size as usize <= cap => {
            let mut b = alloc::vec::Vec::new();
            if fs.read_file(&de, &mut b, cap).is_err() {
                why("read-failed");
                return verdict("bad", 0, 0, "FAIL");
            }
            b
        }
        Ok(_) => {
            why("size-out-of-range");
            return verdict("bad", 0, 0, "FAIL");
        }
        Err(_) => {
            why("UNA.ELF-not-on-the-volume");
            return verdict("bad", 0, 0, "SKIP");
        }
    };
    let plan = match crate::arch::x86_64::elf::validate_elf(&img, sc::user_window_size()) {
        Ok(p) => p,
        Err(e) => {
            serial_println!(":: UNAAPP: reason=loader-refused ({}) ::", e);
            return verdict("bad", 0, 0, "FAIL");
        }
    };
    let window = if plan.model_elf { "elf" } else { "fixed" };
    let note = midden_core::app_note_flags(&img);
    if !plan.model_elf {
        why("not-the-elf-model");
        return verdict(window, 0, 0, "FAIL");
    }
    if note.map_or(true, |f| f & una_abi::APP_FLAG_WINDOWED == 0) {
        why("no-windowed-app-note");
        return verdict(window, 0, 0, "FAIL");
    }

    // The spawn `bg` makes: APPRES sight first, then the image, then the launch name.
    crate::fs::appres::sight(IMAGE);
    crate::serial_line::line_watch_arm(START);
    let (pid, slot, entry) = match sc::spawn_user_image_bg(&img) {
        Ok(t) => {
            crate::video::wm::app_name_arm_launch(crate::video::wm::owner_of_launch(t.1), IMAGE);
            t
        }
        Err(e) => {
            crate::serial_line::line_watch_disarm();
            serial_println!(":: UNAAPP: reason=spawn-refused ({}) ::", e);
            return verdict(window, 0, 0, "FAIL");
        }
    };
    let started = wait_line(pid, START_MS);
    let mut help_ms: Option<u64> = None;
    let mut fail: Option<&'static str> = started.err();
    if fail.is_none() {
        // Ctrl+3 as the decoder delivers it (a bare release of `3`), then the line into the console.
        crate::serial_line::line_watch_arm(PLANNED);
        if !push(slot, una_abi::INPUT_EV_KEY_UP, b'3') || !type_line(slot, b"help\n") {
            fail = Some("input-ring-full");
        } else {
            match wait_line(pid, LINE_MS) {
                Ok(ms) => help_ms = Some(ms),
                Err(e) => fail = Some(e),
            }
        }
    }
    crate::serial_line::line_watch_disarm();
    // Ctrl+Q quits with the packed status; the reap is `jobs`'s.
    let mut status: Option<i32> = None;
    if fail.is_none() && type_line(slot, &[0x11]) {
        let t0 = crate::arch::ticks();
        while crate::arch::ticks() < t0 + EXIT_MS {
            match sc::bg_poll(pid, true) {
                BgPoll::Running => crate::arch::sched::yield_now(),
                BgPoll::Exited(s) => {
                    status = Some(s);
                    break;
                }
                BgPoll::Faulted => {
                    fail = Some("faulted");
                    break;
                }
                _ => {
                    fail = Some("gone");
                    break;
                }
            }
        }
        if status.is_none() && fail.is_none() {
            fail = Some("no-exit");
        }
    }
    let killed = if status.is_none() { sc::bg_kill(pid, slot) } else { "exited" };
    let (rows, cmds, kind, code, panes, win_ok, face) = status.map_or((0, 0, 0, 0, 0, false, false), status_parse);
    let verb = midden_core::ring3::verb_of(code).unwrap_or("none");
    serial_println!(
        "[unaapp] pid={} slot={} entry={:#x} start_ms={:?} help_ms={:?} status={:?} panes={} rows={} cmds={} kind={} verb={} window_ok={} face={} kill={}",
        pid, slot, entry, started.ok(), help_ms, status, panes, rows, cmds, midden_core::ring3::kind_name(kind), verb, win_ok, face, killed
    );
    let help_rows = midden_core::help(&midden_core::Facts::ring3(true)).lines().count() as u32;
    let ok = fail.is_none() && status.is_some() && win_ok && panes == PANES && cmds == 1 && kind == 1 && verb == "help" && rows > help_rows;
    if let Some(r) = fail {
        why(r);
    } else if !ok {
        serial_println!(":: UNAAPP: reason=status-mismatch (want panes=3 cmds=1 kind=Say verb=help rows>{}) ::", help_rows);
    }
    verdict(window, panes, rows, if ok { "PASS" } else { "FAIL" });
}
