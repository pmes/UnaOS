#![no_std]

pub mod arp;
pub mod ethernet;
pub mod interface;
pub mod ipv4;

use arp::ArpStateMachine;
use ethernet::{EtherType, EthernetFrame};
use ipv4::Ipv4Header;

/// The central ingress router.
///
/// Takes a raw Ethernet frame `&[u8]` and routes it up the OSI stack.
/// It reads the Ethernet II EtherType to determine the next layer.
pub fn ingress(buffer: &[u8], arp_state: &ArpStateMachine) {
    let frame = match EthernetFrame::new(buffer) {
        Some(f) => f,
        None => return, // Drop invalid/undersized frames
    };

    let payload = frame.payload();

    match frame.ethertype() {
        EtherType::Arp => {
            // Hands the payload to the ARP module.
            // A higher-level handler or device abstraction would actually transmit the reply.
            // This satisfies the routing logic requirement for now.
            if let Some((_reply_bytes, _dest_mac)) = arp_state.process_packet(payload) {
                // Here, we would construct a new EthernetFrame using _dest_mac,
                // our own MAC as source, and EtherType::Arp, then call `device.transmit(...)`.
                // For the scope of `ingress`, routing to the module is sufficient.
            }
        }
        EtherType::Ipv4 => {
            // Hands the payload to the IPv4 module.
            if let Some(ipv4_header) = Ipv4Header::new(payload) {
                if ipv4_header.verify_checksum() {
                    // Valid IPv4 packet.
                    // Further routing (e.g., to TCP, UDP, ICMP) would happen here based on `ipv4_header.protocol()`.
                }
            }
        }
        _ => {
            // Drop unsupported protocols.
        }
    }
}
