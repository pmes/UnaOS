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

use crate::arch::gdt;
use crate::pal::Event;
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

// FIX: Only import what we use to kill warnings


pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// IDT vectors delivered straight to the local APIC (no 8259/PIC offset).
/// xHCI MSI-X (interrupter 0) and the APIC spurious-interrupt vector (== APIC SVR low byte).
pub const XHCI_MSI_VECTOR: u8 = 0x40;
pub const SPURIOUS_VECTOR: u8 = 0xFF;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_u8()].set_handler_fn(mouse_interrupt_handler);
        // xHCI MSI-X (interrupter 0) delivered directly to the local APIC at vector 0x40 —
        // no 8259/INTx involvement. Spurious-interrupt vector 0xFF matches APIC SVR.
        idt[XHCI_MSI_VECTOR].set_handler_fn(xhci_msi_handler);
        idt[SPURIOUS_VECTOR].set_handler_fn(spurious_handler);
        idt
    };
}

pub fn init_idt() {
    IDT.load();
}

pub fn init_pics() {
    unsafe {
        let mut pics = PICS.lock();
        pics.initialize();
        // Unmask IRQ0 (Timer) and IRQ1 (Keyboard) on PIC1, mask all on PIC2
        pics.write_masks(0b1111_1100, 0b1111_1111);
    }

    // Re-enable and flush 8042 PS/2 Controller (UEFI may leave it disabled/full)
    unsafe {
        use x86_64::instructions::port::Port;
        let mut cmd_port = Port::<u8>::new(0x64);
        let mut data_port = Port::<u8>::new(0x60);

        // 1. Flush any pending data (bounded: a wedged 8042 must not hang boot)
        let mut flush_guard = 0u32;
        while (cmd_port.read() & 1) != 0 && flush_guard < 256 {
            data_port.read();
            flush_guard += 1;
        }

        // 2. Read Configuration Byte
        cmd_port.write(0x20);
        let mut config = data_port.read();

        // 3. Enable First PS/2 Port Interrupt (bit 0)
        config |= 0b0000_0001;

        // 4. Write Configuration Byte
        cmd_port.write(0x60);
        data_port.write(config);

        // 5. Flush any pending data again (bounded, same rationale as above)
        let mut flush_guard2 = 0u32;
        while (cmd_port.read() & 1) != 0 && flush_guard2 < 256 {
            data_port.read();
            flush_guard2 += 1;
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard = PIC_1_OFFSET + 1,
    Mouse = PIC_2_OFFSET + 12,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn as_usize(self) -> usize {
        self as usize
    }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    serial_println!("EXCEPTION: PAGE FAULT");
    serial_println!("Accessed Address: {:?}", Cr2::read());
    serial_println!("Error Code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    crate::hlt_loop();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!("EXCEPTION: GENERAL PROTECTION FAULT");
    serial_println!("Error Code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    crate::hlt_loop();
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::<u8>::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    serial_println!("KEY: {:#X}", scancode);

    let character = match scancode {
        0x02..=0x0B => Some(b"1234567890"[scancode as usize - 0x02]),
        0x10..=0x19 => Some(b"qwertyuiop"[scancode as usize - 0x10]),
        0x1E..=0x26 => Some(b"asdfghjkl"[scancode as usize - 0x1E]),
        0x2C..=0x32 => Some(b"zxcvbnm"[scancode as usize - 0x2C]),
        0x39 => Some(b' '),
        0x1C => Some(b'\n'),
        _ => None,
    };

    if let Some(character) = character {
        crate::pal::push_event(Event::Key(character));
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

/// xHCI MSI-X handler (interrupter 0, IDT vector 0x40). Minimal and lock-free: it
/// acknowledges the controller (clears IMAN.IP / USBSTS.EINT via raw MMIO) so the
/// interrupter can raise again, then EOIs the local APIC. It does NOT drain the event ring
/// — that happens in the polled context (main loop / BOT pump), which owns the controller +
/// event-ring locks. Touching those locks here would self-deadlock (the main loop holds
/// XHCI_CONTROLLER across the synchronous BOT pump). The interrupt's purpose is to wake the
/// CPU from `hlt` so QEMU's main loop runs the async completion and the pump promptly drains
/// the resulting event.
extern "x86-interrupt" fn xhci_msi_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::xhci::interrupt_ack();
    crate::arch::apic::eoi();
}

/// Local APIC spurious-interrupt handler (vector 0xFF, == APIC SVR low byte). By definition
/// the APIC did not actually deliver an interrupt here, so we must NOT send an EOI.
extern "x86-interrupt" fn spurious_handler(_stack_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let _packet: u8 = unsafe { port.read() };

    // Using a safe coordinate for now
    crate::pal::push_event(Event::Mouse { x: 100, y: 100 });

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Mouse.as_u8());
    }
}
