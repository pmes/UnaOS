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
//! 5. DRIVE (AETHERMETAL4, LEDGER SR109): THE CHROME, one step per feature after M3's, same channel: Ctrl+T (0x14)
//!    opens a tab (`:: AETHER: tab -> PASS how=new tabs=2` → `tab=1`); the new tab's address field has the keys,
//!    so Ctrl+L (0x0C) + `/system/doc/welcome.html` + Enter types a URL to a local page (`:: AETHER: nav -> PASS
//!    how=omnibox` → `url=1`); Ctrl+F (0x06) + `aether` + Enter finds in it (`:: AETHER: find -> PASS hits=<n>` →
//!    `find=<n>`), Esc closes the bar; Left goes back to the new-tab page (`:: AETHER: nav -> PASS how=back` →
//!    `back=1`); `=` zooms (`:: AETHER: zoom -> PASS pct=110`, reported) and Ctrl+W closes the tab (`:: AETHER: tab
//!    -> PASS how=close tabs=1`, reported). Every pointer position the drive pushes is WINDOW px (the focus line's
//!    `at=` is reported in them, below the chrome).
//!
//! WITNESS (SR109). `… hover=<0|1> nav=<0|1> select=<n> tab=<0|1> url=<0|1> back=<0|1> find=<n> -> PASS|FAIL|SKIP ::`
//! — PASS additionally needs tab=1, url=1, back=1 and find > 0; `[aetherapp] chrome …` carries each step's tail.
//!
//! 6. OMNIBOX TO THE NETWORK (AETHERMETAL5, SR109 with AETHERNET SR110): last, Ctrl+L + `https://example.org` + Enter
//!    (`:: AETHER: omnibox -> PASS how=enter kind=url text=https://example.org` — written before the fetch starts, so
//!    the step never waits on a network). The fetch itself has no server to reach here (`net_drive`'s reason), so the
//!    step is reported as `:: AETHERNET: omnibox=SKIP(no-server) routed=<url|…> …`, beside the drive's own SKIP line,
//!    and is not in the verdict: the SR109 verdict above is unchanged.
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
    serial_println!(":: AETHERAPP: image={} window=bad painted=0 lines=0 hover=0 nav=0 select=0 tab=0 url=0 back=0 find=0 -> SKIP reason=aarch64-image-owed ::", IMAGE);
}

#[cfg(target_arch = "x86_64")]
pub fn selftest() {
    let verdict_m3 = |window: &str, painted: u64, lines: u64, d: &Drive, v: &str| {
        crate::serial_line::tail_arm(); // the glass reads THIS line's tail, not the program's
        serial_println!(
            ":: AETHERAPP: image={} window={} painted={} lines={} hover={} nav={} select={} tab={} url={} back={} find={} -> {} ::",
            IMAGE, window, painted, lines, d.hover as u8, d.nav as u8, d.select, d.tab as u8, d.url as u8, d.back as u8, d.find, v
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
    serial_println!(
        "[aetherapp] chrome tab={:?} url={:?} find={:?} back={:?} zoom={:?} close={:?} https={:?}",
        drive.tails[5].as_deref(), drive.tails[6].as_deref(), drive.tails[7].as_deref(), drive.tails[8].as_deref(), drive.tails[9].as_deref(), drive.tails[10].as_deref(),
        drive.tails[11].as_deref()
    );
    let m3 = drive.hover && drive.nav && drive.select > 0;
    let chrome = drive.tab && drive.url && drive.back && drive.find > 0; // SR109
    if ok && !m3 {
        why(if drive.tails[0].is_none() { "no-focus-line" } else if !drive.hover { "no-hover" } else if drive.select == 0 { "no-selection" } else { "no-nav" });
    } else if ok && !chrome {
        why(if !drive.tab { "no-tab" } else if !drive.url { "no-omnibox-nav" } else if drive.find == 0 { "no-find" } else { "no-back" });
    }
    net_drive(drive.tails[11].as_deref());
    verdict_m3(window, painted, lines, &drive, if ok && m3 && chrome { "PASS" } else { "FAIL" });
}

/// AETHERNET (LEDGER SR110): the network leg of `tests aether`. AETHER.ELF fetches http:// and https:// pages over the
/// ring-3 socket ABI (crates/user-aether/src/net.rs, `:: AETHER: net url=… status=… bytes=… ms=… tls=… -> …`), and the
/// drive would aim it at a page the KERNEL serves, so the leg needs no outside network. On x86 there is none: no
/// in-kernel HTTP responder exists and smolnet carries no loopback interface a ring-3 socket reaches (the one in-kernel
/// HTTP loopback is aarch64 genet.rs's NET-GATE battery, kernel-side only, and AETHER.ELF is not on that loader yet).
/// So the leg is a SKIP by name, never a pass; the program's own line is the proof on a networked boot (`aether <url>`).
///
/// AETHERMETAL5: the omnibox's https step (`chrome`, last) is reported on the same terms — `routed=url` when the field
/// sent `https://example.org` to the network path (the program's `:: AETHER: net` line follows on its own), the
/// omnibox tail's word otherwise — and SKIP(no-server) by name, never a pass.
#[cfg(target_arch = "x86_64")]
fn net_drive(omni: Option<&str>) {
    serial_println!(":: AETHERNET: drive=SKIP(no-server) reason=no-kernel-served-page (x86: no in-kernel HTTP responder, no ring-3 loopback) proof=aether-url-on-a-networked-boot ::");
    let routed = match omni {
        Some(t) if t.starts_with("PASS") && t.contains("kind=url") => "url",
        Some(t) if t.starts_with("PASS") => "other",
        Some(_) => "fail",
        None => "no-line",
    };
    serial_println!(":: AETHERNET: omnibox=SKIP(no-server) text=https://example.org routed={} proof=aether-omnibox-on-a-networked-boot ::", routed);
}

/// AETHERMETAL M3 (SR96): what the drive read back — each step's verdict tail (focus, hover, select, clip, nav)
/// and the three facts the witness carries.
#[cfg(target_arch = "x86_64")]
#[derive(Default)]
struct Drive {
    /// M3: focus, hover, select, clip, nav; SR109: tab, url, find, back, zoom, close; AETHERMETAL5: the https omnibox.
    tails: [Option<alloc::string::String>; 12],
    hover: bool,
    nav: bool,
    select: u64,
    tab: bool,
    url: bool,
    back: bool,
    find: u64,
}

/// One drive step: arm the seam's watch on `prefix`, push `evs` into the program's ring (owner `slot + 1`), wait up
/// to `STEP_MS` for the line, and return its verdict tail.
#[cfg(target_arch = "x86_64")]
fn step(slot: u64, prefix: &'static str, evs: &[u64]) -> Option<alloc::string::String> {
    const STEP_MS: u64 = 3_000;
    crate::serial_line::line_watch_arm(prefix);
    crate::serial_line::tail_arm();
    if !push(slot, evs) {
        crate::serial_line::line_watch_disarm();
        let _ = crate::serial_line::tail_take();
        return None;
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

/// Push `evs` into the program's ring (owner `slot + 1`) in order; a full ring (32 events) is waited on for up to
/// 500 ms while the program drains it (SR109: a typed URL is ~26 events). False when it never drained.
#[cfg(target_arch = "x86_64")]
fn push(slot: u64, evs: &[u64]) -> bool {
    for &e in evs {
        let t0 = crate::arch::ticks();
        while !crate::arch::syscall::user_input_push_owner(slot + 1, e) {
            if crate::arch::ticks() >= t0 + 500 {
                return false;
            }
            crate::arch::sched::yield_now();
        }
    }
    true
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
    chrome(slot, &mut d);
    d
}

/// AETHERMETAL4 (SR109): the chrome, one drive per feature — open a tab, type a URL to a local page, find in it,
/// back; then zoom and close the tab (reported); last the omnibox's https step (reported, AETHERMETAL5).
#[cfg(target_arch = "x86_64")]
fn chrome(slot: u64, d: &mut Drive) {
    let pass = |t: &Option<alloc::string::String>, want: &str| t.as_deref().is_some_and(|t| t.starts_with("PASS") && t.contains(want));
    d.tails[5] = step(slot, ":: AETHER: tab", &[key(0x14)]);
    d.tab = pass(&d.tails[5], "how=new");
    let mut evs: alloc::vec::Vec<u64> = alloc::vec![key(0x0C)];
    evs.extend(b"/system/doc/welcome.html".iter().map(|&c| key(c)));
    evs.push(key(b'\n'));
    d.tails[6] = step(slot, ":: AETHER: nav", &evs);
    d.url = pass(&d.tails[6], "how=omnibox");
    let mut evs: alloc::vec::Vec<u64> = alloc::vec![key(0x06)];
    evs.extend(b"aether".iter().map(|&c| key(c)));
    evs.push(key(b'\n'));
    d.tails[7] = step(slot, ":: AETHER: find", &evs);
    d.find = d.tails[7].as_deref().filter(|t| t.starts_with("PASS")).and_then(|t| ints(t, "hits=").first().copied()).unwrap_or(0).max(0) as u64;
    if d.tails[7].is_some() {
        let _ = push(slot, &[key(una_abi::KEY_ESC)]); // the bar takes the Esc (with no bar open it would exit)
    }
    d.tails[8] = step(slot, ":: AETHER: nav", &[key(una_abi::KEY_LEFT)]);
    d.back = pass(&d.tails[8], "how=back");
    d.tails[9] = step(slot, ":: AETHER: zoom", &[key(b'=')]);
    d.tails[10] = step(slot, ":: AETHER: tab", &[key(0x17)]);
    // AETHERMETAL5: LAST, because the fetch it starts holds the program until its deadline; the omnibox line is
    // written before the fetch, so the step reads it at once. Reported by `net_drive`, never in the verdict.
    let mut evs: alloc::vec::Vec<u64> = alloc::vec![key(0x0C)];
    evs.extend(b"https://example.org".iter().map(|&c| key(c)));
    evs.push(key(b'\n'));
    d.tails[11] = step(slot, ":: AETHER: omnibox", &evs);
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
