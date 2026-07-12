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

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{icmp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpEndpoint,
    Ipv4Address,
};
use spin::Mutex as SpinMutex;

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

// =============================================================================
// SOCK-2 (ROADMAP §1b): the PERSISTENT smoltcp interface + UDP SocketSet backing
// the socket syscall family (`sys_socket`/`sys_bind`/`sys_sendto`/`sys_recvfrom`).
//
// SOCK-1's ICMP path builds a THROWAWAY Interface + one socket per blocking op. A
// UDP socket has to survive BETWEEN `bind` and `recvfrom` (separate syscalls), so
// SOCK-2 promotes the interface + a real `SocketSet` to a persistent, static-backed
// singleton behind `STACK` (a `spin::Mutex<Option<SmolStack>>`, the `NET_DEVICE`
// mirror). EVERYTHING is static / BSS: the socket-set storage, every UDP socket's
// packet buffers, and the device RX/TX scratch (the ~3 KiB of frame buffers live in
// the `SmolStack` field, NOT on any stack — so the ~5-7 KiB smolnet frame budget the
// arc mandate caps never migrates onto a 16 KiB task/AP stack; only smoltcp's own
// ~2 KiB poll frames touch the caller's stack, and only on the BSP-main-loop witness
// path and the IF-masked syscall path — never an AP scheduler stack). Heap reach = STOP.
//
// The syscall handler runs IF-masked (it cannot `hlt`/block), so `stack_recvfrom` is
// NON-BLOCKING: it drives a BOUNDED poll pump (iteration-, not clock-bounded, exactly
// like SOCK-1's `pump`) and returns the datagram if one landed, else `None` (the
// syscall maps that to `-EAGAIN`). Because the pump reads the RX ring directly (not
// interrupt-gated) and runs to completion inside ONE IF-masked syscall, the whole
// ARP → egress → reply round-trip completes without the main-loop `service_net` poll
// racing it for frames (single CPU; the handler holds the core).
// =============================================================================

/// slirp's built-in DNS server (10.0.2.3:53) — the hermetic UDP responder for the
/// round-trip witnesses. slirp answers ARP for it and replies to a DNS query with a
/// real UDP datagram, so a genuine send→receive round-trip works under the default
/// `./arroyo test` slirp backend with NO external injector and NO netdev change. (The
/// `scripts/net-inject.py` gateway UDP echo on port 9998 is the alternate medium when
/// running under `UNAOS_NET=socket`.)
const DNS_IP: [u8; 4] = [10, 0, 2, 3];
const DNS_PORT: u16 = 53;
/// A minimal DNS A-query for "una.os" (txn id 0x5343 = "SC"). slirp forwards it and
/// returns a datagram (an answer or an error) FROM 10.0.2.3:53 — either proves the
/// round-trip. Emitted verbatim by both the kernel witness and the ring-3 fixture.
const DNS_QUERY: &[u8] = &[
    0x53, 0x43, // txn id
    0x01, 0x00, // flags: RD
    0x00, 0x01, // QDCOUNT = 1
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // AN/NS/AR = 0
    0x03, b'u', b'n', b'a', 0x02, b'o', b's', 0x00, // QNAME "una.os"
    0x00, 0x01, // QTYPE = A
    0x00, 0x01, // QCLASS = IN
];

/// Number of concurrent UDP sockets the persistent set holds (process-scoped caps).
const NSOCK: usize = 4;
/// Per-socket packet-buffer capacities (datagrams / payload bytes). A DNS reply is
/// well under 512 B; 1 KiB payload + 8 packets is ample headroom.
const UDP_PKTS: usize = 8;
const UDP_BUF: usize = 1024;
/// Largest datagram payload a `sys_sendto`/`sys_recvfrom` moves (bounds the user copy).
pub const UDP_MAX_PAYLOAD: usize = UDP_BUF;
/// Bounded poll-pump budgets (iteration-bounded to stay clock-free; a slirp reply lands
/// in a handful of iterations, so these only cap how long an unreachable peer stalls the
/// IF-masked caller — never a hang). `send` just needs to kick ARP + egress; `recv`
/// pumps long enough to complete ARP → egress → reply capture in one call.
const SEND_PUMP: i64 = 20_000;
const RECV_PUMP: i64 = 400_000;

// --- static socket-set + per-socket packet-buffer storage (all BSS, `&'static mut`) ---
static mut SOCK_SET_STORAGE: [SocketStorage<'static>; NSOCK] = [SocketStorage::EMPTY; NSOCK];
static mut UDP_RX_META: [[udp::PacketMetadata; UDP_PKTS]; NSOCK] =
    [[udp::PacketMetadata::EMPTY; UDP_PKTS]; NSOCK];
static mut UDP_RX_DATA: [[u8; UDP_BUF]; NSOCK] = [[0u8; UDP_BUF]; NSOCK];
static mut UDP_TX_META: [[udp::PacketMetadata; UDP_PKTS]; NSOCK] =
    [[udp::PacketMetadata::EMPTY; UDP_PKTS]; NSOCK];
static mut UDP_TX_DATA: [[u8; UDP_BUF]; NSOCK] = [[0u8; UDP_BUF]; NSOCK];

/// Monotonic millisecond clock fed to `iface.poll` — bumped per poll across ALL callers
/// so smoltcp's neighbor/ARP timers advance consistently. Iteration-driven, clock-free.
static POLL_CLOCK: AtomicI64 = AtomicI64::new(1);

/// The persistent stack singleton. Its fields (incl. the 3 KiB device RX/TX scratch)
/// live in BSS via this static, so nothing large sits on the caller's stack.
struct SmolStack {
    iface: Interface,
    sockets: SocketSet<'static>,
    dev: E1000Phy,
    /// socket-id (index) → (smoltcp handle, owning HANDLES row). `None` = free slot.
    reg: [Option<(SocketHandle, usize)>; NSOCK],
}

static STACK: SpinMutex<Option<SmolStack>> = SpinMutex::new(None);

/// Build the persistent stack ONCE (idempotent). Constructs the interface + empty
/// `SocketSet` from the static storage and moves them into `STACK`. Called from the
/// BSP-main-loop witness and the launcher (both large-stack, shallow-chain) BEFORE any
/// ring-3 `sys_socket`, so the one-time construction transient never lands on a ring-3
/// task's syscall stack. `false` if there is no NIC yet.
fn ensure_stack(guard: &mut Option<SmolStack>) -> bool {
    if guard.is_some() {
        return true;
    }
    let Some((mac, our_ip, _up)) = e1000::hw_addr() else {
        return false;
    };
    let mut dev = E1000Phy::new([0; 4]);
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = 0x5343_4B32; // "SCK2"
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
    // SAFETY: the storage statics are borrowed `&'static mut` EXACTLY ONCE, here, under
    // the `STACK` lock with `guard` proven `None` — no aliasing. `SocketSet::new` retains
    // the borrow for the singleton's life; per-socket buffers are borrowed disjointly in
    // `stack_open` (a free `reg` slot ⇒ its buffer set is unborrowed).
    let storage: &'static mut [SocketStorage<'static>] =
        unsafe { &mut *core::ptr::addr_of_mut!(SOCK_SET_STORAGE) };
    let sockets = SocketSet::new(storage);
    *guard = Some(SmolStack { iface, sockets, dev, reg: [None; NSOCK] });
    true
}

/// Drive `iters` poll iterations against the persistent interface (bounded, clock-free).
/// Split-borrows the `SmolStack` fields so `iface.poll` gets `&mut dev` + `&mut sockets`
/// disjointly. Reads the RX ring directly (`raw_rx` in the Device), so it drives ARP,
/// egress of queued datagrams, and inbound delivery — no interrupts required.
fn stack_pump(stack: &mut SmolStack, iters: i64) {
    let SmolStack { iface, sockets, dev, .. } = stack;
    for _ in 0..iters {
        let now = POLL_CLOCK.fetch_add(1, Ordering::Relaxed);
        iface.poll(Instant::from_millis(now), dev, sockets);
    }
}

/// Build the persistent stack now (idempotent) on the CALLER's stack — call this from a large-stack,
/// shallow-chain context (the BSP main loop or a launcher task) BEFORE any ring-3 `sys_socket`, so the
/// one-time ~4 KiB construction transient never lands on a ring-3 task's syscall stack. `false` if no NIC.
pub fn init() -> bool {
    let mut g = STACK.lock();
    ensure_stack(&mut g)
}

/// Allocate a UDP socket in the persistent set, owned by HANDLES row `owner`. Builds the
/// socket from the free slot's STATIC packet buffers and records the smoltcp handle.
/// Returns the socket-id (the `reg` index) or `None` if all `NSOCK` slots are in use / no NIC.
pub fn stack_open(owner: usize) -> Option<usize> {
    let mut g = STACK.lock();
    if !ensure_stack(&mut g) {
        return None;
    }
    let stack = g.as_mut().unwrap();
    let sid = stack.reg.iter().position(|s| s.is_none())?;
    // SAFETY: `sid` is a free `reg` slot, so buffer set `sid` is not borrowed by any live
    // socket; borrow its four static buffers `&'static mut` disjointly (distinct statics,
    // distinct indices) to build the socket, which then OWNS the borrows until removed.
    // `addr_of_mut!(STATIC[sid])` names the element as a PLACE (no intermediate reference),
    // then `from_raw_parts_mut` re-forms the slice — avoiding an autoref through a raw deref.
    let (rx_meta, rx_data, tx_meta, tx_data): (
        &'static mut [udp::PacketMetadata],
        &'static mut [u8],
        &'static mut [udp::PacketMetadata],
        &'static mut [u8],
    ) = unsafe {
        (
            core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(UDP_RX_META[sid]) as *mut udp::PacketMetadata,
                UDP_PKTS,
            ),
            core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(UDP_RX_DATA[sid]) as *mut u8,
                UDP_BUF,
            ),
            core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(UDP_TX_META[sid]) as *mut udp::PacketMetadata,
                UDP_PKTS,
            ),
            core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(UDP_TX_DATA[sid]) as *mut u8,
                UDP_BUF,
            ),
        )
    };
    let socket = udp::Socket::new(
        udp::PacketBuffer::new(rx_meta, rx_data),
        udp::PacketBuffer::new(tx_meta, tx_data),
    );
    let handle = stack.sockets.add(socket);
    stack.reg[sid] = Some((handle, owner));
    Some(sid)
}

/// Bind socket `sid` to a local UDP `port`. `Ok(())` or `Err(())` (already bound / port 0
/// / unknown sid). Idempotent-unfriendly by smoltcp design (rebinding an open socket errors).
pub fn stack_bind(sid: usize, port: u16) -> Result<(), ()> {
    let mut g = STACK.lock();
    let stack = g.as_mut().ok_or(())?;
    let (handle, _) = *stack.reg.get(sid).and_then(|s| s.as_ref()).ok_or(())?;
    stack.sockets.get_mut::<udp::Socket>(handle).bind(port).map_err(|_| ())
}

/// Queue `payload` to `ip:port` on socket `sid` and pump a short bounded loop to kick ARP
/// + egress. `Ok(len)` once queued (best-effort egress), or `Err(())` (socket can't send /
/// buffer full / unbound / unknown sid) → the syscall maps that to `-EAGAIN`.
pub fn stack_sendto(sid: usize, ip: [u8; 4], port: u16, payload: &[u8]) -> Result<usize, ()> {
    let mut g = STACK.lock();
    let stack = g.as_mut().ok_or(())?;
    let (handle, _) = *stack.reg.get(sid).and_then(|s| s.as_ref()).ok_or(())?;
    let ep = IpEndpoint::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), port);
    {
        let sock = stack.sockets.get_mut::<udp::Socket>(handle);
        if !sock.can_send() {
            return Err(());
        }
        sock.send_slice(payload, ep).map_err(|_| ())?;
    }
    stack_pump(stack, SEND_PUMP);
    Ok(payload.len())
}

/// Non-blocking receive on socket `sid`. Pumps a bounded loop (driving ARP → egress →
/// inbound delivery in this ONE IF-masked call), then returns the first datagram
/// `(src_ip, src_port, len)` copied into `out` (truncated to `out.len()`), or `None` if
/// none arrived within the budget → the syscall maps that to `-EAGAIN`. NEVER blocks.
pub fn stack_recvfrom(sid: usize, out: &mut [u8]) -> Option<([u8; 4], u16, usize)> {
    let mut g = STACK.lock();
    let stack = g.as_mut()?;
    let (handle, _) = *stack.reg.get(sid).and_then(|s| s.as_ref())?;
    // Pump in chunks, checking for a delivered datagram between chunks so a fast reply
    // returns promptly without burning the whole budget.
    let mut spent = 0i64;
    while spent < RECV_PUMP {
        stack_pump(stack, 4_000);
        spent += 4_000;
        let sock = stack.sockets.get_mut::<udp::Socket>(handle);
        if sock.can_recv() {
            if let Ok((data, meta)) = sock.recv() {
                let n = data.len().min(out.len());
                out[..n].copy_from_slice(&data[..n]);
                let IpAddress::Ipv4(v4) = meta.endpoint.addr;
                return Some((v4.octets(), meta.endpoint.port, n));
            }
        }
    }
    None
}

/// Remove socket `sid` from the persistent set (drops the udp socket, releasing its static
/// buffers for reuse) and free its `reg` slot. No-op on an unknown/free sid.
pub fn stack_close(sid: usize) {
    let mut g = STACK.lock();
    let Some(stack) = g.as_mut() else { return };
    if let Some(Some((handle, _))) = stack.reg.get(sid).copied() {
        stack.sockets.remove(handle);
        stack.reg[sid] = None;
    }
}

/// Teardown hook: free every socket OWNED by HANDLES row `row` (the caller exited / was
/// killed). Called from `clear_handle_row` so a dying task leaks no persistent socket and
/// a reused slot inherits none. No-op if the stack was never built.
pub fn free_row_sockets(row: usize) {
    let mut g = STACK.lock();
    let Some(stack) = g.as_mut() else { return };
    for sid in 0..NSOCK {
        if let Some((handle, owner)) = stack.reg[sid] {
            if owner == row {
                stack.sockets.remove(handle);
                stack.reg[sid] = None;
            }
        }
    }
}

// --- the boot UDP round-trip witness (M1), driven one-shot from service_net knob-on ---

/// True once the SOCK-2 witness has run (one-shot).
static WITNESS2_DONE: AtomicBool = AtomicBool::new(false);
/// service_net call counter — lets the link/NIC settle before the witness fires.
static WITNESS2_TICKS: AtomicU32 = AtomicU32::new(0);
/// Warm up a little past SOCK-1's witness so the two don't contend on the first passes.
const WITNESS2_WARMUP: u32 = 24;

/// One-shot kernel-side UDP round-trip witness (M1): open a UDP socket in the PERSISTENT
/// set, bind an ephemeral port, send a DNS query to slirp's resolver, pump, receive the
/// reply, close, and emit the UNCOUNTED witness line. Proves the persistent interface +
/// SocketSet carry a real datagram round-trip end to end from the kernel. No-op once done
/// / no NIC. Runs from `service_net` AFTER the NET_DEVICE guard drops (the pump re-locks
/// NET_DEVICE per ring op), on the BSP main loop.
pub fn witness_tick2() {
    if WITNESS2_DONE.load(Ordering::Relaxed) {
        return;
    }
    if WITNESS2_TICKS.fetch_add(1, Ordering::Relaxed) < WITNESS2_WARMUP {
        return;
    }
    if e1000::hw_addr().is_none() {
        return;
    }
    WITNESS2_DONE.store(true, Ordering::Relaxed);

    let recvd = udp_dns_roundtrip();
    serial_println!(
        ":: SOCK-2: smoltcp udp dns query {}:{} -> {} bytes back — witness {} ::",
        e1000::fmt_ip(&DNS_IP),
        DNS_PORT,
        recvd.map(|(_, _, n)| n).unwrap_or(0),
        if matches!(recvd, Some((ip, DNS_PORT, n)) if ip == DNS_IP && n >= 4) {
            "OK"
        } else {
            "INCOMPLETE"
        }
    );
}

/// Kernel-side one-shot DNS round-trip over the persistent stack: open → bind → sendto →
/// recvfrom → close. Returns the reply `(src_ip, src_port, len)` or `None`. The `owner`
/// row is `SHARED_ROW`-like kernel ownership (`usize::MAX`), never a live ring-3 slot, so
/// `free_row_sockets` for any real task never touches it; it is closed explicitly here.
fn udp_dns_roundtrip() -> Option<([u8; 4], u16, usize)> {
    let sid = stack_open(usize::MAX)?;
    let _ = stack_bind(sid, 49200);
    let _ = stack_sendto(sid, DNS_IP, DNS_PORT, DNS_QUERY);
    let mut buf = [0u8; 64];
    let r = stack_recvfrom(sid, &mut buf);
    stack_close(sid);
    r
}
