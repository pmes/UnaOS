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
//! so `n=6/6` certifies the judge and the codec against the standard's field order, not a replay of a capture.

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

/// An Ethernet II frame as the 802.11 data frame to the AP (ToDS): `ethernet_to_data`, then the ring's cap (`tx.rs` `FRAME_MAX`).
/// `None` = shorter than an Ethernet header, or longer than one TX ring frame (`Link` counts it `tx_toolong`).
pub fn to_air(eth: &[u8], sta: Mac, bssid: Mac, seq: u16) -> Option<Vec<u8>> {
    let f = w::ethernet_to_data(eth, sta, bssid, seq)?;
    if f.len() > FRAME_MAX {
        return None;
    }
    Some(f)
}

/// A received frame judged as the AP's data frame to us, then `data_to_ethernet`: Data subtype 0 (§9.2.4.1.3),
/// FromDS = 1 and ToDS = 0 (§9.2.4.1.4), addr1 = `sta` and addr2 = `bssid` (§9.3.2.1, Table 9-26), and the
/// full RFC 1042 SNAP header (AA AA 03 00 00 00, §5.1.4). Anything else is `None` (the codec refuses Protected).
pub fn from_air(frame: &[u8], sta: Mac, bssid: Mac) -> Option<Vec<u8>> {
    if frame.len() < 24 + 8 || frame[0] != FC0_DATA {
        return None;
    }
    if frame[1] & (FC1_TODS | FC1_FROMDS) != FC1_FROMDS {
        return None;
    }
    if frame[4..10] != sta || frame[10..16] != bssid || frame[24..30] != w::SNAP_PREFIX {
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

/// Called by `rx::take` for every clean frame: a Data-type frame (§9.2.4.1.3) is kept, at most `KEEP`, for
/// `Link::recv` to judge; nothing else is.
pub fn keep(frame: &[u8], q: &mut Vec<Vec<u8>>) {
    if frame.len() >= 24 && (frame[0] >> 2) & 0x3 == w::FC_TYPE_DATA && q.len() < KEEP {
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
        let n = self.tx_next;
        self.tx_next = (self.tx_next + 1) % FRAMES;
        self.seq = (self.seq + 1) & 0x0FFF;
        Ok((f, n))
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
    let mut pass = 0u8;
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

/// The station outlived `wifi10()`'s pass (S10a LINKED; `bringup/join.rs::keep`).
pub fn kept() -> bool {
    super::bringup::station_mac().is_some()
}

/// The smoltcp stack rides the station (see `BOUND`).
pub fn bound() -> bool {
    BOUND.load(Ordering::Relaxed)
}

/// `e1000::hw_addr`'s fallback after usbnet's: (the SPROM MAC, 0.0.0.0 — the only honest address before a lease,
/// RFC 2131 §4.1 — link up), or `None` while no station is kept.
pub fn hw_addr() -> Option<([u8; 6], [u8; 4], bool)> {
    let m = super::bringup::station_mac()?;
    BOUND.store(true, Ordering::Relaxed);
    Some((m, [0; 4], true))
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
            keep(&f, &mut q);
            matches!(l.recv(&mut q, &mut back), Some(m) if m == n && back[..6] == STA && back[6..12] == STA && back[12..m] == disc[12..n])
        }
        _ => false,
    };
    let first = if n != 342 { Some("discover-len") } else if !kat_ok { Some("to-air") } else if !stub_ok { Some("stub") } else { None };
    (kat_ok, stub_ok, n, first)
}
