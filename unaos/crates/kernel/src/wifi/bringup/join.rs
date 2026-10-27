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
    pub short: bool,
}

impl Heard {
    pub(super) fn new(me: Mac, ssid: Option<String>) -> Heard {
        Heard { me, target: ssid.map(|s| s.into_bytes()).unwrap_or_default(), bss: None, frames: Vec::new(), watch: None, rates: None, short: false }
    }
}

/// Called by `rx::take` for every clean frame: a beacon / probe response naming the target fixes the BSS;
/// any other management frame whose addr1 (§9.2.4.3) is our MAC is kept for the rung that waits on it.
pub(super) fn hear(frame: &[u8], h: &mut Heard) {
    if frame.len() < 24 || (frame[0] >> 2) & 0x3 != 0 {
        return;
    }
    if let Some(wt) = h.watch.as_mut() {
        wt.frame(frame, h.me, crate::arch::ticks());
        return;
    }
    let sub = frame[0] >> 4;
    if sub == 0x8 || sub == 0x5 {
        if h.bss.is_none() && !h.target.is_empty() {
            h.bss = assoc::bss_match(frame, &h.target);
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
    let r = tx::fire_frame(bar0, p, &frame, ch, FRAME_AUTH, "auth", tx::RATE_1M, w);
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
    let frame = wifi_core::ieee80211::build_assoc_req(me, bss.bssid, &target, 0);
    let r = tx::fire_frame(bar0, p, &frame, ch, FRAME_ASSOC, "assoc-req", tx::RATE_1M, w);
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
        ":: wifiauth: s9 assoc ssid={} bssid={} {} resp={} {} waited={}ms -> {} ::",
        ssid_text(&target), mac_text(bss.bssid), tx_fields(&r), resp, fields, waited, verdict
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
    let gw_text = alloc::format!("{}.{}.{}.{}", gw[0], gw[1], gw[2], gw[3]);
    let mut l = data::Link::new(seen.join.me, bss.bssid);
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
    data::set_link(if reply.is_some() { 1 } else { 2 });
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
        rate = pick_rate(bar0, &prep, seen.join.rates, seen.join.short, ch, &mut link, b.bssid, &mut w);
    }
    seen.data.clear();
    seen.join.frames.clear();
    seen.join.watch = seen.join.bss.map(|b| super::super::data::Watch::new(b.bssid, crate::arch::ticks()));
    let next = link.tx_next(); let mut q = tx::Queue::default(); q.ctl = rate_ctl(seen.join.rates, seen.join.short, rate.rate); // STATIONRATECTL (B881): the rate controller starts at `pick_rate`'s rate, owned by STATION with the queue
    *STATION.lock() = Some(Station { bar0, ch, prep, rxr, seen, link, w, q, lost: None, rate });
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
    st.lost = Some(why);
    st.seen.data.clear();
    let (reason, beacons, tu) = st.seen.join.watch.as_ref().map_or((0, 0, 0), |wt| (wt.reason, wt.beacons, wt.tu));
    let q = &st.q;
    serial_println!(
        "[wifi10] station lost why={} reason={} beacons={} tu={} tx_posted={} tx_sent={} tx_acked={} tx_failed={} tx_stuck={} tx_full={} tx_q={} rx_data={} rate={} ok={} retry={}.{} — the accessor triple answers link-down; tests wifi <ssid> <gw> re-joins (STATIONWATCH B843)",
        why, if why == "beacon-loss" { String::from("-") } else { alloc::format!("{}", reason) }, beacons, tu,
        q.posted, q.sent, q.acked, q.failed, q.stuck, q.full, q.in_flight(), st.link.rx_data, assoc::rate_name(q.ctl.rate()), q.ctl.stats(q.ctl.rate()).0, q.ctl.stats(q.ctl.rate()).1 / 100, q.ctl.stats(q.ctl.rate()).1 % 100 / 10
    );
}

/// From `wifi10()`'s once-per-boot guard: a LOST station is taken out of `STATION` and its engines stopped here, with
/// the lock released (`rx::finish` / `tx::stop` sleep), so the pass may run again. `false` = no station, or a live one.
pub(super) fn rejoin() -> bool {
    let taken = {
        let mut g = STATION.lock();
        if g.as_ref().map_or(false, |st| st.lost.is_some()) { g.take() } else { None }
    };
    let Some(mut st) = taken else { return false };
    let (mac_off, rx_off) = rx::finish(st.bar0, &st.rxr, &mut st.w);
    let tx_off = tx::stop(st.bar0, &mut st.w);
    serial_println!(
        "[wifi10] station rejoin after={} mac-suspended={} rx-reset={} tx-reset={} — the lost station's engines stopped; the pass runs again (STATIONWATCH B843)",
        st.lost.unwrap_or("-"), mac_off as u8, rx_off as u8, tx_off as u8
    );
    true
}

// ── STATIONRATE (rmbp-ledger B855): the station's rate, and the TX header / MACCTL known answers ───────────────────────

/// From `keep`, on the shell task before `STATION` is filled: when the AP listed OFDM 24, ONE Null data frame
/// (`build_null_data`) at OFDM 24 on the Link's next ring frame (`fire_frame`'s bounded status spin) — `probe=ack` iff it was
/// acked at that rate (`Report::acked_at_rate`); then `assoc::station_rate`. STATIONAIR (B862): the rate leaves as ONE
/// `tx::TxRate` — the AP's answer rate (`assoc::ack_rate`) and the short preamble (`assoc::station_short`, the AP's capability
/// bit `short_cap` and a CCK 2/5.5/11 rate). Said once.
fn pick_rate(bar0: u64, p: &tx::Prep, rs: Option<wifi_core::ieee80211::RateSet>, short_cap: bool, ch: u16, link: &mut super::super::data::Link, bssid: Mac, w: &mut rx::Writes) -> tx::TxRate {
    let probe = if assoc::ofdm_listed(rs) {
        let n = link.reserve();
        let null = wifi_core::ieee80211::build_null_data(link.mac(), bssid, 0);
        let r = tx::fire_frame(bar0, p, &null, ch, n, "null-ofdm24", wifi_core::ieee80211::RATE_24M, w);
        if r.acked_at_rate() { "ack" } else { "noack" }
    } else {
        "none"
    };
    let rate = assoc::station_rate(rs, probe == "ack");
    let air = tx::TxRate { rate, ack: assoc::ack_rate(rs, rate), short: assoc::station_short(short_cap, rate), fb: assoc::fallback_rate(rs, rate) };
    serial_println!(
        "[wifi10] station rate={} probe={} probe_at=keep(shell task, before STATION, no lock) ap={} preamble={} ack={} nav={}us — b43 xmit.c:136-189, :884 (STATIONRATE B855); xmit.c:374-375, 802.11-2020 §10.6.6.5.2 (STATIONAIR B862)",
        assoc::rate_name(rate), probe, assoc::rates_text(rs), if air.short { "short" } else { "long" }, assoc::rate_name(air.ack), tx::nav_us(air)
    );
    air
}

/// `tests wifi` (`data::kat` rows 10-14, 17-18): `tx::rate_kat`'s four rows, then the station MACCTL word from S6's `arm` word
/// (ENABLED | INFRA | BEACPROMISC | KEEP_BAD | PROMISC | DISCPMQ -> ENABLED | INFRA | DISCPMQ), then `tx::air_kat`'s two
/// (duration, shortpre; STATIONAIR B862).
pub(in crate::wifi) fn station_kat() -> [bool; 8] {
    let t = tx::rate_kat();
    let a = tx::air_kat();
    [t[0], t[1], t[2], t[3], rx::station_macctl(0x4192_0001) == 0x4002_0001, a[0], a[1], tx::fb_kat()]
}

// ── STATIONRATECTL (rmbp-ledger B881): the kept station's rate follows the ACKs ───────────────────────────────────────

/// From `keep` (shell task, before `STATION`, no lock): the controller over the AP's rate set, starting at `pick_rate`'s rate.
fn rate_ctl(rs: Option<wifi_core::ieee80211::RateSet>, short: bool, start: u8) -> super::super::data::RateCtl {
    super::super::data::RateCtl::new(rs, short, start, crate::arch::ticks())
}

/// A controller rate as ONE `tx::TxRate`, built as `pick_rate` builds its own: the AP's answer rate (`assoc::ack_rate`), the
/// short preamble (`assoc::station_short`), the TX header's fallback (`assoc::fallback_rate`).
fn air_of(c: &super::super::data::RateCtl, rate: u8) -> tx::TxRate {
    tx::TxRate { rate, ack: assoc::ack_rate(c.rs, rate), short: assoc::station_short(c.short, rate), fb: assoc::fallback_rate(c.rs, rate) }
}

/// `station_tx`, under `STATION`: the frame's rate. A group addr1 leaves at the current rate (never ACKed, so never a probe and
/// never counted: `tx::queue` records it as 0); a unicast frame at the controller's next rate.
fn rate_for(st: &mut Station, air: &[u8]) -> tx::TxRate {
    if air.len() < 10 || air[4] & 0x01 != 0 {
        return st.rate;
    }
    let r = st.q.ctl.next_rate();
    if r == st.rate.rate { st.rate } else { air_of(&st.q.ctl, r) }
}

/// `service`, under `STATION`, after the reap: at most one step per second (`RateCtl::decide`); the new rate becomes the
/// station's `TxRate`, said with the old rate's EWMA inputs at the moment of the step.
fn rate_step(st: &mut Station, now: u64) {
    let Some((old, new, why)) = st.q.ctl.decide(now) else { return };
    st.rate = air_of(&st.q.ctl, new);
    let (ok, retry) = st.q.ctl.stats(old);
    serial_println!(
        "[wifi10] station rate -> {}->{} why={} ok={} retry={}.{} — rc80211_minstrel.c shape (EWMA per rate, a sample frame every 16), b43 xmit.c:884 (STATIONRATECTL B881)",
        assoc::rate_name(old), assoc::rate_name(new), why, ok, retry / 100, retry % 100 / 10
    );
}
