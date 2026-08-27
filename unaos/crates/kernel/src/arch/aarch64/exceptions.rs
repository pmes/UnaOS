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

// The IRQ stub banks ELR/SPSR for the EL the exception was taken AT. A single `global_asm!` can't
// `#[cfg]` individual lines, so the whole bank/unbank SEQUENCE is selected here and spliced into
// __vec_irq via `concat!` — the newlines live INSIDE the strings, so this costs no source lines.
// Off tegra the EL is fixed at compile time (baremetal drops to EL1 -> _EL1; UEFI/QEMU-virt stays
// at EL2 -> _EL2) and those arms emit byte-identical asm to before. On TEGRA one table serves BOTH:
// JM4 arms the GIC-600 + CNTP and `timer::verify_live` PROVES interrupt-driven ticks AT EL2, then
// JM6 `boot_tegra::drop_to_el1` re-installs the same table as VBAR_EL1 — and neither digit is right
// for both ("2" UNDEFs at EL1, into the halting __vec_sync; "1" banks the wrong register at EL2), so
// tegra reads CurrentEL at RUNTIME (the __vec_sync/JB1f pattern; x3 scratch, labels 7/8 per-site).
#[cfg(all(not(feature = "tegra"), feature = "baremetal"))] macro_rules! irq_bank { () => { "mrs x0, ELR_EL1\n    mrs x1, SPSR_EL1" }; }
#[cfg(all(not(feature = "tegra"), not(feature = "baremetal")))] macro_rules! irq_bank { () => { "mrs x0, ELR_EL2\n    mrs x1, SPSR_EL2" }; }
#[cfg(feature = "tegra")] macro_rules! irq_bank { () => { "mrs x3, CurrentEL\n    cmp x3, #0x8\n    b.eq 7f\n    mrs x0, ELR_EL1\n    mrs x1, SPSR_EL1\n    b 8f\n7:  mrs x0, ELR_EL2\n    mrs x1, SPSR_EL2\n8:" }; }
#[cfg(all(not(feature = "tegra"), feature = "baremetal"))] macro_rules! irq_unbank { () => { "msr ELR_EL1, x0\n    msr SPSR_EL1, x1" }; }
#[cfg(all(not(feature = "tegra"), not(feature = "baremetal")))] macro_rules! irq_unbank { () => { "msr ELR_EL2, x0\n    msr SPSR_EL2, x1" }; }
#[cfg(feature = "tegra")] macro_rules! irq_unbank { () => { "mrs x3, CurrentEL\n    cmp x3, #0x8\n    b.eq 7f\n    msr ELR_EL1, x0\n    msr SPSR_EL1, x1\n    b 8f\n7:  msr ELR_EL2, x0\n    msr SPSR_EL2, x1\n8:" }; }

// M6a: the Lower-EL-AArch64 synchronous vector (0x400) routes to __vec_svc wherever EL0 exists —
// baremetal AND tegra_el0 (recovered boot-2 capture 2026-08-18: el0-hello's first `svc #0` hit the
// fault logger, ESR=0x56000000 EC=0x15 ELR=0x7800000014 — JETSON-EL0 widened syscall.rs but not this
// vector). Elsewhere it stays the halting __vec_sync, and svc_stub!() emits nothing.
#[cfg(feature = "aarch64_el0")]
macro_rules! svc_vec {
    () => {
        "__vec_svc"
    };
}
#[cfg(not(any(feature = "baremetal", feature = "tegra_el0")))] // EL0-NAMING: NEGATED/RUNTIME — KEPT LONGHAND ON PURPOSE. Cargo feature implication is ONE-WAY: `baremetal`/`tegra_el0` imply `aarch64_el0`, not the reverse, so `not(aarch64_el0)` would diverge from this predicate for anyone who enabled `aarch64_el0` ALONE. No gate leg builds that combination, which is the trap — a byte-identity check over the legs would PASS while the hazard shipped. Positive sites are safe because implication runs their way; these are not.
macro_rules! svc_vec {
    () => {
        "__vec_sync"
    };
}
#[cfg(feature = "aarch64_el0")]
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
#[cfg(not(any(feature = "baremetal", feature = "tegra_el0")))] // EL0-NAMING: NEGATED/RUNTIME — KEPT LONGHAND ON PURPOSE. Cargo feature implication is ONE-WAY: `baremetal`/`tegra_el0` imply `aarch64_el0`, not the reverse, so `not(aarch64_el0)` would diverge from this predicate for anyone who enabled `aarch64_el0` ALONE. No gate leg builds that combination, which is the trap — a byte-identity check over the legs would PASS while the hazard shipped. Positive sites are safe because implication runs their way; these are not.
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
    // suffix is _EL1 baremetal, _EL2 on UEFI, runtime on tegra — see irq_bank!/irq_unbank! above) and
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
    // baremetal drops to EL1 ("1"), UEFI stays at EL2 ("2"); on tegra it is a runtime CurrentEL test.
    .globl __vec_irq
__vec_irq:
    SAVE_GPRS
    SAVE_FP
    "#, irq_bank!(), r#"
    // ^ ELR/SPSR banked above: a fixed EL suffix off tegra, a CurrentEL runtime branch on tegra.
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
    "#, irq_unbank!(), r#"
    // ^ ELR/SPSR restored above under the same EL selection the prologue's irq_bank! used.
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
    SAVE_FP
    // JB1f (panel fold-in #1 — nest-safety): bank ELR/SPSR/SP_EL0 onto the frame, the __vec_irq
    // discipline. Since JB1e this stub can RETURN (heal -> eret), and the handler runs substantial
    // Rust (D-side probe, atomics, serial writes) — a sync fault NESTING inside it re-banks the
    // per-core ELR/SPSR, and without this frame copy the outer eret would resume at the NESTED
    // fault's PC/PSTATE (silent register/PC corruption). Like __vec_irq's tegra irq_bank! arm,
    // the EL here is a RUNTIME check: on tegra this vector serves EL2 (pre-drop, where the
    // fbcon boot-mirror window lives) AND EL1 (post-drop CAPSTONE, where a heal has fired on
    // metal) — and an ELR_EL2 access at EL1 itself UNDEFs. CurrentEL cannot fault; x0-x3 are
    // scratch here (SAVE_GPRS spilled them).
    mrs x2, CurrentEL
    cmp x2, #0x8
    b.eq 2f
    mrs x0, ELR_EL1
    mrs x1, SPSR_EL1
    b 3f
2:  mrs x0, ELR_EL2
    mrs x1, SPSR_EL2
3:  mrs x2, SP_EL0
    stp x0, x1, [sp, #-32]!
    str x2, [sp, #16]
    mov x0, #0
    bl aarch64_fault_handler
    // JB1e (the A78AE erratum-1941500 heal): the handler returns 1 to RETRY the faulting
    // instruction after I-side invalidation (EC=0 with a valid D-side word = the proven stale
    // macro-op/I-side divergence; the chicken bit is EL3-gated on this firmware, so retry is the
    // only OS-side mitigation). The banked ELR/SPSR/SP_EL0 are restored from THIS frame (runtime
    // EL again — heals fire at either EL on tegra) so the eret resumes exactly at the faulting PC
    // even if the handler itself took (and healed) a nested fault. FP is saved/restored because
    // the handler's Rust may clobber SIMD regs (+neon fmt); RESTORE_FP scratches x0/x1, which is
    // why the msr's run first (the __vec_irq epilogue ordering).
    cbz x0, 1f
    ldp x0, x1, [sp]
    ldr x2, [sp, #16]
    add sp, sp, #32
    mrs x3, CurrentEL
    cmp x3, #0x8
    b.eq 4f
    msr ELR_EL1, x0
    msr SPSR_EL1, x1
    b 5f
4:  msr ELR_EL2, x0
    msr SPSR_EL2, x1
5:  msr SP_EL0, x2
    RESTORE_FP
    RESTORE_GPRS
    eret
1:  b .
    .globl __vec_fiq
__vec_fiq:
    SAVE_GPRS
    mov x0, #2
    bl aarch64_fault_handler
    b .
    // ---- SError: drain-tolerant. A fail-closed MMIO probe (poisoned/aborted V3D or PCIe access)
    // can leave a LATENT asynchronous external abort pending, delivered only when PSTATE.A first
    // unmasks — on metal this landed as a fatal SERROR at the first timer tick (R22 sitting-2, both
    // kernels). `drain_pending_serror` opens a bounded, ARMED unmask window; if the SError arrives
    // while armed, aarch64_serror_drain_check consumes it (logs the witness, returns 1) and we
    // RESUME the interrupted context — which is exactly why this stub, unlike the other fault
    // dead-ends, pairs SAVE_FP/RESTORE_FP with SAVE_GPRS (the resumed context's NEON must survive
    // the Rust logging under it). Unarmed, it falls through to the fatal logger exactly as before.
    .globl __vec_serror
__vec_serror:
    SAVE_GPRS
    SAVE_FP
    bl aarch64_serror_drain_check
    cbnz x0, 6f
    mov x0, #3
    bl aarch64_fault_handler
    b .
6:  RESTORE_FP
    RESTORE_GPRS
    eret
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
    VECTORS_LIVE.store(true, Ordering::Release);
    // SError-drain class fix (R22 sitting 2): if a pre-vector fail-closed probe (V3D / PIUSB, both
    // run inside build_boot_info — before this install) requested a drain, the latent asynchronous
    // external abort is still pending in PSTATE.A's shadow. Consume it NOW, at the first point the
    // vectors can catch it, instead of letting enable_irq's permanent A-unmask deliver it as a
    // fatal SERROR at the first timer tick (the metal signature: ESR=0xbf000002 at ELR=first-tick).
    // swap(false) makes this once-only: the BSP installs first on the pi flow; AP installs and the
    // tegra post-drop re-install find the request already consumed.
    if SERROR_DRAIN_PENDING.swap(false, Ordering::AcqRel) {
        drain_now("deferred: pre-vector fail-closed probe");
    }
    // JB1f: surface heals that predate this install — the tegra post-drop re-install runs after
    // the EL2 boot stretch, so a heal whose per-strike line lost the SERIAL_PORT try_lock still
    // shows up here. Silent when zero, so QEMU/pi output is byte-identical.
    let heals = EC0_HEALS.load(Ordering::Relaxed);
    if heals > 0 {
        serial_println!(":: JB1f — EC0 heals so far this boot: {} ::", heals);
    }
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

// ══ SError-drain class fix (R22 sitting 2, the metal SERROR-at-first-tick defect) ══════════════════
//
// A poison-honest/fail-closed MMIO probe (V3D bring-up writes into a block that never came up; a
// VL805 config read the RC answered with abort fill) can leave a PENDING asynchronous external abort
// behind: the access completed from the CPU's view, the fabric's abort response arrives later, and
// with PSTATE.A masked it sits latent until DAIF first unmasks — where it kills the machine as a
// fatal `SERROR ESR=0xbf000002` at whatever instruction happens to be live (on metal: the first
// timer tick, twice, on two kernels). The class fix: after any such probe, the probing module calls
// `serror_drain_request()`. If the vectors are live, a bounded ARMED unmask window consumes the
// abort immediately; if not (both offenders run pre-vector, in build_boot_info), the request is
// latched and `install()` performs the drain the moment the vectors can catch it. Either way a
// fail-closed probe leaves the machine clean, with a serial witness line whenever an abort was
// actually consumed. No protection is weakened: an SError arriving OUTSIDE an armed window still
// takes the fatal logger exactly as before. (FEAT_RAS `esb` would be the architected alternative;
// the unmask window works on every core we run, RAS or not.)

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

/// True once `install()` has pointed VBAR at our table on the boot core.
static VECTORS_LIVE: AtomicBool = AtomicBool::new(false);
/// A pre-vector fail-closed probe requested a drain; `install()` services it once.
static SERROR_DRAIN_PENDING: AtomicBool = AtomicBool::new(false);
/// The drain window is open: `__vec_serror` consumes-and-resumes instead of halting.
static SERROR_DRAIN_ARMED: AtomicBool = AtomicBool::new(false);
/// Total SErrors consumed by armed windows this boot (witness bookkeeping).
static SERROR_DRAINED: AtomicU32 = AtomicU32::new(0);
/// ESR of the most recently drained SError (for the summary witness line).
static SERROR_DRAIN_ESR: AtomicU64 = AtomicU64::new(0);

/// Called by a fail-closed MMIO probe (V3D, PIUSB, GENET — any module whose poison-honest branch
/// may have left an aborted access in flight). Synchronizes the access, then drains the latent
/// SError now (vectors live) or latches the request for `install()` (pre-vector boot stretch).
pub fn serror_drain_request(site: &str) {
    // Complete the aborted access from the CPU's side so the abort, if any, is pending NOW.
    unsafe { core::arch::asm!("dsb sy", "isb", options(nostack, preserves_flags)) };
    if VECTORS_LIVE.load(Ordering::Acquire) {
        drain_now(site);
    } else {
        SERROR_DRAIN_PENDING.store(true, Ordering::Release);
    }
}

/// Open a bounded, armed SError window: mask A (so arming and unmasking are ordered), arm the
/// tolerant handler, unmask A (a pending abort is delivered HERE, into `__vec_serror`'s drain
/// branch), close the window, restore the caller's original A state. Prints one summary witness
/// line when at least one abort was consumed; silent otherwise (QEMU logs stay byte-identical).
fn drain_now(site: &str) {
    let before = SERROR_DRAINED.load(Ordering::Relaxed);
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr daifset, #4", options(nomem, nostack, preserves_flags));
    }
    SERROR_DRAIN_ARMED.store(true, Ordering::SeqCst);
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "isb",
            "msr daifclr, #4", // pending SError (if any) delivered here, armed
            "isb",
            "isb",
            "msr daifset, #4", // window closed
            "isb",
            options(nostack, preserves_flags)
        );
    }
    SERROR_DRAIN_ARMED.store(false, Ordering::SeqCst);
    let drained = SERROR_DRAINED.load(Ordering::Relaxed).wrapping_sub(before);
    if drained > 0 {
        serial_println!(
            ":: SERROR-DRAIN: consumed {} latent async abort(s) (last ESR={:#x}) after fail-closed probe [{}] — machine clean ::",
            drained,
            SERROR_DRAIN_ESR.load(Ordering::Relaxed),
            site
        );
    }
    // Restore the caller's A-bit (DAIF.A is bit 8): only re-unmask if it was unmasked coming in.
    if daif & (1 << 8) == 0 {
        unsafe { core::arch::asm!("msr daifclr, #4", options(nomem, nostack, preserves_flags)) };
    }
}

/// Reached FIRST from `__vec_serror`. Returns 1 (consume + resume the interrupted context) only
/// while a drain window is armed; 0 falls through to the fatal logger — the pre-existing behaviour
/// for every SError outside an armed window (no protection weakened).
#[unsafe(no_mangle)]
extern "C" fn aarch64_serror_drain_check() -> u64 {
    if !SERROR_DRAIN_ARMED.load(Ordering::SeqCst) {
        return 0;
    }
    let esr: u64;
    unsafe {
        if BOOT_EL.load(Ordering::Relaxed) == 2 {
            core::arch::asm!("mrs {}, ESR_EL2", out(reg) esr, options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("mrs {}, ESR_EL1", out(reg) esr, options(nomem, nostack, preserves_flags));
        }
    }
    SERROR_DRAIN_ESR.store(esr, Ordering::Relaxed);
    SERROR_DRAINED.fetch_add(1, Ordering::Relaxed);
    1
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

/// JB1e: heals applied to the A78AE erratum-1941500 phantom this boot (bounded so a genuinely
/// broken instruction stream cannot retry forever). JB1f (panel fold-in #2) split the bound in
/// two: a raised GLOBAL budget (the phantom relocates with every binary layout, and a hot-loop
/// site can legitimately re-strike across iterations — 64 was sized to the isolated-strike era)
/// plus a CONSECUTIVE-same-PC cap that preserves the wedged-core stop: a fault->heal->refault
/// livelock at one PC burns its streak in microseconds and dies with the full syndrome, instead
/// of silently draining the global budget. The streak resets whenever a DIFFERENT PC heals
/// (forward progress proven). Counters are fetch_add/store (the old load-then-store pair was
/// non-atomic); Relaxed — the boot core is the only writer today.
static EC0_HEALS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static EC0_LAST_ELR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static EC0_SAME_PC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
const EC0_HEAL_CAP: u32 = 1024;
const EC0_SAME_PC_CAP: u32 = 32;

/// Fatal-exception logger (called from the sync/fiq/serror stubs; `kind`: 0=sync, 2=fiq, 3=serror).
/// Reads the syndrome/return/fault-address registers for the EL we booted at, prints them, and
/// halts — the ARM analogue of the x86 page-fault/GPF/double-fault handlers.
///
/// JB1e exception: an EC=0x00 "unknown reason" sync fault whose D-side instruction word reads back
/// valid is the PROVEN A78AE erratum-1941500 signature (stale I-side state; MIDR r0p1 is in the
/// affected range, the CPUECTLR_EL1[8] workaround bit ships CLEAR in NVIDIA's firmware, and the
/// bit is EL3-gated — a lower-EL write traps fatally — so an I-side invalidate + RETRY is the only
/// OS-side mitigation). Returns 1 to make __vec_sync restore the frame and eret back to the
/// faulting PC; every heal prints one counted line. All other faults print and halt as before.
#[unsafe(no_mangle)]
extern "C" fn aarch64_fault_handler(kind: u64) -> u64 {
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
    // JB1e: the erratum heal, tried BEFORE the fatal print. Conditions: a sync entry, EC=0, an
    // ELR in the identity-mapped kernel range, a VALID-looking D-side instruction word (nonzero,
    // not all-ones — a real UNDEF encoding would halt below on the next retry anyway), and heal
    // budget left. `ic iallu` refreshes the I-side (macro-op cache included) for this core.
    if kind == 0 && (esr >> 26) & 0x3F == 0 && (0x8000_0000..0x40_0000_0000).contains(&elr) {
        let dword = unsafe { core::ptr::read_volatile(elr as *const u32) };
        if dword != 0 && dword != 0xFFFF_FFFF {
            // JB1f: consecutive-same-PC streak. A different healed PC proves forward progress and
            // resets the streak; the same PC re-faulting past EC0_SAME_PC_CAP with no progress in
            // between is treated as a wedged core and falls through to the fatal print below.
            let same = if EC0_LAST_ELR.load(Ordering::Relaxed) == elr {
                EC0_SAME_PC.fetch_add(1, Ordering::Relaxed) + 1
            } else {
                EC0_LAST_ELR.store(elr, Ordering::Relaxed);
                EC0_SAME_PC.store(1, Ordering::Relaxed);
                1
            };
            if EC0_HEALS.load(Ordering::Relaxed) < EC0_HEAL_CAP && same <= EC0_SAME_PC_CAP {
                let total = EC0_HEALS.fetch_add(1, Ordering::Relaxed) + 1;
                // JB2b hardening: print the heal line WITHOUT a blocking lock. The phantom can strike
                // an instruction INSIDE an in-progress `_print` on this same core — the serial spin
                // Mutex is then already held here, and `without_interrupts` cannot mask a SYNCHRONOUS
                // exception — so the plain serial_println! this used to be would spin-deadlock the
                // very heal that was about to fix the fault (a dark hang instead of a recovery). The
                // fatal path below accepts that lock risk as a last-resort diagnostic; the heal is a
                // continue-running path and must not. try_lock: the uncontended common case prints; a
                // fault-interrupted holder skips the line — the heal itself needs no lock, and the
                // EC0_HEALS counter still records it. The fbcon mirror (its own locks) is skipped
                // too; the headless Orin loses nothing.
                // JB1f dedup: a same-PC streak prints its first strike then every 8th — at loop
                // frequency each ~100-char line costs ~8 ms of 115200-baud UART, which would turn
                // a heal storm into a serial-throttled crawl. The counters record every heal.
                if same == 1 || same % 8 == 0 {
                    if let Some(mut port) = super::serial::SERIAL_PORT.try_lock() {
                        use core::fmt::Write;
                        let _ = writeln!(
                            port,
                            ":: A78AE-1941500 heal #{}: EC=0 at ELR={:#x} (D-side {:#010x} valid, same-PC x{}) — ic iallu + retry ::",
                            total,
                            elr,
                            dword,
                            same,
                        );
                    }
                }
                unsafe {
                    core::arch::asm!(
                        "ic iallu",
                        "dsb ish",
                        "isb",
                        options(nostack, preserves_flags)
                    );
                }
                return 1;
            }
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
    // JB1f: when heals ran this boot, say so at the fatal stop — distinguishes "the budget/streak
    // cap declared this PC wedged" from "a fresh non-healable fault" without counting serial
    // lines (a try_lock-skipped heal prints nothing). Silent when zero (QEMU/pi byte-identical).
    let heals = EC0_HEALS.load(Ordering::Relaxed);
    if heals > 0 {
        serial_println!(
            "EC0 heal tally: {} total (cap {}), same-PC streak {} at ELR={:#x} (cap {})",
            heals,
            EC0_HEAL_CAP,
            EC0_SAME_PC.load(Ordering::Relaxed),
            EC0_LAST_ELR.load(Ordering::Relaxed),
            EC0_SAME_PC_CAP,
        );
    }
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
#[cfg(feature = "aarch64_el0")]
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
        // No current task: a kernel bug — fatal, never a kill. kind=4 can never take the JB1e
        // heal path (it is kind==0-gated), so the handler always prints-and-halts here; the
        // explicit hlt_loop restores the divergence the old `-> !` signature provided.
        aarch64_fault_handler(4);
        crate::arch::hlt_loop();
    };
    // KEYSTAT: name the PID as well as the task name. The name alone is not an identity — every
    // program launched with `bg` runs under the single literal `"bg-user"` (`spawn_user_image_bg`),
    // so with two `STAT.ELF` instances on the panel a fault line said only "one of them died" and
    // the operator's `kill <pid>` (answered "already exited") was the first and only evidence of
    // WHICH. The pid is the number `jobs` prints, the number `kill` takes, and the number the app
    // itself draws in its window, so it is the one token that closes the attribution.
    //
    // `current_id()` is the same task id `SYS_GETPID`/`SYS_GETINFO` return and the same value
    // `spawn_user_slot` handed the `Proc` row, so no translation is involved. It is also LOCK-FREE
    // — one `percpu` read, one `Acquire` load of `SCHED[cpu].current`, one field read — which is
    // the binding constraint here: this handler runs at EL1 with DAIF masked on the faulting task's
    // own kernel stack, and it must not reach for anything the interrupted context could have been
    // holding. (0 is unreachable in practice: `current_name()` above already established a current
    // task, and both read the same slot.)
    let pid = super::sched::current_id().unwrap_or(0);
    if far_valid {
        serial_println!(
            ":: EL0 FAULT: task '{}' pid={} KILLED — EC={:#04x} ISS={:#x} ELR={:#x} FAR={:#x} ::",
            name,
            pid,
            ec,
            iss,
            elr,
            far
        );
    } else {
        serial_println!(
            ":: EL0 FAULT: task '{}' pid={} KILLED — EC={:#04x} ISS={:#x} ELR={:#x} FAR=-- ::",
            name,
            pid,
            ec,
            iss,
            elr
        );
    }
    super::syscall::record_el0_kill(name, ec, far, far_valid); #[cfg(feature = "orintenant")] super::display_tegra::orin_tenant_note_el0_fault(name, pid, esr, elr, far, far_valid); // ORIN-TENANTFAULT: the tenant-rung witness, so a crashed WINDOW OWNER is discriminable from a graceful exit (`orin_tenant_census`'s TENANT-EXITED-CLEAN fired on closes=0 reaped=1 either way). Placed AFTER `record_el0_kill` — that call's early-return arms are what route each fixture family to its own counter, and this one must not perturb them — and BEFORE `sched::exit()`, which never returns. Reads TTBR0_EL1/SPSR_EL1/CurrentEL and one atomic; takes no lock (see its doc). Line-NEUTRAL per this file's PANIC-Location rule.
    super::sched::exit()
}
