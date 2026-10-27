// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (rung S10a, the station's data path over the open link S9 left ASSOCIATED, judged
//! over byte slices; the frames themselves are the shared-core codec `wifi_core::ieee80211`, never re-built here).
//!
//! WIFIDATA (rmbp-ledger B803) — the PURE half of the data path: an Ethernet II frame to the AP as an 802.11
//! data frame (`to_air`: IEEE Std 802.11-2020 §9.3.2.1, ToDS, addr1 = BSSID, addr2 = STA, addr3 = DA, the
//! RFC 1042 LLC/SNAP header of §5.1.4) and back (`from_air`: FromDS, addr1 = STA, addr2 = BSSID, addr3 = SA).
//! The codec is `wifi_core::ieee80211::ethernet_to_data` / `data_to_ethernet`; this file is the JUDGE that
//! codec does not carry (subtype 0 only — a QoS Data frame's header is 26 octets; FromDS and not ToDS; the
//! addresses; the full 6-octet SNAP). `Link` is the station's three `RawNic` operations (send, recv, mac) as
//! inherent fns over the ring numbering; it lives inside `bringup::wifi10()`'s pass, and — WIFIDHCP (B831) — when
//! S10a read LINKED the pass does not stop the engines: `bringup/join.rs::keep` moves the Link, both rings and the
//! receiver state into the one kept station, and `raw_rx` / `raw_tx` / `hw_addr` below are its accessor triple,
//! folded as the next fallback on `drivers/e1000.rs`'s three accessor lines (usbnet's shape, SO56), so smolnet's
//! one phy adapter and its one DHCP client (`smolnet::init` -> `dhcp_acquire`) ride the station when no e1000 and
//! no USB Ethernet answered (BP-net-1, BP-net-16). The device half — the frame out through WIFI10's ring, the
//! answer drained from S6's receiver — is `bringup/join.rs` (`link`, `station_*`). One ARP request (RFC 826) for
//! the gateway the operator named is S10a; smoltcp's DHCP over the kept station is S10b (`verb.rs::wifidhcp`).
//!
//! The KATs are hand-assembled from the clause layouts (no captured data frame exists in `docs/dev/evidence`),
//! so `n=23/23` (STATIONRATECTL B881: `ratectl`, `fallback`; STATIONERP B884: `erp`, `cts`) certifies the judge, the codec, the 1514-octet frame cap, the addr1 filter and the link watch (STATIONWATCH
//! B843), the TX header's rate fields, the station's MACCTL word, the AP's rate set and the probe's Null frame (STATIONRATE
//! B855), the NAV, the short preamble and the DHCP lease seconds (STATIONAIR B862; the `lease` row exists only where smolnet
//! does) against the standard's and b43's field order, not a replay of a capture.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use wifi_core::ieee80211::{self as w, Mac};

use super::bringup::{FRAMES, FRAME_MAX};

/// The first ring frame for data: 0 probe, 1 auth, 2 assoc (`join.rs`); the Link then cycles the ring (`tx.rs` `FRAMES`).
const FRAME_DATA0: usize = 3;
/// Data frames kept between polls by `rx::take` (`keep`); `Link::recv` drains them (the kept station drains often).
const KEEP: usize = 16;
/// FC octet 0 of a Data frame, subtype 0, protocol version 0 (§9.2.4.1.3): `08`.
const FC0_DATA: u8 = (w::SUBTYPE_DATA << 4) | (w::FC_TYPE_DATA << 2);
/// FC octet 1 (§9.2.4.1.1): ToDS = bit 0, FromDS = bit 1.
const FC1_TODS: u8 = 0x01;
const FC1_FROMDS: u8 = 0x02;

/// An Ethernet II frame as the 802.11 data frame to the AP (ToDS): `ethernet_to_data`, then the ring's cap (`tx.rs` `FRAME_MAX`,
/// the 2 KiB body region since STATIONMTU B843: a full 1514-octet Ethernet frame is 1532 octets of air).
/// `None` = shorter than an Ethernet header, or longer than one TX ring frame (`Link` counts it `tx_toolong`).
pub fn to_air(eth: &[u8], sta: Mac, bssid: Mac, seq: u16) -> Option<Vec<u8>> {
    let f = w::ethernet_to_data(eth, sta, bssid, seq)?;
    if f.len() > FRAME_MAX {
        return None;
    }
    Some(f)
}

/// A received frame judged as the AP's data frame to us, then `data_to_ethernet`: Data subtype 0 (§9.2.4.1.3),
/// FromDS = 1 and ToDS = 0 (§9.2.4.1.4), addr1 = `sta` or a group address (STATIONRX B843: the BSS's broadcast /
/// multicast, e.g. the gateway's ARP request for us) and addr2 = `bssid` (§9.3.2.1, Table 9-26), and the
/// full RFC 1042 SNAP header (AA AA 03 00 00 00, §5.1.4). Anything else is `None` (the codec refuses Protected).
pub fn from_air(frame: &[u8], sta: Mac, bssid: Mac) -> Option<Vec<u8>> {
    if frame.len() < 24 + 8 || frame[0] != FC0_DATA {
        return None;
    }
    if frame[1] & (FC1_TODS | FC1_FROMDS) != FC1_FROMDS {
        return None;
    }
    if !to_us(frame, sta) || frame[10..16] != bssid || frame[24..30] != w::SNAP_PREFIX {
        return None;
    }
    w::data_to_ethernet(frame)
}

/// RFC 826 over Ethernet II: an ARP request (EtherType 0x0806; htype 1, ptype 0x0800, hlen 6, plen 4, op 1),
/// broadcast, the target MAC zero — 42 octets. With `sender_ip` 0.0.0.0 it is an ARP probe (RFC 5227 §2.1.1),
/// the only honest sender address before a lease.
pub fn arp_request(sta: Mac, sender_ip: [u8; 4], target_ip: [u8; 4]) -> [u8; 42] {
    let mut f = [0u8; 42];
    f[0..6].copy_from_slice(&[0xFF; 6]);
    f[6..12].copy_from_slice(&sta);
    f[12..14].copy_from_slice(&[0x08, 0x06]);
    f[14..22].copy_from_slice(&[0x00, 0x01, 0x08, 0x00, 6, 4, 0x00, 0x01]);
    f[22..28].copy_from_slice(&sta);
    f[28..32].copy_from_slice(&sender_ip);
    f[38..42].copy_from_slice(&target_ip);
    f
}

/// RFC 826: an ARP reply (op 2, Ethernet/IPv4) — the sender's hardware and protocol addresses, else `None`.
/// The caller matches the sender address against the gateway it asked for; the frame reached us through
/// `from_air`, so its addr1 was our MAC.
pub fn arp_reply_parse(eth: &[u8]) -> Option<(Mac, [u8; 4])> {
    if eth.len() < 42 || eth[12..14] != [0x08, 0x06] || eth[14..22] != [0x00, 0x01, 0x08, 0x00, 6, 4, 0x00, 0x02] {
        return None;
    }
    let mut sha = [0u8; 6];
    sha.copy_from_slice(&eth[22..28]);
    let mut spa = [0u8; 4];
    spa.copy_from_slice(&eth[28..32]);
    Some((sha, spa))
}

/// `tests wifi <ssid> <gw>`: the gateway word as a dotted quad, else `None`.
pub fn ipv4(word: Option<&str>) -> Option<[u8; 4]> {
    let mut ip = [0u8; 4];
    let mut parts = word?.split('.');
    for b in ip.iter_mut() {
        *b = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(ip)
}

/// addr1 (§9.2.4.3) names this station: our MAC, or a group address (the I/G bit, bit 0 of octet 0, §9.2.4.3.2).
fn to_us(frame: &[u8], sta: Mac) -> bool {
    frame[4..10] == sta || frame[4] & 0x01 != 0
}

/// Called by `rx::take` for every clean frame: a Data-type frame (§9.2.4.1.3) whose addr1 is `sta` or a group address
/// (STATIONRX B843: the receiver stays promiscuous, so the filter is here) is kept, at most `KEEP`, for `Link::recv`
/// to judge; nothing else is.
pub fn keep(frame: &[u8], sta: Mac, q: &mut Vec<Vec<u8>>) {
    if frame.len() >= 24 && (frame[0] >> 2) & 0x3 == w::FC_TYPE_DATA && to_us(frame, sta) && q.len() < KEEP {
        q.push(frame.to_vec());
    }
}

/// The station's link for one pass: the `RawNic` seam's three operations (send, recv, mac) as inherent fns.
/// Built after S9 read ASSOCIATED; dropped before the engines stop, or kept with them (`join::keep`) on LINKED. It
/// owns the ring numbering (frame 3 on, cycling `FRAMES`) and the sequence number (§9.2.4.4; the ucode overwrites
/// it, `tx.rs` `TXH_MAC_HWSEQ`).
pub struct Link {
    sta: Mac,
    bssid: Mac,
    seq: u16,
    tx_next: usize,
    pub rx_data: u32,
    pub tx_toolong: u32,
}

impl Link {
    pub fn new(sta: Mac, bssid: Mac) -> Link {
        Link { sta, bssid, seq: 0, tx_next: FRAME_DATA0, rx_data: 0, tx_toolong: 0 }
    }

    /// `RawNic::transmit`'s half: the air frame for `eth` and the ring frame number it goes out on.
    pub fn send(&mut self, eth: &[u8]) -> Result<(Vec<u8>, usize), &'static str> {
        let Some(f) = to_air(eth, self.sta, self.bssid, self.seq) else {
            self.tx_toolong += 1;
            return Err("too-long");
        };
        Ok((f, self.reserve()))
    }

    /// The next ring frame number, taken (and the sequence number advanced): `send`'s numbering, and the kept station's one
    /// OFDM probe frame (STATIONRATE B855), so the ring never carries a gap.
    pub fn reserve(&mut self) -> usize {
        let n = self.tx_next;
        self.tx_next = (self.tx_next + 1) % FRAMES;
        self.seq = (self.seq + 1) & 0x0FFF;
        n
    }

    /// `RawNic::rx_frame_raw`'s half: the next kept frame that `from_air` accepts, as Ethernet, into `out`
    /// (length-clamped); frames the judge refuses are dropped.
    pub fn recv(&mut self, q: &mut Vec<Vec<u8>>, out: &mut [u8]) -> Option<usize> {
        while !q.is_empty() {
            let f = q.remove(0);
            if let Some(eth) = from_air(&f, self.sta, self.bssid) {
                self.rx_data += 1;
                let n = eth.len().min(out.len());
                out[..n].copy_from_slice(&eth[..n]);
                return Some(n);
            }
        }
        None
    }

    /// `RawNic::mac`'s half: the SPROM station MAC.
    pub fn mac(&self) -> Mac {
        self.sta
    }

    /// The next ring frame `send` will use (the kept station's line).
    pub fn tx_next(&self) -> usize {
        self.tx_next
    }
}

// ── KAT fixtures: STA 02:55:4e:41:4f:53, BSSID 00:11:22:33:44:55, gateway 00:11:22:33:44:66 at 192.168.1.1 ──
const STA: Mac = [0x02, 0x55, 0x4e, 0x41, 0x4f, 0x53];
const BSSID: Mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
const GW_MAC: Mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x66];
const GW_IP: [u8; 4] = [192, 168, 1, 1];

/// `tests wifi` — the judge and the codec against hand-assembled frames. `(pass, total, first failing name)`.
pub fn kat() -> (u8, u8, Option<&'static str>) {
    let mut checks: Vec<(&'static str, bool)> = Vec::new();
    // 1. to-air: the 42-octet ARP probe as a ToDS data frame — FC 08 01, duration 0, addr1 BSSID, addr2 STA,
    //    addr3 the broadcast DA, sequence 0; SNAP AA AA 03 00 00 00 08 06; the 28-octet ARP body. 24 + 8 + 28 = 60.
    let mut want = alloc::vec![0x08, 0x01, 0x00, 0x00];
    want.extend_from_slice(&BSSID);
    want.extend_from_slice(&STA);
    want.extend_from_slice(&[0xFF; 6]);
    want.extend_from_slice(&[0x00, 0x00, 0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x08, 0x06]);
    want.extend_from_slice(&[0x00, 0x01, 0x08, 0x00, 0x06, 0x04, 0x00, 0x01]);
    want.extend_from_slice(&STA);
    want.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    want.extend_from_slice(&GW_IP);
    checks.push(("to-air", to_air(&arp_request(STA, [0; 4], GW_IP), STA, BSSID, 0) == Some(want.clone()) && want.len() == 60));
    // 2. from-air: the gateway's ARP reply as a FromDS data frame — FC 08 02, addr1 STA, addr2 BSSID, addr3 the
    //    SA; back to Ethernet (DA STA, SA gateway, 08 06) and the reply's sender read.
    let mut body = alloc::vec![0x00, 0x01, 0x08, 0x00, 0x06, 0x04, 0x00, 0x02];
    body.extend_from_slice(&GW_MAC);
    body.extend_from_slice(&GW_IP);
    body.extend_from_slice(&STA);
    body.extend_from_slice(&[0, 0, 0, 0]);
    let mut rx = alloc::vec![0x08, 0x02, 0x3a, 0x01];
    rx.extend_from_slice(&STA);
    rx.extend_from_slice(&BSSID);
    rx.extend_from_slice(&GW_MAC);
    rx.extend_from_slice(&[0x10, 0x00, 0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x08, 0x06]);
    rx.extend_from_slice(&body);
    let mut eth = Vec::new();
    eth.extend_from_slice(&STA);
    eth.extend_from_slice(&GW_MAC);
    eth.extend_from_slice(&[0x08, 0x06]);
    eth.extend_from_slice(&body);
    checks.push(("from-air", from_air(&rx, STA, BSSID) == Some(eth.clone()) && arp_reply_parse(&eth) == Some((GW_MAC, GW_IP))));
    // 3. qos: QoS Data (FC 88 02) carries a 26-octet header — refused, not mis-decoded.
    let mut q = rx.clone();
    q[0] = 0x88;
    checks.push(("qos", from_air(&q, STA, BSSID).is_none()));
    // 4. tods: our own direction (FC 08 01) is not a frame to us.
    let mut t = rx.clone();
    t[1] = FC1_TODS;
    checks.push(("tods", from_air(&t, STA, BSSID).is_none()));
    // 5. addr1: a frame to another station.
    let mut a = rx.clone();
    a[9] ^= 0x01;
    checks.push(("addr1", from_air(&a, STA, BSSID).is_none()));
    // 6. snap: AA AA 03 00 00 F8 (the 802.1H bridge-tunnel OUI) — the codec reads three octets; the judge six.
    let mut s = rx.clone();
    s[29] = 0xF8;
    checks.push(("snap", from_air(&s, STA, BSSID).is_none()));
    // 7. full-frame (STATIONMTU B843): a 1514-octet Ethernet frame (IPv4, 1500 octets of payload) -> 1514 - 14 + 24 + 8 =
    //    1532 octets of air, within the ring's body region, FC 08 01, SNAP ending 08 00.
    let mut big = alloc::vec![0x5Au8; 1514];
    big[0..6].copy_from_slice(&GW_MAC);
    big[6..12].copy_from_slice(&STA);
    big[12..14].copy_from_slice(&[0x08, 0x00]);
    checks.push(("full-frame", matches!(to_air(&big, STA, BSSID, 0), Some(f) if f.len() == 1532 && f.len() <= FRAME_MAX
        && f[0] == 0x08 && f[1] == FC1_TODS && f[30..32] == [0x08, 0x00] && f[1531] == 0x5A)));
    // 8. keep (STATIONRX B843): the reply to STA is kept; the same frame to another unicast station is not; to broadcast
    //    it is kept and `from_air` accepts it (DA broadcast).
    let mut kq: Vec<Vec<u8>> = Vec::new();
    keep(&rx, STA, &mut kq);
    keep(&a, STA, &mut kq);
    let mut b = rx.clone();
    b[4..10].copy_from_slice(&[0xFF; 6]);
    keep(&b, STA, &mut kq);
    checks.push(("keep", kq.len() == 2 && kq[0] == rx && kq[1] == b
        && matches!(from_air(&b, STA, BSSID), Some(e) if e[0..6] == [0xFF; 6] && e[6..12] == GW_MAC)));
    // 9. watch (STATIONWATCH B843): a Deauthentication (FC c0 00, reason 7) from the BSSID to STA -> deauth; a
    //    Disassociation (FC a0 00, reason 8) -> disassoc; a Beacon (interval 100 TU) then 100 ms ticks: none at 500 ms,
    //    beacon-loss past 10 intervals (1024 ms); one 5 s gap between ticks restarts the clock.
    let mgmt = |fc0: u8, a1: Mac, tail: &[u8]| {
        let mut f = alloc::vec![fc0, 0x00, 0x00, 0x00];
        f.extend_from_slice(&a1);
        f.extend_from_slice(&BSSID);
        f.extend_from_slice(&BSSID);
        f.extend_from_slice(&[0x20, 0x00]);
        f.extend_from_slice(tail);
        f
    };
    let deauth = mgmt(0xC0, STA, &[7u8, 0][..]);
    let disassoc = mgmt(0xA0, STA, &[8u8, 0][..]);
    let beacon = mgmt(FC0_BEACON, [0xFF; 6], &[0u8, 0, 0, 0, 0, 0, 0, 0, 100, 0, 0x01, 0x00][..]);
    let mut wd = Watch::new(BSSID, 0);
    wd.frame(&deauth, STA, 10);
    let mut wa = Watch::new(BSSID, 0);
    wa.frame(&disassoc, STA, 10);
    let mut wb = Watch::new(BSSID, 0);
    wb.frame(&beacon, STA, 0);
    let mut at500 = None;
    let mut lost_at = 0u64;
    for t in (100..=1200u64).step_by(100) {
        let r = wb.tick(t);
        if t == 500 {
            at500 = r;
        }
        if r.is_some() && lost_at == 0 {
            lost_at = t;
        }
    }
    let mut wg = Watch::new(BSSID, 0);
    let gap = (wg.tick(100), wg.tick(5100), wg.tick(5200));
    checks.push(("watch", wd.tick(20) == Some("deauth") && wd.reason == 7 && wa.tick(20) == Some("disassoc") && wa.reason == 8
        && wb.beacons == 1 && wb.tu == 100 && at500.is_none() && lost_at == 1100 && wb.tick(1300) == Some("beacon-loss")
        && gap == (None, None, None)));
    // 10-14 (STATIONRATE B855): the TX header's rate fields and the station's MACCTL word (`bringup/join.rs::station_kat`).
    let k = super::bringup::station_kat();
    checks.push(("plcp-cck1", k[0]));
    checks.push(("plcp-cck11", k[1]));
    checks.push(("plcp-ofdm24", k[2]));
    checks.push(("txhdr-rate", k[3]));
    checks.push(("macctl", k[4]));
    // 15. rates: an 11g association response (Supported 1*,2*,5.5*,11*,6,9,12,18; Extended 24,36,48,54) -> CCK 11 without
    //     the probe's ack, OFDM 24 with it; an 11b-only one -> CCK 11 even "acked"; a malformed IE list (the Extended
    //     element declares 4 octets, carries 2) and one with no rate element -> unread -> CCK 1, never a panic.
    let mut ar = alloc::vec![0x10, 0x00, 0x3a, 0x01];
    ar.extend_from_slice(&STA);
    ar.extend_from_slice(&BSSID);
    ar.extend_from_slice(&BSSID);
    ar.extend_from_slice(&[0x20, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0xc0]);
    let mut b_only = ar.clone();
    b_only.extend_from_slice(&[0x01, 0x04, 0x82, 0x84, 0x8b, 0x96]);
    let mut g = b_only.clone();
    g[31] = 0x08;
    g.extend_from_slice(&[0x0c, 0x12, 0x18, 0x24, 0x32, 0x04, 0x30, 0x48, 0x60, 0x6c]);
    let mut bad = g.clone();
    bad.truncate(bad.len() - 2);
    let rg = super::assoc::assoc_reply_rates(&g, STA, BSSID).and_then(|(_, r)| r);
    let rb = super::assoc::assoc_reply_rates(&b_only, STA, BSSID).and_then(|(_, r)| r);
    let rbad = super::assoc::assoc_reply_rates(&bad, STA, BSSID);
    let rnone = super::assoc::assoc_reply_rates(&ar, STA, BSSID);
    checks.push(("rates", matches!(rg, Some(r) if r.listed == 0x0FFF && r.basic == 0x000F)
        && super::assoc::station_rate(rg, false) == w::RATE_11M && super::assoc::station_rate(rg, true) == w::RATE_24M
        && super::assoc::ofdm_listed(rg) && !super::assoc::ofdm_listed(rb)
        && super::assoc::station_rate(rb, true) == w::RATE_11M
        && matches!(rbad, Some((a, None)) if a.status == 0) && super::assoc::station_rate(None, true) == w::RATE_1M
        && matches!(rnone, Some((_, None))) && super::assoc::rates_text(rg) == "1*,2*,5.5*,11*,6,9,12,18,24,36,48,54"));
    // 16. null: the probe's Null data frame (§9.3.2.1 subtype 4) — FC 48 01, duration 0, BSSID, STA, BSSID, seq 0; 24 octets.
    let mut nul = alloc::vec![0x48, 0x01, 0x00, 0x00];
    nul.extend_from_slice(&BSSID);
    nul.extend_from_slice(&STA);
    nul.extend_from_slice(&BSSID);
    nul.extend_from_slice(&[0x00, 0x00]);
    checks.push(("null", w::build_null_data(STA, BSSID, 0) == nul && nul.len() == 24));
    // 17. duration (STATIONAIR B862): `tx::air_kat`'s NAV row, then the AP's answer rate (§10.6.6.5.2): the 11g set above
    //     (basic 1,2,5.5,11) answers CCK 11 at 11 and OFDM 24 at 24 (no basic OFDM: the mandatory 24); basic 1,2 answers CCK 11
    //     at 2; basic 6,12 answers OFDM 24 at 12; unread answers CCK 5.5 at 5.5; an unknown rate answers CCK 1.
    let b12 = w::RateSet { listed: 0x000F, basic: 0x0003 };
    let b6_12 = w::RateSet { listed: 0x0FFF, basic: 0x000F | (1 << 4) | (1 << 6) };
    checks.push(("duration", k[5] && super::assoc::ack_rate(rg, w::RATE_11M) == w::RATE_11M
        && super::assoc::ack_rate(rg, w::RATE_24M) == w::RATE_24M && super::assoc::ack_rate(Some(b12), w::RATE_11M) == 4
        && super::assoc::ack_rate(Some(b6_12), w::RATE_24M) == 24 && super::assoc::ack_rate(None, 11) == 11
        && super::assoc::ack_rate(None, 7) == w::RATE_1M));
    // 18. shortpre (STATIONAIR B862): `tx::air_kat`'s phy_ctl row, then the capability bit of the 11g response with
    //     capability 0x0021 (ESS + Short Preamble) read set and with 0x0001 read clear; the station uses it for CCK 2/5.5/11 only.
    let mut gs = g.clone();
    gs[24] = 0x21;
    checks.push(("shortpre", k[6] && matches!(super::assoc::assoc_reply_air(&gs, STA, BSSID), Some((a, Some(_), true)) if a.status == 0)
        && !super::assoc::short_preamble(&g) && super::assoc::station_short(true, false, w::RATE_11M) && super::assoc::station_short(true, false, 4)
        && !super::assoc::station_short(true, false, w::RATE_1M) && !super::assoc::station_short(true, false, w::RATE_24M)
        && !super::assoc::station_short(false, false, w::RATE_11M) && !super::assoc::station_short(true, true, w::RATE_11M)));
    // 19. lease (STATIONLEASE B862): a BOOTP reply (op 2, Ethernet, hlen 6, magic 63 82 53 63 at 236) carrying DHCP message
    //     type 5 (ACK), mask 255.255.255.0 and option 51 = 3600 s, read through smoltcp's own parser (`smolnet::lease_of`) ->
    //     3600; the same as an OFFER (type 2) -> unread; a broken magic cookie -> unread.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    {
        let mut ack = alloc::vec![0u8; 236];
        ack[0] = 2;
        ack[1] = 1;
        ack[2] = 6;
        ack[16..20].copy_from_slice(&[192, 168, 1, 50]);
        ack[28..34].copy_from_slice(&STA);
        ack.extend_from_slice(&[0x63, 0x82, 0x53, 0x63, 53, 1, 5, 1, 4, 255, 255, 255, 0, 51, 4, 0x00, 0x00, 0x0E, 0x10, 255]);
        let mut offer = ack.clone();
        offer[242] = 2;
        let mut bad = ack.clone();
        bad[236] = 0x00;
        checks.push(("lease", crate::smolnet::lease_of(&ack) == Some(3600) && crate::smolnet::lease_of(&offer).is_none()
            && crate::smolnet::lease_of(&bad).is_none()));
    }
    ratectl_rows(&mut checks); erp_rows(&mut checks); let mut pass = 0u8; // STATIONRATECTL (B881): `ratectl`, `fallback` at the END of the table (LINE-NEUTRAL); STATIONERP (B884): `erp`, `cts` after them
    let mut first = None;
    for (name, ok) in checks.iter() {
        if *ok {
            pass += 1;
        } else if first.is_none() {
            first = Some(*name);
        }
    }
    (pass, checks.len() as u8, first)
}

/// S10a this boot: 0 not run, 1 LINKED (the gateway's ARP reply heard), 2 NOT-LINKED, 3 not attempted.
static LINK: AtomicU8 = AtomicU8::new(0);

pub fn set_link(v: u8) {
    LINK.store(v, Ordering::Relaxed);
}

/// `ladder.rs`: the S10a row is live when this boot's rung read LINKED.
pub fn linked() -> bool {
    LINK.load(Ordering::Relaxed) == 1
}

/// `tests wifi` (verb.rs), after `assoc::witness`: the KAT line, then S10a's line when the pass never reached it.
pub fn witness() {
    let (p, t, first) = kat();
    serial_println!(
        ":: wifidata: kat n={}/{} -> {}{} ::",
        p, t, if first.is_none() { "PASS" } else { "FAIL" }, first.map(|f| alloc::format!(" first={}", f)).unwrap_or_default()
    );
    if LINK.load(Ordering::Relaxed) == 0 {
        serial_println!(":: wifidata: s10a link NOT ATTEMPTED reason=s9-absent -> ABSENT ::");
    }
}

// ── WIFIDHCP (rmbp-ledger B831): the kept station as the stack's link — usbnet's accessor-triple shape ──────────

/// The stack was built on the station: `hw_addr` answered (it is reached only when e1000 and usbnet both answered
/// `None` — `drivers/e1000.rs`'s fallback chain is lazy), so smoltcp's frames carry the station MAC.
static BOUND: AtomicBool = AtomicBool::new(false);

/// The station outlived `wifi10()`'s pass (S10a LINKED; `bringup/join.rs::keep`) and its link is up (STATIONWATCH B843: a
/// station that heard a deauth / disassoc or lost the beacons is kept no more).
pub fn kept() -> bool {
    super::bringup::station_mac().map_or(false, |(_, up)| up)
}

/// The smoltcp stack rides the station (see `BOUND`).
pub fn bound() -> bool {
    BOUND.load(Ordering::Relaxed)
}

/// `e1000::hw_addr`'s fallback after usbnet's: (the SPROM MAC, 0.0.0.0 — the only honest address before a lease,
/// RFC 2131 §4.1 — link up, or link DOWN once the station's watch dropped it, STATIONWATCH B843), or `None` while no
/// station is kept.
pub fn hw_addr() -> Option<([u8; 6], [u8; 4], bool)> {
    let (m, up) = super::bringup::station_mac()?;
    BOUND.store(true, Ordering::Relaxed);
    Some((m, [0; 4], up))
}

/// `e1000::raw_rx`'s fallback after usbnet's: the next data frame to us as Ethernet.
pub fn raw_rx(out: &mut [u8]) -> Option<usize> {
    if !bound() {
        return None;
    }
    super::bringup::station_rx(out)
}

/// `e1000::raw_tx`'s fallback ahead of usbnet's: `true` = the station owns the stack and took the frame (sent or
/// counted `tx_toolong`); `false` = not ours, the chain goes on.
pub fn raw_tx(frame: &[u8]) -> bool {
    if !bound() {
        return false;
    }
    super::bringup::station_tx(frame);
    true
}

/// smolnet's poll gate (`rx_ready`): the receiver drained once and a data frame kept.
pub fn rx_ready() -> bool {
    bound() && super::bringup::station_pending()
}

/// smolnet's `tx_ready` for the station (STATIONTX B843): room on the station's asynchronous TX ring right now (its
/// statuses reaped first); `false` = back-pressure, smoltcp keeps the packet for a later poll.
pub fn tx_ready() -> bool {
    bound() && super::bringup::station_tx_room()
}

/// S10b this boot: 0 not run, 1 bound (a lease over the station), 2 SILENT, 3 not attempted.
static DHCP: AtomicU8 = AtomicU8::new(0);

pub fn set_dhcp(v: u8) {
    DHCP.store(v, Ordering::Relaxed);
}

/// `ladder.rs`: the S10b row is live when this boot's lease came over the station.
pub fn dhcp_bound() -> bool {
    DHCP.load(Ordering::Relaxed) == 1
}

/// `tests wifi`: the DISCOVER fits the ring and survives the Link both ways. The frame is the hand-rolled stack's
/// full-shape DISCOVER (`net::dhcp::build_discover_rfc`, 14 + 20 + 8 + 300 = 342 octets — never re-built here);
/// `to_air` must give 342 - 14 + 24 + 8 = 360 octets <= `FRAME_MAX`, FC 08 01, SNAP ending 08 00. The STUB: the
/// Link's own `send` output mirrored as the AP's FromDS frame (addr1 STA, addr2 BSSID, addr3 the SA) into its own
/// `recv` must return the 342 octets with the payload unchanged. `(kat_ok, stub_ok, discover_len, first miss)`.
pub fn kat_dhcp() -> (bool, bool, usize, Option<&'static str>) {
    let mut disc = [0u8; 342];
    let n = net::dhcp::build_discover_rfc(&mut disc, 0x5741_4446, 0, STA).unwrap_or(0);
    let air = to_air(&disc[..n], STA, BSSID, 0);
    let kat_ok = n == 342
        && matches!(&air, Some(f) if f.len() == 360 && f.len() <= FRAME_MAX && f[0] == 0x08 && f[1] == FC1_TODS
            && f[24..30] == w::SNAP_PREFIX && f[30..32] == [0x08, 0x00]);
    let mut l = Link::new(STA, BSSID);
    let mut q: Vec<Vec<u8>> = Vec::new();
    let mut back = [0u8; 512];
    let stub_ok = match l.send(&disc[..n]) {
        Ok((mut f, ring)) if ring == FRAME_DATA0 => {
            f[1] = FC1_FROMDS;
            f[4..10].copy_from_slice(&STA);
            f[10..16].copy_from_slice(&BSSID);
            f[16..22].copy_from_slice(&STA);
            keep(&f, STA, &mut q);
            matches!(l.recv(&mut q, &mut back), Some(m) if m == n && back[..6] == STA && back[6..12] == STA && back[12..m] == disc[12..n])
        }
        _ => false,
    };
    let first = if n != 342 { Some("discover-len") } else if !kat_ok { Some("to-air") } else if !stub_ok { Some("stub") } else { None };
    (kat_ok, stub_ok, n, first)
}

// ── STATIONWATCH (rmbp-ledger B843): the kept station's link watch — pure, fed by `join::hear`, ticked on the station's
// own drains under `STATION` (no timer task, no sleep) ────────────────────────────────────────────────────────────

/// FC octet 0 of a Beacon (§9.2.4.1.3): management, subtype 8.
const FC0_BEACON: u8 = w::SUBTYPE_BEACON << 4;
/// Beacon loss: no beacon from the BSSID for this many beacon intervals.
pub const LOST_BEACONS: u64 = 10;
/// The beacon interval assumed until a beacon names its own (§9.4.1.3, in TU of 1024 us).
const TU_DEFAULT: u16 = 100;

/// The link watch: a Deauthentication / Disassociation from the BSSID to the station (`assoc::refusal`, §9.3.3.12 /
/// §9.3.3.4) or `LOST_BEACONS` beacon intervals with no beacon from the BSSID ends the link.
pub struct Watch {
    bssid: Mac,
    /// The beacon interval in TU, from the last beacon's field (§9.3.3.3: octets 32..34 after the 8-octet timestamp).
    pub tu: u16,
    heard_ms: u64,
    tick_ms: u64,
    pub beacons: u32,
    /// The refusal's reason code (§9.4.1.7); 0 for beacon loss.
    pub reason: u16,
    lost: Option<&'static str>,
}

impl Watch {
    pub fn new(bssid: Mac, now_ms: u64) -> Watch {
        Watch { bssid, tu: TU_DEFAULT, heard_ms: now_ms, tick_ms: now_ms, beacons: 0, reason: 0, lost: None }
    }

    fn window_ms(&self) -> u64 {
        LOST_BEACONS * self.tu as u64 * 1024 / 1000
    }

    /// Every clean management frame the receiver took: a Beacon whose addr3 is the BSSID restarts the clock (its interval
    /// field read when the frame carries it — a short beacon keeps the last interval); a refusal to `sta` is the loss.
    pub fn frame(&mut self, f: &[u8], sta: Mac, now_ms: u64) {
        if self.lost.is_some() || f.len() < 24 {
            return;
        }
        if f[0] == FC0_BEACON && f[16..22] == self.bssid {
            if f.len() >= 34 {
                let tu = u16::from_le_bytes([f[32], f[33]]);
                if tu != 0 {
                    self.tu = tu;
                }
            }
            self.beacons += 1;
            self.heard_ms = now_ms;
            return;
        }
        if let Some((why, reason)) = super::assoc::refusal(f, sta, self.bssid) {
            self.lost = Some(why);
            self.reason = reason;
        }
    }

    /// After each drain: the loss (`deauth` / `disassoc` / `beacon-loss`), else `None`. A gap between ticks longer than
    /// half the window restarts the clock — an undrained receiver is not evidence that the beacons stopped.
    pub fn tick(&mut self, now_ms: u64) -> Option<&'static str> {
        if self.lost.is_none() {
            let win = self.window_ms();
            if now_ms.saturating_sub(self.tick_ms) > win / 2 {
                self.heard_ms = now_ms;
            } else if now_ms.saturating_sub(self.heard_ms) > win {
                self.lost = Some("beacon-loss");
            }
            self.tick_ms = now_ms;
        }
        self.lost
    }
}

// ── STATIONRATECTL (rmbp-ledger B881): the kept station's rate follows the ACKs ─────────────────────────────────────

/// Unicast frames between two probes at the next rate up (minstrel's sample frame, `rc80211_minstrel.c`, the reference only).
const PROBE_EVERY: u32 = 16;
/// Consecutive frames at the current rate not acked within the long-retry tries before a step down (`fail`).
const FAIL_K: u32 = 4;
/// Frames at the current rate before its EWMA (success < 50%) may step it down (`window`).
const WINDOW: u32 = 16;
/// Consecutive successes at the current rate an acked probe needs before a step up (`probe-ok`).
const UP_N: u32 = 8;
/// The least time between two steps, so at most one `[wifi10] station rate ->` line per second.
const HOLD_MS: u64 = 1000;

/// The kept station's rate controller — PURE (`Watch`'s shape): `tx::reap`, the one TX-status reader, feeds it per statused
/// unicast frame (`frame`); `join::rate_for` asks it the next frame's rate (`next_rate`, every `PROBE_EVERY`th unicast frame
/// one rate up); `join::rate_step` asks it for a step (`decide`). It lives in `tx::Queue` inside the station, so the one
/// `STATION` lock owns it (no atomics, no lock of its own). The ladder is the AP's listed legacy rates (and the start rate)
/// by speed; per rate an EWMA of success per mille (`(7e + s) / 8`, s = 1000 iff acked within the long-retry tries,
/// `tx::Report::acked_at_rate`'s rule; 1000 before any frame) and an EWMA of retries x100.
#[derive(Default, Clone, Copy)]
pub struct RateCtl {
    pub rs: Option<w::RateSet>,
    pub short: bool,
    ladder: [u8; 12],
    n: usize,
    cur: usize,
    ewma: [u16; 12],
    tries: [u16; 12],
    run: u32,
    fails: u32,
    since: u32,
    probe_at: u32,
    probe: Option<bool>,
    last_ms: u64,
}

impl RateCtl {
    /// The AP's rate set `rs` (unread = the start rate alone, no steps), its short-preamble bit, the start rate (`pick_rate`'s).
    pub fn new(rs: Option<w::RateSet>, short: bool, start: u8, now_ms: u64) -> RateCtl {
        let mut bits = rs.map_or(0u16, |r| r.listed);
        if let Some(i) = w::LEGACY_RATES.iter().position(|r| *r == start) {
            bits |= 1 << i;
        }
        let mut c = RateCtl { rs, short, ewma: [1000; 12], last_ms: now_ms, ..RateCtl::default() };
        for (i, r) in w::LEGACY_RATES.iter().enumerate() {
            if bits & (1 << i) != 0 {
                c.ladder[c.n] = *r;
                c.n += 1;
            }
        }
        c.ladder[..c.n].sort_unstable();
        if c.n == 0 {
            c.ladder[0] = w::RATE_1M;
            c.n = 1;
        }
        c.cur = c.ladder[..c.n].iter().position(|r| *r == start).unwrap_or(0);
        c
    }

    /// The current rate (500 kbit/s units).
    pub fn rate(&self) -> u8 {
        if self.n == 0 { w::RATE_1M } else { self.ladder[self.cur] }
    }

    fn index(&self, rate: u8) -> Option<usize> {
        if rate == 0 { None } else { self.ladder[..self.n].iter().position(|r| *r == rate) }
    }

    /// The next unicast frame's rate: the current one, or every `PROBE_EVERY`th frame the next rate up (none at the top).
    pub fn next_rate(&mut self) -> u8 {
        if self.n == 0 {
            return w::RATE_1M;
        }
        self.probe_at += 1;
        if self.cur + 1 < self.n && self.probe_at >= PROBE_EVERY {
            self.probe_at = 0;
            return self.ladder[self.cur + 1];
        }
        self.ladder[self.cur]
    }

    /// One statused frame posted at `rate` (0 = a group frame, ignored; a rate off the ladder, ignored): acked within the
    /// long-retry tries (`at_rate`) or not, after `frame_count` transmissions.
    pub fn frame(&mut self, rate: u8, at_rate: bool, frame_count: u8) {
        let Some(i) = self.index(rate) else { return };
        let s: u32 = if at_rate { 1000 } else { 0 };
        self.ewma[i] = ((self.ewma[i] as u32 * 7 + s) / 8) as u16;
        self.tries[i] = ((self.tries[i] as u32 * 7 + frame_count.saturating_sub(1) as u32 * 100) / 8) as u16;
        if i == self.cur {
            self.since += 1;
            if at_rate {
                self.run += 1;
                self.fails = 0;
            } else {
                self.fails += 1;
                self.run = 0;
            }
        } else if i == self.cur + 1 {
            self.probe = Some(at_rate);
        }
    }

    /// A step, at most one per `HOLD_MS`, in order: down `fail` (`FAIL_K` consecutive failures), down `window` (`WINDOW`
    /// frames and EWMA < 500), up `probe-ok` (the last probe acked within the tries and `UP_N` consecutive successes). A
    /// failed probe is dropped (the frames after it stay at the current rate). `(old, new, why)`.
    pub fn decide(&mut self, now_ms: u64) -> Option<(u8, u8, &'static str)> {
        if self.n == 0 || now_ms.saturating_sub(self.last_ms) < HOLD_MS {
            return None;
        }
        let c = self.cur;
        let (to, why) = if self.fails >= FAIL_K && c > 0 {
            (c - 1, "fail")
        } else if self.since >= WINDOW && self.ewma[c] < 500 && c > 0 {
            (c - 1, "window")
        } else if self.probe == Some(true) && self.run >= UP_N && c + 1 < self.n {
            (c + 1, "probe-ok")
        } else {
            if self.probe == Some(false) {
                self.probe = None;
            }
            return None;
        };
        let old = self.ladder[c];
        self.cur = to;
        self.run = 0;
        self.fails = 0;
        self.since = 0;
        self.probe_at = 0;
        self.probe = None;
        self.last_ms = now_ms;
        Some((old, self.ladder[to], why))
    }

    /// `rate`'s EWMA inputs for the wire: (success percent, retries per frame x100); (0, 0) off the ladder.
    pub fn stats(&self, rate: u8) -> (u16, u16) {
        self.index(rate).map_or((0, 0), |i| (self.ewma[i] / 10, self.tries[i]))
    }
}

/// `kat`'s last two rows (STATIONRATECTL B881; WPA2's rows follow). `ratectl`: an 11g set (listed 0x0FFF, basic 1,2,5.5,11),
/// start CCK 11 — the 16 next rates are 15 x CCK 11 then OFDM 12 (the probe cadence); all acked: no step at 500 ms (the
/// hold), CCK 11 -> OFDM 12 `probe-ok` at 1000 ms; 4 unacked (frame_count 11) at OFDM 12: none at 1500, OFDM 12 -> CCK 11
/// `fail` at 2000, its EWMA 58% and 4.12 retries; 8 x (acked, unacked, unacked) at CCK 11 (never 4 in a row; EWMA 31.5%):
/// CCK 11 -> OFDM 9 `window` at 3000; an 11b set at its top never probes and holds; an unread set is CCK 1 alone and holds;
/// one unacked frame from a fresh EWMA reads 87% and 1.25; a group frame (rate 0) and an unknown rate are not counted.
/// `fallback`: `assoc::fallback_rate` (the highest basic CCK below) and the TX header's fallback fields (`tx::fb_kat`).
fn ratectl_rows(checks: &mut Vec<(&'static str, bool)>) {
    let g = w::RateSet { listed: 0x0FFF, basic: 0x000F };
    let mut c = RateCtl::new(Some(g), false, w::RATE_11M, 0);
    let mut seq = [0u8; 16];
    for s in seq.iter_mut() {
        *s = c.next_rate();
    }
    let cadence = seq[..15].iter().all(|r| *r == w::RATE_11M) && seq[15] == 24;
    for r in seq.iter() {
        c.frame(*r, true, 1);
    }
    let climb = c.decide(500).is_none() && c.decide(1000) == Some((w::RATE_11M, 24, "probe-ok")) && c.rate() == 24;
    for _ in 0..4 {
        c.frame(24, false, 11);
    }
    let down = c.decide(1500).is_none() && c.decide(2000) == Some((24, w::RATE_11M, "fail")) && c.stats(24) == (58, 412);
    for _ in 0..8 {
        c.frame(w::RATE_11M, true, 1);
        c.frame(w::RATE_11M, false, 11);
        c.frame(w::RATE_11M, false, 11);
    }
    let window = c.decide(3000) == Some((w::RATE_11M, 18, "window"));
    let mut t = RateCtl::new(Some(w::RateSet { listed: 0x000F, basic: 0x000F }), false, w::RATE_11M, 0);
    let mut top = true;
    for _ in 0..32 {
        top &= t.next_rate() == w::RATE_11M;
    }
    for _ in 0..16 {
        t.frame(w::RATE_11M, true, 1);
    }
    top &= t.decide(1000).is_none();
    let mut b = RateCtl::new(None, false, w::RATE_1M, 0);
    for _ in 0..4 {
        b.frame(w::RATE_1M, false, 11);
    }
    let bottom = b.decide(1000).is_none() && b.next_rate() == w::RATE_1M && b.rate() == w::RATE_1M;
    let mut d = RateCtl::new(Some(g), false, w::RATE_11M, 0);
    d.frame(w::RATE_11M, false, 11);
    d.frame(0, true, 1);
    d.frame(99, true, 1);
    let ewma = d.stats(w::RATE_11M) == (87, 125) && d.since == 1;
    checks.push(("ratectl", cadence && climb && down && window && top && bottom && ewma));
    let b12 = w::RateSet { listed: 0x000F, basic: 0x0003 };
    let fb = |rs: Option<w::RateSet>, r: u8| super::assoc::fallback_rate(rs, r);
    checks.push(("fallback", fb(Some(g), w::RATE_24M) == w::RATE_11M && fb(Some(g), w::RATE_11M) == 11 && fb(Some(g), 11) == 4
        && fb(Some(g), 4) == w::RATE_1M && fb(Some(g), w::RATE_1M) == w::RATE_1M && fb(Some(b12), w::RATE_11M) == 4
        && fb(None, w::RATE_24M) == w::RATE_1M && super::bringup::station_kat()[7]));
}

/// `kat`'s rows after B881's (STATIONERP rmbp-ledger B884). `erp`: a beacon (§9.3.3.2) naming UnaNet with capability 0x0021 (ESS +
/// Short Preamble) and an ERP element (§9.4.2.12, element 42) of 0x06 (Use_Protection | Barker_Preamble_Mode) -> `Bss { short: true,
/// erp: Some(6) }`, protection asked, Barker asked, `erp=prot,barker`, S9's capability 0x0021, and CCK 11 then leaves LONG
/// (`station_short(cap, barker, rate)`); the same beacon with capability 0x0001 and ERP 0x00 -> short false, `open,short`, S9
/// capability 0x0001; with no ERP element (or an empty one) -> `None`, `absent`, neither asked. `cts`: `tx::cts_kat`'s header words.
fn erp_rows(checks: &mut Vec<(&'static str, bool)>) {
    use super::assoc;
    let beacon = |cap: u16, erp: &[u8]| {
        let mut f = alloc::vec![0x80, 0x00, 0x00, 0x00];
        f.extend_from_slice(&[0xFF; 6]);
        f.extend_from_slice(&BSSID);
        f.extend_from_slice(&BSSID);
        f.extend_from_slice(&[0x00, 0x00]);
        f.extend_from_slice(&[0; 8]);
        f.extend_from_slice(&[0x64, 0x00]);
        f.extend_from_slice(&cap.to_le_bytes());
        f.extend_from_slice(&[0x00, 0x06]);
        f.extend_from_slice(b"UnaNet");
        f.extend_from_slice(&[0x03, 0x01, 0x0b]);
        f.extend_from_slice(erp);
        f
    };
    let pb = assoc::bss_match(&beacon(0x0021, &[w::EID_ERP, 1, 0x06]), b"UnaNet");
    let ob = assoc::bss_match(&beacon(0x0001, &[w::EID_ERP, 1, 0x00]), b"UnaNet");
    let nb = assoc::bss_match(&beacon(0x0021, &[]), b"UnaNet");
    let eb = assoc::bss_match(&beacon(0x0021, &[w::EID_ERP, 0]), b"UnaNet");
    let prot = matches!(pb, Some(b) if b.short && b.erp == Some(0x06) && !b.protected && assoc::assoc_cap(&b) == 0x0021
        && assoc::erp_protect(b.erp) && assoc::erp_barker(b.erp) && assoc::erp_text(b.erp) == "prot,barker"
        && !assoc::station_short(b.short, assoc::erp_barker(b.erp), w::RATE_11M));
    let open = matches!(ob, Some(b) if !b.short && b.erp == Some(0x00) && assoc::assoc_cap(&b) == 0x0001
        && !assoc::erp_protect(b.erp) && !assoc::erp_barker(b.erp) && assoc::erp_text(b.erp) == "open,short");
    let none = matches!(nb, Some(b) if b.erp.is_none() && assoc::erp_text(b.erp) == "absent" && !assoc::erp_protect(b.erp)
        && assoc::station_short(b.short, assoc::erp_barker(b.erp), w::RATE_11M))
        && matches!(eb, Some(b) if b.erp.is_none());
    checks.push(("erp", prot && open && none));
    checks.push(("cts", super::bringup::station_kat()[8]));
}
