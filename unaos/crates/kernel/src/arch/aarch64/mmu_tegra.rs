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

// ── Translation attributes (ARM ARM DDI0487). ──────────────────────────────────────────────────────
// MAIR: AttrIdx 0 = Normal Inner/Outer Write-Back non-transient (0xFF); AttrIdx 1 = **Device-nGnRE**
// (0x04) — deliberately nGnRE for Tegra (early-write-ack tolerant), NOT the Pi's nGnRnE (0x00). Layout
// is regime-independent, so the same value programs MAIR_EL2 or MAIR_EL1.
const MAIR_VAL: u64 = 0x04FF;

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
}

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
    unsafe { clean_table_to_poc(ttbr0) };
    if el == 2 {
        unsafe { clean_table_to_poc(ttbr0_el1) };
    }
    let (sctlr_old, sctlr_new) = unsafe {
        if el == 2 { enable_el2(ttbr0) } else { enable_el1(ttbr0) }
    };
    // Now our own tables are live and RAM + the Tegra device window are mapped. Point the vector base
    // at our handler (Part C) so a subsequent fault is a recorded syndrome, not R4's dark hang under
    // UEFI's now-possibly-unmapped VBAR.
    unsafe { install_vectors(el) };
    MmuInfo { el, sctlr_old, sctlr_new, tcr, mair, ttbr0, ram_gib_mask, ttbr0_el1 }
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
        return false;
    }
    let g_lo = pa >> 30;
    let g_hi = (pa + size as u64 - 1) >> 30;
    // A scanout framebuffer lives in DRAM (Orin: 0x8000_0000.., GiB 2 upward). GiB 0 is the Tegra
    // Device window and GiB 1 the SYSRAM/peripheral window — never a framebuffer.
    if g_lo < 2 || g_hi >= 64 {
        return false;
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
            if !is_ram_block(cur1) {
                unsafe {
                    l1_el1.add(gi).write_volatile(ram_block(g << 30, 1));
                    clean_desc(l1_el1.add(gi) as u64);
                }
                changed = true;
            }
        }
        all_ok &= is_ram_block(unsafe { l1.add(gi).read_volatile() });
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
            if !is_device_block(cur1) {
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
    let need = crate::allocator::HEAP_SIZE as u64;

    // Collect carveouts to avoid: every non-Usable region the UEFI map declares, plus the DTB
    // `/reserved-memory` carveouts (the firewall ones UEFI hides inside Conventional descriptors).
    const MAX_CARVE: usize = 96;
    let mut carve = [(0u64, 0u64); MAX_CARVE];
    let mut nc = 0usize;
    for r in regions {
        if r.kind != MemoryRegionKind::Usable && nc < MAX_CARVE {
            carve[nc] = (r.phys_start, (r.page_count * 4096) as u64);
            nc += 1;
        }
    }
    let mut fdt_carve = [(0u64, 0u64); 48];
    let nf = super::fdt_tegra::reserved_carveouts(dtb_addr, dtb_size, &mut fdt_carve);
    for &c in fdt_carve.iter().take(nf) {
        if nc < MAX_CARVE {
            carve[nc] = c;
            nc += 1;
        }
    }
    let carveouts = &carve[..nc];

    // VUG-RAS-ANALYZE: publish the carveout-free top bound for the localizer's above-heap sweep
    // (span B) from the SAME carveout set that seats the heap. The heap `[s, s+need)` is proven clear
    // of every range in `carveouts`, so no carveout can straddle `heap_hi`; the LOWEST carveout base
    // at/above `heap_hi` therefore bounds a provably carveout-free `[heap_hi, top)`. Span B must never
    // DC-CIVAC a carveout (cleaning a firewalled line IS the RAS), so the localizer clips to this.
    let publish_above_heap_top = |heap_base: u64| {
        let heap_hi = heap_base + need;
        let mut top = crate::vugras::TEGRA_DRAM_TOP as u64;
        for &(cb, cs) in carveouts {
            if cs != 0 && cb >= heap_hi && cb < top {
                top = cb;
            }
        }
        crate::vugras::VUGRAS_ABOVE_HEAP_TOP.store(top as usize, core::sync::atomic::Ordering::Relaxed);
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
            serial_println!(
                ":: tegra: HEAP-GUARD — kernel heap [{:#x}, {:#x}) ({} MiB), highest clean window (RAS-2 heuristic — NO PCIe dma-ranges in DTB, inbound-DMA window NOT derivable; degraded), clear of {} carveout range(s) (UEFI-reserved + DTB /reserved-memory) ::",
                s,
                s + need,
                need >> 20,
                nc
            );
            publish_above_heap_top(s);
            return Some((s as usize, need as usize));
        }
        serial_println!(
            ":: tegra: HEAP-GUARD — FAIL-CLOSED: no {} MiB DRAM window clear of {} carveout(s) ::",
            need >> 20,
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
        serial_println!(
            ":: tegra: HEAP-GUARD — kernel heap [{:#x}, {:#x}) ({} MiB), highest clean window INSIDE the derived PCIe inbound-DMA window(s) (RAS-2 boundary now DERIVED, not folklore), clear of {} carveout range(s) (UEFI-reserved + DTB /reserved-memory) ::",
            s,
            s + need,
            need >> 20,
            nc
        );
        publish_above_heap_top(s);
        return Some((s as usize, need as usize));
    }

    // A carveout-clean window exists but NONE inside the derived inbound window ⇒ any pick would
    // re-arm the RAS-2 fabric-error class. Refuse, naming the best out-of-window base we would have
    // taken so the divergence is diagnosable rather than a silent bad placement.
    serial_println!(
        ":: tegra: HEAP-GUARD — FAIL-CLOSED: no {} MiB carveout-clean window falls inside the {} derived PCIe inbound-DMA window(s); best clean-but-out-of-window base = {:#x} — REFUSING (would re-arm the RAS-2 inbound fabric-error class) ::",
        need >> 20,
        nd,
        best_uncon.unwrap_or(0)
    );
    None
}
