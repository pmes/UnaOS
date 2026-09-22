// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// I/O APIC (Intel 82093AA) — the redirection table this kernel has never had, and the reason every
// PCI function without an MSI capability could only ever be POLLED.
//
// THE DEFECT THIS ARC CLOSES, measured on the wire (rmbp flight 11, f11.log at 25626 ms):
//
//   :: EHCI-HID: [1] ISRARM REFUSED — this function offers no usable MSI capability, and there is
//   no IOAPIC in this kernel to route INTx to. The endpoint stays on the POLLED re-arm path …
//
// That refusal was accurate in every word. `arch/x86_64/apic.rs` drives the LOCAL APIC only (xAPIC
// MMIO at 0xFEE00000 and the x2APIC MSR bank) and contains no redirection-entry writer of any kind;
// `drivers/pci.rs` can enable MSI and MSI-X and nothing else; `interrupts::disable_legacy_pic`
// masks every 8259 line and `apic::init` leaves LINT0 masked, so there is no legacy virtual-wire
// path either. A function that asserts INTA# had nowhere for that assertion to go, so the driver
// kept its polled re-arm and the internal trackpad paid for it in dark windows (rmbp ledger B139;
// EHCIDARK measured up to 108 ms with `missed<=11384`).
//
// THIS FILE IS RUNG 1: THE CENSUS. It reads what firmware declares and what the hardware answers,
// and it writes NOTHING. The routing API (rung 2) and the PCI arm (rung 3) land on top of it.
//
// CLEAN ROOM. Every register field below is cited to a public specification and to nothing else:
// the Intel 82093AA I/O APIC datasheet (§3.1 the IOREGSEL/IOWIN window, §3.2.1 IOAPICID, §3.2.2
// IOAPICVER), and ACPI 6.x §5.2.12 (MADT: §5.2.12.3 I/O APIC, §5.2.12.5 Interrupt Source Override
// and the MPS INTI flags, §5.2.12.7 Local APIC NMI).
//
// MECHANISM, THE OPEN QUESTION AND THE FLIGHT EXPECTATION: docs/dev/OS/01_BOOT_HAL/ioapic.md.

use core::sync::atomic::{AtomicU32, Ordering};

// ── ACPI 6.x §5.2.12 Table 5.20 — the MADT entry types the walk used to discard ────────────────
/// I/O APIC (ACPI §5.2.12.3): id(2) reserved(3) address(4..8) gsi_base(8..12). Length 12.
const MADT_IO_APIC: u8 = 1;
/// Interrupt Source Override (§5.2.12.5): bus(2) source(3) gsi(4..8) flags(8..10). Length 10.
const MADT_INT_SRC_OVERRIDE: u8 = 2;
/// Local APIC NMI (§5.2.12.7): acpi_uid(2) flags(3..5) lint(5). Length 6.
const MADT_LOCAL_APIC_NMI: u8 = 4;

// ── Intel 82093AA §3.1 — the two-register indirect window ──────────────────────────────────────
/// IOREGSEL: write the index of the register you want at base+0x00.
const IOREGSEL: u64 = 0x00;
/// IOWIN: read/write the selected register's data at base+0x10.
const IOWIN: u64 = 0x10;

// ── Intel 82093AA §3.2 — the indirect register indices ─────────────────────────────────────────
/// IOAPICID (§3.2.1): the controller's own id in bits 27:24.
const REG_ID: u32 = 0x00;
/// IOAPICVER (§3.2.2): version in bits 7:0, "maximum redirection entry" in bits 23:16 — that field
/// is the LAST valid index, so the entry COUNT is it plus one.
const REG_VER: u32 = 0x01;

/// A machine is allowed more than one I/O APIC (each owns a contiguous GSI range starting at its
/// own `gsi_base`). Four covers the 7-series PCH (one) and every part this kernel will meet; a
/// fifth is COUNTED and refused rather than silently dropped.
const MAX_IOAPICS: usize = 4;
/// ISA has 16 IRQ lines, so 16 overrides is the whole legal source space for bus 0.
const MAX_ISOS: usize = 16;
/// One Local APIC NMI entry per CPU is the common shape; the census only reports them.
const MAX_NMIS: usize = 8;

/// Polarity as the wire reports it. `BusDefault` is the ACPI encoding 00 and is NOT a synonym for
/// active-high: what the bus defaults TO depends on the bus, which is why this is carried as its
/// own value and resolved at the point of use rather than flattened here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    BusDefault,
    ActiveHigh,
    ActiveLow,
}

/// Trigger mode as the wire reports it. Same reasoning as `Polarity` for `BusDefault`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    BusDefault,
    Edge,
    Level,
}

impl Polarity {
    pub fn as_str(self) -> &'static str {
        match self {
            Polarity::BusDefault => "bus-default",
            Polarity::ActiveHigh => "active-high",
            Polarity::ActiveLow => "active-low",
        }
    }
    /// ACPI §5.2.12.5 MPS INTI flags, bits 1:0. 00 = bus default, 01 = active high, 10 = reserved
    /// (reported as bus default — an unknown encoding is not a guess), 11 = active low.
    pub fn from_flags(flags: u16) -> Polarity {
        match flags & 0b11 {
            0b01 => Polarity::ActiveHigh,
            0b11 => Polarity::ActiveLow,
            _ => Polarity::BusDefault,
        }
    }
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::BusDefault => "bus-default",
            Trigger::Edge => "edge",
            Trigger::Level => "level",
        }
    }
    /// ACPI §5.2.12.5 MPS INTI flags, bits 3:2. 01 = edge, 11 = level, 00/10 = bus default.
    pub fn from_flags(flags: u16) -> Trigger {
        match (flags >> 2) & 0b11 {
            0b01 => Trigger::Edge,
            0b11 => Trigger::Level,
            _ => Trigger::BusDefault,
        }
    }
}

/// One I/O APIC as the MADT declares it, plus the two fields only the hardware can answer
/// (`entries` / `version`, read from IOAPICVER by `census`). `entries == 0` means the census could
/// not read the controller, and nothing may ever be routed through it.
#[derive(Clone, Copy)]
pub struct IoApic {
    pub id: u8,
    pub addr: u32,
    pub gsi_base: u32,
    pub entries: u8,
    pub version: u8,
}

/// One Interrupt Source Override: "the interrupt that would have been `bus`/`irq` is really on
/// `gsi`, with these MPS INTI flags" (ACPI §5.2.12.5).
#[derive(Clone, Copy)]
pub struct Iso {
    pub bus: u8,
    pub irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

/// One Local APIC NMI entry (ACPI §5.2.12.7). Reported by the census and otherwise unused —
/// `apic::init` already wires LINT1 as NMI unconditionally and this arc does not change that. It is
/// printed because a machine whose firmware names a DIFFERENT LINT is a fact a later arc would need
/// and no capture this project has taken carries it.
#[derive(Clone, Copy)]
pub struct Nmi {
    pub uid: u8,
    pub flags: u16,
    pub lint: u8,
}

/// Everything the MADT walk collected. Filled ONCE on the BSP from inside `acpi::parse_madt`
/// (before SMP bring-up, before any driver), then read-only for the rest of the boot except for the
/// `entries`/`version` fields the census writes back.
pub struct Census {
    ioapics: [IoApic; MAX_IOAPICS],
    n_ioapics: usize,
    isos: [Iso; MAX_ISOS],
    n_isos: usize,
    nmis: [Nmi; MAX_NMIS],
    n_nmis: usize,
    /// Entries the fixed-size arrays above could not hold. Never silently zero — a machine bigger
    /// than this module's static capacity SAYS so on the census line.
    dropped: u32,
}

impl Census {
    const EMPTY: Census = Census {
        ioapics: [IoApic { id: 0, addr: 0, gsi_base: 0, entries: 0, version: 0 }; MAX_IOAPICS],
        n_ioapics: 0,
        isos: [Iso { bus: 0, irq: 0, gsi: 0, flags: 0 }; MAX_ISOS],
        n_isos: 0,
        nmis: [Nmi { uid: 0, flags: 0, lint: 0 }; MAX_NMIS],
        n_nmis: 0,
        dropped: 0,
    };
}

/// A `spin::Mutex` rather than a `Once`: the table is filled INCREMENTALLY by a callback the MADT
/// walk makes once per entry, and the census writes two fields back into it afterwards. Every taker
/// runs on the BSP outside interrupt context (the MADT walk at `acpi::init`, the census immediately
/// after it, and — from rung 2 — a driver's arm during its own `init`), so the lock is never
/// contended and is never taken from an ISR.
pub(crate) static CENSUS: spin::Mutex<Census> = spin::Mutex::new(Census::EMPTY);

/// Census lines emitted. Read by nothing yet; it exists so a boot can say how much of the MADT this
/// module actually understood without walking the table a second time.
static SEEN: AtomicU32 = AtomicU32::new(0);

/// Record one MADT entry. Called from `acpi::parse_madt`'s catch-all arm for EVERY entry type that
/// walk does not itself consume, so the three types below are picked out here rather than by
/// widening the topology match — the topology walk stays exactly the local-APIC list it was.
///
/// SAFETY: `base` must be the mapped address of a MADT entry whose declared length is `len`. Every
/// read below is bounds-checked against `len` first and is unaligned, because MADT entries are
/// byte-packed and firmware places them on no particular boundary.
pub unsafe fn madt_entry(etype: u8, base: usize, len: usize) {
    let mut c = CENSUS.lock();
    match etype {
        MADT_IO_APIC if len >= 12 => {
            let entry = IoApic {
                id: ((base + 2) as *const u8).read_unaligned(),
                addr: ((base + 4) as *const u32).read_unaligned(),
                gsi_base: ((base + 8) as *const u32).read_unaligned(),
                entries: 0,
                version: 0,
            };
            if c.n_ioapics < MAX_IOAPICS {
                let n = c.n_ioapics;
                c.ioapics[n] = entry;
                c.n_ioapics = n + 1;
            } else {
                c.dropped += 1;
            }
        }
        MADT_INT_SRC_OVERRIDE if len >= 10 => {
            let entry = Iso {
                bus: ((base + 2) as *const u8).read_unaligned(),
                irq: ((base + 3) as *const u8).read_unaligned(),
                gsi: ((base + 4) as *const u32).read_unaligned(),
                flags: ((base + 8) as *const u16).read_unaligned(),
            };
            if c.n_isos < MAX_ISOS {
                let n = c.n_isos;
                c.isos[n] = entry;
                c.n_isos = n + 1;
            } else {
                c.dropped += 1;
            }
        }
        MADT_LOCAL_APIC_NMI if len >= 6 => {
            let entry = Nmi {
                uid: ((base + 2) as *const u8).read_unaligned(),
                flags: ((base + 3) as *const u16).read_unaligned(),
                lint: ((base + 5) as *const u8).read_unaligned(),
            };
            if c.n_nmis < MAX_NMIS {
                let n = c.n_nmis;
                c.nmis[n] = entry;
                c.n_nmis = n + 1;
            } else {
                c.dropped += 1;
            }
        }
        _ => return,
    }
    SEEN.fetch_add(1, Ordering::Relaxed);
}

/// Select `reg` in IOREGSEL and read IOWIN (82093AA §3.1).
///
/// SAFETY: `addr` must be a mapped I/O APIC MMIO base. The pair is NOT atomic — a second agent
/// touching IOREGSEL between the two accesses would read the wrong register — which is why every
/// caller in this module holds `CENSUS` across the pair and why no ISR calls in here.
pub(crate) unsafe fn read_reg(addr: u32, reg: u32) -> u32 {
    core::ptr::write_volatile((addr as u64 + IOREGSEL) as *mut u32, reg);
    core::ptr::read_volatile((addr as u64 + IOWIN) as *const u32)
}

/// Print what the MADT declared and what the hardware answers, and map each controller's MMIO
/// window on the way through. Called from `acpi::init`, immediately after the MADT walk that filled
/// the table — the census is full exactly there and not one statement earlier.
///
/// READ-ONLY with respect to the redirection table: not one entry is written here, so a boot may
/// carry this census and route nothing. That is rung 1's whole contract.
pub fn census() {
    let mut c = CENSUS.lock();
    if c.n_ioapics == 0 {
        serial_println!(
            "[ioapic] census ioapics=0 isos={} gsis=0 nmis={} dropped={} — the MADT declares no I/O APIC on this machine, so INTx has nowhere to be delivered and every PCI function without a usable MSI capability stays POLLED, exactly as before this arc == witness ::",
            c.n_isos,
            c.n_nmis,
            c.dropped
        );
        return;
    }

    let mut gsis = 0u32;
    for i in 0..c.n_ioapics {
        let addr = c.ioapics[i].addr;
        // The LAPIC window at 0xFEE00000 is reached raw because UEFI identity-maps it; this one
        // sits 2 MiB below in the same firmware-reserved aperture and could be reached the same
        // way. `map_mmio_window` is called anyway rather than assumed: it asserts the UC typing the
        // register pair requires and CREATES the leaf if the boot map happens not to carry it,
        // which turns a would-be #PF on an address firmware handed us into a normal read. Safe
        // here — `acpi::init` runs long after the heap, so the frame allocator the window walk may
        // need is live.
        crate::arch::memory::map_mmio_window(addr as u64, 0x1000);
        let ver = unsafe { read_reg(addr, REG_VER) };
        let hw_id = unsafe { read_reg(addr, REG_ID) };
        // 0xFFFFFFFF is what an unclaimed MMIO read returns. A controller that answers that, or
        // that claims a maximum redirection entry of 0xFF, is REFUSED and left with `entries = 0`
        // so that rung 2 structurally cannot program it: a routed interrupt on a controller we
        // cannot read would be an interrupt with no way to prove it was ever armed.
        if ver == 0xFFFF_FFFF || ((ver >> 16) & 0xFF) == 0xFF {
            serial_println!(
                "[ioapic] id={} addr={:#x} gsi_base={} entries=0 version=0x00 REFUSED reason=ioapicver-unreadable (read {:#010x}) — nothing will ever be routed through this controller == witness ::",
                c.ioapics[i].id,
                addr,
                c.ioapics[i].gsi_base,
                ver
            );
            continue;
        }
        // §3.2.2: bits 23:16 are the MAXIMUM REDIRECTION ENTRY — the LAST VALID INDEX — so the
        // count is that plus one. Getting this off by one is how a kernel silently refuses the
        // highest GSI on the machine.
        let entries = (((ver >> 16) & 0xFF) as u8).saturating_add(1);
        let version = (ver & 0xFF) as u8;
        c.ioapics[i].entries = entries;
        c.ioapics[i].version = version;
        gsis += entries as u32;
        serial_println!(
            "[ioapic] id={} addr={:#x} gsi_base={} entries={} version={:#04x} hw_id={} == witness ::",
            c.ioapics[i].id,
            addr,
            c.ioapics[i].gsi_base,
            entries,
            version,
            (hw_id >> 24) & 0x0F
        );
    }

    for i in 0..c.n_isos {
        let iso = c.isos[i];
        serial_println!(
            "[ioapic] iso bus={} irq={} -> gsi={} polarity={} trigger={} flags={:#06x} == witness ::",
            iso.bus,
            iso.irq,
            iso.gsi,
            Polarity::from_flags(iso.flags).as_str(),
            Trigger::from_flags(iso.flags).as_str(),
            iso.flags
        );
    }

    for i in 0..c.n_nmis {
        let nmi = c.nmis[i];
        serial_println!(
            "[ioapic] nmi uid={} lint={} polarity={} trigger={} == witness ::",
            nmi.uid,
            nmi.lint,
            Polarity::from_flags(nmi.flags).as_str(),
            Trigger::from_flags(nmi.flags).as_str()
        );
    }

    serial_println!(
        "[ioapic] census ioapics={} isos={} gsis={} nmis={} dropped={} madt_entries={} == witness ::",
        c.n_ioapics,
        c.n_isos,
        gsis,
        c.n_nmis,
        c.dropped,
        SEEN.load(Ordering::Relaxed)
    );
}
