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

/// ARP operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpOperation {
    Request,
    Reply,
    Unknown(u16),
}

impl ArpOperation {
    pub fn from_u16(value: u16) -> Self {
        match value {
            1 => ArpOperation::Request,
            2 => ArpOperation::Reply,
            _ => ArpOperation::Unknown(value),
        }
    }
}

/// A zero-copy parser for an ARP packet over Ethernet and IPv4.
pub struct ArpPacket<'a> {
    buffer: &'a [u8],
}

impl<'a> ArpPacket<'a> {
    /// Creates a new ArpPacket parser from a raw byte slice.
    /// Returns `None` if the buffer is too small to contain an ARP packet.
    pub fn new(buffer: &'a [u8]) -> Option<Self> {
        // Minimum ARP packet size for Ethernet (Hardware Type 1) + IPv4 (Protocol Type 0x0800) is 28 bytes.
        if buffer.len() < 28 {
            return None;
        }

        Some(Self { buffer })
    }

    /// The Hardware Type field (e.g., Ethernet is 1).
    pub fn hardware_type(&self) -> u16 {
        u16::from_be_bytes([self.buffer[0], self.buffer[1]])
    }

    /// The Protocol Type field (e.g., IPv4 is 0x0800).
    pub fn protocol_type(&self) -> u16 {
        u16::from_be_bytes([self.buffer[2], self.buffer[3]])
    }

    /// Length of hardware address (MAC).
    pub fn hardware_address_length(&self) -> u8 {
        self.buffer[4]
    }

    /// Length of protocol address (IP).
    pub fn protocol_address_length(&self) -> u8 {
        self.buffer[5]
    }

    /// The operation (1 for Request, 2 for Reply).
    pub fn operation_raw(&self) -> u16 {
        u16::from_be_bytes([self.buffer[6], self.buffer[7]])
    }

    /// The parsed ARP operation.
    pub fn operation(&self) -> ArpOperation {
        ArpOperation::from_u16(self.operation_raw())
    }

    /// Sender Hardware Address (MAC).
    pub fn sender_hardware_address(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&self.buffer[8..14]);
        mac
    }

    /// Sender Protocol Address (IPv4).
    pub fn sender_protocol_address(&self) -> [u8; 4] {
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&self.buffer[14..18]);
        ip
    }

    /// Target Hardware Address (MAC).
    pub fn target_hardware_address(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&self.buffer[18..24]);
        mac
    }

    /// Target Protocol Address (IPv4).
    pub fn target_protocol_address(&self) -> [u8; 4] {
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&self.buffer[24..28]);
        ip
    }
}

/// A state machine for processing ARP operations and generating replies.
pub struct ArpStateMachine {
    our_ip: [u8; 4],
    our_mac: [u8; 6],
}

impl ArpStateMachine {
    /// Creates a new ARP state machine with our IP and MAC.
    pub fn new(our_ip: [u8; 4], our_mac: [u8; 6]) -> Self {
        Self { our_ip, our_mac }
    }

    /// Processes an incoming ARP packet and optionally generates an ARP reply payload (for the ethernet frame).
    /// Returns `Some(reply_bytes)` if a reply is needed, where `reply_bytes` contains the generated ARP payload and the destination MAC.
    /// The generated payload requires an Ethernet frame wrapper.
    pub fn process_packet(&self, buffer: &[u8]) -> Option<([u8; 28], [u8; 6])> {
        let packet = ArpPacket::new(buffer)?;

        // Only handle IPv4 over Ethernet
        if packet.hardware_type() != 1 || packet.protocol_type() != 0x0800 {
            return None;
        }

        if packet.operation() != ArpOperation::Request {
            return None;
        }

        // If the request is for us, generate a reply
        if packet.target_protocol_address() == self.our_ip {
            let mut reply = [0u8; 28];

            // Hardware Type (Ethernet = 1)
            reply[0..2].copy_from_slice(&1u16.to_be_bytes());
            // Protocol Type (IPv4 = 0x0800)
            reply[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
            // Hardware Address Length (6)
            reply[4] = 6;
            // Protocol Address Length (4)
            reply[5] = 4;
            // Operation (Reply = 2)
            reply[6..8].copy_from_slice(&2u16.to_be_bytes());

            // Sender Hardware Address (Our MAC)
            reply[8..14].copy_from_slice(&self.our_mac);
            // Sender Protocol Address (Our IP)
            reply[14..18].copy_from_slice(&self.our_ip);

            // Target Hardware Address (The original sender's MAC)
            reply[18..24].copy_from_slice(&packet.sender_hardware_address());
            // Target Protocol Address (The original sender's IP)
            reply[24..28].copy_from_slice(&packet.sender_protocol_address());

            return Some((reply, packet.sender_hardware_address()));
        }

        None
    }
}
