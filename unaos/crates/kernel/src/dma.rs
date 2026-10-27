//! CHARTER: Kernel — driver (the kernel's one DMA allocator: one arch-neutral body over two per-arch hooks; DMAONEBODY rmbp-ledger B992)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! DMAONEBODY (rmbp-ledger B992): THE KERNEL'S ONE DMA ALLOCATOR, one body for both arches. Built as two twins by
//! DMAALLOCONE (B986, x86), DMAALLOCXHCI (B988, aarch64) and DMAALLOCEHCI (B989, `free`); the twins differed in two
//! places only, now the arch's two hooks in its `memory.rs`: `dma_bus(va, len)`, the device's bus address of a heap
//! span (x86: `translate`, this kernel's one page walk over its identity-mapped heap; aarch64: the CPU address when
//! the span lies inside `allocator::heap_bounds()`, refused otherwise), and `dma_settle(va, len)`, run once on the
//! zeroed buffer before any device sees it (x86: coherent, nothing; aarch64: clean and invalidate to the PoC, since
//! the BCM2711 PCIe and Tegra XUSB paths do not snoop the data cache). Each arch's `memory.rs` re-exports this
//! module (`pub use crate::dma;`), so every driver's `crate::arch::memory::dma::` path names it unchanged.
//!
//! One allocator for device-visible memory, under the drivers' former private `alloc_zeroed` copies (nvme.rs, ahci.rs
//! and hda.rs `dma_alloc`, mt7925.rs `ring()`, wifi/bringup tx.rs/rx.rs, xhci, e1000, sdhc, the EHCI pools). Every
//! buffer is zeroed, aligned as asked, and comes from the kernel heap. A future IOMMU or non-identity map
//! (ZENBOOK-IOMMU) changes ONE hook per arch: `dma_bus`. Most devices hold their rings for the whole boot; an error
//! path that never handed a buffer to a running engine gives it back through `free` / `free_ident` (DMA-FREE, B989).

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

/// Zeroed, `align`-aligned device memory of `bytes` bytes. With `below_4g` the buffer must end at or below
/// 4 GiB (for a device that cannot program a high address dword); one that does not is freed before any
/// device sees it and the call returns `None` (a bounce path is owed, DMA-BOUNCE). `None` also for zero
/// bytes, an invalid layout, an exhausted heap, or an address the arch's `dma_bus` cannot resolve.
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
    let pa = crate::arch::memory::dma_bus(va, bytes);
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
    // The arch's settle (aarch64: clean and invalidate to the PoC, so the zeros reach DRAM and no stale line shadows
    // the device's later writes; x86: coherent, nothing), before any device can see the buffer.
    crate::arch::memory::dma_settle(va, bytes);
    Some(DmaBuf { va, pa: pa.unwrap_or(0), len: bytes, align })
}

/// The drivers' shim: one u64 that is both the CPU pointer and the bus address, which is how every converted
/// driver uses its buffer. It is the buffer's address when `pa == va` (the identity map) and 0 otherwise, which
/// each driver already treats as "no memory". The identity assumption lives here and nowhere else.
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

/// The boot's DMA census, printed once (at desktop-ready, `bootpace`'s `boot_line`, both arches) on a `census` / tests-at-boot build.
pub fn census() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Relaxed) {
        return;
    }
    crate::census_println!("[dma] census allocs={} bytes={} frees={}", ALLOCS.load(Relaxed), BYTES.load(Relaxed), FREES.load(Relaxed));
}

/// `tests dma`: alignment, zeroing, the below-4G refusal against a fake bound, and the identity shim's pa.
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
                ident_ok &= crate::arch::memory::dma_bus(b.va, b.len) == Some(b.pa) && b.pa == b.va;
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
