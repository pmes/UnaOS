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

#![cfg(any(feature = "net4", feature = "vnet", feature = "smolnet", feature = "genet", feature = "net6"))] // NET6 joins the gate: the shared socket surface lives at this file's tail. LINE-NEUTRAL edit of the existing attribute — no line is added above any code, so no `panic::Location` moves and the knob-off images cannot.

use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
        classify_tx(&self.buf[..n]);
        N::transmit(&self.buf[..n]);
        r
    }
}

// ── NET-ARP-1: TX-emission count witness ─────────────────────────────────────────────────────────
//
// The boot-P7/boot-29 question was "does smoltcp's poll ever get to EMIT?" — these counters answer it
// on the wire side of the seam: every frame smoltcp hands the phy is classified as it crosses
// `TxToken::consume` (i.e. the exact moment it is handed to the NIC TX ring), so the drivers' gated
// `[netarp1] smoltcp emitted N frames (arp-reply=X dhcp=Y)` line is an emission proof, not a poll-loop
// guess. Shared by every adapter (x86 smolnet counts too; only the aarch64 binds print the line today).

static TX_TOTAL: AtomicU32 = AtomicU32::new(0);
static TX_ARP_REPLY: AtomicU32 = AtomicU32::new(0);
static TX_DHCP: AtomicU32 = AtomicU32::new(0);

/// Classify one outbound L2 frame for the NET-ARP-1 emission witness: total, ARP replies
/// (ethertype 0x0806, opcode 2) and DHCP client datagrams (IPv4/UDP 68 → 67).
fn classify_tx(frame: &[u8]) {
    TX_TOTAL.fetch_add(1, Ordering::Relaxed);
    if frame.len() < 14 {
        return;
    }
    let et = u16::from_be_bytes([frame[12], frame[13]]);
    if et == 0x0806 {
        // ARP opcode is bytes 6..8 of the ARP payload (offset 20..22 in the frame); reply = 2.
        if frame.len() >= 22 && frame[20] == 0 && frame[21] == 2 {
            TX_ARP_REPLY.fetch_add(1, Ordering::Relaxed);
        }
    } else if et == 0x0800 && frame.len() >= 14 + 20 && frame[23] == 17 {
        // IPv4/UDP: ports sit right after the IHL-sized header.
        let ihl = ((frame[14] & 0x0f) as usize) * 4;
        let udp = 14 + ihl;
        if frame.len() >= udp + 4 {
            let sp = u16::from_be_bytes([frame[udp], frame[udp + 1]]);
            let dp = u16::from_be_bytes([frame[udp + 2], frame[udp + 3]]);
            if sp == 68 && dp == 67 {
                TX_DHCP.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Snapshot the NET-ARP-1 emission counters: `(total, arp_reply, dhcp)`. Cumulative since boot.
pub fn tx_emission_counts() -> (u32, u32, u32) {
    (
        TX_TOTAL.load(Ordering::Relaxed),
        TX_ARP_REPLY.load(Ordering::Relaxed),
        TX_DHCP.load(Ordering::Relaxed),
    )
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
    /// The first DHCP-provided DNS server, if the lease carried one. `None` on the static-fallback
    /// path (no DHCP), or when a lease offered no DNS option. Callers that need a resolver use this
    /// when present and fall back to querying the gateway (`gw`) when it is `None`. Populated from
    /// smoltcp's `dhcpv4::Config::dns_servers` (first entry) where the lease is processed.
    pub dns: Option<[u8; 4]>,
}

// ── PI-UI-2: the settled-address snapshot ─────────────────────────────────────────────────────────
//
// A read-only, lock-free snapshot of the interface IPv4 the bring-up settled on, so a consumer that has
// no business reaching into a driver's private `NetService`/socket state — the GUI status strip — can
// display the address. `dhcp_or_static` (the single chokepoint EVERY arch's bring-up funnels through)
// records the settled config here; `settled_ipv4()` reads it. Plain atomics: written once per bring-up
// from the net core, read from the render core, so the strip never takes a net lock in the render path.

/// True once a bring-up has settled and recorded an address (before that the strip shows "no lease").
static NET_IP_PRESENT: AtomicBool = AtomicBool::new(false);
/// The settled IPv4, octets packed big-endian (`a<<24 | b<<16 | c<<8 | d`).
static NET_IP: AtomicU32 = AtomicU32::new(0);
/// Whether the settled address came from a DHCP lease (`true`) or the static fallback (`false`).
static NET_LEASED: AtomicBool = AtomicBool::new(false);

/// Record the settled interface config for the read-only snapshot. Called from `dhcp_or_static` on
/// each bring-up (both the lease and the static-fallback paths).
fn record_settled(cfg: &NetConfig) {
    NET_IP.store(u32::from_be_bytes(cfg.ip), Ordering::Relaxed);
    NET_LEASED.store(cfg.leased, Ordering::Relaxed); record_settled_net6(cfg); // NET6 — the prefix/gateway/DNS half of the SAME snapshot, so the shared socket stack can ADOPT this bring-up's config instead of running a second DHCP client over one wire. ⚠ LINE-NEUTRAL append (body at the file tail, `#[inline(always)]`-empty knob-off, so this statement emits nothing and no line in this file moves).
    NET_IP_PRESENT.store(true, Ordering::Release); // publishes every field above
}

/// PI-UI-2: the settled interface IPv4 and whether it was DHCP-leased (`true`) or the static fallback
/// (`false`), or `None` before any bring-up has completed. A lock-free snapshot for read-only
/// consumers such as the GUI status strip.
pub fn settled_ipv4() -> Option<([u8; 4], bool)> {
    if !NET_IP_PRESENT.load(Ordering::Acquire) {
        return None;
    }
    Some((
        NET_IP.load(Ordering::Relaxed).to_be_bytes(),
        NET_LEASED.load(Ordering::Relaxed),
    ))
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
                // Surface the first DHCP-provided DNS server (if any) so a resolver can use the real
                // nameserver instead of falling back to the gateway (NET-14/NET-16 fold).
                let dns = cfg.dns_servers.first().map(|a| a.octets());
                apply_ipv4(iface, ip, prefix_len, gw);
                serial_println!(
                    "{} NET: DHCP lease ip={}.{}.{}.{}/{} gw={}.{}.{}.{} (server {}.{}.{}.{}) after {} polls => PASS ::",
                    prefix,
                    ip[0], ip[1], ip[2], ip[3], prefix_len,
                    gw[0], gw[1], gw[2], gw[3],
                    srv[0], srv[1], srv[2], srv[3],
                    polls,
                );
                let cfg = NetConfig { leased: true, ip, prefix_len, gw, dns };
                record_settled(&cfg); // PI-UI-2: publish the settled address for the GUI status strip
                return cfg;
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
            let cfg = NetConfig {
                leased: false,
                ip: static_ip,
                prefix_len: static_prefix,
                gw: static_gw,
                dns: None,
            };
            record_settled(&cfg); // PI-UI-2: publish the static-fallback address for the GUI status strip
            return cfg;
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

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// NET6 (ROADMAP §1b SOCK-7) — the SHARED socket surface on the aarch64 smoltcp seam.
// ══════════════════════════════════════════════════════════════════════════════════════════════════
//
// ## What this is, and why it lives HERE
//
// ORIN-NET-4 and AARCH64-VNET each bring a NIC up, bind a THROWAWAY `Interface` over its rings, lease
// an address, ping once, and drop the interface on the floor. That proves the seam and leaves the
// network unusable: the shell's `ping`/`arp` fall through to the x86 `e1000` path and print
// "No network device ready", and EL0 has no socket at all. NET6 is the surface above the seam —
// ONE persistent `Interface` + `SocketSet`, the shell verbs, and the EL0 socket family — written ONCE
// and shared by every aarch64 NIC (ONE OS, Peter 2026-08-13). The arch-specific half is the DEVICE
// ADAPTER and nothing else: a driver registers a [`NicOps`] and is done.
//
// It lives at the tail of `net_phy.rs` — the file that already exists to host exactly this, the
// arch-neutral part of the smoltcp seam — for two reasons. (1) `smolnet.rs` is the x86 DEFAULT stack:
// widening it would put new lines in a file that compiles into the shipped x86 image, and
// `panic::Location` embeds the source line, so `./arroyo knoboff` would (correctly) report the x86
// knob-off image MOVED. (2) A new file is a new module in `lib.rs`, which is the same problem one
// level up. A TAIL append inside an existing module gate adds no line above any existing code, and
// every item here is `all(feature = "net6", target_arch = "aarch64")` — so on x86, and on every
// aarch64 image built without the knob, this block compiles to nothing at all.
//
// ## WHICH STACK OWNS THE LEASE (the SOCK-7 question, answered on the wire)
//
// There is exactly ONE DHCP client on the aarch64 seam: smoltcp 0.13's `dhcpv4::Socket`, driven by
// `dhcp_or_static` above, which every aarch64 bring-up funnels through. The hand-rolled `crates/net`
// DHCP engine (`net::dhcp::DhcpClient`) has NO aarch64 caller — its only call sites are in
// `drivers/e1000.rs`, whose `NET_DEVICE` registry is populated by the x86 PCI bring-up alone, so on
// aarch64 it is compiled, never constructed against a NIC, and leases nothing. That is why
// [`LEASE_OWNER`] is a constant rather than a runtime choice, and why every NET6 witness prints it:
// a reader of a boot log should not have to infer which of two stacks produced an address.
//
// NET6 does NOT run a second DHCP client. `ensure()` ADOPTS the config `dhcp_or_static` settled on
// (`settled_config()`), so the address the sockets use is the address the bring-up witness printed.
//
// ## Storage and blocking discipline (inherited from SOCK-2/3, deliberately)
//
// Everything is static / BSS: the socket-set storage, every socket's packet buffers, and the device
// RX/TX scratch (inside the `Stack` field, not on a caller's stack). No heap. Every operation is
// NON-BLOCKING and ITERATION-BOUNDED: a syscall handler runs IF-masked and cannot sleep, so a recv
// drives a bounded poll pump and returns `-EAGAIN` if nothing landed. The `STACK` lock is released
// between pump chunks so a second CPU's socket syscall is never starved, and it is NEVER held across
// a driver's own device-registry lock for longer than one ring op (the `raw_rx`/`raw_tx` discipline).

/// NET6: which stack owns the lease, as a token for the wire. See the module header. UNGATED (it is a
/// string constant; an arch that never references it emits no bytes for it) so every aarch64 bring-up
/// witness can print the owner whether or not the socket surface itself is built.
pub const LEASE_OWNER: &str = "smoltcp-dhcpv4";

/// NET6: the whole settled config of the last bring-up, or `None` before any completed. The shared
/// socket stack adopts this rather than running a DHCP client of its own (module header).
#[cfg(all(feature = "net6", target_arch = "aarch64"))]
pub fn settled_config() -> Option<NetConfig> {
    if !NET_IP_PRESENT.load(Ordering::Acquire) {
        return None;
    }
    let dns = NET_DNS_SRV.load(Ordering::Relaxed);
    Some(NetConfig {
        leased: NET_LEASED.load(Ordering::Relaxed),
        ip: NET_IP.load(Ordering::Relaxed).to_be_bytes(),
        prefix_len: NET_PREFIX.load(Ordering::Relaxed) as u8,
        gw: NET_GW.load(Ordering::Relaxed).to_be_bytes(),
        dns: if dns == 0 { None } else { Some(dns.to_be_bytes()) },
    })
}

/// NET6: the rest of the settled config, beside `NET_IP`/`NET_LEASED` above. Written by the SAME
/// `record_settled` call (folded onto its existing statements, so no line moves), read by
/// `settled_config`. Prefix length, default gateway and leased DNS server (0 = none), octets
/// packed big-endian like `NET_IP`.
#[cfg(all(feature = "net6", target_arch = "aarch64"))]
static NET_PREFIX: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "net6", target_arch = "aarch64"))]
static NET_GW: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "net6", target_arch = "aarch64"))]
static NET_DNS_SRV: AtomicU32 = AtomicU32::new(0);

/// NET6: record the three extra fields. Called from `record_settled` (a LINE-NEUTRAL fold onto its
/// existing body), `#[inline(always)]` and empty knob-off so the folded call emits nothing.
#[cfg(all(feature = "net6", target_arch = "aarch64"))]
#[inline(always)]
pub(crate) fn record_settled_net6(cfg: &NetConfig) {
    NET_PREFIX.store(cfg.prefix_len as u32, Ordering::Relaxed);
    NET_GW.store(u32::from_be_bytes(cfg.gw), Ordering::Relaxed);
    NET_DNS_SRV.store(cfg.dns.map(u32::from_be_bytes).unwrap_or(0), Ordering::Relaxed);
}
/// Knob-off twin: the folded call compiles to zero instructions.
#[cfg(not(all(feature = "net6", target_arch = "aarch64")))]
#[inline(always)]
pub(crate) fn record_settled_net6(_cfg: &NetConfig) {}

#[cfg(all(feature = "net6", target_arch = "aarch64"))]
pub mod net6 {
    //! The shared NET6 socket surface. See the block comment above this module.

    use super::{fmt_mac, RawNic, SmoltcpPhy, LEASE_OWNER};
    // `serial_println!` reaches this nested module through the crate-root textual scope the
    // `#[macro_export]` in `arch/*/serial.rs` installs — no `use` (an absolute path to a
    // macro-expanded `macro_export` macro is not nameable from inside the same crate).
    use core::sync::atomic::{AtomicI64, AtomicPtr, AtomicU32, Ordering};
    use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
    use smoltcp::phy::Device;
    use smoltcp::socket::{icmp, tcp, udp};
    use smoltcp::time::Instant;
    use smoltcp::wire::{
        EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpEndpoint,
        IpListenEndpoint, Ipv4Address,
    };

    /// The serial prefix every NET6 witness carries. SUBSYSTEM-named, never board-named (R16): the
    /// same tag is emitted by the QEMU `virt` fixture and by an Orin metal boot.
    pub const P6: &str = ":: NET6:";

    // ── The device adapter seam ───────────────────────────────────────────────────────────────────

    /// The seam an aarch64 NIC driver registers so the shared stack can move L2 frames through its
    /// rings. Function POINTERS, not methods: each driver reaches its one registered NIC through its
    /// own module-static registry behind a short-held lock — the `raw_rx`/`raw_tx` discipline, never
    /// held across a smoltcp poll.
    pub struct NicOps {
        /// Pop one raw RX Ethernet frame into `out` (recycling the descriptor), `None` if empty.
        pub rx: fn(&mut [u8]) -> Option<usize>,
        /// Transmit one raw L2 frame (smoltcp builds the full Ethernet frame).
        pub tx: fn(&[u8]),
        /// The station MAC, or `None` if the driver never registered a NIC.
        pub mac: fn() -> Option<[u8; 6]>,
        /// PHY link state, for the witness lines.
        pub link_up: fn() -> bool,
        /// A SUBSYSTEM name for the wire (`"virtio-net"`, `"rtl8168"`) — never a board name (R16).
        pub name: &'static str,
    }

    /// The registered adapter (null = none). Lock-free: published Release at bring-up, read Acquire
    /// on the datapath, so a reader that sees the pointer sees the ops behind it.
    static NIC_OPS: AtomicPtr<NicOps> = AtomicPtr::new(core::ptr::null_mut());

    /// Register the live NIC adapter — called ONCE from a driver's bring-up, after its rings are up
    /// and its own registry is populated, and before [`init`].
    pub fn register_nic(ops: &'static NicOps) {
        NIC_OPS.store(ops as *const NicOps as *mut NicOps, Ordering::Release);
    }

    /// The registered adapter, or `None` before any driver registered.
    #[inline]
    fn nic() -> Option<&'static NicOps> {
        let p = NIC_OPS.load(Ordering::Acquire);
        if p.is_null() {
            None
        } else {
            // SAFETY: `register_nic` only ever stores a `&'static NicOps`; nothing clears it.
            Some(unsafe { &*(p as *const NicOps) })
        }
    }

    /// The SUBSYSTEM name of the NIC the stack is bound over, for the wire.
    pub fn nic_name() -> &'static str {
        match nic() {
            Some(n) => n.name,
            None => "none",
        }
    }

    /// The shared [`RawNic`] the NET6 phy binds: every hop routed to whichever adapter registered.
    pub struct Net6Nic;
    impl RawNic for Net6Nic {
        fn rx_frame_raw(out: &mut [u8]) -> Option<usize> {
            match nic() {
                Some(n) => (n.rx)(out),
                None => None,
            }
        }
        fn transmit(frame: &[u8]) {
            if let Some(n) = nic() {
                (n.tx)(frame)
            }
        }
        fn mac() -> Option<[u8; 6]> {
            (nic()?.mac)()
        }
    }

    /// Monotonic milliseconds from the architectural counter. Readable at EL1 and EL2 (both NIC
    /// bring-ups already depend on CNTPCT being live), so it is the one clock every NET6 witness uses
    /// — and it is REAL time, which is why an RTT printed here is a duration and not an iteration
    /// count. `0` if CNTFRQ reads zero (no trustworthy counter), which renders as `rtt_ms=0`.
    #[inline]
    pub fn now_ms() -> i64 {
        let (cnt, frq): (u64, u64);
        unsafe {
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
        }
        if frq == 0 { 0 } else { (cnt.wrapping_mul(1_000) / frq) as i64 }
    }

    // ── Static storage for the persistent stack (BSS; no heap anywhere in this module) ────────────

    /// Concurrent sockets the persistent set holds. A slot backs EITHER a UDP or a TCP socket — the
    /// id space is shared so one handle value word names one registry row and one generation.
    pub const NSOCK: usize = 4;
    const UDP_PKTS: usize = 8;
    const UDP_BUF: usize = 1024;
    /// The largest datagram `sendto`/`recvfrom` will move (the syscall clamps to this).
    pub const UDP_MAX_PAYLOAD: usize = UDP_BUF;
    const TCP_BUF: usize = 2048;
    /// The largest chunk `send`/`recv` will move in one call.
    pub const TCP_MAX_CHUNK: usize = TCP_BUF;

    static mut SOCK_STORAGE: [SocketStorage<'static>; NSOCK] = [SocketStorage::EMPTY; NSOCK];
    static mut UDP_RX_META: [[udp::PacketMetadata; UDP_PKTS]; NSOCK] =
        [[udp::PacketMetadata::EMPTY; UDP_PKTS]; NSOCK];
    static mut UDP_RX_DATA: [[u8; UDP_BUF]; NSOCK] = [[0u8; UDP_BUF]; NSOCK];
    static mut UDP_TX_META: [[udp::PacketMetadata; UDP_PKTS]; NSOCK] =
        [[udp::PacketMetadata::EMPTY; UDP_PKTS]; NSOCK];
    static mut UDP_TX_DATA: [[u8; UDP_BUF]; NSOCK] = [[0u8; UDP_BUF]; NSOCK];
    static mut TCP_RX_DATA: [[u8; TCP_BUF]; NSOCK] = [[0u8; TCP_BUF]; NSOCK];
    static mut TCP_TX_DATA: [[u8; TCP_BUF]; NSOCK] = [[0u8; TCP_BUF]; NSOCK];

    /// Per-slot generation counter — bumped on every close, so a stale handle carrying the old
    /// `(gen, sid)` can never rebind to a first-fit-reused slot (the SOCK-3 fence, U11x discipline).
    static SOCK_GEN: [AtomicU32; NSOCK] = [const { AtomicU32::new(0) }; NSOCK];
    /// Monotonic poll clock fed to `iface.poll`, bumped per poll across ALL callers so smoltcp's
    /// neighbour/retransmit timers advance consistently. Iteration-driven (the real clock is only
    /// used for RTTs), exactly as SOCK-2's is.
    static POLL_CLOCK: AtomicI64 = AtomicI64::new(1);
    /// Next ephemeral source port for an active open.
    static EPHEMERAL: AtomicU32 = AtomicU32::new(49152);

    fn next_ephemeral() -> u16 {
        let v = EPHEMERAL.fetch_add(1, Ordering::Relaxed);
        49152u16.wrapping_add((v % 16_000) as u16)
    }

    /// Which transport a registry slot backs. A UDP handle handed to a stream syscall (or the
    /// reverse) is rejected on this tag BEFORE any typed `get_mut::<T>` — smoltcp's typed accessor
    /// PANICS on a mismatch, so the tag is a fail-closed guard, not a convenience.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Udp,
        Tcp,
    }

    /// The persistent stack singleton. Its fields — including the ~3 KiB device RX/TX scratch — live
    /// in BSS through the static below, so nothing large lands on a syscall stack.
    struct Stack {
        iface: Interface,
        sockets: SocketSet<'static>,
        dev: SmoltcpPhy<Net6Nic>,
        /// socket-id → (smoltcp handle, owning address-space id, transport). `None` = free.
        reg: [Option<(SocketHandle, u64, Kind)>; NSOCK],
    }

    static STACK: spin::Mutex<Option<Stack>> = spin::Mutex::new(None);

    /// Pump budgets, in poll iterations. Iteration- not clock-bounded (the SOCK-2 discipline): a reply
    /// on a local link lands in a handful of iterations, so these only cap how long an unreachable
    /// peer stalls the caller, and every one of them terminates by construction.
    const SEND_PUMP: i64 = 20_000;
    const RECV_PUMP: i64 = 400_000;
    const CONNECT_PUMP: i64 = 400_000;
    /// Iterations per lock hold — the lock is released between chunks so a concurrent socket syscall
    /// on another core is never starved for a whole pump.
    const CHUNK: i64 = 4_000;

    /// Build the persistent stack once (idempotent), ADOPTING the config the bring-up settled on.
    /// `false` if no adapter registered yet. Called under the `STACK` lock.
    fn ensure(guard: &mut Option<Stack>) -> bool {
        if guard.is_some() {
            return true;
        }
        let Some(mac) = Net6Nic::mac() else { return false };
        let mut dev = SmoltcpPhy::<Net6Nic>::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = 0x4e45_5436; // ASCII "NET6"
        let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));
        // ADOPT, never re-lease: `dhcp_or_static` already ran smoltcp's dhcpv4 socket over this NIC
        // and printed the address. A second client for the same MAC could land a different one, and
        // then the sockets and the wire witness would disagree about where this machine lives.
        if let Some(cfg) = super::settled_config() {
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::new(
                    IpAddress::v4(cfg.ip[0], cfg.ip[1], cfg.ip[2], cfg.ip[3]),
                    cfg.prefix_len,
                ));
            });
            iface.routes_mut().remove_default_ipv4_route();
            let _ = iface.routes_mut().add_default_ipv4_route(Ipv4Address::new(
                cfg.gw[0], cfg.gw[1], cfg.gw[2], cfg.gw[3],
            ));
        }
        // SAFETY: the storage static is borrowed `&'static mut` EXACTLY ONCE, here, under the `STACK`
        // lock with `guard` proven `None` — no aliasing. `SocketSet::new` retains the borrow for the
        // singleton's life; per-socket buffers are borrowed disjointly in `open` (a free `reg` slot
        // ⇒ its buffer set is unborrowed).
        let storage: &'static mut [SocketStorage<'static>] =
            unsafe { &mut *core::ptr::addr_of_mut!(SOCK_STORAGE) };
        *guard = Some(Stack { iface, sockets: SocketSet::new(storage), dev, reg: [None; NSOCK] });
        true
    }

    /// Build the persistent stack now (idempotent) and report the shape on the wire. Called from a
    /// driver's bring-up — a large-stack, shallow-chain context — so the one-time construction
    /// transient never lands on a ring-3 task's syscall stack. `false` if no NIC registered.
    pub fn init() -> bool {
        let ok = {
            let mut g = STACK.lock();
            ensure(&mut g)
        };
        let cfg = super::settled_config();
        match (ok, cfg) {
            (true, Some(c)) => serial_println!(
                "{} stack UP over {}: {}.{}.{}.{}/{} gw {}.{}.{}.{} dns {}.{}.{}.{} lease-owner={} [{}] sockets={} ::",
                P6,
                nic_name(),
                c.ip[0], c.ip[1], c.ip[2], c.ip[3], c.prefix_len,
                c.gw[0], c.gw[1], c.gw[2], c.gw[3],
                c.dns.unwrap_or(c.gw)[0], c.dns.unwrap_or(c.gw)[1],
                c.dns.unwrap_or(c.gw)[2], c.dns.unwrap_or(c.gw)[3],
                LEASE_OWNER,
                if c.leased { "dhcp" } else { "static" },
                NSOCK
            ),
            (true, None) => serial_println!(
                "{} stack UP over {} but NO bring-up config settled — no address, no route ::",
                P6, nic_name()
            ),
            (false, _) => serial_println!(
                "{} stack NOT up — no NIC adapter registered (the driver never called register_nic) ::",
                P6
            ),
        }
        ok
    }

    /// The address + prefix the persistent interface carries, or `None` before `init`.
    pub fn ipv4() -> Option<([u8; 4], u8)> {
        let g = STACK.lock();
        let stack = g.as_ref()?;
        match stack.iface.ip_addrs().first() {
            Some(IpCidr::Ipv4(c)) => Some((c.address().octets(), c.prefix_len())),
            _ => None,
        }
    }

    /// The default gateway the bring-up settled on (`None` before any bring-up).
    pub fn gateway() -> Option<[u8; 4]> {
        super::settled_config().map(|c| c.gw)
    }

    /// The resolver to query: the DHCP-offered nameserver when the lease carried one, else the
    /// gateway (a home router answers DNS; slirp's 10.0.2.3 arrives in the lease).
    pub fn resolver() -> Option<[u8; 4]> {
        let c = super::settled_config()?;
        Some(c.dns.unwrap_or(c.gw))
    }

    /// Drive `iters` poll iterations against the persistent interface. Split-borrows the fields so
    /// `iface.poll` gets `&mut dev` + `&mut sockets` disjointly. Reads the RX ring directly through
    /// the adapter, so it drives ARP, egress and inbound delivery with no interrupt required.
    fn pump(stack: &mut Stack, iters: i64) {
        let Stack { iface, sockets, dev, .. } = stack;
        for _ in 0..iters {
            let now = POLL_CLOCK.fetch_add(1, Ordering::Relaxed);
            iface.poll(Instant::from_millis(now), dev, sockets);
        }
    }

    // ── The shell verbs: ping / arp / dns, each with a witness line the next boot scores ──────────

    /// An inbound ARP reply for `target` carries the peer MAC that smoltcp hides behind its neighbour
    /// cache. The blocking verbs snoop the wire for it so `arp` has something to print. Mirrors the
    /// x86 `smolnet::snoop_arp` (a read-only reuse of `net::arp::learn`), spelled out here because
    /// the crate-level `net` dependency is not in scope for this module.
    fn snoop_arp(frame: &[u8], target: [u8; 4], out: &mut Option<[u8; 6]>) {
        if out.is_some() || frame.len() < 42 {
            return;
        }
        if u16::from_be_bytes([frame[12], frame[13]]) != 0x0806 {
            return; // not ARP
        }
        let a = &frame[14..42];
        // htype=1 ethernet, ptype=0x0800 IPv4, hlen=6, plen=4, oper=2 (reply)
        if a[0] != 0 || a[1] != 1 || a[2] != 0x08 || a[3] != 0 || a[4] != 6 || a[5] != 4 || a[7] != 2
        {
            return;
        }
        if a[14..18] == target[..] {
            *out = Some([a[8], a[9], a[10], a[11], a[12], a[13]]);
        }
    }

    /// The RX observer the blocking verbs bind: ARP-snoop for one target.
    struct Snoop {
        target: [u8; 4],
        mac: Option<[u8; 6]>,
    }
    impl super::RxObserver for Snoop {
        fn observe(&mut self, frame: &[u8]) {
            snoop_arp(frame, self.target, &mut self.mac)
        }
    }

    /// Outcome of a [`ping`].
    pub struct PingOutcome {
        /// Echo requests emitted and echo replies matched.
        pub sent: u16,
        pub received: u16,
        /// The peer MAC, if an ARP reply for the target crossed the wire.
        pub mac: Option<[u8; 6]>,
        /// Round-trip of the FIRST reply, in real milliseconds (`0` if none / no counter).
        pub first_rtt_ms: i64,
    }

    /// ICMP identifier stamped on every echo we originate. ASCII "N6".
    const PING_IDENT: u16 = 0x4e36;
    const PING_PAYLOAD: &[u8] = b"unaos-net6";
    /// Real-time bound on a blocking verb, in milliseconds. A verb is a SHELL command and an operator
    /// is waiting on it, so the bound is wall-clock rather than an iteration count: an unreachable
    /// target costs the operator this much and no more, on any clock speed.
    const VERB_BUDGET_MS: i64 = 2_000;

    /// Blocking ICMP ping over the persistent interface's configuration, on a THROWAWAY interface +
    /// ICMP socket (the SOCK-1 shape: a blocking op must not park a socket in the persistent set,
    /// where it would count against ring 3's `NSOCK` budget). Emits one witness line PER SEQUENCE —
    ///
    /// `:: NET6: ping 10.42.0.1 seq=1 rtt_ms=0 -> REPLY ::`
    ///
    /// — so the next boot can score reachability per packet rather than from a summary, then a
    /// closing summary line. All storage is stack-local; no heap growth.
    pub fn ping(target: [u8; 4], count: u16) -> Option<PingOutcome> {
        let mac = Net6Nic::mac()?;
        let (our_ip, plen) = ipv4()?;
        let gw = gateway()?;
        let count = count.clamp(1, 16);

        let mut dev = SmoltcpPhy::<Net6Nic, Snoop>::with_observer(Snoop { target, mac: None });
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = 0x4e36_5049; // "N6PI"
        let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(
                IpAddress::v4(our_ip[0], our_ip[1], our_ip[2], our_ip[3]),
                plen,
            ));
        });
        let _ = iface
            .routes_mut()
            .add_default_ipv4_route(Ipv4Address::new(gw[0], gw[1], gw[2], gw[3]));

        let mut rx_meta = [icmp::PacketMetadata::EMPTY; 8];
        let mut rx_payload = [0u8; 256];
        let mut tx_meta = [icmp::PacketMetadata::EMPTY; 8];
        let mut tx_payload = [0u8; 256];
        let socket = icmp::Socket::new(
            icmp::PacketBuffer::new(&mut rx_meta[..], &mut rx_payload[..]),
            icmp::PacketBuffer::new(&mut tx_meta[..], &mut tx_payload[..]),
        );
        let mut storage: [SocketStorage; 1] = Default::default();
        let mut sockets = SocketSet::new(&mut storage[..]);
        let handle = sockets.add(socket);
        if sockets
            .get_mut::<icmp::Socket>(handle)
            .bind(icmp::Endpoint::Ident(PING_IDENT))
            .is_err()
        {
            return None;
        }

        let remote = IpAddress::v4(target[0], target[1], target[2], target[3]);
        let (mut sent, mut received, mut seq) = (0u16, 0u16, 0u16);
        let mut first_rtt = 0i64;
        let mut clock = 0i64;
        let t0 = now_ms();
        // One outstanding echo at a time, so a reply's RTT belongs to a KNOWN request. `sent_at` is
        // the real-time stamp of the request in flight; `0` = nothing outstanding.
        let mut sent_at = 0i64;
        while now_ms().saturating_sub(t0) < VERB_BUDGET_MS && received < count {
            clock += 1;
            iface.poll(Instant::from_millis(clock), &mut dev, &mut sockets);
            let sock = sockets.get_mut::<icmp::Socket>(handle);
            if sent_at == 0 && seq < count && sock.can_send() {
                seq += 1;
                let repr =
                    Icmpv4Repr::EchoRequest { ident: PING_IDENT, seq_no: seq, data: PING_PAYLOAD };
                if let Ok(buf) = sock.send(repr.buffer_len(), remote) {
                    let caps = dev.capabilities().checksum;
                    repr.emit(&mut Icmpv4Packet::new_unchecked(buf), &caps);
                    sent += 1;
                    sent_at = now_ms().max(1);
                }
            }
            let sock = sockets.get_mut::<icmp::Socket>(handle);
            if sock.can_recv() {
                if let Ok((payload, _addr)) = sock.recv() {
                    if let Ok(pkt) = Icmpv4Packet::new_checked(payload) {
                        let caps = dev.capabilities().checksum;
                        if let Ok(Icmpv4Repr::EchoReply { seq_no, .. }) =
                            Icmpv4Repr::parse(&pkt, &caps)
                        {
                            let rtt = now_ms().saturating_sub(sent_at.max(t0));
                            received += 1;
                            if received == 1 {
                                first_rtt = rtt;
                            }
                            serial_println!(
                                "{} ping {}.{}.{}.{} seq={} rtt_ms={} -> REPLY ::",
                                P6, target[0], target[1], target[2], target[3], seq_no, rtt
                            );
                            sent_at = 0; // the next echo may go out
                        }
                    }
                }
            }
            // The outstanding echo timed out: say so on the wire (an ABSENCE is only evidence when
            // the producing path ran, so a lost sequence gets its own line) and let the next go.
            if sent_at != 0 && now_ms().saturating_sub(sent_at) > VERB_BUDGET_MS / count as i64 {
                serial_println!(
                    "{} ping {}.{}.{}.{} seq={} rtt_ms=- -> TIMEOUT ::",
                    P6, target[0], target[1], target[2], target[3], seq
                );
                sent_at = 0;
            }
        }
        let peer = dev.obs.mac;
        // `fmt_mac` writes ASCII hex into a fixed stack buffer; an unresolved peer renders as dashes
        // rather than being omitted, so the summary line has the same shape either way.
        let pb = match peer {
            Some(m) => fmt_mac(&m),
            None => [b'-'; 17],
        };
        serial_println!(
            "{} ping {}.{}.{}.{} {}/{} replies over {} peer {} -> {} ::",
            P6,
            target[0], target[1], target[2], target[3],
            received, sent, nic_name(),
            core::str::from_utf8(&pb).unwrap_or("<mac>"),
            if received > 0 { "REPLY" } else { "NO REPLY" }
        );
        Some(PingOutcome { sent, received, mac: peer, first_rtt_ms: first_rtt })
    }

    /// Blocking ARP resolve: one echo forces smoltcp to ARP the target; the observer returns the MAC
    /// off the wire. Emits `:: NET6: arp <ip> -> is-at <mac> ::` / `-> NO REPLY ::`.
    pub fn arp(target: [u8; 4]) -> Option<[u8; 6]> {
        let mac = Net6Nic::mac()?;
        let (our_ip, plen) = ipv4()?;
        let gw = gateway()?;
        let mut dev = SmoltcpPhy::<Net6Nic, Snoop>::with_observer(Snoop { target, mac: None });
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = 0x4e36_4152; // "N6AR"
        let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(
                IpAddress::v4(our_ip[0], our_ip[1], our_ip[2], our_ip[3]),
                plen,
            ));
        });
        let _ = iface
            .routes_mut()
            .add_default_ipv4_route(Ipv4Address::new(gw[0], gw[1], gw[2], gw[3]));
        let mut rx_meta = [icmp::PacketMetadata::EMPTY; 4];
        let mut rx_payload = [0u8; 128];
        let mut tx_meta = [icmp::PacketMetadata::EMPTY; 4];
        let mut tx_payload = [0u8; 128];
        let socket = icmp::Socket::new(
            icmp::PacketBuffer::new(&mut rx_meta[..], &mut rx_payload[..]),
            icmp::PacketBuffer::new(&mut tx_meta[..], &mut tx_payload[..]),
        );
        let mut storage: [SocketStorage; 1] = Default::default();
        let mut sockets = SocketSet::new(&mut storage[..]);
        let handle = sockets.add(socket);
        let _ = sockets
            .get_mut::<icmp::Socket>(handle)
            .bind(icmp::Endpoint::Ident(PING_IDENT));
        let remote = IpAddress::v4(target[0], target[1], target[2], target[3]);
        let mut clock = 0i64;
        let t0 = now_ms();
        let mut armed = false;
        while now_ms().saturating_sub(t0) < VERB_BUDGET_MS && dev.obs.mac.is_none() {
            clock += 1;
            iface.poll(Instant::from_millis(clock), &mut dev, &mut sockets);
            if !armed {
                let sock = sockets.get_mut::<icmp::Socket>(handle);
                if sock.can_send() {
                    let repr =
                        Icmpv4Repr::EchoRequest { ident: PING_IDENT, seq_no: 1, data: PING_PAYLOAD };
                    if let Ok(buf) = sock.send(repr.buffer_len(), remote) {
                        let caps = dev.capabilities().checksum;
                        repr.emit(&mut Icmpv4Packet::new_unchecked(buf), &caps);
                        armed = true;
                    }
                }
            }
        }
        match dev.obs.mac {
            Some(m) => {
                let b = fmt_mac(&m);
                serial_println!(
                    "{} arp {}.{}.{}.{} -> is-at {} ::",
                    P6, target[0], target[1], target[2], target[3],
                    core::str::from_utf8(&b).unwrap_or("<mac>")
                );
                Some(m)
            }
            None => {
                serial_println!(
                    "{} arp {}.{}.{}.{} -> NO REPLY ::",
                    P6, target[0], target[1], target[2], target[3]
                );
                None
            }
        }
    }

    /// Blocking DNS A-record lookup over the persistent stack's own UDP socket, through the SHARED
    /// `crate::net_dns` builder/parser (SOCK-8's arch-neutral half — no second wire format). Queries
    /// the DHCP-offered nameserver where the lease carried one, else the gateway. Emits
    /// `:: NET6: dns <host> -> A a.b.c.d (server s.s.s.s) ::` or a typed failure line.
    pub fn dns(host: &str) -> Option<[u8; 4]> {
        let Some(server) = resolver() else {
            serial_println!("{} dns {} -> NO RESOLVER (no lease, no gateway) ::", P6, host);
            return None;
        };
        let mut qbuf = [0u8; 320];
        // The transaction id is the poll clock's low half: two lookups in one boot never collide, and
        // `parse_a` REJECTS a datagram whose id does not match, so a late reply to a previous query
        // can never be read as the answer to this one.
        let txid = (POLL_CLOCK.load(Ordering::Relaxed) as u16) ^ 0x4e36;
        let Some(qlen) = crate::net_dns::build_query(&mut qbuf, txid, host) else {
            serial_println!("{} dns {} -> BAD NAME (unencodable) ::", P6, host);
            return None;
        };
        // A kernel-side lookup borrows a ring-3 socket slot for the duration and gives it straight
        // back; `open`'s owner is the SHELL's address space, which no EL0 teardown will sweep.
        let Some(sid) = open(u64::MAX, false) else {
            serial_println!("{} dns {} -> NO SOCKET (all {} slots in use) ::", P6, host, NSOCK);
            return None;
        };
        let mut answer = None;
        if bind(sid, next_ephemeral()).is_ok()
            && sendto(sid, server, crate::net_dns::DNS_PORT, &qbuf[..qlen]).is_ok()
        {
            let mut rbuf = [0u8; 512];
            if let Some((_src, _sport, n)) = recvfrom(sid, &mut rbuf) {
                match crate::net_dns::parse_a(&rbuf[..n], txid) {
                    crate::net_dns::Dns::Resolved(a) => {
                        serial_println!(
                            "{} dns {} -> A {}.{}.{}.{} (server {}.{}.{}.{}) ::",
                            P6, host, a[0], a[1], a[2], a[3],
                            server[0], server[1], server[2], server[3]
                        );
                        answer = Some(a);
                    }
                    crate::net_dns::Dns::ServerErr(rc) => serial_println!(
                        "{} dns {} -> SERVER ERROR rcode={} (server {}.{}.{}.{}) ::",
                        P6, host, rc, server[0], server[1], server[2], server[3]
                    ),
                    crate::net_dns::Dns::NoAnswer => serial_println!(
                        "{} dns {} -> NO A RECORD (server {}.{}.{}.{}) ::",
                        P6, host, server[0], server[1], server[2], server[3]
                    ),
                    crate::net_dns::Dns::Malformed => serial_println!(
                        "{} dns {} -> MALFORMED REPLY (server {}.{}.{}.{}) ::",
                        P6, host, server[0], server[1], server[2], server[3]
                    ),
                }
            } else {
                serial_println!(
                    "{} dns {} -> NO ANSWER within budget (server {}.{}.{}.{}) ::",
                    P6, host, server[0], server[1], server[2], server[3]
                );
            }
        } else {
            serial_println!("{} dns {} -> SEND FAILED (socket unusable) ::", P6, host);
        }
        close(sid);
        answer
    }

    // ── The socket registry: what the EL0 syscall family drives ───────────────────────────────────

    /// Allocate a socket owned by address space `owner`. `tcp` selects the transport. Returns the
    /// socket-id (the `reg` index), or `None` if every slot is in use / no NIC.
    pub fn open(owner: u64, tcp_sock: bool) -> Option<usize> {
        let mut g = STACK.lock();
        if !ensure(&mut g) {
            return None;
        }
        let stack = g.as_mut().unwrap();
        let sid = stack.reg.iter().position(|s| s.is_none())?;
        // SAFETY: `sid` is a FREE `reg` slot, so buffer set `sid` is borrowed by no live socket.
        // `addr_of_mut!(STATIC[sid])` names the element as a PLACE (no intermediate reference), then
        // `from_raw_parts_mut` re-forms the slice — no autoref through a raw deref. The socket OWNS
        // the borrows until it is removed in `close`, which frees the slot in the same breath.
        let handle = if tcp_sock {
            let (rx, tx): (&'static mut [u8], &'static mut [u8]) = unsafe {
                (
                    core::slice::from_raw_parts_mut(
                        core::ptr::addr_of_mut!(TCP_RX_DATA[sid]) as *mut u8,
                        TCP_BUF,
                    ),
                    core::slice::from_raw_parts_mut(
                        core::ptr::addr_of_mut!(TCP_TX_DATA[sid]) as *mut u8,
                        TCP_BUF,
                    ),
                )
            };
            stack
                .sockets
                .add(tcp::Socket::new(tcp::SocketBuffer::new(rx), tcp::SocketBuffer::new(tx)))
        } else {
            let (rm, rd, tm, td): (
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
            stack.sockets.add(udp::Socket::new(
                udp::PacketBuffer::new(rm, rd),
                udp::PacketBuffer::new(tm, td),
            ))
        };
        stack.reg[sid] = Some((handle, owner, if tcp_sock { Kind::Tcp } else { Kind::Udp }));
        Some(sid)
    }

    /// The slot's generation, for the handle value word's gen fence.
    pub fn sock_gen(sid: usize) -> u32 {
        if sid < NSOCK { SOCK_GEN[sid].load(Ordering::Acquire) } else { 0 }
    }

    /// Is `(owner, sid, generation)` still the LIVE registry row? The single staleness CHECK: a
    /// handle to a freed+reused slot, or one resolved from another address space, fails here and is
    /// never rebound (the SOCK-3/U11x fence).
    pub fn sock_valid(owner: u64, sid: usize, generation: u32) -> bool {
        if sid >= NSOCK || SOCK_GEN[sid].load(Ordering::Acquire) != generation {
            return false;
        }
        let g = STACK.lock();
        match g.as_ref().and_then(|s| s.reg.get(sid)).and_then(|s| s.as_ref()) {
            Some((_, o, _)) => *o == owner,
            None => false,
        }
    }

    /// Remove socket `sid`, free its slot and BUMP its generation so no stale handle can rebind.
    pub fn close(sid: usize) {
        let mut g = STACK.lock();
        let Some(stack) = g.as_mut() else { return };
        if let Some(Some((handle, _, _))) = stack.reg.get(sid).copied() {
            stack.sockets.remove(handle);
            stack.reg[sid] = None;
            SOCK_GEN[sid].fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Close every socket owned by `owner` — the address space's teardown hook, so a process that
    /// exits with sockets open never leaks a registry slot.
    pub fn free_owner(owner: u64) {
        let doomed: [bool; NSOCK] = {
            let g = STACK.lock();
            let mut m = [false; NSOCK];
            if let Some(stack) = g.as_ref() {
                for (i, s) in stack.reg.iter().enumerate() {
                    if let Some((_, o, _)) = s {
                        m[i] = *o == owner;
                    }
                }
            }
            m
        };
        for (i, d) in doomed.iter().enumerate() {
            if *d {
                close(i);
            }
        }
    }

    /// Look up a registry row of the expected transport. `None` = free slot or wrong kind — checked
    /// BEFORE smoltcp's typed accessor, which panics on a mismatch.
    fn row(stack: &Stack, sid: usize, want: Kind) -> Option<SocketHandle> {
        match stack.reg.get(sid).and_then(|s| s.as_ref()) {
            Some((h, _, k)) if *k == want => Some(*h),
            _ => None,
        }
    }

    /// Bind UDP socket `sid` to a local port.
    pub fn bind(sid: usize, port: u16) -> Result<(), ()> {
        if port == 0 {
            return Err(());
        }
        let mut g = STACK.lock();
        let stack = g.as_mut().ok_or(())?;
        let handle = row(stack, sid, Kind::Udp).ok_or(())?;
        stack.sockets.get_mut::<udp::Socket>(handle).bind(port).map_err(|_| ())
    }

    /// Queue `payload` to `ip:port` on UDP socket `sid`, then a short egress pump to kick ARP + TX.
    pub fn sendto(sid: usize, ip: [u8; 4], port: u16, payload: &[u8]) -> Result<usize, ()> {
        let mut g = STACK.lock();
        let stack = g.as_mut().ok_or(())?;
        let handle = row(stack, sid, Kind::Udp).ok_or(())?;
        let ep = IpEndpoint::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), port);
        {
            let sock = stack.sockets.get_mut::<udp::Socket>(handle);
            if !sock.can_send() {
                return Err(());
            }
            sock.send_slice(payload, ep).map_err(|_| ())?;
        }
        pump(stack, SEND_PUMP);
        Ok(payload.len())
    }

    /// Non-blocking receive on UDP socket `sid`: pump a bounded loop, then return the first datagram
    /// `(src_ip, src_port, len)` copied into `out`, or `None` (→ `-EAGAIN`). NEVER blocks.
    pub fn recvfrom(sid: usize, out: &mut [u8]) -> Option<([u8; 4], u16, usize)> {
        let mut spent = 0i64;
        while spent < RECV_PUMP {
            let mut g = STACK.lock();
            let stack = g.as_mut()?;
            let handle = row(stack, sid, Kind::Udp)?;
            pump(stack, CHUNK);
            let sock = stack.sockets.get_mut::<udp::Socket>(handle);
            if sock.can_recv() {
                if let Ok((data, meta)) = sock.recv() {
                    let n = data.len().min(out.len());
                    out[..n].copy_from_slice(&data[..n]);
                    let IpAddress::Ipv4(v4) = meta.endpoint.addr;
                    return Some((v4.octets(), meta.endpoint.port, n));
                }
            }
            drop(g); // release BETWEEN chunks — never spin another core for a whole pump
            spent += CHUNK;
        }
        None
    }

    /// The ring-3 poll model for an active open.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum ConnectOutcome {
        Established,
        InProgress,
        Refused,
    }

    /// The ring-3 poll model for a stream read.
    pub enum RecvOutcome {
        Data(usize),
        WouldBlock,
        Eof,
    }

    /// Active-open TCP socket `sid` to `ip:port`. NON-BLOCKING: issues the SYN if the socket is
    /// closed (a re-call while SYN-SENT just pumps), then chases the handshake within a bounded
    /// budget, releasing the lock between chunks.
    pub fn connect(sid: usize, ip: [u8; 4], port: u16) -> ConnectOutcome {
        {
            let mut g = STACK.lock();
            let Some(stack) = g.as_mut() else { return ConnectOutcome::Refused };
            let Some(handle) = row(stack, sid, Kind::Tcp) else { return ConnectOutcome::Refused };
            let local = next_ephemeral();
            let Stack { iface, sockets, .. } = stack;
            let sock = sockets.get_mut::<tcp::Socket>(handle);
            if !sock.is_open() {
                let remote = IpEndpoint::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), port);
                let le = IpListenEndpoint { addr: None, port: local };
                if sock.connect(iface.context(), remote, le).is_err() {
                    return ConnectOutcome::Refused;
                }
            }
        }
        let mut spent = 0i64;
        while spent < CONNECT_PUMP {
            let mut g = STACK.lock();
            let Some(stack) = g.as_mut() else { return ConnectOutcome::Refused };
            let Some(handle) = row(stack, sid, Kind::Tcp) else { return ConnectOutcome::Refused };
            pump(stack, CHUNK);
            let sock = stack.sockets.get_mut::<tcp::Socket>(handle);
            if sock.state() == tcp::State::Established {
                return ConnectOutcome::Established;
            }
            if !sock.is_active() {
                return ConnectOutcome::Refused; // fell out of SYN-SENT (RST / refused)
            }
            drop(g);
            spent += CHUNK;
        }
        ConnectOutcome::InProgress
    }

    /// Stream-send on TCP socket `sid`. `Ok(n)` = bytes queued; `Err(true)` = would-block (tx ring
    /// full — ring 3 retries, `-EAGAIN`); `Err(false)` = not connected / wrong kind (`-ENOTCONN`).
    pub fn send(sid: usize, data: &[u8]) -> Result<usize, bool> {
        let queued = {
            let mut g = STACK.lock();
            let stack = g.as_mut().ok_or(false)?;
            let handle = row(stack, sid, Kind::Tcp).ok_or(false)?;
            let sock = stack.sockets.get_mut::<tcp::Socket>(handle);
            if !sock.may_send() {
                return Err(false);
            }
            match sock.send_slice(data) {
                Ok(0) => return Err(true),
                Ok(n) => n,
                Err(_) => return Err(false),
            }
        };
        let mut spent = 0i64;
        while spent < SEND_PUMP {
            let mut g = STACK.lock();
            let Some(stack) = g.as_mut() else { break };
            if row(stack, sid, Kind::Tcp).is_none() {
                break;
            }
            pump(stack, CHUNK);
            drop(g);
            spent += CHUNK;
        }
        Ok(queued)
    }

    /// Non-blocking stream-recv on TCP socket `sid`.
    pub fn recv(sid: usize, out: &mut [u8]) -> RecvOutcome {
        let mut spent = 0i64;
        loop {
            {
                let mut g = STACK.lock();
                let Some(stack) = g.as_mut() else { return RecvOutcome::Eof };
                let Some(handle) = row(stack, sid, Kind::Tcp) else { return RecvOutcome::Eof };
                let sock = stack.sockets.get_mut::<tcp::Socket>(handle);
                match sock.recv_slice(out) {
                    Ok(0) => {
                        if !sock.is_open() {
                            return RecvOutcome::Eof;
                        }
                    }
                    Ok(n) => return RecvOutcome::Data(n),
                    Err(tcp::RecvError::Finished) => return RecvOutcome::Eof,
                    Err(tcp::RecvError::InvalidState) => {
                        if !sock.is_open() {
                            return RecvOutcome::Eof;
                        }
                    }
                }
                if spent >= RECV_PUMP {
                    return RecvOutcome::WouldBlock;
                }
                pump(stack, CHUNK);
            }
            spent += CHUNK;
        }
    }

    // ── The QEMU `virt` fixture: the runtime proof that this surface works ────────────────────────

    /// Drive the shared socket surface end-to-end against QEMU user-mode networking (slirp) and emit
    /// the scored witnesses. This is the RUNNABLE half of NET6: the Orin has no QEMU model, so a
    /// jetson green certifies only that this COMPILES AND LINKS — the behaviour is proven here, on
    /// `virt`, over `virtio_net.rs`, through the IDENTICAL shared code an Orin boot runs.
    ///
    /// Three legs, each its own witness line:
    ///   1. `dns`   — a UDP round-trip through the persistent set (`open`/`bind`/`sendto`/`recvfrom`,
    ///                exactly the syscall bodies) to the leased resolver.
    ///   2. `tcp`   — an active open to the slirp gateway's DNS-over-TCP port, a write and a read
    ///                (`open`/`connect`/`send`/`recv`), i.e. the stream half of the family.
    ///   3. `ping`  — ICMP to the gateway, which also prints the per-sequence RTT lines.
    ///
    /// Every leg is bounded; a silent backend makes them print a FAIL line, never hang.
    pub fn fixture() {
        serial_println!(
            "{} fixture: shared socket surface over {} (lease-owner={}) ::",
            P6, nic_name(), LEASE_OWNER
        );
        let Some(gw) = gateway() else {
            serial_println!("{} fixture -> FAIL — no bring-up config (no gateway to talk to) ::", P6);
            return;
        };
        let mut legs = 0u32;
        let mut pass = 0u32;

        // Leg 1 — UDP through the persistent set: the sys_socket/bind/sendto/recvfrom bodies, driven
        // as EL0 drives them. What it measures is the datagram ROUND-TRIP, not the answer's content:
        // an address, NXDOMAIN and SERVFAIL all prove the packet went out and came back.
        legs += 1;
        if udp_echo_leg(gw) {
            pass += 1;
        }

        // Leg 2 — TCP client: connect, send, read.
        legs += 1;
        if tcp_leg(gw) {
            pass += 1;
        }

        // Leg 3 — ICMP, with the per-sequence RTT witnesses.
        legs += 1;
        match ping(gw, 4) {
            Some(o) if o.received > 0 => pass += 1,
            _ => {}
        }

        // The `dns` VERB itself, on the same wire — the shell verb an operator types, exercised here
        // so the boot log carries its witness shape even on a headless run.
        let _ = dns("una.os");
        serial_println!(
            "{} fixture: {}/{} legs passed -> {} ::",
            P6, pass, legs,
            if pass == legs { "PASS" } else { "FAIL" }
        );
    }

    /// Fixture leg 1: a real UDP round-trip on a persistent-set socket, to the leased resolver (slirp
    /// answers DNS from 10.0.2.3:53 with no injector and no netdev change). Drives the EXACT
    /// functions `sys_socket`/`sys_bind`/`sys_sendto`/`sys_recvfrom` call.
    fn udp_echo_leg(gw: [u8; 4]) -> bool {
        let server = resolver().unwrap_or(gw);
        let Some(sid) = open(u64::MAX, false) else {
            serial_println!("{} fixture udp -> FAIL — no free socket slot ::", P6);
            return false;
        };
        let mut q = [0u8; 320];
        let txid = 0x4e36u16;
        let ok = match crate::net_dns::build_query(&mut q, txid, "una.os") {
            Some(qlen) => {
                let bound = bind(sid, 49_252).is_ok();
                let sent = bound
                    && sendto(sid, server, crate::net_dns::DNS_PORT, &q[..qlen]).is_ok();
                let mut r = [0u8; 512];
                match (sent, recvfrom(sid, &mut r)) {
                    (true, Some((src, sport, n))) => {
                        serial_println!(
                            "{} sock udp round-trip {} bytes from {}.{}.{}.{}:{} -> PASS ::",
                            P6, n, src[0], src[1], src[2], src[3], sport
                        );
                        true
                    }
                    (true, None) => {
                        serial_println!(
                            "{} sock udp round-trip to {}.{}.{}.{}:{} -> FAIL (no reply in budget) ::",
                            P6, server[0], server[1], server[2], server[3],
                            crate::net_dns::DNS_PORT
                        );
                        false
                    }
                    (false, _) => {
                        serial_println!("{} sock udp round-trip -> FAIL (bind/send refused) ::", P6);
                        false
                    }
                }
            }
            None => {
                serial_println!("{} sock udp round-trip -> FAIL (query build) ::", P6);
                false
            }
        };
        close(sid);
        ok
    }

    /// Fixture leg 2: the TCP client half — `open`/`connect`/`send`/`recv` against the slirp
    /// resolver's DNS-over-TCP port (the one inbound-capable stream service user-mode networking
    /// offers without an injector). A byte-stream round-trip is the proof; the answer's content is
    /// the resolver's business, not this leg's.
    fn tcp_leg(gw: [u8; 4]) -> bool {
        let server = resolver().unwrap_or(gw);
        let Some(sid) = open(u64::MAX, true) else {
            serial_println!("{} fixture tcp -> FAIL — no free socket slot ::", P6);
            return false;
        };
        let mut ok = false;
        match connect(sid, server, crate::net_dns::DNS_PORT) {
            ConnectOutcome::Established => {
                // DNS-over-TCP frames the query with a 2-byte big-endian length prefix.
                let mut q = [0u8; 320];
                if let Some(qlen) = crate::net_dns::build_query(&mut q[2..], 0x4e37, "una.os") {
                    q[0] = (qlen >> 8) as u8;
                    q[1] = (qlen & 0xff) as u8;
                    match send(sid, &q[..qlen + 2]) {
                        Ok(n) => {
                            let mut r = [0u8; 512];
                            match recv(sid, &mut r) {
                                RecvOutcome::Data(got) => {
                                    serial_println!(
                                        "{} sock tcp round-trip {}.{}.{}.{}:{} sent={} recv={} -> PASS ::",
                                        P6, server[0], server[1], server[2], server[3],
                                        crate::net_dns::DNS_PORT, n, got
                                    );
                                    ok = true;
                                }
                                RecvOutcome::Eof => serial_println!(
                                    "{} sock tcp round-trip -> FAIL (peer closed with no data) ::",
                                    P6
                                ),
                                RecvOutcome::WouldBlock => serial_println!(
                                    "{} sock tcp round-trip -> FAIL (no data in budget) ::",
                                    P6
                                ),
                            }
                        }
                        Err(_) => {
                            serial_println!("{} sock tcp round-trip -> FAIL (send refused) ::", P6)
                        }
                    }
                }
            }
            ConnectOutcome::InProgress => serial_println!(
                "{} sock tcp connect {}.{}.{}.{}:{} -> FAIL (still SYN-SENT at budget) ::",
                P6, server[0], server[1], server[2], server[3], crate::net_dns::DNS_PORT
            ),
            ConnectOutcome::Refused => serial_println!(
                "{} sock tcp connect {}.{}.{}.{}:{} -> FAIL (refused) ::",
                P6, server[0], server[1], server[2], server[3], crate::net_dns::DNS_PORT
            ),
        }
        close(sid);
        ok
    }
}
