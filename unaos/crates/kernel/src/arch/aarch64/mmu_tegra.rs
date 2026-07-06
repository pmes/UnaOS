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
        2 => (TCR_EL2_VAL, MAIR_VAL),
        1 => (TCR_EL1_VAL, MAIR_VAL),
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
            tcr = in(reg) TCR_EL2_VAL,
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
            tcr = in(reg) TCR_EL1_VAL,
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

// ── Part C: bounded fault visibility ────────────────────────────────────────────────────────────────
//
// A minimal, inert-until-it-fires EL2/EL1 vector table. Two 2 KiB-aligned tables (16 entries each, at
// the architectural 0x80 spacing) each funnel to a stub that captures the EL's syndrome/fault-address/
// return-address and tail-calls the shared Rust printer, which prints one line and spins. This turns a
// dark post-switch metal hang into a recorded syndrome. Deliberately does not touch `exceptions.rs`.

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
/// access here can itself UNDEF — every step prints BEFORE the touch, and an UNDEF lands in the
/// instrumented Part-C vector with the announce line naming it.
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
    if (ecx >> 8) & 1 == 0 {
        serial_println!(":: tegra: JB1d — bit8 CLEAR: applying the 1941500 workaround (set) ::");
        unsafe {
            core::arch::asm!(
                "msr S3_0_C15_C1_4, {}",
                "isb",
                in(reg) ecx | (1 << 8),
                options(nostack, preserves_flags)
            );
        }
        let rb: u64;
        unsafe {
            core::arch::asm!("mrs {}, S3_0_C15_C1_4", out(reg) rb, options(nomem, nostack, preserves_flags));
        }
        serial_println!(":: tegra: JB1d — CPUECTLR_EL1 readback={:#x} ::", rb);
    }
}
