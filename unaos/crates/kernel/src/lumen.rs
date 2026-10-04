// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! LUMENAPP (rmbp-ledger B323, R82) — `tests lumen`: a RING-3-FREE witness that the one-program Lumen is
//! on the volume and is the program the loader will take. It runs nothing and fulfils nothing (the old
//! LUMENBIN fixture drove the window against a VEIN.BIN chat daemon over bus verbs; both are retired).
//!
//! WHAT IT CHECKS. `/apps/LUMEN.ELF` is staged; the production validator (`arch::x86_64::elf::
//! validate_elf`, the one `bg` runs) accepts it in the ELF-window model; its `.note.unaos.app` names
//! "UnaOS", type 1, flags bit0 = windowed. And it reports what the program will run on, by the SAME rule
//! the program applies (`vein_core::prefs::plan` — the shared core, so the two cannot disagree): the
//! provider from Principia's `vein` namespace, and the key's state from `vein.key_file` stat'ed through
//! the VFS (an inode id = UnaFS; FAT is refused).
//!
//! WITNESS. `:: LUMENAPP: image=/apps/LUMEN.ELF window=<elf|fixed|bad> provider=<claude|echo>
//! key=<unafs|none|fat-refused> -> PASS|SKIP|FAIL ::`, preceded on SKIP/FAIL by `:: LUMENAPP: reason=… ::`.

use vein_core::prefs::{self as rules, KeyState, Plan};

const IMAGE: &str = "/apps/LUMEN.ELF";

/// The provider and key state the program will see (the shared rule over the kernel's own store).
fn session() -> (&'static str, KeyState) {
    let lit = |k: &str| crate::prefs::get("vein", k).map(|v| v.to_literal());
    let s = |k: &str| crate::prefs::get("vein", k).and_then(|v| v.as_str().map(alloc::string::String::from));
    let key = match s("key_file") {
        None => KeyState::None,
        Some(path) => {
            let mut out = alloc::vec::Vec::new();
            if crate::fs::attrsys::do_stat(path.as_bytes(), crate::fs::vfs::KERNEL_PRINCIPAL, &mut out) != 0 || out.len() < 8 {
                KeyState::None
            } else if u32::from_le_bytes([out[4], out[5], out[6], out[7]]) & una_abi::STAT_HAS_ID != 0 {
                KeyState::UnaFs
            } else {
                KeyState::OnFat
            }
        }
    };
    let prov = lit("provider");
    let tls = lit("tls");
    let ep_s = s("endpoint");
    let ep = match ep_s.as_deref() {
        None | Some("") => Some(rules::DEFAULT_ENDPOINT),
        Some(u) => rules::parse_endpoint(u),
    };
    // The program cannot verify certificates yet (vein_ring3::VERIFIES_CERTS = false); same input here.
    let plan = rules::plan(rules::provider_pref(prov.as_deref().map(str::as_bytes)), ep.as_ref(), key, rules::tls_policy(tls.as_deref().map(str::as_bytes)), false);
    (if matches!(plan, Plan::Claude { .. }) { "claude" } else { "echo" }, key)
}

/// The `.note.unaos.app` flags word, if the image carries one (PT_NOTE, name "UnaOS", type 1).
fn app_note_flags(elf: &[u8]) -> Option<u32> {
    let rd16 = |o: usize| elf.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let rd32 = |o: usize| elf.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let rd64 = |o: usize| elf.get(o..o + 8).map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize);
    let (phoff, phent, phnum) = (rd64(0x20)?, rd16(0x36)?, rd16(0x38)?);
    for i in 0..phnum {
        let p = phoff + i * phent;
        if rd32(p)? != 4 {
            continue; // PT_NOTE only
        }
        let (off, sz) = (rd64(p + 8)?, rd64(p + 0x20)?);
        let mut o = off;
        while o + 12 <= off + sz {
            let (nsz, dsz, ty) = (rd32(o)? as usize, rd32(o + 4)? as usize, rd32(o + 8)?);
            let name = elf.get(o + 12..o + 12 + nsz)?;
            let d = o + 12 + ((nsz + 3) & !3);
            if name == b"UnaOS\0" && ty == 1 && dsz >= 4 {
                return rd32(d);
            }
            o = d + ((dsz + 3) & !3);
        }
    }
    None
}

#[cfg(target_arch = "x86_64")]
pub fn selftest() {
    let (provider, key) = session();
    let verdict = |window: &str, v: &str| {
        serial_println!(":: LUMENAPP: image={} window={} provider={} key={} -> {} ::", IMAGE, window, provider, key.as_str(), v);
    };
    let why = |r: &str| serial_println!(":: LUMENAPP: reason={} ::", r);
    let Ok(fs) = crate::fs::fat::mount_program_source() else {
        why("no-program-volume");
        return verdict("bad", "SKIP");
    };
    let cap = crate::arch::syscall::user_image_cap();
    let img = match fs.find_app("LUMEN.ELF") {
        Ok(de) if de.size != 0 && de.size as usize <= cap => {
            let mut b = alloc::vec::Vec::new();
            match fs.read_file(&de, &mut b, cap) {
                Ok(_) => b,
                Err(_) => {
                    why("read-failed");
                    return verdict("bad", "FAIL");
                }
            }
        }
        Ok(_) => {
            why("size-out-of-range");
            return verdict("bad", "FAIL");
        }
        Err(_) => {
            why("LUMEN.ELF-not-on-the-volume");
            return verdict("bad", "SKIP");
        }
    };
    let plan = match crate::arch::x86_64::elf::validate_elf(&img, crate::arch::syscall::user_window_size()) {
        Ok(p) => p,
        Err(e) => {
            serial_println!(":: LUMENAPP: reason=loader-refused ({}) ::", e);
            return verdict("bad", "FAIL");
        }
    };
    let window = if plan.model_elf { "elf" } else { "fixed" };
    let note = app_note_flags(&img);
    serial_println!(
        "[lumenapp] bytes={} entry={:#x} segs={} stack={} note_flags={:?}",
        img.len(),
        plan.entry,
        plan.nsegs,
        plan.stack,
        note
    );
    if !plan.model_elf {
        why("not-the-elf-model");
        return verdict(window, "FAIL");
    }
    if note.map_or(true, |f| f & una_abi::APP_FLAG_WINDOWED == 0) {
        why("no-windowed-app-note");
        return verdict(window, "FAIL");
    }
    verdict(window, "PASS")
}

/// aarch64: no ELF window on that loader yet, so no LUMEN.ELF image is built for it (owed).
#[cfg(not(target_arch = "x86_64"))]
pub fn selftest() {
    let (provider, key) = session();
    let _ = app_note_flags;
    serial_println!(":: LUMENAPP: reason=aarch64-image-owed ::");
    serial_println!(":: LUMENAPP: image={} window=bad provider={} key={} -> SKIP ::", IMAGE, provider, key.as_str());
}
