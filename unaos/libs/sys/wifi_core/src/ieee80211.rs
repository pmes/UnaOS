// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! IEEE 802.11 management and data frames — the station layer's codec (WIFI1 M3/M4).
//!
//! SOURCES: IEEE Std 802.11 frame formats (`[PUBLIC]`): the 24-byte MAC header, the beacon/probe-
//! response fixed fields + information elements, open-system authentication, the association
//! request/response, and the LLC/SNAP encapsulation of an Ethernet frame in an 802.11 data frame.
//! Nothing here is Broadcom-specific and no driver source was read. Parsers are TOTAL — a short or
//! malformed frame yields `None`, never a panic and never an out-of-bounds read (`#![forbid(unsafe)]`
//! holds crate-wide).

use alloc::vec::Vec;

/// Frame Control `type`/`subtype`, the values this station needs.
pub const FC_TYPE_MGMT: u8 = 0b00;
pub const FC_TYPE_DATA: u8 = 0b10;
pub const SUBTYPE_ASSOC_REQ: u8 = 0b0000;
pub const SUBTYPE_ASSOC_RESP: u8 = 0b0001;
pub const SUBTYPE_PROBE_REQ: u8 = 0b0100;
pub const SUBTYPE_PROBE_RESP: u8 = 0b0101;
pub const SUBTYPE_BEACON: u8 = 0b1000;
pub const SUBTYPE_AUTH: u8 = 0b1011;
pub const SUBTYPE_DATA: u8 = 0b0000;
/// WIFIAUTH (B788): the AP's two refusals (§9.3.3.4 Disassociation, §9.3.3.12 Deauthentication).
pub const SUBTYPE_DISASSOC: u8 = 0b1010;
pub const SUBTYPE_DEAUTH: u8 = 0b1100;

/// Information-element ids this station reads or writes.
pub const EID_SSID: u8 = 0;
pub const EID_DS_PARAM: u8 = 3; // the channel
pub const EID_RSN: u8 = 48;
/// Supported Rates (§9.4.2.3) and Extended Supported Rates (§9.4.2.13).
pub const EID_SUPP_RATES: u8 = 1;
pub const EID_EXT_RATES: u8 = 50;
/// STATIONERP (rmbp-ledger B884): the ERP element (§9.4.2.12) and its first octet's bits — NonERP_Present, Use_Protection,
/// Barker_Preamble_Mode.
pub const EID_ERP: u8 = 42;
pub const ERP_NON_ERP_PRESENT: u8 = 0x01;
pub const ERP_USE_PROTECTION: u8 = 0x02;
pub const ERP_BARKER_PREAMBLE: u8 = 0x04;
/// STATIONSHORTCAP (B884): the Capability Information bits this station writes (§9.4.1.4) — ESS, Short Preamble.
pub const CAP_ESS: u16 = 0x0001;
pub const CAP_SHORT_PREAMBLE: u16 = 0x0020;

/// A 48-bit MAC address.
pub type Mac = [u8; 6];

/// A parsed beacon or probe response — exactly what `wifi scan` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    pub bssid: Mac,
    /// SSID bytes (may be empty for a hidden network); never interpreted as UTF-8 here.
    pub ssid: Vec<u8>,
    /// The DS-parameter channel, if the IE was present.
    pub channel: Option<u8>,
    pub capabilities: u16,
    /// True when an RSN (WPA2) IE is present — the station must then refuse an OPEN join.
    pub rsn: bool,
    /// STATIONERP (B884): the ERP element's first octet (`ERP_*` bits), when the element was present with at least one octet.
    pub erp: Option<u8>,
}

impl Beacon {
    /// `true` when the capability "Privacy" bit or an RSN IE says the network is encrypted.
    pub fn protected(&self) -> bool {
        self.rsn || (self.capabilities & 0x0010) != 0
    }
}

fn subtype_of(fc0: u8) -> (u8, u8) {
    ((fc0 >> 2) & 0b11, (fc0 >> 4) & 0b1111)
}

/// Parse a received management frame as a beacon / probe response. `None` unless it is one and the
/// fixed fields and IEs fit. `addr3` of a beacon is the BSSID.
pub fn parse_beacon(frame: &[u8]) -> Option<Beacon> {
    if frame.len() < 24 + 12 {
        return None;
    }
    let (ty, sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_MGMT || (sub != SUBTYPE_BEACON && sub != SUBTYPE_PROBE_RESP) {
        return None;
    }
    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&frame[16..22]);
    // Fixed fields: 8-byte timestamp, 2-byte beacon interval, 2-byte capabilities.
    let capabilities = u16::from_le_bytes([frame[24 + 10], frame[24 + 11]]);
    let mut ssid = Vec::new();
    let mut channel = None;
    let mut rsn = false;
    let mut erp = None;
    let mut i = 24 + 12;
    while i + 2 <= frame.len() {
        let eid = frame[i];
        let len = frame[i + 1] as usize;
        if i + 2 + len > frame.len() {
            return None; // a declared IE that runs past the frame is malformed
        }
        let body = &frame[i + 2..i + 2 + len];
        match eid {
            EID_SSID => ssid = body.to_vec(),
            EID_DS_PARAM if len >= 1 => channel = Some(body[0]),
            EID_RSN => rsn = true,
            EID_ERP if len >= 1 => erp = Some(body[0]),
            _ => {}
        }
        i += 2 + len;
    }
    Some(Beacon { bssid, ssid, channel, capabilities, rsn, erp })
}

fn push_mgmt_header(out: &mut Vec<u8>, subtype: u8, da: Mac, sa: Mac, bssid: Mac, seq: u16) {
    out.push((subtype << 4) | (FC_TYPE_MGMT << 2)); // Frame Control byte 0
    out.push(0); // byte 1: no flags
    out.extend_from_slice(&[0, 0]); // Duration
    out.extend_from_slice(&da);
    out.extend_from_slice(&sa);
    out.extend_from_slice(&bssid);
    out.extend_from_slice(&(seq << 4).to_le_bytes()); // Sequence Control (frag 0)
}

/// Build an open-system authentication request (algorithm 0, sequence 1).
pub fn build_auth_open(sta: Mac, bssid: Mac, seq: u16) -> Vec<u8> {
    let mut f = Vec::new();
    push_mgmt_header(&mut f, SUBTYPE_AUTH, bssid, sta, bssid, seq);
    f.extend_from_slice(&0u16.to_le_bytes()); // Auth algorithm: Open System
    f.extend_from_slice(&1u16.to_le_bytes()); // Transaction sequence 1
    f.extend_from_slice(&0u16.to_le_bytes()); // Status 0
    f
}

/// The fixed fields of an authentication frame (§9.3.3.11): algorithm (§9.4.1.1, 0 = Open System),
/// transaction sequence (§9.4.1.2; an AP's answer to ours is 2) and status (§9.4.1.9, 0 = success).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthResult {
    pub alg: u16,
    pub seq: u16,
    pub status: u16,
}

/// Parse an authentication frame's fixed fields; `None` unless it is a management/authentication frame
/// long enough to carry them. Addressing is the caller's to check.
pub fn parse_auth(frame: &[u8]) -> Option<AuthResult> {
    if frame.len() < 24 + 6 {
        return None;
    }
    let (ty, sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_MGMT || sub != SUBTYPE_AUTH {
        return None;
    }
    Some(AuthResult {
        alg: u16::from_le_bytes([frame[24], frame[24 + 1]]),
        seq: u16::from_le_bytes([frame[24 + 2], frame[24 + 3]]),
        status: u16::from_le_bytes([frame[24 + 4], frame[24 + 5]]),
    })
}

/// Build an association request (§9.3.3.5) carrying the SSID, Supported Rates (eight: the four CCK
/// rates as basic + 6/9/12/18) and Extended Supported Rates (24/36/48/54) — an AP whose basic set holds
/// OFDM rates refuses an 11b-only request. STATIONSHORTCAP (rmbp-ledger B884): `cap` is the Capability Information word
/// (§9.4.1.4) — `CAP_ESS`, with `CAP_SHORT_PREAMBLE` when the caller's BSS allows it.
pub fn build_assoc_req(sta: Mac, bssid: Mac, ssid: &[u8], seq: u16, cap: u16) -> Vec<u8> {
    let mut f = Vec::new();
    push_mgmt_header(&mut f, SUBTYPE_ASSOC_REQ, bssid, sta, bssid, seq);
    f.extend_from_slice(&cap.to_le_bytes()); // Capability Info
    f.extend_from_slice(&3u16.to_le_bytes()); // Listen interval
    f.push(EID_SSID);
    f.push(ssid.len() as u8);
    f.extend_from_slice(ssid);
    // Supported Rates (§9.4.2.3, 500 kbit/s units): 1,2,5.5,11 Mbit with the "basic" bit, then 6,9,12,18.
    f.push(EID_SUPP_RATES);
    f.push(8);
    f.extend_from_slice(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    // Extended Supported Rates (§9.4.2.13, element 50): 24,36,48,54 Mbit.
    f.push(EID_EXT_RATES);
    f.push(4);
    f.extend_from_slice(&[0x30, 0x48, 0x60, 0x6c]);
    f
}

/// The outcome of an association response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssocResult {
    pub status: u16,
    /// Association id (low 14 bits), valid only when `status == 0`.
    pub aid: u16,
}

pub fn parse_assoc_resp(frame: &[u8]) -> Option<AssocResult> {
    if frame.len() < 24 + 6 {
        return None;
    }
    let (ty, sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_MGMT || sub != SUBTYPE_ASSOC_RESP {
        return None;
    }
    let status = u16::from_le_bytes([frame[24 + 2], frame[24 + 3]]);
    let aid = u16::from_le_bytes([frame[24 + 4], frame[24 + 5]]) & 0x3FFF;
    Some(AssocResult { status, aid })
}

/// Parse a Deauthentication (§9.3.3.12) or Disassociation (§9.3.3.4) frame: `(subtype, reason code)`
/// (§9.4.1.7). `None` for anything else or a frame too short to carry the reason.
pub fn parse_reason(frame: &[u8]) -> Option<(u8, u16)> {
    if frame.len() < 24 + 2 {
        return None;
    }
    let (ty, sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_MGMT || (sub != SUBTYPE_DEAUTH && sub != SUBTYPE_DISASSOC) {
        return None;
    }
    Some((sub, u16::from_le_bytes([frame[24], frame[25]])))
}

/// WIFILEAVEBUILD (zenbook-ledger Z75): Reason Code 3 (§9.4.1.7) — deauthenticated because the sending station is leaving.
pub const REASON_LEAVING: u16 = 3;

/// WIFILEAVEBUILD (Z75): the station's own Deauthentication (§9.3.3.12) — the 24-octet management header from
/// `push_mgmt_header` (DA = addr3 = the BSSID, SA = the station, sequence 0) and the Reason Code little-endian: 26 octets,
/// unprotected. `parse_reason` is its reader; the MT7925 leave (`tests wifileave`) sends it.
pub fn build_deauth(sta: Mac, bssid: Mac, reason: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(26);
    push_mgmt_header(&mut f, SUBTYPE_DEAUTH, bssid, sta, bssid, 0);
    f.extend_from_slice(&reason.to_le_bytes()); // Reason Code
    f
}

/// LLC/SNAP header for an Ethernet-typed payload (RFC 1042): AA-AA-03-00-00-00 then the EtherType.
pub const SNAP_PREFIX: [u8; 6] = [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00];

/// Encapsulate an Ethernet II frame as the body of an 802.11 data frame to the AP (ToDS).
/// Returns the full 802.11 frame. `[PUBLIC]` 802.11 data frame + RFC 1042.
pub fn ethernet_to_data(eth: &[u8], sta: Mac, bssid: Mac, seq: u16) -> Option<Vec<u8>> {
    if eth.len() < 14 {
        return None;
    }
    let mut da = [0u8; 6];
    da.copy_from_slice(&eth[0..6]);
    let ethertype = &eth[12..14];
    let payload = &eth[14..];
    let mut f = Vec::new();
    f.push((SUBTYPE_DATA << 4) | (FC_TYPE_DATA << 2)); // FC0: data, subtype 0
    f.push(0x01); // FC1: ToDS
    f.extend_from_slice(&[0, 0]); // Duration
    f.extend_from_slice(&bssid); // addr1 = RA = BSSID
    f.extend_from_slice(&sta); // addr2 = SA/TA
    f.extend_from_slice(&da); // addr3 = DA
    f.extend_from_slice(&(seq << 4).to_le_bytes()); // Sequence Control
    f.extend_from_slice(&SNAP_PREFIX);
    f.extend_from_slice(ethertype);
    f.extend_from_slice(payload);
    Some(f)
}

/// Decapsulate a received 802.11 data frame (FromDS) back into an Ethernet II frame. `None` unless it
/// is an unprotected data frame carrying an RFC-1042 SNAP header.
pub fn data_to_ethernet(frame: &[u8]) -> Option<Vec<u8>> {
    if frame.len() < 24 + 8 {
        return None;
    }
    let (ty, _sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_DATA {
        return None;
    }
    if frame[1] & 0x40 != 0 {
        return None; // Protected: this OPEN path does not decrypt
    }
    // FromDS (bit 1 of FC1): addr1=DA, addr2=BSSID, addr3=SA.
    let fromds = frame[1] & 0x02 != 0;
    let (da, sa) = if fromds {
        (&frame[4..10], &frame[16..22])
    } else {
        (&frame[4..10], &frame[10..16])
    };
    let snap = &frame[24..30];
    if snap[..3] != SNAP_PREFIX[..3] {
        return None;
    }
    let ethertype = &frame[30..32];
    let payload = &frame[32..];
    let mut eth = Vec::with_capacity(14 + payload.len());
    eth.extend_from_slice(da);
    eth.extend_from_slice(sa);
    eth.extend_from_slice(ethertype);
    eth.extend_from_slice(payload);
    Some(eth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn beacon_frame(ssid: &[u8], ch: u8, rsn: bool) -> Vec<u8> {
        let mut f = vec![(SUBTYPE_BEACON << 4) | (FC_TYPE_MGMT << 2), 0, 0, 0];
        f.extend_from_slice(&[0xff; 6]); // DA
        f.extend_from_slice(&[0x11; 6]); // SA
        f.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]); // BSSID
        f.extend_from_slice(&[0, 0]); // seq
        f.extend_from_slice(&[0; 8]); // timestamp
        f.extend_from_slice(&100u16.to_le_bytes()); // interval
        f.extend_from_slice(&0x0011u16.to_le_bytes()); // caps: ESS + Privacy
        f.push(EID_SSID); f.push(ssid.len() as u8); f.extend_from_slice(ssid);
        f.push(EID_DS_PARAM); f.push(1); f.push(ch);
        if rsn { f.push(EID_RSN); f.push(2); f.extend_from_slice(&[1, 0]); }
        f
    }

    #[test]
    fn beacon_parse() {
        let f = beacon_frame(b"UnaNet", 6, true);
        let b = parse_beacon(&f).unwrap();
        assert_eq!(b.ssid, b"UnaNet");
        assert_eq!(b.channel, Some(6));
        assert_eq!(b.bssid, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert!(b.rsn && b.protected());
        assert_eq!(b.erp, None);
        // STATIONERP (B884): an ERP element (Use_Protection | Barker_Preamble_Mode) is kept; an empty one is not.
        let mut e = beacon_frame(b"UnaNet", 6, false);
        e.extend_from_slice(&[EID_ERP, 1, ERP_USE_PROTECTION | ERP_BARKER_PREAMBLE]);
        assert_eq!(parse_beacon(&e).unwrap().erp, Some(0x06));
        let mut z = beacon_frame(b"UnaNet", 6, false);
        z.extend_from_slice(&[EID_ERP, 0]);
        assert_eq!(parse_beacon(&z).unwrap().erp, None);
        assert!(parse_beacon(&f[..20]).is_none());
        // a malformed IE length is refused, not read past
        let mut bad = f.clone();
        *bad.last_mut().unwrap() = 0; bad[37] = 200;
        // (length field far past the frame → None somewhere in the walk is acceptable; just no panic)
        let _ = parse_beacon(&bad);
    }

    #[test]
    fn auth_and_assoc_roundtrip_shapes() {
        let sta = [2, 0, 0, 0, 0, 1];
        let bssid = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let a = build_auth_open(sta, bssid, 1);
        let (ty, sub) = ((a[0] >> 2) & 3, (a[0] >> 4) & 0xf);
        assert_eq!((ty, sub), (FC_TYPE_MGMT, SUBTYPE_AUTH));
        assert_eq!(u16::from_le_bytes([a[24], a[25]]), 0); // Open System
        let r = build_assoc_req(sta, bssid, b"UnaNet", 2, CAP_ESS);
        assert_eq!(&r[24..26], &[0x01, 0x00]);
        let rs = build_assoc_req(sta, bssid, b"UnaNet", 2, CAP_ESS | CAP_SHORT_PREAMBLE);
        assert_eq!(&rs[24..26], &[0x21, 0x00]);
        assert_eq!(&rs[26..], &r[26..]);
        // header 24 + capability 2 + listen 2 = 28; SSID eid+len = 2; body at 30.
        assert_eq!(r[28], EID_SSID);
        assert_eq!(&r[30..30 + 6], b"UnaNet");
        assert_eq!(&r[36..38], &[EID_SUPP_RATES, 8]);
        assert_eq!(&r[46..52], &[EID_EXT_RATES, 4, 0x30, 0x48, 0x60, 0x6c]);
        let mut resp = a.clone();
        resp[26] = 2; // transaction 2
        resp[28] = 13; // status 13
        assert_eq!(parse_auth(&resp), Some(AuthResult { alg: 0, seq: 2, status: 13 }));
    }

    // WIFILEAVEBUILD (Z75): the deauthentication KAT — the expected octets assembled field by field from §9.3.3.12's layout.
    #[test]
    fn deauth_kat() {
        let sta = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
        let bssid = [0x00, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
        let mut want = vec![0xc0, 0x00]; // Frame Control: management (type 0), deauthentication (subtype 12), no flags
        want.extend_from_slice(&[0x00, 0x00]); // Duration
        want.extend_from_slice(&bssid); // addr1 = DA = the AP
        want.extend_from_slice(&sta); // addr2 = SA = the station
        want.extend_from_slice(&bssid); // addr3 = the BSSID
        want.extend_from_slice(&[0x00, 0x00]); // Sequence Control: fragment 0, sequence 0
        want.extend_from_slice(&[0x03, 0x00]); // Reason Code 3, little-endian
        let f = build_deauth(sta, bssid, REASON_LEAVING);
        assert_eq!(f.len(), 26);
        assert_eq!(f, want);
        assert_eq!(parse_reason(&f), Some((SUBTYPE_DEAUTH, REASON_LEAVING)));
        let g = build_deauth(sta, bssid, 0x0102);
        assert_eq!(&g[24..26], &[0x02, 0x01]);
        assert_eq!(&g[..24], &want[..24]);
    }

    #[test]
    fn data_frame_roundtrip() {
        let sta = [2, 0, 0, 0, 0, 1];
        let bssid = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let mut eth = vec![0x33, 0x33, 0, 0, 0, 1, 2, 0, 0, 0, 0, 1, 0x08, 0x00];
        eth.extend_from_slice(b"hello");
        let f = ethernet_to_data(&eth, sta, bssid, 7).unwrap();
        assert_eq!(f[1] & 0x01, 0x01); // ToDS
        // craft a FromDS version and decapsulate
        let mut rx = f.clone();
        rx[1] = 0x02; // FromDS
        let back = data_to_ethernet(&rx).unwrap();
        assert_eq!(&back[12..14], &[0x08, 0x00]);
        assert_eq!(&back[14..], b"hello");
    }
}

// ── STATIONRATE (rmbp-ledger B855): the AP's rate set from its association response, and the Null data frame the
// station's OFDM probe sends ─────────────────────────────────────────────────────────────────────────────────────

/// The twelve legacy rates in 500 kbit/s units (§9.4.2.3): CCK 1/2/5.5/11, OFDM 6/9/12/18/24/36/48/54. Bit `i` of a
/// `RateSet` word is `LEGACY_RATES[i]`.
pub const LEGACY_RATES: [u8; 12] = [2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108];
/// CCK 1 Mbit/s, CCK 11 Mbit/s and OFDM 24 Mbit/s in the same units.
pub const RATE_1M: u8 = 2;
pub const RATE_11M: u8 = 22;
pub const RATE_24M: u8 = 48;

/// The rates an association response listed (Supported Rates §9.4.2.3 + Extended Supported Rates §9.4.2.13) and the
/// subset marked basic (the octet's top bit), as `LEGACY_RATES` bit masks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RateSet {
    pub listed: u16,
    pub basic: u16,
}

impl RateSet {
    /// `rate` (500 kbit/s units) is one the AP listed.
    pub fn has(&self, rate: u8) -> bool {
        LEGACY_RATES.iter().position(|r| *r == rate).map_or(false, |i| self.listed & (1 << i) != 0)
    }
}

/// The rate elements of an association response (§9.3.3.6: the IEs follow capability, status and AID, 6 octets).
/// TOTAL: `None` unless it is an association response whose IE list fits exactly (a declared IE running past the
/// frame is malformed) and that carries at least one known rate; octets that name no legacy rate (a BSS membership
/// selector, §9.4.2.3) are ignored.
pub fn assoc_resp_rates(frame: &[u8]) -> Option<RateSet> {
    if frame.len() < 24 + 6 {
        return None;
    }
    let (ty, sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_MGMT || sub != SUBTYPE_ASSOC_RESP {
        return None;
    }
    let mut rs = RateSet::default();
    let mut i = 24 + 6;
    while i < frame.len() {
        if i + 2 > frame.len() {
            return None;
        }
        let eid = frame[i];
        let len = frame[i + 1] as usize;
        if i + 2 + len > frame.len() {
            return None;
        }
        if eid == EID_SUPP_RATES || eid == EID_EXT_RATES {
            for &o in &frame[i + 2..i + 2 + len] {
                if let Some(k) = LEGACY_RATES.iter().position(|r| *r == o & 0x7F) {
                    rs.listed |= 1 << k;
                    if o & 0x80 != 0 {
                        rs.basic |= 1 << k;
                    }
                }
            }
        }
        i += 2 + len;
    }
    if rs.listed == 0 { None } else { Some(rs) }
}

/// A Null data frame to the AP (§9.3.2.1, Data type subtype 4, no body): FC 48 01 (ToDS), duration 0, addr1 = addr3 =
/// the BSSID, addr2 = the station, the sequence number — 24 octets.
pub fn build_null_data(sta: Mac, bssid: Mac, seq: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(24);
    f.push((0b0100 << 4) | (FC_TYPE_DATA << 2));
    f.push(0x01);
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&sta);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&(seq << 4).to_le_bytes());
    f
}

/// STATIONROAM-BGSCAN (rmbp-ledger B955): the frame control's Power Management bit (IEEE 802.11-2020 §9.2.4.1.7), octet 1.
pub const FC_FLAG_PM: u8 = 0x10;

/// STATIONROAM-BGSCAN (B955): `build_null_data` with the Power Management bit set (`pm`: the station enters power save, so the AP
/// buffers its frames while it dwells off-channel) or clear (it leaves power save; byte-identical to `build_null_data`).
pub fn build_null_data_pm(sta: Mac, bssid: Mac, seq: u16, pm: bool) -> Vec<u8> {
    let mut f = build_null_data(sta, bssid, seq);
    if pm {
        f[1] |= FC_FLAG_PM;
    }
    f
}

/// STATIONROAM-SWEEP (rmbp-ledger B958): the Country element (IEEE 802.11-2020 §9.4.2.8, the 802.11d regulatory domain).
pub const EID_COUNTRY: u8 = 7;

/// STATIONROAM-SWEEP (B958): a beacon's Country element — the country string (2 letters + the environment octet) and the 2.4 GHz
/// channels 1..14 its subband triplets cover (bit c = channel c). STATIONROAM-5G (B962): `five`, bit i = `NONDFS_5G[i]` covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Country {
    pub cc: [u8; 3],
    pub chans: u16,
    pub five: u16,
}

/// STATIONROAM-SWEEP (B958): the Country element of a beacon / probe response. TOTAL: `None` unless the frame is one, its IE list fits
/// (a declared IE running past the frame is malformed) and it carries a Country element of at least the 3-octet string whose rest is
/// whole (first channel, number of channels, max power) triplets plus at most one pad octet. A triplet whose first octet is 201 or more
/// (an operating extension, §9.4.2.8) is skipped; channels past 14 set no bit.
pub fn country_ie(frame: &[u8]) -> Option<Country> {
    if frame.len() < 24 + 12 {
        return None;
    }
    let (ty, sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_MGMT || (sub != SUBTYPE_BEACON && sub != SUBTYPE_PROBE_RESP) {
        return None;
    }
    let mut found = None;
    let mut i = 24 + 12;
    while i < frame.len() {
        if i + 2 > frame.len() {
            return None;
        }
        let eid = frame[i];
        let len = frame[i + 1] as usize;
        if i + 2 + len > frame.len() {
            return None;
        }
        if eid == EID_COUNTRY && found.is_none() {
            let body = &frame[i + 2..i + 2 + len];
            if len < 3 || (len - 3) % 3 == 2 {
                return None;
            }
            let mut c = Country { cc: [body[0], body[1], body[2]], chans: 0, five: 0 };
            let mut k = 3;
            while k + 3 <= len {
                let (first, n) = (body[k] as u16, body[k + 1] as u16);
                if first < 201 {
                    for ch in first..first + n {
                        if (1..=14).contains(&ch) {
                            c.chans |= 1 << ch;
                        }
                    }
                    if first > 14 { for i in 0..n { if let Some(k5) = NONDFS_5G.iter().position(|x| *x as u16 == first + 4 * i) { c.five |= 1 << k5; } } } // STATIONROAM-5G (B962): a 5 GHz triplet counts channels 4 apart (§9.4.2.8)
                }
                k += 3;
            }
            found = Some(c);
        }
        i += 2 + len;
    }
    found
}

// ── STATIONROAM-5G (rmbp-ledger B962): the channel -> band map, the DFS rule, the SPROM power block choice — PURE ──────────────

/// STATIONROAM-5G (B962): the 5 GHz channels the station may transmit on without radar detection (UNII-1 and UNII-3), in plan order.
pub const NONDFS_5G: [u8; 9] = [36, 40, 44, 48, 149, 153, 157, 161, 165];

/// STATIONROAM-5G (B962): the 5 GHz channels b43's 2059 table carries a row for (`radio_2059.c:58-316`): 36..64, 100..140, 149..165.
fn row_5g(ch: u8) -> bool {
    ch % 4 == 0 && ((36..=64).contains(&ch) || (100..=140).contains(&ch)) || (149..=165).contains(&ch) && (ch - 149) % 4 == 0
}

/// STATIONROAM-5G (B962): a channel's centre frequency (MHz): 1..=13 -> 2407 + 5·ch, 14 -> 2484, a 5 GHz channel on b43's 2059 rows
/// -> 5000 + 5·ch (IEEE 802.11-2020 §E.1); anything else `None`.
pub fn chan_freq(ch: u8) -> Option<u16> {
    match ch {
        1..=13 => Some(2407 + 5 * ch as u16),
        14 => Some(2484),
        _ if row_5g(ch) => Some(5000 + 5 * ch as u16),
        _ => None,
    }
}

/// STATIONROAM-5G (B962): `ch` is a 5 GHz channel number (36 and up).
pub fn is_5g(ch: u8) -> bool {
    ch >= 36
}

/// STATIONROAM-5G (B962): `ch` is a DFS channel (52..=144, UNII-2 and UNII-2e): no radar detection is built, so nothing transmits there.
pub fn dfs(ch: u8) -> bool {
    (52..=144).contains(&ch)
}

/// STATIONROAM-5G (B962): the SPROM per-core power block b43 drives TX power control from for `freq` (`phy_ht.c:633-664`): below
/// 2500 `2g`; 4900..5100 `5gl`; 5100..5500 `5g`; 5500 and up `5gh`; else `None` (b43's fixed defaults, no channel row reaches them).
pub fn pwr_block(freq: u16) -> Option<&'static str> {
    match freq {
        0..=2499 => Some("2g"),
        4900..=5099 => Some("5gl"),
        5100..=5499 => Some("5g"),
        5500..=u16::MAX => Some("5gh"),
        _ => None,
    }
}

// ── MT7925S10 (zenbook-ledger Z55): the RSN element (IEEE Std 802.11-2020 §9.4.2.24) — the station's WPA2-PSK join ─────────
// BP-net-15: frames come only from this codec, so the RSN element the association request carries and the judge of the AP's
// element live here, shared by both radios (the Broadcom's WPA2JOIN is owed). Only WPA2-PSK with CCMP is offered.

/// The RSN element a WPA2-PSK station offers (22 octets): element 48, length 20, version 1, group cipher 00-0F-AC:4 (CCMP),
/// one pairwise cipher CCMP, one AKM 00-0F-AC:2 (PSK), RSN Capabilities 0 (no MFP). Appended after Extended Supported Rates in
/// the association request (§9.3.3.6: the RSNE follows the rate elements) and carried as M2's key data.
pub const RSN_PSK_CCMP: [u8; 22] = [
    EID_RSN, 20, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 2, 0, 0,
];

/// The suite selector of CCMP-128 and of the PSK AKM (§9.4.2.24.2, Table 9-149; §9.4.2.24.3, Table 9-151).
const SUITE_CCMP: [u8; 4] = [0x00, 0x0f, 0xac, 4];
const SUITE_PSK: [u8; 4] = [0x00, 0x0f, 0xac, 2];
/// RSN Capabilities bit 6 = MFPR (management frame protection required, §9.4.2.24.4).
const RSN_CAP_MFPR: u16 = 1 << 6;

/// The whole RSN element (id, length and body) of a beacon / probe response, `None` when it carries none or is not one. TOTAL:
/// an element that runs past the frame ends the walk with `None`.
pub fn rsn_ie(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 24 + 12 {
        return None;
    }
    let (ty, sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_MGMT || (sub != SUBTYPE_BEACON && sub != SUBTYPE_PROBE_RESP) {
        return None;
    }
    let mut i = 24 + 12;
    while i + 2 <= frame.len() {
        let len = frame[i + 1] as usize;
        if i + 2 + len > frame.len() {
            return None;
        }
        if frame[i] == EID_RSN {
            return Some(&frame[i..i + 2 + len]);
        }
        i += 2 + len;
    }
    None
}

/// True when the RSN element `elem` (id + length + body) lets a WPA2-PSK CCMP station join: version 1, group cipher CCMP, CCMP
/// among the pairwise ciphers, PSK among the AKMs, and MFPR clear (absent capabilities read 0). Anything shorter or malformed is
/// false (a TKIP-only, SAE-only or MFP-required network is refused by its caller by name).
pub fn rsn_psk_ccmp(elem: &[u8]) -> bool {
    if elem.len() < 2 || elem[0] != EID_RSN || elem[1] as usize + 2 != elem.len() {
        return false;
    }
    let b = &elem[2..];
    if b.len() < 8 || u16::from_le_bytes([b[0], b[1]]) != 1 || b[2..6] != SUITE_CCMP {
        return false;
    }
    let np = u16::from_le_bytes([b[6], b[7]]) as usize;
    let p_end = 8 + 4 * np;
    if b.len() < p_end + 2 {
        return false;
    }
    let pair_ok = b[8..p_end].chunks(4).any(|s| s == SUITE_CCMP);
    let na = u16::from_le_bytes([b[p_end], b[p_end + 1]]) as usize;
    let a_end = p_end + 2 + 4 * na;
    if b.len() < a_end {
        return false;
    }
    let akm_ok = b[p_end + 2..a_end].chunks(4).any(|s| s == SUITE_PSK);
    let caps = if b.len() >= a_end + 2 { u16::from_le_bytes([b[a_end], b[a_end + 1]]) } else { 0 };
    pair_ok && akm_ok && caps & RSN_CAP_MFPR == 0
}

#[cfg(test)]
mod rsn_tests {
    use super::*;

    fn beacon_with(ie: &[u8]) -> Vec<u8> {
        let mut f = alloc::vec![0u8; 24 + 12];
        f[0] = SUBTYPE_BEACON << 4;
        f.extend_from_slice(&[EID_SSID, 3, b'U', b'n', b'a']);
        f.extend_from_slice(ie);
        f
    }

    #[test]
    fn offer_is_accepted_and_found() {
        assert!(rsn_psk_ccmp(&RSN_PSK_CCMP));
        let f = beacon_with(&RSN_PSK_CCMP);
        assert_eq!(rsn_ie(&f), Some(&RSN_PSK_CCMP[..]));
        assert_eq!(rsn_ie(&beacon_with(&[])), None);
    }

    #[test]
    fn judge_refuses_what_is_not_wpa2_psk_ccmp() {
        // WPA2/WPA3 transition: AKMs PSK + SAE (00-0F-AC:8), MFPC set (bit 7), MFPR clear: accepted.
        let mixed = [EID_RSN, 24, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 2, 0, 0, 0x0f, 0xac, 2, 0, 0x0f, 0xac, 8, 0x80, 0];
        assert!(rsn_psk_ccmp(&mixed));
        let mut mfpr = mixed;
        mfpr[24] = 0xc0; // MFPR set
        assert!(!rsn_psk_ccmp(&mfpr));
        let mut tkip = RSN_PSK_CCMP;
        tkip[13] = 2; // pairwise TKIP only
        assert!(!rsn_psk_ccmp(&tkip));
        let mut sae = RSN_PSK_CCMP;
        sae[19] = 8; // SAE only
        assert!(!rsn_psk_ccmp(&sae));
        assert!(!rsn_psk_ccmp(&RSN_PSK_CCMP[..10]));
        assert!(!rsn_psk_ccmp(&[]));
    }
}
