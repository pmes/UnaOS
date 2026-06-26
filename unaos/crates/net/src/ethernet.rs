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

/// Represents the EtherType field of an Ethernet II frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtherType {
    Ipv4,
    Arp,
    Ipv6,
    Unknown(u16),
}

impl EtherType {
    pub fn from_u16(value: u16) -> Self {
        match value {
            0x0800 => EtherType::Ipv4,
            0x0806 => EtherType::Arp,
            0x86DD => EtherType::Ipv6,
            _ => EtherType::Unknown(value),
        }
    }

    /// The 16-bit on-the-wire value for this EtherType.
    pub fn as_u16(self) -> u16 {
        match self {
            EtherType::Ipv4 => 0x0800,
            EtherType::Arp => 0x0806,
            EtherType::Ipv6 => 0x86DD,
            EtherType::Unknown(v) => v,
        }
    }
}

/// Write just the 14-byte Ethernet II header into `buf`. Returns 14, or `None` if
/// `buf` is too small. Used when the payload is already laid out after the header
/// (e.g. an IPv4 packet built in place).
pub fn write_header(buf: &mut [u8], dst: [u8; 6], src: [u8; 6], ethertype: u16) -> Option<usize> {
    if buf.len() < 14 {
        return None;
    }
    buf[0..6].copy_from_slice(&dst);
    buf[6..12].copy_from_slice(&src);
    buf[12..14].copy_from_slice(&ethertype.to_be_bytes());
    Some(14)
}

/// Build an Ethernet II frame (14-byte header + `payload`) into `buf`. Returns the
/// total frame length written, or `None` if `buf` is too small. Caller-provided
/// buffer keeps this `no_std` and allocation-free; the NIC driver pads short frames.
pub fn write_frame(
    buf: &mut [u8],
    dst: [u8; 6],
    src: [u8; 6],
    ethertype: u16,
    payload: &[u8],
) -> Option<usize> {
    let total = 14 + payload.len();
    if buf.len() < total {
        return None;
    }
    write_header(buf, dst, src, ethertype)?;
    buf[14..total].copy_from_slice(payload);
    Some(total)
}

/// A zero-copy parser for an Ethernet II frame.
pub struct EthernetFrame<'a> {
    buffer: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    /// Creates a new EthernetFrame parser from a raw byte slice.
    /// Returns `None` if the buffer is too small to contain an Ethernet header.
    pub fn new(buffer: &'a [u8]) -> Option<Self> {
        // Minimum Ethernet II header size is 14 bytes:
        // 6 bytes (Dest MAC) + 6 bytes (Src MAC) + 2 bytes (EtherType)
        if buffer.len() < 14 {
            return None;
        }
        Some(Self { buffer })
    }

    /// The destination MAC address (bytes 0..6).
    pub fn destination_mac(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&self.buffer[0..6]);
        mac
    }

    /// The source MAC address (bytes 6..12).
    pub fn source_mac(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&self.buffer[6..12]);
        mac
    }

    /// The raw 16-bit EtherType field (bytes 12..14).
    pub fn ethertype_raw(&self) -> u16 {
        u16::from_be_bytes([self.buffer[12], self.buffer[13]])
    }

    /// The parsed EtherType.
    pub fn ethertype(&self) -> EtherType {
        EtherType::from_u16(self.ethertype_raw())
    }

    /// The payload of the Ethernet frame (everything after byte 14).
    pub fn payload(&self) -> &'a [u8] {
        &self.buffer[14..]
    }
}
