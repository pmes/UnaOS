// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Minimal ACPI parser: just enough to discover the CPU topology for SMP bring-up.
//
// We hand-roll the small walk (RSDP -> XSDT/RSDT -> MADT) in the project's from-scratch,
// zero-dependency style rather than pulling in the `acpi` crate — the MADT walk is a few dozen
// lines and we only need the local-APIC list, so the crate's handler-trait machinery would be
// more abstraction than this warrants.
//
// All ACPI tables are identity-mapped (the bootloader uses physical_memory_offset 0), so a
// physical address is dereferenced directly. Tables are byte-packed and not naturally aligned,
// so every access goes through `read_unaligned` / per-field byte reads.

use crate::arch::gdt::MAX_CPUS;
use x86_64::instructions::port::Port;

/// Root System Description Pointer. Revision 0 = ACPI 1.0 (only the first 20 bytes are valid,
/// use `rsdt_addr`); revision >= 2 = ACPI 2.0+ (the full struct is valid, use `xsdt_addr`).
#[repr(C, packed)]
struct Rsdp {
    signature: [u8; 8], // "RSD PTR "
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_addr: u32,
    length: u32,
    xsdt_addr: u64,
    ext_checksum: u8,
    reserved: [u8; 3],
}

/// The 36-byte header shared by every ACPI system description table (XSDT, RSDT, MADT, ...).
#[repr(C, packed)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

const SDT_HEADER_LEN: usize = 36;
/// MADT layout: SdtHeader (36) + Local APIC address (u32) + Flags (u32); entries follow at 44.
const MADT_ENTRIES_OFFSET: usize = 44;

/// MADT entry type for a Processor Local APIC (one per logical CPU, APIC id <= 255).
const MADT_LOCAL_APIC: u8 = 0;
/// MADT entry type for a Processor Local x2APIC (32-bit APIC id, used for ids > 255).
const MADT_LOCAL_X2APIC: u8 = 9;
/// Processor flags: bit 0 = Enabled, bit 1 = Online Capable. Count a CPU if either is set.
const PROC_USABLE_MASK: u32 = 0b11;

/// Discovered CPU topology. Populated once on the BSP during early boot, then read-only (the
/// APs only read it), so a `spin::Once` is all the synchronisation we need.
pub struct Topology {
    apic_ids: [u32; MAX_CPUS],
    count: usize,
    /// Local APIC MMIO base reported by the MADT (architectural default 0xFEE00000).
    pub local_apic_addr: u64,
}

impl Topology {
    /// APIC ids of the usable CPUs, BSP first as listed by firmware.
    pub fn apic_ids(&self) -> &[u32] {
        &self.apic_ids[..self.count]
    }

    pub fn count(&self) -> usize {
        self.count
    }

    fn push(&mut self, apic_id: u32) {
        // Dedup (a CPU should appear once) and respect the static capacity.
        if self.count >= MAX_CPUS || self.apic_ids[..self.count].contains(&apic_id) {
            return;
        }
        self.apic_ids[self.count] = apic_id;
        self.count += 1;
    }
}

static TOPOLOGY: spin::Once<Topology> = spin::Once::new();

/// Parse the ACPI tables and record the CPU topology. `rsdp_addr` is the physical RSDP address
/// the bootloader found in the UEFI config table (0 if none). Always succeeds: anything missing
/// or malformed degrades gracefully to "this CPU only". Prints the discovered topology.
pub fn init(rsdp_addr: u64) {
    let topo = TOPOLOGY.call_once(|| parse(rsdp_addr));
    serial_println!(
        "ACPI: {} CPU(s) discovered, local APIC @ {:#x}, apic ids {:?}",
        topo.count,
        topo.local_apic_addr,
        topo.apic_ids()
    );
}

/// The discovered topology, if `init` has run.
pub fn topology() -> Option<&'static Topology> {
    TOPOLOGY.get()
}

/// Number of usable CPUs (1 if discovery has not run or found nothing).
pub fn cpu_count() -> usize {
    TOPOLOGY.get().map(|t| t.count).unwrap_or(1)
}

/// Fallback topology: just the CPU we're running on (the BSP), read from its local APIC.
fn uniprocessor(local_apic_addr: u64) -> Topology {
    let mut topo = Topology { apic_ids: [0; MAX_CPUS], count: 0, local_apic_addr };
    topo.push(crate::arch::apic::apic_id() as u32);
    topo
}

fn parse(rsdp_addr: u64) -> Topology {
    const DEFAULT_LAPIC: u64 = 0xFEE0_0000;

    if rsdp_addr == 0 {
        serial_println!("ACPI: no RSDP from bootloader; assuming uniprocessor.");
        return uniprocessor(DEFAULT_LAPIC);
    }

    unsafe {
        let rsdp = (rsdp_addr as *const Rsdp).read_unaligned();
        let signature = rsdp.signature;
        if &signature != b"RSD PTR " {
            serial_println!("ACPI: bad RSDP signature; assuming uniprocessor.");
            return uniprocessor(DEFAULT_LAPIC);
        }

        // ACPI 2.0+ (revision >= 2) gives a 64-bit XSDT; ACPI 1.0 gives a 32-bit RSDT.
        let revision = rsdp.revision;
        let xsdt_addr = rsdp.xsdt_addr;
        let rsdt_addr = rsdp.rsdt_addr;
        let (sdt_addr, entry_size): (u64, usize) = if revision >= 2 && xsdt_addr != 0 {
            (xsdt_addr, 8)
        } else {
            (rsdt_addr as u64, 4)
        };
        // Retain the root SDT so later subsystems can look up other tables by signature
        // (read-only; first user: the EHCI-3 DMAR/VT-d evidence probe).
        SDT_ROOT.call_once(|| (sdt_addr, entry_size));

        match find_table(sdt_addr, entry_size, b"APIC") {
            Some(madt_addr) => parse_madt(madt_addr),
            None => {
                serial_println!("ACPI: MADT not found; assuming uniprocessor.");
                uniprocessor(DEFAULT_LAPIC)
            }
        }
    }
}

static SDT_ROOT: spin::Once<(u64, usize)> = spin::Once::new();

/// Find any ACPI table by 4-byte signature (after `init` has run). Read-only walk; `None`
/// when discovery never ran or the table is absent.
pub fn find_acpi_table(sig: &[u8; 4]) -> Option<u64> {
    SDT_ROOT.get().and_then(|&(addr, sz)| unsafe { find_table(addr, sz, sig) })
}

/// Walk the XSDT/RSDT entry list looking for a table with the given 4-byte signature.
/// `entry_size` is 8 for an XSDT (64-bit pointers) or 4 for an RSDT (32-bit pointers).
unsafe fn find_table(sdt_addr: u64, entry_size: usize, sig: &[u8; 4]) -> Option<u64> {
    let hdr = (sdt_addr as *const SdtHeader).read_unaligned();
    let length = hdr.length as usize;
    let n = length.saturating_sub(SDT_HEADER_LEN) / entry_size;
    let entries_base = sdt_addr + SDT_HEADER_LEN as u64;

    for i in 0..n {
        let slot = entries_base + (i * entry_size) as u64;
        let table_addr: u64 = if entry_size == 8 {
            (slot as *const u64).read_unaligned()
        } else {
            (slot as *const u32).read_unaligned() as u64
        };
        let th = (table_addr as *const SdtHeader).read_unaligned();
        let th_sig = th.signature;
        if &th_sig == sig {
            return Some(table_addr);
        }
    }
    None
}

/// Parse the MADT (Multiple APIC Description Table) into a CPU topology by walking its
/// variable-length entry list and collecting usable Local APIC / x2APIC ids.
unsafe fn parse_madt(madt_addr: u64) -> Topology {
    let hdr = (madt_addr as *const SdtHeader).read_unaligned();
    let total_len = hdr.length as usize;

    let local_apic_addr = ((madt_addr as usize + SDT_HEADER_LEN) as *const u32).read_unaligned();
    let mut topo = Topology {
        apic_ids: [0; MAX_CPUS],
        count: 0,
        local_apic_addr: local_apic_addr as u64,
    };

    let mut off = MADT_ENTRIES_OFFSET;
    while off + 2 <= total_len {
        let base = madt_addr as usize + off;
        let entry_type = (base as *const u8).read_unaligned();
        let entry_len = ((base + 1) as *const u8).read_unaligned() as usize;
        if entry_len < 2 || off + entry_len > total_len {
            break; // malformed / truncated — stop rather than loop or read past the table
        }

        match entry_type {
            MADT_LOCAL_APIC => {
                // type(0) len(1) acpi_uid(2) apic_id(3) flags(4..8)
                let apic_id = ((base + 3) as *const u8).read_unaligned() as u32;
                let flags = ((base + 4) as *const u32).read_unaligned();
                if flags & PROC_USABLE_MASK != 0 {
                    topo.push(apic_id);
                }
            }
            MADT_LOCAL_X2APIC => {
                // type(0) len(1) reserved(2..4) x2apic_id(4..8) flags(8..12) acpi_uid(12..16)
                let x2apic_id = ((base + 4) as *const u32).read_unaligned();
                let flags = ((base + 8) as *const u32).read_unaligned();
                if flags & PROC_USABLE_MASK != 0 {
                    topo.push(x2apic_id);
                }
            }
            _ => {}
        }
        off += entry_len;
    }

    if topo.count == 0 {
        // Never report zero CPUs — fall back to the running CPU.
        topo.push(crate::arch::apic::apic_id() as u32);
    }
    topo
}

// --- DMA Remapping (Intel VT-d) detection (F5) ---

/// DMAR remapping-structure type for a DRHD (DMA Remapping Hardware Unit Definition).
const DMAR_DRHD: u16 = 0;
/// First remapping structure offset within the DMAR table: after the 12-byte body that follows the
/// SDT header (Host Address Width(1) + Flags(1) + Reserved(10)).
const DMAR_STRUCTS_OFFSET: usize = SDT_HEADER_LEN + 12; // 48
/// Global Status Register offset within a DRHD's register set.
const DMAR_GSTS: u64 = 0x1C;
/// GSTS.TES — Translation Enable Status (bit 31): 1 = DMA remapping is actively translating.
const DMAR_GSTS_TES: u32 = 1 << 31;

/// Detect Intel VT-d DMA remapping and report whether it would block the kernel's device DMA.
///
/// The kernel programs no DMA-remapping domains and DMAs physical==bus addresses to identity-mapped
/// heap buffers (xHCI rings/transfer buffers, the e1000 descriptors). If firmware has an IOMMU with
/// translation ENABLED, untranslated device DMA is faulted/blocked — a hard, silent boot failure on
/// metal. This is read-only detection: walk RSDP -> XSDT/RSDT -> "DMAR", and for the first DRHD read
/// GSTS.TES to see whether translation is actively on. Surfaced on the (serial-less) framebuffer so a
/// metal boot can see it; the fix is to disable VT-d in firmware (or add DMAR passthrough — a larger
/// follow-up). Clean "absent" report when there is no DMAR table (QEMU default; Macs that ship VT-d
/// off). 2012 rMBP firmware typically leaves VT-d disabled, but this turns a maybe-blocker into a
/// visible fact instead of a mystery.
pub fn dmar_report(rsdp_addr: u64) {
    if rsdp_addr == 0 {
        return; // no ACPI at all; acpi::init already warned
    }
    unsafe {
        let rsdp = (rsdp_addr as *const Rsdp).read_unaligned();
        if &rsdp.signature != b"RSD PTR " {
            return;
        }
        let (sdt_addr, entry_size): (u64, usize) = if rsdp.revision >= 2 && rsdp.xsdt_addr != 0 {
            (rsdp.xsdt_addr, 8)
        } else {
            (rsdp.rsdt_addr as u64, 4)
        };

        let dmar_addr = match find_table(sdt_addr, entry_size, b"DMAR") {
            Some(a) => a,
            None => {
                serial_println!("DMAR: no IOMMU table (VT-d absent or disabled in firmware) — direct device DMA OK.");
                return;
            }
        };

        let hdr = (dmar_addr as *const SdtHeader).read_unaligned();
        let total_len = hdr.length as usize;
        let flags = ((dmar_addr as usize + SDT_HEADER_LEN + 1) as *const u8).read_unaligned();

        // Walk the remapping structures; for the first DRHD read GSTS.TES from its register set.
        let mut off = DMAR_STRUCTS_OFFSET;
        let mut drhd_count = 0u32;
        let mut first_reg_base = 0u64;
        let mut translation_on = false;
        while off + 4 <= total_len {
            let base = dmar_addr as usize + off;
            let stype = (base as *const u16).read_unaligned();
            let slen = ((base + 2) as *const u16).read_unaligned() as usize;
            if slen < 4 || off + slen > total_len {
                break; // malformed / truncated — stop rather than read past the table
            }
            if stype == DMAR_DRHD {
                drhd_count += 1;
                // DRHD: type(2) len(2) flags(1) rsvd(1) segment(2) register_base(8 @ offset 8).
                let reg_base = ((base + 8) as *const u64).read_unaligned();
                if first_reg_base == 0 && reg_base != 0 {
                    first_reg_base = reg_base;
                    let gsts = ((reg_base + DMAR_GSTS) as *const u32).read_volatile();
                    translation_on = (gsts & DMAR_GSTS_TES) != 0;
                }
            }
            off += slen;
        }

        if translation_on {
            serial_println!(
                "DMAR: *** VT-d translation ENABLED *** ({} DRHD, reg @ {:#x}, flags {:#x}) — device \
                 DMA to identity-mapped heap may be BLOCKED. Disable VT-d in firmware (or add DMAR passthrough).",
                drhd_count, first_reg_base, flags
            );
        } else {
            serial_println!(
                "DMAR: IOMMU present ({} DRHD, reg @ {:#x}, flags {:#x}) but translation OFF — direct device DMA OK.",
                drhd_count, first_reg_base, flags
            );
        }
    }
}

// --- ACPI Power Management Timer (timebase calibration reference) ---

/// The ACPI PM timer's fixed, spec-mandated frequency (ACPI §4.8.3.3). It is the same 3.579545 MHz
/// on every PC regardless of CPU speed / P-states / the (unknown, per-machine) local-APIC timer
/// crystal, so it is the reference clock we calibrate the TSC and APIC timer against on real
/// hardware — where no CPUID leaf 0x15/0x16 exists before Skylake to hand us those rates. See
/// `apic::calibrate`.
pub const PM_TIMER_HZ: u64 = 3_579_545;

/// FADT ("FACP") field offsets (ACPI spec; stable since 1.0 — later revisions only append fields).
const FADT_PM_TMR_BLK: usize = 76; // u32: I/O port of the PM timer counter (legacy 32-bit field)
const FADT_FLAGS: usize = 112; // u32: fixed-feature flags
const FADT_X_PM_TMR_BLK: usize = 208; // 12-byte Generic Address Structure (ACPI 2.0+); preferred
/// FADT flags bit 8 (TMR_VAL_EXT): 1 = the counter is 32-bit, 0 = 24-bit (top 8 bits reserved).
const FADT_FLAG_TMR_VAL_EXT: u32 = 1 << 8;
/// Generic Address Structure `AddressSpaceId` for System I/O space.
const GAS_SPACE_SYSTEM_IO: u8 = 1;

/// A discovered ACPI PM timer: the I/O port of its counter register and a mask selecting the
/// significant bits (24- or 32-bit per the FADT flags). The counter increments at `PM_TIMER_HZ`
/// and wraps at `mask + 1` (24-bit wraps every ~4.687 s; 32-bit every ~20 min).
#[derive(Clone, Copy)]
pub struct PmTimer {
    port: u16,
    mask: u32,
}

impl PmTimer {
    /// Current counter value, masked to its significant width. Monotonic modulo `mask + 1`.
    #[inline]
    pub fn read(&self) -> u32 {
        // SAFETY: reading the ACPI PM timer I/O port is side-effect-free (a free-running counter).
        // The port is the one firmware advertised in the FADT.
        let raw: u32 = unsafe { Port::<u32>::new(self.port).read() };
        raw & self.mask
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Ticks elapsed from `start` to `now`, correct across a single counter wrap. Valid only when
    /// the true elapsed count is < `mask + 1` (keep any measurement window well under the wrap
    /// period: ~4.687 s for a 24-bit counter).
    #[inline]
    pub fn delta(&self, start: u32, now: u32) -> u32 {
        now.wrapping_sub(start) & self.mask
    }

    /// Significant width in bits (24 or 32) — for the wrap mask and diagnostics.
    pub fn bits(&self) -> u32 {
        if self.mask == 0xFFFF_FFFF {
            32
        } else {
            24
        }
    }
}

/// Discover the ACPI PM timer by parsing the FADT ("FACP"). Returns `None` if there is no
/// ACPI/FADT or the firmware advertises no PM timer (then the timebase stays uncalibrated).
///
/// Prefers the ACPI 2.0+ extended `X_PM_TMR_BLK` GAS when it is present, populated, and in System
/// I/O space; otherwise falls back to the legacy 32-bit `PM_TMR_BLK` field. The 24-vs-32-bit width
/// comes from FADT flags bit 8 (TMR_VAL_EXT).
pub fn pm_timer(rsdp_addr: u64) -> Option<PmTimer> {
    if rsdp_addr == 0 {
        return None;
    }
    unsafe {
        let rsdp = (rsdp_addr as *const Rsdp).read_unaligned();
        if &rsdp.signature != b"RSD PTR " {
            return None;
        }
        let (sdt_addr, entry_size): (u64, usize) = if rsdp.revision >= 2 && rsdp.xsdt_addr != 0 {
            (rsdp.xsdt_addr, 8)
        } else {
            (rsdp.rsdt_addr as u64, 4)
        };

        let fadt = find_table(sdt_addr, entry_size, b"FACP")?;
        let hdr = (fadt as *const SdtHeader).read_unaligned();
        let len = hdr.length as usize;

        // Prefer the extended GAS if the table is long enough to contain it and it names a
        // non-zero System-I/O port (a real port fits in 16 bits).
        let mut port: Option<u16> = None;
        if len >= FADT_X_PM_TMR_BLK + 12 {
            let gas = fadt as usize + FADT_X_PM_TMR_BLK;
            let space = (gas as *const u8).read_unaligned();
            let addr = ((gas + 4) as *const u64).read_unaligned();
            if space == GAS_SPACE_SYSTEM_IO && addr != 0 && addr <= 0xFFFF {
                port = Some(addr as u16);
            }
        }
        // Fall back to the legacy 32-bit PM_TMR_BLK field.
        if port.is_none() && len >= FADT_PM_TMR_BLK + 4 {
            let legacy = ((fadt as usize + FADT_PM_TMR_BLK) as *const u32).read_unaligned();
            if legacy != 0 && legacy <= 0xFFFF {
                port = Some(legacy as u16);
            }
        }
        let port = port?;

        let flags = if len >= FADT_FLAGS + 4 {
            ((fadt as usize + FADT_FLAGS) as *const u32).read_unaligned()
        } else {
            0
        };
        let mask = if flags & FADT_FLAG_TMR_VAL_EXT != 0 {
            0xFFFF_FFFF
        } else {
            0x00FF_FFFF
        };

        Some(PmTimer { port, mask })
    }
}

/// One-time diagnostic: discover the PM timer and prove it advances (three reads separated by a
/// short rdtsc-bounded spin). Surfaced on the framebuffer so a serial-less metal boot can confirm
/// the calibration reference clock is live *before* trusting any TSC/APIC frequency derived from
/// it. A "STUCK?" verdict means the port is wrong or the counter is dead — calibration must then
/// fall back to the fixed timer values.
pub fn pm_timer_report(rsdp_addr: u64) {
    match pm_timer(rsdp_addr) {
        Some(pm) => {
            // Short spins between reads. now_cycles() (rdtsc) is uncalibrated here, but ~1e6 cycles
            // is well under a millisecond on any part yet long enough for the 3.58 MHz PM counter
            // to tick — enough to witness advance, not to measure anything.
            let spin = |cycles: u64| {
                let start = crate::arch::now_cycles();
                while crate::arch::now_cycles().wrapping_sub(start) < cycles {
                    core::hint::spin_loop();
                }
            };
            let a = pm.read();
            spin(1_000_000);
            let b = pm.read();
            spin(1_000_000);
            let c = pm.read();
            serial_println!(
                "ACPI PM timer: port {:#x}, {}-bit; {:#x} -> {:#x} -> {:#x} ({})",
                pm.port(),
                pm.bits(),
                a,
                b,
                c,
                if b != a || c != b { "advancing" } else { "STUCK?" }
            );
        }
        None => serial_println!(
            "ACPI PM timer: not found (no FADT / no PM_TMR_BLK) — timebase stays uncalibrated."
        ),
    }
}
