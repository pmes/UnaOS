// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI1 — the `wifi` shell verb (`wifi` · `wifi up` · `wifi scan` · `wifi join <ssid>`).
//!
//! CHARTER: Kernel — kernel-by-ruling. One HOST verb (only the ring holds the radio and the bring-up
//! ladder). It REPORTS the recorded verdict of the bring-up (`status.rs`) and names, honestly and by
//! reason, every device leg this tree carries no legal source to build. It never re-runs the
//! destructive upload — that is the `UNAOS_WIFI3=1` boot, Peter's to schedule (R80: nothing runs at
//! boot but the boot). The beacon/auth/assoc codec is real and host-tested (`wifi_core::ieee80211`);
//! the device legs that drive it (S5c tune, S6 receiver, S7 TX ring, S8 auth, S9 assoc) run only inside
//! `tests wifi [<ssid>]` on `wifi9` / `wifi10` images (`bcm4331.md` §7), never from this verb.

use crate::console::Console;
use super::status;

/// `wifi [up|scan|join <ssid>|status]`. Default and `status` print the recorded verdict.
pub fn shell_verb(args: &[&str], console: &mut Console) {
    match args.first().copied() {
        None | Some("status") => report(console),
        Some("up") => up(console),
        Some("scan") => scan(console),
        Some("join") => join(args.get(1).copied(), console),
        Some(other) => console.println(&alloc::format!(
            "wifi: unknown subcommand '{}' — try: wifi [status|up|scan|join <ssid>]",
            other
        )),
    }
}

fn radio_present() -> bool {
    super::bus::found().map(|c| c.cross_check_ok).unwrap_or(false)
}

fn report(console: &mut Console) {
    if super::bus::found().is_none() {
        console.println("wifi: no BCM4331 radio selected (census did not run, or found none) — see the :: wifi: … :: boot lines");
        return;
    }
    if !radio_present() {
        console.println("wifi: a Broadcom network function was seen but the identity cross-check FAILED (bcm4331.md §0) — the bring-up is gated on it");
        return;
    }
    let (rev, date) = (status::ucode_rev(), status::ucode_date());
    let (y, mo, d) = wifi_core::fw::ucode_date(date);
    console.println(&alloc::format!(
        "wifi: radio=BCM4331 present  ucode={}  d11={}  initvals-wrote={}  scan={}",
        if status::ucode_ok() { "ok" } else { "not-loaded" },
        if status::d11_up() { "up" } else { "down" },
        status::initvals_wrote(),
        status::scan_count().map(|n| alloc::format!("{}", n)).unwrap_or_else(|| "none-yet".into()),
    ));
    if status::ucode_ok() {
        console.println(&alloc::format!(
            "wifi: microcode rev={} built {:04}-{:02}-{:02} (self-published by the running PSM)",
            rev, y, mo, d
        ));
    }
}

fn up(console: &mut Console) {
    if !radio_present() {
        return report(console);
    }
    report(console);
    console.println(
        "wifi: the firmware upload + d11 init is the bring-up ladder (bcm4331.md §S4-W5) and it is \
         DESTRUCTIVE — it resets a radio that arrives running the platform firmware. It runs at boot \
         only under UNAOS_WIFI3=1 (R80: nothing runs at boot but the boot), which is Peter's to \
         schedule; `wifi up` does not re-run it from the shell.",
    );
    console.println(
        "wifi: firmware is user-supplied (CLEAN_ROOM_POLICY §4, R52) — place ucode29_mimo.fw, \
         ht0initvals29.fw and ht0bsinitvals29.fw under /system/firmware/bcm4331/ (or /B43/ on the \
         program-source volume). Extract them with b43-fwcutter from broadcom-wl-5.100.138.",
    );
}

fn scan(console: &mut Console) {
    if !radio_present() {
        return report(console);
    }
    status::set_scan(0);
    console.println(
        "wifi: scan — the beacon/probe-response parser is live and host-tested (wifi_core::ieee80211), \
         and on wifi9 images the receive path is built (the S5c HT-PHY channel tune over htphy_tables.rs, \
         the S6 d11 DMA RX rings), but it runs only inside `tests wifi` (bcm4331.md §7); this verb drives \
         none of it. scan=0, nothing heard from this verb.",
    );
    console.println(&alloc::format!(
        "wifi: scan=0 (d11={}, ucode={})",
        if status::d11_up() { "up" } else { "down" },
        if status::ucode_ok() { "ok" } else { "not-loaded" },
    ));
}

fn join(ssid: Option<&str>, console: &mut Console) {
    let Some(ssid) = ssid else {
        return console.println("wifi: join needs an SSID — `wifi join <ssid>`");
    };
    if !radio_present() {
        return report(console);
    }
    // The join is not this verb's: rungs S8 (open-system auth) and S9 (association) run once per boot in
    // `bringup::wifi10` from `tests wifi <ssid>` (bringup/join.rs, WIFIAUTH B788), over WIFI10's TX ring
    // and the SPROM station MAC. This verb transmits nothing (R80); it names that door, by reason.
    console.println(&alloc::format!(
        "wifi: join '{}' — this verb transmits nothing. Open-system authentication (S8) and \
         association (S9) are built and run once per boot from `tests wifi <ssid>` on a wifi10 image \
         (wifi_core::ieee80211::build_auth_open / build_assoc_req over the WIFI10 TX ring, the replies \
         judged by wifi::assoc), no capture yet; the data-frame <-> Ethernet path for smoltcp is \
         host-tested. WPA2 is S10, unbuilt, owed to CRYPTOCORE (PBKDF2-HMAC-SHA1, HMAC-SHA1, \
         AES-CCMP): an open BSS only. Not attempted here.",
        ssid
    ));
}

/// `tests wifi` — the fixture. One verdict line in the brief's shape.
pub fn selftest() {
    // WIFI5 (B415): the ladder's state as a line, before the WIFI1 verdict (which returns early).
    #[cfg(feature = "wifi9")] super::bringup::wifi9(); #[cfg(feature = "wifi10")] super::bringup::wifi10(crate::tests::arg()); #[cfg(feature = "wifi10")] super::assoc::witness(); super::ladder::witness(); // WIFI9 (B625): S5i/S5c/S6 run first, so WIFI5's live rows and WIFI1's scan= read this boot's verdicts (LINE-NEUTRAL append, before this comment)
    let present = radio_present();
    let ucode = if status::ucode_ok() { "ok" } else { "refused" };
    let d11 = if status::d11_up() { "up" } else { "down" };
    let scan = status::scan_count().unwrap_or(0);
    if !present {
        serial_println!(
            ":: WIFI1: radio ABSENT or cross-check FAILED — no bring-up to verify -> SKIP ::"
        );
        return;
    }
    if !status::ucode_ok() {
        // The firmware is user-supplied; without the UNAOS_WIFI3 upload boot there is nothing to pass.
        serial_println!(
            ":: WIFI1: ucode=refused d11={} scan={} -> SKIP (no firmware uploaded this boot; the upload is the UNAOS_WIFI3=1 boot) ::",
            d11, scan
        );
        return;
    }
    let pass = status::ucode_ok() && status::d11_up();
    serial_println!(
        ":: WIFI1: ucode={} d11={} scan={} -> {} ::",
        ucode, d11, scan, if pass { "PASS" } else { "FAIL" }
    );
}
