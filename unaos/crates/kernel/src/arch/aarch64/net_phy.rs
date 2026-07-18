// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// NET-PHY — the shared aarch64 `smoltcp::phy::Device` adapter (`net4` / `vnet`).
//
// ## Why this module exists
//
// ORIN-NET-4 (`rtl8168_tegra.rs`) and AARCH64-VNET (`virtio_net.rs`) each bind a `smoltcp` interface
// over a NIC's RX/TX rings, and each carried a near-identical copy of the `phy::Device` / `RxToken` /
// `TxToken` boilerplate that sits between smoltcp and the driver's raw-frame accessors (the VNET
// landing flagged this as an integrator-scoped factoring). This module hosts that boilerplate ONCE,
// parameterized over a tiny [`RawNic`] trait (`transmit` / `rx_frame_raw` / `mac`) that each driver
// implements against its own device registry. ZERO behavior change: the adapter's datapath, the
// short-lock discipline, the no-alloc struct-local scratch, and the smoltcp capability shape are the
// exact code the two drivers carried — now shared rather than duplicated.
//
// ## Gating
//
// Compiled only when at least one of the two net features is on (`any(net4, vnet)`); with neither, the
// module — and the smoltcp dep both features pull — vanish. Each driver file remains additionally gated
// on its own feature, so this module compiles under net4-only, vnet-only, both, and neither.

#![cfg(any(feature = "net4", feature = "vnet"))]

use core::marker::PhantomData;

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

/// A full Ethernet frame fits (both drivers' per-descriptor buffers are 2048); the Device scratch is
/// struct-local (no heap growth). Shared by both net adapters.
pub const FRAME_CAP: usize = 1536;

/// The raw-frame seam a driver implements so the shared [`SmoltcpPhy`] can move L2 frames to/from its
/// rings. All three are associated functions (no `self`) because each driver reaches its one registered
/// NIC through a module-static registry (`NET4_DEVICE` / `VNET_DEVICE`) behind a short-held lock — the
/// e1000 `raw_rx`/`raw_tx` discipline: never hold the registry lock across a smoltcp poll.
pub trait RawNic {
    /// Pop one raw RX Ethernet frame into `out` (recycling the descriptor), or `None` if the ring is
    /// empty. Length-clamped by the driver so a misbehaving NIC cannot force an out-of-bounds slice.
    fn rx_frame_raw(out: &mut [u8]) -> Option<usize>;
    /// Transmit one raw L2 frame (smoltcp builds the full Ethernet frame).
    fn transmit(frame: &[u8]);
    /// The station MAC, or `None` if no NIC is registered.
    fn mac() -> Option<[u8; 6]>;
}

/// A `smoltcp::phy::Device` backed by a [`RawNic`]. Owns RX/TX scratch so the tokens can borrow
/// disjoint fields (smoltcp hands out both from one `receive()` to build a reply in place).
pub struct SmoltcpPhy<N: RawNic> {
    rx: [u8; FRAME_CAP],
    rlen: usize,
    tx: [u8; FRAME_CAP],
    _nic: PhantomData<N>,
}

impl<N: RawNic> SmoltcpPhy<N> {
    pub fn new() -> Self {
        SmoltcpPhy { rx: [0; FRAME_CAP], rlen: 0, tx: [0; FRAME_CAP], _nic: PhantomData }
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

impl<N: RawNic> Device for SmoltcpPhy<N> {
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
        self.rlen = len;
        let SmoltcpPhy { rx, rlen, tx, _nic } = self;
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

/// Format a MAC as `xx:xx:xx:xx:xx:xx` for the boot log (no heap — a fixed stack buffer). Shared by
/// both net drivers' bring-up witnesses.
pub fn fmt_mac(mac: &[u8; 6]) -> [u8; 17] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [b':'; 17];
    for i in 0..6 {
        out[i * 3] = HEX[(mac[i] >> 4) as usize];
        out[i * 3 + 1] = HEX[(mac[i] & 0xf) as usize];
    }
    out
}
