// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Data-cache maintenance for non-coherent DMA on the Raspberry Pi 4.
//
// The bare-metal kernel runs with the MMU on and RAM mapped Normal **Write-Back cacheable** (see
// arch/aarch64/boot.rs — cacheable RAM is mandatory because `ldxr/stxr` are CONSTRAINED
// UNPREDICTABLE on Device memory, and every spinlock/atomic depends on them). But the VideoCore
// GPU (the mailbox property channel) and the HVS display controller read RAM **directly** — they
// do not snoop the Cortex-A72's data cache. So any buffer the CPU prepares for them, or that they
// fill for the CPU, needs explicit cache maintenance to the Point of Coherency (PoC):
//
//   * before the GPU reads a buffer we wrote  -> clean   (DC CVAC): push our writes out to RAM;
//   * after  the GPU wrote a buffer we'll read -> invalidate (DC IVAC): drop stale cached copies
//     so the next read re-fetches the GPU's data from RAM.
//
// These are no-ops in QEMU (which models no caches and is always coherent) but load-bearing on
// real silicon — exactly the class of "works in QEMU, black screen on metal" bug this kernel has
// been bitten by before. They operate on whole cache lines; the range is rounded out to line
// boundaries, so callers pass the natural [addr, addr+len) of their buffer.

use core::arch::asm;

/// Smallest data-cache line size in bytes, from CTR_EL0.DminLine (log2 of the line size in
/// 32-bit words). Cortex-A72 reports 64 bytes; reading it at runtime keeps these correct on any
/// other ARMv8 core (e.g. the Jetson's Cortex-A78) without a hardcode.
#[inline]
fn dcache_line_size() -> usize {
    let ctr: u64;
    unsafe { asm!("mrs {}, CTR_EL0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    let dmin_line = (ctr >> 16) & 0xF;
    4usize << dmin_line
}

/// Clean (write back) the data cache over `[addr, addr+len)` to the PoC, then `dsb sy`. Call
/// after writing a buffer the GPU/DMA will read, so our stores are visible in RAM.
#[inline]
pub fn clean_range(addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let line = dcache_line_size();
    let mut p = addr & !(line - 1);
    let end = addr + len;
    while p < end {
        unsafe { asm!("dc cvac, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += line;
    }
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// COMPOSITE-2 — clean (write back) `rows` strided spans of `row_len` bytes, starting at `addr`
/// and stepping `stride` bytes per row, then ONE `dsb sy` for the lot. The rectangular form of
/// [`clean_range`]: the compositor's post-blit clean covers a window's BOX, whose rows are
/// sub-spans of the panel's scanlines, and cleaning them as one contiguous range forces the
/// full-width margins into the sweep — 3.7x the bytes for the bench's 514-wide box. Per-row
/// `clean_range` calls would instead pay one `DSB` per row; the barrier belongs to the rect, not
/// the row, which is why this is its own primitive rather than a loop at the call site.
#[inline]
pub fn clean_rows(addr: usize, row_len: usize, rows: usize, stride: usize) {
    if row_len == 0 || rows == 0 {
        return;
    }
    let line = dcache_line_size();
    for r in 0..rows {
        let start = addr + r * stride;
        let mut p = start & !(line - 1);
        let end = start + row_len;
        while p < end {
            unsafe { asm!("dc cvac, {}", in(reg) p, options(nostack, preserves_flags)) };
            p += line;
        }
    }
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Invalidate the data cache over `[addr, addr+len)` to the PoC, then `dsb sy`. Call before
/// reading a buffer the GPU/DMA just wrote, so the next read misses our (now stale) cached copy
/// and re-fetches from RAM.
///
/// Safety/correctness: `DC IVAC` discards any dirty line in the range without writing it back, so
/// the caller must ensure the CPU has no un-cleaned writes pending in `[addr, addr+len)` (in our
/// use the buffer was `clean_range`d before the GPU ran and untouched by the CPU since, so its
/// lines are clean). When in doubt use [`clean_invalidate_range`].
///
/// WC-D uses this deliberately, and the discard is the POINT rather than a hazard to be avoided: the
/// compositor's scan-out verification must read what is actually in RAM, so it invalidates without
/// cleaning. A `clean_invalidate_range` there would write the CPU's dirty lines back first and thereby
/// repair a missing/short framebuffer flush before measuring it — the instrument would heal the defect it
/// exists to detect. `video::wm::verify_window` redraws the window afterwards to restore the rect.
#[allow(dead_code)]
#[inline]
pub fn invalidate_range(addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let line = dcache_line_size();
    let mut p = addr & !(line - 1);
    let end = addr + len;
    while p < end {
        unsafe { asm!("dc ivac, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += line;
    }
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Clean **and** invalidate over `[addr, addr+len)` to the PoC, then `dsb sy`. The safe superset
/// of the two above (writes back dirty lines, then drops them).
#[inline]
pub fn clean_invalidate_range(addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let line = dcache_line_size();
    let mut p = addr & !(line - 1);
    let end = addr + len;
    while p < end {
        unsafe { asm!("dc civac, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += line;
    }
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Instruction-cache line size in bytes, from CTR_EL0.IminLine (bits[3:0], log2 of the line size in
/// 32-bit words). This is a SEPARATE field from DminLine — they can differ on other Armv8 cores (e.g.
/// the Jetson's Cortex-A78), so the I-cache invalidate loop must NOT reuse `dcache_line_size`.
/// Cortex-A72 reports 64 B for both.
#[inline]
fn icache_line_size() -> usize {
    let ctr: u64;
    unsafe { asm!("mrs {}, CTR_EL0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    let imin_line = ctr & 0xF;
    4usize << imin_line
}

/// Make CPU-written bytes in `[addr, addr+len)` executable — the freshly-loaded/self-modifying-code
/// maintenance the ARM ARM (DDI0487 B2.7.4) requires. We wrote the bytes through the (cacheable) EL1
/// data side; a later instruction fetch (here, EL0 over the same identity VA) can miss them unless we
/// clean the D-cache to the Point of Unification (DC CVAU) and invalidate the stale I-cache lines
/// (IC IVAU). The order is load-bearing and needs BOTH barriers: DC CVAU → `dsb ish` → IC IVAU →
/// `dsb ish` → `isb`. Without the interior `dsb ish`, IC IVAU may retire against a not-yet-cleaned
/// line and the fetch pulls old bytes; the closing `isb` flushes the fetched-ahead pipeline. IVAU/CVAU
/// broadcast Inner-Shareable, so the code is coherent even when it later runs on another core. A no-op
/// in QEMU (no caches) but mandatory on real silicon.
#[inline]
pub fn icache_sync_range(addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let end = addr + len;
    // 1. Clean the D-cache to the PoU, strided by the D-cache line size.
    let dline = dcache_line_size();
    let mut p = addr & !(dline - 1);
    while p < end {
        unsafe { asm!("dc cvau, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += dline;
    }
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    // 2. Invalidate the I-cache to the PoU, strided by the I-cache line size (IminLine, not DminLine).
    let iline = icache_line_size();
    let mut p = addr & !(iline - 1);
    while p < end {
        unsafe { asm!("ic ivau, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += iline;
    }
    unsafe { asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
}
