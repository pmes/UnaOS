// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Kernel — kernel-by-ruling (the ring-3 ABI: `una-abi` declares it once, each arch's
//! `syscall.rs` owns only the user copy; Holocron's users store answers who a program runs as)
//!
//! RING3ABI2 (rmbp-ledger B333): the arch-neutral bodies behind the ring-3 surface's loose ends —
//! the `SYS_WHOAMI` (59) record and the `tests ring3abi` fixture. The args page itself is per-arch
//! (x86 `memory.rs` tail, aarch64 `xwin.rs`), its layout is `una_abi::args_build`'s. Design and witness:
//! `docs/dev/evidence/rmbp-1005/RING3ABI2.md`.

use alloc::vec::Vec;
use una_abi::{EINVAL, ENOENT};

/// The `SYS_WHOAMI` record for user `uid` (0 = the caller runs anonymously): `una_abi::whoami_build` over
/// the users store — the session's name, its uid, and its home as the store records it (never a
/// `/home/<name>` literal). gid = uid (UnaOS has no group store; each user is its own group). `-ENOENT`
/// for an anonymous caller, a uid that is not the open session's, or a build without `login`.
pub fn whoami_record(uid: u32) -> Result<Vec<u8>, i64> {
    #[cfg(feature = "login")]
    {
        use crate::fs::users;
        if uid == 0 {
            return Err(ENOENT);
        }
        let mut nb = [0u8; users::NAME_MAX];
        let n = users::whoami(&mut nb).ok_or(ENOENT)?;
        let name = &nb[..n];
        if users::id_of(name) != Some(uid) {
            return Err(ENOENT);
        }
        let mut hb = [0u8; users::HOME_MAX];
        let h = users::home_of(name, &mut hb).ok_or(ENOENT)?;
        let mut out = alloc::vec![0u8; una_abi::WHOAMI_MAX];
        let k = una_abi::whoami_build(uid, uid, name, &hb[..h], &mut out).ok_or(EINVAL)?;
        out.truncate(k);
        Ok(out)
    }
    #[cfg(not(feature = "login"))]
    {
        let _ = (uid, EINVAL);
        Err(ENOENT)
    }
}

/// The uid of the open session (0 = none) — what a program launched now runs as.
pub fn session_uid() -> u32 {
    #[cfg(feature = "login")]
    {
        let mut nb = [0u8; crate::fs::users::NAME_MAX];
        if let Some(n) = crate::fs::users::whoami(&mut nb) {
            return crate::fs::users::id_of(&nb[..n]).unwrap_or(0);
        }
    }
    0
}

/// Which ring-3 window an x86 image asks for: `elf` (lowest PT_LOAD at or above the ELF window VA),
/// `fixed`, or `missing`/`bad`. Bounds-checked; reads only the program headers.
fn window_of(path: &str) -> &'static str {
    let mt = crate::shell::vfs_mount_table();
    let full = crate::shell::vfs_path(path);
    let Ok(st) = mt.stat(&full) else { return "missing" };
    let Ok(b) = mt.read(&full, 0, (st.size as usize).min(4096)) else { return "missing" };
    let rd = |o: usize, n: usize| -> Option<u64> {
        let s = b.get(o..o + n)?;
        let mut v = 0u64;
        for (i, &c) in s.iter().enumerate() {
            v |= (c as u64) << (8 * i);
        }
        Some(v)
    };
    if b.len() < 64 || b[0..4] != [0x7F, b'E', b'L', b'F'] {
        return "bad";
    }
    let (Some(phoff), Some(phent), Some(phnum)) = (rd(32, 8), rd(54, 2), rd(56, 2)) else { return "bad" };
    let mut lo = u64::MAX;
    for i in 0..phnum as usize {
        let ph = phoff as usize + i * phent as usize;
        if rd(ph, 4) == Some(1) {
            match rd(ph + 16, 8) {
                Some(v) => lo = lo.min(v),
                None => return "bad",
            }
        }
    }
    if lo == u64::MAX {
        "bad"
    } else if lo >= una_abi::USER_XWIN_VA_X86 {
        "elf"
    } else {
        "fixed"
    }
}

/// `tests ring3abi`: one line per rung, then the verdict
/// `:: RING3ABI2: getrandom=<0|1> args=<n> whoami=<name|none> net_window=<..> big_window=<..> arm_sbrk=<1|0|skip> -> PASS|FAIL ::`.
pub fn selftest() {
    // M1 — entropy through the syscall body every build now carries.
    let mut a = [0u8; 32];
    let got = crate::rand::getrandom(&mut a);
    let gr = got == 32 && a != [0u8; 32];
    // M2 — the args page, written by the launchers' own writer into a real slot and read back as ring 3
    // reads it (`una_abi::Args` over the page bytes).
    let (args_n, args_ok) = args_probe();
    // M3 — who the session is, through the SYS_WHOAMI record.
    let uid = session_uid();
    let who = whoami_record(uid).ok();
    let w = who.as_deref().and_then(una_abi::whoami_parse);
    let wname = w.map(|w| core::str::from_utf8(w.name).unwrap_or("?")).unwrap_or("none");
    if let Some(w) = w {
        serial_println!("[ring3abi] whoami uid={} gid={} home={}", w.uid, w.gid, core::str::from_utf8(w.home).unwrap_or("?"));
    }
    // M4 — the staged images' windows (x86 images; aarch64 stages none of them).
    #[cfg(target_arch = "x86_64")]
    let (net, big) = (window_of("/apps/NET.ELF"), window_of("/apps/BIG.ELF"));
    #[cfg(not(target_arch = "x86_64"))]
    let (net, big) = { let _ = window_of; ("skip", "skip") };
    // M5 — the aarch64 extension GiB: a heap page mapped, written, read back and freed.
    #[cfg(target_arch = "aarch64")]
    let arm = match crate::arch::aarch64::xwin::selftest_probe() { Some(true) => "1", Some(false) => "0", None => "skip" };
    #[cfg(not(target_arch = "aarch64"))]
    let arm = "skip";
    let win_ok = |w: &str| w == "elf" || w == "skip";
    let pass = gr && args_ok && win_ok(net) && win_ok(big) && arm != "0";
    serial_println!(
        ":: RING3ABI2: getrandom={} args={} whoami={} net_window={} big_window={} arm_sbrk={} -> {} ::",
        gr as u8, args_n, wname, net, big, arm, if pass { "PASS" } else { "FAIL" }
    );
}

/// Write `net example.com` into a fresh slot's args page with the arch's launcher writer, read it back.
fn args_probe() -> (usize, bool) {
    const WORDS: [&str; 2] = ["net", "example.com"];
    #[cfg(target_arch = "x86_64")]
    {
        use crate::arch::memory;
        let Some(s) = memory::alloc_user_space() else { return (0, false) };
        let wrote = memory::args_write(s, &WORDS);
        let n = memory::args_argc(s);
        memory::free_user_space_by_cr3(memory::slot_cr3(s));
        serial_println!("[ring3abi] args slot={} wrote={} argc={} va={:#x}", s, wrote, n, una_abi::USER_ARGS_VA_X86);
        (n, wrote && n == WORDS.len())
    }
    #[cfg(target_arch = "aarch64")]
    {
        match crate::arch::aarch64::xwin::args_probe(&WORDS) {
            Some(n) => {
                serial_println!("[ring3abi] args argc={} va={:#x}", n, una_abi::USER_ARGS_VA_ARM);
                (n, n == WORDS.len())
            }
            None => (0, false),
        }
    }
}
