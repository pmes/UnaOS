// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// WIFI-1 — PCI-config-space identification of the AirPort radio, as a CROSS-CHECK.
//
// STRICTLY READ-ONLY. Every access here is a PCI configuration-space READ. Nothing is written: not
// the command register, not a BAR, not bus-master, not `cfg:0x80`. No BAR window is mapped and no
// device MMIO is touched. Arc 1 is allowed to look; arc 2 is where the device is first written.
//
// ## This is not a first look, and it must not pretend to be
//
// The radio's identity is ALREADY PINNED ON METAL. `docs/dev/OS/06_NETWORK_STACK/bcm4331.md` §0
// records it from Boots AF and AJ, three boots, via `drivers/bcma.rs`:
//
//     :: bcma: device bdf 4:0.0 14e4:4331 rev=02 ssid=106b:00ef irq=0 pin=A cmd=0x0006 … matches=1 ::
//     :: bcma: bar0=0xc1900000 raw=0xc1900004 type=mem64 prefetch=0 maplen=0x2000 …
//
// behind bridge `0:28.1`, in D0, memory-decode and bus-master already on. So a census that merely
// PRINTED what it saw would be an instrument that cannot fail. Instead this census compares what it
// reads against those recorded values and prints `MATCH` or `MISMATCH` per field. A MISMATCH is a
// real finding — a different machine, a different card, or a firmware that no longer parks the part
// the way it did — and arc 2 is gated on the cross-check passing.
//
// ## The subclass is the whole point
//
// The BCM4331 reports PCI class 0x02 **subclass 0x80** ("other network controller"). The bench rMBP
// ALSO has `14e4:16bc`/`14e4:...` at `3:0.x` — the BCM57765 combo chip, whose Gigabit Ethernet MAC
// is class 0x02 **subclass 0x00** and sits at a LOWER bdf than the radio. Matching on class alone and
// taking the first hit therefore selects the Ethernet MAC of a laptop with no Ethernet jack, and the
// radio is never examined. That is not hypothetical: it is the exact defect `bcm4331.md` §0 records
// (`PciScanner::find_device(0x02, 0x00)` "could never have matched" the radio, and the part "has
// never been examined by this OS" because of it), and it is why `drivers/bcma.rs` carries its own
// `find_wifi()` rather than reusing the class-only helper. This census matches
// `class == 0x02 && subclass == 0x80`, and when the only Broadcom network functions on the machine
// are subclass 0x00 it says so BY NAME instead of silently selecting one.
//
// ## Duplication with drivers/bcma.rs, and why it stands
//
// `bcma::find_wifi()` performs the same sweep with the same discriminator. It is not reused because
// it is private to a module declared `#[cfg(all(target_arch = "x86_64", feature = "bcmarecon"))]`,
// and `bcmarecon` also arms a BAR0 mapping + MMIO recon. Arming that from `UNAOS_WIFI=1` would make
// this arc's "no MMIO, no device write" claim false by construction. The two sweeps are deliberately
// independent and deliberately agree on the discriminator; if one is ever changed the other must be.
//
// ## Sourcing (see the note in `mod.rs`)
//
// Everything here comes from the public PCI base specification (config-header layout, BAR type
// encoding in bits [2:1], class-code list), the public PCI-SIG vendor registry (Broadcom = 0x14E4),
// and this tree's own metal captures in `bcm4331.md` §0. No Linux driver source was consulted for
// this file.

use spin::Mutex;

use crate::arch::pci::{read_config_16, read_config_32};

/// Broadcom's PCI vendor id (public PCI-SIG vendor registry).
pub const VENDOR_BROADCOM: u16 = 0x14E4;
/// PCI base class 0x02 = Network controller (public PCI base spec class-code list).
const CLASS_NETWORK: u8 = 0x02;
/// PCI subclass 0x80 = "other network controller" — what a Broadcom WiFi part reports, and what
/// `find_device(0x02, 0x00)` structurally cannot match. Mirrors `bcma::SUBCLASS_OTHER`.
const SUBCLASS_OTHER: u8 = 0x80;
/// PCI subclass 0x00 = Ethernet — the BCM57765 MAC at `3:0.0`, counted separately so a machine with
/// only that part gets a refusal that names what it DID find.
const SUBCLASS_ETHERNET: u8 = 0x00;

// ── Pinned metal expectations (bcm4331.md §0, Boots AF + AJ, three boots) ───────────────────────
/// The AirPort radio's PCI device id.
const EXPECT_DEVICE: u16 = 0x4331;
/// Its revision.
const EXPECT_REVISION: u8 = 0x02;
/// Its Apple board id (`ssid` in the bcma witness).
const EXPECT_SUBSYS: (u16, u16) = (0x106B, 0x00EF);
/// BAR0's base, 64-bit non-prefetchable, high dword zero.
const EXPECT_BAR0: u64 = 0xC190_0000;
/// Exactly one subclass-0x80 Broadcom function exists on this machine.
const EXPECT_MATCHES: u32 = 1;

/// What the census read out of config space. Purely descriptive — no resource is claimed by
/// recording it. Arc 2 takes `bar0`/`bdf` from here to map the core window.
#[derive(Clone, Copy, Debug)]
pub struct WifiCore {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub subsys_vendor: u16,
    pub subsys_device: u16,
    pub class: u8,
    pub subclass: u8,
    pub revision: u8,
    /// BAR0 base address with the type bits masked off. Zero means firmware left BAR0 unassigned.
    pub bar0: u64,
    /// BAR0 is a 64-bit memory BAR (bits [2:1] == 0b10).
    pub bar0_64: bool,
    /// BAR0 decodes I/O space rather than memory (bit 0 set) — would be a surprise, so it is
    /// reported rather than silently ignored.
    pub bar0_io: bool,
    /// True when every cross-checked field matched the values `bcm4331.md` §0 pinned on metal.
    pub cross_check_ok: bool,
}

static FOUND: Mutex<Option<WifiCore>> = Mutex::new(None);

/// The core the census selected, for a later arc. `None` until `census()` has run and matched.
pub fn found() -> Option<WifiCore> {
    *FOUND.lock()
}

/// Sweep PCI config space for the AirPort radio and cross-check it against the pinned metal facts.
///
/// The sweep shape is the census's, not `find_device`'s: 256 buses x 32 devices, with functions 1..7
/// probed only when function 0's header type carries the multi-function bit — probing them on a
/// single-function device is architecturally undefined. It reaches bus 4 and therefore reaches the
/// radio behind bridge `0:28.1` (proven: `bcma.rs`'s identical sweep found it there on Boot AF).
///
/// Returns `true` only when a subclass-0x80 Broadcom function was found. Every outcome prints
/// exactly one terminal verdict line, and the outcomes are mutually exclusive.
pub fn census() -> bool {
    let mut wifi_seen = 0u32;
    let mut enet_seen = 0u32;
    let mut selected: Option<WifiCore> = None;

    for bus in 0u16..256 {
        for slot in 0u8..32 {
            let v0 = unsafe { read_config_16(bus as u8, slot, 0, 0x00) };
            if v0 == 0xFFFF {
                continue;
            }
            let hdr = unsafe { read_config_32(bus as u8, slot, 0, 0x0C) };
            let multi = (((hdr >> 16) & 0xFF) as u8 & 0x80) != 0;
            let max_func = if multi { 7 } else { 0 };

            for func in 0..=max_func {
                let vendor = unsafe { read_config_16(bus as u8, slot, func, 0x00) };
                if vendor != VENDOR_BROADCOM {
                    continue;
                }
                let class_word = unsafe { read_config_16(bus as u8, slot, func, 0x0A) };
                let class = (class_word >> 8) as u8;
                let subclass = (class_word & 0xFF) as u8;
                if class != CLASS_NETWORK {
                    continue;
                }
                if subclass == SUBCLASS_ETHERNET {
                    // The BCM57765 MAC. Counted so the refusal below can name it, never selected.
                    enet_seen += 1;
                    serial_println!(
                        ":: wifi: brcm net function {:02x}:{:02x}.{} device={:#06x} subclass=0x00 (Ethernet) — NOT the radio, skipped ::",
                        bus as u8, slot, func,
                        unsafe { read_config_16(bus as u8, slot, func, 0x02) }
                    );
                    continue;
                }
                if subclass != SUBCLASS_OTHER {
                    continue;
                }
                wifi_seen += 1;

                let device = unsafe { read_config_16(bus as u8, slot, func, 0x02) };
                let rev_word = unsafe { read_config_32(bus as u8, slot, func, 0x08) };
                let revision = (rev_word & 0xFF) as u8;
                let ssv = unsafe { read_config_16(bus as u8, slot, func, 0x2C) };
                let ssd = unsafe { read_config_16(bus as u8, slot, func, 0x2E) };

                let bar0_raw = unsafe { read_config_32(bus as u8, slot, func, 0x10) };
                let bar0_io = (bar0_raw & 0x1) != 0;
                let bar0_64 = !bar0_io && (bar0_raw & 0x06) == 0x04;
                let bar0 = if bar0_io {
                    (bar0_raw & 0xFFFF_FFFC) as u64
                } else if bar0_64 {
                    let hi = unsafe { read_config_32(bus as u8, slot, func, 0x14) };
                    ((bar0_raw & 0xFFFF_FFF0) as u64) | ((hi as u64) << 32)
                } else {
                    (bar0_raw & 0xFFFF_FFF0) as u64
                };

                let mut core = WifiCore {
                    bus: bus as u8, slot, func,
                    vendor, device, subsys_vendor: ssv, subsys_device: ssd,
                    class, subclass, revision, bar0, bar0_64, bar0_io,
                    cross_check_ok: false,
                };

                // The cross-check. Each field is compared against the metal-recorded value; the line
                // prints both sides so a MISMATCH names the discrepancy rather than just failing.
                let d_ok = device == EXPECT_DEVICE;
                let r_ok = revision == EXPECT_REVISION;
                let s_ok = (ssv, ssd) == EXPECT_SUBSYS;
                let b_ok = bar0 == EXPECT_BAR0 && bar0_64 && !bar0_io;
                core.cross_check_ok = d_ok && r_ok && s_ok && b_ok;

                serial_println!(
                    ":: wifi: radio {:02x}:{:02x}.{} device={:#06x} expected={:#06x} {} rev={:#04x} expected={:#04x} {} subsys={:#06x}:{:#06x} expected={:#06x}:{:#06x} {} ::",
                    core.bus, core.slot, core.func,
                    device, EXPECT_DEVICE, if d_ok { "MATCH" } else { "MISMATCH" },
                    revision, EXPECT_REVISION, if r_ok { "MATCH" } else { "MISMATCH" },
                    ssv, ssd, EXPECT_SUBSYS.0, EXPECT_SUBSYS.1, if s_ok { "MATCH" } else { "MISMATCH" },
                );
                serial_println!(
                    ":: wifi: radio {:02x}:{:02x}.{} bar0={:#018x} expected={:#018x} kind={} expected=mem64 {} ::",
                    core.bus, core.slot, core.func, bar0, EXPECT_BAR0,
                    if core.bar0_io { "io" } else if core.bar0_64 { "mem64" } else { "mem32" },
                    if b_ok { "MATCH" } else { "MISMATCH" },
                );

                if selected.is_none() {
                    selected = Some(core);
                }
            }
        }
    }

    match selected {
        Some(core) => {
            *FOUND.lock() = Some(core);
            let count_ok = wifi_seen == EXPECT_MATCHES;
            if core.bar0 == 0 {
                // Not fatal for arc 1 (nothing is mapped here), but arc 2 cannot proceed without it.
                serial_println!(
                    ":: wifi: WARNING BAR0 unassigned on {:02x}:{:02x}.{} — firmware left the window unallocated; arc 2 will need to size and assign it ::",
                    core.bus, core.slot, core.func
                );
            }
            serial_println!(
                ":: wifi: radio SELECTED {:02x}:{:02x}.{} matches={} expected={} {} cross-check={} (vs bcm4331.md §0, Boots AF/AJ) — config-space read-only, no MMIO ::",
                core.bus, core.slot, core.func, wifi_seen, EXPECT_MATCHES,
                if count_ok { "MATCH" } else { "MISMATCH" },
                if core.cross_check_ok && count_ok { "PASS" } else { "FAIL" },
            );
            true
        }
        None if enet_seen > 0 => {
            serial_println!(
                ":: wifi: REFUSED — {} Broadcom class-0x02 function(s) found but ALL subclass 0x00 (Ethernet, the BCM57765 MAC); the AirPort radio is subclass 0x80 and is absent from this machine's config space — parked ::",
                enet_seen
            );
            false
        }
        None => {
            serial_println!(
                ":: wifi: no Broadcom ({:#06x}) class-0x02/subclass-0x80 function in PCI config space (0 Ethernet-subclass Broadcom functions either) — parked ::",
                VENDOR_BROADCOM
            );
            false
        }
    }
}
