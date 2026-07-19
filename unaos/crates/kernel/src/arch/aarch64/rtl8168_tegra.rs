// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-NET-4 — Realtek RTL8168/8111 GbE driver + smoltcp bind (`net4` gated). The Orin's FIRST
// network path.
//
// ## Ground truth (the metal record of NET-1/2/3 — NOT re-litigated here)
//
// The Jetson Orin Nano devkit's NIC sits behind Tegra234 PCIe controller 0 (`/bus@0/pcie@140a0000`,
// domain 8). NET-3 widened the tegra TCR PS 36→40-bit, enabled controller-0's LTSSM (link UP gen1
// x1), reached the ECAM (`0x2e_2000_0000`, ~184 GiB) through the widened regime, enumerated
// bus1:dev0:fn0 = **Realtek RTL8168/8111, 0x10ec:0x8168**, and sized its BARs: BAR0 I/O 0x100,
// BAR2 mem 0x1000 (the 4 KiB register window — the driver's MMIO), BAR4 mem 0x4000 (MSI-X). NET-4 is
// the driver that stands on that: claim the device, map BAR2, drive the datasheet C+ command /
// descriptor-ring programming model, read the station MAC, and bind smoltcp over the rings.
//
// ## Code-complete-prior-to-metal (by design)
//
// QEMU models no Tegra234 root complex, so the whole driver is `tegra`-gated at the MMIO/DMA layer:
// a `net4`-standalone (virt) build performs no MMIO and prints a single honest witness line
// (`net4_witness_virt`); only `UNAOS_NET4=1 UNAOS_TEGRA=1` on real Orin silicon exercises the rings.
// Correctness comes from `arroyo check`, the QEMU regression non-regression (the tegra code is
// compiled out on virt), unit-testable descriptor logic, and faithful adherence to the RTL8168
// programming model (Realtek datasheet + Linux `drivers/net/ethernet/realtek/r8169_main.c`).
//
// ## DMA / identity-map invariant (and its honest metal risk)
//
// `mmu_tegra` builds an IDENTITY map (VA==PA) for RAM, so — exactly as the x86 e1000 driver relies on
// UEFI's 1:1 tables — a heap allocation's virtual pointer doubles as the physical address the NIC
// DMAs against. The one metal-pending unknown this cannot settle in QEMU: whether the SMMU
// (`smmu_tegra`) is translating (or bypassing) controller-0's PCIe stream IDs. NET-4 programs the
// rings with the identity-physical addresses and documents the SMMU-bypass assumption; an attended
// sitting confirms it (see arch_arm64.md §ORIN-NET-4).
//
// The SECOND metal-pending unknown (review-lens fold): CACHE COHERENCY. Rings and buffers live in
// Normal cacheable RAM and are handed over with `dsb sy` only — `dsb` orders visibility for
// COHERENT observers, it cleans/invalidates nothing. The x86 e1000 seam gets coherent DMA from the
// architecture; aarch64 does not promise it. Correctness therefore assumes Tegra234 controller-0
// is I/O-coherent (ACE-lite) toward DRAM. If metal shows stale descriptors/payloads (rings never
// advance, or torn/zero frames, on a live link), the fix is clean-before-OWN / invalidate-before-
// read on rings + buffers — do NOT weaken the OWN protocol to compensate.
//
// ## Write discipline
//
// The driver, being a driver, DOES the fabric writes NET-3 refused: it enables the device's
// MEM-decode + bus-master (command register), and programs the RTL8168 control/ring registers. Every
// config-space write is announced on serial before issue. It touches ONLY controller-0's downstream
// device (bus1:dev0:fn0) and that device's own register BAR — no other controller, no MSI/MSI-X, no
// other config function.

#![cfg(feature = "net4")]

/// Stable serial prefix so the operator (and `mbench`) can grep the whole NET-4 bring-up as one block.
/// (Used by both the witness half below and — via `use super::P4` — the tegra `metal` driver.)
const P4: &str = ":: PCIE4:";

// ── The witness half (virt / non-tegra build): one honest line, zero MMIO ──────────────────────────

/// The QEMU-safe witness: on a `net4`-but-not-`tegra` build (the only PCIe surface QEMU offers is the
/// virt generic ecam — no Tegra234 RC), there is no device to claim, so print a single line recording
/// that the driver is compiled-present but its bring-up is metal-only, and return. This keeps the
/// GICv3 virt regression runs unperturbed (no MMIO, no ring alloc). Mirrors the `census2` graceful skip.
#[cfg(not(feature = "tegra"))]
pub fn net4_bringup(_dtb_addr: u64, _dtb_size: usize, _ram_gib_mask: u64) {
    serial_println!(
        "{} ORIN-NET-4 RTL8168 driver compiled; no Tegra234 RC on this build (QEMU virt) — bring-up is metal-only (UNAOS_NET4=1 UNAOS_TEGRA=1) ::",
        P4
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// The metal driver (`net4` + `tegra`) — device claim, BAR map, MAC read (M1); rings (M2); bind (M3).
// ══════════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "tegra")]
pub use metal::net4_bringup;

#[cfg(feature = "tegra")]
mod metal {
    use super::P4;
    use crate::arch::aarch64::fdt_tegra::Fdt;
    use crate::arch::aarch64::mmu_tegra::{map_mmio_window, MmioMap};
    use crate::net_phy::{fmt_mac, RawNic, SmoltcpPhy};
    use core::alloc::Layout;
    use core::ptr::{read_volatile, write_volatile};

    /// The Realtek vendor id and the RTL8168/8111 device id the NET-3 metal enumeration found.
    const REALTEK_VENDOR: u16 = 0x10ec;
    const RTL8168_DEVICE: u16 = 0x8168;

    /// Poison patterns that mean ABSENT DECODE, never "present" (the PI-V3D-1 false-PASS lesson, shared
    /// with the NET-1/2/3 recon): `0xffffffff` = master-abort / unclaimed config; `0xdeadbeef` =
    /// firmware register/DRAM fill; `0xa5a5a5a5` = the Tegra CARVEOUT poison fill the NET-4 M1 metal
    /// FAULT left behind (a raw PCIe BAR value deref'd as a CPU PA into a protected carveout — the exact
    /// class this fix-forward closes; see the outbound-iATU block below). A live register read is none.
    #[inline]
    fn is_poison(v: u32) -> bool {
        v == 0xffff_ffff || v == 0xdead_beef || v == 0xa5a5_a5a5
    }

    // ── RTL8168/8111 register offsets (bytes from the BAR2 MMIO window) ──
    /// IDR0..5: the six station-MAC bytes (offsets 0x00..0x05).
    const REG_IDR0: u64 = 0x00;
    /// TNPDS: Transmit Normal-Priority Descriptor Start Address (64-bit; low @ 0x20, high @ 0x24).
    /// The ring base MUST be 256-byte aligned.
    const REG_TNPDS: u64 = 0x20;
    /// ChipCmd (CR): RST (soft reset, self-clearing), RxEnb (RE), TxEnb (TE).
    const REG_CR: u64 = 0x37;
    const CR_RST: u8 = 1 << 4;
    const CR_RE: u8 = 1 << 3;
    const CR_TE: u8 = 1 << 2;
    /// TPPoll: kick the normal-priority TX queue (NPQ) after posting a descriptor.
    const REG_TPPOLL: u64 = 0x38;
    const TPPOLL_NPQ: u8 = 1 << 6;
    /// PHYstatus (8-bit): LinkSts (bit 1) — 1 = link up.
    const REG_PHYSTATUS: u64 = 0x6c;
    const PHYSTATUS_LINKSTS: u8 = 1 << 1;
    /// IMR / ISR: interrupt Mask / Status (16-bit). Polled bring-up ⇒ IMR = 0, ISR write-1-to-clear.
    const REG_IMR: u64 = 0x3c;
    const REG_ISR: u64 = 0x3e;
    /// TCR / RCR: Transmit / Receive Configuration (32-bit).
    const REG_TCR: u64 = 0x40;
    const REG_RCR: u64 = 0x44;
    /// CFG9346 (93C46 command): 0xC0 unlocks the config/registers for write, 0x00 re-locks.
    const REG_CFG9346: u64 = 0x50;
    const CFG9346_UNLOCK: u8 = 0xc0;
    const CFG9346_LOCK: u8 = 0x00;
    /// RMS: Receive packet Max Size (16-bit) — the largest frame the NIC will DMA into a buffer.
    const REG_RMS: u64 = 0xda;
    /// CPlusCmd: the C+ command register (enables the C+ descriptor-ring receive/transmit engine).
    const REG_CPLUSCMD: u64 = 0xe0;
    /// RDSAR: Receive Descriptor Start Address (64-bit; low @ 0xE4, high @ 0xE8). 256-byte aligned.
    const REG_RDSAR: u64 = 0xe4;
    /// MTPS: Max Transmit Packet Size (8-bit, units of 128 bytes).
    const REG_MTPS: u64 = 0xec;

    // ── RCR / TCR field values (datasheet-standard bring-up) ──
    /// RCR: accept-all-packets (promiscuous, for bring-up — mirrors the e1000 driver's promiscuous
    /// bring-up), physical-match, multicast, broadcast; MXDMA unlimited; RX FIFO threshold none.
    const RCR_AAP: u32 = 1 << 0;
    const RCR_APM: u32 = 1 << 1;
    const RCR_AM: u32 = 1 << 2;
    const RCR_AB: u32 = 1 << 3;
    const RCR_MXDMA_UNLIMITED: u32 = 0x7 << 8;
    const RCR_RXFTH_NONE: u32 = 0x7 << 13;
    /// TCR: MXDMA unlimited + the standard IEEE inter-frame gap.
    const TCR_MXDMA_UNLIMITED: u32 = 0x7 << 8;
    const TCR_IFG_STD: u32 = 0x3 << 24;
    /// MTPS ~ 7.5 KiB (0x3B × 128) — well above a 1522-byte frame; matches the r8169 default.
    const MTPS_DEFAULT: u8 = 0x3b;

    // ── C+ descriptor (16 bytes). Written by hardware via DMA, so every access is a whole-struct
    //    volatile read/write on a `packed` copy (a field reference would be unaligned UB). ──
    #[repr(C, packed)]
    #[derive(Clone, Copy)]
    struct Desc {
        /// OWN(31) | EOR(30) | FS(29) | LS(28) | … | frame-length[13:0]. For RX the length field is
        /// the buffer size we advertise; hardware overwrites it with the received length on completion.
        opts1: u32,
        /// VLAN / offload flags — unused in this bring-up (0).
        opts2: u32,
        /// Buffer physical address (identity map ⇒ the allocation's virtual pointer). 64-bit.
        addr: u64,
    }
    /// OWN: 1 = owned by the NIC (RX: ready to receive / TX: ready to send); 0 = owned by the host.
    const DESC_OWN: u32 = 1 << 31;
    /// EOR: End Of Ring — set on the last descriptor so the NIC wraps to descriptor 0.
    const DESC_EOR: u32 = 1 << 30;
    /// FS / LS: First / Last Segment (a single-buffer frame sets both). TX only.
    const DESC_FS: u32 = 1 << 29;
    const DESC_LS: u32 = 1 << 28;
    /// Frame-length / buffer-size field, bits [13:0].
    const DESC_LEN_MASK: u32 = 0x3fff;

    /// RX ring depth (each descriptor 16 bytes; the ring base is 256-byte aligned). 32 mirrors the
    /// e1000 driver's depth.
    const NUM_RX: usize = 32;
    /// TX ring depth.
    const NUM_TX: usize = 8;
    /// Per-descriptor buffer size (one full Ethernet frame fits; fits the 14-bit length field).
    const RX_BUF_SIZE: usize = 2048;
    const TX_BUF_SIZE: usize = 2048;

    // ── NET-4d: RX-window frame classification (the DHCP no-lease RX-side proof) ──
    // Bounds the per-frame serial noise: a full L2/L3/L4 line for the first NET4D_FULL_LINES popped
    // frames, ALWAYS a full line for any BOOTP/DHCP (UDP port 67/68) frame, and a per-category tally
    // for a single window-close summary. Reads frame bytes only; writes no device register.
    const NET4D_FULL_LINES: u64 = 8;
    const RXCAT_N: usize = 6;
    const RXCAT_ARP: usize = 0;
    const RXCAT_DHCP: usize = 1;
    const RXCAT_UDP_OTHER: usize = 2;
    const RXCAT_IPV4_OTHER: usize = 3;
    const RXCAT_IPV6: usize = 4;
    const RXCAT_OTHER: usize = 5;

    /// A decoded IPv4/UDP/BOOTP view of a frame — the fields the DHCP no-lease investigation needs.
    #[derive(Clone, Copy)]
    struct DhcpInfo {
        sport: u16,
        dport: u16,
        sip: [u8; 4],
        dip: [u8; 4],
        /// BOOTP op (1 = BOOTREQUEST from client, 2 = BOOTREPLY from server).
        op: u8,
        /// DHCP message type (option 53): 1 DISCOVER, 2 OFFER, 3 REQUEST, 5 ACK, 6 NAK, … 0 = none.
        mtype: u8,
        /// BOOTP transaction id — the DISCOVER/OFFER correlation the no-lease proof turns on.
        xid: u32,
        /// "your" IP address the server offers (BOOTP yiaddr).
        yiaddr: [u8; 4],
    }

    /// The human name of a DHCP message type (option 53) for the classification lines.
    fn dhcp_mtype_name(t: u8) -> &'static str {
        match t {
            1 => "DISCOVER",
            2 => "OFFER",
            3 => "REQUEST",
            4 => "DECLINE",
            5 => "ACK",
            6 => "NAK",
            7 => "RELEASE",
            8 => "INFORM",
            _ => "none/?",
        }
    }

    /// Decode `frame` as Ethernet/IPv4/UDP/BOOTP, returning the [`DhcpInfo`] view iff it is a UDP frame
    /// on the BOOTP port pair (67/68). READ-ONLY and fully bounds-checked at every step — a malformed or
    /// truncated frame returns `None`, never a panic or an out-of-bounds read. Shared by the TX-time
    /// DISCOVER-xid capture and the RX-window classifier.
    fn decode_dhcp(frame: &[u8]) -> Option<DhcpInfo> {
        if frame.len() < 14 {
            return None;
        }
        // EtherType must be IPv4 (0x0800).
        if u16::from_be_bytes([frame[12], frame[13]]) != 0x0800 {
            return None;
        }
        let ip = frame.get(14..)?;
        if ip.len() < 20 || (ip[0] >> 4) != 4 {
            return None;
        }
        let ihl = ((ip[0] & 0x0f) as usize) * 4;
        if ihl < 20 || ip.len() < ihl + 8 {
            return None;
        }
        // Protocol 17 = UDP.
        if ip[9] != 17 {
            return None;
        }
        let sip = [ip[12], ip[13], ip[14], ip[15]];
        let dip = [ip[16], ip[17], ip[18], ip[19]];
        let udp = &ip[ihl..];
        let sport = u16::from_be_bytes([udp[0], udp[1]]);
        let dport = u16::from_be_bytes([udp[2], udp[3]]);
        if !matches!(sport, 67 | 68) && !matches!(dport, 67 | 68) {
            return None;
        }
        // BOOTP fixed header (236 bytes) + the 4-byte DHCP magic cookie.
        let bootp = udp.get(8..)?;
        if bootp.len() < 240 {
            return None;
        }
        let op = bootp[0];
        let xid = u32::from_be_bytes([bootp[4], bootp[5], bootp[6], bootp[7]]);
        let yiaddr = [bootp[16], bootp[17], bootp[18], bootp[19]];
        // DHCP message type (option 53) — only if the magic cookie is present and the options parse.
        let mut mtype = 0u8;
        if bootp[236] == 0x63 && bootp[237] == 0x82 && bootp[238] == 0x53 && bootp[239] == 0x63 {
            let opts = &bootp[240..];
            let mut i = 0usize;
            while i < opts.len() {
                let tag = opts[i];
                if tag == 0xff {
                    break; // End option.
                }
                if tag == 0x00 {
                    i += 1; // Pad option (no length byte).
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
                    mtype = opts[i + 2];
                }
                i += 2 + l;
            }
        }
        Some(DhcpInfo { sport, dport, sip, dip, op, mtype, xid, yiaddr })
    }

    // ── PCI config-space offsets (in the ECAM, at bus1:dev0:fn0) ──
    const CFG_VENDOR: u64 = 0x00;
    const CFG_COMMAND: u64 = 0x04;
    const CFG_BAR2: u64 = 0x18;
    const CFG_BAR3: u64 = 0x1c;
    /// Command register bits the driver sets so the device's BARs decode and it can master DMA.
    const CMD_MEM_SPACE: u16 = 1 << 1;
    const CMD_BUS_MASTER: u16 = 1 << 2;

    /// Downstream device config base = ECAM base + bus1:dev0:fn0 offset (`bus<<20 | dev<<15 | fn<<12`).
    const BUS1_DEV0_FN0: u64 = 1 << 20;

    /// The claimed NIC: its register-BAR MMIO base, the station MAC, and the C+ RX/TX descriptor
    /// rings + DMA buffers. The rings are allocated from the kernel heap (identity map ⇒ the pointer
    /// doubles as the physical address the NIC DMAs against, exactly like the x86 e1000 driver).
    pub struct Rtl8168 {
        mmio_base: u64,
        mac: [u8; 6],
        rx_ring: *mut Desc,
        rx_buffers: *mut u8,
        rx_cur: usize,
        rx_count: u64,
        tx_ring: *mut Desc,
        tx_buffers: *mut u8,
        tx_cur: usize,
        tx_count: u64,
        /// NET-4c: TX descriptors the NIC never handed back (OWN stayed set) — the
        /// did-a-frame-ever-LEAVE-the-NIC counter for the DHCP no-lease investigation.
        tx_stalled: u64,
        /// NET-4d: the DHCP DISCOVER's BOOTP transaction id, captured at TX time, so RX-side DHCP
        /// frames can be matched/mismatched against it explicitly. `None` until the DISCOVER is sent.
        d_xid: Option<u32>,
        /// NET-4d: RX frames given a full classification line so far — bounds the per-frame noise to
        /// the first `NET4D_FULL_LINES` popped frames (BOOTP/DHCP frames always print regardless).
        rxcls_full: u64,
        /// NET-4d: per-category RX frame tallies for the single window-close summary.
        rxcat: [u64; RXCAT_N],
        /// NET-4d: classification is live only across the DHCP discover window; closed at window end
        /// so the post-window bounded ICMP poll does not re-classify.
        rxcls_active: bool,
    }

    // The driver owns raw DMA pointers; on the single-CPU main-loop/poll discipline it is only ever
    // touched behind the `NET4_DEVICE` mutex, so sharing across contexts is sound.
    unsafe impl Send for Rtl8168 {}

    impl Rtl8168 {
        #[inline]
        fn r8(&self, off: u64) -> u8 {
            unsafe { read_volatile((self.mmio_base + off) as *const u8) }
        }
        #[inline]
        fn w8(&self, off: u64, v: u8) {
            unsafe { write_volatile((self.mmio_base + off) as *mut u8, v) }
        }
        #[inline]
        fn w16(&self, off: u64, v: u16) {
            unsafe { write_volatile((self.mmio_base + off) as *mut u16, v) }
        }
        #[inline]
        fn r32(&self, off: u64) -> u32 {
            unsafe { read_volatile((self.mmio_base + off) as *const u32) }
        }
        #[inline]
        fn w32(&self, off: u64, v: u32) {
            unsafe { write_volatile((self.mmio_base + off) as *mut u32, v) }
        }
        #[inline]
        fn r16(&self, off: u64) -> u16 {
            unsafe { read_volatile((self.mmio_base + off) as *const u16) }
        }

        /// Soft-reset the MAC: set CR.RST and poll (finite backstop) until the controller clears it.
        /// Returns true if the reset completed. Announced before the write (it is a register write).
        fn soft_reset(&self) -> bool {
            serial_println!("{}   >>> REG WRITE (M1): CR[{:#x}] |= RST (soft reset) — issuing ::", P4, REG_CR);
            self.w8(REG_CR, CR_RST);
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
            // RST self-clears when the reset completes; ~1M spins is a generous ceiling (sub-ms on HW).
            const MAX_SPINS: u32 = 1_000_000;
            let mut spins = 0u32;
            while spins < MAX_SPINS {
                if self.r8(REG_CR) & CR_RST == 0 {
                    serial_println!("{}   CR.RST cleared after {} spins — reset complete ::", P4, spins);
                    return true;
                }
                core::hint::spin_loop();
                spins += 1;
            }
            serial_println!("{}   CR.RST STILL set after {} spins — reset did not complete (honest HW result) ::", P4, MAX_SPINS);
            false
        }

        /// Poison-honest liveness probe through the (freshly mapped) register window, done BEFORE any
        /// write — the M3 guard transposed from the V3D-2 lesson: every new MMIO window gets a probe
        /// read before its first write. Reads TCR (0x40), whose chip-version bits ([30:23]) are a
        /// stable RO datum a live RTL8168 always returns (`r8169` reads exactly this to identify the
        /// MAC), and rejects the poison fills — open-bus `0xffffffff`, firmware `0xdeadbeef`, and the
        /// carveout `0xa5a5a5a5` that the M1 metal FAULT left. Returns the value on a live decode, or
        /// `None` on absent decode (so the caller REFUSES rather than issuing the first register write
        /// blind — the fault-at-first-write can never recur). This read is safe: it targets the CPU
        /// aperture the outbound iATU forwards to PCIe, so a mistranslation/link-down returns UR
        /// (all-ones), never a carveout — unlike the raw-BAR deref this fix retired.
        fn probe_alive(&self) -> Option<u32> {
            let tcr = self.r32(REG_TCR);
            if is_poison(tcr) {
                None
            } else {
                Some(tcr)
            }
        }

        /// Read the six station-MAC bytes from IDR0..5 (the RTL8168 loads them from its EEPROM/eFuse at
        /// reset). Reads are byte-wide (the ID registers are a byte array).
        fn read_mac(&self) -> [u8; 6] {
            let mut mac = [0u8; 6];
            for (i, b) in mac.iter_mut().enumerate() {
                *b = self.r8(REG_IDR0 + i as u64);
            }
            mac
        }

        /// Allocate the RX descriptor ring (256-byte aligned) + contiguous packet buffers, and point
        /// each descriptor at its buffer with OWN set (ready for the NIC to fill) and the buffer size
        /// in the length field. The `alloc_zeroed` pointer doubles as the physical address (identity
        /// map), matching the x86 e1000 ring allocation. EOR marks the last descriptor.
        fn alloc_rx(&mut self) {
            let ring_layout = Layout::from_size_align(NUM_RX * core::mem::size_of::<Desc>(), 256).unwrap();
            let buf_layout = Layout::from_size_align(NUM_RX * RX_BUF_SIZE, 4096).unwrap();
            self.rx_ring = unsafe { alloc::alloc::alloc_zeroed(ring_layout) as *mut Desc };
            self.rx_buffers = unsafe { alloc::alloc::alloc_zeroed(buf_layout) };
            for i in 0..NUM_RX {
                let buf_phys = (self.rx_buffers as u64) + (i * RX_BUF_SIZE) as u64;
                let eor = if i == NUM_RX - 1 { DESC_EOR } else { 0 };
                let d = Desc {
                    opts1: DESC_OWN | eor | (RX_BUF_SIZE as u32 & DESC_LEN_MASK),
                    opts2: 0,
                    addr: buf_phys,
                };
                unsafe { write_volatile(self.rx_ring.add(i), d) };
            }
        }

        /// Allocate the TX descriptor ring (256-byte aligned) + frame buffers. Descriptors start
        /// host-owned (OWN clear) so `transmit` can post into them; EOR marks the last descriptor.
        fn alloc_tx(&mut self) {
            let ring_layout = Layout::from_size_align(NUM_TX * core::mem::size_of::<Desc>(), 256).unwrap();
            let buf_layout = Layout::from_size_align(NUM_TX * TX_BUF_SIZE, 4096).unwrap();
            self.tx_ring = unsafe { alloc::alloc::alloc_zeroed(ring_layout) as *mut Desc };
            self.tx_buffers = unsafe { alloc::alloc::alloc_zeroed(buf_layout) };
            for i in 0..NUM_TX {
                let eor = if i == NUM_TX - 1 { DESC_EOR } else { 0 };
                let d = Desc { opts1: eor, opts2: 0, addr: 0 };
                unsafe { write_volatile(self.tx_ring.add(i), d) };
            }
        }

        /// Bring up the C+ descriptor-ring engine after the M1 soft reset: unlock the config
        /// registers, allocate + program the RX/TX rings, set the packet-size / DMA-burst / RX-filter
        /// configuration, enable RX+TX, re-lock the config, and mask interrupts (polled bring-up).
        /// The register-write ORDER follows the RTL8168 programming guide / Linux `r8169` `rtl_hw_start`.
        /// Returns true if a poison-honest readback confirms the device is still answering. Every
        /// register write is announced before issue (they are fabric-visible controller writes).
        fn init_rings(&mut self) -> bool {
            serial_println!("{}   M2 ring bring-up (C+ mode; RTL8168 programming-guide order) ::", P4);
            // Unlock the config/registers for write (93C46 command = 0xC0).
            serial_println!("{}   >>> REG WRITE (M2): CFG9346[{:#x}] = {:#04x} (unlock config) ::", P4, REG_CFG9346, CFG9346_UNLOCK);
            self.w8(REG_CFG9346, CFG9346_UNLOCK);

            // C+ command register: read + log the current value (the reset default already selects the
            // C+ descriptor engine on the RTL8168; we preserve it rather than force reserved bits).
            let cpc = self.r16(REG_CPLUSCMD);
            serial_println!("{}   CPlusCmd[{:#x}] = {:#06x} (C+ engine) ::", P4, REG_CPLUSCMD, cpc);

            // Allocate + program the descriptor rings (256-byte-aligned physical bases).
            self.alloc_rx();
            self.alloc_tx();
            let rx_phys = self.rx_ring as u64;
            let tx_phys = self.tx_ring as u64;
            serial_println!("{}   >>> REG WRITE (M2): RDSAR[{:#x}] = {:#x} (RX ring, {} desc) ::", P4, REG_RDSAR, rx_phys, NUM_RX);
            self.w32(REG_RDSAR, rx_phys as u32);
            self.w32(REG_RDSAR + 4, (rx_phys >> 32) as u32);
            serial_println!("{}   >>> REG WRITE (M2): TNPDS[{:#x}] = {:#x} (TX ring, {} desc) ::", P4, REG_TNPDS, tx_phys, NUM_TX);
            self.w32(REG_TNPDS, tx_phys as u32);
            self.w32(REG_TNPDS + 4, (tx_phys >> 32) as u32);

            // Receive max size + max TX packet size.
            serial_println!("{}   >>> REG WRITE (M2): RMS[{:#x}] = {:#06x}; MTPS[{:#x}] = {:#04x} ::", P4, REG_RMS, RX_BUF_SIZE as u16, REG_MTPS, MTPS_DEFAULT);
            self.w16(REG_RMS, RX_BUF_SIZE as u16);
            self.w8(REG_MTPS, MTPS_DEFAULT);

            // Transmit config (MXDMA unlimited + standard IFG).
            let tcr = TCR_MXDMA_UNLIMITED | TCR_IFG_STD;
            serial_println!("{}   >>> REG WRITE (M2): TCR[{:#x}] = {:#010x} ::", P4, REG_TCR, tcr);
            self.w32(REG_TCR, tcr);

            // Publish the ring descriptors before the engine starts fetching them.
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };

            // Enable RX + TX in the ChipCmd register.
            serial_println!("{}   >>> REG WRITE (M2): CR[{:#x}] = {:#04x} (RxEnb | TxEnb) ::", P4, REG_CR, CR_RE | CR_TE);
            self.w8(REG_CR, CR_RE | CR_TE);

            // Receive config LAST (this arms reception): promiscuous bring-up filter + MXDMA/RXFTH.
            let rcr = RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_MXDMA_UNLIMITED | RCR_RXFTH_NONE;
            serial_println!("{}   >>> REG WRITE (M2): RCR[{:#x}] = {:#010x} (promiscuous bring-up) ::", P4, REG_RCR, rcr);
            self.w32(REG_RCR, rcr);

            // Re-lock the config registers.
            self.w8(REG_CFG9346, CFG9346_LOCK);

            // Polled bring-up: mask every interrupt source, clear any latched status (write-1-to-clear).
            self.w16(REG_IMR, 0);
            self.w16(REG_ISR, 0xffff);
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };

            // Poison-honest readback: a live controller returns a plausible TCR (our written value's
            // MXDMA/IFG bits, not an open-bus all-ones). Reject 0xffffffff / 0xdeadbeef as absent decode.
            let tcr_rb = self.r32(REG_TCR);
            if is_poison(tcr_rb) {
                serial_println!("{}   TCR readback = {:#010x} — POISON (open bus / device stopped answering); ring bring-up FAILED ::", P4, tcr_rb);
                return false;
            }
            serial_println!(
                "{}   rings up: RX @ {:#x} ({} desc) TX @ {:#x} ({} desc); TCR readback {:#010x} (live) ::",
                P4, rx_phys, NUM_RX, tx_phys, NUM_TX, tcr_rb
            );
            true
        }

        /// Link state from PHYstatus.LinkSts (bit 1).
        fn link_up(&self) -> bool {
            self.r8(REG_PHYSTATUS) & PHYSTATUS_LINKSTS != 0
        }

        /// Transmit one raw Ethernet frame (smoltcp builds the full L2 frame): copy it into the next
        /// TX buffer, post an OWN|FS|LS descriptor, kick the normal-priority queue (TPPoll.NPQ), and
        /// wait (bounded) for the NIC to clear OWN. A stalled link leaves OWN set — surfaced, not
        /// silently counted. Mirrors the e1000 `transmit` head/tail discipline.
        fn transmit(&mut self, frame: &[u8]) {
            // NET-4d: capture the DHCP DISCOVER's transaction id the first time we send one, so the
            // RX-window classifier can match/mismatch inbound OFFERs against it. Read-only parse.
            if self.d_xid.is_none() {
                if let Some(di) = decode_dhcp(frame) {
                    if di.op == 1 && di.mtype == 1 {
                        self.d_xid = Some(di.xid);
                        serial_println!(
                            "{}   [net4d] TX DHCP DISCOVER xid={:#010x} (udp {}->{}, {} bytes) — captured for RX xid-match ::",
                            P4, di.xid, di.sport, di.dport, frame.len()
                        );
                    }
                }
            }
            let i = self.tx_cur;
            let len = frame.len().min(TX_BUF_SIZE);
            let buf = unsafe { self.tx_buffers.add(i * TX_BUF_SIZE) };
            unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), buf, len) };
            let eor = if i == NUM_TX - 1 { DESC_EOR } else { 0 };
            let d = Desc {
                opts1: DESC_OWN | DESC_FS | DESC_LS | eor | (len as u32 & DESC_LEN_MASK),
                opts2: 0,
                addr: (self.tx_buffers as u64) + (i * TX_BUF_SIZE) as u64,
            };
            unsafe { write_volatile(self.tx_ring.add(i), d) };
            // Publish the descriptor + buffer before poking the doorbell.
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
            self.w8(REG_TPPOLL, TPPOLL_NPQ);
            self.tx_cur = (i + 1) % NUM_TX;

            // Wait (bounded) for the descriptor to be handed back (OWN cleared by the NIC).
            let mut done = false;
            for _ in 0..1_000_000 {
                let dd = unsafe { read_volatile(self.tx_ring.add(i)) };
                if dd.opts1 & DESC_OWN == 0 {
                    done = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if done {
                self.tx_count += 1;
                // NET-4c: one-shot TX proof on the FIRST consumed frame of the boot (on the
                // armed path that frame is the DHCP DISCOVER). OWN handed back + latched
                // ISR.TOK is NIC-level evidence the frame left the MAC — precisely what the
                // R22 sitting-2 no-lease left unproven.
                if self.tx_count == 1 {
                    let isr = self.r16(REG_ISR);
                    serial_println!(
                        "{}   [net4c] first TX frame ({} bytes) CONSUMED: OWN handed back, ISR={:#06x} (TOK={} TER={}) ::",
                        P4, len, isr, (isr >> 2) & 1, (isr >> 3) & 1);
                }
            } else {
                self.tx_stalled += 1;
                serial_println!("{}   [tx] descriptor {} never completed (OWN still set — link stalled?) ::", P4, i);
            }
        }

        /// NET-4c: bounded, read-only TX/RX evidence snapshot for the DHCP no-lease
        /// investigation — printed after the discover window. TX side: consumed vs stalled
        /// descriptor counts, the last-posted descriptor's OWN bit, and the latched ISR
        /// (TOK/TER/ROK/RER — nothing has cleared ISR since bring-up, so these are
        /// since-bring-up latches). RX side: frames popped plus how many ring slots the NIC
        /// has filled and handed back unread. Reads only; no register is written.
        fn net4c_evidence(&self, label: &str) {
            let isr = self.r16(REG_ISR);
            let last_tx = if self.tx_cur == 0 { NUM_TX - 1 } else { self.tx_cur - 1 };
            let d_opts1 = unsafe { read_volatile(self.tx_ring.add(last_tx)) }.opts1;
            let mut rx_filled = 0usize;
            for i in 0..NUM_RX {
                if unsafe { read_volatile(self.rx_ring.add(i)) }.opts1 & DESC_OWN == 0 {
                    rx_filled += 1;
                }
            }
            serial_println!(
                "{}   [net4c {}] TX consumed={} stalled={} last-desc[{}] opts1={:#010x} (OWN={}) | ISR={:#06x} (ROK={} RER={} TOK={} TER={}) | RX popped={} filled-unread={}/{} ::",
                P4, label, self.tx_count, self.tx_stalled, last_tx, d_opts1, (d_opts1 >> 31) & 1,
                isr, isr & 1, (isr >> 1) & 1, (isr >> 2) & 1, (isr >> 3) & 1,
                self.rx_count, rx_filled, NUM_RX);
        }

        /// NET-4d: classify one popped RX frame during the DHCP discover window and emit a bounded,
        /// read-only evidence line (the RX-side proof for the no-lease: does the OFFER arrive, and if
        /// so does the driver-visible accept path take it?). A full L2/L3/L4 line for the first
        /// `NET4D_FULL_LINES` frames and ALWAYS for any BOOTP/DHCP (UDP 67/68) frame; every frame is
        /// tallied by category for the window-close summary. For a DHCP frame the BOOTP op / message
        /// type / xid / yiaddr are decoded and the xid is matched against the captured DISCOVER xid;
        /// an OFFER is additionally checked against the driver-visible accept conditions. Read-only.
        fn net4d_classify(&mut self, frame: &[u8]) {
            if !self.rxcls_active {
                return;
            }
            let idx = self.rx_count;
            let len = frame.len();
            if len < 14 {
                self.rxcat[RXCAT_OTHER] += 1;
                if self.rxcls_full < NET4D_FULL_LINES {
                    self.rxcls_full += 1;
                    serial_println!("{}   [net4d] rx[{}] len={} runt(<14) — class=other ::", P4, idx, len);
                }
                return;
            }
            let d = &frame[0..6];
            let s = &frame[6..12];
            let et = u16::from_be_bytes([frame[12], frame[13]]);

            // BOOTP/DHCP: full line ALWAYS (unbounded), the frame the investigation is about.
            if let Some(di) = decode_dhcp(frame) {
                self.rxcat[RXCAT_DHCP] += 1;
                self.rxcls_full = self.rxcls_full.saturating_add(1);
                let (xtok, xexp) = match self.d_xid {
                    Some(x) if x == di.xid => ("MATCH", x),
                    Some(x) => ("MISMATCH", x),
                    None => ("no-DISCOVER-xid-seen", 0),
                };
                serial_println!(
                    "{}   [net4d] rx[{}] len={} dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} DHCP {}.{}.{}.{}:{}->{}.{}.{}.{}:{} op={} type={}({}) xid={:#010x} vs-DISCOVER {:#010x} [{}] yiaddr={}.{}.{}.{} ::",
                    P4, idx, len,
                    d[0], d[1], d[2], d[3], d[4], d[5],
                    s[0], s[1], s[2], s[3], s[4], s[5],
                    di.sip[0], di.sip[1], di.sip[2], di.sip[3], di.sport,
                    di.dip[0], di.dip[1], di.dip[2], di.dip[3], di.dport,
                    di.op, di.mtype, dhcp_mtype_name(di.mtype), di.xid, xexp, xtok,
                    di.yiaddr[0], di.yiaddr[1], di.yiaddr[2], di.yiaddr[3]
                );
                // Item 3: an OFFER that a lease never followed — name the driver-visible check.
                if di.mtype == 2 {
                    self.net4d_offer_check(&di, d);
                }
                return;
            }

            // Non-DHCP: categorize (for the summary) and print a full L2 line only within the bound.
            let cat = match et {
                0x0806 => RXCAT_ARP,
                0x86dd => RXCAT_IPV6,
                0x0800 => {
                    let ip = &frame[14..];
                    if ip.len() >= 20 && (ip[0] >> 4) == 4 {
                        let ihl = ((ip[0] & 0x0f) as usize) * 4;
                        if ihl >= 20 && ip.len() >= ihl + 4 && ip[9] == 17 {
                            RXCAT_UDP_OTHER
                        } else {
                            RXCAT_IPV4_OTHER
                        }
                    } else {
                        RXCAT_IPV4_OTHER
                    }
                }
                _ => RXCAT_OTHER,
            };
            self.rxcat[cat] += 1;
            if self.rxcls_full < NET4D_FULL_LINES {
                self.rxcls_full += 1;
                let name = match cat {
                    RXCAT_ARP => "arp",
                    RXCAT_UDP_OTHER => "udp-other",
                    RXCAT_IPV4_OTHER => "ipv4-other",
                    RXCAT_IPV6 => "ipv6",
                    _ => "other",
                };
                serial_println!(
                    "{}   [net4d] rx[{}] len={} dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} et={:#06x} class={} ::",
                    P4, idx, len,
                    d[0], d[1], d[2], d[3], d[4], d[5],
                    s[0], s[1], s[2], s[3], s[4], s[5],
                    et, name
                );
            }
        }

        /// NET-4d: for an inbound DHCP OFFER (message type 2), name the FIRST driver-visible accept
        /// condition it fails — the xid must equal the DISCOVER's, and the destination MAC must be our
        /// station MAC or broadcast. If it passes both, the drop is ABOVE the driver (the smoltcp
        /// dhcpv4 socket), and we say so explicitly rather than blame the wire. Read-only.
        fn net4d_offer_check(&self, di: &DhcpInfo, dst: &[u8]) {
            let xid_ok = self.d_xid == Some(di.xid);
            let is_bcast = dst.iter().all(|&b| b == 0xff);
            let is_ours = dst == &self.mac[..];
            if !xid_ok {
                serial_println!(
                    "{}   [net4d] OFFER xid {:#010x} != DISCOVER xid {:#010x} — driver-visible REJECT: wrong transaction ::",
                    P4, di.xid, self.d_xid.unwrap_or(0)
                );
            } else if !(is_bcast || is_ours) {
                serial_println!(
                    "{}   [net4d] OFFER xid matches but dst MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} is neither our station MAC nor broadcast — driver-visible REJECT: addressed elsewhere ::",
                    P4, dst[0], dst[1], dst[2], dst[3], dst[4], dst[5]
                );
            } else {
                serial_println!(
                    "{}   [net4d] OFFER xid matches DISCOVER + addressed to us ({}) yiaddr={}.{}.{}.{} — passes ALL driver-visible checks; if no lease followed, the drop is ABOVE the driver (smoltcp dhcpv4 socket) ::",
                    P4, if is_bcast { "broadcast" } else { "unicast" },
                    di.yiaddr[0], di.yiaddr[1], di.yiaddr[2], di.yiaddr[3]
                );
            }
        }

        /// NET-4d: close the classification window — emit the per-category RX summary once and stop
        /// classifying (so the post-window bounded ICMP poll does not re-classify). Read-only.
        fn net4d_window_close(&mut self) {
            self.rxcls_active = false;
            let xid = match self.d_xid {
                Some(x) => x,
                None => 0,
            };
            serial_println!(
                "{}   [net4d window-close] RX by category: arp={} dhcp={} udp-other={} ipv4-other={} ipv6={} other={} (total popped={}); DISCOVER xid={:#010x} ({}) ::",
                P4,
                self.rxcat[RXCAT_ARP], self.rxcat[RXCAT_DHCP], self.rxcat[RXCAT_UDP_OTHER],
                self.rxcat[RXCAT_IPV4_OTHER], self.rxcat[RXCAT_IPV6], self.rxcat[RXCAT_OTHER],
                self.rx_count, xid,
                if self.d_xid.is_some() { "sent" } else { "NEVER SENT" }
            );
        }

        /// Pop one completed RX descriptor's raw Ethernet frame into `out` and recycle the descriptor
        /// (re-arm OWN + buffer size), advancing the ring cursor. Returns the copied length, or `None`
        /// if the current descriptor is still NIC-owned (ring empty). The C+ analog of the e1000
        /// `rx_frame_raw` — no responder dispatch, smoltcp owns the stack.
        fn rx_frame_raw(&mut self, out: &mut [u8]) -> Option<usize> {
            let d = unsafe { read_volatile(self.rx_ring.add(self.rx_cur)) };
            // OWN set ⇒ still owned by the NIC (not yet filled) ⇒ ring empty.
            if d.opts1 & DESC_OWN != 0 {
                return None;
            }
            // NET-4e: DMA READ BARRIER between observing OWN-clear and reading the buffer — the fix
            // for the DHCP no-lease. The NIC commits a received frame by writing the payload FIRST and
            // clearing OWN LAST; on weakly-ordered aarch64 the CPU may observe the OWN-clear without yet
            // observing the payload write, so the copy below reads STALE bytes (the `alloc_zeroed` fill
            // ⇒ an all-zero frame). Each descriptor is popped exactly once and recycled, so a single
            // stale read drops that frame forever — which is precisely how the OFFER got through (it won
            // the race) but the follow-on ACK did not (read as zeros), leaving the lease uncompleted.
            // This is Linux r8169's `dma_rmb()` after the OWN check. A barrier (not a cache invalidate)
            // is the honest minimal fix: the TX path already proved controller-0's DMA is CPU-coherent
            // (the DISCOVER's buffer, written cacheable, was read correctly by the NIC), so what was
            // missing is only the ORDERING of the CPU's two observations — not cache maintenance.
            unsafe { core::arch::asm!("dsb ld", options(nostack, preserves_flags)) };
            // Hardware wrote the received length into the length field; clamp so a misbehaving NIC can
            // never make us build an out-of-bounds slice.
            let len = (d.opts1 & DESC_LEN_MASK) as usize;
            let len = len.min(RX_BUF_SIZE).min(out.len());
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.rx_buffers.add(self.rx_cur * RX_BUF_SIZE),
                    out.as_mut_ptr(),
                    len,
                );
            }
            // NET-4e: one-shot witness on the first popped frame — proves the DMA read barrier is on the
            // live RX path (the fix that lets the DHCP ACK be read instead of stale zeros).
            if self.rx_count == 0 {
                serial_println!(
                    "{}   [net4e] first RX pop len={} — DMA read barrier (dsb ld) active between OWN-check and buffer copy ::",
                    P4, len
                );
            }
            self.rx_count += 1;
            // NET-4d: classify this frame (bounded, read-only) while the DHCP discover window is live.
            // Borrows `out` (not `self`), so the &mut self counter updates do not alias.
            self.net4d_classify(&out[..len]);
            // Recycle: re-arm this descriptor for the NIC (OWN + buffer size + EOR on the last slot).
            let eor = if self.rx_cur == NUM_RX - 1 { DESC_EOR } else { 0 };
            let nd = Desc {
                opts1: DESC_OWN | eor | (RX_BUF_SIZE as u32 & DESC_LEN_MASK),
                opts2: 0,
                addr: (self.rx_buffers as u64) + (self.rx_cur * RX_BUF_SIZE) as u64,
            };
            unsafe { write_volatile(self.rx_ring.add(self.rx_cur), nd) };
            self.rx_cur = (self.rx_cur + 1) % NUM_RX;
            Some(len)
        }
    }

    // ── Controller-0 aperture resolution (a lean, self-contained DTB walk) ──

    /// Resolve controller-0's `ecam` region base from the live DTB: find the first `pcie@` node, then
    /// index its `reg`/`reg-names` for "ecam" (4 cells = addr:2 + size:2 per region, big-endian).
    /// READ-ONLY. Returns `None` on any missing/foreign DTB (QEMU virt has no Tegra234 RC). Confirms
    /// the node is a Tegra DesignWare RC and firmware-enabled before returning the base.
    fn resolve_ecam_base(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<u64> {
        if dtb_addr == 0 || dtb_size == 0 {
            return None;
        }
        // The DTB must be in a mapped GiB (GiB 0 device window, or a RAM GiB) before we deref it.
        let g_lo = dtb_addr >> 30;
        let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
        let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
        if !mapped(g_lo) || !mapped(g_hi) {
            return None;
        }
        let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
        let fdt = Fdt::new(blob)?;

        // First `pcie@` node's path.
        const PATH_CAP: usize = 160;
        let mut path0 = [0u8; PATH_CAP];
        let mut plen0 = 0usize;
        let mut found = false;
        fdt.for_each_prop(|e| {
            if found {
                return;
            }
            let leaf = match e.path.iter().rposition(|&b| b == b'/') {
                Some(i) => &e.path[i + 1..],
                None => e.path,
            };
            if leaf.starts_with(b"pcie@") {
                let l = e.path.len().min(PATH_CAP);
                path0[..l].copy_from_slice(&e.path[..l]);
                plen0 = l;
                found = true;
            }
        });
        if !found {
            return None;
        }
        let path = &path0[..plen0];

        // Capture the props we need in one walk: compatible, status, reg, reg-names.
        let mut compatible: Option<&[u8]> = None;
        let mut status: Option<&[u8]> = None;
        let mut reg: Option<&[u8]> = None;
        let mut reg_names: Option<&[u8]> = None;
        fdt.for_each_prop(|e| {
            if e.path != path {
                return;
            }
            let val = &blob[e.val_off..e.val_off + e.val_len];
            match e.name {
                b"compatible" => compatible = Some(val),
                b"status" => status = Some(val),
                b"reg" => reg = Some(val),
                b"reg-names" => reg_names = Some(val),
                _ => {}
            }
        });

        // Tegra DesignWare RC? (a generic virt ecam is not — graceful skip).
        let is_tegra_rc = compatible
            .map(|c| {
                let has = |n: &[u8]| c.windows(n.len()).any(|w| w == n);
                has(b"tegra234-pcie") || has(b"tegra194-pcie") || has(b"snps,dw-pcie")
            })
            .unwrap_or(false);
        if !is_tegra_rc {
            return None;
        }
        // Firmware-enabled? (absent status ⇒ "okay" per the DT spec; anything but okay/ok ⇒ skip.)
        let okay = match status {
            None => true,
            Some(s) => s.split(|&b| b == 0).any(|item| item == b"okay" || item == b"ok"),
        };
        if !okay {
            return None;
        }

        // Index reg-names for "ecam"; read that region's 64-bit base from reg.
        let (reg, names) = (reg?, reg_names?);
        let mut idx = 0usize;
        for item in names.split(|&b| b == 0) {
            if item.is_empty() {
                continue;
            }
            if item == b"ecam" {
                let off = idx * 16; // 4 cells * 4 bytes
                let b = reg.get(off..off + 8)?;
                let hi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
                let lo = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as u64;
                return Some((hi << 32) | lo);
            }
            idx += 1;
        }
        None
    }

    // ══════════════════════════════════════════════════════════════════════════════════════════════
    // DWC outbound iATU — the fix-forward for the ORIN-NET-4 M1 metal FAULT-AT-M1.
    // ══════════════════════════════════════════════════════════════════════════════════════════════
    //
    // ## The fault of record (adjudicated; NOT re-litigated)
    //
    // The NET-4 driver reached the RTL8168 (config reads/writes via the ECAM fine, twice-confirmed),
    // then the FIRST BAR-register write (CR soft reset) raised a RAS Uncorrectable — SNOC "Illegal
    // address (software fault)" / Carveout, `a5a5a5a5` poison fill; recovery needed a DC cut. The
    // adjudication: BAR2 read back `0x4000_4000` — a PCIe BUS address (firmware assigned the device's
    // BARs inside controller-0's PCIe MEM window, whose PCI base is `0x4000_0000`). With the DWC iATU
    // UNPROGRAMMED (the NET-2 finding), there is NO outbound CPU->PCIe MEM translation, so a PCIe bus
    // address is meaningless as a CPU physical address. The old path mapped `0x4000_4000` as a CPU PA —
    // it falls in the GiB-1 SYSRAM/BPMP carveout that `mmu_tegra::fill_table` maps Device-nGnRE, so
    // `map_mmio_window` even returned `AlreadyMapped` without complaint — and the first register write
    // (`0x4000_4000 + CR`) hit a protected Tegra carveout. The bench observation was RIGHT.
    //
    // ## The fix (DWC / pcie-tegra194 sequence-of-record)
    //
    // Program an OUTBOUND iATU region mapping a CPU aperture window (taken from controller-0's DT
    // `ranges`) to the PCIe MEM window, then access the CPU-SIDE aperture address
    // (`cpu_base + (bar_pci - pci_base)`), NEVER the raw BAR value. Firmware's BAR assignment is KEPT
    // (it already sits inside the ranges-described window NET-3 sized it in) and merely TRANSLATED — no
    // BAR reassignment. That is fewer fabric writes and it is the Linux DWC host model of record
    // (`dw_pcie_prog_outbound_atu` walks `ranges` and leaves enumerated BARs in place).
    //
    // ## DWC unrolled-iATU register model
    //
    // Linux `drivers/pci/controller/dwc/pcie-designware.h`. Outbound region N lives at
    // `atu_base + N*0x200`; `atu_base` = controller-0's `atu_dma` reg region (from the DTB), with the
    // DWC-core `dbi + 0x30_0000` fallback documented for a controller that ships no dedicated ATU
    // region. Every iATU register write is announced on serial before issue (the lane's write
    // discipline). These writes target the controller's OWN internal register block (GiB-0 device
    // window, always decoding on a powered RC — NET-2/3 read dbi/appl/ecam there) — NOT a carveout, so
    // they carry none of the M1 fault's risk.
    const ATU_REGION_STRIDE: u64 = 0x200;
    const ATU_UNR_REGION_CTRL1: u64 = 0x00;
    const ATU_UNR_REGION_CTRL2: u64 = 0x04;
    const ATU_UNR_LOWER_BASE: u64 = 0x08;
    const ATU_UNR_UPPER_BASE: u64 = 0x0c;
    const ATU_UNR_LOWER_LIMIT: u64 = 0x10;
    const ATU_UNR_LOWER_TARGET: u64 = 0x14;
    const ATU_UNR_UPPER_TARGET: u64 = 0x18;
    const ATU_UNR_UPPER_LIMIT: u64 = 0x20;
    /// CTRL1 TYPE field: memory outbound = 0x0. CTRL2: region-enable = bit31; increase-region-size =
    /// bit13 (makes LIMIT the full 64-bit UPPER|LOWER pair — required here, the CPU aperture base sits
    /// ~200 GiB up, well beyond 32 bits).
    const ATU_TYPE_MEM: u32 = 0x0;
    const ATU_ENABLE: u32 = 1 << 31;
    const ATU_INCREASE_REGION_SIZE: u32 = 1 << 13;
    /// The DWC-core fallback ATU offset when a controller exposes no dedicated ATU reg region.
    const ATU_DBI_FALLBACK_OFF: u64 = 0x30_0000;

    /// A `ranges` MEM window: its PCIe base, the CPU aperture base it maps to, and its size.
    #[derive(Clone, Copy)]
    struct MemWindow {
        pci_base: u64,
        cpu_base: u64,
        size: u64,
    }

    /// Program one DWC OUTBOUND iATU region `[cpu_base, cpu_base+size)` -> PCIe `[pci_base, …)`, type
    /// MEM, and enable it. `atu_base` must already be reachable (GiB-0 device window; the caller
    /// idempotent-maps it). Base/limit/target are published (`dsb sy`) before the region is armed.
    fn program_outbound_atu(atu_base: u64, index: u64, win: &MemWindow) {
        let region = atu_base + index * ATU_REGION_STRIDE;
        let limit = win.cpu_base + win.size - 1;
        let w = |off: u64, v: u32| unsafe { write_volatile((region + off) as *mut u32, v) };
        serial_println!(
            "{}   M1-fix: outbound iATU region {} @ {:#x} — CPU [{:#x}..{:#x}] -> PCIe {:#x} (type MEM) ::",
            P4, index, region, win.cpu_base, limit, win.pci_base
        );
        serial_println!(
            "{}   >>> ATU WRITE (M1-fix): BASE lo/hi = {:#010x}/{:#010x} ::",
            P4, win.cpu_base as u32, (win.cpu_base >> 32) as u32
        );
        w(ATU_UNR_LOWER_BASE, win.cpu_base as u32);
        w(ATU_UNR_UPPER_BASE, (win.cpu_base >> 32) as u32);
        serial_println!(
            "{}   >>> ATU WRITE (M1-fix): LIMIT lo/hi = {:#010x}/{:#010x} ::",
            P4, limit as u32, (limit >> 32) as u32
        );
        w(ATU_UNR_LOWER_LIMIT, limit as u32);
        w(ATU_UNR_UPPER_LIMIT, (limit >> 32) as u32);
        serial_println!(
            "{}   >>> ATU WRITE (M1-fix): TARGET lo/hi = {:#010x}/{:#010x} ::",
            P4, win.pci_base as u32, (win.pci_base >> 32) as u32
        );
        w(ATU_UNR_LOWER_TARGET, win.pci_base as u32);
        w(ATU_UNR_UPPER_TARGET, (win.pci_base >> 32) as u32);
        serial_println!("{}   >>> ATU WRITE (M1-fix): REGION_CTRL1 = TYPE_MEM ::", P4);
        w(ATU_UNR_REGION_CTRL1, ATU_TYPE_MEM);
        // Publish base/limit/target BEFORE the region goes live.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
        serial_println!(
            "{}   >>> ATU WRITE (M1-fix): REGION_CTRL2 = ENABLE|INCREASE_REGION_SIZE — arming region ::",
            P4
        );
        w(ATU_UNR_REGION_CTRL2, ATU_ENABLE | ATU_INCREASE_REGION_SIZE);
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }

    /// Resolve, from the live DTB, controller-0's `atu_dma` ATU base (with the `dbi + 0x30_0000` DWC
    /// fallback) and the `ranges` MEM window that CONTAINS `bar_pci`. READ-ONLY parse, poison-honest:
    /// a missing/foreign/disabled DTB, an unreachable DTB GiB, or a BAR that no MEM window covers all
    /// return `None`, and the caller REFUSES (clean skip) rather than deref a raw PCIe BAR. Mirrors
    /// `resolve_ecam_base`'s walk (first `pcie@` node, tegra-RC + firmware-`okay` gated).
    fn resolve_atu_and_window(
        dtb_addr: u64,
        dtb_size: usize,
        ram_gib_mask: u64,
        bar_pci: u64,
    ) -> Option<(u64, MemWindow)> {
        if dtb_addr == 0 || dtb_size == 0 {
            return None;
        }
        let g_lo = dtb_addr >> 30;
        let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
        let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
        if !mapped(g_lo) || !mapped(g_hi) {
            return None;
        }
        let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
        let fdt = Fdt::new(blob)?;

        // First `pcie@` node's path.
        const PATH_CAP: usize = 160;
        let mut path0 = [0u8; PATH_CAP];
        let mut plen0 = 0usize;
        let mut found = false;
        fdt.for_each_prop(|e| {
            if found {
                return;
            }
            let leaf = match e.path.iter().rposition(|&b| b == b'/') {
                Some(i) => &e.path[i + 1..],
                None => e.path,
            };
            if leaf.starts_with(b"pcie@") {
                let l = e.path.len().min(PATH_CAP);
                path0[..l].copy_from_slice(&e.path[..l]);
                plen0 = l;
                found = true;
            }
        });
        if !found {
            return None;
        }
        let path = &path0[..plen0];

        let mut compatible: Option<&[u8]> = None;
        let mut status: Option<&[u8]> = None;
        let mut reg: Option<&[u8]> = None;
        let mut reg_names: Option<&[u8]> = None;
        let mut ranges: Option<&[u8]> = None;
        fdt.for_each_prop(|e| {
            if e.path != path {
                return;
            }
            let val = &blob[e.val_off..e.val_off + e.val_len];
            match e.name {
                b"compatible" => compatible = Some(val),
                b"status" => status = Some(val),
                b"reg" => reg = Some(val),
                b"reg-names" => reg_names = Some(val),
                b"ranges" => ranges = Some(val),
                _ => {}
            }
        });

        // Tegra DesignWare RC + firmware-enabled? (same gate as resolve_ecam_base.)
        let is_tegra_rc = compatible
            .map(|c| {
                let has = |n: &[u8]| c.windows(n.len()).any(|w| w == n);
                has(b"tegra234-pcie") || has(b"tegra194-pcie") || has(b"snps,dw-pcie")
            })
            .unwrap_or(false);
        if !is_tegra_rc {
            return None;
        }
        let okay = match status {
            None => true,
            Some(s) => s.split(|&b| b == 0).any(|item| item == b"okay" || item == b"ok"),
        };
        if !okay {
            return None;
        }

        // reg/reg-names region base by name (4 cells = addr:2 + size:2 per region, big-endian).
        let (reg, names) = (reg?, reg_names?);
        let region_base = |want: &[u8]| -> Option<u64> {
            let mut idx = 0usize;
            for item in names.split(|&b| b == 0) {
                if item.is_empty() {
                    continue;
                }
                if item == want {
                    let off = idx * 16;
                    let b = reg.get(off..off + 8)?;
                    let hi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
                    let lo = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as u64;
                    return Some((hi << 32) | lo);
                }
                idx += 1;
            }
            None
        };
        // Prefer the dedicated ATU region; fall back to the DWC-core dbi + 0x30_0000 offset.
        let atu_base = region_base(b"atu_dma")
            .or_else(|| region_base(b"atu"))
            .or_else(|| region_base(b"dbi").map(|d| d + ATU_DBI_FALLBACK_OFF))?;

        // Walk `ranges`: rows of 7 cells (child PCI addr:3, parent CPU addr:2, size:2 = 28 bytes).
        // The child cell-0 high byte's space code ((>>24)&3): 2 = 32-bit MEM, 3 = 64-bit MEM (1 = I/O,
        // skipped). Return the first MEM window whose [pci_base, pci_base+size) contains `bar_pci`.
        let ranges = ranges?;
        let cell = |b: &[u8], i: usize| -> u64 {
            u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]) as u64
        };
        let mut off = 0usize;
        while off + 28 <= ranges.len() {
            let row = &ranges[off..off + 28];
            let space = (cell(row, 0) >> 24) & 0x3;
            let pci_base = (cell(row, 1) << 32) | cell(row, 2);
            let cpu_base = (cell(row, 3) << 32) | cell(row, 4);
            let size = (cell(row, 5) << 32) | cell(row, 6);
            if (space == 2 || space == 3)
                && size != 0
                && bar_pci >= pci_base
                && bar_pci < pci_base + size
            {
                return Some((atu_base, MemWindow { pci_base, cpu_base, size }));
            }
            off += 28;
        }
        None
    }

    /// ORIN-NET-4 entry point (metal): claim controller-0's downstream RTL8168, map its register BAR,
    /// reset the MAC, and read the station MAC. Rings + init (M2) and the smoltcp bind (M3) land in
    /// later milestones. Graceful on any missing/foreign DTB or absent decode (records and returns).
    pub fn net4_bringup(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
        serial_println!(
            "{} ORIN-NET-4 RTL8168/8111 GbE bring-up (DTB @{:#x} size={:#x}) ::",
            P4, dtb_addr, dtb_size
        );

        // ── Resolve + map controller-0's ECAM (the direct hardware config window NET-3 unlocked) ──
        let Some(ecam) = resolve_ecam_base(dtb_addr, dtb_size, ram_gib_mask) else {
            serial_println!(
                "{}   no enabled Tegra234 RC ecam in the DTB — bring-up SKIPPED (graceful; QEMU virt / no-net) ::",
                P4
            );
            return;
        };
        // The ECAM is ~184 GiB — reachable only through NET-3's PS-widen. map_mmio_window refuses it if
        // the widen is not in effect (a pcie3-off build), which the driver reports rather than deref.
        let ecam_size = 256 * 1024 * 1024; // Tegra234 whole-domain ECAM window
        match map_mmio_window(ecam, ecam_size) {
            MmioMap::Mapped | MmioMap::AlreadyMapped => {
                serial_println!("{}   ecam {:#x} mapped Device-nGnRE (via the PS-widened regime) ::", P4, ecam);
            }
            MmioMap::BeyondPsCeiling => {
                serial_println!(
                    "{}   ecam {:#x} BEYOND the PS ceiling — the NET-3 TCR widen is not in effect; bring-up cannot reach config space ::",
                    P4, ecam
                );
                return;
            }
        }
        let dev = ecam + BUS1_DEV0_FN0;

        // ── Confirm the device identity (poison-rejecting; must be the metal-identified Realtek) ──
        let vd = unsafe { core::ptr::read_volatile((dev + CFG_VENDOR) as *const u32) };
        if is_poison(vd) {
            serial_println!(
                "{}   bus1:dev0:fn0 config[0x00] = {:#010x} — ABSENT DECODE (link down / no device answering); bring-up SKIPPED ::",
                P4, vd
            );
            return;
        }
        let vendor = (vd & 0xffff) as u16;
        let device = (vd >> 16) as u16;
        serial_println!("{}   bus1:dev0:fn0 vendor={:#06x} device={:#06x} ::", P4, vendor, device);
        if vendor != REALTEK_VENDOR || device != RTL8168_DEVICE {
            serial_println!(
                "{}   not the metal-identified Realtek RTL8168/8111 ({:#06x}:{:#06x}) — bring-up SKIPPED (won't drive an unknown device) ::",
                P4, REALTEK_VENDOR, RTL8168_DEVICE
            );
            return;
        }

        // ── Enable MEM-space decode + bus-master so the BARs decode and the NIC can DMA (M2 rings) ──
        // This is the driver doing the config write NET-3 deliberately refused. Announced before issue.
        let cmd = unsafe { core::ptr::read_volatile((dev + CFG_COMMAND) as *const u32) };
        let cmd_lo = (cmd & 0xffff) as u16;
        let newcmd = cmd_lo | CMD_MEM_SPACE | CMD_BUS_MASTER;
        serial_println!(
            "{}   >>> CONFIG WRITE (M1): COMMAND[{:#x}] {:#06x} -> {:#06x} (set MEM-space + bus-master) — issuing ::",
            P4, CFG_COMMAND, cmd_lo, newcmd
        );
        unsafe {
            core::ptr::write_volatile((dev + CFG_COMMAND) as *mut u32, (cmd & 0xffff_0000) | newcmd as u32);
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }

        // ── Resolve the register BAR (BAR2: mem 0x1000 per the NET-3 sizing). Handle 64-bit type. ──
        let bar2 = unsafe { core::ptr::read_volatile((dev + CFG_BAR2) as *const u32) };
        if is_poison(bar2) || bar2 == 0 {
            serial_println!("{}   BAR2 = {:#010x} — unimplemented/absent register BAR; bring-up SKIPPED ::", P4, bar2);
            return;
        }
        if bar2 & 1 == 1 {
            serial_println!("{}   BAR2 = {:#010x} is I/O-space (expected memory) — bring-up SKIPPED ::", P4, bar2);
            return;
        }
        let is_64bit = (bar2 >> 1) & 0x3 == 0x2;
        let base_lo = (bar2 & !0xf) as u64;
        let bar_base = if is_64bit {
            let bar3 = unsafe { core::ptr::read_volatile((dev + CFG_BAR3) as *const u32) };
            ((bar3 as u64) << 32) | base_lo
        } else {
            base_lo
        };
        // BAR2 is firmware's assignment: a PCIe BUS address (the M1 FAULT's root fact). NOT a CPU PA.
        let bar_pci = bar_base;
        serial_println!(
            "{}   register BAR2 = {:#x} ({}-bit {}) — this is a PCIe BUS address (needs outbound iATU translation) ::",
            P4, bar_pci, if is_64bit { "64" } else { "32" },
            if (bar2 >> 3) & 1 == 1 { "prefetchable mem" } else { "mem" }
        );
        if bar_pci == 0 {
            serial_println!("{}   BAR2 base is 0 (firmware left it unassigned) — bring-up SKIPPED ::", P4);
            return;
        }
        let bar_size = 0x1000usize;

        // ── M1-FIX: outbound iATU + PCIe->CPU aperture translation (the fault-forward for FAULT-AT-M1) ──
        // The old path mapped `bar_pci` as a CPU PA and wrote the first register there — into a Tegra
        // carveout (RAS Uncorrectable). Instead: resolve controller-0's `ranges` MEM window + `atu_dma`
        // ATU base from the DTB, program an outbound iATU region for that window, then access the
        // CPU-SIDE aperture address. REFUSE (clean skip) on any unresolved piece — never deref the raw
        // BAR again.
        let Some((atu_base, win)) = resolve_atu_and_window(dtb_addr, dtb_size, ram_gib_mask, bar_pci) else {
            serial_println!(
                "{}   M1-fix: no `ranges` MEM window / `atu_dma` base covers BAR2 {:#x} in the DTB — bring-up SKIPPED (REFUSE: will NOT deref a raw PCIe BAR as a CPU address, the FAULT-AT-M1 class) ::",
                P4, bar_pci
            );
            return;
        };
        serial_println!(
            "{}   M1-fix: BAR2 {:#x} in ranges MEM window PCIe [{:#x}..{:#x}) -> CPU base {:#x}; ATU base {:#x} ::",
            P4, bar_pci, win.pci_base, win.pci_base + win.size, win.cpu_base, atu_base
        );
        // The ATU register block is GiB-0 (already mapped); idempotent-map it Device-nGnRE to be safe.
        match map_mmio_window(atu_base, 0x1000) {
            MmioMap::Mapped | MmioMap::AlreadyMapped => {}
            MmioMap::BeyondPsCeiling => {
                serial_println!("{}   M1-fix: ATU base {:#x} unmappable — bring-up SKIPPED ::", P4, atu_base);
                return;
            }
        }
        // Program outbound region 0 for the whole MEM window (announced writes; enabled last).
        program_outbound_atu(atu_base, 0, &win);

        // The CPU-side aperture address for BAR2 = cpu_base + (bar_pci - pci_base). This — NOT bar_pci —
        // is what the CPU dereferences; the iATU forwards it to PCIe. It sits ~200 GiB up, inside the
        // PS-widened 40-bit / 512-GiB-table reach.
        let cpu_addr = win.cpu_base + (bar_pci - win.pci_base);
        match map_mmio_window(cpu_addr, bar_size) {
            MmioMap::Mapped | MmioMap::AlreadyMapped => {
                serial_println!(
                    "{}   BAR2 CPU aperture {:#x} (+{:#x}) mapped Device-nGnRE — registers reachable via iATU ::",
                    P4, cpu_addr, bar_size
                );
            }
            MmioMap::BeyondPsCeiling => {
                serial_println!(
                    "{}   BAR2 CPU aperture {:#x} BEYOND the PS ceiling — cannot map register window; bring-up SKIPPED ::",
                    P4, cpu_addr
                );
                return;
            }
        }

        // ── Construct the driver at the CPU aperture (never the raw BAR value) ──
        let mut nic = Rtl8168 {
            mmio_base: cpu_addr,
            mac: [0; 6],
            rx_ring: core::ptr::null_mut(),
            rx_buffers: core::ptr::null_mut(),
            rx_cur: 0,
            rx_count: 0,
            tx_ring: core::ptr::null_mut(),
            tx_buffers: core::ptr::null_mut(),
            tx_cur: 0,
            tx_count: 0,
            tx_stalled: 0,
            d_xid: None,
            rxcls_full: 0,
            rxcat: [0; RXCAT_N],
            rxcls_active: true,
        };

        // ── M2/M3 GUARD: poison-honest readback through the NEW window BEFORE any register write ──
        // The lesson of FAULT-AT-M1 (and V3D-2) made law: every new MMIO window earns a probe READ
        // before its first WRITE. A live RTL8168 returns a plausible TCR (chip-version bits); poison
        // (open-bus / carveout `a5a5a5a5` / firmware fill) means the iATU/link is not delivering — so
        // we REFUSE cleanly, and the next sitting can never fault on the first write again.
        let Some(tcr_probe) = nic.probe_alive() else {
            serial_println!(
                "{}   M1-fix readback: TCR through the iATU aperture = POISON (open-bus/carveout/absent) — the register window is NOT live; bring-up REFUSED before any write (no first-write fault) ::",
                P4
            );
            return;
        };
        serial_println!(
            "{}   M1-fix readback: TCR = {:#010x} (live, non-poison) — register window confirmed; first write is now safe ::",
            P4, tcr_probe
        );

        // ── Reset the MAC, read the station MAC (M1) ──
        if !nic.soft_reset() {
            serial_println!("{}   MAC reset did not complete — continuing to read MAC (may be stale) ::", P4);
        }
        nic.mac = nic.read_mac();
        let macs = fmt_mac(&nic.mac);
        serial_println!(
            "{}   station MAC = {} ::",
            P4,
            core::str::from_utf8(&macs).unwrap_or("<mac>")
        );

        // ── M2: bring up the C+ RX/TX descriptor rings + init sequence ──
        if !nic.init_rings() {
            serial_println!("{} ORIN-NET-4 bring-up STOPPED after ring init failed (device stopped answering) ::", P4);
            return;
        }

        // Register the driver so the smoltcp bind + any poll path can reach it.
        let link = nic.link_up();
        *NET4_DEVICE.lock() = Some(nic);
        serial_println!(
            "{}   RTL8168 @ BAR2 PCIe {:#x} (CPU aperture {:#x}), MAC read, C+ rings up + RX/TX enabled; PHY link {} ::",
            P4, bar_pci, cpu_addr, if link { "UP" } else { "DOWN" }
        );

        // ── M3: bind a smoltcp phy::Device over the rings (the e1000/smolnet seam) ──
        bind_smoltcp();
        serial_println!("{} ORIN-NET-4 DONE — RTL8168 driver up + smoltcp bound (live traffic = attended metal) ::", P4);
    }

    /// The one registered RTL8168 NIC (populated by [`net4_bringup`]). Mirrors the x86 e1000
    /// `NET_DEVICE` registry; the smoltcp Device adapter reaches the rings through it.
    pub static NET4_DEVICE: spin::Mutex<Option<Rtl8168>> = spin::Mutex::new(None);

    // ── Static FALLBACK addressing (used only if DHCP does not lease within the bounded timeout) ──
    // NET-DHCP made the link's real subnet a DHCP input (the do-it-right fix for the NET-4-landing
    // placeholder): `bind_smoltcp` runs a DHCPv4 client first and only falls back to these values if no
    // lease arrives. They remain here as the honest last resort for a metal link with no DHCP server —
    // the interface still comes up. Documented in arch_arm64.md §ORIN-NET-4.
    const OUR_IP: [u8; 4] = [192, 168, 1, 2];
    const GATEWAY_IP: [u8; 4] = [192, 168, 1, 1];
    /// Bounded DHCP-lease timeout (ms). On a devkit link with a DHCP server the lease lands far inside
    /// this; the bound caps how long a DHCP-less link stalls before the static fallback. The clock is
    /// real time (CNTPCT), so this is non-hanging by construction.
    const DHCP_TIMEOUT_MS: i64 = 5_000;

    /// NET-4c: the discover window is knob-tunable — `UNAOS_NET4_DHCP_MS=<millis>` at build time
    /// widens (or narrows) it for an attended sitting; unset, the default 5 s stands unchanged.
    /// Invalid or zero values fall back to the default (never a hang, never a zero window).
    fn dhcp_timeout_ms() -> i64 {
        if let Some(s) = option_env!("UNAOS_NET4_DHCP_MS") {
            let mut v: i64 = 0;
            let mut any = false;
            for b in s.bytes() {
                if b.is_ascii_digit() && v < 3_600_000 {
                    v = v * 10 + (b - b'0') as i64;
                    any = true;
                } else {
                    any = false;
                    break;
                }
            }
            if any && v > 0 {
                return v;
            }
        }
        DHCP_TIMEOUT_MS
    }

    /// Monotonic millisecond clock from the free-running counter (CNTPCT). Readable at EL2, where
    /// `net4_bringup` runs (before the JC3 EL2→EL1 drop); drives both smoltcp time and the DHCP timeout.
    #[inline]
    fn now_ms() -> i64 {
        let (cnt, frq): (u64, u64);
        unsafe {
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
        }
        if frq == 0 { 0 } else { (cnt.wrapping_mul(1_000) / frq) as i64 }
    }

    // ── Raw L2 accessors over the NET4_DEVICE registry (the shared smoltcp Device seam) ──

    /// Pop one raw RX frame for the smoltcp Device. Short-locks NET4_DEVICE per ring op (the poll must
    /// not hold the lock across a transmit) — the e1000 `raw_rx` discipline.
    fn raw_rx(out: &mut [u8]) -> Option<usize> {
        NET4_DEVICE.lock().as_mut().and_then(|n| n.rx_frame_raw(out))
    }
    /// Transmit one raw L2 frame from the smoltcp Device. Short-locks NET4_DEVICE.
    fn raw_tx(frame: &[u8]) {
        if let Some(n) = NET4_DEVICE.lock().as_mut() {
            n.transmit(frame);
        }
    }
    /// Link-up snapshot for the interface witness. `false` if the NIC never came up.
    fn link_up() -> bool {
        NET4_DEVICE.lock().as_ref().map(|n| n.link_up()).unwrap_or(false)
    }

    // ── The RawNic seam: the shared `net_phy::SmoltcpPhy` moves L2 frames through these ──
    struct Rtl8168Nic;
    impl RawNic for Rtl8168Nic {
        fn rx_frame_raw(out: &mut [u8]) -> Option<usize> {
            raw_rx(out)
        }
        fn transmit(frame: &[u8]) {
            raw_tx(frame)
        }
        fn mac() -> Option<[u8; 6]> {
            NET4_DEVICE.lock().as_ref().map(|n| n.mac)
        }
    }

    // ── smoltcp interface plumbing (the phy::Device itself is the shared `net_phy::SmoltcpPhy`) ──

    use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
    use smoltcp::socket::icmp;
    use smoltcp::time::Instant;
    use smoltcp::wire::{EthernetAddress, HardwareAddress};

    /// Bind a smoltcp `Interface` over the RTL8168 Device and poll it a bounded number of times — the
    /// x86 e1000/smolnet seam, transposed to aarch64/tegra. This PROVES the bind end-to-end (Device +
    /// Interface + ICMP socket construct and poll without fault); on real Orin silicon it drives ARP
    /// for the gateway. In QEMU there is no Tegra234 RC (so this metal path is never reached on virt),
    /// and on metal-pre-subnet-config the poll simply finds an empty ring — the honest pre-metal state.
    /// All storage is stack-local (no heap growth), mirroring `smolnet::pump`.
    fn bind_smoltcp() {
        let Some(mac) = Rtl8168Nic::mac() else {
            serial_println!("{}   smoltcp bind SKIPPED — no NIC registered ::", P4);
            return;
        };
        let up = link_up();
        let mut dev = SmoltcpPhy::<Rtl8168Nic>::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = 0x4e45_5434; // ASCII "NET4"
        let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));

        // ── DHCP first: acquire a lease for the link's real subnet, else fall back to the static
        //    placeholder (NET-DHCP — the do-it-right fix for the NET-4-landing static bring-up IP). The
        //    helper configures the interface in place; the bounded witness poll below then exercises the
        //    seam against whichever config it settled on. ──
        let netcfg = crate::net_phy::dhcp_or_static(
            P4, &mut iface, &mut dev, &now_ms, dhcp_timeout_ms(), OUR_IP, 24, GATEWAY_IP,
        );

        // NET-4c: evidence snapshot right after the discover window — did the DISCOVER
        // actually LEAVE the NIC (TX consumed / ISR.TOK), and did anything at all land in the
        // RX ring during the window? Read-only; printed lease-or-not so both outcomes carry
        // the same evidence shape.
        if let Some(n) = NET4_DEVICE.lock().as_ref() {
            n.net4c_evidence("post-discover-window");
        }
        // NET-4d: close the RX-window classifier and emit the per-category summary (the RX-side proof
        // for the no-lease — did the OFFER arrive, and did the driver-visible accept path take it?).
        if let Some(n) = NET4_DEVICE.lock().as_mut() {
            n.net4d_window_close();
        }

        // One ICMP socket, so the poll has a real socket set to service (proves the full seam binds).
        let mut rx_meta = [icmp::PacketMetadata::EMPTY; 4];
        let mut rx_payload = [0u8; 256];
        let mut tx_meta = [icmp::PacketMetadata::EMPTY; 4];
        let mut tx_payload = [0u8; 256];
        let rx_buffer = icmp::PacketBuffer::new(&mut rx_meta[..], &mut rx_payload[..]);
        let tx_buffer = icmp::PacketBuffer::new(&mut tx_meta[..], &mut tx_payload[..]);
        let socket = icmp::Socket::new(rx_buffer, tx_buffer);
        let mut storage: [SocketStorage; 1] = Default::default();
        let mut sockets = SocketSet::new(&mut storage[..]);
        let _handle = sockets.add(socket);

        // Bounded poll — on metal this pumps ARP for the gateway; pre-subnet / empty-ring it is a
        // no-op. Kept small (this is a bind witness, not a traffic test): the attended sitting drives
        // real ICMP once the link's subnet is known.
        let mut clock: i64 = 0;
        while clock < 4096 {
            clock += 1;
            iface.poll(Instant::from_millis(clock), &mut dev, &mut sockets);
        }
        serial_println!(
            "{}   smoltcp 0.13 Interface BOUND over RTL8168: MAC set, {}.{}.{}.{}/{} + default gw {}.{}.{}.{} [{}], medium=ethernet, polled OK; link {} — live ICMP/ARP is attended-metal ::",
            P4,
            netcfg.ip[0], netcfg.ip[1], netcfg.ip[2], netcfg.ip[3], netcfg.prefix_len,
            netcfg.gw[0], netcfg.gw[1], netcfg.gw[2], netcfg.gw[3],
            if netcfg.leased { "dhcp" } else { "static" },
            if up { "UP" } else { "DOWN" }
        );
    }
}
