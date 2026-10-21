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
}

impl Heard {
    pub(super) fn new(me: Mac, ssid: Option<String>) -> Heard {
        Heard { me, target: ssid.map(|s| s.into_bytes()).unwrap_or_default(), bss: None, frames: Vec::new() }
    }
}

/// Called by `rx::take` for every clean frame: a beacon / probe response naming the target fixes the BSS;
/// any other management frame whose addr1 (§9.2.4.3) is our MAC is kept for the rung that waits on it.
pub(super) fn hear(frame: &[u8], h: &mut Heard) {
    if frame.len() < 24 || (frame[0] >> 2) & 0x3 != 0 {
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
    let r = tx::fire_frame(bar0, p, &frame, ch, FRAME_AUTH, "auth", w);
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
    let r = tx::fire_frame(bar0, p, &frame, ch, FRAME_ASSOC, "assoc-req", w);
    let (reply, waited) = await_reply(bar0, rxr, seen, ch, bss.bssid, w, assoc::assoc_reply);
    let (resp, fields, aid, verdict) = match reply {
        Reply::Got(a) if a.status == 0 => (1, alloc::format!("status=0 aid={}", a.aid), Some(a.aid), String::from("ASSOCIATED")),
        Reply::Got(a) => (1, alloc::format!("status={} aid=-", a.status), None, alloc::format!("NOT-ASSOCIATED reason=status={}", a.status)),
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
        Ok((air, n)) => (tx::fire_frame(bar0, p, &air, ch, n, "data", w), 1),
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

/// Everything the pass armed, kept past it on LINKED: the TX ring, the receiver and its state, the Link.
struct Station {
    bar0: u64,
    ch: u16,
    prep: tx::Prep,
    rxr: rx::Rx,
    seen: rx::Seen,
    link: super::super::data::Link,
    w: rx::Writes,
}

/// THE station. Lock order: smolnet's `STACK` -> `STATION` (smoltcp's poll calls the accessor triple); nothing
/// takes `STATION` then `STACK`. No sleep is taken under it (`rx::drain`, `tx::fire_frame`'s status spin).
static STATION: crate::sync::Mutex<Option<Station>> = crate::sync::Mutex::new(None);

/// From `wifi10()` after its end line, when `link` returned the Link (S10a LINKED): the engines stay armed and
/// move here; from now on only the station's fns below touch them.
pub(super) fn keep(bar0: u64, prep: tx::Prep, rxr: rx::Rx, mut seen: rx::Seen, ch: u16, w: rx::Writes, link: super::super::data::Link) {
    seen.data.clear();
    let next = link.tx_next();
    *STATION.lock() = Some(Station { bar0, ch, prep, rxr, seen, link, w });
    serial_println!("[wifi10] station kept tx_next={} — the TX ring and the receiver stay armed for the stack (WIFIDHCP B831)", next);
}

/// `data::raw_tx`: one Ethernet frame out as a ToDS data frame on the station's next ring frame. `true` = SENT.
pub(in crate::wifi) fn station_tx(eth: &[u8]) -> bool {
    let mut g = STATION.lock();
    let Some(st) = g.as_mut() else { return false };
    match st.link.send(eth) {
        Ok((air, n)) => tx::fire_frame(st.bar0, &st.prep, &air, st.ch, n, "data", &mut st.w).sent(),
        Err(_) => false,
    }
}

/// `data::raw_rx`: the next kept data frame to us as Ethernet; the receiver is drained once when none is kept.
pub(in crate::wifi) fn station_rx(out: &mut [u8]) -> Option<usize> {
    let mut g = STATION.lock();
    let st = g.as_mut()?;
    if let Some(n) = st.link.recv(&mut st.seen.data, out) {
        return Some(n);
    }
    rx::drain(st.bar0, &mut st.rxr, &mut st.seen, st.ch, &mut st.w);
    st.link.recv(&mut st.seen.data, out)
}

/// `data::rx_ready`: the receiver drained once; a data frame is kept. Never waits on the lock.
pub(in crate::wifi) fn station_pending() -> bool {
    let Some(mut g) = STATION.try_lock() else { return false };
    let Some(st) = g.as_mut() else { return false };
    if st.seen.data.is_empty() {
        rx::drain(st.bar0, &mut st.rxr, &mut st.seen, st.ch, &mut st.w);
    }
    !st.seen.data.is_empty()
}

/// `data::hw_addr` / `data::kept`: the station MAC while a station is kept.
pub(in crate::wifi) fn station_mac() -> Option<Mac> {
    STATION.lock().as_ref().map(|st| st.link.mac())
}
