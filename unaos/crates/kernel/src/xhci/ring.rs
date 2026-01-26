use super::trb::Trb;
use crate::serial_println;

const RING_SIZE: usize = 16;

#[repr(C, align(64))] // xHCI requires 64-byte alignment for ring segments
pub struct CommandRing {
    trbs: [Trb; RING_SIZE],
    enqueue_index: usize,
    cycle_bit: bool,
}

impl CommandRing {
    pub const fn new() -> Self {
        Self {
            trbs: [Trb::new(); RING_SIZE],
            enqueue_index: 0,
            cycle_bit: true, // xHCI starts with Consumer Cycle State = 1
        }
    }

    pub fn push_noop(&mut self) -> Result<usize, &'static str> {
        let index = self.enqueue_index;

        // FORCE CYCLE BIT = 1 (Directve UNA-11-CYCLE)
        // We ignore self.cycle_bit for this specific initialization to ensure
        // the hardware sees the transition.
        self.trbs[index] = Trb::new_noop(true);

        // FLUSH CACHE (Directive J11:FLUSH-01)
        let trb_ptr = &self.trbs[index] as *const Trb;
        unsafe {
            core::arch::x86_64::_mm_clflush(trb_ptr as *const u8);
            let control_val = (*trb_ptr).control;
            serial_println!("xHCI DEBUG: CMD TRB = {:#x}", control_val);
        }

        // Advance
        self.enqueue_index += 1;

        // Simple wrap-around logic (The real driver needs a Link TRB here)
        if self.enqueue_index >= RING_SIZE {
            self.enqueue_index = 0;
            self.cycle_bit = !self.cycle_bit; // Flip the color
        }

        Ok(index)
    }

    pub fn push(&mut self, mut trb: Trb) -> Result<usize, &'static str> {
        let index = self.enqueue_index;

        // 1. Set the Cycle Bit on the TRB
        // The hardware checks this bit to verify the TRB is valid and fresh.
        if self.cycle_bit {
            trb.control |= 1;
        } else {
            trb.control &= !1;
        }

        // 2. Write TRB to Ring
        self.trbs[index] = trb;

        // 3. Flush Cache (Safety)
        unsafe {
            let trb_ptr = &self.trbs[index] as *const Trb;
            core::arch::x86_64::_mm_clflush(trb_ptr as *const u8);
        }

        // 4. Advance
        self.enqueue_index += 1;
        if self.enqueue_index >= RING_SIZE {
            // UNA-18-SLOT: Naive wrap. We are not using Link TRBs yet.
            // This is safe ONLY because we are sending < 16 commands.
            self.enqueue_index = 0;
            self.cycle_bit = !self.cycle_bit;
        }

        Ok(index)
    }

    /// Returns the physical address of the ring (assuming identity map for now)
    pub fn get_ptr(&self) -> u64 {
        self.trbs.as_ptr() as u64
    }
}
