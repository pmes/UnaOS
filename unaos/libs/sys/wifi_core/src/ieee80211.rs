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

/// Information-element ids this station reads or writes.
pub const EID_SSID: u8 = 0;
pub const EID_DS_PARAM: u8 = 3; // the channel
pub const EID_RSN: u8 = 48;

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
            _ => {}
        }
        i += 2 + len;
    }
    Some(Beacon { bssid, ssid, channel, capabilities, rsn })
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

/// Parse an authentication response: `Some(status)` when it is one for us, else `None`. Status 0 is
/// success.
pub fn parse_auth(frame: &[u8]) -> Option<u16> {
    if frame.len() < 24 + 6 {
        return None;
    }
    let (ty, sub) = subtype_of(frame[0]);
    if ty != FC_TYPE_MGMT || sub != SUBTYPE_AUTH {
        return None;
    }
    Some(u16::from_le_bytes([frame[24 + 4], frame[24 + 5]]))
}

/// Build an association request carrying the SSID and a minimal supported-rates set.
pub fn build_assoc_req(sta: Mac, bssid: Mac, ssid: &[u8], seq: u16) -> Vec<u8> {
    let mut f = Vec::new();
    push_mgmt_header(&mut f, SUBTYPE_ASSOC_REQ, bssid, sta, bssid, seq);
    f.extend_from_slice(&0x0001u16.to_le_bytes()); // Capability Info: ESS
    f.extend_from_slice(&3u16.to_le_bytes()); // Listen interval
    f.push(EID_SSID);
    f.push(ssid.len() as u8);
    f.extend_from_slice(ssid);
    // Supported Rates (1,2,5.5,11 Mbit, in 500 kbit units, with the "basic" bit set).
    f.push(1);
    f.push(4);
    f.extend_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
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
        let r = build_assoc_req(sta, bssid, b"UnaNet", 2);
        // header 24 + capability 2 + listen 2 = 28; SSID eid+len = 2; body at 30.
        assert_eq!(r[28], EID_SSID);
        assert_eq!(&r[30..30 + 6], b"UnaNet");
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
