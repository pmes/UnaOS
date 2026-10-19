//! CHARTER: Kernel — fs-core
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! UNINSTALL (rmbp-ledger B679, BP-bar-11, BP-storage-15) — an app is its `/apps` entry plus its settings domain,
//! and uninstall leaves nothing.
//!
//! [`uninstall`] removes, through the mount table (BP-storage-1, the one name authority):
//! * the app under `/apps`: the directory `/apps/<name>/` (the one-directory layout), walked bottom-up, and the
//!   flat `/apps/<NAME>.ELF` of the layout every app has today — written as the operator's principal, so ROOTACL's
//!   `system` owner decides (BP-security-6: the administrator's session, or `kernel` for the fixture);
//! * its settings domain `settings/<name>` (and an interrupted swap's `<name>.new`) under EVERY settings folder: the
//!   no-session store's and each user's `<home>/settings`, homes spelt by `users::home_of` (BP-storage-13) and the
//!   folder by `prefs_core::files::DIR` (no `/settings/` literal: the store's paths are prefs.rs's, BP-storage-11).
//!   This is the delete a user makes in Quarry; prefs' delete watch resets the domain in RAM at its next pass;
//! * its dock pin (`dock::unpin_app`, the DOCKPIN store `system.dock.pins`);
//! * its launcher entry: the Launcher's Programs group is the `/apps` listing (launcher.rs header), so the entry
//!   goes with the file; APPRES's per-boot memo of each removed program is dropped (`appres::forget`).
//!
//! Refusals remove nothing: a name that is not one plain segment, a Principia system domain (`display`, `login`,
//! …: a settings pane, never an app), no `/apps` entry by that name, no administrator authority (R100).
//!
//! Wire: `[uninstall] app=<n> apps_files=<n> apps_dirs=<n> settings_removed=<n> pin_removed=<0|1> by=<principal>`,
//! `[uninstall] refused app=<n> why=<w>`; `tests uninstall` →
//! `:: UNINSTALL: app=uninstfx files_before=<n> after=<n> leftover=0 settings_removed=<n> pin_removed=<1|-> lost=0 run=<w> -> PASS ::`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::vfs::{MountTable, NodeKind, KERNEL_PRINCIPAL};

/// Where programs live (ROOTACL's tree; `apptrust::APPS_DIR`).
const APPS: &str = crate::fs::apptrust::APPS_DIR;
/// The walk's bounds: an app directory is a few files, never a volume.
const DEPTH_MAX: usize = 8;
const ENTRIES_MAX: usize = 4096;

/// What [`uninstall`] removed.
#[derive(Clone, Copy, Default)]
pub struct Removed {
    pub apps_files: usize,
    pub apps_dirs: usize,
    pub settings: usize,
    pub pin: bool,
}

/// A name an app can carry: one segment Principia accepts as a domain, not a system domain.
fn name_ok(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > 32 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err("bad-name");
    }
    let d = name.to_ascii_lowercase();
    if !prefs_core::files::valid_domain(&d) {
        return Err("bad-name");
    }
    if prefs_core::files::SYSTEM_DOMAINS.contains(&d.as_str()) {
        return Err("system-domain");
    }
    Ok(())
}

/// The app's entries under `/apps`: `(path, is_dir)` — the directory `<name>` and the flat `<name>.ELF`, by name,
/// ASCII case-insensitive (a FAT name answers to any case).
fn app_entries(mt: &MountTable, name: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let Ok(rows) = mt.read_dir(APPS) else { return out };
    for r in rows {
        let dir = matches!(r.kind, NodeKind::Dir);
        let hit = if dir {
            r.name.eq_ignore_ascii_case(name)
        } else {
            { let nb = r.name.as_bytes(); nb.len() == name.len() + 4 && nb[..name.len()].eq_ignore_ascii_case(name.as_bytes()) && nb[name.len()..].eq_ignore_ascii_case(b".ELF") }
        };
        if hit {
            out.push((alloc::format!("{}/{}", APPS, r.name), dir));
        }
    }
    out
}

/// Remove the tree at `dir` bottom-up as `who`. Answers `(files, dirs)` removed, or the first refusal.
fn remove_tree(mt: &MountTable, dir: &str, who: &str, depth: usize, n: &mut (usize, usize)) -> Result<(), &'static str> {
    if depth > DEPTH_MAX {
        return Err("too-deep");
    }
    let rows = mt.read_dir(dir).map_err(|_| "list")?;
    for r in rows {
        if r.name == "." || r.name == ".." {
            continue;
        }
        let p = alloc::format!("{}/{}", dir, r.name);
        if matches!(r.kind, NodeKind::Dir) {
            remove_tree(mt, &p, who, depth + 1, n)?;
        } else {
            mt.unlink(&p, who).map_err(|_| "unlink")?;
            if p.as_bytes().len() > 4 && p.as_bytes()[p.len() - 4..].eq_ignore_ascii_case(b".ELF") {
                crate::fs::appres::forget(&p);
            }
            n.0 += 1;
        }
        if n.0 + n.1 > ENTRIES_MAX {
            return Err("too-many");
        }
    }
    mt.remove_dir(dir, who).map_err(|_| "rmdir")?;
    n.1 += 1;
    Ok(())
}

/// Every settings folder on the root: the no-session store's (`/settings`, prefs.rs's `base()` = "") and each
/// user's `<home>/settings`. Deduplicated; a folder that does not exist is left out.
pub fn settings_dirs(mt: &MountTable) -> Vec<String> {
    let mut homes: Vec<String> = alloc::vec![String::new()];
    #[cfg(feature = "login")]
    for i in 0..crate::fs::users::MAX_USERS {
        let mut nb = [0u8; crate::fs::users::NAME_MAX];
        let Some(n) = crate::fs::users::name_at(i, &mut nb) else { break };
        let mut hb = [0u8; crate::fs::users::HOME_MAX];
        if let Some(hn) = crate::fs::users::home_of(&nb[..n], &mut hb) {
            if let Ok(h) = core::str::from_utf8(&hb[..hn]) {
                homes.push(String::from(h.trim_end_matches('/')));
            }
        }
    }
    if let Some(h) = crate::prefs::home() {
        homes.push(h);
    }
    let mut out: Vec<String> = Vec::new();
    for h in homes {
        let d = alloc::format!("{}/{}", h, prefs_core::files::DIR);
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&d)) && matches!(mt.stat(&d), Ok(s) if matches!(s.kind, NodeKind::Dir)) {
            out.push(d);
        }
    }
    out
}

/// The principal an operator's uninstall writes `/apps` as: the administrator's session (`apptrust::copy_principal`,
/// R100), or `kernel` while the machine has no administrator yet (`users::admin_authority` says so on the wire).
fn operator_principal() -> Result<String, &'static str> {
    #[cfg(feature = "login")]
    {
        if let Ok(p) = crate::fs::apptrust::copy_principal() {
            return Ok(p);
        }
        crate::fs::users::admin_authority("uninstall")?;
        Ok(String::from(KERNEL_PRINCIPAL))
    }
    #[cfg(not(feature = "login"))]
    {
        Ok(String::from(KERNEL_PRINCIPAL))
    }
}

/// UNINSTALL: remove app `name` — its `/apps` entries as the operator (administrator) principal, its settings domain
/// under every settings folder, its dock pin. Nothing is removed on a refusal.
pub fn uninstall(name: &str) -> Result<Removed, &'static str> {
    let who = operator_principal().inspect_err(|w| serial_println!("[uninstall] refused app={} why={}", name, w))?;
    uninstall_as(name, &who)
}

/// [`uninstall`] as `who` (the fixture runs it as `kernel`, the principal that installed the fixture).
pub fn uninstall_as(name: &str, who: &str) -> Result<Removed, &'static str> {
    let refuse = |why: &'static str| {
        serial_println!("[uninstall] refused app={} why={}", name, why);
        why
    };
    name_ok(name).map_err(refuse)?;
    let mt = crate::shell::vfs_mount_table();
    let entries = app_entries(&mt, name);
    if entries.is_empty() {
        return Err(refuse("not-installed"));
    }
    let mut r = Removed::default();
    let mut n = (0usize, 0usize);
    for (p, dir) in &entries {
        if *dir {
            remove_tree(&mt, p, who, 0, &mut n).map_err(refuse)?;
        } else {
            mt.unlink(p, who).map_err(|_| refuse("unlink"))?;
            crate::fs::appres::forget(p);
            n.0 += 1;
        }
    }
    (r.apps_files, r.apps_dirs) = n;
    // The settings domain: the store's own principal (prefs.rs writes every store path as `kernel`).
    let d = name.to_ascii_lowercase();
    for sd in settings_dirs(&mt) {
        for leaf in [d.clone(), alloc::format!("{}.new", d)] {
            let p = alloc::format!("{}/{}", sd, leaf);
            if mt.stat(&p).is_ok() && mt.unlink(&p, KERNEL_PRINCIPAL).is_ok() {
                r.settings += 1;
            }
        }
    }
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        r.pin = crate::video::dock::unpin_app(&d);
    }
    serial_println!("[uninstall] app={} apps_files={} apps_dirs={} settings_removed={} pin_removed={} by={}", name, r.apps_files, r.apps_dirs, r.settings, r.pin as u8, who);
    Ok(r)
}

/// The `uninstall <name>` verb (midden_core's HOST_VERBS row; shell.rs dispatches it).
pub fn verb(out: &mut dyn FnMut(&str), args: &[&str]) {
    let [name] = args else {
        out("usage: uninstall <name>   (removes /apps/<name>/ and /apps/<name>.ELF, its settings domain for every user, its dock pin)");
        return;
    };
    match uninstall(name) {
        Ok(r) => out(&alloc::format!("uninstall {}: removed {} file(s), {} folder(s) under {}, {} settings file(s), dock pin {}", name, r.apps_files, r.apps_dirs, APPS, r.settings, if r.pin { "removed" } else { "none" })),
        Err(w) => out(&alloc::format!("uninstall {}: refused ({}) — nothing removed", name, w)),
    }
}

// ── `tests uninstall` ───────────────────────────────────────────────────────────────────────────────

/// The fixture app's name (its `/apps` folder, its ELF leaf, its settings domain).
const FX: &str = "uninstfx";
const FX_ELF: &str = "UNINSTFX.ELF";
/// The fixture ELF: the staged ring-3 shell, run in its windowless one-shot mode (`-c help` prints and exits).
const FX_SRC: &str = "/apps/MIDDEN.ELF";
const FX_STANZA: &[u8] = b"uninstfx\0ran\tstr:16\t\"\"\tThe UNINSTALL fixture's run mark\n";
const IMAGE_MAX: u64 = 16 << 20;

/// Register `tests uninstall` once (tests.rs's `ensure_shellux` line).
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("uninstall", selftest);
    }
}

/// The census the diff compares: every entry under `/apps` (recursive, bounded) and every file in every settings
/// folder (the folders themselves are the users' store, not an app's).
fn census(mt: &MountTable) -> Vec<String> {
    fn walk(mt: &MountTable, dir: &str, depth: usize, out: &mut Vec<String>) {
        if depth > DEPTH_MAX || out.len() > ENTRIES_MAX {
            return;
        }
        let Ok(rows) = mt.read_dir(dir) else { return };
        for r in rows {
            if r.name == "." || r.name == ".." {
                continue;
            }
            let p = alloc::format!("{}/{}", dir, r.name);
            let sub = matches!(r.kind, NodeKind::Dir);
            out.push(p.to_ascii_lowercase());
            if sub {
                walk(mt, &p, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(mt, APPS, 0, &mut out);
    for sd in settings_dirs(mt) {
        walk(mt, &sd, DEPTH_MAX, &mut out);
    }
    out
}

/// Run the installed fixture once (no launch-name arm: no APPRES sight, so the run writes no registry row) and
/// reap it. `exited` / `faulted` / `killed` / `spawn-refused` / `no-ring3`.
fn run_once(img: &[u8], path: &str) -> &'static str {
    #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
    {
        use crate::arch::syscall::{self as sc, BgPoll};
        let (pid, slot, _) = match sc::spawn_user_image_bg_argv(img, &[path, "-c", "help"]) {
            Ok(t) => t,
            Err(e) => {
                serial_println!("[uninstall] fixture spawn refused ({})", e);
                return "spawn-refused";
            }
        };
        let t0 = crate::arch::ticks();
        while crate::arch::ticks() < t0 + 20_000 {
            match sc::bg_poll(pid, true) {
                BgPoll::Running => crate::arch::sched::yield_now(),
                BgPoll::Exited(_) => return "exited",
                BgPoll::Faulted => return "faulted",
                _ => return "gone",
            }
        }
        let _ = sc::bg_kill(pid, slot);
        "killed"
    }
    #[cfg(not(any(target_arch = "x86_64", feature = "aarch64_el0")))]
    {
        let _ = (img, path);
        "no-ring3"
    }
}

/// `tests uninstall`: pin the fixture in the dock's list, census, install the fixture ELF to `/apps/uninstfx/`, run
/// it once, write its settings domain, uninstall, census again — nothing the app made may remain and nothing else
/// may go. The pin precedes the first census: the dock list lives in the system's `desktop` domain, not the app's.
pub fn selftest() {
    let verdict = |before: usize, after: usize, leftover: usize, settings: usize, pin: &str, lost: usize, run: &str, v: &str| {
        serial_println!(":: UNINSTALL: app={} files_before={} after={} leftover={} settings_removed={} pin_removed={} lost={} run={} -> {} ::", FX, before, after, leftover, settings, pin, lost, run, v);
    };
    let mt = crate::shell::vfs_mount_table();
    let img = match mt.stat(FX_SRC) {
        Ok(s) if s.size != 0 && s.size <= IMAGE_MAX => match mt.read(FX_SRC, 0, s.size as usize) {
            Ok(b) if b.len() as u64 == s.size => b,
            _ => return verdict(0, 0, 0, 0, "-", 0, "read-failed", "FAIL"),
        },
        _ => {
            serial_println!(":: UNINSTALL: reason={}-not-on-the-volume ::", FX_SRC);
            return verdict(0, 0, 0, 0, "-", 0, "no-fixture", "SKIP");
        }
    };
    // A prior run that stopped midway leaves the fixture behind: clear it so the census starts clean.
    if !app_entries(&mt, FX).is_empty() {
        let _ = uninstall_as(FX, KERNEL_PRINCIPAL);
    }
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    let pins0 = {
        let t = crate::video::dock::pins_text();
        let with = if t.trim().is_empty() { String::from(FX) } else { alloc::format!("{},{}", t, FX) };
        let _ = crate::prefs_client::pref_set(crate::prefs::NS, crate::prefs::key::DOCK_PINS, crate::prefs::PrefValue::Str(with));
        t
    };
    let before = census(&mt);
    // Install: the app's folder and its ELF, as `kernel` (the fixture's installer).
    let dir = alloc::format!("{}/{}", APPS, FX);
    let elf = alloc::format!("{}/{}", dir, FX_ELF);
    let k = KERNEL_PRINCIPAL;
    let installed = mt.create(&dir, NodeKind::Dir, k).is_ok() && mt.create(&elf, NodeKind::File, k).is_ok() && mt.write(&elf, 0, &img, k).map(|n| n == img.len()).unwrap_or(false);
    // Run it once, then its settings domain (`app.uninstfx.*` -> `settings/uninstfx`, through the one store writer).
    let run = if installed { run_once(&img, &elf) } else { "not-installed" };
    let wrote = crate::prefs::declare_kernel(FX_STANZA) && crate::prefs::set(prefs_core::files::APP_NS, "uninstfx.ran", crate::prefs::PrefValue::Str(String::from("1"))).is_ok() && mt.stat(&crate::prefs::domain_path(FX)).is_ok();
    let mid = census(&mt);
    let made = mid.iter().filter(|p| !before.contains(p)).count();
    let r = uninstall_as(FX, KERNEL_PRINCIPAL);
    let after = census(&mt);
    let leftover = after.iter().filter(|p| !before.contains(p)).count();
    let lost = before.iter().filter(|p| !after.contains(p)).count();
    let rem = r.unwrap_or_default();
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    let (pin, pin_ok) = {
        let back = crate::video::dock::pins_text() == pins0;
        (if rem.pin { "1" } else { "0" }, rem.pin && back)
    };
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    let (pin, pin_ok) = ("-", true);
    let run_ok = matches!(run, "exited" | "no-ring3");
    serial_println!("[uninstall] fixture installed={} made={} settings_written={} run={} removed={}/{} files/dirs", installed as u8, made, wrote as u8, run, rem.apps_files, rem.apps_dirs);
    let ok = installed && wrote && r.is_ok() && made >= 3 && leftover == 0 && lost == 0 && rem.settings >= 1 && pin_ok && run_ok;
    verdict(before.len(), after.len(), leftover, rem.settings, pin, lost, run, if ok { "PASS" } else { "FAIL" });
}
