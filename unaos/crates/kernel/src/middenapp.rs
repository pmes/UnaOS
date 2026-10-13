//! CHARTER: Midden — shared-core midden_core; ring-3 app, LUMENAPP shape
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! MIDDENTWIN (LEDGER SR75, R104) — `tests midden`: the ring-3 shell, driven and read back.
//!
//! `APPS/MIDDEN.ELF` (crates/user-midden) is Midden as a ring-3 program — the Code layout's bottom pane — and
//! it plans every line through `midden_core`, the core this kernel's shell links (`shell.rs`). This fixture is
//! `lumen.rs`'s shape (LUMENAPP + LUMENCRASH): find the image on the volume, run the production validator
//! (`arch::x86_64::elf::validate_elf`, the one `bg` runs) in the ELF-window model, check the windowed app note
//! (`midden_core::app_note_flags`, the reader the bare-name launch uses), SPAWN it the way `bg` does
//! (`spawn_user_image_bg`), wait for its first wire line `:: MIDDEN: start`, then TYPE into it the way the
//! input router delivers keys — `INPUT_EV_KEY_DOWN` events into its ring by owner (`user_input_push_owner`,
//! the APPMENU seam) — `help⏎`, then `ls⏎`, each waited on its `:: MIDDEN: plan=` line, then `exit⏎`, and
//! read the program's EXIT STATUS back through `bg_poll` (the reap `jobs` performs). The status packs what the
//! program did (`midden_core::ring3::status`): scrollback rows, lines planned, the last plan's kind and verb.
//!
//! PASS = the loader takes the image in the ELF model with the windowed note, the program starts, both lines
//! are planned, it exits on `exit`, the last plan is `Host wc` after the MIDDEN2 drives (before them it was `Host
//! ls`: the core routed `ls` to the ring, which serves it — whatever the listing then answers), and the scrollback holds more rows than the core's ring-3 `help`
//! text alone (so `help` drew). The single-producer note: the router is the ring's one producer; this fixture
//! is a second one for the length of the test, which is safe while no operator types into the new window (the
//! QEMU lanes have no keyboard input; on the bench, do not type during `tests midden`).
//!
//! RING3PIPE (rmbp-ledger B562) — THE RUN DRIVE, between `help` and `ls`: [`RUN_LINE`]⏎ — the program's `run` launches
//! `/apps/MIDDEN.ELF -c help` (a second MIDDEN.ELF in its windowless one-shot mode) with SYS_SPAWN_PATH's SPAWN_PIPE;
//! the child writes the core's help to its console and exits, and the shell's `pump` drains the pipe into its
//! scrollback line by line, then says `:: MIDDEN: pipe eof ok=1 …` only when at least one line arrived and none was
//! dropped — the watched prefix [`W_RUN`] (the child prints no `pipe` line: only the launcher reads a pipe). The
//! status then carried cmds=3 (help, run, ls), the last plan `Host ls` — MIDDEN2 adds five lines after it (below).
//!
//! MIDDEN2 (LEDGER SR120) — THE SHELL DRIVES, after `ls` and before `exit`, each typed into the ring and waited on the
//! program's own wire line carrying the expected values ([`DRIVES`]): `cd` (`cd /apps⏎` -> `:: MIDDEN: cd ok=1
//! cwd=/apps ::`), completion (`histo` + Tab -> `complete ok=1 word=histo got=history`, then ⏎ runs it -> `plan=Host
//! verb=history`), history recall (Ctrl+R `cd` -> `recall ok=1 via=search line=cd /apps`, ⏎ runs it -> the cd line
//! again; Up -> `recall ok=1 via=up line=cd /apps`), and a pipeline (Ctrl+U, `echo alpha beta gamma | wc⏎` ->
//! `pipeline stages=2 last=wc rows=1 ok=1` — the core's echo captured and counted by the served `wc`). Tab is pushed as
//! byte 0x09 straight into the ring: on the glass the window system takes a bare Tab (x86 `wc_focus_key`) and the
//! operator presses Ctrl+Tab, which the program binds to the same completion. The status then carries cmds=8 (help,
//! run, ls, cd, history, cd, echo, wc) and the last plan `Host wc`.
//!
//! SHELLABI (rmbp-ledger B565) — TWO MORE DRIVES, before the pipeline one (so the last plan stays `Host wc`): a PROGRAM
//! as a pipeline's SECOND stage — [`PROG_PIPE`]: `echo hi | run /apps/MIDDEN.ELF -c cat⏎`, the shell launches the
//! one-shot child with SPAWN_PIPE and hands it the echo's line through `SYS_PIPE_WRITE` (una-abi 71: each byte a
//! KEY_DOWN on the child's ring, then KEY_EOT), the child's `cat` reads it off its ring and writes it back through the
//! RING3PIPE pipe, and at the child's EOF the shell says `:: MIDDEN: pipeline stages=2 last=/apps/MIDDEN.ELF rows=1
//! ok=1 ::` (the child's own `:: MIDDEN: ` wire is not counted as data); and a KILL — `run /apps/MIDDEN.ELF -c cat⏎`
//! (no input fed, so the child waits on its ring) waited on its `plan=Exec … served=1`, then `kill %1⏎`, which the
//! shell serves with `SYS_KILL` (una-abi 72, parenthood-gated) — `:: MIDDEN: kill ok=1 via=builtin pid=<p> …`. The
//! status then carries cmds=12 (the eight above, plus echo, run, run, kill).
//!
//! WITNESS. `:: MIDDENAPP: image=/apps/MIDDEN.ELF window=<elf|fixed|bad> lines=<n> plan=<verb> run=<ok|step> drives=<name:ok|step,…> -> PASS|FAIL ::`
//! preceded by `[middenapp] …` (pid, slot, timings, the decoded status) and, on FAIL/SKIP, `:: MIDDENAPP:
//! reason=… ::`. No `MIDDEN.ELF` on the volume is a SKIP (the builder stages it; a bare kernel has none).

use crate::arch::x86_64::syscall::{self as sc, BgPoll};

const IMAGE: &str = "/apps/MIDDEN.ELF";
const START: &str = ":: MIDDEN: start";
const PLANNED: &str = ":: MIDDEN: plan=";
/// The program's first line comes after its window and the face read off the volume (LUMEN read 1.7 MiB of
/// faces in seconds on flight 23); one 256 KiB mono face is the shell's.
const START_MS: u64 = 20_000;
const LINE_MS: u64 = 5_000;
const EXIT_MS: u64 = 5_000;
/// RING3PIPE (B562): the Run drive's line (Enter included) and the watched EOF prefix; the child loads off the volume.
const RUN_LINE: &[u8] = b"run /apps/MIDDEN.ELF -c help\n";
const W_RUN: &str = ":: MIDDEN: pipe eof ok=1 ";
const RUN_MS: u64 = 10_000;
/// MIDDEN2 (SR120): the shell drives — `(name, keys typed, the watched wire prefix, ms)`, in order. 0x09 Tab, 0x12
/// Ctrl+R, 0x1F Up (una_abi::KEY_UP), 0x15 Ctrl+U (kill the recalled line before the next line is typed). SHELLABI
/// (B565): `prog-pipe`, `launch` and `kill` — a child loads off the volume, so they wait RUN_MS.
const DRIVES: [(&str, &[u8], &str, u64); 10] = [
    ("cd", b"cd /apps\n", ":: MIDDEN: cd ok=1 cwd=/apps ::", DRIVE_MS),
    ("complete", b"histo\t", ":: MIDDEN: complete ok=1 word=histo got=history ", DRIVE_MS),
    ("complete-run", b"\n", ":: MIDDEN: plan=Host verb=history served=1 ", DRIVE_MS),
    ("search", b"\x12cd", ":: MIDDEN: recall ok=1 via=search line=cd /apps ", DRIVE_MS),
    ("search-run", b"\n", ":: MIDDEN: cd ok=1 cwd=/apps ::", DRIVE_MS),
    ("up", b"\x1f", ":: MIDDEN: recall ok=1 via=up line=cd /apps ", DRIVE_MS),
    ("prog-pipe", PROG_PIPE, ":: MIDDEN: pipeline stages=2 last=/apps/MIDDEN.ELF rows=1 ok=1 ::", RUN_MS),
    ("launch", b"run /apps/MIDDEN.ELF -c cat\n", ":: MIDDEN: plan=Exec verb=run served=1 ", RUN_MS),
    ("kill", b"kill %1\n", ":: MIDDEN: kill ok=1 via=builtin ", RUN_MS),
    ("pipe", b"\x15echo alpha beta gamma | wc\n", ":: MIDDEN: pipeline stages=2 last=wc rows=1 ok=1 ::", DRIVE_MS),
];
const DRIVE_MS: u64 = 5_000;
/// SHELLABI (B565): a program as a pipeline's SECOND stage (Ctrl+U first: the `up` drive left `cd /apps` on the line).
const PROG_PIPE: &[u8] = b"\x15echo hi | run /apps/MIDDEN.ELF -c cat\n";
/// MIDDEN2: what the drives leave in the exit status — twelve planned lines (SHELLABI adds echo, run, run, kill), the
/// last `Host wc`.
const WANT_CMDS: u32 = 12;
const WANT_VERB: &str = "wc";

/// Register `tests midden` once (`tests::ensure_shellux`'s line; `tests-at-boot` runs it there).
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("midden", selftest);
    }
}

fn verdict(window: &str, lines: u32, plan: &str, v: &str) {
    verdict_run(window, lines, plan, "unrun", "unrun", v)
}

/// RING3PIPE: the witness with the Run drive's word; MIDDEN2: and the shell drives' (`name:ok|step,…`).
fn verdict_run(window: &str, lines: u32, plan: &str, run: &str, drives: &str, v: &str) {
    serial_println!(":: MIDDENAPP: image={} window={} lines={} plan={} run={} drives={} -> {} ::", IMAGE, window, lines, plan, run, drives, v);
}

fn why(r: &str) {
    serial_println!(":: MIDDENAPP: reason={} ::", r);
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

/// Type `s` into the ring of the process in `slot`, one KEY_DOWN per byte (`\n` is Enter, as the router sends it).
/// SHELLABI (B565): PACED — a byte waits (yielding, up to TYPE_MS) while TYPE_DEPTH events sit unread, so a line longer
/// than the 32-event ring arrives whole instead of being cut at the ring's end; a stalled program still fails the drive.
fn type_line(slot: u64, s: &[u8]) -> bool {
    let mut ok = true;
    for &b in s {
        let t0 = crate::arch::ticks();
        while sc::user_input_depth(slot + 1) >= TYPE_DEPTH && crate::arch::ticks() < t0 + TYPE_MS {
            crate::arch::sched::yield_now();
        }
        ok &= sc::user_input_push_owner(slot + 1, una_abi::input_ev_pack(una_abi::INPUT_EV_KEY_DOWN, b as u64));
    }
    ok
}

/// SHELLABI: `type_line`'s pacing — the unread depth a byte waits under, and how long it waits.
const TYPE_DEPTH: u32 = 16;
const TYPE_MS: u64 = 2_000;

pub fn selftest() {
    let Ok(fs) = crate::fs::rootdisk::program_source() else {
        why("no-program-volume");
        return verdict("bad", 0, "none", "SKIP");
    };
    let cap = sc::user_image_cap();
    let img = match fs.find_app("MIDDEN.ELF") {
        Ok(de) if de.size != 0 && de.size as usize <= cap => {
            let mut b = alloc::vec::Vec::new();
            if fs.read_file(&de, &mut b, cap).is_err() {
                why("read-failed");
                return verdict("bad", 0, "none", "FAIL");
            }
            b
        }
        Ok(_) => {
            why("size-out-of-range");
            return verdict("bad", 0, "none", "FAIL");
        }
        Err(_) => {
            why("MIDDEN.ELF-not-on-the-volume");
            return verdict("bad", 0, "none", "SKIP");
        }
    };
    let plan = match crate::arch::x86_64::elf::validate_elf(&img, sc::user_window_size()) {
        Ok(p) => p,
        Err(e) => {
            serial_println!(":: MIDDENAPP: reason=loader-refused ({}) ::", e);
            return verdict("bad", 0, "none", "FAIL");
        }
    };
    let window = if plan.model_elf { "elf" } else { "fixed" };
    let note = midden_core::app_note_flags(&img);
    if !plan.model_elf {
        why("not-the-elf-model");
        return verdict(window, 0, "none", "FAIL");
    }
    if note.map_or(true, |f| f & una_abi::APP_FLAG_WINDOWED == 0) {
        why("no-windowed-app-note");
        return verdict(window, 0, "none", "FAIL");
    }

    // The spawn `bg` makes (LUMENCRASH's step): APPRES sight first, then the image, then the launch name.
    crate::fs::appres::sight(IMAGE);
    crate::serial_line::line_watch_arm(START);
    let (pid, slot, entry) = match sc::spawn_user_image_bg(&img) {
        Ok(t) => {
            crate::video::wm::app_name_arm_launch(crate::video::wm::owner_of_launch(t.1), IMAGE);
            t
        }
        Err(e) => {
            crate::serial_line::line_watch_disarm();
            serial_println!(":: MIDDENAPP: reason=spawn-refused ({}) ::", e);
            return verdict(window, 0, "none", "FAIL");
        }
    };
    let started = wait_line(pid, START_MS);
    let mut typed = [None::<u64>; 2];
    let mut fail: Option<&'static str> = started.err();
    let mut run: &'static str = "unrun";
    let mut run_ms: Option<u64> = None;
    if fail.is_none() {
        for (i, l) in [&b"help\n"[..], &b"ls\n"[..]].iter().enumerate() {
            if i == 1 {
                // RING3PIPE (B562): the Run drive, between `help` and `ls` — the child's output through the pipe.
                crate::serial_line::line_watch_arm(W_RUN);
                if !type_line(slot, RUN_LINE) {
                    run = "input-ring-full";
                } else {
                    match wait_line(pid, RUN_MS) {
                        Ok(ms) => {
                            run = "ok";
                            run_ms = Some(ms);
                        }
                        Err(e) => run = e,
                    }
                }
            }
            crate::serial_line::line_watch_arm(PLANNED);
            if !type_line(slot, l) {
                fail = Some("input-ring-full");
                break;
            }
            match wait_line(pid, LINE_MS) {
                Ok(ms) => typed[i] = Some(ms),
                Err(e) => {
                    fail = Some(e);
                    break;
                }
            }
        }
    }
    // MIDDEN2 (SR120): the shell drives — cd, completion, history recall (search and Up), a pipeline.
    let mut drives = alloc::string::String::new();
    let mut drives_ok = fail.is_none();
    if fail.is_none() {
        for &(name, keys, watch, ms) in DRIVES.iter() {
            crate::serial_line::line_watch_arm(watch);
            let r = if type_line(slot, keys) { wait_line(pid, ms) } else { Err("input-ring-full") };
            if !drives.is_empty() {
                drives.push(',');
            }
            drives.push_str(name);
            drives.push(':');
            match r {
                Ok(ms) => {
                    drives.push_str("ok");
                    serial_println!("[middenapp] drive={} ms={} watched={}", name, ms, watch.trim_end());
                }
                Err(e) => {
                    drives_ok = false;
                    drives.push_str(e);
                    serial_println!("[middenapp] drive={} -> {} (watched `{}`)", name, e, watch.trim_end());
                    if matches!(e, "faulted" | "exited-early" | "gone") {
                        fail = Some(e);
                        break;
                    }
                }
            }
        }
    } else {
        drives.push_str("unrun");
    }
    crate::serial_line::line_watch_disarm();
    // `exit` closes the window and exits with the packed status; the reap is `jobs`'s.
    let mut status: Option<i32> = None;
    if fail.is_none() && type_line(slot, b"exit\n") {
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
    let (rows, cmds, kind, code, win_ok, face) = status.map_or((0, 0, 0, 0, false, false), midden_core::ring3::status_parse);
    let verb = midden_core::ring3::verb_of(code).unwrap_or("none");
    serial_println!(
        "[middenapp] pid={} slot={} entry={:#x} start_ms={:?} help_ms={:?} ls_ms={:?} status={:?} rows={} cmds={} kind={} verb={} window_ok={} face={} kill={}",
        pid, slot, entry, started.ok(), typed[0], typed[1], status, rows, cmds, midden_core::ring3::kind_name(kind), verb, win_ok, face, killed
    );
    serial_println!("[middenapp] run={} run_ms={:?} line={}", run, run_ms, core::str::from_utf8(RUN_LINE).unwrap_or("?").trim_end());
    let help_rows = midden_core::help(&midden_core::Facts::ring3(true)).lines().count() as u32;
    let base = fail.is_none() && status.is_some() && win_ok && cmds == WANT_CMDS && kind == 2 && verb == WANT_VERB && rows > help_rows;
    let ok = base && run == "ok" && drives_ok;
    if let Some(r) = fail {
        why(r);
    } else if !drives_ok {
        serial_println!(":: MIDDENAPP: reason=drive-mismatch (drives={}) ::", drives);
    } else if !base {
        serial_println!(":: MIDDENAPP: reason=status-mismatch (want cmds={} kind=Host verb={} rows>{}) ::", WANT_CMDS, WANT_VERB, help_rows);
    } else if !ok {
        serial_println!(":: MIDDENAPP: reason=run-mismatch (run={}: want `{}` from `run`'s pipe) ::", run, W_RUN.trim_end());
    }
    verdict_run(window, rows, verb, run, &drives, if ok { "PASS" } else { "FAIL" });
}
