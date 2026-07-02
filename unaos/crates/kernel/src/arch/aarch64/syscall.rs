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

// --- Syscall numbers (a tiny subset for M6a/M6b; SYS_REPORT added for the M6d demo). ---
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
/// M6d demo: report a u64 value to the kernel, keyed by the calling task's name (see `m6d_report`).
/// Demo-only accounting channel — a real OS would not have this; it lets an EL0 program hand the kernel
/// the value it read from its own (slot-private) address space so the verdict can check isolation.
const SYS_REPORT: u64 = 3;

/// M6e demo: the sentinel `sys_exit` status the preemption spinner uses so its exit is accounted to
/// `EL0_SPIN_DONE` and never perturbs the M6b `exited/killed` counters. Demo-only — there is no real
/// userspace yet, so overloading one status value for demo bookkeeping is safe and documented here.
const M6E_SPIN_STATUS: u64 = 0x6E;
/// M6d demo: the sentinel `sys_exit` status every M6d task uses so its exit lands in `EL0_M6D_DONE` and
/// never touches the M6b (`EL0_EXITED_OK/ERR`) or M6e (`EL0_SPIN_DONE`) counters — keeping those verdicts
/// byte-identical. The SYS_EXIT dispatch MUST test this BEFORE the catch-all `else` (see the handler).
const M6D_EXIT_STATUS: u64 = 0x6D;

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

    let slot_a = super::boot::alloc_user_slot()?;
    let slot_b = super::boot::alloc_user_slot()?;
    let slot_c = super::boot::alloc_user_slot()?;
    let slot_d = super::boot::alloc_user_slot()?;
    let slots = [slot_a, slot_b, slot_c, slot_d];

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
            m6d_report(a0);
            0
        }
        SYS_EXIT => {
            // Demo accounting BEFORE the no-return exit. The sentinel statuses are routed to their own
            // counters so the M6b (`exited=1 killed=3`) and M6e (`completed=1`) verdicts stay byte-
            // identical: M6E_SPIN_STATUS -> EL0_SPIN_DONE, M6D_EXIT_STATUS -> EL0_M6D_DONE. Both sentinel
            // arms MUST precede the catch-all `else` (a mis-ordered M6d exit would land in EL0_EXITED_ERR
            // and FAIL the M6b verdict). Otherwise: status 0 = normal completion (hello); nonzero = a
            // fault-test program self-reporting that its intended fault never happened (survivor protocol).
            if a0 == M6E_SPIN_STATUS {
                EL0_SPIN_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == M6D_EXIT_STATUS {
                EL0_M6D_DONE.fetch_add(1, Ordering::AcqRel);
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

/// SYS_WRITE(fd, buf, len): write `len` bytes from the EL0 buffer to the serial console; returns the
/// count, or a negative errno.
///
/// The buffer pointer is an EL0 VA == PA == the EL1 identity VA, so the kernel can read it directly at
/// EL1 — BUT it is UNTRUSTED, so it is bound-checked against USER_REGION before the deref: an EL0
/// caller must not be able to point `buf` at kernel RAM (exfiltration out the serial port), at the
/// Device window (a side-effecting EL1 MMIO read), or at unmapped memory (an EL1 abort that
/// `aarch64_fault_handler` would turn into a core halt). Full copy_from_user is M6f; this closes both
/// holes cheaply. NOTE (A72/Armv8.0): the direct EL1 read of an EL0-accessible (AP=0b01 or 0b11)
/// page is permitted because this core lacks FEAT_PAN; on a PAN-capable core (the Jetson A78 port)
/// this must become an unprivileged load (LDTR) or a validated copy first, else it Permission-faults
/// (EC 0x25).
fn sys_write(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != 1 {
        return -9; // -EBADF (only stdout for M6a/M6b)
    }
    let (base, size) = super::boot::user_region();
    let end = buf.wrapping_add(len);
    // Reject overflow and any range not fully inside USER_REGION.
    if end < buf || buf < base || end > base + size as u64 {
        return -14; // -EFAULT
    }
    let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) };
    // Byte loop (not fmt) keeps the syscall path FP-light and handles non-UTF-8 bytes. Held IRQ-masked
    // at EL1, so the SERIAL_PORT lock can't be re-entered by an interrupt on this core.
    let port = super::serial::SERIAL_PORT.lock();
    for &b in bytes {
        port.write_byte(b);
    }
    len as i64
}
