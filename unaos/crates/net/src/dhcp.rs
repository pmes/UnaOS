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
