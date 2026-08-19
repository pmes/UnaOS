// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Jetson Orin Nano (Tegra234) kernel-owned MMU (the `tegra` feature). The tegra build boots via
// NVIDIA's on-board UEFI, which hands the kernel off with the MMU *already on* — but its translation
// tables map RAM and the firmware's own regions, NOT the Tegra peripheral MMIO. JM2 R4 proved this on
// silicon: the kernel loaded and entered, then faulted on its very first UARTC read (`ldr` @
// 0x0C28_0014), caught by UEFI's still-resident ArmCpuDxe handler, because that device page is
// unmapped in the handoff tables. So before any Tegra peripheral MMIO — the UART, and the GIC/timer in
// JM4 — the kernel must install its OWN translation regime that maps RAM Normal-WB and the Tegra
// device windows Device-nGnRE. This module is that regime: the EL2 analogue of the Pi bare-metal
// `boot::mmu_init`/`enable_mmu` (which we cannot reuse — `boot.rs` is `#[cfg(feature = "baremetal")]`,
// runs at EL1 after `drop_to_el1`, and is compiled out of the tegra/UEFI build).
//
// Runs from `tegra_early_stop` (main.rs), BEFORE `unaos_kernel::init()` and `arch::memory::init` — so
// there is no allocator: statics only, no heap, no locks that need the very MMU we are enabling.
//
// Exception level. UEFI hands off at whatever EL it runs the kernel at; on QEMU `virt,virtualization=
// on` and (expected) on the Orin that is EL2, non-VHE (HCR_EL2.E2H == 0 — the regime EDK2/ArmPkg sets
// up). We read `CurrentEL` first (a register read, cannot fault) and program the regime for the EL we
// are actually at: **EL2 primary, EL1 fallback**. The two regimes use the same table shape but
// different system registers and slightly different leaf-descriptor bits (below).
//
// NOTE (E2H / VHE) — feeds Part D diagnosis + JM4: this module assumes non-VHE EL2 (E2H == 0), which
// is what EDK2 firmware establishes and what the integrator-pinned `TCR_EL2` value below encodes. On
// A72 (QEMU `virt`) VHE does not exist, so E2H is RES0 = 0. On the Orin's A78AE, VHE *exists*; were
// NVIDIA's UEFI to hand off with E2H == 1, this non-VHE `TCR_EL2`/`SCTLR_EL2` programming would be for
// the wrong regime and the switch would fault into the Part-C vector (or dark, if it faults on the
// first post-switch fetch). That would be a documented STOP-tripwire-(a) diagnosis at Part D, not a
// silent workaround here — the EL2 non-VHE design is what the brief pins.

use unaos_boot_info::{BootInfo, MemoryRegion, MemoryRegionKind};

/// A single Level-1 translation table: 512 entries × 8 bytes = one 4 KiB page. With a 4 KiB granule
/// and T0SZ=25 (39-bit VA) the top lookup level is L1 and each entry maps a **1 GiB block**, so these
/// 512 entries cover the whole 512 GiB VA. We fill `L1[0]` (the low-1-GiB peripheral window) as Device
/// and every GiB the firmware memory map calls RAM as Normal; the rest stay invalid (unmapped → a
/// stray access faults into the Part-C vector rather than silently succeeding). Written entry-by-entry
/// in `build_l1` (every one of the 512), so we do not depend on the loader zeroing `.bss`.
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1: PageTable = PageTable([0; 512]);

/// The **EL1-precise twin** of `L1`, built only on the EL2-primary path, for the JM6 EL2 -> EL1 drop
/// (`boot_tegra`). Same 512-entry shape, same RAM-GiB set, but with the EL1&0 leaf recipe: RAM
/// AP[2:1]=0b00 (EL1 read-write, no EL0, EL1-executable), Device UXN|PXN. The live EL2 `L1` CANNOT be
/// reused as the EL1 table — its leaves set AP[1] (RES1 in the single-privilege EL2 regime), which the
/// EL1&0 regime reads as AP[2:1]=0b01 = "EL0 read-write", and the VMSA forces PXN=1 for any region
/// writable at EL0 (DDI 0487, stage-1 instruction access permissions) regardless of the descriptor PXN
/// bit or SCTLR_EL1.WXN. That made every RAM GiB privileged-execute-never and was the JM6 metal dark
/// hang: the first EL1 fetch aborted, and the VBAR_EL1 vector (same RAM) could not even fetch its
/// handler. "No EL0 exists yet" does not matter — the rule is unconditional.
static mut L1_EL1: PageTable = PageTable([0; 512]);

// ── XCARVE-3/6: protected-carveout hole exclusion (a SET of windows) ─────────────────────────────────
//
// Boot-21 capture proved PA 0x26b900000 is firmware-protected carveout DRAM: the RAS (SNOC, IERR =
// Carveout Uncorrectable, SERR = Illegal address) fired from a DC CIVAC of that line at *post-mmu*, with
// none of our own code yet run — there is no software writer, and any cache-line traffic (fill, eviction
// writeback, speculation) that touches the window is rejected by the fabric. Our `build_l1` maps whole
// GiB blocks Normal-WB, so a carveout hidden inside a RAM GiB is *cacheable* today — that is the defect
// class. The correctness fix is to remove every such window from the cacheable map. We do so by
// **unmapping** it (not Device): an unmapped VA has no valid translation, so the MMU can never fill,
// speculate into, or write back that line — the fabric is never touched. (Device would still be
// non-cacheable and safe, but it leaves a live translation an explicit or stray access could reach;
// unmapped is strictly safer and nothing legitimate lives in these windows — the heap-guard seats the
// heap clear of them and the framebuffer carveout is never punched, see below.)
//
// XCARVE-6 GENERALIZES XCARVE-3 from one window to a SET. Boot-25 died on the boot-13 "0xbe" family
// (0xbe0d6c60/70), and a rerun under vug/simmer load added 0xbf77a500 — points spanning
// 0xbe000000..0xc0000000 (XCARVE-8/boot-27 later extended the family past 0xc0000000 — see
// `XCARVE_BE_SIZE`). So a single quirk is no
// longer enough: the map must honor EVERY protected window. The set is (1) the two undeclared QUIRK
// windows the firmware hides inside Conventional/Usable DRAM (0x26b9 and 0xbe), and (2) the STRUCTURAL
// generalization — every DTB `/reserved-memory` carveout that falls inside a RAM GiB — so any window the
// firmware *declares* can never be touched by cache traffic. That retires the whole class, not one address.
//
// Each window is sub-GiB, so the GiB containing it is sub-divided into an L2 table of 512 × 2 MiB blocks —
// every block Normal-WB RAM *except* the block(s) intersecting ANY window, which are left invalid. Windows
// in the same GiB share that GiB's L2 table; one pool slot per split GiB (Orin DRAM = GiB 2..=9, and the
// two quirks land in GiB 2 and GiB 9). The live EL2 table and its EL1-precise twin each get their own
// pool. This is `tegra`-only; on the `virt` gate the set is empty, no L2 split, `build_l1` byte-identical.

/// Max protected windows we exclude: the two QUIRKs plus every DTB `/reserved-memory` carveout inside a
/// RAM GiB. 32 is well above the observed handful; overflow is witnessed (XCARVE-5 no-silent-drop law).
#[cfg(feature = "tegra")]
pub const MAX_HOLES: usize = 32;

/// Max distinct RAM GiBs sub-divided into an L2 table. Orin DRAM spans GiB 2..=9 (8 GiBs) and a carveout
/// can sit in any of them, so 8 always suffices; a hole beyond the pool is witnessed, never silently kept
/// cacheable.
#[cfg(feature = "tegra")]
const MAX_SPLIT_GIB: usize = 8;

/// XCARVE-6/8: the boot-13/boot-25/boot-27 "0xbe" protected window, undeclared by the DTB (like 0x26b9)
/// → QUIRK. XCARVE-6 guessed the classic VPR-shaped **32 MiB** `[0xbe000000, 0xc0000000)`; boot-27
/// REFUTED that guess (SNOC RAS Carveout, ADDR 0x80000000c0883000 → PA 0xc0883000, ~8.5 MiB ABOVE the
/// 32 MiB top, under the same vug/simmer eviction load). Observed family: 0xbe000f80 (boot-13),
/// 0xbe0d6c60/70 (boot-25), 0xbf77a500 (boot-25-rerun), 0xc0883000 (boot-27) — span ≈ 40.5 MiB. No
/// honest extent is readable: the DTB/UEFI sets stay silent (XCARVE-3/6 exhausted them — that IS why
/// this is a QUIRK), and probing the MC GSC carveout config registers from NS-EL2 is the JB1d class
/// (EL3-gated IMPDEF access crashed BL31 on metal) and unverifiable in QEMU. So XCARVE-8 widens the
/// bound: **96 MiB** `[0xbe000000, 0xc4000000)` — a defensible carveout-granule-aligned (64 MiB-aligned
/// top) envelope leaving ~55 MiB of headroom over the highest observed hit. This extent is a bounded
/// GUESS, said so in the banner; a hit above 0xc4000000 refutes it in turn. The window now straddles
/// the GiB 2/3 boundary — `install_carveout_holes` splits every GiB the span touches (the XCARVE-8
/// straddle fix), not just the base GiB.
#[cfg(feature = "tegra")]
const XCARVE_BE_BASE: u64 = 0xbe00_0000;
#[cfg(feature = "tegra")]
const XCARVE_BE_SIZE: u64 = 0x0600_0000; // 96 MiB, ends at 0xc400_0000 — a GUESS (see doc above)

/// XCARVE-9: the PRIMARY "0x26b9" QUIRK window's honest extent. XCARVE-3 guessed the single 2 MiB L2
/// granule containing the boot-21 hit (`[0x26b800000, 0x26ba00000)`); boot-28 run 2 REFUTED that guess
/// (SNOC Carveout Uncorrectable + ACI FillWrite, ADDR 0x800000026bc5ee90 → PA 0x26bc5ee90, ~2.4 MiB
/// ABOVE the 2 MiB top, mid-simmer; EL3 powered off two cores, the machine survived) — the same
/// too-small-extent class XCARVE-8 fixed for the 0xbe window. As with 0xbe, no honest extent is
/// readable (the DTB/UEFI sets are silent over this PA — that IS why it is a QUIRK; MC-register
/// probing is the rejected EL3-crash class). But here the geometry gives a bound the 0xbe window never
/// had: the DTB DOES declare adjacent carveouts — `[0x26b5f0000, 0x26b7f0000)` below and
/// `[0x26c180000, 0x26c400000)` above — and the observed family (0x26b900000, 0x26bc5ee90) sits in the
/// undeclared gap between them, suggesting one contiguous protected span ~`[0x26b5f0000, 0x26c400000)`.
/// So XCARVE-9 widens the QUIRK to fill exactly that gap: **9.5 MiB** `[0x26b800000, 0x26c180000)` —
/// top flush against the declared upper neighbor, never overlapping it (the declared windows stay
/// their own set entries; adjacency is fine). This extent is a bounded GUESS, said so in the banner; a
/// hit in the residual `[0x26b7f0000, 0x26b800000)` sliver or above 0x26c400000 refutes it in turn.
/// Heap is unaffected: it tops at 0x26b3ca000, below every window here (`select_heap_region` re-proves
/// this against the same set).
#[cfg(feature = "tegra")]
const XCARVE_GIB9_BASE: u64 = 0x2_6b80_0000;
#[cfg(feature = "tegra")]
const XCARVE_GIB9_SIZE: u64 = 0x0098_0000; // 9.5 MiB, ends at 0x26c180000 (DTB neighbor base) — a GUESS

/// XCARVE-10: the GiB-9 protected span continues ABOVE the declared `[0x26c180000, 0x26c400000)`
/// DTB carveout. boot-34-retry (2026-07-21) hit SNOC Carveout Uncorrectable + ACI FillWrite at
/// ADDR 0x800000026c6be4a0 → PA 0x26c6be4a0 — above 0x26c400000, refuting the XCARVE-9 assumption
/// that the declared upper neighbor capped the family. Same method as XCARVE-9: no honest extent is
/// readable (DTB/UEFI silent, MC probing is the rejected EL3-crash class), so exclude a bounded
/// GUESS window starting flush at the declared neighbor's top. XCARVE-10 tried 4 MiB and was
/// refuted within the hour; see the XCARVE-11 note at `XCARVE_GIB9B_SIZE` for the full-gap extent
/// now used. Heap (top 0x26b3ca000) unaffected.
#[cfg(feature = "tegra")]
const XCARVE_GIB9B_BASE: u64 = 0x2_6c40_0000;
#[cfg(feature = "tegra")]
// XCARVE-11: the 4 MiB XCARVE-10 guess was refuted within the hour (boot-36-retry RAS at PA
// 0x26d03f600, above 0x26c800000). Four hits now CLIMB monotonically (0x26b900000 → 0x26bc5ee90
// → 0x26c6be4a0 → 0x26d03f600) — incremental widening is a refuted method; the protected span is
// (or behaves as) the ENTIRE undeclared gap. So exclude everything from the declared
// [0x26c180000,0x26c400000) carveout's top to the next declared object above, the
// framebuffer/scanout carveout at 0x279e00000 (CPU-written; never excluded): 218 MiB. Heap
// (top 0x26b3ca000) and every mapped consumer sit below; the gap holds nothing we map. A RAS at
// or above 0x279e00000 (inside fb) would refute the static-window model itself, not the extent.
#[cfg(feature = "tegra")]
const XCARVE_GIB9B_SIZE: u64 = 0x0da0_0000; // 218 MiB, ends at 0x279e00000 (fb carveout base) — full gap

#[cfg(feature = "tegra")]
static mut L2_POOL: [PageTable; MAX_SPLIT_GIB] = [const { PageTable([0; 512]) }; MAX_SPLIT_GIB];
#[cfg(feature = "tegra")]
static mut L2_POOL_EL1: [PageTable; MAX_SPLIT_GIB] = [const { PageTable([0; 512]) }; MAX_SPLIT_GIB];

// Resolved PRIMARY hole (the 0x26b9 window, slot 0), latched once by `init` so `select_heap_region`
// (heap/span exclusion) and the VUGRAS localizer (`crate::vugras`) that key off `XCARVE_TARGET_PA` read
// the SAME extent the map excluded. `0` size ⇒ no hole. The FULL set is in `HOLES_*` / `HOLE_N`.
#[cfg(feature = "tegra")]
static HOLE_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "tegra")]
static HOLE_SIZE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "tegra")]
static HOLE_SOURCE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

// XCARVE-6: the FULL excluded-window set, latched once by `init`, so the VUGRAS identity witness can list
// every window (no-silent-drop). Plain atomic arrays (single-threaded pre-SMP write, later reads).
#[cfg(feature = "tegra")]
static HOLE_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(feature = "tegra")]
static HOLES_BASE: [core::sync::atomic::AtomicU64; MAX_HOLES] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_HOLES];
#[cfg(feature = "tegra")]
static HOLES_SIZE: [core::sync::atomic::AtomicU64; MAX_HOLES] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_HOLES];
#[cfg(feature = "tegra")]
static HOLES_SRC: [core::sync::atomic::AtomicU8; MAX_HOLES] =
    [const { core::sync::atomic::AtomicU8::new(0) }; MAX_HOLES];

// ── NET-4B: the Normal-NC DMA window (rings + RX/TX buffers) ─────────────────────────────────────────
//
// NET-4A's verdict: the RTL8168/Tegra-RC integration reuses the last-fetched descriptor's buffer address
// for every later RX completion (a NIC/RC-internal defect below the driver's lane); every cache-maintenance
// theory on the CACHEABLE ring/buffers was refuted, and the ring DRAM was proven correct. The one
// structural difference left vs Linux-on-this-silicon (which leases) is that r8169's rings live in
// dma_alloc_coherent (non-cacheable/coherent) memory while ours were cacheable DRAM with manual
// clean/invalidate. NET-4B matches the exercised config: a small Normal-NC window the driver lays its
// rings + buffers into, removing every maintenance-vs-fetch race by construction (first principle: do it
// right by matching the config the hardware actually leases under, not by out-guessing a below-lane bug).
//
// PLACEMENT. One L2 2 MiB block is ample (32×2048 RX + 8×2048 TX buffers + both rings < 256 KiB). It must
// be RAM, carveout-clean, DMA-reachable, and never double-used by the heap. The heap-guard evidence
// (`[0x2683ca000, 0x26b3ca000)`, the "highest clean window") shows the usable DRAM tops EXACTLY at the
// heap top on this silicon — a FIXED PA above it would land in reserved DRAM. So the window is DERIVED,
// not hardcoded: `select_heap_region` reserves it as a 2 MiB-aligned block carved from the SAME clean +
// DMA-reachable window it seats the heap in (the heap moves to the top of that window, the NC block sits
// just below it, no overlap by construction — see `select_heap_region`). `NET4B_NC_BASE` is latched there
// and the window is mapped NC by `install_net4b_nc` (below), which patches GiB 9's always-present L2 split.
#[cfg(feature = "tegra")]
pub const NET4B_NC_SIZE: u64 = 2 * 1024 * 1024;
#[cfg(feature = "tegra")]
static NET4B_NC_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// NET-4B: the reserved Normal-NC DMA window `(base, size)`, or `(0, 0)` before `select_heap_region`
/// has run / on the fail-closed path. The RTL8168 driver reads this to lay its rings + buffers in NC RAM.
#[cfg(feature = "tegra")]
pub fn net4b_nc_window() -> (u64, u64) {
    use core::sync::atomic::Ordering;
    let base = NET4B_NC_BASE.load(Ordering::Relaxed);
    (base, if base != 0 { NET4B_NC_SIZE } else { 0 })
}

/// XCARVE-3: the PRIMARY excluded carveout hole `(base, size, source)` (`source`: 0 none, 1 DTB, 2 QUIRK)
/// — the 0x26b9 window that `select_heap_region` and the VUGRAS `XCARVE_TARGET_PA` tripwire key off — or
/// `(0,0,0)` before `init` / when none. See `carveout_holes` for the full XCARVE-6 set.
#[cfg(feature = "tegra")]
pub fn carveout_hole() -> (u64, u64, u8) {
    use core::sync::atomic::Ordering;
    (
        HOLE_BASE.load(Ordering::Relaxed),
        HOLE_SIZE.load(Ordering::Relaxed),
        HOLE_SOURCE.load(Ordering::Relaxed),
    )
}

/// XCARVE-6: enumerate EVERY excluded carveout window into `out`, returning the count. Read by the VUGRAS
/// identity witness so it lists all excluded windows (XCARVE-5 no-silent-drop law).
#[cfg(feature = "tegra")]
pub fn carveout_holes(out: &mut [(u64, u64, u8)]) -> usize {
    use core::sync::atomic::Ordering;
    let n = HOLE_N.load(Ordering::Relaxed).min(out.len()).min(MAX_HOLES);
    for (i, slot) in out.iter_mut().enumerate().take(n) {
        *slot = (
            HOLES_BASE[i].load(Ordering::Relaxed),
            HOLES_SIZE[i].load(Ordering::Relaxed),
            HOLES_SRC[i].load(Ordering::Relaxed),
        );
    }
    n
}

/// XCARVE-6: build the SET of protected windows to exclude, into `out`, returning `(kept, dropped)` where
/// `dropped` counts windows that overflowed `out` (witnessed by the caller — never a silent cap).
///
/// Slot 0 is always the PRIMARY 0x26b9 window (DTB-covering node → source 1, else the 9.5 MiB XCARVE-9
/// QUIRK — a bounded GUESS, see `XCARVE_GIB9_SIZE` → source 2), so `carveout_hole()`/`HOLE_*` stay the 0x26b9 extent every existing consumer expects. Slot 1
/// is the XCARVE-6/8 0xbe window (DTB-covering node → source 1, else the full 96 MiB QUIRK — a bounded GUESS). Then the
/// STRUCTURAL set: every DTB `/reserved-memory` carveout that falls inside a RAM GiB (deduped against the
/// quirks). The framebuffer carveout is NEVER added — it is CPU-written scanout, not a no-touch firewall
/// window; unmapping it would break the panel — so any DTB node intersecting `[fb, fb+fb_size)` is skipped.
#[cfg(feature = "tegra")]
fn resolve_carveout_holes(
    boot_info: &BootInfo,
    ram_gib_mask: u64,
    out: &mut [(u64, u64, u8)],
) -> (usize, usize) {
    const BLK2: u64 = 2 * 1024 * 1024;
    let cap = out.len();
    let mut n = 0usize;
    let mut dropped = 0usize;
    let dtb_addr = boot_info.dtb_addr;
    let dtb_size = boot_info.dtb_size;

    let mut carve = [(0u64, 0u64); 48];
    let nfdt = super::fdt_tegra::reserved_carveouts(dtb_addr, dtb_size, &mut carve);

    // A window sits inside a RAM GiB iff that GiB is Normal-WB in the base map (so it is cacheable today).
    let in_ram_gib = |base: u64| -> bool {
        let g = base >> 30;
        g >= 1 && g < 64 && (ram_gib_mask >> g) & 1 != 0
    };
    // The framebuffer/scanout carveout is legitimately CPU-written — never unmap it. This guard keys on
    // `boot_info.framebuffer_addr`, which is `0` on the "booting without a display" loader path (boot-26)
    // — so it CANNOT catch a fb the kernel later inherits from the DTB `simple-framebuffer` node. XCARVE-7
    // belt-at-punch: a node-name skip ("framebuffer"/"simple-framebuffer") would need `reserved_carveouts`
    // to also thread each node's name string (a signature + classifier change in `fdt_tegra.rs`), which is
    // NOT cheap; and it is not needed for correctness — `map_fb_region`'s XCARVE-7 L2 repair authoritatively
    // re-maps any punched fb block at map time, whatever the DTB called the node. So we keep only the
    // BOOT_INFO guard here and rely on the repair for the DTB-sourced case.
    let fb_lo = boot_info.framebuffer_addr;
    let fb_hi = fb_lo.wrapping_add(boot_info.framebuffer_size as u64);
    let hits_fb = |b: u64, s: u64| -> bool {
        fb_lo != 0 && boot_info.framebuffer_size != 0 && b < fb_hi && fb_lo < b.wrapping_add(s)
    };

    // Resolve one QUIRK PA's window: a DTB `/reserved-memory` node that COVERS `pa` (verbatim, source 1),
    // else the supplied conservatively-bounded fallback `(fb_base, fb_size)` (source 2).
    let resolve_quirk = |pa: u64, fb_base: u64, fb_size: u64| -> (u64, u64, u8) {
        for &(b, s) in carve.iter().take(nfdt) {
            if s != 0 && pa >= b && pa < b.wrapping_add(s) {
                return (b, s, 1);
            }
        }
        (fb_base, fb_size, 2)
    };

    // Slot 0: PRIMARY 0x26b9 window (XCARVE-9: 9.5 MiB QUIRK fallback — the undeclared gap between the
    // DTB's adjacent carveouts; boot-28 refuted the XCARVE-3 single-granule guess, see XCARVE_GIB9_SIZE).
    let target = crate::vugras::XCARVE_TARGET_PA as u64;
    debug_assert_eq!(target & !(BLK2 - 1), XCARVE_GIB9_BASE); // the widened window still contains the boot-21 PA's granule
    let (pb, ps, psrc) = resolve_quirk(target, XCARVE_GIB9_BASE, XCARVE_GIB9_SIZE);
    out[n] = (pb, ps, psrc);
    n += 1;

    // Slot 1: XCARVE-6/8 0xbe window (96 MiB QUIRK fallback — bounded GUESS extent, see XCARVE_BE_SIZE).
    let (bb, bs, bsrc) = resolve_quirk(XCARVE_BE_BASE, XCARVE_BE_BASE, XCARVE_BE_SIZE);
    if n < cap {
        out[n] = (bb, bs, bsrc);
        n += 1;
    } else {
        dropped += 1;
    }

    // Slot 2: XCARVE-10 upper GiB-9 window (4 MiB QUIRK fallback — the gap ABOVE the declared
    // [0x26c180000, 0x26c400000) carveout; boot-34-retry's 0x26c6be4a0 hit refuted the XCARVE-9
    // declared-neighbor cap, see XCARVE_GIB9B_SIZE).
    let (ub, us, usrc) = resolve_quirk(0x2_6c6b_e4a0, XCARVE_GIB9B_BASE, XCARVE_GIB9B_SIZE); // the boot-34-retry hit PA
    if n < cap {
        out[n] = (ub, us, usrc);
        n += 1;
    } else {
        dropped += 1;
    }

    // STRUCTURAL: every remaining DTB `/reserved-memory` carveout inside a RAM GiB (deduped, fb-skipped).
    for &(b, s) in carve.iter().take(nfdt) {
        if s == 0 || !in_ram_gib(b) || hits_fb(b, s) {
            continue;
        }
        if (b == pb && s == ps) || (b == bb && s == bs) || (b == ub && s == us) {
            continue; // already represented by a quirk slot
        }
        if n < cap {
            out[n] = (b, s, 1);
            n += 1;
        } else {
            dropped += 1;
        }
    }
    (n, dropped)
}

/// XCARVE-6: sub-divide every RAM GiB that contains ≥1 excluded window into its own L2 table (512 × 2 MiB),
/// mapping each block Normal-WB RAM for `el` EXCEPT blocks intersecting ANY window, which stay invalid
/// (unmapped), then repoint that GiB's L1 entry at the L2 table. Generalizes the XCARVE-3 single-hole
/// split: windows sharing a GiB share that GiB's L2 table (one `l2_pool` slot per distinct split GiB).
/// Only splits a GiB the base map holds as Normal-WB RAM (a Device/invalid GiB has nothing to protect).
/// XCARVE-8: a window is split in EVERY GiB its span `[hb, hb+hs)` touches, not just the GiB of its base —
/// the widened 96 MiB 0xbe QUIRK straddles the GiB 2/3 boundary, so base-GiB-only collection (the
/// XCARVE-6 shape) would leave the `[0xc0000000, 0xc4000000)` remainder whole+cacheable, exactly the
/// boot-27 defect. Each GiB's L2 punches only the intersecting blocks (the punch loop already
/// intersects per-block, so the per-GiB clip is implicit). Returns
/// `(l2_tables_used, gib_overflow)`: `gib_overflow` counts distinct split-needing GiBs beyond the pool —
/// those are left whole+cacheable and MUST be witnessed (no-silent-drop). Pool slots are cleaned to PoC by
/// the caller before the switch.
#[cfg(feature = "tegra")]
unsafe fn install_carveout_holes(
    l1: *mut u64,
    l2_pool: *mut PageTable,
    el: u64,
    holes: &[(u64, u64, u8)],
) -> (usize, usize) {
    const BLK2: u64 = 2 * 1024 * 1024;
    // Collect the distinct RAM GiBs that contain a window.
    let mut split_gib = [0u64; MAX_SPLIT_GIB];
    let mut used = 0usize;
    let mut overflow = 0usize;
    for &(hb, hs, _) in holes {
        if hs == 0 {
            continue;
        }
        // XCARVE-8 straddle fix: every GiB the window's span touches needs a split (the widened 0xbe
        // QUIRK crosses the GiB 2/3 boundary); base-GiB-only collection left the spill-over cacheable.
        let g_lo = hb >> 30;
        let g_hi = (hb + hs - 1) >> 30;
        for gib in g_lo..=g_hi {
            let gi = gib as usize;
            if gi >= 512 {
                continue;
            }
            let cur = unsafe { l1.add(gi).read_volatile() };
            if !is_ram_block(cur) {
                continue; // not Normal-WB RAM — nothing to protect (already unmapped/Device)
            }
            if split_gib[..used].iter().any(|&g| g == gib) {
                continue; // this GiB already has a pool slot
            }
            if used >= MAX_SPLIT_GIB {
                overflow += 1;
                continue;
            }
            split_gib[used] = gib;
            used += 1;
        }
    }
    // Build one L2 table per split GiB: a block is invalid iff it intersects ANY window (windows in other
    // GiBs never intersect this GiB's PA range, so only this GiB's windows punch it).
    for si in 0..used {
        let gib = split_gib[si];
        let gi = gib as usize;
        let gib_base = gib << 30;
        let l2 = unsafe { l2_pool.add(si) } as *mut u64;
        for i in 0..512usize {
            let blk_lo = gib_base + (i as u64) * BLK2;
            let blk_hi = blk_lo + BLK2;
            let mut punched = false;
            for &(hb, hs, _) in holes {
                if hs != 0 && blk_lo < hb.wrapping_add(hs) && hb < blk_hi {
                    punched = true;
                    break;
                }
            }
            let desc = if punched { 0 } else { ram_block(blk_lo, el) };
            unsafe {
                l2.add(i).write_volatile(desc);
            }
        }
        // Repoint the GiB: L1 now holds a TABLE descriptor (bits[1:0]=0b11) to the L2 table.
        unsafe {
            l1.add(gi).write_volatile((l2 as u64) | 0b11);
        }
    }
    (used, overflow)
}

// ── Translation attributes (ARM ARM DDI0487). ──────────────────────────────────────────────────────
// MAIR: AttrIdx 0 = Normal Inner/Outer Write-Back non-transient (0xFF); AttrIdx 1 = **Device-nGnRE**
// (0x04) — deliberately nGnRE for Tegra (early-write-ack tolerant), NOT the Pi's nGnRnE (0x00);
// AttrIdx 2 = **Normal Inner/Outer Non-Cacheable** (0x44) — the NET-4B DMA window (rings + RX/TX
// buffers), so the NIC's non-coherent descriptor fetches / payload writes see DRAM directly and no
// clean/invalidate race can ever mis-order against them (Linux r8169's dma_alloc_coherent config).
// Layout is regime-independent, so the same value programs MAIR_EL2 or MAIR_EL1.
const MAIR_VAL: u64 = 0x0044_04FF;

// TCR_EL2, non-VHE short format (E2H == 0) — the field layout DIFFERS from TCR_EL1; do not copy the
// EL1 recipe. 0x8081_3519 decodes to: T0SZ=25 [5:0]; IRGN0=WB (0b01) [9:8]; ORGN0=WB (0b01) [11:10];
// SH0=inner (0b11) [13:12]; TG0=4 KiB (0b00) [15:14]; PS=0b001 = 36-bit / 64 GiB [18:16] (covers Orin
// RAM to ~10 GiB); and the two RES1 bits at [23] and [31] that mark the non-VHE short format (TBI0=0,
// [20]). Hand-verified bit-by-bit against the field table.
const TCR_EL2_VAL: u64 = 0x8081_3519;

// TCR_EL1 for the fallback — the exact EL1&0 recipe from `boot.rs` (T0SZ=25, WB/WB, inner, 4 KiB, high
// half disabled via EPD1, IPS=36-bit at [34:32]). Kept separate because at EL1 PS is *IPS* at [34:32]
// and bits 23/31 are EPD1/TG1 — NOT the RES1 bits the non-VHE TCR_EL2 has.
const TCR_EL1_VAL: u64 = 25            // T0SZ  [5:0]
    | (0b01 << 8)                      // IRGN0 = WB
    | (0b01 << 10)                     // ORGN0 = WB
    | (0b11 << 12)                     // SH0   = inner shareable
    | (0b00 << 14)                     // TG0   = 4 KiB
    | (25 << 16)                       // T1SZ  [21:16] (TTBR1 unused; legal value)
    | (1 << 23)                        // EPD1  = disable the TTBR1 table walk
    | (0b10 << 30)                     // TG1   = 4 KiB (legal encoding; TTBR1 unused)
    | (0b001 << 32);                   // IPS   = 36-bit / 64 GiB, at [34:32]

// ORIN-NET-3 (M1, `pcie3`): the PS/IPS *output-address* field, widened from 0b001 (36-bit / 64 GiB)
// to 0b010 (40-bit / 1 TiB), knob-gated. NET-2 proved controller-0's ECAM (`0x2e_2000_0000`, ~184 GiB)
// and MMIO `ranges` (~200 GiB) sit ABOVE the 36-bit output ceiling, so `map_mmio_window` refused them;
// widening PS lets the MMU EMIT those output addresses (they still fall inside the 512-GiB / 39-bit VA
// the L1 table already spans — M1 flips ONLY the output field, not T0SZ, so no new table level is
// needed). At EL2 (non-VHE short format) PS is [18:16]; at EL1&0 IPS is [34:32]. The active values
// below are the ONLY thing the switch programs, so with `pcie3` OFF they fold to the exact NET-2
// literals and the emitted code (and the `mmu-regs` banner's `tcr=`) is byte-identical to baseline.
#[cfg(feature = "pcie3")]
const TCR_EL2_ACTIVE: u64 = (TCR_EL2_VAL & !(0b111 << 16)) | (0b010 << 16);
#[cfg(not(feature = "pcie3"))]
const TCR_EL2_ACTIVE: u64 = TCR_EL2_VAL;
#[cfg(feature = "pcie3")]
const TCR_EL1_ACTIVE: u64 = (TCR_EL1_VAL & !(0b111 << 32)) | (0b010 << 32);
#[cfg(not(feature = "pcie3"))]
const TCR_EL1_ACTIVE: u64 = TCR_EL1_VAL;

// SCTLR bits we toggle. We RMW the firmware's SCTLR (UEFI initialised it, RES1 bits already set) — bic
// M|C to turn the MMU + data cache OFF for the reprogram window, then orr M|C|I to turn MMU + data +
// instruction caches back on. Passed to the asm in registers, not as bitmask immediates (0x5 / 0x1005
// are not encodable AArch64 logical immediates).
const SCTLR_M: u64 = 1 << 0; // MMU enable
const SCTLR_C: u64 = 1 << 2; // data cache
const SCTLR_I: u64 = 1 << 12; // instruction cache

// L1 block-descriptor field bits (bits[1:0]=0b01 = block, AttrIndx[4:2], SH[9:8], AF=bit10).
const DESC_BLOCK: u64 = 0b01;
const DESC_AF: u64 = 1 << 10;
const SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 0 << 2; // MAIR AttrIdx 0
const ATTR_DEVICE: u64 = 1 << 2; // MAIR AttrIdx 1
// NET-4B: MAIR AttrIdx 2 = Normal Inner/Outer Non-Cacheable. Used only for the DMA window.
#[cfg(feature = "tegra")]
const ATTR_NC: u64 = 2 << 2;
// Execute-never. In the **EL2 non-VHE** single-privilege stage-1 regime XN is bit 54 ONLY (bit 53 is
// RES0 there); the UXN/PXN split is EL1&0-regime-specific.
const EL2_XN: u64 = 1 << 54;
// AP[1] (descriptor bit 6) is **RES1** in the EL2/EL3 single-privilege regime — set it in every EL2
// leaf. AP[2]=bit7 governs RW(0)/RO(1); we leave it 0 (read-write) everywhere.
const EL2_AP1_RES1: u64 = 1 << 6;
// EL1&0 fallback: a peripheral block must be non-executable at BOTH ELs, so set UXN|PXN (the Pi recipe,
// boot.rs:95-98). RAM at EL1 uses AP[7:6]=0b00 (EL1 RW, no EL0) and stays executable.
const EL1_UXN: u64 = 1 << 54;
const EL1_PXN: u64 = 1 << 53;

/// Normal, cacheable, inner-shareable, **executable** 1 GiB RAM block (kernel image / stack / heap).
#[inline]
fn ram_block(pa: u64, el: u64) -> u64 {
    if el == 2 {
        pa | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_BLOCK | EL2_AP1_RES1
    } else {
        // EL1&0 (Pi recipe): AP[7:6]=0b00 → EL1 read-write, no EL0, executable at EL1.
        pa | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_BLOCK
    }
}
/// NET-4B: Normal Inner/Outer **Non-Cacheable**, execute-never block (used at L2 = 2 MiB granularity for
/// the DMA window). Shareability is ignored for Normal-NC memory, so no SH field is set. Attribute index
/// 2 (`ATTR_NC`). EL2 sets AP[1] RES1 like every other EL2 leaf; EL1&0 sets UXN|PXN (DMA buffers are
/// never executed). The CPU accesses this window uncached, so the NIC (a non-coherent DMA master) and the
/// CPU share one view of DRAM with no cache-maintenance step between them.
#[cfg(feature = "tegra")]
#[inline]
fn nc_block(pa: u64, el: u64) -> u64 {
    if el == 2 {
        pa | EL2_XN | DESC_AF | ATTR_NC | DESC_BLOCK | EL2_AP1_RES1
    } else {
        pa | EL1_UXN | EL1_PXN | DESC_AF | ATTR_NC | DESC_BLOCK
    }
}
/// Device-nGnRE, execute-never 1 GiB block. No shareability field (ignored for Device memory).
#[inline]
fn device_block(pa: u64, el: u64) -> u64 {
    if el == 2 {
        pa | EL2_XN | DESC_AF | ATTR_DEVICE | DESC_BLOCK | EL2_AP1_RES1
    } else {
        pa | EL1_UXN | EL1_PXN | DESC_AF | ATTR_DEVICE | DESC_BLOCK
    }
}

/// What the banner (Part B) needs to print, plus the diagnostics that feed Part D / JM4.
pub struct MmuInfo {
    /// Exception level we programmed for (2 primary, 1 fallback).
    pub el: u64,
    /// Firmware's SCTLR before → our SCTLR after (RMW: `old | M|C|I`).
    pub sctlr_old: u64,
    pub sctlr_new: u64,
    pub tcr: u64,
    pub mair: u64,
    pub ttbr0: u64,
    /// Bit `i` set ⇔ GiB index `i` (`i * 1 GiB`) is mapped Normal-WB RAM. GiB 0 is always Device
    /// (never in this mask); the u64 width caps at GiB 63 (64 GiB, the PS ceiling) which is well above
    /// Orin RAM's ~10 GiB top.
    pub ram_gib_mask: u64,
    /// PA of the **EL1-precise** table (`L1_EL1`) for the JM6 drop's `TTBR0_EL1` — see the `L1_EL1`
    /// doc for why the live EL2 table cannot serve. On the EL1 fallback path (`el == 1`) `L1` itself
    /// was already built with the EL1 recipe, so this aliases `ttbr0`.
    pub ttbr0_el1: u64,
    /// XCARVE-3: base of the PRIMARY protected-carveout hole (the 0x26b9 window) excluded (unmapped) from
    /// the cacheable map, or `0` if none (always `0` on non-tegra). See `resolve_carveout_holes`.
    pub hole_base: u64,
    /// XCARVE-3: size in bytes of the PRIMARY excluded carveout hole, or `0` if none.
    pub hole_size: u64,
    /// XCARVE-3: source of the PRIMARY hole extent — `0` none/virt, `1` DTB `/reserved-memory` node, `2` a
    /// conservatively-bounded tegra QUIRK entry (the DTB did not declare a reservation over the PA).
    pub hole_source: u8,
    /// XCARVE-6: the FULL excluded-window set (arch-neutral copy for the boot banner — `crate::main`
    /// reads it without an arch-glob path). `holes[..hole_count]` are `(base, size, source)`. `0` count
    /// on non-tegra / when nothing is excluded. The authoritative set also lives in `carveout_holes`.
    pub holes: [(u64, u64, u8); MMU_MAX_HOLES],
    /// XCARVE-6: number of valid entries in `holes` (≤ `MMU_MAX_HOLES`).
    pub hole_count: usize,
    /// XCARVE-6/5 (no-silent-drop): windows that overflowed the resolve set (`MAX_HOLES`) plus distinct
    /// GiBs that overflowed the L2 pool (`MAX_SPLIT_GIB`) — both `0` in practice; non-zero means the
    /// exclusion set is INCOMPLETE and MUST be witnessed loudly by the caller.
    pub hole_dropped: usize,
}

/// XCARVE-6: banner-side copy width of the excluded-window set carried in `MmuInfo` (arch-neutral so
/// `crate::main`'s tegra banner needs no `arch::aarch64` path). Kept modest; the authoritative,
/// full-width set is `carveout_holes` (`MAX_HOLES`).
pub const MMU_MAX_HOLES: usize = 32;

/// Read `CurrentEL` (bits [3:2]). A pure system-register read — cannot fault, so it is always safe to
/// call on the still-active UEFI map before we touch any device MMIO.
#[inline]
fn current_el() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    (v >> 2) & 0b11
}

/// Install our own EL2 (or EL1) translation regime and return the banner/diagnostic snapshot. Silent:
/// prints nothing (the first serial byte of the whole kernel is the caller's post-switch banner, which
/// is only reachable once UARTC is mapped by the very switch this performs).
pub fn init(boot_info: &BootInfo) -> MmuInfo {
    let el = current_el();
    let (tcr, mair) = match el {
        2 => (TCR_EL2_ACTIVE, MAIR_VAL),
        1 => (TCR_EL1_ACTIVE, MAIR_VAL),
        // STOP tripwire (b): CurrentEL is neither 1 nor 2. We cannot safely program a translation
        // regime for an EL we do not understand, and we cannot report it (UARTC is unmapped until we
        // switch). Spin so the operator sees a dark hang that maps to "CurrentEL neither 1 nor 2"
        // rather than a wrong-regime crash — do not improvise a switch.
        _ => loop {
            core::hint::spin_loop();
        },
    };
    let ttbr0 = &raw const L1 as u64;
    // The EL1-precise twin only exists on the EL2-primary path; at the EL1 fallback `L1` itself was
    // built with the EL1 recipe and doubles as both.
    let ttbr0_el1 = if el == 2 { &raw const L1_EL1 as u64 } else { ttbr0 };
    let ram_gib_mask = unsafe { build_l1(boot_info, el) };
    // XCARVE-3/6 (tegra, always-on correctness — NOT knob-gated): punch every protected-carveout window out
    // of the cacheable map. Resolve the SET (the two QUIRKs 0x26b9/0xbe + every DTB `/reserved-memory`
    // carveout inside a RAM GiB), then sub-divide each affected GiB into an L2 table that leaves those
    // windows UNMAPPED in both the live EL2 table and the EL1-precise twin. Latch the set so
    // `select_heap_region` and the VUGRAS localizer exclude the SAME windows. On non-tegra the set is empty
    // and the map is byte-identical.
    #[cfg(feature = "tegra")]
    let (hole_base, hole_size, hole_source, holes_arr, hole_count, hole_dropped, split_used) = {
        use core::sync::atomic::Ordering;
        let mut set = [(0u64, 0u64, 0u8); MAX_HOLES];
        let (n, dropped_set) = resolve_carveout_holes(boot_info, ram_gib_mask, &mut set);
        let mut split_used = 0usize;
        let mut gib_overflow = 0usize;
        if n != 0 {
            unsafe {
                let (u, ov) = install_carveout_holes(
                    &raw mut L1 as *mut u64,
                    &raw mut L2_POOL as *mut PageTable,
                    el,
                    &set[..n],
                );
                split_used = u;
                gib_overflow = ov;
                if el == 2 {
                    install_carveout_holes(
                        &raw mut L1_EL1 as *mut u64,
                        &raw mut L2_POOL_EL1 as *mut PageTable,
                        1,
                        &set[..n],
                    );
                }
            }
        }
        // Latch the full set for `carveout_holes`, and the PRIMARY (slot 0 = 0x26b9) for `carveout_hole`.
        HOLE_N.store(n, Ordering::Relaxed);
        for i in 0..n {
            HOLES_BASE[i].store(set[i].0, Ordering::Relaxed);
            HOLES_SIZE[i].store(set[i].1, Ordering::Relaxed);
            HOLES_SRC[i].store(set[i].2, Ordering::Relaxed);
        }
        let (pb, psz, psrc) = set[0];
        HOLE_BASE.store(pb, Ordering::Relaxed);
        HOLE_SIZE.store(psz, Ordering::Relaxed);
        HOLE_SOURCE.store(psrc, Ordering::Relaxed);
        // Arch-neutral banner copy (≤ MMU_MAX_HOLES).
        let mut arr = [(0u64, 0u64, 0u8); MMU_MAX_HOLES];
        let bn = n.min(MMU_MAX_HOLES);
        arr[..bn].copy_from_slice(&set[..bn]);
        (pb, psz, psrc, arr, bn, dropped_set + gib_overflow, split_used)
    };
    #[cfg(not(feature = "tegra"))]
    let (hole_base, hole_size, hole_source, holes_arr, hole_count, hole_dropped) =
        (0u64, 0u64, 0u8, [(0u64, 0u64, 0u8); MMU_MAX_HOLES], 0usize, 0usize);
    unsafe { clean_table_to_poc(ttbr0) };
    // The L2 pool tables are walked by the table descriptors `install_carveout_holes` wrote into L1, so
    // each used slot must reach RAM before the data cache is dropped for the switch — clean like the L1s.
    #[cfg(feature = "tegra")]
    for si in 0..split_used {
        unsafe { clean_table_to_poc((&raw const L2_POOL as *const PageTable).add(si) as u64) };
    }
    if el == 2 {
        unsafe { clean_table_to_poc(ttbr0_el1) };
        #[cfg(feature = "tegra")]
        for si in 0..split_used {
            unsafe { clean_table_to_poc((&raw const L2_POOL_EL1 as *const PageTable).add(si) as u64) };
        }
    }
    let (sctlr_old, sctlr_new) = unsafe {
        if el == 2 { enable_el2(ttbr0) } else { enable_el1(ttbr0) }
    };
    // Now our own tables are live and RAM + the Tegra device window are mapped. Point the vector base
    // at our handler (Part C) so a subsequent fault is a recorded syndrome, not R4's dark hang under
    // UEFI's now-possibly-unmapped VBAR.
    unsafe { install_vectors(el) };
    MmuInfo {
        el,
        sctlr_old,
        sctlr_new,
        tcr,
        mair,
        ttbr0,
        ram_gib_mask,
        ttbr0_el1,
        hole_base,
        hole_size,
        hole_source,
        holes: holes_arr,
        hole_count,
        hole_dropped,
    }
}

/// Populate `L1`: `L1[0]` = the low-1-GiB Device window (covers UARTC 0x0C28_0000 and the Tegra234 GIC
/// region 0x0F40_0000 for JM4), every RAM GiB the firmware map names = a Normal-WB block, the rest
/// invalid. Returns the RAM-GiB mask. Runs on the still-active UEFI map (all RAM readable), MMU-on;
/// plain volatile writes only.
unsafe fn build_l1(boot_info: &BootInfo, el: u64) -> u64 {
    // 1. Derive the RAM GiBs from the firmware memory map — do NOT hardcode Orin's DRAM span. Usable
    //    (free conventional RAM) and Bootloader (LOADER_CODE/DATA — the loader + our own image) are
    //    RAM; Reserved is NOT mapped wholesale (on EDK2 it can include MMIO / runtime-services pages).
    let mut ram_gib_mask: u64 = 0;
    if boot_info.memory_regions_addr != 0 && boot_info.memory_regions_len != 0 {
        let regions = unsafe {
            core::slice::from_raw_parts(
                boot_info.memory_regions_addr as *const MemoryRegion,
                boot_info.memory_regions_len,
            )
        };
        for r in regions {
            let is_ram = matches!(r.kind, MemoryRegionKind::Usable | MemoryRegionKind::Bootloader);
            if !is_ram || r.page_count == 0 {
                continue;
            }
            let start = r.phys_start;
            let end = r.phys_start + r.page_count * 4096 - 1; // inclusive last byte
            let mut g = start >> 30;
            let g_hi = end >> 30;
            while g <= g_hi {
                // Never mark GiB 0 (that stays the Device window `L1[0]`); cap at the u64 mask width.
                if g >= 1 && g < 64 {
                    ram_gib_mask |= 1u64 << g;
                }
                g += 1;
            }
        }
    }

    // 2. Belt-and-braces: the GiB we are executing from and the GiB the live SP points into MUST be
    //    mapped RAM before the switch, whatever the firmware classified them as — otherwise the very
    //    first post-switch instruction fetch or stack access would fault. `adr .` is the exact current
    //    PC (this loaded image, identity VA==PA); `mov x, sp` the live stack. (R4 ground truth: image
    //    at GiB 9; Orin RAM = 0x8000_0000..0x2_8000_0000 = GiB 2..=9, so these normally just re-mark
    //    already-mapped GiBs — but if the map omitted them we would otherwise switch into a dark hang.)
    let code_va: u64;
    let sp_va: u64;
    unsafe {
        core::arch::asm!("adr {}, .", out(reg) code_va, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, sp", out(reg) sp_va, options(nomem, nostack, preserves_flags));
    }
    for va in [code_va, sp_va] {
        let g = va >> 30;
        if g >= 1 && g < 64 {
            ram_gib_mask |= 1u64 << g;
        }
    }

    // 2b. JM7 (video): the UEFI GOP framebuffer, when the firmware published one (monitor connected
    //     at boot; `framebuffer_addr == 0` on a headless boot — nothing changes there). The GOP
    //     carveout can sit in a Reserved region the RAM scan skipped, and fbcon keeps mirroring
    //     serial output onto it long after this switch — so its GiBs MUST be in the map (both
    //     tables — the EL1 twin is filled from this same mask) or the first post-switch mirror
    //     faults into the Part-C vector. Mapped Normal-WB like RAM (a 1 GiB block cannot carry a
    //     separate attribute without a new MAIR index); CPU-write → scanout coherency is fbcon's
    //     existing damage-tracked `flush_range` → `dc cvac` (the Pi-proven recipe), valid on WB.
    if boot_info.framebuffer_addr != 0 && boot_info.framebuffer_size != 0 {
        let fb_lo = boot_info.framebuffer_addr >> 30;
        let fb_hi = (boot_info.framebuffer_addr + boot_info.framebuffer_size as u64 - 1) >> 30;
        let mut g = fb_lo;
        while g <= fb_hi {
            if g >= 1 && g < 64 {
                ram_gib_mask |= 1u64 << g;
            }
            g += 1;
        }
    }

    // 3. Write all 512 L1 entries. L1[0] = Device (never overwritten by a RAM GiB); GiB 1
    //    (0x4000_0000, more Tegra peripherals) stays unmapped unless a firmware RAM region genuinely
    //    claimed it (the mask decides — we never blanket-map it Normal).
    unsafe { fill_table(&raw mut L1 as *mut u64, ram_gib_mask, el) };
    // 4. EL2-primary path only: build the EL1-precise twin for the JM6 drop — same GiB set, EL1&0
    //    leaf recipe (RAM AP[2:1]=0b00 EL1-executable, Device UXN|PXN). See the `L1_EL1` doc for why
    //    the live EL2 table must not serve as TTBR0_EL1 (AP[1] forces PXN under EL1&0). Built HERE
    //    (init) though the drop runs much later (after JM4 + memory::init): sound because the PC/SP
    //    GiBs re-marked in step 2 cannot move between init and the drop (the image is fixed and the
    //    boot stack is never switched on this path), and the heap memory::init later carves comes
    //    from Usable regions already in this same mask. A future arc that adds a mapping to the live
    //    EL2 `L1` (a new device window, a remap) MUST mirror it here or post-drop code loses it.
    if el == 2 {
        unsafe { fill_table(&raw mut L1_EL1 as *mut u64, ram_gib_mask, 1) };
    }
    ram_gib_mask
}

/// Write all 512 entries of one L1 table with the leaf recipe for `el` (2 = the single-privilege EL2
/// regime, 1 = EL1&0): entry 0 = the low-1-GiB Device window, each `ram_gib_mask` GiB = Normal-WB RAM,
/// the rest invalid. Volatile entry-by-entry writes (no dependence on the loader zeroing `.bss`).
unsafe fn fill_table(l1: *mut u64, ram_gib_mask: u64, el: u64) {
    unsafe {
        l1.add(0).write_volatile(device_block(0, el));
        for i in 1..512usize {
            let desc = if i < 64 && (ram_gib_mask >> i) & 1 != 0 {
                ram_block((i as u64) << 30, el)
            } else {
                0 // invalid: a stray access here faults into the Part-C vector, not silently succeeds
            };
            l1.add(i).write_volatile(desc);
        }
        // JB1: GiB 1 (0x4000_0000) carries the SYSRAM the BPMP IVC shmem lives in (TX 0x4007_0000 /
        // RX 0x4007_1000, read off the firmware DTB on silicon) plus further Tegra peripherals — map
        // it Device-nGnRE like GiB 0 unless the firmware genuinely declared RAM there. Device
        // memory is non-speculative, so this only reduces the stray-touch protection for EXPLICIT
        // accesses — and the JX1 lesson stands regardless: a gated block faults at EL3, mapped or
        // not, so the map was never the real guard.
        if (ram_gib_mask >> 1) & 1 == 0 {
            l1.add(1).write_volatile(device_block(1 << 30, el));
        }
    }
}

/// Clean one 4 KiB table page to the Point of Coherency (`dc cvac` every 64-byte line, then `dsb sy`).
/// The descriptors were written through UEFI's cacheable mapping, so they may sit in the data cache;
/// the table walker must see them in RAM once we drop the data cache for the reprogram window.
unsafe fn clean_table_to_poc(base: u64) {
    let mut off: u64 = 0;
    while off < 4096 {
        unsafe {
            core::arch::asm!("dc cvac, {}", in(reg) base + off, options(nostack, preserves_flags));
        }
        off += 64;
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// EL2 primary switch. ONE `asm!` block, no memory traffic inside (compiler-generated stack spills
/// *between* separate asm blocks would fault or corrupt while the data cache is off). The `in(reg)`
/// inputs are materialised into registers before the block, while caches are still on. Sequence
/// mirrors `boot::enable_mmu`'s one-block discipline: MMU off → reprogram MAIR/TCR/TTBR0 → invalidate
/// → MMU + caches on. Returns `(firmware SCTLR, our SCTLR)`.
unsafe fn enable_el2(ttbr0: u64) -> (u64, u64) {
    let sctlr_old: u64;
    let sctlr_new: u64;
    unsafe {
        core::arch::asm!(
            "mrs {old}, SCTLR_EL2",       // firmware SCTLR (RES1 bits already set — safe to RMW)
            "bic {tmp}, {old}, {mc}",     // clear M|C: MMU + data cache OFF
            "msr SCTLR_EL2, {tmp}",
            "isb",                        // MMU off; PC keeps fetching from the same PA (UEFI identity)
            "msr MAIR_EL2, {mair}",
            "msr TCR_EL2, {tcr}",
            "msr TTBR0_EL2, {ttbr}",
            "tlbi alle2",                 // drop every stale EL2 TLB entry before translation resumes
            "dsb sy",
            "isb",
            "orr {new}, {old}, {mci}",    // set M|C|I on the ORIGINAL firmware value
            "msr SCTLR_EL2, {new}",
            "isb",                        // our tables live: RAM Normal-WB + Tegra Device-nGnRE mapped
            old = out(reg) sctlr_old,
            new = out(reg) sctlr_new,
            tmp = out(reg) _,
            mc = in(reg) SCTLR_M | SCTLR_C,
            mci = in(reg) SCTLR_M | SCTLR_C | SCTLR_I,
            mair = in(reg) MAIR_VAL,
            tcr = in(reg) TCR_EL2_ACTIVE,
            ttbr = in(reg) ttbr0,
            options(nostack, preserves_flags),
        );
    }
    (sctlr_old, sctlr_new)
}

/// EL1&0 fallback switch. Same shape with the EL1 register set (MAIR_EL1/TCR_EL1/TTBR0_EL1,
/// `tlbi vmalle1`). If we are at EL1 the firmware ran the EL1 MMU on before handoff, so SCTLR_EL1 is
/// initialised → RMW is safe (unlike the Pi bare-metal cold-reset path, where SCTLR_EL1 resets UNKNOWN
/// and must be an absolute value). TTBR1_EL1 is zeroed defensively (EPD1=1 already disables its walk).
unsafe fn enable_el1(ttbr0: u64) -> (u64, u64) {
    let sctlr_old: u64;
    let sctlr_new: u64;
    unsafe {
        core::arch::asm!(
            "mrs {old}, SCTLR_EL1",
            "bic {tmp}, {old}, {mc}",
            "msr SCTLR_EL1, {tmp}",
            "isb",
            "msr MAIR_EL1, {mair}",
            "msr TCR_EL1, {tcr}",
            "msr TTBR0_EL1, {ttbr}",
            "msr TTBR1_EL1, xzr",
            "tlbi vmalle1",
            "dsb sy",
            "isb",
            "orr {new}, {old}, {mci}",
            "msr SCTLR_EL1, {new}",
            "isb",
            old = out(reg) sctlr_old,
            new = out(reg) sctlr_new,
            tmp = out(reg) _,
            mc = in(reg) SCTLR_M | SCTLR_C,
            mci = in(reg) SCTLR_M | SCTLR_C | SCTLR_I,
            mair = in(reg) MAIR_VAL,
            tcr = in(reg) TCR_EL1_ACTIVE,
            ttbr = in(reg) ttbr0,
            options(nostack, preserves_flags),
        );
    }
    (sctlr_old, sctlr_new)
}

/// Point the vector base at our own EL2 (or EL1) table + `isb`. Done immediately after the switch: our
/// tables may not map UEFI's VBAR code, so without this a post-switch fault would hang dark instead of
/// reaching the Part-C handler.
unsafe fn install_vectors(el: u64) {
    unsafe {
        if el == 2 {
            let vbar = &raw const tegra_vectors_el2 as u64;
            core::arch::asm!("msr VBAR_EL2, {}", "isb", in(reg) vbar, options(nostack, preserves_flags));
        } else {
            let vbar = &raw const tegra_vectors_el1 as u64;
            core::arch::asm!("msr VBAR_EL1, {}", "isb", in(reg) vbar, options(nostack, preserves_flags));
        }
    }
}

/// Arm VBAR_EL1 at the Part-C EL1 fault vector while still at EL2 (non-VHE, so the EL1-banked write is
/// genuine — no E2H redirection). Called just before the JM6 drop: if anything still aborts on the EL1
/// landing, the vector — now in EL1-executable RAM under `L1_EL1` — prints a syndrome through the
/// mapped UARTC instead of hanging dark. `exceptions::install` replaces it right after the landing.
pub unsafe fn arm_el1_fault_vector() {
    unsafe {
        let vbar = &raw const tegra_vectors_el1 as u64;
        core::arch::asm!("msr VBAR_EL1, {}", "isb", in(reg) vbar, options(nostack, preserves_flags));
    }
}

/// Clean one translation descriptor's cache line to the Point of Coherency so the table walker sees
/// the new value once the data cache is bypassed. Ordering (the `dsb`) is done once by the caller.
#[inline]
unsafe fn clean_desc(addr: u64) {
    unsafe {
        core::arch::asm!("dc cvac, {}", in(reg) addr, options(nostack, preserves_flags));
    }
}

/// A valid Normal-WB 1 GiB block: bits[1:0]=0b01 (block) AND AttrIdx (bits[4:2]) == 0 (MAIR Normal).
#[inline]
fn is_ram_block(desc: u64) -> bool {
    desc & 0b11 == DESC_BLOCK && desc & (0b111 << 2) == ATTR_NORMAL
}

/// A TABLE descriptor (bits[1:0]=0b11): points to a next-level table. XCARVE-3 installs one for the GiB
/// it sub-divides to punch the carveout hole; that table maps every 2 MiB block Normal-WB RAM *except* the
/// protected window, so `map_fb_region`/`map_mmio_window` must treat such a GiB as already-covering RAM
/// and NOT overwrite its L1 entry with a 1 GiB block (which would re-map the hole). The framebuffer
/// carveout shares GiB 9 with the hole on the Orin, so this guard is load-bearing, not defensive.
#[inline]
fn is_table_desc(desc: u64) -> bool {
    desc & 0b11 == 0b11
}

/// XCARVE-7: does any QUIRK (source-2) protected window intersect the 2 MiB block `[blk_lo, blk_hi)`?
/// The fb L2 repair re-maps a punched block ONLY when the answer is no — a QUIRK window is a real
/// no-touch firewall carveout (0x26b9, 0xbe), and a framebuffer overlapping one is a genuine conflict the
/// repair must refuse and surface, not silently re-map. DTB (source-1) windows are NOT a bar: the fb
/// carveout is itself published as a `/reserved-memory` node, and re-mapping that node's block IS the fix.
#[cfg(feature = "tegra")]
fn quirk_hits_block(blk_lo: u64, blk_hi: u64) -> bool {
    use core::sync::atomic::Ordering;
    let n = HOLE_N.load(Ordering::Relaxed).min(MAX_HOLES);
    for i in 0..n {
        if HOLES_SRC[i].load(Ordering::Relaxed) != 2 {
            continue;
        }
        let b = HOLES_BASE[i].load(Ordering::Relaxed);
        let s = HOLES_SIZE[i].load(Ordering::Relaxed);
        if s != 0 && blk_lo < b.wrapping_add(s) && b < blk_hi {
            return true;
        }
    }
    false
}

/// XCARVE-7: repair the carveout-hole L2 split of one GiB so the framebuffer span is mapped. `l1_desc`
/// is the live GiB's L1 TABLE descriptor (its low bits carry the L2 table PA, identity VA==PA). Walks the
/// 512 × 2 MiB blocks; for every block that intersects the fb span `[pa, pa+size)` and is currently
/// INVALID (punched by exclusion), re-maps it Normal-WB RAM (attr for `el`) in the live L2 and — when
/// `patch_twin` — in the EL1 twin's own L2 (attr EL1), with `clean_desc` per written entry. Blocks
/// outside the span, and blocks already RAM, are untouched. A block intersecting a QUIRK (source-2)
/// protected window is REFUSED (left unmapped) and witnessed loudly — fb spans and firewall windows must
/// be disjoint; an overlap is a real conflict to surface. Returns the number of blocks re-mapped, and
/// witnesses `fb L2 repair — N block(s) re-mapped in GiB g [span)` when that is non-zero. The caller's
/// trailing `dsb sy` + TLBI publish the writes for the ACTIVE regime.
#[cfg(feature = "tegra")]
unsafe fn repair_fb_l2(
    l1_desc: u64,
    l1_el1: *mut u64,
    gi: usize,
    patch_twin: bool,
    pa: u64,
    size: usize,
    el: u64,
    gib: u64,
) -> usize {
    const BLK2: u64 = 2 * 1024 * 1024;
    // A table descriptor's next-level table PA is bits [47:12]; the pool tables are 4 KiB aligned and we
    // wrote no upper attribute bits, but mask precisely rather than assume.
    const TABLE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;
    let l2 = (l1_desc & TABLE_ADDR_MASK) as *mut u64;
    // The EL1 twin's L2 for this GiB, if the twin exists and is itself an L2 split.
    let l2_el1: *mut u64 = if patch_twin {
        let d1 = unsafe { l1_el1.add(gi).read_volatile() };
        if is_table_desc(d1) {
            (d1 & TABLE_ADDR_MASK) as *mut u64
        } else {
            core::ptr::null_mut()
        }
    } else {
        core::ptr::null_mut()
    };
    let fb_lo = pa;
    let fb_hi = pa + size as u64;
    let gib_base = gib << 30;
    let mut repaired = 0usize;
    for i in 0..512usize {
        let blk_lo = gib_base + (i as u64) * BLK2;
        let blk_hi = blk_lo + BLK2;
        if !(blk_lo < fb_hi && fb_lo < blk_hi) {
            continue; // block does not intersect the fb span — leave excluded
        }
        if quirk_hits_block(blk_lo, blk_hi) {
            // fb span overlaps a QUIRK firewall window in this block — a genuine conflict. Refuse the
            // repair for this block (stays unmapped) and surface it; do NOT re-map into a protected window.
            serial_println!(
                ":: tegra: XCARVE-7 CONFLICT — fb span [{:#x},{:#x}) overlaps QUIRK window in block [{:#x},{:#x}); repair REFUSED (block left unmapped) ::",
                fb_lo, fb_hi, blk_lo, blk_hi,
            );
            continue;
        }
        let cur = unsafe { l2.add(i).read_volatile() };
        if is_ram_block(cur) {
            continue; // already Normal-WB RAM — nothing to repair
        }
        unsafe {
            l2.add(i).write_volatile(ram_block(blk_lo, el));
            clean_desc(l2.add(i) as u64);
        }
        if !l2_el1.is_null() {
            let cur1 = unsafe { l2_el1.add(i).read_volatile() };
            if !is_ram_block(cur1) {
                unsafe {
                    l2_el1.add(i).write_volatile(ram_block(blk_lo, 1));
                    clean_desc(l2_el1.add(i) as u64);
                }
            }
        }
        repaired += 1;
    }
    if repaired > 0 {
        serial_println!(
            ":: tegra: fb L2 repair — {} block(s) re-mapped in GiB {} [{:#x},{:#x}) ::",
            repaired, gib, fb_lo, fb_hi,
        );
    }
    repaired
}

/// JD1 (video): map the inherited firmware scanout framebuffer's GiB span Normal-WB into BOTH live
/// translation tables — the running EL2 `L1` and the EL1-precise twin `L1_EL1` — so the CPU can draw
/// into the firmware's live scanout and the panel keeps working across the JM6 EL2 -> EL1 drop.
///
/// Called from `tegra_early_stop` AFTER `init` (the MMU is live, so the survey could read the
/// nvdisplay scanout base through the GiB-0 device window) but BEFORE the drop. Most scanout
/// carveouts already fall inside a firmware-declared RAM GiB (mapped at `init` step 1) — for those
/// this is a no-op that just confirms the twin agrees. It only *adds* a GiB when the carveout sits
/// in a Reserved region the RAM scan skipped. Whole 1 GiB blocks (the L1 granularity), Normal-WB
/// like RAM: the DC scans DRAM and does not snoop the CPU cache, so CPU-write -> scanout coherency
/// rides fbcon's existing `flush_range` -> `dc cvac` (the Pi-HVS recipe), valid on Normal-WB.
///
/// Returns `true` iff every GiB spanned by `[pa, pa+size)` is mapped Normal-WB afterwards. Refuses
/// GiB 0/1 (the Device / SYSRAM windows) and GiB >= 64 (beyond the 36-bit IPS ceiling): a scanout
/// base there is a survey misread, not a framebuffer, and the caller must then skip the blit.
pub fn map_fb_region(pa: u64, size: usize) -> bool {
    if pa == 0 || size == 0 {
        return fb_map_refuse("zero base or zero length — not a mappable scanout; base / len", pa, size as u64);
    }
    let g_lo = pa >> 30;
    let g_hi = (pa + size as u64 - 1) >> 30;
    // A scanout framebuffer lives in DRAM (Orin: 0x8000_0000.., GiB 2 upward). GiB 0 is the Tegra
    // Device window and GiB 1 the SYSRAM/peripheral window — never a framebuffer.
    if g_lo < 2 || g_hi >= 64 {
        return fb_map_refuse("span outside the DRAM GiB range 2..63 (GiB lo / GiB hi)", g_lo, g_hi);
    }
    let el = current_el();
    let l1 = &raw mut L1 as *mut u64;
    // The EL1 twin only exists on the EL2-primary path; on the EL1 fallback `L1` itself carries the
    // EL1 recipe and doubles as both, so there is nothing separate to patch there.
    let patch_twin = el == 2;
    let l1_el1 = &raw mut L1_EL1 as *mut u64;
    let mut changed = false;
    let mut all_ok = true;
    for g in g_lo..=g_hi {
        let gi = g as usize;
        let cur = unsafe { l1.add(gi).read_volatile() };
        // XCARVE-3/7: a TABLE descriptor is our carveout-hole L2 split — it maps every 2 MiB block of the
        // GiB Normal-WB EXCEPT the protected window(s), which stay invalid. Overwriting it with a 1 GiB
        // block would re-map those holes, so we never do. XCARVE-3 assumed no framebuffer ever lives in a
        // punched block — boot-26 falsified that: the DTB published the scanout carveout as a
        // `/reserved-memory` node AND `BOOT_INFO.framebuffer_addr == 0` this boot (the "booting without a
        // display" loader path), so the exclusion-time fb guard (`hits_fb`) never matched and the fb's own
        // 2 MiB block was punched; the first scanout write faulted (FAR=0x279e00000, translation L2). The
        // authoritative fix is here: walk this GiB's L2 and re-map every INVALID 2 MiB block intersecting
        // the fb span `[pa, pa+size)` back to Normal-WB RAM (live + EL1 twin, `clean_desc` per entry).
        // Blocks outside the span stay untouched, so the protected windows stay excluded; a block that
        // ALSO intersects a QUIRK protected window is a genuine conflict — refused and witnessed loudly,
        // never papered over. `map_mmio_window`'s Device path keeps the old skip (MMIO never lives in a
        // RAM-split GiB).
        #[cfg(feature = "tegra")]
        if is_table_desc(cur) {
            let n = unsafe { repair_fb_l2(cur, l1_el1, gi, patch_twin, pa, size, el, g) };
            if n > 0 {
                changed = true;
            }
            // The GiB stays a valid TABLE descriptor (still covering RAM), so `all_ok` holds.
            let after = unsafe { l1.add(gi).read_volatile() };
            all_ok &= is_ram_block(after) || is_table_desc(after);
            continue;
        }
        #[cfg(not(feature = "tegra"))]
        if is_table_desc(cur) {
            continue;
        }
        if !is_ram_block(cur) {
            // Invalid (a Reserved-only GiB the RAM scan skipped): map it Normal-WB in the live table.
            unsafe {
                l1.add(gi).write_volatile(ram_block(g << 30, el));
                clean_desc(l1.add(gi) as u64);
            }
            changed = true;
        }
        // Keep the EL1 twin in lock-step so the mapping survives the JM6 drop.
        if patch_twin {
            let cur1 = unsafe { l1_el1.add(gi).read_volatile() };
            if !is_ram_block(cur1) && !is_table_desc(cur1) {
                unsafe {
                    l1_el1.add(gi).write_volatile(ram_block(g << 30, 1));
                    clean_desc(l1_el1.add(gi) as u64);
                }
                changed = true;
            }
        }
        let after = unsafe { l1.add(gi).read_volatile() };
        // A GiB that is STILL neither a RAM block nor an L2 split after the patch above is the one
        // case whose failure the caller's generic line mis-names ("not DRAM GiB 2..63" when the span
        // is squarely inside it) — name the GiB and the descriptor that refused.
        if !(is_ram_block(after) || is_table_desc(after)) {
            fb_map_gib_refused(g, after);
        }
        all_ok &= is_ram_block(after) || is_table_desc(after);
    }
    if changed {
        // Publish the new descriptors and drop stale TLB state for the ACTIVE regime so the very
        // next access (the JD1 blit) walks the fresh map. The `dsb sy` also orders the `dc cvac`s
        // above. On the EL2-primary path the EL1 twin needs no TLBI here — the drop's own
        // `tlbi vmalle1` (boot_tegra::enable_el1_regime) flushes EL1&0 before that regime is armed,
        // and the twin is not the active table until then. (`tlbi alle2` would trap at EL1, so the
        // TLBI must match the regime we are actually running.)
        unsafe {
            if el == 2 {
                core::arch::asm!("dsb sy", "tlbi alle2", "dsb sy", "isb", options(nostack, preserves_flags));
            } else {
                core::arch::asm!("dsb sy", "tlbi vmalle1", "dsb sy", "isb", options(nostack, preserves_flags));
            }
        }
    }
    all_ok
}

/// NET-4B: map the reserved Normal-NC DMA window (`net4b_nc_window`) as Normal Non-Cacheable in the LIVE
/// translation tables. The window sits in GiB 9, which `install_carveout_holes` ALWAYS L2-splits (the
/// XCARVE-9/10/11 GiB-9 QUIRK windows are unconditional on tegra), so its GiB's L1 entry is a TABLE
/// descriptor and the covering 2 MiB block is one L2 entry currently mapped Normal-WB RAM (the block is
/// carveout-clean and outside the heap span by construction — `select_heap_region` reserves it). We flip
/// that block to Normal-NC in the ACTIVE regime's L2 and — before the JM6 EL2→EL1 drop — the EL1-precise
/// twin's L2, clean each written descriptor to PoC, then TLBI the active regime. A clean+invalidate over
/// the window's PA range precedes the flip so no stale Write-Back line survives the cacheability change to
/// later evict over the NIC's DMA (the break-before-make hygiene for a WB→NC attribute change on RAM the
/// kernel has not written). Returns true iff the window is mapped NC; false (fail-closed) if the window is
/// unreserved or its GiB is unexpectedly not split — the driver then REFUSES rather than DMA cacheable.
#[cfg(feature = "tegra")]
pub fn install_net4b_nc() -> bool {
    let (base, size) = net4b_nc_window();
    if base == 0 || size == 0 {
        serial_println!(
            ":: tegra: NET-4B — no NC DMA window reserved (select_heap_region fail-closed or non-tegra); cannot map NC ::"
        );
        return false;
    }
    const BLK2: u64 = 2 * 1024 * 1024;
    const TABLE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;
    let el = current_el();
    let patch_twin = el == 2;
    let l1 = &raw mut L1 as *mut u64;
    let l1_el1 = &raw mut L1_EL1 as *mut u64;
    // Drop any resident Write-Back line for the window before the attribute change (the block is
    // untouched by the kernel, but firmware may have; `dc civac` writes back any dirty line then drops
    // it so nothing can later evict over the NIC's DMA). Closes with the flip's own `dsb`.
    {
        let mut off = 0u64;
        while off < size {
            unsafe {
                core::arch::asm!("dc civac, {}", in(reg) base + off, options(nostack, preserves_flags));
            }
            off += 64;
        }
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }
    let g_lo = base >> 30;
    let g_hi = (base + size - 1) >> 30;
    let mut ok = true;
    for g in g_lo..=g_hi {
        let gi = g as usize;
        let d = unsafe { l1.add(gi).read_volatile() };
        if !is_table_desc(d) {
            serial_println!(
                ":: tegra: NET-4B — GiB {} is not L2-split (L1 desc={:#x}); cannot map the NC window — REFUSING ::",
                g, d
            );
            ok = false;
            continue;
        }
        let l2 = (d & TABLE_ADDR_MASK) as *mut u64;
        let l2_el1: *mut u64 = if patch_twin {
            let d1 = unsafe { l1_el1.add(gi).read_volatile() };
            if is_table_desc(d1) {
                (d1 & TABLE_ADDR_MASK) as *mut u64
            } else {
                core::ptr::null_mut()
            }
        } else {
            core::ptr::null_mut()
        };
        let gib_base = g << 30;
        for i in 0..512usize {
            let blk_lo = gib_base + (i as u64) * BLK2;
            let blk_hi = blk_lo + BLK2;
            if blk_lo < base + size && base < blk_hi {
                unsafe {
                    l2.add(i).write_volatile(nc_block(blk_lo, el));
                    clean_desc(l2.add(i) as u64);
                }
                if !l2_el1.is_null() {
                    unsafe {
                        l2_el1.add(i).write_volatile(nc_block(blk_lo, 1));
                        clean_desc(l2_el1.add(i) as u64);
                    }
                }
            }
        }
    }
    // Publish the new descriptors + drop stale TLB state for the ACTIVE regime (the `dsb sy` also orders
    // the `dc cvac`s above). Mirrors `map_fb_region`: the EL1 twin needs no TLBI here — the JM6 drop's own
    // `tlbi vmalle1` flushes EL1&0 before that regime is armed.
    unsafe {
        if el == 2 {
            core::arch::asm!("dsb sy", "tlbi alle2", "dsb sy", "isb", options(nostack, preserves_flags));
        } else {
            core::arch::asm!("dsb sy", "tlbi vmalle1", "dsb sy", "isb", options(nostack, preserves_flags));
        }
    }
    if ok {
        serial_println!(
            ":: tegra: NET-4B — DMA window [{:#x}, {:#x}) ({} KiB) mapped Normal-NC (MAIR AttrIdx 2) in {} table(s) ::",
            base,
            base + size,
            size >> 10,
            if patch_twin { "EL2 + EL1-twin" } else { "EL1" }
        );
    }
    ok
}

/// A valid Device-nGnRE 1 GiB block: bits[1:0]=0b01 (block) AND AttrIdx (bits[4:2]) == 1 (MAIR Device).
#[cfg(feature = "pcie2")]
#[inline]
fn is_device_block(desc: u64) -> bool {
    desc & 0b11 == DESC_BLOCK && desc & (0b111 << 2) == ATTR_DEVICE
}

/// ORIN-NET-2 (`pcie2`): the outcome of trying to reach an MMIO aperture at EL2 for a read-only walk.
#[cfg(feature = "pcie2")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MmioMap {
    /// The aperture's GiB(s) are already Device-nGnRE (the GiB-0 low peripheral window or the GiB-1
    /// SYSRAM/peripheral window filled at `init`) — no descriptor written; read directly.
    AlreadyMapped,
    /// A new Device-nGnRE 1 GiB block was installed in both live tables to reach the aperture.
    Mapped,
    /// The aperture's PA is at/above the tegra regime's reachable ceiling and cannot be mapped by an
    /// L1 block. Two limits bound reachability (see `map_mmio_window`): the **TCR PS/IPS output
    /// ceiling** (NET-2: 0b001 = 36-bit / 64 GiB; NET-3 `pcie3` widens it to 0b010 = 40-bit / 1 TiB)
    /// and the **L1 table's VA extent** (512 entries × 1 GiB = 512 GiB, unchanged by the PS widen).
    /// A base at/above the tighter of the two would raise an address-size fault or index past the
    /// table, so `map_mmio_window` refuses it (no descriptor written). Un-walked; caller records it.
    BeyondPsCeiling,
}

/// The tegra regime's TCR PS/IPS *output-address* ceiling in GiB. NET-2: 0b001 = 36-bit = 64 GiB.
/// NET-3 (`pcie3`, M1) widens PS to 0b010 = 40-bit = 1 TiB so the MMU may EMIT controller-0's ECAM /
/// MMIO output addresses (~184–212 GiB). Kept in lock-step with `TCR_EL2_ACTIVE` [18:16] / the EL1
/// twin's IPS [34:32]; with `pcie3` OFF it stays 64, so a `pcie2`-only build refuses exactly what
/// NET-2 refused (no behaviour change).
#[cfg(feature = "pcie2")]
const PS_OUTPUT_CEILING_GIB: u64 = if cfg!(feature = "pcie3") { 1024 } else { 64 };

/// The L1 translation table's VA reach: 512 entries × 1 GiB (T0SZ=25, 39-bit VA — see `TCR_EL2_VAL`).
/// The M1 PS widen flips only the *output* field, NOT T0SZ, so the table still spans 512 GiB and
/// `l1.add(gi)` is only in bounds for `gi < 512`. This is the ARRAY-SAFETY guard: `map_mmio_window`
/// must refuse `gib >= 512` regardless of the (wider) PS output ceiling, or it would index past the
/// static table. Every controller-0 aperture (ECAM ~184 GiB, MMIO ~200–212 GiB) sits below it, so
/// after the widen the 512-GiB extent — not the 1-TiB PS ceiling — is the binding limit in practice.
#[cfg(feature = "pcie2")]
const L1_GIB_EXTENT: u64 = 512;

/// ORIN-NET-2 (`pcie2`): reach an MMIO aperture `[pa, pa+size)` for a **READ-ONLY** walk, mapped
/// Device-nGnRE, via the EXISTING kernel page-table path — the same L1-block mechanism `map_fb_region`
/// uses, but Device instead of Normal-WB and idempotent against the already-mapped peripheral windows.
/// This is the ONLY write ORIN-NET-2 performs, and it is only ever a page-table descriptor: no fabric,
/// config, BAR, or system-register write (a TCR.PS widen would be needed for a beyond-ceiling aperture,
/// and this function REFUSES that rather than perform it — see `BeyondPsCeiling`).
///
/// Whole-GiB granularity (the L1 block size): a small aperture just marks its containing GiB Device.
/// Device memory is non-speculative, so marking the GiB only enables EXPLICIT accesses — the caller
/// reads solely the bounded aperture it asked for. Patches BOTH the live EL2 `L1` and the EL1-precise
/// twin `L1_EL1` (like `map_fb_region`) so the window survives a later JM6 EL2→EL1 drop, then flushes
/// the ACTIVE regime's TLB. Idempotent: a GiB already Device (GiB-0/1) returns `AlreadyMapped` without
/// touching the tables. On the EL1-fallback path `L1` itself carries the EL1 recipe (no separate twin).
#[cfg(feature = "pcie2")]
pub fn map_mmio_window(pa: u64, size: usize) -> MmioMap {
    if pa == 0 || size == 0 {
        // A zero base/size is an absent or unresolvable aperture, not a mappable window.
        return MmioMap::BeyondPsCeiling;
    }
    let g_lo = pa >> 30;
    let g_hi = (pa + size as u64 - 1) >> 30;
    // Refuse anything the regime cannot reach: at/above the TCR PS/IPS output ceiling (an L1 block
    // there raises an address-size fault) OR at/above the 512-entry L1 table's VA extent (indexing
    // there walks past the static table). `L1_GIB_EXTENT` is the load-bearing array-safety guard —
    // the M1 PS widen raises the OUTPUT ceiling to 1 TiB but leaves the table at 512 GiB, so 512 is
    // the binding limit after the widen (and every controller-0 aperture is below it). With `pcie3`
    // OFF the output ceiling is 64 GiB, so a `pcie2`-only build refuses exactly what NET-2 refused.
    if g_hi >= PS_OUTPUT_CEILING_GIB || g_hi >= L1_GIB_EXTENT {
        return MmioMap::BeyondPsCeiling;
    }
    let el = current_el();
    let l1 = &raw mut L1 as *mut u64;
    // The EL1 twin only exists on the EL2-primary path; on the EL1 fallback `L1` doubles as both.
    let patch_twin = el == 2;
    let l1_el1 = &raw mut L1_EL1 as *mut u64;
    let mut changed = false;
    let mut all_already = true;
    for g in g_lo..=g_hi {
        let gi = g as usize;
        let cur = unsafe { l1.add(gi).read_volatile() };
        // XCARVE-3: never overwrite a carveout-hole L2 split (a TABLE descriptor) with a Device block —
        // that GiB is RAM (minus the protected window). MMIO apertures live at GiB 0/1 or high PA, never
        // in the RAM-split GiB, so this is a guard against a survey misread, not an expected path.
        if is_table_desc(cur) {
            continue;
        }
        if !is_device_block(cur) {
            unsafe {
                l1.add(gi).write_volatile(device_block(g << 30, el));
                clean_desc(l1.add(gi) as u64);
            }
            changed = true;
            all_already = false;
        }
        if patch_twin {
            let cur1 = unsafe { l1_el1.add(gi).read_volatile() };
            if !is_device_block(cur1) && !is_table_desc(cur1) {
                unsafe {
                    l1_el1.add(gi).write_volatile(device_block(g << 30, 1));
                    clean_desc(l1_el1.add(gi) as u64);
                }
                changed = true;
            }
        }
    }
    if changed {
        // Publish the new descriptors + drop stale TLB state for the ACTIVE regime (the `dsb sy` also
        // orders the `dc cvac`s above). The EL1 twin needs no TLBI here — the JM6 drop's own
        // `tlbi vmalle1` flushes EL1&0 before that regime is armed. Mirrors `map_fb_region`.
        unsafe {
            if el == 2 {
                core::arch::asm!("dsb sy", "tlbi alle2", "dsb sy", "isb", options(nostack, preserves_flags));
            } else {
                core::arch::asm!("dsb sy", "tlbi vmalle1", "dsb sy", "isb", options(nostack, preserves_flags));
            }
        }
    }
    if all_already {
        MmioMap::AlreadyMapped
    } else {
        MmioMap::Mapped
    }
}

// ── Part C: bounded fault visibility ────────────────────────────────────────────────────────────────
//
// A minimal, inert-until-it-fires EL2/EL1 vector table. Two 2 KiB-aligned tables (16 entries each, at
// the architectural 0x80 spacing) each funnel to a stub that captures the EL's syndrome/fault-address/
// return-address and tail-calls the shared Rust printer, which prints one line and spins. This turns a
// dark post-switch metal hang into a recorded syndrome. Deliberately does not touch `exceptions.rs`.
//
// JB1f shrank this table's watch to the switch itself: `tegra_early_stop` installs the full healed
// `exceptions.rs` vectors right after the mmu-regs banner, so Part C covers only the silent `init`
// internals plus the first three serial lines. The 2026-07-11 bench proved why the wider window was
// untenable — the A78AE-1941500 phantom struck fbcon's glyph loop (the boot's heaviest ifetch+store
// stretch) under THIS divergent handler, twice, where the JB1e heal would have retried straight
// through. Keep Part C divergent (probe-and-spin): anything that faults inside the switch window has
// no heal-safe context to return to, and the syndrome+spin IS the design.

unsafe extern "C" {
    static tegra_vectors_el2: u8;
    static tegra_vectors_el1: u8;
}

core::arch::global_asm!(
    r#"
    .section .text
    // EL2 vector table: 16 entries at 0x80 spacing, 2 KiB-aligned for VBAR_EL2. Each entry
    // loads its own INDEX (JB1b lesson: an async entry — IRQ 5 / FIQ 6 — does NOT write ESR, so
    // a funnel that prints a stale ESR looks exactly like an impossible synchronous fault; the
    // index disambiguates). Index map (SP0: 0-3, SPx: 4-7, lower-EL A64: 8-11, A32: 12-15;
    // within each block: sync, irq, fiq, serror).
    .balign 0x800
    .globl tegra_vectors_el2
tegra_vectors_el2:
    .set idx2, 0
    .rept 16
    .balign 0x80
    mov x3, #idx2
    b   tegra_fault_common_el2
    .set idx2, idx2+1
    .endr
tegra_fault_common_el2:
    mrs x0, ESR_EL2
    mrs x1, FAR_EL2
    mrs x2, ELR_EL2
    mrs x4, SPSR_EL2
    b   tegra_fault_handler

    // EL1 fallback vector table (same shape; reads the EL1 syndrome registers).
    .balign 0x800
    .globl tegra_vectors_el1
tegra_vectors_el1:
    .set idx1, 0
    .rept 16
    .balign 0x80
    mov x3, #idx1
    b   tegra_fault_common_el1
    .set idx1, idx1+1
    .endr
tegra_fault_common_el1:
    mrs x0, ESR_EL1
    mrs x1, FAR_EL1
    mrs x2, ELR_EL1
    mrs x4, SPSR_EL1
    b   tegra_fault_handler
"#
);

/// Shared fault printer (tail-called from either stub). Diverges. Prints via the existing serial path —
/// UARTC is device-mapped by the time our vectors are installed. On the EL1 fallback the three values
/// carry the EL1 syndrome/fault-addr/return-addr; the label reads `ESR_EL2` (the primary path) to match
/// the pinned message string. (If a fault ever struck mid-`serial_println!`, holding `SERIAL_PORT`, this
/// would spin on the lock — an accepted risk for a diagnostic of last resort; the common case, a wrong
/// device/RAM mapping caught at the first access, prints cleanly.)
#[unsafe(no_mangle)]
extern "C" fn tegra_fault_handler(esr: u64, far: u64, elr: u64, idx: u64, spsr: u64) -> ! {
    // idx names the vector entry (blocks of 4 — SP0 / SPx / lower-A64 / lower-A32; within each:
    // 0 sync, 1 irq, 2 fiq, 3 serror). ESR is only meaningful for sync/serror entries.
    let kind = match idx & 3 {
        0 => "sync",
        1 => "IRQ",
        2 => "FIQ",
        _ => "SError",
    };
    serial_println!(
        ":: tegra: FAULT — entry {} ({}) ESR={:#x} FAR={:#x} ELR={:#x} SPSR={:#x} ::",
        idx,
        kind,
        esr,
        far,
        elr,
        spsr,
    );
    // The EC=0 phantom probe (see exceptions.rs twin + arch_arm64.md): D-side read-back of the
    // faulting instruction word — equal-to-the-ELF proves a D/I-side divergence (stale I-cache
    // class), different proves memory corruption. Range-guarded against a garbage ELR.
    if (esr >> 26) & 0x3f == 0 && idx & 3 == 0 && (0x8000_0000..0x40_0000_0000).contains(&elr) {
        let dword = unsafe { core::ptr::read_volatile(elr as *const u32) };
        let ctr: u64;
        unsafe {
            core::arch::asm!("mrs {}, CTR_EL0", out(reg) ctr, options(nomem, nostack, preserves_flags));
        }
        serial_println!(
            ":: tegra: EC0-probe — D-side [ELR]={:#010x} CTR_EL0={:#x} ::",
            dword,
            ctr,
        );
    }
    loop {
        core::hint::spin_loop();
    }
}

/// JB1d: the A78AE erratum-1941500 probe — the EC=0 phantom's leading suspect (see arch_arm64.md
/// "JB1 result"; the D-side read-back PROVED an I-side/D-side divergence at the fault). The
/// documented workaround is CPUECTLR_EL1[8]=1 (A78AE r0p1 and earlier), and TF-A's A78AE
/// implementation historically INVERTED it (`bic` instead of `orr`), so BL31-lineage firmware may
/// leave the bit CLEAR — or actively clear it. CPUECTLR_EL1 is IMPLEMENTATION DEFINED
/// (S3_0_C15_C1_4 on the A78 family, per TF-A cortex_a78.h); EL3 may gate lower-EL access, so an
/// access here can itself UNDEF. Every step prints BEFORE the touch. Since JB1f this runs under
/// the HEALED exceptions.rs vectors (not Part-C): a gated read's UNDEF is EC=0 with a VALID
/// D-side word (the mrs encoding itself), indistinguishable from a phantom, so the heal would
/// retry it up to the 32-consecutive same-PC cap and THEN go fatal — the dump prints ESR/ELR/FAR
/// plus the heal tally (streak count + streak ELR), so the announce line above + a 32-streak
/// fatal right after it still names the culprit in one boot. Today's firmware permits the read
/// (metal-proven: the JB1d value line prints every boot); this margin note is for a future
/// firmware that regresses it.
pub fn a78ae_errata_probe() {
    let midr: u64;
    unsafe {
        core::arch::asm!("mrs {}, MIDR_EL1", out(reg) midr, options(nomem, nostack, preserves_flags));
    }
    serial_println!(
        ":: tegra: JB1d — MIDR={:#x} (r{}p{}) — reading CPUECTLR_EL1 (IMPDEF) next ::",
        midr,
        (midr >> 20) & 0xf,
        midr & 0xf,
    );
    let ecx: u64;
    unsafe {
        core::arch::asm!("mrs {}, S3_0_C15_C1_4", out(reg) ecx, options(nomem, nostack, preserves_flags));
    }
    serial_println!(":: tegra: JB1d — CPUECTLR_EL1={:#x} (erratum-1941500 bit8={}) ::", ecx, (ecx >> 8) & 1);
    // METAL VERDICT (2026-07-06): the WRITE is EL3-gated — `msr S3_0_C15_C1_4` from EL2 traps to
    // an UNHANDLED EL3 exception (BL31 crash dump, box reboots; two attended boots confirmed).
    // The bit CANNOT be applied OS-side on this firmware; only an NVIDIA BL31/UEFI update can.
    // Report-only here; the OS-side mitigation is the JB1e heal (exceptions.rs: ic iallu + retry
    // on the proven-stale EC=0 signature).
}

/// ORIN-VUG-RAS — carveout-aware kernel-heap placement for the Orin (tegra). NVIDIA's UEFI reports
/// several firewall-protected DRAM carveouts (TZ, BPMP, DCE, ...) as ordinary Conventional (Usable)
/// memory, so the naive "first Usable region ≥ HEAP_SIZE" pick (`arch::memory::init`) can back the
/// heap with protected DRAM. A cached store into it succeeds into the D-cache and then faults on
/// *writeback* with an SNOC RAS Uncorrectable "Carveout" abort that powers the cores off — the
/// observed vug lockup (a heap String / back-buffer store into a carveout page, evicted a few frames
/// after "crystal live … exit clean" printed, which is why the RAS lands late). This picks a
/// `HEAP_SIZE` window that clears BOTH the non-Usable regions in the UEFI map AND the DTB
/// `/reserved-memory` carveouts, prints one witness line naming the guarded range and the carveout
/// count, and returns `None` (caller fails closed) when no clean window exists. No protection is
/// weakened — this only *narrows* what the heap may claim. Returns `(heap_start, HEAP_SIZE)`.
///
/// ORIN-RAS-2 — prefer the HIGHEST clean window, not the first. Boot-5 metal proved that seating
/// the heap at the DRAM base (0x8000_0000) put the NIC's DMA rings BELOW the PCIe inbound-DMA
/// window: the fabric translated ring writebacks to ~0x0..0x200 and returned "Error response from
/// slave" (RAS Uncorrectable in IOB, cores powered off). The exact inbound-window boundary is not
/// derivable from code — but R22 sitting-2 PROVED the rings DMA correctly with the old high-DRAM
/// heap placement (heap around GiB 9 / the image region). So the highest clean window is the
/// data-justified placement: it restores that proven DMA reachability while keeping the carveout
/// exclusion and fail-closed behavior exactly as-is. Per region we scan DOWN from
/// `region_end - need`, sliding below overlapping carveouts (O(carveouts), not O(region/4K)), and
/// return the highest clean base across all usable regions.
///
/// ORIN-DMA-WINDOW — stop trusting "highest clean" as a proxy for reachability; DERIVE the real
/// inbound-DMA window. `fdt_tegra::pcie_dma_windows` parses the PCIe RC's `dma-ranges` into the
/// firmware-declared bus→CPU inbound window(s). When windows are derived, the chosen heap base must
/// fall fully INSIDE one — otherwise we fail closed (naming the best out-of-window base) rather than
/// silently seat a DMA heap the fabric will reject (the RAS-2 class). When none are derivable (QEMU
/// virt / foreign DTB / no `dma-ranges`) the selector degrades to the RAS-2 highest-clean heuristic
/// and witnesses the degrade. Carveout exclusion + fail-closed behavior are otherwise unchanged.
#[cfg(feature = "tegra")]
pub fn select_heap_region(
    regions: &[MemoryRegion],
    dtb_addr: u64,
    dtb_size: usize,
) -> Option<(usize, usize)> {
    const PAGE: u64 = 4096;
    let heap_need = crate::allocator::HEAP_SIZE as u64;
    // NET-4B: reserve a 2 MiB-aligned Normal-NC DMA window carved from the SAME clean + DMA-reachable
    // span the heap sits in. We search for `heap + one 2 MiB block + up to 2 MiB alignment slack`, seat
    // the heap at the TOP of the found window, and place the NC block 2 MiB-aligned just below it — no
    // overlap by construction, and nothing else double-uses it (the block is outside the heap span, and
    // the kernel hands out no DRAM but the heap). The heap stays high-DRAM (the RAS-2 / NET-4A
    // proven-reachable placement); the NC block is contiguous just below, so it is equally clean and
    // equally inside the (degraded or derived) inbound-DMA window. `install_net4b_nc` maps it NC.
    const NC_BYTES: u64 = NET4B_NC_SIZE;
    let need = heap_need + 2 * NC_BYTES;

    // Collect carveouts to avoid: every non-Usable region the UEFI map declares, plus the DTB
    // `/reserved-memory` carveouts (the firewall ones UEFI hides inside Conventional descriptors).
    // XCARVE-5: 192 (was 96) — boot-23 proved the UEFI+FDT set alone fills 96, which silently
    // dropped the XCARVE-3 hole append below and let span B sweep the unmapped window (sync fault
    // at 0x26b800000). The hole is now seeded FIRST (slot 0, can never be dropped) and any
    // overflow is witnessed — no silent caps.
    const MAX_CARVE: usize = 192;
    let mut carve = [(0u64, 0u64); MAX_CARVE];
    let mut nc = 0usize;
    let mut dropped = 0usize;
    {
        let (hb, hs, _) = carveout_hole();
        if hs != 0 {
            carve[nc] = (hb, hs);
            nc += 1;
        }
    }
    for r in regions {
        if r.kind != MemoryRegionKind::Usable {
            if nc < MAX_CARVE {
                carve[nc] = (r.phys_start, (r.page_count * 4096) as u64);
                nc += 1;
            } else {
                dropped += 1;
            }
        }
    }
    let mut fdt_carve = [(0u64, 0u64); 48];
    let nf = super::fdt_tegra::reserved_carveouts(dtb_addr, dtb_size, &mut fdt_carve);
    // `reserved_carveouts` returns a bare COUNT — 0 covers "the DTB declares none", "the blob was
    // absent/implausible", and "the header failed to parse", and those are very different worlds for
    // a heap that must dodge SNOC-firewalled DRAM. Witness the count either way, and say loudly when
    // it is zero: a zero here means the ONLY protection left is the UEFI-reserved set plus the
    // XCARVE quirk, i.e. the exclusion set is materially weaker than the one the boot assumes.
    if nf == 0 {
        serial_println!(
            ":: tegra: HEAP-GUARD WARNING — DTB /reserved-memory yielded ZERO carveouts (dtb @{:#x} size={:#x}); heap dodges only the UEFI-reserved set + the XCARVE quirk ::",
            dtb_addr, dtb_size
        );
    } else {
        serial_println!(
            ":: tegra: HEAP-GUARD — DTB /reserved-memory contributed {} carveout range(s) (cap 48) to the exclusion set ::",
            nf
        );
    }
    for &c in fdt_carve.iter().take(nf) {
        if nc < MAX_CARVE {
            carve[nc] = c;
            nc += 1;
        } else {
            dropped += 1;
        }
    }
    // XCARVE-3/5: the protected-carveout hole is seeded at slot 0 above (can never be dropped —
    // boot-23's silent drop of a late append at nc==MAX_CARVE is the exact failure this ordering
    // buries). Witness any overflow loudly: a dropped carveout means the exclusion set is
    // incomplete and every consumer (heap seat, span-B top) may be wrong.
    if dropped != 0 {
        serial_println!(
            ":: tegra: HEAP-GUARD WARNING — {} carveout range(s) DROPPED (set full at {}); exclusion set INCOMPLETE ::",
            dropped, MAX_CARVE
        );
    }
    let carveouts = &carve[..nc];

    // VUG-RAS-ANALYZE: publish the carveout-free top bound for the localizer's above-heap sweep
    // (span B) from the SAME carveout set that seats the heap. The heap `[s, s+need)` is proven clear
    // of every range in `carveouts`, so no carveout can straddle `heap_hi`; the LOWEST carveout base
    // at/above `heap_hi` therefore bounds a provably carveout-free `[heap_hi, top)`. Span B must never
    // DC-CIVAC a carveout (cleaning a firewalled line IS the RAS), so the localizer clips to this.
    let publish_above_heap_top = |heap_base: u64| {
        let heap_hi = heap_base + heap_need;
        let mut top = crate::vugras::TEGRA_DRAM_TOP as u64;
        for &(cb, cs) in carveouts {
            if cs != 0 && cb >= heap_hi && cb < top {
                top = cb;
            }
        }
        crate::vugras::VUGRAS_ABOVE_HEAP_TOP.store(top as usize, core::sync::atomic::Ordering::Relaxed);
    };

    // NET-4B: from a clean base `s` of a `need`-sized window, split it into the heap (top) and the NC
    // window (2 MiB-aligned, just below). Latches `NET4B_NC_BASE`, publishes the span-B top for the HEAP
    // (which now tops the window), and returns the heap base. The NC window sits in `[s, heap_base)`,
    // fully inside the proven-clean, proven-in-DMA-window span `[s, s+need)`.
    let seat = |s: u64| -> u64 {
        let heap_base = s + 2 * NC_BYTES;
        let nc_base = (s + NC_BYTES - 1) & !(NC_BYTES - 1);
        NET4B_NC_BASE.store(nc_base, core::sync::atomic::Ordering::Relaxed);
        publish_above_heap_top(heap_base);
        heap_base
    };

    // The greatest carveout BASE that overlaps [s, s+need), if any — the scan-down loop slides the
    // window strictly below it (`s <= cb - need`). Taking the *max* overlapping base is safe: the
    // re-check after each slide catches any lower carveouts, and each slide strictly decreases `s`
    // (an overlap means `s + need > cb`, so `cb - need < s`), so the loop terminates in O(carveouts).
    let overlap_max_base = |s: u64, e: u64| -> Option<u64> {
        let mut hit: Option<u64> = None;
        for &(cb, cs) in carveouts {
            let ce = cb.wrapping_add(cs);
            if cs != 0 && s < ce && cb < e {
                hit = Some(hit.map_or(cb, |h| h.max(cb)));
            }
        }
        hit
    };

    // ORIN-DMA-WINDOW — derive the REAL inbound-DMA window(s) from the PCIe RC's `dma-ranges` (not
    // the RAS-2 "highest clean" folklore boundary). The kernel heap backs the NIC's RX/TX rings +
    // buffers; those DMA-touched allocations MUST fall inside a declared inbound window or a
    // bus-master writeback translates below it → the RAS-2 IOB/SNOC fabric-error class. An empty
    // derivation (QEMU-virt / foreign DTB / no `dma-ranges`) means the window is not derivable on this
    // boot ⇒ degrade to the RAS-2 highest-clean heuristic, but SAY SO on serial (fail loud, not silent).
    let mut win = [(0u64, 0u64); 8];
    let nd = super::fdt_tegra::pcie_dma_windows(dtb_addr, dtb_size, &mut win);
    let windows = &win[..nd];

    // Highest carveout-clean page-aligned base for a `need`-byte window inside `[lo, hi)`, or None.
    // Scans DOWN from `hi - need`, sliding strictly below the highest overlapping carveout each step
    // (O(carveouts), terminating — each slide strictly decreases `s`). Shared by the unconstrained
    // (heuristic) and the window-constrained searches below.
    let highest_clean_in = |lo: u64, hi: u64| -> Option<u64> {
        if hi < need || hi - need < lo {
            return None; // range can't hold a window
        }
        let mut s = (hi - need) & !(PAGE - 1);
        loop {
            if s < lo {
                return None;
            }
            match overlap_max_base(s, s + need) {
                None => return Some(s),
                Some(cb) => {
                    if cb < need {
                        return None;
                    }
                    let next = (cb - need) & !(PAGE - 1);
                    if next >= s {
                        return None; // no downward progress (guard; shouldn't happen)
                    }
                    s = next;
                }
            }
        }
    };

    // best_uncon: highest clean base across ALL usable regions (the RAS-2 heuristic — the fallback and
    // the "what we'd have picked" diagnostic). best_in_win: the highest clean base that ALSO lies
    // fully inside a derived inbound window (the intersection of each usable region with each window).
    let mut best_uncon: Option<u64> = None;
    let mut best_in_win: Option<u64> = None;
    for r in regions {
        if r.kind != MemoryRegionKind::Usable {
            continue;
        }
        let region_end = r.phys_start.wrapping_add((r.page_count * 4096) as u64);
        let region_base = (r.phys_start + PAGE - 1) & !(PAGE - 1);
        if let Some(s) = highest_clean_in(region_base, region_end) {
            best_uncon = Some(best_uncon.map_or(s, |b| b.max(s)));
        }
        for &(wb, ws) in windows {
            let lo = region_base.max(wb);
            let hi = region_end.min(wb.wrapping_add(ws));
            if hi <= lo {
                continue;
            }
            if let Some(s) = highest_clean_in(lo, hi) {
                best_in_win = Some(best_in_win.map_or(s, |b| b.max(s)));
            }
        }
    }

    if nd == 0 {
        // No derivable inbound window: keep the RAS-2 highest-clean heuristic, but witness the degrade.
        if let Some(s) = best_uncon {
            let heap_base = seat(s);
            let (ncb, ncs) = net4b_nc_window();
            serial_println!(
                ":: tegra: HEAP-GUARD — kernel heap [{:#x}, {:#x}) ({} MiB), highest clean window (RAS-2 heuristic — NO PCIe dma-ranges in DTB, inbound-DMA window NOT derivable; degraded), clear of {} carveout range(s) (UEFI-reserved + DTB /reserved-memory) ::",
                heap_base,
                heap_base + heap_need,
                heap_need >> 20,
                nc
            );
            serial_println!(
                ":: tegra: [net4B] Normal-NC DMA window reserved [{:#x}, {:#x}) ({} KiB), carved just below the heap in the same clean+DMA span ::",
                ncb,
                ncb + ncs,
                ncs >> 10
            );
            return Some((heap_base as usize, heap_need as usize));
        }
        serial_println!(
            ":: tegra: HEAP-GUARD — FAIL-CLOSED: no {} MiB DRAM window clear of {} carveout(s) ::",
            heap_need >> 20,
            nc
        );
        return None;
    }

    // Derived window(s) in hand — name the derivation, then require containment (fail loud otherwise).
    serial_println!(
        ":: tegra: HEAP-GUARD — derived {} PCIe inbound-DMA window(s) from dma-ranges; window[0] = [{:#x}, {:#x}) ({} MiB) ::",
        nd,
        windows[0].0,
        windows[0].0.wrapping_add(windows[0].1),
        windows[0].1 >> 20
    );
    if let Some(s) = best_in_win {
        let heap_base = seat(s);
        let (ncb, ncs) = net4b_nc_window();
        serial_println!(
            ":: tegra: HEAP-GUARD — kernel heap [{:#x}, {:#x}) ({} MiB), highest clean window INSIDE the derived PCIe inbound-DMA window(s) (RAS-2 boundary now DERIVED, not folklore), clear of {} carveout range(s) (UEFI-reserved + DTB /reserved-memory) ::",
            heap_base,
            heap_base + heap_need,
            heap_need >> 20,
            nc
        );
        serial_println!(
            ":: tegra: [net4B] Normal-NC DMA window reserved [{:#x}, {:#x}) ({} KiB), carved just below the heap inside the derived inbound-DMA window ::",
            ncb,
            ncb + ncs,
            ncs >> 10
        );
        return Some((heap_base as usize, heap_need as usize));
    }

    // A carveout-clean window exists but NONE inside the derived inbound window ⇒ any pick would
    // re-arm the RAS-2 fabric-error class. Refuse, naming the best out-of-window base we would have
    // taken so the divergence is diagnosable rather than a silent bad placement.
    serial_println!(
        ":: tegra: HEAP-GUARD — FAIL-CLOSED: no {} MiB carveout-clean window falls inside the {} derived PCIe inbound-DMA window(s); best clean-but-out-of-window base = {:#x} — REFUSING (would re-arm the RAS-2 inbound fabric-error class) ::",
        heap_need >> 20,
        nd,
        best_uncon.unwrap_or(0)
    );
    None
}

// ── JD1-MAP witnesses (tail-defined per the Location-shift convention: each refusal rung stays a
// single `return fb_map_refuse(..)` line, so no call-site line numbers move). `map_fb_region` used
// to return a bare `false` from three places, and its one caller then printed ONE generic reason —
// "scanout base … not mappable (not DRAM GiB 2..63)" — which is simply WRONG for two of the three
// (a zero length, and a GiB the patch could not turn into RAM). A headless bench boot could not tell
// them apart. These name the actual rung, with the value that decided it. ────────────────────────

/// One named JD1-MAP refusal witness + the `false` the rung returns.
#[inline(never)]
fn fb_map_refuse(why: &str, a: u64, b: u64) -> bool {
    serial_println!(
        ":: tegra: JD1-MAP REFUSED — {} = {:#x} / {:#x}; the inherited scanout is NOT mapped (caller skips the blit) ::",
        why, a, b
    );
    false
}

/// The per-GiB failure witness: after the L1/L2 patch this GiB is still neither a RAM block nor a
/// carveout L2 split, so the scanout span is not fully backed. Names the GiB and the live descriptor.
#[inline(never)]
fn fb_map_gib_refused(gib: u64, desc: u64) {
    serial_println!(
        ":: tegra: JD1-MAP REFUSED — GiB {} still not RAM after patch (L1 desc={:#x}); scanout span not fully mapped ::",
        gib, desc
    );
}
