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

use unaos_boot_info::{BootInfo, FrameBufferInfo, MemoryRegion, MemoryRegionKind, PixelFormat};

/// A single Level-1 translation table: 512 entries × 8 bytes = one 4 KiB page. With a 4 KiB granule
/// and a 39-bit VA (TCR T0SZ=25) the top lookup level is L1 and each entry maps a 1 GiB block, so
/// these 512 entries cover the whole 512 GiB VA — we only fill the first four (0–4 GiB). Lives in
/// BSS (zeroed by `_start` before we fill it).
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1: PageTable = PageTable([0; 512]);

// --- Translation attributes (ARM ARM DDI0487; values cross-checked against the research pass). ---
// MAIR_EL2: AttrIdx 0 = Normal Inner/Outer Write-Back non-transient (0xFF); AttrIdx 1 =
// Device-nGnRnE (0x00).
const MAIR_EL2_VAL: u64 = 0xFF;

// TCR_EL2 (non-VHE "short" format): T0SZ=25 (39-bit VA → L1 top level, 1 GiB blocks), IRGN0=ORGN0=WB
// (0b01), SH0=Inner-shareable (0b11), TG0=4 KiB (0b00), PS=36-bit/64 GiB (0b001), plus the format's
// RES1 bits 23 and 31.
const TCR_EL2_VAL: u64 = 25            // T0SZ
    | (0b01 << 8)                      // IRGN0 = WB
    | (0b01 << 10)                     // ORGN0 = WB
    | (0b11 << 12)                     // SH0   = inner shareable
    | (0b00 << 14)                     // TG0   = 4 KiB
    | (0b001 << 16)                    // PS    = 36-bit
    | (1 << 23)                        // RES1
    | (1 << 31); // RES1

// L1 block descriptor lower attributes: bits[1:0]=0b01 (block), AttrIndx[4:2], AP[7:6]=0b00 (RW at
// EL2), SH[9:8], AF=bit10.
const DESC_BLOCK: u64 = 0b01;
const DESC_AF: u64 = 1 << 10;
const SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 0 << 2; // AttrIdx 0
const ATTR_DEVICE: u64 = 1 << 2; // AttrIdx 1
const DESC_XN: u64 = 1 << 54; // execute-never (EL2 regime: bit 54)

/// Normal, cacheable, inner-shareable, executable block (RAM where the kernel/heap/stack live).
const fn ram_block(pa: u64) -> u64 {
    pa | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_BLOCK
}
/// Device-nGnRnE, execute-never block (the peripheral window: PL011 0xFE201000, GIC 0xFF841000,
/// mailbox 0xFE00B880 all sit in the 0xC000_0000–0xFFFF_FFFF GiB).
const fn device_block(pa: u64) -> u64 {
    pa | DESC_XN | DESC_AF | ATTR_DEVICE | DESC_BLOCK
}

/// Build the identity map and turn the MMU on. Called (with the MMU still off) from `__rust_boot`
/// after BSS has been zeroed. Plain volatile writes + system-register moves only — no atomics, no
/// locks, nothing that needs the MMU we're about to enable.
pub unsafe fn mmu_init() {
    let table = &raw mut L1 as *mut u64;
    // 0–1 GiB and 1–3 GiB: RAM (Normal). On an 8 GiB Pi there is RAM above 3 GiB too, but the kernel
    // never uses it (heap is in low RAM), so the top GiB is given to the peripherals as Device.
    table.add(0).write_volatile(ram_block(0x0000_0000));
    table.add(1).write_volatile(ram_block(0x4000_0000));
    table.add(2).write_volatile(ram_block(0x8000_0000));
    table.add(3).write_volatile(device_block(0xC000_0000));

    let ttbr0 = &raw const L1 as u64;
    unsafe {
        core::arch::asm!(
            "msr MAIR_EL2, {mair}",
            "msr TCR_EL2,  {tcr}",
            "msr TTBR0_EL2, {ttbr}",
            "dsb sy",
            "isb",
            // Enable MMU (M=bit0), data cache (C=bit2), instruction cache (I=bit12) via read-modify-
            // write so SCTLR_EL2's RES1 bits are preserved.
            "mrs {tmp}, SCTLR_EL2",
            "orr {tmp}, {tmp}, #(1 << 0)",
            "orr {tmp}, {tmp}, #(1 << 2)",
            "orr {tmp}, {tmp}, #(1 << 12)",
            "msr SCTLR_EL2, {tmp}",
            "isb",
            mair = in(reg) MAIR_EL2_VAL,
            tcr = in(reg) TCR_EL2_VAL,
            ttbr = in(reg) ttbr0,
            tmp = out(reg) _,
            options(nostack, preserves_flags),
        );
    }
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
