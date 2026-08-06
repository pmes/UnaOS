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
use x86_64::registers::rflags::RFlags;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

// x86 exception vector numbers (Intel SDM Vol.3 Table 6-1). Reported in the U1b kill line so the
// verdict can match each fixture to the exact fault it was built to provoke.
const VEC_DE: u8 = 0; // divide error
const VEC_DB: u8 = 1; // debug (single-step / hardware breakpoint)
const VEC_BR: u8 = 5; // BOUND range exceeded
const VEC_UD: u8 = 6; // invalid opcode
const VEC_NP: u8 = 11; // segment not present
const VEC_SS: u8 = 12; // stack-segment fault
const VEC_GP: u8 = 13; // general protection
const VEC_PF: u8 = 14; // page fault
const VEC_AC: u8 = 17; // alignment check
const VEC_MC: u8 = 18; // machine check

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
        // queues, scheduler state, or any spin lock; it just counts and returns. It runs on a
        // dedicated NMI IST stack (U1b B3) so an NMI in the pre-swapgs syscall-entry window can't
        // push its frame onto the ring-3 stack — the IST switch is unconditional of CPL.
        unsafe {
            idt.non_maskable_interrupt
                .set_handler_fn(nmi_handler)
                .set_stack_index(gdt::NMI_IST_INDEX);
        }
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        // #DB (vector 1) on its OWN IST (U2 Part-0a). Without an entry here #DB was absent from the
        // IDT: ring 3 can set RFLAGS.TF (`popfq`) then SYSCALL, and the pending single-step trap
        // fires on the FIRST instruction at LSTAR — CPL 0, GS still user, RSP still the ring-3
        // stack. A missing gate escalates that to #NP (same-CPL, no IST) whose frame lands on the
        // user-writable stack → misread as a kernel fault → halt: a user-triggerable AP DoS. The
        // dedicated IST makes the CPU switch stacks unconditionally; the handler stays GS-free.
        unsafe {
            idt.debug
                .set_handler_fn(debug_handler)
                .set_stack_index(gdt::DB_IST_INDEX);
        }
        // #MC (vector 18) on its own IST (U2 Part-0a): always fatal, but on a guaranteed-valid
        // kernel stack so it logs and halts cleanly instead of triple-faulting from a bad window.
        unsafe {
            idt.machine_check
                .set_handler_fn(machine_check_handler)
                .set_stack_index(gdt::MC_IST_INDEX);
        }
        // Fault vectors that a ring-3 task can provoke. Each kills the offending task when the
        // fault was taken from CPL 3 and stays fatal (halts) from CPL 0 (a kernel bug); see the
        // handlers below. #PF and #GP are the U1b demo's live vectors; #UD/#SS/#NP/#DE/#BR/#AC are
        // wired so a ring-3 program can never escalate an unhandled vector into a triple fault.
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.stack_segment_fault.set_handler_fn(stack_segment_fault_handler);
        idt.segment_not_present.set_handler_fn(segment_not_present_handler);
        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.bound_range_exceeded.set_handler_fn(bound_range_exceeded_handler);
        idt.alignment_check.set_handler_fn(alignment_check_handler);
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

/// True iff this exception was taken from ring 3 — the interrupt frame's saved `CS.RPL == 3`.
/// Reading the frame needs no per-CPU state (no GS), so it is safe to consult BEFORE the swapgs
/// that a user-fault kill performs. A CPL-0 fault (a genuine kernel bug) returns false and stays
/// fatal — the U1b requirement that a kernel-context fault is never "killed" and hidden.
#[inline]
fn from_ring3(frame: &InterruptStackFrame) -> bool {
    frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3
}

/// U1b — the ring-3 fault-kill tail, shared by every user-provokable vector (the x86 mirror of
/// aarch64 `aarch64_el0_fault_handler`).
///
/// PRECONDITIONS: the fault was taken from CPL 3, and the CALLER HAS ALREADY EXECUTED `swapgs` so
/// GS again points at this CPU's `PerCpuData` (a ring-3 fault does NOT swapgs automatically, unlike
/// the SYSCALL stub). With GS restored, `this_cpu()` / the scheduler resolve normally. Runs in
/// interrupt context (IF=0, an interrupt gate) on the faulting task's own kernel stack (TSS.RSP0),
/// exactly the context `sys_exit` runs in — so `sched::exit()` is safe: it marks the task FINISHED
/// and switches to the scheduler, which frees the Box (the interrupt frame is abandoned with the
/// stack; nothing unwinds). Never returns.
///
/// Logging here is safe for the same reason it is on aarch64: the interrupted context is ring 3,
/// which holds no kernel lock; a `SERIAL_PORT`/console holder on another core holds it IRQ-masked
/// (bounded) so the worst case is a bounded spin, never a cycle. The demo runs in a BSP-quiet
/// window (`await_u1b_verdict`), so the console is uncontended and the kill line lands intact even
/// on the serial-less framebuffer console.
///
/// `cr2` is meaningful only for #PF (the faulting linear address); pass 0 for the other vectors.
unsafe fn ring3_fault_kill(vec: u8, err: u64, rip: u64, cr2: u64) -> ! {
    // A ring-3 fault must have a current task (ring 3 only runs as a dispatched user task). If
    // `current` is null it is a kernel bug on the user path, not a user fault — treat it as fatal
    // rather than silently "killing" nothing.
    let Some(name) = crate::arch::sched::current_name() else {
        serial_println!(
            ":: RING-3 FAULT from CPL3 with NO current task — vec={} err={:#x} rip={:#x} cr2={:#x} (kernel bug) ::",
            vec, err, rip, cr2
        );
        crate::hlt_loop();
    };
    serial_println!(
        ":: RING-3 FAULT: task '{}' KILLED — vec={} err={:#x} rip={:#x} cr2={:#x} ::",
        name, vec, err, rip, cr2
    );
    crate::arch::syscall::record_ring3_kill(name, vec, err, cr2);
    crate::arch::sched::exit() // never returns; switches to the scheduler on this task's kstack
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    // Read the faulting address BEFORE any swapgs (Cr2 is a plain control register — no GS needed).
    let cr2 = Cr2::read_raw();
    if from_ring3(&stack_frame) {
        // Ring-3 #PF: restore per-CPU GS, then kill the offending task (never returns).
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(
                VEC_PF,
                error_code.bits(),
                stack_frame.instruction_pointer.as_u64(),
                cr2,
            );
        }
    }
    // CPL-0 page fault: a kernel bug — fatal, unchanged.
    // FBCON-PACE F1: un-route the console BEFORE printing. These terminals never panic, so
    // neither of panic's two mirrors fires, and a routed console would HOLD every line after
    // the first (pace gate) on a machine about to hlt forever — fault name on the glass,
    // fault details lost. panic_screen() clears the route and arms the panic mirror, so the
    // diagnostics below reach the panel directly, compositor-free and lock-free.
    crate::video::fbcon::panic_screen();
    serial_println!("EXCEPTION: PAGE FAULT");
    serial_println!("Accessed Address: {:#x}", cr2);
    serial_println!("Error Code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    crate::hlt_loop();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    if from_ring3(&stack_frame) {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(VEC_GP, error_code, stack_frame.instruction_pointer.as_u64(), 0);
        }
    }
    crate::video::fbcon::panic_screen(); // FBCON-PACE F1 — see page_fault_handler.
    serial_println!("EXCEPTION: GENERAL PROTECTION FAULT");
    serial_println!("Error Code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    crate::hlt_loop();
}

/// Emit the CPL-0 (fatal) diagnostic for a non-#PF fault vector, then halt. Factored out so each
/// error-code / no-error-code handler below stays a two-line dispatcher.
fn fatal_fault(what: &str, vec: u8, err: u64, stack_frame: &InterruptStackFrame) -> ! {
    crate::video::fbcon::panic_screen(); // FBCON-PACE F1 — see page_fault_handler.
    serial_println!("EXCEPTION: {} (vec={}, err={:#x})", what, vec, err);
    serial_println!("{:#?}", stack_frame);
    crate::hlt_loop();
}

// --- Additional user-provokable fault vectors. Each: kill the task on a CPL-3 fault, stay fatal
// on a CPL-0 fault. Vectors WITH an error code (#SS/#NP/#AC) pass it through; vectors WITHOUT one
// (#UD/#DE/#BR) pass 0. None carry a CR2, so cr2 is always 0. ---

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    if from_ring3(&stack_frame) {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(VEC_UD, 0, stack_frame.instruction_pointer.as_u64(), 0);
        }
    }
    fatal_fault("INVALID OPCODE", VEC_UD, 0, &stack_frame);
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    if from_ring3(&stack_frame) {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(VEC_DE, 0, stack_frame.instruction_pointer.as_u64(), 0);
        }
    }
    fatal_fault("DIVIDE ERROR", VEC_DE, 0, &stack_frame);
}

extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: InterruptStackFrame) {
    if from_ring3(&stack_frame) {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(VEC_BR, 0, stack_frame.instruction_pointer.as_u64(), 0);
        }
    }
    fatal_fault("BOUND RANGE EXCEEDED", VEC_BR, 0, &stack_frame);
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    if from_ring3(&stack_frame) {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(VEC_SS, error_code, stack_frame.instruction_pointer.as_u64(), 0);
        }
    }
    fatal_fault("STACK-SEGMENT FAULT", VEC_SS, error_code, &stack_frame);
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    if from_ring3(&stack_frame) {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(VEC_NP, error_code, stack_frame.instruction_pointer.as_u64(), 0);
        }
    }
    fatal_fault("SEGMENT NOT PRESENT", VEC_NP, error_code, &stack_frame);
}

extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    if from_ring3(&stack_frame) {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(VEC_AC, error_code, stack_frame.instruction_pointer.as_u64(), 0);
        }
    }
    fatal_fault("ALIGNMENT CHECK", VEC_AC, error_code, &stack_frame);
}

/// U2 Part-0a: count of #DB traps that hit the SYSCALL-entry TF window and were neutralized by
/// clearing TF and resuming (rather than halting). Nonzero means the DoS path was actually
/// exercised — the honest evidence the ledger wants; on platforms whose SYSCALL clears TF before the
/// trap point (so no #DB is delivered) this stays 0 and the fixture simply exits normally.
pub static DB_TF_RESUMED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// #DB (debug, vector 1) handler on a dedicated IST (U2 Part-0a). GS-FREE body — it consults only
/// the interrupt frame until a ring-3 kill performs its own (CPL-3-correct) `swapgs`. Policy:
///   * frame `CS.RPL == 3` — a ring-3 program single-stepped itself: kill the task (a user fault).
///   * frame `CS.RPL == 0` AND RIP inside the `unaos_syscall_entry` stub — the TF-armed-`SYSCALL`
///     case: the pending single-step trap landed on the entry stub at CPL 0 with GS/RSP possibly
///     still the ring-3 ones. Clear TF in the saved RFLAGS and `iretq` to resume the syscall — no
///     GS access, no kill. (Long-mode `iretq` restores RSP/SS unconditionally, so control returns
///     to the stub on the correct stack.)
///   * any other CPL-0 #DB — a genuine kernel debug event we didn't arm: fatal, unchanged policy.
extern "x86-interrupt" fn debug_handler(mut stack_frame: InterruptStackFrame) {
    if from_ring3(&stack_frame) {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
            ring3_fault_kill(VEC_DB, 0, stack_frame.instruction_pointer.as_u64(), 0);
        }
    }
    if crate::arch::syscall::rip_in_entry_stub(stack_frame.instruction_pointer.as_u64()) {
        // Clear TF in the frame's RFLAGS so the resumed stub does not immediately re-trap, then
        // return: the compiler-emitted `iretq` restores the (mutated) frame. GS is never touched.
        unsafe {
            stack_frame.as_mut().update(|f| f.cpu_flags.remove(RFlags::TRAP_FLAG));
        }
        DB_TF_RESUMED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return;
    }
    fatal_fault("DEBUG (#DB)", VEC_DB, 0, &stack_frame);
}

/// #MC (machine check, vector 18) handler on a dedicated IST (U2 Part-0a). Always fatal. GS-free:
/// it only prints (no GS-relative access) and halts; the IST stack guarantees it never triple-faults
/// even if the check lands in the pre-`swapgs`/pre-stack-switch syscall-entry window.
extern "x86-interrupt" fn machine_check_handler(stack_frame: InterruptStackFrame) -> ! {
    // FBCON-PACE F1 (see page_fault_handler). GS-free contract preserved: panic_screen touches
    // only fbcon statics, no GS-relative access.
    crate::video::fbcon::panic_screen();
    serial_println!("EXCEPTION: MACHINE CHECK (#MC, vec={})", VEC_MC);
    serial_println!("{:#?}", stack_frame);
    crate::hlt_loop();
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

/// U3.5: count of timer IRQs taken while the interrupted context was ring 3 (CPL 3) — i.e. the timer
/// PREEMPTED a running preemptible ring-3 task. Cooperative ring 3 (RFLAGS.IF clear, the U1a/U1b/U2/
/// U2.5/U3 fixtures) never lets the timer fire, so this stays 0 for them; a nonzero value is the
/// preemptible-ring-3 proof. Metal-only truth — TCG may under-deliver, so the fixture gates on `> 0`,
/// not an exact count.
pub static IRQS_AT_RING3: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

extern "x86-interrupt" fn timer_interrupt_handler(stack_frame: InterruptStackFrame) {
    // U3.5: a timer that fires while ring 3 is running (a PREEMPTIBLE task — RFLAGS.IF set) enters
    // CPL 0 on TSS.RSP0 with GS still the USER gs. Conditionally `swapgs` so `note_tick()`/
    // `this_cpu()`/the scheduler resolve the kernel per-CPU block (exactly like the U1b ring-3 fault
    // handler), and swap back before the compiler-emitted `iretq`. `from_ring3` reads only the
    // interrupt frame (no GS), so it is safe to consult before the swap. A CPL-0 tick (kernel task /
    // scheduler idle / the cooperative demos, whose IF is clear so the timer never lands in their
    // ring-3 run) takes neither branch — that path is byte-identical to before U3.5.
    let from_user = from_ring3(&stack_frame);
    if from_user {
        unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
        IRQS_AT_RING3.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
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
    // U3.5: restore the user GS before the compiler-emitted `iretq` returns to ring 3. When the timer
    // preempted a ring-3 task, `timer_preempt` switched away here and this runs only at RESUME time —
    // when the task is re-dispatched, control returns from `timer_preempt` to exactly this point.
    // `from_user` lived on the (preserved) kernel stack across the switch. The swap is self-correcting:
    // it leaves `IA32_KERNEL_GS_BASE = &PerCpuData` for the next kernel entry regardless of the
    // intervening switches, because the live GS here is always this CPU's kernel per-CPU pointer (the
    // scheduler and CPU-pinned tasks never leave kernel GS while at CPL 0).
    if from_user {
        unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
    }
}

/// Inter-processor interrupt handler (IDT vector `IPI_VECTOR`). Lock-free and WAKE-ONLY: record
/// the IPI on this CPU and EOI. It deliberately does NOT context-switch — its whole job is that
/// returning from the interrupt breaks the scheduler's idle `hlt`, so the per-CPU scheduler loop
/// re-checks its run queue and picks up work a `spawn` just enqueued. Keeping it switch-free is
/// what makes the running task's `current` pointer single-owner (only the scheduler loop and the
/// timer preempt site ever switch).
extern "x86-interrupt" fn ipi_handler(stack_frame: InterruptStackFrame) {
    // U3.5: the reschedule IPI is a MASKABLE interrupt (interrupt gate), so once ring 3 is
    // preemptible (RFLAGS.IF set) it — exactly like the timer — can be delivered while a ring-3 task
    // runs, entering CPL 0 with GS still the user gs. `note_ipi()` is GS-relative (`this_cpu()`), so
    // conditionally `swapgs` on a CPL-3 entry and swap back before `iretq`, the same treatment the
    // timer handler gets (the brief's "the timer OR ANY IRQ" rule). A CPL-0 IPI — an idle or
    // kernel-mode target, which is every IPI the current build actually sends — takes neither branch,
    // so this stays byte-identical for them. Wake-only (no context switch), so the entry/exit swaps
    // balance within this one invocation; `eoi()`/`note_ipi` in between run under the kernel GS.
    let from_user = from_ring3(&stack_frame);
    if from_user {
        unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
    }
    crate::arch::percpu::note_ipi();
    crate::arch::apic::eoi();
    if from_user {
        unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
    }
}

/// Count of NMIs taken (lock-free introspection; see the NMI handler below).
pub static NMI_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// U2 Part-0c: set once an NMI is observed running on its dedicated NMI IST stack — i.e. the CPU
/// actually *switched stacks* on delivery, not merely that an NMI arrived. This is what upgrades the
/// B3 ledger claim from "IST slot installed" to "NMI taken on IST" (see `syscall::nmi_self_fire`).
pub static NMI_ON_IST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Non-maskable interrupt handler. NMIs ignore IF, so this can interrupt a context switch in
/// progress; it must stay leaf and lock-free (no run queues, no `current`, no spin locks). We
/// only count, witness the IST switch, and return — that keeps `switch_context` NMI-reentrant-safe.
extern "x86-interrupt" fn nmi_handler(_stack_frame: InterruptStackFrame) {
    NMI_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // Read the handler's own RSP and check it against the NMI IST bounds (GS-free — no per-CPU
    // lookup). If it falls inside a CPU's NMI IST stack, the unconditional IST switch happened.
    let rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
    }
    if crate::arch::gdt::rsp_in_nmi_ist(rsp) {
        NMI_ON_IST.store(true, core::sync::atomic::Ordering::Relaxed);
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
