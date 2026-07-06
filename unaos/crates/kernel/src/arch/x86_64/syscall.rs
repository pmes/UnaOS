// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// x86_64 ring-3 userspace + the SYSCALL/SYSRET syscall interface (U1a: the first privilege
// boundary — the x86 mirror of the aarch64 EL0 M6a arc).
//
// The kernel runs at ring 0 on the firmware identity map. A scheduled task drops to ring 3
// (`sched::spawn_user` -> `sched::user_task_trampoline`) at `USER_BASE` and calls back in with
// `syscall`; the CPU vectors to LSTAR (`unaos_syscall_entry`), which switches to the task's kernel
// stack and calls `syscall_dispatch` here, IRQ-masked (SFMASK clears IF). The ABI mirrors the
// aarch64 one: rax = number, args in rdi/rsi/rdx, return in rax. `sys_exit` reclaims the task via
// the scheduler; `sys_write` copies a bound-checked user buffer to the console.
//
// x86 has no `swapgs`-free per-CPU story like aarch64's TPIDR: the existing per-CPU data lives at
// IA32_GS_BASE (see `percpu.rs`) and the whole scheduler — including `sched::exit`, which
// `sys_exit` calls — resolves `this_cpu()` through it. So the stack-switch anchor the brief calls
// `SyscallCpu` is folded INTO `PerCpuData` (fields `syscall_kernel_rsp` / `syscall_user_rsp`),
// KERNEL_GS_BASE holds that same per-CPU pointer while ring 3 runs, and the syscall-entry `swapgs`
// brings it back into GS so `this_cpu()` keeps working in the handler. This is the standard Linux
// scheme and the only shape that doesn't break the scheduler's GS dependency; the `swapgs`
// mechanism and every hardening bit (NXE, W^X code page, per-page NX, SMEP, RSP0) the brief asks
// for are preserved. Full user-GPR preservation across a syscall + fault->task-kill are arc U1b.

use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
};

use x86_64::registers::control::Cr4;
use x86_64::registers::model_specific::{LStar, Msr};
use x86_64::VirtAddr;

use crate::arch::percpu::{KERNEL_RSP_OFFSET, USER_RSP_OFFSET};

// --- Syscall numbers (the tiny U1a subset; mirrors aarch64). ---
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
// U4x: the process-model pair (same numbers as aarch64 U4). `sys_spawn` loads the fixed on-disk
// program (`HELLO.BIN`) into a fresh slot, runs it ring-3 as a CHILD, and returns a small HANDLE
// index into the caller's per-process handle table; `sys_wait(handle)` blocks until that child exits
// and returns its status (or `-ECHILD` if the handle is not in the caller's table).
const SYS_SPAWN: u64 = 8;
const SYS_WAIT: u64 = 9;
// U5x: operate on the caller's OWN handle table as capabilities. `a0` selects the sub-op
// (`CAP_OP_GRANT`/`CAP_OP_REVOKE`); the remaining args are op-specific (see `sys_cap`). GRANT mints a
// new, rights-attenuated handle to the same target as a source handle the caller holds `CAP_GRANT` on;
// REVOKE clears a handle the caller owns. The enforcement layer sits at the handle lookup
// (`handle_resolve`). Same number as aarch64 U5.
const SYS_CAP: u64 = 10;
/// `SYS_CAP` sub-ops (in `a0`). GRANT: `a1`=source handle idx, `a2`=requested rights mask -> new handle
/// idx (attenuated) or a negative errno. REVOKE: `a1`=handle idx to drop -> 0 or a negative errno.
const CAP_OP_GRANT: u64 = 0;
const CAP_OP_REVOKE: u64 = 1;

/// Base of the ring-3 window: 1 TiB — a FRESH top-level slot (PML4 index 2) above the firmware
/// identity map, so mapping it touches no kernel state. `setup` proves it unmapped before use.
pub const USER_BASE: u64 = 0x0000_0100_0000_0000;
/// Window size in 4 KiB pages: code, data, and two stack pages.
const USER_WINDOW_PAGES: u64 = 4;
const PAGE_SIZE: u64 = 0x1000;

// MSR numbers used raw (the ones the x86_64 typed API doesn't cover cleanly here).
const IA32_EFER: u32 = 0xC000_0080;
const IA32_STAR: u32 = 0xC000_0081;
const IA32_FMASK: u32 = 0xC000_0084;
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

// EFER bits.
const EFER_SCE: u64 = 1 << 0; // SYSCALL/SYSRET enable
const EFER_NXE: u64 = 1 << 11; // NX-bit enable
// CR4 bit.
const CR4_SMEP: u64 = 1 << 20;
// SFMASK: RFLAGS bits cleared on syscall entry — IF | TF | DF | AC. Interrupts stay OFF in the
// handler; DF/AC/TF cleared so the kernel starts from an ABI-clean, deterministic RFLAGS.
const SYSCALL_FLAG_MASK: u64 = 0x4_0700;

// --- The baked-in ring-3 program + the U1b fault fixtures. Fully position-independent — every
// reference is RIP-relative and there are only register ops + `syscall`, so each routine runs
// correctly at its VA in the copied code page wherever the blob lands. `unaos_user_blob_{start,end}`
// bound the copy; `unaos_user_{hello,wild_write,code_write,stack_exec}` are the per-routine entries.
//
// The three fixtures (U1b) are fault-SHAPE fixtures, not programs: each provokes ONE specific fault
// the kernel must answer with a task-kill (the x86 mirror of the aarch64 M6b fault blob). If the
// intended fault does NOT happen (broken permissions / a stale writable TLB entry), the fixture
// falls through to `sys_exit(1)` — the SURVIVOR protocol: a self-reported, greppable FAIL instead
// of a ring-3 wedge (ring 3 runs IF-masked/cooperative, so a bare spin would hang its core and
// silence the verdict the failure is supposed to reach). `stack_exec` has no survivor tail — if NX
// were broken the target bytes are BSS zeros, still a fault, but a DIFFERENT vector the (task, vec,
// cr2) accounting counts as killed_UNEXPECTED, failing the verdict as it must. ---
core::arch::global_asm!(
    r#"
    .globl unaos_user_blob_start
    .globl unaos_user_hello
unaos_user_blob_start:
unaos_user_hello:
    mov rax, 1                              // SYS_WRITE
    mov rdi, 1                              // fd = 1 (stdout)
    lea rsi, [rip + unaos_user_msg]         // buf -> the ring-3 VA at run time (RIP-relative)
    mov rdx, [rip + unaos_user_msglen]      // len (from the embedded length word; RIP-relative)
    syscall
    mov rax, 2                              // SYS_EXIT
    mov rdi, 0                              // status = 0
    syscall
1:  jmp 1b                                  // sys_exit never returns; spin as a guard
    // Read-only data living in the (ring-3 RX, kernel-readable) code page. The length is an
    // assemble-time label difference — no `mov reg, sym-sym` immediate (LLVM Intel reads that as a
    // memory operand); the running blob loads it RIP-relative instead.
    .balign 8
unaos_user_msglen:
    .quad unaos_user_msg_end - unaos_user_msg
unaos_user_msg:
    .ascii "hello from ring 3\n"
unaos_user_msg_end:

    // Fixture 1 — write to a kernel-only VA (linear address 0). Ring 3 has no mapping there with the
    // USER bit, so the store faults: #PF, error U(ser) + W(rite) set, CR2 = 0. `mov [rax], rax` with
    // rax==0 writes zeros, so even a bug that let the store through can't scribble garbage.
    .balign 16
    .globl unaos_user_wild_write
unaos_user_wild_write:
    mov rax, 0
    mov qword ptr [rax], rax                // -> #PF (U|W), CR2 = 0
    mov rax, 2                              // survivor: the store didn't fault -> sys_exit(1)
    mov rdi, 1
    syscall
1:  jmp 1b

    // Fixture 2 — write to its OWN code page, now read-only to ring 3 (mapped ring3-RX). The target
    // is its own first instruction, already executed, so a stale-TLB write (bug) cannot corrupt code
    // that still has to run. -> #PF, error U|W set, CR2 inside the code page.
    .balign 16
    .globl unaos_user_code_write
unaos_user_code_write:
    lea rax, [rip + unaos_user_code_write]
    mov byte ptr [rax], 0                   // -> #PF (U|W), CR2 in [USER_BASE, USER_BASE+4KiB)
    mov rax, 2                              // survivor: the store didn't fault -> sys_exit(1)
    mov rdi, 1
    syscall
1:  jmp 1b

    // Fixture 3 — jump into the NX user stack (mapped RW + NX + USER). The instruction FETCH from an
    // NX page at CPL 3 faults: #PF, error U + I(nstruction-fetch) set, CR2 = the branch target in
    // the stack pages. No survivor tail (see the blob header).
    .balign 16
    .globl unaos_user_stack_exec
unaos_user_stack_exec:
    mov rax, rsp
    sub rax, 16
    jmp rax                                 // -> #PF (U|I), CR2 in the data/stack pages
1:  jmp 1b

    // U2 Part-0a fixture — arm RFLAGS.TF, then SYSCALL immediately. rax/rdi are loaded FIRST so the
    // ONLY instruction that executes with TF pending is `syscall` itself: a POPFQ that sets TF defers
    // the single-step trap by one instruction, so it fires for the SYSCALL — landing on the LSTAR
    // entry stub at CPL 0 (GS/RSP still ring-3). The #DB handler clears TF and resumes, so sys_exit(0)
    // runs and the task exits cleanly ("survived"). A platform that instead delivers the #DB in ring 3
    // kills the task — either way the kernel is never halted (the DoS the #DB IST wiring closes).
    .balign 16
    .globl unaos_user_tf_syscall
unaos_user_tf_syscall:
    mov rax, 2                              // SYS_EXIT (set up BEFORE arming TF)
    mov rdi, 0                              // status = 0
    pushfq
    or qword ptr [rsp], 0x100               // set RFLAGS.TF (bit 8)
    popfq                                   // TF armed; single-step deferred to the next instruction
    syscall                                 // -> #DB at the SYSCALL-entry stub (CPL 0)
1:  jmp 1b                                  // sys_exit never returns; spin as a guard

    // U3 fixture — read this process's PRIVATE data-page sentinel and write it to a readback slot,
    // then exit. `lea r8,[rip+blob_start]` = USER_BASE (the blob is loaded at USER_BASE), so
    // [r8+0x1000] is the data page. Under a per-process CR3 that VA hits THIS process's private
    // frame; a task that (wrongly) ran under the shared window would read the wrong sentinel. The
    // GPR scrub zeroed r8 on entry, so the `lea` is the only source of the address. No stack use.
    .balign 16
    .globl unaos_user_u3_reader
unaos_user_u3_reader:
    lea r8, [rip + unaos_user_blob_start]   // r8 = USER_BASE (code page base)
    add r8, 0x1000                          // r8 -> data page (USER_BASE + PAGE)
    mov rax, [r8]                           // read this process's private sentinel
    mov [r8 + 8], rax                       // write it to the readback slot (data page + 8)
    mov rax, 2                              // SYS_EXIT
    mov rdi, 0                              // status = 0
    syscall
1:  jmp 1b

    // U3.5 fixture — a PREEMPTIBLE ring-3 spinner that NEVER syscalls: it increments a counter in its
    // OWN (private-CR3) data page in a tight loop forever. Cooperative ring 3 (RFLAGS.IF clear) would
    // WEDGE its core here — the one-core DoS this arc closes; preemptible ring 3 (IF set) lets the
    // timer evict it so co-located tasks share the core, and the counter (read back through the slot's
    // kernel alias) proves it RESUMES correctly across preemptions under its OWN CR3 (a task run under
    // the wrong CR3 would bump a different frame). `lea` recovers USER_BASE (r8 was scrubbed to 0 on
    // entry); the loop touches only the data page — no stack, no syscall. The kernel watchdog reaps it
    // via the scheduler at a preemption boundary (it never exits).
    .balign 16
    .globl unaos_user_u3_5_spinner
unaos_user_u3_5_spinner:
    lea r8, [rip + unaos_user_blob_start]   // r8 = USER_BASE (code page base)
    add r8, 0x1000                          // r8 -> data page (USER_BASE + PAGE)
1:  inc qword ptr [r8]                       // bump the forward-progress counter (private CR3)
    jmp 1b                                   // spin forever — never syscalls; reaped by the watchdog

    .globl unaos_user_blob_end
unaos_user_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_blob_start: u8;
    static unaos_user_blob_end: u8;
    static unaos_user_hello: u8;
    static unaos_user_wild_write: u8;
    static unaos_user_code_write: u8;
    static unaos_user_stack_exec: u8;
    static unaos_user_tf_syscall: u8;
    static unaos_user_u3_reader: u8;
    static unaos_user_u3_5_spinner: u8;
}

// --- U4x ring-3 fixtures (per-process handle table). ONE blob with TWO fixtures — the PARENT (the
// spawner capability) and the ownership NEGATIVE (the orphan) — copied into each fixture's OWN slot
// and entered at its own offset (the aarch64 __u4 twin). Kept SEPARATE from the U1a/U1b/U3/U3.5 blob
// above so those fixtures stay byte-identical. Both are position-independent and REGISTER-ONLY (no
// memory refs at all — only GPR ops + `syscall`), so they run correctly at any VA and write no user
// stack. ABI (Linux-style): rax = number, args rdi/rsi/rdx, return in rax.
//
// The SYSCALL/SYSRET contract this relies on: the C dispatcher preserves the callee-saved GPRs
// (rbx/rbp/r12-r15) across a syscall (and `switch_context` preserves them across the sys_wait BLOCK),
// so the parent safely accumulates its two handles + two statuses in r12-r15 across four syscalls;
// the sysret tail scrubs only rdi/rsi/rdx/r8-r10, never r12-r15 or rax (the return value).
//
// PARENT (`u4x-parent`): the capability — a spawner reaps MULTIPLE children BY HANDLE. Two `SYS_SPAWN`s
// (two handle indices in r12/r13), two `SYS_WAIT`s (two statuses in r14/r15), then `sys_exit(status)`
// where status = 0 iff BOTH handles were valid (sign clear) AND both children exited 0, else 1 — the
// witness the kernel routes into `U4X_PARENT_OK`.
//
// ORPHAN (`u4x-orphan`): the ownership NEGATIVE — it spawned nothing, so handle #0 is Empty in ITS OWN
// per-process table; `sys_wait(0)` must therefore return `-ECHILD` (-10). It exits 0 iff it saw
// exactly -ECHILD (structural ownership: a task cannot reap a child whose handle is not in its table),
// else 1 — routed into `U4X_ORPHAN_ECHILD`. Deterministic; its handle table is empty.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u4x_blob_start
unaos_user_u4x_blob_start:
    .balign 16
    .globl unaos_user_u4x_parent
unaos_user_u4x_parent:
    mov rax, 8                              // SYS_SPAWN (child A) -> rax = handle_a (>=0) or -errno
    syscall
    mov r12, rax                            // r12 = handle_a  (callee-saved; survives the next syscalls)
    mov rax, 8                              // SYS_SPAWN (child B) -> a SECOND child, a SECOND handle
    syscall
    mov r13, rax                            // r13 = handle_b
    mov rax, 9                              // SYS_WAIT(handle_a) — blocks until child A exits (sched wake)
    mov rdi, r12
    syscall
    mov r14, rax                            // r14 = status_a
    mov rax, 9                              // SYS_WAIT(handle_b) — reap child B by ITS handle
    mov rdi, r13
    syscall
    mov r15, rax                            // r15 = status_b
    mov rdi, 1                              // default exit status = 1 (FAIL); cleared to 0 iff all OK
    test r12, r12
    js 1f                                   // handle_a < 0 (spawn A failed) -> witness stays FAIL
    test r13, r13
    js 1f                                   // handle_b < 0 (spawn B failed) -> witness stays FAIL
    test r14, r14
    jnz 1f                                  // status_a != 0 (child A not clean) -> witness stays FAIL
    test r15, r15
    jnz 1f                                  // status_b != 0 (child B not clean) -> witness stays FAIL
    xor edi, edi                            // all four OK -> exit status 0 (both reaped by handle)
1:  mov rax, 2                              // SYS_EXIT(status) -> U4X_PARENT_OK, U4X_DONE
    syscall
2:  jmp 2b                                  // sys_exit never returns; belt-and-braces guard

    // The ownership negative: sys_wait a handle it never installed (handle #0, Empty in its own table).
    .balign 16
    .globl unaos_user_u4x_orphan
unaos_user_u4x_orphan:
    mov rax, 9                              // SYS_WAIT(handle #0) — Empty in its OWN never-spawned table
    xor edi, edi                            // handle = 0
    syscall                                 // rax should be -ECHILD (-10)
    mov rdi, 1                              // default exit status = 1 (FAIL)
    cmp rax, -10                            // rax == -ECHILD?
    jne 3f
    xor edi, edi                            // saw -ECHILD -> exit 0 (structural ownership enforced)
3:  mov rax, 2                              // SYS_EXIT(status) -> U4X_ORPHAN_ECHILD, U4X_DONE
    syscall
4:  jmp 4b                                  // sys_exit never returns; belt-and-braces guard
    .globl unaos_user_u4x_blob_end
unaos_user_u4x_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u4x_blob_start: u8;
    static unaos_user_u4x_blob_end: u8;
    static unaos_user_u4x_parent: u8;
    static unaos_user_u4x_orphan: u8;
}

// --- U5x ring-3 fixture (handles as capabilities — the aarch64 `__u5_prog_cap` twin). ONE fixture
// (`u5x-cap`) exercising all four ring-3-observable capability behaviours against its OWN slot's handle
// table, which the launcher (`u5x_setup`) pre-endows with two handles:
//   handle 1 = CONSOLE, rights = CAP_WRITE|CAP_GRANT (the "full" console cap — writes and grants from it)
//   handle 2 = CONSOLE, rights = CAP_READ            (a console cap WITHOUT write — the -EACCES negative)
// Position-independent; register-only apart from the RIP-relative message load (writes no user stack, so
// it is safe on any slot). It builds a witness bitmask in r12 (callee-saved — the C dispatcher preserves
// it across each `syscall`, the u4x-parent idiom) and conveys it as its `sys_exit` STATUS, which the
// SYS_EXIT arm routes BY NAME into `U5X_WITNESS` (x86 routes by task name, so no SYS_REPORT is needed —
// the aarch64 twin uses SYS_REPORT only because its sentinel exit status is reserved for demo routing).
// The teardown-clear (behaviour 5) is proven kernel-side by `u5x_launcher` after this fixture exits.
// ABI (Linux-style): rax = number, args rdi/rsi/rdx, return in rax.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u5x_blob_start
unaos_user_u5x_blob_start:
    .balign 16
    .globl unaos_user_u5x_cap
unaos_user_u5x_cap:
    xor r12d, r12d                          // witness bitmask = 0 (callee-saved; survives syscalls)

    // (1) write-cap OK: sys_write(handle 1) -> byte count (>= 0)
    mov rax, 1                              // SYS_WRITE
    mov rdi, 1                              // fd = handle 1 (CONSOLE, CAP_WRITE|CAP_GRANT)
    lea rsi, [rip + unaos_user_u5x_msg]     // buf -> the ring-3 VA at run time (RIP-relative)
    mov rdx, [rip + unaos_user_u5x_msglen]  // len (from the embedded length word)
    syscall
    test rax, rax
    js 1f                                   // negative -> skip bit0 (fail)
    or r12, 1                               // bit0: write-cap OK
1:
    // (2) no-cap -EACCES: sys_write(handle 2, lacks CAP_WRITE) -> -EACCES (-13)
    mov rax, 1
    mov rdi, 2                              // fd = handle 2 (CONSOLE, CAP_READ only)
    lea rsi, [rip + unaos_user_u5x_msg]
    mov rdx, [rip + unaos_user_u5x_msglen]
    syscall
    cmp rax, -13                            // rax == -EACCES ?
    jne 2f
    or r12, 2                               // bit1: no-cap correctly denied
2:
    // (3) attenuation: granting MORE than held is rejected; a subset grant works and its handle writes.
    mov rax, 10                             // SYS_CAP
    xor edi, edi                            // CAP_OP_GRANT (0)
    mov rsi, 1                              // src = handle 1 (CAP_WRITE|CAP_GRANT, NOT CAP_EXEC)
    mov rdx, 6                              // request CAP_WRITE|CAP_EXEC (2|4) -> would amplify -> reject
    syscall
    test rax, rax
    jns 3f                                  // grant SUCCEEDED (>=0) -> attenuation broken -> fail bit2
    mov rax, 10                             // subset grant: CAP_WRITE only (subset of held)
    xor edi, edi                            // CAP_OP_GRANT
    mov rsi, 1                              // src = handle 1
    mov rdx, 2                              // CAP_WRITE
    syscall
    test rax, rax
    js 3f                                   // subset grant failed -> fail bit2
    mov r13, rax                            // r13 = the minted (attenuated) handle idx (callee-saved)
    mov rax, 1                              // write through the minted cap -> must succeed
    mov rdi, r13
    lea rsi, [rip + unaos_user_u5x_msg]
    mov rdx, [rip + unaos_user_u5x_msglen]
    syscall
    test rax, rax
    js 3f                                   // minted cap can't write -> fail bit2
    or r12, 4                               // bit2: attenuation bounded + subset grant usable
3:
    // (4) revoke enforced: revoke handle 1, then a write through it -> -EACCES
    mov rax, 10                             // SYS_CAP
    mov rdi, 1                              // CAP_OP_REVOKE (1)
    mov rsi, 1                              // drop handle 1
    syscall
    test rax, rax
    jnz 4f                                  // revoke must return 0
    mov rax, 1                              // SYS_WRITE(handle 1) — now revoked
    mov rdi, 1
    lea rsi, [rip + unaos_user_u5x_msg]
    mov rdx, [rip + unaos_user_u5x_msglen]
    syscall
    cmp rax, -13                            // -EACCES ?
    jne 4f
    or r12, 8                               // bit3: revoke enforced
4:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U5X_WITNESS
    mov rdi, r12                            // status = witness bitmask
    syscall
5:  jmp 5b                                  // sys_exit never returns; belt-and-braces guard
    // Read-only data in the (ring-3 RX, kernel-readable) code page. Length is an assemble-time label
    // difference loaded RIP-relative (the M6c/hello idiom — no `mov reg, sym-sym` memory-operand trap).
    .balign 8
unaos_user_u5x_msglen:
    .quad unaos_user_u5x_msg_end - unaos_user_u5x_msg
unaos_user_u5x_msg:
    .ascii "u5x: cap write\n"
unaos_user_u5x_msg_end:
    .globl unaos_user_u5x_blob_end
unaos_user_u5x_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u5x_blob_start: u8;
    static unaos_user_u5x_blob_end: u8;
    static unaos_user_u5x_cap: u8;
}

// --- The SYSCALL entry stub (LSTAR target). Naked; the only assembly in the syscall path.
//
// On entry (CPL 0, from SYSCALL): rcx = user RIP, r11 = user RFLAGS, rax = number, rdi/rsi/rdx =
// args, GS = user, IF already cleared by SFMASK. It swaps in this CPU's PerCpuData, switches to the
// task's kernel stack, saves the return frame (rcx/r11), shuffles the (rax, rdi, rsi, rdx) syscall
// args into the C ABI's (rdi, rsi, rdx, rcx), dispatches, restores, and `sysretq`s back to ring 3.
// SYS_EXIT never returns (it switches to the scheduler), so the restore tail runs only for
// returning syscalls. Alignment: after loading rsp = kernel-stack top (16-aligned) the two pushes
// leave rsp 16-aligned, so the `call` meets the SysV (rsp % 16 == 8)-at-entry rule.
//
// U1a does NOT preserve the user's rbx/rbp/r12-r15/rdi/... across a syscall (only rcx/r11, which
// SYSRET requires); the demo blob loads fresh registers for its second syscall, so this is sound
// for the cooperative single-shot. Full GPR preservation is arc U1b. ---
core::arch::global_asm!(
    ".globl unaos_syscall_entry",
    "unaos_syscall_entry:",
    "swapgs",                       // GS -> this CPU's PerCpuData (parked in KERNEL_GS_BASE shadow)
    "mov gs:[{uoff}], rsp",         // save the user rsp
    "mov rsp, gs:[{koff}]",         // switch to this task's kernel stack top
    "push r11",                     // save user RFLAGS  (SYSRET restores from r11)
    "push rcx",                     // save user RIP     (SYSRET restores from rcx); rsp now 16-aligned
    "mov rcx, rdx",                 // arg2 -> 4th C arg
    "mov rdx, rsi",                 // arg1 -> 3rd C arg
    "mov rsi, rdi",                 // arg0 -> 2nd C arg
    "mov rdi, rax",                 // number -> 1st C arg
    "call {dispatch}",              // rax = return value (or never returns: SYS_EXIT -> scheduler)
    "pop rcx",                      // restore user RIP   (rsp now = kernel-stack top, 16-aligned)
    "pop r11",                      // restore user RFLAGS
    // --- U1b B2: canonical-rcx guard (CVE-2012-0217 shape). A non-canonical SYSRET target #GPs at
    // CPL 0 *after* the user rsp is loaded, running the #GP handler on a user-controlled stack.
    // Refuse to sysret such an rcx. rdx is scratch here (a caller-saved leftover, scrubbed below
    // anyway). Assumes 48-bit VAs (setup() asserts LA57 off): sign-extend bit 47 and compare —
    // equal iff rcx was canonical.
    "mov rdx, rcx",
    "shl rdx, 16",
    "sar rdx, 16",
    "cmp rdx, rcx",
    "jne 2f",                       // non-canonical -> kill the task (GS still = PerCpuData here)
    // --- U1b B1: scrub the caller-saved GPRs that carry kernel-dispatcher leftovers to ring 3.
    // rax = return value; rcx/r11 = the SYSRET pair; rbx/rbp/r12-r15 still hold the user's own
    // pre-syscall values (the C dispatch preserved them across the call) — so only these six can
    // leak a kernel pointer to ring 3. Zeroing the 32-bit name clears the full 64-bit register.
    "xor edi, edi",
    "xor esi, esi",
    "xor edx, edx",
    "xor r8d, r8d",
    "xor r9d, r9d",
    "xor r10d, r10d",
    "mov rsp, gs:[{uoff}]",         // restore the user rsp
    "swapgs",                       // GS -> user
    "sysretq",                      // -> ring 3: rip = rcx, rflags = r11, rax = return value
    // Non-canonical return RIP: still CPL 0, still on the kernel stack, GS = PerCpuData (the entry
    // swapgs is not yet undone). Kill the task instead of executing the poisoned sysret.
    "2:",
    "call {noncanon}",              // never returns (sched::exit)
    "ud2",
    // U2 Part-0a: end marker bounding the stub. The #DB handler treats a CPL-0 single-step trap
    // whose RIP lies in [unaos_syscall_entry, unaos_syscall_entry_end) as the TF-armed-SYSCALL case
    // and resumes it (clear TF + iretq) instead of halting — see `rip_in_entry_stub`.
    ".globl unaos_syscall_entry_end",
    "unaos_syscall_entry_end:",
    uoff = const USER_RSP_OFFSET,
    koff = const KERNEL_RSP_OFFSET,
    dispatch = sym syscall_dispatch,
    noncanon = sym syscall_ret_noncanonical,
);

unsafe extern "C" {
    fn unaos_syscall_entry();
    static unaos_syscall_entry_end: u8;
}

/// U2 Part-0a: true iff `rip` lies within the `unaos_syscall_entry` LSTAR stub. The #DB handler uses
/// this to recognise the TF-armed-`SYSCALL` single-step trap (delivered at CPL 0 on the stub's first
/// instruction, GS/RSP possibly still ring-3) and resume it instead of treating it as a fatal
/// kernel #DB. Only the FIRST instruction can actually trap (SFMASK clears TF for the rest of the
/// stub), but bounding the whole stub is strictly safer and equally GS-free.
pub fn rip_in_entry_stub(rip: u64) -> bool {
    let start = unaos_syscall_entry as usize as u64;
    let end = &raw const unaos_syscall_entry_end as u64;
    rip >= start && rip < end
}

// --- U1a/U1b demo accounting (written by the exit + fault-kill paths, read by the verdicts). ---
/// Ring-3 tasks that exited with status 0 (normal completion — the demo expects exactly 1: hello).
static U1A_EXITED_OK: AtomicU32 = AtomicU32::new(0);
/// Ring-3 tasks that exited nonzero — a U1b fault fixture SELF-REPORTING that its intended fault
/// never happened (the survivor protocol). Any nonzero count is a FAIL.
static U1A_EXITED_ERR: AtomicU32 = AtomicU32::new(0);
/// U1b: task-kills whose (task, vector, cr2) matched the demo's expectation table (want exactly 3).
static RING3_KILLED_EXPECTED: AtomicU32 = AtomicU32::new(0);
/// U1b: kills that did NOT match — a fault happened, but not the one the permission model dictates
/// (e.g. NX unset would turn stack-exec's instruction #PF into some other vector). Any is a FAIL.
static RING3_KILLED_UNEXPECTED: AtomicU32 = AtomicU32::new(0);
/// One-shot: the first syscall proves the ring-3 -> ring-0 path is live end to end.
static SYSCALL_LOGGED: AtomicBool = AtomicBool::new(false);
/// One-shot guard for the SMEP status line (identical across the homogeneous CPUs).
static SMEP_LOGGED: AtomicBool = AtomicBool::new(false);
/// One-shot guard for the U2.5 Part 0-ii DR7-cleared status line (the DR7 clear itself is per-CPU).
static DR7_LOGGED: AtomicBool = AtomicBool::new(false);
/// U2 Part-0a: the TF+SYSCALL fixture terminating cleanly. `EXITED_OK` = it reached `sys_exit(0)`
/// (the #DB was neutralized at the entry stub and the syscall resumed); `KILLED` = a platform
/// delivered the #DB in ring 3 and the task was killed. Either is an acceptable "survived" outcome
/// (the kernel was never halted — the DoS the #DB IST wiring closes).
static U2_0A_EXITED_OK: AtomicU32 = AtomicU32::new(0);
static U2_0A_KILLED: AtomicU32 = AtomicU32::new(0);
/// U2: the loaded-from-FAT HELLO.BIN program terminating. `OK` = `sys_exit(0)`; `ERR` = a nonzero
/// exit (a FAIL). A kill (untrusted bytes faulting) leaves both 0 — the verdict times out to FAIL.
static U2_EXITED_OK: AtomicU32 = AtomicU32::new(0);
static U2_EXITED_ERR: AtomicU32 = AtomicU32::new(0);
/// U3: number of per-process-CR3 ring-3 fixture tasks that reached `sys_exit` (the readback verdict
/// waits for this to hit 2 before reading each slot's readback word).
static U3_EXITED: AtomicU32 = AtomicU32::new(0);

// #PF error-code bits (mirror `x86_64::PageFaultErrorCode`), used by `record_ring3_kill` to demand
// the EXACT fault shape each fixture provokes — not merely "it died".
const PF_ERR_WRITE: u64 = 1 << 1; // the access was a write
const PF_ERR_USER: u64 = 1 << 2; // the access was in user mode (CPL 3)
const PF_ERR_INSTR: u64 = 1 << 4; // the access was an instruction fetch (needs EFER.NXE)

/// U1b: reached from `unaos_syscall_entry` when the SYSRET return RIP (rcx) is non-canonical. Runs
/// at CPL 0 on the faulting task's kernel stack with GS = PerCpuData (the entry swapgs is not yet
/// undone), so `sched::exit()` is safe — the same context the fault-kill path uses. Never returns.
#[unsafe(no_mangle)]
extern "C" fn syscall_ret_noncanonical() -> ! {
    let name = crate::arch::sched::current_name().unwrap_or("<ring3>");
    serial_println!(
        ":: EL0-equiv FAULT: task '{}' KILLED — non-canonical sysret rip (CVE-2012-0217 guard) ::",
        name
    );
    // Count it as a kill so a corrupted-RIP program still terminates cleanly; it is not one of the
    // demo's expected fixtures, so it lands in killed_UNEXPECTED (a loud verdict FAIL if it ever
    // fires) rather than silently passing.
    RING3_KILLED_UNEXPECTED.fetch_add(1, Ordering::AcqRel);
    crate::arch::sched::exit()
}

/// U1b accounting: classify a killed ring-3 task against the demo's EXPECTED faults, called from
/// `interrupts::ring3_fault_kill` before it exits the task. The verdict demands the right (task,
/// vector, CR2-region) triple, not just "it died": the stack pages are BSS zeros and `0x00 0x00`
/// decodes as a real instruction, so a broken-NX stack-exec would still fault — but at a different
/// vector/address, which count-only bookkeeping would false-PASS. `vec` is the exception number and
/// `err` its raw error code (the #PF error bits for vector 14; ignored otherwise). `cr2` is the
/// faulting linear address for #PF (0 for other vectors).
pub fn record_ring3_kill(name: &str, vec: u8, err: u64, cr2: u64) {
    const PF: u8 = 14;
    // U2 Part-0a: a ring-3 #DB on the TF+SYSCALL fixture (a platform that delivers the single-step
    // trap in ring 3 rather than at the CPL-0 entry stub). Count it as its own clean-kill outcome,
    // not a U1b "unexpected" kill — the fixture "survived" the same way (the kernel is not halted).
    if name == "u2-tf-syscall" {
        U2_0A_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U4x: a killed process-model task (a spawned CHILD, the PARENT, or the ORPHAN). Off the U1b
    // counter — a kill here is a real U4x bug that fails the U4x verdict, not a phantom U1b regression.
    // For a killed CHILD, also post its Proc `done` (with a nonzero sentinel status) so the parent's
    // blocked sys_wait WAKES instead of hanging — the child never reaches its own sys_exit post.
    // `current_task_id` names the faulting task (still current here — see `ring3_fault_kill`), i.e. the
    // child's pid = its Proc key. (The parent and orphan are not in PROCS — they were spawned by the
    // launcher, not by sys_spawn — so no Proc post.)
    if name == "u4x-child" || name == "u4x-parent" || name == "u4x-orphan" {
        U4X_KILLED.fetch_add(1, Ordering::AcqRel);
        if name == "u4x-child" {
            let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
            if let Some(id) = crate::arch::sched::current_task_id(cpu) {
                if let Some(i) = proc_find_running(id) {
                    PROCS[i].status.store(U4X_KILLED_STATUS, Ordering::Release);
                    PROCS[i].state.store(PEXITED, Ordering::Release);
                    PROCS[i].done.post();
                }
            }
        }
        return;
    }
    // U5x: a killed capability fixture — off the U1b counter (a kill here is a real U5x bug that fails
    // the U5x verdict, not a phantom U1b regression). It is not in PROCS (the launcher spawned it, not
    // sys_spawn), so there is no parent semaphore to post — the launcher times out to FAIL on `U5X_DONE`.
    if name == "u5x-cap" {
        U5X_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    let code_end = USER_BASE + PAGE_SIZE; // the code page is the first page of the window only
    let window_end = USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE;
    let expected = match name {
        // wild-write: a ring-3 write to kernel-only VA 0 -> #PF, User+Write set, CR2 in page 0.
        "u1b-wild-write" => {
            vec == PF && err & PF_ERR_USER != 0 && err & PF_ERR_WRITE != 0 && cr2 < PAGE_SIZE
        }
        // code-write: a ring-3 write to the now-RO code page -> #PF, User+Write set, CR2 in it.
        "u1b-code-write" => {
            vec == PF
                && err & PF_ERR_USER != 0
                && err & PF_ERR_WRITE != 0
                && cr2 >= USER_BASE
                && cr2 < code_end
        }
        // stack-exec: a ring-3 instruction fetch from the NX stack -> #PF, User+Instr set, CR2 in
        // the data/stack pages (above the code page, inside the window).
        "u1b-stack-exec" => {
            vec == PF
                && err & PF_ERR_USER != 0
                && err & PF_ERR_INSTR != 0
                && cr2 >= code_end
                && cr2 < window_end
        }
        _ => false,
    };
    if expected {
        RING3_KILLED_EXPECTED.fetch_add(1, Ordering::AcqRel);
    } else {
        RING3_KILLED_UNEXPECTED.fetch_add(1, Ordering::AcqRel);
    }
}

/// SYSCALL dispatcher, called from `unaos_syscall_entry` on the faulting task's kernel stack with
/// IF masked (SFMASK) and GS = this CPU's PerCpuData (so `this_cpu()` / the scheduler resolve). rax
/// = number, rdi/rsi/rdx = args; the return value goes back in rax. A blocking/exiting syscall may
/// safely `switch_context` here — exactly like `timer_preempt` from the timer ISR.
#[unsafe(no_mangle)]
extern "C" fn syscall_dispatch(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    if !SYSCALL_LOGGED.swap(true, Ordering::Relaxed) {
        serial_println!(":: SYSCALL: nr={} — ring-3 -> ring-0 path live ::", nr);
    }
    match nr {
        SYS_WRITE => sys_write(a0, a1, a2),
        SYS_SPAWN => sys_spawn(),
        SYS_WAIT => sys_wait(a0),
        SYS_CAP => sys_cap(a0, a1, a2),
        SYS_EXIT => {
            // U4x: a spawned CHILD's exit is reaped by its parent's sys_wait through the Proc table,
            // keyed by pid — NOT by any counter and NOT by the handle (the handle is the parent's-side
            // namespace; the child's exit accounting is pid-keyed). This SHORT-CIRCUITS before every
            // check below (the same precedence rule aarch64 U4 uses) so the child's status-0 exit never
            // lands in U1A_EXITED_OK (which U1a's `exited=1` depends on) nor any sentinel counter.
            // Record status + EXITED, post `done` so the (blocked or soon-to-block) parent wakes and
            // reads it. The exiting child's id (its Proc key) was stored by sys_spawn before the child
            // could ever be dispatched, so `proc_find_running` always resolves it here.
            let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
            if let Some(id) = crate::arch::sched::current_task_id(cpu) {
                if let Some(i) = proc_find_running(id) {
                    PROCS[i].status.store(a0 as i32, Ordering::Release);
                    PROCS[i].state.store(PEXITED, Ordering::Release);
                    PROCS[i].done.post();
                    crate::arch::sched::exit(); // never returns
                }
            }
            // Accounting BEFORE the no-return exit, routed by task so the U1a/U1b, Part-0a, U2, and U4x
            // verdicts stay independent (U1a/U1b keep their exact byte-for-byte counts). status 0 =
            // normal completion; nonzero = a program self-reporting failure (the U1b survivor
            // protocol). U1a/U1b tasks fall through to the default branch unchanged.
            match crate::arch::sched::current_name() {
                Some("u2-hello") => {
                    if a0 == 0 {
                        U2_EXITED_OK.fetch_add(1, Ordering::AcqRel);
                    } else {
                        U2_EXITED_ERR.fetch_add(1, Ordering::AcqRel);
                    }
                }
                Some("u2-tf-syscall") => {
                    // Reaching sys_exit means the #DB was neutralized and the syscall resumed — the
                    // fixture always exits 0 (kernel survived).
                    U2_0A_EXITED_OK.fetch_add(1, Ordering::AcqRel);
                }
                Some("u4x-parent") => {
                    // The parent's witness: status 0 iff it reaped BOTH children by handle with status
                    // 0 (see the parent blob). Routed to its own counter so the U1a `exited=1` count
                    // stays byte-for-byte.
                    U4X_PARENT_OK.store((a0 == 0) as u32, Ordering::Release);
                    U4X_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u4x-orphan") => {
                    // The ownership negative: status 0 iff its `sys_wait(0)` on an Empty handle returned
                    // exactly -ECHILD (structural ownership enforced).
                    U4X_ORPHAN_ECHILD.store((a0 == 0) as u32, Ordering::Release);
                    U4X_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u5x-cap") => {
                    // U5x: the capability fixture conveys its 4-bit witness bitmask as its exit STATUS
                    // (routed by name, like the U4x parent/orphan — x86 has no SYS_REPORT). Stored for
                    // the launcher's verdict; `U5X_DONE` gates the read.
                    U5X_WITNESS.store(a0 as u32, Ordering::Release);
                    U5X_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some(n) if n.starts_with("u3-") => {
                    // U3 per-process-CR3 fixture task exiting (its own accounting, so the U1a/U1b
                    // default counts stay byte-for-byte). The readback verdict reads slot memory.
                    U3_EXITED.fetch_add(1, Ordering::AcqRel);
                }
                _ => {
                    if a0 == 0 {
                        U1A_EXITED_OK.fetch_add(1, Ordering::AcqRel);
                    } else {
                        U1A_EXITED_ERR.fetch_add(1, Ordering::AcqRel);
                    }
                }
            }
            crate::arch::sched::exit() // never returns; the stub's restore tail is not reached
        }
        _ => -38, // -ENOSYS
    }
}

/// SYS_WRITE(fd, buf, len): copy `len` bytes from the ring-3 buffer to the serial console; returns
/// the count written, or a negative errno.
///
/// The pointer is a ring-3 VA that (identity map) equals the kernel VA, so the kernel reads it
/// directly — BUT it is UNTRUSTED, so it is bound-checked against the user window before the deref:
/// a ring-3 caller must not be able to point `buf` at kernel RAM (exfiltration out the console) or
/// at unmapped memory (a ring-0 fault). Full copy_from_user is a later arc; this closes the hole
/// cheaply. Emitted through the standard console path (`serial_print!` -> UART **and** framebuffer
/// mirror) so the line is visible on a serial-less machine (fbcon) too, not only in QEMU's
/// serial.log — the rMBP has no 16550, so a UART-only write would vanish. The demo runs in a
/// BSP-quiet window (see `await_verdict`), so the best-effort console lock is uncontended here and
/// the line is not dropped.
fn sys_write(fd: u64, buf: u64, len: u64) -> i64 {
    // U5x: `fd` is a HANDLE INDEX into the caller's per-process table, not the ambient POSIX stdout. It
    // must resolve to a CONSOLE resource carrying CAP_WRITE. No such handle / wrong kind / missing
    // CAP_WRITE all yield -EACCES — the enforcement point (subsuming U1a's `fd != 1 -> -EBADF`). A
    // printing process is endowed this cap at spawn/launch (`install_console_cap`): the shared window at
    // `setup()` (U1a/U1b/U2) and each spawned child in `sys_spawn`, so every prior print still lands; a
    // process WITHOUT it gets -EACCES (the U5x negative). The pointer validation below is unchanged, so
    // a hostile pointer still yields -EFAULT.
    match handle_resolve(caller_row(), fd, CAP_WRITE) {
        Ok(HandleTarget::Console) => {}
        _ => return EACCES,
    }
    let window = USER_WINDOW_PAGES * PAGE_SIZE;
    let end = buf.wrapping_add(len);
    // Reject overflow and any range not fully inside the user window.
    if end < buf || buf < USER_BASE || end > USER_BASE + window {
        return -14; // -EFAULT
    }
    let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) };
    // The console sink is UTF-8. U1a output is ASCII; for any non-UTF-8 tail, write the valid prefix
    // and report that many bytes (an honest partial write) rather than mangling bytes.
    let (text, written) = match core::str::from_utf8(bytes) {
        Ok(s) => (s, len),
        Err(e) => {
            let v = e.valid_up_to();
            (unsafe { core::str::from_utf8_unchecked(&bytes[..v]) }, v as u64)
        }
    };
    serial_print!("{}", text);
    written as i64
}

/// Per-CPU SYSCALL/SYSRET + NX/SMEP setup. Called once per CPU (BSP in `arch::init`, each AP in
/// `ap_entry`) AFTER `gdt::init_cpu` (STAR needs the selector layout) and `percpu::init_cpu` (GS
/// base). NX absence is fatal (STOP) — the data/stack hardening rides on it; QEMU qemu64 has it.
pub fn init() {
    // NX must exist (CPUID.80000001h:EDX bit 20).
    let ext = core::arch::x86_64::__cpuid(0x8000_0001);
    assert!(ext.edx & (1 << 20) != 0, "U1a: CPU lacks NX (CPUID.80000001h:EDX.NX) — STOP");

    // EFER: enable SYSCALL/SYSRET (SCE) and NX (NXE), preserving LME/LMA (read-modify-write).
    unsafe {
        let mut efer = Msr::new(IA32_EFER);
        let v = efer.read();
        efer.write(v | EFER_SCE | EFER_NXE);
    }
    // STAR: SYSCALL loads CS=0x08 / SS=0x10; SYSRET loads CS=(0x13+16)|3=0x23 / SS=(0x13+8)|3=0x1B.
    unsafe { Msr::new(IA32_STAR).write((0x13u64 << 48) | (0x08u64 << 32)) };
    // LSTAR: the 64-bit SYSCALL entry point.
    LStar::write(VirtAddr::new(unaos_syscall_entry as *const () as u64));
    // SFMASK: interrupts (and TF/DF/AC) cleared on entry.
    unsafe { Msr::new(IA32_FMASK).write(SYSCALL_FLAG_MASK) };
    // KERNEL_GS_BASE = 0: in kernel mode the shadow holds the (zero) user gs. The trampoline's
    // swapgs parks THIS CPU's PerCpuData pointer in the shadow while ring 3 runs; the syscall-entry
    // swapgs brings it back into GS for the handler (`this_cpu()` / the scheduler depend on it).
    unsafe { Msr::new(IA32_KERNEL_GS_BASE).write(0) };

    // SMEP (supervisor can't fetch from ring-3 pages) only if the CPU reports it (CPUID.7.0:EBX bit
    // 7). Never SMAP (bit 21) — pre-Broadwell silicon (the metal Ivy Bridge target) lacks it.
    let leaf7 = core::arch::x86_64::__cpuid_count(7, 0);
    let smep = leaf7.ebx & (1 << 7) != 0;
    if smep {
        unsafe { Cr4::write_raw(Cr4::read_raw() | CR4_SMEP) };
    }
    if !SMEP_LOGGED.swap(true, Ordering::Relaxed) {
        if smep {
            serial_println!(":: SMEP on ::");
        } else {
            serial_println!(":: SMEP unsupported (TCG?) — metal Ivy Bridge has it ::");
        }
    }

    // U2.5 Part 0-ii: clear DR7 once per CPU. The U2 #DB policy (interrupts.rs) resumes a CPL-0 #DB
    // whose RIP lands in the syscall-entry stub, on the assumption that RFLAGS.TF is the ONLY #DB
    // source there. A firmware-left hardware breakpoint (DR7 armed at boot) would violate that and
    // wedge the resume-or-kill logic. Zeroing DR7 per-CPU (here, at syscall init) makes the
    // "TF is the only #DB source" assumption enforced rather than merely assumed. Plain asm, no
    // crate API: `mov dr7, r64` is privileged, touches no memory/stack, and preserves flags.
    unsafe {
        core::arch::asm!("mov dr7, {0}", in(reg) 0u64, options(nomem, nostack, preserves_flags));
    }
    if !DR7_LOGGED.swap(true, Ordering::Relaxed) {
        serial_println!(":: U2.5-0: DR7 cleared ::");
    }
}

/// The ring-3 entry points + shared initial user rsp returned by `setup`. `hello` is the U1a
/// well-behaved program; the three fault fixtures are the U1b kill demo. They may share one user
/// stack: ring 3 is non-preemptible (IF-masked) and the tasks are FIFO on one core, so each runs to
/// completion or death before the next is dispatched.
pub struct UserDemo {
    pub sp: u64,
    pub hello: u64,
    pub wild_write: u64,
    pub code_write: u64,
    pub stack_exec: u64,
    /// U2 Part-0a: the TF+SYSCALL DoS fixture (arms `RFLAGS.TF` then `syscall`).
    pub tf_syscall: u64,
}

/// Map the ring-3 window at USER_BASE (4 fresh 4 KiB pages: code ring3-RX / data + 2 stack RW+NX),
/// copy the program blob into the code page, then flip the code page read-only — never W+X. Call
/// once, after the heap is up and `init` has enabled EFER.NXE. Returns the demo entry + user rsp.
pub fn setup() -> UserDemo {
    // 4-level paging only — `translate` walks four levels; LA57 would misread the tables.
    assert!(Cr4::read_raw() & (1 << 12) == 0, "U1a: 5-level paging (LA57) unsupported");

    // Prove every target page is unmapped first: USER_BASE must not alias kernel memory on this
    // machine's firmware map. A hit here is STOP-worthy (refuse to overwrite kernel state).
    for i in 0..USER_WINDOW_PAGES {
        let va = USER_BASE + i * PAGE_SIZE;
        assert!(
            crate::arch::memory::translate(va).is_none(),
            "U1a: USER_BASE page {:#x} already mapped — refusing to overwrite kernel state",
            va
        );
    }

    // Backing frames (identity-mapped: ptr == phys). Page 0 = code, 1 = data, 2..4 = stack.
    let frames: [u64; 4] = [
        crate::arch::memory::alloc_page_frame(),
        crate::arch::memory::alloc_page_frame(),
        crate::arch::memory::alloc_page_frame(),
        crate::arch::memory::alloc_page_frame(),
    ];

    // Copy the blob into the code frame through its (kernel-writable) identity alias.
    let start = &raw const unaos_user_blob_start as usize;
    let end = &raw const unaos_user_blob_end as usize;
    let blob_len = end - start;
    assert!(blob_len as u64 <= PAGE_SIZE, "U1a: user blob does not fit in the code page");
    unsafe {
        core::ptr::copy_nonoverlapping(start as *const u8, frames[0] as *mut u8, blob_len);
    }

    // All the page-table writes (our new PML4 entry lands in the firmware's read-only PML4 page)
    // run with CR0.WP momentarily cleared.
    crate::arch::memory::with_page_tables_writable(|| unsafe {
        // Page 0 (code): ring3-RX and READ-ONLY from the start — never mapped writable at USER_BASE
        // on ANY core (U1b B4). The blob was copied through the identity alias (`frames[0]`) above,
        // NOT through USER_BASE, so the ring-3 mapping never needs the WRITABLE bit; mapping it RO
        // up front makes a cross-core stale-writable TLB entry structurally impossible — no core can
        // cache a writable USER_BASE PTE that never existed. NX clear = executable at ring 3.
        crate::arch::memory::map_user_page(USER_BASE, frames[0], false, false);
        // Pages 1..4 (data + 2 stack): RW + USER + NX (never executable).
        for i in 1..USER_WINDOW_PAGES {
            crate::arch::memory::map_user_page(
                USER_BASE + i * PAGE_SIZE,
                frames[i as usize],
                true,
                true,
            );
        }
    });

    serial_println!(
        ":: U1a: ring-3 window mapped at {:#x} (code RX-RO / data+stack RW-NX), blob {} bytes ::",
        USER_BASE,
        blob_len
    );

    // U5x: endow the SHARED window (`SHARED_ROW` — where `spawn_user` runs U1a/U1b/U2 with `user_cr3 == 0`,
    // so `current_slot()` is None and `caller_row()` falls back to `SHARED_ROW`) with a console write-
    // capability, so `hello`/`u2-hello`'s `sys_write(fd 1)` still reaches the console once writes route
    // through the table. The shared window is never torn down, so this endowment persists for the whole
    // boot; the U1b fault fixtures share `SHARED_ROW` but never write, so the single fixed cap serves them.
    install_console_cap(SHARED_ROW);

    // Per-routine entry VAs: USER_BASE + (label - blob_start), since the blob is copied to the code
    // page base. `hello` sits at offset 0 (== USER_BASE); every entry lies within the code page.
    let entry_va = |label: *const u8| -> u64 { USER_BASE + (label as usize - start) as u64 };
    UserDemo {
        sp: USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16, // 16-aligned top of the window
        hello: entry_va(&raw const unaos_user_hello),
        wild_write: entry_va(&raw const unaos_user_wild_write),
        code_write: entry_va(&raw const unaos_user_code_write),
        stack_exec: entry_va(&raw const unaos_user_stack_exec),
        tf_syscall: entry_va(&raw const unaos_user_tf_syscall),
    }
}

/// U1a verdict, run on the BSP right after `spawn_user`: block (bounded on the ms-clock) until the
/// ring-3 task terminates, then print one PASS/FAIL line. Deliberately BSP-side rather than a task
/// on another core: the BSP prints nothing while it waits, so the ring-3 task's `SYSCALL` + `hello`
/// lines (printed from its AP) reach the console UNCONTENDED and the whole demo lands contiguously
/// in the boot log — decisive on the serial-less metal target, where the log is photographed off
/// the framebuffer and the best-effort fbcon lock silently drops lines under cross-core contention.
/// `ticks()` advances on the BSP (its timer ISR drives the global ms-clock), so the bound is real
/// even though the BSP is not scheduled; a timeout FAIL (`0/0`) means the round-trip never returned.
pub fn await_verdict() {
    let start = crate::arch::ticks();
    let deadline = start + 2000; // ~2 s at the calibrated 1 kHz; the round-trip is sub-millisecond
    while U1A_EXITED_OK.load(Ordering::Acquire) + U1A_EXITED_ERR.load(Ordering::Acquire) == 0
        && crate::arch::ticks() < deadline
    {
        core::hint::spin_loop();
    }
    let ok = U1A_EXITED_OK.load(Ordering::Acquire);
    let err = U1A_EXITED_ERR.load(Ordering::Acquire);
    if ok == 1 && err == 0 {
        serial_println!(":: U1a: user exited ok (sys_exit 0) -> PASS ::");
    } else {
        serial_println!(
            ":: U1a: FAIL — exited_ok={} exited_err={} (want 1/0; 0/0 = ring-3 task never returned) ::",
            ok,
            err
        );
    }
}

/// U1b verdict, run on the BSP after the three fault fixtures are spawned (the fault→kill mirror of
/// aarch64 M6b's `verdict`). Block (bounded on the ms-clock) until all three fixtures have
/// terminated — each as a kill (expected or unexpected) or, if its fault failed to fire, a
/// survivor `sys_exit(1)` — then print one PASS/FAIL line with the full accounting. BSP-side and
/// BSP-quiet for the same reason as `await_verdict`: the AP's KILL lines reach the (serial-less)
/// framebuffer console uncontended and the demo lands contiguously in the photographed boot log.
///
/// PASS demands the EXACT split — hello exited 0 (from the U1a phase), exactly 3 expected kills,
/// no survivors, no unexpected kills — not merely "all three died". A survivor (fault didn't fire)
/// or a wrong-vector kill (a weakened permission) must both read FAIL; a timeout (a fixture that
/// never faulted AND never exited, i.e. a wedged core) leaves the counts short and also FAILs.
pub fn await_u1b_verdict() {
    let start = crate::arch::ticks();
    let deadline = start + 2000; // ~2 s at the calibrated 1 kHz; each fixture faults sub-millisecond
    loop {
        let exp = RING3_KILLED_EXPECTED.load(Ordering::Acquire);
        let unexp = RING3_KILLED_UNEXPECTED.load(Ordering::Acquire);
        let survivors = U1A_EXITED_ERR.load(Ordering::Acquire);
        // The three fixtures each terminate as a kill or a survivor exit; wait until all accounted.
        if exp + unexp + survivors >= 3 || crate::arch::ticks() >= deadline {
            break;
        }
        core::hint::spin_loop();
    }
    let ok = U1A_EXITED_OK.load(Ordering::Acquire);
    let survivors = U1A_EXITED_ERR.load(Ordering::Acquire);
    let exp = RING3_KILLED_EXPECTED.load(Ordering::Acquire);
    let unexp = RING3_KILLED_UNEXPECTED.load(Ordering::Acquire);
    if ok == 1 && exp == 3 && survivors == 0 && unexp == 0 {
        serial_println!(
            ":: U1b: fault isolation — exited=1 killed=3 (all expected vecs), kernel alive -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U1b: fault isolation FAIL — exited_ok={} survivor_exits={} killed_expected={} killed_unexpected={} (want 1/0/3/0) ::",
            ok, survivors, exp, unexp
        );
    }
}

// ---------------------------------------------------------------------------------------------
// U2 Part-0 boundary fixtures + the FAT loader.
// ---------------------------------------------------------------------------------------------

/// U2 Part-0c: the callable twin of the syscall stub's inline canonical-`rcx` guard
/// (`mov rdx,rcx; shl 16; sar 16; cmp rdx,rcx`). Returns true iff `rcx` is a canonical 48-bit VA —
/// bits 63:47 all equal bit 47 (sign-extension of bit 47). The LIVE guard is the asm in
/// `unaos_syscall_entry`; this predicate implements the identical check so the refusal logic can be
/// unit-exercised kernel-side — the end-to-end refusal needs ring-3 code returning to a non-canonical
/// RIP, which is unreachable from our 1 TiB window. Assumes LA57 off (48-bit VAs), which `setup()`
/// asserts.
pub fn rcx_canonical(rcx: u64) -> bool {
    ((rcx as i64) << 16 >> 16) as u64 == rcx
}

/// U2 Part-0c fixture: fire a self-NMI through the REAL IPI path and confirm it was taken on the
/// dedicated NMI IST stack (the honest B3 evidence — "taken on IST", not merely "slot installed").
/// Uses `apic::send_ipi(<own apic id>, 0x4400)`: physical destination = this CPU's APIC id, `0x4400`
/// = level-assert (bit 14) | NMI delivery mode (bits 10:8 = 100b). The Self shorthand is INVALID with
/// NMI delivery per the SDM's valid-ICR-combinations table, and `send_ipi` already handles the
/// x2APIC-vs-xAPIC dispatch, so we never open-code an ICR write. Runs on the BSP after the local APIC
/// + NMI IST are up. Prints one PASS/FAIL line.
pub fn nmi_self_fire() {
    let before = crate::arch::interrupts::NMI_COUNT.load(Ordering::Acquire);
    let dest = crate::arch::apic::apic_id_u32();
    crate::arch::apic::send_ipi(dest, 0x4400); // level-assert | NMI delivery mode
    // A self-NMI is delivered at the next instruction boundary (NMIs ignore IF), so a short bounded
    // spin suffices; the cap is a safety net, not the expected exit (keeps this IF-independent).
    let mut spins = 0u64;
    while crate::arch::interrupts::NMI_COUNT.load(Ordering::Acquire) == before && spins < 10_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }
    let took = crate::arch::interrupts::NMI_COUNT.load(Ordering::Acquire) > before;
    let on_ist = crate::arch::interrupts::NMI_ON_IST.load(Ordering::Acquire);
    if took && on_ist {
        serial_println!(":: U2-0c: self-NMI taken on IST -> PASS ::");
    } else {
        serial_println!(":: U2-0c: self-NMI FAIL — delivered={} on_ist={} ::", took, on_ist);
    }
}

/// U2 Part-0c fixture: exercise the canonical-`rcx` guard's logic kernel-side. Asserts the predicate
/// REFUSES the canonical-boundary value `0x8000_0000_0000_0000` (bit 63 set, bit 47 clear) and
/// ACCEPTS a canonical address. Prints one PASS/FAIL line.
pub fn canonical_guard_selftest() {
    let bad = 0x8000_0000_0000_0000u64; // non-canonical
    let good = USER_BASE; // canonical (low half)
    if !rcx_canonical(bad) && rcx_canonical(good) {
        serial_println!(":: U2-0c: canonical-rcx guard refuses 0x8000_0000_0000_0000 -> PASS ::");
    } else {
        serial_println!(
            ":: U2-0c: canonical-rcx guard FAIL — refuses_bad={} accepts_good={} ::",
            !rcx_canonical(bad),
            rcx_canonical(good)
        );
    }
}

/// U2 Part-0a verdict (BSP-quiet, mirrors `await_verdict`): wait until the TF+SYSCALL fixture has
/// TERMINATED — either resumed+exited (the #DB neutralized at the entry stub) or killed (a ring-3
/// #DB) — then confirm the kernel is still alive and print PASS. A timeout (the fixture neither
/// exited nor was killed = a wedged core, i.e. the DoS was NOT neutralized) is the only FAIL. The
/// resumed-vs-killed split and the #DB-resume count are reported for the honest QEMU-vs-metal record.
pub fn await_u2_0a_verdict() {
    let deadline = crate::arch::ticks() + 2000;
    while U2_0A_EXITED_OK.load(Ordering::Acquire) + U2_0A_KILLED.load(Ordering::Acquire) == 0
        && crate::arch::ticks() < deadline
    {
        core::hint::spin_loop();
    }
    let exited = U2_0A_EXITED_OK.load(Ordering::Acquire);
    let killed = U2_0A_KILLED.load(Ordering::Acquire);
    let resumed = crate::arch::interrupts::DB_TF_RESUMED.load(Ordering::Acquire);
    if exited + killed >= 1 {
        serial_println!(
            ":: U2-0a: TF+SYSCALL survived -> PASS (db_resumed={} exited={} killed={}) ::",
            resumed, exited, killed
        );
    } else {
        serial_println!(
            ":: U2-0a: TF+SYSCALL FAIL — fixture never terminated (db_resumed={} exited={} killed={}) — DoS not neutralized ::",
            resumed, exited, killed
        );
    }
}

/// U2: load a ring-3 program from the FAT volume and run it. ONE-SHOT, fired from the main loop once
/// a block device is present (mirrors `fat::probe_once`'s gate) — NOT co-located with the pre-xHCI
/// U1a/U1b demo, whose window it reuses, because `fat::mount()` needs the usb-storage block device
/// that enumerates asynchronously in the main loop.
///
/// Flow: mount the FAT volume, find + read `HELLO.BIN` (capped at one code page), validate its size,
/// copy it into the RO-from-start code page at `USER_BASE` **through the identity alias** (the U1b B4
/// W^X discipline — the ring-3 mapping is never writable, so W^X holds across cores by construction),
/// then drop it to ring 3 on a scheduled AP. The loaded bytes are UNTRUSTED: nothing about them is
/// trusted beyond the size bound — the program runs only under ring-3 + NX + W^X + SMEP + the U1b
/// fault-kill net, which is the point. A missing volume / file / oversize logs one clean line + skips
/// (the U1a/U1b demos, which ran earlier, are unaffected).
pub fn u2_probe_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    // Gate: storage enumerated (same as fat::probe_once) AND a scheduled AP to run ring 3 on
    // (spawn_user needs a core running the scheduler loop; the BSP is never scheduled).
    if crate::drivers::block::info().is_none() {
        return;
    }
    let online = crate::arch::smp::online_aps();
    let Some(&cpu) = online.first() else {
        DONE.store(true, Ordering::Relaxed);
        serial_println!(":: U2: no application processor online — loader skipped ::");
        return;
    };
    DONE.store(true, Ordering::Relaxed); // one-shot from here regardless of outcome

    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => {
            serial_println!(":: U2: no FAT volume ({:?}) — loader skipped ::", e);
            return;
        }
    };
    let de = match fs.find_in_root("HELLO.BIN") {
        Ok(de) => de,
        Err(_) => {
            serial_println!(":: U2: HELLO.BIN not found on the FAT volume — loader skipped ::");
            return;
        }
    };
    // The program is untrusted: reject anything that does not fit the single ring-3 code page BEFORE
    // reading, from the ON-DISK directory size. `read_file` caps the copy at min(de.size, cap), so a
    // post-read length check could never SEE an oversize file — it would silently truncate then run
    // it, violating the "reject oversized" invariant. Gate on `de.size` instead.
    if de.size == 0 || de.size as u64 > PAGE_SIZE {
        serial_println!(
            ":: U2: HELLO.BIN bad size {} bytes (must be 1..={}) — loader skipped ::",
            de.size,
            PAGE_SIZE
        );
        return;
    }
    // Read the (now known to fit) program into a heap buffer. A short read — a malformed chain ending
    // before `de.size` — yields fewer bytes, still bounded and, guarded below, non-empty; the
    // untrusted bytes run only under ring-3 + NX + W^X + SMEP + the fault-kill net.
    let mut bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if let Err(e) = fs.read_file(&de, &mut bytes, PAGE_SIZE as usize) {
        serial_println!(":: U2: HELLO.BIN read error ({:?}) — loader skipped ::", e);
        return;
    }
    if bytes.is_empty() {
        serial_println!(":: U2: HELLO.BIN read empty — loader skipped ::");
        return;
    }
    // The USER_BASE window was mapped by `setup()` during the U1a/U1b demo (code page RO-from-start).
    // U2.5 Part 0-iii: zero the ENTIRE window — code, data, and both stack pages — before copying the
    // program in, so a second loaded program can never read U1a/U1b fixture residue left in the
    // data/stack pages (the earlier code zeroed only the code page). The four frames are NOT
    // physically contiguous, so translate each page's VA to its own frame; a None on any page takes
    // the existing "window not mapped" skip. Zero (and later copy) go through the identity alias —
    // never through USER_BASE — so the ring-3 code mapping stays read-only (W^X across cores by
    // construction). Page 0 (== USER_BASE) is the code page; its frame is captured for the copy below.
    let mut code_frame: u64 = 0;
    for i in 0..USER_WINDOW_PAGES {
        let va = USER_BASE + i * PAGE_SIZE;
        let Some(frame) = crate::arch::memory::translate(va) else {
            serial_println!(":: U2: USER_BASE window not mapped — loader skipped ::");
            return;
        };
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE as usize);
        }
        if i == 0 {
            code_frame = frame;
        }
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), code_frame as *mut u8, bytes.len());
    }
    let sp = USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16;
    serial_println!(":: U2: HELLO.BIN loaded from FAT ({} bytes) -> ring 3 ::", bytes.len());
    crate::arch::sched::spawn_user("u2-hello", USER_BASE, sp, cpu);
    // Async verdict on a sibling core if available (else the same AP, FIFO after u2-hello): reports
    // the loaded program's clean exit without blocking the interactive main loop.
    let vcpu = online.get(1).copied().unwrap_or(cpu);
    crate::arch::sched::spawn("u2-verdict", u2_verdict, 0, vcpu, crate::arch::sched::PRIO_NORMAL);
}

/// U2 verdict task: wait (bounded on the BSP-driven ms-clock) for the loaded HELLO.BIN to terminate,
/// then print PASS/FAIL. A scheduled kernel thread (not the BSP), polling via `yield_now` so a
/// co-located u2-hello runs to completion.
fn u2_verdict(_: usize) {
    let deadline = crate::arch::ticks() + 2000;
    while U2_EXITED_OK.load(Ordering::Acquire) + U2_EXITED_ERR.load(Ordering::Acquire) == 0
        && crate::arch::ticks() < deadline
    {
        crate::arch::sched::yield_now();
    }
    let ok = U2_EXITED_OK.load(Ordering::Acquire);
    let err = U2_EXITED_ERR.load(Ordering::Acquire);
    if ok == 1 && err == 0 {
        serial_println!(":: U2: loaded program exited ok -> PASS ::");
    } else {
        serial_println!(
            ":: U2: FAIL — exited_ok={} exited_err={} (want 1/0; 0/0 = program never returned or was killed) ::",
            ok, err
        );
    }
    // U4x: signal that U2 is fully done (verdict printed) so the U4x launcher orders its lines AFTER
    // U2's — the x86 twin of aarch64 U4 gating on `M6G_LOADER_DONE`. Set on BOTH the PASS and FAIL
    // paths (U4x waits bounded on it, so a U2 FAIL still lets U4x run rather than hang).
    U2_DONE.store(true, Ordering::Release);
}

/// U3 Part A: prove per-process CR3 isolation with a DETERMINISTIC KERNEL probe (no ring 3). Allocate
/// two address-space slots, plant DISTINCT sentinels at the same user VA in each, then swap CR3 to
/// each and read that VA — confirming each reads its own. This is the metal-catchable proof (the x86
/// twin of M6d's nG probe) that two processes' USER_BASE windows are isolated; it runs on the BSP at
/// boot before any per-process ring-3 task. One-shot; frees both slots when done.
pub fn u3_probe_once() {
    const SENT_A: u64 = 0xA5A5_A5A5_0000_00A1;
    const SENT_B: u64 = 0x5A5A_5A5A_0000_00B2;
    let mut slots = [0usize; 2];
    if !crate::arch::memory::alloc_user_spaces(&mut slots) {
        serial_println!(":: U3: address-space pool exhausted — isolation probe skipped ::");
        return;
    }
    let (a, b) = (slots[0], slots[1]);
    let (a_val, b_val, ok) = crate::arch::memory::probe_isolation(a, b, 0, SENT_A, SENT_B);
    if ok {
        serial_println!(
            ":: U3: per-process CR3 isolation (A={:#018x} B={:#018x} distinct) -> PASS ::",
            a_val, b_val
        );
    } else {
        serial_println!(
            ":: U3: per-process CR3 isolation FAIL (A={:#018x} want {:#018x}; B={:#018x} want {:#018x}) ::",
            a_val, SENT_A, b_val, SENT_B
        );
    }
    crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(a));
    crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(b));
}

// U3 ring-3 fixture sentinels (distinct per task; the low byte tags the task). Planted in each
// slot's data page (window page 1, offset 0); `u3_reader` copies it to the readback word (offset 8).
const U3_SENT_A: u64 = 0xC3C3_C3C3_0000_000A;
const U3_SENT_B: u64 = 0x3C3C_3C3C_0000_000B;
const U3_DATA_OFF: usize = 0x1000; // window page 1 (data) — sentinel at +0, readback at +8

/// U3 Part B/C: TWO ring-3 tasks, each in its OWN private address space (CR3), each running the
/// `u3_reader` blob — read the slot-private sentinel at USER_BASE+PAGE, write it to USER_BASE+PAGE+8,
/// `sys_exit(0)`. This exercises the per-process CR3 DISPATCH (the trampoline installs the task's CR3
/// before `iretq`) and TEARDOWN (`exit` restores the kernel CR3 + frees the slot) end to end: a task
/// that (wrongly) ran under the shared window would read the wrong sentinel, so the readback verdict
/// catches it. Runs on `cpu` (a scheduled AP), FIFO (ring 3 is cooperative). One-shot.
pub fn u3_run_fixture(cpu: usize) {
    let mut slots = [0usize; 2];
    if !crate::arch::memory::alloc_user_spaces(&mut slots) {
        serial_println!(":: U3: address-space pool exhausted — ring-3 fixture skipped ::");
        return;
    }
    let sents = [U3_SENT_A, U3_SENT_B];
    let blob_start = &raw const unaos_user_blob_start as usize;
    let blob_end = &raw const unaos_user_blob_end as usize;
    let blob_len = blob_end - blob_start;
    let reader_entry = USER_BASE + (&raw const unaos_user_u3_reader as usize - blob_start) as u64;
    let sp = USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16;

    for (i, &s) in slots.iter().enumerate() {
        let backing = crate::arch::memory::slot_backing_ptr(s);
        unsafe {
            // Copy the blob into the slot's code page (page 0) through the kernel identity alias —
            // never through USER_BASE, so the code mapping stays read-only (W^X). Plant this slot's
            // private sentinel in the data page and clear its readback word.
            core::ptr::copy_nonoverlapping(blob_start as *const u8, backing, blob_len);
            core::ptr::write_volatile(backing.add(U3_DATA_OFF) as *mut u64, sents[i]);
            core::ptr::write_volatile(backing.add(U3_DATA_OFF + 8) as *mut u64, 0);
        }
    }

    U3_EXITED.store(0, Ordering::Release);
    let names = ["u3-space-a", "u3-space-b"];
    for (i, &s) in slots.iter().enumerate() {
        crate::arch::sched::spawn_user_in_space(
            names[i],
            reader_entry,
            sp,
            cpu,
            crate::arch::memory::slot_cr3(s),
        );
    }

    // Wait (bounded on the BSP-driven ms-clock) for both tasks to exit.
    let deadline = crate::arch::ticks() + 2000;
    while U3_EXITED.load(Ordering::Acquire) < 2 && crate::arch::ticks() < deadline {
        core::hint::spin_loop();
    }

    // Verify each slot's readback equals ITS OWN sentinel (each task ran in its own space). The slots
    // were freed by each task's `exit` teardown, but the static backing persists until re-alloc, so
    // reading it here through the identity alias is valid.
    let mut readbacks = [0u64; 2];
    let mut ok = U3_EXITED.load(Ordering::Acquire) == 2;
    for (i, &s) in slots.iter().enumerate() {
        let backing = crate::arch::memory::slot_backing_ptr(s);
        let rb = unsafe { core::ptr::read_volatile(backing.add(U3_DATA_OFF + 8) as *const u64) };
        readbacks[i] = rb;
        if rb != sents[i] {
            ok = false;
        }
    }
    if ok {
        serial_println!(
            ":: U3: 2 ring-3 tasks each in a private CR3 read their own sentinel (A={:#018x} B={:#018x}) -> PASS ::",
            readbacks[0], readbacks[1]
        );
    } else {
        serial_println!(
            ":: U3: ring-3 per-process CR3 FAIL — exited={} A={:#018x}/want {:#018x} B={:#018x}/want {:#018x} ::",
            U3_EXITED.load(Ordering::Acquire), readbacks[0], sents[0], readbacks[1], sents[1]
        );
    }
}

// ---------------------------------------------------------------------------------------------
// U3.5 — preemptible ring 3 (the x86 twin of aarch64 M6e). Completes the U3 process abstraction:
// a ring-3 task can now be dropped PREEMPTIBLE (RFLAGS.IF set), so the timer evicts it and other
// work shares its core — closing the one-core DoS a never-syscalling program was, and letting the
// per-process CR3 travel through the general scheduler DISPATCH path (not just first entry).
// ---------------------------------------------------------------------------------------------

/// Byte offset of the spinner's forward-progress counter within the slot backing: window page 1
/// (data), offset 0 — matches `unaos_user_u3_5_spinner` writing `[USER_BASE + PAGE]`.
const U3_5_COUNTER_OFF: usize = 0x1000;
/// Steps the kernel co-task takes (each with a sleep) before exiting — the DoS-fix witness.
const U3_5_COTASK_STEPS: u32 = 8;
/// U3.5 co-task progress: bumped once per step. Reaching `U3_5_COTASK_STEPS` proves the spinner did
/// NOT wedge the core (the co-task got CPU time via preemption).
static U3_5_COTASK_PROGRESS: AtomicU32 = AtomicU32::new(0);

/// U3.5 kernel co-task pinned to the SPINNER'S core: take `U3_5_COTASK_STEPS` steps, sleeping between
/// them so it time-shares the core with the preemptible spinner, then exit. Under cooperative ring 3
/// this task would NEVER run (the spinner would hog the core forever); its completion is the proof the
/// DoS is fixed. A KERNEL task (not a second ring-3 task) by design — one user task per core keeps
/// TSS.RSP0 valid without a dispatch-time RSP0 install (an M7-twin concern, out of this arc's scope).
fn u3_5_cotask(_: usize) {
    for _ in 0..U3_5_COTASK_STEPS {
        U3_5_COTASK_PROGRESS.fetch_add(1, Ordering::AcqRel);
        crate::arch::sched::sleep_ticks(2);
    }
}

/// U3.5: prove PREEMPTIBLE ring 3 end to end. Spawn a preemptible ring-3 spinner (`jmp`-loop that
/// bumps a private-CR3 counter and never syscalls) plus a KERNEL co-task on the SAME core `cpu`, then
/// (BSP-side, bounded on the ms-clock) confirm the three properties the arc must deliver:
///   (a) the timer PREEMPTED the spinner — `interrupts::IRQS_AT_RING3 > 0` (the metal-only truth
///       cooperative ring 3 can never show); gate on `> 0`, not an exact count (TCG under-delivers);
///   (b) the co-task RAN to completion — the spinner no longer wedges the core (the DoS fix);
///   (c) the spinner RESUMES correctly across preemptions — its private-CR3 counter keeps CLIMBING
///       (a task resumed under the wrong CR3, or with a corrupt register file, would stop/misbehave).
/// Then the watchdog REAPS the spinner via the scheduler `KillSwitch` (it never exits on its own) and
/// confirms it terminated. Prints one PASS/FAIL line. One-shot; runs after `u3_run_fixture`, so the
/// address-space pool is free and this is the LAST ring-3 fixture (nothing after it relies on the AP's
/// cooperative FIFO ordering). `cpu` is a scheduled AP.
pub fn u3_5_run_fixture(cpu: usize) {
    let Some(slot) = crate::arch::memory::alloc_user_space() else {
        serial_println!(":: U3.5: address-space pool exhausted — preemption fixture skipped ::");
        return;
    };
    let cr3 = crate::arch::memory::slot_cr3(slot);
    let backing = crate::arch::memory::slot_backing_ptr(slot);
    let blob_start = &raw const unaos_user_blob_start as usize;
    let blob_end = &raw const unaos_user_blob_end as usize;
    let blob_len = blob_end - blob_start;
    let spin_entry = USER_BASE + (&raw const unaos_user_u3_5_spinner as usize - blob_start) as u64;
    let sp = USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16;

    // Copy the blob into the slot's code page (page 0) through the kernel identity alias — never
    // through USER_BASE, so the code mapping stays read-only (W^X) — and zero the counter word.
    unsafe {
        core::ptr::copy_nonoverlapping(blob_start as *const u8, backing, blob_len);
        core::ptr::write_volatile(backing.add(U3_5_COUNTER_OFF) as *mut u64, 0);
    }

    U3_5_COTASK_PROGRESS.store(0, Ordering::Release);
    let irqs_before = crate::arch::interrupts::IRQS_AT_RING3.load(Ordering::Acquire);
    let kill = alloc::sync::Arc::new(crate::arch::sched::KillSwitch::new());

    // Co-task first (queued at the spinner's priority), then the preemptible spinner — both on `cpu`.
    serial_println!(":: U3.5: preemptible-ring-3 demo — spinner + co-task on core {} ::", cpu);
    crate::arch::sched::spawn("u3.5-cotask", u3_5_cotask, 0, cpu, crate::arch::sched::PRIO_NORMAL);
    crate::arch::sched::spawn_user_preemptible("u3.5-spinner", spin_entry, sp, cpu, cr3, kill.clone());

    // (a)+(b): wait until the timer has preempted the spinner AND the co-task finished AND the spinner
    // has made some progress. Bounded on the BSP-driven ms-clock (the BSP's timer advances `ticks()`
    // while it spins here, as the U1a/U2/U3 verdicts also rely on).
    let read_counter = || unsafe { core::ptr::read_volatile(backing.add(U3_5_COUNTER_OFF) as *const u64) };
    let deadline = crate::arch::ticks() + 4000;
    loop {
        let irqs = crate::arch::interrupts::IRQS_AT_RING3.load(Ordering::Acquire) - irqs_before;
        let cot = U3_5_COTASK_PROGRESS.load(Ordering::Acquire);
        if (irqs > 0 && cot >= U3_5_COTASK_STEPS && read_counter() > 0) || crate::arch::ticks() >= deadline {
            break;
        }
        core::hint::spin_loop();
    }

    // (c): the spinner is still running (not yet reaped). Sample its counter across a short bounded
    // window and confirm it CLIMBED — forward progress over (necessarily several quanta of) preemptions
    // proves each eviction was correctly resumed. The window spans many quanta at 1 kHz; the counter
    // increments at CPU speed, so any live spinner grows it by a huge margin.
    let progress_1 = read_counter();
    let obs_deadline = crate::arch::ticks() + 100;
    while crate::arch::ticks() < obs_deadline {
        core::hint::spin_loop();
    }
    let progress_2 = read_counter();
    let resumed = progress_1 > 0 && progress_2 > progress_1;

    // Reap the spinner via the scheduler (it never exits) and confirm it terminated within the bound.
    kill.request();
    let kdeadline = crate::arch::ticks() + 2000;
    while !kill.is_reaped() && crate::arch::ticks() < kdeadline {
        core::hint::spin_loop();
    }
    let reaped = kill.is_reaped();

    let irqs = crate::arch::interrupts::IRQS_AT_RING3.load(Ordering::Acquire) - irqs_before;
    let cot = U3_5_COTASK_PROGRESS.load(Ordering::Acquire);
    if irqs > 0 && cot >= U3_5_COTASK_STEPS && resumed && reaped {
        serial_println!(
            ":: U3.5: ring-3 preemption — IRQs-at-ring3={}, co-task ran, spinner resumed -> PASS ::",
            irqs
        );
    } else {
        serial_println!(
            ":: U3.5: ring-3 preemption FAIL — irqs={} cotask={}/{} progress={}->{} reaped={} ::",
            irqs, cot, U3_5_COTASK_STEPS, progress_1, progress_2, reaped
        );
    }

    // Slot lifecycle: on the PASS path the scheduler's reap already restored the kernel CR3 + freed
    // the slot. On a reap TIMEOUT (`!reaped`), the spinner never self-exits (it never syscalls), so it
    // is still ALIVE and running under `cr3` — freeing that slot here would be unsafe (a later
    // `alloc_user_space` could rebuild a live address space underfoot) and could double-free against a
    // late scheduler reap. Leaking one of the 8 slots is harmless: U3.5 is the LAST ring-3 fixture, and
    // a timeout is already a hard FAIL. So we deliberately do NOT free here.
}

// =============================================================================================
// U4x — the x86 process model: sys_spawn (a spawner loads+runs a child from storage, returns a
// HANDLE) + sys_wait (reap by handle) + a per-process handle table. The x86 twin of aarch64's M7/U4;
// it adopts that arc's SETTLED design directly. It completes the process abstraction U3/U3.5 began:
// a parent runs a child program in its OWN address space and reaps it by an owner-scoped HANDLE.
//
// Two static tables, deliberately SEPARATE and complementary (the aarch64 U4 design):
//   * PROCS   — keyed by pid: the process control blocks (exit `status`/`state`/`done`).
//   * HANDLES — keyed by the caller's address-space SLOT (x86 has no architectural ASID, so the slot
//     index plays the role aarch64's ASID does): the spawner's PRIVATE namespace of child
//     capabilities. A handle value is 0 (Empty) or the child's pid (the PROCS key). `sys_spawn`
//     returns the small handle INDEX; `sys_wait` takes it.
//
// Single-writer invariant: exactly one live task runs under any given slot (one task per slot, torn
// down before reuse) and its syscalls are serialized (one at a time, IRQ-masked), so a given
// `HANDLES[slot]` row is only ever touched by its own task. The atomics carry memory ordering
// (publish the pid with Release; a handle read Acquires it), not cross-task contention.
//
// STORAGE / IF NOTE (the load-bearing x86 divergence from the aarch64 twin): aarch64's `sys_spawn`
// reads HELLO.BIN off the SD card synchronously INSIDE the SVC handler because its EMMC2 driver is
// PIO (no interrupts). x86 storage is USB mass-storage over xHCI, whose BOT read pump `hlt()`s to
// await the async completion (`xhci::pump_until_bot_done`) — and `hlt` with IF=0 hangs forever. The
// SYSCALL handler runs IF-masked (SFMASK clears IF), so it CANNOT do a BOT read. So the FAT read is
// hoisted OUT of the syscall: the child program is pre-staged off FAT ONCE by `stage_hello` on the
// BSP main loop (IF=1 — the proven U2 read path), and `sys_spawn` instantiates each child by a pure
// memcpy of the staged bytes into a fresh slot (IF=0-safe). The child still runs storage-loaded
// HELLO.BIN bytes gated on storage being present — the observable behavior is the aarch64 twin's.
//
// SCOPE NOTE (deferred to U5, mirroring the aarch64 U4 note): a handle row is NOT cleared when its
// slot is torn down (`exit`/reap free the slot in sched.rs, out of this file's bookkeeping). U4x
// relies on reapers CONSUMING their handles (`sys_wait` clears on reap), so a well-behaved process
// leaves an empty row at exit and the demo is clean by construction (the parent reaps both children;
// the orphan spawns nothing; parent/orphan/children hold DISTINCT slots while alive, and only the
// parent ever WRITES a row). A capability CHECK at this lookup + grant/attenuate/revoke +
// teardown-clear is U5.
// =============================================================================================

// Negative errnos returned to ring 3 by sys_spawn/sys_wait (Linux values; arch-independent in Linux).
// These never appear in the demo's serial output — the parent only tests the SIGN of the spawn return
// and compares the wait return to -ECHILD — but are named for a future real userspace. (The FAT-read
// failure modes ENOENT/EIO/E2BIG the aarch64 twin maps are handled here at stage time — a failed
// `stage_hello` skips the whole demo — so `sys_spawn` never surfaces them.)
const ECHILD: i64 = -10; // sys_wait: no child with that handle in the caller's table
const EAGAIN: i64 = -11; // the process table (or slot pool, or handle table) is full
const ENODEV: i64 = -19; // no program staged (no block device / no HELLO.BIN at stage time)
// U5x capability errnos.
const EACCES: i64 = -13; // a capability check failed — no such handle / wrong kind / missing right / amplify
const EINVAL: i64 = -22; // a SYS_CAP sub-op selector that is neither GRANT nor REVOKE

/// A killed child's Proc status: a nonzero sentinel the child-KILL path stores so a killed child still
/// WAKES its parent's sys_wait — but with status != 0, so the parent's witness reads FAIL (a killed
/// child is a U4x bug; a clean child exits 0). Used only on a kill.
const U4X_KILLED_STATUS: i32 = 0x4B; // 'K'

// --- Pre-staged child program (HELLO.BIN), read ONCE off FAT by `stage_hello` on the BSP (IF=1) and
// copied into each fresh child slot by `sys_spawn` (a pure memcpy, IF=0-safe). Written once, before
// any sys_spawn, then read-only — published via `HELLO_STAGED` (Release/Acquire). One code page max
// (the ring-3 window's code page size), matching the U2 size bound. ---
static mut HELLO_BYTES: [u8; PAGE_SIZE as usize] = [0; PAGE_SIZE as usize];
static HELLO_LEN: AtomicUsize = AtomicUsize::new(0);
static HELLO_STAGED: AtomicBool = AtomicBool::new(false);

/// Load HELLO.BIN off the FAT volume into the pre-stage buffer. Called ONCE from `u4x_probe_once` on
/// the BSP main loop (IF=1 — the proven U2 read path; see the STORAGE / IF NOTE above). Returns true
/// iff a valid, non-empty, size-bounded program was staged; a missing volume/file/oversize/read-error
/// returns false (the caller then skips the U4x demo cleanly). The bytes are UNTRUSTED — nothing is
/// trusted beyond the one-page size bound; each child runs them only under ring-3 + NX + W^X + SMEP +
/// the U1b fault-kill net.
fn stage_hello() -> bool {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(_) => return false,
    };
    let de = match fs.find_in_root("HELLO.BIN") {
        Ok(de) => de,
        Err(_) => return false,
    };
    // Reject up-front from the ON-DISK directory size (the U2 truncation lesson): `read_file` caps the
    // copy at min(de.size, cap), so a post-read length check could never SEE an oversize file.
    if de.size == 0 || de.size as u64 > PAGE_SIZE {
        return false;
    }
    let mut bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs.read_file(&de, &mut bytes, PAGE_SIZE as usize).is_err() || bytes.is_empty() {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), (&raw mut HELLO_BYTES).cast::<u8>(), bytes.len());
    }
    HELLO_LEN.store(bytes.len(), Ordering::Release);
    HELLO_STAGED.store(true, Ordering::Release); // publish the bytes (a later sys_spawn Acquires this)
    true
}

// --- The process table (pid-keyed): the parent's view of its children's lifecycle. ---
const PFREE: u8 = 0; // entry unused
const PRUNNING: u8 = 1; // claimed; a child is (or is about to be) running under `pid`
const PEXITED: u8 = 2; // the child exited/was killed; `status` is valid, awaiting reap by sys_wait

/// A spawned child's process control block. Static so it OUTLIVES the child's `Task` Box (freed on
/// exit) and its slot teardown — the reap accounting must survive both. `done` is posted exactly once
/// by the child (its exit OR its kill path) and waited exactly once by the parent's sys_wait, so a
/// reaped-then-reused entry always starts at 0 permits (no drain needed) — the M4 balance discipline.
struct Proc {
    /// The child task id; the sys_wait key. 0 while an entry is FREE or a claim's pid is not yet stored.
    pid: AtomicU64,
    /// The child's exit status; valid once `state == PEXITED`.
    status: AtomicI32,
    /// FREE / RUNNING / EXITED — the ownership + lifecycle token (CAS'd FREE->RUNNING to claim).
    state: AtomicU8,
    /// Posted once by the child (SYS_EXIT or the kill path), awaited once by the parent's sys_wait. A
    /// scheduler-post wake, so sys_wait works under QEMU (unlike a timer-driven sleep).
    done: crate::arch::sched::Semaphore,
}
/// A small cap « USER_SLOTS: if it exhausts, sys_spawn returns -EAGAIN, never grows the slot pool.
const MAX_PROCS: usize = 4;
static PROCS: [Proc; MAX_PROCS] = [const {
    Proc {
        pid: AtomicU64::new(0),
        status: AtomicI32::new(0),
        state: AtomicU8::new(PFREE),
        done: crate::arch::sched::Semaphore::new(0),
    }
}; MAX_PROCS];

/// Claim a FREE Proc entry, returning its index. The CAS on `state` (FREE->RUNNING) is the atomic
/// ownership token; the pid=0 placeholder is overwritten with the real child pid (Release) by
/// `sys_spawn` AFTER the child is spawned (the child cannot be dispatched until the parent yields, so
/// the real pid is always in place before any lookup). `None` if the table is full (-> -EAGAIN).
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

/// Find the RUNNING Proc entry whose pid matches — the child-exit / child-kill lookup. Called with a
/// live task id (`> 0`), so it never spuriously matches a fresh claim's pid=0 placeholder.
fn proc_find_running(pid: u64) -> Option<usize> {
    (0..MAX_PROCS).find(|&i| {
        PROCS[i].state.load(Ordering::Acquire) == PRUNNING
            && PROCS[i].pid.load(Ordering::Acquire) == pid
    })
}

/// Find the non-FREE (RUNNING or EXITED) Proc entry whose pid matches — the sys_wait lookup. `None`
/// => the caller has no such child.
fn proc_find_child(pid: u64) -> Option<usize> {
    (0..MAX_PROCS).find(|&i| {
        PROCS[i].state.load(Ordering::Acquire) != PFREE
            && PROCS[i].pid.load(Ordering::Acquire) == pid
    })
}

/// Release a Proc entry to FREE — after reaping in sys_wait, or unwinding a failed sys_spawn claim.
fn proc_free(i: usize) {
    PROCS[i].pid.store(0, Ordering::Release);
    PROCS[i].state.store(PFREE, Ordering::Release);
}

// --- The per-process handle table (slot-keyed): the ownership namespace. ---
const NHANDLE: usize = 8; // handle slots per process (small, static — like MAX_PROCS)
/// `RESERVING` marks a handle slot claimed by an in-flight `sys_spawn` before the real child pid is
/// known (0 = Empty would let a re-scan re-claim it; a real pid is never `u64::MAX`). Overwritten with
/// the pid once the child is spawned, or cleared if the load fails — never observed by another task.
const HANDLE_RESERVING: u64 = u64::MAX;
/// The handle-table row for the SHARED kernel window — the x86 twin of aarch64 ASID 0's row. x86 keys
/// `HANDLES` by address-space SLOT, but the U1a/U1b/U2 fixtures run in the shared window (`user_cr3 == 0`,
/// so `current_slot()` is None and they have no private slot). One extra row (index `USER_SLOTS`) gives
/// them a home for the console cap `setup()` endows; `caller_row()` maps None -> `SHARED_ROW`. This row
/// is never torn down, so its endowment persists for the whole boot.
const SHARED_ROW: usize = crate::arch::memory::USER_SLOTS;
/// `HANDLES[row][idx]`: 0 (Empty), a child pid (`Child`), `HANDLE_CONSOLE` (the console resource), or
/// `HANDLE_RESERVING` (an in-flight `sys_spawn` reservation). `USER_SLOTS + 1` rows: one per private slot
/// plus `SHARED_ROW`.
static HANDLES: [[AtomicU64; NHANDLE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NHANDLE] }; crate::arch::memory::USER_SLOTS + 1];

// =============================================================================================
// U5x — handles as CAPABILITIES: rights, a resource target beyond "child pid", the enforcement CHECK,
// grant/attenuate/revoke, and a teardown-clear. The aarch64 U5 (`arch/aarch64/syscall.rs`) twin, keyed
// by address-space SLOT instead of ASID. U4x built the STRUCTURE (a per-process, slot-keyed handle
// table); U5x turns each handle into a capability — an unforgeable reference carrying RIGHTS, CHECKED at
// the point of use, GRANTABLE (attenuated) and REVOKABLE, its lifetime bounded by the owning slot's
// teardown-clear. Two things are added to what a handle names: a rights bitmask (a sidecar array, so
// U4x's `0`/`RESERVING` value-word sentinels stay byte-identical) and a resource TARGET beyond "child
// pid" (a `CONSOLE` well-known token, so `sys_write` routes through the table). Deliberately minimal:
// two target kinds (Child(pid), Console), a small rights set, no general object table (that is U6+).
// =============================================================================================

/// Capability rights — a small bitmask carried in the sidecar `HANDLE_RIGHTS`, checked at
/// `handle_resolve`. `CAP_WRITE` gates `sys_write`; `CAP_GRANT` gates minting attenuated copies;
/// `CAP_READ`/`CAP_EXEC`/`CAP_REVOKE` round out the model (`CAP_REVOKE` reserved for cross-process
/// revocation — U6; U5x revoke is ownership-based). Values are stable across arches (aarch64 U5 twin).
const CAP_READ: u32 = 1 << 0; // 0x01
const CAP_WRITE: u32 = 1 << 1; // 0x02
const CAP_EXEC: u32 = 1 << 2; // 0x04
const CAP_GRANT: u32 = 1 << 3; // 0x08
const CAP_REVOKE: u32 = 1 << 4; // 0x10 (reserved: cross-process revocation, U6)
// The rights are the distinct low 5 bits — a well-formed bitmask (each a single, non-overlapping bit,
// which the attenuation check `req & !src` relies on). This const-assert verifies that and anchors every
// CAP_* as used, so the bits not yet exercised in Rust this arc (CAP_EXEC — held by no fixture, so the
// attenuation negative bites; CAP_REVOKE — reserved for U6) don't read as dead code.
const _: () = assert!(
    (CAP_READ | CAP_WRITE | CAP_EXEC | CAP_GRANT | CAP_REVOKE) == 0x1F,
    "capability rights must be the distinct low 5 bits"
);

/// The well-known target token stored in a handle's value word to mean "the serial console resource" (as
/// opposed to a child pid). Distinct from `0` (Empty), `HANDLE_RESERVING` (`u64::MAX`), and every real
/// pid (small, monotonic), so the value word alone discriminates Child(pid) from Console without
/// perturbing U4x's sentinel checks. One non-pid token (not a general object table) is the arc's scope.
const HANDLE_CONSOLE: u64 = u64::MAX - 1;

/// The conventional stdout handle index. Every ring-3 program prints with `sys_write(fd=1, ..)`, so the
/// console write-capability is endowed at this fixed index in each printing process's table
/// (`install_console_cap`). Reserved by convention, like fd 1 on POSIX.
const CONSOLE_FD: usize = 1;

/// The rights sidecar: keyed IDENTICALLY to `HANDLES` (`[row][idx]`), so the value word keeps U4x's exact
/// `0`/`RESERVING` sentinel semantics and the rights ride alongside. Written Release beside the value
/// store (rights published BEFORE the value that makes a handle live, so a resolver that observes the
/// value also observes the rights), cleared in `handle_clear`/`clear_handle_row`. `0` == an inert handle.
static HANDLE_RIGHTS: [[AtomicU32; NHANDLE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NHANDLE] }; crate::arch::memory::USER_SLOTS + 1];

/// What a resolved handle NAMES: a child process (by pid — U4x's meaning) or the console resource (U5x).
#[derive(Clone, Copy)]
enum HandleTarget {
    Child(u64),
    Console,
}

/// Why `handle_resolve` refused: the handle is not in the caller's table (out-of-range/Empty/RESERVING),
/// or it is present but lacks a required right. Callers map these to their own errno (`sys_wait` ->
/// `-ECHILD` for either, preserving U4x's structural-ownership semantics; `sys_write`/`sys_cap` ->
/// `-EACCES`).
enum ResolveErr {
    NoHandle,
    Denied,
}

/// The HANDLES row for the caller: its private address-space slot, or `SHARED_ROW` when the caller runs
/// in the shared kernel window (`current_slot()` is None — U1a/U1b/U2). The x86 twin of aarch64's
/// `current_asid()` (0 for the shared window, 1.. for private).
fn caller_row() -> usize {
    crate::arch::memory::current_slot().unwrap_or(SHARED_ROW)
}

/// The U5x enforcement CHECK, at the SINGLE lookup point every handle-consuming path goes through.
/// Resolve `idx` against the caller's own (`row`) table, then require the handle carry every bit in
/// `req`. Returns the target on success. `NoHandle` for out-of-range/Empty/`RESERVING` (a reserving
/// placeholder is never a usable handle); `Denied` when a present handle lacks a required right. The
/// value word is loaded Acquire (synchronizing with the Release store that installed it), then the
/// rights — so a resolver that sees a live value also sees its rights.
fn handle_resolve(row: usize, idx: u64, req: u32) -> Result<HandleTarget, ResolveErr> {
    if idx as usize >= NHANDLE {
        return Err(ResolveErr::NoHandle);
    }
    debug_assert!(row < HANDLES.len(), "handle_resolve: row out of range");
    let raw = HANDLES[row][idx as usize].load(Ordering::Acquire);
    if raw == 0 || raw == HANDLE_RESERVING {
        return Err(ResolveErr::NoHandle);
    }
    let rights = HANDLE_RIGHTS[row][idx as usize].load(Ordering::Acquire);
    if rights & req != req {
        return Err(ResolveErr::Denied);
    }
    Ok(if raw == HANDLE_CONSOLE {
        HandleTarget::Console
    } else {
        HandleTarget::Child(raw)
    })
}

/// Set the rights word at `HANDLES[row][idx]` (Release) — used beside a value store to attach rights to a
/// freshly-installed handle (a child handle in `sys_spawn`, a minted handle in `sys_cap_grant`).
fn handle_set_rights(row: usize, idx: usize, rights: u32) {
    debug_assert!(row < HANDLES.len() && idx < NHANDLE, "handle_set_rights: out of range");
    HANDLE_RIGHTS[row][idx].store(rights, Ordering::Release);
}

/// Install a capability at a FIXED index (not `handle_install`'s first-free scan): store the rights FIRST
/// (Release), then the target value (Release), so a resolver that observes the live value also observes
/// the rights. Used to endow the console write-capability at `CONSOLE_FD` and to plant the U5x demo's
/// fixtures. Always called BEFORE the target process is dispatched (setup / pre-spawn), so there is no
/// concurrent resolver; the ordering is defensive belt-and-braces.
fn install_cap(row: usize, idx: usize, target: u64, rights: u32) {
    debug_assert!(row < HANDLES.len() && idx < NHANDLE, "install_cap: out of range");
    HANDLE_RIGHTS[row][idx].store(rights, Ordering::Release);
    HANDLES[row][idx].store(target, Ordering::Release);
}

/// Endow the process running in `row` with a console WRITE-capability at `CONSOLE_FD` — the bootstrap
/// that lets a ring-3 program print once `sys_write` routes through the table. Given to every printing
/// process: the shared window (`SHARED_ROW`) in `setup`, and each spawned child in `sys_spawn`. A process
/// NOT so endowed gets `-EACCES` from `sys_write` (the U5x negative).
fn install_console_cap(row: usize) {
    install_cap(row, CONSOLE_FD, HANDLE_CONSOLE, CAP_WRITE);
}

/// True iff the entire `HANDLES[row]` row (values AND rights) is clear — the teardown-clear verifier.
/// Read by `u5x_launcher` after the fixture exits and its slot is retired: `free_user_space_by_cr3`
/// clears the row on exit, so this transitions false -> true, proving no stale capability outlives its
/// owning slot.
fn handle_row_is_clear(row: usize) -> bool {
    debug_assert!(row < HANDLES.len(), "handle_row_is_clear: row out of range");
    (0..NHANDLE).all(|i| {
        HANDLES[row][i].load(Ordering::Acquire) == 0
            && HANDLE_RIGHTS[row][i].load(Ordering::Acquire) == 0
    })
}

/// U5x: clear an ENTIRE per-process handle row (every value + its rights) when the owning slot is torn
/// down — the lifecycle half of "U5x owns revoke/teardown-clear", folding U4x's one deferred note (a row
/// was NOT cleared on teardown, so a future slot-reuse could observe stale entries). Called from
/// `memory::free_user_space_by_cr3` BEFORE the slot's used-flag is released (clear-before-release, the
/// ordering aarch64 U5 endorsed), so no concurrent `alloc_user_space` on another core can claim the slot
/// and populate the row between the clear and the release. `slot` is a PRIVATE slot (0..USER_SLOTS);
/// `SHARED_ROW` is never torn down.
pub fn clear_handle_row(slot: usize) {
    debug_assert!(slot < crate::arch::memory::USER_SLOTS, "clear_handle_row: not a private slot");
    for i in 0..NHANDLE {
        // Clear the value first (Empty => `handle_resolve` bails as NoHandle before reading rights), then
        // the rights — so no intermediate state is ever a live handle with stale rights.
        HANDLES[slot][i].store(0, Ordering::Release);
        HANDLE_RIGHTS[slot][i].store(0, Ordering::Release);
    }
}

/// Claim the first Empty slot in `HANDLES[slot]`, storing `pid` (CAS 0->pid), and return its index —
/// the value `sys_spawn` returns to ring 3. `None` if the table is full (-> -EAGAIN). `pid` is
/// `HANDLE_RESERVING` for a pre-spawn reservation, then overwritten with the real pid via `handle_set`.
/// `slot` is always in range (from `current_slot`, 0..USER_SLOTS; debug-asserted).
fn handle_install(slot: usize, pid: u64) -> Option<usize> {
    debug_assert!(slot < HANDLES.len(), "handle_install: slot out of range");
    for (i, h) in HANDLES[slot].iter().enumerate() {
        if h.compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return Some(i);
        }
    }
    None
}

/// Overwrite the pid stored at `HANDLES[slot][idx]` (Release) — replace a `HANDLE_RESERVING`
/// placeholder with the real child pid once `sys_spawn` has it.
fn handle_set(slot: usize, idx: usize, pid: u64) {
    debug_assert!(slot < HANDLES.len() && idx < NHANDLE, "handle_set: out of range");
    HANDLES[slot][idx].store(pid, Ordering::Release);
}

/// The pid at `HANDLES[slot][idx]`, or `None` if the index is out of range or the slot is Empty (0) —
/// i.e. the caller holds no such child handle (structural ownership: -ECHILD). A `HANDLE_RESERVING`
/// placeholder can never be seen here (single-writer: a task is not concurrently spawning and waiting).
fn handle_get(slot: usize, idx: usize) -> Option<u64> {
    if idx >= NHANDLE {
        return None;
    }
    debug_assert!(slot < HANDLES.len(), "handle_get: slot out of range");
    match HANDLES[slot][idx].load(Ordering::Acquire) {
        0 => None,
        pid => Some(pid),
    }
}

/// Clear (0 = Empty) the handle at `HANDLES[slot][idx]` AND its rights sidecar — consumed when its child
/// is reaped in `sys_wait`, revoked by `sys_cap_revoke`, or released when a failed `sys_spawn` unwinds its
/// reservation. U5x: clears BOTH the value and the rights (value first — Empty => `handle_resolve` bails
/// before reading rights — then rights), mirroring the aarch64 twin's `handle_clear`, so a dropped
/// capability never leaves a stale rights word behind an Empty slot for a later re-install to inherit.
fn handle_clear(slot: usize, idx: usize) {
    debug_assert!(slot < HANDLES.len() && idx < NHANDLE, "handle_clear: out of range");
    HANDLES[slot][idx].store(0, Ordering::Release);
    HANDLE_RIGHTS[slot][idx].store(0, Ordering::Release);
}

/// SYS_SPAWN(): instantiate the pre-staged program (HELLO.BIN) in a fresh slot, run it ring-3 as a
/// CHILD of the caller, and return a HANDLE index into the CALLER's per-process handle table (not the
/// raw pid), or a negative errno. The handle IS the ownership token: `sys_wait` takes it, and only a
/// caller whose table holds it can reap the child. No args this arc — the program is fixed (arbitrary
/// program-by-name is a later arc; it needs a validated copy_from_user name).
///
/// Race-freedom (the child cannot exit before its pid is recorded): the whole dispatch runs IRQ-masked
/// and the CHILD is co-located on the CALLER's core, so it stays queued-not-dispatched until the parent
/// yields (which it does only later, in sys_wait). We (1) claim a Proc entry, (2) reserve a handle slot
/// (RESERVING), (3) allocate + fill a fresh address-space slot, (4) spawn the child (queued, not run),
/// (5) store its real pid (Release) into BOTH the Proc entry and the handle slot — all before returning
/// to ring 3. The handle is reserved BEFORE the slot fill so a full handle table fails cleanly with
/// nothing to un-spawn. No FAT I/O here — the program was pre-staged (see the STORAGE / IF NOTE).
fn sys_spawn() -> i64 {
    // Gate: a program must be staged (which required a block device + HELLO.BIN at stage time).
    if !HELLO_STAGED.load(Ordering::Acquire) {
        return ENODEV;
    }
    // The CALLER's address-space SLOT names its per-process handle table — read synchronously here,
    // where the caller's CR3 is live (sys_spawn installs into and sys_wait resolves from the SAME
    // table, since both run as the parent). A caller in the shared kernel window has no slot/table.
    let Some(slot) = crate::arch::memory::current_slot() else {
        return EAGAIN;
    };
    // Claim the Proc entry FIRST, so a failed alloc frees only the entry, and so the pid slot exists to
    // receive the real pid before the child can be dispatched.
    let Some(pi) = proc_reserve() else {
        return EAGAIN; // process table full
    };
    // Reserve a HANDLE slot BEFORE allocating the address space (a RESERVING placeholder). A full
    // handle table fails here with only the Proc entry to release — nothing loaded or spawned yet.
    let Some(h) = handle_install(slot, HANDLE_RESERVING) else {
        proc_free(pi);
        return EAGAIN; // handle table full
    };
    // Allocate a fresh per-process address space for the child (LAST fallible step). On exhaustion,
    // unwind the handle + Proc reservations — no child was spawned, nothing to un-spawn.
    let Some(child_slot) = crate::arch::memory::alloc_user_space() else {
        handle_clear(slot, h);
        proc_free(pi);
        return EAGAIN; // slot pool exhausted
    };
    // Fill the child's slot: scrub the whole window (no prior-process residue), then copy the pre-staged
    // program into the code page (page 0) through the kernel identity alias — never through USER_BASE,
    // so the ring-3 code mapping stays read-only (W^X holds by construction). IF=0-safe (memcpy only).
    let len = HELLO_LEN.load(Ordering::Acquire);
    let backing = crate::arch::memory::slot_backing_ptr(child_slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, (USER_WINDOW_PAGES * PAGE_SIZE) as usize);
        core::ptr::copy_nonoverlapping((&raw const HELLO_BYTES).cast::<u8>(), backing, len);
    }
    // U5x: endow the CHILD's OWN table with a console write-capability (the child runs HELLO.BIN, which
    // `sys_write`s fd 1). Done here, on the freshly-built slot, BEFORE the child is spawned — the child
    // cannot be dispatched until the parent yields (the co-location invariant below), so there is no
    // concurrent resolver of the child's table. Without this the child's first print would -EACCES (routed).
    install_console_cap(child_slot);
    // Co-locate the child on the caller's core (the invariant above): sys_spawn always runs with its
    // ring-3 caller current, so `this_cpu` is the parent's core.
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    let sp = USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16;
    let cr3 = crate::arch::memory::slot_cr3(child_slot);
    let pid = crate::arch::sched::spawn_user_in_space("u4x-child", USER_BASE, sp, cpu, cr3);
    // Record the real pid (Release) into BOTH the Proc entry (pid-keyed exit accounting) and the
    // reserved handle slot (slot-keyed ownership) BEFORE returning to ring 3 — before the parent can
    // yield and let the child run. The child's exit path sees the Proc pid; the parent's later
    // sys_wait resolves the handle to it. U5x: the parent's child handle carries CAP_READ (the ownership
    // token — `sys_wait` gates on kind==Child, not on the right; published Release BEFORE the pid so the
    // handle is never live sans rights).
    PROCS[pi].pid.store(pid, Ordering::Release);
    handle_set_rights(slot, h, CAP_READ);
    handle_set(slot, h, pid);
    h as i64 // the HANDLE index (per-process; two processes can each hold handle 0 to different children)
}

/// SYS_WAIT(handle): block the caller until the child its `handle` refers to exits, then return the
/// child's exit status — or `-ECHILD` if that handle is not in the CALLER's table (out-of-range or
/// Empty). Structural ownership: you can only reap a child whose handle is in YOUR table. The waker is
/// the child's `done.post()` — a scheduler wake (from its SYS_EXIT or kill path), so this works under
/// QEMU. The handle is CONSUMED by the reap (`handle_clear`), so a second sys_wait on it -> -ECHILD.
///
/// We wait on `done` UNCONDITIONALLY: the child posts it exactly once (exit or kill), so this either
/// fast-returns a permit the child already left (child exited first — no park) or parks until it posts.
/// Exactly one post is consumed by exactly one wait, so the reaped entry returns to 0 permits, clean
/// for reuse.
fn sys_wait(handle: u64) -> i64 {
    let row = caller_row();
    // Resolve the handle against the CALLER's OWN table — the structural ownership check, now through the
    // U5x enforcement point. It must be a CHILD handle (U4x's meaning). Out-of-range/Empty (NoHandle), a
    // rights shortfall (Denied), or a CONSOLE handle all mean "you hold no such child" => -ECHILD
    // (byte-identical to U4x for the orphan's `sys_wait(0)` and for a shared-window caller). Waiting
    // requires no resource right — holding the child handle is the ownership token (`req = 0`).
    let pid = match handle_resolve(row, handle, 0) {
        Ok(HandleTarget::Child(pid)) => pid,
        _ => return ECHILD,
    };
    let Some(pi) = proc_find_child(pid) else {
        return ECHILD; // the handle named a pid with no Proc entry (defensive; cannot happen in the demo)
    };
    let woken = PROCS[pi].done.wait();
    debug_assert!(woken, "sys_wait: called off a scheduled task");
    let status = PROCS[pi].status.load(Ordering::Acquire) as i64;
    proc_free(pi); // reap the Proc entry (its `done` is back at 0 permits, free for reuse)
    handle_clear(row, handle as usize); // consume the handle: a second sys_wait on it now -> -ECHILD
    status
}

/// SYS_CAP(op, a1, a2): grant/attenuate/revoke on the CALLER's OWN handle table — capabilities as
/// first-class operations. `op` selects the sub-op. Runs single-writer over the caller's table (one
/// syscall at a time, IRQ-masked; one live task per slot), so no lock is needed. The aarch64 `sys_cap`
/// twin. See `sys_cap_grant`/`sys_cap_revoke`.
fn sys_cap(op: u64, a1: u64, a2: u64) -> i64 {
    let row = caller_row();
    match op {
        CAP_OP_GRANT => sys_cap_grant(row, a1, a2),
        CAP_OP_REVOKE => sys_cap_revoke(row, a1),
        _ => EINVAL,
    }
}

/// SYS_CAP GRANT(src_idx, req_rights): mint a NEW handle in the caller's own table naming the SAME target
/// as `src_idx`, carrying `req_rights` — enforcing the ATTENUATION (monotonic-decrease) invariant: the
/// minted rights can never exceed the granter's rights on the source. Requires `CAP_GRANT` on the source.
/// Returns the new handle index, or a negative errno:
///   -EACCES — no such source handle, source lacks CAP_GRANT, or `req_rights` would AMPLIFY (bits the
///             granter does not hold): the core U5x property — a grant can never produce more rights than
///             the granter.
///   -EAGAIN — the caller's handle table is full (no free slot to mint into; never grown).
/// The mint targets the caller's OWN table (a child spawns nothing to grant into yet); cross-table minting
/// is a straightforward extension once cross-process object naming lands (U6).
fn sys_cap_grant(row: usize, src_idx: u64, req_rights: u64) -> i64 {
    // Resolve the source's raw target + rights (no right required to READ your own handle's descriptor).
    let Some(target) = handle_get(row, src_idx as usize) else {
        return EACCES; // no such source handle
    };
    if target == HANDLE_RESERVING {
        return EACCES; // an in-flight reservation is not a grantable handle (defensive; single-writer)
    }
    let src_rights = HANDLE_RIGHTS[row][src_idx as usize].load(Ordering::Acquire);
    if src_rights & CAP_GRANT == 0 {
        return EACCES; // the source does not authorize granting
    }
    let req = req_rights as u32;
    // Attenuation: reject any requested bit the granter does not itself hold. `req & !src_rights` is
    // exactly the set of amplifying bits; non-empty => the grant would exceed the granter's authority.
    if req & !src_rights != 0 {
        return EACCES;
    }
    // Mint: reuse `handle_install` for the first-free slot claim (the U4x sentinel logic), then attach the
    // attenuated rights. Single-writer over this table (the caller is mid-syscall, not concurrently
    // resolving), so the value-then-rights order carries no race.
    match handle_install(row, target) {
        Some(idx) => {
            handle_set_rights(row, idx, req);
            idx as i64
        }
        None => EAGAIN, // handle table full
    }
}

/// SYS_CAP REVOKE(idx): drop a handle the caller owns (`handle_clear`, which also clears its rights). A
/// process may always drop its OWN capabilities (ownership-based — the caller's table is its own), so no
/// right is required; `CAP_REVOKE` is reserved for cross-process revocation (revocation trees — U6).
/// Returns 0, or -ECHILD if the index is out-of-range/Empty (nothing to revoke). After revoke, any use of
/// the index returns -EACCES (`sys_write`) / -ECHILD (`sys_wait`) — the handle is gone.
fn sys_cap_revoke(row: usize, idx: u64) -> i64 {
    if idx as usize >= NHANDLE || handle_get(row, idx as usize).is_none() {
        return ECHILD; // out-of-range or Empty — no such handle to revoke
    }
    handle_clear(row, idx as usize);
    0
}

// --- U4x demo accounting (written by the exit/kill paths + the parent/orphan witnesses, read by the
// verdict). ---
/// The parent's witness: 1 iff it reaped BOTH children by handle with exit status 0 (its `sys_exit(0)`),
/// else 0. Written by the SYS_EXIT arm for `u4x-parent`.
static U4X_PARENT_OK: AtomicU32 = AtomicU32::new(0);
/// The ownership NEGATIVE: 1 iff `u4x-orphan`'s `sys_wait(0)` on an Empty handle returned exactly
/// -ECHILD (its `sys_exit(0)`), else 0.
static U4X_ORPHAN_ECHILD: AtomicU32 = AtomicU32::new(0);
/// The U4x fixtures (parent + orphan) that reached their sys_exit (the completion signal; want 2). The
/// verdict waits for both before judging.
static U4X_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U4x task (a child, the parent, or the orphan) — a real bug (the children are well-behaved;
/// a kill fails the verdict). Kept OFF the U1b `killed_unexpected` counter (see `record_ring3_kill`).
static U4X_KILLED: AtomicU32 = AtomicU32::new(0);
/// Set once U2's verdict has printed, so the U4x launcher orders its lines AFTER U2's (the aarch64
/// `M6G_LOADER_DONE` twin).
static U2_DONE: AtomicBool = AtomicBool::new(false);
/// Set once the U4x launcher has printed its verdict AND its slots have freed, so the U5x launcher orders
/// its lines AFTER U4x's and runs with a free slot for the teardown-clear proof (the aarch64
/// `U4_LAUNCH_DONE` twin).
static U4X_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

// --- U5x demo accounting (written by the exit/kill paths, read by the launcher's verdict). ---
/// The capability fixture's witness bitmask (its `sys_exit` status, routed by name). One bit per proven
/// behaviour: write-cap OK (bit0), no-cap -EACCES (bit1), attenuated grant bounded + subset grant usable
/// (bit2), revoke enforced (bit3). The verdict PASSes iff it equals `U5X_WITNESS_ALL`.
static U5X_WITNESS: AtomicU32 = AtomicU32::new(0);
/// 1 once the capability fixture reaches `sys_exit` (its witness is then valid). The launcher waits on
/// this before reading `U5X_WITNESS`.
static U5X_DONE: AtomicU32 = AtomicU32::new(0);
/// Incremented if the capability fixture is KILLED (a fault) — any kill is a verdict FAIL.
static U5X_KILLED: AtomicU32 = AtomicU32::new(0);
/// The full witness — all four capability behaviours proven.
const U5X_WITNESS_ALL: u32 = 0xF;

/// U4x fixture run parameters: the parent's + orphan's ring-3 entry VAs (both inside the shared window
/// VA — only the slot FRAME differs, via CR3), the shared initial user rsp, and each fixture's slot
/// CR3. Two tasks, two DISTINCT slots (hence distinct handle-table rows — the isolation the ownership
/// negative proves).
struct U4xDemo {
    parent: u64,
    orphan: u64,
    sp: u64,
    parent_cr3: u64,
    orphan_cr3: u64,
}

/// U4x setup: reserve the Proc semaphores, allocate + build TWO private slots (parent + orphan), copy
/// the U4x blob (both fixtures) into each slot's code page through the identity alias, and return each
/// fixture's entry VA + slot CR3. `None` if slot allocation fails (the whole request is released, not
/// leaked). Called ONCE from the launcher, strictly before the parent (hence any child) exists — so the
/// `done.init()` reservations here cannot race a concurrent wait/post. The parent and orphan get
/// DISTINCT slots (hence distinct handle-table rows): handle #0 means the parent's child A in the
/// parent's table, and Empty in the orphan's — the substrate the negative proves.
fn u4x_setup() -> Option<U4xDemo> {
    // Reserve each Proc semaphore's waiter capacity (the park-side push must not reallocate under the
    // held lock). Done before any child can block a parent on it.
    for p in &PROCS {
        p.done.init();
    }
    let mut slots = [0usize; 2];
    if !crate::arch::memory::alloc_user_spaces(&mut slots) {
        return None;
    }
    let blob_start = &raw const unaos_user_u4x_blob_start as usize;
    let blob_end = &raw const unaos_user_u4x_blob_end as usize;
    let blob_len = blob_end - blob_start;
    assert!(blob_len as u64 <= PAGE_SIZE, "U4x blob does not fit in a code page");
    let parent_off = (&raw const unaos_user_u4x_parent as usize - blob_start) as u64;
    let orphan_off = (&raw const unaos_user_u4x_orphan as usize - blob_start) as u64;
    for &s in &slots {
        let backing = crate::arch::memory::slot_backing_ptr(s);
        unsafe {
            // Scrub the whole window (residue), then copy the blob into the code page (page 0) through
            // the identity alias — never USER_BASE, so the code mapping stays read-only (W^X).
            core::ptr::write_bytes(backing, 0, (USER_WINDOW_PAGES * PAGE_SIZE) as usize);
            core::ptr::copy_nonoverlapping(blob_start as *const u8, backing, blob_len);
        }
    }
    serial_println!(
        ":: U4x: process model — per-process handle table (sys_spawn->handle, sys_wait(handle)) ::"
    );
    Some(U4xDemo {
        parent: USER_BASE + parent_off,
        orphan: USER_BASE + orphan_off,
        sp: USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16,
        parent_cr3: crate::arch::memory::slot_cr3(slots[0]),
        orphan_cr3: crate::arch::memory::slot_cr3(slots[1]),
    })
}

/// U4x launcher + verdict (the `u2_verdict` shape: one gated kernel task on a scheduled sibling core).
/// `demo_cpu` (the task arg) is the core the parent + orphan run on. Flow:
///   1. Wait (bounded, yielding) for `U2_DONE`, so all U2 lines print first.
///   2. Skip silently if no block device.
///   3. `u4x_setup()` (build both slots, print the setup line), then spawn the parent AND the orphan on
///      `demo_cpu`. The parent's two sys_spawns co-locate BOTH children on `demo_cpu` too — the
///      invariant that keeps each child queued-not-dispatched until the parent blocks in sys_wait (so
///      both pids are recorded first). The orphan's `sys_wait(0)` returns immediately (-ECHILD), so it
///      never parks.
///   4. Verdict: wait (bounded) for BOTH fixtures to reach sys_exit (`U4X_DONE == 2`), then PASS iff the
///      parent reaped both children AND the orphan saw -ECHILD AND no U4x task was killed. One PASS line.
pub fn u4x_launcher(demo_cpu: usize) {
    // 1. Gate on U2 (bounded, yielding — don't wedge this sibling core if U2 is slow/absent).
    let wdeadline = crate::arch::ticks() + 10_000;
    while !U2_DONE.load(Ordering::Acquire) && crate::arch::ticks() < wdeadline {
        crate::arch::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No block device -> the children cannot run (their program came off it); skip silently.
    if crate::drivers::block::info().is_none() {
        U4X_LAUNCH_DONE.store(true, Ordering::Release); // release the U5x gate (U5x also skips on no-SD)
        return;
    }

    // 3. Build the parent + orphan slots and spawn both on the demo core.
    let Some(demo) = u4x_setup() else {
        serial_println!(":: U4x: no free address-space slot — process-model demo skipped ::");
        U4X_LAUNCH_DONE.store(true, Ordering::Release); // release the U5x gate even on the skip path
        return;
    };
    crate::arch::sched::spawn_user_in_space(
        "u4x-parent",
        demo.parent,
        demo.sp,
        demo_cpu,
        demo.parent_cr3,
    );
    crate::arch::sched::spawn_user_in_space(
        "u4x-orphan",
        demo.orphan,
        demo.sp,
        demo_cpu,
        demo.orphan_cr3,
    );

    // 4. Verdict: wait (bounded, yielding) for BOTH fixtures to reach sys_exit, then judge.
    let vdeadline = crate::arch::ticks() + 5000;
    while U4X_DONE.load(Ordering::Acquire) < 2 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let parent_ok = U4X_PARENT_OK.load(Ordering::Acquire);
    let orphan = U4X_ORPHAN_ECHILD.load(Ordering::Acquire);
    let killed = U4X_KILLED.load(Ordering::Acquire);
    if parent_ok == 1 && orphan == 1 && killed == 0 {
        serial_println!(
            ":: U4x: x86 process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U4x: x86 process model FAIL — parent_ok={} orphan_echild={} killed={} done={} (want 1/1/0/2) ::",
            parent_ok,
            orphan,
            killed,
            U4X_DONE.load(Ordering::Acquire)
        );
    }
    // Release the U5x gate: the U4x verdict has printed and (both fixtures having exited) the U4x slots
    // are freed, so the U5x launcher may build + endow its fixture slot and order its lines after ours.
    U4X_LAUNCH_DONE.store(true, Ordering::Release);
}

/// U4x one-shot, fired from the main loop once a block device is present (mirrors `u2_probe_once`'s
/// gate). It (BSP, IF=1) PRE-STAGES the child program off FAT (`stage_hello` — the read cannot live in
/// the IF=0 syscall handler, see the STORAGE / IF NOTE), then spawns the launcher on a sibling AP. Does
/// nothing until a block device and a scheduled AP exist; a missing HELLO.BIN skips the demo cleanly.
pub fn u4x_probe_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // retry next loop iteration until storage enumerates
    }
    let online = crate::arch::smp::online_aps();
    let Some(&cpu) = online.first() else {
        DONE.store(true, Ordering::Relaxed);
        serial_println!(":: U4x: no application processor online — process-model demo skipped ::");
        return;
    };
    DONE.store(true, Ordering::Relaxed); // one-shot from here regardless of outcome

    // Pre-stage HELLO.BIN off FAT HERE (BSP main loop, IF=1). A missing volume/file skips the demo.
    if !stage_hello() {
        serial_println!(":: U4x: HELLO.BIN not available — process-model demo skipped ::");
        // U4x is skipped, but the U5x fixture is an inline blob (no HELLO.BIN needed), so release the
        // U5x gate immediately rather than making it wait out its 10 s deadline before running.
        U4X_LAUNCH_DONE.store(true, Ordering::Release);
        return;
    }

    // The launcher runs on a sibling core (or the demo core if only one AP), spawning the parent +
    // orphan on `cpu` — the `u2_probe_once` split.
    let vcpu = online.get(1).copied().unwrap_or(cpu);
    crate::arch::sched::spawn("u4x-launch", u4x_launcher, cpu, vcpu, crate::arch::sched::PRIO_NORMAL);
}

/// U5x fixture run parameters: the capability fixture's ring-3 entry VA (inside the shared window VA —
/// only the slot FRAME differs, via CR3), the initial user rsp, its slot CR3, and its slot INDEX (for the
/// teardown-clear proof — `handle_row_is_clear(slot)`).
struct U5xDemo {
    cap: u64,
    sp: u64,
    cr3: u64,
    slot: usize,
}

/// U5x setup: allocate + build ONE private slot, copy the U5x blob into its code page through the identity
/// alias (the slot's code page is RX-RO from the start — W^X by construction, `build_slot`), then
/// PRE-ENDOW the fixture's table with the two handles the demo exercises:
///   handle 1 = CONSOLE, {CAP_WRITE|CAP_GRANT} — the "full" console cap it writes from and grants from
///   handle 2 = CONSOLE, {CAP_READ}            — a console cap WITHOUT write (the `-EACCES` negative)
/// Emits the U5x setup line; returns the run params. `None` if slot allocation fails. Called ONCE from
/// `u5x_launcher`, after the U4x gate — so a slot is free and no task runs under the fixture's slot yet
/// (the endowment stores cannot race a resolver). Register-only fixture (writes no user stack).
fn u5x_setup() -> Option<U5xDemo> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let blob_start = &raw const unaos_user_u5x_blob_start as usize;
    let blob_end = &raw const unaos_user_u5x_blob_end as usize;
    let blob_len = blob_end - blob_start;
    assert!(blob_len as u64 <= PAGE_SIZE, "U5x blob does not fit in a code page");
    let cap_off = (&raw const unaos_user_u5x_cap as usize - blob_start) as u64;
    let backing = crate::arch::memory::slot_backing_ptr(slot);
    unsafe {
        // Scrub the whole window (residue), then copy the blob into the code page (page 0) through the
        // identity alias — never USER_BASE, so the code mapping stays read-only (W^X).
        core::ptr::write_bytes(backing, 0, (USER_WINDOW_PAGES * PAGE_SIZE) as usize);
        core::ptr::copy_nonoverlapping(blob_start as *const u8, backing, blob_len);
    }
    // Pre-endow the fixture's table (before it is dispatched — no concurrent resolver). Two console caps:
    // a full one (write + grant) at index 1, and a write-LESS one at index 2 for the negative.
    install_cap(slot, 1, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    install_cap(slot, 2, HANDLE_CONSOLE, CAP_READ);
    serial_println!(
        ":: U5x: capabilities — rights + CHECK + grant/attenuate/revoke + routed sys_write ::"
    );
    Some(U5xDemo {
        cap: USER_BASE + cap_off,
        sp: USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16,
        cr3: crate::arch::memory::slot_cr3(slot),
        slot,
    })
}

/// U5x launcher + verdict (the `u4x_launcher` shape: one gated kernel task on a scheduled sibling core).
/// `demo_cpu` (the task arg) is the core the cap fixture runs on. Flow:
///   1. Wait (bounded, yielding) for `U4X_LAUNCH_DONE`, so the U5x lines land after the U4x verdict and
///      the U4x slots have freed.
///   2. Skip silently if no block device — U5x needs NO disk (its fixture is an inline blob), but gating
///      on it keeps the no-storage control path free of demo lines (mirrors U4x).
///   3. `u5x_setup()` (build + pre-endow the fixture's slot), then spawn the fixture on `demo_cpu`.
///   4. Verdict (folded): wait (bounded) for the fixture's exit (`U5X_DONE == 1`), read its witness, then
///      wait (bounded) for its handle row to be cleared — the teardown-clear proof:
///      `sched::exit -> memory::free_user_space_by_cr3` clears the row when the fixture exits,
///      transitioning `handle_row_is_clear` false->true (the fixture holds live handles at exit — the
///      minted cap and the write-less cap — so this genuinely exercises the clear). PASS iff witness ==
///      `U5X_WITNESS_ALL` AND the row cleared AND no U5x kill. Prints ONE PASS line.
pub fn u5x_launcher(demo_cpu: usize) {
    // 1. Gate on the U4x launcher (its verdict printed + its slots freed), bounded + yielding.
    let wdeadline = crate::arch::ticks() + 10_000;
    while !U4X_LAUNCH_DONE.load(Ordering::Acquire) && crate::arch::ticks() < wdeadline {
        crate::arch::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No block device -> keep the no-storage control path free of demo lines (U5x needs no disk).
    if crate::drivers::block::info().is_none() {
        return;
    }

    // 3. Build + pre-endow the fixture slot and spawn it on the demo core.
    let Some(u5) = u5x_setup() else {
        serial_println!(":: U5x: no free address-space slot — capability demo skipped ::");
        return;
    };
    crate::arch::sched::spawn_user_in_space("u5x-cap", u5.cap, u5.sp, demo_cpu, u5.cr3);

    // 4a. Wait (bounded, yielding) for the fixture to reach its exit, then snapshot the witness.
    let vdeadline = crate::arch::ticks() + 5000;
    while U5X_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U5X_WITNESS.load(Ordering::Acquire);
    let killed = U5X_KILLED.load(Ordering::Acquire);

    // 4b. Teardown-clear proof: the fixture exited above, so its exit path cleared its handle row. That
    //     clear runs just after the exit accounting, so poll (bounded) until the row is clear — false->true
    //     when teardown runs. Nothing reuses the slot after (U5x is the last demo), so once clear it stays.
    let tdeadline = crate::arch::ticks() + 2000;
    while !handle_row_is_clear(u5.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = handle_row_is_clear(u5.slot);

    if witness == U5X_WITNESS_ALL && cleared && killed == 0 {
        serial_println!(
            ":: U5x: x86 capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded, revoke enforced, teardown-clear clean -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U5x: x86 capabilities FAIL — witness={:#x} cleared={} killed={} done={} (want {:#x} / true / 0 / 1) ::",
            witness,
            cleared,
            killed,
            U5X_DONE.load(Ordering::Acquire),
            U5X_WITNESS_ALL
        );
    }
}

/// U5x one-shot, fired from the main loop after `u4x_probe_once` (gated on storage like U4x, so the no-SD
/// control path stays free of demo lines). It spawns the U5x launcher on a sibling AP; the launcher gates
/// on `U4X_LAUNCH_DONE`, so ordering + the free-slot precondition hold regardless of interleave. No FAT
/// I/O — the U5x fixture is an inline blob (unlike U2/U4x's disk-loaded child).
pub fn u5x_probe_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // retry next loop iteration until storage enumerates (mirrors u4x_probe_once)
    }
    let online = crate::arch::smp::online_aps();
    let Some(&cpu) = online.first() else {
        DONE.store(true, Ordering::Relaxed);
        serial_println!(":: U5x: no application processor online — capability demo skipped ::");
        return;
    };
    DONE.store(true, Ordering::Relaxed); // one-shot from here regardless of outcome

    let vcpu = online.get(1).copied().unwrap_or(cpu);
    crate::arch::sched::spawn("u5x-launch", u5x_launcher, cpu, vcpu, crate::arch::sched::PRIO_NORMAL);
}
