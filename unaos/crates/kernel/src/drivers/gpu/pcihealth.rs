// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// pcihealth.rs — PCIH: PCIe link-health witness for the Kepler BAR1 wedge theory.
//
// Two metal wedges minutes apart, both inside the BAR1 VRAM aperture — one core held forever
// in a posted WC write burst (phase 3), one in a non-posted read-back (phase 4). Working
// theory: the GK107 host interface stops accepting transactions (no driver power management;
// ASPM state unknown). This module turns that theory into three cheap facts:
//
//   1. BOOT-TIME LINK CENSUS (`census`, unconditional on every kepler boot): one `[pcih] ep`
//      line and one `[pcih] rp` line — PCIe LNKCAP/LNKCTL/LNKSTA, DEVCTL/DEVSTA, the decoded
//      ASPM enable state, and AER presence — for the endpoint and the bridge above it. Config
//      reads only; the device is healthy at this point, so reading the endpoint is safe.
//
//   2. ASPM KILL SWITCH (`noaspm` feature, `UNAOS_NOASPM=1`): clear LNKCTL[1:0] on BOTH ends
//      (endpoint first, then the root port — the disable order the PCIe spec asks for), print
//      one `[pcih] aspm cleared ...` line. Read-modify-write of LNKCTL[1:0] ONLY. Default OFF
//      => the clear is not linked and boots are unchanged.
//
//   3. WEDGE-TIME ROOT-PORT SAMPLER (`rp_at_wedge`, called from `wm::wcser_overdue_probe`'s
//      tripwire): reads ONLY root-port registers — LNKSTA, DEVSTA, secondary status, plus AER
//      status if present. NEVER the endpoint: a hung endpoint would capture the prober core on
//      a non-posted config read, and this probe runs on the input-service core — the last
//      surviving witness of a wedge. The root port's config space completes from the root
//      complex regardless of the endpoint's state. Everything the sampler needs (bdf, PCIe cap
//      offset, ECAM page, AER offset) is cached into statics at kepler init, so the wedge path
//      is a handful of volatile reads + one println — no enumeration, no capability walk. If
//      kepler never initialized (or no bridge above it was found), `PCIH_READY` stays false
//      and the sampler prints nothing.
//
// ECAM: the legacy CF8/CFC mechanism only reaches the first 256 bytes of config space; AER is
// an EXTENDED capability (offset >= 0x100), reachable only through the memory-mapped ECAM
// window ACPI's MCFG table describes. `census` parses MCFG (via the acpi module's table walk),
// maps the two 4 KiB function pages, and cross-checks the ECAM vendor/device dword against the
// CF8 read before trusting it. No MCFG (QEMU pc), or a mismatch => aer=n and the sampler falls
// back to CF8/CFC port reads for LNKSTA/DEVSTA/secondary status (still root-port-only, still
// completed by the root complex).
//
// x86_64-only in effect; the aarch64 shims below keep an `UNAOS_KEPLER=1` aarch64 type-check
// green without emitting a byte (kepler::init aborts before its call site matters there).

#![allow(dead_code)]

#[cfg(target_arch = "x86_64")]
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// True once `census` has cached a root port below. The sampler's only gate.
#[cfg(target_arch = "x86_64")]
static PCIH_READY: AtomicBool = AtomicBool::new(false);
/// Root-port bdf, packed `bus << 8 | slot << 3 | func`.
#[cfg(target_arch = "x86_64")]
static RP_BDF: AtomicU32 = AtomicU32::new(0);
/// Root port's PCIe capability offset (legacy 256-byte config region).
#[cfg(target_arch = "x86_64")]
static RP_PCIE_CAP: AtomicU32 = AtomicU32::new(0);
/// Identity-mapped ECAM byte address of the root port's 4 KiB config page (0 = no ECAM).
#[cfg(target_arch = "x86_64")]
static RP_ECAM: AtomicU64 = AtomicU64::new(0);
/// AER extended-capability offset within the root port's config page (0 = absent).
#[cfg(target_arch = "x86_64")]
static RP_AER: AtomicU32 = AtomicU32::new(0);

/// One volatile 32-bit read from a mapped ECAM config page. `off` must be dword-aligned.
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn ecam_read32(page: u64, off: u16) -> u32 {
    core::ptr::read_volatile((page + (off as u64 & !0x3)) as *const u32)
}

/// Walk the legacy capability list for `want` (e.g. 0x10 = PCIe). 0 when absent.
#[cfg(target_arch = "x86_64")]
fn find_cap(bus: u8, slot: u8, func: u8, want: u8) -> u8 {
    let status = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x06) };
    if status & (1 << 4) == 0 {
        return 0;
    }
    let mut ptr = (unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x34) } & 0xFC) as u8;
    let mut hops = 0;
    while ptr != 0 && hops < 48 {
        let hdr = unsafe { crate::arch::pci::read_config_16(bus, slot, func, ptr) };
        if (hdr & 0xFF) as u8 == want {
            return ptr;
        }
        ptr = ((hdr >> 8) & 0xFC) as u8;
        hops += 1;
    }
    0
}

/// Walk the EXTENDED capability list (ECAM only) for `want` (0x0001 = AER). 0 when absent.
#[cfg(target_arch = "x86_64")]
fn find_ext_cap(page: u64, want: u16) -> u16 {
    let mut off = 0x100u16;
    for _ in 0..64 {
        let hdr = unsafe { ecam_read32(page, off) };
        if hdr == 0 || hdr == 0xFFFF_FFFF {
            return 0;
        }
        if (hdr & 0xFFFF) as u16 == want {
            return off;
        }
        let next = ((hdr >> 20) & 0xFFC) as u16;
        if next == 0 || next < 0x100 {
            return 0;
        }
        off = next;
    }
    0
}

/// ECAM page address for one function, from ACPI MCFG (segment 0 only — this machine has one
/// segment). 0 when there is no MCFG, the bus is outside every entry, or ACPI never ran.
/// The page is NOT yet mapped or verified here; `census` does both.
#[cfg(target_arch = "x86_64")]
fn mcfg_page_for(bus: u8, slot: u8, func: u8) -> u64 {
    let rsdp = crate::arch::acpi::rsdp_addr();
    let (sdt, esz) = match crate::arch::acpi::root_sdt(rsdp) {
        Some(x) => x,
        None => return 0,
    };
    let mcfg = match unsafe { crate::arch::acpi::find_table(sdt, esz, b"MCFG") } {
        Some(a) => a,
        None => return 0,
    };
    let len = unsafe { crate::arch::acpi::table_len(mcfg) };
    // MCFG body: 36-byte SDT header + 8 reserved bytes, then 16-byte entries:
    // base(u64) segment(u16) start_bus(u8) end_bus(u8) reserved(u32).
    let mut off = 44usize;
    while off + 16 <= len {
        let base = unsafe { ((mcfg as usize + off) as *const u64).read_unaligned() };
        let seg = unsafe { ((mcfg as usize + off + 8) as *const u16).read_unaligned() };
        let sb = unsafe { ((mcfg as usize + off + 10) as *const u8).read_unaligned() };
        let eb = unsafe { ((mcfg as usize + off + 11) as *const u8).read_unaligned() };
        if seg == 0 && sb <= bus && bus <= eb {
            return base
                + ((bus as u64) << 20)
                + ((slot as u64) << 15)
                + ((func as u64) << 12);
        }
        off += 16;
    }
    0
}

/// Map + verify one function's ECAM page: the vendor/device dword read through ECAM must match
/// the CF8/CFC read of the same function, or the window is not trusted (0).
#[cfg(target_arch = "x86_64")]
fn ecam_page_verified(bus: u8, slot: u8, func: u8) -> u64 {
    let page = mcfg_page_for(bus, slot, func);
    if page == 0 {
        return 0;
    }
    crate::arch::memory::map_mmio_window(page, 4096);
    let via_ecam = unsafe { ecam_read32(page, 0x00) };
    let via_cf8 = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x00) };
    if via_ecam != via_cf8 {
        serial_println!(
            "[pcih] ecam-mismatch {}:{}.{} ecam={:08x} cf8={:08x} — ecam distrusted",
            bus, slot, func, via_ecam, via_cf8
        );
        return 0;
    }
    page
}

/// Decode LNKCTL[1:0] for the census line.
#[cfg(target_arch = "x86_64")]
fn aspm_str(lnkctl: u16) -> &'static str {
    match lnkctl & 0x3 {
        0 => "off",
        1 => "L0s",
        2 => "L1",
        _ => "L0sL1",
    }
}

/// One census line: PCIe capability registers + AER verdict for one function.
#[cfg(target_arch = "x86_64")]
fn census_line(tag: &str, bus: u8, slot: u8, func: u8, cap: u8, aer: bool) {
    if cap == 0 {
        serial_println!("[pcih] {} bdf={}:{}.{} no-pcie-cap", tag, bus, slot, func);
        return;
    }
    let (lnkcap, lnkctl, lnksta, devctl, devsta) = unsafe {
        (
            crate::arch::pci::read_config_32(bus, slot, func, cap + 0x0C),
            crate::arch::pci::read_config_16(bus, slot, func, cap + 0x10),
            crate::arch::pci::read_config_16(bus, slot, func, cap + 0x12),
            crate::arch::pci::read_config_16(bus, slot, func, cap + 0x08),
            crate::arch::pci::read_config_16(bus, slot, func, cap + 0x0A),
        )
    };
    serial_println!(
        "[pcih] {} bdf={}:{}.{} lnkcap={:08x} lnkctl={:04x} lnksta={:04x} devctl={:04x} devsta={:04x} aspm_en={} aer={}",
        tag, bus, slot, func, lnkcap, lnkctl, lnksta, devctl, devsta,
        aspm_str(lnkctl), if aer { "y" } else { "n" }
    );
}

/// BOOT-TIME LINK CENSUS + sampler arming. Called once from `kepler::init` with the endpoint's
/// bdf, right after bus-master enable — the same window every other config read of the device
/// already rides. Config reads only, except the `noaspm`-gated LNKCTL[1:0] clear.
#[cfg(target_arch = "x86_64")]
pub fn census(ep_bus: u8, ep_slot: u8, ep_func: u8) {
    // ── Endpoint ───────────────────────────────────────────────────────────
    let ep_cap = find_cap(ep_bus, ep_slot, ep_func, 0x10);
    let ep_ecam = ecam_page_verified(ep_bus, ep_slot, ep_func);
    let ep_aer = if ep_ecam != 0 { find_ext_cap(ep_ecam, 0x0001) } else { 0 };
    census_line("ep", ep_bus, ep_slot, ep_func, ep_cap, ep_aer != 0);

    // ── Root port: the type-1 bridge whose SECONDARY bus is the endpoint's bus ─
    // The direct parent of bus N is, by construction, the bridge decoding N as its secondary;
    // on this machine (GK107 at 1:0.0 below the Ivy Bridge CPU root port) that parent IS the
    // root port. Parents live on a lower bus number, so the walk is bounded by ep_bus.
    let mut rp: Option<(u8, u8, u8)> = None;
    'walk: for bus in 0..ep_bus {
        for slot in 0u8..32 {
            for func in 0u8..8 {
                let vendor = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x00) };
                if vendor == 0xFFFF {
                    if func == 0 { break; } else { continue; }
                }
                let hdr = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x0C) };
                if ((hdr >> 16) & 0x7F) as u8 != 0x01 {
                    continue; // not a PCI-PCI bridge header
                }
                let buses = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x18) };
                if ((buses >> 8) & 0xFF) as u8 == ep_bus {
                    rp = Some((bus, slot, func));
                    break 'walk;
                }
            }
        }
    }

    let (rb, rs, rf) = match rp {
        Some(bdf) => bdf,
        None => {
            // Endpoint sits on the root bus itself (or the walk found nothing): no root port
            // to sample. READY stays false — the wedge sampler prints nothing, by design.
            serial_println!("[pcih] rp none (no bridge with secondary bus {})", ep_bus);
            return;
        }
    };

    let rp_cap = find_cap(rb, rs, rf, 0x10);
    let rp_ecam = ecam_page_verified(rb, rs, rf);
    let rp_aer = if rp_ecam != 0 { find_ext_cap(rp_ecam, 0x0001) } else { 0 };
    census_line("rp", rb, rs, rf, rp_cap, rp_aer != 0);

    // ── ASPM kill switch (feature-gated; default off = not linked) ─────────
    // Disable order per the spec (and Linux's aspm.c): downstream component FIRST, then the
    // upstream port. RMW confined to LNKCTL[1:0]; every other bit is carried unchanged.
    #[cfg(feature = "noaspm")]
    {
        if ep_cap != 0 && rp_cap != 0 {
            let ep_old = unsafe { crate::arch::pci::read_config_16(ep_bus, ep_slot, ep_func, ep_cap + 0x10) };
            let ep_new = ep_old & !0x3;
            unsafe { crate::arch::pci::write_config_16(ep_bus, ep_slot, ep_func, ep_cap + 0x10, ep_new) };
            let rp_old = unsafe { crate::arch::pci::read_config_16(rb, rs, rf, rp_cap + 0x10) };
            let rp_new = rp_old & !0x3;
            unsafe { crate::arch::pci::write_config_16(rb, rs, rf, rp_cap + 0x10, rp_new) };
            serial_println!(
                "[pcih] aspm cleared rp {:04x}->{:04x} ep {:04x}->{:04x}",
                rp_old, rp_new, ep_old, ep_new
            );
        } else {
            serial_println!("[pcih] aspm clear skipped — pcie cap missing (ep={:02x} rp={:02x})", ep_cap, rp_cap);
        }
    }

    if rp_cap == 0 {
        // Without the PCIe capability offset the sampler has nothing safe to decode.
        return;
    }

    // ── Arm the wedge sampler ──────────────────────────────────────────────
    RP_BDF.store(((rb as u32) << 8) | ((rs as u32) << 3) | rf as u32, Ordering::Relaxed);
    RP_PCIE_CAP.store(rp_cap as u32, Ordering::Relaxed);
    RP_ECAM.store(rp_ecam, Ordering::Relaxed);
    RP_AER.store(rp_aer as u32, Ordering::Relaxed);
    // Release pairs with the sampler's Acquire: a reader that sees READY sees the cache.
    PCIH_READY.store(true, Ordering::Release);
}

/// WEDGE-TIME ROOT-PORT SAMPLER. Called from `wm::wcser_overdue_probe` on the input-service
/// core when the tripwire prints. ROOT PORT ONLY — never touches the endpoint, whose hung host
/// interface is the very thing under investigation; a non-posted config read to it could
/// capture this core too. Root-port config reads complete from the root complex. ECAM when the
/// census verified it (plain volatile loads), CF8/CFC otherwise. Prints nothing when kepler
/// never armed the cache.
#[cfg(target_arch = "x86_64")]
pub fn rp_at_wedge() {
    if !PCIH_READY.load(Ordering::Acquire) {
        return;
    }
    let bdf = RP_BDF.load(Ordering::Relaxed);
    let (b, s, f) = ((bdf >> 8) as u8, ((bdf >> 3) & 0x1F) as u8, (bdf & 0x7) as u8);
    let cap = RP_PCIE_CAP.load(Ordering::Relaxed) as u8;
    let ecam = RP_ECAM.load(Ordering::Relaxed);
    let (devsta, lnksta, secsta) = if ecam != 0 {
        unsafe {
            (
                (ecam_read32(ecam, cap as u16 + 0x08) >> 16) as u16, // DEVCTL|DEVSTA dword
                (ecam_read32(ecam, cap as u16 + 0x10) >> 16) as u16, // LNKCTL|LNKSTA dword
                (ecam_read32(ecam, 0x1C) >> 16) as u16,              // I/O limit|SECSTA dword
            )
        }
    } else {
        unsafe {
            (
                crate::arch::pci::read_config_16(b, s, f, cap + 0x0A),
                crate::arch::pci::read_config_16(b, s, f, cap + 0x12),
                crate::arch::pci::read_config_16(b, s, f, 0x1E),
            )
        }
    };
    let aer = RP_AER.load(Ordering::Relaxed) as u16;
    if aer != 0 && ecam != 0 {
        let (unc, cor) = unsafe { (ecam_read32(ecam, aer + 0x04), ecam_read32(ecam, aer + 0x10)) };
        serial_println!(
            "[pcih] rp-at-wedge lnksta={:04x} devsta={:04x} secsta={:04x} aer_unc={:08x} aer_cor={:08x}",
            lnksta, devsta, secsta, unc, cor
        );
    } else {
        serial_println!(
            "[pcih] rp-at-wedge lnksta={:04x} devsta={:04x} secsta={:04x} aer=n",
            lnksta, devsta, secsta
        );
    }
}

// ── aarch64 shims — kepler::init type-checks on aarch64 (it aborts at runtime before the GPU
// legs), so these keep an UNAOS_KEPLER=1 aarch64 compile green while emitting nothing. ──────
#[cfg(not(target_arch = "x86_64"))]
pub fn census(_ep_bus: u8, _ep_slot: u8, _ep_func: u8) {}
#[cfg(not(target_arch = "x86_64"))]
pub fn rp_at_wedge() {}
