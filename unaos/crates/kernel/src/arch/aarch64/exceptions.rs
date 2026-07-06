// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AArch64 exception vectors — the ARM analogue of the x86_64 IDT.
//
// AArch64 routes every exception (synchronous fault, IRQ, FIQ, SError) through a single 2 KiB-aligned
// vector table of 16 entries, each 0x80 bytes, addressed by VBAR_ELx. The 16 = 4 exception kinds
// (Sync, IRQ, FIQ, SError) x 4 sources (Current EL with SP0; Current EL with SPx; Lower EL AArch64;
// Lower EL AArch32). We run at the EL UEFI handed us off at (EL2 on QEMU `virt,virtualization=on`,
// per the boot diagnostic), using SPx (SPSel=1), so timer IRQs arrive on the "Current EL with SPx,
// IRQ" entry at offset 0x280. We wire every entry anyway: IRQ entries -> the IRQ stub; everything
// else -> a fault stub that logs ESR/ELR/FAR and halts (the equivalent of the x86 fault handlers).
//
// The IRQ stub saves the full general-purpose register file + FP/SIMD, banks ELR_ELx/SPSR_ELx and
// (M6e) SP_EL0 onto the interrupted task's own kernel stack, calls the Rust handler, restores all of
// it, and `eret`s. Banking ELR/SPSR/SP_EL0 (rather than trusting the CPU's entry-banked copies to
// survive) is what makes preemption sound: the scheduler may context-switch INSIDE the handler, so
// the task resumed in our place takes its own IRQs and overwrites those per-core sysregs — see the
// __vec_irq comment. SP_EL0 is the user stack pointer, live only when the interrupted context was EL0
// (a preemptible EL0 task); banking it lets a timer preempt of EL0 resume with the right user SP. The
// timer/GIC work the handler does is all MMIO + EL0-accessible system registers, so the IRQ path
// itself is EL-agnostic; only installing VBAR and decoding a fault differ by EL.

use core::sync::atomic::{AtomicU8, Ordering};

// The Exception Level we were booted at (2 or 1), latched in `install`. The IRQ/timer hot path is
// EL-uniform, but VBAR install and fault decoding need the right banked-register names.
static BOOT_EL: AtomicU8 = AtomicU8::new(0);

// The IRQ stub banks ELR/SPSR for the EL we run at. Baremetal drops to EL1 (see `boot::drop_to_el1`);
// the UEFI/QEMU-virt build stays at EL2. A single `global_asm!` can't `#[cfg]` individual lines, so
// select the "1"/"2" EL suffix here and splice it into __vec_irq below via `concat!`.
#[cfg(feature = "baremetal")]
macro_rules! irq_el {
    () => {
        "1"
    };
}
#[cfg(not(feature = "baremetal"))]
macro_rules! irq_el {
    () => {
        "2"
    };
}

// M6a: the Lower-EL-AArch64 synchronous vector (0x400) routes to __vec_svc on baremetal (EL0 syscalls
// land there since the kernel is at EL1), but stays the halting __vec_sync on the UEFI/EL2 build (no
// EL0 there). svc_stub!() emits the __vec_svc body only on baremetal (empty otherwise), so the UEFI
// build has no reference to the baremetal-only aarch64_svc_handler.
#[cfg(feature = "baremetal")]
macro_rules! svc_vec {
    () => {
        "__vec_svc"
    };
}
#[cfg(not(feature = "baremetal"))]
macro_rules! svc_vec {
    () => {
        "__vec_sync"
    };
}
#[cfg(feature = "baremetal")]
macro_rules! svc_stub {
    () => {
        r#"
    // ---- SVC (EL0 syscall): save, decode EC, dispatch in Rust on the task's kernel stack, return ----
    // Reached from VBAR_EL1 + 0x400 (Lower EL AArch64, Synchronous): the kernel is at EL1, so an EL0
    // `svc #0` lands here. Bank ELR_EL1/SPSR_EL1/SP_EL0 (per-core, and a blocking syscall would clobber
    // them), then check ESR_EL1.EC == 0x15 (SVC/AArch64). Anything else at this vector (EL0 data/instr
    // abort 0x24/0x20/0x21, alignment 0x22/0x26, unknown 0x00) is routed to the fault dead-end BEFORE
    // x8 is read, so a buggy EL0 program can never wild-dispatch on a garbage syscall number.
    .globl __vec_svc
__vec_svc:
    SAVE_GPRS
    SAVE_FP
    mrs x0, ELR_EL1
    mrs x1, SPSR_EL1
    mrs x2, SP_EL0
    stp x0, x1, [sp, #-32]!
    str x2, [sp, #16]
    mrs x0, ESR_EL1
    lsr x0, x0, #26
    cmp x0, #0x15
    b.ne __vec_svc_fault
    add x0, sp, #560            // -> the SAVE_GPRS base (256 GPR + 528 FP + 32 banked sit below it)
    bl aarch64_svc_handler
    ldp x0, x1, [sp]
    ldr x2, [sp, #16]
    add sp, sp, #32
    msr ELR_EL1, x0
    msr SPSR_EL1, x1
    msr SP_EL0, x2
    RESTORE_FP
    RESTORE_GPRS
    eret
    // Non-SVC lower-EL synchronous exception (M6b): an EL0 abort/alignment/UNDEF/trapped-sysreg
    // KILLS the offending task, not the kernel — aarch64_el0_fault_handler logs, records, and
    // sched::exit()s (never returns; the frame is abandoned with the task's stack). The `b .`
    // guarantees we never fall into the eret tail with a half-restored frame.
__vec_svc_fault:
    bl aarch64_el0_fault_handler
    b .
"#
    };
}
#[cfg(not(feature = "baremetal"))]
macro_rules! svc_stub {
    () => {
        ""
    };
}

core::arch::global_asm!(concat!(
    r#"
    // ---- The vector table: 16 entries x 0x80 bytes, 2 KiB aligned (VBAR requirement) ----
    .section .text.exception_vectors, "ax", %progbits
    .balign 0x800
    .globl __exception_vectors
__exception_vectors:
    // Current EL with SP0
    .balign 0x80
    b __vec_sync          // 0x000 Synchronous
    .balign 0x80
    b __vec_irq           // 0x080 IRQ
    .balign 0x80
    b __vec_fiq           // 0x100 FIQ
    .balign 0x80
    b __vec_serror        // 0x180 SError
    // Current EL with SPx  (this is where our timer IRQ lands: 0x280)
    .balign 0x80
    b __vec_sync          // 0x200 Synchronous
    .balign 0x80
    b __vec_irq           // 0x280 IRQ
    .balign 0x80
    b __vec_fiq           // 0x300 FIQ
    .balign 0x80
    b __vec_serror        // 0x380 SError
    // Lower EL using AArch64 (baremetal: EL0 tasks — 0x400 SVC -> __vec_svc; UEFI/EL2: fault logger)
    .balign 0x80
    b "#, svc_vec!(), r#"          // 0x400 Synchronous (EL0 SVC on baremetal)
    .balign 0x80
    b __vec_irq           // 0x480 IRQ
    .balign 0x80
    b __vec_fiq           // 0x500 FIQ
    .balign 0x80
    b __vec_serror        // 0x580 SError
    // Lower EL using AArch32
    .balign 0x80
    b __vec_sync          // 0x600 Synchronous
    .balign 0x80
    b __vec_irq           // 0x680 IRQ
    .balign 0x80
    b __vec_fiq           // 0x700 FIQ
    .balign 0x80
    b __vec_serror        // 0x780 SError

    // ---- Save/restore macros: the full GP register file (x0-x30), 256-byte 16-aligned frame ----
    // SAVE_GPRS saves GPRs ONLY. The IRQ and SVC stubs (__vec_irq/__vec_svc) pair it with SAVE_FP
    // (below), so they DO preserve the full v0-v31/FPSR/FPCR — mandatory since the scheduler's
    // preemption runs a whole other task (which clobbers the vector file) between save and eret.
    // The "FP is intentionally NOT saved" caveat therefore applies ONLY to the FAULT stubs
    // (__vec_sync/__vec_fiq/__vec_serror), which use SAVE_GPRS ALONE: each is a dead-end that logs
    // ESR/ELR/FAR and halts (aarch64_fault_handler is typed `-> !`), so it never resumes the
    // interrupted context and never needs its FP state. If a fault stub is ever made to RETURN, it
    // must add SAVE_FP/RESTORE_FP first, or it will silently clobber the interrupted thread's NEON.
    .macro SAVE_GPRS
    stp x0,  x1,  [sp, #-256]!
    stp x2,  x3,  [sp, #16]
    stp x4,  x5,  [sp, #32]
    stp x6,  x7,  [sp, #48]
    stp x8,  x9,  [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    str x30,      [sp, #240]
    .endm

    .macro RESTORE_GPRS
    ldp x2,  x3,  [sp, #16]
    ldp x4,  x5,  [sp, #32]
    ldp x6,  x7,  [sp, #48]
    ldp x8,  x9,  [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30,      [sp, #240]
    ldp x0,  x1,  [sp], #256
    .endm

    // ---- FP/SIMD save/restore: v0-v31 (512 B) + FPSR/FPCR (16 B), a 528-byte 16-aligned frame ----
    // Used by the IRQ stub only. The kernel is built `+neon`, so ordinary Rust (memcpy, VecDeque,
    // fmt) autovectorizes; an ASYNC interrupt can land while the interrupted task has live vector
    // state, and the handler tree — plus, once the scheduler preempts, the NEXT task's code —
    // clobbers v0-v31 before this task is resumed. GPR-only save was sound only while the dispatch
    // tree stayed FP-free; preemption (which runs a whole other task between save and eret) breaks
    // that, so we save the full FP file here. Uses x0/x1 as scratch — call only AFTER SAVE_GPRS has
    // spilled them. (FP access is already enabled: the GUI does NEON framebuffer work.)
    .macro SAVE_FP
    sub sp, sp, #528
    mrs x0, fpsr
    mrs x1, fpcr
    stp x0, x1, [sp, #0]
    stp q0,  q1,  [sp, #16]
    stp q2,  q3,  [sp, #48]
    stp q4,  q5,  [sp, #80]
    stp q6,  q7,  [sp, #112]
    stp q8,  q9,  [sp, #144]
    stp q10, q11, [sp, #176]
    stp q12, q13, [sp, #208]
    stp q14, q15, [sp, #240]
    stp q16, q17, [sp, #272]
    stp q18, q19, [sp, #304]
    stp q20, q21, [sp, #336]
    stp q22, q23, [sp, #368]
    stp q24, q25, [sp, #400]
    stp q26, q27, [sp, #432]
    stp q28, q29, [sp, #464]
    stp q30, q31, [sp, #496]
    .endm

    .macro RESTORE_FP
    ldp x0, x1, [sp, #0]
    msr fpsr, x0
    msr fpcr, x1
    ldp q0,  q1,  [sp, #16]
    ldp q2,  q3,  [sp, #48]
    ldp q4,  q5,  [sp, #80]
    ldp q6,  q7,  [sp, #112]
    ldp q8,  q9,  [sp, #144]
    ldp q10, q11, [sp, #176]
    ldp q12, q13, [sp, #208]
    ldp q14, q15, [sp, #240]
    ldp q16, q17, [sp, #272]
    ldp q18, q19, [sp, #304]
    ldp q20, q21, [sp, #336]
    ldp q22, q23, [sp, #368]
    ldp q24, q25, [sp, #400]
    ldp q26, q27, [sp, #432]
    ldp q28, q29, [sp, #464]
    ldp q30, q31, [sp, #496]
    add sp, sp, #528
    .endm

    // ---- IRQ: save, dispatch in Rust, restore, return ----
    // Beyond the GPRs + FP, bank ELR/SPSR (the return PC + PSTATE the CPU banked on entry; the EL
    // suffix is _EL1 on the baremetal build, _EL2 on UEFI — see the irq_el! selector above) and
    // (M6e) SP_EL0, into a 32-byte 16-aligned frame. These are SYSTEM registers, not stacked like
    // x86's interrupt frame, so they are per-*core*, not per-*context*. The scheduler's timer
    // preemption (timer::on_tick -> sched::timer_preempt) does a context switch INSIDE this handler;
    // the task resumed in its place takes its own IRQs, which overwrite ELR/SPSR/SP_EL0. Without
    // saving+restoring them here, a preempted task would later `eret` to another task's PC — or, for
    // a preemptible EL0 task, resume at EL0 with another task's user stack pointer in SP_EL0. The
    // banked copies live on THIS task's kernel stack, which survives the switch, so the epilogue
    // restores the right values. (The cooperative path never touches these — no IRQ, no eret.)
    // SP_EL0 is banked UNCONDITIONALLY: for a Current-EL (EL1/EL2 kernel) preempt it is a harmless
    // same-value round-trip (SP_EL0 is not the active stack there); only a Lower-EL (EL0) preempt
    // carries a live user SP. Do NOT read "the SP_EL0 msr ran" as "the interrupted context was EL0" —
    // test SPSR.M[3:0] for that (see aarch64_irq_handler's M6e counter). The EL is compile-time:
    // baremetal drops to EL1 (irq_el!() = "1"), UEFI stays at EL2 ("2").
    .globl __vec_irq
__vec_irq:
    SAVE_GPRS
    SAVE_FP
    mrs x0, ELR_EL"#, irq_el!(), r#"
    mrs x1, SPSR_EL"#, irq_el!(), r#"
    mrs x2, SP_EL0
    stp x0, x1, [sp, #-32]!
    str x2, [sp, #16]
    bl aarch64_irq_handler
    // INVARIANT: this epilogue (the ELR/SPSR/SP_EL0 restore -> RESTORE_FP/GPRS -> eret) MUST run with
    // IRQ masked. It restores per-core banked registers from THIS task's stack frame; a nested IRQ
    // between the msr's and the eret would re-bank ELR/SPSR/SP_EL0 and corrupt the return. The mask is
    // currently guaranteed for free: exception ENTRY masked all of DAIF, and when the handler
    // context-switched (timer preempt), `switch_context` banked/restored DAIF so the resumed task lands
    // back here still I-masked. Do NOT add any in-handler unmask on this path (e.g. to run a longer
    // bottom-half) without first re-masking before this epilogue.
    ldp x0, x1, [sp]
    ldr x2, [sp, #16]
    add sp, sp, #32
    msr ELR_EL"#, irq_el!(), r#", x0
    msr SPSR_EL"#, irq_el!(), r#", x1
    msr SP_EL0, x2
    RESTORE_FP
    RESTORE_GPRS
    eret

    // ---- Fault paths: save, pass a kind code, log + halt in Rust (never returns) ----
    // The `b .` after each call makes the halt an assembly-level guarantee: aarch64_fault_handler is
    // typed `-> !` (it hlt_loops), but should that ever change, this traps here instead of silently
    // falling through into the next stub's SAVE_GPRS (frame corruption / wrong exception kind).
    .globl __vec_sync
__vec_sync:
    SAVE_GPRS
    mov x0, #0
    bl aarch64_fault_handler
    b .
    .globl __vec_fiq
__vec_fiq:
    SAVE_GPRS
    mov x0, #2
    bl aarch64_fault_handler
    b .
    .globl __vec_serror
__vec_serror:
    SAVE_GPRS
    mov x0, #3
    bl aarch64_fault_handler
    b .
"#,
    svc_stub!()
));

unsafe extern "C" {
    static __exception_vectors: u8;
}

/// Install the exception vector table into VBAR for the current Exception Level, and latch the EL.
/// Must run before interrupts are unmasked. VBAR requires a 2 KiB-aligned base (bits[10:0] RES0);
/// we assert the table came out aligned so a misalignment surfaces loudly instead of silently
/// pointing VBAR a few KiB short of the table.
pub fn install() {
    let vbar = (&raw const __exception_vectors) as u64;
    assert!(vbar & 0x7FF == 0, "exception vector table is not 2 KiB aligned: {:#x}", vbar);

    let el = current_el();
    BOOT_EL.store(el, Ordering::Relaxed);
    unsafe {
        match el {
            2 => {
                core::arch::asm!("msr VBAR_EL2, {}", in(reg) vbar, options(nomem, nostack, preserves_flags));
                // Route physical IRQ/FIQ/SError to EL2 (HCR_EL2: AMO=bit5, IMO=bit4, FMO=bit3).
                // Without this, a physical IRQ taken while executing AT EL2 has target EL1 (the
                // HCR_EL2.IMO==0 default) — a *lower* EL than current — so the architecture leaves it
                // PENDING and it would never reach our handler. Setting IMO makes EL2 the IRQ target.
                let mut hcr: u64;
                core::arch::asm!("mrs {}, HCR_EL2", out(reg) hcr, options(nomem, nostack, preserves_flags));
                hcr |= (1 << 5) | (1 << 4) | (1 << 3);
                core::arch::asm!("msr HCR_EL2, {}", in(reg) hcr, options(nomem, nostack, preserves_flags));
            }
            1 => core::arch::asm!("msr VBAR_EL1, {}", in(reg) vbar, options(nomem, nostack, preserves_flags)),
            other => panic!("aarch64 exceptions: unexpected boot EL{}", other),
        }
        // Synchronize the VBAR/HCR writes before any exception can be taken against them.
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
    serial_println!(":: AARCH64 exception vectors installed (VBAR_EL{} = {:#x}) ::", el, vbar);
}

/// The current Exception Level (3..0) from CurrentEL[3:2].
pub fn current_el() -> u8 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) v, options(nomem, nostack, preserves_flags)) };
    ((v >> 2) & 0b11) as u8
}

/// Unmask IRQs (clear PSTATE.I). `daifclr, #2` clears the I bit (the immediate is a 4-bit field
/// {D=8, A=4, I=2, F=1}). Call only after the vectors are installed and the GIC/timer are up.
#[inline]
pub fn enable_irq() {
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags)) };
    // Baremetal drops to EL1 (boot::drop_to_el1) with DAIF fully masked; now that the vectors are
    // installed, also unmask SError (PSTATE.A, daifclr #4) so a genuine external-abort SError reaches
    // the fault logger (__vec_serror) instead of being held pending forever. The UEFI/EL2 path
    // inherits the firmware DAIF, so leave its A bit alone.
    #[cfg(feature = "baremetal")]
    unsafe { core::arch::asm!("msr daifclr, #4", options(nomem, nostack, preserves_flags)) };
}

/// Rust IRQ dispatcher (called from the IRQ vector stub). EL-agnostic: acknowledge at the GIC CPU
/// interface, route by INTID, and signal EOI. Lock-free in the common (timer) case.
#[unsafe(no_mangle)]
extern "C" fn aarch64_irq_handler() {
    // M6e (baremetal only): if this IRQ interrupted an EL0 task, count it — the crisp, metal-only
    // proof that EL0 is now preemptible (I-unmasked). Read the banked SPSR_EL1 BEFORE gic::handle_irq
    // (which may context-switch and thus clobber the per-core SPSR_EL1). Its value here is THIS
    // entry's, and it stays valid because exception ENTRY masks ALL of DAIF (D,A,I,F=1): no nested
    // IRQ/FIQ/SError can be taken to overwrite SPSR_EL1 before this read — even though enable_irq
    // unmasks SError (PSTATE.A) globally, that clear does not survive exception entry. SPSR.M[4:0]==0
    // is EL0t (an AArch64 EL0 return); any kernel-EL preempt has M != 0. `syscall` is baremetal-only,
    // so the whole block is cfg-gated (the EL2/UEFI/jetson build has no EL0 and no `syscall` module).
    #[cfg(feature = "baremetal")]
    {
        let spsr: u64;
        unsafe {
            core::arch::asm!("mrs {}, SPSR_EL1", out(reg) spsr, options(nomem, nostack, preserves_flags))
        };
        if spsr & 0b1_1111 == 0 {
            super::syscall::note_el0_irq();
        }
    }
    super::gic::handle_irq();
}

/// Fatal-exception logger (called from the sync/fiq/serror stubs; `kind`: 0=sync, 2=fiq, 3=serror).
/// Reads the syndrome/return/fault-address registers for the EL we booted at, prints them, and
/// halts — the ARM analogue of the x86 page-fault/GPF/double-fault handlers.
#[unsafe(no_mangle)]
extern "C" fn aarch64_fault_handler(kind: u64) -> ! {
    let (esr, elr, far): (u64, u64, u64);
    unsafe {
        if BOOT_EL.load(Ordering::Relaxed) == 2 {
            core::arch::asm!("mrs {}, ESR_EL2", out(reg) esr, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, ELR_EL2", out(reg) elr, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, FAR_EL2", out(reg) far, options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("mrs {}, ESR_EL1", out(reg) esr, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, ELR_EL1", out(reg) elr, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, FAR_EL1", out(reg) far, options(nomem, nostack, preserves_flags));
        }
    }
    let what = match kind {
        0 => "SYNCHRONOUS",
        2 => "FIQ",
        3 => "SERROR",
        // M6b: only reached if an EL0 fault arrives with NO current task — 0x400 should be
        // unreachable outside a dispatched user task, so this is a kernel bug, not a user one.
        4 => "EL0-SYNC (no current task)",
        _ => "UNKNOWN",
    };
    // ESR EC field (bits 31:26) classifies the exception — the single most useful number here.
    let ec = (esr >> 26) & 0x3F;
    serial_println!("=== AARCH64 EXCEPTION: {} ===", what);
    serial_println!("ESR={:#x} (EC={:#04x})  ELR={:#x}  FAR={:#x}", esr, ec, elr, far);
    // EC 0x00 ("unknown reason") probe — the Orin EC=0 phantom (see arch_arm64.md "JB1 result"):
    // an UNDEF at an ELR whose bytes disassemble innocently smells like the I-side fetching
    // different bytes than the D-side holds (stale I-cache class). Read the instruction word back
    // through the D-side and print it with CTR_EL0 (IDC/DIC bits say what maintenance the core
    // architecturally requires): D-word == the ELF's encoding proves a D/I divergence; a different
    // word means real memory corruption. Guarded to a plausible identity-mapped kernel range so a
    // garbage ELR cannot fault the fault handler.
    if ec == 0 && kind == 0 && elr >= 0x8000_0000 && elr < 0x40_0000_0000 {
        let dword = unsafe { core::ptr::read_volatile(elr as *const u32) };
        let ctr: u64;
        unsafe {
            core::arch::asm!("mrs {}, CTR_EL0", out(reg) ctr, options(nomem, nostack, preserves_flags));
        }
        serial_println!(
            "EC0-probe: D-side [ELR]={:#010x}  CTR_EL0={:#x} (DIC={} IDC={})",
            dword,
            ctr,
            (ctr >> 29) & 1,
            (ctr >> 28) & 1,
        );
    }
    crate::arch::hlt_loop();
}

/// M6b: a non-SVC synchronous exception from EL0 — data/instruction abort, alignment, UNDEF, a
/// trapped EL0 sysreg access (SCTLR_EL1.UCT/UCI/DZE are 0, so CTR_EL0 reads / DC ops / WFI from
/// EL0 trap too) — KILLS the offending task instead of halting the kernel. Every non-0x15 EC at
/// the 0x400 vector routes here unconditionally: none may be swallowed or re-executed, and a
/// Current-EL fault (a genuine kernel bug) still takes the fatal `__vec_sync` path.
///
/// Reached from `__vec_svc_fault` at EL1 on the task's own kernel stack with DAIF masked — the
/// exact context the metal-proven sys_exit path runs in — so `sched::exit()` is safe: it marks the
/// task FINISHED and switches to the scheduler, which frees the Box (the exception frame is
/// abandoned with the stack; nothing unwinds).
///
/// Logging here is safe (unlike the RX ISR): the interrupted context is EL0, which cannot hold
/// SERIAL_PORT or any kernel lock; a holder on ANOTHER core either holds it IRQ-masked (bounded
/// critical section) or — poll_input's single-register read — unmasked-but-preemptible, so the
/// worst case is spinning until that core re-dispatches the holder. Bounded, no cycle.
///
/// FAR_EL1 is architecturally valid only for instruction/data aborts (EC 0x20/0x24) with ISS.FnV=0
/// and for PC-alignment faults (0x22); for every other EC it holds a stale value, so print `--`.
#[cfg(feature = "baremetal")]
#[unsafe(no_mangle)]
extern "C" fn aarch64_el0_fault_handler() -> ! {
    let (esr, elr, far): (u64, u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, ESR_EL1", out(reg) esr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, ELR_EL1", out(reg) elr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, FAR_EL1", out(reg) far, options(nomem, nostack, preserves_flags));
    }
    let ec = (esr >> 26) & 0x3F;
    let iss = esr & 0x1FF_FFFF;
    let far_valid = match ec {
        0x20 | 0x24 => (iss >> 10) & 1 == 0, // aborts: valid unless ISS.FnV (external abort)
        0x22 => true,                        // PC alignment fault
        _ => false,
    };
    let Some(name) = super::sched::current_name() else {
        aarch64_fault_handler(4); // no current task: a kernel bug — fatal, never a kill
    };
    if far_valid {
        serial_println!(
            ":: EL0 FAULT: task '{}' KILLED — EC={:#04x} ISS={:#x} ELR={:#x} FAR={:#x} ::",
            name,
            ec,
            iss,
            elr,
            far
        );
    } else {
        serial_println!(
            ":: EL0 FAULT: task '{}' KILLED — EC={:#04x} ISS={:#x} ELR={:#x} FAR=-- ::",
            name,
            ec,
            iss,
            elr
        );
    }
    super::syscall::record_el0_kill(name, ec, far, far_valid);
    super::sched::exit()
}
