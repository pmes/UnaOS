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
    const MDIO_WR: u32 = 1 << 26;
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

    // ── PI-GENET-8: BCM54xx PHY LED control (the external BCM54213PE at MDIO addr 1, PHYID1=0x600d
    //    confirms the Broadcom BCM5421x family). The Pi 4's RJ45 LEDs are driven by THIS PHY, not by the
    //    MAC — so the MAC-side EXT_RGMII OOB_DISABLE / forced RGMII_LINK bits have no bearing on them
    //    (that was the red herring). The LED behaviour is set by the PHY's LED-selector SHADOW registers,
    //    reached through the standard Broadcom shadow-access register 0x1c: a write is
    //    `WRITE | (shadow<<10) | data[9:0]`. Left at their power-on / bootloader defaults, a selector can
    //    map an LED to a SOLID source (link/speed or full-duplex — lit the whole time a gigabit
    //    full-duplex link is up), which is exactly the stuck-amber the operator flagged ("no card leaves
    //    the yellow light on"). We reprogram the LED selectors to standard link + activity sources so
    //    neither LED is tied to an always-asserted source. Encodings + register map follow Linux
    //    `bcm-phy-lib` / `brcmphy.h` (BCM_LED_SRC_*, BCM5482_SHD_LEDS1/2) — architectural PHY facts. ──
    const MII_BCM_SHD: u32 = 0x1c; // Broadcom shadow-register access register
    const BCM_SHD_WRITE: u16 = 0x8000; // shadow write-enable
    const BCM_SHD_LEDS1: u16 = 0x0d; // LED Selector 1: LED1 [3:0], LED3 [7:4]
    const BCM_SHD_LEDS2: u16 = 0x0e; // LED Selector 2: LED2 [3:0], LED4 [7:4]
    // LED source encodings (BCM_LED_SRC_*): 0x0 = link/speed indication (solid on link),
    // 0x3 = activity (blinks on RX/TX, dark when idle), 0xe = OFF (tied high). We use only the
    // non-tied-on sources so no LED can sit permanently lit.
    const BCM_LED_SRC_LINKSPD1: u16 = 0x0;
    const BCM_LED_SRC_ACTIVITYLED: u16 = 0x3;

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

        /// Write one MII register on the external PHY over the UMAC MDIO master (bounded busy-wait).
        /// The MDIO write mirror of `mdio_read`; used for the PI-GENET-8 PHY LED-selector programming.
        fn mdio_write(&self, phy: u8, reg: u32, val: u16) {
            let cmd = MDIO_WR
                | ((phy as u32) << MDIO_PMD_SHIFT)
                | (reg << MDIO_REG_SHIFT)
                | (val as u32);
            self.w(UMAC_MDIO_CMD, cmd);
            let v = self.r(UMAC_MDIO_CMD);
            self.w(UMAC_MDIO_CMD, v | MDIO_START_BUSY);
            barrier();
            for _ in 0..100_000 {
                if self.r(UMAC_MDIO_CMD) & MDIO_START_BUSY == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }

        /// PI-GENET-8: write a BCM54xx PHY SHADOW register through the standard shadow-access register
        /// 0x1c (`WRITE | (shadow<<10) | data[9:0]`). Data is masked to 10 bits per the register field.
        fn phy_shd_write(&self, shadow: u16, data: u16) {
            let v = BCM_SHD_WRITE | (shadow << 10) | (data & 0x03ff);
            self.mdio_write(self.phy_addr, MII_BCM_SHD, v);
        }

        /// PI-GENET-8: program the external BCM54213PE's LED selectors to standard link + activity
        /// sources so neither RJ45 LED is tied to an always-asserted source (the observed stuck-amber).
        /// The board's physical green/amber mapping to LED1..LED4 is not documented to us, so BOTH
        /// selector nibbles of BOTH selector registers are set to real (non-tied-on) sources: whichever
        /// pins the board wires out then show link and activity, never a permanently-lit selector. This
        /// is a PHY-side cosmetic fix (MDIO only); it touches no MAC datapath and weakens no protection.
        fn phy_config_leds(&self) {
            // LEDS1: LED1 [3:0] = link/speed, LED3 [7:4] = activity.
            let leds1 = (BCM_LED_SRC_ACTIVITYLED << 4) | BCM_LED_SRC_LINKSPD1;
            // LEDS2: LED2 [3:0] = activity, LED4 [7:4] = link/speed (symmetric to LEDS1).
            let leds2 = (BCM_LED_SRC_LINKSPD1 << 4) | BCM_LED_SRC_ACTIVITYLED;
            serial_println!(
                "{}   >>> PHY MDIO (M2): BCM54xx LED selectors LEDS1={:#06x} LEDS2={:#06x} (link+activity; no tied-on source — stuck-amber fix) ::",
                PG, leds1, leds2
            );
            self.phy_shd_write(BCM_SHD_LEDS1, leds1);
            self.phy_shd_write(BCM_SHD_LEDS2, leds2);
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
            // PI-GENET-7 FIX (the TX mirror of GENET-5): the buffers are CACHED memory, so the frame
            // bytes just written can sit in the D-cache while the MAC DMA-reads stale DRAM — TDMA
            // drains (descriptors consumed) yet nothing real egresses. Clean the buffer's lines to the
            // point of coherency BEFORE publishing the descriptor. (No-op cost on a coherent line.)
            clean_dcache(buf as u64, len);
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
            let sb = unsafe { read_volatile(buf_base as *const u32) };
            // PI-GENET-8 FIX (rx[0] phantom zero-status pop): on EVERY boot the FIRST popped frame reads
            // a ZERO in-buffer status block even after `dc ivac`, while the DESCRIPTOR length_status word
            // (MMIO, always fresh) is a valid non-zero length AND the payload at offset RX_STATUS_PAD is
            // the real frame (metal: rx[0] dsc=0x01987f80 len=342 et=0x0800, pre16 = the live OFFER — the
            // status block alone was zero). From rx[1] on, sb_pre==sb_post==dsc: the status block IS
            // populated for later slots. So the RDMA completion writes back the descriptor length_status
            // + advances the producer index, but the 64-byte status-block write lags / is skipped on the
            // first ring pass. The GENET-6 assumption ("sb==0 => DMA not yet visible; a bounded
            // invalidate+re-read re-poll rescues it") is REFUTED by metal — 10k spins NEVER flipped the
            // block, because nothing writes it for that slot; the loop only burned time while real frames
            // queued. The descriptor length_status word is therefore the AUTHORITATIVE length source (the
            // status block is kept only as the `[genet5]` witness column). Note this also refutes the
            // index-baseline hypothesis: a slot popped one-early would show dsc_ls==0 too (init clears
            // it), but dsc_ls is a valid length here — the slot WAS completed, not popped early.
            let ls = if dsc_ls != 0 { dsc_ls } else { sb };
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

    /// PI-GENET-7: clean (write back) `[addr, addr+len)` to the point of coherency (`dc cvac`, one op
    /// per 64-byte line, bracketed by `dsb`) so the MAC's TX DMA reads the frame the CPU just wrote,
    /// not a stale DRAM line. The TX mirror of `invalidate_dcache`.
    #[inline]
    fn clean_dcache(addr: u64, len: usize) {
        const CACHE_LINE: usize = 64;
        let mut p = addr & !(CACHE_LINE as u64 - 1);
        let end = addr + len as u64;
        unsafe {
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
            while p < end {
                core::arch::asm!("dc cvac, {}", in(reg) p, options(nostack, preserves_flags));
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

        // PI-NET-13: the QEMU TCP/HTTP/mDNS regression gate. Hardware-free (a pure in-kernel loopback
        // seam driving the SAME pool/reaper/http service code the GENET path runs), so it executes here
        // BEFORE the DTB skip — i.e. it runs on QEMU raspi4b, which models no GENET. Armed only by the
        // `nettest` feature (UNAOS_NETTEST=1), so a normal `genet` build never enters it. It prints a
        // self-checking `:: NET-GATE: ... PASS/FAIL [w=0x..] ::` witness the arm/kernel8 battery asserts.
        #[cfg(feature = "nettest")]
        nettest::run();
        // PI-NET-14: the OUTBOUND client gate (DNS resolver + HTTP client), same hardware-free loopback
        // seam. Prints a self-checking `:: NET14-GATE: ... PASS [w=0x..] ::` line the battery asserts.
        #[cfg(feature = "nettest")]
        nettest::run14();

        // PI-NET-15: the FILESYSTEM route gate — the scripted peer fetches `/fs/` + `/fs/<fixture>` off
        // the SAME serving pool, over the same loopback seam, reading the unafs K3 fixture volume the
        // kernel8-test image carries. Prints `:: NET15-GATE: ... PASS [w=0x..] ::` for the battery.
        #[cfg(feature = "nettest")]
        nettest::run15();

        // PI-NET-16: the SNTP client + wall-clock gate — pure hostile-input parser checks plus a live
        // loopback exchange that sets the clock, over the same hardware-free seam. Prints a self-checking
        // `:: NET16-GATE: ... PASS [w=0x..] ::` line the battery asserts.
        #[cfg(feature = "nettest")]
        nettest::run16();

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
        // PI-GENET-8: reprogram the PHY LED selectors off their power-on defaults so the RJ45 amber
        // LED is not left tied to a solid link/full-duplex source (the operator's stuck-yellow report).
        nic.phy_config_leds();
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
        // PI-NET-9: count the replies smoltcp emits (ARP reply / ICMP echo reply) as they cross the
        // wire seam — every outbound frame passes here. Echo *requests* (type 8, our bind_smoltcp ping)
        // and DHCP are NOT counted; only the answers the persistent poll produces bump these.
        net9_classify_tx(frame);
        if let Some(n) = GENET_DEVICE.lock().as_mut() {
            n.transmit(frame);
        }
    }

    // ── PI-NET-9: reply-emission counters (the [net9] witness) ──────────────────────────────────────
    static NET9_ARP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    static NET9_ICMP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

    /// Classify one outbound L2 frame: bump NET9_ARP for an ARP reply (ethertype 0x0806, opcode 2) and
    /// NET9_ICMP for an ICMPv4 echo reply (ethertype 0x0800, proto 1, type 0). These are exactly the
    /// frames smoltcp's `iface.poll` emits in answer to the gateway's who-has / echo-request, so the
    /// counters are an emission proof that the Pi ANSWERED, not a poll-loop guess.
    fn net9_classify_tx(frame: &[u8]) {
        use core::sync::atomic::Ordering::Relaxed;
        if frame.len() < 14 {
            return;
        }
        let et = u16::from_be_bytes([frame[12], frame[13]]);
        if et == 0x0806 {
            if frame.len() >= 22 && frame[20] == 0 && frame[21] == 2 {
                NET9_ARP.fetch_add(1, Relaxed);
            }
        } else if et == 0x0800 && frame.len() >= 14 + 20 {
            let ihl = ((frame[14] & 0x0f) as usize) * 4;
            let l4 = 14 + ihl;
            // proto 1 = ICMP; the ICMP type byte is the first of the L4 header; type 0 = echo reply.
            if frame[23] == 1 && frame.len() > l4 && frame[l4] == 0 {
                NET9_ICMP.fetch_add(1, Relaxed);
            }
        }
    }

    /// Snapshot the PI-NET-9 reply counters `(arp_reply, icmp_echo_reply)`. Cumulative since boot.
    fn net9_counts() -> (u32, u32) {
        use core::sync::atomic::Ordering::Relaxed;
        (NET9_ARP.load(Relaxed), NET9_ICMP.load(Relaxed))
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
    use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
    use smoltcp::phy::Device;
    use smoltcp::socket::{icmp, tcp, udp};
    use smoltcp::time::{Duration, Instant};
    use smoltcp::wire::{
        EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpEndpoint,
    };

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

    // ── PI-NET-9: the PERSISTENT net service ────────────────────────────────────────────────────────
    //
    // bind_smoltcp's DHCP+ping window is bounded — it returns, and the interface stops being polled, so
    // nothing can ever answer the gateway's later ARP who-has / ICMP echo requests. This gives the
    // DHCP-configured `Interface` + `Device` a home BEYOND that window (a static, single-core-owned like
    // GENET_DEVICE) and a scheduled kernel task that polls it on a tick cadence. smoltcp's `iface.poll`
    // answers ARP and ICMP echo BY ITSELF when polled — no protocol code here; the empty SocketSet is
    // just the poll signature's third argument.
    // PI-NET-13: `NetService` is generic over the smoltcp `Device` so the SAME pool/reaper/http/mdns
    // service logic runs over ANY seam. The default type parameter is the GENET phy, so the metal path
    // (`arm_net_service`, the `NET_SERVICE` static) is byte-identical — it instantiates
    // `NetService<SmoltcpPhy<GenetNic>>` by inference, exactly as before. The QEMU loopback gate
    // (`nettest`) instantiates `NetService<SmoltcpPhy<LoopNic>>` and drives the identical methods.
    /// PI-NET-16: the non-blocking re-sync state machine that rides the poll loop. `Idle` holds the
    /// wall-clock ms at which the next query is due; `Waiting` holds the send time and the reply deadline.
    #[derive(Clone, Copy)]
    enum SntpState {
        Idle { due_ms: i64 },
        Waiting { deadline_ms: i64 },
    }

    struct NetService<D: Device = SmoltcpPhy<GenetNic>> {
        iface: Interface,
        dev: D,
        sockets: SocketSet<'static>,
        /// PI-NET-10 / PI-NET-12: the pool of passively-listening HTTP socket handles in `sockets`.
        /// A POOL (not a single socket) gives real accept concurrency + a listen backlog: a browser
        /// opening several parallel connections (Safari preconnect/pipelining) each land on a distinct
        /// listener instead of every SYN-after-the-first being dropped for want of a free TCB.
        http: [SocketHandle; HTTP_POOL],
        /// PI-NET-12: per-listener "request-seen" latch — set once that connection's request bytes have
        /// arrived, so its response is emitted only after the peer's GET is in; cleared on every re-listen.
        req_seen: [bool; HTTP_POOL],
        /// PI-NET-12: per-listener idle clock (ms). Stamped when a listener first goes active (accepts a
        /// connection / half-open SYN); 0 while listening or free. Drives the idle-reaper that frees a TCB
        /// wedged by a peer that connects but never sends a request (Safari speculative socket, SYN flood).
        active_since: [i64; HTTP_POOL],
        /// PI-NET-11: the mDNS UDP socket's handle (bound to 5353, answering `unaos.local`).
        mdns: SocketHandle,
        /// PI-NET-16: the SNTP re-sync UDP socket (bound to the ephemeral client port), the cached time
        /// source resolved at boot, and the non-blocking re-sync state. `sntp_server` is `None` until the
        /// initial sync yields an address; `sntp_state` gates the next ~6-hourly query.
        sntp: SocketHandle,
        sntp_server: Option<[u8; 4]>,
        sntp_state: SntpState,
        /// PI-NET-10: our configured IPv4 (leased or static-fallback), shown on the status page.
        ip: [u8; 4],
        /// PI-NET-15: per-listener request-line capture (method + path + version). Bytes accumulate here
        /// until the end of the request line (`\n`) is seen or the buffer fills, then the route is parsed.
        req_buf: [[u8; REQ_CAP]; HTTP_POOL],
        req_len: [usize; HTTP_POOL],
        /// PI-NET-15: per-listener rendered response + send cursor. Built ONCE when the route resolves
        /// (the file is read into RAM under a single with_unafs hold — the IRQ-mask cost is bounded to
        /// that one read), then streamed out across poll steps through the normal TX path so a large
        /// file never parks a listener under the mount lock. `None` while listening/idle.
        resp: [Option<(alloc::vec::Vec<u8>, usize)>; HTTP_POOL],
    }
    // Single-core service, touched only behind this mutex on the BSP/AP that owns it — same discipline
    // (and the same raw-DMA reachability through GENET_DEVICE) as `Genet`.
    unsafe impl<D: Device> Send for NetService<D> {}
    // The default type parameter resolves `NetService` here to `NetService<SmoltcpPhy<GenetNic>>` — the
    // exact concrete type the metal service used before PI-NET-13.
    static NET_SERVICE: spin::Mutex<Option<NetService>> = spin::Mutex::new(None);

    // ── PI-NET-10: the Pi's first TCP service — a listening socket that serves a status page ──────────
    //
    // A single passive TCP socket lives in the persistent SocketSet. `iface.poll` (already driven every
    // ~4 ms by the net9 task) runs the TCP state machine; each poll we take ONE bounded service step on
    // the socket: re-arm the listener whenever it falls idle, drain the incoming request, and once the
    // peer's GET is in and our TX half is open, emit a small HTTP/1.0 status page and close. Because a
    // smoltcp socket needs an explicit re-listen after RST/close, `http_step` re-listens from EVERY
    // non-open state — so the service survives repeated requests and rude clients (RST mid-handshake,
    // half-open close) without wedging. QEMU raspi4b models no GENET, so `genet_bringup` returns before
    // `bind_smoltcp`/`arm_net_service` ever run: the whole service no-ops under the existing SKIP path.
    /// The service listens on the well-known HTTP port.
    const HTTP_PORT: u16 = 80;
    /// PI-NET-12: number of listening TCP sockets (the accept backlog). Each is an independent TCB with
    /// its own ring buffers; N concurrent client connections can be in flight before any SYN is dropped.
    /// A status page needs little concurrency, but a browser routinely opens 4–6 parallel sockets, and a
    /// pool absorbs that instead of serializing onto one TCB. Bounded (fixed static storage), so a flood
    /// can at worst fill the pool — the idle-reaper (below) then frees wedged TCBs on a deadline.
    const HTTP_POOL: usize = 4;
    /// PI-NET-12: idle-reap deadline (ms). A listener that has been in a non-listening, non-closed state
    /// this long WITHOUT completing a request/response is force-aborted (RST) and re-armed. This is the
    /// structural fix for the wedge: smoltcp's socket `timeout` only fires when TX data is pending, so an
    /// established-but-silent peer (never sends its GET) would otherwise hold a TCB forever. On a LAN a
    /// real client sends its GET within milliseconds of connecting; 3 s is generous headroom.
    const HTTP_IDLE_MS: i64 = 3_000;
    /// PI-NET-12: transport-level guards layered under the app idle-reaper. `set_timeout` aborts a peer
    /// that stalls mid-response (TX data pending, no ACK); `set_keep_alive` probes idle peers so a dead
    /// TCP endpoint is detected and the timeout can fire. Belt-and-suspenders with the idle-reaper.
    const TCP_TIMEOUT_MS: u64 = 3_000;
    const TCP_KEEPALIVE_MS: u64 = 1_000;
    /// TCP socket ring buffers (bytes). RX holds a browser's request line + headers; TX comfortably holds
    /// the full response (headers + page) so a single `send_slice` never truncates before `close`.
    const TCP_RX_CAP: usize = 2048;
    const TCP_TX_CAP: usize = 4096;
    /// Rendered-response scratch cap (HTTP headers + body). The body is bounded well under this.
    const HTTP_RESP_CAP: usize = 1536;
    const HTTP_BODY_CAP: usize = 1024;
    /// aarch64 generic-timer tick rate (Hz) — used only to present uptime as scheduler ticks on the page.
    const TICK_HZ: i64 = 250;

    // ── PI-NET-15: the filesystem route — serve the native unafs volume over HTTP ─────────────────────
    //
    // `GET /` keeps the status page (now with links); `GET /fs/` lists the unafs root; `GET /fs/<NAME>`
    // serves one file's bytes. The mount lock (`with_unafs`) masks IRQs around the polled SD read, so the
    // serve path reads the WHOLE file into a bounded RAM buffer under ONE short hold, then streams that
    // buffer out through the normal TX path — the lock is never held across `send_slice`, and the reaper
    // still covers a stalled peer. A file beyond the cap is refused (413) rather than read.
    /// Largest file the fs route reads into RAM under one hold (bounded kernel RAM; a Pi 4 has GiB, so
    /// 64 KiB × the pool is trivially affordable). Beyond this the request is refused 413.
    const FS_CAP: usize = 64 * 1024;
    /// Request-line capture cap (method + path + HTTP-version; a real request line fits well under this).
    const REQ_CAP: usize = 256;
    /// Longest accepted 8.3 name (`NNNNNNNN.EEE` = 12). Anything longer is rejected as a bad name.
    const FS_NAME_MAX: usize = 12;
    /// with_unafs hold-duration WARN threshold (ms): a hold longer than this masked IRQs long enough that
    /// the bench should see it flagged (the mount-lock IRQ-mask cost the brief calls out).
    const FS_HOLD_WARN_MS: i64 = 50;

    /// PI-NET-15 (gate seam only): when non-zero, overrides [`FS_CAP`] for the file-serve path so the
    /// loopback gate can drive the oversize-refusal branch against an EXISTING fixture (K3PAT.BIN, 12 KiB)
    /// without adding card state. 0 in every production build — a single relaxed load in the serve path,
    /// the same test-seam idiom as `unafs::TEST_FAIL_MIDSTAGE`. Only `nettest::run15` ever writes it.
    static FS_CAP_OVERRIDE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    fn fs_cap() -> usize {
        let o = FS_CAP_OVERRIDE.load(core::sync::atomic::Ordering::Relaxed);
        if o == 0 { FS_CAP } else { o }
    }

    /// Raw free-running counter read (for measuring the with_unafs hold in ticks).
    #[inline]
    fn cntpct() -> u64 {
        let c: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) c, options(nomem, nostack, preserves_flags));
        }
        c
    }
    /// Convert a `(t0, t1)` counter span into `(ticks, milliseconds)` using CNTFRQ.
    fn hold_span(t0: u64, t1: u64) -> (u64, i64) {
        let ticks = t1.wrapping_sub(t0);
        let frq: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
        }
        let ms = if frq == 0 { 0 } else { (ticks.wrapping_mul(1_000) / frq) as i64 };
        (ticks, ms)
    }
    /// Suffix appended to a per-request witness when the hold exceeded the WARN threshold.
    fn hold_warn(ms: i64) -> &'static str {
        if ms > FS_HOLD_WARN_MS { " WARN: with_unafs IRQ-mask > 50ms" } else { "" }
    }

    /// PI-NET-15: the resolved route for one request line.
    enum Route<'a> {
        /// `GET /` — the status page.
        Status,
        /// `GET /fs/` (or `/fs`) — the unafs root directory listing.
        FsList,
        /// `GET /fs/<NAME>` with a name that passed hostile-input validation.
        FsFile(&'a str),
        /// Anything else (bad method, unknown path, or a rejected name) — answered 404.
        NotFound,
    }

    /// PI-NET-15: hostile-input path validation for the `<NAME>` after `/fs/`. 8.3 names ONLY: a single
    /// path component of 1..=12 bytes drawn from `[A-Za-z0-9._-]`. This one charset check rejects `..`
    /// and `.` (guarded explicitly), a nested `/` (directory traversal), `%`-escapes (decode NOTHING —
    /// a `%` is simply not in the set), a zero-length name, and an oversize name.
    fn valid_fs_name(name: &[u8]) -> bool {
        if name.is_empty() || name.len() > FS_NAME_MAX {
            return false;
        }
        if name == b"." || name == b".." {
            return false;
        }
        name.iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-')
    }

    /// PI-NET-15: parse the accumulated request bytes into a [`Route`]. Only `GET` is served; the path is
    /// the token between the first and second space (or CR/LF). Bounds-checked throughout.
    fn parse_route(req: &[u8]) -> Route<'_> {
        let Some(rest) = req.strip_prefix(b"GET ") else {
            return Route::NotFound;
        };
        let end = rest
            .iter()
            .position(|&c| c == b' ' || c == b'\r' || c == b'\n')
            .unwrap_or(rest.len());
        let path = &rest[..end];
        if path == b"/" {
            return Route::Status;
        }
        if path == b"/fs" || path == b"/fs/" {
            return Route::FsList;
        }
        if let Some(name) = path.strip_prefix(b"/fs/") {
            if valid_fs_name(name) {
                // Valid names are a strict ASCII subset, so this never fails.
                if let Ok(s) = core::str::from_utf8(name) {
                    return Route::FsFile(s);
                }
            }
            return Route::NotFound;
        }
        Route::NotFound
    }

    /// PI-NET-15: MIME type from the 8.3 extension. `.htm`/`.html` => text/html; `.txt` or a name with
    /// no extension => text/plain (the default); anything else => application/octet-stream.
    fn content_type_for(name: &str) -> &'static str {
        fn ends_ci(name: &str, suf: &str) -> bool {
            let (n, s) = (name.as_bytes(), suf.as_bytes());
            n.len() >= s.len()
                && n[n.len() - s.len()..]
                    .iter()
                    .zip(s)
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
        }
        if ends_ci(name, ".html") || ends_ci(name, ".htm") {
            "text/html; charset=utf-8"
        } else if ends_ci(name, ".txt") || !name.contains('.') {
            "text/plain; charset=utf-8"
        } else {
            "application/octet-stream"
        }
    }

    /// PI-NET-15: assemble a full HTTP/1.0 response (status line + headers + body) into an owned buffer.
    /// The body is already in RAM; the listener streams the returned bytes out through the normal TX path.
    fn http_response(status: &str, ctype: &str, body: &[u8]) -> alloc::vec::Vec<u8> {
        let hdr = alloc::format!(
            "HTTP/1.0 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
Connection: close\r\nServer: UnaOS/genet\r\n\r\n",
            body.len()
        );
        let mut v = alloc::vec::Vec::with_capacity(hdr.len() + body.len());
        v.extend_from_slice(hdr.as_bytes());
        v.extend_from_slice(body);
        v
    }

    /// PI-NET-15: the outcome of reading one file off the unafs volume under a single with_unafs hold.
    enum FsFileResult {
        Ok(alloc::vec::Vec<u8>),
        TooBig(u64),
        IsDir,
        NotFound,
        MountErr,
    }

    /// PI-NET-15: read one file's bytes into RAM under ONE IRQ-masked with_unafs hold, returning the
    /// result plus the hold duration `(ticks, ms)` so the caller can witness the IRQ-mask cost. Resolve →
    /// stat → (size ≤ cap ?) → read_data(0, size). The lock is dropped before any TX work.
    fn fs_read_file(name: &str) -> (FsFileResult, u64, i64) {
        let cap = fs_cap() as u64;
        let path = alloc::format!("/{}", name);
        let t0 = cntpct();
        let r = crate::fs::unafs::with_unafs(|fs| {
            let id = match fs.resolve_path(&path) {
                Ok(id) => id,
                Err(_) => return FsFileResult::NotFound,
            };
            let inode = match fs.read_inode(id) {
                Ok(i) => i,
                Err(_) => return FsFileResult::NotFound,
            };
            if inode.kind == ::unafs::FileKind::Directory {
                return FsFileResult::IsDir;
            }
            if inode.size > cap {
                return FsFileResult::TooBig(inode.size);
            }
            match fs.read_data(id, 0, inode.size) {
                Ok(d) => FsFileResult::Ok(d),
                Err(_) => FsFileResult::NotFound,
            }
        });
        let (ticks, ms) = hold_span(t0, cntpct());
        (r.unwrap_or(FsFileResult::MountErr), ticks, ms)
    }

    /// PI-NET-15: read the unafs root directory under ONE with_unafs hold — `(name, size, is_dir)` per
    /// entry — plus the hold duration. `Err(())` means the volume could not be mounted/listed.
    #[allow(clippy::type_complexity)]
    fn fs_read_dir() -> (Result<alloc::vec::Vec<(alloc::string::String, u64, bool)>, ()>, u64, i64) {
        let t0 = cntpct();
        let r = crate::fs::unafs::with_unafs(|fs| {
            let root = fs.resolve_path("/").map_err(|_| ())?;
            let entries = fs.ls(root).map_err(|_| ())?;
            let mut out = alloc::vec::Vec::with_capacity(entries.len());
            for de in &entries {
                let size = fs.read_inode(de.inode_id).map(|i| i.size).unwrap_or(0);
                let is_dir = de.kind == ::unafs::FileKind::Directory;
                out.push((de.name.clone(), size, is_dir));
            }
            Ok::<_, ()>(out)
        });
        let (ticks, ms) = hold_span(t0, cntpct());
        (r.unwrap_or(Err(())), ticks, ms)
    }

    /// PI-NET-15: render the `/fs/` directory-listing HTML (name + size, files linked to `/fs/<name>`).
    /// Entry names come from the trusted local volume (8.3), so they are emitted verbatim.
    fn build_fs_list(entries: &[(alloc::string::String, u64, bool)]) -> alloc::vec::Vec<u8> {
        use core::fmt::Write as _;
        let mut body = alloc::string::String::new();
        let _ = write!(
            body,
            "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">\
<title>UnaOS \u{2014} /fs/</title></head><body>\
<h1>unafs volume \u{2014} root</h1><ul>"
        );
        for (name, size, is_dir) in entries {
            if *is_dir {
                let _ = write!(body, "<li>&lt;DIR&gt; {name}/</li>");
            } else {
                let _ = write!(body, "<li><a href=\"/fs/{name}\">{name}</a> \u{2014} {size} bytes</li>");
            }
        }
        let _ = write!(
            body,
            "</ul><p><a href=\"/\">back to status</a></p></body></html>\n"
        );
        http_response("200 OK", "text/html; charset=utf-8", body.as_bytes())
    }

    /// PI-NET-10: cumulative count of status pages served since boot (the `[net10] served N` witness).
    static NET10_SERVED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    fn net10_served() -> u32 {
        NET10_SERVED.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Rate-limit for the `[net10]` served witness: mirror net9 — at most once per ~5 s, unless ≥4 new
    /// requests landed. Change-only (the poll task only reports when the count moved).
    const NET10_REPORT_DELTA: u32 = 4;

    /// PI-NET-12: cumulative count of HTTP TCBs force-reaped (idle/half-open aborts) since boot.
    static NET10_REAPED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    fn net10_reaped() -> u32 {
        NET10_REAPED.load(core::sync::atomic::Ordering::Relaxed)
    }

    // ── PI-NET-11: the mDNS responder — make the Pi answer `unaos.local` on the share segment ─────────
    //
    // A single UDP socket in the persistent SocketSet is bound to the mDNS port 5353, and the interface
    // joins the mDNS IPv4 multicast group 224.0.0.251 so smoltcp's IP layer ACCEPTS the group's frames
    // even if promisc ever drops (the GENET RX filter is promisc for bring-up, but the join keeps the
    // stack correct on its own). Each poll `mdns_step` drains a bounded number of received datagrams,
    // hand-parses the DNS header + first question (length-checking every read — a malformed packet is
    // dropped, never panics or wedges the loop), and when the query is QTYPE A/ANY for `unaos.local`
    // (case-insensitive) it emits a standard mDNS response (QR=1 AA=1, one A answer, TTL 120, cache-flush
    // bit set, RDATA = the current lease IPv4) from port 5353 to 224.0.0.251:5353 — or unicast back to the
    // querier if the query set the QU (unicast-response) bit. QEMU raspi4b models no GENET, so the whole
    // service (and this socket) never arms there: a clean no-op under the existing SKIP path.
    /// The well-known mDNS UDP port.
    const MDNS_PORT: u16 = 5353;
    /// The mDNS IPv4 link-local multicast group.
    const MDNS_MCAST: [u8; 4] = [224, 0, 0, 251];
    /// The name we answer for (a single-label host under `.local`).
    const MDNS_HOST: &[u8] = b"unaos";
    const MDNS_TLD: &[u8] = b"local";
    /// mDNS answer TTL (seconds) — the RFC 6762 default host-record TTL.
    const MDNS_TTL: u32 = 120;
    /// UDP socket ring buffers. A single mDNS query is well under a KiB; a handful of metadata slots
    /// covers the bounded per-poll drain.
    const UDP_META_SLOTS: usize = 8;
    const UDP_RX_CAP: usize = 1024;
    const UDP_TX_CAP: usize = 1024;
    /// Bound on datagrams drained + answered per poll step (keeps the shared poll budget bounded so the
    /// HTTP service and net9 ARP/ICMP answering are never starved by an mDNS flood).
    const MDNS_MAX_PER_POLL: usize = 8;

    /// PI-NET-11: cumulative count of mDNS queries answered since boot (the `[net11] answered N` witness).
    static NET11_ANSWERED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    fn net11_answered() -> u32 {
        NET11_ANSWERED.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// PI-NET-11: parse a DNS query and decide whether it asks for `unaos.local` (QTYPE A or ANY,
    /// case-insensitive). Returns `Some(unicast_requested)` — the query's QU (unicast-response) bit — when
    /// it matches, else `None`. EVERY read is bounds-checked (`get`/`get(..)` with `?`): a truncated or
    /// malformed packet yields `None` and is dropped, never a panic. Name compression (a 0xC0 pointer) is
    /// rejected defensively — a legitimate first-question QNAME in a query is never compressed.
    fn mdns_wants_unaos(pkt: &[u8]) -> Option<bool> {
        // DNS header is 12 bytes: ID, flags, QDCOUNT, ANCOUNT, NSCOUNT, ARCOUNT.
        if pkt.len() < 12 {
            return None;
        }
        let flags = u16::from_be_bytes([pkt[2], pkt[3]]);
        // QR bit (0x8000) must be 0 (a query, not a response); opcode (bits 11..14) must be 0 (standard).
        if flags & 0x8000 != 0 || (flags >> 11) & 0x0f != 0 {
            return None;
        }
        let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]);
        if qdcount == 0 {
            return None;
        }
        // Walk the first question's QNAME labels against ["unaos", "local"], case-insensitively.
        let want: [&[u8]; 2] = [MDNS_HOST, MDNS_TLD];
        let mut off = 12usize;
        let mut li = 0usize;
        loop {
            let len = *pkt.get(off)? as usize;
            off += 1;
            if len == 0 {
                break; // end of name
            }
            if len & 0xc0 != 0 {
                return None; // compression pointer / reserved bits — bail defensively
            }
            let label = pkt.get(off..off + len)?;
            off += len;
            if li >= want.len() {
                return None; // more labels than "unaos.local" has
            }
            let w = want[li];
            if label.len() != w.len() {
                return None;
            }
            for (a, b) in label.iter().zip(w.iter()) {
                if a.to_ascii_lowercase() != *b {
                    return None;
                }
            }
            li += 1;
        }
        if li != want.len() {
            return None; // fewer labels than "unaos.local"
        }
        // QTYPE + QCLASS follow the null terminator.
        let qtype = u16::from_be_bytes([*pkt.get(off)?, *pkt.get(off + 1)?]);
        let qclass = u16::from_be_bytes([*pkt.get(off + 2)?, *pkt.get(off + 3)?]);
        // A = 1, ANY = 255.
        if qtype != 1 && qtype != 255 {
            return None;
        }
        // The QU (unicast-response) bit is the top bit of QCLASS; the low 15 bits carry the class
        // (IN = 1, or ANY = 255).
        let qu = qclass & 0x8000 != 0;
        let cls = qclass & 0x7fff;
        if cls != 1 && cls != 255 {
            return None;
        }
        Some(qu)
    }

    /// PI-NET-11: build a standard mDNS A-record response for `unaos.local` -> `ip` into `out`, returning
    /// its byte length. QR=1 AA=1, QDCOUNT=0 (mDNS responses do not echo the question), one A answer with
    /// the cache-flush bit set in its class, TTL `MDNS_TTL`, RDATA = the four lease octets. `out` must be
    /// at least `MDNS_RESP_LEN` bytes; the answer is a fixed 39 bytes.
    fn build_mdns_response(out: &mut [u8], ip: [u8; 4]) -> usize {
        // 12 (header) + 13 (name: 5+"unaos"+5+"local"+0) + 2 (type) + 2 (class) + 4 (ttl) + 2 (rdlen)
        // + 4 (rdata) = 39.
        const RESP_LEN: usize = 39;
        if out.len() < RESP_LEN {
            return 0;
        }
        // Header.
        out[0..2].copy_from_slice(&0u16.to_be_bytes()); // ID = 0 (mDNS multicast response)
        out[2..4].copy_from_slice(&0x8400u16.to_be_bytes()); // flags: QR=1, AA=1
        out[4..6].copy_from_slice(&0u16.to_be_bytes()); // QDCOUNT = 0
        out[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCOUNT = 1
        out[8..10].copy_from_slice(&0u16.to_be_bytes()); // NSCOUNT = 0
        out[10..12].copy_from_slice(&0u16.to_be_bytes()); // ARCOUNT = 0
        // Answer NAME: 0x05 "unaos" 0x05 "local" 0x00.
        let mut w = 12usize;
        out[w] = MDNS_HOST.len() as u8;
        w += 1;
        out[w..w + MDNS_HOST.len()].copy_from_slice(MDNS_HOST);
        w += MDNS_HOST.len();
        out[w] = MDNS_TLD.len() as u8;
        w += 1;
        out[w..w + MDNS_TLD.len()].copy_from_slice(MDNS_TLD);
        w += MDNS_TLD.len();
        out[w] = 0; // root label
        w += 1;
        // TYPE = A (1); CLASS = IN (1) | cache-flush (0x8000) = 0x8001.
        out[w..w + 2].copy_from_slice(&1u16.to_be_bytes());
        w += 2;
        out[w..w + 2].copy_from_slice(&0x8001u16.to_be_bytes());
        w += 2;
        // TTL.
        out[w..w + 4].copy_from_slice(&MDNS_TTL.to_be_bytes());
        w += 4;
        // RDLENGTH = 4, RDATA = the IPv4 lease.
        out[w..w + 2].copy_from_slice(&4u16.to_be_bytes());
        w += 2;
        out[w..w + 4].copy_from_slice(&ip);
        w += 4;
        debug_assert_eq!(w, RESP_LEN);
        w
    }

    /// A fixed-buffer `core::fmt::Write` sink (no heap): appends until the buffer is full, then silently
    /// drops the tail. Every string we format is far under the caller's cap, so no truncation occurs.
    struct BufWriter<'a> {
        buf: &'a mut [u8],
        len: usize,
    }
    impl core::fmt::Write for BufWriter<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let b = s.as_bytes();
            let n = b.len().min(self.buf.len() - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
            self.len += n;
            Ok(())
        }
    }

    /// PI-NET-10: render the full HTTP/1.0 response (status line + headers + HTML body) into `out`,
    /// returning its byte length. The body reports the OS name, the compiled-in hw-pi4 tip sha
    /// (`UNAOS_GIT_SHA` if the build passed one, else the branch label), uptime, the net9 ARP/ICMP
    /// reply counters, the served count, and our configured IPv4. Two-pass: body first (to know its
    /// length for `Content-Length`), then headers + body.
    fn render_http_response(out: &mut [u8], ip: [u8; 4]) -> usize {
        use core::fmt::Write as _;
        let (arp, icmp) = net9_counts();
        let served = net10_served();
        let up_ms = now_ms().max(0);
        let up_ticks = up_ms * TICK_HZ / 1000;
        let sha = option_env!("UNAOS_GIT_SHA").unwrap_or("hw-pi4");
        // PI-NET-16: the current UTC wall-clock, extrapolated from the last SNTP anchor + CNTPCT elapsed.
        // Before the first successful sync there is no civil time to show — say so honestly.
        let mut time_buf = [0u8; 24];
        let time_str: &str = match wall_unix_now() {
            Some(secs) => {
                let n = render_iso8601(secs, &mut time_buf);
                core::str::from_utf8(&time_buf[..n]).unwrap_or("unsynced")
            }
            None => "unsynced (no SNTP yet)",
        };

        let mut body_buf = [0u8; HTTP_BODY_CAP];
        let mut body = BufWriter { buf: &mut body_buf, len: 0 };
        let _ = write!(
            body,
            "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">\
<title>UnaOS \u{2014} Pi 4</title></head><body>\
<h1>UnaOS answers</h1>\
<p>The Raspberry Pi 4's on-board Broadcom GENET v5 GbE is up, and this page came off \
the kernel's own TCP stack (smoltcp bound over the GENET rings, served by the net9 poll task).</p>\
<ul>\
<li>track: <b>hw-pi4</b> (tip {sha})</li>\
<li>uptime: {up_ms} ms ({up_ticks} ticks @ {TICK_HZ} Hz)</li>\
<li>time (UTC): {time_str}</li>\
<li>net9 replies: arp={arp}, icmp-echo={icmp}</li>\
<li>pages served: {served}</li>\
<li>lease ip: {}.{}.{}.{}</li>\
</ul>\
<p><a href=\"/fs/\">browse the unafs volume (/fs/)</a></p>\
<p>PI-NET-10 \u{2014} the Pi's first TCP service; PI-NET-15 \u{2014} it serves its filesystem.</p>\
</body></html>\n",
            ip[0], ip[1], ip[2], ip[3],
        );
        let blen = body.len;

        let mut hw = BufWriter { buf: out, len: 0 };
        let _ = write!(
            hw,
            "HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
Content-Length: {blen}\r\nConnection: close\r\nServer: UnaOS/genet\r\n\r\n"
        );
        let hlen = hw.len;
        let total = hlen + blen;
        if total <= out.len() {
            out[hlen..total].copy_from_slice(&body_buf[..blen]);
            total
        } else {
            // Unreachable given the caps, but never write past the buffer.
            hlen
        }
    }

    impl<D: Device> NetService<D> {
        /// PI-NET-10: one bounded HTTP service step on the listening socket, run after each `iface.poll`.
        /// Handles every socket state so the service survives repeated requests and rude clients:
        /// any non-open state re-arms `listen(:80)`; an open connection's request is drained (path
        /// ignored — the same status page answers all); once the request is in and TX is writable the
        /// response is sent and the socket closed. `close()` walks the FIN handshake to `Closed`, where
        /// the next step re-listens. A peer RST drops the socket straight to `Closed` \u{2014} same path.
        fn http_step(&mut self, now: i64) {
            let ip = self.ip;
            let mut reaped = 0u32;
            for i in 0..HTTP_POOL {
                {
                    let sock = self.sockets.get_mut::<tcp::Socket>(self.http[i]);

                    // TIME-WAIT can't be re-listened directly (`listen` needs CLOSED), and it pins a TCB
                    // for 2*MSL. We've already delivered the response, so abort it straight to CLOSED and
                    // re-arm on the same pass — no MSL stall, the listener is back in the backlog at once.
                    if sock.state() == tcp::State::TimeWait {
                        sock.abort();
                    }

                    // Re-arm the passive listener from any closed/idle state (post-close, post-RST,
                    // post-abort, or first arm). `listen` errors from a non-CLOSED state; the guard above
                    // guarantees CLOSED. Reset the per-listener request + response state.
                    if !sock.is_open() {
                        let _ = sock.listen(HTTP_PORT);
                        self.req_seen[i] = false;
                        self.active_since[i] = 0;
                        self.req_len[i] = 0;
                        self.resp[i] = None;
                        continue;
                    }

                    // The socket is open. If it has left LISTEN (accepted a connection or a half-open SYN),
                    // clock its idle age and reap it once it overstays without completing service. This is
                    // the wedge fix: a peer that connects but never sends a request no longer holds the TCB.
                    if sock.is_active() {
                        if self.active_since[i] == 0 {
                            self.active_since[i] = now;
                        } else if now.saturating_sub(self.active_since[i]) > HTTP_IDLE_MS {
                            sock.abort(); // RST; re-listened on the next pass via the CLOSED path.
                            self.req_seen[i] = false;
                            self.active_since[i] = 0;
                            self.req_len[i] = 0;
                            self.resp[i] = None;
                            reaped += 1;
                            continue;
                        }
                    }

                    // Phase 1 — accumulate the request line until we can parse a route. Copy new bytes
                    // into the per-listener buffer (bounded to REQ_CAP), consuming the whole RX ring each
                    // step so the window stays open. Once the end of the request line (`\n`) is in, or the
                    // buffer fills, the request is ready to route.
                    if self.resp[i].is_none() && sock.can_recv() {
                        let rl = &mut self.req_len[i];
                        let rb = &mut self.req_buf[i];
                        let _ = sock.recv(|buf| {
                            let take = buf.len().min(REQ_CAP - *rl);
                            rb[*rl..*rl + take].copy_from_slice(&buf[..take]);
                            *rl += take;
                            (buf.len(), ())
                        });
                        if self.req_buf[i][..self.req_len[i]].contains(&b'\n')
                            || self.req_len[i] == REQ_CAP
                        {
                            self.req_seen[i] = true;
                        }
                    }
                }

                // Phase 2 — route + render ONCE (this is where the single with_unafs hold happens, off the
                // socket borrow). The whole file lands in the RAM buffer here; nothing below touches the
                // mount lock, so a large file is streamed without holding IRQs masked.
                if self.req_seen[i] && self.resp[i].is_none() {
                    let vec = self.build_route_response(i, ip);
                    self.resp[i] = Some((vec, 0));
                }

                // Phase 3 — stream the rendered response out through the normal TX path; close when the
                // whole buffer has been enqueued. A file larger than one TX ring simply drains across
                // several poll steps (the listener stays ACTIVE, never parked; the reaper covers a stall).
                if self.resp[i].is_some() {
                    let sock = self.sockets.get_mut::<tcp::Socket>(self.http[i]);
                    if sock.can_send() {
                        if let Some((buf, off)) = self.resp[i].as_mut() {
                            if let Ok(sent) = sock.send_slice(&buf[*off..]) {
                                *off += sent;
                            }
                        }
                    }
                    let done = self
                        .resp[i]
                        .as_ref()
                        .map(|(buf, off)| *off >= buf.len())
                        .unwrap_or(false);
                    if done {
                        let sock = self.sockets.get_mut::<tcp::Socket>(self.http[i]);
                        sock.close();
                        self.resp[i] = None;
                        self.req_seen[i] = false;
                        self.req_len[i] = 0;
                        // Re-clock: close() walks the FIN handshake (still is_active); let it progress on
                        // its own deadline so a peer that never ACKs our FIN is reaped, not pinned.
                        self.active_since[i] = now;
                        NET10_SERVED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            if reaped > 0 {
                NET10_REAPED.fetch_add(reaped, core::sync::atomic::Ordering::Relaxed);
            }
        }

        /// PI-NET-15: resolve the captured request line to a [`Route`] and render its full HTTP response
        /// into an owned buffer. `GET /` → the status page; `GET /fs/` → the unafs root listing; `GET
        /// /fs/<NAME>` → the file bytes (404 on a missing file / rejected name, 413 beyond the cap). The
        /// fs branches read under one with_unafs hold and emit a per-request `[net15]` witness carrying
        /// the hold duration in ticks/ms (with a WARN suffix when the IRQ-mask exceeded the threshold).
        fn build_route_response(&mut self, i: usize, ip: [u8; 4]) -> alloc::vec::Vec<u8> {
            let route = parse_route(&self.req_buf[i][..self.req_len[i]]);
            match route {
                Route::Status => {
                    let mut buf = [0u8; HTTP_RESP_CAP];
                    let n = render_http_response(&mut buf, ip);
                    buf[..n].to_vec()
                }
                Route::FsList => {
                    let (res, ticks, ms) = fs_read_dir();
                    match res {
                        Ok(entries) => {
                            serial_println!(
                                "{} [net15] GET /fs/ => 200 ({} entries; with_unafs hold {} ticks / {} ms){} ::",
                                PG, entries.len(), ticks, ms, hold_warn(ms)
                            );
                            build_fs_list(&entries)
                        }
                        Err(()) => {
                            serial_println!(
                                "{} [net15] GET /fs/ => 500 unafs mount unavailable (hold {} ticks / {} ms) ::",
                                PG, ticks, ms
                            );
                            http_response(
                                "500 Internal Server Error",
                                "text/plain; charset=utf-8",
                                b"unafs mount unavailable\n",
                            )
                        }
                    }
                }
                Route::FsFile(name) => {
                    let (res, ticks, ms) = fs_read_file(name);
                    let warn = hold_warn(ms);
                    match res {
                        FsFileResult::Ok(data) => {
                            serial_println!(
                                "{} [net15] GET /fs/{} => 200 {} bytes (with_unafs hold {} ticks / {} ms){} ::",
                                PG, name, data.len(), ticks, ms, warn
                            );
                            http_response("200 OK", content_type_for(name), &data)
                        }
                        FsFileResult::TooBig(sz) => {
                            serial_println!(
                                "{} [net15] GET /fs/{} => 413 REFUSED (size {} > cap {}; hold {} ticks / {} ms){} ::",
                                PG, name, sz, fs_cap(), ticks, ms, warn
                            );
                            http_response(
                                "413 Payload Too Large",
                                "text/plain; charset=utf-8",
                                b"file exceeds server buffer cap\n",
                            )
                        }
                        FsFileResult::IsDir => {
                            serial_println!("{} [net15] GET /fs/{} => 404 (is a directory) ::", PG, name);
                            http_response(
                                "404 Not Found",
                                "text/plain; charset=utf-8",
                                b"not a file\n",
                            )
                        }
                        FsFileResult::NotFound => {
                            serial_println!("{} [net15] GET /fs/{} => 404 (no such file) ::", PG, name);
                            http_response(
                                "404 Not Found",
                                "text/plain; charset=utf-8",
                                b"no such file\n",
                            )
                        }
                        FsFileResult::MountErr => {
                            serial_println!("{} [net15] GET /fs/{} => 500 (unafs mount unavailable) ::", PG, name);
                            http_response(
                                "500 Internal Server Error",
                                "text/plain; charset=utf-8",
                                b"unafs mount unavailable\n",
                            )
                        }
                    }
                }
                Route::NotFound => http_response(
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    b"not found\n",
                ),
            }
        }

        /// PI-NET-12: census the HTTP listener pool — (listening, active) counts for the TCB witness.
        /// `active` = accepted/in-flight connections (not LISTEN, not CLOSED/TIME-WAIT). `listen == 0`
        /// means the backlog is saturated (every TCB busy) — the poll task flags that explicitly.
        fn http_census(&mut self) -> (u32, u32) {
            let mut listen = 0u32;
            let mut active = 0u32;
            for i in 0..HTTP_POOL {
                let sock = self.sockets.get_mut::<tcp::Socket>(self.http[i]);
                if sock.is_listening() {
                    listen += 1;
                } else if sock.is_active() {
                    active += 1;
                }
            }
            (listen, active)
        }

        /// PI-NET-11: one bounded mDNS service step on the UDP socket, run after each `iface.poll`.
        /// Drains up to `MDNS_MAX_PER_POLL` received datagrams; each is hand-parsed (every read
        /// bounds-checked) and, when it is a query for `unaos.local` (QTYPE A/ANY), answered with an A
        /// record for the current lease IPv4 — multicast to 224.0.0.251:5353, or unicast to the querier
        /// when the QU bit was set. While the interface is unconfigured (no lease yet) we skip answering
        /// but still drain the socket so its RX ring never backs up.
        fn mdns_step(&mut self) {
            let ip = self.iface.ipv4_addr().map(|a| a.octets());
            let sock = self.sockets.get_mut::<udp::Socket>(self.mdns);
            for _ in 0..MDNS_MAX_PER_POLL {
                if !sock.can_recv() {
                    break;
                }
                let mut qbuf = [0u8; 512];
                let (n, meta) = match sock.recv_slice(&mut qbuf) {
                    Ok(v) => v,
                    Err(_) => break, // exhausted, or a too-big datagram dropped — stop draining
                };
                // Skip answering while unconfigured, but the datagram is already drained above.
                let Some(ip) = ip else { continue };
                let Some(qu) = mdns_wants_unaos(&qbuf[..n]) else { continue };
                let mut resp = [0u8; 48];
                let rlen = build_mdns_response(&mut resp, ip);
                if rlen == 0 {
                    continue;
                }
                // Multicast reply by default; unicast to the querier (its source addr + port) if it asked.
                let dst = if qu {
                    meta.endpoint
                } else {
                    IpEndpoint::new(
                        IpAddress::v4(MDNS_MCAST[0], MDNS_MCAST[1], MDNS_MCAST[2], MDNS_MCAST[3]),
                        MDNS_PORT,
                    )
                };
                if sock.send_slice(&resp[..rlen], dst).is_ok() {
                    NET11_ANSWERED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }
            }
        }

        /// PI-NET-16: one non-blocking re-sync step, run after each `iface.poll`. When the ~6 h cadence
        /// comes due it fires ONE SNTP request at the cached server through the pool's UDP socket, then in
        /// later steps reads the reply and re-anchors the wall-clock — never blocking the poll loop. A
        /// missed reply (deadline) or a KoD/malformed reply schedules a nearer retry; the failure path is
        /// witnessed exactly once per attempt. No-ops entirely until the boot sync has cached a server.
        fn sntp_step(&mut self, now: i64) {
            let Some(server) = self.sntp_server else {
                return; // no time source cached (initial sync produced none) — nothing to re-sync
            };
            match self.sntp_state {
                SntpState::Idle { due_ms } => {
                    if now < due_ms {
                        return;
                    }
                    let mut req = [0u8; 48];
                    net16_build_request(&mut req);
                    let dst =
                        IpEndpoint::new(IpAddress::v4(server[0], server[1], server[2], server[3]), NTP_PORT);
                    let s = self.sockets.get_mut::<udp::Socket>(self.sntp);
                    if s.can_send() && s.send_slice(&req, dst).is_ok() {
                        self.sntp_state = SntpState::Waiting { deadline_ms: now + NET16_SNTP_WINDOW_MS };
                    } else {
                        // Socket not ready (unconfigured/wedged) — retry the whole attempt later.
                        self.sntp_state = SntpState::Idle { due_ms: now + NET16_RESYNC_RETRY_MS };
                    }
                }
                SntpState::Waiting { deadline_ms } => {
                    let s = self.sockets.get_mut::<udp::Socket>(self.sntp);
                    if s.can_recv() {
                        let mut rb = [0u8; 128];
                        if let Ok((n, _meta)) = s.recv_slice(&mut rb) {
                            match net16_parse_sntp(&rb[..n]) {
                                Sntp::Ok { unix_secs, stratum } => {
                                    wall_set(unix_secs, stratum);
                                    let mut iso = [0u8; 24];
                                    let l = render_iso8601(unix_secs, &mut iso);
                                    serial_println!(
                                        "{} [net16] resync {}.{}.{}.{} -> {} (stratum {}) ::",
                                        PG, server[0], server[1], server[2], server[3],
                                        core::str::from_utf8(&iso[..l]).unwrap_or("<iso>"), stratum
                                    );
                                    self.sntp_state =
                                        SntpState::Idle { due_ms: now + NET16_RESYNC_INTERVAL_MS };
                                }
                                Sntp::KissOfDeath => {
                                    NET16_REJECTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                                    serial_println!("{} [net16] resync => sntp KoD (rejected) ::", PG);
                                    self.sntp_state =
                                        SntpState::Idle { due_ms: now + NET16_RESYNC_RETRY_MS };
                                }
                                Sntp::Malformed => {
                                    NET16_REJECTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                                    serial_println!("{} [net16] resync => sntp malformed (rejected) ::", PG);
                                    self.sntp_state =
                                        SntpState::Idle { due_ms: now + NET16_RESYNC_RETRY_MS };
                                }
                            }
                        }
                    } else if now >= deadline_ms {
                        NET16_TIMEOUTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        serial_println!("{} [net16] resync => sntp timeout ::", PG);
                        self.sntp_state = SntpState::Idle { due_ms: now + NET16_RESYNC_RETRY_MS };
                    }
                }
            }
        }
    }

    /// Poll cadence: 1 per-core tick ≈ 4 ms at the 250 Hz aarch64 generic-timer tick.
    const NET9_POLL_TICKS: u64 = 1;
    const NET9_POLL_MS: u64 = 4;
    /// Rate-limit floor for the `[net9]` witness: at most once per ~5 s, unless ≥16 new replies landed.
    const NET9_REPORT_MS: i64 = 5_000;
    const NET9_REPORT_DELTA: u32 = 16;

    /// The forever net-service task (scheduled on a secondary core). Each wake polls the persistent
    /// interface so smoltcp answers ARP + ICMP echo, then prints a rate-limited `[net9]` line ONLY when
    /// the reply counts changed (default-quiet: no serial storm on a steady ping).
    fn net_service_poll(_: usize) {
        let (mut last_arp, mut last_icmp) = (0u32, 0u32);
        let mut last_report_ms: i64 = 0;
        // PI-NET-10: served-count witness state (independent of the net9 counters above).
        let mut last_served = 0u32;
        let mut last_served_report_ms: i64 = 0;
        // PI-NET-11: mDNS answered-count witness state (change-only, rate-limited like net9/net10).
        let mut last_answered = 0u32;
        let mut last_answered_report_ms: i64 = 0;
        // PI-NET-12: TCB-census witness state (change-only, rate-limited; plus a saturation flag edge).
        let mut last_census = (u32::MAX, u32::MAX);
        let mut last_census_report_ms: i64 = 0;
        let mut last_reaped = 0u32;
        let mut was_saturated = false;
        loop {
            crate::arch::sched::sleep_ticks(NET9_POLL_TICKS);
            let t = now_ms();
            let mut census = (0u32, 0u32);
            if let Some(ns) = NET_SERVICE.lock().as_mut() {
                ns.iface.poll(Instant::from_millis(t), &mut ns.dev, &mut ns.sockets);
                // PI-NET-10/12: one bounded HTTP service step (pool re-listen, drain, serve, idle-reap).
                ns.http_step(t);
                // PI-NET-11: one bounded mDNS service step (answer `unaos.local` on the share segment).
                ns.mdns_step();
                // PI-NET-16: one non-blocking SNTP re-sync step (fires a query on the ~6 h cadence, reads
                // the reply on a later poll, re-anchors the wall-clock — never blocks this poll loop).
                ns.sntp_step(t);
                census = ns.http_census();
            }
            let (arp, icmp) = net9_counts();
            if arp != last_arp || icmp != last_icmp {
                let delta = arp.wrapping_sub(last_arp) + icmp.wrapping_sub(last_icmp);
                if t.saturating_sub(last_report_ms) >= NET9_REPORT_MS || delta >= NET9_REPORT_DELTA {
                    serial_println!("{} [net9] answered arp={} icmp={} ::", PG, arp, icmp);
                    last_arp = arp;
                    last_icmp = icmp;
                    last_report_ms = t;
                }
            }
            // PI-NET-10: rate-limited, change-only served witness (mirrors the net9 counter cadence).
            let served = net10_served();
            if served != last_served
                && (t.saturating_sub(last_served_report_ms) >= NET9_REPORT_MS
                    || served.wrapping_sub(last_served) >= NET10_REPORT_DELTA)
            {
                serial_println!("{} [net10] served {} requests ::", PG, served);
                last_served = served;
                last_served_report_ms = t;
            }
            // PI-NET-11: rate-limited, change-only answered witness (same cadence as net9/net10).
            let answered = net11_answered();
            if answered != last_answered
                && (t.saturating_sub(last_answered_report_ms) >= NET9_REPORT_MS
                    || answered.wrapping_sub(last_answered) >= NET10_REPORT_DELTA)
            {
                serial_println!("{} [net11] answered {} queries ::", PG, answered);
                last_answered = answered;
                last_answered_report_ms = t;
            }

            // PI-NET-12: TCB-table census. Print when the (listen, active) shape changes and the ~5 s
            // floor has passed (default-quiet on a steady state), and ALWAYS on a reap edge or the moment
            // the backlog saturates (listen==0) — those are the diagnostic edges for the accept wedge.
            let (listen, active) = census;
            let reaped = net10_reaped();
            let saturated = listen == 0;
            let census_changed = (listen, active) != last_census;
            let reap_edge = reaped != last_reaped;
            let sat_edge = saturated && !was_saturated;
            if reap_edge
                || sat_edge
                || (census_changed && t.saturating_sub(last_census_report_ms) >= NET9_REPORT_MS)
            {
                serial_println!(
                    "{} [net12] tcbs listen={} active={} pool={} reaped={} served={}{} ::",
                    PG, listen, active, HTTP_POOL, reaped, net10_served(),
                    if saturated { " SATURATED" } else { "" }
                );
                last_census = (listen, active);
                last_census_report_ms = t;
                last_reaped = reaped;
            }
            was_saturated = saturated;
        }
    }

    /// PI-NET-13: build the HTTP listener POOL + the mDNS UDP socket into `sockets`, and join the mDNS
    /// multicast group on `iface`. Returns `(http_handles, mdns_handle, listening_count, mdns_bound,
    /// mcast_joined)`. Every socket ring buffer is `Box::leak`'d `'static` (the owned-Vec smoltcp path is
    /// off in our no_std feature set), so the handles are valid for a `SocketSet<'static>`. Shared by the
    /// metal `arm_net_service` and the QEMU loopback gate so BOTH exercise the identical pool shape.
    fn build_net_sockets(
        iface: &mut Interface,
        sockets: &mut SocketSet<'static>,
    ) -> ([SocketHandle; HTTP_POOL], SocketHandle, SocketHandle, u32, bool, bool) {
        // The Pi's TCP status service — a POOL of passive sockets, each with its own static (leaked) ring
        // buffers, all listening on :80. The pool is the accept backlog; per-socket timeout + keep-alive
        // plus the app-level idle-reaper (http_step) keep any single TCB from wedging.
        let mut http: [SocketHandle; HTTP_POOL] = [SocketHandle::default(); HTTP_POOL];
        let mut listening = 0u32;
        for slot in http.iter_mut() {
            let tcp_rx: &'static mut [u8] =
                alloc::boxed::Box::leak(alloc::boxed::Box::new([0u8; TCP_RX_CAP]));
            let tcp_tx: &'static mut [u8] =
                alloc::boxed::Box::leak(alloc::boxed::Box::new([0u8; TCP_TX_CAP]));
            let mut http_sock =
                tcp::Socket::new(tcp::SocketBuffer::new(tcp_rx), tcp::SocketBuffer::new(tcp_tx));
            // Transport-level reaping under the app idle-reaper: abort a peer that stalls mid-response,
            // and probe idle peers so a dead endpoint is detected and the timeout can fire.
            http_sock.set_timeout(Some(Duration::from_millis(TCP_TIMEOUT_MS)));
            http_sock.set_keep_alive(Some(Duration::from_millis(TCP_KEEPALIVE_MS)));
            if http_sock.listen(HTTP_PORT).is_ok() {
                listening += 1;
            }
            *slot = sockets.add(http_sock);
        }

        // PI-NET-11: the mDNS responder socket — UDP bound to 5353. Static (leaked) metadata + payload
        // rings, same owned-storage discipline as the TCP sockets above.
        let udp_rx_meta: &'static mut [udp::PacketMetadata; UDP_META_SLOTS] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([udp::PacketMetadata::EMPTY; UDP_META_SLOTS]));
        let udp_rx_payload: &'static mut [u8; UDP_RX_CAP] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([0u8; UDP_RX_CAP]));
        let udp_tx_meta: &'static mut [udp::PacketMetadata; UDP_META_SLOTS] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([udp::PacketMetadata::EMPTY; UDP_META_SLOTS]));
        let udp_tx_payload: &'static mut [u8; UDP_TX_CAP] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([0u8; UDP_TX_CAP]));
        let mut mdns_sock = udp::Socket::new(
            udp::PacketBuffer::new(&mut udp_rx_meta[..], &mut udp_rx_payload[..]),
            udp::PacketBuffer::new(&mut udp_tx_meta[..], &mut udp_tx_payload[..]),
        );
        let bound = mdns_sock.bind(MDNS_PORT).is_ok();
        let mdns = sockets.add(mdns_sock);

        // PI-NET-16: the SNTP re-sync client socket — UDP bound to the ephemeral client port, its outbound
        // queries hitting the cached time source on :123. Same static (leaked) owned-storage discipline.
        let sntp_rx_meta: &'static mut [udp::PacketMetadata; UDP_META_SLOTS] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([udp::PacketMetadata::EMPTY; UDP_META_SLOTS]));
        let sntp_rx_payload: &'static mut [u8; UDP_RX_CAP] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([0u8; UDP_RX_CAP]));
        let sntp_tx_meta: &'static mut [udp::PacketMetadata; UDP_META_SLOTS] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([udp::PacketMetadata::EMPTY; UDP_META_SLOTS]));
        let sntp_tx_payload: &'static mut [u8; UDP_TX_CAP] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new([0u8; UDP_TX_CAP]));
        let mut sntp_sock = udp::Socket::new(
            udp::PacketBuffer::new(&mut sntp_rx_meta[..], &mut sntp_rx_payload[..]),
            udp::PacketBuffer::new(&mut sntp_tx_meta[..], &mut sntp_tx_payload[..]),
        );
        let _ = sntp_sock.bind(NET16_SNTP_SPORT);
        let sntp = sockets.add(sntp_sock);

        // Join the mDNS IPv4 multicast group so smoltcp's IP layer accepts 224.0.0.251 datagrams even if
        // the RX promisc filter ever drops (proto-igmp is off, so no membership report is emitted — the
        // join updates the stack's own accept filter, which is what makes the query reach the socket).
        let mcast = IpAddress::v4(MDNS_MCAST[0], MDNS_MCAST[1], MDNS_MCAST[2], MDNS_MCAST[3]);
        let joined = iface.join_multicast_group(mcast).is_ok();

        (http, mdns, sntp, listening, bound, joined)
    }

    /// Hand the DHCP-configured `iface`/`dev` to the persistent [`NetService`] and register the poll task
    /// on a secondary core. Called at the tail of `bind_smoltcp` (a NIC is guaranteed present). The empty
    /// SocketSet uses leaked static storage (smoltcp's owned-Vec path is off in our no_std feature set).
    fn arm_net_service(mut iface: Interface, mut dev: SmoltcpPhy<GenetNic>, dns_ip: [u8; 4]) {
        // PI-NET-12/16: storage holds the HTTP listener POOL plus the mDNS and SNTP UDP sockets.
        let storage: &'static mut [SocketStorage; HTTP_POOL + 2] =
            alloc::boxed::Box::leak(alloc::boxed::Box::new(Default::default()));
        let mut sockets = SocketSet::new(&mut storage[..]);

        // PI-NET-13: build the HTTP listener pool + mDNS socket + multicast join. Extracted into
        // `build_net_sockets` so the QEMU loopback gate constructs the identical pool the metal service
        // runs — same ring caps, same timeout/keep-alive, same :80 listen, same 224.0.0.251 join. PI-NET-16
        // adds the SNTP re-sync UDP socket to the same pool.
        let (http, mdns, sntp, listening, bound, joined) = build_net_sockets(&mut iface, &mut sockets);

        // The configured IPv4 (leased or static fallback) — shown on the served status page + the mDNS
        // up-witness (mDNS answers only once configured; the address here is what a query would resolve).
        let ip = iface
            .ipv4_addr()
            .map(|a| a.octets())
            .unwrap_or(OUR_IP);

        if listening == HTTP_POOL as u32 {
            serial_println!("{} [net10] http listening :80 (pool of {}) ::", PG, HTTP_POOL);
        } else if listening > 0 {
            serial_println!(
                "{} [net10] http listening :80 ({}/{} of pool armed) ::",
                PG, listening, HTTP_POOL
            );
        } else {
            serial_println!("{} [net10] http listen(:80) FAILED — status service will not accept ::", PG);
        }
        // PI-NET-15: the filesystem route rides the same pool (GET / status, GET /fs/ listing, GET
        // /fs/<name> file bytes; files read into a bounded RAM buffer under one with_unafs hold).
        serial_println!(
            "{} [net15] fs route armed (root listing + /fs/<name>, cap {} KiB) ::",
            PG, FS_CAP / 1024
        );
        if bound && joined {
            serial_println!(
                "{} [net11] mdns responder up (unaos.local -> {}.{}.{}.{}) ::",
                PG, ip[0], ip[1], ip[2], ip[3]
            );
        } else {
            serial_println!(
                "{} [net11] mdns responder DEGRADED (bind5353={} joined224.0.0.251={}) — name resolution will not answer ::",
                PG, bound, joined
            );
        }

        // PI-NET-14: the outbound "UnaOS asks" client — DNS resolve + HTTP GET, once, now that the
        // serving pool + mDNS responder are armed and witnessed. It runs on the BSP BEFORE the net9 poll
        // task is spawned on a secondary, so there is no concurrency against the serving pool. It uses its
        // OWN temporary sockets (local `SocketSet`s scoped inside `net14_ask`, freed on return), so the
        // serving pool is never touched — the census that the net9 task first reports is a clean
        // listen=HTTP_POOL. On a bench segment without upstream reachability the honest witness is a
        // `dns timeout` / `connect timeout` line; the QEMU NET14-GATE is the correctness proof.
        net14_ask(&mut iface, &mut dev, dns_ip);

        // PI-NET-16: the initial (blocking, bounded) time sync — same discipline as net14_ask (BSP, before
        // the poll task spawns, own temporary sockets). Resolves pool.ntp.org (DNS server = the gateway
        // today, the same NetConfig cross-lane gap net14 notes; falls back to querying the gateway directly
        // as the time source), sets the wall-clock, and prints the boot witness. The resolved/fallback
        // server is cached so the poll-loop re-sync can retry it. On a bench segment without upstream the
        // honest witness is a `sntp timeout` line; the QEMU NET16-GATE is the correctness proof.
        let sntp_server = net16_initial_sync(&mut iface, &mut dev, dns_ip, dns_ip);
        // Seed the re-sync state: first opportunistic re-sync one interval out from boot.
        let sntp_state = SntpState::Idle { due_ms: now_ms() + NET16_RESYNC_INTERVAL_MS };

        *NET_SERVICE.lock() = Some(NetService {
            iface,
            dev,
            sockets,
            http,
            req_seen: [false; HTTP_POOL],
            active_since: [0; HTTP_POOL],
            mdns,
            sntp,
            sntp_server,
            sntp_state,
            ip,
            req_buf: [[0u8; REQ_CAP]; HTTP_POOL],
            req_len: [0; HTTP_POOL],
            resp: core::array::from_fn(|_| None),
        });

        // Host the task on a secondary core (never the BSP), like input/render/orphan-reaper. If no AP
        // came up, the service cannot be scheduled — report the degraded state rather than wedge the BSP.
        let online = crate::arch::smp::online_secondaries();
        // PI-SCHED-1 — net9 is a 4 ms BACKGROUND poll service. The original `online.first()` placement
        // pinned it to the RENDER core (render is `online.first()`; frame-pacing critical — the skew
        // net9's own author flagged). At net-arm time every secondary carries the same boot-capstone
        // load, so there is no live "least-loaded" signal to read yet (`sched::core_load_report()`
        // exposes the live counts for on-demand checks); instead pin net9 deterministically OFF the
        // render core, onto the background/orphan-reaper secondary (`online.get(1)` — the most-idle
        // standing service: the reaper sleeps between orphan teardowns), falling back to the input core
        // (`last`) and finally the render core only as APs thin out (single-AP boots coincide, exactly
        // as documented for the input/render split). INPUT/RENDER/reaper placements are untouched
        // (metal-proven); this only moves net9 off the render core's critical path.
        let net_cpu = online.get(1).or(online.last()).or(online.first());
        match net_cpu {
            Some(&cpu) => {
                crate::arch::sched::spawn("net9", net_service_poll, 0, cpu);
                serial_println!(
                    "{} net service task registered (poll every {} ms, core {} — off render core) ::",
                    PG, NET9_POLL_MS, cpu
                );
            }
            None => {
                serial_println!(
                    "{} net service NOT registered — no secondary core online (interface will not be polled) ::",
                    PG
                );
            }
        }
    }

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

        // PI-NET-9: the verdict above is the regression gate and has now printed. Hand the same
        // DHCP-configured `iface` + `dev` to the persistent service so the interface keeps being polled
        // and the Pi ANSWERS the gateway's ARP who-has / ICMP echo requests that arrive seconds later.
        // PI-NET-14: pass the DNS server for the outbound "UnaOS asks" client. The lease carries a DNS
        // server, but `NetConfig` (shared `net_phy.rs`, outside this lane) does not surface it yet, so we
        // use the gateway — the brief's stated fallback, and the resolver on a typical home router.
        arm_net_service(iface, dev, gw);
    }

    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    // PI-NET-14: "UnaOS asks" — the OUTBOUND client half (DNS resolver + HTTP GET).
    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    //
    // Everything above answers the network; this leg initiates. Two pure, hostile-input-hardened parsers
    // (`net14_parse_a`, `net14_http_status`) plus a bounded live driver (`net14_ask`) that runs once
    // post-lease. The DNS parser treats every byte of the response as adversarial: EVERY read is
    // bounds-checked (`get`/`checked_add`), compression pointers are NEVER dereferenced (a pointer is
    // two bytes and terminates a name — so we are immune to the classic compression-loop DoS by
    // construction, not by a visited-set), the label walk carries a hop cap, the answer walk is capped,
    // and any structural violation returns a typed error rather than panicking or looping. The whole
    // client uses its own temporary sockets and never touches the serving pool.

    /// Default host the Pi resolves + fetches. `example.com` serves a stable 200 on plain :80 (no HTTPS
    /// redirect), so a successful metal fetch is unambiguous.
    const NET14_HOST: &str = "example.com";
    /// DNS transaction id stamped on the query and required to match on the response (a mismatched id is
    /// a stale/spoofed datagram and is rejected as malformed). ASCII-ish "N4".
    const NET14_TXID: u16 = 0x4e34;
    /// Ephemeral source ports for the client sockets (DNS udp, HTTP tcp) — well clear of the well-known
    /// ports the service listens on (:80, :5353).
    const NET14_DNS_SPORT: u16 = 49517;
    const NET14_HTTP_SPORT: u16 = 49519;
    /// Bounded real-time windows (CNTPCT ms). DNS: resend the query on a cadence across the window so an
    /// unresolved-neighbor first packet (dropped during ARP) is retried; HTTP: connect + response.
    const NET14_DNS_WINDOW_MS: i64 = 2_500;
    const NET14_DNS_RESEND_MS: i64 = 400;
    const NET14_HTTP_WINDOW_MS: i64 = 4_000;
    /// Short transport timeout so a black-holed SYN aborts to a `connect timeout` witness rather than
    /// riding the whole HTTP window.
    const NET14_TCP_TIMEOUT_MS: u64 = 1_500;
    /// Bytes of the response body captured for the excerpt witness.
    const NET14_EXCERPT: usize = 80;

    /// Outcome of parsing a DNS response — a typed result so every failure mode gets its own witness.
    enum Net14Dns {
        Resolved([u8; 4]),
        Malformed,
        NoAnswer,
        ServerErr(u8),
    }

    /// Build a minimal DNS A-record query for `host` into `out`, returning its length. RD=1 standard
    /// query, one question, QTYPE A / QCLASS IN. Rejects (`None`) an empty/over-long label or a buffer
    /// too small — never writes past `out`.
    fn net14_build_dns_query(out: &mut [u8], txid: u16, host: &str) -> Option<usize> {
        if out.len() < 12 {
            return None;
        }
        out[0..2].copy_from_slice(&txid.to_be_bytes());
        out[2..4].copy_from_slice(&0x0100u16.to_be_bytes()); // flags: RD=1 (standard recursive query)
        out[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
        out[6..12].copy_from_slice(&[0, 0, 0, 0, 0, 0]); // AN/NS/AR = 0
        let mut w = 12usize;
        for label in host.split('.') {
            let lb = label.as_bytes();
            if lb.is_empty() || lb.len() > 63 {
                return None; // an empty or > 63-byte label is not a legal DNS label
            }
            if w + 1 + lb.len() > out.len() {
                return None;
            }
            out[w] = lb.len() as u8;
            w += 1;
            out[w..w + lb.len()].copy_from_slice(lb);
            w += lb.len();
        }
        if w + 5 > out.len() {
            return None;
        }
        out[w] = 0; // root label
        w += 1;
        out[w..w + 2].copy_from_slice(&1u16.to_be_bytes()); // QTYPE = A
        w += 2;
        out[w..w + 2].copy_from_slice(&1u16.to_be_bytes()); // QCLASS = IN
        w += 2;
        Some(w)
    }

    /// Return the offset just past the DNS name encoded at `off` in `pkt`. HOSTILE-INPUT HARDENED:
    /// every byte is bounds-checked; a compression pointer (top two bits set) is NEVER followed — it is
    /// two bytes and terminates the name, so returning `off + 2` is both correct for skipping and immune
    /// to compression loops by construction; a label walk carries a hop cap; reserved length bits
    /// (0x40 / 0x80) are rejected. Returns `None` on any structural violation.
    fn net14_skip_name(pkt: &[u8], mut off: usize) -> Option<usize> {
        let mut labels = 0usize;
        loop {
            let b = *pkt.get(off)?;
            match b & 0xc0 {
                0x00 => {
                    if b == 0 {
                        return Some(off + 1); // root label — end of name
                    }
                    off = off.checked_add(1 + b as usize)?;
                    if off > pkt.len() {
                        return None;
                    }
                    labels += 1;
                    if labels > 127 {
                        return None; // hop cap — a sane name has far fewer than 128 labels
                    }
                }
                0xc0 => {
                    pkt.get(off + 1)?; // bounds-check the 2nd pointer byte; we do NOT dereference it
                    return Some(off + 2);
                }
                _ => return None, // 0x40 / 0x80 reserved in the top two bits — malformed
            }
        }
    }

    /// Parse a DNS response and extract the first A record, or a typed failure. HOSTILE-INPUT HARDENED:
    /// header length checked, transaction id must match (a mismatch is a stale/spoofed datagram), the QR
    /// bit must say "response", a non-zero RCODE surfaces as `ServerErr`, the question + answer sections
    /// are walked with `net14_skip_name` (compression-loop-immune) and every fixed field is
    /// bounds-checked before read, and the answer walk is capped. No path can panic or loop unbounded.
    fn net14_parse_a(pkt: &[u8], txid: u16) -> Net14Dns {
        if pkt.len() < 12 {
            return Net14Dns::Malformed;
        }
        if pkt[0..2] != txid.to_be_bytes() {
            return Net14Dns::Malformed; // not our transaction
        }
        let flags = u16::from_be_bytes([pkt[2], pkt[3]]);
        if flags & 0x8000 == 0 {
            return Net14Dns::Malformed; // QR=0: this is a query, not a response
        }
        let rcode = (flags & 0x000f) as u8;
        if rcode != 0 {
            return Net14Dns::ServerErr(rcode); // NXDOMAIN (3), SERVFAIL (2), REFUSED (5), ...
        }
        let qd = u16::from_be_bytes([pkt[4], pkt[5]]);
        let an = u16::from_be_bytes([pkt[6], pkt[7]]);
        let mut off = 12usize;
        // Skip the question section: each question is a name + QTYPE(2) + QCLASS(2).
        for _ in 0..qd {
            off = match net14_skip_name(pkt, off) {
                Some(o) => o,
                None => return Net14Dns::Malformed,
            };
            off = match off.checked_add(4) {
                Some(o) if o <= pkt.len() => o,
                _ => return Net14Dns::Malformed,
            };
        }
        // Walk the answers (capped) for the first A/IN record with a 4-byte RDATA.
        let mut i = 0u16;
        while i < an && i < 64 {
            off = match net14_skip_name(pkt, off) {
                Some(o) => o,
                None => return Net14Dns::Malformed,
            };
            // Fixed RR header: TYPE(2) CLASS(2) TTL(4) RDLENGTH(2) = 10 bytes.
            if off + 10 > pkt.len() {
                return Net14Dns::Malformed;
            }
            let rtype = u16::from_be_bytes([pkt[off], pkt[off + 1]]);
            let rclass = u16::from_be_bytes([pkt[off + 2], pkt[off + 3]]);
            let rdlen = u16::from_be_bytes([pkt[off + 8], pkt[off + 9]]) as usize;
            let rdata = off + 10;
            if rdata + rdlen > pkt.len() {
                return Net14Dns::Malformed;
            }
            if rtype == 1 && rclass == 1 && rdlen == 4 {
                return Net14Dns::Resolved([pkt[rdata], pkt[rdata + 1], pkt[rdata + 2], pkt[rdata + 3]]);
            }
            off = rdata + rdlen; // skip CNAME / AAAA / etc. and keep looking
            i += 1;
        }
        Net14Dns::NoAnswer
    }

    /// Parse the numeric status code from an HTTP status line (`HTTP/1.x SP DDD SP ...`). Bounds-checked;
    /// returns `None` if the line is too short, not an `HTTP/` line, or the code field is not three digits.
    fn net14_http_status(resp: &[u8]) -> Option<u16> {
        if resp.len() < 12 || &resp[0..5] != b"HTTP/" {
            return None;
        }
        let d = &resp[9..12];
        let mut code = 0u16;
        for &c in d {
            if !c.is_ascii_digit() {
                return None;
            }
            code = code * 10 + (c - b'0') as u16;
        }
        Some(code)
    }

    /// Copy up to `NET14_EXCERPT` body bytes (the region after the `\r\n\r\n` header terminator) into
    /// `out`, sanitising non-printable bytes to '.', and return the sanitised length. If no header
    /// terminator is found the whole response is treated as body (still sanitised + bounded).
    fn net14_body_excerpt(resp: &[u8], out: &mut [u8; NET14_EXCERPT]) -> usize {
        // Find the CRLFCRLF header/body boundary.
        let mut body = resp.len();
        if resp.len() >= 4 {
            for i in 0..resp.len() - 3 {
                if &resp[i..i + 4] == b"\r\n\r\n" {
                    body = i + 4;
                    break;
                }
            }
        }
        let src = &resp[body.min(resp.len())..];
        let n = src.len().min(NET14_EXCERPT);
        for (o, &b) in out.iter_mut().zip(src[..n].iter()) {
            *o = if (0x20..0x7f).contains(&b) { b } else { b'.' };
        }
        n
    }

    /// PI-NET-14: the live outbound client. Resolves `NET14_HOST` via UDP :53 to `dns_ip` (bounded,
    /// with retransmit), then TCP-connects to the resolved address on :80, sends a GET, and witnesses the
    /// status line + a body excerpt. Every failure mode gets a one-line witness so a metal boot localises
    /// the failing leg. Uses only temporary, stack-buffered sockets on `iface`/`dev` — the serving pool
    /// is never touched. Bounded by construction (real-time windows over the free-running counter).
    fn net14_ask(iface: &mut Interface, dev: &mut SmoltcpPhy<GenetNic>, dns_ip: [u8; 4]) {
        // ── DNS resolve ──────────────────────────────────────────────────────────────────────────────
        let ip = {
            let mut rx_meta = [udp::PacketMetadata::EMPTY; 4];
            let mut rx_pl = [0u8; 768];
            let mut tx_meta = [udp::PacketMetadata::EMPTY; 4];
            let mut tx_pl = [0u8; 768];
            let mut sock = udp::Socket::new(
                udp::PacketBuffer::new(&mut rx_meta[..], &mut rx_pl[..]),
                udp::PacketBuffer::new(&mut tx_meta[..], &mut tx_pl[..]),
            );
            if sock.bind(NET14_DNS_SPORT).is_err() {
                serial_println!("{} [net14] dns {} => socket bind FAILED ::", PG, NET14_HOST);
                return;
            }
            let mut storage: [SocketStorage; 1] = Default::default();
            let mut sockets = SocketSet::new(&mut storage[..]);
            let h = sockets.add(sock);

            let mut qbuf = [0u8; 300];
            let Some(qlen) = net14_build_dns_query(&mut qbuf, NET14_TXID, NET14_HOST) else {
                serial_println!("{} [net14] dns {} => query build FAILED ::", PG, NET14_HOST);
                return;
            };
            let dst = IpEndpoint::new(IpAddress::v4(dns_ip[0], dns_ip[1], dns_ip[2], dns_ip[3]), 53);

            let t0 = now_ms();
            let mut next_send = t0;
            let mut outcome: Option<Net14Dns> = None;
            loop {
                let t = now_ms();
                if t.saturating_sub(t0) >= NET14_DNS_WINDOW_MS {
                    break;
                }
                iface.poll(Instant::from_millis(t), dev, &mut sockets);
                let s = sockets.get_mut::<udp::Socket>(h);
                if t >= next_send && s.can_send() {
                    // Retransmit on a cadence: the first datagram is dropped while the DNS-server neighbor
                    // is being ARP-resolved, so a single send would race the resolution and time out.
                    let _ = s.send_slice(&qbuf[..qlen], dst);
                    next_send = t + NET14_DNS_RESEND_MS;
                }
                if s.can_recv() {
                    let mut rb = [0u8; 768];
                    if let Ok((n, _meta)) = s.recv_slice(&mut rb) {
                        outcome = Some(net14_parse_a(&rb[..n], NET14_TXID));
                        break;
                    }
                }
            }

            match outcome {
                Some(Net14Dns::Resolved(ip)) => {
                    serial_println!(
                        "{} [net14] dns {} -> {}.{}.{}.{} ::",
                        PG, NET14_HOST, ip[0], ip[1], ip[2], ip[3]
                    );
                    ip
                }
                Some(Net14Dns::ServerErr(r)) => {
                    serial_println!("{} [net14] dns {} => server rcode {} ::", PG, NET14_HOST, r);
                    return;
                }
                Some(Net14Dns::NoAnswer) => {
                    serial_println!("{} [net14] dns {} => no A record ::", PG, NET14_HOST);
                    return;
                }
                Some(Net14Dns::Malformed) => {
                    serial_println!("{} [net14] dns {} => malformed response (rejected) ::", PG, NET14_HOST);
                    return;
                }
                None => {
                    serial_println!(
                        "{} [net14] dns {} => timeout (no upstream? dns={}.{}.{}.{}) ::",
                        PG, NET14_HOST, dns_ip[0], dns_ip[1], dns_ip[2], dns_ip[3]
                    );
                    return;
                }
            }
        };

        // ── HTTP GET ─────────────────────────────────────────────────────────────────────────────────
        let mut rxb = [0u8; 2048];
        let mut txb = [0u8; 512];
        let mut sock = tcp::Socket::new(
            tcp::SocketBuffer::new(&mut rxb[..]),
            tcp::SocketBuffer::new(&mut txb[..]),
        );
        sock.set_timeout(Some(Duration::from_millis(NET14_TCP_TIMEOUT_MS)));
        let mut storage: [SocketStorage; 1] = Default::default();
        let mut sockets = SocketSet::new(&mut storage[..]);
        let h = sockets.add(sock);
        let remote = (IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), HTTP_PORT);
        if sockets
            .get_mut::<tcp::Socket>(h)
            .connect(iface.context(), remote, NET14_HTTP_SPORT)
            .is_err()
        {
            serial_println!("{} [net14] GET http://{}/ => connect setup FAILED ::", PG, NET14_HOST);
            return;
        }

        // A minimal HTTP/1.1 GET with an explicit Host + Connection: close so the server closes after one
        // response. Fits `txb` comfortably.
        let mut get = [0u8; 160];
        let getlen = {
            use core::fmt::Write as _;
            let mut w = BufWriter { buf: &mut get, len: 0 };
            let _ = write!(
                w,
                "GET / HTTP/1.1\r\nHost: {}\r\nUser-Agent: UnaOS/genet\r\nConnection: close\r\n\r\n",
                NET14_HOST
            );
            w.len
        };

        let t0 = now_ms();
        let mut established = false;
        let mut sent = false;
        let mut rlen = 0usize;
        let mut resp = [0u8; 1024];
        enum Http {
            Got,
            Refused,
            Timeout,
            NoData,
        }
        let result;
        loop {
            let t = now_ms();
            if t.saturating_sub(t0) >= NET14_HTTP_WINDOW_MS {
                result = if established { Http::NoData } else { Http::Timeout };
                break;
            }
            iface.poll(Instant::from_millis(t), dev, &mut sockets);
            let s = sockets.get_mut::<tcp::Socket>(h);
            if s.may_send() {
                established = true;
                if !sent {
                    let _ = s.send_slice(&get[..getlen]);
                    sent = true;
                }
            }
            if s.can_recv() {
                let _ = s.recv(|buf| {
                    let n = buf.len().min(resp.len() - rlen);
                    resp[rlen..rlen + n].copy_from_slice(&buf[..n]);
                    rlen += n;
                    (buf.len(), ())
                });
                // Enough for the status line + a body excerpt — stop reading.
                if rlen >= 12 {
                    result = Http::Got;
                    break;
                }
            }
            if !established && !s.is_active() {
                // The socket left SYN_SENT without ever becoming writable => the peer refused (RST).
                result = Http::Refused;
                break;
            }
        }

        match result {
            Http::Got => {
                let code = net14_http_status(&resp[..rlen]).unwrap_or(0);
                let note = if code == 200 { "" } else { " (non-200)" };
                serial_println!(
                    "{} [net14] GET http://{}/ -> HTTP/1.1 {} ({} bytes){} ::",
                    PG, NET14_HOST, code, rlen, note
                );
                let mut ex = [0u8; NET14_EXCERPT];
                let exn = net14_body_excerpt(&resp[..rlen], &mut ex);
                serial_println!(
                    "{} [net14] body: {} ::",
                    PG,
                    core::str::from_utf8(&ex[..exn]).unwrap_or("<binary>")
                );
                // Close cleanly so we do not leave a TCB in the peer.
                sockets.get_mut::<tcp::Socket>(h).close();
                for _ in 0..40 {
                    iface.poll(Instant::from_millis(now_ms()), dev, &mut sockets);
                }
            }
            Http::Refused => {
                serial_println!("{} [net14] GET http://{}/ => connect refused (RST) ::", PG, NET14_HOST);
            }
            Http::Timeout => {
                serial_println!("{} [net14] GET http://{}/ => connect timeout ::", PG, NET14_HOST);
            }
            Http::NoData => {
                serial_println!("{} [net14] GET http://{}/ => connected, no response ::", PG, NET14_HOST);
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    // PI-NET-16: "UnaOS knows what time it is" — SNTP client + a monotonic-anchored kernel wall-clock.
    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    //
    // UnaOS has a free-running counter (CNTPCT) and a 250 Hz tick, but no notion of *civil* time — nothing
    // that can say "it is 2026-07-22T14:03:07Z". This section adds one, WITHOUT touching the shared
    // kernel-core time files (a proper kernel-wide clock service is noted as an integrator fold below; the
    // state here is module-local by construction). Two halves:
    //
    //   1. A hostile-input-hardened SNTP client (RFC 4330, client mode 3, v4) over UDP :123. It resolves
    //      `pool.ntp.org` via the existing NET-14 DNS client (falling back to the gateway if DNS times
    //      out), sends one request, and parses the 48-byte reply with EVERY field bounds-/sanity-checked.
    //   2. A wall-clock: an (anchor_unix_secs, anchor_cntpct) pair captured at each successful sync. The
    //      current UTC second is `anchor_unix + (cntpct_now - anchor_cntpct) / cntfrq` — the free-running
    //      counter supplies the elapsed time between syns, so the clock advances monotonically between the
    //      ~6-hourly re-syncs without any wall-clock hardware.
    //
    // The initial sync runs once (blocking, bounded) on the BSP at `arm_net_service` time, printing the
    // boot witness. Re-sync rides the persistent poll loop as a small non-blocking state machine
    // (`sntp_step`) over a UDP socket in the service's own pool, on a ~6 h cadence.

    /// Host resolved for the time source. `pool.ntp.org` load-balances across the NTP pool; any one A
    /// record is a stratum-1/2/3 server that answers plain SNTP on :123.
    const NET16_HOST: &str = "pool.ntp.org";
    /// DNS transaction id + ephemeral source ports for the SNTP client sockets (distinct from NET-14's,
    /// well clear of the well-known ports the service listens on).
    const NET16_TXID: u16 = 0x4e36;
    const NET16_DNS_SPORT: u16 = 49523;
    const NET16_SNTP_SPORT: u16 = 49525;
    /// The well-known NTP/SNTP port.
    const NTP_PORT: u16 = 123;
    /// Bounded real-time windows (CNTPCT ms). DNS mirrors NET-14 (resend across the window so an
    /// ARP-dropped first datagram is retried); the SNTP exchange is a single request with a resend cadence.
    const NET16_DNS_WINDOW_MS: i64 = 2_500;
    const NET16_DNS_RESEND_MS: i64 = 400;
    const NET16_SNTP_WINDOW_MS: i64 = 2_000;
    const NET16_SNTP_RESEND_MS: i64 = 500;
    /// Opportunistic re-sync cadence (ms) — every ~6 h of uptime the poll-loop state machine re-queries the
    /// cached server. On a failed re-sync it retries sooner rather than waiting the full interval.
    const NET16_RESYNC_INTERVAL_MS: i64 = 6 * 3_600 * 1_000;
    const NET16_RESYNC_RETRY_MS: i64 = 5 * 60 * 1_000;

    /// Seconds between the NTP epoch (1900-01-01) and the Unix epoch (1970-01-01).
    const NTP_UNIX_DELTA: u64 = 2_208_988_800;
    /// Era-1 offset (2^32 - NTP_UNIX_DELTA): added to an NTP seconds value whose high bit is CLEAR, which
    /// per RFC 4330 §3 denotes a timestamp in the era beginning 2036-02-07 (see the 2036 rollover note).
    const NTP_ERA1_OFFSET: u64 = 2_085_978_496;
    /// Sanity band on the resolved Unix time: reject anything before 2023-11 or after ~2096. A server that
    /// answers with a wildly-out-of-band timestamp (misconfigured, spoofed, or a stratum-0 KoD that slipped
    /// the stratum check) is rejected rather than jamming the clock to a nonsense year.
    const NET16_SANE_MIN_UNIX: u64 = 1_700_000_000; // ~2023-11-14
    const NET16_SANE_MAX_UNIX: u64 = 4_000_000_000; // ~2096-10

    /// The SNTP client request first byte: LI=0 (no warning), VN=4, Mode=3 (client). `(0<<6)|(4<<3)|3`.
    const SNTP_REQ_B0: u8 = 0x23;

    /// PI-NET-16 witness/diagnostic counters (the re-sync state machine bumps these; the gate asserts them).
    static NET16_SYNCS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    static NET16_TIMEOUTS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    static NET16_REJECTS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

    /// The monotonic-anchored wall-clock. Captured at each successful sync: `anchor_unix` is the UTC second
    /// the server reported; `anchor_cntpct` is the free-running counter value read in the same breath. The
    /// live time is extrapolated from the counter, so the clock ticks forward between syns with no drift
    /// beyond the counter's (and never runs backwards within one anchor).
    #[derive(Clone, Copy)]
    struct WallClock {
        anchor_unix: u64,
        anchor_cntpct: u64,
        stratum: u8,
    }
    /// Module-local wall-clock state. INTEGRATOR FOLD: a proper kernel-wide clock service belongs in a
    /// shared core file (so `time`/log timestamps/filesystem mtimes can all read it); this arc keeps the
    /// state here, in-lane, and exposes a small read API. When the shared service lands, this becomes its
    /// backing store or is migrated wholesale.
    static WALL_CLOCK: spin::Mutex<Option<WallClock>> = spin::Mutex::new(None);

    /// Anchor the wall-clock to `unix_secs` (UTC), reading CNTPCT now as the monotonic reference. Bumps the
    /// sync counter. This is the ONLY writer of `WALL_CLOCK`.
    fn wall_set(unix_secs: u64, stratum: u8) {
        *WALL_CLOCK.lock() = Some(WallClock {
            anchor_unix: unix_secs,
            anchor_cntpct: cntpct(),
            stratum,
        });
        NET16_SYNCS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }

    /// Current extrapolated UTC Unix seconds, or `None` if never synced. Adds the counter-measured elapsed
    /// seconds since the anchor to the anchored second — monotonic, non-hanging (CNTPCT is free-running).
    fn wall_unix_now() -> Option<u64> {
        let wc = (*WALL_CLOCK.lock())?;
        let frq: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
        }
        if frq == 0 {
            return Some(wc.anchor_unix);
        }
        let elapsed = cntpct().wrapping_sub(wc.anchor_cntpct) / frq;
        Some(wc.anchor_unix.saturating_add(elapsed))
    }

    /// Snapshot the raw anchor `(anchor_unix, stratum)` — the deterministic, non-extrapolated pair. Used by
    /// the gate to assert an exact rendered ISO string without racing the free-running counter.
    fn wall_anchor() -> Option<(u64, u8)> {
        (*WALL_CLOCK.lock()).map(|wc| (wc.anchor_unix, wc.stratum))
    }

    /// Civil date/time from Unix seconds (UTC), no floats. Howard Hinnant's days→civil algorithm, proven
    /// exact for the whole proleptic-Gregorian range. Returns `(year, month[1..12], day[1..31], h, m, s)`.
    fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
        let days = (secs / 86_400) as i64;
        let rem = (secs % 86_400) as u32;
        let (hh, mm, ss) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
        // Shift the epoch so era math has no negative-days special case for our band (days >= 0 here).
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097; // [0, 146096]
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11]
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
        let year = y + if month <= 2 { 1 } else { 0 };
        (year, month, d, hh, mm, ss)
    }

    /// Render `unix_secs` (UTC) as an ISO-8601 `YYYY-MM-DDTHH:MM:SSZ` string into `out`, returning its
    /// byte length (always 20 for a 4-digit year). Fixed-width, no allocation.
    fn render_iso8601(unix_secs: u64, out: &mut [u8]) -> usize {
        use core::fmt::Write as _;
        let (y, mo, d, h, mi, s) = civil_from_unix(unix_secs);
        let mut w = BufWriter { buf: out, len: 0 };
        let _ = write!(w, "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s);
        w.len
    }

    /// Typed outcome of parsing an SNTP reply — every failure mode gets its own witness.
    enum Sntp {
        /// A usable time: UTC Unix seconds + the server's stratum.
        Ok { unix_secs: u64, stratum: u8 },
        /// Stratum 0 = Kiss-o'-Death (rate-limit / deny). RFC 4330 §8: back off, do NOT use.
        KissOfDeath,
        /// Structurally invalid, wrong mode/version, LI=alarm(3), zero/insane timestamp — rejected.
        Malformed,
    }

    /// Convert a 32-bit NTP seconds field to UTC Unix seconds, era-aware (see the 2036 rollover note): a
    /// value with the high bit SET is era 0 (1900-based, 1968..2036); with it clear, era 1 (2036-based).
    fn ntp_to_unix(ntp_secs: u32) -> u64 {
        let s = ntp_secs as u64;
        if s >= NTP_UNIX_DELTA {
            s - NTP_UNIX_DELTA
        } else {
            s + NTP_ERA1_OFFSET
        }
    }

    /// Parse an SNTP server reply. HOSTILE-INPUT HARDENED: the length is checked before any field read; the
    /// LI/VN/Mode byte is decoded and an alarm LI (3), a version outside 3..=4, or a non-server mode (!=4)
    /// is rejected; stratum 0 surfaces as KoD and stratum > 15 is rejected (reserved); the transmit
    /// timestamp is read from a bounds-checked window and a zero or out-of-sanity-band time is rejected. No
    /// path can panic. `pkt` is the raw UDP payload.
    fn net16_parse_sntp(pkt: &[u8]) -> Sntp {
        if pkt.len() < 48 {
            return Sntp::Malformed; // an SNTP datagram is exactly 48 bytes (no extension/auth here)
        }
        let b0 = pkt[0];
        let li = (b0 >> 6) & 0x3;
        let vn = (b0 >> 3) & 0x7;
        let mode = b0 & 0x7;
        if li == 3 {
            return Sntp::Malformed; // LI=3: server clock unsynchronized (alarm) — do not trust its time
        }
        if !(3..=4).contains(&vn) {
            return Sntp::Malformed; // we speak SNTPv4; accept a v3 responder, reject anything else
        }
        if mode != 4 {
            return Sntp::Malformed; // Mode 4 = server. A non-server reply is stale/spoofed
        }
        let stratum = pkt[1];
        if stratum == 0 {
            return Sntp::KissOfDeath; // stratum 0 = KoD packet
        }
        if stratum > 15 {
            return Sntp::Malformed; // 16..=255 reserved / unsynchronized
        }
        // Transmit timestamp: seconds at [40..44], fraction at [44..48] (fraction unused — 1 s resolution
        // is plenty for a civil clock, and avoids float math entirely).
        let secs = u32::from_be_bytes([pkt[40], pkt[41], pkt[42], pkt[43]]);
        if secs == 0 {
            return Sntp::Malformed; // a zero transmit timestamp is not a real time
        }
        let unix = ntp_to_unix(secs);
        if !(NET16_SANE_MIN_UNIX..=NET16_SANE_MAX_UNIX).contains(&unix) {
            return Sntp::Malformed; // out of the plausible band — refuse to jam the clock to a nonsense year
        }
        Sntp::Ok { unix_secs: unix, stratum }
    }

    /// Build the 48-byte SNTP client request into `out[..48]` (LI=0/VN=4/Mode=3, all other fields zero,
    /// per RFC 4330 §5 for a client-only request).
    fn net16_build_request(out: &mut [u8; 48]) {
        *out = [0u8; 48];
        out[0] = SNTP_REQ_B0;
    }

    /// PI-NET-16: resolve `pool.ntp.org` to a single A record via UDP :53 to `dns_ip`, bounded + retransmit
    /// (the NET-14 DNS shape). Returns `None` on timeout / server-error / malformed / no-answer — the caller
    /// then falls back to the gateway. Uses its own temporary sockets; never touches the serving pool.
    fn net16_resolve(iface: &mut Interface, dev: &mut SmoltcpPhy<GenetNic>, dns_ip: [u8; 4]) -> Option<[u8; 4]> {
        let mut rx_meta = [udp::PacketMetadata::EMPTY; 4];
        let mut rx_pl = [0u8; 768];
        let mut tx_meta = [udp::PacketMetadata::EMPTY; 4];
        let mut tx_pl = [0u8; 768];
        let mut sock = udp::Socket::new(
            udp::PacketBuffer::new(&mut rx_meta[..], &mut rx_pl[..]),
            udp::PacketBuffer::new(&mut tx_meta[..], &mut tx_pl[..]),
        );
        if sock.bind(NET16_DNS_SPORT).is_err() {
            return None;
        }
        let mut storage: [SocketStorage; 1] = Default::default();
        let mut sockets = SocketSet::new(&mut storage[..]);
        let h = sockets.add(sock);

        let mut qbuf = [0u8; 300];
        let qlen = net14_build_dns_query(&mut qbuf, NET16_TXID, NET16_HOST)?;
        let dst = IpEndpoint::new(IpAddress::v4(dns_ip[0], dns_ip[1], dns_ip[2], dns_ip[3]), 53);

        let t0 = now_ms();
        let mut next_send = t0;
        loop {
            let t = now_ms();
            if t.saturating_sub(t0) >= NET16_DNS_WINDOW_MS {
                return None;
            }
            iface.poll(Instant::from_millis(t), dev, &mut sockets);
            let s = sockets.get_mut::<udp::Socket>(h);
            if t >= next_send && s.can_send() {
                let _ = s.send_slice(&qbuf[..qlen], dst);
                next_send = t + NET16_DNS_RESEND_MS;
            }
            if s.can_recv() {
                let mut rb = [0u8; 768];
                if let Ok((n, _meta)) = s.recv_slice(&mut rb) {
                    if let Net14Dns::Resolved(ip) = net14_parse_a(&rb[..n], NET16_TXID) {
                        return Some(ip);
                    }
                    return None; // a response arrived but had no usable A record — fall back
                }
            }
        }
    }

    /// PI-NET-16: one bounded, blocking SNTP exchange with `server` over UDP :123, with a resend cadence
    /// (the first datagram races ARP resolution). Returns the parsed [`Sntp`] outcome and the measured
    /// round-trip time in ms, or `None` on window timeout. Temporary sockets only.
    fn net16_query(
        iface: &mut Interface,
        dev: &mut SmoltcpPhy<GenetNic>,
        server: [u8; 4],
    ) -> Option<(Sntp, i64)> {
        let mut rx_meta = [udp::PacketMetadata::EMPTY; 4];
        let mut rx_pl = [0u8; 256];
        let mut tx_meta = [udp::PacketMetadata::EMPTY; 4];
        let mut tx_pl = [0u8; 256];
        let mut sock = udp::Socket::new(
            udp::PacketBuffer::new(&mut rx_meta[..], &mut rx_pl[..]),
            udp::PacketBuffer::new(&mut tx_meta[..], &mut tx_pl[..]),
        );
        if sock.bind(NET16_SNTP_SPORT).is_err() {
            return None;
        }
        let mut storage: [SocketStorage; 1] = Default::default();
        let mut sockets = SocketSet::new(&mut storage[..]);
        let h = sockets.add(sock);

        let mut req = [0u8; 48];
        net16_build_request(&mut req);
        let dst = IpEndpoint::new(IpAddress::v4(server[0], server[1], server[2], server[3]), NTP_PORT);

        let t0 = now_ms();
        let mut next_send = t0;
        let mut sent_at = t0;
        loop {
            let t = now_ms();
            if t.saturating_sub(t0) >= NET16_SNTP_WINDOW_MS {
                return None;
            }
            iface.poll(Instant::from_millis(t), dev, &mut sockets);
            let s = sockets.get_mut::<udp::Socket>(h);
            if t >= next_send && s.can_send() {
                if s.send_slice(&req, dst).is_ok() {
                    sent_at = t;
                }
                next_send = t + NET16_SNTP_RESEND_MS;
            }
            if s.can_recv() {
                let mut rb = [0u8; 128];
                if let Ok((n, _meta)) = s.recv_slice(&mut rb) {
                    let rtt = now_ms().saturating_sub(sent_at).max(0);
                    return Some((net16_parse_sntp(&rb[..n]), rtt));
                }
            }
        }
    }

    /// PI-NET-16: the initial (blocking, bounded) time sync, run once on the BSP at `arm_net_service`. It
    /// resolves the time source (DNS → `pool.ntp.org`, gateway fallback), queries it, sets the wall-clock,
    /// and prints the boot witness on success or an honest one-liner on each failure mode. Returns the
    /// server address to CACHE for the poll-loop re-sync (whichever address we ended up trying), or `None`
    /// if we could not even form one.
    fn net16_initial_sync(
        iface: &mut Interface,
        dev: &mut SmoltcpPhy<GenetNic>,
        dns_ip: [u8; 4],
        gw: [u8; 4],
    ) -> Option<[u8; 4]> {
        let server = match net16_resolve(iface, dev, dns_ip) {
            Some(ip) => {
                serial_println!(
                    "{} [net16] dns {} -> {}.{}.{}.{} ::",
                    PG, NET16_HOST, ip[0], ip[1], ip[2], ip[3]
                );
                ip
            }
            None => {
                serial_println!(
                    "{} [net16] dns {} => timeout — falling back to gateway {}.{}.{}.{} as time source ::",
                    PG, NET16_HOST, gw[0], gw[1], gw[2], gw[3]
                );
                gw
            }
        };

        match net16_query(iface, dev, server) {
            Some((Sntp::Ok { unix_secs, stratum }, rtt)) => {
                wall_set(unix_secs, stratum);
                let mut iso = [0u8; 24];
                let n = render_iso8601(unix_secs, &mut iso);
                serial_println!(
                    "{} [net16] sntp {}.{}.{}.{} -> {} (stratum {}, rtt ~{} ms) ::",
                    PG, server[0], server[1], server[2], server[3],
                    core::str::from_utf8(&iso[..n]).unwrap_or("<iso>"),
                    stratum, rtt
                );
            }
            Some((Sntp::KissOfDeath, _)) => {
                NET16_REJECTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                serial_println!("{} [net16] sntp {}.{}.{}.{} => sntp KoD (rejected) ::", PG,
                    server[0], server[1], server[2], server[3]);
            }
            Some((Sntp::Malformed, _)) => {
                NET16_REJECTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                serial_println!("{} [net16] sntp {}.{}.{}.{} => sntp malformed (rejected) ::", PG,
                    server[0], server[1], server[2], server[3]);
            }
            None => {
                NET16_TIMEOUTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                serial_println!("{} [net16] sntp {}.{}.{}.{} => sntp timeout ::", PG,
                    server[0], server[1], server[2], server[3]);
            }
        }
        Some(server)
    }

    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    // PI-NET-13: the QEMU TCP/HTTP/mDNS regression gate — a hardware-free loopback seam + scripted peer.
    // ══════════════════════════════════════════════════════════════════════════════════════════════════
    //
    // The GENET path no-ops under QEMU raspi4b (which models no GENET), so every TCP/HTTP/mDNS regression
    // was metal-only. This module gives that service logic a DETERMINISTIC, hardware-free home: an
    // in-kernel loopback `Device` (two frame queues wired kernel<->peer) carries frames between the REAL
    // `NetService` (the identical pool/reaper/http/mdns methods, now generic over the smoltcp `Device`)
    // and a scripted smoltcp "peer" interface that opens connections, sends GETs, floods half-opens, and
    // reads responses. A manually-advanced clock drives BOTH stacks AND the idle-reaper's deadline, so the
    // exact PI-NET-12 accept-wedge scenario (saturate -> reap -> recover) is reproducible without a NIC.
    //
    // Armed only by the `nettest` feature (UNAOS_NETTEST=1). It runs at the TOP of `genet_bringup`, before
    // the DTB skip, so QEMU raspi4b executes it. The verdict is a self-checking `:: NET-GATE: ... PASS
    // [w=0x..] ::` bitmask line the arm/kernel8 battery asserts.
    #[cfg(feature = "nettest")]
    mod nettest {
        use super::*;
        use alloc::boxed::Box;
        use alloc::collections::VecDeque;
        use alloc::vec::Vec;
        use smoltcp::iface::{Config, Context};
        use smoltcp::wire::IpCidr;

        /// The kernel service's IPv4 on the loopback segment (the peer connects here).
        const KIP: [u8; 4] = [10, 0, 0, 1];
        /// The scripted peer's IPv4.
        const PIP: [u8; 4] = [10, 0, 0, 2];
        const KMAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x01];
        const PMAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x02];
        /// Wall-clock ms each pump iteration advances the shared clock. Small enough that a handshake
        /// completes in a few iters, large enough that a bounded pump covers seconds cheaply.
        const STEP_MS: i64 = 20;
        /// Per-peer-socket ring buffers (a request line + the status page both fit comfortably).
        const PEER_CAP: usize = 2048;

        // ── The loopback channel: two frame FIFOs. `K_RX` = frames destined for the KERNEL (peer TX);
        //    `P_RX` = frames destined for the PEER (kernel TX). `VecDeque::new` is not `const`, so they
        //    are `Option`-wrapped and initialised at `run()` entry. A hard cap drops frames rather than
        //    growing unbounded if some pathology loops (a test must never OOM the boot). ──
        static K_RX: spin::Mutex<Option<VecDeque<Vec<u8>>>> = spin::Mutex::new(None);
        static P_RX: spin::Mutex<Option<VecDeque<Vec<u8>>>> = spin::Mutex::new(None);
        const QUEUE_CAP: usize = 256;

        fn push(q: &spin::Mutex<Option<VecDeque<Vec<u8>>>>, frame: &[u8]) {
            if let Some(dq) = q.lock().as_mut() {
                if dq.len() < QUEUE_CAP {
                    dq.push_back(frame.to_vec());
                }
            }
        }
        fn pop(q: &spin::Mutex<Option<VecDeque<Vec<u8>>>>, out: &mut [u8]) -> Option<usize> {
            let frame = q.lock().as_mut()?.pop_front()?;
            let n = frame.len().min(out.len());
            out[..n].copy_from_slice(&frame[..n]);
            Some(n)
        }

        /// The kernel side of the loopback: RX drains `K_RX` (peer -> kernel); TX fills `P_RX`.
        struct KernelLoopNic;
        impl RawNic for KernelLoopNic {
            fn rx_frame_raw(out: &mut [u8]) -> Option<usize> {
                pop(&K_RX, out)
            }
            fn transmit(frame: &[u8]) {
                push(&P_RX, frame);
            }
            fn mac() -> Option<[u8; 6]> {
                Some(KMAC)
            }
        }
        /// The peer side: mirror image — RX drains `P_RX`; TX fills `K_RX`.
        struct PeerLoopNic;
        impl RawNic for PeerLoopNic {
            fn rx_frame_raw(out: &mut [u8]) -> Option<usize> {
                pop(&P_RX, out)
            }
            fn transmit(frame: &[u8]) {
                push(&K_RX, frame);
            }
            fn mac() -> Option<[u8; 6]> {
                Some(PMAC)
            }
        }

        /// Build a smoltcp `Interface` with a static /24 address on the loopback segment.
        fn build_iface<D: Device>(dev: &mut D, mac: [u8; 6], ip: [u8; 4], seed: u64) -> Interface {
            let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
            config.random_seed = seed;
            let mut iface = Interface::new(config, dev, Instant::from_millis(0));
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), 24));
            });
            iface
        }

        /// One shared pump step across BOTH stacks and the service logic, at the current clock. Kernel
        /// polls first (drains its RX, runs the pool/reaper/http/mdns service, emits replies), then the
        /// peer polls (drains those replies, emits its next frames). Advances `clk` by `STEP_MS`.
        fn pump(
            ks: &mut NetService<SmoltcpPhy<KernelLoopNic>>,
            pface: &mut Interface,
            pdev: &mut SmoltcpPhy<PeerLoopNic>,
            psock: &mut SocketSet<'static>,
            clk: &mut i64,
            iters: usize,
        ) {
            for _ in 0..iters {
                let t = Instant::from_millis(*clk);
                ks.iface.poll(t, &mut ks.dev, &mut ks.sockets);
                ks.http_step(*clk);
                ks.mdns_step();
                pface.poll(t, pdev, psock);
                *clk += STEP_MS;
            }
        }

        /// Open a fresh peer TCP socket (leaked `'static` rings) and actively connect it to the kernel
        /// service's `:80` from `local_port`. Returns its handle.
        fn open_peer(psock: &mut SocketSet<'static>, cx: &mut Context, local_port: u16) -> SocketHandle {
            let rx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
            let tx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
            let mut s = tcp::Socket::new(tcp::SocketBuffer::new(rx), tcp::SocketBuffer::new(tx));
            let _ = s.connect(cx, (IpAddress::v4(KIP[0], KIP[1], KIP[2], KIP[3]), HTTP_PORT), local_port);
            psock.add(s)
        }

        /// A full client fetch on peer socket `h`: wait for the handshake, send a GET, drain the reply,
        /// and report whether the status line was `HTTP/1.0 200`. Bounded by `pump`'s iteration budget.
        fn fetch_200(
            ks: &mut NetService<SmoltcpPhy<KernelLoopNic>>,
            pface: &mut Interface,
            pdev: &mut SmoltcpPhy<PeerLoopNic>,
            psock: &mut SocketSet<'static>,
            clk: &mut i64,
            h: SocketHandle,
        ) -> bool {
            // Wait for the handshake to complete (peer may_send), then push the request once.
            let mut sent = false;
            let mut got200 = false;
            for _ in 0..80 {
                pump(ks, pface, pdev, psock, clk, 1);
                let s = psock.get_mut::<tcp::Socket>(h);
                if !sent && s.may_send() {
                    let _ = s.send_slice(b"GET / HTTP/1.0\r\n\r\n");
                    sent = true;
                }
                if s.can_recv() {
                    let _ = s.recv(|buf| {
                        if buf.len() >= 12 && &buf[..12] == b"HTTP/1.0 200" {
                            got200 = true;
                        }
                        (buf.len(), ())
                    });
                }
                if got200 {
                    break;
                }
            }
            // Close the peer half so the kernel's FIN handshake can complete to TIME-WAIT (which
            // `http_step` aborts + re-listens) rather than stalling in FIN_WAIT_2 forever — otherwise the
            // served TCB never recycles. Pump a little to walk both sides through the close.
            if got200 {
                psock.get_mut::<tcp::Socket>(h).close();
                pump(ks, pface, pdev, psock, clk, 20);
            }
            got200
        }

        /// Run the loopback battery and print the `NET-GATE` witness. Bitmask `w`:
        ///   0x1 basic handshake + GET -> 200; 0x2 half-open flood -> SATURATED -> reaped -> recovery
        ///   fetch; 0x4 FIN close recycles the TCB back to LISTEN; 0x8 table-full connection gets RST.
        pub fn run() {
            *K_RX.lock() = Some(VecDeque::new());
            *P_RX.lock() = Some(VecDeque::new());

            // Kernel service: the REAL NetService methods over the loopback phy + the identical socket pool.
            let mut kdev = SmoltcpPhy::<KernelLoopNic>::new();
            let mut kiface = build_iface(&mut kdev, KMAC, KIP, 0x4b45_524e); // "KERN"
            let kstorage: &'static mut [SocketStorage; HTTP_POOL + 2] =
                Box::leak(Box::new(Default::default()));
            let mut ksockets = SocketSet::new(&mut kstorage[..]);
            let (http, mdns, sntp, listening, _bound, _joined) =
                build_net_sockets(&mut kiface, &mut ksockets);
            let mut ks = NetService {
                iface: kiface,
                dev: kdev,
                sockets: ksockets,
                http,
                req_seen: [false; HTTP_POOL],
                active_since: [0; HTTP_POOL],
                mdns,
                sntp,
                sntp_server: None,
                sntp_state: SntpState::Idle { due_ms: i64::MAX },
                ip: KIP,
                req_buf: [[0u8; REQ_CAP]; HTTP_POOL],
                req_len: [0; HTTP_POOL],
                resp: core::array::from_fn(|_| None),
            };

            // Peer stack: a plain smoltcp interface + its own socket set on the other end of the channel.
            let mut pdev = SmoltcpPhy::<PeerLoopNic>::new();
            let mut pface = build_iface(&mut pdev, PMAC, PIP, 0x50_4545_52); // "PEER"
            let pstorage: &'static mut [SocketStorage; 16] = Box::leak(Box::new(Default::default()));
            let mut psock = SocketSet::new(&mut pstorage[..]);

            let mut clk: i64 = 0;
            let mut w: u32 = 0;

            serial_println!("{} [net13] loopback gate: pool armed listening={}/{} ::", PG, listening, HTTP_POOL);

            // ── Scenario A (0x1): a full client handshake + GET returns the 200 status page. ──
            {
                let h = open_peer(&mut psock, pface.context(), 49001);
                let ok = fetch_200(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, h);
                if ok {
                    w |= 0x1;
                }
                serial_println!("{} [net13] A handshake+GET => {} ::", PG, if ok { "200 PASS" } else { "FAIL" });
            }

            // ── Scenario C (0x4): after the served connection closes, the TCB recycles back to LISTEN
            //    (FIN handshake -> TIME-WAIT -> abort -> re-listen), so the whole pool is listening again. ──
            {
                pump(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, 80);
                let (listen, _active) = ks.http_census();
                let recycled = listen == HTTP_POOL as u32;
                if recycled {
                    w |= 0x4;
                }
                serial_println!(
                    "{} [net13] C fin-close recycles => listen={}/{} {} ::",
                    PG, listen, HTTP_POOL, if recycled { "PASS" } else { "FAIL" }
                );
            }

            // ── Scenario B/D (0x2 + 0x8): half-open flood saturates the pool, a table-full connection is
            //    RST, then the idle-reaper frees the wedged TCBs and a fresh fetch recovers. ──
            {
                // Open HTTP_POOL half-open connections (connect, never send a request).
                let mut flood = [SocketHandle::default(); HTTP_POOL];
                for (i, slot) in flood.iter_mut().enumerate() {
                    *slot = open_peer(&mut psock, pface.context(), 49100 + i as u16);
                }
                // Pump briefly (well under the 3 s reap deadline) to complete all four handshakes.
                pump(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, 40);
                let (listen_sat, active_sat) = ks.http_census();
                let saturated = listen_sat == 0;
                serial_println!(
                    "{} [net13] B flood => listen={} active={} {} ::",
                    PG, listen_sat, active_sat, if saturated { "SATURATED" } else { "not-saturated" }
                );

                // Scenario D: a 5th connection while saturated finds no listening TCB and is RST.
                let extra = open_peer(&mut psock, pface.context(), 49200);
                pump(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, 30);
                let refused = !psock.get_mut::<tcp::Socket>(extra).is_active();
                if saturated && refused {
                    w |= 0x8;
                }
                serial_println!(
                    "{} [net13] D table-full 5th conn => {} ::",
                    PG, if refused { "RST/refused PASS" } else { "accepted FAIL" }
                );

                // Advance the clock GRADUALLY past the idle-reap deadline. A single big jump would let
                // smoltcp's transport keepalive/timeout abort the wedged TCBs itself (no reap count);
                // stepping keeps the peer answering keepalive probes so the connections stay ESTABLISHED
                // until the APP idle-reaper (`http_step`, HTTP_IDLE_MS) is the one that force-aborts them —
                // exactly the PI-NET-12 wedge fix under test. ~4.4 s of steps clears the 3 s deadline.
                let reaped_before = net10_reaped();
                pump(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, 220);
                let reaped_delta = net10_reaped().wrapping_sub(reaped_before);
                let (listen_rec, _a) = ks.http_census();
                serial_println!(
                    "{} [net13] B reaped +{} (listen recovered={}/{}) ::",
                    PG, reaped_delta, listen_rec, HTTP_POOL
                );

                // Recovery fetch: the service must accept + serve again after the flood was reaped.
                let r = open_peer(&mut psock, pface.context(), 49300);
                let recovered = fetch_200(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, r);
                if saturated && reaped_delta >= 1 && recovered {
                    w |= 0x2;
                }
                serial_println!(
                    "{} [net13] B recovery fetch => {} ::",
                    PG, if recovered { "200 PASS" } else { "FAIL" }
                );
            }

            // ── mDNS (diagnostic, not gated): a query for unaos.local over the same loopback is answered.
            //    Exercises the SAME `mdns_step` the metal responder runs, over the loopback Device. ──
            let mdns_ok = mdns_probe(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk);
            serial_println!("{} [net13] mdns unaos.local => {} ::", PG, if mdns_ok { "answered" } else { "no-answer" });

            let pass = w == 0xf;
            serial_println!(
                ":: NET-GATE: tcp/http/mdns loopback battery {} [w=0x{:x}] (basic|flood-reap-recover|fin-recycle|table-full) ::",
                if pass { "PASS" } else { "FAIL" }, w
            );
        }

        /// Build a well-formed DNS A response for `example.com` -> `ip` with transaction id `txid`. The
        /// answer name uses a compression pointer (0xC0 0x0C) back to the question name — the exact shape
        /// a real resolver emits — so the gate exercises `net14_skip_name`'s pointer handling.
        fn build_a_response(txid: u16, ip: [u8; 4], rcode: u8, ancount: u16) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&txid.to_be_bytes());
            v.extend_from_slice(&(0x8180u16 | rcode as u16).to_be_bytes()); // QR=1 RD=1 RA=1 + rcode
            v.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
            v.extend_from_slice(&ancount.to_be_bytes()); // ANCOUNT
            v.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
            v.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
            // Question: example.com A IN (name starts at offset 12).
            v.push(7);
            v.extend_from_slice(b"example");
            v.push(3);
            v.extend_from_slice(b"com");
            v.push(0);
            v.extend_from_slice(&1u16.to_be_bytes()); // QTYPE A
            v.extend_from_slice(&1u16.to_be_bytes()); // QCLASS IN
            if ancount > 0 {
                // Answer: name = pointer to offset 12, A IN, ttl 300, rdlen 4, rdata ip.
                v.extend_from_slice(&[0xc0, 0x0c]);
                v.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
                v.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
                v.extend_from_slice(&300u32.to_be_bytes()); // TTL
                v.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
                v.extend_from_slice(&ip);
            }
            v
        }

        /// Poll both loopback stacks once at the shared clock and advance it (client gate variant — no
        /// service methods; the kernel side runs a bare client socket).
        fn pump14(
            kiface: &mut Interface,
            kdev: &mut SmoltcpPhy<KernelLoopNic>,
            ksock: &mut SocketSet<'static>,
            piface: &mut Interface,
            pdev: &mut SmoltcpPhy<PeerLoopNic>,
            psock: &mut SocketSet<'static>,
            clk: &mut i64,
            iters: usize,
        ) {
            for _ in 0..iters {
                let t = Instant::from_millis(*clk);
                kiface.poll(t, kdev, ksock);
                piface.poll(t, pdev, psock);
                *clk += STEP_MS;
            }
        }

        /// PI-NET-14: the OUTBOUND client gate — the counterpart to `run()` (which tests the service half).
        /// Bitmask `w`:
        ///   0x01 parser accepts a well-formed A response; 0x02 rejects a truncated response;
        ///   0x04 rejects a name that never terminates (hop cap / no compression-loop hang);
        ///   0x08 surfaces a server RCODE; 0x10 a live loopback DNS query/response resolves;
        ///   0x20 a live loopback HTTP GET returns 200; 0x40 connect-to-closed-port is refused (RST);
        ///   0x80 connect-to-black-hole times out.
        pub fn run14() {
            *K_RX.lock() = Some(VecDeque::new());
            *P_RX.lock() = Some(VecDeque::new());
            let mut w: u32 = 0;

            // ── Pure hostile-input parser checks (no sockets — the security surface tested in isolation). ─
            let good = build_a_response(NET14_TXID, [93, 184, 216, 34], 0, 1);
            if let Net14Dns::Resolved([93, 184, 216, 34]) = net14_parse_a(&good, NET14_TXID) {
                w |= 0x01;
            }
            serial_println!(
                "{} [net14t] parse well-formed A => {} ::",
                PG, if w & 0x01 != 0 { "resolved PASS" } else { "FAIL" }
            );

            // Truncate the well-formed response mid-answer (drop the last 3 rdata bytes): rdlen now
            // over-runs the buffer, which the bounds check must reject.
            let mut trunc = good.clone();
            trunc.truncate(trunc.len() - 3);
            let rej_trunc = matches!(net14_parse_a(&trunc, NET14_TXID), Net14Dns::Malformed);
            if rej_trunc {
                w |= 0x02;
            }
            serial_println!(
                "{} [net14t] reject truncated => {} ::",
                PG, if rej_trunc { "malformed PASS" } else { "FAIL" }
            );

            // A name that is 130 one-byte labels with NO root terminator: `net14_skip_name` must bail
            // (hop cap / running past the buffer) rather than loop — the compression-loop-DoS guard.
            let mut loopname = Vec::new();
            for _ in 0..130 {
                loopname.push(1u8);
                loopname.push(b'a');
            }
            let rej_loop = net14_skip_name(&loopname, 0).is_none();
            if rej_loop {
                w |= 0x04;
            }
            serial_println!(
                "{} [net14t] reject non-terminating name => {} ::",
                PG, if rej_loop { "bailed PASS" } else { "FAIL" }
            );

            // A response carrying RCODE=3 (NXDOMAIN) must surface as a server error, not a resolve.
            let nx = build_a_response(NET14_TXID, [0, 0, 0, 0], 3, 0);
            let rcode_ok = matches!(net14_parse_a(&nx, NET14_TXID), Net14Dns::ServerErr(3));
            if rcode_ok {
                w |= 0x08;
            }
            serial_println!(
                "{} [net14t] surface server rcode => {} ::",
                PG, if rcode_ok { "rcode3 PASS" } else { "FAIL" }
            );

            // ── Live loopback socket scenarios. Kernel = client (KIP), peer = server (PIP). ────────────
            let mut kdev = SmoltcpPhy::<KernelLoopNic>::new();
            let mut kiface = build_iface(&mut kdev, KMAC, KIP, 0x4b31_3400); // "K14"
            let mut pdev = SmoltcpPhy::<PeerLoopNic>::new();
            let mut piface = build_iface(&mut pdev, PMAC, PIP, 0x5031_3400); // "P14"
            let kstore: &'static mut [SocketStorage; 4] = Box::leak(Box::new(Default::default()));
            let mut ksock = SocketSet::new(&mut kstore[..]);
            let pstore: &'static mut [SocketStorage; 4] = Box::leak(Box::new(Default::default()));
            let mut psock = SocketSet::new(&mut pstore[..]);
            let mut clk: i64 = 0;

            // Scenario 0x10: DNS query/response over the loopback. Kernel udp -> peer udp :53 -> A reply.
            {
                let krx: &'static mut [udp::PacketMetadata; 4] =
                    Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4]));
                let krxp: &'static mut [u8; 768] = Box::leak(Box::new([0u8; 768]));
                let ktx: &'static mut [udp::PacketMetadata; 4] =
                    Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4]));
                let ktxp: &'static mut [u8; 768] = Box::leak(Box::new([0u8; 768]));
                let mut ku = udp::Socket::new(
                    udp::PacketBuffer::new(&mut krx[..], &mut krxp[..]),
                    udp::PacketBuffer::new(&mut ktx[..], &mut ktxp[..]),
                );
                let _ = ku.bind(NET14_DNS_SPORT);
                let kh = ksock.add(ku);

                let prx: &'static mut [udp::PacketMetadata; 4] =
                    Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4]));
                let prxp: &'static mut [u8; 768] = Box::leak(Box::new([0u8; 768]));
                let ptx: &'static mut [udp::PacketMetadata; 4] =
                    Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4]));
                let ptxp: &'static mut [u8; 768] = Box::leak(Box::new([0u8; 768]));
                let mut pu = udp::Socket::new(
                    udp::PacketBuffer::new(&mut prx[..], &mut prxp[..]),
                    udp::PacketBuffer::new(&mut ptx[..], &mut ptxp[..]),
                );
                let _ = pu.bind(53u16);
                let ph = psock.add(pu);

                let mut qbuf = [0u8; 300];
                let qlen = net14_build_dns_query(&mut qbuf, NET14_TXID, "example.com").unwrap_or(0);
                let dst = IpEndpoint::new(IpAddress::v4(PIP[0], PIP[1], PIP[2], PIP[3]), 53);

                let mut resolved: Option<[u8; 4]> = None;
                let mut sent = false;
                for _ in 0..60 {
                    pump14(&mut kiface, &mut kdev, &mut ksock, &mut piface, &mut pdev, &mut psock, &mut clk, 1);
                    let ks = ksock.get_mut::<udp::Socket>(kh);
                    if !sent && ks.can_send() {
                        let _ = ks.send_slice(&qbuf[..qlen], dst);
                        sent = true;
                    }
                    if ks.can_recv() {
                        let mut rb = [0u8; 768];
                        if let Ok((n, _m)) = ks.recv_slice(&mut rb) {
                            if let Net14Dns::Resolved(ip) = net14_parse_a(&rb[..n], NET14_TXID) {
                                resolved = Some(ip);
                            }
                            break;
                        }
                    }
                    // Peer: on receiving the query, answer with a well-formed A record to the querier.
                    let ps = psock.get_mut::<udp::Socket>(ph);
                    if ps.can_recv() {
                        let mut qb = [0u8; 768];
                        if let Ok((_n, meta)) = ps.recv_slice(&mut qb) {
                            let resp = build_a_response(NET14_TXID, [93, 184, 216, 34], 0, 1);
                            let _ = ps.send_slice(&resp, meta.endpoint);
                        }
                    }
                }
                let dns_rt = resolved == Some([93, 184, 216, 34]);
                if dns_rt {
                    w |= 0x10;
                }
                serial_println!(
                    "{} [net14] loopback dns example.com => {} ::",
                    PG, if dns_rt { "93.184.216.34 PASS" } else { "FAIL" }
                );
            }

            // Scenario 0x20: HTTP GET over the loopback. Peer listens :80, answers 200; kernel fetches.
            {
                let plisten = {
                    let rx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
                    let tx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
                    let mut s = tcp::Socket::new(tcp::SocketBuffer::new(rx), tcp::SocketBuffer::new(tx));
                    let _ = s.listen(80u16);
                    psock.add(s)
                };
                let kfetch = {
                    let rx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
                    let tx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
                    let mut s = tcp::Socket::new(tcp::SocketBuffer::new(rx), tcp::SocketBuffer::new(tx));
                    s.set_timeout(Some(Duration::from_millis(NET14_TCP_TIMEOUT_MS)));
                    let _ = s.connect(
                        kiface.context(),
                        (IpAddress::v4(PIP[0], PIP[1], PIP[2], PIP[3]), 80),
                        NET14_HTTP_SPORT,
                    );
                    ksock.add(s)
                };
                let mut got200 = false;
                let mut sent = false;
                let mut answered = false;
                for _ in 0..100 {
                    pump14(&mut kiface, &mut kdev, &mut ksock, &mut piface, &mut pdev, &mut psock, &mut clk, 1);
                    // Kernel client: send GET once writable, capture status on recv.
                    let ks = ksock.get_mut::<tcp::Socket>(kfetch);
                    if ks.may_send() && !sent {
                        let _ = ks.send_slice(b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n");
                        sent = true;
                    }
                    if ks.can_recv() {
                        let mut rb = [0u8; 256];
                        let mut n = 0usize;
                        let _ = ks.recv(|buf| {
                            n = buf.len().min(rb.len());
                            rb[..n].copy_from_slice(&buf[..n]);
                            (buf.len(), ())
                        });
                        if net14_http_status(&rb[..n]) == Some(200) {
                            got200 = true;
                            break;
                        }
                    }
                    // Peer server: once the request is in, answer a 200 and close.
                    let ps = psock.get_mut::<tcp::Socket>(plisten);
                    if ps.can_recv() && !answered {
                        let mut junk = [0u8; 256];
                        let _ = ps.recv_slice(&mut junk);
                        let _ = ps.send_slice(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                        );
                        ps.close();
                        answered = true;
                    }
                }
                if got200 {
                    w |= 0x20;
                }
                serial_println!(
                    "{} [net14] loopback GET => {} ::",
                    PG, if got200 { "HTTP/1.1 200 PASS" } else { "FAIL" }
                );
            }

            // Scenario 0x40: connect to a CLOSED peer port (:81, no listener) => the peer RSTs the SYN.
            {
                let rx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
                let tx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
                let mut s = tcp::Socket::new(tcp::SocketBuffer::new(rx), tcp::SocketBuffer::new(tx));
                s.set_timeout(Some(Duration::from_millis(NET14_TCP_TIMEOUT_MS)));
                let _ = s.connect(
                    kiface.context(),
                    (IpAddress::v4(PIP[0], PIP[1], PIP[2], PIP[3]), 81),
                    NET14_HTTP_SPORT + 2,
                );
                let h = ksock.add(s);
                let mut refused = false;
                let mut established = false;
                for _ in 0..40 {
                    pump14(&mut kiface, &mut kdev, &mut ksock, &mut piface, &mut pdev, &mut psock, &mut clk, 1);
                    let s = ksock.get_mut::<tcp::Socket>(h);
                    if s.may_send() {
                        established = true;
                    }
                    if !established && !s.is_active() {
                        refused = true; // RST closed the SYN_SENT socket before it ever became writable
                        break;
                    }
                }
                if refused {
                    w |= 0x40;
                }
                serial_println!(
                    "{} [net14] connect closed-port => {} ::",
                    PG, if refused { "RST/refused PASS" } else { "FAIL" }
                );
            }

            // Scenario 0x80: connect to a BLACK HOLE (10.0.0.9 — no host on the segment). ARP goes
            // unanswered, the SYN never lands, and the transport timeout aborts the socket => timeout.
            {
                let rx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
                let tx: &'static mut [u8] = Box::leak(Box::new([0u8; PEER_CAP]));
                let mut s = tcp::Socket::new(tcp::SocketBuffer::new(rx), tcp::SocketBuffer::new(tx));
                s.set_timeout(Some(Duration::from_millis(NET14_TCP_TIMEOUT_MS)));
                let _ = s.connect(
                    kiface.context(),
                    (IpAddress::v4(10, 0, 0, 9), 80),
                    NET14_HTTP_SPORT + 3,
                );
                let h = ksock.add(s);
                let mut established = false;
                let mut timed_out = false;
                // Pump well past the transport timeout (STEP_MS * iters >> NET14_TCP_TIMEOUT_MS).
                for _ in 0..200 {
                    pump14(&mut kiface, &mut kdev, &mut ksock, &mut piface, &mut pdev, &mut psock, &mut clk, 1);
                    let s = ksock.get_mut::<tcp::Socket>(h);
                    if s.may_send() {
                        established = true;
                        break; // should NEVER happen — there is no host at .9
                    }
                    if !s.is_active() {
                        timed_out = true;
                        break;
                    }
                }
                if timed_out && !established {
                    w |= 0x80;
                }
                serial_println!(
                    "{} [net14] connect black-hole => {} ::",
                    PG, if timed_out && !established { "timeout PASS" } else { "FAIL" }
                );
            }

            let pass = w == 0xff;
            serial_println!(
                ":: NET14-GATE: dns/http client battery {} [w=0x{:x}] (parse-ok|parse-malformed|parse-loop|parse-rcode|dns-rt|http-200|refused|timeout) ::",
                if pass { "PASS" } else { "FAIL" }, w
            );
        }

        // ── PI-NET-15 gate helpers ────────────────────────────────────────────────────────────────────
        /// The K3 fixture volume the kernel8-test image carries (staged by `arroyo`): a known text file
        /// and a 12 KiB pattern file (byte i = (i*7+3)&0xFF) spanning several unafs blocks.
        const FIX_HELLO: &[u8] = b"Hello from native UnaFS on the Pi 4!\n";

        /// Does `hay` contain the contiguous subsequence `needle`?
        fn contains_sub(hay: &[u8], needle: &[u8]) -> bool {
            !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
        }
        /// Parse the numeric status code out of an `HTTP/1.0 NNN ...` response line.
        fn status_code(resp: &[u8]) -> Option<u32> {
            let rest = resp.strip_prefix(b"HTTP/1.0 ")?;
            core::str::from_utf8(rest.get(..3)?).ok()?.parse().ok()
        }
        /// The response body (everything past the `\r\n\r\n` header terminator).
        fn body_of(resp: &[u8]) -> &[u8] {
            resp.windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| &resp[p + 4..])
                .unwrap_or(&[])
        }
        /// Drive one full peer request against the kernel serving pool and return the complete response
        /// bytes. Sends `req` once the handshake completes, drains every reply (so the kernel can keep
        /// streaming a multi-block file), and stops once the kernel has closed its half.
        fn fetch_full(
            ks: &mut NetService<SmoltcpPhy<KernelLoopNic>>,
            pface: &mut Interface,
            pdev: &mut SmoltcpPhy<PeerLoopNic>,
            psock: &mut SocketSet<'static>,
            clk: &mut i64,
            h: SocketHandle,
            req: &[u8],
        ) -> Vec<u8> {
            let mut sent = false;
            let mut acc: Vec<u8> = Vec::new();
            for _ in 0..300 {
                pump(ks, pface, pdev, psock, clk, 1);
                let s = psock.get_mut::<tcp::Socket>(h);
                if !sent && s.may_send() {
                    let _ = s.send_slice(req);
                    sent = true;
                }
                if s.can_recv() {
                    let _ = s.recv(|buf| {
                        acc.extend_from_slice(buf);
                        (buf.len(), ())
                    });
                }
                if sent && !acc.is_empty() && !s.may_recv() {
                    break;
                }
            }
            psock.get_mut::<tcp::Socket>(h).close();
            pump(ks, pface, pdev, psock, clk, 20);
            acc
        }

        /// PI-NET-15: the FILESYSTEM route gate — the scripted peer fetches the unafs volume off the SAME
        /// serving pool `run()` exercises, reading the K3 fixtures the kernel8-test image carries. Bitmask
        /// `w`: 0x01 `/fs/` lists a fixture; 0x02 `/fs/K3HELLO.TXT` returns the exact bytes; 0x04 a
        /// traversal `/fs/../evil` is rejected (404, not a 200); 0x08 a missing file 404s; 0x10 an oversize
        /// file is refused 413 (driven against K3PAT.BIN via a gate-scoped cap override — no card state
        /// added). PASS = 0x1f. A multi-block full-serve of K3PAT.BIN at the real cap is a diagnostic line.
        pub fn run15() {
            *K_RX.lock() = Some(VecDeque::new());
            *P_RX.lock() = Some(VecDeque::new());

            let mut kdev = SmoltcpPhy::<KernelLoopNic>::new();
            let mut kiface = build_iface(&mut kdev, KMAC, KIP, 0x4b31_3500); // "K15"
            let kstorage: &'static mut [SocketStorage; HTTP_POOL + 2] =
                Box::leak(Box::new(Default::default()));
            let mut ksockets = SocketSet::new(&mut kstorage[..]);
            let (http, mdns, sntp, listening, _b, _j) = build_net_sockets(&mut kiface, &mut ksockets);
            let mut ks = NetService {
                iface: kiface,
                dev: kdev,
                sockets: ksockets,
                http,
                req_seen: [false; HTTP_POOL],
                active_since: [0; HTTP_POOL],
                mdns,
                sntp,
                sntp_server: None,
                sntp_state: SntpState::Idle { due_ms: i64::MAX },
                ip: KIP,
                req_buf: [[0u8; REQ_CAP]; HTTP_POOL],
                req_len: [0; HTTP_POOL],
                resp: core::array::from_fn(|_| None),
            };

            let mut pdev = SmoltcpPhy::<PeerLoopNic>::new();
            let mut pface = build_iface(&mut pdev, PMAC, PIP, 0x5031_3500); // "P15"
            let pstorage: &'static mut [SocketStorage; 16] = Box::leak(Box::new(Default::default()));
            let mut psock = SocketSet::new(&mut pstorage[..]);

            let mut clk: i64 = 0;
            let mut w: u32 = 0;
            serial_println!(
                "{} [net15] fs gate: pool armed listening={}/{} (cap {} KiB) ::",
                PG, listening, HTTP_POOL, FS_CAP / 1024
            );

            // ── A (0x01): GET /fs/ lists the K3HELLO.TXT fixture. ──
            {
                let h = open_peer(&mut psock, pface.context(), 49401);
                let resp = fetch_full(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, h,
                    b"GET /fs/ HTTP/1.0\r\n\r\n");
                let ok = status_code(&resp) == Some(200) && contains_sub(&resp, b"K3HELLO.TXT");
                if ok { w |= 0x01; }
                serial_println!("{} [net15] A GET /fs/ lists fixture => {} ::", PG, if ok { "PASS" } else { "FAIL" });
            }

            // ── B (0x02): GET /fs/K3HELLO.TXT returns the exact fixture bytes. ──
            {
                let h = open_peer(&mut psock, pface.context(), 49402);
                let resp = fetch_full(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, h,
                    b"GET /fs/K3HELLO.TXT HTTP/1.0\r\n\r\n");
                let ok = status_code(&resp) == Some(200) && body_of(&resp) == FIX_HELLO;
                if ok { w |= 0x02; }
                serial_println!("{} [net15] B GET /fs/K3HELLO.TXT exact bytes => {} ::", PG, if ok { "PASS" } else { "FAIL" });
            }

            // ── C (0x04): GET /fs/../evil is rejected by name validation (404, never a 200/traversal). ──
            {
                let h = open_peer(&mut psock, pface.context(), 49403);
                let resp = fetch_full(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, h,
                    b"GET /fs/../evil HTTP/1.0\r\n\r\n");
                let ok = status_code(&resp) == Some(404);
                if ok { w |= 0x04; }
                serial_println!("{} [net15] C GET /fs/../evil rejected => {} ::", PG, if ok { "404 PASS" } else { "FAIL" });
            }

            // ── D (0x08): GET /fs/MISSING.TXT 404s. ──
            {
                let h = open_peer(&mut psock, pface.context(), 49404);
                let resp = fetch_full(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, h,
                    b"GET /fs/MISSING.TXT HTTP/1.0\r\n\r\n");
                let ok = status_code(&resp) == Some(404);
                if ok { w |= 0x08; }
                serial_println!("{} [net15] D GET /fs/MISSING.TXT => {} ::", PG, if ok { "404 PASS" } else { "FAIL" });
            }

            // ── E (0x10): oversize refusal. Lower the cap to 4 KiB (gate-only override), fetch K3PAT.BIN
            //    (12 KiB > cap): the serve path must refuse 413 without reading it. Reset the override. ──
            {
                FS_CAP_OVERRIDE.store(4096, core::sync::atomic::Ordering::Relaxed);
                let h = open_peer(&mut psock, pface.context(), 49405);
                let resp = fetch_full(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, h,
                    b"GET /fs/K3PAT.BIN HTTP/1.0\r\n\r\n");
                FS_CAP_OVERRIDE.store(0, core::sync::atomic::Ordering::Relaxed);
                let ok = status_code(&resp) == Some(413);
                if ok { w |= 0x10; }
                serial_println!("{} [net15] E GET /fs/K3PAT.BIN (cap 4 KiB) => {} ::", PG, if ok { "413 PASS" } else { "FAIL" });
            }

            // ── Diagnostic (not gated): full multi-block serve of K3PAT.BIN at the real 64 KiB cap —
            //    proves extent-walking a several-block file streams byte-faithful through the TX path. ──
            {
                let h = open_peer(&mut psock, pface.context(), 49406);
                let resp = fetch_full(&mut ks, &mut pface, &mut pdev, &mut psock, &mut clk, h,
                    b"GET /fs/K3PAT.BIN HTTP/1.0\r\n\r\n");
                let body = body_of(&resp);
                let pattern_ok = body.len() == 12288
                    && body.iter().enumerate().all(|(i, &b)| b == ((i * 7 + 3) & 0xFF) as u8);
                serial_println!(
                    "{} [net15] F multi-block K3PAT.BIN => {} ({} bytes) ::",
                    PG, if status_code(&resp) == Some(200) && pattern_ok { "200 pattern-faithful" } else { "MISMATCH" },
                    body.len()
                );
            }

            let pass = w == 0x1f;
            serial_println!(
                ":: NET15-GATE: fs route battery {} [w=0x{:x}] (fs-list|exact-bytes|traversal-reject|404|oversize-413) ::",
                if pass { "PASS" } else { "FAIL" }, w
            );
        }

        // ── PI-NET-16 gate helpers ────────────────────────────────────────────────────────────────────
        /// The injected wall-clock instant the SNTP gate scripts: 2026-07-22T15:30:45Z. `INJ_UNIX` is its
        /// UTC Unix seconds (independently computed: 20656 days to 2026-07-22 × 86400 + 55845 s-of-day) and
        /// `INJ_ISO` the string `render_iso8601` must reproduce from it — the round-trip correctness anchor.
        const INJ_UNIX: u64 = 1_784_734_245;
        const INJ_ISO: &str = "2026-07-22T15:30:45Z";
        /// A black-hole address with no host on the loopback segment (ARP goes unanswered) — drives the
        /// re-sync timeout path deterministically.
        const BLACKHOLE: [u8; 4] = [10, 0, 0, 9];

        /// Build a 48-byte SNTP server reply carrying transmit-timestamp `unix_secs` (converted to the NTP
        /// era-0 epoch), with the given `li`/`vn`/`mode`/`stratum`. The gate uses this to script both
        /// well-formed and adversarial replies against `net16_parse_sntp`.
        fn build_sntp_reply(unix_secs: u64, li: u8, vn: u8, mode: u8, stratum: u8) -> Vec<u8> {
            let mut v: Vec<u8> = Vec::new();
            v.resize(48, 0u8);
            v[0] = ((li & 0x3) << 6) | ((vn & 0x7) << 3) | (mode & 0x7);
            v[1] = stratum;
            let ntp_secs = (unix_secs + NTP_UNIX_DELTA) as u32;
            v[40..44].copy_from_slice(&ntp_secs.to_be_bytes());
            v
        }

        /// PI-NET-16: the SNTP client + wall-clock gate. Bitmask `w`:
        ///   0x01 parser accepts a well-formed reply → the exact injected time (round-trips through the NTP
        ///        epoch AND renders to `INJ_ISO`); 0x02 rejects a short (<48 B) packet; 0x04 surfaces a
        ///        stratum-0 Kiss-o'-Death; 0x08 rejects an LI=3 (alarm/unsynchronized) reply; 0x10 a live
        ///        loopback SNTP exchange sets the wall-clock and the anchored ISO matches `INJ_ISO`; 0x20 a
        ///        re-sync to a black-hole address times out (honest timeout path). PASS = 0x3f.
        pub fn run16() {
            *K_RX.lock() = Some(VecDeque::new());
            *P_RX.lock() = Some(VecDeque::new());
            let mut w: u32 = 0;

            // ── Pure hostile-input parser checks (the security surface in isolation). ──
            let good = build_sntp_reply(INJ_UNIX, 0, 4, 4, 2);
            let parse_ok = match net16_parse_sntp(&good) {
                Sntp::Ok { unix_secs, stratum } => {
                    let mut iso = [0u8; 24];
                    let n = render_iso8601(unix_secs, &mut iso);
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
                "{} [net16t] parse well-formed => {} ::",
                PG, if parse_ok { "resolved+ISO PASS" } else { "FAIL" }
            );

            let short = &good[..40];
            let rej_short = matches!(net16_parse_sntp(short), Sntp::Malformed);
            if rej_short {
                w |= 0x02;
            }
            serial_println!(
                "{} [net16t] reject short packet => {} ::",
                PG, if rej_short { "malformed PASS" } else { "FAIL" }
            );

            let kod = build_sntp_reply(INJ_UNIX, 0, 4, 4, 0);
            let is_kod = matches!(net16_parse_sntp(&kod), Sntp::KissOfDeath);
            if is_kod {
                w |= 0x04;
            }
            serial_println!(
                "{} [net16t] surface KoD (stratum 0) => {} ::",
                PG, if is_kod { "KoD PASS" } else { "FAIL" }
            );

            let alarm = build_sntp_reply(INJ_UNIX, 3, 4, 4, 2);
            let rej_alarm = matches!(net16_parse_sntp(&alarm), Sntp::Malformed);
            if rej_alarm {
                w |= 0x08;
            }
            serial_println!(
                "{} [net16t] reject LI=3 alarm => {} ::",
                PG, if rej_alarm { "malformed PASS" } else { "FAIL" }
            );

            // ── Live loopback SNTP exchange over the REAL service `sntp_step`: kernel = client (KIP), peer
            //    answers on :123 with the injected timestamp. Asserts the wall-clock is anchored and the
            //    anchored ISO is exactly INJ_ISO. ──
            {
                *WALL_CLOCK.lock() = None; // start unsynced so this scenario proves the set
                let mut kdev = SmoltcpPhy::<KernelLoopNic>::new();
                let mut kiface = build_iface(&mut kdev, KMAC, KIP, 0x4b31_3600); // "K16"
                let kstorage: &'static mut [SocketStorage; HTTP_POOL + 2] =
                    Box::leak(Box::new(Default::default()));
                let mut ksockets = SocketSet::new(&mut kstorage[..]);
                let (http, mdns, sntp, _l, _b, _j) = build_net_sockets(&mut kiface, &mut ksockets);
                let mut ks = NetService {
                    iface: kiface,
                    dev: kdev,
                    sockets: ksockets,
                    http,
                    req_seen: [false; HTTP_POOL],
                    active_since: [0; HTTP_POOL],
                    mdns,
                    sntp,
                    // Seed the peer (PIP) as the time source and make the first re-sync due immediately.
                    sntp_server: Some(PIP),
                    sntp_state: SntpState::Idle { due_ms: 0 },
                    ip: KIP,
                    req_buf: [[0u8; REQ_CAP]; HTTP_POOL],
                    req_len: [0; HTTP_POOL],
                    resp: core::array::from_fn(|_| None),
                };

                let mut pdev = SmoltcpPhy::<PeerLoopNic>::new();
                let mut pface = build_iface(&mut pdev, PMAC, PIP, 0x5031_3600); // "P16"
                let pstorage: &'static mut [SocketStorage; 4] = Box::leak(Box::new(Default::default()));
                let mut psock = SocketSet::new(&mut pstorage[..]);
                // Peer NTP server socket bound to :123.
                let prx: &'static mut [udp::PacketMetadata; 4] =
                    Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4]));
                let prxp: &'static mut [u8; 256] = Box::leak(Box::new([0u8; 256]));
                let ptx: &'static mut [udp::PacketMetadata; 4] =
                    Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4]));
                let ptxp: &'static mut [u8; 256] = Box::leak(Box::new([0u8; 256]));
                let mut pu = udp::Socket::new(
                    udp::PacketBuffer::new(&mut prx[..], &mut prxp[..]),
                    udp::PacketBuffer::new(&mut ptx[..], &mut ptxp[..]),
                );
                let _ = pu.bind(NTP_PORT);
                let ph = psock.add(pu);

                let mut clk: i64 = 0;
                for _ in 0..160 {
                    let t = Instant::from_millis(clk);
                    ks.iface.poll(t, &mut ks.dev, &mut ks.sockets);
                    ks.sntp_step(clk);
                    pface.poll(t, &mut pdev, &mut psock);
                    let ps = psock.get_mut::<udp::Socket>(ph);
                    if ps.can_recv() {
                        let mut qb = [0u8; 128];
                        if let Ok((_n, meta)) = ps.recv_slice(&mut qb) {
                            let reply = build_sntp_reply(INJ_UNIX, 0, 4, 4, 2);
                            let _ = ps.send_slice(&reply, meta.endpoint);
                        }
                    }
                    clk += STEP_MS;
                    if wall_anchor().is_some() {
                        // Let the step settle back to Idle; the anchor is set.
                        break;
                    }
                }
                let live_ok = match wall_anchor() {
                    Some((anchor_unix, stratum)) => {
                        let mut iso = [0u8; 24];
                        let n = render_iso8601(anchor_unix, &mut iso);
                        stratum == 2 && core::str::from_utf8(&iso[..n]) == Ok(INJ_ISO)
                    }
                    None => false,
                };
                if live_ok {
                    w |= 0x10;
                }
                serial_println!(
                    "{} [net16] loopback sntp sets clock => {} ::",
                    PG, if live_ok { "2026-07-22T15:30:45Z PASS" } else { "FAIL" }
                );

                // ── Timeout path: re-target the SAME service at a black-hole address, make a re-sync due,
                //    and pump well past the window — the reply never comes, so `sntp_step` must take the
                //    timeout branch (NET16_TIMEOUTS bumps) and fall back to Idle. ──
                let to_before = NET16_TIMEOUTS.load(core::sync::atomic::Ordering::Relaxed);
                ks.sntp_server = Some(BLACKHOLE);
                ks.sntp_state = SntpState::Idle { due_ms: clk };
                for _ in 0..200 {
                    let t = Instant::from_millis(clk);
                    ks.iface.poll(t, &mut ks.dev, &mut ks.sockets);
                    ks.sntp_step(clk);
                    pface.poll(t, &mut pdev, &mut psock);
                    clk += STEP_MS;
                }
                let timed_out =
                    NET16_TIMEOUTS.load(core::sync::atomic::Ordering::Relaxed) > to_before;
                if timed_out {
                    w |= 0x20;
                }
                serial_println!(
                    "{} [net16] resync black-hole => {} ::",
                    PG, if timed_out { "timeout PASS" } else { "FAIL" }
                );
            }

            let pass = w == 0x3f;
            serial_println!(
                ":: NET16-GATE: sntp client battery {} [w=0x{:x}] (parse-ok+iso|reject-short|kod|reject-alarm|live-set-clock|resync-timeout) ::",
                if pass { "PASS" } else { "FAIL" }, w
            );
        }

        /// Send a single mDNS A query for `unaos.local` from a peer UDP socket to 224.0.0.251:5353 and
        /// check `mdns_step` answered (the responder's `net11_answered` counter moved). Diagnostic only.
        fn mdns_probe(
            ks: &mut NetService<SmoltcpPhy<KernelLoopNic>>,
            pface: &mut Interface,
            pdev: &mut SmoltcpPhy<PeerLoopNic>,
            psock: &mut SocketSet<'static>,
            clk: &mut i64,
        ) -> bool {
            use smoltcp::socket::udp;
            let rx_meta: &'static mut [udp::PacketMetadata; 4] =
                Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4]));
            let rx_pl: &'static mut [u8; 512] = Box::leak(Box::new([0u8; 512]));
            let tx_meta: &'static mut [udp::PacketMetadata; 4] =
                Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4]));
            let tx_pl: &'static mut [u8; 512] = Box::leak(Box::new([0u8; 512]));
            let mut us = udp::Socket::new(
                udp::PacketBuffer::new(&mut rx_meta[..], &mut rx_pl[..]),
                udp::PacketBuffer::new(&mut tx_meta[..], &mut tx_pl[..]),
            );
            if us.bind(5353u16).is_err() {
                return false;
            }
            let uh = psock.add(us);
            // Peer must accept the multicast group the query targets so the responder's multicast answer
            // is delivered back (the responder replies to 224.0.0.251:5353 for a non-QU query).
            let _ = pface.join_multicast_group(IpAddress::v4(224, 0, 0, 251));

            // A minimal DNS query: 1 question, QNAME "unaos"."local", QTYPE A, QCLASS IN.
            let query: [u8; 12 + 13 + 4] = {
                let mut q = [0u8; 29];
                q[0..2].copy_from_slice(&0u16.to_be_bytes()); // ID
                q[2..4].copy_from_slice(&0u16.to_be_bytes()); // flags: standard query
                q[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
                let mut o = 12;
                q[o] = 5;
                o += 1;
                q[o..o + 5].copy_from_slice(b"unaos");
                o += 5;
                q[o] = 5;
                o += 1;
                q[o..o + 5].copy_from_slice(b"local");
                o += 5;
                q[o] = 0;
                o += 1;
                q[o..o + 2].copy_from_slice(&1u16.to_be_bytes()); // QTYPE A
                o += 2;
                q[o..o + 2].copy_from_slice(&1u16.to_be_bytes()); // QCLASS IN
                q
            };
            let before = net11_answered();
            let dst = IpEndpoint::new(IpAddress::v4(224, 0, 0, 251), 5353);
            let _ = psock.get_mut::<udp::Socket>(uh).send_slice(&query, dst);
            pump(ks, pface, pdev, psock, clk, 20);
            net11_answered() != before
        }
    }
}
