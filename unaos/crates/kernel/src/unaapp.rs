//! CHARTER: Tabula — shared-core edit_core (the centre pane) and Midden — shared-core midden_core (the console); the Una vessel's ring-3 face, MIDDENTWIN shape
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
//! UNAMETAL2 (LEDGER SR98) — SAVE and HIGHLIGHT. On a UnaFS root the fixture first lays [`FIX_PATH`] = [`FIX_SRC`]
//! with KERNEL authority (no owner row: a public object the program may write as `anon` or as the session user),
//! spawns the program with argv `[UNA.ELF, FIX_PATH]` (it opens argv[1]'s file, a Rust source, so syntax_core lexes
//! it), waits on `:: UNA: open path=`, types [`FIX_EDIT`] into the editor (focused at start), Ctrl+S (0x13) — the
//! default kernel's `SYS_PATH_WRITE`, the caller's principal under the write ACL — and waits on `:: UNA: save
//! path=`; then legs 1–3. After the exit the file is read back and must be [`FIX_SAVED`] byte-exact. The status
//! carries SAVED (the last Ctrl+S answered rc=0) and HIGHLIGHT (the document's syntax_core span count, cap 15; the
//! fixture text lexes to 7). An informational ACL leg asks `path_write_as` to write `/apps/UNAPROBE.TXT` as `anon`
//! (ROOTACL's `system` tree): `acl=refused` expected; it is not in PASS. On a FAT root the save leg is `saved=skip`
//! (a FAT volume is written by its mounting principal only) and the program starts with no argument.
//!
//! PASS = the loader takes the image in the ELF model with the windowed note, the program starts, reports three
//! panes, its window, ONE console line planned whose kind is Say and verb `help`, and more console rows than the
//! core's ring-3 `help` text alone (so the help drew in the console pane); and, on a UnaFS root, saved=1, the bytes
//! on the volume are the edited text, and highlight > 0. The single-producer note is middenapp.rs's: do not type
//! into the new window during `tests una` on the bench.
//!
//! WITNESS. `:: UNAAPP: image=/apps/UNA.ELF window=<elf|fixed|bad> panes=<n> lines=<n> saved=<1|0|skip>
//! highlight=<n> -> PASS|FAIL|SKIP ::` preceded by `[unaapp] …` (pid, slot, timings, the decoded status, the bytes
//! check, the ACL leg) and, on FAIL/SKIP, `:: UNAAPP: reason=… ::`. No `UNA.ELF` on the volume is a SKIP (the
//! builder stages it; a bare kernel has none).

use crate::arch::x86_64::syscall::{self as sc, BgPoll};

const IMAGE: &str = "/apps/UNA.ELF";
const START: &str = ":: UNA: start";
const PLANNED: &str = ":: UNA: plan=";
/// UNAMETAL2: the editor's open and save lines.
const OPENED: &str = ":: UNA: open path=";
const SAVED: &str = ":: UNA: save path=";
/// UNAMETAL2: the save leg's scratch file (UnaFS root only), its text, the keys typed into it, the bytes expected back.
const FIX_DIR: &str = "/var/tmp/una";
const FIX_PATH: &str = "/var/tmp/una/fix.rs";
const FIX_SRC: &[u8] = b"fn main() {}\n";
const FIX_EDIT: &[u8] = b"// una\n";
const FIX_SAVED: &[u8] = b"// una\nfn main() {}\n";
/// UNAMETAL2: the ACL leg's probe — a name under ROOTACL's `system` tree `/apps`, which `anon` may not create.
const ACL_PROBE: &str = "/apps/UNAPROBE.TXT";
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

/// user-una's `status` read back (UNAMETAL2 layout): `(rows, cmds, kind, verb, panes, window, face, saved, spans)` —
/// rows `[9:0]`, cmds `[11:10]`, kind `[13:12]`, verb code `[21:14]`, panes `[23:22]`, window `[24]`, face `[25]`,
/// saved `[26]`, highlight spans `[30:27]` (cap 15). Kept in step with crates/user-una/src/main.rs `status` by hand
/// (the program links no kernel crate, the kernel no program).
const fn status_parse(s: i32) -> (u32, u32, u32, u8, u32, bool, bool, bool, u32) {
    let s = s as u32;
    (
        s & 0x3FF,
        (s >> 10) & 3,
        (s >> 12) & 3,
        ((s >> 14) & 0xFF) as u8,
        (s >> 22) & 3,
        (s >> 24) & 1 != 0,
        (s >> 25) & 1 != 0,
        (s >> 26) & 1 != 0,
        (s >> 27) & 0xF,
    )
}

fn verdict(window: &str, panes: u32, lines: u32, saved: &str, spans: u32, v: &str) {
    serial_println!(":: UNAAPP: image={} window={} panes={} lines={} saved={} highlight={} -> {} ::", IMAGE, window, panes, lines, saved, spans, v);
}

/// UNAMETAL2: lay [`FIX_PATH`] = [`FIX_SRC`] with KERNEL authority (missing directories made, an old file replaced).
/// Kernel-created native objects carry no owner row, so the program may write the file whoever it runs as.
fn lay_fixture(mt: &crate::fs::vfs::MountTable) -> Result<(), &'static str> {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as K};
    for d in ["/var", "/var/tmp", FIX_DIR] {
        match mt.stat(d) {
            Ok(st) if st.kind == NodeKind::Dir => {}
            Ok(_) => return Err("fixture-dir-is-a-file"),
            Err(_) => {
                mt.create(d, NodeKind::Dir, K).map_err(|_| "fixture-mkdir")?;
            }
        }
    }
    if mt.stat(FIX_PATH).is_ok() {
        mt.unlink(FIX_PATH, K).map_err(|_| "fixture-unlink")?;
    }
    mt.create(FIX_PATH, NodeKind::File, K).map_err(|_| "fixture-create")?;
    match mt.write(FIX_PATH, 0, FIX_SRC, K) {
        Ok(n) if n == FIX_SRC.len() => Ok(()),
        _ => Err("fixture-write"),
    }
}

/// UNAMETAL2: the ACL leg — may `anon` create a file under `/apps` through the default kernel's path write?
/// `refused` is the expected answer; an `open` result is unlinked again with KERNEL authority.
fn acl_leg(mt: &crate::fs::vfs::MountTable) -> &'static str {
    let r = sc::path_write_as(ACL_PROBE, una_abi::PATH_W_TRUNC, 0, b"x", "anon");
    if r == una_abi::EACCES {
        "refused"
    } else if r >= 0 {
        let _ = mt.unlink(ACL_PROBE, crate::fs::vfs::KERNEL_PRINCIPAL);
        "open"
    } else {
        "error"
    }
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
        return verdict("bad", 0, 0, "skip", 0, "SKIP");
    };
    let cap = sc::user_image_cap();
    let img = match fs.find_app("UNA.ELF") {
        Ok(de) if de.size != 0 && de.size as usize <= cap => {
            let mut b = alloc::vec::Vec::new();
            if fs.read_file(&de, &mut b, cap).is_err() {
                why("read-failed");
                return verdict("bad", 0, 0, "skip", 0, "FAIL");
            }
            b
        }
        Ok(_) => {
            why("size-out-of-range");
            return verdict("bad", 0, 0, "skip", 0, "FAIL");
        }
        Err(_) => {
            why("UNA.ELF-not-on-the-volume");
            return verdict("bad", 0, 0, "skip", 0, "SKIP");
        }
    };
    let plan = match crate::arch::x86_64::elf::validate_elf(&img, sc::user_window_size()) {
        Ok(p) => p,
        Err(e) => {
            serial_println!(":: UNAAPP: reason=loader-refused ({}) ::", e);
            return verdict("bad", 0, 0, "skip", 0, "FAIL");
        }
    };
    let window = if plan.model_elf { "elf" } else { "fixed" };
    let note = midden_core::app_note_flags(&img);
    if !plan.model_elf {
        why("not-the-elf-model");
        return verdict(window, 0, 0, "skip", 0, "FAIL");
    }
    if note.map_or(true, |f| f & una_abi::APP_FLAG_WINDOWED == 0) {
        why("no-windowed-app-note");
        return verdict(window, 0, 0, "skip", 0, "FAIL");
    }

    // UNAMETAL2: the save leg's file, on a UnaFS root only (a FAT root is written by its mounting principal).
    let mt = crate::shell::vfs_mount_table();
    let native = matches!(mt.volume_name("/"), Ok(v) if v == "native");
    let laid: Result<(), &'static str> = if native { lay_fixture(&mt) } else { Err("fat-root") };
    if let (true, Err(r)) = (native, laid) {
        why(r);
        return verdict(window, 0, 0, "0", 0, "FAIL");
    }
    let acl = if native { acl_leg(&mt) } else { "n/a" };
    let argv: &[&str] = if laid.is_ok() { &["UNA.ELF", FIX_PATH] } else { &["UNA.ELF"] };

    // The spawn `bg` makes: APPRES sight first, then the image, then the launch name.
    crate::fs::appres::sight(IMAGE);
    crate::serial_line::line_watch_arm(if laid.is_ok() { OPENED } else { START });
    let (pid, slot, entry) = match sc::spawn_user_image_bg_argv(&img, argv) {
        Ok(t) => {
            crate::video::wm::app_name_arm_launch(crate::video::wm::owner_of_launch(t.1), IMAGE);
            t
        }
        Err(e) => {
            crate::serial_line::line_watch_disarm();
            serial_println!(":: UNAAPP: reason=spawn-refused ({}) ::", e);
            return verdict(window, 0, 0, "skip", 0, "FAIL");
        }
    };
    let started = wait_line(pid, START_MS);
    let mut help_ms: Option<u64> = None;
    let mut fail: Option<&'static str> = started.err();
    let mut save_ms: Option<u64> = None;
    if fail.is_none() && laid.is_ok() {
        // UNAMETAL2: the edit into the focused editor, then Ctrl+S — waited on the program's save line.
        crate::serial_line::line_watch_arm(SAVED);
        if !type_line(slot, FIX_EDIT) || !type_line(slot, &[0x13]) {
            fail = Some("input-ring-full");
        } else {
            match wait_line(pid, LINE_MS) {
                Ok(ms) => save_ms = Some(ms),
                Err(e) => fail = Some(e),
            }
        }
    }
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
    let (rows, cmds, kind, code, panes, win_ok, face, saved, spans) = status.map_or((0, 0, 0, 0, 0, false, false, false, 0), status_parse);
    let verb = midden_core::ring3::verb_of(code).unwrap_or("none");
    // UNAMETAL2: what the save left on the volume.
    let bytes_ok = laid.is_ok() && matches!(mt.read(FIX_PATH, 0, FIX_SAVED.len() + 64), Ok(b) if b.as_slice() == FIX_SAVED);
    serial_println!(
        "[unaapp] pid={} slot={} entry={:#x} start_ms={:?} save_ms={:?} help_ms={:?} status={:?} panes={} rows={} cmds={} kind={} verb={} window_ok={} face={} saved={} spans={} bytes={} acl={} kill={}",
        pid, slot, entry, started.ok(), save_ms, help_ms, status, panes, rows, cmds, midden_core::ring3::kind_name(kind), verb, win_ok, face, saved, spans,
        if laid.is_err() { "n/a" } else if bytes_ok { "ok" } else { "bad" }, acl, killed
    );
    let help_rows = midden_core::help(&midden_core::Facts::ring3(true)).lines().count() as u32;
    let base = fail.is_none() && status.is_some() && win_ok && panes == PANES && cmds == 1 && kind == 1 && verb == "help" && rows > help_rows;
    let save_ok = laid.is_err() || (saved && bytes_ok && spans > 0);
    let ok = base && save_ok;
    if let Some(r) = fail {
        why(r);
    } else if !base {
        serial_println!(":: UNAAPP: reason=status-mismatch (want panes=3 cmds=1 kind=Say verb=help rows>{}) ::", help_rows);
    } else if !save_ok {
        serial_println!(":: UNAAPP: reason=save-mismatch (want saved=1 bytes=ok highlight>0 at {}) ::", FIX_PATH);
    }
    let saved_word = if laid.is_err() { "skip" } else if saved { "1" } else { "0" };
    verdict(window, panes, rows, saved_word, spans, if ok { "PASS" } else { "FAIL" });
}
