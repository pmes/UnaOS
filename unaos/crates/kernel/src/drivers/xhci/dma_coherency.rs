// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! XHCI-COHERENCE — the single DMA-coherency seam for the shared xHCI driver.
//!
//! Every xHCI DMA structure (command ring, event ring + ERST, DCBAA, device/input
//! contexts, scratchpad array + buffers, transfer rings, transfer data buffers) is
//! allocated from the kernel's Write-Back **cacheable** heap. On an I/O-coherent host
//! — x86_64, and the Intel xHCI on the 2012 rMBP — the CPU caches and the controller's
//! DMA path snoop each other, so a bare `fence`/`dmb` is enough and these functions are
//! not needed. On a **non-coherent** bus they are mandatory: the BCM2711 PCIe root
//! complex → VIA VL805 path does NOT snoop the A72 data caches (PIUSB-8), and the
//! Tegra234 XUSB fabric loses its `dma-coherent` handoff at ExitBootServices — in both
//! cases a PCIe/fabric master neither sees the CPU's dirty cache lines nor is seen when
//! it DMA-writes DRAM. The CPU must therefore:
//!   * **clean** (write-back) memory it has produced BEFORE the controller reads it
//!     (a TRB pushed before its doorbell; a context/DCBAA/ERST/scratchpad before the
//!     command or the Run bit that makes the controller fetch it);
//!   * **invalidate** (drop the stale line) memory the controller has produced BEFORE
//!     the CPU reads it (an event-ring TRB at dequeue; a device context or transfer
//!     completion buffer after its transfer event).
//!
//! These are the ONLY cache-maintenance primitives in `drivers/xhci`. They are gated by
//! `target_arch`, NOT by a board feature, so BOTH aarch64 boards (Pi 4, Jetson) receive
//! the maintenance from ONE seam; on x86_64 every function is an `#[inline(always)]`
//! empty body that compiles to nothing, leaving the coherent path byte-identical to the
//! pre-seam driver.
//!
//! Non-coherent addressing assumption (holds on both aarch64 boards): the xHCI DMA heap
//! is identity-mapped, so the CPU virtual address the driver holds equals the physical
//! address the controller DMAs to — a `dc c*vac` by VA maintains the right line.

/// AArch64 D-cache line granule. A `DC {C,CI,I}VAC` operates on the whole line
/// containing the address; both the A72 (BCM2711 / Pi 4) and the Carmel + A78AE (Orin)
/// use 64-byte lines, so a 64-byte stride cleans/invalidates a span without gaps.
#[cfg(target_arch = "aarch64")]
const CACHE_LINE: usize = 64;

/// Clean (write-back to the Point of Coherency) the D-cache lines spanning
/// `[addr, addr + len)`, then `dsb sy`. Call AFTER the CPU has written DMA memory the
/// (non-snooping) controller will READ — command/transfer-ring TRBs before their
/// doorbell; input contexts before their command; DCBAA / ERST / scratchpad before the
/// Run bit. `dc cvac` keeps the (clean) line resident, so it is correct for memory the
/// CPU may keep reading.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn clean(addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let mut p = addr & !(CACHE_LINE - 1);
    let end = addr + len;
    while p < end {
        // SAFETY: cache maintenance by VA on identity-mapped DMA memory; no memory access.
        unsafe { core::arch::asm!("dc cvac, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += CACHE_LINE;
    }
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Clean **and** invalidate the D-cache lines spanning `[addr, addr + len)` to the Point
/// of Coherency, then `dsb sy`. Use at a bidirectional / zeroed-handoff boundary — a
/// freshly-zeroed event ring or output (device) context the CPU has just written but the
/// controller will next own and DMA-write: the clean pushes the zeros to DRAM, the
/// invalidate guarantees the CPU's next read observes the controller's data, not the
/// stale zero line. Also the safe choice at the event-ring dequeue check (the ring is
/// CPU-read-only, so there is nothing dirty to lose).
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn clean_inval(addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let mut p = addr & !(CACHE_LINE - 1);
    let end = addr + len;
    while p < end {
        // SAFETY: cache maintenance by VA on identity-mapped DMA memory; no memory access.
        unsafe { core::arch::asm!("dc civac, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += CACHE_LINE;
    }
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Invalidate (drop, without write-back) the D-cache lines spanning `[addr, addr + len)`,
/// then `dsb sy`. Use ONLY on memory the CPU has NOT written since the controller last
/// owned it — a transfer completion / data buffer, or a device context, that the
/// controller DMA-wrote and the CPU is about to read. `dc ivac` discards the line, so it
/// must never cover CPU-dirty bytes (use `clean_inval` there). On this driver's paths the
/// buffers invalidated here are controller-produced and CPU-read-only between transfers.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn inval(addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let mut p = addr & !(CACHE_LINE - 1);
    let end = addr + len;
    while p < end {
        // SAFETY: cache maintenance by VA on identity-mapped DMA memory; no memory access.
        unsafe { core::arch::asm!("dc ivac, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += CACHE_LINE;
    }
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

// ── Coherent targets (x86_64 and any non-aarch64): the CPU caches and the controller's
//    DMA path snoop each other, so cache maintenance is unnecessary. These bodies are
//    empty and `#[inline(always)]`, so every call site compiles to nothing and the x86
//    codegen is byte-identical to the pre-seam driver. ──
#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
pub fn clean(_addr: usize, _len: usize) {}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
pub fn clean_inval(_addr: usize, _len: usize) {}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
pub fn inval(_addr: usize, _len: usize) {}
