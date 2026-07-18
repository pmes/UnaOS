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

#![cfg(any(feature = "net4", feature = "vnet", feature = "smolnet"))]

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
