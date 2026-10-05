// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Midden — shared-core
//!
//! EXECNAME (rmbp-ledger B322, RULINGS R82) — `tests exec`: a program is named by its format and
//! launched by its name. For each ring-3 program the x86 builder stages under `APPS/` it resolves the
//! BARE name through the shell's own resolver (`shell::exec_resolve_display`, the path `which` and the
//! launch take), reads the image off the namespace, and reads its `.note.unaos.app` launch declaration
//! (`arch::elf::app_flags` → `midden_core::app_note_flags`), then applies `midden_core::launch_mode`.
//! It launches nothing: the claim is "the name finds the program and the program says how it runs".
//!
//! Wire: one `:: EXECNAME: <name> -> <path> flags=<f> launch=<detach|foreground> ::` per name, then
//! `:: EXECNAME: resolved=<k>/<n> windowed=<w> console=<c> -> PASS|FAIL ::`, or
//! `:: EXECNAME: SKIP (<reason>) ::` when no program source is mounted.
//!
//! PASS = every name resolves to `/apps/<NAME>.ELF` and every declaration matches the table below
//! (LUMEN windowed, PREFS resident, NET and BIG console with a note present). VEIN left this table at the
//! merge11 fold: LUMENAPP (B323, R82) retired the chat daemon; SELFDIAG (B324) added DIAG (console), so the line reads 5/5.

/// (bare name, expected flags — `None` = not checked).
const STAGED: &[(&str, Option<u32>)] = &[
    ("prefs", Some(una_abi::APP_FLAG_RESIDENT)),
    ("lumen", Some(una_abi::APP_FLAG_WINDOWED)),
    ("net", Some(0)),
    ("big", Some(0)),
    ("diag", Some(0)), // SELFDIAG (B324): APPS/DIAG.ELF, console
];

pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    if !matches!(mt.stat("/apps"), Ok(st) if matches!(st.kind, crate::fs::vfs::NodeKind::Dir)) {
        serial_println!(":: EXECNAME: SKIP (no program source mounted at /apps) ::");
        return;
    }
    let (mut resolved, mut windowed, mut ok) = (0usize, 0usize, true);
    for &(word, want) in STAGED {
        let Some(path) = crate::shell::exec_resolve_display(word) else {
            serial_println!(":: EXECNAME: {} -> NOT FOUND ::", word);
            ok = false;
            continue;
        };
        let leaf = path.strip_prefix("/apps/").unwrap_or("").as_bytes(); // want `/apps/<WORD>.ELF`, any case
        let leaf_ok = leaf.len() == word.len() + 4 && leaf[..word.len()].eq_ignore_ascii_case(word.as_bytes())
            && leaf[word.len()..].eq_ignore_ascii_case(b".ELF");
        let bytes = match mt.stat(&path) {
            Ok(st) => mt.read(&path, 0, st.size as usize).ok(),
            Err(_) => None,
        };
        let Some(bytes) = bytes else {
            serial_println!(":: EXECNAME: {} -> {} UNREADABLE ::", word, path);
            ok = false;
            continue;
        };
        resolved += 1;
        let note = midden_core::app_note_flags(&bytes);
        let flags = crate::arch::elf::app_flags(&bytes);
        if flags & una_abi::APP_FLAG_WINDOWED != 0 {
            windowed += 1;
        }
        let mode = match midden_core::launch_mode(flags) {
            midden_core::LaunchMode::Detach => "detach",
            midden_core::LaunchMode::Foreground => "foreground",
        };
        let match_ok = match want {
            None => true,
            Some(w) => note.is_some() && flags == w,
        };
        ok &= leaf_ok && match_ok;
        serial_println!(":: EXECNAME: {} -> {} flags={} note={} launch={}{} ::", word, path, flags,
            if note.is_some() { "yes" } else { "no" }, mode,
            if leaf_ok && match_ok { "" } else { " MISMATCH" });
    }
    let n = STAGED.len();
    serial_println!(":: EXECNAME: resolved={}/{} windowed={} console={} -> {} ::", resolved, n, windowed,
        resolved - windowed, if ok && resolved == n { "PASS" } else { "FAIL" });
}

/// Register `tests exec` exactly once (x86: the five are x86 images; the aarch64 images are owed).
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("exec", selftest);
    }
}
