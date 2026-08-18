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
use super::dma_coherency;

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

#[repr(C, align(64))]
pub struct EventRing {
    pub trbs: [Trb; EVENT_RING_SIZE],
    pub dequeue_index: usize,
    pub cycle_bit: bool, // What we expect the hardware to write
    /// PIUSB-43: total TRBs ever consumed from this ring (monotonic, wrap-proof — `dequeue_index`
    /// alone is mod-256 and cannot distinguish "no events" from "exactly 256k events"). Read-only
    /// telemetry for the enum-portsc witness; costs one integer increment per pop.
    pub popped: u64,
}

unsafe impl Send for EventRing {}
unsafe impl Sync for EventRing {}

impl EventRing {
    pub const fn new() -> Self {
        Self {
            trbs: [Trb::new(); EVENT_RING_SIZE],
            dequeue_index: 0,
            cycle_bit: true, // xHCI starts writing 1s
            popped: 0,
        }
    }

    // Check if the current TRB at dequeue_index is fresh.
    // The event ring is written by the controller via DMA, so the control field MUST be
    // read with a volatile load — a plain read can be hoisted/cached by the compiler,
    // making a tight poll loop spin forever on a stale value.
    pub fn has_event(&self) -> bool {
        // XHCI-COHERENCE (consumer boundary): the controller DMA-writes event TRBs into this ring.
        // On a non-coherent bus (Pi 4 PCIe, Tegra XUSB post-EBS) the CPU's cached line for the
        // current dequeue slot can be stale — the freshly-DMA'd cycle bit never observed — so a
        // tight poll spins forever. Invalidate the dequeue TRB's line(s) before the volatile read so
        // the CPU sees DRAM. This is the general aarch64 seam that SUPERSEDES the old tegra-only
        // `has_event_after_invalidate` (identical `dc civac` + `dsb`, now covering the Pi too). The
        // ring is CPU-read-only, so clean+invalidate loses nothing; on x86_64 this is a no-op and the
        // read below is unchanged.
        dma_coherency::clean_inval(
            &self.trbs[self.dequeue_index] as *const Trb as usize,
            core::mem::size_of::<Trb>(),
        );
        // Read the whole (aligned) TRB volatile — Trb is `packed`, so taking a reference
        // to an individual field is unaligned/illegal; copy it out, then read the field.
        let trb = unsafe { core::ptr::read_volatile(&self.trbs[self.dequeue_index]) };
        let cycle_state = (trb.control & 1) != 0;
        cycle_state == self.cycle_bit
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
        let trb = unsafe { core::ptr::read_volatile(&self.trbs[self.dequeue_index]) };

        // Advance
        self.popped = self.popped.wrapping_add(1); // PIUSB-43 consumed-count witness
        self.dequeue_index += 1;
        if self.dequeue_index >= EVENT_RING_SIZE {
            self.dequeue_index = 0;
            self.cycle_bit = !self.cycle_bit; // Flip expectation
        }

        // Note: We will need to write the ERDP (Event Ring Dequeue Pointer)
        // back to hardware later to tell it we processed this slot.
        Some(trb)
    }

    /// Returns the physical address of the ring (assuming identity map)
    pub fn get_ptr(&self) -> u64 {
        self.trbs.as_ptr() as u64
    }

    /// BOT-RESCUE M2: return the event ring to its post-`new()` state — every TRB zeroed AND the
    /// consumer position/colour reset to the xHCI initial expectation (index 0, expecting cycle 1).
    ///
    /// Two latent bugs fixed here. `write_bytes(ptr, 0, EVENT_RING_SIZE)` zeroed 256 **bytes**, not
    /// 256 **TRBs** — 16 of the 256 slots, leaving 240 stale entries whose cycle bits still read as
    /// the colour the consumer expects, so the next `has_event()` would report a fresh event that is
    /// a replay of an old one. And it reset neither `dequeue_index` nor `cycle_bit`, so even a fully
    /// zeroed ring would be consumed from the middle with the wrong expected colour.
    ///
    /// Currently UNCALLED: nothing in the driver clears the event ring after bring-up (the ring is
    /// created once by `EventRing::new` and consumed for the life of the boot). It is fixed anyway
    /// because a two-line "clear the ring" helper that silently corrupts the consumer handshake is
    /// exactly the trap a future controller-reset path would step in, and it costs nothing to close.
    /// The ERDP is NOT written here: the caller owns re-publishing the dequeue pointer to hardware
    /// (`advance_erdp`), and a helper that touched MMIO would not be safe to call before the
    /// interrupter exists.
    pub fn clear(&mut self) {
        unsafe {
            core::ptr::write_bytes(
                self.trbs.as_mut_ptr() as *mut u8,
                0,
                EVENT_RING_SIZE * core::mem::size_of::<Trb>(),
            );
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            // XHCI-COHERENCE: the controller DMA-writes this ring; push the zeros out to DRAM so a
            // non-snooping master does not later fetch our dirty lines over its own events. No-op x86.
            dma_coherency::clean(
                self.trbs.as_ptr() as usize,
                EVENT_RING_SIZE * core::mem::size_of::<Trb>(),
            );
        }
        self.dequeue_index = 0;
        self.cycle_bit = true;
        self.popped = 0;
    }
}
