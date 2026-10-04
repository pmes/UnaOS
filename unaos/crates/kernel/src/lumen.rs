// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! LUMENBIN (rmbp-ledger B305) — `tests lumen`: the kernel-side fixture for `APPS/LUMEN.BIN`, the ring-3
//! Lumen chat window (crates/user-lumen). This file fulfils NOTHING: the chat verbs (130..=133) are owned
//! by VEIN.BIN in ring 3, and the kernel may not register them (kernel tags win, so a kernel fake would
//! change the very routing the fixture is meant to observe). It only drives window and input plumbing.
//!
//! WHAT IT DOES. Loads LUMEN.BIN off the mounted program volume through the real ELF loader (the `bg`
//! path), focuses it, types `hi` + Enter through the production router seam (`user_input_enqueue`, the
//! function the shell's drain calls), and reads the program's WITNESS BLOCK straight out of its slot: the
//! first eight words of its RW segment (`.data.lumenwit`, pinned there by the crate's link script; the
//! segment's p_vaddr is read from the same ELF bytes that were loaded). Leg 1 — with nobody registered for
//! ChatSend the kernel answers `-ENOENT` and the window must count it (`enoent=1`, the line it renders is
//! "no provider: start VEIN.BIN"). Leg 2 — VEIN.BIN is spawned if the volume carries it, then a second
//! message must get at least one ChatReply; with no VEIN.BIN the leg is SKIP with the reason, never a
//! fabricated pass.
//!
//! WITNESS. `:: LUMEN: window=<0|1> sent=<n> replies=<n> enoent=<0|1> -> PASS|SKIP|FAIL ::`, preceded on
//! a SKIP by `:: LUMEN: skip reason=<why> ::`.

#![cfg_attr(not(all(target_arch = "x86_64", feature = "wc")), allow(dead_code))]


/// The witness block's layout (crates/user-lumen/src/main.rs `WIT`): magic, window, sent, replies,
/// enoent, ready, cleared, keys.
const WIT_MAGIC: u32 = 0x4E4D_554C;
const W_WINDOW: usize = 1;
const W_SENT: usize = 2;
const W_REPLIES: usize = 3;
const W_ENOENT: usize = 4;
const W_KEYS: usize = 7;

/// The RW PT_LOAD's p_vaddr in a static ELF64 image (the witness block's offset from the window base).
fn rw_vaddr(elf: &[u8]) -> Option<usize> {
    let rd16 = |o: usize| elf.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let rd32 = |o: usize| elf.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let rd64 = |o: usize| elf.get(o..o + 8).map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize);
    if elf.get(0..4)? != b"\x7fELF" {
        return None;
    }
    let (phoff, phent, phnum) = (rd64(0x20)?, rd16(0x36)?, rd16(0x38)?);
    (0..phnum).map(|i| phoff + i * phent).find(|&p| rd32(p) == Some(1) && rd32(p + 4).is_some_and(|f| f & 2 != 0)).and_then(|p| rd64(p + 0x10))
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn selftest() {
    use crate::arch::syscall as sc;
    let verdict = |w: [u32; 8], v: &str| {
        serial_println!(":: LUMEN: window={} sent={} replies={} enoent={} -> {} ::", w[W_WINDOW], w[W_SENT], w[W_REPLIES], w[W_ENOENT], v);
    };
    let skip = |why: &str| {
        serial_println!(":: LUMEN: skip reason={} ::", why);
        verdict([0; 8], "SKIP");
    };
    let Ok(fs) = crate::fs::fat::mount_program_source() else { return skip("no-program-volume") };
    let load = |name: &str| -> Option<alloc::vec::Vec<u8>> {
        let de = fs.find_app(name).ok()?;
        let cap = sc::user_window_size();
        if de.size == 0 || de.size as usize > cap {
            return None;
        }
        let mut b = alloc::vec::Vec::new();
        fs.read_file(&de, &mut b, cap).ok()?;
        Some(b)
    };
    let Some(img) = load("LUMEN.BIN") else { return skip("LUMEN.BIN-not-on-the-volume") };
    let Some(off) = rw_vaddr(&img).filter(|&o| o + 32 <= sc::user_window_size()) else { return skip("LUMEN.BIN-has-no-RW-segment") };
    let (pid, slot, _entry) = match sc::spawn_user_image_bg(&img) {
        Ok(v) => v,
        Err(why) => {
            serial_println!(":: LUMEN: spawn refused: {} ::", why);
            return verdict([0; 8], "FAIL");
        }
    };
    let slot = slot as usize;
    crate::video::wm::app_name_arm(crate::video::wm::owner_of_launch(slot as u64), "LUMEN.BIN");
    let read = || -> [u32; 8] {
        let p = unsafe { crate::arch::memory::slot_backing_ptr(slot).add(off) } as *const u32;
        core::array::from_fn(|i| unsafe { p.add(i).read_volatile() })
    };
    let wait = |ms: u64, done: &dyn Fn([u32; 8]) -> bool| -> [u32; 8] {
        let end = crate::arch::ticks() + ms;
        loop {
            let w = read();
            if (w[0] == WIT_MAGIC && done(w)) || crate::arch::ticks() >= end {
                return w;
            }
            crate::arch::sched::yield_now();
        }
    };
    let type_line = |s: &[u8]| {
        sc::user_input_set_active(slot as u64 + 1);
        for &b in s {
            let _ = sc::user_input_enqueue(crate::pal::Event::Key(b));
        }
    };

    // The window exists and the first ChatStatus has had time to come back.
    let w = wait(4_000, &|w| w[W_WINDOW] == 1);
    let w = if w[W_WINDOW] == 1 { wait(1_500, &|_| false) } else { w };
    // Leg 1: a send with (normally) nobody on ChatSend.
    type_line(b"hi\n");
    let w1 = wait(4_000, &|w| w[W_SENT] >= 1 && (w[W_ENOENT] == 1 || w[W_REPLIES] >= 1));
    let window_ok = w[0] == WIT_MAGIC && w[W_WINDOW] == 1;
    let leg1 = w1[W_SENT] >= 1 && w1[W_KEYS] >= 3 && (w1[W_ENOENT] == 1 || w1[W_REPLIES] >= 1);

    // Leg 2: VEIN.BIN, if staged, registers 130..=133; the second send must get a reply.
    let mut vein = None;
    let mut w2 = w1;
    let mut skip_why = None;
    if w1[W_REPLIES] >= 1 {
        // A fulfiller was already live in this boot: leg 2 is already proved by leg 1.
    } else if let Some(vimg) = load("VEIN.BIN") {
        match sc::spawn_user_image_bg(&vimg) {
            Ok((vp, vs, _)) => {
                crate::video::wm::app_name_arm(crate::video::wm::owner_of_launch(vs), "VEIN.BIN");
                vein = Some((vp, vs));
                let _ = wait(1_500, &|_| false); // registration
                type_line(b"ping\n");
                w2 = wait(30_000, &|w| w[W_SENT] >= 2 && w[W_REPLIES] >= 1); // a model's first chunk; bounded
            }
            Err(why) => {
                serial_println!(":: LUMEN: VEIN.BIN spawn refused: {} ::", why);
                skip_why = Some("VEIN.BIN-spawn-refused");
            }
        }
    } else {
        skip_why = Some("VEIN.BIN-not-on-the-volume-reply-leg-unproved");
    }

    if let Some((vp, vs)) = vein {
        let _ = sc::bg_kill(vp, vs);
    }
    let _ = sc::bg_kill(pid, slot as u64);
    sc::user_input_set_active(0); // the keyboard back to the shell

    if !(window_ok && leg1) {
        serial_println!(":: LUMEN: leg1 window_ok={} sent={} keys={} enoent={} replies={} ::", window_ok as u8, w1[W_SENT], w1[W_KEYS], w1[W_ENOENT], w1[W_REPLIES]);
        return verdict(w1, "FAIL");
    }
    match skip_why {
        Some(why) => {
            serial_println!(":: LUMEN: skip reason={} ::", why);
            verdict(w2, "SKIP")
        }
        None => verdict(w2, if w2[W_REPLIES] >= 1 { "PASS" } else { "FAIL" }),
    }
}

/// Off x86 `wc` the window plumbing this fixture drives (focus + the x86 slot backing) is not compiled.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn selftest() {
    serial_println!(":: LUMEN: skip reason=needs-x86-wc ::");
    serial_println!(":: LUMEN: window=0 sent=0 replies=0 enoent=0 -> SKIP ::");
}
