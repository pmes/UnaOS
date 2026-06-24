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

/// Protocol number for ICMP (IPv4 header `protocol` field).
pub const PROTO_ICMP: u8 = 1;

/// Standard 1's-complement checksum over `data` (even length expected for an IPv4
/// header). Returns the value to store in the checksum field.
fn ones_complement_sum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8; // trailing odd byte in the high position
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Write a minimal 20-byte IPv4 header into `out` for a packet carrying `payload_len`
/// bytes of `protocol` from `src` to `dst`. Sets TTL=64, DF, and the header checksum.
/// The payload is expected to follow at `out[20..]`. Returns 20, or `None` if too small.
pub fn write_header(
    out: &mut [u8],
    src: [u8; 4],
    dst: [u8; 4],
    protocol: u8,
    payload_len: usize,
) -> Option<usize> {
    if out.len() < 20 {
        return None;
    }
    let total = (20 + payload_len) as u16;
    out[0] = 0x45; // version 4, IHL 5 (20 bytes)
    out[1] = 0; // DSCP/ECN
    out[2..4].copy_from_slice(&total.to_be_bytes());
    out[4..6].copy_from_slice(&0u16.to_be_bytes()); // identification
    out[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // flags=DF, fragment offset 0
    out[8] = 64; // TTL
    out[9] = protocol;
    out[10..12].copy_from_slice(&0u16.to_be_bytes()); // checksum (zero before computing)
    out[12..16].copy_from_slice(&src);
    out[16..20].copy_from_slice(&dst);
    let csum = ones_complement_sum(&out[0..20]);
    out[10..12].copy_from_slice(&csum.to_be_bytes());
    Some(20)
}

/// A zero-copy parser for an IPv4 header.
pub struct Ipv4Header<'a> {
    buffer: &'a [u8],
}

impl<'a> Ipv4Header<'a> {
    /// Creates a new Ipv4Header parser from a raw byte slice.
    /// Returns `None` if the buffer is too small to contain a minimal IPv4 header.
    pub fn new(buffer: &'a [u8]) -> Option<Self> {
        // Minimum IPv4 header length is 20 bytes.
        if buffer.len() < 20 {
            return None;
        }

        // Check the version field (first 4 bits of the first byte)
        let version = buffer[0] >> 4;
        if version != 4 {
            return None;
        }

        // RFC 791: IHL counts 32-bit words and must be >= 5 (a 20-byte minimum
        // header). Reject sub-minimum values up front — otherwise header_length
        // would be < 20 and verify_checksum()/payload() would read the wrong
        // byte ranges (under-summing the checksum, mislabeling header as payload).
        let ihl = buffer[0] & 0x0F;
        if ihl < 5 {
            return None;
        }
        let header_length = (ihl as usize) * 4;

        // Ensure we have enough bytes for the reported header length
        if buffer.len() < header_length {
            return None;
        }

        Some(Self { buffer })
    }

    /// Header length in bytes, derived from the IHL field.
    pub fn header_length(&self) -> usize {
        let ihl = self.buffer[0] & 0x0F;
        (ihl as usize) * 4
    }

    /// Total length of the IPv4 packet (header + payload), in bytes.
    pub fn total_length(&self) -> u16 {
        u16::from_be_bytes([self.buffer[2], self.buffer[3]])
    }

    /// Protocol field (e.g., TCP, UDP, ICMP).
    pub fn protocol(&self) -> u8 {
        self.buffer[9]
    }

    /// Source IPv4 address.
    pub fn source_ip(&self) -> [u8; 4] {
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&self.buffer[12..16]);
        ip
    }

    /// Destination IPv4 address.
    pub fn destination_ip(&self) -> [u8; 4] {
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&self.buffer[16..20]);
        ip
    }

    /// Computes the 16-bit IPv4 header checksum natively.
    /// Returns true if the checksum is valid.
    pub fn verify_checksum(&self) -> bool {
        let mut sum: u32 = 0;
        let hlen = self.header_length();

        for i in (0..hlen).step_by(2) {
            let word = u16::from_be_bytes([self.buffer[i], self.buffer[i + 1]]);
            sum += word as u32;
        }

        // Add back carry outs from top 16 bits to low 16 bits
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        // The checksum should evaluate to 0xFFFF before inverting.
        // Therefore, taking the 1's complement of the sum should yield 0.
        (!sum as u16) == 0
    }

    /// The payload of the IPv4 packet.
    pub fn payload(&self) -> &'a [u8] {
        let header_len = self.header_length();
        let total_len = self.total_length() as usize;

        // Ensure bounds are safe to access
        let end_idx = total_len.min(self.buffer.len());

        if header_len > end_idx {
            &[]
        } else {
            &self.buffer[header_len..end_idx]
        }
    }
}
