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
    #[cfg(feature = "wifi9")] super::bringup::wifi9(); #[cfg(feature = "wifi10")] super::bringup::wifi10(crate::tests::arg(), super::data::ipv4(crate::tests::arg2().as_deref())); #[cfg(feature = "wifi10")] super::assoc::witness(); #[cfg(feature = "wifi10")] super::data::witness(); #[cfg(feature = "wifi10")] wifidhcp(); super::ladder::witness(); // WIFI9 (B625): S5i/S5c/S6 run first, so WIFI5's live rows and WIFI1's scan= read this boot's verdicts (LINE-NEUTRAL append, before this comment)
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

/// WIFIDHCP (rmbp-ledger B831) — rung S10b, from `tests wifi` after S10a's witness (R80: nothing at boot). The KAT
/// line (`data::kat_dhcp`: the 342-octet DISCOVER fits the ring, the Link stub carries it both ways), then, when
/// S10a kept the station (`data::kept`) and the Desktop services gate is open (BP-net-4): smolnet's own stack and
/// its ONE DHCP client (`smolnet::init` -> `dhcp_acquire`, the same client every link uses) over the station,
/// which `drivers/e1000.rs`'s accessor triple reaches as the fallback after e1000 and usbnet. Waits up to
/// `WAIT_MS` for the lease (`service_poll` applies a late one), no lock held while it sleeps.
#[cfg(feature = "wifi10")]
fn wifidhcp() {
    let (kat, stub, len, first) = super::data::kat_dhcp();
    serial_println!(
        ":: wifidhcp: kat={} nic={} discover_len={} -> {}{} ::",
        if kat { "ok" } else { "FAIL" }, if stub { "stub" } else { "stub-FAIL" }, len,
        if first.is_none() { "PASS" } else { "FAIL" }, first.map(|f| alloc::format!(" first={}", f)).unwrap_or_default()
    );
    let absent = |why: &str| {
        super::data::set_dhcp(3);
        serial_println!(":: WIFIDHCP: dhcp=NOT ATTEMPTED reason={} -> ABSENT ::", why);
    };
    if !super::data::kept() {
        return absent("no-station-link");
    }
    #[cfg(not(feature = "smolnet"))]
    return absent("no-stack(smolnet off)");
    #[cfg(feature = "smolnet")]
    {
        const WAIT_MS: u64 = 12_000;
        if !crate::boot::services_gate("net") {
            return absent("services-gate-closed");
        }
        let t0 = crate::arch::now_cycles();
        let hz = crate::bootpace::origin_hz().max(1);
        let ms = || crate::arch::now_cycles().wrapping_sub(t0) / (hz / 1000).max(1);
        {
            let _s = crate::net_tick::scope(crate::net_tick::Who::Dhcp);
            let _ = crate::smolnet::init();
            while !crate::smolnet::leased() && ms() < WAIT_MS {
                crate::smolnet::service_poll();
                crate::arch::sched::sleep_ms(5);
            }
        }
        if !super::data::bound() {
            return absent("stack-on-other-link");
        }
        let ip4 = |o: [u8; 4]| if o == [0; 4] { alloc::string::String::from("-") } else { alloc::format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]) };
        let waited = ms();
        match crate::smolnet::lease_ip() {
            Some(ip) => {
                let (gw, dns) = crate::smolnet::route_view();
                super::data::set_dhcp(1);
                serial_println!(
                    ":: WIFIDHCP: dhcp=bound ip={} gw={} dns={} lease={} waited={}ms -> PASS ::",
                    ip4(ip), ip4(gw), ip4(dns), crate::smolnet::lease_secs().map_or(alloc::string::String::from("unread"), |s| alloc::format!("{}", s)), waited
                );
            }
            None => {
                super::data::set_dhcp(2);
                serial_println!(":: WIFIDHCP: dhcp=none ip=- gw=- dns=- lease=- waited={}ms -> SILENT ::", waited);
            }
        }
    }
}

// ── MT7925S7 (zenbook-ledger Z34): `tests wifiscan` — the station's scan over the MT7925 (S7) ────────────────────────────
/// The most BSSes one scan lists (each once, on its own `[wifi] scan bss=` line).
const SCAN_LIST_MAX: usize = 32;

/// `tests wifiscan` (MT7925S7, zenbook-ledger Z34): the operator's scan, never at boot (BP-net-7). The driver's KAT first
/// (`mt7925::scan_kat`, hand-assembled request and RX descriptors); with no MT7925 registered (QEMU, the rMBP) that is the
/// verdict and nothing touches a device. Else `mt7925::scan` hands each received beacon / probe response here raw: parsed by
/// the ONE codec (`wifi_core::ieee80211::parse_beacon`), counted, each new BSSID listed once (bounded `SCAN_LIST_MAX`; the
/// dedup array lives on this frame and is dropped at return, no list is kept), and the beacon count recorded in the
/// station's scan record (`status::set_scan`, the `scan=` of `wifi` and `tests wifi`). PASS = the KAT held and at least one
/// BSS was heard.
pub fn wifiscan() {
    let kat = crate::drivers::mt7925::scan_kat();
    let k = if kat { "ok" } else { "FAIL" };
    if !crate::drivers::mt7925::registered() {
        serial_println!(":: wifiscan: kat={} radio=absent -> {} ::", k, if kat { "PASS" } else { "FAIL" });
        return;
    }
    let mut seen = [[0u8; 6]; SCAN_LIST_MAX];
    let (mut n, mut beacons, mut frames) = (0usize, 0u32, 0u32);
    let _ = crate::drivers::mt7925::scan(&mut |f: &[u8]| -> bool {
        frames += 1;
        let Some(b) = wifi_core::ieee80211::parse_beacon(f) else { return false };
        beacons += 1;
        if n >= SCAN_LIST_MAX || seen[..n].contains(&b.bssid) {
            return false;
        }
        seen[n] = b.bssid;
        n += 1;
        let ssid: alloc::string::String =
            b.ssid.iter().take(32).map(|&c| if (0x20..0x7f).contains(&c) && c != b'"' { c as char } else { '.' }).collect();
        let ch = b.channel.map_or(alloc::string::String::from("-"), |c| alloc::format!("{}", c));
        let m = b.bssid;
        serial_println!(
            "[wifi] scan bss={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} ch={} sec={} ssid=\"{}\"",
            m[0], m[1], m[2], m[3], m[4], m[5], ch, if b.protected() { "protected" } else { "open" }, ssid
        );
        true
    });
    status::set_scan(beacons);
    serial_println!(
        "[wifi] scan radio=mt7925 frames={} beacons={} bss={} -> {}",
        frames, beacons, n, if n > 0 { "listed" } else { "empty" }
    );
    serial_println!(":: wifiscan: kat={} radio=mt7925 bss={} -> {} ::", k, n, if kat && n > 0 { "PASS" } else { "FAIL" });
}
