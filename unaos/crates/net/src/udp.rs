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

//! Minimal UDP: parse + build. The checksum covers the IPv4 pseudo-header
//! (src/dst IP, protocol, UDP length) plus the UDP header and payload.

/// UDP protocol number in the IPv4 header.
pub const PROTO_UDP: u8 = 17;

/// Zero-copy parser for a UDP datagram (header is 8 bytes).
pub struct UdpDatagram<'a> {
    buffer: &'a [u8],
}

impl<'a> UdpDatagram<'a> {
    pub fn new(buffer: &'a [u8]) -> Option<Self> {
        if buffer.len() < 8 {
            return None;
        }
        Some(Self { buffer })
    }

    pub fn source_port(&self) -> u16 {
        u16::from_be_bytes([self.buffer[0], self.buffer[1]])
    }

    pub fn dest_port(&self) -> u16 {
        u16::from_be_bytes([self.buffer[2], self.buffer[3]])
    }

    /// Datagram length (header + data) per the UDP `length` field.
    pub fn length(&self) -> u16 {
        u16::from_be_bytes([self.buffer[4], self.buffer[5]])
    }

    /// The UDP payload, bounded by both the `length` field and the buffer.
    pub fn payload(&self) -> &'a [u8] {
        let end = (self.length() as usize).min(self.buffer.len());
        if end < 8 {
            &[]
        } else {
            &self.buffer[8..end]
        }
    }
}

/// UDP checksum over the IPv4 pseudo-header + UDP header/data. A computed value of
/// 0 is transmitted as 0xFFFF (RFC 768), so 0 always means "no checksum".
fn checksum(src_ip: [u8; 4], dst_ip: [u8; 4], udp: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    // Pseudo-header: src(4) + dst(4) as 16-bit words, then protocol and UDP length.
    sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
    sum += PROTO_UDP as u32;
    sum += udp.len() as u32;
    // UDP header + data.
    let mut i = 0;
    while i + 1 < udp.len() {
        sum += u16::from_be_bytes([udp[i], udp[i + 1]]) as u32;
        i += 2;
    }
    if i < udp.len() {
        sum += (udp[i] as u32) << 8; // trailing odd byte in the high position
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let c = !(sum as u16);
    if c == 0 { 0xFFFF } else { c }
}

/// Write a UDP datagram (8-byte header + `payload`) into `out`, addressed from
/// `src_port`@`src_ip` to `dst_port`@`dst_ip` (the IPs are needed for the checksum
/// pseudo-header). Returns the total length, or `None` if `out` is too small.
pub fn write_datagram(
    out: &mut [u8],
    src_port: u16,
    dst_port: u16,
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    payload: &[u8],
) -> Option<usize> {
    let total = 8 + payload.len();
    if out.len() < total {
        return None;
    }
    out[0..2].copy_from_slice(&src_port.to_be_bytes());
    out[2..4].copy_from_slice(&dst_port.to_be_bytes());
    out[4..6].copy_from_slice(&(total as u16).to_be_bytes());
    out[6..8].copy_from_slice(&0u16.to_be_bytes()); // checksum zero before computing
    out[8..total].copy_from_slice(payload);
    let c = checksum(src_ip, dst_ip, &out[..total]);
    out[6..8].copy_from_slice(&c.to_be_bytes());
    Some(total)
}
