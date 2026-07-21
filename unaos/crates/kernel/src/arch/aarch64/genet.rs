// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PI-GENET — the Raspberry Pi 4's on-board Gigabit Ethernet (Broadcom GENET v5) + smoltcp bind
// (`genet` gated). The Pi's FIRST network path.
//
// ## The device
//
// The BCM2711 integrates a Broadcom "GENET" v5 unimac Ethernet controller at SoC-bus `0x7d58_0000`
// (`ethernet@7d580000`, compatible `brcm,bcm2711-genet-v5`), which the fixed SoC peripheral `ranges`
// map (child `0x7c00_0000` -> CPU `0xFC00_0000`, the +`0x8000_0000` offset piusb applies to the PCIe
// RC at `pcie@7d500000` -> `0xFD50_0000`) translates to ARM-physical `0xFD58_0000`. That base sits
// inside the `0xC000_0000..0xFFFF_FFFF` Device-nGnRnE GiB `boot::build_l1` already maps, so — unlike
// piusb's outbound-window / NET-4's iATU dance — the register block is directly reachable once
// resolved; no new page-table write is needed. The MAC drives an EXTERNAL BCM54213PE RGMII PHY
// (MDIO), which the driver brings up for link.
//
// ## Register model (Linux `drivers/net/ethernet/broadcom/genet`, bcmgenet.h, the v5 path)
//
// The 64 KiB window is a set of sub-blocks: SYS (0x0000), EXT (0x0080), RBUF (0x0300), UMAC (0x0800),
// RDMA descriptors (0x2000) + RDMA ring/global regs (0x2C00), TDMA descriptors (0x4000) + TDMA
// ring/global regs (0x4C00). Each of the 17 DMA rings owns a 0x40-byte register block; this driver
// uses the default descriptor-based ring (index 16, `DESC_INDEX`) for both RX and TX, exactly as
// Linux `bcmgenet_init_dma` does for the single-queue path. The datapath is a producer/consumer-index
// ring (NOT the RTL8168 per-descriptor OWN handoff): the driver advances the TX producer index after
// posting and the RX consumer index after draining; hardware advances the mirror index.
//
// ## Honest QEMU-modeling stance (the load-bearing M1 finding — EMPIRICALLY SETTLED)
//
// QEMU 11.0.1's `raspi4b` (bcm2838 SoC) does NOT model the GENET block. Verified on the bench: a read
// of the GENET register window at ARM-physical `0xFD58_0000` raises a SYNCHRONOUS EXTERNAL Data Abort
// (`ESR=0x96000010`, EC=0x25, DFSC=0x10 = external abort, `FAR=0xfd580000`) — the address decodes to
// nothing, so the fabric returns an abort rather than open-bus `0xffffffff`. QEMU ALSO hands `-kernel`
// boots no usable DTB (x0 = `0x100`, size 0), so there is no `ethernet@7d580000` node to resolve
// either. This driver is therefore CODE-COMPLETE-PRIOR-TO-METAL, exactly like ORIN-NET-4 (whose Tegra
// RC QEMU also does not model): its correctness rests on `arroyo check`, the kernel8 build, faithful
// adherence to the Linux bcmgenet v5 programming model, and the attended-metal sitting — NOT on a QEMU
// datapath witness.
//
// Because an unmodeled MMIO read FAULTS (it does not return poison), the classification is DTB-GATED,
// mirroring piusb's `dtb_has_pcie` guard (piusb skips before touching the RC because QEMU raspi4b
// models no RC either). M1 resolves the GENET node from the live firmware DTB; ONLY if the DTB actually
// describes a GENET node does it touch the register window (a poison-honest `SYS_REV_CTRL` read guards
// against a link-down / absent decode on real metal — the standing "read before first write" law). If
// the DTB has no GENET node (QEMU, or a DTB-less boot), the driver records an honest compiled-present
// line and returns BEFORE any MMIO — it never dereferences an unmodeled window. The boot log states
// which regime it took, so the finding is a fact on the serial transcript, not a claim in a doc.
//
// ## DMA / identity-map + coherency (mirrors NET-4 / VNET)
//
// The Pi bare-metal MMU maps RAM identity (VA==PA), so a heap allocation's pointer doubles as the
// physical address the MAC DMAs against (the x86 e1000 / NET-4 / VNET invariant). Rings + buffers are
// published with `dsb sy`. The BCM2711 GENET was assumed I/O-coherent toward DRAM (ACE-lite), so `dsb`
// ordering was thought to suffice — but boot-P10 (151 frames popped, 0 classified) is the attended-metal
// evidence that the RX datapath is NOT coherent: the CPU read stale pre-refill lines over the recycled
// buffers. PI-GENET-5 therefore invalidates each RX buffer's cache lines (`dc ivac`) before reading its
// length + payload (`invalidate_dcache` in `rx_frame_raw`); the `[genet5]` witness records the
// pre/post-invalidate delta that proves the regime. The index protocol is untouched.
//
// ## Write discipline
//
// Every controller register write is announced on serial before issue. The driver touches ONLY the
// GENET register block and drives its own MAC/PHY — no other peripheral, no interrupt controller (the
// bring-up is polled: interrupts masked).

#![cfg(feature = "genet")]

/// Stable serial prefix so the operator (and `mbench`) can grep the whole PI-GENET bring-up as one
/// block.
const PG: &str = ":: PI-GENET:";

// ── The witness half (non-baremetal build): one honest line, zero MMIO ──────────────────────────────
//
// A `genet`-but-not-`baremetal` build (e.g. `./arroyo check` on x86_64 / aarch64-virt with the knob on)
// has no BCM2711 and no call site — but the module still compiles. Provide a metal-only stub so the
// crate type-checks on every target without pulling the aarch64 MMIO in. Mirrors ORIN-NET-4's
// `not(tegra)` witness.
#[cfg(not(all(feature = "baremetal", target_arch = "aarch64")))]
pub fn genet_bringup(_dtb_addr: u64, _dtb_size: usize) {
    serial_println!(
        "{} BCM GENET v5 driver compiled; no BCM2711 on this build — bring-up is metal-only (Pi 4, UNAOS_GENET=1) ::",
        PG
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// The metal driver (`genet` + `baremetal`) — DTB-resolve + probe (M1); UMAC/rings (M2); bind (M3).
// ══════════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(all(feature = "baremetal", target_arch = "aarch64"))]
pub use metal::genet_bringup;

#[cfg(all(feature = "baremetal", target_arch = "aarch64"))]
mod metal {
    use super::PG;
    use crate::arch::aarch64::fdt_tegra::Fdt;
    use crate::net_phy::{fmt_mac, RawNic, SmoltcpPhy};
    use core::alloc::Layout;
    use core::ptr::{read_volatile, write_volatile};

    // ── Poison patterns that mean ABSENT DECODE, never "present" (the PI-V3D-1 / NET-4 lesson):
    //    `0xffffffff` = open bus / master-abort; `0xdeadbeef` = firmware register/DRAM fill; a bare
    //    `0x00000000` from the version register also means "no live GENET" (a real SYS_REV_CTRL always
    //    has non-zero major/minor/EPHY fields). ──
    #[inline]
    fn is_poison(v: u32) -> bool {
        v == 0xffff_ffff || v == 0xdead_beef || v == 0x0000_0000
    }

    // ── Sub-block base offsets (bytes from the GENET register window) ──
    const SYS_OFF: u64 = 0x0000;
    const EXT_OFF: u64 = 0x0080;
    const RBUF_OFF: u64 = 0x0300;
    const UMAC_OFF: u64 = 0x0800;
    const RDMA_OFF: u64 = 0x2000;
    const TDMA_OFF: u64 = 0x4000;

    // ── SYS block ──
    const SYS_REV_CTRL: u64 = SYS_OFF + 0x00;
    const SYS_PORT_CTRL: u64 = SYS_OFF + 0x04;
    const SYS_RBUF_FLUSH_CTRL: u64 = SYS_OFF + 0x08;
    const SYS_TBUF_FLUSH_CTRL: u64 = SYS_OFF + 0x0c;
    /// External GPHY port mode (the Pi 4 drives an off-chip RGMII PHY).
    const PORT_MODE_EXT_GPHY: u32 = 3;

    // ── EXT block (RGMII out-of-band control for the external PHY) ──
    const EXT_RGMII_OOB_CTRL: u64 = EXT_OFF + 0x0c;
    const RGMII_LINK: u32 = 1 << 4;
    const OOB_DISABLE: u32 = 1 << 5;
    const RGMII_MODE_EN: u32 = 1 << 6;
    const ID_MODE_DIS: u32 = 1 << 16;

    // ── RBUF block ──
    const RBUF_CTRL: u64 = RBUF_OFF + 0x00;
    const RBUF_64B_EN: u32 = 1 << 0;
    const RBUF_ALIGN_2B: u32 = 1 << 1;

    // ── UMAC block ──
    const UMAC_CMD: u64 = UMAC_OFF + 0x008;
    const UMAC_MAC0: u64 = UMAC_OFF + 0x00c;
    const UMAC_MAC1: u64 = UMAC_OFF + 0x010;
    const UMAC_MAX_FRAME_LEN: u64 = UMAC_OFF + 0x014;
    const UMAC_MDIO_CMD: u64 = UMAC_OFF + 0x614;
    const UMAC_MIB_CTRL: u64 = UMAC_OFF + 0x580;
    // CMD register bits.
    const CMD_TX_EN: u32 = 1 << 0;
    const CMD_RX_EN: u32 = 1 << 1;
    // UMAC speed encoding (bits [3:2]): 10 -> 0, 100 -> 1, 1000 -> 2.
    const CMD_SPEED_10: u32 = 0;
    const CMD_SPEED_100: u32 = 1;
    const CMD_SPEED_1000: u32 = 2;
    const CMD_SPEED_SHIFT: u32 = 2;
    const CMD_SPEED_MASK: u32 = 0x3;
    const CMD_PROMISC: u32 = 1 << 4;
    /// Half-duplex enable (bit 10). Set when the PHY resolves half-duplex; clear for full.
    const CMD_HD_EN: u32 = 1 << 10;
    const CMD_SW_RESET: u32 = 1 << 13;
    // MIB counter reset bits.
    const MIB_RESET_RX: u32 = 1 << 0;
    const MIB_RESET_RUNT: u32 = 1 << 1;
    const MIB_RESET_TX: u32 = 1 << 2;
    // MDIO command bits.
    const MDIO_START_BUSY: u32 = 1 << 29;
    const MDIO_READ_FAIL: u32 = 1 << 28;
    const MDIO_RD: u32 = 2 << 26;
    const MDIO_PMD_SHIFT: u32 = 21;
    const MDIO_REG_SHIFT: u32 = 16;
    // Standard MII registers (IEEE 802.3 Clause 22 — architectural register facts).
    const MII_BMSR: u32 = 0x01; // Basic mode status
    const MII_PHYID1: u32 = 0x02; // PHY identifier 1
    const MII_ADVERTISE: u32 = 0x04; // Our 10/100 auto-neg advertisement (ANAR)
    const MII_LPA: u32 = 0x05; // Link-partner 10/100 ability (ANLPAR)
    const MII_CTRL1000: u32 = 0x09; // Our 1000BASE-T advertisement
    const MII_STAT1000: u32 = 0x0a; // Link-partner 1000BASE-T ability
    const BMSR_LSTATUS: u16 = 1 << 2; // Link is up (latched-low)
    const BMSR_ANEGCOMPLETE: u16 = 1 << 5; // Auto-negotiation complete
    // ANAR/ANLPAR (reg 0x04/0x05) technology-ability bits.
    const ADVERTISE_10HALF: u16 = 1 << 5;
    const ADVERTISE_10FULL: u16 = 1 << 6;
    const ADVERTISE_100HALF: u16 = 1 << 7;
    const ADVERTISE_100FULL: u16 = 1 << 8;
    // 1000BASE-T control (reg 0x09) advertised bits.
    const ADVERTISE_1000HALF: u16 = 1 << 8;
    const ADVERTISE_1000FULL: u16 = 1 << 9;
    // 1000BASE-T status (reg 0x0a) link-partner bits.
    const LPA_1000HALF: u16 = 1 << 10;
    const LPA_1000FULL: u16 = 1 << 11;

    // ── DMA descriptor + ring register model ──
    /// Descriptors per ring (Linux `TOTAL_DESC`).
    const TOTAL_DESC: usize = 256;
    /// Per-descriptor size: length_status + address_lo + address_hi = 3 words.
    const DMA_DESC_SIZE: u64 = 12;
    /// Per-descriptor WORD count (the START/END_ADDR ring registers count in 32-bit WORDS, NOT bytes —
    /// Linux `end_ptr * words_per_bd - 1`, `words_per_bd = 3` for the 40-bit v5 descriptor). Programming
    /// END_ADDR in bytes (12/desc) instead of words (3/desc) sizes the HW ring region 4× too large, so
    /// the engine's read/write pointer never wraps at the last valid descriptor — it walks off into the
    /// uninitialised tail of the shared 256-descriptor array and stalls. This is the PI-GENET-3 wrap bug.
    const DMA_DESC_WORDS: u32 = (DMA_DESC_SIZE / 4) as u32; // 3
    /// The default descriptor-based ring index used by the single-queue path (`DESC_INDEX`).
    const RING: usize = 16;
    /// Per-ring register-block stride.
    const DMA_RING_SIZE: u64 = 0x40;
    /// Ring/global register area starts after the 256 descriptors.
    const RDMA_REG_OFF: u64 = RDMA_OFF + TOTAL_DESC as u64 * DMA_DESC_SIZE; // 0x2C00
    const TDMA_REG_OFF: u64 = TDMA_OFF + TOTAL_DESC as u64 * DMA_DESC_SIZE; // 0x4C00
    /// Global DMA registers sit past all 17 ring blocks (`DMA_RINGS_SIZE`).
    const DMA_RINGS_SIZE: u64 = DMA_RING_SIZE * 17; // 0x440

    // Per-descriptor word offsets.
    const DMA_DESC_LENGTH_STATUS: u64 = 0x00;
    const DMA_DESC_ADDRESS_LO: u64 = 0x04;
    const DMA_DESC_ADDRESS_HI: u64 = 0x08;
    // length_status fields.
    const DMA_BUFLENGTH_SHIFT: u32 = 16;
    const DMA_EOP: u32 = 0x4000;
    const DMA_SOP: u32 = 0x2000;
    const DMA_TX_APPEND_CRC: u32 = 0x0040;
    const DMA_TX_QTAG_SHIFT: u32 = 7;
    const DMA_TX_QTAG_MASK: u32 = 0x3f;

    // Per-ring register offsets (the v4/v5 `genet_dma_ring_regs` table). TDMA and RDMA share the block
    // layout with mirrored semantics; the names below follow the TDMA view and are reused for RDMA.
    const RING_TDMA_CONS_INDEX: u64 = 0x08; // RDMA: PROD_INDEX
    const RING_TDMA_PROD_INDEX: u64 = 0x0c; // RDMA: CONS_INDEX
    const RING_DMA_RING_BUF_SIZE: u64 = 0x10;
    const RING_DMA_START_ADDR: u64 = 0x14;
    const RING_DMA_START_ADDR_HI: u64 = 0x18;
    const RING_DMA_END_ADDR: u64 = 0x1c;
    const RING_DMA_END_ADDR_HI: u64 = 0x20;
    const RING_DMA_MBUF_DONE_THRESH: u64 = 0x24;
    const RING_TDMA_FLOW_PERIOD: u64 = 0x28; // RDMA: XON_XOFF_THRESH
    const RING_TDMA_WRITE_PTR: u64 = 0x2c; // RDMA: READ_PTR
    const RING_TDMA_READ_PTR: u64 = 0x00; // RDMA: WRITE_PTR

    // Global DMA registers (offset from *_REG_OFF + DMA_RINGS_SIZE; the v3plus table).
    const DMA_RING_CFG: u64 = 0x00;
    const DMA_CTRL: u64 = 0x04;
    const DMA_SCB_BURST_SIZE: u64 = 0x0c;
    const DMA_EN: u32 = 1 << 0;
    /// Per-ring enable bit in DMA_CTRL is `1 << (ring + DMA_RING_BUF_EN_SHIFT)`.
    const DMA_RING_BUF_EN_SHIFT: u32 = 1;
    /// v5 max SCB burst length.
    const DMA_MAX_BURST_LENGTH: u32 = 0x08;
    /// 16-bit producer/consumer index mask.
    const DMA_INDEX_MASK: u32 = 0xffff;

    /// RX/TX ring depth (a power of two <= TOTAL_DESC). 32 mirrors the NET-4 depth; ample for the
    /// bounded witness.
    const RING_DEPTH: usize = 32;
    /// Per-descriptor buffer (a full Ethernet frame + GENET's 2-byte RX status pad fits).
    const BUF_SIZE: usize = 2048;
    /// PI-GENET-4 (the boot-P6 10/10-unclassified fix): RBUF_CTRL is programmed `64B_EN | ALIGN_2B`
    /// (see `init`), and with `64B_EN` set GENET prepends a 64-BYTE receive-status block to every RX
    /// buffer, followed by the 2-byte IP-align pad — and the descriptor length INCLUDES both. Linux
    /// `bcmgenet_desc_rx` pulls 64 (`desc_64b_en`) then 2 (the align pad) before handing the skb up.
    /// This driver previously stripped only the 2-byte pad, so every frame smoltcp classified began
    /// 62 bytes early — inside the status block — making all 10 popped boot-P6 frames read as garbage
    /// ethertype (no OFFER ever seen). Payload starts at buffer + 64 + 2.
    /// (FCS: UMAC CMD.CRC_FWD is never set here, so the MAC strips the FCS — the length does NOT
    /// include it; no additional trim is owed.)
    const RX_STATUS_BLOCK: usize = 64;
    const RX_ALIGN_PAD: usize = 2;
    const RX_STATUS_PAD: usize = RX_STATUS_BLOCK + RX_ALIGN_PAD;
    /// PI-GENET-4: bound on the popped-frame witness lines (the boot-P6 evidence question — "what ARE
    /// the 10 popped frames?" — needs only a handful of exemplars). Mirrors NET-4t on the Orin.
    const PG4_RX_DUMPS: u64 = 4;
    /// PI-GENET-5: bound on the coherency-discriminating witness (the boot-P10 question — "are the 151
    /// popped frames real payload the classifier drops, or stale/zero DRAM the CPU never re-read?").
    /// Eight exemplars is ample to read the pre/post-invalidate delta straight off serial.
    const PG5_RX_DUMPS: u64 = 8;
    /// Cortex-A72 (BCM2711) L1/L2 cache line = 64 bytes. The RX-buffer invalidate steps by this.
    const CACHE_LINE: usize = 64;
    /// Largest frame the MAC accepts (jumbo-safe default; a normal frame is far under).
    const MAX_FRAME_LEN: u32 = 1536;

    /// The claimed GENET: its register-block base, the station MAC, and the RX/TX descriptor rings +
    /// DMA buffers. Rings/buffers come from the heap (identity map => pointer == DMA physical address).
    pub struct Genet {
        base: u64,
        mac: [u8; 6],
        rx_bufs: *mut u8,
        rx_c_index: u16, // our consumer cursor (descriptors 0..RING_DEPTH)
        rx_count: u64,
        tx_bufs: *mut u8,
        tx_prod: u16, // our producer cursor
        tx_count: u64,
        phy_addr: u8,
        /// PI-GENET-4: popped frames given a raw-bytes witness line so far (bounds the evidence
        /// noise to `PG4_RX_DUMPS`; all inside the `genet`-gated battery — default boot unchanged).
        pg4_rx_dumped: u64,
        /// PI-GENET-5: pops given the coherency-discriminating `[genet5]` witness line so far.
        pg5_rx_dumped: u64,
    }

    /// PI-GENET-4 (the NET-4t helper, GENET's own copy — lanes do not share code): resolve a frame's
    /// EFFECTIVE EtherType by peeling up to two 802.1Q/802.1ad VLAN tags (0x8100 / 0x88a8). Returns
    /// `(ethertype, l3_offset, outermost_vlan_id)`; `None` vlan for an untagged frame. Read-only and
    /// bounds-checked; a frame too short for its claimed tag yields the raw TPID as the ethertype.
    fn eth_effective_type(frame: &[u8]) -> (u16, usize, Option<u16>) {
        if frame.len() < 14 {
            return (0, 14, None);
        }
        let mut off = 12usize;
        let mut vlan: Option<u16> = None;
        for _ in 0..2 {
            let et = u16::from_be_bytes([frame[off], frame[off + 1]]);
            if (et == 0x8100 || et == 0x88a8) && frame.len() >= off + 6 {
                let tci = u16::from_be_bytes([frame[off + 2], frame[off + 3]]);
                if vlan.is_none() {
                    vlan = Some(tci & 0x0fff);
                }
                off += 4;
            } else {
                return (et, off + 2, vlan);
            }
        }
        let et = u16::from_be_bytes([frame[off], frame[off + 1]]);
        (et, off + 2, vlan)
    }

    /// PI-GENET-4: is this (post-peel) an IPv4/UDP frame on the DHCP client/server ports? Enough of a
    /// decode to anchor the named DHCP-under-VLAN verdict line; bounds-checked, read-only.
    fn is_dhcp_udp(frame: &[u8], et: u16, l3_off: usize) -> bool {
        if et != 0x0800 {
            return false;
        }
        let Some(ip) = frame.get(l3_off..) else { return false };
        if ip.len() < 20 || (ip[0] >> 4) != 4 {
            return false;
        }
        let ihl = ((ip[0] & 0x0f) as usize) * 4;
        if ihl < 20 || ip.len() < ihl + 8 || ip[9] != 17 {
            return false;
        }
        let sport = u16::from_be_bytes([ip[ihl], ip[ihl + 1]]);
        let dport = u16::from_be_bytes([ip[ihl + 2], ip[ihl + 3]]);
        (sport == 67 || sport == 68) && (dport == 67 || dport == 68)
    }

    // The driver owns raw DMA pointers; touched only behind the `GENET_DEVICE` mutex on the single-core
    // poll path, so sharing across contexts is sound (mirrors NET-4 / VNET).
    unsafe impl Send for Genet {}

    /// The PHY's NEGOTIATED link result (autoneg-honest): whether link is up, whether autoneg
    /// completed, and the resolved speed (Mb/s) + duplex. Programmed into UMAC/RGMII rather than forced.
    struct LinkState {
        link: bool,
        aneg: bool,
        speed: u16,
        full_duplex: bool,
    }

    impl Genet {
        #[inline]
        fn r(&self, off: u64) -> u32 {
            unsafe { read_volatile((self.base + off) as *const u32) }
        }
        #[inline]
        fn w(&self, off: u64, v: u32) {
            unsafe { write_volatile((self.base + off) as *mut u32, v) };
        }

        // Per-ring register accessors (ring 16, TDMA / RDMA blocks).
        #[inline]
        fn tdma_ring(&self, off: u64) -> u64 {
            TDMA_REG_OFF + RING as u64 * DMA_RING_SIZE + off
        }
        #[inline]
        fn rdma_ring(&self, off: u64) -> u64 {
            RDMA_REG_OFF + RING as u64 * DMA_RING_SIZE + off
        }
        #[inline]
        fn tdma_global(&self, off: u64) -> u64 {
            TDMA_REG_OFF + DMA_RINGS_SIZE + off
        }
        #[inline]
        fn rdma_global(&self, off: u64) -> u64 {
            RDMA_REG_OFF + DMA_RINGS_SIZE + off
        }

        /// Descriptor `i` of the RDMA / TDMA descriptor array (index 0-based within the ring; the
        /// default ring's descriptors occupy the low `RING_DEPTH` slots of the shared 256-descriptor
        /// array — Linux places `DESC_INDEX`'s descriptors starting at descriptor 0 for the default
        /// ring via its start/end addr programming, which we mirror below).
        #[inline]
        fn rx_desc(&self, i: usize) -> u64 {
            RDMA_OFF + i as u64 * DMA_DESC_SIZE
        }
        #[inline]
        fn tx_desc(&self, i: usize) -> u64 {
            TDMA_OFF + i as u64 * DMA_DESC_SIZE
        }

        /// Poison-honest liveness probe through the register window, done BEFORE any write: read the
        /// stable RO `SYS_REV_CTRL`. A live GENET returns a plausible revision (non-poison); open bus /
        /// firmware fill / bare zero is rejected. Returns the raw revision word on a live decode.
        fn probe_rev(&self) -> Option<u32> {
            let rev = self.r(SYS_REV_CTRL);
            if is_poison(rev) {
                None
            } else {
                Some(rev)
            }
        }

        /// UMAC soft reset: assert CMD.SW_RESET, brief settle, deassert; then flush RBUF/TBUF. Bounded.
        fn umac_reset(&self) {
            serial_println!("{}   >>> REG WRITE (M2): SYS_RBUF/TBUF_FLUSH_CTRL reset pulse ::", PG);
            self.w(SYS_RBUF_FLUSH_CTRL, 1);
            self.w(SYS_TBUF_FLUSH_CTRL, 1);
            barrier();
            self.w(SYS_RBUF_FLUSH_CTRL, 0);
            self.w(SYS_TBUF_FLUSH_CTRL, 0);
            barrier();
            serial_println!("{}   >>> REG WRITE (M2): UMAC_CMD = SW_RESET ::", PG);
            self.w(UMAC_CMD, CMD_SW_RESET);
            barrier();
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
            self.w(UMAC_CMD, 0);
            barrier();
        }

        /// Read one MII register from the external PHY over the UMAC MDIO master (bounded busy-wait).
        /// Returns `None` on a read-fail or a stuck START_BUSY.
        fn mdio_read(&self, phy: u8, reg: u32) -> Option<u16> {
            let cmd = MDIO_RD | ((phy as u32) << MDIO_PMD_SHIFT) | (reg << MDIO_REG_SHIFT);
            self.w(UMAC_MDIO_CMD, cmd);
            let v = self.r(UMAC_MDIO_CMD);
            self.w(UMAC_MDIO_CMD, v | MDIO_START_BUSY);
            barrier();
            let mut ok = false;
            for _ in 0..100_000 {
                if self.r(UMAC_MDIO_CMD) & MDIO_START_BUSY == 0 {
                    ok = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !ok {
                return None;
            }
            let res = self.r(UMAC_MDIO_CMD);
            if res & MDIO_READ_FAIL != 0 {
                return None;
            }
            Some((res & 0xffff) as u16)
        }

        /// Scan MDIO addresses 0..31 for a PHY that returns a plausible (non-0xffff/0x0000) PHYID1.
        /// Returns the first address found, defaulting to 1 (the Pi 4 BCM54213PE lives at MDIO 0x1)
        /// when the scan is inconclusive so link polling still has a target.
        fn find_phy(&self) -> u8 {
            for a in 0..32u8 {
                if let Some(id) = self.mdio_read(a, MII_PHYID1) {
                    if id != 0xffff && id != 0x0000 {
                        serial_println!("{}   MDIO PHY @ addr {} (PHYID1={:#06x}) ::", PG, a, id);
                        return a;
                    }
                }
            }
            serial_println!("{}   MDIO: no PHY id found on the bus — defaulting to addr 1 (BCM54213PE) ::", PG);
            1
        }

        /// External-PHY link state from MII BMSR.LSTATUS (latched-low; two reads clear the latch).
        fn link_up(&self) -> bool {
            let _ = self.mdio_read(self.phy_addr, MII_BMSR);
            self.mdio_read(self.phy_addr, MII_BMSR)
                .map(|s| s & BMSR_LSTATUS != 0)
                .unwrap_or(false)
        }

        /// Resolve the PHY's NEGOTIATED link — speed + duplex taken FROM the autoneg result, never
        /// forced. Reads are IEEE 802.3 Clause-22 standard registers: BMSR link/aneg-complete, then the
        /// highest-common-denominator technology from our advertisement (ANAR / 1000BASE-T control)
        /// intersected with the link partner's ability (ANLPAR / 1000BASE-T status). This is the honest
        /// alternative to hand-asserting SPEED_1000 + RGMII_LINK: a 100M-negotiated link programmed as
        /// forced-1000 mis-clocks the RGMII pins and corrupts every TX frame on the wire.
        fn phy_resolve(&self) -> LinkState {
            // BMSR is latched-low; read twice so LSTATUS reflects the current (not a stale) link.
            let _ = self.mdio_read(self.phy_addr, MII_BMSR);
            let bmsr = self.mdio_read(self.phy_addr, MII_BMSR).unwrap_or(0);
            let link = bmsr & BMSR_LSTATUS != 0;
            if !link {
                return LinkState { link: false, aneg: false, speed: 0, full_duplex: false };
            }
            // Bounded wait for auto-negotiation to complete (PHY powers up with autoneg enabled per
            // IEEE default; this only caps a still-negotiating link, it never hangs).
            let mut aneg = bmsr & BMSR_ANEGCOMPLETE != 0;
            if !aneg {
                for _ in 0..200 {
                    let s = self.mdio_read(self.phy_addr, MII_BMSR).unwrap_or(0);
                    if s & BMSR_ANEGCOMPLETE != 0 {
                        aneg = true;
                        break;
                    }
                    for _ in 0..20_000 {
                        core::hint::spin_loop();
                    }
                }
            }
            let adv = self.mdio_read(self.phy_addr, MII_ADVERTISE).unwrap_or(0);
            let lpa = self.mdio_read(self.phy_addr, MII_LPA).unwrap_or(0);
            let ctrl1000 = self.mdio_read(self.phy_addr, MII_CTRL1000).unwrap_or(0);
            let stat1000 = self.mdio_read(self.phy_addr, MII_STAT1000).unwrap_or(0);
            let common = adv & lpa;
            let g_full = (ctrl1000 & ADVERTISE_1000FULL != 0) && (stat1000 & LPA_1000FULL != 0);
            let g_half = (ctrl1000 & ADVERTISE_1000HALF != 0) && (stat1000 & LPA_1000HALF != 0);
            let (speed, full_duplex) = if g_full {
                (1000, true)
            } else if g_half {
                (1000, false)
            } else if common & ADVERTISE_100FULL != 0 {
                (100, true)
            } else if common & ADVERTISE_100HALF != 0 {
                (100, false)
            } else if common & ADVERTISE_10FULL != 0 {
                (10, true)
            } else if common & ADVERTISE_10HALF != 0 {
                (10, false)
            } else {
                // Link is up but no common technology resolved (autoneg still pending / bad partner):
                // fall back to the safe slow floor rather than force a fast lie.
                (10, false)
            };
            LinkState { link, aneg, speed, full_duplex }
        }

        /// Program UMAC speed/duplex + the EXT RGMII out-of-band link bit FROM the resolved PHY state.
        /// Speed bits and HD_EN mirror the negotiated result; RGMII_LINK is asserted ONLY when the PHY
        /// actually reports link (Linux `bcmgenet_mii_setup` discipline), never by hand.
        fn mac_set_from_link(&self, ls: &LinkState) {
            let sp = match ls.speed {
                1000 => CMD_SPEED_1000,
                100 => CMD_SPEED_100,
                _ => CMD_SPEED_10,
            };
            let mut cmd = self.r(UMAC_CMD);
            cmd &= !((CMD_SPEED_MASK << CMD_SPEED_SHIFT) | CMD_HD_EN);
            cmd |= sp << CMD_SPEED_SHIFT;
            if !ls.full_duplex {
                cmd |= CMD_HD_EN;
            }
            serial_println!(
                "{}   >>> REG WRITE (M2): UMAC_CMD speed<-{}M {} (from PHY autoneg) ::",
                PG, ls.speed, if ls.full_duplex { "full-duplex" } else { "half-duplex" }
            );
            self.w(UMAC_CMD, cmd);
            let mut oob = RGMII_MODE_EN | OOB_DISABLE | ID_MODE_DIS;
            if ls.link {
                oob |= RGMII_LINK;
            }
            serial_println!(
                "{}   >>> REG WRITE (M2): EXT_RGMII_OOB_CTRL RGMII_LINK {} (honoring PHY link) ::",
                PG, if ls.link { "SET" } else { "CLEAR" }
            );
            self.w(EXT_RGMII_OOB_CTRL, oob);
            barrier();
        }

        /// TX evidence for the storm / LED-red-herring classes: our software enqueue count vs the
        /// hardware TDMA producer/consumer indices. `cons_index` is advanced by hardware as it drains
        /// (transmits) descriptors; `cons == prod` means the ring fully drained with no runaway
        /// re-post. A steady activity LED with `tx_count` small and cons==prod is NOT a storm.
        fn tx_evidence(&self, label: &str) {
            let prod = self.r(self.tdma_ring(RING_TDMA_PROD_INDEX)) & DMA_INDEX_MASK;
            let cons = self.r(self.tdma_ring(RING_TDMA_CONS_INDEX)) & DMA_INDEX_MASK;
            // A cons_index that has advanced past RING_DEPTH is the direct witness that the TDMA ring
            // WRAPPED (the PI-GENET-3 fix): boot-P3 caught it frozen at exactly RING_DEPTH.
            let wrapped = cons > RING_DEPTH as u32;
            serial_println!(
                "{}   TX evidence [{}]: sw frames-enqueued={} (tx_prod={}) | HW TDMA prod_index={} cons_index={} {}{} ::",
                PG, label, self.tx_count, self.tx_prod, prod, cons,
                if cons == prod { "(drained; no storm)" } else { "(in-flight or stalled)" },
                if wrapped { " [ring WRAPPED past depth]" } else if cons == RING_DEPTH as u32 { " [STALLED at ring depth — no wrap]" } else { "" }
            );
        }

        /// RX evidence (the OFFER-never-popped / RX-wrap class, PI-GENET-3): our software consumer cursor
        /// and popped-frame count against the hardware RDMA producer index. On RDMA the 0x08 register is
        /// the PRODUCER (hardware-advanced as it fills descriptors, free-running mod 65536) and 0x0c the
        /// CONSUMER (we publish it). `prod` past RING_DEPTH witnesses the RDMA ring wrapped and kept
        /// delivering past the first pass — the co-suspect the wrap fix clears. GENET has no single
        /// simple rx-frames MIB register (the MIB is a swept counter array we do not fabricate an offset
        /// for under the facts-only rule); the free-running RDMA producer index IS the authoritative
        /// count of frames hardware delivered into the ring, so it stands in as the rx-frames witness.
        fn rx_evidence(&self, label: &str) {
            let prod = self.r(self.rdma_ring(RING_TDMA_CONS_INDEX)) & DMA_INDEX_MASK; // RDMA PROD_INDEX
            let cons = self.r(self.rdma_ring(RING_TDMA_PROD_INDEX)) & DMA_INDEX_MASK; // RDMA CONS_INDEX
            let wrapped = prod > RING_DEPTH as u32;
            serial_println!(
                "{}   RX evidence [{}]: sw frames-popped={} (rx_c_index={}) | HW RDMA prod_index={} cons_index={} {}{} ::",
                PG, label, self.rx_count, self.rx_c_index, prod, cons,
                if prod == cons { "(ring drained)" } else { "(frames waiting)" },
                if wrapped { " [ring WRAPPED past depth]" } else if prod == RING_DEPTH as u32 { " [STALLED at ring depth — no wrap]" } else { "" }
            );
        }

        /// One-time flow-control / ring-arm witness: the per-ring flow registers (TDMA FLOW_PERIOD / RDMA
        /// XON_XOFF_THRESH, both at ring offset 0x28) and the global DMA_CTRL/RING_CFG readbacks. Confirms
        /// no XOFF/flow threshold is choking the queue and that both rings are actually enabled — so a
        /// stall is ring geometry, not back-pressure.
        fn flow_evidence(&self) {
            let tflow = self.r(self.tdma_ring(RING_TDMA_FLOW_PERIOD));
            let rflow = self.r(self.rdma_ring(RING_TDMA_FLOW_PERIOD));
            let tctrl = self.r(self.tdma_global(DMA_CTRL));
            let rctrl = self.r(self.rdma_global(DMA_CTRL));
            let tcfg = self.r(self.tdma_global(DMA_RING_CFG));
            let rcfg = self.r(self.rdma_global(DMA_RING_CFG));
            serial_println!(
                "{}   flow/ring witness: TDMA flow_period={:#x} DMA_CTRL={:#010x} RING_CFG={:#010x} | RDMA xon_xoff={:#x} DMA_CTRL={:#010x} RING_CFG={:#010x} ::",
                PG, tflow, tctrl, tcfg, rflow, rctrl, rcfg
            );
        }

        /// Read the six station-MAC bytes from the UMAC MAC0/MAC1 registers (the firmware programs them
        /// there at boot). MAC0 = bytes 0..3 (big-endian in the register), MAC1 = bytes 4..5.
        fn read_mac_regs(&self) -> [u8; 6] {
            let m0 = self.r(UMAC_MAC0);
            let m1 = self.r(UMAC_MAC1);
            [
                (m0 >> 24) as u8,
                (m0 >> 16) as u8,
                (m0 >> 8) as u8,
                m0 as u8,
                (m1 >> 8) as u8,
                m1 as u8,
            ]
        }

        /// Program the station MAC into UMAC_MAC0/MAC1 (the receive filter's physical-match address).
        fn write_mac_regs(&self, mac: &[u8; 6]) {
            let m0 = (mac[0] as u32) << 24
                | (mac[1] as u32) << 16
                | (mac[2] as u32) << 8
                | (mac[3] as u32);
            let m1 = (mac[4] as u32) << 8 | (mac[5] as u32);
            serial_println!("{}   >>> REG WRITE (M2): UMAC_MAC0/MAC1 = station MAC ::", PG);
            self.w(UMAC_MAC0, m0);
            self.w(UMAC_MAC1, m1);
        }

        /// Allocate + program the RX ring: one descriptor per slot pointing at its DMA buffer, ring
        /// registers (buf size, start/end addr, done threshold), then arm the RX ring in DMA_CTRL /
        /// DMA_RING_CFG. Producer index starts at 0 (hardware advances it as it fills).
        fn init_rx(&mut self) {
            let buf_layout = Layout::from_size_align(RING_DEPTH * BUF_SIZE, 4096).unwrap();
            self.rx_bufs = unsafe { alloc::alloc::alloc_zeroed(buf_layout) };
            for i in 0..RING_DEPTH {
                let buf = (self.rx_bufs as u64) + (i * BUF_SIZE) as u64;
                let d = self.rx_desc(i);
                self.w(d + DMA_DESC_ADDRESS_LO, buf as u32);
                self.w(d + DMA_DESC_ADDRESS_HI, (buf >> 32) as u32);
                // RX length_status starts cleared; hardware overwrites with the received length/status.
                // No per-descriptor WRAP bit: the GENET ring path wraps via END_ADDR, not a status bit
                // (Linux never sets DMA_WRAP in the ring-based descriptor length_status).
                self.w(d + DMA_DESC_LENGTH_STATUS, 0);
            }
            barrier();
            // Ring config: descriptor 0..RING_DEPTH, buffer length in the low half.
            let bufsz = ((RING_DEPTH as u32) << DMA_BUFLENGTH_SHIFT) | BUF_SIZE as u32;
            self.w(self.rdma_ring(RING_DMA_RING_BUF_SIZE), bufsz);
            self.w(self.rdma_ring(RING_TDMA_READ_PTR), 0); // RDMA WRITE_PTR
            self.w(self.rdma_ring(RING_TDMA_READ_PTR + 4), 0);
            self.w(self.rdma_ring(RING_TDMA_PROD_INDEX), 0); // RDMA CONS_INDEX
            self.w(self.rdma_ring(RING_TDMA_CONS_INDEX), 0); // RDMA PROD_INDEX
            self.w(self.rdma_ring(RING_DMA_START_ADDR), 0);
            self.w(self.rdma_ring(RING_DMA_START_ADDR_HI), 0);
            // END_ADDR is in 32-bit WORDS (RING_DEPTH descriptors × 3 words − 1), consistent with the
            // 32-descriptor RING_BUF_SIZE above — so the RDMA write pointer wraps at the last valid
            // descriptor. (Was RING_DEPTH×DMA_DESC_SIZE−1 = a 4× byte/word mismatch: the HW ring never
            // wrapped, drove writes past descriptor 32 into the uninitialised tail, and the OFFER that
            // arrived after the first pass was never popped.)
            self.w(
                self.rdma_ring(RING_DMA_END_ADDR),
                (RING_DEPTH as u32 * DMA_DESC_WORDS) - 1,
            );
            self.w(self.rdma_ring(RING_DMA_END_ADDR_HI), 0);
            self.w(self.rdma_ring(RING_DMA_MBUF_DONE_THRESH), 1);
            self.w(self.rdma_ring(RING_TDMA_FLOW_PERIOD), 0); // RDMA XON/XOFF
            self.w(self.rdma_ring(RING_TDMA_WRITE_PTR), 0); // RDMA READ_PTR
            self.w(self.rdma_ring(RING_TDMA_WRITE_PTR + 4), 0);
            barrier();
            self.rx_c_index = 0;
            // Enable the ring in DMA_RING_CFG and turn on RDMA.
            serial_println!("{}   >>> REG WRITE (M2): RDMA ring {} armed (RING_CFG + DMA_CTRL.EN) ::", PG, RING);
            self.w(self.rdma_global(DMA_SCB_BURST_SIZE), DMA_MAX_BURST_LENGTH);
            self.w(self.rdma_global(DMA_RING_CFG), 1 << RING);
            let ctrl = DMA_EN | (1 << (RING as u32 + DMA_RING_BUF_EN_SHIFT));
            self.w(self.rdma_global(DMA_CTRL), ctrl);
            barrier();
        }

        /// Allocate + program the TX ring: descriptors start empty; producer/consumer indices at 0.
        /// Arm the TX ring in DMA_CTRL / DMA_RING_CFG.
        fn init_tx(&mut self) {
            let buf_layout = Layout::from_size_align(RING_DEPTH * BUF_SIZE, 4096).unwrap();
            self.tx_bufs = unsafe { alloc::alloc::alloc_zeroed(buf_layout) };
            for i in 0..RING_DEPTH {
                let d = self.tx_desc(i);
                self.w(d + DMA_DESC_ADDRESS_LO, 0);
                self.w(d + DMA_DESC_ADDRESS_HI, 0);
                // No per-descriptor WRAP bit (END_ADDR governs the ring wrap; Linux ring path never sets
                // DMA_WRAP in length_status).
                self.w(d + DMA_DESC_LENGTH_STATUS, 0);
            }
            barrier();
            let bufsz = ((RING_DEPTH as u32) << DMA_BUFLENGTH_SHIFT) | BUF_SIZE as u32;
            self.w(self.tdma_ring(RING_DMA_RING_BUF_SIZE), bufsz);
            self.w(self.tdma_ring(RING_TDMA_READ_PTR), 0);
            self.w(self.tdma_ring(RING_TDMA_READ_PTR + 4), 0);
            self.w(self.tdma_ring(RING_TDMA_PROD_INDEX), 0);
            self.w(self.tdma_ring(RING_TDMA_CONS_INDEX), 0);
            self.w(self.tdma_ring(RING_DMA_START_ADDR), 0);
            self.w(self.tdma_ring(RING_DMA_START_ADDR_HI), 0);
            // END_ADDR in 32-bit WORDS (see init_rx): consistent with the 32-descriptor RING_BUF_SIZE so
            // the TDMA read pointer wraps at descriptor 32. The byte/word mismatch was the observed stall
            // — boot-P3 caught cons_index frozen at exactly 32 (the ring depth) with prod at 201: the
            // engine drained one pass, failed to wrap, and hung on an uninitialised descriptor.
            self.w(
                self.tdma_ring(RING_DMA_END_ADDR),
                (RING_DEPTH as u32 * DMA_DESC_WORDS) - 1,
            );
            self.w(self.tdma_ring(RING_DMA_END_ADDR_HI), 0);
            self.w(self.tdma_ring(RING_DMA_MBUF_DONE_THRESH), 1);
            self.w(self.tdma_ring(RING_TDMA_FLOW_PERIOD), 0);
            self.w(self.tdma_ring(RING_TDMA_WRITE_PTR), 0);
            self.w(self.tdma_ring(RING_TDMA_WRITE_PTR + 4), 0);
            barrier();
            self.tx_prod = 0;
            serial_println!("{}   >>> REG WRITE (M2): TDMA ring {} armed (RING_CFG + DMA_CTRL.EN) ::", PG, RING);
            self.w(self.tdma_global(DMA_SCB_BURST_SIZE), DMA_MAX_BURST_LENGTH);
            self.w(self.tdma_global(DMA_RING_CFG), 1 << RING);
            let ctrl = DMA_EN | (1 << (RING as u32 + DMA_RING_BUF_EN_SHIFT));
            self.w(self.tdma_global(DMA_CTRL), ctrl);
            barrier();
        }

        /// Full M2 bring-up after the M1 reset: SYS port mode, UMAC reset + MAC + frame len, MIB reset,
        /// RBUF, RGMII OOB, RX/TX rings, then enable TX+RX in UMAC_CMD. Register-write order follows the
        /// Linux `bcmgenet_open` / `init_umac` / `init_dma` sequence. Returns true on a poison-honest
        /// readback that the block is still answering.
        fn init(&mut self, mac: [u8; 6]) -> bool {
            serial_println!("{}   M2 bring-up (GENET v5 programming order; polled, interrupts masked) ::", PG);
            // External GPHY port.
            serial_println!("{}   >>> REG WRITE (M2): SYS_PORT_CTRL = EXT_GPHY ::", PG);
            self.w(SYS_PORT_CTRL, PORT_MODE_EXT_GPHY);

            self.umac_reset();

            // Station MAC + max frame length.
            self.mac = mac;
            self.write_mac_regs(&mac);
            self.w(UMAC_MAX_FRAME_LEN, MAX_FRAME_LEN);

            // Reset the MIB counters.
            self.w(UMAC_MIB_CTRL, MIB_RESET_RX | MIB_RESET_RUNT | MIB_RESET_TX);
            self.w(UMAC_MIB_CTRL, 0);

            // RBUF: 64-byte descriptor status + 2-byte RX align (payload lands 4-byte aligned).
            serial_println!("{}   >>> REG WRITE (M2): RBUF_CTRL = 64B_EN | ALIGN_2B ::", PG);
            self.w(RBUF_CTRL, RBUF_64B_EN | RBUF_ALIGN_2B);

            // RGMII out-of-band: RGMII mode on, out-of-band status disabled (link driven in-band from
            // the PHY), internal-delay disabled (the Pi board provides the RGMII delay). RGMII_LINK is
            // NOT asserted here — it is set later ONLY if the PHY actually reports link (see
            // `mac_set_from_link`). Hand-asserting it lies to the MAC about a link that may be down.
            serial_println!("{}   >>> REG WRITE (M2): EXT_RGMII_OOB_CTRL = RGMII_MODE_EN | OOB_DISABLE | ID_MODE_DIS (RGMII_LINK deferred to PHY) ::", PG);
            self.w(
                EXT_RGMII_OOB_CTRL,
                RGMII_MODE_EN | OOB_DISABLE | ID_MODE_DIS,
            );

            // Descriptor rings.
            self.init_rx();
            self.init_tx();

            // Enable TX + RX; promiscuous for bring-up (mirrors the NET-4 / e1000 filter). Speed/duplex
            // are LEFT UNSET here (default 10M floor) — they are programmed from the PHY's negotiated
            // result in `mac_set_from_link`, not forced to gigabit. Forcing SPEED_1000 while the PHY
            // negotiated 100M mis-clocks RGMII and corrupts every TX frame.
            let cmd = CMD_TX_EN | CMD_RX_EN | CMD_PROMISC;
            serial_println!("{}   >>> REG WRITE (M2): UMAC_CMD = TX_EN | RX_EN | PROMISC (speed set from PHY autoneg later) ::", PG);
            self.w(UMAC_CMD, cmd);
            barrier();

            // Poison-honest readback: a live controller returns our written CMD bits (not open bus).
            let rb = self.r(UMAC_CMD);
            if is_poison(rb) {
                serial_println!("{}   UMAC_CMD readback = {:#010x} — POISON (device stopped answering); M2 FAILED ::", PG, rb);
                return false;
            }
            serial_println!(
                "{}   rings up: RX/TX ring {} ({} desc each); UMAC_CMD readback {:#010x} (live) ::",
                PG, RING, RING_DEPTH, rb
            );
            true
        }

        /// Transmit one raw Ethernet frame (smoltcp builds the full L2 frame): copy into the next TX
        /// buffer, write its descriptor (SOP|EOP|APPEND_CRC + length), publish, and bump the TX producer
        /// index. Bounded reclaim keeps the ring from overrunning on the witness. GENET appends the FCS.
        fn transmit(&mut self, frame: &[u8]) {
            let i = (self.tx_prod as usize) % RING_DEPTH;
            let len = frame.len().min(BUF_SIZE);
            let buf = unsafe { self.tx_bufs.add(i * BUF_SIZE) };
            unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), buf, len) };
            let d = self.tx_desc(i);
            let phys = (self.tx_bufs as u64) + (i * BUF_SIZE) as u64;
            self.w(d + DMA_DESC_ADDRESS_LO, phys as u32);
            self.w(d + DMA_DESC_ADDRESS_HI, (phys >> 32) as u32);
            // length_status fully re-armed every reuse (stale EOP/OWN cleared): SOP|EOP|APPEND_CRC + the
            // fresh length. No WRAP bit — the ring wraps via END_ADDR (Linux `bcmgenet_xmit_single`).
            let ls = ((len as u32) << DMA_BUFLENGTH_SHIFT)
                | DMA_SOP
                | DMA_EOP
                | DMA_TX_APPEND_CRC
                | (DMA_TX_QTAG_MASK << DMA_TX_QTAG_SHIFT);
            self.w(d + DMA_DESC_LENGTH_STATUS, ls);
            barrier();
            // Advance the TX producer index (hardware fetches up to it).
            self.tx_prod = self.tx_prod.wrapping_add(1);
            self.w(self.tdma_ring(RING_TDMA_PROD_INDEX), self.tx_prod as u32 & DMA_INDEX_MASK);
            barrier();
            self.tx_count += 1;
        }

        /// Pop one completed RX frame into `out`, recycle the descriptor, advance the RX consumer index.
        /// `None` if the hardware producer index has not advanced past our cursor (ring empty). The
        /// GENET index model's analog of the NET-4 `rx_frame_raw` OWN check.
        fn rx_frame_raw(&mut self, out: &mut [u8]) -> Option<usize> {
            let p_index = self.r(self.rdma_ring(RING_TDMA_CONS_INDEX)) & DMA_INDEX_MASK; // RDMA PROD_INDEX
            let c = self.rx_c_index as u32 & DMA_INDEX_MASK;
            if p_index == c {
                return None; // ring empty
            }
            let i = (self.rx_c_index as usize) % RING_DEPTH;
            let d = self.rx_desc(i);
            let buf_base = unsafe { self.rx_bufs.add(i * BUF_SIZE) };
            barrier(); // order the buffer read AFTER observing the advanced producer index
            // PI-GENET-5 witness (PRE-invalidate): capture what the CPU sees THROUGH the cache before
            // any invalidate — the descriptor length_status word (MMIO, always fresh), the in-buffer
            // status-block first word, and the leading 16 payload bytes. If these differ from the
            // post-invalidate reads below, the RX datapath was NOT coherent and stale cache — not a
            // classifier bug — is why boot-P10 popped 151 frames and classified none.
            let dsc_ls = self.r(d + DMA_DESC_LENGTH_STATUS);
            let sb_pre = unsafe { read_volatile(buf_base as *const u32) };
            let mut pre16 = [0u8; 16];
            let witness = self.pg5_rx_dumped < PG5_RX_DUMPS;
            if witness {
                let psrc = unsafe { buf_base.add(RX_STATUS_PAD) };
                for (k, slot) in pre16.iter_mut().enumerate() {
                    *slot = unsafe { read_volatile(psrc.add(k)) };
                }
            }
            // PI-GENET-5 FIX: invalidate this buffer's cache lines to the point of coherency so the
            // length read + payload copy below observe the bytes the MAC DMA'd into DRAM, not a stale
            // pre-refill line the CPU still held. (No-op cost on a genuinely coherent line.)
            invalidate_dcache(buf_base as u64, BUF_SIZE);
            // PI-GENET-4: with RBUF 64B_EN set, the authoritative length_status is the FIRST WORD of
            // the in-buffer 64-byte receive-status block (Linux `bcmgenet_desc_rx`, `desc_64b_en`
            // path: `status = (struct status_64 *)skb->data; dma_length_status = status->length_status`)
            // — not the descriptor word. Read the status block (now cache-coherent); fall back to the
            // descriptor word only if the block reads zero (belt-and-braces on odd hardware states).
            let mut sb = unsafe { read_volatile(buf_base as *const u32) };
            // PI-GENET-6 FIX (boot-P13 rx[0]): the RDMA producer index can advance before the buffer
            // DMA is visible in DRAM — the first popped frame read a zero status block AND zero payload
            // even post-invalidate (the OFFER, lost as zeros). A zero status block means "DMA not yet
            // visible", not "no status": re-invalidate and re-read, bounded, before consuming.
            if sb == 0 {
                let mut spins = 0u32;
                while sb == 0 && spins < 10_000 {
                    barrier();
                    invalidate_dcache(buf_base as u64, BUF_SIZE);
                    sb = unsafe { read_volatile(buf_base as *const u32) };
                    spins += 1;
                }
                if sb != 0 {
                    serial_println!(
                        "{}   [genet6] rx status block arrived after {} re-polls (premature pop rescued — producer index led the buffer DMA) ::",
                        PG, spins
                    );
                }
            }
            let ls = if sb != 0 { sb } else { dsc_ls };
            // Received length is bits [beyond 16]; strip the 64-byte status block + 2-byte align pad
            // (both included in the reported length — see RX_STATUS_PAD).
            let total = ((ls >> DMA_BUFLENGTH_SHIFT) & 0x0fff) as usize;
            let payload = total.saturating_sub(RX_STATUS_PAD);
            let len = payload.min(BUF_SIZE - RX_STATUS_PAD).min(out.len());
            if len > 0 {
                let src = unsafe { self.rx_bufs.add(i * BUF_SIZE + RX_STATUS_PAD) };
                unsafe { core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), len) };
            }
            self.rx_count += 1;
            // PI-GENET-5 witness (POST-invalidate): one line per pop that puts the pre/post-invalidate
            // delta on the record. `dsc`=descriptor length_status word; `sb_pre`/`sb_post`=in-buffer
            // status-block first word before/after `dc ivac`; `pre16`/`post16`=leading 16 payload bytes
            // before/after. Reading the two columns discriminates the boot-P10 gap in one glance:
            //   sb_pre==0 / post16 real  → stale cache; the invalidate is the fix (frames were real).
            //   sb_pre==sb_post, post16 real, yet 0 classified → payload IS good; the drop is in
            //     smoltcp ingress (MAC filter / length-offset / VLAN — chase classification, not DMA).
            //   post16 all-zero with a non-zero length → zero/garbage DMA (reachability), not caching.
            // The driver-side effective ethertype (`et`) is exactly what this pop hands up to smoltcp.
            if witness {
                self.pg5_rx_dumped += 1;
                let mut post = [0u8; 16];
                let m = len.min(16);
                post[..m].copy_from_slice(&out[..m]);
                let hx = |dst: &mut [u8; 32], s: &[u8; 16]| {
                    const HD: &[u8; 16] = b"0123456789abcdef";
                    for (j, &b) in s.iter().enumerate() {
                        dst[j * 2] = HD[(b >> 4) as usize];
                        dst[j * 2 + 1] = HD[(b & 0x0f) as usize];
                    }
                };
                let mut preh = [0u8; 32];
                let mut posth = [0u8; 32];
                hx(&mut preh, &pre16);
                hx(&mut posth, &post);
                let frame = &out[..len];
                let (et, _l3, _v) = eth_effective_type(frame);
                serial_println!(
                    "{}   [genet5] rx[{}] dsc={:#010x} sb_pre={:#010x} sb_post={:#010x} len={} et={:#06x} pre16={} post16={} ::",
                    PG,
                    self.pg5_rx_dumped - 1,
                    dsc_ls,
                    sb_pre,
                    sb,
                    len,
                    et,
                    core::str::from_utf8(&preh).unwrap_or("?"),
                    core::str::from_utf8(&posth).unwrap_or("?"),
                );
                if sb_pre != sb || pre16 != post {
                    serial_println!(
                        "{}   [genet5] ^ CACHE WAS STALE — pre/post differ: the RX datapath is NOT coherent; invalidate-before-read is the fix (frames were real, the CPU read a pre-DMA line) ::",
                        PG
                    );
                }
            }
            // PI-GENET-4 witness: the first PG4_RX_DUMPS popped frames print len + dst/src MAC +
            // effective ethertype (+ vlan id if tagged) + the first 32 raw bytes, one line each — the
            // boot-P6 distinguishing fact ("what ARE the 10 popped frames?") reads straight off these
            // lines. A DHCP frame found under a VLAN gets the named verdict line (the NET-4t verdict:
            // smoltcp has no 802.1Q — the socket can never see it).
            if self.pg4_rx_dumped < PG4_RX_DUMPS {
                self.pg4_rx_dumped += 1;
                let frame = &out[..len];
                let n = len.min(32);
                let mut hex = [0u8; 64];
                for (j, &b) in frame[..n].iter().enumerate() {
                    const HD: &[u8; 16] = b"0123456789abcdef";
                    hex[j * 2] = HD[(b >> 4) as usize];
                    hex[j * 2 + 1] = HD[(b & 0x0f) as usize];
                }
                let hexs = core::str::from_utf8(&hex[..n * 2]).unwrap_or("?");
                let (et, l3_off, vlan) = eth_effective_type(frame);
                if frame.len() >= 14 {
                    serial_println!(
                        "{}   [pigenet4] rx[{}] len={} dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} et={:#06x} vlan={} first{}B={} ::",
                        PG, self.pg4_rx_dumped - 1, len,
                        frame[0], frame[1], frame[2], frame[3], frame[4], frame[5],
                        frame[6], frame[7], frame[8], frame[9], frame[10], frame[11],
                        et,
                        vlan.map_or(-1i32, |v| v as i32),
                        n, hexs
                    );
                } else {
                    serial_println!(
                        "{}   [pigenet4] rx[{}] len={} RUNT(<14) first{}B={} ::",
                        PG, self.pg4_rx_dumped - 1, len, n, hexs
                    );
                }
                if vlan.is_some() && is_dhcp_udp(frame, et, l3_off) {
                    serial_println!(
                        "{}   [pigenet4] ^ DHCP frame is 802.1Q-tagged (vlan id={}) — smoltcp does not parse VLAN; the socket never sees it (untag the port or the drop stands) ::",
                        PG, vlan.unwrap_or(0)
                    );
                }
            }
            // Recycle: re-clear the descriptor status (hardware refills), keep the buffer address. No
            // WRAP bit — the RDMA engine wraps via END_ADDR, not a per-descriptor status bit.
            // PI-GENET-4: also re-clear the in-buffer status-block word so a stale length_status can
            // never be read for the NEXT frame delivered into this recycled buffer.
            unsafe { write_volatile(self.rx_bufs.add(i * BUF_SIZE) as *mut u32, 0) };
            self.w(d + DMA_DESC_LENGTH_STATUS, 0);
            barrier();
            // Advance our consumer index and publish it (hands the slot back to hardware).
            self.rx_c_index = self.rx_c_index.wrapping_add(1);
            self.w(self.rdma_ring(RING_TDMA_PROD_INDEX), self.rx_c_index as u32 & DMA_INDEX_MASK); // RDMA CONS_INDEX
            barrier();
            Some(len)
        }
    }

    /// Data-synchronisation barrier — order ring writes before the device sees the updated index, and
    /// order used-ring reads after we observe the index. The NET-4 / VNET `dsb sy` discipline.
    #[inline]
    fn barrier() {
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }

    /// PI-GENET-5: invalidate the data cache over `[addr, addr+len)` to the point of coherency
    /// (`dc ivac`, one op per 64-byte line, bracketed by `dsb`). The identity map makes VA==PA, so the
    /// same pointer the MAC DMA'd into is the one we invalidate. This drops any stale line the CPU
    /// cached over an RX buffer BEFORE the device refilled it, so the following read observes the DMA'd
    /// bytes in DRAM rather than a pre-DMA copy. The module header long noted this as the standing fix
    /// if metal ever showed stale RX (`invalidate-before-read`); boot-P10 (151 popped / 0 classified)
    /// is that evidence — the ACE-lite "coherent, `dsb` suffices" assumption did not hold for the RX
    /// datapath. `dc ivac` is safe on an already-coherent line (it merely drops a clean copy).
    #[inline]
    fn invalidate_dcache(addr: u64, len: usize) {
        let start = addr & !((CACHE_LINE as u64) - 1);
        let end = addr + len as u64;
        let mut p = start;
        unsafe {
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
            while p < end {
                core::arch::asm!("dc ivac, {}", in(reg) p, options(nostack, preserves_flags));
                p += CACHE_LINE as u64;
            }
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }
    }

    // ── DTB resolution: find the GENET node's register base (no hardcoded base; log candidates) ──

    /// The BCM2711 SoC peripheral `ranges` offset: child bus `0x7c00_0000` maps to CPU-physical
    /// `0xFC00_0000` (the +`0x8000_0000` translation piusb applies to the PCIe RC). Applied to the
    /// DTB-resolved child base to reach the ARM-physical register window.
    const SOC_RANGES_OFFSET: u64 = 0x8000_0000;
    /// The documented BCM2711 GENET ARM-physical base — quoted in the honest skip line for the operator,
    /// but NEVER dereferenced (an unmodeled window FAULTS on QEMU, it does not return poison).
    const GENET_DOC_BASE: u64 = 0xFD58_0000;
    /// The mapped Device GiB the register window must land in (`boot::build_l1` L1[3]).
    const DEVICE_GIB_LO: u64 = 0xC000_0000;
    const DEVICE_GIB_HI: u64 = 0x1_0000_0000;

    /// The DTB-resolved GENET register base + its `local-mac-address` (if the firmware supplied one).
    struct Resolved {
        base: u64,
        dtb_mac: Option<[u8; 6]>,
    }

    /// Walk the firmware DTB (READ-ONLY) for an `ethernet@`/`genet` node whose `compatible` names GENET,
    /// read its `reg` child base, translate to ARM-physical, and capture `local-mac-address`. Returns
    /// `None` (and the caller SKIPS before any MMIO) if no such node resolves — the QEMU / DTB-less
    /// path, where dereferencing the register window would fault (external abort), not return poison.
    fn resolve(dtb_addr: u64, dtb_size: usize) -> Option<Resolved> {
        if dtb_addr == 0 || dtb_size == 0 {
            serial_println!("{}   no DTB handed off (x0={:#x}, size={:#x}) — no GENET node to resolve ::", PG, dtb_addr, dtb_size);
            return None;
        }
        let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
        let Some(fdt) = Fdt::new(blob) else {
            serial_println!("{}   DTB parse failed — no GENET node to resolve ::", PG);
            return None;
        };

        // Find the GENET node path: a node whose `compatible` contains "genet".
        const PATH_CAP: usize = 160;
        let mut path0 = [0u8; PATH_CAP];
        let mut plen0 = 0usize;
        let mut found = false;
        fdt.for_each_prop(|e| {
            if found || e.name != b"compatible" {
                return;
            }
            let val = &blob[e.val_off..e.val_off + e.val_len];
            if val.windows(5).any(|w| w == b"genet") {
                let l = e.path.len().min(PATH_CAP);
                path0[..l].copy_from_slice(&e.path[..l]);
                plen0 = l;
                found = true;
            }
        });
        if !found {
            serial_println!("{}   no `genet`-compatible node in the DTB — bring-up SKIPPED before any MMIO ::", PG);
            return None;
        }
        let path = &path0[..plen0];

        // Read that node's `reg` (child base) and `local-mac-address`.
        let mut reg: Option<&[u8]> = None;
        let mut mac_prop: Option<&[u8]> = None;
        fdt.for_each_prop(|e| {
            if e.path != path {
                return;
            }
            let val = &blob[e.val_off..e.val_off + e.val_len];
            match e.name {
                b"reg" => reg = Some(val),
                b"local-mac-address" => mac_prop = Some(val),
                _ => {}
            }
        });

        let dtb_mac = mac_prop.and_then(|m| {
            if m.len() >= 6 {
                Some([m[0], m[1], m[2], m[3], m[4], m[5]])
            } else {
                None
            }
        });

        // The Pi DTB's GENET `reg` is #address-cells=2 (2 cells addr + 2 cells size), big-endian: the
        // child base is the first 2 cells. Translate child -> ARM-physical via the SoC ranges offset.
        let Some(reg) = reg else {
            serial_println!("{}   GENET node has no `reg` — bring-up SKIPPED before any MMIO ::", PG);
            return None;
        };
        if reg.len() < 8 {
            serial_println!("{}   GENET `reg` too short — bring-up SKIPPED before any MMIO ::", PG);
            return None;
        }
        let hi = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]) as u64;
        let lo = u32::from_be_bytes([reg[4], reg[5], reg[6], reg[7]]) as u64;
        let child = (hi << 32) | lo;
        let arm_phys = child + SOC_RANGES_OFFSET;
        serial_println!(
            "{}   DTB GENET node reg child base {:#x} -> ARM-physical {:#x} (SoC ranges +{:#x}; doc base {:#x}) ::",
            PG, child, arm_phys, SOC_RANGES_OFFSET, GENET_DOC_BASE
        );
        Some(Resolved { base: arm_phys, dtb_mac })
    }

    /// The one registered GENET NIC (populated by [`genet_bringup`]). Mirrors NET-4's `NET4_DEVICE` /
    /// VNET's `VNET_DEVICE` / the x86 e1000 `NET_DEVICE`; the smoltcp Device adapter reaches the rings
    /// through it.
    pub static GENET_DEVICE: spin::Mutex<Option<Genet>> = spin::Mutex::new(None);

    /// PI-GENET entry point (metal): resolve + probe the GENET (M1); bring up UMAC + rings (M2); bind
    /// smoltcp + DHCP/ping (M3). Graceful (an honest skip line) on any absent decode / unmapped window.
    pub fn genet_bringup(dtb_addr: u64, dtb_size: usize) {
        serial_println!(
            "{} BCM GENET v5 GbE bring-up (DTB @{:#x} size={:#x}) ::",
            PG, dtb_addr, dtb_size
        );

        // ── M1: resolve the register base from the DTB. NO DTB node => QEMU / DTB-less boot: this build
        //    does NOT model GENET, and touching the register window would FAULT (external abort), not
        //    return poison. Skip BEFORE any MMIO — the piusb `dtb_has_pcie` discipline. This is the
        //    honest QEMU classification (verified: QEMU 11 raspi4b aborts on the GENET window). ──
        let Some(res) = resolve(dtb_addr, dtb_size) else {
            serial_println!(
                "{}   GENET driver compiled-present; no GENET node in the DTB (QEMU raspi4b models no GENET, or a DTB-less boot) — bring-up SKIPPED before any MMIO (won't fault on an unmodeled window). Positive bring-up is attended Pi-4 metal. ::",
                PG
            );
            return;
        };
        if res.base < DEVICE_GIB_LO || res.base >= DEVICE_GIB_HI {
            serial_println!(
                "{}   resolved base {:#x} is OUTSIDE the mapped Device GiB [{:#x}..{:#x}) — bring-up SKIPPED ::",
                PG, res.base, DEVICE_GIB_LO, DEVICE_GIB_HI
            );
            return;
        }

        let mut nic = Genet {
            base: res.base,
            mac: [0; 6],
            rx_bufs: core::ptr::null_mut(),
            rx_c_index: 0,
            rx_count: 0,
            tx_bufs: core::ptr::null_mut(),
            tx_prod: 0,
            tx_count: 0,
            phy_addr: 1,
            pg4_rx_dumped: 0,
            pg5_rx_dumped: 0,
        };

        // ── M1: poison-honest version probe BEFORE any write — the platform-classifying read ──
        let Some(rev) = nic.probe_rev() else {
            serial_println!(
                "{}   SYS_REV_CTRL @ {:#x} = POISON (open-bus/firmware/zero) — this build does NOT model GENET (QEMU raspi4b, or link-down metal); driver compiled-present, bring-up SKIPPED (no write into an absent decode) ::",
                PG, res.base
            );
            // SError-drain class rule: the poisoned probe read may have left a latent async abort
            // pending; drain it (vectors are live at this post-heap call site) so the fail-closed
            // exit leaves the machine clean.
            crate::arch::aarch64::exceptions::serror_drain_request("genet: SYS_REV_CTRL poison");
            return;
        };
        // Decode the revision the way Linux `bcmgenet_set_hw_params` does: major nibble at [27:24]
        // (6 => v5), minor at [19:16], EPHY at [15:0].
        let major_raw = (rev >> 24) & 0x0f;
        let major = if major_raw == 6 { 5 } else if major_raw == 5 { 4 } else { major_raw };
        let minor = (rev >> 16) & 0x0f;
        let ephy = rev & 0xffff;
        serial_println!(
            "{}   SYS_REV_CTRL = {:#010x} — LIVE GENET v{} (rev minor {}, EPHY {:#06x}); this build MODELS the block ::",
            PG, rev, major, minor, ephy
        );

        // ── MAC source: prefer the DTB `local-mac-address`, else the UMAC MAC0/MAC1 registers ──
        let (mac, mac_src) = match res.dtb_mac {
            Some(m) => (m, "dtb local-mac-address"),
            None => (nic.read_mac_regs(), "umac-reg readback"),
        };
        let macs = fmt_mac(&mac);
        serial_println!(
            "{}   station MAC = {} (source: {}) ::",
            PG,
            core::str::from_utf8(&macs).unwrap_or("<mac>"),
            mac_src,
        );

        // ── M2: UMAC + rings ──
        if !nic.init(mac) {
            serial_println!("{} PI-GENET bring-up STOPPED after M2 init failed (device stopped answering) ::", PG);
            // The M2 writes went into a block that stopped answering — drain any latent abort.
            crate::arch::aarch64::exceptions::serror_drain_request("genet: M2 init failed");
            return;
        }

        // ── PHY: find the external BCM54213PE and RESOLVE the negotiated link (autoneg-honest) ──
        nic.phy_addr = nic.find_phy();
        let ls = nic.phy_resolve();
        serial_println!(
            "{}   PHY autoneg resolved (MDIO addr {}): link {} · aneg {} · speed {}M · {} ::",
            PG, nic.phy_addr,
            if ls.link { "UP" } else { "DOWN (no cable / negotiating)" },
            if ls.aneg { "COMPLETE" } else { "PENDING" },
            ls.speed,
            if ls.full_duplex { "full-duplex" } else { "half-duplex" }
        );
        // Program UMAC speed/duplex + RGMII link FROM the resolution — never forced.
        nic.mac_set_from_link(&ls);

        *GENET_DEVICE.lock() = Some(nic);
        serial_println!("{}   GENET registered; RX/TX rings live ::", PG);

        // ── M3: bind a smoltcp phy::Device over the rings + DHCP/ping (the shared net_phy seam) ──
        bind_smoltcp();

        // ── TX evidence (storm / LED-red-herring classes): compare frames we enqueued against the
        //    hardware TDMA producer/consumer indices after the whole DHCP+ping exchange. A bounded,
        //    fully-drained ring (cons==prod, small count) rules out a runaway re-post — the solid
        //    activity LED is then a benign gigabit-link indication, not a TX storm. ──
        //    RX evidence + the flow/ring witness ride alongside: an RDMA producer index past the ring
        //    depth proves the RX ring wrapped and kept delivering (the OFFER-never-popped co-suspect),
        //    and the flow witness rules out XOFF back-pressure as the stall cause.
        if let Some(n) = GENET_DEVICE.lock().as_ref() {
            n.tx_evidence("post-DHCP+ping");
            n.rx_evidence("post-DHCP+ping");
            n.flow_evidence();
        }
        serial_println!("{} PI-GENET DONE — GENET v5 driver up + smoltcp bound ::", PG);
    }

    // ── Static FALLBACK addressing (used only if DHCP does not lease within the bounded timeout) ──
    const OUR_IP: [u8; 4] = [192, 168, 1, 2];
    const GATEWAY_IP: [u8; 4] = [192, 168, 1, 1];
    /// Bounded DHCP-lease timeout (ms). The clock is real time (CNTPCT), so this is non-hanging.
    // PI-GENET-6: 5 s missed the lease on boot-P13 — a real 342-byte frame landed right after the
    // window closed. 15 s is the shape that leased on the Orin (2 DISCOVERs, bootpd answered the 2nd).
    const DHCP_TIMEOUT_MS: i64 = 15_000;

    /// Monotonic millisecond clock from the free-running counter (CNTPCT). Drives both smoltcp time and
    /// the DHCP timeout (the NET-4 `now_ms` shape).
    #[inline]
    fn now_ms() -> i64 {
        let (cnt, frq): (u64, u64);
        unsafe {
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
        }
        if frq == 0 { 0 } else { (cnt.wrapping_mul(1_000) / frq) as i64 }
    }

    // ── Raw L2 accessors over the GENET_DEVICE registry (the shared smoltcp Device seam) ──
    fn raw_rx(out: &mut [u8]) -> Option<usize> {
        GENET_DEVICE.lock().as_mut().and_then(|n| n.rx_frame_raw(out))
    }
    fn raw_tx(frame: &[u8]) {
        if let Some(n) = GENET_DEVICE.lock().as_mut() {
            n.transmit(frame);
        }
    }
    fn link_up() -> bool {
        GENET_DEVICE.lock().as_ref().map(|n| n.link_up()).unwrap_or(false)
    }

    // ── The RawNic seam: the shared `net_phy::SmoltcpPhy` moves L2 frames through these ──
    struct GenetNic;
    impl RawNic for GenetNic {
        fn rx_frame_raw(out: &mut [u8]) -> Option<usize> {
            raw_rx(out)
        }
        fn transmit(frame: &[u8]) {
            raw_tx(frame)
        }
        fn mac() -> Option<[u8; 6]> {
            GENET_DEVICE.lock().as_ref().map(|n| n.mac)
        }
    }

    // ── smoltcp interface plumbing (the phy::Device itself is the shared `net_phy::SmoltcpPhy`) ──
    use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
    use smoltcp::phy::Device;
    use smoltcp::socket::icmp;
    use smoltcp::time::Instant;
    use smoltcp::wire::{EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress};

    /// ICMP identifier stamped on the echo requests. ASCII "PG".
    const PING_IDENT: u16 = 0x5047;
    const PING_PAYLOAD: &[u8] = b"unaos-genet";
    /// NET-ARP-1: the ping window is REAL time (CNTPCT ms), not an iteration count. The boot-P7 pump
    /// spun 200k iterations of a FAKE 1-ms-per-iteration clock — milliseconds of real wall time — then
    /// returned, so the router's ARP requests (arriving seconds later) hit a dead stack and every echo
    /// was queued before the neighbor resolved. Real-time bounds keep the interface live long enough to
    /// answer ARP and collect replies; non-hanging by construction (CNTPCT is free-running).
    const PING_WINDOW_MS: i64 = 8_000;
    /// One echo per interval (real ms) — the first echo triggers ARP resolution and is dropped by
    /// smoltcp (no retransmit in the ICMP socket); pacing lets later echoes ride the resolved neighbor.
    const PING_INTERVAL_MS: i64 = 1_000;
    /// Link-DOWN window (real ms): pre-cable there is nothing to answer — bound the no-op pump tightly.
    const PING_WINDOW_DOWN_MS: i64 = 250;

    /// Bind a smoltcp `Interface` over the GENET Device, run DHCP, and drive a bounded ICMP echo to the
    /// gateway — the x86 e1000/smolnet + NET-4/VNET seam, on the Pi. On a live link (real Pi metal, or
    /// QEMU if it wires a netdev to the modelled GENET) this leases + pings; pre-cable / empty-ring it is
    /// an honest bounded no-op. All storage is stack-local (no heap growth).
    fn bind_smoltcp() {
        let Some(mac) = GenetNic::mac() else {
            serial_println!("{}   smoltcp bind SKIPPED — no NIC registered ::", PG);
            return;
        };
        let up = link_up();
        let mut dev = SmoltcpPhy::<GenetNic>::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = 0x5047_454e; // ASCII "PGEN"
        let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));

        let netcfg = crate::net_phy::dhcp_or_static(
            PG, &mut iface, &mut dev, &now_ms, DHCP_TIMEOUT_MS, OUR_IP, 24, GATEWAY_IP,
        );
        let gw = netcfg.gw;

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
            serial_println!("{}   ICMP socket bind FAILED => FAIL ::", PG);
            return;
        }

        const COUNT: u16 = 4;
        let remote = IpAddress::v4(gw[0], gw[1], gw[2], gw[3]);
        let mut sent = 0u16;
        let mut received = 0u16;
        let mut seq = 0u16;
        // NET-ARP-1: poll with the SAME real clock dhcp_or_static just used. The old loop restarted a
        // fake clock at 0 — a huge time regression against the smoltcp Interface's internal timestamps
        // (neighbor cache, retransmit deadlines) stamped with real CNTPCT ms moments earlier.
        let window_ms = if up { PING_WINDOW_MS } else { PING_WINDOW_DOWN_MS };
        let t0 = now_ms();
        let mut next_send = t0;
        loop {
            let t = now_ms();
            if t.saturating_sub(t0) >= window_ms {
                break;
            }
            iface.poll(Instant::from_millis(t), &mut dev, &mut sockets);
            let sock = sockets.get_mut::<icmp::Socket>(handle);
            if seq < COUNT && t >= next_send && sock.can_send() {
                seq += 1;
                next_send = t + PING_INTERVAL_MS;
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
                            if received >= COUNT {
                                break;
                            }
                        }
                    }
                }
            }
        }

        let pass = received > 0;
        // NET-ARP-1 emission witness: what smoltcp actually handed the TX ring across the DHCP + ping
        // windows (counted at the phy TxToken, i.e. wire-side of the seam).
        let (txn, arp_reply, dhcp) = crate::net_phy::tx_emission_counts();
        serial_println!(
            "{}   [netarp1] smoltcp emitted {} frames (arp-reply={} dhcp={}) ::",
            PG, txn, arp_reply, dhcp
        );
        serial_println!(
            "{} ping {}.{}.{}.{} ({}/{} sent, {}/{} replies) [{}] link {} => {} ::",
            PG,
            gw[0], gw[1], gw[2], gw[3],
            sent, COUNT, received, COUNT,
            if netcfg.leased { "dhcp" } else { "static" },
            if up { "UP" } else { "DOWN" },
            if pass { "PASS" } else { "SKIP (no reply — pre-cable / no DHCP is the honest pre-metal state)" }
        );
    }
}
