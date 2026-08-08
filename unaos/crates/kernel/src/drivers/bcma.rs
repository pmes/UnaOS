// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! BCMA-RECON — read-only reconnaissance of the Broadcom WiFi radio (`UNAOS_BCMARECON=1`).
//!
//! ## Why this module exists
//!
//! GR20's PCI census (`arch/x86_64/pci.rs::full_census`) established that the "network controller"
//! this kernel has announced on every 2012 rMBP boot —
//!
//! ```text
//! :: x86_64 PCI: Found network controller (class 0x02) vendor 0x14e4 at 3:0.0 ::
//! ```
//!
//! — is matched by `PciScanner::find_device(0x02, 0x00)`, i.e. **subclass 0x00 = Ethernet**, and
//! that filter returns on its FIRST hit. `14e4:16bc` at `3:0.1` is the BCM57765 SDXC reader; its
//! function-0 sibling is that combo chip's Gigabit Ethernet MAC. A BCM4331 WiFi part reports
//! subclass **0x80** ("other network controller") and therefore *cannot* have matched that filter:
//! the radio in this machine has never been looked at by our own kernel, not once.
//!
//! This module is the first arc of the native-driver path. It converts assumptions into facts.
//! S0 wrote nothing; S1/L0 write exactly ONE register — the `cfg:0x80` window selector — and
//! restore it, MATCH-verified.
//!
//! ## The hard constraint: READ-ONLY past the selector, and honest about where read-only stops
//!
//! Every device access below is a PCI **config-space read** or an **MMIO read** through BAR0,
//! except the five `write_config_32` calls to `CFG_BAR0_WIN` (selftest no-op, ChipCommon, EROM,
//! d11 base, restore). No `write_volatile`, no MMIO write, no other config write exists in this
//! file. That is not a style preference; it is the arc's whole contract, because the
//! alternative — poking a radio whose
//! backplane we have not enumerated — is how you wedge a bus you cannot yet reset.
//!
//! Mapping BAR0 (`arch::memory::map_mmio_window`) is a page-table edit, not a device access: the
//! device sees nothing. It is the same seam `sdhc::probe` and the GPU drivers use.
//!
//! The consequence, stated up front because it is the arc's main finding: **a BCMA part on a PCIe
//! host exposes exactly ONE backplane core through BAR0 at a time**, selected by a PCI *config*
//! register. Everything not currently in that window — the enumeration ROM, the 802.11 core, the
//! PHY version register — is behind one 32-bit config write this arc refuses to make. Each of those
//! is reported as an explicit `REFUSED` witness naming the register, not as a missing line.
//!
//! ## What Boot AF changed
//!
//! The first metal reading falsified this file's central assumption. It had assumed firmware leaves
//! `BCMA_PCI_BAR0_WIN` (cfg:0x80) parked at the ChipCommon enumeration base, so that ChipCommon at
//! least would be free. On the 2012 rMBP it does not:
//!
//! ```text
//! bar0_win(cfg:0x80)=0x18001000 bar0_win2(cfg:0xac)=0x18101000
//! ```
//!
//! `0x18001000` is `BCMA_ADDR_BASE + 1*BCMA_CORE_SIZE` and `0x18101000` is
//! `BCMA_WRAP_BASE + 1*BCMA_CORE_SIZE` — the (`core->addr`, `core->wrap`) pair Linux's
//! `bcma_host_pci_switch_core()` writes for **core index 1**, which on a BCM4331 is the D11 802.11
//! core. Apple's firmware left the window on the radio core, not on ChipCommon. So BAR0+0 was never
//! ChipCommon on this machine, and the `0xffffffff` at BAR0+0 is the reading of an off-window core,
//! not a verdict about the function.
//!
//! That reading was also **misreported**, by this file: stage 5 tested "all-ones" BEFORE it tested
//! "is the window even on ChipCommon", so the `window-elsewhere` refusal — which was present and
//! correct — could never fire whenever the off-window core happened to be unreadable. The order is
//! now window-first, which is the only order in which either refusal means what it says.
//!
//! Boot AF also settled that this is NOT a mapping fault of ours. The map wire line
//! (`:: x86 mmio-map: 0xc1900000..0xc1902000 uc=1 (PAT PA3) wc-kept=0 ::`) retypes the same 2 MiB
//! leaf that `sdhc`'s BAR at `0xc1820000` sits in, and `sdhc` reads its registers successfully in
//! the same boot through that same leaf. Identical VA, identical PAT typing, one works.
//!
//! ## Stage 4b: the wrapper window is the discriminator
//!
//! BAR0's SECOND 4 KiB is a separate window onto the selected core's AXI/OOB **wrapper**, selected
//! by cfg:0xAC (Linux `bcma_host_pci_aread32` reads `mmio + 1*BCMA_CORE_SIZE + offset`). The wrapper
//! is exactly the block that stays alive while its core is held in reset — it is how a driver takes
//! the core OUT of reset. Reading `IOCTL`/`RESET_CTL` there is read-only and splits the two live
//! explanations of an all-ones core window:
//!
//! * wrapper answers, `RESET_CTL.reset=1` or `IOCTL.clk=0` -> the core in the window is in reset /
//!   clock-gated. The function decodes fine; we were pointed at a dark core.
//! * wrapper ALSO reads all-ones -> the function is not answering at all, and the blame moves
//!   upstream to the link or the bridge window, not to a core's reset state.
//!
//! Whichever it is, it is a fact about this machine that the next boot prints.
//!
//! ## Witness contract
//!
//! One bounded block, `:: bcma: begin … ::` … `:: bcma: end … ::`, in the tree's `:: subsystem: ::`
//! idiom. Every stage can say NO:
//!
//! * no class-0x02/subclass-0x80 function on the machine  -> `no-device` (QEMU's expected reading);
//! * a non-Broadcom vendor                                -> identity printed, BCMA decode refused;
//! * the function parked in D1/D2/D3                      -> `REFUSED reason=power-state`;
//! * memory decode off in COMMAND                         -> `REFUSED reg=cfg:0x04.bit1`;
//! * BAR0 unassigned, or not identity-mapped after the map -> `REFUSED reason=bar0-*`;
//! * the BAR0 window not parked at the enumeration base   -> `REFUSED reason=window-elsewhere`
//!   (tested FIRST, because it is a fact about cfg:0x80 and needs no MMIO read to be true);
//! * ChipCommon reading all-ones with the window correct  -> `REFUSED reason=not-decoding`;
//! * EROM outside the live window                         -> `REFUSED reg=cfg:0x80` + the address;
//! * no SPROM advertised and no OTP subregion programmed  -> `sprom=absent otp=absent`.
//!
//! Nothing prints a plausible default in place of a value it could not read. A stage that did not
//! run says so; a stage that ran and found zero says that instead.
//!
//! ## Sourcing
//!
//! Register offsets and EROM encodings follow Linux `drivers/bcma` (`bcma_regs.h`,
//! `bcma_driver_chipcommon.h`, `scan.c`) and `b43`'s `B43_MMIO_PHY_VER`. Where this file is not
//! confident of a bit assignment it prints the RAW word and labels the decode as unverified rather
//! than asserting it — a recon arc that guesses is worth less than one that reports.
//!
//! x86_64 only; compiled solely under the `bcmarecon` feature. Knob OFF => this module does not
//! exist, no call site is emitted, and media are byte-identical.

use crate::arch::pci::{read_config_16, read_config_32};

// ── PCI-side constants ──────────────────────────────────────────────────────────────────────────

/// PCI class 0x02 = Network controller.
const CLASS_NETWORK: u8 = 0x02;
/// PCI subclass 0x80 = "other network controller". This is the subclass Broadcom WiFi parts report
/// and the one `find_device(0x02, 0x00)` structurally cannot match.
const SUBCLASS_OTHER: u8 = 0x80;
/// Broadcom's PCI vendor id.
const VENDOR_BROADCOM: u16 = 0x14E4;

const CFG_CMD_STS: u8 = 0x04; // command (lo16) + status (hi16)
const CFG_CLASS: u8 = 0x08; // rev / prog-if / subclass / class
const CFG_HDR: u8 = 0x0C; // cacheline / latency / header type / BIST
const CFG_BAR0: u8 = 0x10;
const CFG_BAR1: u8 = 0x14; // high half of a 64-bit BAR0
const CFG_SUBSYS: u8 = 0x2C; // subsystem vendor (lo16) + subsystem device (hi16)
const CFG_CAP_PTR: u8 = 0x34;
const CFG_INTR: u8 = 0x3C; // interrupt line (byte0) + pin (byte1)

/// Capability id 0x01 = PCI Power Management. PMCSR sits 4 bytes past the capability header; its
/// low 2 bits are the D-state. An MMIO read of a function parked in D3hot returns undefined data on
/// most silicon, so the state is read BEFORE any BAR access and gates it.
const CAP_ID_PM: u8 = 0x01;

/// `BCMA_PCI_BAR0_WIN` — the backplane address the first 4 KiB of BAR0 decodes to. Linux's
/// `bcma_scan_switch_core()` writes this register (and nothing else) to move the window from core
/// to core; it is the single register that gates every backplane read past ChipCommon.
/// **The ONLY register this module ever writes** (S1/L0: three moves + restore, MATCH-verified;
/// S0 never wrote it).
const CFG_BAR0_WIN: u8 = 0x80;
/// `BCMA_PCI_BAR0_WIN2` — the backplane address the SECOND 4 KiB of BAR0 decodes to (the wrapper
/// window). Read here only to record what firmware left behind.
const CFG_BAR0_WIN2: u8 = 0xAC;

// ── BCMA backplane constants ────────────────────────────────────────────────────────────────────

/// `BCMA_ADDR_BASE` / `SI_ENUM_BASE` — the backplane address of the ChipCommon core, and the value
/// firmware is expected to have left in `BAR0_WIN`. If the live window is anything else, the
/// registers at BAR0+0 are NOT ChipCommon and this file refuses to decode them.
const BCMA_ADDR_BASE: u32 = 0x1800_0000;

/// `BCMA_WRAP_BASE` — the backplane base of the core-WRAPPER address space. On the parts this file
/// targets, core *i*'s registers sit at `BCMA_ADDR_BASE + i*BCMA_CORE_SIZE` and its wrapper at
/// `BCMA_WRAP_BASE + i*BCMA_CORE_SIZE`, which is why a (cfg:0x80, cfg:0xAC) pair of
/// `(0x18001000, 0x18101000)` is readable as "core index 1, wrapper index 1" and not as noise. The
/// EROM is authoritative for both addresses; this grid is used ONLY to render an index for the
/// reader, and every line that uses it says so.
const BCMA_WRAP_BASE: u32 = 0x1810_0000;

/// One backplane core's register window. Also the size of the BAR0 core window, which is what makes
/// "is the EROM reachable" a simple containment test.
const BCMA_CORE_SIZE: u32 = 0x1000;

/// Ceiling on the core index this file will render from a raw window address. `BCMA_MAX_NR_CORES`
/// in Linux is 16; a window past that is off the grid and is printed as unindexed rather than as a
/// large index that would look like a decode.
const BCMA_MAX_NR_CORES: u32 = 16;

/// How much of BAR0 is mapped, and it is NOT a guess: on a BCMA PCIe host BAR0 is exactly two 4 KiB
/// apertures, and Linux's own accessors say which is which — `bcma_host_pci_read32` reads
/// `mmio + offset` (the core window, selected by cfg:0x80) and `bcma_host_pci_aread32` reads
/// `mmio + 1*BCMA_CORE_SIZE + offset` (the wrapper window, selected by cfg:0xAC). No bcma accessor
/// in Linux ever indexes past `2*BCMA_CORE_SIZE`, so 0x2000 is the whole architecturally-defined
/// extent of BAR0 for this driver model, whatever the BAR's physical size turns out to be.
///
/// The physical size is a separate question and remains UNKNOWN: reading it needs the write-all-ones
/// / read-mask / restore sizing dance, and this arc does not write. The relationship is one-sided
/// and safe in the direction that matters — the true size is >= 0x2000 (both windows exist and are
/// used), so mapping 0x2000 cannot map past the BAR.
const BAR0_MAP_LEN: usize = 0x2000;

/// Byte offset of the wrapper aperture inside BAR0. See [`BAR0_MAP_LEN`].
const BAR0_WRAP_OFF: u64 = 0x1000;

// AXI/OOB core-wrapper registers (Linux `include/linux/bcma/bcma.h`). These live in the WRAPPER
// window, not the core window, and they are readable while the core itself is held in reset — which
// is the whole reason stage 4b can tell "core is dark" from "function is dead".
const WRAP_IOCTL: u64 = 0x0408; // BCMA_IOCTL
const WRAP_IOST: u64 = 0x0500; // BCMA_IOST
const WRAP_RESET_CTL: u64 = 0x0800; // BCMA_RESET_CTL
const WRAP_RESET_ST: u64 = 0x0804; // BCMA_RESET_ST

const IOCTL_CLK: u32 = 0x0001; // BCMA_IOCTL_CLK
const IOCTL_FGC: u32 = 0x0002; // BCMA_IOCTL_FGC
const IOST_GATED_CLK: u32 = 0x2000_0000; // BCMA_IOST_GATED_CLK
const RESET_CTL_RESET: u32 = 0x0001; // BCMA_RESET_CTL_RESET

// ChipCommon core register offsets (Linux `bcma_driver_chipcommon.h`).
const CC_ID: u64 = 0x0000; // chip id / rev / package / #cores / chip type
const CC_CAP: u64 = 0x0004; // capabilities
const CC_OTPS: u64 = 0x0010; // OTP status
const CC_OTPC: u64 = 0x0014; // OTP control
const CC_OTPP: u64 = 0x0018; // OTP prog — the OTP READ COMMAND register (a WRITE; refused here)
const CC_OTPL: u64 = 0x001C; // OTP layout
const CC_CHIPSTATUS: u64 = 0x002C; // chip status (per-chip bit meanings)
const CC_CAP_EXT: u64 = 0x00AC; // capabilities extension
const CC_EROM: u64 = 0x00FC; // backplane address of the enumeration ROM
const CC_SPROM: u64 = 0x0800; // SPROM shadow — the offset Linux uses on the BCM4331 (see S2)
/// Alternate SPROM shadow (the PCIe-core rev >= 6 offset; **not** the 4331's — see
/// [`sprom_offset_for`]). Only S2 touches it, and only to dump it for the record beside the chosen
/// offset, so that one capture can settle the offset rule either way.
#[cfg(feature = "bcmaS1")]
const CC_SPROM_PCIE6: u64 = 0x0830;

/// `BCMA_CC_SROM_CONTROL` — the SPROM interface control register, and the SECOND half of Linux's
/// `bcma_sprom_ext_available()`: on a ChipCommon CORE rev >= 31 that advertises `CAP_SPROM`, an
/// external SPROM is *actually present* iff `SROM_CONTROL.PRESENT` is set. Reading it turns the
/// BLOCKED branch below from a guess ("the shadow looks empty, maybe the PA mux") into a
/// determination ("the chip itself says no external SPROM is attached").
///
/// This is a PRESENCE-detection register. It is emphatically NOT the offset selector — conflating
/// the two is the defect this stage was bounced for; see [`sprom_offset_for`].
#[cfg(feature = "bcmaS1")]
const CC_SROM_CONTROL: u64 = 0x0190;
/// `BCMA_CC_SROM_CONTROL_PRESENT` = **bit 0**, verified against
/// `bcma_driver_chipcommon.h:281` (`0x00000001`). The first cut had `0x00800000` — which IS a real
/// macro in that same header, but it is `BCMA_CC_CAP_BROM`, a bit in the CAPABILITIES register, not
/// in SROM_CONTROL at all. Every defined bit here lives in [31:29] or [5:0]; bit 23 is undefined and
/// would have read 0, turning the "no external SPROM attached" branch into a CONFIDENT WRONG
/// determination on a board whose ChipCommon `cap` bit 30 already says sprom=present. The raw word
/// is still printed beside the decode.
#[cfg(feature = "bcmaS1")]
const SROM_CONTROL_PRESENT: u32 = 0x0000_0001;

/// `BCMA_CC_CAP_SPROM` — an SPROM is present on this board.
const CC_CAP_SPROM: u32 = 0x4000_0000;
/// `BCMA_CC_CAP_PMU` — the chip has a PMU (rev >= 20).
const CC_CAP_PMU: u32 = 0x1000_0000;
/// `BCMA_CC_CAP_OTPS` mask/shift/base — OTP size is `1 << (base + field)` words when the field is
/// non-zero, and "no OTP" when it is zero.
const CC_CAP_OTPS_MASK: u32 = 0x0038_0000;
const CC_CAP_OTPS_SHIFT: u32 = 19;
const CC_CAP_OTPS_BASE: u32 = 5;

/// `BCMA_CC_OTPS_GUP_*` — which OTP subregions have actually been programmed. These four bits are
/// the read-only evidence that decides whether this board's calibration lives in OTP at all, which
/// is the question Apple boards exist to make interesting.
const OTPS_GUP_HW: u32 = 0x0000_0100;
const OTPS_GUP_SW: u32 = 0x0000_0200;
const OTPS_GUP_CI: u32 = 0x0000_0400;
const OTPS_GUP_FUSE: u32 = 0x0000_0800;

// ── d11 (802.11 MAC) core registers — b43 `b43.h` MMIO offsets ──────────────────────────────────
//
// These are offsets INSIDE the d11 core's own 4 KiB register window, i.e. what BAR0+0 decodes to
// once cfg:0x80 carries the d11 core's backplane base. Every one of them is a status or control
// register with NO read side effect. The registers deliberately NOT read here are the ones that
// would make "read-only" a lie:
//
// * `B43_MMIO_GEN_IRQ_REASON` (0x128) and the per-ring `DMA*_REASON` words are **read-to-clear** —
//   reading them would destroy interrupt state the firmware/PHY bring-up stages will need;
// * `B43_MMIO_RADIO_CONTROL` (0x3E2) / `RADIO_DATA*` — reading a radio register requires first
//   WRITING the register index to the control port, so the radio's own id is not a read-only fact;
// * `B43_MMIO_SHM_CONTROL` (0x160) / `SHM_DATA` — same shape: SHM is an indirect window opened by a
//   write. The MACCTL `SHM_ENABLED`/`IHR_ENABLED` bits below are the read-only part of that story.
#[cfg(feature = "bcmaS1")]
const D11_MACCTL: u64 = 0x0120; // B43_MMIO_MACCTL — MAC control (r/w, no side effect)
#[cfg(feature = "bcmaS1")]
const D11_GEN_IRQ_MASK: u64 = 0x012C; // B43_MMIO_GEN_IRQ_MASK (the MASK, not the read-to-clear REASON)
#[cfg(feature = "bcmaS1")]
const D11_RADIO_HWENABLED_HI: u64 = 0x0158; // B43_MMIO_RADIO_HWENABLED_HI (u32, phy rev >= 3)
#[cfg(feature = "bcmaS1")]
const D11_TSF_LOW: u64 = 0x0180; // B43_MMIO_REV3PLUS_TSF_LOW
#[cfg(feature = "bcmaS1")]
const D11_TSF_HIGH: u64 = 0x0184; // B43_MMIO_REV3PLUS_TSF_HIGH
#[cfg(feature = "bcmaS1")]
const D11_PHY_VER: u64 = 0x03E0; // B43_MMIO_PHY_VER (u16) — the PHY identity word
#[cfg(feature = "bcmaS1")]
const D11_RADIO_HWENABLED_LO: u64 = 0x049A; // B43_MMIO_RADIO_HWENABLED_LO (u16, phy rev < 3)

// `B43_MACCTL_*`. Only the bits whose meaning is unambiguous in b43's header are decoded; the RAW
// word is on the same line, so a reader can check any bit this file declines to name.
#[cfg(feature = "bcmaS1")]
const MACCTL_ENABLED: u32 = 0x0000_0001;
#[cfg(feature = "bcmaS1")]
const MACCTL_PSM_RUN: u32 = 0x0000_0002;
#[cfg(feature = "bcmaS1")]
const MACCTL_SHM_ENABLED: u32 = 0x0000_0100;
#[cfg(feature = "bcmaS1")]
const MACCTL_IHR_ENABLED: u32 = 0x0000_0400;
#[cfg(feature = "bcmaS1")]
const MACCTL_BE: u32 = 0x0001_0000;
#[cfg(feature = "bcmaS1")]
const MACCTL_AWAKE: u32 = 0x0400_0000;
#[cfg(feature = "bcmaS1")]
const MACCTL_GMODE: u32 = 0x8000_0000;

// `B43_PHYVER_*` field masks.
#[cfg(feature = "bcmaS1")]
const PHYVER_ANALOG: u16 = 0xF000;
#[cfg(feature = "bcmaS1")]
const PHYVER_ANALOG_SHIFT: u16 = 12;
#[cfg(feature = "bcmaS1")]
const PHYVER_TYPE: u16 = 0x0F00;
#[cfg(feature = "bcmaS1")]
const PHYVER_TYPE_SHIFT: u16 = 8;
#[cfg(feature = "bcmaS1")]
const PHYVER_VERSION: u16 = 0x00FF;

/// `B43_PHYTYPE_HT`. The BCM4331 carries the HT-PHY, and this is the number that says so. It is the
/// falsifiable predicate `docs/dev/OS/06_NETWORK_STACK/bcm4331.md` §S3 already names.
#[cfg(feature = "bcmaS1")]
const PHYTYPE_HT: u16 = 7;

// ── AI/aidmp agent (wrapper) identification block ───────────────────────────────────────────────
//
// The AXI agent Broadcom wraps each backplane core in is an ARM DMP, and it ends with the standard
// ARM PrimeCell/CoreSight identification registers: PeripheralID4..7 at 0xFD0..0xFDC, PeripheralID0..3
// at 0xFE0..0xFEC and ComponentID0..3 at 0xFF0..0xFFC (Broadcom `aidmp.h`, `struct aidmp` tail).
//
// This block is the arc's independent cross-check on the EROM walk. The EROM is a ROM we parse; the
// DMP ids are registers the core answers with. If they agree on the core's part number, the walk's
// claim that `0x18001000` is core id 0x812 is corroborated by the silicon at that address rather
// than only by our own decode of a table.
//
// The transcription is labelled UNVERIFIED on the witness line and the decode never overrides the
// raw words, because two things could make a mismatch innocent: the offsets could be transposed, or
// Broadcom could simply not program the DMP part-number field with the BCMA core id. A MISMATCH is
// therefore reported as a finding to chase, never as a verdict against the walk.
#[cfg(feature = "bcmaS1")]
const DMP_ID_BASE: u64 = 0x0FD0;
#[cfg(feature = "bcmaS1")]
const DMP_ID_WORDS: u64 = 12; // 0xFD0..0xFFC inclusive, 12 dwords

/// ARM component-id preamble, `CIDR0/1/2/3`. `CIDR1`'s low nibble is part of the preamble; its high
/// nibble is the component class.
#[cfg(feature = "bcmaS1")]
const CID0_PREAMBLE: u32 = 0x0D;
#[cfg(feature = "bcmaS1")]
const CID1_PREAMBLE: u32 = 0x00; // low nibble
#[cfg(feature = "bcmaS1")]
const CID2_PREAMBLE: u32 = 0x05;
#[cfg(feature = "bcmaS1")]
const CID3_PREAMBLE: u32 = 0xB1;

/// How many 16-bit SPROM shadow words to dump. 220 is `SSB_SPROMSIZE_WORDS_R4`, the size of a
/// revision-8 SPROM (what a BCM4331 board carries). 220 words = 440 bytes, so the highest byte
/// touched is 0x830 + 0x1B7 = 0x9E7, comfortably inside the 4 KiB core window.
const SPROM_WORDS: u64 = 220;

// ── SROM revision-8 field offsets (S2) ──────────────────────────────────────────────────────────
//
// Byte offsets INSIDE the SPROM image, i.e. relative to the shadow base (`CC_SPROM`/`CC_SPROM_PCIE6`);
// the MMIO address is `shadow_base + field`. Transcribed from Linux `include/linux/ssb/ssb_regs.h`
// (`SSB_SPROM8_*` / `SSB_SPROM1_SPID`), which is what `bcma_sprom_extract_r8` reads. The
// transcription is labelled UNVERIFIED on every S2 witness line and the RAW word is always printed
// beside the decode — a bring-up recon that guesses a bit is worth less than one that shows its
// evidence.
//
// **Nothing here is corroborated by a capture, and the first cut of this block claimed otherwise.**
// It justified its offsets as "matching what S0's dump already decodes" — which is circular twice
// over: S0 carried the *same* constants, and on this machine S0 has never once reached ChipCommon
// (it refuses at `window-elsewhere`), so its dump has never run. There is no prior reading to agree
// with. What these offsets have instead is a STRUCTURAL check: the rev-8 block is contiguous —
// `BOARDREV 0x82`, `BFL 0x84/0x86`, `BFL2 0x88/0x8A`, `IL0MAC 0x8C`, `ANTAVAIL 0x9C` — so an offset
// that lands outside that run is convicted by the layout itself. That is what caught `0x4A`/`0x5C`,
// which are the *rev-4* block. Metal settles the rest.
#[cfg(feature = "bcmaS1")]
const SP8_SPID: u64 = 0x0004; // board_type (SSB_SPROM1_SPID) — should equal the PCI ssid device id
/// `SSB_SPROM8_IL0MAC` (ssb_regs.h) — the station MAC, 3 big-endian 16-bit words.
///
/// **0x8C, not 0x4A.** The first cut of this stage used `0x4A`, which is not the rev-8 MAC and is
/// not even the rev-4 MAC: it is `SSB_SPROM4_BFL2HI`/`SSB_SPROM5_BFLLO`, i.e. a **boardflags** word.
/// Reading it and calling it a station MAC produces six plausible-looking bytes that are not an
/// address, and the `unicast` test on them is a coin flip rather than a check. The correct offset
/// sits immediately after this revision's boardflags block (`BOARDREV 0x82`, `BFL* 0x84..0x8A`),
/// which is the structural cross-check on the transcription.
///
/// Not `bcmaS1`-gated: the S0 recon decodes a MAC candidate too, and it carried the same wrong
/// literal inline. One constant, both paths, so the bug cannot be re-seeded in the half that a
/// review is not looking at.
const SP8_IL0MAC: u64 = 0x008C;
/// `SSB_SPROM8_ANTAVAIL` (ssb_regs.h) — [7:0] = 2.4GHz antenna mask, [15:8] = 5GHz antenna mask.
/// **0x9C, not 0x5C** — `0x5C` is the *rev-4* antavail, the same revision-block slip as the MAC above.
#[cfg(feature = "bcmaS1")]
const SP8_ANTAVAIL: u64 = 0x009C;
#[cfg(feature = "bcmaS1")]
const SP8_BOARDREV: u64 = 0x0082; // SSB_SPROM8_BOARDREV
#[cfg(feature = "bcmaS1")]
const SP8_BFL_LO: u64 = 0x0084; // SSB_SPROM8_BFLLO  (boardflags, low half)
#[cfg(feature = "bcmaS1")]
const SP8_BFL_HI: u64 = 0x0086; // SSB_SPROM8_BFLHI  (boardflags, high half)
#[cfg(feature = "bcmaS1")]
const SP8_BFL2_LO: u64 = 0x0088; // SSB_SPROM8_BFL2LO (boardflags2, low half)
#[cfg(feature = "bcmaS1")]
const SP8_BFL2_HI: u64 = 0x008A; // SSB_SPROM8_BFL2HI (boardflags2, high half)

// ── EROM entry encodings — Broadcom AI ("aidmp") enumeration ROM ────────────────────────────────
//
// Layout as decoded by Linux `drivers/bcma/scan.{h,c}` (`SCAN_*`) and Broadcom's `siutils_priv.h`
// (`CIA_*`, `CIB_*`, `AD_*`, `SD_*`). Boot AJ's walk found ZERO cores against the previous
// transcription, and the abort line it printed is the proof of WHICH transcription was wrong:
//
// ```text
// :: bcma: erom-abort at entry 4 while consuming 2 master ports of core id=0xf80 ::
// ```
//
// The first EROM word on a BCM4331 is ChipCommon's CIA. Decoding it with the OLD masks
// (`id = word[23:12]`) yields `0xf80`; decoding it with the masks below (`id = word[19:8]`,
// `manuf = word[31:20]`) yields `manuf=0x4bf (BCM) id=0x800 (chipcommon) class=0` from the SAME
// word `0x4BF80001`. Only one of those is a core this chip has.
//
// The same line convicts the CIB masks, at the bit level. `nmp=2` came out of the old
// `(cib >> 13) & 0xF`, so ChipCommon's real CIB has bits 16:13 = `0b0010` — bit 14 set, 13/15/16
// clear — and bit 14 is the low bit of `CIB_NMW` below, i.e. ChipCommon has an odd number of master
// wrappers (1). Meanwhile "entry 4" is arithmetic: the walk read CIA at 0, CIB at 1, then consumed
// TWO entries before failing a tag check, so entry 2 really did carry the master-port tag (`nmp`
// is genuinely >= 1) and entry 3 — ChipCommon's FIRST ADDRESS descriptor, its `base` — was eaten as
// if it were a second master port. A count of 1/1/1/1 for nmp/nsp/nmw/nsw reproduces the observed
// `nmp=2` exactly through the old mask. Every field below is anchored to that reading; nothing here
// is asserted about a bit the capture is silent on.

/// Entry is populated. Bit 0 of every EROM dword.
const ER_VALID: u32 = 0x0000_0001;
/// Tag field for COMPONENT / MASTER-PORT / END entries: bits 3:1.
const ER_TAG: u32 = 0x0000_000E;
/// Tag field for ADDRESS entries: bits 2:1 **only** (Linux `SCAN_ER_TAGX`).
///
/// This narrower mask is not a nicety. In an address descriptor bit 3 is NOT tag — it is
/// [`AD_AG32`], "a high address dword follows". Classifying an address descriptor with the full
/// `0xE` therefore mis-reads every 64-bit-capable descriptor as "not an address", and Linux uses
/// `SCAN_ER_TAGX` in exactly one place for exactly this reason: `bcma_erom_get_addr_desc`.
const ER_TAGX: u32 = 0x0000_0006;
const ER_TAG_CI: u32 = 0x0000_0000; // component identifier (CIA, then CIB)
const ER_TAG_MP: u32 = 0x0000_0002; // master port descriptor
const ER_TAG_ADDR: u32 = 0x0000_0004; // address descriptor
const ER_TAG_END: u32 = 0x0000_000E; // end of the ROM
/// The end sentinel is the WHOLE word, not just its tag: `ER_TAG_END | ER_VALID`.
const ER_END_WORD: u32 = ER_TAG_END | ER_VALID;
const ER_BAD: u32 = 0xFFFF_FFFF;

// Component Identifier A — manufacturer / core id / class, above the 4-bit tag.
const CIA_MFG_MASK: u32 = 0xFFF0_0000;
const CIA_MFG_SHIFT: u32 = 20;
const CIA_ID_MASK: u32 = 0x000F_FF00;
const CIA_ID_SHIFT: u32 = 8;
const CIA_CLASS_MASK: u32 = 0x0000_00F0;
const CIA_CLASS_SHIFT: u32 = 4;

// Component Identifier B — port and wrapper counts, and the CORE revision.
//
// The CIB is a contiguous tiling of FOUR 5-bit counts starting immediately above the tag nibble:
// nmp at bits 8:4, nsp at 13:9, nmw at 18:14, nsw at 23:19, then the 8-bit rev at 31:24. Each mask
// is the previous one shifted left by 5; each shift is the previous plus 5. That tiling is the
// whole check — a mask narrower than 5 bits orphans the count's low bit, and the S1b review caught
// exactly that here: `nmp` shipped as `0x1E0 >> 5` (bits 8:5, four bits) dropped bit 4, so a true
// count of 1 decoded as `floor(1/2) = 0`, the walk consumed zero master ports, then read
// ChipCommon's real master-port descriptor as its "first slave address" and aborted with cores=0 —
// the SAME class of off-by-a-bit error this arc convicted the old CIA masks for. `nsp` had the
// identical latent slip (`0x1E00`, bits 12:9). Both are now the full 5 bits.
const CIB_NMP_MASK: u32 = 0x0000_01F0; // # master ports  — bits 8:4
const CIB_NMP_SHIFT: u32 = 4;
const CIB_NSP_MASK: u32 = 0x0000_3E00; // # slave ports   — bits 13:9
const CIB_NSP_SHIFT: u32 = 9;
const CIB_NMW_MASK: u32 = 0x0007_C000; // # master wrappers — bits 18:14
const CIB_NMW_SHIFT: u32 = 14;
const CIB_NSW_MASK: u32 = 0x00F8_0000; // # slave wrappers  — bits 23:19
const CIB_NSW_SHIFT: u32 = 19;
const CIB_REV_MASK: u32 = 0xFF00_0000;
const CIB_REV_SHIFT: u32 = 24;

// Address descriptor.
const AD_ADDR_MASK: u32 = 0xFFFF_F000;
const AD_PORT_MASK: u32 = 0x0000_0F00; // which port of the component this address belongs to
const AD_PORT_SHIFT: u32 = 8;
const AD_TYPE_MASK: u32 = 0x0000_00C0;
const AD_TYPE_SHIFT: u32 = 6;
const AD_TYPE_SLAVE: u32 = 0; // the core's own register window
const AD_TYPE_BRIDGE: u32 = 1; // a backplane bridge, not a core
const AD_TYPE_SWRAP: u32 = 2; // slave wrapper — reset/clock control
const AD_TYPE_MWRAP: u32 = 3; // master wrapper
const AD_SZ_MASK: u32 = 0x0000_0030;
const AD_SZ_SZD: u32 = 0x0000_0030; // size lives in a following SIZE descriptor
/// Address is 64-bit: a high dword follows. **Bit 3**, which is why address descriptors are
/// classified with [`ER_TAGX`] and not [`ER_TAG`].
const AD_AG32: u32 = 0x0000_0008;
/// In a SIZE descriptor, "a high size dword follows".
const SD_SG32: u32 = 0x0000_0008;

/// Manufacturer ids (Linux `BCMA_MANUF_*`).
const MANUF_ARM: u16 = 0x43B;
const MANUF_MIPS: u16 = 0x4A7;
const MANUF_BCM: u16 = 0x4BF;
/// `BCMA_CORE_DEFAULT` — an ARM-manufactured component with this id is a placeholder, not a core.
const CORE_ID_DEFAULT: u16 = 0xFFF;
/// `BCMA_CORE_80211` — the d11 radio core. The one id this whole path exists to locate.
const CORE_ID_80211: u16 = 0x812;
/// `BCMA_CORE_CHIPCOMMON` — core index 0 on every AI backplane. Its CORE revision (from the EROM
/// CIB, not the chip rev in `chipid[19:16]`) is NOT what selects the SPROM shadow offset — that was the refuted first cut. It selects
/// whether SROM_CONTROL's PRESENT bit is authoritative for SPROM PRESENCE (`bcma_sprom_ext_available`).
const CORE_ID_CHIPCOMMON: u16 = 0x800;

/// Hard iteration ceiling on the EROM walk, in dwords, and it is the ARCHITECTURAL bound rather
/// than a round number: the EROM is reached through the 4 KiB BAR0 core window, so dword 1024 is
/// the first read that would leave the window entirely. A real EROM is on the order of 60-100
/// dwords, so hitting this is itself information and is reported. This is the STRUCTURAL bound; the
/// TSC deadline below is the WALL-CLOCK bound, and both are needed — a walk over a stuck-at-zero
/// window would satisfy neither on its own.
const EROM_MAX_ENTRIES: u32 = BCMA_CORE_SIZE / 4;

/// How many cores are printed. The 4331 backplane has ~8; 32 cannot be reached honestly.
const EROM_MAX_CORES: u32 = 32;

// ── Bounded time ────────────────────────────────────────────────────────────────────────────────

/// A wall-clock deadline in `now_cycles()` (rdtsc) units.
///
/// TSC, not `arch::ms()`: `ms()` is derived from the local-APIC tick, which only advances when the
/// APIC-timer ISR runs and `EFLAGS.IF` is set. This probe runs inside `pci::init` where neither is
/// guaranteed, so a millisecond loop there can hang forever while looking like it is counting.
/// `now_cycles()` is free-running and invariant on Ivy Bridge, and `hw_wait_budget()` is the same
/// budget every other bounded wait in this kernel uses.
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
    fn elapsed_cycles(&self) -> u64 {
        crate::arch::now_cycles().wrapping_sub(self.t0)
    }
}

/// Cycles → whole ms at print time via the BPACE ledger's own rate, or raw ticks when that rate is
/// still unknown. Same expression as `arch::x86_64::pci::gpace_fmt`, for the same reason: never
/// fabricate a millisecond out of a guessed frequency.
fn fmt_dur(cy: u64) -> (u64, &'static str) {
    let hz = crate::bootpace::origin_hz();
    if hz >= 1000 { (cy / (hz / 1000), "ms") } else { (cy, "cy") }
}

// ── MMIO ────────────────────────────────────────────────────────────────────────────────────────

/// Read a u32 from the identity-mapped BAR0 window.
///
/// # Safety
/// The caller must have mapped the window (`map_mmio_window`) AND proved the page present with
/// `translate()`. `read_volatile` of a BCMA ChipCommon register has no side effect — every offset
/// this file reads is a status/identity register, not a FIFO or a read-to-clear.
unsafe fn r32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}

/// Read a u16 out of the SPROM shadow, which is a 16-bit-wide window.
///
/// # Safety
/// As [`r32`].
unsafe fn r16(base: u64, off: u64) -> u16 {
    core::ptr::read_volatile((base + off) as *const u16)
}

// ── Capability walk (read-only) ─────────────────────────────────────────────────────────────────

/// Return the config offset of the first capability whose id is `want`, or 0 if absent.
///
/// # Safety
/// Config-space reads only.
unsafe fn find_cap(bus: u8, dev: u8, func: u8, want: u8) -> u8 {
    let status = (read_config_32(bus, dev, func, CFG_CMD_STS) >> 16) as u16;
    if status & (1 << 4) == 0 {
        return 0; // the function advertises no capability list at all
    }
    let mut ptr = (read_config_32(bus, dev, func, CFG_CAP_PTR) & 0xFC) as u8;
    let mut guard = 0u8;
    while ptr >= 0x40 && ptr != 0xFF && guard < 48 {
        let cap = read_config_32(bus, dev, func, ptr);
        if (cap & 0xFF) as u8 == want {
            return ptr;
        }
        ptr = ((cap >> 8) & 0xFC) as u8;
        guard += 1;
    }
    0
}

// ── Device discovery ────────────────────────────────────────────────────────────────────────────

/// Find the FIRST class-0x02 / subclass-0x80 function, and count how many exist.
///
/// Deliberately NOT `PciScanner::find_device`: that helper takes (class, subclass) but only ever
/// probes function 0 of each device, and this arc exists because a targeted filter hid a device for
/// the life of the project. The sweep here is the census's shape — 256 buses x 32 devices, with
/// functions 1..7 probed only when function 0's header type carries the multi-function bit (0x80),
/// because probing them on a single-function device is architecturally undefined.
///
/// Returns `(bdf, total_matches)`.
fn find_wifi() -> (Option<(u8, u8, u8)>, u32) {
    let mut first: Option<(u8, u8, u8)> = None;
    let mut count = 0u32;
    for bus in 0u16..256 {
        for dev in 0u8..32 {
            let v0 = unsafe { read_config_16(bus as u8, dev, 0, 0x00) };
            if v0 == 0xFFFF {
                continue;
            }
            let hdr0 = ((unsafe { read_config_32(bus as u8, dev, 0, CFG_HDR) } >> 16) & 0xFF) as u8;
            let max_func: u8 = if (hdr0 & 0x80) != 0 { 7 } else { 0 };
            for func in 0..=max_func {
                if unsafe { read_config_16(bus as u8, dev, func, 0x00) } == 0xFFFF {
                    continue;
                }
                let cr = unsafe { read_config_32(bus as u8, dev, func, CFG_CLASS) };
                let class = ((cr >> 24) & 0xFF) as u8;
                let sub = ((cr >> 16) & 0xFF) as u8;
                if class == CLASS_NETWORK && sub == SUBCLASS_OTHER {
                    count += 1;
                    if first.is_none() {
                        first = Some((bus as u8, dev, func));
                    }
                }
            }
        }
    }
    (first, count)
}

// ── EROM decode helpers ─────────────────────────────────────────────────────────────────────────

/// Well-known BCMA core ids, for the reader. `?` is an honest "we do not name this id" — the
/// numeric id is on the line already and is the deliverable.
///
/// Transcribed from Linux `include/linux/bcma/bcma.h`. The previous table in this file was wrong on
/// nearly every entry past ChipCommon — it named 0x812 "mips-33" and 0x820 "d11(802.11)" when
/// `BCMA_CORE_80211` is **0x812** and `BCMA_CORE_PCIE` is **0x820**. Nothing had run the EROM walk
/// yet, so no capture could catch it; it would have mislabelled every core on the first successful
/// walk and, worse, sent a bring-up stage at the PCIe core while calling it the radio.
///
/// Re-checked entry-by-entry against the header for S1b, which corrected four more: 0x836 was named
/// "shim" (`BCMA_CORE_SHIM` is **0x837**; 0x836 has no name), 0x83D "usb30-host" (it is
/// `USB30_DEV`), 0x83E "arm-ca9" (it is `ARM_CR4`; `ARMCA9` is **0x510**), and the sub-0x800 range —
/// where the ids this walker must recognise as "not a core" live — was absent entirely.
fn core_name(id: u16) -> &'static str {
    match id {
        0x367 => "oob-router",
        0x50B => "ns-chipcommon-b",
        0x510 => "arm-ca9",
        0x5DC => "4706-mac-gbit-common",
        0x700 => "invalid",
        0x800 => "chipcommon",
        0x801 => "iline20",
        0x802 => "sram",
        0x803 => "sdram",
        0x804 => "pci",
        0x805 => "mips",
        0x806 => "ethernet",
        0x807 => "v90",
        0x808 => "usb11-hostdev",
        0x809 => "adsl",
        0x80A => "iline100",
        0x80B => "ipsec",
        0x80C => "utopia",
        0x80D => "pcmcia",
        0x80E => "internal-mem",
        0x80F => "memc-sdram",
        0x810 => "ofdm",
        0x811 => "extif",
        0x812 => "d11(802.11)",
        0x813 => "phy-a",
        0x814 => "phy-b",
        0x815 => "phy-g",
        0x816 => "mips-3302",
        0x817 => "usb11-host",
        0x818 => "usb11-dev",
        0x819 => "usb20-host",
        0x81A => "usb20-dev",
        0x81B => "sdio-host",
        0x81C => "roboswitch",
        0x81D => "para-ata",
        0x81E => "sata-xordma",
        0x81F => "ethernet-gbit",
        0x820 => "pcie",
        0x821 => "phy-n",
        0x822 => "sram-ctl",
        0x823 => "mini-macphy",
        0x824 => "arm-1176",
        0x825 => "arm-7tdmi",
        0x826 => "phy-lp",
        0x827 => "pmu",
        0x828 => "phy-ssn",
        0x829 => "sdio-dev",
        0x82A => "arm-cm3",
        0x82B => "phy-ht",
        0x82C => "mips-74k",
        0x82D => "mac-gbit",
        0x82E => "ddr12-mem-ctl",
        0x82F => "pcie-rc",
        0x830 => "ocp-ocp-bridge",
        0x831 => "shared-common",
        0x832 => "ocp-ahb-bridge",
        0x833 => "spi-host",
        0x834 => "i2s",
        0x835 => "sdr-ddr1-mem-ctl",
        0x837 => "shim",
        0x83B => "phy-ac",
        0x83C => "pcie2",
        0x83D => "usb30-dev",
        0x83E => "arm-cr4",
        0x840 => "gci",
        0x846 => "cmem",
        0x847 => "arm-ca7",
        0x849 => "sys-mem",
        0xFFF => "default(placeholder)",
        _ => "?",
    }
}

/// Manufacturer id → name. Broadcom's own backplane cores read `0x4bf`; the ARM cores Broadcom
/// licenses (CM3 on a 4331) read `0x43b`, and a component that reads neither is a decode smell
/// worth seeing on the line rather than a number the reader has to look up.
fn manuf_name(m: u16) -> &'static str {
    match m {
        MANUF_ARM => "arm",
        MANUF_MIPS => "mips",
        MANUF_BCM => "bcm",
        _ => "?",
    }
}

/// Cores that legitimately carry NO wrapper. Linux refuses to treat a wrapper-less component as a
/// core except for this exact list (`bcma_get_next_core`), because a wrapper-less component is
/// normally a bus artefact rather than something a driver can reset and drive.
fn core_needs_no_wrapper(id: u16) -> bool {
    matches!(id, 0x5DC /* 4706 mac-gbit common */ | 0x50B /* ns chipcommon b */ | 0x827 /* pmu */ | 0x840 /* gci */)
}

/// Render a raw backplane window address as a core (or wrapper) index on the standard grid.
///
/// Returns `None` when the address is not on the grid at all, which is itself information: a window
/// pointing somewhere off-grid is not a core selection this file can name, and it says so instead of
/// dividing anyway and printing a plausible index.
fn grid_index(addr: u32, base: u32) -> Option<u32> {
    if addr < base {
        return None;
    }
    let off = addr - base;
    if (off & (BCMA_CORE_SIZE - 1)) != 0 {
        return None;
    }
    let idx = off / BCMA_CORE_SIZE;
    if idx < BCMA_MAX_NR_CORES {
        Some(idx)
    } else {
        None
    }
}

/// Address-descriptor port type.
fn addr_type_name(t: u32) -> &'static str {
    match t {
        AD_TYPE_SLAVE => "slave",
        AD_TYPE_BRIDGE => "bridge",
        AD_TYPE_SWRAP => "swrap",
        AD_TYPE_MWRAP => "mwrap",
        _ => "?",
    }
}

/// A bounded, **pushback-capable** cursor over the enumeration ROM.
///
/// Pushback is the property the previous walker lacked and the reason Boot AJ found zero cores.
/// An EROM is not a fixed-arity record: a component declares how many PORTS it has, not how many
/// address descriptors, and each port carries one or more descriptors terminated only by the next
/// descriptor failing to match that port and type. The only way to read it is to look at the next
/// word, consume it if it matches, and leave the cursor untouched if it does not — Linux's
/// `bcma_erom_get_ent` / `bcma_erom_push_ent` pair. Here that is expressed directly: [`Erom::peek`]
/// never advances, and every accessor advances only on a match.
///
/// Both bounds live in the cursor rather than in the walk, so no accessor can read past them: the
/// structural [`EROM_MAX_ENTRIES`] (the 4 KiB window's dword count) and a wall-clock [`Deadline`].
/// Once either fires, `overrun` names it and every subsequent read returns `None`.
struct Erom<F: Fn(u32) -> u32> {
    read: F,
    /// Cursor, in dwords from the EROM base.
    i: u32,
    dl: Deadline,
    /// Empty until a bound (or an all-ones read) stopped the walk; then the name of that bound.
    overrun: &'static str,
}

impl<F: Fn(u32) -> u32> Erom<F> {
    fn new(read: F) -> Self {
        Erom { read, i: 0, dl: Deadline::new(), overrun: "" }
    }

    /// True once a bound has fired. Distinguishes "this entry is not what I asked for" (a normal,
    /// expected pushback) from "the walk cannot continue" — which is the distinction that decides
    /// whether a `None` below means *end of a port's descriptors* or *stop and report*.
    fn stopped(&self) -> bool {
        !self.overrun.is_empty()
    }

    /// The word at the cursor WITHOUT consuming it.
    fn peek(&mut self) -> Option<u32> {
        if self.i >= EROM_MAX_ENTRIES {
            self.overrun = "entry-cap";
            return None;
        }
        if self.dl.expired() {
            self.overrun = "tsc-deadline";
            return None;
        }
        let v = (self.read)(self.i);
        if v == ER_BAD {
            self.overrun = "all-ones";
            return None;
        }
        Some(v)
    }

    /// Consume the word at the cursor unconditionally. Used only for the continuation dwords of a
    /// descriptor already accepted (a 64-bit address half, a size descriptor), which are raw data
    /// and carry no tag of their own.
    fn take(&mut self) -> Option<u32> {
        let v = self.peek()?;
        self.i += 1;
        Some(v)
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

    /// Is the cursor sitting on the end sentinel? Non-consuming.
    fn at_end(&mut self) -> bool {
        matches!(self.peek(), Some(e) if e == ER_END_WORD)
    }

    /// One master-port descriptor. Consumes on match only.
    fn mst_port(&mut self) -> Option<u32> {
        let ent = self.peek()?;
        if (ent & ER_VALID) == 0 || (ent & ER_TAG) != ER_TAG_MP {
            return None;
        }
        self.i += 1;
        Some(ent)
    }

    /// Is the cursor sitting on a BRIDGE address descriptor? Non-consuming
    /// (Linux `bcma_erom_is_bridge`). A bridge component is a backplane artefact, not a core.
    fn at_bridge(&mut self) -> bool {
        match self.peek() {
            Some(e) => {
                (e & ER_VALID) != 0
                    && (e & ER_TAGX) == ER_TAG_ADDR
                    && ((e & AD_TYPE_MASK) >> AD_TYPE_SHIFT) == AD_TYPE_BRIDGE
            }
            None => false,
        }
    }

    /// One address descriptor of exactly `want_type` on exactly `want_port`, returning its base
    /// address; `None` (cursor unmoved) when the next entry is anything else.
    ///
    /// The port match is not decoration: it is the ONLY terminator a port's descriptor list has.
    /// Linux's `bcma_erom_get_addr_desc` checks valid, tag (with the narrow [`ER_TAGX`]), type AND
    /// port for exactly this reason.
    fn addr_desc(&mut self, want_type: u32, want_port: u32) -> Option<u64> {
        let ent = self.peek()?;
        if (ent & ER_VALID) == 0
            || (ent & ER_TAGX) != ER_TAG_ADDR
            || ((ent & AD_TYPE_MASK) >> AD_TYPE_SHIFT) != want_type
            || ((ent & AD_PORT_MASK) >> AD_PORT_SHIFT) != want_port
        {
            return None;
        }
        self.i += 1;
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
        Some(addr)
    }

    /// Advance to the next component identifier (or the end sentinel) without consuming it —
    /// Linux `bcma_erom_skip_component`. Used when a component turns out not to be a core: its
    /// remaining descriptors must still be stepped over, or every core after it decodes as noise.
    fn skip_component(&mut self) {
        loop {
            let ent = match self.peek() {
                Some(e) => e,
                None => return,
            };
            if ent == ER_END_WORD || ((ent & ER_VALID) != 0 && (ent & ER_TAG) == ER_TAG_CI) {
                return;
            }
            self.i += 1;
        }
    }
}

/// Everything the EROM walk learned about the 802.11 core, carried out of the walk so a later stage
/// can point the window at it without re-walking.
///
/// `wrap` is Linux's `core->wrap` choice — the MASTER wrapper when the core has one, else the slave
/// wrapper — and it is carried SEPARATELY from `mwrap`/`swrap` because the two are not
/// interchangeable and because the previous `erom-d11` line got this exactly wrong: it printed
/// `swrap` as the cfg:0xac target, and on this part the d11 core has `nsw=0`, so that line advised
/// writing `0x00000000` into the wrapper selector. The value firmware itself left in cfg:0xac —
/// `0x18101000` — is the MASTER wrapper.
#[derive(Clone, Copy)]
struct D11 {
    base: u64,
    wrap: u64,
    wrap_kind: &'static str,
    mwrap: u64,
    swrap: u64,
    rev: u32,
}

/// Walk the enumeration ROM through an already-open window and print one line per core.
///
/// `read` yields the dword at EROM entry index `i`. It is a closure so the walker is independent of
/// HOW the EROM was reached: S0 can only reach it when firmware happens to have parked the BAR0
/// window on it, while S1 points cfg:0x80 at it deliberately.
///
/// The structure follows Linux `bcma_bus_scan` / `bcma_get_next_core` step for step, because the
/// EROM's arity is data-dependent and any shortcut desynchronises the cursor:
///
/// 1. CIA + CIB (both tagged CI) — identity, revision, port and wrapper counts;
/// 2. reject non-cores: an ARM `0xFFF` placeholder, a component with **no slave port**, a
///    wrapper-less component that is not one of the four ids allowed to be wrapper-less, and a
///    bridge — each SKIPPED to the next component rather than parsed;
/// 3. `nmp` master-port descriptors;
/// 4. the first slave address descriptor on port 0 — the core's register base, the value a driver
///    puts in `BCMA_PCI_BAR0_WIN`;
/// 5. every remaining slave-port descriptor, then the master wrappers, then the slave wrappers,
///    each read until the port/type stops matching.
///
/// Bounded twice over by the cursor and it reports which bound stopped it. Returns the number of
/// cores printed and, when it found one, the 802.11 core's addresses.
fn walk_erom<F: Fn(u32) -> u32>(read: F) -> (u32, Option<D11>, Option<u32>) {
    let mut e = Erom::new(read);
    let mut cores = 0u32;
    let mut skipped = 0u32;
    let mut stop = "end-tag";
    // The deliverable of this whole arc: where the 802.11 core lives.
    let mut d11: Option<D11> = None;
    // The ChipCommon (core index 0) revision, carried out for S2's SPROM-offset choice. It is the
    // CORE rev (EROM CIB), which is what Linux `bcma_sprom_get` keys the shadow offset on — NOT the
    // chip rev in `chipid[19:16]`.
    let mut cc_rev: Option<u32> = None;

    loop {
        // ── 1. CIA + CIB ────────────────────────────────────────────────────────────────────────
        let cia = match e.ci() {
            Some(v) => v,
            None => {
                // The ONE clean exit: the cursor is on the end sentinel and `stop` keeps its
                // initial "end-tag". Anything else overwrites it, so a walk that ends any other way
                // cannot be mistaken for a complete one.
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

        // ── 2. the not-a-core filters ───────────────────────────────────────────────────────────
        let reject = if mfg == MANUF_ARM && id == CORE_ID_DEFAULT {
            "arm-placeholder"
        } else if nsp == 0 {
            "no-slave-port"
        } else if nmw + nsw == 0 && !core_needs_no_wrapper(id) {
            "no-wrapper"
        } else if e.at_bridge() {
            "bridge"
        } else {
            ""
        };
        if !reject.is_empty() {
            // A skipped component is PRINTED. It is not noise: it is the difference between "this
            // chip has 7 cores" and "this chip has 7 cores and 2 components we chose not to call
            // cores", and a reader comparing our count against Linux's needs to see which.
            serial_println!(
                ":: bcma: erom-skip id={:#05x} ({}) mfg={:#05x}({}) rev={} reason={} nsp={} nmp={} nsw={} nmw={} ::",
                id, core_name(id), mfg, manuf_name(mfg), rev, reject, nsp, nmp, nsw, nmw
            );
            skipped += 1;
            e.skip_component();
            if e.stopped() {
                stop = e.overrun;
                break;
            }
            continue;
        }

        // ── 3. master ports (no address; consumed so the addresses below land at the right index)
        let mut m = 0u32;
        while m < nmp {
            if e.mst_port().is_none() {
                break;
            }
            m += 1;
        }
        if m != nmp {
            stop = if e.stopped() { e.overrun } else { "mp-malformed" };
            serial_println!(
                ":: bcma: erom-abort at entry {} consuming master port {}/{} of core id={:#05x} ({}) cia={:#010x} cib={:#010x} ::",
                e.i, m, nmp, id, core_name(id), cia, cib
            );
            break;
        }

        // ── 4. the core's register base: first slave address descriptor, port 0 ─────────────────
        let base = match e.addr_desc(AD_TYPE_SLAVE, 0) {
            Some(a) => a,
            None => {
                stop = if e.stopped() { e.overrun } else { "no-slave-addr" };
                // Carry the raw CIA/CIB words, as the mp-malformed abort does: this is the abort a
                // CIB count-mask bug fires (a too-small nmp under-consumes the master ports, so the
                // real master-port descriptor lands here in place of the slave address), and the
                // predictions doc names these raw words as the lever for a second failed boot.
                serial_println!(
                    ":: bcma: erom-abort at entry {} — core id={:#05x} ({}) declares nsp={} nmp={} but its next entry is not a {} address descriptor on port 0; cia={:#010x} cib={:#010x} ::",
                    e.i, id, core_name(id), nsp, nmp, addr_type_name(AD_TYPE_SLAVE), cia, cib
                );
                break;
            }
        };

        // ── 5. the remaining slave-port descriptors, then the wrappers ──────────────────────────
        let mut extra_slave = 0u32;
        let mut p = 0u32;
        while p < nsp && !e.stopped() {
            while e.addr_desc(AD_TYPE_SLAVE, p).is_some() {
                extra_slave += 1;
            }
            p += 1;
        }

        let mut mwrap: u64 = 0;
        let mut w = 0u32;
        while w < nmw && !e.stopped() {
            let mut j = 0u32;
            while let Some(a) = e.addr_desc(AD_TYPE_MWRAP, w) {
                if w == 0 && j == 0 {
                    mwrap = a;
                }
                j += 1;
            }
            w += 1;
        }

        // Linux's slave-wrapper port numbering: a component with more than one slave PORT numbers
        // its slave wrappers from 1, not 0 (`u8 hack = (ports[1] == 1) ? 0 : 1;`). Getting this
        // wrong does not corrupt the walk — the port match simply never fires and the wrapper reads
        // as absent — which is precisely the kind of silent hole this walk exists to close.
        let hack = if nsp == 1 { 0 } else { 1 };
        let mut swrap: u64 = 0;
        let mut s = 0u32;
        while s < nsw && !e.stopped() {
            let mut j = 0u32;
            while let Some(a) = e.addr_desc(AD_TYPE_SWRAP, s + hack) {
                if s == 0 && j == 0 {
                    swrap = a;
                }
                j += 1;
            }
            s += 1;
        }

        // `wrap` as Linux computes `core->wrap`: the master wrapper when there is one, else the
        // slave wrapper. BOTH are printed, because they are not interchangeable — the SLAVE wrapper
        // is where IOCTL/RESET_CTL live, and it is the one cfg:0xAC must carry to take a core out
        // of reset. A bring-up stage handed a master wrapper looks exactly like a core that will
        // not wake up.
        let (wrap, wrap_kind) = if mwrap != 0 {
            (mwrap, addr_type_name(AD_TYPE_MWRAP))
        } else if swrap != 0 {
            (swrap, addr_type_name(AD_TYPE_SWRAP))
        } else {
            (0, "none")
        };

        if id == CORE_ID_80211 && d11.is_none() {
            d11 = Some(D11 { base, wrap, wrap_kind, mwrap, swrap, rev });
        }
        if id == CORE_ID_CHIPCOMMON && cc_rev.is_none() {
            cc_rev = Some(rev);
        }

        if cores < EROM_MAX_CORES {
            serial_println!(
                ":: bcma: core[{}] id={:#05x} ({}) rev={} mfg={:#05x}({}) class={} base={:#x} wrap={:#x}({}) mwrap={:#x} swrap={:#x} nsp={} nmp={} nsw={} nmw={} extra-slave-addr={} ::",
                cores, id, core_name(id), rev, mfg, manuf_name(mfg), class, base, wrap, wrap_kind,
                mwrap, swrap, nsp, nmp, nsw, nmw, extra_slave
            );
        }
        cores += 1;
        if e.stopped() {
            stop = e.overrun;
            break;
        }
    }

    let (ev, eu) = fmt_dur(e.dl.elapsed_cycles());
    serial_println!(
        ":: bcma: erom-walk cores={} skipped={} entries={} stop={} elapsed={}{} ::",
        cores, skipped, e.i, stop, ev, eu
    );
    // The arc's product, on its own awk-able line: the backplane address of the 802.11 core.
    //
    // The cfg:0xac advice is taken from `wrap` — Linux's `core->wrap`, i.e. the MASTER wrapper when
    // the core has one — and NOT from `swrap`. Boots AL/AM/AN printed `swrap=0x00000000` on this
    // exact core (it declares `nsw=0 nmw=1`), so the previous form of this line advised writing
    // `0x00000000` into the wrapper selector: it would have pointed the wrapper aperture at the
    // bottom of the backplane instead of at the radio, and the value firmware itself left in
    // cfg:0xac (`0x18101000`) is precisely the `mwrap` printed here. All three addresses are on the
    // line so the advice can be checked against them.
    match d11 {
        Some(d) => serial_println!(
            ":: bcma: erom-d11 FOUND id={:#05x} rev={} base={:#010x} wrap={:#010x}({}) mwrap={:#010x} swrap={:#010x} — cfg:0x80 <- {:#010x} puts the radio's registers at BAR0+0; cfg:0xac <- {:#010x} (the WRAP address — master wrapper when the core has one, NOT swrap) puts its reset/clock control at BAR0+0x1000 ::",
            CORE_ID_80211, d.rev, d.base, d.wrap, d.wrap_kind, d.mwrap, d.swrap,
            d.base as u32, d.wrap as u32
        ),
        None => serial_println!(
            ":: bcma: erom-d11 ABSENT — no core id={:#05x} (802.11) in the inventory above after {} cores and {} skips; the radio's backplane base is NOT known and must not be guessed ::",
            CORE_ID_80211, cores, skipped
        ),
    }
    (cores, d11, cc_rev)
}

// ── The probe ───────────────────────────────────────────────────────────────────────────────────

/// Entry point, called once from `arch::x86_64::pci::init` under the `bcmarecon` feature.
///
/// Runs the read-only reconnaissance ([`recon_readonly`]) unconditionally, then — only when the
/// `bcmaS1` knob is armed on top of it — the S1 window stage ([`s1_window_walk`]), which is the WiFi
/// path's FIRST config-space WRITE. S1 rides on recon: it reuses this module's device-find and
/// BAR-map helpers, and it does not exist in the binary unless `bcmaS1` is set (which itself pulls in
/// `bcmarecon`, so the module and this call site are always present when S1 is). The read-only pass
/// is never removed or skipped — S0's `REFUSED … window-elsewhere` line is its own deliverable, and
/// S1 prints a second, clearly-prefixed (`bcma-s1:`) block after it.
pub fn recon() {
    recon_readonly();
    #[cfg(feature = "bcmaS1")]
    s1_window_walk();
}

/// BCMA-RECON. Read-only; the S0 reconnaissance pass. Writes nothing.
fn recon_readonly() {
    let dl = Deadline::new();
    serial_println!(
        ":: bcma: begin — READ-ONLY recon of PCI class 0x02/sub 0x80 (config reads + BAR0 reads; no config write, no register write, no BAR sizing) ::"
    );

    // ── Stage 0: is there a radio at all? ───────────────────────────────────────────────────────
    let (found, matches) = find_wifi();
    let (bus, dev, func) = match found {
        Some(b) => b,
        None => {
            // This is the honest QEMU reading, and it is also the reading that would REFUTE the
            // metal prediction. It is not an error and it is not silence.
            serial_println!(
                ":: bcma: no-device — no PCI function reports class 0x02 subclass 0x80 on this machine; nothing to recon (expected under QEMU, which models no BCM4331) ::"
            );
            let (ev, eu) = fmt_dur(dl.elapsed_cycles());
            serial_println!(":: bcma: end ok=0 stage=find reason=no-device elapsed={}{} ::", ev, eu);
            return;
        }
    };

    let vend = unsafe { read_config_16(bus, dev, func, 0x00) };
    let devid = unsafe { read_config_16(bus, dev, func, 0x02) };
    let cr = unsafe { read_config_32(bus, dev, func, CFG_CLASS) };
    let ss = unsafe { read_config_32(bus, dev, func, CFG_SUBSYS) };
    let intr = unsafe { read_config_32(bus, dev, func, CFG_INTR) };
    let cmd_sts = unsafe { read_config_32(bus, dev, func, CFG_CMD_STS) };
    let command = (cmd_sts & 0xFFFF) as u16;
    let pin = ((intr >> 8) & 0xFF) as u8;
    serial_println!(
        ":: bcma: device bdf {}:{}.{} {:04x}:{:04x} rev={:02x} ssid={:04x}:{:04x} irq={} pin={} cmd={:#06x} (mem-decode={} busmaster={}) matches={} ::",
        bus, dev, func, vend, devid, (cr & 0xFF) as u8,
        (ss & 0xFFFF) as u16, (ss >> 16) as u16,
        (intr & 0xFF) as u8,
        if pin == 0 { '-' } else { (b'A' + pin.saturating_sub(1)) as char },
        command, (command >> 1) & 1, (command >> 2) & 1, matches
    );

    // The capability list is what a driver arc reads first (MSI? PCIe endpoint?). Reuse the
    // existing `[PCI-PROBE]` dump rather than writing a second capability walker.
    crate::drivers::pci::PciScanner::probe_irq_caps(bus, dev, func);

    // ── Stage 1: refuse politely on a part that is not a Broadcom backplane device ──────────────
    if vend != VENDOR_BROADCOM {
        serial_println!(
            ":: bcma: REFUSED stage=vendor reason=not-broadcom vendor={:#06x} — identity above is the finding; BCMA decode applies only to 0x14e4 parts ::",
            vend
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma: end ok=0 stage=vendor elapsed={}{} ::", ev, eu);
        return;
    }

    // ── Stage 2: power state. An MMIO read of a D3hot function returns undefined data. ──────────
    // `pm <= 0xF8` is a correctness guard, not paranoia: PMCSR is at `pm + 4`, and a capability
    // header at 0xFC would make that sum wrap a `u8` back to 0x00 in a release build — reading the
    // VENDOR ID and reporting it as a power state. A capability whose PMCSR does not fit in config
    // space is malformed, and the honest handling is to treat it as absent.
    let pm = {
        let p = unsafe { find_cap(bus, dev, func, CAP_ID_PM) };
        if p <= 0xF8 { p } else { 0 }
    };
    let pstate = if pm != 0 {
        let pmcsr = unsafe { read_config_32(bus, dev, func, pm + 4) };
        let d = (pmcsr & 0x3) as u8;
        // FULL decode, because "state=D0" alone leaves a reader unable to check the claim. PCI PM
        // 1.2 PMCSR: [1:0] PowerState, [3] NoSoftReset, [8] PME_En, [12:9] Data_Select,
        // [14:13] Data_Scale, [15] PME_Status. Boot AF read 0x4008 = D0, NoSoftReset=1, PME_En=0,
        // PME_Status=0, Data_Scale=2 — i.e. the function is awake and has no pending wake event,
        // which is why "parked in D3" is NOT the explanation for the all-ones BAR0 reads.
        serial_println!(
            ":: bcma: power cap@{:#04x} pmcsr={:#06x} state=D{} no-soft-reset={} pme_en={} pme_status={} data_sel={} data_scale={} ::",
            pm, (pmcsr & 0xFFFF) as u16, d,
            (pmcsr >> 3) & 1, (pmcsr >> 8) & 1, (pmcsr >> 15) & 1,
            (pmcsr >> 9) & 0xF, (pmcsr >> 13) & 0x3
        );
        d
    } else {
        // No PM capability is a legal (if unusual) reading, and it is NOT the same as D0. Say which.
        serial_println!(":: bcma: power cap=absent — no PM capability; D-state unknown, treated as D0 ::");
        0
    };
    if pstate != 0 {
        serial_println!(
            ":: bcma: REFUSED stage=power reason=power-state d={} reg=cfg:{:#04x}+4(PMCSR) — waking the function is a WRITE; not in this arc ::",
            pstate, pm
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma: end ok=0 stage=power elapsed={}{} ::", ev, eu);
        return;
    }

    // ── Stage 3: memory decode. Without COMMAND bit 1 the BAR does not answer. ──────────────────
    if command & 0x0002 == 0 {
        serial_println!(
            ":: bcma: REFUSED stage=decode reason=mem-decode-off reg=cfg:0x04.bit1 cmd={:#06x} — every BAR0 read below would return all-ones; enabling decode is a WRITE, not in this arc ::",
            command
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma: end ok=0 stage=decode elapsed={}{} ::", ev, eu);
        return;
    }

    // ── Stage 4: BAR0. Raw decode only — no sizing (that needs a write/restore dance). ──────────
    let bar0_raw = unsafe { read_config_32(bus, dev, func, CFG_BAR0) };
    if bar0_raw == 0 || (bar0_raw & 1) != 0 {
        serial_println!(
            ":: bcma: REFUSED stage=bar0 reason={} bar0={:#010x} — a BCMA part's register window is a MEMORY BAR0; nothing to map ::",
            if bar0_raw == 0 { "bar0-unassigned" } else { "bar0-is-io" }, bar0_raw
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma: end ok=0 stage=bar0 elapsed={}{} ::", ev, eu);
        return;
    }
    let is64 = (bar0_raw & 0x6) == 0x4;
    let pf = (bar0_raw >> 3) & 1;
    let bar0: u64 = if is64 {
        let hi = unsafe { read_config_32(bus, dev, func, CFG_BAR1) } as u64;
        ((bar0_raw & 0xFFFF_FFF0) as u64) | (hi << 32)
    } else {
        (bar0_raw & 0xFFFF_FFF0) as u64
    };
    let bar1_raw = if is64 { unsafe { read_config_32(bus, dev, func, CFG_BAR1) } } else { 0 };
    let win = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let win2 = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN2) };
    // Which core the two windows are pointed at, rendered as an index on the standard grid. This is
    // the line Boot AF needed and did not have: `0x18001000` looks like "ChipCommon, off by a page"
    // to a reader who does not have the grid in their head, and it is nothing of the sort — it is a
    // different core entirely.
    let win_idx = grid_index(win, BCMA_ADDR_BASE);
    let win2_idx = grid_index(win2, BCMA_WRAP_BASE);
    serial_println!(
        ":: bcma: bar0={:#x} raw={:#010x} bar1_raw={:#010x} type={} prefetch={} maplen={:#x} (core-win bar0+0x0, wrap-win bar0+{:#x}) bar0_win(cfg:0x80)={:#010x} bar0_win2(cfg:0xac)={:#010x} ::",
        bar0, bar0_raw, bar1_raw, if is64 { "mem64" } else { "mem32" }, pf, BAR0_MAP_LEN,
        BAR0_WRAP_OFF, win, win2
    );
    serial_print!(":: bcma: window-decode core-index=");
    match win_idx {
        Some(a) => {
            serial_print!("{}", a);
        }
        None => {
            serial_print!("off-grid");
        }
    }
    serial_print!(" wrap-index=");
    match win2_idx {
        Some(b) => {
            serial_print!("{}", b);
        }
        None => {
            serial_print!("off-grid");
        }
    }
    serial_println!(
        " paired={} on-enum-base={} grid(core {:#010x}+i*{:#x}, wrap {:#010x}+i*{:#x}) — BAR0+0 decodes to backplane {:#010x}, BAR0+{:#x} to {:#010x} ::",
        (win_idx.is_some() && win_idx == win2_idx) as u8,
        (win == BCMA_ADDR_BASE) as u8,
        BCMA_ADDR_BASE, BCMA_CORE_SIZE, BCMA_WRAP_BASE, BCMA_CORE_SIZE,
        win, BAR0_WRAP_OFF, win2
    );

    // Identity-map the register block uncacheable. This is a page-table edit; the device sees
    // nothing. Same seam as `sdhc::probe` and the GPU BARs.
    crate::arch::memory::map_mmio_window(bar0, BAR0_MAP_LEN);
    if crate::arch::memory::translate(bar0).is_none() {
        serial_println!(
            ":: bcma: REFUSED stage=map reason=bar0-unmapped bar0={:#x} — the window is not present in the live page tables after map_mmio_window; reading it would fault ::",
            bar0
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma: end ok=0 stage=map elapsed={}{} ::", ev, eu);
        return;
    }

    // ── Stage 4b: the WRAPPER window — read-only, and the discriminator for an all-ones core ────
    //
    // BAR0's second 4 KiB is the AXI/OOB wrapper of whatever core cfg:0xAC selects. The wrapper is
    // powered and readable while its core is held in reset (that is its job — it is where the reset
    // is released from), so it separates "the window points at a dark core" from "the function is
    // not answering". Every offset read here is a status/control register with no read side effect.
    let wr_ioctl = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
    let wr_iost = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOST) };
    let wr_rstc = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_RESET_CTL) };
    let wr_rsts = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_RESET_ST) };
    let wrap_dead = wr_ioctl == 0xFFFF_FFFF
        && wr_iost == 0xFFFF_FFFF
        && wr_rstc == 0xFFFF_FFFF
        && wr_rsts == 0xFFFF_FFFF;
    // Linux `bcma_core_is_enabled()`: clocked (CLK set, FGC clear) AND not in reset.
    let clk = (wr_ioctl & IOCTL_CLK) != 0;
    let fgc = (wr_ioctl & IOCTL_FGC) != 0;
    let in_reset = (wr_rstc & RESET_CTL_RESET) != 0;
    let gated = (wr_iost & IOST_GATED_CLK) != 0;
    let core_enabled = !wrap_dead && clk && !fgc && !in_reset;
    serial_println!(
        ":: bcma: wrapper win2={:#010x} at bar0+{:#x}: ioctl={:#010x} iost={:#010x} resetctl={:#010x} resetst={:#010x} ::",
        win2, BAR0_WRAP_OFF, wr_ioctl, wr_iost, wr_rstc, wr_rsts
    );
    if wrap_dead {
        // Both apertures silent. The core's reset state cannot be the explanation, because the
        // block that survives reset is silent too. Blame is upstream of the function.
        serial_println!(
            ":: bcma: wrapper-verdict all-ones — the WRAPPER aperture is silent as well, so a core held in reset does NOT explain this; the function or the path to it (link down, or the bridge above not forwarding this address) is the remaining explanation ::"
        );
    } else {
        serial_println!(
            ":: bcma: wrapper-verdict answering clk={} fgc={} gated-clk={} in-reset={} core-enabled={} (Linux bcma_core_is_enabled) — the function DOES decode; whatever the core window reads is a property of the selected core, not of the function ::",
            clk as u8, fgc as u8, gated as u8, in_reset as u8, core_enabled as u8
        );
    }

    // ── Stage 5: ChipCommon — the one core reachable without moving the window ──────────────────
    //
    // ORDER MATTERS, and Boot AF is why. The window test comes FIRST: it is a statement about
    // cfg:0x80 that is true or false without reading a single MMIO byte, whereas "all-ones" is only
    // meaningful once BAR0+0 is known to be ChipCommon at all. With the tests the other way round,
    // an off-window core that happens to be in reset reads all-ones and gets reported as
    // `not-decoding` — a verdict about the FUNCTION drawn from evidence about a core we never meant
    // to be looking at. That is exactly what this file printed on Boot AF.
    let raw_at_bar0 = unsafe { r32(bar0, CC_ID) };
    if win != BCMA_ADDR_BASE {
        serial_println!(
            ":: bcma: REFUSED stage=chipcommon reason=window-elsewhere bar0_win={:#010x} expected={:#010x} raw@bar0+0={:#010x} — BAR0+0 is NOT ChipCommon on this reading; reaching it needs ONE 32-bit config WRITE of {:#010x} to cfg:0x80 (Linux bcma_scan_switch_core does exactly this, unconditionally, at scan start), which this arc does not make ::",
            win, BCMA_ADDR_BASE, raw_at_bar0, BCMA_ADDR_BASE
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma: end ok=0 stage=chipcommon elapsed={}{} ::", ev, eu);
        return;
    }
    let chipid = raw_at_bar0;
    if chipid == 0xFFFF_FFFF {
        serial_println!(
            ":: bcma: REFUSED stage=chipcommon reason=not-decoding chipid=0xffffffff — the window IS on the enumeration base and ChipCommon still reads all-ones (function not decoding, or the window is dead) ::"
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma: end ok=0 stage=chipcommon elapsed={}{} ::", ev, eu);
        return;
    }

    let cc_id = (chipid & 0xFFFF) as u16;
    let cc_rev = (chipid >> 16) & 0xF;
    let cc_pkg = (chipid >> 20) & 0xF;
    let cc_ncores = (chipid >> 24) & 0xF;
    let cc_type = (chipid >> 28) & 0xF;
    let cap = unsafe { r32(bar0, CC_CAP) };
    let cap_ext = unsafe { r32(bar0, CC_CAP_EXT) };
    let chipst = unsafe { r32(bar0, CC_CHIPSTATUS) };
    let erom = unsafe { r32(bar0, CC_EROM) };

    // RAW first, decode second — a decode bug must never be able to hide the evidence it came from.
    serial_println!(
        ":: bcma: cc-raw chipid={:#010x} cap={:#010x} cap_ext={:#010x} chipstatus={:#010x} erom={:#010x} ::",
        chipid, cap, cap_ext, chipst, erom
    );
    serial_println!(
        ":: bcma: chip id={:#06x} rev={} pkg={} ncores={} type={} ({}) pmu={} ::",
        cc_id, cc_rev, cc_pkg, cc_ncores, cc_type,
        match cc_type { 0 => "ssb/sb", 1 => "bcma/erom", 2 => "bcma-single", _ => "?" },
        if cap & CC_CAP_PMU != 0 { 1 } else { 0 }
    );
    if cc_type == 0 {
        serial_println!(
            ":: bcma: note chip-type=0 (SSB) — this part has NO enumeration ROM; core discovery would use the SSB fixed-slot layout instead ::"
        );
    }

    // ── Stage 6: the EROM. Reachable only if it happens to sit inside the live window. ──────────
    //
    // The containment test is the honest form of the question. `bcma_scan_switch_core()` in Linux
    // moves the window by writing cfg:0x80; firmware parks it at the ChipCommon base, and a real
    // EROM sits a 64 KiB region away, so on the rMBP this test is expected to FAIL and print the
    // refusal below. That refusal — with the EROM's own backplane address in it — is the arc's
    // deliverable for this stage: it names the exact register the next stage must write.
    let win_end = (win as u64) + (BCMA_CORE_SIZE as u64);
    if erom != 0 && erom != ER_BAD && (erom as u64) >= (win as u64) && (erom as u64) < win_end {
        let off = (erom as u64) - (win as u64);
        serial_println!(
            ":: bcma: erom {:#010x} IS inside the live window (bar0+{:#x}) — walking read-only ::",
            erom, off
        );
        walk_erom(|i| unsafe { r32(bar0, off + (i as u64) * 4) });
    } else {
        serial_println!(
            ":: bcma: REFUSED stage=erom reason=out-of-window erom={:#010x} window=[{:#010x},{:#010x}) reg=cfg:0x80(BCMA_PCI_BAR0_WIN) — the EROM is on the backplane, not in BAR0; reaching it needs ONE 32-bit config WRITE of {:#010x} to cfg:0x80 (Linux bcma_scan_switch_core), which this arc does not make. Core inventory unavailable; ChipCommon says ncores={} ::",
            erom, win, win_end as u32, erom, cc_ncores
        );
    }

    // ── Stage 7: SPROM or OTP — the board identity a driver cannot calibrate without ────────────
    let otp_field = (cap & CC_CAP_OTPS_MASK) >> CC_CAP_OTPS_SHIFT;
    let otp_words: u32 = if otp_field == 0 { 0 } else { 1u32 << (CC_CAP_OTPS_BASE + otp_field) };
    let otps = unsafe { r32(bar0, CC_OTPS) };
    let otpc = unsafe { r32(bar0, CC_OTPC) };
    let otpl = unsafe { r32(bar0, CC_OTPL) };
    let gup_hw = (otps & OTPS_GUP_HW) != 0;
    let gup_sw = (otps & OTPS_GUP_SW) != 0;
    let gup_ci = (otps & OTPS_GUP_CI) != 0;
    let gup_fuse = (otps & OTPS_GUP_FUSE) != 0;
    let otp_programmed = gup_hw || gup_sw || gup_ci || gup_fuse;
    let sprom_present = (cap & CC_CAP_SPROM) != 0;

    serial_println!(
        ":: bcma: identity sprom={} otp={} otp_words={} otps={:#010x} otpc={:#010x} otpl={:#010x} gup(hw={} sw={} ci={} fuse={}) ::",
        if sprom_present { "present" } else { "absent" },
        if otp_words == 0 { "absent" } else if otp_programmed { "present-programmed" } else { "present-blank" },
        otp_words, otps, otpc, otpl,
        gup_hw as u8, gup_sw as u8, gup_ci as u8, gup_fuse as u8
    );

    if sprom_present {
        // Shadow offset + the REASON for it, from the one shared helper (see `sprom_offset_for`).
        // This used to read `if cc_rev >= 31 { PCIE6 }` with `cc_rev` being the CHIP rev — wrong
        // twice over: the rev-31 test belongs to SPROM presence detection, not offset selection, and
        // the BCM4331 is the part Linux explicitly excludes from the alternate offset. Printing the
        // reason is what makes a wrong offset catchable from a capture.
        let (spoff, spoff_reason) = sprom_offset_for(cc_id);
        let mut all_ff = true;
        let mut all_00 = true;
        let sdl = Deadline::new();
        let mut w = 0u64;
        // 8 words per line, so the dump is 28 lines for a rev-8 SPROM and stays awk-friendly.
        while w < SPROM_WORDS && !sdl.expired() {
            serial_print!(":: bcma: sprom+{:#05x}", spoff + w * 2);
            let mut k = 0u64;
            while k < 8 && w + k < SPROM_WORDS {
                let v = unsafe { r16(bar0, spoff + (w + k) * 2) };
                if v != 0xFFFF { all_ff = false; }
                if v != 0x0000 { all_00 = false; }
                serial_print!(" {:04x}", v);
                k += 1;
            }
            serial_println!(" ::");
            w += 8;
        }
        let last = unsafe { r16(bar0, spoff + (SPROM_WORDS - 1) * 2) };
        let srev = (last & 0xFF) as u8;
        // The MAC candidate ([`SP8_IL0MAC`] = 0x8C), stored big-endian per 16-bit word. This read
        // used the literal `0x4A` — which is not the rev-8 MAC and not even the rev-4 MAC, but
        // `SSB_SPROM4_BFL2HI`/`SSB_SPROM5_BFLLO`, a BOARDFLAGS word. It now shares S2's constant.
        let m0 = unsafe { r16(bar0, spoff + SP8_IL0MAC) };
        let m1 = unsafe { r16(bar0, spoff + SP8_IL0MAC + 2) };
        let m2 = unsafe { r16(bar0, spoff + SP8_IL0MAC + 4) };
        let mac = [
            (m0 >> 8) as u8, (m0 & 0xFF) as u8,
            (m1 >> 8) as u8, (m1 & 0xFF) as u8,
            (m2 >> 8) as u8, (m2 & 0xFF) as u8,
        ];
        // Falsifiable, and cheap: a real station MAC is unicast (bit 0 of byte 0 clear) and is
        // neither all-zero nor all-ones. This is NOT a CRC — the SPROM CRC-8 polynomial is not
        // transcribed in this arc, so validity is reported as unchecked rather than asserted.
        let unicast = (mac[0] & 1) == 0;
        let degenerate = all_ff || all_00
            || (mac == [0, 0, 0, 0, 0, 0]) || (mac == [0xFF; 6]);
        serial_println!(
            ":: bcma: sprom-decode offset={:#05x} offset-reason={} mac-off={:#05x} words={} last={:#06x} rev={} rev-supported={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} unicast={} all_ff={} all_00={} verdict={} (crc NOT computed in this arc; `unicast` alone is a coin flip — S2's wifi-s2: lines carry the OUI check) ::",
            spoff, spoff_reason, SP8_IL0MAC, SPROM_WORDS, last, srev,
            if (8..=11).contains(&srev) { 1 } else { 0 },
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
            unicast as u8, all_ff as u8, all_00 as u8,
            if degenerate { "NO-DATA" } else if unicast && (8..=11).contains(&srev) { "PLAUSIBLE" } else { "SUSPECT" }
        );
        // BCM4331-specific: Linux calls `bcma_chipco_bcm4331_ext_pa_lines_ctl(cc, false)` BEFORE
        // reading the SPROM on this exact chip, because the external PA control lines are muxed
        // onto the SPROM pins. That is a ChipCommon WRITE. Say so on every 4331 reading, so a
        // NO-DATA verdict above is never mistaken for "this board has no SPROM".
        if cc_id == 0x4331 || cc_id == 43431 {
            serial_println!(
                ":: bcma: NOTE stage=sprom chip=4331 — Linux clears the external-PA lines (ChipCommon CTL, a WRITE) before reading this shadow; an unreliable dump above may be that mux, not an empty SPROM. Write not made in this arc ::"
            );
        }
    } else {
        serial_println!(
            ":: bcma: sprom-absent — ChipCommon capabilities bit 30 clear; this board's calibration is in OTP (or nowhere) ::"
        );
    }

    // OTP CONTENTS are not reachable read-only: the OTP read path is a command written to OTPP.
    if otp_words != 0 {
        serial_println!(
            ":: bcma: REFUSED stage=otp reason=read-needs-command reg=cc:{:#06x}(OTPP) words={} programmed={} — OTP contents are fetched by WRITING a read command to OTPP and polling OTPS; this arc reports only the status word above ::",
            CC_OTPP, otp_words, otp_programmed as u8
        );
    } else {
        serial_println!(":: bcma: otp-absent — ChipCommon capabilities OTP-size field is 0; no OTP on this part ::");
    }

    // ── Stage 8: PHY type/revision — behind the D11 core, behind the same one write ─────────────
    //
    // `B43_MMIO_PHY_VER` (0x3E0) lives in the 802.11 core's register window, and that window is
    // reached by putting the D11 core's backplane base into cfg:0x80 — a base that comes from the
    // EROM, which is itself behind that register. One write gates the entire chain, which is why
    // it is reported once, precisely, rather than as three separate disappointments.
    serial_println!(
        ":: bcma: REFUSED stage=phy reason=core-not-in-window reg=cfg:0x80(BCMA_PCI_BAR0_WIN) — PHY_VERSION is d11+0x3e0; reaching the d11 core needs its backplane base (from the EROM) written to cfg:0x80. PHY type/rev UNKNOWN — not guessed ::"
    );

    let (ev, eu) = fmt_dur(dl.elapsed_cycles());
    serial_println!(
        ":: bcma: end ok=1 stage=chipcommon chip={:#06x} rev={} ncores={} erom={:#010x} sprom={} otp_words={} elapsed={}{} — one config write (cfg:0x80) separates this from the full backplane inventory ::",
        cc_id, cc_rev, cc_ncores, erom, sprom_present as u8, otp_words, ev, eu
    );
}

// ── S1: point cfg:0x80 at ChipCommon, read identity + EROM, then RESTORE ──────────────────────────
//
// `UNAOS_BCMAS1=1`. This is the WiFi path's FIRST WRITE, and it is exactly one register: PCI config
// `0x80` (`BCMA_PCI_BAR0_WIN`), the backplane-window selector. On a BCMA part behind PCIe, BAR0+0
// decodes to whatever backplane address sits in cfg:0x80; firmware on the 2012 rMBP parked it at
// `0x18001000` (core index 1, the d11 radio — Boot AF), so ChipCommon and the EROM are unreachable
// read-only. S1 writes cfg:0x80 to `0x18000000` (ChipCommon / `BCMA_ADDR_BASE`) — Linux's
// `bcma_scan_switch_core(bus, BCMA_ADDR_BASE)`, issued unconditionally before its first `CC_ID` read
// — reads the chip identity and the EROM core list (moving the window once more to the EROM base),
// and RESTORES the window to the pre-image it recorded.
//
// The restore discipline is the whole safety case, and Boot AF is why it is not a formality: the
// pre-image on THIS machine is `0x18001000`, **not** the enumeration base `0x18000000`. A stage that
// "restored" to `0x18000000` because that is what it assumed firmware left would silently move the
// radio's window for every later boot stage AND look clean in every log line. The restore pushes the
// value it READ and witnesses MATCH/FAILED against that stored value — never against `0x18000000`,
// which appears nowhere as a restore target.
//
// The unwind is PROVEN before the state-changing write (igpu discipline: a self-test, not a bare
// write). Every read/write is bounded; the EROM walk carries the same structural + TSC bounds as
// S0's. Guarded so the write is issued ONLY against a Broadcom 4331 — QEMU models no such part, so
// there the block stops at `no-device` and no write is ever issued.
#[cfg(feature = "bcmaS1")]
fn s1_window_walk() {
    // BCM4331 PCI device id. Inlined (not a module const) so no unused-const warning exists when the
    // `bcmaS1` knob is off; the guard below is the ONLY gate on the first write, so it is spelled out
    // where it is used.
    const DEVID_BCM4331: u16 = 0x4331;

    let dl = Deadline::new();
    serial_println!(
        ":: bcma-s1: begin — the WiFi path's FIRST WRITE: move cfg:0x80 to ChipCommon (0x18000000), read chip id + EROM, then RESTORE the recorded pre-image (never 0x18000000) ::"
    );

    // Reuse S0's device-find. QEMU models no BCM4331 -> None -> not one config write is issued.
    let (found, matches) = find_wifi();
    let (bus, dev, func) = match found {
        Some(b) => b,
        None => {
            serial_println!(
                ":: bcma-s1: no-device — no class 0x02/sub 0x80 function on this machine; no window write issued (expected under QEMU, which models no BCM4331) ::"
            );
            let (ev, eu) = fmt_dur(dl.elapsed_cycles());
            serial_println!(
                ":: bcma-s1: end ok=0 stage=find reason=no-device wrote-cfg80=0 matches={} elapsed={}{} ::",
                matches, ev, eu
            );
            return;
        }
    };

    // GUARD — the single gate on the first write. cfg:0x80 is a 4331-specific backplane selector;
    // moving it on any other part is exactly the wedge this path exists to avoid, so the write is
    // refused unless BOTH the Broadcom vendor id AND the 4331 device id read back. This is what keeps
    // a stray class-0x02/sub-0x80 function in some other machine (or a future QEMU model) from taking
    // the write.
    let vend = unsafe { read_config_16(bus, dev, func, 0x00) };
    let devid = unsafe { read_config_16(bus, dev, func, 0x02) };
    if vend != VENDOR_BROADCOM || devid != DEVID_BCM4331 {
        serial_println!(
            ":: bcma-s1: REFUSED stage=identity reason=not-4331 vendor={:#06x} device={:#06x} — the cfg:0x80 selector is 4331-specific; NO window write issued ::",
            vend, devid
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(
            ":: bcma-s1: end ok=0 stage=identity wrote-cfg80=0 elapsed={}{} ::",
            ev, eu
        );
        return;
    }

    // S1 writes ONLY cfg:0x80. It does NOT wake the function (a PMCSR write) or enable memory decode
    // (a COMMAND write) — those are OTHER writes S0 refuses and S1 must not make either. Re-check
    // both read-only; if either does not already hold, the window write below would be pointless (a
    // moved window that BAR0 will not answer), so refuse without writing.
    let pm = {
        let p = unsafe { find_cap(bus, dev, func, CAP_ID_PM) };
        if p <= 0xF8 { p } else { 0 }
    };
    let pstate = if pm != 0 {
        (unsafe { read_config_32(bus, dev, func, pm + 4) } & 0x3) as u8
    } else {
        0
    };
    let command = (unsafe { read_config_32(bus, dev, func, CFG_CMD_STS) } & 0xFFFF) as u16;
    if pstate != 0 || (command & 0x0002) == 0 {
        serial_println!(
            ":: bcma-s1: REFUSED stage=precond reason={} d={} cmd={:#06x} — S1 writes only cfg:0x80; waking (PMCSR) / enabling decode (COMMAND) are OTHER writes, not this arc's; NO window write issued ::",
            if pstate != 0 { "not-d0" } else { "mem-decode-off" }, pstate, command
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma-s1: end ok=0 stage=precond wrote-cfg80=0 elapsed={}{} ::", ev, eu);
        return;
    }

    // Map BAR0 (a page-table edit; the device sees nothing) so the ChipCommon/EROM reads below land.
    let bar0_raw = unsafe { read_config_32(bus, dev, func, CFG_BAR0) };
    if bar0_raw == 0 || (bar0_raw & 1) != 0 {
        serial_println!(
            ":: bcma-s1: REFUSED stage=bar0 reason={} bar0={:#010x} — no memory BAR0 to read the window through; NO window write issued ::",
            if bar0_raw == 0 { "bar0-unassigned" } else { "bar0-is-io" }, bar0_raw
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma-s1: end ok=0 stage=bar0 wrote-cfg80=0 elapsed={}{} ::", ev, eu);
        return;
    }
    let is64 = (bar0_raw & 0x6) == 0x4;
    let bar0: u64 = if is64 {
        let hi = unsafe { read_config_32(bus, dev, func, CFG_BAR1) } as u64;
        ((bar0_raw & 0xFFFF_FFF0) as u64) | (hi << 32)
    } else {
        (bar0_raw & 0xFFFF_FFF0) as u64
    };
    crate::arch::memory::map_mmio_window(bar0, BAR0_MAP_LEN);
    if crate::arch::memory::translate(bar0).is_none() {
        serial_println!(
            ":: bcma-s1: REFUSED stage=map reason=bar0-unmapped bar0={:#x} — window not present after map_mmio_window; NO config write issued ::",
            bar0
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma-s1: end ok=0 stage=map wrote-cfg80=0 elapsed={}{} ::", ev, eu);
        return;
    }

    // ── Step 1: record the PRE-IMAGE. This value — whatever firmware actually left — is the ONE
    // restore target. 0x18000000 is never used as a restore value anywhere below. ────────────────
    let pre_win = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let pre_win2 = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN2) };
    serial_println!(
        ":: bcma-s1: pre-image cfg:0x80={:#010x} cfg:0xac={:#010x} — the 0x80 value here is the RESTORE TARGET (on this machine 0x18001000, NOT the enumeration base 0x18000000) ::",
        pre_win, pre_win2
    );

    // ── Step 1b: prove the UNWIND before moving the window. Write the pre-image BACK to itself — a
    // no-op, since it is the value cfg:0x80 already holds, so the window does not move and the device
    // sees no state change — and read it back. This exercises the exact write+readback path the final
    // restore depends on. If it does not round-trip, the config-write path is untrustworthy and the
    // window is left exactly where firmware put it. (igpu discipline: a self-test, not a bare write.)
    unsafe { crate::arch::pci::write_config_32(bus, dev, func, CFG_BAR0_WIN, pre_win) };
    let selftest = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    if selftest != pre_win {
        serial_println!(
            ":: bcma-s1: REFUSED stage=unwind-selftest reason=readback-mismatch wrote={:#010x} readback={:#010x} — the restore path does NOT round-trip; the window has NOT been moved off the firmware pre-image ::",
            pre_win, selftest
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(
            ":: bcma-s1: end ok=0 stage=unwind-selftest wrote-cfg80=1(no-op, in-place) restore=N/A elapsed={}{} ::",
            ev, eu
        );
        return;
    }
    serial_println!(
        ":: bcma-s1: unwind-selftest PASS — wrote the pre-image to itself, read back {:#010x} (window unmoved); the restore path round-trips ::",
        selftest
    );

    // From HERE the window WILL move. There are NO early returns between the move and the restore at
    // the end — every path below falls through to Step 4.

    // ── Step 2: THE WRITE. cfg:0x80 <- 0x18000000 (ChipCommon / BCMA_ADDR_BASE). ──────────────────
    unsafe { crate::arch::pci::write_config_32(bus, dev, func, CFG_BAR0_WIN, BCMA_ADDR_BASE) };
    let win_now = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    serial_println!(
        ":: bcma-s1: WROTE cfg:0x80 old={:#010x} new={:#010x} readback={:#010x} took={} (Linux bcma_scan_switch_core(BCMA_ADDR_BASE)) ::",
        pre_win, BCMA_ADDR_BASE, win_now, (win_now == BCMA_ADDR_BASE) as u8
    );

    // ── Step 3: read ChipCommon through BAR0+0, now that the window points at it. ──────────────────
    let chipid = unsafe { r32(bar0, CC_ID) };
    let cc_id = (chipid & 0xFFFF) as u16;
    let window_confirmed;
    if chipid == 0xFFFF_FFFF {
        // The window is on the enumeration base and ChipCommon STILL reads all-ones. This REFUTES the
        // window hypothesis — moving cfg:0x80 was not the blocker — and is a finding, not a silent
        // pass. Blame moves upstream (PCIe link, or 0:28.1 not forwarding 0xc1900000). Say so, then
        // fall through to the restore.
        window_confirmed = false;
        serial_println!(
            ":: bcma-s1: REFUTED window-hypothesis — cfg:0x80 readback={:#010x} (ChipCommon base) yet BAR0+0 reads 0xffffffff. Moving the window was NOT the blocker; the fault is upstream (link, or the bridge above not forwarding this address). Restoring and reporting. ::",
            win_now
        );
    } else {
        window_confirmed = true;
        let cc_rev = (chipid >> 16) & 0xF;
        let cc_pkg = (chipid >> 20) & 0xF;
        let cc_ncores = (chipid >> 24) & 0xF;
        let cc_type = (chipid >> 28) & 0xF;
        let cap = unsafe { r32(bar0, CC_CAP) };
        let cap_ext = unsafe { r32(bar0, CC_CAP_EXT) };
        let chipst = unsafe { r32(bar0, CC_CHIPSTATUS) };
        let erom = unsafe { r32(bar0, CC_EROM) };
        // RAW first, decode second — a decode bug must not hide the evidence it came from.
        serial_println!(
            ":: bcma-s1: cc-raw chipid={:#010x} cap={:#010x} cap_ext={:#010x} chipstatus={:#010x} erom={:#010x} ::",
            chipid, cap, cap_ext, chipst, erom
        );
        // Field-by-field, with the bit range on every field, so the decode is checkable against the
        // raw word on the line above without a header to hand. Masks are Linux
        // `BCMA_CC_ID_{ID,REV,PKG,NRCORES,TYPE}` verbatim; Boot AJ's 0x13924331 decodes to
        // id=0x4331 rev=2 pkg=9 nrcores=3 type=1, and every one of those is what the register says.
        // `nrcores` is labelled with its provenance because it is NOT the core count on this part —
        // see the cross-check after the walk.
        serial_println!(
            ":: bcma-s1: chip id[15:0]={:#06x} rev[19:16]={} pkg[23:20]={} nrcores[27:24]={}(SB-era CoreCount, advisory) type[31:28]={} ({}) pmu={} is-4331={} ::",
            cc_id, cc_rev, cc_pkg, cc_ncores, cc_type,
            match cc_type { 0 => "ssb/sb", 1 => "bcma/erom", 2 => "bcma-single", _ => "?" },
            (cap & CC_CAP_PMU != 0) as u8, (cc_id == DEVID_BCM4331) as u8
        );

        // ── Step 3a: board-identity PRESENCE, read while ChipCommon is in the window. ────────────
        //
        // S0 already knows how to print this, but on this machine S0 never gets to: it refuses at
        // `window-elsewhere` because firmware parks cfg:0x80 on the radio core, so the SPROM/OTP
        // presence bits have never actually been read on metal. They are three register reads and
        // the window is already where they live, so they are read here rather than left to S2.
        //
        // PRESENCE only. The CONTENTS of either store are out of this arc's reach and say so on
        // their own lines: the SPROM shadow needs the 4331's external-PA line mux cleared first (a
        // ChipCommon WRITE), and OTP is fetched by writing a read command to OTPP and polling OTPS.
        let otps = unsafe { r32(bar0, CC_OTPS) };
        let otpc = unsafe { r32(bar0, CC_OTPC) };
        let otpl = unsafe { r32(bar0, CC_OTPL) };
        let otp_field = (cap & CC_CAP_OTPS_MASK) >> CC_CAP_OTPS_SHIFT;
        let otp_words: u32 = if otp_field == 0 { 0 } else { 1u32 << (CC_CAP_OTPS_BASE + otp_field) };
        let otp_programmed = (otps & (OTPS_GUP_HW | OTPS_GUP_SW | OTPS_GUP_CI | OTPS_GUP_FUSE)) != 0;
        serial_println!(
            ":: wifi-l0: cc-identity sprom={} (cap bit30) otp={} otp_words={} otps={:#010x} otpc={:#010x} otpl={:#010x} gup(hw={} sw={} ci={} fuse={}) — PRESENCE only; SPROM contents need the 4331 PA-line mux cleared (a ChipCommon WRITE) and OTP contents need a read command written to OTPP, neither of which this arc makes ::",
            if cap & CC_CAP_SPROM != 0 { "present" } else { "absent" },
            if otp_words == 0 { "absent" } else if otp_programmed { "present-programmed" } else { "present-blank" },
            otp_words, otps, otpc, otpl,
            (otps & OTPS_GUP_HW != 0) as u8, (otps & OTPS_GUP_SW != 0) as u8,
            (otps & OTPS_GUP_CI != 0) as u8, (otps & OTPS_GUP_FUSE != 0) as u8
        );

        // ── Step 3b: the EROM. It sits on the backplane at `erom`, outside the ChipCommon window,
        // so reaching it needs cfg:0x80 pointed at it — another window write, restored with the rest
        // at Step 4. The window register is 4 KiB-granular (its low 12 bits are ignored by hardware),
        // and the EROM base is page-aligned, so writing `erom` directly and reading from BAR0+0 is
        // exactly Linux's `bcma_scan_switch_core(erombase)`. The recon's walker is reused verbatim.
        if erom != 0 && erom != ER_BAD {
            let erom_base = erom & 0xFFFF_F000;
            let erom_off = (erom & 0x0000_0FFF) as u64;
            unsafe { crate::arch::pci::write_config_32(bus, dev, func, CFG_BAR0_WIN, erom_base) };
            let ew = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
            serial_println!(
                ":: bcma-s1: WROTE cfg:0x80 old={:#010x} new={:#010x} readback={:#010x} — window now on the EROM; walking read-only ::",
                BCMA_ADDR_BASE, erom_base, ew
            );
            let (cores, d11, cc_core_rev) = walk_erom(|i| unsafe { r32(bar0, erom_off + (i as u64) * 4) });
            // The cross-check, corrected. Boot AJ printed `match=0` and treated it as evidence
            // against the walk; only half of that was sound. `BCMA_CC_ID_NRCORES` (chipid[27:24])
            // is the SB-era CoreCount field: `sb_numcores()` reads it, and Linux's bcma — the
            // driver model for a socitype=1 part like this one — never reads it at all, because on
            // an AI backplane the EROM is the only authority on the core list. So a MISMATCH is not
            // by itself a defect and must not be reported as one. What IS a defect, unambiguously,
            // is a walk that finds zero cores while ChipCommon answers, and that gets its own
            // verdict.
            serial_println!(
                ":: bcma-s1: erom-cross-check walk-cores={} chipcommon-nrcores={} equal={} verdict={} — NRCORES is the SB-era CoreCount field (chipid[27:24]); on socitype={} (bcma/erom) the EROM walk is authoritative and Linux's bcma never reads NRCORES, so a difference is EXPECTED, not a fault. Zero cores IS a fault. ::",
                cores, cc_ncores, (cores == cc_ncores) as u8,
                if cores == 0 { "WALK-FAILED" } else { "WALK-OK" }, cc_type
            );

            // ── WIFI-L0: reach the radio core itself. Uses the SAME selector (cfg:0x80) already
            // written twice above and restored at Step 4; no other register is written. ──────────
            match d11 {
                Some(d) => d11_l0(bus, dev, func, bar0, d),
                None => serial_println!(
                    ":: wifi-l0: SKIPPED reason=no-d11-in-erom — the walk named no core id={:#05x}; the radio's backplane base is not known and this stage will not guess one ::",
                    CORE_ID_80211
                ),
            }

            // ── WIFI-S2: the board's IDENTITY and CAPABILITY from the SPROM shadow. Uses the SAME
            // cfg:0x80 selector (moved back to ChipCommon), restored at Step 4; no other write. The
            // SPROM offset needs the ChipCommon CORE rev the walk just captured, which is why S2
            // runs here (after the walk) rather than at Step 3a. ─────────────────────────────────
            s2_board_identity(bus, dev, func, bar0, cc_id, cc_core_rev);
        } else {
            serial_println!(
                ":: bcma-s1: erom pointer={:#010x} — ChipCommon reports no usable EROM; core inventory not walked (chip type={}) ::",
                erom, cc_type
            );
        }
    }

    // ── Step 4: RESTORE. Push the value READ at Step 1 (pre_win), never 0x18000000. Witness
    // MATCH/FAILED by comparing the read-back to the STORED pre-image. A failed restore leaves the
    // radio's window moved for every later boot stage to misread — this comparison is the safety
    // case, and it is against `pre_win`, not against any assumed enumeration base. ────────────────
    unsafe { crate::arch::pci::write_config_32(bus, dev, func, CFG_BAR0_WIN, pre_win) };
    let restored = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let restore_ok = restored == pre_win;
    serial_println!(
        ":: bcma-s1: RESTORE cfg:0x80 <- pre-image {:#010x} readback={:#010x} restored={} (compared to the STORED pre-image, NOT to 0x18000000) ::",
        pre_win, restored, if restore_ok { "MATCH" } else { "FAILED" }
    );
    if !restore_ok {
        serial_println!(
            ":: bcma-s1: !! RESTORE FAILED — cfg:0x80 is {:#010x}, not the firmware pre-image {:#010x}; the next stage will misread the backplane. Hard finding. ::",
            restored, pre_win
        );
    }

    let (ev, eu) = fmt_dur(dl.elapsed_cycles());
    serial_println!(
        ":: bcma-s1: end ok={} stage=window chip={:#06x} window-hypothesis={} restore={} wrote-cfg80=1 elapsed={}{} ::",
        (window_confirmed && restore_ok) as u8, cc_id,
        if window_confirmed { "CONFIRMED" } else { "REFUTED" },
        if restore_ok { "MATCH" } else { "FAILED" }, ev, eu
    );
}

// ── WIFI-L0: the d11 core's first reach ─────────────────────────────────────────────────────────
//
// `UNAOS_BCMAS1=1`, same knob and same block as S1. This stage adds **no new write target**: it
// moves cfg:0x80 — the one selector S1 already writes twice and restores at Step 4 — onto the d11
// core's backplane base, and reads. It writes no core register, no wrapper register and no other
// config register. Enabling the core, releasing its reset, opening SHM or touching the radio are all
// WRITES and all belong to S3 and later.
//
// ## Why the wrapper needs no write at all on this machine
//
// The wrapper aperture is BAR0's second 4 KiB and is selected by cfg:0xAC, which this arc does not
// write. It does not have to: Boots AL/AM/AN show firmware leaving `cfg:0xAC = 0x18101000`, and the
// EROM walk independently reports the d11 core's master wrapper at exactly `0x18101000`. So the
// wrapper reads below are the RADIO's wrapper — but that is a claim with a check, and the check is
// printed: the live cfg:0xAC value is compared against the EROM's wrap address, and when they differ
// the wrapper stage REFUSES (naming cfg:0xAC) instead of reading someone else's agent registers and
// calling them the radio's.
//
// ## What "reach" means here, and how it can fail
//
// Three independent facts have to line up, and each is its own line:
//
// 1. **The selector took.** cfg:0x80 reads back the d11 base. A config-level fact.
// 2. **The agent answers, and agrees with the ROM.** The AI/aidmp wrapper's ARM identification
//    block (0xFD0..0xFFC) carries a component-id preamble and a 12-bit part number. The preamble is
//    a self-check on the offsets; the part number is the cross-check against the EROM's `id=0x812`.
// 3. **The core answers, and it is the right kind of core.** `PHY_VER` (d11+0x3E0) decodes to a PHY
//    type, and for a BCM4331 that type must be 7 (HT). This is the predicate `bcm4331.md` §S3
//    already names, reached here one stage earlier than planned because the radio core arrives out
//    of reset (Boots AL/AM/AN: `resetctl=0x00000000`, `ioctl=0x00002055`, core-enabled=1).
//
// A window read of `0xFFFFFFFF` refutes (1); a preamble mismatch refutes the transcription, not the
// walk; a `phy_type != 7` would say the part is not what four boots have called it. Each is reported
// as itself.
#[cfg(feature = "bcmaS1")]
fn d11_l0(bus: u8, dev: u8, func: u8, bar0: u64, d: D11) {
    let dl = Deadline::new();
    // Read cfg:0xAC LIVE rather than reusing S1's step-1 snapshot. Nothing in this file writes that
    // register, so the two must be equal — which is exactly why reading it again costs nothing and
    // why a witness that says "live" should not be quoting a value captured several stages earlier.
    let live_win2 = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN2) };
    serial_println!(
        ":: wifi-l0: begin d11 id={:#05x} rev={} base={:#010x} wrap={:#010x}({}) — moving cfg:0x80 (the ONE selector S1 already writes and restores) onto the radio core; NO core register, NO wrapper register and NO other config register is written ::",
        CORE_ID_80211, d.rev, d.base, d.wrap, d.wrap_kind
    );

    // ── 1. The selector. Same register, same proven path; restored by S1's Step 4. ───────────────
    let want = d.base as u32;
    unsafe { crate::arch::pci::write_config_32(bus, dev, func, CFG_BAR0_WIN, want) };
    let win_now = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let win_ok = win_now == want;
    serial_println!(
        ":: wifi-l0: WROTE cfg:0x80 new={:#010x} readback={:#010x} took={} — BAR0+0 now decodes to the d11 core's register window ::",
        want, win_now, win_ok as u8
    );

    // ── 2. The wrapper, reached WITHOUT writing cfg:0xAC — and only if it is provably the d11's. ──
    let wrap_is_d11 = d.wrap != 0 && (live_win2 as u64) == d.wrap;
    let mut agent_ok = false;
    let mut dmp_part: u32 = 0;
    let mut dmp_rev: u32 = 0;
    let mut core_enabled = false;
    if !wrap_is_d11 {
        serial_println!(
            ":: wifi-l0: REFUSED stage=wrapper reason=cfg0xac-elsewhere live-cfg:0xac={:#010x} erom-wrap={:#010x} reg=cfg:0xac(BCMA_PCI_BAR0_WIN2) — the wrapper aperture at bar0+{:#x} is NOT this core's agent; pointing it at the radio is a SECOND config write and this arc makes only the cfg:0x80 one. Reset state and the agent id block are UNREAD, not assumed ::",
            live_win2, d.wrap, BAR0_WRAP_OFF
        );
    } else {
        let wr_ioctl = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
        let wr_iost = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOST) };
        let wr_rstc = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_RESET_CTL) };
        let wr_rsts = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_RESET_ST) };
        let wrap_dead = wr_ioctl == ER_BAD && wr_iost == ER_BAD && wr_rstc == ER_BAD && wr_rsts == ER_BAD;
        let clk = (wr_ioctl & IOCTL_CLK) != 0;
        let fgc = (wr_ioctl & IOCTL_FGC) != 0;
        let in_reset = (wr_rstc & RESET_CTL_RESET) != 0;
        let gated = (wr_iost & IOST_GATED_CLK) != 0;
        core_enabled = !wrap_dead && clk && !fgc && !in_reset;
        serial_println!(
            ":: wifi-l0: wrap cfg:0xac={:#010x} == erom-wrap {:#010x} (no cfg:0xac write needed): ioctl={:#010x} iost={:#010x} resetctl={:#010x} resetst={:#010x} clk={} fgc={} gated-clk={} in-reset={} core-enabled={} (Linux bcma_core_is_enabled) ::",
            live_win2, d.wrap, wr_ioctl, wr_iost, wr_rstc, wr_rsts,
            clk as u8, fgc as u8, gated as u8, in_reset as u8, core_enabled as u8
        );

        // The agent's ARM identification block. RAW first, in two lines of six dwords, so a decode
        // bug can never hide the words it came from.
        let mut idw = [0u32; DMP_ID_WORDS as usize];
        let mut k = 0u64;
        while k < DMP_ID_WORDS {
            idw[k as usize] = unsafe { r32(bar0, BAR0_WRAP_OFF + DMP_ID_BASE + k * 4) };
            k += 1;
        }
        serial_println!(
            ":: wifi-l0: dmp-raw +0xfd0={:#010x} +0xfd4={:#010x} +0xfd8={:#010x} +0xfdc={:#010x} +0xfe0={:#010x} +0xfe4={:#010x} +0xfe8={:#010x} +0xfec={:#010x} +0xff0={:#010x} +0xff4={:#010x} +0xff8={:#010x} +0xffc={:#010x} ::",
            idw[0], idw[1], idw[2], idw[3], idw[4], idw[5], idw[6], idw[7], idw[8], idw[9], idw[10], idw[11]
        );
        // PeripheralID0..3 at 0xFE0..0xFEC -> idw[4..8]; ComponentID0..3 at 0xFF0..0xFFC -> idw[8..12].
        let pid0 = idw[4];
        let pid1 = idw[5];
        let pid2 = idw[6];
        let cid0 = idw[8];
        let cid1 = idw[9];
        let cid2 = idw[10];
        let cid3 = idw[11];
        let preamble_ok = (cid0 & 0xFF) == CID0_PREAMBLE
            && (cid1 & 0x0F) == CID1_PREAMBLE
            && (cid2 & 0xFF) == CID2_PREAMBLE
            && (cid3 & 0xFF) == CID3_PREAMBLE;
        let cclass = (cid1 >> 4) & 0xF;
        dmp_part = (pid0 & 0xFF) | ((pid1 & 0x0F) << 8);
        dmp_rev = (pid2 >> 4) & 0xF;
        let jep106 = ((pid1 >> 4) & 0xF) | ((pid2 & 0x7) << 4);
        let id_match = dmp_part == CORE_ID_80211 as u32;
        agent_ok = preamble_ok;
        serial_println!(
            ":: wifi-l0: dmp-decode preamble={} class={} part={:#05x} rev={} jep106={:#04x} jedec-used={} vs erom(id={:#05x} rev={}) id-match={} rev-match={} — offsets/fields are Broadcom aidmp.h + ARM PrimeCell, transcription UNVERIFIED against this part; a MISMATCH here indicts the transcription or Broadcom's use of the field, NOT the EROM walk, and is a finding to chase rather than a verdict ::",
            if preamble_ok { "OK" } else { "MISMATCH" }, cclass, dmp_part, dmp_rev, jep106,
            ((pid2 >> 3) & 1) as u8,
            CORE_ID_80211, d.rev, id_match as u8, (dmp_rev == d.rev) as u8
        );
    }

    // ── 3. The core window. Refused outright if the wrapper says the core is held in reset. ──────
    let mut phy_ok = false;
    let mut phy_type: u16 = 0xFFFF;
    if !win_ok {
        serial_println!(
            ":: wifi-l0: REFUSED stage=core reason=selector-not-taken readback={:#010x} want={:#010x} — BAR0+0 does not decode to the d11 core, so nothing read there would be the radio ::",
            win_now, want
        );
    } else if wrap_is_d11 && !core_enabled {
        serial_println!(
            ":: wifi-l0: REFUSED stage=core reason=core-not-enabled — the agent says this core is in reset or unclocked; its register window would read back nothing meaningful and taking it out of reset is a WRAPPER WRITE (bcma_core_enable), which is stage S3, not this arc ::"
        );
    } else {
        if !wrap_is_d11 {
            // The core reads below still happen — the selector is confirmed and BAR0+0 is the d11's
            // window — but the reset state behind them was never read, so say so on its own line
            // rather than letting the absence of a caveat imply the core was checked.
            serial_println!(
                ":: wifi-l0: NOTE stage=core reset-state=UNKNOWN — the agent was not reachable (above), so the reads below are taken WITHOUT knowing whether the core is out of reset; an all-ones window here is therefore ambiguous between 'dark core' and 'not decoding' ::"
            );
        }
        // Every offset below is a status/identity register with no read side effect; the
        // read-to-clear interrupt-reason words and the indirect SHM/radio ports are deliberately
        // not touched (see the constant block).
        let raw0 = unsafe { r32(bar0, 0) };
        let macctl = unsafe { r32(bar0, D11_MACCTL) };
        let irqmask = unsafe { r32(bar0, D11_GEN_IRQ_MASK) };
        let hwen_hi = unsafe { r32(bar0, D11_RADIO_HWENABLED_HI) };
        let hwen_lo = unsafe { r16(bar0, D11_RADIO_HWENABLED_LO) };
        serial_println!(
            ":: wifi-l0: core raw@+0x000={:#010x} macctl={:#010x} (enabled={} psm-run={} shm={} ihr={} big-endian={} awake={} gmode={}) irqmask={:#010x} hwen-hi={:#010x}(bit16={}) hwen-lo={:#06x}(bit4={}) ::",
            raw0, macctl,
            (macctl & MACCTL_ENABLED != 0) as u8, (macctl & MACCTL_PSM_RUN != 0) as u8,
            (macctl & MACCTL_SHM_ENABLED != 0) as u8, (macctl & MACCTL_IHR_ENABLED != 0) as u8,
            (macctl & MACCTL_BE != 0) as u8, (macctl & MACCTL_AWAKE != 0) as u8,
            (macctl & MACCTL_GMODE != 0) as u8,
            irqmask, hwen_hi, ((hwen_hi >> 16) & 1) as u8, hwen_lo, ((hwen_lo >> 4) & 1) as u8
        );

        // TSF is the d11 core's own free-running counter. Two samples of it are a CLOCK-LIVENESS
        // reading that no identity register can give: if the low word advances between two reads
        // taken microseconds apart, the core is not merely decoding, it is running. If it does not
        // advance that is NOT a fault — the MAC may simply be disabled — so it is reported as a fact
        // and not as a verdict.
        let tsf_lo0 = unsafe { r32(bar0, D11_TSF_LOW) };
        let tsf_hi0 = unsafe { r32(bar0, D11_TSF_HIGH) };
        let tsf_lo1 = unsafe { r32(bar0, D11_TSF_LOW) };
        let tsf_hi1 = unsafe { r32(bar0, D11_TSF_HIGH) };
        let advanced = (tsf_hi1, tsf_lo1) != (tsf_hi0, tsf_lo0);
        serial_println!(
            ":: wifi-l0: tsf sample0={:#010x}:{:#010x} sample1={:#010x}:{:#010x} advanced={} — the d11 TSF counter; advanced=1 means the core is CLOCKED AND RUNNING, advanced=0 means only that the MAC is not counting (macctl above says whether it is enabled) and is not by itself a fault ::",
            tsf_hi0, tsf_lo0, tsf_hi1, tsf_lo1, advanced as u8
        );

        // The crux read: PHY identity.
        let pv = unsafe { r16(bar0, D11_PHY_VER) };
        let analog = (pv & PHYVER_ANALOG) >> PHYVER_ANALOG_SHIFT;
        phy_type = (pv & PHYVER_TYPE) >> PHYVER_TYPE_SHIFT;
        let phy_rev = pv & PHYVER_VERSION;
        phy_ok = pv != 0xFFFF && pv != 0x0000;
        serial_println!(
            ":: wifi-l0: phy-ver raw={:#06x} analog={} type={} ({}) rev={} expected-type={} (HT-PHY, the BCM4331's) type-match={} readable={} ::",
            pv, analog, phy_type, phy_type_name(phy_type), phy_rev,
            PHYTYPE_HT, (phy_type == PHYTYPE_HT) as u8, phy_ok as u8
        );
    }

    // ── 4. REACH verdict — the three facts, named, and never collapsed into one bit silently. ────
    let reached = win_ok && phy_ok && phy_type == PHYTYPE_HT;
    serial_println!(
        ":: wifi-l0: REACH verdict={} selector-took={} agent-answers={} agent-part={:#05x} phy-readable={} phy-type={} — the radio core is REACHED when the selector takes AND its PHY identity word decodes to the HT-PHY; anything less is named above rather than averaged into this line ::",
        if reached { "REACHED" } else { "NOT-REACHED" },
        win_ok as u8, agent_ok as u8, dmp_part, phy_ok as u8, phy_type
    );
    let (ev, eu) = fmt_dur(dl.elapsed_cycles());
    serial_println!(
        ":: wifi-l0: end ok={} d11-rev={} dmp-rev={} phy-type={} wrote-cfg80=1 wrote-cfg-ac=0(audited) wrote-core-regs=0(audited) elapsed={}{} — next gate is FIRMWARE: a d11 MAC runs downloadable microcode, which this tree does not and will not carry (docs/MANIFESTO/CLEAN_ROOM_POLICY.md); see bcm4331.md S4 ::",
        reached as u8, d.rev, dmp_rev, phy_type, ev, eu
    );
}

/// b43 `B43_PHYTYPE_*`. `?` is an honest "not a type this file names"; the number is on the line.
#[cfg(feature = "bcmaS1")]
fn phy_type_name(t: u16) -> &'static str {
    match t {
        0 => "A",
        1 => "B",
        2 => "G",
        4 => "N",
        5 => "LP",
        6 => "SSLPN",
        7 => "HT",
        8 => "LCN",
        9 => "LCNXN",
        10 => "LCN40",
        11 => "AC",
        _ => "?",
    }
}

/// Which ChipCommon offset the SPROM shadow is read from, and the REASON, so the witness can carry
/// both. Returns `(offset, reason)`.
///
/// **The BCM4331 uses `0x800`, and it is the chip explicitly excluded from the alternative.** This
/// function exists because the first cut of S2 got the rule backwards, and the wrong rule is an easy
/// one to re-derive, so the sourcing is recorded here rather than in a commit message:
///
/// * Current Linux `drivers/bcma/sprom.c` sets `u16 offset = BCMA_CC_SPROM` on the external-SPROM
///   path and never moves it — `BCMA_CC_SPROM_PCIE6` does not appear in that file at all.
/// * The historical (v3.2-era) code that did use `0x830` reads
///   `offset = (chipinfo.id == 0x4331) ? BCMA_CC_SPROM : BCMA_CC_SPROM_PCIE6;` — i.e. the 4331 is
///   the ONE part named as an exception, and the rule it stands in for is **PCIe-core rev >= 6**,
///   not a ChipCommon revision.
/// * The `rev >= 31` test that the first cut borrowed appears exactly once in `sprom.c`, inside
///   `bcma_sprom_ext_available()`, where it selects **SROM_CONTROL over CHIPSTATUS for PRESENCE
///   DETECTION**. It has nothing to do with the shadow's address. Two different functions.
///
/// So on this part the answer is `0x800` — which is what the old S0 code happened to print, though
/// for the wrong reason (it keyed on the chip rev). Both the offset AND the reason go on the wire,
/// and S2 dumps the shadow at BOTH candidates so a single metal boot settles it either way.
///
/// Not `bcmaS1`-gated: S0 chooses a shadow offset as well and had its own copy of the broken rule.
/// One function, both paths.
fn sprom_offset_for(chip_id: u16) -> (u64, &'static str) {
    if chip_id == 0x4331 {
        (CC_SPROM, "bcm4331-named-exception(Linux: 4331 ? BCMA_CC_SPROM : PCIE6); current sprom.c uses BCMA_CC_SPROM unconditionally")
    } else {
        (CC_SPROM, "default BCMA_CC_SPROM — current Linux sprom.c never moves the offset on the external-SPROM path")
    }
}

/// Is this OUI one we recognise as Apple's or Broadcom's?
///
/// **Deliberately NON-EXHAUSTIVE, and the verdict leans the safe way because of it.** Apple alone
/// holds several hundred OUIs; this table is the subset that can be transcribed with confidence,
/// weighted to the 2010-2012 era this board is from. An unrecognised OUI therefore yields
/// `oui-known=0` and downgrades the verdict to `SUSPECT` — which UNDERSTATES a genuine Apple MAC
/// whose prefix is simply missing here. That direction is chosen on purpose: this check exists to
/// stop the stage calling six boardflags bytes a MAC, and a false `SUSPECT` costs a line of reading
/// while a false `PLAUSIBLE` is exactly the failure the review caught. The raw bytes are always on
/// the line, so a reader can overrule this table from the capture.
#[cfg(feature = "bcmaS1")]
fn oui_known(o: [u8; 3]) -> &'static str {
    match o {
        // Broadcom.
        [0x00, 0x10, 0x18] | [0x00, 0x1B, 0xE9] | [0x18, 0xC0, 0x86] => "broadcom",
        // Apple — era-weighted subset (2008-2013 Macs), non-exhaustive.
        [0x00, 0x1B, 0x63]
        | [0x00, 0x1E, 0xC2]
        | [0x00, 0x1F, 0xF3]
        | [0x00, 0x21, 0xE9]
        | [0x00, 0x22, 0x41]
        | [0x00, 0x23, 0x12]
        | [0x00, 0x23, 0x6C]
        | [0x00, 0x23, 0xDF]
        | [0x00, 0x24, 0x36]
        | [0x00, 0x25, 0x00]
        | [0x00, 0x25, 0x4B]
        | [0x00, 0x25, 0xBC]
        | [0x00, 0x26, 0x08]
        | [0x00, 0x26, 0x4A]
        | [0x00, 0x26, 0xB0]
        | [0x00, 0x26, 0xBB]
        | [0x00, 0x3E, 0xE1]
        | [0x04, 0x0C, 0xCE]
        | [0x04, 0x1E, 0x64]
        | [0x04, 0x54, 0x53]
        | [0x04, 0xF1, 0x3E]
        | [0x08, 0x00, 0x07]
        | [0x0C, 0x3E, 0x9F] => "apple",
        _ => "",
    }
}

// ── WIFI-S2: board identity + capability from the SPROM shadow ───────────────────────────────────
//
// `UNAOS_BCMAS1=1`, same knob and same block as S1/L0. This stage adds **no new write target**: it
// moves cfg:0x80 — the one selector S1 already writes and restores at Step 4 — back onto ChipCommon
// (`0x18000000`, where the SPROM shadow lives) and reads. It writes no ChipCommon register, no core
// register and no second config register.
//
// ## What it reads, and why read-only reaches it
//
// The SPROM "shadow" is a 16-bit window inside ChipCommon's own register block
// (`CC_SPROM`). The offset is `0x800` for this part, unconditionally — see `sprom_offset_for` for
// the three sourcing points. (The first cut of this arc claimed `0x830` keyed on ChipCommon CORE
// rev >= 31; that was REFUTED in review: the BCM4331 is the one chip Linux names as EXCLUDED from
// 0x830, and the rev>=31 test belongs to SPROM PRESENCE detection, not offset selection. The
// inversion is recorded rather than hidden because the wrong rule read plausibly.)
// Every read is an `r16` of a status/identity word with no side effect. The three
// facts a stack needs come out of it: the station MAC (`il0macaddr`, the WiFi twin of BT's BD_ADDR),
// the enabled bands (the antenna-available masks), and the board id (`board_type`, `board_rev`,
// `boardflags`).
//
// ## Where read-only stops, and the write it will name instead
//
// On the BCM4331 the external power-amplifier control lines are muxed onto the SPROM pins; Linux
// calls `bcma_chipco_bcm4331_ext_pa_lines_ctl(cc, false)` — a ChipCommon chip-control WRITE — before
// reading the shadow. That write is past this arc's read-only ceiling. So if the shadow reads
// all-`0xFFFF` (or all-zero), S2 does NOT poke the mux: it reports `BLOCKED`, NAMES the write (as L0
// names the cfg:0x80 write it declines), and notes that the identity may instead live in OTP
// (`gup(ci)` above), whose CONTENTS need a read command written to OTPP — also a write, also refused.
//
// ## Honesty about the field offsets
//
// Only the MAC and the SROM revision are treated as confident (they match S0's decode and
// `bcm4331.md`). Every other field prints its RAW word beside an UNVERIFIED decode, and `board_type`
// carries its own self-check: Linux takes it from `SSB_SPROM1_SPID`, which equals the PCI subsystem
// device id, so a mismatch against the live `ssid` device convicts the offset rather than the board.
#[cfg(feature = "bcmaS1")]
fn s2_board_identity(bus: u8, dev: u8, func: u8, bar0: u64, chip_id: u16, cc_core_rev: Option<u32>) {
    let dl = Deadline::new();

    // ── 1. Point the ONE selector back at ChipCommon; the SPROM shadow is in its window. Same
    // cfg:0x80 register S1 writes and restores at Step 4 — no restore of its own here. ────────────
    unsafe { crate::arch::pci::write_config_32(bus, dev, func, CFG_BAR0_WIN, BCMA_ADDR_BASE) };
    let win_now = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let win_ok = win_now == BCMA_ADDR_BASE;

    // The offset and the REASON for it, both on the wire. See `sprom_offset_for`: on the 4331 this is
    // 0x800, and the ChipCommon-rev test that once chose 0x830 here belongs to PRESENCE detection,
    // not to offset selection. The core rev is still carried and printed — it is what gates the
    // SROM_CONTROL presence read below — but it no longer picks the address.
    let ccrev = cc_core_rev.unwrap_or(0);
    let ccrev_known = cc_core_rev.is_some();
    let (spoff, spoff_reason) = sprom_offset_for(chip_id);
    serial_println!(
        ":: wifi-s2: begin — board identity from the SPROM shadow; cfg:0x80 back on ChipCommon ({:#010x}) readback={:#010x} took={} — shadow offset={:#06x} reason={} (chip={:#06x}); ChipCommon CORE rev={} rev-known={} is used ONLY for the SROM_CONTROL presence test, NOT for this offset ::",
        BCMA_ADDR_BASE, win_now, win_ok as u8, spoff, spoff_reason, chip_id, ccrev, ccrev_known as u8
    );
    if !win_ok {
        serial_println!(
            ":: wifi-s2: REFUSED stage=selector reason=not-taken readback={:#010x} want={:#010x} — BAR0+0 does not decode to ChipCommon, so the shadow reads below would not be the SPROM; nothing read ::",
            win_now, BCMA_ADDR_BASE
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(
            ":: wifi-s2: end ok=0 stage=selector sprom-readable=0 wrote-cfg80=1(selector, restored by S1) elapsed={}{} ::",
            ev, eu
        );
        return;
    }

    // ── 2. PRESENCE, determined rather than inferred. This is the OTHER half of Linux's
    // `bcma_sprom_ext_available()` — and the function whose `rev >= 31` test the first cut of this
    // stage mistook for an offset rule. On a ChipCommon CORE rev >= 31 that advertises CAP_SPROM, an
    // external SPROM is present iff SROM_CONTROL.PRESENT is set; below rev 31 the answer comes from
    // CHIPSTATUS instead and this register is not the authority, so it is read but not ruled on. One
    // register read, and it is what turns the BLOCKED branch from a guess into a determination. ────
    let srom_ctl = unsafe { r32(bar0, CC_SROM_CONTROL) };
    let ctl_authoritative = ccrev_known && ccrev >= 31;
    let ext_present = (srom_ctl & SROM_CONTROL_PRESENT) != 0;
    serial_println!(
        ":: wifi-s2: srom-control raw={:#010x} @cc+{:#06x} present-bit={:#010x} present={} authoritative={} (Linux bcma_sprom_ext_available: the PRESENT bit decides ONLY when the ChipCommon CORE rev >= 31 — here rev={} known={}; below that CHIPSTATUS is the authority and this line is advisory). Transcription UNVERIFIED; raw word is the evidence ::",
        srom_ctl, CC_SROM_CONTROL, SROM_CONTROL_PRESENT, ext_present as u8,
        ctl_authoritative as u8, ccrev, ccrev_known as u8
    );

    // ── 3. Dump the shadow read-only at BOTH candidate offsets. The decode below uses `spoff`
    // (0x800 for this part), but a single metal boot should be able to settle the offset question
    // either way without a second flight, so the alternate window is dumped beside it. Degeneracy is
    // tracked ONLY for the chosen offset, and `words_read` is counted so a loop cut short by the TSC
    // deadline cannot masquerade as an all-ff verdict. ─────────────────────────────────────────────
    let mut all_ff = true;
    let mut all_00 = true;
    let mut words_read = 0u64;
    let sdl = Deadline::new();
    let mut w = 0u64;
    while w < SPROM_WORDS && !sdl.expired() {
        serial_print!(":: wifi-s2: sprom+{:#05x}", spoff + w * 2);
        let mut k = 0u64;
        while k < 8 && w + k < SPROM_WORDS {
            let v = unsafe { r16(bar0, spoff + (w + k) * 2) };
            if v != 0xFFFF {
                all_ff = false;
            }
            if v != 0x0000 {
                all_00 = false;
            }
            serial_print!(" {:04x}", v);
            words_read += 1;
            k += 1;
        }
        serial_println!(" ::");
        w += 8;
    }
    let dump_complete = words_read == SPROM_WORDS;
    if !dump_complete {
        // A truncated dump cannot support ANY verdict about the shadow's contents — least of all
        // "all-ff", which a zero-word read would satisfy vacuously.
        serial_println!(
            ":: wifi-s2: dump-truncated words-read={} of {} stop=tsc-deadline — the degeneracy verdict below is NOT taken from a partial dump ::",
            words_read, SPROM_WORDS
        );
    }

    // The alternate offset, dumped for the record so the offset rule is settleable from one capture.
    // Not decoded, not used in any verdict — evidence only.
    let altoff = if spoff == CC_SPROM { CC_SPROM_PCIE6 } else { CC_SPROM };
    let adl = Deadline::new();
    let mut aw = 0u64;
    while aw < SPROM_WORDS && !adl.expired() {
        serial_print!(":: wifi-s2: sprom-alt+{:#05x}", altoff + aw * 2);
        let mut k = 0u64;
        while k < 8 && aw + k < SPROM_WORDS {
            serial_print!(" {:04x}", unsafe { r16(bar0, altoff + (aw + k) * 2) });
            k += 1;
        }
        serial_println!(" ::");
        aw += 8;
    }
    serial_println!(
        ":: wifi-s2: sprom-alt-note offset={:#06x} dumped for the record and NOT decoded — the chosen offset is {:#06x} ({}). If the decode below is degenerate while this alternate window carries structure, the offset rule is refuted and this capture is enough to say so ::",
        altoff, spoff, spoff_reason
    );

    // ── 4. BLOCKED? An all-FF / all-00 shadow on the 4331 is the PA-line mux, not an empty board —
    // unless SROM_CONTROL authoritatively says no external SPROM is attached, in which case it is a
    // determination. Name the write; do not make it. ─────────────────────────────────────────────
    if dump_complete && (all_ff || all_00) {
        serial_println!(
            ":: wifi-s2: BLOCKED stage=sprom-shadow reason=all-{} — the shadow at offset {:#06x} reads uniformly {} across all {} words. SROM_CONTROL.PRESENT={} authoritative={} => {}. On the BCM4331 the external-PA control lines are muxed onto the SPROM pins; Linux clears them with bcma_chipco_bcm4331_ext_pa_lines_ctl() — a ChipCommon chip-control WRITE — BEFORE reading this shadow. That write is past this arc's read-only ceiling and is NOT made. Identity may instead live in OTP (gup(ci)), whose CONTENTS need a read command written to OTPP — also a write, also refused. MAC/bands/board UNKNOWN, not guessed ::",
            if all_ff { "ff" } else { "00" }, spoff,
            if all_ff { "0xffff" } else { "0x0000" }, words_read,
            ext_present as u8, ctl_authoritative as u8,
            if ctl_authoritative && !ext_present {
                "DETERMINED: no external SPROM is attached — the empty shadow is the truth, not the mux"
            } else if ctl_authoritative && ext_present {
                "an external SPROM IS attached, so the empty shadow points at the PA-line mux"
            } else {
                "presence not authoritative at this ChipCommon rev; mux vs absent is UNDECIDED"
            }
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(
            ":: wifi-s2: end ok=0 stage=sprom-shadow sprom-readable=0 reason={} srom-present={} words-read={} wrote-cfg80=1(selector, restored by S1) wrote-sprom-ctl=0(audited) elapsed={}{} ::",
            if ctl_authoritative && !ext_present { "no-external-sprom" } else { "pa-line-mux-or-absent" },
            ext_present as u8, words_read, ev, eu
        );
        return;
    }

    // ── 4. Decode. RAW words are on the dump above; the MAC and revision are confident, the rest is
    // labelled UNVERIFIED with board_type carrying its own ssid self-check. ──────────────────────
    let last = unsafe { r16(bar0, spoff + (SPROM_WORDS - 1) * 2) };
    let srev = (last & 0xFF) as u8;
    let rev_supported = (8..=11).contains(&srev);

    let m0 = unsafe { r16(bar0, spoff + SP8_IL0MAC) };
    let m1 = unsafe { r16(bar0, spoff + SP8_IL0MAC + 2) };
    let m2 = unsafe { r16(bar0, spoff + SP8_IL0MAC + 4) };
    let mac = [
        (m0 >> 8) as u8,
        (m0 & 0xFF) as u8,
        (m1 >> 8) as u8,
        (m1 & 0xFF) as u8,
        (m2 >> 8) as u8,
        (m2 & 0xFF) as u8,
    ];
    let unicast = (mac[0] & 1) == 0;
    let locally_admin = (mac[0] & 2) != 0;
    let mac_degenerate = mac == [0, 0, 0, 0, 0, 0] || mac == [0xFF; 6];
    // `unicast` alone is a COIN FLIP, not a check — bit 0 of any arbitrary byte is clear half the
    // time, which is precisely how the first cut of this stage could have printed six boardflags
    // bytes and called them a plausible MAC. The OUI test is what gives the claim something it can
    // actually fail on, and the table is non-exhaustive in the safe direction (see `oui_known`).
    let oui = [mac[0], mac[1], mac[2]];
    let oui_vendor = oui_known(oui);
    let oui_is_known = !oui_vendor.is_empty();
    serial_println!(
        ":: wifi-s2: mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} oui={:02x}:{:02x}:{:02x} oui-known={} oui-vendor={} unicast={} locally-admin={} degenerate={} — il0macaddr @sprom+{:#05x} (SSB_SPROM8_IL0MAC, 3 BE words); the radio's identity, the WiFi twin of BT's BD_ADDR. `unicast` alone is a coin flip and is NOT sufficient; the OUI table is NON-EXHAUSTIVE, so oui-known=0 downgrades the verdict rather than convicting the address — the raw bytes here are the evidence ::",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
        oui[0], oui[1], oui[2], oui_is_known as u8,
        if oui_is_known { oui_vendor } else { "unrecognised" },
        unicast as u8, locally_admin as u8, mac_degenerate as u8, SP8_IL0MAC
    );

    // Band capability: the antenna-available word. Low byte = 2.4GHz antenna mask, high byte = 5GHz.
    // A nonzero 5GHz mask is the SPROM's own statement that the board is 5GHz-capable; the 4331 is a
    // dual-band 3x3:3 part, so both bytes are expected nonzero.
    let antavail = unsafe { r16(bar0, spoff + SP8_ANTAVAIL) };
    let ant_bg = (antavail & 0x00FF) as u8;
    let ant_a = ((antavail >> 8) & 0xFF) as u8;
    let band_24 = ant_bg != 0;
    let band_5 = ant_a != 0;
    serial_println!(
        ":: wifi-s2: band antavail-raw={:#06x} @sprom+{:#05x} ant-2.4ghz-mask={:#04x} ant-5ghz-mask={:#04x} band-2.4ghz={} band-5ghz={} dual-band={} — SSB_SPROM8_ANTAVAIL (lo=2.4GHz, hi=5GHz antenna masks). Decode UNVERIFIED (offset from ssb_regs.h); RAW word shown, verdict is the SPROM's statement of which bands are populated, not the PHY's ::",
        antavail, SP8_ANTAVAIL, ant_bg, ant_a, band_24 as u8, band_5 as u8, (band_24 && band_5) as u8
    );

    // Board id. `board_type` is SSB_SPROM1_SPID, and in the normal case the SPROM is what programs
    // the PCIe subsystem id, so the two agree — which makes this a useful cross-check but NOT a law:
    // a mismatch convicts the OFFSET **or** the board (a board that does not program the subsystem
    // id from this word), and the witness says exactly that rather than blaming the offset alone.
    // boardflags/rev are UNVERIFIED, RAW on the dump above.
    let ss = unsafe { read_config_32(bus, dev, func, CFG_SUBSYS) };
    let ss_dev = (ss >> 16) as u16;
    let board_type = unsafe { r16(bar0, spoff + SP8_SPID) };
    let board_rev = unsafe { r16(bar0, spoff + SP8_BOARDREV) };
    let bfl_lo = unsafe { r16(bar0, spoff + SP8_BFL_LO) };
    let bfl_hi = unsafe { r16(bar0, spoff + SP8_BFL_HI) };
    let bfl2_lo = unsafe { r16(bar0, spoff + SP8_BFL2_LO) };
    let bfl2_hi = unsafe { r16(bar0, spoff + SP8_BFL2_HI) };
    let type_matches_ssid = board_type == ss_dev;
    serial_println!(
        ":: wifi-s2: board type={:#06x} (SSB_SPROM1_SPID @sprom+{:#05x}; pci-ssid-device={:#06x} match={}) rev={:#06x} @sprom+{:#05x} boardflags={:04x}{:04x} boardflags2={:04x}{:04x} — the SPROM normally programs the PCIe subsystem id, so a match corroborates the offset; a MISMATCH convicts the offset OR the board (one that does not source its subsystem id from this word), not the offset alone. boardflags/rev decode UNVERIFIED, RAW dump above ::",
        board_type, SP8_SPID, ss_dev, type_matches_ssid as u8, board_rev, SP8_BOARDREV,
        bfl_hi, bfl_lo, bfl2_hi, bfl2_lo
    );

    // ── 5. Verdict + end.
    //
    // PLAUSIBLE has to be able to FAIL on the headline claim — the MAC — or it is decoration. The
    // first cut gated it on `unicast && rev_supported && type_matches_ssid`, and every one of those
    // three could pass while the MAC itself was six bytes of boardflags read from the wrong offset:
    // `board_type`/`board_rev` sit at offsets that were correct all along, so they corroborate the
    // SHADOW, not the MAC's offset, and `unicast` on a wrong word is a coin flip. So the MAC now
    // carries its own independent condition (a recognised OUI) and the verdict names which leg failed
    // rather than collapsing them.
    //
    // Source note: SPROM present AND OTP gup(ci) programmed, so the shadow may be OTP-backed on this
    // Apple board; either way this is the read-only path.
    let mac_credible = !mac_degenerate && unicast && oui_is_known;
    let shadow_credible = rev_supported && type_matches_ssid;
    let verdict = if mac_degenerate {
        "NO-MAC"
    } else if mac_credible && shadow_credible {
        "PLAUSIBLE"
    } else {
        "SUSPECT"
    };
    serial_println!(
        ":: wifi-s2: identity-verdict {} mac-credible={} (non-degenerate={} unicast={} oui-known={}) shadow-credible={} (srom-rev={} rev-supported={} board-type-matches-ssid={}) dual-band={} source=SPROM-shadow(OTP gup(ci) may back it) — PLAUSIBLE requires the MAC leg AND the shadow leg independently: board_type/board_rev corroborate the SHADOW, never the MAC's own offset, so they cannot stand in for it. SPROM CRC-8 NOT computed in this arc (polynomial not transcribed); a wrong table would lie BAD on a good SPROM ::",
        verdict, mac_credible as u8, (!mac_degenerate) as u8, unicast as u8, oui_is_known as u8,
        shadow_credible as u8, srev, rev_supported as u8, type_matches_ssid as u8,
        (band_24 && band_5) as u8
    );
    let (ev, eu) = fmt_dur(dl.elapsed_cycles());
    serial_println!(
        ":: wifi-s2: end ok={} sprom-readable=1 words-read={} offset={:#06x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} mac-credible={} srom-rev={} dual-band={} srom-present={} wrote-cfg80=1(selector, restored by S1) wrote-sprom-ctl=0(audited) wrote-otpp=0(audited) elapsed={}{} — the MAC's offset (SSB_SPROM8_IL0MAC 0x8c) and this offset rule (0x800 for the 4331) are TRANSCRIBED, not yet corroborated on metal; the alt-offset dump above is what settles them. Next writes (PA-line mux, OTP command, core enable) are S2-contents/S3 and past read-only ::",
        (verdict == "PLAUSIBLE") as u8, words_read, spoff,
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], mac_credible as u8,
        srev, (band_24 && band_5) as u8, ext_present as u8, ev, eu
    );
}
