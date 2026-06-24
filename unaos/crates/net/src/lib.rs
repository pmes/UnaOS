#![no_std]
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


pub mod arp;
pub mod ethernet;
pub mod icmp;
pub mod interface;
pub mod ipv4;

use arp::ArpStateMachine;
use ethernet::{EtherType, EthernetFrame};
use ipv4::Ipv4Header;

/// The central ingress router.
///
/// Takes a raw Ethernet frame `&[u8]`, routes it up the stack, and — when a reply
/// is warranted — writes a complete outgoing Ethernet frame into `tx_buf` and
/// returns its length. The caller (the NIC driver) transmits `tx_buf[..len]`.
/// Returns `None` when nothing should be sent (dropped/unsupported/no reply).
pub fn ingress(buffer: &[u8], arp_state: &ArpStateMachine, tx_buf: &mut [u8]) -> Option<usize> {
    let frame = EthernetFrame::new(buffer)?; // Drop invalid/undersized frames

    let payload = frame.payload();

    match frame.ethertype() {
        EtherType::Arp => {
            // Build and return an ARP reply (wrapped in an Ethernet frame) when the
            // request targets our IP; the driver transmits it.
            let (reply, dest_mac) = arp_state.process_packet(payload)?;
            ethernet::write_frame(
                tx_buf,
                dest_mac,
                arp_state.our_mac(),
                EtherType::Arp.as_u16(),
                &reply,
            )
        }
        EtherType::Ipv4 => {
            let ip = Ipv4Header::new(payload)?;
            // Only handle valid packets addressed to us.
            if !ip.verify_checksum() || ip.destination_ip() != arp_state.our_ip() {
                return None;
            }
            if ip.protocol() != ipv4::PROTO_ICMP {
                return None; // UDP/TCP dispatch arrives in later phases.
            }

            // Reply to ICMP echo requests (ping). Build the response bottom-up directly
            // into tx_buf: Ethernet[0..14] | IPv4[14..34] | ICMP[34..].
            let icmp_len = icmp::write_echo_reply(tx_buf.get_mut(34..)?, ip.payload())?;
            ipv4::write_header(
                tx_buf.get_mut(14..34)?,
                arp_state.our_ip(),
                ip.source_ip(),
                ipv4::PROTO_ICMP,
                icmp_len,
            )?;
            ethernet::write_header(
                tx_buf.get_mut(0..14)?,
                frame.source_mac(),
                arp_state.our_mac(),
                EtherType::Ipv4.as_u16(),
            )?;
            Some(14 + 20 + icmp_len)
        }
        _ => None, // Drop unsupported protocols.
    }
}
