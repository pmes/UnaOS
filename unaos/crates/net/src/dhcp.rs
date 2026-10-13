// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Minimal DHCP client (RFC 2131): DISCOVER -> OFFER -> REQUEST -> ACK. Builds full
//! broadcast frames (Ethernet/IPv4/UDP 68->67) and parses server replies. A tiny state
//! machine (`DhcpClient`) drives the exchange; the NIC driver transmits its output and
//! applies the resulting lease.

use crate::ethernet::{self, EthernetFrame, EtherType};
use crate::ipv4::{self, Ipv4Header};
use crate::udp;

pub const SERVER_PORT: u16 = 67;
pub const CLIENT_PORT: u16 = 68;

const OP_REQUEST: u8 = 1;
const OP_REPLY: u8 = 2;
const MAGIC: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

pub const MSG_DISCOVER: u8 = 1;
pub const MSG_OFFER: u8 = 2;
pub const MSG_REQUEST: u8 = 3;
pub const MSG_ACK: u8 = 5;

/// Build a BOOTP/DHCP message into `buf` (zeroed first). Returns the length written.
fn build_message(
    buf: &mut [u8],
    xid: u32,
    mac: [u8; 6],
    msg_type: u8,
    requested_ip: Option<[u8; 4]>,
    server_ip: Option<[u8; 4]>,
) -> Option<usize> {
    if buf.len() < 300 {
        return None;
    }
    for b in buf[..300].iter_mut() {
        *b = 0;
    }
    buf[0] = OP_REQUEST; // op = BOOTREQUEST
    buf[1] = 1; // htype = Ethernet
    buf[2] = 6; // hlen
    buf[4..8].copy_from_slice(&xid.to_be_bytes());
    buf[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast (we have no IP yet)
    buf[28..34].copy_from_slice(&mac); // chaddr (first 6 bytes)
    buf[236..240].copy_from_slice(&MAGIC); // options magic cookie

    let mut o = 240;
    buf[o] = 53; buf[o + 1] = 1; buf[o + 2] = msg_type; o += 3; // option 53: DHCP message type
    if let Some(ip) = requested_ip {
        buf[o] = 50; buf[o + 1] = 4; buf[o + 2..o + 6].copy_from_slice(&ip); o += 6; // 50: requested IP
    }
    if let Some(ip) = server_ip {
        buf[o] = 54; buf[o + 1] = 4; buf[o + 2..o + 6].copy_from_slice(&ip); o += 6; // 54: server identifier
    }
    buf[o] = 55; buf[o + 1] = 3; buf[o + 2] = 1; buf[o + 3] = 3; buf[o + 4] = 6; o += 5; // 55: param request list
    buf[o] = 255; o += 1; // end
    Some(o)
}

/// Wrap a DHCP message in a broadcast UDP/IPv4/Ethernet frame (src 0.0.0.0:68 ->
/// 255.255.255.255:67). Returns the total frame length.
fn wrap(frame: &mut [u8], mac: [u8; 6], dhcp: &[u8]) -> Option<usize> {
    let bcast_ip = [255u8, 255, 255, 255];
    let any_ip = [0u8; 4];
    // Ethernet[0..14] | IPv4[14..34] | UDP[34..42] | DHCP[42..].
    let udp_len = udp::write_datagram(frame.get_mut(34..)?, CLIENT_PORT, SERVER_PORT, any_ip, bcast_ip, dhcp)?;
    ipv4::write_header(frame.get_mut(14..34)?, any_ip, bcast_ip, udp::PROTO_UDP, udp_len)?;
    ethernet::write_header(frame.get_mut(0..14)?, [0xFF; 6], mac, EtherType::Ipv4.as_u16())?;
    Some(14 + 20 + udp_len)
}

/// Build a complete DHCP DISCOVER frame into `frame`.
pub fn build_discover(frame: &mut [u8], xid: u32, mac: [u8; 6]) -> Option<usize> {
    let mut dhcp = [0u8; 300];
    let n = build_message(&mut dhcp, xid, mac, MSG_DISCOVER, None, None)?;
    wrap(frame, mac, &dhcp[..n])
}

/// Build a complete DHCP REQUEST frame into `frame` (selecting `requested_ip` from `server_ip`).
pub fn build_request(
    frame: &mut [u8],
    xid: u32,
    mac: [u8; 6],
    requested_ip: [u8; 4],
    server_ip: [u8; 4],
) -> Option<usize> {
    let mut dhcp = [0u8; 300];
    let n = build_message(&mut dhcp, xid, mac, MSG_REQUEST, Some(requested_ip), Some(server_ip))?;
    wrap(frame, mac, &dhcp[..n])
}

/// Parsed fields of a DHCP server reply (OFFER / ACK).
#[derive(Clone, Copy)]
pub struct DhcpReply {
    pub xid: u32,
    pub msg_type: u8,
    pub your_ip: [u8; 4],
    pub server_ip: [u8; 4],
}

/// Parse a received frame as a DHCP reply to our client (UDP 67 -> 68). Returns `None` if it
/// isn't one, so the caller can fall through to the normal ingress path.
pub fn parse_reply(frame: &[u8]) -> Option<DhcpReply> {
    let eth = EthernetFrame::new(frame)?;
    if eth.ethertype() != EtherType::Ipv4 {
        return None;
    }
    let ip = Ipv4Header::new(eth.payload())?;
    if ip.protocol() != udp::PROTO_UDP {
        return None;
    }
    let dg = udp::UdpDatagram::new(ip.payload())?;
    if dg.dest_port() != CLIENT_PORT || dg.source_port() != SERVER_PORT {
        return None;
    }
    let d = dg.payload();
    if d.len() < 240
        || d[0] != OP_REPLY
        || d[236] != MAGIC[0]
        || d[237] != MAGIC[1]
        || d[238] != MAGIC[2]
        || d[239] != MAGIC[3]
    {
        return None;
    }

    let xid = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    let mut your_ip = [0u8; 4];
    your_ip.copy_from_slice(&d[16..20]);

    // Walk options for the message type (53) and server identifier (54).
    let mut msg_type = 0u8;
    let mut server_ip = [0u8; 4];
    let mut i = 240;
    while i < d.len() {
        let code = d[i];
        if code == 255 {
            break; // end
        }
        if code == 0 {
            i += 1; // pad
            continue;
        }
        if i + 1 >= d.len() {
            break;
        }
        let len = d[i + 1] as usize;
        if i + 2 + len > d.len() {
            break;
        }
        let val = &d[i + 2..i + 2 + len];
        match code {
            53 if len >= 1 => msg_type = val[0],
            54 if len >= 4 => server_ip.copy_from_slice(&val[0..4]),
            _ => {}
        }
        i += 2 + len;
    }

    Some(DhcpReply { xid, msg_type, your_ip, server_ip })
}

/// DHCP client lease state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DhcpState {
    Idle,
    Discovering,
    Requesting,
    Bound,
}

/// A minimal DHCP client state machine. The driver calls [`discover`](Self::discover) once at
/// bring-up, feeds each DHCP reply to [`on_reply`](Self::on_reply), and applies `leased_ip`
/// when the state reaches `Bound`.
pub struct DhcpClient {
    pub state: DhcpState,
    xid: u32,
    mac: [u8; 6],
    pub leased_ip: Option<[u8; 4]>,
}

impl DhcpClient {
    pub fn new(mac: [u8; 6], xid: u32) -> Self {
        Self { state: DhcpState::Idle, xid, mac, leased_ip: None }
    }

    /// Begin discovery: write a DISCOVER frame into `out`, return its length.
    pub fn discover(&mut self, out: &mut [u8]) -> Option<usize> {
        self.state = DhcpState::Discovering;
        build_discover(out, self.xid, self.mac)
    }

    /// Handle a DHCP reply. On OFFER (while discovering) writes a REQUEST into `out` and
    /// returns its length; on ACK (while requesting) records the lease (-> Bound) and returns
    /// None. Replies for other transactions/states are ignored.
    pub fn on_reply(&mut self, reply: &DhcpReply, out: &mut [u8]) -> Option<usize> {
        if reply.xid != self.xid {
            return None;
        }
        match (self.state, reply.msg_type) {
            (DhcpState::Discovering, MSG_OFFER) => {
                self.state = DhcpState::Requesting;
                build_request(out, self.xid, self.mac, reply.your_ip, reply.server_ip)
            }
            (DhcpState::Requesting, MSG_ACK) => {
                self.leased_ip = Some(reply.your_ip);
                self.state = DhcpState::Bound;
                None
            }
            _ => None,
        }
    }
}

// ── USBNETDHCP (rmbp-ledger B538) — APPENDED: the USB link's client. Nothing above this line changed,
// so the e1000 path (`on_reply`, the unpadded builders) and every image that does not call these compiles
// exactly as before.

/// RFC 1542 §2.1: the minimum BOOTP message a client sends (some servers drop shorter ones; flight 31's
/// smoltcp DISCOVER carried 262 octets).
pub const BOOTP_MIN: usize = 300;
/// DHCPNAK (RFC 2131 §3.1.4).
pub const MSG_NAK: u8 = 6;

/// A DHCP message in its full zero-padded `BOOTP_MIN` buffer (broadcast flag set by `build_message`),
/// wrapped in the broadcast frame: 14 + 20 + 8 + 300 = 342 bytes.
fn build_min(
    frame: &mut [u8],
    xid: u32,
    mac: [u8; 6],
    msg_type: u8,
    requested_ip: Option<[u8; 4]>,
    server_ip: Option<[u8; 4]>,
) -> Option<usize> {
    let mut dhcp = [0u8; BOOTP_MIN];
    build_message(&mut dhcp, xid, mac, msg_type, requested_ip, server_ip)?;
    wrap(frame, mac, &dhcp)
}

/// A server reply with the lease's options: 1 subnet mask, 3 first router, 6 first DNS server,
/// 51 lease seconds (zeros when absent).
#[derive(Clone, Copy)]
pub struct DhcpLease {
    pub reply: DhcpReply,
    pub mask: [u8; 4],
    pub router: [u8; 4],
    pub dns: [u8; 4],
    pub lease_s: u32,
}

/// `parse_reply` plus the lease options. `None` when the frame is not a DHCP reply (UDP 67 -> 68).
pub fn parse_lease(frame: &[u8]) -> Option<DhcpLease> {
    let reply = parse_reply(frame)?;
    let eth = EthernetFrame::new(frame)?;
    let ip = Ipv4Header::new(eth.payload())?;
    let dg = udp::UdpDatagram::new(ip.payload())?;
    let d = dg.payload();
    let mut l = DhcpLease { reply, mask: [0; 4], router: [0; 4], dns: [0; 4], lease_s: 0 };
    let mut i = 240;
    while i < d.len() {
        let code = d[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        if i + 1 >= d.len() {
            break;
        }
        let len = d[i + 1] as usize;
        if i + 2 + len > d.len() {
            break;
        }
        let v = &d[i + 2..i + 2 + len];
        match code {
            1 if len >= 4 => l.mask.copy_from_slice(&v[..4]),
            3 if len >= 4 => l.router.copy_from_slice(&v[..4]),
            6 if len >= 4 => l.dns.copy_from_slice(&v[..4]),
            51 if len >= 4 => l.lease_s = u32::from_be_bytes([v[0], v[1], v[2], v[3]]),
            _ => {}
        }
        i += 2 + len;
    }
    Some(l)
}

/// What [`DhcpClient::handle`] did with a reply.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Handled {
    /// Another transaction, or a message this state does not take.
    Ignored,
    /// An OFFER was taken: a REQUEST of this many bytes is in `out`.
    Request(usize),
    /// The ACK landed: `leased_ip` is set, state `Bound`.
    Bound,
    /// The server refused (NAK): back to `Idle`, discover again.
    Nak,
}

impl DhcpClient {
    /// The current transaction id.
    pub fn xid(&self) -> u32 {
        self.xid
    }

    /// (Re)start discovery under a fresh `xid` (RFC 2131 §4.1: a new transaction per retransmitted
    /// DISCOVER is allowed; it keeps a late OFFER for an old one from being mistaken for the new one):
    /// a padded broadcast DISCOVER into `out`, its length returned.
    pub fn discover_min(&mut self, xid: u32, out: &mut [u8]) -> Option<usize> {
        self.xid = xid;
        self.state = DhcpState::Discovering;
        self.leased_ip = None;
        build_min(out, xid, self.mac, MSG_DISCOVER, None, None)
    }

    /// `on_reply`'s state machine over a parsed lease, with padded output and NAK handling: OFFER while
    /// Discovering -> a REQUEST for its address from its server; ACK while Requesting -> Bound;
    /// NAK while Requesting -> Idle.
    pub fn handle(&mut self, lease: &DhcpLease, out: &mut [u8]) -> Handled {
        let r = &lease.reply;
        if r.xid != self.xid {
            return Handled::Ignored;
        }
        match (self.state, r.msg_type) {
            (DhcpState::Discovering, MSG_OFFER) => {
                match build_min(out, self.xid, self.mac, MSG_REQUEST, Some(r.your_ip), Some(r.server_ip)) {
                    Some(n) => {
                        self.state = DhcpState::Requesting;
                        Handled::Request(n)
                    }
                    None => Handled::Ignored,
                }
            }
            (DhcpState::Requesting, MSG_ACK) => {
                self.leased_ip = Some(r.your_ip);
                self.state = DhcpState::Bound;
                Handled::Bound
            }
            (DhcpState::Requesting, MSG_NAK) => {
                self.state = DhcpState::Idle;
                self.leased_ip = None;
                Handled::Nak
            }
            _ => Handled::Ignored,
        }
    }
}

// ── DHCPREPLUG (rmbp-ledger B632) — APPENDED: the RFC 2131 §4.4.1 / RFC 2132 DISCOVER and REQUEST the USB link's
// client sends, the decode that audits a frame we sent, and the decode of ANY UDP 67/68 frame we hear. Nothing above
// this line changed, so the e1000 path (`on_reply`, the unpadded builders) and the B538 builders compile as before.

/// Option 12 (RFC 2132 §3.14).
pub const HOST_NAME: &[u8] = b"unaos";
/// Option 55 (RFC 2132 §9.8): subnet mask, router, DNS, domain name, broadcast address, lease time, T1, T2.
pub const PARAM_LIST: [u8; 8] = [1, 3, 6, 15, 28, 51, 58, 59];

/// One option TLV at `*o`.
fn put_opt(b: &mut [u8], o: &mut usize, code: u8, val: &[u8]) {
    b[*o] = code;
    b[*o + 1] = val.len() as u8;
    b[*o + 2..*o + 2 + val.len()].copy_from_slice(val);
    *o += 2 + val.len();
}

/// A BOOTREQUEST of the full RFC shape in a zeroed `BOOTP_MIN` buffer: op 1, htype 1, hlen 6, hops 0, `xid`, `secs`
/// (seconds since this acquisition began, §4.4.1), flags BROADCAST (0x8000, §4.1: we cannot take a unicast before
/// an address is bound), ciaddr 0, chaddr = `mac`, the magic cookie, then 53 type, 61 client-id (type 1 + chaddr,
/// §9.14 — the SAME in DISCOVER and REQUEST, §4.2), 50 requested address and 54 server identifier (REQUEST in
/// SELECTING, §4.3.2), 12 host-name, 55 parameter list, 255 end; the tail stays zero pad to 300 octets (RFC 1542).
fn build_rfc(
    b: &mut [u8; BOOTP_MIN],
    xid: u32,
    secs: u16,
    mac: [u8; 6],
    msg_type: u8,
    requested_ip: Option<[u8; 4]>,
    server_ip: Option<[u8; 4]>,
) {
    *b = [0; BOOTP_MIN];
    b[0] = OP_REQUEST;
    b[1] = 1;
    b[2] = 6;
    b[4..8].copy_from_slice(&xid.to_be_bytes());
    b[8..10].copy_from_slice(&secs.to_be_bytes());
    b[10..12].copy_from_slice(&0x8000u16.to_be_bytes());
    b[28..34].copy_from_slice(&mac);
    b[236..240].copy_from_slice(&MAGIC);
    let mut o = 240;
    put_opt(b, &mut o, 53, &[msg_type]);
    put_opt(b, &mut o, 61, &[1, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]]);
    if let Some(ip) = requested_ip {
        put_opt(b, &mut o, 50, &ip);
    }
    if let Some(ip) = server_ip {
        put_opt(b, &mut o, 54, &ip);
    }
    put_opt(b, &mut o, 12, HOST_NAME);
    put_opt(b, &mut o, 55, &PARAM_LIST);
    b[o] = 255;
}

/// The full-shape DISCOVER in its broadcast frame (14 + 20 + 8 + 300 = 342 bytes).
pub fn build_discover_rfc(frame: &mut [u8], xid: u32, secs: u16, mac: [u8; 6]) -> Option<usize> {
    let mut d = [0u8; BOOTP_MIN];
    build_rfc(&mut d, xid, secs, mac, MSG_DISCOVER, None, None);
    wrap(frame, mac, &d)
}

impl DhcpClient {
    /// `discover_min` with the full option set and `secs`.
    pub fn discover_rfc(&mut self, xid: u32, secs: u16, out: &mut [u8]) -> Option<usize> {
        self.xid = xid;
        self.state = DhcpState::Discovering;
        self.leased_ip = None;
        build_discover_rfc(out, xid, secs, self.mac)
    }

    /// `handle` with the full-shape REQUEST (61, 12 and 55 carried as in the DISCOVER).
    pub fn handle_rfc(&mut self, lease: &DhcpLease, secs: u16, out: &mut [u8]) -> Handled {
        let r = &lease.reply;
        if r.xid != self.xid {
            return Handled::Ignored;
        }
        match (self.state, r.msg_type) {
            (DhcpState::Discovering, MSG_OFFER) => {
                let mut d = [0u8; BOOTP_MIN];
                build_rfc(&mut d, self.xid, secs, self.mac, MSG_REQUEST, Some(r.your_ip), Some(r.server_ip));
                match wrap(out, self.mac, &d) {
                    Some(n) => {
                        self.state = DhcpState::Requesting;
                        Handled::Request(n)
                    }
                    None => Handled::Ignored,
                }
            }
            (DhcpState::Requesting, MSG_ACK) => {
                self.leased_ip = Some(r.your_ip);
                self.state = DhcpState::Bound;
                Handled::Bound
            }
            (DhcpState::Requesting, MSG_NAK) => {
                self.state = DhcpState::Idle;
                self.leased_ip = None;
                Handled::Nak
            }
            _ => Handled::Ignored,
        }
    }
}

/// The 16-bit one's-complement sum of `d` (odd tail byte high), unfolded carries folded.
fn sum16(mut s: u32, d: &[u8]) -> u32 {
    let mut i = 0;
    while i + 1 < d.len() {
        s += u16::from_be_bytes([d[i], d[i + 1]]) as u32;
        i += 2;
    }
    if i < d.len() {
        s += (d[i] as u32) << 8;
    }
    while (s >> 16) != 0 {
        s = (s & 0xFFFF) + (s >> 16);
    }
    s
}

/// The UDP checksum verdict over pseudo-header + segment: 0 = none sent (legal, RFC 768), 1 = verifies, 2 = wrong.
fn udp_sum_verdict(src: &[u8], dst: &[u8], seg: &[u8]) -> u8 {
    if seg.len() < 8 || (seg[6] == 0 && seg[7] == 0) {
        return 0;
    }
    let mut ph = [0u8; 12];
    ph[0..4].copy_from_slice(src);
    ph[4..8].copy_from_slice(dst);
    ph[9] = udp::PROTO_UDP;
    ph[10..12].copy_from_slice(&(seg.len() as u16).to_be_bytes());
    if sum16(sum16(0, &ph), seg) == 0xFFFF { 1 } else { 2 }
}

/// Every field of a DHCP client frame as it sits in the buffer the NIC was handed (RFC 2131 §4.4.1, RFC 2132).
#[derive(Clone, Copy)]
pub struct TxAudit {
    pub eth_dst: [u8; 6],
    pub eth_src: [u8; 6],
    pub ethertype: u16,
    pub ip_ver_ihl: u8,
    pub ip_tos: u8,
    pub ip_len: u16,
    pub ip_id: u16,
    pub ip_frag: u16,
    pub ip_ttl: u8,
    pub ip_proto: u8,
    pub ip_csum: u16,
    pub ip_csum_ok: bool,
    pub ip_src: [u8; 4],
    pub ip_dst: [u8; 4],
    pub udp_sport: u16,
    pub udp_dport: u16,
    pub udp_len: u16,
    pub udp_csum: u16,
    /// 0 none sent, 1 verifies, 2 wrong.
    pub udp_csum_verdict: u8,
    pub op: u8,
    pub htype: u8,
    pub hlen: u8,
    pub hops: u8,
    pub xid: u32,
    pub secs: u16,
    pub flags: u16,
    pub ciaddr: [u8; 4],
    pub chaddr: [u8; 6],
    pub cookie_ok: bool,
    /// Option codes in wire order (the first 16), and how many there were.
    pub opts: [u8; 16],
    pub n_opts: usize,
    pub msg_type: u8,
    /// 61 present as type 1 + chaddr.
    pub client_id_ok: bool,
    pub host: [u8; 16],
    pub host_len: usize,
    pub params: [u8; 16],
    pub n_params: usize,
    pub end_ok: bool,
    pub bootp_len: usize,
    /// The first check that failed, or "none".
    pub fail: &'static str,
}

/// Decode a DHCP client frame byte by byte and check it against RFC 2131 §4.4.1 / RFC 2132. `None` when the frame is
/// too short to hold Ethernet + IPv4 + UDP + the 240-octet fixed BOOTP part.
pub fn audit_request(f: &[u8]) -> Option<TxAudit> {
    if f.len() < 14 + 20 {
        return None;
    }
    let ihl = ((f[14] & 0x0f) as usize) * 4;
    let l4 = 14 + ihl;
    if ihl < 20 || f.len() < l4 + 8 + 240 {
        return None;
    }
    let be16 = |i: usize| u16::from_be_bytes([f[i], f[i + 1]]);
    let mut a = TxAudit {
        eth_dst: [0; 6], eth_src: [0; 6], ethertype: be16(12),
        ip_ver_ihl: f[14], ip_tos: f[15], ip_len: be16(16), ip_id: be16(18), ip_frag: be16(20), ip_ttl: f[22],
        ip_proto: f[23], ip_csum: be16(24), ip_csum_ok: sum16(0, &f[14..l4]) == 0xFFFF,
        ip_src: [f[26], f[27], f[28], f[29]], ip_dst: [f[30], f[31], f[32], f[33]],
        udp_sport: be16(l4), udp_dport: be16(l4 + 2), udp_len: be16(l4 + 4), udp_csum: be16(l4 + 6), udp_csum_verdict: 0,
        op: 0, htype: 0, hlen: 0, hops: 0, xid: 0, secs: 0, flags: 0, ciaddr: [0; 4], chaddr: [0; 6], cookie_ok: false,
        opts: [0; 16], n_opts: 0, msg_type: 0, client_id_ok: false, host: [0; 16], host_len: 0, params: [0; 16],
        n_params: 0, end_ok: false, bootp_len: 0, fail: "none",
    };
    a.eth_dst.copy_from_slice(&f[0..6]);
    a.eth_src.copy_from_slice(&f[6..12]);
    let seg_end = (l4 + (a.udp_len as usize).max(8 + 240)).min(f.len()); // a lying length still leaves 240 octets
    a.udp_csum_verdict = udp_sum_verdict(&f[26..30], &f[30..34], &f[l4..seg_end]);
    let d = &f[l4 + 8..seg_end];
    a.bootp_len = d.len();
    a.op = d[0];
    a.htype = d[1];
    a.hlen = d[2];
    a.hops = d[3];
    a.xid = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    a.secs = u16::from_be_bytes([d[8], d[9]]);
    a.flags = u16::from_be_bytes([d[10], d[11]]);
    a.ciaddr.copy_from_slice(&d[12..16]);
    a.chaddr.copy_from_slice(&d[28..34]);
    a.cookie_ok = d[236..240] == MAGIC;
    let mut i = 240;
    while i < d.len() {
        let code = d[i];
        if code == 255 {
            a.end_ok = true;
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        if i + 1 >= d.len() || i + 2 + d[i + 1] as usize > d.len() {
            break;
        }
        let v = &d[i + 2..i + 2 + d[i + 1] as usize];
        if a.n_opts < a.opts.len() {
            a.opts[a.n_opts] = code;
        }
        a.n_opts += 1;
        match code {
            53 if !v.is_empty() => a.msg_type = v[0],
            61 => a.client_id_ok = v.len() == 7 && v[0] == 1 && v[1..7] == a.chaddr,
            12 => {
                a.host_len = v.len().min(a.host.len());
                a.host[..a.host_len].copy_from_slice(&v[..a.host_len]);
            }
            55 => {
                a.n_params = v.len().min(a.params.len());
                a.params[..a.n_params].copy_from_slice(&v[..a.n_params]);
            }
            _ => {}
        }
        i += 2 + v.len();
    }
    a.fail = if a.eth_dst != [0xff; 6] {
        "eth-dst"
    } else if a.ethertype != 0x0800 {
        "ethertype"
    } else if a.ip_ver_ihl != 0x45 {
        "ip-ver-ihl"
    } else if !a.ip_csum_ok {
        "ip-csum"
    } else if a.ip_proto != udp::PROTO_UDP {
        "ip-proto"
    } else if a.ip_src != [0; 4] {
        "ip-src"
    } else if a.ip_dst != [0xff; 4] {
        "ip-dst"
    } else if a.ip_len as usize != 20 + a.udp_len as usize || 14 + a.ip_len as usize > f.len() {
        "ip-len"
    } else if a.udp_sport != CLIENT_PORT || a.udp_dport != SERVER_PORT {
        "udp-ports"
    } else if a.udp_csum_verdict == 2 {
        "udp-csum"
    } else if a.op != OP_REQUEST || a.htype != 1 || a.hlen != 6 || a.hops != 0 {
        "bootp-hdr"
    } else if a.flags & 0x8000 == 0 {
        "flags-broadcast"
    } else if a.chaddr != a.eth_src {
        "chaddr"
    } else if !a.cookie_ok {
        "cookie"
    } else if a.msg_type == 0 {
        "opt53"
    } else if !a.end_ok {
        "opt255"
    } else if a.bootp_len < BOOTP_MIN {
        "bootp-len"
    } else {
        "none"
    };
    Some(a)
}

/// Any IPv4 UDP frame with port 67 or 68 on either side — a server's OFFER/ACK to anyone, another client's
/// DISCOVER/REQUEST — decoded as far as its bytes go (BOOTP fields zero when the payload is under 240 octets or
/// carries no cookie).
#[derive(Clone, Copy)]
pub struct HeardDhcp {
    pub eth_src: [u8; 6],
    pub eth_dst: [u8; 6],
    pub ip_src: [u8; 4],
    pub ip_dst: [u8; 4],
    pub sport: u16,
    pub dport: u16,
    /// 0 none sent, 1 verifies, 2 wrong.
    pub udp_csum_verdict: u8,
    pub bootp_len: usize,
    pub op: u8,
    pub msg_type: u8,
    pub xid: u32,
    pub flags: u16,
    pub yiaddr: [u8; 4],
    pub chaddr: [u8; 6],
    pub server_id: [u8; 4],
    pub lease_s: u32,
}

/// Decode a heard UDP 67/68 frame. `None` when it is not IPv4 UDP with port 67 or 68 on either side.
pub fn hear(f: &[u8]) -> Option<HeardDhcp> {
    if f.len() < 34 || u16::from_be_bytes([f[12], f[13]]) != 0x0800 || f[23] != udp::PROTO_UDP {
        return None;
    }
    let l4 = 14 + ((f[14] & 0x0f) as usize) * 4;
    if f.len() < l4 + 8 {
        return None;
    }
    let sport = u16::from_be_bytes([f[l4], f[l4 + 1]]);
    let dport = u16::from_be_bytes([f[l4 + 2], f[l4 + 3]]);
    if !(sport == SERVER_PORT || sport == CLIENT_PORT || dport == SERVER_PORT || dport == CLIENT_PORT) {
        return None;
    }
    let ulen = u16::from_be_bytes([f[l4 + 4], f[l4 + 5]]) as usize;
    let seg_end = (l4 + ulen.max(8)).min(f.len());
    let mut h = HeardDhcp {
        eth_src: [0; 6], eth_dst: [0; 6], ip_src: [f[26], f[27], f[28], f[29]], ip_dst: [f[30], f[31], f[32], f[33]],
        sport, dport, udp_csum_verdict: udp_sum_verdict(&f[26..30], &f[30..34], &f[l4..seg_end]),
        bootp_len: seg_end - (l4 + 8), op: 0, msg_type: 0, xid: 0, flags: 0, yiaddr: [0; 4], chaddr: [0; 6],
        server_id: [0; 4], lease_s: 0,
    };
    h.eth_dst.copy_from_slice(&f[0..6]);
    h.eth_src.copy_from_slice(&f[6..12]);
    let d = &f[l4 + 8..seg_end];
    if d.len() < 240 || d[236..240] != MAGIC {
        return Some(h);
    }
    h.op = d[0];
    h.xid = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    h.flags = u16::from_be_bytes([d[10], d[11]]);
    h.yiaddr.copy_from_slice(&d[16..20]);
    h.chaddr.copy_from_slice(&d[28..34]);
    let mut i = 240;
    while i + 1 < d.len() {
        let code = d[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        let len = d[i + 1] as usize;
        if i + 2 + len > d.len() {
            break;
        }
        let v = &d[i + 2..i + 2 + len];
        match code {
            53 if len >= 1 => h.msg_type = v[0],
            54 if len >= 4 => h.server_id.copy_from_slice(&v[..4]),
            51 if len >= 4 => h.lease_s = u32::from_be_bytes([v[0], v[1], v[2], v[3]]),
            _ => {}
        }
        i += 2 + len;
    }
    Some(h)
}

/// RFC 2132 §9.6 message-type names.
pub fn msg_name(t: u8) -> &'static str {
    match t {
        1 => "discover",
        2 => "offer",
        3 => "request",
        4 => "decline",
        5 => "ack",
        6 => "nak",
        7 => "release",
        8 => "inform",
        0 => "none",
        _ => "other",
    }
}
