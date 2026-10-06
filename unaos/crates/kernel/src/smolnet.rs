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
// rings + an `Interface` (10.0.2.15/24, gw 10.0.2.2) that carries the shell's `ping`/`arp`/`ifconfig`
// and the boot connectivity witness through the mature stack. As of SMOLNET-DEFAULT (2026-07-17)
// smoltcp is the DEFAULT x86 net stack; this module compiles by default and is dropped only under
// `UNAOS_NOSMOLNET=1` (the opt-out to the hand-rolled stack). The hand-rolled `net` engines stay
// compiled and live regardless and still own `nc` / `nc -u` / `curl`, the TCP echo listener, and
// driver DHCP.
//
// Design (see unaos/docs/dev/OS/08_NET/networking.md):
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

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicUsize, Ordering};

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::Device; // the phy::Device impl itself lives in the shared `crate::net_phy` adapter
use smoltcp::socket::{dhcpv4, icmp, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpEndpoint,
    IpListenEndpoint, Ipv4Address, Ipv4Cidr,
};
use crate::sync::Mutex as SpinMutex;

use crate::drivers::e1000::{self, PingOutcome};
use crate::net_phy::{RawNic, RxObserver, SmoltcpPhy};

/// slirp's virtual gateway (the default route + the witness's ICMP target). Mirrors `e1000.rs`.
const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];
/// ICMP identifier stamped on every echo request we originate. ASCII "UN" (matches the hand-rolled
/// stack's `PING_IDENT`), so a shared responder can't confuse the two.
const PING_IDENT: u16 = 0x554E;
/// Payload carried in the echo requests we originate.
const PING_PAYLOAD: &[u8] = b"unaos-ping";
/// ICMP socket ring capacities (packets / payload bytes). Our echoes are ~18 bytes; 512 is ample.
const ICMP_META: usize = 8;
const ICMP_PAYLOAD: usize = 512;

// --- the Device adapter over the e1000e rings (the shared `crate::net_phy` adapter) ---

// The `phy::Device` / `RxToken` / `TxToken` boilerplate now lives ONCE in `crate::net_phy::SmoltcpPhy`
// (shared with the aarch64 net drivers). x86 supplies the two pieces that were e1000-specific: the
// `RawNic` seam over the e1000e ring accessors, and an ARP-snooping `RxObserver` that reproduces the old
// `E1000Phy::receive`'s per-frame ARP snoop. `SmolPhy` is the concrete phy type this module binds — the
// shared adapter parameterized over both. ZERO behavior change from the pre-share `E1000Phy`.

/// The e1000e implementation of the shared [`RawNic`] seam: the ring accessors `crate::net_phy` moves L2
/// frames through. Each briefly locks `NET_DEVICE` (the `raw_rx`/`raw_tx` discipline — never held across a
/// smoltcp poll).
struct E1000Nic;
impl RawNic for E1000Nic {
    fn rx_frame_raw(out: &mut [u8]) -> Option<usize> {
        e1000::raw_rx(out)
    }
    fn transmit(frame: &[u8]) {
        e1000::raw_tx(frame)
    }
    fn mac() -> Option<[u8; 6]> {
        e1000::hw_addr().map(|(mac, _, _)| mac)
    }
    /// NETCLOCK M3: the USB dongle's TX ring is bounded and drained asynchronously; full = back-pressure.
    fn tx_ready() -> bool {
        #[cfg(feature = "usbnet")]
        if !e1000::nic_present() {
            return crate::drivers::xhci::usbnet::tx_room();
        }
        true
    }
}

/// The x86 RX observer: snoop an inbound ARP reply for `target`, recording the sender's MAC. smoltcp
/// hides the resolved neighbor MAC, so the `arp`/`ping` shell commands recover it by watching the wire —
/// this observer runs on every received frame (exactly where the pre-share `E1000Phy::receive` snooped).
/// The persistent stack builds it with `target = [0; 4]` (matches no real peer — snoop stays inert), so
/// only the blocking `pump` (which sets a real target and reads back `obs.snoop`) surfaces a MAC.
struct ArpSnoop {
    /// The IP whose ARP reply we want to surface as a MAC (`arp`/`ping` peer).
    target: [u8; 4],
    /// The snooped target MAC, once an ARP reply for `target` is seen on the wire.
    snoop: Option<[u8; 6]>,
}

impl ArpSnoop {
    fn new(target: [u8; 4]) -> Self {
        ArpSnoop { target, snoop: None }
    }
}

impl RxObserver for ArpSnoop {
    fn observe(&mut self, frame: &[u8]) {
        snoop_arp(frame, self.target, &mut self.snoop);
    }
}

/// The concrete phy this module binds: the shared adapter over the e1000e `RawNic` + the ARP-snoop observer.
type SmolPhy = SmoltcpPhy<E1000Nic, ArpSnoop>;

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
    let mut dev = SmolPhy::with_observer(ArpSnoop::new(target));

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
    // NETCLOCK M2: real clock, polled when this interface's `poll_delay` runs out or a frame waits; capped by the wall.
    let deadline = now_ms() + (PING_BASE_MS + PING_PER_MS * count as i64).min(PING_MAX_MS);
    let mut next_poll: i64 = 0;

    loop {
        let now = now_ms();
        if now >= deadline {
            break;
        }
        if now < next_poll && !rx_ready(true) {
            core::hint::spin_loop();
            continue;
        }
        iface.poll(Instant::from_millis(now), &mut dev, &mut sockets);
        crate::net_tick::note_poll();
        next_poll = now + match iface.poll_delay(Instant::from_millis(now), &sockets) {
            Some(d) => (d.total_millis() as i64).max(MIN_POLL_MS),
            None => IDLE_POLL_MS,
        };

        if stop_on_arp && dev.obs.snoop.is_some() {
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
                next_poll = 0; // queued: egress on the next pass
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

    PumpResult { mac: dev.obs.snoop, sent, received }
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

/// A one-line summary of the smolnet interface for `ifconfig` (knob-on).
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
// singleton behind `STACK` (a `crate::sync::Mutex<Option<SmolStack>>`, the `NET_DEVICE`
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

/// Number of concurrent sockets the persistent set holds (process-scoped caps). A slot
/// backs EITHER a UDP or a TCP socket (never both at once — SOCK-3 shares the id space so
/// the handle value word's socket-id maps to one registry, one generation counter).
const NSOCK: usize = 4;
/// Per-socket packet-buffer capacities (datagrams / payload bytes). A DNS reply is
/// well under 512 B; 1 KiB payload + 8 packets is ample headroom.
const UDP_PKTS: usize = 8;
const UDP_BUF: usize = 1024;
/// Largest datagram payload a `sys_sendto`/`sys_recvfrom` moves (bounds the user copy).
pub const UDP_MAX_PAYLOAD: usize = UDP_BUF;
/// SOCK-3: per-TCP-socket stream ring-buffer capacity (rx / tx), all BSS. 2 KiB each is a
/// modest window that comfortably carries the witness exchange and a small request/response;
/// the id space is shared with UDP so only a slot in one role at a time consumes a role's buffers.
const TCP_BUF: usize = 2048;
/// Largest stream chunk a single `sys_send`/`sys_recv` moves (bounds the user copy per call —
/// a stream is resumable, so ring 3 loops for more).
pub const TCP_MAX_CHUNK: usize = TCP_BUF;
/// NETCLOCK (B335) M2: every pump is bounded by the WALL (`now_ms`), never by an iteration count — an
/// iteration costs a controller pass on the USB dongle, so a count bounded nothing (boot 20: >=1M polls/s).
/// `recv`/`connect` wait up to the NETHANG cap for ARP -> egress -> reply; `send` only kicks one poll (the
/// frame leaves on the next due poll); `accept` is ONE gated poll (ring 3 and the SOCK-6 witness re-drive it).
const RECV_MS: i64 = 2_000;
const CONNECT_MS: i64 = 2_000;
const ACCEPT_MS: i64 = 0;
/// On an e1000 the hand-rolled stack's `nic.poll()` drains the SAME RX ring at the top of every
/// `service_net` pass (the module header's two-drains race), so a zero-length accept would almost never see
/// the injector's SYN; the e1000 listener keeps a short window. Polls inside it are still gated.
const ACCEPT_E1000_MS: i64 = 20;
const SEND_MS: i64 = 0;
/// SOCK-5 / NETCLOCK: one DHCP try waits this long for a lease on the main loop; a lease that lands later
/// (smoltcp retransmits DISCOVER on its own 10 s timer) is applied by `service_poll`.
const DHCP_WAIT_MS: i64 = 2_000;
/// NETCLOCK: the throwaway ICMP pump's cap (`ping`/`arp`/SOCK-1): base + per echo, at most 5 s.
const PING_BASE_MS: i64 = 1_000;
const PING_PER_MS: i64 = 500;
const PING_MAX_MS: i64 = 5_000;

/// SOCK-5: one-shot latch — the DHCP acquisition (and its witness line) runs exactly once, from the
/// first `init()` call (review fix: never from a lazy `ensure_stack` first-touch).
static DHCP_ATTEMPTED: AtomicBool = AtomicBool::new(false);

// --- static socket-set + per-socket packet-buffer storage (all BSS, `&'static mut`) ---
// SOCK-5: NSOCK ring-3 socket slots + ONE reserved slot for the kernel-internal DHCP client
// socket (which never appears in `reg`, so `stack_open*` still sees exactly NSOCK free slots).
static mut SOCK_SET_STORAGE: [SocketStorage<'static>; NSOCK + 1] =
    [SocketStorage::EMPTY; NSOCK + 1];
static mut UDP_RX_META: [[udp::PacketMetadata; UDP_PKTS]; NSOCK] =
    [[udp::PacketMetadata::EMPTY; UDP_PKTS]; NSOCK];
static mut UDP_RX_DATA: [[u8; UDP_BUF]; NSOCK] = [[0u8; UDP_BUF]; NSOCK];
static mut UDP_TX_META: [[udp::PacketMetadata; UDP_PKTS]; NSOCK] =
    [[udp::PacketMetadata::EMPTY; UDP_PKTS]; NSOCK];
static mut UDP_TX_DATA: [[u8; UDP_BUF]; NSOCK] = [[0u8; UDP_BUF]; NSOCK];
/// SOCK-3: per-slot TCP stream ring buffers (BSS, borrowed `&'static mut` exactly once when a
/// TCP socket is built into free slot `sid`, released back when it is removed).
static mut TCP_RX_DATA: [[u8; TCP_BUF]; NSOCK] = [[0u8; TCP_BUF]; NSOCK];
static mut TCP_TX_DATA: [[u8; TCP_BUF]; NSOCK] = [[0u8; TCP_BUF]; NSOCK];

/// SOCK-3: per-slot GENERATION counter — the recycled-slot fence carried into the socket handle's
/// value word `(gen << 32) | (sid + 1)` (the U11x file-id discipline). Bumped every time a slot is
/// freed (`stack_close` / `free_row_sockets`), so a stale handle carrying an OLD gen fails
/// `socket_desc_validate` after the slot is first-fit-reused by a different socket — no rebind. This
/// is the SOCK-2-review REQUIRED fold: it makes a transferable socket (a future `SYS_CAP`/`SYS_XFER`
/// arc) safe by construction, and closes the UAF the moment a socket outlives its registry slot.
static SOCK_GEN: [AtomicU32; NSOCK] = [const { AtomicU32::new(0) }; NSOCK];

/// NETCLOCK (B335) M2: the stack's clock — REAL milliseconds. It was a counter bumped once per poll
/// ("iteration-driven, clock-free"), so at ~1M polls/s on the USB dongle smoltcp's ARP and DHCP timers ran
/// ~1000x the wall and the link carried a DISCOVER/ARP storm (boot 20: tx=58380, 269/s, nothing received).
/// `clock::uptime_ms` is the TSC-backed monotonic (it advances with IF clear, unlike the APIC tick); the
/// raw TSC over the `hw_wait_budget` rate stands in before calibration. Clamped never to step back.
/// ARMNET (B346): the body moved to `clock::stack_ms`, the one stack clock both arches share.
fn now_ms() -> i64 {
    crate::clock::stack_ms()
}
/// NETCLOCK M2: when smoltcp next asked to be polled (`now_ms` scale; 0 = now). Written by `poll_now`
/// from `Interface::poll_delay`; `kick` brings it forward after a socket enqueue.
static NEXT_POLL_MS: AtomicI64 = AtomicI64::new(0);
/// Nothing scheduled (`poll_delay` = None): look again once a second. Never closer than 1 ms (a TX ring
/// in back-pressure keeps `poll_delay` at 0 until the controller drains it).
const IDLE_POLL_MS: i64 = 1_000;
const MIN_POLL_MS: i64 = 1;

/// Which transport a registry slot backs. A UDP handle handed to a stream syscall (or vice versa)
/// is rejected on this tag BEFORE any `get_mut::<T>` (smoltcp's typed accessor PANICS on a mismatch).
/// SOCK-7: a `Tcp` slot carries the INDEX of the static stream-buffer set its socket was built from —
/// decoupled from the reg-slot index so an accepted connection can be PEELED into a fresh reg slot
/// while keeping the buffers its established smoltcp socket already owns (the persistent-listener seam).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SockKind {
    Udp,
    /// TCP, holding the `TCP_RX_DATA`/`TCP_TX_DATA` buffer-set index the socket was built from.
    Tcp(usize),
}

/// The persistent stack singleton. Its fields (incl. the 3 KiB device RX/TX scratch)
/// live in BSS via this static, so nothing large sits on the caller's stack.
struct SmolStack {
    iface: Interface,
    sockets: SocketSet<'static>,
    dev: SmolPhy,
    /// socket-id (index) → (smoltcp handle, owning HANDLES row, transport kind). `None` = free slot.
    reg: [Option<(SocketHandle, usize, SockKind)>; NSOCK],
    /// SOCK-7: which TCP stream-buffer sets are in use. Decoupled from the reg-slot index so a
    /// re-armed listener can take a FRESH buffer set while the peeled connection keeps the set its
    /// established socket already owns. `true` = the `TCP_RX_DATA`/`TCP_TX_DATA[i]` set is borrowed.
    tcp_buf_used: [bool; NSOCK],
    /// SOCK-7: the port each reg slot listens on (recorded by `stack_listen`), so `stack_accept`'s
    /// re-arm knows which port to re-listen on after peeling an accepted connection. `0` = not a listener.
    listen_port: [u16; NSOCK],
    /// SOCK-5: the kernel-internal DHCPv4 client socket (in the reserved storage slot, NOT in `reg`).
    /// `dhcp_acquire` (from `init()`, one-shot, chunked) drives it to a lease that replaces the
    /// build-time static config.
    dhcp: Option<SocketHandle>,
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
    let mut dev = SmolPhy::with_observer(ArpSnoop::new([0; 4]));
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = 0x5343_4B32; // "SCK2"
    let iface = Interface::new(config, &mut dev, Instant::from_millis(0));
    // SAFETY: the storage statics are borrowed `&'static mut` EXACTLY ONCE, here, under
    // the `STACK` lock with `guard` proven `None` — no aliasing. `SocketSet::new` retains
    // the borrow for the singleton's life; per-socket buffers are borrowed disjointly in
    // `stack_open` (a free `reg` slot ⇒ its buffer set is unborrowed).
    let storage: &'static mut [SocketStorage<'static>] =
        unsafe { &mut *core::ptr::addr_of_mut!(SOCK_SET_STORAGE) };
    let mut sockets = SocketSet::new(storage);
    // The DHCP socket takes the reserved (NSOCK+1-th) storage slot and is NOT recorded in `reg`, so
    // it never counts against ring-3 socket allocation and no `stack_open*`/`free_row_sockets` touches it.
    let dhcp_handle = sockets.add(dhcpv4::Socket::new());
    let mut stack = SmolStack {
        iface,
        sockets,
        dev,
        reg: [None; NSOCK],
        tcp_buf_used: [false; NSOCK],
        listen_port: [0; NSOCK],
        dhcp: Some(dhcp_handle),
    };
    // The static lease + slirp gateway are applied AT BUILD (review fix): any first-touch —
    // including a lazy ring-3 `sys_socket` on a boot where no launcher pre-built the stack — gets a
    // configured, working interface with NO pump under the lock and nothing on the syscall stack.
    // The DHCP acquisition that REPLACES this config runs only from `init()` (the large-stack boot
    // path), chunked with lock releases — see `dhcp_acquire`.
    apply_ipv4_config(
        &mut stack,
        Ipv4Cidr::new(Ipv4Address::new(our_ip[0], our_ip[1], our_ip[2], our_ip[3]), 24),
        Ipv4Address::new(GATEWAY_IP[0], GATEWAY_IP[1], GATEWAY_IP[2], GATEWAY_IP[3]),
    );
    *guard = Some(stack);
    true
}

/// Apply an IPv4 address + default route to the persistent interface, replacing any prior config
/// (the one config surface — `ensure_stack`'s static build config and `dhcp_acquire`'s lease both
/// route through here).
fn apply_ipv4_config(stack: &mut SmolStack, cidr: Ipv4Cidr, gw: Ipv4Address) {
    stack.iface.update_ip_addrs(|addrs| {
        addrs.clear();
        let _ = addrs.push(IpCidr::Ipv4(cidr));
    });
    let _ = stack.iface.routes_mut().remove_default_ipv4_route();
    let _ = stack.iface.routes_mut().add_default_ipv4_route(gw);
    // SNTP-X86: record the live default gateway so the SNTP client can target it (a DHCP lease's router
    // replaces the static slirp gateway here). Packed big-endian into an atomic (0 = never configured).
    let o = gw.octets();
    CURRENT_GW.store(u32::from_be_bytes([o[0], o[1], o[2], o[3]]), Ordering::Relaxed);
}

/// SNTP-X86: the live default gateway (big-endian octets), updated by every `apply_ipv4_config`. The SNTP
/// client targets it as the time source (slirp's usernet gateway 10.0.2.2 answers as a router; a DHCP
/// lease's router option replaces it). `0` = never configured — the client falls back to `GATEWAY_IP`.
static CURRENT_GW: AtomicU32 = AtomicU32::new(0);
/// USBNET4 M3: true only once a DHCP lease was applied (the static slirp config does not count).
static LEASED: AtomicBool = AtomicBool::new(false);

/// The current SNTP time-source target: the live default gateway, or the static slirp gateway before any
/// config. When DNS is available (SOCK-8), the SNTP client prefers a resolved `pool.ntp.org`; this gateway
/// is the fallback SNTP responder — on real hardware behind a router that runs NTP, or a LAN time server.
fn sntp_target() -> [u8; 4] {
    match CURRENT_GW.load(Ordering::Relaxed) {
        0 => GATEWAY_IP,
        v => v.to_be_bytes(),
    }
}

/// SOCK-8: the DHCP-provided DNS server (big-endian octets), recorded by `dhcp_acquire` from the lease's
/// `dhcpv4::Config::dns_servers` (first entry). `0` = no lease / the lease offered no DNS option — the
/// resolver then falls back to querying the gateway. (slirp's DHCP offers 10.0.2.3, its built-in forwarder.)
static CURRENT_DNS: AtomicU32 = AtomicU32::new(0);

/// SOCK-8: the resolver's nameserver target — the DHCP-provided DNS server if the boot lease carried one,
/// else the live default gateway (`sntp_target`'s gateway). On slirp this resolves to 10.0.2.3 (the lease's
/// DNS option), which forwards to the host resolver; on real hardware it is the LAN/ISP nameserver.
fn dns_server() -> [u8; 4] {
    match CURRENT_DNS.load(Ordering::Relaxed) {
        0 => sntp_target(),
        v => v.to_be_bytes(),
    }
}

/// SOCK-5 (shaped by the review fix): run the interface's DHCPv4 client to a lease and apply it —
/// **chunked**, releasing the `STACK` lock every `TCP_CHUNK` poll iterations (the same SOCK-2-review
/// discipline the TCP connect pump follows) so a concurrent socket syscall on another CPU never spins
/// on the lock for the whole acquisition, and never holding it for more than one chunk on this
/// IF-masked CPU. One-shot (`DHCP_ATTEMPTED`) and called only from `init()` — the large-stack boot
/// path; a lazy ring-3-first-touch `ensure_stack` never runs this (it applies the static config at
/// build), so the acquisition can never land on a 16 KiB syscall stack. On a lease the static config
/// is REPLACED (address + router, `apply_ipv4_config`); on a silent server (budget exhausted) the
/// static fallback simply stands. Emits the SOCK-5 witness line exactly once either way.
fn dhcp_acquire() {
    if DHCP_ATTEMPTED.swap(true, Ordering::AcqRel) {
        return;
    }
    // NETCLOCK: a wall-bounded wait (STACK released between polls); the DISCOVER goes out on the first due
    // poll and is retransmitted on smoltcp's own timer in REAL seconds, not once per 10k loop iterations.
    DHCP_RUNNING.store(true, Ordering::Release);
    kick();
    let got = pump_until(DHCP_WAIT_MS, dhcp_event);
    DHCP_RUNNING.store(false, Ordering::Release);

    let mut g = STACK.lock();
    let Some(stack) = g.as_mut() else { return };
    if let Some((cidr, router, dns_srv)) = got {
        dhcp_apply(stack, cidr, router, dns_srv);
    } else {
        // The build-time static config stands untouched; report it honestly.
        let (addr, plen) = match stack.iface.ip_addrs().first() {
            Some(IpCidr::Ipv4(c)) => (c.address().octets(), c.prefix_len()),
            _ => ([0, 0, 0, 0], 0),
        };
        serial_println!(
            ":: SOCK-5: smoltcp dhcpv4 no offer — static fallback stands {}.{}.{}.{}/{} gw {} — witness INCOMPLETE ::",
            addr[0], addr[1], addr[2], addr[3], plen,
            e1000::fmt_ip(&GATEWAY_IP)
        );
    }
}

/// SOCK-5: apply a lease (address + router replace the static config; the DNS server is recorded) and
/// print the lease witness. From `dhcp_acquire`, or from `service_poll` when the lease lands later.
fn dhcp_apply(stack: &mut SmolStack, cidr: Ipv4Cidr, router: Option<Ipv4Address>, dns_srv: Option<Ipv4Address>) {
    let gw = router.unwrap_or(Ipv4Address::new(
        GATEWAY_IP[0],
        GATEWAY_IP[1],
        GATEWAY_IP[2],
        GATEWAY_IP[3],
    ));
    apply_ipv4_config(stack, cidr, gw);
    LEASED.store(true, Ordering::Relaxed);
    // SOCK-8: record the leased DNS server (if any) so the resolver targets the real nameserver.
    if let Some(d) = dns_srv {
        let o = d.octets();
        CURRENT_DNS.store(u32::from_be_bytes([o[0], o[1], o[2], o[3]]), Ordering::Relaxed);
    }
    let addr = cidr.address().octets();
    serial_println!(
        ":: SOCK-5: smoltcp dhcpv4 lease {}.{}.{}.{}/{} gw {} — witness OK ::",
        addr[0], addr[1], addr[2], addr[3], cidr.prefix_len(),
        e1000::fmt_ip(&gw.octets())
    );
}

/// NETCLOCK M2: a socket just queued data or opened a connection — the next gated poll is due now.
fn kick() {
    NEXT_POLL_MS.store(0, Ordering::Relaxed);
}

/// NETCLOCK M2: is a received frame waiting? e1000: the next RX descriptor's DD bit. USB: the RX ring, and
/// with `drive` (a pump, where the main loop's controller pass is not running) one paced controller pass first.
fn rx_ready(drive: bool) -> bool {
    if e1000::rx_ready() {
        return true;
    }
    #[cfg(feature = "usbnet")]
    if crate::drivers::xhci::usbnet::rx_ready(drive) {
        return true;
    }
    let _ = drive;
    false
}

/// NETCLOCK M2: the poll gate — smoltcp's own `poll_delay` has run out, or a frame arrived.
fn poll_due(drive: bool) -> bool {
    now_ms() >= NEXT_POLL_MS.load(Ordering::Relaxed) || rx_ready(drive)
}

/// NETCLOCK M2: THE poll of the persistent interface: at the real clock, counted, and the next one
/// scheduled from smoltcp's `poll_delay`. Split-borrows the `SmolStack` fields so `iface.poll` gets
/// `&mut dev` + `&mut sockets` disjointly. Callers hold `STACK` and decided the poll is due (or kicked).
fn poll_now(stack: &mut SmolStack) {
    let SmolStack { iface, sockets, dev, .. } = stack;
    let now = now_ms();
    iface.poll(Instant::from_millis(now), dev, sockets);
    crate::net_tick::note_poll();
    let after = now_ms();
    let delay = match iface.poll_delay(Instant::from_millis(after), sockets) {
        Some(d) => (d.total_millis() as i64).max(MIN_POLL_MS),
        None => IDLE_POLL_MS,
    };
    NEXT_POLL_MS.store(after + delay, Ordering::Relaxed);
}

/// NETCLOCK M2/M4: drive the persistent stack until `check` answers or `budget_ms` of wall time passes.
/// `STACK` is held for ONE gated poll plus the check and RELEASED between iterations (M4: every TCP and UDP
/// pump, not only `stack_recvfrom`), so the main loop's `net_tick` and another core's socket syscall get it
/// between any two polls. `check` runs on the first pass even when no poll is due (a reply may already be
/// buffered) and after every poll. Between polls the caller spins (`spin_loop`) — the TSC clock advances
/// with IF clear, so the deadline holds in a masked syscall. `None` = budget spent (or no stack).
fn pump_until<T>(budget_ms: i64, mut check: impl FnMut(&mut SmolStack) -> Option<T>) -> Option<T> {
    let deadline = now_ms() + budget_ms;
    let mut first = true;
    loop {
        let due = poll_due(true);
        if first || due {
            let mut g = STACK.lock();
            let stack = g.as_mut()?;
            if due {
                poll_now(stack);
            }
            if let Some(v) = check(stack) {
                return Some(v);
            }
        }
        first = false;
        if now_ms() >= deadline {
            if budget_ms > ACCEPT_E1000_MS {
                PUMP_CAPPED.fetch_add(1, Ordering::Relaxed); // a real wait ran out (not the per-pass accept window)
            }
            return None;
        }
        core::hint::spin_loop();
    }
}

/// NETCLOCK M2: the idle service, once per main-loop pass (`e1000::service_net`). One poll when smoltcp
/// asked for one or a frame is waiting — never per pass — and a DHCP lease that lands after `dhcp_acquire`
/// gave up is applied here. Never spins on `STACK` (another core's syscall may hold it: next pass).
pub fn service_poll() {
    if !poll_due(false) {
        return;
    }
    let Some(mut g) = STACK.try_lock() else { return };
    let Some(stack) = g.as_mut() else { return };
    poll_now(stack);
    if LEASED.load(Ordering::Relaxed) || DHCP_RUNNING.load(Ordering::Relaxed) || !DHCP_ATTEMPTED.load(Ordering::Relaxed) {
        return;
    }
    if let Some((cidr, router, dns)) = dhcp_event(stack) {
        dhcp_apply(stack, cidr, router, dns);
    }
}

/// NETCLOCK: a `dhcp_acquire` is pumping (it owns the DHCP socket's events until it returns).
static DHCP_RUNNING: AtomicBool = AtomicBool::new(false);

/// The DHCP socket's lease event, if one is pending: (address, router, first DNS server).
fn dhcp_event(stack: &mut SmolStack) -> Option<(Ipv4Cidr, Option<Ipv4Address>, Option<Ipv4Address>)> {
    let h = stack.dhcp?;
    match stack.sockets.get_mut::<dhcpv4::Socket>(h).poll() {
        Some(dhcpv4::Event::Configured(cfg)) => Some((cfg.address, cfg.router, cfg.dns_servers.first().copied())),
        Some(dhcpv4::Event::Deconfigured) | None => None,
    }
}

/// Build the persistent stack now (idempotent) on the CALLER's stack — call this from a large-stack,
/// shallow-chain context (the BSP main loop or a launcher task) BEFORE any ring-3 `sys_socket`, so the
/// one-time ~4 KiB construction transient never lands on a ring-3 task's syscall stack. `false` if no NIC.
pub fn init() -> bool {
    let ok = {
        let mut g = STACK.lock();
        ensure_stack(&mut g)
    };
    if ok {
        // SOCK-5 (review fix): the DHCP acquisition runs HERE — the large-stack boot path — one-shot,
        // with the STACK lock released between bounded chunks. Never from ensure_stack.
        dhcp_acquire();
    }
    ok
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
    stack.reg[sid] = Some((handle, owner, SockKind::Udp));
    Some(sid)
}

/// Bind socket `sid` to a local UDP `port`. `Ok(())` or `Err(())` (already bound / port 0
/// / unknown sid). Idempotent-unfriendly by smoltcp design (rebinding an open socket errors).
pub fn stack_bind(sid: usize, port: u16) -> Result<(), ()> {
    let mut g = STACK.lock();
    let stack = g.as_mut().ok_or(())?;
    let (handle, _, kind) = *stack.reg.get(sid).and_then(|s| s.as_ref()).ok_or(())?;
    if kind != SockKind::Udp {
        return Err(()); // a TCP handle routed to sys_bind — reject before the typed accessor panics
    }
    stack.sockets.get_mut::<udp::Socket>(handle).bind(port).map_err(|_| ())
}

/// Queue `payload` to `ip:port` on socket `sid` and pump a short bounded loop to kick ARP
/// + egress. `Ok(len)` once queued (best-effort egress), or `Err(())` (socket can't send /
/// buffer full / unbound / unknown sid) → the syscall maps that to `-EAGAIN`.
pub fn stack_sendto(sid: usize, ip: [u8; 4], port: u16, payload: &[u8]) -> Result<usize, ()> {
    #[cfg(feature = "usbdebug")]
    serial_println!("[net] poll enter op=sendto sid={} len={} masked={}", sid, payload.len(), !x86_64::instructions::interrupts::are_enabled() as u8);
    let r = stack_sendto_inner(sid, ip, port, payload);
    #[cfg(feature = "usbdebug")]
    serial_println!("[net] poll exit op=sendto sid={} ok={}", sid, r.is_ok() as u8);
    r
}
fn stack_sendto_inner(sid: usize, ip: [u8; 4], port: u16, payload: &[u8]) -> Result<usize, ()> {
    let mut g = STACK.lock();
    let stack = g.as_mut().ok_or(())?;
    let (handle, _, kind) = *stack.reg.get(sid).and_then(|s| s.as_ref()).ok_or(())?;
    if kind != SockKind::Udp {
        return Err(()); // a TCP handle routed to sys_sendto — reject before the typed accessor panics
    }
    let ep = IpEndpoint::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), port);
    {
        let sock = stack.sockets.get_mut::<udp::Socket>(handle);
        if !sock.can_send() {
            return Err(());
        }
        sock.send_slice(payload, ep).map_err(|_| ())?;
    }
    // NETCLOCK: the datagram is queued; one poll now egresses it (or starts ARP). The rest rides the
    // gated polls of `stack_recvfrom` / `service_poll` — no 20k-iteration send pump.
    kick();
    poll_now(stack);
    let _ = SEND_MS;
    Ok(payload.len())
}

/// Non-blocking receive on socket `sid`. Pumps a bounded loop (driving ARP → egress →
/// inbound delivery in this ONE IF-masked call), then returns the first datagram
/// `(src_ip, src_port, len)` copied into `out` (truncated to `out.len()`), or `None` if
/// none arrived within the budget → the syscall maps that to `-EAGAIN`. NEVER blocks.
pub fn stack_recvfrom(sid: usize, out: &mut [u8]) -> Option<([u8; 4], u16, usize)> {
    #[cfg(feature = "usbdebug")]
    serial_println!("[net] poll enter op=recvfrom sid={} masked={}", sid, !x86_64::instructions::interrupts::are_enabled() as u8);
    let r = stack_recvfrom_bounded(sid, out);
    #[cfg(feature = "usbdebug")]
    serial_println!("[net] poll exit op=recvfrom sid={} got={}", sid, r.is_some() as u8);
    r
}

/// NETHANG: the receive pump with `STACK` released between chunks (the `TCP_CHUNK` rule
/// `dhcp_acquire` already follows) and a wall-clock deadline beside the iteration budget. A dead link
/// costs the caller at most `hw_wait_budget()` (2 s) and then answers `None` (-> -EAGAIN / ENOENT).
/// The BSP main loop's `net_tick` can take `STACK` between two chunks. Before, one IF-masked syscall
/// held it for all `RECV_PUMP` polls.
fn stack_recvfrom_bounded(sid: usize, out: &mut [u8]) -> Option<([u8; 4], u16, usize)> {
    // NETCLOCK M2/M4: `pump_until` — gated polls, `STACK` released between every two, the wall the bound.
    pump_until(RECV_MS, |stack| {
        // Re-validated every pass: the lock was released, so the slot may have been closed.
        let Some(&Some((handle, _, kind))) = stack.reg.get(sid) else { return Some(None) };
        if kind != SockKind::Udp {
            return Some(None); // a TCP handle routed to sys_recvfrom — reject before the typed accessor panics
        }
        let sock = stack.sockets.get_mut::<udp::Socket>(handle);
        if sock.can_recv() {
            if let Ok((data, meta)) = sock.recv() {
                let n = data.len().min(out.len());
                out[..n].copy_from_slice(&data[..n]);
                let IpAddress::Ipv4(v4) = meta.endpoint.addr;
                return Some(Some((v4.octets(), meta.endpoint.port, n)));
            }
        }
        None
    })
    .flatten()
}

/// NETHANG: how many pumps hit the wall-clock cap rather than their iteration budget.
static PUMP_CAPPED: AtomicU32 = AtomicU32::new(0);
pub fn pump_capped() -> u32 {
    PUMP_CAPPED.load(Ordering::Relaxed)
}
/// NETHANG: is `STACK` free right now? (The fixture's `held_locks` count: a call that returned and
/// left this held is the boot-20 shape.)
pub fn stack_lock_free() -> bool {
    STACK.try_lock().is_some()
}

/// Remove socket `sid` from the persistent set (drops the udp socket, releasing its static
/// buffers for reuse) and free its `reg` slot. No-op on an unknown/free sid.
pub fn stack_close(sid: usize) {
    let mut g = STACK.lock();
    let Some(stack) = g.as_mut() else { return };
    if let Some(Some((handle, _, kind))) = stack.reg.get(sid).copied() {
        stack.sockets.remove(handle);
        stack.reg[sid] = None;
        // SOCK-7: release the socket's TCP stream-buffer set back to the free-list (decoupled from sid).
        if let SockKind::Tcp(buf) = kind {
            free_tcp_buf(stack, buf);
        }
        if sid < NSOCK {
            stack.listen_port[sid] = 0;
        }
        // SOCK-3: bump the slot generation so a stale handle carrying the old (gen, sid) fails
        // `socket_desc_validate` after this slot is first-fit-reused (no rebind — the U11x fence).
        if sid < NSOCK {
            SOCK_GEN[sid].fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// Teardown hook: free every socket OWNED by HANDLES row `row` (the caller exited / was
/// killed). Called from `clear_handle_row` so a dying task leaks no persistent socket and
/// a reused slot inherits none. No-op if the stack was never built.
pub fn free_row_sockets(row: usize) {
    let mut g = STACK.lock();
    let Some(stack) = g.as_mut() else { return };
    for sid in 0..NSOCK {
        if let Some((handle, owner, kind)) = stack.reg[sid] {
            if owner == row {
                stack.sockets.remove(handle);
                stack.reg[sid] = None;
                // SOCK-7: release the TCP stream-buffer set (decoupled from sid).
                if let SockKind::Tcp(buf) = kind {
                    free_tcp_buf(stack, buf);
                }
                stack.listen_port[sid] = 0;
                // SOCK-3: bump the generation on teardown too, so a recycled slot never hands its
                // next tenant a stale-gen socket-id a lingering handle could rebind to.
                SOCK_GEN[sid].fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

// =============================================================================
// SOCK-3 (ROADMAP §1b): TCP CLIENT sockets over the SAME persistent stack.
//
// A TCP socket rides the existing `STACK` singleton + `reg` (kind-tagged `Tcp`), with its own static
// stream ring buffers (`TCP_RX_DATA`/`TCP_TX_DATA`, BSS). The three stream syscalls funnel here:
//
//  * `stack_connect` — the ACTIVE open. NON-BLOCKING with a ring-3 poll model: the FIRST call (state
//    Closed) issues the SYN and pumps a bounded loop chasing the 3-way handshake; if the SYN-ACK lands
//    it returns `Established`, else `InProgress`. A ring-3 caller re-invokes `sys_connect` on
//    `-EINPROGRESS`; the re-call sees state SynSent, SKIPS re-issuing the SYN (idempotent), and pumps
//    more. Slirp's RTT is microseconds, so the handshake almost always completes inside the first call
//    — but the poll model means the IF-masked handler NEVER blocks on a slow/lossy peer.
//  * `stack_send` — enqueue into the tx ring (once `may_send`) + a short bounded egress pump.
//  * `stack_recv` — a bounded poll pump returning the first stream bytes, `WouldBlock` (`-EAGAIN`) if
//    none yet, or `Eof` (a clean `0`) once the peer's FIN is delivered and the rx ring is drained.
//
// EVERY TCP pump releases the `STACK` lock BETWEEN chunks (`tcp_pump_chunked`) — the SOCK-2-review
// REQUIRED fold: another CPU's socket syscall never spins on `STACK.lock()` for a whole ~400k-iter
// pump. A chunk re-acquires the lock and re-validates the reg slot, so a concurrent teardown is seen
// as the connection vanishing (returned as `Refused`/`Eof`), never a use-after-free.
// =============================================================================

/// The current generation of registry slot `sid` — packed into a freshly-minted socket handle's value
/// word `(gen << 32) | (sid + 1)` at `sys_socket` time (the U11x file-id discipline). A mint reads the
/// gen the socket is about to live under (no free can intervene between `stack_open*` and this read on
/// the minting task). `0` for an out-of-range id.
pub fn sock_gen(sid: usize) -> u32 {
    if sid >= NSOCK {
        return 0;
    }
    SOCK_GEN[sid].load(Ordering::Acquire)
}

/// Validate a decoded `(sid, gen)` against the LIVE registry under the `STACK` lock: the slot must be
/// present, OWNED by `row`, and its CURRENT generation must equal the handle's packed `gen`. `true` =
/// a live descriptor this caller owns; `false` = stale (freed+reused since the handle was minted),
/// foreign, or free → the syscall maps that to `-EACCES`. This is the recycled-slot fence the SOCK-2
/// review made REQUIRED before any socket becomes transferable (mirrors `file_desc_validate`'s gen check).
pub fn sock_valid(row: usize, sid: usize, generation: u32) -> bool {
    if sid >= NSOCK {
        return false;
    }
    let g = STACK.lock();
    let Some(stack) = g.as_ref() else { return false };
    match stack.reg[sid] {
        Some((_, owner, _)) => owner == row && SOCK_GEN[sid].load(Ordering::Acquire) == generation,
        None => false,
    }
}

/// SOCK-4: MOVE a socket's registry OWNERSHIP to `new_row` — the descriptor migration a transferred
/// socket cap needs. `sock_valid` is owner-scoped, so a socket handed to another row via `SYS_XFER`
/// would fail its owner CHECK there unless the persistent socket's `reg` owner follows the cap. Called
/// from `sys_recv` when it installs a received `KIND_SOCKET` handle. Under the `STACK` lock: iff slot
/// `sid` is present AND its CURRENT generation equals `generation` (the gen the received handle carries)
/// AND its CURRENT owner is `from_row` (the transfer's SENDER — the depositor must still own the socket
/// at delivery), reassign its owner to `new_row` and return `true`. A gen mismatch (the socket was
/// freed+reused since the transfer was deposited), an owner mismatch (the socket already MOVED away —
/// the sender's residual `CAP_GRANT` handle must not re-migrate a socket it no longer owns out from
/// under the current owner), or a free/absent slot returns `false` — the received handle then stays
/// dead (fails `sock_valid`), never rebinding to a DIFFERENT tenant of the slot. Only the owner field
/// moves; the smoltcp handle + its stream/packet buffers are untouched (a MOVE, not a re-open), so an
/// in-flight connection or bound port survives the hand-off. After the move the GRANTOR's original
/// handle is owner-mismatched (dead), so a socket has exactly ONE owner at any instant — the teardown
/// (`free_row_sockets`) and the gen fence both stay single-owner-correct.
pub fn reassign_owner(sid: usize, generation: u32, from_row: usize, new_row: usize) -> bool {
    if sid >= NSOCK {
        return false;
    }
    let mut g = STACK.lock();
    let Some(stack) = g.as_mut() else { return false };
    match stack.reg[sid] {
        Some((handle, owner, kind))
            if owner == from_row && SOCK_GEN[sid].load(Ordering::Acquire) == generation =>
        {
            stack.reg[sid] = Some((handle, new_row, kind));
            true
        }
        _ => false, // free, absent, stale (freed+reused), or already moved away from the sender
    }
}

/// SOCK-3: rotating ephemeral local port for active opens (49152..=65535, the IANA dynamic range),
/// so back-to-back connects on a reused slot never collide in smoltcp's TIME_WAIT.
static TCP_EPHEMERAL: AtomicU32 = AtomicU32::new(49152);
fn next_ephemeral() -> u16 {
    let p = TCP_EPHEMERAL.fetch_add(1, Ordering::Relaxed);
    (49152 + (p % (65535 - 49152 + 1))) as u16
}

/// The outcome of a bounded `stack_connect` pump.
pub enum ConnectOutcome {
    /// The 3-way handshake completed — the socket is ESTABLISHED (ring 3: return 0).
    Established,
    /// Still SYN-SENT at budget exhaustion — the caller re-drives `sys_connect` (ring 3: `-EINPROGRESS`).
    InProgress,
    /// The peer refused / reset, or the socket/stack is gone / wrong-kind (ring 3: `-ECONNREFUSED`).
    Refused,
}

/// The outcome of a bounded `stack_recv` pump.
pub enum RecvOutcome {
    /// `n` stream bytes were copied into the caller's buffer (ring 3: the byte count).
    Data(usize),
    /// No bytes yet, connection still open (ring 3: `-EAGAIN`).
    WouldBlock,
    /// The peer closed its send half (FIN) and the rx ring is drained, or the connection is gone
    /// (ring 3: a clean `0` — end of stream).
    Eof,
}

/// SOCK-7: the outcome of a bounded `stack_accept` pump on a PERSISTENT listener.
pub enum AcceptOutcome {
    /// A peer completed the 3-way handshake. SOCK-7: the established smoltcp socket is PEELED into a
    /// FRESH reg slot (carried here) and the listener slot is RE-ARMED in place on the same port, so the
    /// listener SURVIVES the accept. Ring 3 mints a `KIND_SOCKET` handle for this fresh connection
    /// socket-id and streams on it (`send`/`sock_recv`); the caller's listener handle stays valid.
    Connected(usize),
    /// No inbound connection yet — still LISTENING at budget exhaustion (ring 3: `-EAGAIN`, re-drive
    /// accept). Also returned when a handshake completed but no reg slot / buffer set is free to peel it
    /// into (NSOCK back-pressure): the established connection stays buffered in the listener socket and
    /// is peeled by a later accept once a slot frees — never lost.
    Pending,
    /// The socket is not armed for listen (never listened / already connected+closed / gone / wrong-kind).
    /// Ring 3: `-EINVAL`.
    NotListening,
}

/// SOCK-7: claim a free TCP stream-buffer set index (decoupled from the reg-slot index). `None` if all
/// `NSOCK` sets are in use. INVARIANT: the count of used buffer sets equals the count of live TCP sockets,
/// which is ≤ the count of used reg slots — so whenever a reg slot is free a buffer set is free too (the
/// peel + re-arm never starves). Under the `STACK` lock (the caller holds it).
fn alloc_tcp_buf(stack: &mut SmolStack) -> Option<usize> {
    let idx = stack.tcp_buf_used.iter().position(|&u| !u)?;
    stack.tcp_buf_used[idx] = true;
    Some(idx)
}

/// SOCK-7: release a TCP stream-buffer set (its owning socket was removed). Under the `STACK` lock.
fn free_tcp_buf(stack: &mut SmolStack, idx: usize) {
    if idx < NSOCK {
        stack.tcp_buf_used[idx] = false;
    }
}

/// SOCK-7: build a TCP socket from stream-buffer set `buf` (BSS, borrowed `&'static mut` EXACTLY once —
/// `buf` is a claimed set, so its two static buffers are not borrowed by any live socket). Mirrors the
/// SOCK-2 `stack_open` discipline: `addr_of_mut!(STATIC[buf])` names the element as a PLACE (no
/// intermediate reference), then `from_raw_parts_mut` re-forms the slice (no autoref through a raw deref).
fn build_tcp_socket(buf: usize) -> tcp::Socket<'static> {
    let (rx_data, tx_data): (&'static mut [u8], &'static mut [u8]) = unsafe {
        (
            core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(TCP_RX_DATA[buf]) as *mut u8,
                TCP_BUF,
            ),
            core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(TCP_TX_DATA[buf]) as *mut u8,
                TCP_BUF,
            ),
        )
    };
    tcp::Socket::new(tcp::SocketBuffer::new(rx_data), tcp::SocketBuffer::new(tx_data))
}

/// Allocate a TCP socket in the persistent set, owned by HANDLES row `owner`. Builds the socket from a
/// free STATIC stream-buffer set (SOCK-7: allocated from the buffer free-list, no longer pinned to the
/// reg-slot index) and records the handle kind-tagged `Tcp(buf)`. Returns the socket-id (the `reg`
/// index) or `None` if all slots / buffer sets are in use / no NIC.
pub fn stack_open_tcp(owner: usize) -> Option<usize> {
    let mut g = STACK.lock();
    if !ensure_stack(&mut g) {
        return None;
    }
    let stack = g.as_mut().unwrap();
    let sid = stack.reg.iter().position(|s| s.is_none())?;
    let buf = alloc_tcp_buf(stack)?;
    let socket = build_tcp_socket(buf);
    let handle = stack.sockets.add(socket);
    stack.reg[sid] = Some((handle, owner, SockKind::Tcp(buf)));
    Some(sid)
}

/// Look up a TCP registry slot: the handle iff `sid` is present AND kind-tagged `Tcp`. `None` = free /
/// wrong-kind (a UDP handle routed to a stream syscall — rejected before smoltcp's typed accessor panics).
fn tcp_handle(stack: &SmolStack, sid: usize) -> Option<SocketHandle> {
    match stack.reg.get(sid).and_then(|s| s.as_ref()) {
        Some((h, _, SockKind::Tcp(_))) => Some(*h),
        _ => None,
    }
}

/// Pump the persistent interface for `budget` iterations, RELEASING the `STACK` lock between
/// `TCP_CHUNK`-sized chunks (the SOCK-2-review fold: no cross-CPU busy-wait on `STACK.lock()` for a
/// full pump). `check` runs under the lock after each chunk with the TCP socket; returning
/// `Some(outcome)` stops early. Re-validates the reg slot each chunk — if the socket vanished
/// (concurrent teardown), returns `on_gone`.
fn tcp_pump_chunked<T>(
    sid: usize,
    budget_ms: i64,
    on_gone: T,
    mut check: impl FnMut(&mut tcp::Socket) -> Option<T>,
) -> Option<T> {
    // NETCLOCK M4: one gated poll per `STACK` hold (was a 4000-poll chunk), the wall the bound.
    let mut gone = Some(on_gone);
    pump_until(budget_ms, |stack| {
        let Some(handle) = tcp_handle(stack, sid) else { return gone.take() };
        check(stack.sockets.get_mut::<tcp::Socket>(handle))
    })
}

/// Active-open TCP socket `sid` to `ip:port`. NON-BLOCKING (ring-3 poll model): issues the SYN if the
/// socket is Closed (idempotent — a re-call while SYN-SENT just pumps), then pumps a bounded loop
/// (lock-released chunks) chasing the handshake. Returns `Established` / `InProgress` / `Refused`.
pub fn stack_connect(sid: usize, ip: [u8; 4], port: u16) -> ConnectOutcome {
    // (1) Issue the SYN under the lock, but ONLY from state Closed (so a poll re-call is a no-op open).
    {
        let mut g = STACK.lock();
        let Some(stack) = g.as_mut() else { return ConnectOutcome::Refused };
        let Some(handle) = tcp_handle(stack, sid) else { return ConnectOutcome::Refused };
        let local = next_ephemeral();
        let SmolStack { iface, sockets, .. } = stack;
        let sock = sockets.get_mut::<tcp::Socket>(handle);
        if !sock.is_open() {
            // state Closed (fresh, or a prior connection that closed) — issue the active open.
            let remote = IpEndpoint::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), port);
            let le = IpListenEndpoint { addr: None, port: local };
            if sock.connect(iface.context(), remote, le).is_err() {
                return ConnectOutcome::Refused;
            }
            kick(); // NETCLOCK: the SYN is queued — poll now
        }
    }
    // (2) Pump chunked (lock released between chunks) chasing ESTABLISHED.
    let out = tcp_pump_chunked(sid, CONNECT_MS, ConnectOutcome::Refused, |sock| {
        if sock.state() == tcp::State::Established {
            Some(ConnectOutcome::Established)
        } else if !sock.is_active() {
            // fell out of SYN-SENT without establishing (RST / refused) — Closed again.
            Some(ConnectOutcome::Refused)
        } else {
            None // still SYN-SENT — keep pumping
        }
    });
    out.unwrap_or(ConnectOutcome::InProgress)
}

/// Stream-send on TCP socket `sid`: enqueue `data` into the tx ring (once the connection may send),
/// then a short bounded egress pump. `Ok(n)` = bytes queued (`n < data.len()` if the ring filled);
/// `Err(true)` = would-block (tx ring full right now — ring 3 retries, `-EAGAIN`); `Err(false)` = not
/// connected / wrong-kind / gone (ring 3 `-ENOTCONN`).
pub fn stack_send(sid: usize, data: &[u8]) -> Result<usize, bool> {
    let queued = {
        let mut g = STACK.lock();
        let stack = g.as_mut().ok_or(false)?;
        let handle = tcp_handle(stack, sid).ok_or(false)?;
        let sock = stack.sockets.get_mut::<tcp::Socket>(handle);
        if !sock.may_send() {
            return Err(false); // not ESTABLISHED (still connecting, or the tx half is closed)
        }
        match sock.send_slice(data) {
            Ok(0) => return Err(true), // tx ring momentarily full — would block
            Ok(n) => n,
            Err(_) => return Err(false), // send half closed
        }
    };
    // Kick egress (lock released between chunks); the socket vanishing mid-pump is harmless here.
    kick(); // NETCLOCK: data queued — one poll egresses it
    let _ = tcp_pump_chunked(sid, SEND_MS, (), |_sock| None);
    Ok(queued)
}

/// Non-blocking stream-recv on TCP socket `sid`: pump a bounded loop (lock-released chunks) driving the
/// connection, then return the first stream bytes copied into `out`, `WouldBlock` if none arrived
/// within the budget, or `Eof` once the peer's FIN is delivered and the rx ring is drained.
pub fn stack_recv(sid: usize, out: &mut [u8]) -> RecvOutcome {
    // A closure that tries a dequeue; `Some(outcome)` stops the pump.
    let try_recv = |sock: &mut tcp::Socket, out: &mut [u8]| -> Option<RecvOutcome> {
        match sock.recv_slice(out) {
            Ok(0) => {
                // Established but no data yet — keep pumping unless the connection is fully gone.
                if !sock.is_open() { Some(RecvOutcome::Eof) } else { None }
            }
            Ok(n) => Some(RecvOutcome::Data(n)),
            Err(tcp::RecvError::Finished) => Some(RecvOutcome::Eof), // clean FIN, all data delivered
            Err(tcp::RecvError::InvalidState) => {
                if !sock.is_open() { Some(RecvOutcome::Eof) } else { None }
            }
        }
    };
    // First, a lock-held immediate check (a reply may already be buffered from a prior pump).
    {
        let mut g = STACK.lock();
        if let Some(stack) = g.as_mut() {
            if let Some(handle) = tcp_handle(stack, sid) {
                if let Some(o) = try_recv(stack.sockets.get_mut::<tcp::Socket>(handle), out) {
                    return o;
                }
            } else {
                return RecvOutcome::Eof; // gone / wrong-kind
            }
        } else {
            return RecvOutcome::Eof;
        }
    }
    // Then pump chunked (lock released between chunks) until data / EOF / budget.
    tcp_pump_chunked(sid, RECV_MS, RecvOutcome::Eof, |sock| try_recv(sock, out))
        .unwrap_or(RecvOutcome::WouldBlock)
}

// =============================================================================
// SOCK-6/7 (ROADMAP §1b): TCP SERVER / LISTEN sockets over the SAME persistent stack.
//
// A ring-3 process turns a TCP socket into a passive server with two syscalls:
//   * `sys_listen(handle, port)` -> `stack_listen`: arm the socket as a LISTENER on a local port
//     (smoltcp `tcp::Socket::listen`). Passive — no pump; the accept pump drives the inbound handshake.
//     SOCK-7 additionally records the port so `stack_accept` can re-arm the listener after an accept.
//   * `sys_accept(handle)` -> `stack_accept`: NON-BLOCKING poll for an inbound connection.
//
// SOCK-7 (PERSISTENT LISTENER). smoltcp has no listen->child model: when a peer completes the handshake
// the LISTENING socket transitions to ESTABLISHED IN PLACE (the listener BECOMES the connection). SOCK-6
// exposed exactly that — single-accept-per-listen: accepting CONSUMED the listener. SOCK-7 makes the
// listener SURVIVE by decoupling the stream buffers from the reg-slot index (the buffer free-list): when
// `stack_accept` finds the listener socket ESTABLISHED it PEELS the connection — the established smoltcp
// socket (which keeps the stream-buffer set it already owns) is MOVED to a FRESH reg slot, and the
// ORIGINAL listener slot is RE-ARMED IN PLACE with a NEW smoltcp socket on a fresh buffer set, listening
// again on the same port. The listener slot keeps its socket-id AND its generation, so the caller's
// listener handle stays valid across unbounded accepts; each accept returns a fresh gen-fenced
// connection socket-id. Bounded by NSOCK: peeling needs a free reg slot + buffer set — the invariant
// "used buffer sets ≤ used reg slots < NSOCK when a slot is free" guarantees a free buffer whenever a
// slot is free, and when NEITHER is free the handshake stays buffered in the listener and is peeled by a
// later accept (Pending / -EAGAIN), never lost. Composes with SOCK-4 — each connection cap can be
// `SYS_XFER`'d to a handler (inetd-style hand-off) while the listener keeps accepting.
//
// Every accept pump releases the `STACK` lock between chunks, the SOCK-2-review fold, so a concurrent
// socket syscall on another CPU never spins on `STACK.lock()` for a whole pump. The PEEL itself happens
// under a single lock hold (it only shuffles reg entries + adds one smoltcp socket — no pump).
// =============================================================================

/// SOCK-6: arm TCP socket `sid` as a passive LISTENER on local `port`. `Ok(())`, or `Err(())` (unknown
/// sid / wrong-kind / port 0 / smoltcp refuses — the socket is already open). No pump: listening is
/// passive descriptor state (the accept pump drives the inbound handshake), so this is IF-masked-safe.
pub fn stack_listen(sid: usize, port: u16) -> Result<(), ()> {
    if port == 0 {
        return Err(());
    }
    let mut g = STACK.lock();
    let stack = g.as_mut().ok_or(())?;
    let handle = tcp_handle(stack, sid).ok_or(())?;
    stack.sockets.get_mut::<tcp::Socket>(handle).listen(port).map_err(|_| ())?;
    // SOCK-7: remember the port so `stack_accept`'s re-arm re-listens on it after peeling a connection.
    if sid < NSOCK {
        stack.listen_port[sid] = port;
    }
    Ok(())
}

/// SOCK-7: non-blocking ACCEPT on PERSISTENT listening socket `sid`. Pumps a bounded loop (lock released
/// between chunks) driving the inbound handshake; when the listener socket reaches ESTABLISHED it PEELS
/// the connection into a fresh reg slot and RE-ARMS the listener in place (see `peel_and_rearm`), then
/// reports `Connected(conn_sid)`. `Pending` if none arrived within the budget, or if a handshake landed
/// but no slot/buffer was free to peel it (the connection stays buffered; a later accept peels it —
/// ring 3 re-drives `sys_accept`). `NotListening` if the socket is not armed for listen / vanished /
/// wrong-kind. NEVER blocks. The listener socket-id + generation are unchanged, so the caller's listener
/// handle survives across unbounded accepts.
pub fn stack_accept(sid: usize) -> AcceptOutcome {
    // NETCLOCK M2: ONE gated poll (the SOCK-6 witness calls this every `service_net` pass and ring 3
    // re-drives `sys_accept`; it was 40,000 polls = 40 smoltcp-seconds per pass, the boot-20 DISCOVER storm).
    let budget = if e1000::nic_present() { ACCEPT_E1000_MS } else { ACCEPT_MS };
    pump_until(budget, |stack| {
        let Some(handle) = tcp_handle(stack, sid) else { return Some(AcceptOutcome::NotListening) };
        match stack.sockets.get_mut::<tcp::Socket>(handle).state() {
            // Still waiting for a SYN, or mid-handshake (SYN received, ACK pending) — keep pumping.
            tcp::State::Listen | tcp::State::SynReceived => None,
            // Never armed / listener closed without connecting — not an accept-able socket.
            tcp::State::Closed => Some(AcceptOutcome::NotListening),
            // ESTABLISHED (or past it) — a peer connected; peel the connection + re-arm the listener.
            _ => Some(peel_and_rearm(stack, sid)),
        }
    })
    .unwrap_or(AcceptOutcome::Pending) // budget spent still LISTENING -> caller re-drives
}

/// SOCK-7: the listener socket at reg slot `lsid` has reached ESTABLISHED — a peer connected. PEEL the
/// established connection into a fresh reg slot and RE-ARM `lsid` as a listener again on the same port, so
/// the listener SURVIVES. Under the `STACK` lock (the caller holds it), no pump. Returns
/// `Connected(conn_sid)` on success, or `Pending` if no free reg slot / buffer set is available to peel
/// into (NSOCK back-pressure): the established connection stays buffered in the listener socket and is
/// peeled by a later accept once a slot frees — never lost, and the listener is NOT consumed.
fn peel_and_rearm(stack: &mut SmolStack, lsid: usize) -> AcceptOutcome {
    // The listener entry: its established smoltcp handle + the buffer set that socket owns + the port.
    let Some((estab_handle, owner, SockKind::Tcp(estab_buf))) = stack.reg[lsid] else {
        return AcceptOutcome::NotListening;
    };
    let port = if lsid < NSOCK { stack.listen_port[lsid] } else { 0 };
    if port == 0 {
        // Established but we never recorded a listen port (shouldn't happen for a real listener) — treat
        // as not-a-listener rather than re-arm on port 0.
        return AcceptOutcome::NotListening;
    }
    // Need a fresh reg slot for the peeled connection...
    let Some(conn_sid) = stack.reg.iter().position(|s| s.is_none()) else {
        return AcceptOutcome::Pending; // back-pressure: buffered, peeled later
    };
    // ...and a fresh buffer set for the re-armed listener. (Invariant: a free slot ⇒ a free buffer.)
    let Some(new_buf) = alloc_tcp_buf(stack) else {
        return AcceptOutcome::Pending;
    };
    // Build + arm the replacement listener on the SAME port, into a fresh smoltcp socket.
    let new_listener = build_tcp_socket(new_buf);
    // SOCK-7 review NOTE-3 (zeolite rider): `SOCK_SET_STORAGE` is an EXACT fit — `NSOCK` ring-3
    // slots + 1 reserved DHCP socket — and `SocketSet::add` PANICS when full. This `add` is the
    // occupancy PEAK: the re-armed listener is inserted while the established socket is still in
    // the set (about to be repurposed as the peeled connection), so nothing is removed first. The
    // peel guard above (a free reg slot ⇒ a free buffer set ⇒ room) keeps it within capacity today;
    // assert the invariant so a future `NSOCK`/reserved-socket change trips HERE in a debug build
    // rather than panicking smoltcp in the field.
    debug_assert!(
        stack.sockets.iter().count() < NSOCK + 1,
        "zeolite/SOCK-7: SocketSet at capacity before peel_and_rearm add — occupancy invariant broken"
    );
    let new_handle = stack.sockets.add(new_listener);
    if stack.sockets.get_mut::<tcp::Socket>(new_handle).listen(port).is_err() {
        // Could not re-arm — roll back the half-built listener so nothing leaks, and report the
        // connection is still buffered (Pending): the OLD listener slot is untouched, so the next accept
        // sees ESTABLISHED again and retries the peel.
        stack.sockets.remove(new_handle);
        free_tcp_buf(stack, new_buf);
        return AcceptOutcome::Pending;
    }
    // Commit: the established socket becomes the CONNECTION in the fresh slot (keeps its buffer set); the
    // listener slot is RE-ARMED in place with the new listening socket. The listener slot keeps its
    // socket-id AND generation (no bump) so the caller's listener handle stays valid; the connection slot
    // carries whatever generation it currently holds (the mint reads it), like any fresh open.
    stack.reg[conn_sid] = Some((estab_handle, owner, SockKind::Tcp(estab_buf)));
    stack.reg[lsid] = Some((new_handle, owner, SockKind::Tcp(new_buf)));
    // The connection slot is not a listener; the listener slot keeps listening on the same port.
    if conn_sid < NSOCK {
        stack.listen_port[conn_sid] = 0;
    }
    AcceptOutcome::Connected(conn_sid)
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

// --- the boot TCP round-trip witness (SOCK-3 M1), driven one-shot from service_net knob-on ---

/// True once the SOCK-3 witness has run (one-shot).
static WITNESS3_DONE: AtomicBool = AtomicBool::new(false);
/// service_net call counter — lets the link/NIC settle before the witness fires.
static WITNESS3_TICKS: AtomicU32 = AtomicU32::new(0);
/// Warm up past SOCK-2's witness so the boot self-test + the two UDP/ICMP witnesses settle first.
const WITNESS3_WARMUP: u32 = 40;

/// slirp forwards a guest TCP connection to its built-in DNS resolver (10.0.2.3:53) out to the host's
/// resolver over TCP, so a DNS-over-TCP query is a genuine hermetic 3-way-handshake + stream round-trip
/// under the DEFAULT `./arroyo test` slirp backend — the TCP analogue of SOCK-2's UDP-DNS medium, no
/// injector and no netdev change. DNS-over-TCP (RFC 7766) prefixes the 24-byte query with its 2-byte
/// big-endian length. The reply is `[len BE][answer]`, ≥ 2 bytes — either proves the stream round-trip.
const DNS_TCP_QUERY: [u8; 26] = {
    let mut q = [0u8; 26];
    q[0] = 0x00;
    q[1] = 24; // DNS_QUERY length (big-endian u16)
    let mut i = 0;
    while i < 24 {
        q[2 + i] = DNS_QUERY[i];
        i += 1;
    }
    q
};

/// One-shot kernel-side TCP round-trip witness (M1): open a TCP socket in the persistent set,
/// active-open to slirp's resolver over TCP (poll-looping `connect` until ESTABLISHED), send a
/// DNS-over-TCP query, receive the reply, close, and emit the UNCOUNTED witness line. Proves the
/// persistent interface + `SocketSet` carry a real byte-stream round-trip end to end from the kernel.
/// No-op once done / no NIC. Runs from `service_net` on the BSP main loop, AFTER the NET_DEVICE guard
/// drops (the pump re-locks NET_DEVICE per ring op).
pub fn witness_tick3() {
    if WITNESS3_DONE.load(Ordering::Relaxed) {
        return;
    }
    if WITNESS3_TICKS.fetch_add(1, Ordering::Relaxed) < WITNESS3_WARMUP {
        return;
    }
    if e1000::hw_addr().is_none() {
        return;
    }
    WITNESS3_DONE.store(true, Ordering::Relaxed);

    let (established, nbytes) = tcp_dns_roundtrip();
    serial_println!(
        ":: SOCK-3: smoltcp tcp connect {}:{} {}, {} bytes back — witness {} ::",
        e1000::fmt_ip(&DNS_IP),
        DNS_PORT,
        if established { "established" } else { "REFUSED" },
        nbytes,
        if established && nbytes > 0 { "OK" } else { "INCOMPLETE" }
    );
}

/// Kernel-side one-shot TCP round-trip over the persistent stack: open → connect (poll) → send →
/// recv → close. Returns `(established, bytes_received)`. The `owner` row is kernel ownership
/// (`usize::MAX`), never a live ring-3 slot, so `free_row_sockets` for a real task never touches it;
/// it is closed explicitly here.
fn tcp_dns_roundtrip() -> (bool, usize) {
    let Some(sid) = stack_open_tcp(usize::MAX) else {
        return (false, 0);
    };
    // Poll-loop the non-blocking connect until ESTABLISHED (each call pumps a bounded chunk).
    let mut established = false;
    for _ in 0..8 {
        match stack_connect(sid, DNS_IP, DNS_PORT) {
            ConnectOutcome::Established => {
                established = true;
                break;
            }
            ConnectOutcome::InProgress => continue,
            ConnectOutcome::Refused => break,
        }
    }
    if !established {
        stack_close(sid);
        return (false, 0);
    }
    let _ = stack_send(sid, &DNS_TCP_QUERY);
    let mut buf = [0u8; 64];
    let n = match stack_recv(sid, &mut buf) {
        RecvOutcome::Data(n) => n,
        _ => 0,
    };
    stack_close(sid);
    (true, n)
}

// --- the TCP SERVER witness (SOCK-6/7 M2), driven STATEFUL from service_net knob-on ---
//
// Unlike SOCK-1..3/5 (one-shot, guest-INITIATED round-trips hermetic under slirp), a server witness
// needs a peer to connect INTO the guest — which slirp's NAT will not do. The witness therefore ARMS a
// PERSISTENT listener and stays LISTENING across service_net passes, awaiting `scripts/net-inject.py`'s
// gateway-side active-opens under `UNAOS_NET=socket`. Under the default (hermetic) slirp backend no peer
// ever connects, so it prints the honest SOCK-6 + SOCK-7 `witness PENDING` notes once and keeps listening
// cheaply (light per-pass pump) — the mission stays green.
//
// SOCK-7 extends the SOCK-6 witness into a TWO-accept machine on ONE persistent listener:
//   * the FIRST inbound connection is accepted + echoed and latches the SOCK-6 `witness OK` line
//     (basic listen/accept still works — the regression);
//   * the listener SURVIVES the accept (the SOCK-7 point), so the SECOND inbound connection to the SAME
//     port is accepted + echoed on the SAME persistent listener and latches the SOCK-7 `witness OK` line.
// The injector's `sock7` verb drives exactly these two sequential connections. Each accept PEELS a fresh
// connection socket-id; the listener socket-id is constant across both.

/// SOCK-6/7 listen port for the server witness (avoids the hand-rolled stack's ports: 7 echo, 7777/7778
/// self-test, 9998/9999 UDP, 53 DNS). `net-inject.py` active-opens here.
pub const SOCK6_LISTEN_PORT: u16 = 8080;
/// Witness state: 0 = idle (arm the persistent listener); 1 = listening for connection #1 (poll accept);
/// 2 = serving #1 (await its probe, echo, latch SOCK-6 OK); 3 = listening for connection #2 on the SAME
/// persistent listener; 4 = serving #2 (echo, latch SOCK-7 OK); 5 = done.
static WITNESS6_STATE: AtomicU32 = AtomicU32::new(0);
/// service_net call counter — settle past SOCK-3's witness before arming.
static WITNESS6_TICKS: AtomicU32 = AtomicU32::new(0);
/// The PERSISTENT listening socket-id, stored between passes (the socket lives in the persistent set,
/// owned by the kernel `usize::MAX` row so no ring-3 teardown frees it). Constant across both accepts —
/// the SOCK-7 point. `usize::MAX` = none armed yet.
static WITNESS6_SID: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The PEELED connection socket-id currently being served (set by each accept). `usize::MAX` = none.
static WITNESS7_CONN: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The one-shot PENDING notes print exactly once even though the machine may RE-ARM across a
/// frame-stealing-lost attempt (the two RX drains — hand-rolled `poll()` and the smolnet pump — race
/// for inbound frames; a lost handshake/probe just re-arms and retries rather than latching INCOMPLETE).
static WITNESS6_PENDING_SHOWN: AtomicBool = AtomicBool::new(false);
/// Warm up past SOCK-3's witness (WARMUP 40) so the boot self-test + the ICMP/UDP/TCP witnesses settle.
const WITNESS6_WARMUP: u32 = 56;

/// SOCK-6/7 server witness (M2), STATEFUL across `service_net` passes: arm a PERSISTENT TCP LISTENER on
/// `SOCK6_LISTEN_PORT`, accept + echo a FIRST inbound connection (SOCK-6 OK), then — the listener having
/// survived — accept + echo a SECOND inbound connection on the SAME listener (SOCK-7 OK). Proves the
/// persistent listen/accept seam carries repeated inbound connections. Runs on the BSP main loop AFTER
/// the NET_DEVICE guard drops (the pumps re-lock NET_DEVICE per ring op). No-op once both served / no NIC.
/// The ring-3 `sys_listen`/`sys_accept` syscalls wrap this exact seam.
pub fn witness_tick6() {
    let state = WITNESS6_STATE.load(Ordering::Relaxed);
    if state == 5 {
        return; // both connections were accepted + echoed and the SOCK-7 witness latched
    }
    if WITNESS6_TICKS.fetch_add(1, Ordering::Relaxed) < WITNESS6_WARMUP {
        return;
    }
    if e1000::hw_addr().is_none() {
        return;
    }

    if state == 0 {
        // Arm the persistent listener. Kernel ownership (`usize::MAX`) — never a ring-3 slot, so
        // `free_row_sockets` for a real task never frees it; it is closed explicitly below on teardown.
        let Some(sid) = stack_open_tcp(usize::MAX) else {
            return; // no free slot / no NIC yet — retry next pass
        };
        if stack_listen(sid, SOCK6_LISTEN_PORT).is_err() {
            stack_close(sid);
            return;
        }
        WITNESS6_SID.store(sid, Ordering::Relaxed);
        WITNESS6_STATE.store(1, Ordering::Relaxed);
        // Announce the armed persistent listener exactly once (re-arms after a lost attempt stay quiet).
        if !WITNESS6_PENDING_SHOWN.swap(true, Ordering::Relaxed) {
            serial_println!(
                ":: SOCK-6: smoltcp tcp listen :{} armed — awaiting inbound connect (UNAOS_NET=socket injector) — witness PENDING ::",
                SOCK6_LISTEN_PORT
            );
            serial_println!(
                ":: SOCK-7: persistent listener :{} armed — awaiting a SECOND inbound connect (survives accept) — witness PENDING ::",
                SOCK6_LISTEN_PORT
            );
        }
        return;
    }

    let lsid = WITNESS6_SID.load(Ordering::Relaxed);

    // States 1 & 3: LISTENING — poll accept on the persistent listener (a light bounded pump per pass;
    // the injector re-drives). Each accept peels a fresh connection socket-id; the listener survives.
    if state == 1 || state == 3 {
        match stack_accept(lsid) {
            AcceptOutcome::Connected(conn_sid) => {
                // A peer completed the handshake — remember the peeled connection + advance to SERVING.
                // Do NOT recv here: the probe may not have landed yet, so serving gets its own passes.
                WITNESS7_CONN.store(conn_sid, Ordering::Relaxed);
                WITNESS6_STATE.store(if state == 1 { 2 } else { 4 }, Ordering::Relaxed);
            }
            AcceptOutcome::Pending => { /* still listening — poll again next pass */ }
            AcceptOutcome::NotListening => {
                // The listener vanished / never armed — re-arm from scratch on the next pass.
                stack_close(lsid);
                WITNESS6_SID.store(usize::MAX, Ordering::Relaxed);
                WITNESS6_STATE.store(0, Ordering::Relaxed);
            }
        }
        return;
    }

    // States 2 & 4: SERVING — the peeled connection is ESTABLISHED; wait (across passes) for the peer's
    // probe, echo it, close the CONNECTION (never the persistent listener), and latch the OK line. A clean
    // EOF before any probe means the peer closed early — drop the connection and go back to listening.
    let conn = WITNESS7_CONN.load(Ordering::Relaxed);
    let mut buf = [0u8; 64];
    match stack_recv(conn, &mut buf) {
        RecvOutcome::Data(n) => {
            let sent = stack_send(conn, &buf[..n]).unwrap_or(0);
            stack_close(conn); // close the CONNECTION; the listener `lsid` stays armed (persistent)
            WITNESS7_CONN.store(usize::MAX, Ordering::Relaxed);
            if state == 2 {
                WITNESS6_STATE.store(3, Ordering::Relaxed); // first accept done — listen for the second
                serial_println!(
                    ":: SOCK-6: smoltcp tcp accept :{} — received {} bytes, echoed {} back — witness {} ::",
                    SOCK6_LISTEN_PORT, n, sent,
                    if sent == n { "OK" } else { "INCOMPLETE" }
                );
            } else {
                WITNESS6_STATE.store(5, Ordering::Relaxed); // second accept done — latch SOCK-7 OK
                serial_println!(
                    ":: SOCK-7: smoltcp tcp accept :{} #2 — received {} bytes, echoed {} back on a PERSISTENT listener (second inbound connection accepted after the first was consumed) — witness {} ::",
                    SOCK6_LISTEN_PORT, n, sent,
                    if sent == n { "OK" } else { "INCOMPLETE" }
                );
            }
        }
        RecvOutcome::WouldBlock => { /* connected, no probe yet — retry recv next pass */ }
        RecvOutcome::Eof => {
            // The peer closed before sending a probe (a lost/aborted attempt) — drop the connection and
            // go back to LISTENING on the persistent listener, rather than latch a false INCOMPLETE.
            stack_close(conn);
            WITNESS7_CONN.store(usize::MAX, Ordering::Relaxed);
            WITNESS6_STATE.store(if state == 2 { 1 } else { 3 }, Ordering::Relaxed);
        }
    }
}

// =============================================================================
// SNTP-X86 (CLOCK-1 x86 follow-up): the x86 smolnet SNTP CLIENT that anchors the shared kernel clock.
//
// The pi/genet stack learns civil time over SNTP (PI-NET-16); CLOCK-1 gave both arches the shared
// `crate::clock` civil service (`set_anchor`/`unix_now`/`iso8601_now`), but x86 had no network writer —
// `time` there stayed `unsynced` until a manual `date -s`. This closes that gap: one RFC 4330 client-mode
// request over the PERSISTENT UDP stack, its reply parsed by the shared, hostile-input-hardened
// `crate::net_sntp` parser, then `crate::clock::set_anchor(unix, mono, Sntp{stratum})`.
//
// Target: UnaOS smolnet has no DNS client yet (SOCK-8+), so the client cannot resolve `pool.ntp.org`. It
// targets the LIVE default gateway (`sntp_target()` — the DHCP-leased router, or slirp's 10.0.2.2). Under
// the hermetic `./arroyo test` slirp backend the gateway does NOT answer :123, so the boot witness prints
// the honest `no reply` one-liner and the clock stays unsynced; the deterministic `sntp_x86_gate` (below,
// `witness`-feature) proves the parser + clock-anchor path with canned datagrams in ANY environment. On
// real hardware behind a router/LAN server that answers NTP, the same client anchors `time` for real.
// =============================================================================

/// SNTP-X86 client ephemeral source port — distinct from the SOCK-2 witness (49200) so a co-resident
/// witness never collides on a bound port.
const SNTP_SPORT: u16 = 49250;

/// SNTP-X86: one bounded, non-blocking sync attempt against `server`. Opens a kernel-owned UDP socket in
/// the persistent set (`usize::MAX` owner — never a ring-3 slot, so no task teardown frees it), binds the
/// ephemeral client port, sends the 48-byte client request to `server:123`, pumps for the reply, and
/// parses it. On a usable reply it anchors `crate::clock` as `Sntp{stratum}` (capturing the monotonic tick
/// in the same breath) and returns `(unix_secs, stratum)`. `None` on no NIC / no reply / KoD / malformed —
/// the caller emits the honest witness. The socket is always closed before returning.
pub fn sntp_sync_once(server: [u8; 4]) -> Option<(u64, u8)> {
    let sid = stack_open(usize::MAX)?;
    let mut req = [0u8; crate::net_sntp::SNTP_LEN];
    crate::net_sntp::build_request(&mut req);
    let _ = stack_bind(sid, SNTP_SPORT);
    let _ = stack_sendto(sid, server, crate::net_sntp::NTP_PORT, &req);
    let mut buf = [0u8; 64];
    let got = stack_recvfrom(sid, &mut buf);
    stack_close(sid);
    let (_src, _port, n) = got?;
    sntp_apply(&buf[..n], &mut crate::clock::LiveClock) // CONSOLEFIX M1: the one datagram -> clock path; THIS caller is the only one that names the live clock
}

/// CONSOLEFIX M1 (B365): THE path from an SNTP datagram to a clock. Parses with the shared
/// `crate::net_sntp` parser and, on a usable reply only, plants `(unix, mono, Sntp{stratum})` on `sink`,
/// capturing the monotonic tick "in the same breath" (the CLOCK-1 seam). KoD / malformed / short plant
/// nothing. The live client passes `clock::LiveClock`; `sntp_x86_gate` passes a `clock::FixtureClock`,
/// so the fixture runs the live code and never touches the live anchor.
pub fn sntp_apply(pkt: &[u8], sink: &mut dyn crate::clock::AnchorSink) -> Option<(u64, u8)> {
    match crate::net_sntp::parse(pkt) {
        crate::net_sntp::Sntp::Ok { unix_secs, stratum } => {
            let mono = crate::clock::mono_ticks().unwrap_or(0);
            sink.plant(unix_secs, mono, crate::clock::ClockSource::Sntp { stratum });
            Some((unix_secs, stratum))
        }
        _ => None,
    }
}

/// True once the SNTP-X86 boot witness has run (one-shot).
static WITNESS_SNTP_DONE: AtomicBool = AtomicBool::new(false);
/// service_net call counter — settle well past the SOCK-1..6 witnesses (and the boot DHCP) before syncing.
static WITNESS_SNTP_TICKS: AtomicU32 = AtomicU32::new(0);
/// Warm up past SOCK-6's witness (WARMUP 56) so the boot self-test, the ICMP/UDP/TCP/server witnesses, and
/// the SOCK-5 DHCP lease all settle — the SNTP client then targets the leased router if DHCP replaced it.
const WITNESS_SNTP_WARMUP: u32 = 2; // SNTPDRV M3: was 72 service_net passes; now counts 5 s clock ticks (net_tick::service_tick), so ~10 s of settle

/// SNTP-X86 boot witness: knob-on, one-shot — sync the shared clock from the live gateway over SNTP and
/// emit the witness line. Runs on the BSP main loop from `service_net`, AFTER the NET_DEVICE guard drops
/// (the pump re-locks NET_DEVICE per ring op). Hermetically the gateway is silent, so this prints the
/// honest `no reply` note once; on real hardware with an NTP-answering router it anchors `time` for real.
/// No-op once done / no NIC.
pub fn witness_tick_sntp() {
    if WITNESS_SNTP_DONE.load(Ordering::Relaxed) {
        return;
    }
    if WITNESS_SNTP_TICKS.fetch_add(1, Ordering::Relaxed) < WITNESS_SNTP_WARMUP {
        return;
    }
    if e1000::hw_addr().is_none() {
        return;
    }
    WITNESS_SNTP_DONE.store(true, Ordering::Relaxed);

    // SOCK-8: prefer a resolved `pool.ntp.org` when DNS is available; fall back to the live gateway if the
    // resolve fails (no lease DNS / no reply). Either way the target is an honest, reachable-or-not IP the
    // witness prints. Under hermetic slirp the resolve may actually succeed (10.0.2.3 forwards to the host),
    // but the SNTP leg then still needs an NTP-answering server on that address to anchor `time`.
    // USBNET4 M3: never a hardcoded 10.0.2.2 on a metal that has no lease. Targets: pool.ntp.org via DNS
    // (only with a lease), then the leased router; no lease -> `none`, no query.
    let mut from = "none";
    let mut server = [0u8; 4];
    if LEASED.load(Ordering::Relaxed) {
        match resolve(SNTP_POOL_HOST) {
            Some(ip) => { server = ip; from = "dns"; }
            None => { server = sntp_target(); from = "lease"; }
        }
    }
    static NONE_SAID: AtomicBool = AtomicBool::new(false);
    if from != "none" || !NONE_SAID.swap(true, Ordering::Relaxed) {
        serial_println!("[sntp] target={} from={}", e1000::fmt_ip(&server), from);
    }
    if from == "none" {
        WITNESS_SNTP_DONE.store(false, Ordering::Relaxed); // retry on a later tick once a lease lands
        return;
    }
    match sntp_sync_once(server) {
        Some((unix, stratum)) => {
            let mut iso = [0u8; 24];
            let n = crate::clock::render_iso8601(unix, &mut iso);
            let iso_str = core::str::from_utf8(&iso[..n]).unwrap_or("????");
            serial_println!(
                ":: SMOLNET: [sntp] {} -> {} (stratum {}) ::",
                e1000::fmt_ip(&server),
                iso_str,
                stratum
            );
        }
        None => {
            serial_println!(
                ":: SMOLNET: [sntp] {} no reply — clock unsynced (gateway answers no NTP under hermetic slirp; needs a real NTP server) ::",
                e1000::fmt_ip(&server)
            );
        }
    }
}

// --- SNTP-X86 deterministic gate (witness-feature): the parser + clock-anchor path, canned + hermetic ---

/// The injected wall-clock instant the SNTP gate scripts: 2026-07-22T15:30:45Z. Mirrors pi/genet's NET16
/// fixture so the two arches assert the SAME round-trip anchor. `INJ_UNIX` is its UTC Unix second;
/// `INJ_ISO` the string `crate::clock::render_iso8601` must reproduce from it.
#[cfg(feature = "witness")]
const INJ_UNIX: u64 = 1_784_734_245;
#[cfg(feature = "witness")]
const INJ_ISO: &str = "2026-07-22T15:30:45Z";

/// SNTP-X86 deterministic gate (armed by the `witness` battery feature): drive canned SNTP datagrams
/// through the shared `crate::net_sntp` parser and the `crate::clock` anchor path, asserting each outcome,
/// with NO NIC or network required — so `./arroyo test` proves x86 SNTP correctness in any environment.
/// Prints the self-checking `:: SNTP-X86-GATE: ... PASS [w=0x..] ::` line the x86 battery asserts.
/// Bitmask `w`: 0x01 well-formed reply parses to the exact Unix second AND renders to `INJ_ISO`; 0x02 a
/// short (<48 B) datagram is rejected; 0x04 stratum 0 surfaces as Kiss-o'-Death; 0x08 an LI=3 alarm reply
/// is rejected; 0x10 a live parse anchors `crate::clock` as `Sntp{stratum}` and the anchored ISO is
/// exactly `INJ_ISO`.
#[cfg(feature = "witness")]
pub fn sntp_x86_gate() {
    use crate::net_sntp::{self, Sntp};
    let mut w: u32 = 0;

    // 0x01 — well-formed reply: exact Unix second + ISO round-trip.
    let good = net_sntp::build_reply(INJ_UNIX, 0, 4, 4, 2);
    let parse_ok = match net_sntp::parse(&good) {
        Sntp::Ok { unix_secs, stratum } => {
            let mut iso = [0u8; 24];
            let n = crate::clock::render_iso8601(unix_secs, &mut iso);
            unix_secs == INJ_UNIX
                && stratum == 2
                && core::str::from_utf8(&iso[..n]) == Ok(INJ_ISO)
        }
        _ => false,
    };
    if parse_ok {
        w |= 0x01;
    }
    serial_println!(
        ":: [sntp-x86] parse well-formed => {} ::",
        if parse_ok { "resolved+ISO PASS" } else { "FAIL" }
    );

    // 0x02 — reject a short packet.
    let rej_short = matches!(net_sntp::parse(&good[..40]), Sntp::Malformed);
    if rej_short {
        w |= 0x02;
    }
    serial_println!(
        ":: [sntp-x86] reject short packet => {} ::",
        if rej_short { "malformed PASS" } else { "FAIL" }
    );

    // 0x04 — surface stratum-0 Kiss-o'-Death.
    let kod = net_sntp::build_reply(INJ_UNIX, 0, 4, 4, 0);
    let is_kod = matches!(net_sntp::parse(&kod), Sntp::KissOfDeath);
    if is_kod {
        w |= 0x04;
    }
    serial_println!(
        ":: [sntp-x86] surface KoD (stratum 0) => {} ::",
        if is_kod { "KoD PASS" } else { "FAIL" }
    );

    // 0x08 — reject an LI=3 alarm reply.
    let alarm = net_sntp::build_reply(INJ_UNIX, 3, 4, 4, 2);
    let rej_alarm = matches!(net_sntp::parse(&alarm), Sntp::Malformed);
    if rej_alarm {
        w |= 0x08;
    }
    serial_println!(
        ":: [sntp-x86] reject LI=3 alarm => {} ::",
        if rej_alarm { "malformed PASS" } else { "FAIL" }
    );

    // 0x10 — the anchor path, on a FIXTURE clock (CONSOLEFIX M1, B365). Flight 22: this leg planted its canned
    // July-22 value on the LIVE anchor; the RTC had anchored first, so the old cleanup left the canned value
    // standing for the rest of the boot (stamps 05:13 -> 15:30, the bar "wed 22 jul"). It now plants a
    // `clock::FixtureClock` through `sntp_apply` — the function the live client calls — and asserts the
    // deterministic (non-extrapolated) anchor renders exactly INJ_ISO and is tagged Sntp{2}.
    // 0x20 — the live clock did not move: `raw_anchor` before == after (source and base second).
    let before = crate::clock::raw_anchor();
    let mut fixture = crate::clock::FixtureClock::new();
    let set_ok = match sntp_apply(&good, &mut fixture) {
        Some((unix_secs, 2)) if unix_secs == INJ_UNIX => match fixture.raw_anchor() {
            Some((anchor_unix, crate::clock::ClockSource::Sntp { stratum: 2 })) => {
                let mut iso = [0u8; 24];
                let n = crate::clock::render_iso8601(anchor_unix, &mut iso);
                core::str::from_utf8(&iso[..n]) == Ok(INJ_ISO) && fixture.unix_now().is_some()
            }
            _ => false,
        },
        _ => false,
    };
    // A KoD datagram must plant nothing, on the fixture either.
    let mut kod_clock = crate::clock::FixtureClock::new();
    let kod_inert = sntp_apply(&kod, &mut kod_clock).is_none() && kod_clock.raw_anchor().is_none();
    fixture.clear();
    if set_ok && kod_inert {
        w |= 0x10;
    }
    serial_println!(
        ":: [sntp-x86] canned reply anchors the FIXTURE clock => {} ::",
        if set_ok && kod_inert { "2026-07-22T15:30:45Z PASS" } else { "FAIL" }
    );
    let after = crate::clock::raw_anchor();
    let untouched = before == after;
    if untouched {
        w |= 0x20;
    }
    let src = after.map(|(_, s)| crate::clock::source_word(s)).unwrap_or("unset");
    match (before, after) {
        (Some((b, _)), Some((a, _))) => serial_println!(":: [sntp-x86] live clock untouched source={} before={} after={} => {} ::", src, b, a, if untouched { "PASS" } else { "FAIL" }),
        (None, None) => serial_println!(":: [sntp-x86] live clock untouched source=unset before=none after=none => PASS ::"),
        _ => serial_println!(":: [sntp-x86] live clock untouched source={} before={} after={} => FAIL ::", src, before.is_some() as u8, after.is_some() as u8),
    }

    let pass = w == 0x3f;
    serial_println!(
        ":: SNTP-X86-GATE: x86 sntp client battery {} [w=0x{:x}] (parse-ok+iso|reject-short|kod|reject-alarm|fixture-clock|live-untouched) -> {} ::",
        if pass { "PASS" } else { "FAIL" },
        w,
        if pass { "PASS" } else { "FAIL" }
    );
}

// =============================================================================
// SOCK-8 (ROADMAP §1b): the x86 smolnet DNS CLIENT over the shared, arch-neutral `crate::net_dns`.
//
// Until now smolnet had no way to turn a name into an address, so SNTP-X86 could only target the gateway.
// SOCK-8 gives smolnet a resolver built on the SAME hostile-input-hardened wire logic the pi/genet
// PI-NET-14 client uses — now extracted to `crate::net_dns` (the `net_sntp` sharing pattern), which genet
// migrates onto in a later fold. `resolve()` sends one A-record query over the PERSISTENT UDP stack to the
// DHCP-provided DNS server (`dns_server()` — the lease's DNS option, else the gateway) and parses the reply
// with `net_dns::parse_a`. The SNTP client now prefers a resolved `pool.ntp.org` (see `witness_tick_sntp`).
//
// Under the hermetic `./arroyo test` slirp backend, slirp's DHCP offers 10.0.2.3 as the DNS server AND
// forwards queries to the host resolver, so a live boot resolve may actually SUCCEED — captured as a bonus
// witness. The deterministic `dns_x86_gate` (below, `witness`-feature) proves the parser with canned
// datagrams in ANY environment and NEVER depends on external reachability.
// =============================================================================

/// The host the SNTP client resolves for its time source (RFC-standard NTP pool name).
const SNTP_POOL_HOST: &str = "pool.ntp.org";
/// SOCK-8 resolver ephemeral source port — distinct from the SOCK-2 witness (49200) and the SNTP client
/// (49250) so a co-resident witness never collides on a bound port.
const DNS_SPORT: u16 = 49260;
/// A fixed transaction id for the resolver's queries. `parse_a` requires the reply to echo it (a mismatch
/// is a stale/spoofed datagram). ASCII "SD" (Sock-Dns).
const DNS_TXID: u16 = 0x5344;

/// SOCK-8: resolve `host` to a single IPv4 A record via one bounded, non-blocking UDP query to the
/// DHCP-provided DNS server (`dns_server()`, gateway fallback). Opens a kernel-owned UDP socket in the
/// persistent set (`usize::MAX` owner — never a ring-3 slot, so no task teardown frees it), binds the
/// ephemeral client port, sends the A-record query to `server:53`, pumps for the reply, and parses it with
/// the shared, hostile-input-hardened `crate::net_dns::parse_a`. Returns the four address octets on a
/// resolved A record, or `None` on no NIC / query-build failure / no reply / a malformed / no-answer /
/// server-error reply. The socket is always closed before returning.
pub fn resolve(host: &str) -> Option<[u8; 4]> {
    let server = dns_server();
    let mut qbuf = [0u8; 300];
    let qlen = crate::net_dns::build_query(&mut qbuf, DNS_TXID, host)?;
    let sid = stack_open(usize::MAX)?;
    let _ = stack_bind(sid, DNS_SPORT);
    let _ = stack_sendto(sid, server, crate::net_dns::DNS_PORT, &qbuf[..qlen]);
    let mut buf = [0u8; 512];
    let got = stack_recvfrom(sid, &mut buf);
    stack_close(sid);
    let (_src, _port, n) = got?;
    match crate::net_dns::parse_a(&buf[..n], DNS_TXID) {
        crate::net_dns::Dns::Resolved(ip) => Some(ip),
        _ => None, // malformed / no A record / server error — the caller falls back
    }
}

/// True once the SOCK-8 boot DNS witness has run (one-shot).
static WITNESS_DNS_DONE: AtomicBool = AtomicBool::new(false);
/// service_net call counter — settle past the SOCK-5 DHCP lease (so `dns_server()` sees the leased DNS) but
/// before the SNTP witness, which reuses the resolver.
static WITNESS_DNS_TICKS: AtomicU32 = AtomicU32::new(0);
/// Warm up past SOCK-6's witness (WARMUP 56) + the DHCP lease, and ahead of the SNTP witness (WARMUP 72).
const WITNESS_DNS_WARMUP: u32 = 64;

/// SOCK-8 boot DNS witness: knob-on, one-shot — resolve `pool.ntp.org` through the shared DNS client and
/// emit the witness line. Runs on the BSP main loop from `service_net`, AFTER the NET_DEVICE guard drops
/// (the UDP pump re-locks NET_DEVICE per ring op). Under hermetic slirp the resolve may SUCCEED (10.0.2.3
/// forwards to the host resolver) — captured as the live-resolve witness; if it times out, the honest
/// `no answer` line prints instead. Either way the mission stays green. No-op once done / no NIC.
pub fn witness_tick_dns() {
    if WITNESS_DNS_DONE.load(Ordering::Relaxed) {
        return;
    }
    if WITNESS_DNS_TICKS.fetch_add(1, Ordering::Relaxed) < WITNESS_DNS_WARMUP {
        return;
    }
    if e1000::hw_addr().is_none() {
        return;
    }
    WITNESS_DNS_DONE.store(true, Ordering::Relaxed);

    let server = dns_server();
    match resolve(SNTP_POOL_HOST) {
        Some(ip) => serial_println!(
            ":: SMOLNET: [dns] {} -> {} (via {}) ::",
            SNTP_POOL_HOST,
            e1000::fmt_ip(&ip),
            e1000::fmt_ip(&server)
        ),
        None => serial_println!(
            ":: SMOLNET: [dns] {} no answer (via {}) — resolver reachable only where the nameserver replies ::",
            SNTP_POOL_HOST,
            e1000::fmt_ip(&server)
        ),
    }
}

// --- SOCK-8 deterministic gate (witness-feature): the DNS parser, canned + hermetic ---

/// SOCK-8 deterministic gate (armed by the `witness` battery feature): drive canned DNS datagrams through
/// the shared `crate::net_dns` parser, asserting each outcome, with NO NIC or network required — so
/// `./arroyo test` proves x86 DNS parsing correctness in any environment. Prints the self-checking
/// `:: DNS-X86-GATE: ... PASS [w=0x..] ::` line the x86 battery asserts. Bitmask `w`: 0x01 a well-formed A
/// reply (with the universal compression pointer in the answer name, NEVER dereferenced) parses to the
/// exact address; 0x02 a truncated reply (RR cut short) is rejected as `Malformed`; 0x04 a compression LOOP
/// (a QNAME that is a self-pointer) is rejected WITHOUT hanging (loop-immune by construction); 0x08 a
/// non-zero RCODE (NXDOMAIN=3) surfaces as `ServerErr(3)`.
#[cfg(feature = "witness")]
pub fn dns_x86_gate() {
    use crate::net_dns::{self, Dns};
    let mut w: u32 = 0;

    // 0x01 — well-formed A reply parses to the exact address. The answer name is a compression pointer
    // (0xc0 0x0c) exactly as a real resolver emits — `parse_a` accepts it as a name terminator without
    // dereferencing it, and still extracts the A RDATA.
    let mut good = [0u8; 512];
    let gp = net_dns::build_a_reply(&mut good, DNS_TXID, "example.com", [93, 184, 216, 34], 0, 1)
        .unwrap_or(0);
    let parse_ok = matches!(net_dns::parse_a(&good[..gp], DNS_TXID), Dns::Resolved([93, 184, 216, 34]));
    if parse_ok {
        w |= 0x01;
    }
    serial_println!(
        ":: [dns-x86] parse well-formed A => {} ::",
        if parse_ok { "93.184.216.34 PASS" } else { "FAIL" }
    );

    // 0x02 — a truncated reply (the answer RR cut off mid-header) is rejected. Chop the well-formed reply
    // to just past the question so the single claimed answer's fixed header runs off the end.
    let trunc_len = gp.saturating_sub(8); // drop the last 8 bytes of the 16-byte answer RR
    let rej_trunc = matches!(net_dns::parse_a(&good[..trunc_len], DNS_TXID), Dns::Malformed);
    if rej_trunc {
        w |= 0x02;
    }
    serial_println!(
        ":: [dns-x86] reject truncated answer => {} ::",
        if rej_trunc { "malformed PASS" } else { "FAIL" }
    );

    // 0x04 — a compression LOOP is rejected without hanging. The QNAME at offset 12 is a self-referential
    // pointer (0xc0 0x0c → points at itself); `skip_name` treats it as a two-byte terminator and does NOT
    // follow it (loop-immune by construction), so the walk terminates — and because the claimed single
    // answer is absent, the parse is rejected as Malformed rather than looping forever.
    let loop_pkt: [u8; 18] = [
        0x53, 0x44, // txid = DNS_TXID
        0x81, 0x80, // QR=1 RD=1 RA=1, rcode 0
        0x00, 0x01, // QDCOUNT = 1
        0x00, 0x01, // ANCOUNT = 1 (claimed, but absent)
        0x00, 0x00, 0x00, 0x00, // NS/AR = 0
        0xc0, 0x0c, // QNAME = pointer to offset 12 (itself) — a compression loop
        0x00, 0x01, // QTYPE A
        0x00, 0x01, // QCLASS IN
    ];
    let rej_loop = matches!(net_dns::parse_a(&loop_pkt, DNS_TXID), Dns::Malformed);
    if rej_loop {
        w |= 0x04;
    }
    serial_println!(
        ":: [dns-x86] reject compression loop (no hang) => {} ::",
        if rej_loop { "malformed PASS" } else { "FAIL" }
    );

    // 0x08 — a non-zero RCODE surfaces as ServerErr. Build an NXDOMAIN (rcode 3), zero answers.
    let mut nx = [0u8; 64];
    let np = net_dns::build_a_reply(&mut nx, DNS_TXID, "nope.example", [0, 0, 0, 0], 3, 0).unwrap_or(0);
    let is_nx = matches!(net_dns::parse_a(&nx[..np], DNS_TXID), Dns::ServerErr(3));
    if is_nx {
        w |= 0x08;
    }
    serial_println!(
        ":: [dns-x86] surface NXDOMAIN (rcode 3) => {} ::",
        if is_nx { "ServerErr(3) PASS" } else { "FAIL" }
    );

    let pass = w == 0x0f;
    serial_println!(
        ":: DNS-X86-GATE: x86 dns client battery {} [w=0x{:x}] (parse-A|reject-truncated|reject-loop|rcode) ::",
        if pass { "PASS" } else { "FAIL" },
        w
    );
}

/// SNTPDRV M1: true once the SNTP-X86 one-shot has run to its verdict (`net_tick` witnesses on it).
pub fn sntp_done() -> bool {
    WITNESS_SNTP_DONE.load(Ordering::Relaxed)
}

/// USBNET6: true once a DHCP lease replaced the static config (the `tests usbnet` census reads it).
pub fn leased() -> bool {
    LEASED.load(Ordering::Relaxed)
}

/// USBNET6 M3: tries so far of the link-driven DHCP acquisition (bounded: a silent network is not retried forever).
#[cfg(feature = "usbnet")]
static DHCP_LINK_TRIES: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "usbnet")]
const DHCP_LINK_MAX: u32 = 6;

/// USBNET6 M3: DHCP on the USB Ethernet link. Flight 19 printed NO `SOCK-5` line on any boot: `dhcp_acquire`
/// runs only from `init()`, and `init()` only from the ring-3 socket syscalls, while the stack on the metal is
/// built lazily by the SOCK-2 witness with the static slirp config — so the dongle received frames for minutes
/// and the lease was never asked for. Driven from the 5 s `net_tick::service_tick` (main loop, large stack):
/// once the dongle's PHY reports link and no lease has landed, build the stack if needed and run the same
/// chunked one-shot acquisition (re-armed per try), at most `DHCP_LINK_MAX` tries, one `SOCK-5` line each.
/// The e1000 path is untouched (its DHCP stays where it was).
#[cfg(feature = "usbnet")]
pub fn dhcp_link_tick() {
    use crate::drivers::xhci::usbnet;
    if LEASED.load(Ordering::Relaxed) || e1000::nic_present() {
        return;
    }
    if !usbnet::is_up() || (usbnet::kind() == usbnet::KIND_AX88179 && !usbnet::link_up()) {
        return;
    }
    if usbnet::take_rx_resumed() && DHCP_LINK_TRIES.swap(0, Ordering::Relaxed) >= DHCP_LINK_MAX {
        serial_println!("[usbnet] dhcp link tries re-armed (rx resumed after a stall)"); // NETFRAME (B368)
    }
    if DHCP_LINK_TRIES.fetch_add(1, Ordering::Relaxed) >= DHCP_LINK_MAX {
        return;
    }
    {
        let mut g = STACK.lock();
        if !ensure_stack(&mut g) {
            return;
        }
    }
    DHCP_ATTEMPTED.store(false, Ordering::Release);
    dhcp_acquire();
}

// =============================================================================
// NETCLOCK (B335) M5: `tests netclock` — the idle clock on a live USB link, measured.
// =============================================================================

/// Idle bounds (per second, over the 5 s window) and the stack-side loan hold bound.
#[cfg(feature = "usbnet")]
const NETCLOCK_POLLS_MAX: u64 = 50;
#[cfg(feature = "usbnet")]
const NETCLOCK_TX_MAX: u64 = 5;
#[cfg(feature = "usbnet")]
const NETCLOCK_LOAN_MAX_US: u64 = 500;
#[cfg(feature = "usbnet")]
const NETCLOCK_WINDOW_MS: i64 = 5_000;

/// `tests netclock`: 5 s of idle on a live USB link. The shell holds the main loop while a fixture runs, so
/// the fixture runs the main loop's two net calls itself — the xHCI pass (`usbnet::main_pass`: event drain,
/// FTDI drain, the full `service_usbnet`) and `service_poll` — and counts what the stack does on its own:
/// `polls_per_s` (every `iface.poll`), `tx_per_s` (bulk-OUT completions), `loan_held_max_us` (the longest
/// stack-side controller hold, `usbnet::drive`), `rx_ok` (frames received this boot).
/// `:: NETCLOCK: polls_per_s=<n> tx_per_s=<n> loan_held_max_us=<n> rx_ok=<n> -> PASS|FAIL ::`, PASS at
/// polls <= 50/s, tx <= 5/s, loan <= 500 us. No dongle / no PHY link / an e1000 link -> SKIP with the reason.
#[cfg(feature = "usbnet")]
pub fn netclock_selftest() {
    use crate::drivers::xhci::usbnet;
    let skip = if e1000::nic_present() {
        Some("e1000-link")
    } else if !usbnet::is_up() {
        Some("no-dongle")
    } else if usbnet::kind() == usbnet::KIND_AX88179 && !usbnet::link_up() {
        Some("no-link")
    } else {
        None
    };
    if let Some(why) = skip {
        serial_println!(":: NETCLOCK: polls_per_s=0 tx_per_s=0 loan_held_max_us=0 rx_ok={} -> SKIP reason={} ::", usbnet::rx_ok(), why);
        return;
    }
    {
        let mut g = STACK.lock();
        if !ensure_stack(&mut g) {
            serial_println!(":: NETCLOCK: polls_per_s=0 tx_per_s=0 loan_held_max_us=0 rx_ok=0 -> SKIP reason=no-stack ::");
            return;
        }
    }
    // CONSOLEFIX M1 (B365): flight 22 PASSed this on a stack that never got a lease (`SOCK-5: dhcpv4 no offer —
    // static fallback stands 10.0.2.15/24`, `[sntp] target=0.0.0.0 from=none`). The poll budget of a stack with
    // no lease says nothing about time arriving: no lease, an honest SKIP.
    if !leased() {
        serial_println!(":: NETCLOCK: polls_per_s=0 tx_per_s=0 loan_held_max_us=0 rx_ok={} lease=none clock={} -> SKIP reason=no-lease ::", usbnet::rx_ok(), crate::clock::source_word(crate::clock::source()));
        return;
    }
    let p0 = crate::net_tick::polls();
    let t0 = usbnet::tx_frames();
    usbnet::loan_reset();
    let start = now_ms();
    while now_ms() - start < NETCLOCK_WINDOW_MS {
        usbnet::main_pass();
        service_poll();
        for _ in 0..256 {
            core::hint::spin_loop();
        }
    }
    let el = (now_ms() - start).max(1) as u64;
    let polls_ps = (crate::net_tick::polls() - p0) * 1000 / el;
    let tx_ps = (usbnet::tx_frames() - t0) * 1000 / el;
    let loan = usbnet::loan_held_max_us();
    let ok = polls_ps <= NETCLOCK_POLLS_MAX && tx_ps <= NETCLOCK_TX_MAX && loan <= NETCLOCK_LOAN_MAX_US;
    serial_println!(
        ":: NETCLOCK: polls_per_s={} tx_per_s={} loan_held_max_us={} rx_ok={} lease=yes gw={} dns={} clock={} -> {} ::",
        polls_ps, tx_ps, loan, usbnet::rx_ok(), e1000::fmt_ip(&sntp_target()), e1000::fmt_ip(&dns_server()), crate::clock::source_word(crate::clock::source()), if ok { "PASS" } else { "FAIL" }
    );
}

/// NETFRAME (B368): `tests usbnet7` runs the stack's poll itself (the shell holds the main loop while a fixture
/// runs): build the stack if needed (its DHCP client then retransmits on smoltcp's own timer), one `service_poll`.
#[cfg(feature = "usbnet")]
pub fn usbnet7_poll() -> bool {
    {
        let mut g = STACK.lock();
        if !ensure_stack(&mut g) {
            return false;
        }
    }
    service_poll();
    true
}
