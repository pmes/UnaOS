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
         host-tested. WPA2-PSK is S10 on the MT7925 path (`tests wifijoin <ssid> <passphrase>`, CCMP keys held by the radio); \
         on the BCM4331 path WPA2 is unbuilt (WPA2JOIN owed): an open BSS only. Not attempted here.",
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
        let waited = stack_dhcp(WAIT_MS); // MT7925S9 (Z46): the smoltcp drive is the ONE `stack_dhcp` (this file's tail), shared with `wifijoin`
        if !super::data::bound() {
            return absent("stack-on-other-link");
        }
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

// ── MT7925S8 (zenbook-ledger Z38): `tests wifijoin <ssid>` — the station's open-system join over the MT7925 (S8) ─────────────
/// A printable SSID for a wire line: non-printables and quotes as '.', at most 32 (the wifiscan rule; SSID-TEXT-SHARED owed).
fn ssid_view(b: &[u8]) -> alloc::string::String {
    b.iter().take(32).map(|&c| if (0x20..0x7f).contains(&c) && c != b'"' { c as char } else { '.' }).collect()
}

/// `tests wifijoin <ssid>` (MT7925S8, zenbook-ledger Z38): the operator's join, never at boot (BP-net-7), the SSID from the same
/// `crate::tests::arg()` the Broadcom `tests wifi <ssid>` reads. The KATs first (`mt7925::join_kat`: the TXD/txp, DEV_INFO,
/// BSS_INFO and ROC bytes; `assoc::kat`: the ONE codec's auth/assoc frames and the judges over canned frames); with no MT7925
/// registered (QEMU, the rMBP) that is the verdict and nothing touches a device. Else `mt7925::scan` hands each frame here and the
/// first OPEN BSS naming exactly the SSID with a DS channel is the target (a protected one only sets a flag: WPA2 is S10); then
/// `mt7925::join` takes the frames from `wifi_core::ieee80211` (`build_auth_open`, `build_assoc_req` with `assoc::assoc_cap`) and
/// judges the replies with `assoc::auth_reply` / `assoc_reply` / `refusal`. PASS = the KATs held and the association was granted.
pub fn wifijoin() {
    use crate::drivers::mt7925 as mt;
    use wifi_core::ieee80211 as w;
    let s9w = mt::watch_kat(super::assoc::refusal); let s9r = mt::rejoin_kat(); let kat = mt::join_kat() && super::assoc::kat().2.is_none() && super::assoc::wpa2_kat().iter().all(|&k| k) && s9w && s9r; // S9REJOIN (zenbook-ledger Z59): `s9r`, the rejoin's backoff sequence over a stub attempt and the unlink bodies; S9WATCH (zenbook-ledger Z56): the S9 link watch's rows with the station's own judge (LINE-NEUTRAL)
    let k = if kat { "ok" } else { "FAIL" };
    if !mt::registered() {
        serial_println!(":: wifijoin: kat={} s9watch={} s9rejoin={} radio=absent -> {} ::", k, if s9w { "ok" } else { "FAIL" }, if s9r { "ok" } else { "FAIL" }, if kat { "PASS" } else { "FAIL" }); // S9WATCH (Z56): s9watch=; S9REJOIN (Z59): s9rejoin=
        return;
    }
    mt::rejoin_keep(None); let ssid = crate::tests::arg().unwrap_or_default(); // S9REJOIN (zenbook-ledger Z59): the operator's join forgets the kept plan (a successful open join keeps its own)
    let refuse = |why: &str| {
        serial_println!("[wifi] join radio=mt7925 ssid=\"{}\" -> NOT ATTEMPTED why={}", ssid_view(ssid.as_bytes()), why);
        serial_println!(":: wifijoin: kat={} radio=mt7925 -> FAIL why={} ::", k, why);
    };
    if ssid.is_empty() {
        return refuse("no-ssid(type tests wifijoin <ssid>)");
    }
    if ssid.len() > 32 {
        return refuse("ssid-too-long");
    }
    let Some(me) = mt::mac() else { return refuse("no-mac") };
    let sb = ssid.as_bytes();
    let (mut best, mut prot): (Option<super::assoc::Bss>, bool) = (None, false); let mut pass = crate::tests::arg2().unwrap_or_default().into_bytes(); let mut rsn: Option<alloc::vec::Vec<u8>> = None;
    let _ = mt::scan(&mut |f: &[u8]| -> bool {
        let Some(b) = super::assoc::bss_match(f, sb) else { return false };
        if b.protected && (pass.is_empty() || !w::rsn_ie(f).map_or(false, w::rsn_psk_ccmp)) {
            prot = true;
        } else if best.is_none() && b.channel.is_some() {
            best = Some(b); rsn = if b.protected { w::rsn_ie(f).map(|e| e.to_vec()) } else { None };
            return true;
        }
        false
    });
    let Some(bss) = best else { return refuse(if !prot { "not-heard" } else if pass.is_empty() { "protected-no-psk" } else { "rsn-unsupported" }) };
    let (bssid, ch) = (bss.bssid, bss.channel.unwrap_or(0)); let pk = rsn.as_ref().map(|_| super::assoc::wpa_pmk(sb, &pass)); crypto_core::ct::Zeroize::zeroize(&mut pass[..]);
    let cap = super::assoc::assoc_cap(&bss); let pmk = match pk { Some(Err(e)) => return refuse(e), Some(Ok(k)) => Some(crypto_core::ct::SecretBytes(k)), None => None };
    let mt_ = |m: [u8; 6]| alloc::format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5]);
    let mut seen = Seen::default(); // S9REJOIN (zenbook-ledger Z59): the frames and the judge hoisted into the ONE `join_frames` (the rejoin runs it too)
    let plan = mt::JoinPlan { ssid: &ssid_view(sb), bssid, channel: ch, watch: super::assoc::refusal }; // S9WATCH (zenbook-ledger Z56): the station's refusal judge kept by the S9 link
    let rep = join_frames(&plan, me, sb, cap, pmk.is_some(), &mut seen);
    let Seen { auth, assoc, refused } = seen;
    let rep = match rep {
        Ok(r) => r,
        Err(e) => {
            serial_println!("[wifi] join radio=mt7925 ssid=\"{}\" -> NOT ATTEMPTED why={}", plan.ssid, e);
            serial_println!(":: wifijoin: kat={} radio=mt7925 -> FAIL ::", k);
            return;
        }
    };
    let why = |ph: u8, st: &mt::Step, status: Option<u16>| -> alloc::string::String {
        match st {
            mt::Step::Ans(mt::Answer::Refused(c)) => match refused {
                Some((p, what, reason)) if p == ph => alloc::format!("{}(reason={})", what, reason),
                _ => alloc::format!("status={}", status.unwrap_or(*c)),
            },
            mt::Step::Tx(t) => alloc::format!("tx:{}", t),
            _ => alloc::string::String::from("no-response"),
        }
    };
    let authed = matches!(rep.auth, mt::Step::Ans(mt::Answer::Ok(_)));
    let resp = auth.map_or(alloc::string::String::from("alg=- seq=- status=-"), |a| alloc::format!("alg={} seq={} status={}", a.alg, a.seq, a.status));
    serial_println!(
        ":: wifiauth: s8 auth ssid={} bssid={} ch={} radio=mt7925 resp={} -> {} ::",
        plan.ssid, mt_(bssid), ch, resp,
        if authed { alloc::string::String::from("AUTHENTICATED") } else { alloc::format!("NOT-AUTHENTICATED reason={}", why(0, &rep.auth, auth.map(|a| a.status))) }
    );
    let aid = match rep.assoc {
        mt::Step::Ans(mt::Answer::Ok(a)) => Some(a),
        _ => None,
    };
    if !authed {
        serial_println!(":: wifiauth: s9 assoc NOT ATTEMPTED reason=s8-NOT-AUTHENTICATED -> ABSENT ::");
    } else {
        let resp = assoc.map_or(alloc::string::String::from("status=- aid=-"), |a| alloc::format!("status={} aid={}", a.status, if a.status == 0 { alloc::format!("{}", a.aid) } else { alloc::string::String::from("-") }));
        serial_println!(
            ":: wifiauth: s9 assoc ssid={} bssid={} cap={:#06x} radio=mt7925 resp={} -> {} ::",
            plan.ssid, mt_(bssid), cap, resp,
            if aid.is_some() { alloc::string::String::from("ASSOCIATED") } else { alloc::format!("NOT-ASSOCIATED reason={}", why(1, &rep.assoc, assoc.map(|a| a.status))) }
        );
    }
    serial_println!(
        ":: wifijoin: kat={} s9watch={} s9rejoin={} radio=mt7925 ssid={} auth={} assoc={} -> {} ::", // S9WATCH (Z56): s9watch=; S9REJOIN (Z59): s9rejoin=
        k, if s9w { "ok" } else { "FAIL" }, if s9r { "ok" } else { "FAIL" }, plan.ssid, if authed { "ok" } else { "no" }, aid.map_or(alloc::string::String::from("-"), |a| alloc::format!("aid={}", a)),
        if kat && aid.is_some() { "PASS" } else { "FAIL" }
    );
    if aid.is_some() {
        let open = rsn.is_none(); if s10_wpa2(me, bssid, ch, &ssid, rsn, pmk) { if open { rejoin_keep(sb, bssid, ch, cap); } s9_dhcp(); } // S9REJOIN (zenbook-ledger Z59): an OPEN join's plan kept for the automatic rejoin (a WPA2 one needs S10-PMK-KEEP).  MT7925S9 (zenbook-ledger Z46): smolnet's one DHCP client over the S9 link, then the link's counts
    }
}

// ── MT7925S9 (zenbook-ledger Z46): the station's DHCP over the MT7925 link (S9) ─────────────────────────────────────────────
/// The ONE smoltcp DHCP drive (hoisted out of `wifidhcp`): smolnet's own stack and its ONE client (`smolnet::init` ->
/// `dhcp_acquire`) under the net-tick DHCP scope, then up to `wait_ms` of `service_poll` until a lease (no lock held while it
/// sleeps). Returns the ms waited.
#[cfg(feature = "smolnet")]
fn stack_dhcp(wait_ms: u64) -> u64 {
    let t0 = crate::arch::now_cycles();
    let hz = crate::bootpace::origin_hz().max(1);
    let ms = || crate::arch::now_cycles().wrapping_sub(t0) / (hz / 1000).max(1);
    {
        let _s = crate::net_tick::scope(crate::net_tick::Who::Dhcp);
        let _ = crate::smolnet::init();
        while !crate::smolnet::leased() && ms() < wait_ms {
            crate::smolnet::service_poll();
            crate::arch::sched::sleep_ms(5);
        }
    }
    ms()
}

/// A dotted quad for a wire line, `-` for 0.0.0.0 (hoisted out of `wifidhcp`).
#[cfg(feature = "smolnet")]
fn ip4(o: [u8; 4]) -> alloc::string::String {
    if o == [0; 4] { alloc::string::String::from("-") } else { alloc::format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]) }
}

/// After `tests wifijoin` read ASSOCIATED (MT7925S9, zenbook-ledger Z46): with the S9 link up (`mt7925::linked`) and the Desktop
/// services gate open (BP-net-4), smolnet's stack is built over it (`drivers/e1000.rs`'s accessor triple reaches the link when no
/// e1000 and no USB Ethernet answered) and its ONE DHCP client asks for a lease (`stack_dhcp`); then the driver's link line.
fn s9_dhcp() {
    use crate::drivers::mt7925 as mt;
    let absent = |why: &str| serial_println!(":: wifijoin: s9 dhcp=NOT ATTEMPTED reason={} -> ABSENT ::", why);
    if !mt::linked() {
        absent("no-link");
        return mt::say_link();
    }
    #[cfg(not(feature = "smolnet"))]
    {
        absent("no-stack(smolnet off)");
        return mt::say_link();
    }
    #[cfg(feature = "smolnet")]
    {
        const WAIT_MS: u64 = 12_000;
        if !crate::boot::services_gate("net") {
            absent("services-gate-closed");
            return mt::say_link();
        }
        let waited = stack_dhcp(WAIT_MS);
        if !mt::bound() {
            absent("stack-on-other-link");
            return mt::say_link();
        }
        match crate::smolnet::lease_ip() {
            Some(ip) => {
                let (gw, dns) = crate::smolnet::route_view();
                serial_println!(
                    ":: wifijoin: s9 dhcp=bound ip={} gw={} dns={} lease={} waited={}ms -> PASS ::",
                    ip4(ip), ip4(gw), ip4(dns), crate::smolnet::lease_secs().map_or(alloc::string::String::from("unread"), |s| alloc::format!("{}", s)), waited
                );
            }
            None => serial_println!(":: wifijoin: s9 dhcp=none ip=- gw=- dns=- lease=- waited={}ms -> SILENT ::", waited),
        }
        mt::say_link();
    }
}

// ── MT7925S10 (zenbook-ledger Z55): the WPA2-PSK handshake over the MT7925 link (S10) ───────────────────────────────────────────
/// After `tests wifijoin` read ASSOCIATED to a WPA2-PSK BSS: the supplicant's side of the 4-way handshake (IEEE Std 802.11-2020
/// §12.7.6; the pure half is `assoc::Handshake`). An open BSS (`rsn`/`pmk` None) returns true with no line. Else: a fresh SNonce from
/// the kernel's DRBG; up to 2 s for message 1 from `mt::eapol_rx` (the link's EAPOL mailbox); message 2 out through `mt::eapol_tx`;
/// up to 2 s for message 3 (a repeated message 1 is answered with message 2 again, counted `replay=`); message 4 out and its fetch
/// awaited BEFORE the keys are handed to the radio (`mt::set_keys`: linux and wpa_supplicant's order — the AP must see message 4
/// under the old, unencrypted state); one verdict line. True = SECURED, so the caller runs its DHCP; false = refused, said by name,
/// no DHCP. The PMK, the PTK and the keys are wiped on every exit (their owners' Drop); no key byte is printed.
fn s10_wpa2(me: [u8; 6], bssid: [u8; 6], ch: u8, ssid: &str, rsn: Option<alloc::vec::Vec<u8>>, pmk: Option<crypto_core::ct::SecretBytes<32>>) -> bool {
    use super::assoc::{eapol_eth, eapol_payload, Handshake, Why};
    use crate::drivers::mt7925 as mt;
    let (Some(rsn), Some(pmk)) = (rsn, pmk) else { return true };
    let refuse = |why: &str| -> bool {
        serial_println!("[mt7925] s10 wpa2 -> REFUSED why={}", why);
        serial_println!(":: wifijoin: s10 wpa2 -> FAIL why={} ::", why);
        false
    };
    if !mt::linked() {
        return refuse("no-link");
    }
    let mut sn = [0u8; 32];
    crate::rand::drbg_fill(&mut sn);
    let mut hs = Handshake::new(bssid, me, sn, &rsn, &pmk.0);
    let (c0, t0) = (mt::eapol_counts(), crate::arch::ms());
    let ms = || crate::arch::ms().saturating_sub(t0);
    let next = |deadline: u64| -> Option<alloc::vec::Vec<u8>> {
        loop {
            if let Some(eth) = mt::eapol_rx() {
                if let Some(p) = eapol_payload(&eth, &me, &bssid) {
                    return Some(p.to_vec());
                }
            }
            if ms() >= deadline {
                return None;
            }
            crate::arch::sched::sleep_ms(1);
        }
    };
    let Some(m1) = next(ms() + 2000) else { return refuse("timeout(step=1)") };
    let t1 = ms();
    let m2 = match hs.on_m1(&m1) {
        Ok(m) => m,
        Err(w) => return refuse(w.name()),
    };
    if !mt::eapol_tx(&eapol_eth(&me, &bssid, &m2)) {
        return refuse("tx(step=2)");
    }
    let (mut again, mut dl) = (0u32, ms() + 2000);
    let (m4, keys) = loop {
        let Some(f) = next(dl) else { return refuse("timeout(step=3)") };
        match hs.on_m3(&f) {
            Ok(r) => break r,
            Err(Why::Frame) => {
                if let Ok(m2) = hs.on_m1(&f) {
                    again += 1;
                    dl = ms() + 2000;
                    if !mt::eapol_tx(&eapol_eth(&me, &bssid, &m2)) {
                        return refuse("tx(step=2)");
                    }
                }
            }
            Err(w) => return refuse(w.name()),
        }
    };
    let t3 = ms();
    if !mt::eapol_tx(&eapol_eth(&me, &bssid, &m4)) {
        return refuse("tx(step=4)");
    }
    let c1 = mt::eapol_counts();
    serial_println!(
        "[mt7925] s10 wpa2 m1={} m3={} replay={} eapol={} other={} -> keys",
        t1, t3, again, c1.0.wrapping_sub(c0.0), c1.1.wrapping_sub(c0.1)
    );
    if let Err(e) = mt::set_keys(bssid, ch, &keys.tk, &keys.gtk, keys.gtk_id) {
        return refuse(&alloc::format!("set-key({})", e));
    }
    serial_println!(":: wifijoin: s10 wpa2 ssid={} -> SECURED ::", ssid_view(ssid.as_bytes()));
    true
}

/// `tests wpa2` (MT7925S10, zenbook-ledger Z55): the WPA2-PSK known-answer legs, no radio needed — `assoc::wpa2_kat`'s five (PBKDF2
/// Annex J.4, the PRF Annex J.3 and a pinned PTK, the EAPOL-Key MIC, the RFC 3394 unwrap, the message frames) and `mt7925::s10_kat`'s
/// key-command bodies. One verdict line.
pub fn wpa2() {
    let a = super::assoc::wpa2_kat();
    let keys = crate::drivers::mt7925::s10_kat();
    let ok = |b: bool| if b { "ok" } else { "FAIL" };
    serial_println!(
        ":: WPA2KAT: pbkdf2={} prf={} mic={} unwrap={} frame={} keys={} -> {} ::",
        ok(a[0]), ok(a[1]), ok(a[2]), ok(a[3]), ok(a[4]), ok(keys), if a.iter().all(|&b| b) && keys { "PASS" } else { "FAIL" }
    );
}

// ── S9REJOIN (zenbook-ledger Z59): the station's half of the MT7925 rejoin — ONE join_frames, the kept plan, the attempt ─────────
/// What the judge heard during one join: the authentication and association answers and a deauth / disassoc (phase, kind, reason).
#[derive(Default)]
struct Seen {
    auth: Option<wifi_core::ieee80211::AuthResult>,
    assoc: Option<wifi_core::ieee80211::AssocResult>,
    refused: Option<(u8, &'static str, u16)>,
}

/// The station's frames and judge for ONE join over the MT7925 (hoisted unchanged out of `wifijoin`; the operator's join and the
/// automatic rejoin both run it): `build(0)` the open-system authentication request, `build(1)` the association request (with
/// S10's RSN element when `wpa`); the judge answers with `assoc::auth_reply` / `assoc_reply` / `refusal`, recording into `seen`.
fn join_frames(
    plan: &crate::drivers::mt7925::JoinPlan, me: [u8; 6], sb: &[u8], cap: u16, wpa: bool, seen: &mut Seen,
) -> Result<crate::drivers::mt7925::JoinRep, &'static str> {
    use crate::drivers::mt7925 as mt;
    use wifi_core::ieee80211 as w;
    let bssid = plan.bssid;
    mt::join(
        plan,
        &mut |phase: u8| if phase == 0 { w::build_auth_open(me, bssid, 0) } else { let mut f = w::build_assoc_req(me, bssid, sb, 0, cap); if wpa { f.extend_from_slice(&w::RSN_PSK_CCMP); } f },
        &mut |phase: u8, f: &[u8]| {
            if phase == 0 {
                if let Some(a) = super::assoc::auth_reply(f, me, bssid) {
                    seen.auth = Some(a);
                    let good = a.alg == 0 && a.seq == 2 && a.status == 0;
                    return Some(if good { mt::Answer::Ok(0) } else { mt::Answer::Refused(if a.status != 0 { a.status } else { 0xffff }) });
                }
            } else if let Some(a) = super::assoc::assoc_reply(f, me, bssid) {
                seen.assoc = Some(a);
                return Some(if a.status == 0 { mt::Answer::Ok(a.aid) } else { mt::Answer::Refused(a.status) });
            }
            if let Some((what, reason)) = super::assoc::refusal(f, me, bssid) {
                seen.refused = Some((phase, what, reason));
                return Some(mt::Answer::Refused(reason));
            }
            None
        },
    )
}

/// After a successful OPEN `tests wifijoin` (the S9 link up): the plan kept in the driver for the automatic rejoin — the SSID bytes,
/// the BSS, its channel and capability, the station's judge, `rejoin_again` and `s9_dhcp`.
fn rejoin_keep(sb: &[u8], bssid: [u8; 6], ch: u8, cap: u16) {
    use crate::drivers::mt7925 as mt;
    if !mt::linked() {
        return;
    }
    let mut ssid = [0u8; 32];
    let n = sb.len().min(32);
    ssid[..n].copy_from_slice(&sb[..n]);
    mt::rejoin_keep(Some(mt::RejoinPlan {
        ssid, ssid_len: n as u8, bssid, channel: ch, cap, watch: super::assoc::refusal, again: rejoin_again, dhcp: s9_dhcp,
    }));
}

/// ONE rejoin attempt from the driver's `mt7925-rejoin` task: the kept plan through the same `join_frames` -> `mt7925::join` (open:
/// no RSN element), no scan (the ROC tunes to the kept channel). Err `auth` / `assoc` / `link` (associated, the S9 link refused) or
/// the join's own refusal word; the driver prints the verdict.
fn rejoin_again(p: &crate::drivers::mt7925::RejoinPlan) -> Result<(), &'static str> {
    use crate::drivers::mt7925 as mt;
    let Some(me) = mt::mac() else { return Err("no-mac") };
    let sb = &p.ssid[..(p.ssid_len as usize).min(32)];
    let plan = mt::JoinPlan { ssid: &ssid_view(sb), bssid: p.bssid, channel: p.channel, watch: p.watch };
    let mut seen = Seen::default();
    let rep = join_frames(&plan, me, sb, p.cap, false, &mut seen)?;
    if !matches!(rep.auth, mt::Step::Ans(mt::Answer::Ok(_))) {
        return Err("auth");
    }
    if !matches!(rep.assoc, mt::Step::Ans(mt::Answer::Ok(_))) {
        return Err("assoc");
    }
    if mt::linked() { Ok(()) } else { Err("link") }
}
