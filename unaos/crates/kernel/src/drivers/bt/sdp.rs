//! CHARTER: Comscan — driver (Bluetooth; SDP client queries ride BTHID's SDP channel and its continuation loop, B339; this file is the A2DP sink query and its answer, and the SDP server on PSM 0x0001, SDPSERVER B769)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A2DP (rmbp-ledger B739) — SDP discovery of the A2DP sink service on a paired device.
//!
//! The SDP CLIENT exists in the BTHID host (`drivers/ehci/bthid.rs`: `sdp_request` sends a
//! ServiceSearchAttributeRequest on the SDP channel, `on_sdp` gathers the AttributeLists across continuation states,
//! at most 8 rounds, then hands the whole byte run to a parser). For an audio link the request's two data elements
//! come from [`a2dp_params`] and the answer goes to [`a2dp_sink`] instead of the HID descriptor walk.
//!
//! The query: ServiceSearchPattern = { UUID16 0x110B AudioSink }, AttributeIDList = { range 0x0004..0x0009 }
//! (ProtocolDescriptorList 0x0004 .. BluetoothProfileDescriptorList 0x0009). The answer read: the uint16 that
//! follows UUID 0x0100 (L2CAP) is the PSM, the one after UUID 0x0019 (AVDTP) its version, the one after UUID
//! 0x110D (AdvancedAudioDistribution) the A2DP version. A data element is walked by its header (type << 3 | size
//! index; index 5/6/7 = an 8/16/32-bit length follows); a sequence is entered, not skipped, so the walk is flat.
//!
//! THE SERVER (SDPSERVER, rmbp-ledger B769): a device that asks about US (a headset reconnecting on power-on looks for
//! the source before it opens AVDTP) is answered on the SDP channel it opened ([`Bt::sdp_serve`], [`serve`]):
//! ServiceSearch, ServiceAttribute and ServiceSearchAttribute over the records this host offers ([`records`]: A2DP
//! Source 0x110A, an AVRCP Target 0x110C stub, PnP Information 0x1200). The server holds NO state: the records are built
//! per request and the continuation state is two bytes, the offset into the (deterministic) full answer, so a long
//! answer is cut at min(MaximumAttributeByteCount, peer MTU) and resumed where the peer says. The same [`element`] walk
//! reads the request's search pattern and attribute ID list. Wire: `[sdp] server req=<search|attr|search-attr|error>
//! from=<addr> records=<n> bytes=<n>`.

use super::{fmt_addr, Bt};
use alloc::vec::Vec;

pub const UUID_L2CAP: u16 = 0x0100;
pub const UUID_AVDTP: u16 = 0x0019;
pub const UUID_AUDIO_SINK: u16 = 0x110B;
pub const UUID_A2DP: u16 = 0x110D;

/// The ServiceSearchAttributeRequest parameters before the continuation state: the search pattern, the maximum
/// attribute byte count (0x0280, as the HID query), the attribute ID range.
pub fn a2dp_params(p: &mut Vec<u8>) {
    p.extend_from_slice(&[0x35, 0x03, 0x19, (UUID_AUDIO_SINK >> 8) as u8, UUID_AUDIO_SINK as u8]); // DES { UUID16 AudioSink }
    p.extend_from_slice(&0x0280u16.to_be_bytes());
    p.extend_from_slice(&[0x35, 0x05, 0x0A, 0x00, 0x04, 0x00, 0x09]); // DES { uint32 range 0x0004..0x0009 }
}

/// What the sink record says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SinkRecord {
    /// The L2CAP PSM of AVDTP (0x0019 when the record names none).
    pub psm: u16,
    /// AVDTP version, e.g. 0x0103 = 1.3 (0 = not stated).
    pub avdtp: u16,
    /// A2DP version (0 = not stated).
    pub a2dp: u16,
}

/// One data element: (type, value offset, value length); a sequence's "value" is entered by the walk.
fn element(b: &[u8], i: usize) -> Option<(u8, usize, usize)> {
    let h = *b.get(i)?;
    let (ty, sz) = (h >> 3, h & 7);
    let (off, len) = match sz {
        0 if ty == 0 => (i + 1, 0),
        0 => (i + 1, 1),
        1 => (i + 1, 2),
        2 => (i + 1, 4),
        3 => (i + 1, 8),
        4 => (i + 1, 16),
        5 => (i + 2, *b.get(i + 1)? as usize),
        6 => (i + 3, u16::from_be_bytes([*b.get(i + 1)?, *b.get(i + 2)?]) as usize),
        _ => (i + 5, u32::from_be_bytes([*b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?, *b.get(i + 4)?]) as usize),
    };
    if off + len > b.len() {
        return None;
    }
    Some((ty, off, len))
}

/// The A2DP sink record in an SDP AttributeLists byte run, or None when the run names no AVDTP (no sink record).
pub fn a2dp_sink(b: &[u8]) -> Option<SinkRecord> {
    let mut r = SinkRecord { psm: 0x0019, avdtp: 0, a2dp: 0 };
    let mut seen_avdtp = false;
    let mut last_uuid: Option<u16> = None;
    let mut i = 0usize;
    while i < b.len() {
        let Some((ty, off, len)) = element(b, i) else { break };
        let mut uuid = None;
        match (ty, len) {
            (6 | 7, _) => {
                i = off; // enter the sequence
                continue;
            }
            (3, 2) => uuid = Some(u16::from_be_bytes([b[off], b[off + 1]])),
            (1, 2) => {
                let v = u16::from_be_bytes([b[off], b[off + 1]]);
                match last_uuid {
                    Some(UUID_L2CAP) => r.psm = v,
                    Some(UUID_AVDTP) => r.avdtp = v,
                    Some(UUID_A2DP) => r.a2dp = v,
                    _ => {}
                }
            }
            _ => {}
        }
        if uuid == Some(UUID_AVDTP) {
            seen_avdtp = true;
        }
        last_uuid = uuid;
        i = off + len;
    }
    seen_avdtp.then_some(r)
}

/// A sink record as a headset answers it (one record: L2CAP PSM 0x0019, AVDTP 1.3, A2DP 1.3), for `tests a2dp`.
pub fn fixture_record() -> Vec<u8> {
    alloc::vec![
        0x35, 0x22, // the record's attribute list
        0x09, 0x00, 0x04, 0x35, 0x10, 0x35, 0x06, 0x19, 0x01, 0x00, 0x09, 0x00, 0x19, 0x35, 0x06, 0x19, 0x00, 0x19, 0x09, 0x01, 0x03, // 0x0004
        0x09, 0x00, 0x09, 0x35, 0x08, 0x35, 0x06, 0x19, 0x11, 0x0D, 0x09, 0x01, 0x03, // 0x0009
    ]
}

// ── The SDP SERVER (SDPSERVER, rmbp-ledger B769) ───────────────────────────────────────────────────────────────────

pub const UUID_AUDIO_SOURCE: u16 = 0x110A;
pub const UUID_AVRCP_TARGET: u16 = 0x110C;
pub const UUID_PNP: u16 = 0x1200;
const UUID_AVCTP: u16 = 0x0017;
const UUID_AVRCP: u16 = 0x110E;
const UUID_BROWSE_ROOT: u16 = 0x1002;

// PDU identifiers.
const ERROR_RSP: u8 = 0x01;
const SEARCH_REQ: u8 = 0x02;
const SEARCH_RSP: u8 = 0x03;
const ATTR_REQ: u8 = 0x04;
const ATTR_RSP: u8 = 0x05;
const SEARCH_ATTR_REQ: u8 = 0x06;
const SEARCH_ATTR_RSP: u8 = 0x07;

// ErrorCodes.
const E_BAD_HANDLE: u16 = 0x0002;
const E_SYNTAX: u16 = 0x0003;
const E_PDU_SIZE: u16 = 0x0004;
const E_CONTINUATION: u16 = 0x0005;

/// The first record handle (0x00000000 is the SDP server's own record, not offered here).
const HANDLE0: u32 = 0x0001_0001;

fn uuid16(v: u16) -> [u8; 3] {
    [0x19, (v >> 8) as u8, v as u8]
}

fn uint16(v: u16) -> [u8; 3] {
    [0x09, (v >> 8) as u8, v as u8]
}

fn uint32(v: u32) -> [u8; 5] {
    let b = v.to_be_bytes();
    [0x0A, b[0], b[1], b[2], b[3]]
}

/// A data element sequence of `parts` (8-bit length header up to 255 bytes, else 16-bit).
fn seq(parts: &[&[u8]]) -> Vec<u8> {
    let n: usize = parts.iter().map(|p| p.len()).sum();
    let mut v = Vec::with_capacity(n + 3);
    if n <= 0xFF {
        v.extend_from_slice(&[0x35, n as u8]);
    } else {
        v.push(0x36);
        v.extend_from_slice(&(n as u16).to_be_bytes());
    }
    for p in parts {
        v.extend_from_slice(p);
    }
    v
}

/// One service record: its handle and its attributes (ID, value element), in ascending ID order.
pub struct Rec {
    pub handle: u32,
    attrs: Vec<(u16, Vec<u8>)>,
}

/// The records this host offers: A2DP Source (AVDTP on PSM 0x0019, AVDTP 1.3, A2DP 1.3, player), an AVRCP Target stub
/// (AVCTP's PSM 0x0017 is not served yet: the record lets a headset see the target role), PnP Information.
pub fn records() -> [Rec; 3] {
    let browse = seq(&[&uuid16(UUID_BROWSE_ROOT)]);
    let source = Rec {
        handle: HANDLE0,
        attrs: alloc::vec![
            (0x0000, uint32(HANDLE0).to_vec()),
            (0x0001, seq(&[&uuid16(UUID_AUDIO_SOURCE)])),
            (0x0004, seq(&[&seq(&[&uuid16(UUID_L2CAP), &uint16(0x0019)]), &seq(&[&uuid16(UUID_AVDTP), &uint16(0x0103)])])),
            (0x0005, browse.clone()),
            (0x0009, seq(&[&seq(&[&uuid16(UUID_A2DP), &uint16(0x0103)])])),
            (0x0311, uint16(0x0001).to_vec()), // SupportedFeatures: player
        ],
    };
    let target = Rec {
        handle: HANDLE0 + 1,
        attrs: alloc::vec![
            (0x0000, uint32(HANDLE0 + 1).to_vec()),
            (0x0001, seq(&[&uuid16(UUID_AVRCP_TARGET)])),
            (0x0004, seq(&[&seq(&[&uuid16(UUID_L2CAP), &uint16(UUID_AVCTP)]), &seq(&[&uuid16(UUID_AVCTP), &uint16(0x0104)])])),
            (0x0005, browse.clone()),
            (0x0009, seq(&[&seq(&[&uuid16(UUID_AVRCP), &uint16(0x0106)])])),
            (0x0311, uint16(0x0001).to_vec()), // SupportedFeatures: category 1
        ],
    };
    let pnp = Rec {
        handle: HANDLE0 + 2,
        attrs: alloc::vec![
            (0x0000, uint32(HANDLE0 + 2).to_vec()),
            (0x0001, seq(&[&uuid16(UUID_PNP)])),
            (0x0005, browse),
            (0x0009, seq(&[&seq(&[&uuid16(UUID_PNP), &uint16(0x0103)])])),
            (0x0200, uint16(0x0103).to_vec()), // SpecificationID
            (0x0201, uint16(0xFFFF).to_vec()), // VendorID: no assigned company
            (0x0202, uint16(0x0000).to_vec()), // ProductID
            (0x0203, uint16(0x0100).to_vec()), // Version
            (0x0204, alloc::vec![0x28, 0x01]), // PrimaryRecord: true
            (0x0205, uint16(0x0001).to_vec()), // VendorIDSource: Bluetooth SIG
        ],
    };
    [source, target, pnp]
}

/// The UUIDs a ServiceSearchPattern names (UUID16, UUID32, UUID128 on the Bluetooth base), as 32-bit values; None
/// when it is not a sequence of UUIDs. A UUID128 off the base can match nothing here and is kept as an impossible value.
fn uuids_of(b: &[u8]) -> Option<Vec<u32>> {
    const BASE: [u8; 12] = [0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB];
    let (ty, off, len) = element(b, 0)?;
    if ty != 6 {
        return None;
    }
    let mut v = Vec::new();
    let mut i = off;
    while i < off + len {
        let (t, o, l) = element(b, i)?;
        let d = &b[o..o + l];
        v.push(match (t, l) {
            (3, 2) => u16::from_be_bytes([d[0], d[1]]) as u32,
            (3, 4) => u32::from_be_bytes([d[0], d[1], d[2], d[3]]),
            (3, 16) if d[4..] == BASE => u32::from_be_bytes([d[0], d[1], d[2], d[3]]),
            (3, 16) => u32::MAX,
            _ => return None,
        });
        i = o + l;
    }
    (!v.is_empty() && v.len() <= 12).then_some(v)
}

/// The attribute ID ranges an AttributeIDList names (a uint16 ID is the range id..=id; a uint32 is hi..=lo).
fn ids_wanted(b: &[u8]) -> Option<Vec<(u16, u16)>> {
    let (ty, off, len) = element(b, 0)?;
    if ty != 6 {
        return None;
    }
    let mut v = Vec::new();
    let mut i = off;
    while i < off + len {
        let (t, o, l) = element(b, i)?;
        let d = &b[o..o + l];
        v.push(match (t, l) {
            (1, 2) => (u16::from_be_bytes([d[0], d[1]]), u16::from_be_bytes([d[0], d[1]])),
            (1, 4) => (u16::from_be_bytes([d[0], d[1]]), u16::from_be_bytes([d[2], d[3]])),
            _ => return None,
        });
        i = o + l;
    }
    (!v.is_empty()).then_some(v)
}

/// Every UUID16 a record's attribute values carry (sequences entered by [`element`]).
fn record_uuids(r: &Rec) -> Vec<u32> {
    let mut v = Vec::new();
    for (_, val) in &r.attrs {
        let mut i = 0;
        while let Some((t, o, l)) = element(val, i) {
            match (t, l) {
                (6 | 7, _) => {
                    i = o;
                    continue;
                }
                (3, 2) => v.push(u16::from_be_bytes([val[o], val[o + 1]]) as u32),
                _ => {}
            }
            i = o + l;
        }
    }
    v
}

/// A record matches a pattern when every UUID in it appears in the record.
fn matches(r: &Rec, uuids: &[u32]) -> bool {
    let have = record_uuids(r);
    uuids.iter().all(|u| have.contains(u))
}

/// A record's AttributeList for the wanted ID ranges: DES { (uint16 ID, value) ... } in ascending ID order.
fn attr_list(r: &Rec, ids: &[(u16, u16)]) -> Vec<u8> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for (id, val) in &r.attrs {
        if ids.iter().any(|&(lo, hi)| (lo..=hi).contains(id)) {
            let mut p = uint16(*id).to_vec();
            p.extend_from_slice(val);
            parts.push(p);
        }
    }
    let refs: Vec<&[u8]> = parts.iter().map(|p| &p[..]).collect();
    seq(&refs)
}

/// The ContinuationState at the end of a request's parameters (one length byte, then that many bytes): the bytes
/// before it and the state itself.
fn split_cont(p: &[u8], at: usize) -> Option<&[u8]> {
    let n = *p.get(at)? as usize;
    (n <= 16 && at + 1 + n == p.len()).then(|| &p[at + 1..])
}

/// One answer's parameters cut from the full byte run `run`: the 16-bit byte count, the chunk from the offset `cont`
/// names (two bytes, big-endian; empty = 0), then the ContinuationState for the rest (0 = done). The chunk is at most
/// `max` bytes and keeps the whole PDU inside the peer's MTU (5 header + 2 count + 3 continuation).
fn chunk(run: &[u8], cont: &[u8], max: u16, mtu: u16) -> Result<Vec<u8>, u16> {
    let off = match cont.len() {
        0 => 0,
        2 => u16::from_be_bytes([cont[0], cont[1]]) as usize,
        _ => return Err(E_CONTINUATION),
    };
    if off > 0 && off >= run.len() {
        return Err(E_CONTINUATION);
    }
    let n = (max as usize).min((mtu as usize).saturating_sub(10)).max(1);
    let end = (off + n).min(run.len());
    let mut v = Vec::with_capacity(5 + end - off);
    v.extend_from_slice(&((end - off) as u16).to_be_bytes());
    v.extend_from_slice(&run[off..end]);
    if end < run.len() {
        v.push(2);
        v.extend_from_slice(&(end as u16).to_be_bytes());
    } else {
        v.push(0);
    }
    Ok(v)
}

fn pdu(id: u8, tid: u16, params: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + params.len());
    v.push(id);
    v.extend_from_slice(&tid.to_be_bytes());
    v.extend_from_slice(&(params.len() as u16).to_be_bytes());
    v.extend_from_slice(params);
    v
}

/// What the server answered: the response PDU, the request's name for the wire, how many records it named, its bytes.
pub struct Served {
    pub rsp: Vec<u8>,
    pub op: &'static str,
    pub records: usize,
    pub bytes: usize,
}

/// The one request `req` (a whole SDP PDU) answered for a peer whose L2CAP MTU is `mtu`.
pub fn serve(req: &[u8], mtu: u16) -> Served {
    let tid = if req.len() >= 3 { u16::from_be_bytes([req[1], req[2]]) } else { 0 };
    let op = match req.first() {
        Some(&SEARCH_REQ) => "search",
        Some(&ATTR_REQ) => "attr",
        Some(&SEARCH_ATTR_REQ) => "search-attr",
        _ => "error",
    };
    let (rsp, records) = match answer(req, mtu) {
        Ok(x) => x,
        Err(code) => (pdu(ERROR_RSP, tid, &code.to_be_bytes()), 0),
    };
    let bytes = rsp.len();
    Served { rsp, op, records, bytes }
}

/// The answer to a well-formed request, or the ErrorCode the ErrorResponse carries.
fn answer(req: &[u8], mtu: u16) -> Result<(Vec<u8>, usize), u16> {
    if req.len() < 5 {
        return Err(E_PDU_SIZE);
    }
    let tid = u16::from_be_bytes([req[1], req[2]]);
    let p = &req[5..];
    if u16::from_be_bytes([req[3], req[4]]) as usize != p.len() {
        return Err(E_PDU_SIZE);
    }
    let recs = records();
    match req[0] {
        SEARCH_REQ => {
            let (_, off, len) = element(p, 0).ok_or(E_SYNTAX)?;
            let uuids = uuids_of(p).ok_or(E_SYNTAX)?;
            let at = off + len;
            let max = u16::from_be_bytes([*p.get(at).ok_or(E_SYNTAX)?, *p.get(at + 1).ok_or(E_SYNTAX)?]) as usize;
            let cont = split_cont(p, at + 2).ok_or(E_SYNTAX)?;
            if !cont.is_empty() {
                return Err(E_CONTINUATION); // three records always fit one answer
            }
            let hits: Vec<u32> = recs.iter().filter(|r| matches(r, &uuids)).map(|r| r.handle).take(max).collect();
            let mut v = Vec::with_capacity(5 + 4 * hits.len());
            v.extend_from_slice(&(hits.len() as u16).to_be_bytes());
            v.extend_from_slice(&(hits.len() as u16).to_be_bytes());
            for h in &hits {
                v.extend_from_slice(&h.to_be_bytes());
            }
            v.push(0);
            Ok((pdu(SEARCH_RSP, tid, &v), hits.len()))
        }
        ATTR_REQ => {
            if p.len() < 7 {
                return Err(E_SYNTAX);
            }
            let handle = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
            let max = u16::from_be_bytes([p[4], p[5]]);
            let (_, off, len) = element(p, 6).ok_or(E_SYNTAX)?;
            let ids = ids_wanted(&p[6..off + len]).ok_or(E_SYNTAX)?;
            let cont = split_cont(p, off + len).ok_or(E_SYNTAX)?;
            let r = recs.iter().find(|r| r.handle == handle).ok_or(E_BAD_HANDLE)?;
            Ok((pdu(ATTR_RSP, tid, &chunk(&attr_list(r, &ids), cont, max, mtu)?), 1))
        }
        SEARCH_ATTR_REQ => {
            let (_, off, len) = element(p, 0).ok_or(E_SYNTAX)?;
            let uuids = uuids_of(p).ok_or(E_SYNTAX)?;
            let at = off + len;
            let max = u16::from_be_bytes([*p.get(at).ok_or(E_SYNTAX)?, *p.get(at + 1).ok_or(E_SYNTAX)?]);
            let (_, ioff, ilen) = element(p, at + 2).ok_or(E_SYNTAX)?;
            let ids = ids_wanted(&p[at + 2..ioff + ilen]).ok_or(E_SYNTAX)?;
            let cont = split_cont(p, ioff + ilen).ok_or(E_SYNTAX)?;
            let lists: Vec<Vec<u8>> = recs.iter().filter(|r| matches(r, &uuids)).map(|r| attr_list(r, &ids)).collect();
            let refs: Vec<&[u8]> = lists.iter().map(|l| &l[..]).collect();
            Ok((pdu(SEARCH_ATTR_RSP, tid, &chunk(&seq(&refs), cont, max, mtu)?), lists.len()))
        }
        _ => Err(E_SYNTAX),
    }
}

/// The SDP CLIENT's channel among a link's channels (BTHID's `chan_psm` for PSM 0x0001): the one this host opened and is
/// not closing — a channel the peer opened is the server's, and must neither carry our query nor be closed by it.
pub(super) fn client_chan(chans: &[super::Chan]) -> Option<usize> {
    chans.iter().position(|c| c.psm == super::PSM_SDP && c.ours && c.st != super::ChanSt::WaitDiscRsp)
}

impl Bt {
    /// A request on an SDP channel the PEER opened: answered by [`serve`] within that channel's MTU, on its own channel.
    pub(super) fn sdp_serve(&mut self, li: usize, ci: usize, req: &[u8]) {
        let c = self.links[li].chans[ci];
        let s = serve(req, c.rmtu);
        serial_println!("[sdp] server req={} from={} records={} bytes={}", s.op, fmt_addr(&self.links[li].addr), s.records, s.bytes);
        let h = self.links[li].handle;
        self.l2_send(h, c.rcid, &s.rsp);
    }
}

/// The AttributeLists of a ServiceSearchAttribute answer gathered across continuation states the way a client does
/// (BTHID's `on_sdp`): ask, take the chunk, resend with the state, at most 32 rounds.
fn gather(pattern: &[u8], max: u16, ids: &[u8], mtu: u16) -> Option<Vec<u8>> {
    let mut acc = Vec::new();
    let mut cont: Vec<u8> = Vec::new();
    for round in 0..32u16 {
        let mut p = pattern.to_vec();
        p.extend_from_slice(&max.to_be_bytes());
        p.extend_from_slice(ids);
        p.push(cont.len() as u8);
        p.extend_from_slice(&cont);
        let s = serve(&pdu(SEARCH_ATTR_REQ, round + 1, &p), mtu);
        let b = &s.rsp;
        if b.first() != Some(&SEARCH_ATTR_RSP) || b.len() < 8 || b.len() > mtu as usize || u16::from_be_bytes([b[1], b[2]]) != round + 1 {
            return None;
        }
        let cnt = u16::from_be_bytes([b[5], b[6]]) as usize;
        acc.extend_from_slice(b.get(7..7 + cnt)?);
        let clen = *b.get(7 + cnt)? as usize;
        if clen == 0 {
            return Some(acc);
        }
        cont = b.get(8 + cnt..8 + cnt + clen)?.to_vec();
    }
    None
}

/// `tests a2dp` `sdp_server` leg — the server against a synthetic peer whose reader is the CLIENT's own parser: a
/// ServiceSearchAttribute for 0x110A returns the source record ([`a2dp_sink`] reads PSM 0x0019, AVDTP 1.3, A2DP 1.3
/// from it), the same answer over continuation (16 bytes a round, MTU 48) is byte-identical, ServiceSearch counts,
/// ServiceAttribute byte for byte, and the two errors (bad handle, bad continuation state).
pub fn server_legs() -> Result<(), &'static str> {
    let source = [0x35, 0x03, 0x19, 0x11, 0x0A];
    let all = [0x35, 0x05, 0x0A, 0x00, 0x00, 0xFF, 0xFF];
    let whole = gather(&source, 0x0280, &all, 672).ok_or("search-attr")?;
    if a2dp_sink(&whole) != Some(SinkRecord { psm: 0x0019, avdtp: 0x0103, a2dp: 0x0103 }) {
        return Err("source-record");
    }
    if gather(&source, 16, &all, 48).as_deref() != Some(&whole[..]) {
        return Err("continuation");
    }
    let mut p = alloc::vec![0x35, 0x03, 0x19, 0x01, 0x00, 0x00, 0x08, 0x00];
    let s = serve(&pdu(SEARCH_REQ, 7, &p), 672);
    if s.records != 2 || s.rsp != [SEARCH_RSP, 0x00, 0x07, 0x00, 0x0D, 0x00, 0x02, 0x00, 0x02, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x02, 0x00] {
        return Err("search-l2cap");
    }
    p[3..5].copy_from_slice(&[0x11, 0x24]);
    if serve(&pdu(SEARCH_REQ, 8, &p), 672).records != 0 {
        return Err("search-hid");
    }
    let mut a = alloc::vec![0x00, 0x01, 0x00, 0x01, 0x00, 0x40, 0x35, 0x03, 0x09, 0x00, 0x01, 0x00];
    let s = serve(&pdu(ATTR_REQ, 9, &a), 672);
    if s.rsp != [ATTR_RSP, 0x00, 0x09, 0x00, 0x0D, 0x00, 0x0A, 0x35, 0x08, 0x09, 0x00, 0x01, 0x35, 0x03, 0x19, 0x11, 0x0A, 0x00] {
        return Err("attr");
    }
    a[3] = 0x09;
    if serve(&pdu(ATTR_REQ, 10, &a), 672).rsp != [ERROR_RSP, 0x00, 0x0A, 0x00, 0x02, 0x00, 0x02] {
        return Err("bad-handle");
    }
    let mut bad = source.to_vec();
    bad.extend_from_slice(&0x0280u16.to_be_bytes());
    bad.extend_from_slice(&all);
    bad.extend_from_slice(&[0x02, 0xFF, 0xFF]);
    if serve(&pdu(SEARCH_ATTR_REQ, 11, &bad), 672).rsp != [ERROR_RSP, 0x00, 0x0B, 0x00, 0x02, 0x00, 0x05] {
        return Err("bad-continuation");
    }
    Ok(())
}
