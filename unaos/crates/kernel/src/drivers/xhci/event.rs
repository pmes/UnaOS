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

use super::trb::Trb;

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct ErstEntry {
    pub ring_address: u64, // Physical address of the ring segment
    pub size: u16,         // Number of TRBs in this segment
    pub _rsvd: u16,
    pub _rsvd2: u32,
}

#[repr(C, align(64))]
pub struct ErstTable {
    pub entries: [ErstEntry; 1],
}

// Sized generously: the controller cannot post a completion event while the ring is
// full, and a synchronous bring-up burst (plus interleaved HID events) can fill a tiny
// ring before the main loop drains it. Must match the ERST segment size in
// init_interrupter().
pub const EVENT_RING_SIZE: usize = 256;

/// JETSON-XCARVE (boot-15 SNOC "Carveout Uncorrectable" FillWrite): the event ring's TRB segment is
/// HEAP-allocated (`super::ring::allocate_ring`, 64-byte aligned, zeroed ⇒ every slot == `Trb::new()`),
/// NOT an inline `[Trb; N]` array in this struct. When `EventRing` lived inline in the `static
/// EVENT_RING`, its TRBs sat in kernel-image `.bss` — inside the *bootloader*-chosen image load extent
/// (boot-15: `[0x25adea000, +0x18f000)`), which `HEAP-GUARD` does NOT vet for firewall carveouts the way
/// it vets the heap. The CPU's ring-construction store into that un-vetted `.bss` then FillWrite-RASed on
/// writeback (surfaced frames later by the VUGRAS DC-CIVAC sweep's cache churn; the fault ADDR is
/// record-format per the bit-63 XCARVE erratum, so it read as `0x26b900000` — near DRAM top — not as the
/// literal `.bss` PA). Every OTHER xHC DMA structure — command/transfer rings, DCBAA, scratchpad, per-slot
/// contexts, HID buffers — is heap-backed; this makes the event ring uniform with them, inside the
/// firewall-clean, DMA-window-resident heap.
#[repr(C)]
pub struct EventRing {
    /// Heap PA of the 64-byte-aligned ring segment (`EVENT_RING_SIZE` TRBs). The controller DMA-writes
    /// events here; the CPU reads them via the volatile accessors below.
    trbs: *mut Trb,
    pub dequeue_index: usize,
    pub cycle_bit: bool, // What we expect the hardware to write
}

unsafe impl Send for EventRing {}
unsafe impl Sync for EventRing {}

impl EventRing {
    pub fn new() -> Self {
        // Heap ring segment — see the struct doc for why the TRBs must NOT live in the static's .bss.
        // `allocate_ring` zero-fills, so every TRB starts as `Trb::new()` (all-zero), matching the old
        // inline `[Trb::new(); EVENT_RING_SIZE]` byte-for-byte.
        let trbs = super::ring::allocate_ring(EVENT_RING_SIZE) as *mut Trb;
        Self {
            trbs,
            dequeue_index: 0,
            cycle_bit: true, // xHCI starts writing 1s
        }
    }

    // Check if the current TRB at dequeue_index is fresh.
    // The event ring is written by the controller via DMA, so the control field MUST be
    // read with a volatile load — a plain read can be hoisted/cached by the compiler,
    // making a tight poll loop spin forever on a stale value.
    pub fn has_event(&self) -> bool {
        // Read the whole (aligned) TRB volatile — Trb is `packed`, so taking a reference
        // to an individual field is unaligned/illegal; copy it out, then read the field.
        let trb = unsafe { core::ptr::read_volatile(self.trbs.add(self.dequeue_index)) };
        let cycle_state = (trb.control & 1) != 0;
        cycle_state == self.cycle_bit
    }

    /// JB3 boot-8 experiment (Tegra-only): invalidate the CPU's cached copy of the whole
    /// ring (`dc civac` per line — clean+invalidate, safe for a ring the CPU only reads),
    /// then re-run the freshness check. If an event MATERIALIZES that the cached read
    /// missed, the controller's DMA writes are landing in DRAM and the failure is CPU-side
    /// cache snooping (the `dma-coherent` fabric handoff died with ExitBootServices) — not
    /// an SMMU/fabric drop.
    #[cfg(feature = "tegra")]
    pub fn has_event_after_invalidate(&self) -> bool {
        let base = self.trbs as usize;
        let end = base + EVENT_RING_SIZE * core::mem::size_of::<Trb>();
        let mut p = base & !63;
        while p < end {
            unsafe {
                core::arch::asm!("dc civac, {}", in(reg) p, options(nostack, preserves_flags))
            };
            p += 64;
        }
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
        self.has_event()
    }

    pub fn pop(&mut self) -> Option<Trb> {
        if !self.has_event() {
            return None;
        }

        // DMA-read ordering: `has_event` proved freshness from the TRB's control word; the full
        // read below loads the parameter/status words at DIFFERENT addresses, and aarch64 allows
        // load-load reordering across addresses — those loads could be satisfied with pre-DMA
        // stale data even though the cycle bit read fresh. `fence(Acquire)` lowers to `dmb ishld`
        // (the xHC is a coherent inner-shareable observer); on x86 (TSO) it is compiler-only,
        // zero codegen. Linux's event-ring handler issues the same `rmb()` after its cycle check.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

        // Volatile read of the DMA-written TRB (see has_event).
        let trb = unsafe { core::ptr::read_volatile(self.trbs.add(self.dequeue_index)) };

        // Advance
        self.dequeue_index += 1;
        if self.dequeue_index >= EVENT_RING_SIZE {
            self.dequeue_index = 0;
            self.cycle_bit = !self.cycle_bit; // Flip expectation
        }

        // Note: We will need to write the ERDP (Event Ring Dequeue Pointer)
        // back to hardware later to tell it we processed this slot.
        Some(trb)
    }

    /// Returns the physical address of the ring (identity-mapped; now a HEAP PA — see the struct doc).
    pub fn get_ptr(&self) -> u64 {
        self.trbs as u64
    }

    pub fn clear(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.trbs, 0, EVENT_RING_SIZE);
        }
    }
}
