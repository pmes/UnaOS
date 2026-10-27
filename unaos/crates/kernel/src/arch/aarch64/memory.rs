use unaos_boot_info::BootInfo;
#[cfg(not(feature = "tegra"))]
use unaos_boot_info::MemoryRegionKind;

pub fn init(boot_info: &'static mut BootInfo) {
    serial_println!(":: AARCH64 Memory Init ::");
    
    let regions = unsafe {
        core::slice::from_raw_parts(
            boot_info.memory_regions_addr as *const unaos_boot_info::MemoryRegion,
            boot_info.memory_regions_len,
        )
    };
    
    // ORIN-VUG-RAS: on tegra, place the heap CARVEOUT-AWARE. NVIDIA's UEFI reports firewall-protected
    // DRAM carveouts as ordinary Usable memory, so the naive "first Usable region ≥ HEAP_SIZE" pick
    // below can back the heap with protected DRAM — a cached store into it faults on writeback with an
    // SNOC RAS Uncorrectable "Carveout" abort that powers the cores off (the vug lockup). The tegra
    // selector excludes both the UEFI non-Usable regions AND the DTB /reserved-memory carveouts, prints
    // a HEAP-GUARD witness line, and fails closed if no clean window exists. Gated to tegra so every
    // other aarch64 target (virt, pi) keeps the original selection byte-for-byte.
    #[cfg(feature = "tegra")]
    {
        match super::mmu_tegra::select_heap_region(regions, boot_info.dtb_addr, boot_info.dtb_size) {
            Some((s, sz)) => unsafe { crate::allocator::init_heap_raw(s as *mut u8, sz) },
            None => panic!("tegra: no carveout-clear DRAM window for the kernel heap"),
        }
    }

    #[cfg(not(feature = "tegra"))]
    {
        let mut heap_start = 0;
        let mut heap_size = 0;

        for region in regions {
            if region.kind == MemoryRegionKind::Usable {
                let size = (region.page_count * 4096) as usize;
                if size >= crate::allocator::HEAP_SIZE {
                    heap_start = region.phys_start;
                    heap_size = crate::allocator::HEAP_SIZE;
                    break;
                }
            }
        }

        if heap_size > 0 {
            unsafe {
                crate::allocator::init_heap_raw(heap_start as *mut u8, heap_size);
            }
        } else {
            panic!("Failed to find usable memory for heap");
        }
    }
}

/// FOCUSARM64 (SR180): the caller's address-space slot, at the path x86 names it (`arch::memory::current_slot`) — `wm::focussteal::note_arm` reads it on both desktops. The body is `xwin`'s (TTBR0_EL1's ASID).
#[cfg(feature = "aarch64_el0")]
pub use super::xwin::current_slot;
// ═══════════════════════════════════════════════════════════════════════════════════════════════
// DMAONEBODY (rmbp-ledger B992) — THE KERNEL'S ONE DMA ALLOCATOR, AARCH64 HOOKS (tail-appended; was DMAALLOCXHCI B988's twin).
// ═══════════════════════════════════════════════════════════════════════════════════════════════
/// The allocator's ONE body is `crate::dma` (DMAONEBODY B992); re-exported here so every driver's `crate::arch::memory::dma::`
/// path names it unchanged on both arches. This file keeps only aarch64's two hooks.
pub use crate::dma;

/// `dma`'s bus address of `[va, va + len)`: `va` itself when the span lies inside the identity-mapped heap (`init` hands
/// the heap to `init_heap_raw` at its PHYSICAL start; tegra: the carveout-clear window's start), `None` otherwise. This
/// half has no page walk.
pub fn dma_bus(va: u64, len: usize) -> Option<u64> {
    let (lo, hi) = crate::allocator::heap_bounds();
    let end = va.checked_add(len as u64)?;
    if va >= lo as u64 && end <= hi as u64 { Some(va) } else { None }
}

/// `dma`'s settle on a fresh zeroed buffer: the BCM2711 PCIe and Tegra XUSB paths do not snoop the data cache, so the
/// zeros are cleaned and invalidated to the PoC here, the ONE place, before any device can see the buffer.
pub fn dma_settle(va: u64, len: usize) {
    super::cache::clean_invalidate_range(va as usize, len);
}
