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
// CF8 read before trusting it. No MCFG (QEMU pc), or a mismatch => the sampler DOES NOT ARM.
// It has no CF8/CFC fallback: CF8 is an unlocked address/data port pair, this sampler runs on a
// non-BSP core at ~1 kHz, and a store stolen by that race could land in the root port's LNKCTL
// and disable the link. See PCIH-NOCF8 in `census` for the full reasoning and the evidence that
// the bench machine was already on the ECAM path.
//
// Bounds: every offset this module forms comes from a capability list the DEVICE wrote, and
// both lists can name a base whose body would leave the region it was found in. See the
// PCIH-BOUNDS block below — the two `const fn` predicates are the whole guarantee, and they
// refuse rather than clamp.
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
/// Identity-mapped ECAM byte address of the ENDPOINT's 4 KiB config page (0 = none/unverified).
///
/// PCIH-OWN: this exists so the endpoint window `census` maps is OWNED rather than orphaned.
/// x86's `arch::memory::map_mmio_window` has no inverse in this tree — there is no
/// `unmap_mmio_window`, and adding page-table teardown is not this module's to invent — so a
/// mapping made here lives until reboot no matter what. The choice is therefore not "map or
/// unmap" but "map and record" versus "map and forget": the second is the leak. One 4 KiB UC
/// window, created once per boot, reachable through [`ep_ecam_page`], and named as an input of
/// the recovery design (`docs/dev/OS/08_VIDEO/PCIE-RP-RECOVERY.md`) — the only path that reaches
/// the endpoint's EXTENDED config space, which is where its AER status lives.
#[cfg(target_arch = "x86_64")]
static EP_ECAM: AtomicU64 = AtomicU64::new(0);

// ── PCIH-BOUNDS ─────────────────────────────────────────────────────────────────────────────
// Every offset this module forms is `base + delta` where `base` came from a capability list the
// DEVICE wrote. Both lists can legally hand back a base high enough that `base + delta` leaves
// the region the base was found in — and in this module neither addition used to be checked:
//
//   * legacy list: `find_cap` masks its pointer with 0xFC, so `cap` can be 0xFC. `cap + 0x12`
//     (LNKSTA) is 0x10E, which does not fit in the `u8` the config accessors take. The kernel
//     workspace declares no `[profile]` and `arroyo` builds `--release`, so `overflow-checks`
//     is off and the addition WRAPS: 0xFC + 0x12 == 0x0E. The reads would then sample the
//     BIST/header-type/latency dword, and the `noaspm` leg's `cap + 0x10` would WRITE
//     (0xFC + 0x10 == 0x0C) into Cache Line Size / Latency Timer.
//   * extended list: `find_ext_cap`'s next-pointer field is 12 bits, so `off` can be 0xFFC —
//     the last dword of the page. `aer + 0x10` is then 0x100C, one dword PAST the single 4 KiB
//     `map_mmio_window(page, 4096)` window, i.e. a load off the end of the mapping.
//
// Both are near-misses on real silicon (a real capability body has to fit somewhere), so the
// point of the two predicates below is not to fix an observed failure — it is to make the
// property PROVABLE instead of inferred from what firmware happens to emit. They are `const fn`
// on purpose: pure arithmetic, no config I/O, and delimited by the two marker lines below so a
// host-run proof can `sed` them out of THIS file verbatim and exercise them — the proof is then
// about the shipped code rather than about a copy of it that may have drifted.
//
// The refusal is a REFUSAL, never a clamp. A capability whose body does not fit is unreadable
// through this window; a truncated read of it would be a lie printed in the same format as a
// true one, and this module's whole job is producing evidence a metal boot can be trusted on.
// ── PCIH-BOUNDS-BEGIN ───────────────────────────────────────────────────────────────────────
/// The legacy, CF8/CFC-reachable config region: 256 bytes.
pub const LEGACY_CFG_LEN: u16 = 0x100;
/// The first byte a capability pointer may legally name. Below this is the standard header
/// (vendor/device/command/status/BARs), which is not a capability and must never be walked as
/// one — a malformed pointer of, say, 0x04 would otherwise decode the Command register as a
/// capability header and hand back a `cap` that indexes into the standard header.
pub const CAP_PTR_FLOOR: u8 = 0x40;
/// One ECAM function page — exactly what [`ecam_page_verified`] maps and all that is mapped.
pub const ECAM_PAGE_LEN: u32 = 0x1000;
/// Bytes this module reads inside the legacy PCIe capability, measured from its header: LNKSTA
/// sits at `+0x12`, so `+0x13` is the last byte touched and `0x14` is the span that must fit.
pub const PCIE_CAP_SPAN: u16 = 0x14;
/// Bytes this module reads inside an extended capability, measured from its header: the AER
/// correctable-status dword sits at `+0x10`, so `+0x13` is the last byte touched.
pub const EXT_CAP_SPAN: u16 = 0x14;

/// Does a capability body of `span` bytes based at `cap` lie wholly inside the legacy 256-byte
/// config region — i.e. can every `cap + k` for `k < span` be formed in `u8` without wrapping?
#[inline]
pub const fn cap_fits(cap: u8, span: u16) -> bool {
    cap >= CAP_PTR_FLOOR && (cap as u16) + span <= LEGACY_CFG_LEN
}

/// Does a capability body of `span` bytes based at `off` lie wholly inside the ONE mapped 4 KiB
/// ECAM page? Extended capabilities start at 0x100, so anything below that is malformed.
#[inline]
pub const fn ecam_fits(off: u16, span: u16) -> bool {
    off >= 0x100 && (off as u32) + (span as u32) <= ECAM_PAGE_LEN
}
// ── PCIH-BOUNDS-END ─────────────────────────────────────────────────────────────────────────

/// One volatile 32-bit read from a mapped ECAM config page. `off` must be dword-aligned.
///
/// PRECONDITION: `ecam_fits(off, 4)` — the dword must lie inside the single 4 KiB window
/// [`ecam_page_verified`] mapped. Every caller derives `off` from a value [`find_ext_cap`] has
/// already bounded, or from a fixed offset below 0x100 in the standard header; the
/// `debug_assert` is here so a future caller that breaks the invariant trips in a debug build
/// rather than reading off the end of the mapping in the release build that actually ships.
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn ecam_read32(page: u64, off: u16) -> u32 {
    debug_assert!(
        (off as u32) + 4 <= ECAM_PAGE_LEN,
        "pcih: ecam_read32 off past the mapped page"
    );
    core::ptr::read_volatile((page + (off as u64 & !0x3)) as *const u32)
}

/// Walk the legacy capability list for `want` (e.g. 0x10 = PCIe) and return its offset only if a
/// body of `span` bytes based there fits inside the legacy 256-byte config region. 0 otherwise —
/// absent, malformed, or present-but-unreadable are all "do not use it".
///
/// `span` is not decoration: it is what makes `cap + k` provably `u8`-safe at every call site.
/// The caller passes the largest offset it will ever form (+1); with [`PCIE_CAP_SPAN`] that caps
/// `cap` at 0xEC, so `cap + 0x12` is at most 0xFE and the release-build wrap described in the
/// PCIH-BOUNDS block cannot be reached.
#[cfg(target_arch = "x86_64")]
fn find_cap(bus: u8, slot: u8, func: u8, want: u8, span: u16) -> u8 {
    let status = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x06) };
    if status & (1 << 4) == 0 {
        return 0;
    }
    let mut ptr = (unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x34) } & 0xFC) as u8;
    let mut hops = 0;
    // `ptr` is masked to 0xFC, so the two-byte header read at `ptr` is always in-region; the
    // floor is what keeps the walk from decoding the standard header as a capability.
    while ptr >= CAP_PTR_FLOOR && hops < 48 {
        let hdr = unsafe { crate::arch::pci::read_config_16(bus, slot, func, ptr) };
        if (hdr & 0xFF) as u8 == want {
            if !cap_fits(ptr, span) {
                serial_println!(
                    "[pcih] cap {:02x} at {}:{}.{} off={:02x} — {} body bytes leave the 256-byte \
                     config region, refused",
                    want, bus, slot, func, ptr, span
                );
                return 0;
            }
            return ptr;
        }
        ptr = ((hdr >> 8) & 0xFC) as u8;
        hops += 1;
    }
    0
}

/// Walk the EXTENDED capability list (ECAM only) for `want` (0x0001 = AER) and return its offset
/// only if a body of `span` bytes based there fits inside the ONE 4 KiB page `census` mapped.
/// 0 otherwise.
///
/// The next-pointer field is 12 bits, so this walk can legally arrive at 0xFFC, where the header
/// is the last dword of the page and every register of the capability body is outside the
/// mapping. Refusing there is the difference between "we know this port has no readable AER" and
/// a load one dword past a `map_mmio_window(page, 4096)` window.
#[cfg(target_arch = "x86_64")]
fn find_ext_cap(page: u64, want: u16, span: u16) -> u16 {
    let mut off = 0x100u16;
    for _ in 0..64 {
        // The header dword itself must be in-page before it is read. `off` starts at 0x100 and
        // every later value comes from the 12-bit next field masked to 0xFFC, so this holds by
        // construction today; the test is what keeps it holding if either bound ever moves.
        if !ecam_fits(off, 4) {
            return 0;
        }
        let hdr = unsafe { ecam_read32(page, off) };
        if hdr == 0 || hdr == 0xFFFF_FFFF {
            return 0;
        }
        if (hdr & 0xFFFF) as u16 == want {
            if !ecam_fits(off, span) {
                serial_println!(
                    "[pcih] ext-cap {:04x} at off={:03x} — {} body bytes leave the mapped 4 KiB \
                     page, refused",
                    want, off, span
                );
                return 0;
            }
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
///
/// PRECONDITION: `cap == 0` or `cap_fits(cap, PCIE_CAP_SPAN)` — i.e. `cap` came from
/// [`find_cap`] with [`PCIE_CAP_SPAN`]. That is what makes the five `cap + k` additions below
/// `u8`-safe in a release build with `overflow-checks` off.
#[cfg(target_arch = "x86_64")]
fn census_line(tag: &str, bus: u8, slot: u8, func: u8, cap: u8, aer: bool) {
    if cap == 0 {
        serial_println!("[pcih] {} bdf={}:{}.{} no-pcie-cap", tag, bus, slot, func);
        return;
    }
    debug_assert!(cap_fits(cap, PCIE_CAP_SPAN), "pcih: census_line cap out of region");
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
    let ep_cap = find_cap(ep_bus, ep_slot, ep_func, 0x10, PCIE_CAP_SPAN);
    let ep_ecam = ecam_page_verified(ep_bus, ep_slot, ep_func);
    // PCIH-OWN: record the window before anything else can return early. See `EP_ECAM`.
    EP_ECAM.store(ep_ecam, Ordering::Relaxed);
    let ep_aer = if ep_ecam != 0 { find_ext_cap(ep_ecam, 0x0001, EXT_CAP_SPAN) } else { 0 };
    census_line("ep", ep_bus, ep_slot, ep_func, ep_cap, ep_aer != 0);

    // ── Root port: the type-1 bridge whose SECONDARY bus is the endpoint's bus ─
    // The direct parent of bus N is, by construction, the bridge decoding N as its secondary;
    // on this machine (GK107 at 1:0.0 below the Ivy Bridge CPU root port) that parent IS the
    // root port. Parents live on a lower bus number, so the walk is bounded by ep_bus.
    //
    // MULTIFUNCTION DISCIPLINE (matches the tree's other enumerator, `drivers/ehci/mod.rs`
    // ~14743): probe function 0 first and walk 1..=7 only when its header type has bit 7 set.
    // Probing the higher functions of a single-function device is out of spec and its result is
    // undefined — the common silicon behaviour is to ALIAS function 0 across all eight, which
    // would make this walk "find" the same bridge up to eight times and read seven phantom
    // devices per slot on every boot. Reads only, so the old shape was a correctness and
    // tidiness defect rather than a hazard; matching the precedent is the point.
    let mut rp: Option<(u8, u8, u8)> = None;
    'walk: for bus in 0..ep_bus {
        for slot in 0u8..32 {
            if unsafe { crate::arch::pci::read_config_16(bus, slot, 0, 0x00) } == 0xFFFF {
                continue; // no function 0 => no device in this slot, per spec
            }
            let hdr0 = unsafe { crate::arch::pci::read_config_32(bus, slot, 0, 0x0C) };
            let max_func = if (hdr0 >> 16) & 0x80 != 0 { 7u8 } else { 0u8 };
            for func in 0..=max_func {
                // Function 0's header dword is already in hand; only the higher functions cost
                // a probe, and only when the device declared itself multifunction.
                let hdr = if func == 0 {
                    hdr0
                } else {
                    if unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x00) } == 0xFFFF
                    {
                        continue;
                    }
                    unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x0C) }
                };
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

    let rp_cap = find_cap(rb, rs, rf, 0x10, PCIE_CAP_SPAN);
    let rp_ecam = ecam_page_verified(rb, rs, rf);
    let rp_aer = if rp_ecam != 0 { find_ext_cap(rp_ecam, 0x0001, EXT_CAP_SPAN) } else { 0 };
    census_line("rp", rb, rs, rf, rp_cap, rp_aer != 0);

    // ── PCIH-SECBASE: the boot value of the two bridge registers the wedge story turns on ───
    //
    // `rp-at-wedge` has read `secsta=2000` on every wedged boot (8, 9, 11) and that reading has
    // been carrying weight as evidence — bit 13 is Received Master Abort. But secondary status
    // is a WRITE-1-TO-CLEAR LATCH THAT IS NEVER CLEARED, and the ordinary, expected way for bit
    // 13 to get set is bus enumeration: every config probe of an absent device below this bridge
    // master-aborts and latches it. This kernel walks buses 0..=255 in more than one place. So
    // `secsta=2000` at 118 s is equally consistent with "the endpoint stopped answering during
    // the wedge" and with "something probed an empty slot on bus 1 during boot, ninety seconds
    // before anything went wrong" — and the sampler cannot tell those apart, because it has
    // nothing to compare against.
    //
    // One read at boot bounds the question on the very next sitting: if this line already says
    // `secsta=2000`, the wedge-time reading is boot residue and carries no information about the
    // wedge; if it says `secsta=0000`, the latch was set between here and the tripwire and the
    // reading means what it has been taken to mean. This is a READ, deliberately — clearing the
    // latch (W1C) is what would make every later sample a true delta, and that is the right
    // instrument, but it is a write to a shared bridge register and it belongs in a change that
    // is about the sampler rather than riding in on a bounds-hardening arc.
    //
    // BRIDGECTL (0x3E) is captured with it because bit 6 of that register IS the secondary bus
    // reset, and any recovery has to preserve the other bits (VGA enable, ISA enable, error
    // forwarding, and the bit 0/1 parity/SERR enables) across the pulse. Its boot value is the
    // only correct thing to restore to, and reading it at wedge time from a possibly-dying core
    // is not something to rely on. See `docs/dev/OS/08_VIDEO/PCIE-RP-RECOVERY.md`.
    //
    // Caveat this line cannot fix on its own: `census` runs inside `pci::init`, so enumeration
    // that happens LATER (the EHCI driver's own 0..=255 walk) can still set the latch after this
    // sample. A zero here therefore narrows the window rather than closing it.
    let (rp_secsta, rp_brctl) = unsafe {
        (
            crate::arch::pci::read_config_16(rb, rs, rf, 0x1E),
            crate::arch::pci::read_config_16(rb, rs, rf, 0x3E),
        )
    };
    serial_println!(
        "[pcih] rp-boot bdf={}:{}.{} secsta={:04x} bridgectl={:04x} (secsta is a since-boot W1C \
         latch — compare rp-at-wedge against THIS, not against zero)",
        rb, rs, rf, rp_secsta, rp_brctl
    );

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

    // ── PCIH-NOCF8: the sampler arms on a VERIFIED ECAM PAGE OR NOT AT ALL ─────────────────
    //
    // `rp_at_wedge` runs on the input-service core — a non-BSP core, on its ~1 kHz loop, at an
    // arbitrary instant. `arch::x86_64::pci`'s CF8/CFC accessors are an address-port write
    // followed by a data-port access with NO lock between them, so two cores interleaving there
    // do not merely misread: core A's CF8 write can land between core B's CF8 write and B's CFC
    // *write*, redirecting B's 32-bit store to whichever register A selected. The tree has live
    // config WRITERS (`ensure_bus_master` in the EHCI path; the `CFG_BAR0_WIN` sliding-window
    // stores in `bcma`/`wifi::bringup`), and the registers this sampler selects on the root port
    // are the worst possible targets for a stolen store: `cap + 0x10` is the LNKCTL|LNKSTA
    // dword, whose LNKCTL half carries Link Disable (bit 4) and Retrain Link (bit 5), and 0x1C
    // is the I/O-limit|secondary-status dword. A stolen write of an arbitrary BAR-window address
    // into LNKCTL can DISABLE THE LINK — the wedge diagnostic causing the catastrophe it exists
    // to investigate.
    //
    // The general fix is a tree-wide CF8 lock with interrupt masking around the address/data
    // pair, in `arch/x86_64/pci.rs`. That is its own arc: it is a change to every config access
    // in the kernel, it has to reason about config access from interrupt context, and a lock
    // taken by THIS module alone would be worse than none — it would advertise a mutual
    // exclusion the other side never joins. So this arc removes the hazard instead of
    // pretending to manage it: the sampler is ECAM-only, and ECAM is plain MMIO — one
    // independent load per register, no shared latch, cross-core safe by construction.
    //
    // Nothing is lost where it matters. On the bench (MacBookPro10,1) the census's `ep … aer=y`
    // line proves MCFG parsed and the endpoint's ECAM page verified, so the root port's did too
    // and the wedge sampler was already on the ECAM path in boots 8–11; its `aer=n` is the Ivy
    // Bridge port genuinely having no AER extended capability, not a missing window. On QEMU no
    // GK107 exists, `census` never runs, and `PCIH_READY` stays false. The only configuration
    // this refusal disarms is a GK107 on a machine with no usable MCFG — exactly the machine
    // where the CF8 path would have been unlocked cross-core port I/O.
    //
    // `census` itself keeps its CF8 reads: it runs once on the BSP inside `pci::init`, the same
    // sequential boot phase every other enumerator in this tree walks the bus in, with the APs
    // still idling in the scheduler. The novel exposure was the ~1 kHz runtime path, and that is
    // what this closes.
    if rp_ecam == 0 {
        serial_println!(
            "[pcih] rp {}:{}.{} — no verified ecam page; wedge sampler NOT armed (it will not \
             fall back to unlocked CF8/CFC from a non-BSP core)",
            rb, rs, rf
        );
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

/// The root port's bdf, as `census` resolved it. `None` until the sampler is armed. This and
/// [`ep_ecam_page`] are the recovery rung's inputs — see
/// `docs/dev/OS/08_VIDEO/PCIE-RP-RECOVERY.md`; a secondary-bus reset has to name the bridge
/// whose Bridge Control register it sets, and resolving that at wedge time would mean walking
/// the bus from a core that may be the last one running.
#[cfg(target_arch = "x86_64")]
pub fn rp_bdf() -> Option<(u8, u8, u8)> {
    if !PCIH_READY.load(Ordering::Acquire) {
        return None;
    }
    let bdf = RP_BDF.load(Ordering::Relaxed);
    Some(((bdf >> 8) as u8, ((bdf >> 3) & 0x1F) as u8, (bdf & 0x7) as u8))
}

/// The endpoint's verified ECAM page, or 0. See [`EP_ECAM`] — this is the owner-of-record for
/// the one 4 KiB window `census` maps for the endpoint, and the only path to the GK107's
/// extended config space (where its AER status lives) after a reset.
#[cfg(target_arch = "x86_64")]
pub fn ep_ecam_page() -> u64 {
    EP_ECAM.load(Ordering::Relaxed)
}

/// WEDGE-TIME ROOT-PORT SAMPLER. Called from `wm::wcser_overdue_probe` on the input-service
/// core when the tripwire prints. Two refusals define it:
///
///   * **ROOT PORT ONLY** — never the endpoint, whose hung host interface is the very thing
///     under investigation; a non-posted config read to it could capture this core too. The
///     root port's config space completes from the root complex regardless.
///   * **ECAM ONLY** — no CF8/CFC fallback. See the PCIH-NOCF8 note in `census`: the CF8
///     address/data pair is unlocked, this runs on a non-BSP core at ~1 kHz, and a stolen store
///     could land in the root port's LNKCTL and disable the link. `census` refuses to arm
///     without a verified ECAM page, so `ecam != 0` here is an invariant; the check is kept as
///     the invariant's local statement, not as a branch that expects to be taken.
///
/// Prints nothing when kepler never armed the cache.
#[cfg(target_arch = "x86_64")]
pub fn rp_at_wedge() {
    if !PCIH_READY.load(Ordering::Acquire) {
        return;
    }
    let cap = RP_PCIE_CAP.load(Ordering::Relaxed) as u16;
    let ecam = RP_ECAM.load(Ordering::Relaxed);
    if ecam == 0 {
        return; // invariant: census does not arm without one
    }
    let (devsta, lnksta, secsta) = unsafe {
        (
            (ecam_read32(ecam, cap + 0x08) >> 16) as u16, // DEVCTL|DEVSTA dword
            (ecam_read32(ecam, cap + 0x10) >> 16) as u16, // LNKCTL|LNKSTA dword
            (ecam_read32(ecam, 0x1C) >> 16) as u16,       // I/O limit|SECSTA dword
        )
    };
    let aer = RP_AER.load(Ordering::Relaxed) as u16;
    if aer != 0 {
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
