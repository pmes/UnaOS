// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI5 (rmbp-ledger B415) — the BCM4331 rung ledger as ONE typed line (`tests wifi`, R80).
//!
//! CHARTER: Kernel — driver. The table below IS `docs/dev/OS/06_NETWORK_STACK/bcm4331.md` §7, row for
//! row; every `Confirmed`/`Refuted` carries the flight and the wire line it was read from
//! (DRIVERS-METHOD §3: no rung is settled by reading). Two rows are re-measured LIVE this boot: `F`
//! (the firmware set is on THIS medium AND pinned: `staged_count() == 3`, FWPIN B455) and `R0` (this boot printed
//! `-> UPLOADED`, which re-confirms it rather than leaving it on a September capture). The witness
//! states the SPEC'S shape of readiness, not the output we happened to get (R89):
//!
//! `READY` ⇔ the next armed rung (S5's first boot, bcm4331.md §8) has (a) an unwind proven on metal
//! — the reboot, S4u, confirmed f13→f14 — and (b) the firmware set staged on the medium this kernel
//! booted from. Anything else is `NOT-READY reason=<the missing term>`.

use super::status;

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    Confirmed,
    Refuted,
    Open,
    Parked,
}

/// bcm4331.md §7, in table order. `(rung, status, where it was read)`.
const LEDGER: [(&str, St, &str); 27] = [
    ("R0", St::Confirmed, "f13 33884ms -> UPLOADED rev=666 verify MATCH ready=1"),
    ("S0", St::Confirmed, "f25 radio 04:00.0 device=0x4331 MATCH"),
    ("S1", St::Confirmed, "f25 RESTORE cfg:0x80 restored=MATCH"),
    ("S1u", St::Refuted, "f25 unwind-selftest discriminating=0"),
    ("S1b", St::Confirmed, "f25 erom-walk verdict=WALK-OK"),
    ("S1b'", St::Parked, "bridge arity mismatches off the d11 path"),
    ("S1c", St::Confirmed, "f25 d11 FOUND rev=29 phy type=7"),
    ("S2r", St::Open, "no wifi-s2 line in f13-f25"),
    ("S2", St::Open, "SPROM/OTP write path unbuilt"),
    ("S3", St::Confirmed, "f25 reachability=SATISFIED(no-write)"),
    ("S3o", St::Confirmed, "f13 shared[+0x00]=0x0288 vs ours 0x029a"),
    ("S3s", St::Confirmed, "f13 window-answers=YES shm-enabled=0"),
    ("S4p", St::Confirmed, "f13 prologue post psm-run=0 MATCH"),
    ("S4m", St::Open, "f13 macctl 0x404 readback 0x80000404 took=0 (bit 31)"),
    ("S4", St::Confirmed, "f13 upload handshake rev=666"),
    ("S4u", St::Confirmed, "f14 core-pre psm-run=1 shared 0x0288 after f13's upload"),
    ("S4w", St::Refuted, "f13 prologue post macctl=0x80000000 psm-run=0"),
    ("S4i", St::Open, "C1 pre-image read not yet flown"),
    ("S5a", St::Confirmed, "f13 V4 order 0x0205917f radio 0x2059 mfg 0x17f"),
    ("S5b", St::Confirmed, "f13 phy-alive verdict=PHY-ALIVE"),
    ("S5r", St::Parked, "AI wrapper PHY_RESET bit unpinned"),
    ("S5i", St::Open, "initvals never written on metal"),
    ("S5c", St::Parked, "HT-PHY 2059 tables in no legal source; reopens on C2"),
    ("S6", St::Open, "DMA rings unbuilt"),
    ("S7", St::Open, "MAC/station unbuilt"),
    ("S8", St::Open, "WPA2 unbuilt"),
    ("F", St::Open, "firmware on the medium: live"),
];

/// `tests wifi` — `:: WIFI5: rungs=<n> confirmed=<n> refuted=<n> open=<n> parked=<n> upload_unwind=<…> -> READY|NOT-READY ::`
pub fn witness() {
    let staged = super::firmware::staged_count();
    let uploaded = status::ucode_ok();
    let (mut c, mut r, mut o, mut p) = (0u32, 0u32, 0u32, 0u32);
    for (id, st, _) in LEDGER.iter() {
        // F is the one row this boot measures: the set on THIS medium.
        let st = if *id == "F" && staged >= 3 { St::Confirmed } else { *st };
        match st {
            St::Confirmed => c += 1,
            St::Refuted => r += 1,
            St::Open => o += 1,
            St::Parked => p += 1,
        }
    }
    // S4u (reboot) is confirmed on metal; S4i (in-boot re-upload of the C1 pre-image) is open.
    let unwind = "reboot-proven";
    // FWPIN (B455): nothing stages unless its SHA-256 equals its `unaos/firmware/b43.pins` row, so a full set
    // IS a pinned set; short of it, the refusal (unpinned / violates-layout / pin-mismatch) is the reason.
    let refusal = super::firmware::refusal();
    let fw = if staged >= 3 {
        alloc::string::String::from("fw=pinned sha=match")
    } else {
        refusal.map(|r| alloc::format!("fw=refused reason={}", r)).unwrap_or_else(|| alloc::string::String::from("fw=absent"))
    };
    let reason = if staged < 3 { Some(refusal.unwrap_or("firmware-not-staged")) } else { None };
    let hint = match reason {
        Some("unpinned") => " (pin the WIFI-FW build line's sha256 rows into unaos/firmware/b43.pins)",
        Some(_) => " (UNAOS_WIFI_FW_PATH stages the set onto the card's /FIRMWARE/)",
        None => "",
    };
    serial_println!(
        ":: WIFI5: rungs={} confirmed={} refuted={} open={} parked={} upload_unwind={} staged={}/3 {} uploaded_this_boot={} -> {}{} ::",
        LEDGER.len(), c, r, o, p, unwind, staged, fw, uploaded as u8,
        if reason.is_none() { "READY" } else { "NOT-READY" },
        reason.map(|s| alloc::format!(" reason={}{}", s, hint)).unwrap_or_default()
    );
}
