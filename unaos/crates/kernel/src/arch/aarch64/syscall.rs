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

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering};

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
/// M7/U4: load the fixed on-disk program (`HELLO.BIN`) into a fresh per-task slot, run it at EL0 as a CHILD,
/// and return a HANDLE index into the CALLER's per-process handle table (U4 — not the raw pid), or a negative
/// errno. No args this arc — arbitrary program-by-name is M8 (it needs a validated `copy_from_user` name).
/// See `sys_spawn`.
const SYS_SPAWN: u64 = 8;
/// M7/U4: block the caller until the child referred to by the HANDLE in `a0` exits, then return its exit
/// status (or -ECHILD if that handle is not in the caller's table — structural ownership). Woken by the
/// child's `done.post()` — a scheduler wake, so it works under QEMU. See `sys_wait`.
const SYS_WAIT: u64 = 9;
/// U5: operate on the caller's OWN handle table as capabilities. `a0` selects the sub-op
/// (`CAP_OP_GRANT`/`CAP_OP_REVOKE`); the remaining args are op-specific (see `sys_cap`). GRANT mints a new,
/// rights-attenuated handle to the same target as a source handle the caller holds `CAP_GRANT` on; REVOKE
/// clears a handle the caller owns. The enforcement layer sits at the handle lookup (`handle_resolve`).
const SYS_CAP: u64 = 10;
/// `SYS_CAP` sub-ops (in `a0`). GRANT: `a1`=source handle idx, `a2`=requested rights mask -> new handle idx
/// (attenuated) or a negative errno. REVOKE: `a1`=handle idx to drop -> 0 or a negative errno.
const CAP_OP_GRANT: u64 = 0;
const CAP_OP_REVOKE: u64 = 1;

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
/// U4 demo: the sentinel `sys_exit` status the process-model fixtures (`el0-u4parent`, `el0-u4orphan`) use so
/// THEIR exits land in `EL0_U4_DONE`, never perturbing the M6b/M6d/M6e/M6f/M6g counters (same sentinel
/// discipline). A spawned CHILD is reaped through the Proc table by pid (see the SYS_EXIT arm), not by this
/// status. Fresh value (the retired M7 demo used `0x77`); distinct from the M6D/M6E/M6F sentinels and 0.
const U4_EXIT_STATUS: u64 = 0x74;
/// U4 demo: the nonzero WITNESS token the parent reports iff it reaped BOTH children by handle with status 0.
/// A token (not a pid) — `sys_spawn` now returns a handle, so the verdict only needs non-zero-means-both-ok.
/// Must match `movz x23, #0xC4` in `__u4_prog_parent`; `u4_launcher` only checks it is non-zero.
const U4_WITNESS_TOKEN: u64 = 0xC4;
/// U4: the exit status the child-KILL path stores into a child's Proc entry so a killed child still wakes
/// its parent's `sys_wait` (rather than hanging it) — non-zero so the parent's witness computes 0 (a killed
/// child is a FAIL). A normal child exits with its own status (0 for `HELLO.BIN`); this is used only on a kill.
const U4_KILLED_STATUS: i32 = 0x4B; // 'K'
/// U5 demo: the sentinel `sys_exit` status the capability fixture (`el0-u5cap`) uses so ITS exit lands in
/// `EL0_U5_DONE`, never perturbing the M6b/M6d/M6e/M6f/M6g/U4 counters (same sentinel discipline). Distinct
/// from every prior sentinel (0x6D/0x6E/0x6F/0x74) and 0. Tested BEFORE the catch-all `else` in SYS_EXIT.
const U5_EXIT_STATUS: u64 = 0x75;
/// U5 demo: the witness bitmask the capability fixture reports, one bit per proven behaviour — write-cap OK
/// (bit0), no-cap `-EACCES` (bit1), attenuated grant bounded + subset grant works (bit2), revoke enforced
/// (bit3). `u5_launcher` PASSes iff the fixture reports exactly `U5_WITNESS_ALL` (all four). Must match the
/// `add x23, x23, #{1,2,4,8}` steps in `__u5_prog_cap`.
const U5_WITNESS_ALL: u64 = 0xF;

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

// --- U4 inline EL0 fixtures (per-process handle table). ONE blob with TWO fixtures — the PARENT and the
// ownership NEGATIVE (the orphan) — copied into each fixture's own slot; each task enters at its own offset
// (the M6d/M6f multi-fixture-blob shape). Both are position-independent, register-only (write no user stack,
// so they are safe on any slot under preemption). ABI: x8=nr, args x0-x2, ret x0.
//
// PARENT (`el0-u4parent`): the U4 capability — a spawner reaps MULTIPLE children BY HANDLE. `SYS_SPAWN` now
// returns a small HANDLE index into the caller's per-process handle table (not a raw pid); `SYS_WAIT` takes
// that handle. Two spawns (two handles in x19/x20), two waits (two statuses in x21/x22), then a WITNESS
// (a nonzero token iff both handles were valid — sign bit clear — AND both children exited status 0, else 0),
// and `sys_exit(U4_EXIT_STATUS)` (`0x74` -> EL0_U4_DONE, off every prior counter).
//
// ORPHAN (`el0-u4orphan`): the ownership NEGATIVE — it spawned nothing, so handle #0 is Empty in ITS OWN
// per-process table; `sys_wait(0)` must therefore return `-ECHILD` (-10). It reports 1 iff it saw exactly
// -ECHILD (structural ownership: a task cannot reap a child whose handle is not in its table), else 0, then
// exits with the same sentinel. Deterministic — needs no cross-fixture pid plumbing (its table is empty).
core::arch::global_asm!(
    r#"
    .globl __u4_blob_start
__u4_blob_start:
    .balign 4
    .globl __u4_prog_parent
__u4_prog_parent:
    mov  x8, #8                            // SYS_SPAWN -> handle_a (a handle index >=0, or a negative errno)
    svc  #0
    mov  x19, x0                           // x19 = handle_a
    mov  x8, #8                            // SYS_SPAWN -> handle_b (a SECOND child, a SECOND handle)
    svc  #0
    mov  x20, x0                           // x20 = handle_b
    mov  x0, x19                           // SYS_WAIT(handle_a) — blocks until child A exits (scheduler wake)
    mov  x8, #9
    svc  #0
    mov  x21, x0                           // x21 = status_a
    mov  x0, x20                           // SYS_WAIT(handle_b) — reap child B by its handle
    mov  x8, #9
    svc  #0
    mov  x22, x0                           // x22 = status_b
    mov  x23, xzr                          // witness = 0
    tbnz x19, #63, 1f                      // handle_a < 0 (spawn A failed) -> witness stays 0
    tbnz x20, #63, 1f                      // handle_b < 0 (spawn B failed) -> witness stays 0
    cbnz x21, 1f                           // status_a != 0 (child A not clean) -> witness stays 0
    cbnz x22, 1f                           // status_b != 0 (child B not clean) -> witness stays 0
    movz x23, #0xC4                        // all four OK -> witness = U4_WITNESS_TOKEN (nonzero)
1:  mov  x0, x23                           // SYS_REPORT(witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U4_EXIT_STATUS) -> EL0_U4_DONE
    movz x0, #0x74
    svc  #0
2:  b 2b                                   // sys_exit never returns; belt-and-braces guard

    // The ownership negative: sys_wait a handle it never installed.
    .balign 4
    .globl __u4_prog_orphan
__u4_prog_orphan:
    mov  x0, #0                            // SYS_WAIT(handle #0) — Empty in its OWN never-spawned table
    mov  x8, #9
    svc  #0
    mov  x1, xzr                           // report = 0
    cmn  x0, #10                           // x0 == -ECHILD (-10)?  (x0 + 10 == 0 -> Z)
    cinc x1, x1, eq                        // saw -ECHILD -> report = 1 (structural ownership enforced)
    mov  x0, x1                            // SYS_REPORT(1 iff -ECHILD, else 0)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U4_EXIT_STATUS) -> EL0_U4_DONE
    movz x0, #0x74
    svc  #0
3:  b 3b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
    .globl __u4_blob_end
__u4_blob_end:
"#
);

unsafe extern "C" {
    static __u4_blob_start: u8;
    static __u4_blob_end: u8;
    static __u4_prog_parent: u8;
    static __u4_prog_orphan: u8;
}

// --- U5 inline EL0 fixture (handles as capabilities). ONE fixture (`el0-u5cap`) exercising all four EL0-
// observable capability behaviours against its OWN table, which the launcher pre-endows with two handles:
//   handle 1 = CONSOLE, rights = CAP_WRITE|CAP_GRANT (the "full" console cap)
//   handle 2 = CONSOLE, rights = CAP_READ            (a console cap WITHOUT write — the negative)
// Position-independent, register-only (writes no user stack — safe on any slot under preemption). It builds a
// witness bitmask in x23 (one bit per passed check) and SYS_REPORTs it, then exits with the U5 sentinel. The
// teardown-clear (behaviour 5) is proven kernel-side by `u5_launcher` after this fixture exits. ABI: x8=nr,
// args x0-x2, ret x0. The `mov x2, #(9f-8f)` message length is assembled to an immediate (the M6c idiom).
core::arch::global_asm!(
    r#"
    .globl __u5_blob_start
__u5_blob_start:
    .balign 4
    .globl __u5_prog_cap
__u5_prog_cap:
    mov  x23, xzr                          // witness bitmask = 0

    // (1) write-cap OK: sys_write(handle 1) -> byte count (>= 0)
    mov  x8, #1
    mov  x0, #1
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    tbnz x0, #63, 1f                       // negative -> skip bit0 (fail)
    add  x23, x23, #1                      // bit0: write-cap OK
1:
    // (2) no-cap -EACCES: sys_write(handle 2, lacks CAP_WRITE) -> -EACCES (-13)
    mov  x8, #1
    mov  x0, #2
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    cmn  x0, #13                           // x0 == -13 (-EACCES) ?
    b.ne 2f
    add  x23, x23, #2                      // bit1: no-cap correctly denied
2:
    // (3) attenuation: granting MORE than held is rejected; a subset grant works and its handle writes.
    mov  x8, #10                           // SYS_CAP
    mov  x0, #0                            // CAP_OP_GRANT
    mov  x1, #1                            // src = handle 1 (CAP_WRITE|CAP_GRANT, NOT CAP_EXEC)
    mov  x2, #6                            // request CAP_WRITE|CAP_EXEC (2|4) -> would amplify -> reject
    svc  #0
    tbz  x0, #63, 3f                       // grant SUCCEEDED (>=0) -> attenuation broken -> fail bit2
    mov  x8, #10                           // subset grant: CAP_WRITE only (subset of held)
    mov  x0, #0
    mov  x1, #1
    mov  x2, #2                            // CAP_WRITE
    svc  #0
    tbnz x0, #63, 3f                       // subset grant failed -> fail bit2
    mov  x20, x0                           // x20 = the minted (attenuated) handle idx
    mov  x8, #1                            // write through the minted cap -> must succeed
    mov  x0, x20
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    tbnz x0, #63, 3f                       // minted cap can't write -> fail bit2
    add  x23, x23, #4                      // bit2: attenuation bounded + subset grant usable
3:
    // (4) revoke enforced: revoke handle 1, then a write through it -> -EACCES
    mov  x8, #10                           // SYS_CAP
    mov  x0, #1                            // CAP_OP_REVOKE
    mov  x1, #1                            // drop handle 1
    svc  #0
    cbnz x0, 4f                            // revoke must return 0
    mov  x8, #1
    mov  x0, #1                            // handle 1 now revoked
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    cmn  x0, #13                           // -EACCES ?
    b.ne 4f
    add  x23, x23, #8                      // bit3: revoke enforced
4:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U5_EXIT_STATUS) -> EL0_U5_DONE
    movz x0, #0x75
    svc  #0
5:  b 5b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "u5: cap write\n"
9:
    .balign 4
    .globl __u5_blob_end
__u5_blob_end:
"#
);

unsafe extern "C" {
    static __u5_blob_start: u8;
    static __u5_blob_end: u8;
    static __u5_prog_cap: u8;
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

// --- M6g accounting (load a program FROM STORAGE — the disk-loaded EL0 program). Decoupled from every
// prior counter: the disk blob (the M6c `hello` bytes, read off the SD card's FAT volume) calls
// `sys_exit(0)`, which would otherwise land in the M6b `EL0_EXITED_OK` and corrupt `exited=1`. The
// SYS_EXIT / kill paths route by task NAME ("m6g-hello") into these counters instead, so every M6b/M6d/
// M6e/M6f verdict stays byte-identical. Read by the M6g loader (which doubles as its own verdict). ---
/// The disk-loaded EL0 program exited with status 0 (the expected outcome; want 1).
static EL0_M6G_DONE: AtomicU32 = AtomicU32::new(0);
/// The disk-loaded EL0 program exited nonzero — a self-reported failure (survivor protocol). Any is a FAIL.
static EL0_M6G_ERR: AtomicU32 = AtomicU32::new(0);
/// The disk-loaded EL0 program was KILLED by a fault (the untrusted bytes tripped the M6b fault-kill net).
static EL0_M6G_KILLED: AtomicU32 = AtomicU32::new(0);
/// Set by `m6f_verdict` as its last act: the M6g loader waits on this so every LOADER M6g line lands after
/// the M6b/M6e/M6d/M6f verdict lines (the Part-B probe's two early M6g lines land before the demo).
static M6F_VERDICT_PRINTED: AtomicBool = AtomicBool::new(false);

// =============================================================================================
// U4 accounting — the process model + per-process handle table: sys_spawn (load+run a child from storage,
// return a HANDLE into the caller's table) + sys_wait (reap the child a handle refers to). Evolves M7.
// =============================================================================================

/// Set when `m6g_loader` returns (every path). The U4 launcher gates on this so (a) all M6g lines print
/// FIRST — ordering — and (b) the M6d/M6f/M6g slots have freed (their tasks exited), so the parent's, the
/// orphan's, and the children's slot allocations succeed. (M6d + M6f hold all 8 slots when the BSP wires the
/// demo; they free as their fixtures exit, so U4's slots can only be claimed at run-time, after this gate —
/// see `u4_launcher`.)
static M6G_LOADER_DONE: AtomicBool = AtomicBool::new(false);

/// A spawned child's exit STATUS is stored valid once `state == PEXITED`.
const PFREE: u8 = 0; // entry unused
const PRUNNING: u8 = 1; // claimed; a child is (or is about to be) running under `pid`
const PEXITED: u8 = 2; // the child exited/was killed; `status` is valid, awaiting reap by sys_wait

/// The process table: parent + up to a few children. Static so it OUTLIVES each child's `Task` Box (which is
/// freed on exit) and each child's slot teardown — the reap accounting must survive both. `MAX_PROCS` is a
/// small cap « USER_SLOTS (8): if it exhausts, sys_spawn returns `-EAGAIN`, never grows the slot pool (a STOP
/// tripwire). `done` is posted exactly once by the child (its exit OR its kill path) and waited exactly once
/// by the parent's sys_wait, so a reaped-then-reused entry always starts at 0 permits (no drain needed).
const MAX_PROCS: usize = 4;
struct Proc {
    /// The child task id; the sys_wait key. 0 while an entry is FREE or a claim's pid is not yet stored.
    pid: AtomicU64,
    /// The child's exit status; valid once `state == PEXITED`.
    status: AtomicI32,
    /// FREE / RUNNING / EXITED — the ownership + lifecycle token (CAS'd FREE->RUNNING to claim).
    state: AtomicU8,
    /// Posted once by the child (SYS_EXIT or the kill path), awaited once by the parent's sys_wait. The
    /// scheduler-post wake makes sys_wait work under QEMU (unlike a timer-driven `sleep_ticks`).
    done: super::sched::Semaphore,
}
static PROCS: [Proc; MAX_PROCS] = [const {
    Proc {
        pid: AtomicU64::new(0),
        status: AtomicI32::new(0),
        state: AtomicU8::new(PFREE),
        done: super::sched::Semaphore::new(0),
    }
}; MAX_PROCS];

/// The parent's WITNESS (reported via SYS_REPORT): `U4_WITNESS_TOKEN` (nonzero) iff it reaped BOTH children
/// by handle with exit status 0, else 0. `u4_launcher`'s verdict demands it be non-zero (and no kill). A
/// token, not a pid — `sys_spawn` now returns a handle, so the verdict only needs non-zero-means-both-ok.
static U4_PARENT_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The ownership NEGATIVE result: 1 iff `el0-u4orphan`'s `sys_wait(0)` on an Empty handle returned exactly
/// `-ECHILD` (structural ownership enforced — it holds no such handle), else 0. Read by the verdict.
static U4_ORPHAN_ECHILD: AtomicU32 = AtomicU32::new(0);
/// The U4 fixtures (parent + orphan) that reached their `0x74` sentinel exit (the completion signal; want 2).
/// Read by the verdict, which waits for both before judging.
static EL0_U4_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U4 task (a child, the parent, or the orphan) — a real bug (the children are well-behaved; a kill
/// fails the verdict). Kept OFF the M6b `killed_unexpected` counter (see `record_el0_kill`) so a U4 failure is
/// its own FAIL.
static EL0_U4_KILLED: AtomicU32 = AtomicU32::new(0);

/// The U5 capability fixture's WITNESS bitmask (reported via SYS_REPORT): one bit per proven behaviour (see
/// `U5_WITNESS_ALL`). `u5_launcher` PASSes iff it equals `U5_WITNESS_ALL` (all four capability semantics held).
static U5_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U5 fixture (`el0-u5cap`) reached its `0x75` sentinel exit (want 1). Read by `u5_launcher`, which waits
/// for it before judging — and, once set, the fixture's slot is torn down so its handle row is clear.
static EL0_U5_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U5 task — a real bug (the capability fixture is well-behaved). Off the M6b `killed_unexpected`
/// counter (see `record_el0_kill`), so a U5 fault fails only the U5 verdict.
static EL0_U5_KILLED: AtomicU32 = AtomicU32::new(0);
/// Set by `u4_launcher` at its every exit path (PASS/FAIL/skip) — the gate `u5_launcher` waits on so the U5
/// lines land strictly AFTER the U4 verdict (and the U4 slots have freed). Mirrors the M6g_LOADER_DONE idiom.
static U4_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

/// Claim a FREE Proc entry, returning its index. CAS on `state` (FREE->RUNNING) is the atomic ownership
/// token; the pid=0 placeholder is overwritten with the real child pid (Release) by the caller AFTER the
/// child is spawned (see `sys_spawn` — the child cannot be dispatched until the parent yields, so the real
/// pid is always in place before any lookup). `None` if the table is full (-> `-EAGAIN`, never grow the pool).
fn proc_reserve() -> Option<usize> {
    for i in 0..MAX_PROCS {
        if PROCS[i]
            .state
            .compare_exchange(PFREE, PRUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            PROCS[i].pid.store(0, Ordering::Release);
            PROCS[i].status.store(0, Ordering::Release);
            return Some(i);
        }
    }
    None
}

/// Find the RUNNING Proc entry whose pid matches — the child-exit / child-kill lookup. Called with a live
/// task id (`> 0`), so it never spuriously matches a fresh claim's pid=0 placeholder.
fn proc_find_running(pid: u64) -> Option<usize> {
    (0..MAX_PROCS).find(|&i| {
        PROCS[i].state.load(Ordering::Acquire) == PRUNNING && PROCS[i].pid.load(Ordering::Acquire) == pid
    })
}

/// Find the non-FREE (RUNNING or EXITED) Proc entry whose pid matches — the sys_wait lookup. `None` => the
/// caller has no such child (`-ECHILD`).
fn proc_find_child(pid: u64) -> Option<usize> {
    (0..MAX_PROCS).find(|&i| {
        PROCS[i].state.load(Ordering::Acquire) != PFREE && PROCS[i].pid.load(Ordering::Acquire) == pid
    })
}

/// Release a Proc entry to FREE — after reaping in sys_wait, or unwinding a failed sys_spawn claim.
fn proc_free(i: usize) {
    PROCS[i].pid.store(0, Ordering::Release);
    PROCS[i].state.store(PFREE, Ordering::Release);
}

// ---------------------------------------------------------------------------------------------
// U4 Part A — the per-process handle table (keyed by ASID; the ownership namespace)
// ---------------------------------------------------------------------------------------------
//
// A small fixed handle table PER PROCESS, indexed by the process's ASID. Each EL0 process runs in its own
// M6d slot with a distinct ASID (1..=USER_SLOTS); the shared/boot context is ASID 0 (a valid but
// unused-by-U4 index — U4's fixtures each run in their OWN slot, so their tables are `HANDLES[asid >= 1]`).
// A handle value is `0` when Empty; otherwise it is the CHILD task id (pid) the handle refers to — the key
// into `PROCS`. So the two structures are deliberately SEPARATE and complementary: `PROCS` is keyed by pid
// (the process control blocks: exit `status`/`state`/`done`), `HANDLES` is keyed by ASID (the spawner's
// private namespace of child capabilities). Static, const-init, no heap — the `PROCS` discipline.
//
// Single-writer invariant: exactly one live task runs under any given ASID (one task per slot; a slot is
// torn down before it can be reused), and that task's syscalls are serialized (one SVC at a time), so a
// given `HANDLES[asid]` ROW is only ever touched by its own task. The atomics carry memory-ordering
// (publish the pid store with Release; a later handle read Acquires it), not cross-task contention.
//
// SCOPE NOTE (deferred to U5): a row is NOT cleared when its slot/ASID is torn down (teardown lives in
// `boot.rs`, out of this arc's lane). U4 relies on reapers CONSUMING their handles (`sys_wait` clears on
// reap) — so a well-behaved process leaves an empty row at exit, and the U4 demo is clean by construction
// (the parent reaps both children; the orphan spawns nothing; parent/orphan/children hold DISTINCT ASIDs
// while alive, and only the parent ever WRITES a row). A process that exits with UN-reaped handles would
// leave stale entries a future ASID-reuse could observe — harmless today (nothing reuses a row it did not
// write) but a real lifecycle concern once processes churn slots freely. That belongs to U5, which owns
// handle lifecycle (revoke / teardown-clear) alongside the capability CHECK it adds at this same lookup.
const NHANDLE: usize = 8; // handle slots per process (small, static — like MAX_PROCS)
/// `RESERVING` marks a handle slot claimed by an in-flight `sys_spawn` before the real child pid is known
/// (0 = Empty would let a re-scan re-claim it; a real pid is never `u64::MAX`). Overwritten with the pid
/// once the child is spawned, or cleared if the load fails — never observed by any other task (single-writer).
const HANDLE_RESERVING: u64 = u64::MAX;
static HANDLES: [[AtomicU64; NHANDLE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NHANDLE] }; super::boot::USER_SLOTS + 1];

// ---------------------------------------------------------------------------------------------
// U5 — handles as CAPABILITIES: rights, a resource target beyond "child pid", the enforcement CHECK
// ---------------------------------------------------------------------------------------------
//
// U4 built the STRUCTURE (a per-process, ASID-keyed handle table). U5 turns each handle into a capability:
// an unforgeable reference that carries RIGHTS, is CHECKED at the point of use, can be GRANTED (attenuated)
// and REVOKED, and whose lifetime is bounded by the owning ASID's teardown-clear. Two things are added to
// what a handle names — a rights bitmask (a sidecar array, so U4's `0`/`RESERVING` sentinel logic in the
// value word stays byte-identical) and a resource TARGET beyond "child pid" (a `CONSOLE` well-known token,
// so `sys_write` can route through the table). Deliberately minimal: two target kinds (CHILD(pid), CONSOLE),
// a small rights set, no general object table (that is U6+).

/// Capability rights — a small bitmask carried in the sidecar `HANDLE_RIGHTS`, checked at `handle_resolve`.
/// `CAP_WRITE` gates `sys_write`; `CAP_GRANT` gates minting attenuated copies; `CAP_READ`/`CAP_EXEC`/
/// `CAP_REVOKE` round out the model (CAP_REVOKE is reserved for cross-process revocation — U6; U5 revoke is
/// ownership-based). The values are stable across arches (documented in the permission-model doc).
const CAP_READ: u32 = 1 << 0; // 0x01
const CAP_WRITE: u32 = 1 << 1; // 0x02
const CAP_EXEC: u32 = 1 << 2; // 0x04
const CAP_GRANT: u32 = 1 << 3; // 0x08
const CAP_REVOKE: u32 = 1 << 4; // 0x10 (reserved: cross-process revocation, U6)
// The rights are the distinct low 5 bits — a well-formed bitmask (each a single, non-overlapping bit, which
// the attenuation check `req & !src` relies on). This const-assert verifies that and anchors every CAP_* as
// used, so the model bits not yet exercised in Rust this arc (CAP_EXEC — held by no fixture, so the
// attenuation negative bites; CAP_REVOKE — reserved for U6) don't read as dead code.
const _: () = assert!(
    (CAP_READ | CAP_WRITE | CAP_EXEC | CAP_GRANT | CAP_REVOKE) == 0x1F,
    "capability rights must be the distinct low 5 bits"
);

/// The well-known target token stored in a handle's value word to mean "the serial console resource" (as
/// opposed to a child pid). Distinct from `0` (Empty), `HANDLE_RESERVING` (`u64::MAX`), and every real pid
/// (small, monotonic from `NEXT_TID`), so the value word alone discriminates CHILD(pid) from CONSOLE without
/// perturbing U4's sentinel checks. Keeping it to one non-pid token (not a general object table) is the arc's
/// scope line.
const HANDLE_CONSOLE: u64 = u64::MAX - 1;

/// The conventional stdout handle index. Every EL0 program prints with `sys_write(fd=1, ..)` (the M6c hello
/// blob, the M6f hostile fixture, the disk-loaded children), so the console write-capability is endowed at
/// this fixed index in each printing process's table (`install_console_cap`). Reserved by convention, like
/// fd 1 on a POSIX system — a spawner that both prints and holds child handles would see `handle_install`
/// route children around it (no such process exists this arc; noted in the landing report).
const CONSOLE_FD: usize = 1;

/// The rights sidecar: keyed IDENTICALLY to `HANDLES` (`[asid][idx]`), so the value word keeps U4's exact
/// `0`/`RESERVING` sentinel semantics and the rights ride alongside. Written with Release beside the value
/// store (rights published BEFORE the value that makes a handle live, so a resolver that observes the value
/// also observes the rights), cleared in `handle_clear` / `clear_handle_row`. `0` rights == an inert handle.
static HANDLE_RIGHTS: [[AtomicU32; NHANDLE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NHANDLE] }; super::boot::USER_SLOTS + 1];

/// `-EACCES`: a capability check failed — no such handle / wrong kind / missing right / an attenuation
/// violation (a grant that would amplify rights). The single errno U5's CHECK returns to EL0.
const EACCES: i64 = -13;
/// `-EINVAL`: a `SYS_CAP` sub-op selector that is neither GRANT nor REVOKE.
const EINVAL: i64 = -22;

/// What a resolved handle NAMES: a child process (by pid — U4's meaning) or the console resource (U5). The
/// two target kinds this arc supports; a general object table is U6+.
#[derive(Clone, Copy)]
enum HandleTarget {
    Child(u64),
    Console,
}

/// Why `handle_resolve` refused: the handle is not in the caller's table (out-of-range or Empty), or it is
/// present but lacks a required right. Callers map these to their own errno (`sys_wait` -> `-ECHILD` for
/// either, preserving U4's structural-ownership semantics; `sys_write`/`sys_cap` -> `-EACCES`).
enum ResolveErr {
    NoHandle,
    Denied,
}

/// The U5 enforcement CHECK, at the SINGLE lookup point every handle-consuming path goes through. Resolve
/// `idx` against the caller's own (`asid`) table, then require the handle carry every bit in `req`. Returns
/// the target on success. `NoHandle` for out-of-range/Empty/`RESERVING` (a reserving placeholder is never a
/// usable handle); `Denied` when a present handle lacks a required right. The value word is loaded Acquire
/// (synchronizing with the Release store that installed it), then the rights — so a resolver that sees a live
/// value also sees its rights.
fn handle_resolve(asid: u64, idx: u64, req: u32) -> Result<HandleTarget, ResolveErr> {
    if idx as usize >= NHANDLE {
        return Err(ResolveErr::NoHandle);
    }
    debug_assert!((asid as usize) < HANDLES.len(), "handle_resolve: asid out of range");
    let raw = HANDLES[asid as usize][idx as usize].load(Ordering::Acquire);
    if raw == 0 || raw == HANDLE_RESERVING {
        return Err(ResolveErr::NoHandle);
    }
    let rights = HANDLE_RIGHTS[asid as usize][idx as usize].load(Ordering::Acquire);
    if rights & req != req {
        return Err(ResolveErr::Denied);
    }
    Ok(if raw == HANDLE_CONSOLE {
        HandleTarget::Console
    } else {
        HandleTarget::Child(raw)
    })
}

/// Set the rights word at `HANDLES[asid][idx]` (Release) — used beside a value store to attach rights to a
/// freshly-installed handle (a child handle in `sys_spawn`, a minted handle in `sys_cap_grant`).
fn handle_set_rights(asid: u64, idx: usize, rights: u32) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "handle_set_rights: out of range");
    HANDLE_RIGHTS[asid as usize][idx].store(rights, Ordering::Release);
}

/// Install a capability at a FIXED index (not `handle_install`'s first-free scan): store the rights FIRST
/// (Release), then the target value (Release), so a resolver that observes the live value also observes the
/// rights. Used to endow the console write-capability at `CONSOLE_FD` and to plant the U5 demo's fixtures.
/// Always called BEFORE the target process is dispatched (setup / pre-spawn), so there is no concurrent
/// resolver; the ordering is the defensive belt-and-braces.
fn install_cap(asid: u64, idx: usize, target: u64, rights: u32) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "install_cap: out of range");
    HANDLE_RIGHTS[asid as usize][idx].store(rights, Ordering::Release);
    HANDLES[asid as usize][idx].store(target, Ordering::Release);
}

/// Endow the process running under `asid` with a console WRITE-capability at `CONSOLE_FD` — the bootstrap
/// that lets an EL0 program print once `sys_write` routes through the table. Given at spawn/launch to every
/// process meant to print: the shared window (ASID 0: `el0-hello`) in `setup`, each M6f/M6g/U4-child slot in
/// their setup/spawn paths. A process NOT so endowed gets `-EACCES` from `sys_write` (the U5 negative).
fn install_console_cap(asid: u64) {
    install_cap(asid, CONSOLE_FD, HANDLE_CONSOLE, CAP_WRITE);
}

/// True iff the entire `HANDLES[asid]` row (values AND rights) is clear — the teardown-clear verifier. Read by
/// `u5_launcher` after the fixture exits and its slot is retired: `boot::teardown_user_slot` clears the row on
/// exit, so this transitions false -> true, proving no stale capability outlives its owning ASID.
fn handle_row_is_clear(asid: u64) -> bool {
    debug_assert!((asid as usize) < HANDLES.len(), "handle_row_is_clear: asid out of range");
    (0..NHANDLE).all(|i| {
        HANDLES[asid as usize][i].load(Ordering::Acquire) == 0
            && HANDLE_RIGHTS[asid as usize][i].load(Ordering::Acquire) == 0
    })
}

/// U5: clear an ENTIRE per-process handle row (every value + its rights) when the owning slot/ASID is torn
/// down — the lifecycle half of "U5 owns revoke/teardown-clear", folding U4's one deferred note (a row was
/// NOT cleared on teardown, so a future ASID-reuse could observe stale entries). Called from
/// `boot::teardown_user_slot` (aarch64 lane) BEFORE the slot's used-flag is released, so no concurrent
/// `alloc_user_slot` on another core can claim the slot and populate the row between the clear and the
/// release (see the ordering note there). `asid` is 1..=USER_SLOTS (ASID 0 is never torn down).
pub fn clear_handle_row(asid: u64) {
    debug_assert!(asid >= 1 && (asid as usize) < HANDLES.len(), "clear_handle_row: asid out of range");
    for i in 0..NHANDLE {
        // Clear the value first (Empty => `handle_resolve` bails as NoHandle before reading rights), then the
        // rights — so no intermediate state is ever a live handle with wrong rights.
        HANDLES[asid as usize][i].store(0, Ordering::Release);
        HANDLE_RIGHTS[asid as usize][i].store(0, Ordering::Release);
    }
}

/// The ASID of the address space the caller is running in, read from `TTBR0_EL1[63:48]`. A syscall executes
/// with the caller's `TTBR0_EL1` live (M6d), so this names the CALLER's per-process handle table. Read
/// SYNCHRONOUSLY inside the SVC handler — resolving a handle against the wrong ASID would reap the wrong
/// child or spuriously `-ECHILD`. (Placed with the handle helpers it serves; the asm-wrapper twin of
/// `remask_irq`.)
#[inline]
fn current_asid() -> u64 {
    let ttbr0: u64;
    // SAFETY: a plain read of a system register; no memory access, no clobber.
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0, options(nomem, nostack, preserves_flags)) };
    ttbr0 >> 48
}

/// Claim the first Empty slot in `HANDLES[asid]`, storing `pid` (CAS 0->pid), and return its index — the
/// value `sys_spawn` returns to EL0. `None` if the table is full (-> `-EAGAIN`, never grow it). Mirrors
/// `proc_reserve`. `asid` is always in range (0..=USER_SLOTS from a 16-bit TTBR0 ASID with USER_SLOTS==8;
/// debug-asserted). `pid` is `HANDLE_RESERVING` for a pre-spawn reservation, then overwritten with the real
/// pid via `handle_set` — or the real pid directly.
fn handle_install(asid: u64, pid: u64) -> Option<usize> {
    debug_assert!((asid as usize) < HANDLES.len(), "handle_install: asid out of range");
    let table = &HANDLES[asid as usize];
    for (i, slot) in table.iter().enumerate() {
        if slot.compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return Some(i);
        }
    }
    None
}

/// Overwrite the pid stored at `HANDLES[asid][idx]` (Release) — used to replace a `HANDLE_RESERVING`
/// placeholder with the real child pid once `sys_spawn` has it.
fn handle_set(asid: u64, idx: usize, pid: u64) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "handle_set: out of range");
    HANDLES[asid as usize][idx].store(pid, Ordering::Release);
}

/// The pid at `HANDLES[asid][idx]`, or `None` if the index is out of range or the slot is Empty (0) — i.e.
/// the caller holds no such child handle (structural ownership: `-ECHILD`). A `HANDLE_RESERVING` placeholder
/// can never be seen here (single-writer: the same task is not concurrently spawning and waiting).
fn handle_get(asid: u64, idx: usize) -> Option<u64> {
    if idx >= NHANDLE {
        return None;
    }
    debug_assert!((asid as usize) < HANDLES.len(), "handle_get: asid out of range");
    match HANDLES[asid as usize][idx].load(Ordering::Acquire) {
        0 => None,
        pid => Some(pid),
    }
}

/// Clear (0 = Empty) the handle at `HANDLES[asid][idx]` AND its sidecar rights — the handle is consumed when
/// its child is reaped in `sys_wait`, released when a failed `sys_spawn` unwinds its reservation, or dropped
/// by `sys_cap` REVOKE. Value cleared first (Empty => `handle_resolve` bails before reading rights), then the
/// rights word, so no intermediate state is ever a live handle carrying stale rights.
fn handle_clear(asid: u64, idx: usize) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "handle_clear: out of range");
    HANDLES[asid as usize][idx].store(0, Ordering::Release);
    HANDLE_RIGHTS[asid as usize][idx].store(0, Ordering::Release);
}

/// U4: record a value a process-model fixture reported via SYS_REPORT, keyed by the reporting task's name —
/// the PARENT's witness token and the ORPHAN's ownership result. Keyed by name like `m6d_report`/`m6f_report`;
/// the SYS_REPORT arm calls all three and each ignores the others' tasks.
fn u4_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-u4parent") => U4_PARENT_WITNESS.store(value, Ordering::Release),
        Some("el0-u4orphan") => U4_ORPHAN_ECHILD.store(value as u32, Ordering::Release),
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

/// U5: record the capability fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
/// Called from the SYS_REPORT arm alongside the M6d/M6f/U4 reporters; each ignores the others' names.
fn u5_report(value: u64) {
    if super::sched::current_name() == Some("el0-u5cap") {
        U5_WITNESS.store(value, Ordering::Release);
    }
}

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
    // U5: endow the SHARED window (ASID 0 — where `spawn_user` runs `el0-hello`) with a console write-
    // capability, so hello's `sys_write(fd 1)` still reaches the console once writes route through the table.
    // The shared window is never torn down (ASID 0), so this endowment persists for the whole boot; the M6b
    // fault fixtures and the M6e spinner share ASID 0 but never write, so the single fixed cap serves them all.
    install_console_cap(0);
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
    // M6g: the untrusted disk-loaded program. A kill here is contained (the whole point) and reported by
    // the loader's own verdict — route it to its own counter, never the M6b `killed_unexpected` count.
    if name == "m6g-hello" {
        EL0_M6G_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U4: a killed process-model task (a spawned CHILD, the PARENT, or the ORPHAN). Off the M6b counter — a
    // kill here is a real U4 bug that fails the U4 verdict, not a phantom M6b regression. For a killed CHILD,
    // also post its Proc `done` (with a non-zero sentinel status) so the parent's blocked `sys_wait` WAKES
    // instead of hanging — the child never reaches its own SYS_EXIT post. `current_id()` is the faulting task
    // (still current here — see aarch64_el0_fault_handler), i.e. the child's pid = its Proc key. (The parent
    // and orphan are not in PROCS — they were spawned by the launcher, not by sys_spawn — so no Proc post.)
    if name == "u4-child" || name == "el0-u4parent" || name == "el0-u4orphan" {
        EL0_U4_KILLED.fetch_add(1, Ordering::AcqRel);
        if name == "u4-child" {
            if let Some(id) = super::sched::current_id() {
                if let Some(i) = proc_find_running(id) {
                    PROCS[i].status.store(U4_KILLED_STATUS, Ordering::Release);
                    PROCS[i].state.store(PEXITED, Ordering::Release);
                    PROCS[i].done.post();
                }
            }
        }
        return;
    }
    // U5: the capability fixture is well-behaved (register-only, no faults); a kill here is a real U5 bug.
    // Route it to its own counter, never the M6b `killed_unexpected` count, so a U5 fault fails only the U5
    // verdict (its missing SYS_REPORT already leaves the witness incomplete).
    if name == "el0-u5cap" {
        EL0_U5_KILLED.fetch_add(1, Ordering::AcqRel);
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
    // U5: endow each M6f slot with a console write-capability. The hostile fixture `sys_write(fd 1)`s with
    // BAD pointers expecting -EFAULT: it must hold the cap so the resolve passes and the pointer range check
    // (still unchanged) is what refuses it. The other three fixtures don't write; the endowment is harmless.
    for &s in &slots {
        install_console_cap(super::boot::slot_ttbr0(s) >> 48);
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
    // Release so the M6g loader (which polls this Acquire) sees every M6f verdict line published first —
    // its own late lines then land strictly after the M6f verdict.
    M6F_VERDICT_PRINTED.store(true, Ordering::Release);
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
            // Route by the reporting task's name: M6d names land in m6d_report, M6f names in m6f_report, the
            // U4 parent/orphan in u4_report, the U5 cap fixture in u5_report; each ignores the others' names, so
            // calling all is safe and additive.
            m6d_report(a0);
            m6f_report(a0);
            u4_report(a0);
            u5_report(a0);
            0
        }
        SYS_YIELD => sys_yield(),
        SYS_SLEEP_MS => sys_sleep_ms(a0),
        SYS_GETPID => super::sched::current_id().map(|id| id as i64).unwrap_or(-1),
        SYS_GETINFO => sys_getinfo(a0),
        SYS_SPAWN => sys_spawn(),
        SYS_WAIT => sys_wait(a0),
        SYS_CAP => sys_cap(a0, a1, a2),
        SYS_EXIT => {
            // Demo accounting BEFORE the no-return exit. The sentinel statuses are routed to their own
            // counters so the M6b (`exited=1 killed=3`) and M6e (`completed=1`) verdicts stay byte-
            // identical: M6E_SPIN_STATUS -> EL0_SPIN_DONE, M6D_EXIT_STATUS -> EL0_M6D_DONE, M6F_EXIT_STATUS
            // -> EL0_M6F_DONE. All three sentinel arms MUST precede the catch-all `else` (a mis-ordered
            // sentinel exit would land in EL0_EXITED_ERR and FAIL the M6b verdict). Otherwise: status 0 =
            // normal completion (hello); nonzero = a fault-test program self-reporting that its intended
            // fault never happened (survivor protocol).
            // U4: a spawned CHILD's exit is reaped by its parent's sys_wait through the Proc table, keyed by
            // pid — NOT by any counter, and NOT by the handle (the handle is the parent's-side namespace; the
            // child's exit accounting is pid-keyed). This precedes every check below (the same precedence rule)
            // and SHORT-CIRCUITS to sched::exit() so the child's status-0 exit never lands in EL0_EXITED_OK
            // (M6b's `exited=1`) nor any sentinel counter. Record status + EXITED, then post `done` so the
            // (blocked or soon-to-block) parent's sys_wait wakes and reads the status. current_id() is the
            // exiting child = its Proc key (stored by sys_spawn before the child could ever be dispatched).
            if let Some(id) = super::sched::current_id() {
                if let Some(i) = proc_find_running(id) {
                    PROCS[i].status.store(a0 as i32, Ordering::Release);
                    PROCS[i].state.store(PEXITED, Ordering::Release);
                    PROCS[i].done.post();
                    super::sched::exit(); // never returns
                }
            }
            // M6g: the disk-loaded program (the M6c `hello` bytes off the SD card) exits with status 0.
            // Route by NAME, BEFORE the sentinel-status checks, so its exit lands in the M6g counters and
            // never corrupts the M6b `EL0_EXITED_OK` accounting (which `exited=1` depends on).
            if super::sched::current_name() == Some("m6g-hello") {
                if a0 == 0 {
                    EL0_M6G_DONE.fetch_add(1, Ordering::AcqRel);
                } else {
                    EL0_M6G_ERR.fetch_add(1, Ordering::AcqRel);
                }
            } else if a0 == M6E_SPIN_STATUS {
                EL0_SPIN_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == M6D_EXIT_STATUS {
                EL0_M6D_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == M6F_EXIT_STATUS {
                EL0_M6F_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U4_EXIT_STATUS {
                EL0_U4_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U5_EXIT_STATUS {
                EL0_U5_DONE.fetch_add(1, Ordering::AcqRel);
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
    // U5: `fd` is a HANDLE INDEX into the caller's per-process table, not the ambient POSIX stdout. It must
    // resolve to a CONSOLE resource carrying CAP_WRITE. No such handle / wrong kind / missing CAP_WRITE all
    // yield -EACCES — the enforcement point. A printing process is endowed this cap at spawn/launch
    // (`install_console_cap`), so every M6*/U4 print still lands; a process WITHOUT it gets -EACCES (the U5
    // negative). The pointer validation below is unchanged, so a hostile pointer still yields -EFAULT (the
    // M6f fixture, which HOLDS the console cap, resolves past this check and then hits the range check).
    match handle_resolve(current_asid(), fd, CAP_WRITE) {
        Ok(HandleTarget::Console) => {}
        _ => return EACCES,
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

// =============================================================================================
// U4: sys_spawn (load+run a child from storage, return a HANDLE) + sys_wait (reap by handle) + shared loader
// =============================================================================================

// Negative errnos returned to EL0 by sys_spawn/sys_wait (Linux-aarch64 values). These never appear in the
// demo's serial output — the parent fixture only tests the SIGN of the spawn return — but are named for the
// (future) real userspace that will interpret them. `EFAULT` (-14) is already defined for the M6f copies.
const ENOENT: i64 = -2; // no such file (HELLO.BIN missing)
const EIO: i64 = -5; // read/mount I/O error, or an empty file
const E2BIG: i64 = -7; // the program is larger than one code page
const ECHILD: i64 = -10; // sys_wait: no child with that pid
const EAGAIN: i64 = -11; // the process table (or slot pool) is full
const ENODEV: i64 = -19; // no block device / FAT volume to load from

/// A program successfully loaded into a fresh per-task slot: the EL0 entry VA, the initial SP_EL0, the
/// slot's TTBR0, and (for the M6g loader's log line) the slot id, byte length, and FAT kind.
struct Loaded {
    base: u64,
    sp: u64,
    ttbr0: u64,
    slot: usize,
    len: usize,
    kind: crate::fs::fat::FatKind,
}

/// Why `load_program_into_slot` could not produce a `Loaded`. The `FatKind` rides along on the post-mount
/// variants so the M6g loader can reproduce its exact "FAT mounted from SD (..)" progress line before its
/// specific skip line (keeping the M6g gate byte-identical); sys_spawn maps every variant to a negative errno.
enum SpawnErr {
    NoMount(crate::fs::fat::FatError),
    NoFile(crate::fs::fat::FatKind),
    BadSize(crate::fs::fat::FatKind, u32),
    ReadErr(crate::fs::fat::FatKind, crate::fs::fat::FatError),
    Empty(crate::fs::fat::FatKind),
    NoSlot(crate::fs::fat::FatKind),
}

/// Map a load failure to the errno sys_spawn returns to EL0.
fn spawn_errno(e: &SpawnErr) -> i64 {
    match e {
        SpawnErr::NoMount(_) => ENODEV,
        SpawnErr::NoFile(_) => ENOENT,
        SpawnErr::BadSize(_, _) => E2BIG,
        SpawnErr::ReadErr(_, _) | SpawnErr::Empty(_) => EIO,
        SpawnErr::NoSlot(_) => EAGAIN,
    }
}

/// The shared loader CORE for both `m6g_loader` and `sys_spawn`: mount the FAT volume off the SD card, find
/// and size-check the fixed program (`HELLO.BIN`), read it, copy it into a FRESH per-task slot's code page,
/// I-cache-sync, protect the page EL0-RX/EL1-RO BEFORE any task exists, and return the run parameters. It
/// PRINTS NOTHING (so sys_spawn stays silent inside the U4 flow) — the M6g loader reconstructs its serial
/// lines from the `Loaded`/`SpawnErr` result.
///
/// The slot is allocated LAST — after every fallible step (mount/find/size/read) — so no failure path ever
/// leaves an allocated slot to free (the A72 exposes no "free an unused slot" primitive, and
/// `teardown_user_slot` would repoint the caller's live TTBR0). A single `alloc_user_slot` attempt suffices:
/// both callers run only after M6d/M6f/M6g released their slots, so the pool has room. The loaded bytes are
/// UNTRUSTED — nothing about them is trusted beyond the one-page size bound; they run only under EL0 +
/// per-page permissions + the M6b fault-kill net (no signature, no allowlist). That containment is the point.
fn load_program_into_slot() -> Result<Loaded, SpawnErr> {
    let fs = crate::fs::fat::mount().map_err(SpawnErr::NoMount)?;
    let kind = fs.kind();
    let de = fs.find_in_root("HELLO.BIN").map_err(|_| SpawnErr::NoFile(kind))?;
    // Reject up-front from the ON-DISK directory size (the U2 truncation lesson): `read_file` caps the copy
    // at min(de.size, cap), so a post-read length check could never SEE an oversize file — it would silently
    // truncate then run it. Gate on `de.size` against the single code page instead.
    let cap = super::boot::USER_CODE_SIZE;
    if de.size == 0 || de.size as u64 > cap as u64 {
        return Err(SpawnErr::BadSize(kind, de.size));
    }
    let mut bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    fs.read_file(&de, &mut bytes, cap).map_err(|e| SpawnErr::ReadErr(kind, e))?;
    if bytes.is_empty() {
        return Err(SpawnErr::Empty(kind));
    }
    let slot = super::boot::alloc_user_slot().ok_or(SpawnErr::NoSlot(kind))?;
    let (base, size) = super::boot::user_region();
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), backing, bytes.len()) };
    super::cache::icache_sync_range(backing as usize, bytes.len());
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    Ok(Loaded {
        base,
        sp: (base + size as u64) & !0xF, // 16-aligned window top = initial SP_EL0
        ttbr0: super::boot::slot_ttbr0(slot),
        slot,
        len: bytes.len(),
        kind,
    })
}

/// SYS_SPAWN(): load the fixed on-disk program (`HELLO.BIN`) into a fresh slot, run it at EL0 as a CHILD of
/// the caller, and return a HANDLE index into the CALLER's per-process handle table (U4 — not the raw pid),
/// or a negative errno. The handle IS the ownership token: `sys_wait` takes it, and it can only be reaped by
/// a caller whose table holds it. No arguments this arc — the program is fixed; arbitrary program-by-name is
/// M8 (it needs a validated `copy_from_user` name, a STOP tripwire here).
///
/// Race-freedom (the child cannot exit before its pid is recorded): the whole SVC handler runs IRQ-masked and
/// the CHILD is co-located on the CALLER's core, so the child stays queued-not-dispatched until the parent
/// yields (which it does only later, in sys_wait). We (1) claim a Proc entry, (2) reserve a handle slot in the
/// caller's table, (3) load the program, (4) spawn the child (queued, not run), (5) store its real pid with
/// Release into BOTH the Proc entry and the handle slot — all before returning to EL0, hence strictly before
/// the parent can yield and let the child run. The child's exit/kill lookup therefore always observes the
/// stored Proc pid. This co-location invariant is load-bearing and QEMU-true (no sched change needed). The
/// handle slot is reserved BEFORE the load so a full handle table fails cleanly with nothing to un-spawn.
fn sys_spawn() -> i64 {
    // Gate: we need a block device to load the child off. -ENODEV so a no-SD boot fails the spawn cleanly.
    if crate::drivers::block::info().is_none() {
        return ENODEV;
    }
    // The CALLER's ASID names its per-process handle table — read synchronously here, where the caller's
    // TTBR0 is live (sys_spawn installs into and sys_wait resolves from the SAME table, since both run as
    // the parent).
    let asid = current_asid();
    // Claim the Proc entry FIRST, so a failed load frees nothing but the entry, and (crucially) so the pid
    // slot exists to receive the real pid before the child can be dispatched.
    let Some(i) = proc_reserve() else {
        return EAGAIN; // process table full
    };
    // Reserve a HANDLE slot in the caller's table BEFORE spawning (a RESERVING placeholder, overwritten with
    // the real pid below). A full handle table fails here with only the Proc entry to release — no child has
    // been loaded or spawned yet, so there is nothing to un-spawn.
    let Some(h) = handle_install(asid, HANDLE_RESERVING) else {
        proc_free(i);
        return EAGAIN; // handle table full
    };
    let loaded = match load_program_into_slot() {
        Ok(l) => l,
        Err(e) => {
            handle_clear(asid, h); // release the reserved handle slot
            proc_free(i); // no address-space slot was allocated on any load-failure path — release the entry
            return spawn_errno(&e);
        }
    };
    // U5: endow the CHILD's OWN table with a console write-capability (the child runs `HELLO.BIN`, which
    // `sys_write`s fd 1). Done here, on the freshly-built slot, BEFORE the child is spawned — the child cannot
    // be dispatched until the parent yields (the co-location invariant below), so there is no concurrent
    // resolver of the child's table. Without this the child's first print would return -EACCES (routed).
    install_console_cap(loaded.ttbr0 >> 48);
    // Co-locate the child on the caller's core (the invariant above): sys_spawn always runs with its EL0
    // caller current, so `this_cpu` is the parent's core.
    let cpu = super::percpu::this_cpu().cpu_index as usize;
    let pid = super::sched::spawn_user_slot("u4-child", loaded.base, loaded.sp, loaded.ttbr0, cpu);
    // Record the real pid (Release) into BOTH the Proc entry (pid-keyed exit accounting) and the reserved
    // handle slot (ASID-keyed ownership namespace) BEFORE returning to EL0 — before the parent can yield and
    // let the child run. The child's exit path sees the Proc pid; the parent's later sys_wait resolves the
    // handle to it. U5: the parent's child handle carries CAP_READ (the ownership token; `sys_wait` gates on
    // kind==Child, not on the right — published Release before the pid so the handle is never live sans rights).
    PROCS[i].pid.store(pid, Ordering::Release);
    handle_set_rights(asid, h, CAP_READ);
    handle_set(asid, h, pid);
    h as i64 // return the HANDLE index (per-process; two processes can each hold handle 0 to different children)
}

/// SYS_WAIT(handle): block the caller until the child its `handle` refers to exits, then return the child's
/// exit status — or `-ECHILD` if that handle is not in the CALLER's table (out-of-range or Empty). Structural
/// ownership: you can only reap a child whose handle is in YOUR table; a foreign or stale handle simply isn't
/// there. The waker is the child's `done.post()` — a SCHEDULER wake (from the child's SYS_EXIT or its kill
/// path), so this works under QEMU (unlike a timer-driven sleep).
///
/// We wait on `done` UNCONDITIONALLY (not only when the child is still RUNNING): the child posts `done`
/// exactly once (exit or kill), so waiting once either fast-returns a permit the child already left (child
/// exited first — no park) or parks until the child posts. Exactly one post is consumed by exactly one wait,
/// so the reaped entry's semaphore returns to 0 permits and is clean for reuse — the balance the process
/// table relies on. (Under QEMU the child, co-located, cannot run until we block here, so it is always the
/// park path; the fast path is the metal case where a timer preempts the parent between spawn and wait.)
///
/// The handle is CONSUMED by the reap (`handle_clear`), so a second `sys_wait` on the same handle returns
/// `-ECHILD` (Empty) — correct. `PROCS` stays keyed by pid (exit accounting); `HANDLES` by ASID (ownership).
fn sys_wait(handle: u64) -> i64 {
    let asid = current_asid();
    // Resolve the handle against the CALLER's OWN table — the structural ownership check, now through the U5
    // enforcement point. It must be a CHILD handle (U4's meaning). Out-of-range/Empty (NoHandle), a rights
    // shortfall (Denied), or a CONSOLE handle all mean "you hold no such child" => -ECHILD (byte-identical to
    // U4 for the orphan's `sys_wait(0)`). Waiting requires no resource right — holding the child handle is the
    // ownership token (`req = 0`); child handles carry CAP_READ for model completeness, not as a wait gate.
    let pid = match handle_resolve(asid, handle, 0) {
        Ok(HandleTarget::Child(pid)) => pid,
        _ => return ECHILD,
    };
    let Some(i) = proc_find_child(pid) else {
        return ECHILD; // the handle named a pid with no Proc entry (defensive; cannot happen in the demo)
    };
    let woken = PROCS[i].done.wait();
    debug_assert!(woken, "sys_wait: called off a scheduled task");
    // `Semaphore::wait` restores the SVC-entry DAIF (IRQ masked on exception entry), so IRQ is already masked
    // here; re-mask defensively so the `__vec_svc` epilogue's banked ELR/SPSR/SP_EL0 restore is guaranteed
    // I-masked regardless of any future change to wait()'s IRQ discipline (the sys_yield/sys_sleep contract).
    remask_irq();
    let status = PROCS[i].status.load(Ordering::Acquire) as i64;
    proc_free(i); // reap the Proc entry (its `done` is back at 0 permits, free for reuse)
    handle_clear(asid, handle as usize); // consume the handle: a second sys_wait on it now returns -ECHILD
    status
}

/// SYS_CAP(op, a1, a2): grant/attenuate/revoke on the CALLER's OWN handle table — capabilities as first-class
/// operations. `op` selects the sub-op. Runs single-writer over the caller's table (one SVC at a time, one
/// live task per ASID), so no lock is needed. See `sys_cap_grant`/`sys_cap_revoke`.
fn sys_cap(op: u64, a1: u64, a2: u64) -> i64 {
    let asid = current_asid();
    match op {
        CAP_OP_GRANT => sys_cap_grant(asid, a1, a2),
        CAP_OP_REVOKE => sys_cap_revoke(asid, a1),
        _ => EINVAL,
    }
}

/// SYS_CAP GRANT(src_idx, req_rights): mint a NEW handle in the caller's own table naming the SAME target as
/// `src_idx`, carrying `req_rights` — enforcing the ATTENUATION (monotonic-decrease) invariant: the minted
/// rights can never exceed the granter's rights on the source. Requires `CAP_GRANT` on the source. Returns
/// the new handle index, or a negative errno:
///   -EACCES — no such source handle, source lacks CAP_GRANT, or `req_rights` would AMPLIFY (bits the granter
///             does not hold): the core U5 property — a grant can never produce more rights than the granter.
///   -EAGAIN — the caller's handle table is full (no free slot to mint into; never grown).
/// For this arc the mint targets the caller's OWN table (a child spawns nothing to grant into yet); minting
/// into another table is a straightforward extension once cross-process object naming lands (U6).
fn sys_cap_grant(asid: u64, src_idx: u64, req_rights: u64) -> i64 {
    // Resolve the source's raw target + rights (no right required to READ your own handle's descriptor).
    let Some(target) = handle_get(asid, src_idx as usize) else {
        return EACCES; // no such source handle
    };
    if target == HANDLE_RESERVING {
        return EACCES; // an in-flight reservation is not a grantable handle (defensive; single-writer)
    }
    let src_rights = HANDLE_RIGHTS[asid as usize][src_idx as usize].load(Ordering::Acquire);
    if src_rights & CAP_GRANT == 0 {
        return EACCES; // the source does not authorize granting
    }
    let req = req_rights as u32;
    // Attenuation: reject any requested bit the granter does not itself hold. `req & !src_rights` is exactly
    // the set of amplifying bits; non-empty => the grant would exceed the granter's authority.
    if req & !src_rights != 0 {
        return EACCES;
    }
    // Mint: reuse `handle_install` for the first-free slot claim (the U4 sentinel logic), then attach the
    // attenuated rights. Single-writer over this table (the caller is mid-syscall, not concurrently resolving),
    // so the value-then-rights order carries no race.
    match handle_install(asid, target) {
        Some(idx) => {
            handle_set_rights(asid, idx, req);
            idx as i64
        }
        None => EAGAIN, // handle table full
    }
}

/// SYS_CAP REVOKE(idx): drop a handle the caller owns (`handle_clear`, which also clears its rights). A
/// process may always drop its OWN capabilities (ownership-based — the caller's table is its own), so no
/// right is required here; `CAP_REVOKE` is reserved for cross-process revocation (revocation trees — U6).
/// Returns 0, or -ECHILD if the index is out-of-range/Empty (nothing to revoke). After revoke, any use of the
/// index returns -EACCES (`sys_write`) / -ECHILD (`sys_wait`) — the handle is gone.
fn sys_cap_revoke(asid: u64, idx: u64) -> i64 {
    if idx as usize >= NHANDLE || handle_get(asid, idx as usize).is_none() {
        return ECHILD; // out-of-range or Empty — no such handle to revoke
    }
    handle_clear(asid, idx as usize);
    0
}

// =============================================================================================
// M6g: load a program FROM STORAGE and run it at EL0 (the Pi twin of x86 U2)
// =============================================================================================

/// M6g loader (also its own verdict). A kernel task spawned once on a scheduled AP AFTER the M6f verdict
/// spawn. On the bare-metal Pi the program comes off the very microSD card the Pi booted from: the
/// Part-B EMMC2/SDHCI probe (on the BSP) already registered the SD block backend, so here we mount its
/// FAT volume, read `HELLO.BIN` (the same `USER_BLOB` bytes M6c bakes in — carried onto the boot media as
/// `HELLO.BIN`), size-check it, copy it into a fresh M6d per-task slot's code page, protect the page
/// (EL0-RX/EL1-RO) BEFORE the task exists, and drop it to EL0. The loaded bytes are UNTRUSTED: nothing
/// about them is trusted beyond the size bound — the program runs only under EL0 + per-page permissions +
/// the M6b fault-kill net (size-bounded only; no signature, no allowlist). That containment is the point.
///
/// Ordering: it first waits (bounded) for `M6F_VERDICT_PRINTED` so every LOADER line lands AFTER the
/// M6b/M6e/M6d/M6f verdict lines (the Part-B probe's two lines already printed early, on the BSP). A
/// missing SD device / FAT volume / file / oversize logs one clean skip line and returns.
pub fn m6g_loader(arg: usize) {
    m6g_loader_run(arg);
    // Release the U4 gate: by here every M6g line has printed AND the M6d/M6f/M6g slots have freed (their
    // tasks exited), so the U4 launcher may build the parent + orphan + children. Set on EVERY path (load /
    // skip / no-SD) so the launcher never waits out its deadline; the launcher separately re-checks for an SD.
    M6G_LOADER_DONE.store(true, Ordering::Release);
}

fn m6g_loader_run(_: usize) {
    // 1. Wait (bounded ~8 s CNTPCT, yielding — the m6d_verdict idiom) for the M6f verdict to publish, so
    //    the loader's lines follow every prior verdict line rather than racing into the middle of them.
    let wstart = super::timer::cntpct();
    let wdeadline = 8 * super::timer::cntfrq();
    while !M6F_VERDICT_PRINTED.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot from here (spawned once, but guard defensively like u2_probe_once).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. Gate: the Part-B probe registered an SD block device (the empty gate is the no-SD control path).
    if crate::drivers::block::info().is_none() {
        serial_println!(":: M6g: no SD card found — loader skipped ::");
        return;
    }

    // 3-6. Load HELLO.BIN into a fresh slot via the shared core, then reproduce M6g's EXACT serial lines
    //      from the result (the core is silent so sys_spawn stays quiet inside the U4 flow). Every skip path
    //      first echoes the "FAT mounted from SD (..)" progress line (mirroring the original mid-flow print)
    //      so the M6g gate is byte-identical. On success: emit the two M6g lines then drop the program to
    //      EL0 on THIS core (the loader's), so the folded verdict's cooperative yield guarantees dispatch.
    let loaded = match load_program_into_slot() {
        Ok(l) => l,
        Err(SpawnErr::NoMount(e)) => {
            serial_println!(":: M6g: no FAT volume ({:?}) — loader skipped ::", e);
            return;
        }
        Err(SpawnErr::NoFile(kind)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: HELLO.BIN not found on the FAT volume — loader skipped ::");
            return;
        }
        Err(SpawnErr::BadSize(kind, sz)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(
                ":: M6g: HELLO.BIN bad size {} bytes (must be 1..={}) — loader skipped ::",
                sz,
                super::boot::USER_CODE_SIZE
            );
            return;
        }
        Err(SpawnErr::ReadErr(kind, e)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: HELLO.BIN read error ({:?}) — loader skipped ::", e);
            return;
        }
        Err(SpawnErr::Empty(kind)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: HELLO.BIN read empty — loader skipped ::");
            return;
        }
        Err(SpawnErr::NoSlot(kind)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: no free address-space slot — loader skipped ::");
            return;
        }
    };
    serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", loaded.kind);
    serial_println!(
        ":: M6g: HELLO.BIN loaded from SD ({} bytes) -> EL0 (slot {}, ASID {}) ::",
        loaded.len,
        loaded.slot,
        loaded.ttbr0 >> 48
    );
    let run_cpu = super::percpu::this_cpu().cpu_index as usize;
    // U5: endow the disk-loaded program's slot with a console write-capability so its `sys_write(fd 1)`
    // reaches the console once writes route through the table (it prints "hello from EL0").
    install_console_cap(loaded.ttbr0 >> 48);
    super::sched::spawn_user_slot("m6g-hello", loaded.base, loaded.sp, loaded.ttbr0, run_cpu);

    // 7. Verdict (folded in — no extra task): wait (bounded ~2 s, yielding so m6g-hello runs on this core)
    //    for the disk program to terminate, then print PASS/FAIL. The disk blob's `sys_exit(0)` is routed
    //    by name into EL0_M6G_DONE; a fault into EL0_M6G_KILLED; a nonzero exit into EL0_M6G_ERR.
    let vstart = super::timer::cntpct();
    let vdeadline = 2 * super::timer::cntfrq();
    while EL0_M6G_DONE.load(Ordering::Acquire)
        + EL0_M6G_ERR.load(Ordering::Acquire)
        + EL0_M6G_KILLED.load(Ordering::Acquire)
        == 0
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let done = EL0_M6G_DONE.load(Ordering::Acquire);
    let err = EL0_M6G_ERR.load(Ordering::Acquire);
    let killed = EL0_M6G_KILLED.load(Ordering::Acquire);
    if done == 1 && err == 0 && killed == 0 {
        serial_println!(":: M6g: disk-loaded EL0 program exited ok -> PASS ::");
    } else {
        serial_println!(
            ":: M6g: disk-loaded EL0 program FAIL — done={} err={} killed={} (want 1/0/0) ::",
            done, err, killed
        );
    }
}

// =============================================================================================
// U4: the process-model demo — the parent + orphan slots + the gated launcher/verdict
// =============================================================================================

/// The U4 fixtures' run parameters: the parent's and the orphan's EL0 entry VAs (both inside the shared
/// window VA — only the slot FRAME differs, via TTBR0), the shared initial SP_EL0, and each fixture's slot
/// TTBR0. Two tasks, two slots (with DISTINCT ASIDs — the isolation the ownership negative proves).
struct U4Demo {
    parent: u64,
    orphan: u64,
    sp: u64,
    ttbr0_parent: u64,
    ttbr0_orphan: u64,
}

/// U4 setup: reserve the Proc semaphores, then allocate + build TWO private slots (parent + orphan) via the
/// unwinding `alloc_user_slots`, copy the U4 blob (both fixtures) into each slot's code page, I-cache-sync,
/// and protect each code page (EL0-RX/EL1-RO). Emits the U4 setup line; returns both entries + slot roots.
/// `None` if slot allocation fails (the whole request is released, not leaked). Called ONCE, from
/// `u4_launcher`, AFTER the M6g gate — so the M6d/M6f/M6g slots have freed (at BSP-wiring time all 8 are held
/// by M6d+M6f) and strictly before the parent (hence any child) exists, which is why the `done.init()`
/// reservations here cannot race a concurrent wait/post (the M4 discipline).
///
/// The parent and orphan get DISTINCT slots (hence distinct ASIDs), so their per-process handle tables are
/// distinct rows of `HANDLES` — the substrate the negative proves: handle #0 means the parent's child A in
/// the parent's table, and Empty in the orphan's.
fn u4_setup() -> Option<U4Demo> {
    for p in &PROCS {
        p.done.init();
    }
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = shared initial SP_EL0
    let bstart = &raw const __u4_blob_start as usize;
    let bend = &raw const __u4_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U4 blob does not fit in a code page");
    // Entry VAs = base + each fixture's offset within the blob (an eret to a misaligned entry is EC 0x22).
    let entry = |label: *const u8| -> u64 {
        let va = base + (label as usize - bstart) as u64;
        assert!(va & 3 == 0, "U4 fixture entry misaligned");
        va
    };
    let parent = entry(&raw const __u4_prog_parent);
    let orphan = entry(&raw const __u4_prog_orphan);

    // Two slots, released together on partial failure (the M6d/M6f unwind). slots[0] = parent, [1] = orphan.
    let mut slots = [0usize; 2];
    if !super::boot::alloc_user_slots(&mut slots) {
        return None;
    }
    // Copy the whole blob into each slot's code page (identity backing VA) + I-cache sync (DC CVAU/IC IVAU;
    // PIPT L1 caches make it fetchable at the aliased EL0 window VA), then protect each EL0-RX/EL1-RO.
    for &s in &slots {
        let backing = super::boot::slot_backing_ptr(s);
        unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
        super::cache::icache_sync_range(backing as usize, blen);
    }
    for &s in &slots {
        unsafe { super::boot::protect_user_slot_code(s, super::boot::USER_CODE_SIZE) };
    }

    serial_println!(
        ":: U4: process model — per-process handle table (sys_spawn->handle, sys_wait(handle)) ::"
    );
    Some(U4Demo {
        parent,
        orphan,
        sp,
        ttbr0_parent: super::boot::slot_ttbr0(slots[0]),
        ttbr0_orphan: super::boot::slot_ttbr0(slots[1]),
    })
}

/// U4 launcher + verdict (the M6g-loader shape: one gated kernel task). Spawned on a scheduled sibling core;
/// `demo_cpu` (the task arg) is the demo core the parent + orphan run on. Flow:
///   1. Wait (bounded) for `M6G_LOADER_DONE`, so all M6g lines print first AND the slots have freed.
///   2. Skip silently if no SD device (the parent's sys_spawn loads the children off the card — nothing to run).
///   3. `u4_setup()` (build both slots, print the U4 setup line), then spawn the parent AND the orphan on
///      `demo_cpu`. The parent's two `sys_spawn`s co-locate BOTH children on `demo_cpu` too — the invariant
///      that keeps each child queued-not-dispatched until the parent blocks in sys_wait (so both pids are
///      recorded first; load-bearing for two children exactly as M7's was for one). The orphan's
///      `sys_wait(0)` returns immediately (-ECHILD), so it never parks — no deadlock with the co-located work.
///   4. Verdict (folded): wait (bounded CNTPCT) for BOTH fixtures to reach their sentinel exit
///      (`EL0_U4_DONE == 2`), then PASS iff the parent reaped both children (witness non-zero) AND the orphan
///      saw -ECHILD (ownership enforced) AND no U4 task was killed. Prints ONE PASS line.
/// The U4 lines (setup, the two children's `hello from EL0` — the THIRD and FOURTH in a full boot — and the
/// PASS) all land after the M6g lines and in that order (setup precedes the spawns; the children's hellos
/// precede the parent's exit, which precedes EL0_U4_DONE reaching 2, which the verdict polls before PASS).
pub fn u4_launcher(demo_cpu: usize) {
    // 1. Gate on the M6g loader (its lines printed + its/M6d's/M6f's slots freed).
    let wstart = super::timer::cntpct();
    let wdeadline = 10 * super::timer::cntfrq();
    while !M6G_LOADER_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No SD device -> the children cannot be loaded; skip silently (keeps the no-SD control path free of
    //    U4 lines, mirroring how M6g's own no-SD path is the empty control).
    if crate::drivers::block::info().is_none() {
        U4_LAUNCH_DONE.store(true, Ordering::Release); // release the U5 gate (U5 also gates on the SD)
        return;
    }

    // 3. Build the parent + orphan slots and spawn both on the demo core.
    let Some(u4) = u4_setup() else {
        serial_println!(":: U4: no free address-space slot — process-model demo skipped ::");
        U4_LAUNCH_DONE.store(true, Ordering::Release); // release the U5 gate
        return;
    };
    super::sched::spawn_user_slot("el0-u4parent", u4.parent, u4.sp, u4.ttbr0_parent, demo_cpu);
    super::sched::spawn_user_slot("el0-u4orphan", u4.orphan, u4.sp, u4.ttbr0_orphan, demo_cpu);

    // 4. Folded verdict: wait (bounded ~5 s, yielding) for BOTH fixtures to reach their sentinel exit, then
    //    judge. Two children (two disk loads) + the orphan complete well under this budget under QEMU.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U4_DONE.load(Ordering::Acquire) < 2
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U4_PARENT_WITNESS.load(Ordering::Acquire);
    let orphan = U4_ORPHAN_ECHILD.load(Ordering::Acquire);
    let killed = EL0_U4_KILLED.load(Ordering::Acquire);
    // The parent reports the token iff it reaped BOTH children by handle with status 0, else 0 — so
    // `== U4_WITNESS_TOKEN` is exactly "both reaped OK" (tighter than the M7 `!= 0`, and it pins the
    // fixture/verdict contract on one constant).
    if witness == U4_WITNESS_TOKEN && orphan == 1 && killed == 0 {
        serial_println!(
            ":: U4: process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD (per-process handle tables) -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U4: process model FAIL — witness={:#x} orphan_echild={} killed={} done={} (want nonzero / 1 / 0 / 2) ::",
            witness,
            orphan,
            killed,
            EL0_U4_DONE.load(Ordering::Acquire)
        );
    }
    // Release the U5 gate: the U4 verdict line has printed and the U4 slots have freed, so `u5_launcher` may
    // now run its capability demo (its lines land strictly after this).
    U4_LAUNCH_DONE.store(true, Ordering::Release);
}

// =============================================================================================
// U5: the capability demo — the cap fixture's slot + endowment + the gated launcher/verdict
// =============================================================================================

/// The U5 fixture's run parameters: the cap fixture's EL0 entry VA (inside the shared window VA — only the
/// slot FRAME differs, via TTBR0), the initial SP_EL0, its slot TTBR0, and its ASID (so the launcher can
/// pre-endow the fixture's table and, after exit, verify the teardown-clear of that exact row).
struct U5Demo {
    cap: u64,
    sp: u64,
    ttbr0: u64,
    asid: u64,
}

/// U5 setup: allocate + build ONE private slot, copy the U5 blob into its code page, I-cache-sync, protect it
/// EL0-RX/EL1-RO, then PRE-ENDOW the fixture's table with the two handles the demo exercises:
///   handle 1 = CONSOLE, {CAP_WRITE|CAP_GRANT} — the "full" console cap it writes from and grants from
///   handle 2 = CONSOLE, {CAP_READ}            — a console cap WITHOUT write (the `-EACCES` negative)
/// Emits the U5 setup line; returns the run params. `None` if slot allocation fails. Called ONCE from
/// `u5_launcher`, after the U4 gate — so a slot is free and no task runs under the fixture's ASID yet (the
/// endowment stores can't race a resolver). Register-only fixture (writes no user stack), so one slot suffices.
fn u5_setup() -> Option<U5Demo> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = initial SP_EL0
    let bstart = &raw const __u5_blob_start as usize;
    let bend = &raw const __u5_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U5 blob does not fit in a code page");
    let cap = {
        let va = base + (&raw const __u5_prog_cap as usize - bstart) as u64;
        assert!(va & 3 == 0, "U5 fixture entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    let asid = ttbr0 >> 48;
    // Pre-endow the fixture's table (before it is dispatched — no concurrent resolver). Two console caps: a
    // full one (write + grant) at index 1, and a write-LESS one at index 2 for the negative.
    install_cap(asid, 1, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    install_cap(asid, 2, HANDLE_CONSOLE, CAP_READ);
    serial_println!(
        ":: U5: capabilities — rights + CHECK + grant/attenuate/revoke + routed sys_write ::"
    );
    Some(U5Demo { cap, sp, ttbr0, asid })
}

/// U5 launcher + verdict (the `u4_launcher` shape: one gated kernel task on a sibling core). `demo_cpu` (the
/// task arg) is the core the cap fixture runs on. Flow:
///   1. Wait (bounded) for `U4_LAUNCH_DONE`, so the U5 lines land after the U4 verdict and the U4 slots freed.
///   2. Skip silently if no SD device — U5 needs NO disk (its fixture is an inline blob), but gating on the SD
///      keeps the no-SD control path free of demo lines, mirroring M6g/U4.
///   3. `u5_setup()` (build + pre-endow the fixture's slot), then spawn the fixture on `demo_cpu`.
///   4. Verdict (folded): wait (bounded) for the fixture's sentinel exit (`EL0_U5_DONE == 1`), read its
///      witness bitmask, then wait (bounded) for its handle row to be cleared — the teardown-clear proof:
///      `sched::exit -> boot::teardown_user_slot` clears the row when the fixture exits, transitioning
///      `handle_row_is_clear` false->true (the fixture holds live handles at exit — the minted cap and the
///      write-less cap — so this genuinely exercises the clear). PASS iff witness == `U5_WITNESS_ALL` AND the
///      row cleared AND no U5 kill. Prints ONE PASS line.
pub fn u5_launcher(demo_cpu: usize) {
    // 1. Gate on the U4 launcher (its verdict printed + the U4 slots freed).
    let wstart = super::timer::cntpct();
    let wdeadline = 10 * super::timer::cntfrq();
    while !U4_LAUNCH_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No SD device -> keep the no-SD control path free of demo lines (U5 itself needs no disk, but this
    //    mirrors M6g/U4's control discipline).
    if crate::drivers::block::info().is_none() {
        return;
    }

    // 3. Build + pre-endow the fixture slot and spawn it on the demo core.
    let Some(u5) = u5_setup() else {
        serial_println!(":: U5: no free address-space slot — capability demo skipped ::");
        return;
    };
    super::sched::spawn_user_slot("el0-u5cap", u5.cap, u5.sp, u5.ttbr0, demo_cpu);

    // 4a. Wait (bounded ~5 s, yielding) for the fixture to reach its sentinel exit, then snapshot the witness.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U5_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U5_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U5_KILLED.load(Ordering::Acquire);

    // 4b. Teardown-clear proof: the fixture exited above, so its exit path cleared its handle row. That clear
    //     runs just AFTER the sentinel increment, so poll (bounded) until the row is clear — false->true when
    //     teardown runs. Nothing reuses the slot after (U5 is the last demo), so once clear it stays clear.
    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !handle_row_is_clear(u5.asid)
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = handle_row_is_clear(u5.asid);

    if witness == U5_WITNESS_ALL && cleared && killed == 0 {
        serial_println!(
            ":: U5: capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded, revoke enforced, teardown-clear clean -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U5: capabilities FAIL — witness={:#x} cleared={} killed={} done={} (want {:#x} / true / 0 / 1) ::",
            witness,
            cleared,
            killed,
            EL0_U5_DONE.load(Ordering::Acquire),
            U5_WITNESS_ALL
        );
    }
}
