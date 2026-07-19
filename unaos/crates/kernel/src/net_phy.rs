// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// NET-PHY — the shared, arch-neutral `smoltcp::phy::Device` adapter.
//
// ## Why this module exists
//
// Three NIC seams bind a `smoltcp` interface over a device's RX/TX rings, and each carried a
// near-identical copy of the `phy::Device` / `RxToken` / `TxToken` boilerplate that sits between smoltcp
// and the driver's raw-frame accessors:
//   * x86 `smolnet.rs`     — the e1000e (SOCK-1..7, the DEFAULT x86 net stack)
//   * aarch64 `rtl8168_tegra.rs` — ORIN-NET-4 (Realtek RTL8168 on Orin)
//   * aarch64 `virtio_net.rs`    — AARCH64-VNET (virtio-net under QEMU virt)
//
// This module hosts that boilerplate ONCE, parameterized over a tiny [`RawNic`] trait
// (`transmit` / `rx_frame_raw` / `mac`) that each driver implements against its own device registry.
// ZERO behavior change: the adapter's datapath, the short-lock discipline, the no-alloc struct-local
// scratch, and the smoltcp capability shape are the exact code the drivers carried — now shared rather
// than duplicated across two arches.
//
// ## Home
//
// This lives at the crate root (`crates/kernel/src/net_phy.rs`), NOT under `arch/`, because it is shared
// by both the x86 default net stack and the aarch64 net drivers. It cannot live in a module named `net`:
// the kernel depends on an EXTERNAL crate `net` (`net::ethernet` / `net::arp`, used by `smolnet.rs`), and
// an internal `crate::net` module would shadow that extern crate inside this crate. A flat top-level file
// next to `smolnet.rs` is the arch-neutral home that avoids the collision.
//
// ## The RX observer seam
//
// x86 `smolnet.rs` additionally snoops inbound ARP replies as they cross `receive()` (smoltcp hides the
// resolved neighbor MAC, so the `arp`/`ping` shell commands recover it by watching the wire). The aarch64
// drivers do not. Rather than fork the adapter, `SmoltcpPhy` is generic over an [`RxObserver`] `O` (default
// `()` — a zero-cost no-op) whose `observe` runs on every received frame BEFORE the tokens are minted.
// aarch64 uses `O = ()` (compiles to the exact pre-share datapath); x86 supplies an ARP-snooping observer,
// reproducing its old `receive()` byte-for-byte.
//
// ## Gating
//
// Compiled only when at least one net feature is on (`any(net4, vnet, smolnet)`) — each of those pulls the
// optional `smoltcp` dep. With none, the module — and the smoltcp dep — vanish. Each driver / stack file
// remains additionally gated on its own feature (and, for smolnet, `target_arch = "x86_64"`), so this
// module compiles under any combination without dead-code warnings.

#![cfg(any(feature = "net4", feature = "vnet", feature = "smolnet", feature = "genet"))]

use core::marker::PhantomData;

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

/// A full Ethernet frame fits (the drivers' per-descriptor buffers are 2048); the Device scratch is
/// struct-local (no heap growth). Shared by every net adapter.
pub const FRAME_CAP: usize = 1536;

/// The raw-frame seam a driver implements so the shared [`SmoltcpPhy`] can move L2 frames to/from its
/// rings. All three are associated functions (no `self`) because each driver reaches its one registered
/// NIC through a module-static registry (`NET_DEVICE` / `NET4_DEVICE` / `VNET_DEVICE`) behind a short-held
/// lock — the e1000 `raw_rx`/`raw_tx` discipline: never hold the registry lock across a smoltcp poll.
pub trait RawNic {
    /// Pop one raw RX Ethernet frame into `out` (recycling the descriptor), or `None` if the ring is
    /// empty. Length-clamped by the driver so a misbehaving NIC cannot force an out-of-bounds slice.
    fn rx_frame_raw(out: &mut [u8]) -> Option<usize>;
    /// Transmit one raw L2 frame (smoltcp builds the full Ethernet frame).
    fn transmit(frame: &[u8]);
    /// The station MAC, or `None` if no NIC is registered.
    fn mac() -> Option<[u8; 6]>;
}

/// An observer run on every frame the phy receives, BEFORE the RX/TX tokens are minted. The default
/// implementation for `()` is a zero-cost no-op (the aarch64 drivers use it, compiling to the exact
/// pre-share datapath). x86 `smolnet` supplies an ARP-snooping observer so `arp`/`ping` can recover the
/// resolved neighbor MAC that smoltcp hides.
pub trait RxObserver {
    /// Called once per received frame with the raw L2 bytes. Must not block / re-enter the NIC.
    fn observe(&mut self, frame: &[u8]);
}

impl RxObserver for () {
    #[inline(always)]
    fn observe(&mut self, _frame: &[u8]) {}
}

/// A `smoltcp::phy::Device` backed by a [`RawNic`], with an optional [`RxObserver`]. Owns RX/TX scratch so
/// the tokens can borrow disjoint fields (smoltcp hands out both from one `receive()` to build a reply in
/// place). `O` defaults to `()` (no observer) — the aarch64 shape.
pub struct SmoltcpPhy<N: RawNic, O: RxObserver = ()> {
    rx: [u8; FRAME_CAP],
    rlen: usize,
    tx: [u8; FRAME_CAP],
    /// The per-frame RX observer (ARP-snoop on x86; `()` = nothing on aarch64). Public so a caller can
    /// read whatever state the observer accumulated after a poll (e.g. the snooped MAC).
    pub obs: O,
    _nic: PhantomData<N>,
}

impl<N: RawNic> SmoltcpPhy<N, ()> {
    /// A phy with no RX observer (the aarch64 shape). `SmoltcpPhy::<Nic>::new()`.
    pub fn new() -> Self {
        SmoltcpPhy {
            rx: [0; FRAME_CAP],
            rlen: 0,
            tx: [0; FRAME_CAP],
            obs: (),
            _nic: PhantomData,
        }
    }
}

impl<N: RawNic, O: RxObserver> SmoltcpPhy<N, O> {
    /// A phy carrying the given RX observer (the x86 ARP-snoop shape).
    pub fn with_observer(obs: O) -> Self {
        SmoltcpPhy {
            rx: [0; FRAME_CAP],
            rlen: 0,
            tx: [0; FRAME_CAP],
            obs,
            _nic: PhantomData,
        }
    }
}

pub struct PhyRxToken<'a> {
    buf: &'a [u8],
}
pub struct PhyTxToken<'a, N: RawNic> {
    buf: &'a mut [u8],
    _nic: PhantomData<N>,
}

impl RxToken for PhyRxToken<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(self.buf)
    }
}
impl<N: RawNic> TxToken for PhyTxToken<'_, N> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let n = len.min(self.buf.len());
        let r = f(&mut self.buf[..n]);
        N::transmit(&self.buf[..n]);
        r
    }
}

impl<N: RawNic, O: RxObserver> Device for SmoltcpPhy<N, O> {
    type RxToken<'a>
        = PhyRxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = PhyTxToken<'a, N>
    where
        Self: 'a;

    fn receive(&mut self, _t: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let len = N::rx_frame_raw(&mut self.rx)?;
        // Run the RX observer (ARP-snoop on x86; no-op on aarch64) before minting the tokens, exactly
        // where the x86 `receive()` snooped.
        self.obs.observe(&self.rx[..len]);
        self.rlen = len;
        let SmoltcpPhy { rx, rlen, tx, obs: _, _nic } = self;
        Some((
            PhyRxToken { buf: &rx[..*rlen] },
            PhyTxToken { buf: tx, _nic: PhantomData },
        ))
    }

    fn transmit(&mut self, _t: Instant) -> Option<Self::TxToken<'_>> {
        Some(PhyTxToken { buf: &mut self.tx, _nic: PhantomData })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1500;
        caps
    }
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// NET-DHCP — a shared, arch-neutral DHCPv4 bring-up helper on the smoltcp seam.
// ══════════════════════════════════════════════════════════════════════════════════════════════════
//
// Both aarch64 NIC seams (virtio-net under QEMU, RTL8168 on Orin metal) previously bound their smoltcp
// `Interface` to a hard-coded STATIC address. That is wrong for a real link whose subnet is a metal
// input (flagged at the ORIN-NET-4 landing). This helper runs smoltcp's `dhcpv4` socket over an already-
// built `Interface` until a lease is acquired or a bounded timeout elapses, then configures the interface
// in place — DHCP-leased on success, the caller's static values on timeout (fallback PRESERVED, so a
// link with no DHCP server still comes up). It is arch-neutral: the caller supplies a monotonic
// millisecond clock (which drives BOTH smoltcp's notion of time and the timeout) and the static
// fallback. x86 `smolnet` could reuse this too (a future fold — it is not wired here).

use smoltcp::iface::{Interface, SocketSet, SocketStorage};
use smoltcp::socket::dhcpv4;
use smoltcp::wire::{IpAddress, IpCidr, Ipv4Address};

/// The IPv4 configuration a [`dhcp_or_static`] bring-up settled on — either DHCP-leased or the static
/// fallback. The caller reads it to drive its own witness (e.g. ping the gateway) against whichever
/// config the interface actually took.
#[derive(Clone, Copy)]
pub struct NetConfig {
    /// `true` if these values came from a DHCP lease; `false` if the static fallback was applied.
    pub leased: bool,
    /// The configured interface address.
    pub ip: [u8; 4],
    /// The configured prefix length (e.g. 24).
    pub prefix_len: u8,
    /// The default gateway / router.
    pub gw: [u8; 4],
}

/// Apply an IPv4 address + default route to `iface` in place (replacing any prior config). Shared by
/// both the DHCP-lease and the static-fallback paths so they configure the interface identically.
fn apply_ipv4(iface: &mut Interface, ip: [u8; 4], prefix_len: u8, gw: [u8; 4]) {
    iface.update_ip_addrs(|addrs| {
        addrs.clear();
        let _ = addrs.push(IpCidr::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), prefix_len));
    });
    iface.routes_mut().remove_default_ipv4_route();
    let _ = iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(gw[0], gw[1], gw[2], gw[3]));
}

/// Run a DHCPv4 client over `iface`/`dev` until a lease is acquired or `timeout_ms` elapses, then
/// configure the interface in place and return the settled [`NetConfig`].
///
/// * On lease: applies the leased address + default route, emits
///   `<prefix> NET: DHCP lease ip=<ip>/<prefix> gw=<gw> (server <srv>) => PASS`, returns `leased=true`.
/// * On timeout: emits an honest `no lease within <n>ms — falling back to static <ip>` line, applies
///   the static config (the pre-DHCP behaviour — a DHCP-less link still comes up), returns `leased=false`.
///
/// `now_ms` is a monotonic millisecond clock supplied by the caller (each arch has its own time source);
/// it drives BOTH the smoltcp `Instant` fed to `poll` and the wall-clock timeout, so the bound is real
/// time, not iteration count. The DHCP socket's storage is entirely stack-local (no heap growth): a
/// single-slot `SocketSet` scoped to this call, dropped on return before the caller builds its own.
pub fn dhcp_or_static<D: Device>(
    prefix: &str,
    iface: &mut Interface,
    dev: &mut D,
    now_ms: &dyn Fn() -> i64,
    timeout_ms: i64,
    static_ip: [u8; 4],
    static_prefix: u8,
    static_gw: [u8; 4],
) -> NetConfig {
    let mut storage: [SocketStorage; 1] = Default::default();
    let mut sockets = SocketSet::new(&mut storage[..]);
    let handle = sockets.add(dhcpv4::Socket::new());

    serial_println!("{} NET: DHCP discover (timeout {} ms) ::", prefix, timeout_ms);
    let start = now_ms();
    // NET-4k poll-cadence witness: count how many times we drive `iface.poll` across the window. The
    // Orin RTL8168 no-lease audit put the poll/dispatch cadence under suspicion; this busy loop is the
    // exact seam the QEMU `vnet` path leases through, so surfacing the poll count on BOTH outcomes lets
    // boot-14 compare the metal cadence against the known-good virtio lease directly.
    let mut polls: u64 = 0;
    loop {
        let t = now_ms();
        iface.poll(Instant::from_millis(t), dev, &mut sockets);
        polls += 1;

        match sockets.get_mut::<dhcpv4::Socket>(handle).poll() {
            Some(dhcpv4::Event::Configured(cfg)) => {
                let ip = cfg.address.address().octets();
                let prefix_len = cfg.address.prefix_len();
                // A lease without a router is honoured, but our witnesses need a gateway; fall back to
                // the static gateway if the server offered none (rare, but keeps the route sane).
                let gw = cfg.router.map(|r| r.octets()).unwrap_or(static_gw);
                let srv = cfg.server.address.octets();
                apply_ipv4(iface, ip, prefix_len, gw);
                serial_println!(
                    "{} NET: DHCP lease ip={}.{}.{}.{}/{} gw={}.{}.{}.{} (server {}.{}.{}.{}) after {} polls => PASS ::",
                    prefix,
                    ip[0], ip[1], ip[2], ip[3], prefix_len,
                    gw[0], gw[1], gw[2], gw[3],
                    srv[0], srv[1], srv[2], srv[3],
                    polls,
                );
                return NetConfig { leased: true, ip, prefix_len, gw };
            }
            Some(dhcpv4::Event::Deconfigured) => {}
            None => {}
        }

        if now_ms().saturating_sub(start) >= timeout_ms {
            apply_ipv4(iface, static_ip, static_prefix, static_gw);
            serial_println!(
                "{} NET: no lease within {} ms ({} polls) — falling back to static {}.{}.{}.{}/{} gw {}.{}.{}.{} ::",
                prefix, timeout_ms, polls,
                static_ip[0], static_ip[1], static_ip[2], static_ip[3], static_prefix,
                static_gw[0], static_gw[1], static_gw[2], static_gw[3],
            );
            return NetConfig {
                leased: false,
                ip: static_ip,
                prefix_len: static_prefix,
                gw: static_gw,
            };
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// NET-4j — the DHCP no-lease reproducer (witness-gated, self-contained, no NIC required).
// ══════════════════════════════════════════════════════════════════════════════════════════════════
//
// Orin metal boot-11 (R23s1) captured a DHCP OFFER that passed every driver-visible check (xid match,
// unicast to our station MAC, valid yiaddr) yet never produced a lease — the drop was ABOVE the driver,
// in the smoltcp dhcpv4 socket. This reproducer replays that exchange deterministically, with NO network
// backend: it drives a real `smoltcp::socket::dhcpv4::Socket` over a fake in-memory `phy::Device` carrying
// the SAME `DeviceCapabilities` shape the metal seam uses (medium Ethernet, MTU 1500, default checksum
// caps), polls once to emit the DISCOVER, then injects a synthesized OFFER echoing that DISCOVER's xid.
//
// It asserts the integration CONTRACT the metal seam relies on:
//   * a well-formed OFFER (valid checksums, chaddr==MAC, option-54 server-identifier present) makes the
//     socket transition Discovering -> Requesting and EMIT a REQUEST — PASS;
//   * an OFFER identical but for the missing option-54 server-identifier is silently DROPPED and NO
//     REQUEST is emitted — proving that gate is exactly what turns a driver-valid OFFER into a no-lease.
//
// Because the fixed `random_seed` the seams use makes the DISCOVER's xid deterministic, the reproducer's
// xid equals the metal boot-11 xid (0x51fb1e94 under seed 0x4e455434) — it replays the real exchange, not
// an analogue. Runs on the aarch64 `vnet` QEMU gate: `UNAOS_WITNESS=1 UNAOS_VNET=1 ./arroyo test-arm`.
#[cfg(feature = "witness")]
pub mod net4j_repro {
    use super::*;
    use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
    use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
    use smoltcp::socket::dhcpv4;
    use smoltcp::time::Instant;
    use smoltcp::wire::{
        DhcpMessageType, DhcpPacket, DhcpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
        EthernetRepr, HardwareAddress, IpProtocol, Ipv4Address, Ipv4Packet, Ipv4Repr, UdpPacket,
        UdpRepr,
    };

    const OUR_MAC: [u8; 6] = [0x4c, 0xbb, 0x47, 0x25, 0x49, 0xc8];
    const SRV_MAC: [u8; 6] = [0x8a, 0x66, 0x5a, 0x72, 0x49, 0x64];
    const SRV_IP: Ipv4Address = Ipv4Address::new(192, 168, 2, 1);
    const YIADDR: Ipv4Address = Ipv4Address::new(192, 168, 2, 2);
    const CAP: usize = 600;

    /// A fake `phy::Device`: injects one queued inbound frame, captures the LAST outbound frame + its
    /// DHCP message type. No heap; all scratch is struct-local (mirrors `SmoltcpPhy`).
    struct FakeDev {
        inbound: Option<([u8; CAP], usize)>,
        rx: [u8; CAP],
        tx: [u8; CAP],
        /// Last captured outbound frame + length, written through a raw pointer from the TxToken (the
        /// token borrows `tx` disjointly, so the capture cannot alias it). Single-threaded witness path.
        cap_buf: [u8; CAP],
        cap_len: usize,
        cap_mtype: u8,
    }
    impl FakeDev {
        fn new() -> Self {
            FakeDev {
                inbound: None,
                rx: [0; CAP],
                tx: [0; CAP],
                cap_buf: [0; CAP],
                cap_len: 0,
                cap_mtype: 0,
            }
        }
    }
    struct RxTok<'a> {
        buf: &'a [u8],
    }
    struct TxTok<'a> {
        buf: &'a mut [u8],
        cap_buf: *mut [u8; CAP],
        cap_len: *mut usize,
        cap_mtype: *mut u8,
    }
    impl RxToken for RxTok<'_> {
        fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
            f(self.buf)
        }
    }
    impl TxToken for TxTok<'_> {
        fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
            let n = len.min(self.buf.len());
            let r = f(&mut self.buf[..n]);
            // SAFETY: the capture fields are distinct from `tx` (which `buf` borrows); single-threaded.
            // Form the reference by an EXPLICIT deref (never an implicit autoref through the raw pointer).
            unsafe {
                let cap: &mut [u8; CAP] = &mut *self.cap_buf;
                cap[..n].copy_from_slice(&self.buf[..n]);
                *self.cap_len = n;
                *self.cap_mtype = dhcp_mtype_of(&self.buf[..n]);
            }
            r
        }
    }
    impl Device for FakeDev {
        type RxToken<'a>
            = RxTok<'a>
        where
            Self: 'a;
        type TxToken<'a>
            = TxTok<'a>
        where
            Self: 'a;
        fn receive(&mut self, _t: Instant) -> Option<(RxTok<'_>, TxTok<'_>)> {
            let (frame, flen) = self.inbound.take()?;
            let n = flen.min(self.rx.len());
            self.rx[..n].copy_from_slice(&frame[..n]);
            let cap_buf: *mut [u8; CAP] = &mut self.cap_buf;
            let cap_len: *mut usize = &mut self.cap_len;
            let cap_mtype: *mut u8 = &mut self.cap_mtype;
            Some((
                RxTok { buf: &self.rx[..n] },
                TxTok { buf: &mut self.tx[..], cap_buf, cap_len, cap_mtype },
            ))
        }
        fn transmit(&mut self, _t: Instant) -> Option<TxTok<'_>> {
            let cap_buf: *mut [u8; CAP] = &mut self.cap_buf;
            let cap_len: *mut usize = &mut self.cap_len;
            let cap_mtype: *mut u8 = &mut self.cap_mtype;
            Some(TxTok { buf: &mut self.tx[..], cap_buf, cap_len, cap_mtype })
        }
        fn capabilities(&self) -> DeviceCapabilities {
            // The EXACT shape the metal seams advertise (default checksum caps => verify on RX).
            let mut caps = DeviceCapabilities::default();
            caps.medium = Medium::Ethernet;
            caps.max_transmission_unit = 1500;
            caps
        }
    }

    /// Parse a raw L2 frame's DHCP message type (option 53), or `0` if not a DHCP frame.
    fn dhcp_mtype_of(frame: &[u8]) -> u8 {
        let start = 42usize; // eth(14)+ip(20)+udp(8)
        if frame.len() < start + 240 {
            return 0;
        }
        let bootp = &frame[start..];
        if !(bootp[236] == 0x63 && bootp[237] == 0x82 && bootp[238] == 0x53 && bootp[239] == 0x63) {
            return 0;
        }
        let opts = &bootp[240..];
        let mut i = 0usize;
        while i < opts.len() {
            let tag = opts[i];
            if tag == 0xff {
                break;
            }
            if tag == 0x00 {
                i += 1;
                continue;
            }
            if i + 1 >= opts.len() {
                break;
            }
            let l = opts[i + 1] as usize;
            if i + 2 + l > opts.len() {
                break;
            }
            if tag == 53 && l >= 1 {
                return opts[i + 2];
            }
            i += 2 + l;
        }
        0
    }

    /// BOOTP xid of an emitted DISCOVER frame (bytes 4..8 of the BOOTP header).
    fn bootp_xid(frame: &[u8]) -> Option<u32> {
        let b = 42usize;
        if frame.len() < b + 8 {
            return None;
        }
        Some(u32::from_be_bytes([frame[b + 4], frame[b + 5], frame[b + 6], frame[b + 7]]))
    }

    /// Build the synthesized OFFER (echoing `xid`) into `out`; returns its length. `with_server_id`
    /// toggles the option-54 gate. Uses smoltcp's own wire emit so the checksums are always valid.
    fn build_offer(out: &mut [u8; CAP], xid: u32, with_server_id: bool) -> usize {
        let dhcp = DhcpRepr {
            message_type: DhcpMessageType::Offer,
            transaction_id: xid,
            secs: 0,
            client_hardware_address: EthernetAddress(OUR_MAC),
            client_ip: Ipv4Address::UNSPECIFIED,
            your_ip: YIADDR,
            server_ip: SRV_IP,
            router: Some(SRV_IP),
            subnet_mask: Some(Ipv4Address::new(255, 255, 255, 0)),
            relay_agent_ip: Ipv4Address::UNSPECIFIED,
            broadcast: false,
            requested_ip: None,
            client_identifier: None,
            server_identifier: if with_server_id { Some(SRV_IP) } else { None },
            parameter_request_list: None,
            dns_servers: None,
            max_size: None,
            lease_duration: Some(86400),
            renew_duration: None,
            rebind_duration: None,
            additional_options: &[],
        };
        let dhcp_len = dhcp.buffer_len();
        let mut dhcp_buf = [0u8; 400];
        dhcp.emit(&mut DhcpPacket::new_unchecked(&mut dhcp_buf[..dhcp_len])).unwrap();

        let udp_repr = UdpRepr { src_port: 67, dst_port: 68 };
        let ip_repr = Ipv4Repr {
            src_addr: SRV_IP,
            dst_addr: YIADDR,
            next_header: IpProtocol::Udp,
            payload_len: udp_repr.header_len() + dhcp_len,
            hop_limit: 64,
        };
        let caps = DeviceCapabilities::default().checksum;
        let eth = EthernetRepr {
            src_addr: EthernetAddress(SRV_MAC),
            dst_addr: EthernetAddress(OUR_MAC),
            ethertype: EthernetProtocol::Ipv4,
        };
        let eth_hdr = eth.buffer_len();
        let ip_total = ip_repr.buffer_len() + ip_repr.payload_len;
        let total = eth_hdr + ip_total;
        for b in out[..total].iter_mut() {
            *b = 0;
        }
        eth.emit(&mut EthernetFrame::new_unchecked(&mut out[..eth_hdr]));
        {
            let mut ipp = Ipv4Packet::new_unchecked(&mut out[eth_hdr..eth_hdr + ip_total]);
            ip_repr.emit(&mut ipp, &caps);
            let ip_hdr = ip_repr.buffer_len();
            let udp_area = &mut ipp.payload_mut()[..udp_repr.header_len() + dhcp_len];
            let mut up = UdpPacket::new_unchecked(udp_area);
            udp_repr.emit(
                &mut up,
                &SRV_IP.into(),
                &YIADDR.into(),
                dhcp_len,
                |b| b.copy_from_slice(&dhcp_buf[..dhcp_len]),
                &caps,
            );
            let _ = ip_hdr;
        }
        total
    }

    /// Drive one OFFER variant through a fresh smoltcp dhcpv4 socket over the fake device. Returns
    /// `(discover_xid, request_emitted)`.
    fn run_variant(with_server_id: bool) -> (u32, bool) {
        let mut dev = FakeDev::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(OUR_MAC)));
        config.random_seed = 0x4e45_5434; // "NET4" — the seam's seed => the metal xid.
        let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));

        let mut storage: [SocketStorage; 1] = Default::default();
        let mut sockets = SocketSet::new(&mut storage[..]);
        let _handle = sockets.add(dhcpv4::Socket::new());

        let mut t = 0i64;
        iface.poll(Instant::from_millis(t), &mut dev, &mut sockets);
        let xid = bootp_xid(&dev.cap_buf[..dev.cap_len]).unwrap_or(0);

        let mut offer = [0u8; CAP];
        let olen = build_offer(&mut offer, xid, with_server_id);
        dev.inbound = Some((offer, olen));
        dev.cap_mtype = 0;

        let mut request = false;
        for _ in 0..8 {
            t += 100;
            iface.poll(Instant::from_millis(t), &mut dev, &mut sockets);
            if dev.cap_mtype == 3 {
                request = true; // a REQUEST was emitted
            }
        }
        (xid, request)
    }

    /// Run the NET-4j reproducer and emit the witness lines. Returns `true` iff both assertions hold
    /// (well-formed OFFER -> REQUEST, server-id-less OFFER -> no REQUEST).
    pub fn run(prefix: &str) -> bool {
        let (xid, req_ok) = run_variant(true);
        let (_, req_missing) = run_variant(false);
        let pass = req_ok && !req_missing;
        serial_println!(
            "{} NET-4j reproducer: DISCOVER xid={:#010x} | well-formed OFFER => REQUEST={} | OFFER w/o server-id => REQUEST={} | {} ::",
            prefix, xid, req_ok, req_missing,
            if pass { "PASS" } else { "FAIL" }
        );
        pass
    }
}

/// Format a MAC as `xx:xx:xx:xx:xx:xx` for the boot log (no heap — a fixed stack buffer). Shared by the
/// net drivers' bring-up witnesses.
pub fn fmt_mac(mac: &[u8; 6]) -> [u8; 17] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [b':'; 17];
    for i in 0..6 {
        out[i * 3] = HEX[(mac[i] >> 4) as usize];
        out[i * 3 + 1] = HEX[(mac[i] & 0xf) as usize];
    }
    out
}
