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
    /// ONSET-3: did the most recent `push` cross the Link TRB and start a new lap? Read by the BOT
    /// witnesses (`wrapped=`, `wrapped_tx=`, `wrap_db=`). This is the honest form of the `idx == 0`
    /// test the driver used to make: index 0 is also the FIRST slot of a virgin ring, where no wrap
    /// happened and no Link was crossed. Instrumentation only — nothing branches on it.
    wrapped_on_last_push: bool,
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
        let mut r = Self {
            trbs: ptr,
            num_trbs,
            enqueue_index: 0,
            cycle_bit: true, // xHCI starts with Consumer Cycle State = 1
            wrapped_on_last_push: false,
        };
        r.place_link_trb();
        r
    }

    /// ONSET-3: write the ring's Link TRB into the LAST slot, once, at construction (and again at
    /// `reset`). Address = this ring's base, TC (Toggle Cycle) = 1, cycle = the ring's initial
    /// Consumer Cycle State.
    ///
    /// **Why this exists — CORRECTNESS HARDENING, NOT THE ONSET FIX.** State this plainly because
    /// the arc's first draft got it wrong. In the gr9 capture (boot 4) the OUT pipe's Stop Endpoint
    /// posted **cc=26, Stopped, with a VALID TRB Transfer Length** (`resync stopev dci=2 dir=out
    /// ev_stopped=1`), and the post-stop TR Dequeue Pointer sat ON the awaited data TRB at index 0.
    /// Per xHCI 1.2 §4.6.9 / §6.4.5 that pair says the controller had already crossed this Link,
    /// fetched the data TD and was EXECUTING it when the driver stopped the endpoint; `db_out_d=0`
    /// over the whole ~6 s wait says it was owed no further doorbell. The ring-wrap onset is
    /// therefore NOT a controller that failed to cross a Link, and nothing here is claimed to fix
    /// it. (Note also that cc=27 alone does not carry that reading: the same recovery shows
    /// `ev_stopped_li=1` on the IN pipe whose own strand scan read `gap=0 live=0` with no CSW yet
    /// pushed — an idle endpoint. Only cc=26-with-a-valid-length is the in-progress discriminator.)
    ///
    /// What is nevertheless wrong with the old arrangement, on its own merits: `push` AUTHORED the
    /// whole Link TRB lazily, at the instant the enqueue index reached `num_trbs - 1`. For the whole
    /// of every lap before that instant the last slot held a *stale data TRB* — a Normal TRB still
    /// pointing at a real DMA buffer, with a real length, carrying whatever cycle bit the previous
    /// lap left there. Two slots ahead of a producer that is refused only at the `would_lap` margin,
    /// that is a slot the controller could walk into with a matching stored cycle and EXECUTE: a
    /// replayed DMA against a buffer the driver has since reused. No capture shows that happening,
    /// and this is not offered as an explanation of one; it is a window that should not exist.
    ///
    /// With the Link pre-placed, slot `num_trbs - 1` is a Link TRB from the first instruction the
    /// controller could possibly fetch from this ring. The only thing `push` still writes there is
    /// the single cycle-bit dword that arms it for the lap it terminates (`arm_link_trb`), so the
    /// worst reading the controller can ever take from that slot is "Link, not yet produced" —
    /// the correct, safe reading, rather than a stale Normal TRB.
    fn place_link_trb(&mut self) {
        let idx = self.num_trbs - 1;
        let mut link_trb = Trb::new();
        link_trb.parameter = self.get_ptr();
        // Type 6 (Link TRB), TC=1 (Toggle Cycle), cycle = the lap colour this Link terminates.
        let mut control = (6u32 << 10) | (1 << 1);
        if self.cycle_bit {
            control |= 1;
        }
        link_trb.control = control;
        self.write_trb(idx, link_trb);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }

    /// ONSET-3: arm the pre-placed Link TRB for the lap it terminates by writing ONLY its cycle-bit
    /// dword — the address, the type and the Toggle Cycle bit were laid down at construction and
    /// never change for the life of the ring.
    ///
    /// `colour` is the cycle the CONSUMER expects at this slot on the lap now ending, i.e. the
    /// producer's cycle bit BEFORE the wrap toggles it (xHCI 1.2 §4.9.1: a TRB is "produced" when
    /// its Cycle bit equals the consumer's Cycle State, and §4.11.5.1: a Link TRB with TC set
    /// toggles that state as the controller steps over it).
    fn arm_link_trb(&mut self, colour: bool) {
        let idx = self.num_trbs - 1;
        let mut control = (6u32 << 10) | (1 << 1);
        if colour {
            control |= 1;
        }
        unsafe {
            // `Trb` is `repr(C, packed)`, so `control` sits at byte offset 12 of a 16-byte TRB in a
            // 64-byte-aligned allocation — naturally 4-byte aligned. Address it as bytes rather than
            // through a packed field reference, which would be an unaligned-reference hazard.
            let p = (self.trbs as *mut u8).add(idx * 16 + 12) as *mut u32;
            core::ptr::write_volatile(p, control);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            // XHCI-COHERENCE: producer boundary — the armed Link must reach DRAM before the doorbell
            // that follows it, or a non-snooping master reads the previous lap's colour and parks at
            // the ring end. Clean the whole TRB line (the offset is inside it). No-op on x86.
            dma_coherency::clean(self.trbs.add(idx) as usize, core::mem::size_of::<Trb>());
        }
    }

    pub fn push_noop(&mut self) -> Result<usize, &'static str> {
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
        // ONSET-3: route through the one producer path, so the No-Op can never be written INTO the
        // Link slot. The old body advanced `enqueue_index` blindly and wrapped at `num_trbs`, which
        // with a pre-placed Link TRB would have overwritten it with a Command No-Op and left the
        // ring with no wrap edge at all. `push_noop` is only ever called once (the command ring's
        // bring-up probe, at index 0), so this is behaviour-identical today; it is here so the
        // invariant holds for any later caller.
        let index = self.produce(Trb::new_noop(self.cycle_bit))?;

        unsafe {
            let trb_ptr = self.trbs.add(index);
            let control_val = core::ptr::read_volatile(trb_ptr).control;
            serial_println!("xHCI DEBUG: CMD TRB = {:#x}", control_val);
        }

        Ok(index)
    }

    /// Enqueue one TRB, crossing the Link TRB when the producer reaches the ring's last usable slot.
    ///
    /// ONSET-3 changed two things about the wrap, and neither is claimed to explain the metal
    /// ring-wrap onset (see `place_link_trb` — the capture's `ev_stopped=1` refutes the
    /// controller-never-crossed-the-Link reading):
    ///
    ///  1. The Link TRB is no longer authored here. It was placed at construction; the wrap writes
    ///     only its cycle-bit dword (`arm_link_trb`), so the last slot is never a stale data TRB.
    ///  2. **The payload is written BEFORE the Link is armed.** The old order was: author the Link
    ///     (making the path to index 0 live), then toggle, then write index 0. Between those two
    ///     stores index 0 still held the CURRENT lap's own earlier TRB — same address space, the
    ///     colour the consumer was about to stop expecting — so a controller walking the ring
    ///     concurrently could cross a live Link into a slot the producer had not yet rewritten.
    ///     Arming the Link last makes the wrap edge atomic from the consumer's side: index 0 is
    ///     already valid for the new lap before anything can route the controller to it.
    ///
    /// `wrapped_on_last_push` records whether this call crossed the Link, for the BOT witness.
    pub fn push(&mut self, trb: Trb) -> Result<usize, &'static str> {
        self.produce(trb)
    }

    fn produce(&mut self, mut trb: Trb) -> Result<usize, &'static str> {
        // The last slot is the Link TRB's, permanently. Reaching it means this push starts a new lap
        // at index 0 under the toggled colour.
        let wrapping = self.enqueue_index == self.num_trbs - 1;
        let index = if wrapping { 0 } else { self.enqueue_index };
        let colour = if wrapping { !self.cycle_bit } else { self.cycle_bit };

        // 1. Set the Cycle Bit on the TRB — the hardware's sole test that it is valid and fresh.
        if colour {
            trb.control |= 1;
        } else {
            trb.control &= !1;
        }

        // 2. Write the payload TRB (and flush it to DRAM — `write_trb` cleans the line).
        self.write_trb(index, trb);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 3. Only now arm the Link for the lap that just ended, and adopt the new colour/position.
        if wrapping {
            self.arm_link_trb(self.cycle_bit);
            self.cycle_bit = colour;
            self.enqueue_index = 1;
            super::BOT_RING_WRAPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        } else {
            self.enqueue_index += 1;
        }
        self.wrapped_on_last_push = wrapping;

        Ok(index)
    }

    /// ONSET-3: did the most recent `push` cross the Link TRB? The driver used to infer this from
    /// `push` returning index 0, which is also what a virgin ring's very first push returns — so the
    /// `wrapped=` witness over-counted by exactly one per ring per boot. Pure read.
    pub fn wrapped_on_last_push(&self) -> bool { self.wrapped_on_last_push }

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
    ///
    /// ONSET-3 — capacity accounting with a PERMANENTLY occupied Link slot. Nothing below changes,
    /// and that is the point worth writing down rather than re-deriving:
    ///   * `enqueue_index` still ranges over `0..num_trbs`, and still RESTS on `num_trbs - 1` (the
    ///     Link slot) between the push that filled `num_trbs - 2` and the push that wraps. The Link
    ///     slot was never a data slot under the old lazy scheme either — `push` claimed it for the
    ///     Link the moment the producer arrived there — so pre-placing the Link removes zero usable
    ///     capacity. `used` and the `used + 2 >= n` threshold are arithmetic over the same domain.
    ///   * The two reserved slots are still the right number: one is the Link, and one is the
    ///     full/empty discriminator a cycle-bit ring cannot do without. A ring of `n` TRBs therefore
    ///     carries at most `n - 2` outstanding data TRBs, exactly as before.
    ///   * `index_of` accepts the Link slot's address, so a controller parked ON the Link is counted
    ///     as a consumer position like any other — which is correct: it has not yet crossed.
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
        self.wrapped_on_last_push = false;
        // ONSET-3: the zeroing above erased the Link TRB along with everything else. Put it back
        // before the caller re-points the endpoint context at this ring's base, or the ring would
        // have no wrap edge at all and the producer's first lap would run off the end into a slot
        // holding zeros (control = 0 -> TRB type 0, Reserved).
        self.place_link_trb();
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

    /// VUGRAS: the ring's `[lo, hi)` byte span (base PA .. base + num_trbs*sizeof(Trb)). Named in the
    /// RAS localizer's decode table so a fault ADDR inside a transfer ring is attributable.
    pub fn span(&self) -> (usize, usize) {
        let lo = self.trbs as usize;
        (lo, lo + self.num_trbs * core::mem::size_of::<Trb>())
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

    /// ONSET-2 (M2 witness 4): the four raw dwords of the TRB at ring index `index`, as they read
    /// back from DRAM. `None` if the index is out of range.
    ///
    /// **What it exists for.** `TIMEOUT-TRB` prints only the AWAITED TRB, so the Link TRB's cycle
    /// bit, its Toggle Cycle bit and its target address have only ever been reasoned about from
    /// `push`'s source — never observed. `push` writes the Link TRB **lazily**, at the moment the
    /// enqueue index reaches `num_trbs - 1`, so for the whole of every lap the last slot holds a
    /// stale TRB carrying the colour the controller is no longer expecting. Whether that is what
    /// stops the controller at a wrap is the arc's ranked hypothesis, and it cannot be settled
    /// without the bytes.
    ///
    /// The caller is expected to dump index `num_trbs - 1` (the Link slot) and `num_trbs - 2` (the
    /// TRB immediately ahead of it) when a timeout reports `wrapped=true`.
    ///
    /// HEALTHY-BUT-IDLE READING: both slots always hold *something* — the Link slot holds a Type 6
    /// TRB with TC set, and index `n-2` holds whatever data TRB last occupied it. This is forensic
    /// detail, not an alarm: nothing about the presence or absence of these dwords is itself a
    /// fault, and the line is only ever printed at a timeout.
    /// Invalidate is the caller's business (rings are identity-mapped and coherent on x86).
    ///
    /// ONSET-3 UPDATE — the paragraph above used to say the Link slot holds a Type 6 TRB "once the
    /// ring has wrapped at least once", because `push` authored it lazily. It is now placed at
    /// construction, so the Link slot reads as Type 6 + TC from the very first fetch and its ONLY
    /// varying field is the cycle bit. That narrows what this witness can say and is worth stating:
    /// `dw3` on the Link slot is now a pure cycle-colour reading — `(dw3 & 1)` equal to the ring's
    /// live `cycle_bit` means the Link is armed for the CURRENT lap (the producer has already
    /// wrapped past it), and unequal means it terminates the lap in progress. A malformed or absent
    /// Link, which the old lazy scheme could produce and which the gr9 capture was read against, is
    /// no longer a reachable state — so observing one would mean memory corruption, not a driver
    /// ordering bug.
    pub fn trb_raw(&self, index: usize) -> Option<(u32, u32, u32, u32)> {
        if index >= self.num_trbs {
            return None;
        }
        let t = unsafe { core::ptr::read_volatile(self.trbs.add(index)) };
        Some(((t.parameter & 0xFFFF_FFFF) as u32, (t.parameter >> 32) as u32, t.status, t.control))
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
    /// If the enqueue index is the ring's last slot, that slot holds the Link TRB (ONSET-3: placed
    /// at construction rather than written by the next `push`), so pointing at it is still correct
    /// and needs no special case. What the controller finds there is a Link carrying the PREVIOUS
    /// lap's colour — `arm_link_trb` only refreshes it at the wrap — so with DCS set to this ring's
    /// live `cycle_bit` the controller reads "not yet produced", stops, and resumes on the next
    /// doorbell once the wrapping `push` has armed it. That is the same outcome the lazy scheme gave
    /// (where the slot held a stale DATA TRB), one state safer: the controller can never mistake it
    /// for executable work.
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
        // ONSET-3: never defuse the Link slot. A command TRB can never live there — `produce`
        // reserves `num_trbs - 1` for the Link — so this cannot fire on any address the abort path
        // legitimately produces; it is here so that a corrupted or misread `phys` is REFUSED rather
        // than silently honoured into destroying the ring's only wrap edge.
        if idx == self.num_trbs - 1 {
            return false;
        }
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
