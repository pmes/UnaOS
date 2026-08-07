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
//! This module is the first arc of the native-driver path. It converts assumptions into facts and
//! it writes nothing.
//!
//! ## The hard constraint: READ-ONLY, and honest about where read-only stops
//!
//! Every device access below is a PCI **config-space read** or an **MMIO read** through BAR0. There
//! is no `write_config_*` call and no `write_volatile` anywhere in this file. That is not a style
//! preference; it is the arc's whole contract, because the alternative — poking a radio whose
//! backplane we have not enumerated — is how you wedge a bus you cannot yet reset.
//!
//! Mapping BAR0 (`arch::memory::map_mmio_window`) is a page-table edit, not a device access: the
//! device sees nothing. It is the same seam `sdhc::probe` and the GPU drivers use.
//!
//! The consequence, stated up front because it is the arc's main finding: **a BCMA part on a PCIe
//! host exposes exactly ONE backplane core through BAR0 at a time**, selected by a PCI *config*
//! register, and firmware leaves that selector at the ChipCommon enumeration base. So ChipCommon is
//! readable and everything else — the enumeration ROM, the 802.11 core, the PHY version register —
//! is behind one 32-bit config write this arc refuses to make. Each of those is reported as an
//! explicit `REFUSED` witness naming the register, not as a missing line.
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
//! * ChipCommon reading all-ones                          -> `REFUSED reason=not-decoding`;
//! * the BAR0 window not parked at the enumeration base   -> `REFUSED reason=window-elsewhere`;
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
/// **This arc does not write it.**
const CFG_BAR0_WIN: u8 = 0x80;
/// `BCMA_PCI_BAR0_WIN2` — the backplane address the SECOND 4 KiB of BAR0 decodes to (the wrapper
/// window). Read here only to record what firmware left behind.
const CFG_BAR0_WIN2: u8 = 0xAC;

// ── BCMA backplane constants ────────────────────────────────────────────────────────────────────

/// `BCMA_ADDR_BASE` / `SI_ENUM_BASE` — the backplane address of the ChipCommon core, and the value
/// firmware is expected to have left in `BAR0_WIN`. If the live window is anything else, the
/// registers at BAR0+0 are NOT ChipCommon and this file refuses to decode them.
const BCMA_ADDR_BASE: u32 = 0x1800_0000;

/// One backplane core's register window. Also the size of the BAR0 core window, which is what makes
/// "is the EROM reachable" a simple containment test.
const BCMA_CORE_SIZE: u32 = 0x1000;

/// How much of BAR0 is mapped. 0x2000 = the core window (0x0000) plus the wrapper window (0x1000),
/// which is the smallest BAR0 any BCMA PCIe part presents. Nothing at or past 0x1000 is READ — the
/// BAR's true size is unknowable without a sizing write and this arc does not size BARs — so the
/// second page is mapped only so a future stage does not have to re-map.
const BAR0_MAP_LEN: usize = 0x2000;

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
const CC_SPROM: u64 = 0x0800; // SPROM shadow, ChipCommon rev < 31
const CC_SPROM_PCIE6: u64 = 0x0830; // SPROM shadow, ChipCommon rev >= 31

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

/// How many 16-bit SPROM shadow words to dump. 220 is `SSB_SPROMSIZE_WORDS_R4`, the size of a
/// revision-8 SPROM (what a BCM4331 board carries). 220 words = 440 bytes, so the highest byte
/// touched is 0x830 + 0x1B7 = 0x9E7, comfortably inside the 4 KiB core window.
const SPROM_WORDS: u64 = 220;

/// EROM entry encodings (Linux `drivers/bcma/scan.c`).
const ER_VALID: u32 = 0x0000_0001;
const ER_TAG: u32 = 0x0000_000E;
const ER_TAG_CI: u32 = 0x0000_0000; // component identifier (a core)
const ER_TAG_MP: u32 = 0x0000_0002; // master port descriptor
const ER_TAG_ADDR: u32 = 0x0000_0004; // address descriptor
const ER_TAG_END: u32 = 0x0000_000E; // end of the ROM
const ER_BAD: u32 = 0xFFFF_FFFF;

const CIA_MFG_MASK: u32 = 0x0000_0FFF;
const CIA_ID_MASK: u32 = 0x00FF_F000;
const CIA_ID_SHIFT: u32 = 12;
const CIA_CLASS_MASK: u32 = 0x0F00_0000;
const CIA_CLASS_SHIFT: u32 = 24;

const CIB_NMW_MASK: u32 = 0x0000_00F8; // # master wrappers
const CIB_NMW_SHIFT: u32 = 3;
const CIB_NSW_MASK: u32 = 0x0000_1F00; // # slave wrappers
const CIB_NSW_SHIFT: u32 = 8;
const CIB_NMP_MASK: u32 = 0x0001_E000; // # master ports
const CIB_NMP_SHIFT: u32 = 13;
const CIB_NSP_MASK: u32 = 0x001E_0000; // # slave ports
const CIB_NSP_SHIFT: u32 = 17;
const CIB_REV_MASK: u32 = 0xFF00_0000;
const CIB_REV_SHIFT: u32 = 24;

const ADDR_AS_TYPE: u32 = 0x0000_0002; // "size is in the next dword" flag
const ADDR_AS_64: u32 = 0x0000_0004; // address is 64-bit (a second dword follows)
const ADDR_TYPE_MASK: u32 = 0x0000_00C0;
const ADDR_TYPE_SHIFT: u32 = 6;
const ADDR_SZ_MASK: u32 = 0x0000_0038;
const ADDR_SZ_SZD: u32 = 0x0000_0018; // "size descriptor follows"
const ADDR_ADDR_MASK: u32 = 0xFFFF_F000;

/// Hard iteration ceiling on the EROM walk. A real EROM is on the order of 60-100 dwords; 512 is
/// far above any plausible one, so hitting it is itself information and is reported. This is the
/// STRUCTURAL bound; the TSC deadline below is the WALL-CLOCK bound, and both are needed — a walk
/// over a stuck-at-zero window would satisfy neither on its own.
const EROM_MAX_ENTRIES: u32 = 512;

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
fn core_name(id: u16) -> &'static str {
    match id {
        0x800 => "chipcommon",
        0x801 => "ilinelt",
        0x807 => "pcie-g1",
        0x80C => "sdio-dev",
        0x812 => "mips-33",
        0x81B => "usb20-host",
        0x81C => "usb20-dev",
        0x820 => "d11(802.11)",
        0x829 => "pcie-g2",
        0x82C => "gmac-cmn",
        0x82D => "gmac",
        0x835 => "pmu",
        0x83E => "ns-pcie2",
        _ => "?",
    }
}

/// Address-descriptor port type.
fn addr_type_name(t: u32) -> &'static str {
    match t {
        0 => "slave",
        1 => "bridge",
        2 => "swrap",
        3 => "mwrap",
        _ => "?",
    }
}

/// Walk the enumeration ROM through an already-open window and print one line per core.
///
/// `read` yields the dword at EROM entry index `i`. It is a closure so the walker is independent of
/// HOW the EROM was reached: this arc can only reach it when firmware happens to have parked the
/// BAR0 window on it, but the same walker serves the stage that programs the window deliberately.
///
/// Bounded twice over (structural `EROM_MAX_ENTRIES`, wall-clock `Deadline`) and it reports which
/// bound stopped it. Returns the number of cores printed.
fn walk_erom<F: Fn(u32) -> u32>(read: F) -> u32 {
    let dl = Deadline::new();
    let mut i = 0u32;
    let mut cores = 0u32;
    let mut stop = "end-tag";

    loop {
        if i >= EROM_MAX_ENTRIES {
            stop = "entry-cap";
            break;
        }
        if dl.expired() {
            stop = "tsc-deadline";
            break;
        }
        let cia = read(i);
        i += 1;

        if cia == ER_BAD {
            stop = "all-ones";
            break;
        }
        if (cia & ER_TAG) == ER_TAG_END {
            break;
        }
        if (cia & ER_VALID) == 0 || (cia & ER_TAG) != ER_TAG_CI {
            // Not a component identifier: skip it rather than mis-decoding it. A malformed EROM
            // reaches the entry cap above, which is reported.
            continue;
        }

        let cib = read(i);
        i += 1;
        if cib == ER_BAD || (cib & ER_TAG) != ER_TAG_CI {
            stop = "cib-malformed";
            break;
        }

        let mfg = (cia & CIA_MFG_MASK) as u16;
        let id = ((cia & CIA_ID_MASK) >> CIA_ID_SHIFT) as u16;
        let class = (cia & CIA_CLASS_MASK) >> CIA_CLASS_SHIFT;
        let rev = (cib & CIB_REV_MASK) >> CIB_REV_SHIFT;
        let nmw = (cib & CIB_NMW_MASK) >> CIB_NMW_SHIFT;
        let nsw = (cib & CIB_NSW_MASK) >> CIB_NSW_SHIFT;
        let nmp = (cib & CIB_NMP_MASK) >> CIB_NMP_SHIFT;
        let nsp = (cib & CIB_NSP_MASK) >> CIB_NSP_SHIFT;

        // Master port descriptors carry no address; they are consumed so the address descriptors
        // that follow are read at the right index. Their tag is CHECKED rather than assumed — a
        // miscounted `nmp` would otherwise silently shift every address below by a dword and
        // produce a core list of plausible-looking nonsense, which is the one failure mode of an
        // EROM walk that a reader cannot spot from the output.
        let mut m = 0;
        while m < nmp && i < EROM_MAX_ENTRIES {
            let mp = read(i);
            i += 1;
            m += 1;
            if mp == ER_BAD || (mp & ER_TAG) != ER_TAG_MP {
                stop = "mp-malformed";
                break;
            }
        }
        if stop != "end-tag" {
            serial_println!(
                ":: bcma: erom-abort at entry {} while consuming {} master ports of core id={:#05x} ::",
                i, nmp, id
            );
            break;
        }

        // Address descriptors: nsp slave ports, then nmw + nsw wrapper ports. The FIRST slave
        // address is the core's register base — the address a driver would put in BAR0_WIN — and
        // the first slave WRAPPER address is where its reset/clock control lives. Both are printed.
        let total_addr = nsp + nmw + nsw;
        let mut base: u64 = 0;
        let mut wrap: u64 = 0;
        let mut wrap_kind: &'static str = "none";
        let mut a = 0u32;
        let mut printed_extra = 0u32;
        while a < total_addr && i < EROM_MAX_ENTRIES && !dl.expired() {
            let ad = read(i);
            i += 1;
            if ad == ER_BAD || (ad & ER_TAG) != ER_TAG_ADDR {
                stop = "addr-malformed";
                break;
            }
            let mut addr = (ad & ADDR_ADDR_MASK) as u64;
            if (ad & ADDR_AS_64) != 0 {
                addr |= (read(i) as u64) << 32;
                i += 1;
            }
            if (ad & ADDR_SZ_MASK) == ADDR_SZ_SZD {
                // A size descriptor follows (one dword, two when 64-bit).
                let sz = read(i);
                i += 1;
                if (sz & ADDR_AS_TYPE) != 0 {
                    i += 1;
                }
            }
            let ty = (ad & ADDR_TYPE_MASK) >> ADDR_TYPE_SHIFT;
            if a < nsp {
                if a == 0 {
                    base = addr;
                } else {
                    printed_extra += 1;
                }
            } else if ty == 2 {
                // Slave wrapper — the window that carries this core's reset/clock control, and the
                // one Linux puts in `core->wrap`. PREFERRED over a master wrapper even when a
                // master wrapper was seen first (the EROM emits master wrappers before slave ones),
                // because a bring-up stage driving the wrong wrapper looks exactly like a core that
                // will not come out of reset.
                wrap = addr;
                wrap_kind = addr_type_name(ty);
            } else if ty == 3 && wrap == 0 {
                wrap = addr;
                wrap_kind = addr_type_name(ty);
            }
            a += 1;
        }

        if cores < EROM_MAX_CORES {
            serial_println!(
                ":: bcma: core[{}] id={:#05x} ({}) rev={} mfg={:#05x} class={} base={:#x} wrap={:#x}({}) nsp={} nmp={} nsw={} nmw={} extra-slave-addr={} ::",
                cores, id, core_name(id), rev, mfg, class, base, wrap, wrap_kind, nsp, nmp, nsw,
                nmw, printed_extra
            );
        }
        cores += 1;
        if stop != "end-tag" {
            break;
        }
    }

    let (ev, eu) = fmt_dur(dl.elapsed_cycles());
    serial_println!(
        ":: bcma: erom-walk cores={} entries={} stop={} elapsed={}{} ::",
        cores, i, stop, ev, eu
    );
    cores
}

// ── The probe ───────────────────────────────────────────────────────────────────────────────────

/// BCMA-RECON. Read-only; called once from `arch::x86_64::pci::init` under the `bcmarecon` feature.
pub fn recon() {
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
        serial_println!(
            ":: bcma: power cap@{:#04x} pmcsr={:#06x} state=D{} ::",
            pm, (pmcsr & 0xFFFF) as u16, d
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
    let win = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let win2 = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN2) };
    serial_println!(
        ":: bcma: bar0={:#x} raw={:#010x} type={} prefetch={} maplen={:#x} bar0_win(cfg:0x80)={:#010x} bar0_win2(cfg:0xac)={:#010x} ::",
        bar0, bar0_raw, if is64 { "mem64" } else { "mem32" }, pf, BAR0_MAP_LEN, win, win2
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

    // ── Stage 5: ChipCommon — the one core reachable without moving the window ──────────────────
    let chipid = unsafe { r32(bar0, CC_ID) };
    if chipid == 0xFFFF_FFFF {
        serial_println!(
            ":: bcma: REFUSED stage=chipcommon reason=not-decoding chipid=0xffffffff — BAR0 reads all-ones (function not decoding, or the window is dead) ::"
        );
        let (ev, eu) = fmt_dur(dl.elapsed_cycles());
        serial_println!(":: bcma: end ok=0 stage=chipcommon elapsed={}{} ::", ev, eu);
        return;
    }
    // Is BAR0+0 actually ChipCommon? Only if firmware left the window at the enumeration base.
    // Anything else and the registers below are some other core's, so they are NOT decoded.
    if win != BCMA_ADDR_BASE {
        serial_println!(
            ":: bcma: REFUSED stage=chipcommon reason=window-elsewhere bar0_win={:#010x} expected={:#010x} raw@bar0+0={:#010x} — BAR0+0 is NOT ChipCommon on this reading; moving the window is a WRITE to cfg:0x80, not in this arc ::",
            win, BCMA_ADDR_BASE, chipid
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
        // Shadow offset moves at ChipCommon rev 31 (Linux `bcma_sprom_get`). Print which was used —
        // a dump at the wrong offset is exactly the failure this field lets a reader catch.
        let spoff = if cc_rev >= 31 { CC_SPROM_PCIE6 } else { CC_SPROM };
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
        // The MAC candidate (SSB_SPROM8_IL0MAC, byte offset 0x4A within the SPROM image), stored
        // big-endian per 16-bit word.
        let m0 = unsafe { r16(bar0, spoff + 0x4A) };
        let m1 = unsafe { r16(bar0, spoff + 0x4C) };
        let m2 = unsafe { r16(bar0, spoff + 0x4E) };
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
            ":: bcma: sprom-decode offset={:#05x} words={} last={:#06x} rev={} rev-supported={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} unicast={} all_ff={} all_00={} verdict={} (crc NOT computed in this arc) ::",
            spoff, SPROM_WORDS, last, srev,
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
