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
use lazy_static::lazy_static;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

/// IDT vectors. This is a pure local-APIC system — there is no 8259 PIC, hence no PIC vector
/// offset. The APIC timer fires `TIMER_VECTOR`, the xHCI MSI-X interrupter (interrupter 0)
/// fires `XHCI_MSI_VECTOR`, and `SPURIOUS_VECTOR` == the APIC SVR low byte.
pub const TIMER_VECTOR: u8 = 0x20;
pub const XHCI_MSI_VECTOR: u8 = 0x40;
/// Inter-processor interrupt vector (reschedule/wake; scheduler foundation). 0x41 is reserved
/// for the NIC, so IPIs use 0x42.
pub const IPI_VECTOR: u8 = 0x42;
pub const SPURIOUS_VECTOR: u8 = 0xFF;

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
        // All interrupts are delivered directly by the local APIC: the timer (heartbeat),
        // the xHCI MSI-X interrupter, and the APIC spurious-interrupt vector.
        idt[TIMER_VECTOR].set_handler_fn(timer_interrupt_handler);
        idt[XHCI_MSI_VECTOR].set_handler_fn(xhci_msi_handler);
        idt[IPI_VECTOR].set_handler_fn(ipi_handler);
        idt[SPURIOUS_VECTOR].set_handler_fn(spurious_handler);
        idt
    };
}

pub fn init_idt() {
    IDT.load();
}

/// Disable the legacy 8259 PIC by masking every IRQ line, so it can never assert. This is a
/// pure local-APIC system (APIC timer + xHCI MSI-X), and LINT0 is no longer wired as ExtINT
/// (see `apic::init`), so nothing should ever arrive via the PIC. We don't trust firmware to
/// have masked it — we silence it explicitly with raw OCW1 writes to the data ports
/// (0x21 = PIC1, 0xA1 = PIC2). The PS/2 8042 controller is likewise left untouched and silent
/// (its IRQs are masked here and undeliverable). No legacy ISA interrupt source reaches the CPU.
pub fn disable_legacy_pic() {
    use x86_64::instructions::port::Port;
    unsafe {
        Port::<u8>::new(0x21).write(0xFF); // mask all IRQs on PIC1
        Port::<u8>::new(0xA1).write(0xFF); // mask all IRQs on PIC2
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
    // Local APIC timer tick. Lock-free: bump the global heartbeat plus this CPU's own tick
    // counter (each core's timer fires independently), then issue an APIC EOI.
    let prev = crate::arch::apic::APIC_TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    crate::arch::percpu::note_tick();
    // One-shot breadcrumb the very first time any CPU's timer fires — confirms the local-APIC
    // timer path (xAPIC MMIO or x2APIC MSR LVT) is delivering, without spamming every tick.
    if prev == 0 {
        serial_println!("APIC: heartbeat live (first timer tick).");
    }
    crate::arch::apic::eoi();
}

/// Inter-processor interrupt handler (IDT vector `IPI_VECTOR`). Lock-free: record the IPI on
/// this CPU and EOI. This is the scheduler-wakeup primitive — an IPI knocks a core out of `hlt`
/// so it can re-check its run queue; for now it just proves cross-CPU signalling works.
extern "x86-interrupt" fn ipi_handler(_stack_frame: InterruptStackFrame) {
    crate::arch::percpu::note_ipi();
    crate::arch::apic::eoi();
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
