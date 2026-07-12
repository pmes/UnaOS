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
//
// SOCK-1 (ROADMAP §1b): the smoltcp 0.13 adapter. A `smoltcp::phy::Device` over the e1000e RX/TX
// rings + an `Interface` (10.0.2.15/24, gw 10.0.2.2) that carries the shell's `ping`/`arp`/`netinfo`
// and the boot connectivity witness through the mature stack, KNOB-ON (`UNAOS_SMOLNET=1`). The
// hand-rolled `net` engines stay compiled and still own `connect`/`fetch`/`udpsend` this arc.
//
// Design (see docs/dev/OS/08_NET/networking.md):
//  * Everything is STATIC / stack-local — no heap growth. Each blocking op (`ping`/`arp`/witness)
//    builds a throwaway `Interface` + one ICMP socket on its own stack, pumps a bounded poll loop,
//    and drops it. smoltcp's neighbor cache re-ARPs per op (that IS the "ARP-triggering poll").
//  * The `Device` reaches the NIC via the additive `e1000::raw_rx`/`raw_tx` accessors, each of which
//    briefly locks `NET_DEVICE`. Tokens never hold that lock across another lock (no reentrancy).
//  * Poll-driven ONLY. Never invoked from the MSI handler. Single-CPU main-loop / shell discipline:
//    while a blocking op runs, `service_net()`'s hand-rolled `poll()` is not running (same CPU), so
//    the two RX drains never race. (Residual: a non-ICMP/ARP frame arriving inside a smolnet pump
//    window is dropped by smoltcp rather than served by the hand-rolled listeners — bounded, and no
//    worse than the hand-rolled `ping`'s own pump monopolising the CPU.)
//  * ARP MAC surfacing: smoltcp hides the resolved neighbor MAC, so the Device snoops inbound ARP
//    replies for the target IP (via `net::arp::learn`, a read-only reuse) into `snoop`.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::icmp;
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address,
};

use crate::drivers::e1000::{self, PingOutcome};

/// slirp's virtual gateway (the default route + the witness's ICMP target). Mirrors `e1000.rs`.
const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];
/// ICMP identifier stamped on every echo request we originate. ASCII "UN" (matches the hand-rolled
/// stack's `PING_IDENT`), so a shared responder can't confuse the two.
const PING_IDENT: u16 = 0x554E;
/// Payload carried in the echo requests we originate.
const PING_PAYLOAD: &[u8] = b"unaos-ping";
/// A full Ethernet frame fits (RX_BUF_SIZE in the driver is 2048); the scratch is stack-local.
const FRAME_CAP: usize = 1536;
/// ICMP socket ring capacities (packets / payload bytes). Our echoes are ~18 bytes; 512 is ample.
const ICMP_META: usize = 8;
const ICMP_PAYLOAD: usize = 512;
/// Bounded poll-pump iterations per blocking op. Iteration- (not wall-clock-) bounded to stay
/// clock-free; a reply on a local link lands in a handful of iterations, so this only caps how long
/// an unreachable target stalls the caller. Mirrors the hand-rolled `PUMP_ITERS`.
const PUMP_ITERS: i64 = 2_000_000;

// --- the Device adapter over the e1000e rings ---

/// A `smoltcp::phy::Device` backed by the e1000e. Owns its RX/TX scratch (so the RX/TX tokens can
/// borrow disjoint fields — smoltcp hands out both from one `receive()` to build a reply in place).
struct E1000Phy {
    rx: [u8; FRAME_CAP],
    rlen: usize,
    tx: [u8; FRAME_CAP],
    /// The IP whose ARP reply we want to surface as a MAC (`arp`/`ping` peer).
    target: [u8; 4],
    /// The snooped target MAC, once an ARP reply for `target` is seen on the wire.
    snoop: Option<[u8; 6]>,
}

impl E1000Phy {
    fn new(target: [u8; 4]) -> Self {
        E1000Phy { rx: [0; FRAME_CAP], rlen: 0, tx: [0; FRAME_CAP], target, snoop: None }
    }
}

/// Snoop an inbound ARP reply for `target`, recording the sender's MAC. Reuses `net::arp::learn`
/// (read-only) so the parse matches the hand-rolled stack exactly; the `net` crate is untouched.
fn snoop_arp(frame: &[u8], target: [u8; 4], out: &mut Option<[u8; 6]>) {
    if let Some(eth) = net::ethernet::EthernetFrame::new(frame) {
        if eth.ethertype() == net::ethernet::EtherType::Arp {
            if let Some((ip, mac)) = net::arp::learn(eth.payload()) {
                if ip == target {
                    *out = Some(mac);
                }
            }
        }
    }
}

struct PhyRxToken<'a> {
    buf: &'a [u8],
}
struct PhyTxToken<'a> {
    buf: &'a mut [u8],
}

impl RxToken for PhyRxToken<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(self.buf)
    }
}

impl TxToken for PhyTxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let n = len.min(self.buf.len());
        let r = f(&mut self.buf[..n]);
        e1000::raw_tx(&self.buf[..n]);
        r
    }
}

impl Device for E1000Phy {
    type RxToken<'a> = PhyRxToken<'a>;
    type TxToken<'a> = PhyTxToken<'a>;

    fn receive(&mut self, _t: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let len = e1000::raw_rx(&mut self.rx)?;
        snoop_arp(&self.rx[..len], self.target, &mut self.snoop);
        self.rlen = len;
        // Split the borrow into disjoint fields so both tokens can be handed out at once.
        let E1000Phy { rx, rlen, tx, .. } = self;
        Some((PhyRxToken { buf: &rx[..*rlen] }, PhyTxToken { buf: tx }))
    }

    fn transmit(&mut self, _t: Instant) -> Option<Self::TxToken<'_>> {
        Some(PhyTxToken { buf: &mut self.tx })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1500;
        caps
    }
}

// --- the blocking ICMP pump ---

/// Result of one [`pump`] run.
struct PumpResult {
    mac: Option<[u8; 6]>,
    sent: u16,
    received: u16,
}

/// Build a throwaway interface + ICMP socket and drive `count` echo requests to `target`, pumping a
/// bounded poll loop. `stop_on_arp` returns as soon as the target MAC is resolved (the `arp` path,
/// which doesn't care about the echo reply); otherwise it returns once `count` replies land. All
/// storage is stack-local — no heap growth.
fn pump(mac: [u8; 6], our_ip: [u8; 4], target: [u8; 4], count: u16, stop_on_arp: bool) -> PumpResult {
    let mut dev = E1000Phy::new(target);

    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    // A fixed seed is fine: ICMP has no port/sequence collision concern (SOCK-2's TCP will want a
    // real seed). ASCII "SCK1".
    config.random_seed = 0x5343_4B31;
    let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(
            IpAddress::v4(our_ip[0], our_ip[1], our_ip[2], our_ip[3]),
            24,
        ));
    });
    let _ = iface.routes_mut().add_default_ipv4_route(Ipv4Address::new(
        GATEWAY_IP[0],
        GATEWAY_IP[1],
        GATEWAY_IP[2],
        GATEWAY_IP[3],
    ));

    let mut rx_meta = [icmp::PacketMetadata::EMPTY; ICMP_META];
    let mut rx_payload = [0u8; ICMP_PAYLOAD];
    let mut tx_meta = [icmp::PacketMetadata::EMPTY; ICMP_META];
    let mut tx_payload = [0u8; ICMP_PAYLOAD];
    let rx_buffer = icmp::PacketBuffer::new(&mut rx_meta[..], &mut rx_payload[..]);
    let tx_buffer = icmp::PacketBuffer::new(&mut tx_meta[..], &mut tx_payload[..]);
    let socket = icmp::Socket::new(rx_buffer, tx_buffer);

    let mut storage: [SocketStorage; 1] = Default::default();
    let mut sockets = SocketSet::new(&mut storage[..]);
    let handle = sockets.add(socket);
    if sockets
        .get_mut::<icmp::Socket>(handle)
        .bind(icmp::Endpoint::Ident(PING_IDENT))
        .is_err()
    {
        return PumpResult { mac: None, sent: 0, received: 0 };
    }

    let remote = IpAddress::v4(target[0], target[1], target[2], target[3]);
    let mut sent = 0u16;
    let mut received = 0u16;
    let mut seq = 0u16;
    let mut clock: i64 = 0;

    while clock < PUMP_ITERS {
        clock += 1;
        iface.poll(Instant::from_millis(clock), &mut dev, &mut sockets);

        if stop_on_arp && dev.snoop.is_some() {
            break;
        }

        let sock = sockets.get_mut::<icmp::Socket>(handle);
        if seq < count && sock.can_send() {
            seq += 1;
            let repr = Icmpv4Repr::EchoRequest { ident: PING_IDENT, seq_no: seq, data: PING_PAYLOAD };
            if let Ok(buf) = sock.send(repr.buffer_len(), remote) {
                let mut pkt = Icmpv4Packet::new_unchecked(buf);
                let caps = dev.capabilities().checksum;
                repr.emit(&mut pkt, &caps);
                sent += 1;
            }
        }
        if sock.can_recv() {
            if let Ok((payload, _addr)) = sock.recv() {
                if let Ok(pkt) = Icmpv4Packet::new_checked(payload) {
                    let caps = dev.capabilities().checksum;
                    if let Ok(Icmpv4Repr::EchoReply { .. }) = Icmpv4Repr::parse(&pkt, &caps) {
                        received += 1;
                        if received >= count {
                            break;
                        }
                    }
                }
            }
        }
    }

    PumpResult { mac: dev.snoop, sent, received }
}

// --- public entry points (called from the shell net-command region + service_net) ---

/// Blocking smoltcp ICMP ping for the `ping` shell command. Returns the same [`PingOutcome`] shape
/// the hand-rolled path does, so the shell renderer is unchanged. `None` if no NIC.
pub fn ping(ip: [u8; 4], count: u16) -> Option<PingOutcome> {
    let (mac, our_ip, _up) = e1000::hw_addr()?;
    let r = pump(mac, our_ip, ip, count, false);
    Some(PingOutcome {
        // "resolved" mirrors the hand-rolled semantics: an unreachable host (no ARP, no reply)
        // reports unresolved so the shell prints "host unreachable".
        resolved: r.mac.is_some() || r.received > 0,
        mac: r.mac,
        sent: r.sent,
        received: r.received,
    })
}

/// Blocking smoltcp ARP resolve for the `arp` shell command: send one echo (which forces smoltcp to
/// ARP the target) and return the snooped peer MAC. `None` if unresolved / no NIC.
pub fn arp_resolve(ip: [u8; 4]) -> Option<[u8; 6]> {
    let (mac, our_ip, _up) = e1000::hw_addr()?;
    pump(mac, our_ip, ip, 1, true).mac
}

/// A one-line summary of the smolnet interface for `netinfo` (knob-on).
pub fn info_line() -> alloc::string::String {
    match e1000::hw_addr() {
        Some((_mac, ip, up)) => alloc::format!(
            "smolnet: iface {}/24  gw {}  medium=ethernet  link {}  (smoltcp 0.13.1)",
            e1000::fmt_ip(&ip),
            e1000::fmt_ip(&GATEWAY_IP),
            if up { "UP" } else { "DOWN" }
        ),
        None => alloc::string::String::from("smolnet: no interface (no NIC)"),
    }
}

// --- the boot connectivity witness (M2), driven one-shot from service_net knob-on ---

/// True once the witness has run (one-shot).
static WITNESS_DONE: AtomicBool = AtomicBool::new(false);
/// service_net call counter — lets the link/NIC settle before the witness fires.
static WITNESS_TICKS: AtomicU32 = AtomicU32::new(0);
/// service_net calls to skip before arming the witness (the boot self-test + link bring-up warm up).
const WITNESS_WARMUP: u32 = 16;

/// One-shot smoltcp connectivity witness: knob-on, ping the gateway ×4 through the mature stack and
/// emit the UNCOUNTED witness line. Called from `e1000::service_net()` each main-loop pass, AFTER
/// the NET_DEVICE guard is dropped (the ping pump re-locks it per ring op). No-op once done / no NIC.
pub fn witness_tick() {
    if WITNESS_DONE.load(Ordering::Relaxed) {
        return;
    }
    if WITNESS_TICKS.fetch_add(1, Ordering::Relaxed) < WITNESS_WARMUP {
        return;
    }
    // Only run once the NIC exists (link state isn't a precondition — smoltcp drives its own ARP).
    if e1000::hw_addr().is_none() {
        return;
    }
    WITNESS_DONE.store(true, Ordering::Relaxed);

    let recv = ping(GATEWAY_IP, 4).map(|o| o.received).unwrap_or(0);
    serial_println!(
        ":: SOCK-1: smoltcp icmp echo {} {}/4 replies — witness {} ::",
        e1000::fmt_ip(&GATEWAY_IP),
        recv,
        if recv >= 4 { "OK" } else { "INCOMPLETE" }
    );
}
