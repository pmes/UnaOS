//! CHARTER: Stria — shared-core demux_core+vp8/vp9/av1/h264_core+audio_core; ring-3 app, LUMENAPP shape
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! STRIAMETAL (LEDGER SR139) — `tests stria`: the witness that Stria, UnaOS's A/V handler (CODEX §2), is a program on
//! the image and plays a video. The kernel decodes nothing for it: APPS/STRIA.ELF (crates/user-stria) demuxes,
//! decodes and presents in ring 3 over the shared cores, and sounds through una-abi STRIAMETAL's SYS_AUDIO_* (rmbp-
//! ledger B618). This file is aether.rs's shape (AETHERMETAL SR66, ARMAETHER A110) for that image:
//!
//! 1. STATIC. `/apps/STRIA.ELF` is on the program volume; the production validator (`arch::syscall::launch_plan`,
//!    the arch's loader check through the ARMTESTS facade A109) accepts it in the ELF-window model; its
//!    `.note.unaos.app` names "UnaOS", type 1, flags bit0 = windowed (`midden_core::app_note_flags`).
//! 2. THE CLIP. `system/test-f/TEST.WEBM` — builder/testf.list's row (VOLUMES B366: Chromium's bear-320x240.webm,
//!    VP8 + Vorbis, BSD-3-Clause, sha256-pinned), found where `tests play` finds its samples (`volumes::testf_find`:
//!    `/system/test-f`, then the card twin `/boot/system/test-f`). No clip = SKIP by name, never a pass.
//! 3. SPAWN. The image runs as `bg` runs it (APPRES sight, `spawn_user_image_bg_argv` with argv
//!    `[STRIA.ELF, <clip>]`, the launch name armed) and the fixture waits up to WAIT_MS for the program's END line
//!    (`:: STRIA: end -> …`, latched at the console seam by `serial_line::line_watch_*`), reads its numbers from the
//!    seam's verdict-tail recorder (`tail_arm` / `tail_take`, QUIETBOOT3), then kills the job so no window is left.
//!    The program's own lines (`:: STRIA: open …`, `:: STRIA: audio …`, `:: STRIA: frame n=…` every 30th) and the
//!    kernel's `[audio] open|arm|close` lines are on the wire beside it.
//!
//! WITNESS. `:: STRIAAPP: image=/apps/STRIA.ELF window=<elf|fixed|bad> frames=<n> audio=<ok|none> drift_ms=<n> -> PASS|FAIL|SKIP ::`
//! (preceded on SKIP/FAIL by `:: STRIAAPP: reason=… ::`) and `[striaapp] …` with the loader's plan, the clip and the
//! timings. PASS = the image validated in the ELF model with a windowed note and the program ended PASS with
//! frames > 0. `audio=` is the program's word: `ok` when the sound's stream clocked the pictures (x86 with
//! UNAOS_HDATONE and an HDA the codec walk drove), `none` otherwise — aarch64 refuses the open by name
//! (`refused=no-aarch64-sink`) and the pictures run on the kernel's clock — so it reports and never decides.
//!
//! The knob is `UNAOS_STRIA=1` → feature `stria` (it implies `selfdiag`: SYS_PATH_READ is the file surface the program
//! reads the clip through). Registered from `tests::ensure_shellux`, so the QEMU lanes (`tests-at-boot`) run it at
//! boot and the desktop runs it as `tests stria`; both arches, through the launch facade (the launch handle is the
//! 0-based slot on x86 and the ASID on aarch64; printed under `slot=` on both).

use crate::arch::syscall as sc;

const IMAGE: &str = "/apps/STRIA.ELF";
const CLIP: &str = "TEST.WEBM";
const REPORT: &str = ":: STRIA: end";
/// The clip is 2.7 s; the wait covers the stream's bring-up, a slow decode under QEMU's TCG and a window retry.
const WAIT_MS: u64 = 45_000;

/// Register `tests stria` exactly once.
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("stria", selftest);
    }
}

pub fn selftest() {
    let verdict = |window: &str, frames: u64, audio: &str, drift: u64, v: &str| {
        crate::serial_line::tail_arm(); // the glass reads THIS line's tail, not the program's
        serial_println!(":: STRIAAPP: image={} window={} frames={} audio={} drift_ms={} -> {} ::", IMAGE, window, frames, audio, drift, v);
    };
    let why = |r: &str| serial_println!(":: STRIAAPP: reason={} ::", r);
    let (img, window) = match check(&why) {
        Ok(t) => t,
        Err(v) => return verdict("bad", 0, "none", 0, v),
    };
    if window != "elf" {
        return verdict(window, 0, "none", 0, "FAIL");
    }
    let mt = crate::shell::vfs_mount_table();
    let Some(clip) = crate::fs::volumes::testf_find(&mt, CLIP) else {
        why("no-clip (system/test-f/TEST.WEBM is not staged: builder/testf.list's row, or the arm card's copy)");
        return verdict(window, 0, "none", 0, "SKIP");
    };
    let t_sight = crate::arch::ms();
    crate::fs::appres::sight(IMAGE);
    let sight_ms = crate::arch::ms().saturating_sub(t_sight);
    crate::serial_line::line_watch_arm(REPORT);
    crate::serial_line::tail_arm();
    let t_spawn = crate::arch::ms();
    let (pid, slot, entry) = match sc::spawn_user_image_bg_argv(&img, &["STRIA.ELF", clip.as_str()]) {
        Ok(t) => {
            crate::video::wm::app_name_arm_launch(sc::owner_of_launch(t.1), IMAGE);
            t
        }
        Err(e) => {
            crate::serial_line::line_watch_disarm();
            let _ = crate::serial_line::tail_take();
            serial_println!(":: STRIAAPP: reason=spawn-refused ({}) ::", e);
            return verdict(window, 0, "none", 0, "FAIL");
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
    let killed = sc::bg_kill(pid, slot);
    serial_println!(
        "[striaapp] pid={} slot={} entry={:#x} clip={} sight_ms={} spawn_ms={} report_ms={} wait_ms={} kill={} tail={:?}",
        pid, slot, entry, clip, sight_ms, spawn_ms, took, WAIT_MS, killed, tail.as_deref()
    );
    if !hit {
        why("no-end-line");
        return verdict(window, 0, "none", 0, "FAIL");
    }
    let t = tail.unwrap_or_default();
    let num = |k: &str| -> Option<u64> {
        let at = t.find(k)? + k.len();
        let d: alloc::string::String = t[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
        d.parse().ok()
    };
    let audio = if t.contains("audio=ok") { "ok" } else { "none" };
    let (Some(frames), Some(drift)) = (num("frames="), num("drift_ms=")) else {
        why("end-line-unread");
        return verdict(window, 0, audio, 0, "FAIL");
    };
    let ok = t.starts_with("PASS") && frames > 0;
    if !ok {
        why(if !t.starts_with("PASS") { "program-said-fail" } else { "no-frame" });
    }
    verdict(window, frames, audio, drift, if ok { "PASS" } else { "FAIL" });
}

/// The static half: the image off the program volume, the loader's plan, the app note. `Ok((image, window))`;
/// `Err` is the verdict word — SKIP when there is no program volume or the image is not staged on it (Lumen's
/// rule), FAIL when it is there and cannot be read or loaded.
fn check(why: &dyn Fn(&str)) -> Result<(alloc::vec::Vec<u8>, &'static str), &'static str> {
    let Ok(fs) = crate::fs::rootdisk::program_source() else {
        why("no-program-volume");
        return Err("SKIP");
    };
    let cap = sc::user_image_cap();
    let img = match fs.find_app("STRIA.ELF") {
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
            why("STRIA.ELF-not-on-the-volume");
            return Err("SKIP");
        }
    };
    let plan = match sc::launch_plan(&img) {
        Ok(p) => p,
        Err(e) => {
            serial_println!(":: STRIAAPP: reason=loader-refused ({}) ::", e);
            return Err("FAIL");
        }
    };
    let window = if plan.model_elf { "elf" } else { "fixed" };
    let note = midden_core::app_note_flags(&img);
    serial_println!("[striaapp] bytes={} entry={:#x} segs={} stack={} note_flags={:?}", img.len(), plan.entry, plan.nsegs, plan.stack, note);
    if !plan.model_elf {
        why("not-the-elf-model");
    } else if note.map_or(true, |f| f & una_abi::APP_FLAG_WINDOWED == 0) {
        why("no-windowed-app-note");
        return Ok((img, "bad"));
    }
    Ok((img, window))
}
