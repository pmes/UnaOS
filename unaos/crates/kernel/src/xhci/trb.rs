#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub const fn new() -> Self {
        Self { parameter: 0, status: 0, control: 0 }
    }

    // A "No Op" command is the safest way to test the ring.
    // Type ID for No Op is 23.
    pub fn new_noop(cycle_bit: bool) -> Self {
        let mut t = Self::new();
        // TRB Type 23 starts at bit 10 of the control field
        // Bit 5 is IOC (Interrupt On Completion)
        // Cycle bit is bit 0
        let type_val = 23u32 << 10;
        let ioc = 1u32 << 5;
        let cycle = if cycle_bit { 1 } else { 0 };

        t.control = type_val | ioc | cycle;
        t
    }
}
