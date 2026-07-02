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
//
// Per-CPU GDT / TSS.
//
// Under SMP every CPU needs its OWN GDT, TSS and IST stack: the TSS descriptor in a GDT
// encodes that one CPU's TSS base, the `ltr` that loads it sets a busy bit in *that* GDT's
// descriptor, and the IST entry hands out a stack the CPU jumps to on a double fault. A single
// shared GDT/TSS would corrupt the moment a second CPU loaded it (busy-bit fault) or took a
// fault (two CPUs scribbling one IST stack). So the control blocks are per-CPU, indexed by a
// logical CPU number (the BSP is 0; APs get their index at discovery).
//
// They live in static (`.bss`) storage rather than the heap for two reasons: stable addresses
// are mandatory (the GDTR and the TSS descriptor hold raw linear pointers that must stay valid
// for as long as the CPU runs), and `init()` runs on the BSP *before* the heap is initialised.

use x86_64::registers::segmentation::SegmentSelector;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
/// IST slot for the NMI vector (U1b B3). An NMI ignores IF and can land in the pre-`swapgs`,
/// pre-stack-switch window of `unaos_syscall_entry` — where rsp still points at the ring-3
/// (user-writable) stack. Giving the NMI its own IST stack makes the CPU switch to a dedicated
/// kernel stack on every NMI regardless of CPL, so its frame can never be pushed onto (or read
/// from) the user stack. The handler stays GS-free (see `interrupts::nmi_handler`), so it needs
/// nothing but a valid stack — which the IST guarantees.
pub const NMI_IST_INDEX: u16 = 1;
/// Number of per-CPU IST stacks we reserve: [0] double fault, [1] NMI.
const NUM_IST: usize = 2;

// Fixed selectors for the per-CPU GDT layout built by `init_cpu` (identical on every CPU). The
// APPEND ORDER below is load-bearing for SYSCALL/SYSRET: SYSCALL loads CS = STAR[47:32] and
// SS = that + 8, SYSRET64 loads CS = STAR[63:48] + 16 and SS = that + 8. With STAR[47:32]=0x08
// and STAR[63:48]=0x13 (see `syscall::init`) the segments resolve to the four selectors here.
/// Kernel code (idx 1) — SYSCALL target CS.
pub const KERNEL_CODE_SEL: u16 = 0x08;
/// Kernel data (idx 2) — SYSCALL loads this as SS.
pub const KERNEL_DATA_SEL: u16 = 0x10;
/// User data (idx 3, DPL 3) — SYSRET loads (0x18 | RPL 3) as SS; also the `iretq` first-entry SS.
pub const USER_DATA_SEL: u16 = 0x1B;
/// User code (idx 4, DPL 3) — SYSRET loads (0x20 | RPL 3) as CS; also the `iretq` first-entry CS.
pub const USER_CODE_SEL: u16 = 0x23;

/// Upper bound on CPUs we reserve per-CPU control blocks for. QEMU `-smp` and the target
/// laptops sit well under this; bump it (and re-check the `.bss` cost below) if a machine has
/// more cores.
pub const MAX_CPUS: usize = 8;

/// Per-CPU double-fault IST stack size. The handler only prints and panics, but `{:#?}`
/// formatting of the stack frame wants some headroom, so give it 8 KiB.
const IST_STACK_SIZE: usize = 4096 * 2;

/// One CPU's segmentation control block. Kept small (the IST stacks live in a separate static
/// so this array stays compact). `code_selector`/`tss_selector` are retained for the upcoming
/// AP / userspace work even though nothing reads them yet.
struct PerCpu {
    gdt: GlobalDescriptorTable,
    tss: TaskStateSegment,
    #[allow(dead_code)]
    code_selector: SegmentSelector,
    #[allow(dead_code)]
    tss_selector: SegmentSelector,
}

impl PerCpu {
    const fn new() -> Self {
        PerCpu {
            gdt: GlobalDescriptorTable::new(),
            tss: TaskStateSegment::new(),
            code_selector: SegmentSelector::NULL,
            tss_selector: SegmentSelector::NULL,
        }
    }
}

/// Per-CPU control blocks. Addresses must be stable (GDTR / TSS descriptor hold raw pointers),
/// which static storage guarantees.
static mut CPUS: [PerCpu; MAX_CPUS] = [const { PerCpu::new() }; MAX_CPUS];

/// Per-CPU IST stacks: `NUM_IST` per CPU (kept out of `PerCpu` so that struct stays small).
/// Index 0 = double fault, index 1 = NMI (see `DOUBLE_FAULT_IST_INDEX` / `NMI_IST_INDEX`).
/// 8 KiB × NUM_IST × MAX_CPUS in `.bss`.
static mut IST_STACKS: [[[u8; IST_STACK_SIZE]; NUM_IST]; MAX_CPUS] =
    [[[0; IST_STACK_SIZE]; NUM_IST]; MAX_CPUS];

/// Build and load this CPU's GDT + TSS. `cpu_index` selects the per-CPU block: the BSP passes
/// 0; each AP passes the logical index it was assigned at discovery. Runs once per CPU, early
/// on that CPU (BSP: before the heap exists; AP: in `ap_main`).
///
/// Steps: wire the double-fault IST entry to this CPU's dedicated IST stack, build the GDT
/// (kernel code segment + a TSS descriptor for *this* TSS), `lgdt` it, reload CS, `ltr` the
/// TSS, then null the data segment registers so the CPU stops re-validating the stale
/// bootloader segment selectors against our fresh GDT (which otherwise #GPs).
pub fn init_cpu(cpu_index: usize) {
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, FS, GS, SS};
    use x86_64::instructions::tables::load_tss;

    assert!(cpu_index < MAX_CPUS, "gdt::init_cpu: cpu_index out of range");

    unsafe {
        // Raw pointers into the static per-CPU storage. We never form a `&`/`&mut` to the
        // `static mut` itself (edition 2024 forbids that); the addresses are stable for the
        // CPU's lifetime, which is exactly what `lgdt`/`ltr` require.
        let cpu = &raw mut CPUS[cpu_index];

        // 1. Point this CPU's IST entries at its own dedicated IST stacks (top = base + size).
        //    Slot 0 = double fault, slot 1 = NMI — separate stacks so an NMI taken while the
        //    double-fault handler runs (or vice versa) cannot scribble the other's frame.
        let df_stack = &raw const IST_STACKS[cpu_index][DOUBLE_FAULT_IST_INDEX as usize];
        let nmi_stack = &raw const IST_STACKS[cpu_index][NMI_IST_INDEX as usize];
        (*cpu).tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            VirtAddr::from_ptr(df_stack) + IST_STACK_SIZE as u64;
        (*cpu).tss.interrupt_stack_table[NMI_IST_INDEX as usize] =
            VirtAddr::from_ptr(nmi_stack) + IST_STACK_SIZE as u64;

        // 2. Build the GDT in the order the SYSCALL/SYSRET selector consts above require:
        //    kernel code (0x08), kernel data (0x10), user data (0x18, DPL3), user code (0x20, DPL3),
        //    then a TSS descriptor (two slots, 0x28) for THIS CPU's TSS. `tss_segment_unchecked`
        //    only reads the pointer's value (it encodes the base address); it does not dereference,
        //    so aliasing the `&raw mut cpu` is fine.
        let tss_ptr: *const TaskStateSegment = &raw const (*cpu).tss;
        let code_selector = (*cpu).gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = (*cpu).gdt.append(Descriptor::kernel_data_segment());
        let user_data = (*cpu).gdt.append(Descriptor::user_data_segment());
        let user_code = (*cpu).gdt.append(Descriptor::user_code_segment());
        let tss_selector = (*cpu).gdt.append(Descriptor::tss_segment_unchecked(tss_ptr));
        // Lock the layout: the naked SYSCALL/SYSRET paths hard-code these selector values.
        assert_eq!(code_selector.0, KERNEL_CODE_SEL, "gdt: kernel code selector moved");
        assert_eq!(kernel_data.0, KERNEL_DATA_SEL, "gdt: kernel data selector moved");
        assert_eq!(user_data.0 | 3, USER_DATA_SEL, "gdt: user data selector moved");
        assert_eq!(user_code.0 | 3, USER_CODE_SEL, "gdt: user code selector moved");
        (*cpu).code_selector = code_selector;
        (*cpu).tss_selector = tss_selector;

        // 3. Load the GDT. `load_unsafe` (vs `load`) lifts the 'static bound — the table lives
        //    in the static `CPUS` array for the CPU's whole life and is never mutated again.
        (*cpu).gdt.load_unsafe();

        // 4. Reload CS with the new kernel code selector and load the task register.
        CS::set_reg(code_selector);
        load_tss(tss_selector);

        // 5. Sanitise the data segments to the null selector.
        let null = SegmentSelector::NULL;
        SS::set_reg(null);
        DS::set_reg(null);
        ES::set_reg(null);
        FS::set_reg(null);
        GS::set_reg(null);
    }
}

/// BSP entry point, called from `arch::init` before the heap exists. The BSP is logical CPU 0.
pub fn init() {
    init_cpu(0);
}

/// U1a: set `cpu_index`'s TSS.RSP0 — the kernel stack the CPU switches to when an interrupt or
/// exception is taken FROM ring 3 (a CPL 3→0 transition). Written by `user_task_trampoline` before
/// it drops a task to ring 3, so a fault in that task lands on the task's own kernel stack instead
/// of triple-faulting. (SYSCALL does not consult RSP0 — it uses the per-CPU `syscall_kernel_rsp`
/// anchor instead — so this only covers the fault/interrupt path.) The IST double-fault entry is
/// untouched. Writing the loaded TSS's RSP0 field is a plain memory store the CPU re-reads on the
/// next privilege-change; no `ltr` reload is needed.
pub fn set_privilege_stack0(cpu_index: usize, rsp: u64) {
    assert!(cpu_index < MAX_CPUS, "gdt::set_privilege_stack0: cpu_index out of range");
    unsafe {
        let cpu = &raw mut CPUS[cpu_index];
        (*cpu).tss.privilege_stack_table[0] = VirtAddr::new(rsp);
    }
}
