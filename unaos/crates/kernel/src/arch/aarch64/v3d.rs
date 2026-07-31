// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PI-V3D-1 — VideoCore VI (V3D 4.2) GPU foundation on the Raspberry Pi 4 (BCM2711).
//
// The first GPU silicon UnaOS touches. This is deliberately NOT a triangle: it proves the full
// non-graphics chain — firmware power domain, clock, MMIO register access, the V3D-private MMU,
// a control-list fetch, and a tile store — with the smallest job that exercises all of it: the GPU
// CLEARS a buffer to a known colour and the CPU verifies the bytes. A triangle (binner control list
// + shader record) is the explicit NEXT arc; nothing here starts it.
//
// ── ATTENDED-METAL-UNVERIFIED ──────────────────────────────────────────────────────────────────
// QEMU's `raspi4b` machine does NOT model V3D. Everything past the probe's absence check
// (`ident_looks_live`) therefore runs ONLY on real silicon at an attended sitting; in QEMU the
// probe detects the absent block, prints a graceful-degradation line, and returns. So M2/M3 below
// are correct-by-construction against the Linux/Mesa references cited inline, not QEMU-exercised.
// Do NOT treat "no V3D in QEMU" as a divergence.
//
// References of record (fold the key URLs into arch_arm64.md §PI-V3D):
//   * Register layout: Linux `drivers/gpu/drm/v3d/v3d_regs.h` (hub + core + MMU offsets, field bits).
//   * V3D MMU: Linux `drivers/gpu/drm/v3d/v3d_mmu.c` (flat page table, PTE bits, flush sequence).
//   * Render-control-list packets: Mesa `src/broadcom/cle/v3d_packet_v33.xml` (4.2 encodings — the
//     VC4-era packet numbers/sizes do NOT transfer).
//   * Structure reference: librerpi/lk-overlay `v3d.c`.
//
// Coherency: V3D is NOT coherent with the Cortex-A72 data cache. Every buffer the CPU writes for the
// GPU is `cache::clean_range`d before the kick; every buffer the GPU writes for the CPU is
// `cache::clean_invalidate_range`d before the readback (see the M4 cache-maintenance audit in the
// doc). No-ops in QEMU, load-bearing on metal.
//
// Memory-safety invariant (the arc's review lens): the V3D can only reach RAM through PTEs we mark
// VALID in its page table. We map ONLY the arena's own pages (identity: iova == phys), leaving every
// other PTE invalid — so a control list that referenced any address outside the arena would fault in
// the V3D MMU rather than scribble kernel RAM. Every V3D-visible address written into a control list
// is bounds-checked to lie inside the arena before the kick.

use super::cache;
use super::mailbox;
use core::sync::atomic::{AtomicBool, Ordering};

// ─── MMIO bases (ARM physical; Device-nGnRnE-mapped by boot.rs L1[3], the 0xC000_0000–0xFFFF_FFFF
// GiB block — same window as the mailbox/PL011/GIC, so no new MMU mapping is needed). ───
const V3D_HUB_BASE: usize = 0xFEC0_0000; // ARM PA of the V3D hub (VC bus 0x7EC0_0000)
const V3D_CORE0_BASE: usize = V3D_HUB_BASE + 0x4000; // core 0 register block

// ─── PI-V3D-3: the PM / ASB (AXI async bridge) enable step. ───────────────────────────────────────
// PI-V3D-2's metal verdict (2026-07-18, non-relitigable): firmware power domain 10 ACKed ON, clock id
// 5 rate 500 MHz ACKed, clock GATE ACKed active — yet the V3D hub STILL reads 0xdeadbeef (BUS-POISON,
// probe fail-closed correctly). Conclusion of record: the RPi firmware property-tag power+clock path
// is NOT sufficient to decode the V3D block on BCM2711.
//
// Adjudication (Linux `drivers/soc/bcm/bcm2835-power.c` + `arch/arm/boot/dts/bcm2711.dtsi`, rpi-6.1.y):
// on BCM2711 the V3D power domain (`BCM2835_POWER_DOMAIN_GRAFX_V3D`) is brought up by
// `bcm2835_asb_power_on(PM_GRAFX, ASB_V3D_M_CTRL, ASB_V3D_S_CTRL, PM_V3DRSTN)`. Two of its steps are
// DISTINCT from the firmware power-domain path and are the missing piece:
//   (1) deassert the V3D reset — set PM_V3DRSTN (bit 6) in PM_GRAFX, written with the PM password.
//   (2) release the two async AXI bridges — clear ASB_REQ_STOP in ASB_V3D_M_CTRL then ASB_V3D_S_CTRL
//       (each written with the PM password) and wait for ASB_ACK to clear.
// The V3D ASB registers live in the `rpivid_asb` reg block, NOT the legacy `asb` block: in the DT the
// `pm` node's third reg range is `<0x7ec11000 0x20>` "rpivid_asb", and `bcm2835_asb_control` routes
// ASB_V3D_{S,M}_CTRL to `power->rpivid_asb` when present (always, on BCM2711). The PM_POWUP/inrush/
// memory-repair sequence (`bcm2835_power_power_on`) is SKIPPED on BCM2711 (`if (power->rpivid_asb)
// return 0`) — the firmware already did it, which is why our mailbox SET_DOMAIN_STATE domain 10 ACKs.
// So we KEEP the firmware power/rate/gate steps (ACKed-working, still necessary) and ADD only the
// reset-deassert + ASB-release step, sequenced after them.
//
// Both bases are ARM PAs inside the 0xC000_0000–0xFFFF_FFFF Device-nGnRnE window already mapped by
// boot.rs L1[3] — no new MMU mapping. QEMU raspi4b models neither the rpivid_asb block nor V3D, so
// every read/write here is poison/absent-tolerant and every wait is a finite CNTPCT backstop: on QEMU
// the ASB regs are unbacked (read 0, ACK already clear → no wait, no fault), and the IDENT0 probe that
// follows still lands on the honest BLOCK-DOWN. On metal the discriminating expectation becomes
// BLOCK-UP.
const PM_BASE: usize = 0xFE10_0000; // ARM PA of the PM block (VC bus 0x7E10_0000, DT "pm")
const PM_GRAFX: usize = 0x010C; // graphics power-domain control register
const PM_V3DRSTN: u32 = 1 << 6; // deassert = V3D out of reset (bcm2835-power PM_V3DRSTN)
const PM_PASSWORD: u32 = 0x5A00_0000; // every PM (and ASB) write must carry this in the top byte

const RPIVID_ASB_BASE: usize = 0xFEC1_1000; // ARM PA of the rpivid_asb block (VC bus 0x7EC1_1000)
const ASB_V3D_S_CTRL: usize = 0x08; // V3D slave AXI bridge control
const ASB_V3D_M_CTRL: usize = 0x0C; // V3D master AXI bridge control
const ASB_REQ_STOP: u32 = 1 << 0; // request the bridge stopped (clear to release)
const ASB_ACK: u32 = 1 << 1; // bridge stopped acknowledge (clears when released)

// ─── PI-V3D-69: the rest of the ASB register file, and the second (legacy) ASB block. ─────────────
// `bcm2835-power.c` names four bits in an ASB control word — `ASB_REQ_STOP` (0), `ASB_ACK` (1),
// `ASB_EMPTY` (2), `ASB_FULL` (3) — and only ever READS/WRITES the first two. The other two describe
// the bridge's transaction FIFO and are exactly the never-looked-at state this arc opens: a bridge that
// is released (REQ_STOP=0, ACK=0) but whose FIFO never leaves EMPTY across a leg saw no traffic.
const ASB_EMPTY: u32 = 1 << 2; // bridge transaction FIFO empty
const ASB_FULL: u32 = 1 << 3; // bridge transaction FIFO full
// PI-V3D-70 — the ASB identity register, settled against the SHIPPING DTB rather than reconstructed.
// `/soc/watchdog@7e100000` (compatible "brcm,bcm2711-pm") carries THREE reg ranges with reg-names
// "pm", "asb", "rpivid_asb":
//     <0x7e100000 0x114>   pm
//     <0x7e00a000 0x024>   asb          (the LEGACY block)
//     <0x7ec11000 0x020>   rpivid_asb   (the block bcm2835_asb_control routes V3D to on BCM2711)
// Both bases this driver already uses are therefore DT-CORRECT (VC 0x7e00a000 → ARM 0xFE00A000,
// VC 0x7ec11000 → ARM 0xFEC11000) — the base is no longer an open question.
//
// The LENGTHS are the load-bearing part. Mainline's `bcm2835-power.c` reads its identity word,
// `ASB_AXI_BRDG_ID`, at offset **0x20** and expects the ASCII tag 'brdg'. That offset is INSIDE the
// legacy range (len 0x24) and OUTSIDE the rpivid range (len 0x20). So:
//   * the legacy block HAS an in-range identity register and it is the only identity check available;
//   * the rpivid block has NO identity register in its DT range at all.
// `[v3d69]`'s "BRDG_VERSION" column read offset 0x00 of the rpivid window and called it an identity
// word. Offset 0x00 is not an identity register in any range, so the 0x00000000 that column reported
// was NEVER evidence of a dead window — it was a read of a non-identity offset. This arc retires that
// column and reads the real one.
const ASB_AXI_BRDG_ID: usize = 0x20;
/// ASCII 'b','r','d','g' as a little-endian word — the tag `bcm2835-power`'s ASB probe expects.
const ASB_BRDG_ID_EXPECT: u32 = 0x6272_6467;
// ARM PA = VC bus address with the 0x7exxxxxx → 0xFExxxxxx fixup; both blocks land in the same
// Device-nGnRnE GiB boot.rs L1[3] already maps, so neither read opens a new MMU mapping.
const LEGACY_ASB_BASE: usize = 0xFE00_A000;

// PM_GRAFX field map from `bcm2835-power.c`'s generic power-domain bits. Only `PM_V3DRSTN` (bit 6,
// above) is corroborated in this tree — this driver writes it and metal echoes it. The rest of this map
// is RECONSTRUCTED FROM MEMORY of the mainline header and is UNCERTAIN; the `[v3d69]` witness therefore
// prints the RAW word first and gives every decoded field below the caveat it deserves. In particular
// `bcm2835_power_power_on` (the POWUP → POWOK → ISPOW → MEMREP/MRDONE → ISFUNC ramp these bits belong
// to) returns EARLY on BCM2711 — the firmware owns that sequence — so mainline never writes them either
// and the expectation is "whatever start4.elf left", not "what Linux programmed".
const PM_POWUP: u32 = 1 << 3;
const PM_POWOK: u32 = 1 << 4;
const PM_ISFUNC: u32 = 1 << 5;
const PM_MRDONE: u32 = 1 << 7;
const PM_MEMREP: u32 = 1 << 8;
const PM_ISPOW: u32 = 1 << 9;
const PM_ENAB: u32 = 1 << 12;

// ─── Hub registers (offset from V3D_HUB_BASE), per v3d_regs.h. ───
const V3D_HUB_IDENT0: usize = 0x0008;
const V3D_HUB_IDENT1: usize = 0x000C;
const V3D_HUB_IDENT2: usize = 0x0010;
const V3D_HUB_IDENT3: usize = 0x0014;

// PI-V3D-61: the HUB_IDENT1 field map, corrected. Up to V3D-60 this file read the technology version
// as the TOP BYTE of HUB_IDENT1 and compared it against the literal `0x42` — both halves of that were
// wrong, and the metal reading (HUB_IDENT1=0x000e1124 -> "raw=0x00, expects 0x42") was a decode
// artifact, not a silicon mismatch. The mainline driver's field map for this register puts the
// identity in the LOW half-word, in four-bit fields, and derives its single version NUMBER as
// `tver * 10 + rev` — a DECIMAL composition, so "V3D 4.2" is the number **42**, never the hex byte
// 0x42. Field positions restated in our own words from `v3d_regs.h`'s HUB_IDENT1 masks; the shifts
// below are corroborated by the bench word itself (see v3d.md §35): 0x000e1124 decodes to tver=4,
// rev=2 (=> 4.2, the generation every packet encoding in this file targets), ncores=1, nhosts=1, and
// the three feature bits TFU/TSY/MSO set with L3C clear — a coherent single-core 4.2 configuration,
// where the old decode produced the incoherent "version 0x00, 4 cores".
const V3D_HUB_IDENT1_TVER_SHIFT: u32 = 0; // bits 3:0 — technology version, MAJOR
const V3D_HUB_IDENT1_REV_SHIFT: u32 = 4; // bits 7:4 — technology revision, MINOR
const V3D_HUB_IDENT1_NCORES_SHIFT: u32 = 8; // bits 11:8 — number of V3D cores behind this hub
const V3D_HUB_IDENT1_NHOSTS_SHIFT: u32 = 12; // bits 15:12 — number of host interfaces
const V3D_HUB_IDENT1_NIB_MASK: u32 = 0xF; // every field above is one nibble wide
const V3D_HUB_IDENT1_WITH_L3C: u32 = 1 << 16;
const V3D_HUB_IDENT1_WITH_TFU: u32 = 1 << 17;
const V3D_HUB_IDENT1_WITH_TSY: u32 = 1 << 18;
const V3D_HUB_IDENT1_WITH_MSO: u32 = 1 << 19;
/// The generation this driver's CL packing, QPU word encoding and register map were audited against,
/// in the mainline `tver * 10 + rev` composition: V3D 4.2 == 42 (decimal).
const V3D_VERSION_EXPECTED: u32 = 42;
// HUB_IDENT2 bit 8 — the hub reports whether an MMU is present. The only field of that word this file
// has a corroborated position for; the rest stays raw.
const V3D_HUB_IDENT2_WITH_MMU: u32 = 1 << 8;
// CORE IDENT0: the low three bytes are the ASCII signature 'V','3','D' (0x00443356 as a word),
// and the top byte carries the core's MAJOR version — a second, independent witness to the hub's
// TVER field. The signature check is what makes "these words came from a live V3D" falsifiable.
const V3D_CTL_IDENT0_VER_SHIFT: u32 = 24;
const V3D_CTL_IDENT0_SIG: u32 = 0x00443356; // 'V'=0x56, '3'=0x33, 'D'=0x44
// CORE IDENT1 bits 3:0 — the core's own revision nibble; must agree with the hub's REV.
const V3D_CTL_IDENT1_REV_MASK: u32 = 0xF;

/// Decode HUB_IDENT1 / CORE IDENT0 / CORE IDENT1 into the mainline version NUMBER
/// (`tver * 10 + rev`), the core count, and the two corroborating witnesses. Pure function, no MMIO.
///
/// `ncores` is returned RAW. The pre-V3D-61 code clamped it with `.max(1)` — a defensible hedge when
/// the field being read was the wrong one, but under the corrected map an `NCORES` of 0 is a genuine
/// anomaly (a hub reporting no cores behind it) and laundering it into a plausible 1 would hide exactly
/// the class of thing this arc exists to stop hiding.
fn v3d_ident_version(hub1: u32, c0: u32, c1: u32) -> (u32, u32, u32, u32, bool, bool) {
    let tver = (hub1 >> V3D_HUB_IDENT1_TVER_SHIFT) & V3D_HUB_IDENT1_NIB_MASK;
    let rev = (hub1 >> V3D_HUB_IDENT1_REV_SHIFT) & V3D_HUB_IDENT1_NIB_MASK;
    let ncores = (hub1 >> V3D_HUB_IDENT1_NCORES_SHIFT) & V3D_HUB_IDENT1_NIB_MASK;
    let ver = tver * 10 + rev;
    let sig_ok = (c0 & 0x00FF_FFFF) == V3D_CTL_IDENT0_SIG;
    let core_ver_ok =
        ((c0 >> V3D_CTL_IDENT0_VER_SHIFT) & 0xFF) == tver && (c1 & V3D_CTL_IDENT1_REV_MASK) == rev;
    (ver, tver, rev, ncores, sig_ok, core_ver_ok)
}

// PI-V3D-52 (Rung 1): the HUB interrupt MASK block. The kernel's `v3d_irq_enable`
// (`drivers/gpu/drm/v3d/v3d_irq.c`) is FOUR register writes, not two: after the per-core
// INT_MSK_SET/CLR (mirrored in `v3d_irq_enable` below since V3D-49) it ALSO unmasks the HUB
// interrupt working set once at probe — `V3D_HUB_INT_MSK_SET = ~V3D_HUB_IRQS`,
// `V3D_HUB_INT_MSK_CLR = V3D_HUB_IRQS`. UnaOS had never touched the hub mask; the hub-INT half of
// the kernel probe was the last unmirrored byte-exact bring-up divergence (v3d.md §26). Offsets are
// the SAME low slots as the core INT block (0x50–0x64) but relative to V3D_HUB_BASE — a DISTINCT
// MMIO block from the core registers. Transcribed from Linux `v3d_regs.h` (register facts only,
// GPL-2.0-only header): the hub IDENT0..3 at 0x08..0x14 already in this file confirm the layout, and
// the INT block follows the TFU region at 0x50. (1 = that hub interrupt is MASKED/disabled.)
// PI-V3D-62: the hub interrupt STATUS/CLEAR slots, needed to witness the MMU fault interrupts this arc
// arms. Same audited source and same layout family as the mask trio below (v3d_regs.h): the block runs
// STS 0x50, SET 0x54, CLR 0x58, MSK_STS 0x5c, MSK_SET 0x60, MSK_CLR 0x64. STS is the RAW latched vector
// (the mask gates CPU delivery, not the latch), and CLR is write-1-to-clear — the same read-then-echo
// idiom `clear_mmu_fault_latch` already uses on V3D_MMU_CTL.
const V3D_HUB_INT_STS: usize = 0x0050; // latched hub interrupt status (V3D_HUB_INT_STS)
const V3D_HUB_INT_CLR: usize = 0x0058; // write-1-to-CLEAR a latched hub interrupt (V3D_HUB_INT_CLR)
const V3D_HUB_INT_MSK_STS: usize = 0x005c; // current hub interrupt mask (V3D_HUB_INT_MSK_STS)
const V3D_HUB_INT_MSK_SET: usize = 0x0060; // write-1-to-MASK (disable) a hub interrupt (V3D_HUB_INT_MSK_SET)
const V3D_HUB_INT_MSK_CLR: usize = 0x0064; // write-1-to-UNMASK (enable) a hub interrupt (V3D_HUB_INT_MSK_CLR)
// Hub interrupt bit layout (v3d_regs.h, ver<71 block):
const V3D_HUB_INT_TFUC: u32 = 1 << 1; // TFU (texture-format-utility) conversion complete
const V3D_HUB_INT_MMU_CAP: u32 = 1 << 3; // hub-MMU address-cap exceeded
const V3D_HUB_INT_MMU_PTI: u32 = 1 << 4; // hub-MMU page-table invalid
const V3D_HUB_INT_MMU_WRV: u32 = 1 << 5; // hub-MMU write violation
// The kernel's hub working set for V3D 4.2 (`V3D_HUB_IRQS`, v3d_irq.c, ver<71 path):
// MMU_WRV | MMU_PTI | MMU_CAP | TFUC = 0x3a. This is the exact bitset `v3d_irq_enable` UNMASKS on the
// hub (MSK_CLR) and everything else it MASKS (MSK_SET = ~this).
const V3D_HUB_IRQS: u32 = V3D_HUB_INT_MMU_WRV | V3D_HUB_INT_MMU_PTI | V3D_HUB_INT_MMU_CAP | V3D_HUB_INT_TFUC;

// V3D MMU (in the hub), per v3d_regs.h / v3d_mmu.c. The register OFFSETS and the V3D_MMU_CTL BIT
// FIELDS below are transcribed verbatim from Linux `drivers/gpu/drm/v3d/v3d_regs.h` (torvalds/linux
// master). PI-V3D-4 root cause: the earlier constants here were fabricated. V3D_MMU_VIO_ADDR/
// DEBUG_INFO pointed at V3D_MMU_HIT (0x1208) / VIO_ADDR (0x1234) instead of the real slots, and —
// fatally — the CTL bit fields were invented at the *top* of the word (ENABLE=1<<31 …). The real
// ENABLE is BIT(0): the enable write therefore set only reserved bits, so the MMU never enabled and
// the readback (undefined/reserved bits do not latch) came back 0x00000000 — precisely the "M2 MMU
// program writes read back zero" metal symptom (R22 sitting-2). Correct layout:
//   0x1204 PT_PA_BASE · 0x1208 HIT · 0x120c MISSES · 0x1210 STALLS · 0x1214 ADDR_CAP ·
//   0x122c VIO_ID · 0x1230 ILLEGAL_ADDR · 0x1234 VIO_ADDR · 0x1238 DEBUG_INFO.
const V3D_MMUC_CONTROL: usize = 0x1000;
const V3D_MMU_CTL: usize = 0x1200;
const V3D_MMU_PT_PA_BASE: usize = 0x1204;
const V3D_MMU_VIO_ID: usize = 0x122c; // PI-V3D-5 fault-witness: id of the client that violated
const V3D_MMU_ILLEGAL_ADDR: usize = 0x1230;
const V3D_MMU_VIO_ADDR: usize = 0x1234;
const V3D_MMU_DEBUG_INFO: usize = 0x1238;

const V3D_MMUC_CONTROL_ENABLE: u32 = 1 << 0;
const V3D_MMUC_CONTROL_FLUSH: u32 = 1 << 1;

const V3D_MMU_CTL_ENABLE: u32 = 1 << 0;
const V3D_MMU_CTL_PT_INVALID_ENABLE: u32 = 1 << 16;
const V3D_MMU_CTL_PT_INVALID_ABORT: u32 = 1 << 19;
const V3D_MMU_CTL_WRITE_VIOLATION_ABORT: u32 = 1 << 11;
// PI-V3D-62: the INTERRUPT companions of the abort bits, plus the address-cap pair. `[v3d60] initdelta`
// named these as a STANDING gap and deliberately refused to guess their positions; they are now read off
// the same audited source as every other constant in this block (Linux `drivers/gpu/drm/v3d/v3d_regs.h`,
// torvalds/linux master — register/hardware facts only, no GPL-2.0-only code reproduced). The
// enable/abort/int/exception quartet for each condition sits in a descending run:
//   CAP_EXCEEDED 27 / _ABORT 26 / _INT 25 / _EXCEPTION 24
//   PT_INVALID   20 / _ABORT 19 / _INT 18 / _EXCEPTION 17 / _ENABLE 16
//   WRITE_VIOLATION 12 / _ABORT 11 / _INT 10 / _EXCEPTION 9
// `v3d_mmu_set_page_table` writes ENABLE | PT_INVALID_{ENABLE,ABORT,INT} | WRITE_VIOLATION_{ABORT,INT} |
// CAP_EXCEEDED_{ABORT,INT} | TLB_CLEAR. The _EXCEPTION halves are NOT in mainline's set and are not
// mirrored here. Arming the _INT halves is what makes a refused write REPORT instead of vanish.
const V3D_MMU_CTL_PT_INVALID_INT: u32 = 1 << 18;
const V3D_MMU_CTL_WRITE_VIOLATION_INT: u32 = 1 << 10;
const V3D_MMU_CTL_CAP_EXCEEDED_ABORT: u32 = 1 << 26;
const V3D_MMU_CTL_CAP_EXCEEDED_INT: u32 = 1 << 25;
const V3D_MMU_CTL_TLB_CLEAR: u32 = 1 << 2;
const V3D_MMU_CTL_TLB_CLEARING: u32 = 1 << 7;
const V3D_MMU_ILLEGAL_ADDR_ENABLE: u32 = 1 << 31;
// PI-V3D-5 MMU fault-status bits (v3d_regs.h, read side of V3D_MMU_CTL — set by hardware when a
// translation faults). Used only to WITNESS a job-store fault; they change no programmed value.
const V3D_MMU_CTL_PT_INVALID: u32 = 1 << 20; // an access hit an invalid PTE
const V3D_MMU_CTL_WRITE_VIOLATION: u32 = 1 << 12; // a write hit a non-writeable page
const V3D_MMU_CTL_CAP_EXCEEDED: u32 = 1 << 27; // an access exceeded the page-table address cap

// V3D MMU PTE bits (v3d_mmu.c). The page-number field is phys >> 12.
const V3D_MMU_PAGE_SHIFT: u32 = 12;
const V3D_PTE_VALID: u32 = 1 << 28;
const V3D_PTE_WRITEABLE: u32 = 1 << 29;

// ─── Core 0 registers (offset from V3D_CORE0_BASE), per v3d_regs.h. ───
const V3D_CTL_IDENT0: usize = 0x0000;
const V3D_CTL_IDENT1: usize = 0x0004;
const V3D_CTL_IDENT2: usize = 0x0008;

// ─── PI-V3D-12: GPU-side cache maintenance (core 0). Offsets + bits transcribed VERBATIM from Linux
// `drivers/gpu/drm/v3d/v3d_regs.h` (register/hardware facts; GPL-2.0-only header, facts-only — same
// discipline as the CT-queue and MMU constants above). Linux `v3d_gem.c::v3d_invalidate_caches` runs
// before EVERY job (both `v3d_bin_job_run` and `v3d_render_job_run` call it, per v3d_sched.c): on
// V3D >= 4.1 the live steps are the L2T flush (L2TCACTL: L2TFLS with FLM=FLUSH) and the slice-cache
// invalidate (SLCACTL: all-0xF TVCCS/TDCCS/UCC/ICC); the GCA/L3 step is ver<41-only and the L2C
// invalidate ver<33-only — both no-ops on the Pi 4's 4.2, so neither is transcribed here.
const V3D_CTL_SLCACTL: usize = 0x0024; // slice-cache control (TMU-vertex/TMU-data/uniform/instruction)
const V3D_CTL_L2TCACTL: usize = 0x0030; // L2T cache control
const V3D_L2TCACTL_L2TFLS: u32 = 1 << 0; // flush start; reads 1 while the flush is in progress
const V3D_L2TCACTL_FLM_FLUSH: u32 = 0 << 1; // FLM field [2:1] = FLUSH (write-back + invalidate)
// PI-V3D-53: FLM=CLEAR (invalidate-only, mode 1) — the mode the kernel's per-job INPUT invalidate uses.
// `v3d_invalidate_l2t` (v3d_gem.c, GPL-2.0-only, facts-only) writes L2TFLSTA=0/L2TFLEND=~0 then
// `L2TCACTL = L2TFLS | FLM=CLEAR` — NOT FLM=FLUSH. FLM values (v3d_regs.h): 0=FLUSH(wb+inv),
// 1=CLEAR(inv-only), 2=CLEAN(wb-only).
const V3D_L2TCACTL_FLM_CLEAR: u32 = 1 << 1; // FLM field [2:1] = CLEAR (invalidate-only)
// PI-V3D-51: the L2T flush ADDRESS-RANGE bounds the L2TCACTL flush above walks. Offsets transcribed from
// Linux `v3d_regs.h` (register facts; GPL-2.0-only header, facts-only): V3D_CTL_L2TFLSTA=0x034,
// V3D_CTL_L2TFLEND=0x038. `v3d_init_core` writes STA=0 and END=~0 (flush the WHOLE address space) for
// EVERY V3D version — it is NOT the ver<41 MISCCFG branch, it runs unconditionally — and `v3d_init_hw_state`
// is called at the tail of EVERY `v3d_reset_v3d`. UnaOS's V3D-50 OFF→ON power-cycle returns these to their
// power-on-reset value and nothing re-established them, so the per-kick L2TCACTL FLM=FLUSH in
// `invalidate_gpu_caches` walked an unestablished (POR) range — the exact core-init step §24's audit table
// missed (it accounted only for the ver<40 MISCCFG write, not the unconditional L2TFL* pair).
const V3D_CTL_L2TFLSTA: usize = 0x0034; // L2T flush start address (v3d_regs.h)
const V3D_CTL_L2TFLEND: usize = 0x0038; // L2T flush end address   (v3d_regs.h)
const V3D_SLCACTL_INVALIDATE_ALL: u32 = (0xF << 24) | (0xF << 16) | (0xF << 8) | 0xF;

// ─── PI-V3D-34: TMU/GMP block-state witness constants. Offsets + bits transcribed VERBATIM from Linux
// `drivers/gpu/drm/v3d/v3d_regs.h` (register/hardware facts; GPL-2.0-only header, facts-only). These
// are the configuration/enable-state registers a v42 TMU general store depends on — dumped read-only so
// the next boot's SAW-NOTHING branch (TMU-issue PCTR battery all zero) can decide block-wide-disabled
// vs probe-specific. NONE of these are written anywhere in UnaOS bring-up (see the witness annotations).
//
// MISCCFG (core CTL): OVRTMUOUT overrides the TMU output type to come from the sampler-uniform config
// word instead of the hardware default. Linux `v3d_init_core` writes MISCCFG=OVRTMUOUT ONLY on ver<41
// (`if (v3d->ver < V3D_GEN_41)`); the Pi 4's V3D 4.2 (ver 42) is >= 41, so the KMD leaves MISCCFG at its
// reset value and the TMU output type is taken from the config word — so OURS must match (untouched).
const V3D_CTL_MISCCFG: usize = 0x0018;
const V3D_MISCCFG_OVRTMUOUT: u32 = 1 << 0; // override TMU output type from the sampler-config word
const V3D_CTL_MISCCFG_QRMAXCNT_MASK: u32 = 0x7 << 1; // [3:1] queued-request max count
// L2TCACTL.TMUWCF = TMU write-combiner flush — the TMU-store-drain-specific bit of the L2T control we
// already drive for FLM=FLUSH. Dumped to show whether the TMU write combiner is being flushed alongside.
const V3D_L2TCACTL_TMUWCF: u32 = 1 << 8;
// GMP (Graphics Memory Protection), per-core block at core+0x800 (ver<71 layout; ver42 uses this base).
// A GMP write-violation drops the store SILENTLY with no MMU fault on some v3d revisions — the exact
// signature of "store accepted but never lands, MMU fault latch clean". Linux NEVER writes any GMP
// register in init or submit (v3d_gem.c reads GMP only in v3d_idle_axi); GMP therefore sits in its reset
// state, where CFG.PROT_ENABLE=0 = protection DISABLED = all accesses allowed. So the reset default is
// allow-all, NOT default-deny — but only the silicon read-back proves the latched state, hence this dump.
const V3D_GMP_STATUS: usize = 0x0800;
const V3D_GMP_CFG: usize = 0x0804;
const V3D_GMP_VIO_ADDR: usize = 0x0808;
const V3D_GMP_VIO_TYPE: usize = 0x080c;
const V3D_GMP_TABLE_ADDR: usize = 0x0810;
const V3D_GMP_VALID_LINES: usize = 0x0820;
const V3D_GMP_CFG_PROT_ENABLE: u32 = 1 << 0; // protection enable — 0 at reset = allow all accesses
const V3D_GMP_CFG_STOP_REQ: u32 = 1 << 1;
const V3D_GMP_CFG_LBURSTEN: u32 = 1 << 3;
const V3D_GMP_STATUS_VIO: u32 = 1 << 0; // a protection violation was latched
const V3D_GMP_STATUS_INVPROT: u32 = 1 << 1; // an access hit an invalid protection entry
const V3D_GMP_STATUS_CNTOVF: u32 = 1 << 2;
const V3D_GMP_STATUS_WR_ACTIVE: u32 = 1 << 5;
const V3D_GMP_STATUS_RD_ACTIVE: u32 = 1 << 4;
const V3D_GMP_STATUS_GMPRST: u32 = 1 << 31; // GMP in reset (protection tables not loaded)

// ─── PI-V3D-21: performance-counter (PCTR) block, core 0. Offsets + field layout transcribed VERBATIM
// from Linux `drivers/gpu/drm/v3d/v3d_regs.h` (V3D 4.x / "V4" variant; register/hardware facts,
// GPL-2.0-only header, facts-only discipline). Used to WITNESS coordinate-shader QPU execution without
// perturbing the shader (see the pctr_* functions). The programming sequence (SRC selects → EN=mask →
// CLR=mask → OVERFLOW=mask; read PCTRx; EN=0) is the exact `v3d_perfmon_start`/`v3d_perfmon_stop` idiom.
const V3D_V4_PCTR_0_EN: usize = 0x0650; // per-counter enable mask (bit i enables counter i)
const V3D_V4_PCTR_0_CLR: usize = 0x0654; // per-counter clear-to-0 mask
const V3D_PCTR_0_OVERFLOW: usize = 0x0658; // per-counter overflow-clear mask
const V3D_V4_PCTR_0_SRC_0_3: usize = 0x0660; // source-select for counters 0..3 (four 7-bit S0..S3 fields)
const V3D_V4_PCTR_0_SRC_4_7: usize = 0x0664; // source-select for counters 4..7 (SRC_0_3 + 4; S4..S7 fields)
const V3D_PCTR_0_PCTR0: usize = 0x0680; // counter 0 output; counter i output = PCTR0 + 4*i
// Counter SOURCE ids — the `enum v3d_perfcnt` INDEX from Linux uapi `include/uapi/drm/v3d_drm.h`. On
// V3D 4.2 (ver<71) the enum index IS the hardware source id written into the SRC field (cross-checked:
// CYCLE_COUNT sits at enum index 32, matching v3d_regs.h `V3D_PCTR_CYCLE_COUNT(ver)=32` for ver<71).
const PCTR_SRC_QPU_ACTIVE_CYCLES_VERTEX_COORD_USER: u32 = 14; // QPU cycles executing vertex/coord USER shaders
const PCTR_SRC_QPU_CYCLES_VALID_INSTR: u32 = 16; // QPU cycles issuing a valid instruction
const PCTR_SRC_CYCLE_COUNT: u32 = 32; // total core clock cycles (block-was-clocked sanity)
// PI-V3D-33 TMU-block witness sources — same `enum drm_v3d_perfcnt` index==source-id contract, and all
// three fall BETWEEN the code's own verified anchors (16 valid_instr, 32 cycle_count), so they inherit
// the same cross-check. These three counter the TMU pipe directly so the next boot distinguishes "the
// QPU never issued the general store to the TMU" (all three read 0) from "the TMU was engaged but the
// write never reached DRAM" (any nonzero → the store fired; a drain/L2T/address defect, not an issue
// defect). No known-working TMU op exists on this hardware to calibrate against (both fragment shaders
// use TLB writes / ldvary, never a TMU lookup — see FS_WORDS / GRAD_FS_WORDS), so the trio is read as a
// battery: any member nonzero proves the TMU block saw activity.
const PCTR_SRC_QPU_CYCLES_WAITING_TMU: u32 = 17; // QPU cycles stalled waiting on the TMU
const PCTR_SRC_TMU_TCACHE_ACCESS: u32 = 24; // TMU tcache accesses (per TMU op that touches the cache)
const PCTR_SRC_TMU_TCACHE_MISS: u32 = 25; // TMU tcache misses (a store to a fresh line misses)

// ─── PI-V3D-63 (RANK 1, `ptbctr`) — the PTB/CLE-side counter bank. ───────────────────────────────────
//
// The arc's brief asks for three NAMED sources: a CLE bin-thread active-cycles counter, a PTB
// primitives-binned counter, and a PTB primitives-clipped/discarded counter. **None of the three can be
// cross-checked here.** The file's sourcing rule (see the PI-V3D-33 block above) admits a source id only
// when it falls BETWEEN the two verified anchors — 16 `QPU_CYCLES_VALID_INSTR` and 32 `CYCLE_COUNT`,
// the latter pinned to `v3d_regs.h V3D_PCTR_CYCLE_COUNT(ver)=32` for ver<71. Every candidate for those
// three names lies OUTSIDE that bracket (the PTB primitive family below index 14, the CLE thread family
// above 25), and this tree carries no copy of `enum drm_v3d_perfcnt` to transcribe from. Naming them
// from memory is exactly the fabricated-constant bug class this campaign has been convicted of three
// times (PI-V3D-4/6/7), so:
//
//   **All three NAMED sources are DROPPED, not guessed.** The witness line says so on every boot.
//
// What is armed instead is a SEARCH, not a claim: the seven source ids below are written into the SRC
// mux and their counters are printed RAW, by index, with NO semantic label — the same treatment V3D-60
// gave the unsourced MMU interrupt halves. Writing an id into a 7-bit SRC field selects a mux; it
// asserts nothing about what that mux carries. The identification, if it comes, comes from the
// DIFFERENTIAL (a zero-primitive frame vs. the full draw, the same bank on both), never from us. The ids
// bracket the two families the brief names — the sub-14 primitive/front-end region and the post-25
// thread/stall region — chosen as a search window and defensible only as one.
//
// Slot 7 always carries the anchored `PCTR_SRC_CYCLE_COUNT` (32) as the instrument's own control: if it
// reads zero the bank was never counting and every other slot in the bank is INCONCLUSIVE, not clean.
const V3D63_SWEEP_SRC: [u32; 7] = [1, 10, 11, 12, 13, 28, 29];
const V3D63_PCTR_MASK: u32 = 0xFF; // eight counters enabled (0..7)
// **Slot 2 is RESERVED for `PCTR_SRC_CYCLE_COUNT` and the sweep may never occupy it.** `wait_fldone`
// samples PCTR counter 2 on every bin wait and `emit_v3d55_clock_liveness` prints it as a hard hardware
// verdict — `counter2(src32 CYCLE_COUNT)`, gated only on `PCTR_EN` bit 2 being set, which this bank
// always sets. A sweep id parked in slot 2 would make all four banked rungs print a FABRICATED
// "[v3d55] CYCLE_COUNT FLAT / the V3D core clock is NOT advancing" verdict off an unidentified mux, and
// it would contradict this bank's own control. Pinning src32 to slot 2 keeps the pre-existing `[v3d55]`
// witness truthful across V3D-63's rungs AND serves as this bank's control — one counter, both roles,
// no re-labelling of an established witness string. The seven swept ids therefore live in slots
// 0,1,3,4,5,6,7; `v3d63_slot` is the only place that mapping is written down.
const V3D63_CTRL_SLOT: usize = 2; // slot 2 = src32 CYCLE_COUNT — the [v3d55] slot AND this bank's control

/// PI-V3D-63 — sweep index → PCTR counter slot, skipping the reserved src32 control slot.
const fn v3d63_slot(i: usize) -> usize {
    if i < V3D63_CTRL_SLOT {
        i
    } else {
        i + 1
    }
}

// CLE (control-list executor) — CT1 is the RENDER queue. Submitting a job = write the ring's start
// address to CT1QBA and its end address to CT1QEA; the hardware runs [BA, EA).
const V3D_CLE_CT0CS: usize = 0x0100; // CT0 (bin) control/status — witness only (render job uses CT1)
const V3D_CLE_CT1CS: usize = 0x0104; // CT1 control/status (bit5 = CTRUN busy)
const V3D_CLE_CT1CA: usize = 0x0114; // CT1 current address — the address the CLE is executing at
// PI-V3D-7 kick-path root cause: the CT1 queue-submit registers were at FABRICATED offsets. The
// begin/end addresses were written to 0x324/0x334 — not even inside the CLE register block (which
// ends at CT1QCFG 0x178). The verbatim v3d_regs.h queue slots are CT{0,1}QBA at 0x160/0x164 and
// CT{0,1}QEA at 0x168/0x16c. Writing CT1QEA is the CLE's GO signal; sending it to 0x334 meant CT1's
// real queue-end (0x16c) never fired, so the render CLE never started — CT1CA stuck at 0, CTRUN
// never latched. That is precisely the boot-P3 "never-started" signature (same fabricated-offset
// class as the PI-V3D-4 MMU-constant bug). Corrected to the transcribed offsets below.
const V3D_CLE_CT1QBA: usize = 0x0164; // CT1 queue begin address (v3d_regs.h V3D_CLE_CT1QBA)
const V3D_CLE_CT1QEA: usize = 0x016c; // CT1 queue end address (v3d_regs.h V3D_CLE_CT1QEA) — QEA write kicks
const V3D_CLE_CT1CS_CTRUN: u32 = 1 << 5; // per v3d_regs.h V3D_CLE_CTRUN

// PI-V3D-8 — CT0 (the BINNING queue). The M4 triangle first runs a BIN job on CT0 (the coordinate
// shader transforms the vertices, the PTB bins them into per-tile lists), then the RENDER job on CT1
// consumes those lists via BRANCH_TO_IMPLICIT_TILE_LIST. Every offset below is transcribed VERBATIM
// from Linux `drivers/gpu/drm/v3d/v3d_regs.h` (register offsets are hardware facts — safe to lift from
// the GPL-2.0-only header; same discipline as the PI-V3D-7 CT1 fix). NOT invented — the CT1 side is
// merely CT0+4 in every case, which the file already relies on for CT1QBA/CT1QEA.
//   0x100 CT0CS · 0x110 CT0CA · 0x160 CT0QBA · 0x168 CT0QEA · 0x170 CT0QMA · 0x174 CT0QMS.
// V3D_CLE_CT0CS is already declared above (0x0100) as the M3 witness-only register.
const V3D_CLE_CT0CA: usize = 0x0110; // CT0 current address (v3d_regs.h V3D_CLE_CT0CA)
const V3D_CLE_CT0QBA: usize = 0x0160; // CT0 queue begin address (v3d_regs.h V3D_CLE_CT0QBA)
const V3D_CLE_CT0QEA: usize = 0x0168; // CT0 queue end address (v3d_regs.h V3D_CLE_CT0QEA) — QEA write kicks
const V3D_CLE_CT0QMA: usize = 0x0170; // CT0 bin TILE-ALLOCATION memory base (v3d_regs.h V3D_CLE_CT0QMA)
const V3D_CLE_CT0QMS: usize = 0x0174; // CT0 bin TILE-ALLOCATION memory size  (v3d_regs.h V3D_CLE_CT0QMS)
// PI-V3D-9 boot-P5 root cause: the M4 base wrote the tile-STATE region (192 B) into CT0QMA/QMS as if it
// were the tile-ALLOCATION pool and never programmed CT0QTS at all. Per Linux v3d_sched.c
// `v3d_bin_job_run` the three are DISTINCT: CT0QMA/QMS = tile-ALLOCATION memory (the pool the binner
// grows per-tile primitive lists into), CT0QTS = tile-STATE data array (ENABLE-gated). Handing the
// binner a 192-byte "pool" overflowed it immediately → it walked off into an unmapped page →
// PT_INVALID (MMU_fault bit20) with CT0CA halted mid-list. Corrected below. CT0QTS offset + ENABLE bit
// transcribed VERBATIM from Linux v3d_regs.h (register facts; GPL-2.0-only header, facts-only).
const V3D_CLE_CT0QTS: usize = 0x015c; // CT0 bin tile-STATE data array base (v3d_regs.h V3D_CLE_CT0QTS)
const V3D_CLE_CT0QTS_ENABLE: u32 = 1 << 1; // v3d_regs.h V3D_CLE_CT0QTS_ENABLE — gate the tile-state write
// PI-V3D-13 fact-check (Linux v3d_regs.h + v3d_sched.c v3d_bin_job_run, verbatim; facts only —
// GPLv2): CT0QTS=0x15c with ENABLE=BIT(1), CT0QBA=0x160, CT0QEA=0x168, CT0QMA=0x170, CT0QMS=0x174;
// bin submit order = invalidate caches, then CT0QMA (pool base) → CT0QMS (pool SIZE, not end) →
// CT0QTS|ENABLE → CT0QBA → CT0QEA (GO). On 4.x the TILE_BINNING_MODE_CFG packet carries only the
// tile-alloc BLOCK-SIZE enums (Mesa v3dvx_cmd_buffer.c job_emit_binning_prolog), never the pool
// address — the pool/state addresses travel ONLY through these registers. This file's programming
// already matches all of it (PI-V3D-9); PI-V3D-13 adds the pre-kick readback + post-bin pool-head
// witnesses so the next metal sitting sees exactly which half of that story the silicon disputes.
// CTnCS status: only CTRUN (bit5) is corroborated across sources for V3D 4.x; the remaining bits differ
// from the VideoCore-IV layout and are reported raw rather than guessed (no fabricated bit names).
//
// PI-V3D-59 AMENDMENT — this finding STANDS and is not refuted. What V3D-59 adds is a decode of the
// remaining bits under the VC4-era map anyway, explicitly labelled a hedged inference, used for
// DIAGNOSTIC OUTPUT ONLY and gating no behaviour (the CTRSTA write it makes possible stays disarmed).
// The "remaining bits differ from the VideoCore-IV layout" caution above is the reason those readings
// are hedged and the reason CTSEMA/CTRTSD are printed as raw windows rather than booleans — see the
// PI-V3D-59 bit-map block further down, which carries the full lineage and the falsifier.
const V3D_CLE_CTNCS_CTRUN: u32 = 1 << 5;

// ── V3D-41 PTB / frame-completion witness registers ──────────────────────────────────────────────
// Offsets transcribed VERBATIM from Linux drivers/gpu/drm/v3d/v3d_regs.h (register offsets are hardware
// facts; GPLv2-only header, facts-only — same discipline as the PI-V3D-8/9/13 CT0 lifts above). These
// discriminate the V3D-40 wall ("CT0CA reached EA, coord shader RAN, pool+tile-state stayed zero") into
// its two remaining branches — a bin frame that NEVER started the PTB, vs one that started but wrote
// nothing — by reading state the CT0CS/pool witnesses alone cannot see:
//   CT0LC/CT0PC — CLE list-counter / primitive-counter (how many list items / primitives the CLE fed).
//   BFC/RFC     — bin / render FRAME-completion counters: BFC increments once per completed bin frame
//                 (v3d_irq.c FLDONE path). BFC advancing across the kick = a bin frame COMPLETED (the PTB
//                 ran a frame); BFC unchanged = START_TILE_BINNING never brought up a PTB frame despite
//                 the CLE walking BA→EA. This is the decisive "started vs never-started" bit.
//   PCS         — CLE pipeline control/status (bin/render busy + empty). Bit names past CTRUN are NOT
//                 corroborated for 4.x, so PCS is reported RAW (no fabricated bit decode; §5 law).
//   PTB BPCA/BPCS/BPOA/BPOS — the PTB's binning-primitive-list write pointer + size, and its overflow
//                 allocation pointer + size. BPCA is the address the PTB is writing tile lists INTO; if
//                 binning emitted anything, BPCA advances off the pool base (CT0QMA). BPCA == pool base
//                 (or 0) with BFC advanced = the frame ran but the PTB produced no primitive-list bytes
//                 (empty bin — geometry clipped/culled to nothing on-chip). BPOA nonzero = the binner
//                 requested an overflow block (it ran out of pool — the opposite failure).
const V3D_CLE_CT0LC: usize = 0x0120; // CT0 list counter (v3d_regs.h V3D_CLE_CT0LC)
const V3D_CLE_CT0PC: usize = 0x0128; // CT0 primitive-list counter (v3d_regs.h V3D_CLE_CT0PC)
const V3D_CLE_PCS: usize = 0x0130; // CLE pipeline control/status (v3d_regs.h V3D_CLE_PCS) — raw
// PI-V3D-46: PCS bit decode. The Linux v3d_regs.h treats PCS as opaque, but the field layout is public
// in the Broadcom VideoCore IV 3D Architecture Reference Guide (VideoCoreIV-AG100-R, §V3D_PCS) and is
// unchanged across the CLE in V3D 4.x: BMACTIVE=bit0 (binning pipeline IN USE — set by START_TILE_BINNING,
// cleared when the bin frame flushes/retires), BMBUSY=bit1 (a binning operation actually in progress),
// RMACTIVE=bit2 / RMBUSY=bit3 (the render-side pair), BMOOM=bit8 (PTB ran out of binning memory). So the
// P44 read PCS=0x1 = BMACTIVE set, BMBUSY/RMACTIVE/RMBUSY/BMOOM all clear = binning mode still ACTIVE with
// NO work in progress and NO out-of-memory — the bin frame never tore down though the CLE consumed FLUSH.
const V3D_PCS_BMACTIVE: u32 = 1 << 0; // Binning Mode Active — pipeline in use
const V3D_PCS_BMBUSY: u32 = 1 << 1; // Binning Mode Busy — a bin op is in progress
const V3D_PCS_RMACTIVE: u32 = 1 << 2; // Rendering Mode Active
const V3D_PCS_RMBUSY: u32 = 1 << 3; // Rendering Mode Busy
const V3D_PCS_BMOOM: u32 = 1 << 8; // Binning Mode Out Of Memory (PTB pool exhausted)
const V3D_CLE_BFC: usize = 0x0134; // bin frame count (v3d_regs.h V3D_CLE_BFC)
const V3D_CLE_RFC: usize = 0x0138; // render frame count (v3d_regs.h V3D_CLE_RFC)
const V3D_PTB_BPCA: usize = 0x0300; // PTB binning primitive-list current address (v3d_regs.h V3D_PTB_BPCA)
const V3D_PTB_BPCS: usize = 0x0304; // PTB binning primitive-list current size    (v3d_regs.h V3D_PTB_BPCS)
const V3D_PTB_BPOA: usize = 0x0308; // PTB binning primitive-list overflow address (v3d_regs.h V3D_PTB_BPOA)
const V3D_PTB_BPOS: usize = 0x030c; // PTB binning primitive-list overflow size    (v3d_regs.h V3D_PTB_BPOS)

// ── PI-V3D-59: the three CLE/PTB registers the whole V3D-40..58 campaign never read ──────────────
// Offsets transcribed VERBATIM from Linux `drivers/gpu/drm/v3d/v3d_regs.h` (GPL-2.0-only header;
// register offsets and bit positions are hardware facts, lifted facts-only under the same discipline
// as every other offset in this block). All three are defined upstream and are written/read by NO
// path in the mainline v3d driver — which is exactly why they have never appeared in this file, and
// exactly why their live values are unknown territory for the "frame opens but never closes" wedge.
//
//   CT0SYNC/CT1SYNC — the CLE per-thread SEMAPHORE registers. On the VC4-era block of the same family
//                     the binner and renderer rendezvous through a semaphore that `INCREMENT_SEMAPHORE`
//                     (bin) and `WAIT_ON_SEMAPHORE` (render) move, and `CTnCS.CTSEMA` reflects. Modern
//                     Mesa emits neither packet (see `v3d59_mainline_ledger`), so the semaphore should
//                     sit at its reset value across our frames — a NON-reset reading at S0 would mean
//                     the block carried CLE rendezvous state into the bin from the preceding CT1 job.
//   BXCF            — PTB "binner extra config". `V3D_PTB_BXCF_CLIPDISA` (bit0) disables the PTB's
//                     clipper; `V3D_PTB_BXCF_RWORDERDISA` (bit1) disables read/write ordering. Mainline
//                     never writes it, so it should read 0 after the reset cycle. A stray CLIPDISA (or
//                     any set bit) in a block we reset ourselves is a bring-up fact worth one read.
const V3D_CLE_CT0SYNC: usize = 0x0154; // CT0 semaphore (v3d_regs.h V3D_CLE_CT0SYNC) — never touched
const V3D_CLE_CT1SYNC: usize = 0x0158; // CT1 semaphore (v3d_regs.h V3D_CLE_CT1SYNC) — never touched
const V3D_PTB_BXCF: usize = 0x0310; // PTB binner extra config (v3d_regs.h V3D_PTB_BXCF)
const V3D_PTB_BXCF_CLIPDISA: u32 = 1 << 0; // v3d_regs.h V3D_PTB_BXCF_CLIPDISA
const V3D_PTB_BXCF_RWORDERDISA: u32 = 1 << 1; // v3d_regs.h V3D_PTB_BXCF_RWORDERDISA

// ── PI-V3D-59: the CTnCS bit map — a HEDGED VC4-family INFERENCE, not a corroboration ────────────
//
// §32 declined to decode `CTnCS` past `CTRUN` because mainline `v3d/v3d_regs.h` defines no
// `V3D_CLE_CT0CS` bit fields, and line ~299 of this file records the stronger finding that only CTRUN
// "is corroborated across sources for V3D 4.x; the remaining bits differ from the VideoCore-IV layout."
// **This block does NOT refute that finding, and must not be read as doing so.** It borrows the VC4-era
// map anyway, on a stated and falsifiable basis, and every reading it produces is hedged accordingly.
//
// What the borrow actually rests on — stated truthfully, because the first draft of this block got the
// lineage wrong and claimed a precedent that does not exist:
//
//   * This driver's rule has always been **offsets from the headers, semantics from the ARG**. The
//     `V3D_PCS` decode at ~304 above says in terms that Linux `v3d_regs.h` treats PCS as OPAQUE and
//     takes BMACTIVE/BMBUSY/RMACTIVE/RMBUSY/BMOOM from the Broadcom VideoCore IV 3D Architecture
//     Reference Guide (VideoCoreIV-AG100-R). §30 did the same for `BPCA`/`BPCS`: ARG for the field
//     meaning, `vc4_regs.h`/`v3d_regs.h` cited only to establish OFFSET IDENTITY at 0x300/0x304.
//   * So the honest description of what follows is: `drivers/gpu/drm/vc4/vc4_regs.h` publishes a bit
//     map for the register at offset 0x100, an offset identical across the VC4 and V3D 4.x CLE —
//
//       V3D_CT0CS 0x00100 / V3D_CTNCS(n) · CTRSTA BIT(15) · CTSEMA BIT(12) · CTRTSD BIT(8)
//                                          CTRUN  BIT(5)  · CTSUBS BIT(4)  · CTERR BIT(3) · CTMODE BIT(0)
//
//     and this is a **VC4-era ARG-family map carried across on offset identity alone**. That is the same
//     *class* of inference §30 and the PCS decode rest on, but it is weaker in one specific way: for PCS
//     and BPCA/BPCS the ARG text and the header agree, whereas here line ~299 records that the 4.x
//     layout diverges from VideoCore IV somewhere past CTRUN, without saying where.
//   * A live corroboration crack, found while re-checking the header for this arc: `vc4_regs.h` declares
//     `CTSEMA` and `CTRTSD` as single-bit `BIT(12)`/`BIT(8)`, while the ARG describes the semaphore and
//     return-to-sub-list-depth as MULTI-BIT count fields. The two published sources for the same
//     register disagree about field WIDTH. That is exactly the divergence ~299 warns about, so this file
//     prints those two as raw masked WINDOWS (see `V3D_CLE_CTNCS_CTSEMA_WIN`/`CTRTSD_WIN`) and promises
//     no boolean and no depth.
//
// **The falsifier.** The borrowed map is indicted — not merely unconfirmed — if `[v3d59] ctstate` reads
// `CTERR` SET at S0 on a block that has just come through a fresh OFF->ON reset cycle and that then
// renders a clean CT1 frame. A control thread cannot be both errored-from-birth and healthy enough to
// retire a render frame; that combination means bit 3 is not CTERR on 4.x and the whole map is wrong.
// The `[v3d59] ctstate` verdict logic checks for exactly that and says so.
//
// Nothing here is used to gate behaviour. `CTRSTA` stays disarmed (V3D59_ARM_CT0_RESET); the decode is
// diagnostic output whose every verdict names its own hedge. Under that constraint a hedged inference is
// worth printing where a fabricated constant driving a register write would not be — the PI-V3D-4/-6 bug
// class was fabricated values steering the hardware, not labelled guesses in a log line.
const V3D_CLE_CTNCS_CTRSTA: u32 = 1 << 15; // reset the control thread (write-1) — INFERRED, disarmed
const V3D_CLE_CTNCS_CTSUBS: u32 = 1 << 4; // control thread executing a SUB-list — INFERRED
const V3D_CLE_CTNCS_CTERR: u32 = 1 << 3; // control thread ERROR — INFERRED
const V3D_CLE_CTNCS_CTMODE: u32 = 1 << 0; // control thread mode — INFERRED
// The two fields whose published widths DISAGREE between vc4_regs.h (single-bit) and the ARG
// (multi-bit). Printed as raw windows and reported as values, never as booleans: on the ARG reading a
// semaphore count of 2 or a sub-list depth of 2 would read `0` through a BIT(12)/BIT(8) test, which is
// precisely how a wrong decode manufactures a reassuring log line.
const V3D_CLE_CTNCS_CTSEMA_WIN: u32 = 0b111 << 12; // ARG: semaphore count, bits 14:12 (vc4_regs.h: BIT(12) only)
const V3D_CLE_CTNCS_CTSEMA_SHIFT: u32 = 12;
const V3D_CLE_CTNCS_CTRTSD_WIN: u32 = 0b11 << 8; // ARG: return-to-sub-list depth, bits 9:8 (vc4_regs.h: BIT(8) only)
const V3D_CLE_CTNCS_CTRTSD_SHIFT: u32 = 8;

// PI-V3D-44: per-CORE interrupt status/clear registers (v3d_regs.h V3D_CTL_INT_*, read via
// V3D_CORE_READ in v3d_irq.c on the 4.x path). The kernel driver treats a completed bin as
// "retired" ONLY when V3D_INT_FLDONE (binning FLUSH done) latches in INT_STS — NOT when the
// CT0CS run bit drops (the CLE can idle while the PTB is still draining / awaiting overflow).
// Our post-kick "idle" predicate has always been CT0CS-based, which is why P40 read the pool
// pre-retire (BFC Δ0, PCS bit0=1). These offsets let us poll the true retire signal instead.
const V3D_CTL_INT_STS: usize = 0x0050; // latched interrupt status (v3d_regs.h V3D_CTL_INT_STS)
const V3D_CTL_INT_CLR: usize = 0x0058; // write-1-to-clear latched interrupts (V3D_CTL_INT_CLR)
// PI-V3D-49: the interrupt MASK block (v3d_regs.h V3D_CTL_INT_MSK_*). MSK_STS reads the current mask
// (1 = that interrupt is masked/disabled); MSK_SET sets mask bits (disable); MSK_CLR clears mask bits
// (enable). The kernel programs these ONCE at probe in `v3d_irq_enable` — NOT per bin job — to unmask
// its working set before any job runs; `v3d_bin_job_run` itself writes no mask. Our driver never
// programmed the mask at all: the block ran every bin frame at the mask's power-on-reset value, an
// un-audited frame-level enable. V3D-49 makes us kernel-faithful: unmask the working set once at
// bring-up (see `v3d_irq_enable`).
const V3D_CTL_INT_MSK_STS: usize = 0x005c; // current interrupt mask (1 = masked) (V3D_CTL_INT_MSK_STS)
const V3D_CTL_INT_MSK_SET: usize = 0x0060; // write-1-to-MASK (disable) an interrupt (V3D_CTL_INT_MSK_SET)
const V3D_CTL_INT_MSK_CLR: usize = 0x0064; // write-1-to-UNMASK (enable) an interrupt (V3D_CTL_INT_MSK_CLR)
// V3D 4.x per-core interrupt bit layout (v3d_regs.h, the 33_5+ block):
const V3D_INT_FRDONE: u32 = 1 << 0; // render frame done
const V3D_INT_FLDONE: u32 = 1 << 1; // binning flush done — the true bin-retire signal
const V3D_INT_OUTOMEM: u32 = 1 << 2; // binner ran out of tile-alloc memory (needs an overflow block)
const V3D_INT_SPILLUSE: u32 = 1 << 3; // QPU spill-memory used
const V3D_INT_TRFB: u32 = 1 << 4; // transform-feedback-block done (v3d_regs.h V3D_INT_TRFB)
const V3D_INT_GMPV: u32 = 1 << 5; // GMP (memory-protection) violation
// PI-V3D-45: the top half-word of INT_STS is the per-QPU host-interrupt vector. Transcribed VERBATIM
// from Linux `drivers/gpu/drm/v3d/v3d_regs.h` (register/hardware facts; GPL-2.0-only header, facts-only):
//   # define V3D_INT_QPU_MASK   0xffff0000
//   # define V3D_INT_QPU_SHIFT  16
// Each bit [16+n] latches when QPU n raises a HOST interrupt — the QPU thread executed an instruction
// carrying the `sig: thrsw + interrupt` (thread-end-with-host-int) signal (Mesa qpu_instr.h V3D_QPU_SIG,
// the QPU's `sig.int` / program-end-interrupt path). So INT_STS=0x0001_0000 (bit16, n=0) = "QPU 0 raised
// a program-end host interrupt" — the QPU RAN and signalled completion, independent of the PTB's FLDONE.
const V3D_INT_QPU_MASK: u32 = 0xffff_0000; // [31:16] per-QPU host-interrupt vector
const V3D_INT_QPU_SHIFT: u32 = 16; // bit (16+n) = QPU n raised a host interrupt
// PI-V3D-49: the kernel's working interrupt set for V3D 4.2 (`V3D_CORE_IRQS`, v3d_irq.c, ver<71 path):
// OUTOMEM | FLDONE | FRDONE | CSDDONE(BIT7 for ver<71) | GMPV(BIT5). This is the exact bitset
// `v3d_irq_enable` UNMASKS (MSK_CLR) and everything else it MASKS (MSK_SET = ~this). FLDONE (BIT1) — the
// bin-retire signal our poll waits on — is in it. Note the per-QPU host-interrupt half-word (bits 16+)
// is deliberately NOT in the kernel's set: those latch in INT_STS regardless (which is why our witnesses
// saw bit16 despite never touching the mask — INT_STS is the RAW latched vector, the mask gates only the
// CPU IRQ line, so masking never inverted our FLDONE=0 reads; see v3d.md §23).
const V3D_INT_CSDDONE_LT71: u32 = 1 << 7; // compute-shader-dispatch done (ver < 71)
const V3D_CORE_IRQS: u32 =
    V3D_INT_OUTOMEM | V3D_INT_FLDONE | V3D_INT_FRDONE | V3D_INT_CSDDONE_LT71 | V3D_INT_GMPV; // 0xa7

// PI-V3D-44 overflow (BPO) pool — the tail of the arena (0x36000, above the probe bin CL at
// 0x35000+0x1000). The V3D binner requests overflow tile-list memory via the OUTOMEM interrupt;
// the kernel driver answers by writing BPOA (block address) + BPOS (block size) — v3d_irq.c's
// V3D_INT_OUTOMEM path → v3d_overflow_mem_work. We give the PTB NO overflow pool today (BPOA/BPOS
// unset = 0), so if the initial 128 B tile-alloc block is exhausted the binner stalls waiting for
// overflow memory that never arrives — FLDONE never fires. Pre-arm a small overflow block so the
// PTB can complete the flush without the interrupt round-trip.
const OFF_PROBE_BIN_OVERFLOW: usize = 0x36000; // probe overflow tile-list pool (BPOA)
const PROBE_BIN_OVERFLOW_BYTES: usize = 0x2000; // 8 KiB — ample overflow for the probe bin
const _: () = assert!(OFF_PROBE_BIN_OVERFLOW + PROBE_BIN_OVERFLOW_BYTES <= ARENA_BYTES);

// PI-V3D-48 empty-frame bisection scratch (arena tail, above the overflow pool). The ladder builds each
// rung's CL into OFF_PROBE_BIN_CL (free after probe_job) and reuses the M4 tile-alloc / tile-state / BPO
// regions; it needs only its own NULL coord-shader program + shader record for the PrimsNullShader rung.
const OFF_BISECT_NULL_CODE: usize = 0x38000; // NULL coord shader: the 4-word Mesa thread-end tail
const OFF_BISECT_NULL_SHADREC: usize = 0x38400; // shader record whose CS/VS/FS select the NULL program
const _: () = assert!(OFF_BISECT_NULL_SHADREC + 36 + 16 <= ARENA_BYTES);

// PI-V3D-63 (RANK 2, `ctrartifact`) — the RELOCATED bin control list. The whole probe is a placement
// differential: the SAME `BinContent` published at two different arena offsets, so a `CT0LC` that reads
// back the list's own base (the P45 reading `CT0LC=0x10000` is bit-identical to `OFF_BIN_CL`) is
// distinguishable from a `CT0LC` that counts list items. One free page above the bisection scratch.
const OFF_V3D63_BIN_CL_ALT: usize = 0x39000;
const _: () = assert!(OFF_V3D63_BIN_CL_ALT + 0x1000 <= ARENA_BYTES);
const _: () = assert!(OFF_V3D63_BIN_CL_ALT >= OFF_BISECT_NULL_SHADREC + 0x400);

// PI-V3D-68 — the frame-geometry ladder's own TILE-STATE DATA ARRAY (the CT0QTS target).
//
// Every bin frame this campaign has ever submitted — every rung of every arc — used the SAME 64x64
// frame, which at the 64x64 bin tile Mesa picks for 1 RT / 32 bpp / no MSAA is exactly ONE tile, and
// therefore a 256 B TSDA at `OFF_TILESTATE` (whose page has room for 16 tiles at most). A geometry
// ladder needs a bigger array than that page can hold, and it must not disturb the region every banked
// verdict above was taken on. So the ladder gets a dedicated 24 KiB region in the arena tail.
//
// SIZING, with the tile-size contingency stated rather than assumed. This tree's established tile size
// for our configuration is 64x64 (see the `TILE_STATE_TILES` note). If that were wrong and the real bin
// tile were 32x32, every geometry below would carry FOUR TIMES the tiles. The region is sized so that
// even under that contingency the largest rung's TSDA still fits INSIDE it — 256x256 is 16 tiles at
// 64x64 (4 KiB) and 64 tiles at 32x32 (16 KiB), both under 24 KiB — so a mis-estimated tile size can
// cost the ladder a reading, never a write outside the arena. `v3d68_geom_sound` enforces exactly that
// bound at run time, and the ladder fail-closes rather than kick a rung it cannot bound.
const OFF_V3D68_TSDA: usize = 0x3A000;
const V3D68_TSDA_BYTES: usize = 0x6000; // 24 KiB = 96 tiles at 256 B/tile
const _: () = assert!(OFF_V3D68_TSDA + V3D68_TSDA_BYTES <= ARENA_BYTES);
const _: () = assert!(OFF_V3D68_TSDA >= OFF_V3D63_BIN_CL_ALT + 0x1000);
const _: () = assert!(OFF_V3D68_TSDA % 32 == 0); // CT0QTS carries the base in its upper bits (ENABLE=bit1)

// ─── The V3D buffer arena. One page-aligned static in BSS. Because the bare-metal kernel is
// identity-mapped in low RAM (VA == PA), the address of this static IS its ARM physical address,
// which is exactly what the V3D MMU page table and the control lists need. Sized generously and
// used sparsely; every sub-region is a bounded slice of it. ───
const ARENA_PAGES: usize = 64; // 256 KiB — ample for a 64×64 clear target + control list + PT scratch
const PAGE: usize = 4096;
const ARENA_BYTES: usize = ARENA_PAGES * PAGE;

#[repr(C, align(4096))]
struct Arena {
    bytes: [u8; ARENA_BYTES],
}
static mut V3D_ARENA: Arena = Arena { bytes: [0; ARENA_BYTES] };

// The V3D MMU page table: one u32 PTE per 4 KiB of iova, indexed by (iova >> 12). We identity-map the
// arena (iova == phys) and leave every other entry invalid, so the arena's top phys page bounds the
// table size. The kernel image + BSS (hence the arena) sits far below the cap on the Pi 4;
// `program_mmu` asserts the arena fits before filling, never overflowing.
//
// PI-V3D-71: grown from 8192 (32 MiB of iova) to 16384 (64 MiB) so the `[v3d71]` mainline-geometry
// window (bin buffers at the piOS dump's ~0x03000000+ iovas) has real, PUBLISHED-zero PTEs to sit in.
// Confinement is unchanged — every entry outside the arena identity map (and, for the one deep-battery
// rung, outside the v3d71 window while it is mapped) is zero — and the growth is itself a hardening:
// an iova in [32 MiB, 64 MiB) previously indexed PAST the table's end into whatever DRAM followed it,
// where a stray VALID-looking word could have translated; now it hits a published zero and faults.
const PT_CAP: usize = 16384; // 16384 PTEs × 4 B = 64 KiB, covers iova [0, 64 MiB)
#[repr(C, align(4096))]
struct PageTable {
    ptes: [u32; PT_CAP],
}
static mut V3D_PT: PageTable = PageTable { ptes: [0; PT_CAP] };

// PI-V3D-62: the MMU illegal-address SCRATCH page. Mainline allocates one page for this and points
// V3D_MMU_ILLEGAL_ADDR at its physical page number; the register is a PHYSICAL redirect, not an iova, so
// an access the MMU refuses is steered to this page instead of to translated memory. The defining
// property is that the page belongs to NO job and is mapped by NOTHING: `program_mmu` never writes a PTE
// for it, so its page-table entry stays invalid, and it lies outside the arena. That makes the landing
// site of an illegal access DISTINGUISHABLE from every legal write — which arena page 0 (mapped
// VALID+WRITEABLE, and inside the very space the PTB writes) was not. It also turns the page into a
// witness: pre-seed it with a sentinel, and a sentinel that is gone after the frame is direct evidence
// that a refused access LANDED here.
#[repr(C, align(4096))]
struct MmuScratch {
    bytes: [u8; PAGE],
}
static mut V3D_MMU_SCRATCH: MmuScratch = MmuScratch { bytes: [0; PAGE] };

/// The sentinel word pre-seeded across the illegal-address scratch page before a bin frame. Chosen
/// distinct from the arena's own 0xDEADBEEF fill so the two witnesses can never be confused.
const V3D62_SCRATCH_SENTINEL: u32 = 0x5C_A7_C4_11; // "SCRATCH" — arbitrary, only its uniqueness matters

#[inline]
fn mmio_read(base: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}
#[inline]
fn mmio_write(base: usize, off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, val) }
}
/// Full system barrier — ensure prior Device-nGnRnE register writes have reached the endpoint before
/// the following readback observes their effect. Device memory is already strongly ordered, but the
/// V3D hub sits behind the async AXI bridge; a `dsb sy` makes the program→verify handoff explicit.
#[inline]
fn dsb() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// ARM physical base of the arena (== its VA under the identity map).
#[inline]
fn arena_phys() -> usize {
    &raw const V3D_ARENA as usize
}
#[inline]
fn pt_phys() -> usize {
    &raw const V3D_PT as usize
}
/// ARM physical base of the MMU illegal-address scratch page (== its VA under the identity map).
#[inline]
fn mmu_scratch_phys() -> usize {
    &raw const V3D_MMU_SCRATCH as usize
}

/// A caller-supplied handle to the panel framebuffer, for the M3 visible witness (metal only).
#[derive(Clone, Copy)]
pub struct FbTarget {
    pub base: u64,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub stride_px: usize,
    pub bytes_per_pixel: usize,
}

/// The clear colour the GPU writes and the CPU verifies. UnaOS teal, RGBA8888 little-endian in the
/// tile buffer; byte order on store is configured in the RCL (BGRA vs RGBA) — the CPU verify reads
/// the same 32-bit word the store wrote, so the check is order-agnostic.
const CLEAR_RGBA: u32 = 0x00A6_8CFF; // (documented; exact channel order is store-config dependent)

/// Small render target: 64×64 RGBA8 = 16 KiB, one contiguous arena region.
const TARGET_W: usize = 64;
const TARGET_H: usize = 64;
const TARGET_BPP: usize = 4;
const TARGET_BYTES: usize = TARGET_W * TARGET_H * TARGET_BPP;

// Arena layout (byte offsets into the arena; all 4 KiB-aligned starts):
const OFF_TARGET: usize = 0; // [0, 16 KiB)  the clear target the GPU stores into
const OFF_RCL: usize = 0x8000; // [32 KiB, …) the main render control list (CT1 [BA,EA))
// PI-V3D-6: the render is a two-level control list, exactly as Mesa's v3dX(emit_rcl) builds it. The
// main list (OFF_RCL) is what CT1 executes; it branches into a *generic per-tile list* (OFF_SUBLIST)
// once per supertile via START_ADDRESS_OF_GENERIC_TILE_LIST + SUPERTILE_COORDINATES, and that sub-list
// carries the actual tile-buffer STORE. The tile-allocation scratch (OFF_TILEALLOC) is the base a
// binner would fill; our clear-only render never emits BRANCH_TO_IMPLICIT_TILE_LIST, so it is never
// dereferenced — present only because MULTICORE_RENDERING_TILE_LIST_SET_BASE requires an address.
const OFF_SUBLIST: usize = 0x9000; // generic per-tile list (branched to per supertile)
const OFF_TILEALLOC: usize = 0xA000; // tile-alloc base (inert: no binned geometry)

// ── PI-V3D-8 (M4 triangle) arena regions. All 4 KiB-aligned, all ABOVE the M3 regions so the M3
// clear-job is untouched (it must still PASS as the regression witness). Every region is inside the
// 256 KiB arena (top used byte 0x20000 < ARENA_BYTES 0x40000) and therefore inside the identity MMU
// map — a control list referencing any of these iovas is confined by the V3D MMU exactly like M3. ──
const OFF_M4_TARGET: usize = 0x0C000; // [48 KiB) the 64×64 RGBA8 the render stores the triangle into
const OFF_BIN_CL: usize = 0x10000; // binning control list (CT0 [BA,EA))
const OFF_TILESTATE: usize = 0x11000; // bin tile-state data array (CT0QTS; 256 B/tile, 1 tile here)
const OFF_BIN_TILEALLOC: usize = 0x12000; // bin tile-allocation memory (binner output; render reads it)
const OFF_M4_RCL: usize = 0x1A000; // M4 render control list (CT1 [BA,EA))
const OFF_M4_SUBLIST: usize = 0x1B000; // M4 generic per-tile list (branch-to-implicit + store)
const OFF_SHADREC: usize = 0x1C000; // GL Shader State Record (32-B aligned) + attribute record
const OFF_VTXDATA: usize = 0x1D000; // triangle vertex attribute data (3 verts × vec4 clip position)
const OFF_CS_CODE: usize = 0x1E000; // coordinate shader QPU code (binning: transform → VPM)
const OFF_VS_CODE: usize = 0x1E800; // vertex shader QPU code (render: transform + varyings → VPM)
const OFF_FS_CODE: usize = 0x1F000; // fragment shader QPU code (solid colour → TLB)
const OFF_DEFAULT_ATTRS: usize = 0x1F800; // default attribute values block (shader-record field)
// PI-V3D-9 uniform streams (each a bounded slice of one page, all inside the identity-mapped arena).
// The FS/VS/CS shader-record uniforms-address fields point here; the QPU pops these in FIFO order via
// the ldunifrf signal (and, for the FS, the TLBU-config pops).
const OFF_FS_UNIF: usize = 0x20000; // fragment-shader uniform stream (colour channels + TLB configs)
const OFF_CS_UNIF: usize = 0x20040; // coordinate-shader uniform stream (VPM read offsets)
const OFF_VS_UNIF: usize = 0x20080; // vertex-shader uniform stream (VPM read offsets)
// PI-V3D-14 pool sizing per Mesa v3d_util.c v3d_tile_alloc_sizes (the config the PTB is validated
// against): tiles_size = layers × tiles_x × tiles_y × 128 (INITIAL block, STATIC_ASSERTed == 128);
// pool = align(tiles_size, 4096) + 8192 ("the HW won't trigger OOM during the first allocations")
// + a draw-scaled continuation slush, page-aligned. Our 64×64 fb = 1 layer × 1×1 tiles →
// tiles_size = 128 → align 4096 + 8192 = 12,288 → page-aligned 16 KiB with slush. The existing
// 32 KiB region already covers Mesa's minimum with 2× headroom — kept (no arena-layout change).
const BIN_TILEALLOC_BYTES: usize = 0x8000; // 32 KiB of tile-alloc scratch for the binner

/// The solid triangle colour the fragment shader writes and the CPU verifies INSIDE the primitive.
/// Distinct from CLEAR_RGBA so the sample test can tell inside (this) from outside (clear). UnaOS
/// amber, RGBA8888 little-endian. (Exact channel order is store-config dependent, same as CLEAR_RGBA;
/// the CPU verify reads the same 32-bit word the store wrote, so the check is order-agnostic.)
const TRI_RGBA: u32 = 0x00FF_B000;


/// Entry point: bring the V3D up far enough to clear a buffer and verify it. Called once on the BSP,
/// single-threaded, after `emmc2::probe` (the mailbox is free by then). `fb` is the panel
/// framebuffer for the optional M3 visible blit (metal); `None` = serial-only witness.
///
/// Anti-hang discipline: every wait below is a FINITE wall-clock backstop off the free-running
/// CNTPCT (the ORIN-SMP determinism lesson), never an unbounded spin.
pub fn bringup(fb: Option<FbTarget>) {
    serial_println!(":: V3D: PI-V3D-1 bring-up starting (VideoCore VI / V3D 4.2) ::");

    // PI-V3D-79: the genpd-faithful MINIMAL bringup — the S1S source audit's discriminator.
    // On piOS, v3d_reset() is the HANG path only. Boot-time bring-up is genpd's
    // bcm2835_asb_power_on ALONE: deassert PM_V3DRSTN, release the two rpivid bridges, from the
    // firmware-parked state — no mailbox power/rate/gate, no reset assert (pd->clk resolves NULL
    // on 2711, so mainline's clock choreography no-ops). Everything else our bringup adds is a
    // suspect this knob removes in one boot: retire under MINIMAL convicts one of our extra acts.
    const V3D79_MINIMAL: bool = cfg!(feature = "v3d79_minimal");
    if V3D79_MINIMAL {
        serial_println!(
            ":: V3D: [v3d79] MINIMAL bringup — genpd-faithful: NO mailbox set_power_domain/set_clock_rate/set_clock_state, NO [v3d50] OFF half. Deassert V3DRSTN + release rpivid M,S — exactly bcm2835_asb_power_on on BCM2711, nothing else. Firmware clock state is whatever handoff left ([v3d55] clkdom reads it, read-only) ::"
        );
    }

    // ── M1: power, clock, probe. ───────────────────────────────────────────────────────────────
    // Power THEN clock, in that order (a powered-but-unclocked block reads garbage registers).
    if !V3D79_MINIMAL { match mailbox::set_power_domain(mailbox::POWER_DOMAIN_V3D, 1) {
        Some(1) => serial_println!(":: V3D: power domain {} ON ::", mailbox::POWER_DOMAIN_V3D),
        other => {
            serial_println!(
                ":: V3D: power domain did not report ON (got {:?}) — skipping GPU bring-up ::",
                other
            );
            return;
        }
    }
    match mailbox::set_clock_rate(mailbox::CLOCK_ID_V3D, 500_000_000) {
        Some(hz) => serial_println!(":: V3D: clock id {} rate set to {} Hz ::", mailbox::CLOCK_ID_V3D, hz),
        None => {
            serial_println!(":: V3D: clock rate set FAILED — skipping GPU bring-up ::");
            return;
        }
    }
    // Open the clock GATE. `set_clock_rate` above programs the *frequency* but the RPi firmware treats
    // rate and enable-state independently: a rate-set-but-gated clock leaves V3D powered-but-unclocked,
    // and its registers then read open-bus poison (0xdeadbeef). THIS is the PI-V3D-1 metal false-pass
    // gap — power + rate both ACKed, yet the block never decoded. Open the gate explicitly and require
    // the firmware to confirm the clock present AND active.
    match mailbox::set_clock_state(mailbox::CLOCK_ID_V3D, true) {
        Some(true) => serial_println!(":: V3D: clock id {} gate ENABLED (active) ::", mailbox::CLOCK_ID_V3D),
        other => {
            serial_println!(
                ":: V3D: clock gate did not report active (got {:?}) — skipping GPU bring-up ::",
                other
            );
            return;
        }
    } }

    // PI-V3D-50: the kernel-faithful V3D core RESET CYCLE (was PI-V3D-3's ON-half-only `enable_pm_asb`).
    // The kernel `v3d_reset_v3d` power-CYCLES the GRAFX_V3D domain (OFF then ON) to return the core to a
    // clean reset; UnaOS only ever ran the ON half once, on a firmware-powered block, leaving any stale
    // CLE/PTB/hub state uncleared — the wedge below per-job programming that §10–23 cornered. Sequenced
    // AFTER the firmware power/rate/gate steps and BEFORE the probe, so the probe reads a decoded block.
    // Best-effort + poison-honest: on QEMU these registers are absent and every wait is a finite backstop,
    // so the run still lands on the honest BLOCK-DOWN below; on metal this is what turns BUS-POISON into
    // BLOCK-UP and (the V3D-50 hypothesis) unwedges the empty-frame retire.
    // PI-V3D-60: the ONLY window in which VideoCore-firmware V3D state is still observable — after
    // power/clock/gate, before our OFF->ON cycle wipes it. Read-only, no budget, and it reads the hub
    // identity word first so an absent/poison block (QEMU raspi4b) is never followed into a core
    // register. This is the warm-handoff discriminator; see the PI-V3D-60 block.
    v3d60_residue_pre();

    if V3D79_MINIMAL {
        // [v3d79]: the genpd ON half alone — deassert V3DRSTN, release M then S. No OFF half.
        enable_pm_asb();
    } else {
        v3d_reset_cycle();
    }

    // Let the freshly powered + clocked + bridged block settle before its first register read (a
    // bounded wall-clock delay off CNTPCT — finite by construction, never an unbounded spin).
    settle_ms(2);

    // Poison-honest presence gate — the SOLE V3D thing QEMU raspi4b exercises, and it MUST NOT fault.
    // We read HUB IDENT0 FIRST and decide on it alone, because a core-register read on an absent block
    // raises a synchronous external abort (EC=0x25) — and `AARCH64 EXCEPTION` is a forbidden regression
    // pattern. The probe discriminates THREE fail-safe verdicts (PI-V3D-1's false-pass was a gate that
    // only rejected zero and so accepted the 0xdeadbeef firmware fill as "present"):
    //   * BLOCK-UP   — a live, non-poison identity word  → proceed to the core registers.
    //   * BLOCK-DOWN — 0x00000000 (absent/unpowered; QEMU raspi4b's hub-base read) → skip cleanly.
    //   * BUS-POISON — 0xdeadbeef / 0xffffffff open-bus/firmware fill, NOT a live register → skip
    //                  (fail-closed). This is the value that false-PASSED on metal.
    // BLOCK-DOWN and BUS-POISON both return BEFORE any core-register access, so neither can fault.
    // (The Device window is MMU-mapped by boot.rs, so an absent read is a bus/external abort from an
    // unbacked address, not a translation fault — only a real V3D backs 0xFEC04000.)
    match probe_hub_ident0() {
        V3dPresence::Up(v) => serial_println!(
            ":: V3D: probe verdict BLOCK-UP — hub IDENT0 = {:#010x} (live V3D identity) ::",
            v
        ),
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: probe verdict BLOCK-DOWN — hub IDENT0 = 0x00000000 (block absent/unpowered; expected in QEMU raspi4b) — GPU bring-up skipped, graceful degradation ::"
            );
            return;
        }
        V3dPresence::Poison(v) => {
            serial_println!(
                ":: V3D: probe verdict BUS-POISON — hub IDENT0 = {:#010x} (open-bus/firmware fill, NOT a live register — the powered+clocked path did not bring the block up) — GPU bring-up skipped, fail-closed ::",
                v
            );
            // SError-drain class fix: the powered/clocked/bridged sequence above wrote into a block
            // that never came up — any of those accesses may have left a LATENT async external
            // abort pending (the R22 sitting-2 first-tick SERROR). Drain before returning so the
            // fail-closed branch leaves the machine clean.
            super::exceptions::serror_drain_request("v3d: BUS-POISON probe");
            return;
        }
    }

    // Verdict BLOCK-UP → this is real silicon. Now the rest of the IDENT block + the core registers are
    // safe to read (they are backed on metal).
    let hub1 = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT1);
    let hub2 = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT2);
    let hub3 = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT3);
    let c0 = mmio_read(V3D_CORE0_BASE, V3D_CTL_IDENT0);
    let c1 = mmio_read(V3D_CORE0_BASE, V3D_CTL_IDENT1);
    let c2 = mmio_read(V3D_CORE0_BASE, V3D_CTL_IDENT2);
    serial_println!(
        ":: V3D: HUB_IDENT1..3 = {:#010x} {:#010x} {:#010x} ::",
        hub1, hub2, hub3
    );
    serial_println!(":: V3D: CTL_IDENT0..2 = {:#010x} {:#010x} {:#010x} ::", c0, c1, c2);

    // Decode the version from HUB_IDENT1's low nibbles (V3D-61 corrected map): major = TVER (3:0),
    // minor = REV (7:4), and the mainline single-number form is `tver * 10 + rev` — decimal 42 on a
    // Pi 4's V3D 4.2. Core count is bits 11:8, NOT the low nibble the pre-V3D-61 code read.
    let (ver, tver, rev, ncores, _, _) = v3d_ident_version(hub1, c0, c1);
    serial_println!(
        ":: V3D: PRESENT — tech version {}.{} (ver={}, expect V3D 4.2 = 42 on Pi 4); cores={} ::",
        tver,
        rev,
        ver,
        ncores
    );
    serial_println!(":: V3D: M1 probe PASS (powered, clocked, IDENT live) ::");

    // V3D-DEEP probe-budget note. Printed HERE — past the presence gate, before any job — so the wire
    // read always states which half of the battery this boot ran. Honesty over silence: a shorter log is
    // only trustworthy if it says what is missing from it.
    if V3D_DEEP {
        serial_println!(
            ":: V3D: [v3d] deep=on — slow banked-verdict probes ARMED: [v3d48] bisection ladder (6 rungs x ~0.5 s FLDONE backstop) + [v3d63] ptb-unit matrix (4 more rungs, same backstop) + [v3d64] muxcal (1 more rung + a 4 ms idle window + 1 extra CT1 job) + [v3d66] ptbsweep (2 passes x [1 rung + a 4 ms idle window + 1 extra CT1 job]) + [v3d68] binwall (4 more rungs, same backstop, plus a poison fill/scan of a 24 KiB tile-state region and the 32 KiB pool per rung) + [v3d69] initdiff (1 more rung, same backstop, between two READ-ONLY PM/ASB/MMUC fabric stations) + [v3d71] mainline-geom (1 more rung, same backstop, the bin window remapped to the piOS dump's iova geometry with in-flight ASB fabric + [v3d73] CLE-progress samplers across its wait) + [v3d73s] mainline-exact-submit (1 more rung, same backstop, the byte-exact v3d_bin_job_run write sequence with nothing interleaved) + [v3d74] thread-swap (1 more rung, same backstop + poison scans: the retiring M3 RCL submitted on CT0's queue with the [v3d73] sampler armed; leg B BCL-on-CT1 is a documented no-op) = 21 rungs, [v3d59] frameclose (64 x 1 ms), [v3d58] rerender (extra CT1 job). Expect ~10 s of extra boot — a stall of that length here is the BUDGET, not a hang ::"
        );
    } else {
        serial_println!(
            ":: V3D: [v3d] deep=off (banked verdicts skipped) — NOT run this boot: [v3d48] empty-frame bisection ladder (all 6 rungs banked non-retire), [v3d59] frameclose (banked DEAD-OPEN, zero bit changes), [v3d58] rerender (banked clean), [v3d71] mainline-geometry rung + fabric/[v3d73] CLE-progress samplers, [v3d73s] mainline-exact-submit rung, [v3d74] thread-swap rung. Fast probes only; re-arm with UNAOS_V3D_DEEP=1 ::"
        );
    }

    // PI-V3D-60: the identity read as an explicit boot-state CHECK (is this the 4.2 the whole packet
    // encoding targets?), then the post-reset half of the warm-handoff pair — the same registers the
    // pre-reset station sampled, diffed field by field, so the log states what our reset actually did.
    v3d60_ident(hub1, hub2, hub3, c0, c1, c2);
    v3d60_residue_post();

    // PI-V3D-51: the post-reset core-init the kernel runs at the tail of EVERY v3d_reset_v3d (via
    // v3d_init_hw_state → v3d_init_core): establish the L2T flush address window (L2TFLSTA=0,
    // L2TFLEND=~0). Our V3D-50 power-cycle reset left these at POR and nothing re-established them, so
    // the per-kick L2TCACTL FLM=FLUSH walked an unestablished range. Kernel order is reset → init_hw_state
    // → MMU reinit, so this sits between the reset/probe and M2. Core-relative — safe now the block is UP.
    v3d_init_hw_state();

    // ── M2: MMU. ────────────────────────────────────────────────────────────────────────────────
    if !program_mmu() {
        serial_println!(":: V3D: M2 MMU program FAILED — halting bring-up (fail-closed) ::");
        // The MMU-program writes are the R22 sitting-2 metal offender: they targeted a block whose
        // probe passed but whose MMU window aborted, leaving a latent SError for the first DAIF
        // unmask. Drain it here so the fail-closed exit leaves the machine clean.
        super::exceptions::serror_drain_request("v3d: M2 MMU program failed");
        return;
    }
    serial_println!(":: V3D: M2 MMU PASS (arena identity-mapped, confined, TLB flushed) ::");

    // PI-V3D-49: unmask the core interrupt working set once, kernel-faithfully (v3d_irq_enable), now the
    // block is powered and MMU-mapped and before any CT0/CT1 kick. FLDONE (the bin-retire signal every
    // kick's wait_fldone polls) had never been unmasked; every prior boot ran the frame at the mask's
    // power-on-reset value. See v3d.md §23 for why this is the empty-frame verdict's named frame-level fix.
    // PI-V3D-52 (Rung 1): `v3d_irq_enable` now also mirrors the HUB-INT half of the kernel probe (the
    // last unmirrored byte-exact divergence) — see v3d.md §26.
    v3d_irq_enable();

    // ── M3: clear job. ──────────────────────────────────────────────────────────────────────────
    let m3_pass = clear_job(fb);
    if m3_pass {
        serial_println!(":: V3D: M3 clear-job PASS (GPU cleared buffer; CPU byte-verified) ::");
    } else {
        serial_println!(":: V3D: M3 clear-job did not verify — see lines above ::");
    }
    // PI-V3D-58: latch the RENDER engine's verdict. This is the reference the `[v3d58] xengine` line is
    // drawn against — a render frame that opened, stored to arena memory and retired proves the block's
    // write path, MMU and clock are live, which is what makes the bin wall bin-EXCLUSIVE rather than a
    // global store failure. Every prior boot had this fact on the wire and never compared it to the bin.
    v3d58_note_render(m3_pass);

    // ── M4: the first triangle. Bin one triangle on CT0, render it on CT1 (implicit tile list), then
    // CPU-verify inside/outside samples. M3's PASS above is the regression witness — M4 runs AFTER it,
    // in its own arena regions, and never touches M3's buffers. ATTENDED-METAL-UNVERIFIED (QEMU raspi4b
    // never reaches here; on QEMU the run returned at BLOCK-DOWN far above). ────────────────────────
    let m4_pass = triangle_job(fb);

    // ── PI-V3D-11: the visible graphics battery (M5 gradient → M6 animated → M7 multi-primitive →
    // M8 blit-to-scanout). Purely ADDITIVE stages layered on the M4 scaffold: M3 + M4 above remain
    // the regression witnesses and none of their buffers or kick code is touched. PI-V3D-12: gated
    // on the M4 verdict — the ONLY battery gate. The stages reuse the M4 shaders/scaffold, so on an
    // M4 FAIL they can only bury the M4 witness in derivative noise; the boot the triangle lands,
    // the battery runs. ─────────────────────────────────────────────────────────────────────────
    if m4_pass {
        battery(fb);
        // PI-APP-1: the block is up, the MMU is programmed, and the visible battery just ran to
        // completion off `fb`. Latch that state so the `v3d` shell app can REPLAY the visible stages
        // on the live framebuffer while the system is up (the boot flash is too fast for the monitor
        // to catch). Replay reuses THIS initialized state — power/clock/PM-ASB/MMU stay enabled from
        // boot; only the per-stage jobs (which rebuild their own arena control lists idempotently) are
        // re-kicked. We store the exact FbTarget the boot battery used so replay is byte-for-byte the
        // same path, and do NOT re-enter `bringup` (which would re-power/re-clock/re-program the MMU).
        unsafe {
            V3D_REPLAY_FB = fb;
        }
        V3D_REPLAY_READY.store(true, Ordering::Release);
    } else {
        serial_println!(":: V3D: PI-V3D-11 battery SKIPPED — gated on the M4 triangle verdict (FAIL this boot) ::");
    }

    // Belt-and-suspenders for the whole bring-up: whatever path M2/M3/M4 took, no latent async abort
    // from a V3D register access may outlive this function (the SError-drain class rule).
    super::exceptions::serror_drain_request("v3d: bring-up exit");
}

/// The three discriminated outcomes of the V3D presence probe. Only `Up` proceeds past the gate.
enum V3dPresence {
    /// A live, non-poison hub identity word — real silicon with the block up.
    Up(u32),
    /// Hub IDENT0 reads 0x00000000 — block absent / unpowered (QEMU raspi4b's hub-base read).
    Down,
    /// Hub IDENT0 reads an open-bus / firmware-fill poison signature (`0xffffffff` / `0xdeadbeef`) —
    /// NOT a live register. Carries the offending word for the metal log.
    Poison(u32),
}

/// Open-bus / firmware-fill poison signatures on the BCM2711. NEITHER is ever live data:
///   * `0xffffffff` — the classic unbacked-read / all-ones bus return.
///   * `0xdeadbeef` — the VideoCore firmware's register/DRAM fill; the exact value the V3D core block
///     returned at the PI-V3D-1 attended sitting, which the old zero-only gate FALSE-PASSED as live.
/// (Mirrors `pcie_probe::is_poison`; kept local so the V3D lane owns its own liveness rule.)
#[inline]
fn is_poison(v: u32) -> bool {
    v == 0xFFFF_FFFF || v == 0xDEAD_BEEF
}

/// Poison-honest presence probe: read HUB IDENT0 and classify it into one of the three verdicts.
///
/// A freshly powered/clocked block can take a moment to answer, so a poison read is retried within a
/// short bounded settle window (never an unbounded spin) before it is called BUS-POISON — but a `0`
/// read is a definitive BLOCK-DOWN (the QEMU-absent / unpowered signature) and returns at once. Any
/// non-zero, non-poison word is a live identity → BLOCK-UP.
fn probe_hub_ident0() -> V3dPresence {
    // ~50 ms settle budget for a poison→live transition; finite off CNTPCT.
    let deadline = super::timer::cntpct() + super::timer::cntfrq() / 20;
    loop {
        let v = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT0);
        if v == 0x0000_0000 {
            return V3dPresence::Down;
        }
        if !is_poison(v) {
            return V3dPresence::Up(v);
        }
        if super::timer::cntpct() >= deadline {
            return V3dPresence::Poison(v);
        }
        core::hint::spin_loop();
    }
}

/// Busy-wait a bounded ~`ms` milliseconds off the free-running CNTPCT — a settling delay for a freshly
/// powered/clocked block before its first register read. Finite by construction (the anti-hang rule).
fn settle_ms(ms: u64) {
    let deadline = super::timer::cntpct() + (super::timer::cntfrq() * ms) / 1000;
    while super::timer::cntpct() < deadline {
        core::hint::spin_loop();
    }
}

/// PI-V3D-50: the kernel-faithful V3D core RESET CYCLE — the bring-up step the probe path performs and
/// we never did. Prior arcs (§10–23) proved every *per-job* register write byte-exact yet the empty bin
/// frame still never retires (BMACTIVE held set forever, INT_STS=0, FLDONE never fires). That wedge sits
/// BELOW per-job programming, in the bring-up/reset state the kernel driver inherits from its probe path.
///
/// The kernel's reset is `v3d_reset_v3d` → (BCM2711 has a reset-controller, so) `reset_control_reset(v3d->
/// reset)`. On the Pi 4 that reset id is `BCM2835_RESET_V3D`, and `bcm2835-pm`'s `bcm2835_reset_reset`
/// implements it as **power the GRAFX_V3D domain OFF, then back ON** — i.e. `bcm2835_asb_power_off` then
/// `bcm2835_asb_power_on`. That full OFF→ON cycle is what returns the V3D core (CLE/PTB/hub state machine)
/// to a clean reset state. UnaOS only ever ran the ON half ONCE (`enable_pm_asb`, PI-V3D-3), on a block the
/// firmware had already powered — so any stale internal state the firmware left (a bin pipeline that never
/// cleanly idled, a half-reset PTB) was never cleared. A block out of clean reset can fetch/consume a CL
/// (CLE walks, QPU runs to program-end) while the frame-accounting/flush unit sits in a half-reset state
/// that never latches FLDONE — exactly the P46 signature.
///
/// This mirrors `bcm2835_asb_power_off` (the OFF half) followed by our existing ON half:
///   OFF: stop the two async AXI bridges (set `ASB_REQ_STOP`, wait `ASB_ACK` to SET = quiesced) master
///        then slave, then assert the V3D reset (clear `PM_V3DRSTN` in `PM_GRAFX`). The BCM2711 SKIPs the
///        POWUP/MEMPD memory-power path (`if (power->rpivid_asb) return 0`), same as the ON half's note.
///   ON:  the existing `enable_pm_asb` — deassert `PM_V3DRSTN`, release both bridges (clear `ASB_REQ_STOP`,
///        wait `ASB_ACK` to CLEAR = released).
/// Every PM/ASB write carries the PM password. Best-effort/QEMU-safe: unbacked ASB regs read 0 (ACK never
/// sets → OFF bridge-stop backstop returns at once; ACK already clear → ON release returns at once), and
/// the IDENT0 probe downstream is the real verdict gate. `[v3d50]` before/after witnesses on every reg.
fn v3d_reset_cycle() {
    serial_println!(
        ":: V3D: [v3d50] core reset CYCLE — kernel `reset_control_reset(BCM2835_RESET_V3D)` = GRAFX_V3D power OFF then ON (bcm2835_asb_power_off → _on). Prior bring-up ran only the ON half once on a firmware-powered block; this OFF→ON returns the CLE/PTB/hub to clean reset ::"
    );
    // ── OFF half: `bcm2835_asb_power_off` (BCM2711 path) ──────────────────────────────────────────
    // (1) Stop the two async AXI bridges — request stopped and wait for the ACK to SET (bridge quiesced).
    //     Master first, then slave (Linux `bcm2835_asb_power_off` order is the reverse of power_on).
    let grafx_pre = mmio_read(PM_BASE, PM_GRAFX);
    serial_println!(
        ":: V3D: [v3d50] reset OFF — PM_GRAFX pre={:#010x} (PM_V3DRSTN={}) — stopping ASB bridges + asserting reset ::",
        grafx_pre, (grafx_pre & PM_V3DRSTN != 0) as u32
    );
    asb_stop("V3D master (ASB_V3D_M_CTRL)", ASB_V3D_M_CTRL);
    asb_stop("V3D slave  (ASB_V3D_S_CTRL)", ASB_V3D_S_CTRL);
    // (2) Assert the V3D reset: clear PM_V3DRSTN in PM_GRAFX (with the PM password), holding the core in
    //     reset while the bridges are stopped.
    let grafx_off = mmio_read(PM_BASE, PM_GRAFX);
    mmio_write(PM_BASE, PM_GRAFX, PM_PASSWORD | (grafx_off & !PM_V3DRSTN));
    let grafx_asserted = mmio_read(PM_BASE, PM_GRAFX);
    serial_println!(
        ":: V3D: [v3d50] reset OFF — PM_GRAFX assert V3DRSTN(clear bit6): pre={:#010x} post={:#010x} (PM_V3DRSTN now {}){} ::",
        grafx_off, grafx_asserted, (grafx_asserted & PM_V3DRSTN != 0) as u32,
        if is_poison(grafx_asserted) { " (poison/absent — QEMU or block-down)" } else { "" }
    );
    // Hold the core in reset briefly (bounded off CNTPCT) before releasing — the reset must be observed.
    settle_ms(1);

    // ── ON half: `bcm2835_asb_power_on` (deassert reset, release bridges) — the existing PI-V3D-3 step ──
    enable_pm_asb();
}

/// PI-V3D-50: stop one V3D async AXI bridge (the OFF-half of the reset cycle) — mirror
/// `bcm2835_asb_enable`'s stop path: set `ASB_REQ_STOP` (with the PM password) and wait, bounded, for
/// `ASB_ACK` to SET (the bridge acknowledging it has quiesced). The counterpart of `asb_release`.
/// Announced-before-issue; poison-honest readback; never fatal (a bridge that will not ACK is logged and
/// the reset proceeds — the ON half + IDENT0 probe deliver the honest verdict). QEMU-safe: unbacked reg
/// reads 0, ACK never sets, the backstop returns false at once and we proceed.
fn asb_stop(what: &str, reg: usize) {
    let cur = mmio_read(RPIVID_ASB_BASE, reg);
    serial_println!(
        ":: V3D: [v3d50] reset OFF — ASB stop {} — cur {:#010x} -> set ASB_REQ_STOP (pw), wait ACK SET ::",
        what, cur
    );
    mmio_write(RPIVID_ASB_BASE, reg, PM_PASSWORD | (cur | ASB_REQ_STOP));
    let stopped = wait_bit_set(RPIVID_ASB_BASE, reg, ASB_ACK, what);
    let rb = mmio_read(RPIVID_ASB_BASE, reg);
    serial_println!(
        ":: V3D: [v3d50] reset OFF — ASB {} readback {:#010x} — {}{} ::",
        what, rb,
        if stopped { "ACK set (bridge stopped)" } else { "ACK never set (backstop hit — proceeding)" },
        if is_poison(rb) { ", poison/absent (QEMU or block-down)" } else { "" }
    );
}

/// PI-V3D-3: the PM / ASB enable step (the ON half of the PI-V3D-50 reset cycle). On BCM2711 the firmware
/// property-tag power+clock path leaves the V3D held in reset with its async AXI bridges stopped
/// (PI-V3D-2 metal: powered+clocked yet 0xdeadbeef). Mirror the two BCM2711-relevant steps of Linux
/// `bcm2835_asb_power_on` for the GRAFX_V3D domain: (1) deassert PM_V3DRSTN in PM_GRAFX, (2) release
/// ASB_V3D_M_CTRL then ASB_V3D_S_CTRL. Every PM/ASB write carries the PM password. Best-effort: a bridge
/// that never ACKs (or reads poison) is logged and we proceed — the IDENT0 probe that follows is the real
/// verdict gate (it BUS-POISONs honestly if the block still did not decode). Announced-before-issue
/// writes, poison-honest readbacks, bounded settles — nothing here can fault or hang (QEMU-safe).
fn enable_pm_asb() {
    // (1) Deassert the V3D reset in PM_GRAFX (bit PM_V3DRSTN), preserving the other bits, PM password
    // in the top byte. Read-modify-write via the Device window; the read is poison-tolerant (we only
    // OR in our bit and re-stamp the password, so any bus value is harmless).
    let grafx = mmio_read(PM_BASE, PM_GRAFX);
    serial_println!(
        ":: V3D: PM/ASB deassert V3D reset — PM_GRAFX {:#010x} -> set PM_V3DRSTN (pw) ::",
        grafx
    );
    mmio_write(PM_BASE, PM_GRAFX, PM_PASSWORD | (grafx | PM_V3DRSTN));
    let grafx_rb = mmio_read(PM_BASE, PM_GRAFX);
    serial_println!(
        ":: V3D: PM_GRAFX readback {:#010x}{} ::",
        grafx_rb,
        if is_poison(grafx_rb) { " (poison/absent — QEMU or block-down)" } else { "" }
    );

    // (2) Release the two async AXI bridges: master first, then slave (Linux order). Clear ASB_REQ_STOP
    // and wait for ASB_ACK to clear, bounded.
    asb_release("V3D master (ASB_V3D_M_CTRL)", ASB_V3D_M_CTRL);
    asb_release("V3D slave  (ASB_V3D_S_CTRL)", ASB_V3D_S_CTRL);
}

/// Release one V3D async AXI bridge in the rpivid_asb block: clear ASB_REQ_STOP (with the PM password)
/// and wait, with a finite CNTPCT backstop, for ASB_ACK to clear. Announced-before-issue; poison-honest
/// readback. Never fatal — a bridge that will not release is logged and the caller proceeds to let the
/// IDENT0 probe deliver the honest verdict.
fn asb_release(what: &str, reg: usize) {
    let cur = mmio_read(RPIVID_ASB_BASE, reg);
    serial_println!(
        ":: V3D: PM/ASB release {} — cur {:#010x} -> clear ASB_REQ_STOP (pw) ::",
        what, cur
    );
    mmio_write(RPIVID_ASB_BASE, reg, PM_PASSWORD | (cur & !ASB_REQ_STOP));
    // Wait ~5 ms for ACK to clear (Linux uses 1 µs on real silicon; we are generous). On QEMU the
    // register is unbacked/reads 0, so ACK is already clear and this returns at once.
    let released = wait_bit_clear(RPIVID_ASB_BASE, reg, ASB_ACK, what);
    let rb = mmio_read(RPIVID_ASB_BASE, reg);
    serial_println!(
        ":: V3D: PM/ASB {} readback {:#010x} — {}{} ::",
        what,
        rb,
        if released { "ACK clear (bridge released)" } else { "ACK still set (backstop hit — proceeding)" },
        if is_poison(rb) { ", poison/absent (QEMU or block-down)" } else { "" }
    );
}

/// M2: build a flat V3D page table that identity-maps ONLY the arena (every other PTE invalid), then
/// program + enable the V3D MMU and flush its TLB. Returns false (fail-closed) if the arena would not
/// fit the table or the TLB-clear never settles. Confinement is the review-lens property: the GPU can
/// reach the arena and nothing else.
fn program_mmu() -> bool {
    let base = arena_phys();
    let end = base + ARENA_BYTES;
    let top_page = (end + PAGE - 1) >> V3D_MMU_PAGE_SHIFT; // number of PTEs needed to index the arena top
    if top_page > PT_CAP {
        serial_println!(
            ":: V3D: arena top page {} exceeds page-table capacity {} — cannot map (fail-closed) ::",
            top_page, PT_CAP
        );
        return false;
    }
    debug_assert!(base % PAGE == 0, "arena not page-aligned");

    // Fill: invalidate everything up to the arena top, then mark ONLY the arena's own pages valid
    // (identity — pte page-number == phys page-number). Bounded by PT_CAP throughout.
    let pt = &raw mut V3D_PT;
    unsafe {
        for i in 0..top_page {
            (*pt).ptes[i] = 0; // invalid
        }
        let first = base >> V3D_MMU_PAGE_SHIFT;
        for p in 0..ARENA_PAGES {
            let pfn = first + p;
            // pfn indexes within [0, top_page) ⊆ [0, PT_CAP) by construction.
            (*pt).ptes[pfn] = V3D_PTE_VALID | V3D_PTE_WRITEABLE | (pfn as u32);
        }
    }
    // Publish the table to RAM: the V3D reads it directly (non-coherent). Clean the FULL table
    // (PT_CAP entries, 32 KiB — cheap), not just the used prefix: the tail PTEs [top_page, PT_CAP)
    // are our invalidation barrier, and their zero-init could otherwise sit un-published in the
    // D-cache while the V3D read stale DRAM there — a stray out-of-arena iova must hit a PUBLISHED
    // zero (fault) and never a garbage word with the VALID bit set. (Lens should-fix: this makes the
    // "every other PTE invalid" confinement invariant hold unconditionally.)
    cache::clean_range(pt_phys(), PT_CAP * 4);

    // Program the MMU: table base (in pages), fault-abort policy, illegal-address catcher, enable +
    // flush. Sequence per v3d_mmu.c::v3d_mmu_set_page_table + v3d_mmu_flush_all.
    let pt_base_pages = (pt_phys() >> V3D_MMU_PAGE_SHIFT) as u32;
    // PI-V3D-62 gap (2) CLOSED: arm the fault-REPORTING halves alongside the aborts. Mainline's
    // `v3d_mmu_set_page_table` writes ENABLE | PT_INVALID_{ENABLE,ABORT,INT} | WRITE_VIOLATION_{ABORT,INT}
    // | CAP_EXCEEDED_{ABORT,INT}; UnaOS wrote the ABORT halves only, so a refused access aborted SILENTLY
    // — no hub interrupt latched, and the campaign's whole instrument set was blind to it. The address-cap
    // pair joins the set for the same reason: an over-cap access was previously neither aborted nor
    // reported. This changes fault REPORTING only; it grants the GPU no address it could not already
    // reach, so the arena-only confinement invariant is untouched.
    let ctl_want = V3D_MMU_CTL_ENABLE
        | V3D_MMU_CTL_PT_INVALID_ENABLE
        | V3D_MMU_CTL_PT_INVALID_ABORT
        | V3D_MMU_CTL_PT_INVALID_INT
        | V3D_MMU_CTL_WRITE_VIOLATION_ABORT
        | V3D_MMU_CTL_WRITE_VIOLATION_INT
        | V3D_MMU_CTL_CAP_EXCEEDED_ABORT
        | V3D_MMU_CTL_CAP_EXCEEDED_INT;
    // PI-V3D-62 gap (1) CLOSED: aim the illegal-address catcher at the DEDICATED scratch page instead of
    // arena page 0. ILLEGAL_ADDR takes a PHYSICAL page number (mainline passes `mmu_scratch_paddr >>
    // V3D_MMU_PAGE_SHIFT`), and the page it names must belong to no job and be mapped by nothing — see
    // V3D_MMU_SCRATCH. Aimed into the arena, an illegal access was indistinguishable, at the memory it
    // landed on, from a legal one; aimed here, the landing site is unambiguous and observable.
    let scratch = mmu_scratch_phys();
    let scratch_pfn = scratch >> V3D_MMU_PAGE_SHIFT;
    let illegal_want = (scratch_pfn as u32) | V3D_MMU_ILLEGAL_ADDR_ENABLE;
    let ctl_before = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let illegal_before = mmio_read(V3D_HUB_BASE, V3D_MMU_ILLEGAL_ADDR);
    serial_println!(
        ":: V3D: MMU program — PT_PA_BASE<={:#010x} (pt@{:#x}) CTL<={:#010x} ILLEGAL_ADDR<={:#010x} ::",
        pt_base_pages, pt_phys(), ctl_want, illegal_want
    );
    mmio_write(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE, pt_base_pages);
    mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, ctl_want);
    mmio_write(V3D_HUB_BASE, V3D_MMU_ILLEGAL_ADDR, illegal_want);
    dsb();

    // [v3d62] the two closed gaps, stated as programmed values with their before/after — the line the
    // next boot is read for. The scratch page's own PTE is proved invalid rather than assumed: it is a
    // separate static from the arena, so the fill loop above either zeroed its slot (pfn < top_page) or
    // never touched it (pfn >= top_page, where the table's zero-init stands and is published by the
    // full-table clean). Either way the entry must read 0; a non-zero reading would mean the scratch page
    // is reachable through translation and the catcher has been aimed at mapped memory again.
    let scratch_pte = if scratch_pfn < PT_CAP {
        unsafe { (&raw const V3D_PT).cast::<u32>().add(scratch_pfn).read_volatile() }
    } else {
        0 // beyond the table entirely — unmapped by construction, nothing to read
    };
    let scratch_in_arena = scratch_pfn >= (base >> V3D_MMU_PAGE_SHIFT)
        && scratch_pfn < (base >> V3D_MMU_PAGE_SHIFT) + ARENA_PAGES;
    let ctl_after = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    serial_println!(
        ":: V3D: [v3d62] mmufix — ILLEGAL_ADDR scratch page PA={:#x} (pfn={:#x}) PTE={:#010x} unmapped={} in-arena={} | ILLEGAL_ADDR {:#010x}->{:#010x} (was arena page {:#x}) | MMU_CTL {:#010x}->{:#010x} (wrote {:#010x}: PT_INVALID_INT={} WRITE_VIOLATION_INT={} CAP_EXCEEDED_ABORT={} CAP_EXCEEDED_INT={} newly armed vs the abort-only policy) — {} ::",
        scratch, scratch_pfn, scratch_pte, (scratch_pte == 0) as u32, scratch_in_arena as u32,
        illegal_before, mmio_read(V3D_HUB_BASE, V3D_MMU_ILLEGAL_ADDR),
        base >> V3D_MMU_PAGE_SHIFT,
        ctl_before, ctl_after, ctl_want,
        (ctl_after & V3D_MMU_CTL_PT_INVALID_INT != 0) as u32,
        (ctl_after & V3D_MMU_CTL_WRITE_VIOLATION_INT != 0) as u32,
        (ctl_after & V3D_MMU_CTL_CAP_EXCEEDED_ABORT != 0) as u32,
        (ctl_after & V3D_MMU_CTL_CAP_EXCEEDED_INT != 0) as u32,
        if scratch_pte != 0 || scratch_in_arena {
            "SCRATCH PAGE IS NOT ISOLATED — it is mapped, or it lies inside the arena. The catcher is aimed at memory a job can reach and the v3d62 instrument is NOT trustworthy this boot; treat any scratch-page witness below as inconclusive"
        } else if ctl_after & ctl_want != ctl_want {
            "MMU_CTL DID NOT ECHO the fault-reporting policy. The block accepted the write only in part (or reads back stale): the interrupt halves may not be armed, so an absence of reported faults below proves nothing"
        } else {
            "both V3D-60 standing gaps are closed: the illegal-address catcher now lands on an unmapped page belonging to no job, and the MMU is armed to REPORT (interrupt) as well as abort on page-table-invalid, write-violation and address-cap. A refused PTB write can no longer vanish unwitnessed"
        }
    );

    // Flush the MMU cache + TLB. Finite backstop on the TLB-clearing bit (never an unbounded spin).
    mmio_write(V3D_HUB_BASE, V3D_MMUC_CONTROL, V3D_MMUC_CONTROL_FLUSH | V3D_MMUC_CONTROL_ENABLE);
    mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, mmio_read(V3D_HUB_BASE, V3D_MMU_CTL) | V3D_MMU_CTL_TLB_CLEAR);
    if !wait_bit_clear(V3D_HUB_BASE, V3D_MMU_CTL, V3D_MMU_CTL_TLB_CLEARING, "MMU TLB clear") {
        return false;
    }

    // Ensure the programming writes have landed at the hub before we read the state back.
    dsb();

    // Verify: MMU reports enabled (CTL.ENABLE=bit0 latched), no violation address latched. The
    // readback is now against the CORRECT offsets/bits — a live block echoes ENABLE|PT_INVALID_ENABLE|
    // aborts; the all-zero readback that fail-closed on metal was the fabricated-constants bug.
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let ptb = mmio_read(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE);
    let vio = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
    let dbg = mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO);
    let enabled = ctl & V3D_MMU_CTL_ENABLE != 0;
    serial_println!(
        ":: V3D: MMU readback CTL={:#010x} (ENABLE={}) PT_PA_BASE={:#010x} VIO_ADDR={:#010x} DEBUG={:#010x} (mapped {} arena pages @ {:#x}) ::",
        ctl, enabled as u32, ptb, vio, dbg, ARENA_PAGES, base
    );
    enabled
}

/// M3: build a minimal render control list (RCL) that clears the tile buffer to CLEAR_RGBA and stores
/// it into the target buffer, kick CT1, poll to completion with a finite backstop, then have the CPU
/// byte-verify the target. On success, blit the target into the panel framebuffer (metal witness).
///
/// The RCL is a two-level render-only list (main + generic per-tile sub-list, no binner/shaders) per
/// Mesa v3d_packet_v33.xml 4.2 encodings + v3dx_rcl.c ordering — see `build_rcl`.
/// ATTENDED-METAL-UNVERIFIED: QEMU never runs this.
fn clear_job(fb: Option<FbTarget>) -> bool {
    // Pre-seed the target with a sentinel DIFFERENT from the clear colour, so a passing verify proves
    // the GPU actually wrote (not a lucky pre-existing pattern).
    fill_target(0xDEAD_BEEF);

    let (rcl_len, sublist_len) = build_rcl();
    // Publish the target (sentinel) + BOTH control lists to RAM for the non-coherent GPU. The main
    // list is what CT1 fetches; the generic per-tile sub-list is branched to per supertile, so it must
    // be published too (PI-V3D-6: the store lives in the sub-list — an unpublished sub-list is exactly
    // the "CLE ran, store landed nowhere" class-B failure this arc fixes).
    cache::clean_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
    cache::clean_range(arena_phys() + OFF_RCL, rcl_len);
    cache::clean_range(arena_phys() + OFF_SUBLIST, sublist_len);

    // Kick CT1 (render queue): begin address .. end address. Both are arena-internal identity iovas,
    // bounds-checked here — the memory-safety guarantee for what the CLE fetches.
    let ba = arena_phys() + OFF_RCL;
    let ea = ba + rcl_len;
    if !arena_contains(ba, rcl_len) {
        serial_println!(":: V3D: RCL range escapes the arena — refusing kick (fail-closed) ::");
        return false;
    }
    // PI-V3D-5 job-never-ran witness (class A): snapshot the CLE status the instant BEFORE the kick,
    // so the post-kick reads have a baseline. CTRUN clearing could mean "finished" OR "never started";
    // only a before/after pair disambiguates.
    let cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let ca_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
    // Order matters: program the queue-BEGIN address first, then writing the queue-END address is the
    // CLE's GO trigger (v3d_regs.h / the kernel v3d_gem submit path: CT1QBA then CT1QEA). With the
    // offsets now correct, this QEA write is what actually starts CT1.
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QBA, ba as u32);
    dsb(); // BA must be latched before the EA write triggers the fetch
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QEA, ea as u32);
    dsb(); // ensure the GO (QEA) write reaches the CLE before we sample its status
    // Tight kick witness: sample CT1CS + CT1CA the instant after the GO write. A started CLE latches
    // CTRUN here and CT1CA leaves 0/BA to walk the list; a never-started CLE shows CTRUN=0 and CT1CA
    // unchanged from ca_pre. This pair is the boot-P4 discriminator's ground truth.
    let cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let ca_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);

    // Poll for CT1 idle (CTRUN clears when the list finishes) with a finite ~500 ms backstop.
    let idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT1CS, V3D_CLE_CT1CS_CTRUN, "CT1 render");

    // PI-V3D-5 two-class witness block. Read the CLE progress + the V3D MMU fault status BEFORE the
    // verify, so the metal log tells job-never-ran (class A) from job-ran-but-wrote-elsewhere/faulted
    // (class B) regardless of what the verify then reports. All reads; nothing here is programmed.
    let cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let ct0cs = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct1ca = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
    let mmu_ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let vio_addr = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
    let vio_id = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ID);
    let mmu_fault = mmu_ctl
        & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    // PI-V3D-7 discriminator fix. The old `ran` test used `ct1ca != BA` as proof of execution — but a
    // CLE that NEVER STARTED also satisfies that, because its CT1CA reads 0 (≠ the non-zero BA). That
    // false-positive mislabeled boot-P3's never-started CLE as CLASS-B RAN-NO-FAULT. Correct truth
    // table: the CLE ran ONLY if we ever OBSERVED CTRUN set (in any of pre/kicked/done) OR CT1CA
    // actually ADVANCED — i.e. it points INTO the list range (BA, EA] rather than sitting at 0 or BA.
    // A never-started CLE has CTRUN never seen AND CT1CA at 0/BA → CLASS-A.
    let ctrun_ever = (cs_pre | cs_kicked | cs_done) & V3D_CLE_CT1CS_CTRUN != 0;
    let ct1ca_advanced =
        ct1ca != 0 && ct1ca != ba as u32 && ct1ca >= ba as u32 && ct1ca <= ea as u32;
    let ran = ctrun_ever || ct1ca_advanced;
    // PI-V3D-8 mislabel fix. The old "RAN-NO-FAULT" else-branch asserted "store landed off-target"
    // UNCONDITIONALLY — so even a SUCCESSFUL run (CLE ran, no fault, store correct, verify passes) was
    // clued as a class-B failure. Do the verify FIRST (only meaningful once the CLE has idled) and let
    // its result pick the label: a verified store is RAN-OK, an unverified one is the genuine class-B
    // off-target case. The old code did the invalidate+verify AFTER this block; it now lives here and
    // the later return simply reuses the result.
    let verified = if idled {
        cache::clean_invalidate_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
        Some(verify_target(CLEAR_RGBA))
    } else {
        None
    };
    let class = if mmu_fault != 0 {
        "CLASS-B MMU-FAULT (store faulted in the V3D MMU — job wrote nowhere)"
    } else if !ran {
        "CLASS-A JOB-NEVER-RAN (CTRUN never observed AND CT1CA never advanced from 0/BA — CLE did not start)"
    } else if !idled {
        "INDETERMINATE (CLE started but CTRUN never cleared — backstop hit)"
    } else if verified == Some(true) {
        "RAN-OK (CLE executed, no MMU fault, store byte-verified)"
    } else {
        "CLASS-B RAN-NO-FAULT (CLE executed with no MMU fault — store landed off-target: RCL encoding)"
    };
    serial_println!(
        ":: V3D: M3 clue — CT1CS pre={:#010x} kicked={:#010x} done={:#010x} CT1CA pre={:#010x} kicked={:#010x} done={:#010x} CT0CS={:#010x} (BA={:#010x} EA={:#010x}) ran={} — {} ::",
        cs_pre, cs_kicked, cs_done, ca_pre, ca_kicked, ct1ca, ct0cs, ba as u32, ea as u32, ran as u32, class
    );
    serial_println!(
        ":: V3D: M3 clue — MMU_CTL={:#010x} (PT_INVALID={} WRITE_VIOLATION={} CAP_EXCEEDED={}) VIO_ADDR={:#010x} VIO_ID={:#010x} ::",
        mmu_ctl,
        (mmu_ctl & V3D_MMU_CTL_PT_INVALID != 0) as u32,
        (mmu_ctl & V3D_MMU_CTL_WRITE_VIOLATION != 0) as u32,
        (mmu_ctl & V3D_MMU_CTL_CAP_EXCEEDED != 0) as u32,
        vio_addr, vio_id
    );
    // SError-drain correlation witness: a V3D store that faulted on the bus can leave a latent async
    // external abort that the global SError-drain would otherwise consume silently at bring-up exit
    // (or, worse, at the first timer tick). Drain it HERE, labelled to this exact kick→poll window, so
    // the "consumed N latent async abort(s)" line — if any — is unambiguously correlated with the M3
    // clear-job store, not with M1/M2. Zero drained here = the store did not raise a bus fault.
    super::exceptions::serror_drain_request("v3d: M3 clear-job kick window");

    if !idled {
        serial_println!(":: V3D: CT1 did not idle within budget — no verify (anti-hang backstop hit) ::");
        return false;
    }

    // The GPU's writes were already read back and byte-verified above (the `verified` snapshot the
    // clue label used — the clean_invalidate there is what forces DRAM truth, defeating a stale-CPU-line
    // false negative). Reuse it; on success blit the target to the panel (metal visible witness).
    let ok = verified == Some(true);
    if ok {
        if let Some(fb) = fb {
            blit_target(&fb);
        }
    }
    ok
}

// ─── V3D 4.2 (BCM2711) control-list packet encodings ──────────────────────────────────────────────
// PI-V3D-6: the placeholder that boot-P2 convicted (CLASS-B RAN-NO-FAULT) wrote a 0x1a-byte stream of
// bare opcode bytes with NO field packing, several WRONG opcodes (114 for "clear colors" — actually
// Blend Enables; 125 for "end-of-tile" — actually Tile Coordinates Implicit), a STORE with the target
// address at the wrong byte offset and no format/stride/buffer fields, and — fatally — NO
// SUPERTILE_COORDINATES, so nothing ever triggered a tile store. The CLE happily ran the malformed
// bytes to completion (no MMU fault) and wrote nowhere. This is the correct encoding.
//
// All opcodes, field bit-positions, sizes, enum values and packet lengths below are transcribed
// verbatim from Mesa `src/broadcom/cle/v3d_packet_v33.xml` (`gen="3.3" max_ver="42"`, the V3D 4.2
// variants) and the emission ORDER follows Mesa `src/gallium/drivers/v3d/v3dx_rcl.c`
// (`v3dX(emit_rcl)` + `emit_render_layer` + `v3d_rcl_emit_generic_per_tile_list`). Mesa is
// MIT-licensed — verbatim-liftable WITH attribution (memory: unaos-license-gplv3). No Linux-kernel
// (GPL-2.0-only) v3d source is used here.
//
// Packing convention (Mesa `gen_pack_header.py`): byte 0 is the opcode; every XML `start` bit is
// relative to the bit AFTER the opcode, i.e. absolute packet bit = XML start + 8. Packet length =
// max(field end bit)/8 + 1 bytes. `set_bits` writes a field LSB-first at its absolute bit.

// Packet opcodes (v3d_packet_v33.xml `code=`).
const P_TRMC: u8 = 121; // Tile Rendering Mode Cfg (sub-id field selects Common/Color/Clear/ZS variant)
const P_TILE_COORDINATES: u8 = 124;
const P_TILE_COORDINATES_IMPLICIT: u8 = 125;
const P_STORE_TILE_BUFFER_GENERAL: u8 = 29;
const P_CLEAR_TILE_BUFFERS: u8 = 25;
const P_END_OF_LOADS: u8 = 26;
const P_END_OF_TILE_MARKER: u8 = 27;
const P_FLUSH_VCD_CACHE: u8 = 19;
const P_GENERIC_TILE_LIST: u8 = 20; // Start Address of Generic Tile List
const P_RETURN_FROM_SUB_LIST: u8 = 18;
const P_PRIM_LIST_FORMAT: u8 = 56;
const P_SET_INSTANCEID: u8 = 54;
const P_TILE_LIST_INITIAL_BLOCK_SIZE: u8 = 126;
const P_MULTICORE_TILE_LIST_BASE: u8 = 123; // Multicore Rendering Tile List Set Base
const P_MULTICORE_SUPERTILE_CFG: u8 = 122;
const P_SUPERTILE_COORDINATES: u8 = 23;
// PI-V3D-10 boot-P6 root cause #2 (the render-kick gate): this constant was 0 — the "Halt" opcode —
// mislabeled as Mesa's END_OF_RENDERING. In v3d_packet.xml they are DISTINCT packets: code 0 = Halt,
// code 13 = "End of rendering" (shortname end_render), and BOTH v3dx_rcl.c (gallium) and
// v3dvx_cmd_buffer.c (v3dv) terminate every RCL with END_OF_RENDERING, never Halt. The difference is
// load-bearing for the QUEUED kick path (CTnQBA/QEA): END_OF_RENDERING completes the FRAME (the CLE
// returns to idle and the next queued CT1 job may dispatch), while Halt merely stops the CLE with the
// frame still open. M3's Halt-terminated list therefore "passed" (its store had already landed) but
// left CT1 wedged in the halted frame — the exact boot-P6 signature: M4's CT1QEA write was accepted,
// CTRUN never set, CT1CA parked at M3's end (0x001f806a). Same fabricated-value class as PI-V3D-4/-7.
const P_END_OF_RENDERING: u8 = 13; // "End of rendering" (v3d_packet.xml code 13) — NOT Halt (0)

// TILE_RENDERING_MODE_CFG sub-ids (v42 `sub-id` field defaults).
const TRMC_SUBID_COMMON: u64 = 0;
const TRMC_SUBID_COLOR: u64 = 1;
const TRMC_SUBID_ZS_CLEAR_VALUES: u64 = 2;
const TRMC_SUBID_CLEAR_COLORS_PART1: u64 = 3;

// Internal-format enum values (v3d_packet_v33.xml enums). rgba8 unorm render target: 32-bit internal
// BPP (Internal BPP "32" = 0), internal type "8" = 2; stored Output Image Format rgba8 = 27.
const INTERNAL_BPP_32: u64 = 0;
const INTERNAL_TYPE_8: u64 = 2;
const OUTPUT_IMAGE_FORMAT_RGBA8: u64 = 27;
const MEMORY_FORMAT_RASTER: u64 = 0;
const PRIM_TYPE_LIST_TRIANGLES: u64 = 2;
// Tile-allocation block-size enum (v3d_packet.xml, shared by TILE_BINNING_MODE_CFG and
// TILE_LIST_INITIAL_BLOCK_SIZE): 64b = 0, 128b = 1, 256b = 2. PI-V3D-14: Mesa's ONLY exercised
// config on silicon is 128B initial + 64B overflow — v3d_limits.h defines
// V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE 128 / V3D_TILE_ALLOC_OVERFLOW_BLOCK_SIZE 64 with
// enum = (size >> 7), and v3d_util.c STATIC_ASSERTs the initial size == 128. Both emitters
// (v3dvx_cmd_buffer.c job_emit_binning_prolog + cmd_buffer_render_pass_setup_render_pass_rcl,
// v3dx_draw.c/v3dx_rcl.c on the GL side) use INITIAL(=1) in the bin config's initial-block field
// AND in the render list's TILE_LIST_INITIAL_BLOCK_SIZE ("needs to match the value from binning
// mode config"), and OVERFLOW(=0) only in the bin config's (overflow) block-size field.
const TILE_ALLOC_BLOCK_SIZE_64B: u64 = 0;
const TILE_ALLOC_BLOCK_SIZE_128B: u64 = 1;

/// Build BOTH control lists (main at OFF_RCL, generic per-tile sub-list at OFF_SUBLIST) and publish the
/// sub-list to RAM for the non-coherent GPU. Returns `(main_len, sublist_len)` in bytes; the caller
/// kicks CT1 over `[OFF_RCL, OFF_RCL+main_len)` and has already published the main list + target.
///
/// Shape (single 64×64 tile = single supertile, no binned geometry — a pure clear+store), per Mesa
/// `v3dX(emit_rcl)`. ATTENDED-METAL-UNVERIFIED: QEMU raspi4b models no V3D, so this is
/// correct-by-construction against the cited Mesa sources, refined at the attended sitting.
fn build_rcl() -> (usize, usize) {
    let target = (arena_phys() + OFF_TARGET) as u32;
    let sublist_start = (arena_phys() + OFF_SUBLIST) as u32;
    let tile_alloc = (arena_phys() + OFF_TILEALLOC) as u32;
    let stride = (TARGET_W * TARGET_BPP) as u64; // raster row stride in bytes (64 px × 4 B = 256)

    // ── Generic per-tile sub-list (OFF_SUBLIST) — executed once per SUPERTILE_COORDINATES. Carries the
    // real tile-buffer STORE. Order per v3d_rcl_emit_generic_per_tile_list (V3D >= 41 path). We omit
    // BRANCH_TO_IMPLICIT_TILE_LIST: there is no binned geometry, so no implicit tile list to run. ──
    let mut s = RclWriter::new(OFF_SUBLIST);
    s.pkt(Pkt::new(P_TILE_COORDINATES_IMPLICIT, 1).done()); // single coords; END_OF_LOADS flips load→render
    s.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
    // PTB assumes triangles as the initial primitive mode; SET_INSTANCEID(0) — hw does not default it.
    s.pkt(Pkt::new(P_PRIM_LIST_FORMAT, 2).f(0, 6, PRIM_TYPE_LIST_TRIANGLES).done());
    s.pkt(Pkt::new(P_SET_INSTANCEID, 5).f(0, 32, 0).done());
    // STORE_TILE_BUFFER_GENERAL: RT0 → target, raster, rgba8, row stride. Address is a full 32-bit
    // field at XML start 64 (packet byte 9) — the exact slot the placeholder missed. This is the write
    // the CPU verifies.
    s.pkt(
        Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13)
            .f(0, 4, 0) // Buffer to Store = Render target 0
            .f(4, 3, MEMORY_FORMAT_RASTER)
            .f(12, 6, OUTPUT_IMAGE_FORMAT_RGBA8)
            .f(28, 20, stride) // Height in UB or Stride (raster → byte stride)
            .f(64, 32, target as u64) // Address
            .done(),
    );
    // GFXH-1461/1689: after the per-buffer store, clear the tile buffers (job->clear set).
    s.pkt(
        Pkt::new(P_CLEAR_TILE_BUFFERS, 2)
            .f(0, 1, 1) // Clear all Render Targets
            .f(1, 1, 1) // Clear Z/Stencil Buffer
            .done(),
    );
    s.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    s.pkt(Pkt::new(P_RETURN_FROM_SUB_LIST, 1).done());
    let sublist_len = s.len();
    let sublist_end = sublist_start + sublist_len as u32;
    // Publish the sub-list now (the main list + target are published by the caller).
    cache::clean_range(arena_phys() + OFF_SUBLIST, sublist_len);

    // ── Main render control list (OFF_RCL) — what CT1 executes. Frame config first (COMMON must be the
    // first TILE_RENDERING_MODE_CFG, ZS_CLEAR_VALUES last), then the per-layer render. ──
    let mut w = RclWriter::new(OFF_RCL);

    // TILE_RENDERING_MODE_CFG (Common): 64×64 frame, 1 render target (minus_one → 0), 32-bit max BPP,
    // no MSAA, no double-buffer, Early-Z LT/LE, depth type 0.
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COMMON)
            .f(4, 4, 0) // Number of Render Targets (minus_one: 1 RT → 0)
            .f(8, 16, TARGET_W as u64) // Image Width (pixels)
            .f(24, 16, TARGET_H as u64) // Image Height (pixels)
            .f(40, 2, INTERNAL_BPP_32) // Maximum BPP of all render targets
            .done(),
    );
    // TILE_RENDERING_MODE_CFG (Clear Colors Part1): RT0 clear value low 32 bits = CLEAR_RGBA. For a
    // 32-bit-BPP target only Part1 is needed (Mesa emits Part2/Part3 only for >= 64/128-bit BPP).
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_CLEAR_COLORS_PART1)
            .f(4, 4, 0) // Render Target number
            .f(8, 32, CLEAR_RGBA as u64) // Clear Color low 32 bits
            .done(),
    );
    // TILE_RENDERING_MODE_CFG (Color, v42): RT0 = 32-bit BPP, internal type "8" (rgba8 unorm), no clamp.
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COLOR)
            .f(4, 2, INTERNAL_BPP_32) // Render Target 0 Internal BPP
            .f(6, 4, INTERNAL_TYPE_8) // Render Target 0 Internal Type
            .f(10, 2, 0) // Render Target 0 Clamp = none
            .done(),
    );
    // TILE_RENDERING_MODE_CFG (ZS Clear Values) — ends the rendering-mode config. No Z/S buffer; clear
    // values are inert but the packet must terminate config.
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_ZS_CLEAR_VALUES)
            .f(8, 8, 0) // Stencil Clear Value
            .f(16, 32, 0) // Z Clear Value
            .done(),
    );
    // TILE_LIST_INITIAL_BLOCK_SIZE — must precede the first branch; auto-chained, 128-byte first
    // block (PI-V3D-14: must MATCH the bin config's initial-block-size — Mesa fact).
    w.pkt(
        Pkt::new(P_TILE_LIST_INITIAL_BLOCK_SIZE, 2)
            .f(0, 2, TILE_ALLOC_BLOCK_SIZE_128B) // Size of first block
            .f(2, 1, 1) // Use auto-chained tile lists
            .done(),
    );

    // Per-layer render (single layer). MULTICORE_RENDERING_TILE_LIST_SET_BASE: the tile-alloc base (64-
    // byte-aligned). Address field is 26 bits at XML start 6 → the 64-aligned address's bits [6..31].
    w.pkt(
        Pkt::new(P_MULTICORE_TILE_LIST_BASE, 5)
            .f(0, 4, 0) // Tile List Set Number
            .f(6, 26, (tile_alloc >> 6) as u64) // address (64-byte aligned)
            .done(),
    );
    // MULTICORE_RENDERING_SUPERTILE_CFG: 1×1 tiles, one 1×1 supertile, single core, one bin tile list.
    w.pkt(
        Pkt::new(P_MULTICORE_SUPERTILE_CFG, 9)
            .f(0, 8, 0) // Supertile Width in Tiles (minus_one: 1 → 0)
            .f(8, 8, 0) // Supertile Height in Tiles (minus_one: 1 → 0)
            .f(16, 8, 1) // Total Frame Width in Supertiles
            .f(24, 8, 1) // Total Frame Height in Supertiles
            .f(32, 12, 1) // Total Frame Width in Tiles
            .f(44, 12, 1) // Total Frame Height in Tiles
            .f(61, 3, 0) // Number of Bin Tile Lists (minus_one: 1 → 0)
            .done(),
    );

    // Initial tile-buffer clear (also the GFXH-1742 double-dummy-store workaround on V3D 4.x). Clears
    // the tile buffer to the clear color before the first tile inherits stale contents.
    w.pkt(
        Pkt::new(P_TILE_COORDINATES, 4)
            .f(0, 12, 0) // tile column number
            .f(12, 12, 0) // tile row number
            .done(),
    );
    for i in 0..2 {
        if i > 0 {
            w.pkt(
                Pkt::new(P_TILE_COORDINATES, 4).f(0, 12, 0).f(12, 12, 0).done(),
            );
        }
        w.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
        // STORE (Buffer to Store = None = 8) — the dummy store that latches TLB type/size.
        w.pkt(Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13).f(0, 4, 8).done());
        if i == 0 {
            w.pkt(
                Pkt::new(P_CLEAR_TILE_BUFFERS, 2)
                    .f(0, 1, 1) // Clear all Render Targets
                    .f(1, 1, 1) // Clear Z/Stencil Buffer
                    .done(),
            );
        }
        w.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    }
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());

    // Branch target for the generic per-tile list, then execute the single supertile (this runs the
    // sub-list, which performs the real store), then halt.
    w.pkt(
        Pkt::new(P_GENERIC_TILE_LIST, 9)
            .f(0, 32, sublist_start as u64) // start
            .f(32, 32, sublist_end as u64) // end
            .done(),
    );
    w.pkt(
        Pkt::new(P_SUPERTILE_COORDINATES, 3)
            .f(0, 8, 0) // column number in supertiles
            .f(8, 8, 0) // row number in supertiles
            .done(),
    );
    w.pkt(Pkt::new(P_END_OF_RENDERING, 1).done());

    (w.len(), sublist_len)
}

/// A fixed-capacity control-list packet: byte 0 is the opcode, the rest is the field payload. Fields
/// are packed by their v3d_packet_v33.xml bit position via `f` (absolute packet bit = XML start + 8,
/// per Mesa's opcode-shift convention). `len` is the packet's exact byte length.
struct Pkt {
    buf: [u8; 16],
    len: usize,
}
impl Pkt {
    #[inline]
    fn new(opcode: u8, len: usize) -> Self {
        let mut buf = [0u8; 16];
        buf[0] = opcode;
        Pkt { buf, len }
    }
    /// Pack a field: `xml_start` is the v3d_packet_v33.xml `start` bit; the opcode-shift (+8) is applied
    /// here so callers can quote XML offsets verbatim.
    #[inline]
    fn f(&mut self, xml_start: usize, width: usize, val: u64) -> &mut Self {
        set_bits(&mut self.buf, xml_start + 8, width, val);
        self
    }
    #[inline]
    fn done(&self) -> (&[u8], usize) {
        (&self.buf, self.len)
    }
}

/// Write `width` bits of `val` (LSB-first) into `buf` starting at absolute bit `bit`.
#[inline]
fn set_bits(buf: &mut [u8], mut bit: usize, mut width: usize, val: u64) {
    let mut v = val;
    while width > 0 {
        let byte = bit / 8;
        let off = bit % 8;
        let take = core::cmp::min(8 - off, width);
        let mask = ((1u64 << take) - 1) as u8;
        buf[byte] |= ((v as u8) & mask) << off;
        v >>= take;
        bit += take;
        width -= take;
    }
}

/// A bounded writer into the arena. Every append is checked against the arena end; it can only ever
/// write inside V3D_ARENA (the review-lens no-overrun guarantee for control-list construction).
struct RclWriter {
    off: usize,
    start: usize,
}
impl RclWriter {
    fn new(start_off: usize) -> Self {
        RclWriter { off: start_off, start: start_off }
    }
    #[inline]
    fn put(&mut self, b: u8) {
        if self.off >= ARENA_BYTES {
            return; // saturating — never writes past the arena; the control lists are far smaller
        }
        unsafe {
            (*(&raw mut V3D_ARENA)).bytes[self.off] = b;
        }
        self.off += 1;
    }
    /// Append one encoded packet's exact bytes (`(&buf, len)` from `Pkt::done`).
    #[inline]
    fn pkt(&mut self, packet: (&[u8], usize)) {
        let (buf, len) = packet;
        for &b in &buf[..len] {
            self.put(b);
        }
    }
    #[inline]
    fn len(&self) -> usize {
        self.off - self.start
    }
}

/// True if `[addr, addr+len)` lies wholly inside the arena.
#[inline]
fn arena_contains(addr: usize, len: usize) -> bool {
    let base = arena_phys();
    addr >= base && len <= ARENA_BYTES && addr - base <= ARENA_BYTES - len
}

/// Fill the target region with a 32-bit pattern (CPU-side; pre-seed sentinel).
fn fill_target(pattern: u32) {
    let arena = &raw mut V3D_ARENA;
    unsafe {
        let mut i = 0;
        while i < TARGET_BYTES {
            for b in pattern.to_le_bytes() {
                (*arena).bytes[OFF_TARGET + i] = b;
                i += 1;
            }
        }
    }
}

/// CPU-side verify: every 32-bit word of the target equals `expect`. Reports the first mismatch.
fn verify_target(expect: u32) -> bool {
    let arena = &raw const V3D_ARENA;
    let mut i = 0;
    while i + 4 <= TARGET_BYTES {
        let w = unsafe {
            let b = &(*arena).bytes;
            u32::from_le_bytes([
                b[OFF_TARGET + i],
                b[OFF_TARGET + i + 1],
                b[OFF_TARGET + i + 2],
                b[OFF_TARGET + i + 3],
            ])
        };
        if w != expect {
            serial_println!(
                ":: V3D: verify mismatch at word {} — got {:#010x} expect {:#010x} ::",
                i / 4, w, expect
            );
            return false;
        }
        i += 4;
    }
    true
}

/// Blit the verified 64×64 target into the top-left of the panel framebuffer — the metal visible
/// witness. Bounds-checked against both the target and the framebuffer; clips to whatever fits.
fn blit_target(fb: &FbTarget) {
    if fb.base == 0 || fb.bytes_per_pixel < 4 {
        return;
    }
    let arena = &raw const V3D_ARENA;
    let w = TARGET_W.min(fb.width);
    let h = TARGET_H.min(fb.height);
    for y in 0..h {
        for x in 0..w {
            let src = OFF_TARGET + (y * TARGET_W + x) * TARGET_BPP;
            let px = unsafe {
                let b = &(*arena).bytes;
                u32::from_le_bytes([b[src], b[src + 1], b[src + 2], b[src + 3]])
            };
            let dst = fb.base as usize + y * fb.stride_px * fb.bytes_per_pixel + x * fb.bytes_per_pixel;
            // Confine the write to the framebuffer extent.
            if dst + 4 <= fb.base as usize + fb.size {
                unsafe { core::ptr::write_volatile(dst as *mut u32, px) };
            }
        }
    }
}

/// Poll `reg` at `base` until `mask` clears, with a finite ~500 ms wall-clock backstop off CNTPCT.
/// Returns false on timeout (the caller fails closed). This is the anti-hang discipline: never an
/// unbounded spin — a wedged GPU degrades the boot to "no V3D", it does not hang it.
fn wait_bit_clear(base: usize, reg: usize, mask: u32, what: &str) -> bool {
    let deadline = super::timer::cntpct() + super::timer::cntfrq() / 2;
    while mmio_read(base, reg) & mask != 0 {
        if super::timer::cntpct() >= deadline {
            serial_println!(":: V3D: timeout waiting for {} (backstop) ::", what);
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Poll `base+reg & mask` until it becomes SET, with the same finite ~0.5 s CNTPCT backstop as
/// `wait_bit_clear`. Used by the V3D-50 core-reset OFF half: `bcm2835_asb_power_off` requests a bridge
/// stopped and waits for `ASB_ACK` to SET (the bridge acknowledging it has quiesced). On QEMU the
/// rpivid_asb block is unbacked (reads 0), so ACK never sets and the backstop returns false — logged,
/// non-fatal (the IDENT0 probe downstream is the real verdict gate).
fn wait_bit_set(base: usize, reg: usize, mask: u32, what: &str) -> bool {
    let deadline = super::timer::cntpct() + super::timer::cntfrq() / 2;
    while mmio_read(base, reg) & mask == 0 {
        if super::timer::cntpct() >= deadline {
            serial_println!(":: V3D: timeout waiting for {} to SET (backstop) ::", what);
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// PI-V3D-44: wait for the binner to actually RETIRE, not merely for CT0CS's run bit to drop.
///
/// V3D-43 STOP verdict falsified the wrong-opcode theory (our bin-list tail is Mesa v3d 4.2's exact
/// job_emit_binning_flush — single FLUSH op 4). P40 metal then showed the CLE consumed the whole list
/// incl. the FLUSH (CT0CA=EA) and the PTB emitted 0x3000 bytes (BPCA 0x1a2000→0x1a5000), yet BFC held
/// at Δ0 and raw PCS bit0=1 at the post-idle readout — the flush never RETIRED. The cause named by
/// V3D-43: our "idle" predicate is CT0CS-based; the binner can still be draining (or stalled waiting
/// on overflow memory) when the CT0CS run bit clears. The kernel v3d driver never polls CT0CS for this
/// — it waits for the BIN done IRQ V3D_INT_FLDONE (v3d_irq.c). We poll the same register bit.
///
/// Bounded poll of INT_STS FLDONE off the free-running CNTPCT (same ~0.5 s backstop shape as
/// `wait_bit_clear`). Returns (raw INT_STS at exit, microseconds waited, FLDONE-observed). On a
/// timeout the RAW status is printed so the next boot names which interrupt DID fire — OUTOMEM (binner
/// stalled for overflow memory it was never given) and GMPV especially. The witness the brief names is
/// emitted by the caller with the BFC pre/post pair.
/// PI-V3D-46: rebuild the exact TILE_BINNING_MODE_CFG (v42) packet we submit — from the SAME `Pkt` path
/// `build_bin_cl_generic` uses — and hex-dump its 9 bytes. Proves on metal that the config the binner ran
/// is byte-identical to the Mesa v3d_packet.xml gen-4.2 contract. Audited field-by-field in V3D-46 against
/// `v3dX(job_emit_binning_prolog)` (Mesa `v3dvx_cmd_buffer.c`) and the genxml packet code 120:
///   bits[2..4)=initial block size (128B→1), bits[4..6)=block size (64B→0), bits[8..12)=RT count-1 (1→0),
///   bits[12..14)=max BPP (32-bit→0), bits[32..48)=width-1 (63), bits[48..64)=height-1 (63) — 8/8 MATCH.
fn dump_bin_mode_cfg_bytes() {
    let mut p = Pkt::new(P_TILE_BINNING_MODE_CFG, 9);
    p.f(2, 2, TILE_ALLOC_BLOCK_SIZE_128B)
        .f(4, 2, TILE_ALLOC_BLOCK_SIZE_64B)
        .f(8, 4, 0)
        .f(12, 2, INTERNAL_BPP_32)
        .f(32, 16, (TARGET_W - 1) as u64)
        .f(48, 16, (TARGET_H - 1) as u64);
    let (buf, len) = p.done();
    serial_println!(
        ":: V3D: [v3d46] TILE_BINNING_MODE_CFG bytes ({} B) = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} — opcode=120, initial=128B/overflow=64B, RTs=1, BPP=32, {}x{} px (minus_one 63x63) — audited 8/8 vs Mesa v42 ::",
        len,
        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8],
        TARGET_W, TARGET_H,
    );
}

/// PI-V3D-52 (Rung 2): audit the TILE_BINNING_MODE_CFG (v42, code 120) field set for a "tile-state
/// auto-initialise" enable bit an EMPTY bin frame would uniquely depend on. The contingency hypothesis
/// was that Mesa/the kernel never submits a naked config+START+FLUSH because the tile-state array is
/// auto-initialised by a config-packet bit our Empty rung leaves clear — with tile state never
/// initialised the FLUSH has nothing to write back and BMACTIVE never clears.
///
/// FINDING (audited against the authoritative in-repo v42 field map — the §V3D-46 8-field enumeration
/// dumped by `dump_bin_mode_cfg_bytes`, which is byte-verified against Mesa `v3d_packet.xml` gen 4.2):
/// the v42 code-120 packet has NO auto-initialise-tile-state field. That field existed only in the
/// pre-v41 (v3.3) TILE_BINNING_MODE_CFG and was REMOVED in the v41+ restructure — on V3D 4.2 the
/// tile-state array is initialised implicitly by START_TILE_BINNING, not by a config bit. The v42
/// field set is exactly {initial-block-size@2, block-size@4, RT-count@8, max-BPP@12, MSAA-4x@14,
/// double-buffer@15, width@32, height@48}; bits 14/15 are correctly clear for our single-sample,
/// single-buffer 64×64 frame. So the Empty rung's config is COMPLETE and byte-exact — there is no
/// divergent tile-state-init bit to set, and no behavioural change is warranted (instrument-and-name).
/// The ladder's own differential remains the live test: if `state-no-prims` retires while `empty-frame`
/// does not, the missing packet is a PROLOGUE/state packet, not a config bit — named directly there.
fn audit_bin_mode_cfg_autoinit() {
    // Rebuild the exact v42 config we submit and confirm bits 14 (MSAA) and 15 (double-buffer) — the only
    // config flags an empty frame could differ on — are clear, and report the auto-init audit verdict.
    let mut p = Pkt::new(P_TILE_BINNING_MODE_CFG, 9);
    p.f(2, 2, TILE_ALLOC_BLOCK_SIZE_128B)
        .f(4, 2, TILE_ALLOC_BLOCK_SIZE_64B)
        .f(8, 4, 0)
        .f(12, 2, INTERNAL_BPP_32)
        .f(32, 16, (TARGET_W - 1) as u64)
        .f(48, 16, (TARGET_H - 1) as u64);
    let (buf, _len) = p.done();
    // byte 1 carries bits 8..16: MSAA@14 → bit6 of buf[1], double-buffer@15 → bit7 of buf[1].
    let msaa = (buf[1] >> 6) & 1;
    let dbuf = (buf[1] >> 7) & 1;
    serial_println!(
        ":: V3D: [v3d52] tile-binning-mode-cfg auto-init audit — v42 code-120 has NO auto-initialise-tile-state field (that bit is pre-v41/v3.3-only; on 4.2 START_TILE_BINNING inits tile state). Empty-rung config MSAA-4x@14={} double-buffer@15={} (both want 0), 8/8 v42 fields byte-exact — config is COMPLETE, no divergent bit; Empty's non-retire is NOT a missing config flag. If state-no-prims retires while empty wedges, the gap is a prologue/state packet ::",
        msaa, dbuf,
    );
}

/// PI-V3D-53: the honest verdict on the Rung 3 TMUWCF drain candidate V3D-52 staged. Sourced against the
/// kernel (v3d_gem.c / v3d_sched.c / v3d_irq.c, GPL-2.0-only, facts-only): `V3D_L2TCACTL_TMUWCF` (bit8) is
/// written in exactly ONE place — `v3d_clean_caches()`, which (1) writes `L2TCACTL=TMUWCF` and polls it
/// clear, then (2) writes `L2TCACTL=FLM=CLEAN` and polls L2TFLS clear. `v3d_clean_caches` runs ONLY as the
/// dedicated `V3D_CACHE_CLEAN` job (`v3d_cache_clean_job_run`), scheduled AFTER the render job and gated on
/// the userspace `DRM_V3D_SUBMIT_CL_FLUSH_CACHE` flag. It is NOT in `v3d_bin_job_run`, NOT in
/// `v3d_render_job_run`, NOT in `v3d_irq`. So TMUWCF is a POST-RENDER cache-clean op — downstream of the
/// bin FLDONE / BFC++ handshake the empty rung wedges on (`BMACTIVE=1`, frame still open). Arming a TMUWCF
/// drain in the bin frame path would run a rung the kernel PROVABLY does not run for bin jobs — a
/// fabricated fix. Per the no-fabricated-fix law it is NOT armed. This witness records L2TCACTL + the
/// sourced verdict; the actually-derived next candidate (kernel-exact FLM=CLEAR pre-job invalidate,
/// `bin_prejob_invalidate_kernel_exact`) is the `[v3d53]` empty rung below.
fn refute_tmuwcf_drain_candidate() {
    let l2tcactl = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TCACTL);
    serial_println!(
        ":: V3D: [v3d53] tmuwcf-drain REFUTED for bin path — TMUWCF(bit8) is written ONLY in kernel v3d_clean_caches (V3D_CACHE_CLEAN job, post-RENDER, DRM_V3D_SUBMIT_CL_FLUSH_CACHE-gated); NEVER in v3d_bin_job_run/v3d_render_job_run/v3d_irq. The empty rung wedges at the bin FLDONE handshake (BMACTIVE=1), UPSTREAM of any post-render clean — arming TMUWCF here runs a rung the kernel never runs for bin jobs (fabricated fix). NOT armed. L2TCACTL={:#010x} (L2TFLS={} TMUWCF={}). Derived next candidate = kernel-exact FLM=CLEAR pre-job invalidate (see [v3d53] empty-after-clear-invalidate rung) ::",
        l2tcactl,
        (l2tcactl & V3D_L2TCACTL_L2TFLS != 0) as u32,
        (l2tcactl & V3D_L2TCACTL_TMUWCF != 0) as u32,
    );
}

/// PI-V3D-54 (RANK 1) — a single-register transition tracker for the CL-progression time-series.
/// Instead of retaining the ~500 raw ~1 ms samples across the 0.5 s retire-wait, we fold each register
/// into first value / last value / number of distinct changes / the µs of the FIRST move off `first` /
/// the µs of the LAST change (the stall point). Five of these (CT0CA, CT0CS, CT0LC, CT0PC, BPCA) give
/// the compact `[v3d54] trace` line: start offset, stall offset, end offset — the fetches-but-stalls vs
/// never-fetches discriminator the single-shot post-wait sampling was hiding (see the design brief).
#[derive(Clone, Copy)]
struct TraceReg {
    first: u32,
    last: u32,
    changes: u32,
    first_move_us: u32,
    last_change_us: u32,
}

impl TraceReg {
    fn new(v: u32) -> Self {
        Self { first: v, last: v, changes: 0, first_move_us: 0, last_change_us: 0 }
    }
    fn update(&mut self, v: u32, us: u32) {
        if v != self.last {
            if self.changes == 0 {
                self.first_move_us = us;
            }
            self.changes += 1;
            self.last_change_us = us;
            self.last = v;
        }
    }
}

/// PI-V3D-54 (RANK 1) — emit the folded CL-progression time-series for one CT0 bin kick. `ba`/`ea` are the
/// submitted [begin, end) CL bounds so CT0CA is reported as a byte OFFSET into the list; BPCA is reported
/// raw + as an advance delta off its first sample (the PTB write pointer climbing = primitive bytes
/// emitted). The interpretation names the fork: CT0CA never leaves BA (never-fetches → RANK 2 submission
/// audit decides), CT0CA stalls mid-list (fetches-but-chokes at that offset), or CT0CA reaches EA (CLE
/// walked the whole list → the wall is downstream of the CLE).
#[allow(clippy::too_many_arguments)]
fn emit_v3d54_trace(
    what: &str,
    ba: u32,
    ea: u32,
    samples: u32,
    span_us: u64,
    ca: &TraceReg,
    cs: &TraceReg,
    lc: &TraceReg,
    pc: &TraceReg,
    bpca: &TraceReg,
) {
    let ca_off = |v: u32| v.wrapping_sub(ba);
    let ca_first_off = ca_off(ca.first);
    let ca_last_off = ca_off(ca.last);
    let reached_ea = ca.last == ea;
    let never_left_ba = ca.changes == 0 && ca.last == ba;
    let bpca_adv = bpca.last.wrapping_sub(bpca.first);
    let interp: &str = if never_left_ba {
        "CT0CA NEVER left BA across the whole wait — the CLE never fetched the list; the GO was a no-op or the list is mis-submitted (the [v3d54] submit audit decides BA/EA/length)"
    } else if reached_ea {
        if bpca_adv != 0 {
            "CT0CA walked BA->EA (whole list consumed) and BPCA advanced — the CLE ran the list and the PTB emitted primitive bytes; the missing retire is downstream (FLDONE/BFC generation), not the CLE walk"
        } else {
            "CT0CA walked BA->EA (whole list consumed) but BPCA never advanced — the CLE stepped every packet yet the PTB wrote no primitive-list bytes; the wall is the PTB write / frame-close, not the CLE walk"
        }
    } else {
        "CT0CA advanced off BA then STALLED mid-list at the offset above — the CLE choked on the packet at that byte offset (fetches-but-stalls)"
    };
    serial_println!(
        ":: V3D: [v3d54] trace ({}) samples={} span={}us BA={:#010x} EA={:#010x}(len={}) | CT0CA off {:#x}->{:#x}{} moves={} first_move={}us stall@{}us | CT0CS first={:#010x} last={:#010x}(CTRUN {}->{}) changes={} | CT0LC {:#x}->{:#x} | CT0PC {:#x}->{:#x} | BPCA {:#010x}->{:#010x} adv={:#x} moves={} — {} ::",
        what, samples, span_us, ba, ea, ea.wrapping_sub(ba),
        ca_first_off, ca_last_off,
        if reached_ea { "(=EA)" } else if never_left_ba { "(=BA)" } else { "(mid)" },
        ca.changes, ca.first_move_us, ca.last_change_us,
        cs.first, cs.last,
        (cs.first & V3D_CLE_CTNCS_CTRUN != 0) as u32,
        (cs.last & V3D_CLE_CTNCS_CTRUN != 0) as u32,
        cs.changes,
        lc.first, lc.last,
        pc.first, pc.last,
        bpca.first, bpca.last, bpca_adv, bpca.changes,
        interp,
    );
}

/// PI-V3D-54 (RANK 2) — the empty-bisect submission audit. Read BACK the latched CT0 queue registers
/// (CT0QBA/CT0QEA) after the GO and compare against the intended [BA,EA) and the CL byte length we built.
/// Returns whether the latched span is SOUND (CT0QBA==BA, CT0QEA==EA, EA−BA == built length, non-empty).
/// A mis-latched EA (==BA, or a span that does not match the built length) means the CLE was handed a
/// DIFFERENT list than we composed — a non-retire is then a submission artifact, not a frame-close failure,
/// and the whole "empty frame must retire" premise is being tested against a list that never ran.
fn v3d54_submit_audit(what: &str, ba: u32, ea: u32, len: usize) -> bool {
    let qba = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QBA);
    let qea = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QEA);
    let latched_span = qea.wrapping_sub(qba);
    let ba_ok = qba == ba;
    let ea_ok = qea == ea;
    let empty_latch = qea == qba;
    let span_ok = latched_span as usize == len;
    let sound = ba_ok && ea_ok && span_ok && !empty_latch && len != 0;
    serial_println!(
        ":: V3D: [v3d54] submit ({}) — intended BA={:#010x} EA={:#010x} len={} | latched CT0QBA={:#010x} CT0QEA={:#010x} span={} — BA {} EA {} span {} — {} ::",
        what, ba, ea, len, qba, qea, latched_span,
        if ba_ok { "OK" } else { "MISMATCH" },
        if ea_ok { "OK" } else { "MISMATCH" },
        if span_ok { "OK" } else { "MISMATCH" },
        if sound {
            "submission SOUND — the CLE was handed exactly the [BA,EA) list we built; a non-retire here is a genuine frame-close fact, not a submission artifact"
        } else if empty_latch {
            "EA==BA LATCHED — the GO enqueued a ZERO-length list; the CLE has nothing to walk. This is a submission defect, NOT a frame-close failure — the empty-frame premise sits on a list that never ran (see the [v3d54] resubmit)"
        } else {
            "latched span != built length — the CLE will walk a DIFFERENT list than we composed; the retire verdict is a submission artifact (see the [v3d54] resubmit)"
        }
    );
    sound
}

// ─── PI-V3D-55: RANK 3 (clock-domain / flush liveness) + RANK 4 (tile-state readback) ─────────────
//
// P53/P54 closed the register-mirror line: submission is SOUND (CT0QBA/CT0QEA latch exact), the first
// kick's CLE walks BA→EA in full, the QPU runs (INT_STS bit16 latches), and every mirror-able probe
// register (L2T window §25, hub-INT §26, CLEAR invalidate §27, TMUWCF §27) landed a byte-exact no-op.
// What never happens is the CLE→PTB frame close: BPCA never advances, FLDONE never fires, CTRUN stays
// set so every later kick no-ops. Two orthogonal questions survive, and V3D-55 asks both in one boot:
//
//   RANK 3 — is the flush domain even CLOCKED? The QPU provably executes, but QPU execution and the
//     CLE/PTB flush unit need not share a live clock domain. (a) CYCLE_COUNT (PCTR src32) sampled as a
//     DELTA across the retire-wait says whether the core clock free-runs while FLDONE stays dead;
//     (b) the firmware V3D clock is COMMANDED to 500 MHz by `bringup` and never read back — read the
//     granted rate + gate state; (c) MISCCFG.QRMAXCNT[3:1] (queued-request max count) is declared in
//     this file and has never been programmed or even audited against its reset/DT default — a floored
//     request queue could starve the PTB. Reads only; the QRMAXCNT write stays DISARMED (below).
//
//   RANK 4 — did the PTB WRITE anything? bin_pool_witness reads only the first 8 bytes of the pool and
//     of the tile-state array. Read the WHOLE tile-state array plus the pool head after the L2T
//     write-back, and dump the V3D PTEs covering the CL / tile-state / pool iovas. Tile-state written
//     but BFC Δ0 narrows the defect to the FLDONE/BFC latch itself; nothing written despite a BPCA that
//     once advanced (P40) means BPCA is a phantom pointer and the address question reopens.
//
// Every line below is a new `[v3d55]` serial witness on the existing probe path (default-quiet law: it
// fires only where the V3D probe battery already runs) and is READ-ONLY MMIO, with the single exception
// of the explicitly-disarmed QRMAXCNT write.

/// PI-V3D-55 (RANK 3c): arm the scoped MISCCFG QRMAXCNT write. **DISARMED by default and left that way
/// by this arc** — the brief permits the write ONLY behind evidence of divergence, and V3D-55 is the
/// boot that first *collects* that evidence. The audit below reports the latched QRMAXCNT every boot;
/// if metal shows it floored at 0 (a starved PTB request queue), the next arc flips this const and the
/// write fires. Flipping it alone is not enough — the write is additionally gated on the divergence
/// actually being observed at runtime, so an armed-but-clean block is still never written.
const V3D55_ARM_QRMAXCNT: bool = false;
/// The QRMAXCNT[3:1] value the armed write would program: the field maximum (7 queued requests), i.e.
/// the least-starved setting. Only ever written when `V3D55_ARM_QRMAXCNT` AND a floored read coincide.
const V3D55_QRMAXCNT_WANT: u32 = 0x7;

/// PI-V3D-55 (RANK 3b + 3c): the clock-domain audit. Reads back what the FIRMWARE actually granted for
/// `CLOCK_ID_V3D` (rate + gate state) against the 500 MHz `bringup` commanded but never verified, and
/// audits `MISCCFG` — specifically QRMAXCNT[3:1], declared in this file since V3D-34 and never once
/// programmed. Linux `v3d_init_core` writes MISCCFG only on ver<41 (the Pi 4 is v42), and the bcm2711
/// DT programs no QRMAXCNT for v3d, so the EXPECTED value is whatever the block's reset default is —
/// this line puts that default on serial for the first time. Pure reads unless `V3D55_ARM_QRMAXCNT`.
fn v3d55_clock_domain_audit(tag: &str) {
    // (b) firmware clock readback — the half `bringup` never did.
    //
    // PI-V3D-55 evidence-integrity fix: use `get_clock_rate_RAW`. The ordinary `get_clock_rate` folds a
    // successful transaction reporting rate==0 into `None` (its EMMC2 caller wants "usable rate or
    // nothing"), which would make the single most diagnostic reading available here — the firmware
    // granting the V3D clock 0 Hz — print as a mailbox FAILURE and leave the 0 Hz verdict unreachable.
    // The raw query returns `None` ONLY for a transport failure, so `Some(0)` is a real zero grant.
    let rate = mailbox::get_clock_rate_raw(mailbox::CLOCK_ID_V3D);
    let state = mailbox::get_clock_state(mailbox::CLOCK_ID_V3D);
    let rate_hz = rate.unwrap_or(0);
    let state_w = state.unwrap_or(0);
    let active = state_w & 0x1 != 0;
    let notexist = state_w & 0x2 != 0;
    serial_println!(
        ":: V3D: [v3d55] clkdom ({}) — commanded=500000000 Hz | GET_CLOCK_RATE={} Hz (mailbox {}) GET_CLOCK_STATE={:#010x} (active={} not_exist={}, mailbox {}) — {} ::",
        tag,
        rate_hz,
        // OK means the TRANSACTION succeeded; the Hz field is then the granted rate verbatim, 0
        // included. On FAILED the Hz field is a placeholder and carries no information.
        if rate.is_some() { "OK (rate verbatim, 0 = a real 0 Hz grant)" } else { "FAILED (Hz field meaningless)" },
        state_w,
        active as u32,
        notexist as u32,
        if state.is_some() { "OK" } else { "FAILED" },
        if rate.is_none() || state.is_none() {
            "clock readback FAILED — no clock-domain verdict from firmware this boot (QEMU has no V3D clock; metal decides)"
        } else if !active || notexist {
            "the firmware reports the V3D clock GATED OFF (or absent) DESPITE bringup's set_clock_state(true) ACK — the block is powered but unclocked, which alone explains a dead flush unit"
        } else if rate_hz == 0 {
            "clock gate ACTIVE but the granted RATE reads 0 Hz — the block is gated on at a null frequency; the flush domain cannot tick"
        } else if rate_hz != 500_000_000 {
            "clock ACTIVE but the GRANTED rate differs from the commanded 500 MHz — firmware clamped/substituted the V3D clock; not fatal on its own, but the commanded-vs-granted gap was never visible before"
        } else {
            "clock ACTIVE at exactly the commanded 500 MHz — the firmware side of the clock domain is clean; any dead-flush verdict must come from the CYCLE_COUNT delta ([v3d55] clkliv) or the core config below"
        }
    );

    // (c) MISCCFG / QRMAXCNT audit.
    let misccfg = mmio_read(V3D_CORE0_BASE, V3D_CTL_MISCCFG);
    let qrmaxcnt = (misccfg & V3D_CTL_MISCCFG_QRMAXCNT_MASK) >> 1;
    let floored = qrmaxcnt == 0;
    serial_println!(
        ":: V3D: [v3d55] misccfg ({}) — MISCCFG={:#010x} OVRTMUOUT(bit0)={} QRMAXCNT[3:1]={} — EXPECTED: reset default (Linux v3d_init_core writes MISCCFG only on ver<41; the Pi 4 is v42, and the bcm2711 DT programs no QRMAXCNT for v3d) — {} — scoped QRMAXCNT write {} ::",
        tag,
        misccfg,
        (misccfg & V3D_MISCCFG_OVRTMUOUT != 0) as u32,
        qrmaxcnt,
        if floored {
            "QRMAXCNT reads 0 (FLOORED): the core will queue the minimum number of outstanding requests — a starved PTB request queue is a live BCM2711 candidate for a bin that emits nothing"
        } else {
            "QRMAXCNT nonzero: the request queue is not floored — no divergence from a sane default, so no QoS write is justified (the flush wall is elsewhere)"
        },
        if V3D55_ARM_QRMAXCNT && floored {
            "ARMED + divergence observed: writing the field maximum below"
        } else if V3D55_ARM_QRMAXCNT {
            "ARMED but no divergence observed — NOT written (the write is gated on a floored read, never issued blind)"
        } else {
            "DISARMED (V3D-55 collects the evidence; the write is a next-arc decision — see V3D55_ARM_QRMAXCNT)"
        }
    );

    // The ONLY non-read in this arc, and it is doubly gated: the const must be flipped by a future arc
    // AND the block must actually read floored. Scoped + reversible: a single read-modify-write of the
    // QRMAXCNT field, every other MISCCFG bit preserved, echoed back.
    if V3D55_ARM_QRMAXCNT && floored {
        let want = (misccfg & !V3D_CTL_MISCCFG_QRMAXCNT_MASK)
            | ((V3D55_QRMAXCNT_WANT << 1) & V3D_CTL_MISCCFG_QRMAXCNT_MASK);
        mmio_write(V3D_CORE0_BASE, V3D_CTL_MISCCFG, want);
        dsb();
        let echo = mmio_read(V3D_CORE0_BASE, V3D_CTL_MISCCFG);
        serial_println!(
            ":: V3D: [v3d55] qrmaxcnt ({}) — MISCCFG {:#010x} -> wrote {:#010x} -> echo {:#010x} (QRMAXCNT {} -> {}) — {} ::",
            tag, misccfg, want, echo,
            qrmaxcnt,
            (echo & V3D_CTL_MISCCFG_QRMAXCNT_MASK) >> 1,
            if echo == want {
                "the field LATCHED — if this boot's bin now retires, the floored request queue was the wall"
            } else {
                "the write did NOT latch — QRMAXCNT is read-only/ignored on this silicon; refute the QoS branch"
            }
        );
    }
}

/// PI-V3D-55 (RANK 3a): the clock-liveness verdict folded from the retire-wait's CYCLE_COUNT samples.
/// `en` is the PCTR enable mask read at the start of the wait — counter 2 carries src32 CYCLE_COUNT and
/// is armed only where `pctr_setup_cs_witness` ran (the PROBE bin), so an unarmed wait yields NO verdict
/// rather than a fabricated "flat clock". A flat CYCLE_COUNT across a 0.5 s wait is the strongest
/// possible statement that the core/flush domain is not ticking; a climbing one moves the whole
/// investigation back onto the CL/PTB frame-close path with the clock branch closed.
fn emit_v3d55_clock_liveness(what: &str, en: u32, cyc: &TraceReg, span_us: u64, samples: u32) {
    let armed = en & (1 << 2) != 0;
    let delta = cyc.last.wrapping_sub(cyc.first);
    serial_println!(
        ":: V3D: [v3d55] clkliv ({}) — PCTR_EN={:#010x} counter2(src32 CYCLE_COUNT) armed={} {:#010x}->{:#010x} Δ={} moves={} first_move={}us last_change={}us over {}us ({} samples) — {} ::",
        what, en, armed as u32, cyc.first, cyc.last, delta, cyc.changes,
        cyc.first_move_us, cyc.last_change_us, span_us, samples,
        if !armed {
            "counter 2 was NOT enabled across this wait — no clock verdict here (the PCTR battery is armed only around the PROBE bin; read the PROBE line for the domain answer)"
        } else if delta == 0 {
            "CYCLE_COUNT FLAT across the whole wait: the V3D core clock is NOT advancing while we wait for the flush — the flush domain is unclocked/half-clocked, and the fix is a clock/QoS one, NOT a CL-packet one (cross-check [v3d55] clkdom)"
        } else {
            "CYCLE_COUNT ADVANCED while FLDONE stayed dead: the CORE COUNTER domain is live and free-running. This does NOT close the clock branch — the CLE/PTB flush unit may sit on a separately-gated sub-domain that this counter does not observe; it only rules out a wholly-unclocked core. Reading it together with [v3d55] clkdom: a clean firmware grant + a live core counter makes 'clocked but never TRIGGERED' the leading reading, and the CLE→PTB frame-close path the next place to look"
        }
    );
}

/// PI-V3D-55 (RANK 4): the MMU-translation preamble to the PTE dump. The `[v3d55] pte` lines below read
/// OUR CPU-side `V3D_PT` static — the table we *intended* to publish — and therefore cannot, on their
/// own, detect a table that never reached DRAM, a `PT_PA_BASE` pointing somewhere else, or an MMU that
/// is not enabled at all. This line supplies exactly that missing half by reading back the GPU's own
/// translation configuration: `MMU_CTL.ENABLE`, the latched `PT_PA_BASE` (in pages) against the physical
/// address of the table we programmed, and the standing fault bits. Together the two halves are a real
/// discrimination; either alone is not.
fn v3d55_mmu_config_witness(tag: &str) {
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let pt_base = mmio_read(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE);
    let want_base = (pt_phys() >> V3D_MMU_PAGE_SHIFT) as u32;
    let enabled = ctl & V3D_MMU_CTL_ENABLE != 0;
    let base_ok = pt_base == want_base;
    let faults = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    serial_println!(
        ":: V3D: [v3d55] mmucfg ({}) — MMU_CTL={:#010x} ENABLE={} fault_bits={:#010x} | PT_PA_BASE={:#010x} (pages) vs ours={:#010x} (table phys {:#010x}) — {} ::",
        tag, ctl, enabled as u32, faults, pt_base, want_base, pt_phys(),
        if !enabled {
            "the V3D MMU is NOT ENABLED — the PTEs below describe a table the hardware is not consulting; every address the GPU issues is untranslated and the aliasing question is WIDE OPEN"
        } else if !base_ok {
            "MMU enabled but PT_PA_BASE does NOT point at the table we filled — the GPU is walking DIFFERENT page tables than the PTEs below decode; those lines describe memory the hardware never reads"
        } else if faults != 0 {
            "MMU enabled, base correct, but a FAULT is latched in MMU_CTL — a translation actually failed this run; the PTE lines below name which region"
        } else {
            "MMU enabled and PT_PA_BASE matches the table we programmed — the PTE decodes below are describing the SAME table the hardware walks (still a CPU-side read of what we published, not a hardware translation probe)"
        }
    );
}

/// PI-V3D-55 (RANK 4): decode one V3D PTE for a given iova out of the page table we programmed.
///
/// **Scope caveat, stated at the read site:** this reads the CPU-side `V3D_PT` static — our INTENDED
/// mapping — and is NOT a hardware translation probe. It cannot by itself detect a table whose cleaned
/// bytes never reached DRAM, a mis-latched `PT_PA_BASE`, or a disabled MMU; `[v3d55] mmucfg` above
/// covers those, and the two lines must be read together. What this line does establish is whether the
/// mapping we published is itself valid, writeable and truly identity for the regions the bin touches.
fn v3d55_pte_line(tag: &str, what: &str, iova: u32) {
    let idx = (iova as usize) >> V3D_MMU_PAGE_SHIFT;
    let in_range = idx < PT_CAP;
    let pte = if in_range { unsafe { (*(&raw const V3D_PT)).ptes[idx] } } else { 0 };
    let valid = pte & V3D_PTE_VALID != 0;
    let writeable = pte & V3D_PTE_WRITEABLE != 0;
    let pfn = pte & !(V3D_PTE_VALID | V3D_PTE_WRITEABLE);
    let identity = valid && ((pfn as usize) << V3D_MMU_PAGE_SHIFT) == (iova as usize & !(PAGE - 1));
    serial_println!(
        ":: V3D: [v3d55] pte ({}) {} iova={:#010x} — CPU-side PT[{}]={:#010x} (our PUBLISHED table, not a hardware translation — pair with [v3d55] mmucfg) VALID={} WRITEABLE={} pfn={:#x} (maps phys {:#010x}) — {} ::",
        tag, what, iova, idx, pte, valid as u32, writeable as u32, pfn,
        (pfn as usize) << V3D_MMU_PAGE_SHIFT,
        if !in_range {
            "iova is BEYOND the page table's capacity — the GPU cannot reach this address at all"
        } else if !valid {
            "PTE INVALID — the GPU would fault on this address; a silent no-write here is an addressing defect, not a frame-close one"
        } else if !identity {
            "PTE VALID but NOT identity — the GPU sees a DIFFERENT physical page than the CPU reads; every CPU readback of this region has been reading the wrong memory"
        } else if !writeable {
            "PTE valid + identity but READ-ONLY — a PTB write here is dropped by the MMU"
        } else {
            "PTE valid, writeable, identity-mapped AS PUBLISHED — the mapping we intended is correct; whether the GPU actually walks this table is the [v3d55] mmucfg line's question, not this one's"
        }
    );
}

/// PI-V3D-55 (RANK 4): the tile-state readback. `bin_pool_witness` reads only the first 8 bytes of the
/// pool and of the tile-state array; that is too coarse to distinguish "the PTB wrote tile-state but no
/// FLDONE fired" from "the PTB never wrote anything". Read the WHOLE tile-state array (TILE_STATE_BYTES,
/// the 48-B-per-tile TSDA generously sized for the single 64×64 tile) plus the pool head, AFTER an L2T
/// write-back so the binner's L2T-parked bytes reach DRAM (the V3D-42 mechanism), and count nonzero
/// words rather than eyeballing a prefix. Then dump the PTEs covering the CL, tile-state and pool iovas.
/// Read-only: an L2T write-back plus CPU-side cache maintenance and loads.
fn v3d55_tilestate_readback(tag: &str, cl_iova: u32, ts_iova: u32, pool_iova: u32) {
    // Drain the binner's writes out of L2T into DRAM, then drop the CPU's stale lines (V3D-42 order).
    //
    // PI-V3D-55 evidence-integrity fix: `invalidate_gpu_caches` DISCARDS the L2TFLS poll result, and an
    // all-zero readback is only evidence of "the PTB never wrote" if the write-back actually COMPLETED.
    // Under precisely the failure this arc hunts — a dead/half-clocked flush domain — the flush silently
    // no-ops, L2TFLS never clears, the binner's bytes stay parked in L2T and DRAM reads zero for a reason
    // that has nothing to do with the PTB. So run the same sequence inline and KEEP the completion bit,
    // then thread it into the verdict below. (Byte-identical to `invalidate_gpu_caches`, result retained.)
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_FLUSH);
    let flush_done =
        wait_bit_clear(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS, "L2T write-back (v3d55 tile-state readback)");
    mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
    dsb();
    let l2t_after = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TCACTL);
    cache::clean_invalidate_range(arena_phys() + OFF_TILESTATE, TILE_STATE_BYTES);
    cache::clean_invalidate_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    dsb();

    // Full tile-state array scan. The iova is identity-mapped to the arena VA, so the CPU load address
    // is literally the address the GPU was handed in CT0QTS — the same-page property [v3d42] prints.
    let words = TILE_STATE_BYTES / 4;
    let mut nonzero = 0u32;
    let mut first_nz_word = -1i32;
    let mut w = [0u32; 8];
    for i in 0..words {
        let v = unsafe { core::ptr::read_volatile((ts_iova as usize + i * 4) as *const u32) };
        if v != 0 {
            nonzero += 1;
            if first_nz_word < 0 {
                first_nz_word = i as i32;
            }
        }
        if i < 8 {
            w[i] = v;
        }
    }
    // Pool head + the PTB write pointer, side by side: BPCA off the pool base says the PTB CLAIMED to
    // emit bytes; a zero pool head with an advanced BPCA is the P40 phantom-pointer signature.
    let bpca = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA);
    let bpcs = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS);
    // PI-V3D-55 evidence-integrity fix: scan the WHOLE tile-alloc pool, not a 64-byte prefix. The
    // "the pool reads all-zero therefore BPCA is a phantom" reading below is a claim about the whole
    // pool, and BPCA was observed at P40 some 0x3000 bytes IN — far past any prefix. A prefix scan
    // could not have seen those bytes, so the prefix could never have supported the claim.
    let pool_words = BIN_TILEALLOC_BYTES / 4;
    let mut pool_nonzero = 0u32;
    let mut pool_first_nz = -1i32;
    let mut p = [0u32; 8];
    for i in 0..pool_words {
        let v = unsafe { core::ptr::read_volatile((pool_iova as usize + i * 4) as *const u32) };
        if v != 0 {
            pool_nonzero += 1;
            if pool_first_nz < 0 {
                pool_first_nz = i as i32;
            }
        }
        if i < 8 {
            p[i] = v;
        }
    }
    let bfc = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);

    serial_println!(
        ":: V3D: [v3d55] tilestate ({}) — TSDA iova={:#010x} bytes={} words={} nonzero_words={} first_nz=[{}] | L2T write-back completed={} (L2TCACTL after={:#010x}) | head w0..w7 = {:#010x} {:#010x} {:#010x} {:#010x} {:#010x} {:#010x} {:#010x} {:#010x} — {} ::",
        tag, ts_iova, TILE_STATE_BYTES, words, nonzero, first_nz_word,
        flush_done as u32, l2t_after,
        w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7],
        if nonzero != 0 {
            // A nonzero readback is positive evidence regardless of the flush outcome — bytes are there.
            "the PTB WROTE tile-state: a bin frame demonstrably produced per-tile output, so the defect is DOWNSTREAM of the write — isolated to the FLDONE/BFC frame-close latch itself (the narrowest target yet)"
        } else if !flush_done {
            // The negative reading is only as good as the drain that produced it.
            "tile-state reads zero BUT THE L2T WRITE-BACK DID NOT COMPLETE (L2TFLS never cleared before the backstop) — this is NOT evidence that the PTB wrote nothing: the binner's bytes may still be parked in L2T, exactly as a dead/half-clocked flush domain would leave them. NO upstream/downstream verdict from this line; read [v3d55] clkliv and clkdom first"
        } else {
            "tile-state is ENTIRELY ZERO after a COMPLETED L2T write-back: on this evidence the PTB wrote no per-tile state, placing the defect UPSTREAM of frame-close (the bin frame produced no output at all)"
        }
    );
    serial_println!(
        ":: V3D: [v3d55] pool ({}) — pool iova={:#010x} bytes={} words={} nonzero_words={} first_nz=[{}] (FULL-pool scan) | L2T write-back completed={} | head w0..w7 = {:#010x} {:#010x} {:#010x} {:#010x} {:#010x} {:#010x} {:#010x} {:#010x} | BPCA={:#010x} (adv {:#x} off pool base) BPCS={:#010x} BFC={:#010x} — {} ::",
        tag, pool_iova, BIN_TILEALLOC_BYTES, pool_words, pool_nonzero, pool_first_nz,
        flush_done as u32,
        p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7],
        bpca, bpca.wrapping_sub(pool_iova), bpcs, bfc,
        if pool_nonzero != 0 {
            "the pool carries binner bytes — primitive-list output exists in DRAM; frame-close is the only missing step"
        } else if !flush_done {
            "pool reads all-zero BUT THE L2T WRITE-BACK DID NOT COMPLETE — no PTB-write verdict from this line (the bytes may still be parked in L2T); see [v3d55] tilestate and the clock witnesses"
        } else if bpca == 0 {
            // A block whose registers read 0x0 is not a PTB parked at the pool base — do not conflate.
            "BPCA reads 0x0, which is NOT the pool base — the register is either genuinely reset or the block is not returning live values; this says nothing about where a PTB write pointer stands (check the other CLE/PTB registers in the [v3d45] dump for a block-wide zero/poison pattern)"
        } else if bpca != pool_iova {
            "BPCA ADVANCED off the pool base yet the FULL pool reads all-zero after a COMPLETED L2T write-back — on this evidence BPCA is a PHANTOM/aliased write pointer and the address question reopens (see [v3d55] mmucfg + the pte lines)"
        } else {
            "pool all-zero with BPCA exactly at the pool base after a completed write-back — the PTB emitted nothing and never moved its write pointer this kick"
        }
    );
    // The MMU configuration readback (what the GPU actually walks) FIRST, then our published PTEs —
    // neither half is a translation probe on its own; the pair is what carries the discrimination.
    v3d55_mmu_config_witness(tag);
    v3d55_pte_line(tag, "bin CL     ", cl_iova);
    v3d55_pte_line(tag, "tile-state ", ts_iova);
    v3d55_pte_line(tag, "tile-alloc ", pool_iova);
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-56 — make the phantom bytes FINDABLE.
//
// Standing V3D-55 evidence: the CLE walks BA→EA, the QPU runs, the core is clocked (CYCLE_COUNT +249M
// over the 500 ms wait), the GPU MMU is enabled and fault-free with our identity table latched — and
// yet the 32 KiB tile-alloc pool and the 192 B tile-state array read ENTIRELY ZERO after a COMPLETED
// L2T write-back, while BPCA sits 0x3000 bytes off the pool base. The recorded verdict was "BPCA is a
// phantom/aliased write pointer".
//
// That verdict rests on an inference this arc removes: a pool that STARTS all-zero cannot distinguish
// "the PTB never wrote" from "the PTB wrote zeros". Every witness to date pre-zeroed the pool, so a
// PTB write of zero bytes — precisely what an EMPTY tile list would emit — has been INVISIBLE. Poison
// the pool and the tile-state before the kick with an index-encoding pattern and ANY touch, zero or
// not, becomes a positive observation.
//
// Read-only except the poison fill itself (which replaces the existing zero fill of the same two
// regions — same bytes touched, different value; no new region is written).
// ══════════════════════════════════════════════════════════════════════════════════════════════════

/// Arm the V3D-56 poison. ON by default: it is this arc's primary witness, and it is strictly more
/// informative than the zero fill it replaces (a zeroed pool cannot witness a zero-valued write).
/// Flip to `false` to restore the historical pre-zeroed pool for a like-for-like diff against the
/// V3D-40..55 logs.
const V3D56_POISON: bool = true;
/// Poison base pattern. The stored word is `V3D56_POISON_SEED ^ index`, so every word is (a) instantly
/// recognisable by its high half (0xA5A5….) and (b) self-locating — a word found displaced elsewhere
/// still names the pool index it came from. `^` (not `+`) keeps the high half clean for pool indices
/// up to 0xFFFF, i.e. the whole 32 KiB / 8192-word pool.
const V3D56_POISON_SEED: u32 = 0xA5A5_A5A5;

#[inline]
fn v3d56_poison_word(i: usize) -> u32 {
    V3D56_POISON_SEED ^ (i as u32)
}

// ── The BPCA-semantics finding (V3D-56 item 3) — the phantom verdict RETRACTED on source evidence ──
//
// BPCA/BPCS are documented, and v42 keeps the VC4 (V3D 2.x) meaning at the same offsets 0x300/0x304
// (Linux `drivers/gpu/drm/v3d/v3d_regs.h`: `V3D_PTB_BPCA 0x00300` / `BPCS 0x00304`; identical in
// `drivers/gpu/drm/vc4/vc4_regs.h`). The upstream headers carry no comments; the field text is the
// VideoCore IV 3D Architecture Reference Guide's:
//
//     V3D_BPCA — "Current Address Of Binning Memory Pool"   (read-only)
//     V3D_BPCS — "Remaining Size Of Binning Memory Pool"    (read-only)
//
// i.e. BPCA is the pool's **allocation pointer** (a byte address; on v42 an MMU virtual address), and
// BPCS the **bytes remaining**. Neither is a bytes-WRITTEN counter — the ISA's written-work counters
// are CT0LC/CT0PC. This is the load-bearing distinction the phantom verdict missed.
//
// The decisive fact, from Mesa `src/broadcom/common/v3d_util.c` (`v3d_tile_alloc_sizes`), quoted:
//
//     /* The PTB will request the tile alloc initial size per tile at start
//      * of tile binning. The size must match the initial block size
//      * configured in the TILE_BINNING_MODE_CFG packet. */
//     uint32_t tiles_size = layers * tiles_x * tiles_y * V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE;
//     /* The PTB allocates in aligned 4k chunks after the initial setup. */
//     uint32_t alloc_size = align(tiles_size, 4096);
//     /* Include the first two chunk allocations that the PTB does so that
//      * we definitely clear the OOM condition before triggering one (the HW
//      * won't trigger OOM during the first allocations). */
//     alloc_size += 8192;
//
// So the PTB RESERVES per-tile initial blocks at START_TILE_BINNING — unconditionally, before and
// independent of any primitive — and thereafter grabs memory in aligned 4 KiB chunks, performing at
// least two such chunk allocations up front. **Reservation moves BPCA and writes nothing.**
//
// Apply the formula to our frame. 64×64 target, 64×64 tiles, 1 layer → tiles_x = tiles_y = 1:
//
//     tiles_size  = 1 × 1 × 1 × 128                  = 128 B   (V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE,
//                                                               `src/broadcom/common/v3d_limits.h`,
//                                                               and the block size our
//                                                               TILE_BINNING_MODE_CFG programs)
//     align(128, 4096)                               = 0x1000
//     + the two up-front 4 KiB chunk allocations     = 0x2000
//     ────────────────────────────────────────────────────────
//     expected BPCA advance on an EMPTY bin          = 0x3000
//
// The V3D-40 and V3D-55 metal boots measured BPCA advanced by **exactly 0x3000** off the pool base —
// the formula's value to the byte.
//
// TWO SCOPE LIMITS, both load-bearing; neither may be dropped when this finding is quoted:
//
//  (1) **The 0x3000 match is NON-DISCRIMINATING on its own.** Reservation rounds up to 4 KiB, so a bin
//      that wrote a SMALL tile list *inside* the reserved initial block leaves BPCA at exactly the same
//      0x3000. Advance-matches-prediction is therefore consistent with both "wrote nothing" and "wrote
//      a little", and cannot separate them. Only the POISON separates them — which is why the
//      retraction verdict in `[v3d56] bpca-vs-bytes` is gated on the CONJUNCTION (advance == predicted
//      AND poison fully intact) and never on the advance alone.
//
//  (2) **The 0x3000 was measured on the FULL probe draw list, not on an empty one.** The probe kicks
//      `build_bin_cl_generic` — a complete draw list with geometry — which is only *effectively* empty
//      because the coord shader never dispatches (`valid_instr=0`, the V3D-35/V3D-40 wall). So the
//      honest statement is: the advance is architectural for a bin frame that BINNED NO PRIMITIVES,
//      whatever the list nominally contained. Whether the §22 bisect Empty rung produces the same
//      0x3000 has never been measured; that measurement would close the gap.
//
// CONCLUSION (scoped as above): BPCA advancing 0x3000 over a pool the poison proves untouched is
// ARCHITECTURAL for a frame that binned no primitives. On that conjunction the "phantom/aliased write
// pointer" verdict is RETRACTED — there would be no phantom bytes because there were never any bytes:
// BPCA reports reserved space, and this driver read it as written space. The address question does not
// reopen; the GPU MMU evidence (enabled, fault-free, our table latched) stands unchallenged, and the
// defect collapses back to FLDONE generation, which `v3d56_emit_int_triple` below instruments.
//
// STATUS: **PENDING FIRST METAL BOOT.** The register semantics and the Mesa formula are settled from
// source and need no confirmation. The retraction's second conjunct — poison intact — has never been
// observed, because no `[v3d56]` line has ever run. Until a P56 boot prints one, this is a
// well-supported prediction, not a measured result.
//
// Corroborating detail for the FLDONE hunt (same sources): FLDONE is asserted by the binning FLUSH
// completing, not by CT0CA reaching CT0EA — CT0CA==CT0EA says only that the CLE consumed the list.
// The required BCL shape is TILE_BINNING_MODE_CFG → START_TILE_BINNING → (draws) → FLUSH (packet
// code 4, `src/broadcom/cle/v3d_packet.xml`), kicked by CT0QBA then CT0QEA. This driver's empty rung
// already emits exactly that chain (`P_START_TILE_BINNING` = 6, `P_FLUSH` = 4 below), so the list
// shape is not the gap either. Mask polarity, from `v3d_irq_enable` (`v3d_irq.c`) —
// `INT_MSK_SET = ~V3D_CORE_IRQS` then `INT_MSK_CLR = V3D_CORE_IRQS` — confirms **1 = MASKED**, as
// this driver has assumed since V3D-49.
/// The BPCA advance the Mesa reservation formula predicts for this frame on an EMPTY bin:
/// `align(layers × tiles_x × tiles_y × 128, 4096) + 8192`, with 1×1 tiles → `0x1000 + 0x2000`.
/// Reported alongside the measured advance so the match (or a divergence) is on serial, not in a
/// comment.
const V3D56_EXPECTED_EMPTY_BPCA_ADVANCE: u32 = 0x1000 + 0x2000;

/// Fill `bytes` at arena offset `off` with the index-encoding poison and clean it to PoC so the GPU
/// (which reads DRAM behind its own L2T, not the CPU's caches) sees the poison and not a stale line.
fn v3d56_poison_region(off: usize, bytes: usize) {
    for i in 0..(bytes / 4) {
        arena_write_u32(off + i * 4, v3d56_poison_word(i));
    }
    cache::clean_range(arena_phys() + off, bytes);
}

/// Outcome of one poisoned-region scan. `first`/`last` are word indices, −1 when untouched.
struct V3d56Scan {
    words: usize,
    intact: u32,
    zeroed: u32,
    overwritten: u32,
    first: i32,
    last: i32,
    first_got: u32,
    first_exp: u32,
}

/// Scan a poisoned region for disturbance. Reads through `iova` — identity-mapped to the arena VA, so
/// this is literally the address the GPU was handed, the same property `[v3d55] pte` publishes. The
/// caller is responsible for the L2T write-back + CPU invalidate that make DRAM authoritative.
///
/// Classifies every word into INTACT (poison as written), ZEROED (v == 0 — the write an empty tile
/// list would make, and the class no prior witness could see) or OVERWRITTEN (any other value).
fn v3d56_scan(iova: u32, bytes: usize) -> V3d56Scan {
    let words = bytes / 4;
    let mut s = V3d56Scan {
        words,
        intact: 0,
        zeroed: 0,
        overwritten: 0,
        first: -1,
        last: -1,
        first_got: 0,
        first_exp: 0,
    };
    for i in 0..words {
        let v = unsafe { core::ptr::read_volatile((iova as usize + i * 4) as *const u32) };
        let exp = v3d56_poison_word(i);
        if v == exp {
            s.intact += 1;
            continue;
        }
        if v == 0 {
            s.zeroed += 1;
        } else {
            s.overwritten += 1;
        }
        if s.first < 0 {
            s.first = i as i32;
            s.first_got = v;
            s.first_exp = exp;
        }
        s.last = i as i32;
    }
    s
}

/// Emit one `[v3d56] poison` witness for a scanned region.
fn v3d56_emit_scan(tag: &str, what: &str, iova: u32, s: &V3d56Scan, flush_done: bool) {
    let touched = s.zeroed + s.overwritten;
    serial_println!(
        ":: V3D: [v3d56] poison ({}) {} iova={:#010x} words={} — INTACT={} ZEROED={} OVERWRITTEN={} touched={} | first_touched=[{}] (got {:#010x} expected {:#010x}) last_touched=[{}] | byte span [{:#x},{:#x}] | L2T write-back completed={} — {} ::",
        tag, what, iova, s.words, s.intact, s.zeroed, s.overwritten, touched,
        s.first, s.first_got, s.first_exp, s.last,
        if s.first < 0 { 0 } else { s.first as usize * 4 },
        if s.last < 0 { 0 } else { s.last as usize * 4 + 3 },
        flush_done as u32,
        if !flush_done {
            "L2T WRITE-BACK DID NOT COMPLETE — no touched/untouched verdict from this line at all; the bytes may still be parked in L2T (same caveat [v3d55] tilestate carries)"
        } else if touched == 0 {
            "POISON FULLY INTACT after a completed write-back: the PTB wrote NOTHING here — not primitives, not zeros, not a header. This is the first witness able to say that, because a pre-zeroed region could never distinguish 'never written' from 'written with zeros'"
        } else if s.overwritten == 0 {
            "POISON ZEROED (and only zeroed): the PTB DID write this region — with zero bytes. Every previous boot pre-zeroed the pool, so this write has been invisible for the entire V3D-40..55 series and the 'nothing was written' premise under the phantom-BPCA verdict is FALSE. Zero-valued tile-list output is what an EMPTY bin frame emits; cross-read the BPCA-semantics note below"
        } else {
            "POISON OVERWRITTEN with non-zero data: the PTB emitted real bytes here. The pool is NOT empty and the phantom-BPCA verdict is RETRACTED for this region — the defect is downstream of the write, at frame-close (FLDONE/BFC)"
        }
    );
}

// ── The landing-zone sweep (the in-lane form of the alias question) ────────────────────────────────
//
// The brief asks for a CPU read of the pool's physical page "through the other BCM2711 aliases — the
// 0xC0000000 uncached-SDRAM alias window and the 0x80000000 alias". That premise does not hold on this
// part, and acting on it would manufacture a false positive:
//
//   * The 0x0/0x4/0x8/0xC0000000 quadrant aliases are **VideoCore bus** addresses (the L1/L2 cache
//     behaviour selectors the VPU's MMU applies), not ARM physical addresses. They are what the
//     mailbox hands back for a firmware buffer, and what a VPU-side peripheral consumes.
//   * V3D on BCM2711 does not issue VC bus addresses. It issues IOVAs into its OWN MMU (V3D_MMU_CTL /
//     PT_PA_BASE — the table this driver publishes), and the translated result goes straight onto the
//     SoC fabric as a plain physical address. There is no VC-alias stage in that path.
//   * On a 4 GiB Pi 4 the ARM physical address 0xC0000000 is REAL, DISTINCT DRAM (BCM2711 places the
//     low peripheral window at 0xFC00_0000 and main peripherals at 0xFE00_0000; everything below is
//     contiguous RAM). Mapping it and finding nonzero bytes would prove only that some unrelated part
//     of the system owns that memory — it could not be evidence about our pool.
//   * Establishing such a mapping would require `memory.rs`, which is outside this arc's lane
//     (v3d.rs + v3d.md, mailbox additive) — a STOP tripwire even if the premise had held.
//
// The sound in-lane substitute is strictly stronger for the actual question ("where did the bytes
// land?"): the ONLY addresses the V3D MMU grants this job are the identity-mapped arena pages. If the
// PTB wrote anywhere the GPU can reach, the bytes are in the arena. So digest EVERY arena page across
// the bin window (immediately before the GO, again after the post-retire readback) and report which
// pages CHANGED. A change outside the pool + tile-state pages IS the phantom landing zone, named by
// page. A change in NO page, with BPCA advanced, is the phantom verdict standing — but now on a
// whole-address-space observation rather than an inference from two pre-zeroed regions.

/// Per-page digest of the whole arena: (nonzero-word count, order-sensitive checksum) per 4 KiB page.
/// Order-sensitive so a permutation of the same bytes still registers as a change.
struct V3d56ArenaDigest {
    pages: usize,
    sum: [u32; V3D56_DIGEST_MAX_PAGES],
}

/// Static cap on the digest table. `ARENA_BYTES / PAGE` today; the const assert below keeps it honest
/// if the arena ever grows.
const V3D56_DIGEST_MAX_PAGES: usize = 64;
const _: () = assert!(ARENA_BYTES / PAGE <= V3D56_DIGEST_MAX_PAGES);

/// Digest every arena page. Pure CPU reads of DRAM; the caller does the cache maintenance that makes
/// DRAM authoritative (pre-kick the arena is CPU-clean by construction, post-kick the L2T write-back +
/// `clean_invalidate_range` in the readback path have already run).
fn v3d56_arena_digest() -> V3d56ArenaDigest {
    let pages = ARENA_BYTES / PAGE;
    let mut d = V3d56ArenaDigest { pages, sum: [0u32; V3D56_DIGEST_MAX_PAGES] };
    let base = arena_phys();
    for p in 0..pages {
        let mut acc: u32 = 0x811c_9dc5; // FNV-1a offset basis; any order-sensitive mix would do
        for i in 0..(PAGE / 4) {
            let v = unsafe { core::ptr::read_volatile((base + p * PAGE + i * 4) as *const u32) };
            acc = (acc ^ v).wrapping_mul(0x0100_0193);
        }
        d.sum[p] = acc;
    }
    d
}

/// One whitelisted (legitimately-writable) arena region for the landing sweep, with the label the
/// witness prints when it changes.
struct V3d56Expected {
    label: &'static str,
    first: usize, // first page index, inclusive
    last: usize,  // last page index, inclusive
}

/// The regions a CORRECTLY-FUNCTIONING probe job is allowed to write. Anything outside these is the
/// phantom landing zone.
///
/// The first two are the bin outputs under test. The last two are the V3D-56 evidence-integrity fix:
/// they are *targets of the probe*, not anomalies, and whitelisting them is what keeps this witness
/// from crying "phantom" at success —
///
///   * `OFF_PROBE_SCRATCH` is the TMU-store target — the four words the probe's coord shader writes
///     and `probe_word` reads back. Producing that write is the entire reason the probe series exists
///     (V3D-28/V3D-35). A WORKING probe changes this page BY DEFINITION; flagging it STRAY would make
///     the sweep report its loudest failure verdict precisely when the GPU finally started working.
///     The canary tail lives in the same page and is scanned separately by the existing V3D-28 canary
///     check, which is the witness that actually adjudicates a misplaced store.
///   * `OFF_PROBE_BIN_OVERFLOW` is the PTB overspill pool (`BPOA`). A PTB that exhausts the initial
///     tile-alloc block and spills into its overflow block is doing exactly what the architecture
///     specifies; the `OUTOMEM` path exists to service it. That is an architectural write to a page
///     we handed the hardware on purpose, not a displaced one.
const V3D56_EXPECTED_REGIONS: [V3d56Expected; 4] = [
    V3d56Expected {
        label: "tile-state (bin output)",
        first: OFF_TILESTATE / PAGE,
        last: (OFF_TILESTATE + TILE_STATE_BYTES - 1) / PAGE,
    },
    V3d56Expected {
        label: "tile-alloc pool (bin output)",
        first: OFF_BIN_TILEALLOC / PAGE,
        last: (OFF_BIN_TILEALLOC + BIN_TILEALLOC_BYTES - 1) / PAGE,
    },
    V3d56Expected {
        label: "probe TMU scratch (expected)",
        first: OFF_PROBE_SCRATCH / PAGE,
        last: (OFF_PROBE_SCRATCH + PROBE_CANARY_BYTES - 1) / PAGE,
    },
    V3d56Expected {
        label: "PTB overspill (expected)",
        first: OFF_PROBE_BIN_OVERFLOW / PAGE,
        last: (OFF_PROBE_BIN_OVERFLOW + PROBE_BIN_OVERFLOW_BYTES - 1) / PAGE,
    },
];

/// Compare two arena digests and emit the `[v3d56] landing` witness. Changes inside
/// `V3D56_EXPECTED_REGIONS` are reported with their label; everything else is STRAY.
fn v3d56_emit_landing(tag: &str, pre: &V3d56ArenaDigest, post: &V3d56ArenaDigest) {
    let mut changed = 0u32;
    let mut expected = 0u32; // changes inside a whitelisted region
    let mut stray = 0u32; // changes ANYWHERE else — the phantom landing zone
    let mut first_stray: i32 = -1;
    let mut last_stray: i32 = -1;
    let mut hit = [0u32; V3D56_EXPECTED_REGIONS.len()]; // per-region changed-page counts
    let pages = pre.pages.min(post.pages);
    for p in 0..pages {
        if pre.sum[p] == post.sum[p] {
            continue;
        }
        changed += 1;
        let mut whitelisted = false;
        for (r, region) in V3D56_EXPECTED_REGIONS.iter().enumerate() {
            if p >= region.first && p <= region.last {
                hit[r] += 1;
                whitelisted = true;
                break;
            }
        }
        if whitelisted {
            expected += 1;
        } else {
            stray += 1;
            if first_stray < 0 {
                first_stray = p as i32;
            }
            last_stray = p as i32;
        }
    }
    serial_println!(
        ":: V3D: [v3d56] landing ({}) — arena {} pages ({:#x} B @ {:#010x}, the ENTIRE address space the V3D MMU grants this job) | changed={} expected={} STRAY={} | per-region: {} p[{}..{}]={} · {} p[{}..{}]={} · {} p[{}..{}]={} · {} p[{}..{}]={} | first_stray_page={} (off {:#x}) last_stray_page={} — {} ::",
        tag, pages, ARENA_BYTES, arena_phys(),
        changed, expected, stray,
        V3D56_EXPECTED_REGIONS[0].label, V3D56_EXPECTED_REGIONS[0].first, V3D56_EXPECTED_REGIONS[0].last, hit[0],
        V3D56_EXPECTED_REGIONS[1].label, V3D56_EXPECTED_REGIONS[1].first, V3D56_EXPECTED_REGIONS[1].last, hit[1],
        V3D56_EXPECTED_REGIONS[2].label, V3D56_EXPECTED_REGIONS[2].first, V3D56_EXPECTED_REGIONS[2].last, hit[2],
        V3D56_EXPECTED_REGIONS[3].label, V3D56_EXPECTED_REGIONS[3].first, V3D56_EXPECTED_REGIONS[3].last, hit[3],
        first_stray,
        if first_stray < 0 { 0 } else { first_stray as usize * PAGE },
        last_stray,
        if stray != 0 {
            "STRAY PAGE CHANGED across the bin window: bytes landed OUTSIDE every region this job legitimately writes (bin outputs, the probe TMU scratch, the PTB overspill pool), inside the mapped arena. Read the offset above against the arena layout to name the region, and against BPCA to derive the pointer's true base"
        } else if changed != 0 {
            "every changed page is a region this job is SUPPOSED to write (see the per-region counts) — nothing is displaced. Note this is NOT by itself a failure reading: a probe TMU-scratch or overspill hit is what SUCCESS looks like. The [v3d56] poison lines say what landed in the bin outputs"
        } else {
            "NO arena page changed across the bin window. Since the V3D MMU grants this job no other addresses, the PTB wrote NOWHERE the GPU can reach — BPCA advanced without any accompanying memory traffic at all. Read this together with the BPCA-semantics finding: an advance with no write is consistent with a pointer that is reserved/pre-allocated rather than consumed"
        }
    );
}

/// PI-V3D-56 (item 4): the interrupt enable/status/masked triple across the retire wait. Emitted from
/// `wait_fldone` on both exits. `msk` is the CURRENT mask read from `V3D_CTL_INT_MSK_STS`, where a SET
/// bit means MASKED (disabled) — the `MSK_SET`/`MSK_CLR` pair this driver has used since V3D-49.
/// `masked_out` is the delivery-relevant product: bits latched in STS that the mask is suppressing.
///
/// This discriminates the two readings of "FLDONE never fires": the flush never happens (STS bit1 never
/// latches, mask irrelevant), versus the flush happens but delivery is masked (STS bit1 latches while
/// MSK bit1 is set). Note `wait_fldone` polls INT_STS directly — the RAW latch, which the mask does NOT
/// gate — so a masked-but-latched FLDONE would already have been seen; this line puts that reasoning on
/// serial with the numbers instead of leaving it implicit.
///
/// **Scope (default-quiet).** Gated on `V3D56_INT_TRIPLE`, and deliberately fired on EVERY `wait_fldone`
/// exit rather than only the probe's. That is one line per bin kick, which is the intended cost: every
/// caller of `wait_fldone` *is* a bin-retire wait (the probe, the §22 bisect rungs, the §28 resubmit,
/// M4), the arc's whole remaining question is why FLDONE never asserts, and the bisect rungs are exactly
/// where a rung-to-rung difference in the mask or the latch would show. Restricting it to the probe
/// would blind the comparison that matters most. The enclosing V3D battery is itself default-quiet — it
/// returns at `BLOCK-DOWN` wherever no V3D block exists (all of QEMU), so none of this reaches a
/// non-metal log.
const V3D56_INT_TRIPLE: bool = true;

fn v3d56_emit_int_triple(what: &str, sts_entry: u32, msk_entry: u32, sts_exit: u32, msk_exit: u32) {
    if !V3D56_INT_TRIPLE {
        return;
    }
    let masked_out_exit = sts_exit & msk_exit;
    serial_println!(
        ":: V3D: [v3d56] int ({}) — ENTRY INT_STS={:#010x} INT_MSK_STS={:#010x} | EXIT INT_STS={:#010x} INT_MSK_STS={:#010x} (1=MASKED) | FLDONE(bit1): latched={} masked={} | working-set V3D_CORE_IRQS={:#010x} unmasked_now={:#010x} | latched-but-masked={:#010x} — {} ::",
        what, sts_entry, msk_entry, sts_exit, msk_exit,
        (sts_exit & V3D_INT_FLDONE != 0) as u32,
        (msk_exit & V3D_INT_FLDONE != 0) as u32,
        V3D_CORE_IRQS,
        !msk_exit & V3D_CORE_IRQS,
        masked_out_exit,
        if sts_exit & V3D_INT_FLDONE != 0 {
            "FLDONE LATCHED in the raw status — the flush completed; whatever else is wrong, it is not the flush"
        } else if msk_exit & V3D_INT_FLDONE != 0 {
            "FLDONE is MASKED — but INT_STS is the RAW latch and the mask does not gate it, so a masked FLDONE would still have shown here. The mask is not the reason bit1 is clear; the flush genuinely did not complete. (Unmask it anyway for the CPU-delivery path: MSK_CLR = V3D_CORE_IRQS.)"
        } else {
            "FLDONE is UNMASKED and NOT latched across the whole wait: the flush never completed. The wall is the flush/frame-close unit itself, not interrupt routing — no mask, enable or delivery change can move this"
        }
    );
}

/// PI-V3D-54 (RANK 1): poll the CL-progression time-series across the retire-wait (not once after it).
/// `trace_ba`/`trace_ea` are the submitted CL bounds for the `[v3d54] trace` fold; sampling runs at ~1 ms
/// cadence and is summarised (transitions, not raw samples) on BOTH the FLDONE-retire and timeout exits.
fn wait_fldone(what: &str, trace_ba: u32, trace_ea: u32) -> (u32, u64, bool) {
    let frq = super::timer::cntfrq();
    let start = super::timer::cntpct();
    let deadline = start + frq / 2; // ~0.5 s, matching wait_bit_clear's backstop
    // PI-V3D-54: ~1 ms sampling cadence in counter units; fold the five CL/PTB progression registers.
    let sample_tick = (frq / 1000).max(1);
    let mut next_sample = start;
    let mut samples: u32 = 0;
    let mut t_ca = TraceReg::new(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA));
    let mut t_cs = TraceReg::new(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS));
    let mut t_lc = TraceReg::new(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0LC));
    let mut t_pc = TraceReg::new(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0PC));
    let mut t_bpca = TraceReg::new(mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA));
    // PI-V3D-55 (RANK 3a): fold CYCLE_COUNT (PCTR counter 2, src32) on the SAME cadence — the delta
    // across this wait is the clock-domain liveness verdict. The enable mask is captured once so the
    // verdict can say "not armed" instead of fabricating a flat-clock reading on an unarmed wait.
    let pctr_en = mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN);
    let mut t_cyc = TraceReg::new(mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 8));
    // PI-V3D-56 (item 4): capture the interrupt status/mask pair at wait ENTRY so the exit line can
    // report the triple across the window rather than a single end-of-wait snapshot.
    let sts_entry = mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS);
    let msk_entry = mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_MSK_STS);
    loop {
        let now = super::timer::cntpct();
        // PI-V3D-54: fold a progression sample at the ~1 ms cadence (bounded work — five reads, no logging).
        if now >= next_sample {
            let us = (now.wrapping_sub(start)).saturating_mul(1_000_000) / frq.max(1);
            let us32 = us.min(u32::MAX as u64) as u32;
            t_ca.update(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA), us32);
            t_cs.update(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS), us32);
            t_lc.update(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0LC), us32);
            t_pc.update(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0PC), us32);
            t_bpca.update(mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA), us32);
            t_cyc.update(mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 8), us32); // [v3d55] RANK 3a
            // PI-V3D-71 leg 2: fold one ASB fabric sample at the same cadence — a no-op (one static
            // bool read, zero MMIO) on every wait except the one the [v3d71] rung arms it across.
            v3d71_sampler_take();
            // PI-V3D-73: fold one CLE-progress sample at the same cadence — same contract (one
            // static bool read, zero MMIO, on every wait except the ones a rung arms it across).
            v3d73_sampler_take();
            samples = samples.saturating_add(1);
            next_sample = now + sample_tick;
        }
        let sts = mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS);
        if sts & V3D_INT_FLDONE != 0 {
            let waited_us = (super::timer::cntpct().wrapping_sub(start)).saturating_mul(1_000_000) / frq.max(1);
            // PI-V3D-56 (item 4): the enable/status/masked triple — read the mask BEFORE the W1C below.
            v3d56_emit_int_triple(what, sts_entry, msk_entry, sts, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_MSK_STS));
            // W1C the latched status so a stale FLDONE from a prior kick never masquerades as this one's.
            mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, sts);
            dsb();
            emit_v3d54_trace(what, trace_ba, trace_ea, samples, waited_us, &t_ca, &t_cs, &t_lc, &t_pc, &t_bpca);
            emit_v3d55_clock_liveness(what, pctr_en, &t_cyc, waited_us, samples);
            return (sts, waited_us, true);
        }
        if super::timer::cntpct() >= deadline {
            let waited_us = (super::timer::cntpct().wrapping_sub(start)).saturating_mul(1_000_000) / frq.max(1);
            // PI-V3D-56 (item 4): the triple on the TIMEOUT exit too — this is the exit the empty rung
            // actually takes, so it is the one that has to name flush-vs-delivery.
            //
            // Re-read INT_STS here rather than reusing the loop's `sts`: that sample was taken before
            // the deadline check and can be up to one poll-iteration stale. On the exit whose whole
            // claim is "FLDONE never latched across the entire wait", a stale final sample could miss a
            // bit that set in the last microseconds and turn a near-miss into a false negative. The
            // mask is read at the same instant so the pair is coherent. WITSWEEP: the [v3d44] line
            // below (and the timeout return value) now print/carry this fresh `sts_final` sample —
            // previously the fresh read fed only the [v3d56] triple and was then discarded, so the
            // timeout line and every downstream inheritor reported the provably-stale loop `sts`.
            let sts_final = mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS);
            v3d56_emit_int_triple(what, sts_entry, msk_entry, sts_final, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_MSK_STS));
            let qpu_vec = (sts_final & V3D_INT_QPU_MASK) >> V3D_INT_QPU_SHIFT; // which QPU(s) raised a host int
            serial_println!(
                ":: V3D: [v3d44] FLDONE timeout ({}) — INT_STS={:#010x} (FRDONE={} FLDONE={} OUTOMEM={} SPILLUSE={} TRFB={} GMPV={} QPU_vec={:#06x}) — {} ::",
                what, sts_final,
                (sts_final & V3D_INT_FRDONE != 0) as u32,
                (sts_final & V3D_INT_FLDONE != 0) as u32,
                (sts_final & V3D_INT_OUTOMEM != 0) as u32,
                (sts_final & V3D_INT_SPILLUSE != 0) as u32,
                (sts_final & V3D_INT_TRFB != 0) as u32,
                (sts_final & V3D_INT_GMPV != 0) as u32,
                qpu_vec,
                if sts_final & V3D_INT_OUTOMEM != 0 {
                    "OUTOMEM fired: the binner stalled waiting for overflow tile-alloc memory — the pre-armed BPOA/BPOS block was too small or not honoured"
                } else if sts_final & V3D_INT_GMPV != 0 {
                    "GMPV fired: a GMP memory-protection violation blocked the flush"
                } else if sts_final & V3D_INT_QPU_MASK != 0 {
                    "QPU host-interrupt latched but NO FLDONE: the coord shader ran to a program-end-interrupt yet the PTB never flushed — the QPU signalled completion while the bin pipeline stayed open (see [v3d45] CLE/PTB dump for the wedge)"
                } else if sts_final == 0 {
                    "no interrupt latched at all: the flush never even began to retire (chase START_TILE_BINNING / PTB bring-up)"
                } else {
                    "an interrupt other than FLDONE latched — see the raw bits above"
                }
            );
            // [v3d45] — corner the binner: dump the CLE + PTB state machine at the timeout so ONE more boot
            // names the wedge. Every offset below is already fact-verified in this file (v3d_regs.h lifts,
            // PI-V3D-7/8/9/13/41). CT{0,1}CS carry the CTRUN busy bit (bit5); CT{0,1}CA are the addresses the
            // CLE halted at; PCS is the raw pipeline control/status (bin/render busy+empty); CT0LC/CT0PC are
            // the list/primitive counters the CLE fed; BPCA/BPCS the PTB write pointer + size. A binner that
            // emitted bytes (BPCA off pool base) but left CTRUN set / PCS busy while the QPU already raised its
            // program-end interrupt is a wedged-QPU-holding-the-bin-open signature — exactly the null hypothesis.
            let ct0cs = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
            let ct1cs = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
            let ct0ca = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
            let ct1ca = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
            let pcs = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
            let ct0lc = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0LC);
            let ct0pc = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0PC);
            let bpca = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA);
            let bpcs = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS);
            serial_println!(
                ":: V3D: [v3d45] wedge dump — CT0CS={:#010x}(CTRUN={}) CT0CA={:#010x} | CT1CS={:#010x}(CTRUN={}) CT1CA={:#010x} | PCS={:#010x}(raw) CT0LC={:#010x} CT0PC={:#010x} | BPCA={:#010x} BPCS={:#010x} — {} ::",
                ct0cs, (ct0cs & V3D_CLE_CTNCS_CTRUN != 0) as u32, ct0ca,
                ct1cs, (ct1cs & V3D_CLE_CT1CS_CTRUN != 0) as u32, ct1ca,
                pcs, ct0lc, ct0pc, bpca, bpcs,
                if ct0cs & V3D_CLE_CTNCS_CTRUN != 0 {
                    "CT0 CTRUN still set: the BIN CLE never idled — the PTB is holding the bin list open (wedged QPU / unflushed primitive list)"
                } else {
                    "CT0 CTRUN clear: the BIN CLE idled but FLDONE never latched — flush stalled DOWNSTREAM of the CLE (PTB drain / tile-state writeback)"
                }
            );
            // [v3d46] — full PCS decode (the brief's ask). PCS bits are now named (see the V3D_PCS_* facts
            // block): a BMACTIVE-set / BMBUSY-clear read is the exact P44 signature — binning mode is still
            // ACTIVE (the frame never tore down) yet nothing is in progress and the pool is not exhausted.
            serial_println!(
                ":: V3D: [v3d46] PCS decode — PCS={:#010x} BMACTIVE={} BMBUSY={} RMACTIVE={} RMBUSY={} BMOOM={} — {} ::",
                pcs,
                (pcs & V3D_PCS_BMACTIVE != 0) as u32,
                (pcs & V3D_PCS_BMBUSY != 0) as u32,
                (pcs & V3D_PCS_RMACTIVE != 0) as u32,
                (pcs & V3D_PCS_RMBUSY != 0) as u32,
                (pcs & V3D_PCS_BMOOM != 0) as u32,
                if pcs & V3D_PCS_BMOOM != 0 {
                    "BMOOM set: the PTB exhausted its tile-alloc pool — the wedge is overflow starvation (feed BPOA/BPOS)"
                } else if (pcs & V3D_PCS_BMACTIVE != 0) && (pcs & V3D_PCS_BMBUSY == 0) {
                    "BMACTIVE set + BMBUSY clear: binning mode held OPEN with no op in progress — the bin frame never retired though FLUSH was consumed; the coord-shader thread-end handshake to the PTB is the named suspect (QPU raised a program-end host interrupt, INT_STS bit16, yet the PTB never closed the frame)"
                } else if pcs & V3D_PCS_BMBUSY != 0 {
                    "BMBUSY still set: a bin op is genuinely still in progress — the binner has not finished emitting"
                } else {
                    "BMACTIVE clear: binning mode already torn down — the missing FLDONE is a stale-read / IRQ-mask issue, not a live wedge"
                }
            );
            // [v3d46] — hex-dump the exact TILE_BINNING_MODE_CFG packet bytes we submit, so P45 confirms on
            // metal that the config the binner ran matches the Mesa v42 contract (audited byte-for-byte in
            // V3D-46: 8/8 fields MATCH — width/height minus_one, RT count minus_one, 128B initial + 64B
            // overflow block, 32-bit max BPP). Rebuilt here from the SAME Pkt path build_bin_cl_generic uses.
            dump_bin_mode_cfg_bytes();
            // Also surface the GMP block state — a silent GMP write-drop leaves STATUS.VIO set with no MMU fault
            // (see the V3D_GMP_* facts block); reading it here rules that class in or out in the same boot.
            let gmp_status = mmio_read(V3D_CORE0_BASE, V3D_GMP_STATUS);
            let gmp_cfg = mmio_read(V3D_CORE0_BASE, V3D_GMP_CFG);
            serial_println!(
                ":: V3D: [v3d45] GMP witness — STATUS={:#010x}(VIO={} INVPROT={} GMPRST={}) CFG={:#010x}(PROT_ENABLE={}) — {} ::",
                gmp_status,
                (gmp_status & V3D_GMP_STATUS_VIO != 0) as u32,
                (gmp_status & V3D_GMP_STATUS_INVPROT != 0) as u32,
                (gmp_status & V3D_GMP_STATUS_GMPRST != 0) as u32,
                gmp_cfg, (gmp_cfg & V3D_GMP_CFG_PROT_ENABLE != 0) as u32,
                if gmp_status & V3D_GMP_STATUS_VIO != 0 {
                    "GMP VIO latched: a memory-protection violation silently dropped an access — a candidate for the missing flush"
                } else {
                    "GMP clean: no protection violation latched — the wedge is not a GMP drop"
                }
            );
            emit_v3d54_trace(what, trace_ba, trace_ea, samples, waited_us, &t_ca, &t_cs, &t_lc, &t_pc, &t_bpca);
            // PI-V3D-55 (RANK 3a): the timeout path is where the clock verdict matters most — 0.5 s of
            // wall clock elapsed with no flush; did the core clock advance at all across it?
            emit_v3d55_clock_liveness(what, pctr_en, &t_cyc, waited_us, samples);
            // WITSWEEP: return the fresh timeout-instant sample, not the loop's stale `sts` — the
            // callers that print this value ([v3d44] FLDONE wait, the bisect ladder, [v3d54]
            // resubmit) inherit evidence, and stale evidence poisons every one of them.
            return (sts_final, waited_us, false);
        }
        core::hint::spin_loop();
    }
}

/// PI-V3D-10: decode the V3D MMU violation witness pair into (client name, true VA). Hardware facts
/// from Linux drm/v3d (facts-only, no code lifted): the violating AXI client is VIO_ID >> 5 indexing
/// {L2T, PTB, PSE, TLB, CLE, TFU, MMU, GMP} on V3D 4.1+ (v3d_irq.c), and VIO_ADDR holds the VA
/// right-shifted by (va_width − 32), where va_width = 30 + DEBUG_INFO[7:4] (v3d_drv.c). Boot-P6
/// ground truth: DEBUG_INFO 0x550 → va_width 35 → shift 3; VIO_ADDR 0x04841800 → VA 0x2420C000.
fn vio_decode(vio_id: u32, vio_addr: u32) -> (&'static str, u64) {
    const CLIENTS: [&str; 8] = ["L2T", "PTB", "PSE", "TLB", "CLE", "TFU", "MMU", "GMP"];
    let client = CLIENTS[((vio_id >> 5) & 0x7) as usize];
    let dbg = mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO);
    let va_width = 30 + ((dbg >> 4) & 0xF) as u64;
    let shift = va_width.saturating_sub(32);
    (client, (vio_addr as u64) << shift)
}

/// PI-V3D-9: clear any latched V3D-MMU translation fault (PT_INVALID / WRITE_VIOLATION / CAP_EXCEEDED),
/// mirroring Linux `v3d_irq.c`: read V3D_MMU_CTL and write it straight back — the fault-status bits are
/// write-1-to-clear, so echoing the read value clears the latched fault while the ENABLE/abort config
/// bits (also echoed) are preserved. Reports whether a fault was actually latched (the witness the
/// attended sitting reads to correlate a render-kick refusal with a sticky bin fault). Reads-then-one-
/// write; cannot fault or hang (QEMU-safe — the CTL reads 0/absent there and the write-back is inert).
fn clear_mmu_fault_latch(when: &str) {
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    if fault != 0 {
        let vio_addr = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
        let vio_id = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ID);
        mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, ctl); // W1C: echo clears the sticky fault bits
        dsb();
        let after = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
        let (client, va) = vio_decode(vio_id, vio_addr);
        serial_println!(
            ":: V3D: MMU fault-latch CLEARED ({}) — was CTL={:#010x} (PT_INVALID={} WRITE_VIOLATION={} CAP_EXCEEDED={}) VIO_ADDR={:#010x} VIO_ID={:#010x} (client {} @ VA {:#010x}) -> CTL={:#010x} ::",
            when, ctl,
            (fault & V3D_MMU_CTL_PT_INVALID != 0) as u32,
            (fault & V3D_MMU_CTL_WRITE_VIOLATION != 0) as u32,
            (fault & V3D_MMU_CTL_CAP_EXCEEDED != 0) as u32,
            vio_addr, vio_id, client, va, after
        );
    } else {
        serial_println!(":: V3D: MMU fault-latch clear ({}) — none latched (CTL={:#010x}) ::", when, ctl);
    }
}

/// PI-V3D-12: the pre-kick GPU-cache invalidate — the Linux `v3d_invalidate_caches` idiom every job
/// submission runs (v3d_sched.c calls it in BOTH `v3d_bin_job_run` and `v3d_render_job_run`). On the
/// Pi 4's V3D 4.2 the two live steps are:
///   (1) L2T flush (L2TCACTL <= L2TFLS | FLM=FLUSH): write back + invalidate the L2T — this is what
///       PUBLISHES a prior GPU engine's memory writes (the PTB's binned tile lists) to the next
///       engine's fetch path, and drops any stale line caching the CPU's pre-job contents;
///   (2) slice-cache invalidate (SLCACTL <= all-0xF): drop the per-slice TMU/uniform/instruction
///       caches so shaders fetch current code/uniforms.
/// The L2TFLS wait is the standard finite backstop (Linux waits on the same bit in v3d_clean_caches);
/// a timeout is logged and the caller proceeds — the kick's own witnesses stay decisive. Boot-P7 root
/// cause (PI-V3D-12): our driver never did ANY of this. M3 survived because its CT1 only ever read
/// CPU-published lists (CPU-side cache cleans cover CPU→GPU); M4's render is the FIRST job whose CLE
/// must observe ANOTHER GPU job's output (the bin's tile lists) — with the L2T never flushed, the
/// BRANCH_TO_IMPLICIT_TILE_LIST fetch at the tile-alloc base returned the stale pre-bin zero-fill,
/// opcode 0x00 = Halt, and the CLE stopped inside the pool (CT1CA parked BELOW BA) before ever
/// reaching the sub-list's STORE — render "clean", zero stores.
fn invalidate_gpu_caches(what: &str) {
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_FLUSH);
    let _ = wait_bit_clear(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS, what);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
    dsb();
}

/// PI-V3D-53: the kernel-EXACT per-job INPUT cache invalidate — the last L2TCACTL flush-mode / sequence
/// divergence around a bin job. The kernel's `v3d_invalidate_caches` (v3d_gem.c, GPL-2.0-only, facts-only)
/// is `v3d_flush_l3` (no-op on BCM2711 — no L3) → `v3d_invalidate_slices` (SLCACTL all-0xF) →
/// `v3d_invalidate_l2t` (`L2TFLSTA=0`; `L2TFLEND=~0`; `L2TCACTL = L2TFLS | FLM=CLEAR`). It differs from
/// `invalidate_gpu_caches` (above) in THREE ways: (a) SLCACTL runs FIRST, then L2T (we do L2T then SLCACTL);
/// (b) the flush window is re-established per invalidate (we set it once in `v3d_init_hw_state`);
/// (c) **FLM=CLEAR** (invalidate-only) — NOT FLM=FLUSH (writeback+invalidate). For a bin job the inputs
/// (CL, tile-state, shader records, VBO) are ALL CPU-published to DRAM via `cache::clean_range`, so an
/// invalidate-only re-fetch is exactly correct and byte-faithful to the kernel; on a freshly-reset core the
/// L2T holds no dirty lines, so CLEAR and FLUSH converge — faithful-but-weak-prior (like §26 Rung 1),
/// mirror-exact-then-metal-decides. Distinct from the render-side `invalidate_gpu_caches` FLM=FLUSH, which
/// is the metal-CONFIRMED V3D-12 fix (it PUBLISHES the binner's tile lists to the render CLE) and is left
/// untouched. Returns L2TCACTL before/after for the `[v3d53]` witness. The poll on L2TFLS is our standard
/// finite backstop (the kernel's per-job invalidate is fire-and-forget; polling is a safe superset).
fn bin_prejob_invalidate_kernel_exact(what: &str) -> (u32, u32) {
    let before = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TCACTL);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TFLSTA, 0);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TFLEND, !0);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_CLEAR);
    let _ = wait_bit_clear(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS, what);
    let after = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TCACTL);
    dsb();
    (before, after)
}

/// PI-V3D-49: unmask the core interrupt working set once at bring-up, byte-for-byte as the kernel's
/// `v3d_irq_enable` (`drivers/gpu/drm/v3d/v3d_irq.c`, ver<71 path):
///   V3D_CORE_WRITE(core, V3D_CTL_INT_MSK_SET, ~V3D_CORE_IRQS(ver));  // mask everything else
///   V3D_CORE_WRITE(core, V3D_CTL_INT_MSK_CLR,  V3D_CORE_IRQS(ver));  // unmask our set (incl. FLDONE)
/// The kernel does this ONCE at probe — not per job — so every bin frame runs with FLDONE unmasked.
/// Our driver had never programmed the mask, so the block ran every frame at the mask's power-on-reset
/// value: the one frame-level enable that the per-packet audits (§19–22) never covered, because they
/// were per-packet. `wait_fldone` polls INT_STS (the raw latched vector, mask-independent), so this
/// does NOT change our polling contract; it makes the block kernel-faithful and — should this silicon
/// gate FLDONE *generation* (not just delivery) on the unmask — is the fix the empty-frame verdict
/// (§23) points at. Idempotent; no ISR is installed (we poll), so unmasking bits we don't IRQ-service
/// is safe. Reads MSK_STS before/after so the P46 metal boot records the power-on-reset mask state —
/// which itself settles whether FLDONE was masked at reset (the inversion candidate).
fn v3d_irq_enable() {
    let msk_por = mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_MSK_STS);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_MSK_SET, !V3D_CORE_IRQS);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_MSK_CLR, V3D_CORE_IRQS);
    dsb();
    let msk_now = mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_MSK_STS);
    serial_println!(
        ":: V3D: [v3d49] irq-enable — MSK_STS por={:#010x} (FLDONE {}) -> now={:#010x} (FLDONE {}) unmasked set={:#010x} — {} ::",
        msk_por,
        if msk_por & V3D_INT_FLDONE != 0 { "MASKED" } else { "unmasked" },
        msk_now,
        if msk_now & V3D_INT_FLDONE != 0 { "MASKED" } else { "unmasked" },
        V3D_CORE_IRQS,
        if msk_por & V3D_INT_FLDONE != 0 {
            "FLDONE was MASKED at power-on-reset — every prior boot polled a gated retire signal; this unmask is the empty-frame fix"
        } else {
            "FLDONE was already unmasked at reset — INT_STS latches raw, so masking never inverted the FLDONE=0 verdict; the wedge is downstream of the frame enables"
        }
    );

    // PI-V3D-52 (Rung 1): the HUB-INT half of `v3d_irq_enable` — the second, last unmirrored byte-exact
    // kernel probe write. The kernel unmasks BOTH the per-core set (above) AND the hub set once at probe;
    // UnaOS mirrored only the core half (V3D-49). If this silicon gates the CLE→PTB frame-close FLDONE
    // latch behind the hub's aggregate interrupt path (the hub aggregates the core's completion signal),
    // unmasking the hub working set is what lets the Empty frame retire. Weakened by §23 (raw INT_STS
    // latches regardless of the CORE mask, so the hub likely latches raw too) — faithful-but-maybe-not-
    // the-fix — but it is the LAST kernel divergence; the method is "mirror exactly, then metal decides."
    // Idempotent; no hub ISR is installed (we poll core INT_STS), so unmasking bits we don't service is
    // safe. Reads MSK_STS before/after so the boot records the hub mask's power-on-reset state.
    let hub_msk_por = mmio_read(V3D_HUB_BASE, V3D_HUB_INT_MSK_STS);
    mmio_write(V3D_HUB_BASE, V3D_HUB_INT_MSK_SET, !V3D_HUB_IRQS);
    mmio_write(V3D_HUB_BASE, V3D_HUB_INT_MSK_CLR, V3D_HUB_IRQS);
    dsb();
    let hub_msk_now = mmio_read(V3D_HUB_BASE, V3D_HUB_INT_MSK_STS);
    serial_println!(
        ":: V3D: [v3d52] hub-irq-enable — HUB_MSK_STS por={:#010x} -> now={:#010x} unmasked set={:#010x} (MMU_WRV|MMU_PTI|MMU_CAP|TFUC) — mirrors the hub half of the kernel v3d_irq_enable (was NEVER written before V3D-52; the last byte-exact probe divergence). {} ::",
        hub_msk_por,
        hub_msk_now,
        V3D_HUB_IRQS,
        if hub_msk_por & V3D_HUB_IRQS == V3D_HUB_IRQS {
            "hub working set was fully MASKED at reset — this unmask is a genuine state change; if the hub gates the core FLDONE latch, the Empty frame now retires"
        } else {
            "hub working set was NOT fully masked at reset — firmware left part of it open; unmasking completes the kernel-exact set regardless"
        }
    );
}

/// PI-V3D-51: the post-reset core init step the kernel runs after EVERY reset and UnaOS never did.
///
/// The kernel's `v3d_reset_v3d` (`v3d_gem.c`) ends by calling `v3d_init_hw_state` → `v3d_init_core`,
/// which — for EVERY V3D version, unconditionally — writes the L2T flush ADDRESS RANGE:
///     V3D_CORE_WRITE(core, V3D_CTL_L2TFLSTA, 0);
///     V3D_CORE_WRITE(core, V3D_CTL_L2TFLEND, ~0);
/// (The ver<41-only MISCCFG=OVRTMUOUT write is a SEPARATE, conditional step — the one §24's audit table
/// accounted for; the L2TFL* pair it MISSED.) STA=0/END=~0 establishes the flush window as the WHOLE
/// address space, so every subsequent `V3D_CTL_L2TCACTL` FLM=FLUSH (our `invalidate_gpu_caches`, run
/// before every kick, and the frame-completion write-back the binner drives) walks the full range.
///
/// UnaOS's V3D-50 OFF→ON power-cycle returns L2TFLSTA/L2TFLEND to their power-on-reset value; nothing
/// re-established them, so every per-kick L2T flush operated over an UNESTABLISHED range. This is the
/// first divergence BELOW the byte-exact per-job programming (§10–23) and the reset cycle (§24): the
/// bin frame's flush/write-back depends on an L2T flush window the kernel guarantees and we never set.
///
/// Sequenced to mirror the kernel: AFTER the reset cycle and BEFORE the MMU re-program (kernel order is
/// `v3d_reset_v3d`[incl. init_hw_state] → MMU reinit). Core-relative writes, so it runs only after the
/// BLOCK-UP probe verdict (an absent block would abort a core read). Echoes both registers back so the
/// P48 metal boot confirms the CLE latched the window. Idempotent; QEMU raspi4b returns at BLOCK-DOWN
/// before this point, so the step is dormant there — metal decides whether it unwedges the empty frame.
fn v3d_init_hw_state() {
    let sta_por = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TFLSTA);
    let end_por = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TFLEND);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TFLSTA, 0);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TFLEND, !0);
    dsb();
    let sta_now = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TFLSTA);
    let end_now = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TFLEND);
    serial_println!(
        ":: V3D: [v3d51] init-hw-state — L2TFLSTA por={:#010x}->{:#010x} (want 0) | L2TFLEND por={:#010x}->{:#010x} (want 0xffffffff) — kernel `v3d_init_core` writes STA=0/END=~0 unconditionally after EVERY reset; the L2T flush window our per-kick FLM=FLUSH walks was at POR, never established — {} ::",
        sta_por, sta_now, end_por, end_now,
        if sta_por == 0 && end_por == !0 {
            "POR already matched the kernel window (firmware left it established) — this write is a no-op and the empty-frame wedge sits below the L2T flush range too"
        } else {
            "POR did NOT match the kernel window — the reset left the L2T flush range unestablished; this is the missing v3d_init_hw_state step and the empty-frame fix candidate"
        }
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-8 — M4: the first triangle (bin on CT0 → render on CT1 → CPU sample-verify).
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// M4 adds the BINNING side of the pipeline (CT0), which M1–M3 never exercised, then a render pass that
// CONSUMES the binner's per-tile lists via BRANCH_TO_IMPLICIT_TILE_LIST. The shape (single 64×64 tile,
// one supertile, one triangle) is the minimal thing that puts real geometry + shaders through the GPU.
//
// ── PACKET FACTS (CL side — fully cited, correct-by-construction) ────────────────────────────────
// All binning-side opcodes / field bit-layouts below are transcribed VERBATIM from Mesa
// `src/broadcom/cle/v3d_packet.xml` (gen 4.2, `min_ver="42"` — the V3D 4.2 variants; identical to the
// v3d_packet_v33.xml `max_ver=42` set the M3 render list uses). Emission ORDER follows Mesa
// `src/gallium/drivers/v3d/v3dx_draw.c` (`v3dX(start_binning)` prologue + `v3dX(draw_vbo)` draw emit)
// and `v3dx_rcl.c` for the render side. Mesa is MIT-licensed — verbatim-liftable WITH attribution
// (memory: unaos-license-gplv3). No Linux-kernel (GPL-2.0-only) CLE source is used; only register
// OFFSETS are lifted from the kernel v3d_regs.h (hardware facts).
//
// ── QPU SHADER FACTS (the metal-refinement surface — honestly flagged) ───────────────────────────
// A binned+rendered triangle needs THREE QPU programs: a COORDINATE shader (binning: transform the
// vertices, write clip/screen coords to the VPM so the PTB can bin them), a VERTEX shader (render:
// same transform + emit varyings), and a FRAGMENT shader (write the solid colour to the TLB). Mesa
// COMPILES these from NIR through its VIR→QPU backend; it does not ship pre-assembled blobs, and QEMU
// `raspi4b` models no V3D, so NONE of this can be exercised or byte-checked off-metal. Rather than
// FABRICATE QPU words (the exact trap that convicted PI-V3D-4's MMU constants and PI-V3D-7's queue
// offsets — twice), PI-V3D-9 generates every shader word with Mesa's OWN packer
// (`v3d_qpu_instr_pack`, ver=42) from explicit instruction structs, round-trips each through Mesa's
// unpacker, and cross-checks the generator against four canonical `qpu_disasm.c` vectors bit-exactly
// (see the "VERIFIED QPU shader bodies" block below for the full provenance + the honest split between
// Mesa-verified ENCODING and the silicon-tuned geometry/colour quantities that remain the attended-
// metal-refinement surface). `triangle_job` witnesses the CT0 bin discriminator and the CT1 render
// regardless, and the CPU sample-verify reports exactly which samples matched, so the sitting is
// decisive.

// ─── Binning-side + shared packet opcodes (v3d_packet.xml `code=`). ───
const P_FLUSH: u8 = 4; // Flush — terminates the binning list (binner-done signal)
const P_START_TILE_BINNING: u8 = 6; // must follow the bin-mode config before geometry
const P_BRANCH_TO_IMPLICIT_TILE_LIST: u8 = 21; // render: run the binner's per-tile list for this tile
const P_VERTEX_ARRAY_PRIMS: u8 = 36; // non-indexed draw
const P_GL_SHADER_STATE: u8 = 64; // points at the GL Shader State Record + attribute records
const P_VCM_CACHE_SIZE: u8 = 71;
const P_OCCLUSION_QUERY_COUNTER: u8 = 92; // v3d_packet.xml code 92 — addr 0 = disable stale OQ state
const P_NUMBER_OF_LAYERS: u8 = 119;
const P_TILE_BINNING_MODE_CFG: u8 = 120; // v42 variant (max_ver=42)
// PI-V3D-17 — clip/viewport/config state (v3d_packet.xml, gen 4.2). Codes transcribed VERBATIM:
//   Cfg Bits code=96 (max_ver=42), clip_window code=107, Viewport Offset code=108,
//   Clipper XY Scaling code=110 (max_ver=42), Clipper Z Scale and Offset code=111.
const P_CFG_BITS: u8 = 96; // "Cfg Bits" (max_ver=42) — facing/cull + rasterizer config
const P_CLIP_WINDOW: u8 = 107; // "clip_window" — scissor/clip rect in pixels
const P_VIEWPORT_OFFSET: u8 = 108; // "Viewport Offset" — screen-space centre (coarse int + fine u14.8)
const P_CLIPPER_XY_SCALING: u8 = 110; // "Clipper XY Scaling" (max_ver=42) — half w/h in 1/256 px, f32
const P_CLIPPER_Z_SCALE_AND_OFFSET: u8 = 111; // "Clipper Z Scale and Offset" — z scale/offset, f32

const V3D_PRIM_TRIANGLES: u64 = 4; // VERTEX_ARRAY_PRIMS "mode" (enum Primitive) — NOT the PRIM_LIST value
// PI-V3D-57 (confirmed divergence #1): the tile-STATE data array is **256 bytes per tile** on v42, not
// the 48 this constant assumed (48·4 = 192 B — UNDER-sized for even our single tile). Authority: Mesa
// `v3d_tile_alloc_sizes` (src/broadcom/common/v3d_util.c), whose closing line is
//     *tile_state_size = layers * tiles_x * tiles_y * 256;
// and which every v42 emitter (gallium `alloc_tile_state`, v3dv `cmd_buffer_ensure_tile_state`) sizes
// the CT0QTS BO with. The 48 came from the per-tile TSDA *record* size, which is not what the PTB is
// handed. A 64×64 target with a 64×64 tile (1 RT, 32 bpp, no MSAA → Mesa's largest tile) is exactly one
// tile, so the correct array is 1·1·1·256 = 256 B. Fits the same dedicated page at OFF_TILESTATE
// (0x11000, next region 0x12000), so poison/scan/zero coverage simply grows to the real extent.
const TILE_STATE_TILES: usize = 1; // layers · tiles_x · tiles_y for the 64×64 target (one 64×64 tile)
const TILE_STATE_BYTES_PER_TILE: usize = 256; // Mesa v3d_tile_alloc_sizes: tile_state = tiles · 256
const TILE_STATE_BYTES: usize = TILE_STATE_TILES * TILE_STATE_BYTES_PER_TILE; // TSDA: 256 B
const _: () = assert!(OFF_TILESTATE + TILE_STATE_BYTES <= OFF_BIN_TILEALLOC);
// Mesa's minimum tile-allocation pool for the same frame (v3d_tile_alloc_sizes): the PTB reserves the
// INITIAL block per tile at START_TILE_BINNING, the driver rounds that to 4 KiB and adds the two 4 KiB
// chunks the PTB grabs before OOM can fire. Our BIN_TILEALLOC_BYTES (32 KiB) must be at least this.
const MESA_MIN_TILE_ALLOC_BYTES: usize =
    (TILE_STATE_TILES * 128).next_multiple_of(4096) + 8192; // = 12 KiB for one tile
const _: () = assert!(BIN_TILEALLOC_BYTES >= MESA_MIN_TILE_ALLOC_BYTES);

// PI-V3D-23: the VCM (Vertex Cache Manager) cache size — the number of 16-vertex OUTPUT batches the
// hardware buffers between the coordinate shader's VPM output and the PTB. Prior arcs wrote 1; Mesa's
// `v3d_vs_set_prog_data` (broadcom/compiler/vir.c) NEVER emits 1: it computes
//   vcm_cache_size = CLAMP(vpm_output_batches - 1, 2, 4)   (the field's HW-valid floor is 2, ceiling 4)
// and `v3d_compute_vpm_config` copies it verbatim into VCM_CACHE_SIZE's binning+rendering fields
// (vpm_cfg{,_bin}->Vc). For our fixed minimal draw on the Pi 4's 16 KiB VPM:
//   sector = V3D_CHANNELS(16)·4·8 = 512 B → 16384/512 = 32 sectors → half = 16;
//   vpm_output_size = 1 sector (6-word coord output rounds to 1); vpm_input_size folds to 0
//   (separate_segments = false, vir.c) → vpm_output_batches = 16/1 = 16 → CLAMP(15,2,4) = 4.
// The CLAMP ceiling is 4, so any 1-sector-output shader yields 4 regardless of exact VPM size. THE
// PI-V3D-23 empty-bin fix: Vc = 1 is below the GFXH-1744 floor (Mesa: "we can't go lower than 2 due to
// GFXH-1744, which makes an odd hardware bug that manifests as corrupt vertices"); a starved VCM cannot
// stage the coord shader's binned vertices for the PTB, so the PTB emits nothing — pool stays all-zero,
// exactly the observed wall (shader proven running, no fault, empty bin).
const VCM_CACHE_BATCHES: u64 = 4;

// ─── The minimal QPU packer (V3D 4.x / VideoCore VI). ───
// Field shifts VERBATIM from Mesa `qpu_pack.c`: OP_MUL[63:58] SIG[57:53] COND[52:46] MM(45) MA(44)
// WADDR_M[43:38] WADDR_A[37:32] OP_ADD[31:24] MUL_B[23:21] MUL_A[20:18] ADD_B[17:15] ADD_A[14:12]
// RADDR_A[11:6] RADDR_B[5:0]. Opcode values from `qpu_pack.c`: add-NOP op=187 (mux a=0,b=0); mul-NOP
// op=15 (mux b=4); WADDR_NOP=6, WADDR_TLB=7. MM=MA=1 mark the write registers "magic" (Mesa sets both
// even in its NOP — which is why the canonical NOP is 0x3c003186bb800000, not …0186…).
const QPU_A_NOP: u64 = 187;
const QPU_M_NOP_OPMUL: u64 = 15;
const QPU_M_NOP_MUXB: u64 = 4;
const QPU_WADDR_NOP: u64 = 6;

/// The canonical V3D 4.x NOP instruction, derived from fields and equal to Mesa's `0x3c003186bb800000`.
const fn qpu_nop() -> u64 {
    (QPU_M_NOP_OPMUL << 58) // OP_MUL = mul NOP
        | (1u64 << 45) // MM (magic mul write)
        | (1u64 << 44) // MA (magic add write)
        | (QPU_WADDR_NOP << 38) // WADDR_M = nop
        | (QPU_WADDR_NOP << 32) // WADDR_A = nop
        | (QPU_A_NOP << 24) // OP_ADD = add NOP
        | (QPU_M_NOP_MUXB << 21) // MUL_B mux
}
const _: () = assert!(qpu_nop() == 0x3c00_3186_bb80_0000);

// ════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-9 — VERIFIED QPU shader bodies (replacing the V3D-8 NOP skeletons).
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// PROVENANCE (absolute — this driver has been convicted THREE times for fabricated words; not again):
// every 64-bit word below was produced by Mesa's OWN packer, `v3d_qpu_instr_pack(devinfo.ver=42, …)`,
// from an explicit `struct v3d_qpu_instr` — i.e. these ARE Mesa's encoder output, not hand-authored
// bit patterns. The generator (`scratchpad/mesa/qpu_gen.c`, MIT, links Mesa's qpu_instr.c + qpu_pack.c
// from mesa 26.3.0-devel) additionally ROUND-TRIPS each word (pack → v3d_qpu_instr_unpack → repack,
// require identical) and, as a harness self-test, reproduces four canonical vectors from Mesa's
// `src/broadcom/qpu/tests/qpu_disasm.c` BIT-EXACTLY, proving the struct→word path matches Mesa's
// documented disasm semantics:
//     nop                     = 0x3c003186bb800000   (also the file's qpu_nop() self-check)
//     or rf0,r3,r3;mov vpm,r3 = 0x3c002380b6edb000
//     vfpack tlb, r0, r1      = 0x3c00318735808000
//     fadd r1,r1,r5 ; thrsw   = 0x3c20318105829000
// Mesa is MIT-licensed — verbatim-liftable WITH attribution (memory: unaos-license-gplv3).
//
// VERIFICATION LEVEL (honest, per the module's ATTENDED-METAL-UNVERIFIED banner): every WORD's ENCODING
// is Mesa-verified (round-trips against Mesa's own pack/unpack). The PROGRAMS follow Mesa's documented
// emit ORDER for the minimal cases (fragment: emit_frag_end + vir_emit_tlb_color_write; coord/vertex:
// ntq_emit_vpm_read + emit_store_output_vs + emit_vert_end). What remains the attended-metal-refinement
// surface — NOT fabrication, but silicon-tuned quantities QEMU cannot exercise — is: the coordinate
// viewport transform + exact VPM output layout/segment sizes, the VPM read-offset/setup values, and the
// FS colour-channel order + f16 rounding that make the stored word land exactly on TRI_RGBA. Each such
// quantity is called out at its uniform/word below.

/// FRAGMENT shader (solid colour → TLB). Mesa emit_frag_end path: 4× ldunifrf load colour rgba into
/// rf0..rf3, passthrough-Z `mov tlbu` (pops the Z TLB-config uniform), then two VFPACKs pack rgba to
/// f16 and write TLB (the rg write is the `u` variant, popping the colour TLB-config uniform). thrsw +
/// two nops close the (single) thread. Uniform FIFO order: r,g,b,a, Z-config, colour-config.
const FS_WORDS: [u64; 10] = [
    0x3d80_3186_bb80_0000, // nop ; ldunifrf.rf0   (rf0 <- colour.r)
    0x3d80_7186_bb80_0000, // nop ; ldunifrf.rf1   (rf1 <- colour.g)
    0x3d80_b186_bb80_0000, // nop ; ldunifrf.rf2   (rf2 <- colour.b)
    0x3d80_f186_bb80_0000, // nop ; ldunifrf.rf3   (rf3 <- colour.a)
    0x3c00_3206_bbe0_0000, // mov tlbu, r0         (passthrough-Z; pops Z TLB-config)
    0x3c00_3188_3583_e001, // vfpack tlbu, rf0, rf1 (colour r,g → f16; pops colour TLB-config)
    0x3c00_3187_3583_e083, // vfpack tlb, rf2, rf3  (colour b,a → f16)
    0x3c20_3186_bb80_0000, // nop ; thrsw          (last thread switch)
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// COORDINATE / VERTEX shader body — the SIX-word screen-space output (same program for the bin CS and
/// render VS variants), written with the V3D 4.2 STVPMV output mechanism.
///
/// PI-V3D-20 ROOT-CAUSE FIX: PI-V3D-9/17/18/19 wrote the VPM output with the *streamed* VC4 / V3D-3.3
/// mechanism — a `vpmsetup` to arm a VPM segment, then `mov vpm, rfN` (magic waddr VPM=14) auto-advancing
/// an implicit write pointer. That mechanism DOES NOT EXIST for per-vertex shader output on V3D 4.x
/// (ver==42, the Pi 4). Mesa proves it: `vir_VPM_WRITE` (src/broadcom/compiler/nir_to_vir.c) emits ONE
/// `vir_STVPMV(c, vir_uniform_ui(c, vpm_index), val)` per output component — a store-VPM with an EXPLICIT
/// integer VPM offset — and NO `mov vpm` / `vpmsetup` anywhere in the ver-42 VS/CS output path. So every
/// prior `mov vpm` (clip words AND the V3D-19 screen words) wrote an unconfigured magic register; the PTB
/// read zero screen coords and binned an empty-but-legal list (metal boot-P19: pool/tile-STATE all zero,
/// CL clean). No word-count change (4→6) ever moved it because the *addressing form* was wrong, not the
/// count. This body switches to STVPMV with explicit per-component offsets (0..5), sourced as uniforms
/// (Mesa-faithful) into rf9..rf14, and DROPS vpmsetup (unused on 4.x VS/CS output). VPMWT stays
/// (GFXH-1684, ver==42 emit_vert_end). `vpmsetup` DOES pack on ver 42 (opcode 187, first_ver 33) but on
/// 4.x it arms VPM *DMA* descriptors, not the shader output stream — an irrelevant channel.
///
/// W=1 SIMPLIFICATION (LOUD): TRI_VERTS all carry Wc = 1.0, so 1/Wc = 1.0 and NO reciprocal (SFU/recip)
/// is emitted — the transform collapses to Xs = f2i32(floor(Xc·8192)) (8192 = vp_scale 32·256). This
/// holds ONLY for W=1 geometry; a perspective draw (W≠1) would need a per-vertex reciprocal here.
///
/// Mesa order: 4× ldvpmv_in read the vec4 clip position into rf0..rf3 (each reloads the read-offset into
/// rf5); ldunifrf loads 8192.0f into rf6 then the six output offsets 0..5 into rf9..rf14; per screen axis
/// fmul→ffloor→ftoiz into rf7/rf8; then 6× `stvpmv rf<off>, rf<val>` store clip[0..3] + screen[4,5];
/// vpmwt (GFXH-1684); thrsw + two nops end. Registers: rf0..3 clip, rf5 in-offset, rf6=8192.0, rf7=Xs,
/// rf8=Ys, rf9..14 out-offsets 0..5.
///
/// PROVENANCE: every word Mesa-packed (v3d_qpu_instr_pack, ver 42) + round-tripped by
/// scripts/pi-v3d20-qpu-gen.c (see its .out.txt). Metal-refinement surface (unchanged stance): the
/// ldunifrf read-offsets and the RF write→read hazard scheduling — QEMU models no V3D, metal decides.
///
/// PI-V3D-26 (Mesa-COMPILED cross-check): this hand-structured body was validated against a real
/// `v3d_compile()` run (ver 4.2) of the passthrough VS — see scripts/pi-v3d26-mesa-compile.c and its
/// .out.txt. With the key configured as the driver configures the binning VS
/// (num_used_outputs = 0 for the last-geometry-stage coord shader), Mesa's coord shader stores clip
/// Xc,Yc,Zc,Wc to VPM offsets **0,1,2,3** and screen Xs,Ys to **4,5**, with viewport scale
/// 32·256 = **8192** — byte-for-byte this contract. Mesa additionally emits an unconditional
/// `recip(1/Wc)` and delivers the scale via QUNIFORM_VIEWPORT_{X,Y}_SCALE rather than a baked 8192.0
/// literal; both are numeric no-ops for the W = 1 test geometry, so this body is functionally
/// Mesa-equivalent. The coord shader is exonerated at the authoritative level — the empty-bin wall is
/// NOT in these words. (No word changed: fabricating a swap to Mesa's register allocation on a
/// QEMU-untestable path would risk regressing a metal-equivalent shader.) See v3d.md §11.
const CS_VS_WORDS: [u64; 27] = [
    0x3d81_6180_bc80_6140, // ldvpmv_in rf0, rf5 ; ldunifrf.rf5   (attr[0] -> Xc)
    0x3d81_6181_bc80_6140, // ldvpmv_in rf1, rf5 ; ldunifrf.rf5   (attr[1] -> Yc)
    0x3d81_6182_bc80_6140, // ldvpmv_in rf2, rf5 ; ldunifrf.rf5   (attr[2] -> Zc)
    0x3d81_6183_bc80_6140, // ldvpmv_in rf3, rf5 ; ldunifrf.rf5   (attr[3] -> Wc)
    0x3d81_b186_bb80_0000, // nop ; ldunifrf.rf6                  (rf6 <- 8192.0f vp_scale)
    0x3d82_7186_bb80_0000, // nop ; ldunifrf.rf9                  (out-offset 0)
    0x3d82_b186_bb80_0000, // nop ; ldunifrf.rf10                 (out-offset 1)
    0x3d82_f186_bb80_0000, // nop ; ldunifrf.rf11                 (out-offset 2)
    0x3d83_3186_bb80_0000, // nop ; ldunifrf.rf12                 (out-offset 3)
    0x3d83_7186_bb80_0000, // nop ; ldunifrf.rf13                 (out-offset 4)
    0x3d83_b186_bb80_0000, // nop ; ldunifrf.rf14                 (out-offset 5)
    0x5400_11c6_bbf8_0006, // fmul rf7, rf0, rf6                  (Xc · 8192.0 ; W=1 so no 1/Wc)
    0x3c00_2187_f680_61c0, // ffloor rf7, rf7                     (floor, ver==42 path)
    0x3c00_2187_f583_e1c0, // ftoiz rf7, rf7                      (f2i32)
    0x5400_1206_bbf8_0046, // fmul rf8, rf1, rf6                  (Yc · 8192.0)
    0x3c00_2188_f680_6200, // ffloor rf8, rf8                     (floor, ver==42 path)
    0x3c00_2188_f583_e200, // ftoiz rf8, rf8                      (f2i32)
    0x3c00_2180_f883_e240, // stvpmv rf9, rf0                     (out0 clip Xc @ offset 0)
    0x3c00_2180_f883_e281, // stvpmv rf10, rf1                    (out1 clip Yc @ offset 1)
    0x3c00_2180_f883_e2c2, // stvpmv rf11, rf2                    (out2 clip Zc @ offset 2)
    0x3c00_2180_f883_e303, // stvpmv rf12, rf3                    (out3 clip Wc @ offset 3)
    0x3c00_2180_f883_e347, // stvpmv rf13, rf7                    (out4 screen Xs @ offset 4)
    0x3c00_2180_f883_e388, // stvpmv rf14, rf8                    (out5 screen Ys @ offset 5)
    0x3c00_3186_bb81_6000, // vpmwt                               (VPM writes complete before end)
    0x3c20_3186_bb80_0000, // nop ; thrsw                         (end)
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// Write a table of QPU words (little-endian fetch order) into the arena at `off`. Returns byte length.
fn write_shader_words(off: usize, words: &[u64]) -> usize {
    for (i, w) in words.iter().enumerate() {
        arena_write_u64(off + i * 8, *w);
    }
    words.len() * 8
}

/// [v3d47] CS thread-end witness — dump the final six QPU words of the published coordinate shader as
/// they sit in the arena (the exact bytes the CLE hands the QPU), immediately before the bin GO, so the
/// P45 metal read confirms WHAT RAN rather than trusting the source constant. The `sig` field of each
/// word is decoded (bits[57:53]; SIG_THRSW==1) so the terminal thread-switch is visible on serial: a
/// clean coord-shader thread-end is `… vpmwt(sig=none) ; nop(sig=thrsw) ; nop ; nop` — byte-for-byte the
/// tail Mesa's own `v3d_compile` (ver 4.2) emits for a `threads=4` binning coord shader
/// (scripts/pi-v3d26-mesa-compile.out.txt words 18..21). See v3d.md §21: V3D-47 audited this tail against
/// that reference and found NO divergence — sig=thrsw, no sig.int, vpmwt correctly ahead of thrsw — so no
/// word was changed; this witness is the confirmation instrument for the P45 read.
fn cs_tail_witness(tag: &str, code_off: usize, word_count: usize) {
    // Re-read from DRAM through the arena so a corrupt publish (not just the constant) would show.
    cache::clean_invalidate_range(arena_phys() + code_off, word_count * 8);
    let start = word_count.saturating_sub(6);
    serial_println!(
        ":: V3D: [v3d47] {} CS tail @arena+{:#x} (words {}..{} of {}) — the exact published bytes ::",
        tag,
        code_off + start * 8,
        start,
        word_count,
        word_count
    );
    for i in start..word_count {
        let w = arena_u64(code_off + i * 8);
        let sig = ((w >> 53) & 0x1f) as u32; // V3D 4.x sig field: bits[57:53]; SIG_THRSW == 1
        let sig_name = match sig {
            0 => "none",
            1 => "thrsw",
            _ => "other",
        };
        serial_println!(
            ":: V3D:   [v3d47]   w[{:>2}] = {:#018x}  sig={:#04x}({}) ::",
            i,
            w,
            sig,
            sig_name
        );
    }
    serial_println!(
        ":: V3D: [v3d47] expected (Mesa v42 v3d_compile coord tail): vpmwt(sig=none) → nop(sig=thrsw) → nop → nop — NO sig.int; byte-for-byte per scripts/pi-v3d26-mesa-compile.out.txt [18..21] ::"
    );
}

/// The fragment-shader uniform stream (FIFO order matches FS_WORDS' pops). Colour channels are the
/// unorm8 decomposition of TRI_RGBA as f32; the exact channel order + f16 rounding that lands the
/// stored word on TRI_RGBA is the metal-refinement surface. The two TLB config words follow Mesa
/// vir_emit_tlb_color_write: Z = passthrough/per-pixel (0xffffff84), colour = F16 RT0 vec4 per-pixel
/// (0xffffff3f).
fn write_fs_uniforms(off: usize) -> usize {
    let r = ((TRI_RGBA & 0xFF) as f32 / 255.0).to_bits();
    let g = (((TRI_RGBA >> 8) & 0xFF) as f32 / 255.0).to_bits();
    let b = (((TRI_RGBA >> 16) & 0xFF) as f32 / 255.0).to_bits();
    let a = (((TRI_RGBA >> 24) & 0xFF) as f32 / 255.0).to_bits();
    let unif: [u32; 6] = [r, g, b, a, 0xFFFF_FF84, 0xFFFF_FF3F];
    for (i, w) in unif.iter().enumerate() {
        arena_write_u32(off + i * 4, *w);
    }
    unif.len() * 4
}

/// The coord/vertex uniform stream: the four VPM read-offsets (attribute component 0..3) the
/// ldvpmv_in instructions consume via ldunifrf.rf5, then the 8192.0f viewport scale (vp_scale =
/// viewport.scale 32 · clipper_xy_granularity 256) the screen-space `ldunifrf.rf6` consumes to compute
/// Xs/Ys = f2i32(floor(coord · 8192)), then the SIX output VPM offsets 0..5 (PI-V3D-20) that the
/// `ldunifrf.rf9..rf14` load for the STVPMV stores (Mesa sources these as `vir_uniform_ui(c, vpm_index)`).
/// Read-offsets are the metal-refinement surface; 8192.0 is the V3D-18-proven contract constant; the
/// output offsets are the fixed VPM out-slots 0..5 of the 6-word coordinate contract.
fn write_geo_uniforms(off: usize) -> usize {
    let unif: [u32; 11] = [0, 1, 2, 3, 0x4600_0000 /* 8192.0f32 */, 0, 1, 2, 3, 4, 5];
    for (i, w) in unif.iter().enumerate() {
        arena_write_u32(off + i * 4, *w);
    }
    unif.len() * 4
}

/// Store a little-endian u64 into the arena.
#[inline]
fn arena_write_u64(off: usize, v: u64) {
    let bytes = v.to_le_bytes();
    let arena = &raw mut V3D_ARENA;
    unsafe {
        for (i, b) in bytes.iter().enumerate() {
            (*arena).bytes[off + i] = *b;
        }
    }
}
/// Store a little-endian u32 into the arena.
#[inline]
fn arena_write_u32(off: usize, v: u32) {
    let bytes = v.to_le_bytes();
    let arena = &raw mut V3D_ARENA;
    unsafe {
        for (i, b) in bytes.iter().enumerate() {
            (*arena).bytes[off + i] = *b;
        }
    }
}
/// Read a single arena byte (for CPU-side witnesses that decode a struct field back out of the arena).
#[inline]
/// PI-V3D-57 (confirmed divergence #2 — ORDER): clear the PTB overflow allocation the way the kernel
/// does. `v3d_bin_job_run` (drivers/gpu/drm/v3d/v3d_sched.c) writes `V3D_PTB_BPOS = 0` as the FIRST
/// thing it does for a bin job — under the queue lock, BEFORE `v3d_invalidate_caches` and before the
/// CT0QMA/QMS/QTS/QBA/QEA latch — precisely so the PTB enters the frame with no overflow block carried
/// over from a previous job. Every kick in this file wrote BPOS=0 *after* CT0QBA instead (V3D-49 got the
/// value right and the position wrong): between the QMA/QTS latch and the BPOS clear the PTB has already
/// been handed its pool with a stale overflow descriptor still live. Value-identical, order-divergent —
/// so this restores the kernel's actual pre-job sequence at every CT0 kick.
fn bin_prejob_bpos_clear(what: &str) {
    mmio_write(V3D_CORE0_BASE, V3D_PTB_BPOS, 0);
    dsb();
    if V3D57_CL_AUDIT {
        serial_println!(
            ":: V3D: [v3d57] {} pre-job BPOS=0 (kernel-exact ORDER: first write of v3d_bin_job_run, before cache-invalidate and before the CT0QMA/QMS/QTS latch) — BPOA={:#010x} BPOS={:#010x} ::",
            what,
            mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOA),
            mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOS)
        );
    }
}

fn arena_byte(off: usize) -> u8 {
    let arena = &raw const V3D_ARENA;
    unsafe { (*arena).bytes[off] }
}
/// Read a little-endian u32 struct field back out of the arena (CPU-side witness decode).
#[inline]
fn arena_u32(off: usize) -> u32 {
    u32::from_le_bytes([
        arena_byte(off),
        arena_byte(off + 1),
        arena_byte(off + 2),
        arena_byte(off + 3),
    ])
}
/// Read a little-endian u64 QPU word back out of the arena (CPU-side witness decode — used by the
/// [v3d47] CS tail witness to confirm the exact bytes the CLE hands the QPU, not the source constant).
#[inline]
fn arena_u64(off: usize) -> u64 {
    (arena_u32(off) as u64) | ((arena_u32(off + 4) as u64) << 32)
}
/// Copy raw bytes into the arena at `off` (bounded — saturates at the arena end, never overruns).
fn arena_write_bytes(off: usize, src: &[u8]) {
    let arena = &raw mut V3D_ARENA;
    unsafe {
        for (i, b) in src.iter().enumerate() {
            if off + i >= ARENA_BYTES {
                break;
            }
            (*arena).bytes[off + i] = *b;
        }
    }
}
/// Fill a 32-bit pattern across `len` bytes at `off` (CPU-side sentinel pre-seed).
fn fill_region(off: usize, len: usize, pattern: u32) {
    let p = pattern.to_le_bytes();
    let arena = &raw mut V3D_ARENA;
    unsafe {
        let mut i = 0;
        while i < len {
            (*arena).bytes[off + i] = p[i & 3];
            i += 1;
        }
    }
}

/// The triangle's three clip-space vertices, each a vec4 (x, y, z, w) IEEE-754 f32. NDC in [-1,1];
/// a centred triangle so its interior samples land near (32,32) of the 64×64 target and its exterior
/// samples land in the corners. The COORDINATE shader is responsible for the viewport transform to the
/// 64×64 screen (its exact math is part of the metal-refined shader body). Attribute 0, stride 16 B.
const TRI_VERTS: [[f32; 4]; 3] = [
    [-0.6, -0.6, 0.5, 1.0], // lower-left
    [0.6, -0.6, 0.5, 1.0],  // lower-right
    [0.0, 0.6, 0.5, 1.0],   // top-centre
];

/// Emit one field (LSB-first) into a raw struct buffer at ABSOLUTE bit `start` — like `set_bits`, but
/// for a memory STRUCT (GL Shader State Record / attribute record) which has NO leading opcode byte, so
/// XML `start` bits are used directly (no +8 shift). Address fields whose XML size is < 32 carry the
/// aligned address already shifted by the caller.
#[inline]
fn sf(buf: &mut [u8], start: usize, width: usize, val: u64) {
    set_bits(buf, start, width, val);
}

/// Read one LSB-first field of `width` bits at ABSOLUTE bit `start` out of a raw struct buffer — the
/// inverse of `sf`/`set_bits`. Used by the [v3d38] record-diff witness to decode a GL Shader State
/// Record field-by-field straight out of the arena bytes.
#[inline]
fn gf(buf: &[u8], mut bit: usize, mut width: usize) -> u64 {
    let mut out: u64 = 0;
    let mut shift = 0;
    while width > 0 {
        let byte = bit / 8;
        let off = bit % 8;
        let take = core::cmp::min(8 - off, width);
        let mask = ((1u64 << take) - 1) as u8;
        let chunk = ((buf[byte] >> off) & mask) as u64;
        out |= chunk << shift;
        shift += take;
        bit += take;
        width -= take;
    }
    out
}

/// [v3d38] witness: field-by-field dump + DIFF of the two 36-byte GL Shader State Records — the probe's
/// (at OFF_PROBE_SHADREC) and the confirmed-dispatching M4's (at OFF_SHADREC) — decoded per the v42
/// genxml "GL Shader State Record" layout. Both records must already be published to the arena. Every
/// field is printed for both records; a field that differs is annotated `DIFF`. This is the witness for
/// V3D-38's thesis that, with the two binning CLs proven byte-identical (v3d36) and threading refuted
/// both ways (v3d37), the RECORD CONTENTS the GL_SHADER_STATE pointer selects are the last variable.
fn witness_shadrec_diff() {
    // Copy both 36-byte records out of the arena.
    let mut p = [0u8; 36];
    let mut m = [0u8; 36];
    for i in 0..36 {
        p[i] = arena_byte(OFF_PROBE_SHADREC + i);
        m[i] = arena_byte(OFF_SHADREC + i);
    }
    // (name, bit start, width) per the v42 GL Shader State Record. Address fields are stored pre-shifted
    // (>>3 for the 29-bit code fields); the raw extracted value is printed as-is.
    let fields: [(&str, usize, usize); 22] = [
        ("point_size_in_shaded_vertex_data", 0, 1),
        ("enable_clipping", 1, 1),
        ("vertex_id_read_by_coord", 2, 1),
        ("instance_id_read_by_coord", 3, 1),
        ("fs_number_of_varyings", 24, 8),
        ("cs_output_vpm_segment_size", 32, 4),
        ("cs_input_vpm_segment_size", 40, 4),
        ("vs_output_vpm_segment_size", 48, 4),
        ("vs_input_vpm_segment_size", 56, 4),
        ("default_attr_values_addr", 64, 32),
        ("fs_4way_threadable", 96, 1),
        ("fs_single_seg", 97, 1),
        ("fs_propagate_nans", 98, 1),
        ("fs_code_addr>>3", 99, 29),
        ("fs_uniforms_addr", 128, 32),
        ("vs_4way_threadable", 160, 1),
        ("vs_2way_threadable", 161, 1),
        ("vs_code_addr>>3", 163, 29),
        ("vs_uniforms_addr", 192, 32),
        ("cs_4way_threadable", 224, 1),
        ("cs_code_addr>>3", 227, 29),
        ("cs_uniforms_addr", 256, 32),
    ];
    serial_println!(
        ":: V3D: [v3d38] GL Shader State Record field diff — PROBE @{:#010x} vs M4 @{:#010x} (v42 layout; both records published to RAM) ::",
        (arena_phys() + OFF_PROBE_SHADREC) as u32,
        (arena_phys() + OFF_SHADREC) as u32
    );
    let mut ndiff = 0u32;
    for (name, start, width) in fields.iter() {
        let pv = gf(&p, *start, *width);
        let mv = gf(&m, *start, *width);
        let diff = pv != mv;
        if diff {
            ndiff += 1;
        }
        serial_println!(
            ":: V3D: [v3d38]   {:<32} probe={:#010x} M4={:#010x} {} ::",
            name, pv, mv,
            if diff { "<-- DIFF" } else { "==" }
        );
    }
    serial_println!(
        ":: V3D: [v3d38] record diff summary — {} field(s) differ. Post-borrow the FS/VS slots should read `==` (probe now shares M4's known-good FS+VS); only the CS code/uniform pointers should carry the probe's TMU-store program. ::",
        ndiff
    );
}

/// Build the GL Shader State Record (v42, 36 bytes) at OFF_SHADREC and one GL Shader State Attribute
/// Record (16 bytes) immediately after it. Layout VERBATIM from Mesa `v3d_packet.xml` struct
/// "GL Shader State Record" (max_ver=42) + "GL Shader State Attribute Record"; field values follow
/// `v3dx_draw.c` `v3dX(draw_vbo)`'s shader-record emit for a trivial 1-attribute solid draw. Returns
/// the number of attribute arrays (for the GL_SHADER_STATE packet). Code addresses are 29-bit fields at
/// the top of a 32-bit aligned word (low 3 bits are the threadability/nan flags) → the address is
/// written pre-shifted `>> 3`.
fn build_shader_record() -> u32 {
    let cs = (arena_phys() + OFF_CS_CODE) as u64;
    let vs = (arena_phys() + OFF_VS_CODE) as u64;
    let fs = (arena_phys() + OFF_FS_CODE) as u64;
    let defaults = (arena_phys() + OFF_DEFAULT_ATTRS) as u64;
    let vtx = (arena_phys() + OFF_VTXDATA) as u64;

    let mut rec = [0u8; 36];
    sf(&mut rec, 1, 1, 1); // Enable clipping
    // FS: 0 varyings (solid colour). VPM segment sizes: 1 segment each (minimal); the coordinate/vertex
    // shaders each get a single input+output VPM block. These are conservative minimal values, refined
    // with the real shader's prog_data at the sitting.
    sf(&mut rec, 24, 8, 0); // Number of varyings in Fragment Shader
    // PI-V3D-25: VPM segment sizes VERBATIM from Mesa `v3d_vs_set_prog_data` (broadcom/compiler/vir.c,
    // this arc's checkout). Mesa runs it for BOTH the coord (bin) and vertex variants and, to share one
    // VPM block between input and output (`separate_segments = false`, "necessary for our VCM setup to
    // avoid varying corruption"), FOLDS the input into the output and ZEROES the input size:
    //     vpm_output_size = MAX(vpm_output_size, vpm_input_size);  vpm_input_size = 0;   (vir.c:918-920)
    // So the shader record's INPUT segment-size fields must be 0, not 1. `v3dvx_pipeline.c` writes
    // `coordinate_shader_input_vpm_segment_size = prog_data_vs_bin->vpm_input_size` (= 0). Prior arcs
    // (and v3d.md §7) wrongly declared input = 1: a bogus separate 1-sector input block, so the VCD's
    // attribute DMA and the shader's `ldvpmv_in` reads addressed different VPM rows — the shader read
    // zeros, every vertex collapsed to (0,0), and the PTB legitimately binned the degenerate point to
    // nothing. This is the V3D-24 attribute-fetch hypothesis, root-caused CPU-side. output = 1 (6 coord
    // words → align(6,8)/8 = 1 sector) is correct and unchanged. No shader word changes (§5 untouched).
    sf(&mut rec, 32, 4, 1); // Coord Shader output VPM segment size (Mesa vpm_output_size = 1)
    sf(&mut rec, 40, 4, 0); // Coord Shader input VPM segment size  (Mesa folds input → 0)
    sf(&mut rec, 48, 4, 1); // Vertex Shader output VPM segment size (Mesa vpm_output_size = 1)
    sf(&mut rec, 56, 4, 0); // Vertex Shader input VPM segment size  (Mesa folds input → 0)
    sf(&mut rec, 64, 32, defaults); // Address of default attribute values
    // Fragment shader: flags at 96/97/98 (4-way threadable, final section, propagate NaNs), addr@99(29).
    let fs_unif = (arena_phys() + OFF_FS_UNIF) as u64;
    let vs_unif = (arena_phys() + OFF_VS_UNIF) as u64;
    let cs_unif = (arena_phys() + OFF_CS_UNIF) as u64;
    sf(&mut rec, 96, 1, 1); // FS 4-way threadable
    sf(&mut rec, 98, 1, 1); // FS propagate NaNs (v42)
    sf(&mut rec, 99, 29, fs >> 3); // FS code address
    sf(&mut rec, 128, 32, fs_unif); // FS uniforms address (PI-V3D-9: colour + TLB config stream)
    sf(&mut rec, 160, 1, 1); // VS 4-way threadable
    sf(&mut rec, 162, 1, 1); // VS propagate NaNs (v42)
    sf(&mut rec, 163, 29, vs >> 3); // VS code address
    sf(&mut rec, 192, 32, vs_unif); // VS uniforms address (PI-V3D-9: VPM read-offset stream)
    sf(&mut rec, 224, 1, 1); // CS 4-way threadable
    sf(&mut rec, 226, 1, 1); // CS propagate NaNs (v42)
    sf(&mut rec, 227, 29, cs >> 3); // CS code address
    sf(&mut rec, 256, 32, cs_unif); // CS uniforms address (PI-V3D-9: VPM read-offset stream)
    arena_write_bytes(OFF_SHADREC, &rec);

    // One attribute record (vec4 position, f32), immediately after the 36-byte record.
    let mut attr = [0u8; 16];
    sf(&mut attr, 0, 32, vtx); // Address
    sf(&mut attr, 32, 2, 3); // Vec size (encodes 4 components: 4-1)
    sf(&mut attr, 34, 3, 2); // Type = Attribute float
    sf(&mut attr, 40, 4, 4); // Number of values read by Coordinate shader
    sf(&mut attr, 44, 4, 4); // Number of values read by Vertex shader
    sf(&mut attr, 64, 32, 16); // Stride (bytes per vertex)
    sf(&mut attr, 96, 32, 0xFFFF); // Maximum Index
    arena_write_bytes(OFF_SHADREC + 36, &attr);

    1 // one attribute array
}

/// Build the BINNING control list (CT0) at OFF_BIN_CL. Prologue per `v3dX(start_binning)`, draw emit
/// per `v3dX(draw_vbo)`. Returns its byte length.
fn build_bin_cl(num_attrs: u32) -> usize {
    build_bin_cl_generic(OFF_BIN_CL, OFF_SHADREC, num_attrs)
}

/// PI-V3D-48 — the empty-frame bisection. `build_bin_cl_content` emits the SAME bin CL as the real draw
/// but truncated to a chosen "rung" so ONE metal boot can localise which packet class introduces the
/// wedge (empty frame retires but the full draw does not). Each rung is a strict superset of the one
/// below it, so the FLDONE verdict per rung walks the offending packet down to a single class.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BinContent {
    /// Full draw: fixed-function state + GL_SHADER_STATE + VERTEX_ARRAY_PRIMS (the real M4 / probe bin).
    Full,
    /// Empty frame — the discriminating experiment. NUMBER_OF_LAYERS + TILE_BINNING_MODE_CFG +
    /// FLUSH_VCD_CACHE + START_TILE_BINNING + FLUSH. Zero primitives, zero shader/viewport state. Per the
    /// kernel `v3d_bin_job_run` an empty bin frame must still retire (FLDONE + BFC++). If THIS retires the
    /// frame-level handshake is sound and the wedge enters with the state/prims below.
    Empty,
    /// Config + START + the full fixed-function state + GL_SHADER_STATE, but NO VERTEX_ARRAY_PRIMS: the
    /// binner has a shader selected but is handed no primitives to walk.
    StateNoPrims,
    /// Full state + VERTEX_ARRAY_PRIMS, but GL_SHADER_STATE selects a NULL coord shader (the exonerated
    /// 4-word Mesa thread-end tail, no VPM output). Isolates the primitive-walk/shader-dispatch handshake
    /// from the specific 6-word transform the real coord shader emits.
    PrimsNullShader,
}

/// Same bin CL as `build_bin_cl`, but with the control-list and shader-record offsets parameterised so
/// a second (probe) draw can reuse the identical prologue/state/draw emit against its own record.
/// PI-V3D-27 uses it to bin the Mesa-compiled TMU-store probe over the SAME vertex buffer + attribute
/// record as the real draw.
fn build_bin_cl_generic(cl_off: usize, shadrec_off: usize, num_attrs: u32) -> usize {
    build_bin_cl_content(cl_off, shadrec_off, num_attrs, BinContent::Full)
}

/// PI-V3D-48 bisection body. `content` selects the rung (see `BinContent`). `Full` reproduces the exact
/// legacy `build_bin_cl_generic` byte stream (the real draw); the reduced rungs drop packet classes from
/// the top down. The prologue (NUMBER_OF_LAYERS, TILE_BINNING_MODE_CFG, FLUSH_VCD_CACHE, START_TILE_BINNING)
/// and the terminating FLUSH are common to every rung — those are the frame-level handshake under test.
fn build_bin_cl_content(cl_off: usize, shadrec_off: usize, num_attrs: u32, content: BinContent) -> usize {
    build_bin_cl_content_geom(cl_off, shadrec_off, num_attrs, content, TARGET_W, TARGET_H)
}

/// PI-V3D-68 — `build_bin_cl_content` with the FRAME GEOMETRY parameterised.
///
/// `fb_w`/`fb_h` land in `TILE_BINNING_MODE_CFG`'s width/height fields (both `minus_one`) and NOWHERE
/// ELSE: the clip window, viewport offset and clipper scaling stay at their `TARGET_W`/`TARGET_H` values
/// on every rung, so the geometry ladder varies EXACTLY ONE thing — the frame the PTB is told to tile —
/// and a `Full` rung at a larger frame draws the same triangle into the same corner tile. Passing
/// `(TARGET_W, TARGET_H)` reproduces the pre-V3D-68 byte stream exactly; every existing caller does.
fn build_bin_cl_content_geom(
    cl_off: usize,
    shadrec_off: usize,
    num_attrs: u32,
    content: BinContent,
    fb_w: usize,
    fb_h: usize,
) -> usize {
    let shadrec = (arena_phys() + shadrec_off) as u32;
    let mut w = RclWriter::new(cl_off);

    // NUMBER_OF_LAYERS (single layer → minus_one 0), required before the bin-mode config.
    w.pkt(Pkt::new(P_NUMBER_OF_LAYERS, 2).f(0, 8, 0).done());
    // TILE_BINNING_MODE_CFG (v42): 64×64 frame, 1 RT, no MSAA, no double-buffer, 32-bit max BPP.
    // PI-V3D-14: 128-byte INITIAL block + 64-byte overflow block — Mesa's only silicon-exercised
    // config (v3d_limits.h INITIAL=128/OVERFLOW=64; boot-P9 showed the binner never wrote the pool
    // under 64B/64B). Field bits: width@32(16,minus_one), height@48(16,minus_one),
    // num RT@8(4,minus_one), max bpp@12(2), block size@4(2), initial block size@2(2).
    w.pkt(
        Pkt::new(P_TILE_BINNING_MODE_CFG, 9)
            .f(2, 2, TILE_ALLOC_BLOCK_SIZE_128B) // tile allocation initial block size 128b
            .f(4, 2, TILE_ALLOC_BLOCK_SIZE_64B) // tile allocation (overflow) block size 64b
            .f(8, 4, 0) // Number of Render Targets (minus_one: 1 → 0)
            .f(12, 2, INTERNAL_BPP_32) // Maximum BPP of all render targets
            .f(32, 16, (fb_w - 1) as u64) // Width in pixels (minus_one) — PI-V3D-68: the ladder's variable
            .f(48, 16, (fb_h - 1) as u64) // Height in pixels (minus_one)
            .done(),
    );
    // Flush any stale VCD, disable any leftover occlusion-query state, then START_TILE_BINNING (must
    // precede geometry). PI-V3D-23: Mesa's `v3d_start_binning` (gallium v3dx_draw.c) emits
    // OCCLUSION_QUERY_COUNTER with a null address between FLUSH_VCD_CACHE and START_TILE_BINNING —
    // "Disable any leftover OQ state from another job." A stale-enabled OQ counter can gate the PTB's
    // primitive accounting, so we match Mesa's prologue verbatim (addr 0 = OQ disabled).
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());
    // PI-V3D-48: the `Empty` rung is the minimal frame — config + START + FLUSH only (no OQ-disable, no
    // state, no prims). Every richer rung keeps Mesa's OQ-disable in the prologue.
    if content != BinContent::Empty {
        w.pkt(Pkt::new(P_OCCLUSION_QUERY_COUNTER, 5).f(0, 32, 0).done());
    }
    w.pkt(Pkt::new(P_START_TILE_BINNING, 1).done());
    // PI-V3D-48: `Empty` stops here — START_TILE_BINNING then straight to the terminating FLUSH, no
    // fixed-function state and no draw. An empty bin frame must still retire (kernel `v3d_bin_job_run`).
    if content == BinContent::Empty {
        w.pkt(Pkt::new(P_FLUSH, 1).done());
        return w.len();
    }

    // ── PI-V3D-17: clip/viewport/config state (V3D-16 verdict). Without these the hardware clipper
    // runs at power-on-reset zeros — zero viewport scale collapses every primitive to a point and the
    // binner writes an empty-but-legal bin (tile-alloc pool never touched). All opcodes/lengths/field
    // bits are VERBATIM from Mesa v3d_packet.xml gen 4.2; the transform values follow Mesa's own
    // v3dX(emit) (v3dx_emit.c / v3dvx_cmd_buffer.c) fixed-function viewport emit.
    //
    // CS-COORDS CONSISTENCY — PI-V3D-18 RESOLUTION (supersedes the earlier "shader OR fixed-function"
    // framing, which was a false dichotomy). Mesa's coordinate (bin) shader emits BOTH: the fixed-
    // function viewport state below AND two screen-space words the shader itself writes. Authoritative
    // layout — `v3d_nir_setup_vpm_layout_vs` / `v3d_nir_emit_ff_vpm_outputs`
    // (src/broadcom/compiler/v3d_nir_lower_io.c): for is_coord the VPM OUTPUT is SIX words —
    //     offset 0..3 : Xc, Yc, Zc, Wc     (raw clip-space position; state->pos[0..3])
    //     offset 4    : Xs = f2i32(floor( Xc · vp_scale_x · 1/Wc ))   ← screen X, .8 fixed-point, INT
    //     offset 5    : Ys = f2i32(floor( Yc · vp_scale_y · 1/Wc ))   ← screen Y
    // where vp_scale = viewport.scale · clipper_xy_granularity = 32 · 256 = 8192 (v3d_uniforms.c
    // QUNIFORM_VIEWPORT_X_SCALE; granularity 256.0f for ver 42, v3d_device_info.c). The Xs/Ys are
    // CENTRE-RELATIVE (no +centre in the shader); the +32,+32 centre is supplied by VIEWPORT_OFFSET
    // below — so the two mechanisms COMPOSE, they do not double-apply. The PTB bins from the SCREEN
    // words (out-offsets 4,5). PI-V3D-19 RESOLVES the residual V3D-17/18 empty-bin cause: CS_VS_WORDS now
    // emits all SIX words — the 4 clip words THEN Xs,Ys (fmul·8192 → ffloor → ftoiz → mov vpm, per axis),
    // Mesa-packed by scripts/pi-v3d19-qpu-gen.c. W=1 SIMPLIFICATION: TRI_VERTS all carry Wc=1.0, so
    // 1/Wc=1.0 and the transform is floor(coord·8192) with NO reciprocal (this holds ONLY for W=1
    // geometry). The fixed-function state below is CORRECT and stays; the record's VPM segment sizes are
    // also CORRECT (Mesa packs them in SECTORS = align(words,8)/8: 4 in → 1, 6 out → 1; vir.c
    // v3d_vs_set_prog_data — re-verified: 6 words still round to 1 sector, record unchanged). QEMU models
    // no V3D so the real verdict is the next metal boot's tile-alloc pool / tile-STATE going non-zero;
    // cs_vpm_output_witness() prints the expected Xs/Ys per vertex to check against.

    // CFG_BITS (code 96, v42): enable BOTH forward- and reverse-facing primitives (no cull); every
    // other bit 0 (no depth/stencil/blend). Fields: fwd-facing@0(1), rev-facing@1(1). Length 4
    // (opcode + 3 payload; max field bit 21 → 3 bytes).
    w.pkt(
        Pkt::new(P_CFG_BITS, 4)
            .f(0, 1, 1) // Enable Forward Facing Primitive
            .f(1, 1, 1) // Enable Reverse Facing Primitive
            .done(),
    );
    // CLIP_WINDOW (code 107): left=0, bottom=0, width=TARGET_W, height=TARGET_H. Fields:
    // left@0(16), bottom@16(16), width@32(16), height@48(16). Length 9 (opcode + 8 payload).
    w.pkt(
        Pkt::new(P_CLIP_WINDOW, 9)
            .f(0, 16, 0) // Clip Window Left Pixel Coordinate
            .f(16, 16, 0) // Clip Window Bottom Pixel Coordinate
            .f(32, 16, TARGET_W as u64) // Clip Window Width in pixels
            .f(48, 16, TARGET_H as u64) // Clip Window Height in pixels
            .done(),
    );
    // VIEWPORT_OFFSET (code 108): screen-space centre (32,32). Per v3dx_emit.c the fine coords hold
    // viewport.translate (the centre, in pixels) and coarse=0 for non-negative centres. Fine X/Y are
    // type u14.8 (value × 256): 32.0 px → 8192. Fields: fine_x@0(22,u14.8), coarse_x@22(10,int),
    // fine_y@32(22,u14.8), coarse_y@54(10,int). Length 9 (opcode + 8 payload; max field bit 63).
    const VP_FINE_CENTRE: u64 = (TARGET_W as u64 / 2) * 256; // 32 px × 256 = 8192 (u14.8)
    w.pkt(
        Pkt::new(P_VIEWPORT_OFFSET, 9)
            .f(0, 22, VP_FINE_CENTRE) // Fine X (u14.8): centre 32.0 px
            .f(22, 10, 0) // Coarse X (int): 0
            .f(32, 22, VP_FINE_CENTRE) // Fine Y (u14.8): centre 32.0 px
            .f(54, 10, 0) // Coarse Y (int): 0
            .done(),
    );
    // CLIPPER_XY_SCALING (code 110, v42): viewport half-extent in 1/256th px, as f32. Per
    // v3dx_emit.c the field is viewport.scale × 256.0f; half-width of a 64 px viewport = 32 px →
    // 32 × 256 = 8192.0f32. Fields: half-width@0(32,float), half-height@32(32,float). Length 9.
    let half_scale = (((TARGET_W as f32) / 2.0) * 256.0).to_bits() as u64; // 8192.0f32
    w.pkt(
        Pkt::new(P_CLIPPER_XY_SCALING, 9)
            .f(0, 32, half_scale) // Viewport Half-Width in 1/256th of pixel
            .f(32, 32, half_scale) // Viewport Half-Height in 1/256th of pixel
            .done(),
    );
    // CLIPPER_Z_SCALE_AND_OFFSET (code 111): map NDC z [-1,1] → depth [0,1]. Per v3dx_emit.c the
    // fields are viewport.scale[2] (=0.5) and viewport.translate[2] (=0.5). Fields:
    // z_scale@0(32,float), z_offset@32(32,float). Length 9.
    w.pkt(
        Pkt::new(P_CLIPPER_Z_SCALE_AND_OFFSET, 9)
            .f(0, 32, (0.5f32).to_bits() as u64) // Viewport Z Scale (Zc to Zs)
            .f(32, 32, (0.5f32).to_bits() as u64) // Viewport Z Offset (Zc to Zs)
            .done(),
    );

    // Draw state: VCM cache size, the shader-state pointer, then the prim. PI-V3D-23: Vc = 4 (was the
    // GFXH-1744-illegal 1) for BOTH the binning and rendering fields — the Mesa-computed value for this
    // draw (see VCM_CACHE_BATCHES). Field layout per v3d_packet.xml code 71: binning@0(4), rendering@4(4),
    // neither minus_one — so the raw field IS the batch count.
    w.pkt(
        Pkt::new(P_VCM_CACHE_SIZE, 2)
            .f(0, 4, VCM_CACHE_BATCHES) // 16-vertex batches for binning
            .f(4, 4, VCM_CACHE_BATCHES) // 16-vertex batches for rendering
            .done(),
    );
    // GL_SHADER_STATE: address is a 27-bit field @ start5 → the record's 32-byte-aligned address's top
    // 27 bits; number of attribute arrays in the low 5 bits.
    //
    // PI-V3D-10 boot-P6 root cause #1 (the out-of-arena bin fault): this packet was emitted with
    // length 4 — opcode + only THREE payload bytes — but the address field spans XML bits [5, 31], so
    // the payload is 4 bytes and the packet is 5 bytes total (v3d_packet.xml code 64). The CLE
    // therefore consumed the FOLLOWING packet's opcode byte — VERTEX_ARRAY_PRIMS, 36 = 0x24 — as the
    // shader-record address's top byte and fetched the record at 0x24000000 | shadrec. Boot-P6 proof:
    // VIO_ID 0x81 >> 5 = client 4 = CLE (v3d_irq.c v3d_41_axi_ids), and VIO_ADDR 0x04841800 scaled by
    // (va_width − 32) = 3 (DEBUG_INFO 0x550 → VA_WIDTH field 5 → va_width 35, per v3d_drv.c) gives
    // VA 0x2420C000 = 0x24 << 24 | 0x20C000 — exactly the shader record (arena+0x1C000) with the 0x24
    // opcode byte on top. The "POR-shaped garbage" was our own next opcode. Length corrected to 5.
    w.pkt(
        Pkt::new(P_GL_SHADER_STATE, 5)
            .f(0, 5, num_attrs as u64) // number of attribute arrays
            .f(5, 27, (shadrec >> 5) as u64) // record address (32-byte aligned)
            .done(),
    );
    // VERTEX_ARRAY_PRIMS: draw 3 vertices as a triangle list. mode@0(8)=TRIANGLES(4), length@8(32)=3,
    // index of first vertex@40(32)=0. PI-V3D-48: the `StateNoPrims` rung OMITS this — state + shader
    // selected but no primitives to walk, so the binner runs no vertex shading and emits no list bytes.
    if content != BinContent::StateNoPrims {
        w.pkt(
            Pkt::new(P_VERTEX_ARRAY_PRIMS, 10)
                .f(0, 8, V3D_PRIM_TRIANGLES)
                .f(8, 32, 3) // Length (vertex count)
                .f(40, 32, 0) // Index of First Vertex
                .done(),
        );
    }
    // FLUSH terminates the binning list (the binner-done marker CT0 walks to).
    w.pkt(Pkt::new(P_FLUSH, 1).done());
    w.len()
}

/// Build the M4 RENDER control list (CT1) at OFF_M4_RCL + its generic per-tile sub-list at
/// OFF_M4_SUBLIST. Mirrors the M3 RCL but (a) targets OFF_M4_TARGET, and (b) the sub-list runs
/// BRANCH_TO_IMPLICIT_TILE_LIST so the render EXECUTES the binner's per-tile geometry list (the M3
/// clear-only list omitted this branch — here it is the whole point). Returns `(main_len, sublist_len)`.
fn build_m4_rcl() -> (usize, usize) {
    let target = (arena_phys() + OFF_M4_TARGET) as u32;
    let sublist_start = (arena_phys() + OFF_M4_SUBLIST) as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let stride = (TARGET_W * TARGET_BPP) as u64;

    // ── Generic per-tile sub-list: run the implicit (binned) tile list, then store the tile buffer. ──
    let mut s = RclWriter::new(OFF_M4_SUBLIST);
    s.pkt(Pkt::new(P_TILE_COORDINATES_IMPLICIT, 1).done());
    s.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
    s.pkt(Pkt::new(P_PRIM_LIST_FORMAT, 2).f(0, 6, PRIM_TYPE_LIST_TRIANGLES).done());
    // THE new branch: execute the binner's per-tile primitive list for this tile (set number 0). This is
    // what draws the triangle the binner produced — the M3 clear-job had no geometry so omitted it.
    s.pkt(Pkt::new(P_BRANCH_TO_IMPLICIT_TILE_LIST, 2).f(0, 8, 0).done());
    // Store RT0 → OFF_M4_TARGET, raster, rgba8, row stride (the write the CPU sample-verifies).
    s.pkt(
        Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13)
            .f(0, 4, 0) // Buffer to Store = Render target 0
            .f(4, 3, MEMORY_FORMAT_RASTER)
            .f(12, 6, OUTPUT_IMAGE_FORMAT_RGBA8)
            .f(28, 20, stride)
            .f(64, 32, target as u64)
            .done(),
    );
    s.pkt(Pkt::new(P_CLEAR_TILE_BUFFERS, 2).f(0, 1, 1).f(1, 1, 1).done());
    s.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    s.pkt(Pkt::new(P_RETURN_FROM_SUB_LIST, 1).done());
    let sublist_len = s.len();
    let sublist_end = sublist_start + sublist_len as u32;
    cache::clean_range(arena_phys() + OFF_M4_SUBLIST, sublist_len);

    // ── Main render list: frame config (clear colour = CLEAR_RGBA so OUTSIDE the triangle reads clear),
    // then the single-supertile render that branches into the sub-list. Same structure as M3. ──
    let mut w = RclWriter::new(OFF_M4_RCL);
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COMMON)
            .f(4, 4, 0)
            .f(8, 16, TARGET_W as u64)
            .f(24, 16, TARGET_H as u64)
            .f(40, 2, INTERNAL_BPP_32)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_CLEAR_COLORS_PART1)
            .f(4, 4, 0)
            .f(8, 32, CLEAR_RGBA as u64)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COLOR)
            .f(4, 2, INTERNAL_BPP_32)
            .f(6, 4, INTERNAL_TYPE_8)
            .f(10, 2, 0)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_ZS_CLEAR_VALUES)
            .f(8, 8, 0)
            .f(16, 32, 0)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TILE_LIST_INITIAL_BLOCK_SIZE, 2)
            .f(0, 2, TILE_ALLOC_BLOCK_SIZE_128B) // PI-V3D-14: match bin config's initial block
            .f(2, 1, 1)
            .done(),
    );
    w.pkt(
        Pkt::new(P_MULTICORE_TILE_LIST_BASE, 5)
            .f(0, 4, 0)
            .f(6, 26, (tile_alloc >> 6) as u64)
            .done(),
    );
    w.pkt(
        Pkt::new(P_MULTICORE_SUPERTILE_CFG, 9)
            .f(0, 8, 0)
            .f(8, 8, 0)
            .f(16, 8, 1)
            .f(24, 8, 1)
            .f(32, 12, 1)
            .f(44, 12, 1)
            .f(61, 3, 0)
            .done(),
    );
    // Initial tile-buffer clear (GFXH-1742 double-dummy-store workaround), same as M3.
    w.pkt(Pkt::new(P_TILE_COORDINATES, 4).f(0, 12, 0).f(12, 12, 0).done());
    for i in 0..2 {
        if i > 0 {
            w.pkt(Pkt::new(P_TILE_COORDINATES, 4).f(0, 12, 0).f(12, 12, 0).done());
        }
        w.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
        w.pkt(Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13).f(0, 4, 8).done());
        if i == 0 {
            w.pkt(Pkt::new(P_CLEAR_TILE_BUFFERS, 2).f(0, 1, 1).f(1, 1, 1).done());
        }
        w.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    }
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());
    w.pkt(
        Pkt::new(P_GENERIC_TILE_LIST, 9)
            .f(0, 32, sublist_start as u64)
            .f(32, 32, sublist_end as u64)
            .done(),
    );
    w.pkt(Pkt::new(P_SUPERTILE_COORDINATES, 3).f(0, 8, 0).f(8, 8, 0).done());
    w.pkt(Pkt::new(P_END_OF_RENDERING, 1).done());
    (w.len(), sublist_len)
}

/// The CT0 (binning) run/never-ran discriminator — the PI-V3D-7 idiom extended to CT0. Given the
/// pre/kicked/done CS+CA snapshots and the [BA,EA) queue range, classify whether the BIN CLE actually
/// started. Same truth table as the CT1 render discriminator: RAN iff CTRUN was ever observed OR CT0CA
/// advanced INTO (BA, EA]; a never-started CLE has CTRUN never seen AND CT0CA at 0/BA.
fn ct0_ran(cs_pre: u32, cs_kicked: u32, cs_done: u32, ca_done: u32, ba: u32, ea: u32) -> bool {
    let ctrun_ever = (cs_pre | cs_kicked | cs_done) & V3D_CLE_CT1CS_CTRUN != 0; // CTRUN bit is shared
    let ca_advanced = ca_done != 0 && ca_done != ba && ca_done >= ba && ca_done <= ea;
    ctrun_ever || ca_advanced
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-27: the Mesa-COMPILED attribute-DMA probe (v3d.md §12).
//
// The empty-bin wall's one surviving candidate after §8/§10/§11: the VCD never DMAs the vertex
// attributes into the VPM, so the coord shader's `ldvpmv_in` reads collapse every vertex to Xc=Yc=0 →
// a degenerate zero-area primitive the PTB legitimately bins to nothing (consistent with every witness:
// shader runs (PCTR §4), no fault, CL clean, empty pool). Every CPU→GPU hand-off is exonerated and the
// VPM output is on-chip/CPU-unreadable (§3), so the only way to settle it is to make the QPU itself tell
// us what it loaded. V3D-26 compiled — with Mesa's real `v3d_compile()` (ver 4.2), authoritative words —
// a passthrough coord VS that ALSO `store_ssbo`s its four loaded attribute components; Mesa lowered the
// store to `mov tmud ×4 → mov tmuau → tmuwt` (the TMUAU-config-coupled words §10 said could not be
// hand-authored to §5 confidence). This arc wires that probe into the M4 bin path as a one-off draw
// BEFORE the real bin: same vertex buffer + attribute record, its own shader record with the coord slot
// pointing at the probe words, and a QUNIFORM_UBO_ADDR uniform aimed at a CPU-visible scratch buffer.
// After the bin idles we invalidate + read the four stored words back — a direct readout of what the VCD
// actually delivered to the QPU for THIS draw's attribute fetch.
//
// PROVENANCE (§5 fabricated-constant law): every PROBE_WORDS entry is transcribed verbatim from the
// V3D-26 harness reference output (scripts/pi-v3d26-mesa-compile.out.txt, the PROBE VS section) — real
// `v3d_compile` bytes, not hand-authored. The uniform stream is likewise the harness stream, with the
// two driver-patched slots filled exactly as the real draw fills them: QUNIFORM_VIEWPORT_{X,Y}_SCALE →
// 8192.0f32 (0x46000000, the V3D-18 contract constant write_geo_uniforms already bakes) and
// QUNIFORM_UBO_ADDR → the scratch buffer's (identity-mapped) V3D address. Those are driver uniform
// VALUES, not shader words — no QPU word is touched.

/// PI-V3D-27 arena regions — placed in the free tail above the M5–M8 battery (top used byte 0x33100 <
/// 0x40000). The bin scratch (tile-alloc / tile-state) is REUSED from the M4 regions: the probe bins
/// first, then `triangle_job` re-zeros + re-cleans OFF_BIN_TILEALLOC / OFF_TILESTATE before the real
/// bin, so the reuse is invisible to the real draw.
const OFF_PROBE_CODE: usize = 0x34000; // probe coord-shader QPU code (25 words = 200 B)
const OFF_PROBE_UNIF: usize = 0x34400; // probe uniform stream (12 words = 48 B)
const OFF_PROBE_SHADREC: usize = 0x34800; // probe GL Shader State Record (32-B aligned) + attr record
const OFF_PROBE_SCRATCH: usize = 0x34C00; // TMU-store target: 4 words the QPU writes (CPU reads back)
const OFF_PROBE_BIN_CL: usize = 0x35000; // probe binning control list (CT0)
const _: () = assert!(OFF_PROBE_BIN_CL + 0x1000 <= ARENA_BYTES);

/// PI-V3D-28 canary window: the 4-word TMU-store target lives at word [0..4]; words [4..PROBE_CANARY_WORDS]
/// are seeded with per-index canaries (0xCA00_00NN) so a store that lands at the WRONG address inside this
/// page reveals itself (which slot flipped, and to what) rather than reading back as an untouched sentinel.
/// 32 words = 128 B stays well inside the 0x400-byte gap to OFF_PROBE_BIN_CL. Word [0..4] hold the
/// 0x55555555 "never-landed" sentinel; the canary tail brackets the target on the high side.
const PROBE_CANARY_WORDS: usize = 32;
const PROBE_CANARY_BYTES: usize = PROBE_CANARY_WORDS * 4;
const _: () = assert!(OFF_PROBE_SCRATCH + PROBE_CANARY_BYTES <= OFF_PROBE_BIN_CL);

/// The Mesa-COMPILED probe coord shader (25 words), transcribed byte-for-byte from the V3D-26 harness
/// PROBE VS output (scripts/pi-v3d26-mesa-compile.out.txt). It is a full coord shader — it loads the
/// four attribute components (`ldvpmv_in` → rf3..rf6), STORES them to SSBO 0 via TMU
/// (`mov tmud ×4 → mov tmuau → tmuwt`), and ALSO emits the six-word STVPMV VPM output so the bin stays
/// legal. threads=2, tmu_count=1. NO word is hand-authored (§5): these are real `v3d_compile` bytes.
const PROBE_WORDS: [u64; 25] = [
    0x3c40_3186_bb80_0000, // nop ; nop ; ldunif
    0x3d90_2183_bc80_5000, // ldvpmv_in rf3, r5 ; nop ; ldunifrf.r0
    0x3d90_72c6_bbf8_00c0, // nop ; mov tmud, rf3 ; ldunifrf.r1     (store attr[0])
    0x3d90_a184_bc80_0000, // ldvpmv_in rf4, r0 ; nop ; ldunifrf.r2
    0x3d90_e185_bc80_1000, // ldvpmv_in rf5, r1 ; nop ; ldunifrf.r3
    0x3c00_32c6_bbf8_0100, // nop ; mov tmud, rf4                   (store attr[1])
    0x3c00_2186_bc80_2000, // ldvpmv_in rf6, r2 ; nop
    0x3c00_32c6_bbf8_0140, // nop ; mov tmud, rf5                   (store attr[2])
    0x3c00_32c6_bbf8_0180, // nop ; mov tmud, rf6                   (store attr[3])
    // PI-V3D-33 byte-precise waddr decode of THIS word (0x3c00_3346_bbec_0000), against Mesa's ver-42 QPU
    // packing (qpu_pack.c: MA=bit44, WADDR_A=bits[43:38]) and the `enum v3d_qpu_waddr` (qpu_instr.h):
    //   bit44 MA=1  → the add-ALU result is a MAGIC write (to a special register, not a regfile slot);
    //   bits[43:38] = 0b001101 = 13 = V3D_QPU_WADDR_TMUAU  (TMUD=11, TMUA=12, TMUAU=13 — cross-checked
    //   against word[5] `mov tmud` whose WADDR_A decodes to 11=TMUD in the very same layout).
    // VERDICT: the encoding is CORRECT — this genuinely names TMUAU (13), NOT TMUA (12). The "TMUA-vs-TMUAU
    // mix-up → default config → a lookup not a write" hypothesis is REFUTED. TMUAU means the TMU consumes
    // the next word of the shader's uniform FIFO as its write config (u5=0xFFFFFFFC, audited 12/12 by
    // [v3d32]); a plain TMUA would have used the last-loaded config. Config delivery is correct by encoding.
    0x3c00_3346_bbec_0000, // nop ; mov tmuau, r3                   (r3 = UBO_ADDR → fire the store)
    0x3d91_2180_f883_50c0, // stvpmv r5, rf3 ; nop ; ldunifrf.r4
    0x5440_2047_ba9a_f0c6, // recip rf7, rf6 ; fmul r1, rf3, r4 ; ldunif
    0x5440_20c0_f8bb_0100, // stvpmv r0, rf4 ; fmul r3, rf4, r5 ; ldunif
    0x5440_2080_f8e7_5147, // stvpmv r5, rf5 ; fmul r2, r1, rf7 ; ldunif
    0x5400_3004_f6cc_21c0, // ffloor r4, r2 ; fmul r0, r3, rf7
    0x3c40_2180_f883_5180, // stvpmv r5, rf6 ; nop ; ldunif
    0x3c00_3181_f583_c000, // ftoiz r1, r4 ; nop
    0x3c00_3182_f680_0000, // ffloor r2, r0 ; nop
    0x3c60_2180_f880_d000, // stvpmv r5, r1 ; nop ; thrsw ; ldunif
    0x3c20_3183_f583_a000, // ftoiz r3, r2 ; nop ; thrsw
    0x3c00_2180_f881_d000, // stvpmv r5, r3 ; nop
    0x3c00_3186_bb81_6000, // vpmwt -
    0x3c20_3184_bb81_5000, // tmuwt r4 ; nop ; thrsw
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// The probe uniform stream (FIFO order matches PROBE_WORDS' ldunif/ldunifrf pops), transcribed from the
/// V3D-26 harness with the two driver-patched slots resolved. `scratch_v3d` is the identity-mapped V3D
/// address the TMU store targets (QUNIFORM_UBO_ADDR).
fn write_probe_uniforms(off: usize, scratch_v3d: u32) -> usize {
    // Reference stream (harness): u0..u3 = component indices 0,1,2,3; u4 = UBO_ADDR (0 placeholder →
    // scratch); u5 = CONSTANT 0xfffffffc; u6/u7 = VIEWPORT_X/Y_SCALE (0 placeholder → 8192.0f32);
    // u8..u11 = CONSTANT 2,3,4,5. Only the four store slots (u0..u4) govern the witness; the scales and
    // trailing constants keep the coord math well-defined so the bin stays legal.
    let unif: [u32; 12] = [
        0, 1, 2, 3,             // ldvpmv_in component indices (verbatim)
        scratch_v3d,            // u4: QUNIFORM_UBO_ADDR → CPU-visible scratch (driver-supplied)
        0xFFFF_FFFC,            // u5: CONSTANT (verbatim)
        0x4600_0000,            // u6: QUNIFORM_VIEWPORT_X_SCALE → 8192.0f32 (driver-supplied)
        0x4600_0000,            // u7: QUNIFORM_VIEWPORT_Y_SCALE → 8192.0f32 (driver-supplied)
        2, 3, 4, 5,             // u8..u11: CONSTANT (verbatim)
    ];
    for (i, wv) in unif.iter().enumerate() {
        arena_write_u32(off + i * 4, *wv);
    }
    unif.len() * 4
}

/// Build the probe's GL Shader State Record + attribute record at OFF_PROBE_SHADREC. The COORD slot
/// points at PROBE_WORDS + the probe uniform stream; the attribute record is byte-identical to the real
/// draw's (same OFF_VTXDATA base, vec4 f32, stride 16, in=0/out=1 segment sizes) so the probe witnesses
/// THIS draw's attribute fetch. The VS/FS slots point at the probe code too — a bin-only job never
/// executes them, so their contents are irrelevant; the addresses only need to be valid arena pointers.
///
/// V3D-31 (CS 4-way-threadable = 1): the [v3d28] store-never-issued verdict was root-caused to the CS
/// thread-end shape. PROBE_WORDS carries `thrsw` on words [18],[19] (a consecutive pair) AND [22]
/// (tmuwt). Cross-referenced against the SAME harness's threads=4 coord/vertex shaders
/// (scripts/pi-v3d26-mesa-compile.out.txt), which each carry exactly ONE terminal `thrsw ; nop ; nop`
/// and NO mid-program switch, the [18]/[19] pair is unambiguously Mesa's mid-shader THREAD SWITCH — a
/// structure that appears only in this multi-segment threads=2 probe. It is NOT a thread-end. With the
/// record declaring the CS as non-threadable (bit 224 = 0), the hardware ran a single thread section
/// and terminated at the [18] switch's delay slots, dropping words [21] vpmwt / [22] tmuwt so the
/// TMU store fired at [9] never drained. This build now sets bit 224 = 1 (CS 4-way threadable), matching
/// the working `build_shader_record`, so the hardware honours the [18]/[19] switch and runs on to the
/// terminal [22] thrsw where tmuwt drains the store. The Mesa artifact (PROBE_WORDS) stays byte-verbatim
/// (§5 untouched); only the record's threading declaration changes. Returns the attribute count.
fn build_probe_shader_record() -> u32 {
    // V3D-39 WITNESS (Task A — the dispatch wall): the [v3d38] record diff proved the probe record now
    // equals the confirmed-dispatching M4 record on EVERY field except the two CS pointers (cs_code_addr,
    // cs_uniforms_addr) — FS/VS borrowed, CL byte-identical (v3d36), threading refuted both ways (v3d37).
    // Yet the probe coord shader still NEVER dispatches (valid_instr=0). The ONLY remaining variable is the
    // CS PROGRAM ITSELF: PROBE_WORDS is Mesa's "PROBE VS" — a VERTEX-shader-shaped threads=2 program with a
    // MID-shader thrsw (words [18],[19]) — sitting in the COORD slot. The V3D-26 harness only ever emitted
    // three VS variants (coord VS, render VS, probe VS); it never compiled a bin_mode/CS variant, and no
    // CS-mode probe artifact exists in scripts/ to swap in (the Mesa harness checkout — session 0f1145d9's
    // scratchpad — is deleted, so it cannot be re-run here). So land the decisive witness the brief names:
    // put M4's KNOWN-DISPATCHABLE CS program (CS_VS_WORDS @ OFF_CS_CODE — a pure VPM passthrough that
    // provably bins with valid_instr>0) into the probe record's CS slot, keeping everything else the probe
    // record (probe's CS uniforms are irrelevant to the dispatch question). Verdict via the [v3d35] PROBE
    // battery: if the probe bin now dispatches (valid_instr/coord > 0), the CS BYTES gate dispatch — the
    // probe program's thread-switching/threads=2 shape is refused by v42 coord/bin dispatch. If it STILL
    // reads valid_instr=0 with a dispatchable program in the slot, the gate is the record/arena ADDRESS
    // window (the 0x34800 probe-record range vs the 0x1C000 M4 range) and the next arc hunts address windows.
    let code = (arena_phys() + OFF_CS_CODE) as u64; // V3D-39: M4's CS program, not OFF_PROBE_CODE
    let unif = (arena_phys() + OFF_PROBE_UNIF) as u64;
    let defaults = (arena_phys() + OFF_DEFAULT_ATTRS) as u64;
    let vtx = (arena_phys() + OFF_VTXDATA) as u64;
    // V3D-38 BORROW: the FS/VS slots now point at M4's KNOWN-GOOD fragment + vertex programs (published by
    // triangle_job before probe_job runs; cleaned to RAM in probe_job's clean batch). See the FS/VS emit
    // block below for the rationale — a bin-only job executes only the CS, so borrowing M4's validated
    // FS/VS while keeping the probe's TMU-store program in the CS slot is semantically safe and removes the
    // last record-content variable the hardware might validate/prefetch at bin time.
    let fs = (arena_phys() + OFF_FS_CODE) as u64;
    let vs = (arena_phys() + OFF_VS_CODE) as u64;
    let fs_unif = (arena_phys() + OFF_FS_UNIF) as u64;
    let vs_unif = (arena_phys() + OFF_VS_UNIF) as u64;

    let mut rec = [0u8; 36];
    sf(&mut rec, 1, 1, 1); // Enable clipping
    sf(&mut rec, 24, 8, 0); // Number of varyings in Fragment Shader
    // VPM segment sizes — same Mesa contract as the real record (§10): out = 1 sector, in = 0.
    sf(&mut rec, 32, 4, 1); // Coord Shader output VPM segment size
    sf(&mut rec, 40, 4, 0); // Coord Shader input VPM segment size
    sf(&mut rec, 48, 4, 1); // Vertex Shader output VPM segment size
    sf(&mut rec, 56, 4, 0); // Vertex Shader input VPM segment size
    sf(&mut rec, 64, 32, defaults); // Address of default attribute values
    // FS / VS slots — V3D-38 BORROW: the bin-only job executes ONLY the CS, so the FS/VS slots are never
    // dispatched. Prior arcs pointed them at the probe program (a VS-shaped, multi-segment TMU-store coord
    // shader with mid-shader thrsw) — a nonsense program to sit in the FS slot. Even at bin time the
    // hardware may VALIDATE or PREFETCH the FS/VS descriptors (single-seg/flags/addr) as part of accepting
    // the shader-state record, and refuse the whole dispatch on a malformed FS. Point the FS/VS slots at
    // M4's KNOWN-GOOD, provably-dispatching fragment/vertex programs + their uniform streams; only the CS
    // keeps the probe's TMU-store program. This removes the last record-content variable while keeping the
    // probe's purpose intact (the store fires at bin time from the CS). See the [v3d38] record diff.
    // GL Shader State Record per-shader flag group (Mesa `v3dX(pack)`, v3d_packet_v42.xml / v3d_emit.c):
    // bit0 = 4-way threadable = (prog_data.base.threads == 4), bit1 = 2-way threadable = (threads == 2),
    // bit2 = propagate NaNs.
    //
    // V3D-36 (THIS arc): the P34 capture proved the probe coord shader NEVER DISPATCHES — the
    // probe-scoped PCTR battery read valid_instr=0 / cycle_count=508 (SHADER NEVER RAN) while the SAME
    // boot's M4 coord shader read valid_instr=55 (SHADER RAN). The [v3d36] CL decode (probe_job +
    // triangle_job) shows the two binning CLs are byte-for-byte identical — same build_bin_cl_generic,
    // same NUMBER_OF_LAYERS / TILE_BINNING_MODE_CFG / clip+viewport state / VCM_CACHE_SIZE /
    // GL_SHADER_STATE / VERTEX_ARRAY_PRIMS(mode=4,count=3) / FLUSH — differing only in the 27-bit
    // GL_SHADER_STATE record pointer. So the control list is EXONERATED; the dispatch gate is in the
    // shader-state record the pointer selects. The ONE dispatch-governing field that differs from the
    // confirmed-dispatching M4 coord shader is threadability: M4 declares the CS 4-WAY (bit 224) and its
    // coord shader dispatches; V3D-32 flipped the probe CS to 2-WAY (bit 225, 4-way clear) and — per the
    // only probe-scoped reading we have — its coord shader stopped dispatching entirely. The V3D-31→32
    // threadability flip is exactly where the dispatch regressed, so restore the CS (and, to mirror the
    // working record, the FS/VS slots) to 4-WAY threadable. The store-drain concern V3D-32 raised is a
    // DOWNSTREAM symptom that only bites once the thread runs; getting valid_instr>0 is the gate this arc
    // targets, and the next boot's [v3d35] reading is its verdict. PROBE_WORDS stays byte-verbatim.
    sf(&mut rec, 96, 1, 1); // FS 4-way threadable (mirror the M4 record)
    sf(&mut rec, 98, 1, 1); // FS propagate NaNs (v42)
    sf(&mut rec, 99, 29, fs >> 3); // FS code address — V3D-38 BORROW: M4's known-good fragment shader
    sf(&mut rec, 128, 32, fs_unif); // FS uniforms address — M4's FS stream (colour + TLB config)
    sf(&mut rec, 160, 1, 1); // VS 4-way threadable (mirror the M4 record)
    sf(&mut rec, 162, 1, 1); // VS propagate NaNs (v42)
    sf(&mut rec, 163, 29, vs >> 3); // VS code address — V3D-38 BORROW: M4's known-good vertex shader
    sf(&mut rec, 192, 32, vs_unif); // VS uniforms address — M4's VS read-offset stream
    // Coord shader — the one the bin runs. V3D-36: 4-WAY threadable, matching the M4 coord shader that
    // provably dispatches (valid_instr=55). The 2-way declaration (V3D-32) is the dispatch regression the
    // P34 capture caught (valid_instr=0).
    sf(&mut rec, 224, 1, 1); // CS 4-way threadable (mirror the dispatching M4 coord shader)
    sf(&mut rec, 226, 1, 1); // CS propagate NaNs (v42)
    sf(&mut rec, 227, 29, code >> 3); // CS code address — V3D-39: M4's known-dispatchable CS_VS_WORDS
    sf(&mut rec, 256, 32, unif); // CS uniforms address (probe stream: indices + UBO_ADDR + scales)
    arena_write_bytes(OFF_PROBE_SHADREC, &rec);

    // Attribute record — byte-identical to the real draw's (build_shader_record): vec4 f32, stride 16.
    let mut attr = [0u8; 16];
    sf(&mut attr, 0, 32, vtx); // Address
    sf(&mut attr, 32, 2, 3); // Vec size (4 components)
    sf(&mut attr, 34, 3, 2); // Type = Attribute float
    sf(&mut attr, 40, 4, 4); // Number of values read by Coordinate shader
    sf(&mut attr, 44, 4, 4); // Number of values read by Vertex shader
    sf(&mut attr, 64, 32, 16); // Stride
    sf(&mut attr, 96, 32, 0xFFFF); // Maximum Index
    arena_write_bytes(OFF_PROBE_SHADREC + 36, &attr);
    1
}

// ═══ V3D-58 — the cross-engine write/retire asymmetry, and where the bin frame actually opens ═══════
//
// P56 metal closed every remaining CL-side suspect: the bin control list is byte-exact to Mesa's v42
// encoding (§31), the tile-state array is Mesa-sized (256 B), `BPOS=0` is issued in `v3d_bin_job_run`'s
// exact position, submission is SOUND, the CLE walks BA→EA, the QPU runs to program-end, the firmware
// clock is ACTIVE at 500 MHz, the MMU is enabled and fault-free with our table latched — and the poison
// still comes back FULLY INTACT over both the tile-state array (64/64 words) and the whole 32 KiB pool
// (8192/8192 words), with `[v3d56] landing` reporting **zero** changed pages across the entire 64-page
// arena. `FLDONE` is unmasked and never latches.
//
// The brief's fork for this arc was "binner never fetches/starts" vs "starts but the PTB store path is
// blocked". Two readings ALREADY IN THE P56 CAPTURE decide large parts of it, and neither has been
// stated on serial. V3D-58 exists to state them, and to sample the one window nobody has sampled.
//
// ── Refuted here, from evidence already on the wire ────────────────────────────────────────────────
//
// **R1 — "the V3D store path is dead" is REFUTED.** The same boot that wedges the bin prints
// `M3 clear-job PASS (GPU cleared buffer; CPU byte-verified)` with `RFC=1`. The render CLE (CT1) ran an
// RCL, its `STORE_TILE_BUFFER_GENERAL` landed in arena memory, and the CPU byte-verified it against a
// `0xDEADBEEF` sentinel. That store went through the SAME V3D MMU (same `MMU_CTL`, same `PT_PA_BASE`,
// same identity-mapped arena), the SAME L2T/slice cache configuration, the SAME GMP (`PROT_ENABLE=0`),
// the SAME AXI fabric and the SAME 500 MHz clock as the bin. So every hypothesis that blocks V3D writes
// *globally* — MMU write-permission, GMP silent drop, L2T/slice ordering, a dead write clock, an AXI/QoS
// floor — is refuted by a working engine, not by argument. Whatever is wrong is **bin-path-exclusive**.
//
// **R2 — "the PTB never starts" is REFUTED *as stated*.** `BPCS` (bytes REMAINING in the pool) read
// `0x5000` against a `0x8000` pool, exactly complementing the `0x3000` `BPCA` advance. A pool the PTB
// never entered would leave `BPCS` at the size we latched into `CT0QMS`. The PTB therefore *did* act on
// `START_TILE_BINNING`: it performed the per-tile initial-block reservation and the two up-front 4 KiB
// chunk allocations that `v3d_tile_alloc_sizes` describes (§30). It started, it allocated — and then it
// wrote nothing and never closed the frame.
//
// So the surviving statement is narrower than either horn of the brief's fork: **the bin frame OPENS
// (`PCS.BMACTIVE=1`, pool reserved) and never CLOSES (`FLDONE` dead, `BFC` Δ0, `BMACTIVE` stuck set),
// while a render frame on the same block opens, writes and closes cleanly.**
//
// ── What V3D-58 adds ───────────────────────────────────────────────────────────────────────────────
//
// **(A) `[v3d58] xengine`** — the asymmetry, on one line, with the shared-resource inventory beside it,
// so the refutation above is a *witness* rather than a doc claim. It reports the render engine's
// verdict (`clear_job`, captured at M3) and the bin engine's verdict from the same boot, and names the
// resources both share as re-audited register reads taken at bin time.
//
// **(B) `[v3d58] station`** — the un-sampled window. Every prior boot read `PCS`/`BPCA`/`BPCS` only in
// the post-wait wedge dump, so we have never known *when* `BMACTIVE` sets or *when* the pool reservation
// happens. Five stations bracket the CT0 kick: S0 (before any CT0 programming), S1 (after the
// QMA/QMS/QTS latch, before QBA), S2 (after QBA, before the GO), S3 (immediately after the GO), S4 (at
// wait exit). Each records `PCS`, `BPCA`, `BPCS`, `BFC`, `CT0CS`, `CT0CA`. This is the discriminator the
// brief asked for, and its readings are mutually exclusive:
//
//   * `BMACTIVE` **already 1 at S0** ⇒ the block left reset (or the M3 CT1 job) with a bin frame still
//     open, and our `START_TILE_BINNING` is stacking onto it. That would explain the entire campaign at
//     a stroke and it has never been checked. The fix would be a CT0 thread/frame abort before the kick.
//   * `BPCS` **drops at S1** (the `CT0QMS` write alone) ⇒ the `0x3000` advance is a register-latch
//     artifact and R2 above is wrong; the PTB may never have run.
//   * `BPCS` **drops between S2 and S4** ⇒ the reservation is a real `START_TILE_BINNING` action and R2
//     stands: the PTB executes, so the wall is the FLUSH/frame-close step alone.
//   * `BMACTIVE` **0 at S0 and 0 at S4** ⇒ the frame never opened at all and `FLDONE`'s absence is a
//     consequence, not the defect.
//
// **(C) `[v3d58] rerender`** — the negative control the campaign never ran. After the bin wedges, re-run
// the *proven-good* CT1 clear job. If it still passes, the wedge is confined to CT0/PTB and the block is
// otherwise healthy — which makes "the bin frame-close unit specifically" the whole remaining surface.
// If it now FAILS, the bin kick wedged shared state (CLE, L2T, MMU or the pipeline), and the bin defect
// has a blast radius that every prior post-bin readback in this file was taken inside of. Either way it
// is a fact no boot has ever collected, and it costs one re-run of an existing, verified job.
//
// ── Explicitly NOT armed ───────────────────────────────────────────────────────────────────────────
//
// A CT0 thread reset (`CTRSTA`) is the obvious follow-on if S0 reads `BMACTIVE=1`, and it is deliberately
// NOT implemented here. Mainline `v3d_regs.h` defines **no** bit fields for `V3D_CLE_CT0CS` (§30), so
// every `CTnCS` bit beyond the `CTRUN` this driver already relies on would be a fabricated constant —
// the exact class of bug PI-V3D-4 and PI-V3D-6 were. V3D-58 *collects the evidence* that would justify
// it; the write is a next-arc decision taken against a corroborated bit position.
//
// **Scope.** (A) and (B) are read-only MMIO. (C) re-runs `clear_job(None)` — an existing verified path,
// with `None` so it does not repaint the panel — and touches only M3's own arena buffers, never the bin
// regions the `[v3d55]`/`[v3d56]` readbacks have already been taken from at that point. All three are
// inside the V3D probe battery, which short-circuits at `BLOCK-DOWN` on QEMU, so none of it reaches a
// default boot log (quiet-boot law).

/// V3D-DEEP — the probe-budget line. Every diagnostic in this file is cheap EXCEPT three, whose
/// verdicts are already BANKED on metal and which together cost ~3.5 s of anti-hang backstop (plus
/// metres of serial) on every armed boot — the visible "stall at the M3 square":
///
///   * the `[v3d48]` empty-frame bisection ladder — 6 rungs, each ending in a ~0.5 s FLDONE backstop
///     that always times out (~3.0 s), all six verdicts banked as non-retire;
///   * `[v3d59] frameclose` — 64 x 1 ms post-wedge samples (~64 ms), banked DEAD-OPEN: zero bit
///     changes across the whole extra window, so re-running it every boot buys nothing;
///   * `[v3d58] rerender` — one extra full CT1 clear job, banked clean.
///
/// The fast probes (the `[v3d40]` probe kick and its single FLDONE wait, all the pure-read decodes —
/// `[v3d54/55/56/57/58/59] ctstate`, stations, poison, landing) stay on by default: they are the live
/// wire read of THIS boot. Arm the slow half with `UNAOS_V3D_DEEP=1` (feature `v3d_deep`) when the
/// bench is deliberately re-opening one of the banked questions. When it is off, `bringup` prints one
/// `[v3d] deep=off …` line naming exactly what was skipped, so a wire read is never silently short.
const V3D_DEEP: bool = cfg!(feature = "v3d_deep");

/// PI-V3D-69 — the READ-ONLY half of the initialisation-diff arc: the `[v3d69] fabric` stations, which
/// read the power/bridge state between the SoC and the V3D block (PM_GRAFX, both V3D ASB control words
/// in both candidate ASB blocks, the MMU-cache enable) and print it against what mainline's
/// `bcm2835_asb_power_on` + `v3d_mmu_set_page_table` leave behind. Pure MMIO reads plus one banked
/// control rung; rides `V3D_DEEP` because its differential station sits either side of that rung.
///
/// PI-V3D-70 corrects the identity column against the SHIPPING DTB: both ASB bases are DT-correct,
/// the rpivid block's DT range (len 0x20) has NO identity register, and the legacy block's
/// `ASB_AXI_BRDG_ID` at +0x20 (in range at len 0x24) is the one identity check that exists. The
/// station lines carry the `[v3d70]` tag from that correction on.
const V3D69_FABRIC: bool = V3D_DEEP;

/// PI-V3D-69 — the WRITE half, deliberately on its own knob (`UNAOS_V3D69_REENABLE=1`, feature
/// `v3d69_reenable`) and OFF by default even on a deep boot. It re-runs the mainline enable sequence
/// (`enable_pm_asb` = deassert PM_V3DRSTN, release the master then the slave bridge) and re-kicks the
/// `[v3d69]` control rung. It is a SEPARATE knob because it is the only part of this arc that writes,
/// and because it is only meaningful on the one outcome branch where the read half found a bridge that
/// is NOT released: re-releasing an already-released bridge perturbs a live block for nothing. When the
/// knob is armed but the read half found the fabric clean, the rung declines to run and says so.
const V3D69_REENABLE: bool = cfg!(feature = "v3d69_reenable");

/// Gate for the `[v3d58] station` progression sampling and the `[v3d58] xengine` asymmetry line.
/// Read-only MMIO; one flip to `false` silences both. Cheap — default-on.
const V3D58_STATIONS: bool = true;

/// Gate for the `[v3d58] rerender` negative control (re-runs the M3 clear job after the wedged bin).
/// Costs one extra CT1 job on metal and repaints nothing (`clear_job(None)`). Banked clean on metal —
/// V3D-DEEP only.
const V3D58_RERENDER_CONTROL: bool = V3D_DEEP;

/// Captured at M3: did the RENDER engine (CT1) complete a frame and land a byte-verified store?
/// This is the reference the `[v3d58] xengine` line is drawn against — see R1 above.
static V3D58_RENDER_OK: AtomicBool = AtomicBool::new(false);
/// Set once `clear_job` has actually been attempted, so `xengine` can distinguish "render FAILED" from
/// "render never ran this boot" (the probe is reachable on paths that skip M3).
static V3D58_RENDER_RAN: AtomicBool = AtomicBool::new(false);

/// Record the render engine's verdict for the V3D-58 cross-engine comparison. Called once, at M3.
fn v3d58_note_render(ok: bool) {
    V3D58_RENDER_RAN.store(true, Ordering::Release);
    V3D58_RENDER_OK.store(ok, Ordering::Release);
}

/// One sampling station around the CT0 bin kick — see `[v3d58] station`.
#[derive(Clone, Copy)]
struct V3d58Station {
    pcs: u32,
    bpca: u32,
    bpcs: u32,
    bfc: u32,
    ct0cs: u32,
    ct0ca: u32,
    // PI-V3D-59 additions — all pure reads, none of them ever sampled before this arc.
    ct0sync: u32,
    ct1sync: u32,
    bpoa: u32,
    bpos: u32,
    bxcf: u32,
    ct0lc: u32,
    ct0pc: u32,
}

/// Sample the bin-frame state registers. Pure reads; safe at any point in the kick sequence.
fn v3d58_sample() -> V3d58Station {
    V3d58Station {
        pcs: mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS),
        bpca: mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA),
        bpcs: mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS),
        bfc: mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC),
        ct0cs: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS),
        ct0ca: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA),
        ct0sync: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0SYNC),
        ct1sync: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1SYNC),
        bpoa: mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOA),
        bpos: mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOS),
        bxcf: mmio_read(V3D_CORE0_BASE, V3D_PTB_BXCF),
        ct0lc: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0LC),
        ct0pc: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0PC),
    }
}

/// Emit the five-station bin-frame progression and its verdict.
///
/// `pool_base` / `pool_size` are the values latched into `CT0QMA`/`CT0QMS`, so the reservation can be
/// stated as a delta against what we handed the PTB rather than as a bare register value.
fn v3d58_emit_stations(what: &str, s: &[V3d58Station; 5], pool_base: u32, pool_size: u32) {
    if !V3D58_STATIONS {
        return;
    }
    let names = ["S0 pre-program", "S1 post-QMA/QMS/QTS", "S2 post-QBA", "S3 post-GO", "S4 wait-exit"];
    for (i, st) in s.iter().enumerate() {
        serial_println!(
            "::   [v3d58] {} — PCS={:#010x} (BMACTIVE={} BMBUSY={} RMACTIVE={} RMBUSY={} BMOOM={}) BPCA={:#010x} BPCS={:#010x} BFC={:#010x} CT0CS={:#010x} CT0CA={:#010x} ::",
            names[i], st.pcs,
            (st.pcs & V3D_PCS_BMACTIVE != 0) as u32,
            (st.pcs & V3D_PCS_BMBUSY != 0) as u32,
            (st.pcs & V3D_PCS_RMACTIVE != 0) as u32,
            (st.pcs & V3D_PCS_RMBUSY != 0) as u32,
            (st.pcs & V3D_PCS_BMOOM != 0) as u32,
            st.bpca, st.bpcs, st.bfc, st.ct0cs, st.ct0ca
        );
    }
    let bm0 = s[0].pcs & V3D_PCS_BMACTIVE != 0; // frame already open BEFORE we touched CT0?
    let bm4 = s[4].pcs & V3D_PCS_BMACTIVE != 0;
    // Where did the pool reservation happen? On the reading this driver carries from §30, BPCS is the
    // PTB's REMAINING-bytes register, so a drop below the size we latched is the PTB consuming the pool.
    // Which station it drops at is the whole question: at S1 it is a side effect of the CT0QMS write, at
    // S3/S4 it is START_TILE_BINNING acting.
    //
    // **S0 is EXCLUDED from this scan.** At S0 nothing has been latched into CT0QMS yet, so whatever BPCS
    // reads there is the block's reset/stale value and is NOT comparable to `pool_size`. Including it
    // would let any small leftover value score as `resv_at=0` and fire the "CT0QMA/QMS latch artifact"
    // verdict for a write that had not happened. S0's BPCS is reported on its own terms instead.
    let s0_bpcs_below = s[0].bpcs < pool_size; // stale/reset — descriptive only, never a verdict input
    // A drop **to zero** is FULL consumption — the strongest "the PTB acted" reading there is — so the
    // test is "below the latched size, zero included", not "nonzero and below".
    let mut resv_at: i32 = -1;
    for (i, st) in s.iter().enumerate().skip(1) {
        if st.bpcs < pool_size {
            resv_at = i as i32;
            break;
        }
    }
    let resv_full = resv_at >= 0 && s[resv_at as usize].bpcs == 0;
    let adv4 = s[4].bpca.wrapping_sub(pool_base);
    let verdict = if bm0 {
        "BMACTIVE was ALREADY SET at S0 — before this driver wrote a single CT0 register. A bin frame was left OPEN by the reset path or by the preceding CT1 job, and every START_TILE_BINNING since has been stacking onto a frame that was never closed. This is a bring-up defect UPSTREAM of everything V3D-40..57 audited, and it would produce exactly the observed signature (list walks, pool reserves, FLUSH never closes a frame it did not open). Next arc: a corroborated CT0 frame/thread abort before the kick — NOT the fabricated CTRSTA bit (see the V3D-58 facts block)"
    } else if !bm4 {
        "BMACTIVE is CLEAR at S0 and STILL CLEAR at S4 — the bin frame never opened at all. START_TILE_BINNING did not put the pipeline into binning mode, so the missing FLDONE is a CONSEQUENCE, not the defect: chase what gates BMACTIVE (frame enables / PTB bring-up), not the flush unit"
    } else if resv_at >= 3 {
        "BMACTIVE set only after the GO and the pool reservation happened at/after S3 — START_TILE_BINNING genuinely EXECUTED: the PTB opened the frame and consumed the pool. The frame then never closed. Combined with [v3d58] xengine (the render engine writes and retires on this same block), the remaining surface is the BIN FLUSH/frame-close step alone"
    } else if resv_at == 1 {
        "the pool reservation was ALREADY VISIBLE at S1 — BPCS dropped below the size we latched by the CT0QMA/QMS/QTS write alone, before CT0QBA and before the GO. The 0x3000 advance is then a register-latch artifact and NOT evidence that the PTB ran: the V3D-56 'the PTB reserved' reading is wrong and 'the binner never started' is back on the table"
    } else if resv_at == 2 {
        "BPCS dropped at S2 — after CT0QBA but still BEFORE the GO. Neither a pure latch artifact nor START_TILE_BINNING acting; the queue-begin write itself is moving PTB state, which no model of this block predicts. Treat as a NEW fact and re-read the raw stations"
    } else {
        "mixed reading — BMACTIVE opened after the GO but the pool reservation station is ambiguous; read BPCS across the five stations by hand before drawing a verdict"
    };
    serial_println!(
        ":: V3D: [v3d58] station ({}) — pool base={:#010x} size={:#x} (as latched into CT0QMA/CT0QMS) | BMACTIVE S0..S4 = {}{}{}{}{} | BPCS S0={:#x} (PRE-LATCH: reset/stale, below-size={} carries NO verdict) S1={:#x} -> S4={:#x} | first-drop-station={} (S1..S4 only{}) | BPCA advance at S4 = {:#x} | BFC {:#010x}->{:#010x} (Δ{}) — {} ::",
        what, pool_base, pool_size,
        (s[0].pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (s[1].pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (s[2].pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (s[3].pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (s[4].pcs & V3D_PCS_BMACTIVE != 0) as u32,
        s[0].bpcs, s0_bpcs_below as u32, s[1].bpcs, s[4].bpcs,
        if resv_at < 0 { -1 } else { resv_at },
        if resv_full { ", dropped to ZERO = FULL pool consumption" } else { "" },
        adv4,
        s[0].bfc, s[4].bfc, s[4].bfc.wrapping_sub(s[0].bfc),
        verdict
    );
}

/// The cross-engine asymmetry line — R1 above, stated with numbers instead of prose.
///
/// `bin_retired` is this boot's bin verdict (`FLDONE` latched). The render verdict comes from the M3
/// clear job captured in `V3D58_RENDER_OK`. The shared-resource columns are re-read HERE, at bin time,
/// so the line cannot claim a shared configuration it did not observe.
fn v3d58_xengine(what: &str, bin_retired: bool, bin_wrote: bool) {
    if !V3D58_STATIONS {
        return;
    }
    let render_ran = V3D58_RENDER_RAN.load(Ordering::Acquire);
    let render_ok = V3D58_RENDER_OK.load(Ordering::Acquire);
    let mmu_ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let pt_base = mmio_read(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE);
    let l2t = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TCACTL);
    let rfc = mmio_read(V3D_CORE0_BASE, V3D_CLE_RFC);
    let bfc = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    let verdict = if !render_ran {
        "NO VERDICT — the render engine never ran this boot, so there is no reference to compare the bin against. This line is only evidence when M3 has executed"
    } else if render_ok && !bin_retired && !bin_wrote {
        "ASYMMETRY CONFIRMED. The RENDER engine (CT1) completed a frame and landed a byte-verified store, while the BIN engine (CT0) on the SAME block, SAME MMU table, SAME L2T config, SAME clock and SAME arena consumed its list and wrote nothing. Every hypothesis that blocks V3D memory writes GLOBALLY is therefore refuted by a working engine rather than by argument: MMU write-permission, GMP silent drop, L2T/slice ordering, a dead write clock, and an AXI/QoS floor are ALL off the table. The defect is bin-path-exclusive — read [v3d58] station for whether the bin frame opens at all"
    } else if render_ok && bin_retired {
        "BOTH engines retired — the bin wall is GONE this boot. Whatever changed since P56 is the fix; diff the arc"
    } else if render_ok && bin_wrote {
        "the render engine retires AND the bin engine wrote memory but did not signal FLDONE — the narrowest possible target: the frame-close LATCH, with the PTB store path proven live on both engines"
    } else {
        "the RENDER engine did not verify either — this is NOT the bin-exclusive asymmetry. A block on which even the proven-good clear job fails is broken upstream of the bin question; fix M3 before reading any bin verdict"
    };
    serial_println!(
        ":: V3D: [v3d58] xengine ({}) — RENDER(CT1) ran={} verified-store={} RFC={:#010x} | BIN(CT0) retired={} wrote-any-arena-byte={} BFC={:#010x} | SHARED at bin time: MMU_CTL={:#010x} (ENABLE={} faults={:#x}) PT_PA_BASE={:#010x} L2TCACTL={:#010x} arena={:#x}+{:#x} — {} ::",
        what, render_ran as u32, render_ok as u32, rfc,
        bin_retired as u32, bin_wrote as u32, bfc,
        mmu_ctl,
        (mmu_ctl & V3D_MMU_CTL_ENABLE != 0) as u32,
        mmu_ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED),
        pt_base, l2t, arena_phys(), ARENA_BYTES,
        verdict
    );
}

/// The negative control — re-run the proven-good CT1 clear job AFTER the bin has wedged, and report
/// whether the block is still able to render. See (C) in the V3D-58 facts block.
///
/// `clear_job(None)` re-seeds its own target with the `0xDEADBEEF` sentinel and byte-verifies the GPU's
/// store; passing `None` suppresses the panel blit so this control is invisible on screen. It touches
/// only M3's arena buffers (`0x0`/`0x8000`/`0x9000`, disjoint from the pool at `0x12000`, the tile-state
/// array at `0x11000` and the probe scratch at `0x34C00`).
///
/// **It must be called LAST in `probe_job`**, after `[v3d41]`, the `[v3d28]` MMU-fault read and the
/// V3D-28 post-bin L2T drain — not merely after the `[v3d55]`/`[v3d56]` readbacks. A CT1 job retires a
/// render frame (inflating an RFC delta measured against a pre-GO snapshot), can raise its own MMU
/// fault, and puts L2T traffic between bin idle and the TMU drain. The memory regions are disjoint, so
/// the contamination is register/cache state only — which ordering alone fixes.
fn v3d58_rerender_control(what: &str) {
    if !V3D58_RERENDER_CONTROL {
        return;
    }
    let before_ok = V3D58_RENDER_OK.load(Ordering::Acquire);
    if !V3D58_RENDER_RAN.load(Ordering::Acquire) || !before_ok {
        serial_println!(
            ":: V3D: [v3d58] rerender ({}) — SKIPPED: the M3 clear job did not pass earlier this boot, so a post-bin re-run has no baseline to be a control against ::",
            what
        );
        return;
    }
    let after_ok = clear_job(None);
    serial_println!(
        ":: V3D: [v3d58] rerender ({}) — M3 clear job re-run AFTER the wedged bin: pre-bin={} post-bin={} (CT1, panel blit suppressed) — {} ::",
        what, before_ok as u32, after_ok as u32,
        if after_ok {
            "the RENDER engine STILL works after the bin wedge — the wedge is CONFINED to CT0/PTB and leaves the CLE, MMU, L2T and store path healthy. Every post-bin readback this file takes is therefore being taken on a sound block, and the remaining surface really is the bin frame-close unit alone"
        } else {
            "the RENDER engine has STOPPED WORKING since the M3 baseline. TWO readings fit and this line does NOT choose between them: (a) the BIN WEDGE has a blast radius beyond CT0, leaving shared state (CLE, pipeline, L2T or MMU) broken — in which case every bin readback in this file ([v3d55] tilestate/pool, [v3d56] poison/landing, [v3d41], [v3d28]) was taken on a block already in a bad state and must be re-examined; or (b) something OTHER than the bin frame between M3 and here broke CT1 — bin_prejob_bpos_clear, pctr_setup_cs_witness (which re-arms the PCTR block), the v3d55 clock/MISCCFG audit with its two mailbox round-trips, the L2T/SLC flushes, or the whole-arena cache invalidate. Bisect by re-running this control at each of those points before attributing the failure to the bin"
        }
    );
}

// ══ PI-V3D-59 — the paradox, the mainline audit that closed four theories, and what is left ═══════
//
// P57 metal left the arc holding a contradiction rather than a suspect:
//
//   * `BMACTIVE` S0..S4 = `00001` — the bin frame OPENS, and only after the GO;
//   * `BPCS` drops below the latched pool size ALREADY AT S1 — before `CT0QBA`, before the GO — so the
//     `0x3000` "reservation" is a REGISTER-LATCH ARTIFACT of the `CT0QMS` write and V3D-56's "the PTB
//     reserved" reading is retracted (§30's R2 falls);
//   * `CT0CA` walks `BA→EA`, `CT0CS` reads 0 after, `BFC` Δ0, `FLDONE` never latches, the poison in the
//     pool and the tile-state array is fully INTACT — no PTB memory write ever happens;
//   * `[v3d58] xengine` + `[v3d58] rerender`: a CT1 render frame on the SAME block byte-verifies a store
//     before the bin and again after it. The block is sound; the defect is bin-path-exclusive.
//
// So: `START_TILE_BINNING` opens a frame and initialises no tile state, and `FLUSH` closes nothing.
//
// ── (a)/(b) THE MAINLINE AUDIT — four theories closed by citation, not by argument ────────────────
//
// Sources read for this arc, facts-only: Linux `drivers/gpu/drm/v3d/v3d_sched.c` + `v3d_regs.h` +
// `drivers/gpu/drm/vc4/vc4_regs.h` (GPL-2.0-only), and Mesa `src/gallium/drivers/v3d/v3dx_draw.c`,
// `v3dx_job.c`, `v3d_job.c`, `src/broadcom/cle/v3d_packet.xml` (MIT).
//
// T1 — "BPOS=0 starves the PTB; mainline arms an overflow pool BEFORE the frame."  **REFUTED.**
//      `v3d_bin_job_run` writes `V3D_PTB_BPOS = 0` as its FIRST register write of every bin job, under
//      the queue lock; its comment there gives the reason as clearing the overflow allocation so a
//      previous job's overflow block is not carried into this one (paraphrased — the kernel is
//      GPL-2.0-only and UnaOS is GPL-3.0-or-later, so its comment TEXT cannot ride in this tree; the
//      FACT it states is not copyrightable and is what we rely on). `BPOA`/`BPOS` are written NOWHERE else in the driver
//      except `v3d_overflow_mem_work`, which runs only in RESPONSE to the `V3D_INT_OUTOMEM` interrupt.
//      Every Mesa frame on every Pi 4 therefore enters binning with `BPOS=0` and no overflow block.
//      A zero overflow pool cannot be what stops the PTB from starting — mainline never has one either.
//      (The arc brief ranked this HIGH; the citation demotes it. `V3D59_ARM_OVERFLOW` below converts the
//      citation into a metal demonstration if anyone wants the refutation by experiment.)
//
// T2 — "the per-frame `CT0QMA`/`QMS`/`QTS` latch resets an open frame / should be written once."
//      **REFUTED as a divergence.** `v3d_bin_job_run` writes `CT0QMA`+`CT0QMS` (guarded by `job->qma`)
//      then `CT0QTS | V3D_CLE_CT0QTS_ENABLE` (guarded by `job->qts`) then `CT0QBA` then `CT0QEA`, on
//      EVERY bin job — per-frame, in exactly our order, with exactly our values-present condition.
//      The S1 artifact is explained and benign: latching `CT0QMS` is what (re)loads the PTB's
//      remaining-size register, so `BPCS` tracking the write is the register doing its job. Mainline
//      does the identical write on every frame it retires.
//
// T3 — "`CT0QTS` is a tile COUNT, not the tile-state ADDRESS."  **REFUTED.** Mesa `v3d_job.c` sets
//      `job->submit.qts = job->tile_state->offset` — a BO address — alongside `qma = tile_alloc->offset`
//      and `qms = tile_alloc->size`, under the comment *"On V3D 4.1, the tile alloc/state setup moved to
//      register writes instead of binner packets."* QTS is an address; ENABLE is `BIT(1)`. Ours matches.
//
// T4 — "the bin CL is missing a terminator/semaphore packet (`FLUSH_ALL_STATE`, `INCREMENT_SEMAPHORE`)."
//      **REFUTED.** Mesa `v3dX(bcl_epilogue)` (v3dx_job.c) emits, for a job with no transform feedback
//      and no primitives-generated query, exactly ONE packet: `FLUSH` — *"We just FLUSH here to tell the
//      HW to cap the bin CLs with a return. Any remaining state changes won't be flushed to the bins
//      first -- you would need FLUSH_ALL for that, but the HW for hasn't been validated"*. No semaphore
//      packet is emitted at all on the modern V3D path. And `v3dX(start_binning)` (v3dx_draw.c) is
//      `[NUMBER_OF_LAYERS] → TILE_BINNING_MODE_CFG → FLUSH_VCD_CACHE → OCCLUSION_QUERY_COUNTER →
//      START_TILE_BINNING` — our prologue verbatim. V3D-52's auto-init audit also re-confirms against the
//      actual XML: `v3d_packet.xml` code 120 `max_ver="42"` carries exactly eight fields and no
//      auto-initialise-tile-state bit (that field is `min_ver="71"`-side restructuring, absent on 4.2).
//
// The register protocol and the packet stream are now BOTH mainline-exact end to end. What has never
// been read is the CLE control-thread's own state.
//
// ── (c) RANK 1 — the CTnCS bit decode and the two never-sampled registers ─────────────────────────
//
// `CT0CS` has been printed as a raw hex word since PI-V3D-13 and decoded only for `CTRUN`. §32 refused
// to go further for want of a corroborated bit map. `vc4_regs.h` supplies one (see the constants block
// above), and it names a bit that would explain the entire paradox in one reading: **`CTERR` (bit 3)** —
// a control-thread error. A CLE that latched CTERR would walk to EA, drop CTRUN, and emit nothing: the
// exact signature. `CTSUBS` (bit 4) and `CTRTSD` (bit 8) say whether the thread believes it is inside a
// sub-list; `CTSEMA` (bit 12) exposes the semaphore the never-read `CT0SYNC`/`CT1SYNC` registers hold.
// `BXCF` (PTB binner extra config, 0x310) is defined by mainline, written by no mainline path, and has
// never been read here — a set `CLIPDISA` would be a bring-up fact, not a packet fact.
//
// RANK 2 — "frame open and STALLED" vs "frame open and IDLE" has never been distinguished, because
// `PCS` is sampled once at wait exit and never again. `[v3d59] frameclose` re-samples the wedged block
// for 64 ms: if `BMACTIVE` eventually clears, or `BMBUSY` ever toggles, or `BFC`/`BPCA` ever move, the
// binner is slow/blocked and the 500 ms wait is the wrong instrument; if all five registers are frozen
// solid, the frame is dead-open and the target is the frame-close latch alone.
//
// Both are pure reads and cost one boot. The two behavioural arms below are DISARMED by default: this
// arc collects the evidence that would justify them, exactly as §32 required of `CTRSTA`.

/// `[v3d59] ctstate` — decode `CT0CS`/`PCS` bit-by-bit at all five stations, plus the never-read
/// `CT0SYNC`/`CT1SYNC`/`BXCF`/`BPOA`/`BPOS`/`CT0LC`/`CT0PC`. Pure reads.
const V3D59_CTSTATE: bool = true;
/// `[v3d59] frameclose` — post-wedge time series of the bin-frame registers. Pure reads; ~64 ms.
///
/// **PI-V3D-60: BANKED — a DEEP probe, not a default one.** Its verdict is delivered and standing:
/// across the extended window not one of `PCS`, `CT0CS`, `BFC`, `BPCA`, `BPCS` changed by a single bit,
/// with `BMACTIVE` held set and `BMBUSY` clear — the bin frame is DEAD-OPEN, not slow and not
/// overflow-stalled. Re-running it every boot buys nothing and costs a visible stall in the boot square
/// (which the bench operator reads as a hang). Gated behind the budget-trim arc's `UNAOS_V3D_DEEP`
/// knob — arm deep only to re-measure that specific verdict. The V3D-60 `V3D60_*` constants stay
/// unconditionally true, being fast probes with no wait at all.
const V3D59_FRAMECLOSE: bool = V3D_DEEP;
/// `[v3d59] mainline` — the refutation ledger, so the metal log carries its own citations.
const V3D59_LEDGER: bool = true;
/// **DISARMED.** Arm a PTB overflow block (`BPOA`/`BPOS`) before the GO. Mainline provably never does
/// this (T1 above), so arming it runs a rung the kernel does not run — kept as a one-flip metal
/// demonstration of the citation, never as a default.
const V3D59_ARM_OVERFLOW: bool = false;
/// **DISARMED.** Issue a CT0 control-thread reset (`CTRSTA`, bit 15, corroborated by `vc4_regs.h`)
/// before programming the bin job. Justified only if `[v3d59] ctstate` reads `CTERR` set, or `BMACTIVE`
/// set at S0 — neither of which P57 saw. Collect first, write second.
const V3D59_ARM_CT0_RESET: bool = false;

/// Number of post-wedge samples and the spacing between them for `[v3d59] frameclose`.
const V3D59_FRAMECLOSE_SAMPLES: u32 = 64;
const V3D59_FRAMECLOSE_STEP_MS: u64 = 1;

/// Render the INFERRED `CTnCS` decode. `CTRUN` is the only corroborated bit; the rest are the hedged
/// VC4-family borrow documented at the bit-map block. `CTSEMA`/`CTRTSD` come back as field VALUES from
/// their raw windows, never as booleans — the two published sources disagree on their widths.
fn v3d59_ctncs_flags(cs: u32) -> (u32, u32, u32, u32, u32, u32) {
    (
        (cs & V3D_CLE_CTNCS_CTRUN != 0) as u32,
        (cs & V3D_CLE_CTNCS_CTERR != 0) as u32,
        (cs & V3D_CLE_CTNCS_CTSUBS != 0) as u32,
        (cs & V3D_CLE_CTNCS_CTRTSD_WIN) >> V3D_CLE_CTNCS_CTRTSD_SHIFT,
        (cs & V3D_CLE_CTNCS_CTSEMA_WIN) >> V3D_CLE_CTNCS_CTSEMA_SHIFT,
        (cs & V3D_CLE_CTNCS_CTMODE != 0) as u32,
    )
}

/// Emit the standing mainline-refutation ledger. One line, so the metal capture carries the citations
/// that closed T1..T4 rather than making the next reader re-derive them.
fn v3d59_mainline_ledger() {
    if !V3D59_LEDGER {
        return;
    }
    serial_println!(
        ":: V3D: [v3d59] mainline — audited against Linux v3d_sched.c/v3d_regs.h/vc4_regs.h (GPL-2.0-only: FACTS ONLY, no comment text reproduced) + Mesa v3dx_draw.c/v3dx_job.c/v3d_job.c/v3d_packet.xml. CLOSED: (T1) BPOS=0 is v3d_bin_job_run's FIRST write on EVERY bin job, done so no previous job's overflow block carries in; BPOA/BPOS are written ONLY by v3d_overflow_mem_work in response to OUTOMEM — mainline NEVER pre-arms an overflow pool, so BPOS=0 cannot be what stops the PTB. (T2) QMA/QMS then QTS|ENABLE then QBA then QEA, per-frame, our exact order — the S1 BPCS drop is the CT0QMS latch reloading the remaining-size register: benign AS A DIVERGENCE (it is not a divergence at all), which says nothing about whether the PTB is healthy — it only removes the write ORDER from the suspect list. (T3) qts = tile_state BO ADDRESS (v3d_job.c), not a count. (T4) v3dX(bcl_epilogue) emits FLUSH alone (no INCREMENT_SEMAPHORE, no FLUSH_ALL_STATE) and v3dX(start_binning) is NUMBER_OF_LAYERS/TILE_BINNING_MODE_CFG/FLUSH_VCD_CACHE/OCCLUSION_QUERY_COUNTER/START_TILE_BINNING — our CL verbatim; v3d_packet.xml code 120 max_ver=42 has NO auto-init-tile-state field. Register protocol and packet stream are both mainline-exact; the unread surface is the CLE control-thread state — see [v3d59] ctstate ::"
    );
}

/// `[v3d59] ctstate` — the CTnCS/PCS bit decode across the five V3D-58 stations, plus the registers no
/// boot has ever sampled. Reads only; every constant is cited in the block above.
fn v3d59_emit_ctstate(what: &str, s: &[V3d58Station; 5]) {
    if !V3D59_CTSTATE {
        return;
    }
    let names = ["S0 pre-program", "S1 post-QMA/QMS/QTS", "S2 post-QBA", "S3 post-GO", "S4 wait-exit"];
    for (i, st) in s.iter().enumerate() {
        let (run, err, subs, rtsd, sema, mode) = v3d59_ctncs_flags(st.ct0cs);
        serial_println!(
            "::   [v3d59] {} — CT0CS={:#010x} (CTRUN={} | INFERRED: CTERR={} CTSUBS={} CTRTSD[9:8]={} CTSEMA[14:12]={} CTMODE={}) CT0SYNC={:#010x} CT1SYNC={:#010x} CT0LC={:#x} CT0PC={:#x} | BPOA={:#010x} BPOS={:#010x} BXCF={:#010x} (CLIPDISA={} RWORDERDISA={}) ::",
            names[i], st.ct0cs, run, err, subs, rtsd, sema, mode,
            st.ct0sync, st.ct1sync, st.ct0lc, st.ct0pc,
            st.bpoa, st.bpos, st.bxcf,
            (st.bxcf & V3D_PTB_BXCF_CLIPDISA != 0) as u32,
            (st.bxcf & V3D_PTB_BXCF_RWORDERDISA != 0) as u32,
        );
    }
    let err_any = s.iter().any(|st| st.ct0cs & V3D_CLE_CTNCS_CTERR != 0);
    let err_at = s.iter().position(|st| st.ct0cs & V3D_CLE_CTNCS_CTERR != 0);
    let err_at_s0 = s[0].ct0cs & V3D_CLE_CTNCS_CTERR != 0;
    let subs4 = s[4].ct0cs & V3D_CLE_CTNCS_CTSUBS != 0;
    let sema_moved = s[0].ct0sync != s[4].ct0sync || s[0].ct1sync != s[4].ct1sync;
    let sync0_nonzero = s[0].ct0sync != 0 || s[0].ct1sync != 0;
    let bxcf_set = s[0].bxcf != 0;
    let lc_moved = s[4].ct0lc != s[0].ct0lc;
    let pc_moved = s[4].ct0pc != s[0].ct0pc;
    // The BORROWED-MAP FALSIFIER, checked before any verdict that leans on the map. The probe is the
    // first CT0 kick after a fresh reset cycle, and M3's CT1 render frame retires cleanly on this block
    // (see [v3d58] xengine). A control thread cannot be errored-from-birth AND healthy enough to retire
    // a render frame — so bit 3 reading SET at S0 in that situation indicts the VC4-era map itself.
    let map_indicted = err_at_s0 && V3D58_RENDER_OK.load(Ordering::Acquire);
    let verdict = if map_indicted {
        "THE BORROWED BIT MAP IS INDICTED, NOT THE HARDWARE. Bit 3 reads SET at S0 — before this driver touched a single CT0 register, on a block that had just come through a fresh reset cycle and whose CT1 render frame retired cleanly this boot. A control thread cannot be errored-from-birth and simultaneously healthy enough to complete a render frame. So bit 3 is NOT CTERR on V3D 4.x, the VC4-era CTnCS map carried across on offset identity is WRONG for this block, and every INFERRED column on the station lines above must be discarded (this file's line ~299 caution was right). Do NOT arm V3D59_ARM_CT0_RESET on this reading — CTRSTA's position is from the same discredited map"
    } else if err_any {
        "bit 3 (INFERRED CTERR) IS SET. On the borrowed VC4-family map that means the CT0 control thread latched an ERROR — the CLE did not merely fail to produce work, it FAULTED — and that single bit would account for the whole paradox (list walked to EA, CTRUN dropped, no PTB write, no FLDONE, frame left open). HEDGE: the map is an inference carried on offset identity and this file's line ~299 records that the 4.x layout diverges from VideoCore IV somewhere past CTRUN, so this is a LEAD, not a verdict. It did NOT read set at S0, so the falsifier above does not fire. The station index says when it latched; next arc: corroborate bit 3 independently, then consider CTRSTA (V3D59_ARM_CT0_RESET) and hunt what the CLE rejected at that station"
    } else if subs4 {
        "bit 4 (INFERRED CTSUBS) is SET at wait exit — on the borrowed map, the CT0 thread believes it is still inside a SUB-LIST at the end of a list that walked to EA, and a thread parked in a sub-list never reaches the top-level FLUSH's completion semantics: exactly a frame that opens and never closes. Read the CTRTSD[9:8] window on the station lines for a candidate nesting depth — but note the two published sources disagree on that field's WIDTH, so treat the number as raw bits, not an established depth. Audit every BRANCH/RETURN in the bin CL"
    } else if sync0_nonzero || sema_moved {
        "the CLE SEMAPHORE registers are NOT at rest. Mesa emits no INCREMENT_SEMAPHORE/WAIT_ON_SEMAPHORE on the modern V3D path (see [v3d59] mainline), so CT0SYNC/CT1SYNC should be reset-valued and static across our frames. HEDGE — TWO reasons this row is weaker than it looks: (1) the READ SIDE EFFECTS of CT0SYNC/CT1SYNC are unverified, and a semaphore register that decrements or clears on read would be MOVED BY THIS PROBE ITSELF (five stations = five reads per register), manufacturing the sema_moved=1 it reports; (2) their reset values are unknown, so 'non-zero at S0' is not by itself abnormal. Confirm with a single-read boot before concluding the block carries CLE rendezvous state into the bin"
    } else if bxcf_set {
        "the PTB BXCF (binner extra config) is NON-ZERO on a block we reset ourselves, and mainline writes it from no path. Whatever set it (reset default, firmware, or the preceding CT1 frame) is configuring the PTB behind us — read the CLIPDISA/RWORDERDISA columns above and treat this as a bring-up fact"
    } else if !lc_moved && !pc_moved {
        "clean CT0CS at every station, semaphores at rest, BXCF zero — and CT0LC/CT0PC BOTH unmoved across the whole kick: the CLE walked BA->EA without the list-item or primitive counters registering a single item. A CLE that consumed the address range but counted nothing is not executing the list it fetched; the next surface is the CLE's fetch/decode path (address-space aliasing of the CL itself), not the PTB"
    } else {
        "no inferred-CTERR, no sub-list parking, semaphores at rest, BXCF zero, and CT0LC/CT0PC DID move — the control thread executed the list cleanly and by its own accounting fed items to the PTB, yet the PTB wrote nothing and the frame never closed. Every CLE-side explanation this decode can reach is excluded — with the standing caveat that the decode itself is a hedged VC4-family borrow, so 'clean CT0CS' means 'clean under a map we have not independently confirmed for 4.x'. On that reading the wall is inside the PTB, between item-accept and pool-write. Read [v3d59] frameclose for whether it is stalled or dead — DEEP-only, so it does NOT appear on this boot unless the log carries a [v3d] deep=on line; re-run with UNAOS_V3D_DEEP=1"
    };
    serial_println!(
        ":: V3D: [v3d59] ctstate ({}) — [decode past CTRUN is an INFERRED VC4-family map, see the PI-V3D-59 bit-map block; map-indicted={}] bit3/CTERR seen={} (first station={}, at-S0={}) | CTSUBS@S4={} CTRTSD[9:8]@S4={} CTSEMA[14:12]@S4={} (widths DISPUTED between vc4_regs.h and the ARG — raw windows, not booleans) | CT0SYNC {:#010x}->{:#010x} CT1SYNC {:#010x}->{:#010x} (at-rest-at-S0={}; read side effects UNVERIFIED — 5 reads/register) | BXCF S0={:#010x} S4={:#010x} | CT0LC {:#x}->{:#x} (moved={}) CT0PC {:#x}->{:#x} (moved={}) — {} ::",
        what,
        map_indicted as u32,
        err_any as u32,
        match err_at { Some(i) => i as i32, None => -1 },
        err_at_s0 as u32,
        subs4 as u32,
        (s[4].ct0cs & V3D_CLE_CTNCS_CTRTSD_WIN) >> V3D_CLE_CTNCS_CTRTSD_SHIFT,
        (s[4].ct0cs & V3D_CLE_CTNCS_CTSEMA_WIN) >> V3D_CLE_CTNCS_CTSEMA_SHIFT,
        s[0].ct0sync, s[4].ct0sync, s[0].ct1sync, s[4].ct1sync,
        (!sync0_nonzero) as u32,
        s[0].bxcf, s[4].bxcf,
        s[0].ct0lc, s[4].ct0lc, lc_moved as u32,
        s[0].ct0pc, s[4].ct0pc, pc_moved as u32,
        verdict
    );
}

/// `[v3d59] frameclose` — keep watching the wedged block after the FLDONE wait gives up.
///
/// Every reading of "the frame never closes" has come from a SINGLE sample taken at wait exit. That
/// cannot distinguish a binner that is stalled forever from one that is merely slower than our 500 ms
/// backstop, nor a frozen block from one whose `BMBUSY` is toggling. This polls the five bin-frame
/// registers `V3D59_FRAMECLOSE_SAMPLES` times at `V3D59_FRAMECLOSE_STEP_MS` spacing and reports whether
/// ANY of them ever moves. Pure reads — it cannot perturb the state it is measuring.
fn v3d59_frameclose_poll(what: &str) {
    if !V3D59_FRAMECLOSE {
        return;
    }
    let pcs0 = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
    let cs0 = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let bfc0 = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    let bpca0 = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA);
    let bpcs0 = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS);
    let mut pcs_changes = 0u32;
    let mut cs_changes = 0u32;
    let mut bmactive_cleared = false;
    let mut bmbusy_seen = (pcs0 & V3D_PCS_BMBUSY) != 0;
    let mut bmoom_seen = (pcs0 & V3D_PCS_BMOOM) != 0;
    let mut bfc_moved = false;
    let mut bpca_moved = false;
    let mut bpcs_moved = false;
    let mut last_pcs = pcs0;
    let mut last_cs = cs0;
    for _ in 0..V3D59_FRAMECLOSE_SAMPLES {
        settle_ms(V3D59_FRAMECLOSE_STEP_MS);
        let pcs = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
        let cs = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
        if pcs != last_pcs {
            pcs_changes += 1;
            last_pcs = pcs;
        }
        if cs != last_cs {
            cs_changes += 1;
            last_cs = cs;
        }
        if pcs & V3D_PCS_BMACTIVE == 0 {
            bmactive_cleared = true;
        }
        if pcs & V3D_PCS_BMBUSY != 0 {
            bmbusy_seen = true;
        }
        if pcs & V3D_PCS_BMOOM != 0 {
            bmoom_seen = true;
        }
        if mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC) != bfc0 {
            bfc_moved = true;
        }
        if mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA) != bpca0 {
            bpca_moved = true;
        }
        if mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS) != bpcs0 {
            bpcs_moved = true;
        }
    }
    let (pcs_last, cs_last) = (last_pcs, last_cs);
    let span_ms = V3D59_FRAMECLOSE_SAMPLES as u64 * V3D59_FRAMECLOSE_STEP_MS;
    let any_motion = pcs_changes > 0 || cs_changes > 0 || bfc_moved || bpca_moved || bpcs_moved;
    let verdict = if bmactive_cleared && bfc_moved {
        "the frame CLOSED after the FLDONE backstop gave up. The bin is not wedged, it is SLOW — BMACTIVE cleared and BFC advanced during this extra window. Every 'never retires' verdict in this file was measured with too short a wait; re-run the campaign with a longer backstop before reading any of them"
    } else if bmactive_cleared {
        "BMACTIVE CLEARED during the extra window but BFC did NOT advance — the bin frame tore down without ever counting as a completed frame. That is an ABORTED frame, not a hung one: the teardown path runs, the completion path does not. Target the BFC/FLDONE latch and look for what aborts the frame"
    } else if bmoom_seen {
        "BMOOM latched during the extra window — the PTB IS out of binning memory after all, just later than any single-shot sample could see. This reopens the overflow question the OUTOMEM reads at P43/P44 closed; flip V3D59_ARM_OVERFLOW and re-run"
    } else if any_motion {
        "the frame is still open but the block is NOT frozen — at least one bin-frame register moved during the extra window. The binner is making (or attempting) progress with the frame open; read the change counts above and extend the window before calling it a hang"
    } else if bmbusy_seen {
        "FROZEN WITH BMBUSY SET. Across the extra window not one of PCS, CT0CS, BFC, BPCA or BPCS changed by a single bit — but BMACTIVE and BMBUSY are BOTH held set. The block says a binning operation is IN PROGRESS and that operation has made no observable progress for the whole window: a hard STALL mid-op, not an idle open frame. Whatever the PTB is waiting on, it is not the CLE (CT0CS is static) and not overflow memory (BMOOM clear)"
    } else {
        "FROZEN. Across the extra window not one of PCS, CT0CS, BFC, BPCA or BPCS changed by a single bit, with BMACTIVE held set and BMBUSY clear at every sample. This is not a slow binner and not an overflow stall: the bin frame is DEAD-OPEN — opened by START_TILE_BINNING, never advanced, never closed, with nothing in flight. Combined with [v3d58] rerender (CT1 still renders afterwards) the target is the PTB frame unit alone, and the discriminator left is [v3d59] ctstate's inferred CTERR/CTSUBS decode"
    };
    serial_println!(
        ":: V3D: [v3d59] frameclose ({}) — extra window {}ms x{} samples AFTER the FLDONE backstop | PCS {:#010x}->{:#010x} changes={} (BMACTIVE-ever-cleared={} BMBUSY-ever-set={} BMOOM-ever-set={}) | CT0CS {:#010x}->{:#010x} changes={} | BFC moved={} BPCA moved={} BPCS moved={} — {} ::",
        what, span_ms, V3D59_FRAMECLOSE_SAMPLES,
        pcs0, pcs_last, pcs_changes,
        bmactive_cleared as u32, bmbusy_seen as u32, bmoom_seen as u32,
        cs0, cs_last, cs_changes,
        bfc_moved as u32, bpca_moved as u32, bpcs_moved as u32,
        verdict
    );
}

/// Optional arm: hand the PTB an overflow block before the GO. **Off by default** — mainline provably
/// never pre-arms one (T1), so this is a deliberate divergence kept as a one-flip metal demonstration.
/// Emits its line either way, so a capture always states which rung was run.
fn v3d59_arm_overflow(what: &str) {
    if !V3D59_CTSTATE {
        return;
    }
    if V3D59_ARM_OVERFLOW {
        let ovf = (arena_phys() + OFF_PROBE_BIN_OVERFLOW) as u32;
        mmio_write(V3D_CORE0_BASE, V3D_PTB_BPOA, ovf);
        mmio_write(V3D_CORE0_BASE, V3D_PTB_BPOS, PROBE_BIN_OVERFLOW_BYTES as u32);
        dsb();
        serial_println!(
            ":: V3D: [v3d59] arm-overflow ({}) — ARMED (DIVERGENT from mainline by design): BPOA={:#010x} BPOS={:#x} written after the CT0QMA/QMS/QTS latch and before CT0QBA. Mainline writes BPOA/BPOS ONLY from v3d_overflow_mem_work on OUTOMEM; if the bin retires with this armed and not without, the PTB needs an overflow block up front on this silicon and v3d_bin_job_run's BPOS=0 is insufficient for us — readback BPOA={:#010x} BPOS={:#010x} ::",
            what, ovf, PROBE_BIN_OVERFLOW_BYTES,
            mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOA),
            mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOS),
        );
    } else {
        serial_println!(
            ":: V3D: [v3d59] arm-overflow ({}) — DISARMED (kernel-exact): the frame runs with BPOS=0, exactly as v3d_bin_job_run leaves it on every mainline bin job. The brief's HIGH-ranked \"no overflow pool = no initial allocation\" theory is REFUTED by citation (see [v3d59] mainline T1); flip V3D59_ARM_OVERFLOW to refute it by demonstration instead ::",
            what
        );
    }
}

/// Optional arm: reset the CT0 control thread before programming the job. **Off by default.** The bit
/// is `CTRSTA` (15) from the VC4-era `vc4_regs.h` map, carried across on offset identity — a hedged
/// inference, not a corroboration (see the PI-V3D-59 bit-map block, and line ~299's standing caution
/// that the 4.x layout diverges past `CTRUN`). It is not JUSTIFIED until `[v3d59] ctstate` shows the
/// inferred `CTERR` set away from S0, or a frame open at S0 — and if bit 3 reads set AT S0 on a block
/// whose CT1 renders cleanly, the map is indicted and this write must not be issued at all.
fn v3d59_ct0_frame_reset(what: &str) {
    if !V3D59_CTSTATE {
        return;
    }
    let pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    if V3D59_ARM_CT0_RESET {
        mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CTNCS_CTRSTA);
        dsb();
        settle_ms(1);
        let post = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
        let pcs = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
        serial_println!(
            ":: V3D: [v3d59] ct0-reset ({}) — ARMED: wrote CTRSTA(bit15) to CT0CS before programming the job. CT0CS {:#010x}->{:#010x} (CTERR {}->{}), PCS={:#010x} (BMACTIVE={}). If the bin retires with this armed and not without, the wedge was inherited CLE thread state ::",
            what, pre, post,
            (pre & V3D_CLE_CTNCS_CTERR != 0) as u32,
            (post & V3D_CLE_CTNCS_CTERR != 0) as u32,
            pcs,
            (pcs & V3D_PCS_BMACTIVE != 0) as u32,
        );
    } else {
        serial_println!(
            ":: V3D: [v3d59] ct0-reset ({}) — DISARMED: CT0CS={:#010x} (INFERRED CTERR={} CTSUBS={}). CTRSTA(bit15) comes from the VC4-era vc4_regs.h map carried across on OFFSET IDENTITY — an inference of the same class as this driver's PCS/BPCA decodes, but weaker, because line ~299 records that the 4.x CTnCS layout diverges from VideoCore IV somewhere past CTRUN. So §32's objection is SOFTENED, not void: the write stays unjustified until [v3d59] ctstate reads bit3 set (and NOT at S0, which would indict the map instead) or a frame already open at S0. Evidence first ::",
            what, pre,
            (pre & V3D_CLE_CTNCS_CTERR != 0) as u32,
            (pre & V3D_CLE_CTNCS_CTSUBS != 0) as u32,
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-60 — the boot-state / warm-handoff discriminators for the PTB frame unit.
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// STANDING METAL FACTS this arc builds on (established, NOT re-litigated here):
//   * The bin control list is SOUND and EXECUTES — `CT0LC` 0x0→0x10000 and `CT0PC` 0x0→0x3 both MOVED,
//     so the control thread walked the list and by its own accounting fed items to the PTB.
//   * A CT1 RENDER frame on the SAME memory block is byte-verified, and a post-wedge re-render still
//     passes — the block's write path, clock, MMU and arena are all live. No global write blocker.
//   * The PTB frame unit is DEAD-OPEN: the frame opens at station S4, `BMACTIVE` sticks set, `BMBUSY`
//     never sets, `[v3d59] frameclose` saw ZERO bit changes across its extended window, `BPCA` advances
//     with no traffic anywhere the V3D MMU grants, `BFC` stays 0, no `CTERR`, no sub-list, semaphores
//     at rest. The wall is item-accept-WITHOUT-pool-write, inside the PTB frame open/close unit alone.
//   * V3D-56's "0x3000 reservation" reading is RETRACTED (a register-latch artifact).
//
// Every CL-side and per-job-register explanation is therefore closed. What has NEVER been examined is
// the state the block is in BEFORE the first bin job — the boot/handoff surface:
//
// ── (a) The WARM-HANDOFF hypothesis ──────────────────────────────────────────────────────────────
// UnaOS is a COLD-BOOT bare-metal driver: `bringup` powers the domain, sets the clock, and then
// POWER-CYCLES the V3D (`v3d_reset_cycle`, the OFF→ON GRAFX_V3D cycle) before reading a single
// register. Linux does the same reset — but it attaches to a block the VideoCore firmware has already
// been driving, and the firmware's own graphics stack has run frames through this PTB. If any part of
// the PTB frame unit is established by a FIRST frame rather than by register programming, a driver that
// only ever cold-starts would never get it, and no amount of per-job byte-exactness would help.
//
// The hypothesis is testable in one boot, read-only: sample the whole bin-frame register set BEFORE our
// reset cycle (the first V3D read of the boot) and again after it. `BFC`/`RFC` non-zero at that first
// read means the firmware HAS driven frames through this block. Everything at reset value means the
// block arrives virgin, the firmware never opened a PTB frame, and there is nothing warm to inherit —
// which KILLS the hypothesis rather than leaving it hanging. The second half of the pair also answers a
// question no boot has asked: does our OFF→ON cycle actually CHANGE the PTB registers, or is the
// "reset" a no-op that never reaches the frame unit?
//
// ── (b) The IDENT / boot-state checks ────────────────────────────────────────────────────────────
// The kernel driver reads the hub and core identity registers at probe and derives its VERSION from
// them; every version-conditional path (and, on our side, every v42 packet encoding in this file) hangs
// off that number. This file has printed the raw IDENT words since PI-V3D-1 and decoded exactly two
// fields. `[v3d60] ident` restates the decode as an explicit CHECK — is the technology version the 4.2
// this driver's whole CL packing targets, is core 0 the core we drive — because a version mismatch
// would silently invalidate the campaign's foundation. PI-V3D-61: the CHECK itself was miscoded (top
// byte vs low nibbles, hex 0x42 vs decimal 42) and P59's mismatch reading is retracted; the version now
// comes from `v3d_ident_version` and is corroborated by the core signature. Fields beyond those audited are
// printed RAW: no fabricated bit names (the standing rule; this driver has been convicted three times).
//
// ── (c) The INIT-DELTA ledger ────────────────────────────────────────────────────────────────────
// `[v3d60] initdelta` walks, row by row, the registers the mainline kernel driver programs before its
// first bin job and prints OUR value beside the expectation. All facts, in our own words — no GPL
// comment text is reproduced. Two rows were GENUINE GAPS this audit found; PI-V3D-62 closed both, and
// they are now ordinary readback checks:
//
//   (1) `MMU_ILLEGAL_ADDR`. Mainline allocates a DEDICATED scratch page for this — memory that belongs
//       to no job and that nothing else maps — and points the illegal-address catcher at it. UnaOS
//       pointed it at ARENA PAGE 0, which is inside the very address space the PTB writes and is mapped
//       VALID+WRITEABLE in our page table, making "an illegal access" indistinguishable from "a legal
//       access" at the memory it lands on. V3D-62 added `V3D_MMU_SCRATCH`, an unmapped page outside the
//       arena, and aims the catcher there — which also turns the page into a witness (see below).
//
//   (2) `MMU_CTL` fault policy. Mainline enables BOTH the abort and the INTERRUPT response for the
//       page-table-invalid, write-violation AND address-cap conditions; UnaOS wrote the ABORT halves of
//       the first two only. V3D-60 declined to guess the interrupt-companion bit positions rather than
//       invent them (this driver has been convicted three times for fabricated constants); V3D-62 read
//       them off the same audited register source as the rest of this file and `program_mmu` now writes
//       mainline's full set. A silently-swallowed PTB write is exactly the class of failure a
//       fault-reporting policy exists to surface.
//
// The instrument that reads those reports is `[v3d62] fault` — three independent channels across the bin
// frame (MMU fault latches + VIO address/ID, the hub interrupt vector, and the scratch page's sentinel).
// See the PI-V3D-62 section further down for what each channel proves.
//
// ── (d) The two cheap discriminators [v3d59] left owed ───────────────────────────────────────────
// `[v3d60] syncrd` — `[v3d59] ctstate` hedged its semaphore row because it read `CT0SYNC`/`CT1SYNC`
// five times per boot and a register with read side effects would be moved BY THE PROBE. A pair of
// BACK-TO-BACK reads at a quiescent station settles it: if read #2 differs from read #1 with nothing
// happening in between, the registers self-modify on read and `[v3d59]`'s `sema_moved` is an artifact.
//
// `[v3d60] gmpdelta` — the GMP (memory-protection) block and the MMU fault latches have been read once,
// post-probe. A silent drop is a DELTA question: sample both PRE-kick and at wait-exit and report what
// latched DURING the frame. A GMP violation or an MMU cap/PT-invalid latch across the frame would be
// the silent-drop mechanism the campaign has been hunting; clean across the frame leaves the PTB frame
// unit standing alone as the wall.
//
// Everything in this section is READ-ONLY. No register is written, so no write needs justifying, and
// `CTRSTA` stays disarmed. QEMU raspi4b models no V3D: the pre-reset probe reads the hub identity word
// first and returns before touching a core register unless that word is live, which is the same
// poison-honest gate `probe_hub_ident0` uses (QEMU reads 0x00000000 there and every V3D-60 line is
// skipped). Budgets: none of these probes waits on anything — no deadline, no polling window.

/// `[v3d60] residue` — the pre-reset / post-reset bin-frame snapshot pair (the warm-handoff test).
const V3D60_RESIDUE: bool = true;
/// `[v3d60] ident` — hub/core identity as an explicit boot-state CHECK, not a raw dump.
const V3D60_IDENT: bool = true;
/// `[v3d60] initdelta` — the ours-vs-mainline init ledger, emitted pre-kick.
const V3D60_INITDELTA: bool = true;
/// `[v3d60] syncrd` — the back-to-back CTnSYNC read-side-effect test.
const V3D60_SYNCRD: bool = true;
/// `[v3d60] gmpdelta` — GMP + MMU fault-latch delta ACROSS the bin frame.
const V3D60_GMPDELTA: bool = true;

/// The bin-frame + boot-state register set, sampled as one snapshot. Pure reads.
#[derive(Clone, Copy)]
struct V3d60Snap {
    hub_ident0: u32,
    hub_ident1: u32,
    mmu_ctl: u32,
    mmu_pt_base: u32,
    pcs: u32,
    ct0cs: u32,
    ct1cs: u32,
    bfc: u32,
    rfc: u32,
    bpca: u32,
    bpcs: u32,
    bpoa: u32,
    bpos: u32,
    bxcf: u32,
    ct0qma: u32,
    ct0qms: u32,
    ct0qts: u32,
    l2tflsta: u32,
    l2tflend: u32,
    misccfg: u32,
    gmp_cfg: u32,
    gmp_status: u32,
    int_msk_sts: u32,
    hub_int_msk_sts: u32,
}

/// The pre-reset snapshot, carried from `bringup`'s pre-reset station to the post-reset station.
/// Single-threaded BSP bring-up; `static mut` matches this module's existing idiom (`V3D_REPLAY_FB`).
static mut V3D60_PRE: Option<V3d60Snap> = None;

/// Take the full snapshot. CORE-relative reads — the caller MUST have established that the hub
/// identity word is live (an absent block aborts on a core read).
fn v3d60_snap() -> V3d60Snap {
    V3d60Snap {
        hub_ident0: mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT0),
        hub_ident1: mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT1),
        mmu_ctl: mmio_read(V3D_HUB_BASE, V3D_MMU_CTL),
        mmu_pt_base: mmio_read(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE),
        pcs: mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS),
        ct0cs: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS),
        ct1cs: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS),
        bfc: mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC),
        rfc: mmio_read(V3D_CORE0_BASE, V3D_CLE_RFC),
        bpca: mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA),
        bpcs: mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS),
        bpoa: mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOA),
        bpos: mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOS),
        bxcf: mmio_read(V3D_CORE0_BASE, V3D_PTB_BXCF),
        ct0qma: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMA),
        ct0qms: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMS),
        ct0qts: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QTS),
        l2tflsta: mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TFLSTA),
        l2tflend: mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TFLEND),
        misccfg: mmio_read(V3D_CORE0_BASE, V3D_CTL_MISCCFG),
        gmp_cfg: mmio_read(V3D_CORE0_BASE, V3D_GMP_CFG),
        gmp_status: mmio_read(V3D_CORE0_BASE, V3D_GMP_STATUS),
        int_msk_sts: mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_MSK_STS),
        hub_int_msk_sts: mmio_read(V3D_HUB_BASE, V3D_HUB_INT_MSK_STS),
    }
}

/// Print one snapshot as two dense lines (hub/boot state, then the bin-frame set).
fn v3d60_emit_snap(station: &str, s: &V3d60Snap) {
    serial_println!(
        "::   [v3d60] {} boot-state — HUB_IDENT0={:#010x} HUB_IDENT1={:#010x} | MMU_CTL={:#010x} (ENABLE={}) MMU_PT_PA_BASE={:#010x} | L2TFLSTA={:#010x} L2TFLEND={:#010x} MISCCFG={:#010x} | GMP_CFG={:#010x} GMP_STATUS={:#010x} | INT_MSK_STS={:#010x} (FLDONE {}) HUB_INT_MSK_STS={:#010x} ::",
        station, s.hub_ident0, s.hub_ident1,
        s.mmu_ctl, (s.mmu_ctl & V3D_MMU_CTL_ENABLE != 0) as u32, s.mmu_pt_base,
        s.l2tflsta, s.l2tflend, s.misccfg,
        s.gmp_cfg, s.gmp_status,
        s.int_msk_sts,
        if s.int_msk_sts & V3D_INT_FLDONE != 0 { "MASKED" } else { "unmasked" },
        s.hub_int_msk_sts,
    );
    serial_println!(
        "::   [v3d60] {} bin-frame — PCS={:#010x} (BMACTIVE={} BMBUSY={} BMOOM={}) CT0CS={:#010x} CT1CS={:#010x} | BFC={:#010x} RFC={:#010x} | BPCA={:#010x} BPCS={:#010x} BPOA={:#010x} BPOS={:#010x} BXCF={:#010x} | CT0QMA={:#010x} CT0QMS={:#010x} CT0QTS={:#010x} ::",
        station, s.pcs,
        (s.pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (s.pcs & V3D_PCS_BMBUSY != 0) as u32,
        (s.pcs & V3D_PCS_BMOOM != 0) as u32,
        s.ct0cs, s.ct1cs, s.bfc, s.rfc,
        s.bpca, s.bpcs, s.bpoa, s.bpos, s.bxcf,
        s.ct0qma, s.ct0qms, s.ct0qts,
    );
}

/// `[v3d60] residue (pre-reset)` — the FIRST V3D register read of the boot, taken after power/clock/gate
/// and BEFORE `v3d_reset_cycle`. This is the only window in which firmware state is still observable:
/// our own OFF→ON power cycle is the next thing that happens.
///
/// Poison-honest and QEMU-safe: one hub IDENT0 read decides (no retry loop, no budget). Zero = block
/// absent (QEMU raspi4b), poison = block not decoding — either way we return WITHOUT touching a core
/// register, exactly like the `probe_hub_ident0` gate.
fn v3d60_residue_pre() {
    if !V3D60_RESIDUE {
        return;
    }
    // Settle before the very first register read of the boot. The clock gate was opened microseconds
    // ago and a freshly gated block can take a moment to answer; `probe_hub_ident0` gets this settle
    // from `bringup` (a 2 ms wait plus a poison-retry window) and we sit BEFORE both. Without it a
    // transient identity word could either send us into core reads on a block that is not answering
    // yet, or — worse — hand the verdict logic garbage that reads as "the firmware ran frames". One
    // bounded 2 ms wait off CNTPCT, matching bringup's own post-reset settle; no retry loop, because a
    // single honest read is the whole point of this station.
    settle_ms(2);
    let id0 = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT0);
    if id0 == 0 || is_poison(id0) {
        serial_println!(
            ":: V3D: [v3d60] residue (pre-reset) — SKIPPED: hub IDENT0 reads {:#010x} ({}) before our reset cycle, so no core register may be read here. The warm-handoff question is UNANSWERED this boot (expected on QEMU raspi4b, which models no V3D) ::",
            id0,
            if id0 == 0 { "block absent/unpowered" } else { "open-bus/firmware poison" }
        );
        return;
    }
    let s = v3d60_snap();
    serial_println!(
        ":: V3D: [v3d60] residue (pre-reset) — the FIRST V3D read of the boot, taken after power/clock/gate and BEFORE the OFF->ON reset cycle. This is the only window where VideoCore-firmware state is still observable ::"
    );
    v3d60_emit_snap("pre-reset", &s);
    let fw_ran_frames = s.bfc != 0 || s.rfc != 0;
    let fw_mmu = s.mmu_ctl & V3D_MMU_CTL_ENABLE != 0 || s.mmu_pt_base != 0;
    let fw_frame_open = s.pcs & V3D_PCS_BMACTIVE != 0;
    let fw_queue = s.ct0qma != 0 || s.ct0qms != 0 || s.ct0qts != 0 || s.bpca != 0 || s.bpcs != 0;
    let verdict = if fw_frame_open {
        "A BIN FRAME IS ALREADY OPEN at cold boot, before this driver has touched anything. The firmware left the PTB mid-frame and our reset cycle is the only thing that could close it — check the post-reset line below for whether it did. If BMACTIVE survives our reset, every START_TILE_BINNING we have ever issued has been stacking onto a frame that was open before we arrived"
    } else if fw_ran_frames {
        "THE FIRMWARE HAS DRIVEN FRAMES THROUGH THIS BLOCK — a frame counter is non-zero before we touch it. The warm-handoff hypothesis is LIVE: whatever a first firmware frame establishes in the PTB, we then destroy with our OFF->ON power cycle and never re-establish. Compare the post-reset line: any field the reset returns to zero is state a warm-attached driver would have kept"
    } else if fw_mmu || fw_queue {
        "no frame counted, but the block is NOT virgin — the firmware left MMU and/or CT0-queue state established. Partial handoff: the firmware configured the block without completing a bin frame, so the warm-handoff hypothesis narrows to configuration rather than to a first frame"
    } else {
        "THE BLOCK IS VIRGIN AT OUR ENTRY — no frame counted, no MMU established, no CT0 queue state, no open frame. The VideoCore firmware never drove a bin frame through this PTB, so there is NOTHING WARM TO INHERIT and the firmware-warm-handoff hypothesis is DEAD as an explanation for the dead-open frame. The PTB must be startable from cold by register programming alone, and the wall stays where [v3d59] left it"
    };
    serial_println!(
        ":: V3D: [v3d60] residue (pre-reset) verdict — firmware-ran-frames={} (BFC={:#x} RFC={:#x}) firmware-MMU-established={} firmware-CT0-queue-state={} frame-already-open={} — {} ::",
        fw_ran_frames as u32, s.bfc, s.rfc, fw_mmu as u32, fw_queue as u32, fw_frame_open as u32, verdict
    );
    unsafe {
        V3D60_PRE = Some(s);
    }
}

/// `[v3d60] residue (post-reset)` — the same registers after the OFF→ON cycle, diffed field by field
/// against the pre-reset snapshot. Answers a question no boot has asked: does our reset cycle actually
/// REACH the PTB frame unit, or does it leave those registers exactly as it found them?
fn v3d60_residue_post() {
    if !V3D60_RESIDUE {
        return;
    }
    let pre = match unsafe { V3D60_PRE } {
        Some(p) => p,
        None => return, // pre-reset station never ran (block absent/poison) — nothing to diff.
    };
    let s = v3d60_snap();
    v3d60_emit_snap("post-reset", &s);
    // Count how many of the bin-frame + boot-state fields our reset cycle actually moved.
    let frame_fields: [(&str, u32, u32); 10] = [
        ("PCS", pre.pcs, s.pcs),
        ("CT0CS", pre.ct0cs, s.ct0cs),
        ("BFC", pre.bfc, s.bfc),
        ("RFC", pre.rfc, s.rfc),
        ("BPCA", pre.bpca, s.bpca),
        ("BPCS", pre.bpcs, s.bpcs),
        ("BPOA", pre.bpoa, s.bpoa),
        ("BPOS", pre.bpos, s.bpos),
        ("BXCF", pre.bxcf, s.bxcf),
        ("CT0QMA", pre.ct0qma, s.ct0qma),
    ];
    let mut moved = 0u32;
    for (_, a, b) in frame_fields.iter() {
        if a != b {
            moved += 1;
        }
    }
    let boot_moved = (pre.mmu_ctl != s.mmu_ctl) as u32
        + (pre.l2tflsta != s.l2tflsta) as u32
        + (pre.l2tflend != s.l2tflend) as u32
        + (pre.int_msk_sts != s.int_msk_sts) as u32;
    let frame_still_open = s.pcs & V3D_PCS_BMACTIVE != 0;
    let verdict = if frame_still_open {
        "BMACTIVE IS STILL SET AFTER OUR RESET CYCLE. The OFF->ON GRAFX_V3D power cycle does not close (or does not reach) an open PTB bin frame — the block enters every job of every boot with a frame already open. That is a bring-up-level defect and a direct candidate for the dead-open wall"
    } else if moved == 0 && boot_moved == 0 {
        "OUR RESET CYCLE CHANGED NOTHING. Not one bin-frame or boot-state register differs across the OFF->ON power cycle. Either the block was already at these values (consistent with a virgin pre-reset reading) or the cycle never reached the core at all — cross-read the [v3d50] ASB/PM lines: if the bridges ACKed, the reset ran and the registers were simply already clean; if they did not, this driver has never actually reset the V3D"
    } else {
        "the reset cycle DID move block state — the fields that changed are the ones the OFF->ON cycle returns to reset value. Any field that was non-zero pre-reset and zero after is precisely the state a warm-attached driver (firmware first, kernel second) would have kept and we discard"
    };
    serial_println!(
        ":: V3D: [v3d60] residue (post-reset) verdict — bin-frame fields moved by the reset={}/10 boot-state fields moved={}/4 | BMACTIVE pre={} post={} | BFC {:#x}->{:#x} RFC {:#x}->{:#x} BPCA {:#x}->{:#x} — {} ::",
        moved, boot_moved,
        (pre.pcs & V3D_PCS_BMACTIVE != 0) as u32, frame_still_open as u32,
        pre.bfc, s.bfc, pre.rfc, s.rfc, pre.bpca, s.bpca,
        verdict
    );
}

/// `[v3d60] ident` — the hub/core identity read as a CHECK rather than a dump: is this the 4.2 silicon
/// every packet encoding in this file targets, and is core 0 the core we drive? Fields beyond the two
/// this file has audited are reported RAW — the no-fabricated-bit-names rule applies here as everywhere
/// in this module. V3D-61 corrected the version decode itself: the number lives in `HUB_IDENT1`'s low
/// nibbles as `TVER*10 + REV` (decimal 42 for V3D 4.2), not in the register's top byte, and it is
/// cross-checked against the core's `'V3D'` ASCII signature and its own version nibbles.
fn v3d60_ident(hub1: u32, hub2: u32, hub3: u32, c0: u32, c1: u32, c2: u32) {
    if !V3D60_IDENT {
        return;
    }
    let (ver, tver, rev, ncores, sig_ok, core_ver_ok) = v3d_ident_version(hub1, c0, c1);
    let nhosts = (hub1 >> V3D_HUB_IDENT1_NHOSTS_SHIFT) & V3D_HUB_IDENT1_NIB_MASK;
    // This driver's whole CL packing is the v42 (V3D 4.2) variant set — see the packet-facts block and
    // every `v3d_packet.xml max_ver=42` citation. The kernel driver derives the SAME number from this
    // register — `tver * 10 + rev`, decimal — and gates its version-conditional paths on it.
    let ver_ok = ver == V3D_VERSION_EXPECTED;
    serial_println!(
        ":: V3D: [v3d60] ident — HUB_IDENT1={:#010x} -> version {}.{} (ver={}, ours-expects {} = V3D 4.2, the variant EVERY packet encoding in this file targets) cores={} (we drive core 0) hosts={} | feats L3C={} TFU={} TSY={} MSO={} | HUB_IDENT2={:#010x} (MMU-present={}) HUB_IDENT3={:#010x} | CORE0 IDENT0={:#010x} ('V3D' signature {}, core-major={}) IDENT1={:#010x} IDENT2={:#010x} (rest raw — this file decodes only corroborated fields and invents none) | corroboration: signature={} core-version-agrees-with-hub={} — {} ::",
        hub1, tver, rev, ver, V3D_VERSION_EXPECTED, ncores, nhosts,
        (hub1 & V3D_HUB_IDENT1_WITH_L3C != 0) as u32,
        (hub1 & V3D_HUB_IDENT1_WITH_TFU != 0) as u32,
        (hub1 & V3D_HUB_IDENT1_WITH_TSY != 0) as u32,
        (hub1 & V3D_HUB_IDENT1_WITH_MSO != 0) as u32,
        hub2, (hub2 & V3D_HUB_IDENT2_WITH_MMU != 0) as u32, hub3,
        c0, if sig_ok { "OK" } else { "ABSENT" }, (c0 >> V3D_CTL_IDENT0_VER_SHIFT) & 0xFF,
        c1, c2,
        sig_ok as u32, core_ver_ok as u32,
        if ver_ok && sig_ok && core_ver_ok {
            "version CONFIRMED on three independent witnesses (hub TVER/REV, core IDENT0 'V3D' signature + major byte, core IDENT1 revision nibble): the silicon is the generation the CL packing, the shader-word encoding and the register offsets were all audited against. The campaign's foundation holds. (V3D-61: the pre-V3D-61 probe read this version from HUB_IDENT1's TOP BYTE and compared it against the HEX 0x42; both were wrong, and P59's 'VERSION MISMATCH' was that decode's artifact — RETRACTED)"
        } else if ver_ok {
            if !sig_ok {
                "version field reads 4.2 but the CORE SIGNATURE witness disagrees — CORE0 IDENT0's low three bytes are not the ASCII 'V3D' mark, so the core register window may not be the block the hub describes. Treat the identity as UNSETTLED and resolve before trusting further PTB readings"
            } else {
                "version field reads 4.2 and the core signature is present, but the CORE VERSION witness disagrees — CORE0 IDENT0's major byte and/or CORE0 IDENT1's revision nibble do not match the hub's TVER/REV. Treat the identity as UNSETTLED and resolve before trusting further PTB readings"
            }
        } else {
            "VERSION MISMATCH — under the V3D-61-corrected field map (version = HUB_IDENT1 TVER*10 + REV, decimal) the block does NOT report 42. This driver's packet encoding, QPU word packing and register map were all audited against V3D 4.2; a genuine mismatch invalidates the campaign's foundation and MUST be resolved before any further PTB reading is trusted"
        }
    );
}

/// `[v3d60] initdelta` — the init ledger: for every register the mainline kernel driver programs before
/// its first bin job, print OUR value beside the expectation and mark whether they agree. Facts stated
/// in our own words; no kernel comment text is reproduced. Pure reads.
fn v3d60_initdelta(tag: &str) {
    if !V3D60_INITDELTA {
        return;
    }
    // `gaps` counts MEASURED divergences — rows whose verdict comes from a register readback and could
    // read either way on any given boot. `standing_gaps` counts rows that are a property of this build
    // and read the same every boot. The only such row was the missing MMU fault-INTERRUPT policy, which
    // PI-V3D-62 closed by writing mainline's full set; the counter stays (the ledger will grow again) but
    // reads zero, and both former GAP rows are now ordinary MEASURED readback checks. Keeping the two
    // counts apart is what makes `gaps=0` a reachable, meaningful verdict.
    let mut gaps = 0u32;
    let standing_gaps = 0u32;
    serial_println!(
        ":: V3D: [v3d60] initdelta ({}) — every register mainline programs before its FIRST bin job, ours beside the expectation. Read-only audit; facts restated in our own words ::",
        tag
    );

    // ── MMU page table base ──────────────────────────────────────────────────────────────────────
    let pt_base = mmio_read(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE);
    let pt_want = (pt_phys() >> V3D_MMU_PAGE_SHIFT) as u32;
    let pt_ok = pt_base == pt_want;
    gaps += !pt_ok as u32;
    serial_println!(
        "::   [v3d60] MMU_PT_PA_BASE ours={:#010x} want={:#010x} match={} — mainline programs the table base in PAGES before enabling the MMU; ours is the confined arena-only table ::",
        pt_base, pt_want, pt_ok as u32
    );

    // ── MMU control / fault policy — GAP (2) ─────────────────────────────────────────────────────
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    // PI-V3D-62 CLOSED this row: the expectation is now mainline's FULL set — the abort halves plus the
    // interrupt companions plus the address-cap pair — so a shortfall here is a MEASURED divergence
    // (the block did not accept what we wrote), not a standing property of the build.
    let ctl_want = V3D_MMU_CTL_ENABLE
        | V3D_MMU_CTL_PT_INVALID_ENABLE
        | V3D_MMU_CTL_PT_INVALID_ABORT
        | V3D_MMU_CTL_PT_INVALID_INT
        | V3D_MMU_CTL_WRITE_VIOLATION_ABORT
        | V3D_MMU_CTL_WRITE_VIOLATION_INT
        | V3D_MMU_CTL_CAP_EXCEEDED_ABORT
        | V3D_MMU_CTL_CAP_EXCEEDED_INT;
    let policy_ok = ctl & ctl_want == ctl_want;
    let latched = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    gaps += !policy_ok as u32;
    serial_println!(
        "::   [v3d60] MMU_CTL ours={:#010x} want={:#010x} full-fault-policy-present={} (abort+INTERRUPT for pt-invalid/write-violation/address-cap) fault-latched={:#010x} — V3D-60 named the missing interrupt halves a STANDING gap and declined to guess their bit positions; V3D-62 read them off the audited register source and `program_mmu` now writes mainline's full set. This row is CLOSED: it is a plain readback check from here on ::",
        ctl, ctl_want, policy_ok as u32, latched
    );

    // ── MMU illegal-address catcher — CLOSED by PI-V3D-62 ────────────────────────────────────────
    let illegal = mmio_read(V3D_HUB_BASE, V3D_MMU_ILLEGAL_ADDR);
    let illegal_page = (illegal & !V3D_MMU_ILLEGAL_ADDR_ENABLE) as usize;
    let arena_page0 = arena_phys() >> V3D_MMU_PAGE_SHIFT;
    let points_into_arena = illegal_page >= arena_page0 && illegal_page < arena_page0 + ARENA_PAGES;
    let scratch_pfn = mmu_scratch_phys() >> V3D_MMU_PAGE_SHIFT;
    let points_at_scratch = illegal_page == scratch_pfn;
    gaps += (points_into_arena || !points_at_scratch) as u32;
    serial_println!(
        "::   [v3d60] MMU_ILLEGAL_ADDR ours={:#010x} (page {:#x}, enable={}) points-at-dedicated-scratch={} (scratch pfn {:#x}) points-INTO-our-arena={} — V3D-62 CLOSED this row: the catcher now names a page that belongs to no job and that our page table does not map, matching mainline's dedicated-scratch allocation. An illegal access is distinguishable from a legal one at the memory it lands on, and `[v3d62] fault` reads that page as a witness ::",
        illegal, illegal_page, (illegal & V3D_MMU_ILLEGAL_ADDR_ENABLE != 0) as u32,
        points_at_scratch as u32, scratch_pfn, points_into_arena as u32
    );

    // ── L2T flush window (V3D-51 established this) ───────────────────────────────────────────────
    let sta = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TFLSTA);
    let end = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TFLEND);
    let l2t_ok = sta == 0 && end == !0;
    gaps += !l2t_ok as u32;
    serial_println!(
        "::   [v3d60] L2TFLSTA/L2TFLEND ours={:#010x}/{:#010x} want=0x00000000/0xffffffff match={} — mainline establishes the whole-address-space L2T flush window after every reset (the V3D-51 step) ::",
        sta, end, l2t_ok as u32
    );

    // ── Interrupt working sets (V3D-49 core half, V3D-52 hub half) ───────────────────────────────
    let msk = mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_MSK_STS);
    let hub_msk = mmio_read(V3D_HUB_BASE, V3D_HUB_INT_MSK_STS);
    let fldone_open = msk & V3D_INT_FLDONE == 0;
    gaps += !fldone_open as u32;
    serial_println!(
        "::   [v3d60] INT_MSK_STS ours={:#010x} FLDONE-unmasked={} | HUB_INT_MSK_STS ours={:#010x} — mainline unmasks its core and hub working sets once at probe; both halves are mirrored here (V3D-49 / V3D-52) ::",
        msk, fldone_open as u32, hub_msk
    );

    // ── MISCCFG — mainline leaves it alone on this generation ────────────────────────────────────
    let misccfg = mmio_read(V3D_CORE0_BASE, V3D_CTL_MISCCFG);
    serial_println!(
        "::   [v3d60] MISCCFG ours={:#010x} (OVRTMUOUT={}) — mainline writes the TMU-output override only on the pre-4.1 path; on 4.2 the register stays at its reset value and UnaOS never writes it. No divergence, recorded for completeness ::",
        misccfg, (misccfg & V3D_MISCCFG_OVRTMUOUT != 0) as u32
    );

    // ── GMP — mainline never writes it; reset state is allow-all ─────────────────────────────────
    let gmp_cfg = mmio_read(V3D_CORE0_BASE, V3D_GMP_CFG);
    let prot_on = gmp_cfg & V3D_GMP_CFG_PROT_ENABLE != 0;
    gaps += prot_on as u32;
    serial_println!(
        "::   [v3d60] GMP_CFG ours={:#010x} PROT_ENABLE={} — mainline writes NO memory-protection register at init; the reset state has protection disabled (allow-all, not default-deny). Protection reading ENABLED here would mean something outside this driver armed it ::",
        gmp_cfg, prot_on as u32
    );

    serial_println!(
        ":: V3D: [v3d60] initdelta ({}) verdict — MEASURED divergences={} STANDING gaps={} (total {}) — {} ::",
        tag, gaps, standing_gaps, gaps + standing_gaps,
        if gaps == 0 && standing_gaps == 0 {
            "our pre-first-bin-job register state matches every row of the mainline ledger this audit can check, INCLUDING the two rows V3D-60 found open — the fault-reporting policy and the dedicated illegal-address scratch page are both in place (V3D-62). No boot-state divergence remains to explain the dead-open frame, and the MMU can no longer refuse a write unreported"
        } else if gaps == 0 {
            "every MEASURED row matches mainline and no standing gap remains in this ledger"
        } else {
            "the rows whose check reads 0 are real differences between our boot state and the state mainline hands its first bin job. If either MMU row is among them, the V3D-62 fault-reporting instrument is not fully established this boot and `[v3d62] fault`'s exoneration branch must NOT be read as conclusive"
        }
    );
}

/// `[v3d60] syncrd` — settle the read-side-effect question `[v3d59] ctstate` hedged on. Two BACK-TO-BACK
/// reads of each CLE semaphore register at a quiescent station, nothing in between. A difference can
/// only be the read itself.
fn v3d60_syncrd(tag: &str) {
    if !V3D60_SYNCRD {
        return;
    }
    let a1 = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0SYNC);
    let a2 = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0SYNC);
    let b1 = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1SYNC);
    let b2 = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1SYNC);
    let self_modifying = a1 != a2 || b1 != b2;
    serial_println!(
        ":: V3D: [v3d60] syncrd ({}) — back-to-back reads at a QUIESCENT station, no write and no kick in between: CT0SYNC {:#010x} then {:#010x} | CT1SYNC {:#010x} then {:#010x} — {} ::",
        tag, a1, a2, b1, b2,
        if self_modifying {
            "THE SEMAPHORE REGISTERS SELF-MODIFY ON READ. Nothing happened between the two reads but the read itself, so [v3d59] ctstate's `sema_moved` row is a PROBE ARTIFACT (five stations = five reads per register) and is RETRACTED. Any future decode must sample these registers at most once per boot"
        } else {
            "the semaphore registers are STABLE under back-to-back reads — no read side effect. [v3d59] ctstate's semaphore row therefore stands as measured, and its five-reads-per-register hedge can be dropped: whatever it reported about CT0SYNC/CT1SYNC motion was real block state, not the probe moving what it measured"
        }
    );
}

/// `[v3d60] gmpdelta` — the memory-protection and MMU fault latches, sampled PRE-kick and again at
/// wait-exit, reported as what latched DURING the frame. A silent drop is a delta question, and every
/// prior reading of these registers was a single post-hoc sample.
#[derive(Clone, Copy)]
struct V3d60Prot {
    gmp_status: u32,
    gmp_vio_addr: u32,
    gmp_vio_type: u32,
    mmu_ctl: u32,
    mmu_vio_addr: u32,
    mmu_vio_id: u32,
}

fn v3d60_prot_sample() -> V3d60Prot {
    V3d60Prot {
        gmp_status: mmio_read(V3D_CORE0_BASE, V3D_GMP_STATUS),
        gmp_vio_addr: mmio_read(V3D_CORE0_BASE, V3D_GMP_VIO_ADDR),
        gmp_vio_type: mmio_read(V3D_CORE0_BASE, V3D_GMP_VIO_TYPE),
        mmu_ctl: mmio_read(V3D_HUB_BASE, V3D_MMU_CTL),
        mmu_vio_addr: mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR),
        mmu_vio_id: mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ID),
    }
}

fn v3d60_emit_gmpdelta(tag: &str, pre: &V3d60Prot, post: &V3d60Prot) {
    if !V3D60_GMPDELTA {
        return;
    }
    let fault_mask = V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED;
    let mmu_pre = pre.mmu_ctl & fault_mask;
    let mmu_post = post.mmu_ctl & fault_mask;
    let mmu_new = mmu_post & !mmu_pre;
    let gmp_vio_pre = pre.gmp_status & (V3D_GMP_STATUS_VIO | V3D_GMP_STATUS_INVPROT);
    let gmp_vio_post = post.gmp_status & (V3D_GMP_STATUS_VIO | V3D_GMP_STATUS_INVPROT);
    let gmp_new = gmp_vio_post & !gmp_vio_pre;
    let verdict = if gmp_new != 0 {
        "A MEMORY-PROTECTION VIOLATION LATCHED DURING THE BIN FRAME. This is the silent-drop mechanism the campaign has been hunting: the protection block refuses the PTB's pool write, the write never lands, and no MMU fault and no CTERR is ever raised — item-accept-without-pool-write, exactly. Read the violation address/type columns for what the PTB was reaching for"
    } else if mmu_new & V3D_MMU_CTL_CAP_EXCEEDED != 0 {
        "THE MMU ADDRESS CAP WAS EXCEEDED DURING THE FRAME. The PTB issued an address beyond the page-table cap — the access is capped, not translated, and the pool write goes nowhere our page table describes. That reconciles 'BPCA advances' with 'no traffic anywhere the MMU grants'"
    } else if mmu_new != 0 {
        "AN MMU FAULT LATCHED DURING THE BIN FRAME (page-table-invalid and/or write-violation) that was NOT latched before the kick. The PTB's write was refused by translation; read MMU_VIO_ADDR/VIO_ID for the address and the client that issued it"
    } else if gmp_vio_post != 0 || mmu_post != 0 {
        "a protection/translation fault is latched, but it was ALREADY latched before this kick — it belongs to an earlier job (or to the reset state), not to this bin frame. Nothing new was refused during the frame"
    } else {
        "CLEAN ACROSS THE FRAME — no protection violation, no page-table-invalid, no write-violation, no address-cap event latched between the pre-kick sample and wait-exit. The PTB's missing pool write was NOT refused by the memory-protection block and NOT refused by the MMU; both are exonerated for this frame, and the PTB frame open/close unit stands alone as the wall"
    };
    serial_println!(
        ":: V3D: [v3d60] gmpdelta ({}) — ACROSS the bin frame (pre-kick -> wait-exit) | GMP_STATUS {:#010x}->{:#010x} (violation bits newly set={:#010x}) VIO_ADDR {:#010x}->{:#010x} VIO_TYPE {:#010x}->{:#010x} | MMU_CTL fault bits {:#010x}->{:#010x} (newly set={:#010x}) VIO_ADDR {:#010x}->{:#010x} VIO_ID {:#010x}->{:#010x} — {} ::",
        tag,
        pre.gmp_status, post.gmp_status, gmp_new,
        pre.gmp_vio_addr, post.gmp_vio_addr,
        pre.gmp_vio_type, post.gmp_vio_type,
        mmu_pre, mmu_post, mmu_new,
        pre.mmu_vio_addr, post.mmu_vio_addr,
        pre.mmu_vio_id, post.mmu_vio_id,
        verdict
    );
}

// ═══ PI-V3D-62: the fault-REPORTING instrument ═══════════════════════════════════════════════════
//
// V3D-60 measured two STANDING gaps against the state mainline hands its first bin job, and named both
// as "mechanisms by which a refused PTB write lands, or vanishes, unreported". `program_mmu` above now
// closes them. This pair of witnesses is what makes the closure USEFUL: arming a fault-reporting policy
// buys nothing unless something reads the report.
//
// `v3d62_arm` runs at the pre-kick station, after the stale-latch clear. It clears the hub's latched
// interrupt vector (so anything seen afterwards belongs to THIS frame) and seeds the scratch page with a
// sentinel. `v3d62_frame_witness` runs at wait-exit, BEFORE the post-bin latch clear that would destroy
// the evidence, and reports three independent channels:
//
//   (1) MMU_CTL fault latches + VIO_ADDR/VIO_ID/DEBUG_INFO — the address a refusal was issued for and
//       the client that issued it. Newly meaningful now the address-cap condition is armed at all.
//   (2) HUB_INT_STS MMU bits (PTI/WRV/CAP) — the interrupt half, latched RAW regardless of the mask.
//       This channel did not exist before this arc: with the _INT bits unset, the hub never latched.
//   (3) The scratch page. A sentinel that survives means nothing was redirected there; a sentinel that
//       is GONE is direct evidence that a refused access LANDED, which is the "write goes somewhere
//       unaccounted" branch no register can show. This channel only became sound once the catcher stopped
//       pointing into the arena — a scratch page inside the job's own mapped space would be overwritten
//       by legal traffic and the reading would be meaningless.
//
// Read-only w.r.t. the job: the hub-interrupt clear and the sentinel seed touch no CLE or PTB state, and
// the scratch page is unmapped memory no job can address through translation. Both are gated on the hub
// identity word being live, so QEMU raspi4b (which models no V3D) skips them.

/// `[v3d62] fault` — the across-frame fault-report witness (all three channels).
const V3D62_FAULT: bool = true;

/// Pre-kick: clear the hub interrupt latch and seed the illegal-address scratch page with the sentinel,
/// so the wait-exit reading is attributable to THIS frame.
fn v3d62_arm(tag: &str) {
    if !V3D62_FAULT {
        return;
    }
    let sts_entry = mmio_read(V3D_HUB_BASE, V3D_HUB_INT_STS);
    // W1C: echo back what is latched (the same idiom `clear_mmu_fault_latch` uses on MMU_CTL).
    if sts_entry != 0 {
        mmio_write(V3D_HUB_BASE, V3D_HUB_INT_CLR, sts_entry);
    }
    // Seed the scratch page. It is ordinary cacheable RAM to the CPU; clean it so the sentinel is in DRAM
    // where a GPU-side redirect would overwrite it, rather than sitting in the D-cache masking the test.
    let scratch = mmu_scratch_phys();
    unsafe {
        let p = (&raw mut V3D_MMU_SCRATCH).cast::<u32>();
        for i in 0..PAGE / 4 {
            p.add(i).write_volatile(V3D62_SCRATCH_SENTINEL);
        }
    }
    cache::clean_range(scratch, PAGE);
    dsb();
    let sts_now = mmio_read(V3D_HUB_BASE, V3D_HUB_INT_STS);
    serial_println!(
        ":: V3D: [v3d62] arm ({}) — HUB_INT_STS {:#010x}->{:#010x} (MMU bits cleared: PTI/WRV/CAP mask {:#010x}) | scratch page {:#x} seeded with {:#010x} x{} words — anything reported at wait-exit belongs to THIS frame ::",
        tag, sts_entry, sts_now,
        V3D_HUB_INT_MMU_PTI | V3D_HUB_INT_MMU_WRV | V3D_HUB_INT_MMU_CAP,
        scratch, V3D62_SCRATCH_SENTINEL, PAGE / 4
    );
}

/// Wait-exit: read all three fault-report channels and deliver the verdict. MUST be called BEFORE the
/// post-frame `clear_mmu_fault_latch`, which is write-1-to-clear and would erase channel (1).
fn v3d62_frame_witness(tag: &str) {
    if !V3D62_FAULT {
        return;
    }
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    let vio_addr = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
    let vio_id = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ID);
    let dbg = mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO);
    let hub_sts = mmio_read(V3D_HUB_BASE, V3D_HUB_INT_STS);
    let hub_mmu = hub_sts & (V3D_HUB_INT_MMU_PTI | V3D_HUB_INT_MMU_WRV | V3D_HUB_INT_MMU_CAP);

    // Channel (3): did anything land on the unmapped scratch page? Invalidate first — the GPU is
    // non-coherent, so a CPU read without invalidation could return our own cached sentinel and report a
    // clean page over a dirtied one (a false negative on the ONE channel that catches a silent landing).
    let scratch = mmu_scratch_phys();
    cache::invalidate_range(scratch, PAGE);
    let mut dirty_words = 0usize;
    let mut first_dirty_off = 0usize;
    let mut first_dirty_val = 0u32;
    unsafe {
        let p = (&raw const V3D_MMU_SCRATCH).cast::<u32>();
        for i in 0..PAGE / 4 {
            let w = p.add(i).read_volatile();
            if w != V3D62_SCRATCH_SENTINEL {
                if dirty_words == 0 {
                    first_dirty_off = i * 4;
                    first_dirty_val = w;
                }
                dirty_words += 1;
            }
        }
    }

    let verdict = if dirty_words != 0 {
        "AN ACCESS WAS REDIRECTED TO THE ILLEGAL-ADDRESS SCRATCH PAGE DURING THIS FRAME. The sentinel is gone from an unmapped page belonging to no job, so a GPU access the MMU REFUSED still carried a write, and the catcher absorbed it. This is the 'refused write lands somewhere unaccounted' mechanism V3D-60 named — the PTB's pool write going to the catcher rather than to the pool would explain 'BPCA advances, pool stays empty' exactly. The dirty offset and the MMU_VIO_ADDR/VIO_ID columns say what was reaching for what"
    } else if fault != 0 || hub_mmu != 0 {
        "AN MMU FAULT WAS REPORTED ACROSS THIS FRAME, but the scratch page is untouched — the refusal raised a fault without an accompanying write landing. Read MMU_VIO_ADDR for the address refused and MMU_VIO_ID for the client that issued it; the interrupt half (HUB_INT_STS) is the channel this arc armed and it is now carrying information"
    } else {
        "NO FAULT REPORTED AND THE SCRATCH PAGE IS PRISTINE — under the ARMED policy. This is a materially stronger exoneration than V3D-60's: the MMU is now configured to report page-table-invalid, write-violation and address-cap on both the abort and interrupt channels, and the illegal-address catcher aims at an unmapped page that no legal access can touch. The bin frame's missing PTB write was therefore neither refused by translation nor silently absorbed by the catcher. The MMU is exonerated with the instrument that could have convicted it"
    };
    serial_println!(
        ":: V3D: [v3d62] fault ({}) — MMU_CTL={:#010x} fault-latched={:#010x} (PT_INVALID={} WRITE_VIOLATION={} CAP_EXCEEDED={}) VIO_ADDR={:#010x} VIO_ID={:#010x} DEBUG_INFO={:#010x} | HUB_INT_STS={:#010x} MMU-int bits={:#010x} (PTI={} WRV={} CAP={}) | scratch page {:#x}: dirty words={}/{} first-dirty off={:#x} val={:#010x} — {} ::",
        tag, ctl, fault,
        (fault & V3D_MMU_CTL_PT_INVALID != 0) as u32,
        (fault & V3D_MMU_CTL_WRITE_VIOLATION != 0) as u32,
        (fault & V3D_MMU_CTL_CAP_EXCEEDED != 0) as u32,
        vio_addr, vio_id, dbg,
        hub_sts, hub_mmu,
        (hub_mmu & V3D_HUB_INT_MMU_PTI != 0) as u32,
        (hub_mmu & V3D_HUB_INT_MMU_WRV != 0) as u32,
        (hub_mmu & V3D_HUB_INT_MMU_CAP != 0) as u32,
        scratch, dirty_words, PAGE / 4, first_dirty_off, first_dirty_val,
        verdict
    );
}

/// PI-V3D-27: run the Mesa-compiled attribute-DMA probe as a one-off bin job over the real draw's vertex
/// buffer, then read back the four attribute words the QPU stored via TMU. Runs BEFORE the real M4 bin;
/// reuses the M4 bin-scratch regions (re-zeroed by triangle_job before the real kick). Assumes
/// OFF_VTXDATA + OFF_DEFAULT_ATTRS already hold this draw's data (triangle_job step 0). Prints the
/// three-way discrimination witness; returns nothing (diagnostic-only, never gates M4).
fn probe_job() {
    serial_println!(
        ":: V3D: [v3d28] attribute-DMA probe — Mesa-compiled TMU-store coord shader over the real vertex buffer; post-bin L2T flush drains the store, canary window catches wrong-address landings ::"
    );
    let scratch_v3d = (arena_phys() + OFF_PROBE_SCRATCH) as u32;

    // Publish probe code + uniforms + record; pre-seed scratch with the 0x55 "store never landed"
    // sentinel (distinct from loaded-zeros 0x00000000 and from real coords).
    let code_len = write_shader_words(OFF_PROBE_CODE, &PROBE_WORDS);
    let unif_len = write_probe_uniforms(OFF_PROBE_UNIF, scratch_v3d);
    let num_attrs = build_probe_shader_record();
    let bin_len = build_bin_cl_generic(OFF_PROBE_BIN_CL, OFF_PROBE_SHADREC, num_attrs);
    // V3D-38: publish the M4 shader-state record now (triangle_job builds it again identically after us)
    // so the [v3d38] field diff below can decode the confirmed-dispatching M4 record alongside the probe's.
    // Idempotent — writes OFF_SHADREC from the same M4 FS/VS/CS addresses triangle_job already published.
    build_shader_record();
    // Seed the store target (words 0..4) with the 0x55 "never-landed" sentinel and the tail (words
    // 4..PROBE_CANARY_WORDS) with per-index canaries 0xCA00_00NN, so a wrong-address landing inside the
    // page is caught by which canary flipped rather than masquerading as an untouched sentinel.
    fill_region(OFF_PROBE_SCRATCH, 16, 0x5555_5555);
    for i in 4..PROBE_CANARY_WORDS {
        arena_write_u32(OFF_PROBE_SCRATCH + i * 4, 0xCA00_0000 | i as u32);
    }

    // Publish everything to RAM for the non-coherent GPU (vertex data + defaults were written by the
    // caller; clean them here so the probe sees them).
    cache::clean_range(arena_phys() + OFF_PROBE_CODE, code_len);
    cache::clean_range(arena_phys() + OFF_PROBE_UNIF, unif_len);
    cache::clean_range(arena_phys() + OFF_PROBE_SHADREC, 36 + 16);
    cache::clean_range(arena_phys() + OFF_PROBE_BIN_CL, bin_len);
    cache::clean_range(arena_phys() + OFF_PROBE_SCRATCH, PROBE_CANARY_BYTES);
    cache::clean_range(arena_phys() + OFF_VTXDATA, TRI_VERTS.len() * 16);
    cache::clean_range(arena_phys() + OFF_DEFAULT_ATTRS, 16);
    // V3D-38 BORROW: the probe record now points its FS/VS slots at M4's known-good programs + uniform
    // streams. triangle_job wrote those bytes before calling probe_job but does not clean them until AFTER
    // us, so flush them to RAM here — the non-coherent GPU must see M4's real FS/VS at bin time, not stale
    // DRAM. Also flush the M4 record we just published for the [v3d38] witness.
    cache::clean_range(arena_phys() + OFF_FS_CODE, FS_WORDS.len() * 8);
    cache::clean_range(arena_phys() + OFF_VS_CODE, CS_VS_WORDS.len() * 8);
    // V3D-39 (Task A): the probe record's CS slot now points at OFF_CS_CODE (M4's known-dispatchable CS
    // program). triangle_job wrote those bytes before calling us but does not clean them until AFTER us, so
    // flush them to RAM here — the non-coherent GPU must run M4's CS at the probe bin, not stale DRAM.
    cache::clean_range(arena_phys() + OFF_CS_CODE, CS_VS_WORDS.len() * 8);
    cache::clean_range(arena_phys() + OFF_FS_UNIF, 6 * 4);
    cache::clean_range(arena_phys() + OFF_VS_UNIF, 11 * 4);
    cache::clean_range(arena_phys() + OFF_SHADREC, 36 + 16);

    // ── [v3d38] shader-state record field diff (probe vs confirmed-dispatching M4) ────────────────
    // With the two binning CLs proven byte-identical (v3d36) and CS threading refuted both 2-way and 4-way
    // (v3d37, clean counters), the RECORD the GL_SHADER_STATE pointer selects is the last variable. Dump
    // both 36-byte records field-by-field so the difference is decidable from the log. Post-V3D-38-borrow
    // the FS/VS fields read `==` (probe shares M4's FS+VS); only the CS code/uniform pointers still carry
    // the probe's TMU-store program.
    witness_shadrec_diff();

    // ── [v3d36] PROBE bin CL structural decode ──────────────────────────────────────────────────
    // Put the probe's binning CL on serial packet-by-packet, so it can be diffed against the M4 bin CL
    // (decoded from triangle_job under the same [v3d36] tag). Both come from build_bin_cl_generic, so
    // they must agree packet-for-packet except the GL_SHADER_STATE `record` pointer. If they do, the
    // control list is EXONERATED as the cause of the coord shader never dispatching (valid_instr=0) and
    // the difference lives in the shader-state record — see build_probe_shader_record.
    decode_cl_packets("PROBE", OFF_PROBE_BIN_CL, bin_len);
    // PI-V3D-57: the same list again, read back from the published bytes and packing-checked field by
    // field against the audited v42 encoding (see the packing-consistency note on v3d57_cl_mesa_diff).
    v3d57_cl_mesa_diff("PROBE", OFF_PROBE_BIN_CL, bin_len);

    // ── [v3d30] executed-bytes witness ──────────────────────────────────────────────────────────
    // v3d28 VERDICT store-never-issued (canaries + sentinel intact, no fault) proves the TMU write
    // never drained. Before blaming the TMU config, put the GROUND TRUTH on serial: (a) which code
    // PAs the shader record's CS/VS/FS start-address fields actually resolve to (read BACK from the
    // arena, decoded exactly as the CLE will), vs the probe program PA we intended; and (b) the
    // first-4 / last-4 QPU words physically present AT the recorded CS start address — so the capture
    // shows the bytes the QPU runs, not the bytes we think we wrote. The tail words also expose the
    // thrsw/thread-end shape: PROBE_WORDS carries `thrsw` on words [18],[19] AND [22] (the tmuwt
    // word) — [19] sits in [18]'s thrsw delay slot, so the thread can terminate after [18]'s two
    // delay slots ([19],[20]), leaving [21] vpmwt and [22] tmuwt UNEXECUTED and the store (fired at
    // [9] `mov tmuau`) never completed → dropped. This witness makes that decidable from the capture.
    // V3D-39 (Task A): the probe record's CS slot now intentionally holds M4's CS program (OFF_CS_CODE),
    // NOT OFF_PROBE_CODE — the decisive dispatch witness. Expect CS start == M4's CS code PA (the swap), and
    // the word[9] decode below to show a CS_VS_WORDS instruction (a passthrough, NOT the probe's tmuau).
    let probe_code_pa = (arena_phys() + OFF_CS_CODE) as u32; // V3D-39: the intended (M4) CS program PA
    // Decode the record straight from the arena (same field layout build_probe_shader_record wrote):
    //   FS code @bit99  w29 → word@byte12 >>3<<3 ; VS code @bit163 → word@byte20 ; CS code @bit227 →
    //   word@byte28. The low bits of the CS/VS/FS words carry the threadability/propagate-NaN flags.
    let rec_w12 = probe_word(OFF_PROBE_SHADREC + 12);
    let rec_w20 = probe_word(OFF_PROBE_SHADREC + 20);
    let rec_w28 = probe_word(OFF_PROBE_SHADREC + 28);
    let rec_cs_unif = probe_word(OFF_PROBE_SHADREC + 32);
    let fs_code_pa = rec_w12 & 0xFFFF_FFF8;
    let vs_code_pa = rec_w20 & 0xFFFF_FFF8;
    let cs_code_pa = rec_w28 & 0xFFFF_FFF8;
    let cs_4way = rec_w28 & 1; // bit 224 (V3D-36: expect 1 — mirror the dispatching M4 coord shader)
    let cs_2way = (rec_w28 >> 1) & 1; // bit 225 (V3D-36: expect 0 — the 2-way flip killed dispatch)
    let cs_propnan = (rec_w28 >> 2) & 1; // bit 226
    let vs_2way = (rec_w20 >> 1) & 1; // bit 161
    serial_println!(
        ":: V3D: [v3d30] shader-record decode — probe code PA={:#010x} | CS start={:#010x} (4way={} 2way={} propNaN={}) VS start={:#010x} (2way={}) FS start={:#010x} | CS unif={:#010x} — CS start {} probe code ::",
        probe_code_pa, cs_code_pa, cs_4way, cs_2way, cs_propnan, vs_code_pa, vs_2way, fs_code_pa, rec_cs_unif,
        if cs_code_pa == probe_code_pa { "==" } else { "!= (MISMATCH — bin ran the WRONG program)" }
    );
    // Read the executed QPU words BACK from the arena at the recorded CS start address. If the record
    // points elsewhere the readback follows it; here CS start == probe code, so off resolves to the
    // probe program. Print first 4 and last 4 of the 25-word program as full 64-bit instruction words.
    let cs_off = (cs_code_pa as usize).wrapping_sub(arena_phys());
    let qword = |instr: usize| -> u64 {
        let o = cs_off + instr * 8;
        (probe_word(o) as u64) | ((probe_word(o + 4) as u64) << 32)
    };
    if arena_contains(cs_code_pa as usize, PROBE_WORDS.len() * 8) {
        serial_println!(
            ":: V3D: [v3d30] executed QPU words @CS start — first4=[{:#018x} {:#018x} {:#018x} {:#018x}] ::",
            qword(0), qword(1), qword(2), qword(3)
        );
        serial_println!(
            ":: V3D: [v3d30] executed QPU words @CS start — last4=[{:#018x} {:#018x} {:#018x} {:#018x}] (word22=tmuwt+thrsw; thrsw also on words 18,19 — thread may end before tmuwt) ::",
            qword(21), qword(22), qword(23), qword(24)
        );
        // PI-V3D-33: echo the static waddr decode of word[9] into the capture alongside the bytes that ran,
        // so the "is the store even asking the TMU for a write?" question is answered from the log itself.
        // MA(bit44)=1 magic-write, WADDR_A(bits43:38)=13=TMUAU (not 12=TMUA) — config IS pulled from the
        // uniform FIFO. Encoding exonerated; if the TMU witness below reads SAW-NOTHING the defect is the
        // ISSUE path (thread-end/quad-mask), not the config/address.
        let w9 = qword(9);
        let ma = (w9 >> 44) & 1;
        let waddr_a = (w9 >> 38) & 0x3f;
        serial_println!(
            ":: V3D: [v3d33] word[9]={:#018x} decode — MA(magic-write)={} WADDR_A={} ({}) — general store {} the TMU for a write ::",
            w9, ma, waddr_a,
            match waddr_a { 11 => "TMUD", 12 => "TMUA", 13 => "TMUAU", 6 => "NOP", _ => "OTHER" },
            if ma == 1 && waddr_a == 13 { "correctly asks" } else { "does NOT correctly ask" }
        );
    } else {
        serial_println!(
            ":: V3D: [v3d30] CS start PA {:#010x} + program length escapes the arena — cannot read back executed bytes ::",
            cs_code_pa
        );
    }

    // ── [v3d32] uniform-stream witness ────────────────────────────────────────────────────────────
    // A threads-correct record still drops the store if the tmuau at word [9] pops a wrong TMU write
    // CONFIG. The probe pops its uniforms in FIFO order; the TMU store target (u4 = UBO_ADDR → scratch)
    // and the write config (u5 = 0xFFFFFFFC) both travel in this stream. Dump the 12 words physically AT
    // the record's CS-unif pointer (read back from the arena, exactly what the QPU FIFO will pop) and
    // compare each to the artifact's expected list (scripts/pi-v3d26-mesa-compile.out.txt PROBE VS,
    // u0..u11), PASS/DIVERGE per slot. Slot 4 is the driver-patched UBO_ADDR (compared to scratch_v3d,
    // not the artifact's 0 placeholder); slots 6/7 are the driver-patched viewport scales (0x46000000 =
    // 8192.0f, vs the artifact's 0 placeholder). A DIVERGE on u5 (config) or u4 (address) is a silent
    // store-drop / wrong-address cause independent of threading.
    let cs_unif_off = (rec_cs_unif as usize).wrapping_sub(arena_phys());
    let expect_unif: [u32; 12] = [
        0, 1, 2, 3, scratch_v3d, 0xFFFF_FFFC, 0x4600_0000, 0x4600_0000, 2, 3, 4, 5,
    ];
    if arena_contains(rec_cs_unif as usize, 12 * 4) {
        let mut diverged = 0u32;
        for i in 0..12 {
            let got = probe_word(cs_unif_off + i * 4);
            let exp = expect_unif[i];
            if got != exp {
                diverged += 1;
                serial_println!(
                    ":: V3D: [v3d32] uniform u[{:2}] got={:#010x} exp={:#010x} DIVERGE{} ::",
                    i, got, exp,
                    match i { 4 => " (UBO_ADDR/scratch — store TARGET)", 5 => " (0xFFFFFFFC — TMU write CONFIG)", 6 | 7 => " (viewport scale 8192.0f)", _ => "" }
                );
            }
        }
        serial_println!(
            ":: V3D: [v3d32] uniform-stream witness @CS unif={:#010x} — {} — u4(UBO_ADDR)={:#010x} u5(config)={:#010x} (expect config 0xfffffffc; store target = scratch {:#010x}) ::",
            rec_cs_unif,
            if diverged == 0 { "12/12 PASS — stream matches the Mesa artifact byte-for-byte" } else { "DIVERGE — see per-slot lines above" },
            probe_word(cs_unif_off + 16), probe_word(cs_unif_off + 20), scratch_v3d
        );
    } else {
        serial_println!(
            ":: V3D: [v3d32] CS unif {:#010x} + 48 B escapes the arena — cannot read back the uniform stream ::",
            rec_cs_unif
        );
    }
    // ─────────────────────────────────────────────────────────────────────────────────────────────

    // Bin scratch (reused from M4): zero + clean — or, since PI-V3D-56, POISON + clean. Same two
    // regions, same bytes touched; the only change is the value. A zeroed pool cannot witness a
    // zero-valued PTB write, which is exactly the write an empty tile list makes — see the V3D-56
    // block above for why that blind spot underwrites the whole phantom-BPCA verdict.
    if V3D56_POISON {
        v3d56_poison_region(OFF_TILESTATE, TILE_STATE_BYTES);
        v3d56_poison_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
        serial_println!(
            ":: V3D: [v3d56] armed — tile-state ({} B) + tile-alloc pool ({} B) filled with poison word[i] = {:#010x}^i (was: zeroed) and cleaned to PoC. Post-job any word reading 0 is a PTB write of ZERO — a class every boot from V3D-40 to V3D-55 was structurally unable to observe ::",
            TILE_STATE_BYTES, BIN_TILEALLOC_BYTES, V3D56_POISON_SEED
        );
    } else {
        fill_region(OFF_TILESTATE, TILE_STATE_BYTES, 0);
        fill_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES, 0);
        cache::clean_range(arena_phys() + OFF_TILESTATE, TILE_STATE_BYTES);
        cache::clean_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    }
    // PI-V3D-44: zero + publish the overflow tile-list pool so the pre-armed BPOA/BPOS block below
    // is fresh, coherent memory the PTB can spill into if the 128 B initial block runs out.
    for i in (0..PROBE_BIN_OVERFLOW_BYTES).step_by(4) {
        arena_write_u32(OFF_PROBE_BIN_OVERFLOW + i, 0);
    }
    cache::clean_range(arena_phys() + OFF_PROBE_BIN_OVERFLOW, PROBE_BIN_OVERFLOW_BYTES);

    let bin_ba = (arena_phys() + OFF_PROBE_BIN_CL) as u32;
    let bin_ea = bin_ba + bin_len as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let ts = (arena_phys() + OFF_TILESTATE) as u32;
    if !arena_contains(bin_ba as usize, bin_len)
        || !arena_contains(OFF_PROBE_SCRATCH + arena_phys(), PROBE_CANARY_BYTES)
        || !arena_contains(tile_alloc as usize, BIN_TILEALLOC_BYTES)
        || !arena_contains(ts as usize, TILE_STATE_BYTES)
    {
        serial_println!(":: V3D: [v3d28] probe range escapes the arena — skipping probe (fail-closed) ::");
        return;
    }

    // ── [v3d35] scope the PCTR battery to the PROBE bin (the store's ONLY home) ────────────────────
    // Root fact V3D-35 surfaces: every prior [v3d33] "TMU SAW-NOTHING" reading was armed around the REAL
    // M4 bin (pctr_read_cs_witness("M4 post-bin") in triangle_job), whose coord shader CS_VS_WORDS is a
    // pure VPM-in→VPM-out passthrough with NO TMU op. That shader CANNOT touch the TMU, so tcache_access=0
    // there is a tautology, not evidence about the store. The probe — the ONLY shader in the kernel that
    // issues a general TMU store (word[9] `mov tmuau`) — was never PCTR-instrumented. The battery has been
    // measuring the wrong program. Arm it HERE, around the probe GO, so tcache_access / cycles_waiting_tmu
    // and valid_instr are finally scoped to the store that matters.
    //
    // TMUAU-LAUNCH vs TMUWT-DRAIN reconciliation (Mesa v3d ntq_emit_tmu_general / qpu_instr.h, v42):
    // a general TMU store LAUNCHES when the address register is written — writing TMUA/TMUAU issues the
    // memory transaction to the TMU immediately; the TMUD writes before it only stage the data. `tmuwt`
    // ("TMU write wait", word[22]) is NOT a launch: it is a completion barrier that stalls the thread until
    // outstanding TMU writes have drained. So the store fires at [9] (PRE the [18]/[19] thread switch); the
    // TMU sees it there regardless of whether the post-switch tail ([20]-[22]) ever resumes. Consequence:
    // if this PROBE-scoped tcache_access reads 0, the leading "post-switch segment never resumes" theory is
    // REFUTED as the cause — the store's issuance does not depend on the tail. The verdict then reduces to
    // valid_instr: a full-run count (~23·lanes) with tcache_access=0 means the thread reached [9] but the
    // TMU rejected the launch (config/quad-mask/block state) — chase the TMU; a truncated count (< ~9·lanes)
    // means the thread died before [9] and the store word never executed — chase thread lifetime/dispatch.
    // ── [v3d58] STATION S0 — the virgin sample ───────────────────────────────────────────────────────
    // Taken before this driver touches ANY CT0 or PTB register for this job. The probe is the first CT0
    // bin kick of the boot (M3 kicked CT1 only), so a `PCS.BMACTIVE=1` here means the block left the
    // reset cycle — or the preceding CT1 render frame — with a BIN frame still open, and every
    // START_TILE_BINNING since has been stacking onto a frame nobody closed. No boot has ever sampled
    // this window; see the V3D-58 facts block for why that reading would explain the whole campaign.
    let st0 = v3d58_sample();
    // [v3d59] the citation ledger + the optional CT0 thread reset. The ledger is emitted before the kick
    // so the capture carries its own mainline provenance; the reset is DISARMED and reports the virgin
    // CT0CS decode either way (it must run after S0 so the S0 sample stays a true pre-program reading).
    v3d59_mainline_ledger();
    // [v3d60] the ours-vs-mainline init ledger — read-only, taken at this quiescent pre-kick station.
    // (`v3d60_syncrd`, the CTnSYNC read-side-effect adjudicator, deliberately does NOT sit here: it
    // would add two reads per register INSIDE the S0..S4 window whose semaphore readings it exists to
    // adjudicate. It runs after `v3d59_emit_ctstate`, once that series is closed — see below.)
    v3d60_initdelta("v3d40 PROBE pre-kick");
    v3d59_ct0_frame_reset("v3d40 PROBE");
    clear_mmu_fault_latch("v3d28 pre-probe");
    // [v3d60] the PRE half of the fault-latch delta — taken AFTER the MMU fault latch is cleared, so
    // anything the post-frame sample shows was latched BY THIS FRAME and not inherited.
    let prot_pre = v3d60_prot_sample();
    // PI-V3D-57: kernel-exact ORDER — BPOS=0 is `v3d_bin_job_run`'s FIRST write, ahead of the cache
    // invalidate and the CT0 tile-memory latch (see `bin_prejob_bpos_clear`).
    bin_prejob_bpos_clear("v3d40 PROBE");
    invalidate_gpu_caches("L2T flush (probe bin pre-kick)");
    // ── [v3d55] RANK 3b/3c: audit the clock domain + MISCCFG BEFORE the kick ─────────────────────────
    // The QPU provably executes yet the flush unit never fires; before blaming another packet, read the
    // clock the block was GRANTED (bringup commands 500 MHz and never reads back) and the QRMAXCNT
    // request-queue depth that has been declared-but-never-audited since V3D-34. Pure reads (the scoped
    // QRMAXCNT write is disarmed — see V3D55_ARM_QRMAXCNT).
    v3d55_clock_domain_audit("v3d40 PROBE pre-kick");
    // ── [v3d40] the probe kick was the ONE CT0 kick without a CT0CA-progression witness ──────────────
    // V3D-40 root fact: after v3d36 (CLs byte-identical), v3d38 (records field-identical bar cs_uniforms)
    // and v3d39 (M4's OWN CS program in the probe slot) the probe STILL reads valid_instr=0 while M4 —
    // same CLE, same registers, byte-identical CL — dispatches (valid_instr=57). The variable left is no
    // longer WHAT the job says but HOW/WHERE/WHEN it runs. The probe is the FIRST CT0 bin kick of the boot
    // (M3's clear_job kicks CT1 only); M4 is the SECOND. Yet the probe kick never captured the CT0CS/CT0CA
    // progression the M4 kick has carried since PI-V3D-13 — so we have never known whether the probe's CLE
    // even ADVANCES through its control list (CT0CA: BA→EA) or is silently refused at the GO (CT0CA stuck
    // at BA / CT0CS ERROR). Instrument the probe kick to the exact shape as M4's (pre / immediately-after-
    // GO / at-idle snapshots of CT0CS + CT0CA, the kick-register echo, ct0_ran, and the pool witness) so
    // the next metal boot DIFFS the two kicks directly: if the probe's CT0CA reaches EA like M4's, the CLE
    // ran the list and the wall is downstream (dispatch/QPU); if CT0CA stays at BA, the GO was refused and
    // the wall is the kick/CLE-state itself (needs a reset/flush between the M3 CT1 job and the first CT0
    // bin). Reads only — no shader-visible state is touched.
    let ct0_cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, tile_alloc);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, BIN_TILEALLOC_BYTES as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, ts | V3D_CLE_CT0QTS_ENABLE);
    dsb();
    // [v3d40] echo the tile-memory registers before the GO — same check M4's kick runs, so a probe-vs-M4
    // discrepancy in what the slots latch is visible in the log.
    bin_mem_prekick_witness("v3d40 PROBE", tile_alloc, BIN_TILEALLOC_BYTES as u32, ts | V3D_CLE_CT0QTS_ENABLE);
    // [v3d58] STATION S1 — the tile-memory registers are latched, the list is NOT yet queued. If BPCS
    // has already dropped below the size we wrote into CT0QMS, the pool "reservation" is a side effect
    // of the register write and NOT the PTB acting on START_TILE_BINNING.
    let st1 = v3d58_sample();
    // [v3d59] the overflow arm sits HERE — after S1, so the S1 latch-artifact reading stays uncontaminated
    // by a BPOS write, and before CT0QBA, so an armed pool is live for the whole frame. Disarmed by
    // default: mainline enters every bin frame with BPOS=0 (see [v3d59] mainline T1).
    v3d59_arm_overflow("v3d40 PROBE");
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, bin_ba);
    dsb();
    // [v3d58] STATION S2 — list begin address queued, GO (CT0QEA) not yet issued.
    let st2 = v3d58_sample();
    // Arm the execution trio (src14/16/32) + TMU battery (src24/17/25) so they span the probe bin. This is
    // read-only w.r.t. the shader (see pctr_* above) — it cannot perturb execution.
    pctr_setup_cs_witness();
    // PI-V3D-49 established the VALUE (BPOS=0, no pre-armed overflow block: the OUTOMEM-starvation theory
    // was refuted at P43/P44 with OUTOMEM=0/BMOOM=0). PI-V3D-57 moved the WRITE to where the kernel
    // actually issues it — first, ahead of the cache invalidate and the CT0 tile-memory latch — see
    // `bin_prejob_bpos_clear` above the pre-kick sequence.
    // Clear any latched interrupts so this kick's FLDONE (and OUTOMEM/GMPV) reads fresh, not a stale
    // bit from a prior job.
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
    dsb();
    // [v3d41] snapshot the frame-completion counters immediately before the GO — the post-idle diff
    // (ptb_frame_witness below) is the started-vs-never-started discriminator for the V3D-40 wall.
    let bfc_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    let rfc_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_RFC);
    // PI-V3D-56: digest EVERY arena page immediately before the GO. The arena is the entire address
    // space the V3D MMU grants this job, so the post-job diff answers "where did the bytes land?" over
    // the whole reachable space, not just the two regions we happen to read back.
    let arena_pre = v3d56_arena_digest();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, bin_ea); // GO
    dsb();
    // [v3d40] tight kick witness: sample CT0CS + CT0CA the instant after the GO write — a started CLE
    // latches CTRUN here and CT0CA begins advancing off BA. This is the snapshot M4's kick has and the
    // probe's never did.
    let ct0_cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    // [v3d58] STATION S3 — the instant after the GO. BMACTIVE setting HERE (and not at S0/S1/S2) is what
    // proves START_TILE_BINNING actually opened the bin frame.
    let st3 = v3d58_sample();
    let idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CT1CS_CTRUN, "CT0 probe bin");
    let ct0_cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    let probe_ran = ct0_ran(ct0_cs_pre, ct0_cs_kicked, ct0_cs_done, ct0_ca_done, bin_ba, bin_ea);
    let ca_advanced = ct0_ca_done != 0 && ct0_ca_done != bin_ba && ct0_ca_done >= bin_ba && ct0_ca_done <= bin_ea;
    serial_println!(
        ":: V3D: [v3d40] PROBE bin clue — CT0CS pre={:#010x} kicked={:#010x} done={:#010x} CT0CA pre={:#010x} kicked={:#010x} done={:#010x} (BA={:#010x} EA={:#010x}) ran={} idled={} — CT0CA {} (advance {}; if BA-stuck the GO was refused, if ==EA the CLE ran the list and the wall is downstream) ::",
        ct0_cs_pre, ct0_cs_kicked, ct0_cs_done, ct0_ca_pre, ct0_ca_kicked, ct0_ca_done,
        bin_ba, bin_ea, probe_ran as u32, idled as u32,
        if ct0_ca_done == bin_ea { "reached EA" } else if ct0_ca_done == bin_ba || ct0_ca_done == 0 { "stuck at BA/0" } else { "mid-list" },
        ca_advanced as u32
    );
    // ── [v3d44] THE flush-retire wait — poll the true retire signal (INT_STS FLDONE), not CT0CS ──────
    // The CT0CS run bit dropping (`idled` above) only means the CLE stopped fetching; V3D-43 proved the
    // PTB can still be draining/stalled then. Wait for FLDONE — the binning-flush-done interrupt the
    // kernel driver treats as retire — BEFORE the L2T flush + pool readback, so the pool read below
    // reflects a RETIRED bin, not the P40 pre-retire snapshot (BFC Δ0, PCS bit0=1). The witness the
    // brief names carries the raw INT_STS, the microseconds waited, whether FLDONE retired, and the
    // BFC pre/post pair (BFC advancing is the independent frame-completed corroboration of FLDONE).
    // PI-V3D-54 (RANK 2): audit the actually-latched CT0 queue registers vs the intended [BA,EA) and the
    // real CL byte length, BEFORE trusting any retire verdict — a mis-latched EA (==BA / truncated) would
    // make the CLE walk a different list than we built (or none). Echo them under `[v3d54] submit`.
    v3d54_submit_audit("v3d40 PROBE", bin_ba, bin_ea, bin_len);
    // PI-V3D-54 (RANK 1): trace the CL progression across this retire-wait (BA/EA fold the CT0CA offset).
    let (fldone_sts, fldone_us, fldone_retired) = wait_fldone("v3d44 PROBE bin flush", bin_ba, bin_ea);
    let bfc_after_flush = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    serial_println!(
        ":: V3D: [v3d44] FLDONE wait — INT_STS={:#010x} waited={}us retired={} BFC={:#010x}->{:#010x} (Δ{}) BPOA={:#010x} BPOS={:#010x} (V3D-49: BPOS=0 at frame start, kernel-exact) — {} ::",
        fldone_sts, fldone_us, fldone_retired as u32,
        bfc_pre, bfc_after_flush, bfc_after_flush.wrapping_sub(bfc_pre),
        mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOA), mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOS),
        if fldone_retired {
            "the bin flush RETIRED — the P40 read-too-early wall is closed; the pool read below is post-retire truth"
        } else if fldone_sts & V3D_INT_OUTOMEM != 0 {
            "OUTOMEM with BPOS=0: the binner exhausted the initial tile-alloc pool and needs an overflow block (the OUTOMEM path, refuted at P43/P44)"
        } else if fldone_sts & V3D_INT_QPU_MASK != 0 {
            "QPU host-interrupt (bit16+) latched, FLDONE did NOT: the coord shader reached program-end but the PTB never flushed — see the [v3d45] wedge dump above for CTRUN/PCS/BPCA"
        } else {
            "FLDONE never fired — the bin did not retire; see the raw INT_STS bits for what did"
        }
    );
    // ── [v3d58] STATION S4 + the five-station verdict ────────────────────────────────────────────────
    // Sampled at wait exit, BEFORE the L2T flushes and cache maintenance below can perturb anything. The
    // five stations together answer the question every prior boot inferred rather than measured: WHERE in
    // the kick sequence the bin frame opens and the pool reservation happens — which separates "the frame
    // was already open before we started" from "START_TILE_BINNING opened it" from "it never opened".
    let st4 = v3d58_sample();
    v3d58_emit_stations(
        "v3d40 PROBE",
        &[st0, st1, st2, st3, st4],
        tile_alloc,
        BIN_TILEALLOC_BYTES as u32,
    );
    // [v3d59] the two readings the five stations cannot give on their own: the CTnCS/PCS bit decode
    // (CTERR/CTSUBS/CTSEMA — never named until this arc) plus the never-read CT0SYNC/CT1SYNC/BXCF, and
    // then a second look at the wedged block to separate "stalled" from "dead-open". Both pure reads.
    v3d59_emit_ctstate("v3d40 PROBE", &[st0, st1, st2, st3, st4]);
    // [v3d60] the read-side-effect adjudicator runs HERE — after the S0..S4 series is sampled AND
    // emitted, never inside it. `[v3d59] ctstate` hedged its semaphore row on the possibility that its
    // own five reads per register moved what they measured; a probe that answers that question must not
    // itself add reads to the window under adjudication. The block is quiescent at this point (the
    // frame is wedged open, CT0CS static), which is exactly the condition the test needs: two
    // back-to-back reads with nothing between them but the reads.
    // (ordered after the fault-latch delta below, whose post sample must sit as close to wait-exit as
    // it can — see the `v3d60_syncrd` call following it.)
    // [v3d60] close the fault-latch delta at wait-exit, BEFORE the L2T flushes and cache maintenance
    // below can perturb anything: what did the memory-protection block or the MMU refuse DURING the
    // frame? Every prior reading of these registers was a single post-hoc sample.
    v3d60_emit_gmpdelta("v3d40 PROBE", &prot_pre, &v3d60_prot_sample());
    v3d60_syncrd("v3d40 PROBE post-ctstate");
    v3d59_frameclose_poll("v3d40 PROBE post-bin");
    // Read the battery now the probe has idled — THE decisive witness for V3D-35. valid_instr says how far
    // the probe thread got (reached the [9] store word or died before it); tcache_access says whether the
    // TMU saw the store (launched at [9], independent of the post-switch tail per the reconciliation above).
    pctr_read_cs_witness("v3d35 PROBE bin");
    // [v3d40] did the PROBE binner write its tile-alloc pool + tile-state, exactly as M4's post-bin
    // witness reports? A probe that reached EA (CT0CA above) but left the pool all-zero — while M4's pool
    // goes nonzero — localises the divergence to the bin run itself, not the kick.
    bin_pool_witness("v3d40 PROBE post-bin");
    // [v3d55] RANK 4: the 8-byte prefix bin_pool_witness reads is too coarse to separate "PTB wrote
    // tile-state but no FLDONE" from "PTB never wrote". Scan the WHOLE tile-state array + pool head and
    // dump the PTEs covering the CL / tile-state / pool iovas so the aliasing question closes by bytes.
    v3d55_tilestate_readback("v3d40 PROBE post-bin", bin_ba, ts, tile_alloc);
    // ── [v3d56] the poison scan + the whole-arena landing sweep ───────────────────────────────────
    // Run our own L2T write-back and KEEP the completion bit (the V3D-55 evidence-integrity rule: an
    // "untouched" readback is only evidence if the drain actually finished), then invalidate the CPU's
    // view of the WHOLE arena.
    //
    // `invalidate_range` rather than `clean_invalidate_range` is a belt-and-braces preference here, NOT
    // a claim that the clean variant is unsafe — `v3d55_tilestate_readback` calls `clean_invalidate_range`
    // over the pool and tile-state one call earlier in this very path, and that call is correct. Every
    // arena writer in this file cleans to PoC after writing (the poison fill included), so at this point
    // there are no dirty CPU lines over the arena and the two primitives are equivalent. The plain
    // invalidate is chosen only because it cannot become wrong if that invariant is ever broken by a
    // future writer that forgets its clean: a clean pass would then write CPU-stale bytes back over
    // GPU-written ones, destroying exactly the evidence this sweep exists to find.
    // PI-V3D-58: did the bin engine write ANY byte the GPU can reach? Set from the poison scans below;
    // stays false when the poison battery is disarmed, and the `[v3d58] xengine` verdict is written so
    // that a false reading is never over-claimed (it names the retire verdict as the primary column).
    let mut bin_wrote_any = false;
    if V3D56_POISON {
        mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_FLUSH);
        let flush_done =
            wait_bit_clear(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS, "L2T write-back (v3d56 poison scan)");
        mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
        dsb();
        cache::invalidate_range(arena_phys(), ARENA_BYTES);
        dsb();
        let ts_scan = v3d56_scan(ts, TILE_STATE_BYTES);
        let pool_scan = v3d56_scan(tile_alloc, BIN_TILEALLOC_BYTES);
        v3d56_emit_scan("v3d40 PROBE post-bin", "tile-state", ts, &ts_scan, flush_done);
        v3d56_emit_scan("v3d40 PROBE post-bin", "tile-alloc", tile_alloc, &pool_scan, flush_done);
        // Cross-read the pool scan against BPCA: BPCA's advance is a claim about how far the PTB's
        // write pointer moved; the poison says how far it actually WROTE. Naming the two side by side
        // is what settles the phantom question — see docs/dev/OS/08_VIDEO/v3d.md §30 for the
        // BPCA-semantics finding this line has to be read against.
        let bpca = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA);
        let adv = bpca.wrapping_sub(tile_alloc);
        let touched_bytes = if pool_scan.last < 0 { 0 } else { (pool_scan.last as u32 + 1) * 4 };
        serial_println!(
            ":: V3D: [v3d56] bpca-vs-bytes (v3d40 PROBE post-bin) — BPCA={:#010x} pool_base={:#010x} advance={:#x} ({} B) | Mesa v3d_tile_alloc_sizes predicts {:#x} for an EMPTY bin on this 1x1-tile frame (align(1*1*1*128,4096)+8192) — match={} | poison says touched through byte {:#x} ({} B) | delta={} — {} ::",
            bpca, tile_alloc, adv, adv,
            V3D56_EXPECTED_EMPTY_BPCA_ADVANCE,
            (adv == V3D56_EXPECTED_EMPTY_BPCA_ADVANCE) as u32,
            touched_bytes, touched_bytes,
            adv as i64 - touched_bytes as i64,
            if pool_scan.zeroed + pool_scan.overwritten == 0 && adv == V3D56_EXPECTED_EMPTY_BPCA_ADVANCE {
                "PHANTOM VERDICT RETRACTED. BPCA advanced by EXACTLY the reservation the Mesa formula predicts for an empty bin, over a pool the poison proves the PTB never touched. BPCA is the pool ALLOCATION pointer (VC4/v42 ARG: 'Current Address Of Binning Memory Pool'), not a bytes-written counter — reservation moves it and writes nothing. There are no phantom bytes because there were never any bytes; the address question does NOT reopen and the MMU evidence stands. The defect is FLDONE generation on an empty frame — see [v3d56] int"
            } else if pool_scan.zeroed + pool_scan.overwritten == 0 && adv != 0 {
                "BPCA advanced over a pool the PTB provably never touched (poison fully intact), but NOT by the predicted reservation size. Reservation-without-write is still the leading reading — BPCA is an allocation pointer, so an advance is not evidence of a write — yet the size mismatch means our tile geometry or the programmed initial block size differs from what the formula assumed; re-derive tiles_x/tiles_y and the TILE_BINNING_MODE_CFG block size before drawing a conclusion"
            } else if adv >= touched_bytes && touched_bytes != 0 {
                "the PTB both advanced AND wrote, with the advance at or ahead of the last touched byte — consistent, unremarkable pointer behaviour. BPCA is NOT a phantom; the bytes were findable all along and the missing step is frame-close"
            } else if touched_bytes != 0 {
                "touched bytes extend BEYOND the BPCA advance — the PTB wrote further than its reported pointer. THIS is a genuine pointer/address inconsistency and the phantom line stays open"
            } else {
                "BPCA at the pool base with the poison intact: the PTB neither moved nor wrote. Nothing was reserved and nothing was emitted — the bin frame did not begin"
            }
        );
        let arena_post = v3d56_arena_digest();
        v3d56_emit_landing("v3d40 PROBE post-bin", &arena_pre, &arena_post);
        // PI-V3D-58: "the bin engine wrote something" for the cross-engine line. Either poisoned region
        // showing a ZEROED or OVERWRITTEN word is a PTB store; the poison classes are what make that a
        // real observation rather than an inference from an all-zero readback (see §30 Item 1).
        bin_wrote_any = ts_scan.zeroed + ts_scan.overwritten + pool_scan.zeroed + pool_scan.overwritten > 0;
    }
    // ── [v3d58] the cross-engine asymmetry + the post-bin render control ─────────────────────────────
    // `xengine` states, with numbers, the fact that has been sitting in every capture since M3 started
    // passing: a RENDER frame on this block opens, stores to arena memory and retires, while a BIN frame
    // on the same block with the same MMU table, L2T config, clock and arena consumes its list and writes
    // nothing. That refutes every global-write-failure hypothesis by demonstration.
    v3d58_xengine("v3d40 PROBE post-bin", fldone_retired, bin_wrote_any);
    // NOTE: the `[v3d58] rerender` control does NOT belong here. It runs a full CT1 job, and everything
    // below still reads bin state: `ptb_frame_witness` diffs RFC against an `rfc_pre` latched before the
    // CT0 GO (a control render would inflate that delta), the MMU fault latch read below feeds the
    // `[v3d28]` verdict (a fault from the control job's store would be attributed to the bin), and the
    // V3D-28 TMU drain wants no intervening L2T traffic between bin idle and its flush. It is called at
    // the very END of this function instead — see the call after `clear_mmu_fault_latch`.
    // [v3d41] the decisive discriminator: with the pool + tile-state proven zero above and the coord
    // shader proven to have RUN (v3d35), did START_TILE_BINNING actually complete a PTB bin FRAME? BFC's
    // pre→post delta answers "started vs never-started"; BPCA (the PTB write pointer vs the pool base)
    // corroborates whether any primitive-list bytes were emitted at all.
    ptb_frame_witness("v3d41 PROBE post-bin", bfc_pre, rfc_pre, tile_alloc);
    let mmu_ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault = mmu_ctl
        & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);

    // THE V3D-28 FIX: the TMU store lands in the GPU's L2T cache, not DRAM. `tmuwt` in the shader only
    // waits for the TMU to accept the write into L2T — it does NOT write L2T back to DRAM. V3D-27 flushed
    // L2T only PRE-kick, so the CPU read DRAM and always saw the untouched 0x55555555 sentinel (that is the
    // whole "probe-inconclusive" wall). Flush L2T (write-back + invalidate) AFTER the bin idles so the
    // store reaches DRAM, THEN invalidate the CPU's stale copy and read back.
    invalidate_gpu_caches("L2T write-back (probe post-bin — drain the TMU store to DRAM)");
    // Read back the four TMU-stored words + scan the canary tail. Invalidate the CPU's stale copy first.
    cache::clean_invalidate_range(arena_phys() + OFF_PROBE_SCRATCH, PROBE_CANARY_BYTES);
    let w0 = probe_word(OFF_PROBE_SCRATCH);
    let w1 = probe_word(OFF_PROBE_SCRATCH + 4);
    let w2 = probe_word(OFF_PROBE_SCRATCH + 8);
    let w3 = probe_word(OFF_PROBE_SCRATCH + 12);
    // Canary scan: did any tail word (4..PROBE_CANARY_WORDS) flip off its 0xCA00_00NN seed? That means the
    // store landed at the WRONG address inside the page. Report the first disturbed slot.
    let mut canary_hit_idx: i32 = -1;
    let mut canary_hit_val: u32 = 0;
    for i in 4..PROBE_CANARY_WORDS {
        let v = probe_word(OFF_PROBE_SCRATCH + i * 4);
        if v != (0xCA00_0000 | i as u32) {
            canary_hit_idx = i as i32;
            canary_hit_val = v;
            break;
        }
    }

    // Expected: whichever vertex won the (offset-0) store race. All three carry Zc=0.5 (0x3F000000) and
    // Wc=1.0 (0x3F800000); Xc/Yc ∈ {±0.6 = 0x3F19999A / 0xBF19999A, 0.0}. Discriminate three ways.
    const SENT: u32 = 0x5555_5555;
    let untouched = w0 == SENT && w1 == SENT && w2 == SENT && w3 == SENT;
    let all_zero = w0 == 0 && w1 == 0 && w2 == 0 && w3 == 0;
    let is_coord = |v: u32| v == 0x3F19_999A || v == 0xBF19_999A || v == 0x0000_0000;
    let real_coords = is_coord(w0) && is_coord(w1) && w2 == 0x3F00_0000 && w3 == 0x3F80_0000;

    serial_println!(
        ":: V3D: [v3d28] probe idled={} MMU_fault={:#x} (post-bin L2T flushed) — loaded attr v=({:#010x},{:#010x},{:#010x},{:#010x}) expect Zc=0x3f000000 Wc=0x3f800000 Xc/Yc∈{{0x3f19999a,0xbf19999a,0x00000000}} ::",
        idled as u32, fault, w0, w1, w2, w3
    );
    if canary_hit_idx >= 0 {
        serial_println!(
            ":: V3D: [v3d28] CANARY DISTURBED — tail word[{}] = {:#010x} (seed {:#010x}): the TMU store landed at the WRONG address (page-relative +{:#x}), not the UBO_ADDR target. The tmuau/UBO_ADDR value or its offset is off. ::",
            canary_hit_idx, canary_hit_val, 0xCA00_0000u32 | canary_hit_idx as u32, canary_hit_idx * 4
        );
    } else {
        serial_println!(
            ":: V3D: [v3d28] canary window intact (words 4..{}) — no wrong-address landing inside the scratch page ::",
            PROBE_CANARY_WORDS
        );
    }
    if untouched {
        if canary_hit_idx >= 0 {
            serial_println!(
                ":: V3D: [v3d28] VERDICT store-landed-elsewhere — target untouched but a canary flipped: the store issued and drained to DRAM but at the wrong address. Fix the UBO_ADDR uniform (u4) / tmuau offset; the TMU path itself works. NOT a VCD verdict. ::"
            );
        } else {
            serial_println!(
                ":: V3D: [v3d28] VERDICT store-never-issued — target sentinel AND all canaries intact after the post-bin L2T flush: no TMU write drained anywhere in the page. Suspect the coord shader never reached `mov tmuau` (thrsw/thread-end drain) or the tmud/tmuau config. NOT a VCD verdict. ::"
            );
        }
    } else if all_zero {
        serial_println!(
            ":: V3D: [v3d28] VERDICT MISMATCH (loaded-zeros) — store landed but attributes are ZERO: the VCD did NOT DMA the vertex buffer into VPM. Attribute-fetch is the empty-bin wall — audit the attribute-record base/stride/enable + VCD setup against Mesa for THIS draw. ::"
        );
    } else if real_coords {
        serial_println!(
            ":: V3D: [v3d28] VERDICT MATCH (real coords) — the VCD delivered the attributes intact. Attribute fetch is EXONERATED; the wall moves downstream to primitive assembly / VCM → PTB handoff. ::"
        );
    } else {
        serial_println!(
            ":: V3D: [v3d28] VERDICT MISMATCH (unexpected pattern) — store landed at the target with non-zero, non-vertex data: partial DMA, wrong stride/base, or a store race artifact. Compare the words above against OFF_VTXDATA byte-for-byte. ::"
        );
    }
    // PI-V3D-34: pre-arm the SAW-NOTHING branch. If the TMU-issue PCTR battery reads all-zero next boot,
    // the block-state dump below decides TMU-block-wide-disabled vs probe-specific — GMP first (silent
    // drop with a clean MMU latch), plus MISCCFG/L2T/SLC and the record-carries-no-TMU-config fact.
    // Read-only; runs regardless of the probe verdict so the config context is always captured.
    tmu_gmp_block_state_witness("v3d34 post-probe");
    // Leave the M4 bin registers untouched here — triangle_job reprograms QMA/QMS/QTS/QBA/QEA and
    // re-arms the PCTR counters for the real bin below.
    clear_mmu_fault_latch("v3d28 post-probe");
    // ── [v3d58] the post-bin render control — LAST, after every bin readback in this function ────────
    // Placed here and nowhere earlier: it kicks a real CT1 job, so it must not run before `[v3d41]`
    // (whose RFC delta is measured against a pre-GO snapshot), before the MMU fault read that feeds
    // `[v3d28]`, or before the V3D-28 post-bin L2T drain. Every bin-state readback in `probe_job` —
    // `[v3d54]`, `[v3d55]`, `[v3d56]`, `[v3d41]`, `[v3d34]` and `[v3d28]` — is now complete and captured
    // before this control perturbs a single register.
    v3d58_rerender_control("v3d40 PROBE post-bin");
}

/// PI-V3D-48 — build the NULL coord-shader record at OFF_BISECT_NULL_SHADREC. Structurally identical to
/// `build_shader_record` (same VPM segment sizes, clip enable, one attribute) but the CS/VS/FS code
/// address fields all select OFF_BISECT_NULL_CODE — the exonerated 4-word Mesa thread-end tail (vpmwt →
/// nop;thrsw → nop → nop), which writes NOTHING to VPM. Used by the `PrimsNullShader` rung: prims present,
/// a real dispatching thread, but no coordinate output — isolating the primitive-walk/dispatch handshake
/// from the specific 6-word transform of the real coord shader. Uniform streams reuse the M4 streams (the
/// null program pops no uniforms, so the addresses are inert placeholders the record still must carry).
fn build_bisect_null_shader_record() {
    let nullc = (arena_phys() + OFF_BISECT_NULL_CODE) as u64;
    let defaults = (arena_phys() + OFF_DEFAULT_ATTRS) as u64;
    let vtx = (arena_phys() + OFF_VTXDATA) as u64;
    let fs_unif = (arena_phys() + OFF_FS_UNIF) as u64;
    let vs_unif = (arena_phys() + OFF_VS_UNIF) as u64;
    let cs_unif = (arena_phys() + OFF_CS_UNIF) as u64;

    let mut rec = [0u8; 36];
    sf(&mut rec, 1, 1, 1); // Enable clipping
    sf(&mut rec, 24, 8, 0); // Number of varyings in Fragment Shader
    sf(&mut rec, 32, 4, 1); // Coord Shader output VPM segment size
    sf(&mut rec, 40, 4, 0); // Coord Shader input VPM segment size
    sf(&mut rec, 48, 4, 1); // Vertex Shader output VPM segment size
    sf(&mut rec, 56, 4, 0); // Vertex Shader input VPM segment size
    sf(&mut rec, 64, 32, defaults); // Address of default attribute values
    sf(&mut rec, 96, 1, 1); // FS 4-way threadable
    sf(&mut rec, 98, 1, 1); // FS propagate NaNs (v42)
    sf(&mut rec, 99, 29, nullc >> 3); // FS code address → NULL program
    sf(&mut rec, 128, 32, fs_unif); // FS uniforms address (inert)
    sf(&mut rec, 160, 1, 1); // VS 4-way threadable
    sf(&mut rec, 162, 1, 1); // VS propagate NaNs (v42)
    sf(&mut rec, 163, 29, nullc >> 3); // VS code address → NULL program
    sf(&mut rec, 192, 32, vs_unif); // VS uniforms address (inert)
    sf(&mut rec, 224, 1, 1); // CS 4-way threadable
    sf(&mut rec, 226, 1, 1); // CS propagate NaNs (v42)
    sf(&mut rec, 227, 29, nullc >> 3); // CS code address → NULL program
    sf(&mut rec, 256, 32, cs_unif); // CS uniforms address (inert)
    arena_write_bytes(OFF_BISECT_NULL_SHADREC, &rec);

    // One attribute record (vec4 position, f32) — same as the real record so VERTEX_ARRAY_PRIMS has an
    // attribute array to reference (the null program never reads it).
    let mut attr = [0u8; 16];
    sf(&mut attr, 0, 32, vtx); // Address
    sf(&mut attr, 32, 2, 3); // Vec size (4 components)
    sf(&mut attr, 34, 3, 2); // Type = Attribute float
    sf(&mut attr, 40, 4, 4); // Values read by Coordinate shader
    sf(&mut attr, 44, 4, 4); // Values read by Vertex shader
    sf(&mut attr, 64, 32, 16); // Stride (bytes per vertex)
    sf(&mut attr, 96, 32, 0xFFFF); // Maximum Index
    arena_write_bytes(OFF_BISECT_NULL_SHADREC + 36, &attr);
}

/// PI-V3D-48 — submit ONE bisection rung and witness its frame-level retire. Mirrors the probe/M4 bin
/// kick byte-for-byte (same QMA/QMS/QTS setup, same pre-armed BPOA/BPOS overflow block, same INT clear,
/// same FLDONE wait) but binds the CL to `content`'s truncated packet set. Emits the discriminating
/// `[v3d48] <rung>` line — retired, BFC delta, and the full PCS decode — and, on timeout, the retained
/// `[v3d44/45/46]` wedge suite fires from `wait_fldone`. Diagnostic-only; never gates M4.
fn submit_bisect_rung(rung: &str, content: BinContent, shadrec_off: usize) {
    submit_bisect_rung_tagged("v3d48", rung, content, shadrec_off);
}

/// As `submit_bisect_rung`, but with a caller-chosen witness `tag` on the verdict line — so the
/// PI-V3D-50 empty-after-fix re-run (post-reset-cycle) can print under `[v3d50]` while the bisection
/// ladder keeps its `[v3d48]` tag.
fn submit_bisect_rung_tagged(tag: &str, rung: &str, content: BinContent, shadrec_off: usize) {
    submit_bisect_rung_at(tag, rung, content, shadrec_off, OFF_PROBE_BIN_CL, false);
}

/// As `submit_bisect_rung_tagged`, but with the CL's arena offset chosen by the caller and an optional
/// PI-V3D-63 counter bank armed across the kick. `cl_off` is what makes the V3D-63 RANK 2 placement
/// differential possible: the identical `content` is published at two offsets, so a `CT0LC` that mirrors
/// the list's base separates from a `CT0LC` that counts items. When `bank` is set, the V3D-63 sweep bank
/// (see `V3D63_SWEEP_SRC`) is armed immediately before the GO and read at wait-exit — the counters then
/// span exactly this frame. Returns the readback so the caller can compare rungs against each other.
/// With `bank` clear the return still carries the rung's identity and its `retired` verdict (the RANK 2
/// words and the counters read zero and must not be cited) — `None` means fail-closed before the kick,
/// and only that. See the PI-V3D-66 note at the `else` branch below.
fn submit_bisect_rung_at(
    tag: &str,
    rung: &str,
    content: BinContent,
    shadrec_off: usize,
    cl_off: usize,
    bank: bool,
) -> Option<V3d63Rung> {
    submit_bisect_rung_geom(tag, rung, content, shadrec_off, cl_off, bank, None)
}

/// PI-V3D-68 — `submit_bisect_rung_at` with the frame GEOMETRY parameterised.
///
/// `geom == None` is the pre-V3D-68 rung, byte for byte: a `TARGET_W`x`TARGET_H` frame, the 256 B
/// tile-state array at `OFF_TILESTATE`, and both bin regions ZERO-filled before the kick. Every
/// `[v3d48]`/`[v3d50]`/`[v3d51]`/`[v3d53]`/`[v3d63]`/`[v3d66]` caller passes `None` and is unaffected.
///
/// `geom == Some(g)` is the ladder rung and changes exactly three coupled things, all of them the one
/// variable: (a) `TILE_BINNING_MODE_CFG`'s width/height; (b) the tile-state array, which MUST scale with
/// the tile count or the frame is mis-programmed rather than merely re-shaped — so the rung is handed
/// the dedicated `OFF_V3D68_TSDA` region, sized to the whole 24 KiB regardless of the rung so an
/// under-provisioned TSDA can never be the confound; (c) the packing witness's expected width/height.
/// It also POISONS the tile-state array and the tile-alloc pool instead of zeroing them ([v3d56]'s
/// index-encoding seed), because "the PTB wrote zeros" and "the PTB wrote nothing" are different answers
/// and a zero-filled region cannot tell them apart.
fn submit_bisect_rung_geom(
    tag: &str,
    rung: &str,
    content: BinContent,
    shadrec_off: usize,
    cl_off: usize,
    bank: bool,
    geom: Option<V3d68Geom>,
) -> Option<V3d63Rung> {
    let (fb_w, fb_h) = match geom {
        Some(g) => (g.w, g.h),
        None => (TARGET_W, TARGET_H),
    };
    let exp_tiles = match geom {
        Some(g) => g.tiles(),
        None => TILE_STATE_TILES,
    };
    let (ts_off, ts_bytes) = match geom {
        Some(_) => (OFF_V3D68_TSDA, V3D68_TSDA_BYTES),
        None => (OFF_TILESTATE, TILE_STATE_BYTES),
    };
    let bin_len = build_bin_cl_content_geom(cl_off, shadrec_off, 1, content, fb_w, fb_h);

    // Fresh, coherent bin scratch + overflow pool for this rung (reuses the M4 / probe regions).
    if geom.is_some() {
        // PI-V3D-68: poison, not zero — see the doc comment above. `v3d56_poison_region` cleans to PoC.
        v3d56_poison_region(ts_off, ts_bytes);
        v3d56_poison_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    } else {
        fill_region(OFF_TILESTATE, TILE_STATE_BYTES, 0);
        fill_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES, 0);
        cache::clean_range(arena_phys() + OFF_TILESTATE, TILE_STATE_BYTES);
        cache::clean_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    }
    for i in (0..PROBE_BIN_OVERFLOW_BYTES).step_by(4) {
        arena_write_u32(OFF_PROBE_BIN_OVERFLOW + i, 0);
    }
    cache::clean_range(arena_phys() + OFF_PROBE_BIN_OVERFLOW, PROBE_BIN_OVERFLOW_BYTES);
    cache::clean_range(arena_phys() + cl_off, bin_len);

    let bin_ba = (arena_phys() + cl_off) as u32;
    let bin_ea = bin_ba + bin_len as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let ts = (arena_phys() + ts_off) as u32;
    if !arena_contains(bin_ba as usize, bin_len)
        || !arena_contains(tile_alloc as usize, BIN_TILEALLOC_BYTES)
        || !arena_contains(ts as usize, ts_bytes)
    {
        serial_println!(":: V3D: [{}] {} — range escapes the arena, skipping (fail-closed) ::", tag, rung);
        return None;
    }
    // Decode the rung's CL packet-by-packet so the log shows exactly which packets this rung submitted.
    decode_cl_packets("v3d48 bisect", cl_off, bin_len);
    // PI-V3D-57: the same list again, read back from the published bytes and packing-checked field by
    // field against the audited v42 encoding (see the packing-consistency note on v3d57_cl_mesa_diff).
    v3d57_cl_mesa_diff_geom("v3d48 bisect", cl_off, bin_len, fb_w, fb_h, exp_tiles);

    clear_mmu_fault_latch("v3d48 bisect pre-kick");
    // PI-V3D-57: kernel-exact ORDER — BPOS=0 before the invalidate and before the CT0 tile-memory latch.
    bin_prejob_bpos_clear(rung);
    // PI-V3D-53: the `v3d53`-tagged rung runs the KERNEL-EXACT per-job input invalidate
    // (`v3d_invalidate_caches`: slices-first → L2T window re-established → FLM=CLEAR) instead of our
    // `invalidate_gpu_caches` FLM=FLUSH, and witnesses L2TCACTL before/after. This is the derived next
    // candidate after the TMUWCF drain was refuted for the bin path (post-render clean only). Every other
    // rung keeps FLM=FLUSH, so the v3d51(FLUSH) vs v3d53(CLEAR) empty rungs are a controlled differential:
    // if v3d53 retires while v3d51 wedges, the pre-job invalidate mode/sequence was the wall; if both wedge,
    // cache sequencing is exonerated and the break sits below all mirror-able L2TCACTL state.
    if tag == "v3d53" {
        let (l2t_before, l2t_after) =
            bin_prejob_invalidate_kernel_exact("L2T CLEAR (v3d53 bisect pre-kick)");
        serial_println!(
            ":: V3D: [v3d53] {} kernel-exact pre-job invalidate — L2TCACTL {:#010x}->{:#010x} (FLM: ours=FLUSH(0) -> kernel=CLEAR(1); SLCACTL-first + L2TFLSTA=0/L2TFLEND=~0 re-established) — mirrors v3d_invalidate_caches byte-for-byte; TMUWCF(bit8) NOT armed (post-render clean only, not bin path) ::",
            rung, l2t_before, l2t_after,
        );
    } else {
        invalidate_gpu_caches("L2T flush (v3d48 bisect pre-kick)");
    }
    let qts_val = ts | V3D_CLE_CT0QTS_ENABLE;
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, tile_alloc);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, BIN_TILEALLOC_BYTES as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, qts_val);
    dsb();
    // PI-V3D-49: decode the frame-level enables the brief named for the "Empty does NOT retire" verdict,
    // echoed back from the CLE after the writes latch. CT0QTS = tile-STATE base | ENABLE(BIT1); the
    // kernel's `v3d_bin_job_run` writes exactly `V3D_CLE_CT0QTS_ENABLE(BIT1) | qts`, so our composed
    // value is byte-identical (base 32-byte-aligned, ENABLE at bit1 — NOT bit0). CT0QMS is the raw
    // tile-alloc pool SIZE in bytes (job->qms), not an end address or block count. Both are echoed so
    // P46 confirms the CLE latched them.
    let qts_echo = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QTS);
    let qms_echo = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMS);
    serial_println!(
        ":: V3D: [v3d49] {} frame enables — CT0QTS wrote={:#010x} echo={:#010x} (base={:#010x} ENABLE(bit1)={}) | CT0QMS wrote={:#x} echo={:#x} (raw bytes) — QTS ENABLE is bit1 per v3d_regs.h, base|0x2 is kernel-exact ::",
        rung, qts_val, qts_echo, qts_val & !V3D_CLE_CT0QTS_ENABLE,
        (qts_echo & V3D_CLE_CT0QTS_ENABLE != 0) as u32,
        BIN_TILEALLOC_BYTES as u32, qms_echo,
    );
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, bin_ba);
    dsb();
    // PI-V3D-49 fixed the VALUE (BPOS=0; the kernel never pre-arms BPOA/BPOS before the GO — overflow is
    // armed lazily on OUTOMEM in `v3d_overflow_mem_work`). PI-V3D-57 fixed the POSITION: the clear is now
    // issued at the top of this kick, before the invalidate and the QMA/QMS/QTS latch, as the kernel does.
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
    dsb();
    let bfc_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    // PI-V3D-63 (RANK 1): arm the sweep bank LAST before the GO, so the counters span this frame and
    // nothing else. Writes only PCTR config registers — the same registers `pctr_setup_cs_witness`
    // already writes for the coord-shader witness; no CLE/PTB state is touched.
    if bank {
        v3d63_pctr_arm();
    }
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, bin_ea); // GO
    dsb();
    // PI-V3D-54 (RANK 2): audit the latched queue registers vs the intended [BA,EA) + built length BEFORE
    // the retire-wait — the whole empty-frame premise turns on the CLE having been handed the list we
    // built. EA==BA / a wrong span means the "empty did not retire" verdict is a submission artifact.
    let submit_sound = v3d54_submit_audit(tag, bin_ba, bin_ea, bin_len);
    let idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CT1CS_CTRUN, "CT0 v3d48 bisect");
    // PI-V3D-54 (RANK 1): trace the CL progression across this rung's retire-wait.
    let (sts, us, retired) = wait_fldone("v3d48 bisect", bin_ba, bin_ea);
    // PI-V3D-63: stop + read the sweep bank at wait-exit, BEFORE the post-kick latch clear and before any
    // richer readback perturbs the block. The CT0/PCS words below are the RANK 2 placement witness.
    let v3d63 = if bank {
        Some(V3d63Rung {
            rung_off: cl_off as u32,
            bin_ba,
            bin_len: bin_len as u32,
            retired,
            int_sts: sts, // WITSWEEP: pre-W1C sample from wait_fldone — see V3d63Rung
            ct0lc: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0LC),
            ct0pc: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0PC),
            ct0ca: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA),
            ct0cs: mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS),
            pcs: mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS),
            ctr: v3d63_pctr_read(),
        })
    } else {
        // PI-V3D-66: when `bank` is clear the caller may have armed its OWN counter bank around this
        // kick (the widened four-id sweep does exactly that), so this function must neither arm nor
        // read the PCTR here — but the caller still needs to know whether the rung KICKED and whether
        // it RETIRED. The unbanked variant therefore returns the rung's identity plus `retired`, with
        // the RANK 2 register witnesses left at zero and an all-zero `ctr`. **No extra MMIO is issued
        // on the unbanked path**, so the [v3d48]/[v3d50]/[v3d51]/[v3d53] ladder rungs behave and print
        // exactly as they did before this arc. `None` still means, and only means, "fail-closed before
        // the kick" — never "did not retire".
        Some(V3d63Rung {
            rung_off: cl_off as u32,
            bin_ba,
            bin_len: bin_len as u32,
            retired,
            int_sts: sts, // WITSWEEP: pre-W1C sample from wait_fldone — no extra MMIO issued
            ct0lc: 0,
            ct0pc: 0,
            ct0ca: 0,
            ct0cs: 0,
            pcs: 0,
            ctr: V3d63Ctr::default(),
        })
    };
    let bfc_after = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    let pcs = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
    // The empty-frame verdict is the discriminator the whole arc turns on; every richer rung's verdict is
    // read relative to it (see the [v3d48] header for the decision tree).
    let verdict = if retired {
        match content {
            BinContent::Empty => "empty frame RETIRED — the frame-level handshake WORKS; the wedge enters with the state/prims rungs below",
            BinContent::StateNoPrims => "state-but-no-prims RETIRED — fixed-function state + GL_SHADER_STATE are innocent; watch the prims rung",
            BinContent::PrimsNullShader => "prims-null-shader RETIRED — the primitive walk + dispatch handshake works with a no-output shader; the wedge is the real coord shader's VPM output / VCM→PTB handoff",
            BinContent::Full => "full draw RETIRED",
        }
    } else if content == BinContent::Empty {
        "empty frame did NOT retire — the frame handshake itself never worked; the frame-level enables (CT0QTS/QMS/CT1-order §23) and the reset cycle (§24) are audited byte-exact, and V3D-51 established the L2T flush window (L2TFLSTA=0/L2TFLEND=~0, the [v3d51] init-hw-state step) — if it still wedges the break is below the CLE→PTB FLDONE generation; see the [v3d44/45/46] wedge dump above"
    } else {
        "did NOT retire — this rung's added packet class is the offending one (the rung below it retired); localise there"
    };
    serial_println!(
        ":: V3D: [{}] {} — retired={} BFC {:#010x}->{:#010x} (Δ{}) PCS={:#010x}(BMACTIVE={} BMBUSY={} RMACTIVE={} RMBUSY={} BMOOM={}) idled={} INT_STS={:#010x} waited={}us — {} ::",
        tag, rung, retired as u32, bfc_pre, bfc_after, bfc_after.wrapping_sub(bfc_pre),
        pcs,
        (pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (pcs & V3D_PCS_BMBUSY != 0) as u32,
        (pcs & V3D_PCS_RMACTIVE != 0) as u32,
        (pcs & V3D_PCS_RMBUSY != 0) as u32,
        (pcs & V3D_PCS_BMOOM != 0) as u32,
        idled as u32, sts, us, verdict
    );
    // PI-V3D-54 (RANK 2): the FIX-and-re-run leg. If the submission audit above proved the CLE was handed
    // a mis-latched list (EA==BA / wrong span) AND this is an Empty rung that did NOT retire, the non-retire
    // is a submission artifact, not a frame-close fact. Re-latch the queue registers with strict fencing and
    // re-GO ONCE in the same boot, then re-audit + re-trace under the `[v3d54] resubmit` tag. If the re-latch
    // reads sound and the rung now retires, the wedge WAS the submission (retract the empty-frame premise);
    // if EA==BA persists through a clean re-latch, the defect is upstream of the queue write (the GO itself),
    // still a submission fact, not a frame-close one. Metal-gated: dormant on QEMU raspi4b (no V3D block, so
    // `submit_sound` cannot be observed false there) and a strict no-op whenever the first submission is sound.
    if !submit_sound && content == BinContent::Empty && !retired {
        serial_println!(
            ":: V3D: [v3d54] resubmit ({}) — first submission was UNSOUND and the empty rung did not retire; re-latching CT0QMA/QMS/QTS/QBA with strict fencing and re-issuing the GO to decide submission-artifact vs GO-itself ::",
            tag
        );
        let qts_val = ts | V3D_CLE_CT0QTS_ENABLE;
        // PI-V3D-57: kernel-exact ORDER — the overflow clear leads the re-latch, as in `v3d_bin_job_run`.
        bin_prejob_bpos_clear("v3d54 resubmit");
        mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, tile_alloc);
        dsb();
        mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, BIN_TILEALLOC_BYTES as u32);
        dsb();
        mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, qts_val);
        dsb();
        mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, bin_ba);
        dsb();
        mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
        dsb();
        let bfc_pre2 = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
        mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, bin_ea); // re-GO
        dsb();
        let sound2 = v3d54_submit_audit("v3d54 resubmit", bin_ba, bin_ea, bin_len);
        let idled2 = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CT1CS_CTRUN, "CT0 v3d54 resubmit");
        let (sts2, us2, retired2) = wait_fldone("v3d54 resubmit", bin_ba, bin_ea);
        let bfc_after2 = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
        serial_println!(
            ":: V3D: [v3d54] resubmit ({}) — re-latch sound={} retired={} BFC {:#010x}->{:#010x} (Δ{}) idled={} INT_STS={:#010x} waited={}us — {} ::",
            tag, sound2 as u32, retired2 as u32, bfc_pre2, bfc_after2, bfc_after2.wrapping_sub(bfc_pre2),
            idled2 as u32, sts2, us2,
            if sound2 && retired2 {
                "the re-latch was SOUND and the empty rung RETIRED — the original wedge WAS the submission; retract the empty-frame non-retire premise"
            } else if sound2 {
                "the re-latch read SOUND yet the empty rung STILL did not retire — the queue registers were not the wall; a genuine frame-close fact stands"
            } else {
                "EA==BA / wrong span persisted through a strictly-fenced re-latch — the defect is upstream of the queue write (the GO path), still a submission fact"
            }
        );
    }
    clear_mmu_fault_latch("v3d48 bisect post-kick");
    v3d63
}

/// PI-V3D-48 — the empty-frame bisection ladder. Runs a sequence of increasingly-populated bin frames on
/// CT0, each with the SAME QMA/QMS/QTS/BPOA setup + FLDONE wait as the real draw, so ONE metal boot walks
/// the offending packet class down to a single rung:
///
///   1. `Empty`           — config + START + FLUSH, zero state, zero prims. The discriminating experiment:
///                          per the kernel `v3d_bin_job_run` this MUST retire (FLDONE + BFC++). If it does
///                          not, the frame handshake itself is broken — the [v3d44/45/46] wedge dump names
///                          which frame-level enable (and every per-packet audit was measuring the wrong
///                          layer). If it DOES, the frame handshake is sound and the wedge is in the rungs.
///   2. `StateNoPrims`    — + full fixed-function state + GL_SHADER_STATE, no VERTEX_ARRAY_PRIMS.
///   3. `PrimsNullShader` — + VERTEX_ARRAY_PRIMS with a NULL (no-output) coord shader.
///
/// The real M4 draw (full state + prims + the real coord shader) runs AFTER, unchanged — so the ladder
/// brackets the M4 bin from below. Diagnostic-only: reuses the M4 bin-scratch regions (re-zeroed here and
/// again by triangle_job before the real kick), touches no real-draw CL, never gates M4. QEMU raspi4b
/// models no V3D block, so the ladder is dormant there — P45 metal reads the decision tree.
///
/// V3D-DEEP: the ladder is the single most expensive diagnostic in this file — six rungs, each ending in
/// a ~0.5 s FLDONE anti-hang backstop that always times out (~3.0 s of wall clock), plus a full CL decode
/// + Mesa diff per rung on the serial line. All six verdicts are banked on metal (every rung wedges,
/// including `Empty`), so re-walking the ladder on every armed boot buys nothing and is most of the
/// visible boot stall. Gated behind `v3d_deep`; the caller prints what was skipped.
fn empty_frame_bisection() {
    if !V3D_DEEP {
        return;
    }
    serial_println!(
        ":: V3D: [v3d48] empty-frame bisection — walk the wedge down: Empty (config+START+FLUSH) → StateNoPrims → PrimsNullShader, each with the M4 QMA/QMS/QTS/BPOA setup + FLDONE wait. Empty MUST retire (kernel v3d_bin_job_run); if it does, the frame handshake is sound and the offending packet is the first rung that stops retiring ::"
    );
    // Publish the NULL coord shader (the exonerated 4-word Mesa thread-end tail — CS_VS_WORDS[23..27],
    // vpmwt → nop;thrsw → nop → nop) + its record once.
    const NULL_CS_WORDS: [u64; 4] = [
        CS_VS_WORDS[23], CS_VS_WORDS[24], CS_VS_WORDS[25], CS_VS_WORDS[26],
    ];
    let null_len = write_shader_words(OFF_BISECT_NULL_CODE, &NULL_CS_WORDS);
    build_bisect_null_shader_record();
    cache::clean_range(arena_phys() + OFF_BISECT_NULL_CODE, null_len);
    cache::clean_range(arena_phys() + OFF_BISECT_NULL_SHADREC, 36 + 16);
    // OFF_SHADREC (the real M4 record) was already published + cleaned by probe_job; StateNoPrims reuses it.
    cs_tail_witness("v3d48 NULL", OFF_BISECT_NULL_CODE, NULL_CS_WORDS.len());

    // PI-V3D-52 (Rung 2): audit the v42 TILE_BINNING_MODE_CFG for a tile-state auto-init bit the Empty
    // rung might leave clear (contingency hypothesis) — finding: no such bit in v42, config is complete.
    audit_bin_mode_cfg_autoinit();
    // PI-V3D-53 (Rung 3 verdict): the TMU write-combiner drain candidate V3D-52 staged is REFUTED for the
    // bin path — TMUWCF is a post-render clean-caches op, never run for bin jobs. Record the sourced verdict
    // (not armed). The derived next candidate — the kernel-exact FLM=CLEAR pre-job invalidate — runs as the
    // [v3d53] empty rung below.
    refute_tmuwcf_drain_candidate();

    // PI-V3D-50: re-run the EMPTY rung first, tagged [v3d50], as the direct before/after witness for the
    // new kernel-faithful reset cycle (`v3d_reset_cycle` ran in bring-up this boot). If the OFF→ON core
    // reset unwedged the frame handshake, THIS is where it shows: `retired=1 BFC Δ1 PCS=…BMACTIVE=0`,
    // retiring the whole seven-layer empty-bin investigation. If it still reads `retired=0 BFC Δ0
    // BMACTIVE=1`, the reset cycle was not the wall and the wedge sits below the CLE→PTB FLDONE generation.
    submit_bisect_rung_tagged("v3d50", "empty-after-fix", BinContent::Empty, OFF_SHADREC);
    // PI-V3D-51: re-run the EMPTY rung again, tagged [v3d51], as the direct before/after witness for the
    // missing post-reset core-init (`v3d_init_hw_state` established L2TFLSTA=0/L2TFLEND=~0 in bring-up this
    // boot — the L2T flush window the per-kick FLM=FLUSH walks). If establishing that window unwedged the
    // frame flush/write-back, THIS reads `retired=1 BFC Δ1 PCS=…BMACTIVE=0`, retiring the empty-bin
    // investigation as an un-init'd L2T flush range. If it still reads `retired=0 BFC Δ0 BMACTIVE=1` with
    // the [v3d51] init-hw-state witness confirming the window latched, the wedge sits below the L2T flush
    // range too and the retained [v3d44/45/46] dump names the next layer.
    submit_bisect_rung_tagged("v3d51", "empty-after-init-hw-state", BinContent::Empty, OFF_SHADREC);
    // PI-V3D-53: the EMPTY rung once more, tagged [v3d53], but with the KERNEL-EXACT per-job input
    // invalidate (`v3d_invalidate_caches`: SLCACTL-first → L2T window re-established → FLM=CLEAR) in place of
    // our FLM=FLUSH — the last L2TCACTL flush-mode/sequence divergence around a bin job, and the derived
    // candidate after the TMUWCF drain was refuted for the bin path. Read as a differential against the
    // v3d51 empty rung directly above (identical except FLM mode/sequence): if THIS retires while v3d51
    // wedged, the pre-job invalidate was the wall; if both wedge, the wedge is confirmed below all
    // mirror-able L2TCACTL state and the retained [v3d44/45/46] dump names the next layer.
    submit_bisect_rung_tagged("v3d53", "empty-after-clear-invalidate", BinContent::Empty, OFF_SHADREC);
    // PI-V3D-72: arm the clock-liveness bank across THE canonical [v3d48] Empty rung — the exact wait
    // whose P82 clkliv line read "counter 2 was NOT enabled". Arm → kick (no PCTR MMIO on the unbanked
    // rung path, so the bank spans the whole CTRUN + FLDONE wait) → read/stop/emit. Every other ladder
    // rung is untouched and prints exactly as before.
    v3d72_pctr_arm("v3d48 empty-frame");
    submit_bisect_rung("empty-frame", BinContent::Empty, OFF_SHADREC);
    v3d72_pctr_read_emit("v3d48 empty-frame");
    submit_bisect_rung("state-no-prims", BinContent::StateNoPrims, OFF_SHADREC);
    submit_bisect_rung("prims-null-shader", BinContent::PrimsNullShader, OFF_BISECT_NULL_SHADREC);
    // PI-V3D-63: the two PTB-unit discriminators ride the tail of the ladder — same boot, same block
    // state, and after every banked rung above has printed.
    v3d63_ptb_unit_probes();
    // PI-V3D-64: the mux calibration rides the same tail, AFTER v3d63 — it reuses that arc's bank and
    // its banked-rung submitter, and its L2 leg runs a CT1 job, so it must be last for the same reason
    // `[v3d58] rerender` is last in `probe_job`: a retiring render frame perturbs register/cache state
    // that every bin readback above is taken on.
    v3d64_mux_calibration();
    // PI-V3D-66: the widened sweep rides the very tail, AFTER v3d64 — same reasoning as v3d64's own
    // placement (its legs run CT1 jobs and banked bin rungs, so every bin readback above must already
    // have been taken), and it must follow v3d64 specifically because its verdict is read against the
    // classes v3d64/v3d65 established on this same boot.
    v3d66_ptb_sweep();
    // PI-V3D-68: the frame-geometry ladder rides the very tail, after every banked verdict and every
    // calibration leg above has been taken. It is placed last for two reasons beyond the standing one
    // (its rungs perturb the block): it is the ONLY battery that re-points CT0QTS at a different
    // tile-state region and that publishes POISON rather than zeros into the shared bin regions, so
    // every reading taken above is taken on the arrangement those verdicts were banked under.
    v3d68_binner_wall();
    // PI-V3D-69: the initialisation diff rides the very tail, after [v3d68]. Its pre-station is meant
    // to read the SoC↔V3D fabric as it stood through the WHOLE battery above, so it wants every other
    // rung already behind it; and its own control rung must not sit in front of a reading another
    // verdict is banked on.
    v3d69_fabric_probe();
    // PI-V3D-71: the dump-diff discriminator rides the very tail, after [v3d69]'s stations — it is the
    // ONLY battery that touches the V3D page table (window map/unmap around its one rung) and the only
    // one that latches non-identity iovas into CT0Q*, so every reading above is taken on the exact
    // translation those verdicts were banked under; its in-flight ASB sampler must also not overlap
    // any wait another witness times.
    v3d71_mainline_geometry();
    // PI-V3D-73 leg 3: the mainline-exact submit rides LAST — after [v3d71] has unmapped its
    // window (this rung's identity addresses must run on the translation every banked verdict was
    // taken under) and after every other reading has been taken on the instrumented-submit
    // arrangement those verdicts were banked under.
    v3d73_mainline_submit();
    // PI-V3D-74: the thread-swap discriminator rides after [v3d73s] — its leg A latches CT0Q*
    // with an RCL-class queue, so every bin-class reading above must already be banked; and its
    // verdict is read AGAINST [v3d73]'s loaded-but-never-fetches fact from this same boot.
    v3d74_thread_swap();
    // PI-V3D-75: the fabric-condition experiments ride LAST — [v3d74a]'s thread-0-never-starts
    // verdict must already be banked on THIS boot's untouched fabric before either experiment
    // (firmware ENABLE_QPU, then the M_CTRL=0x4040 write) mutates it.
    v3d75_fabric_condition();
    // PI-V3D-76: the full-window sweep rides dead-last — the wedged-state register file, in the
    // bench dump's exact line format, for a whole-file diff against the piOS --sweep capture.
    v3d76_sweep();
    // PI-V3D-77: the unnamed-word transplants ride after the sweep (so the sweep records the
    // pre-transplant file) — the diff's two config-shaped divergences, poked and kicked.
    v3d77_unnamed_words();
    // PI-V3D-80: the display handover rides ABSOLUTELY LAST — NOTIFY_DISPLAY_DONE stops the
    // firmware display driver, so scanout of the firmware framebuffer may die the moment it runs.
    // Every screen-dependent reading this boot owns must already be banked.
    // PI-V3D-81: unless the reply-less display leg is armed, in which case this rung STANDS DOWN and
    // [v3d81d] below owns the one send. Sending the tag here first would mean [v3d81d]'s pre-station
    // reads a firmware that has already been told to hand over, and nothing after it could be
    // attributed to the reply-less send. Stood down loudly, never silently: the wire says so.
    #[cfg(not(feature = "v3d81_display"))]
    v3d80_display_done();
    #[cfg(feature = "v3d81_display")]
    serial_println!(
        ":: V3D: [v3d80] STOOD DOWN — UNAOS_V3D81_DISPLAY is armed, so NOTIFY_DISPLAY_DONE is sent EXACTLY ONCE this boot, reply-less, in [v3d81d]. Nothing was sent here and no [v3d80] verdict is implied ::"
    );
    // PI-V3D-81: the reply-less legs ride after [v3d80] — dead last in the file. Their display leg
    // can stop the firmware display driver, and their whole point is to be read against the
    // doorbell-waiting sends above, so every one of those must already have printed.
    #[cfg(feature = "v3d81")]
    v3d81_replyless_notify();
}

/// PI-V3D-80 — the display handover. P90/P91 closed the ARM-side space: identical firmware
/// binaries (hash), identical config lines, genpd-faithful bring-up — still thread-0-never-starts.
/// The one remaining mailbox act the working system performs that we never have is vc4's
/// probe-time `NOTIFY_DISPLAY_DONE` (0x00030066, zero-length): the firmware display driver stops
/// and the ARM owns the display/GPU complex. Hypothesis: the VPU withholds V3D thread-start until
/// that claim. The rung sends it, re-reads the [v3d77] status words (a change there = the VPU
/// reacted), and runs the [v3d75] kick probe. ⚠ Panel may go black from this rung on — by design.
///
/// PI-V3D-81: not compiled when the reply-less display leg is armed. The tag must be sent at most
/// once per boot or no reading can be attributed to a send, and when [v3d81d] is present it owns
/// the send. Cfg'd out rather than skipped at runtime so the two can never both fire.
#[cfg(not(feature = "v3d81_display"))]
fn v3d80_display_done() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down | V3dPresence::Poison(_) => {
            serial_println!(":: V3D: [v3d80] SKIPPED — hub absent/poison. No handover sent ::");
            return;
        }
    }
    let c0 = mmio_read(V3D_HUB_BASE, 0x4068);
    serial_println!(
        ":: V3D: [v3d80] display-done handover — sending NOTIFY_DISPLAY_DONE (0x00030066, vc4's probe act; the firmware display driver stops — the panel may go BLACK from here, serial carries the verdict). core+0x68 pre={:#010x} ::",
        c0
    );
    match mailbox::notify_display_done() {
        Some(()) => serial_println!(":: V3D: [v3d80] NOTIFY_DISPLAY_DONE — mailbox OK ::"),
        None => {
            serial_println!(":: V3D: [v3d80] NOTIFY_DISPLAY_DONE — MAILBOX FAILED — no handover happened; the kick below is a null control ::");
        }
    }
    settle_ms(5);
    let c1 = mmio_read(V3D_HUB_BASE, 0x4068);
    serial_println!(
        ":: V3D: [v3d80] core+0x68 after handover: {:#010x} (pre {:#010x}, working-part ref 0x00000003) — {} ::",
        c1, c0,
        if c1 != c0 { "THE VPU REACTED — the status word moved on the handover" } else { "unchanged" }
    );
    let ok = v3d75_kick_probe("v3d80 post-handover");
    serial_println!(
        ":: V3D: [v3d80] verdict — kick={} — {} ::",
        ok as u32,
        if ok { "THE WALL WAS THE HANDOVER: thread 0 starts once the ARM claims the display/GPU complex (NOTIFY_DISPLAY_DONE). Productionize: send it in bringup before first submit, and take over scanout ourselves" }
        else { "handover did not free thread 0 — the last documented mailbox act is exonerated; the ARM-testable universe is CLOSED (v3d.md §46.5) and what remains is VPU-internal state requiring firmware-side instrumentation" }
    );
}

/// PI-V3D-81 — the settle interval a reply-less post waits before it reads the effect, in ms.
/// `UNAOS_V3D81_SETTLE_MS`, decimal; absent/zero/garbage => 250 ms, capped at 10 s so a typo cannot
/// turn the anti-hang rule off. It is a knob because the right value is unknown: it must be long
/// enough for a firmware that stops its whole display driver to finish doing so, and short enough
/// that a boot is still a boot. Whatever it is, the wire states it — an instrument that needs the
/// reader to remember the build is not an instrument.
#[cfg(feature = "v3d81")]
const V3D81_SETTLE_MS: u64 = v3d81_settle_ms();

#[cfg(feature = "v3d81")]
const fn v3d81_settle_ms() -> u64 {
    let b = match option_env!("UNAOS_V3D81_SETTLE_MS") {
        Some(s) => s.as_bytes(),
        None => return 250,
    };
    let mut v: u64 = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] < b'0' || b[i] > b'9' {
            return 250;
        }
        v = v * 10 + (b[i] - b'0') as u64;
        i += 1;
    }
    if v == 0 {
        250
    } else if v > 10_000 {
        10_000
    } else {
        v
    }
}

/// PI-V3D-81 — one reading of everything a reply-less send could plausibly move, taken as a unit so
/// a before/after pair is comparable field by field. Registers first and firmware second, because
/// the register reads are free and must not be taken on the far side of a property call that might
/// itself perturb something.
#[cfg(feature = "v3d81")]
#[derive(Clone, Copy)]
struct V3d81Station {
    /// core0+0x68 — the wedge's visible SIGNATURE (§46.3): 0x10001 on every wedged boot, 0x3 on a
    /// piOS part whose thread 0 fetches. Read-only (P88), which is exactly why it is a good witness.
    c68: u32,
    /// hub+0x68 — its unnamed companion, same provenance, same read-only verdict.
    h68: u32,
    m_ctrl: u32,
    s_ctrl: u32,
    /// CT0CS — 0x70 wedged vs 0x20 while genuinely fetching. Between a pre- and a post-station only
    /// the send happens, so movement here is signal and not job residue.
    ct0cs: u32,
    /// `GET_CLOCK_RATE(V3D)`, raw. Doubles as the POSITIVE CONTROL: an answer here proves the
    /// mailbox transport and our cache invalidate are both alive at this instant, which is what
    /// makes an untouched reply buffer a fact about the firmware instead of a fact about us.
    fw_rate: Option<u32>,
    /// `GET_CLOCK_STATE(V3D)` — bit0 active, bit1 not-present. An ENABLE_QPU that actually ran the
    /// firmware's V3D init has a plausible reason to move this even if no register does.
    fw_gate: Option<u32>,
    /// `GET_PHYS_WH`, raw (0x0 preserved, not folded into a failure). The display-driver liveness
    /// read: the machine-readable stand-in for P93's attended panel.
    fw_wh: Option<(u32, u32)>,
}

#[cfg(feature = "v3d81")]
fn v3d81_station() -> V3d81Station {
    let c68 = mmio_read(V3D_HUB_BASE, 0x4068);
    let h68 = mmio_read(V3D_HUB_BASE, 0x0068);
    let m_ctrl = mmio_read(RPIVID_ASB_BASE, ASB_V3D_M_CTRL);
    let s_ctrl = mmio_read(RPIVID_ASB_BASE, ASB_V3D_S_CTRL);
    let ct0cs = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    // A late doorbell from the reply-less post would be consumed by the first query below as if it
    // were that query's own reply — over a buffer the query has already overwritten. Drain first.
    mailbox::drain_property_fifo();
    let fw_rate = mailbox::get_clock_rate_raw(mailbox::CLOCK_ID_V3D);
    let fw_gate = mailbox::get_clock_state(mailbox::CLOCK_ID_V3D);
    let fw_wh = mailbox::query_display_size_raw();
    V3d81Station { c68, h68, m_ctrl, s_ctrl, ct0cs, fw_rate, fw_gate, fw_wh }
}

#[cfg(feature = "v3d81")]
fn v3d81_print_station(tag: &str, when: &str, st: &V3d81Station) {
    serial_println!(
        ":: V3D: [{}] station {} — core+0x68={:#010x} (wedged ref 0x00010001, fetching ref 0x00000003) hub+0x68={:#010x} RPIVID M_CTRL={:#010x} S_CTRL={:#010x} CT0CS={:#010x} | firmware: V3D rate={:?} gate={:?} display_wh={:?} ::",
        tag, when, st.c68, st.h68, st.m_ctrl, st.s_ctrl, st.ct0cs, st.fw_rate, st.fw_gate, st.fw_wh
    );
}

/// Did anything the ARM can see move across the send? Every field participates: the point is not
/// which one moved but whether the VPU reacted AT ALL, because a single moved field converts a dead
/// kick from "no information" into "acted, and the effect is elsewhere".
#[cfg(feature = "v3d81")]
fn v3d81_moved(pre: &V3d81Station, post: &V3d81Station) -> bool {
    pre.c68 != post.c68
        || pre.h68 != post.h68
        || pre.m_ctrl != post.m_ctrl
        || pre.s_ctrl != post.s_ctrl
        || pre.ct0cs != post.ct0cs
        || pre.fw_rate != post.fw_rate
        || pre.fw_gate != post.fw_gate
        || pre.fw_wh != post.fw_wh
}

/// PI-V3D-81 — one reply-less leg: station, send, settle, station, kick, verdict. `display_leg`
/// adds the display-driver liveness reading, which only means something for the handover tag.
#[cfg(feature = "v3d81")]
fn v3d81_leg(
    tag: &str,
    tag_id: u32,
    name: &str,
    kick_label: &str,
    display_leg: bool,
    send: fn(u64) -> mailbox::NoReplySend,
) {
    let pre = v3d81_station();
    v3d81_print_station(tag, "PRE-send", &pre);
    serial_println!(
        ":: V3D: [{}] sending {} ({:#010x}) REPLY-LESS — post the tag, wait for NO doorbell, settle {} ms, then read the EFFECT. Reply-lessness is itself the claim under test (v3d.md §46.5): a doorbell arriving inside the settle refutes it for this tag ::",
        tag, name, tag_id, V3D81_SETTLE_MS
    );
    let r = send(V3D81_SETTLE_MS);
    serial_println!(
        ":: V3D: [{}] send — posted={} settle={}us (asked {}ms) | doorbells={} first={}us other-ch={} stale-pre={} | buffer@{:#x}: overall={:#010x} tag_code={:#010x} word5={:#010x} -> {} ::",
        tag, r.posted as u32, r.settle_us, V3D81_SETTLE_MS, r.doorbells, r.first_doorbell_us,
        r.other_ch, r.stale_pre, r.buf_pa, r.overall, r.tag_code, r.word5,
        if !r.posted {
            "NOT SENT — the write FIFO never accepted the message. Nothing below carries any reading"
        } else if r.doorbells > 0 {
            "A REPLY CAME — this tag is NOT reply-less at this settle. §46.5's protocol reading does not cover it, the ordinary mbox_call path can carry it, and P92/P93's timeouts want re-explaining"
        } else if r.buffer_written() {
            "REPLY-LESS AND ACTED — no doorbell, yet the VPU stamped its response into our buffer: the message was parsed and THE TAG WAS HANDLED. This is the fact P92/P93 could not see"
        } else {
            "no doorbell and an untouched buffer — the tag left no trace in the protocol itself; the station diff and the control below are what decide ignored-vs-acted-elsewhere"
        }
    );
    let post = v3d81_station();
    v3d81_print_station(tag, "POST-send", &post);
    let moved = v3d81_moved(&pre, &post);
    // The control. `fw_rate` answering AFTER the send proves the transport and the invalidate were
    // both working when we read the buffer — without it, "the buffer was untouched" is a statement
    // about our reader, not about the firmware, and the whole leg is uninterpretable.
    let control = post.fw_rate.is_some();
    if display_leg {
        serial_println!(
            ":: V3D: [{}] display-liveness — GET_PHYS_WH pre={:?} post={:?} | GET_CLOCK_RATE(V3D) pre={:?} post={:?} — {} ::",
            tag, pre.fw_wh, post.fw_wh, pre.fw_rate, post.fw_rate,
            if post.fw_wh != pre.fw_wh && control {
                "THE FIRMWARE DISPLAY DRIVER STOPPED — the display query moved while an unrelated clock query still answers, so this is the display side specifically. The handover HAPPENED, read on the wire, with no panel and no attended observer"
            } else if control {
                "the firmware still reports the same display mode with the mailbox alive — its display driver did NOT stop. Combined with an untouched buffer this is the IGNORED reading P93 could only infer from panel survival"
            } else {
                "both queries degraded — a stopped display driver and a stopped mailbox are not separable from here. The register channel above still stands; the panel (attended) breaks this particular tie"
            }
        );
    }
    let kick = v3d75_kick_probe(kick_label);
    serial_println!(
        ":: V3D: [{}] verdict — kick={} acted-on-buffer={} doorbell={} station-moved={} control-alive={} — {} ::",
        tag, kick as u32, r.buffer_written() as u32, (r.doorbells > 0) as u32, moved as u32,
        control as u32,
        if !r.posted {
            "NO SEND — instrument failure, no verdict. Fix before citing"
        } else if kick {
            "THE WALL WAS THIS TAG — thread 0 starts once it is sent reply-less. Productionize the send in bringup before first submit (and take over whatever the firmware just stopped owning)"
        } else if r.buffer_written() || moved {
            "ACTED, EFFECT ELSEWHERE — the firmware demonstrably processed the tag and thread 0 STILL never starts. This is a REFUTATION of the hypothesis, not a null: bank it and take the tag off the open list"
        } else if control {
            "IGNORED — the control answers, so the reader was alive and the firmware left no trace in any channel: on this firmware the tag does nothing an ARM can see, and the hypothesis stays UNTESTED rather than refuted (exactly P93's position, now stated from evidence instead of from panel survival)"
        } else {
            "INCONCLUSIVE — the control did not answer, so an untouched buffer cannot be attributed to the firmware. Re-run. If the control fails ONLY on the display leg, that failure is itself evidence the display driver stopped"
        }
    );
}

/// PI-V3D-81 — the reply-less NOTIFY battery: the one open thread in a closed campaign.
///
/// §46.5's discovery is a protocol fact with a methodological consequence. On this firmware a
/// NOTIFY-class tag is acted on WITHOUT a FIFO doorbell, so `mbox_call` — which returns only when a
/// doorbell arrives — reports failure for a tag the VPU may have honoured perfectly. Two hypotheses
/// were therefore never tested, only mis-read: [v3d80]'s `NOTIFY_DISPLAY_DONE` (P92 at 500 ms, P93
/// at 5 s, both "MAILBOX FAILED") and [v3d75a]'s `SET_ENABLE_QPU` (P86, recorded as "tag unhandled
/// by this firmware"). Each is sent here without waiting for a reply, and read by its EFFECT.
///
/// ### The discrimination this rung exists to make
/// P93's lesson is that the panel SURVIVING proved only that the tag was ignored — a null result,
/// requiring an attended human, and unable to tell "nothing happened" from "something happened
/// somewhere I am not looking". Three channels replace it, all machine-readable on the wire:
///
///   1. **The reply buffer.** The VPU writes its response into our request buffer by DMA; the
///      doorbell is a separate mechanism. `overall = 0x8000_0000`, or `tag_code` bit 31 set, means
///      the message was parsed and THE TAG WAS HANDLED — acted, independent of any effect. Nobody
///      read those words in P92/P93: the buffer was posted and abandoned when the wait timed out.
///   2. **The registers.** core+0x68 (the wedge signature), hub+0x68, both RPIVID ASB bridge words,
///      CT0CS — pre and post, bracketing only the send. Any movement = the VPU reacted.
///   3. **The firmware's own state.** V3D clock rate and gate, and the display mode, asked before
///      and after. For the handover tag the last of these is the point: a firmware that stopped its
///      display driver should stop answering (or answer differently) about DISPLAY geometry while
///      still answering about CLOCKS. That asymmetry is the panel observation, made in software.
///
/// And the control that makes a null reading mean anything: `GET_CLOCK_RATE` after the send, on a
/// tag with nothing to do with either hypothesis. If it answers, the transport and our cache
/// invalidate were both alive when we read the buffer, so an untouched buffer is the FIRMWARE's
/// silence. If it does not, the leg is INCONCLUSIVE and says so rather than inventing a verdict.
///
/// ### Ignored vs acted-with-the-effect-elsewhere, stated plainly
/// IGNORED = no doorbell AND an untouched buffer AND no station movement, WITH the control
/// answering. ACTED = any one of those channels moves. The kick decides only the hypothesis itself;
/// the other three decide whether a dead kick is a REFUTATION (acted, thread 0 still dead — the tag
/// comes off the list) or merely another NULL (ignored — the tag remains untested and the next
/// instrument must reach the firmware some other way). That distinction is the whole arc.
///
/// Each leg is separately armable (`UNAOS_V3D81_QPU`, `UNAOS_V3D81_DISPLAY`) because the two are
/// independent hypotheses, and a boot that sends both cannot attribute a change to either. The
/// armed set, the settle, and the tag ids all ride on the wire, so a capture is datable and
/// self-describing without the reader knowing how it was built. When a leg is armed, the
/// doorbell-waiting send of the same tag above ([v3d75a], [v3d80]) stands down — one send per tag
/// per boot, or the pre-station is a lie.
///
/// ⚠ The display leg may take the panel to black permanently. That is the hypothesis, not a fault.
#[cfg(feature = "v3d81")]
fn v3d81_replyless_notify() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down | V3dPresence::Poison(_) => {
            serial_println!(
                ":: V3D: [v3d81] SKIPPED — hub absent/poison (QEMU raspi4b or block-down). Nothing was sent and NO verdict is implied ::"
            );
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d81] reply-less NOTIFY battery — ARMED LEGS qpu={} display={} | settle={}ms | tags SET_ENABLE_QPU=0x00030012 NOTIFY_DISPLAY_DONE=0x00030066 | each armed leg is the ONLY send of its tag on this boot (the doorbell-waiting rung above stands down); each leg reads the reply BUFFER, the register station and the firmware's own state, with GET_CLOCK_RATE as the positive control ::",
        cfg!(feature = "v3d81_qpu") as u32,
        cfg!(feature = "v3d81_display") as u32,
        V3D81_SETTLE_MS
    );

    // Leg Q first: it cannot take the panel away, so its readings are taken while the display is
    // still whatever the firmware made it. Leg D can end the boot's video and therefore rides last.
    #[cfg(feature = "v3d81_qpu")]
    v3d81_leg(
        "v3d81q",
        0x0003_0012,
        "SET_ENABLE_QPU(1)",
        "v3d81q post-enable-qpu",
        false,
        |ms| mailbox::enable_qpu_noreply(1, ms),
    );
    #[cfg(not(feature = "v3d81_qpu"))]
    serial_println!(
        ":: V3D: [v3d81q] NOT ARMED — no reply-less SET_ENABLE_QPU on this boot. [v3d75a] above sent it the doorbell-waiting way and its reading (whatever it says) stands unchanged ::"
    );

    #[cfg(feature = "v3d81_display")]
    {
        serial_println!(
            ":: V3D: [v3d81d] ⚠ the next send may stop the firmware display driver — the panel may go BLACK from here and not come back. Serial is unaffected and carries the whole verdict; every screen-dependent reading this boot owns is already banked above ::"
        );
        v3d81_leg(
            "v3d81d",
            0x0003_0066,
            "NOTIFY_DISPLAY_DONE",
            "v3d81d post-handover",
            true,
            mailbox::notify_display_done_noreply,
        );
    }
    #[cfg(not(feature = "v3d81_display"))]
    serial_println!(
        ":: V3D: [v3d81d] NOT ARMED — no reply-less NOTIFY_DISPLAY_DONE on this boot. [v3d80] above sent it the doorbell-waiting way, which is the send P92/P93 proved cannot see its own effect ::"
    );
}

/// PI-V3D-77 — the sweep diff's verdict, acted on. diff(P87 [v3d76] wedged sweep, piOS mid-bin2
/// --sweep) filtered to structural (non-address, non-counter) divergences leaves exactly TWO
/// words, both UNNAMED — absent from mainline v3d_regs.h (which ends the interrupt bank at
/// 0x64/INT_MSK_CLR) and from every map this campaign transcribed:
///
///   core0+0x068: piOS 0x00000003, ours 0x00010001   hub+0x068: piOS 0x00000002, ours 0x00000000
///
/// No driver writes them, so the piOS values are the FIRMWARE/hardware state of a V3D whose
/// thread 0 fetches, and ours is the state of one whose thread 0 never starts. Ours carrying
/// bit16 (0x10001) where the working part shows 0x3 is exactly the shape of a latched per-thread
/// condition — the space [v3d74a] said the hunt must move to. Two transplants, readback-audited
/// (a W1C or RO word shows itself in the readback), each followed by the [v3d75] kick probe:
/// retire = THE WALL WAS THE UNNAMED WORD; no-hold/no-retire = recorded and exonerated in turn.
fn v3d77_unnamed_words() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down | V3dPresence::Poison(_) => {
            serial_println!(":: V3D: [v3d77] SKIPPED — hub absent/poison. No experiment ran ::");
            return;
        }
    }
    let c0 = mmio_read(V3D_HUB_BASE, 0x4068);
    let h0 = mmio_read(V3D_HUB_BASE, 0x0068);
    serial_println!(
        ":: V3D: [v3d77] unnamed-words — station: core0+0x68={:#010x} (piOS ref 0x00000003) hub+0x68={:#010x} (piOS ref 0x00000002) — both absent from v3d_regs.h; transplant each, readback-audited, kick after each ::",
        c0, h0
    );
    // A: core0+0x68 ← 0x3 (the working part's value; if W1C this clears whatever ours latched).
    mmio_write(V3D_HUB_BASE, 0x4068, 0x0000_0003);
    dsb();
    let c1 = mmio_read(V3D_HUB_BASE, 0x4068);
    serial_println!(
        ":: V3D: [v3d77a] core0+0x68 wrote 0x3, readback {:#010x} ({}) ::",
        c1,
        if c1 == c0 { "unchanged — RO or self-restoring" } else if c1 == 3 { "held" } else { "changed to a third value — W1C-like" }
    );
    let a = v3d75_kick_probe("v3d77a core-unnamed");
    if a {
        serial_println!(":: V3D: [v3d77] verdict — A=1 — THE WALL WAS core0+0x68: thread 0 starts once the unnamed word reads the working value; name it, productionize the write ::");
        return;
    }
    // B: hub+0x68 ← 0x2.
    mmio_write(V3D_HUB_BASE, 0x0068, 0x0000_0002);
    dsb();
    let h1 = mmio_read(V3D_HUB_BASE, 0x0068);
    serial_println!(
        ":: V3D: [v3d77b] hub+0x68 wrote 0x2, readback {:#010x} ({}) ::",
        h1,
        if h1 == h0 { "unchanged — RO or self-restoring" } else if h1 == 2 { "held" } else { "changed to a third value" }
    );
    let b = v3d75_kick_probe("v3d77b hub-unnamed");
    serial_println!(
        ":: V3D: [v3d77] verdict — A(core+0x68)=0 B(hub+0x68)={} — {} ::",
        b as u32,
        if b { "THE WALL WAS hub+0x68 — thread 0 starts with the hub word transplanted; name it, productionize" }
        else { "both unnamed words dead — recorded and exonerated; the remaining diff surface is the hub 0x84-0xA8 counter block (static on ours = no AXI traffic, symptom) and the firmware-side init outside the register file entirely (VPU code path; next space is a start4.elf/config.txt condition diff — dtoverlay=vc4-kms-v3d on/off with OUR kernel)" }
    );
}

/// PI-V3D-76 — the full-window register sweep, wedged state. [v3d75] exonerated the curated
/// divergences (M_CTRL is VPU-owned and its transplant bounced; ENABLE_QPU is unhandled), so the
/// firmware-init diff moves from curated registers to the WHOLE defined file: hub control
/// 0x0000-0x0100, MMUC 0x1000-0x1040, MMU 0x1200-0x1240, core0 0x4000-0x4A00 — defined windows
/// only, no undefined-offset reads. Nonzero words only, in the bench script's exact
/// `SWEEP phys=0x… val=0x…` format, so `diff` against a piOS `--sweep` capture is the instrument.
/// Runs dead-last in the battery: this is the settled WEDGED state, every rung's residue included
/// (which is the point — the piOS side is a settled RENDERING state).
fn v3d76_sweep() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down | V3dPresence::Poison(_) => {
            serial_println!(":: V3D: [v3d76] SKIPPED — hub absent/poison. No sweep ::");
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d76] sweep — nonzero words in hub[0x0,0x100) mmuc[0x1000,0x1040) mmu[0x1200,0x1240) core0[0x4000,0x4A00), wedged state, bench-dump line format ::"
    );
    const WINDOWS: [(usize, usize); 4] =
        [(0x0000, 0x100), (0x1000, 0x40), (0x1200, 0x40), (0x4000, 0xA00)];
    for (start, span) in WINDOWS {
        let mut off = 0;
        while off < span {
            let v = mmio_read(V3D_HUB_BASE, start + off);
            if v != 0 {
                serial_println!("SWEEP phys={:#010x} val={:#010x}", V3D_HUB_BASE + start + off, v);
            }
            off += 4;
        }
    }
    serial_println!(":: V3D: [v3d76] sweep end ::");
}

/// PI-V3D-63 — one banked rung's readback: the RANK 2 placement witness (the CT0/PCS words) plus the
/// RANK 1 counter bank read across the same frame. Compared across rungs by `v3d63_ptb_unit_probes`.
#[derive(Clone, Copy)]
struct V3d63Rung {
    rung_off: u32,  // the CL's ARENA OFFSET — the value a base-mirroring CT0LC would echo
    bin_ba: u32,    // the CL's PHYSICAL base as latched into CT0QBA
    bin_len: u32,   // built list length in bytes
    retired: bool,
    // WITSWEEP: the INT_STS sample `wait_fldone` took BEFORE its retire-path W1C — the only sample
    // that can still show FLDONE (and companions) for a retired rung. Callers must use this, never
    // a post-wait MMIO re-read, which is 0 by construction after the W1C.
    int_sts: u32,
    ct0lc: u32,
    ct0pc: u32,
    ct0ca: u32,
    ct0cs: u32,
    pcs: u32,
    ctr: V3d63Ctr,
}

/// PI-V3D-63 — the eight PCTR outputs of one sweep bank, plus the latched overflow mask. Slots 0..6 are
/// `V3D63_SWEEP_SRC` (UNIDENTIFIED source ids, printed raw); slot 7 is the anchored src32 CYCLE_COUNT
/// control that decides whether the bank counted at all.
#[derive(Clone, Copy, Default)]
struct V3d63Ctr {
    c: [u32; 8],
    ovf: u32,
}

impl V3d63Ctr {
    /// The anchored control — total core cycles across the frame. Zero ⇒ the bank never counted and
    /// every other slot is INCONCLUSIVE, never "clean".
    fn ctrl(&self) -> u32 {
        self.c[V3D63_CTRL_SLOT]
    }
    /// Bitmask over the SWEEP INDEX (bit i = `V3D63_SWEEP_SRC[i]`) of sources that registered events.
    ///
    /// A counter's OVERFLOW bit is ORed in alongside its value: a source that wrapped exactly back to
    /// zero across the frame would otherwise read as "never moved", which is the same silent-instrument
    /// failure the campaign keeps having to unwind. Overflow is a positive statement that the counter
    /// ran past its width, so it counts as movement.
    fn moved(&self) -> u32 {
        let mut m = 0u32;
        let mut i = 0;
        while i < V3D63_SWEEP_SRC.len() {
            let s = v3d63_slot(i);
            if self.c[s] != 0 || self.ovf & (1 << s) != 0 {
                m |= 1 << i;
            }
            i += 1;
        }
        m
    }
}

/// PI-V3D-63 (RANK 1) — arm the sweep bank. Programming order is the exact Linux
/// `v3d_perfmon.c::v3d_perfmon_start` idiom the file already uses in `pctr_setup_cs_witness`, including
/// the PI-V3D-39 SRC read-back that forces the posted source-selects to retire before the counters are
/// cleared and enabled (the mitigation for the non-deterministic source latch). Eight counters: slots
/// 0..6 carry `V3D63_SWEEP_SRC`, slot 7 carries the anchored `PCTR_SRC_CYCLE_COUNT`.
fn v3d63_pctr_arm() {
    // Slots 0,1 = sweep[0,1]; slot 2 = src32 CYCLE_COUNT (RESERVED — see V3D63_CTRL_SLOT); slot 3 =
    // sweep[2]. Slots 4..7 = sweep[3..7]. This packing IS `v3d63_slot`, written out in field order.
    let src_0_3 = (V3D63_SWEEP_SRC[0] & 0x7f)
        | ((V3D63_SWEEP_SRC[1] & 0x7f) << 8)
        | ((PCTR_SRC_CYCLE_COUNT & 0x7f) << 16)
        | ((V3D63_SWEEP_SRC[2] & 0x7f) << 24);
    let src_4_7 = (V3D63_SWEEP_SRC[3] & 0x7f)
        | ((V3D63_SWEEP_SRC[4] & 0x7f) << 8)
        | ((V3D63_SWEEP_SRC[5] & 0x7f) << 16)
        | ((V3D63_SWEEP_SRC[6] & 0x7f) << 24);
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, 0); // stop any prior arming before reprogramming
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_0_3, src_0_3);
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_4_7, src_4_7);
    dsb();
    // PI-V3D-39: read the selects back so the posted stores RETIRE and the mux is provably latched.
    let _srclat = mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_0_3)
        | mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_4_7);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_CLR, V3D63_PCTR_MASK); // zero WHILE stopped
    mmio_write(V3D_CORE0_BASE, V3D_PCTR_0_OVERFLOW, V3D63_PCTR_MASK); // clear latched overflow
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, V3D63_PCTR_MASK); // enable LAST, from a clean zero
    dsb();
}

/// PI-V3D-63 (RANK 1) — read the eight counters, latch the overflow mask, and stop the bank.
fn v3d63_pctr_read() -> V3d63Ctr {
    dsb();
    let mut out = V3d63Ctr::default();
    let mut i = 0usize;
    while i < 8 {
        out.c[i] = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 4 * i);
        i += 1;
    }
    out.ovf = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_OVERFLOW);
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, 0); // stop counting
    dsb();
    out
}

// ─── PI-V3D-72 — is the clock even ticking? The PCTR bank armed ACROSS the wedged bin's wait. ────────
//
// The P82 wire named the gap itself: both `[v3d55]` clkliv lines on the final v3d48 rung and the v3d71
// mainline-geom rung printed "counter 2 was NOT enabled across this wait — no clock verdict here". The
// CYCLE_COUNT battery had only ever been armed around the PROBE bin, so the campaign has NEVER measured
// whether the core's clock domain is even ADVANCING while BMACTIVE=1 on our wedged bin — and with every
// fabric/routing/input theory dead ([v3d71f]: the PTB never issues one beat; [v3d71]: still dead under
// mainline geometry with poison intact), a gated internal clock sub-domain is the leading remaining
// theory. PCTR is the one instrument inside the core, so this arc arms it across the arming window and
// nowhere else.
//
// Four slots, every source id one this file has ALREADY anchored (the PI-V3D-33 sourcing rule: ids are
// admitted only against the verified 16/32 anchors; no PTB-family id survives that rule — same verdict
// V3D-63 recorded when it dropped its three named sources — and src35 was explored by v3d67 and is not
// re-litigated here):
//   slot 0 = src14 QPU_ACTIVE_CYCLES_VERTEX_COORD_USER — QPU executing vertex/coord USER shaders
//   slot 1 = src16 QPU_CYCLES_VALID_INSTR              — QPU issuing any valid instruction
//   slot 2 = src32 CYCLE_COUNT                          — the RESERVED [v3d55] slot (file law, see
//            V3D63_CTRL_SLOT): arming it here is what turns the existing clkliv witness from
//            "armed=0, no verdict" into a real Δ across the wedged wait, with zero wait_fldone changes
//   slot 3 = src24 TMU_TCACHE_ACCESS                    — TMU cache traffic (intra-core memory side)
// Together they discriminate "core clocked but binner unit dead" (Δsrc32>0, unit slots 0) from "whole
// core frozen" (Δsrc32=0) from "core unexpectedly ACTIVE" (any unit slot >0) — the reading key is in
// the witness text below and in v3d.md §43.
//
// Scope: PCTR writes are counter-file config — the same registers `pctr_setup_cs_witness` and
// `v3d63_pctr_arm` already write and that mainline's v3d_perfmon.c writes on every perfmon start; no
// CLE/PTB/fabric state is touched, no off-mainline register experimentation. The bank is stopped
// (EN=0, the perfmon-stop idiom) at read, so every rung outside the two arming windows prints exactly
// what it printed before this arc.
const V3D72_PCTR_MASK: u32 = 0x0F; // slots 0..3

/// PI-V3D-72 — arm the four-slot bank. Exact `v3d_perfmon_start` idiom (EN=0 → SRC → read-back to
/// retire the posted selects (PI-V3D-39) → CLR while stopped → OVERFLOW clear → EN last), byte-for-byte
/// the `v3d63_pctr_arm` sequence with this arc's source packing. Called LAST before the GO so the
/// counters span exactly the arming window; the `[v3d72] clkarm` line is the wire proof of the window.
fn v3d72_pctr_arm(what: &str) {
    let src_0_3 = (PCTR_SRC_QPU_ACTIVE_CYCLES_VERTEX_COORD_USER & 0x7f)
        | ((PCTR_SRC_QPU_CYCLES_VALID_INSTR & 0x7f) << 8)
        | ((PCTR_SRC_CYCLE_COUNT & 0x7f) << 16)
        | ((PCTR_SRC_TMU_TCACHE_ACCESS & 0x7f) << 24);
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, 0); // stop any prior arming before reprogramming
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_0_3, src_0_3);
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_4_7, 0);
    dsb();
    // PI-V3D-39: read the selects back so the posted stores RETIRE and the mux is provably latched.
    let _srclat = mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_0_3)
        | mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_4_7);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_CLR, V3D72_PCTR_MASK); // zero WHILE stopped
    mmio_write(V3D_CORE0_BASE, V3D_PCTR_0_OVERFLOW, V3D72_PCTR_MASK); // clear latched overflow
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, V3D72_PCTR_MASK); // enable LAST, from a clean zero
    dsb();
    serial_println!(
        ":: V3D: [v3d72] clkarm ({}) — PCTR bank armed ACROSS this rung's waits (perfmon-start idiom, EN={:#04x}): slot0=src14 QPU_ACTIVE_CYCLES_VERTEX_COORD_USER slot1=src16 QPU_CYCLES_VALID_INSTR slot2=src32 CYCLE_COUNT (the reserved [v3d55] clkliv slot — that witness now carries a REAL verdict on this wait) slot3=src24 TMU_TCACHE_ACCESS — all four cleared to zero while stopped, so each read below IS its Δ across the window; read the verdict on the [v3d72] clkliv-x line after the wait ::",
        what, V3D72_PCTR_MASK,
    );
}

/// PI-V3D-72 — read the four counters at wait-exit, stop the bank (perfmon-stop: EN=0), and emit the
/// Δ line in the `[v3d55]` clkliv idiom: every source printed with id + name + delta, verdict inline.
/// The reading key (also v3d.md §43): Δsrc32=0 ⇒ THE WALL FOUND — the core's own cycle counter is
/// frozen during our bin = gated clock domain; Δsrc32>0 with unit slots 0 ⇒ clock exonerated, the wall
/// is a unit-level enable/state; any unit slot >0 ⇒ the core is ACTIVE, re-read that unit's branch.
fn v3d72_pctr_read_emit(what: &str) {
    dsb();
    let en = mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN);
    let d_qpu_user = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0);
    let d_qpu_instr = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 4);
    let d_cyc = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 8);
    let d_tmu = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 12);
    let ovf = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_OVERFLOW);
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, 0); // perfmon-stop: leave the file as every other rung sees it
    dsb();
    // Overflow counts as movement — a counter that wrapped exactly to zero must not read "never moved"
    // (the same silent-instrument failure class V3d63Ctr::moved guards against).
    let cyc_moved = d_cyc != 0 || ovf & (1 << 2) != 0;
    let qpu_moved = d_qpu_user != 0 || d_qpu_instr != 0 || ovf & 0b0011 != 0;
    let tmu_moved = d_tmu != 0 || ovf & (1 << 3) != 0;
    let verdict = if en & V3D72_PCTR_MASK != V3D72_PCTR_MASK {
        "the bank was NOT fully armed at read (EN lost the mask inside the window) — every Δ on this line is INCONCLUSIVE, not zero; find what reprogrammed the counter file inside the arming window before citing this line"
    } else if !cyc_moved {
        "Δ=0 on src32 CYCLE_COUNT across a wait the wall held open — THE WALL FOUND: the core's own cycle counter is FROZEN while the wedged bin sits with BMACTIVE=1. That is a GATED CLOCK sub-domain, invisible in every config register this campaign has read, and mainline's missing init is a clock/power UNLOCK, not a unit enable. Cross-check this window's [v3d55] clkliv line (same slot 2 at ~1 ms cadence) — the two instruments must agree"
    } else if !qpu_moved && !tmu_moved {
        "src32 CYCLE_COUNT ADVANCED while every unit-side source read zero — the clock branch is EXONERATED on the wedged bin itself: the core is clocked and idle (no QPU issue, no TMU-cache traffic) while the binner never emits. The wall is a UNIT-LEVEL enable/state inside an always-clocked core; the gated-clock theory is dead for every domain this counter file can observe"
    } else {
        "src32 CYCLE_COUNT advanced AND a unit-side source registered events during a wait that never retired — the core is not merely clocked but ACTIVE somewhere; read which Δ moved above (QPU issue vs TMU-cache) and re-open that unit's branch before any clock theory"
    };
    serial_println!(
        ":: V3D: [v3d72] clkliv-x ({}) — PCTR_EN(at read)={:#010x} OVERFLOW={:#010x} | Δ src32 CYCLE_COUNT={} | Δ src14 QPU_ACTIVE_CYCLES_VERTEX_COORD_USER={} | Δ src16 QPU_CYCLES_VALID_INSTR={} | Δ src24 TMU_TCACHE_ACCESS={} (counters cleared at arm: each value IS its Δ across the arming window) — {} ::",
        what, en, ovf, d_cyc, d_qpu_user, d_qpu_instr, d_tmu, verdict,
    );
}

/// PI-V3D-63 — print one banked rung: the RANK 2 placement words and the RANK 1 raw counter bank.
///
/// Every swept counter is printed as `srcNN=value` with NO name attached. That is deliberate and it is
/// the honest form: this tree cannot cross-check those ids against the `enum drm_v3d_perfcnt` anchors
/// (16/32), so it makes no claim about which unit each mux carries. Only slot 2 (src32 CYCLE_COUNT) is
/// named, because only slot 2 is anchored — and slot 2 is also the counter the pre-existing `[v3d55]`
/// clock-liveness witness samples on every wait, which is exactly why the sweep may not occupy it.
fn v3d63_emit_rung(what: &str, r: &V3d63Rung) {
    serial_println!(
        "::   [v3d63] rung {} — CL off={:#x} BA={:#010x} len={} retired={} | CT0LC={:#x} CT0PC={:#x} CT0CA={:#010x} CT0CS={:#010x}(CTRUN={}) PCS={:#010x}(BMACTIVE={} BMBUSY={}) | bank: src{}={} src{}={} src{}={} src{}={} src{}={} src{}={} src{}={} [ALL SEVEN UNIDENTIFIED — raw mux reads, no name claimed] | src32 CYCLE_COUNT={} (control) OVERFLOW={:#010x} ::",
        what, r.rung_off, r.bin_ba, r.bin_len, r.retired as u32,
        r.ct0lc, r.ct0pc, r.ct0ca, r.ct0cs,
        (r.ct0cs & V3D_CLE_CT1CS_CTRUN != 0) as u32,
        r.pcs,
        (r.pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (r.pcs & V3D_PCS_BMBUSY != 0) as u32,
        V3D63_SWEEP_SRC[0], r.ctr.c[v3d63_slot(0)],
        V3D63_SWEEP_SRC[1], r.ctr.c[v3d63_slot(1)],
        V3D63_SWEEP_SRC[2], r.ctr.c[v3d63_slot(2)],
        V3D63_SWEEP_SRC[3], r.ctr.c[v3d63_slot(3)],
        V3D63_SWEEP_SRC[4], r.ctr.c[v3d63_slot(4)],
        V3D63_SWEEP_SRC[5], r.ctr.c[v3d63_slot(5)],
        V3D63_SWEEP_SRC[6], r.ctr.c[v3d63_slot(6)],
        r.ctr.ctrl(), r.ctr.ovf,
    );
    // The [v3d55] clock-liveness line printed inside this rung's wait read THIS bank's slot 2, which is
    // pinned to src32 — so that verdict is sound across V3D-63's rungs and needs no re-reading.
}

/// PI-V3D-63 — the arc's two discriminators, run as one 2×2 matrix on a single boot.
///
/// **RANK 2 `ctrartifact`** — falsify `CT0LC`/`CT0PC` as counters. The campaign's most-cited fact ("the
/// control list executed") rests on `CT0LC` 0→`0x10000` and `CT0PC` 0→3, and `0x10000` is bit-identical
/// to `OFF_BIN_CL`. So publish the SAME `BinContent` at two arena offsets (`OFF_PROBE_BIN_CL` and
/// `OFF_V3D63_BIN_CL_ALT`) and submit both. A base-mirroring `CT0LC` tracks the placement; a counting
/// `CT0LC` does not. The second axis is content/length: an `Empty` rung and a `Full` rung at each
/// offset, so "placement-independent" and "content-dependent" are separated instead of confounded.
/// `CT0PC` read on a rung with literally zero primitives is the independent kill-shot for C2.
///
/// **RANK 1 `ptbctr`** — the same counter bank armed across all four rungs. See the `V3D63_SWEEP_SRC`
/// block for why all three of the brief's NAMED sources are DROPPED and a raw, unlabeled sweep is armed
/// instead. The bank's own control is src32 `CYCLE_COUNT`; if it reads zero the bank never counted and
/// the whole RANK 1 verdict is INCONCLUSIVE, never clean.
///
/// Poison-honest: gated on a live hub-identity read, and prints an explicit SKIPPED line when the block
/// is down. QEMU raspi4b models no V3D block at all and returns at `BLOCK-DOWN` long before this runs,
/// so this probe is dormant there — `kernel8-test` green means NO REGRESSION and nothing more.
fn v3d63_ptb_unit_probes() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: [v3d63] SKIPPED — hub IDENT0 reads 0x00000000 (block absent/unpowered; the QEMU raspi4b signature). Neither ptbctr nor ctrartifact was measured this boot; no verdict is implied ::"
            );
            return;
        }
        V3dPresence::Poison(w) => {
            serial_println!(
                ":: V3D: [v3d63] SKIPPED — hub IDENT0 reads poison {:#010x} (open-bus / firmware fill, not a live register). Neither ptbctr nor ctrartifact was measured this boot; no verdict is implied ::",
                w
            );
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d63] ptb-unit probes — 2x2 matrix (Empty|Full) x (CL@{:#x}|CL@{:#x}), one PCTR sweep bank armed across every rung. RANK 2 reads the placement differential on CT0LC/CT0PC; RANK 1 reads the bank. Each rung carries the same ~0.5 s FLDONE backstop as the ladder above ::",
        OFF_PROBE_BIN_CL, OFF_V3D63_BIN_CL_ALT
    );

    let e1 = submit_bisect_rung_at("v3d63", "empty@base", BinContent::Empty, OFF_SHADREC, OFF_PROBE_BIN_CL, true);
    let e2 = submit_bisect_rung_at("v3d63", "empty@alt", BinContent::Empty, OFF_SHADREC, OFF_V3D63_BIN_CL_ALT, true);
    let f1 = submit_bisect_rung_at("v3d63", "full@base", BinContent::Full, OFF_SHADREC, OFF_PROBE_BIN_CL, true);
    let f2 = submit_bisect_rung_at("v3d63", "full@alt", BinContent::Full, OFF_SHADREC, OFF_V3D63_BIN_CL_ALT, true);

    let (e1, e2, f1, f2) = match (e1, e2, f1, f2) {
        (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
        _ => {
            serial_println!(
                ":: V3D: [v3d63] ctrartifact/ptbctr INCONCLUSIVE — at least one of the four rungs fail-closed before its kick (see the range-escapes line above), so neither the placement differential nor the counter bank has its full matrix. Instrument first; read no verdict off a partial matrix ::"
            );
            return;
        }
    };
    v3d63_emit_rung("empty@base", &e1);
    v3d63_emit_rung("empty@alt", &e2);
    v3d63_emit_rung("full@base", &f1);
    v3d63_emit_rung("full@alt", &f2);

    // ── RANK 2 verdict — is CT0LC a mirror or a counter? ─────────────────────────────────────────────
    let lc_tracks_place = e1.ct0lc != e2.ct0lc || f1.ct0lc != f2.ct0lc;
    let lc_tracks_content = e1.ct0lc != f1.ct0lc;
    let lc_echoes_off = e1.ct0lc == e1.rung_off && e2.ct0lc == e2.rung_off;
    let lc_echoes_ba = e1.ct0lc == e1.bin_ba && e2.ct0lc == e2.bin_ba;
    let lc_echoes_len = e1.ct0lc == e1.bin_len && e2.ct0lc == e2.bin_len;
    let ct_verdict = if lc_echoes_off || lc_echoes_ba {
        "CT0LC READS BACK THE LIST'S OWN ADDRESS at BOTH placements — it is a MIRROR, not a count. C1 (\"the control list executed\") is FALSIFIED on its primary evidence: the whole campaign's 0x10000 reading was OFF_BIN_CL echoed back. Re-aim at the CT0 kick / bin-thread start, and re-read every PTB verdict as taken on a thread that may never have run"
    } else if lc_tracks_place {
        "CT0LC CHANGED with the CL's placement while the content was held fixed — it carries an address-derived term, not a pure item count. C1's evidence is CONTAMINATED at minimum; treat \"the control list executed\" as unproven"
    } else if lc_tracks_content {
        "CT0LC is PLACEMENT-INDEPENDENT and CONTENT-DEPENDENT (same value at both offsets, different between Empty and Full) — it behaves as a genuine list-item counter. C1 STANDS, and now on a controlled reading instead of a coincidental one"
    } else {
        "CT0LC is placement-independent but ALSO content-independent — it reads the same on a zero-packet frame and the full draw. That is not a count of anything this frame contains; C1's evidence is INERT (neither mirror-proven nor counter-proven). Do not cite it either way"
    };
    // CT0PC on a rung with literally zero primitives is C2's independent test.
    let pc_verdict = if e1.ct0pc == 0 && e2.ct0pc == 0 {
        "CT0PC reads 0 on both zero-primitive rungs — consistent with a real primitive count, and it removes the P45 CT0PC=3 reading's ambiguity only if the Full rungs above read nonzero"
    } else {
        "CT0PC reads NONZERO on a frame containing literally zero primitives — CT0PC is NOT a primitive count, and C2 (\"items were fed to the PTB\") loses its last support independently of RANK 1"
    };
    serial_println!(
        ":: V3D: [v3d63] ctrartifact — CT0LC empty {:#x}@{:#x} vs {:#x}@{:#x} | full {:#x}@{:#x} vs {:#x}@{:#x} | echoes-offset={} echoes-BA={} echoes-len={} tracks-placement={} tracks-content={} || CT0PC empty {:#x}/{:#x} full {:#x}/{:#x} (zero-prim rungs read {:#x},{:#x}) | CT0CS CTRUN-at-exit empty {}/{} full {}/{} — {} ; {} ::",
        e1.ct0lc, e1.rung_off, e2.ct0lc, e2.rung_off,
        f1.ct0lc, f1.rung_off, f2.ct0lc, f2.rung_off,
        lc_echoes_off as u32, lc_echoes_ba as u32, lc_echoes_len as u32,
        lc_tracks_place as u32, lc_tracks_content as u32,
        e1.ct0pc, e2.ct0pc, f1.ct0pc, f2.ct0pc, e1.ct0pc, e2.ct0pc,
        (e1.ct0cs & V3D_CLE_CT1CS_CTRUN != 0) as u32,
        (e2.ct0cs & V3D_CLE_CT1CS_CTRUN != 0) as u32,
        (f1.ct0cs & V3D_CLE_CT1CS_CTRUN != 0) as u32,
        (f2.ct0cs & V3D_CLE_CT1CS_CTRUN != 0) as u32,
        ct_verdict, pc_verdict,
    );

    // ── RANK 1 verdict — did the bank count, and did anything move? ──────────────────────────────────
    let ctrl_live = e1.ctr.ctrl() != 0 && e2.ctr.ctrl() != 0 && f1.ctr.ctrl() != 0 && f2.ctr.ctrl() != 0;
    let moved_empty = e1.ctr.moved() | e2.ctr.moved();
    let moved_full = f1.ctr.moved() | f2.ctr.moved();
    let ptb_verdict = if !ctrl_live {
        "INCONCLUSIVE — the bank's own src32 CYCLE_COUNT control read ZERO on at least one rung, so the counter block was not counting and NOTHING here is a measurement. Instrument first: fix the arming before any slot in this bank is read as evidence. This is explicitly NOT a clean result"
    } else if moved_empty == 0 && moved_full == 0 {
        "the block was demonstrably CLOCKED (src32 nonzero on every rung) and NOT ONE swept source registered a single event — on the full draw as well as on the zero-primitive frames. Read with the standing caveat that this tree cannot say what these seven muxes carry: it is evidence that seven unidentified event sources are silent, and it becomes evidence about the PTB/CLE only once a source id is transcribed from `enum drm_v3d_perfcnt` and cross-checked against the 16/32 anchors. That transcription is the next arc's first task"
    } else if moved_full != 0 && moved_empty == 0 {
        "swept sources moved on the FULL draw and stayed ZERO on the zero-primitive frames — a content-dependent signal, which is exactly the signature a genuine primitive/thread counter would show. Identify the moving ids against the uapi enum before naming what they prove; the differential itself is sound regardless"
    } else {
        "at least one swept source moved on a frame containing ZERO primitives — whatever those muxes carry, it is not gated on primitive content. Identify the moving ids before reading anything into it"
    };
    serial_println!(
        ":: V3D: [v3d63] ptbctr — DROPPED (not guessed): CLE-bin-thread-active-cycles, PTB-prims-binned, PTB-prims-clipped — none of the three can be cross-checked against this file's verified anchors (16 valid_instr, 32 cycle_count) and no `enum drm_v3d_perfcnt` copy exists in this tree; naming them would be the PI-V3D-4/6/7 fabricated-constant class. ARMED instead: raw sweep of UNIDENTIFIED source ids {:?} in slots 0,1,3,4,5,6,7 + src32 CYCLE_COUNT pinned to slot 2 (the slot [v3d55] clkliv samples every wait — the sweep may never occupy it) serving as this bank's control | bank-counted(all rungs)={} | src32 empty {}/{} full {}/{} | moved-mask empty={:#04x} full={:#04x} (bit i = V3D63_SWEEP_SRC[i]) | OVERFLOW empty {:#010x}/{:#010x} full {:#010x}/{:#010x} — {} ::",
        V3D63_SWEEP_SRC, ctrl_live as u32,
        e1.ctr.ctrl(), e2.ctr.ctrl(), f1.ctr.ctrl(), f2.ctr.ctrl(),
        moved_empty, moved_full,
        e1.ctr.ovf, e2.ctr.ovf, f1.ctr.ovf, f2.ctr.ovf,
        ptb_verdict,
    );
}

// ─── PI-V3D-64 (`muxcal`) — calibrate the seven unidentified muxes against known ground truth. ───────
//
// V3D-63 armed a raw sweep of seven UNIDENTIFIED PCTR source ids and refused to name them; P65v2 metal
// (capture pi4-r23s1o) came back with, on ZERO-primitive bin rungs, src13 ≈ 161M, src28 ≈ 645M against
// src32 CYCLE_COUNT ≈ 645M, and src1/10/11/12/29 flat at 0. That is a shape, not an identity: src28
// looks 1:1 with the core clock and src13 looks like clock/4, but "looks like" is exactly the reasoning
// that produced the PI-V3D-4/6/7 fabricated constants. This tree still carries no `enum drm_v3d_perfcnt`
// to transcribe from, and naming an id from memory remains BANNED.
//
// The honest identification channel is CALIBRATION: run the SAME bank across legs whose truth we
// already know, and classify each mux by BEHAVIOUR alone.
//
//   * **L1 idle** — a fixed-duration wall-clock window with NO job submitted. A clock-derived mux
//     accumulates here in proportion to wall time; a work-gated one cannot, because there is no work.
//   * **L2 render** — the known-good CT1 clear job (`clear_job`, the M3 job that RETIRES and whose store
//     is byte-verified; the same job `[v3d58] rerender` reuses — this arc invents no new job). Real
//     render work, so render-gated muxes move here and nowhere else.
//   * **L3 bin** — one `Empty` rung of the `[v3d48]` ladder, banked, as the bin-side leg. This is the
//     wedging path: the frame opens and never retires, so a mux gated on bin RETIREMENT stays flat while
//     a clock-derived one keeps climbing through the backstop.
//
// Every slot is reported as a permille RATIO against src32's delta on the same leg, which is the whole
// point: a ratio held constant across three legs of wildly different duration and workload is a strong
// statement about what a mux is derived from, and it is one this file can make WITHOUT a name. The
// verdict assigns one of four BEHAVIOUR CLASSES and claims nothing further:
//
//   `CLOCK-LIKE` / `WORK-GATED-RENDER` / `SILENT` / `OTHER`
//
// **The standing rule this arc establishes:** when a future arc finally transcribes an id from the uapi
// enum, that name must MATCH the class measured here or the transcription is REJECTED. Calibration is
// the check on the transcription, not the other way round.
//
// Writes: PCTR config registers only (the same `v3d63_pctr_arm` idiom). NOT ARMED — no CT*/GMP/MMU_CTL
// write beyond what the reused legs already perform, and no new MMIO window.

/// PI-V3D-64 — the L1 idle window, in milliseconds of wall clock off CNTPCT.
const V3D64_IDLE_MS: u64 = 4;
/// PI-V3D-64 — CLOCK-LIKE tolerance band, in permille of the src32 control (±10% around 1:1).
const V3D64_CLOCK_LO: u64 = 900;
const V3D64_CLOCK_HI: u64 = 1100;

/// PI-V3D-64 — one calibration leg: its label, and the sweep bank read across it (`None` = the leg did
/// not run, which is a reportable state, never a zero).
#[derive(Clone, Copy)]
struct V3d64Leg {
    tag: &'static str,
    ctr: Option<V3d63Ctr>,
}

impl V3d64Leg {
    /// A leg is a MEASUREMENT only if its own src32 control moved. A leg whose control read zero was not
    /// counting, and its slots are INCONCLUSIVE — never "silent".
    fn valid(&self) -> bool {
        match self.ctr {
            Some(c) => c.ctrl() != 0,
            None => false,
        }
    }
    fn raw(&self, i: usize) -> u32 {
        match self.ctr {
            Some(c) => c.c[v3d63_slot(i)],
            None => 0,
        }
    }
    /// Movement includes a latched OVERFLOW: a mux that wrapped exactly to zero must not read as flat.
    fn moved_i(&self, i: usize) -> bool {
        match self.ctr {
            Some(c) => c.c[v3d63_slot(i)] != 0 || c.ovf & (1 << v3d63_slot(i)) != 0,
            None => false,
        }
    }
    /// Did THIS slot latch an overflow on this leg? A wrapped counter's raw u32 is a residue, not a
    /// delta, so its permille is meaningless and must never be band-tested (a mux that wrapped once and
    /// landed inside the ±10% window would otherwise be classified CLOCK-LIKE off a truncated value).
    fn ovf_i(&self, i: usize) -> bool {
        match self.ctr {
            Some(c) => c.ovf & (1 << v3d63_slot(i)) != 0,
            None => false,
        }
    }
    /// This slot as permille of the leg's own src32 delta — the leg-independent quantity. Valid ONLY
    /// when `ovf_i` is clear; see there.
    fn permille(&self, i: usize) -> u64 {
        match self.ctr {
            Some(c) if c.ctrl() != 0 => (self.raw(i) as u64 * 1000) / c.ctrl() as u64,
            _ => 0,
        }
    }
    fn ctrl(&self) -> u32 {
        match self.ctr {
            Some(c) => c.ctrl(),
            None => 0,
        }
    }
    fn ovf(&self) -> u32 {
        match self.ctr {
            Some(c) => c.ovf,
            None => 0,
        }
    }
}

/// PI-V3D-64 — classify one swept source id by BEHAVIOUR across the legs. No name is claimed, and none
/// can be: the return value describes how a mux responded to known work, nothing about what it counts.
fn v3d64_classify(legs: &[V3d64Leg; 3], i: usize) -> &'static str {
    // The caller has already guaranteed at least two valid legs (it prints `muxcal INCONCLUSIVE` and
    // returns otherwise), so there is no under-populated case to handle here.
    let mut any_moved = false;
    let mut clocklike = true;
    let mut wrapped = false;
    let mut k = 0usize;
    while k < legs.len() {
        if legs[k].valid() {
            if legs[k].moved_i(i) {
                any_moved = true;
            }
            // A wrapped slot's raw value is a residue: band-testing it would manufacture a CLOCK-LIKE
            // verdict out of a truncated count. Overflow anywhere disqualifies the ratio outright.
            if legs[k].ovf_i(i) {
                wrapped = true;
                clocklike = false;
            }
            let p = legs[k].permille(i);
            if !(V3D64_CLOCK_LO..=V3D64_CLOCK_HI).contains(&p) {
                clocklike = false;
            }
        }
        k += 1;
    }
    if !any_moved {
        return "SILENT — zero on every valid leg, overflow included: idle, a retiring render frame and a bin kick all left it flat";
    }
    if wrapped {
        return "OTHER (RATIO UNUSABLE) — this slot latched an OVERFLOW on at least one valid leg, so its raw value is a wrapped residue and its permille is not a ratio. It moved; it is explicitly NOT band-tested against src32 and CANNOT be classified CLOCK-LIKE from this boot. Shorten the leg or widen the counter and re-measure";
    }
    // CLOCK-LIKE and WORK-GATED-RENDER both make a claim ABOUT THE IDLE LEG ("no work exists to gate
    // on" / "flat on idle"). Neither may be asserted when L1 dropped out — and an invalid L1 is the
    // PLAUSIBLE failure here, since src32 CYCLE_COUNT reading zero in pure idle is exactly what an
    // activity-gated clock would do. With L1 invalid the differential is real but idle-blind, so the
    // slot falls through to OTHER, whose wording claims nothing about idle.
    if clocklike && legs[0].valid() {
        return "CLOCK-LIKE — within tolerance of 1:1 with src32 CYCLE_COUNT on EVERY valid leg, INCLUDING the pure-idle leg where no work exists to gate on. Derived from the core clock, not from work";
    }
    // Render-gated: moved on L2 (the retiring CT1 job) and on no other valid leg.
    if legs[0].valid() && legs[1].valid() && legs[1].moved_i(i) {
        let mut only_render = true;
        k = 0;
        while k < legs.len() {
            if k != 1 && legs[k].valid() && legs[k].moved_i(i) {
                only_render = false;
            }
            k += 1;
        }
        if only_render {
            return "WORK-GATED-RENDER — moved ONLY on the retiring CT1 render leg, flat on the pure-idle leg and on the bin kick. Gated on real render work";
        }
    }
    if !legs[0].valid() && (clocklike || (legs[1].valid() && legs[1].moved_i(i))) {
        return "OTHER (IDLE-BLIND) — the L1 idle leg carried no live src32 control, so this boot cannot say whether the mux runs with no work present. The cross-leg differential stands, but neither CLOCK-LIKE nor WORK-GATED-RENDER may be asserted without the idle leg; re-measure with a valid L1";
    }
    "OTHER — it moves, but neither 1:1 with src32 nor render-only. Read the per-leg permille above: a ratio HELD CONSTANT across legs is clock-derived at that divisor, a ratio that VARIES is work-correlated"
}

/// PI-V3D-64 — emit one leg's raw bank.
fn v3d64_emit_leg(leg: &V3d64Leg, note: &str) {
    if leg.ctr.is_none() {
        serial_println!(
            "::   [v3d64] leg {} — NOT RUN ({}) — this leg contributes no reading; the classification below is taken on the legs that did run and says so ::",
            leg.tag, note
        );
        return;
    }
    serial_println!(
        "::   [v3d64] leg {} — {} | src32 CYCLE_COUNT={} (control, valid={}) | src{}={} src{}={} src{}={} src{}={} src{}={} src{}={} src{}={} [ALL SEVEN UNIDENTIFIED — raw mux reads, no name claimed] | OVERFLOW={:#010x} ::",
        leg.tag, note, leg.ctrl(), leg.valid() as u32,
        V3D63_SWEEP_SRC[0], leg.raw(0),
        V3D63_SWEEP_SRC[1], leg.raw(1),
        V3D63_SWEEP_SRC[2], leg.raw(2),
        V3D63_SWEEP_SRC[3], leg.raw(3),
        V3D63_SWEEP_SRC[4], leg.raw(4),
        V3D63_SWEEP_SRC[5], leg.raw(5),
        V3D63_SWEEP_SRC[6], leg.raw(6),
        leg.ovf(),
    );
}

/// PI-V3D-64 — the mux calibration battery. Same 8-slot bank as `[v3d63]`, run across three legs whose
/// ground truth is already established, then classified by behaviour alone.
///
/// Poison-honest: gated on a live hub-identity read with an explicit SKIPPED line when the block is
/// down, and every leg that does not run is REPORTED as not-run rather than folded in as a zero. QEMU
/// raspi4b models no V3D block and returns at `BLOCK-DOWN` long before this runs, so this probe is
/// dormant there — `kernel8-test` green for this arc means NO REGRESSION and nothing more.
///
/// Slot 2 remains `PCTR_SRC_CYCLE_COUNT`, per the `[v3d55]`/`V3D63_CTRL_SLOT` contract: it is the slot
/// `wait_fldone` samples for the clock-liveness verdict AND this bank's own control. The sweep never
/// occupies it, so the L3 bin leg's `[v3d55]` line stays truthful.
fn v3d64_mux_calibration() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: [v3d64] SKIPPED — hub IDENT0 reads 0x00000000 (block absent/unpowered; the QEMU raspi4b signature). No leg was measured this boot and NO mux was classified; no verdict is implied ::"
            );
            return;
        }
        V3dPresence::Poison(w) => {
            serial_println!(
                ":: V3D: [v3d64] SKIPPED — hub IDENT0 reads poison {:#010x} (open-bus / firmware fill, not a live register). No leg was measured this boot and NO mux was classified; no verdict is implied ::",
                w
            );
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d64] muxcal — the SAME 8-slot [v3d63] bank across three legs of KNOWN ground truth: L1 a {} ms pure-idle window (no job submitted), L2 the known-good CT1 clear job (the retiring M3 render job [v3d58] rerender reuses — no new job invented), L3 one banked Empty bin rung. Every slot is reported as permille of src32 CYCLE_COUNT on its own leg. This probe classifies BEHAVIOUR and claims NO names ::",
        V3D64_IDLE_MS
    );

    // ── L1 — pure idle. Arm, burn a fixed CNTPCT window with no submission, read. ────────────────────
    let frq = super::timer::cntfrq();
    let t0 = super::timer::cntpct();
    v3d63_pctr_arm();
    settle_ms(V3D64_IDLE_MS);
    let l1 = v3d63_pctr_read();
    let idle_us = (super::timer::cntpct().wrapping_sub(t0)).saturating_mul(1_000_000) / frq.max(1);

    // ── L2 — the known-good CT1 render job. Only a job that PASSED is ground truth. ──────────────────
    let render_ran = V3D58_RENDER_RAN.load(Ordering::Acquire);
    let render_ok = V3D58_RENDER_OK.load(Ordering::Acquire);
    let (l2, l2_note) = if !render_ran || !render_ok {
        (None, "the M3 clear job did not pass earlier this boot, so there is no known-good render leg to calibrate against")
    } else {
        v3d63_pctr_arm();
        let ok = clear_job(None); // panel blit suppressed; re-seeds + byte-verifies its own target
        let c = v3d63_pctr_read();
        if ok {
            (Some(c), "CT1 clear job RETIRED and byte-verified — real render work")
        } else {
            (
                Some(c),
                "CT1 clear job FAILED its verify on THIS run — the bank spans a render frame that did not complete; read this leg as work-attempted, not work-retired",
            )
        }
    };

    // ── L3 — the bin-side leg: one banked Empty rung of the existing ladder. ─────────────────────────
    let l3 = submit_bisect_rung_at(
        "v3d64",
        "bin-empty",
        BinContent::Empty,
        OFF_SHADREC,
        OFF_PROBE_BIN_CL,
        true,
    );
    let (l3_ctr, l3_note) = match l3 {
        Some(r) => (
            Some(r.ctr),
            if r.retired {
                "Empty bin rung RETIRED — bin work that completed"
            } else {
                "Empty bin rung did NOT retire (the banked wedge) — the bank spans an OPEN bin frame plus its ~0.5 s FLDONE backstop"
            },
        ),
        None => (None, "the rung fail-closed before its kick (range escape)"),
    };

    let legs = [
        V3d64Leg { tag: "L1 idle", ctr: Some(l1) },
        V3d64Leg { tag: "L2 render", ctr: l2 },
        V3d64Leg { tag: "L3 bin", ctr: l3_ctr },
    ];

    serial_println!(
        "::   [v3d64] leg L1 idle — pure-idle window, no job submitted, span_us={} (target {} ms) ::",
        idle_us, V3D64_IDLE_MS
    );
    v3d64_emit_leg(&legs[0], "no job submitted");
    v3d64_emit_leg(&legs[1], l2_note);
    v3d64_emit_leg(&legs[2], l3_note);

    let mut valid_n = 0usize;
    let mut k = 0usize;
    while k < legs.len() {
        if legs[k].valid() {
            valid_n += 1;
        }
        k += 1;
    }
    if valid_n < 2 {
        serial_println!(
            ":: V3D: [v3d64] muxcal INCONCLUSIVE — only {} of 3 legs carried a live src32 CYCLE_COUNT control (L1={} L2={} L3={}). A classification needs at least two legs to differentiate across; NOTHING here is a measurement and no mux is classified. Instrument the bank before reading any slot as evidence — this is explicitly NOT a clean result ::",
            valid_n, legs[0].ctrl(), legs[1].ctrl(), legs[2].ctrl()
        );
        return;
    }

    let mut i = 0usize;
    while i < V3D63_SWEEP_SRC.len() {
        serial_println!(
            "::   [v3d64] muxcal src{} — L1 idle={} ({} permille of src32) | L2 render={} ({} permille) | L3 bin={} ({} permille) | valid L1={} L2={} L3={} — CLASS: {} ::",
            V3D63_SWEEP_SRC[i],
            legs[0].raw(i), legs[0].permille(i),
            legs[1].raw(i), legs[1].permille(i),
            legs[2].raw(i), legs[2].permille(i),
            legs[0].valid() as u32, legs[1].valid() as u32, legs[2].valid() as u32,
            v3d64_classify(&legs, i),
        );
        i += 1;
    }

    serial_println!(
        ":: V3D: [v3d64] muxcal verdict — {} of 3 legs valid; sweep ids {:?} classified by BEHAVIOUR ONLY (CLOCK-LIKE / WORK-GATED-RENDER / SILENT / OTHER). NO source id is named here and none may be inferred from this line: a class says how a mux responded to a pure-idle window, a retiring CT1 render frame and a bin kick, and says nothing about which unit it counts. THE STANDING RULE THIS ESTABLISHES: when a later arc transcribes an id from `enum drm_v3d_perfcnt` and cross-checks it against this file's verified anchors (16 valid_instr, 32 cycle_count), the transcribed NAME MUST MATCH THE CLASS MEASURED HERE — a name implying render/primitive work on a mux measured CLOCK-LIKE, or a clock/cycle name on a mux measured WORK-GATED-RENDER, is REJECTED as mis-transcribed. Calibration checks the transcription, never the reverse ::",
        valid_n, V3D63_SWEEP_SRC
    );

    // PI-V3D-65: submit this arc's single transcribed source id to the rule the line above establishes.
    // It reads the SAME legs — no extra job, no extra register write, no second measurement.
    v3d65_anchor_check(&legs);
}

// ─── PI-V3D-65 (`srcname`) — transcribe ONE PCTR source id and let V3D-64's calibration judge it. ────
//
// V3D-63 DROPPED three named sources rather than guess them; V3D-64 closed with a standing rule: when
// an id is finally transcribed, THE NAME MUST MATCH THE MEASURED CLASS or the transcription is
// REJECTED. This arc transcribes exactly ONE id and submits it to that rule in the same boot, on the
// same legs, with a verdict word printed on the wire.
//
// ── SOURCE, cited exactly ───────────────────────────────────────────────────────────────────────────
// The blocker V3D-63 and V3D-64 both recorded — "this tree carries no copy of `enum drm_v3d_perfcnt`
// to transcribe from" — is RETIRED. The build host carries the Linux uapi header itself:
//
//   file : include/uapi/drm/v3d_drm.h  (Linux uapi header)
//   copy : /usr/src/kernels/7.1.5-200.fc44.x86_64/include/uapi/drm/v3d_drm.h — Fedora `kernel-devel`
//          for 7.1.5-200.fc44, the build host's own running kernel (reachable from this sandbox as
//          /run/host/usr/src/kernels/7.1.5-200.fc44.x86_64/include/uapi/drm/v3d_drm.h)
//   what : the anonymous `enum { V3D_PERFCNT_* }` at the tail of the header, first member
//          `V3D_PERFCNT_FEP_VALID_PRIMTS_NO_PIXELS` at index 0.
//   note : the enum's own comment says these indices "are only valid for V3D 4.2". This hardware IS
//          V3D 4.2 (Pi 4), so that deprecation note is a POSITIVE applicability statement for us, not
//          a warning against use. Newer cores must query DRM_IOCTL_V3D_PERFMON_GET_COUNTER instead;
//          we are not a newer core.
//
// ── INDEX CROSS-CHECK — what licenses the transcription ─────────────────────────────────────────────
// This is a strictly stronger check than V3D-63's "must fall between the 16/32 anchors" bracket rule.
// Counting that enum from 0, EVERY source id this file already carries from earlier arcs lands on
// exactly the name it was given — six for six, no misses:
//
//   14 → QPU_ACTIVE_CYCLES_VERTEX_COORD_USER   (PCTR_SRC_QPU_ACTIVE_CYCLES_VERTEX_COORD_USER)
//   16 → QPU_CYCLES_VALID_INSTR                (PCTR_SRC_QPU_CYCLES_VALID_INSTR)      — anchor
//   17 → QPU_CYCLES_TMU_STALL                  (PCTR_SRC_QPU_CYCLES_WAITING_TMU)
//   24 → TMU_TCACHE_ACCESS                     (PCTR_SRC_TMU_TCACHE_ACCESS)
//   25 → TMU_TCACHE_MISS                       (PCTR_SRC_TMU_TCACHE_MISS)
//   32 → CYCLE_COUNT                           (PCTR_SRC_CYCLE_COUNT)                 — anchor
//
// 32 == CYCLE_COUNT independently re-pins the `v3d_regs.h V3D_PCTR_CYCLE_COUNT(ver)=32` anchor for
// ver<71 from a second, unrelated file. Six independent hits with zero misses is not an accident of
// ordering: on 4.2 the enum INDEX IS the hardware source id, exactly as this file has assumed since
// PI-V3D-21 — now demonstrated rather than assumed.
//
// ── THE ONE ID, and why this one ────────────────────────────────────────────────────────────────────
// Index 28 is `V3D_PERFCNT_BIN_ACTIVE`. It is picked over every other candidate because it is the only
// id that is BOTH bin/PTB-side AND already carries a MEASURED V3D-64 behaviour class on this same boot:
//
//   * It is the sweep's one mover. P65v2 metal (capture pi4-r23s1o) read src28 ≈ 645M against a src32
//     CYCLE_COUNT control of ≈ 645M on ZERO-primitive bin rungs — the only swept slot at 1:1 there.
//     V3D-64 flagged that as the ambiguity worth resolving: free-running clock, or gated on something
//     the bin kick supplies? Under the name BIN_ACTIVE the second reading is the live one, and it is
//     the campaign's first direct observable OF THE BINNER: the frame opens, the bin unit stays ACTIVE
//     for the whole ~0.5 s FLDONE backstop, and it still never retires. "The binner is engaged and
//     never finishes" is precisely the wall this track has been circling.
//   * Every other bin/PTB-side candidate in the enum (35 PTB_PRIMS_BINNED, the PTB_MEM_* family, 57
//     CLE_ACTIVE) is NOT in `V3D63_SWEEP_SRC`, so its name would land beyond the reach of V3D-64's
//     rail this boot — an UNCHECKED name, which is the exact thing this arc exists to refuse. One id,
//     and it must be one that calibration can actually judge.
//   * The five confirmed-silent ids (1, 10, 11, 12, 29) are checkable but uninformative: a SILENT
//     measurement is consistent with almost any name on a bin path that bins no primitives.
//
// This arc names ONE id. 1, 10, 11, 12, 13 and 29 stay RAW and UNNAMED — the V3D-63 treatment holds
// for them, and nothing below may be read as naming them by adjacency.
const V3D65_SRC_ID: u32 = 28;
const V3D65_SRC_NAME: &str = "V3D_PERFCNT_BIN_ACTIVE";

/// PI-V3D-65 — where `V3D65_SRC_ID` sits in the `[v3d63]` sweep, or `None` if a later edit drops it
/// from the bank. Resolved from `V3D63_SWEEP_SRC` rather than hard-coded, so the anchor-check cannot
/// silently read the wrong slot if the sweep is ever reordered.
const fn v3d65_sweep_index() -> Option<usize> {
    let mut i = 0usize;
    while i < V3D63_SWEEP_SRC.len() {
        if V3D63_SWEEP_SRC[i] == V3D65_SRC_ID {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// PI-V3D-65 — the anchor check. Reads the legs `[v3d64]` already measured and asks whether the
/// transcribed NAME survives them.
///
/// The predicates are derived from the name and from nothing else. `BIN_ACTIVE` means "the binner is
/// active", so, in V3D-64's own vocabulary:
///
///   * **P1 idle-flat** — L1 is a pure-idle window with NO job submitted. A counter of bin activity
///     MUST read zero there (overflow included). If it ticks with no bin work, it is measuring the
///     clock, not the binner, and the name is wrong.
///   * **P2 bin-live** — L3 is the banked `Empty` rung, whose bin frame demonstrably OPENS and is held
///     open across the ~0.5 s backstop. A counter of bin activity MUST move there. A `SILENT`
///     measurement falsifies the name just as hard as a clock-like one.
///   * **P3 class** — consequently the measured V3D-64 class must be neither `CLOCK-LIKE` nor
///     `SILENT`. P3 is redundant with P1+P2 by construction; it is evaluated anyway so the verdict is
///     stated against the published class string and not only against raw slot values.
///
/// **L2 is REPORTED and is NOT a reject condition.** Whether a CT1-only clear job spins the bin unit
/// is a question the name `BIN_ACTIVE` does not settle, and an anchor-check may not reject on a
/// prediction the name never made. It is printed so the capture carries the observation.
///
/// A verdict requires L1 and L3 to be VALID legs (their own src32 control moved). Without them the
/// probe prints INCONCLUSIVE and says neither PASS nor REJECTED — a name is never confirmed off an
/// uninstrumented bank, which is the failure mode this whole campaign keeps unwinding.
///
/// QEMU raspi4b models no V3D PCTR block at all; this never runs there (the caller returns at the hub
/// identity gate with a `SKIPPED` line). The verdict below is METAL-ATTENDED, always.
fn v3d65_anchor_check(legs: &[V3d64Leg; 3]) {
    let i = match v3d65_sweep_index() {
        Some(i) => i,
        None => {
            serial_println!(
                ":: V3D: [v3d65] anchor-check SKIPPED — src{} ({}) is not carried in the [v3d63] sweep bank {:?}, so V3D-64 measured no class for it and this transcription has NOTHING to be checked against. An unchecked name is not published: re-add the id to the sweep or drop the transcription ::",
                V3D65_SRC_ID, V3D65_SRC_NAME, V3D63_SWEEP_SRC
            );
            return;
        }
    };

    serial_println!(
        ":: V3D: [v3d65] srcname — ONE source id transcribed from `enum {{ V3D_PERFCNT_* }}` in Linux uapi include/uapi/drm/v3d_drm.h (host copy: /usr/src/kernels/7.1.5-200.fc44.x86_64/…), whose own comment scopes those indices to V3D 4.2 — THIS hardware. Licensed by a six-for-six index cross-check against the ids this file already carries (14 vertex_coord_user, 16 valid_instr, 17 tmu_stall, 24 tcache_access, 25 tcache_miss, 32 cycle_count), which also re-pins the v3d_regs.h CYCLE_COUNT=32 anchor from a second file. TRANSCRIBED: src{} = {} (sweep index {}, PCTR slot {}). No other swept id is named — 1/10/11/12/13/29 stay RAW per [v3d63] ::",
        V3D65_SRC_ID, V3D65_SRC_NAME, i, v3d63_slot(i)
    );

    let class = v3d64_classify(legs, i);

    if !legs[0].valid() || !legs[2].valid() {
        serial_println!(
            ":: V3D: [v3d65] anchor-check INCONCLUSIVE — the check needs BOTH the L1 idle leg (for the idle-flat predicate) and the L3 bin leg (for the bin-live predicate) to have carried a live src32 control, and this boot had L1 valid={} (src32={}) L3 valid={} (src32={}). NO verdict is returned: the name {} is NEITHER confirmed NOR rejected here, and must not be cited either way off this boot. Instrument the bank and re-measure ::",
            legs[0].valid() as u32, legs[0].ctrl(),
            legs[2].valid() as u32, legs[2].ctrl(),
            V3D65_SRC_NAME
        );
        return;
    }

    let p1 = !legs[0].moved_i(i); // idle-flat
    let p2 = legs[2].moved_i(i); // bin-live
    let p3 = !class.starts_with("CLOCK-LIKE") && !class.starts_with("SILENT");
    let pass = p1 && p2 && p3;

    serial_println!(
        "::   [v3d65] anchor-check predicates for src{} = {} — P1 idle-flat (L1 raw={} ovf={}, must be 0/0): {} | P2 bin-live (L3 raw={} ovf={}, must MOVE): {} | P3 measured class is neither CLOCK-LIKE nor SILENT: {} | observed L2 render raw={} ovf={} ({}‰ of src32) — REPORTED ONLY, not a reject condition: the name BIN_ACTIVE makes no claim about whether a CT1-only clear job spins the bin unit ::",
        V3D65_SRC_ID, V3D65_SRC_NAME,
        legs[0].raw(i), legs[0].ovf_i(i) as u32, if p1 { "HOLDS" } else { "FAILS" },
        legs[2].raw(i), legs[2].ovf_i(i) as u32, if p2 { "HOLDS" } else { "FAILS" },
        if p3 { "HOLDS" } else { "FAILS" },
        legs[1].raw(i), legs[1].ovf_i(i) as u32, legs[1].permille(i),
    );
    serial_println!(
        "::   [v3d65] measured class for src{} (from [v3d64], this boot, these legs) — {} ::",
        V3D65_SRC_ID, class
    );

    if pass {
        serial_println!(
            ":: V3D: [v3d65] anchor-check VERDICT: PASS — src{} = {} survives V3D-64's rail. It is FLAT across a pure-idle window with no job submitted, and it MOVES on a bin kick whose frame opens and never retires. Every predicate the NAME makes is borne out by behaviour measured independently of the name. The campaign therefore has, for the first time, a NAMED bin-side observable: BIN_ACTIVE says the bin unit IS ENGAGED across the whole FLDONE backstop while the frame still fails to close — the binner is not absent and not asleep, it is running and not finishing. Read with two standing limits: this confirms the ID↔NAME transcription and the mux's behaviour, NOT any semantic beyond \"bin unit active\"; and it names ONE id — 1/10/11/12/13/29 remain unnamed ::",
            V3D65_SRC_ID, V3D65_SRC_NAME
        );
    } else {
        serial_println!(
            ":: V3D: [v3d65] anchor-check VERDICT: REJECTED — src{} = {} does NOT survive V3D-64's rail (P1 idle-flat={} P2 bin-live={} P3 class={}). The transcription is treated as MIS-APPLIED and the name is WITHDRAWN: src{} reverts to a RAW unnamed mux under the [v3d63] treatment and NOTHING in this file may cite it as the binner. Note what is and is not implicated — the index cross-check (six-for-six against 14/16/17/24/25/32) is independent of this outcome, so the likely fault is the id↔MUX mapping on this silicon (a SRC field that does not select the enum's source on 4.2), not the enum reading. Calibration checks the transcription, never the reverse ::",
            V3D65_SRC_ID, V3D65_SRC_NAME,
            p1 as u32, p2 as u32, p3 as u32, V3D65_SRC_ID
        );
    }
}

// ─── PI-V3D-66 (`ptbsweep`) — widen the calibration sweep to the PTB-neighbourhood ids. MEASURE ONLY ─
//
// ── What V3D-65 actually settled, and why this arc names nothing ────────────────────────────────────
// V3D-65 transcribed src28 = `V3D_PERFCNT_BIN_ACTIVE` from the uapi enum and submitted it to V3D-64's
// rail. **The anchor check REJECTED it**: on metal src28 measured CLOCK-LIKE — 1:1 with the src32
// CYCLE_COUNT control on EVERY leg including the pure-idle one, where by the name's own predicate a bin
// counter must read zero. The six-for-six index cross-check (14/16/17/24/25/32 land on the names this
// file already gave them) is independent of that outcome and STANDS, so the enum reading is right and
// the fault is downstream of it: on this silicon the 7-bit SRC field does not select the enum's source
// for every id. **id↔mux validity is PARTIAL, and it is partial in a way we cannot predict per id.**
//
// That is the whole reason this arc is a MEASUREMENT and not a transcription. With id↔mux only
// partially valid, a name carried over from the enum is worth exactly as much as the behaviour that
// backs it — so V3D-66 widens the bank, measures the new muxes across the SAME three legs of known
// ground truth, and publishes the raw numbers and the behaviour classes. **NOTHING is named here.**
// Transcription is left to V3D-67, which will have this arc's classes to be judged against.
//
// ── The candidate ids, cited exactly ────────────────────────────────────────────────────────────────
// Same header, same copy, same enum as V3D-65:
//
//   file : include/uapi/drm/v3d_drm.h  (Linux uapi header)
//   copy : /usr/src/kernels/7.1.5-200.fc44.x86_64/include/uapi/drm/v3d_drm.h — Fedora `kernel-devel`
//          for the build host's own running kernel (reachable here as /run/host/usr/src/kernels/…)
//   what : the anonymous `enum { V3D_PERFCNT_* }` at the tail of the header, first member
//          `V3D_PERFCNT_FEP_VALID_PRIMTS_NO_PIXELS` at index 0; the enum's own comment scopes these
//          indices to V3D 4.2 — THIS hardware.
//
// The eight indices this arc adds, transcribed with their enum members as they appear in that copy
// (line numbers are that file's):
//
//   30 → V3D_PERFCNT_L2T_HITS                              (v3d_drm.h:653)
//   31 → V3D_PERFCNT_L2T_MISSES                            (v3d_drm.h:654)
//   33 → V3D_PERFCNT_QPU_CYCLES_STALLED_VERTEX_COORD_USER  (v3d_drm.h:656)
//   34 → V3D_PERFCNT_QPU_CYCLES_STALLED_FRAGMENT           (v3d_drm.h:657)
//   35 → V3D_PERFCNT_PTB_PRIMS_BINNED                      (v3d_drm.h:658)
//   36 → V3D_PERFCNT_AXI_WRITES_WATCH_0                    (v3d_drm.h:659)
//   37 → V3D_PERFCNT_AXI_READS_WATCH_0                     (v3d_drm.h:660)
//   38 → V3D_PERFCNT_AXI_WRITE_STALLS_WATCH_0              (v3d_drm.h:661)
//
// **A correction to the brief that raised this arc, recorded rather than quietly absorbed.** The set
// was requested as "the PTB_BLOCKED_CYCLES / PTB_PRIMS_BINNED family". In THIS header there is no
// `V3D_PERFCNT_PTB_BLOCKED_CYCLES` at all — `grep PTB` over the enum returns exactly seven members
// (10 PRIM_VIEWPOINT_DISCARD, 11 PRIM_CLIP, 12 PRIM_REV, 35 PRIMS_BINNED, plus PTB_MEM_WRITES,
// PTB_MEM_READS, PTB_W_MEM_WORDS in the AXI/memory tail). Of the eight indices above exactly ONE, 35,
// is a PTB member; 30/31 are L2T, 33/34 are QPU stall counters and 36/37/38 are AXI watch-0 counters.
// The indices are swept as given because the SET is what the brief pinned and because — after V3D-65 —
// what an id's enum member SAYS is not what licenses anything anyway. The window is swept as a search
// window, and it is defensible only as one. What each mux turns out to be is decided below by
// behaviour, and 35's PTB name buys it no head start over the other seven.
//
// ── Batching against the PCTR slot count ────────────────────────────────────────────────────────────
// The hardware bank is EIGHT counters (`V3D_V4_PCTR_0_SRC_0_3` / `_4_7`, one 7-bit SRC field each), and
// slot 2 is permanently reserved for the src32 CYCLE_COUNT control per the `[v3d55]`/`V3D63_CTRL_SLOT`
// contract — `wait_fldone` samples PCTR counter 2 on every bin wait and prints a hard clock-liveness
// verdict off it, so a sweep id parked there would fabricate that verdict. Seven usable sweep slots,
// and `[v3d63]`'s seven ids already occupy all of them. The eight new ids therefore cannot ride the
// existing bank at all, and they do not: they run as TWO ADDITIONAL PASSES of four, each pass a fresh
// arm of the same 8-counter bank with only slots 0,1,2,3,4 enabled (`V3D66_PCTR_MASK`), the same
// `v3d63_slot` packing, the same src32 control in slot 2 — and each pass re-runs the SAME three legs so
// every permille ratio is taken against a control measured on its own pass. Slots 5..7 are left
// unselected AND disabled in `PCTR_EN`; they are never read and never printed.
//
// The existing `V3D63_SWEEP_SRC` bank is untouched — `[v3d63]` and `[v3d64]`/`[v3d65]` print exactly
// what they printed before this arc, byte for byte.
//
// ── Cost ────────────────────────────────────────────────────────────────────────────────────────────
// Two passes x (a 4 ms idle window + one CT1 clear job + one banked Empty bin rung with its ~0.5 s
// FLDONE backstop) = 2 more rungs and ~1 s of extra deep boot. The `[v3d] deep=on` budget line is
// updated to say so.
//
// Writes: PCTR config registers only (the `v3d63_pctr_arm` idiom), plus the legs' own already-existing
// submissions. No new MMIO window, no new job, no new CL.

/// PI-V3D-66 — pass A of the widened sweep. Raw indices; NO name is attached to any of them.
const V3D66_BATCH_A: [u32; 4] = [30, 31, 33, 34];
/// PI-V3D-66 — pass B of the widened sweep. Raw indices; NO name is attached to any of them.
const V3D66_BATCH_B: [u32; 4] = [35, 36, 37, 38];
/// PI-V3D-66 — counters enabled per pass: slots 0,1,3,4 carry the four swept ids and slot 2 carries the
/// reserved src32 CYCLE_COUNT control. Slots 5..7 stay DISABLED so an unselected mux (SRC field 0) can
/// never be mistaken for a measurement.
const V3D66_PCTR_MASK: u32 = 0x1F;

/// PI-V3D-66 — arm the bank for one four-id pass. Identical programming order to `v3d63_pctr_arm`
/// (stop → program SRC → read back the selects so the posted stores retire, per PI-V3D-39 → clear
/// counters and overflow WHILE stopped → enable last), differing only in the narrower enable mask.
fn v3d66_pctr_arm(batch: &[u32; 4]) {
    // Slot packing IS `v3d63_slot`: 0→0, 1→1, control→2, 2→3, 3→4. Slots 5..7 select source 0 but are
    // masked out of PCTR_EN below, so they never count and are never read.
    let src_0_3 = (batch[0] & 0x7f)
        | ((batch[1] & 0x7f) << 8)
        | ((PCTR_SRC_CYCLE_COUNT & 0x7f) << 16)
        | ((batch[2] & 0x7f) << 24);
    let src_4_7 = batch[3] & 0x7f;
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, 0);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_0_3, src_0_3);
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_4_7, src_4_7);
    dsb();
    // PI-V3D-39: read the selects back so the posted stores RETIRE and the mux is provably latched.
    let _srclat = mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_0_3)
        | mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_4_7);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_CLR, V3D63_PCTR_MASK); // clear all eight regardless
    mmio_write(V3D_CORE0_BASE, V3D_PCTR_0_OVERFLOW, V3D63_PCTR_MASK);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, V3D66_PCTR_MASK); // enable LAST, from a clean zero
    dsb();
}

/// PI-V3D-66 — classify one swept mux by BEHAVIOUR across the three legs, with one class more than
/// `[v3d64]` had vocabulary for.
///
/// **`WORK-GATED-BIN`, defined by predicate and by nothing else:**
///
/// > the L1 idle leg is VALID **and** the L3 bin leg is VALID **and** the slot MOVED on L3 (raw != 0 or
/// > a latched overflow) **and** the slot did NOT move on L1 **and** the slot did not move on L2 if L2
/// > is valid.
///
/// In words: it moves on a bin kick and on nothing else that this boot can show it. It is the exact
/// mirror of `[v3d64]`'s `WORK-GATED-RENDER` about the other work leg, and it is the class the render
/// CLE's wall actually needs — a mux in it is a candidate witness for whether the binner emits anything
/// at all, and V3D-67 may test a PTB name against it.
///
/// The predicate requires L1 valid on purpose: "moves only on bin" is a claim about the idle leg, and
/// a boot whose L1 dropped out cannot make it. Such a slot falls through to `[v3d64]`'s own
/// `OTHER (IDLE-BLIND)` wording, which claims nothing about idle.
///
/// `WORK-GATED-BIN` cannot collide with `CLOCK-LIKE`: the predicate requires a VALID L1 on which the
/// slot read zero, whose permille is 0 and therefore outside the ±10% band `v3d64_classify` requires on
/// every valid leg. Under the old vocabulary such a slot printed a bare `OTHER`; the split is a
/// refinement of `OTHER`, not a reinterpretation of any class `[v3d64]` already published. `[v3d63]`,
/// `[v3d64]` and `[v3d65]` keep calling `v3d64_classify` directly and their output is unchanged.
fn v3d66_classify(legs: &[V3d64Leg; 3], i: usize) -> &'static str {
    if legs[0].valid()
        && legs[2].valid()
        && legs[2].moved_i(i)
        && !legs[0].moved_i(i)
        && !(legs[1].valid() && legs[1].moved_i(i))
    {
        return "WORK-GATED-BIN — moved ONLY on the bin-kick leg: flat on the pure-idle window where no work exists to gate on, flat on the retiring CT1 render frame, and moving on the banked Empty bin rung. Gated on bin-side work and on nothing else this boot can show it. NO NAME IS CLAIMED — this says how the mux responded, not which unit it counts";
    }
    v3d64_classify(legs, i)
}

/// PI-V3D-66 — emit one pass's raw bank for one leg. Four swept ids, printed by index, unnamed.
fn v3d66_emit_leg(pass: &str, batch: &[u32; 4], leg: &V3d64Leg, note: &str) {
    if leg.ctr.is_none() {
        serial_println!(
            "::   [v3d66] pass {} leg {} — NOT RUN ({}) — this leg contributes no reading; the classification below is taken on the legs that did run and says so ::",
            pass, leg.tag, note
        );
        return;
    }
    serial_println!(
        "::   [v3d66] pass {} leg {} — {} | src32 CYCLE_COUNT={} (control, valid={}) | src{}={} src{}={} src{}={} src{}={} [ALL FOUR UNIDENTIFIED — raw mux reads, NO name claimed: V3D-65 proved id↔mux validity is PARTIAL on this silicon] | OVERFLOW={:#010x} ::",
        pass, leg.tag, note, leg.ctrl(), leg.valid() as u32,
        batch[0], leg.raw(0),
        batch[1], leg.raw(1),
        batch[2], leg.raw(2),
        batch[3], leg.raw(3),
        leg.ovf(),
    );
}

/// PI-V3D-66 — run ONE four-id pass across the same three legs `[v3d64]` calibrates on, then classify.
///
/// The legs are reproduced from `v3d64_mux_calibration` deliberately rather than shared: each pass must
/// carry its OWN src32 control, measured on its own arm, or the permille ratios would be taken against
/// a control from a different window. No new job class is invented — L2 is the same known-good
/// `clear_job`, L3 the same banked `Empty` rung of the `[v3d48]` ladder.
fn v3d66_run_pass(pass: &str, batch: &[u32; 4]) {
    // ── L1 — pure idle. Arm, burn a fixed CNTPCT window with no submission, read. ────────────────────
    let frq = super::timer::cntfrq();
    let t0 = super::timer::cntpct();
    v3d66_pctr_arm(batch);
    settle_ms(V3D64_IDLE_MS);
    let l1 = v3d63_pctr_read();
    let idle_us = (super::timer::cntpct().wrapping_sub(t0)).saturating_mul(1_000_000) / frq.max(1);

    // ── L2 — the known-good CT1 render job. Only a job that PASSED is ground truth. ──────────────────
    let render_ran = V3D58_RENDER_RAN.load(Ordering::Acquire);
    let render_ok = V3D58_RENDER_OK.load(Ordering::Acquire);
    let (l2, l2_note) = if !render_ran || !render_ok {
        (None, "the M3 clear job did not pass earlier this boot, so there is no known-good render leg to calibrate against")
    } else {
        v3d66_pctr_arm(batch);
        let ok = clear_job(None); // panel blit suppressed; re-seeds + byte-verifies its own target
        let c = v3d63_pctr_read();
        if ok {
            (Some(c), "CT1 clear job RETIRED and byte-verified — real render work")
        } else {
            (
                Some(c),
                "CT1 clear job FAILED its verify on THIS run — the bank spans a render frame that did not complete; read this leg as work-attempted, not work-retired",
            )
        }
    };

    // ── L3 — the bin-side leg. The banked rung arms the [v3d63] bank itself, so this pass re-arms its
    //    own four-id bank around the submission instead of reading the rung's returned counters
    //    (`bank: false` — the rung must NOT re-arm the seven-id [v3d63] bank over ours). Slot 2 still
    //    carries src32 CYCLE_COUNT, so the `[v3d55]` clock-liveness line `wait_fldone` prints from PCTR
    //    counter 2 inside this rung's wait remains truthful, exactly as under the [v3d63] bank.
    v3d66_pctr_arm(batch);
    let l3r = submit_bisect_rung_at(
        "v3d66",
        "bin-empty",
        BinContent::Empty,
        OFF_SHADREC,
        OFF_PROBE_BIN_CL,
        false,
    );
    let l3 = v3d63_pctr_read();
    let (l3_ctr, l3_note) = match l3r {
        Some(r) => (
            Some(l3),
            if r.retired {
                "Empty bin rung RETIRED — bin work that completed"
            } else {
                "Empty bin rung did NOT retire (the banked wedge) — the bank spans an OPEN bin frame plus its ~0.5 s FLDONE backstop"
            },
        ),
        None => (None, "the rung fail-closed before its kick (range escape)"),
    };

    let legs = [
        V3d64Leg { tag: "L1 idle", ctr: Some(l1) },
        V3d64Leg { tag: "L2 render", ctr: l2 },
        V3d64Leg { tag: "L3 bin", ctr: l3_ctr },
    ];

    serial_println!(
        "::   [v3d66] pass {} — swept ids {:?} in PCTR slots 0,1,3,4 with src32 CYCLE_COUNT pinned to slot 2 (PCTR_EN={:#04x}); L1 idle span_us={} (target {} ms) ::",
        pass, batch, V3D66_PCTR_MASK, idle_us, V3D64_IDLE_MS
    );
    v3d66_emit_leg(pass, batch, &legs[0], "no job submitted");
    v3d66_emit_leg(pass, batch, &legs[1], l2_note);
    v3d66_emit_leg(pass, batch, &legs[2], l3_note);

    let mut valid_n = 0usize;
    let mut k = 0usize;
    while k < legs.len() {
        if legs[k].valid() {
            valid_n += 1;
        }
        k += 1;
    }
    if valid_n < 2 {
        serial_println!(
            ":: V3D: [v3d66] pass {} INCONCLUSIVE — only {} of 3 legs carried a live src32 CYCLE_COUNT control (L1={} L2={} L3={}). A classification needs at least two legs to differentiate across; NOTHING in this pass is a measurement and no mux is classified. This is explicitly NOT a clean result ::",
            pass, valid_n, legs[0].ctrl(), legs[1].ctrl(), legs[2].ctrl()
        );
        return;
    }

    let mut i = 0usize;
    while i < batch.len() {
        serial_println!(
            "::   [v3d66] pass {} src{} — L1 idle={} ({} permille of src32) | L2 render={} ({} permille) | L3 bin={} ({} permille) | valid L1={} L2={} L3={} — CLASS: {} ::",
            pass, batch[i],
            legs[0].raw(i), legs[0].permille(i),
            legs[1].raw(i), legs[1].permille(i),
            legs[2].raw(i), legs[2].permille(i),
            legs[0].valid() as u32, legs[1].valid() as u32, legs[2].valid() as u32,
            v3d66_classify(&legs, i),
        );
        i += 1;
    }

    serial_println!(
        ":: V3D: [v3d66] pass {} verdict — {} of 3 legs valid; ids {:?} classified by BEHAVIOUR ONLY (CLOCK-LIKE / WORK-GATED-RENDER / WORK-GATED-BIN / SILENT / OTHER). NO source id is named in this pass and none may be inferred from it. THE STANDING RULE, UNCHANGED: when a later arc transcribes one of these ids from `enum drm_v3d_perfcnt`, the transcribed NAME MUST MATCH THE CLASS MEASURED HERE or the transcription is REJECTED — calibration checks the transcription, never the reverse. V3D-65 already exercised that rule to a REJECT (src28 = BIN_ACTIVE measured CLOCK-LIKE and the name was withdrawn), which is why this arc lands measurements and leaves naming to V3D-67: the enum reading survived, the id↔MUX mapping on this silicon did not, and only behaviour can tell the two apart ::",
        pass, valid_n, batch
    );
}

/// PI-V3D-66 — the widened sweep: two four-id passes over the PTB-neighbourhood candidate indices.
///
/// Poison-honest and hub-gated exactly like `[v3d64]`: an explicit `SKIPPED` line when the block is
/// down, and every leg that does not run is REPORTED as not-run rather than folded in as a zero. QEMU
/// raspi4b models no V3D block, so this is dormant there and `kernel8-test` green means NO REGRESSION
/// and nothing more — the classes below are METAL-ATTENDED, always.
fn v3d66_ptb_sweep() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: [v3d66] SKIPPED — hub IDENT0 reads 0x00000000 (block absent/unpowered; the QEMU raspi4b signature). No pass was measured this boot and NO mux was classified; no verdict is implied ::"
            );
            return;
        }
        V3dPresence::Poison(w) => {
            serial_println!(
                ":: V3D: [v3d66] SKIPPED — hub IDENT0 reads poison {:#010x} (open-bus / firmware fill, not a live register). No pass was measured this boot and NO mux was classified; no verdict is implied ::",
                w
            );
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d66] ptbsweep — widening the [v3d64] calibration to eight more source ids from the SAME uapi enum copy (/usr/src/kernels/7.1.5-200.fc44.x86_64/include/uapi/drm/v3d_drm.h): {:?} then {:?}. Seven sweep slots exist per bank (slot 2 is reserved for the src32 CYCLE_COUNT control), so these run as TWO passes of four, each re-running the same three legs (L1 {} ms pure idle / L2 the known-good CT1 clear job / L3 one banked Empty bin rung) with its own control. This probe MEASURES; it names NOTHING ::",
        V3D66_BATCH_A, V3D66_BATCH_B, V3D64_IDLE_MS
    );
    v3d66_run_pass("A", &V3D66_BATCH_A);
    v3d66_run_pass("B", &V3D66_BATCH_B);
    serial_println!(
        ":: V3D: [v3d66] ptbsweep verdict — eight ids swept across two passes, {:?} + {:?}, every one of them RAW and UNNAMED. What the enum calls them is recorded in the source comment and is deliberately NOT printed on the wire, because V3D-65 proved on this silicon that an enum member and the mux its index selects can disagree: the six-for-six index cross-check (14/16/17/24/25/32) stands, and src28 = BIN_ACTIVE was still REJECTED for measuring CLOCK-LIKE on a pure-idle leg. id↔mux validity is therefore PARTIAL and per-id, and a name is licensed only by behaviour. THE STANDING RULE: a transcribed name must match the class measured here or it is REJECTED; calibration checks the transcription, never the reverse. Transcription is V3D-67's job, and it now has classes to be judged against ::",
        V3D66_BATCH_A, V3D66_BATCH_B
    );
}

// ─── PI-V3D-68 (`binwall`) — vary the FRAME, the one input the campaign has never varied. ───────────
//
// THE WALL, restated so the ladder below is read against it and not against a theory: on Pi 4 silicon
// the binner frame handshake never completes. `BMACTIVE` sets at the GO and never clears, `BMBUSY`
// never sets, `BFC` stays flat, `FLDONE` never fires, `INT_STS` reads 0 after the full backstop — while
// CT1 render frames on the same block retire and byte-verify every boot. Submission ([v3d54]), register
// mirrors ([v3d23/49]), reset ([v3d50]), the L2T window and flush mode ([v3d51/53]), packet encoding
// ([v3d57]), the MMU and GMP with the fault policy armed ([v3d60/62]), the clock domain ([v3d55/64])
// and the CL-epilogue question ([v3d59] T4: `v3dX(bcl_epilogue)` emits a bare FLUSH — no
// INCREMENT_SEMAPHORE, no FLUSH_ALL_STATE — so our terminator is already mainline-exact) are all closed.
//
// WHAT IS LEFT UNVARIED. Go back through every rung this campaign has ever kicked — `[v3d40]`'s probe,
// the six `[v3d48]` ladder rungs, `[v3d50/51/53]`, `[v3d63]`'s 2x2, `[v3d64]`/`[v3d66]`'s L3 legs, the
// real M4 draw. Every single one of them programmed the SAME `TILE_BINNING_MODE_CFG`: a 64x64 frame,
// which at this tree's established 64x64 bin tile is EXACTLY ONE TILE. The campaign has varied the CL's
// content, its placement, its length, the cache mode around it, the reset before it and the interrupt
// policy over it — and has never once varied the frame the PTB was asked to tile. §31's audit proved
// that one geometry is encoded exactly as Mesa encodes it; it could not prove that geometry is one the
// PTB can close, because it never compared it with another.
//
// A one-tile frame is the most degenerate legal frame there is, and it is the frame on which the
// campaign's most load-bearing fact — "an empty bin frame MUST retire" — was measured. That premise is
// itself unvaried: it is sourced from the kernel's `v3d_bin_job_run`, which asserts nothing of the kind,
// and NO Mesa path ever submits one (gallium drops a job whose draw bounds enclose nothing; v3dv never
// starts a frame it will not draw into). "The empty frame did not retire" may simply be the hardware's
// honest answer to a question no driver asks.
//
// So this arc varies the frame, on both axes at once, and reads four things per rung: whether it
// retired, whether ANY register reading moved with tile count, whether the PTB's reservation pointer
// scaled with tile count, and whether a poisoned tile-state array 16x larger than the old one caught a
// single PTB byte.
//
// THE RESERVATION PREDICTION — the part that can produce a POSITIVE PTB-side signal on a frame that
// never retires. Mesa `v3d_util.c::v3d_tile_alloc_sizes` states that the PTB requests the tile-alloc
// INITIAL block PER TILE at `START_TILE_BINNING`, before and independent of any primitive, then
// allocates in aligned 4 KiB chunks (two of them up front). §30 measured `BPCA` advancing by EXACTLY
// `align(1*128, 4096) + 8192 = 0x3000` on the one-tile frame — the formula's value to the byte, over a
// pool the poison proved untouched. That single calibration point cannot separate "the PTB reserved
// per-tile" from "the PTB grabs three 4 KiB chunks regardless". A 16-tile frame reserves 2048 B, which
// still rounds to 0x3000 — but under the 32x32 tile contingency the same frame is 64 tiles and reserves
// 8192 B, which rounds to 0x4000. So the ladder's `BPCA` column measures the effective tile size AND
// tests the per-tile reservation model at the same time, on a wedged frame, without needing a retire.
//
// WHAT IS DELIBERATELY NOT ARMED. `BXCF` (PTB binner extra config) is read and printed per rung and is
// never written — mainline writes it from no path, and a rung that both re-shaped the frame and poked an
// undocumented PTB config register would discriminate nothing. `MISCCFG.QRMAXCNT` stays disarmed for the
// same reason and by the standing `V3D55_ARM_QRMAXCNT` decision. `CTRSTA` stays disarmed. No new MMIO
// window is opened, no GMP or `MMU_CTL` write is made, and the real M4 draw is untouched.

/// PI-V3D-68 — the bin tile this tree's configuration selects: 1 RT, 32 bpp internal, no MSAA, no
/// double-buffer, which is Mesa's largest tile. Established in the `TILE_STATE_TILES` note, not
/// re-derived here; the ladder's own `BPCA` column is what tests it on silicon.
const V3D68_BIN_TILE_W: usize = 64;
const V3D68_BIN_TILE_H: usize = 64;
/// PI-V3D-68 — the tile-size contingency factor the arena bound is taken against: if the real bin tile
/// were 32x32 rather than 64x64, every rung would carry 4x the tiles. A rung is only kicked when its
/// TSDA fits the dedicated region even at that factor (`v3d68_geom_sound`).
const V3D68_TILE_CONTINGENCY: usize = 4;

/// PI-V3D-68 — one rung's frame geometry. The ONLY thing that differs between ladder rungs of the same
/// `BinContent`.
#[derive(Clone, Copy)]
struct V3d68Geom {
    w: usize,
    h: usize,
}

impl V3d68Geom {
    const fn tiles_x(&self) -> usize {
        (self.w + V3D68_BIN_TILE_W - 1) / V3D68_BIN_TILE_W
    }
    const fn tiles_y(&self) -> usize {
        (self.h + V3D68_BIN_TILE_H - 1) / V3D68_BIN_TILE_H
    }
    /// Tile count under this tree's established 64x64 bin tile (1 layer).
    const fn tiles(&self) -> usize {
        self.tiles_x() * self.tiles_y()
    }
    /// Tile count under the 32x32 contingency — what the frame would carry if the established tile size
    /// were wrong. Only ever used to BOUND the TSDA, never to claim a reading.
    const fn tiles_contingent(&self) -> usize {
        self.tiles() * V3D68_TILE_CONTINGENCY
    }
    /// The `BPCA` advance Mesa's `v3d_tile_alloc_sizes` reservation predicts for this frame:
    /// `align(tiles * V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE, 4096) + 8192`, with the initial block at 128 B
    /// (`v3d_limits.h`, and the enum our `TILE_BINNING_MODE_CFG` programs).
    const fn predicted_bpca(&self, tiles: usize) -> u32 {
        ((tiles * 128).next_multiple_of(4096) + 8192) as u32
    }
}

/// PI-V3D-68 — a rung may only be kicked when BOTH tile-size readings of its TSDA fit the dedicated
/// region. Fail-closed: a geometry that cannot be bounded is reported and skipped, never kicked.
fn v3d68_geom_sound(g: &V3d68Geom) -> bool {
    g.tiles_contingent() * TILE_STATE_BYTES_PER_TILE <= V3D68_TSDA_BYTES
        && BIN_TILEALLOC_BYTES >= (g.tiles_contingent() * 128).next_multiple_of(4096) + 8192
}

/// PI-V3D-68 — the ladder. Three `Empty` frames of growing tile count, then the smallest non-empty
/// frame at a non-degenerate geometry. The first entry is the campaign's banked frame, re-run here as
/// the in-boot control so every other rung is read as a differential against it and not against a
/// reading from another boot.
const V3D68_LADDER: [(&str, V3d68Geom, BinContent); 4] = [
    ("empty 64x64 (1 tile — the banked control)", V3d68Geom { w: 64, h: 64 }, BinContent::Empty),
    ("empty 128x128 (4 tiles)", V3d68Geom { w: 128, h: 128 }, BinContent::Empty),
    ("empty 256x256 (16 tiles)", V3d68Geom { w: 256, h: 256 }, BinContent::Empty),
    ("minprim 256x256 (16 tiles, full draw)", V3d68Geom { w: 256, h: 256 }, BinContent::Full),
];

/// PI-V3D-68 — one rung's measured surface beyond what `V3d63Rung` already carries.
#[derive(Clone, Copy)]
struct V3d68Read {
    kicked: bool,
    retired: bool,
    tiles: usize,
    pcs: u32,
    ct0ca: u32,
    ct0cs: u32,
    ct0lc: u32,
    ct0pc: u32,
    bfc: u32,
    bpca_adv: u32,
    bpcs: u32,
    bpos: u32,
    bxcf: u32,
    int_sts: u32,
    flush_done: bool,
    ts_touched: u32,
    pool_touched: u32,
}

impl V3d68Read {
    const NOT_RUN: Self = Self {
        kicked: false,
        retired: false,
        tiles: 0,
        pcs: 0,
        ct0ca: 0,
        ct0cs: 0,
        ct0lc: 0,
        ct0pc: 0,
        bfc: 0,
        bpca_adv: 0,
        bpcs: 0,
        bpos: 0,
        bxcf: 0,
        int_sts: 0,
        flush_done: false,
        ts_touched: 0,
        pool_touched: 0,
    };
    /// The bits this rung contributes to the "did ANYTHING respond to the frame shape?" comparison.
    /// Deliberately EXCLUDES `bpca_adv` and the touched counts, which are compared separately: they are
    /// the quantitative predictions, and folding them into a bare equality would let a matching
    /// prediction read as "no response".
    fn shape_signature(&self) -> (u32, u32, u32, u32, u32, u32, u32, u32, u32) {
        // `CT0CA` and `BPCS`/`BPOS` belong here: every `Empty` rung publishes a byte-identical control
        // list (only the two width/height fields differ, at a fixed length), so all three are directly
        // comparable across the geometry axis.
        (
            self.pcs, self.ct0cs, self.ct0lc, self.ct0pc, self.bfc, self.int_sts, self.ct0ca,
            self.bpcs, self.bpos,
        )
    }
}

/// PI-V3D-68 — kick one ladder rung and read everything the arc needs off it.
///
/// The rung itself is `submit_bisect_rung_geom` — the SAME kick sequence, cache maintenance, submission
/// audit and `FLDONE` backstop the `[v3d48]` ladder uses, with the geometry supplied. `bank: true` so
/// the `[v3d63]` sweep bank spans the frame and slot 2 keeps the `[v3d55]` clock-liveness witness
/// truthful inside the wait, exactly as under `[v3d63]`.
fn v3d68_run_rung(name: &str, g: &V3d68Geom, content: BinContent) -> V3d68Read {
    if !v3d68_geom_sound(g) {
        serial_println!(
            ":: V3D: [v3d68] {} — NOT RUN (fail-closed): a {}x{} frame is {} tiles at the established 64x64 bin tile and {} under the 32x32 contingency, whose tile-state array ({} B) exceeds the dedicated {} B region. A geometry this ladder cannot BOUND is never kicked; this rung contributes no reading and is not folded in as a zero ::",
            name, g.w, g.h, g.tiles(), g.tiles_contingent(),
            g.tiles_contingent() * TILE_STATE_BYTES_PER_TILE, V3D68_TSDA_BYTES
        );
        return V3d68Read::NOT_RUN;
    }
    let ts = (arena_phys() + OFF_V3D68_TSDA) as u32;
    let pool = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    // `OFF_SHADREC` for every rung: the `Full` rung reuses the real M4 shader record and vertex data
    // that `probe_job` already published and byte-audited, so the non-empty rung introduces no new
    // shader, no new record and no new geometry — only the frame it is binned into differs.
    let r = submit_bisect_rung_geom("v3d68", name, content, OFF_SHADREC, OFF_PROBE_BIN_CL, true, Some(*g));
    let r = match r {
        Some(r) => r,
        None => {
            serial_println!(
                ":: V3D: [v3d68] {} — NOT RUN: the rung fail-closed before its kick (range escape, see the line above). No reading ::",
                name
            );
            return V3d68Read::NOT_RUN;
        }
    };
    // PTB-side registers, read before anything else perturbs the block.
    let bpca = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA);
    let bpcs = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS);
    let bpos = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOS);
    let bxcf = mmio_read(V3D_CORE0_BASE, V3D_PTB_BXCF);
    // WITSWEEP: INT_STS comes from the rung's pre-W1C sample (`r.int_sts`), NOT a re-read here.
    // `wait_fldone` W1C-clears the latched status on its retire path, so a post-wait MMIO re-read
    // printed 0 for every RETIRED rung by construction — and that pinned zero fed shape_signature(),
    // making retired and non-retired rungs contribute asymmetric evidence to the geometry comparison.
    let int_sts = r.int_sts;
    let bfc = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    // The [v3d56] evidence-integrity rule: an "untouched" readback is only evidence if the L2T
    // write-back actually finished, so keep the completion bit and print it with the verdict.
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_FLUSH);
    let flush_done = wait_bit_clear(
        V3D_CORE0_BASE,
        V3D_CTL_L2TCACTL,
        V3D_L2TCACTL_L2TFLS,
        "L2T write-back (v3d68 geom scan)",
    );
    mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
    dsb();
    cache::invalidate_range(arena_phys() + OFF_V3D68_TSDA, V3D68_TSDA_BYTES);
    cache::invalidate_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    dsb();
    let ts_scan = v3d56_scan(ts, V3D68_TSDA_BYTES);
    let pool_scan = v3d56_scan(pool, BIN_TILEALLOC_BYTES);
    v3d56_emit_scan("v3d68", "tile-state (24 KiB, whole region)", ts, &ts_scan, flush_done);
    v3d56_emit_scan("v3d68", "tile-alloc pool", pool, &pool_scan, flush_done);

    let adv = bpca.wrapping_sub(pool);
    let pred_est = g.predicted_bpca(g.tiles());
    let pred_cont = g.predicted_bpca(g.tiles_contingent());
    serial_println!(
        ":: V3D: [v3d68] {} — {}x{} frame, {} tiles (64x64 tile) / {} tiles (32x32 contingency) | retired={} BFC={:#010x} PCS={:#010x}(BMACTIVE={} BMBUSY={} BMOOM={}) CT0CA={:#010x} CT0CS={:#010x}(CTRUN={}) CT0LC={:#x} CT0PC={:#x} INT_STS={:#010x} | PTB: BPCA advance={:#x} vs predicted {:#x} (64x64 tile) / {:#x} (32x32) — match64={} match32={} | BPCS={:#x} BPOS={:#x} BXCF={:#010x}(CLIPDISA={} RWORDERDISA={}, READ ONLY — never written by us or by mainline) | poison: tile-state touched={} of {} words, pool touched={} of {} words, L2T write-back completed={} ::",
        name, g.w, g.h, g.tiles(), g.tiles_contingent(),
        r.retired as u32, bfc, r.pcs,
        (r.pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (r.pcs & V3D_PCS_BMBUSY != 0) as u32,
        (r.pcs & V3D_PCS_BMOOM != 0) as u32,
        r.ct0ca, r.ct0cs,
        (r.ct0cs & V3D_CLE_CT1CS_CTRUN != 0) as u32,
        r.ct0lc, r.ct0pc, int_sts,
        adv, pred_est, pred_cont,
        (adv == pred_est) as u32, (adv == pred_cont) as u32,
        bpcs, bpos, bxcf,
        (bxcf & V3D_PTB_BXCF_CLIPDISA != 0) as u32,
        (bxcf & V3D_PTB_BXCF_RWORDERDISA != 0) as u32,
        ts_scan.zeroed + ts_scan.overwritten, ts_scan.words,
        pool_scan.zeroed + pool_scan.overwritten, pool_scan.words,
        flush_done as u32,
    );
    V3d68Read {
        kicked: true,
        retired: r.retired,
        tiles: g.tiles(),
        pcs: r.pcs,
        ct0ca: r.ct0ca,
        ct0cs: r.ct0cs,
        ct0lc: r.ct0lc,
        ct0pc: r.ct0pc,
        bfc,
        bpca_adv: adv,
        bpcs,
        bpos,
        bxcf,
        int_sts,
        flush_done,
        ts_touched: ts_scan.zeroed + ts_scan.overwritten,
        pool_touched: pool_scan.zeroed + pool_scan.overwritten,
    }
}

/// PI-V3D-68 — the frame-geometry / minimal-non-empty-frame battery.
///
/// Poison-honest and hub-gated exactly like `[v3d63]`/`[v3d64]`/`[v3d66]`: an explicit `SKIPPED` line
/// when the block is down, and a rung that did not run is REPORTED as not-run rather than folded in as a
/// zero. QEMU raspi4b models no V3D block and returns at `BLOCK-DOWN` long before this runs, so the
/// whole battery is dormant there and `kernel8-test` green for this arc means NO REGRESSION and nothing
/// more — every verdict below is metal-attended.
fn v3d68_binner_wall() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: [v3d68] SKIPPED — hub IDENT0 reads 0x00000000 (block absent/unpowered; the QEMU raspi4b signature). No frame geometry was varied this boot and NO verdict is implied ::"
            );
            return;
        }
        V3dPresence::Poison(w) => {
            serial_println!(
                ":: V3D: [v3d68] SKIPPED — hub IDENT0 reads poison {:#010x} (open-bus / firmware fill, not a live register). No frame geometry was varied this boot and NO verdict is implied ::",
                w
            );
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d68] binwall — varying the ONE input this campaign has never varied: the FRAME. Every rung ever kicked ([v3d40], the six [v3d48] rungs, [v3d50/51/53], [v3d63]'s 2x2, [v3d64]/[v3d66]'s L3 legs, the M4 draw) programmed the same 64x64 TILE_BINNING_MODE_CFG = exactly ONE 64x64 tile, the most degenerate legal frame there is. Ladder: {} rungs, Empty at 1 / 4 / 16 tiles then the smallest non-empty frame at 16 tiles, each with the [v3d48] kick sequence and its ~0.5 s FLDONE backstop, each writing a POISONED {} B tile-state array (16x the old 256 B detector) and a poisoned {} B pool. Reads per rung: retire, the CT0/PCS words, the PTB reservation pointer against Mesa's per-tile formula, and whether a single PTB byte landed anywhere ::",
        V3D68_LADDER.len(), V3D68_TSDA_BYTES, BIN_TILEALLOC_BYTES
    );

    let mut reads = [V3d68Read::NOT_RUN; 4];
    let mut i = 0usize;
    while i < V3D68_LADDER.len() {
        let (name, g, content) = V3D68_LADDER[i];
        reads[i] = v3d68_run_rung(name, &g, content);
        i += 1;
    }

    // ── Aggregate the ladder ─────────────────────────────────────────────────────────────────────────
    let mut kicked = 0u32;
    let mut retired_any = false;
    let mut retired_mask = 0u32;
    let mut wrote_any = false;
    // Evidence-integrity gate for the poison column ([v3d55]/[v3d56] rule): an "untouched" readback is
    // only evidence when the L2T write-back it was taken behind actually completed. Any rung whose drain
    // did not finish disqualifies the "PTB wrote nothing" reading for the WHOLE ladder — it is never
    // silently downgraded to a clean result.
    let mut drains_all_done = true;
    // BXCF is the one PTB configuration register this campaign has ever observed and never controlled.
    // Track it across the rungs so the verdict can say whether it is inert state or something that moves.
    let mut bxcf_first: Option<u32> = None;
    let mut bxcf_stable = true;
    let mut i = 0usize;
    while i < reads.len() {
        if reads[i].kicked {
            kicked += 1;
            if reads[i].retired {
                retired_any = true;
                retired_mask |= 1 << i;
            }
            if reads[i].ts_touched != 0 || reads[i].pool_touched != 0 {
                wrote_any = true;
            }
            if !reads[i].flush_done {
                drains_all_done = false;
            }
            match bxcf_first {
                None => bxcf_first = Some(reads[i].bxcf),
                Some(b) => {
                    if reads[i].bxcf != b {
                        bxcf_stable = false;
                    }
                }
            }
        }
        i += 1;
    }
    if kicked < 2 {
        serial_println!(
            ":: V3D: [v3d68] binwall INCONCLUSIVE — only {} of {} rungs kicked, so there is no differential to read. A geometry ladder needs at least two frames to compare; NOTHING here is a measurement and no candidate is excluded. This is explicitly NOT a clean result ::",
            kicked, V3D68_LADDER.len()
        );
        return;
    }

    // Did ANY register reading respond to the frame shape? Compare every kicked Empty rung against the
    // first kicked one. The `Full` rung is excluded from this comparison — it varies content as well as
    // shape, so a difference there would not be attributable to geometry.
    let mut base: Option<usize> = None;
    let mut shape_responded = false;
    let mut bpca_responded = false;
    let mut i = 0usize;
    while i < reads.len() {
        // Ladder entries 0..2 are the Empty rungs; entry 3 is the Full one (see V3D68_LADDER).
        if reads[i].kicked && i < 3 {
            match base {
                None => base = Some(i),
                Some(b) => {
                    if reads[i].shape_signature() != reads[b].shape_signature() {
                        shape_responded = true;
                    }
                    if reads[i].bpca_adv != reads[b].bpca_adv {
                        bpca_responded = true;
                    }
                }
            }
        }
        i += 1;
    }
    // Did the minimal non-empty frame at a non-degenerate geometry behave differently from the empty one
    // at the SAME geometry? Entries 2 and 3 are the matched pair.
    let content_pair = reads[2].kicked && reads[3].kicked;
    let content_responded =
        content_pair && reads[2].shape_signature() != reads[3].shape_signature();

    let verdict = if retired_any {
        "A BIN FRAME RETIRED. The wall is not unconditional: the frame the PTB was asked to tile was the wall, or part of it. Read the retired-mask against the ladder — a retire on a multi-tile EMPTY frame means the one-tile frame is the defect and every 'empty does not retire' verdict in this campaign was measured on a degenerate frame; a retire only on the non-empty rung means a 4.2 binner does not close a frame it was given nothing to bin, and the campaign's most load-bearing premise is RETRACTED. Either way the next arc is a bring-up arc, not a probe arc"
    } else if !drains_all_done {
        "no rung retired, and the poison column CANNOT be read: at least one rung's L2T write-back did not complete inside its backstop, so bytes the PTB may have written could still be parked in the cache. The retire and register columns above stand on their own (they are MMIO reads, not memory reads); the 'PTB wrote nothing' reading is INCONCLUSIVE for this ladder and must not be cited. Instrument first — re-run and read the drain-completion column before any memory-side verdict"
    } else if wrote_any {
        "no rung retired, but the PTB WROTE — poison was disturbed in the tile-state array or the pool on at least one rung. That is the first bin-side memory traffic this campaign has ever caught, and it was caught by growing the detector, not by changing the engine. The wall is now strictly downstream of the write: item-accept and pool-write both happen, and only frame-close does not. Take the touched extents per rung as the next arc's handle"
    } else if shape_responded || bpca_responded {
        "no rung retired and no byte landed, but the frame unit IS RESPONSIVE to the frame shape — a reading moved with tile count. Whatever moved is a live handle on a unit the campaign has been treating as inert: it proves the geometry reaches the PTB front end, which means the front end is running and the wall sits behind a stage that geometry can still reach. Name the moving column and drive the next arc off it"
    } else {
        "no rung retired, no byte landed, and EVERY reading is bit-identical across 1, 4 and 16 tiles and across empty vs non-empty content. Frame geometry is now EXCLUDED with the same force as encoding, submission, cache mode, reset, MMU and clock: the bin pipeline responds to nothing this driver can put in front of it. That removes the last shape- or content-shaped theory on the board and leaves exactly two never-written mainline-defined configuration registers — PTB BXCF (printed per rung above, read-only here) and MISCCFG.QRMAXCNT (disarmed since V3D-55) — plus the possibility that this block needs an initialisation this campaign has not found. Report the BXCF column with this verdict; it is the only PTB configuration state we have ever observed and never controlled"
    };
    serial_println!(
        ":: V3D: [v3d68] binwall verdict — kicked={}/{} | retired-any={} (mask={:#06b}) | PTB wrote anywhere={} (L2T drains all completed={} — the poison column is evidence only when this is 1) | register readings responded to tile count={} | BPCA reservation responded to tile count={} | empty-vs-non-empty at the same 16-tile geometry differed={} (pair present={}) | BXCF={:#010x} stable across rungs={} (READ ONLY; never written by us or by mainline) | tiles per rung = {}/{}/{}/{} — {} ::",
        kicked, V3D68_LADDER.len(), retired_any as u32, retired_mask,
        wrote_any as u32, drains_all_done as u32,
        shape_responded as u32, bpca_responded as u32,
        content_responded as u32, content_pair as u32,
        bxcf_first.unwrap_or(0), bxcf_stable as u32,
        reads[0].tiles, reads[1].tiles, reads[2].tiles, reads[3].tiles,
        verdict,
    );
    serial_println!(
        ":: V3D: [v3d68] binwall scope — this battery varied FRAME GEOMETRY and FRAME CONTENT and nothing else. It did NOT write BXCF, MISCCFG.QRMAXCNT or CTRSTA, did not touch the GMP or MMU_CTL, opened no new MMIO window, and left the real M4 draw untouched. The BPCA columns are read against Mesa v3d_util.c::v3d_tile_alloc_sizes (the PTB reserves the INITIAL block PER TILE at START_TILE_BINNING, then allocates aligned 4 KiB chunks); a match at BOTH tile-size hypotheses on the same rung means the prediction did not discriminate on this frame and the tile size stays as established, NOT as measured ::"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// PI-V3D-69 — the initialisation diff: read the FABRIC, not the engine.
//
// Every arc from V3D-40 to V3D-68 probed state this driver puts in front of the block: the control
// list, its placement, the caches around it, the reset before it, the interrupt policy over it, the
// counter bank across it, and finally (V3D-68) the frame itself. V3D-68's verdict named what is left:
// PTB `BXCF` (which mainline defines and writes from no path), `MISCCFG.QRMAXCNT` (disarmed since
// V3D-55), or an initialisation this campaign has not found. This arc goes after the third.
//
// The reconstruction of mainline's cold-power → first-bin-kick path (see v3d.md §41) puts exactly four
// things between the SoC and a binner that can master a write, and this driver has only ever WRITTEN
// three of them and never READ any of them back as a verdict:
//   1. PM_GRAFX — the graphics power-domain word. Mainline's BCM2711 path writes only PM_V3DRSTN here
//      (`bcm2835_power_power_on` returns early when `rpivid_asb` is present), so the isolation/power
//      bits are the FIRMWARE's business and the honest expectation is "start4.elf left them sane".
//   2. ASB_V3D_M_CTRL / ASB_V3D_S_CTRL — the two async AXI bridges. Released = REQ_STOP clear AND ACK
//      clear. We write the release; we have never read the words back as an independent verdict, and
//      we have never looked at the FIFO half of the word (EMPTY/FULL) at all.
//   3. Which ASB BLOCK backs those two offsets — `bcm2835_asb_control` re-routes them to `rpivid_asb`
//      on BCM2711. If that routing is wrong in this driver, our release landed on a dead window.
//   4. V3D_MMUC_CONTROL.ENABLE — the MMU cache. `v3d_mmu_flush_all` writes FLUSH|ENABLE and this
//      driver mirrors that write, but nothing has ever read the register back to confirm the ENABLE
//      bit LATCHED.
//
// **The hypothesis this arc was built to test, and why it is ranked LAST, not first.** The attractive
// story is that the binner's AXI path through the master bridge was never enabled: the block would
// then be alive on the register bus (every MMIO read works, jobs latch, registers echo) yet unable to
// master a write to memory — which matches every observation this campaign has, including the total
// absence of an error or an interrupt. It does not survive contact with the render engine. A CT1
// render job on this same block, this same boot, retires and lands CPU-verified pixels in arena memory
// (`[v3d58] xengine`); the CLE fetches every bin control list out of DRAM and `CT0CA` walks it to `EA`;
// and the V3D MMU's own page-table walk is a memory read. V3D 4.2 puts ONE master port behind the hub
// MMU per core — the PTB does not have a private one — so those three facts are direct evidence that
// the master bridge passes both reads and writes. A stopped master bridge would take the render path
// and the list fetch down with the binner, and it demonstrably does not.
//
// That is a refutation on inference, though, and the inference rests on a block diagram this tree has
// never read a register out of. The stations below cost four MMIO reads and settle the branch on the
// wire instead: if the words come back released, the hypothesis is closed with a measurement rather
// than an argument, and §41's ranking stands unchanged; if either bridge reads STOPPED, the campaign's
// model of its own block was wrong and the render evidence has to be re-read.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// One station's read of the SoC↔V3D fabric. Pure MMIO reads, no writes, no waits.
#[derive(Clone, Copy)]
struct V3d69Fabric {
    pm_grafx: u32,
    m: u32,    // rpivid_asb ASB_V3D_M_CTRL — the master bridge, the one a PTB write would cross
    s: u32,    // rpivid_asb ASB_V3D_S_CTRL
    lbrdg: u32, // legacy asb +0x20 ASB_AXI_BRDG_ID — the ONE in-DT-range identity word (PI-V3D-70)
    lm: u32,   // legacy asb ASB_V3D_M_CTRL — the routing falsifier
    ls: u32,   // legacy asb ASB_V3D_S_CTRL
    mmuc: u32, // V3D_MMUC_CONTROL — the MMU cache enable, written by us and never read back
    // PI-V3D-71 leg 3 — MMU_DEBUG_INFO (hub +0x1238), the hub-MMU capability word. Both piOS dumps
    // (idle AND mid-render) read 0x00000550; the dump-diff table carried our side as UNKNOWN because
    // the only prior read of it (`program_mmu`'s readback, `vio_decode`'s width fetch) never printed
    // under a station tag the diff could line up against. One read closes that row.
    dbg: u32,
}

impl V3d69Fabric {
    /// Released, per `bcm2835_asb_control(.., enable=true)`: REQ_STOP written clear and ACK observed
    /// clear. Both halves, because a bridge that acknowledges STOPPED while REQ_STOP reads clear is a
    /// bridge that did not take our write.
    fn released(w: u32) -> bool {
        w & ASB_REQ_STOP == 0 && w & ASB_ACK == 0
    }
    fn m_released(&self) -> bool {
        Self::released(self.m)
    }
    fn s_released(&self) -> bool {
        Self::released(self.s)
    }
    /// PI-V3D-70 — the ASB identity check, on the ONE register the DT actually covers: the legacy
    /// block's `ASB_AXI_BRDG_ID` at +0x20, which must read the ASCII tag 'brdg'. A match says the
    /// `pm`-node reg table this driver derives BOTH ASB bases from is the live one; it is an
    /// identity check on the ASB address space, not on any single bridge's state.
    fn brdg_id_ok(&self) -> bool {
        self.lbrdg == ASB_BRDG_ID_EXPECT
    }
    /// The whole fabric as mainline would leave it after `bcm2835_asb_power_on` + the MMU set.
    fn clean(&self) -> bool {
        self.pm_grafx & PM_V3DRSTN != 0
            && self.m_released()
            && self.s_released()
            && self.mmuc & V3D_MMUC_CONTROL_ENABLE != 0
    }
    /// Everything that could differ between two stations, folded to one word for the differential.
    fn signature(&self) -> u32 {
        self.pm_grafx ^ self.m.rotate_left(8) ^ self.s.rotate_left(16) ^ self.mmuc.rotate_left(24)
    }
}

/// Take one fabric station. READ ONLY — eight MMIO loads, every one of them inside the Device-nGnRnE
/// window boot.rs already maps, and every one of them poison-tolerant (an absent block reads 0 or
/// 0xffffffff and the emitter says so rather than decoding it).
fn v3d69_read_fabric() -> V3d69Fabric {
    V3d69Fabric {
        pm_grafx: mmio_read(PM_BASE, PM_GRAFX),
        m: mmio_read(RPIVID_ASB_BASE, ASB_V3D_M_CTRL),
        s: mmio_read(RPIVID_ASB_BASE, ASB_V3D_S_CTRL),
        lbrdg: mmio_read(LEGACY_ASB_BASE, ASB_AXI_BRDG_ID),
        lm: mmio_read(LEGACY_ASB_BASE, ASB_V3D_M_CTRL),
        ls: mmio_read(LEGACY_ASB_BASE, ASB_V3D_S_CTRL),
        mmuc: mmio_read(V3D_HUB_BASE, V3D_MMUC_CONTROL),
        dbg: mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO), // PI-V3D-71 leg 3
    }
}

/// Emit one `[v3d69] fabric` station: the raw words first, the decodes second, the caveats attached to
/// the fields that carry them.
fn v3d69_emit_fabric(station: &str, f: &V3d69Fabric) {
    serial_println!(
        ":: V3D: [v3d69] fabric ({}) — PM_GRAFX={:#010x} V3DRSTN(bit6, CORROBORATED)={} | UNCERTAIN map: POWUP={} POWOK={} ISFUNC={} MRDONE={} MEMREP={} ISPOW={} ENAB={} (mainline's BCM2711 path never writes these — bcm2835_power_power_on returns early when rpivid_asb is present — so they are the FIRMWARE's, and the expectation is 'start4.elf left them sane', not 'Linux programmed them') ::",
        station,
        f.pm_grafx,
        (f.pm_grafx & PM_V3DRSTN != 0) as u32,
        (f.pm_grafx & PM_POWUP != 0) as u32,
        (f.pm_grafx & PM_POWOK != 0) as u32,
        (f.pm_grafx & PM_ISFUNC != 0) as u32,
        (f.pm_grafx & PM_MRDONE != 0) as u32,
        (f.pm_grafx & PM_MEMREP != 0) as u32,
        (f.pm_grafx & PM_ISPOW != 0) as u32,
        (f.pm_grafx & PM_ENAB != 0) as u32,
    );
    serial_println!(
        ":: V3D: [v3d70] fabric ({}) rpivid_asb@{:#x} (DT-VERIFIED base, `pm` node reg-names[2] VC 0x7ec11000, len 0x20) — NO IDENTITY REGISTER IN THE DT RANGE: mainline's ASB_AXI_BRDG_ID sits at +0x20, one word PAST this block's 0x20-long range, so this window has no in-range identity word and none is printed. [v3d69]'s BRDG_VERSION column read +0x00 here and called its 0x00000000 a dead window — that was a read of a NON-IDENTITY offset and is retired; see the legacy-asb line for the identity check that the DT does cover | M_CTRL={:#010x} REQ_STOP={} ACK={} EMPTY={} FULL={} released={} | S_CTRL={:#010x} REQ_STOP={} ACK={} EMPTY={} FULL={} released={} — the MASTER word is the one a PTB write would have to cross ::",
        station,
        RPIVID_ASB_BASE,
        f.m,
        (f.m & ASB_REQ_STOP != 0) as u32,
        (f.m & ASB_ACK != 0) as u32,
        (f.m & ASB_EMPTY != 0) as u32,
        (f.m & ASB_FULL != 0) as u32,
        f.m_released() as u32,
        f.s,
        (f.s & ASB_REQ_STOP != 0) as u32,
        (f.s & ASB_ACK != 0) as u32,
        (f.s & ASB_EMPTY != 0) as u32,
        (f.s & ASB_FULL != 0) as u32,
        f.s_released() as u32,
    );
    serial_println!(
        ":: V3D: [v3d70] fabric ({}) legacy asb@{:#x} (DT-VERIFIED base, `pm` node reg-names[1] VC 0x7e00a000, len 0x24) — ASB_AXI_BRDG_ID(+0x20)={:#010x} expect={:#010x} ('brdg') match={} — THE identity check: the 0x24-long legacy range is the only ASB range in the DT that reaches +0x20, so this one word is what decides whether the `pm`-node reg table both ASB bases come from is the live address space. A match validates the ADDRESS SPACE, not any bridge's state | M_CTRL={:#010x} S_CTRL={:#010x} — routing falsifier: bcm2835_asb_control re-routes ASB_V3D_{{S,M}}_CTRL to rpivid_asb whenever that block is present, which on BCM2711 is always, so these two words should be some OTHER peripheral's bridges or dead. If the legacy words look like released V3D bridges and the rpivid ones do not, this driver has been releasing the wrong window since PI-V3D-3 ::",
        station,
        LEGACY_ASB_BASE,
        f.lbrdg,
        ASB_BRDG_ID_EXPECT,
        f.brdg_id_ok() as u32,
        f.lm,
        f.ls,
    );
    serial_println!(
        ":: V3D: [v3d69] fabric ({}) V3D_MMUC_CONTROL={:#010x} ENABLE={} FLUSH={} — mainline v3d_mmu_flush_all writes FLUSH|ENABLE and this driver mirrors it in program_mmu; this is the FIRST read-back. ENABLE=0 here means the MMU cache never latched on and every V3D memory access this campaign has measured ran with it off ::",
        station,
        f.mmuc,
        (f.mmuc & V3D_MMUC_CONTROL_ENABLE != 0) as u32,
        (f.mmuc & V3D_MMUC_CONTROL_FLUSH != 0) as u32,
    );
    // PI-V3D-71 leg 3 — the one UNKNOWN-ours row of the piOS dump-diff table, closed with one read.
    // Decode per mainline v3d_drv.c: va_width = 30 + DEBUG_INFO[7:4], pa_width = 30 + DEBUG_INFO[11:8]
    // (the same fields `vio_decode` has used since boot-P6, where 0x550 → va_width 35 was the ground
    // truth that decoded the CLE fault VA). This is a hardware capability word, not programmed state:
    // a mismatch against mainline's 0x550 would say the hub MMUs themselves differ; a match removes it.
    let va_f = (f.dbg >> 4) & 0xF;
    let pa_f = (f.dbg >> 8) & 0xF;
    let dbg_match = f.dbg == 0x0000_0550;
    serial_println!(
        ":: V3D: [v3d71] fabric ({}) MMU_DEBUG_INFO(hub +0x1238)={:#010x} — VA_WIDTH field {} (va {} bits) PA_WIDTH field {} (pa {} bits) VERSION nibble {} (v3d_drv.c decode) | mainline piOS dumps read 0x00000550 in BOTH the idle and mid-render captures; match={} — {} ::",
        station,
        f.dbg,
        va_f, 30 + va_f,
        pa_f, 30 + pa_f,
        f.dbg & 0xF,
        dbg_match as u32,
        if dbg_match {
            "the hub-MMU capability word is bit-identical to working mainline; the dump-diff table's one UNKNOWN-ours row closes as EXCLUDED MEASURED"
        } else {
            "the word DIFFERS from mainline's 0x550 — a hub-MMU capability divergence between the two systems; report it, and re-read every VIO_ADDR decode in this campaign against the va_width printed here"
        },
    );
}

/// PI-V3D-69 — the fabric probe. Two read-only stations either side of one banked `Empty` control rung,
/// then the verdict; and, on its own knob and only on the one outcome branch that makes it meaningful,
/// the mainline re-enable + re-kick.
///
/// Placed at the very tail of `empty_frame_bisection`, after `[v3d68]` — the same reasoning every
/// battery before it carries (it must not perturb a reading another verdict is banked on), plus its
/// own: the pre-station is meant to read the fabric as it stood through the WHOLE diagnostic battery,
/// so it wants every other rung already behind it.
fn v3d69_fabric_probe() {
    if !V3D69_FABRIC {
        return;
    }
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: [v3d69] SKIPPED — hub IDENT0 reads 0x00000000 (block absent/unpowered; the QEMU raspi4b signature). The PM/ASB fabric was NOT read this boot and NO verdict is implied — in particular, QEMU models neither the rpivid_asb block nor V3D, so a fabric reading here would be a reading of nothing ::"
            );
            return;
        }
        V3dPresence::Poison(w) => {
            serial_println!(
                ":: V3D: [v3d69] SKIPPED — hub IDENT0 reads poison {:#010x} (open-bus / firmware fill, not a live register). The PM/ASB fabric was NOT read this boot and NO verdict is implied ::",
                w
            );
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d69] initdiff — nine arcs probed the state this driver puts in front of the block and [v3d68] excluded the last of it. This one reads the FABRIC BETWEEN the SoC and the block instead: PM_GRAFX, both V3D async-AXI-bridge control words in BOTH candidate ASB blocks, and the MMU-cache enable — every one of them written at some point by this driver or by firmware, and not one of them ever READ BACK as a verdict. Two read-only stations either side of one banked Empty control rung. The bridge hypothesis (a master bridge that passes register reads but not memory writes) is ranked LAST and not first, because a CT1 render frame retires and lands pixels through the same single master port this same boot — see the [v3d58] xengine line. These stations settle that branch on the wire instead of on inference ::"
    );

    let pre = v3d69_read_fabric();
    v3d69_emit_fabric("pre-rung ", &pre);

    // One banked Empty control rung, so the post-station reads the fabric as a bin kick left it. This
    // is the [v3d48] rung verbatim (same content, same CL offset, same cache maintenance, same ~0.5 s
    // FLDONE backstop) — the arc adds no new engine behaviour, only two stations around it.
    let rung = submit_bisect_rung_tagged("v3d69", "fabric-control", BinContent::Empty, OFF_SHADREC);
    let _ = rung;

    let post = v3d69_read_fabric();
    v3d69_emit_fabric("post-rung", &post);

    let moved = pre.signature() != post.signature();
    let m_fifo_moved = (pre.m & (ASB_EMPTY | ASB_FULL)) != (post.m & (ASB_EMPTY | ASB_FULL));
    let id_ok = pre.brdg_id_ok();
    let verdict = if !id_ok {
        "the legacy ASB's ASB_AXI_BRDG_ID does NOT read 'brdg'. The `pm`-node reg table both ASB bases are derived from is DT-verified, so a mismatch here is not an address guess going wrong — it is the ASB address space not answering as mainline's own probe expects. Until that is explained no bridge column above may be cited: the release writes since PI-V3D-3 went to a window whose identity failed its own check. STOP and report; do not run another rung"
    } else if !pre.m_released() || !pre.s_released() {
        "A V3D ASB BRIDGE IS NOT RELEASED. The block answers every register read and latches every job while a bridge that its memory traffic must cross reads STOPPED. This contradicts the render evidence ([v3d58] xengine: a CT1 frame retired and landed CPU-verified pixels through what this campaign believed was the same single master port) — so either the two engines do NOT share a master, or one of these two readings is not what it appears. Do not act on this line alone: re-read it against the xengine verdict and the legacy-block routing column above, and only then arm UNAOS_V3D69_REENABLE"
    } else if pre.mmuc & V3D_MMUC_CONTROL_ENABLE == 0 {
        "both bridges are released and PM_GRAFX has V3DRSTN deasserted, but V3D_MMUC_CONTROL.ENABLE READS BACK CLEAR — the MMU cache this driver believed it enabled in program_mmu never latched. That is a genuine initialisation hole and the first one this arc has found: mainline's v3d_mmu_flush_all leaves ENABLE set and every V3D memory access it makes runs behind it. It does NOT by itself explain a bin-exclusive wall (the render path crosses the same MMU), so it is a defect to fix and then re-measure, not a verdict on the binner"
    } else {
        "THE FABRIC IS CLEAN AND ITS IDENTITY IS CONFIRMED. The legacy ASB reads the 'brdg' tag at the DT-covered +0x20, so the `pm`-node reg table both bases come from is the live address space; PM_GRAFX has V3DRSTN deasserted; both V3D async AXI bridges read released (REQ_STOP=0, ACK=0) in the rpivid_asb block mainline routes them to, with real acks either side of a stop/release; and the MMU cache reads ENABLEd. Fabric identity was the last undecidable column and it is now decided, so the BRIDGE/ROUTING BRANCH CLOSES — measured, not inferred from the render path — and PM/ASB power sequencing joins encoding, submission, cache mode, reset, MMU, clock, frame geometry and frame content on the excluded list. The wall's remaining candidates are BXCF, MISCCFG.QRMAXCNT, or deeper block init; nothing below this line is in the fabric"
    };
    serial_println!(
        ":: V3D: [v3d70] initdiff verdict — ASB_AXI_BRDG_ID match={} | PM_GRAFX V3DRSTN={} | M released={} S released={} | MMUC ENABLE={} | fabric changed across the bin rung={} (master FIFO EMPTY/FULL changed={}) | fabric clean vs mainline={} — {} ::",
        id_ok as u32,
        (pre.pm_grafx & PM_V3DRSTN != 0) as u32,
        pre.m_released() as u32,
        pre.s_released() as u32,
        (pre.mmuc & V3D_MMUC_CONTROL_ENABLE != 0) as u32,
        moved as u32,
        m_fifo_moved as u32,
        pre.clean() as u32,
        verdict,
    );
    serial_println!(
        ":: V3D: [v3d69] initdiff scope — the stations above are READS. Nothing in this arc wrote PM_GRAFX, either ASB block, MMUC_CONTROL, BXCF, MISCCFG or CTRSTA; the one rung is the existing [v3d48] Empty kick unchanged; the real M4 draw is untouched. The FIFO (EMPTY/FULL) columns are a two-sample read either side of a ~0.5 s wait, NOT a sampler: 'EMPTY across both stations' is consistent with a bridge that was busy and drained, and is evidence of NOTHING on its own. Only the REQ_STOP/ACK columns carry a verdict ::"
    );

    v3d69_reenable_rung(&pre);
}

/// PI-V3D-69 — the WRITE half, on its own `UNAOS_V3D69_REENABLE` knob. Runs the mainline enable
/// sequence (`enable_pm_asb`: deassert PM_V3DRSTN, then release the master and the slave bridge, each
/// with the PM password and a bounded ACK wait) and re-kicks the `[v3d69]` control rung, so the retire
/// verdict either side of the re-enable is a same-boot differential.
///
/// It DECLINES to run when the read half found the fabric already clean. That is not timidity: writing
/// a release to an already-released bridge is a perturbation of a live block that can only add a
/// confound to every reading banked after it, and the outcome would be uninterpretable either way.
fn v3d69_reenable_rung(pre: &V3d69Fabric) {
    if !V3D69_REENABLE {
        return;
    }
    if pre.clean() {
        serial_println!(
            ":: V3D: [v3d69] reenable DECLINED — UNAOS_V3D69_REENABLE is armed, but the read half found the fabric CLEAN (V3DRSTN deasserted, both bridges released, MMUC enabled). Re-running the enable sequence on an already-enabled fabric writes PM_GRAFX and both bridge words for nothing and would leave every later reading on this boot standing behind an unnecessary perturbation. The rung is gated on the finding, not on the knob; nothing was written ::"
        );
        return;
    }
    serial_println!(
        ":: V3D: [v3d69] reenable ARMED — the read half found the fabric NOT clean, so the mainline enable sequence (bcm2835_asb_power_on's ON half: set PM_V3DRSTN with the PM password, then clear ASB_REQ_STOP on the MASTER and then the SLAVE, each with a bounded ACK-clear wait) runs now and the [v3d69] control rung is re-kicked behind it. Read the two rungs as a differential: a retire ONLY on the second means the fabric was the wall ::"
    );
    enable_pm_asb();
    settle_ms(2);
    let after = v3d69_read_fabric();
    v3d69_emit_fabric("re-enabled", &after);
    let _ = submit_bisect_rung_tagged(
        "v3d69",
        "fabric-control-after-reenable",
        BinContent::Empty,
        OFF_SHADREC,
    );
    let fixed = after.clean();
    serial_println!(
        ":: V3D: [v3d69] reenable verdict — fabric clean AFTER the sequence={} (M released {}->{}, S released {}->{}, V3DRSTN {}->{}) — {} ::",
        fixed as u32,
        pre.m_released() as u32,
        after.m_released() as u32,
        pre.s_released() as u32,
        after.s_released() as u32,
        (pre.pm_grafx & PM_V3DRSTN != 0) as u32,
        (after.pm_grafx & PM_V3DRSTN != 0) as u32,
        if fixed {
            "the enable sequence MOVED the fabric into the mainline state. Read the two [v3d69] fabric-control rungs above: if the second retired and the first did not, the wall was the un-released fabric and this is the end of the campaign's bin question; if neither retired, the fabric was genuinely not the wall and this arc has excluded it by construction rather than by inference"
        } else {
            "the enable sequence did NOT move the fabric into the mainline state — the bridge words did not take the release even with the PM password. That is a hardware-or-address finding, not a driver one: either the block base is wrong (see the legacy-asb routing column) or the bridge is held by something outside this driver's reach. STOP and report; do not iterate on writes to a window that does not answer"
        },
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// PI-V3D-71 — the piOS dump-diff discriminator: mainline-geometry iovas + the in-flight fabric read.
//
// The two attended piOS dumps (v3d-dump-{idle,mid-render}, bench capture 2026-07-29) were taken on
// working mainline on THIS Pi 4, with v3d+vc4 loaded — the first ground-truth register images of a
// binner that demonstrably closes frames. Diffed against our side, the dumps EXCLUDE the two named
// suspects [v3d68] left standing: PTB `BXCF` reads 0x00000000 on working mainline (bit-identical to
// ours — MEASURED, not inferred from "no write path") and `MISCCFG` reads 0x00000006 (ditto). Do not
// re-litigate either. Frame shape, frame content and fabric identity were all excluded prior.
//
// What the diff leaves as PTB-visible differences between the two systems is the ADDRESS GEOMETRY of
// the bin frame: mainline's buffers live behind its MMU at iovas in the tens of MiB — tile-alloc pool
// CT0QMA=0x0129A000/0x09BDF000 with CT0QMS=0x0008B000/0x00088000 (~544 KiB), bin CL CT0QBA at
// 0x03033000/0x0338A000, tile-state CT0QTS at 0x0132_5002/0x0437F002 — while every rung this campaign
// ever kicked handed the PTB identity-mapped arena addresses under 0x400000 and a 32 KiB pool. Leg 1
// rebuilds OUR page table so the SAME physical arena appears at mainline-LIKE iovas (pool ~0x03000000+
// sized at the dump's 0x88000; CL and tile-state above it) and kicks the banked Empty rung once.
// Leg 2 answers the question no exit-state read can: DOES A SINGLE BEAT ENTER THE FABRIC while the
// frame is open? The mid-render dump shows rpivid_asb `M_CTRL`=0x00008000 under live master writes
// (0x00000004 = FIFO-EMPTY quiescent, the idle dump's word and our own [v3d70] reading); sampling it
// DURING the FLDONE wait converts "no bytes landed" into either "no beat ever issued" (core-internal
// wall) or "beats issue and die downstream" (fabric re-opens, measured in-flight). Leg 3 closes the
// dump-diff table's one UNKNOWN-ours row (MMU_DEBUG_INFO) inside the [v3d69] fabric stations.
//
// Constraint honoured throughout: NO off-mainline config-register writes. Leg 1 is a page-table +
// job-address change in OUR allocator (the CT0Q* writes are the same per-job latches every rung
// makes; the MMU flush is mainline's own v3d_mmu_flush_all idiom program_mmu already mirrors);
// legs 2 and 3 are pure reads.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The v3d71 iova window (all page-aligned). Pool at the dump's neighbourhood and SIZE; CL and
/// tile-state ABOVE it, with an unmapped guard page between pool and CL so a pool overrun faults
/// rather than corrupting the list.
const V3D71_POOL_IOVA: usize = 0x0300_0000; // mainline-like bin tile-alloc pool base
const V3D71_POOL_BYTES: usize = 0x88000; // 544 KiB — the mid-render dump's CT0QMS, verbatim
const V3D71_CL_IOVA: usize = 0x0308_9000; // bin CL, above the pool (mainline places its CL above too)
const V3D71_TS_IOVA: usize = 0x0308_a000; // tile-state array, 24 KiB (the [v3d68] poisoned detector)
const V3D71_WINDOW_END: usize = V3D71_TS_IOVA + V3D68_TSDA_BYTES;
const _: () = assert!(V3D71_POOL_IOVA % PAGE == 0 && V3D71_POOL_BYTES % PAGE == 0);
const _: () = assert!(V3D71_POOL_IOVA + V3D71_POOL_BYTES + PAGE <= V3D71_CL_IOVA); // guard page
const _: () = assert!(V3D71_CL_IOVA + PAGE <= V3D71_TS_IOVA);
const _: () = assert!(V3D71_WINDOW_END / PAGE <= PT_CAP); // the grown table covers the whole window
const _: () = assert!(V3D71_TS_IOVA % 32 == 0); // CT0QTS carries the base in its upper bits
const _: () = assert!(BIN_TILEALLOC_BYTES % PAGE == 0); // the alias cycle below needs whole pages

/// PI-V3D-71 — map the window: pool iovas CYCLICALLY ALIASED onto the arena's 32 KiB physical pool
/// (same physical arena — the 544 KiB is iova-side only, and any PTB write through ANY alias lands in
/// the poisoned physical pool where the scan reads), CL page onto the probe-CL page, tile-state 1:1
/// onto the dedicated [v3d68] TSDA pages. Publishes the PTEs and runs mainline's flush (MMUC
/// FLUSH|ENABLE + MMU_CTL TLB_CLEAR, the program_mmu tail verbatim). Fail-closed on a flush timeout.
fn v3d71_map_window(map: bool) -> bool {
    let pt = &raw mut V3D_PT;
    let pool_first_pfn = (arena_phys() + OFF_BIN_TILEALLOC) >> V3D_MMU_PAGE_SHIFT;
    let pool_pages = BIN_TILEALLOC_BYTES / PAGE;
    let cl_pfn = (arena_phys() + OFF_PROBE_BIN_CL) >> V3D_MMU_PAGE_SHIFT;
    let ts_first_pfn = (arena_phys() + OFF_V3D68_TSDA) >> V3D_MMU_PAGE_SHIFT;
    unsafe {
        for p in 0..(V3D71_POOL_BYTES / PAGE) {
            (*pt).ptes[V3D71_POOL_IOVA / PAGE + p] = if map {
                V3D_PTE_VALID | V3D_PTE_WRITEABLE | (pool_first_pfn + (p % pool_pages)) as u32
            } else {
                0
            };
        }
        (*pt).ptes[V3D71_CL_IOVA / PAGE] =
            if map { V3D_PTE_VALID | V3D_PTE_WRITEABLE | cl_pfn as u32 } else { 0 };
        for p in 0..(V3D68_TSDA_BYTES / PAGE) {
            (*pt).ptes[V3D71_TS_IOVA / PAGE + p] = if map {
                V3D_PTE_VALID | V3D_PTE_WRITEABLE | (ts_first_pfn + p) as u32
            } else {
                0
            };
        }
    }
    // Publish the whole touched index span (pool..window-end covers the guard + CL + TS slots too —
    // the guard entry was never written and stays a published zero, which is exactly its job).
    cache::clean_range(
        pt_phys() + (V3D71_POOL_IOVA / PAGE) * 4,
        ((V3D71_WINDOW_END - V3D71_POOL_IOVA) / PAGE) * 4,
    );
    // Mainline's v3d_mmu_flush_all idiom, exactly as program_mmu's tail runs it.
    mmio_write(V3D_HUB_BASE, V3D_MMUC_CONTROL, V3D_MMUC_CONTROL_FLUSH | V3D_MMUC_CONTROL_ENABLE);
    mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, mmio_read(V3D_HUB_BASE, V3D_MMU_CTL) | V3D_MMU_CTL_TLB_CLEAR);
    let ok = wait_bit_clear(
        V3D_HUB_BASE,
        V3D_MMU_CTL,
        V3D_MMU_CTL_TLB_CLEARING,
        if map { "v3d71 window map TLB clear" } else { "v3d71 window unmap TLB clear" },
    );
    dsb();
    ok
}

/// PI-V3D-71 leg 2 — the in-flight fabric sampler's accumulator. Armed only across the `[v3d71]`
/// rung's FLDONE wait; `wait_fldone` folds one sample per ~1 ms tick when armed and never issues an
/// MMIO for it otherwise. Distinct-value tracking is capped at 4 words per register (mainline shows
/// exactly two: 0x4 quiescent, 0x8000 under live master writes); overflow is reported, never dropped.
struct V3d71Sampler {
    armed: bool,
    samples: u32,
    bm_active: u32, // samples taken while PCS.BMACTIVE read 1 (the frame provably open)
    m_min: u32,
    m_max: u32,
    s_min: u32,
    s_max: u32,
    m_vals: [u32; 4],
    m_n: u32,
    m_more: bool, // >4 distinct M_CTRL words seen (reported, not silently capped)
    s_vals: [u32; 4],
    s_n: u32,
    s_more: bool,
}

static mut V3D71_SAMPLER: V3d71Sampler = V3d71Sampler {
    armed: false,
    samples: 0,
    bm_active: 0,
    m_min: u32::MAX,
    m_max: 0,
    s_min: u32::MAX,
    s_max: 0,
    m_vals: [0; 4],
    m_n: 0,
    m_more: false,
    s_vals: [0; 4],
    s_n: 0,
    s_more: false,
};

/// Fold `v` into a capped distinct-value set. Returns overflow (a fifth distinct word arrived).
fn v3d71_fold_distinct(vals: &mut [u32; 4], n: &mut u32, v: u32) -> bool {
    let mut i = 0usize;
    while i < *n as usize {
        if vals[i] == v {
            return false;
        }
        i += 1;
    }
    if (*n as usize) < vals.len() {
        vals[*n as usize] = v;
        *n += 1;
        false
    } else {
        true
    }
}

/// Arm the sampler (reset the accumulator). Called immediately before the `[v3d71]` GO.
fn v3d71_sampler_arm() {
    let s = &raw mut V3D71_SAMPLER;
    unsafe {
        (*s) = V3d71Sampler {
            armed: true,
            samples: 0,
            bm_active: 0,
            m_min: u32::MAX,
            m_max: 0,
            s_min: u32::MAX,
            s_max: 0,
            m_vals: [0; 4],
            m_n: 0,
            m_more: false,
            s_vals: [0; 4],
            s_n: 0,
            s_more: false,
        };
    }
}

/// One in-flight sample: rpivid_asb M_CTRL + S_CTRL (pure reads of the DT-verified window [v3d70]
/// confirmed live) and PCS for the BMACTIVE qualifier. No-op unless armed.
fn v3d71_sampler_take() {
    let s = &raw mut V3D71_SAMPLER;
    unsafe {
        if !(*s).armed {
            return;
        }
        let m = mmio_read(RPIVID_ASB_BASE, ASB_V3D_M_CTRL);
        let sc = mmio_read(RPIVID_ASB_BASE, ASB_V3D_S_CTRL);
        let pcs = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
        (*s).samples = (*s).samples.saturating_add(1);
        if pcs & V3D_PCS_BMACTIVE != 0 {
            (*s).bm_active = (*s).bm_active.saturating_add(1);
        }
        (*s).m_min = (*s).m_min.min(m);
        (*s).m_max = (*s).m_max.max(m);
        (*s).s_min = (*s).s_min.min(sc);
        (*s).s_max = (*s).s_max.max(sc);
        let mut mv = (*s).m_vals;
        let mut mn = (*s).m_n;
        if v3d71_fold_distinct(&mut mv, &mut mn, m) {
            (*s).m_more = true;
        }
        (*s).m_vals = mv;
        (*s).m_n = mn;
        let mut sv = (*s).s_vals;
        let mut sn = (*s).s_n;
        if v3d71_fold_distinct(&mut sv, &mut sn, sc) {
            (*s).s_more = true;
        }
        (*s).s_vals = sv;
        (*s).s_n = sn;
    }
}

/// Disarm and emit the `[v3d71f]` witness. The reading key: mainline's M_CTRL reads 0x00000004
/// (FIFO-EMPTY quiescent) at idle and 0x00008000 under live master writes (bench dumps, this Pi 4).
/// Ours moving off the quiescent word during an OPEN bin frame = writes ISSUE into the fabric and die
/// downstream (fabric/routing re-opens, measured in-flight); ours pinned at the quiescent word across
/// the whole wait with BMACTIVE=1 = the PTB never issued a single beat (core-internal verdict).
fn v3d71_sampler_emit() {
    let s = &raw mut V3D71_SAMPLER;
    unsafe {
        (*s).armed = false;
        let smp = &*s;
        let m_moved = smp.m_n > 1 || smp.m_more;
        let s_moved = smp.s_n > 1 || smp.s_more;
        let verdict = if smp.samples == 0 {
            "ZERO samples landed — the wait exited before the first ~1 ms tick; INCONCLUSIVE, nothing here is a measurement"
        } else if m_moved || s_moved {
            "AN ASB WORD MOVED while the bin frame was in flight — the fabric saw state change during the wait. Mainline's live-master signature is M_CTRL=0x8000; if that word appears in the distinct list, PTB writes ISSUE into the fabric and die DOWNSTREAM of the bridge — the fabric/routing branch re-opens, this time on an in-flight measurement, not an exit-state read"
        } else if smp.bm_active > 0 {
            "M_CTRL and S_CTRL NEVER left their quiescent words across the whole wait while BMACTIVE=1 — on working mainline the master word reads 0x8000 under live writes, so a PTB that were emitting would have shown here. The PTB provably never issues a single beat into the fabric: the wall is CORE-INTERNAL, upstream of the master bridge, and no fabric/routing theory survives"
        } else {
            "BMACTIVE was never observed set across the sampled window — the sampler did not overlap an open bin frame and the pinned-quiescent reading carries NO verdict"
        };
        serial_println!(
            ":: V3D: [v3d71f] fabric-sampler — {} samples (~1 ms cadence across the FLDONE wait), BMACTIVE-set samples={} | M_CTRL min={:#010x} max={:#010x} distinct={}{} vals=[{:#010x} {:#010x} {:#010x} {:#010x}] | S_CTRL min={:#010x} max={:#010x} distinct={}{} vals=[{:#010x} {:#010x} {:#010x} {:#010x}] (unused slots print 0) | mainline key: 0x4=quiescent FIFO-EMPTY, 0x8000=live master writes (bench dumps) — {} ::",
            smp.samples,
            smp.bm_active,
            smp.m_min,
            smp.m_max,
            smp.m_n,
            if smp.m_more { "(+MORE, capped)" } else { "" },
            smp.m_vals[0], smp.m_vals[1], smp.m_vals[2], smp.m_vals[3],
            smp.s_min,
            smp.s_max,
            smp.s_n,
            if smp.s_more { "(+MORE, capped)" } else { "" },
            smp.s_vals[0], smp.s_vals[1], smp.s_vals[2], smp.s_vals[3],
            verdict,
        );
    }
}

// ─── PI-V3D-73 — does the CLE ever FETCH? The in-flight CLE-progress sampler + the submit diff ───
//
// P83 established (do not re-litigate): the core is CLOCKED AND COMPLETELY IDLE across the wedged
// wait ([v3d72] clkliv-x: Δsrc32=703,077,589 while src14/src16/src24 all read 0), the PTB never
// issues one beat into the fabric ([v3d71f]), BMACTIVE=1 with no fault and the poison intact. The
// wall is a unit-level enable/trigger — "clocked but never TRIGGERED" — and the open question is
// WHERE on the CLE→PTB path the trigger dies: does control-list thread 0 ever START (fetch its
// first word), or does the list execute and the FLUSH terminator fail to close the frame?
//
// The latent evidence, made explicit here: the P83 [v3d54] trace under the v3d71 mainline-geom
// rung printed `CT0CA off 0xfd2c2000` against BA=0x03089000 — i.e. raw CT0CA=0x0034b000, the
// PREVIOUS (identity-iova) kick's list address. Under the one kick whose QBA was unique in the
// whole campaign, CT0CA never held the new QBA at all. Every earlier rung reused the same CL
// address, so their `CT0CA == BA` readings could never distinguish "loaded QBA, never advanced"
// from "stale value that happens to equal QBA". The [v3d54] trace also reports CA as an OFFSET
// from BA, which turned that stale raw word into a misleading "(mid) stalled mid-list" verdict.
// This sampler reports the RAW word, tracks whether CA is ever INSIDE [QBA,QEA], and names the
// stale address when it is not.
//
// Design (the [v3d71f] idiom exactly): a static accumulator armed only around the rung(s) that
// want it; `wait_fldone` folds one sample per ~1 ms tick when armed (one static bool read, zero
// MMIO, on every other wait). Sampled registers: CT0CA (raw, min/max/distinct + in-span
// tracking vs the armed QBA/QEA), CT0CS (raw distinct words; only CTRUN(bit5) is decoded — the
// corroborated bit, per the §32/§40 CTnCS hedge), CT0LC, CT0PC, BFC. All five are seeded at arm
// time (pre-GO), so `first` is the before-kick word and a stale-hold reads as changes=0.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// One sampled register's fold: first(=pre-GO seed)/last/min/max, change count, distinct words
/// (capped at 4, overflow reported — the [v3d71f] rule: capped, never silently dropped).
#[derive(Clone, Copy)]
struct V3d73Reg {
    first: u32,
    last: u32,
    min: u32,
    max: u32,
    changes: u32,
    vals: [u32; 4],
    n: u32,
    more: bool,
}

impl V3d73Reg {
    const fn zeroed() -> Self {
        Self { first: 0, last: 0, min: 0, max: 0, changes: 0, vals: [0; 4], n: 0, more: false }
    }
    fn seed(&mut self, v: u32) {
        *self = Self { first: v, last: v, min: v, max: v, changes: 0, vals: [v, 0, 0, 0], n: 1, more: false };
    }
    fn fold(&mut self, v: u32) {
        if v != self.last {
            self.changes += 1;
            self.last = v;
        }
        self.min = self.min.min(v);
        self.max = self.max.max(v);
        let mut vals = self.vals;
        let mut n = self.n;
        if v3d71_fold_distinct(&mut vals, &mut n, v) {
            self.more = true;
        }
        self.vals = vals;
        self.n = n;
    }
}

struct V3d73Sampler {
    armed: bool,
    samples: u32,
    qba: u32, // the armed job's queue-begin — the address CA must hold if thread 0 ever loads it
    qea: u32, // the armed job's queue-end
    ca_in_span: u32,      // samples with QBA <= CT0CA <= QEA (the CLE provably holding OUR list)
    ca_eq_qba: u32,       // samples with CT0CA == QBA exactly (loaded, zero advance)
    ca_max_in_span: u32,  // highest in-span CT0CA seen — how far the fetch ever got vs QEA
    ca: V3d73Reg,
    cs: V3d73Reg,
    lc: V3d73Reg,
    pc: V3d73Reg,
    bfc: V3d73Reg,
}

static mut V3D73_SAMPLER: V3d73Sampler = V3d73Sampler {
    armed: false,
    samples: 0,
    qba: 0,
    qea: 0,
    ca_in_span: 0,
    ca_eq_qba: 0,
    ca_max_in_span: 0,
    ca: V3d73Reg::zeroed(),
    cs: V3d73Reg::zeroed(),
    lc: V3d73Reg::zeroed(),
    pc: V3d73Reg::zeroed(),
    bfc: V3d73Reg::zeroed(),
};

/// Arm the CLE-progress sampler for the job about to be kicked at [qba, qea). Seeds every tracked
/// register with its PRE-GO word, so a CLE that never touches the queue reads changes=0 with the
/// stale pre-kick address printed as first==last.
fn v3d73_sampler_arm(qba: u32, qea: u32) {
    let s = &raw mut V3D73_SAMPLER;
    unsafe {
        (*s).armed = true;
        (*s).samples = 0;
        (*s).qba = qba;
        (*s).qea = qea;
        (*s).ca_in_span = 0;
        (*s).ca_eq_qba = 0;
        (*s).ca_max_in_span = 0;
        (*s).ca.seed(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA));
        (*s).cs.seed(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS));
        (*s).lc.seed(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0LC));
        (*s).pc.seed(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0PC));
        (*s).bfc.seed(mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC));
    }
}

/// One in-flight sample of the five CLE-progress registers. No-op unless armed.
fn v3d73_sampler_take() {
    let s = &raw mut V3D73_SAMPLER;
    unsafe {
        if !(*s).armed {
            return;
        }
        let ca = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
        (*s).samples = (*s).samples.saturating_add(1);
        if ca >= (*s).qba && ca <= (*s).qea {
            (*s).ca_in_span = (*s).ca_in_span.saturating_add(1);
            (*s).ca_max_in_span = (*s).ca_max_in_span.max(ca);
        }
        if ca == (*s).qba {
            (*s).ca_eq_qba = (*s).ca_eq_qba.saturating_add(1);
        }
        (*s).ca.fold(ca);
        (*s).cs.fold(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS));
        (*s).lc.fold(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0LC));
        (*s).pc.fold(mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0PC));
        (*s).bfc.fold(mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC));
    }
}

/// Disarm and emit the `[v3d73]` witness with the fetch verdict in-line. The reading key (v3d.md
/// §44): CA never inside [QBA,QEA] = the CLE NEVER LOADED this job's queue — thread 0 never
/// started, the trigger is upstream of list execution entirely; CA pinned at QBA = loaded but the
/// first word was never fetched/advanced past; CA reached QEA with the PTB silent ([v3d71f]) =
/// the list EXECUTES and the FLUSH terminator fails to trigger the PTB frame-close.
fn v3d73_sampler_emit(what: &str) {
    let s = &raw mut V3D73_SAMPLER;
    unsafe {
        (*s).armed = false;
        let smp = &*s;
        let bfc_delta = smp.bfc.last.wrapping_sub(smp.bfc.first);
        let verdict: &str = if smp.samples == 0 {
            "ZERO samples landed — the wait exited before the first ~1 ms tick; INCONCLUSIVE, nothing here is a measurement"
        } else if bfc_delta != 0 {
            "BFC ADVANCED across the wait — a bin frame CLOSED under the sampler; read this leg's retire witness, the wedge did not reproduce here"
        } else if smp.ca_in_span == 0 {
            "CT0CA NEVER HELD AN ADDRESS INSIDE [QBA,QEA] at any sample — THE CLE NEVER FETCHES ITS FIRST WORD: the queue latched exactly ([v3d54] submit) yet control-list thread 0 never consumed it, and CA still holds the stale pre-kick word printed above. The trigger is UPSTREAM of list execution entirely — a thread-start condition, not list content, not addresses, not the terminator"
        } else if smp.ca_max_in_span == smp.qba {
            "CT0CA held QBA and NEVER ADVANCED — the CLE loaded this job's queue but never fetched (or never retired) its first word. The trigger dies at thread start / first fetch, upstream of every packet in the list; terminator/frame-close semantics were never reached"
        } else if smp.ca_max_in_span >= smp.qea {
            "CT0CA ADVANCED TO QEA — the list EXECUTES. With the PTB provably silent across the same class of wait ([v3d71f]) and no FLDONE, the FLUSH terminator fails to trigger the PTB frame-close: the wall moves INTO the terminator/frame-close semantics"
        } else {
            "CT0CA advanced into the list then STALLED before QEA at the max-in-span offset above — the CLE fetches but chokes mid-list at that byte"
        };
        serial_println!(
            ":: V3D: [v3d73] cle-progress ({}) — samples={} QBA={:#010x} QEA={:#010x}(len={}) | CT0CA pre-GO={:#010x} last={:#010x} min={:#010x} max={:#010x} distinct={}{} vals=[{:#010x} {:#010x} {:#010x} {:#010x}] changes={} in-span={} eq-QBA={} max-in-span={:#010x}(off {:#x} of {:#x}) | CT0CS {:#010x}->{:#010x} (CTRUN {}->{}) distinct={} | CT0LC {:#x}->{:#x} | CT0PC {:#x}->{:#x} | BFC {:#010x}->{:#010x} (Δ{}) — {} ::",
            what,
            smp.samples,
            smp.qba,
            smp.qea,
            smp.qea.wrapping_sub(smp.qba),
            smp.ca.first,
            smp.ca.last,
            smp.ca.min,
            smp.ca.max,
            smp.ca.n,
            if smp.ca.more { "(+MORE, capped)" } else { "" },
            smp.ca.vals[0], smp.ca.vals[1], smp.ca.vals[2], smp.ca.vals[3],
            smp.ca.changes,
            smp.ca_in_span,
            smp.ca_eq_qba,
            smp.ca_max_in_span,
            smp.ca_max_in_span.wrapping_sub(smp.qba),
            smp.qea.wrapping_sub(smp.qba),
            smp.cs.first,
            smp.cs.last,
            (smp.cs.first & V3D_CLE_CTNCS_CTRUN != 0) as u32,
            (smp.cs.last & V3D_CLE_CTNCS_CTRUN != 0) as u32,
            smp.cs.n,
            smp.lc.first,
            smp.lc.last,
            smp.pc.first,
            smp.pc.last,
            smp.bfc.first,
            smp.bfc.last,
            bfc_delta,
            verdict,
        );
    }
}

/// PI-V3D-74: read back the span evidence of the LAST armed+emitted sampler window, so a leg-level
/// verdict can fold the fetch facts into its own reading without re-deriving them from the wire.
/// Returns (samples, ca_in_span, ca_max_in_span) — the disarmed accumulator is left untouched.
fn v3d73_sampler_read_span() -> (u32, u32, u32) {
    let s = &raw const V3D73_SAMPLER;
    unsafe { ((*s).samples, (*s).ca_in_span, (*s).ca_max_in_span) }
}

/// PI-V3D-71 leg 1 (+ leg 2 riding its wait) — the mainline-geometry shape-match rung.
///
/// One `[v3d48]`-style Empty-frame kick (same 64x64 frame — shape and content are excluded prior, so
/// the ONE variable here is where the buffers sit in iova space and how big the pool claims to be),
/// against the v3d71 window: pool at the dump's ~0x03000000 sized at its 0x88000, CL and tile-state
/// above it. Same kernel-exact submit order, same FLDONE backstop, the [v3d68] 24 KiB poisoned
/// tile-state detector plus the poisoned physical pool every iova alias funnels into. Hub-gated,
/// poison-honest, and the window is UNMAPPED afterwards so every later job (the real M4 draw included)
/// runs on the exact page table its verdicts were always banked under.
fn v3d71_mainline_geometry() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: [v3d71] SKIPPED — hub IDENT0 reads 0x00000000 (block absent/unpowered; the QEMU raspi4b signature). No window was mapped, no rung was kicked and NO verdict is implied ::"
            );
            return;
        }
        V3dPresence::Poison(w) => {
            serial_println!(
                ":: V3D: [v3d71] SKIPPED — hub IDENT0 reads poison {:#010x} (open-bus / firmware fill, not a live register). No window was mapped, no rung was kicked and NO verdict is implied ::",
                w
            );
            return;
        }
    }
    let pool_phys = arena_phys() + OFF_BIN_TILEALLOC;
    let cl_phys = arena_phys() + OFF_PROBE_BIN_CL;
    let ts_phys = arena_phys() + OFF_V3D68_TSDA;
    serial_println!(
        ":: V3D: [v3d71] mainline-geom — the piOS dump-diff discriminator. BXCF and MISCCFG are EXCLUDED MEASURED (bit-identical on working mainline: 0x00000000 / 0x00000006); frame shape/content/fabric-identity excluded prior. The last PTB-visible difference is ADDRESS GEOMETRY: mainline bins into iovas in the tens of MiB with a ~544 KiB pool, we bin into sub-4 MiB identity iovas with 32 KiB. This rung re-plumbs OUR page table: pool iova={:#010x} size={:#x} (the dump's CT0QMS, iova-ALIASED cyclically onto the 32 KiB physical pool at {:#010x} — same physical arena, any write through any alias lands in poison), CL iova={:#010x} (phys {:#010x}), tile-state iova={:#010x} (24 KiB, 1:1 onto phys {:#010x}), guard page unmapped at {:#010x} ::",
        V3D71_POOL_IOVA, V3D71_POOL_BYTES, pool_phys, V3D71_CL_IOVA, cl_phys, V3D71_TS_IOVA, ts_phys,
        V3D71_POOL_IOVA + V3D71_POOL_BYTES,
    );
    if !v3d71_map_window(true) {
        serial_println!(
            ":: V3D: [v3d71] mainline-geom NOT RUN (fail-closed) — the window-map TLB clear never settled; kicking a rung against an unflushed translation would measure the TLB, not the geometry. No verdict ::"
        );
        return;
    }

    // The banked Empty CL, byte-identical to the [v3d48] control rung (same offset, same 64x64 frame),
    // already decoded + Mesa-diffed under [v3d48] this same boot — only its ADDRESS differs here.
    let bin_len = build_bin_cl_content_geom(OFF_PROBE_BIN_CL, OFF_SHADREC, 1, BinContent::Empty, TARGET_W, TARGET_H);
    v3d56_poison_region(OFF_V3D68_TSDA, V3D68_TSDA_BYTES);
    v3d56_poison_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    for i in (0..PROBE_BIN_OVERFLOW_BYTES).step_by(4) {
        arena_write_u32(OFF_PROBE_BIN_OVERFLOW + i, 0);
    }
    cache::clean_range(arena_phys() + OFF_PROBE_BIN_OVERFLOW, PROBE_BIN_OVERFLOW_BYTES);
    cache::clean_range(cl_phys, bin_len);

    let cl_iova = V3D71_CL_IOVA as u32;
    let cl_ea = cl_iova + bin_len as u32;

    // Kernel-exact submit order, mirrored from submit_bisect_rung_geom — the only deltas are the
    // ADDRESSES latched (window iovas instead of arena identity) and the pool SIZE (the dump's).
    clear_mmu_fault_latch("v3d71 pre-kick");
    bin_prejob_bpos_clear("v3d71 mainline-geom");
    invalidate_gpu_caches("L2T flush (v3d71 pre-kick)");
    let qts_val = V3D71_TS_IOVA as u32 | V3D_CLE_CT0QTS_ENABLE;
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, V3D71_POOL_IOVA as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, V3D71_POOL_BYTES as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, qts_val);
    dsb();
    let qts_echo = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QTS);
    let qms_echo = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMS);
    let qma_echo = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMA);
    serial_println!(
        ":: V3D: [v3d71] frame enables — CT0QMA wrote={:#010x} echo={:#010x} | CT0QMS wrote={:#x} echo={:#x} | CT0QTS wrote={:#010x} echo={:#010x} (ENABLE(bit1)={}) — the mainline-shaped triple, latched ::",
        V3D71_POOL_IOVA, qma_echo, V3D71_POOL_BYTES, qms_echo, qts_val, qts_echo,
        (qts_echo & V3D_CLE_CT0QTS_ENABLE != 0) as u32,
    );
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, cl_iova);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
    dsb();
    let bfc_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    // PI-V3D-72: arm the clock-liveness bank across THIS rung's waits — the second P82 wait whose
    // clkliv line carried no verdict, and the one taken with every input mainline-shaped. Armed last
    // before the GO (the v3d63 idiom) so the four Δs span exactly this frame; read + stopped after
    // the fabric-sampler emit below, before any post-rung readback perturbs the block.
    v3d72_pctr_arm("v3d71 mainline-geom");
    v3d71_sampler_arm(); // leg 2: the fabric sampler spans exactly this frame's wait
    // PI-V3D-73: the CLE-progress sampler spans the same wait — the one kick in the whole campaign
    // whose QBA is unique in iova space, so a stale CT0CA cannot masquerade as a loaded one.
    v3d73_sampler_arm(cl_iova, cl_ea);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, cl_ea); // GO
    dsb();
    let submit_sound = v3d54_submit_audit("v3d71", cl_iova, cl_ea, bin_len);
    let idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CT1CS_CTRUN, "CT0 v3d71 mainline-geom");
    let (sts, us, retired) = wait_fldone("v3d71 mainline-geom", cl_iova, cl_ea);
    v3d71_sampler_emit(); // [v3d71f]
    v3d73_sampler_emit("v3d71 mainline-geom"); // [v3d73] — the fetch verdict, in-flight
    v3d72_pctr_read_emit("v3d71 mainline-geom"); // [v3d72] — read + stop the bank at wait-exit

    let bpca = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA);
    let bpcs = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS);
    let bfc_after = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    let pcs = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
    // The MMU fault surface is load-bearing HERE as nowhere else: a mis-built window would fault the
    // CLE's very first fetch and a "still dead" read would be an instrument artifact. Read before W1C.
    let mmu_ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault =
        mmu_ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);

    // The [v3d68] evidence-integrity rule: drain the L2T, then scan the poison through the PHYSICAL
    // addresses (the CPU reads DRAM; every iova alias funnels into these same pages).
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_FLUSH);
    let flush_done = wait_bit_clear(
        V3D_CORE0_BASE,
        V3D_CTL_L2TCACTL,
        V3D_L2TCACTL_L2TFLS,
        "L2T write-back (v3d71 scan)",
    );
    mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
    dsb();
    cache::invalidate_range(ts_phys, V3D68_TSDA_BYTES);
    cache::invalidate_range(pool_phys, BIN_TILEALLOC_BYTES);
    dsb();
    let ts_scan = v3d56_scan(ts_phys as u32, V3D68_TSDA_BYTES);
    let pool_scan = v3d56_scan(pool_phys as u32, BIN_TILEALLOC_BYTES);
    v3d56_emit_scan("v3d71", "tile-state (24 KiB, phys of iova 0x0308a000)", ts_phys as u32, &ts_scan, flush_done);
    v3d56_emit_scan("v3d71", "tile-alloc pool (32 KiB phys behind all 544 KiB of pool iova)", pool_phys as u32, &pool_scan, flush_done);

    let touched = ts_scan.zeroed + ts_scan.overwritten + pool_scan.zeroed + pool_scan.overwritten;
    let verdict = if fault != 0 {
        "AN MMU FAULT LATCHED ACROSS THE RUNG — the window translation itself refused an access, so nothing downstream of it was measured. This is an INSTRUMENT fault, not a geometry verdict: fix the map (see the fault-latch line below for client + VA) before citing this rung for anything"
    } else if retired || touched != 0 {
        "the address-geometry/pool-size branch is CONVICTED — the SAME Empty frame that has wedged at identity iovas for the whole campaign produced a retire or PTB memory traffic the moment its buffers sat at mainline-LIKE iovas with a mainline-size pool. The wall was never the engine: it is where this driver PLACES the bin frame. Next arc: adopt mainline's allocation geometry outright and re-run the full ladder"
    } else if !flush_done {
        "no retire and no fault, but the L2T drain did not complete so the poison column cannot be read — the register half says still-dead, the memory half is INCONCLUSIVE; re-run before banking the exclusion"
    } else {
        "STILL DEAD under mainline geometry — no retire, no fault, poison fully intact through a completed drain, with the pool at the dump's own size and every buffer at mainline-like iovas. The LAST PTB-visible difference between the two systems is now EXCLUDED MEASURED: no register the PTB can see distinguishes our bin frame from mainline's, and the wall moves conclusively off the frame's inputs — read [v3d71f] above for which side of the master bridge it lives on"
    };
    serial_println!(
        ":: V3D: [v3d71] mainline-geom verdict — retired={} BFC {:#010x}->{:#010x} (Δ{}) PCS={:#010x}(BMACTIVE={} BMBUSY={} BMOOM={}) idled={} INT_STS={:#010x} waited={}us submit_sound={} | BPCA={:#010x} (advance off pool iova = {:#x}) BPCS={:#x} | poison touched: tile-state={} of {} words, pool={} of {} words (L2T drain completed={}) | MMU_CTL={:#010x} fault latched={} (PT_INVALID={} WRITE_VIOLATION={} CAP_EXCEEDED={}) — {} ::",
        retired as u32, bfc_pre, bfc_after, bfc_after.wrapping_sub(bfc_pre),
        pcs,
        (pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (pcs & V3D_PCS_BMBUSY != 0) as u32,
        (pcs & V3D_PCS_BMOOM != 0) as u32,
        idled as u32, sts, us, submit_sound as u32,
        bpca, bpca.wrapping_sub(V3D71_POOL_IOVA as u32), bpcs,
        ts_scan.zeroed + ts_scan.overwritten, ts_scan.words,
        pool_scan.zeroed + pool_scan.overwritten, pool_scan.words,
        flush_done as u32,
        mmu_ctl, (fault != 0) as u32,
        (fault & V3D_MMU_CTL_PT_INVALID != 0) as u32,
        (fault & V3D_MMU_CTL_WRITE_VIOLATION != 0) as u32,
        (fault & V3D_MMU_CTL_CAP_EXCEEDED != 0) as u32,
        verdict,
    );
    clear_mmu_fault_latch("v3d71 post-kick");
    // Restore the exact pre-v3d71 translation: zero the window PTEs and flush again, so the real M4
    // draw (and every later job) runs on the page table its verdicts were always banked under.
    let unmapped = v3d71_map_window(false);
    serial_println!(
        ":: V3D: [v3d71] window unmapped={} — the page table is back to the arena-identity-only map; scope: this rung wrote PTEs in OUR table, the per-job CT0Q* latches and mainline's own MMU-flush sequence, and NOTHING else — no BXCF, no MISCCFG, no CTRSTA, no GMP, no new MMIO window ::",
        unmapped as u32,
    );
}

// ─── PI-V3D-73 leg 3 — the mainline-EXACT submit sequence, with nothing interleaved ──────────────
//
// The submit-sequence diff (v3d.md §44.2, compiled from the mainline facts this file already
// records — the v3d_bin_job_run order at the CT0Q* facts block, the [v3d59] mainline ledger
// T1–T4, v3d_invalidate_caches at bin_prejob_invalidate_kernel_exact) finds NO register that
// mainline writes on the bin submit path and we do not: BPOS=0 → invalidate → (perfmon switch,
// a no-op when no perfmon is attached — the common mainline case) → CT0QMA → CT0QMS →
// CT0QTS|ENABLE → CT0QBA → CT0QEA (the QEA write IS the GO; queue-register auto-start, no CTnCS
// write anywhere on the path). What the diff DOES find is that no kick in this campaign has ever
// been that sequence and nothing else. Every rung interleaves instrument MMIO inside it:
//
//   (a) an INT_CLR W1C write BETWEEN CT0QBA and CT0QEA (mainline writes nothing between the
//       queue pair — its ISR does the W1C; ours sits inside the QBA→QEA span on every kick);
//   (b) echo READS of CT0QTS/QMS/QMA between the tile-memory latch and CT0QBA ([v3d49]);
//   (c) FLM=FLUSH instead of mainline's CLEAR on every rung but [v3d53] — and the [v3d53]
//       control rung that fixed (c) still carried (a) and (b).
//
// Each divergence is individually benign-looking and (c) was individually controlled, but the
// conjunction "byte-exact v3d_bin_job_run transcription, zero interleaved MMIO" has never run —
// and with the config space exhausted measured (P83) and the trigger provably upstream of list
// execution, an un-run mainline-documented SEQUENCE is exactly where an unfound trigger could
// still hide. This leg runs it once: identity addresses (the canonical [v3d48] Empty frame, the
// v3d71 window already unmapped), poison detectors intact, the [v3d73] sampler across its wait.
// Within license: every write below is one mainline makes on this path, in mainline's order —
// this is the REMOVAL of off-mainline instrument traffic, not an experiment.
// ─────────────────────────────────────────────────────────────────────────────────────────────────
fn v3d73_mainline_submit() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: [v3d73s] SKIPPED — hub IDENT0 reads 0x00000000 (block absent/unpowered; the QEMU raspi4b signature). No rung was kicked and NO verdict is implied ::"
            );
            return;
        }
        V3dPresence::Poison(w) => {
            serial_println!(
                ":: V3D: [v3d73s] SKIPPED — hub IDENT0 reads poison {:#010x} (open-bus / firmware fill, not a live register). No rung was kicked and NO verdict is implied ::",
                w
            );
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d73s] mainline-exact submit — the submit-sequence diff's one un-run leg: BPOS=0 -> v3d_invalidate_caches (SLC-first, window re-established, FLM=CLEAR) -> CT0QMA -> CT0QMS -> CT0QTS|ENABLE -> CT0QBA -> CT0QEA as SEVEN UNINTERRUPTED WRITES — no INT_CLR inside the QBA->QEA pair, no echo reads inside the latch block, exactly v3d_bin_job_run. Every prior kick interleaved instrument MMIO into this sequence; this one removes it ::"
    );

    // The canonical Empty CL at identity addresses (the v3d71 window is unmapped; this is the
    // translation every banked verdict was taken under).
    let bin_len = build_bin_cl_content_geom(OFF_PROBE_BIN_CL, OFF_SHADREC, 1, BinContent::Empty, TARGET_W, TARGET_H);
    v3d56_poison_region(OFF_V3D68_TSDA, V3D68_TSDA_BYTES);
    v3d56_poison_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    for i in (0..PROBE_BIN_OVERFLOW_BYTES).step_by(4) {
        arena_write_u32(OFF_PROBE_BIN_OVERFLOW + i, 0);
    }
    cache::clean_range(arena_phys() + OFF_PROBE_BIN_OVERFLOW, PROBE_BIN_OVERFLOW_BYTES);
    cache::clean_range(arena_phys() + OFF_PROBE_BIN_CL, bin_len);

    let ba = (arena_phys() + OFF_PROBE_BIN_CL) as u32;
    let ea = ba + bin_len as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let ts_phys = arena_phys() + OFF_V3D68_TSDA;
    let qts_val = ts_phys as u32 | V3D_CLE_CT0QTS_ENABLE;

    // Every instrument access happens BEFORE the sequence: stale-latch W1C (mainline's ISR keeps
    // STS clean between jobs; ours must clear it here or a stale bit would fake a retire), the
    // fault-latch clear, the BFC baseline, and the [v3d73] sampler's pre-GO seeds.
    clear_mmu_fault_latch("v3d73s pre-kick");
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
    dsb();
    let bfc_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    v3d73_sampler_arm(ba, ea);

    // ── The transcription. v3d_bin_job_run, write for write, in order, nothing between. ──
    // (1) BPOS=0 — the first write of every mainline bin job (facts block at V3D_CLE_CT0QTS).
    mmio_write(V3D_CORE0_BASE, V3D_PTB_BPOS, 0);
    dsb();
    // (2) v3d_invalidate_caches — the kernel-exact FLM=CLEAR idiom ([v3d53]'s helper: SLCACTL
    //     first, L2T window re-established, L2TCACTL=L2TFLS|FLM_CLEAR; the poll is our standard
    //     finite backstop, a safe superset of the kernel's fire-and-forget).
    let (l2t_before, l2t_after) = bin_prejob_invalidate_kernel_exact("L2T CLEAR (v3d73s pre-kick)");
    // (3) v3d_switch_perfmon — a NO-OP when no perfmon is attached to the job, which is the
    //     mainline common case; deliberately not armed here so the [v3d72] two-window law holds
    //     and this leg stays a pure sequence transcription.
    // (4..8) the five CLE latches, back to back. The QEA write is the GO (queue auto-start).
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, tile_alloc);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, BIN_TILEALLOC_BYTES as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, qts_val);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, ba);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, ea); // GO
    dsb();
    // ── End of the transcription; instruments resume (all post-GO). ──

    serial_println!(
        ":: V3D: [v3d73s] sequence — L2TCACTL {:#010x}->{:#010x} (FLM=CLEAR, SLC-first, window re-established) | QMA..QEA issued as five consecutive nGnRnE writes, one dsb after QEA; deltas vs every prior kick: no INT_CLR between QBA and QEA, no CT0Q* echo reads before QBA, mainline invalidate mode ::",
        l2t_before, l2t_after,
    );
    let submit_sound = v3d54_submit_audit("v3d73s", ba, ea, bin_len);
    let idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CT1CS_CTRUN, "CT0 v3d73s mainline-submit");
    let (sts, us, retired) = wait_fldone("v3d73s mainline-submit", ba, ea);
    v3d73_sampler_emit("v3d73s mainline-submit"); // [v3d73] — the fetch verdict on THIS sequence

    let bfc_after = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    let pcs = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
    let mmu_ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault =
        mmu_ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);

    // The [v3d68] evidence-integrity rule: drain the L2T, then scan the poison detectors.
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_FLUSH);
    let flush_done = wait_bit_clear(
        V3D_CORE0_BASE,
        V3D_CTL_L2TCACTL,
        V3D_L2TCACTL_L2TFLS,
        "L2T write-back (v3d73s scan)",
    );
    mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
    dsb();
    cache::invalidate_range(ts_phys, V3D68_TSDA_BYTES);
    cache::invalidate_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    dsb();
    let ts_scan = v3d56_scan(ts_phys as u32, V3D68_TSDA_BYTES);
    let pool_scan = v3d56_scan(tile_alloc, BIN_TILEALLOC_BYTES);
    v3d56_emit_scan("v3d73s", "tile-state (24 KiB, identity)", ts_phys as u32, &ts_scan, flush_done);
    v3d56_emit_scan("v3d73s", "tile-alloc pool (32 KiB, identity)", tile_alloc, &pool_scan, flush_done);
    let touched = ts_scan.zeroed + ts_scan.overwritten + pool_scan.zeroed + pool_scan.overwritten;

    let verdict = if fault != 0 {
        "AN MMU FAULT LATCHED — instrument fault, not a sequence verdict; fix before citing this rung"
    } else if retired || touched != 0 {
        "the SUBMIT SEQUENCE is CONVICTED — the same Empty frame that wedges under the instrumented submit retired (or produced PTB traffic) the moment the sequence was v3d_bin_job_run verbatim with nothing interleaved. The trigger was being broken by our own instrument MMIO inside the submit; adopt this uninterrupted sequence as the ONLY submit path and re-run the full ladder"
    } else if !flush_done {
        "no retire and no fault, but the L2T drain did not complete — the poison column is INCONCLUSIVE; re-run before banking the exclusion"
    } else {
        "STILL DEAD under the mainline-exact sequence — no retire, no fault, poison intact through a completed drain, with zero interleaved MMIO between BPOS and the GO. The SUBMIT SEQUENCE is now exhausted alongside the config space: no write mainline makes that we lack, no order it keeps that we broke. Read [v3d73] above for where the fetch dies; the trigger hunt moves fully upstream of the submit interface"
    };
    serial_println!(
        ":: V3D: [v3d73s] verdict — retired={} BFC {:#010x}->{:#010x} (Δ{}) PCS={:#010x}(BMACTIVE={} BMBUSY={} BMOOM={}) idled={} INT_STS={:#010x} waited={}us submit_sound={} | poison touched: tile-state={} of {} words, pool={} of {} words (L2T drain completed={}) | MMU_CTL={:#010x} fault latched={} — {} ::",
        retired as u32, bfc_pre, bfc_after, bfc_after.wrapping_sub(bfc_pre),
        pcs,
        (pcs & V3D_PCS_BMACTIVE != 0) as u32,
        (pcs & V3D_PCS_BMBUSY != 0) as u32,
        (pcs & V3D_PCS_BMOOM != 0) as u32,
        idled as u32, sts, us, submit_sound as u32,
        ts_scan.zeroed + ts_scan.overwritten, ts_scan.words,
        pool_scan.zeroed + pool_scan.overwritten, pool_scan.words,
        flush_done as u32,
        mmu_ctl, (fault != 0) as u32,
        verdict,
    );
    clear_mmu_fault_latch("v3d73s post-kick");
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// PI-V3D-74 — the THREAD-SWAP discriminator: is THREAD 0 dead as a thread, or are BIN-class
// lists dead as a class?
//
// The P84 wire established, and this rung does not re-litigate: [v3d73s] = the byte-exact
// mainline submit sequence changes NOTHING (the submit interface is exhausted alongside the
// config space). [v3d73] cle-progress = CT0's queue LOADS (CA=our list, CTRUN=1) but the CLE
// NEVER FETCHES THE FIRST WORD — 500 samples, zero movement, the core clocked (703M cycles) and
// idle. Meanwhile THREAD 1 WORKS on the same CLE hardware: CT1/RCL jobs fetch, execute, retire
// and land byte-verified pixels ([v3d58] xengine). Thread 1 alive; thread 0 loaded-but-never-
// fetches. Two hypotheses fit, and they diverge completely on where the hunt goes next:
//
//   (i)  BIN-class lists are dead as a CLASS — something about a bin submission upstream of the
//        first fetch (the QMA/QMS/QTS tile-memory arming state, or bin-mode selection) kills the
//        fetch before it starts. Then thread 0 CAN fetch, and the next instrument targets what
//        distinguishes a bin submission before its first word.
//   (ii) THREAD 0 is dead as a THREAD — it never starts regardless of list class: a per-thread
//        enable/power state no register this campaign knows controls. Then the hunt leaves the
//        core's programming interface entirely (firmware/VPU-side per-thread init becomes the
//        leading space) and the next discriminator is a piOS dump taken MID-BIN (the S1S bench
//        ask — v3d.md §45).
//
// Leg A (`[v3d74a]` rcl-on-ct0) splits them: take the minimal render list that thread 1 provably
// executes — the M3 clear-job RCL, the smallest retiring rung in this file, byte-verified on
// metal since M3 — and submit it on CT0's queue, otherwise byte-identical to the retiring CT1
// submit (QBA then QEA, nothing else — deliberately NO QMA/QMS/QTS arming: that is the bin-class
// state under suspicion), with the [v3d73] sampler armed across the wait. A bonus of the swap:
// QBA here is the RCL's address, which NO prior CT0 kick ever latched — so a CA==QBA reading is
// unambiguous (the [v3d73] stale-word trap cannot reproduce on this leg).
//
// Leg B (`[v3d74b]` bcl-on-ct1, the mirror) is a DOCUMENTED NO-OP — see its witness line: the
// v3d_regs.h map this file transcribed says the bin tile-memory latches are thread-0-only
// (CT0QTS 0x15c / CT0QMA 0x170 / CT0QMS 0x174 have no CT1 counterparts — 0x158 is CT1SYNC,
// 0x178 is CT1QCFG), so "the bin-side arming, as v3d71 does" cannot be expressed for thread 1,
// and a bin list reaching START_TILE_BINNING on a thread with no PTB plumbing risks wedging CT1
// — the only thread this campaign has ever seen retire, and the working reference every
// cross-engine verdict rests on. The discriminator is one-sided by hardware design; leg A
// carries the verdict alone.
//
// Rides UNAOS_V3D_DEEP, LAST — after [v3d73s], so every banked reading above is taken on the
// arrangement its verdict was banked under. Same poison detectors, same FLDONE-wait window
// (the wait is the sampler's tick source; an RCL retires with FRDONE not FLDONE, so the wait
// always runs its full backstop and the INT decode on its timeout line is part of the reading).
// Reading key: the witness text below + v3d.md §45.
// ─────────────────────────────────────────────────────────────────────────────────────────────────
fn v3d74_thread_swap() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: [v3d74] SKIPPED — hub IDENT0 reads 0x00000000 (block absent/unpowered; the QEMU raspi4b signature). No rung was kicked and NO verdict is implied ::"
            );
            return;
        }
        V3dPresence::Poison(w) => {
            serial_println!(
                ":: V3D: [v3d74] SKIPPED — hub IDENT0 reads poison {:#010x} (open-bus / firmware fill, not a live register). No rung was kicked and NO verdict is implied ::",
                w
            );
            return;
        }
    }
    serial_println!(
        ":: V3D: [v3d74] thread-swap — thread 0 dead, or bin-class lists dead? [v3d73] proved CT0 loads but never fetches while CT1 retires RCLs on the same CLE. Leg A submits the PROVEN M3 RCL on CT0's queue, byte-identical to the retiring CT1 submit (QBA->QEA only, NO QMA/QMS/QTS — the bin-class arming is the suspect being removed), [v3d73] sampler armed. CA advances/retires = THREAD 0 CAN FETCH, the death is BIN-JOB-CONFIG-specific upstream of fetch; CA pinned = THREAD 0 NEVER STARTS regardless of class, the hunt leaves the core's programming interface (next: a piOS dump MID-BIN — the S1S bench ask) ::"
    );

    // ── Leg A: the M3 RCL — the smallest list thread 1 provably executes — on CT0's queue. ──
    // Prep is clear_job's, verbatim: sentinel-seeded target, both lists built + published.
    fill_target(0xDEAD_BEEF);
    let (rcl_len, sublist_len) = build_rcl();
    cache::clean_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
    cache::clean_range(arena_phys() + OFF_RCL, rcl_len);
    cache::clean_range(arena_phys() + OFF_SUBLIST, sublist_len);
    let ba = arena_phys() + OFF_RCL;
    let ea = ba + rcl_len;
    if !arena_contains(ba, rcl_len) {
        serial_println!(":: V3D: [v3d74a] RCL range escapes the arena — refusing kick (fail-closed) ::");
        return;
    }
    // The [v3d68] evidence-integrity detectors: if anything on this leg makes PTB-class writes,
    // the poison says so (an executing RCL writes the TARGET, never these regions).
    v3d56_poison_region(OFF_V3D68_TSDA, V3D68_TSDA_BYTES);
    v3d56_poison_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);

    // All instrument accesses BEFORE the kick (the [v3d73s] discipline): stale-latch W1C, fault
    // latch clear, RFC/BFC baselines, sampler pre-GO seeds.
    clear_mmu_fault_latch("v3d74a pre-kick");
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
    dsb();
    let rfc_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_RFC);
    let bfc_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    v3d73_sampler_arm(ba as u32, ea as u32);

    // ── The swap itself: the retiring CT1 submit (clear_job's two writes), on CT0's slots. ──
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, ba as u32);
    dsb(); // BA latched before the EA write triggers the fetch (clear_job's ordering, verbatim)
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, ea as u32); // GO — queue auto-start
    dsb();
    let cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ca_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);

    let submit_sound = v3d54_submit_audit("v3d74a", ba as u32, ea as u32, rcl_len);
    // The FLDONE wait is the sampler's tick window. An RCL that executes retires with FRDONE
    // (bit0), which this wait does NOT return early on — so a fetching thread 0 still runs the
    // full ~0.5 s backstop and the [v3d44] timeout line prints FRDONE=1: that print is a POSITIVE
    // reading on this leg, not a failure.
    let (sts, us, fldone) = wait_fldone("v3d74a rcl-on-ct0", ba as u32, ea as u32);
    v3d73_sampler_emit("v3d74a rcl-on-ct0"); // [v3d73] — the fetch verdict on the swapped class
    let (smp_n, smp_in_span, smp_max) = v3d73_sampler_read_span();

    let cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    let rfc_after = mmio_read(V3D_CORE0_BASE, V3D_CLE_RFC);
    let bfc_after = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    let frdone = sts & V3D_INT_FRDONE != 0;
    let mmu_ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault =
        mmu_ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);

    // Evidence-integrity: drain the L2T, then scan the poison detectors ([v3d68] rule).
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_FLUSH);
    let flush_done = wait_bit_clear(
        V3D_CORE0_BASE,
        V3D_CTL_L2TCACTL,
        V3D_L2TCACTL_L2TFLS,
        "L2T write-back (v3d74a scan)",
    );
    mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
    dsb();
    let ts_phys = arena_phys() + OFF_V3D68_TSDA;
    cache::invalidate_range(ts_phys, V3D68_TSDA_BYTES);
    cache::invalidate_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);
    dsb();
    let ts_scan = v3d56_scan(ts_phys as u32, V3D68_TSDA_BYTES);
    let pool_scan = v3d56_scan((arena_phys() + OFF_BIN_TILEALLOC) as u32, BIN_TILEALLOC_BYTES);
    v3d56_emit_scan("v3d74a", "tile-state (24 KiB, identity)", ts_phys as u32, &ts_scan, flush_done);
    v3d56_emit_scan(
        "v3d74a",
        "tile-alloc pool (32 KiB, identity)",
        (arena_phys() + OFF_BIN_TILEALLOC) as u32,
        &pool_scan,
        flush_done,
    );

    // The store verify — the same DRAM-truth readback clear_job banks its PASS on. Meaningful as
    // a positive only; a false here with CA pinned is just the sentinel surviving an unrun list.
    cache::clean_invalidate_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
    let verified = verify_target(CLEAR_RGBA);

    let idled = cs_done & V3D_CLE_CTNCS_CTRUN == 0 && cs_kicked & V3D_CLE_CTNCS_CTRUN != 0;
    let rfc_delta = rfc_after.wrapping_sub(rfc_pre);
    // Execution evidence, strongest first: byte-verified store / a render-frame retire signal /
    // CA walked to QEA. Fetch evidence: CA ever advanced past QBA into the span.
    let executed = verified || frdone || fldone || rfc_delta != 0 || smp_max >= ea as u32;
    let fetched = smp_in_span > 0 && smp_max > ba as u32;
    let loaded = smp_in_span > 0;
    let verdict = if fault != 0 {
        "AN MMU FAULT LATCHED — instrument fault, not a discriminator verdict; fix before citing this rung"
    } else if smp_n == 0 {
        "ZERO samples landed — the wait exited before the first tick; INCONCLUSIVE, nothing here is a measurement"
    } else if executed {
        "THREAD 0 CAN FETCH AND EXECUTE — the render-class list that thread 1 retires ran on CT0's queue the moment the bin-class arming was absent. Thread 0 is ALIVE as a thread; the death is specific to BIN-JOB CONFIGURATION upstream of the first fetch — whatever distinguishes a bin submission before its first word (the QMA/QMS/QTS tile-memory arming state and its interactions with thread start). Instrument THAT next: the arming-state permutation ladder"
    } else if fetched {
        "THREAD 0 FETCHES an RCL-class list but stalled mid-span at the max-in-span offset above — thread start is LIST-CLASS-DEPENDENT: the bin-class death remains upstream of fetch, and the mid-list choke on this leg is a separate localisable fact (an RCL run without render-side plumbing may legitimately choke past the fetch). The class discrimination stands: thread 0 STARTS when the submission is not bin-armed"
    } else if loaded {
        "CT0CA LOADED the RCL's queue address — unambiguous on this leg, no prior CT0 kick ever latched this address — but NEVER FETCHED past it: the same loaded-but-never-fetches signature as the bin class. THREAD 0 ITSELF NEVER STARTS regardless of list class — a per-thread enable/power state no register this campaign knows controls. The hunt leaves the core's programming interface: firmware/VPU-side per-thread init is the leading space, and the next discriminator is a piOS register dump taken MID-BIN (the S1S bench ask, v3d.md §45)"
    } else {
        "CT0CA NEVER HELD the RCL's queue address at any sample — thread 0 did not even load the swapped queue. THREAD 0 ITSELF NEVER STARTS regardless of list class — a per-thread enable/power state no register this campaign knows controls. The hunt leaves the core's programming interface: firmware/VPU-side per-thread init is the leading space, and the next discriminator is a piOS register dump taken MID-BIN (the S1S bench ask, v3d.md §45)"
    };
    serial_println!(
        ":: V3D: [v3d74a] verdict — store-verified={} FRDONE={} FLDONE={} RFC {:#010x}->{:#010x} (Δ{}) BFC {:#010x}->{:#010x} (Δ{}) | CT0CS kicked={:#010x} done={:#010x} (CTRUN {}->{}) idled={} CT0CA kicked={:#010x} done={:#010x} | sampler: samples={} in-span={} max-in-span={:#010x} vs QBA={:#010x} QEA={:#010x} | INT_STS={:#010x} waited={}us submit_sound={} | poison touched: tile-state={} of {}, pool={} of {} (drain completed={}) | MMU_CTL={:#010x} fault={} — {} ::",
        verified as u32, frdone as u32, fldone as u32,
        rfc_pre, rfc_after, rfc_delta,
        bfc_pre, bfc_after, bfc_after.wrapping_sub(bfc_pre),
        cs_kicked, cs_done,
        (cs_kicked & V3D_CLE_CTNCS_CTRUN != 0) as u32,
        (cs_done & V3D_CLE_CTNCS_CTRUN != 0) as u32,
        idled as u32,
        ca_kicked, ca_done,
        smp_n, smp_in_span, smp_max, ba as u32, ea as u32,
        sts, us, submit_sound as u32,
        ts_scan.zeroed + ts_scan.overwritten, ts_scan.words,
        pool_scan.zeroed + pool_scan.overwritten, pool_scan.words,
        flush_done as u32,
        mmu_ctl, (fault != 0) as u32,
        verdict,
    );
    // Leave the field clean: W1C whatever this leg latched (FRDONE on a positive) so no later
    // reader inherits it, and clear any fault latch the same way every rung does.
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
    dsb();
    clear_mmu_fault_latch("v3d74a post-kick");

    // ── Leg B: the mirror (BCL-on-CT1) — a DOCUMENTED NO-OP, on the tree's own register map. ──
    serial_println!(
        ":: V3D: [v3d74b] bcl-on-ct1 — DOCUMENTED NO-OP, deliberately NOT kicked. The mirror would submit the Empty bin list on CT1's queue with the bin-side memory latches armed as v3d71 does — but the v3d_regs.h map this file transcribed says those latches are THREAD-0-ONLY: CT0QTS(0x15c)/CT0QMA(0x170)/CT0QMS(0x174) have NO CT1 counterparts (0x158 is CT1SYNC, 0x178 is CT1QCFG). Thread 1 has no PTB tile-memory plumbing to arm, so the mirror cannot be expressed — and a bin list reaching START_TILE_BINNING on a thread with no PTB access risks wedging CT1, the ONLY thread this campaign has ever seen retire ([v3d58] xengine) and the working reference every cross-engine verdict rests on. The discriminator is one-sided by hardware design; [v3d74a] carries the verdict alone ::"
    );
}

/// PI-V3D-75 — the fabric-condition experiments, from the S1S mid-bin piOS dump
/// (`v3d-dump-mid-bin.txt`, 2026-07-30, in-span=True, settle-proven: CT0CS 0x20→0x00 with BFC
/// 0xE4→0xE5 across 1 ms). The dump's three divergences from every wedged UnaOS reading:
///
///   1. `RPIVID_ASB_V3D_M_CTRL`: piOS mid-bin `0x4040` vs ours `0x4` after release. Mainline never
///      writes any ASB register on BCM2711 (`bcm2835_power_power_on` returns early when rpivid_asb
///      is present), so `0x4040` is the FIRMWARE's value — and our boot's entry read is `0x7`
///      (parked), so the firmware only establishes bits 6/14 on the KMS-overlay boot. Both our
///      stop/release are RMW: we never cleared those bits; we never received them.
///   2. `LEGACY_ASB_V3D_{S,M}_CTRL`: piOS `0x5` (REQ_STOP held — legacy bridges deliberately
///      parked). Read-only station here; recorded, not driven.
///   3. `CT0CS` while genuinely fetching reads `0x20` (CTRUN alone); every wedged read is `0x70`
///      (bits 4 and 6 on top). Not driven here either — but every [v3d75] kick prints CS so the
///      signature rides the same line.
///
/// Two experiments, ordered least- to most-invasive, each followed by the [v3d74a] discriminator
/// kick (the proven M3 RCL on CT0, bare QBA→QEA, sampler armed):
///   A. mailbox `SET_ENABLE_QPU(1)` — ask the firmware to run ITS V3D init, then re-read M_CTRL.
///      If the firmware's own path sets the missing bits, the fix is a mailbox call, not a poke.
///   B. direct `M_CTRL = pw|0x4040` — transplant the working value verbatim, readback-audited.
/// A kick that retires (store-verify / FRDONE / RFC Δ / CA in-span) after A or B convicts the
/// fabric condition; both-still-dead banks the M_CTRL bits as exonerated alongside the submit.
fn v3d75_fabric_condition() {
    match probe_hub_ident0() {
        V3dPresence::Up(_) => {}
        V3dPresence::Down | V3dPresence::Poison(_) => {
            serial_println!(
                ":: V3D: [v3d75] SKIPPED — hub absent/poison (QEMU raspi4b or block-down). No experiment ran and NO verdict is implied ::"
            );
            return;
        }
    }
    let m0 = mmio_read(RPIVID_ASB_BASE, ASB_V3D_M_CTRL);
    let s0 = mmio_read(RPIVID_ASB_BASE, ASB_V3D_S_CTRL);
    let lm = mmio_read(LEGACY_ASB_BASE, ASB_V3D_M_CTRL);
    let ls = mmio_read(LEGACY_ASB_BASE, ASB_V3D_S_CTRL);
    serial_println!(
        ":: V3D: [v3d75] fabric-condition — station: RPIVID M_CTRL={:#010x} S_CTRL={:#010x} | LEGACY M={:#010x} S={:#010x} | piOS mid-bin reference M=0x00004040 S=0x00000004 LEGACY=0x00000005/0x00000005 — experiments A (firmware ENABLE_QPU) then B (M_CTRL=0x4040 transplant), each followed by the [v3d74a] discriminator kick ::",
        m0, s0, lm, ls
    );

    // ── Experiment A: the firmware's own V3D init path. ──
    // PI-V3D-81: when the reply-less QPU leg is armed this send stands down and [v3d81q] owns the
    // one send of the tag — otherwise [v3d81q]'s pre-station would be reading a firmware that had
    // already been asked once, and its post-send diff would attribute this rung's effect to itself.
    // The rest of the rung still runs, which turns it into something it could not be before: the
    // same-boot PRE-SEND control for [v3d81q], taken at this exact position in the battery.
    #[cfg(not(feature = "v3d81_qpu"))]
    match mailbox::set_enable_qpu(1) {
        Some(r) => serial_println!(":: V3D: [v3d75a] SET_ENABLE_QPU(1) — mailbox OK reply={:#010x} ::", r),
        None => serial_println!(":: V3D: [v3d75a] SET_ENABLE_QPU(1) — MAILBOX FAILED (tag unhandled or call error) ::"),
    }
    #[cfg(feature = "v3d81_qpu")]
    serial_println!(
        ":: V3D: [v3d75a] SEND STOOD DOWN — UNAOS_V3D81_QPU is armed, so SET_ENABLE_QPU is sent EXACTLY ONCE this boot, reply-less, in [v3d81q]. The station and kick below are this boot's PRE-SEND control ::"
    );
    let m_a = mmio_read(RPIVID_ASB_BASE, ASB_V3D_M_CTRL);
    #[cfg(not(feature = "v3d81_qpu"))]
    serial_println!(
        ":: V3D: [v3d75a] M_CTRL after ENABLE_QPU: {:#010x} (was {:#010x}, piOS ref 0x00004040) — {} ::",
        m_a, m0,
        if m_a != m0 { "THE FIRMWARE PATH TOUCHED THE BRIDGE — the missing init is mailbox-reachable" }
        else { "unchanged — ENABLE_QPU alone does not program this bridge on this firmware" }
    );
    #[cfg(feature = "v3d81_qpu")]
    serial_println!(
        ":: V3D: [v3d75a] M_CTRL pre-send control: {:#010x} (station {:#010x}, piOS ref 0x00004040) — no tag was sent, so equality here is the expected null and the value [v3d81q]'s post-send read is compared against ::",
        m_a, m0
    );
    #[cfg(not(feature = "v3d81_qpu"))]
    let a = v3d75_kick_probe("v3d75a post-enable-qpu");
    #[cfg(feature = "v3d81_qpu")]
    let a = v3d75_kick_probe("v3d75a pre-send-control");

    // ── Experiment B: transplant the working value verbatim (skipped if A already retires). ──
    if !a {
        mmio_write(RPIVID_ASB_BASE, ASB_V3D_M_CTRL, PM_PASSWORD | 0x4040);
        dsb();
        let m_b = mmio_read(RPIVID_ASB_BASE, ASB_V3D_M_CTRL);
        serial_println!(
            ":: V3D: [v3d75b] M_CTRL transplant — wrote pw|0x4040, readback {:#010x} ({}) ::",
            m_b,
            if m_b == 0x4040 { "held" } else { "did NOT hold — bits read-only or firmware-owned" }
        );
        let b = v3d75_kick_probe("v3d75b post-transplant");
        serial_println!(
            ":: V3D: [v3d75] verdict — A(enable-qpu)={} B(m-ctrl-0x4040)={} — {} ::",
            a as u32, b as u32,
            if b { "THE WALL WAS THE FABRIC CONDITION — the transplanted M_CTRL value lets thread 0 start; productionize the init" }
            else { "both experiments dead — the M_CTRL divergence is a passenger, not the condition; the firmware-init diff hunt continues (next stations: the KMS overlay's other VPU-side writes — full piOS/UnaOS boot-state diff)" }
        );
    } else {
        serial_println!(
            ":: V3D: [v3d75] verdict — A(enable-qpu)=1 — THE WALL WAS FIRMWARE-SIDE V3D INIT, and it is MAILBOX-REACHABLE: SET_ENABLE_QPU(1) lets thread 0 start; productionize the call in bringup ::"
        );
    }
}

/// PI-V3D-75's discriminator kick: the [v3d74a] leg, compacted — the proven M3 RCL on CT0's queue,
/// bare QBA→QEA, [v3d73] sampler armed, store-verify + retire signals read. Returns true if the
/// kick shows execution (store-verified / FRDONE / FLDONE / RFC Δ / CA reached QEA). Poison scans
/// are not re-run here — [v3d74a] establishes evidence-integrity for this boot's arrangement.
fn v3d75_kick_probe(label: &str) -> bool {
    fill_target(0xDEAD_BEEF);
    let (rcl_len, sublist_len) = build_rcl();
    cache::clean_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
    cache::clean_range(arena_phys() + OFF_RCL, rcl_len);
    cache::clean_range(arena_phys() + OFF_SUBLIST, sublist_len);
    let ba = arena_phys() + OFF_RCL;
    let ea = ba + rcl_len;
    if !arena_contains(ba, rcl_len) {
        serial_println!(":: V3D: [{}] RCL range escapes the arena — refusing kick (fail-closed) ::", label);
        return false;
    }
    clear_mmu_fault_latch(label);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
    dsb();
    let rfc_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_RFC);
    v3d73_sampler_arm(ba as u32, ea as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, ba as u32);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, ea as u32); // GO — queue auto-start
    dsb();
    let (sts, us, fldone) = wait_fldone(label, ba as u32, ea as u32);
    let (smp_n, smp_in_span, smp_max) = v3d73_sampler_read_span();
    let cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    let rfc_delta = mmio_read(V3D_CORE0_BASE, V3D_CLE_RFC).wrapping_sub(rfc_pre);
    let frdone = sts & V3D_INT_FRDONE != 0;
    cache::clean_invalidate_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
    let verified = verify_target(CLEAR_RGBA);
    let executed = verified || frdone || fldone || rfc_delta != 0 || smp_max >= ea as u32;
    serial_println!(
        ":: V3D: [{}] kick — store-verified={} FRDONE={} FLDONE={} RFCΔ={} | CT0CS={:#010x} CT0CA={:#010x} | sampler samples={} in-span={} max-in-span={:#010x} vs QBA={:#010x} QEA={:#010x} | INT_STS={:#010x} waited={}us -> {} ::",
        label, verified as u32, frdone as u32, fldone as u32, rfc_delta,
        cs_done, ca_done, smp_n, smp_in_span, smp_max, ba as u32, ea as u32, sts, us,
        if executed { "EXECUTED" } else if smp_in_span > 0 { "fetched-not-retired" } else { "DEAD" }
    );
    mmio_write(V3D_CORE0_BASE, V3D_CTL_INT_CLR, mmio_read(V3D_CORE0_BASE, V3D_CTL_INT_STS));
    dsb();
    clear_mmu_fault_latch(label);
    executed
}

/// Read a little-endian u32 out of the arena at `off` (probe scratch readback).
fn probe_word(off: usize) -> u32 {
    let arena = &raw const V3D_ARENA;
    unsafe {
        let b = &(*arena).bytes;
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
}

/// M4: bin one triangle on CT0, render it on CT1 (implicit tile list), CPU sample-verify.
/// ATTENDED-METAL-UNVERIFIED — QEMU never reaches here. On success prints the M4 PASS witness + a
/// sample table; the QPU shader body is the one metal-refinement seam (see the module banner).
/// Returns the M4 verdict (PI-V3D-12: `bringup` gates the battery on it — the battery stages layer on
/// the M4 scaffold, so running them over a failed triangle only buries the M4 witness in noise).
fn triangle_job(fb: Option<FbTarget>) -> bool {
    serial_println!(":: V3D: M4 triangle — binning on CT0, render on CT1 (implicit tile list) ::");

    // (0) Publish the shader programs, uniform streams, vertex data, default attributes. The shader
    // bodies are now REAL Mesa-packer-generated + round-trip-verified QPU words (PI-V3D-9), not NOPs:
    // coordinate/vertex passthrough (VPM in → VPM out) and a solid-colour fragment (rgba → TLB).
    let cs_len = write_shader_words(OFF_CS_CODE, &CS_VS_WORDS);
    let vs_len = write_shader_words(OFF_VS_CODE, &CS_VS_WORDS);
    let fs_len = write_shader_words(OFF_FS_CODE, &FS_WORDS);
    let fs_unif_len = write_fs_uniforms(OFF_FS_UNIF);
    let cs_unif_len = write_geo_uniforms(OFF_CS_UNIF);
    let vs_unif_len = write_geo_uniforms(OFF_VS_UNIF);
    for (i, v) in TRI_VERTS.iter().enumerate() {
        for (j, comp) in v.iter().enumerate() {
            arena_write_u32(OFF_VTXDATA + i * 16 + j * 4, comp.to_bits());
        }
    }
    fill_region(OFF_DEFAULT_ATTRS, 16, 0); // zeroed default attribute values

    // (0.5) PI-V3D-27: run the Mesa-compiled attribute-DMA probe over THIS draw's vertex buffer before
    // the real bin — a direct readout of what the VCD delivered to the QPU (does it DMA attributes into
    // VPM, or does the coord shader read zeros?). Diagnostic-only: it reuses the M4 bin-scratch regions
    // (re-zeroed below), touches no real-draw CL, and never gates M4. QEMU models no V3D so the verdict
    // is metal; see v3d.md §12.
    probe_job();

    // (0.6) PI-V3D-48: the empty-frame bisection ladder. With every per-packet suspect exonerated
    // (shader words, TILE_BINNING_MODE_CFG, FLUSH terminator, submit order, GMP, overflow pool) yet the
    // full draw's bin never retiring (FLDONE never fires, BMACTIVE stays set), submit a sequence of
    // increasingly-populated bin frames — Empty → StateNoPrims → PrimsNullShader — each with the same
    // frame-level setup + FLDONE wait, so ONE metal boot localises the offending packet class. The real
    // M4 draw below runs unchanged and remains the regression witness. Diagnostic-only; QEMU has no V3D.
    empty_frame_bisection();

    // (1) Build the shader record + attribute record, the binning CL, and the render CL.
    let num_attrs = build_shader_record();
    let bin_len = build_bin_cl(num_attrs);
    let (rcl_len, sublist_len) = build_m4_rcl();

    // (2) Pre-seed the M4 target with a sentinel distinct from BOTH colours, so the sample-verify proves
    // the GPU wrote every pixel it claims (neither clear nor triangle can appear by luck).
    fill_region(OFF_M4_TARGET, TARGET_BYTES, 0x5555_5555);

    // (3) Publish everything to RAM for the non-coherent GPU (shaders, verts, record, both lists, target,
    // and the tile-state / tile-alloc scratch the binner writes and the render reads).
    cache::clean_range(arena_phys() + OFF_CS_CODE, cs_len);
    cache::clean_range(arena_phys() + OFF_VS_CODE, vs_len);
    cache::clean_range(arena_phys() + OFF_FS_CODE, fs_len);
    cache::clean_range(arena_phys() + OFF_FS_UNIF, fs_unif_len);
    cache::clean_range(arena_phys() + OFF_CS_UNIF, cs_unif_len);
    cache::clean_range(arena_phys() + OFF_VS_UNIF, vs_unif_len);
    cache::clean_range(arena_phys() + OFF_VTXDATA, TRI_VERTS.len() * 16);
    cache::clean_range(arena_phys() + OFF_DEFAULT_ATTRS, 16);
    cache::clean_range(arena_phys() + OFF_SHADREC, 36 + 16);
    cache::clean_range(arena_phys() + OFF_BIN_CL, bin_len);
    cache::clean_range(arena_phys() + OFF_M4_RCL, rcl_len);
    cache::clean_range(arena_phys() + OFF_M4_TARGET, TARGET_BYTES);
    let _ = sublist_len; // published inside build_m4_rcl
    fill_region(OFF_TILESTATE, TILE_STATE_BYTES, 0);
    fill_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES, 0);
    cache::clean_range(arena_phys() + OFF_TILESTATE, TILE_STATE_BYTES);
    cache::clean_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);

    // (4) Kick CT0 (the BIN queue). PI-V3D-9 boot-P5 fix: program the tile-ALLOCATION pool (CT0QMA/QMS)
    // AND the tile-STATE array (CT0QTS, ENABLE-gated) as the DISTINCT regions they are — the base
    // conflated them, handing the binner a 192-byte "pool" that overflowed into an unmapped page
    // (PT_INVALID). Order per Linux v3d_sched.c v3d_bin_job_run: QMA, QMS, QTS, then QBA (begin), then
    // QEA (GO). All addresses are arena-internal identity iovas, bounds-checked (memory-safety).
    let bin_ba = (arena_phys() + OFF_BIN_CL) as u32;
    let bin_ea = bin_ba + bin_len as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32; // the binner's growable pool (CT0QMA/QMS)
    let ts = (arena_phys() + OFF_TILESTATE) as u32; // the tile-state data array (CT0QTS)
    if !arena_contains(bin_ba as usize, bin_len)
        || !arena_contains(tile_alloc as usize, BIN_TILEALLOC_BYTES)
        || !arena_contains(ts as usize, TILE_STATE_BYTES)
    {
        serial_println!(":: V3D: M4 bin range escapes the arena — refusing kick (fail-closed) ::");
        return false;
    }
    // PI-V3D-15 (brief lead #1 attribution): clear any stale MMU fault BEFORE the bin kick so the
    // post-bin decode below is provably THIS bin's fault — the M4 bin clue's MMU_fault=0x100000 could
    // otherwise be a fault latched by program_mmu/M3 and never cleared (there was no pre-bin clear).
    // (brief lead #2): dump the exact bin CL byte stream the binner will parse, to read against Mesa's
    // emit order for a mis-sized packet shifting an opcode into an address field (PI-V3D-10 class).
    clear_mmu_fault_latch("v3d15 pre-bin (attribution)");
    // PI-V3D-62: arm the fault-REPORTING instrument for this bin frame — clear the hub interrupt latch
    // and seed the (now unmapped, job-less) illegal-address scratch page. Must follow the MMU-latch clear
    // above so all three channels share one attribution window. Closed at wait-exit by
    // `v3d62_frame_witness`, which runs BEFORE the post-bin W1C clear that would erase channel (1).
    v3d62_arm("M4 bin");
    // PI-V3D-23: announce the two Mesa-conformance corrections this arc applies to the bin CL, so the
    // metal log names the change under test. VCM Vc: 1 → 4 (GFXH-1744 floor is 2; Mesa computes 4 for
    // this draw), + OCCLUSION_QUERY_COUNTER(addr=0) added to the prologue (Mesa's OQ-disable). The
    // discriminator is the post-bin tile-alloc pool / tile-STATE going non-zero (bin_pool_witness).
    serial_println!(
        ":: V3D: [v3d23] M4 bin — VCM_CACHE_SIZE Vc={} (was 1; GFXH-1744 floor 2, Mesa-computed 4) + OCCLUSION_QUERY_COUNTER disable in prologue — WATCH the tile-alloc pool / tile-STATE below ::",
        VCM_CACHE_BATCHES
    );
    dump_cl_bytes("M4 bin", OFF_BIN_CL, bin_len, 64);
    // [v3d36] decode the WORKING M4 bin CL packet-by-packet — the reference the PROBE decode (emitted by
    // probe_job under the same tag) is diffed against. Same builder → same packets; the only field that
    // may differ is GL_SHADER_STATE `record` (M4 record vs probe record).
    decode_cl_packets("M4", OFF_BIN_CL, bin_len);
    // PI-V3D-57: the same list again, read back from the published bytes and packing-checked field by
    // field against the audited v42 encoding (see the packing-consistency note on v3d57_cl_mesa_diff).
    v3d57_cl_mesa_diff("M4", OFF_BIN_CL, bin_len);
    // PI-V3D-57: kernel-exact ORDER — `v3d_bin_job_run`'s FIRST write is BPOS=0, and only then the
    // per-job cache invalidate below. (V3D-12 read "invalidate first" from the middle of that function.)
    bin_prejob_bpos_clear("M4");
    // PI-V3D-12: the Linux per-job pre-kick cache invalidate (v3d_bin_job_run, right after the BPOS clear).
    invalidate_gpu_caches("L2T flush (M4 bin pre-kick)");
    let ct0_cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, tile_alloc); // tile-allocation pool base
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, BIN_TILEALLOC_BYTES as u32); // …and its size
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, ts | V3D_CLE_CT0QTS_ENABLE); // tile-state array (enabled)
    dsb();
    // PI-V3D-13 witness: prove the bin-memory registers hold what we wrote BEFORE the GO.
    bin_mem_prekick_witness(
        "M4",
        tile_alloc,
        BIN_TILEALLOC_BYTES as u32,
        ts | V3D_CLE_CT0QTS_ENABLE,
    );
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, bin_ba);
    dsb(); // BA latched before the GO
    // PI-V3D-21: arm the QPU-execution performance counters immediately before the GO so they span the
    // bin (coord-shader) run. Read-only w.r.t. the shader — cannot perturb execution (see pctr_* above).
    pctr_setup_cs_witness();
    // [v3d47] dump the published coord-shader thread-end bytes right before the GO — P45 confirms WHAT
    // RAN. V3D-47 finding: this tail is byte-for-byte Mesa's own v3d_compile coord tail (no divergence).
    cs_tail_witness("M4", OFF_CS_CODE, CS_VS_WORDS.len());
    // (The BPOS=0 overflow clear this kick used to issue here now leads the sequence — PI-V3D-57.)
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, bin_ea); // GO
    dsb();
    let ct0_cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    let bin_idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CT1CS_CTRUN, "CT0 bin");
    let ct0_cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    let mmu_ctl_bin = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let bin_fault = mmu_ctl_bin
        & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    let bin_ran = ct0_ran(ct0_cs_pre, ct0_cs_kicked, ct0_cs_done, ct0_ca_done, bin_ba, bin_ea);
    serial_println!(
        ":: V3D: M4 bin clue — CT0CS pre={:#010x} kicked={:#010x} done={:#010x} CT0CA pre={:#010x} kicked={:#010x} done={:#010x} (BA={:#010x} EA={:#010x}) ran={} idled={} MMU_fault={:#x} ::",
        ct0_cs_pre, ct0_cs_kicked, ct0_cs_done, ct0_ca_pre, ct0_ca_kicked, ct0_ca_done,
        bin_ba, bin_ea, bin_ran as u32, bin_idled as u32, bin_fault
    );
    // PI-V3D-21: read the QPU-execution counters now the bin has idled — THE decisive verdict for this
    // arc (coord-shader QPU active cycles nonzero ⇒ the shader ran). Done before the render kick so the
    // reading isolates the bin (coord) shader.
    pctr_read_cs_witness("M4 post-bin");
    // PI-V3D-13 witness: post-bin, did the binner's output actually land in the pool?
    bin_pool_witness("M4 post-bin");
    // PI-V3D-18 witness (V3D-16-mandated): the shader-state record bytes the CLE handed the PTB, plus
    // the CONTRACTED 6-word coordinate-shader VPM output vs. our 4-word passthrough — so every boot
    // shows the two screen-space words (out-offsets 4,5) the CS omits, the confirmed empty-bin cause.
    cs_vpm_output_witness("M4 post-bin");
    // PI-V3D-29: audit the attribute record's base/stride/enable/count fields + the shader-record VPM
    // segment-size gates against the Mesa v42 contract — the pre-armed verdict tool for V3D-28's
    // loaded-zeros branch (VCD never DMA'd the vertex buffer into VPM). Instrumentation only.
    attr_record_audit_witness("M4 post-bin", num_attrs);
    // PI-V3D-15 (brief lead #1): decode WHERE the bin faulted — the clue above reports the fault BITS
    // but never the address. With the latch cleared pre-kick, a fault here is THIS bin's, and its VA
    // tells whether the binner walked off the arena (our encoding bug) or idled legally in-bounds.
    bin_fault_witness("M4 bin");
    // PI-V3D-62: close the fault-reporting window while the evidence is still standing. Deliberately
    // placed BEFORE `clear_mmu_fault_latch("post-bin")` below — that clear is write-1-to-clear on exactly
    // the bits this witness reads.
    v3d62_frame_witness("M4 bin");
    super::exceptions::serror_drain_request("v3d: M4 bin kick window");

    // PI-V3D-9 boot-P5 fix: clear any latched V3D-MMU fault BEFORE the render kick. Boot-P5 showed the
    // render CT1 refused to start (CTRUN never latched, CT1CA parked at M3's end) while the MMU carried
    // a latched PT_INVALID+WRITE_VIOLATION from the (then-broken) bin — a fault the abort policy holds
    // sticky, wedging subsequent submissions. The clear is the exact Linux v3d_irq.c idiom: read
    // V3D_MMU_CTL and write it back (the fault status bits are write-1-to-clear; writing the read-back
    // value clears them while preserving ENABLE/abort config). Harmless when no fault is latched (the
    // fault bits read 0, so the write-back is a no-op on them). With the bin fault fixed above this is
    // belt-and-suspenders; it also un-wedges the render if any unrelated fault slipped in.
    clear_mmu_fault_latch("post-bin");

    // (5) Kick CT1 (the RENDER queue) over the M4 RCL — same submit path as M3, different list. It
    // consumes the binner's per-tile lists via BRANCH_TO_IMPLICIT_TILE_LIST.
    let rcl_ba = (arena_phys() + OFF_M4_RCL) as u32;
    let rcl_ea = rcl_ba + rcl_len as u32;
    if !arena_contains(rcl_ba as usize, rcl_len) {
        serial_println!(":: V3D: M4 render range escapes the arena — refusing kick (fail-closed) ::");
        return false;
    }
    // PI-V3D-12 — THE boot-P7 fix. The render CLE consumes the BINNER's tile lists; without the Linux
    // per-job invalidate (v3d_render_job_run also runs it) the L2T still held the CPU's pre-bin
    // zero-fill of the tile-alloc pool, so the BRANCH_TO_IMPLICIT_TILE_LIST fetched 0x00 = Halt at the
    // pool base and the CLE stopped there (boot-P7: CT1CA done 0x00206000 = arena+OFF_BIN_TILEALLOC,
    // BELOW BA) without ever reaching the sub-list's STORE. Flush L2T + invalidate slices so the
    // render observes the bin's actual output.
    invalidate_gpu_caches("L2T flush (M4 render pre-kick)");
    let r_cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QBA, rcl_ba);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QEA, rcl_ea); // GO
    dsb();
    let r_cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let r_idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT1CS, V3D_CLE_CT1CS_CTRUN, "CT1 M4 render");
    let r_cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let r_ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
    let mmu_ctl_r = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let r_fault = mmu_ctl_r
        & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    let r_ran = ct0_ran(r_cs_pre, r_cs_kicked, r_cs_done, r_ca_done, rcl_ba, rcl_ea);
    // PI-V3D-9: decode the one CTnCS bit corroborated for V3D 4.x — CTRUN (bit5, "list running"). The
    // kicked snapshot having CTRUN set is the positive proof the render actually started (boot-P5 had
    // CTRUN clear here → the wedge the fault-latch clear above targets); other CTnCS bits are reported
    // raw, not guessed.
    let r_ctrun_kicked = (r_cs_kicked & V3D_CLE_CTNCS_CTRUN != 0) as u32;
    serial_println!(
        ":: V3D: M4 render clue — CT1CS pre={:#010x} kicked={:#010x} (CTRUN={}) done={:#010x} CT1CA done={:#010x} (BA={:#010x} EA={:#010x}) ran={} idled={} MMU_fault={:#x} ::",
        r_cs_pre, r_cs_kicked, r_ctrun_kicked, r_cs_done, r_ca_done, rcl_ba, rcl_ea, r_ran as u32, r_idled as u32, r_fault
    );
    // PI-V3D-12 CA-locus decode: CT1CA below BA is NOT a stale queued job — the CLE's CA follows
    // branches. Parked inside the bin tile-alloc pool = the BRANCH_TO_IMPLICIT_TILE_LIST destination,
    // i.e. the CLE halted INSIDE the (stale/empty) binned tile list before the sub-list's STORE.
    let ta_base = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    if r_ca_done >= ta_base && r_ca_done < ta_base + BIN_TILEALLOC_BYTES as u32 {
        serial_println!(
            ":: V3D: M4 render clue — CT1CA parked IN the bin tile-alloc pool (+{:#x}): the CLE halted inside the implicit (binned) tile list, before the STORE ::",
            r_ca_done - ta_base
        );
    }
    super::exceptions::serror_drain_request("v3d: M4 render kick window");

    if !bin_idled || !r_idled {
        serial_println!(":: V3D: M4 — a CLE did not idle within budget (anti-hang backstop) — no verify ::");
        return false;
    }

    // (6) CPU sample-verify: pull the target back from DRAM and check inside/outside samples.
    cache::clean_invalidate_range(arena_phys() + OFF_M4_TARGET, TARGET_BYTES);
    let pass = verify_triangle_samples();
    if pass {
        serial_println!(":: V3D: M4 triangle — PASS (inside samples = triangle colour, outside = clear) ::");
        if let Some(fb) = fb {
            blit_m4_target(&fb);
        }
    } else {
        serial_println!(":: V3D: M4 triangle — FAIL/UNRENDERED (see sample table; QPU shader body is the metal-refinement seam) ::");
    }
    pass
}

/// Read one 32-bit pixel from the M4 target at (x, y).
#[inline]
fn m4_sample(x: usize, y: usize) -> u32 {
    let off = OFF_M4_TARGET + (y * TARGET_W + x) * TARGET_BPP;
    let arena = &raw const V3D_ARENA;
    unsafe {
        let b = &(*arena).bytes;
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
}

/// Sample-verify the rendered triangle: ≥3 interior points must equal TRI_RGBA and ≥3 exterior points
/// must equal CLEAR_RGBA (per the brief). Interior points cluster around the centroid (~32,32); exterior
/// points sit in the corners the centred triangle does not cover. Prints the full sample table (the M4
/// witness) so the attended sitting sees exactly what landed even on a partial render.
fn verify_triangle_samples() -> bool {
    // Screen coords chosen from TRI_VERTS mapped to the 64×64 viewport (y-down): centroid ≈ (32,34).
    let inside: [(usize, usize); 3] = [(32, 34), (26, 40), (38, 40)];
    let outside: [(usize, usize); 3] = [(2, 2), (61, 2), (32, 4)];
    let mut ok = true;
    for (x, y) in inside {
        let px = m4_sample(x, y);
        let hit = px == TRI_RGBA;
        ok &= hit;
        serial_println!(
            ":: V3D: M4 sample IN  ({:2},{:2}) = {:#010x} expect {:#010x} {} ::",
            x, y, px, TRI_RGBA, if hit { "OK" } else { "MISS" }
        );
    }
    for (x, y) in outside {
        let px = m4_sample(x, y);
        let hit = px == CLEAR_RGBA;
        ok &= hit;
        serial_println!(
            ":: V3D: M4 sample OUT ({:2},{:2}) = {:#010x} expect {:#010x} {} ::",
            x, y, px, CLEAR_RGBA, if hit { "OK" } else { "MISS" }
        );
    }
    ok
}

/// Blit the M4 target next to the M3 target on the panel (metal visible witness) — offset to the right
/// so both are visible. Bounds-clipped to the framebuffer.
fn blit_m4_target(fb: &FbTarget) {
    if fb.base == 0 || fb.bytes_per_pixel < 4 {
        return;
    }
    let x_origin = TARGET_W + 8; // to the right of the M3 blit
    let w = TARGET_W.min(fb.width.saturating_sub(x_origin));
    let h = TARGET_H.min(fb.height);
    for y in 0..h {
        for x in 0..w {
            let px = m4_sample(x, y);
            let dst = fb.base as usize
                + y * fb.stride_px * fb.bytes_per_pixel
                + (x_origin + x) * fb.bytes_per_pixel;
            if dst + 4 <= fb.base as usize + fb.size {
                unsafe { core::ptr::write_volatile(dst as *mut u32, px) };
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-11 — the visible graphics battery (M5..M8), LAYERED on the M4 triangle scaffold.
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// Four short on-screen stages, each serial-witnessed (`:: V3D: M<stage> … ::`) + eyeball-verified at
// the attended sitting. Everything below is ADDITIVE: new arena regions above the M4 regions, new
// builder/kick functions that mirror (not modify) the M4 idiom, and one call in `bringup`. The M3
// clear and M4 triangle above stay byte-identical as the head-of-battery regressions.
// ATTENDED-METAL-UNVERIFIED throughout — QEMU raspi4b returns at BLOCK-DOWN long before this runs.
//
// ── QPU-word provenance (the standing thrice-convicted rule — no fabricated bit patterns) ────────
// GRAD_VS_WORDS (the render-path M5 vertex shader) is FULLY Mesa-packed (v3d_qpu_instr_pack, ver 42) +
// round-tripped by scripts/pi-v3d22-qpu-gen.c — see its .out.txt; PI-V3D-22 switched it off the
// (dead-on-4.x) mov-vpm/vpmsetup mechanism to STVPMV, so no `mov vpm` word remains to derive from.
// GRAD_FS_WORDS below is still derived from the PI-V3D-9 Mesa-packer-verified vectors already in this
// file by SINGLE-FIELD surgery, where the touched field's encoding is itself corroborated by multiple
// in-file verified words (the same "CT1 = CT0 + 4" class the CT0 registers used):
//   * SIG field [57:53]: corroborated by nop(sig=0), thrsw(sig=1 → bit53, in-file 0x3c20…) and
//     ldunifrf(sig=12 → bits56+55, in-file 0x3d80…). ldvary is sig=8 (Mesa qpu_pack.c v41 sig map,
//     the same table that yields 1/12 for the corroborated entries) → bit56 alone.
//   * SIG dest addr [51:46] (rf#): corroborated by the in-file ldunifrf.rf0..rf3 (+0x0000/bit46/
//     bit47/bits46+47) and ldunifrf.rf5 words.
//   * WADDR_A [37:32]: corroborated by the in-file ldvpmv_in rf0..rf3 sequence (+0..3 at bit 32).
// The gradient-FS SEMANTICS (raw ldvary A-coefficients written to the TLB without the fmul/fadd(W,C)
// interpolation evaluation) are the honest metal-refinement seam of this arc, exactly like the
// PI-V3D-9 viewport/VPM quantities — flagged at the M5 verdict.

// ─── Battery arena regions: 0x21000..0x34000, all 4 KiB-aligned starts, all ABOVE the M4 regions
// (top prior used byte ≈ 0x200C0) and inside the 256 KiB arena → inside the identity MMU map. ───
const OFF_M5_TARGET: usize = 0x21000; // [16 KiB) M5 gradient render target
const OFF_M5_VTX: usize = 0x25000; // 3 verts × 32 B (vec4 pos + vec4 colour, interleaved)
const OFF_M5_FS_CODE: usize = 0x25800; // gradient fragment shader (ldvary path)
const OFF_M5_VS_CODE: usize = 0x26000; // gradient vertex shader (render-path STVPMV output)
const OFF_M5_FS_UNIF: usize = 0x26800; // gradient FS uniforms (alpha + TLB configs)
const OFF_M5_VS_UNIF: usize = 0x26880; // gradient VS uniforms (8 in-offsets + XY/Z scale + 8 out-offsets)
const OFF_M5_SHADREC: usize = 0x26900; // M5 shader record + 2 attribute records (32-B aligned)
const OFF_M5_BIN_CL: usize = 0x27000;
const OFF_M5_RCL: usize = 0x28000;
const OFF_M5_SUBLIST: usize = 0x29000;
const OFF_BAT_TARGET: usize = 0x2A000; // [16 KiB) shared M6/M7 render target
const OFF_BAT_VTX: usize = 0x2E000; // animated / multi-primitive vertex data
const OFF_BAT_BIN_CL: usize = 0x2F000;
const OFF_BAT_RCL: usize = 0x30000;
const OFF_BAT_SUBLIST: usize = 0x31000;
const OFF_BAT_SHADREC: usize = 0x32000; // M6 record @+0; M7 records @+128×k (k=0..3; 52 B each)
const OFF_M7_UNIF: usize = 0x33000; // 4 FS uniform streams, 64-B stride (one per M7 colour draw)
const _: () = assert!(OFF_M7_UNIF + 4 * 64 <= ARENA_BYTES);

// ─── Battery QPU shader bodies (field-surgery derivations — provenance in the banner above). ───

/// M5 gradient FRAGMENT shader: pop three varyings (r, g, b A-coefficients) via ldvary into rf0..rf2,
/// alpha from the uniform FIFO into rf3, then the same passthrough-Z + double-VFPACK TLB write as the
/// verified FS_WORDS. Uniform FIFO order: alpha, Z-config, colour-config. Metal-refinement seam: the
/// varying interpolation math (fmul/fadd with W and the C coefficient) is NOT evaluated — the raw
/// per-fragment ldvary results land in the TLB, which is sufficient for the M5 witness (three
/// pairwise-distinct non-clear interior samples) but not yet colour-exact.
const GRAD_FS_WORDS: [u64; 10] = [
    0x3d00_3186_bb80_0000, // nop ; ldvary.rf0   (varying 0 → r)
    0x3d00_7186_bb80_0000, // nop ; ldvary.rf1   (varying 1 → g)
    0x3d00_b186_bb80_0000, // nop ; ldvary.rf2   (varying 2 → b)
    0x3d80_f186_bb80_0000, // nop ; ldunifrf.rf3 (rf3 <- alpha)   [verbatim in-file word]
    0x3c00_3206_bbe0_0000, // mov tlbu, r0       (passthrough-Z; pops Z TLB-config)
    0x3c00_3188_3583_e001, // vfpack tlbu, rf0, rf1 (pops colour TLB-config)
    0x3c00_3187_3583_e083, // vfpack tlb, rf2, rf3
    0x3c20_3186_bb80_0000, // nop ; thrsw
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// M5 gradient VERTEX shader — the RENDER-path VS, written with the V3D 4.2 STVPMV output mechanism.
///
/// PI-V3D-22 applies the PI-V3D-20 root-cause fix to this shader. The prior body wrote its per-vertex
/// VPM output with the streamed VC4 / V3D-3.3 mechanism — `vpmsetup` + `mov vpm, rfN` (magic waddr
/// VPM=14) — which DOES NOT EXIST for per-vertex shader output on V3D 4.x (ver==42, the Pi 4). Mesa's
/// `vir_VPM_WRITE` (src/broadcom/compiler/nir_to_vir.c) emits ONE `vir_STVPMV(c, vir_uniform_ui(c,
/// vpm_index), val)` per output component — a store-VPM with an EXPLICIT VPM offset — and no
/// `mov vpm`/`vpmsetup` in the ver-42 VS output path. The old `mov vpm` writes went to an unconfigured
/// magic register; the rasterizer read nothing. `vpmsetup` is DROPPED; VPMWT stays (GFXH-1684).
///
/// NON-COORD (render) VPM CONTRACT — Mesa `v3d_nir_setup_vpm_layout_vs` (v3d_nir_lower_io.c), for
/// is_coord==false / is_last_geometry_stage: the position block is FOUR words —
///     [Xs @0, Ys @1, Zs @2, 1/Wc @3]  — then user varyings at offset 4+.
/// This is NOT the six-word coordinate-shader layout ([Xc,Yc,Zc,Wc, Xs,Ys]); the render VS does not
/// emit the four clip words (only the is_coord path does, at offsets 0..3). PI-V3D-18 landed this split.
///
/// SCREEN MATH — Mesa `v3d_nir_emit_ff_vpm_outputs` (same file): rcp_wc = frcp(Wc);
///     Xs = f2i32(ffloor(Xc·vp_scale·rcp_wc)), Ys likewise (ffloor is the ver==42 branch);
///     Zs = Zc·viewport_z_scale·rcp_wc + viewport_z_offset;  out3 = rcp_wc.
/// vp_scale = viewport.scale(32)·clipper_xy_granularity(256) = 8192. viewport_z_scale/offset are the
/// SAME viewport Z params the M5 RCL programs into CLIPPER_Z_SCALE_AND_OFFSET (0.5/0.5), sourced here
/// as uniforms exactly as Mesa sources QUNIFORM_VIEWPORT_Z_SCALE/_OFFSET.
///
/// W=1 SIMPLIFICATION (LOUD, same stance as PI-V3D-19/20): M5's TRI_VERTS all carry Wc = 1.0, so
/// rcp_wc = 1.0 and NO reciprocal is emitted — Xs = f2i32(floor(Xc·8192)); Zs = Zc·0.5 + 0.5; the
/// 1/Wc word is the rf3 (Wc) passthrough, exact because Wc==1.0. A perspective draw (W≠1) would need a
/// per-vertex reciprocal restored on Xs/Ys/Zs here.
///
/// Register map: rf0..3 = clip Xc,Yc,Zc,Wc; rf5 = in read-offset (reused); rf6 = 8192.0; rf7 = Xs;
/// rf8 = Ys; rf10..13 = colour r,g,b,a; rf14 = z_scale; rf15 = z_offset; rf16 = Zs; rf20..27 = the
/// eight output VPM offsets 0..7 (Mesa-sourced as ui uniforms). Uniform FIFO: [in-off 0..7, 8192.0f,
/// 0.5f, 0.5f, out-off 0..7] = 19 words. Colour flows VS→FS as four varyings @4..7 (num_varyings=4,
/// record unchanged); the FS's raw (un-interpolated) varying read stays the M5 metal-refinement seam.
///
/// PROVENANCE: every word Mesa-packed (v3d_qpu_instr_pack, ver 42) + round-tripped by
/// scripts/pi-v3d22-qpu-gen.c (see its .out.txt); the carried V3D-20 words (ldvpmv_in, fmul/ffloor/
/// ftoiz, stvpmv) are bit-identical to CS_VS_WORDS. QEMU models no V3D — metal decides.
const GRAD_VS_WORDS: [u64; 39] = [
    0x3d81_6180_bc80_6140, // ldvpmv_in rf0, rf5 ; ldunifrf.rf5   (pos.Xc)
    0x3d81_6181_bc80_6140, // ldvpmv_in rf1, rf5 ; ldunifrf.rf5   (pos.Yc)
    0x3d81_6182_bc80_6140, // ldvpmv_in rf2, rf5 ; ldunifrf.rf5   (pos.Zc)
    0x3d81_6183_bc80_6140, // ldvpmv_in rf3, rf5 ; ldunifrf.rf5   (pos.Wc)
    0x3d81_618a_bc80_6140, // ldvpmv_in rf10, rf5 ; ldunifrf.rf5  (col.r)
    0x3d81_618b_bc80_6140, // ldvpmv_in rf11, rf5 ; ldunifrf.rf5  (col.g)
    0x3d81_618c_bc80_6140, // ldvpmv_in rf12, rf5 ; ldunifrf.rf5  (col.b)
    0x3d81_618d_bc80_6140, // ldvpmv_in rf13, rf5 ; ldunifrf.rf5  (col.a)
    0x3d81_b186_bb80_0000, // nop ; ldunifrf.rf6    (rf6 <- 8192.0f vp_scale)
    0x3d83_b186_bb80_0000, // nop ; ldunifrf.rf14   (rf14 <- viewport_z_scale 0.5f)
    0x3d83_f186_bb80_0000, // nop ; ldunifrf.rf15   (rf15 <- viewport_z_offset 0.5f)
    0x3d85_3186_bb80_0000, // nop ; ldunifrf.rf20   (out-offset 0)
    0x3d85_7186_bb80_0000, // nop ; ldunifrf.rf21   (out-offset 1)
    0x3d85_b186_bb80_0000, // nop ; ldunifrf.rf22   (out-offset 2)
    0x3d85_f186_bb80_0000, // nop ; ldunifrf.rf23   (out-offset 3)
    0x3d86_3186_bb80_0000, // nop ; ldunifrf.rf24   (out-offset 4)
    0x3d86_7186_bb80_0000, // nop ; ldunifrf.rf25   (out-offset 5)
    0x3d86_b186_bb80_0000, // nop ; ldunifrf.rf26   (out-offset 6)
    0x3d86_f186_bb80_0000, // nop ; ldunifrf.rf27   (out-offset 7)
    0x5400_11c6_bbf8_0006, // fmul rf7, rf0, rf6    (Xc · 8192.0 ; W=1 so no 1/Wc)
    0x3c00_2187_f680_61c0, // ffloor rf7, rf7       (floor, ver==42 path)
    0x3c00_2187_f583_e1c0, // ftoiz rf7, rf7        (f2i32 -> Xs)
    0x5400_1206_bbf8_0046, // fmul rf8, rf1, rf6    (Yc · 8192.0)
    0x3c00_2188_f680_6200, // ffloor rf8, rf8       (floor, ver==42 path)
    0x3c00_2188_f583_e200, // ftoiz rf8, rf8        (f2i32 -> Ys)
    0x5400_1406_bbf8_008e, // fmul rf16, rf2, rf14  (Zc · z_scale ; W=1 so no 1/Wc)
    0x3c00_2190_0583_e40f, // fadd rf16, rf16, rf15 (+ z_offset -> Zs)
    0x3c00_2180_f883_e507, // stvpmv rf20, rf7      (out0 screen Xs @ offset 0)
    0x3c00_2180_f883_e548, // stvpmv rf21, rf8      (out1 screen Ys @ offset 1)
    0x3c00_2180_f883_e590, // stvpmv rf22, rf16     (out2 screen Zs @ offset 2)
    0x3c00_2180_f883_e5c3, // stvpmv rf23, rf3      (out3 1/Wc = Wc = 1.0 @ offset 3)
    0x3c00_2180_f883_e60a, // stvpmv rf24, rf10     (out4 varying col.r @ offset 4)
    0x3c00_2180_f883_e64b, // stvpmv rf25, rf11     (out5 varying col.g @ offset 5)
    0x3c00_2180_f883_e68c, // stvpmv rf26, rf12     (out6 varying col.b @ offset 6)
    0x3c00_2180_f883_e6cd, // stvpmv rf27, rf13     (out7 varying col.a @ offset 7)
    0x3c00_3186_bb81_6000, // vpmwt                 (VPM writes complete before end)
    0x3c20_3186_bb80_0000, // nop ; thrsw           (end)
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// The M5 per-vertex colours (unorm8 RGBA words) — one primary per corner, so interpolation (or even
/// raw per-fragment varying data) yields three PAIRWISE-DISTINCT interior samples near the corners.
const M5_VERT_COLOURS: [u32; 3] = [0x0000_00FF, 0x0000_FF00, 0x00FF_0000]; // red, green, blue

/// M6 animation cadence: 24 rotation steps × 6 revolutions ≈ 5 s at ~33 ms/frame.
const M6_FRAMES: usize = 144;
const M6_FRAME_PACE_MS: u64 = 30;

/// 24-step unit-circle table (cos, sin at k×15°), f32 — no libm in the kernel; precomputed.
const ROT24: [(f32, f32); 24] = [
    (1.0, 0.0),
    (0.965926, 0.258819),
    (0.866025, 0.5),
    (0.707107, 0.707107),
    (0.5, 0.866025),
    (0.258819, 0.965926),
    (0.0, 1.0),
    (-0.258819, 0.965926),
    (-0.5, 0.866025),
    (-0.707107, 0.707107),
    (-0.866025, 0.5),
    (-0.965926, 0.258819),
    (-1.0, 0.0),
    (-0.965926, -0.258819),
    (-0.866025, -0.5),
    (-0.707107, -0.707107),
    (-0.5, -0.866025),
    (-0.258819, -0.965926),
    (0.0, -1.0),
    (0.258819, -0.965926),
    (0.5, -0.866025),
    (0.707107, -0.707107),
    (0.866025, -0.5),
    (0.965926, -0.258819),
];

/// M7 draw colours (one per 3-wedge group of the 12-triangle pinwheel): red, green, blue, amber.
const M7_COLOURS: [u32; 4] = [0x0000_00FF, 0x0000_FF00, 0x00FF_0000, TRI_RGBA];

/// The battery sentinel — distinct from CLEAR_RGBA, TRI_RGBA and every M5/M7 draw colour, so a
/// sample equal to it proves "GPU never wrote this pixel".
const BAT_SENTINEL: u32 = 0x5555_5555;

/// One bin→render job outcome, shared by every battery stage (the M4 kick idiom, parameterised).
struct JobResult {
    bin_ran: bool,
    bin_idled: bool,
    r_ran: bool,
    r_idled: bool,
    /// OR of the MMU fault-status bits observed after the bin and after the render.
    fault: u32,
}
impl JobResult {
    fn clean(&self) -> bool {
        self.bin_ran && self.bin_idled && self.r_ran && self.r_idled && self.fault == 0
    }
}

/// Quiet variant of `clear_mmu_fault_latch`: same Linux v3d_irq.c read-echo W1C idiom, but returns the
/// latched fault bits instead of printing — the M6 frame loop calls this per frame and 144 lines of
/// latch chatter would bury the serial witness (quiet-boot law). A non-zero return is reported once in
/// the stage verdict.
fn clear_mmu_fault_latch_quiet() -> u32 {
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    if fault != 0 {
        mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, ctl); // W1C echo
        dsb();
    }
    fault
}

/// PI-V3D-13 pre-kick witness: read back the three bin-memory registers just written (CT0QMA =
/// tile-allocation pool base, CT0QMS = its size, CT0QTS = tile-state array base | ENABLE). The
/// PI-V3D-13 fact-check confirmed the programming model against Linux v3d_regs.h/v3d_sched.c
/// verbatim (offsets 0x170/0x174/0x15c, ENABLE=BIT(1), order QMA→QMS→QTS→QBA→QEA-GO), so a readback
/// that does NOT echo what we wrote is itself the boot-P8 clue: either the slots are not where the
/// silicon holds them or the writes are not landing.
fn bin_mem_prekick_witness(tag: &str, qma_w: u32, qms_w: u32, qts_w: u32) {
    let qma = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMA);
    let qms = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMS);
    let qts = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QTS);
    serial_println!(
        ":: V3D: {} bin-mem regs — CT0QMA={:#010x} (wrote {:#010x}) CT0QMS={:#010x} (wrote {:#010x}) CT0QTS={:#010x} (wrote {:#010x}) echo={} ::",
        tag, qma, qma_w, qms, qms_w, qts, qts_w,
        (qma == qma_w && qms == qms_w && qts == qts_w) as u32
    );
}

/// Read `n` (≤8) bytes from the arena at `off` into a fixed 8-byte buffer WITHOUT any cache
/// maintenance — a raw CPU load through whatever the D-cache currently holds. Used to capture the
/// PRE-flush (possibly stale) view for the V3D-42 re-read.
fn arena_head8(off: usize) -> [u8; 8] {
    let arena = &raw const V3D_ARENA;
    let mut b = [0u8; 8];
    unsafe {
        for (i, h) in b.iter_mut().enumerate() {
            *h = (*arena).bytes[off + i];
        }
    }
    b
}

/// PI-V3D-13 post-bin witness — V3D-42 corrected. Did the BINNER actually write its output into the
/// tile-alloc pool / tile-state? The subtlety this arc fixes: the binner's writes land in the GPU's
/// L2T cache, NOT DRAM — the PTB's `tmuwt`/store acceptance only reaches L2T (exactly the V3D-28
/// mechanism proven for the probe-scratch TMU store). This witness previously read the pool BEFORE any
/// post-bin L2T write-back (the render pre-kick flush ran AFTER it), so the CPU invalidated its own
/// cache and read stale-zero DRAM while the binner's bytes sat in L2T — the P40 contradiction (BPCA
/// advanced 0x3000 off the pool base, yet every CPU pool read was zero). The read path was suspect,
/// not the binner. Fix: FLUSH the V3D L2T (write-back + invalidate) so the binner's pool/tile-state
/// writes reach DRAM, THEN invalidate the CPU's stale copy and read. The GPU-given iova (CT0QMA =
/// arena_phys()+OFF_BIN_TILEALLOC, V3D-MMU-identity-mapped) and the CPU-read VA are printed side by
/// side — under the identity map they are the SAME physical page, which is the point the P40 evidence
/// forced us to prove. A pre-flush (stale) re-read is captured first so the log shows the L2T flush
/// flipping zeros → binner bytes. Nonzero head = the binner wrote a tile list.
fn bin_pool_witness(tag: &str) -> bool {
    let pool_iova = (arena_phys() + OFF_BIN_TILEALLOC) as u32; // exactly what was written to CT0QMA
    let ts_iova = (arena_phys() + OFF_TILESTATE) as u32; // exactly what was written to CT0QTS (sans ENABLE)

    // (1) PRE-flush snapshot: the stale view the un-fixed witness reported. Invalidate the CPU line so
    // this reflects current DRAM (not a stale CPU line), but do NOT touch the GPU L2T yet — if the
    // binner's bytes are still parked in L2T this reads zero, and the post-flush read below flips it.
    cache::clean_invalidate_range(arena_phys() + OFF_BIN_TILEALLOC, 64);
    cache::clean_invalidate_range(arena_phys() + OFF_TILESTATE, 8);
    let pool_pre = arena_head8(OFF_BIN_TILEALLOC);
    let ts_pre = arena_head8(OFF_TILESTATE);

    // (2) THE V3D-42 FIX: write the V3D L2T back to DRAM so the binner's pool + tile-state writes are
    // visible to the CPU, then drop the CPU's now-stale zero lines and re-read from DRAM truth.
    invalidate_gpu_caches("L2T write-back (post-bin pool readback — drain the binner's writes to DRAM)");
    cache::clean_invalidate_range(arena_phys() + OFF_BIN_TILEALLOC, 64);
    cache::clean_invalidate_range(arena_phys() + OFF_TILESTATE, 8);
    let pool = arena_head8(OFF_BIN_TILEALLOC);
    let ts = arena_head8(OFF_TILESTATE);

    let wrote = pool.iter().any(|&b| b != 0);
    // Address witness: the GPU was handed `pool_iova`; the CPU reads `cpu_va`. Under the V3D-MMU
    // identity map these name the same physical page — printing both lets the metal log confirm the
    // binner and the CPU touch the same memory (the P40 same-page question, settled inline).
    let cpu_va = unsafe { core::ptr::addr_of!((*(&raw const V3D_ARENA)).bytes[OFF_BIN_TILEALLOC]) } as usize;
    serial_println!(
        ":: V3D: [v3d42] {} pool addr — GPU iova (CT0QMA)={:#010x} CPU read VA={:#010x} ({}) | tile-STATE iova (CT0QTS)={:#010x} ::",
        tag, pool_iova, cpu_va,
        if cpu_va == pool_iova as usize { "SAME physical page (identity map)" } else { "MISMATCH — GPU and CPU address different pages!" },
        ts_iova
    );
    serial_println!(
        ":: V3D: {} tile-alloc pool[0..8] pre-L2T-flush = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} → post-flush = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} — {} ::",
        tag,
        pool_pre[0], pool_pre[1], pool_pre[2], pool_pre[3], pool_pre[4], pool_pre[5], pool_pre[6], pool_pre[7],
        pool[0], pool[1], pool[2], pool[3], pool[4], pool[5], pool[6], pool[7],
        if wrote {
            if pool_pre.iter().all(|&b| b == 0) { "nonzero AFTER L2T flush: the binner WROTE the pool (writes were parked in L2T — V3D-42)" }
            else { "nonzero: the binner WROTE the pool" }
        } else { "all zero: the binner never wrote the pool" }
    );
    // PI-V3D-17 (V3D-16 ask): the tile-STATE array head (CT0QTS). The PTB writes per-tile state (TSDA)
    // here as it bins; nonzero corroborates the pool witness. Same pre/post-flush re-read.
    serial_println!(
        ":: V3D: {} tile-STATE[0..8] pre-L2T-flush = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} → post-flush = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} — {} ::",
        tag,
        ts_pre[0], ts_pre[1], ts_pre[2], ts_pre[3], ts_pre[4], ts_pre[5], ts_pre[6], ts_pre[7],
        ts[0], ts[1], ts[2], ts[3], ts[4], ts[5], ts[6], ts[7],
        if ts.iter().any(|&b| b != 0) { "nonzero: the PTB wrote tile-state" } else { "all zero: no tile-state written" }
    );
    wrote
}

/// V3D-41 PTB / frame-completion witness — the discriminator downstream of the V3D-40 verdict.
///
/// P39 metal established: the coord shader RAN (v3d35 valid_instr=53, src14=28), CT0CA advanced BA→EA,
/// CTRUN latched then cleared, no MMU fault — yet tile-alloc pool AND tile-state stayed all-zero. The
/// CLE walked the whole list; the PTB wrote nothing. Two branches survive and the CT0CS/pool witnesses
/// cannot separate them:
///   (A) START_TILE_BINNING consumed clean but never brought up a PTB bin frame ("never started");
///   (B) a bin frame ran to completion but produced zero primitive-list bytes ("started, writes lost /
///       empty bin" — every screen-space primitive clipped/culled on-chip).
///
/// The frame-completion counter BFC decides it: read it before the GO and after idle. A `bfc_pre` is
/// captured by the caller immediately before the CT0QEA GO; this fn reads the post-idle counters and
/// diffs. BFC advanced by ≥1 ⇒ branch (B) (a frame completed — chase the on-chip clipper/VCM/PTB write
/// path); BFC unchanged ⇒ branch (A) (no frame — chase START_TILE_BINNING / PTB bring-up / the CT0
/// primitive feed). BPCA (the PTB write pointer) corroborates: advanced off `pool_base` ⇒ the PTB
/// emitted bytes (the CPU read path is then suspect, not the binner); still at `pool_base`/0 ⇒ it did
/// not. BPOA nonzero ⇒ overflow requested (a distinct pool-exhaustion failure, not this one). All reads
/// are of GPU status/counter registers — no shader-visible state is touched. PCS is reported RAW: its
/// bit layout past CTRUN is uncorroborated for V3D 4.x, so no bit names are fabricated (§5 law).
fn ptb_frame_witness(tag: &str, bfc_pre: u32, rfc_pre: u32, pool_base: u32) {
    let bfc = mmio_read(V3D_CORE0_BASE, V3D_CLE_BFC);
    let rfc = mmio_read(V3D_CORE0_BASE, V3D_CLE_RFC);
    let pcs = mmio_read(V3D_CORE0_BASE, V3D_CLE_PCS);
    let ct0lc = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0LC);
    let ct0pc = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0PC);
    let bpca = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCA);
    let bpcs = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPCS);
    let bpoa = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOA);
    let bpos = mmio_read(V3D_CORE0_BASE, V3D_PTB_BPOS);
    let bfc_delta = bfc.wrapping_sub(bfc_pre);
    let rfc_delta = rfc.wrapping_sub(rfc_pre);
    let bpca_advanced = bpca != 0 && bpca != pool_base;
    serial_println!(
        ":: V3D: [v3d41] {} frame counters — BFC {:#010x}->{:#010x} (Δ{}) RFC {:#010x}->{:#010x} (Δ{}) — {} ::",
        tag, bfc_pre, bfc, bfc_delta, rfc_pre, rfc, rfc_delta,
        if bfc_delta >= 1 {
            "BIN FRAME COMPLETED: START_TILE_BINNING brought up a PTB frame — branch (B) writes-lost/empty-bin (chase clipper/VCM/PTB write path, NOT bring-up)"
        } else {
            "NO BIN FRAME: the PTB frame never completed despite CT0CA reaching EA — branch (A) never-started (chase START_TILE_BINNING / PTB bring-up / CT0 primitive feed)"
        }
    );
    serial_println!(
        ":: V3D: [v3d41] {} CLE feed + PTB pointer — CT0LC={:#010x} CT0PC={:#010x} PCS={:#010x} (raw) | BPCA={:#010x} (pool base {:#010x}) BPCS={:#010x} BPOA={:#010x} BPOS={:#010x} — PTB write pointer {} ::",
        tag, ct0lc, ct0pc, pcs, bpca, pool_base, bpcs, bpoa, bpos,
        if bpca_advanced {
            "ADVANCED off the pool base: the PTB emitted primitive-list bytes (if the pool head still reads zero the CPU READ path is suspect, not the binner)"
        } else if bpoa != 0 {
            "at/near pool base but BPOA nonzero: the binner requested an OVERFLOW block (pool-exhaustion — a distinct failure)"
        } else {
            "still at the pool base/0: the PTB never advanced its write pointer — it produced no primitive-list bytes"
        }
    );
}

/// PI-V3D-21: the coordinate-shader EXECUTION witness — the read-only route. After V3D-17/18/20 proved
/// every CPU→GPU hand-off correct (clip state, 6-word STVPMV output, Mesa-packed) yet the tile-alloc
/// pool/tile-STATE stayed all-zero with the CL consumed clean and no fault, the single unproven link is
/// QPU execution itself: no side effect from the coord shader has EVER been observed. This route settles
/// it WITHOUT touching the shader — hardware performance counters. The decisive counter,
/// QPU_ACTIVE_CYCLES_VERTEX_COORD_USER (source 14), ticks ONLY while the QPU runs a vertex/coord USER
/// shader — exactly our coordinate shader on the bin queue — so a nonzero reading is unambiguous proof
/// the CS ran, and it cannot be confounded by CLE activity or the fragment shader (a different counter).
/// A TMU general-store witness (the alternative) was REJECTED: it perturbs the shader (new QPU words →
/// fabricated-constant risk, 3 convictions) and, worse, makes a null result ambiguous — a non-writing
/// store cannot distinguish "QPU never ran" from "store mis-encoded/unsupported in a bin-mode coord
/// shader", the precise question this arc must answer. The PCTR route is read-only w.r.t. the shader and
/// gives a clean yes/no. (Mesa/kernel do emit TMU general stores in vertex/coord shaders — SSBO/image/
/// transform-feedback via nir_to_vir.c ntq_emit_tmu_general — so it is not architecturally impossible;
/// it is merely the wrong instrument for a "did it execute at all" probe.)
///
/// Programming is the exact Linux `v3d_perfmon_start` idiom (v3d_perfmon.c, V3D 4.x path): pack the
/// source ids into the 7-bit S-fields of PCTR_0_SRC_0_3 (counters 0..3) and PCTR_0_SRC_4_7 (counters
/// 4,5), enable via EN=mask, then CLR=mask (reset to 0) and OVERFLOW=mask. Called just before the bin GO
/// so the counters span the bin. PI-V3D-33 adds counters 3,4,5 = the TMU-issue battery (tcache access,
/// cycles-waiting-TMU, tcache miss) on top of the V3D-21 execution trio (0,1,2).
fn pctr_setup_cs_witness() {
    // counter 0 = QPU active cycles in vertex/coord user shaders (the decisive execution witness),
    // counter 1 = QPU cycles issuing a valid instruction (corroboration), counter 2 = core cycle count
    // (block-was-clocked sanity). All three live in source group 0 (counters 0..3), packed S0/S1/S2.
    let channel = (PCTR_SRC_QPU_ACTIVE_CYCLES_VERTEX_COORD_USER & 0x7f)
        | ((PCTR_SRC_QPU_CYCLES_VALID_INSTR & 0x7f) << 8)
        | ((PCTR_SRC_CYCLE_COUNT & 0x7f) << 16)
        // PI-V3D-33: counter 3 (S3, bits 24..30) = TMU tcache accesses — the direct "did the TMU see the
        // general store?" witness, sharing this same source register.
        | ((PCTR_SRC_TMU_TCACHE_ACCESS & 0x7f) << 24);
    // PI-V3D-33: counters 4,5 in the next source group (SRC_4_7): S4 = QPU cycles waiting on the TMU,
    // S5 = TMU tcache misses. Together with counter 3 they form the TMU-engaged battery.
    let channel_4_7 =
        (PCTR_SRC_QPU_CYCLES_WAITING_TMU & 0x7f) | ((PCTR_SRC_TMU_TCACHE_MISS & 0x7f) << 8);
    let mask: u32 = 0b11_1111; // six counters enabled (0..5)
    // PI-V3D-37: arm in the exact Linux v3d_perfmon.c::v3d_perfmon_start order — STOP, program sources
    // while stopped, CLR+OVERFLOW while stopped, then EN LAST.
    //
    // PI-V3D-39 (Task B — the true mechanism, from the P36 multi-boot capture): V3D-37's reorder did NOT
    // cure the src16 "aliasing" — the SAME kernel, across boots, reads M4's valid_instr(src16) as 55 (the
    // true coord-shader count) on SOME boots and ~2,116,4xx on OTHERS, with cycle_count(src32) STABLE at
    // ~8,465,5xx every boot. So it is NOT an arming-ORDER defect (the ordered+barriered sequence is present
    // in the very capture that still flips) and NOT a field-packing defect (the three SRC shifts pack
    // distinct, correct 8-bit fields — S0..S3 at 0/8/16/24 in SRC_0_3, S4/S5 at 0/8 in SRC_4_7, each masked
    // 0x7f, matching v3d_regs.h V3D_PCTR_0_SRC_MASK; counter0=src14 and counter2=src32, written by the SAME
    // SRC_0_3 store, are ALWAYS sane). It is a NON-DETERMINISTIC SOURCE-LATCH: the garbage value decomposes
    // as cycle_count/4 + true_count (2116432 = 2116387 + ~45), and it strikes counter1(src16) and
    // counter4(src17) — the two QPU-PIPELINE sources — TOGETHER (src17 leaks the identical ~cycle/4 in
    // lockstep, flipping [v3d33] between SAW-NOTHING and a spurious "TMU ENGAGED"), while the non-pipeline
    // counters never leak. On the leaking boots those two counters ALSO count a free-running ~core/4 term
    // across the whole bin on top of their real event — the signature of a source-select that did not
    // cleanly latch before counting began.
    //
    // Software mitigation (the do-it-right cure available from ring 0): make the SRC-select stores fully
    // RETIRE before anything enables the counters — a bare dsb() orders the posted writes but does not prove
    // the block latched them, and posted MMIO to the V3D core can retire after the barrier. READ the two SRC
    // registers back (read-after-write forces the store to complete and the select to be observably latched)
    // between programming and CLR/EN. The verdict does NOT rely on this succeeding: [v3d21] keys RAN/NEVER-
    // RAN off counter0 (src14 QPU_ACTIVE_CYCLES_VERTEX_COORD_USER), the shader-scoped counter that reads sane
    // every boot; src16/src17 are corroboration only and are now labeled race-prone at the read site.
    //
    // Two witnesses (probe then M4) reuse slots 0..5 sequentially; the leading EN=0 guarantees the prior
    // arming is fully quiesced before this one reprograms the shared source-selects.
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, 0); // stop any prior counting before reprogramming
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_0_3, channel);
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_4_7, channel_4_7);
    dsb(); // order the source-select stores ahead of the read-back
    // PI-V3D-39: read the SRC selects back so the posted stores RETIRE and the selects are provably latched
    // before the counters are cleared/enabled — the mitigation for the non-deterministic src16/src17 latch.
    let _srclat = mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_0_3)
        | mmio_read(V3D_CORE0_BASE, V3D_V4_PCTR_0_SRC_4_7);
    dsb(); // source-selects latched before the counters are cleared/enabled
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_CLR, mask); // zero the counters WHILE stopped
    mmio_write(V3D_CORE0_BASE, V3D_PCTR_0_OVERFLOW, mask); // clear latched overflow flags
    dsb(); // clear lands before enable
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, mask); // enable LAST — counting starts from a clean zero
    dsb();
}

/// PI-V3D-21: read the three PCTR counters after the bin idled, disable them, and print the decisive
/// verdict line. counter 0 (QPU_ACTIVE_CYCLES_VERTEX_COORD_USER) nonzero ⇒ the coordinate shader's QPU
/// program executed. QEMU raspi4b has no V3D, so these reads return 0 → "NEVER RAN" there; metal decides.
fn pctr_read_cs_witness(tag: &str) {
    dsb();
    let coord = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0); // counter 0
    let vinstr = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 4); // counter 1
    let cycles = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 8); // counter 2
    // PI-V3D-33 TMU battery: counter 3 = tcache accesses, 4 = cycles waiting TMU, 5 = tcache misses.
    let tmu_acc = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 12); // counter 3
    let tmu_wait = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 16); // counter 4
    let tmu_miss = mmio_read(V3D_CORE0_BASE, V3D_PCTR_0_PCTR0 + 20); // counter 5
    mmio_write(V3D_CORE0_BASE, V3D_V4_PCTR_0_EN, 0); // stop counting
    dsb();
    // PI-V3D-39: the RAN/NEVER-RAN verdict keys off counter0 (src14 QPU_ACTIVE_CYCLES_VERTEX_COORD_USER) —
    // the shader-scoped counter that reads sane every boot. valid_instr(src16) is a RACE-PRONE corroboration
    // only: the P36 multi-boot capture proved it non-deterministically reads either the true count (~55) or
    // cycle_count/4+true on the leaking boots (see pctr_setup_cs_witness for the mechanism + mitigation).
    serial_println!(
        ":: V3D: [v3d21] {} CS-exec proof via PCTR — QPU_ACTIVE_CYCLES_VERTEX_COORD(src14)={} valid_instr(src16)={}{} cycle_count(src32)={} — SHADER {} (verdict=src14) ::",
        tag, coord, vinstr,
        if vinstr > cycles / 8 { " [race-leak: ~cyc/4, disregard]" } else { "" },
        cycles,
        if coord != 0 { "RAN" } else { "NEVER RAN" }
    );
    // PI-V3D-33/35: the TMU-issue witness. It only reflects a store when armed around the PROBE bin (tag
    // "v3d35 PROBE bin") — the probe's word[9] `mov tmuau` is the ONLY TMU op in the whole kernel (both
    // fragment shaders write the TLB, never the TMU; the real M4 coord shader CS_VS_WORDS is a pure
    // VPM-passthrough with no TMU op). Armed around the M4 bin (tag "M4 post-bin") these three are 0 BY
    // CONSTRUCTION and carry no information — V3D-35's finding. A nonzero here under the probe tag is the
    // first TMU-block activity UnaOS has ever recorded on this silicon.
    let tmu_engaged = tmu_acc != 0 || tmu_wait != 0 || tmu_miss != 0;
    serial_println!(
        ":: V3D: [v3d33] {} TMU-issue witness via PCTR — tcache_access(src24)={} cycles_waiting_tmu(src17)={} tcache_miss(src25)={} — TMU {} ({}) ::",
        tag, tmu_acc, tmu_wait, tmu_miss,
        if tmu_engaged { "ENGAGED" } else { "SAW-NOTHING" },
        if tmu_engaged {
            "store issued to the TMU — chase the drain/L2T/address, NOT the issue"
        } else {
            "general store never reached the TMU block — chase the issue path (shader/thread-end/waddr), NOT the drain"
        }
    );
}

/// PI-V3D-34: TMU/GMP block-state witness — the pre-arm for V3D-33's SAW-NOTHING branch.
///
/// V3D-33 proved the probe's word[9] genuinely names TMUAU(13) and that this general store is the FIRST
/// TMU op UnaOS has ever issued on this silicon (no other shader touches the TMU — both fragment shaders
/// write only the TLB). If the next boot's TMU-issue PCTR battery (tcache_access / cycles_waiting_tmu /
/// tcache_miss) reads all-zero (SAW-NOTHING), the defect may be TMU-block-wide (never enabled/configured)
/// rather than probe-specific. This witness puts the whole set of configuration/enable-state registers a
/// v42 TMU general store depends on on serial, read-only, each annotated MESA-EXPECTS vs OURS where the
/// expectation is derivable from the Linux v3d KMD / Mesa v42 contract. It changes NO state (pure reads).
///
/// The three axes the brief names:
///  (1) shader-record TMU config: the GL Shader State Record + Attribute Record carry NO TMU config word;
///      on v42 the TMU config is delivered in the UNIFORM stream (the 0xfffffffc config word u5 carries,
///      already witnessed by [v3d32]) and consumed by the TMUAU write — so there is nothing TMU-specific
///      to read out of the record here; the record path is exonerated by [v3d18]/[v3d25]/[v3d29].
///  (2) core registers gating TMU operation: MISCCFG.OVRTMUOUT (ver>=41 → KMD leaves it at reset, TMU
///      output type comes from the config word), the L2T control incl. TMUWCF (TMU write-combiner flush),
///      and the SLC TMU-cache clear fields (TVCCS/TDCCS) — the caches a TMU store's data traverses.
///  (3) GMP: a GMP write-violation drops a store SILENTLY with no MMU fault — the exact signature of the
///      store-accepted-but-never-lands wall with a clean fault latch. Linux never programs the GMP, so it
///      sits in reset (CFG.PROT_ENABLE=0 = allow-all); the read-back proves the actual latched state.
fn tmu_gmp_block_state_witness(tag: &str) {
    dsb();
    // (2) MISCCFG — the TMU-output-type gate. MESA-EXPECTS on ver42(>=41): untouched at reset value,
    // OVRTMUOUT=0 (KMD writes OVRTMUOUT only on ver<41); the TMU output type then comes from the config
    // word the TMUAU consumes. A set OVRTMUOUT here would mean something enabled it out of band.
    let misccfg = mmio_read(V3D_CORE0_BASE, V3D_CTL_MISCCFG);
    serial_println!(
        ":: V3D: [v3d34] {} MISCCFG={:#010x} — OVRTMUOUT(bit0)={} QRMAXCNT[3:1]={} — MESA-EXPECTS ver42>=41: KMD leaves MISCCFG untouched (OVRTMUOUT written only ver<41), TMU output type from the config word — OURS: UnaOS never writes MISCCFG ::",
        tag, misccfg,
        (misccfg & V3D_MISCCFG_OVRTMUOUT != 0) as u32,
        (misccfg & V3D_CTL_MISCCFG_QRMAXCNT_MASK) >> 1
    );

    // (2) L2TCACTL — the L2T cache control we already drive for the post-bin FLM=FLUSH drain. TMUWCF is
    // the TMU write-combiner flush bit; dump the live control word so the SAW-NOTHING boot shows whether
    // the TMU write combiner participates in our flush. MESA-EXPECTS: idle between ops (L2TFLS clear).
    let l2tcactl = mmio_read(V3D_CORE0_BASE, V3D_CTL_L2TCACTL);
    serial_println!(
        ":: V3D: [v3d34] {} L2TCACTL={:#010x} — L2TFLS(bit0,in-progress)={} TMUWCF(bit8,TMU-write-combiner-flush)={} — MESA-EXPECTS: idle between jobs (FLM=FLUSH driven per-drain by invalidate_gpu_caches); TMUWCF is NOT set by our flush ::",
        tag, l2tcactl,
        (l2tcactl & V3D_L2TCACTL_L2TFLS != 0) as u32,
        (l2tcactl & V3D_L2TCACTL_TMUWCF != 0) as u32
    );

    // (2) SLCACTL — slice caches a TMU store's data traverses (TVCCS=TMU-vertex-cache, TDCCS=TMU-data-
    // cache). We drive all-0xF invalidate per job; between ops the field reads back its idle state.
    let slcactl = mmio_read(V3D_CORE0_BASE, V3D_CTL_SLCACTL);
    serial_println!(
        ":: V3D: [v3d34] {} SLCACTL={:#010x} — TVCCS[27:24]={:#x} TDCCS[19:16]={:#x} UCC[11:8]={:#x} ICC[3:0]={:#x} — MESA-EXPECTS: TMU-vertex/TMU-data caches invalidated (0xF) per job by invalidate_gpu_caches; OURS drives SLCACTL_INVALIDATE_ALL ::",
        tag, slcactl,
        (slcactl >> 24) & 0xF, (slcactl >> 16) & 0xF, (slcactl >> 8) & 0xF, slcactl & 0xF
    );

    // (3) GMP — the prime silent-drop candidate. Linux never writes any GMP register; GMP sits in reset,
    // where CFG.PROT_ENABLE=0 = protection disabled = ALL accesses allowed (NOT default-deny). Read back
    // CFG + STATUS: if PROT_ENABLE reads 1, or STATUS.VIO/INVPROT is latched, THAT is the silent-drop the
    // MMU fault latch cannot see. Expected on a clean block: CFG.PROT_ENABLE=0 and STATUS.VIO=0.
    let gmp_cfg = mmio_read(V3D_CORE0_BASE, V3D_GMP_CFG);
    let gmp_status = mmio_read(V3D_CORE0_BASE, V3D_GMP_STATUS);
    let gmp_vio_addr = mmio_read(V3D_CORE0_BASE, V3D_GMP_VIO_ADDR);
    let gmp_vio_type = mmio_read(V3D_CORE0_BASE, V3D_GMP_VIO_TYPE);
    let gmp_table = mmio_read(V3D_CORE0_BASE, V3D_GMP_TABLE_ADDR);
    let gmp_valid = mmio_read(V3D_CORE0_BASE, V3D_GMP_VALID_LINES);
    let prot_on = gmp_cfg & V3D_GMP_CFG_PROT_ENABLE != 0;
    let vio = gmp_status & V3D_GMP_STATUS_VIO != 0;
    let invprot = gmp_status & V3D_GMP_STATUS_INVPROT != 0;
    serial_println!(
        ":: V3D: [v3d34] {} GMP_CFG={:#010x} — PROT_ENABLE(bit0)={} STOP_REQ(bit1)={} LBURSTEN(bit3)={} — MESA-EXPECTS: KMD never writes GMP → reset state, PROT_ENABLE=0 = ALLOW-ALL (not default-deny) ::",
        tag, gmp_cfg,
        prot_on as u32,
        (gmp_cfg & V3D_GMP_CFG_STOP_REQ != 0) as u32,
        (gmp_cfg & V3D_GMP_CFG_LBURSTEN != 0) as u32
    );
    serial_println!(
        ":: V3D: [v3d34] {} GMP_STATUS={:#010x} — VIO(bit0)={} INVPROT(bit1)={} CNTOVF(bit2)={} RD_ACTIVE(bit4)={} WR_ACTIVE(bit5)={} GMPRST(bit31)={} VIO_ADDR={:#010x} VIO_TYPE={:#010x} — MESA-EXPECTS: VIO=0, no violation latched ::",
        tag, gmp_status,
        vio as u32, invprot as u32,
        (gmp_status & V3D_GMP_STATUS_CNTOVF != 0) as u32,
        (gmp_status & V3D_GMP_STATUS_RD_ACTIVE != 0) as u32,
        (gmp_status & V3D_GMP_STATUS_WR_ACTIVE != 0) as u32,
        (gmp_status & V3D_GMP_STATUS_GMPRST != 0) as u32,
        gmp_vio_addr, gmp_vio_type
    );
    serial_println!(
        ":: V3D: [v3d34] {} GMP_TABLE_ADDR={:#010x} VALID_LINES={:#010x} — protection-table base + loaded-line count (both 0 when unconfigured; irrelevant while PROT_ENABLE=0) ::",
        tag, gmp_table, gmp_valid
    );

    // Decisive one-liner: does the GMP explain a silent store drop? Only if protection is ON *and* a
    // write violation latched. With PROT_ENABLE=0 the GMP is exonerated as the SAW-NOTHING cause.
    let gmp_could_drop = prot_on && (vio || invprot);
    serial_println!(
        ":: V3D: [v3d34] {} GMP verdict — protection {} → GMP {} the silent-store-drop cause (PROT_ENABLE={}, VIO={}, INVPROT={}); (1) shader record carries NO TMU config word — TMU config is uniform-delivered (config 0xfffffffc, see [v3d32]) ::",
        tag,
        if prot_on { "ENABLED" } else { "DISABLED (allow-all)" },
        if gmp_could_drop { "IS A CANDIDATE for" } else { "is EXONERATED as" },
        prot_on as u32, vio as u32, invprot as u32
    );
}

/// PI-V3D-18 witness (V3D-16-mandated post-bin CS/VPM audit). Two records the next metal boot reads
/// to confirm what the hardware actually consumed for the coordinate (bin) shader:
///
///  (1) the 52 shader-state bytes at OFF_SHADREC — the 36-byte GL Shader State Record + the 16-byte
///      GL Shader State Attribute Record — the exact bytes the CLE's GL_SHADER_STATE fetch handed the
///      PTB. (The coordinate shader's VPM OUTPUT is on-chip and NOT CPU/DRAM-readable — there is no
///      V3D_VPM CPU window for per-QPU shader output on 4.x; Mesa reads VPM back only via LDVPM inside a
///      shader, never from the CPU — so the record bytes + the tile-alloc pool/tile-STATE are the only
///      readable witnesses of what the hardware did, per the V3D-16 fallback ask. PI-V3D-20: a TMU-store
///      readback debug variant was considered and SKIPPED — it is not trivial and would build a debug
///      subsystem the brief forbids; the pool/tile-STATE going non-zero remains the decisive verdict.)
///
///  (2) the CONTRACTED coordinate-shader VPM output vs. what CS_VS_WORDS actually emits, per vertex.
///      Mesa `v3d_nir_setup_vpm_layout_vs` (src/broadcom/compiler/v3d_nir_lower_io.c): for is_coord
///      the output layout is SIX words — pos[0..3] = clip Xc,Yc,Zc,Wc at offsets 0..3, THEN the two
///      screen-space words the PTB bins from at offsets 4,5: Xs = f2i32(floor(Xc·vp_scale_x·(1/Wc))),
///      Ys = f2i32(floor(Yc·vp_scale_y·(1/Wc))) (floor path is the ver==42 branch in
///      `v3d_nir_emit_ff_vpm_outputs`; f2i gives INTEGER .8 fixed-point, CENTRE-RELATIVE — the centre
///      is added by the fixed-function VIEWPORT_OFFSET). vp_scale = viewport.scale·clipper_xy_granularity
///      = 32 · 256 = 8192 (v3d_uniforms.c QUNIFORM_VIEWPORT_X_SCALE; granularity 256.0f for ver 42,
///      v3d_device_info.c). PI-V3D-20 stores all six output words via STVPMV at explicit VPM offsets
///      0..5 (screen Xs/Ys = fmul·8192 → ffloor → ftoiz, W=1 so no 1/Wc), Mesa-packed by
///      scripts/pi-v3d20-qpu-gen.c — correcting the V3D-9/19 mov-vpm/vpmsetup streamed path, which is not
///      the v42 output mechanism and wrote nowhere the PTB reads. This line prints the expected Xs/Ys per
///      vertex so the next metal boot can check the PTB's binned coords
///      against them (the CS VPM output is on-chip; the tile-alloc pool / tile-STATE going non-zero is
///      the real verdict).
fn cs_vpm_output_witness(tag: &str) {
    // (1) shader-state record + attribute record bytes (36 + 16 = 52).
    cache::clean_invalidate_range(arena_phys() + OFF_SHADREC, 52);
    dump_shadrec_bytes(tag, OFF_SHADREC, 52);
    // PI-V3D-25 DISCRIMINATOR — decode the four VPM segment-size nibbles the CLE just handed the PTB
    // (record bytes 4/5/6/7 low nibble = coord-out/coord-in/vertex-out/vertex-in). Mesa's
    // `v3d_vs_set_prog_data` folds the INPUT into the output and zeroes it (vir.c:918-920), so the
    // hardware-correct values are out=1, in=0. A prior in=1 built a spurious separate input block that
    // mis-aligned the VCD attribute DMA vs the shader's ldvpmv_in reads (the coord shader read zeros →
    // degenerate primitive → empty bin, the V3D-24 attribute-fetch hypothesis). This line proves the
    // corrected in=0 landed; the decisive verdict remains bin_pool_witness going non-zero on this boot.
    let seg_co = arena_byte(OFF_SHADREC + 4) & 0x0F;
    let seg_ci = arena_byte(OFF_SHADREC + 5) & 0x0F;
    let seg_vo = arena_byte(OFF_SHADREC + 6) & 0x0F;
    let seg_vi = arena_byte(OFF_SHADREC + 7) & 0x0F;
    serial_println!(
        ":: V3D: [v3d25] {} VPM segment sizes coord(out={} in={}) vertex(out={} in={}) — Mesa contract out=1 in=0 (vir.c v3d_vs_set_prog_data folds input→output); in=0 aligns VCD attribute DMA with the shader ldvpmv_in reads: {} ::",
        tag, seg_co, seg_ci, seg_vo, seg_vi,
        if seg_ci == 0 && seg_vi == 0 { "MATCH (attribute-fetch fix aboard)" } else { "MISMATCH (input block still spurious)" }
    );
    // (2) contracted 6-word CS output vs. our 4-word passthrough, per vertex. Center-relative screen
    // coords (Mesa's shader math; VIEWPORT_OFFSET adds the +32,+32 centre in fixed function).
    let vp_scale: f64 = ((TARGET_W as f64) / 2.0) * 256.0; // 8192.0
    for (i, v) in TRI_VERTS.iter().enumerate() {
        let (xc, yc, zc, wc) = (v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64);
        let rcp_wc = if wc != 0.0 { 1.0 / wc } else { 0.0 };
        let xs = floor_i32(xc * vp_scale * rcp_wc);
        let ys = floor_i32(yc * vp_scale * rcp_wc);
        serial_println!(
            ":: V3D: [v3d20] {} CS-out v{} — CONTRACT[6] Xc={} Yc={} Zc={} Wc={} | Xs={} Ys={} (centre-rel .8fp) — CS_VS_WORDS now STORES all 6 via STVPMV @explicit out-offsets 0..5 (was mov-vpm/vpmsetup: wrong mechanism for v42, wrote nowhere); PTB should bin these ::",
            tag, i,
            (xc * 1000.0) as i32, (yc * 1000.0) as i32, (zc * 1000.0) as i32, (wc * 1000.0) as i32,
            xs, ys
        );
    }
    // PI-V3D-24 DISCRIMINATOR — the screen-coordinate encoding is byte-faithful to Mesa; PROVE the
    // transformed triangle lands ON the tile grid. The shader emits CENTRE-RELATIVE screen coords
    // (Xs/Ys above); the hardware PTB composes them with VIEWPORT_OFFSET's centre (fine .8 = 8192 →
    // 32.0 px, exactly what our CL emits) to get the ABSOLUTE framebuffer position it bins from.
    // Mesa contract confirmed verbatim this arc against src/broadcom/compiler/v3d_nir_lower_io.c
    // `v3d_nir_emit_ff_vpm_outputs` (scale-only, no in-shader offset, `f2i32(ffloor(pos·scale·1/Wc))`,
    // .8 fixed-point) + genxml v41+ VIEWPORT_OFFSET (s14.8 centre @0, coarse @22) / CLIPPER_XY_SCALING
    // (half-extent·256 as f32). If every vertex below is INSIDE the 0..W/0..H clip window, the empty
    // bin is NOT an encoding/geometry defect — the surviving candidate is the VPM INPUT (attribute
    // fetch) collapsing the triangle to a degenerate point (see v3d.md §8).
    let centre_fp: f64 = ((TARGET_W as f64) / 2.0) * 256.0; // VIEWPORT_OFFSET fine = 8192 (.8) = 32.0 px
    for (i, v) in TRI_VERTS.iter().enumerate() {
        let (xc, yc, wc) = (v[0] as f64, v[1] as f64, v[3] as f64);
        let rcp_wc = if wc != 0.0 { 1.0 / wc } else { 0.0 };
        let xs = floor_i32(xc * vp_scale * rcp_wc) as f64;
        let ys = floor_i32(yc * vp_scale * rcp_wc) as f64;
        // absolute framebuffer position in pixels = (centre-rel .8 + centre .8) / 256
        let px_x = (xs + centre_fp) / 256.0;
        let px_y = (ys + centre_fp) / 256.0;
        let inside = px_x >= 0.0 && px_x <= TARGET_W as f64 && px_y >= 0.0 && px_y <= TARGET_H as f64;
        serial_println!(
            ":: V3D: [v3d24] {} abs v{} — screen px ({}.{:02},{}.{:02}) after +VIEWPORT_OFFSET(32,32) — clip window 0..{}×0..{}: {} — encoding Mesa-verified (v3d_nir_lower_io.c + genxml v41+), so on-grid ⇒ empty bin is NOT the encoding ::",
            tag, i,
            px_x as i32, (((px_x - px_x as i64 as f64) * 100.0) as i32).abs(),
            px_y as i32, (((px_y - px_y as i64 as f64) * 100.0) as i32).abs(),
            TARGET_W, TARGET_H,
            if inside { "INSIDE (should bin to a tile)" } else { "OUTSIDE (off every tile)" }
        );
    }
}

/// PI-V3D-29 — ATTRIBUTE-RECORD AUDIT WITNESS (pre-arm for the loaded-zeros branch).
///
/// V3D-28's fixed probe decodes the next boot as real-coords / loaded-zeros / landed-elsewhere /
/// never-issued. If it reads LOADED-ZEROS (the VCD never DMA'd the vertex buffer into VPM), the named
/// next step is: audit the attribute record's base/stride/enable/count fields — every knob that gates
/// the VCD's per-vertex attribute fetch — against what Mesa v42 emits for THIS draw. This witness
/// pre-arms that step: it decodes the GL Shader State Attribute Record (16 B @ OFF_SHADREC+36) and the
/// shader-record fields that gate fetch (VPM segment sizes + the GL_SHADER_STATE attribute-array count)
/// back out of the arena EXACTLY AS WRITTEN, cross-checks each against the Mesa-v42 contract value, and
/// prints PASS/DIVERGE per field with both sides. It also re-dumps the full 52-byte shader-state record
/// (record + attr) as raw hex, one line per 16 B, so a human can diff it against a Mesa-packed record
/// offline. Instrumentation only — no field is rewritten, no CL/kick path changes.
///
/// Mesa contract of record (facts-only; sourced verbatim from this arc's build_shader_record(), itself
/// Mesa-verbatim per v3d_packet.xml "GL Shader State Attribute Record" (max_ver=42) + v3dX(draw_vbo),
/// cross-checked against scripts/pi-v3d26-mesa-compile.out.txt: vpm_input_size=0 vpm_output_size=1
/// vcm_cache_size=4 separate_segments=0). For a trivial 1-attribute vec4-f32 solid draw:
///   attr.Address = OFF_VTXDATA PA (must point at the vertex buffer)   attr.Vec size = 3 (4 comps)
///   attr.Type = 2 (Attribute float)   CS values read = 4   VS values read = 4   Stride = 16
///   Maximum Index = 0xFFFF   coord/vertex INPUT VPM segment = 0 (folded)   OUTPUT segment = 1
///   GL_SHADER_STATE attribute arrays = 1
/// QEMU raspi4b models no V3D, so nothing here can DIVERGE in QEMU (the arena bytes are exactly what the
/// CPU just wrote); the audit is the metal-boot verdict tool for the loaded-zeros branch.
fn attr_record_audit_witness(tag: &str, num_attrs: u32) {
    // The record + attr were cache-clean-invalidated at the top of cs_vpm_output_witness (its caller
    // site precedes this one); re-sync defensively so the decode reads DRAM, not a stale CPU line.
    cache::clean_invalidate_range(arena_phys() + OFF_SHADREC, 52);

    const ATTR: usize = OFF_SHADREC + 36; // attribute record base (immediately after the 36-B record)
    // ── Decode the GL Shader State Attribute Record exactly as written. ──
    let addr = arena_u32(ATTR);
    let b4 = arena_byte(ATTR + 4);
    let vec_size = b4 & 0x3; // @32(2): 4 comps encoded as (4-1)=3
    let attr_type = (b4 >> 2) & 0x7; // @34(3): Attribute float = 2
    let signed_int = (b4 >> 5) & 0x1; // @37(1)
    let norm_int = (b4 >> 6) & 0x1; // @38(1)
    let b5 = arena_byte(ATTR + 5);
    let cs_nvals = b5 & 0xF; // @40(4): values read by Coordinate shader
    let vs_nvals = (b5 >> 4) & 0xF; // @44(4): values read by Vertex shader
    let instance_div = (arena_u32(ATTR + 4) >> 16) & 0xFFFF; // @48(16): instance divisor
    let stride = arena_u32(ATTR + 8); // @64(32)
    let max_index = arena_u32(ATTR + 12); // @96(32)
    // ── Shader-record fields that gate the fetch (VPM segment sizes; low nibble of bytes 4..7). ──
    let coord_out = arena_byte(OFF_SHADREC + 4) & 0xF;
    let coord_in = arena_byte(OFF_SHADREC + 5) & 0xF;
    let vertex_out = arena_byte(OFF_SHADREC + 6) & 0xF;
    let vertex_in = arena_byte(OFF_SHADREC + 7) & 0xF;

    // ── Mesa v42 contract for this draw. ──
    let exp_addr = (arena_phys() + OFF_VTXDATA) as u32;
    let addr_is_vtx = addr == exp_addr;
    serial_println!(
        ":: V3D: [v3d29] {} ATTR AUDIT — GL Shader State Attribute Record @arena+{:#x} (16 B), vs Mesa v42 ::",
        tag, ATTR
    );
    // Field-by-field PASS/DIVERGE, both values, so a metal boot reads the verdict directly.
    macro_rules! audit {
        ($name:expr, $got:expr, $exp:expr, $note:expr) => {
            serial_println!(
                ":: V3D: [v3d29]   {:<22} got={:#x} exp={:#x} {} — {} ::",
                $name, $got as u64, $exp as u64,
                if ($got as u64) == ($exp as u64) { "PASS" } else { "DIVERGE" },
                $note
            );
        };
    }
    // Address is the fetch BASE — the single most load-bearing field for the loaded-zeros branch: if the
    // VCD's attribute base does not point at the vertex buffer, it DMAs garbage/zeros into VPM.
    audit!("attr.Address", addr, exp_addr,
        if addr_is_vtx { "points at vertex buffer (OFF_VTXDATA)" } else { "does NOT point at OFF_VTXDATA — fetch base wrong" });
    audit!("attr.Vec size", vec_size, 3u32, "4 components (encoded 4-1)");
    audit!("attr.Type", attr_type, 2u32, "Attribute float (v42)");
    audit!("attr.Signed int", signed_int, 0u32, "float attr: unsigned");
    audit!("attr.Normalized int", norm_int, 0u32, "float attr: not normalized");
    // CS/VS values-read are the per-vertex component COUNTS the VCD fetches for each shader stage; a 0
    // here is the textbook loaded-zeros cause (VCD DMAs nothing → VPM stays zero → degenerate primitive).
    audit!("attr.CS values read", cs_nvals, 4u32, "Coordinate shader reads vec4 → 4 (0 ⇒ loaded-zeros)");
    audit!("attr.VS values read", vs_nvals, 4u32, "Vertex shader reads vec4 → 4 (0 ⇒ loaded-zeros)");
    audit!("attr.Instance Divisor", instance_div, 0u32, "non-instanced draw");
    audit!("attr.Stride", stride, 16u32, "16 B per vertex (vec4 f32)");
    audit!("attr.Maximum Index", max_index, 0xFFFFu32, "trivial draw ceiling");
    // Shader-record VPM segment sizes gate WHERE the VCD DMA lands vs where the shader ldvpmv_in reads
    // (the V3D-24/25 alignment story): input folded to 0, output 1 sector.
    audit!("rec.CS out VPM seg", coord_out, 1u32, "coord output = 1 sector (6 words)");
    audit!("rec.CS in VPM seg", coord_in, 0u32, "Mesa folds input→output → 0");
    audit!("rec.VS out VPM seg", vertex_out, 1u32, "vertex output = 1 sector");
    audit!("rec.VS in VPM seg", vertex_in, 0u32, "Mesa folds input→output → 0");
    // The GL_SHADER_STATE attribute-array count is the effective enable mask — how many attribute records
    // the VCD walks. 0 would mean the VCD fetches NO attributes at all (a distinct loaded-zeros path).
    audit!("gl_shader.num_attrs", num_attrs, 1u32, "one enabled attribute array (VCD walk count)");

    // ── Raw shader-state record bytes for offline diff against a Mesa-packed record (record + attr). ──
    serial_println!(
        ":: V3D: [v3d29] {} raw shader-state record — 52 B @arena+{:#x} (36 B record + 16 B attr), one line/16 B ::",
        tag, OFF_SHADREC
    );
    let arena = &raw const V3D_ARENA;
    let mut i = 0usize;
    while i < 52 {
        let mut line = [0u8; 16];
        let mut c = 0;
        while c < 16 && i + c < 52 {
            line[c] = unsafe { (*arena).bytes[OFF_SHADREC + i + c] };
            c += 1;
        }
        serial_println!(
            "::   [v3d29]   +{:#05x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            i, line[0], line[1], line[2], line[3], line[4], line[5], line[6], line[7],
            line[8], line[9], line[10], line[11], line[12], line[13], line[14], line[15]
        );
        i += 16;
    }
}

/// floor(x) → i32 without libm (kernel no_std). `as i64` truncates toward zero; adjust down for
/// negatives with a fractional part to get a true floor.
#[inline]
fn floor_i32(x: f64) -> i32 {
    let t = x as i64;
    let f = if x < 0.0 && (t as f64) != x { t - 1 } else { t };
    f as i32
}

/// Hex-dump `n` arena bytes at `off` under the [v3d18] tag (the shader-state record witness; the CL
/// dumper's tag is fixed to [v3d15], so this dedicated copy keeps the arc tag correct).
fn dump_shadrec_bytes(tag: &str, off: usize, n: usize) {
    let arena = &raw const V3D_ARENA;
    serial_println!(
        ":: V3D: [v3d18] {} shader-state record — {} bytes @ arena+{:#x} (36 B record + 16 B attr) ::",
        tag, n, off
    );
    let mut i = 0;
    while i < n {
        let mut line = [0u8; 16];
        let mut c = 0;
        while c < 16 && i + c < n {
            line[c] = unsafe { (*arena).bytes[off + i + c] };
            c += 1;
        }
        serial_println!(
            "::   [v3d18]   +{:#05x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            i, line[0], line[1], line[2], line[3], line[4], line[5], line[6], line[7],
            line[8], line[9], line[10], line[11], line[12], line[13], line[14], line[15]
        );
        i += 16;
    }
}

/// PI-V3D-15 fault witness (brief lead #1). The M4 bin clue reported the MMU fault BITS
/// (MMU_fault=0x100000 = PT_INVALID) but never WHERE. Read-only decode (does NOT clear): report the
/// violating AXI client (VIO_ID), the true faulting VA (VIO_ADDR un-shifted via DEBUG_INFO va_width),
/// the ILLEGAL_ADDR trap slot, and — the discriminator — whether that VA lies INSIDE the identity-
/// mapped arena. Inside-arena = not a confinement escape (a CL/shader address or a legally-idle bin);
/// outside-arena = the binner walked off the mapped region, i.e. a mis-encoded CL address field (the
/// PI-V3D-10 boot-P6 class). Reads-only; QEMU-safe (CTL reads 0/absent → "no fault latched").
fn bin_fault_witness(tag: &str) {
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    let vio_addr = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
    let vio_id = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ID);
    let illegal = mmio_read(V3D_HUB_BASE, V3D_MMU_ILLEGAL_ADDR);
    let dbg = mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO);
    let (client, va) = vio_decode(vio_id, vio_addr);
    let base = arena_phys() as u64;
    let top = base + ARENA_BYTES as u64;
    let locus = if fault == 0 {
        "no fault latched"
    } else if va >= base && va < top {
        "faulting VA INSIDE arena — confinement-legal (CL/shader address or legally-idle bin), NOT an out-of-arena walk-off"
    } else {
        "faulting VA OUTSIDE arena — the binner walked off the mapped region (mis-encoded CL address field: PI-V3D-10 class)"
    };
    serial_println!(
        ":: V3D: [v3d15] {} MMU fault decode — CTL={:#010x} (PT_INVALID={} WRITE_VIOLATION={} CAP_EXCEEDED={}) client={} VIO_ADDR={:#010x} VIO_ID={:#010x} ILLEGAL_ADDR={:#010x} DEBUG={:#010x} -> VA={:#012x} arena=[{:#012x},{:#012x}) — {} ::",
        tag, ctl,
        (fault & V3D_MMU_CTL_PT_INVALID != 0) as u32,
        (fault & V3D_MMU_CTL_WRITE_VIOLATION != 0) as u32,
        (fault & V3D_MMU_CTL_CAP_EXCEEDED != 0) as u32,
        client, vio_addr, vio_id, illegal, dbg, va, base, top, locus
    );
}

/// PI-V3D-15 CL byte-dump witness (brief lead #2). Hex-dump the emitted control-list bytes (bounded to
/// `cap`) so the exact packet stream the binner parses is on the wire — a mis-sized packet that shifts
/// a following opcode byte into an address field (the PI-V3D-10 boot-P6 GL_SHADER_STATE fault) is
/// visible here as the wrong bytes at the wrong offset when read against Mesa's emit order. Reads the
/// arena bytes the CPU just wrote (pre-kick); 16 bytes per line, tail bytes past the count are padding.
fn dump_cl_bytes(tag: &str, off: usize, len: usize, cap: usize) {
    let arena = &raw const V3D_ARENA;
    let n = len.min(cap);
    serial_println!(
        ":: V3D: [v3d15] {} CL byte stream — {} of {} bytes @ arena+{:#x} ::",
        tag, n, len, off
    );
    let mut i = 0;
    while i < n {
        let mut line = [0u8; 16];
        let mut c = 0;
        while c < 16 && i + c < n {
            line[c] = unsafe { (*arena).bytes[off + i + c] };
            c += 1;
        }
        serial_println!(
            "::   [v3d15]   +{:#05x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            i, line[0], line[1], line[2], line[3], line[4], line[5], line[6], line[7],
            line[8], line[9], line[10], line[11], line[12], line[13], line[14], line[15]
        );
        i += 16;
    }
}

/// [v3d36] Decode a BINNING control list packet-by-packet to serial — opcode + name + key fields, one
/// line per packet — so the probe bin CL and the working M4 bin CL can be diffed on the capture.
///
/// V3D-36 root fact (P34 capture, cu.usbmodem143302.log): the probe bin's coord shader reads
/// valid_instr=0 / cycle_count=508 — SHADER NEVER RAN — while the SAME boot's M4 bin reads
/// valid_instr=55 SHADER RAN. The brief hypothesised the probe was a stripped-down bin whose CL lacked
/// a dispatch-gating packet (draw/clip/state). This witness settles that hypothesis FROM THE LOG: both
/// CLs are emitted by the identical `build_bin_cl_generic(cl_off, shadrec_off, num_attrs)`, so this
/// decode is expected to show them byte-for-byte identical EXCEPT the 27-bit GL_SHADER_STATE record
/// pointer (probe record vs M4 record). If they match packet-for-packet, the CL is EXONERATED and the
/// only structural difference gating coord-shader dispatch lives in the shader-state record the pointer
/// selects (threadability / code / uniforms) — not in the control list.
///
/// Field bit offsets follow the build path: `Pkt::f(xml_start, ..)` shifts by +8 (the opcode byte), so
/// a field documented at XML bit `xml_start` sits at absolute packet bit `xml_start + 8`.
fn decode_cl_packets(tag: &str, off: usize, len: usize) {
    serial_println!(
        ":: V3D: [v3d36] {} bin CL — packet decode ({} bytes @ arena+{:#x}) ::",
        tag, len, off
    );
    // Read `width` bits of the field at XML bit `xml_start` out of the packet whose first byte is arena
    // byte `p` (absolute packet bit = xml_start + 8 for the opcode-shift).
    let getb = |p: usize, xml_start: usize, width: usize| -> u64 {
        let mut abit = xml_start + 8;
        let mut w = width;
        let mut got = 0usize;
        let mut v: u64 = 0;
        while w > 0 {
            let byte = arena_byte(p + abit / 8) as u64;
            let o = abit % 8;
            let take = core::cmp::min(8 - o, w);
            let mask = (1u64 << take) - 1;
            v |= ((byte >> o) & mask) << got;
            abit += take;
            got += take;
            w -= take;
        }
        v
    };
    let mut i = 0usize;
    let mut idx = 0u32;
    while i < len {
        let p = off + i;
        let op = arena_byte(p);
        // Length table for the opcodes build_bin_cl_generic emits (byte length incl. opcode).
        let plen: usize = match op {
            P_FLUSH_VCD_CACHE | P_START_TILE_BINNING | P_FLUSH => 1,
            P_NUMBER_OF_LAYERS | P_VCM_CACHE_SIZE => 2,
            P_CFG_BITS => 4,
            P_OCCLUSION_QUERY_COUNTER | P_GL_SHADER_STATE => 5,
            P_TILE_BINNING_MODE_CFG | P_CLIP_WINDOW | P_VIEWPORT_OFFSET | P_CLIPPER_XY_SCALING
            | P_CLIPPER_Z_SCALE_AND_OFFSET => 9,
            P_VERTEX_ARRAY_PRIMS => 10,
            _ => 1, // unknown → advance one byte; the idx guard below bounds the walk
        };
        match op {
            P_TILE_BINNING_MODE_CFG => serial_println!(
                "::   [v3d36] [{:2}] op={:3} TILE_BINNING_MODE_CFG      w={} h={} (px) ::",
                idx, op, getb(p, 32, 16) + 1, getb(p, 48, 16) + 1
            ),
            P_CFG_BITS => serial_println!(
                "::   [v3d36] [{:2}] op={:3} CFG_BITS                   fwd={} rev={} ::",
                idx, op, getb(p, 0, 1), getb(p, 1, 1)
            ),
            P_CLIP_WINDOW => serial_println!(
                "::   [v3d36] [{:2}] op={:3} CLIP_WINDOW                l={} b={} w={} h={} ::",
                idx, op, getb(p, 0, 16), getb(p, 16, 16), getb(p, 32, 16), getb(p, 48, 16)
            ),
            P_VCM_CACHE_SIZE => serial_println!(
                "::   [v3d36] [{:2}] op={:3} VCM_CACHE_SIZE             bin={} render={} ::",
                idx, op, getb(p, 0, 4), getb(p, 4, 4)
            ),
            P_GL_SHADER_STATE => serial_println!(
                "::   [v3d36] [{:2}] op={:3} GL_SHADER_STATE            num_attrs={} record={:#010x} ::",
                idx, op, getb(p, 0, 5), getb(p, 5, 27) << 5
            ),
            P_VERTEX_ARRAY_PRIMS => serial_println!(
                "::   [v3d36] [{:2}] op={:3} VERTEX_ARRAY_PRIMS         mode={} count={} first={} ::",
                idx, op, getb(p, 0, 8), getb(p, 8, 32), getb(p, 40, 32)
            ),
            _ => {
                let name = match op {
                    P_NUMBER_OF_LAYERS => "NUMBER_OF_LAYERS",
                    P_FLUSH_VCD_CACHE => "FLUSH_VCD_CACHE",
                    P_OCCLUSION_QUERY_COUNTER => "OCCLUSION_QUERY_COUNTER",
                    P_START_TILE_BINNING => "START_TILE_BINNING",
                    P_VIEWPORT_OFFSET => "VIEWPORT_OFFSET",
                    P_CLIPPER_XY_SCALING => "CLIPPER_XY_SCALING",
                    P_CLIPPER_Z_SCALE_AND_OFFSET => "CLIPPER_Z_SCALE_AND_OFFSET",
                    P_FLUSH => "FLUSH (bin terminator)",
                    _ => "UNKNOWN",
                };
                serial_println!(
                    "::   [v3d36] [{:2}] op={:3} {} (len {}) ::",
                    idx, op, name, plen
                );
            }
        }
        if op == P_FLUSH {
            break; // the bin terminator — end of the binning list
        }
        i += plen;
        idx += 1;
        if idx > 40 {
            serial_println!("::   [v3d36] [..] decode guard hit (>40 packets) — stopping ::");
            break;
        }
    }
}

// ─── PI-V3D-57: the bin-CL packing witness ─────────────────────────────────────────────────────────
//
// V3D-57 brief: "why does the binner not start?" — settle the CL half of that question. The AUDIT was
// done off-metal and mechanically, against Mesa's own `src/broadcom/cle/v3d_packet.xml` (the file the
// packers are generated from): every `Pkt::new` in this driver had its byte length and every field's
// (start, width) checked against a v42-applicable XML variant, and the load-bearing facts — code 120 is
// 9 bytes, width/height are pixels-MINUS-ONE at bits 32/48 (the v41+ form), the block enums are 1/128B
// and 0/64B — were confirmed there. See v3d.md §31 for the table.
//
// WHAT THIS WITNESS IS, PRECISELY. It is NOT an independent re-derivation of Mesa's encoding: the
// `mesa=` column is written from the same audited constants the emitter uses, so a matching line proves
// the bytes IN ARENA MEMORY carry the value the builder intended at the offset the audit blessed —
// a PACKING-CONSISTENCY check (builder vs the bytes the CLE will actually fetch), not a second opinion
// on Mesa. That is still the thing no off-metal check can give: it reads the published list back out of
// the arena. Read the `mesa=` column as "the audited expected encoding", and a DIVERGE line as "the
// bytes in memory are not what this driver's own audited packing says they should be".
//
// Mesa authorities for the expected column (all MIT, attributed):
//   · prologue + order: `v3dX(start_binning)` (gallium v3dx_draw.c) and `v3dX(job_emit_binning_prolog)`
//     (v3dv v3dvx_cmd_buffer.c) — NUMBER_OF_LAYERS → TILE_BINNING_MODE_CFG → FLUSH_VCD_CACHE →
//     [OCCLUSION_QUERY_COUNTER, gallium only] → START_TILE_BINNING → …draws…
//   · terminator: `v3dX(bcl_epilogue)` (v3dx_job.c) / `v3dX(job_emit_binning_flush)` (v3dvx_cmd_buffer.c)
//     — a bare FLUSH (code 4). NOT FLUSH_ALL_STATE (5) ("you would need FLUSH_ALL for that, but the HW
//     hasn't been validated"), and NO INCREMENT_SEMAPHORE (7): the semaphore pair is a VC4-era idiom the
//     v3d 4.x emitters never use.
//   · block sizes: v3d_limits.h — INITIAL=128 (enum 1), OVERFLOW=64 (enum 0), enum = size >> 7.
//   · tile memory: `v3d_tile_alloc_sizes` (v3d_util.c) — tile_state = tiles·256, tile_alloc =
//     align(tiles·128, 4096) + 8192 (+ a draw-scaled continuation pool).
//
// Volume: ~60–100 lines per boot, one shot per CL (PROBE, each bisect rung, M4) — deliberate, and in the
// V3D-46/54/56 one-shot-witness precedent. `V3D57_CL_AUDIT` is a plain const: ONE flip to `false`
// silences the whole battery once the metal capture has been taken.
const V3D57_CL_AUDIT: bool = true;

/// One field line of the [v3d57] witness: name, the value read back out of the published bytes, the
/// audited expected encoding, verdict. Returns 1 when the field DIVERGES, so the caller can total the
/// divergences for the verdict line. (The expected column comes from this driver's audited constants —
/// see the packing-consistency note above; it is not an independent re-derivation of Mesa.)
#[must_use]
fn v3d57_field(name: &str, ours: u64, want: u64) -> u32 {
    let bad = ours != want;
    serial_println!(
        "::     [v3d57]   {:<34} ours={:<12} mesa={:<12} {} ::",
        name, ours, want,
        if bad { "DIVERGE" } else { "OK" }
    );
    bad as u32
}

/// [v3d57] Dump a BINNING control list packet-by-packet with the per-Mesa expected encoding beside every
/// field. Prints, per packet: index, byte offset, opcode, name, the raw bytes as built, the XML packet
/// length we used vs Mesa's, then one `v3d57_field` line per field. Closes with the prologue-ORDER check
/// and the tile-memory sizing check (the two things the CL bytes alone cannot show). Read-only.
fn v3d57_cl_mesa_diff(tag: &str, off: usize, len: usize) {
    v3d57_cl_mesa_diff_geom(tag, off, len, TARGET_W, TARGET_H, TILE_STATE_TILES);
}

/// PI-V3D-68 — `v3d57_cl_mesa_diff` with the EXPECTED frame geometry (and the tile count derived from
/// it) supplied by the caller, so a geometry-ladder rung is packing-checked against the encoding it
/// actually intended instead of printing two DIVERGE lines for the one field the rung is varying on
/// purpose. Passing `(TARGET_W, TARGET_H, TILE_STATE_TILES)` reproduces the pre-V3D-68 output byte for
/// byte; the plain `v3d57_cl_mesa_diff` above does exactly that for every pre-existing caller.
fn v3d57_cl_mesa_diff_geom(
    tag: &str,
    off: usize,
    len: usize,
    exp_w: usize,
    exp_h: usize,
    exp_tiles: usize,
) {
    if !V3D57_CL_AUDIT {
        return;
    }
    serial_println!(
        ":: V3D: [v3d57] {} bin CL — packing check vs the audited v42 encoding ({} bytes @ arena+{:#x}); read back from the PUBLISHED bytes, expected column = this driver's audited constants (audit authority: v3d_packet.xml v42 + v3dX(start_binning)/bcl_epilogue) ::",
        tag, len, off
    );
    // Field read: XML `start` is relative to the bit AFTER the opcode byte, so absolute bit = start + 8.
    let getb = |p: usize, xml_start: usize, width: usize| -> u64 {
        let mut abit = xml_start + 8;
        let (mut w, mut got, mut v) = (width, 0usize, 0u64);
        while w > 0 {
            let byte = arena_byte(p + abit / 8) as u64;
            let o = abit % 8;
            let take = core::cmp::min(8 - o, w);
            v |= ((byte >> o) & ((1u64 << take) - 1)) << got;
            abit += take;
            got += take;
            w -= take;
        }
        v
    };
    let mut i = 0usize;
    let mut idx = 0u32;
    let mut diverged = 0u32;
    // Prologue-order tracking: the packet sequence Mesa's binning prolog emits, in order.
    let mut order_ok = true;
    let mut seen_cfg = false;
    let mut seen_start = false;
    while i < len {
        let p = off + i;
        let op = arena_byte(p);
        let (name, plen): (&str, usize) = match op {
            P_NUMBER_OF_LAYERS => ("NUMBER_OF_LAYERS", 2),
            P_TILE_BINNING_MODE_CFG => ("TILE_BINNING_MODE_CFG", 9),
            P_FLUSH_VCD_CACHE => ("FLUSH_VCD_CACHE", 1),
            P_OCCLUSION_QUERY_COUNTER => ("OCCLUSION_QUERY_COUNTER", 5),
            P_START_TILE_BINNING => ("START_TILE_BINNING", 1),
            P_CFG_BITS => ("CFG_BITS", 4),
            P_CLIP_WINDOW => ("CLIP_WINDOW", 9),
            P_VIEWPORT_OFFSET => ("VIEWPORT_OFFSET", 9),
            P_CLIPPER_XY_SCALING => ("CLIPPER_XY_SCALING", 9),
            P_CLIPPER_Z_SCALE_AND_OFFSET => ("CLIPPER_Z_SCALE_AND_OFFSET", 9),
            P_VCM_CACHE_SIZE => ("VCM_CACHE_SIZE", 2),
            P_GL_SHADER_STATE => ("GL_SHADER_STATE", 5),
            P_VERTEX_ARRAY_PRIMS => ("VERTEX_ARRAY_PRIMS", 10),
            P_FLUSH => ("FLUSH (bin terminator)", 1),
            _ => ("UNKNOWN — NOT A MESA BIN PACKET", 1),
        };
        // Raw bytes as built (first up-to-9 payload bytes; every bin packet is <= 10 B).
        let b = |k: usize| -> u32 { if k < plen { arena_byte(p + k) as u32 } else { 0 } };
        serial_println!(
            "::   [v3d57] [{:2}] +{:#05x} op={:3} {} len={} bytes={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            idx, i, op, name, plen,
            b(0), b(1), b(2), b(3), b(4), b(5), b(6), b(7), b(8), b(9)
        );
        match op {
            P_NUMBER_OF_LAYERS => {
                // Mesa: config.number_of_layers = layers (=1); the XML field is minus_one, so the
                // packed value is 0 for a single-layer framebuffer.
                diverged += v3d57_field("number of layers (minus_one)", getb(p, 0, 8), 0);
            }
            P_TILE_BINNING_MODE_CFG => {
                seen_cfg = true;
                // v42 variant (code 120, max_ver=42). Widths/starts verbatim from v3d_packet.xml:
                //   msaa@0(1) dbuf@1(1)?  — NO: v42 places the two mode bits high (see below).
                //   init_block@2(2) block@4(2) RTs@8(4,minus_one) bpp@12(2) msaa@14(1) dbuf@15(1)
                //   width@32(16,minus_one) height@48(16,minus_one)
                diverged += v3d57_field("tile alloc INITIAL block (enum)", getb(p, 2, 2), 1); // 128 B (128>>7)
                diverged += v3d57_field("tile alloc overflow block (enum)", getb(p, 4, 2), 0); // 64 B (64>>7)
                diverged += v3d57_field("number of RTs (minus_one)", getb(p, 8, 4), 0); // MAX2(nr_cbufs,1)=1
                diverged += v3d57_field("max BPP of all RTs (enum)", getb(p, 12, 2), 0); // internal bpp 32
                diverged += v3d57_field("multisample mode 4x", getb(p, 14, 1), 0); // job->msaa = false
                diverged += v3d57_field("double-buffer in non-ms", getb(p, 15, 1), 0); // job->double_buffer = false
                diverged += v3d57_field("width in px (minus_one)", getb(p, 32, 16), (exp_w - 1) as u64);
                diverged += v3d57_field("height in px (minus_one)", getb(p, 48, 16), (exp_h - 1) as u64);
            }
            P_OCCLUSION_QUERY_COUNTER => {
                // gallium's OQ-disable: cl_emit(..., counter) with the address left at its 0 default.
                diverged += v3d57_field("OQ counter address (0=disabled)", getb(p, 0, 32), 0);
            }
            P_CFG_BITS => {
                diverged += v3d57_field("enable fwd-facing primitive", getb(p, 0, 1), 1);
                diverged += v3d57_field("enable rev-facing primitive", getb(p, 1, 1), 1);
            }
            P_CLIP_WINDOW => {
                diverged += v3d57_field("clip window left px", getb(p, 0, 16), 0);
                diverged += v3d57_field("clip window bottom px", getb(p, 16, 16), 0);
                diverged += v3d57_field("clip window width px", getb(p, 32, 16), TARGET_W as u64);
                diverged += v3d57_field("clip window height px", getb(p, 48, 16), TARGET_H as u64);
            }
            P_VIEWPORT_OFFSET => {
                // v3dx_emit.c: fine = viewport.translate · 256 (u14.8), coarse = 0 for a centred vp.
                let fine = (TARGET_W as u64 / 2) * 256;
                diverged += v3d57_field("viewport fine X (u14.8)", getb(p, 0, 22), fine);
                diverged += v3d57_field("viewport coarse X", getb(p, 22, 10), 0);
                diverged += v3d57_field("viewport fine Y (u14.8)", getb(p, 32, 22), fine);
                diverged += v3d57_field("viewport coarse Y", getb(p, 54, 10), 0);
            }
            P_CLIPPER_XY_SCALING => {
                // v3dx_emit.c: viewport.scale · clipper_xy_granularity(256.0f for v42) as f32 bits.
                let want = (((TARGET_W as f32) / 2.0) * 256.0).to_bits() as u64;
                diverged += v3d57_field("half-width f32 (1/256 px)", getb(p, 0, 32), want);
                diverged += v3d57_field("half-height f32 (1/256 px)", getb(p, 32, 32), want);
            }
            P_CLIPPER_Z_SCALE_AND_OFFSET => {
                diverged += v3d57_field("z scale f32", getb(p, 0, 32), (0.5f32).to_bits() as u64);
                diverged += v3d57_field("z offset f32", getb(p, 32, 32), (0.5f32).to_bits() as u64);
            }
            P_VCM_CACHE_SIZE => {
                // vir.c v3d_vs_set_prog_data: CLAMP(vpm_output_batches - 1, 2, 4) — never 1.
                diverged += v3d57_field("VCM batches (binning)", getb(p, 0, 4), VCM_CACHE_BATCHES);
                diverged += v3d57_field("VCM batches (rendering)", getb(p, 4, 4), VCM_CACHE_BATCHES);
            }
            P_GL_SHADER_STATE => {
                // The record pointer is frame data, not a constant — witness the ALIGNMENT invariant
                // Mesa relies on instead (the 27-bit field holds addr >> 5, so addr must be 32 B aligned).
                let rec = getb(p, 5, 27) << 5;
                serial_println!(
                    "::     [v3d57]   {:<34} record={:#010x} attrs={} 32B-aligned={} ::",
                    "GL_SHADER_STATE (frame data)", rec, getb(p, 0, 5),
                    (rec & 31 == 0) as u32
                );
            }
            P_VERTEX_ARRAY_PRIMS => {
                diverged += v3d57_field("primitive mode (enum Primitive)", getb(p, 0, 8), V3D_PRIM_TRIANGLES);
                diverged += v3d57_field("vertex count", getb(p, 8, 32), 3);
                diverged += v3d57_field("index of first vertex", getb(p, 40, 32), 0);
            }
            P_START_TILE_BINNING => {
                seen_start = true;
                // Order law, quoted in both Mesa emitters: "Binning mode lists must have a Start Tile
                // Binning item (6) after any prefix state data before the binning list proper starts."
                if !seen_cfg {
                    order_ok = false;
                    serial_println!("::     [v3d57]   START_TILE_BINNING before TILE_BINNING_MODE_CFG — ORDER DIVERGES from Mesa ::");
                }
            }
            P_FLUSH => {
                // Terminator identity check: Mesa ends every bin CL with FLUSH (4).
                diverged += v3d57_field("terminator opcode (4=FLUSH)", op as u64, P_FLUSH as u64);
            }
            _ => {}
        }
        if op == P_FLUSH {
            i += plen;
            idx += 1;
            break;
        }
        i += plen;
        idx += 1;
        if idx > 40 {
            serial_println!("::   [v3d57] [..] decode guard hit (>40 packets) — stopping ::");
            break;
        }
    }
    if !seen_start || !seen_cfg {
        order_ok = false;
    }
    // The two facts the CL bytes cannot carry: the prologue order, and the tile memory the REGISTERS
    // (not the packets) hand the PTB on v42 — v3d_job.c: "On V3D 4.1, the tile alloc/state setup moved
    // to register writes instead of binner packets."
    // Tile-STATE sizing, compared against the LITERAL Mesa formula rather than against the constant that
    // is derived from it — `TILE_STATE_BYTES == TILE_STATE_TILES * TILE_STATE_BYTES_PER_TILE` holds by
    // definition, so comparing those two would print an unreachable DIVERGE arm. The literal 256 is the
    // closing line of Mesa `v3d_tile_alloc_sizes` (v3d_util.c): tile_state = layers·tiles_x·tiles_y·256.
    let ts_want = exp_tiles * 256;
    // PI-V3D-68: both sizings are now taken against the rung's OWN tile count. For every pre-V3D-68
    // caller `exp_tiles == TILE_STATE_TILES == 1` and `ts_ours`/`alloc_min` collapse to the constants
    // this line has always printed, so the output is unchanged for them.
    let ts_ours = if exp_tiles == TILE_STATE_TILES { TILE_STATE_BYTES } else { V3D68_TSDA_BYTES };
    let alloc_min = (exp_tiles * 128).next_multiple_of(4096) + 8192;
    serial_println!(
        ":: V3D: [v3d57] {} verdict — packets={} packing-diverged={} prologue-order={} (CFG->[VCD]->[OQ]->START, terminator FLUSH/4 not FLUSH_ALL_STATE/5, no INCREMENT_SEMAPHORE) | tile-STATE bytes ours={} mesa={} ({} tile x 256, v3d_tile_alloc_sizes) {} | tile-ALLOC bytes ours={} mesa-min={} (align(tiles*128,4096)+8192) {} ::",
        tag, idx, diverged,
        if order_ok { "OK" } else { "DIVERGE" },
        ts_ours, ts_want, exp_tiles,
        if ts_ours >= ts_want { "OK" } else { "DIVERGE" },
        BIN_TILEALLOC_BYTES, alloc_min,
        if BIN_TILEALLOC_BYTES >= alloc_min { "OK" } else { "DIVERGE" },
    );
}

/// Kick one bin (CT0) + render (CT1) job pair over already-built, already-published control lists.
/// Mirrors the M4 kick sequence exactly (QMA/QMS/QTS → QBA → QEA-GO on CT0; QBA → QEA-GO on CT1;
/// finite backstops; fault-latch clear between the two) without touching the M4 code — so a V3D-10
/// change to the M4 kick path composes by mirroring the same fix here at rebase. The tile-alloc pool
/// and tile-state array are re-zeroed + re-published per call (the binner scribbles both).
fn kick_bin_render(bin_off: usize, bin_len: usize, rcl_off: usize, rcl_len: usize) -> JobResult {
    let mut res = JobResult { bin_ran: false, bin_idled: false, r_ran: false, r_idled: false, fault: 0 };

    let bin_ba = (arena_phys() + bin_off) as u32;
    let bin_ea = bin_ba + bin_len as u32;
    let rcl_ba = (arena_phys() + rcl_off) as u32;
    let rcl_ea = rcl_ba + rcl_len as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let ts = (arena_phys() + OFF_TILESTATE) as u32;
    if !arena_contains(bin_ba as usize, bin_len) || !arena_contains(rcl_ba as usize, rcl_len) {
        serial_println!(":: V3D: battery job range escapes the arena — refusing kick (fail-closed) ::");
        return res;
    }

    // Fresh binner scratch (same regions the M4 job used — free for reuse once M4 has completed).
    fill_region(OFF_TILESTATE, TILE_STATE_BYTES, 0);
    fill_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES, 0);
    cache::clean_range(arena_phys() + OFF_TILESTATE, TILE_STATE_BYTES);
    cache::clean_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);

    // CT0 (bin): BPOS=0 → invalidate → QMA/QMS/QTS → QBA → QEA (GO), per Linux v3d_sched.c
    // v3d_bin_job_run, whose first write is the overflow clear and only then the per-job cache
    // invalidate (PI-V3D-57 ordering fix; PI-V3D-12 mirror of the M4 kick fix).
    mmio_write(V3D_CORE0_BASE, V3D_PTB_BPOS, 0); // quiet: the battery runs per-frame (quiet-boot law)
    dsb();
    invalidate_gpu_caches("L2T flush (battery bin pre-kick)");
    // PI-V3D-15 mirror (V3D-11 law): the M4 kick clears any stale MMU fault BEFORE the bin so a post-
    // bin fault is attributable. Mirror it here QUIETLY — the battery runs per-frame and a verbose
    // decode/dump per frame would bury the serial witness (quiet-boot law); the verbose [v3d15] decode
    // stays on the one-shot M4 discriminator path. Accumulate any pre-existing fault into res.fault.
    res.fault |= clear_mmu_fault_latch_quiet();
    let cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, tile_alloc);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, BIN_TILEALLOC_BYTES as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, ts | V3D_CLE_CT0QTS_ENABLE);
    dsb();
    // PI-V3D-13 witness mirror of the M4 kick path.
    bin_mem_prekick_witness(
        "battery",
        tile_alloc,
        BIN_TILEALLOC_BYTES as u32,
        ts | V3D_CLE_CT0QTS_ENABLE,
    );
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, bin_ba);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, bin_ea); // GO
    dsb();
    let cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    res.bin_idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CTNCS_CTRUN, "CT0 battery bin");
    let cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    res.bin_ran = ct0_ran(cs_pre, cs_kicked, cs_done, ca_done, bin_ba, bin_ea);
    // PI-V3D-13 witness mirror of the M4 kick path.
    bin_pool_witness("battery post-bin");
    // Fault-latch hygiene between bin and render (the boot-P5 sticky-fault wedge), quiet per-frame.
    res.fault |= clear_mmu_fault_latch_quiet();

    // CT1 (render): QBA → QEA (GO). PI-V3D-12: the pre-kick invalidate here is what publishes the
    // bin's tile lists to the render CLE's branch fetch (the boot-P7 zero-stores root cause).
    invalidate_gpu_caches("L2T flush (battery render pre-kick)");
    let r_cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QBA, rcl_ba);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QEA, rcl_ea); // GO
    dsb();
    let r_cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    res.r_idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT1CS, V3D_CLE_CT1CS_CTRUN, "CT1 battery render");
    let r_cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let r_ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
    res.r_ran = ct0_ran(r_cs_pre, r_cs_kicked, r_cs_done, r_ca_done, rcl_ba, rcl_ea);
    res.fault |= clear_mmu_fault_latch_quiet();
    res
}

/// A generalised GL Shader State Record + attribute records, mirroring `build_shader_record` with the
/// addresses/counts parameterised. `attrs`: (data address, stride, values read by CS, values read by
/// VS) per attribute record. Writes record + attribute records at `rec_off`; returns attr count.
fn build_shader_record_at(
    rec_off: usize,
    cs_off: usize,
    vs_off: usize,
    fs_off: usize,
    fs_unif_off: usize,
    vs_unif_off: usize,
    cs_unif_off: usize,
    num_varyings: u64,
    attrs: &[(usize, u32, u64, u64)],
) -> u32 {
    let cs = (arena_phys() + cs_off) as u64;
    let vs = (arena_phys() + vs_off) as u64;
    let fs = (arena_phys() + fs_off) as u64;
    let defaults = (arena_phys() + OFF_DEFAULT_ATTRS) as u64;

    let mut rec = [0u8; 36];
    sf(&mut rec, 1, 1, 1); // Enable clipping
    sf(&mut rec, 24, 8, num_varyings); // Number of varyings in Fragment Shader
    // PI-V3D-25: input segment sizes = 0 (Mesa folds input → output; vir.c:918-920). See build_shader_record.
    sf(&mut rec, 32, 4, 1); // Coord Shader output VPM segment size
    sf(&mut rec, 40, 4, 0); // Coord Shader input VPM segment size  (Mesa folds input → 0)
    sf(&mut rec, 48, 4, 1); // Vertex Shader output VPM segment size
    sf(&mut rec, 56, 4, 0); // Vertex Shader input VPM segment size  (Mesa folds input → 0)
    sf(&mut rec, 64, 32, defaults);
    sf(&mut rec, 96, 1, 1); // FS 4-way threadable
    sf(&mut rec, 98, 1, 1); // FS propagate NaNs
    sf(&mut rec, 99, 29, fs >> 3);
    sf(&mut rec, 128, 32, (arena_phys() + fs_unif_off) as u64);
    sf(&mut rec, 160, 1, 1); // VS 4-way threadable
    sf(&mut rec, 162, 1, 1);
    sf(&mut rec, 163, 29, vs >> 3);
    sf(&mut rec, 192, 32, (arena_phys() + vs_unif_off) as u64);
    sf(&mut rec, 224, 1, 1); // CS 4-way threadable
    sf(&mut rec, 226, 1, 1);
    sf(&mut rec, 227, 29, cs >> 3);
    sf(&mut rec, 256, 32, (arena_phys() + cs_unif_off) as u64);
    arena_write_bytes(rec_off, &rec);

    for (i, &(addr_off, stride, cs_reads, vs_reads)) in attrs.iter().enumerate() {
        let mut attr = [0u8; 16];
        sf(&mut attr, 0, 32, (arena_phys() + addr_off) as u64);
        sf(&mut attr, 32, 2, 3); // Vec size (4 components)
        sf(&mut attr, 34, 3, 2); // Type = Attribute float
        sf(&mut attr, 40, 4, cs_reads);
        sf(&mut attr, 44, 4, vs_reads);
        sf(&mut attr, 64, 32, stride as u64);
        sf(&mut attr, 96, 32, 0xFFFF); // Maximum Index
        arena_write_bytes(rec_off + 36 + i * 16, &attr);
    }
    cache::clean_range(arena_phys() + rec_off, 36 + attrs.len() * 16);
    attrs.len() as u32
}

/// A generalised binning control list, mirroring `build_bin_cl` with the list offset and DRAWS
/// parameterised: `draws` = (shader-record offset, attr count, first vertex, vertex count) per draw —
/// M5/M6 issue one draw, M7 issues four (one per colour group). Returns the list byte length.
fn build_bin_cl_at(cl_off: usize, draws: &[(usize, u32, u32, u32)]) -> usize {
    let mut w = RclWriter::new(cl_off);
    w.pkt(Pkt::new(P_NUMBER_OF_LAYERS, 2).f(0, 8, 0).done());
    w.pkt(
        Pkt::new(P_TILE_BINNING_MODE_CFG, 9)
            .f(2, 2, TILE_ALLOC_BLOCK_SIZE_128B) // PI-V3D-14: 128B initial (Mesa-exercised config)
            .f(4, 2, TILE_ALLOC_BLOCK_SIZE_64B) // 64B overflow (Mesa OVERFLOW_BLOCK_SIZE)
            .f(8, 4, 0)
            .f(12, 2, INTERNAL_BPP_32)
            .f(32, 16, (TARGET_W - 1) as u64)
            .f(48, 16, (TARGET_H - 1) as u64)
            .done(),
    );
    // PI-V3D-23: OQ-disable in the prologue + VCM Vc = 4 (was 1, GFXH-1744-illegal) — see build_bin_cl.
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());
    w.pkt(Pkt::new(P_OCCLUSION_QUERY_COUNTER, 5).f(0, 32, 0).done());
    w.pkt(Pkt::new(P_START_TILE_BINNING, 1).done());
    w.pkt(
        Pkt::new(P_VCM_CACHE_SIZE, 2)
            .f(0, 4, VCM_CACHE_BATCHES)
            .f(4, 4, VCM_CACHE_BATCHES)
            .done(),
    );
    for &(rec_off, num_attrs, first, count) in draws {
        let shadrec = (arena_phys() + rec_off) as u32;
        w.pkt(
            // 5-byte packet — address field spans bits [5,31] (PI-V3D-10 boot-P6 root cause #1;
            // a 4-byte emission makes the CLE eat the next opcode as the record-address MSB).
            Pkt::new(P_GL_SHADER_STATE, 5)
                .f(0, 5, num_attrs as u64)
                .f(5, 27, (shadrec >> 5) as u64)
                .done(),
        );
        w.pkt(
            Pkt::new(P_VERTEX_ARRAY_PRIMS, 10)
                .f(0, 8, V3D_PRIM_TRIANGLES)
                .f(8, 32, count as u64)
                .f(40, 32, first as u64)
                .done(),
        );
    }
    w.pkt(Pkt::new(P_FLUSH, 1).done());
    let len = w.len();
    cache::clean_range(arena_phys() + cl_off, len);
    len
}

/// A generalised M4-style render control list (main list + generic per-tile sub-list with
/// BRANCH_TO_IMPLICIT_TILE_LIST), mirroring `build_m4_rcl` with the offsets parameterised. Publishes
/// both lists; returns the MAIN list byte length (the CT1 [BA, EA) extent).
fn build_battery_rcl(rcl_off: usize, sublist_off: usize, target_off: usize) -> usize {
    let target = (arena_phys() + target_off) as u32;
    let sublist_start = (arena_phys() + sublist_off) as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let stride = (TARGET_W * TARGET_BPP) as u64;

    let mut s = RclWriter::new(sublist_off);
    s.pkt(Pkt::new(P_TILE_COORDINATES_IMPLICIT, 1).done());
    s.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
    s.pkt(Pkt::new(P_PRIM_LIST_FORMAT, 2).f(0, 6, PRIM_TYPE_LIST_TRIANGLES).done());
    s.pkt(Pkt::new(P_BRANCH_TO_IMPLICIT_TILE_LIST, 2).f(0, 8, 0).done());
    s.pkt(
        Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13)
            .f(0, 4, 0)
            .f(4, 3, MEMORY_FORMAT_RASTER)
            .f(12, 6, OUTPUT_IMAGE_FORMAT_RGBA8)
            .f(28, 20, stride)
            .f(64, 32, target as u64)
            .done(),
    );
    s.pkt(Pkt::new(P_CLEAR_TILE_BUFFERS, 2).f(0, 1, 1).f(1, 1, 1).done());
    s.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    s.pkt(Pkt::new(P_RETURN_FROM_SUB_LIST, 1).done());
    let sublist_len = s.len();
    let sublist_end = sublist_start + sublist_len as u32;
    cache::clean_range(arena_phys() + sublist_off, sublist_len);

    let mut w = RclWriter::new(rcl_off);
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COMMON)
            .f(4, 4, 0)
            .f(8, 16, TARGET_W as u64)
            .f(24, 16, TARGET_H as u64)
            .f(40, 2, INTERNAL_BPP_32)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_CLEAR_COLORS_PART1)
            .f(4, 4, 0)
            .f(8, 32, CLEAR_RGBA as u64)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COLOR)
            .f(4, 2, INTERNAL_BPP_32)
            .f(6, 4, INTERNAL_TYPE_8)
            .f(10, 2, 0)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_ZS_CLEAR_VALUES)
            .f(8, 8, 0)
            .f(16, 32, 0)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TILE_LIST_INITIAL_BLOCK_SIZE, 2)
            .f(0, 2, TILE_ALLOC_BLOCK_SIZE_128B) // PI-V3D-14: match bin config's initial block
            .f(2, 1, 1)
            .done(),
    );
    w.pkt(
        Pkt::new(P_MULTICORE_TILE_LIST_BASE, 5)
            .f(0, 4, 0)
            .f(6, 26, (tile_alloc >> 6) as u64)
            .done(),
    );
    w.pkt(
        Pkt::new(P_MULTICORE_SUPERTILE_CFG, 9)
            .f(0, 8, 0)
            .f(8, 8, 0)
            .f(16, 8, 1)
            .f(24, 8, 1)
            .f(32, 12, 1)
            .f(44, 12, 1)
            .f(61, 3, 0)
            .done(),
    );
    w.pkt(Pkt::new(P_TILE_COORDINATES, 4).f(0, 12, 0).f(12, 12, 0).done());
    for i in 0..2 {
        if i > 0 {
            w.pkt(Pkt::new(P_TILE_COORDINATES, 4).f(0, 12, 0).f(12, 12, 0).done());
        }
        w.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
        w.pkt(Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13).f(0, 4, 8).done());
        if i == 0 {
            w.pkt(Pkt::new(P_CLEAR_TILE_BUFFERS, 2).f(0, 1, 1).f(1, 1, 1).done());
        }
        w.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    }
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());
    w.pkt(
        Pkt::new(P_GENERIC_TILE_LIST, 9)
            .f(0, 32, sublist_start as u64)
            .f(32, 32, sublist_end as u64)
            .done(),
    );
    w.pkt(Pkt::new(P_SUPERTILE_COORDINATES, 3).f(0, 8, 0).f(8, 8, 0).done());
    w.pkt(Pkt::new(P_END_OF_RENDERING, 1).done());
    let len = w.len();
    cache::clean_range(arena_phys() + rcl_off, len);
    len
}

/// FS uniform stream for a solid colour `rgba` at `off` (unorm8 → f32 channels + the two TLB configs,
/// same FIFO order as `write_fs_uniforms`). Publishes; returns the byte length.
fn write_fs_uniforms_colour(off: usize, rgba: u32) -> usize {
    let r = ((rgba & 0xFF) as f32 / 255.0).to_bits();
    let g = (((rgba >> 8) & 0xFF) as f32 / 255.0).to_bits();
    let b = (((rgba >> 16) & 0xFF) as f32 / 255.0).to_bits();
    let a = (((rgba >> 24) & 0xFF) as f32 / 255.0).to_bits();
    let unif: [u32; 6] = [r, g, b, a, 0xFFFF_FF84, 0xFFFF_FF3F];
    for (i, w) in unif.iter().enumerate() {
        arena_write_u32(off + i * 4, *w);
    }
    cache::clean_range(arena_phys() + off, unif.len() * 4);
    unif.len() * 4
}

/// Read one 32-bit pixel from an arbitrary battery target at (x, y).
#[inline]
fn target_sample(target_off: usize, x: usize, y: usize) -> u32 {
    let off = target_off + (y * TARGET_W + x) * TARGET_BPP;
    let arena = &raw const V3D_ARENA;
    unsafe {
        let b = &(*arena).bytes;
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
}

/// Blit a 64×64 battery target to the panel at pixel origin (x0, y0) — the same bounds-clipped volatile
/// idiom as `blit_target`/`blit_m4_target`, parameterised.
fn blit_target_at(fb: &FbTarget, target_off: usize, x0: usize, y0: usize) {
    if fb.base == 0 || fb.bytes_per_pixel < 4 {
        return;
    }
    let w = TARGET_W.min(fb.width.saturating_sub(x0));
    let h = TARGET_H.min(fb.height.saturating_sub(y0));
    for y in 0..h {
        for x in 0..w {
            let px = target_sample(target_off, x, y);
            let dst = fb.base as usize
                + (y0 + y) * fb.stride_px * fb.bytes_per_pixel
                + (x0 + x) * fb.bytes_per_pixel;
            if dst + 4 <= fb.base as usize + fb.size {
                unsafe { core::ptr::write_volatile(dst as *mut u32, px) };
            }
        }
    }
}

/// Write one vec4-f32 vertex into the arena at `off`.
#[inline]
fn write_vert4(off: usize, v: [f32; 4]) {
    for (j, c) in v.iter().enumerate() {
        arena_write_u32(off + j * 4, c.to_bits());
    }
}

/// ── M5: the GRADIENT triangle — per-vertex colour varyings through the QPU varying path. ─────────
fn m5_gradient_job(fb: Option<FbTarget>) {
    serial_println!(":: V3D: M5 gradient — per-vertex colour varyings (ldvary FS) ::");

    // Shaders: verified CS (position passthrough) for binning; the render-path STVPMV gradient VS + ldvary FS.
    let vs_len = write_shader_words(OFF_M5_VS_CODE, &GRAD_VS_WORDS);
    let fs_len = write_shader_words(OFF_M5_FS_CODE, &GRAD_FS_WORDS);
    cache::clean_range(arena_phys() + OFF_M5_VS_CODE, vs_len);
    cache::clean_range(arena_phys() + OFF_M5_FS_CODE, fs_len);

    // FS uniforms: alpha=1.0 then the two TLB configs (FIFO order of GRAD_FS_WORDS' pops).
    let unif: [u32; 3] = [1.0f32.to_bits(), 0xFFFF_FF84, 0xFFFF_FF3F];
    for (i, w) in unif.iter().enumerate() {
        arena_write_u32(OFF_M5_FS_UNIF + i * 4, *w);
    }
    cache::clean_range(arena_phys() + OFF_M5_FS_UNIF, unif.len() * 4);
    // VS uniforms (PI-V3D-22): the render-path VS FIFO — eight VPM in-offsets (vec4 pos + vec4 colour),
    // then the 8192.0f XY scale, the viewport Z scale/offset (0.5/0.5, the CLIPPER_Z params), then the
    // eight output VPM offsets 0..7 the STVPMV stores consume (Mesa sources these as vir_uniform_ui).
    // In-offsets are the metal-refinement surface; the scales/out-offsets are the Mesa contract values.
    let vs_unif: [u32; 19] = [
        0, 1, 2, 3, 4, 5, 6, 7, // VPM read-offsets (pos.xyzw, col.rgba)
        0x4600_0000,            // 8192.0f32 vp_scale
        0x3F00_0000,            // 0.5f32 viewport_z_scale
        0x3F00_0000,            // 0.5f32 viewport_z_offset
        0, 1, 2, 3, 4, 5, 6, 7, // output VPM offsets 0..7
    ];
    for (i, w) in vs_unif.iter().enumerate() {
        arena_write_u32(OFF_M5_VS_UNIF + i * 4, *w);
    }
    cache::clean_range(arena_phys() + OFF_M5_VS_UNIF, vs_unif.len() * 4);

    // Interleaved vertex data: [pos vec4 | colour vec4] × 3, stride 32 B. Colours are the f32
    // decomposition of the per-vertex primaries.
    for (i, v) in TRI_VERTS.iter().enumerate() {
        write_vert4(OFF_M5_VTX + i * 32, *v);
        let c = M5_VERT_COLOURS[i];
        let col = [
            (c & 0xFF) as f32 / 255.0,
            ((c >> 8) & 0xFF) as f32 / 255.0,
            ((c >> 16) & 0xFF) as f32 / 255.0,
            1.0,
        ];
        write_vert4(OFF_M5_VTX + i * 32 + 16, col);
    }
    cache::clean_range(arena_phys() + OFF_M5_VTX, 3 * 32);

    // Shader record: CS reads only position (attr 0); VS reads position + colour (attrs 0 and 1);
    // 4 varyings (the colour vec4) flow VS → FS.
    let num_attrs = build_shader_record_at(
        OFF_M5_SHADREC,
        OFF_CS_CODE, // verified position-only coordinate shader (binning needs no colour)
        OFF_M5_VS_CODE,
        OFF_M5_FS_CODE,
        OFF_M5_FS_UNIF,
        OFF_M5_VS_UNIF,
        OFF_CS_UNIF, // the M4 CS read-offset stream (still published; position-only)
        4,
        &[(OFF_M5_VTX, 32, 4, 4), (OFF_M5_VTX + 16, 32, 0, 4)],
    );
    let bin_len = build_bin_cl_at(OFF_M5_BIN_CL, &[(OFF_M5_SHADREC, num_attrs, 0, 3)]);
    let rcl_len = build_battery_rcl(OFF_M5_RCL, OFF_M5_SUBLIST, OFF_M5_TARGET);

    fill_region(OFF_M5_TARGET, TARGET_BYTES, BAT_SENTINEL);
    cache::clean_range(arena_phys() + OFF_M5_TARGET, TARGET_BYTES);

    let job = kick_bin_render(OFF_M5_BIN_CL, bin_len, OFF_M5_RCL, rcl_len);
    cache::clean_invalidate_range(arena_phys() + OFF_M5_TARGET, TARGET_BYTES);

    // Witness: three interior samples near the three coloured corners must be pairwise DISTINCT and
    // neither clear nor sentinel (interpolation produced per-corner-dominated colours); two exterior
    // corners must be the clear colour. Colour-exactness is the flagged metal seam (raw ldvary).
    let s0 = target_sample(OFF_M5_TARGET, 16, 48); // near lower-left (red) corner
    let s1 = target_sample(OFF_M5_TARGET, 47, 48); // near lower-right (green) corner
    let s2 = target_sample(OFF_M5_TARGET, 32, 18); // near top (blue) corner
    let o0 = target_sample(OFF_M5_TARGET, 2, 2);
    let o1 = target_sample(OFF_M5_TARGET, 61, 2);
    let interior_live = |s: u32| s != CLEAR_RGBA && s != BAT_SENTINEL;
    let distinct = s0 != s1 && s1 != s2 && s0 != s2;
    let pass = job.clean()
        && distinct
        && interior_live(s0)
        && interior_live(s1)
        && interior_live(s2)
        && o0 == CLEAR_RGBA
        && o1 == CLEAR_RGBA;
    serial_println!(
        ":: V3D: M5 gradient {} — in={:#010x}/{:#010x}/{:#010x} distinct={} out={:#010x}/{:#010x} ran={}/{} idled={}/{} faults={:#x} (varying math = metal seam) ::",
        if pass { "PASS" } else { "FAIL" },
        s0, s1, s2, distinct as u32, o0, o1,
        job.bin_ran as u32, job.r_ran as u32, job.bin_idled as u32, job.r_idled as u32, job.fault
    );
    super::exceptions::serror_drain_request("v3d: M5 gradient kick window");
    if pass {
        if let Some(fb) = fb {
            blit_target_at(&fb, OFF_M5_TARGET, 2 * (TARGET_W + 8), 0); // right of the M4 blit
        }
    }
}

/// Build the shared M6/M7 solid-colour scaffold: a shader record at OFF_BAT_SHADREC (+`rec_slot`×128)
/// using the VERIFIED M4 shaders with vertex data at OFF_BAT_VTX and FS uniforms at `fs_unif_off`.
fn build_bat_solid_record(rec_slot: usize, fs_unif_off: usize) -> (usize, u32) {
    let rec_off = OFF_BAT_SHADREC + rec_slot * 128;
    let n = build_shader_record_at(
        rec_off,
        OFF_CS_CODE,
        OFF_VS_CODE,
        OFF_FS_CODE,
        fs_unif_off,
        OFF_VS_UNIF,
        OFF_CS_UNIF,
        0, // solid colour: no varyings (the M4 shape)
        &[(OFF_BAT_VTX, 16, 4, 4)],
    );
    (rec_off, n)
}

/// ── M6: the ANIMATED triangle — re-record + re-kick per frame, ~5 s of sustained bin/render. ─────
fn m6_animated_job(fb: Option<FbTarget>) {
    serial_println!(
        ":: V3D: M6 animate — {} frames @ ~{} ms (sustained bin/render loop) ::",
        M6_FRAMES, M6_FRAME_PACE_MS
    );

    // Solid-colour scaffold: verified M4 shaders (already written + published by triangle_job), one
    // record whose attribute data lives at OFF_BAT_VTX. FS uniforms reuse the amber M4 stream.
    let (rec_off, num_attrs) = build_bat_solid_record(0, OFF_FS_UNIF);
    let bin_len = build_bin_cl_at(OFF_BAT_BIN_CL, &[(rec_off, num_attrs, 0, 3)]);
    let rcl_len = build_battery_rcl(OFF_BAT_RCL, OFF_BAT_SUBLIST, OFF_BAT_TARGET);

    fill_region(OFF_BAT_TARGET, TARGET_BYTES, BAT_SENTINEL);
    cache::clean_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);

    let mut frames_ok = 0usize;
    let mut faults = 0u32;
    let mut fault_frames = 0usize;
    // 144 rotation frames + one final identity frame (so the closing sample-verify has a known pose).
    for frame in 0..=M6_FRAMES {
        let (c, s) = if frame == M6_FRAMES { ROT24[0] } else { ROT24[frame % ROT24.len()] };
        for (i, v) in TRI_VERTS.iter().enumerate() {
            let (x, y) = (v[0], v[1]);
            write_vert4(
                OFF_BAT_VTX + i * 16,
                [x * c - y * s, x * s + y * c, v[2], v[3]],
            );
        }
        cache::clean_range(arena_phys() + OFF_BAT_VTX, 3 * 16);
        let job = kick_bin_render(OFF_BAT_BIN_CL, bin_len, OFF_BAT_RCL, rcl_len);
        if job.clean() {
            frames_ok += 1;
        }
        if job.fault != 0 {
            faults |= job.fault;
            fault_frames += 1;
        }
        if frame < M6_FRAMES {
            settle_ms(M6_FRAME_PACE_MS); // ~5 s of wall-clock animation for the eyeball witness
        }
        // Live on-glass animation: blit each frame as it completes (the eyeball IS the witness).
        if let Some(fbt) = fb {
            cache::clean_invalidate_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);
            blit_target_at(&fbt, OFF_BAT_TARGET, 0, TARGET_H + 8); // below the M3 blit
        }
    }

    // Closing verify on the identity-pose final frame: centroid = triangle colour, corner = clear.
    cache::clean_invalidate_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);
    let centroid = target_sample(OFF_BAT_TARGET, 32, 34);
    let corner = target_sample(OFF_BAT_TARGET, 2, 2);
    let total = M6_FRAMES + 1;
    let pass = frames_ok == total && faults == 0 && centroid == TRI_RGBA && corner == CLEAR_RGBA;
    serial_println!(
        ":: V3D: M6 animate {} — frames={}/{} faults={:#x} (fault-frames={}) centroid={:#010x} corner={:#010x} ::",
        if pass { "PASS" } else { "FAIL" },
        frames_ok, total, faults, fault_frames, centroid, corner
    );
    super::exceptions::serror_drain_request("v3d: M6 animate kick window");
}

/// ── M7: the MULTI-PRIMITIVE frame — a 12-wedge pinwheel in four colours (four draws, one frame). ─
fn m7_multiprim_job(fb: Option<FbTarget>) {
    serial_println!(":: V3D: M7 multiprim — 12-triangle pinwheel, 4 colour draws ::");

    // Vertex data: wedge k = centre, rim(θk), rim(θk+30°); θk = k·30° (every other ROT24 entry).
    const R: f32 = 0.8;
    for k in 0..12 {
        let (c0, s0) = ROT24[(2 * k) % 24];
        let (c1, s1) = ROT24[(2 * k + 2) % 24];
        let base = OFF_BAT_VTX + k * 3 * 16;
        write_vert4(base, [0.0, 0.0, 0.5, 1.0]);
        write_vert4(base + 16, [R * c0, R * s0, 0.5, 1.0]);
        write_vert4(base + 32, [R * c1, R * s1, 0.5, 1.0]);
    }
    cache::clean_range(arena_phys() + OFF_BAT_VTX, 12 * 3 * 16);

    // Four draws: 3 consecutive wedges each, distinct FS uniform stream (solid colour per group) —
    // multi-colour without any new QPU words: the verified FS reads its colour from the uniform FIFO.
    let mut draws: [(usize, u32, u32, u32); 4] = [(0, 0, 0, 0); 4];
    for (k, &colour) in M7_COLOURS.iter().enumerate() {
        let unif_off = OFF_M7_UNIF + k * 64;
        write_fs_uniforms_colour(unif_off, colour);
        let (rec_off, n) = build_bat_solid_record(k, unif_off);
        draws[k] = (rec_off, n, (k * 9) as u32, 9);
    }
    let bin_len = build_bin_cl_at(OFF_BAT_BIN_CL, &draws);
    let rcl_len = build_battery_rcl(OFF_BAT_RCL, OFF_BAT_SUBLIST, OFF_BAT_TARGET);

    fill_region(OFF_BAT_TARGET, TARGET_BYTES, BAT_SENTINEL);
    cache::clean_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);

    let job = kick_bin_render(OFF_BAT_BIN_CL, bin_len, OFF_BAT_RCL, rcl_len);
    cache::clean_invalidate_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);

    // Witness: one sample inside each colour group's mid-wedge (live: neither clear nor sentinel —
    // the exact colour-to-quadrant mapping depends on the viewport transform, the flagged PI-V3D-9
    // metal seam) + two rim corners the R=0.8 pinwheel cannot reach = clear.
    let q: [(usize, usize); 4] = [(41, 25), (23, 25), (23, 43), (41, 43)];
    let mut lives = 0u32;
    let mut vals = [0u32; 4];
    for (i, &(x, y)) in q.iter().enumerate() {
        let s = target_sample(OFF_BAT_TARGET, x, y);
        vals[i] = s;
        if s != CLEAR_RGBA && s != BAT_SENTINEL {
            lives += 1;
        }
    }
    let o0 = target_sample(OFF_BAT_TARGET, 1, 1);
    let o1 = target_sample(OFF_BAT_TARGET, 62, 62);
    let pass = job.clean() && lives == 4 && o0 == CLEAR_RGBA && o1 == CLEAR_RGBA;
    serial_println!(
        ":: V3D: M7 multiprim {} — quads={:#010x}/{:#010x}/{:#010x}/{:#010x} live={}/4 out={:#010x}/{:#010x} ran={}/{} idled={}/{} faults={:#x} ::",
        if pass { "PASS" } else { "FAIL" },
        vals[0], vals[1], vals[2], vals[3], lives, o0, o1,
        job.bin_ran as u32, job.r_ran as u32, job.bin_idled as u32, job.r_idled as u32, job.fault
    );
    super::exceptions::serror_drain_request("v3d: M7 multiprim kick window");
    if pass {
        if let Some(fbt) = fb {
            blit_target_at(&fbt, OFF_BAT_TARGET, TARGET_W + 8, TARGET_H + 8);
        }
    }
}

/// ── M8: BLIT TO SCANOUT — composite the battery render target onto the live framebuffer console and
/// read the written words back from the panel memory (end-to-end GPU→glass witness). The blit is a
/// bounded 64×64 region (the GUI stays usable); readback compares three probe pixels source↔panel. ─
fn m8_blit_scanout(fb: Option<FbTarget>) {
    let Some(fbt) = fb else {
        serial_println!(":: V3D: M8 blit SKIP — no framebuffer target (serial-only run) ::");
        return;
    };
    if fbt.base == 0 || fbt.bytes_per_pixel < 4 {
        serial_println!(":: V3D: M8 blit SKIP — framebuffer not blittable (bpp<4 or null base) ::");
        return;
    }
    // Composite the M7 scene (the battery target's final contents) at a fixed console-corner slot.
    let (x0, y0) = (2 * (TARGET_W + 8), TARGET_H + 8);
    blit_target_at(&fbt, OFF_BAT_TARGET, x0, y0);

    // Readback witness: three probe pixels re-read VOLATILE from the panel memory must equal the
    // source target words (proves the composite landed in scanout-visible memory, not a stale cache).
    let probes: [(usize, usize); 3] = [(0, 0), (32, 34), (63, 63)];
    let mut ok = 0u32;
    let mut got = [0u32; 3];
    let mut want = [0u32; 3];
    for (i, &(x, y)) in probes.iter().enumerate() {
        want[i] = target_sample(OFF_BAT_TARGET, x, y);
        let dst = fbt.base as usize
            + (y0 + y) * fbt.stride_px * fbt.bytes_per_pixel
            + (x0 + x) * fbt.bytes_per_pixel;
        if x0 + x < fbt.width && y0 + y < fbt.height && dst + 4 <= fbt.base as usize + fbt.size {
            got[i] = unsafe { core::ptr::read_volatile(dst as *const u32) };
            if got[i] == want[i] {
                ok += 1;
            }
        }
    }
    let pass = ok == probes.len() as u32;
    serial_println!(
        ":: V3D: M8 blit {} — probes {}/{} panel={:#010x}/{:#010x}/{:#010x} src={:#010x}/{:#010x}/{:#010x} @({},{}) ::",
        if pass { "PASS" } else { "FAIL" },
        ok, probes.len(), got[0], got[1], got[2], want[0], want[1], want[2], x0, y0
    );
}

/// PI-V3D-11 battery entry: run the four visible stages in order. Called from `bringup` AFTER the M3
/// clear + M4 triangle regressions; only reachable on metal (QEMU returned at BLOCK-DOWN). Each stage
/// is independent — a FAIL prints its verdict and the battery continues (every stage is a witness the
/// attended sitting wants regardless of the others).
fn battery(fb: Option<FbTarget>) {
    serial_println!(":: V3D: PI-V3D-11 battery — M5 gradient, M6 animate, M7 multiprim, M8 blit ::");
    m5_gradient_job(fb);
    m6_animated_job(fb);
    m7_multiprim_job(fb);
    m8_blit_scanout(fb);
    super::exceptions::serror_drain_request("v3d: battery exit");
}

/// The number of VISIBLE battery stages `battery` replays (M5 gradient, M6 animate, M7 multiprim,
/// M8 blit). Kept as one constant so the `v3d` app's `stages=N` witness never drifts from `battery`.
const VISIBLE_BATTERY_STAGES: u32 = 4;

// ── PI-APP-1 replay state. Latched once at the tail of a successful boot `bringup` (block up, MMU
// programmed, visible battery already run). The `v3d` shell app reads it to REPLAY the visible stages
// on the live framebuffer WITHOUT re-entering the init path. `V3D_REPLAY_FB` is written exactly once
// (single-threaded boot, pre-shell) and only ever read after `V3D_REPLAY_READY` is observed true, so
// the plain `static mut` needs no further synchronisation beyond the acquire/release on the flag.
static V3D_REPLAY_READY: AtomicBool = AtomicBool::new(false);
static mut V3D_REPLAY_FB: Option<FbTarget> = None;

/// PI-APP-1: replay the VISIBLE V3D battery on the live framebuffer, on demand from the shell.
///
/// Re-entry safety: this does NOT call `bringup`. It reuses the state boot already established — the
/// V3D power domain, clock gate, PM/ASB bridges and the V3D MMU all stay enabled from boot, and the
/// buffer arena stays identity-mapped. Each visible stage (`m5..m8`) rebuilds its own control list
/// into fixed arena offsets from scratch and re-kicks the GPU, so re-running them is idempotent — no
/// static needs re-init and no init step is duplicated. If boot never brought the block up (QEMU
/// raspi4b returns at BLOCK-DOWN; any fail-closed probe/MMU verdict), the flag is false and we print
/// a skip witness and touch no MMIO — the serial-only gate stays clean.
///
/// Prints `:: V3D: app replay start ::` / `:: V3D: app replay done (stages=N) ::` for the bench.
/// Returns the number of stages replayed (0 when the block was never up).
pub fn run_visible_battery_again() -> u32 {
    serial_println!(":: V3D: app replay start ::");
    if !V3D_REPLAY_READY.load(Ordering::Acquire) {
        serial_println!(
            ":: V3D: app replay done (stages=0) — V3D not brought up this boot (absent/fail-closed); nothing to replay ::"
        );
        return 0;
    }
    // SAFETY: written once at boot before any shell exists; only read here after the acquire load
    // above observed the release store, so the FbTarget is fully published.
    let fb = unsafe { V3D_REPLAY_FB };
    battery(fb);
    serial_println!(
        ":: V3D: app replay done (stages={}) ::",
        VISIBLE_BATTERY_STAGES
    );
    VISIBLE_BATTERY_STAGES
}
