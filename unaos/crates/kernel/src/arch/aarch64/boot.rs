// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Bare-metal Raspberry Pi 4 boot (the `baremetal` feature). The VideoCore GPU ROM reads the microSD
// slot perfectly (it loads start4.elf/RPI_EFI.fd from it) — it's only EDK2/UEFI's own SD driver that
// can't, which is why the UEFI path needs USB. So here we cut UEFI out: the GPU ROM loads our flat
// `kernel8.img` directly to 0x80000 and jumps to `_start` (in main.rs's .text.boot) at EL2 with x0 =
// DTB, MMU off, caches off. `_start` (asm) parks secondary cores, zeroes BSS, sets the stack, and
// calls `__rust_boot`, which calls the two functions here — `mmu_init` then `build_boot_info` — and
// then enters the normal `kernel_main`.
//
// Why the MMU is mandatory (not just nice): the GPU hands off with the MMU OFF, where all memory is
// treated as Device/Strongly-ordered. AArch64 exclusive accesses (`ldxr`/`stxr`) — which every
// spinlock and atomic in this kernel rely on — are CONSTRAINED UNPREDICTABLE on non-Normal-cacheable
// memory. So before any of the kernel's locks run we must enable the MMU with at least RAM mapped
// Normal cacheable. We use the simplest possible map: a single L1 table of 1 GiB identity blocks.
//
// EL: the firmware hands every core off at EL2 (the hypervisor level — incidental, not for isolation).
// A normal OS runs at EL1, so each core calls `drop_to_el1` (below) BEFORE `mmu_init`/`enable_mmu`; the
// MMU and everything after it therefore run in the EL1&0 translation regime (TTBR0_EL1/TCR_EL1/
// SCTLR_EL1). The drop is purely additive — it configures the EL1-facing controls that would otherwise
// trap or read UNKNOWN, and leaves the (now-unused-for-translation) EL2 regime alone.

use unaos_boot_info::{BootInfo, FrameBufferInfo, MemoryRegion, MemoryRegionKind, PixelFormat};

/// A single Level-1 translation table: 512 entries × 8 bytes = one 4 KiB page. With a 4 KiB granule
/// and a 39-bit VA (TCR T0SZ=25) the top lookup level is L1 and each entry maps a 1 GiB block, so
/// these 512 entries cover the whole 512 GiB VA — we only fill the first four (0–4 GiB). Lives in
/// BSS (zeroed by `_start` before we fill it).
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1: PageTable = PageTable([0; 512]);

// --- Translation attributes (ARM ARM DDI0487). We drop to EL1 (see `drop_to_el1`) before enabling
// the MMU, so these program the EL1&0 regime (TTBR0_EL1/TCR_EL1/MAIR_EL1/SCTLR_EL1). ---
// MAIR: AttrIdx 0 = Normal Inner/Outer Write-Back non-transient (0xFF); AttrIdx 1 = Device-nGnRnE
// (0x00). Regime-independent — same layout at EL1 and EL2.
const MAIR_VAL: u64 = 0xFF;

// TCR_EL1 (NOT TCR_EL2 — the field layout differs). T0SZ=25 (39-bit VA → L1 top level, 1 GiB blocks),
// IRGN0=ORGN0=WB, SH0=Inner-shareable, TG0=4 KiB. The TTBR1 (high) half is unused, so EPD1=1 disables
// its table walk (TTBR1_EL1 stays 0); T1SZ/TG1 are given legal values regardless. Note the two traps
// for anyone copying TCR_EL2: PS is IPS at bits [34:32] (not [18:16]), and bits 23/31 are EPD1/TG1
// here, NOT the RES1 bits the non-VHE TCR_EL2 short format has.
const TCR_EL1_VAL: u64 = 25            // T0SZ  [5:0]
    | (0b01 << 8)                      // IRGN0 = WB
    | (0b01 << 10)                     // ORGN0 = WB
    | (0b11 << 12)                     // SH0   = inner shareable
    | (0b00 << 14)                     // TG0   = 4 KiB
    | (25 << 16)                       // T1SZ  [21:16] (TTBR1 unused; legal value)
    | (1 << 23)                        // EPD1  = disable the TTBR1 table walk
    | (0b10 << 30)                     // TG1   = 4 KiB (legal encoding; TTBR1 unused)
    | (0b001 << 32);                   // IPS   = 36-bit / 64 GiB, at [34:32]

// SCTLR_EL1 built as an ABSOLUTE value, not read-modify-write. SCTLR_EL1 resets to an architecturally
// UNKNOWN value (unlike SCTLR_EL2, which firmware initialised before handing off — which is why the
// old EL2 path could RMW it safely). An RMW here could read the RES1 bits as 0 and leave them cleared
// → CONSTRAINED UNPREDICTABLE translation/execution. So OR the Armv8.0 Cortex-A72 SCTLR_EL1 RES1 mask
// (0x30D00800 = bits 29,28,23,22,20,11) with M (MMU), C (data cache), I (instruction cache).
const SCTLR_EL1_VAL: u64 = 0x30D0_0800 | (1 << 0) | (1 << 2) | (1 << 12);

// L1 block descriptor lower attributes: bits[1:0]=0b01 (block), AttrIndx[4:2], AP[7:6]=0b00
// (privileged RW, no EL0 access — the EL1&0 regime), SH[9:8], AF=bit10.
const DESC_BLOCK: u64 = 0b01;
const DESC_AF: u64 = 1 << 10;
const SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 0 << 2; // AttrIdx 0
const ATTR_DEVICE: u64 = 1 << 2; // AttrIdx 1
// Execute-never. The EL1&0 translation regime splits execute-never into UXN (bit 54, EL0) and PXN
// (bit 53, EL1); a Device/peripheral block must set PXN to be non-executable by the EL1 kernel (the
// old EL2 regime had only bit 54 = XN). Set both so peripheral memory is never executable at any EL.
const DESC_XN: u64 = (1 << 54) | (1 << 53);

/// Normal, cacheable, inner-shareable, executable block (RAM where the kernel/heap/stack live).
const fn ram_block(pa: u64) -> u64 {
    pa | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_BLOCK
}
/// Device-nGnRnE, execute-never block (the peripheral window: PL011 0xFE201000, GIC 0xFF841000,
/// mailbox 0xFE00B880 all sit in the 0xC000_0000–0xFFFF_FFFF GiB).
const fn device_block(pa: u64) -> u64 {
    pa | DESC_XN | DESC_AF | ATTR_DEVICE | DESC_BLOCK
}

/// Build the identity map and turn the MMU on. Called (with the MMU still off, at EL1 after
/// `drop_to_el1`) from `__rust_boot` after BSS has been zeroed. Plain volatile writes + system-
/// register moves only — no atomics, no locks, nothing that needs the MMU we're about to enable.
pub unsafe fn mmu_init() {
    build_l1();
    unsafe { enable_mmu() };
}

/// Populate the single L1 translation table. BSP-only: the secondaries reuse this same table (they
/// call `enable_mmu` alone), so it must be built exactly once, before any core turns its MMU on.
fn build_l1() {
    let table = &raw mut L1 as *mut u64;
    unsafe {
        // 0–1 GiB and 1–3 GiB: RAM (Normal). On an 8 GiB Pi there is RAM above 3 GiB too, but the
        // kernel never uses it (heap is in low RAM), so the top GiB is peripherals as Device.
        table.add(0).write_volatile(ram_block(0x0000_0000));
        table.add(1).write_volatile(ram_block(0x4000_0000));
        table.add(2).write_volatile(ram_block(0x8000_0000));
        table.add(3).write_volatile(device_block(0xC000_0000));
    }
}

/// Point this core's TTBR0_EL1 at the (already-built) L1 table and turn the MMU + caches on. Run on
/// EACH core with its MMU still off, AFTER `drop_to_el1` has put it at EL1 — the BSP after `build_l1`,
/// every secondary after the spin-table release (`arch::aarch64::smp`). System-register moves only;
/// no atomics/locks (the very thing the MMU is being enabled to make sound). The table is shared, so
/// no per-core state here.
pub unsafe fn enable_mmu() {
    let ttbr0 = &raw const L1 as u64;
    unsafe {
        core::arch::asm!(
            "msr MAIR_EL1, {mair}",
            "msr TCR_EL1,  {tcr}",
            "msr TTBR0_EL1, {ttbr}",
            "msr TTBR1_EL1, xzr", // high half unused (EPD1=1 disables its walk); zero it defensively
            "tlbi vmalle1",       // drop any stale EL1&0 TLB entries before translation is enabled
            "dsb sy",
            "isb",
            // Enable MMU (M=bit0), data cache (C=bit2), instruction cache (I=bit12) via an ABSOLUTE
            // SCTLR_EL1 write (SCTLR_EL1 resets UNKNOWN, so no RMW — SCTLR_EL1_VAL carries the A72
            // RES1 bits). See the const's comment.
            "msr SCTLR_EL1, {sctlr}",
            "isb",
            mair = in(reg) MAIR_VAL,
            tcr = in(reg) TCR_EL1_VAL,
            ttbr = in(reg) ttbr0,
            sctlr = in(reg) SCTLR_EL1_VAL,
            options(nostack, preserves_flags),
        );
    }
}

// --- EL2 -> EL1 drop. Every core is handed off at EL2; a normal OS runs at EL1, so we drop each core
// before it enables its MMU. MUST be naked asm: an ordinary Rust fn's prologue/epilogue would spill/
// reload x30 and adjust SP around the `eret`, and the eret skips the epilogue — corrupting the frame.
// Runs at EL2, MMU OFF, no stack traffic, x30 untouched; `eret`s back to the caller now at EL1 (same
// SP/frame — the standard "return to x30" drop trick). ---
core::arch::global_asm!(
    r#"
    .globl drop_to_el1
drop_to_el1:
    // MPIDR_EL1/MIDR_EL1 read at EL1 return VMPIDR_EL2/VPIDR_EL2 — seed them with the real values so
    // the SMP core-id read (smp.rs) stays correct at EL1.
    mrs   x0, mpidr_el1
    msr   vmpidr_el2, x0
    mrs   x0, midr_el1
    msr   vpidr_el2, x0
    // CPTR_EL2 = 0x33FF: clear TFP (bit 10) so EL1/EL0 FP/SIMD does NOT trap to EL2 (the kernel is
    // +neon; the GUI blits NEON, memcpy/fmt autovectorize). CPTR_EL2.TFP takes precedence over
    // CPACR_EL1.FPEN and resets UNKNOWN, so this must be explicit; 0x33FF keeps the non-VHE RES1 bits
    // set (do NOT 'msr cptr_el2, xzr').
    mov   x0, #0x33ff
    msr   cptr_el2, x0
    // MDCR_EL2 = 0: don't route EL1 debug/PMU exceptions to the (now abandoned) EL2 vectors.
    msr   mdcr_el2, xzr
    // CNTHCTL_EL2 EL1PCTEN+EL1PCEN=1: let EL1 read CNTPCT / use CNTP_* without trapping to EL2
    // (timer.rs touches these every tick). CNTVOFF_EL2=0 so CNTVCT shares the physical timebase.
    mrs   x0, cnthctl_el2
    orr   x0, x0, #0x3
    msr   cnthctl_el2, x0
    msr   cntvoff_el2, xzr
    // CPACR_EL1.FPEN=0b11 (bits [21:20]): the EL1-side FP/SIMD enable.
    mov   x0, #(0b11 << 20)
    msr   cpacr_el1, x0
    // HCR_EL2 = RW only (bit 31): EL1 executes AArch64; IMO/FMO/AMO cleared so a physical IRQ taken at
    // EL1 targets EL1 natively (no EL2 routing). Bare write (HCR_EL2 resets 0 on A72), not an RMW.
    mov   x0, #(1 << 31)
    msr   hcr_el2, x0
    // Land at EL1h (SPx = SP_EL1) with DAIF masked; SP_EL1 = current SP so the stack is continuous;
    // ELR_EL2 = the return address (x30), so the eret returns to our caller now running at EL1.
    mov   x0, sp
    msr   sp_el1, x0
    mov   x0, #0x3c5
    msr   spsr_el2, x0
    msr   elr_el2, x30
    isb
    eret
"#
);

unsafe extern "C" {
    /// Drop this core EL2 -> EL1 and return to the caller now executing at EL1. Call at EL2 with the
    /// MMU OFF, before `enable_mmu`. See the naked asm above for the full sequence and rationale.
    pub fn drop_to_el1();
}

// --- BootInfo for the bare-metal path (no UEFI to provide one). ---

/// A usable RAM region for the kernel heap. Placed at 32 MiB, 64 MiB long (≥ the 48 MiB HEAP_SIZE),
/// which clears the kernel image + stack (at 0x80000, a few MiB) and the firmware/DTB structures in
/// low memory. The Pi 4 has ≥ 1 GiB, so this is always backed.
static mut MEM_REGIONS: [MemoryRegion; 1] = [MemoryRegion {
    phys_start: 0x0200_0000,
    page_count: 0x4000, // 64 MiB / 4 KiB
    kind: MemoryRegionKind::Usable,
}];

static mut BOOT_INFO: BootInfo = BootInfo {
    framebuffer_addr: 0,
    framebuffer_size: 0,
    framebuffer_info: FrameBufferInfo {
        width: 0,
        height: 0,
        stride: 0,
        bytes_per_pixel: 4,
        pixel_format: PixelFormat::Unknown,
    },
    physical_memory_offset: 0,
    dtb_addr: 0,
    dtb_size: 0,
    rsdp_addr: 0,
    memory_regions_addr: 0,
    memory_regions_len: 0,
    edid_native_width: 0,
    edid_native_height: 0,
    edid_source: 0,
    mode_action: 0,
};

/// Synthesize the BootInfo the kernel expects. `dtb_addr` carries the pointer the GPU ROM passed in
/// x0. Phase 2 asks the VideoCore GPU for a framebuffer over the mailbox and, on success, fills the
/// framebuffer fields so `kernel_main` brings up the full GUI on HDMI; if the mailbox call fails the
/// fields stay 0 and the kernel falls back to the serial-only console (Phase 1 behaviour).
pub fn build_boot_info(dtb: u64) -> &'static mut BootInfo {
    unsafe {
        let regions = &raw const MEM_REGIONS as u64;
        let bi = &raw mut BOOT_INFO;
        (*bi).dtb_addr = dtb;
        (*bi).memory_regions_addr = regions;
        (*bi).memory_regions_len = 1;

        // VideoCore mailbox framebuffer. Safe to call here: mmu_init() has run, so the mailbox MMIO
        // (Device-mapped at 0xFE00B880) and the cache maintenance the driver does both work. Serial
        // is up (PL011), so the driver's diagnostics reach the Debug Probe even though fbcon — which
        // it's about to make possible — isn't online yet.
        if let Some(fb) = super::mailbox::init_framebuffer() {
            (*bi).framebuffer_addr = fb.base;
            (*bi).framebuffer_size = fb.size;
            (*bi).framebuffer_info = fb.info;
        }
        &mut *bi
    }
}
