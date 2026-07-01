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
pub const NIC_MSI_VECTOR: u8 = 0x41;
/// Inter-processor interrupt vector (reschedule/wake; scheduler foundation). 0x41 is reserved
/// for the NIC, so IPIs use 0x42.
pub const IPI_VECTOR: u8 = 0x42;
pub const SPURIOUS_VECTOR: u8 = 0xFF;

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        // Minimal, lock-free NMI handler. LINT1 is wired as NMI (see `apic::init`), and NMIs
        // ignore IF — so one can land mid-context-switch. This handler must never touch run
        // queues, scheduler state, or any spin lock; it just counts and returns.
        idt.non_maskable_interrupt.set_handler_fn(nmi_handler);
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
        idt[NIC_MSI_VECTOR].set_handler_fn(nic_msi_handler);
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
    // Local APIC timer tick. Lock-free. This CPU's own tick counter (each core's timer fires
    // independently at the calibrated 1 kHz) drives the per-CPU `sleep_ticks` deadlines.
    crate::arch::percpu::note_tick();
    // The GLOBAL millisecond clock (`APIC_TICKS`, read by `ticks()`/`ms()`) is advanced by ONE core
    // only — the BSP (logical cpu 0). Every core ticks at 1 kHz, so summing all of them would run
    // the "ms since boot" clock at (core-count) kHz — 8× fast on the 8-core rMBP. The BSP is always
    // online and services the main loop, so its 1 kHz tick is the single-rate wall-clock heartbeat.
    if crate::arch::percpu::this_cpu().cpu_index == 0 {
        let prev = crate::arch::apic::APIC_TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // One-shot breadcrumb the first time the BSP's timer fires — confirms the local-APIC timer
        // path (xAPIC MMIO or x2APIC MSR LVT) is delivering, without spamming every tick.
        if prev == 0 {
            serial_println!("APIC: heartbeat live (first timer tick).");
        }
    }
    // EOI BEFORE any context switch: otherwise the in-service bit would block this CPU's
    // subsequent timer ticks for the whole descheduled lifetime of a preempted task.
    crate::arch::apic::eoi();
    // Preemption point. No-op unless a scheduled task is running on THIS cpu and its quantum
    // expired; runs with IF=0 (interrupt gate) and the preempted task's `iretq` restores its IF.
    crate::arch::sched::timer_preempt();
}

/// Inter-processor interrupt handler (IDT vector `IPI_VECTOR`). Lock-free and WAKE-ONLY: record
/// the IPI on this CPU and EOI. It deliberately does NOT context-switch — its whole job is that
/// returning from the interrupt breaks the scheduler's idle `hlt`, so the per-CPU scheduler loop
/// re-checks its run queue and picks up work a `spawn` just enqueued. Keeping it switch-free is
/// what makes the running task's `current` pointer single-owner (only the scheduler loop and the
/// timer preempt site ever switch).
extern "x86-interrupt" fn ipi_handler(_stack_frame: InterruptStackFrame) {
    crate::arch::percpu::note_ipi();
    crate::arch::apic::eoi();
}

/// Count of NMIs taken (lock-free introspection; see the NMI handler below).
pub static NMI_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Non-maskable interrupt handler. NMIs ignore IF, so this can interrupt a context switch in
/// progress; it must stay leaf and lock-free (no run queues, no `current`, no spin locks). We
/// only count and return — that keeps `switch_context` NMI-reentrant-safe.
extern "x86-interrupt" fn nmi_handler(_stack_frame: InterruptStackFrame) {
    NMI_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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

/// e1000e MSI handler (IDT vector 0x41). Lock-free, mirroring the xHCI handler: acknowledge
/// the NIC (read ICR to clear its interrupt causes via raw MMIO) so it can raise again, then
/// EOI the local APIC. It does NOT drain the RX ring — that happens in the polled main loop
/// (`e1000::service_net`), which owns the NET_DEVICE lock; taking that lock here could
/// deadlock. The interrupt's purpose is to wake the CPU from `hlt` so RX is serviced promptly.
extern "x86-interrupt" fn nic_msi_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::e1000::interrupt_ack();
    crate::arch::apic::eoi();
}

/// Local APIC spurious-interrupt handler (vector 0xFF, == APIC SVR low byte). By definition
/// the APIC did not actually deliver an interrupt here, so we must NOT send an EOI.
extern "x86-interrupt" fn spurious_handler(_stack_frame: InterruptStackFrame) {}
