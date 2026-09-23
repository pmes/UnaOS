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
/// offset. THREE numbers are RESERVED BY NAME and cannot move: the APIC timer's `TIMER_VECTOR`,
/// the reschedule IPI's `IPI_VECTOR` (read by `smp.rs` and `sched.rs` to build the ICR word), and
/// `SPURIOUS_VECTOR`, which IS the local APIC's SVR low byte. **Every other device vector is
/// ALLOCATED** — see `vectors` below. There is no `XHCI_MSI_VECTOR`, `NIC_MSI_VECTOR` or
/// `EHCI_MSI_VECTOR` const any more; a driver that wants a number ASKS for one
/// (`docs/dev/OS/01_BOOT_HAL/vectors.md`, rmbp-ledger B168).
pub const TIMER_VECTOR: u8 = 0x20;
/// Inter-processor interrupt vector (reschedule/wake; scheduler foundation). Reserved by name
/// BEFORE the first allocation runs, which is why the allocator's third answer is 0x43 and not
/// 0x42 — the EHCI's number, unchanged across this fold.
pub const IPI_VECTOR: u8 = 0x42;
pub const SPURIOUS_VECTOR: u8 = 0xFF;

/// VECTORS (rmbp-ledger B168) — the IDT vector allocator.
///
/// **THE DEFECT.** Until this module the x86 kernel numbered its interrupt vectors BY HAND: five
/// `pub const`s at the top of this file, each registered by name in the IDT below, each named
/// again by the driver that programmed its MSI (`pci.rs` for the NIC and the xHCI,
/// `drivers/ehci/mod.rs` for the EHCI) or — since IOAPIC, B147 — its redirection entry. The next
/// device that needed a vector (the HDA controller's completion, an AHCI port, a second EHCI or
/// xHCI function, the UVC isochronous pipe when CAMERA2 opens it) copied the pattern and picked
/// the next free number BY EYE. Two drivers that picked the same one would have collided IN
/// SILENCE: `idt[v].set_handler_fn` is a plain overwrite, so the second registration wins, the
/// first ISR stops running, and on the wire that is INDISTINGUISHABLE from a device that never
/// interrupted. rmbp-queue's KVBLANK row states the same defect from the other side —
/// *"`arch/x86_64/interrupts.rs` has three hard-coded IDT vectors and no allocator a PCI function
/// can join"*.
///
/// **THE WIRE DOES NOT CHANGE, and that is a requirement rather than a courtesy.** The three
/// device vectors this kernel already had are the allocator's FIRST THREE ANSWERS, in the order
/// the IDT seeds them: `xhci` 0x40, `nic` 0x41, `ehci` 0x43 (0x42 is `ipi`, reserved before the
/// first `alloc` runs, so the third answer skips it). Every capture recorded on this bench is
/// therefore byte-comparable across the fold, which is what lets a regression in the interrupt
/// path be seen at all. A later arc may renumber — it must say so and re-pin the captures.
///
/// **ALLOCATION ORDER IS PINNED AT IDT CONSTRUCTION, NOT AT PROBE TIME**, and that is
/// load-bearing: the drivers actually probe in the order EHCI (`pci::init` → `ehci::init`), xHCI,
/// NIC, so an allocator asked at each driver's own MSI site would have answered `ehci` 0x40 and
/// moved every number on the wire. The drivers ASK — `vectors::of("xhci")` — and the seeding
/// decides.
///
/// **THE RANGE.** The allocator's domain is `RANGE_LO..=RANGE_HI` (0x30–0xEF): above the 32 Intel
/// exception vectors and above the 0x20–0x2F band this kernel gives the APIC timer, below the
/// 0xF0–0xFF band where the SVR's spurious vector lives. `alloc` hands out from `FLOOR` (0x40)
/// upward and leaves 0x30–0x3F alone, for two reasons: the wire reason above, and because a
/// vector's PRIORITY on x86 is its number >> 4 (Intel SDM Vol. 3 §10.8.3) — the low band is where
/// a deliberately LOW-priority device belongs, not where the next arrival lands by accident.
pub mod vectors {
    use super::{IPI_VECTOR, SPURIOUS_VECTOR, TIMER_VECTOR};
    use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
    use spin::Mutex;
    use x86_64::structures::idt::{HandlerFunc, InterruptDescriptorTable};

    /// The allocator's domain, inclusive. Nothing outside it is ever chosen.
    pub const RANGE_LO: u8 = 0x30;
    pub const RANGE_HI: u8 = 0xEF;
    /// The lowest number `alloc` will hand out. See the module block for why it is not `RANGE_LO`.
    const FLOOR: u8 = 0x40;

    #[derive(Clone, Copy)]
    struct Slot {
        name: &'static str,
        /// `true` = RESERVED BY NAME (timer, IPI, spurious): a number this kernel cannot move. It
        /// is never handed out by `alloc` and is not counted in the census's `allocated=`.
        reserved: bool,
    }

    /// vector -> owner. One entry per IDT slot, not per allocatable slot, so the two reservations
    /// that sit OUTSIDE the range (0x20 and 0xFF) are recorded in the same table the census prints
    /// — one table a reader can check against the IDT, never two that must be added up.
    static TABLE: Mutex<[Option<Slot>; 256]> = Mutex::new([None; 256]);
    /// Successful `alloc` calls. `reserve` does not count: a reservation is a number this kernel
    /// was already born with, and folding the two would make `allocated=` unreadable.
    static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
    /// The LOADED IDT, published by `init_idt`. A runtime `alloc` writes its entry through this
    /// pointer, so a driver that probes late can join. Null until `init_idt` runs, and an `alloc`
    /// before that point is REFUSED on the wire rather than writing through it.
    static LIVE: AtomicPtr<InterruptDescriptorTable> = AtomicPtr::new(core::ptr::null_mut());

    /// Publish the loaded IDT. Called by `init_idt` on every CPU; the pointer is the same on all of
    /// them (one table, `lidt`-ed per CPU), so the repeat stores are idempotent.
    pub(super) fn publish(idt: *mut InterruptDescriptorTable) {
        LIVE.store(idt, Ordering::Release);
    }

    /// `(vector, reserved)` for `name`. The second field is what lets a refusal say WHICH mistake
    /// was made — asking twice for a device name, or asking for one of the three numbers this
    /// kernel cannot move — instead of one undifferentiated "taken".
    fn owner(t: &[Option<Slot>; 256], name: &str) -> Option<(u8, bool)> {
        t.iter().enumerate().find_map(|(i, s)| match s {
            Some(s) if s.name == name => Some((i as u8, s.reserved)),
            _ => None,
        })
    }

    fn holder(t: &[Option<Slot>; 256], name: &str) -> Option<u8> {
        owner(t, name).map(|(v, _)| v)
    }

    fn free_count(t: &[Option<Slot>; 256]) -> usize {
        (FLOOR..=RANGE_HI).filter(|&v| t[v as usize].is_none()).count()
    }

    /// Reserve `vector` for `name` — a number this kernel cannot move. The caller registers the
    /// handler itself (the three reserved entries are set in the IDT initializer exactly as they
    /// always were); this records the OWNERSHIP, so `alloc` can never hand the number to a device
    /// and so the census prints one table instead of two.
    pub(super) fn reserve(vector: u8, name: &'static str) {
        let mut t = TABLE.lock();
        let clash = t[vector as usize].map(|s| s.name).or_else(|| {
            holder(&t, name).map(|_| name)
        });
        if let Some(by) = clash {
            drop(t);
            serial_println!(
                "[vectors] reserve name={} vector={:#04x} -> REFUSED reason=already-owned by={} == witness ::",
                name, vector, by
            );
            return;
        }
        t[vector as usize] = Some(Slot { name, reserved: true });
    }

    /// The vector `name` holds, or `None` if this kernel allocated it none. **This is how a driver
    /// asks** — there is no const left to name. A `None` is not a diagnosis the caller may ignore:
    /// programming a device with a number nobody registered delivers an interrupt to whatever
    /// handler happens to sit at it, which is the collision this module exists to refuse.
    pub fn of(name: &str) -> Option<u8> {
        holder(&TABLE.lock(), name)
    }

    /// Allocate the lowest free vector at or above `FLOOR`, register `handler` at it in the LIVE
    /// IDT, and record `(vector, name)`. Every `None` is accompanied by a witness on the wire.
    ///
    /// **REFUSES a name that is already allocated or reserved**, and that refusal IS the defect
    /// this module closes: the hand-numbered kernel would have overwritten the IDT entry and one
    /// ISR would have stopped running, silently, with no line anywhere to say so.
    ///
    /// The handler is registered BEFORE the number is returned, so the caller cannot program a
    /// device with a vector whose entry is not yet live.
    pub fn alloc(name: &'static str, handler: HandlerFunc) -> Option<u8> {
        let idt = LIVE.load(Ordering::Acquire);
        if idt.is_null() {
            serial_println!(
                "[vectors] alloc name={} -> REFUSED reason=idt-not-loaded — `init_idt` has not published a table yet, so there is nowhere to register a handler == witness ::",
                name
            );
            return None;
        }
        // SAFETY: `LIVE` holds the one `'static` IDT, published by `init_idt` after `lidt`. Writing
        // a descriptor of a LOADED IDT is defined — the CPU reads the entry at DELIVERY — and the
        // entry written here is not deliverable until its caller programs a device with the number
        // returned below. `TABLE`'s mutex serialises the choice of slot, and each slot is written
        // exactly once, so there is one writer per descriptor.
        let v = alloc_in(unsafe { &mut *idt }, name, handler)?;
        serial_println!(
            "[vectors] alloc name={} vector={:#04x} == witness ::",
            name, v
        );
        census();
        Some(v)
    }

    /// The allocator proper. `alloc` is this against the LIVE IDT; the IDT initializer is this
    /// against the table it is still building, which is where the first three answers are decided.
    pub(super) fn alloc_in(
        idt: &mut InterruptDescriptorTable,
        name: &'static str,
        handler: HandlerFunc,
    ) -> Option<u8> {
        let mut t = TABLE.lock();
        if let Some((held, reserved)) = owner(&t, name) {
            drop(t);
            serial_println!(
                "[vectors] alloc name={} -> REFUSED reason=duplicate-name held={:#04x} kind={} — that name already owns a vector, and a second registration would OVERWRITE its IDT entry: one ISR would stop running and the wire would look exactly like a device that never interrupted == witness ::",
                name,
                held,
                if reserved { "reserved" } else { "allocated" }
            );
            return None;
        }
        let mut chosen = None;
        for v in FLOOR..=RANGE_HI {
            if t[v as usize].is_none() {
                t[v as usize] = Some(Slot { name, reserved: false });
                chosen = Some(v);
                break;
            }
        }
        drop(t);
        let Some(v) = chosen else {
            serial_println!(
                "[vectors] alloc name={} -> REFUSED reason=range-full range={:#04x}-{:#04x} floor={:#04x} — every vector this allocator may hand out is owned == witness ::",
                name, RANGE_LO, RANGE_HI, FLOOR
            );
            return None;
        };
        idt[v].set_handler_fn(handler);
        ALLOCATED.fetch_add(1, Ordering::Relaxed);
        Some(v)
    }

    /// ONE line, the whole table, sorted by vector:
    ///
    /// ```text
    /// [vectors] allocated=3 free=172 table=timer:0x20,xhci:0x40,nic:0x41,ipi:0x42,ehci:0x43,spurious:0xff == witness ::
    /// ```
    ///
    /// `allocated=` counts `alloc` successes; the three RESERVED names appear in `table=` but not
    /// in that count. `free=` is what `alloc` could STILL hand out — the unowned slots from `FLOOR`
    /// to `RANGE_HI` — so it answers "how many more devices fit" rather than restating the range.
    ///
    /// **WHERE IT IS PRINTED, and why not later.** Every vector this kernel hands out is allocated
    /// while the IDT is CONSTRUCTED (that is what pins the numbers), so the table is COMPLETE at
    /// `init_idt` and the line is UNCONDITIONAL — it reaches the wire of the default image that
    /// boots the metal whether or not a NIC, an xHCI or an EHCI was ever found, which a census
    /// printed from a driver's own arm site could not promise. A runtime `alloc` re-prints it, so
    /// the LAST `[vectors] allocated=` line of any capture is always the final table.
    ///
    /// Allocation-free (the heap does not exist yet at `init_idt`): the name list is formatted into
    /// a fixed stack buffer, and a table too long for it ends in `,…` rather than being silently
    /// short — `allocated=` and `free=` are exact either way.
    pub fn census() {
        struct Buf {
            b: [u8; 1024],
            n: usize,
            cut: bool,
        }
        impl core::fmt::Write for Buf {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                for &c in s.as_bytes() {
                    if self.n < self.b.len() {
                        self.b[self.n] = c;
                        self.n += 1;
                    } else {
                        self.cut = true;
                    }
                }
                Ok(())
            }
        }
        use core::fmt::Write;
        let t = TABLE.lock();
        let mut buf = Buf { b: [0; 1024], n: 0, cut: false };
        let mut first = true;
        for v in 0..=u8::MAX {
            if let Some(s) = t[v as usize] {
                let _ = write!(buf, "{}{}:{:#04x}", if first { "" } else { "," }, s.name, v);
                first = false;
            }
        }
        let free = free_count(&t);
        let allocated = ALLOCATED.load(Ordering::Relaxed);
        drop(t);
        // Every byte came from a `&str`, so the slice is UTF-8 by construction; the fallback exists
        // so a truncation that lands mid-codepoint prints a line instead of losing the census.
        let table = core::str::from_utf8(&buf.b[..buf.n]).unwrap_or("<non-utf8>");
        serial_println!(
            "[vectors] allocated={} free={} table={}{} == witness ::",
            allocated,
            free,
            table,
            if buf.cut { ",…" } else { "" }
        );
    }

    /// The three numbers this kernel cannot move, recorded before the first allocation. Split out
    /// of the IDT initializer only so the reservation list and the seeding order read as one block.
    pub(super) fn reserve_fixed() {
        reserve(TIMER_VECTOR, "timer");
        reserve(IPI_VECTOR, "ipi");
        reserve(SPURIOUS_VECTOR, "spurious");
    }
}

/// The IDT lives behind an `UnsafeCell` because `vectors::alloc` writes entries into it AFTER
/// `lidt` — that is exactly what makes the allocator joinable by a driver that probes late (the
/// HDA controller's completion, an AHCI port, the UVC isochronous pipe) rather than only by this
/// file. Writing a descriptor of a loaded IDT is defined: the CPU reads the entry at DELIVERY, and
/// the entry `alloc` writes is not deliverable until its caller programs the device with the
/// number `alloc` returned. Single-writer by construction — the BSP builds the table, `TABLE`'s
/// mutex serialises the choice of slot, and each slot is written exactly once.
struct IdtCell(core::cell::UnsafeCell<InterruptDescriptorTable>);
// SAFETY: see the block above. The cell is written only through `vectors`, never read except by
// the CPU's own descriptor fetch and by `lidt`.
unsafe impl Sync for IdtCell {}

lazy_static! {
    static ref IDT: IdtCell = {
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
        // All interrupts are delivered directly by the local APIC: the timer (heartbeat), the
        // allocated device vectors, the reschedule IPI, and the APIC spurious-interrupt vector.
        //
        // THE THREE RESERVED NUMBERS FIRST, and the order is load-bearing rather than tidy: with
        // `ipi` recorded before the first `alloc`, 0x42 is already owned when the EHCI's turn
        // comes, so the allocator's third answer is 0x43 — the number the deleted
        // `EHCI_MSI_VECTOR` const carried, and the number every capture on this bench was recorded
        // against. Their handlers are registered here, by name, exactly as they always were: a
        // reservation records ownership, it does not install anything.
        vectors::reserve_fixed();
        idt[TIMER_VECTOR].set_handler_fn(timer_interrupt_handler);
        idt[IPI_VECTOR].set_handler_fn(ipi_handler);
        idt[SPURIOUS_VECTOR].set_handler_fn(spurious_handler);
        // THE ALLOCATOR'S FIRST THREE ANSWERS — 0x40, 0x41, 0x43, in this order, which is why the
        // wire does not move. This is ALSO the only place the order can be pinned: the drivers
        // probe EHCI → xHCI → NIC (`pci::init`), so an allocator asked at each driver's own MSI
        // site would have answered `ehci` 0x40 and renumbered everything. Each driver now ASKS
        // (`vectors::of("xhci")`) instead of naming a const.
        let _ = vectors::alloc_in(&mut idt, "xhci", xhci_msi_handler);
        let _ = vectors::alloc_in(&mut idt, "nic", nic_msi_handler);
        // ISRARM: the EHCI HID completion vector. Gated on the same feature as the driver that
        // arms it, so a build without `ehcihid` carries neither the entry nor the handler — and
        // therefore cannot be handed an interrupt it has no driver to service. Knob-off the name
        // is simply absent from the table, and the census says so.
        #[cfg(feature = "ehcihid")]
        let _ = vectors::alloc_in(&mut idt, "ehci", ehci_msi_handler);
        // ONE census line per boot, here because here is where the table is COMPLETE and because
        // here it is UNCONDITIONAL — see `vectors::census`.
        vectors::census();
        IdtCell(core::cell::UnsafeCell::new(idt))
    };
}

pub fn init_idt() {
    // SAFETY: `IDT` is `'static` and never moves, so the table `lidt` points at outlives every
    // delivery. `load_unsafe` is the non-`'static` spelling of `load`; it is used because the
    // table now lives inside an `UnsafeCell` (see `IdtCell`) and executes exactly one `lidt`.
    unsafe { (*IDT.0.get()).load_unsafe() };
    // Publish the loaded table so a driver that probes later can join the allocator. Idempotent:
    // every AP calls `init_idt` and stores the same pointer.
    vectors::publish(IDT.0.get());
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
        // PFWIRE: this is the FATAL branch (a kernel bug on the user path) — it ends in `hlt_loop`
        // with IF=0, so drain the diagnostic to serial the same way the CPL-0 handlers do. The
        // RECOVERABLE kill below (task killed, kernel continues) is deliberately NOT armed: it must
        // leave the serial path on its normal Mutex. See page_fault_handler / review §5.
        crate::serial_ring::enter_panic_mode();
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
    // CPL-0 page fault: a kernel bug — fatal.
    // PFWIRE: this core is about to `hlt` forever with IF=0 (interrupt gate), so it can never be
    // the next `SERIAL1` holder to drain the staging ring. If the fault was taken while this core
    // held `SERIAL1` — anywhere inside a `serial_println!` emit region — the four diagnostic lines
    // below would be `try_stage`d into a ring nobody drains and lost: a brick with no serial line.
    // `enter_panic_mode()` takes the serial path off the Mutex onto `RawUart`'s lock-free
    // synchronous byte writes (the same escape hatch `#[panic_handler]` arms at main.rs), so the
    // faulting address reaches the wire regardless of who holds the lock. It is a single relaxed
    // atomic store — no lock, never cleared, idempotent — so it is safe from this context and
    // re-entrant (a fault inside the drain re-enters it harmlessly). See review-m3b-draft.md §5/C2.
    crate::serial_ring::enter_panic_mode();
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
    crate::serial_ring::enter_panic_mode(); // PFWIRE — drain the fault to serial; see page_fault_handler.
    crate::video::fbcon::panic_screen(); // FBCON-PACE F1 — see page_fault_handler.
    serial_println!("EXCEPTION: GENERAL PROTECTION FAULT");
    serial_println!("Error Code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    crate::hlt_loop();
}

/// Emit the CPL-0 (fatal) diagnostic for a non-#PF fault vector, then halt. Factored out so each
/// error-code / no-error-code handler below stays a two-line dispatcher.
fn fatal_fault(what: &str, vec: u8, err: u64, stack_frame: &InterruptStackFrame) -> ! {
    crate::serial_ring::enter_panic_mode(); // PFWIRE — drain the fault to serial; see page_fault_handler.
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
    // PFWIRE (see page_fault_handler). GS-free contract preserved: `enter_panic_mode` is a relaxed
    // store to a global AtomicBool (RIP-relative, no GS-relative access).
    crate::serial_ring::enter_panic_mode();
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
    // DEADMAN — the once-per-second witness that survives a wedged render-service pass.
    //
    // POSITION IS LOAD-BEARING, both ends. AFTER `eoi()`: the emit is the longest thing this handler
    // can do, and holding the in-service bit across it would block this core's own subsequent timer
    // ticks — the instrument would suppress the very heartbeat it rides. BEFORE `timer_preempt()`:
    // that call can switch away and not return for the descheduled lifetime of the task, so anything
    // after it is not on the timer's clock at all, which is the entire property this witness exists
    // to have.
    //
    // Steady-state cost on 999 of every 1000 ticks, on every core, is one relaxed load of the
    // per-CPU index, one relaxed load of the deadline and one compare. `tick()` returns immediately
    // on every core but the BSP (the only core that advances `APIC_TICKS`, hence the only one whose
    // clock reading is the wall clock). A no-op inline shim when the knob is off.
    crate::deadman::tick();
    // WCSER-ISR (boot 13, 2026-08-22) — DRIVE THE OVERDUE PROBE FROM HERE, BESIDE DEADMAN, BECAUSE
    // ITS OWN LOOP IS A WEDGE VICTIM.
    //
    // The probe's home was the head of `x86_input_service`'s loop, deliberately AHEAD of the event
    // pump — boot 8B had shown the pump blocking into a wedged GUI and taking the probe's 5 s
    // repeats with it. Boot 13 shows that was not far enough forward: the probe printed
    // `PASS OVERDUE holder=c1 ... win=8 phase=33 row=792` exactly ONCE, at the crossing, and never
    // again through 78 further seconds of hold, while `[wcser]`'s own rollup kept printing
    // `held_ms=` from a different path. The whole input-service loop is a gate victim, not just its
    // pump, so a probe anywhere inside it dies with the wedge it exists to report.
    //
    // This path demonstrably survives: DEADMAN emitted 111 consecutive lines through the same hold.
    // What that buys is the one field the single boot-13 sample could not give — whether `row=`
    // ADVANCES. A frozen row is one MMIO write that never returned; a crawling row is the same loop
    // running at microscopic speed. Those have different causes and the wire could not tell them
    // apart.
    //
    // Cost on 999 of every 1000 ticks is what `wcser_overdue_probe` already charges at its head:
    // BSP check, then one relaxed load of the holder and an early return while the gate is free.
    // `serial_println!` is safe from an IRQ-masked context here for exactly the reasons
    // `deadman::emit` documents above — `_print` is `try_lock`-only and cannot wait on anything.
    #[cfg(feature = "witness")]
    if crate::arch::percpu::this_cpu().cpu_index == 0 {
        crate::video::wm::wcser_overdue_probe();
    }
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
    // W5I2 — the NMI PROBE's record (tail block `w5nmi`). Keeps every property this handler has:
    // GS-free (the LAPIC id is read from the APIC, the frame from the IST stack), lock-free, no
    // print, no framebuffer. It compares its own LAPIC id against the armed target and writes
    // rip/cs into that core's slot; a LINT1 hardware NMI or a `nmi_self_fire` on any other core
    // matches nothing and costs one relaxed load. Cfg'd with the probe; the knob-off handler is
    // this function minus these two lines, and every `panic::Location` in this file is above.
    #[cfg(any(feature = "witness", feature = "bar1wedge"))]
    w5nmi::record(&_stack_frame);
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

/// EHCI HID completion handler (ISRARM, IDT vector 0x43). **This one does real work, and that is
/// the difference between it and the two handlers above** — the xHCI and NIC handlers exist only to
/// wake the CPU from `hlt` and leave the ring to the polled context. Here the polled context IS the
/// defect: an armed HID interrupt-IN endpoint holds exactly one report, so between the completion
/// and the next service pass the endpoint is dark and the trackpad's reports are lost on the wire —
/// up to 2390 of them in one metal boot, measured by EHCIDARK. Re-arming from the completion itself
/// is the only place that window can be closed, so `interrupt_ack` lifts the report out and re-arms.
///
/// It is still bounded and still lock-free: no allocation, no printing, no `EHCI_HID` (the pass
/// holds that `Mutex` across a `hw_wait_budget()` control transfer, so taking it here would
/// self-deadlock exactly as the xHCI handler's comment warns), at most `ISR_MAX_EPS` endpoints, and
/// a per-endpoint claim that is TRY-only in both directions and spins in neither. The full safety
/// argument, field by field, is the ISRARM block in `drivers/ehci/mod.rs`.
///
/// EOI last, mirroring the other MSI handlers: the driver half must complete before the in-service
/// bit is cleared, or a second completion could re-enter it on the same core mid-walk.
#[cfg(feature = "ehcihid")]
extern "x86-interrupt" fn ehci_msi_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::ehci::interrupt_ack();
    crate::arch::apic::eoi();
}

/// Local APIC spurious-interrupt handler (vector 0xFF, == APIC SVR low byte). By definition
/// the APIC did not actually deliver an interrupt here, so we must NOT send an EOI.
extern "x86-interrupt" fn spurious_handler(_stack_frame: InterruptStackFrame) {}

// =================================================================================================
// W5I2 — the NMI PROBE of a declared-dead render core (tail block).
// =================================================================================================
//
// THE QUESTION. After WCSER-STEAL takes the compositor gate from a core that has been inside one
// pass for `COMP_GATE_STEAL_MS`, that core is declared dead and never returns (`revenants=0` on
// all 96 rollups of flights 8 and 9; PCIE-RP-RECOVERY.md §12.1). Every register the tree can read
// is flat across the event, and W5SCOPE's branch (d) — the core is parked at retirement on a BAR1
// store that left its queue and was never acknowledged — is the one branch the capture is
// consistent with. No instrument has ever tested it. An NMI is the discriminator (§12.3 I2): a core
// hardware-parked on an in-flight store recognises no interrupt at any priority, so the NMI stays
// pending forever and the probe reports `taken=n`; a core spinning in software takes it at the
// next instruction boundary regardless of IF, and its handler can say WHERE it was.
//
// THE MECHANISM. `nmi_probe(core)` arms one slot, sends an NMI IPI to that core's LAPIC id through
// `apic::send_nmi_bounded` (the same `0x4400` ICR word `syscall::nmi_self_fire` uses, with a bounded
// delivery-status wait so the PROBING core can never be parked by the probe), and spins at most
// 10 ms on the slot's `TAKEN` counter. The handler side is `record`, called from `nmi_handler`:
// it stores rip and cs into the slot and bumps the counter — no lock, no print, no framebuffer,
// no GS. The caller then prints ONE line and returns the verdict to whoever asked.
//
// `in_blit` — WHAT IT MEASURES, HONESTLY. `FrameBuffer::blit` (`video/framebuffer.rs`) has no
// address range in this kernel: it is inlined into its span-flush caller (`nm` on the flight
// artifact shows no `blit` symbol), and its `copy_nonoverlapping` lowers to a CALL to
// compiler-builtins' `memcpy`, a 47-byte leaf (`rep movsb` / `rep movsq` / `rep movsb` / `ret`)
// which IS the instruction that stores into the BAR1 aperture. So the copy site's range is
// `memcpy`'s, and that is the one range this file can bound exactly: its start is the linker's
// answer to `memcpy as usize`, its end is the first `ret` (0xC3) after the entry — the function is
// a straight-line leaf with no 0xC3 byte in any operand (verified on the 891c4dec artifact by
// `objdump -d`; the bytes are quoted in PCIE-RP-RECOVERY.md §12.3). The kernel keeps no symbol
// table or unwind table in the loaded image (two FDEs in the whole `.eh_frame`), so there is no
// second source. The line prints the range it used beside the verdict so a reader with the
// artifact's `nm -S memcpy` can check the bound; `in_blit=?` is printed when no `ret` is found
// within the scan, never a guess. A rip inside the SPAN-FLUSH loop that calls the copy is
// `in_blit=n` by this definition: the copy has retired and the loop went round again, which is a
// software fault with a rip to name, not a parked store.
//
// WHAT QEMU CAN AND CANNOT PROVE. TCG delivers an NMI to a spinning core and to a halted core
// alike; nothing in QEMU can hold a store in flight forever. The self-test below therefore proves
// `taken=y` on a known spin loop with the rip inside it and `in_blit=n`, and `taken=y` on an idle
// (HLT) core as the negative control. `taken=n` on metal is the ONLY new fact this instrument can
// carry, and it is also the verdict the go-red mutation of `record` reproduces in QEMU (a false
// negative by construction), which is how the printing path of that verdict was exercised.
//
// CFG. Everything here is under `any(witness, bar1wedge)`: `witness` for the self-test's sake and
// because the x86 battery is the only place it runs in QEMU; `bar1wedge` because it is the knob
// the W5 instrument family flies under (PCIE-RP-RECOVERY.md §12.3 I1's call at the steal is cfg
// `bar1wedge`) and the seat's one-line `nmi_probe(dead)` at that post-steal hook must compile
// whichever of the two that hook carries. The steal machinery itself is ungated x86 code
// (`video/wm.rs`, the `COMP_GATE` block in `composite`); only its accounting is `witness`. With
// both knobs off nothing below is compiled, `nmi_handler` is its pre-W5I2 body, and no
// `panic::Location` in this file sits below the handler — measured, not argued: `./arroyo knoboff
// witness 891c4dec`.
#[cfg(any(feature = "witness", feature = "bar1wedge"))]
pub use w5nmi::{nmi_probe, NmiProbe};
#[cfg(feature = "witness")]
pub use w5nmi::selftest::once as nmi_probe_selftest_once;

#[cfg(any(feature = "witness", feature = "bar1wedge"))]
mod w5nmi {
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use crate::arch::gdt::MAX_CPUS;
    use x86_64::structures::idt::InterruptStackFrame;

    /// LAPIC id the caller is probing; `u32::MAX` = nothing armed. The handler records only when
    /// its own LAPIC id equals this word, so a hardware LINT1 NMI, a `nmi_self_fire`, or a probe
    /// NMI that a parked core finally takes seconds later (after the caller disarmed) writes nothing.
    static TARGET: AtomicU32 = AtomicU32::new(u32::MAX);
    /// Logical core index of the armed probe — the slot `record` writes. Stored BEFORE `TARGET`
    /// (Release) and read after it (Acquire), so a matching handler always sees the right slot.
    static SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
    /// One probe in flight at a time; a second caller is REFUSED on the wire, never queued.
    static BUSY: AtomicBool = AtomicBool::new(false);
    /// Per-core record: the interrupted rip, cs, and a counter that says the record is fresh.
    static RIP: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
    static CS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
    static TAKEN: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

    /// Bytes scanned past `memcpy`'s entry for its `ret`. The function is 47 bytes on 891c4dec;
    /// twice that is the cap, and a miss prints `in_blit=?` rather than extending the scan.
    const MEMCPY_SCAN: usize = 96;
    /// Bounded wait for the NMI to be taken. 10 ms is four orders of magnitude above the
    /// microseconds a spinning core needs, and short enough that the post-steal caller (a live
    /// core mid-composite) is not a second freeze.
    const WAIT_MS: u64 = 10;

    /// The handler half. Runs in NMI context on the PROBED core: three loads, two stores, one
    /// fetch_add; no lock, no print, no GS (`apic_id_u32` reads the LAPIC, the frame is on the IST
    /// stack). See the tail-block comment for why this is the whole of what an NMI may do here.
    #[inline]
    pub(super) fn record(frame: &InterruptStackFrame) {
        let target = TARGET.load(Ordering::Acquire);
        if target == u32::MAX || crate::arch::apic::apic_id_u32() != target {
            return;
        }
        let slot = SLOT.load(Ordering::Relaxed);
        if slot >= MAX_CPUS {
            return;
        }
        RIP[slot].store(frame.instruction_pointer.as_u64(), Ordering::Relaxed);
        CS[slot].store(frame.code_segment.0 as u64, Ordering::Relaxed);
        TAKEN[slot].fetch_add(1, Ordering::Release);
    }

    /// What one probe measured. `in_blit` is `None` when `taken` is false or when `memcpy`'s range
    /// could not be bounded (printed `?`).
    #[derive(Clone, Copy)]
    pub struct NmiProbe {
        pub taken: bool,
        pub rip: u64,
        pub cs: u64,
        pub in_blit: Option<bool>,
    }

    /// `[lo, hi)` of compiler-builtins' `memcpy` — the copy instruction `FrameBuffer::blit`'s
    /// `copy_nonoverlapping` calls — or `None` if no `ret` is found within `MEMCPY_SCAN` bytes.
    /// Read-only, volatile, kernel text: the bytes are mapped and executable on every core.
    pub(super) fn memcpy_range() -> Option<(usize, usize)> {
        unsafe extern "C" {
            fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
        }
        let lo = memcpy as *const () as usize;
        for i in 0..MEMCPY_SCAN {
            // SAFETY: kernel text at `memcpy`, within a bounded scan; a plain byte read.
            if unsafe { core::ptr::read_volatile((lo + i) as *const u8) } == 0xC3 {
                return Some((lo, lo + i + 1));
            }
        }
        None
    }

    /// `ms` milliseconds in `now_cycles()` units: the calibrated TSC when there is one, else the
    /// same 2.5e9-per-second guess `arch::HW_WAIT_BUDGET` carries.
    pub(super) fn tsc_budget(ms: u64) -> u64 {
        let hz = crate::arch::apic::tsc_hz();
        let per_ms = if hz != 0 { hz / 1000 } else { 2_500_000 };
        per_ms.saturating_mul(ms)
    }

    /// Spin until `cond()` or until `budget` cycles have elapsed; true iff `cond()` held.
    pub(super) fn spin_until(budget: u64, cond: impl Fn() -> bool) -> bool {
        let t0 = crate::arch::now_cycles();
        loop {
            if cond() {
                return true;
            }
            if crate::arch::now_cycles().wrapping_sub(t0) >= budget {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    /// `{:#x}..{:#x}` or `?` for the range field of the line.
    struct Range(Option<(usize, usize)>);
    impl core::fmt::Display for Range {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self.0 {
                Some((lo, hi)) => write!(f, "{:#x}..{:#x}", lo, hi),
                None => write!(f, "?"),
            }
        }
    }

    /// Probe `core` with an NMI and print ONE line:
    ///
    /// ```text
    /// :: W5: nmi core=c<n> taken=<y/n> rip=<hex|-> in_blit=<y/n/?> cs=<hex|-> memcpy=<lo>..<hi> icr=<ok|busy> ::
    /// ```
    ///
    /// `taken=n` after the 10 ms wait is the hardware-parked verdict (the NMI is left pending on
    /// that core; it is harmless and is recorded by nobody if the core ever resumes, because the
    /// slot is disarmed). `icr=busy` (xAPIC only) says the local APIC never reported the IPI
    /// delivered within the send's own bound — the probe still waited and reports what it saw.
    /// Returns `None` and prints `REFUSED why=<self|offline|no-such-core|busy>` when the question
    /// cannot be asked: a self-NMI is `nmi_self_fire`'s job, an offline slot has no LAPIC to
    /// address, and two probes at once would share one record.
    ///
    /// Caller context: any kernel task with the kernel GS (it reads `this_cpu()`), interrupts in any
    /// state; it allocates nothing and takes no lock. The wire is written with `serial_println!`,
    /// which is `try_lock`-only and cannot block.
    pub fn nmi_probe(core: usize) -> Option<NmiProbe> {
        let me = crate::arch::percpu::this_cpu().cpu_index as usize;
        let slot = crate::arch::percpu::cpu(core);
        // `init_cpu` writes `cpu_index = index`; a never-initialised slot reads 0, which only the
        // BSP (always online) legitimately carries. Lock-free and allocation-free, unlike
        // `smp::online_aps()`, because the post-steal caller sits inside a composite pass.
        let online = matches!(slot, Some(c) if c.cpu_index as usize == core);
        let why = if core >= MAX_CPUS {
            Some("no-such-core")
        } else if core == me {
            Some("self")
        } else if !online {
            Some("offline")
        } else {
            None
        };
        if let Some(why) = why {
            serial_println!(":: W5: nmi core=c{} REFUSED why={} ::", core, why);
            return None;
        }
        if BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            serial_println!(":: W5: nmi core=c{} REFUSED why=busy ::", core);
            return None;
        }
        let apic = slot.map(|c| c.apic_id).unwrap_or(u32::MAX);
        let before = TAKEN[core].load(Ordering::Acquire);
        SLOT.store(core, Ordering::Relaxed);
        TARGET.store(apic, Ordering::Release);
        let icr_ok = crate::arch::apic::send_nmi_bounded(apic);
        let taken = spin_until(tsc_budget(WAIT_MS), || {
            TAKEN[core].load(Ordering::Acquire) != before
        });
        TARGET.store(u32::MAX, Ordering::Release);
        let (rip, cs) = if taken {
            (RIP[core].load(Ordering::Relaxed), CS[core].load(Ordering::Relaxed))
        } else {
            (0, 0)
        };
        let range = memcpy_range();
        let in_blit = match (taken, range) {
            (true, Some((lo, hi))) => Some((rip as usize) >= lo && (rip as usize) < hi),
            _ => None,
        };
        let yn = |b: Option<bool>| match b {
            Some(true) => "y",
            Some(false) => "n",
            None => "?",
        };
        let icr = if icr_ok { "ok" } else { "busy" };
        if taken {
            serial_println!(
                ":: W5: nmi core=c{} taken=y rip={:#x} in_blit={} cs={:#x} memcpy={} icr={} ::",
                core, rip, yn(in_blit), cs, Range(range), icr
            );
        } else {
            serial_println!(
                ":: W5: nmi core=c{} taken=n rip=- in_blit=? cs=- memcpy={} icr={} ::",
                core, Range(range), icr
            );
        }
        BUSY.store(false, Ordering::Release);
        Some(NmiProbe { taken, rip, cs, in_blit })
    }

    /// The x86 witness self-test (W5I2 job 2): prove the probe on a known spin loop, then on an
    /// idle core, once per boot, from the tail of `sched::emit_load_witness` — the existing
    /// witness ladder site whose first call is the BSP's `-prejoin` line, when every AP has been
    /// dispatching for a while and no render service yet holds a worker.
    #[cfg(feature = "witness")]
    pub mod selftest {
        use super::{spin_until, tsc_budget};
        use core::sync::atomic::{AtomicBool, Ordering};

        static ONCE: AtomicBool = AtomicBool::new(false);
        /// Set by the spin loop itself, from INSIDE its address range, so a probe that observes it
        /// cannot land on an instruction outside `[unaos_nmi_spin, unaos_nmi_spin_end)`.
        static PARKED: AtomicBool = AtomicBool::new(false);
        /// The loop's exit condition; the byte the asm polls.
        static RELEASE: AtomicBool = AtomicBool::new(false);
        /// The task left the loop and is about to exit.
        static DONE: AtomicBool = AtomicBool::new(false);

        // The known loop, in asm so its range is two linker symbols and not a compiler's guess —
        // the same shape `syscall.rs` gives `unaos_syscall_entry`/`unaos_syscall_entry_end`.
        // `rdi` = &RELEASE (polled), `rsi` = &PARKED (set first, inside the range). Leaf, no stack.
        core::arch::global_asm!(
            ".globl unaos_nmi_spin",
            ".globl unaos_nmi_spin_end",
            "unaos_nmi_spin:",
            "    mov byte ptr [rsi], 1",
            "2:  pause",
            "    cmp byte ptr [rdi], 0",
            "    je 2b",
            "    ret",
            "unaos_nmi_spin_end:",
        );
        unsafe extern "C" {
            fn unaos_nmi_spin(release: *const u8, parked: *mut u8);
            static unaos_nmi_spin_end: u8;
        }

        /// The parked task: enters the loop with IF=0 — WEDGEINJ's shape, and the reason an NMI is
        /// the probe at all — and leaves only when `RELEASE` is set.
        fn spin_task(_: usize) {
            crate::arch::without_interrupts(|| unsafe {
                unaos_nmi_spin(RELEASE.as_ptr() as *const u8, PARKED.as_ptr() as *mut u8)
            });
            DONE.store(true, Ordering::Release);
        }

        fn spin_range() -> (usize, usize) {
            (unaos_nmi_spin as *const () as usize, &raw const unaos_nmi_spin_end as usize)
        }

        /// One-shot. Lines, in order (the `:: W5: nmi core=…` lines are `nmi_probe`'s own):
        ///
        /// ```text
        /// :: W5: nmi core=c<t> taken=y rip=<hex> in_blit=n cs=0x8 memcpy=<lo>..<hi> icr=ok ::
        /// :: W5: nmi selftest spin core=c<t> taken=y rip_in_spin=y in_blit=n spin=<lo>..<hi> released=y -> PASS ::
        /// :: W5: nmi core=c<t> taken=y rip=<hex> in_blit=n cs=0x8 memcpy=<lo>..<hi> icr=ok ::
        /// :: W5: nmi selftest idle core=c<t> taken=y rip_in_spin=n -> PASS (NMI wakes HLT; QEMU cannot produce taken=n, only metal can) ::
        /// ```
        ///
        /// `SKIPPED why=…` (no worker core, or the spin task not dispatched within 200 ms) is
        /// neither PASS nor FAIL: the fixture did not run, and the line says so.
        pub fn once() {
            if ONCE.swap(true, Ordering::AcqRel) {
                return;
            }
            let me = crate::arch::percpu::this_cpu().cpu_index as usize;
            let pool = crate::arch::smp::worker_pool_len();
            // The LAST worker: the least likely to be carrying a pinned fixture at this moment.
            let target = (0..pool)
                .rev()
                .filter_map(crate::arch::smp::worker_cpu)
                .find(|&c| c != me);
            let Some(target) = target else {
                serial_println!(
                    ":: W5: nmi selftest SKIPPED why=no-worker-core pool={} me=c{} ::",
                    pool, me
                );
                return;
            };
            crate::arch::sched::spawn("nmi-spin", spin_task, 0, target, crate::arch::sched::PRIO_RT);
            if !spin_until(tsc_budget(200), || PARKED.load(Ordering::Acquire)) {
                RELEASE.store(true, Ordering::Release);
                serial_println!(
                    ":: W5: nmi selftest SKIPPED why=spin-not-parked core=c{} within 200 ms ::",
                    target
                );
                return;
            }
            let (lo, hi) = spin_range();
            let in_spin = |r: Option<super::NmiProbe>| {
                r.is_some_and(|p| p.taken && (p.rip as usize) >= lo && (p.rip as usize) < hi)
            };
            let yn = |b: bool| if b { "y" } else { "n" };
            // Positive: the parked core takes the NMI inside the loop, and the loop is not the copy.
            let r1 = super::nmi_probe(target);
            RELEASE.store(true, Ordering::Release);
            let released = spin_until(tsc_budget(200), || DONE.load(Ordering::Acquire));
            let taken1 = r1.is_some_and(|p| p.taken);
            let blit1 = r1.and_then(|p| p.in_blit);
            let pass1 = taken1 && in_spin(r1) && blit1 == Some(false) && released;
            serial_println!(
                ":: W5: nmi selftest spin core=c{} taken={} rip_in_spin={} in_blit={} spin={:#x}..{:#x} released={} -> {} ::",
                target,
                yn(taken1),
                yn(in_spin(r1)),
                match blit1 { Some(true) => "y", Some(false) => "n", None => "?" },
                lo,
                hi,
                yn(released),
                if pass1 { "PASS" } else { "FAIL" }
            );
            // Negative control: the same core, now idle in the scheduler's `sti; hlt`, still takes
            // it — an NMI wakes HLT. What QEMU cannot show is `taken=n`: TCG holds no store in
            // flight, so that verdict exists only on metal.
            spin_until(tsc_budget(5), || false);
            let r2 = super::nmi_probe(target);
            let taken2 = r2.is_some_and(|p| p.taken);
            let pass2 = taken2 && !in_spin(r2);
            serial_println!(
                ":: W5: nmi selftest idle core=c{} taken={} rip_in_spin={} -> {} (NMI wakes HLT; QEMU cannot produce taken=n, only metal can) ::",
                target,
                yn(taken2),
                yn(in_spin(r2)),
                if pass2 { "PASS" } else { "FAIL" }
            );
        }
    }
}
