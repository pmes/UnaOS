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
// ## Write discipline
//
// The driver, being a driver, DOES the fabric writes NET-3 refused: it enables the device's
// MEM-decode + bus-master (command register), and programs the RTL8168 control/ring registers. Every
// config-space write is announced on serial before issue. It touches ONLY controller-0's downstream
// device (bus1:dev0:fn0) and that device's own register BAR — no other controller, no MSI/MSI-X, no
// other config function.

#![cfg(feature = "net4")]

use super::fdt_tegra::Fdt;

/// Stable serial prefix so the operator (and `mbench`) can grep the whole NET-4 bring-up as one block.
const P4: &str = ":: PCIE4:";

/// The Realtek vendor id and the RTL8168/8111 device id the NET-3 metal enumeration found.
const REALTEK_VENDOR: u16 = 0x10ec;
const RTL8168_DEVICE: u16 = 0x8168;

/// Poison patterns that mean ABSENT DECODE, never "present" (the PI-V3D-1 false-PASS lesson, shared
/// with the NET-1/2/3 recon): `0xffffffff` = master-abort / unclaimed config; `0xdeadbeef` = firmware
/// register/DRAM fill. A live config/register read is neither.
#[inline]
fn is_poison(v: u32) -> bool {
    v == 0xffff_ffff || v == 0xdead_beef
}

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
    use super::{is_poison, Fdt, P4, REALTEK_VENDOR, RTL8168_DEVICE};
    use crate::arch::aarch64::mmu_tegra::{map_mmio_window, MmioMap};
    use core::alloc::Layout;
    use core::ptr::{read_volatile, write_volatile};

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
    /// TPPoll: kick the normal-priority TX queue (NPQ) after posting a descriptor. (M3 `transmit`.)
    #[allow(dead_code)]
    const REG_TPPOLL: u64 = 0x38;
    #[allow(dead_code)]
    const TPPOLL_NPQ: u8 = 1 << 6;
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
    /// FS / LS: First / Last Segment (a single-buffer frame sets both). TX only. (M3 `transmit`.)
    #[allow(dead_code)]
    const DESC_FS: u32 = 1 << 29;
    #[allow(dead_code)]
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
        // The ring cursors + counters are advanced by the M3 raw_rx/transmit poll path.
        #[allow(dead_code)]
        rx_cur: usize,
        #[allow(dead_code)]
        rx_count: u64,
        tx_ring: *mut Desc,
        tx_buffers: *mut u8,
        #[allow(dead_code)]
        tx_cur: usize,
        #[allow(dead_code)]
        tx_count: u64,
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
    }

    /// Format a MAC as `xx:xx:xx:xx:xx:xx` for the boot log (no heap — a fixed stack buffer).
    fn fmt_mac(mac: &[u8; 6]) -> [u8; 17] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = [b':'; 17];
        for i in 0..6 {
            out[i * 3] = HEX[(mac[i] >> 4) as usize];
            out[i * 3 + 1] = HEX[(mac[i] & 0xf) as usize];
        }
        out
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
        serial_println!(
            "{}   register BAR2 = {:#x} ({}-bit {}) ::",
            P4, bar_base, if is_64bit { "64" } else { "32" },
            if (bar2 >> 3) & 1 == 1 { "prefetchable mem" } else { "mem" }
        );
        if bar_base == 0 {
            serial_println!("{}   BAR2 base is 0 (firmware left it unassigned) — bring-up SKIPPED ::", P4);
            return;
        }
        // Map the 4 KiB register window (BAR2 sized to 0x1000). The Tegra MMIO ranges live ~200 GiB,
        // within the PS-widened 40-bit / 512-GiB-table reach.
        let bar_size = 0x1000usize;
        match map_mmio_window(bar_base, bar_size) {
            MmioMap::Mapped | MmioMap::AlreadyMapped => {
                serial_println!("{}   BAR2 {:#x} (+{:#x}) mapped Device-nGnRE — registers reachable ::", P4, bar_base, bar_size);
            }
            MmioMap::BeyondPsCeiling => {
                serial_println!("{}   BAR2 {:#x} BEYOND the PS ceiling — cannot map register window; bring-up SKIPPED ::", P4, bar_base);
                return;
            }
        }

        // ── Construct the driver, reset the MAC, read the station MAC (M1) ──
        let mut nic = Rtl8168 {
            mmio_base: bar_base,
            mac: [0; 6],
            rx_ring: core::ptr::null_mut(),
            rx_buffers: core::ptr::null_mut(),
            rx_cur: 0,
            rx_count: 0,
            tx_ring: core::ptr::null_mut(),
            tx_buffers: core::ptr::null_mut(),
            tx_cur: 0,
            tx_count: 0,
        };
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

        // Register the driver so the smoltcp bind (M3) + any poll path can reach it.
        *NET4_DEVICE.lock() = Some(nic);
        serial_println!(
            "{} ORIN-NET-4 M2 DONE — RTL8168 @ BAR2 {:#x}, MAC read, C+ rings up + RX/TX enabled (smoltcp bind = M3) ::",
            P4, bar_base
        );
    }

    /// The one registered RTL8168 NIC (populated by [`net4_bringup`]). Mirrors the x86 e1000
    /// `NET_DEVICE` registry; the smoltcp Device adapter (M3) reaches the rings through it.
    pub static NET4_DEVICE: spin::Mutex<Option<Rtl8168>> = spin::Mutex::new(None);
}
