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
// DMAALLOCXHCI (rmbp-ledger B988) — THE KERNEL'S ONE DMA ALLOCATOR, AARCH64 HALF (tail-appended).
// ═══════════════════════════════════════════════════════════════════════════════════════════════
/// The aarch64 twin of x86's `arch::memory::dma` (B986), same API, so `crate::arch::memory::dma` names ONE
/// allocator on both arches (arch/mod.rs's cfg_if re-export is the shared place). Two differences, both the
/// arch's: (1) the bus address. `init` hands the heap to `init_heap_raw` at its PHYSICAL start (tegra: the
/// carveout-clear window's start), so a heap address IS its bus address; this half has no page walk, so the
/// bus address is the CPU address when the whole buffer lies inside `allocator::heap_bounds()` (the identity
/// window) and the buffer is refused otherwise. (2) coherency. The BCM2711 PCIe and Tegra XUSB paths do not
/// snoop the data cache, so the zeroed buffer is cleaned and invalidated to the PoC here, the ONE place
/// (`super::cache::clean_invalidate_range`), before any device can see it. One arch-neutral body with per-arch
/// bus/settle hooks is owed (DMA-ONEBODY); `free` / `free_ident` give memory back, as on x86 (DMA-FREE, B989).
pub mod dma {
    use alloc::alloc::{alloc_zeroed, dealloc, Layout};
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

    /// A device-visible buffer: the CPU's address, the device's bus address, and the length in bytes.
    pub struct DmaBuf {
        pub va: u64,
        pub pa: u64,
        pub len: usize,
        /// The alignment it was allocated with, so `free` can rebuild its layout (DMAALLOCEHCI B989).
        pub align: usize,
    }

    /// Successful `alloc` calls since boot (the KAT's uncounted `take`s excluded).
    static ALLOCS: AtomicU64 = AtomicU64::new(0);
    /// Bytes those calls handed out.
    static BYTES: AtomicU64 = AtomicU64::new(0);
    /// Buffers given back through `free` / `free_ident` (DMAALLOCEHCI B989).
    static FREES: AtomicU64 = AtomicU64::new(0);

    /// The 32-bit bus window a `below_4g` device can address (the Pi's RC inbound window is RAM @ 0, 4 GiB).
    const LIMIT_4G: u64 = 0x1_0000_0000;

    /// The bus address of `[va, va + len)`: `va` itself when the span lies inside the identity-mapped heap.
    fn bus(va: u64, len: usize) -> Option<u64> {
        let (lo, hi) = crate::allocator::heap_bounds();
        let end = va.checked_add(len as u64)?;
        if va >= lo as u64 && end <= hi as u64 { Some(va) } else { None }
    }

    /// Zeroed, `align`-aligned device memory of `bytes` bytes, cleaned to the PoC. With `below_4g` the buffer
    /// must end at or below 4 GiB; one that does not is freed before any device sees it and the call returns
    /// `None` (DMA-BOUNCE owed). `None` also for zero bytes, an invalid layout, an exhausted heap, or an address
    /// outside the identity-mapped heap.
    pub fn alloc(bytes: usize, align: usize, below_4g: bool) -> Option<DmaBuf> {
        let b = take(bytes, align, if below_4g { Some(LIMIT_4G) } else { None })?;
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(b.len as u64, Relaxed);
        Some(b)
    }

    /// The allocation itself, uncounted (the KAT's entry). `limit`: the bus address the buffer must end at or below.
    fn take(bytes: usize, align: usize, limit: Option<u64>) -> Option<DmaBuf> {
        if bytes == 0 {
            return None;
        }
        let layout = Layout::from_size_align(bytes, align).ok()?;
        // SAFETY: a non-zero-sized, valid layout.
        let p = unsafe { alloc_zeroed(layout) };
        if p.is_null() {
            return None;
        }
        let va = p as u64;
        let pa = bus(va, bytes);
        let fits = match (pa, limit) {
            (None, _) => false,
            (Some(pa), Some(lim)) => pa.checked_add(bytes as u64).map_or(false, |end| end <= lim),
            (Some(_), None) => true,
        };
        if !fits {
            // SAFETY: allocated just above with this layout and never handed to a device.
            unsafe { dealloc(p, layout) };
            return None;
        }
        // The zeros reach DRAM and no stale line survives to shadow the device's later writes.
        super::super::cache::clean_invalidate_range(va as usize, bytes);
        Some(DmaBuf { va, pa: pa.unwrap_or(0), len: bytes, align })
    }

    /// The drivers' shim: one u64 that is both the CPU pointer and the bus address (0 = no memory), as on x86.
    pub fn ident(bytes: usize, align: usize) -> u64 {
        match alloc(bytes, align, false) {
            Some(b) if b.pa == b.va => b.va,
            _ => 0,
        }
    }

    /// Give a buffer back (DMA-FREE, DMAALLOCEHCI B989): deallocated with the layout it was allocated with
    /// (`len`, `align`) and counted in the census. A zero length or an invalid layout is a no-op.
    ///
    /// # Safety
    /// `b` came from `alloc` (or is `free_ident`'s rebuild of an `ident` buffer) and is given back once, and no
    /// device can still reach it: never handed to one, or its engine is stopped / the command that would have used
    /// it never took effect. `DmaBuf`'s fields are public, so this is the caller's promise, not the type's.
    pub unsafe fn free(b: DmaBuf) {
        if b.len == 0 {
            return;
        }
        let Ok(layout) = Layout::from_size_align(b.len, b.align) else {
            return;
        };
        // SAFETY: the caller's contract above; the layout is the one `take` allocated with.
        unsafe { dealloc(b.va as *mut u8, layout) };
        FREES.fetch_add(1, Relaxed);
    }

    /// `ident`'s inverse: give back a buffer `ident(bytes, align)` returned as `va` (0 is a no-op, as `ident`'s
    /// "no memory"). The identity assumption (`pa == va`) stays in this module.
    ///
    /// # Safety
    /// As `free`: `va` came from `ident(bytes, align)` with these exact arguments and no device can still reach it.
    pub unsafe fn free_ident(va: u64, bytes: usize, align: usize) {
        if va == 0 {
            return;
        }
        // SAFETY: the caller's contract above.
        unsafe { free(DmaBuf { va, pa: va, len: bytes, align }) };
    }

    /// The boot's DMA census, printed once on a `census` / tests-at-boot build (the caller is owed: DMA-CENSUS-ARM).
    pub fn census() {
        static DONE: AtomicBool = AtomicBool::new(false);
        if DONE.swap(true, Relaxed) {
            return;
        }
        crate::census_println!("[dma] census allocs={} bytes={} frees={}", ALLOCS.load(Relaxed), BYTES.load(Relaxed), FREES.load(Relaxed));
    }

    /// `tests dma`: alignment, zeroing, the below-4G refusal against a fake bound, and the identity window.
    pub fn selftest() {
        let mut align_ok = true;
        let mut zero_ok = true;
        let mut ident_ok = true;
        for &(bytes, align) in &[(4096usize, 4096usize), (1000, 64), (8192, 4096)] {
            match take(bytes, align, None) {
                Some(b) => {
                    align_ok &= b.va % align as u64 == 0 && b.len == bytes;
                    // SAFETY: `b.len` bytes at `b.va` were allocated just above.
                    let s = unsafe { core::slice::from_raw_parts(b.va as *const u8, b.len) };
                    zero_ok &= s.iter().all(|&x| x == 0);
                    ident_ok &= bus(b.va, b.len) == Some(b.pa) && b.pa == b.va;
                    // SAFETY: allocated by `take` with exactly this layout; no device saw it.
                    unsafe { dealloc(b.va as *mut u8, Layout::from_size_align_unchecked(bytes, align)) };
                }
                None => {
                    align_ok = false;
                }
            }
        }
        // The fake bound: 4 KiB. A 4 KiB buffer could end there only at address 0, which no allocation returns.
        let refused = match take(4096, 4096, Some(0x1000)) {
            Some(b) => {
                // SAFETY: allocated by `take` (4096, 4096); no device saw it.
                unsafe { dealloc(b.va as *mut u8, Layout::from_size_align_unchecked(4096, 4096)) };
                false
            }
            None => true,
        };
        let zero_len = take(0, 64, None).is_none();
        // DMAALLOCEHCI (B989): the free leg — one counted alloc given back through `free`; both counters move by one.
        let (a0, f0) = (ALLOCS.load(Relaxed), FREES.load(Relaxed));
        let free_ok = match alloc(4096, 4096, false) {
            Some(b) => {
                // SAFETY: allocated just above by `alloc`; no device saw it.
                unsafe { free(b) };
                ALLOCS.load(Relaxed) == a0 + 1 && FREES.load(Relaxed) == f0 + 1
            }
            None => false,
        };
        let pass = align_ok && zero_ok && ident_ok && refused && zero_len && free_ok;
        serial_println!(
            ":: DMA: align={} zero={} below4g={} ident={} free={} census={}/{}/{} -> {} ::",
            if align_ok { "ok" } else { "bad" },
            if zero_ok { "ok" } else { "bad" },
            if refused { "refused" } else { "bad" },
            if ident_ok { "ok" } else { "bad" },
            if free_ok { "ok" } else { "bad" },
            ALLOCS.load(Relaxed),
            BYTES.load(Relaxed),
            FREES.load(Relaxed),
            if pass { "PASS" } else { "FAIL" }
        );
    }
}
