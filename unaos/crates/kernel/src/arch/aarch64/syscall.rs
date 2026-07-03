// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 EL0 userspace + the SVC syscall interface (M6a: the first privilege boundary; M6b: fault
// isolation + per-page user permissions; M6c: the well-behaved `hello` program moved OUT of kernel
// `.text` into a separately linked, baked-in flat blob — `USER_BLOB` below).
//
// The kernel runs at EL1 (see boot::drop_to_el1). A user task drops to EL0 (sched::spawn_user) and
// calls back in with `svc #0`; because the kernel is at EL1 and HCR_EL2.TGE=0, that SVC is taken to
// EL1 at VBAR_EL1 + 0x400, where the `__vec_svc` stub (exceptions.rs) saves the frame, checks
// ESR_EL1.EC==0x15 (SVC from AArch64), and calls `aarch64_svc_handler` here — on the faulting task's
// own kernel stack, IRQ-masked. The ABI is the Linux-aarch64 one: x8 = syscall number, args in x0–x5,
// return in x0.
//
// M6b: any OTHER synchronous exception from EL0 (abort/alignment/UNDEF/trapped sysreg) kills the
// task — `aarch64_el0_fault_handler` (exceptions.rs) logs it, records it here (`record_el0_kill`),
// and exits the task; the kernel survives. The user window is permission-split: the CODE page is
// EL0-RX/EL1-RO (flipped by boot::protect_user_code after the blob copy — the kernel's first live
// page-table update), the DATA/STACK pages are EL0-RW and never executable. The M6b demo proves all
// of it with four EL0 programs (one well-behaved — the M6c loaded blob — and three deliberately
// faulting inline fixtures) and a verdict task that demands the EXACT outcome split — see `verdict`
// and main.rs. M6f adds a real copy_from_user and a wider surface.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// --- Syscall numbers. WRITE/EXIT are the M6a/M6b core; REPORT is the M6d demo channel; YIELD/SLEEP_MS/
// GETPID/GETINFO are the M6f "real" surface (all thin over existing scheduler/timer primitives). The
// numbering is common across arches (documented in userspace.md) so the x86 U-side port stays aligned. ---
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
/// M6d demo: report a u64 value to the kernel, keyed by the calling task's name (see `m6d_report`).
/// Demo-only accounting channel — a real OS would not have this; it lets an EL0 program hand the kernel
/// the value it read from its own (slot-private) address space so the verdict can check isolation.
const SYS_REPORT: u64 = 3;
/// M6f: cooperatively give up the CPU — thin over `sched::yield_now()`. Returns 0.
const SYS_YIELD: u64 = 4;
/// M6f: sleep ~`a0` milliseconds — thin over `sched::sleep_ticks()` (ms→ticks at the 250 Hz tick, round
/// up). Returns 0. QEMU has no delivered timer IRQ, so it falls back to a cooperative yield there.
const SYS_SLEEP_MS: u64 = 5;
/// M6f: return the calling task's id (pid) in x0.
const SYS_GETPID: u64 = 6;
/// M6f: write a fixed {pid, ticks} struct to the user pointer in x0 via `copy_to_user`. Returns 0 or -EFAULT.
const SYS_GETINFO: u64 = 7;

/// M6e demo: the sentinel `sys_exit` status the preemption spinner uses so its exit is accounted to
/// `EL0_SPIN_DONE` and never perturbs the M6b `exited/killed` counters. Demo-only — there is no real
/// userspace yet, so overloading one status value for demo bookkeeping is safe and documented here.
const M6E_SPIN_STATUS: u64 = 0x6E;
/// M6d demo: the sentinel `sys_exit` status every M6d task uses so its exit lands in `EL0_M6D_DONE` and
/// never touches the M6b (`EL0_EXITED_OK/ERR`) or M6e (`EL0_SPIN_DONE`) counters — keeping those verdicts
/// byte-identical. The SYS_EXIT dispatch MUST test this BEFORE the catch-all `else` (see the handler).
const M6D_EXIT_STATUS: u64 = 0x6D;
/// M6f demo: the sentinel `sys_exit` status every M6f fixture uses so its exit lands in `EL0_M6F_DONE` and
/// never perturbs the M6b/M6d/M6e counters (same discipline as M6D/M6E). Tested BEFORE the catch-all `else`.
const M6F_EXIT_STATUS: u64 = 0x6F;

// --- The inline EL0 FIXTURES: three fault-SHAPE fixtures (M6b) + one preemption spinner (M6e). These
// are fixtures, not programs, so they stay inline in the kernel image; only the well-behaved `hello`
// routine moved out to a separately linked blob in M6c (see `USER_BLOB` below). Fully
// position-independent — every reference is a PC-relative `adr` and there are only svc + mov-immediate
// + register ops — so they run correctly wherever the copy lands. `__fault_blob_{start,end}` bound the
// copy; the `__user_prog_*` labels are the per-fixture entries.
//
// The three fault fixtures each provoke ONE specific fault the kernel must answer with a task-kill. If
// the fault DOESN'T happen (broken permissions / stale TLB), the fixture falls through to sys_exit(1)
// — the SURVIVOR protocol: a self-reported, greppable FAIL. The tail self-exits rather than `b .`
// because QEMU raspi4b delivers no timer IRQ, so an EL0 spin is UNpreemptible THERE regardless of M6e
// (on metal, M6e now WOULD preempt it) — a `b .` survivor would wedge its core for the full
// kernel8-test window and silence the same-core verdict the failure is supposed to reach. ---
core::arch::global_asm!(
    r#"
    .globl __fault_blob_start
__fault_blob_start:
    // Write to PA 0x0 — EL1-only RAM (AP=0b00) -> EL0 data abort, EC=0x24, FAR=0x0. `str xzr` so
    // even a bug that lets the store through writes zeros, not garbage, over the dead spin-table.
    .balign 4
    .globl __user_prog_wild_write
__user_prog_wild_write:
    mov x0, #0
    str xzr, [x0]
    mov x8, #2                              // survivor: the store didn't fault -> sys_exit(1)
    mov x0, #1
    svc #0
1:  b 1b

    // Write to its OWN code page (EL0-RO after protect_user_code) -> EC=0x24, FAR in the code page.
    // The 4-byte target is exactly its own FIRST instruction — already executed — so if a stale-TLB
    // write sneaks through it cannot corrupt code that still has to run (the survivor exit(1) tail).
    .balign 4
    .globl __user_prog_code_write
__user_prog_code_write:
    adr x0, __user_prog_code_write
    str wzr, [x0]
    mov x8, #2                              // survivor: the store didn't fault -> sys_exit(1)
    mov x0, #1
    svc #0
1:  b 1b

    // Branch into the user STACK page (EL0-readable but UXN=1) -> instruction abort, EC=0x20,
    // FAR = the branch target in the data pages. No survivor tail is needed: if UXN were broken
    // the target bytes are BSS zeros = UDF, still a kill — but with EC 0x00, which the (task, EC,
    // FAR-page) bookkeeping counts as killed_UNEXPECTED, failing the verdict as it must.
    .balign 4
    .globl __user_prog_stack_exec
__user_prog_stack_exec:
    sub x0, sp, #16
    br x0
1:  b 1b

    // M6e preemption spinner: a long, register-only, syscall-free EL0 loop, then sys_exit with the
    // M6E sentinel status. With I unmasked at EL0 (spawn_user, M6e) the ONLY thing that can switch it
    // away is a timer IRQ, so on metal it is preempted mid-loop and interleaves with the co-located
    // capstone/kernel tasks (aarch64_irq_handler counts the EL0 IRQs; see `m6e_verdict`). It writes
    // NO memory (register-only), so it shares the demo user stack safely under preemptive interleave.
    // Count 0x0200_0000 (~33.5M) ≈ a few timer quanta on a 1.5 GHz A72 (>=1 preempt on metal), and
    // bounded (~sub-second under QEMU TCG, which never preempts it — so it never hangs the regression).
    .balign 4
    .globl __user_prog_spin
__user_prog_spin:
    movz x9, #0x0200, lsl #16              // loop count = 0x0200_0000
1:  subs x9, x9, #1
    b.ne 1b
    mov x8, #2                             // SYS_EXIT
    movz x0, #0x6E                         // M6E sentinel status -> EL0_SPIN_DONE (M6b counters stay pure)
    svc #0
2:  b 2b                                   // sys_exit never returns; belt-and-braces guard

    .balign 4
    .globl __fault_blob_end
__fault_blob_end:
"#
);

unsafe extern "C" {
    static __fault_blob_start: u8;
    static __fault_blob_end: u8;
    static __user_prog_wild_write: u8;
    static __user_prog_code_write: u8;
    static __user_prog_stack_exec: u8;
    static __user_prog_spin: u8;
}

// --- M6d inline EL0 fixtures (per-task address spaces). Position-independent, register/stack-only, so
// they run wherever the kernel copies them into a slot's code page. Each program does its work, hands the
// kernel a value via SYS_REPORT (keyed by the task name in `m6d_report`), then `sys_exit(M6D_EXIT_STATUS)`
// so its exit is accounted to `EL0_M6D_DONE` and never perturbs the M6b/M6e counters. All reads/writes go
// through SP_EL0 (the slot-private stack) — the whole point of M6d — so the fixtures need no absolute VA.
// The whole blob (all three fixtures) is copied into EACH slot's code page; a task enters at its own
// fixture's offset. `[sp,#-0x100]` addresses the sentinel the kernel plants in data page 3. ---
core::arch::global_asm!(
    r#"
    .globl __m6d_blob_start
__m6d_blob_start:
    // same-VA isolation: read the slot-private sentinel the kernel planted at [top-0x100], report it,
    // exit. Two tasks (A and B) run this at the SAME VA in DIFFERENT slots, so each reports its own
    // slot's value — the verdict checks they are distinct and each equals what was planted.
    .balign 4
    .globl __m6d_prog_same_va
__m6d_prog_same_va:
    ldr x0, [sp, #-0x100]
    mov x8, #3                             // SYS_REPORT(value = x0)
    svc #0
    mov x8, #2                             // SYS_EXIT
    movz x0, #0x6D                         // M6D_EXIT_STATUS -> EL0_M6D_DONE (M6b/M6e counters stay pure)
    svc #0
1:  b 1b

    // stack write/readback (the capability this arc unlocks): push a known pattern onto the slot-private
    // user stack, pop it back, report the readback. A store to a non-writable stack would DATA-ABORT and
    // kill the task (no report -> verdict FAIL), so a correct report proves the EL0 stack is writable.
    .balign 4
    .globl __m6d_prog_stack_write
__m6d_prog_stack_write:
    movz x1, #0x1234
    movk x1, #0xABCD, lsl #16              // x1 = 0xABCD1234
    str x1, [sp, #-16]!                    // push (SP_EL0 -= 16)
    ldr x0, [sp], #16                      // pop back into x0 (SP_EL0 += 16)
    mov x8, #3                             // SYS_REPORT(readback)
    svc #0
    mov x8, #2
    movz x0, #0x6D
    svc #0
2:  b 2b

    // SP-relative sentinel readback: spin (register-only, preemptible), then read the planted sentinel
    // through SP and report it. On metal (IRQs>0) this proves SP_EL0 VALUE fidelity across preemption —
    // the spinner is interrupted mid-loop and must resume with the right user SP for the later
    // `[sp,#-0x100]` to hit its own sentinel (the M6e spinner could not observe this). Under QEMU (no
    // Group-1 IRQ) it still validates the slot mapping + read path.
    .balign 4
    .globl __m6d_prog_sp_sentinel
__m6d_prog_sp_sentinel:
    movz x9, #0x0080, lsl #16              // spin ~8.4M iterations (bounded; sub-second under QEMU TCG)
3:  subs x9, x9, #1
    b.ne 3b
    ldr x0, [sp, #-0x100]
    mov x8, #3                             // SYS_REPORT(sentinel)
    svc #0
    mov x8, #2
    movz x0, #0x6D
    svc #0
4:  b 4b

    .balign 4
    .globl __m6d_blob_end
__m6d_blob_end:
"#
);

unsafe extern "C" {
    static __m6d_blob_start: u8;
    static __m6d_blob_end: u8;
    static __m6d_prog_same_va: u8;
    static __m6d_prog_stack_write: u8;
    static __m6d_prog_sp_sentinel: u8;
}

// --- M6f inline EL0 fixtures (validated user pointers + wider syscall surface). Position-independent,
// register/stack-only, so they run wherever the kernel copies them into a slot's code page. Each runs on its
// OWN private slot (`spawn_user_slot`) — the getinfo fixture WRITES its stack (copy_to_user target), which
// the shared window forbids (the M6e stack STOP tripwire) — and exits with `M6F_EXIT_STATUS` (0x6F) so it
// lands in `EL0_M6F_DONE`, never perturbing the M6b/M6d/M6e counters. `adr xN, __m6f_blob_start` recovers
// the window base (the blob is copied at code-page offset 0 in each slot), used to synthesize hostile VAs.
// ABI: x8=nr, args x0-x2, ret x0. Numbers: WRITE=1, EXIT=2, REPORT=3, YIELD=4, SLEEP_MS=5, GETPID=6,
// GETINFO=7. `sys_write(fd,buf,len)` = (x0,x1,x2). ---
core::arch::global_asm!(
    r#"
    .globl __m6f_blob_start
__m6f_blob_start:
    // getinfo/copy_to_user round-trip (well-behaved): getpid -> x19; sys_getinfo(&info on our slot stack)
    // -> the kernel writes the pid+ticks struct there via copy_to_user; read info.pid back -> x21; witness is
    // the pid iff (info.pid == getpid && != 0), else 0 (so a mismatched/zero round-trip fails the verdict).
    // Then sys_write a short summary from the code page (the validated copy_from_user read path), report the
    // witness, exit. Writes ONLY its slot-private stack (sp-0x40, a data page), safe under preemption.
    .balign 4
    .globl __m6f_prog_getinfo
__m6f_prog_getinfo:
    mov  x8, #6                            // SYS_GETPID
    svc  #0
    mov  x19, x0                           // x19 = pid (P)
    sub  x20, sp, #0x40                    // x20 = &info (slot-private, writable data page)
    mov  x0, x20
    mov  x8, #7                            // SYS_GETINFO(&info) -> copy_to_user writes the pid+ticks struct
    svc  #0
    ldr  x21, [x20]                        // x21 = info.pid (S), round-tripped through copy_to_user
    mov  x22, xzr                          // witness = 0
    cmp  x21, x19
    b.ne 1f
    cbz  x19, 1f
    mov  x22, x19                          // matched & non-zero -> witness = pid
1:  mov  x0, #1                            // sys_write summary: fd=stdout
    adr  x1, __m6f_getinfo_msg
    mov  x2, #16                           // "el0: getinfo ok\n"
    mov  x8, #1                            // SYS_WRITE (routed through copy_from_user)
    svc  #0
    mov  x0, x22                           // SYS_REPORT(witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(M6F_EXIT_STATUS)
    movz x0, #0x6F
    svc  #0
2:  b 2b

    // hostile pointers (each must ERROR-RETURN -EFAULT, NOT kill the task): count the -14 returns in x19.
    //   1) sys_write to kernel RAM VA (0x4000_0000, L1[1] EL1-only) — exfiltration attempt
    //   2) sys_write just past the window (base + 0x4000); EL1-only under the slot root (copied kernel
    //      mapping), so only the range check refuses it — NOT a translation fault

    //   3) sys_write whose length wraps the address space (base + ~0 overflows)
    //   4) sys_getinfo targeting the RO code page (base) — copy_to_user must refuse the write target
    // A stray store or a kill would prevent the report (count != 4 -> verdict FAIL); a copy_to_user that
    // actually wrote the RO page would fault the KERNEL (halt) -> no verdict at all. Report the count, exit.
    .balign 4
    .globl __m6f_prog_hostile
__m6f_prog_hostile:
    mov  x19, xzr                          // count of EFAULT (-14) returns
    adr  x9, __m6f_blob_start              // x9 = user window base (code page)
    mov  x0, #1                            // (1) kernel/MMIO VA
    movz x1, #0x4000, lsl #16              // x1 = 0x4000_0000
    mov  x2, #8
    mov  x8, #1
    svc  #0
    cmn  x0, #14                           // x0 == -14 ?  (x0 + 14 == 0 -> Z)
    cinc x19, x19, eq
    mov  x0, #1                            // (2) just past the window (base+0x4000): EL1-only under the
                                           //     slot root (copied kernel mapping) -> range check refuses it
    add  x1, x9, #0x4000
    mov  x2, #8
    mov  x8, #1
    svc  #0
    cmn  x0, #14
    cinc x19, x19, eq
    mov  x0, #1                            // (3) length wraps (base + ~0)
    mov  x1, x9
    movn x2, #0xFF                         // x2 = 0xFFFF_FFFF_FFFF_FF00
    mov  x8, #1
    svc  #0
    cmn  x0, #14
    cinc x19, x19, eq
    mov  x0, x9                            // (4) sys_getinfo(RO code-page VA) — copy_to_user must refuse
    mov  x8, #7
    svc  #0
    cmn  x0, #14
    cinc x19, x19, eq
    mov  x0, x19                           // SYS_REPORT(count of refusals; want 4)
    mov  x8, #3
    svc  #0
    mov  x8, #2
    movz x0, #0x6F
    svc  #0
2:  b 2b

    // yield fixture: SYS_YIELD in a loop, then report the completed iteration count. Co-located with the
    // sleep fixture on one core; the two cooperatively interleave (the kernel counts the yield<->sleep
    // switches). Register-only, so preemption cannot corrupt anything.
    .balign 4
    .globl __m6f_prog_yield
__m6f_prog_yield:
    mov  x19, #8                           // iterations
    mov  x20, xzr
1:  mov  x8, #4                            // SYS_YIELD
    svc  #0
    add  x20, x20, #1
    cmp  x20, x19
    b.lt 1b
    mov  x0, x20                           // SYS_REPORT(completed count; want 8)
    mov  x8, #3
    svc  #0
    mov  x8, #2
    movz x0, #0x6F
    svc  #0
2:  b 2b

    // sleep fixture: SYS_SLEEP_MS in a loop (a real timed sleep on metal; a cooperative yield under QEMU,
    // where the timer IRQ is not delivered), then report the completed iteration count.
    .balign 4
    .globl __m6f_prog_sleep
__m6f_prog_sleep:
    mov  x19, #8
    mov  x20, xzr
1:  mov  x0, #2                            // sleep 2 ms
    mov  x8, #5                            // SYS_SLEEP_MS(a0 = ms)
    svc  #0
    add  x20, x20, #1
    cmp  x20, x19
    b.lt 1b
    mov  x0, x20                           // SYS_REPORT(completed count; want 8)
    mov  x8, #3
    svc  #0
    mov  x8, #2
    movz x0, #0x6F
    svc  #0
2:  b 2b

    .balign 4
__m6f_getinfo_msg:
    .ascii "el0: getinfo ok\n"
    .balign 4
    .globl __m6f_blob_end
__m6f_blob_end:
"#
);

unsafe extern "C" {
    static __m6f_blob_start: u8;
    static __m6f_blob_end: u8;
    static __m6f_prog_getinfo: u8;
    static __m6f_prog_hostile: u8;
    static __m6f_prog_yield: u8;
    static __m6f_prog_sleep: u8;
}

/// The `hello` EL0 program (M6c), built as a SEPARATE link product (`crates/user-blob`) and baked in
/// as a flat binary instead of living in the kernel's `.text`. `arroyo kernel8` builds it — a naked,
/// position-independent `sys_write("hello from EL0\n") + sys_exit(0)` routine — for the bare aarch64
/// target and `llvm-objcopy -O binary`s it to `target/user_blob.bin` BEFORE the kernel build; here we
/// `include_bytes!` it and copy it into the user CODE page at `setup()`, where it runs at EL0 exactly
/// like the old inline routine. The path is relative to this crate's manifest dir
/// (`unaos/crates/kernel`) → `unaos/target/user_blob.bin`; `include_bytes!` registers the file as a
/// rebuild dependency, so a changed routine re-triggers the kernel compile. Only ever compiled in the
/// baremetal build (this whole module is `#[cfg(feature = "baremetal")]`), so `./arroyo check`/`build`
/// — which do not build the blob — never need the file to exist.
static USER_BLOB: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/user_blob.bin"));

// --- M6b demo accounting. Written by the syscall/kill paths, read by `verdict`. ---
/// EL0 tasks that exited with status 0 (normal completion — the demo expects exactly 1: hello).
static EL0_EXITED_OK: AtomicU32 = AtomicU32::new(0);
/// EL0 tasks that exited nonzero — a fault-test program SELF-REPORTING that its intended fault
/// never happened (the survivor protocol). Any nonzero count is a FAIL.
static EL0_EXITED_ERR: AtomicU32 = AtomicU32::new(0);
/// Kills whose (task, EC, FAR-page) matched the demo's expectation table (want exactly 3).
static EL0_KILLED_EXPECTED: AtomicU32 = AtomicU32::new(0);
/// Kills that did NOT match — a fault happened, but not the one the permission model dictates
/// (e.g. UXN unset would turn stack-exec's instruction abort into an EC-0x00 UDF kill).
static EL0_KILLED_UNEXPECTED: AtomicU32 = AtomicU32::new(0);
/// Set by `tlb_warm` once the demo core has cached the pre-protect code-page mapping.
pub static TLB_WARMED: AtomicBool = AtomicBool::new(false);

// --- M6e demo accounting (decoupled from M6b so `exited=1 killed=3` stays byte-identical). ---
/// The preemption spinner reached its `sys_exit` (via the M6E sentinel status). 1 = it ran to
/// completion — under QEMU WITHOUT being preempted; on metal having been preempted (see below) and
/// then correctly resumed (the proof SP_EL0 banking works). Read by `m6e_verdict`.
static EL0_SPIN_DONE: AtomicU32 = AtomicU32::new(0);
/// IRQs taken while an EL0 task was the interrupted context (counted in `aarch64_irq_handler`, any
/// INTID — the timer, or any SPI such as the PL011 RX — and demo-WIDE: the spinner AND any of the four
/// M6b programs that a tick catches at EL0, since M6e makes them all preemptible). The crisp metal-only
/// proof that EL0 is preemptible: >0 on the real Pi 4, exactly 0 under QEMU raspi4b (no Group-1 IRQ is
/// ever delivered). The spinner's own resume-correctness proof is carried separately by
/// `EL0_SPIN_DONE == 1` (it completed after being interrupted). Read by `m6e_verdict`.
static EL0_IRQS_AT_EL0: AtomicU64 = AtomicU64::new(0);

/// M6e: count an IRQ taken while EL0 was running — called from `aarch64_irq_handler` when the banked
/// SPSR shows an EL0t return. Relaxed: a monotonic demo counter read once at the verdict, not a
/// synchronization point. NOTE (M6d): this stays demo-WIDE — it now also counts timer IRQs taken inside
/// the four M6d EL0 tasks, so on METAL `IRQs-taken-at-EL0` grows beyond the pre-M6d value (more
/// preemptible EL0 tasks). That value was always metal-variable; the QEMU regression stays `IRQs=0` (no
/// Group-1 IRQ is delivered there, so this is never called under QEMU) — see `m6e_verdict`.
#[inline]
pub fn note_el0_irq() {
    EL0_IRQS_AT_EL0.fetch_add(1, Ordering::Relaxed);
    // Part 0 fold #5: also bump the current (preempted) task's OWN counter. At IRQ time this core's
    // `current` is the preempted EL0 task, so `current_name` names it; the aggregate above stays for the
    // M6e verdict, this refines it to exact per-task attribution for the M6f verdict.
    if let Some(ctr) = task_preempt_counter(super::sched::current_name()) {
        ctr.fetch_add(1, Ordering::Relaxed);
    }
}

/// Map a demo EL0 task name to its per-task preempt counter (Part 0 fold #5), or None for any other task
/// (kernel tasks, the M6b/M6c fault fixtures + hello + spinner — not individually attributed).
fn task_preempt_counter(name: Option<&str>) -> Option<&'static AtomicU64> {
    Some(match name? {
        "el0-samevaA" => &PRE_SAMEVA_A,
        "el0-samevaB" => &PRE_SAMEVA_B,
        "el0-stackwrite" => &PRE_STACKWRITE,
        "el0-spsentinel" => &PRE_SPSENTINEL,
        "el0-yield" => &PRE_YIELD,
        "el0-sleep" => &PRE_SLEEP,
        _ => return None,
    })
}

// --- M6d demo accounting (per-task address spaces). Decoupled from the M6b/M6e counters — M6d tasks exit
// with `M6D_EXIT_STATUS` (routed to `EL0_M6D_DONE`) and any M6d kill is routed to `EL0_M6D_KILLED` (see
// `record_el0_kill`), so `exited=1 killed=3` (M6b) and `completed=1` (M6e) stay byte-identical. ---
/// M6d tasks that reached their sentinel `sys_exit` (the demo's completion signal; want 4).
static EL0_M6D_DONE: AtomicU32 = AtomicU32::new(0);
/// M6d tasks KILLED by a fault — a real per-slot ASID/permission bug. Kept OFF the M6b `killed_unexpected`
/// counter so an M6d metal failure surfaces as its own missing report/FAIL, not as a phantom M6b regression.
static EL0_M6D_KILLED: AtomicU32 = AtomicU32::new(0);
/// Values reported (via SYS_REPORT) by the four M6d tasks, keyed by name in `m6d_report`.
static M6D_REPORT_A: AtomicU64 = AtomicU64::new(0); // el0-samevaA read its slot sentinel
static M6D_REPORT_B: AtomicU64 = AtomicU64::new(0); // el0-samevaB read its slot sentinel
static M6D_REPORT_STACK: AtomicU64 = AtomicU64::new(0); // el0-stackwrite read its stack push/pop back
static M6D_REPORT_SP: AtomicU64 = AtomicU64::new(0); // el0-spsentinel read its sentinel through SP
/// The kernel-side deterministic nG detector's verdict (see `boot::probe_slot_isolation`, folded into the
/// same-VA PASS): the metal analogue of M6b's `tlb_warm` — true iff two slot roots resolved the SAME VA to
/// their OWN frames (a global nG bug would make both resolve to slot A's frame).
static M6D_PROBE_OK: AtomicBool = AtomicBool::new(false);

// M6d sentinel values planted into each reader task's slot-private data page (page 3, [top-0x100]). The
// low bits encode the slot's ASID so a cross-slot bleed fails the `== planted` check, not just distinctness.
const M6D_SENTINEL_A: u64 = 0xA5A5_0000_0000_0001; // slot A (ASID 1)
const M6D_SENTINEL_B: u64 = 0x5A5A_0000_0000_0002; // slot B (ASID 2)
const M6D_SENTINEL_SP: u64 = 0x5EED_0000_0000_0004; // slot D (ASID 4)
const M6D_STACK_PATTERN: u64 = 0xABCD_1234; // the in-program pattern el0-stackwrite pushes/pops

// --- M6f demo accounting (validated user pointers + wider syscall surface). Decoupled from the
// M6b/M6d/M6e counters exactly like M6d: M6f tasks exit with `M6F_EXIT_STATUS` -> `EL0_M6F_DONE`, and any
// M6f kill routes to `EL0_M6F_KILLED` (see `record_el0_kill`), so `exited=1 killed=3` (M6b), `completed=1`
// (M6e), and the M6d lines all stay byte-identical. Read by `m6f_verdict`. ---
/// M6f fixtures that reached their sentinel `sys_exit` (the demo's completion signal; want 4).
static EL0_M6F_DONE: AtomicU32 = AtomicU32::new(0);
/// M6f fixtures KILLED by a fault — a real bug (the hostile fixture's whole point is EFAULT returns, NOT
/// kills). Kept OFF the M6b counter so an M6f failure surfaces as its own FAIL, not a phantom M6b regression.
static EL0_M6F_KILLED: AtomicU32 = AtomicU32::new(0);
/// getinfo fixture witness: the pid it read back from the copy_to_user'd struct iff it matched SYS_GETPID
/// (and was non-zero), else 0. Non-zero == the to-user round-trip carried the correct value.
static M6F_GETINFO_WITNESS: AtomicU64 = AtomicU64::new(0);
/// hostile fixture: how many of its 4 bad pointers the kernel refused with -EFAULT (want 4).
static M6F_HOSTILE_REFUSED: AtomicU32 = AtomicU32::new(0);
/// yield / sleep fixtures: the loop iteration count each completed (want `M6F_ITERS` each — proof both ran).
static M6F_YIELD_DONE: AtomicU32 = AtomicU32::new(0);
static M6F_SLEEP_DONE: AtomicU32 = AtomicU32::new(0);
/// Observed yield<->sleep runner switches (see `note_interleave`); > 0 proves the two fixtures interleaved.
static M6F_INTERLEAVE_SWITCHES: AtomicU32 = AtomicU32::new(0);
/// Interleave witness state: 0 = no yielding M6f task has run yet; 1 = el0-yield last; 2 = el0-sleep last.
static M6F_INTERLEAVE_LAST: AtomicU32 = AtomicU32::new(0);
/// Iterations each interleave fixture loops (must match the `mov x19, #8` in the two inline programs).
const M6F_ITERS: u32 = 8;

// Per-task EL0 preempt counters (Part 0 review fold #5). `note_el0_irq` bumps the CURRENT (preempted)
// task's own counter, keyed by name, in addition to the demo-wide `EL0_IRQS_AT_EL0` aggregate — so the M6f
// verdict attributes preemption per slot task EXACTLY, refining the aggregate the M6d ledger called out as
// coarse. Name-keyed statics (not a `Task` field) so the count survives the task's teardown for the verdict
// to read. Metal-only signal: QEMU delivers no timer IRQ, so `note_el0_irq` is never called and all stay 0;
// on the real Pi 4 the timer preempts running EL0 tasks and these go > 0.
static PRE_SAMEVA_A: AtomicU64 = AtomicU64::new(0);
static PRE_SAMEVA_B: AtomicU64 = AtomicU64::new(0);
static PRE_STACKWRITE: AtomicU64 = AtomicU64::new(0);
static PRE_SPSENTINEL: AtomicU64 = AtomicU64::new(0);
static PRE_YIELD: AtomicU64 = AtomicU64::new(0);
static PRE_SLEEP: AtomicU64 = AtomicU64::new(0);

/// M6d: record a value an EL0 task reported via SYS_REPORT, keyed by the reporting task's name. Called on
/// the reporting task's own kernel stack (from the SVC handler), IRQ-masked.
fn m6d_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-samevaA") => M6D_REPORT_A.store(value, Ordering::Release),
        Some("el0-samevaB") => M6D_REPORT_B.store(value, Ordering::Release),
        Some("el0-stackwrite") => M6D_REPORT_STACK.store(value, Ordering::Release),
        Some("el0-spsentinel") => M6D_REPORT_SP.store(value, Ordering::Release),
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

/// The EL0 demo entry points (EL0 VAs inside the code page) and the shared initial SP_EL0.
///
/// All programs SHARE one user stack (`sp`). Through M6a–M6c that was safe because EL0 was
/// non-preemptible; under M6e EL0 IS preemptible (SP_EL0 banked in `__vec_irq`), so the shared stack
/// is now safe for a DIFFERENT, load-bearing reason: **no EL0 demo program writes its user stack** —
/// hello (`USER_BLOB`) and the spinner are register-only, and the fault fixtures fault or exit before
/// any push. With SP_EL0 banked per-task, preemptive interleave cannot corrupt a stack nobody writes.
/// STOP TRIPWIRE: the first EL0 program that actually WRITES its user stack needs per-task user stacks
/// (extend the user window in `boot.rs`) — that is M6d-adjacent and OUT of this lane; stop and hand it
/// to the integrator rather than growing the window here.
pub struct El0Demo {
    pub sp: u64,
    pub hello: u64,
    pub wild_write: u64,
    pub code_write: u64,
    pub stack_exec: u64,
    /// M6e preemption spinner (`__user_prog_spin`).
    pub spin: u64,
}

/// Copy the EL0 programs into the user window (`boot::user_region`) and do the I-cache maintenance;
/// return the demo entry points. Call once, after `mmu_init`. Does NOT protect the code page — the
/// caller warms the demo core's TLB first, then calls `protect()` (the copies here are exactly why
/// the page must still be EL1-writable). The window is identity-mapped, so entries are base + copy
/// offsets and each program's PC-relative `adr`s resolve in place.
///
/// M6c: two blobs share the ONE code page. The loaded `hello` program (`USER_BLOB`, out of kernel
/// `.text`) goes at offset 0 — the kernel enters it at the base — and the inline fault fixtures
/// (`__fault_blob_*`) go right after it. Both must fit in `USER_CODE_SIZE`.
pub fn setup() -> El0Demo {
    let (base, size) = super::boot::user_region();
    let hello_len = USER_BLOB.len();
    // 16-align the fixtures' start so their first instruction is 4-aligned (an eret/exec into a
    // misaligned entry is EC 0x22) and the icache maintenance below covers whole cache lines.
    let fault_off = (hello_len + 0xF) & !0xF;
    let fstart = &raw const __fault_blob_start as usize;
    let fend = &raw const __fault_blob_end as usize;
    let fault_len = fend - fstart;
    let total = fault_off + fault_len;
    // Everything must fit in the CODE page — the only page protect_user_code makes EL0-executable; a
    // program straddling into the data pages would abort mid-run.
    assert!(
        total <= super::boot::USER_CODE_SIZE,
        "user code (hello blob + fault fixtures) does not fit in the code page"
    );
    unsafe {
        // hello (the loaded blob) at the base; the inline fault fixtures at base + fault_off.
        core::ptr::copy_nonoverlapping(USER_BLOB.as_ptr(), base as *mut u8, hello_len);
        core::ptr::copy_nonoverlapping(
            fstart as *const u8,
            (base + fault_off as u64) as *mut u8,
            fault_len,
        );
    }
    // Freshly-written code: clean D to the PoU + invalidate the I-cache so the EL0 fetch (possibly on
    // another core — IC IVAU broadcasts Inner-Shareable) sees the new bytes across BOTH copies. This
    // is the DC CVAU/IC IVAU sequence M6a/M6b rely on; KEEP it for the M6c loaded-blob copy — it is
    // exactly what makes the copied program executable on real caches. Metal-only; QEMU no-op.
    super::cache::icache_sync_range(base as usize, total);
    serial_println!(":: M6c: user blob loaded ({} bytes) ::", hello_len);
    // An eret to a misaligned entry is EC 0x22 (PC alignment) — assert every entry came out
    // 4-aligned. Each fixture VA = base + fault_off + its offset within the fault blob.
    let fentry = |label: *const u8| -> u64 {
        let va = base + fault_off as u64 + (label as usize - fstart) as u64;
        assert!(va & 3 == 0, "user program entry misaligned");
        va
    };
    // `hello` enters at the copy's offset 0 (base). base is structurally 16 KiB-aligned (the region's
    // `#[repr(align(0x4000))]`), but assert it here too so it gets the same guard as the fixtures —
    // a future USER_REGION relocation can't silently produce a misaligned EL0 entry.
    assert!(base & 3 == 0, "hello entry misaligned");
    El0Demo {
        sp: (base + size as u64) & !0xF, // 16-aligned top of the window = initial user stack pointer
        hello: base, // the loaded blob's `_start` is at offset 0 of the copy (base is 16 KiB-aligned)
        wild_write: fentry(&raw const __user_prog_wild_write),
        code_write: fentry(&raw const __user_prog_code_write),
        stack_exec: fentry(&raw const __user_prog_stack_exec),
        spin: fentry(&raw const __user_prog_spin),
    }
}

/// M6b: deterministically WARM the demo core's TLB with the pre-protect (RW, XN) code-page mapping.
/// Runs as a kernel task pinned to the core that will run the EL0 demo, BEFORE `protect()`: the
/// volatile read walks the tables and caches the old descriptor in THIS core's TLB, so a broken
/// broadcast TLBI leaves a deterministic stale entry right where the demo executes — hello's first
/// EL0 fetch then dies through the stale UXN=1 (killed_unexpected -> FAIL) or code-write's store
/// sneaks through the stale RW (survivor exit(1) -> FAIL). Without this the demo core's TLB is cold
/// (only the BSP touches USER_REGION pre-protect: the blob copy) and a missing TLBI would pass
/// silently — QEMU can't test the TLBI at all (it re-walks), so the warm-up is what makes the METAL
/// run the real detector.
pub fn tlb_warm(_: usize) {
    let (base, _) = super::boot::user_region();
    // M6d: warm THIS core's TLB with the SHARED (ASID-0/boot-context) code-page mapping — the mapping the
    // M6b EL0 tasks (which run on the boot root) use. Since M6d a per-slot task may have left a slot root
    // live on this core; the shared user VA maps to a DIFFERENT (slot) frame under a slot root, so walking
    // it there would warm the wrong entry. Force the boot root live first (this is a kernel task, so
    // `dispatch_next` did no root switch), IRQ-masked so no preempt reswaps TTBR0 between the set and the
    // read. Leaving the boot root live is fine — the next dispatch installs the incoming task's root.
    unsafe {
        core::arch::asm!(
            "msr daifset, #2",
            "msr TTBR0_EL1, {boot}",
            "isb",
            boot = in(reg) super::boot::boot_ttbr0(),
            options(nostack, preserves_flags),
        );
        core::ptr::read_volatile(base as *const u8);
        core::arch::asm!("msr daifclr, #2", options(nostack, preserves_flags));
    }
    TLB_WARMED.store(true, Ordering::Release);
}

/// Flip the code page to its final EL0-RX/EL1-RO shape (`boot::protect_user_code`) and report the
/// BSP-side AT-probe verdicts. Call strictly AFTER `setup()` (the copy needs the page writable) and
/// after the demo core's TLB warm-up. A clean probe is best-effort evidence (AT may re-walk rather
/// than consult the TLB); a bad probe is always a real, loud failure.
pub fn protect() {
    let (base, _) = super::boot::user_region();
    let (el0_read_ok, el1_write_denied) =
        unsafe { super::boot::protect_user_code(base, super::boot::USER_CODE_SIZE) };
    if el0_read_ok && el1_write_denied {
        serial_println!(
            ":: M6b: user code page EL0-RX/EL1-RO (AT probe: EL0-read OK, EL1-write denied) ::"
        );
    } else {
        serial_println!(
            ":: M6b WARNING: protect probe unexpected (el0_read_ok={} el1_write_denied={}) — stale TLB after the TLBI? ::",
            el0_read_ok,
            el1_write_denied
        );
    }
}

/// M6b accounting: classify a killed task against the demo's EXPECTED faults. The verdict demands
/// the right (task, EC, FAR-page) triple, not just "it died": the stack page is BSS zeros and
/// 0x00000000 decodes as UDF, so with UXN accidentally unset stack-exec would still die (EC 0x00) —
/// count-only bookkeeping would false-PASS the very permission claim the test exists to prove.
/// Called from `aarch64_el0_fault_handler` before it exits the task.
pub fn record_el0_kill(name: &str, ec: u64, far: u64, far_valid: bool) {
    // M6d tasks (per-task address spaces) are NOT part of the M6b fault-isolation verdict. A kill among
    // them means a genuine per-slot ASID/permission bug — it must land in its OWN counter, never inflate
    // the M6b `killed_unexpected` count (which would masquerade as an M6b regression and hide the real
    // fault). Their missing SYS_REPORT already FAILs the M6d verdict line.
    if matches!(name, "el0-samevaA" | "el0-samevaB" | "el0-stackwrite" | "el0-spsentinel") {
        EL0_M6D_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // M6f fixtures likewise: a kill among them is a real bug (they must EFAULT-return, never fault) and
    // must land in its own counter, never inflating the M6b `killed_unexpected` count.
    if matches!(name, "el0-getinfo" | "el0-hostile" | "el0-yield" | "el0-sleep") {
        EL0_M6F_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    let (base, size) = super::boot::user_region();
    let code = super::boot::USER_CODE_SIZE as u64;
    let expected = far_valid
        && match name {
            // an EL0 write to PA 0x0 (EL1-only RAM): data abort, FAR in page 0 of the PA space
            "el0-wild-write" => ec == 0x24 && far >> 12 == 0,
            // an EL0 write to the (now read-only) code page: data abort, FAR in the code page
            "el0-code-write" => ec == 0x24 && far >= base && far < base + code,
            // an EL0 fetch from the UXN stack page: instruction abort, FAR in the data pages
            "el0-stack-exec" => ec == 0x20 && far >= base + code && far < base + size as u64,
            _ => false,
        };
    if expected {
        EL0_KILLED_EXPECTED.fetch_add(1, Ordering::AcqRel);
    } else {
        EL0_KILLED_UNEXPECTED.fetch_add(1, Ordering::AcqRel);
    }
}

/// M6b verdict task: wait (bounded) for all four M6b EL0 programs (hello + three fault fixtures) to
/// terminate, then print one PASS/FAIL line with the full accounting. Spawned on a DIFFERENT core than
/// the demo tasks so a wedged demo core (the fingerprint of a broken TLBI) still produces a verdict —
/// a timeout FAIL with the counts — instead of a silent half-dead boot. (The M6e spinner accounts
/// separately, via `EL0_SPIN_DONE`, so it does not perturb this verdict's `done >= 4`.) Time-bounded
/// via CNTPCT (which advances in QEMU even though the timer IRQ never fires there), not a yield count
/// (meaningless on a core with other work).
pub fn verdict(_: usize) {
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq(); // ~5 s; the whole demo completes in well under 1 s
    loop {
        let done = EL0_EXITED_OK.load(Ordering::Acquire)
            + EL0_EXITED_ERR.load(Ordering::Acquire)
            + EL0_KILLED_EXPECTED.load(Ordering::Acquire)
            + EL0_KILLED_UNEXPECTED.load(Ordering::Acquire);
        if done >= 4 || super::timer::cntpct().wrapping_sub(start) > deadline {
            break;
        }
        super::sched::yield_now();
    }
    let ok = EL0_EXITED_OK.load(Ordering::Acquire);
    let err = EL0_EXITED_ERR.load(Ordering::Acquire);
    let exp = EL0_KILLED_EXPECTED.load(Ordering::Acquire);
    let unexp = EL0_KILLED_UNEXPECTED.load(Ordering::Acquire);
    // The EXACT split, not the sum: hello killed (exited=0/killed=4), a survivor, or a wrong-EC
    // kill must all read FAIL — "every program terminated" is not the claim being proven.
    if ok == 1 && exp == 3 && err == 0 && unexp == 0 {
        serial_println!(
            ":: M6b: EL0 fault isolation — exited=1 killed=3 (all expected ECs), kernel alive -> PASS ::"
        );
    } else {
        serial_println!(
            ":: M6b: EL0 fault isolation FAIL — exited_ok={} survivor_exits={} killed_expected={} killed_unexpected={} (want 1/0/3/0) ::",
            ok,
            err,
            exp,
            unexp
        );
    }
}

/// M6e verdict task: wait (bounded, CNTPCT) for the preemption spinner to finish, then report whether
/// EL0 was actually preempted. Spawned like the M6b verdict on a scheduled core that co-tenants the
/// capstone workers, so it polls with `yield_now` (never monopolizes the core). The line is
/// deterministic under QEMU (the spinner completes its bounded loop -> completed=1; no timer IRQ ->
/// IRQs=0) and carries the metal-only signal in `IRQs`: on the real Pi 4 the timer (and any other SPI)
/// preempts running EL0 tasks, so `IRQs > 0` (demo-wide) — and the spinner STILL completes, which is
/// the distinct proof that SP_EL0 banking resumed it with the right user stack pointer. Time-bounded
/// via CNTPCT (advances in QEMU even without the timer IRQ), matching the M6b verdict.
pub fn m6e_verdict(_: usize) {
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq(); // ~5 s; the spinner finishes in well under 1 s either way
    while EL0_SPIN_DONE.load(Ordering::Acquire) == 0
        && super::timer::cntpct().wrapping_sub(start) <= deadline
    {
        super::sched::yield_now();
    }
    let done = EL0_SPIN_DONE.load(Ordering::Acquire);
    let irqs = EL0_IRQS_AT_EL0.load(Ordering::Relaxed);
    serial_println!(
        ":: M6e: EL0 preemptible — spinner completed={} IRQs-taken-at-EL0={} (metal: completed=1 & IRQs>0; QEMU: completed=1 & IRQs=0) ::",
        done,
        irqs
    );
}

/// The M6d demo's per-task entry points (all at the SAME user VAs — the point of ASID isolation) and the
/// per-task slot roots (`TTBR0` values from `boot::slot_ttbr0`). One shared initial SP_EL0 (each slot's
/// window has the same VA layout; only the frames differ).
pub struct M6dDemo {
    pub sp: u64,
    pub same_va: u64,
    pub stack_write: u64,
    pub sp_sentinel: u64,
    pub ttbr0_a: u64,
    pub ttbr0_b: u64,
    pub ttbr0_stack: u64,
    pub ttbr0_sp: u64,
}

/// M6d setup: allocate four private address-space slots, copy the M6d blob into each slot's code page
/// (through the slot backing's Global identity VA — never the EL0 window VA), plant each reader's
/// slot-private data sentinel, I-cache-sync, protect the code pages, and run the deterministic on-metal
/// nG detector. Emits the M6d setup line and returns the per-task entries + slot roots. Called once on the
/// BSP (which runs on the boot root) after the M6b/M6e demo. `None` if a slot allocation fails.
pub fn m6d_setup() -> Option<M6dDemo> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // shared initial SP_EL0 (top of the window, 16-aligned)
    let sent_off = size as u64 - 0x100; // the sentinel VA offset: EL0 reads [sp, #-0x100]

    // Blob bytes + per-fixture offsets (mirrors `setup`'s fault-fixture math).
    let bstart = &raw const __m6d_blob_start as usize;
    let bend = &raw const __m6d_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "M6d blob does not fit in a code page");
    let entry = |label: *const u8| -> u64 {
        let off = label as usize - bstart;
        let va = base + off as u64;
        assert!(va & 3 == 0, "M6d program entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };

    // Multi-alloc with partial-failure unwind (M6d review fold): the old four sequential `alloc_user_slot()?`
    // calls leaked earlier-claimed slots when a later one failed. `alloc_user_slots` releases what it got and
    // returns false on exhaustion, so a failed M6d setup frees the whole request.
    let mut slots = [0usize; 4];
    if !super::boot::alloc_user_slots(&mut slots) {
        return None;
    }
    let [slot_a, slot_b, slot_c, slot_d] = slots;

    // Copy the blob into each slot's code page (identity VA) + I-cache sync (DC CVAU/IC IVAU by the
    // identity VA; A72 caches are PIPT, so the code is fetchable at the aliased EL0 window VA).
    for &s in &slots {
        let backing = super::boot::slot_backing_ptr(s);
        unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
        super::cache::icache_sync_range(backing as usize, blen);
    }
    // Plant the readers' slot-private sentinels (page 3, [top-0x100]) via the identity VA. Pure data on a
    // PIPT D-cache — coherent with the EL0/probe read of the same frame at the window VA, no maintenance.
    unsafe {
        *(super::boot::slot_backing_ptr(slot_a).add(sent_off as usize) as *mut u64) = M6D_SENTINEL_A;
        *(super::boot::slot_backing_ptr(slot_b).add(sent_off as usize) as *mut u64) = M6D_SENTINEL_B;
        *(super::boot::slot_backing_ptr(slot_d).add(sent_off as usize) as *mut u64) = M6D_SENTINEL_SP;
    }
    // Protect every slot's code page (EL0-RX/EL1-RO). After this the code page is no longer EL1-writable.
    for &s in &slots {
        unsafe { super::boot::protect_user_slot_code(s, super::boot::USER_CODE_SIZE) };
    }
    // Deterministic on-metal nG detector (the arc's #1 metal risk): swap TTBR0 between slot A and B roots
    // reading the SAME VA — a global (nG=0) user leaf would resolve both to slot A's frame. QEMU re-walks
    // -> always PASS; metal caches -> a broken nG is caught. Folded into the same-VA PASS below.
    let probe_ok = unsafe {
        super::boot::probe_slot_isolation(slot_a, slot_b, sent_off, M6D_SENTINEL_A, M6D_SENTINEL_B)
    };
    M6D_PROBE_OK.store(probe_ok, Ordering::Release);

    serial_println!(
        ":: M6d: per-task address spaces (8 slots, ASID 1-8, nG user / global kernel) ::"
    );

    Some(M6dDemo {
        sp,
        same_va: entry(&raw const __m6d_prog_same_va),
        stack_write: entry(&raw const __m6d_prog_stack_write),
        sp_sentinel: entry(&raw const __m6d_prog_sp_sentinel),
        ttbr0_a: super::boot::slot_ttbr0(slot_a),
        ttbr0_b: super::boot::slot_ttbr0(slot_b),
        ttbr0_stack: super::boot::slot_ttbr0(slot_c),
        ttbr0_sp: super::boot::slot_ttbr0(slot_d),
    })
}

/// M6d verdict task: wait (bounded, CNTPCT) for the four M6d tasks to finish, then print the three PASS/
/// FAIL lines. Spawned on a sibling core like the M6b/M6e verdicts. Isolation is proven by `same_va` (two
/// tasks reading distinct slot-private sentinels at the SAME VA) PLUS the deterministic kernel probe;
/// `stack_write` and `sp_sentinel` are path-liveness checks (the stack is writable; SP_EL0 addresses the
/// slot after preemption). A killed M6d task never reports, so its line FAILs (bounded by the deadline).
pub fn m6d_verdict(_: usize) {
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq(); // ~5 s; the whole demo completes well under 1 s
    while EL0_M6D_DONE.load(Ordering::Acquire) < 4
        && super::timer::cntpct().wrapping_sub(start) <= deadline
    {
        super::sched::yield_now();
    }
    let a = M6D_REPORT_A.load(Ordering::Acquire);
    let b = M6D_REPORT_B.load(Ordering::Acquire);
    let st = M6D_REPORT_STACK.load(Ordering::Acquire);
    let spv = M6D_REPORT_SP.load(Ordering::Acquire);
    let probe = M6D_PROBE_OK.load(Ordering::Acquire);
    let killed = EL0_M6D_KILLED.load(Ordering::Acquire);

    // same-VA isolation: each task read its OWN slot's sentinel at the same VA; distinct + each == planted
    // + the deterministic kernel probe agreed (nG is real on metal). The full triple, never bare distinctness.
    if a == M6D_SENTINEL_A && b == M6D_SENTINEL_B && a != b && probe {
        serial_println!(":: M6d: same-VA isolation A={:#x} B={:#x} distinct -> PASS ::", a, b);
    } else {
        serial_println!(
            ":: M6d: same-VA isolation A={:#x} B={:#x} probe={} killed={} -> FAIL ::",
            a, b, probe, killed
        );
    }
    if st == M6D_STACK_PATTERN {
        serial_println!(":: M6d: EL0 stack write/readback -> PASS ::");
    } else {
        serial_println!(":: M6d: EL0 stack write/readback (got {:#x}) -> FAIL ::", st);
    }
    if spv == M6D_SENTINEL_SP {
        serial_println!(":: M6d: SP-relative sentinel readback -> PASS ::");
    } else {
        serial_println!(
            ":: M6d: SP-relative sentinel readback (got {:#x} want {:#x}) -> FAIL ::",
            spv, M6D_SENTINEL_SP
        );
    }
}

/// The M6f demo's per-fixture entry points (EL0 VAs inside each slot's code page) + the per-fixture slot
/// roots (`TTBR0` from `boot::slot_ttbr0`). One shared initial SP_EL0 (every slot's window has the same VA
/// layout; only the frames differ). Each fixture runs on its OWN private slot because the getinfo fixture
/// WRITES its stack (copy_to_user target) — forbidden on the shared window by the M6e stack STOP tripwire.
pub struct M6fDemo {
    pub sp: u64,
    pub getinfo: u64,
    pub hostile: u64,
    pub yield_prog: u64,
    pub sleep_prog: u64,
    pub ttbr0_getinfo: u64,
    pub ttbr0_hostile: u64,
    pub ttbr0_yield: u64,
    pub ttbr0_sleep: u64,
}

/// M6f setup: allocate four private slots (via the unwinding `alloc_user_slots`), copy the M6f blob into
/// each slot's code page (through the Global identity backing VA, never the EL0 window VA), I-cache-sync,
/// and protect the code pages. Emits the M6f setup line; returns the per-fixture entries + slot roots.
/// Called once on the BSP after the M6d demo. `None` if slot allocation fails (the whole request is
/// released, not leaked). Plants no sentinel — the getinfo fixture writes its own struct via copy_to_user.
pub fn m6f_setup() -> Option<M6fDemo> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // shared initial SP_EL0 (16-aligned top of the window)

    let bstart = &raw const __m6f_blob_start as usize;
    let bend = &raw const __m6f_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "M6f blob does not fit in a code page");
    let entry = |label: *const u8| -> u64 {
        let off = label as usize - bstart;
        let va = base + off as u64;
        assert!(va & 3 == 0, "M6f program entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };

    let mut slots = [0usize; 4];
    if !super::boot::alloc_user_slots(&mut slots) {
        return None;
    }
    // Copy the blob into each slot's code page (identity VA) + I-cache sync (DC CVAU/IC IVAU by the identity
    // VA; A72 caches are PIPT, so the code is fetchable at the aliased EL0 window VA), then protect it.
    for &s in &slots {
        let backing = super::boot::slot_backing_ptr(s);
        unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
        super::cache::icache_sync_range(backing as usize, blen);
    }
    for &s in &slots {
        unsafe { super::boot::protect_user_slot_code(s, super::boot::USER_CODE_SIZE) };
    }

    serial_println!(
        ":: M6f: validated user pointers — copy_from_user/copy_to_user + syscall surface (4 EL0 fixtures) ::"
    );

    Some(M6fDemo {
        sp,
        getinfo: entry(&raw const __m6f_prog_getinfo),
        hostile: entry(&raw const __m6f_prog_hostile),
        yield_prog: entry(&raw const __m6f_prog_yield),
        sleep_prog: entry(&raw const __m6f_prog_sleep),
        ttbr0_getinfo: super::boot::slot_ttbr0(slots[0]),
        ttbr0_hostile: super::boot::slot_ttbr0(slots[1]),
        ttbr0_yield: super::boot::slot_ttbr0(slots[2]),
        ttbr0_sleep: super::boot::slot_ttbr0(slots[3]),
    })
}

/// M6f verdict task: wait (bounded, CNTPCT) for the four M6f fixtures to exit, then print the three PASS/
/// FAIL lines + the per-task EL0 preempt breakdown (Part 0 fold #5). Spawned on a sibling core like the
/// other verdicts. Lines: (1) getinfo/copy_to_user round-trip — the witness is non-zero iff the pid read
/// back from the struct copy_to_user wrote equalled SYS_GETPID; (2) 4 hostile pointers refused (EFAULT), 0
/// kills — the hostile fixture counted 4 EFAULT returns and was NOT killed (a kill, or a kernel halt from a
/// stray store, would have prevented the report); (3) yield/sleep interleave — both fixtures completed all
/// iterations AND the kernel observed > 0 runner switches between them. The preempt line is QEMU-0 /
/// metal->0, so the next reflash reads exact per-slot-task preemption (the M6d ledger's aggregate refined).
pub fn m6f_verdict(_: usize) {
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq(); // ~5 s; the whole demo completes well under 1 s
    while EL0_M6F_DONE.load(Ordering::Acquire) < 4
        && super::timer::cntpct().wrapping_sub(start) <= deadline
    {
        super::sched::yield_now();
    }
    let getinfo = M6F_GETINFO_WITNESS.load(Ordering::Acquire);
    let hostile = M6F_HOSTILE_REFUSED.load(Ordering::Acquire);
    let ydone = M6F_YIELD_DONE.load(Ordering::Acquire);
    let sdone = M6F_SLEEP_DONE.load(Ordering::Acquire);
    let switches = M6F_INTERLEAVE_SWITCHES.load(Ordering::Acquire);
    let killed = EL0_M6F_KILLED.load(Ordering::Acquire);

    if getinfo != 0 && killed == 0 {
        serial_println!(":: M6f: getinfo/copy_to_user round-trip -> PASS ::");
    } else {
        serial_println!(
            ":: M6f: getinfo/copy_to_user round-trip (witness={:#x} killed={}) -> FAIL ::",
            getinfo, killed
        );
    }
    if hostile == 4 && killed == 0 {
        serial_println!(":: M6f: 4 hostile pointers refused (EFAULT), 0 kills -> PASS ::");
    } else {
        serial_println!(
            ":: M6f: hostile pointers refused={} killed={} (want 4/0) -> FAIL ::",
            hostile, killed
        );
    }
    if ydone == M6F_ITERS && sdone == M6F_ITERS && switches > 0 {
        serial_println!(":: M6f: yield/sleep interleave -> PASS ::");
    } else {
        serial_println!(
            ":: M6f: yield/sleep interleave (yield={} sleep={} switches={}) -> FAIL ::",
            ydone, sdone, switches
        );
    }
    // Per-task EL0 preempt breakdown (Part 0 fold #5): the exact per-slot-task attribution the M6d ledger's
    // aggregate `IRQs-taken-at-EL0` lacked. QEMU: all 0 (no timer IRQ). Metal: > 0 for the tasks a tick caught.
    serial_println!(
        ":: M6f: per-task EL0 preempts — samevaA={} samevaB={} stackwrite={} spsentinel={} yield={} sleep={} (metal >0; QEMU 0) ::",
        PRE_SAMEVA_A.load(Ordering::Relaxed),
        PRE_SAMEVA_B.load(Ordering::Relaxed),
        PRE_STACKWRITE.load(Ordering::Relaxed),
        PRE_SPSENTINEL.load(Ordering::Relaxed),
        PRE_YIELD.load(Ordering::Relaxed),
        PRE_SLEEP.load(Ordering::Relaxed),
    );
}

/// One-shot: the first syscall proves the EL0→EL1 path is live end to end (logged off the ISR-free SVC
/// path, so `serial_println!` is safe here — unlike the RX ISR, nothing on this core holds SERIAL_PORT).
static SVC_LOGGED: AtomicBool = AtomicBool::new(false);

/// SVC dispatcher, called from the `__vec_svc` stub with a pointer to the saved GPR frame (SAVE_GPRS
/// layout: register x{i} is at byte 8*i, so x0 at frame+0, x8 at frame+64). Reads x8 = number and
/// x0–x5 = args, writes the return value into the x0 slot. Runs at EL1 on the faulting task's own
/// kernel stack with IRQ masked (exception entry masks it), so a blocking/exiting syscall may safely
/// `switch_context`, exactly like `timer_preempt` from `__vec_irq`.
#[unsafe(no_mangle)]
extern "C" fn aarch64_svc_handler(frame: *mut u64) {
    let nr = unsafe { *frame.add(8) }; // x8
    let a0 = unsafe { *frame.add(0) }; // x0
    let a1 = unsafe { *frame.add(1) }; // x1
    let a2 = unsafe { *frame.add(2) }; // x2

    if !SVC_LOGGED.swap(true, Ordering::Relaxed) {
        serial_println!(":: SVC: EC=0x15 nr={} — EL0->EL1 syscall path live ::", nr);
    }

    let ret: i64 = match nr {
        SYS_WRITE => sys_write(a0, a1, a2),
        SYS_REPORT => {
            // Route by the reporting task's name: M6d names land in m6d_report, M6f names in m6f_report;
            // each ignores the other's names, so calling both is safe and additive.
            m6d_report(a0);
            m6f_report(a0);
            0
        }
        SYS_YIELD => sys_yield(),
        SYS_SLEEP_MS => sys_sleep_ms(a0),
        SYS_GETPID => super::sched::current_id().map(|id| id as i64).unwrap_or(-1),
        SYS_GETINFO => sys_getinfo(a0),
        SYS_EXIT => {
            // Demo accounting BEFORE the no-return exit. The sentinel statuses are routed to their own
            // counters so the M6b (`exited=1 killed=3`) and M6e (`completed=1`) verdicts stay byte-
            // identical: M6E_SPIN_STATUS -> EL0_SPIN_DONE, M6D_EXIT_STATUS -> EL0_M6D_DONE, M6F_EXIT_STATUS
            // -> EL0_M6F_DONE. All three sentinel arms MUST precede the catch-all `else` (a mis-ordered
            // sentinel exit would land in EL0_EXITED_ERR and FAIL the M6b verdict). Otherwise: status 0 =
            // normal completion (hello); nonzero = a fault-test program self-reporting that its intended
            // fault never happened (survivor protocol).
            if a0 == M6E_SPIN_STATUS {
                EL0_SPIN_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == M6D_EXIT_STATUS {
                EL0_M6D_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == M6F_EXIT_STATUS {
                EL0_M6F_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == 0 {
                EL0_EXITED_OK.fetch_add(1, Ordering::AcqRel);
            } else {
                EL0_EXITED_ERR.fetch_add(1, Ordering::AcqRel);
            }
            super::sched::exit() // never returns; the __vec_svc eret tail is not reached
        }
        _ => -38, // -ENOSYS
    };
    unsafe { *frame.add(0) = ret as u64 }; // return value in x0
}

// =============================================================================================
// M6f: validated user-pointer copies (copy_from_user / copy_to_user) + the wider syscall surface
// =============================================================================================

/// The single error `copy_from_user`/`copy_to_user` return: a user pointer/length failed validation
/// (outside the task's window, a wrapping range, or — to-user only — the read-only code page). Mapped to
/// `-EFAULT` (`EFAULT`) at the syscall boundary. A bad pointer ARG is an error RETURN, never a task-kill:
/// kills are reserved for faults the HARDWARE raises (M6b), not a syscall arg the kernel can reject cheaply.
pub struct Efault;

/// `-EFAULT`, the errno a rejected user pointer returns to EL0.
const EFAULT: i64 = -14;

/// Validate that `[user_va, user_va+len)` lies entirely inside the calling task's EL0 window. `writable`
/// additionally excludes the read-only CODE page (page 0, `[base, base+USER_CODE_SIZE)`), so `copy_to_user`
/// refuses a write aimed there (an EL1 store to an AP=0b11 page Permission-faults -> the kernel-fault path
/// halts the core; we reject BEFORE any deref instead of taking that fault). Checks, in order: `len == 0`
/// is handled by the callers' fast path; `checked_add` rejects a length that wraps; the range must sit
/// fully in `[lo, base+size)`. A syscall executes with the caller's TTBR0/ASID live (M6d), so a user VA in
/// this window can only reach that task's OWN frames — validation + that guarantee is the PAN-less software
/// discipline (A72 is Armv8.0, no FEAT_PAN; on a PAN-capable port this must become an LDTR/unprivileged copy).
fn user_range_ok(user_va: u64, len: usize, writable: bool) -> bool {
    let (base, size) = super::boot::user_region();
    let Some(end) = user_va.checked_add(len as u64) else {
        return false; // length wraps the address space
    };
    let lo = if writable { base + super::boot::USER_CODE_SIZE as u64 } else { base };
    user_va >= lo && end <= base + size as u64
}

/// Copy `len` bytes from the EL0 buffer at `user_va` into `kdst`, after validating the whole SOURCE range
/// is inside the caller's user window. Never dereferences the pointer until all checks pass. `Err(Efault)`
/// on a bad pointer/length. Factored out of the M6b SYS_WRITE bound-check; `kdst.len() >= len` is a
/// kernel-side contract (debug-asserted).
pub fn copy_from_user(kdst: &mut [u8], user_va: u64, len: usize) -> Result<(), Efault> {
    if len == 0 {
        return Ok(());
    }
    debug_assert!(kdst.len() >= len, "copy_from_user: kdst smaller than len (kernel bug)");
    if !user_range_ok(user_va, len, false) {
        return Err(Efault);
    }
    // SAFETY: range validated inside the user window; the syscall runs with the caller's TTBR0 live, so the
    // VA resolves to the caller's own frames, readable at EL1 (AP=0b01/0b11) on this PAN-less A72.
    unsafe { core::ptr::copy_nonoverlapping(user_va as *const u8, kdst.as_mut_ptr(), len) };
    Ok(())
}

/// Copy `len` bytes from `ksrc` to the EL0 buffer at `user_va`, after validating the whole DESTINATION
/// range is inside the caller's WRITABLE user window (the RO code page is excluded, so a write aimed there
/// is refused with `Efault`, never a faulting EL1 store). The to-user twin of `copy_from_user`.
pub fn copy_to_user(user_va: u64, ksrc: &[u8], len: usize) -> Result<(), Efault> {
    if len == 0 {
        return Ok(());
    }
    debug_assert!(ksrc.len() >= len, "copy_to_user: ksrc smaller than len (kernel bug)");
    if !user_range_ok(user_va, len, true) {
        return Err(Efault);
    }
    // SAFETY: range validated inside the writable user window (code page excluded); caller's TTBR0 live.
    unsafe { core::ptr::copy_nonoverlapping(ksrc.as_ptr(), user_va as *mut u8, len) };
    Ok(())
}

/// SYS_WRITE(fd, buf, len): write `len` bytes from the EL0 buffer to the serial console; returns the count,
/// or a negative errno. Routed through `copy_from_user` (M6f): validate the WHOLE range up front so a
/// hostile pointer yields `-EFAULT` with NO partial output (byte-identical to the pre-M6f all-or-nothing
/// behaviour), then stream to the console in bounded stack chunks THROUGH the validated copy primitive.
fn sys_write(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != 1 {
        return -9; // -EBADF (only stdout for M6a/M6b)
    }
    let total = len as usize;
    if !user_range_ok(buf, total, false) {
        return EFAULT; // reject before ANY output (matches the old all-or-nothing semantics)
    }
    let mut chunk = [0u8; 256];
    let mut off = 0usize;
    // Byte loop (not fmt) keeps the syscall path FP-light and handles non-UTF-8 bytes. Held IRQ-masked at
    // EL1 (exception entry), so the SERIAL_PORT lock can't be re-entered by an interrupt on this core;
    // copy_from_user does a plain memcpy (no serial, no block) under the lock.
    let port = super::serial::SERIAL_PORT.lock();
    while off < total {
        let n = core::cmp::min(chunk.len(), total - off);
        // A subrange of the already-validated range, so copy_from_user's re-check always passes here.
        if copy_from_user(&mut chunk[..n], buf + off as u64, n).is_err() {
            return EFAULT;
        }
        for &b in &chunk[..n] {
            port.write_byte(b);
        }
        off += n;
    }
    len as i64
}

/// The fixed struct SYS_GETINFO writes to EL0. `#[repr(C)]` so the byte layout is stable for the user
/// program that reads it back: `pid` at offset 0, `ticks` at offset 8 (16 bytes total).
#[repr(C)]
struct UserInfo {
    pid: u64,
    ticks: u64,
}

/// SYS_GETINFO(user_ptr): write a small fixed {pid, ticks} struct to the caller's buffer via
/// `copy_to_user` — the to-user direction's exerciser. Returns 0, or `-EFAULT` if the pointer/length fails
/// validation (e.g. aimed at the RO code page) — an error RETURN, never a task-kill.
fn sys_getinfo(user_ptr: u64) -> i64 {
    let info = UserInfo {
        pid: super::sched::current_id().unwrap_or(0),
        ticks: super::timer::ticks(),
    };
    // SAFETY: view `info` as its raw bytes for the copy; `UserInfo` is `#[repr(C)]` plain-old-data.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &info as *const UserInfo as *const u8,
            core::mem::size_of::<UserInfo>(),
        )
    };
    match copy_to_user(user_ptr, bytes, bytes.len()) {
        Ok(()) => 0,
        Err(Efault) => EFAULT,
    }
}

/// SYS_YIELD: cooperatively give up the CPU — thin over `sched::yield_now()`. `yield_now` unmasks IRQ on
/// return, but the `__vec_svc` epilogue that runs after this handler restores per-core banked
/// ELR/SPSR/SP_EL0 and MUST be I-masked, so re-mask before returning (see `remask_irq`). Records one
/// interleave observation for the M6f yield/sleep witness. Returns 0.
fn sys_yield() -> i64 {
    note_interleave();
    super::sched::yield_now();
    remask_irq();
    0
}

/// SYS_SLEEP_MS(ms): block the calling EL0 task ~`ms` milliseconds — thin over `sched::sleep_ticks`
/// (ms→ticks at the 250 Hz per-core tick, rounding UP so a sub-tick sleep still waits >= 1 tick; M6f adds
/// no scheduler primitive). QEMU delivers no timer IRQ, so `sleep_ticks` (whose only waker is the tick)
/// would park the task FOREVER; when the timer is not live, fall back to a cooperative `yield_now` — the
/// same guard `input_service`/`rx_backstop` use — so the interleave demo makes progress and the regression
/// never hangs. The real timed sleep rides along on metal. Both `sleep_ticks` and `yield_now` unmask IRQ,
/// so re-mask before returning to the I-masked `__vec_svc` epilogue. Returns 0.
fn sys_sleep_ms(ms: u64) -> i64 {
    /// The scheduler tick rate; mirrors `timer::TICK_HZ` (private there). Only used for the ms→ticks
    /// conversion — no timer register is touched (the STOP tripwire on timer timing stands).
    const TICK_HZ: u64 = 250;
    let ticks = (ms.saturating_mul(TICK_HZ) + 999) / 1000; // round up
    note_interleave();
    if super::timer::is_live() {
        super::sched::sleep_ticks(ticks);
    } else {
        super::sched::yield_now();
    }
    remask_irq();
    0
}

/// Re-mask IRQ (set PSTATE.I). `yield_now`/`sleep_ticks` unmask on return, but the `__vec_svc` epilogue
/// after this handler restores the per-core banked ELR_EL1/SPSR_EL1/SP_EL0 and MUST be I-masked — a nested
/// IRQ between those `msr`s and the `eret` would re-bank them and corrupt the EL0 return (the same
/// invariant the `__vec_irq` epilogue documents). Exception entry masks DAIF, so the handler is entered
/// I-masked; the two syscalls that unmask (via a scheduler switch) re-mask here before returning.
#[inline]
fn remask_irq() {
    unsafe { core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags)) };
}

/// M6f: record one yield/sleep interleave observation. Called from the SYS_YIELD/SYS_SLEEP_MS handlers
/// with the reporting task current; the two interleave fixtures run on one core, so a change of runner
/// since the previous yielding syscall is one observed switch (`M6F_INTERLEAVE_SWITCHES > 0` proves both
/// ran and the scheduler passed control back and forth). Only the two named M6f interleave tasks
/// participate (kernel `yield_now` callers don't come through the syscall path; other EL0 tasks aren't
/// named these). Under QEMU the interleave is purely the SYS_YIELD round-robin; on metal the timer also
/// preempts them.
fn note_interleave() {
    let tag = match super::sched::current_name() {
        Some("el0-yield") => 1u32,
        Some("el0-sleep") => 2u32,
        _ => return,
    };
    let last = M6F_INTERLEAVE_LAST.swap(tag, Ordering::AcqRel);
    if last != 0 && last != tag {
        M6F_INTERLEAVE_SWITCHES.fetch_add(1, Ordering::AcqRel);
    }
}

/// M6f: record a value an M6f EL0 fixture reported via SYS_REPORT, keyed by the reporting task's name.
/// (M6d names fall through to `m6d_report`, which the SYS_REPORT arm also calls; the name spaces are
/// disjoint, so each function ignores the other's tasks.)
fn m6f_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-getinfo") => M6F_GETINFO_WITNESS.store(value, Ordering::Release),
        Some("el0-hostile") => M6F_HOSTILE_REFUSED.store(value as u32, Ordering::Release),
        Some("el0-yield") => M6F_YIELD_DONE.store(value as u32, Ordering::Release),
        Some("el0-sleep") => M6F_SLEEP_DONE.store(value as u32, Ordering::Release),
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}
