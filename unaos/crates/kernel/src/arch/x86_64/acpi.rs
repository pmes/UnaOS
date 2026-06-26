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

        match find_table(sdt_addr, entry_size, b"APIC") {
            Some(madt_addr) => parse_madt(madt_addr),
            None => {
                serial_println!("ACPI: MADT not found; assuming uniprocessor.");
                uniprocessor(DEFAULT_LAPIC)
            }
        }
    }
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
