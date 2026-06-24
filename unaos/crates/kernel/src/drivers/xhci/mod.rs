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

pub mod trb;
pub mod ring;
pub mod event;
pub mod context;

use ring::TransferRing;
use self::trb::Trb;
use self::event::{EventRing, ErstEntry, ErstTable};
use self::context::{InputContext, DeviceContext};
use spin::Mutex;
use alloc::vec::Vec;
use alloc::boxed::Box;

/// USB HID Boot Keyboard Scancode to ASCII mapping.
/// Index is the HID usage ID (0x00..0x67). Returns (unshifted, shifted).
/// 0 means no printable character.
const HID_SCANCODE_TO_ASCII: [(u8, u8); 104] = [
    (0, 0),       // 0x00: Reserved
    (0, 0),       // 0x01: ErrorRollOver
    (0, 0),       // 0x02: POSTFail
    (0, 0),       // 0x03: ErrorUndefined
    (b'a', b'A'), // 0x04
    (b'b', b'B'), // 0x05
    (b'c', b'C'), // 0x06
    (b'd', b'D'), // 0x07
    (b'e', b'E'), // 0x08
    (b'f', b'F'), // 0x09
    (b'g', b'G'), // 0x0A
    (b'h', b'H'), // 0x0B
    (b'i', b'I'), // 0x0C
    (b'j', b'J'), // 0x0D
    (b'k', b'K'), // 0x0E
    (b'l', b'L'), // 0x0F
    (b'm', b'M'), // 0x10
    (b'n', b'N'), // 0x11
    (b'o', b'O'), // 0x12
    (b'p', b'P'), // 0x13
    (b'q', b'Q'), // 0x14
    (b'r', b'R'), // 0x15
    (b's', b'S'), // 0x16
    (b't', b'T'), // 0x17
    (b'u', b'U'), // 0x18
    (b'v', b'V'), // 0x19
    (b'w', b'W'), // 0x1A
    (b'x', b'X'), // 0x1B
    (b'y', b'Y'), // 0x1C
    (b'z', b'Z'), // 0x1D
    (b'1', b'!'), // 0x1E
    (b'2', b'@'), // 0x1F
    (b'3', b'#'), // 0x20
    (b'4', b'$'), // 0x21
    (b'5', b'%'), // 0x22
    (b'6', b'^'), // 0x23
    (b'7', b'&'), // 0x24
    (b'8', b'*'), // 0x25
    (b'9', b'('), // 0x26
    (b'0', b')'), // 0x27
    (b'\n', b'\n'), // 0x28: Return/Enter
    (0x1B, 0x1B), // 0x29: Escape
    (0x08, 0x08), // 0x2A: Backspace
    (b'\t', b'\t'), // 0x2B: Tab
    (b' ', b' '), // 0x2C: Space
    (b'-', b'_'), // 0x2D
    (b'=', b'+'), // 0x2E
    (b'[', b'{'), // 0x2F
    (b']', b'}'), // 0x30
    (b'\\', b'|'), // 0x31
    (0, 0),       // 0x32: Non-US # and ~
    (b';', b':'), // 0x33
    (b'\'', b'"'), // 0x34
    (b'`', b'~'), // 0x35
    (b',', b'<'), // 0x36
    (b'.', b'>'), // 0x37
    (b'/', b'?'), // 0x38
    (0, 0),       // 0x39: Caps Lock
    (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), // 0x3A-0x3F: F1-F6
    (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), // 0x40-0x45: F7-F12
    (0, 0), // 0x46: PrintScreen
    (0, 0), // 0x47: ScrollLock
    (0, 0), // 0x48: Pause
    (0, 0), // 0x49: Insert
    (0, 0), // 0x4A: Home
    (0, 0), // 0x4B: PageUp
    (0x7F, 0x7F), // 0x4C: Delete
    (0, 0), // 0x4D: End
    (0, 0), // 0x4E: PageDown
    (0, 0), // 0x4F: Right Arrow
    (0, 0), // 0x50: Left Arrow
    (0, 0), // 0x51: Down Arrow
    (0, 0), // 0x52: Up Arrow
    (0, 0), // 0x53: Num Lock
    (b'/', b'/'), // 0x54: Keypad /
    (b'*', b'*'), // 0x55: Keypad *
    (b'-', b'-'), // 0x56: Keypad -
    (b'+', b'+'), // 0x57: Keypad +
    (b'\n', b'\n'), // 0x58: Keypad Enter
    (b'1', b'1'), // 0x59: Keypad 1
    (b'2', b'2'), // 0x5A: Keypad 2
    (b'3', b'3'), // 0x5B: Keypad 3
    (b'4', b'4'), // 0x5C: Keypad 4
    (b'5', b'5'), // 0x5D: Keypad 5
    (b'6', b'6'), // 0x5E: Keypad 6
    (b'7', b'7'), // 0x5F: Keypad 7
    (b'8', b'8'), // 0x60: Keypad 8
    (b'9', b'9'), // 0x61: Keypad 9
    (b'0', b'0'), // 0x62: Keypad 0
    (b'.', b'.'), // 0x63: Keypad .
    (0, 0),       // 0x64: Non-US \ and |
    (0, 0),       // 0x65: Application
    (0, 0),       // 0x66: Power
    (b'=', b'='), // 0x67: Keypad =
];


pub fn init(base_address: u64) {
    serial_println!("xHCI: Virtual Handoff. Base Address: {:#x}", base_address);

    let cap_ptr = base_address as *const u32;
    let cap_word = unsafe { core::ptr::read_volatile(cap_ptr) };
    let cap_length = (cap_word & 0xFF) as u8;

    let op_base = base_address + cap_length as u64;
    serial_println!("xHCI: CapLength: {}, Operational Base: {:#x}", cap_length, op_base);

    let usbcmd_ptr = op_base as *mut u32;
    let usbsts_ptr = (op_base + 0x04) as *const u32;

    unsafe {
        // Halt Controller
        let cmd = core::ptr::read_volatile(usbcmd_ptr);
        core::ptr::write_volatile(usbcmd_ptr, cmd & !1);

        loop {
            let status = core::ptr::read_volatile(usbsts_ptr);
            if (status & 1) != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        serial_println!("xHCI: Controller Halted.");

        // Reset Controller
        let cmd = core::ptr::read_volatile(usbcmd_ptr);
        core::ptr::write_volatile(usbcmd_ptr, cmd | 2);

        loop {
            let current_cmd = core::ptr::read_volatile(usbcmd_ptr);
            if (current_cmd & 2) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Wait for Controller Not Ready (CNR) to clear
        loop {
            let status = core::ptr::read_volatile(usbsts_ptr);
            if (status & (1 << 11)) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        serial_println!("xHCI: Controller Reset Complete.");
    }

    serial_println!("[XHCI] CONTROLLER RESET.");
}

pub static XHCI_CONTROLLER: spin::Mutex<Option<XhciController>> = spin::Mutex::new(None);

pub static COMMAND_RING: Mutex<Option<TransferRing>> = Mutex::new(None);
pub static EVENT_RING: Mutex<Option<EventRing>> = Mutex::new(None);

pub static mut ERST_TABLE: ErstTable = ErstTable { entries: [ErstEntry { ring_address: 0, size: 0, _rsvd: 0, _rsvd2: 0 }] };

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
    serial_println!("xHCI: Ringing Doorbell at {:#x} with Target {}", doorbell_addr, target);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    core::ptr::write_volatile(doorbell_addr as *mut u32, target);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

pub struct DeviceSlot {
    pub active: bool,
    pub port_id: u8,
    pub input_context: *mut InputContext,
    pub output_context: *mut DeviceContext,
    pub ep0_ring: Option<TransferRing>,
    
    pub bulk_in_ring: Option<TransferRing>,
    pub bulk_out_ring: Option<TransferRing>,
    pub data_buffer: Option<*mut u8>,

    pub is_mouse: bool,
    pub mouse_ep: u8,
    pub mouse_mps: u16,
    pub mouse_state: u8,
    pub mouse_ring: Option<TransferRing>,

    pub is_keyboard: bool,
    pub keyboard_ep: u8,
    pub keyboard_mps: u16,
    pub keyboard_state: u8,
    pub keyboard_ring: Option<TransferRing>,
    
    pub descriptor_buffer: *mut u8,
}

unsafe impl Send for DeviceSlot {}
unsafe impl Sync for DeviceSlot {}

impl DeviceSlot {
    pub fn new() -> Self {
        let desc_layout = core::alloc::Layout::from_size_align(256, 64).unwrap();
        let desc_buffer = unsafe { alloc::alloc::alloc_zeroed(desc_layout) };
        Self {
            active: false,
            port_id: 0,
            input_context: core::ptr::null_mut(),
            output_context: core::ptr::null_mut(),
            ep0_ring: None,
            bulk_in_ring: None,
            bulk_out_ring: None,
            data_buffer: None,
            is_mouse: false,
            mouse_ep: 0,
            mouse_mps: 0,
            mouse_state: 0,
            mouse_ring: None,
            is_keyboard: false,
            keyboard_ep: 0,
            keyboard_mps: 0,
            keyboard_state: 0,
            keyboard_ring: None,
            descriptor_buffer: desc_buffer,
        }
    }
}

pub struct XhciController {
    base_addr: usize,
    op_base: usize,
    pub irq_vector: u8,
    pub max_slots: u8,
    pub max_ports: u8,
    pub dcbaap: *mut u64,
    pub slots: Vec<DeviceSlot>,
    pub pending_ports: Vec<u8>,
    
    pub configuring_slot: u8,
    pub event_ring_phys_base: u64,
}

unsafe impl Send for XhciController {}
unsafe impl Sync for XhciController {}

impl XhciController {
    pub unsafe fn new(base_addr: usize) -> Self {
        let cap_ptr = base_addr as *const u32;
        let cap_word = core::ptr::read_volatile(cap_ptr);

        let cap_length = (cap_word & 0xFF) as u8;
        let version = (cap_word >> 16) as u16;

        let op_base = base_addr + cap_length as usize;

        // Log it to verify we aren't seeing ghosts anymore
        serial_println!("xHCI: CapBase={:#x}, OpBase={:#x}, Version={:#x}", base_addr, op_base, version);

        // Read Max Slots and Max Ports from HCSPARAMS1
        let hcsparams1_ptr = (base_addr + 0x04) as *const u32;
        let hcsparams1 = core::ptr::read_volatile(hcsparams1_ptr);
        let max_slots = (hcsparams1 & 0xFF) as u8;
        let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;

        serial_println!("xHCI: MaxSlots={}, MaxPorts={}", max_slots, max_ports);

        let mut slots = Vec::new();
        for _ in 0..=max_slots {
            slots.push(DeviceSlot::new());
        }

        XhciController {
            base_addr,
            op_base,
            irq_vector: 0,
            max_slots,
            max_ports,
            dcbaap: core::ptr::null_mut(),
            slots,
            pending_ports: Vec::new(),
            configuring_slot: 0,
            event_ring_phys_base: 0,
        }
    }



    pub fn send_noop_command(&mut self) -> Result<usize, &'static str> {
        COMMAND_RING.lock().as_mut().unwrap().push_noop()
    }

    pub fn send_command(&mut self, trb: Trb) -> Result<usize, &'static str> {
        let res = COMMAND_RING.lock().as_mut().unwrap().push(trb);
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
        let mut ring_guard = EVENT_RING.lock();
        let ring = ring_guard.as_mut().unwrap();
        let mut command_completed = false;

        // Check for event
        while ring.has_event() {
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
                                if let Some(port_to_map) = self.pending_ports.pop() {
                                    serial_println!("xHCI: Proceeding to Address Device (Slot {}, Port {})...", slot_id, port_to_map);
                                    self.address_device(slot_id as u8, port_to_map);
                                }
                                // UNA-21-ACCELERATE: Check if we were configuring endpoints
                                else if self.configuring_slot == slot_id as u8 {
                                    serial_println!("xHCI: Endpoints Configured (Slot {}). Proceeding to SCSI Read...", slot_id);
                                    self.configuring_slot = 0;
                                    self.send_scsi_read(slot_id as u8);
                                }
                                else if self.slots[slot_id as usize].mouse_state == 1 {
                                    serial_println!("xHCI: Mouse Endpoints Configured (Slot {}). Proceeding to Set Configuration...", slot_id);
                                    self.slots[slot_id as usize].mouse_state = 2;
                                    self.send_set_configuration(slot_id as u8, 1);
                                }
                                else if self.slots[slot_id as usize].keyboard_state == 1 {
                                    serial_println!("xHCI: Keyboard Endpoints Configured (Slot {}). Proceeding to Set Configuration...", slot_id);
                                    self.slots[slot_id as usize].keyboard_state = 2;
                                    self.send_set_configuration(slot_id as u8, 1);
                                }
                                else {
                                    // UNA-19-IDENTITY: If pending_ports is empty, we assume Address Device just finished.
                                    serial_println!("xHCI: >>> SLOT {} ENABLED & ADDRESSED <<<", slot_id);
                                    self.request_device_descriptor(slot_id as u8);
                                }
                            }
                        } else {
                            serial_println!("xHCI: >>> COMMAND FAILED (Code {}) <<<", completion_code);
                            // UNA-19-HALT: Stop on Code 5
                            if completion_code == 5 {
                                serial_println!("xHCI: CRITICAL FAILURE: TRB ERROR (CODE 5).");
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

                        // UNA-19-REVEAL: If success or short packet, check buffer
                        if completion_code == 1 || completion_code == 13 {
                            // UNA-21-DEBUG: Force Transition based on Endpoint ID
                            // EP1 = Control (Device Descriptor)
                            // EP3 = Bulk IN (SCSI Read)

                            if endpoint_id == 1 && slot_id > 0 { // EP0 (Control) -> Device Descriptor
                                if self.slots[slot_id as usize].mouse_state == 2 {
                                    serial_println!("xHCI: >>> MOUSE SET_CONFIGURATION COMPLETE <<<");
                                    self.slots[slot_id as usize].mouse_state = 3;
                                    self.queue_mouse_read(slot_id as u8);
                                } else if self.slots[slot_id as usize].keyboard_state == 2 {
                                    serial_println!("xHCI: >>> KEYBOARD SET_CONFIGURATION COMPLETE <<<");
                                    self.slots[slot_id as usize].keyboard_state = 3;
                                    self.queue_keyboard_read(slot_id as u8);
                                } else {
                                    serial_println!("xHCI: >>> INTERCEPTED DESCRIPTOR EVENT (Slot 1 EP 1) <<<");
                                    unsafe {
                                        let desc_buf = self.slots[slot_id as usize].descriptor_buffer;
                                        let desc_data = core::slice::from_raw_parts(desc_buf, 256);
                                        let vid = (desc_data[8] as u16) | ((desc_data[9] as u16) << 8);
                                    let pid = (desc_data[10] as u16) | ((desc_data[11] as u16) << 8);

                                    serial_println!(">>> SYSTEM ALERT: NEW HARDWARE DETECTED <<<");
                                    serial_println!(">>> [CONTACT ESTABLISHED] SLOT {}", slot_id);
                                    serial_println!(">>> VENDOR ID : [{:04x}]", vid);
                                    serial_println!(">>> PRODUCT ID: [{:04x}]", pid);

                                    // UNA-22-HAUL: Inspect Class Code
                                    let class_code = desc_data[4];
                                    let subclass = desc_data[5];
                                    let protocol = desc_data[6];

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
                                        serial_println!("xHCI: Composite Device. Requesting Configuration Descriptor...");
                                        self.request_configuration_descriptor(slot_id as u8);
                                    } else if desc_data[1] == 0x02 { // Configuration Descriptor Response
                                        serial_println!("xHCI: >>> CONFIGURATION DESCRIPTOR RECEIVED <<<");
                                        // Parse Configuration Descriptor to find HID Interfaces
                                        let mut offset = 0;
                                        let total_length = (desc_data[2] as u16) | ((desc_data[3] as u16) << 8);
                                        serial_println!("xHCI: Configuration Descriptor Total Length: {}", total_length);
                                        
                                        // Track current interface state while parsing
                                        let mut current_intf_class: u8 = 0;
                                        let mut current_intf_protocol: u8 = 0;
                                        let mut found_hid = false;
                                        
                                        while offset < total_length as usize && offset < 256 {
                                            if offset + 1 >= 256 { break; }
                                            let length = desc_data[offset] as usize;
                                            if length == 0 { break; }
                                            let desc_type = desc_data[offset + 1];
                                            
                                            if desc_type == 0x04 { // Interface Descriptor
                                                if offset + 7 >= 256 { break; }
                                                current_intf_class = desc_data[offset + 5];
                                                let intf_subclass = desc_data[offset + 6];
                                                current_intf_protocol = desc_data[offset + 7];
                                                serial_println!("xHCI: Interface: Class={:#x} Sub={:#x} Proto={:#x}", 
                                                    current_intf_class, intf_subclass, current_intf_protocol);
                                                
                                                found_hid = current_intf_class == 0x03; // HID class
                                            } else if desc_type == 0x05 && found_hid { // Endpoint Descriptor
                                                if offset + 6 >= 256 { break; }
                                                let ep_addr = desc_data[offset + 2];
                                                let ep_attr = desc_data[offset + 3];
                                                if (ep_attr & 0x03) == 0x03 && (ep_addr & 0x80) != 0 { // Interrupt IN
                                                    let ep_mps = (desc_data[offset + 4] as u16) | ((desc_data[offset + 5] as u16) << 8);
                                                    let ep_interval = desc_data[offset + 6];
                                                    
                                                    if current_intf_protocol == 1 {
                                                        // USB HID Boot Keyboard
                                                        serial_println!("xHCI: >>> KEYBOARD INTERRUPT IN EP FOUND: {:#x}, MPS: {}, Interval: {} <<<", ep_addr, ep_mps, ep_interval);
                                                        self.slots[slot_id as usize].keyboard_ep = ep_addr;
                                                        self.slots[slot_id as usize].keyboard_mps = ep_mps;
                                                        self.slots[slot_id as usize].is_keyboard = true;
                                                        self.configure_keyboard_endpoints(slot_id as u8, ep_addr, ep_mps, ep_interval);
                                                        found_hid = false; // Don't double-match
                                                    } else {
                                                        // Mouse, Tablet, or generic HID (protocol 2 or 0)
                                                        serial_println!("xHCI: >>> MOUSE/TABLET INTERRUPT IN EP FOUND: {:#x}, MPS: {}, Interval: {} <<<", ep_addr, ep_mps, ep_interval);
                                                        self.slots[slot_id as usize].mouse_ep = ep_addr;
                                                        self.slots[slot_id as usize].mouse_mps = ep_mps;
                                                        self.slots[slot_id as usize].is_mouse = true;
                                                        self.configure_mouse_endpoints(slot_id as u8, ep_addr, ep_mps, ep_interval);
                                                        found_hid = false; // Don't double-match
                                                    }
                                                }
                                            }
                                            offset += length;
                                        }
                                    }
                                }
                                }
                            } else if endpoint_id > 1 && slot_id > 0 { // Non-EP0 Transfer Event
                                unsafe {
                                    let slot = &self.slots[slot_id as usize];
                                    
                                    // Compute expected DCI for mouse and keyboard
                                    let mouse_dci = if slot.is_mouse && slot.mouse_ep != 0 {
                                        let ep_num = slot.mouse_ep & 0x0F;
                                        let dir_in = (slot.mouse_ep & 0x80) != 0;
                                        Some((ep_num * 2) + if dir_in { 1 } else { 0 })
                                    } else { None };
                                    
                                    let keyboard_dci = if slot.is_keyboard && slot.keyboard_ep != 0 {
                                        let ep_num = slot.keyboard_ep & 0x0F;
                                        let dir_in = (slot.keyboard_ep & 0x80) != 0;
                                        Some((ep_num * 2) + if dir_in { 1 } else { 0 })
                                    } else { None };
                                    
                                    if mouse_dci == Some(endpoint_id as u8) {
                                        // --- MOUSE / TABLET ---
                                        if let Some(data_buf_ptr) = slot.data_buffer {
                                            let data_data = core::slice::from_raw_parts(data_buf_ptr, 512);
                                            let _buttons = data_data[0];
                                            let x = (data_data[1] as u16) | ((data_data[2] as u16) << 8);
                                            let y = (data_data[3] as u16) | ((data_data[4] as u16) << 8);
                                            
                                            if x != 0 || y != 0 {
                                                crate::pal::push_event(crate::pal::Event::MouseAbsolute { x: x as i32, y: y as i32 });
                                            }
                                            
                                            self.queue_mouse_read(slot_id as u8);
                                        }
                                    } else if keyboard_dci == Some(endpoint_id as u8) {
                                        // --- KEYBOARD ---
                                        if let Some(data_buf_ptr) = slot.data_buffer {
                                            let report = core::slice::from_raw_parts(data_buf_ptr, 8);
                                            // USB HID Boot Keyboard Report Format:
                                            // Byte 0: Modifier keys (bit 1 = L-Shift, bit 5 = R-Shift)
                                            // Byte 1: Reserved
                                            // Bytes 2-7: Key codes (up to 6 simultaneous keys)
                                            let modifiers = report[0];
                                            let shift = (modifiers & 0x22) != 0; // L-Shift (bit 1) or R-Shift (bit 5)
                                            
                                            for i in 2..8 {
                                                let keycode = report[i];
                                                if keycode == 0 { continue; } // No key
                                                if keycode == 1 { continue; } // ErrorRollOver
                                                
                                                if (keycode as usize) < HID_SCANCODE_TO_ASCII.len() {
                                                    let (unshifted, shifted) = HID_SCANCODE_TO_ASCII[keycode as usize];
                                                    let ascii = if shift { shifted } else { unshifted };
                                                    if ascii != 0 {
                                                        serial_println!("xHCI: KEY: '{}' (scancode {:#x})", ascii as char, keycode);
                                                        crate::pal::push_event(crate::pal::Event::Key(ascii));
                                                    }
                                                }
                                            }
                                            
                                            self.queue_keyboard_read(slot_id as u8);
                                        }
                                    } else if let Some(data_buf_ptr) = slot.data_buffer {
                                        // --- BULK IN (Mass Storage SCSI Read) ---
                                        let data_data = core::slice::from_raw_parts(data_buf_ptr, 512);
                                        serial_println!("xHCI: >>> BULK IN TRANSFER COMPLETE (SCSI Read) <<<");
                                        let sig = core::str::from_utf8(&data_data[0..21]).unwrap_or("INVALID");
                                        serial_println!("xHCI: SECTOR 0 SIGNATURE: {}", sig);

                                        if sig == "UNA-OS-DISK-001-ALPHA" {
                                            serial_println!("xHCI: >>> MISSION SUCCESS. TARGET ACQUIRED. <<<");
                                        } else {
                                            serial_println!("xHCI: >>> SIGNATURE MISMATCH <<<");
                                        }
                                        serial_println!(">>> DISK READ COMPLETE <<<");
                                    }
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

    pub unsafe fn init_pointers(&mut self, ring_phys_addr: u64) {
        unsafe {
            // 1. Allocate and set DCBAAP
            let dcbaap_size = (self.max_slots as usize + 1) * 8;
            let layout = core::alloc::Layout::from_size_align(dcbaap_size, 64).unwrap();
            let dcbaap_ptr = alloc::alloc::alloc_zeroed(layout) as *mut u64;
            self.dcbaap = dcbaap_ptr;

            let dcbaap_reg = (self.op_base + 0x30) as *mut u64;
            core::ptr::write_volatile(dcbaap_reg, dcbaap_ptr as u64);
            serial_println!("xHCI: DCBAAP set to {:#x}", dcbaap_ptr as u64);

            // 2. Set Command Ring Control Register (CRCR)
            // OpBase + 0x18.
            // MUST set Bit 0 (RCS - Ring Cycle State) to 1 to match our initial Ring state.
            let crcr_reg = (self.op_base + 0x18) as *mut u64;
            let crcr_value = ring_phys_addr | 1;
            core::ptr::write_volatile(crcr_reg, crcr_value);
            serial_println!("xHCI: CRCR set to {:#x}", crcr_value);
        }
    }

    // Call this AFTER init_pointers but BEFORE run
    pub fn init_interrupter(&mut self, event_ring_phys: u64, erst_table_phys: u64) {
        unsafe {
            // SAVE THIS for later use in the interrupt/event loop (ERDP updates)
            EVENT_RING_PHYS_BASE = event_ring_phys;

            // 1. Calculate Runtime Base
            // Read RTSOFF (Offset 0x18 in Capability Regs)
            let rtsoff_ptr = (self.base_addr + 0x18) as *const u32;
            let rtsoff = core::ptr::read_volatile(rtsoff_ptr) & !0x1F; // Clear lower 5 bits? Spec says 32-byte aligned.
            let runtime_base = self.base_addr + rtsoff as usize;

            // Interrupter 0 Base = RuntimeBase + 0x20
            let ir0_base = runtime_base + 0x20;
            serial_println!("xHCI: RuntimeBase={:#x}, IR0 Base={:#x}", runtime_base, ir0_base);

            // 2. Setup the Segment Table (ERST)
            // NOTE: Caller holds the EVENT_RING lock and passes us the phys addr.
            // Do NOT lock EVENT_RING here or we deadlock.
            ERST_TABLE.entries[0] = ErstEntry {
                ring_address: event_ring_phys,
                size: 16, // Must match EVENT_RING_SIZE in event.rs
                _rsvd: 0,
                _rsvd2: 0,
            };
            EVENT_RING_PHYS_BASE = event_ring_phys;

            // 3. Write ERSTSZ (Segment Table Size) - Offset 0x08
            // Value = 1 (We have 1 segment)
            let erstsz_ptr = (ir0_base + 0x08) as *mut u32;
            core::ptr::write_volatile(erstsz_ptr, 1);

            // 4. Write ERSTBA (Segment Table Base Address) - Offset 0x10
            let erstba_ptr = (ir0_base + 0x10) as *mut u64;
            core::ptr::write_volatile(erstba_ptr, erst_table_phys);

            // 5. Write ERDP (Event Ring Dequeue Pointer) - Offset 0x18
            // Initialize to the start of the ring.
            // PRESERVE BIT 3 (EHB - Event Handler Busy)? No, clear it initially.
            let erdp_ptr = (ir0_base + 0x18) as *mut u64;
            core::ptr::write_volatile(erdp_ptr, event_ring_phys); // Pointer to the RING, not the table

            // 6. GAG the Interrupter (IMAN - Interrupter Management) - Offset 0x00
            // Bit 0 = IP (Interrupt Pending), Bit 1 = IE (Interrupt Enable)
            // UNA-19-SILENCE: Clear Bit 1 (IE) and Bit 0 (IP)
            let iman_ptr = (ir0_base + 0x00) as *mut u32;
            let iman = core::ptr::read_volatile(iman_ptr);
            core::ptr::write_volatile(iman_ptr, iman & !0x3);

            serial_println!("xHCI: Interrupter 0 GAGGED (IMAN.IE Cleared).");
        }
    }

    pub fn start(&mut self) {
        unsafe {
            // Write 1 to USBCMD.RS (Run/Stop)
            let usbcmd_ptr = self.op_base as *mut u32;
            let cmd = core::ptr::read_volatile(usbcmd_ptr);
            core::ptr::write_volatile(usbcmd_ptr, cmd | 1);

            // Wait until USBSTS.HCH (Halted) is 0
            let usbsts_ptr = (self.op_base + 0x04) as *const u32;
            loop {
                let status = core::ptr::read_volatile(usbsts_ptr);
                if (status & 1) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            serial_println!("xHCI: Controller Started!");

            // Power on all ports
            // MaxPorts is in HCSPARAMS1 (Offset 0x04 from CapBase)
            let hcsparams1_ptr = (self.base_addr + 0x04) as *const u32;
            let max_ports = (core::ptr::read_volatile(hcsparams1_ptr) & 0xFF) as u8;
            serial_println!("xHCI: Max Ports = {}", max_ports);

            for i in 1..=max_ports {
                let port_offset = 0x400 + (i as usize - 1) * 0x10;
                let portsc_ptr = (self.op_base + port_offset) as *mut u32;
                let status = core::ptr::read_volatile(portsc_ptr);
                
                // Bit 9: PP (Port Power)
                if (status & (1 << 9)) == 0 {
                    serial_println!("xHCI: Powering on Port {}", i);
                    core::ptr::write_volatile(portsc_ptr, status | (1 << 9));
                } else {
                    serial_println!("xHCI: Port {} already powered. Status: {:#x}", i, status);
                }
            }

            // After powering on, manually trigger handle_port_change for any already connected devices
            for i in 1..=max_ports {
                let port_offset = 0x400 + (i as usize - 1) * 0x10;
                let portsc_ptr = (self.op_base + port_offset) as *const u32;
                let status = core::ptr::read_volatile(portsc_ptr);
                
                if (status & 1) != 0 || (status & (1 << 17)) != 0 {
                    serial_println!("xHCI DEBUG: Port {} matched! Calling handle_port_change. Status: {:#x}", i, status);
                    self.handle_port_change(i);
                }
            }
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
        self.pending_ports.push(port_id);

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

            // 0. Allocate Contexts and Ring
            let input_layout = core::alloc::Layout::from_size_align(core::mem::size_of::<InputContext>(), 64).unwrap();
            let output_layout = core::alloc::Layout::from_size_align(core::mem::size_of::<DeviceContext>(), 64).unwrap();
            
            let input_ctx_virt = alloc::alloc::alloc_zeroed(input_layout) as *mut InputContext;
            let output_ctx_virt = alloc::alloc::alloc_zeroed(output_layout) as *mut DeviceContext;
            let ep0_ring = ring::TransferRing::new(16);
            let ep0_ring_phys = ep0_ring.get_ptr();

            let output_ctx_phys = output_ctx_virt as u64;
            let input_ctx_phys = input_ctx_virt as u64;

            // Store them in slot
            let slot = &mut self.slots[slot_id as usize];
            slot.input_context = input_ctx_virt;
            slot.output_context = output_ctx_virt;
            slot.ep0_ring = Some(ep0_ring);
            slot.port_id = port_id;
            slot.active = true;

            // 1. LINK DCBAAP
            // Point the Slot ID entry to the Output Context
            let dcbaap_ptr = self.dcbaap;
            *dcbaap_ptr.add(slot_id as usize) = output_ctx_phys;
            serial_println!("xHCI: DCBAAP[{}] linked to {:#x}", slot_id, output_ctx_phys);

            // 2. FILL INPUT CONTEXT (MANUAL OFFSET CALCULATION)
            let base_ptr = input_ctx_virt as *mut u32;

            // Clear Input Context (33 * 32 = 1056 bytes)
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);

            // 3a. INPUT CONTROL CONTEXT (Offset 0x00)
            base_ptr.add(1).write_volatile(3); // Enable Slot (Bit 0) and EP0 (Bit 1)

            // 3b. SLOT CONTEXT (Offset 0x20 -> Index 8 in u32)
            let slot_ctx_ptr = base_ptr.add(8);
            slot_ctx_ptr.add(0).write_volatile(1 << 27); // Context Entries (Bits 27-31) = 1
            slot_ctx_ptr.add(1).write_volatile((port_id as u32) << 16); // Root Hub Port Number

            // 3c. ENDPOINT 0 CONTEXT (Offset 0x40 -> Index 16 in u32)
            let ep0_ctx_ptr = base_ptr.add(16);
            ep0_ctx_ptr.add(1).write_volatile((4 << 3) | (3 << 1) | (64 << 16)); // EP Type = 4, CErr = 3, MPS = 64
            ep0_ctx_ptr.add(2).write_volatile((ep0_ring_phys as u32) | 1); // Bit 0 must match Cycle Bit (1)
            ep0_ctx_ptr.add(3).write_volatile((ep0_ring_phys >> 32) as u32);
            ep0_ctx_ptr.add(4).write_volatile(8); // Average TRB Length = 8

            serial_println!("xHCI: Input Context Initialized (Manual Offsets). Phys={:#x}", input_ctx_phys);

            // 4. SEND ADDRESS DEVICE COMMAND
            let trb = Trb {
                parameter: input_ctx_phys,
                status: 0,
                control: (11 << 10) | ((slot_id as u32) << 24),
            };

            if let Err(e) = self.send_command(trb) {
                serial_println!("xHCI: Failed to send Address Device command: {}", e);
            } else {
                self.configuring_slot = slot_id;
            }
        }
    }

    pub fn configure_endpoints(&mut self, slot_id: u8) {
        unsafe {
            serial_println!("xHCI: UNA-21 Configuring Endpoints for Slot {}...", slot_id);

            // 1. GET POINTERS
            let slot = &mut self.slots[slot_id as usize];
            let input_ctx_virt = slot.input_context;
            let output_ctx_virt = slot.output_context;
            let base_ptr = input_ctx_virt as *mut u32;
            
            let bulk_in_ring = ring::TransferRing::new(16);
            let bulk_in_phys = bulk_in_ring.get_ptr();
            slot.bulk_in_ring = Some(bulk_in_ring);

            let bulk_out_ring = ring::TransferRing::new(16);
            let bulk_out_phys = bulk_out_ring.get_ptr();
            slot.bulk_out_ring = Some(bulk_out_ring);

            let data_layout = core::alloc::Layout::from_size_align(512, 64).unwrap();
            slot.data_buffer = Some(alloc::alloc::alloc_zeroed(data_layout));

            // 2. CLEAR INPUT CONTEXT (Safety first)
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);

            // 3. INPUT CONTROL CONTEXT (Offset 0x00)
            base_ptr.add(1).write_volatile(0x18);

            // 4. SLOT CONTEXT (Offset 0x20 -> Index 8)
            let slot_ctx_ptr = base_ptr.add(8);
            // Copy from OUTPUT_CONTEXT
            for i in 0..8 {
                let val = core::ptr::read_volatile((output_ctx_virt as *const u32).add(i));
                slot_ctx_ptr.add(i).write_volatile(val);
            }
            // Update Context Entries = 5 (Bits 27:31)
            let old_dw0 = slot_ctx_ptr.add(0).read_volatile();
            let new_dw0 = (old_dw0 & !(0x1F << 27)) | (5 << 27);
            slot_ctx_ptr.add(0).write_volatile(new_dw0);

            // 5. ENDPOINT 1 IN (Index 3) -> Offset 0x60
            let ep1_in_ptr = base_ptr.add(24);
            ep1_in_ptr.add(1).write_volatile((6 << 3) | (3 << 1) | (512 << 16));
            ep1_in_ptr.add(2).write_volatile((bulk_in_phys as u32) | 1);
            ep1_in_ptr.add(3).write_volatile((bulk_in_phys >> 32) as u32);
            ep1_in_ptr.add(4).write_volatile(512);

            // 6. ENDPOINT 2 OUT (Index 4) -> Offset 0x80
            let ep2_out_ptr = base_ptr.add(32);
            ep2_out_ptr.add(1).write_volatile((2 << 3) | (3 << 1) | (512 << 16));
            ep2_out_ptr.add(2).write_volatile((bulk_out_phys as u32) | 1);
            ep2_out_ptr.add(3).write_volatile((bulk_out_phys >> 32) as u32);
            ep2_out_ptr.add(4).write_volatile(512);

            serial_println!("xHCI: Input Context Configured for Bulk Transport.");

            // 7. SEND CONFIGURE ENDPOINT COMMAND
            let trb = Trb {
                parameter: input_ctx_virt as u64,
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

            let (desc_phys, data_phys, cbw_ptr) = {
                let slot = &self.slots[slot_id as usize];
                (slot.descriptor_buffer as u64, slot.data_buffer.unwrap() as u64, slot.descriptor_buffer)
            };

            core::ptr::write_bytes(cbw_ptr, 0, 64);

            *cbw_ptr.add(0) = 0x55; *cbw_ptr.add(1) = 0x53; *cbw_ptr.add(2) = 0x42; *cbw_ptr.add(3) = 0x43;
            *cbw_ptr.add(4) = 0xEF; *cbw_ptr.add(5) = 0xBE; *cbw_ptr.add(6) = 0xAD; *cbw_ptr.add(7) = 0xDE;
            *cbw_ptr.add(8) = 0x00; *cbw_ptr.add(9) = 0x02; *cbw_ptr.add(10) = 0x00; *cbw_ptr.add(11) = 0x00;
            *cbw_ptr.add(12) = 0x80; *cbw_ptr.add(13) = 0x00; *cbw_ptr.add(14) = 10;
            *cbw_ptr.add(15) = 0x28; *cbw_ptr.add(23) = 1;

            let out_trb = Trb {
                parameter: desc_phys,
                status: 31,
                control: (1 << 10) | (1 << 5),
            };
            self.slots[slot_id as usize].bulk_out_ring.as_mut().unwrap().push(out_trb).unwrap();
            self.ring_doorbell(slot_id, 4);

            let in_trb = Trb {
                parameter: data_phys,
                status: 512,
                control: (1 << 10) | (1 << 5) | (1 << 2),
            };
            self.slots[slot_id as usize].bulk_in_ring.as_mut().unwrap().push(in_trb).unwrap();
            self.ring_doorbell(slot_id, 3);

            serial_println!("xHCI: SCSI Command Dispatched.");
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

    fn push_ep0(&mut self, slot_id: u8, trb: Trb) {
        unsafe {
            if let Some(ep0_ring) = &mut self.slots[slot_id as usize].ep0_ring {
                ep0_ring.push(trb).unwrap();
            } else {
                serial_println!("xHCI: push_ep0 failed, no ep0_ring for slot {}", slot_id);
            }
        }
    }

    pub fn request_device_descriptor(&mut self, slot_id: u8) {
        serial_println!("xHCI: Requesting Device Descriptor for Slot {}...", slot_id);

        let desc_phys = self.slots[slot_id as usize].descriptor_buffer as u64;
        if desc_phys == 0 {
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
        self.push_ep0(slot_id, setup_trb);

        // 2. Data Stage
        let data_trb = Trb {
            parameter: desc_phys,
            status: 18, // Length 18 bytes
            control: (3 << 10) // Type 3 (Data Stage)
                   | (1 << 16), // DIR (1 = IN)
        };
        self.push_ep0(slot_id, data_trb);

        // 3. Status Stage
        let status_trb = Trb {
            parameter: 0,
            status: 0,
            control: (4 << 10) // Type 4 (Status Stage)
                   | (1 << 5)  // IOC (Interrupt On Completion)
                   | (0 << 16), // DIR (0 = OUT)
        };
        self.push_ep0(slot_id, status_trb);

        // 4. Ring Doorbell (Slot 1, Target 1 for EP0)
        self.ring_doorbell(slot_id, 1);
    }

    pub fn request_configuration_descriptor(&mut self, slot_id: u8) {
        serial_println!("xHCI: Requesting Configuration Descriptor for Slot {}...", slot_id);

        let desc_phys = self.slots[slot_id as usize].descriptor_buffer as u64;
        if desc_phys == 0 {
            serial_println!("xHCI: CRITICAL ERROR - Descriptor Buffer Phys Addr is 0!");
            return;
        }

        // 1. Setup Stage
        // bmRequestType = 0x80 (Device to Host, Standard, Device)
        // bRequest = 0x06 (GET_DESCRIPTOR)
        // wValue = 0x0200 (Descriptor Type = 2 for Configuration, Index = 0)
        // wIndex = 0x0000
        // wLength = 0x0040 (64 bytes)
        // Little Endian u64: 0x0040000002000680
        let setup_trb = Trb {
            parameter: 0x0040000002000680,
            status: 8, // Transfer Length
            control: (2 << 10) | (1 << 6) | (3 << 16), // Type 2 | IDT | TRT (IN)
        };
        self.push_ep0(slot_id, setup_trb);

        // 2. Data Stage
        let data_trb = Trb {
            parameter: desc_phys,
            status: 64, // Length 64 bytes
            control: (3 << 10) | (1 << 16), // Type 3 | DIR (IN)
        };
        self.push_ep0(slot_id, data_trb);

        // 3. Status Stage
        let status_trb = Trb {
            parameter: 0,
            status: 0,
            control: (4 << 10) | (1 << 5) | (0 << 16), // Type 4 | IOC | DIR (OUT)
        };
        self.push_ep0(slot_id, status_trb);

        // 4. Ring Doorbell
        self.ring_doorbell(slot_id, 1);
    }

    pub fn configure_mouse_endpoints(&mut self, slot_id: u8, ep_addr: u8, mps: u16, interval: u8) {
        unsafe {
            serial_println!("xHCI: Configuring Mouse Endpoints for Slot {}, EP Addr {:#x}...", slot_id, ep_addr);

            // 1. GET POINTERS
            let slot = &mut self.slots[slot_id as usize];
            let input_ctx_virt = slot.input_context;
            let output_ctx_virt = slot.output_context;
            let base_ptr = input_ctx_virt as *mut u32;

            let mouse_ring = ring::TransferRing::new(16);
            let mouse_ring_phys = mouse_ring.get_ptr();
            slot.mouse_ring = Some(mouse_ring);

            let data_layout = core::alloc::Layout::from_size_align(512, 64).unwrap();
            slot.data_buffer = Some(alloc::alloc::alloc_zeroed(data_layout));

            // 2. CLEAR INPUT CONTEXT
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);

            // The DCI (Device Context Index) for an endpoint is:
            // DCI = (Endpoint Number * 2) + Direction
            // Where Direction is 1 for IN, 0 for OUT
            let ep_num = ep_addr & 0x0F;
            let dir_in = (ep_addr & 0x80) != 0;
            let dci = (ep_num * 2) + if dir_in { 1 } else { 0 };

            // Input Control Context
            base_ptr.add(1).write_volatile((1 << dci) | 1);

            // Slot Context
            let slot_ctx_ptr = base_ptr.add(8);
            // Copy from OUTPUT_CONTEXT
            for i in 0..8 {
                let val = core::ptr::read_volatile((output_ctx_virt as *const u32).add(i));
                slot_ctx_ptr.add(i).write_volatile(val);
            }
            // Update Context Entries = DCI (Bits 27:31)
            let old_dw0 = slot_ctx_ptr.add(0).read_volatile();
            let new_dw0 = (old_dw0 & !(0x1F << 27)) | ((dci as u32) << 27);
            slot_ctx_ptr.add(0).write_volatile(new_dw0);

            let ep_ctx_ptr = base_ptr.add(16 + ((dci - 1) * 8) as usize);
            
            // Read Speed from OUTPUT_CONTEXT Slot Context DW0 (Bits 20:23)
            let out_dw0 = core::ptr::read_volatile((output_ctx_virt as *const u32).add(0));
            let speed = (out_dw0 >> 20) & 0x0F;

            // Interval Calculation depends on Speed
            let interval_xhci = if speed == 3 || speed >= 4 {
                // High-Speed (3) and SuperSpeed (4+): bInterval - 1
                (interval.saturating_sub(1)) as u32
            } else {
                // Low-Speed / Full-Speed: RoundDown(log2(bInterval)) + 3
                if interval > 0 {
                    (31 - (interval as u32).leading_zeros()) + 3
                } else {
                    0
                }
            };

            // DW0: Interval (16:23) | Max ESIT Payload (24:31)
            ep_ctx_ptr.add(0).write_volatile((interval_xhci << 16) | ((mps as u32) << 24));

            // DW1: MPS=mps, EP Type=7 (Interrupt IN), CErr=3
            ep_ctx_ptr.add(1).write_volatile((7 << 3) | (3 << 1) | ((mps as u32) << 16));

            // DW2: Dequeue Pointer Lo | DCS (Cycle Bit = 1)
            ep_ctx_ptr.add(2).write_volatile((mouse_ring_phys as u32) | 1);
            // DW3: Dequeue Pointer Hi
            ep_ctx_ptr.add(3).write_volatile((mouse_ring_phys >> 32) as u32);
            // DW4: Avg TRB Len
            ep_ctx_ptr.add(4).write_volatile(mps as u32);

            serial_println!("xHCI: Input Context Configured for Mouse Interrupt IN (DCI {}).", dci);

            let trb = Trb {
                parameter: input_ctx_virt as u64,
                status: 0,
                control: (12 << 10) | ((slot_id as u32) << 24),
            };

            if let Err(e) = self.send_command(trb) {
                serial_println!("xHCI: Failed to send Configure Endpoint command: {}", e);
            } else {
                self.slots[slot_id as usize].mouse_state = 1; // Waiting for Configure Endpoint completion
                self.ring_doorbell(0, 0); // Ring doorbell for Command Ring
            }
        }
    }

    pub fn send_set_configuration(&mut self, slot_id: u8, config_val: u8) {
        unsafe {
            serial_println!("xHCI: Sending SET_CONFIGURATION({}) to Slot {}", config_val, slot_id);
            let setup_trb = Trb {
                parameter: 0x0000000000000900 | ((config_val as u64) << 16), // bmRequestType=0, bRequest=9 (SET_CONFIGURATION), wValue=config_val
                status: 8, // Length 8
                control: (2 << 10) | (0 << 16) | (1 << 6), // Type 2 (Setup Stage), TRT=0 (No Data Stage), IDT=1
            };
            let s_param = setup_trb.parameter;
            let s_status = setup_trb.status;
            let s_ctrl = setup_trb.control;
            serial_println!("xHCI: Setup TRB -> Param: {:#x}, Status: {:#x}, Control: {:#x}", s_param, s_status, s_ctrl);
            self.push_ep0(slot_id, setup_trb);

            let status_trb = Trb {
                parameter: 0,
                status: 0,
                control: (4 << 10) | (1 << 5) | (1 << 16), // Type 4 (Status Stage), IOC=1, DIR=1 (IN)
            };
            let st_param = status_trb.parameter;
            let st_status = status_trb.status;
            let st_ctrl = status_trb.control;
            serial_println!("xHCI: Status TRB -> Param: {:#x}, Status: {:#x}, Control: {:#x}", st_param, st_status, st_ctrl);
            self.push_ep0(slot_id, status_trb);

            self.ring_doorbell(slot_id, 1);
        }
    }

    pub fn queue_mouse_read(&mut self, slot_id: u8) {
        unsafe {
            let ep_num = self.slots[slot_id as usize].mouse_ep & 0x0F;
            let dir_in = (self.slots[slot_id as usize].mouse_ep & 0x80) != 0;
            let dci = (ep_num * 2) + if dir_in { 1 } else { 0 };

            let data_phys = self.slots[slot_id as usize].data_buffer.unwrap() as u64;

            let in_trb = Trb {
                parameter: data_phys,
                status: self.slots[slot_id as usize].mouse_mps as u32, // Length
                control: (1 << 10) | (1 << 5), // Type 1 | IOC
            };
            self.slots[slot_id as usize].mouse_ring.as_mut().unwrap().push(in_trb).unwrap();
            self.ring_doorbell(slot_id, dci as u32);
            serial_println!("xHCI: Mouse Read Queued.");
        }
    }

    pub fn configure_keyboard_endpoints(&mut self, slot_id: u8, ep_addr: u8, mps: u16, interval: u8) {
        unsafe {
            serial_println!("xHCI: Configuring Keyboard Endpoints for Slot {}, EP Addr {:#x}...", slot_id, ep_addr);

            // 1. GET POINTERS
            let slot = &mut self.slots[slot_id as usize];
            let input_ctx_virt = slot.input_context;
            let output_ctx_virt = slot.output_context;
            let base_ptr = input_ctx_virt as *mut u32;

            let keyboard_ring = ring::TransferRing::new(16);
            let keyboard_ring_phys = keyboard_ring.get_ptr();
            slot.keyboard_ring = Some(keyboard_ring);

            // Allocate data buffer if not already allocated
            if slot.data_buffer.is_none() {
                let data_layout = core::alloc::Layout::from_size_align(512, 64).unwrap();
                slot.data_buffer = Some(alloc::alloc::alloc_zeroed(data_layout));
            }

            // 2. CLEAR INPUT CONTEXT
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);

            // Compute DCI: DCI = (Endpoint Number * 2) + Direction (1=IN, 0=OUT)
            let ep_num = ep_addr & 0x0F;
            let dir_in = (ep_addr & 0x80) != 0;
            let dci = (ep_num * 2) + if dir_in { 1 } else { 0 };

            // Input Control Context: Add flag for this DCI + Slot Context
            base_ptr.add(1).write_volatile((1 << dci) | 1);

            // Slot Context: Copy from Output Context
            let slot_ctx_ptr = base_ptr.add(8);
            for i in 0..8 {
                let val = core::ptr::read_volatile((output_ctx_virt as *const u32).add(i));
                slot_ctx_ptr.add(i).write_volatile(val);
            }
            // Update Context Entries = DCI
            let old_dw0 = slot_ctx_ptr.add(0).read_volatile();
            let new_dw0 = (old_dw0 & !(0x1F << 27)) | ((dci as u32) << 27);
            slot_ctx_ptr.add(0).write_volatile(new_dw0);

            // Endpoint Context at offset for this DCI
            let ep_ctx_ptr = base_ptr.add(16 + ((dci - 1) * 8) as usize);

            // Read Speed from Output Context Slot Context DW0 (Bits 20:23)
            let out_dw0 = core::ptr::read_volatile((output_ctx_virt as *const u32).add(0));
            let speed = (out_dw0 >> 20) & 0x0F;

            // Interval depends on speed
            let interval_xhci = if speed == 3 || speed >= 4 {
                (interval.saturating_sub(1)) as u32
            } else {
                if interval > 0 {
                    (31 - (interval as u32).leading_zeros()) + 3
                } else {
                    0
                }
            };

            // DW0: Interval | Max ESIT Payload
            ep_ctx_ptr.add(0).write_volatile((interval_xhci << 16) | ((mps as u32) << 24));

            // DW1: MPS=mps, EP Type=7 (Interrupt IN), CErr=3
            ep_ctx_ptr.add(1).write_volatile((7 << 3) | (3 << 1) | ((mps as u32) << 16));

            // DW2: Dequeue Pointer Lo | DCS (Cycle Bit = 1)
            ep_ctx_ptr.add(2).write_volatile((keyboard_ring_phys as u32) | 1);
            // DW3: Dequeue Pointer Hi
            ep_ctx_ptr.add(3).write_volatile((keyboard_ring_phys >> 32) as u32);
            // DW4: Avg TRB Len
            ep_ctx_ptr.add(4).write_volatile(mps as u32);

            serial_println!("xHCI: Input Context Configured for Keyboard Interrupt IN (DCI {}).", dci);

            let trb = Trb {
                parameter: input_ctx_virt as u64,
                status: 0,
                control: (12 << 10) | ((slot_id as u32) << 24),
            };

            if let Err(e) = self.send_command(trb) {
                serial_println!("xHCI: Failed to send Configure Endpoint command: {}", e);
            } else {
                self.slots[slot_id as usize].keyboard_state = 1;
                self.ring_doorbell(0, 0);
            }
        }
    }

    pub fn queue_keyboard_read(&mut self, slot_id: u8) {
        unsafe {
            let ep_num = self.slots[slot_id as usize].keyboard_ep & 0x0F;
            let dir_in = (self.slots[slot_id as usize].keyboard_ep & 0x80) != 0;
            let dci = (ep_num * 2) + if dir_in { 1 } else { 0 };

            let data_phys = self.slots[slot_id as usize].data_buffer.unwrap() as u64;

            let in_trb = Trb {
                parameter: data_phys,
                status: self.slots[slot_id as usize].keyboard_mps as u32,
                control: (1 << 10) | (1 << 5), // Type 1 (Normal) | IOC
            };
            self.slots[slot_id as usize].keyboard_ring.as_mut().unwrap().push(in_trb).unwrap();
            self.ring_doorbell(slot_id, dci as u32);
            serial_println!("xHCI: Keyboard Read Queued.");
        }
    }
}
