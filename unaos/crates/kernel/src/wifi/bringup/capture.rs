// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI5 M3 (rmbp-ledger B415) — the PRE-IMAGE captures, read-only on the data side, taken on the
//! armed `wifi3` boot after every gate and BEFORE `upload BEGIN` (bcm4331.md §7 "Capture plan";
//! DRIVERS-METHOD §2: the ground truth is a state that works — here, the platform firmware's).
//!
//! CHARTER: Kernel — driver. A child of `bringup` so it uses that file's covenanted accessors
//! (`r16`/`r32`, and `w32` for the ONE register this module writes: `SHM_CONTROL`, the read path's
//! address-window selector — the same single write the flown `shm-probe` makes). No `SHM_DATA`
//! write, no `MACCTL`, no wrapper, no radio port. Every select is counted in `Writes::core_regs`.
//!
//! * **C2 — the EFI's MAC/PHY working state.** d11 MMIO 0x000–0x7FE as 16-bit reads, every word
//!   with its offset; SKIPPED (printed `----`): 0x128/0x12A (Generic IRQ Reason, READ-TO-CLEAR —
//!   [SPEC-V3 ChipInit steps 3/6]), 0x164/0x166 (SHM data ports — a read ADVANCES the window, [SPEC-V3
//!   SHM]), 0x3F8–0x3FE (radio/PHY data ports — a read is an indirect access through a latch the
//!   resident PSM owns). 0x800–0xFFF is NOT read: no spec page names a register there for rev 29,
//!   and an unmapped backplane read is the one way this capture could wedge the bus (owed, see §8).
//!   Then shared memory 0x0000–0x0FFF through routing 0x0001 ([SPEC-V3 SHM], fact 1), 1024 dwords.
//! * **C1 — the resident microcode, the in-boot unwind's pre-image (rung S4i's rung 0).** Routing
//!   0x0300 ([SPEC-V3 SHM + MicrocodeUpload]) read for exactly the `words` our upload will
//!   overwrite — the pre-image of our own writes and nothing more.
//! * **C1's discriminator.** shared[+0x00] and MACCTL re-read after C1: still `0x0288` with
//!   `psm-run=1` ⇒ reading ucode memory under a running PSM is non-disturbing and S4i proceeds;
//!   anything else ⇒ S4i is refuted on this silicon and the reboot stays the only unwind.
//! * **S5i rung 0 (read-only).** Every initvals record whose offset falls inside C2 is compared to
//!   the EFI's captured word: the premise "an initvals offset is a d11 MMIO byte offset" ([SPEC-V3
//!   InitialValues]) predicts agreement far above chance; near-zero agreement refutes the offset
//!   reading BEFORE the first initvals write on metal.

use super::{r16, r32, w32, Writes, D11_MACCTL, D11_SHM_CONTROL, D11_SHM_DATA, MACCTL_PSM_RUN, SHM_ROUTE_SHARED, SHM_ROUTE_UCODE};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;
use crate::sync::Mutex;

/// C2's MMIO capture, kept for C3's post-image diff (single writer: `bringup_once`, once per boot).
static C2_MMIO: Mutex<Vec<Option<u16>>> = Mutex::new(Vec::new());

/// C2's MMIO extent (bytes), 16-bit reads. WIFI5 fixed it at 0x800; WIFI6 (B439) derives it from the
/// EROM walk (`derive_c2_bound` in `bringup.rs`: the d11 slave port's decoded size, only when the
/// walk's base gap agrees, clipped to the 4 KiB aperture) and falls back to 0x800 otherwise.
fn c2_mmio_end() -> u16 {
    match super::super::status::c2_bound() {
        0 => 0x0800,
        b => b.min(0x1000) as u16,
    }
}
/// C2's shared-memory extent: 1024 dwords = shared bytes 0x0000..0x1000.
const C2_SHARED_DWORDS: usize = 1024;
/// Words per printed row.
const ROW: usize = 16;

/// An MMIO halfword offset C2 must not read (side effect on read).
fn skipped(off: u16) -> bool {
    matches!(off, 0x128 | 0x12A | 0x164 | 0x166) || (0x3F8..=0x3FE).contains(&off)
}

fn fnv(mut h: u32, w: u32) -> u32 {
    for b in w.to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Print `words` as rows of 16 with their word offset; all-zero rows are collapsed into one range line.
fn dump32(tag: &str, words: &[u32], unit: u32) {
    let mut zero_from: Option<usize> = None;
    for (i, row) in words.chunks(ROW).enumerate() {
        let at = i * ROW;
        if row.iter().all(|w| *w == 0) {
            zero_from.get_or_insert(at);
            continue;
        }
        if let Some(z) = zero_from.take() {
            serial_println!("[wifi5] {} +{:#06x}..+{:#06x} zero", tag, z as u32 * unit, at as u32 * unit);
        }
        let mut s = String::new();
        for w in row {
            let _ = write!(s, " {:08x}", w);
        }
        serial_println!("[wifi5] {} +{:#06x}{}", tag, at as u32 * unit, s);
    }
    if let Some(z) = zero_from {
        serial_println!("[wifi5] {} +{:#06x}..+{:#06x} zero", tag, z as u32 * unit, words.len() as u32 * unit);
    }
}

/// The pre-image captures. `words`/`staged_fnv` are the staged ucode's stream facts.
pub(super) fn pre_image(bar0: u64, words: u32, staged_fnv: u32, w: &mut Writes) {
    let c2_end = c2_mmio_end();
    serial_println!(
        "[wifi6] c2 mmio extent 0x000-{:#05x} x16 (bound from the EROM, see `[wifi6] c2-bound`)",
        c2_end - 2
    );
    serial_println!(
        ":: WIFI5: capture begin — C2 (d11 MMIO from 0x000 x16, shared 0x0000-0x0fff) then C1 (ucode memory, {} words) — READ-ONLY on the data side; SHM_CONTROL is the one register written ::",
        words
    );

    // ── C2a: MMIO. ──────────────────────────────────────────────────────────────────────────────
    let mut mmio: Vec<Option<u16>> = Vec::with_capacity((c2_end / 2) as usize);
    let mut off = 0u16;
    while off < c2_end {
        mmio.push(if skipped(off) { None } else { Some(unsafe { r16(bar0, off as u64) }) });
        off += 2;
    }
    for (i, row) in mmio.chunks(ROW).enumerate() {
        let mut s = String::new();
        for v in row {
            match v {
                Some(x) => { let _ = write!(s, " {:04x}", x); }
                None => s.push_str(" ----"),
            }
        }
        serial_println!("[wifi5] c2-mmio +{:#05x}{}", i * ROW * 2, s);
    }

    *C2_MMIO.lock() = mmio.clone();

    // ── C2b: shared memory through routing 0x0001, auto-incrementing from byte 0 (fact 2). ───────
    unsafe { w32(bar0, D11_SHM_CONTROL, SHM_ROUTE_SHARED << 16) };
    w.core_regs += 1;
    let mut shared: Vec<u32> = Vec::with_capacity(C2_SHARED_DWORDS);
    for _ in 0..C2_SHARED_DWORDS {
        shared.push(unsafe { r32(bar0, D11_SHM_DATA) });
    }
    dump32("c2-shared", &shared, 4);
    // Self-check against the flown shm-probe: shared[+0x00]'s low half was 0x0288 on every boot f13-f20.
    serial_println!(
        ":: WIFI5: c2 shared[+0x00]={:#010x} lo16={:#06x} (the shm-probe's r16 read 0x0288 on f13-f20; equal ⇒ the dword stream starts where the probe read) ::",
        shared[0], shared[0] & 0xFFFF
    );
    #[cfg(feature = "wifi4")]
    super::live::note_pre(shared[0]); // WIFI7 (B505): S5u's pre-image.

    // ── C1: the resident ucode, exactly the words our stream will overwrite. ────────────────────
    unsafe { w32(bar0, D11_SHM_CONTROL, SHM_ROUTE_UCODE << 16) };
    w.core_regs += 1;
    let mut uc: Vec<u32> = Vec::with_capacity(words as usize);
    let mut h = 0x811c_9dc5u32;
    let mut nonzero = 0u32;
    for _ in 0..words {
        let v = unsafe { r32(bar0, D11_SHM_DATA) };
        h = fnv(h, v);
        if v != 0 { nonzero += 1; }
        uc.push(v);
    }
    dump32("c1-ucode", &uc, 1);

    // ── C1's discriminator: did the read disturb the running PSM? ───────────────────────────────
    unsafe { w32(bar0, D11_SHM_CONTROL, SHM_ROUTE_SHARED << 16) };
    w.core_regs += 1;
    let after = unsafe { r16(bar0, D11_SHM_DATA) };
    let macctl = unsafe { r32(bar0, D11_MACCTL) };
    let psm = (macctl & MACCTL_PSM_RUN) != 0;
    let degenerate = nonzero == 0 || uc.iter().all(|v| *v == 0xFFFF_FFFF);
    let verdict = if degenerate {
        "REFUTED(degenerate read — routing 0x0300 does not read ucode memory under a running PSM; S4i refuted, the reboot stays the unwind)"
    } else if h == staged_fnv {
        "AMBIGUOUS(the read equals the STAGED image — a warm reboot kept our last upload; not the platform firmware's)"
    } else if after == 0x0288 && psm {
        "NON-DISTURBING(S4i proceeds: the restore boot streams this pre-image back and must read 0x0288)"
    } else {
        "DISTURBING(the resident PSM changed under the read; S4i refuted)"
    };
    serial_println!(
        ":: WIFI5: c1 ucode-pre words={} fnv1a={:#010x} first={:#010x} last={:#010x} nonzero={} equals-staged={} shared-after={:#06x} want=0x0288 macctl-after={:#010x} psm-run={} want=1 -> {} ::",
        words, h, uc.first().copied().unwrap_or(0), uc.last().copied().unwrap_or(0), nonzero,
        (h == staged_fnv) as u8, after, macctl, psm as u8, verdict
    );

    // ── S5i rung 0: initvals records vs the EFI's captured words. ───────────────────────────────
    let at = |o: u16| -> Option<u16> { if o < c2_end { mmio[(o / 2) as usize] } else { None } };
    for role in ["initvals", "bsinitvals"] {
        let parsed = super::super::firmware::with_staged(role, |d| wifi_core::fw::records(d));
        let Some(Ok(recs)) = parsed else {
            serial_println!(":: WIFI5: s5i-rung0 {} not-parsed — no comparison ::", role);
            continue;
        };
        let (mut agree, mut differ, mut outside) = (0u32, 0u32, 0u32);
        let mut row = String::new();
        let mut n = 0;
        for r in &recs {
            let cap: Option<u32> = if r.wide {
                match (at(r.offset), at(r.offset.wrapping_add(2))) {
                    (Some(lo), Some(hi)) => Some(((hi as u32) << 16) | lo as u32),
                    _ => None,
                }
            } else {
                at(r.offset).map(|v| v as u32)
            };
            match cap {
                None => outside += 1,
                Some(c) if c == r.value => agree += 1,
                Some(c) => {
                    differ += 1;
                    let _ = write!(row, " {:03x}={:x}/{:x}", r.offset, r.value, c);
                    n += 1;
                    if n == 8 {
                        serial_println!("[wifi5] s5i-delta {}{}", role, row);
                        row.clear();
                        n = 0;
                    }
                }
            }
        }
        if n > 0 {
            serial_println!("[wifi5] s5i-delta {}{}", role, row);
        }
        serial_println!(
            ":: WIFI5: s5i-rung0 {} records={} agree={} differ={} outside-c2={} (delta rows: offset=initval/efi) — premise 'an initvals offset is a d11 MMIO byte offset' [SPEC-V3 InitialValues]: agree far above chance confirms; agree near 0 refutes it BEFORE the first initvals write ::",
            role, recs.len(), agree, differ, outside
        );
    }
}

/// C3 — the post-image (WIFI5 M5, S5's first boot): the SAME MMIO extent and skips as C2, re-read
/// after the upload + initvals, from `phy_once` (wifi4) once its gate passed. Prints ONLY the words
/// that differ from the EFI's C2, with their offsets. Read-only, no select written.
///
/// Reading: `changed` ⊆ the s5i-delta offsets ∪ the ucode's own MAC state ⇒ our core reset left the
/// rest of the EFI's MAC state standing (S5 rung 0's MMIO half); a changed word OUTSIDE both is a
/// state the reset or the ucode cleared that the EFI had set — the first candidate S5 write, with
/// its value already captured (`observed=<efi> source=capture`, DRIVERS-METHOD §2).
#[cfg(feature = "wifi4")]
pub(super) fn post_image(bar0: u64) {
    let c2 = C2_MMIO.lock();
    if c2.is_empty() {
        serial_println!(":: WIFI5: c3 SKIP reason=no-c2-this-boot ::");
        return;
    }
    let (mut same, mut changed, mut regs) = (0u32, 0u32, 0u32);
    let mut row = String::new();
    let mut n = 0;
    for (i, efi) in c2.iter().enumerate() {
        let off = (i * 2) as u16;
        let Some(efi) = efi else { continue };
        regs += 1;
        let now = unsafe { r16(bar0, off as u64) };
        if now == *efi {
            same += 1;
            continue;
        }
        changed += 1;
        let _ = write!(row, " {:03x}={:04x}/{:04x}", off, efi, now);
        n += 1;
        if n == 8 {
            serial_println!("[wifi5] c3-delta{}", row);
            row.clear();
            n = 0;
        }
    }
    if n > 0 {
        serial_println!("[wifi5] c3-delta{}", row);
    }
    serial_println!(
        ":: WIFI5: c3 post-upload+initvals mmio regs={} same={} changed={} (delta rows: offset=efi/now) — S5 rung 0's MMIO half: compare the changed set with the s5i-delta offsets ::",
        regs, same, changed
    );
}

/// WIFI6 (B439): C2's captured word at an MMIO byte offset — the S4i unwind's pre-image source
/// (`unwind.rs`), read from the ONE store C2 filled. `None`: not captured (skipped or past the bound).
#[cfg(feature = "wifi5")]
pub(super) fn c2_word(off: u16) -> Option<u16> {
    C2_MMIO.lock().get((off / 2) as usize).copied().flatten()
}
