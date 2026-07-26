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
    NET_LEASED.store(cfg.leased, Ordering::Relaxed);
    NET_IP_PRESENT.store(true, Ordering::Release); // publishes the two fields above
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
    loop {
        let t = now_ms();
        iface.poll(Instant::from_millis(t), dev, &mut sockets);

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
                    "{} NET: DHCP lease ip={}.{}.{}.{}/{} gw={}.{}.{}.{} (server {}.{}.{}.{}) => PASS ::",
                    prefix,
                    ip[0], ip[1], ip[2], ip[3], prefix_len,
                    gw[0], gw[1], gw[2], gw[3],
                    srv[0], srv[1], srv[2], srv[3],
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
                "{} NET: no lease within {} ms — falling back to static {}.{}.{}.{}/{} gw {}.{}.{}.{} ::",
                prefix, timeout_ms,
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
