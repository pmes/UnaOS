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

const EVENT_RING_SIZE: usize = 16;

#[repr(C, align(64))]
pub struct EventRing {
    pub trbs: [Trb; EVENT_RING_SIZE],
    pub dequeue_index: usize,
    pub cycle_bit: bool, // What we expect the hardware to write
}

impl EventRing {
    pub const fn new() -> Self {
        Self {
            trbs: [Trb::new(); EVENT_RING_SIZE],
            dequeue_index: 0,
            cycle_bit: true, // xHCI starts writing 1s
        }
    }

    // Check if the current TRB at dequeue_index is fresh
    pub fn has_event(&self) -> bool {
        let trb = &self.trbs[self.dequeue_index];
        let cycle_state = (trb.control & 1) != 0;
        cycle_state == self.cycle_bit
    }

    pub fn pop(&mut self) -> Option<Trb> {
        if !self.has_event() {
            return None;
        }

        let trb = self.trbs[self.dequeue_index];

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

    /// Returns the physical address of the ring (assuming identity map)
    pub fn get_ptr(&self) -> u64 {
        self.trbs.as_ptr() as u64
    }

    pub fn clear(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.trbs.as_mut_ptr(), 0, EVENT_RING_SIZE);
        }
    }
}
