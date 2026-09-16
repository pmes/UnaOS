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
    EP_ECAM.store(ep_ecam, Ordering::Relaxed); w5_cache_bar0(ep_bus, ep_slot, ep_func); // W5I1 — BAR0 base cached here, before any early return, from config 0x10 (kepler::init maps it only after census returns); same-line fold so no line below moves
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
    // BAR1WEDGE (default OFF, `UNAOS_BAR1WEDGE=1`): the first-stall instrument's boot half —
    // baseline cache, the completion-timeout decode, and the arm-time W1C clear of the three
    // sticky latches. See the BAR1WEDGE block at the file tail. Nothing above this line moves.
    #[cfg(feature = "bar1wedge")]
    bw_arm(rb, rs, rf, rp_cap, rp_ecam, rp_aer);

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
    // BAR1WEDGE (default OFF): the first-stall block, on the same crossing and the same refusals
    // (root port only, ECAM only, READ only). See the file tail.
    #[cfg(feature = "bar1wedge")]
    bw_sample(ecam, cap, aer, lnksta, devsta, secsta);
}

// ── aarch64 shims — kepler::init type-checks on aarch64 (it aborts at runtime before the GPU
// legs), so these keep an UNAOS_KEPLER=1 aarch64 compile green while emitting nothing. ──────
#[cfg(not(target_arch = "x86_64"))]
pub fn census(_ep_bus: u8, _ep_slot: u8, _ep_func: u8) {}
#[cfg(not(target_arch = "x86_64"))]
pub fn rp_at_wedge() {}

// ── BAR1WEDGE — the FIRST-STALL register block (`UNAOS_BAR1WEDGE=1`, feature `bar1wedge`) ────────
//
// THE RUNG THIS IS, AND WHY IT IS THIS ONE. The shut-out register (`SHUTOUT-REGISTER.md` §6) names
// exactly one coded, never-flown experiment for ledger A1: P5, the BAR1 UC retype
// (`UNAOS_BAR1EXP=uc`, `arch/x86_64/memory.rs`). Its code exists; what does not exist is a way to
// SCORE a boot that carries it. A UC boot yields one bit — wedge or no wedge — and that bit is not
// a refutation, because the UC arm also runs the aperture ~6.8x slower: "no wedge under UC" is
// equally explained by "the paint burst never reached the rate that wedges". The falsifier the
// flight needs is a register block taken at the FIRST stall — not after the core is dead, and not
// as a value with nothing to compare it to.
//
// Three things were missing from the existing sampler, and all three are read-only root-port facts:
//
//   1. **LNKCTL is read and thrown away.** `rp_at_wedge` loads the LNKCTL|LNKSTA dword and keeps
//      only the top half. So the ASPM state IN FORCE AT THE WEDGE has never been on the wire — only
//      the state `census` printed ~100 s earlier — and neither has Link Disable or Retrain Link.
//      P3's verdict ("ASPM is shut out as a cure") rests on the boot-time line alone.
//   2. **The completion-timeout configuration is never read.** Device Control 2 [3:0] is the bound
//      within which a non-posted read to a silent endpoint returns all-ones instead of never
//      returning, and [4] can disable the mechanism outright. That number is the load-bearing
//      unknown of `PCIE-RP-RECOVERY.md` §3.2 (can the sacrificial prober survive?) and §8.2 (can the
//      seized core ever be freed?), and it is a boot-time constant nobody has ever printed.
//   3. **Every sticky latch is boot residue until something clears it.** §1.3 of that document says
//      so for secondary status and recommends the W1C clear "for the next arc"; §10's order of work
//      makes it rung 1. It is true of two more fields nobody had noticed: LNKSTA[15:14] (Link
//      Bandwidth Management Status, Link Autonomous Bandwidth Status) are RW1C, they are BOTH SET in
//      the `lnksta=d081` that three boots have quoted as evidence, and this kernel has never cleared
//      them either. So `d081` has been read as a live reading of a clean link when two of its bits
//      are since-boot latches.
//
// WHAT THIS RUNG DOES, therefore: cache the root port's boot values, print the completion-timeout
// decode once, CLEAR the three W1C latches at arm time so every later sample is a delta against a
// known zero, and print one complete `[pcih] wedge-sample` line per tripwire crossing carrying the
// missing fields plus the delta against that baseline — and naming, on the same line, which memory
// type the panel aperture carries this boot (`aperture=uc|wc`), so a BAR1UC flight's wire is
// self-labelling instead of depending on the operator's memory of the knob line.
//
// THE CLEAR IS AT ARM TIME AND NOWHERE ELSE. `census` runs on the BSP inside `pci::init`, the same
// sequential boot phase the `noaspm` leg already writes LNKCTL from, so a CF8 write here carries the
// exact hazard profile of a write this module already makes. The sampler keeps BOTH of its defining
// refusals — root port only, ECAM only — and gains a third: it never writes. A W1C from the ~1 kHz
// tripwire band would be a new store into a shared bridge register issued from the timer ISR while a
// core is wedged, to buy a per-crossing delta; that trade is refused here and named in
// `PCIE-RP-RECOVERY.md` as the rung above this one.
//
// RESIDUAL, STATED RATHER THAN ARGUED AWAY: `census` runs inside `pci::init`, and the EHCI driver's
// own `0..=255` bus walk happens LATER, so a master abort it provokes can re-latch secondary status
// after this clear. The clear narrows the window from "the whole boot" to "after `pci::init`"; it
// does not close it. Closing it needs a second clear once enumeration is complete — a second call
// site, in another file, and therefore not this change.
//
// Every register and every field printed below is decoded, with its spec section, in
// `docs/dev/OS/08_VIDEO/PCIE-RP-RECOVERY.md` §11. Specs: PCI Express Base Specification Revision
// 3.0 §7.8 (PCI Express Capability Structure) and §7.10 (Advanced Error Reporting Capability);
// PCI-to-PCI Bridge Architecture Specification Revision 1.2 §3.2.5.7 (Secondary Status).

/// Bytes of the PCIe capability this rung reads, measured from its header: Device Control 2 sits at
/// `+0x28` and is 16-bit, so `+0x29` is the last byte touched and `0x2A` is the span that must fit
/// inside the legacy 256-byte config region.
///
/// KEPT SEPARATE FROM [`PCIE_CAP_SPAN`] ON PURPOSE. Widening the span `find_cap` is called with
/// would make a root port whose PCIe capability sits above 0xD6 fail `cap_fits` — and `find_cap`
/// returning 0 costs the census line, the ASPM clear AND the wedge sampler, three instruments that
/// ride every Kepler boot unconditionally, to satisfy a rung that is default OFF. This span is
/// therefore checked at the point of use, and a capability too high to carry it loses the v2 fields
/// and nothing else (`v2=0` on the wire).
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
pub const PCIE_CAP_SPAN_V2: u16 = 0x2A;

/// Link Status RW1C bits (PCIe r3.0 §7.8.8): [14] Link Bandwidth Management Status, [15] Link
/// Autonomous Bandwidth Status. Everything else in the register is read-only.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
pub const LNKSTA_W1C: u16 = 0xC000;
/// Device Status RW1C bits (PCIe r3.0 §7.8.5): [0] Correctable Error Detected, [1] Non-Fatal Error
/// Detected, [2] Fatal Error Detected, [3] Unsupported Request Detected. [4] AUX Power Detected and
/// [5] Transactions Pending are read-only.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
pub const DEVSTA_W1C: u16 = 0x000F;
/// Secondary Status RW1C bits (PCI-to-PCI Bridge r1.2 §3.2.5.7): [8] Master Data Parity Error,
/// [11] Signaled Target Abort, [12] Received Target Abort, [13] Received Master Abort, [14] Received
/// System Error, [15] Detected Parity Error. [9:10] are the read-only DEVSEL timing field.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
pub const SECSTA_W1C: u16 = 0xF900;

/// Sentinel for "this boot could not read the PCIe capability v2 registers". Chosen because a real
/// 16-bit register read can never produce it, so the sampler never has to carry a second flag.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
const BW_NO_V2: u32 = 0xFFFF_FFFF;

/// Root-port LNKSTA immediately AFTER the arm-time W1C clear — the zero every later sample deltas
/// against. Same for the three siblings below.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_LNKSTA0: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_DEVSTA0: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_SECSTA0: AtomicU32 = AtomicU32::new(0);
/// Root-port LNKCTL at boot, for the at-wedge comparison the census line alone cannot make.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_LNKCTL0: AtomicU32 = AtomicU32::new(0);
/// Root-port Device Control 2 at boot, or [`BW_NO_V2`].
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_DEVCTL2: AtomicU32 = AtomicU32::new(BW_NO_V2);
/// Tripwire crossings this boot. `n=1` is the FIRST STALL — the sample this rung exists for.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_SAMPLES: AtomicU32 = AtomicU32::new(0);

/// The memory type the panel aperture carries on THIS build, named on every BAR1WEDGE line so a
/// capture says which arm of the P5 experiment flew without anyone consulting the knob line.
/// `bar1exp-uc` is the one knob that retypes it (`arch/x86_64/memory.rs`, `set_framebuffer_wc`).
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
fn bw_aperture() -> &'static str {
    if cfg!(feature = "bar1exp-uc") { "uc" } else { "wc" }
}

/// Decode the Completion Timeout Value field, Device Control 2 [3:0] (PCIe r3.0 §7.8.16, Table 7-20).
/// The ranges are the spec's letter classes; a device need only implement the default and whichever
/// classes Device Capabilities 2 [3:0] advertises.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
fn bw_cto_str(devctl2: u16) -> &'static str {
    match devctl2 & 0xF {
        0x0 => "50us-50ms(default)",
        0x1 => "50us-100us(A)",
        0x2 => "1ms-10ms(A)",
        0x5 => "16ms-55ms(B)",
        0x6 => "65ms-210ms(B)",
        0x9 => "260ms-900ms(C)",
        0xA => "1s-3.5s(C)",
        0xD => "4s-13s(D)",
        0xE => "17s-64s(D)",
        _ => "reserved",
    }
}

/// Decode Link Status [3:0], Current Link Speed (PCIe r3.0 §7.8.8).
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
fn bw_speed_str(lnksta: u16) -> &'static str {
    match lnksta & 0xF {
        0x1 => "2.5GT/s",
        0x2 => "5.0GT/s",
        0x3 => "8.0GT/s",
        _ => "unknown",
    }
}

/// BAR1WEDGE, boot half. Called from [`census`] with the root port's bdf, its PCIe capability offset
/// (already bounded by `cap_fits(cap, PCIE_CAP_SPAN)`), its verified ECAM page and its AER offset.
///
/// CF8 reads and three CF8 W1C writes, on the BSP, inside `pci::init` — the same phase and the same
/// accessors the `noaspm` leg uses. Each write carries ONLY the RW1C bits that were read as SET, so
/// no bit this rung did not observe is ever written, and a register with nothing latched is not
/// written at all.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
fn bw_arm(rb: u8, rs: u8, rf: u8, cap: u8, ecam: u64, aer: u16) {
    debug_assert!(cap_fits(cap, PCIE_CAP_SPAN), "pcih: bw_arm cap out of region");
    // PCI Express Capabilities Register (§7.8.2) [3:0] = Capability Version. Device Capabilities 2 /
    // Device Control 2 exist only from version 2; on a version-1 capability `cap + 0x24` is whatever
    // capability the device put there next, and reading it as DEVCAP2 would print a fiction.
    let pciecap = unsafe { crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x02) };
    let capver = (pciecap & 0xF) as u8;
    let v2 = capver >= 2 && cap_fits(cap, PCIE_CAP_SPAN_V2);
    let (devcap2, devctl2) = if v2 {
        unsafe {
            (
                crate::arch::pci::read_config_32(rb, rs, rf, cap + 0x24),
                crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x28),
            )
        }
    } else {
        (0u32, 0u16)
    };
    let (lnkctl, lnksta, devsta, secsta) = unsafe {
        (
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x10),
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x12),
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x0A),
            crate::arch::pci::read_config_16(rb, rs, rf, 0x1E),
        )
    };

    serial_println!(
        ":: BAR1WEDGE: rung=first-stall armed=UNAOS_BAR1WEDGE aperture={} rp={}:{}.{} capver={} \
         v2={} baseline=lnksta={:04x}({} x{}) lnkctl={:04x}(aspm={}) devsta={:04x} secsta={:04x} \
         devctl2={:04x} aer={} ::",
        bw_aperture(),
        rb,
        rs,
        rf,
        capver,
        v2 as u8,
        lnksta,
        bw_speed_str(lnksta),
        (lnksta >> 4) & 0x3F,
        lnkctl,
        aspm_str(lnkctl),
        devsta,
        secsta,
        devctl2,
        if aer != 0 { "y" } else { "n" }
    );

    // The completion-timeout facts, printed once because they are boot-time constants. This is the
    // number `PCIE-RP-RECOVERY.md` §3.2 needs to say whether a sacrificial endpoint probe returns,
    // and §8.2 needs to say whether a core stalled on a non-posted read can ever be released.
    if v2 {
        serial_println!(
            "[pcih] bar1wedge cto rp devcap2={:08x} ranges={:x} cto_dis_sup={} devctl2={:04x} \
             value={} dis={} — the bound a non-posted read to a silent endpoint completes within",
            devcap2,
            devcap2 & 0xF,
            (devcap2 >> 4) & 1,
            devctl2,
            bw_cto_str(devctl2),
            (devctl2 >> 4) & 1
        );
    } else {
        serial_println!(
            "[pcih] bar1wedge cto rp UNREADABLE capver={} cap={:02x} — PCIe capability version < 2, \
             or a {}-byte body would leave the 256-byte config region; no completion-timeout fact \
             this boot",
            capver, cap, PCIE_CAP_SPAN_V2
        );
    }

    // The root port's AER pair at boot, when it has AER at all. On the Ivy Bridge PEG port of the
    // bench machine the census has read `aer=n` on every boot since 8, so this is expected to be
    // silent there — and that silence is the fact, recorded rather than inferred from a missing line.
    if aer != 0 && ecam != 0 {
        let (unc, cor) = unsafe { (ecam_read32(ecam, aer + 0x04), ecam_read32(ecam, aer + 0x10)) };
        serial_println!(
            "[pcih] bar1wedge aer-boot rp uesta={:08x} cesta={:08x}",
            unc, cor
        );
    }

    // ── The arm-time W1C clear ───────────────────────────────────────────────────────────────────
    let ls_w1c = lnksta & LNKSTA_W1C;
    let ds_w1c = devsta & DEVSTA_W1C;
    let ss_w1c = secsta & SECSTA_W1C;
    unsafe {
        if ls_w1c != 0 {
            crate::arch::pci::write_config_16(rb, rs, rf, cap + 0x12, ls_w1c);
        }
        if ds_w1c != 0 {
            crate::arch::pci::write_config_16(rb, rs, rf, cap + 0x0A, ds_w1c);
        }
        if ss_w1c != 0 {
            crate::arch::pci::write_config_16(rb, rs, rf, 0x1E, ss_w1c);
        }
    }
    // Read back rather than assume: a latch that does not clear is itself a finding, and the
    // baseline the sampler deltas against must be what the hardware says, never what was intended.
    let (lnksta1, devsta1, secsta1) = unsafe {
        (
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x12),
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x0A),
            crate::arch::pci::read_config_16(rb, rs, rf, 0x1E),
        )
    };
    serial_println!(
        "[pcih] bar1wedge sticky-cleared at-arm lnksta {:04x}->{:04x} devsta {:04x}->{:04x} \
         secsta {:04x}->{:04x} (w1c written {:04x}/{:04x}/{:04x}) — EHCI's later bus walk can still \
         re-latch secsta; this narrows the window, it does not close it",
        lnksta, lnksta1, devsta, devsta1, secsta, secsta1, ls_w1c, ds_w1c, ss_w1c
    );

    BW_LNKSTA0.store(lnksta1 as u32, Ordering::Relaxed);
    BW_DEVSTA0.store(devsta1 as u32, Ordering::Relaxed);
    BW_SECSTA0.store(secsta1 as u32, Ordering::Relaxed);
    BW_LNKCTL0.store(lnkctl as u32, Ordering::Relaxed);
    BW_DEVCTL2.store(if v2 { devctl2 as u32 } else { BW_NO_V2 }, Ordering::Relaxed);
}

/// BAR1WEDGE, wedge half. Called from [`rp_at_wedge`] on every tripwire crossing, with the three
/// registers that function has already loaded — no register is read twice for this line.
///
/// READ ONLY, ROOT PORT ONLY, ECAM ONLY. It adds two loads to the crossing (the LNKCTL|LNKSTA dword
/// for the control half, and Device Control 2 when the boot found it readable), takes no lock,
/// allocates nothing, and branches on two relaxed atomics. The crossing is at most 1 Hz.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
fn bw_sample(ecam: u64, cap: u16, aer: u16, lnksta: u16, devsta: u16, secsta: u16) {
    let n = BW_SAMPLES.fetch_add(1, Ordering::Relaxed) + 1;
    let lnkctl = unsafe { (ecam_read32(ecam, cap + 0x10) & 0xFFFF) as u16 };
    let d2 = BW_DEVCTL2.load(Ordering::Relaxed);
    let devctl2 = if d2 == BW_NO_V2 {
        None
    } else {
        Some(unsafe { (ecam_read32(ecam, cap + 0x28) & 0xFFFF) as u16 })
    };
    // Delta = bits SET NOW that the post-clear baseline did not carry. Zero means the latch has not
    // moved since `pci::init`; nonzero names exactly which bits the interval added.
    let d_lnksta = lnksta & !(BW_LNKSTA0.load(Ordering::Relaxed) as u16);
    let d_devsta = devsta & !(BW_DEVSTA0.load(Ordering::Relaxed) as u16);
    let d_secsta = secsta & !(BW_SECSTA0.load(Ordering::Relaxed) as u16);
    let (unc, cor) = if aer != 0 {
        unsafe { (ecam_read32(ecam, aer + 0x04), ecam_read32(ecam, aer + 0x10)) }
    } else {
        (0, 0)
    };
    serial_println!(
        "[pcih] wedge-sample n={} first={} aperture={} lnksta={:04x} d_lnksta={:04x} ({} x{} \
         training={} bwmgmt={} autobw={}) lnkctl={:04x} lnkctl0={:04x} aspm={} lnkdis={} retrain={} \
         devsta={:04x} d_devsta={:04x} secsta={:04x} d_secsta={:04x} devctl2={:04x} cto={} dis={} \
         aer={} uesta={:08x} cesta={:08x}",
        n,
        (n == 1) as u8,
        bw_aperture(),
        lnksta,
        d_lnksta,
        bw_speed_str(lnksta),
        (lnksta >> 4) & 0x3F,
        (lnksta >> 11) & 1,
        (lnksta >> 14) & 1,
        (lnksta >> 15) & 1,
        lnkctl,
        BW_LNKCTL0.load(Ordering::Relaxed) as u16,
        aspm_str(lnkctl),
        (lnkctl >> 4) & 1,
        (lnkctl >> 5) & 1,
        devsta,
        d_devsta,
        secsta,
        d_secsta,
        devctl2.unwrap_or(0),
        match devctl2 {
            Some(v) => bw_cto_str(v),
            None => "unreadable",
        },
        match devctl2 {
            Some(v) => ((v >> 4) & 1) as u8,
            None => 0,
        },
        if aer != 0 { "y" } else { "n" },
        unc,
        cor
    );
}

// ── SECSTA2 — the SECOND sticky clear, once enumeration is complete ─────────────────────────────
//
// The rung above BAR1WEDGE's arm-time clear, named by that rung's own residual note (the block
// above, and `PCIE-RP-RECOVERY.md` §11): the at-arm clear narrows the window in which something
// other than the wedge can latch the root port's sticky status bits, but it does not close it,
// because `census` runs INSIDE `pci::init` and bus enumeration continues after it returns.
//
// WHICH ENUMERATION, THOUGH — AND THE ANSWER IS NOT THE ONE THAT WAS WRITTEN DOWN. Both that note
// and §11 name "the EHCI driver's own `0..=255` walk" as the remaining contributor. Reading the
// call order refutes it: `drivers::ehci::init` (its walk at `drivers/ehci/mod.rs:17468`) is called
// from `arch/x86_64/pci.rs:838`, and the Kepler dispatch that reaches `census` — and therefore
// `bw_arm`'s clear — is at `arch/x86_64/pci.rs:1016`. EHCI walks the bus BEFORE the at-arm clear,
// so whatever it latched is already inside the `secsta=` the at-arm line prints as its "before"
// value and is already wiped by that clear. The walks that genuinely follow it are the three in
// the tail of the same function:
//
//   * `sdhc::probe` → `PciScanner::storage_inventory` — `drivers/pci.rs:84`, buses 0..=255;
//   * `ahci::probe` — `drivers/ahci.rs:286`, buses 0..=255 (knob `UNAOS_AHCI`, default OFF);
//   * `init_network` → `PciScanner::find_device` — `drivers/pci.rs:141`, buses 0..=255, and the
//     LAST enumeration walk `pci::init` performs.
//
// So the honest call site is the tail of `pci::init` itself, not `drivers/ehci` — see the call
// statement there for the rest of the reasoning, including why it sits after the GPACE report.
//
// WHAT `relatch=` BUYS, AND IT IS THE POINT OF THE RUNG. It is the set of RW1C bits that were set
// AGAIN between the at-arm clear and the end of enumeration — i.e. the bits ENUMERATION ITSELF
// sets on this machine, measured rather than assumed. `relatch=secsta:2000` says a bus walk does
// latch Received Master Abort here, which is exactly the mechanism §1.3 says makes W7's
// `secsta=2000` at the wedge unfalsifiable — and it makes the post-enum baseline mandatory rather
// than tidy. `relatch=secsta:0000` says no walk below this bridge master-aborts at all, and a
// `d_secsta=2000` at the first stall then has nothing left to blame but the wedge.
//
// THE WRITE PATH IS THE AT-ARM ONE, UNCHANGED, AND IT NEEDS NO MAPPING RE-VERIFIED. `bw_arm`'s
// clear is CF8/CFC (`arch::pci::write_config_16`) — port I/O into the legacy 256-byte config
// region, not a store through the ECAM window — so there is no `map_mmio_window` writability to
// re-establish at this later point and no `clear=SKIPPED(unmapped)` branch to take: both registers
// written here (`0x1E`, and `cap + 0x12` with `cap` already bounded by `cap_fits(cap,
// PCIE_CAP_SPAN)`) are CF8-reachable by construction. PCIH-NOCF8's refusal is about the ~1 kHz
// tripwire band on a non-BSP core; this runs on the BSP inside `pci::init`, the same sequential
// boot phase `census` reads config space in and `bw_arm` already WRITES it in, and before the main
// loop hands another core a config writer (`wifi::service`, `main.rs:1261`).
//
// DEVSTA — SECSTA2's DELIBERATE LEFTOVER, NOW CLOSED. SECSTA2's own text ended: "`BW_DEVSTA0` is
// deliberately left holding its at-arm value, so `d_devsta` on the `wedge-sample` line still deltas
// against `pci::init`'s Kepler dispatch while `d_secsta`/`d_lnksta` delta against the end of
// enumeration … Closing it is two lines here (read `cap + 0x0A`, clear `DEVSTA_W1C`, store it) and
// is left to the seat rather than taken silently." This is that close: Device Status (PCIe base
// spec §7.5.3.5, `cap + 0x0A`) is read, its RW1C bits are cleared under the same rule the other two
// obey (write only what was read as SET), and the read-back becomes the baseline, so ALL THREE of
// the `wedge-sample` line's deltas now measure the same window — "since the end of enumeration" — instead of two of
// them measuring one window and the third measuring another.
//
// WHY IT MATTERS EVEN THOUGH `devsta=0000` ON EVERY BOOT CAPTURED SO FAR, and this is the honest
// version rather than the flattering one: the value is not the point, the WINDOW is. A reader who
// sees `d_lnksta`, `d_secsta` and `d_devsta` on one line reads them as one measurement, and until
// now one of the three silently was not. A `d_devsta=0004` (Unsupported Request Detected — the bit
// a read of a wedged BAR is most likely to set) could have been latched by ANY of the three
// post-at-arm enumeration walks named above rather than by the wedge, and nothing on the wire said
// so. The mixed baseline was a reading hazard, not a wrong number; `relatch=devsta:` is now the
// measurement that says which walks set it, exactly as `relatch=secsta:` does.
//
// NO NEW REGISTER MAPPING TO RE-VERIFY: `cap + 0x0A` is the same offset `bw_arm`'s at-arm clear
// already reads and writes through CF8, `cap` is already bounded by `cap_fits(cap, PCIE_CAP_SPAN)`,
// and `0x0A < 0x12`, so the span argument the block above makes for LNKSTA covers this by
// construction. No ECAM store, no `clear=SKIPPED(unmapped)` branch, no extra debug_assert.
//
// RESIDUAL, in the same voice the block above uses: `wifi::service` (`wifi/bus.rs:125`, buses
// 0..=255, knob `UNAOS_WIFI`) sweeps config space from the main loop, i.e. AFTER this clear, on
// every boot that arms it. On such a boot the window is "after the first wifi census" rather than
// "after enumeration", and a `d_secsta` reading has that one named alternative left. No A1 flight
// row asks for the knob (`grep -c UNAOS_WIFI docs/dev/OS/rmbp-queue.md docs/dev/OS/rmbp-ledger.md`
// = 0/0), and any given boot settles it from its own `⚡ kernel features:` banner.
//
// ── WIFISWEEP — that residual FIRED, and the dismissal above is the defect (rmbp-ledger B113) ──
//
// Flights 8 and 9 both carried `UNAOS_WIFI=1`. The grep two paragraphs up asked "does a queue file
// name this knob" when the decision needed "does the FLIGHT LINE arm it", and the answer on the
// wire was yes. Flight 8's capture, measured rather than argued:
//
//   [  23263ms] [pcih] bar1wedge sticky-cleared post-enum … relatch=secsta:2000 …
//   [  23516ms] :: wifi: brcm net function 03:00.0 … (Ethernet) — NOT the radio, skipped ::
//   [  23516ms] :: wifi: radio 04:00.0 device=0x4331 … MATCH ::
//
// 253 ms AFTER the clear, a full `for bus in 0u16..256` x 32-slot config-space sweep. A config read
// to an absent function on bus 1 — the bus BELOW the root port under test (`[pcih] ep bdf=1:0.0`) —
// master-aborts and sets that bridge's Secondary Status. So the baseline this rung stored was taken
// BEFORE the last walk of the boot, the `wedge-sample`'s `d_secsta=2000 at n=1` (both apertures,
// both flights) was read against it, and `relatch=secsta:2000` could have been that later walk's
// latch as easily as enumeration's. The rung's own doc sentence — "after the LAST bus walk of the
// boot" — was FALSE on every boot that armed the wifi knob.
//
// WHERE THE LAST WALK ACTUALLY IS, MEASURED IN THE SOURCE AND IN THE CAPTURE. Every `0..=255`-shaped
// config walk in this kernel (`grep -rn '0\.\.=255\|0u16\.\.256' crates/kernel/src`) is one of:
// `PciScanner::{enumerate_buses,storage_inventory,find_device}`, `full_census`, `detect_gpus`,
// `ahci::probe`, `ehci::init`, `ehci_scout::{scout,configure_and_relook}`, `bcma::recon`,
// `vperf::pci_display_probe` — ALL of them called from inside `arch::x86_64::pci::init` — and
// `wifi::bus::census` (`wifi/bus.rs:125`), the ONE that is not: it is reached from
// `wifi::service`'s first pass on the main loop (`main.rs`, three storage-ready sites). Nothing
// after it walks config space: in flight 8 the last of its lines is at 23562 ms and the next config
// access of any kind is the storm's own `rp-at-wedge` sample at 331272 ms, five minutes later. The
// walks are therefore NOT spread across the boot — they end, once, on the first main-loop pass —
// so the honest fix is the one B113 asked for first ("move the post-enum clear below the wifi
// census") and not the `baseline=at-storm` fallback.
//
// SO THE CALL SITE MOVED, from the tail of `pci::init` to the main loop IMMEDIATELY AFTER
// `wifi::service()`, at all three of the storage-ready passes `main.rs` carries (the same three
// `wifi::service` itself sits at, for the same reason: which pass a given x86 build reaches depends
// on its knobs). That point is after the last config walk on BOTH builds — with `wifi`, because the
// census has just returned on this very pass; without it, because `wifi::service` is cfg-erased and
// the point is still below `init_network`'s `find_device`, the last walk `pci::init` performs. The
// rung is now one-shot on a latch of its own ([`BW_POST_ENUM_DONE`]) because a loop site calls it
// every iteration forever.
//
// AND THE LINE NOW SAYS WHAT RAN IN BETWEEN. `walks_since_enum=<n>` / `walkers=<names>` count the
// post-enumeration config-space walks this boot recorded through [`note_wifi_census`], so the next
// flight's `relatch=` is attributable instead of ambiguous: `walks_since_enum=1 walkers=wifi-census`
// says the reading covers enumeration AND the wifi sweep (nothing is left to blame at the wedge);
// `walks_since_enum=0 walkers=none` says it covers enumeration alone. The count is MEASURED, not
// inferred from the knob: it is bumped at the call site, after `wifi::service()` has returned from
// the pass on which its forward-only state machine ran `bus::census()`.
//
// THE NO-DEVICE CASE NOW SPEAKS, which is what makes the order scorable off metal. The old body
// returned silently when `PCIH_READY` was false, so a q35 boot printed nothing and no gate could
// score WHERE this rung runs. It now prints one refusal line naming the same `site=` /
// `walks_since_enum=` fields, so `arroyo test` on q35 — which has no Kepler and no bcm4331, but does
// run the wifi census and park — witnesses the ORDER (refusal AFTER the wifi line) even though it
// can witness nothing about the registers.
//
// WHAT DID NOT CHANGE, and each was re-checked rather than assumed: the write path (CF8/CFC, the
// at-arm one, same two bounded offsets); the BSP (the main loop runs on cpu 0 — `SCHED-X86: BSP
// entered run loop cpu=0` — and `wifi::service`, the config accessor next to it, is on that same
// loop, so the unlocked CF8 address/data pair is no more exposed than it already was; `rp_at_wedge`,
// the one config reader on another core, is ECAM-only by construction and never touches CF8); the
// knob (`bar1wedge`, no new one); and boot pacing (GPACE samples `span` inside `pci::init`, and this
// call is now outside that function entirely, so a knob-ON boot's tiling row is untouched — the
// reason the old site sat below the GPACE block now holds trivially).

/// Bytes of the legacy type-1 header this rung reads and writes: Secondary Status is 16-bit at
/// `0x1E`, so `0x20` is the span. Below [`LEGACY_CFG_LEN`] by construction — named so the claim is
/// arithmetic rather than a reader's memory of the header layout.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
pub const SECSTA_END: u16 = 0x20;

/// BAR1WEDGE, POST-ENUMERATION half. Called from the main loop IMMEDIATELY AFTER `wifi::service()`
/// (three sites in `main.rs`, the three storage-ready passes) — WIFISWEEP moved it there from the
/// tail of `arch::x86_64::pci::init`, which was NOT after the last config walk of the boot on any
/// boot that armed `UNAOS_WIFI`; see the WIFISWEEP block above for the flight-8 measurement. Speaks
/// exactly ONCE per boot ([`BW_POST_ENUM_DONE`]) because a loop site calls it forever.
///
/// Re-reads the root port's Secondary Status, Link Status and Device Status, prints what the boot's
/// walks re-latched since the at-arm clear, clears the RW1C bits a second time, and makes the
/// read-backs the baseline all three of `bw_sample`'s `d_secsta` / `d_lnksta` / `d_devsta` delta
/// against (DEVSTA joined them here; see the section header for why).
///
/// On a machine with no GK107 (QEMU q35, every non-kepler boot) `PCIH_READY` is false and no
/// register is read or written — but one REFUSAL line is printed, carrying the same `site=` and
/// `walks_since_enum=` fields, so the ORDER this rung runs in is scorable off metal.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
pub fn sticky_clear_post_enum() {
    // One shot, and the latch is taken ABOVE the `PCIH_READY` gate so the refusal is one-shot too.
    if BW_POST_ENUM_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let walks = BW_WALKS_SINCE_ENUM.load(Ordering::Relaxed);
    if !PCIH_READY.load(Ordering::Acquire) {
        serial_println!(
            "[pcih] bar1wedge post-enum clear REFUSED reason=no-root-port (PCIH_READY=0 — \
             `census` never armed, so this machine has no GK107 and no bridge above it) \
             site=post-wifi walks_since_enum={} walkers={} — printed anyway, as the ORDER witness: \
             this line stands exactly where the clear would have run, below the LAST config-space \
             walk of the boot",
            walks,
            bw_walkers()
        );
        return;
    }
    let bdf = RP_BDF.load(Ordering::Relaxed);
    let (rb, rs, rf) = ((bdf >> 8) as u8, ((bdf >> 3) & 0x1F) as u8, (bdf & 0x7) as u8);
    let cap = RP_PCIE_CAP.load(Ordering::Relaxed) as u8;
    if cap == 0 {
        return; // invariant: census returns before arming when `find_cap` gave it nothing
    }
    // The same two preconditions `bw_arm` asserts, restated locally: this function forms `cap +
    // 0x12` and `0x1E`, and both must lie inside the 256-byte region CF8 can reach.
    debug_assert!(cap_fits(cap, PCIE_CAP_SPAN), "pcih: post-enum cap out of region");
    debug_assert!(SECSTA_END <= LEGACY_CFG_LEN, "pcih: post-enum secsta off region");

    let (secsta, lnksta, devsta) = unsafe {
        (
            crate::arch::pci::read_config_16(rb, rs, rf, 0x1E),
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x12),
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x0A),
        )
    };
    // The at-arm baseline, read BEFORE it is overwritten: `relatch` is what is set now that the
    // post-clear reading did not carry. Masked to the RW1C bits so a read-only field that legally
    // changed during enumeration (negotiated width, current speed; DEVSTA's AUX Power Detected and
    // Transactions Pending) can never be reported as a latch.
    let at_arm_ss = BW_SECSTA0.load(Ordering::Relaxed) as u16;
    let at_arm_ls = BW_LNKSTA0.load(Ordering::Relaxed) as u16;
    let at_arm_ds = BW_DEVSTA0.load(Ordering::Relaxed) as u16;
    let relatch_ss = secsta & SECSTA_W1C & !at_arm_ss;
    let relatch_ls = lnksta & LNKSTA_W1C & !at_arm_ls;
    let relatch_ds = devsta & DEVSTA_W1C & !at_arm_ds;

    // Same discipline as the at-arm clear: write ONLY the RW1C bits observed SET, and do not write
    // a register with nothing latched.
    let ss_w1c = secsta & SECSTA_W1C;
    let ls_w1c = lnksta & LNKSTA_W1C;
    let ds_w1c = devsta & DEVSTA_W1C;
    unsafe {
        if ls_w1c != 0 {
            crate::arch::pci::write_config_16(rb, rs, rf, cap + 0x12, ls_w1c);
        }
        if ds_w1c != 0 {
            crate::arch::pci::write_config_16(rb, rs, rf, cap + 0x0A, ds_w1c);
        }
        if ss_w1c != 0 {
            crate::arch::pci::write_config_16(rb, rs, rf, 0x1E, ss_w1c);
        }
    }
    // Read back rather than assume — a latch that does not clear is itself a finding, and the
    // baseline must be what the hardware says.
    let (secsta1, lnksta1, devsta1) = unsafe {
        (
            crate::arch::pci::read_config_16(rb, rs, rf, 0x1E),
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x12),
            crate::arch::pci::read_config_16(rb, rs, rf, cap + 0x0A),
        )
    };
    serial_println!(
        "[pcih] bar1wedge sticky-cleared post-enum rp={}:{}.{} site=post-wifi \
         walks_since_enum={} walkers={} secsta={:04x}->{:04x} lnksta={:04x}->{:04x} \
         devsta={:04x}->{:04x} relatch=secsta:{:04x} lnksta:{:04x} devsta:{:04x} \
         at-arm=secsta:{:04x} lnksta:{:04x} devsta:{:04x} (w1c written {:04x}/{:04x}/{:04x}) — \
         relatch is what EVERY config-space walk of this boot set after the at-arm clear: \
         enumeration, plus the walkers named above (WIFISWEEP — this clear now runs BELOW the \
         wifi census, not above it); wedge-sample d_secsta/d_lnksta/d_devsta ALL delta against \
         THIS baseline",
        rb, rs, rf, walks, bw_walkers(), secsta, secsta1, lnksta, lnksta1, devsta, devsta1,
        relatch_ss, relatch_ls, relatch_ds, at_arm_ss, at_arm_ls, at_arm_ds, ss_w1c, ls_w1c, ds_w1c
    );

    BW_SECSTA0.store(secsta1 as u32, Ordering::Relaxed);
    BW_LNKSTA0.store(lnksta1 as u32, Ordering::Relaxed);
    BW_DEVSTA0.store(devsta1 as u32, Ordering::Relaxed);
}

/// WIFISWEEP. One-shot latch for [`sticky_clear_post_enum`]: its call site is now a main-loop one
/// (three of them), so without this the rung would clear and re-baseline on every iteration and the
/// `relatch=` field — the whole finding — would read `0000` from the second pass onward.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_POST_ENUM_DONE: AtomicBool = AtomicBool::new(false);

/// WIFISWEEP. Config-space bus walks recorded since `pci::init` returned, i.e. since the end of
/// enumeration. Bumped only through the `note_*` functions below, each of which is idempotent, so
/// this counts WALKS and not loop passes.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_WALKS_SINCE_ENUM: AtomicU32 = AtomicU32::new(0);

/// WIFISWEEP. True once the wifi arc's `bus::census()` sweep has run. Separate from the counter so
/// [`bw_walkers`] can NAME the walk rather than only count it.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
static BW_WIFI_CENSUS_NOTED: AtomicBool = AtomicBool::new(false);

/// WIFISWEEP. The post-enumeration walkers this boot actually ran, for the `walkers=` field. A
/// name list and not a knob reading: `wifi-census` appears only after [`note_wifi_census`] has been
/// reached, which happens on the pass `wifi::service()` performed the sweep on.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
fn bw_walkers() -> &'static str {
    if BW_WIFI_CENSUS_NOTED.load(Ordering::Relaxed) {
        "wifi-census"
    } else {
        "none"
    }
}

/// WIFISWEEP. Record that the wifi arc's one config-space sweep (`wifi::bus::census`,
/// `wifi/bus.rs:125` — `for bus in 0u16..256` x 32 slots) has run.
///
/// Called from `main.rs` IMMEDIATELY AFTER `unaos_kernel::wifi::service()`, under
/// `all(target_arch = "x86_64", feature = "wifi", feature = "bar1wedge")`, at each of the three
/// storage-ready passes. Correct by the arc's own shape: `wifi::service` is forward-only and runs
/// `bus::census()` on its FIRST call and never again, so "service has returned at least once" and
/// "the sweep has happened exactly once" are the same fact. Idempotent, so the loop cannot inflate
/// the count — 1 is the honest answer however many times the loop turns.
#[cfg(all(target_arch = "x86_64", feature = "bar1wedge"))]
pub fn note_wifi_census() {
    if !BW_WIFI_CENSUS_NOTED.swap(true, Ordering::Relaxed) {
        BW_WALKS_SINCE_ENUM.fetch_add(1, Ordering::Relaxed);
    }
}

// ── W5I1 — the POST-STEAL BAR0 DUMP from the stealing core (PCIE-RP-RECOVERY.md §12.3, I1) ─────────
//
// WHAT IT ANSWERS. W5SCOPE (§12.1) refuted every link-side branch of W5 on flights 8 and 9 with the
// odometer alone: after each of the seven steals the NEW holder's CPU blits into the same BAR1 window
// completed within 70–369 ms while the parked core's one store never retired (`revenants=0`). The
// branch left standing is (d) — a per-core transaction lost or unacknowledged UPSTREAM of the root
// port — and nothing in the tree has ever asked the DEVICE what it latched about that store. This is
// the instrument that asks: one shot, from the core that just took the gate, reading the Kepler's
// host-side BAR0 state and printing one `:: W5: site=post-steal … ::` line beside `GATE STOLEN`.
//
// WHY THIS SITE AND NO OTHER — the non-posted-read ordering rule. PCIe r3.0 §2.4.1 Table 2-40: a
// Non-Posted Request must not pass a Posted Request. A BAR0 or ECAM read of the endpoint issued from
// the SAMPLER path (the tripwire or the deadman, on the service core, WHILE the store is stuck) would
// queue behind the stuck posted write if that write were at the root port, would never be
// transmitted, would run no completion timer, and would park the service core — this laptop's only
// serial carrier (§1.2, B114). §12.3 says so and refuses that site. Immediately AFTER the steal the
// situation is different in the only way that matters: the link is PROVEN (on this capture, within
// 70–369 ms of every steal) to be sinking the next holder's posted writes, so a non-posted read is
// transmitted and bounded by the 50 ms completion timeout the boot measured (`cto=50us-50ms dis=0`),
// and the reader is the live core the steal already chose — not the corpse, not the pump. Under
// branch (d) the read is harmless; under the refuted branches it is the same read `wm.rs`'s next
// holder is about to make a million times.
//
// THE RULES OF THE READ, each one a bound and not a habit:
//   * exactly the registers §12.3 lists, each read ONCE, in §12.3's order; no loop, no poll, no
//     retry, and NEVER a write — every one of these is a status latch or a fault record, and a write
//     here would be the injector this instrument exists to rule out;
//   * `0xBADFxxxx` is the nonexistent-PRI signature this tree already decodes (`kepler.rs`, the
//     bar1-identity rung: `(x >> 16) == 0xBADF`) and `0xFFFFFFFF` is a completion timeout's all-ones;
//     both are PRINTED AS READ — the value is the finding, and a reader who sees `badf` in a field
//     knows that register does not exist on this part (the UNPINNED set below is nouveau's GF100
//     family map, never verified on GK107, exactly as `kepler.rs:3799` says of `0x1700`);
//   * one `serial_println!` through the same path `GATE STOLEN` uses, so the line cannot deadlock on
//     a lock the dead core holds — this function takes no lock and allocates nothing;
//   * NO CF8/CFC. The steal runs on an arbitrary core at an arbitrary instant; PCIH-NOCF8 forbids the
//     unlocked address/data pair there. The endpoint's PCI Status comes through the ECAM page `census`
//     verified ([`EP_ECAM`], a plain MMIO load), or prints `none` when `census` never verified one.
//
// THE REGISTERS (BAR0 byte offsets; `kepler.rs::regs` for the pinned ones):
//   pinned    PMC_INTR_0 `0x000100` · PFIFO_INTR_0 `0x002100` · PFIFO flush `0x070000` (read only —
//             bit 1 busy; the trigger write at `kepler.rs:1693` is `nvidia-kepler-pfifoflush`'s and is
//             not made here)
//   ep_pcists the endpoint's PCI Status word (config `0x06`, the upper half of the command|status
//             dword at `0x04`) through the ECAM page `census` cached — the brief's "via config space
//             through the census path". §12.3's table names the PBUS mirror `0x1804` for the same
//             word; the config path was chosen because it is INDEPENDENT of BAR0: if every BAR0 field
//             on the line reads all-ones and `ep_pcists` reads sane, the host interface's PRI path is
//             dead while the PCIe core still answers config requests, which the mirror could not say
//   UNPINNED  PBUS_INTR_0 `0x001100` · PRI fault addr/data `0x009084`/`0x009088` · PFIFO fault-unit
//             mask `0x00259c` · BAR1 fault record `0x002840`/`0x2844`/`0x2848`/`0x284c` (unit 4 on
//             the GK104 family: inst / vaddr lo / vaddr hi / info)
//
// WHERE BAR0 COMES FROM. `census` runs from `kepler::init` BEFORE that function reads and maps BAR0
// (`kepler.rs:1384`), so the base is cached here from config `0x10` (`w5_cache_bar0`, folded onto the
// `EP_ECAM.store` line in `census` so no line below it moves). Whether it is MAPPED at steal time is
// not assumed: `kepler::init` can abort after `census` returns and before or after the map
// (`probe-abort bar0-unmapped` / `bar1-unmapped` / `bar1-not-64bit`), so the dump asks the live page
// tables the same question `kepler::init` asks (`arch::memory::translate`) and prints the short
// `bar0=unmapped` form when the answer is no. On a machine with no GK107 (QEMU q35) `census` never
// runs, the base stays 0, and the short form is what a steal prints there.
//
// `aim=` is `wm.rs`'s `BLIT_AIM_CORE[dead]` — the byte offset INTO THE PANEL SURFACE (`py * fb_row +
// x0 * bpp`) of the dead core's last `copy_nonoverlapping`, the same value `GATE STOLEN` prints as
// `blit_aim=`; the surface is at BAR1 + `0x20000` on the bench (`KDHEAD: … vram_off=00020000`), so the
// BAR1 offset is `aim + 0x20000` and the bus address `0x90000000 + 0x20000 + aim`. It is carried
// here so `bar1_fault`'s `lo/hi` can be compared against it on one line.
//
// SCORING (the outcome table is §12.3's, restated so the wire is readable without the doc):
//   `bar1_fault=` info valid with lo/hi near aim+0x20000 → the GPU MMU faulted the parked store's
//     page; (b) reopens as a page-table fault — but a faulted posted write is DROPPED, not held, so a
//     parked core beside a fault is still (d) and the fault is a second defect
//   `pbus_intr=` PRI TIMEOUT or `pri_fault=` nonzero → the PRI path wedged; (b) reopens for BAR0
//     traffic, still not for the posted store
//   `pfifo_intr=` MMU-fault bit with `fault_mask=0` → a fault `kepler.rs:1429`'s interrupt disable
//     masked; read the record anyway
//   all zero → the GPU latched nothing about the parked store; (d) stands alone
//   every BAR0 field all-ones or `badf` while `ep_pcists` is sane → the PRI path is dead and the PCIe
//     core is not; the read itself is the finding (and it RETURNED — the completion timeout bounded it)
//
// Read with `awk 'index($0,":: W5:")'`. This block is under the module's `nvidia-kepler` gate (the
// same region `rp_at_wedge`'s call at `wm.rs:10469` sits in) and the call site is folded onto the
// `COMP_STEALS.fetch_add` line at `wm.rs:4888` under `witness`, so a knob-off image moves by no byte.

/// W5I1 — the endpoint's BAR0 base as `census` read it from config `0x10`/`0x14` (type-bits masked;
/// the 64-bit high half OR'd in when the BAR says so). 0 until `census` runs, and 0 forever on a
/// machine with no GK107 or an I/O-space BAR0, which the dump reports as `bar0=unmapped`.
#[cfg(target_arch = "x86_64")]
static W5_BAR0: AtomicU64 = AtomicU64::new(0);

/// W5I1 — cache the endpoint's BAR0 base. Called once from [`census`] on the BSP inside `pci::init`,
/// with the same CF8 accessors and in the same sequential phase every other config read there uses.
/// Two config reads at most; no write.
#[cfg(target_arch = "x86_64")]
fn w5_cache_bar0(bus: u8, slot: u8, func: u8) {
    let lo = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x10) };
    if lo & 0x1 != 0 || lo == 0xFFFF_FFFF {
        return; // I/O-space BAR0, or no device answered: nothing MMIO to dump
    }
    let mut base = (lo & 0xFFFF_FFF0) as u64;
    if (lo >> 1) & 0x3 == 0x2 {
        let hi = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x14) };
        base |= (hi as u64) << 32;
    }
    W5_BAR0.store(base, Ordering::Relaxed);
}

/// W5I1 — one field that is either a 16-bit register or `none`, so the line keeps one token per
/// field whether or not `census` verified the endpoint's ECAM page.
#[cfg(target_arch = "x86_64")]
struct W5Hex16(Option<u16>);
#[cfg(target_arch = "x86_64")]
impl core::fmt::Display for W5Hex16 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{:04x}", v),
            None => f.write_str("none"),
        }
    }
}

/// W5I1 — THE POST-STEAL DUMP. Called from `wm::composite`'s steal arm on the core that has just
/// taken the gate (`from`), immediately after `COMP_STEALS.fetch_add` (`wm.rs:4888`), with the dead
/// core, the hold length in ms and the dead core's last blit aim. Prints exactly one line and returns;
/// see the block above for every rule it obeys and every register it reads.
#[cfg(target_arch = "x86_64")]
pub fn w5_post_steal(from: usize, dead: usize, held: u64, aim: u64) {
    let bar0 = W5_BAR0.load(Ordering::Relaxed);
    if bar0 == 0 || crate::arch::memory::translate(bar0).is_none() {
        serial_println!(
            ":: W5: site=post-steal from=c{} dead=c{} held={} bar0=unmapped ::",
            from, dead, held
        );
        w5_nmi(dead); return; // W5I2 wired at the W5I1 fold: NMI the corpse after the short form
    }
    // One volatile dword load per register, in §12.3's order. `bar0` is identity-mapped by
    // `kepler::init`'s `map_mmio_window` (the same base `kepler::mmio_read` dereferences), and every
    // offset below is inside the 16 MiB BAR0 the census sized.
    let rd = |off: u64| unsafe { core::ptr::read_volatile((bar0 + off) as *const u32) };
    let pmc_intr = rd(0x0100);
    let pfifo_intr = rd(0x2100);
    let flush = rd(0x7_0000);
    let ep = EP_ECAM.load(Ordering::Relaxed);
    let ep_pcists = if ep != 0 { Some((unsafe { ecam_read32(ep, 0x04) } >> 16) as u16) } else { None };
    let pbus_intr = rd(0x1100);
    let pri_addr = rd(0x9084);
    let pri_data = rd(0x9088);
    let fault_mask = rd(0x259c);
    let bf_inst = rd(0x2840);
    let bf_lo = rd(0x2844);
    let bf_hi = rd(0x2848);
    let bf_info = rd(0x284c);
    serial_println!(
        ":: W5: site=post-steal from=c{} dead=c{} held={} aim={:#x} pmc_intr={:08x} \
         pfifo_intr={:08x} flush={:08x} ep_pcists={} pbus_intr={:08x} pri_fault={:08x}/{:08x} \
         fault_mask={:08x} bar1_fault={:08x}/{:08x}/{:08x}/{:08x} ::",
        from, dead, held, aim, pmc_intr, pfifo_intr, flush, W5Hex16(ep_pcists), pbus_intr,
        pri_addr, pri_data, fault_mask, bf_inst, bf_lo, bf_hi, bf_info
    ); w5_nmi(dead); // W5I2 wired at the W5I1 fold: registers first, then NMI the corpse (§12.3 I2)
}

/// aarch64 shim — `kepler::init` aborts before any steal can exist there; keeps an armed aarch64
/// type-check green while emitting nothing, exactly as the shims above do.
#[cfg(not(target_arch = "x86_64"))]
pub fn w5_post_steal(_from: usize, _dead: usize, _held: u64, _aim: u64) {}

/// W5I2's probe, called by `w5_post_steal` AFTER its register line so the NMI cannot perturb the
/// dump. `nmi_probe` prints its own `:: W5: nmi core=c<n> taken=<y/n> …` line and takes no lock;
/// absent from images that carry neither `witness` nor `bar1wedge` (the probe's own cfg).
#[cfg(all(target_arch = "x86_64", any(feature = "witness", feature = "bar1wedge")))]
fn w5_nmi(dead: usize) { let _ = crate::arch::interrupts::nmi_probe(dead); }
#[cfg(not(all(target_arch = "x86_64", any(feature = "witness", feature = "bar1wedge"))))]
fn w5_nmi(_dead: usize) {}
