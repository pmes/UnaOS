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
// RUNG 1 IS THE CENSUS (`madt_entry` / `census`): it reads what firmware declares and what the
// hardware answers, and writes NOTHING. RUNG 2 IS THE ROUTE (`route_gsi` / `set_mask` / `unroute` /
// `route_pci_intx`): the first redirection-entry writer this kernel has ever had. The PCI arm that
// calls it from a driver is rung 3.
//
// CLEAN ROOM. Every register field below is cited to a public specification and to nothing else:
// the Intel 82093AA I/O APIC datasheet (§3.1 the IOREGSEL/IOWIN window, §3.2.1 IOAPICID, §3.2.2
// IOAPICVER, §3.2.4 the 64-bit redirection entry), ACPI 6.x §5.2.12 (MADT: §5.2.12.3 I/O APIC,
// §5.2.12.5 Interrupt Source Override and the MPS INTI flags, §5.2.12.7 Local APIC NMI), and PCI
// Local Bus 3.0 (§2.2.6 INTx is level-triggered and active low, §6.2.4 the Interrupt Line and
// Interrupt Pin configuration registers).
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
/// IOREDTBL[0] low dword (§3.2.4). Entry n occupies index 0x10 + 2n (low) and 0x11 + 2n (high).
const REG_REDTBL: u32 = 0x10;

// ── Intel 82093AA §3.2.4 — the 64-bit redirection entry, field by field ────────────────────────
/// Bits 10:8 delivery mode. `000` = Fixed: deliver `vector` to the listed destination, with no
/// arbitration and no redirection hint. The only mode this kernel programs.
const DELIVERY_FIXED: u32 = 0b000 << 8;
/// Bit 11 destination mode. 0 = physical (bits 63:56 are an APIC id), 1 = logical.
const DEST_PHYSICAL: u32 = 0 << 11;
/// Bit 13 interrupt input pin polarity: 0 = active high, 1 = active low.
const POLARITY_LOW: u32 = 1 << 13;
/// Bit 15 trigger mode: 0 = edge, 1 = level.
const TRIGGER_LEVEL: u32 = 1 << 15;
/// Bit 16 interrupt mask: 1 = this entry delivers nothing.
const MASK: u32 = 1 << 16;
/// Bits 12 (delivery status) and 14 (remote IRR) are READ-ONLY status the controller owns, so they
/// are excluded from every read-back comparison. Everything else in the low dword is ours.
const RO_STATUS: u32 = (1 << 12) | (1 << 14);
/// A redirection entry with nothing in it but the mask — what `unroute` leaves behind, and the
/// controller's own post-reset state (§3.2.4: the mask bit is set to 1 after a hardware reset).
const ENTRY_MASKED_EMPTY: u32 = MASK;

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

/// Census lines emitted. It exists so a boot can say how much of the MADT this module actually
/// understood without walking the table a second time.
static SEEN: AtomicU32 = AtomicU32::new(0);

/// Redirection entries this module currently holds programmed. Reported on the arm line so a boot
/// can say how many interrupts this kernel routed without walking the hardware again.
static ROUTED: AtomicU32 = AtomicU32::new(0);

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

/// Select `reg` in IOREGSEL and write IOWIN (82093AA §3.1). Same non-atomicity note as `read_reg`;
/// the same lock discipline answers it.
///
/// SAFETY: `addr` must be a mapped I/O APIC MMIO base.
pub(crate) unsafe fn write_reg(addr: u32, reg: u32, val: u32) {
    core::ptr::write_volatile((addr as u64 + IOREGSEL) as *mut u32, reg);
    core::ptr::write_volatile((addr as u64 + IOWIN) as *mut u32, val);
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

// ── RUNG 2: THE ROUTE ──────────────────────────────────────────────────────────────────────────
//
// THE FIRST REDIRECTION-ENTRY WRITER IN THIS KERNEL'S HISTORY. Everything above this line reads;
// everything below it can write exactly one thing — a 64-bit IOREDTBL entry — and reads it back
// before it will say it did.

/// Why a route could not be programmed. Every one of these is a REFUSAL that leaves the machine
/// exactly as it was, which is the same posture `ehci::isr_arm_controller` takes about MSI: a
/// half-armed interrupt controller is worse than an unarmed one, because the driver's fallback is
/// correct and its own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RouteErr {
    /// No controller's `[gsi_base, gsi_base + entries)` range contains this GSI.
    NoController,
    /// The controller owning this GSI failed its IOAPICVER read in `census`, so `entries` is 0 and
    /// it is structurally unroutable. See the refusal `census` prints for it.
    Unreadable,
    /// The entry read back as something other than what was written.
    Readback,
}

impl RouteErr {
    pub fn as_str(self) -> &'static str {
        match self {
            RouteErr::NoController => "no-controller-owns-this-gsi",
            RouteErr::Unreadable => "controller-unreadable",
            RouteErr::Readback => "entry-readback-mismatch",
        }
    }
}

/// Find the controller index and redirection-entry index for a GSI. `None` when no controller owns
/// it, or when the owning controller was refused by the census (`entries == 0`) — which is what
/// makes an unreadable controller unroutable rather than merely undocumented.
fn locate(c: &Census, gsi: u32) -> Option<(usize, u32)> {
    for i in 0..c.n_ioapics {
        let ia = c.ioapics[i];
        if ia.entries == 0 || ia.addr == 0 {
            continue;
        }
        if gsi >= ia.gsi_base && gsi < ia.gsi_base + ia.entries as u32 {
            return Some((i, gsi - ia.gsi_base));
        }
    }
    None
}

/// Program ONE redirection entry, MASKED, and return the 64-bit entry as read back.
///
/// The entry is built per 82093AA §3.2.4: `vector` in bits 7:0, delivery mode Fixed, physical
/// destination mode, the caller's polarity and trigger, the destination APIC id in bits 63:56, and
/// the MASK bit SET.
///
/// **MASKED IS NOT A DETAIL.** An entry that goes live the instant it is written can deliver to a
/// vector whose handler the caller has not finished preparing — and the caller is the only code
/// that knows when that is true. Unmasking is `set_mask`, a separate call, made by the caller when
/// it is ready.
///
/// **THE WRITE ORDER IS LOAD-BEARING TOO:** high dword (destination) first, low dword second, so
/// the dword carrying the vector and the mask is the last thing the controller latches.
///
/// Then it READS THE ENTRY BACK AND COMPARES — "programmed" is a measurement here, not a write we
/// hope landed, which is the same rule `isr_arm_controller` applies to `USBINTR`. Bits 12 and 14
/// are the controller's own read-only status and are excluded; every field this function chose is
/// compared, and a mismatch is a refusal.
pub fn route_gsi(
    gsi: u32,
    vector: u8,
    polarity: Polarity,
    trigger: Trigger,
    dest_apic: u8,
) -> Result<u64, RouteErr> {
    let c = CENSUS.lock();
    let (i, idx) = locate(&c, gsi).ok_or(RouteErr::NoController)?;
    let addr = c.ioapics[i].addr;

    let mut lo = vector as u32 | DELIVERY_FIXED | DEST_PHYSICAL | MASK;
    if polarity == Polarity::ActiveLow {
        lo |= POLARITY_LOW;
    }
    if trigger == Trigger::Level {
        lo |= TRIGGER_LEVEL;
    }
    let hi = (dest_apic as u32) << 24;

    let (rlo, rhi) = unsafe {
        write_reg(addr, REG_REDTBL + 2 * idx + 1, hi);
        write_reg(addr, REG_REDTBL + 2 * idx, lo);
        (
            read_reg(addr, REG_REDTBL + 2 * idx),
            read_reg(addr, REG_REDTBL + 2 * idx + 1),
        )
    };
    if (rlo & !RO_STATUS) != (lo & !RO_STATUS) || (rhi & 0xFF00_0000) != hi {
        return Err(RouteErr::Readback);
    }
    ROUTED.fetch_add(1, Ordering::Relaxed);
    Ok(((rhi as u64) << 32) | rlo as u64)
}

/// Set or clear the MASK bit (§3.2.4 bit 16) on an already-programmed entry, leaving every other
/// field exactly as it was. Read-modify-write, then read back — an unmask that did not stick is
/// precisely the state that makes an ISR silently absent, so it is measured rather than assumed.
/// Returns the entry's low dword as read back.
pub fn set_mask(gsi: u32, masked: bool) -> Result<u32, RouteErr> {
    let c = CENSUS.lock();
    let (i, idx) = locate(&c, gsi).ok_or(RouteErr::NoController)?;
    let addr = c.ioapics[i].addr;
    unsafe {
        let lo = read_reg(addr, REG_REDTBL + 2 * idx);
        let next = if masked { lo | MASK } else { lo & !MASK };
        write_reg(addr, REG_REDTBL + 2 * idx, next);
        let back = read_reg(addr, REG_REDTBL + 2 * idx);
        if (back & MASK) != (next & MASK) {
            return Err(RouteErr::Readback);
        }
        Ok(back)
    }
}

/// Retire a route: put the entry back to the controller's post-reset shape (§3.2.4). The mask is
/// written in the SAME dword as the cleared vector, so the entry can never be briefly live with a
/// vector of zero.
pub fn unroute(gsi: u32) -> Result<(), RouteErr> {
    let c = CENSUS.lock();
    let (i, idx) = locate(&c, gsi).ok_or(RouteErr::NoController)?;
    let addr = c.ioapics[i].addr;
    unsafe {
        write_reg(addr, REG_REDTBL + 2 * idx, ENTRY_MASKED_EMPTY);
        write_reg(addr, REG_REDTBL + 2 * idx + 1, 0);
        if read_reg(addr, REG_REDTBL + 2 * idx) & MASK == 0 {
            return Err(RouteErr::Readback);
        }
    }
    ROUTED.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

/// How many redirection entries this module currently holds programmed.
pub fn routed() -> u32 {
    ROUTED.load(Ordering::Relaxed)
}

/// Which GSI does this PCI function's INTx pin land on, and with what polarity and trigger?
///
/// THE ANSWER COMES FROM THE FUNCTION'S OWN CONFIG SPACE (PCI 3.0 §6.2.4): Interrupt Pin at 0x3D
/// says which of INTA#..INTD# it asserts (0 = none), and Interrupt Line at 0x3C is the IRQ number
/// FIRMWARE programmed for it.
///
/// ⚠ THE DSDT `_PRT` IS NOT CONSULTED, AND THAT IS THIS RUNG'S STATED LIMIT, not an oversight. The
/// authoritative PCI interrupt routing table is `_PRT`, which is AML — an interpreter this kernel
/// does not have and this arc does not write. So the mapping is: take firmware's own Interrupt
/// Line and push it through the Interrupt Source Override table (identity when no override names
/// it). On a machine whose firmware left Interrupt Line unprogrammed the honest answer is REFUSED
/// — `reason=no-firmware-line`, printed — and the caller keeps polling exactly as it did before.
/// See docs/dev/OS/01_BOOT_HAL/ioapic.md §4.
///
/// THE DEFAULT POLARITY AND TRIGGER ARE PCI'S, NOT ISA'S: INTx is LEVEL TRIGGERED and ACTIVE LOW
/// (PCI 3.0 §2.2.6). An override naming this line wins over that default, because an override is
/// firmware telling us about this specific line; an override whose flags say `bus-default` leaves
/// the PCI default in place, which is what `BusDefault` being its own value buys.
pub fn route_pci_intx(bus: u8, dev: u8, func: u8) -> Option<(u32, Polarity, Trigger)> {
    let intr = unsafe { crate::arch::pci::read_config_32(bus, dev, func, 0x3C) };
    let line = (intr & 0xFF) as u8;
    let pin = ((intr >> 8) & 0xFF) as u8;
    let pin_name = if pin >= 1 && pin <= 4 { (b'A' + pin - 1) as char } else { '?' };

    if pin == 0 || pin > 4 {
        serial_println!(
            "[ioapic] route bdf={}:{}.{} pin=INT{} line={} -> REFUSED reason=no-intx-pin — this function declares no INTx pin, so there is no legacy assertion to route and nothing was written == witness ::",
            bus, dev, func, pin_name, line
        );
        return None;
    }
    if line == 0 || line == 0xFF {
        serial_println!(
            "[ioapic] route bdf={}:{}.{} pin=INT{} line={} -> REFUSED reason=no-firmware-line — firmware programmed no IRQ for this function and this rung does not interpret the DSDT _PRT, so the GSI is UNKNOWN rather than guessed; nothing was written == witness ::",
            bus, dev, func, pin_name, line
        );
        return None;
    }

    let (mut gsi, mut pol, mut trig) = (line as u32, Polarity::ActiveLow, Trigger::Level);
    let mut via = "identity";
    {
        let c = CENSUS.lock();
        for i in 0..c.n_isos {
            let iso = c.isos[i];
            if iso.bus == 0 && iso.irq == line {
                gsi = iso.gsi;
                if Polarity::from_flags(iso.flags) != Polarity::BusDefault {
                    pol = Polarity::from_flags(iso.flags);
                }
                if Trigger::from_flags(iso.flags) != Trigger::BusDefault {
                    trig = Trigger::from_flags(iso.flags);
                }
                via = "iso";
                break;
            }
        }
    }

    serial_println!(
        "[ioapic] route bdf={}:{}.{} pin=INT{} line={} -> gsi={} via={} polarity={} trigger={} == witness ::",
        bus, dev, func, pin_name, line, gsi, via, pol.as_str(), trig.as_str()
    );
    Some((gsi, pol, trig))
}

// ── RUNG 3: THE PCI ARM ────────────────────────────────────────────────────────────────────────

/// Route this PCI function's INTx to `vector` and unmask it. `true` means an interrupt that was
/// previously UNDELIVERABLE now has somewhere to go; `false` means nothing was changed and the
/// caller must keep whatever fallback it had.
///
/// **THE ORDER IS THE ARGUMENT.** The redirection entry is programmed MASKED; the function's PCI
/// Interrupt Disable bit (COMMAND bit 10, PCI 3.0 §6.2.2) is cleared so INTA# can assert at all;
/// and only THEN is the entry unmasked. Unmasking first would open a window in which a pin
/// firmware left asserted delivers to a vector before the function was ready — the same failure
/// `ehci::isr_arm_controller` already avoids by publishing its operational base before it touches
/// MSI, and for the same reason.
///
/// **THE INTERRUPT DISABLE BIT IS THE QUIETEST WAY THIS FAILS.** `enable_msi` sets it by
/// implication (MSI masks INTx per spec) and firmware may have left it set; an entry routed
/// correctly to a function that cannot assert is a perfect route and a dead interrupt. It is
/// cleared and READ BACK, and the read-back value rides the arm line.
///
/// **NO EOI SPECIAL CASE, and it is worth saying why there is none** — a reader will look for one.
/// A level-triggered entry's Remote IRR is cleared by the LOCAL APIC's EOI broadcast on every part
/// this kernel runs on (Intel SDM Vol. 3 §10.8.5; 82093AA §3.2.4), and every handler in
/// `interrupts.rs` already writes the local-APIC EOI register last. So the handler side needed no
/// change and got none.
pub fn route_pci_function(bus: u8, dev: u8, func: u8, vector: u8) -> bool {
    let (gsi, pol, trig) = match route_pci_intx(bus, dev, func) {
        Some(v) => v,
        None => return false, // route_pci_intx has already printed the reason
    };
    let dest = crate::arch::apic::apic_id();

    let entry = match route_gsi(gsi, vector, pol, trig, dest) {
        Ok(e) => e,
        Err(e) => {
            serial_println!(
                "[ioapic] route bdf={}:{}.{} gsi={} vector={:#04x} -> REFUSED reason={} — the redirection entry was not programmed and nothing on this function changed == witness ::",
                bus, dev, func, gsi, vector, e.as_str()
            );
            return false;
        }
    };

    let cmd = unsafe { crate::arch::pci::read_config_16(bus, dev, func, 0x04) };
    if cmd & (1 << 10) != 0 {
        unsafe { crate::arch::pci::write_config_16(bus, dev, func, 0x04, cmd & !(1u16 << 10)) };
    }
    let cmd_back = unsafe { crate::arch::pci::read_config_16(bus, dev, func, 0x04) };

    let unmasked = match set_mask(gsi, false) {
        Ok(lo) => lo,
        Err(e) => {
            serial_println!(
                "[ioapic] route bdf={}:{}.{} gsi={} -> REFUSED reason=unmask-{} — the entry is programmed but still MASKED, so the vector could never fire == witness ::",
                bus, dev, func, gsi, e.as_str()
            );
            return false;
        }
    };

    serial_println!(
        "[ioapic] armed bdf={}:{}.{} gsi={} vector={:#04x} dest_apic={} entry={:#018x} unmasked_lo={:#010x} intx_disable={} routed={} == witness ::",
        bus, dev, func, gsi, vector, dest, entry, unmasked,
        (cmd_back >> 10) & 1,
        routed()
    );
    true
}
