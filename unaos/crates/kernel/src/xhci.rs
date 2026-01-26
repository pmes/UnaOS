pub mod trb;
pub mod ring;
pub mod event;
pub mod context;

use ring::CommandRing;
use self::trb::Trb;
use self::event::{EventRing, ErstEntry, ErstTable};
use self::context::{InputContext, DeviceContext};
use spin::Mutex;
use crate::serial_println;
use x86_64::PhysAddr;

pub static COMMAND_RING: Mutex<CommandRing> = Mutex::new(CommandRing::new());
pub static EVENT_RING: Mutex<EventRing> = Mutex::new(EventRing::new());
// UNA-21-ACCELERATE: Bulk Transport Rings
pub static BULK_IN_RING: Mutex<CommandRing> = Mutex::new(CommandRing::new());
pub static BULK_OUT_RING: Mutex<CommandRing> = Mutex::new(CommandRing::new());

pub static mut ERST_TABLE: ErstTable = ErstTable { entries: [ErstEntry { ring_address: 0, size: 0, _rsvd: 0, _rsvd2: 0 }] };

#[repr(C, align(64))]
pub struct Dcbaap {
    ptrs: [u64; 256],
}

// Static, zero-initialized "Parking Lot"
pub static mut DCBAAP_TABLE: Dcbaap = Dcbaap { ptrs: [0; 256] };

// UNA-18-ADDRESS: Parking Lot for Contexts and EP0 Ring
pub static mut INPUT_CONTEXT: InputContext = InputContext::new();
pub static mut OUTPUT_CONTEXT: DeviceContext = DeviceContext::new();
pub static mut EP0_RING: [Trb; 16] = [Trb::new(); 16];

#[repr(C, align(64))]
pub struct DescriptorBuffer {
    pub data: [u8; 64],
}

// UNA-19-BUFFER: Aligned buffer for descriptor data
pub static mut DESCRIPTOR_BUFFER: DescriptorBuffer = DescriptorBuffer { data: [0; 64] };

#[repr(C, align(64))]
pub struct DataBuffer {
    pub data: [u8; 512],
}

// UNA-21-STORAGE: 512-byte buffer for Sector 0
pub static mut DATA_BUFFER: DataBuffer = DataBuffer { data: [0; 512] };

// Store Physical Address of the Event Ring for Runtime ERDP updates
static mut EVENT_RING_PHYS_BASE: u64 = 0;

/// THE GREAT UNIFICATION
/// Rings the xHCI Doorbell using raw assembly to ensure
/// strict ordering and immediate execution.
///
/// # Safety
/// Direct MMIO write. The address must be valid.
#[inline(always)]
pub unsafe fn ring_doorbell_asm(doorbell_addr: u64, target: u32) {
    serial_println!("xHCI [ASM]: Ringing Doorbell at {:#x} with Target {}", doorbell_addr, target);
    core::arch::asm!("mfence", options(nostack, preserves_flags));
    core::arch::asm!(
        "mov [{0}], {1:e}",
        in(reg) doorbell_addr,
        in(reg) target,
        options(nostack, preserves_flags)
    );
    core::arch::asm!("mfence", options(nostack, preserves_flags));
}

pub struct XhciController {
    base_addr: usize,
    op_base: usize,
    pending_port_id: u8,
    configuring_slot: u8, // UNA-21: Track which slot is being configured
    // Physical Addresses for Contexts (provided by main.rs via init_contexts)
    input_context_phys: u64,
    output_context_phys: u64,
    ep0_ring_phys: u64,
    descriptor_phys: u64,
    event_ring_phys_base: u64,
    // UNA-21: Bulk Phys Addrs
    bulk_in_ring_phys: u64,
    bulk_out_ring_phys: u64,
    data_buffer_phys: u64,
    // EP0 Management
    ep0_enqueue_index: usize,
    ep0_cycle_state: bool,
}

impl XhciController {
    pub unsafe fn new(base_addr: usize) -> Self {
        let cap_ptr = base_addr as *const u32;
        let cap_word = core::ptr::read_volatile(cap_ptr);

        let cap_length = (cap_word & 0xFF) as u8;
        let version = (cap_word >> 16) as u16;

        let op_base = base_addr + cap_length as usize;

        // Log it to verify we aren't seeing ghosts anymore
        serial_println!("xHCI: CapBase={:#x}, OpBase={:#x}, Version={:#x}", base_addr, op_base, version);

        XhciController {
            base_addr,
            op_base,
            pending_port_id: 0,
            configuring_slot: 0,
            input_context_phys: 0,
            output_context_phys: 0,
            ep0_ring_phys: 0,
            descriptor_phys: 0,
            event_ring_phys_base: 0,
            bulk_in_ring_phys: 0,
            bulk_out_ring_phys: 0,
            data_buffer_phys: 0,
            ep0_enqueue_index: 0,
            ep0_cycle_state: true, // Rings start with Cycle=1
        }
    }

    pub fn init_contexts(&mut self, input_phys: u64, output_phys: u64, ep0_phys: u64, desc_phys: u64, event_ring_phys: u64, bulk_in_phys: u64, bulk_out_phys: u64, data_phys: u64) {
        // 1. Log Input
        serial_println!("xHCI: Contexts Init. Input={:#x}, Event={:#x}", input_phys, event_ring_phys);

        // 2. Assign Struct
        self.input_context_phys = input_phys;
        self.output_context_phys = output_phys;
        self.ep0_ring_phys = ep0_phys;
        self.descriptor_phys = desc_phys;
        self.event_ring_phys_base = event_ring_phys;
        self.bulk_in_ring_phys = bulk_in_phys;
        self.bulk_out_ring_phys = bulk_out_phys;
        self.data_buffer_phys = data_phys;

        // 3. UPDATE THE STATIC GLOBAL (Critical for Polling)
        unsafe {
            EVENT_RING_PHYS_BASE = event_ring_phys;
        }
    }

    pub fn send_noop_command(&mut self) -> Result<usize, &'static str> {
        COMMAND_RING.lock().push_noop()
    }

    pub fn send_command(&mut self, trb: Trb) -> Result<usize, &'static str> {
        let res = COMMAND_RING.lock().push(trb);
        if res.is_ok() {
            // Ring the Doorbell for the Host Controller (Slot 0)
            // Target 0 = Command Ring
            self.ring_doorbell(0, 0);
        }
        res
    }

    fn read_portsc(&self, port_id: u8) -> u32 {
        unsafe {
            let port_offset = 0x400 + (port_id as usize - 1) * 0x10;
            let portsc_ptr = (self.op_base + port_offset) as *const u32;
            core::ptr::read_volatile(portsc_ptr)
        }
    }

    fn write_portsc(&self, port_id: u8, val: u32) {
        unsafe {
            let port_offset = 0x400 + (port_id as usize - 1) * 0x10;
            let portsc_ptr = (self.op_base + port_offset) as *mut u32;
            core::ptr::write_volatile(portsc_ptr, val);
        }
    }

    pub fn ring_doorbell(&mut self, slot_id: u8, target: u32) {
        unsafe {
            // 1. Find Doorbell Offset (Offset 0x14 in Cap Regs)
            let dboff_ptr = (self.base_addr + 0x14) as *const u32;
            let dboff = core::ptr::read_volatile(dboff_ptr) & !0x03; // 4-byte aligned

            // 2. Doorbell Register 0 is at Base + DBOFF
            // Each doorbell is 32-bits. Register index is the Slot ID.
            // Slot ID 0 is always the Command Ring.
            let db_addr = self.base_addr + dboff as usize + (slot_id as usize * 4);
            let db_ptr = db_addr as *mut u32;

            // 3. Write the Target using ASM
            // We bypass standard write to ensure ordering.
            ring_doorbell_asm(db_addr as u64, target);

            // DEBUG: DOORBELL ADDRESS VERIFICATION
            serial_println!("xHCI DEBUG: DBOFF Register = {:#x}", core::ptr::read_volatile(dboff_ptr));
            serial_println!("xHCI DEBUG: Calculated DB[0] Addr = {:#x}", self.base_addr + dboff as usize);
            serial_println!("xHCI DEBUG: Actual Write Addr    = {:#x}", db_ptr as usize);

            serial_println!("xHCI: DOORBELL RUNG (Slot {}, Target {}).", slot_id, target);
        }
    }

    pub fn poll_events(&mut self) -> bool {
        let mut ring = EVENT_RING.lock(); // Lock the static ring
        let mut command_completed = false;
        let mut retries = 0;

        // UNA-19-POLLING: Loop with timeout to catch events in Polling Mode
        loop {
            // Check for event
            if ring.has_event() {
                retries = 0; // Reset timeout if we find an event (chaining)
                serial_println!("xHCI: Event Detected!");

                if let Some(trb) = ring.pop() {
                    let param = trb.parameter;
                    let status = trb.status;
                    let control = trb.control;

                    // UNA-21-VERBOSE: Dump Raw TRB
                    serial_println!("xHCI RAW: Param={:#x} Status={:#x} Control={:#x}", param, status, control);

                    // 1. EXTRACT THE TYPE
                    // Control Field: Bits 15:10 = TRB Type
                    let trb_type = (control >> 10) & 0x3F;

                // 2. DISPATCH
                match trb_type {
                    33 => { // COMMAND COMPLETION EVENT
                        let command_ptr = param;
                        let completion_code = (status >> 24) & 0xFF;
                        let slot_id = (control >> 24) & 0xFF;

                        serial_println!("xHCI: [Event] Command Completion. Ptr={:#x}, Slot={}, Code={}",
                            command_ptr, slot_id, completion_code);

                        // Completion Code 1 = Success
                        if completion_code == 1 {
                            serial_println!("xHCI: >>> COMMAND SUCCESS <<<");
                            if slot_id > 0 {
                                serial_println!("xHCI: SLOT ID ALLOCATED: {}", slot_id);

                                // UNA-18-ADDRESS: If we have a pending port ID, this is likely the result of Enable Slot.
                                // Proceed to Address Device.
                                if self.pending_port_id > 0 {
                                    serial_println!("xHCI: Proceeding to Address Device (Slot {}, Port {})...", slot_id, self.pending_port_id);
                                    let port_to_map = self.pending_port_id;
                                    self.pending_port_id = 0; // Clear it to avoid duplicate calls
                                    self.address_device(slot_id as u8, port_to_map);
                                }
                                // UNA-21-ACCELERATE: Check if we were configuring endpoints
                                else if self.configuring_slot == slot_id as u8 {
                                    serial_println!("xHCI: Endpoints Configured (Slot {}). Proceeding to SCSI Read...", slot_id);
                                    self.configuring_slot = 0;
                                    self.send_scsi_read(slot_id as u8);
                                }
                                else {
                                    // UNA-19-IDENTITY: If pending_port_id is 0, we assume Address Device just finished.
                                    serial_println!("xHCI: >>> SLOT {} ENABLED & ADDRESSED <<<", slot_id);
                                    self.request_device_descriptor(slot_id as u8);
                                }
                            }
                        } else {
                            serial_println!("xHCI: >>> COMMAND FAILED (Code {}) <<<", completion_code);
                            // UNA-19-HALT: Stop on Code 5
                            if completion_code == 5 {
                                serial_println!("xHCI: CRITICAL FAILURE: TRB ERROR (CODE 5). SYSTEM HALTED.");
                                loop { core::hint::spin_loop(); }
                            }
                        }
                        command_completed = true;
                    },
                    34 => { // PORT STATUS CHANGE EVENT
                        let port_id = ((param >> 24) & 0xFF) as u8;
                        serial_println!("xHCI: [Event] Port Status Change. Port={}", port_id);

                        // UNA-18-SLOT: Handle Reset Complete & Enable Slot
                        // 1. Read the register to see WHAT changed
                        let port_sc = self.read_portsc(port_id);

                        // 2. Check for PRC (Port Reset Change - Bit 21)
                        if (port_sc & (1 << 21)) != 0 {
                            serial_println!("xHCI: [Port {}] Reset Complete. Clearing Change Bit...", port_id);

                            // Write 1 to Bit 21 to clear the change notification
                            // Preserve other bits (read-modify-write)
                            self.write_portsc(port_id, port_sc | (1 << 21));

                            // 3. Check if Port is now ENABLED (Bit 1)
                            // Re-read or just check the value we had (though clearing bit might be needed first?
                            // Standard practice: Read again to be sure, or check the value we just read.
                            // If PRC is set, PED (Bit 1) should be valid now.
                            if (port_sc & (1 << 1)) != 0 {
                                serial_println!("xHCI: [Port {}] is ENABLED. Requesting Slot...", port_id);
                                self.enable_slot(port_id);
                            }
                        }

                        // Also run the old handler for other changes (Connects)
                        self.handle_port_change(port_id);
                    },
                    32 => { // TRANSFER EVENT
                        let transfer_len = status & 0xFFFFFF;
                        let completion_code = (status >> 24) & 0xFF;
                        let slot_id = (control >> 24) & 0xFF; // Slot ID is in Control Bits 31:24
                        let endpoint_id = (control >> 16) & 0x1F; // Endpoint ID in Control Bits 16:20

                        // UNA-21-VERBOSE: SCREAMING DETAILS
                        serial_println!("xHCI DEBUG: [Transfer Event] Slot={}, EP={}, Code={}, Len={}",
                            slot_id, endpoint_id, completion_code, transfer_len);

                        // UNA-19-REVEAL: If success, check buffer
                        if completion_code == 1 {
                            // UNA-21-DEBUG: Force Transition based on Endpoint ID
                            // EP1 = Control (Device Descriptor)
                            // EP3 = Bulk IN (SCSI Read)

                            if endpoint_id == 1 && slot_id == 1 { // EP0 (Control) -> Device Descriptor
                                serial_println!("xHCI: >>> INTERCEPTED DESCRIPTOR EVENT (Slot 1 EP 1) <<<");
                                unsafe {
                                    let vid = (DESCRIPTOR_BUFFER.data[8] as u16) | ((DESCRIPTOR_BUFFER.data[9] as u16) << 8);
                                    let pid = (DESCRIPTOR_BUFFER.data[10] as u16) | ((DESCRIPTOR_BUFFER.data[11] as u16) << 8);

                                    serial_println!(">>> SYSTEM ALERT: NEW HARDWARE DETECTED <<<");
                                    serial_println!(">>> [CONTACT ESTABLISHED] SLOT {}", slot_id);
                                    serial_println!(">>> VENDOR ID : [{:04x}]", vid);
                                    serial_println!(">>> PRODUCT ID: [{:04x}]", pid);

                                    // UNA-22-HAUL: Inspect Class Code
                                    let class_code = DESCRIPTOR_BUFFER.data[4];
                                    let subclass = DESCRIPTOR_BUFFER.data[5];
                                    let protocol = DESCRIPTOR_BUFFER.data[6];

                                    serial_println!("xHCI: Device Found. Class={:#x} Sub={:#x} Proto={:#x}",
                                        class_code, subclass, protocol);

                                    if class_code == 0x08 { // 0x08 = Mass Storage
                                        serial_println!("xHCI: >>> CARGO DETECTED (MASS STORAGE) <<<");
                                        serial_println!("xHCI: Initiating Bulk Transport Setup...");

                                        // UNA-21-CONFIG: Initiate Endpoint Configuration
                                        self.configuring_slot = slot_id as u8;
                                        self.configure_endpoints(slot_id as u8);
                                    } else if class_code == 0x00 {
                                        // Class 0 means "Look at Interface Descriptor" (Common for Flash Drives too)
                                        serial_println!("xHCI: Composite Device. checking Interface...");
                                    }
                                }
                            } else if endpoint_id == 3 { // EP1 IN (Bulk IN) -> SCSI Read
                                unsafe {
                                    serial_println!("xHCI: >>> BULK IN TRANSFER COMPLETE (SCSI Read) <<<");
                                    // Check Signature
                                    let sig = core::str::from_utf8(&DATA_BUFFER.data[0..21]).unwrap_or("INVALID");
                                    serial_println!("xHCI: SECTOR 0 SIGNATURE: {}", sig);

                                    if sig == "UNA-OS-DISK-001-ALPHA" {
                                        serial_println!("xHCI: >>> MISSION SUCCESS. TARGET ACQUIRED. <<<");
                                    } else {
                                        serial_println!("xHCI: >>> SIGNATURE MISMATCH <<<");
                                    }

                                    // UNA-19-STABILIZE: The Parking Brake
                                    serial_println!(">>> SYSTEM STABILIZED. HALTING. <<<");
                                    loop { x86_64::instructions::hlt(); }
                                }
                            }
                        }
                    },
                    _ => {
                        serial_println!("xHCI: [Event] Unknown Type {}. Param={:#x}, Status={:#x}",
                            trb_type, param, status);
                    }
                }

                // --- THE ACKNOWLEDGEMENT ---
                // We must update the ERDP (Event Ring Dequeue Pointer) to the NEW index.
                // ERDP Register is at RuntimeBase + 0x20 (IR0) + 0x18.
                // Note: We calculated IR0 Base in init_interrupter, but we need it here.
                // For now, re-calculate or store it. Let's re-calc for safety/statelessness.
                unsafe {
                    let rtsoff_ptr = (self.base_addr + 0x18) as *const u32;
                    let rtsoff = core::ptr::read_volatile(rtsoff_ptr) & !0x1F;
                    let ir0_base = self.base_addr + rtsoff as usize + 0x20;

                    // Calculate physical address of the current Dequeue Pointer
                    // We need the address of the *next* slot (which ring.dequeue_index now points to)
                    // Assumption: ring.get_ptr() returns the base address of the array.
                    // Each TRB is 16 bytes.
                    // We explicitly cast to u64 to avoid overflow.

                    if EVENT_RING_PHYS_BASE == 0 {
                        serial_println!("xHCI: PANIC - EVENT_RING_PHYS_BASE is 0!");
                        loop { core::hint::spin_loop(); }
                    }

                    // UNA-19-MATH: Ensure we add the physical base to the offset!
                    let segment_base = EVENT_RING_PHYS_BASE;
                    let offset = ring.dequeue_index as u64 * 16;
                    let new_dequeue_ptr = segment_base + offset;

                    // Write ERDP.
                    // Bit 3 is "Event Handler Busy" (EHB). Writing 1 clears it.
                    // We OR in 8 (1000 binary) to clear the busy flag.
                    let erdp_reg = (ir0_base + 0x18) as *mut u64;
                    core::ptr::write_volatile(erdp_reg, new_dequeue_ptr | 8);

                    serial_println!("xHCI: ERDP Advanced to {:#x}", new_dequeue_ptr);
                }
                } // Close if let Some(trb)
            } else {
                // No event found. Spin and retry.
                retries += 1;
                if retries > 1_000_000 {
                    break;
                }
                core::hint::spin_loop();
            }
        }
        command_completed
    }

    pub fn read_version(&self) -> u16 {
        unsafe {
            let cap_ptr = self.base_addr as *const u32;
            let cap_word = core::ptr::read_volatile(cap_ptr);
            (cap_word >> 16) as u16
        }
    }

    pub fn reset(&mut self) {
        let usbcmd_ptr = self.op_base as *mut u32;
        let usbsts_ptr = (self.op_base + 0x04) as *const u32; // Status reg is at +0x04

        unsafe {
            serial_println!("xHCI: Asserting HCRST...");
            let cmd = core::ptr::read_volatile(usbcmd_ptr);
            // Write 1 to Bit 1 (HCRST)
            core::ptr::write_volatile(usbcmd_ptr, cmd | 2);

            // POLL: Wait for HCRST (Bit 1) to clear (hardware clears it when done)
            loop {
                let current_cmd = core::ptr::read_volatile(usbcmd_ptr);
                if (current_cmd & 2) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            serial_println!("xHCI: Reset Complete.");

            // POLL: Wait for CNR (Controller Not Ready, Bit 11 in USBSTS) to clear
            // The controller needs time to re-initialize after reset.
            loop {
                let status = core::ptr::read_volatile(usbsts_ptr);
                if (status & (1 << 11)) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            serial_println!("xHCI: Controller Ready.");
        }
    }

    pub fn init_pointers(&mut self, ring_phys_addr: PhysAddr, dcbaap_phys: PhysAddr) {
        unsafe {
            // 1. Set DCBAAP
            // (In a real driver, we'd read HCSPARAMS1 to check max slots, but 256 covers all)
            let dcbaap_reg = (self.op_base + 0x30) as *mut u64;
            core::ptr::write_volatile(dcbaap_reg, dcbaap_phys.as_u64());
            serial_println!("xHCI: DCBAAP set to {:#x}", dcbaap_phys);

            // 2. Set Command Ring Control Register (CRCR)
            // OpBase + 0x18.
            // MUST set Bit 0 (RCS - Ring Cycle State) to 1 to match our initial Ring state.
            let crcr_reg = (self.op_base + 0x18) as *mut u64;
            let crcr_value = ring_phys_addr.as_u64() | 1;
            core::ptr::write_volatile(crcr_reg, crcr_value);
            serial_println!("xHCI: CRCR set to {:#x}", crcr_value);
        }
    }

    // Call this AFTER init_pointers but BEFORE run
    pub fn init_interrupter(&mut self, event_ring_phys: PhysAddr, erst_table_phys: PhysAddr) {
        unsafe {
            // SAVE THIS for later use in the interrupt/event loop (ERDP updates)
            EVENT_RING_PHYS_BASE = event_ring_phys.as_u64();

            // 1. Calculate Runtime Base
            // Read RTSOFF (Offset 0x18 in Capability Regs)
            let rtsoff_ptr = (self.base_addr + 0x18) as *const u32;
            let rtsoff = core::ptr::read_volatile(rtsoff_ptr) & !0x1F; // Clear lower 5 bits? Spec says 32-byte aligned.
            let runtime_base = self.base_addr + rtsoff as usize;

            // Interrupter 0 Base = RuntimeBase + 0x20
            let ir0_base = runtime_base + 0x20;
            serial_println!("xHCI: RuntimeBase={:#x}, IR0 Base={:#x}", runtime_base, ir0_base);

            // 2. Setup the Segment Table (ERST)
            // Ensure Event Ring memory is clean (Directive UNA-12-CONFIG)
            EVENT_RING.lock().clear();

            // We use 1 segment, pointing to our static EVENT_RING
            ERST_TABLE.entries[0] = ErstEntry {
                ring_address: event_ring_phys.as_u64(),
                size: 16, // Must match EVENT_RING_SIZE
                _rsvd: 0,
                _rsvd2: 0,
            };

            // 3. Write ERSTSZ (Segment Table Size) - Offset 0x08
            // Value = 1 (We have 1 segment)
            let erstsz_ptr = (ir0_base + 0x08) as *mut u32;
            core::ptr::write_volatile(erstsz_ptr, 1);

            // 4. Write ERSTBA (Segment Table Base Address) - Offset 0x10
            let erstba_ptr = (ir0_base + 0x10) as *mut u64;
            core::ptr::write_volatile(erstba_ptr, erst_table_phys.as_u64());

            // 5. Write ERDP (Event Ring Dequeue Pointer) - Offset 0x18
            // Initialize to the start of the ring.
            // PRESERVE BIT 3 (EHB - Event Handler Busy)? No, clear it initially.
            let erdp_ptr = (ir0_base + 0x18) as *mut u64;
            core::ptr::write_volatile(erdp_ptr, event_ring_phys.as_u64()); // Pointer to the RING, not the table

            // 6. GAG the Interrupter (IMAN - Interrupter Management) - Offset 0x00
            // Bit 0 = IP (Interrupt Pending), Bit 1 = IE (Interrupt Enable)
            // UNA-19-SILENCE: Clear Bit 1 (IE) and Bit 0 (IP)
            let iman_ptr = (ir0_base + 0x00) as *mut u32;
            let iman = core::ptr::read_volatile(iman_ptr);
            core::ptr::write_volatile(iman_ptr, iman & !0x3);

            serial_println!("xHCI: Interrupter 0 GAGGED (IMAN.IE Cleared).");
        }
    }

    fn handle_port_change(&self, port_id: u8) {
        unsafe {
            // 1. Get the Port Register Set
            // PORTSC is at op_base + 0x400 + (port_id - 1) * 0x10
            // Note: port_id is 1-based from the Event TRB.
            let port_offset = 0x400 + (port_id as usize - 1) * 0x10;
            let portsc_ptr = (self.op_base + port_offset) as *mut u32;

            let mut status = core::ptr::read_volatile(portsc_ptr);
            serial_println!("xHCI: Port {} Status: {:#x}", port_id, status);

            // PHASE 1: ACKNOWLEDGE (Clear CSC if set)
            // Bit 17: CSC (Connect Status Change). RW1C (Read/Write 1 to Clear).
            if (status & (1 << 17)) != 0 {
                serial_println!("xHCI: Clearing CSC on Port {}", port_id);
                // Clear CSC (Bit 17) by writing 1 to it.
                // Preserve other R/W bits, but ensure PR (Bit 4) is 0 to avoid unintended reset.
                let clear_csc = (status & !(1 << 4)) | (1 << 17);
                core::ptr::write_volatile(portsc_ptr, clear_csc);

                // Re-read status after clear
                status = core::ptr::read_volatile(portsc_ptr);
            }

            // 2. Check for Connection (Bit 0: CCS - Current Connect Status)
            if (status & 1) != 0 {
                // Only reset if enabled bit (Bit 1: PED) is 0 (not yet enabled)
                // AND we are not already resetting (Bit 4: PR)
                if (status & 2) == 0 && (status & (1 << 4)) == 0 {
                    serial_println!("xHCI: Device Connected on Port {}. Resetting...", port_id);
                    // 3. Initiate Port Reset (Bit 4: PR)
                    core::ptr::write_volatile(portsc_ptr, status | (1 << 4));
                }
            }
        }
    }

    pub fn diagnose_command_ring(&self, original_ptr: u64) {
        unsafe {
            // 1. READ CRCR (Command Ring Control Register)
            // Offset 0x18 from OpBase
            let crcr_reg = (self.op_base + 0x18) as *const u64;
            let crcr_raw = core::ptr::read_volatile(crcr_reg);

            // Mask bits 0-5 to get the pointer (address is 64-byte aligned, so low 6 bits are flags)
            let crcr_ptr = crcr_raw & !0x3F;

            serial_println!("xHCI DEBUG: CRCR State Analysis");
            serial_println!("   Started At: {:#x}", original_ptr);
            serial_println!("   Current:    {:#x}", crcr_ptr);
            serial_println!("   Raw CRCR:   {:#x}", crcr_raw);

            if crcr_ptr == original_ptr {
                serial_println!("   CONCLUSION: STALLED. Hardware never fetched the command.");
                serial_println!("   POSSIBLE CAUSES: Doorbell missed, Cycle Bit mismatch, or Bad Address.");
            } else {
                serial_println!("   CONCLUSION: EXECUTED. Hardware moved past the command.");
                serial_println!("   ISSUE: Event Ring lost the receipt.");
            }
        }
    }

    pub fn check_vitals(&mut self) {
        unsafe {
            // 1. CHECK USBSTS (USB Status Register)
            // Offset 0x04 from Operational Base
            let usbsts_ptr = (self.op_base + 0x04) as *const u32;
            let usbsts = core::ptr::read_volatile(usbsts_ptr);

            serial_println!("xHCI DEBUG: USBSTS = {:#x}", usbsts);
            if (usbsts & (1 << 12)) != 0 { serial_println!("   CRITICAL: HCE (Host Controller Error) SET!"); }
            if (usbsts & (1 << 2)) != 0 { serial_println!("   CRITICAL: HSE (Host System Error) SET!"); }

            // 2. CHECK DOORBELL ACCESSIBILITY (The "Cliff" Test)
            // We try to READ the Doorbell register.
            // Even though it's Write-Only, reading it should NOT crash if mapped.
            // If this causes a Page Fault, we know the mapping is too small.
            let db_ptr = (self.base_addr + 0x2000) as *mut u32; // DBOFF is assumed 0x2000 for this test
            serial_println!("xHCI DEBUG: Testing Doorbell Memory Access at {:#p}...", db_ptr);

            let _probe = core::ptr::read_volatile(db_ptr);
            serial_println!("xHCI DEBUG: Doorbell Memory is Accessible. (Value: {:#x})", _probe);

            // 3. CHECK COMMAND WRAPPER
            // Ensure we are writing 32-bits, not 64-bits.
            // Doorbell registers are strictly 32-bit.
            core::ptr::write_volatile(db_ptr, 0);
            serial_println!("xHCI DEBUG: Doorbell 0 (Target 0) manually written.");
        }
    }

    pub fn run(&mut self) {
        unsafe {
            // 1. READ MAX SLOTS (HCSPARAMS1 is Offset 0x04 from CAPABILITY BASE)
            let hcsparams1_ptr = (self.base_addr + 0x04) as *const u32;
            let hcsparams1 = core::ptr::read_volatile(hcsparams1_ptr);
            let max_slots = hcsparams1 & 0xFF; // Bits 0-7

            serial_println!("xHCI: Hardware supports {} Device Slots.", max_slots);

            // 2. WRITE CONFIG REGISTER (Offset 0x38 from OPERATIONAL BASE)
            // Bits 0-7: MaxSlotsEn
            let config_ptr = (self.op_base + 0x38) as *mut u32;
            core::ptr::write_volatile(config_ptr, max_slots);

            serial_println!("xHCI: CONFIG register set to {}.", max_slots);

            // 3. RUN
            let usbcmd_ptr = self.op_base as *mut u32;
            let usbsts_ptr = (self.op_base + 0x04) as *const u32;

            serial_println!("xHCI: Starting Engine (INTERRUPTS DISABLED)...");
            let cmd = core::ptr::read_volatile(usbcmd_ptr);
            // UNA-19-POLLING: Clear Bit 2 (INTE) to disable interrupts (Polling Mode)
            // Set Bit 0 (Run)
            core::ptr::write_volatile(usbcmd_ptr, (cmd & !(1 << 2)) | 1);

            // POLL: Wait for HCHalted (Bit 0 in Status) to CLEAR.
            // This confirms the hardware is executing.
            loop {
                let status = core::ptr::read_volatile(usbsts_ptr);
                if (status & 1) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            serial_println!("xHCI: ENGINE RUNNING (HCHalted cleared).");
        }
    }

    pub fn enable_slot(&mut self, port_id: u8) {
        serial_println!("xHCI: Sending ENABLE_SLOT command for Port {}...", port_id);
        self.pending_port_id = port_id;

        // TRB Type 9 = Enable Slot
        // Control: (Type 9 << 10)
        // Cycle Bit is handled by the Ring.
        let trb = Trb {
            parameter: 0,
            status: 0,
            control: (9 << 10),
        };

        if let Err(e) = self.send_command(trb) {
            serial_println!("xHCI: Failed to send Enable Slot command: {}", e);
        }
    }

    pub fn address_device(&mut self, slot_id: u8, port_id: u8) {
        unsafe {
            serial_println!("xHCI: Addressing Device (Slot {}, Port {})...", slot_id, port_id);

            // 0. ADDRESS RESOLUTION
            // We use the Physical Addresses stored in the controller for linking and TRBs.
            // We use Virtual (Raw) Pointers for writing to the structures.
            let output_ctx_phys = self.output_context_phys;
            let input_ctx_phys = self.input_context_phys;
            let ep0_ring_phys = self.ep0_ring_phys;

            // Virtual Pointers for Writing
            let _output_ctx_virt = &raw mut OUTPUT_CONTEXT;
            let input_ctx_virt = &raw mut INPUT_CONTEXT;
            let ep0_ring_virt = &raw mut EP0_RING;

            // 1. LINK DCBAAP
            // Point the Slot ID entry to the Output Context
            DCBAAP_TABLE.ptrs[slot_id as usize] = output_ctx_phys;
            serial_println!("xHCI: DCBAAP[{}] linked to {:#x}", slot_id, output_ctx_phys);

            // 2. PREPARE EP0 RING
            // Nothing to do but zero it
            core::ptr::write_bytes(ep0_ring_virt as *mut u8, 0, 16 * 16);

            // 3. FILL INPUT CONTEXT (MANUAL OFFSET CALCULATION)
            // We cannot trust the struct layout due to internal padding caused by alignments.
            // We use raw pointer arithmetic to write to exact offsets as per xHCI Spec.
            // Input Context Structure:
            // 0x00 - 0x1F: Input Control Context (32 bytes)
            // 0x20 - 0x3F: Slot Context (32 bytes)
            // 0x40 - 0x5F: Endpoint 0 Context (32 bytes)

            let base_ptr = input_ctx_virt as *mut u32;

            // 1. THE GUARD (Safety Check)
            if self.input_context_phys == 0 {
                panic!("xHCI: FATAL - Input Context Physical Address is 0! Init failed?");
            }

            // 2. THE WRITE
            // UNA-22-POINTER: Reduced size to 2048 to prevent BSS overflow
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 2048);

            // 3a. INPUT CONTROL CONTEXT (Offset 0x00)
            // DW1 (Offset 0x04): Add Context Flags
            // Enable Slot (Bit 0) and EP0 (Bit 1) -> Val = 3
            base_ptr.add(1).write_volatile(3);

            // 3b. SLOT CONTEXT (Offset 0x20 -> Index 8 in u32)
            let slot_ctx_ptr = base_ptr.add(8);

            // Slot Context DW0 (Offset 0x00 relative to SlotCtx): Route, Speed, Context Entries
            // Context Entries (Bits 27-31) = 1
            slot_ctx_ptr.add(0).write_volatile(1 << 27);

            // Slot Context DW1 (Offset 0x04): Root Hub Port Number
            slot_ctx_ptr.add(1).write_volatile((port_id as u32) << 16);

            // 3c. ENDPOINT 0 CONTEXT (Offset 0x40 -> Index 16 in u32)
            let ep0_ctx_ptr = base_ptr.add(16);

            // EP0 Context DW1 (Offset 0x04): MaxPacketSize, EP Type, CErr
            // EP Type = 4 (Control), CErr = 3, MPS = 64
            ep0_ctx_ptr.add(1).write_volatile((4 << 3) | (3 << 1) | (64 << 16));

            // EP0 Context DW2 (Offset 0x08): TR Dequeue Pointer Lo | DCS
            // Bit 0 must match Cycle Bit (1)
            ep0_ctx_ptr.add(2).write_volatile((ep0_ring_phys as u32) | 1);

            // EP0 Context DW3 (Offset 0x0C): TR Dequeue Pointer Hi
            ep0_ctx_ptr.add(3).write_volatile((ep0_ring_phys >> 32) as u32);

            // EP0 Context DW4 (Offset 0x10): Average TRB Length
            ep0_ctx_ptr.add(4).write_volatile(8);

            serial_println!("xHCI: Input Context Initialized (Manual Offsets). Phys={:#x}", input_ctx_phys);
            // Dump for verification
            serial_println!("   ICC[0]: {:#x}, ICC[1]: {:#x}", base_ptr.read_volatile(), base_ptr.add(1).read_volatile());
            serial_println!("   Slot[0]: {:#x}", slot_ctx_ptr.read_volatile());

            // 4. SEND ADDRESS DEVICE COMMAND
            // TRB Type 11
            // Param: Input Context Ptr (Physical)
            // Control: (Type 11 << 10) | (Slot ID << 24)
            // BSR (Block Set Address Request) = 0 (Bit 9) -> Sends SET_ADDRESS to device
            let trb = Trb {
                parameter: input_ctx_phys,
                status: 0,
                control: (11 << 10) | ((slot_id as u32) << 24),
            };

            if let Err(e) = self.send_command(trb) {
                serial_println!("xHCI: Failed to send Address Device command: {}", e);
            }
        }
    }

    pub fn configure_endpoints(&mut self, slot_id: u8) {
        unsafe {
            serial_println!("xHCI: UNA-21 Configuring Endpoints for Slot {}...", slot_id);

            // 1. GET POINTERS
            let input_ctx_virt = &raw mut INPUT_CONTEXT;
            let base_ptr = input_ctx_virt as *mut u32;

            // 2. CLEAR INPUT CONTEXT (Safety first)
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 2048);

            // 3. INPUT CONTROL CONTEXT (Offset 0x00)
            // Add Context Flags (DW1, Offset 0x04)
            // We are modifying the device to ADD contexts.
            // Slot Context (Bit 0) + EP1 IN (Bit 3) + EP2 OUT (Bit 4)
            // Val = 1 | 8 | 16 = 25 (0x19)
            base_ptr.add(1).write_volatile(0x19);

            // 4. SLOT CONTEXT (Offset 0x20 -> Index 8)
            let slot_ctx_ptr = base_ptr.add(8);
            // DW0: Context Entries = 5 (Covering up to Index 5/EP2 IN, though we use 4/EP2 OUT)
            // We set it to 5 just to be safe.
            slot_ctx_ptr.add(0).write_volatile(5 << 27);

            // 5. ENDPOINT 1 IN (Index 3) -> Offset 0x60
            // 0x60 / 4 = 24 (Index)
            let ep1_in_ptr = base_ptr.add(24);

            // DW1: MPS=512, EP Type=6 (Bulk IN), CErr=3
            // (6 << 3) | (3 << 1) | (512 << 16)
            ep1_in_ptr.add(1).write_volatile((6 << 3) | (3 << 1) | (512 << 16));

            // DW2: Dequeue Pointer Lo | DCS (Cycle Bit = 1)
            ep1_in_ptr.add(2).write_volatile((self.bulk_in_ring_phys as u32) | 1);
            // DW3: Dequeue Pointer Hi
            ep1_in_ptr.add(3).write_volatile((self.bulk_in_ring_phys >> 32) as u32);
            // DW4: Avg TRB Len = 512
            ep1_in_ptr.add(4).write_volatile(512);

            // 6. ENDPOINT 2 OUT (Index 4) -> Offset 0x80
            // 0x80 / 4 = 32 (Index)
            let ep2_out_ptr = base_ptr.add(32);

            // DW1: MPS=512, EP Type=2 (Bulk OUT), CErr=3
            // (2 << 3) | (3 << 1) | (512 << 16)
            ep2_out_ptr.add(1).write_volatile((2 << 3) | (3 << 1) | (512 << 16));

            // DW2: Dequeue Pointer Lo | DCS (Cycle Bit = 1)
            ep2_out_ptr.add(2).write_volatile((self.bulk_out_ring_phys as u32) | 1);
            // DW3: Dequeue Pointer Hi
            ep2_out_ptr.add(3).write_volatile((self.bulk_out_ring_phys >> 32) as u32);
            // DW4: Avg TRB Len = 512
            ep2_out_ptr.add(4).write_volatile(512);

            serial_println!("xHCI: Input Context Configured for Bulk Transport.");

            // 7. SEND CONFIGURE ENDPOINT COMMAND
            // Type 12
            // Param: Input Context Phys
            let trb = Trb {
                parameter: self.input_context_phys,
                status: 0,
                control: (12 << 10) | ((slot_id as u32) << 24),
            };

            if let Err(e) = self.send_command(trb) {
                serial_println!("xHCI: Failed to send Configure Endpoint command: {}", e);
            }
        }
    }

    pub fn send_scsi_read(&mut self, slot_id: u8) {
        unsafe {
            serial_println!("xHCI: UNA-21 Initiating SCSI Read (Sector 0)...");

            // 1. CONSTRUCT CBW (Command Block Wrapper)
            // We recycle DESCRIPTOR_BUFFER to hold the 31-byte CBW.
            let cbw_ptr = &raw mut DESCRIPTOR_BUFFER.data as *mut u8;
            core::ptr::write_bytes(cbw_ptr, 0, 64); // Clear it

            // Write CBW Signature: 'USBC' (Little Endian) -> 0x43425355
            *cbw_ptr.add(0) = 0x55;
            *cbw_ptr.add(1) = 0x53;
            *cbw_ptr.add(2) = 0x42;
            *cbw_ptr.add(3) = 0x43;

            // Tag: 0xDEADBEEF
            *cbw_ptr.add(4) = 0xEF;
            *cbw_ptr.add(5) = 0xBE;
            *cbw_ptr.add(6) = 0xAD;
            *cbw_ptr.add(7) = 0xDE;

            // Data Transfer Length: 512 (0x200)
            *cbw_ptr.add(8) = 0x00;
            *cbw_ptr.add(9) = 0x02;
            *cbw_ptr.add(10) = 0x00;
            *cbw_ptr.add(11) = 0x00;

            // Flags: 0x80 (Data IN)
            *cbw_ptr.add(12) = 0x80;

            // LUN: 0
            *cbw_ptr.add(13) = 0x00;

            // CDB Length: 10
            *cbw_ptr.add(14) = 10;

            // CDB (SCSI READ 10): 0x28
            *cbw_ptr.add(15) = 0x28; // OpCode
            // LBA 0 (Bytes 17-20 are 0)
            // Transfer Length (Blocks) = 1 (Byte 22 = 0, Byte 23 = 1)
            *cbw_ptr.add(23) = 1;

            serial_println!("xHCI: CBW Constructed.");

            // 2. PUSH TO BULK OUT RING (EP2 OUT - Index 4)
            // Normal TRB (Type 1)
            // Length: 31
            let out_trb = Trb {
                parameter: self.descriptor_phys, // Pointing to CBW
                status: 31, // Transfer Length
                control: (1 << 10) | (1 << 5), // Type 1 (Normal) | IOC
            };
            BULK_OUT_RING.lock().push(out_trb).unwrap();

            // RING DOORBELL: Target 4
            self.ring_doorbell(slot_id, 4);


            // 3. PUSH TO BULK IN RING (EP1 IN - Index 3)
            // Normal TRB (Type 1)
            // Length: 512
            let in_trb = Trb {
                parameter: self.data_buffer_phys, // Pointing to Data Buffer
                status: 512,
                control: (1 << 10) | (1 << 5) | (1 << 2), // Type 1 | IOC | ISP (Short Packet)
            };
            BULK_IN_RING.lock().push(in_trb).unwrap();

            // RING DOORBELL: Target 3
            self.ring_doorbell(slot_id, 3);

            serial_println!("xHCI: SCSI Command Dispatched (OUT and IN Rings rung).");
        }
    }

    pub unsafe fn scan_ports(&mut self) {
        // 1. GET MAX PORTS
        // HCSPARAMS1 is at Capability Base + 0x04
        let hcsparams1_ptr = (self.base_addr + 0x04) as *const u32;
        let hcsparams1 = core::ptr::read_volatile(hcsparams1_ptr);
        let max_ports = (hcsparams1 >> 24) & 0xFF; // Top 8 bits

        serial_println!("xHCI: Scanning {} Ports...", max_ports);

        // 2. ITERATE PORTS
        for i in 0..max_ports {
            let port_id = (i + 1) as u8;
            let port_csc = self.read_portsc(port_id);

            // Check CCS (Current Connect Status) - Bit 0
            if (port_csc & 1) != 0 {
                serial_println!("xHCI: [PORT {}] DEVICE DETECTED! (Status: {:#x})", port_id, port_csc);

                // 3. RESET PORT (The Handshake)
                // Write 1 to PR (Port Reset) - Bit 4
                // We use Read-Modify-Write to preserve other bits (like PP).
                let reset_cmd = port_csc | (1 << 4);
                self.write_portsc(port_id, reset_cmd);

                serial_println!("xHCI: [PORT {}] Reset Signal Sent. Waiting for Enable...", port_id);
            }
        }
    }

    fn push_ep0(&mut self, mut trb: Trb) {
        unsafe {
            let ring_ptr = &raw mut EP0_RING;
            let index = self.ep0_enqueue_index;

            // 1. Set Cycle Bit
            if self.ep0_cycle_state {
                trb.control |= 1;
            } else {
                trb.control &= !1;
            }

            // 2. Write
            (*ring_ptr)[index] = trb;

            // 3. Flush
            let trb_ptr = &(*ring_ptr)[index] as *const Trb;
            core::arch::x86_64::_mm_clflush(trb_ptr as *const u8);

            // 4. Advance
            self.ep0_enqueue_index += 1;
            if self.ep0_enqueue_index >= 16 {
                self.ep0_enqueue_index = 0;
                self.ep0_cycle_state = !self.ep0_cycle_state;
            }
        }
    }

    pub fn request_device_descriptor(&mut self, slot_id: u8) {
        serial_println!("xHCI: Requesting Device Descriptor for Slot {}...", slot_id);

        if self.descriptor_phys == 0 {
            serial_println!("xHCI: CRITICAL ERROR - Descriptor Buffer Phys Addr is 0!");
            return;
        }

        // 1. Setup Stage
        // 0x80 06 00 01 00 00 12 00
        // Little Endian u64: 0x0012000001000680
        let setup_trb = Trb {
            parameter: 0x0012000001000680,
            status: 8, // Transfer Length (Always 8 for Setup)
            control: (2 << 10) // Type 2 (Setup Stage)
                   | (1 << 6)  // IDT (Immediate Data)
                   | (3 << 16), // TRT (3 = IN Data Stage)
        };
        self.push_ep0(setup_trb);

        // 2. Data Stage
        let data_trb = Trb {
            parameter: self.descriptor_phys,
            status: 18, // Length 18 bytes
            control: (3 << 10) // Type 3 (Data Stage)
                   | (1 << 16), // DIR (1 = IN)
        };
        self.push_ep0(data_trb);

        // 3. Status Stage
        let status_trb = Trb {
            parameter: 0,
            status: 0,
            control: (4 << 10) // Type 4 (Status Stage)
                   | (1 << 5)  // IOC (Interrupt On Completion)
                   | (0 << 16), // DIR (0 = OUT)
        };
        self.push_ep0(status_trb);

        // 4. Ring Doorbell (Slot 1, Target 1 for EP0)
        self.ring_doorbell(slot_id, 1);
    }
}
