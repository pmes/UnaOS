// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// WIFI-2 — map BAR0, walk the bcma enumeration ROM from OUR OWN reads, identify the d11 802.11 core,
// read its wrapper + core state, and apply the enable rule. Arc 2 of the BCM4331 ladder.
//
// ## What this arc is allowed to do, stated before anything else
//
// | rung | device access | reversible? |
// | --- | --- | --- |
// | R0 precondition | PCI config READS (identity, PMCSR, COMMAND, BARs) | n/a |
// | R1 map | `map_mmio_window` — a page-table edit. **The device sees nothing.** | n/a |
// | R2 pre-image + unwind self-test | writes cfg:0x80 **with the value it already holds** (a no-op) | n/a |
// | R3 ChipCommon | cfg:0x80 <- `0x18000000`, then MMIO READS | yes — R8 restores |
// | R4 EROM walk | cfg:0x80 <- erom base, then MMIO READS | yes — R8 restores |
// | R5 d11 window | cfg:0x80 <- d11 base, then MMIO READS (core + wrapper) | yes — R8 restores |
// | R6 enable rule | **NO WRITE when the core already arrives enabled** (the metal-expected case); a
// |                | witnessed `RESET_CTL`/`IOCTL` pair only when it does not, **unwound in place on
// |                | the STILL-DOWN path** so a failed enable leaves the wrapper as it was found | yes |
// | R7 upload | **REFUSED** — see "The upload, and why it does not happen here" | n/a |
// | R8 restore | cfg:0x80 <- the recorded pre-image, MATCH-verified | — |
//
// Every device write in this file carries a **pre-read / write / post-read witness triple**, and
// every count on the `end` line is a FIELD of [`Writes`] incremented at the write site — including
// the audited zeros (`wrote-core-regs=`, `uploaded-bytes=`), which were string literals in the first
// cut of this file and are now values. An audited zero that the code cannot violate is worth having;
// an audited zero that is a literal is worth nothing.
//
// ## The gate that opened, and the gate that did not
//
// Arc 1 flew on metal this morning (Boot A, `~/unaos-bench/capture/gr25-bootA/ttyUSB0.log`):
//
// ```text
// :: wifi: radio SELECTED 04:00.0 matches=1 expected=1 MATCH cross-check=PASS (vs bcm4331.md §0, Boots AF/AJ) ::
// :: wifi: firmware set INCOMPLETE 0/3 staged (0 rejected) on source=sdhc label='UNAOS-X86' … ::
// ```
//
// So the identity gate arc 2 is conditioned on is **PASS**, and the firmware gate is **0/3**. This
// file is written to fly usefully on exactly that machine: the whole core-walk ladder runs, and the
// destructive half parks with a line that says why.
//
// ## The upload, and why it does not happen here — a NAMED UNKNOWN, not an omission
//
// `bcm4331.md` §S4 describes the upload as: `SHM_CONTROL <- (B43_SHM_UCODE << 16) | 0`, then stream
// big-endian-decoded 32-bit words into `SHM_DATA`. **The numeric value of the `B43_SHM_UCODE` routing
// selector appears in no source this module may use.** It is not in §S4 (which names the symbol, not
// the value), it is not in this tree's own constants (`drivers/bcma.rs` carries only
// `SHM_ROUTE_SHARED = 0x0001`, the routing S4a's read probe uses), and it has never been measured on
// our metal. The one place it exists is the b43 header — GPL Linux WiFi driver source, off-limits for
// `src/wifi/` under `docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §2.
//
// Guessing it is not a small risk dressed as a large one. §S4a's whole safety argument for touching
// `SHM_CONTROL` is that the probe **never writes the data port**, so a wrong routing reads a
// different word — garbage in, garbage out, no damage. An UPLOAD inverts that asymmetry exactly: a
// wrong routing streams tens of kilobytes into whatever bank the window actually selected. So the
// upload rung refuses at the selector, names the unknown on the wire, and says what would settle it.
//
// And because the upload cannot follow, **§S4's reset-to-known-state prologue is not made either.**
// That prologue asserts `RESET_CTL.RESET`, which clears `MACCTL.PSM_RUN` and destroys the microcode
// this radio arrives running (`bcm4331.md` §5 risk 4: "the reset destroys the firmware-resident
// microcode the radio arrives running, and only a successful upload restores a working state").
// Making a destructive, unrecoverable write whose only justification is an upload that cannot happen
// is the improvisation the arc discipline forbids. It is NAMED here and MADE by the arc that carries
// the upload, exactly as §S3 named it and declined to make it.
//
// ## Sourcing — what was ADOPTED, and from where
//
// **This file's constant block and its EROM cursor scaffolding are ADOPTED FROM `drivers/bcma.rs`,
// and therefore INHERIT that module's recorded Group-B taint.** Said plainly because the first cut of
// this header said the opposite, and the opposite was false:
//
//   * the ~59 register offsets, bit masks and EROM entry encodings below are the sibling's, name for
//     name and value for value — including [`EROM_MAX_CORES`], which is a print budget rather than a
//     property of any silicon and so cannot have been arrived at independently;
//   * [`Erom`]'s scaffolding — the struct, `new`, `stopped`, `peek`, `take`, `ci`, `at_end`,
//     `master_port` — is the sibling's executable code.
//
// `drivers/bcma.rs` states in its own header that its "register offsets and EROM encodings follow
// Linux `drivers/bcma` (`bcma_regs.h`, `bcma_driver_chipcommon.h`, `scan.c`) and `b43`'s
// `B43_MMIO_PHY_VER`". That provenance travels with the adoption. Under
// `docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §2 this file is therefore on the **Group-B side** for those
// facts, and `src/wifi/` no longer carries a subsystem-wide clean-room claim. The taint is RECORDED,
// not laundered — an illusion of cleanliness is worth less than a ledger that is true.
//
// **What IS independent** is the parse strategy and nothing else: [`Erom::address`] and the walk
// driver in [`walk_erom`] consume a component's descriptors by TAG until the next identifier, rather
// than by the declared port counts, which is a different algorithm from the sibling's and is what
// frees those counts to serve as the arity cross-check. That distinction is worth one paragraph, not
// a clean-room claim over the file.
//
// The implementer of THIS file read no GPL Linux WiFi driver source directly; every externally-
// derived fact arrived through `bcm4331.md` and `drivers/bcma.rs`. That is a statement about the
// route, not a laundering of the origin, and it does not change the classification above.
//
// Per-fact tags, at every constant:
//
//   * `[METAL]`  — measured by THIS kernel on OUR bench and recorded in `bcm4331.md`. The strongest
//                  class: a value we can point at a capture for.
//   * `[PUBLIC]` — the public PCI base specification / PCI-SIG registry.
//   * `[LEDGER]` — externally derived, AND corroborated by one of our captures in a way that could
//                  have come out otherwise. The corroboration is NAMED at the constant.
//   * `[EXT-CORROBORATION-WEAK]` — externally derived, and the capture usually cited for it is
//                  **non-discriminating**: the register answered with something, or a decode of it
//                  was self-consistent, but no reading available to us would have come out
//                  differently had the offset or bit position been wrong. Its only discriminating
//                  source is b43. **R6 — the one rung that writes a backplane register — is gated
//                  entirely on facts in this class**, and that is stated here rather than left for a
//                  reader to work out.
//   * `[EXT-UNPINNED]` — externally derived and not corroborated at all. Decoded and printed, never
//                  written, never load-bearing.

use crate::arch::pci::{read_config_16, read_config_32, write_config_32};

// ── PCI config offsets ──────────────────────────────────────────────────────────────────────────

/// `[PUBLIC]` Command (low 16) + Status (high 16).
const CFG_CMD_STS: u8 = 0x04;
/// `[PUBLIC]` Capability list pointer.
const CFG_CAP_PTR: u8 = 0x34;
/// `[PUBLIC]` Capability id 0x01 = PCI Power Management. PMCSR sits 4 bytes past the header; its low
/// two bits are the D-state. An MMIO read of a function parked in D3hot returns undefined data, so
/// the state is checked before the window is touched.
const CAP_ID_PM: u8 = 0x01;
/// `[LEDGER]` `BCMA_PCI_BAR0_WIN` — the backplane address the first 4 KiB of BAR0 decodes to.
/// Corroborated on OUR metal three times over: Boot AF read `0x18001000` out of it, Boot AJ wrote
/// `0x18000000` into it and ChipCommon answered with `chipid=0x13924331`, and the restore round-
/// tripped. This is the single register that gates the whole backplane.
const CFG_BAR0_WIN: u8 = 0x80;
/// `[LEDGER]` `BCMA_PCI_BAR0_WIN2` — the backplane address the SECOND 4 KiB of BAR0 (the wrapper
/// aperture) decodes to. Corroborated by Boot AF (`0x18101000`) agreeing to the bit with the master
/// wrapper the EROM walk independently reported on Boots AL/AM/AN.
const CFG_BAR0_WIN2: u8 = 0xAC;

// ── Backplane geometry ──────────────────────────────────────────────────────────────────────────

/// `[METAL]` `BCMA_ADDR_BASE` — ChipCommon's backplane address. Boot AJ: writing this into cfg:0x80
/// turned a function that had read all-ones for the life of the project into a chip that answers.
const BCMA_ADDR_BASE: u32 = 0x1800_0000;
/// `[LEDGER]` One core's register window, and the granularity of the window selector (its low 12 bits
/// are ignored by hardware). Corroborated: firmware's own `(0x18001000, 0x18101000)` pair is
/// `base + 1*0x1000` on both grids.
const BCMA_CORE_SIZE: u32 = 0x1000;
/// `[LEDGER]` How much of BAR0 is mapped: two 4 KiB apertures, the core window and the wrapper
/// window. `bcm4331.md` §S0 argues the inequality runs the safe way — both apertures exist and are
/// used, so the true BAR is at least this big and mapping it cannot map past the BAR. Corroborated by
/// every boot since AF, which mapped exactly this and read both apertures successfully.
const BAR0_MAP_LEN: usize = 0x2000;
/// `[LEDGER]` Byte offset of the wrapper aperture inside BAR0.
const BAR0_WRAP_OFF: u64 = 0x1000;

// ── Core-wrapper (agent) registers, relative to the wrapper aperture ────────────────────────────
//
// `[EXT-CORROBORATION-WEAK]` — all seven constants in this block, and the downgrade is deliberate.
//
// The corroboration previously claimed for them was: "all four were READ on Boots AL/AM/AN and
// returned a self-consistent set (`ioctl=0x00002055 iost=0x0000100c resetctl=0 resetst=0`) rather
// than all-ones". That is **non-discriminating**. Four arbitrary offsets inside a live 4 KiB agent
// window will also return non-all-ones values, and `resetctl=0`/`resetst=0` are exactly what a
// *wrong* offset landing on a reserved word returns. No reading available to us would have come out
// differently had these offsets been wrong, so the capture cannot be cited as evidence for them.
//
// Their only discriminating source is b43 / Linux `bcma`, reached here through `drivers/bcma.rs`.
// **R6 writes `RESET_CTL` and `IOCTL` on the not-enabled branch, so the one rung in this file that
// writes a backplane register is gated entirely on this class.** That is the honest position and it
// is why R6's writes are individually unwound (see [`reach_d11`]) rather than merely "reversible in
// principle".

const WRAP_IOCTL: u64 = 0x0408;
const WRAP_IOST: u64 = 0x0500;
const WRAP_RESET_CTL: u64 = 0x0800;
const WRAP_RESET_ST: u64 = 0x0804;

/// `[EXT-CORROBORATION-WEAK]` `BCMA_IOCTL_CLK`. Half of the two-bit enable rule. Our captured
/// `ioctl=0x00002055` satisfies `& 0x3 == 0x1` — but a word with bit 0 set is not evidence that bit 0
/// MEANS "clocked", and no capture of ours distinguishes the two.
const IOCTL_CLK: u32 = 0x0000_0001;
/// `[EXT-CORROBORATION-WEAK]` `BCMA_IOCTL_FGC` — forced gated clock. Same class as above.
const IOCTL_FGC: u32 = 0x0000_0002;
/// `[EXT-CORROBORATION-WEAK]` `BCMA_RESET_CTL_RESET`. `resetctl=0x00000000` on our metal is
/// consistent with this bit position and with every other bit position, which is precisely the
/// problem with citing it.
const RESET_CTL_RESET: u32 = 0x0000_0001;

// ── d11 core registers, relative to the core aperture (BAR0+0 with the window on the d11 base) ──

/// `[EXT-CORROBORATION-WEAK]` `B43_MMIO_MACCTL`. Read on our metal (Boot AO: `macctl=0xc0020403`).
/// The value is non-degenerate, which corroborates "something lives at +0x120" and nothing more.
const D11_MACCTL: u64 = 0x0120;
/// `[LEDGER]` `B43_MMIO_REV3PLUS_TSF_LOW` / `_HIGH`. Read on our metal, and the corroboration IS
/// discriminating for once: the pair ADVANCED between two samples and decoded to a plausible
/// microsecond count. A wrong offset would have to land on another free-running counter to fake that.
const D11_TSF_LOW: u64 = 0x0180;
const D11_TSF_HIGH: u64 = 0x0184;
/// `[LEDGER]` `B43_MMIO_PHY_VER`, 16 bits. Discriminating: our metal read `0x9701`, which decodes to
/// analog 9 / type 7 (HT) / rev 1 — a reading that is both structured and exactly what an HT-PHY 4331
/// should say. A wrong offset producing that is a coincidence, not an explanation.
const D11_PHY_VER: u64 = 0x03E0;

/// `[EXT-CORROBORATION-WEAK]` `MACCTL.PSM_RUN`. §S3 reads bit 1 of `0xc0020403` as "the microcode
/// processor is executing" and builds the destructive-prologue argument on it — but that reading is
/// b43's decode of the word, not a measurement of the bit's meaning. Decoded and printed here; the
/// only thing that RESTS on it is a witness digit and a paragraph of prose, never a write.
const MACCTL_PSM_RUN: u32 = 0x0000_0002;
/// `[EXT-UNPINNED]` `MACCTL.PSM_JMP0`. Named by §S4 as the bit the prologue sets before an upload.
/// Never written here; decoded only, so an incorrect bit position costs a wrong digit on a witness
/// line and nothing else.
const MACCTL_PSM_JMP0: u32 = 0x0000_0004;
/// `[EXT-CORROBORATION-WEAK]` `MACCTL.SHM_ENABLED`. No capture of ours distinguishes this bit.
const MACCTL_SHM_ENABLED: u32 = 0x0000_0100;
/// `[EXT-CORROBORATION-WEAK]` `MACCTL.SHM_UPPER`. Same.
const MACCTL_SHM_UPPER: u32 = 0x0000_0200;
/// `[EXT-CORROBORATION-WEAK]` `MACCTL.BE`. Same.
const MACCTL_BE: u32 = 0x0001_0000;

// ── EROM entry encodings ────────────────────────────────────────────────────────────────────────
//
// `[LEDGER]` Every mask here is recorded in `bcm4331.md` §S1b, and §S1b's own derivation is anchored
// to a word WE read off this chip: the first EROM dword `0x4BF80001` decodes under this layout to
// `mfg=0x4bf` (BCM), `id=0x800` (ChipCommon), `class=0`, `tag=CI`, `valid=1` — and under the
// alternative it decodes to the impossible `id=0xf80` with the manufacturer field sitting on top of
// the tag nibble. Boots AL/AM/AN then walked the whole ROM under this layout and produced a core list
// whose d11 entry agrees to the bit with two config registers Apple's firmware wrote. That is the
// corroboration; the ledger's stated upstream is Linux `scan.h`.

/// Bit 0 of every EROM dword: the entry is populated.
const ER_VALID: u32 = 0x0000_0001;
/// Tag field for COMPONENT / MASTER-PORT / END entries: bits 3:1.
const ER_TAG: u32 = 0x0000_000E;
/// Tag field for ADDRESS entries: bits 2:1 **only**. Bit 3 of an address descriptor is not tag — it
/// is [`AD_AG32`] — so classifying an address entry with the full `0xE` mis-reads every 64-bit-capable
/// descriptor. §S1b records this as one of the three defects that made the first walk find zero cores.
const ER_TAGX: u32 = 0x0000_0006;
const ER_TAG_CI: u32 = 0x0000_0000;
const ER_TAG_MP: u32 = 0x0000_0002;
const ER_TAG_ADDR: u32 = 0x0000_0004;
/// The end sentinel is the WHOLE word, not just its tag.
const ER_END_WORD: u32 = 0x0000_000F;
/// An all-ones read is not an entry — it is a dead window.
const ER_ALL_ONES: u32 = 0xFFFF_FFFF;

const CIA_MFG_MASK: u32 = 0xFFF0_0000;
const CIA_MFG_SHIFT: u32 = 20;
const CIA_ID_MASK: u32 = 0x000F_FF00;
const CIA_ID_SHIFT: u32 = 8;
const CIA_CLASS_MASK: u32 = 0x0000_00F0;
const CIA_CLASS_SHIFT: u32 = 4;

const CIB_NMP_MASK: u32 = 0x0000_01F0; // master ports,    bits 8:4
const CIB_NMP_SHIFT: u32 = 4;
const CIB_NSP_MASK: u32 = 0x0000_3E00; // slave ports,     bits 13:9
const CIB_NSP_SHIFT: u32 = 9;
const CIB_NMW_MASK: u32 = 0x0007_C000; // master wrappers, bits 18:14
const CIB_NMW_SHIFT: u32 = 14;
const CIB_NSW_MASK: u32 = 0x00F8_0000; // slave wrappers,  bits 19:23
const CIB_NSW_SHIFT: u32 = 19;
const CIB_REV_MASK: u32 = 0xFF00_0000;
const CIB_REV_SHIFT: u32 = 24;

const AD_ADDR_MASK: u32 = 0xFFFF_F000;
const AD_PORT_MASK: u32 = 0x0000_0F00;
const AD_PORT_SHIFT: u32 = 8;
const AD_TYPE_MASK: u32 = 0x0000_00C0;
const AD_TYPE_SHIFT: u32 = 6;
const AD_TYPE_SLAVE: u32 = 0;
const AD_TYPE_BRIDGE: u32 = 1;
const AD_TYPE_SWRAP: u32 = 2;
const AD_TYPE_MWRAP: u32 = 3;
const AD_SZ_MASK: u32 = 0x0000_0030;
/// The size lives in a following SIZE descriptor rather than in this word.
const AD_SZ_SZD: u32 = 0x0000_0030;
/// A high address dword follows. **Bit 3** — see [`ER_TAGX`].
const AD_AG32: u32 = 0x0000_0008;
/// In a SIZE descriptor, "a high size dword follows".
const SD_SG32: u32 = 0x0000_0008;

/// `[LEDGER]` Broadcom's backplane manufacturer id. Our own first EROM word carries it.
const MANUF_BCM: u16 = 0x04BF;
/// `[LEDGER]` The d11 radio core — the one id this path exists to locate. Boots AL/AM/AN found
/// exactly one, at the address firmware had independently parked the window on.
const CORE_ID_80211: u16 = 0x0812;
/// `[LEDGER]` ChipCommon, core index 0 on every AI backplane. Our own EROM's first component.
const CORE_ID_CHIPCOMMON: u16 = 0x0800;

/// Structural ceiling on the walk, in dwords, computed at the CALL SITE from the EROM's byte offset
/// within its page: `(BCMA_CORE_SIZE - erom_off) / 4`.
///
/// **It is not a constant, and the fixed `BCMA_CORE_SIZE / 4` it replaces was wrong.** The EROM
/// pointer carries a byte offset inside the selected page (`0x18107000` happens to be page-aligned,
/// so `erom_off = 0`), but nothing guarantees that: with a non-zero offset a 1024-entry cap lets the
/// cursor read `erom_off + 4092` — past the end of the 4 KiB CORE window and into the WRAPPER
/// aperture at `BAR0+0x1000`, where a walk would decode agent registers as EROM entries. The bound
/// now closes at the window edge whatever the offset is.
fn erom_max_entries(erom_off: u64) -> u32 {
    ((BCMA_CORE_SIZE as u64).saturating_sub(erom_off) / 4) as u32
}
/// How many core lines are printed. The 4331 backplane has ~8; 32 cannot be reached honestly.
/// **Adopted from `drivers/bcma.rs` verbatim** — a print budget, not a property of any silicon, which
/// is one of the reasons this file's clean-room claim was withdrawn (see the header).
const EROM_MAX_CORES: u32 = 32;

// ── Pinned metal expectations. Each is CROSS-CHECKED, never assumed ─────────────────────────────

/// `[METAL]` Boot AJ, ChipCommon `0x00` with the window on the enumeration base.
const EXPECT_CHIPID: u32 = 0x1392_4331;
/// `[METAL]` Boot AJ, ChipCommon `0xFC`.
const EXPECT_EROM: u32 = 0x1810_7000;
/// `[METAL]` Boots AL/AM/AN, three identical walks.
const EXPECT_D11_BASE: u64 = 0x1800_1000;
const EXPECT_D11_MWRAP: u64 = 0x1810_1000;
const EXPECT_D11_REV: u32 = 29;
/// `[METAL]` Boot AO, `phy-ver raw=0x9701`. Type 7 is the HT-PHY, and it is the reading that says
/// this part is outside `brcmsmac` and inside the b43/`phy_ht` lineage.
const EXPECT_PHY_TYPE: u16 = 7;
/// `[LEDGER]` Broadcom's PCI vendor id / the 4331's device id — the two-part guard on the first write.
const VENDOR_BROADCOM: u16 = 0x14E4;
const DEVID_BCM4331: u16 = 0x4331;

// ── Bounded time ────────────────────────────────────────────────────────────────────────────────

/// A wall-clock deadline in `now_cycles()` (rdtsc) units.
///
/// TSC, not `arch::ms()`: `ms()` is derived from the local-APIC tick, which advances only when the
/// APIC-timer ISR runs. This code is driven from the main loop, but the same discipline every bounded
/// wait in this kernel uses is the free-running counter, and a millisecond loop that silently stops
/// counting is a hang that looks like progress.
struct Deadline {
    t0: u64,
    budget: u64,
}

impl Deadline {
    fn new() -> Self {
        Deadline { t0: crate::arch::now_cycles(), budget: crate::arch::hw_wait_budget() }
    }
    fn expired(&self) -> bool {
        crate::arch::now_cycles().wrapping_sub(self.t0) > self.budget
    }
    fn elapsed(&self) -> u64 {
        crate::arch::now_cycles().wrapping_sub(self.t0)
    }
}

/// Cycles → whole ms at print time via the BPACE ledger's own measured rate, or raw ticks when that
/// rate is not yet known. Never fabricate a millisecond out of a guessed frequency.
fn fmt_dur(cy: u64) -> (u64, &'static str) {
    let hz = crate::bootpace::origin_hz();
    if hz >= 1000 { (cy / (hz / 1000), "ms") } else { (cy, "cy") }
}

/// Spin for at least `us` microseconds when the TSC rate is known, else for a fixed, conservative
/// cycle count. Returns the units it actually honoured so a witness can say which — a settle time
/// claimed in µs off an uncalibrated counter is a fabricated number.
fn settle(us: u64) -> (u64, &'static str) {
    let hz = crate::bootpace::origin_hz();
    let (want, unit) = if hz >= 1_000_000 { (us * (hz / 1_000_000), "us") } else { (us * 4_000, "cy") };
    let t0 = crate::arch::now_cycles();
    while crate::arch::now_cycles().wrapping_sub(t0) < want {
        core::hint::spin_loop();
    }
    (if unit == "us" { us } else { want }, unit)
}

// ── MMIO ────────────────────────────────────────────────────────────────────────────────────────

/// Read a u32 from the mapped BAR0 window.
///
/// # Safety
/// The caller must have mapped the window (`map_mmio_window`) AND proved the page present with
/// `translate()`. Every offset this file reads is a status or identity register: none is a FIFO and
/// none is read-to-clear. `GEN_IRQ_REASON` (d11+0x128) and the DMA `*_REASON` words ARE read-to-clear
/// and are deliberately never touched here — reading them would destroy state a later arc needs.
unsafe fn r32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}

/// Read a u16 from the mapped BAR0 window (the PHY identity word is 16 bits wide).
///
/// # Safety
/// As [`r32`].
unsafe fn r16(base: u64, off: u64) -> u16 {
    core::ptr::read_volatile((base + off) as *const u16)
}

/// Write a u32 into the mapped BAR0 window.
///
/// **Exactly one call site** — [`enable_core`], and only on the branch where the core did NOT arrive
/// enabled. It is deliberately not a general-purpose accessor: every other rung in this file is
/// MMIO-read-only, and a convenient `w32` in scope is how that stops being true by accident.
///
/// # Safety
/// The caller must have mapped and verified the window, must have established by a LIVE `cfg:0x80`
/// readback that BAR0+0 decodes to the d11 core (the same offset in another core's window is another
/// register entirely), and must have argued the specific register's side effects. See [`enable_core`].
unsafe fn w32(base: u64, off: u64, val: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, val);
}

// ── The EROM cursor ─────────────────────────────────────────────────────────────────────────────

/// A bounded, pushback-capable cursor over the enumeration ROM.
///
/// **The parse strategy here is tag-driven, and that is a deliberate design choice rather than a
/// transcription.** An EROM component declares PORT counts, not descriptor counts; a port carries one
/// or more descriptors and its list ends only when the next entry stops matching. A walker that
/// consumes a FIXED number of descriptors desynchronises on the first multi-descriptor port and then
/// reports plausible-looking nonsense — that is precisely the third of the three defects `bcm4331.md`
/// §S1b records.
///
/// This cursor avoids the whole class differently: after a component's identifier pair, it consumes
/// **every** master-port and address entry the tags identify, in order, until the next entry is a
/// component identifier or the end sentinel. Synchronisation therefore does not depend on the
/// declared counts at all — which frees those counts to be used as what they should be, an
/// independent CROSS-CHECK printed as `declared(...)` vs `observed(...)` on every core line. A count
/// mask that is wrong shows up as an arity MISMATCH on the wire instead of as a silent desync.
///
/// Both bounds live in the cursor rather than in the walk, so no accessor can read past them: the
/// structural `max` (from [`erom_max_entries`], computed against the EROM's offset within its page,
/// so the cursor cannot walk out of the core window and into the wrapper aperture) and a wall-clock
/// [`Deadline`]. Once either fires, `overrun` names it and every subsequent read returns `None`.
///
/// **Adopted from `drivers/bcma.rs`**: the struct and the `stopped`/`peek`/`take`/`ci`/`at_end`/
/// `master_port` accessors are that module's code, and inherit its recorded Group-B provenance. What
/// is independent here is [`Erom::address`] and the walk driver — see the file header.
struct Erom<F: Fn(u32) -> u32> {
    read: F,
    /// Cursor, in dwords from the EROM base.
    i: u32,
    /// Structural ceiling, in dwords, for THIS EROM's placement in the window.
    max: u32,
    dl: Deadline,
    /// Empty until a bound (or an all-ones read) stopped the walk; then the name of that bound.
    overrun: &'static str,
}

impl<F: Fn(u32) -> u32> Erom<F> {
    fn new(read: F, max: u32) -> Self {
        Erom { read, i: 0, max, dl: Deadline::new(), overrun: "" }
    }

    /// True once a bound has fired. This is what distinguishes "the next entry is not the kind I
    /// asked for" (a normal pushback) from "the walk cannot continue" (stop and report).
    fn stopped(&self) -> bool {
        !self.overrun.is_empty()
    }

    /// The word at the cursor WITHOUT consuming it.
    fn peek(&mut self) -> Option<u32> {
        if self.i >= self.max {
            self.overrun = "entry-cap";
            return None;
        }
        if self.dl.expired() {
            self.overrun = "tsc-deadline";
            return None;
        }
        let v = (self.read)(self.i);
        if v == ER_ALL_ONES {
            self.overrun = "all-ones";
            return None;
        }
        Some(v)
    }

    /// Consume the word at the cursor unconditionally. Used ONLY for the continuation dwords of a
    /// descriptor already accepted (a 64-bit address half, a size descriptor) — raw data that carries
    /// no tag of its own.
    fn take(&mut self) -> Option<u32> {
        let v = self.peek()?;
        self.i += 1;
        Some(v)
    }

    /// Is the cursor sitting on the end sentinel? Non-consuming.
    fn at_end(&mut self) -> bool {
        matches!(self.peek(), Some(e) if e == ER_END_WORD)
    }

    /// A component-identifier word (CIA, then CIB). Consumes on match; on a mismatch the cursor does
    /// NOT move, which is what lets the caller then test the same position for the end sentinel.
    fn ci(&mut self) -> Option<u32> {
        let ent = self.peek()?;
        if (ent & ER_VALID) == 0 || (ent & ER_TAG) != ER_TAG_CI {
            return None;
        }
        self.i += 1;
        Some(ent)
    }

    /// One master-port descriptor. Consumes on match only. Carries no address.
    fn master_port(&mut self) -> Option<u32> {
        let ent = self.peek()?;
        if (ent & ER_VALID) == 0 || (ent & ER_TAG) != ER_TAG_MP {
            return None;
        }
        self.i += 1;
        Some(ent)
    }

    /// One address descriptor of ANY type on ANY port, with its continuation dwords consumed.
    /// Returns `(type, port, base)`. `None` (cursor unmoved) when the next entry is not an address.
    fn address(&mut self) -> Option<(u32, u32, u64)> {
        let ent = self.peek()?;
        if (ent & ER_VALID) == 0 || (ent & ER_TAGX) != ER_TAG_ADDR {
            return None;
        }
        self.i += 1;
        let ty = (ent & AD_TYPE_MASK) >> AD_TYPE_SHIFT;
        let port = (ent & AD_PORT_MASK) >> AD_PORT_SHIFT;
        let mut addr = (ent & AD_ADDR_MASK) as u64;
        if (ent & AD_AG32) != 0 {
            addr |= (self.take()? as u64) << 32;
        }
        if (ent & AD_SZ_MASK) == AD_SZ_SZD {
            // The size lives in a following descriptor, which itself may be two dwords wide.
            let sz = self.take()?;
            if (sz & SD_SG32) != 0 {
                self.take()?;
            }
        }
        Some((ty, port, addr))
    }
}

/// What the walk learned about the 802.11 core.
#[derive(Clone, Copy)]
struct D11 {
    base: u64,
    /// The MASTER wrapper. **Not interchangeable with the slave wrapper**, and this distinction has
    /// already cost one wrong deliverable line: §S1c records that `erom-d11 FOUND` once advised
    /// `cfg:0xac <- 0x00000000` because it took the address from `swrap`, and this core declares
    /// `nsw=0 nmw=1`. Both are carried, separately, and the one firmware itself used is `mwrap`.
    mwrap: u64,
    swrap: u64,
    rev: u32,
    /// Did THIS component's observed descriptor counts match the ones its CIB declared?
    ///
    /// Carried out of the walk rather than merely printed, because a MISMATCH on the d11 specifically
    /// is not a cosmetic finding: the arity cross-check exists to catch a wrong CIB mask, and a wrong
    /// CIB mask means the cursor's idea of where this component's descriptors ended may be wrong,
    /// which means `base`/`mwrap` may have been taken from a NEIGHBOURING component. Opening the core
    /// window on an address obtained that way is exactly what the cross-check was built to prevent, so
    /// [`bringup_once`] refuses on it.
    arity_ok: bool,
}

fn core_name(id: u16) -> &'static str {
    match id {
        CORE_ID_CHIPCOMMON => "chipcommon",
        CORE_ID_80211 => "802.11(d11)",
        _ => "?",
    }
}

/// Walk the enumeration ROM through an already-open window, printing one line per component.
///
/// `read` yields the dword at EROM entry index `i`; it is a closure so the walk is independent of HOW
/// the window was pointed at the ROM. Returns the component count and the d11 core if one was found.
fn walk_erom<F: Fn(u32) -> u32>(read: F, max_entries: u32) -> (u32, Option<D11>) {
    let mut e = Erom::new(read, max_entries);
    let mut cores = 0u32;
    let mut arity_mismatch = 0u32;
    let mut stop = "end-tag";
    let mut d11: Option<D11> = None;

    loop {
        let cia = match e.ci() {
            Some(v) => v,
            None => {
                // The ONE clean exit: the cursor is on the end sentinel and `stop` keeps its initial
                // value. Anything else overwrites it, so a walk that ends any other way cannot be
                // mistaken for a complete one.
                if e.stopped() {
                    stop = e.overrun;
                } else if !e.at_end() {
                    stop = "not-ci";
                }
                break;
            }
        };
        let cib = match e.ci() {
            Some(v) => v,
            None => {
                stop = if e.stopped() { e.overrun } else { "cib-malformed" };
                break;
            }
        };

        let mfg = ((cia & CIA_MFG_MASK) >> CIA_MFG_SHIFT) as u16;
        let id = ((cia & CIA_ID_MASK) >> CIA_ID_SHIFT) as u16;
        let class = (cia & CIA_CLASS_MASK) >> CIA_CLASS_SHIFT;
        let rev = (cib & CIB_REV_MASK) >> CIB_REV_SHIFT;
        let nmp = (cib & CIB_NMP_MASK) >> CIB_NMP_SHIFT;
        let nsp = (cib & CIB_NSP_MASK) >> CIB_NSP_SHIFT;
        let nmw = (cib & CIB_NMW_MASK) >> CIB_NMW_SHIFT;
        let nsw = (cib & CIB_NSW_MASK) >> CIB_NSW_SHIFT;

        // Consume this component's descriptors by TAG, not by declared arity. See [`Erom`].
        let mut obs_mp = 0u32;
        let mut obs = [0u32; 4]; // observed address descriptors, indexed by AD_TYPE_*
        let mut base: u64 = 0;
        let mut mwrap: u64 = 0;
        let mut swrap: u64 = 0;
        loop {
            if e.master_port().is_some() {
                obs_mp += 1;
                continue;
            }
            match e.address() {
                Some((ty, _port, addr)) => {
                    if (ty as usize) < obs.len() {
                        obs[ty as usize] += 1;
                    }
                    match ty {
                        AD_TYPE_SLAVE if base == 0 => base = addr,
                        AD_TYPE_MWRAP if mwrap == 0 => mwrap = addr,
                        AD_TYPE_SWRAP if swrap == 0 => swrap = addr,
                        _ => {}
                    }
                }
                None => break,
            }
        }
        if e.stopped() {
            stop = e.overrun;
        }

        // The declared counts are now free to be a CROSS-CHECK rather than the parse driver. A
        // difference is a real finding about the CIB masks, and it is printed rather than folded away.
        let arity_ok = obs_mp == nmp
            && obs[AD_TYPE_MWRAP as usize] == nmw
            && obs[AD_TYPE_SWRAP as usize] == nsw
            && obs[AD_TYPE_SLAVE as usize] >= nsp.min(1);
        if !arity_ok {
            arity_mismatch += 1;
        }

        if id == CORE_ID_80211 && d11.is_none() && base != 0 {
            d11 = Some(D11 { base, mwrap, swrap, rev, arity_ok });
        }

        if cores < EROM_MAX_CORES {
            serial_println!(
                ":: wifi2: erom-core[{}] id={:#05x} ({}) mfg={:#05x}{} rev={} class={} base={:#x} mwrap={:#x} swrap={:#x} bridge={} declared(nmp={} nsp={} nmw={} nsw={}) observed(mp={} slave={} swrap={} mwrap={}) arity={} ::",
                cores, id, core_name(id), mfg, if mfg == MANUF_BCM { "(bcm)" } else { "" },
                rev, class, base, mwrap, swrap, obs[AD_TYPE_BRIDGE as usize],
                nmp, nsp, nmw, nsw,
                obs_mp, obs[AD_TYPE_SLAVE as usize], obs[AD_TYPE_SWRAP as usize],
                obs[AD_TYPE_MWRAP as usize],
                if arity_ok { "MATCH" } else { "MISMATCH" },
            );
        }
        cores += 1;
        if e.stopped() {
            break;
        }
    }

    let (ev, eu) = fmt_dur(e.dl.elapsed());
    serial_println!(
        ":: wifi2: erom-walk components={} entries={} arity-mismatches={} stop={} verdict={} elapsed={}{} — the walk is TAG-driven, so the declared port counts above are an independent cross-check and not the parse driver ::",
        cores, e.i, arity_mismatch, stop,
        if cores == 0 { "WALK-FAILED" } else { "WALK-OK" }, ev, eu
    );
    (cores, d11)
}

// ── State ───────────────────────────────────────────────────────────────────────────────────────

/// Audited write counters. Every field is incremented AT its write site, and the `end` line prints
/// the FIELDS — including the zeros. The first cut of this file printed `wrote-core-regs=0(audited)`
/// and `uploaded-bytes=0(audited)` as string literals, which is an audited zero that audits nothing:
/// the day something writes a core register, a literal keeps saying zero. These are values now, and a
/// zero on the wire is a statement the code path can be held to.
struct Writes {
    /// R2's in-place probe writes — the discriminating round-trip and its immediate undo. They move
    /// no window; counted separately so `wrote-cfg80` is never read as "the window moved 5 times".
    cfg80_selftest: u32,
    /// R3/R4/R5 window moves.
    cfg80_moves: u32,
    /// R8.
    cfg80_restore: u32,
    cfg_ac_moves: u32,
    cfg_ac_restore: u32,
    /// R6's enable writes (wrapper `RESET_CTL`/`IOCTL`).
    wrapper_writes: u32,
    /// R6's unwind writes on the STILL-DOWN path.
    wrapper_unwinds: u32,
    /// Any write to a d11 CORE-window register — `MACCTL`, `SHM_CONTROL`, `SHM_DATA`,
    /// `RADIO_CONTROL`. **No site in this file increments it**, which is the point: it is printed
    /// from the field, so the claim is checked by the compiler's own reachability rather than by a
    /// reader trusting a literal.
    core_regs: u32,
    /// Microcode bytes streamed into the core. Same argument.
    upload_bytes: u64,
}

impl Writes {
    fn new() -> Self {
        Writes {
            cfg80_selftest: 0, cfg80_moves: 0, cfg80_restore: 0,
            cfg_ac_moves: 0, cfg_ac_restore: 0,
            wrapper_writes: 0, wrapper_unwinds: 0,
            core_regs: 0, upload_bytes: 0,
        }
    }
    fn cfg80_total(&self) -> u32 {
        self.cfg80_selftest + self.cfg80_moves + self.cfg80_restore
    }
    fn cfg_ac_total(&self) -> u32 {
        self.cfg_ac_moves + self.cfg_ac_restore
    }
    fn wrapper_total(&self) -> u32 {
        self.wrapper_writes + self.wrapper_unwinds
    }
}

/// The `end` line, printed from [`Writes`]' fields on every exit path so the audited counts cannot
/// diverge between the refusal exits and the normal one.
fn end_line(dl: &Deadline, ok: bool, stage: &str, d11: &str, w: &Writes, restore: &str) {
    let (ev, eu) = fmt_dur(dl.elapsed());
    serial_println!(
        ":: wifi2: end ok={} stage={} d11={} wrote-cfg80={}(selftest={} moves={} restore={}) wrote-cfg0xac={}(moves={} restore={}) wrote-wrapper={}(enable={} unwind={}) wrote-core-regs={}(audited — MACCTL, SHM_CONTROL, SHM_DATA and RADIO_CONTROL have no write site in this file) uploaded-bytes={}(audited) restore={} elapsed={}{} ::",
        ok as u8, stage, d11,
        w.cfg80_total(), w.cfg80_selftest, w.cfg80_moves, w.cfg80_restore,
        w.cfg_ac_total(), w.cfg_ac_moves, w.cfg_ac_restore,
        w.wrapper_total(), w.wrapper_writes, w.wrapper_unwinds,
        w.core_regs, w.upload_bytes, restore, ev, eu
    );
}

/// Refuse before anything has been written, with one line and one `end` line.
fn refuse(dl: &Deadline, w: &Writes, stage: &str, reason: &str, detail: &str) {
    serial_println!(
        ":: wifi2: REFUSED stage={} reason={} {} — NOTHING written (no cfg:0x80, no cfg:0xac, no MMIO write) ::",
        stage, reason, detail
    );
    end_line(dl, false, stage, "UNREACHED", w, "N/A");
}

/// Arc 2's single pass. Called once per boot from [`crate::wifi::service`], only after arc 1's census
/// selected the radio and its firmware-staging pass has run (so `staged_count()` is final).
///
/// Forward-only and terminal: it prints one `end` line whatever happens and never retries.
pub fn bringup_once() {
    let dl = Deadline::new();
    let mut w = Writes::new();

    serial_println!(
        ":: wifi2: begin — arc 2: map BAR0, walk the EROM from our own reads, identify the d11 core, read its wrapper + core state, apply the enable rule. WRITES: cfg:0x80 (the window selector, pre-image restored) and, ONLY IF the core does not arrive enabled, a reversible RESET_CTL/IOCTL pair. The §S4 upload prologue (a core reset that DESTROYS the resident microcode) is NOT made by this arc ::"
    );

    // ── R0: preconditions, LIVE. ────────────────────────────────────────────────────────────────
    //
    // The census's `WifiCore` is re-read rather than trusted: it was captured at `S_START`, possibly
    // thousands of main-loop iterations ago, and it is about to guard a config WRITE. §S4a makes
    // exactly this correction for its own MMIO write ("a witness that says 'live' should not be
    // quoting a value captured several stages earlier"); the same rule applies a fortiori here.
    let Some(c) = super::bus::found() else {
        refuse(&dl, &w, "census", "no-radio-selected", "arc 1's census selected no subclass-0x80 Broadcom function");
        return;
    };
    if !c.cross_check_ok {
        refuse(
            &dl, &w, "census", "cross-check-failed",
            "arc 1 reported cross-check=FAIL against bcm4331.md §0; arc 2 is GATED on that check passing and will not write a window selector on a part it cannot identify",
        );
        return;
    }
    let (bus, dev, func) = (c.bus, c.slot, c.func);
    let vend = unsafe { read_config_16(bus, dev, func, 0x00) };
    let devid = unsafe { read_config_16(bus, dev, func, 0x02) };
    if vend != VENDOR_BROADCOM || devid != DEVID_BCM4331 {
        serial_println!(
            ":: wifi2: identity drifted since the census — live vendor={:#06x} device={:#06x}, census recorded {:#06x}:{:#06x} ::",
            vend, devid, c.vendor, c.device
        );
        refuse(&dl, &w, "identity", "not-4331", "cfg:0x80 is a 4331-specific backplane selector; moving it on another part is exactly the wedge this ladder avoids");
        return;
    }

    // D0 + memory decode. Arc 2 writes ONLY the window selector: waking the function (a PMCSR write)
    // and enabling decode (a COMMAND write) are OTHER writes and belong to whoever needs them. If
    // either does not already hold, moving the window would be pointless — a moved window that BAR0
    // will not answer — so refuse without writing.
    let pm = {
        let p = {
            let status = (unsafe { read_config_32(bus, dev, func, CFG_CMD_STS) } >> 16) as u16;
            if status & (1 << 4) == 0 {
                0u8
            } else {
                let mut ptr = (unsafe { read_config_32(bus, dev, func, CFG_CAP_PTR) } & 0xFC) as u8;
                let mut found = 0u8;
                let mut guard = 0u8;
                while ptr >= 0x40 && ptr != 0xFF && guard < 48 {
                    let cap = unsafe { read_config_32(bus, dev, func, ptr) };
                    if (cap & 0xFF) as u8 == CAP_ID_PM {
                        found = ptr;
                        break;
                    }
                    ptr = ((cap >> 8) & 0xFC) as u8;
                    guard += 1;
                }
                found
            }
        };
        if p <= 0xF8 { p } else { 0 }
    };
    let pstate = if pm != 0 { (unsafe { read_config_32(bus, dev, func, pm + 4) } & 0x3) as u8 } else { 0 };
    let command = (unsafe { read_config_32(bus, dev, func, CFG_CMD_STS) } & 0xFFFF) as u16;
    let mem_decode = (command & 0x0002) != 0;
    serial_println!(
        ":: wifi2: precond {:02x}:{:02x}.{} vendor={:#06x} device={:#06x} d-state={} mem-decode={} bus-master={} cmd={:#06x} bar0={:#018x} kind={} — LIVE config re-read, not the census's cached copy ::",
        bus, dev, func, vend, devid, pstate, mem_decode as u8,
        ((command & 0x0004) != 0) as u8, command, c.bar0,
        if c.bar0_io { "io" } else if c.bar0_64 { "mem64" } else { "mem32" },
    );
    if pstate != 0 || !mem_decode {
        refuse(
            &dl, &w, "precond", if pstate != 0 { "not-d0" } else { "mem-decode-off" },
            "arc 2 writes only cfg:0x80; waking (PMCSR) or enabling decode (COMMAND) are OTHER writes and not this arc's",
        );
        return;
    }
    if c.bar0 == 0 || c.bar0_io {
        refuse(
            &dl, &w, "bar0", if c.bar0 == 0 { "bar0-unassigned" } else { "bar0-is-io" },
            "no memory BAR0 to reach the backplane through",
        );
        return;
    }
    let bar0 = c.bar0;

    // ── R1: map BAR0. A page-table edit; the device sees nothing. ───────────────────────────────
    crate::arch::memory::map_mmio_window(bar0, BAR0_MAP_LEN);
    match crate::arch::memory::translate(bar0) {
        Some(pa) => serial_println!(
            ":: wifi2: map bar0={:#x} len={:#x} translate=ok pa={:#x} — two 4 KiB apertures: BAR0+0 is the core window (cfg:0x80), BAR0+{:#x} the wrapper window (cfg:0xac). A page-table edit only; the device sees nothing ::",
            bar0, BAR0_MAP_LEN, pa, BAR0_WRAP_OFF
        ),
        None => {
            refuse(&dl, &w, "map", "bar0-unmapped", "window not present after map_mmio_window");
            return;
        }
    }

    // ── R2: the pre-image, and PROVE the unwind before moving anything. ─────────────────────────
    //
    // The pre-image on THIS machine is `0x18001000`, NOT the enumeration base. A stage that
    // "restored" to `0x18000000` because that is what it assumed firmware left would silently move
    // the radio's window for every later boot stage and look clean in every log line. The restore
    // pushes the value READ; `0x18000000` appears nowhere below as a restore target.
    let pre_win = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let pre_win2 = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN2) };
    serial_println!(
        ":: wifi2: pre-image cfg:0x80={:#010x} cfg:0xac={:#010x} — the 0x80 value is the ONE restore target (on this machine 0x18001000, the d11 core, NOT the enumeration base 0x18000000) ::",
        pre_win, pre_win2
    );

    // The self-test, and it must be DISCRIMINATING. The first cut of this file wrote the pre-image
    // back to itself and checked the readback — which a config-write path that DROPS EVERY WRITE
    // passes, because the register still holds the pre-image. A self-test that cannot fail is exactly
    // the instrument defect this tree keeps finding in its own gates.
    //
    // The probe value is `pre_win | 0xFFF`. The window selector is 4 KiB-granular — hardware ignores
    // its low 12 bits when decoding — so this value selects the SAME backplane page as the pre-image
    // and the window provably cannot move, while being a DIFFERENT 32-bit word from the one the
    // register already holds. Then the pre-image is pushed back and verified.
    //
    // Two outcomes are honest, and they are reported separately:
    //   * readback == pre_win | 0xFFF  => the write path demonstrably took. `discriminating=1`.
    //   * readback == pre_win          => this silicon MASKS the low 12 bits on write-back. The probe
    //                                     could not distinguish "write took" from "write dropped", and
    //                                     no other value we may write leaves the window unmoved. Not a
    //                                     failure — an ADVISORY, printed as `discriminating=0`, so a
    //                                     reader knows the round-trip below is the weaker claim.
    // Anything else, or a final readback that is not the pre-image, is a hard refusal.
    let probe = pre_win | 0xFFF;
    unsafe { write_config_32(bus, dev, func, CFG_BAR0_WIN, probe) };
    w.cfg80_selftest += 1;
    let probe_back = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    unsafe { write_config_32(bus, dev, func, CFG_BAR0_WIN, pre_win) };
    w.cfg80_selftest += 1;
    let selftest = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };

    let discriminating = probe_back == probe;
    let probe_sane = discriminating || probe_back == pre_win;
    if !probe_sane || selftest != pre_win {
        serial_println!(
            ":: wifi2: REFUSED stage=unwind-selftest reason={} probe-wrote={:#010x} probe-readback={:#010x} restore-wrote={:#010x} restore-readback={:#010x} — the config-write path does NOT round-trip; cfg:0x80 has NOT been moved off the firmware pre-image's page ::",
            if !probe_sane { "probe-readback-unrecognised" } else { "restore-readback-mismatch" },
            probe, probe_back, pre_win, selftest
        );
        end_line(&dl, false, "unwind-selftest", "UNREACHED", &w, "N/A");
        return;
    }
    serial_println!(
        ":: wifi2: unwind-selftest PASS discriminating={} probe={:#010x} probe-readback={:#010x} restore-readback={:#010x} — the probe differs from the pre-image only in the low 12 bits the selector ignores, so the window provably did NOT move{} ::",
        discriminating as u8, probe, probe_back, selftest,
        if discriminating {
            ", and the readback proves the write path took"
        } else {
            "; this silicon masks those bits on readback, so the write path is NOT proven — advisory, see the code comment"
        }
    );

    // From HERE the window WILL move. There are NO early returns between this point and the restore
    // at R8: R3-R7 live in `explore`, whose every exit is a RETURN TO HERE, so the restore below is
    // unconditional by construction rather than by careful reading.
    let (ok, stage, d11_state) = explore(bus, dev, func, bar0, pre_win, pre_win2, &mut w);

    // ── R8: RESTORE. Push the value READ at R2, and verify against THAT value. ──────────────────
    unsafe { write_config_32(bus, dev, func, CFG_BAR0_WIN, pre_win) };
    w.cfg80_restore += 1;
    let back = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let restored = back == pre_win;
    serial_println!(
        ":: wifi2: RESTORE cfg:0x80 <- pre-image {:#010x} readback={:#010x} restored={} — compared against the RECORDED pre-image, never against 0x18000000 ::",
        pre_win, back, if restored { "MATCH" } else { "FAILED" }
    );
    let mut restored_all = restored;
    if w.cfg_ac_moves > 0 {
        unsafe { write_config_32(bus, dev, func, CFG_BAR0_WIN2, pre_win2) };
        w.cfg_ac_restore += 1;
        let back2 = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN2) };
        restored_all &= back2 == pre_win2;
        serial_println!(
            ":: wifi2: RESTORE cfg:0xac <- pre-image {:#010x} readback={:#010x} restored={} ::",
            pre_win2, back2, if back2 == pre_win2 { "MATCH" } else { "FAILED" }
        );
    }

    end_line(&dl, ok, stage, d11_state, &w, if restored_all { "MATCH" } else { "FAILED" });
}

/// R3-R7, between the pre-image and the restore.
///
/// Every exit from this function is a plain `return`, and its only caller performs the restore
/// immediately afterwards — which is why none of the refusals below has to remember to unwind the
/// window. Returns `(ok, stage, d11_state)` for the `end` line.
#[allow(clippy::too_many_arguments)]
fn explore(
    bus: u8, dev: u8, func: u8, bar0: u64, pre_win: u32, pre_win2: u32, w: &mut Writes,
) -> (bool, &'static str, &'static str) {
    // ── R3: ChipCommon. ─────────────────────────────────────────────────────────────────────────
    unsafe { write_config_32(bus, dev, func, CFG_BAR0_WIN, BCMA_ADDR_BASE) };
    w.cfg80_moves += 1;
    let win_cc = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let cc_took = win_cc == BCMA_ADDR_BASE;
    serial_println!(
        ":: wifi2: WROTE cfg:0x80 pre={:#010x} new={:#010x} readback={:#010x} took={} — window now on ChipCommon ::",
        pre_win, BCMA_ADDR_BASE, win_cc, cc_took as u8
    );

    // `took=` is CHECKED, not merely printed. R5 already refused on its own selector readback, and a
    // file that enforces its most important invariant in one place while only printing it in two
    // others is a file whose reader cannot tell which lines are load-bearing.
    if !cc_took {
        serial_println!(
            ":: wifi2: REFUSED stage=chipcommon reason=selector-did-not-take readback={:#010x} want={:#010x} — BAR0+0 is not ChipCommon, so +0x00/+0xFC would be some other core's registers. No ChipCommon value is reported and no address derived from one is written ::",
            win_cc, BCMA_ADDR_BASE
        );
        return (false, "chipcommon", "UNREACHED");
    }

    let chipid = unsafe { r32(bar0, 0x0000) };
    let erom = unsafe { r32(bar0, 0x00FC) };
    // RAW first, decode second — a decode bug must never hide the evidence it came from.
    let chipid_ok = chipid == EXPECT_CHIPID;
    serial_println!(
        ":: wifi2: cc-raw chipid={:#010x} expected={:#010x} {} erom={:#010x} expected={:#010x} {} ::",
        chipid, EXPECT_CHIPID, if chipid_ok { "MATCH" } else { "MISMATCH" },
        erom, EXPECT_EROM, if erom == EXPECT_EROM { "MATCH" } else { "MISMATCH" },
    );

    if chipid == ER_ALL_ONES {
        // The window is on the enumeration base and ChipCommon STILL reads all-ones. Moving the
        // window was not the blocker; blame is upstream (the PCIe link, or the bridge above not
        // forwarding this address). A finding, not a silent pass.
        serial_println!(
            ":: wifi2: REFUTED window-hypothesis — cfg:0x80 reads back {:#010x} (ChipCommon base) yet BAR0+0 reads 0xffffffff. Restoring and reporting; the fault is upstream of the selector ::",
            win_cc
        );
        return (false, "chipcommon", "UNREACHED");
    }
    if !chipid_ok {
        // This MISMATCH was PRINTED and then DISCARDED in the first cut of this file: the code went on
        // to read BAR0+0xFC, call it an EROM pointer, and WRITE the derived address into cfg:0x80.
        // But `chipid != 0x13924331` means BAR0+0 is not the ChipCommon this ledger describes — a
        // different part, a different silicon revision, or a window that landed elsewhere — and on any
        // of those `+0xFC` is not an enumeration-ROM pointer. Deriving a selector value from it is
        // pointing the window at an address a register we cannot identify handed us.
        //
        // The chip id is 32 bits of structure (id/rev/pkg/nrcores/type), so the test is sharp, and the
        // fields are printed so a reader can see WHICH one moved.
        serial_println!(
            ":: wifi2: REFUSED stage=chipcommon reason=chipid-mismatch chipid={:#010x} expected={:#010x} id[15:0]={:#06x} rev[19:16]={} pkg[23:20]={} type[31:28]={} — BAR0+0 is not the ChipCommon bcm4331.md §0 pinned, so BAR0+0xFC is NOT known to be an EROM pointer and NO address derived from it is written into the selector ::",
            chipid, EXPECT_CHIPID, (chipid & 0xFFFF) as u16,
            (chipid >> 16) & 0xF, (chipid >> 20) & 0xF, (chipid >> 28) & 0xF,
        );
        return (false, "chipcommon", "UNREACHED");
    }

    serial_println!(
        ":: wifi2: cc-decode id[15:0]={:#06x} rev[19:16]={} pkg[23:20]={} nrcores[27:24]={}(SB-era CoreCount — ADVISORY; on a socitype-1 part the EROM is the only authority) type[31:28]={} ({}) is-4331={} ::",
        (chipid & 0xFFFF) as u16, (chipid >> 16) & 0xF, (chipid >> 20) & 0xF,
        (chipid >> 24) & 0xF, (chipid >> 28) & 0xF,
        match (chipid >> 28) & 0xF { 0 => "ssb/sb", 1 => "bcma/erom", 2 => "bcma-single", _ => "?" },
        ((chipid & 0xFFFF) as u16 == DEVID_BCM4331) as u8,
    );

    // ── R4: the EROM. ───────────────────────────────────────────────────────────────────────────
    if erom == 0 || erom == ER_ALL_ONES {
        serial_println!(
            ":: wifi2: erom-pointer UNUSABLE erom={:#010x} — ChipCommon answers but names no enumeration ROM; the core inventory is NOT known and will not be guessed ::",
            erom
        );
        return (false, "erom", "UNREACHED");
    }
    // The selector is 4 KiB-granular (its low 12 bits are ignored by hardware) and the EROM pointer
    // carries a byte offset within that page, so the page goes in the register and the offset is
    // applied to the read — and to the walk's structural bound, see [`erom_max_entries`].
    let erom_base = erom & 0xFFFF_F000;
    let erom_off = (erom & 0x0000_0FFF) as u64;
    let max_entries = erom_max_entries(erom_off);
    unsafe { write_config_32(bus, dev, func, CFG_BAR0_WIN, erom_base) };
    w.cfg80_moves += 1;
    let win_er = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let er_took = win_er == erom_base;
    serial_println!(
        ":: wifi2: WROTE cfg:0x80 pre={:#010x} new={:#010x} readback={:#010x} took={} — window now on the EROM (page {:#010x} + offset {:#x}, max-entries={} to the core-window edge); walking READ-ONLY ::",
        BCMA_ADDR_BASE, erom_base, win_er, er_took as u8, erom_base, erom_off, max_entries
    );
    if !er_took {
        // Same rule as R3 and R5. A walk over a window that is not on the EROM decodes whatever core
        // IS selected as an entry list, and every address it then reports would be fiction handed
        // straight back to the selector.
        serial_println!(
            ":: wifi2: REFUSED stage=erom reason=selector-did-not-take readback={:#010x} want={:#010x} — the window is not on the enumeration ROM; no walk is performed and no core address is reported ::",
            win_er, erom_base
        );
        return (false, "erom", "UNREACHED");
    }

    let (_components, d11) =
        walk_erom(|i| unsafe { r32(bar0, erom_off + (i as u64) * 4) }, max_entries);

    let Some(d) = d11 else {
        serial_println!(
            ":: wifi2: d11 ABSENT — no component id={:#05x} (802.11) with a slave address in the inventory above; the radio's backplane base is NOT known and must not be guessed ::",
            CORE_ID_80211
        );
        return (false, "erom", "ABSENT");
    };

    // The sharp cross-check, between two ENTIRELY independent sources: a pair of config registers
    // Apple's firmware wrote before we existed, and an on-chip ROM we walked ourselves. Agreement to
    // the bit is what makes the walk a measurement rather than a plausible-looking decode.
    let base_ok = d.base == EXPECT_D11_BASE;
    let mwrap_ok = d.mwrap == EXPECT_D11_MWRAP;
    let rev_ok = d.rev == EXPECT_D11_REV;
    serial_println!(
        ":: wifi2: d11 FOUND id={:#05x} rev={} expected={} {} base={:#010x} expected={:#010x} {} mwrap={:#010x} expected={:#010x} {} swrap={:#010x} arity={} ::",
        CORE_ID_80211, d.rev, EXPECT_D11_REV, if rev_ok { "MATCH" } else { "MISMATCH" },
        d.base, EXPECT_D11_BASE, if base_ok { "MATCH" } else { "MISMATCH" },
        d.mwrap, EXPECT_D11_MWRAP, if mwrap_ok { "MATCH" } else { "MISMATCH" },
        d.swrap, if d.arity_ok { "MATCH" } else { "MISMATCH" },
    );
    serial_println!(
        ":: wifi2: d11 cross-check base-vs-cfg0x80-preimage={} ({:#010x} vs {:#010x}) mwrap-vs-cfg0xac-preimage={} ({:#010x} vs {:#010x}) — a config register FIRMWARE wrote versus an on-chip ROM WE walked; agreement means the walk measured the backplane rather than decoded a coincidence ::",
        if d.base == pre_win as u64 { "MATCH" } else { "MISMATCH" }, d.base, pre_win,
        if d.mwrap == pre_win2 as u64 { "MATCH" } else { "MISMATCH" }, d.mwrap, pre_win2,
    );

    // ALL FOUR conditions gate the core window, and two of them were defects in the first cut:
    //
    //   * `rev_ok` / `base_ok` were checked (they always were);
    //   * `mwrap_ok` was COMPUTED AND PRINTED AND THEN IGNORED — so a d11 whose master wrapper
    //     disagreed with four metal boots still got cfg:0xac pointed at that address and both R6
    //     writes issued through the resulting aperture. The one MISMATCH that leads DIRECTLY to a
    //     backplane write was the one the code did not act on;
    //   * `arity_ok` was printed per component and never consulted — but an arity MISMATCH means the
    //     cursor's idea of where this component ended may be wrong, so `base`/`mwrap` may have been
    //     read out of a NEIGHBOURING component's descriptors. An address obtained that way is exactly
    //     what the cross-check exists to keep out of the selector.
    if !(rev_ok && base_ok && mwrap_ok && d.arity_ok) {
        let why = if !rev_ok {
            "rev-mismatch"
        } else if !base_ok {
            "base-mismatch"
        } else if !mwrap_ok {
            "mwrap-mismatch"
        } else {
            "arity-mismatch"
        };
        serial_println!(
            ":: wifi2: d11 REFUSED reason={} — the core window is NOT opened and cfg:0xac is NOT pointed at this component. Reading d11+0x120 on the wrong core reads another register entirely, and writing its wrapper writes another core's reset control ::",
            why
        );
        return (false, "d11", "FOUND");
    }

    // ── R5/R6/R7 ────────────────────────────────────────────────────────────────────────────────
    (reach_d11(bus, dev, func, bar0, &d, pre_win2, w), "d11", "FOUND")
}

/// R5-R7: point the window at the d11 core, read its state, apply the enable rule, and decide about
/// the upload. Returns whether the core ended reachable and enabled.
///
/// The caller has already established that the EROM's d11 identity matches four metal boots. This
/// function opens the core window; every read below is of a status or identity register.
fn reach_d11(bus: u8, dev: u8, func: u8, bar0: u64, d: &D11, pre_win2: u32, w: &mut Writes) -> bool {
    unsafe { write_config_32(bus, dev, func, CFG_BAR0_WIN, d.base as u32) };
    w.cfg80_moves += 1;
    let win = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    serial_println!(
        ":: wifi2: WROTE cfg:0x80 new={:#010x} readback={:#010x} took={} — window now on the RADIO core; BAR0+0 is d11 register 0 ::",
        d.base as u32, win, (win == d.base as u32) as u8
    );
    if win != d.base as u32 {
        serial_println!(
            ":: wifi2: REFUSED stage=selector reason=did-not-take — the core window is not on the d11 base, so d11+0x120 would be another core's register. No core read is reported and NOTHING is written ::"
        );
        return false;
    }

    // ── R5a: the core window. ───────────────────────────────────────────────────────────────────
    let macctl = unsafe { r32(bar0, D11_MACCTL) };
    if macctl == ER_ALL_ONES {
        serial_println!(
            ":: wifi2: REFUSED stage=core reason=core-window-dark macctl=0xffffffff — the selector took but the core window reads all-ones; nothing is written into a window that is not answering ::"
        );
        return false;
    }
    let phy = unsafe { r16(bar0, D11_PHY_VER) };
    let phy_type = (phy >> 8) & 0xF;
    let tsf_lo = unsafe { r32(bar0, D11_TSF_LOW) };
    let tsf_hi = unsafe { r32(bar0, D11_TSF_HIGH) };
    serial_println!(
        ":: wifi2: core-pre macctl={:#010x} psm-run={} psm-jmp0={} shm-enabled={} shm-upper={} big-endian={} tsf={:#010x}:{:#010x} ::",
        macctl,
        ((macctl & MACCTL_PSM_RUN) != 0) as u8, ((macctl & MACCTL_PSM_JMP0) != 0) as u8,
        ((macctl & MACCTL_SHM_ENABLED) != 0) as u8, ((macctl & MACCTL_SHM_UPPER) != 0) as u8,
        ((macctl & MACCTL_BE) != 0) as u8, tsf_hi, tsf_lo,
    );
    serial_println!(
        ":: wifi2: core-pre phy-ver raw={:#06x} analog={} type={} expected={} {} rev={} — type 7 is the HT-PHY, the reading that puts this part outside brcmsmac and inside the b43/phy_ht lineage ::",
        phy, (phy >> 12) & 0xF, phy_type, EXPECT_PHY_TYPE,
        if phy_type == EXPECT_PHY_TYPE { "MATCH" } else { "MISMATCH" }, phy & 0xF,
    );

    // ── R5b: the wrapper aperture. ──────────────────────────────────────────────────────────────
    //
    // The wrapper is reached through BAR0+0x1000, whose backplane address is cfg:0xac. Firmware left
    // that register on this core's MASTER wrapper, and the EROM independently reported the same
    // address — so on this machine the aperture needs NO write. It is CHECKED, not assumed: pointing
    // that aperture at the radio when it is somewhere else is a second config write, and this arc
    // makes it only when it must, with the pre-image recorded for the restore.
    let want_wrap = d.mwrap;
    if want_wrap == 0 {
        serial_println!(
            ":: wifi2: REFUSED stage=wrapper reason=no-master-wrapper — the EROM reports no master wrapper for the d11 core (swrap={:#x}); the reset/clock registers are NOT guessed onto the slave wrapper ::",
            d.swrap
        );
        return false;
    }
    let live_ac = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN2) };
    if live_ac as u64 != want_wrap {
        unsafe { write_config_32(bus, dev, func, CFG_BAR0_WIN2, want_wrap as u32) };
        w.cfg_ac_moves += 1;
        let back = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN2) };
        serial_println!(
            ":: wifi2: WROTE cfg:0xac pre={:#010x} new={:#010x} readback={:#010x} took={} — firmware had the wrapper aperture elsewhere; pre-image {:#010x} is restored at the end ::",
            live_ac, want_wrap as u32, back, (back as u64 == want_wrap) as u8, pre_win2
        );
        if back as u64 != want_wrap {
            serial_println!(
                ":: wifi2: REFUSED stage=wrapper reason=cfg0xac-did-not-take — no agent register is read or written through an aperture that is not where it was pointed ::"
            );
            return false;
        }
    } else {
        serial_println!(
            ":: wifi2: wrapper aperture cfg:0xac={:#010x} == the EROM's master wrapper {:#010x} — firmware's own value, NOT written by us; agent registers are read at BAR0+{:#x} ::",
            live_ac, want_wrap as u32, BAR0_WRAP_OFF
        );
    }

    let ioctl = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
    let iost = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOST) };
    let rctl = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_RESET_CTL) };
    let rst = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_RESET_ST) };
    serial_println!(
        ":: wifi2: wrapper-pre ioctl={:#010x} iost={:#010x} resetctl={:#010x} resetst={:#010x} ::",
        ioctl, iost, rctl, rst
    );

    // ── R6: the enable rule, re-measured on THIS boot. ──────────────────────────────────────────
    //
    // Two tests and no other bit: `(IOCTL & (CLK|FGC)) == CLK`, and `RESET_CTL.RESET` clear. Four
    // metal boots have printed a wrapper that satisfies both, which is why §S3 concluded reachability
    // is a no-op on this part AS IT ARRIVES. That conclusion is a MEASUREMENT and is re-taken here
    // rather than assumed: a cold power cycle that skips the EFI AirPort init, a different card, or a
    // firmware revision that leaves the radio down all print the other verdict.
    let clk_ok = (ioctl & (IOCTL_CLK | IOCTL_FGC)) == IOCTL_CLK;
    let not_reset = (rctl & RESET_CTL_RESET) == 0;
    let enabled = clk_ok && not_reset;
    serial_println!(
        ":: wifi2: enable-rule (ioctl&(CLK|FGC))={:#06x} want={:#06x} match={} (resetctl&RESET)={:#x} clear={} => core-enabled={} ::",
        ioctl & (IOCTL_CLK | IOCTL_FGC), IOCTL_CLK, clk_ok as u8,
        rctl & RESET_CTL_RESET, not_reset as u8, enabled as u8
    );

    if enabled {
        serial_println!(
            ":: wifi2: reachability=SATISFIED(no-write) enable-writes-made=0 — the core arrives out of reset and clocked, so bringing it out of reset costs ZERO device writes. Every read above was taken WITHOUT them, which is the corroboration ::"
        );
    } else {
        // The core did NOT arrive enabled. This is the branch §S3 calls the alarm, and it is the only
        // branch on which this arc writes a backplane register.
        //
        // What is written, and why each is reversible: RESET_CTL's RESET bit is DEASSERTED (a bit we
        // clear, and re-asserting it restores the pre-image exactly), and IOCTL's CLK bit is SET with
        // FGC cleared — leaving every other IOCTL bit exactly as read, so the pre-image is one
        // masked write away. Neither is the destructive direction: this arc never ASSERTS reset, and
        // asserting reset is what would clear PSM_RUN and destroy resident microcode. The pre-images
        // are on the wire so the write can be undone by hand from the capture alone.
        serial_println!(
            ":: wifi2: reachability=REQUIRED — the core did NOT arrive enabled (§S3's alarm). Making the REVERSIBLE half only: deassert RESET_CTL.RESET and set IOCTL.CLK with FGC clear, every other bit left as read. This arc NEVER asserts reset; asserting it clears PSM_RUN and destroys resident microcode ::"
        );

        let new_rctl = rctl & !RESET_CTL_RESET;
        unsafe { w32(bar0, BAR0_WRAP_OFF + WRAP_RESET_CTL, new_rctl) };
        w.wrapper_writes += 1;
        let (sv, su) = settle(1);
        let post_rctl = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_RESET_CTL) };
        serial_println!(
            ":: wifi2: enable-write 1/2 RESET_CTL pre={:#010x} wrote={:#010x} post={:#010x} took={} settle={}{} — unwind target: RESET_CTL <- {:#010x} ::",
            rctl, new_rctl, post_rctl, (post_rctl == new_rctl) as u8, sv, su, rctl
        );

        let new_ioctl = (ioctl | IOCTL_CLK) & !IOCTL_FGC;
        unsafe { w32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL, new_ioctl) };
        w.wrapper_writes += 1;
        let (sv, su) = settle(1);
        let post_ioctl = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
        serial_println!(
            ":: wifi2: enable-write 2/2 IOCTL pre={:#010x} wrote={:#010x} post={:#010x} took={} settle={}{} — unwind target: IOCTL <- {:#010x} ::",
            ioctl, new_ioctl, post_ioctl, (post_ioctl == new_ioctl) as u8, sv, su, ioctl
        );

        let v_clk = (post_ioctl & (IOCTL_CLK | IOCTL_FGC)) == IOCTL_CLK;
        let v_res = (post_rctl & RESET_CTL_RESET) == 0;
        serial_println!(
            ":: wifi2: enable-verify (ioctl&(CLK|FGC))={:#06x} (resetctl&RESET)={:#x} => core-enabled={} verdict={} — the rule is re-evaluated on the POST-write readbacks, never on what we intended to write ::",
            post_ioctl & (IOCTL_CLK | IOCTL_FGC), post_rctl & RESET_CTL_RESET,
            (v_clk && v_res) as u8,
            if v_clk && v_res { "ENABLED" } else { "STILL-DOWN" }
        );
        if !(v_clk && v_res) {
            // THE UNWIND. The first cut of this file had none: it printed "reversible: IOCTL <- …"
            // and then returned, leaving the wrapper half-written with the undo instructions on the
            // wire for a human to perform. A write is not reversible because its pre-image was
            // printed; it is reversible because something reverses it.
            //
            // Direction matters and is worth stating, because re-asserting `RESET_CTL.RESET` looks
            // superficially like the destructive write this arc refuses to make. It is not.
            // `RESET_CTL.RESET` was found SET on this branch — firmware itself left it that way — so
            // putting it back is RESTORATION to the state the machine handed us, not the §S4 prologue
            // that asserts reset on a RUNNING core to clear `PSM_RUN`. There is no resident microcode
            // to destroy here: the core arrived in reset, which is exactly why this branch ran.
            //
            // IOCTL is restored first, then RESET_CTL, so the core is not re-held in reset while its
            // clock configuration is still ours — the reverse of the order the writes were made in.
            unsafe { w32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL, ioctl) };
            w.wrapper_unwinds += 1;
            let (usv, usu) = settle(1);
            let un_ioctl = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
            serial_println!(
                ":: wifi2: enable-unwind 1/2 IOCTL pre={:#010x} wrote={:#010x} post={:#010x} restored={} settle={}{} ::",
                post_ioctl, ioctl, un_ioctl, if un_ioctl == ioctl { "MATCH" } else { "FAILED" }, usv, usu
            );

            unsafe { w32(bar0, BAR0_WRAP_OFF + WRAP_RESET_CTL, rctl) };
            w.wrapper_unwinds += 1;
            let (usv, usu) = settle(1);
            let un_rctl = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_RESET_CTL) };
            serial_println!(
                ":: wifi2: enable-unwind 2/2 RESET_CTL pre={:#010x} wrote={:#010x} post={:#010x} restored={} settle={}{} — re-asserting a bit FIRMWARE had asserted is RESTORATION, not the §S4 prologue: that prologue resets a RUNNING core to clear PSM_RUN, and this core arrived in reset, which is why this branch ran at all ::",
                post_rctl, rctl, un_rctl, if un_rctl == rctl { "MATCH" } else { "FAILED" }, usv, usu
            );

            serial_println!(
                ":: wifi2: REFUSED stage=enable reason=still-down — the enable writes did not take; the wrapper has been UNWOUND to the words it was found with (ioctl={:#010x} resetctl={:#010x}), unwind-verified={} ::",
                ioctl, rctl,
                if un_ioctl == ioctl && un_rctl == rctl { "MATCH" } else { "FAILED" }
            );
            return false;
        }
    }

    // ── R7: the upload. ─────────────────────────────────────────────────────────────────────────
    upload_gate(macctl, w);
    true
}

/// R7 — the microcode upload, and the three gates it does not pass.
///
/// This function makes NO device access. It is the honest accounting of why arc 2 stops here, and it
/// is written as code rather than as a comment so that a boot capture carries the reason.
///
/// WIFI-SETVAL (GR27) inserted gate 2 — full-set validation — between the completeness gate and the
/// routing refusal. It is the consumption rung the loader could not perform even WITH the files
/// present: before it, the initvals and bsinitvals images were staged and then never examined by
/// anything, the dry-run covered the ucode alone, and no verdict existed for arc 3's upload to gate
/// on. The rung is deliberately a DRY-RUN — the two facts that would let bytes move (the
/// `B43_SHM_UCODE` routing value, gate 3, and the initvals RECORD format, which `bcm4331.md` §S4
/// does not describe beyond "register init table") are both unpinned for this module, so the largest
/// verifiable rung is validating the whole set against the facts §S4 DOES pin and recording the
/// verdict the upload arc must consume.
fn upload_gate(macctl: u32, w: &Writes) {
    let staged = super::firmware::staged_count();
    let want = super::firmware::FW_SET_LEN;

    if staged != want {
        // Gate 1 — and note WHICH write this gate is protecting. It is not the upload that is
        // dangerous to skip; it is the PROLOGUE that would precede it.
        serial_println!(
            ":: wifi2: upload SKIPPED reason=firmware-set-incomplete staged={}/{} — and the §S4 PROLOGUE is skipped with it. That prologue asserts RESET_CTL.RESET, which clears MACCTL.PSM_RUN (currently {}) and DESTROYS the microcode this radio arrives running; bcm4331.md §5 risk 4 records that only a SUCCESSFUL upload restores a working state. A destructive unrecoverable write whose justification is an upload that cannot happen is not made. Parked after the core walk ::",
            staged, want, ((macctl & MACCTL_PSM_RUN) != 0) as u8
        );
        return;
    }

    // Gate 2 — WIFI-SETVAL: the set is on the media; validate ALL of it before anything else is
    // even discussed. A hard park here outranks the routing refusal below on purpose: the day the
    // routing IS pinned, this gate is most of what stands between a corrupt or misclassified set and
    // a stream into the core — with one named residual it cannot see: a corruption that preserves
    // the type byte, the declared size AND the shape rule (whole words, or a count-matching record
    // walk) passes; the FNV digest reports it only across boots, never against ground truth. The
    // §S4 handshake — the UCODEREV echo after upload — is the backstop that catches what this
    // dry-run cannot. It is stream into the core — never a blind push of unvalidated bytes.
    if !validate_set() {
        serial_println!(
            ":: wifi2: upload REFUSED reason=set-validation-failed — HARD PARK: no byte of an unvalidated or ambiguous set is ever streamed at the core, whatever the routing turns out to be, and the §S4 PROLOGUE (the reset that destroys the resident microcode) is skipped with it. The fix is on the media: re-extract the set with b43-fwcutter and re-stage the stick (see wifi_bcma.md, \"The bench round\") ::"
        );
        upload_not_attempted(w);
        return;
    }

    // Gate 3 — the set is present AND valid, and the arc still stops, at a NAMED UNKNOWN.
    //
    // §S4's upload is `SHM_CONTROL <- (B43_SHM_UCODE << 16) | 0` then a stream into `SHM_DATA`. The
    // numeric value of that routing selector is in no source this module may use (see the file
    // header). §S4a's safety argument for touching SHM_CONTROL rests entirely on never writing the
    // data port — a wrong routing then costs a wrong number on a witness line. An upload inverts
    // that: a wrong routing streams tens of kilobytes into whatever bank was actually selected.
    serial_println!(
        ":: wifi2: upload REFUSED reason=shm-ucode-routing-UNPINNED — bcm4331.md §S4 names the selector B43_SHM_UCODE but records no VALUE for it; this tree carries only SHM_ROUTE_SHARED=0x0001 (the READ routing S4a uses) and no capture of ours has measured the ucode routing. The only source that carries it is b43 driver source, which CLEAN_ROOM_POLICY §2 puts off-limits for src/wifi/. A wrong routing on a READ costs a wrong number (§S4a); a wrong routing on a 90 KB STREAM into SHM_DATA writes that stream into whatever bank was selected. NOT guessed. What settles it: the value from a source legal for this module, or a metal probe that identifies the routing read-only ::"
    );
    upload_not_attempted(w);
}

/// WVAL-REPLAY — the census-ABSENT leg. Runs the set-completeness gate and [`validate_set`] and
/// nothing else, then prints one terminal park line.
///
/// This is the whole of arc 2 that does not need a radio, and it is written as its OWN function
/// rather than as a flag threaded through [`bringup_once`] for a reason a reviewer can check
/// mechanically: there is no PCI config read, no `map_mmio_window`, no `r32`/`w32`, no [`Writes`]
/// and no call to [`explore`] anywhere in this body, so "no device rung ran" is a property of the
/// call graph rather than of a branch someone has to trust. The companion spec
/// (`scripts/specs/x86-wifival.spec`) asserts the same fact from the other side, by FORBIDding the
/// witness lines every device rung prints.
///
/// The caller ([`super::finish_and_park`]) routes here only when [`super::bus::census`] came up
/// ABSENT, which on metal never happens — so this function is unreachable on the machine the radio
/// is in, and `bringup_once` keeps that path byte-for-byte.
#[cfg(feature = "wifival")]
pub fn validate_replay() {
    let staged = super::firmware::staged_count();
    let want = super::firmware::FW_SET_LEN;

    serial_println!(
        ":: wifi: wifival REPLAY begin — the radio is ABSENT, so arc 2's device ladder (BAR0 map, cfg:0x80 window moves, EROM walk, core/wrapper reads, the enable rule) is NOT entered. What DOES run is the part that never needed silicon: the set-completeness gate and the full set-validation dry-run over the staged bytes ::"
    );

    // Gate 1, verbatim in intent with `upload_gate`'s: an incomplete set has nothing to validate,
    // and the disagreement is the finding. No MACCTL here — reading it would be a device access,
    // which is exactly what this leg does not do.
    if staged != want {
        serial_println!(
            ":: wifi: wifival REPLAY set-validation SKIPPED reason=firmware-set-incomplete staged={}/{} — the media under this boot does not carry the full set, so there is nothing for the dry-run to walk ::",
            staged, want
        );
        serial_println!(
            ":: wifi: wifival REPLAY park verdict=INCOMPLETE staged={}/{} — set-completeness + set-validation dry-run only; NO PCI access, NO BAR map, NO window-selector move, NO core walk, NO device write was reachable on this leg ::",
            staged, want
        );
        return;
    }

    let ok = validate_set();
    serial_println!(
        ":: wifi: wifival REPLAY park verdict={} staged={}/{} — set-completeness + set-validation dry-run only; NO PCI access, NO BAR map, NO window-selector move, NO core walk, NO device write was reachable on this leg. This verdict is the SAME computation arc 2's gate 2 makes on metal, from the same one `classify_header`; what it does NOT do is authorise anything ::",
        if ok { "VALID" } else { "INVALID" }, staged, want
    );
}

/// Gate 2 — WIFI-SETVAL: validate the COMPLETE staged set, dry-run, no device access. One line per
/// role, one cross-set line, one verdict line. Returns whether arc 3's upload may ever consume this
/// set.
///
/// W3 was ANSWERED on metal (rmbp1-boot1; `firmware.rs` module note carries the evidence and the
/// provenance), and the REQUIRED rules are now per-role, replacing the refuted cross-file
/// layout-uniformity rule — the microcode and the initvals legitimately carry DIFFERENT payload
/// shapes under the one 8-byte container, and "uniform" is exactly the reading the 178-byte
/// bsinitvals file falsified:
///
///   * the ucode is a word-stream (`hdr=words`): declared size == payload bytes, whole be32 words;
///   * both initvals are record-streams (`hdr=records`): a clean record walk whose count equals the
///     declared size — verified by walking, never by arithmetic alone;
///   * `stream_ok` carries each rule's verdict, computed by the one `classify_header`.
///
/// ADVISORY (reported, never gated on): the version bytes. Observed ver=0x01 on all three (metal),
/// but no legal source pins what other extractions produce, so it stays a report.
fn validate_set() -> bool {
    let hs = super::firmware::set_headers();
    let want = super::firmware::FW_SET_LEN;
    if hs.len() != want {
        serial_println!(
            ":: wifi2: set-validate FAILED — the set reports COMPLETE but only {}/{} roles are retrievable from the staging buffers; that disagreement is itself the finding ::",
            hs.len(), want
        );
        return false;
    }

    let mut all_ok = true;
    for h in &hs {
        let want_layout = match h.role {
            "ucode" => "words",
            _ => "records",
        };
        let ok = h.layout == want_layout && h.stream_ok;
        all_ok &= ok;
        serial_println!(
            ":: wifi2: set-validate {} bytes={} fnv1a={:#010x} hdr={} want={} type={:#04x} ver={:#04x} declared={} records={} payload-bytes={} stream-ok={} => {} — dry-run, NO device access; this is the exact stream arc 3 would push for this role ::",
            h.role, h.len, h.digest, h.layout, want_layout, h.kind, h.ver, h.declared, h.records,
            h.len.saturating_sub(8), h.stream_ok as u8, if ok { "VALID" } else { "INVALID" },
        );
    }

    if let (Some(uc), Some(iv), Some(bs)) = (
        hs.iter().find(|h| h.role == "ucode"),
        hs.iter().find(|h| h.role == "initvals"),
        hs.iter().find(|h| h.role == "bsinitvals"),
    ) {
        serial_println!(
            ":: wifi2: set-validate cross type(initvals)==type(bsinitvals)={} type(ucode)!=type(initvals)={} ver-uniform={} (ADVISORY — the type rules are REQUIRED per-role above; ver is reported, not gated) ::",
            (iv.kind == bs.kind) as u8, (uc.kind != iv.kind) as u8,
            (uc.ver == iv.ver && iv.ver == bs.ver) as u8,
        );
    }

    serial_println!(
        ":: wifi2: set-validation verdict={} — W3 is ANSWERED (rmbp1-boot1): one 8-byte container, size=bytes for the ucode word-stream, size=RECORD-COUNT for the initvals record-streams; arc 3's upload is GATED on this verdict — a VALID here is its precondition, never its permission ::",
        if all_ok { "VALID" } else { "INVALID" }
    );
    all_ok
}

/// The audited zeros are FIELDS, not literals. `core_regs` covers `SHM_CONTROL`, `SHM_DATA`,
/// `MACCTL` and `RADIO_CONTROL` alike — they are all d11 core-window registers and this file has no
/// write site for any of them — so one counter carries the whole claim and the compiler's
/// reachability, not a reader's trust, is what keeps it at zero. One print site, shared by both
/// terminal refusals, so the two exits cannot drift.
fn upload_not_attempted(w: &Writes) {
    serial_println!(
        ":: wifi2: upload NOT ATTEMPTED uploaded-bytes={}(audited) wrote-core-regs={}(audited — SHM_CONTROL, SHM_DATA, MACCTL and RADIO_CONTROL share this counter and no site in this file increments it) — the resident microcode is untouched and still running ::",
        w.upload_bytes, w.core_regs
    );
}
