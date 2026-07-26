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

use core::alloc::Layout;
use super::trb::Trb;


const RING_SIZE: usize = 16;

pub fn allocate_ring(num_trbs: usize) -> *mut u8 {
    unsafe {
        let size = num_trbs * 16;
        let layout = Layout::from_size_align(size, 64).unwrap();
        alloc::alloc::alloc_zeroed(layout)
    }
}

pub fn allocate_buffer(size: usize) -> *mut u8 {
    unsafe {
        let layout = Layout::from_size_align(size, 64).unwrap();
        alloc::alloc::alloc_zeroed(layout)
    }
}

#[repr(C, align(64))] // xHCI requires 64-byte alignment for ring segments
pub struct TransferRing {
    trbs: *mut Trb,
    num_trbs: usize,
    enqueue_index: usize,
    cycle_bit: bool,
}

unsafe impl Send for TransferRing {}
unsafe impl Sync for TransferRing {}

impl TransferRing {
    pub fn new(num_trbs: usize) -> Self {
        let ptr = allocate_ring(num_trbs) as *mut Trb;
        Self {
            trbs: ptr,
            num_trbs,
            enqueue_index: 0,
            cycle_bit: true, // xHCI starts with Consumer Cycle State = 1
        }
    }

    pub fn push_noop(&mut self) -> Result<usize, &'static str> {
        let index = self.enqueue_index;

        // FORCE CYCLE BIT = 1 (Directve UNA-11-CYCLE)
        // We ignore self.cycle_bit for this specific initialization to ensure
        // the hardware sees the transition.
        unsafe {
            *self.trbs.add(index) = Trb::new_noop(true);

            // FLUSH CACHE (Directive J11:FLUSH-01)
            let trb_ptr = self.trbs.add(index);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            let control_val = (*trb_ptr).control;
            serial_println!("xHCI DEBUG: CMD TRB = {:#x}", control_val);
        }

        // Advance
        self.enqueue_index += 1;

        // Simple wrap-around logic (The real driver needs a Link TRB here)
        if self.enqueue_index >= self.num_trbs {
            self.enqueue_index = 0;
            self.cycle_bit = !self.cycle_bit; // Flip the color
        }

        Ok(index)
    }

    pub fn push(&mut self, mut trb: Trb) -> Result<usize, &'static str> {
        if self.enqueue_index == self.num_trbs - 1 {
            // Need to insert Link TRB to wrap around
            let mut link_trb = Trb::new();
            link_trb.parameter = self.get_ptr();
            // Type 6 (Link TRB), TC=1 (Toggle Cycle)
            let mut control = (6 << 10) | (1 << 1);
            if self.cycle_bit {
                control |= 1;
            }
            link_trb.control = control;
            
            self.write_trb(self.enqueue_index, link_trb);
            unsafe {
                core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            }
            
            self.enqueue_index = 0;
            self.cycle_bit = !self.cycle_bit;
        }

        let index = self.enqueue_index;

        // 1. Set the Cycle Bit on the TRB
        // The hardware checks this bit to verify the TRB is valid and fresh.
        if self.cycle_bit {
            trb.control |= 1;
        } else {
            trb.control &= !1;
        }

        // 2. Write TRB to Ring
        self.write_trb(index, trb);

        // 3. Flush Cache (Safety)
        unsafe {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }

        // 4. Advance
        self.enqueue_index += 1;

        Ok(index)
    }

    fn write_trb(&mut self, index: usize, trb: Trb) {
        unsafe {
            core::ptr::write_volatile(self.trbs.add(index), trb);
        }
    }

    /// Returns the physical address of the ring (assuming identity map for now)
    pub fn get_ptr(&self) -> u64 {
        self.trbs as u64
    }

    /// Ring index of the TRB at physical address `phys`, if it lies inside this ring.
    fn index_of(&self, phys: u64) -> Option<usize> {
        let base = self.trbs as u64;
        if phys < base {
            return None;
        }
        let off = phys - base;
        let idx = (off / 16) as usize;
        if off % 16 != 0 || idx >= self.num_trbs {
            return None;
        }
        Some(idx)
    }

    /// The cycle bit currently stored in the TRB at `phys` (1 if out of range — a safe
    /// default for composing a CRCR RCS). Used by the command-abort handshake.
    pub fn trb_cycle(&self, phys: u64) -> u32 {
        match self.index_of(phys) {
            Some(idx) => unsafe { core::ptr::read_volatile(self.trbs.add(idx)).control & 1 },
            None => 1,
        }
    }

    /// The value an xHCI **Set TR Dequeue Pointer** command (xHCI 1.2 §4.6.10 / §6.4.3.9) needs
    /// to resynchronize the controller's dequeue pointer with THIS ring's enqueue pointer: the
    /// physical address of the next slot the driver will `push` into, OR'd with the ring's current
    /// cycle bit as DCS (Dequeue Cycle State).
    ///
    /// BOT error recovery uses it after a failed stage: the controller's dequeue pointer is then
    /// parked somewhere behind the enqueue pointer, with one or more stranded TRBs (a queued CBW /
    /// data / CSW the transaction never retired) between them. Restarting the endpoint without
    /// moving the dequeue pointer would re-execute those stranded TRBs against a device that has
    /// just been reset — the desynchronisation the recovery exists to end. Pointing the controller
    /// at the enqueue slot discards exactly the stranded TRBs and nothing else.
    ///
    /// If the enqueue index is the ring's last slot, the next `push` writes a Link TRB there; the
    /// controller reads it, follows it to index 0 and toggles its cycle — so pointing at that slot
    /// is still correct and needs no special case.
    pub fn enqueue_ptr_dcs(&self) -> u64 {
        let dcs = if self.cycle_bit { 1u64 } else { 0u64 };
        (self.get_ptr() + (self.enqueue_index as u64) * 16) | dcs
    }

    /// Overwrite the TRB at `phys` IN PLACE with a Command No-Op (TRB type 23), preserving
    /// its cycle bit. Command-abort recovery: after CRCR.CA stops the ring, the controller's
    /// dequeue pointer still references the aborted command, and restarting the ring would
    /// re-execute it — Linux's trb_to_noop does exactly this defusal. Returns false if
    /// `phys` is not inside this ring.
    pub fn replace_with_noop(&mut self, phys: u64) -> bool {
        let idx = match self.index_of(phys) {
            Some(i) => i,
            None => return false,
        };
        unsafe {
            let p = self.trbs.add(idx);
            let cycle = core::ptr::read_volatile(p).control & 1;
            core::ptr::write_volatile(p, Trb { parameter: 0, status: 0, control: (23 << 10) | cycle });
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }
        true
    }
}
