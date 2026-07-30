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
use super::dma_coherency;


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
        // XHCI-COHERENCE: zeroed-handoff — `alloc_zeroed` writes the ring's zeros into (dirty) cache
        // lines; on a non-coherent bus the controller would otherwise read stale DRAM (a phantom
        // valid cycle bit in an un-pushed slot). Clean the whole zeroed ring to DRAM before the
        // controller can fetch from it. No-op x86.
        dma_coherency::clean(ptr as usize, num_trbs * core::mem::size_of::<Trb>());
        Self {
            trbs: ptr,
            num_trbs,
            enqueue_index: 0,
            cycle_bit: true, // xHCI starts with Consumer Cycle State = 1
        }
    }

    pub fn push_noop(&mut self) -> Result<usize, &'static str> {
        let index = self.enqueue_index;

        // BOT-RESCUE M2: honour the ring's LIVE cycle bit.
        //
        // This used to hard-code `true` ("Directive UNA-11-CYCLE") on the theory that a freshly
        // initialised ring needs a 0->1 transition to be visible. That is true only for the FIRST
        // No-Op on a virgin ring — where `cycle_bit` is already `true`, so the two agree and this
        // change is a no-op for every call the driver makes today (the command ring's bring-up
        // probe). It is a latent trap for any later call: after one wrap `cycle_bit` is false, the
        // hard-coded 1 would then be the STALE colour, the controller would treat the No-Op as an
        // un-produced slot, and the ring would silently stop being consumed (xHCI 1.2 §4.9.1 —
        // the Cycle bit is the sole producer/consumer handshake). Reading the field costs nothing
        // and removes the trap.
        unsafe {
            *self.trbs.add(index) = Trb::new_noop(self.cycle_bit);

            // FLUSH CACHE (Directive J11:FLUSH-01)
            let trb_ptr = self.trbs.add(index);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            // XHCI-COHERENCE: producer boundary — push the No-Op TRB out to DRAM before its
            // doorbell so the non-snooping controller fetches the fresh cycle bit (no-op on x86).
            dma_coherency::clean(trb_ptr as usize, core::mem::size_of::<Trb>());
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

    /// BOT-RESCUE M2: our own live enqueue position and cycle colour, for the timeout witness.
    /// The one pair of numbers that, read against the controller's TR Dequeue Pointer + DCS from
    /// the endpoint context, says whether the controller is BEHIND us (it has not fetched what we
    /// produced — a controller/endpoint fault) or level with us (it fetched everything and the
    /// DEVICE is silent). Pure reads.
    pub fn enqueue_index(&self) -> usize { self.enqueue_index }
    pub fn cycle_bit(&self) -> bool { self.cycle_bit }
    pub fn num_trbs(&self) -> usize { self.num_trbs }

    /// BOT-RESCUE M2: would enqueueing ONE more TRB lap the controller's dequeue pointer?
    ///
    /// xHCI 1.2 §4.9.1/§4.9.2: a Transfer Ring is a producer/consumer ring whose only full/empty
    /// discriminator is the Cycle bit, so the producer MUST NOT advance its enqueue pointer past
    /// the consumer's dequeue pointer — doing so overwrites TRBs the controller has not yet
    /// fetched and re-colours slots it is still walking. This ring tracked no consumer position at
    /// all: during BOT error recovery, where the endpoint is stalled and the controller's dequeue
    /// pointer is parked, a retry loop could push straight through it.
    ///
    /// `ctx_deq` is the raw Endpoint Context TR Dequeue Pointer field (low bits carry DCS), read
    /// by the caller from the OUTPUT device context. If it does not address a TRB inside THIS ring
    /// the answer is "no" — an unreadable consumer position must never manufacture a refusal on a
    /// healthy device.
    ///
    /// Margin: refuse while fewer than two slots would remain free, so the Link TRB slot the wrap
    /// path needs is always available. With a 16-TRB ring the refusal threshold is 14 outstanding
    /// TRBs; a healthy BOT transaction has at most ONE outstanding TRB (each stage is awaited to
    /// completion before the next is queued), so this predicate is false by construction on every
    /// healthy transfer and can only fire against a controller that has stopped consuming.
    pub fn would_lap(&self, ctx_deq: u64) -> bool {
        let deq = match self.index_of(ctx_deq & !0xFu64) {
            Some(i) => i,
            None => return false,
        };
        let n = self.num_trbs;
        // Outstanding = how far our enqueue pointer has run ahead of the controller's dequeue.
        let used = (self.enqueue_index + n - deq) % n;
        used + 2 >= n
    }

    /// BOT-RESCUE M2: return the ring to a known-clean producer state — every slot zeroed, enqueue
    /// at index 0, cycle bit back to the xHCI initial Consumer Cycle State of 1. Used ONLY by
    /// recovery escalation (a), which re-programs the endpoint context to point at this ring's
    /// base with DCS=1 in the same breath; continuing from wherever the failed transaction left
    /// the pointers would leave the driver's and the controller's ideas of the ring disagreeing
    /// about both position and colour.
    ///
    /// Safety: the caller must have stopped/reset the endpoint first, so the controller is not
    /// concurrently fetching from this ring.
    pub fn reset(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.trbs as *mut u8, 0, self.num_trbs * core::mem::size_of::<Trb>());
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            // XHCI-COHERENCE: the zeros must reach DRAM before the controller is pointed here, or a
            // non-snooping master could fetch a stale (valid-looking) cycle bit. No-op on x86.
            dma_coherency::clean(self.trbs as usize, self.num_trbs * core::mem::size_of::<Trb>());
        }
        self.enqueue_index = 0;
        self.cycle_bit = true;
    }

    fn write_trb(&mut self, index: usize, trb: Trb) {
        unsafe {
            let p = self.trbs.add(index);
            core::ptr::write_volatile(p, trb);
            // XHCI-COHERENCE: producer boundary — the controller (command ring or any transfer
            // ring) DMA-reads this TRB after its doorbell; clean the line to DRAM so a non-snooping
            // master sees it (cycle bit included). No-op on coherent x86_64.
            dma_coherency::clean(p as usize, core::mem::size_of::<Trb>());
        }
    }

    /// Returns the physical address of the ring (assuming identity map for now)
    pub fn get_ptr(&self) -> u64 {
        self.trbs as u64
    }

    /// USB-WRITE-2: the (physical address, Dequeue Cycle State) the controller should resume at
    /// after a bulk STALL is cleared — the CURRENT enqueue slot (the next TRB the host will push),
    /// carrying this ring's live cycle bit. Feeds a Set TR Dequeue Pointer command so a halted
    /// endpoint restarts PAST the faulted TRB instead of re-fetching it. Pairs with a Reset
    /// Endpoint command (Halted -> Stopped) and a device-side CLEAR_FEATURE(ENDPOINT_HALT).
    pub fn dequeue_reset_target(&self) -> (u64, u32) {
        let phys = self.trbs as u64 + (self.enqueue_index as u64) * 16;
        (phys, if self.cycle_bit { 1 } else { 0 })
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

    /// BOT-PHASE: how many TRBs lie between the controller's dequeue pointer and our enqueue
    /// pointer, and how many of those the controller would still consider VALID (produced).
    ///
    /// `ctx_deq` is the raw Endpoint Context TR Dequeue Pointer field, low bits carrying the
    /// Dequeue Cycle State — the same reading `would_lap` takes, and subject to the same caveat
    /// GUARD-STATE established: **it only means anything while the endpoint is NOT Running.** The
    /// caller is responsible for that; this is pure arithmetic over device memory.
    ///
    /// Returns `(gap, live)`:
    ///   * `gap` — slots from the controller's dequeue index up to (not including) ours;
    ///   * `live` — of those, the ones whose stored Cycle bit equals the cycle the CONSUMER expects
    ///     at that position. Per xHCI 1.2 §4.9.1 the Cycle bit is the entire producer/consumer
    ///     handshake, so `live` is precisely "TRBs the controller will execute if its doorbell
    ///     rings again". The walk starts from the DCS carried in `ctx_deq` and toggles the expected
    ///     cycle whenever it steps over a Link TRB with Toggle Cycle set, exactly as the controller
    ///     does.
    ///
    /// `live > 0` at an error exit is a STRANDED TRANSFER DESCRIPTOR: a CBW, data stage or CSW the
    /// transaction never retired, which the next doorbell would replay into a device whose BOT phase
    /// machine has moved on. That is the phase-desync mechanism this arc closes; `live == 0` after
    /// the recovery's Set TR Dequeue Pointer is the proof it closed.
    ///
    /// `None` if `ctx_deq` does not address a TRB inside this ring (an unreadable consumer position
    /// must never be reported as a strand).
    pub fn strand_scan(&self, ctx_deq: u64) -> Option<(usize, usize)> {
        let deq = self.index_of(ctx_deq & !0xFu64)?;
        let n = self.num_trbs;
        let gap = (self.enqueue_index + n - deq) % n;
        let mut expect = (ctx_deq & 1) as u32;
        let mut live = 0usize;
        let mut i = deq;
        for _ in 0..gap {
            let trb = unsafe { core::ptr::read_volatile(self.trbs.add(i)) };
            if (trb.control & 1) == expect {
                live += 1;
            }
            // Link TRB (type 6) with Toggle Cycle (bit 1) flips the consumer's expected colour.
            if ((trb.control >> 10) & 0x3F) == 6 && (trb.control & (1 << 1)) != 0 {
                expect ^= 1;
            }
            i += 1;
            if i >= n {
                i = 0;
            }
        }
        Some((gap, live))
    }

    /// BOTEV: does `phys` address a TRB inside THIS ring? Pure predicate over `index_of`, no reads
    /// of device memory. Used by the BOT recovery witness to name which pipe (bulk IN vs bulk OUT)
    /// the timed-out transfer was waiting on, from nothing but the stranded TRB address.
    pub fn contains(&self, phys: u64) -> bool {
        self.index_of(phys).is_some()
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
            // XHCI-COHERENCE: producer boundary — the defused No-Op must reach DRAM before the ring
            // is restarted (doorbell 0), else the controller re-reads the wedged command. No-op x86.
            dma_coherency::clean(p as usize, core::mem::size_of::<Trb>());
        }
        true
    }
}
