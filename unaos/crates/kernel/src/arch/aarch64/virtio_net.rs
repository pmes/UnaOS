// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AARCH64-VNET — a virtio-net-mmio driver on the QEMU `virt` machine + smoltcp bind (`vnet` gated).
// The QEMU-testable proof of the aarch64 smoltcp seam.
//
// ## Why this exists (relationship to NET-4)
//
// ORIN-NET-4 (`arch/aarch64/rtl8168_tegra.rs`) built the aarch64 smoltcp seam — a `phy::Device` over a
// NIC's RX/TX rings, an `Interface` + ICMP socket, the identity-map DMA invariant — but QEMU models no
// Tegra234 root complex, so that whole path is `tegra`-gated and only ever COMPILE-tested off metal (a
// `net4`/virt build prints one honest witness line and does no MMIO). AARCH64-VNET exercises the
// IDENTICAL seam shape against a device QEMU DOES model: a `virtio-net-device` on the `virt` machine's
// virtio-mmio bus, driven end-to-end with real packets over QEMU user-mode networking (slirp). It is the
// pre-metal confidence that the ring → `phy::Device` → `Interface` → ICMP-echo path is correct before
// the Orin sitting drives the real RTL8168.
//
// ## Transport: LEGACY virtio-mmio (what QEMU `virt` provides by default)
//
// QEMU's `virtio-mmio` bus defaults to `force-legacy=true`, so the `virt` machine's 32 virtio-mmio
// transports (fixed at `0x0a00_0000`, stride `0x200`, in the low-1-GiB Device window both the EL2
// firmware map and the EL1 drop cover) present the LEGACY (version 1) interface: a single `QueuePFN`
// register + `GuestPageSize`, the legacy split-virtqueue layout, and a 10-byte `virtio_net_hdr` (no
// `num_buffers` — we do NOT negotiate `MRG_RXBUF`). The driver reads the `Version` register and reports
// it; a version-2 (modern) transport is reported and skipped honestly rather than mis-driven.
//
// ## Feature negotiation (kept minimal)
//
// Of the offered device features the driver accepts ONLY `VIRTIO_NET_F_MAC` (bit 5) — enough to read the
// station MAC from config space — and nothing else: no checksum/GSO offload, no mergeable RX buffers.
// That keeps the header a fixed 10 bytes and the datapath a plain copy.
//
// ## DMA / identity-map invariant (mirrors NET-4)
//
// The `virt` EL2 firmware map (and the JC3 EL1 drop) map RAM identity (VA==PA), so — exactly as the x86
// e1000 driver relies on UEFI's 1:1 tables and NET-4 relies on `mmu_tegra`'s identity map — a heap
// allocation's virtual pointer doubles as the physical address the device DMAs against. The virtqueue
// rings + buffers are `alloc_zeroed`, page-aligned, and their identity-physical addresses are handed to
// the device (the legacy `QueuePFN = phys >> 12` with `GuestPageSize = 4096`). QEMU `virt` has no SMMU
// in the default invocation, so guest-physical IS the device's DMA target — the seam runs unmediated,
// which is what makes it a faithful QEMU proof of the ring mechanics.

#![cfg(feature = "vnet")]

use core::alloc::Layout;
use core::ptr::{read_volatile, write_volatile};

/// Stable serial prefix so the whole VNET bring-up greps as one block.
const PV: &str = ":: AARCH64 VNET:";

// ── The QEMU `virt` virtio-mmio transport window (fixed; no DTB on the GICv3 handoff) ──
/// Base of the 32 virtio-mmio transports on QEMU `virt`.
const VIRTIO_MMIO_BASE: u64 = 0x0a00_0000;
/// Per-transport stride.
const VIRTIO_MMIO_STRIDE: u64 = 0x200;
/// Transport count.
const VIRTIO_MMIO_COUNT: usize = 32;

// ── virtio-mmio register offsets (legacy + shared) ──
const R_MAGIC: u64 = 0x000; // "virt" = 0x74726976
const R_VERSION: u64 = 0x004; // 1 = legacy, 2 = modern
const R_DEVICE_ID: u64 = 0x008; // 1 = network
const R_DEVICE_FEATURES: u64 = 0x010;
const R_DEVICE_FEATURES_SEL: u64 = 0x014;
const R_DRIVER_FEATURES: u64 = 0x020;
const R_DRIVER_FEATURES_SEL: u64 = 0x024;
const R_GUEST_PAGE_SIZE: u64 = 0x028; // legacy only
const R_QUEUE_SEL: u64 = 0x030;
const R_QUEUE_NUM_MAX: u64 = 0x034;
const R_QUEUE_NUM: u64 = 0x038;
const R_QUEUE_ALIGN: u64 = 0x03c; // legacy only
const R_QUEUE_PFN: u64 = 0x040; // legacy only
const R_QUEUE_NOTIFY: u64 = 0x050;
const R_STATUS: u64 = 0x070;
const R_CONFIG: u64 = 0x100; // device config space (net: MAC at +0)

const MAGIC_VIRT: u32 = 0x7472_6976;
const DEVICE_ID_NET: u32 = 1;

// ── device status bits ──
const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FAILED: u32 = 128;

// ── virtio-net feature bits (low 32; legacy uses features-sel 0) ──
const VIRTIO_NET_F_MAC: u32 = 1 << 5;

// ── virtqueue: split-ring layout (legacy) ──
/// Queue depth (power of two, ≤ `QueueNumMax`). 16 is ample for the bounded ICMP witness.
const QUEUE_SIZE: usize = 16;
/// Legacy ring alignment (== `GuestPageSize`; the used ring starts at this boundary past the avail ring).
const QUEUE_ALIGN: usize = 4096;
/// Per-buffer size: a full 1514-byte Ethernet frame + the 10-byte virtio-net header, rounded up.
const BUF_SIZE: usize = 2048;
/// virtio-net header length with our (minimal) negotiated features — a plain `struct virtio_net_hdr`.
const NET_HDR_LEN: usize = 10;

/// The two well-known virtio-net virtqueues.
const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;

// ── virtqueue descriptor flags ── (single-buffer descriptors only, so no VIRTQ_DESC_F_NEXT chaining)
const VIRTQ_DESC_F_WRITE: u16 = 2;

/// A split virtqueue descriptor (16 bytes, device-visible via DMA).
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// A legacy split virtqueue: one page-aligned contiguous region holding the descriptor table, the
/// available ring, and (aligned past them) the used ring, plus a pool of per-descriptor DMA buffers.
/// All addresses are identity-physical (the alloc pointer == the device's DMA address).
struct VirtQueue {
    /// Base of the ring region (identity phys). `QueuePFN` = `region >> 12`.
    region: u64,
    /// Byte offset of the avail ring within `region`.
    avail_off: usize,
    /// Byte offset of the used ring within `region`.
    used_off: usize,
    /// Base of the contiguous buffer pool (identity phys); descriptor `i`'s buffer is `bufs + i*BUF_SIZE`.
    bufs: u64,
    /// Last used-ring index this driver has consumed.
    last_used: u16,
    /// Round-robin cursor for posting fresh descriptors (TX).
    next_desc: u16,
}

impl VirtQueue {
    /// Region byte size for `QUEUE_SIZE`: `align_up(desc + avail, QUEUE_ALIGN)` + used, page-rounded.
    fn region_bytes() -> (usize, usize, usize) {
        let desc = QUEUE_SIZE * core::mem::size_of::<VirtqDesc>();
        let avail = 6 + 2 * QUEUE_SIZE; // flags(2)+idx(2)+ring(2*N)+used_event(2)
        let used = 6 + 8 * QUEUE_SIZE; // flags(2)+idx(2)+ring(8*N)+avail_event(2)
        let used_off = (desc + avail + QUEUE_ALIGN - 1) & !(QUEUE_ALIGN - 1);
        (used_off, used, used_off + used)
    }

    /// Allocate the ring region + buffer pool (both zeroed, page-aligned), returning an armed queue.
    fn alloc() -> VirtQueue {
        let (used_off, _used, total) = Self::region_bytes();
        let region_bytes = (total + QUEUE_ALIGN - 1) & !(QUEUE_ALIGN - 1);
        let region_layout = Layout::from_size_align(region_bytes, QUEUE_ALIGN).unwrap();
        let buf_layout = Layout::from_size_align(QUEUE_SIZE * BUF_SIZE, 4096).unwrap();
        let region = unsafe { alloc::alloc::alloc_zeroed(region_layout) } as u64;
        let bufs = unsafe { alloc::alloc::alloc_zeroed(buf_layout) } as u64;
        VirtQueue {
            region,
            avail_off: QUEUE_SIZE * core::mem::size_of::<VirtqDesc>(),
            used_off,
            bufs,
            last_used: 0,
            next_desc: 0,
        }
    }

    #[inline]
    fn desc_ptr(&self, i: usize) -> *mut VirtqDesc {
        (self.region as *mut VirtqDesc).wrapping_add(i)
    }
    #[inline]
    fn set_desc(&self, i: usize, addr: u64, len: u32, flags: u16) {
        unsafe { write_volatile(self.desc_ptr(i), VirtqDesc { addr, len, flags, next: 0 }) };
    }
    #[inline]
    fn avail_idx(&self) -> u16 {
        unsafe { read_volatile((self.region + self.avail_off as u64 + 2) as *const u16) }
    }
    #[inline]
    fn set_avail_idx(&self, v: u16) {
        unsafe { write_volatile((self.region + self.avail_off as u64 + 2) as *mut u16, v) };
    }
    #[inline]
    fn set_avail_ring(&self, slot: usize, desc: u16) {
        let off = self.avail_off as u64 + 4 + 2 * (slot % QUEUE_SIZE) as u64;
        unsafe { write_volatile((self.region + off) as *mut u16, desc) };
    }
    #[inline]
    fn used_idx(&self) -> u16 {
        unsafe { read_volatile((self.region + self.used_off as u64 + 2) as *const u16) }
    }
    /// Read used-ring element `slot`'s `(desc_id, written_len)`.
    #[inline]
    fn used_elem(&self, slot: usize) -> (u32, u32) {
        let base = self.region + self.used_off as u64 + 4 + 8 * (slot % QUEUE_SIZE) as u64;
        let id = unsafe { read_volatile(base as *const u32) };
        let len = unsafe { read_volatile((base + 4) as *const u32) };
        (id, len)
    }
    #[inline]
    fn buf(&self, i: usize) -> u64 {
        self.bufs + (i * BUF_SIZE) as u64
    }
}

/// Data-synchronisation barrier — order ring writes before the device sees the updated index / notify
/// (and order used-ring reads after we observe the index). Mirrors the NET-4 `dsb sy` discipline.
#[inline]
fn barrier() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// A claimed virtio-net device: its MMIO transport base, station MAC, and the RX/TX virtqueues.
pub struct VirtioNet {
    mmio: u64,
    mac: [u8; 6],
    rx: VirtQueue,
    tx: VirtQueue,
    rx_count: u64,
    tx_count: u64,
}

// The driver owns raw DMA pointers; it is only ever touched behind `VNET_DEVICE`, so it is Send-sound
// on the single-core cooperative witness path (mirrors NET-4's `unsafe impl Send for Rtl8168`).
unsafe impl Send for VirtioNet {}

impl VirtioNet {
    #[inline]
    fn r32(&self, off: u64) -> u32 {
        unsafe { read_volatile((self.mmio + off) as *const u32) }
    }
    #[inline]
    fn w32(&self, off: u64, v: u32) {
        unsafe { write_volatile((self.mmio + off) as *mut u32, v) };
    }

    /// Pop one received frame (strip the 10-byte virtio-net header) into `out`, recycle the descriptor,
    /// and advance the used cursor. `None` if the RX ring is empty. Length-clamped so a misbehaving
    /// device can never make us build an out-of-bounds slice (the NET-4 `rx_frame_raw` discipline).
    fn rx_frame_raw(&mut self, out: &mut [u8]) -> Option<usize> {
        let uidx = self.rx.used_idx();
        if uidx == self.rx.last_used {
            return None;
        }
        barrier();
        let slot = self.rx.last_used as usize;
        let (id, total) = self.rx.used_elem(slot);
        let id = (id as usize) % QUEUE_SIZE;
        let total = (total as usize).min(BUF_SIZE);
        let frame_len = total.saturating_sub(NET_HDR_LEN).min(out.len());
        if frame_len > 0 {
            let src = (self.rx.buf(id) + NET_HDR_LEN as u64) as *const u8;
            unsafe { core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), frame_len) };
        }
        self.rx_count += 1;
        // Recycle: re-post this descriptor (device-writable, full buffer) into the avail ring.
        self.rx.set_desc(id, self.rx.buf(id), BUF_SIZE as u32, VIRTQ_DESC_F_WRITE);
        let a = self.rx.avail_idx();
        self.rx.set_avail_ring(a as usize, id as u16);
        barrier();
        self.rx.set_avail_idx(a.wrapping_add(1));
        barrier();
        self.w32(R_QUEUE_NOTIFY, RX_QUEUE as u32);
        self.rx.last_used = self.rx.last_used.wrapping_add(1);
        Some(frame_len)
    }

    /// Transmit one raw Ethernet frame: prepend a zeroed 10-byte virtio-net header, post a device-read
    /// descriptor, and notify the TX queue. Reclaims completed TX descriptors opportunistically so the
    /// ring never overruns on the bounded witness. Mirrors the NET-4 `transmit` head discipline.
    fn transmit(&mut self, frame: &[u8]) {
        // Opportunistic reclaim (drop any completed TX used entries; buffers are reused round-robin).
        while self.tx.used_idx() != self.tx.last_used {
            self.tx.last_used = self.tx.last_used.wrapping_add(1);
        }
        let i = self.tx.next_desc as usize % QUEUE_SIZE;
        let len = frame.len().min(BUF_SIZE - NET_HDR_LEN);
        let buf = self.tx.buf(i);
        unsafe {
            // Zeroed virtio-net header (flags=0, gso_type=NONE) then the frame.
            core::ptr::write_bytes(buf as *mut u8, 0, NET_HDR_LEN);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), (buf + NET_HDR_LEN as u64) as *mut u8, len);
        }
        self.tx.set_desc(i, buf, (NET_HDR_LEN + len) as u32, 0);
        let a = self.tx.avail_idx();
        self.tx.set_avail_ring(a as usize, i as u16);
        barrier();
        self.tx.set_avail_idx(a.wrapping_add(1));
        barrier();
        self.w32(R_QUEUE_NOTIFY, TX_QUEUE as u32);
        self.tx.next_desc = self.tx.next_desc.wrapping_add(1);
        self.tx_count += 1;
    }
}

/// Configure virtqueue `sel` on transport `mmio` (legacy PFN model): select it, size it to
/// `QUEUE_SIZE`, allocate the ring region, and publish `QueuePFN`. Returns the armed queue, or `None`
/// if the device advertises no such queue (`QueueNumMax == 0`).
fn setup_queue(mmio: u64, sel: u16) -> Option<VirtQueue> {
    unsafe {
        write_volatile((mmio + R_QUEUE_SEL) as *mut u32, sel as u32);
        let max = read_volatile((mmio + R_QUEUE_NUM_MAX) as *const u32);
        if max == 0 || (max as usize) < QUEUE_SIZE {
            return None;
        }
        let q = VirtQueue::alloc();
        write_volatile((mmio + R_QUEUE_NUM) as *mut u32, QUEUE_SIZE as u32);
        write_volatile((mmio + R_QUEUE_ALIGN) as *mut u32, QUEUE_ALIGN as u32);
        write_volatile((mmio + R_QUEUE_PFN) as *mut u32, (q.region >> 12) as u32);
        Some(q)
    }
}

/// Scan the fixed virtio-mmio window for the first live virtio-net (legacy) transport, negotiate the
/// minimal feature set, set up the RX/TX virtqueues, pre-post the RX buffers, and read the station MAC.
/// Returns the armed device, or `None` (with an honest serial line) on absence / a modern transport.
fn probe_and_init() -> Option<VirtioNet> {
    for slot in 0..VIRTIO_MMIO_COUNT {
        let mmio = VIRTIO_MMIO_BASE + slot as u64 * VIRTIO_MMIO_STRIDE;
        let magic = unsafe { read_volatile((mmio + R_MAGIC) as *const u32) };
        if magic != MAGIC_VIRT {
            continue;
        }
        let version = unsafe { read_volatile((mmio + R_VERSION) as *const u32) };
        let devid = unsafe { read_volatile((mmio + R_DEVICE_ID) as *const u32) };
        if devid != DEVICE_ID_NET {
            continue; // some other virtio device (block, rng, …) or an empty slot (devid 0)
        }
        serial_println!("{}   found virtio-net at slot {} ({:#x}) version {} ::", PV, slot, mmio, version);
        if version != 1 {
            serial_println!(
                "{}   transport is MODERN (version {}) — this driver implements the QEMU-default LEGACY interface; skipping ::",
                PV, version
            );
            return None;
        }

        let mut dev = VirtioNet {
            mmio,
            mac: [0; 6],
            rx: VirtQueue { region: 0, avail_off: 0, used_off: 0, bufs: 0, last_used: 0, next_desc: 0 },
            tx: VirtQueue { region: 0, avail_off: 0, used_off: 0, bufs: 0, last_used: 0, next_desc: 0 },
            rx_count: 0,
            tx_count: 0,
        };

        // ── Status handshake: reset → ACKNOWLEDGE → DRIVER ──
        dev.w32(R_STATUS, 0);
        dev.w32(R_STATUS, STATUS_ACKNOWLEDGE);
        dev.w32(R_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        // ── Feature negotiation (accept ONLY VIRTIO_NET_F_MAC of the low 32) ──
        dev.w32(R_DEVICE_FEATURES_SEL, 0);
        let offered = dev.r32(R_DEVICE_FEATURES);
        let accepted = offered & VIRTIO_NET_F_MAC;
        dev.w32(R_DRIVER_FEATURES_SEL, 0);
        dev.w32(R_DRIVER_FEATURES, accepted);
        serial_println!(
            "{}   device features {:#010x}; accepted {:#010x} (F_MAC={}) ::",
            PV, offered, accepted, if accepted & VIRTIO_NET_F_MAC != 0 { "yes" } else { "no" }
        );

        // ── Legacy guest page size (governs the QueuePFN shift) ──
        dev.w32(R_GUEST_PAGE_SIZE, QUEUE_ALIGN as u32);

        // ── Set up the RX + TX virtqueues ──
        let Some(rx) = setup_queue(mmio, RX_QUEUE) else {
            serial_println!("{}   RX virtqueue unavailable — bring-up FAILED ::", PV);
            dev.w32(R_STATUS, STATUS_FAILED);
            return None;
        };
        let Some(tx) = setup_queue(mmio, TX_QUEUE) else {
            serial_println!("{}   TX virtqueue unavailable — bring-up FAILED ::", PV);
            dev.w32(R_STATUS, STATUS_FAILED);
            return None;
        };
        dev.rx = rx;
        dev.tx = tx;

        // ── Pre-post every RX descriptor (device-writable, full buffer) so the device can fill them ──
        for i in 0..QUEUE_SIZE {
            dev.rx.set_desc(i, dev.rx.buf(i), BUF_SIZE as u32, VIRTQ_DESC_F_WRITE);
            dev.rx.set_avail_ring(i, i as u16);
        }
        barrier();
        dev.rx.set_avail_idx(QUEUE_SIZE as u16);
        barrier();

        // ── Read the station MAC from config space (valid because F_MAC was negotiated) ──
        for (i, b) in dev.mac.iter_mut().enumerate() {
            *b = unsafe { read_volatile((mmio + R_CONFIG + i as u64) as *const u8) };
        }

        // ── DRIVER_OK: the device is live ──
        dev.w32(R_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK);
        dev.w32(R_QUEUE_NOTIFY, RX_QUEUE as u32); // kick the RX queue so buffers are consumed
        return Some(dev);
    }
    serial_println!("{}   no virtio-net transport in the virtio-mmio window — SKIPPED (no -netdev?) ::", PV);
    None
}

/// The one registered virtio-net device (populated by [`vnet_bringup`]). Mirrors NET-4's `NET4_DEVICE`
/// / the x86 e1000 `NET_DEVICE` registry; the smoltcp Device adapter reaches the rings through it.
pub static VNET_DEVICE: spin::Mutex<Option<VirtioNet>> = spin::Mutex::new(None);

// ── Static bring-up addressing: the slirp subnet (guest is 10.0.2.15; gateway/ping target 10.0.2.2) ──
const OUR_IP: [u8; 4] = [10, 0, 2, 15];
const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];
/// A full Ethernet frame fits (BUF_SIZE is 2048); the Device scratch is struct-local.
const FRAME_CAP: usize = 1536;

// ── Raw L2 accessors over the VNET_DEVICE registry (the smoltcp Device seam; NET-4 `raw_rx`/`raw_tx`) ──
fn raw_rx(out: &mut [u8]) -> Option<usize> {
    VNET_DEVICE.lock().as_mut().and_then(|n| n.rx_frame_raw(out))
}
fn raw_tx(frame: &[u8]) {
    if let Some(n) = VNET_DEVICE.lock().as_mut() {
        n.transmit(frame);
    }
}
fn hw_addr() -> Option<[u8; 6]> {
    VNET_DEVICE.lock().as_ref().map(|n| n.mac)
}

/// Format a MAC as `xx:xx:xx:xx:xx:xx` for the boot log (no heap — a fixed stack buffer). NET-4 shape.
fn fmt_mac(mac: &[u8; 6]) -> [u8; 17] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [b':'; 17];
    for i in 0..6 {
        out[i * 3] = HEX[(mac[i] >> 4) as usize];
        out[i * 3 + 1] = HEX[(mac[i] & 0xf) as usize];
    }
    out
}

// ── The smoltcp phy::Device over the virtio-net rings (the x86 e1000/smolnet + NET-4 seam) ──

use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::icmp;
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address,
};

/// ICMP identifier stamped on the echo requests we originate. ASCII "VN".
const PING_IDENT: u16 = 0x564e;
const PING_PAYLOAD: &[u8] = b"unaos-vnet";
/// Bounded poll-pump iterations for the witness (iteration-, not wall-clock-bounded — a slirp reply on
/// the local link lands in a handful of iterations, so this only caps how long an unreachable target
/// stalls the boot). Non-hanging by construction. Mirrors smolnet's `PUMP_ITERS` spirit, smaller.
const PUMP_ITERS: i64 = 200_000;

/// A `smoltcp::phy::Device` backed by the virtio-net rings. Owns RX/TX scratch so the tokens can borrow
/// disjoint fields (smoltcp hands out both from one `receive()` to build a reply in place). NET-4 shape.
struct VirtioPhy {
    rx: [u8; FRAME_CAP],
    rlen: usize,
    tx: [u8; FRAME_CAP],
}
impl VirtioPhy {
    fn new() -> Self {
        VirtioPhy { rx: [0; FRAME_CAP], rlen: 0, tx: [0; FRAME_CAP] }
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
        raw_tx(&self.buf[..n]);
        r
    }
}
impl Device for VirtioPhy {
    type RxToken<'a> = PhyRxToken<'a>;
    type TxToken<'a> = PhyTxToken<'a>;

    fn receive(&mut self, _t: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let len = raw_rx(&mut self.rx)?;
        self.rlen = len;
        let VirtioPhy { rx, rlen, tx } = self;
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

/// Read the free-running counter (µs) for the RTT measurement. Readable at EL2 (the witness runs before
/// the JC3 EL2→EL1 drop); `busy_delay_ms`/CAPSTONE already depend on CNTPCT being live here.
#[inline]
fn now_us() -> u64 {
    let (cnt, frq): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
    }
    if frq == 0 { 0 } else { cnt.wrapping_mul(1_000_000) / frq }
}

/// Bind a smoltcp `Interface` over the virtio-net Device and drive an ICMP echo to the slirp gateway —
/// the x86 e1000/smolnet + NET-4 seam, exercised with REAL packets. Sends up to 4 echoes, pumping a
/// bounded poll loop; reports the round-trip time and a self-checking PASS/FAIL witness line. All
/// storage is stack-local (no heap growth), mirroring `smolnet::pump` / NET-4's `bind_smoltcp`.
fn bind_and_ping() {
    let Some(mac) = hw_addr() else {
        serial_println!("{}   smoltcp bind SKIPPED — no NIC registered ::", PV);
        return;
    };
    let mut dev = VirtioPhy::new();
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = 0x564e_4554; // ASCII "VNET"
    let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(
            IpAddress::v4(OUR_IP[0], OUR_IP[1], OUR_IP[2], OUR_IP[3]),
            24,
        ));
    });
    let _ = iface.routes_mut().add_default_ipv4_route(Ipv4Address::new(
        GATEWAY_IP[0], GATEWAY_IP[1], GATEWAY_IP[2], GATEWAY_IP[3],
    ));

    let mut rx_meta = [icmp::PacketMetadata::EMPTY; 8];
    let mut rx_payload = [0u8; 512];
    let mut tx_meta = [icmp::PacketMetadata::EMPTY; 8];
    let mut tx_payload = [0u8; 512];
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
        serial_println!("{}   ICMP socket bind FAILED => FAIL ::", PV);
        return;
    }

    const COUNT: u16 = 4;
    let remote = IpAddress::v4(GATEWAY_IP[0], GATEWAY_IP[1], GATEWAY_IP[2], GATEWAY_IP[3]);
    let mut sent = 0u16;
    let mut received = 0u16;
    let mut seq = 0u16;
    let mut clock: i64 = 0;
    let start = now_us();
    let mut first_reply_us = 0u64;

    while clock < PUMP_ITERS {
        clock += 1;
        iface.poll(Instant::from_millis(clock), &mut dev, &mut sockets);

        let sock = sockets.get_mut::<icmp::Socket>(handle);
        if seq < COUNT && sock.can_send() {
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
                        if received == 0 {
                            first_reply_us = now_us().saturating_sub(start);
                        }
                        received += 1;
                        if received >= COUNT {
                            break;
                        }
                    }
                }
            }
        }
    }

    let pass = received > 0;
    serial_println!(
        "{} ping {}.{}.{}.{} RTT {} us ({}/{} sent, {}/{} replies) => {} ::",
        PV,
        GATEWAY_IP[0], GATEWAY_IP[1], GATEWAY_IP[2], GATEWAY_IP[3],
        first_reply_us, sent, COUNT, received, COUNT,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// AARCH64-VNET entry point: probe the virtio-mmio window for a virtio-net transport, bring it up
/// (legacy transport, minimal features, RX/TX virtqueues, MAC), register it, then bind smoltcp and run
/// the ICMP-echo witness against the slirp gateway. Graceful (an honest skip line) if no `-netdev` is
/// attached (knob-on but no QEMU net device) or the transport is modern. Called on the QEMU `virt`
/// GICv3 path before the EL2→EL1 drop (heap up, virtio-mmio in the mapped Device window).
pub fn vnet_bringup() {
    serial_println!("{} virtio-net-mmio bring-up (QEMU virt, legacy transport) ::", PV);
    let Some(dev) = probe_and_init() else {
        return;
    };
    let mac = dev.mac;
    let macs = fmt_mac(&mac);
    serial_println!(
        "{}   virtio-net up: station MAC {}, RX/TX virtqueues armed ({} desc each) ::",
        PV,
        core::str::from_utf8(&macs).unwrap_or("<mac>"),
        QUEUE_SIZE
    );
    *VNET_DEVICE.lock() = Some(dev);
    bind_and_ping();
    serial_println!("{} AARCH64-VNET DONE — virtio-net driver up + smoltcp bound ::", PV);
}
