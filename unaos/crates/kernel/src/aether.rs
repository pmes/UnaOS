//! CHARTER: Aether — shared-core html_core+css_core+font_core; ring-3 app, LUMENAPP shape
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AETHERMETAL M1 (LEDGER SR66) — `tests aether`: the witness that Aether, the browser, is a program on the
//! image and paints a page. The kernel builds and fulfils nothing of the browser: APPS/AETHER.ELF (crates/
//! user-aether) parses, cascades, lays out and paints in ring 3 on the shared cores. This file is lumen.rs's
//! LUMENAPP + LUMENCRASH shape (B323, B326) for that image:
//!
//! 1. STATIC. `/apps/AETHER.ELF` is on the program volume; the production validator (`arch::x86_64::elf::
//!    validate_elf`, the one `bg` runs) accepts it in the ELF-window model; its `.note.unaos.app` names "UnaOS",
//!    type 1, flags bit0 = windowed (`midden_core::app_note_flags`, the shell's own reader).
//! 2. SPAWN. The image runs as `bg` runs it (`spawn_user_image_bg`, the APPRES sight first and the launch name
//!    armed, LUMENFAST's order) and the fixture waits up to 20 s for the program's report line, then kills the
//!    job so it leaves no window behind.
//! 3. REPORT. The program's numbers come back over LUMEN's channel — its own wire line at the ring-3 console
//!    seam: `serial_line::line_watch_*` latches the moment `:: AETHER: page` is written, and the seam's verdict-
//!    tail recorder (`serial_line::tail_arm` / `tail_take`, QUIETBOOT3) holds that line's text after its `-> `,
//!    from which `painted=` and `lines=` are read. The recorder is re-armed before this file's own verdict, so a
//!    `tests` run still reads THIS fixture's tail for the glass.
//!
//! WITNESS. `:: AETHERAPP: image=/apps/AETHER.ELF window=<elf|fixed|bad> painted=<n> lines=<n> -> PASS|FAIL|SKIP ::`
//! (preceded on SKIP/FAIL by `:: AETHERAPP: reason=… ::`), and `[aetherapp] …` with the loader's plan and the
//! spawn's timings. PASS = the image validated in the ELF model with a windowed note, the program reported PASS
//! (it read the page) with `lines > 0` and `painted > 0`.
//!
//! The knob is `UNAOS_AETHER=1` → feature `aether` (it implies `selfdiag`: SYS_PATH_READ is the file surface the
//! program reads the page and the faces through). Registered from `tests::ensure_shellux`, so the QEMU lanes
//! (`tests-at-boot`) run it at boot and the desktop runs it as `tests aether`.

const IMAGE: &str = "/apps/AETHER.ELF";
#[cfg(target_arch = "x86_64")]
const REPORT: &str = ":: AETHER: page";

/// Register `tests aether` exactly once.
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("aether", selftest);
    }
}

/// aarch64: no AETHER.ELF image is built for that loader yet (owed with Lumen's).
#[cfg(not(target_arch = "x86_64"))]
pub fn selftest() {
    serial_println!(":: AETHERAPP: reason=aarch64-image-owed ::");
    serial_println!(":: AETHERAPP: image={} window=bad painted=0 lines=0 -> SKIP reason=aarch64-image-owed ::", IMAGE);
}

#[cfg(target_arch = "x86_64")]
pub fn selftest() {
    let verdict = |window: &str, painted: u64, lines: u64, v: &str| {
        crate::serial_line::tail_arm(); // the glass reads THIS line's tail, not the program's
        serial_println!(":: AETHERAPP: image={} window={} painted={} lines={} -> {} ::", IMAGE, window, painted, lines, v);
    };
    let why = |r: &str| serial_println!(":: AETHERAPP: reason={} ::", r);
    let (img, window) = match check(&why) {
        Ok(t) => t,
        Err(v) => return verdict("bad", 0, 0, v),
    };
    if window != "elf" {
        return verdict(window, 0, 0, "FAIL");
    }
    const WAIT_MS: u64 = 20_000;
    let t_sight = crate::arch::ms();
    crate::fs::appres::sight(IMAGE);
    let sight_ms = crate::arch::ms().saturating_sub(t_sight);
    crate::serial_line::line_watch_arm(REPORT);
    crate::serial_line::tail_arm();
    let t_spawn = crate::arch::ms();
    let (pid, slot, entry) = match crate::arch::syscall::spawn_user_image_bg(&img) {
        Ok(t) => {
            crate::video::wm::app_name_arm_launch(crate::video::wm::owner_of_launch(t.1), IMAGE);
            t
        }
        Err(e) => {
            crate::serial_line::line_watch_disarm();
            serial_println!(":: AETHERAPP: reason=spawn-refused ({}) ::", e);
            return verdict(window, 0, 0, "FAIL");
        }
    };
    let spawn_ms = crate::arch::ms().saturating_sub(t_spawn);
    let t0 = crate::arch::ticks();
    let mut hit = false;
    while crate::arch::ticks() < t0 + WAIT_MS {
        if crate::serial_line::line_watch_hit() {
            hit = true;
            break;
        }
        crate::arch::sched::yield_now();
    }
    let took = crate::arch::ticks().saturating_sub(t0);
    crate::serial_line::line_watch_disarm();
    // The watch latches at the seam's entry, before the line is emitted: let the emit (and its tail) land.
    let t1 = crate::arch::ticks();
    while hit && crate::arch::ticks() < t1 + 50 {
        crate::arch::sched::yield_now();
    }
    let tail = crate::serial_line::tail_take();
    let killed = crate::arch::syscall::bg_kill(pid, slot);
    serial_println!(
        "[aetherapp] pid={} slot={} entry={:#x} sight_ms={} spawn_ms={} report_ms={} wait_ms={} kill={} tail={:?}",
        pid, slot, entry, sight_ms, spawn_ms, took, WAIT_MS, killed, tail.as_deref()
    );
    if !hit {
        why("no-report-line");
        return verdict(window, 0, 0, "FAIL");
    }
    let t = tail.unwrap_or_default();
    let field = |k: &str| -> Option<u64> {
        let at = t.find(k)? + k.len();
        let d: alloc::string::String = t[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
        d.parse().ok()
    };
    let (Some(painted), Some(lines)) = (field("painted="), field("lines=")) else {
        why("report-unread");
        return verdict(window, 0, 0, "FAIL");
    };
    let ok = t.starts_with("PASS") && painted > 0 && lines > 0;
    if !ok {
        why(if !t.starts_with("PASS") { "program-said-fail" } else { "nothing-painted" });
    }
    verdict(window, painted, lines, if ok { "PASS" } else { "FAIL" });
}

/// The static half: the image off the program volume, the loader's plan, the app note. `Ok((image, window))`;
/// `Err` is the verdict word — SKIP when there is no program volume or the image is not staged on it (Lumen's
/// rule), FAIL when it is there and cannot be read or loaded.
#[cfg(target_arch = "x86_64")]
fn check(why: &dyn Fn(&str)) -> Result<(alloc::vec::Vec<u8>, &'static str), &'static str> {
    let Ok(fs) = crate::fs::rootdisk::program_source() else {
        why("no-program-volume");
        return Err("SKIP");
    };
    let cap = crate::arch::syscall::user_image_cap();
    let img = match fs.find_app("AETHER.ELF") {
        Ok(de) if de.size != 0 && de.size as usize <= cap => {
            let mut b = alloc::vec::Vec::new();
            if fs.read_file(&de, &mut b, cap).is_err() {
                why("read-failed");
                return Err("FAIL");
            }
            b
        }
        Ok(_) => {
            why("size-out-of-range");
            return Err("FAIL");
        }
        Err(_) => {
            why("AETHER.ELF-not-on-the-volume");
            return Err("SKIP");
        }
    };
    let plan = match crate::arch::x86_64::elf::validate_elf(&img, crate::arch::syscall::user_window_size()) {
        Ok(p) => p,
        Err(e) => {
            serial_println!(":: AETHERAPP: reason=loader-refused ({}) ::", e);
            return Err("FAIL");
        }
    };
    let window = if plan.model_elf { "elf" } else { "fixed" };
    let note = midden_core::app_note_flags(&img);
    serial_println!("[aetherapp] bytes={} entry={:#x} segs={} stack={} note_flags={:?}", img.len(), plan.entry, plan.nsegs, plan.stack, note);
    if !plan.model_elf {
        why("not-the-elf-model");
    } else if note.map_or(true, |f| f & una_abi::APP_FLAG_WINDOWED == 0) {
        why("no-windowed-app-note");
        return Ok((img, "bad"));
    }
    Ok((img, window))
}
