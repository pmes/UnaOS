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
//! 4. DRIVE (AETHERMETAL M3, LEDGER SR96). Before the kill, the fixture plays the operator into the program's
//!    ring by owner (`user_input_push_owner`, unaapp.rs's channel), one step per wire line, each watched at the
//!    seam (`line_watch_arm`) and read from the verdict tail: Tab (`:: AETHER: focus -> PASS href=… at=x0,y,x1`
//!    — the first link's first text box, window px), the pointer placed on that box by one MOUSE_ABS in window px
//!    (AETHERPTR B543: the ring's pointer contract; `:: AETHER: hover -> PASS href=…` → `hover=1`), a left-button drag
//!    across it (`:: AETHER: select -> PASS chars=<n>` → `select=<n>`), Ctrl+C (`:: AETHER: clip … set=<rc>`,
//!    reported, not judged: the clipboard answers only the FOCUSED program), and a click on the link
//!    (`:: AETHER: nav -> PASS how=click path=…` → `nav=1`: welcome.html's first link is keys.html beside it).
//!
//! WITNESS (M3). `:: AETHERAPP: image=/apps/AETHER.ELF window=<…> painted=<n> lines=<n> hover=<0|1> nav=<0|1>
//! select=<n> -> PASS|FAIL|SKIP ::` — PASS additionally needs hover=1, nav=1 and select > 0; `[aetherapp] drive
//! …` carries each step's tail and the clip rc.
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
    serial_println!(":: AETHERAPP: image={} window=bad painted=0 lines=0 hover=0 nav=0 select=0 -> SKIP reason=aarch64-image-owed ::", IMAGE);
}

#[cfg(target_arch = "x86_64")]
pub fn selftest() {
    let verdict_m3 = |window: &str, painted: u64, lines: u64, d: &Drive, v: &str| {
        crate::serial_line::tail_arm(); // the glass reads THIS line's tail, not the program's
        serial_println!(
            ":: AETHERAPP: image={} window={} painted={} lines={} hover={} nav={} select={} -> {} ::",
            IMAGE, window, painted, lines, d.hover as u8, d.nav as u8, d.select, v
        );
    };
    let verdict = |window: &str, painted: u64, lines: u64, v: &str| verdict_m3(window, painted, lines, &Drive::default(), v);
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
    // M3: play the operator into the ring before the kill (only once the program has painted its page).
    let drive = if hit && tail.as_deref().is_some_and(|t| t.starts_with("PASS")) { drive(slot) } else { Drive::default() };
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
    serial_println!(
        "[aetherapp] drive focus={:?} hover={:?} select={:?} clip={:?} nav={:?}",
        drive.tails[0].as_deref(), drive.tails[1].as_deref(), drive.tails[2].as_deref(), drive.tails[3].as_deref(), drive.tails[4].as_deref()
    );
    let m3 = drive.hover && drive.nav && drive.select > 0;
    if ok && !m3 {
        why(if drive.tails[0].is_none() { "no-focus-line" } else if !drive.hover { "no-hover" } else if drive.select == 0 { "no-selection" } else { "no-nav" });
    }
    net_drive();
    verdict_m3(window, painted, lines, &drive, if ok && m3 { "PASS" } else { "FAIL" });
}

/// AETHERNET (LEDGER SR110): the network leg of `tests aether`. AETHER.ELF fetches http:// and https:// pages over the
/// ring-3 socket ABI (crates/user-aether/src/net.rs, `:: AETHER: net url=… status=… bytes=… ms=… tls=… -> …`), and the
/// drive would aim it at a page the KERNEL serves, so the leg needs no outside network. On x86 there is none: no
/// in-kernel HTTP responder exists and smolnet carries no loopback interface a ring-3 socket reaches (the one in-kernel
/// HTTP loopback is aarch64 genet.rs's NET-GATE battery, kernel-side only, and AETHER.ELF is not on that loader yet).
/// So the leg is a SKIP by name, never a pass; the program's own line is the proof on a networked boot (`aether <url>`).
#[cfg(target_arch = "x86_64")]
fn net_drive() {
    serial_println!(":: AETHERNET: drive=SKIP(no-server) reason=no-kernel-served-page (x86: no in-kernel HTTP responder, no ring-3 loopback) proof=aether-url-on-a-networked-boot ::");
}

/// AETHERMETAL M3 (SR96): what the drive read back — each step's verdict tail (focus, hover, select, clip, nav)
/// and the three facts the witness carries.
#[cfg(target_arch = "x86_64")]
#[derive(Default)]
struct Drive {
    tails: [Option<alloc::string::String>; 5],
    hover: bool,
    nav: bool,
    select: u64,
}

/// One drive step: arm the seam's watch on `prefix`, push `evs` into the program's ring (owner `slot + 1`), wait up
/// to `STEP_MS` for the line, and return its verdict tail.
#[cfg(target_arch = "x86_64")]
fn step(slot: u64, prefix: &'static str, evs: &[u64]) -> Option<alloc::string::String> {
    const STEP_MS: u64 = 3_000;
    crate::serial_line::line_watch_arm(prefix);
    crate::serial_line::tail_arm();
    for &e in evs {
        if !crate::arch::syscall::user_input_push_owner(slot + 1, e) {
            crate::serial_line::line_watch_disarm();
            let _ = crate::serial_line::tail_take();
            return None;
        }
    }
    let t0 = crate::arch::ticks();
    let mut hit = false;
    while crate::arch::ticks() < t0 + STEP_MS {
        if crate::serial_line::line_watch_hit() {
            hit = true;
            break;
        }
        crate::arch::sched::yield_now();
    }
    crate::serial_line::line_watch_disarm();
    let t1 = crate::arch::ticks();
    while hit && crate::arch::ticks() < t1 + 50 {
        crate::arch::sched::yield_now();
    }
    let tail = crate::serial_line::tail_take();
    if hit { tail } else { None }
}

/// The packed events the drive pushes (the router's encoding, `una_abi::input_ev_pack`).
#[cfg(target_arch = "x86_64")]
fn key(b: u8) -> u64 {
    una_abi::input_ev_pack(una_abi::INPUT_EV_KEY_DOWN, b as u64)
}
#[cfg(target_arch = "x86_64")]
fn abs(x: i64, y: i64) -> u64 {
    una_abi::input_ev_pack(una_abi::INPUT_EV_MOUSE_ABS, una_abi::input_ev_xy_payload(x as i32, y as i32)) // AETHERPTR (B543): window px, what x86's producer hands a windowed program
}
#[cfg(target_arch = "x86_64")]
fn button(mask: u8) -> u64 {
    una_abi::input_ev_pack(una_abi::INPUT_EV_BUTTON, mask as u64)
}

/// `key=<int>[,<int>…]` out of a tail: the integers after `key`, comma separated.
#[cfg(target_arch = "x86_64")]
fn ints(t: &str, key: &str) -> alloc::vec::Vec<i64> {
    let Some(at) = t.find(key) else { return alloc::vec::Vec::new() };
    let rest = &t[at + key.len()..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '-' || c == ',')).unwrap_or(rest.len());
    rest[..end].split(',').filter_map(|d| d.parse().ok()).collect()
}

/// AETHERMETAL M3: Tab → the first link's box; pointer onto it (hover); drag across it (select); Ctrl+C (clip);
/// click (nav).
#[cfg(target_arch = "x86_64")]
fn drive(slot: u64) -> Drive {
    let mut d = Drive::default();
    d.tails[0] = step(slot, ":: AETHER: focus", &[key(b'\t')]);
    let at = d.tails[0].as_deref().filter(|t| t.starts_with("PASS")).map(|t| ints(t, "at=")).unwrap_or_default();
    let [x0, y, x1] = at[..] else { return d };
    d.tails[1] = step(slot, ":: AETHER: hover", &[abs(x0 + 2, y)]);
    d.hover = d.tails[1].as_deref().is_some_and(|t| t.starts_with("PASS"));
    let span = (x1 - x0 - 4).max(4);
    d.tails[2] = step(slot, ":: AETHER: select", &[button(1), abs(x0 + 2 + span, y), button(0)]);
    d.select = d.tails[2].as_deref().filter(|t| t.starts_with("PASS")).and_then(|t| ints(t, "chars=").first().copied()).unwrap_or(0).max(0) as u64;
    d.tails[3] = step(slot, ":: AETHER: clip", &[key(0x03)]);
    d.tails[4] = step(slot, ":: AETHER: nav", &[button(1), button(0)]);
    d.nav = d.tails[4].as_deref().is_some_and(|t| t.starts_with("PASS"));
    d
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
