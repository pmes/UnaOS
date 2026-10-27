// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (rungs S8/S9, the station's open-system join over WIFI10's TX ring and S6's
//! receiver; no CODEX §2 handler owns a radio register).
//!
//! WIFIAUTH (rmbp-ledger B788) — the device half of S8 (Open System authentication, IEEE Std 802.11-2020
//! §9.3.3.11, §12.3.3.2) and S9 (association, §9.3.3.5/§9.3.3.6), state 1 -> 2 -> 3 of §11.3.1. Runs inside
//! `bringup::wifi10()`'s once-per-boot pass from `tests wifi <ssid>` only (R80, BP-net-7: nothing transmits at
//! boot), after the probe window, while the TX ring (`tx::prep`) and the receiver (`rx::arm`) are armed.
//! The frames are `wifi_core::ieee80211`'s; the judges are `wifi::assoc`'s. One attempt per rung (the ucode
//! retries per frame up to the long limit `tx::retry_limits` wrote); each answer waits at most `REPLY_MS`.

use super::super::assoc::{self, Bss};
use super::{rx, tx};
use alloc::string::String;
use alloc::vec::Vec;
use wifi_core::ieee80211::Mac;

/// The bounded wait for the AP's answer to one frame, polled in `SLICE_US` slices.
const REPLY_MS: u32 = 500;
const SLICE_US: u64 = 25_000;
/// Management frames addressed to us kept between polls (beacons / probe responses are not kept).
const KEEP: usize = 4;
/// Frame numbers on the boot's TX ring: 0 is S7's probe request.
const FRAME_AUTH: usize = 1;
const FRAME_ASSOC: usize = 2;

/// What the receiver heard for the join: the target BSS by SSID and the frames addressed to us.
#[derive(Default)]
pub(super) struct Heard {
    pub me: Mac,
    /// The SSID the operator named (`tests wifi <ssid>`); empty = no join this boot.
    pub target: Vec<u8>,
    pub bss: Option<Bss>,
    pub frames: Vec<Vec<u8>>,
    /// STATIONWATCH (B843): the kept station's link watch, armed by `keep`; while armed every management frame feeds it.
    pub watch: Option<super::super::data::Watch>,
    /// STATIONRATE (B855): the rate set S9's ASSOCIATED answer listed (`None` = unread); `keep` picks the station's rate from it.
    pub rates: Option<wifi_core::ieee80211::RateSet>,
    /// STATIONSHORTPRE (B862): S9's ASSOCIATED answer set the capability Short Preamble bit (`assoc::short_preamble`).
    pub short: bool, pub gw: Option<[u8; 4]>, pub ring: usize, pub scan: super::super::data::BssTable, pub pin: Option<Mac>, pub home: Option<Mac>, pub gw_mac: Option<Mac>, pub resp: u16, // STATIONROAM-ACTIVE (B951): `resp`, the DS channels (bit c; bit 0 = no DS element) of the SSID's probe responses to us noted in this dwell; STATIONROAM (B897): the scan table, the BSSID a join is pinned to, the BSS last linked, the gateway MAC S10a's ARP reply named; STATIONREJOIN (B891): the gateway S10a ARPed (for `associate`'s S10a) and the rungs' ring base (0 on the boot's pass)
}

impl Heard {
    pub(super) fn new(me: Mac, ssid: Option<String>) -> Heard {
        Heard { me, target: ssid.map(|s| s.into_bytes()).unwrap_or_default(), bss: None, frames: Vec::new(), watch: None, rates: None, short: false, gw: None, ring: 0, scan: Default::default(), pin: None, home: None, gw_mac: None, resp: 0 }
    }
}

/// Called by `rx::take` for every clean frame: a beacon / probe response naming the target fixes the BSS;
/// any other management frame whose addr1 (§9.2.4.3) is our MAC is kept for the rung that waits on it.
pub(super) fn hear(frame: &[u8], rssi: i8, h: &mut Heard) {
    if frame.len() < 24 || (frame[0] >> 2) & 0x3 != 0 {
        return;
    }
    note(frame, rssi, h); // STATIONROAM (B897): every beacon of the SSID into the scan table while a window is open
    if let Some(wt) = h.watch.as_mut() {
        wt.frame(frame, h.me, crate::arch::ticks());
        return;
    }
    let sub = frame[0] >> 4;
    if sub == 0x8 || sub == 0x5 {
        if h.bss.is_none() && !h.target.is_empty() {
            let pin = h.pin; h.bss = assoc::bss_match(frame, &h.target).filter(|b| pin.map_or(true, |p| p == b.bssid)); // STATIONROAM (B897): a roam / re-join pins its BSSID (None on the boot's pass)
        }
    } else if frame[4..10] == h.me && h.frames.len() < KEEP {
        h.frames.push(frame.to_vec());
    }
}

/// How one rung's wait ended.
pub(super) enum Reply<T> {
    Got(T),
    Refused(&'static str, u16),
    None,
}

/// Poll the receiver in `SLICE_US` slices for up to `REPLY_MS`, draining the kept frames through `pick`; a
/// Deauthentication / Disassociation from the BSS (§9.3.3.12 / §9.3.3.4) ends the wait with its reason.
/// Returns the reply and the milliseconds waited.
fn await_reply<T>(
    bar0: u64, rxr: &mut rx::Rx, seen: &mut rx::Seen, ch: u16, bssid: Mac, w: &mut rx::Writes,
    pick: fn(&[u8], Mac, Mac) -> Option<T>,
) -> (Reply<T>, u32) {
    let me = seen.join.me;
    let mut waited = 0u32;
    while waited < REPLY_MS {
        rx::poll(bar0, rxr, seen, ch, SLICE_US, w);
        waited += (SLICE_US / 1000) as u32;
        let frames = core::mem::take(&mut seen.join.frames);
        for f in frames.iter() {
            if let Some(t) = pick(f, me, bssid) {
                return (Reply::Got(t), waited);
            }
            if let Some((what, reason)) = assoc::refusal(f, me, bssid) {
                return (Reply::Refused(what, reason), waited);
            }
        }
    }
    (Reply::None, waited)
}

fn mac_text(m: Mac) -> String {
    alloc::format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}

fn ssid_text(b: &[u8]) -> String {
    b.iter().map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '?' }).collect()
}

/// The TX half of a rung's line: `tx=<SENT|NOT-SENT> acked=<0|1> frame_count=<n>`.
fn tx_fields(r: &tx::Report) -> String {
    alloc::format!("tx={} acked={} frame_count={}", if r.sent() { "SENT" } else { "NOT-SENT" }, r.acked as u8, r.frame_count)
}

/// S8 — Open System authentication (§9.3.3.11, §12.3.3.2): `build_auth_open` (algorithm 0, transaction 1)
/// out as ring frame 1; the BSS's transaction-2 answer judged by `assoc::auth_reply`. `true` = status 0.
fn s8_auth(bar0: u64, p: &tx::Prep, rxr: &mut rx::Rx, seen: &mut rx::Seen, bss: Bss, ch: u16, w: &mut rx::Writes) -> bool {
    let me = seen.join.me;
    let ssid = ssid_text(&seen.join.target);
    let frame = wifi_core::ieee80211::build_auth_open(me, bss.bssid, 0);
    let r = tx::fire_frame(bar0, p, &frame, ch, (seen.join.ring + FRAME_AUTH) % tx::FRAMES, "auth", tx::RATE_1M, w);
    let (reply, waited) = await_reply(bar0, rxr, seen, ch, bss.bssid, w, assoc::auth_reply);
    let (resp, fields, verdict) = match reply {
        Reply::Got(a) => {
            let f = alloc::format!("alg={} seq={} status={}", a.alg, a.seq, a.status);
            let v = if a.alg != 0 {
                alloc::format!("NOT-AUTHENTICATED reason=alg={}", a.alg)
            } else if a.seq != 2 {
                alloc::format!("NOT-AUTHENTICATED reason=seq={}", a.seq)
            } else if a.status != 0 {
                alloc::format!("NOT-AUTHENTICATED reason=status={}", a.status)
            } else {
                String::from("AUTHENTICATED")
            };
            (1, f, v)
        }
        Reply::Refused(what, reason) => (0, String::from("alg=- seq=- status=-"), alloc::format!("NOT-AUTHENTICATED reason={}(reason={})", what, reason)),
        Reply::None => {
            let why = if r.sent() { alloc::format!("no-response(acked={})", r.acked as u8) } else { alloc::format!("tx:{}", r.reason()) };
            (0, String::from("alg=- seq=- status=-"), alloc::format!("NOT-AUTHENTICATED reason={}", why))
        }
    };
    serial_println!(
        ":: wifiauth: s8 auth ssid={} bssid={} ch={} {} resp={} {} waited={}ms -> {} ::",
        ssid, mac_text(bss.bssid), ch, tx_fields(&r), resp, fields, waited, verdict
    );
    verdict == "AUTHENTICATED"
}

/// S9 — association (§9.3.3.5 / §9.3.3.6): `build_assoc_req` (ESS, listen interval 3, SSID, Supported and
/// Extended Supported Rates) out as ring frame 2; the response judged by `assoc::assoc_reply`. `Some(aid)`
/// = status 0 and the AID granted (§9.4.1.8).
fn s9_assoc(bar0: u64, p: &tx::Prep, rxr: &mut rx::Rx, seen: &mut rx::Seen, bss: Bss, ch: u16, w: &mut rx::Writes) -> Option<u16> {
    let me = seen.join.me;
    let target = seen.join.target.clone();
    let cap = assoc::assoc_cap(&bss); // STATIONSHORTCAP (B884): ESS, with Short Preamble when the beacon allows it
    let frame = wifi_core::ieee80211::build_assoc_req(me, bss.bssid, &target, 0, cap);
    let r = tx::fire_frame(bar0, p, &frame, ch, (seen.join.ring + FRAME_ASSOC) % tx::FRAMES, "assoc-req", tx::RATE_1M, w);
    let (reply, waited) = await_reply(bar0, rxr, seen, ch, bss.bssid, w, assoc::assoc_reply_air);
    let (resp, fields, aid, verdict) = match reply {
        Reply::Got((a, rates, short)) if a.status == 0 => {
            seen.join.rates = rates;
            seen.join.short = short;
            (1, alloc::format!("status=0 aid={}", a.aid), Some(a.aid), String::from("ASSOCIATED"))
        }
        Reply::Got((a, _, _)) => (1, alloc::format!("status={} aid=-", a.status), None, alloc::format!("NOT-ASSOCIATED reason=status={}", a.status)),
        Reply::Refused(what, reason) => (0, String::from("status=- aid=-"), None, alloc::format!("NOT-ASSOCIATED reason={}(reason={})", what, reason)),
        Reply::None => {
            let why = if r.sent() { alloc::format!("no-response(acked={})", r.acked as u8) } else { alloc::format!("tx:{}", r.reason()) };
            (0, String::from("status=- aid=-"), None, alloc::format!("NOT-ASSOCIATED reason={}", why))
        }
    };
    serial_println!(
        ":: wifiauth: s9 assoc ssid={} bssid={} cap={:#06x} {} resp={} {} waited={}ms -> {} ::",
        ssid_text(&target), mac_text(bss.bssid), cap, tx_fields(&r), resp, fields, waited, verdict
    );
    aid
}

/// Both rungs' NOT ATTEMPTED lines, and the verdicts recorded as not attempted.
fn absent(why: String) {
    super::super::status::set_join(3, 3, 0);
    serial_println!(":: wifiauth: s8 auth NOT ATTEMPTED reason={} -> ABSENT ::", why);
    serial_println!(":: wifiauth: s9 assoc NOT ATTEMPTED reason=s8-ABSENT -> ABSENT ::");
}

/// S8 then S9, from `wifi10()` after the probe window with both engines armed. The gates, in order: an
/// SSID was named (1..=32 octets, §9.4.2.2); a beacon / probe response of that SSID was heard; the BSS is
/// open (WPA is unbuilt). S9 runs only on AUTHENTICATED. Returns the frames sent (0, 1 or 2).
pub(super) fn run(bar0: u64, p: &tx::Prep, rxr: &mut rx::Rx, seen: &mut rx::Seen, ch: u16, w: &mut rx::Writes) -> u32 {
    let n = seen.join.target.len();
    if n == 0 {
        absent(String::from("no-ssid(type tests wifi <ssid>)"));
        return 0;
    }
    if n > 32 {
        absent(alloc::format!("ssid-too-long({})", n));
        return 0;
    }
    let Some(bss) = seen.join.bss else {
        absent(alloc::format!("bss-not-heard(ssid={})", ssid_text(&seen.join.target)));
        return 0;
    };
    if bss.protected {
        absent(alloc::format!("protected(bssid={}; WPA unbuilt)", mac_text(bss.bssid)));
        return 0;
    }
    seen.join.frames.clear();
    if !s8_auth(bar0, p, rxr, seen, bss, ch, w) {
        super::super::status::set_join(2, 3, 0);
        serial_println!(":: wifiauth: s9 assoc NOT ATTEMPTED reason=s8-NOT-AUTHENTICATED -> ABSENT ::");
        return 1;
    }
    match s9_assoc(bar0, p, rxr, seen, bss, ch, w) {
        Some(aid) => super::super::status::set_join(1, 1, aid),
        None => super::super::status::set_join(1, 2, 0),
    }
    2
}

/// WIFIDATA (B803) — rung S10a, from `wifi10()` after `run` with both engines still armed: the station's
/// `data::Link` sends ONE ARP probe (RFC 826 / RFC 5227, sender 0.0.0.0) for the gateway the operator named
/// (`tests wifi <ssid> <gw>`) as a ToDS data frame on ring frame 3, then drains S6's receiver for up to
/// `REPLY_MS`, judging every kept data frame with `data::from_air` and the reply with `data::arp_reply_parse`.
/// Gates: S9 read ASSOCIATED; a gateway was named. Returns the data frames sent (0 or 1) and, on LINKED only, the
/// Link for `keep` (WIFIDHCP B831: the station then outlives the pass).
pub(super) fn link(bar0: u64, p: &tx::Prep, rxr: &mut rx::Rx, seen: &mut rx::Seen, ch: u16, gw: Option<[u8; 4]>, w: &mut rx::Writes) -> (u32, Option<super::super::data::Link>) {
    use super::super::data;
    let skip = |why: String| {
        data::set_link(3);
        serial_println!(":: wifidata: s10a link NOT ATTEMPTED reason={} -> ABSENT ::", why);
    };
    let (_, s9, _) = super::super::status::join();
    let (Some(bss), 1) = (seen.join.bss, s9) else {
        skip(String::from(if s9 == 0 || s9 == 3 { "s9-absent" } else { "s9-NOT-ASSOCIATED" }));
        return (0, None);
    };
    let Some(gw) = gw else {
        skip(String::from("no-gateway(type tests wifi <ssid> <gw-ip>)"));
        return (0, None);
    };
    let gw_text = alloc::format!("{}.{}.{}.{}", gw[0], gw[1], gw[2], gw[3]); seen.join.gw = Some(gw); // STATIONREJOIN (B891): kept for a re-join's S10a
    let mut l = data::Link::new(seen.join.me, bss.bssid).rebased(seen.join.ring);
    let (r, sent) = match l.send(&data::arp_request(l.mac(), [0; 4], gw)) {
        Ok((air, n)) => (tx::fire_frame(bar0, p, &air, ch, n, "data", tx::RATE_1M, w), 1),
        Err(why) => {
            serial_println!("[wifi10] tx-poke kind=data REFUSED reason={} — nothing written", why);
            (tx::Report::default(), 0)
        }
    };
    seen.data.clear();
    let mut buf = [0u8; 256];
    let mut reply = None;
    let mut waited = 0u32;
    while sent == 1 && reply.is_none() && waited < REPLY_MS {
        rx::poll(bar0, rxr, seen, ch, SLICE_US, w);
        waited += (SLICE_US / 1000) as u32;
        while let Some(n) = l.recv(&mut seen.data, &mut buf) {
            if let Some((mac, ip)) = data::arp_reply_parse(&buf[..n]) {
                if ip == gw {
                    reply = Some(mac);
                    break;
                }
            }
        }
    }
    let verdict = match reply {
        Some(_) => String::from("LINKED"),
        None if !r.sent() => alloc::format!("NOT-LINKED reason=tx:{}", if sent == 1 { r.reason() } else { String::from("refused") }),
        None => alloc::format!("NOT-LINKED reason=no-arp-reply(acked={} rx_data={})", r.acked as u8, l.rx_data),
    };
    data::set_link(if reply.is_some() { 1 } else { 2 }); if reply.is_some() { seen.join.gw_mac = reply; } // STATIONROAM (B897): the gateway's MAC, compared after a roam
    serial_println!(
        ":: wifidata: s10a link ssid={} gw={} tx={} acked={} rx_data={} arp={} waited={}ms -> {} ::",
        ssid_text(&seen.join.target), gw_text, if r.sent() { "SENT" } else { "NOT-SENT" }, r.acked as u8, l.rx_data,
        reply.map(|m| alloc::format!("reply mac={}", mac_text(m))).unwrap_or_else(|| String::from("none")), waited, verdict
    );
    (sent, if reply.is_some() { Some(l) } else { None })
}

// ── WIFIDHCP (rmbp-ledger B831): the kept station ────────────────────────────────────────────────────────────

/// Everything the pass armed, kept past it on LINKED: the TX ring, the receiver and its state, the Link; the station's
/// asynchronous TX queue and, once the watch dropped the link, why (STATIONWATCH / STATIONTX B843).
struct Station {
    bar0: u64,
    ch: u16,
    prep: tx::Prep,
    rxr: rx::Rx,
    seen: rx::Seen,
    link: super::super::data::Link,
    w: rx::Writes,
    q: tx::Queue,
    lost: Option<&'static str>,
    /// STATIONRATE (B855): every station data frame's rate (500 kbit/s units), chosen by `keep`; STATIONAIR (B862): with the
    /// AP's ACK rate and the short preamble, one `tx::TxRate`.
    rate: tx::TxRate,
}

/// THE station. Lock order: smolnet's `STACK` -> `STATION` (smoltcp's poll calls the accessor triple); nothing
/// takes `STATION` then `STACK`. No sleep is taken under it (`rx::drain`, `tx::fire_frame`'s status spin).
static STATION: crate::sync::Mutex<Option<Station>> = crate::sync::Mutex::new(None);

/// From `wifi10()` after its end line, when `link` returned the Link (S10a LINKED): the engines stay armed and
/// move here; from now on only the station's fns below touch them.
pub(super) fn keep(bar0: u64, prep: tx::Prep, rxr: rx::Rx, mut seen: rx::Seen, ch: u16, mut w: rx::Writes, mut link: super::super::data::Link) {
    // STATIONRATE (B855): the receive filter and the station's rate, set here on the shell task BEFORE `STATION` is filled
    // (no lock held; the pass still owns the engines).
    let mut rate = tx::TxRate::long(tx::RATE_1M);
    if let Some(b) = seen.join.bss {
        let (pre, post) = rx::station_filter(bar0, b.bssid, &mut w);
        // STATIONTEMPLATE (B862): the MAC and BSSID into template RAM beside the BSSID filter (b43 main.c:679-703).
        let words = tx::templates(bar0, seen.join.me, b.bssid);
        let template = if words == 3 { String::from("written") } else { alloc::format!("short({})", words) };
        serial_println!(
            "[wifi10] station filter promisc=0 beacpromisc=0 keepbad=0 bssid={} macctl={:#010x}->{:#010x} template={} — b43 main.c:690, :3103-3140 (STATIONRATE B855); main.c:679-703 (STATIONTEMPLATE B862)",
            mac_text(b.bssid), pre, post, template
        );
        rate = pick_rate(bar0, &prep, seen.join.rates, seen.join.short, b.erp, ch, &mut link, b.bssid, &mut w);
    }
    seen.data.clear();
    seen.join.frames.clear();
    seen.join.watch = seen.join.bss.map(|b| super::super::data::Watch::new(b.bssid, crate::arch::ticks())); seen.join.home = seen.join.bss.map(|b| b.bssid); seen.join.scan.arm(crate::arch::ticks()); // STATIONROAM (B897): the BSS linked, the first scan window ROAM_SCAN_MS out
    let next = link.tx_next(); let mut q = tx::Queue::default(); q.ctl = rate_ctl(seen.join.rates, seen.join.short, rate.rate); // STATIONRATECTL (B881): the rate controller starts at `pick_rate`'s rate, owned by STATION with the queue
    *STATION.lock() = Some(Station { bar0, ch, prep, rxr, seen, link, w, q, lost: None, rate }); rejoin_spawn(); // STATIONREJOIN (B891): the station task, started once
    serial_println!("[wifi10] station kept tx_next={} — the TX ring and the receiver stay armed for the stack (WIFIDHCP B831)", next);
}

/// `data::raw_tx`: one Ethernet frame out as a ToDS data frame on the station's next ring frame, posted and not waited
/// on (STATIONTX B843: the status is reaped on a later pass). `true` = posted. No room = refused BEFORE the Link numbers
/// the frame, so the ring never carries a gap.
pub(in crate::wifi) fn station_tx(eth: &[u8]) -> bool {
    let mut g = STATION.lock();
    let Some(st) = g.as_mut() else { return false };
    if st.lost.is_some() {
        return false;
    }
    let now = crate::arch::ticks();
    tx::reap(st.bar0, &mut st.q, now);
    if !st.q.room() {
        st.q.full += 1;
        return false;
    }
    match st.link.send(eth) {
        Ok((air, n)) => { let r = rate_for(st, &air); tx::queue(st.bar0, &st.prep, &mut st.q, &air, st.ch, n, now, r, &mut st.w) } // STATIONRATECTL (B881): the controller's rate (a probe every 16th unicast frame)
        Err(_) => false,
    }
}

/// `data::tx_ready`: room on the station's TX ring (statuses reaped first). Never waits on the lock: contended or
/// lost = no room.
pub(in crate::wifi) fn station_tx_room() -> bool {
    let Some(mut g) = STATION.try_lock() else { return false };
    let Some(st) = g.as_mut() else { return false };
    if st.lost.is_some() {
        return false;
    }
    tx::reap(st.bar0, &mut st.q, crate::arch::ticks());
    st.q.room()
}

/// `data::raw_rx`: the next kept data frame to us as Ethernet; the station is serviced once when none is kept.
pub(in crate::wifi) fn station_rx(out: &mut [u8]) -> Option<usize> {
    let mut g = STATION.lock();
    let st = g.as_mut()?;
    if st.lost.is_some() {
        return None;
    }
    if let Some(n) = st.link.recv(&mut st.seen.data, out) {
        return Some(n);
    }
    service(st);
    st.link.recv(&mut st.seen.data, out)
}

/// `data::rx_ready`: the station serviced once; a data frame is kept. Never waits on the lock.
pub(in crate::wifi) fn station_pending() -> bool {
    let Some(mut g) = STATION.try_lock() else { return false };
    let Some(st) = g.as_mut() else { return false };
    if st.lost.is_some() {
        return false;
    }
    if st.seen.data.is_empty() {
        service(st);
    }
    !st.seen.data.is_empty()
}

/// `data::hw_addr` / `data::kept`: the station MAC and whether its link is up (not lost), while a station is kept.
pub(in crate::wifi) fn station_mac() -> Option<(Mac, bool)> {
    STATION.lock().as_ref().map(|st| (st.link.mac(), st.lost.is_none()))
}

// ── STATIONWATCH / STATIONTX (rmbp-ledger B843): the station serviced on its own drains, dropped on a loss, re-joined ──

/// One service pass under `STATION` (no sleep): the receiver drained (`hear` feeds the watch), the TX statuses reaped,
/// the watch ticked; a loss drops the link (`lose`).
fn service(st: &mut Station) {
    rx::drain(st.bar0, &mut st.rxr, &mut st.seen, st.ch, &mut st.w);
    let now = crate::arch::ticks();
    tx::reap(st.bar0, &mut st.q, now); rate_step(st, now); // STATIONRATECTL (B881): at most one step per second, under STATION
    let why = st.seen.join.watch.as_mut().and_then(|wt| wt.tick(now));
    if let Some(why) = why {
        lose(st, why);
    }
}

/// The link is gone: every accessor answers link-down from now on (the engines stay as they are — stopping them sleeps,
/// so `rejoin` does it from the shell task). Said once.
fn lose(st: &mut Station, why: &'static str) {
    st.lost = Some(why); let rejoins = rejoin_lost(why); // STATIONREJOIN (B891): LOST_AT / LOST_WHY recorded, the re-join armed (STATION -> REJOIN)
    st.seen.data.clear();
    let (reason, beacons, tu) = st.seen.join.watch.as_ref().map_or((0, 0, 0), |wt| (wt.reason, wt.beacons, wt.tu));
    let q = &st.q;
    serial_println!(
        "[wifi10] station lost why={} reason={} beacons={} tu={} tx_posted={} tx_sent={} tx_acked={} tx_failed={} tx_stuck={} tx_full={} tx_q={} rx_data={} rate={} ok={} retry={}.{} rejoins={} — the accessor triple answers link-down; the station task re-joins (STATIONWATCH B843, STATIONREJOIN B891)",
        why, if why == "beacon-loss" { String::from("-") } else { alloc::format!("{}", reason) }, beacons, tu,
        q.posted, q.sent, q.acked, q.failed, q.stuck, q.full, q.in_flight(), st.link.rx_data, assoc::rate_name(q.ctl.rate()), q.ctl.stats(q.ctl.rate()).0, q.ctl.stats(q.ctl.rate()).1 / 100, q.ctl.stats(q.ctl.rate()).1 % 100 / 10, rejoins
    );
}

/// From `wifi10()`'s once-per-boot guard (an operator's second `tests wifi`): STATIONREJOIN (B891) — the pass is NEVER
/// reopened (its PHY one-shots, `htphyinit::tx_prep`, WIFI11's power control, `tx::prep`, `rx::arm`, ran once this boot);
/// a LOST station gets ONE re-join attempt now on the same PHY (`rejoin_step`, the backoff skipped). Always `false`.
pub(super) fn rejoin() -> bool {
    rejoin_step(true);
    false
}

// ── STATIONRATE (rmbp-ledger B855): the station's rate, and the TX header / MACCTL known answers ───────────────────────

/// From `keep`, on the shell task before `STATION` is filled: when the AP listed OFDM 24, ONE Null data frame
/// (`build_null_data`) at OFDM 24 on the Link's next ring frame (`fire_frame`'s bounded status spin) — `probe=ack` iff it was
/// acked at that rate (`Report::acked_at_rate`); then `assoc::station_rate`. STATIONAIR (B862): the rate leaves as ONE
/// `tx::TxRate` — the AP's answer rate (`assoc::ack_rate`) and the short preamble (`assoc::station_short`, the AP's capability
/// bit `short_cap` and a CCK 2/5.5/11 rate). Said once. STATIONERP (B884): the beacon's ERP octet `erp` — Barker_Preamble_Mode
/// forces the long preamble (`assoc::erp_barker`), Use_Protection puts every OFDM frame behind a CTS-to-self (`TxRate.prot`).
fn pick_rate(bar0: u64, p: &tx::Prep, rs: Option<wifi_core::ieee80211::RateSet>, short_cap: bool, erp: Option<u8>, ch: u16, link: &mut super::super::data::Link, bssid: Mac, w: &mut rx::Writes) -> tx::TxRate {
    let probe = if assoc::ofdm_listed(rs) {
        let n = link.reserve();
        let null = wifi_core::ieee80211::build_null_data(link.mac(), bssid, 0);
        let r = tx::fire_frame(bar0, p, &null, ch, n, "null-ofdm24", wifi_core::ieee80211::RATE_24M, w);
        if r.acked_at_rate() { "ack" } else { "noack" }
    } else {
        "none"
    };
    let rate = assoc::station_rate(rs, probe == "ack");
    let air = tx::TxRate { rate, ack: assoc::ack_rate(rs, rate), short: assoc::station_short(short_cap, assoc::erp_barker(erp), rate), fb: assoc::fallback_rate(rs, rate), prot: assoc::erp_protect(erp) };
    serial_println!(
        "[wifi10] station rate={} probe={} probe_at=keep(shell task, before STATION, no lock) ap={} preamble={} ack={} nav={}us erp={} — b43 xmit.c:136-189, :884 (STATIONRATE B855); xmit.c:374-375, 802.11-2020 §10.6.6.5.2 (STATIONAIR B862); xmit.c:421-539, §9.4.2.12 (STATIONERP B884)",
        assoc::rate_name(rate), probe, assoc::rates_text(rs), if air.short { "short" } else { "long" }, assoc::rate_name(air.ack), tx::nav_us(air), assoc::erp_text(erp)
    );
    air
}

/// `tests wifi` (`data::kat` rows 10-14, 17-18): `tx::rate_kat`'s four rows, then the station MACCTL word from S6's `arm` word
/// (ENABLED | INFRA | BEACPROMISC | KEEP_BAD | PROMISC | DISCPMQ -> ENABLED | INFRA | DISCPMQ), then `tx::air_kat`'s two
/// (duration, shortpre; STATIONAIR B862), `tx::fb_kat` (B881), `tx::cts_kat` (the CTS-to-self header words; STATIONERP B884).
pub(in crate::wifi) fn station_kat() -> [bool; 9] {
    let t = tx::rate_kat();
    let a = tx::air_kat();
    [t[0], t[1], t[2], t[3], rx::station_macctl(0x4192_0001) == 0x4002_0001, a[0], a[1], tx::fb_kat(), tx::cts_kat()]
}

// ── STATIONRATECTL (rmbp-ledger B881): the kept station's rate follows the ACKs ───────────────────────────────────────

/// From `keep` (shell task, before `STATION`, no lock): the controller over the AP's rate set, starting at `pick_rate`'s rate.
fn rate_ctl(rs: Option<wifi_core::ieee80211::RateSet>, short: bool, start: u8) -> super::super::data::RateCtl {
    super::super::data::RateCtl::new(rs, short, start, crate::arch::ticks())
}

/// A controller rate as ONE `tx::TxRate`, built as `pick_rate` builds its own: the AP's answer rate (`assoc::ack_rate`), the
/// short preamble (`assoc::station_short`), the TX header's fallback (`assoc::fallback_rate`); STATIONERP (B884): the BSS's ERP
/// octet `erp` (Barker -> long preamble, Use_Protection -> `prot`).
fn air_of(c: &super::super::data::RateCtl, erp: Option<u8>, rate: u8) -> tx::TxRate {
    tx::TxRate { rate, ack: assoc::ack_rate(c.rs, rate), short: assoc::station_short(c.short, assoc::erp_barker(erp), rate), fb: assoc::fallback_rate(c.rs, rate), prot: assoc::erp_protect(erp) }
}

/// `station_tx`, under `STATION`: the frame's rate. A group addr1 leaves at the current rate (never ACKed, so never a probe and
/// never counted: `tx::queue` records it as 0); a unicast frame at the controller's next rate.
fn rate_for(st: &mut Station, air: &[u8]) -> tx::TxRate {
    if air.len() < 10 || air[4] & 0x01 != 0 {
        return st.rate;
    }
    let r = st.q.ctl.next_rate();
    if r == st.rate.rate { st.rate } else { air_of(&st.q.ctl, st.seen.join.bss.and_then(|b| b.erp), r) }
}

/// `service`, under `STATION`, after the reap: at most one step per second (`RateCtl::decide`); the new rate becomes the
/// station's `TxRate`, said with the old rate's EWMA inputs at the moment of the step.
fn rate_step(st: &mut Station, now: u64) {
    let Some((old, new, why)) = st.q.ctl.decide(now) else { return };
    st.rate = air_of(&st.q.ctl, st.seen.join.bss.and_then(|b| b.erp), new);
    let (ok, retry) = st.q.ctl.stats(old);
    serial_println!(
        "[wifi10] station rate -> {}->{} why={} ok={} retry={}.{} — rc80211_minstrel.c shape (EWMA per rate, a sample frame every 16), b43 xmit.c:884 (STATIONRATECTL B881)",
        assoc::rate_name(old), assoc::rate_name(new), why, ok, retry / 100, retry % 100 / 10
    );
}

// ── STATIONREJOIN (rmbp-ledger B891): a lost station re-joins on the same PHY ───────────────────────────────────────────

/// The re-join's state (PURE, `data::Rejoin`: LOST_AT, LOST_WHY, the attempt count and the backoff). Lock order: smolnet's
/// `STACK` -> `STATION` -> `REJOIN` (`lose` takes it under `STATION`); never held while taking `STATION` or `STACK`, never
/// across a sleep.
static REJOIN: crate::sync::Mutex<super::super::data::Rejoin> = crate::sync::Mutex::new(super::super::data::Rejoin::new());
/// The station task has been started (once per boot, by the first `keep`).
static SPAWNED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The station task's check cadence.
const REJOIN_POLL_MS: u64 = 250;
/// How long a re-join listens for the lost BSS's beacon (passive: `keep`'s BSSID filter passes it) before `failed:hear`.
const HEAR_MS: u32 = 1000;

/// From `lose`, under `STATION`: the loss recorded and the re-join armed (first attempt 1 s later). Returns the re-joins
/// that linked this boot (the `station lost` line's `rejoins=`).
fn rejoin_lost(why: &'static str) -> u32 {
    let mut r = REJOIN.lock();
    r.lose(crate::arch::ticks(), why);
    r.rejoins
}

/// From `keep`, on the task that kept the station, no lock held: the station task, once per boot (net_tick's shape and CPU
/// choice: the drive sleeps for seconds, so it sits on neither the render nor the service loop).
fn rejoin_spawn() {
    if SPAWNED.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let cpu = crate::arch::smp::xhci_worker_cpu(0).or(crate::arch::smp::service_cpu()).unwrap_or(0);
    crate::arch::sched::spawn("wifi-station", rejoin_task, 0, cpu, crate::arch::sched::PRIO_NORMAL);
}

/// The station task: every `REJOIN_POLL_MS`, a due re-join of a lost station (`rejoin_step`); idle otherwise.
fn rejoin_task(_: usize) {
    loop {
        crate::arch::sched::sleep_ms(REJOIN_POLL_MS);
        rejoin_step(false); roam_step(); // STATIONROAM (B897): the scan window and the roam, on the same task
    }
}

/// One re-join attempt when a LOST station is kept and its backoff ran out (`force`: the operator's `tests wifi`, now). The
/// Station is TAKEN out of `STATION` (the lock released at once) and owned by value while the rungs sleep; on LINKED `keep`
/// refills `STATION`, on a failure the lost Station goes back and the backoff doubles (capped 30 s). Said once per attempt.
fn rejoin_step(force: bool) {
    let now = crate::arch::ticks();
    let taken = {
        let mut g = STATION.lock();
        let lost = g.as_ref().map_or(false, |st| st.lost.is_some());
        if lost && (force || REJOIN.lock().due(now)) { g.take() } else { None }
    };
    let Some(st) = taken else { return };
    let why = st.lost.unwrap_or("-"); let from = st.seen.join.home; let ch0 = st.ch; // STATIONROAM (B897): the BSS lost, for the roam line; STATIONROAM-XCHAN (B898): its channel
    let (n, after) = REJOIN.lock().attempt(now);
    let verdict = match associate(st, None) {
        Ok(to) => {
            REJOIN.lock().linked();
            if let Some(from) = from.filter(|f| *f != to) {
                let (got, ch1) = STATION.lock().as_ref().map_or((None, ch0), |s| (s.seen.join.scan.get(to).map(|e| e.rssi), s.ch));
                roam_line(from, to, "loss", None, got, (ch0, ch1), "linked");
            }
            String::from("linked")
        }
        Err((st, rung)) => {
            *STATION.lock() = Some(st);
            REJOIN.lock().failed(crate::arch::ticks());
            alloc::format!("failed:{}", rung)
        }
    };
    serial_println!(
        "[wifi10] station rejoin n={} after={} why={} -> {} — the same PHY: no tx_prep, power control or tx::prep re-run (STATIONREJOIN B891)",
        n, after, why, verdict
    );
    if verdict == "linked" {
        restack();
    }
}

/// The per-association half of the join on the kept engines: the old queue's TX statuses reaped (so the rungs' `fire_frame`
/// spins read only their own); the rungs' ring base set so S8 leaves on the old Link's next frame (the ring continues, no
/// stale descriptor is walked); the watch and the BSS cleared, so `hear` re-fixes the BSS from its beacons within `HEAR_MS`;
/// then `run` (S8, S9), `link` (S10a, the gateway the operator named) and `keep`. `Err` names the rung that failed.
/// STATIONROAM-XCHAN (B898): the pin carries the BSS's channel; the PHY is retuned to it (`retune`) before the hear loop.
fn associate(mut st: Station, pin: Option<(Mac, u8)>) -> Result<Mac, (Station, &'static str)> {
    tx::reap(st.bar0, &mut st.q, crate::arch::ticks());
    st.seen.join.ring = (st.link.tx_next() + tx::FRAMES - FRAME_AUTH) % tx::FRAMES;
    st.seen.join.watch = None;
    st.seen.join.bss = None;
    st.seen.join.frames.clear();
    st.seen.data.clear();
    // STATIONROAM (B897): BEACPROMISC on, so the target's beacons pass the old BSSID filter (`keep`'s `station_filter` narrows it
    // again on LINKED; every failure below narrows it first); no pin = the re-join after a loss: one scan window, the strongest.
    rx::scan_filter(st.bar0, true, &mut st.w);
    let (pin, pch) = match pin.or_else(|| scan(&mut st)) {
        Some(b) => b,
        None => return Err(narrow(st, "hear")),
    };
    if retune(&mut st, pch).is_err() {
        return Err(narrow(st, "tune"));
    }
    st.seen.join.pin = Some(pin); st.seen.join.ring = (st.link.tx_next() + tx::FRAMES - FRAME_AUTH) % tx::FRAMES; // STATIONROAM-ACTIVE (B951): the rung base re-read after the scan, whose sweep's probes took ring frames (S8 would re-post the last probe's frame: same TXINDEX, never sent)
    let mut waited = 0u32;
    while st.seen.join.bss.is_none() && waited < HEAR_MS {
        rx::poll(st.bar0, &mut st.rxr, &mut st.seen, st.ch, SLICE_US, &mut st.w);
        waited += (SLICE_US / 1000) as u32;
    }
    if st.seen.join.bss.is_none() {
        return Err(narrow(st, "hear"));
    }
    if run(st.bar0, &st.prep, &mut st.rxr, &mut st.seen, st.ch, &mut st.w) < 2 {
        return Err(narrow(st, "s8"));
    }
    if super::super::status::join().1 != 1 {
        return Err(narrow(st, "s9"));
    }
    let gw = st.seen.join.gw;
    let (_, kept) = link(st.bar0, &st.prep, &mut st.rxr, &mut st.seen, st.ch, gw, &mut st.w);
    let Some(l) = kept else { return Err(narrow(st, "s10a")) };
    let Station { bar0, ch, prep, rxr, seen, w, .. } = st;
    keep(bar0, prep, rxr, seen, ch, w, l);
    Ok(pin)
}

/// After a LINKED re-join, no wifi lock held: when smoltcp rides the station, its one DHCP socket is reset through smoltcp's
/// public `reset()` (`smolnet::dhcp_reset`, LEASERENEW's path: the held lease is lost, the next one is a first lease).
#[cfg(feature = "smolnet")]
fn restack() {
    if super::super::data::bound() {
        let _ = crate::smolnet::dhcp_reset();
    }
}

#[cfg(not(feature = "smolnet"))]
fn restack() {}

// ── STATIONROAM (rmbp-ledger B897): another BSS of the same SSID, same channel — a scan window, a pick, `associate` pinned ──

/// From `hear`, for every clean management frame: while a scan window is open, a beacon / probe response of the SSID (open BSSes
/// only: WPA is unbuilt) into the table with the frame's RSSI (`data::ht_power`) and its DS channel (0 = absent). STATIONROAM-ACTIVE (B951):
/// a probe response addressed to us also sets its DS channel's bit in `Heard.resp` (`dwell`'s early end).
fn note(frame: &[u8], rssi: i8, h: &mut Heard) {
    let sub = frame[0] >> 4;
    if !h.scan.is_open() || (sub != 0x8 && sub != 0x5) {
        return;
    }
    if let Some(b) = assoc::bss_match(frame, &h.target) {
        if !b.protected {
            h.scan.see(b.bssid, b.channel.unwrap_or(0), rssi, crate::arch::ticks()); if sub == 0x5 && frame[4..10] == h.me { h.resp |= super::super::data::resp_bit(b.channel.unwrap_or(0)); } // STATIONROAM-ACTIVE (B951): an answer to our probe, by its DS channel
        }
    }
}

/// From `associate` with no pin (the re-join after a loss; the Station owned by value, the filter already widened): one window of
/// `ROAM_WINDOW_MS` on the receiver, then (STATIONROAM-XCHAN B898: the link is down, so unconditionally) the dwells on the other
/// grid channels with the home channel restored (`sweep`), then the strongest fresh BSS of the SSID and its channel.
fn scan(st: &mut Station) -> Option<(Mac, u8)> {
    st.seen.join.scan.open(crate::arch::ticks());
    rx::poll(st.bar0, &mut st.rxr, &mut st.seen, st.ch, super::super::data::ROAM_WINDOW_MS * 1000, &mut st.w);
    let _ = sweep(st, "lost");
    st.seen.join.scan.close();
    super::super::data::roam_pick(None, st.ch as u8, &st.seen.join.scan)
}

/// Every `associate` failure: the filter narrowed back (BEACPROMISC off), the window closed and the pin dropped, then the `Err`.
fn narrow(mut st: Station, rung: &'static str) -> (Station, &'static str) {
    rx::scan_filter(st.bar0, false, &mut st.w);
    st.seen.join.scan.close();
    st.seen.join.pin = None;
    (st, rung)
}

/// The station task, after `rejoin_step`, every `REJOIN_POLL_MS`: on a linked (not lost) station, under `STATION` and without a
/// sleep — one `service` (so the window's beacons are drained even when the stack is idle), then the window: `due` opens it
/// (BEACPROMISC on, `rx::scan_filter`), `ended` closes it (off) and asks `data::roam_pick` for a BSS 6 dB stronger than the one
/// linked. A pick TAKES the Station out of `STATION` and runs `associate` pinned to it on the kept engines (no PHY one-shot, no
/// `tx::prep`): LINKED -> `keep` refilled `STATION`, and the DHCP socket is reset only when S10a's ARP reply named another gateway
/// MAC; a failure goes through `lose` ("roam": the line, `REJOIN` armed) back into `STATION`, so the re-join ladder's own scan
/// window picks again (the old BSS when it is still the strongest). Said once per roam. STATIONROAM-XCHAN (B898): every pass samples
/// the idle gate (`BssTable::quiet` over `Link.rx_uni` / `Queue.posted`; STATIONROAM-IDLEUNI B951: group frames do not count); a window that ends with the station quiet and nothing in
/// flight is NOT closed under the lock: the Station is TAKEN and, no lock held, `sweep` dwells on the other grid channels inside the
/// same open window and restores the home channel; then the window closes and `roam_pick` may name a BSS on another channel, which
/// `associate` retunes to. A sweep that cannot restore the home channel is a loss (`lose(st, "tune")`, the re-join ladder retunes).
fn roam_step() {
    let now = crate::arch::ticks();
    let taken = {
        let mut g = STATION.lock();
        let Some(st) = g.as_mut() else { return };
        if st.lost.is_some() {
            return;
        }
        service(st);
        if st.lost.is_some() {
            return;
        }
        let quiet = st.seen.join.scan.quiet(st.link.rx_uni, st.q.posted, now); // STATIONROAM-XCHAN (B898): the idle gate, sampled every pass; STATIONROAM-IDLEUNI (B951): unicast frames to us only
        if st.seen.join.scan.due(now) {
            st.seen.join.scan.open(now);
            rx::scan_filter(st.bar0, true, &mut st.w);
            return;
        }
        if !st.seen.join.scan.ended(now) {
            return;
        }
        if st.seen.join.home.is_some() && quiet && st.q.in_flight() == 0 {
            g.take().map(|st| (st, None)) // STATIONROAM-XCHAN (B898): the window stays open and wide; the dwells extend it, outside the lock
        } else {
            st.seen.join.scan.close();
            rx::scan_filter(st.bar0, false, &mut st.w);
            let Some(home) = st.seen.join.home else { return };
            let Some(to) = super::super::data::roam_pick(Some(home), st.ch as u8, &st.seen.join.scan) else { return };
            g.take().map(|st| (st, Some(to)))
        }
    };
    let Some((mut st, pick)) = taken else { return };
    let to = match pick {
        Some(p) => p,
        None => {
            let swept = sweep(&mut st, "idle");
            st.seen.join.scan.close();
            rx::scan_filter(st.bar0, false, &mut st.w);
            if swept.is_err() {
                lose(&mut st, "tune");
                *STATION.lock() = Some(st);
                return;
            }
            let p = st.seen.join.home.and_then(|home| super::super::data::roam_pick(Some(home), st.ch as u8, &st.seen.join.scan));
            match p {
                Some(p) => p,
                None => {
                    *STATION.lock() = Some(st);
                    return;
                }
            }
        }
    };
    let Some(from) = st.seen.join.home else {
        *STATION.lock() = Some(st);
        return;
    };
    let was = st.seen.join.scan.get(from).map(|e| e.rssi);
    let got = st.seen.join.scan.get(to.0).map(|e| e.rssi);
    let ch0 = st.ch;
    let gw0 = st.seen.join.gw_mac;
    let verdict = match associate(st, Some(to)) {
        Ok(_) => {
            let moved = STATION.lock().as_ref().map_or(false, |s| s.seen.join.gw_mac != gw0);
            if moved {
                restack();
            }
            String::from("linked")
        }
        Err((mut st, rung)) => {
            lose(&mut st, "roam");
            *STATION.lock() = Some(st);
            alloc::format!("failed:{}", rung)
        }
    };
    roam_line(from, to.0, "rssi", was, got, (ch0, to.1 as u16), &verdict);
}

/// `[wifi10] station roam from= to= why=<rssi|loss> rssi=<dBm|->-><dBm|-> ch=<a>-><b> -> <linked|failed:<rung>>`.
fn roam_line(from: Mac, to: Mac, why: &str, was: Option<i8>, got: Option<i8>, ch: (u16, u16), verdict: &str) {
    let dbm = |v: Option<i8>| v.map(|x| alloc::format!("{}", x)).unwrap_or_else(|| String::from("-"));
    serial_println!(
        "[wifi10] station roam from={} to={} why={} rssi={}->{} ch={}->{} -> {} — same SSID, no deauth to the old BSS; no tx::prep re-run (STATIONROAM B897, STATIONROAM-XCHAN B898)",
        mac_text(from), mac_text(to), why, dbm(was), dbm(got), ch.0, ch.1, verdict
    );
}

// ── STATIONROAM-XCHAN (rmbp-ledger B898): the channel switch on the kept PHY, and the off-channel dwells ──────────────────────

/// `[wifi10] station tune` lines said so far (the first 8 are said, then only a non-DONE).
static TUNES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The PHY moved to `ch` on a Station TAKEN out of `STATION` (owned by value, so nothing else reaches the d11's indirect PHY/radio
/// ports; no lock held, `tune` sleeps), on the `wifi-station` task: b43's op_config shape — the MAC suspended (`rx::suspend`), the SHM
/// cookie (`rx::chan_cookie`, phy_common.c:425-432), `htphyinit::switch_channel` (`tune` alone), the MAC enabled (`rx::resume`).
/// No one-shot re-run: the TX ring, the receiver, `tx_prep` and WIFI11's power control stay as the pass left them (b43_switch_channel
/// touches no DMA; 2.4 GHz power control is per band). `Station.ch` follows, so every TX header's chan_radio_code matches the cookie
/// (a stale header is the ucode's `chan-mismatch`). A refusal writes no PHY register and puts the cookie back. `Err` = refused or
/// a partial readback (the PHY then says `ch`, and the caller restores or loses).
fn retune(st: &mut Station, ch: u8) -> Result<(), &'static str> {
    let from = st.ch;
    if ch as u16 == from {
        return Ok(());
    }
    let suspended = rx::suspend(st.bar0, &mut st.w, "wifi10-tune");
    let pre = rx::chan_cookie(st.bar0, ch as u16, &mut st.w);
    let r = super::htphyinit::switch_channel(st.bar0, ch as u16);
    if r.is_err() {
        let _ = rx::chan_cookie(st.bar0, from, &mut st.w);
    } else {
        st.ch = ch as u16;
    }
    rx::resume(st.bar0, &mut st.w);
    let (verdict, done, out) = match &r {
        Ok(t) if t.readback_ok == t.readback_n && t.cca_ok => (String::from("DONE"), true, Ok(())),
        Ok(t) if t.readback_ok == t.readback_n => (String::from("PARTIAL"), false, Ok(())),
        Ok(_) => (String::from("PARTIAL"), false, Err("readback")),
        Err(why) => (alloc::format!("REFUSED reason={}", why), false, Err(*why)),
    };
    let n = TUNES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n < 8 || !done {
        let (writes, rb, cca) = match &r {
            Ok(t) => (
                alloc::format!("{}", t.writes),
                if t.readback_ok == t.readback_n { String::from("MATCH") } else { alloc::format!("{}/{}", t.readback_ok, t.readback_n) },
                if t.cca_ok { "rst2rx" } else { "TIMEOUT" },
            ),
            Err(_) => (String::from("0"), String::from("-"), "-"),
        };
        serial_println!(
            "[wifi10] station tune ch={}->{} writes={} readback={} cca={} cookie={:#06x} mac-suspended={} -> {} — phy_common.c:416-441, phy_ht.c:35-77 + :763-805; no one-shot re-run (STATIONROAM-XCHAN B898)",
            from, ch, writes, rb, cca, pre, suspended as u8, verdict
        );
    }
    out
}

/// Inside an OPEN scan window (BEACPROMISC on), on a TAKEN Station: one `ROAM_DWELL_MS` dwell on each channel of
/// `data::dwell_plan(home)` (the grid without the home channel, which the window already heard), `join::note` filling the table with
/// each beacon's DS channel, then the home channel restored. The data path stalls for its length (`stall=`). `why`: `idle` (the
/// gate) or `lost` (the re-join). `Err` only when the home channel could not be restored. Said once per sweep. STATIONROAM-ACTIVE
/// (B951): each dwell is `dwell` — one probe request, then the receiver until `data::dwell_end` (`probes=`, `answered=`).
fn sweep(st: &mut Station, why: &str) -> Result<u32, &'static str> {
    let home = st.ch as u8;
    let t0 = crate::arch::ticks();
    let (plan, n) = super::super::data::dwell_plan(home);
    let mut ok = true; let (mut probes, mut answered) = (0u32, 0u32); // STATIONROAM-ACTIVE (B951): probes SENT, dwells answered
    for c in plan[..n].iter() {
        if retune(st, *c).is_err() {
            ok = false;
            break;
        }
        let (s, a) = dwell(st); probes += s; answered += a; // STATIONROAM-ACTIVE (B951): one probe, then the dwell (early end on an answer)
    }
    let back = retune(st, home);
    let heard = st.seen.join.scan.e.iter().flatten().filter(|s| s.last_ms >= t0 && s.channel != home).count() as u32;
    let stall = crate::arch::ticks().saturating_sub(t0);
    let mut list = String::new();
    for (i, c) in plan[..n].iter().enumerate() {
        if i > 0 {
            list.push(',');
        }
        list.push_str(&alloc::format!("{}", c));
    }
    serial_println!(
        "[wifi10] station sweep why={} home={} plan={} dwell={}ms heard={} stall={}ms probes={} answered={} -> {} — the data path stalls for the sweep; the window's own channel is not re-dwelled (STATIONROAM-XCHAN B898); one probe request per dwell (STATIONROAM-ACTIVE B951)",
        why, home, list, super::super::data::ROAM_DWELL_MS, heard, stall, probes, answered, if ok && back.is_ok() { "done" } else { "failed:tune" }
    );
    back.map(|_| heard)
}

// ── STATIONROAM-ACTIVE (rmbp-ledger B951): one probe request per off-channel dwell, the dwell ended early by an answer ──────────

/// One dwell of `sweep` on the channel the PHY was just retuned to (`Station.ch`; `retune` ended with `rx::resume`, so the MAC is
/// enabled and the TX header's channel matches the cookie), on a TAKEN Station (it owns `prep` by value: always there). ONE broadcast
/// probe request — the one template, `tx::probe_request` (§9.3.3.9, wildcard SSID, DS = this channel), S7's frame — at 1 Mbit/s CCK
/// long, on the Link's next ring frame (`Link::reserve`, `pick_rate`'s shape: the ring never carries a gap) through `tx::fire_frame`
/// (its bounded status spin reads the probe's own status; `tx::Queue` never sees it, so `posted`, the idle gate and the rate
/// controller do not count it). Then the receiver in `ROAM_SLICE_MS` slices until `data::dwell_end`: `ROAM_DWELL_MS` from the fire
/// (the spin counts against it), or `ROAM_PROBE_MS` once a probe response of the SSID to us from a BSS on this channel was noted
/// (`Heard.resp`, cleared before the fire). Returns (1 if the probe was SENT, 1 if the dwell was answered).
fn dwell(st: &mut Station) -> (u32, u32) {
    let ch = st.ch as u8;
    let t0 = crate::arch::ticks();
    st.seen.join.resp = 0;
    let n = st.link.reserve();
    let probe = tx::probe_request(st.link.mac(), st.ch);
    let r = tx::fire_frame(st.bar0, &st.prep, &probe, st.ch, n, "sweep-probe", tx::RATE_1M, &mut st.w);
    loop {
        let el = crate::arch::ticks().saturating_sub(t0);
        if super::super::data::dwell_end(ch, st.seen.join.resp, el) {
            break;
        }
        let left = super::super::data::ROAM_DWELL_MS.saturating_sub(el).min(super::super::data::ROAM_SLICE_MS);
        rx::poll(st.bar0, &mut st.rxr, &mut st.seen, st.ch, left * 1000, &mut st.w);
    }
    (r.sent() as u32, super::super::data::answered(ch, st.seen.join.resp) as u32)
}
