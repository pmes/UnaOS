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
/// U7x: revoke a TRANSFER the caller previously made with `SYS_XFER` (`a1` = the transfer id SYS_XFER
/// returned). Sender-only (the transfer RECORD is sender-owned); single-level — revoking a transfer makes
/// the RECEIVED capability stale at its next `handle_resolve` (and discards it if still pending in the
/// recipient's inbox), but does NOT cascade through further re-transfers (revocation TREES are deferred).
const CAP_OP_XREVOKE: u64 = 2;
// U6bx: REAL File handles — the object table's first resource syscalls on a non-Console kind (the
// aarch64 pi4 U6b twin; same numbers). OPEN(name_ptr, name_len) looks the name up in the BSP-STAGED
// file set — not the disk: the SYSCALL handler runs IF-masked and the xHCI BOT read pump `hlt()`s, so
// an in-handler disk read would hang the core (see the STORAGE / IF NOTE at the pre-stage buffer) —
// and mints a File handle carrying `CAP_READ`. READ(handle, buf, len) serves the staged bytes through
// that handle, gated by File + `CAP_READ` at `handle_resolve` (the `sys_write` Console twin).
const SYS_OPEN: u64 = 11;
const SYS_READ: u64 = 12;
// U7x: cross-process capability transfer — the FIRST cross-process op on the object table (the aarch64
// pi4 U7 twin; same numbers). XFER(dest, src, req_rights) deposits an ATTENUATED copy of a capability
// the caller holds into the recipient's per-SLOT transfer INBOX (the one deliberately cross-slot
// surface, CAS-managed); the recipient names itself by being a `Child` handle in the SENDER's own table
// (owner-scoped delegation — no global process namespace). RECV() pulls a pending capability out of the
// CALLER's own inbox into the CALLER's own handle row — so every handle-table row keeps its single
// writer (the sender NEVER writes the recipient's row). Returns: XFER -> a transfer id (for a later
// CAP_OP_XREVOKE), RECV -> a handle index. x86 divergence: rows are keyed by address-space SLOT, and the
// SHARED_ROW (the U1a/U1b/U2 kernel window, torn down never and owned by no single process) is refused
// as a transfer endpoint — both XFER and RECV from a shared-window caller return -EACCES.
const SYS_XFER: u64 = 13;
const SYS_RECV: u64 = 14;

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

// --- U6x ring-3 fixture (the general object table — the aarch64 `__u6_prog_spawn` twin). ONE fixture
// (`u6x-spawn`) — the printing SPAWNER the U5x table couldn't serve: a process that BOTH prints (holds a
// console cap at the reserved `CONSOLE_FD`) AND spawns 2+ children (auto-allocated, distinct object
// handles), proving zero index collision. Position-independent; register-only apart from the RIP-relative
// message loads (writes no user stack, so it is safe on any slot). It builds a witness bitmask in r12
// (callee-saved — preserved across each `syscall`, the u4x/u5x idiom) and conveys it as its `sys_exit`
// STATUS, which the SYS_EXIT arm routes BY NAME into `U6X_WITNESS` (x86 needs no SYS_REPORT). The scaffold
// File/Socket kinds are proven kernel-side by `u6x_launcher` (no ring-3 syscall routes through them yet).
// ABI (Linux-style): rax = number, args rdi/rsi/rdx, return in rax.
//
// FLOW: (1) print BEFORE spawning (console cap works) -> bit0. (2) spawn child A, child B -> r13/r14; both
// handles must be >=0, neither the reserved console index (1), and distinct -> bit1. (3) print AFTER the two
// spawns -> bit2: the console cap at index 1 SURVIVED — under U5x a child could have been auto-allocated onto
// index 1 and then clobbered by the console install (or vice versa); U6x's reserved-index allocator makes
// that impossible. (4) reap BOTH children by handle, each status 0 -> bit3. Witness == 0xF iff all four held.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u6x_blob_start
unaos_user_u6x_blob_start:
    .balign 16
    .globl unaos_user_u6x_spawn
unaos_user_u6x_spawn:
    xor r12d, r12d                          // witness bitmask = 0 (callee-saved; survives syscalls)

    // (1) print BEFORE spawning — the console cap at the reserved index (fd=1) works
    mov rax, 1                              // SYS_WRITE
    mov rdi, 1                              // fd = CONSOLE_FD (the reserved console handle index)
    lea rsi, [rip + unaos_user_u6x_msg_pre] // buf -> the ring-3 VA at run time (RIP-relative)
    mov rdx, [rip + unaos_user_u6x_msg_pre_len]
    syscall
    test rax, rax
    js 1f                                   // negative -> skip bit0 (fail)
    or r12, 1                               // bit0: print-before-spawn OK
1:
    // (2) spawn TWO children — distinct handles auto-allocated around the reserved console index
    mov rax, 8                              // SYS_SPAWN -> handle_a
    syscall
    mov r13, rax                            // r13 = handle_a
    mov rax, 8                              // SYS_SPAWN -> handle_b
    syscall
    mov r14, rax                            // r14 = handle_b
    test r13, r13
    js 2f                                   // handle_a < 0 (spawn A failed) -> fail bit1
    test r14, r14
    js 2f                                   // handle_b < 0 (spawn B failed) -> fail bit1
    cmp r13, 1                              // handle_a == CONSOLE_FD ? (must NOT land on the reserved index)
    je 2f
    cmp r14, 1                              // handle_b == CONSOLE_FD ?
    je 2f
    cmp r13, r14                            // handle_a == handle_b ? (must be distinct)
    je 2f
    or r12, 2                               // bit1: both valid, off the reserved index, distinct
2:
    // (3) print AFTER spawning — the console cap MUST have survived the two spawns (the no-collision proof)
    mov rax, 1                              // SYS_WRITE
    mov rdi, 1                              // fd = console (still intact iff no collision)
    lea rsi, [rip + unaos_user_u6x_msg_post]
    mov rdx, [rip + unaos_user_u6x_msg_post_len]
    syscall
    test rax, rax
    js 3f                                   // console clobbered -> negative -> fail bit2
    or r12, 4                               // bit2: print-after-spawn OK (console survived the spawns)
3:
    // (4) reap BOTH children by their handles — each must exit status 0
    mov rax, 9                              // SYS_WAIT(handle_a)
    mov rdi, r13
    syscall
    mov r15, rax                            // r15 = status_a
    mov rax, 9                              // SYS_WAIT(handle_b)
    mov rdi, r14
    syscall                                 // rax = status_b
    test r15, r15
    jnz 4f                                  // status_a != 0 -> fail bit3
    test rax, rax
    jnz 4f                                  // status_b != 0 -> fail bit3
    or r12, 8                               // bit3: both children reaped with status 0
4:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U6X_WITNESS
    mov rdi, r12                            // status = witness bitmask
    syscall
5:  jmp 5b                                  // sys_exit never returns; belt-and-braces guard
    // Read-only data in the (ring-3 RX, kernel-readable) code page. Each length is an assemble-time label
    // difference loaded RIP-relative (the U5x/hello idiom — no `mov reg, sym-sym` memory-operand trap).
    .balign 8
unaos_user_u6x_msg_pre_len:
    .quad unaos_user_u6x_msg_pre_end - unaos_user_u6x_msg_pre
unaos_user_u6x_msg_pre:
    .ascii "u6x: parent print (pre-spawn)\n"
unaos_user_u6x_msg_pre_end:
    .balign 8
unaos_user_u6x_msg_post_len:
    .quad unaos_user_u6x_msg_post_end - unaos_user_u6x_msg_post
unaos_user_u6x_msg_post:
    .ascii "u6x: parent print (post-spawn; console survived 2 spawns)\n"
unaos_user_u6x_msg_post_end:
    .globl unaos_user_u6x_blob_end
unaos_user_u6x_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u6x_blob_start: u8;
    static unaos_user_u6x_blob_end: u8;
    static unaos_user_u6x_spawn: u8;
}

// --- U6bx ring-3 fixture (REAL File handles — the aarch64 `__u6b_prog_file` twin). ONE fixture
// (`u6bx-file`) exercising the object table's FIRST resource syscall on a non-Console object: it opens the
// staged file by name (SYS_OPEN -> a File handle carrying CAP_READ), reads through it (SYS_READ), compares
// the bytes against the kernel-planted expected prefix, and proves the SYS_READ CHECK rejects both a File
// handle stripped of CAP_READ (the rights arm, pre-endowed at `U6BX_NOCAP_IDX`) and a non-File handle
// carrying CAP_READ (the kind arm, a Socket at `U6BX_SOCK_IDX`). Register-only (writes no user stack): the
// read dest is the DATA page (window page 1) and the compare target the kernel-planted page-2 prefix, both
// fixed window VAs. Its own SYS_OPEN first-free-claims index 0 (index 1 = the reserved CONSOLE_FD, never
// auto-allocated; 2/3 are pre-endowed). Witness bitmask (5 bits — see `U6BX_WITNESS_ALL`) conveyed as its
// exit STATUS, which the SYS_EXIT arm routes BY NAME into `U6BX_WITNESS` (the u5x/u6x idiom — x86 needs no
// SYS_REPORT). ABI (Linux-style): rax = number, args rdi/rsi/rdx, return in rax; rcx/r11 are the
// SYSCALL-clobbered pair, so state rides r12-r15 (restored across syscalls like every other GPR).
core::arch::global_asm!(
    r#"
    .globl unaos_user_u6bx_blob_start
unaos_user_u6bx_blob_start:
    .balign 16
    .globl unaos_user_u6bx_file
unaos_user_u6bx_file:
    xor r12d, r12d                          // witness bitmask = 0 (survives syscalls)
    lea r14, [rip + unaos_user_u6bx_blob_start] // window base (the blob runs at the code-page base)
    mov r15, r14
    add r14, 0x1000                         // r14 -> read buffer (the writable DATA page, window page 1)
    add r15, 0x2000                         // r15 -> kernel-planted expected prefix (window page 2)

    // (0) SYS_OPEN("HELLO.BIN") -> a File handle carrying CAP_READ
    mov rax, 11                             // SYS_OPEN
    lea rdi, [rip + unaos_user_u6bx_name]   // name ptr (in the RO code page — ring-3 readable)
    mov rsi, [rip + unaos_user_u6bx_namelen] // name len (from the embedded length word)
    syscall
    mov r13, rax                            // r13 = handle (>= 0) or -errno
    test r13, r13
    js 1f                                   // open failed (negative) -> skip bit0 + the read/bytes checks
    add r12, 1                              // bit0: open OK

    // (1) SYS_READ(handle, buf, 16) -> exactly 16 bytes back
    mov rax, 12                             // SYS_READ
    mov rdi, r13                            // the File handle SYS_OPEN minted
    mov rsi, r14                            // dest buf (data page)
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 1f                                  // short/failed read -> skip bit1 + the bytes check
    add r12, 2                              // bit1: read OK (16 bytes)

    // (2) the 16 read bytes must equal the kernel-planted staged prefix (two 8-byte compares)
    mov rax, [r14]
    cmp rax, [r15]
    jne 1f
    mov rax, [r14 + 8]
    cmp rax, [r15 + 8]
    jne 1f
    add r12, 4                              // bit2: bytes match the staged source
1:
    // (3) a File handle WITHOUT CAP_READ (pre-endowed, backed by a real descriptor) -> -EACCES (the rights arm)
    mov rax, 12                             // SYS_READ
    mov rdi, 2                              // U6BX_NOCAP_IDX
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, -13                            // exactly -EACCES ?
    jne 2f
    add r12, 8                              // bit3: no-CAP_READ File -> -EACCES
2:
    // (4) a non-File handle (a Socket carrying CAP_READ, pre-endowed) -> -EACCES (the kind arm)
    mov rax, 12                             // SYS_READ
    mov rdi, 3                              // U6BX_SOCK_IDX
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, -13
    jne 3f
    add r12, 16                             // bit4: wrong-kind handle -> -EACCES
3:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U6BX_WITNESS
    mov rdi, r12
    syscall
4:  jmp 4b                                  // sys_exit never returns; belt-and-braces guard

    .balign 8
unaos_user_u6bx_namelen:
    .quad unaos_user_u6bx_name_end - unaos_user_u6bx_name
unaos_user_u6bx_name:
    .ascii "HELLO.BIN"
unaos_user_u6bx_name_end:
    .globl unaos_user_u6bx_blob_end
unaos_user_u6bx_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u6bx_blob_start: u8;
    static unaos_user_u6bx_blob_end: u8;
    static unaos_user_u6bx_file: u8;
}

// --- U7x ring-3 fixtures (cross-process capability transfer — the aarch64 `__u7_prog_{parent,child}`
// twins). TWO fixtures in one blob, run in two separate slots (each slot gets the whole blob; only the
// entry differs). Both are register-only apart from the RIP-relative message load and — the one x86
// twist — the CHILD's single store to its OWN RW data (the USED word at window +0x3008, its "first write
// through the transferred cap landed" cue; pi4 conveyed this via SYS_REPORT, which x86 does not have).
// The GO word at window +0x3000 is written ONLY by the kernel (the launcher, through the slot backing);
// the fixtures merely poll it.
//
// SEQUENCING (the x86 divergence from pi4's SYS_YIELD-cooperative co-location): x86 ring 3 is
// IF-masked/cooperative and has no yield syscall, so a polling fixture HOGS its core. Each fixture
// therefore runs on its OWN dedicated AP (the launcher on a third), and the polls are plain bounded
// spins: GO polls are pure user-memory loads (+`pause`); RECV polls re-issue the syscall until it stops
// returning -EAGAIN. Budgets are generous but finite — an exhausted budget falls through to the witness
// exit, so the verdict FAILs honestly instead of wedging the core forever.
//
// PARENT (`u7x-parent`, pre-endowed: U7X_DEST_IDX = a Child handle naming the child fixture;
// U7X_SRC_IDX = a full Console cap CAP_WRITE|CAP_GRANT):
//   (0) an OVER-RIGHTS transfer (req = CAP_WRITE|CAP_EXEC, bits the source lacks) must be -EACCES — the
//       attenuation invariant crosses processes intact;
//   (1) XFER t1 (req = CAP_WRITE) -> a transfer id (saved for the revoke);
//   then it spins on ITS GO word (the launcher releases it only after the child's first USE lands, so
//   the revoke is provably use-then-revoke);
//   (2) SYS_CAP XREVOKE(t1) -> 0;  (3) XFER t2 (the "revoke done" signal the child unblocks on) -> ok.
// CHILD (`u7x-child`, row deliberately EMPTY at spawn — the single-writer snapshot proves the parent's
// deposit never touched it): spins on its GO (released only after the launcher has verified the
// pending-deposit/untouched-row snapshot), then (0) RECV t1 -> h1; (1) WRITES through h1 (the
// transferred Console cap — the line lands on the console) and stores the USED word; (2) RECV t2;
// (3) the revoked h1 must now be -EACCES. Each conveys a 4-bit witness bitmask as its `sys_exit` STATUS
// (routed by name — the u5x idiom). ABI (Linux-style): rax = number, args rdi/rsi/rdx, return in rax;
// witness rides r12, saved values r13, budgets r14, the GO pointer rbx (all callee-saved).
core::arch::global_asm!(
    r#"
    .globl unaos_user_u7x_blob_start
unaos_user_u7x_blob_start:
    .balign 16
    .globl unaos_user_u7x_parent
unaos_user_u7x_parent:
    xor  r12d, r12d                         // witness bitmask = 0

    // (0) over-rights XFER: dest=U7X_DEST_IDX, src=U7X_SRC_IDX, req=CAP_WRITE|CAP_EXEC (6) -> -EACCES
    mov  rax, 13                            // SYS_XFER
    mov  rdi, 2                             // U7X_DEST_IDX (the Child handle)
    mov  rsi, 3                             // U7X_SRC_IDX (Console, CAP_WRITE|CAP_GRANT — no CAP_EXEC)
    mov  rdx, 6                             // req = CAP_WRITE|CAP_EXEC — would AMPLIFY -> must be refused
    syscall
    cmp  rax, -13                           // exactly -EACCES ?
    jne  1f
    or   r12, 1                             // b0: cross-process attenuation held
1:
    // (1) XFER t1: req = CAP_WRITE (2) -> transfer id >= 1
    mov  rax, 13                            // SYS_XFER
    mov  rdi, 2
    mov  rsi, 3
    mov  rdx, 2                             // req = CAP_WRITE (a strict subset of the source's rights)
    syscall
    mov  r13, rax                           // r13 = t1's transfer id (or -errno)
    test r13, r13
    js   9f                                 // deposit failed -> report the partial witness
    or   r12, 2                             // b1: t1 deposited

    // spin on the parent GO word (window +0x3000; the launcher releases it after the child's first USE)
    lea  rbx, [rip + unaos_user_u7x_blob_start]
    add  rbx, 0x3000                        // rbx = GO VA (the blob runs at the code-page base)
    mov  r14, 0x8000000                     // bounded poll budget (pure loads + pause; ~seconds of spin)
2:  mov  rax, [rbx]
    test rax, rax
    jnz  3f
    pause
    dec  r14
    jnz  2b
    jmp  9f                                 // GO never released -> partial witness (verdict FAILs)
3:
    // (2) revoke t1: SYS_CAP(CAP_OP_XREVOKE=2, transfer id) -> 0
    mov  rax, 10                            // SYS_CAP
    mov  rdi, 2                             // CAP_OP_XREVOKE
    mov  rsi, r13
    syscall
    test rax, rax
    jnz  9f
    or   r12, 4                             // b2: revoke accepted

    // (3) XFER t2 — the "revoke done" signal the child unblocks on
    mov  rax, 13                            // SYS_XFER
    mov  rdi, 2
    mov  rsi, 3
    mov  rdx, 2
    syscall
    test rax, rax
    js   9f
    or   r12, 8                             // b3: t2 deposited
9:  mov  rax, 2                             // SYS_EXIT(witness) -> routed by name into U7X_PARENT_WITNESS
    mov  rdi, r12
    syscall
10: jmp  10b                                // sys_exit never returns; belt-and-braces guard

    .balign 16
    .globl unaos_user_u7x_child
unaos_user_u7x_child:
    xor  r12d, r12d                         // witness bitmask = 0
    lea  rbx, [rip + unaos_user_u7x_blob_start]
    add  rbx, 0x3000                        // rbx = GO VA (USED word = GO + 8)

    // spin on the child GO (released only after the launcher's single-writer snapshot)
    mov  r14, 0x8000000                     // bounded poll budget
11: mov  rax, [rbx]
    test rax, rax
    jnz  12f
    pause
    dec  r14
    jnz  11b
    jmp  19f                                // never released -> report the (empty) witness

12: // (0) RECV t1 -> h1 (poll: -EAGAIN means the deposit is not yet visible — it always is by GO time)
    mov  r14, 0x100000                      // bounded syscall-poll budget
13: mov  rax, 14                            // SYS_RECV
    syscall
    test rax, rax
    jns  14f                                // >= 0 -> received
    pause
    dec  r14
    jnz  13b
    jmp  19f                                // nothing ever arrived -> partial witness
14: mov  r13, rax                           // r13 = h1 (the received, transferred Console cap)
    or   r12, 1                             // b0: received

    // (1) USE it: sys_write(h1, msg, len) — the transferred capability actually carries authority
    mov  rax, 1                             // SYS_WRITE
    mov  rdi, r13
    lea  rsi, [rip + unaos_user_u7x_msg]
    mov  rdx, [rip + unaos_user_u7x_msglen]
    syscall
    cmp  rax, 1
    jl   15f                                // write failed -> no USED store (the launcher's wait FAILs honestly)
    or   r12, 2                             // b1: used
    mov  qword ptr [rbx + 8], 1             // the USED word — the launcher's revoke cue (own RW data page)

15: // (2) RECV t2 — the parent's "revoke done" signal
    mov  r14, 0x100000
16: mov  rax, 14                            // SYS_RECV
    syscall
    test rax, rax
    jns  17f
    pause
    dec  r14
    jnz  16b
    jmp  19f
17: or   r12, 4                             // b2: t2 received

    // (3) the revoked h1 must now be STALE: sys_write(h1) -> -EACCES (single-level revoke enforced at use)
    mov  rax, 1                             // SYS_WRITE
    mov  rdi, r13
    lea  rsi, [rip + unaos_user_u7x_msg]
    mov  rdx, [rip + unaos_user_u7x_msglen]
    syscall
    cmp  rax, -13                           // exactly -EACCES ?
    jne  19f
    or   r12, 8                             // b3: revoke enforced
19: mov  rax, 2                             // SYS_EXIT(witness) -> routed by name into U7X_CHILD_WITNESS
    mov  rdi, r12
    syscall
20: jmp  20b                                // sys_exit never returns; belt-and-braces guard

    .balign 8
unaos_user_u7x_msglen:
    .quad unaos_user_u7x_msg_end - unaos_user_u7x_msg
unaos_user_u7x_msg:
    .ascii "u7x: child prints via the transferred cap\n"
unaos_user_u7x_msg_end:
    .globl unaos_user_u7x_blob_end
unaos_user_u7x_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u7x_blob_start: u8;
    static unaos_user_u7x_blob_end: u8;
    static unaos_user_u7x_parent: u8;
    static unaos_user_u7x_child: u8;
}

// --- U8x ring-3 fixture (revocation trees — the aarch64 `__u8_prog_tree` twin). ONE fixture (`u8x-tree`)
// exercising the derivation-ledger semantics observable from a SINGLE process: a grant CHAIN (grant ->
// re-grant) dies WHOLE when the PARENT capability — one carrying `CAP_REVOKE` — is revoked (the U7x escape
// #1, closed locally); a revoke WITHOUT `CAP_REVOKE` keeps U5x's ownership semantics (drops only the
// caller's own row entry — the derived copy survives); and a double revoke returns the correct errno with no
// ledger corruption. The CROSS-process half (re-transfer cascade + generation-tagged inboxes) is proven
// kernel-side by `u8_kernel_check` (it needs three cooperating processes — a fixture race would be staged,
// not real). Position-independent, register-only apart from the RIP-relative message loads (writes no user
// stack — safe on any slot under preemption). Pre-endowed by the launcher: index 2 = a console cap
// `CAP_WRITE|CAP_GRANT|CAP_REVOKE` (the revocable parent), index 3 = a console cap `CAP_WRITE|CAP_GRANT`
// (no CAP_REVOKE — the locality negative). It builds a witness bitmask in r12 (callee-saved, the u5x idiom)
// and conveys it as its `sys_exit` STATUS, routed BY NAME into `U8X_WITNESS` (x86 has no SYS_REPORT — the
// aarch64 twin uses one only because its sentinel exit status is reserved for demo routing). ABI
// (Linux-style): rax = number, args rdi/rsi/rdx, return in rax; witness rides r12, the derived handles
// r13/r14/r15 (all callee-saved — preserved across each `syscall`). Errnos: -EACCES=-13, -ECHILD=-10.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u8x_blob_start
unaos_user_u8x_blob_start:
    .balign 16
    .globl unaos_user_u8x_tree
unaos_user_u8x_tree:
    xor  r12d, r12d                         // witness bitmask = 0 (callee-saved; survives syscalls)

    // (0) the grant CHAIN: g1 = GRANT(src=2, CAP_WRITE|CAP_GRANT) -> g2 = GRANT(g1, CAP_WRITE) -> write
    //     through g2 lands (a two-deep derived capability carries real authority pre-revoke)
    mov  rax, 10                            // SYS_CAP
    xor  edi, edi                           // CAP_OP_GRANT (0)
    mov  rsi, 2                             // src = U8X_SRC_IDX (CAP_WRITE|CAP_GRANT|CAP_REVOKE)
    mov  rdx, 0xA                           // req = CAP_WRITE|CAP_GRANT (a strict subset)
    syscall
    test rax, rax
    js   1f                                 // grant failed -> skip bit0
    mov  r13, rax                           // r13 = g1 (the child cap)
    mov  rax, 10                            // re-grant: g2 = GRANT(g1, CAP_WRITE)
    xor  edi, edi
    mov  rsi, r13
    mov  rdx, 2                             // CAP_WRITE
    syscall
    test rax, rax
    js   1f
    mov  r14, rax                           // r14 = g2 (the grandchild cap)
    mov  rax, 1                             // write through g2 -> must land
    mov  rdi, r14
    lea  rsi, [rip + unaos_user_u8x_msg1]
    mov  rdx, [rip + unaos_user_u8x_msg1len]
    syscall
    test rax, rax
    js   1f
    or   r12, 1                             // bit0: grant chain works
1:
    // (1) revoke the PARENT: index 2 carries CAP_REVOKE, so this kills the derivation SUBTREE -> 0; then
    //     immediately revoke it AGAIN -> exactly -ECHILD (the double-revoke errno, checked HERE while index
    //     2 is provably still Empty — a later first-free grant may legitimately reuse it)
    mov  rax, 10                            // SYS_CAP
    mov  rdi, 1                             // CAP_OP_REVOKE
    mov  rsi, 2
    syscall
    test rax, rax
    jnz  2f                                 // revoke must return 0
    mov  rax, 10                            // double revoke of 2 -> -ECHILD (-10; the row is Empty now)
    mov  rdi, 1
    mov  rsi, 2
    syscall
    cmp  rax, -10                           // exactly -ECHILD ?
    jne  2f
    or   r12, 2                             // bit1: parent revoke accepted; double revoke errno'd
2:
    // (2) BOTH descendant copies are now stale at use: write via g1 -> -EACCES; write via g2 -> -EACCES
    mov  rax, 1
    mov  rdi, r13
    lea  rsi, [rip + unaos_user_u8x_msg1]
    mov  rdx, [rip + unaos_user_u8x_msg1len]
    syscall
    cmp  rax, -13                           // exactly -EACCES ?
    jne  3f
    mov  rax, 1
    mov  rdi, r14
    lea  rsi, [rip + unaos_user_u8x_msg1]
    mov  rdx, [rip + unaos_user_u8x_msg1len]
    syscall
    cmp  rax, -13
    jne  3f
    or   r12, 4                             // bit2: the whole subtree died with the parent
3:
    // (3) locality + errno negatives: g4 = GRANT(src=3, CAP_WRITE); revoke index 3 (NO CAP_REVOKE) -> 0 but
    //     LOCAL only — g4 still writes; then a double revoke of 3 -> exactly -ECHILD
    mov  rax, 10
    xor  edi, edi                           // CAP_OP_GRANT
    mov  rsi, 3                             // src = U8X_SRC2_IDX (CAP_WRITE|CAP_GRANT — no CAP_REVOKE)
    mov  rdx, 2                             // CAP_WRITE
    syscall
    test rax, rax
    js   4f
    mov  r15, rax                           // r15 = g4
    mov  rax, 10                            // revoke index 3 — right-less, so row-local only
    mov  rdi, 1
    mov  rsi, 3
    syscall
    test rax, rax
    jnz  4f
    mov  rax, 1                             // g4 must STILL write (no CAP_REVOKE => no subtree kill)
    mov  rdi, r15
    lea  rsi, [rip + unaos_user_u8x_msg2]
    mov  rdx, [rip + unaos_user_u8x_msg2len]
    syscall
    test rax, rax
    js   4f
    mov  rax, 10                            // double revoke of 3 -> -ECHILD (-10; already Empty)
    mov  rdi, 1
    mov  rsi, 3
    syscall
    cmp  rax, -10
    jne  4f
    or   r12, 8                             // bit3: right-less revoke stayed local; double revoke errno'd
4:
    mov  rax, 2                             // SYS_EXIT(witness) -> routed by name into U8X_WITNESS
    mov  rdi, r12
    syscall
5:  jmp  5b                                 // sys_exit never returns; belt-and-braces guard
    // Read-only data in the (ring-3 RX, kernel-readable) code page. Lengths are assemble-time label
    // differences loaded RIP-relative (the u5x/u7x idiom).
    .balign 8
unaos_user_u8x_msg1len:
    .quad unaos_user_u8x_msg1_end - unaos_user_u8x_msg1
unaos_user_u8x_msg1:
    .ascii "u8x: write via the grandchild cap\n"
unaos_user_u8x_msg1_end:
    .balign 8
unaos_user_u8x_msg2len:
    .quad unaos_user_u8x_msg2_end - unaos_user_u8x_msg2
unaos_user_u8x_msg2:
    .ascii "u8x: right-less revoke stays local\n"
unaos_user_u8x_msg2_end:
    .globl unaos_user_u8x_blob_end
unaos_user_u8x_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u8x_blob_start: u8;
    static unaos_user_u8x_blob_end: u8;
    static unaos_user_u8x_tree: u8;
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
    // U6x: a killed printing-spawner fixture — off the U1b counter (a kill here is a real U6x bug that fails
    // the U6x verdict, not a phantom U1b regression). It is not in PROCS (the launcher spawned it, not
    // sys_spawn), so no parent semaphore to post — the launcher times out to FAIL on `U6X_DONE`. (A killed
    // U6x CHILD shares the `u4x-child` name above — its kill posts a non-zero status that fails the reap.)
    if name == "u6x-spawn" {
        U6X_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U6bx: a killed File-handle fixture — off the U1b counter (a kill here is a real U6bx bug that fails
    // the U6bx verdict, not a phantom U1b regression). It is not in PROCS (the launcher spawned it, not
    // sys_spawn), so no parent semaphore to post — the launcher times out to FAIL on `U6BX_DONE`.
    if name == "u6bx-file" {
        U6BX_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U7x: the transfer fixtures are well-behaved (register-only apart from the child's own-data USED
    // store; they fault on nothing). A kill here is a real U7x bug — route it to its own counter, never
    // the U1b `killed_unexpected` count, so a U7x fault fails only the U7x verdict. A killed CHILD's
    // launcher-planted Proc entry goes EXITED too (the exit-arm twin), so the pid->slot map never
    // vouches for a dead recipient. No `done` post — the launcher waits on deadline counters.
    if name == "u7x-parent" || name == "u7x-child" {
        U7X_KILLED.fetch_add(1, Ordering::AcqRel);
        let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
        if let Some(id) = crate::arch::sched::current_task_id(cpu) {
            if let Some(i) = proc_find_running(id) {
                PROCS[i].state.store(PEXITED, Ordering::Release);
            }
        }
        return;
    }
    // U8x: the revocation-tree fixture is register-only and well-behaved; a kill is a real U8x bug — its own
    // counter, never the U1b `killed_unexpected` count. Not in PROCS (the launcher spawned it, not
    // sys_spawn), so no parent semaphore to post — the launcher times out to FAIL on `U8X_DONE`.
    if name == "u8x-tree" {
        U8X_KILLED.fetch_add(1, Ordering::AcqRel);
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
        SYS_OPEN => sys_open(a0, a1),
        SYS_READ => sys_read(a0, a1, a2),
        SYS_XFER => sys_xfer(a0, a1, a2),
        SYS_RECV => sys_recv(),
        SYS_EXIT => {
            // U7x: the transfer fixtures exit BY NAME, BEFORE the Proc short-circuit below — the CHILD
            // has a launcher-PLANTED Proc entry (the pid->slot map sys_xfer resolves its dest through),
            // so without this precedence its exit would be swallowed by the child-reap path (posting a
            // `done` nobody waits on) and `U7X_DONE` would never gate the verdict. The exit STATUS is
            // the fixture's witness bitmask (the u5x idiom — x86 has no SYS_REPORT); the parent (not in
            // PROCS) rides the same arm for symmetry. The planted entry is marked EXITED so a late
            // sys_xfer to this recipient fails the RUNNING check instead of depositing into a torn-down
            // inbox; the launcher frees the entry after its verdict (no `done` post — it waits on the
            // counter).
            {
                let nm = crate::arch::sched::current_name();
                if nm == Some("u7x-parent") || nm == Some("u7x-child") {
                    if nm == Some("u7x-parent") {
                        U7X_PARENT_WITNESS.store(a0 as u32, Ordering::Release);
                    } else {
                        U7X_CHILD_WITNESS.store(a0 as u32, Ordering::Release);
                    }
                    U7X_DONE.fetch_add(1, Ordering::AcqRel);
                    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
                    if let Some(id) = crate::arch::sched::current_task_id(cpu) {
                        if let Some(i) = proc_find_running(id) {
                            PROCS[i].state.store(PEXITED, Ordering::Release);
                        }
                    }
                    crate::arch::sched::exit(); // never returns
                }
            }
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
                Some("u6x-spawn") => {
                    // U6x: the printing spawner conveys its 4-bit witness bitmask as its exit STATUS (routed
                    // by name, same as u5x-cap — x86 has no SYS_REPORT). Its two children exit via the
                    // pid-keyed short-circuit above (they are `u4x-child`s), so only the parent reaches here.
                    // Stored for the launcher's verdict; `U6X_DONE` gates the read.
                    U6X_WITNESS.store(a0 as u32, Ordering::Release);
                    U6X_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u6bx-file") => {
                    // U6bx: the File-handle fixture conveys its 5-bit witness bitmask as its exit STATUS
                    // (routed by name, same as u5x-cap/u6x-spawn — x86 has no SYS_REPORT). Stored for the
                    // launcher's verdict; `U6BX_DONE` gates the read.
                    U6BX_WITNESS.store(a0 as u32, Ordering::Release);
                    U6BX_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u8x-tree") => {
                    // U8x: the revocation-tree fixture conveys its 4-bit witness bitmask as its exit STATUS
                    // (routed by name, same idiom). It has NO planted Proc entry (a single register-only
                    // fixture — the kernel-side check plants its own scratch entries), so it takes the
                    // ordinary by-name path, not the u7x pid-keyed short-circuit above. `U8X_DONE` gates
                    // the launcher's read.
                    U8X_WITNESS.store(a0 as u32, Ordering::Release);
                    U8X_DONE.fetch_add(1, Ordering::AcqRel);
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
const EINVAL: i64 = -22; // a SYS_CAP sub-op neither GRANT nor REVOKE; a malformed SYS_OPEN name length
// U6bx File-handle errnos (the aarch64 U6b set, minus the mount/dir codes a staged set cannot produce).
const EFAULT: i64 = -14; // a user range outside the window (or, for a SYS_READ dest, not writable within it)
const ENOENT: i64 = -2; // SYS_OPEN: no staged file by that name (the staged set is the x86 "volume")
const EIO: i64 = -5; // SYS_READ: a live descriptor over an unstaged source — a kernel bug; fail closed
const EMFILE: i64 = -24; // SYS_OPEN: the caller's open-file table is full

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

// =============================================================================================
// U6bx: REAL File handles — the staged-file set + per-task open-file descriptors + SYS_OPEN/SYS_READ
// (the aarch64 pi4 U6b twin, keyed by SLOT/row instead of ASID).
// =============================================================================================
//
// pi4's U6b reads the disk INSIDE the SVC handler — its EMMC2 driver is PIO, so an in-handler FAT walk is
// just MMIO polling. x86 CANNOT mirror that: storage is USB-over-xHCI, whose BOT read pump `hlt()`s
// awaiting the transfer event, and the SYSCALL handler runs IF-masked (SFMASK) — an in-handler disk read
// would sleep a core that can never wake (the exact U4x sys_spawn divergence; the STORAGE / IF NOTE
// above). The honest x86 scope: `SYS_OPEN` opens a file from the BSP-STAGED set — files the BSP pre-read
// at IF=1 over the proven U2 FAT path — and `SYS_READ` serves the staged bytes. The CAPABILITY layer (the
// File kind + CAP_READ + the single `handle_resolve` CHECK, the descriptor sidecars, the reserve-last/
// unwind open, the teardown-clear) is byte-for-byte the pi4 twin; ONLY the source of the bytes differs.
// Arbitrary-runtime-file open awaits an interrupt-driven / IF-safe storage path (deferred).

/// The staged-file NAME table: index k names the source `staged_bytes(k)` serves. ONE entry today —
/// HELLO.BIN, the buffer `stage_hello` fills for sys_spawn (shared; written once, then read-only). A
/// future file rides by adding its name here + a stage buffer + a `staged_bytes` arm.
const STAGED_NAMES: [&str; 1] = ["HELLO.BIN"];
/// Upper bound on a SYS_OPEN name (the aarch64 twin's `MAX_NAME`): a dotted 8.3 name is at most 12 bytes.
const MAX_NAME: usize = 12;

/// The staged bytes behind staged-file `idx`, or `None` if that stage has not published. Index 0 =
/// HELLO.BIN = `HELLO_BYTES[..HELLO_LEN]`, gated by `HELLO_STAGED` (Acquire, pairing with the
/// `stage_hello` Release) — written ONCE on the BSP before any consumer could hold a descriptor, then
/// read-only, so the returned slice is stable for the rest of the boot (the sys_spawn contract).
fn staged_bytes(idx: u32) -> Option<&'static [u8]> {
    match idx {
        0 if HELLO_STAGED.load(Ordering::Acquire) => {
            let len = HELLO_LEN.load(Ordering::Acquire);
            Some(unsafe { core::slice::from_raw_parts((&raw const HELLO_BYTES).cast::<u8>(), len) })
        }
        _ => None,
    }
}

/// Look a validated name up in the staged set: `Some((staged idx, size))` iff that file is staged NOW.
/// The size comes from the staged bytes (not a directory entry) — the staged set IS the x86 "volume".
fn staged_lookup(name: &str) -> Option<(u32, u32)> {
    for (k, n) in STAGED_NAMES.iter().enumerate() {
        if *n == name {
            return staged_bytes(k as u32).map(|b| (k as u32, b.len() as u32));
        }
    }
    None
}

// --- Per-task open-file descriptors: parallel atomic sidecars keyed `[row][idx]` exactly like
// HANDLES/HANDLE_RIGHTS/HANDLE_KIND (`SHARED_ROW` included), so a File handle's value word carries only
// the +1-biased descriptor index (never the `0`/`u64::MAX` sentinels). Access is single-writer per row at
// any instant — a row is populated ONLY before its task is dispatched (`u6bx_launcher`'s pre-endow) or BY
// that one task mid-syscall (IF-masked), and cleared at teardown after the task exits — so the
// Release/Acquire discipline is belt-and-braces (the HANDLE_RIGHTS twin). Presence is a dedicated
// `FILE_USED` flag (NOT an overloaded index sentinel), so descriptor 0 is representable. Read-only,
// sequential (no seek) — the arc's scope, mirroring pi4 U6b. ---
const NFILE: usize = 4; // open files per task (small, static — the demo opens at most two per row)

/// Per-descriptor presence: `true` == `[row][idx]` holds a live open file. Claimed (CAS false->true) in
/// `files_alloc`, cleared in `files_free`/`clear_files_row`. The single source of truth for "is this
/// file-id valid" — `sys_read` re-checks it after decoding a handle's file-id (defense in depth).
static FILE_USED: [[AtomicBool; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicBool::new(false) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// The open file's STAGED-SET index (which staged source serves it — the x86 stand-in for pi4's
/// `FILE_CLUSTER` chain head). Meaningful only where `FILE_USED`.
static FILE_STAGED: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// The open file's total byte size (the EOF bound `sys_read` clamps against). Meaningful only where `FILE_USED`.
static FILE_SIZE: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// The sequential read offset — advanced by exactly the count each `sys_read` delivers. Meaningful only
/// where `FILE_USED`. No seek this arc: the offset only ever moves forward.
static FILE_OFFSET: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];

/// Claim the first free descriptor in the caller's FILES row for a freshly-opened file, returning its
/// index (the caller biases it to the file-id `idx + 1`). Publishes staged-idx/size/offset after the
/// `FILE_USED` CAS claim — safe because a resolver only reaches a descriptor via a File HANDLE, which
/// `sys_open` installs strictly AFTER this returns (stored Release regardless, belt-and-braces — the pi4
/// `files_alloc` discipline). `None` if the row is full (-> `-EMFILE`; never grown).
fn files_alloc(row: usize, staged_idx: u32, size: u32) -> Option<usize> {
    debug_assert!(row < FILE_USED.len(), "files_alloc: row out of range");
    for k in 0..NFILE {
        if FILE_USED[row][k].compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
        {
            FILE_STAGED[row][k].store(staged_idx, Ordering::Release);
            FILE_SIZE[row][k].store(size, Ordering::Release);
            FILE_OFFSET[row][k].store(0, Ordering::Release);
            return Some(k);
        }
    }
    None
}

/// Release descriptor `idx` in the caller's FILES row — the unwind for a `sys_open` that allocated a
/// descriptor but then failed to install its handle (the sys_spawn reserve/unwind discipline). Clears the
/// fields then drops `FILE_USED` LAST (Release), so the slot is never seen free with stale fields.
fn files_free(row: usize, idx: usize) {
    debug_assert!(row < FILE_USED.len() && idx < NFILE, "files_free: out of range");
    FILE_STAGED[row][idx].store(0, Ordering::Release);
    FILE_SIZE[row][idx].store(0, Ordering::Release);
    FILE_OFFSET[row][idx].store(0, Ordering::Release);
    FILE_USED[row][idx].store(false, Ordering::Release);
}

/// Clear an ENTIRE per-task open-file row at teardown — the file twin of `clear_handle_row`'s handle wipe
/// (which calls this). Presence dropped last per descriptor, so no torn intermediate looks live. `slot`
/// is a PRIVATE slot (0..USER_SLOTS); `SHARED_ROW` is never torn down (its opens persist, like its caps).
fn clear_files_row(slot: usize) {
    debug_assert!(slot < crate::arch::memory::USER_SLOTS, "clear_files_row: not a private slot");
    for k in 0..NFILE {
        FILE_STAGED[slot][k].store(0, Ordering::Release);
        FILE_SIZE[slot][k].store(0, Ordering::Release);
        FILE_OFFSET[slot][k].store(0, Ordering::Release);
        FILE_USED[slot][k].store(false, Ordering::Release);
    }
}

/// True iff the entire FILES row for `row` is free — the U6bx teardown-clear verifier (the file twin of
/// `handle_row_is_clear`, the aarch64 `files_row_is_clear` twin). Read by `u6bx_launcher` after the
/// fixture exits and its slot retires: teardown clears the row, transitioning this false->true, proving
/// no open file outlives its owning slot.
fn files_row_is_clear(row: usize) -> bool {
    debug_assert!(row < FILE_USED.len(), "files_row_is_clear: row out of range");
    (0..NFILE).all(|k| !FILE_USED[row][k].load(Ordering::Acquire))
}

/// SYS_OPEN(name_ptr, name_len) -> a handle index, or a negative errno. The FIRST resource-OPEN through
/// the object table (the aarch64 U6b twin): validate + copy the name, look it up in the STAGED set,
/// record an open-file descriptor in the caller's FILES row, and install a `File` handle (first-free)
/// carrying `CAP_READ` — the capability a later SYS_READ presents.
///
/// Ordering mirrors the twin (and sys_spawn): every fallible READ-ONLY lookup first (name bound/copy,
/// staged lookup), so a failure there returns with nothing to unwind; RESOURCES claimed last (a
/// descriptor, then a handle). The one failure that must unwind is a full handle table AFTER a descriptor
/// was claimed — `files_free` it, then `-EAGAIN`. Errnos: `-EINVAL` (bad name length), `-EFAULT` (name
/// range outside the window), `-ENOENT` (not in the staged set — non-UTF-8 names match nothing),
/// `-EMFILE` (FILES row full), `-EAGAIN` (handle table full).
fn sys_open(name_ptr: u64, name_len: u64) -> i64 {
    let row = caller_row();
    // 1. Bound + copy the name — the sys_write pointer discipline: the WHOLE range inside the user
    //    window (overflow rejected), then a bounded direct read (ring-3 VA == kernel VA in the live CR3).
    let n = name_len as usize;
    if n == 0 || n > MAX_NAME {
        return EINVAL;
    }
    let window = USER_WINDOW_PAGES * PAGE_SIZE;
    let end = name_ptr.wrapping_add(name_len);
    if end < name_ptr || name_ptr < USER_BASE || end > USER_BASE + window {
        return EFAULT;
    }
    let mut namebuf = [0u8; MAX_NAME];
    namebuf[..n].copy_from_slice(unsafe { core::slice::from_raw_parts(name_ptr as *const u8, n) });
    let Ok(name) = core::str::from_utf8(&namebuf[..n]) else {
        return ENOENT; // a non-UTF-8 name matches no staged entry
    };
    // 2. Read-only lookup — nothing claimed yet, so a miss returns cleanly.
    let Some((sidx, size)) = staged_lookup(name) else {
        return ENOENT;
    };
    // 3. Claim resources LAST — a descriptor, then a handle. Only a full handle table needs unwinding.
    let Some(fid) = files_alloc(row, sidx, size) else {
        return EMFILE; // this task's open-file row is full
    };
    let file_id = (fid + 1) as u64; // +1 bias: never the value word's 0 (Empty) / u64::MAX (RESERVING)
    let Some(h) = handle_install(row, HANDLE_RESERVING) else {
        files_free(row, fid); // no handle slot — release the descriptor we just claimed (no leak)
        return EAGAIN;
    };
    // Publish the kind + rights, then the live file-id LAST (Release) — a resolver that observes the live
    // value also observes File + CAP_READ. Single-writer over this row (mid-syscall); belt-and-braces.
    handle_set_kind(row, h, KIND_FILE);
    handle_set_rights(row, h, CAP_READ);
    handle_set(row, h, file_id);
    h as i64
}

/// SYS_READ(handle, buf, len) -> the byte count (`0` = EOF), or a negative errno. The object table's
/// first resource-read CHECK on a non-Console object: `handle_resolve(row, handle, CAP_READ)` must yield
/// a `File`. A missing right (`Denied`), a non-File kind (Console/Child/Socket), or no handle (Empty/oob)
/// ALL return `-EACCES` — the single enforcement point, the twin of `sys_write`'s Console+CAP_WRITE. Then
/// it clamps the request to the bytes left from the descriptor's offset, validates the WHOLE destination
/// up front (a bad buffer is `-EFAULT` with no copy and NO offset change), serves the bytes from the
/// staged source (the honest x86 divergence — see the section note), and advances the offset by exactly
/// the count delivered. Sequential — no seek.
fn sys_read(handle: u64, buf: u64, len: u64) -> i64 {
    let row = caller_row();
    // The CHECK: File + CAP_READ, or -EACCES. Identical shape to sys_write's Console + CAP_WRITE resolve.
    let file_id = match handle_resolve(row, handle, CAP_READ) {
        Ok(HandleTarget::File(id)) => id,
        _ => return EACCES,
    };
    // Decode the file-id -> descriptor index (undo the +1 bias) and re-check presence (defense in depth:
    // a live File handle always has a live descriptor, but never trust the value word blindly).
    let Some(idx) = (file_id as usize).checked_sub(1) else {
        return EACCES;
    };
    if idx >= NFILE || !FILE_USED[row][idx].load(Ordering::Acquire) {
        return EACCES;
    }
    let size = FILE_SIZE[row][idx].load(Ordering::Acquire);
    // U7x (folding the ledgered U6bx note): the offset advance is now a tx-exact CAS CLAIM, not a
    // load->store — two SHARED_ROW tasks racing one descriptor each claim a DISJOINT byte range (the
    // winner advances the offset before copying; the loser re-reads and claims the next range), so
    // concurrent reads are well-defined instead of double-delivering one range. Private slots keep their
    // single-writer discipline untouched (the CAS never retries there). The destination is validated
    // BEFORE the claim, so an -EFAULT still moves no offset and loses no bytes.
    let window_end = USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE;
    let (offset, want) = loop {
        let offset = FILE_OFFSET[row][idx].load(Ordering::Acquire);
        // Bytes available from the current offset, clamped to the request. `offset` advances only by
        // claimed counts and never exceeds `size`, so the subtraction cannot underflow; 0 = clean EOF.
        let want = core::cmp::min(len as usize, size.saturating_sub(offset) as usize);
        if want == 0 {
            return 0; // EOF, or the caller requested nothing
        }
        // Validate the WHOLE destination BEFORE any claim or copy — and it must be WRITABLE user memory:
        // inside the window AND past the read-only code page (page 0 is ring3-RX/RO; a kernel store there
        // would either fault under CR0.WP or corrupt W^X-protected code — excluded by construction, the
        // x86 stand-in for the twin's `user_range_ok(.., writable)`). A bad buffer is -EFAULT with no
        // copy and no offset move.
        let end = buf.wrapping_add(want as u64);
        if end < buf || buf < USER_BASE + PAGE_SIZE || end > window_end {
            return EFAULT;
        }
        if FILE_OFFSET[row][idx]
            .compare_exchange(offset, offset + want as u32, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break (offset, want); // the range [offset, offset+want) is now exclusively ours
        }
    };
    // Serve from the staged source (written once, read-only — stable across the whole boot).
    let Some(src) = staged_bytes(FILE_STAGED[row][idx].load(Ordering::Acquire)) else {
        return EIO; // a live descriptor over an unstaged source is a kernel bug; fail closed
    };
    // offset..offset+want lies inside the staged bytes: `size` was captured from this same source at
    // open, the source never shrinks (written once), and offset only advances by claimed counts. The
    // defensive clamp keeps even a violated assumption from over-reading the source (the claim above
    // already advanced the offset by `want`; a short `got` here is an impossible-source-shrink fail-safe,
    // not a real path).
    let got = core::cmp::min(want, src.len().saturating_sub(offset as usize));
    if got == 0 {
        return 0; // treat a (impossible) source shrink as EOF rather than over-read
    }
    // The dest range was validated above and the ring-3 VA equals the kernel VA in the live CR3 (the
    // sys_write discipline, write-side): a plain bounded copy into the caller's RW pages.
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr().add(offset as usize), buf as *mut u8, got);
    }
    got as i64
}

// =============================================================================================
// U7x: cross-process capability transfer — the per-SLOT transfer INBOX, the sender-owned transfer
// RECORDS (the revoke ledger), and SYS_XFER / SYS_RECV / SYS_CAP-XREVOKE. The aarch64 pi4 U7 twin,
// keyed by address-space SLOT/row instead of ASID.
// =============================================================================================
//
// THE INVARIANT THIS DESIGN EXISTS TO PRESERVE: every `HANDLES[row]` row has exactly ONE writer — its
// own task (U4x's lock-free foundation). A naive transfer where sender A writes recipient B's row would
// break that. So A never touches B's row: A deposits an attenuated `(kind, target, rights)` descriptor
// into B's per-slot INBOX — the one deliberately cross-slot surface, where every claim/consume/retract
// is a tx-exact CAS — and B pulls it into its OWN row with SYS_RECV (B writes only itself). Revocation
// reaches B the same one-way: the sender flips a bit in ITS OWN transfer RECORD, and B's next
// `handle_resolve` of the received cap reads it (the read-side hook in handle_resolve) — nobody ever
// writes another task's row. Delegation is OWNER-SCOPED: the recipient is named by a `Child` handle in
// the SENDER's table (no global process namespace).
//
// Slot/record state words reuse the handle protocol: `0` = free, `HANDLE_RESERVING` (u64::MAX) = an
// in-flight claim, anything else = live (a slot holds the transfer id; a record holds its tx). Sidecars
// are published (Release) BEFORE the state word goes live and read (Acquire) after observing it — the
// HANDLE_RIGHTS discipline. Transfer ids are globally unique (a monotonic counter), which is what makes
// the tx-exact CASes ABA-safe: a consumer, a retracting sender, and a tearing-down recipient can race
// and exactly one wins each slot.
//
// x86 divergences from the pi4 twin (both from the slot keying): (a) the pid->recipient-row map is the
// new `Proc.slot` (+1-biased; pi4 added `Proc.asid`); (b) the SHARED_ROW — the shared kernel window,
// which is never torn down and belongs to no single process — is refused as a transfer ENDPOINT: a
// shared-window caller gets `-EACCES` from both SYS_XFER and SYS_RECV (a transfer into an immortal,
// multi-tenant row could never be safely torn down or revoked-by-teardown; pi4 had no such row).
//
// Scope (mirrors pi4 exactly): single-LEVEL revoke (no cascade through re-transfers — revocation TREES
// are deferred); Console/Socket payloads only (`File` is refused: a file-id indexes the SENDER's
// per-row FILES table, so a cross-row File transfer needs descriptor migration — deferred with writes/
// seek; `Child` is refused: delegating reap rights is a process-model question, not a transfer one);
// records are a small fixed ledger (`MAX_XFERS`) whose lifetime IS the transfer's (claimed at XFER,
// released by whichever of handle-drop / pending-discard / sender-retract / recipient-teardown ends
// it). One residual TOCTOU is accepted and documented at the sys_xfer post-check.

/// Pending-transfer slots per recipient (per row). Small and static, like NHANDLE — a full inbox is
/// `-EAGAIN` (the sender retries or gives up), never grown.
const NXFER: usize = 4;
/// Sender-side transfer records (the revoke ledger), global — each live transfer holds exactly one.
const MAX_XFERS: usize = 8;
/// Bit 63 of a RECORD's TX word marks the (still-live) transfer REVOKED. The flag rides IN the state word
/// so the revoke is a **tx-exact CAS** like every other transition — a separate flag word would race the
/// free/reclaim cycle (the pi4 review-confirmed stale-revoke: a delayed store landing on a freed-and-
/// reclaimed record would revoke an unrelated sender's fresh transfer, or mint one born-revoked). txids
/// are a monotonic counter from 1, so bit 63 is never set on a genuine id; `txid | BIT` can never alias
/// `RESERVING` (that would need `txid == i64::MAX`).
const XFER_REVOKED_BIT: u64 = 1 << 63;

/// The inbox slot's STATE word: 0 = free, `HANDLE_RESERVING` = mid-claim, else = the transfer id (live).
/// `USER_SLOTS + 1` rows for index symmetry with HANDLES; the SHARED_ROW row exists but is refused as an
/// endpoint (see the section note), so it stays permanently clear.
static XFER_SLOT_TX: [[AtomicU64; NXFER]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NXFER] }; crate::arch::memory::USER_SLOTS + 1];
/// The pending descriptor: what kind of object the transferred cap names. Meaningful only where TX is live.
static XFER_SLOT_KIND: [[AtomicU8; NXFER]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU8::new(KIND_EMPTY) }; NXFER] }; crate::arch::memory::USER_SLOTS + 1];
/// The pending descriptor's target payload (the value word the received handle will carry).
static XFER_SLOT_TARGET: [[AtomicU64; NXFER]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NXFER] }; crate::arch::memory::USER_SLOTS + 1];
/// The pending descriptor's (already attenuated) rights.
static XFER_SLOT_RIGHTS: [[AtomicU32; NXFER]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NXFER] }; crate::arch::memory::USER_SLOTS + 1];
/// The record index + 1 backing this pending transfer (0 = none — a kernel bug on a live slot).
static XFER_SLOT_REC: [[AtomicU32; NXFER]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NXFER] }; crate::arch::memory::USER_SLOTS + 1];

/// A record's STATE word: 0 = free, `HANDLE_RESERVING` = mid-claim, `txid` = the live transfer it
/// ledgers, `txid | XFER_REVOKED_BIT` = that transfer, revoked (read by `handle_resolve` — the received
/// cap goes stale — and by `sys_recv` — a still-pending revoked transfer is discarded, never delivered).
static XFER_REC_TX: [AtomicU64; MAX_XFERS] = [const { AtomicU64::new(0) }; MAX_XFERS];
/// The row (slot, as u64) that made the transfer — only IT may XREVOKE (sender-owned; checked in
/// sys_cap_xrevoke). Disowned to `u64::MAX` (never a real row) when the sender's slot tears down, so
/// revoke authority dies with the sender instead of passing to the slot's next tenant.
static XFER_REC_SENDER: [AtomicU64; MAX_XFERS] = [const { AtomicU64::new(0) }; MAX_XFERS];
/// The next transfer id — globally unique, monotonic from 1 (never 0/u64::MAX, the state sentinels).
static XFER_NEXT_TX: AtomicU64 = AtomicU64::new(1);

/// Which transfer RECORD (index + 1; 0 = not a transferred cap) a RECEIVED handle references — the
/// revocation hook `handle_resolve` reads. Keyed `[row][idx]` like the other handle sidecars, and — the
/// point — written ONLY by the row's own task (`sys_recv`) or its teardown: the sender reaches a received
/// cap exclusively through the record, never through this row.
static HANDLE_XFER_REC: [[AtomicU32; NHANDLE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NHANDLE] }; crate::arch::memory::USER_SLOTS + 1];

/// Claim a free transfer record and mint its transfer id: CAS the state word 0 -> RESERVING, publish the
/// sender (Release), then the tx LAST (live-last, the handle_install discipline; the revoked flag needs no
/// reset — it lives in the TX word this store replaces whole).
fn xfer_rec_claim(sender_row: u64) -> Option<(usize, u64)> {
    for r in 0..MAX_XFERS {
        if XFER_REC_TX[r]
            .compare_exchange(0, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            XFER_REC_SENDER[r].store(sender_row, Ordering::Release);
            let tx = XFER_NEXT_TX.fetch_add(1, Ordering::AcqRel);
            XFER_REC_TX[r].store(tx, Ordering::Release);
            return Some((r, tx));
        }
    }
    None
}

/// Release a transfer record — by exactly ONE of: the received handle's drop (`handle_clear`), a pending
/// revoked transfer's discard (`sys_recv`), the sender's retract (`sys_xfer` post-check), or the
/// recipient's inbox teardown. Fields cleared first, the state word freed LAST (the files_free shape).
fn xfer_rec_free(r: usize) {
    debug_assert!(r < MAX_XFERS, "xfer_rec_free: out of range");
    XFER_REC_SENDER[r].store(0, Ordering::Release);
    XFER_REC_DERIV[r].store(0, Ordering::Release); // U8x: the derivation sidecar clears with the record
    XFER_REC_DERIV_ID[r].store(0, Ordering::Release);
    XFER_REC_TX[r].store(0, Ordering::Release); // clears the revoked bit with the id (one word)
}

/// True iff the whole record ledger is free — the U7x leak verifier (every transfer's lifetime closed).
fn xfer_recs_all_free() -> bool {
    (0..MAX_XFERS).all(|r| XFER_REC_TX[r].load(Ordering::Acquire) == 0)
}

/// Zero a CLAIMED inbox slot's descriptor fields and free the slot (state word 0 LAST). The caller must
/// OWN the slot — i.e. hold its tx-exact CAS win (consume/retract/teardown) or its RESERVING claim.
fn xfer_slot_release(row: usize, k: usize) {
    debug_assert!(row < XFER_SLOT_TX.len() && k < NXFER, "xfer_slot_release: out of range");
    XFER_SLOT_KIND[row][k].store(KIND_EMPTY, Ordering::Release);
    XFER_SLOT_TARGET[row][k].store(0, Ordering::Release);
    XFER_SLOT_RIGHTS[row][k].store(0, Ordering::Release);
    XFER_SLOT_REC[row][k].store(0, Ordering::Release);
    XFER_SLOT_DERIV[row][k].store(0, Ordering::Release); // U8x sidecars clear with the slot
    XFER_SLOT_GEN[row][k].store(0, Ordering::Release);
    XFER_SLOT_TX[row][k].store(0, Ordering::Release);
}

/// True iff `row`'s inbox holds no live or in-flight slot — the teardown/leak verifier.
fn xfer_row_is_clear(row: usize) -> bool {
    debug_assert!(row < XFER_SLOT_TX.len(), "xfer_row_is_clear: row out of range");
    (0..NXFER).all(|k| XFER_SLOT_TX[row][k].load(Ordering::Acquire) == 0)
}

/// Teardown-clear a slot's transfer inbox (called from `clear_handle_row`): claim each live slot by
/// tx-exact CAS (so a racing consumer/retractor never double-frees), free its record, release the slot. A
/// slot mid-claim (`RESERVING`) belongs to a sender between its CAS and its live-store; that sender's own
/// post-check retracts it (it re-reads the recipient's Proc state, which is no longer RUNNING by the time
/// this teardown runs) — one pass here is sufficient, not a spin.
fn clear_xfer_inbox_row(row: usize) {
    for k in 0..NXFER {
        let tx = XFER_SLOT_TX[row][k].load(Ordering::Acquire);
        if tx == 0 || tx == HANDLE_RESERVING {
            continue;
        }
        if XFER_SLOT_TX[row][k]
            .compare_exchange(tx, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let rec = XFER_SLOT_REC[row][k].load(Ordering::Acquire);
            if rec != 0 {
                xfer_rec_free((rec - 1) as usize);
            }
            // U8x: a swept (never-delivered) deposit drops its derivation node with the slot.
            let dn = XFER_SLOT_DERIV[row][k].load(Ordering::Acquire);
            if dn != 0 {
                deriv_drop(dn);
            }
            xfer_slot_release(row, k);
        }
    }
}

/// SYS_XFER(dest, src, req_rights) -> a transfer id (>= 1), or a negative errno. Deposit an ATTENUATED
/// copy of a capability the caller holds into the recipient's inbox — the cross-process delegation
/// primitive (a shell handing an editor a console cap), owner-scoped: `dest` must be a `Child` handle in
/// the CALLER's own table.
///
/// Flow: resolve `dest` (must be `Child`; `-ECHILD` otherwise) -> resolve `src` in the caller's own table
/// (must carry `CAP_GRANT`, the delegation right — the same authority `sys_cap_grant` demands; `-EACCES`)
/// -> enforce ATTENUATION (`req & !src_rights != 0` => `-EACCES` — the monotonic-decrease invariant,
/// cross-process now) -> map the child pid to its SLOT through its Proc entry (must be RUNNING;
/// `-ECHILD`) -> claim a record, then CAS-claim an inbox slot (each full => `-EAGAIN`, the record
/// unwound) -> publish the descriptor, tx LAST -> POST-CHECK the recipient.
///
/// The post-check closes the deposit-vs-exit race from the sender's side: if the recipient exited between
/// the RUNNING check and the deposit going live, its teardown may have already swept the inbox — so the
/// sender re-reads the Proc entry and, on any change, RETRACTS its own deposit (tx-exact CAS; the loser
/// of the race does nothing — the winner freed the record) and returns `-ECHILD`. Residual (documented,
/// accepted this arc, same as pi4): if the recipient exited, its slot was recycled, AND the new tenant
/// consumed the deposit — all between the live-store and this post-check — the cap lands with the new
/// tenant. Closing that needs generation-tagged inboxes, which ride the revocation-tree arc.
///
/// Payload kinds: `Console`/`Socket` only. `File` is refused (`-EACCES`): its file-id indexes the
/// SENDER's per-row FILES table — a cross-row File transfer needs descriptor migration (deferred).
/// `Child` is refused (`-EACCES`): delegating reap rights is a process-model arc, not a transfer one.
/// A SHARED_ROW caller is refused (`-EACCES` — the x86 divergence; see the section note).
fn sys_xfer(dest: u64, src: u64, req_rights: u64) -> i64 {
    // The caller must own a PRIVATE row: the shared kernel window is not a transfer endpoint.
    let Some(row) = crate::arch::memory::current_slot() else {
        return EACCES;
    };
    sys_xfer_from(row, dest, src, req_rights)
}

/// The `sys_xfer` body, parameterized on the sending ROW — the dispatcher passes `current_slot()`; the U8x
/// kernel-side check drives the SAME code path with scratch rows (no ring-3 detour, no duplicate logic).
/// The SHARED_ROW refusal lives in the `sys_xfer` wrapper above (a ring-3 caller invariant), so this body
/// never needs it; the kernel check only ever passes private scratch rows.
fn sys_xfer_from(row: usize, dest: u64, src: u64, req_rights: u64) -> i64 {
    // 1. The recipient: a Child handle in the CALLER's OWN table (structural, owner-scoped delegation).
    let pid = match handle_resolve(row, dest, 0) {
        Ok(HandleTarget::Child(pid)) => pid,
        _ => return ECHILD,
    };
    // 2. The source capability: present, carrying CAP_GRANT (the delegation right), of a transferable kind.
    let Some(target) = handle_get(row, src as usize) else {
        return EACCES;
    };
    if target == HANDLE_RESERVING {
        return EACCES; // an in-flight reservation is not a transferable handle (defensive; single-writer)
    }
    let src_kind = handle_kind(row, src as usize);
    if src_kind != KIND_CONSOLE && src_kind != KIND_SOCKET {
        return EACCES; // File needs descriptor migration; Child would delegate reaping — both refused
    }
    let src_rights = HANDLE_RIGHTS[row][src as usize].load(Ordering::Acquire);
    if src_rights & CAP_GRANT == 0 {
        return EACCES; // the source does not authorize delegation
    }
    // U7x: a revoked RECEIVED cap must not be re-TRANSFERRED onward either (the sys_cap_grant laundering
    // check's transfer twin) — post-revoke delegation is refused; pre-revoke copies are the tree arc.
    let src_rec = HANDLE_XFER_REC[row][src as usize].load(Ordering::Acquire);
    if src_rec != 0
        && XFER_REC_TX[(src_rec - 1) as usize].load(Ordering::Acquire) & XFER_REVOKED_BIT != 0
    {
        return EACCES;
    }
    // U8x: a source on a revoked derivation chain must not be re-transferred either (the tree-deep twin of
    // the record check above — post-revoke delegation is refused however deep the chain).
    let src_dn = HANDLE_DERIV[row][src as usize].load(Ordering::Acquire);
    if src_dn != 0 && deriv_stale(src_dn) {
        return EACCES;
    }
    // 3. Attenuation across processes: any requested bit the sender does not hold is an amplification.
    let req = req_rights as u32;
    if req & !src_rights != 0 {
        return EACCES;
    }
    // 4. pid -> the recipient's row (the inbox key), via the Proc table; it must be RUNNING now (and is
    //    re-checked after the deposit — see the post-check below). The +1 bias undone; 0 = no slot
    //    recorded (a shared-window task is not a transfer recipient).
    let Some(pi) = proc_find_child(pid) else {
        return ECHILD;
    };
    if PROCS[pi].state.load(Ordering::Acquire) != PRUNNING {
        return ECHILD;
    }
    let Some(dst_row) = PROCS[pi].slot.load(Ordering::Acquire).checked_sub(1) else {
        return ECHILD;
    };
    if dst_row >= crate::arch::memory::USER_SLOTS {
        return ECHILD; // never a private row (defensive; sys_spawn only records private slots)
    }
    // U8x: snapshot the recipient's inbox GENERATION before depositing — the deposit is stamped with it,
    // RECV verifies it, and the post-check re-reads it (a change = the recipient tore down = retract).
    let dst_gen = SLOT_GEN[dst_row].load(Ordering::Acquire);
    // 5. Record the derivation edge (delivered cap -> source), then claim the revoke ledger entry, then
    //    the inbox slot; any exhaustion unwinds cleanly (-EAGAIN).
    let Some((node, node_id)) = deriv_derive_from(row, src as usize) else {
        return EAGAIN;
    };
    let Some((rec, tx)) = xfer_rec_claim(row as u64) else {
        deriv_drop(node);
        return EAGAIN;
    };
    // The record remembers the transfer's node (+ its id, the ABA guard) so XREVOKE kills the subtree.
    XFER_REC_DERIV[rec].store(node, Ordering::Release);
    XFER_REC_DERIV_ID[rec].store(node_id, Ordering::Release);
    let Some(slot) = (0..NXFER).find(|&k| {
        XFER_SLOT_TX[dst_row][k]
            .compare_exchange(0, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }) else {
        xfer_rec_free(rec);
        deriv_drop(node);
        return EAGAIN; // the recipient's inbox is full
    };
    // 6. Publish the descriptor (Release), the tx LAST — a recipient that observes the live tx observes
    //    the whole descriptor (the handle publish-order discipline, applied to the one cross-row surface).
    XFER_SLOT_KIND[dst_row][slot].store(src_kind, Ordering::Release);
    XFER_SLOT_TARGET[dst_row][slot].store(target, Ordering::Release);
    XFER_SLOT_RIGHTS[dst_row][slot].store(req, Ordering::Release);
    XFER_SLOT_REC[dst_row][slot].store((rec + 1) as u32, Ordering::Release);
    XFER_SLOT_DERIV[dst_row][slot].store(node, Ordering::Release);
    XFER_SLOT_GEN[dst_row][slot].store(dst_gen, Ordering::Release);
    XFER_SLOT_TX[dst_row][slot].store(tx, Ordering::Release);
    // 7. POST-CHECK + retract (see the doc comment). Same entry, same pid, still RUNNING, SAME inbox
    //    generation — or undo ours. The generation re-read (U8x) closes the residual TOCTOU U7x documented:
    //    even if the entry looks unchanged, a teardown-and-recycle inside the window bumped the generation,
    //    and a deposit stamped with the OLD one is retracted here (or discarded at RECV — both sides hold).
    if PROCS[pi].state.load(Ordering::Acquire) != PRUNNING
        || PROCS[pi].pid.load(Ordering::Acquire) != pid
        || SLOT_GEN[dst_row].load(Ordering::Acquire) != dst_gen
    {
        if XFER_SLOT_TX[dst_row][slot]
            .compare_exchange(tx, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            xfer_slot_release(dst_row, slot);
            xfer_rec_free(rec);
            deriv_drop(node);
        }
        // CAS failure = the recipient's teardown (or a last-instant consume) won the slot and freed (or
        // took ownership of) the record + node — either way they are no longer ours to unwind.
        return ECHILD;
    }
    tx as i64
}

/// SYS_RECV() -> a handle index, or `-EAGAIN` when nothing is pending (the caller retries) — also
/// `-EAGAIN` when the caller's handle table is full (the pending transfer stays queued). The recipient's
/// half of the transfer: scan the CALLER's OWN inbox, claim the first live slot (tx-exact CAS), and
/// install the descriptor into the CALLER's OWN handle row — the single-writer invariant is preserved
/// because the only row written is the caller's, by the caller, mid-syscall. A SHARED_ROW caller is
/// refused (`-EACCES` — the x86 divergence; the shared window is not a transfer endpoint).
///
/// A transfer revoked while still PENDING is discarded here (record freed, slot released, scan continues)
/// — it is never delivered. A delivered cap records its transfer (the `HANDLE_XFER_REC` sidecar, stored
/// BEFORE the live value) so a later revoke reaches it at `handle_resolve`.
fn sys_recv() -> i64 {
    let Some(row) = crate::arch::memory::current_slot() else {
        return EACCES;
    };
    sys_recv_for(row)
}

/// The `sys_recv` body, parameterized on the receiving ROW — the dispatcher passes `current_slot()`; the
/// U8x kernel-side check drives the SAME code path with scratch rows (no ring-3 detour, no duplicate
/// logic). The SHARED_ROW refusal lives in the `sys_recv` wrapper above (a ring-3 caller invariant).
fn sys_recv_for(row: usize) -> i64 {
    if row >= XFER_SLOT_TX.len() {
        return EAGAIN; // defensive; a real ring-3 caller always has an in-range row
    }
    for k in 0..NXFER {
        let tx = XFER_SLOT_TX[row][k].load(Ordering::Acquire);
        if tx == 0 || tx == HANDLE_RESERVING {
            continue;
        }
        // Claim-to-consume (tx-exact): losing means a racing retract/teardown owns the slot — move on.
        if XFER_SLOT_TX[row][k]
            .compare_exchange(tx, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        let kind = XFER_SLOT_KIND[row][k].load(Ordering::Acquire);
        let target = XFER_SLOT_TARGET[row][k].load(Ordering::Acquire);
        let rights = XFER_SLOT_RIGHTS[row][k].load(Ordering::Acquire);
        let rec = XFER_SLOT_REC[row][k].load(Ordering::Acquire);
        let node = XFER_SLOT_DERIV[row][k].load(Ordering::Acquire);
        let dep_gen = XFER_SLOT_GEN[row][k].load(Ordering::Acquire);
        // Revoked while pending, a recordless slot (a kernel bug, failed closed), or — U8x — a deposit
        // stamped for a PREVIOUS tenant of this row (its generation predates the current one: the sender
        // aimed at a process that tore down; the recycled slot's new tenant must never consume it):
        // discard, keep scanning.
        if rec == 0
            || XFER_REC_TX[(rec - 1) as usize].load(Ordering::Acquire) & XFER_REVOKED_BIT != 0
            || dep_gen != SLOT_GEN[row].load(Ordering::Acquire)
        {
            if rec != 0 {
                xfer_rec_free((rec - 1) as usize);
            }
            if node != 0 {
                deriv_drop(node); // the undelivered cap's node dies with the deposit
            }
            xfer_slot_release(row, k);
            continue;
        }
        // Install into the CALLER's OWN row: reserve first-free, publish kind + rights + the transfer
        // reference + the derivation node, then the live value LAST. A full table re-queues the transfer
        // (restore the tx — we own the slot, so a plain Release store is safe) and returns -EAGAIN.
        let Some(h) = handle_install(row, HANDLE_RESERVING) else {
            XFER_SLOT_TX[row][k].store(tx, Ordering::Release);
            return EAGAIN;
        };
        handle_set_kind(row, h, kind);
        handle_set_rights(row, h, rights);
        HANDLE_XFER_REC[row][h].store(rec, Ordering::Release); // the revocation hook, pre-live
        HANDLE_DERIV[row][h].store(node, Ordering::Release); // the derivation edge, pre-live (U8x)
        handle_set(row, h, target);
        xfer_slot_release(row, k); // consume the inbox slot (record ownership moved to the handle)
        return h as i64;
    }
    EAGAIN
}

/// SYS_CAP XREVOKE(transfer id): the SENDER invalidates a transfer it made. Single-level: the received
/// cap goes stale at its next `handle_resolve` (or the pending deposit is discarded at RECV) — but a cap
/// the recipient already re-granted/re-transferred onward is NOT cascaded (revocation TREES, deferred).
/// Sender-only: the record carries the transferring row; anyone else gets `-EACCES`. An unknown/already-
/// closed transfer id is `-ENOENT` (ids are globally unique, so a stale id can never alias a new one).
fn sys_cap_xrevoke(row: usize, txid: u64) -> i64 {
    if txid == 0 || txid == HANDLE_RESERVING || txid & XFER_REVOKED_BIT != 0 {
        return ENOENT; // never a live transfer id
    }
    for r in 0..MAX_XFERS {
        if XFER_REC_TX[r].load(Ordering::Acquire) == txid {
            if XFER_REC_SENDER[r].load(Ordering::Acquire) != row as u64 {
                return EACCES; // only the sender may revoke its transfer (disowned records match no row)
            }
            // TX-EXACT, like every other record/slot transition (the pi4 review-confirmed fix): the
            // revoked bit can only land while the record still ledgers THIS transfer. A lost CAS means the
            // record was freed (and possibly reclaimed for someone else's transfer) between the find and
            // the flip — the transfer is already closed, NOTHING is written to the record's current
            // tenant, and the caller honestly gets -ENOENT.
            // U8x: capture the transfer's derivation node (+ its publish-time id) BEFORE the CAS — a won
            // CAS proves the record still ledgered this transfer when read, so the pair is this transfer's.
            let dn = XFER_REC_DERIV[r].load(Ordering::Acquire);
            let dn_id = XFER_REC_DERIV_ID[r].load(Ordering::Acquire);
            return if XFER_REC_TX[r]
                .compare_exchange(txid, txid | XFER_REVOKED_BIT, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                // Mark the transfer's node revoked (id-guarded — a concurrently dropped-and-reclaimed node
                // is left alone; it had no children, so nothing escapes). This is what makes the revoke a
                // TREE: every re-grant/re-transfer below the delivered cap dies at next use.
                deriv_revoke_if(dn, dn_id);
                0
            } else {
                ENOENT
            };
        }
    }
    ENOENT
}

// =============================================================================================
// U8x: revocation TREES (the derivation ledger) + generation-tagged inboxes — closing U7x's two
// documented escapes. The aarch64 pi4 U8 twin, slot-keyed.
// =============================================================================================
//
// U7x left two honest gaps (its SECURITY.md entry): (1) revoke was SINGLE-LEVEL — a recipient who re-granted
// or re-transferred a received cap created a DERIVED copy a later revoke never reached; (2) the sys_xfer
// post-check had a residual TOCTOU — recipient-exit + slot-recycle + new-tenant-consume inside the
// deposit-live -> post-check window could deliver a transfer to the wrong tenant. U8x closes both.
//
// (1) THE DERIVATION LEDGER. Every mint that derives one capability from another records an edge
// child -> parent in a bounded static ledger of NODES (the U7x static-atomic-array discipline — no heap,
// state-exact CAS transitions, Release-publish / Acquire-read). `sys_cap_grant` (a local mint) and
// `sys_xfer`/`sys_recv` (a delivered transfer) both derive; handles installed by spawn/open/endow are ROOTS
// (no node until they first act as a grant/transfer source — the node is created LAZILY then). Revocation is
// MARK-ONE-NODE; staleness is discovered at USE: `handle_resolve` walks child -> root through the ledger
// (bounded — the ledger is bounded and a parent is always created before its child, so cycles are impossible
// by construction) and fails `Denied` if ANY ancestor is revoked. This keeps U7x's load-bearing invariant:
// **no revoke path ever writes another row** — the stale-at-use pattern, generalized. Revoke is O(1);
// resolve pays the bounded walk.
//
// NODE LIFETIME (the documented choice: tombstones until the subtree drains). A node frees when its owning
// handle DROPS **and** it has no live children; a dropped node with live children persists as a TOMBSTONE
// (still walkable, still carrying its revoked bit) until its last child frees — then the free cascades up
// through any drained tombstoned ancestors (`deriv_drop`). Freeing is arbitrated by a state-exact CAS on the
// node's ID word (two racing freers — the owner's drop vs a child's cascade — resolve to exactly one winner).
// Every walkable node is PINNED: a resolver only reaches a node through a live handle (whose own node cannot
// free) and live child edges (a live child holds `KIDS > 0` on its parent), so no walk ever reads a freed/
// reclaimed node. Ledger exhaustion is `-EAGAIN` at mint/transfer time (the U4x resource-bound discipline —
// claim last, unwind on failure, no leak on any path).
//
// (2) GENERATION-TAGGED INBOXES. A per-slot generation word is bumped at teardown (the same site as the U7x
// inbox sweep, BEFORE the sweep). A deposit stamps the recipient's current generation into its slot; RECV
// verifies the stamp against the CURRENT generation (a mismatch = the deposit was aimed at a PREVIOUS tenant
// — discarded, never delivered) and the sender's post-check re-reads the generation (a change = retract). A
// recycled slot's new tenant is therefore structurally unable to consume a stale deposit, from BOTH sides.

/// Derivation ledger capacity — bounded and static like `MAX_XFERS`. The demo peak is ~6 live nodes.
const MAX_DERIV: usize = 16;
/// Bit 63 of a node's ID word marks the node (and thereby its whole subtree, at resolve time) REVOKED. The
/// flag rides IN the state word so revocation is a state-exact CAS (the `XFER_REVOKED_BIT` discipline); node
/// ids are a monotonic counter from 1, so bit 63 never aliases a genuine id.
const DERIV_REVOKED_BIT: u64 = 1 << 63;

/// A node's STATE word: 0 = free, `HANDLE_RESERVING` = mid-claim, else = its unique node id (live), possibly
/// `| DERIV_REVOKED_BIT` (live, revoked). The id makes revoke/free ABA-safe (a reclaimed slot carries a NEW
/// id, so a stale revoke-by-expected-id can never hit the new tenant — see `deriv_revoke_if`).
static DERIV_ID: [AtomicU64; MAX_DERIV] = [const { AtomicU64::new(0) }; MAX_DERIV];
/// The node's parent edge: parent node index + 1, or 0 = a root. Set once at claim, cleared at free.
static DERIV_PARENT: [AtomicU32; MAX_DERIV] = [const { AtomicU32::new(0) }; MAX_DERIV];
/// Live children count — what pins a tombstoned parent until its subtree drains.
static DERIV_KIDS: [AtomicU32; MAX_DERIV] = [const { AtomicU32::new(0) }; MAX_DERIV];
/// The owning handle dropped (tombstone flag): the node frees when this is set AND `KIDS == 0`.
static DERIV_DROPPED: [AtomicBool; MAX_DERIV] = [const { AtomicBool::new(false) }; MAX_DERIV];
/// The next node id — monotonic from 1 (never 0/u64::MAX, the state sentinels).
static DERIV_NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Which derivation node (index + 1; 0 = a root with no node yet) a handle's capability is. Keyed
/// `[row][idx]` like every handle sidecar; written only by the row's own task (mid-syscall) or its teardown.
static HANDLE_DERIV: [[AtomicU32; NHANDLE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NHANDLE] }; crate::arch::memory::USER_SLOTS + 1];

/// The derivation node riding a PENDING deposit (index + 1) — ownership passes inbox-slot -> received handle
/// at RECV; every discard path (revoked-pending, generation-stale, retract, teardown sweep) drops it instead.
static XFER_SLOT_DERIV: [[AtomicU32; NXFER]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NXFER] }; crate::arch::memory::USER_SLOTS + 1];
/// The recipient GENERATION stamped into a pending deposit — RECV delivers only on an exact match with the
/// recipient's CURRENT generation (see `SLOT_GEN`).
static XFER_SLOT_GEN: [[AtomicU64; NXFER]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NXFER] }; crate::arch::memory::USER_SLOTS + 1];

/// The transfer record's derivation node (index + 1) + that node's ID at publish time — what
/// `sys_cap_xrevoke` marks so the revoke reaches everything DERIVED from the transferred cap (re-grants,
/// re-transfers), not just the directly received handle. The captured ID makes the mark ABA-safe: if the
/// node was dropped and its slot reclaimed by the time the revoke lands, the id no longer matches and
/// nothing is written (a dropped node had no children left to protect, so nothing escapes).
static XFER_REC_DERIV: [AtomicU32; MAX_XFERS] = [const { AtomicU32::new(0) }; MAX_XFERS];
static XFER_REC_DERIV_ID: [AtomicU64; MAX_XFERS] = [const { AtomicU64::new(0) }; MAX_XFERS];

/// Per-slot inbox GENERATION: bumped (AcqRel) at the TOP of `clear_handle_row` — i.e. strictly before the
/// teardown's inbox sweep — so any deposit stamped with the old generation is dead-on-arrival for the slot's
/// next tenant even if it lands after the sweep passed its slot. `SHARED_ROW` never tears down. Sized
/// `USER_SLOTS + 1` for index symmetry with the other row-keyed sidecars.
static SLOT_GEN: [AtomicU64; crate::arch::memory::USER_SLOTS + 1] =
    [const { AtomicU64::new(0) }; crate::arch::memory::USER_SLOTS + 1];

/// Claim a free derivation node under `parent_ref` (a node index + 1, or 0 for a root): CAS the ID word
/// 0 -> RESERVING, publish the edge + zeroed counters, bump the parent's KIDS (the parent is pinned — the
/// caller holds its owning handle live), then the fresh id LAST (live-last). Returns `(node index + 1, id)`,
/// or `None` when the ledger is exhausted (-> `-EAGAIN` at the caller, nothing to unwind).
fn deriv_claim(parent_ref: u32) -> Option<(u32, u64)> {
    debug_assert!(parent_ref as usize <= MAX_DERIV, "deriv_claim: bad parent ref");
    for n in 0..MAX_DERIV {
        if DERIV_ID[n]
            .compare_exchange(0, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            DERIV_PARENT[n].store(parent_ref, Ordering::Release);
            DERIV_KIDS[n].store(0, Ordering::Release);
            DERIV_DROPPED[n].store(false, Ordering::Release);
            if parent_ref != 0 {
                DERIV_KIDS[(parent_ref - 1) as usize].fetch_add(1, Ordering::AcqRel);
            }
            let id = DERIV_NEXT_ID.fetch_add(1, Ordering::AcqRel);
            DERIV_ID[n].store(id, Ordering::Release);
            return Some(((n + 1) as u32, id));
        }
    }
    None
}

/// Mark node `nref` (index + 1) REVOKED — state-exact on the CURRENT id, idempotent (an already-revoked id
/// CASes to itself). Caller must PIN the node (own its live handle, or hold its record's txid — see
/// `deriv_revoke_if` for the unpinned case). Descendants discover the mark at their next `handle_resolve`.
fn deriv_revoke(nref: u32) {
    debug_assert!(nref >= 1 && (nref as usize) <= MAX_DERIV, "deriv_revoke: bad ref");
    let n = (nref - 1) as usize;
    let cur = DERIV_ID[n].load(Ordering::Acquire);
    if cur == 0 || cur == HANDLE_RESERVING {
        return; // freed/mid-claim — nothing live to mark (pinned callers never see this)
    }
    let _ = DERIV_ID[n].compare_exchange(
        cur,
        cur | DERIV_REVOKED_BIT,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

/// Mark node `nref` REVOKED only while it still carries `expect_id` — the ABA-safe form for callers that do
/// NOT pin the node (`sys_cap_xrevoke`: the recipient may drop the received handle — freeing the node —
/// concurrently with the sender's revoke; a reclaimed slot carries a NEW id, so the mark can never hit an
/// unrelated node). A lost CAS means the node was freed (it had no live children — nothing escapes).
fn deriv_revoke_if(nref: u32, expect_id: u64) {
    if nref == 0 || (nref as usize) > MAX_DERIV {
        return;
    }
    let n = (nref - 1) as usize;
    let _ = DERIV_ID[n]
        .compare_exchange(expect_id, expect_id | DERIV_REVOKED_BIT, Ordering::AcqRel, Ordering::Acquire);
}

/// True iff node `nref` or ANY ancestor is revoked — the read-side walk `handle_resolve` pays. Bounded by
/// `MAX_DERIV` (cycles are impossible: a parent is claimed strictly before its child and edges never change);
/// every node on the path is pinned (see the section comment), so a freed/reclaimed node is never read.
fn deriv_stale(nref: u32) -> bool {
    let mut r = nref;
    for _ in 0..MAX_DERIV {
        if r == 0 {
            return false; // reached a root, nothing revoked on the path
        }
        let n = (r - 1) as usize;
        if n >= MAX_DERIV {
            return true; // corrupt reference — fail closed
        }
        let id = DERIV_ID[n].load(Ordering::Acquire);
        if id == 0 || id == HANDLE_RESERVING {
            return false; // structurally unreachable on a pinned walk; benign stop (defensive)
        }
        if id & DERIV_REVOKED_BIT != 0 {
            return true;
        }
        r = DERIV_PARENT[n].load(Ordering::Acquire);
    }
    true // walk budget exhausted — impossible by construction; fail closed
}

/// Drop node `nref` (its owning handle/pending-slot released it): tombstone it, and FREE it iff its subtree
/// has drained — then cascade the free up through any drained tombstoned ancestors. The free is arbitrated
/// by a state-exact CAS on the ID word (the owner's drop and a child's cascade can race; exactly one wins).
/// A revoked tombstone keeps its bit until the free, so late resolvers of surviving descendants still deny.
fn deriv_drop(nref: u32) {
    let mut r = nref;
    loop {
        if r == 0 || (r as usize) > MAX_DERIV {
            return;
        }
        let n = (r - 1) as usize;
        // SeqCst (not Release/Acquire) on the DROPPED×KIDS handshake below: the parent-drop side
        // (store DROPPED here, then load KIDS) and the child-drop side (fetch_sub KIDS, then load
        // DROPPED, further down) form a store-buffering / Dekker pair. With only Release/Acquire
        // (no StoreLoad fence) a concurrent parent-vs-child drop of the same chain could have BOTH
        // sides read stale — the parent sees KIDS != 0 and the child sees DROPPED == false — so
        // neither frees this node and it leaks as a permanent tombstone (fail-closed: ledger
        // exhaustion -> -EAGAIN). SeqCst puts the four handshake ops in one total order, which
        // forbids the double-stale outcome; the both-free case stays arbitrated by the DERIV_ID CAS
        // below. Keep the four SeqCst ops symmetric with the twin arch. (U8/U8x concurrency lens.)
        DERIV_DROPPED[n].store(true, Ordering::SeqCst);
        if DERIV_KIDS[n].load(Ordering::SeqCst) != 0 {
            return; // live children pin it — a TOMBSTONE until the subtree drains
        }
        let mut id = DERIV_ID[n].load(Ordering::Acquire);
        loop {
            if id == 0 || id == HANDLE_RESERVING {
                return; // already freed / mid-claim (a racing freer won)
            }
            match DERIV_ID[n].compare_exchange(
                id,
                HANDLE_RESERVING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                // A racing REVOKE (deriv_revoke_if from a sender's xrevoke on this unpinned
                // node) only flips DERIV_REVOKED_BIT and NEVER frees — we remain the sole
                // freer, so retry against the refreshed word; returning here would leak the
                // node (and every tombstoned ancestor it pins) permanently, exhausting the
                // ledger to -EAGAIN. A racing FREER leaves 0/RESERVING — caught above on the
                // reload. (aarch64 U8 review must-fix, carried into the twin.)
                Err(cur) => id = cur,
            }
        }
        let parent = DERIV_PARENT[n].load(Ordering::Acquire);
        DERIV_PARENT[n].store(0, Ordering::Release);
        DERIV_DROPPED[n].store(false, Ordering::Release);
        DERIV_ID[n].store(0, Ordering::Release); // freed LAST
        if parent == 0 {
            return;
        }
        // Un-child the parent; if we took its LAST kid and it is tombstoned, cascade the free up.
        if DERIV_KIDS[(parent - 1) as usize].fetch_sub(1, Ordering::SeqCst) != 1 {
            return;
        }
        // SeqCst: child side of the store-buffering handshake (see the DROPPED store above).
        if !DERIV_DROPPED[(parent - 1) as usize].load(Ordering::SeqCst) {
            return; // the parent's owning handle is still live — it frees on its own drop
        }
        r = parent;
    }
}

/// True iff the whole derivation ledger is free — the U8x leak verifier (every node's lifetime closed).
fn deriv_all_free() -> bool {
    (0..MAX_DERIV).all(|n| DERIV_ID[n].load(Ordering::Acquire) == 0)
}

/// Ensure the source handle at `[row][src]` has a derivation node (creating a lazy ROOT node if not), and
/// mint a CHILD node under it — the shared derive step of `sys_cap_grant` and `sys_xfer`. Returns the child
/// `(node ref, id)`, or `None` on ledger exhaustion (-> `-EAGAIN`; a root node created en route stays
/// attached to the source handle and frees with it — never a leak).
fn deriv_derive_from(row: usize, src: usize) -> Option<(u32, u64)> {
    let src_node = match HANDLE_DERIV[row][src].load(Ordering::Acquire) {
        0 => {
            let (n, _) = deriv_claim(0)?;
            HANDLE_DERIV[row][src].store(n, Ordering::Release);
            n
        }
        n => n,
    };
    deriv_claim(src_node)
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
    /// U7x: the child's address-space SLOT, +1-biased (0 = none) — the pid->slot map `sys_xfer` resolves a
    /// `Child` dest handle through (the transfer inbox is keyed by the RECIPIENT's slot; the x86 twin of
    /// pi4's `Proc.asid`, biased because slot 0 is a valid slot). Stored (Release) beside the pid by
    /// `sys_spawn` (and by the U7x launcher for its planted fixture entry); 0 while FREE.
    slot: AtomicUsize,
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
        slot: AtomicUsize::new(0),
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
            PROCS[i].slot.store(0, Ordering::Release);
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
    PROCS[i].slot.store(0, Ordering::Release); // U7x: drop the pid->slot map with the entry
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
/// `CAP_READ`/`CAP_EXEC`/`CAP_REVOKE` round out the model (U8x: revoking a handle carrying `CAP_REVOKE`
/// kills its derivation SUBTREE; a right-less revoke stays local — U5x's ownership semantics). Values are
/// stable across arches (aarch64 U5 twin).
const CAP_READ: u32 = 1 << 0; // 0x01
const CAP_WRITE: u32 = 1 << 1; // 0x02
const CAP_EXEC: u32 = 1 << 2; // 0x04
const CAP_GRANT: u32 = 1 << 3; // 0x08
const CAP_REVOKE: u32 = 1 << 4; // 0x10 (U8x: revoking a handle carrying this kills its derivation SUBTREE)
// The rights are the distinct low 5 bits — a well-formed bitmask (each a single, non-overlapping bit,
// which the attenuation check `req & !src` relies on). This const-assert verifies that and anchors every
// CAP_* as used, so the model bit not yet exercised in Rust this arc (CAP_EXEC — held by no fixture, so
// the attenuation negative bites) doesn't read as dead code.
const _: () = assert!(
    (CAP_READ | CAP_WRITE | CAP_EXEC | CAP_GRANT | CAP_REVOKE) == 0x1F,
    "capability rights must be the distinct low 5 bits"
);

/// The well-known target token stored in a handle's value word to mean "the serial console resource" (as
/// opposed to a child pid). Distinct from `0` (Empty), `HANDLE_RESERVING` (`u64::MAX`), and every real
/// pid (small, monotonic), so the value word alone discriminates Child(pid) from Console without
/// perturbing U4x's sentinel checks. One non-pid token (not a general object table) is the arc's scope.
const HANDLE_CONSOLE: u64 = u64::MAX - 1;

/// The RESERVED console handle index — the stdout convention (fd 1, like POSIX). Every ring-3 program
/// prints with `sys_write(fd=1, ..)`, so the console write-capability is endowed here
/// (`install_console_cap`). U6x: this index is now a RESERVED region the first-free allocator
/// (`handle_install`) SKIPS — so a process may hold a console cap here AND N auto-allocated child/object
/// caps with **zero index collision, for any interleaving of installs**. This closes the U5x design note:
/// there, `install_console_cap`'s unconditional store to a fixed index could clobber a child that
/// `handle_install`'s first-free scan had already placed at index 1 (harmless only because no process both
/// printed and spawned). Reserving the index — rather than allocating the console through the shared
/// allocator — keeps the `fd=1` stdout ABI byte-identical for every existing blob (index 0 stays a general
/// slot; children/objects fill {0, 2, 3, ..}). See `handle_install`.
const CONSOLE_FD: usize = 1;

/// The rights sidecar: keyed IDENTICALLY to `HANDLES` (`[row][idx]`), so the value word keeps U4x's exact
/// `0`/`RESERVING` sentinel semantics and the rights ride alongside. Written Release beside the value
/// store (rights published BEFORE the value that makes a handle live, so a resolver that observes the
/// value also observes the rights), cleared in `handle_clear`/`clear_handle_row`. `0` == an inert handle.
static HANDLE_RIGHTS: [[AtomicU32; NHANDLE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NHANDLE] }; crate::arch::memory::USER_SLOTS + 1];

// ---------------------------------------------------------------------------------------------
// U6x — the general OBJECT descriptor: a handle is (kind, target, rights), first-free allocated for ALL
// kinds (the aarch64 pi4 U6a twin, keyed by SLOT/row instead of ASID).
// ---------------------------------------------------------------------------------------------
//
// U5x made a handle a capability but the descriptor was FIXED-SHAPE: two kinds only, discriminated by a
// magic value word (`HANDLE_CONSOLE` vs a pid), with the console pinned at a fixed index. U6x turns it into
// a general object descriptor without disturbing the lock-free allocator: the KIND rides in a PARALLEL
// sidecar (`HANDLE_KIND`, keyed identically to `HANDLES`/`HANDLE_RIGHTS`), exactly like the rights.
//
// Why a sidecar (not the value word's high bits): the value word's ONLY reserved values stay `0` (Empty —
// the allocator's free marker) and `u64::MAX` (RESERVING — an in-flight claim), byte-identical to U4x/U5x.
// The kind lives elsewhere, so a `File(id)`/`Socket(id)` may carry an ARBITRARY id in the value word with
// no high-bit masking and no risk of a real `(kind, id)` aliasing Empty/RESERVING — the STOP tripwire the
// brief names is structurally impossible here (the only ids to avoid remain `0` and `u64::MAX`, the same two
// a real pid already avoids; the demo's scaffold ids are small). It mirrors the existing `HANDLE_RIGHTS`
// sidecar 1:1 — same shape, same publish-before-the-live-value / observe-after discipline.
const KIND_EMPTY: u8 = 0; // no object — matches the value word's `0`=Empty (a cleared/free slot)
const KIND_CHILD: u8 = 1; // a child process (value word = its pid) — U4x's meaning
const KIND_CONSOLE: u8 = 2; // the serial console (value word = the `HANDLE_CONSOLE` token) — U5x's meaning
const KIND_FILE: u8 = 3; // U6x scaffold: a file object (value word = an opaque id); no fs syscall routes here yet
const KIND_SOCKET: u8 = 4; // U6x scaffold: a socket object (value word = an opaque id); no net syscall routes yet

/// The KIND sidecar: keyed IDENTICALLY to `HANDLES`/`HANDLE_RIGHTS` (`[row][idx]`). Discriminates what a
/// live handle NAMES (`KIND_*`), so the value word carries only the target payload (a pid / the console
/// token / an object id) and keeps U4x/U5x's `0`=Empty / `u64::MAX`=RESERVING sentinels intact. Written
/// with Release BEFORE the value store that makes a handle live (so a resolver observing the live value
/// also observes the kind), cleared in `handle_clear`/`clear_handle_row`. `KIND_EMPTY` (0) == an
/// inert/absent slot (the const-init).
static HANDLE_KIND: [[AtomicU8; NHANDLE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU8::new(KIND_EMPTY) }; NHANDLE] }; crate::arch::memory::USER_SLOTS + 1];

/// What a resolved handle NAMES — the general object descriptor's kind + payload. `Child(pid)` (U4x) and
/// `Console` (U5x) are the live kinds every consumer routes through; `File(id)`/`Socket(id)` are U6x
/// SCAFFOLDS — defined and resolvable so the table is provably general, though no fs/net syscall routes
/// through them yet (adding those is U7+/out of scope). The payload is the handle's value word (a pid, or an
/// opaque object id; `Console` carries none).
#[derive(Clone, Copy)]
enum HandleTarget {
    Child(u64),
    Console,
    File(u64),
    Socket(u64),
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

/// The enforcement CHECK, at the SINGLE lookup point every handle-consuming path goes through. Resolve
/// `idx` against the caller's own (`row`) table, then require the handle carry every bit in `req`. Returns
/// the general target on success. `NoHandle` for out-of-range/Empty/`RESERVING` (a reserving placeholder is
/// never a usable handle); `Denied` when a present handle lacks a required right. The value word is loaded
/// Acquire (synchronizing with the Release store that installed it), then the rights, then the KIND — so a
/// resolver that sees a live value also sees its rights and kind (they are stored BEFORE the value goes
/// live). U6x: the kind comes from the `HANDLE_KIND` sidecar (not a magic value word), so the payload is
/// dispatched by kind — `Child`/`File`/`Socket` carry the value word as pid/id; `Console` ignores it. A
/// live value with `KIND_EMPTY` is a kernel bug (kind is always published before the value) — treated
/// defensively as `NoHandle`.
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
    // U7x: a RECEIVED (transferred) capability goes STALE the moment its sender revokes the transfer. The
    // revocation state lives in the sender-owned transfer RECORD — never in the recipient's row, which only
    // the recipient writes (single-writer preserved); this read-side check is how the sender's revoke reaches
    // the recipient's next use. One extra Acquire load; `0` (not a transferred cap) for every other handle.
    let rec = HANDLE_XFER_REC[row][idx as usize].load(Ordering::Acquire);
    if rec != 0 && XFER_REC_TX[(rec - 1) as usize].load(Ordering::Acquire) & XFER_REVOKED_BIT != 0 {
        return Err(ResolveErr::Denied);
    }
    // U8x: a DERIVED capability is stale if ANY ancestor node in the derivation ledger is revoked — the
    // bounded child->root walk that makes revocation a TREE (a re-grant or re-transfer chain dies whole
    // when any node above it is marked). Roots (no node) skip the walk entirely; no revoke path ever
    // wrote this row — staleness is discovered here, at use (the U7x record pattern, generalized).
    let dn = HANDLE_DERIV[row][idx as usize].load(Ordering::Acquire);
    if dn != 0 && deriv_stale(dn) {
        return Err(ResolveErr::Denied);
    }
    match HANDLE_KIND[row][idx as usize].load(Ordering::Acquire) {
        KIND_CHILD => Ok(HandleTarget::Child(raw)),
        KIND_CONSOLE => Ok(HandleTarget::Console),
        KIND_FILE => Ok(HandleTarget::File(raw)),
        KIND_SOCKET => Ok(HandleTarget::Socket(raw)),
        _ => Err(ResolveErr::NoHandle), // KIND_EMPTY / unknown on a live value — a kernel bug; fail closed
    }
}

/// Set the rights word at `HANDLES[row][idx]` (Release) — used beside a value store to attach rights to a
/// freshly-installed handle (a child handle in `sys_spawn`, a minted handle in `sys_cap_grant`).
fn handle_set_rights(row: usize, idx: usize, rights: u32) {
    debug_assert!(row < HANDLES.len() && idx < NHANDLE, "handle_set_rights: out of range");
    HANDLE_RIGHTS[row][idx].store(rights, Ordering::Release);
}

/// Set the KIND byte at `HANDLE_KIND[row][idx]` (Release) — the U6x twin of `handle_set_rights`, stored
/// beside the value/rights when a handle is installed (a child in `sys_spawn` = `KIND_CHILD`; a mint in
/// `sys_cap_grant` = the source's kind). Published BEFORE the value goes live (see `handle_resolve`).
fn handle_set_kind(row: usize, idx: usize, kind: u8) {
    debug_assert!(row < HANDLES.len() && idx < NHANDLE, "handle_set_kind: out of range");
    HANDLE_KIND[row][idx].store(kind, Ordering::Release);
}

/// The KIND byte at `HANDLE_KIND[row][idx]` (Acquire) — read alongside `handle_get`'s value when a caller
/// needs the raw descriptor (e.g. `sys_cap_grant`, whose mint must copy the source handle's kind).
/// `KIND_EMPTY` for an out-of-range/absent slot.
fn handle_kind(row: usize, idx: usize) -> u8 {
    if idx >= NHANDLE {
        return KIND_EMPTY;
    }
    debug_assert!(row < HANDLES.len(), "handle_kind: row out of range");
    HANDLE_KIND[row][idx].load(Ordering::Acquire)
}

/// Install a capability at a FIXED index (not `handle_install`'s first-free scan): store the KIND and
/// rights FIRST (Release), then the target value (Release, LAST) — so a resolver that observes the live
/// value also observes the kind + rights. Used to endow the console cap at `CONSOLE_FD` and to plant the
/// U5x/U6x demo fixtures (console / File / Socket). Always called BEFORE the target process is dispatched
/// (setup / pre-spawn), so there is no concurrent resolver; the ordering is defensive belt-and-braces.
fn install_cap(row: usize, idx: usize, kind: u8, target: u64, rights: u32) {
    debug_assert!(row < HANDLES.len() && idx < NHANDLE, "install_cap: out of range");
    HANDLE_KIND[row][idx].store(kind, Ordering::Release);
    HANDLE_RIGHTS[row][idx].store(rights, Ordering::Release);
    HANDLES[row][idx].store(target, Ordering::Release);
}

/// Endow the process running in `row` with a console WRITE-capability at the RESERVED `CONSOLE_FD` — the
/// bootstrap that lets a ring-3 program print once `sys_write` routes through the table. Given to every
/// printing process: the shared window (`SHARED_ROW`) in `setup`, each spawned child in `sys_spawn`, and
/// the U6x printing spawner in its launcher. A process NOT so endowed gets `-EACCES` from `sys_write` (the
/// U5x negative). U6x: because `handle_install` SKIPS `CONSOLE_FD`, this store can never clobber (nor be
/// clobbered by) an auto-allocated child/object handle, for any ordering of installs.
fn install_console_cap(row: usize) {
    install_cap(row, CONSOLE_FD, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE);
}

/// True iff the entire `HANDLES[row]` row (values, rights AND kinds) is clear — the teardown-clear
/// verifier. Read by `u5x_launcher` after the fixture exits and its slot is retired:
/// `free_user_space_by_cr3` clears the row on exit, so this transitions false -> true, proving no stale
/// capability outlives its owning slot.
fn handle_row_is_clear(row: usize) -> bool {
    debug_assert!(row < HANDLES.len(), "handle_row_is_clear: row out of range");
    (0..NHANDLE).all(|i| {
        HANDLES[row][i].load(Ordering::Acquire) == 0
            && HANDLE_RIGHTS[row][i].load(Ordering::Acquire) == 0
            && HANDLE_KIND[row][i].load(Ordering::Acquire) == KIND_EMPTY
            // U7x: the transfer-reference sidecar is part of "clear" too — the single-writer snapshot and
            // the teardown proof must not miss a stale record reference behind an otherwise-empty slot.
            && HANDLE_XFER_REC[row][i].load(Ordering::Acquire) == 0
            // U8x: likewise the derivation sidecar — a stale node reference is part of "not clear".
            && HANDLE_DERIV[row][i].load(Ordering::Acquire) == 0
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
    // U8x: bump this slot's inbox GENERATION first — strictly BEFORE the inbox sweep below — so every
    // deposit stamped for the dying tenant is dead-on-arrival for the slot's next tenant even if it lands
    // after the sweep passed its slot (RECV verifies the stamp; the sender's post-check re-reads this word).
    // This closes the U7x-documented sys_xfer TOCTOU (exit + recycle + consume inside the deposit window).
    SLOT_GEN[slot].fetch_add(1, Ordering::AcqRel);
    for i in 0..NHANDLE {
        // Clear the value first (Empty => `handle_resolve` bails as NoHandle before reading rights/kind),
        // then the rights and kind — so no intermediate state is ever a live handle with stale rights/kind.
        HANDLES[slot][i].store(0, Ordering::Release);
        HANDLE_RIGHTS[slot][i].store(0, Ordering::Release);
        HANDLE_KIND[slot][i].store(KIND_EMPTY, Ordering::Release);
        // U7x: a received cap's transfer record is released with its handle (the handle_clear twin), so a
        // torn-down recipient leaks no record and a reused slot inherits no stale transfer reference.
        let rec = HANDLE_XFER_REC[slot][i].swap(0, Ordering::AcqRel);
        if rec != 0 {
            xfer_rec_free((rec - 1) as usize);
        }
        // U8x: the teardown is a drop of every handle the dying task held — its derivation nodes free (or
        // tombstone until their subtrees drain), so a reused slot inherits no stale derivation reference.
        let dn = HANDLE_DERIV[slot][i].swap(0, Ordering::AcqRel);
        if dn != 0 {
            deriv_drop(dn);
        }
    }
    // U7x: wipe this slot's transfer INBOX alongside its handles — a pending (undelivered) transfer to a
    // dying process is discarded and its record freed, so a REUSED slot starts with an empty inbox (no
    // stale pending capability for the next tenant to receive). Claim-to-clear per slot (tx-exact CAS) so
    // a racing consumer or retractor never double-frees; a sender racing this teardown re-checks its
    // recipient AFTER depositing (the sys_xfer post-check) and retracts, closing the deposit-into-a-dead-
    // slot path from its side.
    clear_xfer_inbox_row(slot);
    // U7x: DISOWN any still-live transfer this dying slot sent (SENDER -> u64::MAX, never a real row):
    // revoke authority dies with the sender, so the slot's next tenant can neither revoke nor be blamed
    // for the old tenant's transfers (txids are monotonic and were returned to ring 3 — without this, a
    // recycled sender slot could enumerate and kill its predecessor's live delegations). The CAS is
    // owner-exact: a record freed or reclaimed by another sender in the window simply fails the exchange.
    // The orphaned transfer stays live for its recipient — irrevocable until the revocation-tree arc
    // re-homes derivations.
    for r in 0..MAX_XFERS {
        let _ =
            XFER_REC_SENDER[r].compare_exchange(slot as u64, u64::MAX, Ordering::AcqRel, Ordering::Acquire);
    }
    // U6bx: the slot's open-FILE row rides the same teardown (handles first, so no File handle can name a
    // descriptor this wipe has already freed) — covers both the exit and the fault-kill path, exactly like
    // the handles (the aarch64 `clear_handle_row` -> `clear_files_row` twin).
    clear_files_row(slot);
}

/// Claim the first Empty slot in `HANDLES[slot]`, storing `value` (CAS 0->value), and return its index —
/// the handle `sys_spawn` / `sys_cap_grant` return to ring 3. `None` if the table is full (-> -EAGAIN).
/// Callers claim with `HANDLE_RESERVING`, then publish the kind + rights + real value (value LAST — see
/// `sys_spawn` / `sys_cap_grant`); `value` may be any non-`0` word (the CAS treats only `0` as free).
/// `slot` is always in range (from `current_slot`, 0..USER_SLOTS; debug-asserted).
///
/// U6x — the collision fix: the scan SKIPS the reserved `CONSOLE_FD`. That index belongs to the console cap
/// by convention (`install_console_cap`, an unconditional fixed-index store); by never handing it out here,
/// an auto-allocated child/object handle can never land on it and be clobbered by a later console install,
/// nor clobber a console already there — for ANY interleaving of installs. So the general allocator hands
/// out {0, 2, 3, .., NHANDLE-1}; the console lives at `CONSOLE_FD`. This closes the one U5x design note (a
/// printing spawner colliding a child with the console).
fn handle_install(slot: usize, value: u64) -> Option<usize> {
    debug_assert!(slot < HANDLES.len(), "handle_install: slot out of range");
    for (i, h) in HANDLES[slot].iter().enumerate() {
        if i == CONSOLE_FD {
            continue; // reserved for the console cap — never auto-allocated (the U6x no-collision invariant)
        }
        if h.compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire).is_ok() {
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

/// Clear (0 = Empty) the handle at `HANDLES[slot][idx]` AND its rights + kind sidecars — consumed when its
/// child is reaped in `sys_wait`, revoked by `sys_cap_revoke`, or released when a failed `sys_spawn` unwinds
/// its reservation. Clears the value first (Empty => `handle_resolve` bails before reading rights/kind),
/// then the rights and kind, mirroring the aarch64 twin's `handle_clear`, so a dropped capability never
/// leaves a stale rights/kind word behind an Empty slot for a later re-install to inherit.
fn handle_clear(slot: usize, idx: usize) {
    debug_assert!(slot < HANDLES.len() && idx < NHANDLE, "handle_clear: out of range");
    HANDLES[slot][idx].store(0, Ordering::Release);
    HANDLE_RIGHTS[slot][idx].store(0, Ordering::Release);
    HANDLE_KIND[slot][idx].store(KIND_EMPTY, Ordering::Release);
    // U7x: if this was a RECEIVED (transferred) capability, dropping the handle ends the transfer — free
    // its record (the transfer's whole lifetime is: XFER claims the record, the received handle references
    // it, this clear releases it; a pending-discard or sender-retract frees it on the other paths). The
    // swap clears the sidecar so a re-installed handle at this index never inherits a stale record.
    let rec = HANDLE_XFER_REC[slot][idx].swap(0, Ordering::AcqRel);
    if rec != 0 {
        xfer_rec_free((rec - 1) as usize);
    }
    // U8x: dropping the handle drops its derivation node — freed if its subtree has drained, else a
    // tombstone until it does (see `deriv_drop`). Swap-clears the sidecar so a re-installed handle at this
    // index never inherits a stale node reference. NOTE the order: the record is freed FIRST (above), the
    // node dropped after — `sys_cap_xrevoke` relies on it (a won tx-exact CAS there implies the node it
    // captured was not yet dropped-and-reclaimed; the id guard covers the residue).
    let dn = HANDLE_DERIV[slot][idx].swap(0, Ordering::AcqRel);
    if dn != 0 {
        deriv_drop(dn);
    }
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
    // sys_wait resolves the handle to it. U5x/U6x: the parent's child handle is KIND_CHILD carrying
    // CAP_READ (the ownership token — `sys_wait` gates on kind==Child, not on the right). Kind + rights are
    // published Release BEFORE the pid (the live value word), so the handle is never observed live without
    // its kind/rights.
    // U7x: the child's slot (+1-biased) is published BEFORE the pid (the entry's live key) — a sys_xfer
    // that finds this entry by pid always observes the slot its inbox deposit is keyed by.
    PROCS[pi].slot.store(child_slot + 1, Ordering::Release);
    PROCS[pi].pid.store(pid, Ordering::Release);
    handle_set_kind(slot, h, KIND_CHILD);
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
        CAP_OP_XREVOKE => sys_cap_xrevoke(row, a1), // U7x: revoke a transfer this caller made (sender-only)
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
/// is a straightforward extension once cross-process handle-transfer lands (U7). U6x: the mint names the
/// SAME (kind, target) as the source — a grant attenuates RIGHTS, never re-kinds the object. So a granted
/// console cap stays a console cap; a granted File stays that File.
fn sys_cap_grant(row: usize, src_idx: u64, req_rights: u64) -> i64 {
    // Resolve the source's raw target + kind + rights (no right required to READ your own handle's descriptor).
    let Some(target) = handle_get(row, src_idx as usize) else {
        return EACCES; // no such source handle
    };
    if target == HANDLE_RESERVING {
        return EACCES; // an in-flight reservation is not a grantable handle (defensive; single-writer)
    }
    let src_kind = handle_kind(row, src_idx as usize);
    let src_rights = HANDLE_RIGHTS[row][src_idx as usize].load(Ordering::Acquire);
    if src_rights & CAP_GRANT == 0 {
        return EACCES; // the source does not authorize granting
    }
    // U7x: a revoked RECEIVED cap is stale for DELEGATION too, not only for use — without this check a
    // recipient holding CAP_GRANT on a transferred cap could mint a fresh, revocation-free copy AFTER the
    // sender revoked (post-revoke laundering, the pi4 review-confirmed hole). Copies minted BEFORE the
    // revoke remain the documented revocation-TREE scope (derivation records chase those).
    let src_rec = HANDLE_XFER_REC[row][src_idx as usize].load(Ordering::Acquire);
    if src_rec != 0
        && XFER_REC_TX[(src_rec - 1) as usize].load(Ordering::Acquire) & XFER_REVOKED_BIT != 0
    {
        return EACCES;
    }
    // U8x: a source whose derivation chain is revoked is stale for delegation too — a mint from a dead
    // subtree would be a fresh, revocation-free copy (the laundering check, now tree-deep).
    let src_dn = HANDLE_DERIV[row][src_idx as usize].load(Ordering::Acquire);
    if src_dn != 0 && deriv_stale(src_dn) {
        return EACCES;
    }
    let req = req_rights as u32;
    // Attenuation: reject any requested bit the granter does not itself hold. `req & !src_rights` is
    // exactly the set of amplifying bits; non-empty => the grant would exceed the granter's authority.
    if req & !src_rights != 0 {
        return EACCES;
    }
    // U8x: record the derivation EDGE (mint -> source) so a later revoke of the source (or any ancestor)
    // reaches this copy at its next use. Ledger exhaustion is -EAGAIN with nothing else claimed yet.
    let Some((new_node, _)) = deriv_derive_from(row, src_idx as usize) else {
        return EAGAIN;
    };
    // Mint: claim a first-free slot with `handle_install` (a RESERVING placeholder — the value word goes
    // live LAST), then publish the source's kind + the attenuated rights + the derivation node, then the
    // real target value. Single-writer over this table (the caller is mid-syscall, not concurrently
    // resolving), so the kind/rights-then-value order is the defensive belt-and-braces that keeps a live
    // value from ever being seen sans kind/rights.
    match handle_install(row, HANDLE_RESERVING) {
        Some(idx) => {
            handle_set_kind(row, idx, src_kind);
            handle_set_rights(row, idx, req);
            HANDLE_DERIV[row][idx].store(new_node, Ordering::Release);
            handle_set(row, idx, target); // publish the live value LAST (Release)
            idx as i64
        }
        None => {
            deriv_drop(new_node); // unwind the freshly-minted node (it has no children — frees now)
            EAGAIN // handle table full
        }
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
    // U7x (folding the ledgered U6bx note): revoking a FILE handle releases its open-file DESCRIPTOR too —
    // without this, the descriptor outlived every handle to it and repeat open->revoke loops exhausted the
    // FILES row to a permanent -EMFILE. The +1-biased file-id decodes to the descriptor index; a stale or
    // out-of-range id (a kernel bug) is simply skipped — the handle clear below still denies every use.
    // Honest scope: a GRANT-minted duplicate File handle shares the descriptor, so revoking either one
    // frees it and the survivor's reads fail CLOSED (-EACCES at the FILE_USED re-check) — per-descriptor
    // refcounts ride the revocation-tree arc.
    if handle_kind(row, idx as usize) == KIND_FILE {
        if let Some(fid) = handle_get(row, idx as usize)
            .filter(|&v| v != HANDLE_RESERVING)
            .and_then(|v| (v as usize).checked_sub(1))
        {
            if fid < NFILE && FILE_USED[row][fid].load(Ordering::Acquire) {
                files_free(row, fid);
            }
        }
    }
    // U8x: CAP_REVOKE gets its real semantics — revoking a handle that CARRIES the right marks its
    // derivation node revoked, killing the whole subtree derived from it (every re-grant, and every
    // re-transfer whose chain passes through it) at the descendants' next use. Without the right the revoke
    // keeps U5x's ownership semantics: the caller's own row entry drops, derived copies survive. The mark
    // precedes the clear (the clear's `deriv_drop` tombstones the node, preserving the bit for as long as
    // any descendant lives). A handle with no node has no descendants — nothing to mark.
    let rights = HANDLE_RIGHTS[row][idx as usize].load(Ordering::Acquire);
    let dn = HANDLE_DERIV[row][idx as usize].load(Ordering::Acquire);
    if rights & CAP_REVOKE != 0 && dn != 0 {
        deriv_revoke(dn);
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
/// Set once the U5x launcher has printed its verdict AND its slot has freed, so the U6x launcher orders its
/// lines AFTER U5x's and runs with a free slot (the aarch64 `U5_LAUNCH_DONE` twin; the x86 `U4X_LAUNCH_DONE`
/// idiom).
static U5X_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

// --- U6x demo accounting (the general object table; written by the exit/kill paths, read by the launcher's
// verdict). ---
/// The printing-spawner fixture's witness bitmask (its `sys_exit` status, routed by name). One bit per
/// proven behaviour: print-before-spawn OK (bit0), both child handles valid + off the reserved console index
/// + distinct (bit1), print-AFTER-spawn OK — the console cap survived two spawns (bit2), both children reaped
/// status 0 (bit3). The verdict PASSes iff it equals `U6X_WITNESS_ALL` AND the kernel-side check held.
static U6X_WITNESS: AtomicU32 = AtomicU32::new(0);
/// 1 once the printing-spawner reaches `sys_exit` (its witness is then valid). The launcher waits on this
/// before reading `U6X_WITNESS`.
static U6X_DONE: AtomicU32 = AtomicU32::new(0);
/// Incremented if the printing-spawner fixture is KILLED (a fault) — any kill is a verdict FAIL.
static U6X_KILLED: AtomicU32 = AtomicU32::new(0);
/// The kernel-side U6x check result (set by `u6x_launcher`): the general object table resolves the
/// `File`/`Socket` scaffold kinds (with the right rights, `Denied` without) AND the first-free allocator
/// skips the reserved `CONSOLE_FD` so an interleaved console-install + two child installs collide on no
/// index. Both must hold for the U6x PASS.
static U6X_KINDS_OK: AtomicBool = AtomicBool::new(false);
/// The full witness — all four object-table behaviours proven.
const U6X_WITNESS_ALL: u32 = 0xF;
/// Set once the U6x launcher has printed its verdict AND its slot has freed, so the U6bx launcher orders
/// its lines AFTER U6x's and runs with a free slot (the `U5X_LAUNCH_DONE` idiom; the aarch64
/// `U6_LAUNCH_DONE` twin).
static U6X_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

// --- U6bx demo accounting (real File handles; written by the exit/kill paths, read by the launcher's
// verdict). ---
/// The File-handle fixture's witness bitmask (its `sys_exit` status, routed by name). One bit per proven
/// behaviour: open OK (bit0), 16-byte read OK (bit1), the read bytes match the staged source (bit2), a
/// File handle WITHOUT `CAP_READ` -> `-EACCES` (bit3 — the rights arm), a non-File handle WITH `CAP_READ`
/// -> `-EACCES` (bit4 — the kind arm). The verdict PASSes iff it equals `U6BX_WITNESS_ALL` AND the FILES
/// row teardown-cleared.
static U6BX_WITNESS: AtomicU32 = AtomicU32::new(0);
/// 1 once the File-handle fixture reaches `sys_exit` (its witness is then valid). The launcher waits on
/// this before reading `U6BX_WITNESS`.
static U6BX_DONE: AtomicU32 = AtomicU32::new(0);
/// Incremented if the File-handle fixture is KILLED (a fault) — any kill is a verdict FAIL.
static U6BX_KILLED: AtomicU32 = AtomicU32::new(0);
/// The full witness — all five File-handle behaviours proven (the aarch64 `U6B_WITNESS_ALL` twin).
const U6BX_WITNESS_ALL: u32 = 0x1F;
/// The handle indices `u6bx_launcher` pre-endows for the fixture's two negatives, off the allocator's
/// first-free path (the fixture's own SYS_OPEN claims index 0; index 1 = the reserved `CONSOLE_FD`). The
/// `mov rdi, {2,3}` operands in the fixture blob MUST match these (the aarch64 U6B_*_IDX twin).
const U6BX_NOCAP_IDX: usize = 2;
const U6BX_SOCK_IDX: usize = 3;
/// Set once the U6bx launcher has printed its verdict AND its slot has freed, so the U7x launcher orders
/// its lines AFTER U6bx's and runs with free slots (the `U6X_LAUNCH_DONE` idiom; the aarch64
/// `U6B_LAUNCH_DONE` twin — U6bx was the last demo before U7x, so it previously released no gate).
static U6BX_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

// --- U7x demo accounting (cross-process transfer; written by the exit/kill paths, read by the launcher's
// verdict). ---
/// The parent's pre-endowed handle indices: the `Child` handle naming the recipient (`U7X_DEST_IDX`) and
/// the full Console cap it transfers from (`U7X_SRC_IDX`, `CAP_WRITE|CAP_GRANT` — CAP_GRANT is the
/// delegation right XFER requires on its source). The `mov rdi/rsi, {2,3}` operands in
/// `unaos_user_u7x_parent` MUST match (the aarch64 U7_*_IDX twin).
const U7X_DEST_IDX: usize = 2;
const U7X_SRC_IDX: usize = 3;
/// The full per-fixture witness — 4 bits each. Parent: over-rights XFER `-EACCES` (b0), XFER t1 ok (b1),
/// XREVOKE t1 ok (b2), XFER t2 ok (b3). Child: RECV t1 (b0), USED the transferred Console cap — a real
/// `sys_write` through it landed (b1), RECV t2 (b2), the revoked t1 cap now `-EACCES` (b3).
const U7X_WITNESS_ALL: u32 = 0xF;
/// Window offsets of the launcher-written GO word (the fixtures only poll it) and the child-written USED
/// word (its "first write through the transferred cap landed" cue — pi4 conveyed this via SYS_REPORT,
/// which x86 lacks; the child stores to its OWN RW page instead and the launcher reads it through the
/// slot's identity backing). The `add rbx, 0x3000` / `[rbx + 8]` in the blob MUST match.
const U7X_GO_OFF: usize = 0x3000;
const U7X_USED_OFF: usize = 0x3008;
/// The parent fixture's final witness bitmask (its `sys_exit` status, routed by name ahead of the Proc
/// short-circuit — see the SYS_EXIT arm).
static U7X_PARENT_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The child fixture's final witness bitmask (same routing).
static U7X_CHILD_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The U7x fixtures that reached their witness exit (want 2 — parent + child). Read by `u7x_launcher`'s
/// deadline-bounded wait.
static U7X_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U7x fixture — a real U7x bug (both are well-behaved). Off the U1b counter.
static U7X_KILLED: AtomicU32 = AtomicU32::new(0);

// --- U8x demo accounting (revocation trees; the single-process fixture's witness + the kernel-side
// cross-process check, read by the launcher's verdict). ---
/// The handle indices `u8x_launcher` pre-endows in the fixture's table (off index 0 — first-free-claimed by
/// the fixture's own grants — and off the reserved `CONSOLE_FD`): a full console cap WITH `CAP_REVOKE` (the
/// tree-revoke parent) and one WITHOUT it (the locality negative). The `mov rsi, {2,3}` operands in
/// `unaos_user_u8x_tree` MUST match (the aarch64 U8_*_IDX twin).
const U8X_SRC_IDX: usize = 2;
const U8X_SRC2_IDX: usize = 3;
/// The full witness bitmask the revocation-tree fixture reports (as its exit status): bit0 grant chain works
/// (grant -> re-grant -> write through the grandchild cap lands), bit1 revoking the PARENT (a handle carrying
/// `CAP_REVOKE`) returns 0 AND revoking it again returns exactly `-ECHILD`, bit2 BOTH descendant copies are
/// `-EACCES` at their next use (the subtree kill), bit3 a revoke WITHOUT `CAP_REVOKE` stays LOCAL (the derived
/// copy still writes) and its double-revoke errno too. `u8x_launcher` PASSes iff it equals `U8X_WITNESS_ALL`
/// AND the kernel-side re-transfer-cascade + generation checks passed.
const U8X_WITNESS_ALL: u32 = 0xF;
/// The U8x fixture's final witness bitmask (its `sys_exit` status, routed by name — the u5x idiom).
static U8X_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The U8x fixture (`u8x-tree`) reached its witness exit (want 1). Read by `u8x_launcher`'s bounded wait.
static U8X_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U8x fixture — a real bug (it is register-only and well-behaved). Off the U1b counter.
static U8X_KILLED: AtomicU32 = AtomicU32::new(0);

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
    install_cap(slot, 1, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    install_cap(slot, 2, KIND_CONSOLE, HANDLE_CONSOLE, CAP_READ);
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
        U5X_LAUNCH_DONE.store(true, Ordering::Release); // release the U6x gate (U6x also gates on storage)
        return;
    }

    // 3. Build + pre-endow the fixture slot and spawn it on the demo core.
    let Some(u5) = u5x_setup() else {
        serial_println!(":: U5x: no free address-space slot — capability demo skipped ::");
        U5X_LAUNCH_DONE.store(true, Ordering::Release); // release the U6x gate even on the skip path
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
    // Release the U6x gate: the U5x verdict has printed and (the fixture having exited) the U5x slot has
    // freed, so the U6x launcher may build + endow its fixture slot and order its lines after ours.
    U5X_LAUNCH_DONE.store(true, Ordering::Release);
}

/// U6x fixture run parameters: the printing-spawner's ring-3 entry VA (inside the shared window VA — only
/// the slot FRAME differs, via CR3), the initial user rsp, its slot CR3, and its slot INDEX (so the launcher
/// can run the kernel-side object-table checks against — and then endow — that exact row).
struct U6xDemo {
    spawn: u64,
    sp: u64,
    cr3: u64,
    slot: usize,
}

/// U6x setup: allocate + build ONE private slot, copy the U6x blob into its code page through the identity
/// alias (the slot's code page is RX-RO from the start — W^X by construction), and return the run params.
/// Does NOT endow the console cap — the launcher runs its kernel-side checks against the fresh (empty) row
/// FIRST, then endows the live console cap. Also (re-)reserves the PROCS semaphores' waiter capacity so the
/// two children the fixture spawns can be waited on without a park-side reallocation (idempotent — u4x_setup
/// reserves them too, but this keeps U6x self-contained). Emits the U6x setup line; `None` if slot
/// allocation fails. Called ONCE from `u6x_launcher`, after the U5x gate — so a slot is free and no task runs
/// under the fixture's slot yet (the checks/endowment cannot race a resolver). Register-only fixture (writes
/// no user stack), so one slot suffices.
fn u6x_setup() -> Option<U6xDemo> {
    // Reserve each Proc semaphore's waiter capacity before any child can block the parent on it (the
    // u4x_setup discipline; idempotent when u4x already ran, which it has by the U5x -> U6x gate chain).
    for p in &PROCS {
        p.done.init();
    }
    let slot = crate::arch::memory::alloc_user_space()?;
    let blob_start = &raw const unaos_user_u6x_blob_start as usize;
    let blob_end = &raw const unaos_user_u6x_blob_end as usize;
    let blob_len = blob_end - blob_start;
    assert!(blob_len as u64 <= PAGE_SIZE, "U6x blob does not fit in a code page");
    let spawn_off = (&raw const unaos_user_u6x_spawn as usize - blob_start) as u64;
    let backing = crate::arch::memory::slot_backing_ptr(slot);
    unsafe {
        // Scrub the whole window (residue), then copy the blob into the code page (page 0) through the
        // identity alias — never USER_BASE, so the code mapping stays read-only (W^X).
        core::ptr::write_bytes(backing, 0, (USER_WINDOW_PAGES * PAGE_SIZE) as usize);
        core::ptr::copy_nonoverlapping(blob_start as *const u8, backing, blob_len);
    }
    serial_println!(
        ":: U6x: general object table — (kind, target, rights) descriptors, first-free alloc skips the reserved console index ::"
    );
    Some(U6xDemo {
        spawn: USER_BASE + spawn_off,
        sp: USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16,
        cr3: crate::arch::memory::slot_cr3(slot),
        slot,
    })
}

/// U6x kernel-side check — run against the fixture's FRESH, empty row before it is endowed/dispatched (no
/// concurrent resolver): prove the two things the ring-3 fixture cannot observe itself.
///
///   (A) NO INDEX COLLISION for any interleaving: auto-allocate two handles (`handle_install`), THEN install
///       the console cap at the reserved `CONSOLE_FD`. Under U5x's allocator the second auto-handle would
///       land on index 1 and the console's unconditional store would clobber it; U6x's allocator skips the
///       reserved index, so both auto-handles keep their values AND the console lands intact — verified via
///       `handle_get`. This is the exact interleaving the U5x review flagged, exercised directly.
///   (B) The scaffold FILE/SOCKET kinds RESOLVE to their kind carrying the required right, and to `Denied`
///       (`-EACCES`-equivalent) without it — proving the table is genuinely general (not console-only). The
///       ids are small non-sentinel words (never `0` / `u64::MAX`), so the value word never aliases
///       Empty/RESERVING.
///
/// Returns true iff every check held. Leaves the row dirty; the caller `clear_handle_row`s it before endowing
/// the live console cap.
fn u6x_kernel_check(slot: usize) -> bool {
    // (A) no-collision stress — the exact interleaving the U5x review flagged (spawn-onto-1, then console-install).
    let (Some(a), Some(b)) = (handle_install(slot, 0xA1), handle_install(slot, 0xB2)) else {
        return false; // a fresh 8-slot row cannot be full; a None here is a kernel bug -> fail closed
    };
    install_console_cap(slot); // unconditional store at CONSOLE_FD — must neither clobber nor be clobbered by a/b
    let nocollide = a != CONSOLE_FD
        && b != CONSOLE_FD
        && a != b
        && handle_get(slot, a) == Some(0xA1)
        && handle_get(slot, b) == Some(0xB2)
        && handle_get(slot, CONSOLE_FD) == Some(HANDLE_CONSOLE);

    // (B) File/Socket scaffold kinds resolve to their kind with the right rights, Denied without.
    install_cap(slot, 3, KIND_FILE, 0x100, CAP_READ);
    install_cap(slot, 4, KIND_SOCKET, 0x200, CAP_READ | CAP_WRITE);
    let file_ok = matches!(handle_resolve(slot, 3, CAP_READ), Ok(HandleTarget::File(0x100)))
        && matches!(handle_resolve(slot, 3, CAP_WRITE), Err(ResolveErr::Denied));
    let sock_ok = matches!(handle_resolve(slot, 4, CAP_READ | CAP_WRITE), Ok(HandleTarget::Socket(0x200)))
        && matches!(handle_resolve(slot, 4, CAP_EXEC), Err(ResolveErr::Denied));

    nocollide && file_ok && sock_ok
}

/// U6x launcher + verdict (the `u5x_launcher` shape: one gated kernel task on a scheduled sibling core).
/// `demo_cpu` (the task arg) is the core the printing spawner runs on. Flow:
///   1. Wait (bounded, yielding) for `U5X_LAUNCH_DONE`, so the U6x lines land after the U5x verdict and the
///      U5x slot has freed.
///   2. Skip silently if no block device — the fixture's two children load `HELLO.BIN` off it (as U4x does).
///   3. `u6x_setup()` (build the fixture slot, print the setup line), run the KERNEL-SIDE object-table checks
///      against its fresh row (`u6x_kernel_check`), `clear_handle_row` the scratch, endow the live console
///      cap, and spawn `u6x-spawn` on `demo_cpu`. Its two `sys_spawn`s co-locate BOTH children on `demo_cpu`
///      (the U4x co-location invariant — each child stays queued-not-dispatched until the parent blocks in
///      its first `sys_wait`, so both pids are recorded first).
///   4. Verdict (folded): wait (bounded) for the fixture's exit (`U6X_DONE == 1`), then PASS iff its witness
///      == `U6X_WITNESS_ALL` (printed before AND after two spawns with no collision, both children reaped
///      clean) AND the kernel-side check held AND no U6x kill. Prints ONE PASS line, then releases the U6bx
///      gate (`U6X_LAUNCH_DONE`) so the File-handle demo orders after this one.
pub fn u6x_launcher(demo_cpu: usize) {
    // 1. Gate on the U5x launcher (its verdict printed + its slot freed), bounded + yielding.
    let wdeadline = crate::arch::ticks() + 10_000;
    while !U5X_LAUNCH_DONE.load(Ordering::Acquire) && crate::arch::ticks() < wdeadline {
        crate::arch::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No block device -> the children cannot be loaded; skip silently (mirrors U4x/U5x's control discipline).
    if crate::drivers::block::info().is_none() {
        U6X_LAUNCH_DONE.store(true, Ordering::Release); // release the U6bx gate (it also gates on storage)
        return;
    }

    // 3. Build the fixture slot, run the kernel-side checks against its fresh row, then endow + spawn it.
    let Some(u6) = u6x_setup() else {
        serial_println!(":: U6x: no free address-space slot — object-table demo skipped ::");
        U6X_LAUNCH_DONE.store(true, Ordering::Release); // release the U6bx gate even on the skip path
        return;
    };
    U6X_KINDS_OK.store(u6x_kernel_check(u6.slot), Ordering::Release);
    clear_handle_row(u6.slot); // wipe the scratch handles the check planted...
    install_console_cap(u6.slot); // ...then endow the LIVE console cap the fixture prints through.
    crate::arch::sched::spawn_user_in_space("u6x-spawn", u6.spawn, u6.sp, demo_cpu, u6.cr3);

    // 4. Folded verdict: wait (bounded, yielding) for the fixture's exit, then judge. Two children (two
    //    spawns) + the parent complete well under this budget under QEMU.
    let vdeadline = crate::arch::ticks() + 5000;
    while U6X_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U6X_WITNESS.load(Ordering::Acquire);
    let kinds_ok = U6X_KINDS_OK.load(Ordering::Acquire);
    let killed = U6X_KILLED.load(Ordering::Acquire);
    if witness == U6X_WITNESS_ALL && kinds_ok && killed == 0 {
        serial_println!(
            ":: U6x: x86 general object table — printing spawner + 2 children, no index collision, File/Socket kinds resolve -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U6x: x86 general object table FAIL — witness={:#x} kinds_ok={} killed={} done={} (want {:#x} / true / 0 / 1) ::",
            witness,
            kinds_ok,
            killed,
            U6X_DONE.load(Ordering::Acquire),
            U6X_WITNESS_ALL
        );
    }
    // Release the U6bx gate: the U6x verdict has printed and (the fixture having exited) the U6x slot has
    // freed, so the U6bx launcher may build + endow its fixture slot and order its lines after ours.
    U6X_LAUNCH_DONE.store(true, Ordering::Release);
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

/// U6x one-shot, fired from the main loop after `u5x_probe_once` (gated on storage like U4x/U5x). It spawns
/// the U6x launcher on a sibling AP; the launcher gates on `U5X_LAUNCH_DONE`, so ordering + the free-slot
/// precondition hold regardless of interleave. The printing spawner's two children run HELLO.BIN, so — like
/// U4x — it needs the program staged: `u4x_probe_once` stages it first (idempotent `HELLO_STAGED`), and this
/// re-stages defensively if needed. Without a FAT volume/HELLO.BIN the children cannot run, so the demo is
/// skipped cleanly (no false FAIL) — exactly as U4x skips.
pub fn u6x_probe_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // retry next loop iteration until storage enumerates (mirrors u4x/u5x_probe_once)
    }
    let online = crate::arch::smp::online_aps();
    let Some(&cpu) = online.first() else {
        DONE.store(true, Ordering::Relaxed);
        serial_println!(":: U6x: no application processor online — object-table demo skipped ::");
        return;
    };
    DONE.store(true, Ordering::Relaxed); // one-shot from here regardless of outcome

    // The spawner's 2 children run HELLO.BIN off FAT (via sys_spawn's pre-staged buffer). Ensure it is
    // staged; a missing volume/file means nothing to spawn, so skip cleanly rather than emit a false FAIL.
    if !HELLO_STAGED.load(Ordering::Acquire) && !stage_hello() {
        serial_println!(":: U6x: HELLO.BIN not available — object-table demo skipped ::");
        return;
    }

    let vcpu = online.get(1).copied().unwrap_or(cpu);
    crate::arch::sched::spawn("u6x-launch", u6x_launcher, cpu, vcpu, crate::arch::sched::PRIO_NORMAL);
}

/// U6bx fixture run parameters: the File-handle fixture's ring-3 entry VA (inside the shared window VA —
/// only the slot FRAME differs, via CR3), the initial user rsp, its slot CR3, and its slot INDEX (so the
/// launcher can pre-endow the fixture's table, plant the expected bytes through the slot's identity
/// backing, and — after exit — verify the FILES-row teardown-clear).
struct U6bxDemo {
    file: u64,
    sp: u64,
    cr3: u64,
    slot: usize,
}

/// U6bx setup: allocate + build ONE private slot, copy the U6bx blob into its code page through the
/// identity alias (the slot's code page is RX-RO from the start — W^X by construction), and return the
/// run params. Does NOT pre-endow — the launcher endows the two negative-test handles and plants the
/// expected bytes after this returns (before dispatch, no concurrent resolver). Emits the U6bx setup
/// line; `None` if slot allocation fails. Called ONCE from `u6bx_launcher`, after the U6x gate — so a
/// slot is free and no task runs under the fixture's slot yet. Register-only fixture (writes no user
/// stack; its writable targets are the data page and the kernel-planted page-2 prefix), one slot suffices.
fn u6bx_setup() -> Option<U6bxDemo> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let blob_start = &raw const unaos_user_u6bx_blob_start as usize;
    let blob_end = &raw const unaos_user_u6bx_blob_end as usize;
    let blob_len = blob_end - blob_start;
    assert!(blob_len as u64 <= PAGE_SIZE, "U6bx blob does not fit in a code page");
    let file_off = (&raw const unaos_user_u6bx_file as usize - blob_start) as u64;
    let backing = crate::arch::memory::slot_backing_ptr(slot);
    unsafe {
        // Scrub the whole window (residue), then copy the blob into the code page (page 0) through the
        // identity alias — never USER_BASE, so the code mapping stays read-only (W^X).
        core::ptr::write_bytes(backing, 0, (USER_WINDOW_PAGES * PAGE_SIZE) as usize);
        core::ptr::copy_nonoverlapping(blob_start as *const u8, backing, blob_len);
    }
    serial_println!(
        ":: U6bx: real File handles — SYS_OPEN/SYS_READ routed through the object table (File + CAP_READ; BSP-staged source) ::"
    );
    Some(U6bxDemo {
        file: USER_BASE + file_off,
        sp: USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16,
        cr3: crate::arch::memory::slot_cr3(slot),
        slot,
    })
}

/// U6bx launcher + verdict (the `u6x_launcher` shape: one gated kernel task on a scheduled sibling core).
/// `demo_cpu` (the task arg) is the core the File-handle fixture runs on. Flow:
///   1. Wait (bounded) for `U6X_LAUNCH_DONE`, so the U6bx lines land after the U6x verdict and the U6x
///      slot has freed.
///   2. Skip silently if no block device / nothing staged — the staged set backs both the fixture's own
///      open AND the no-CAP_READ negative's descriptor (mirrors U4x/U5x/U6x's control-path discipline;
///      `u6bx_probe_once` already staged or skipped with its own line).
///   3. `u6bx_setup()` (build the fixture slot, print the setup line), then PRE-ENDOW its table + PLANT
///      the expected bytes (all before dispatch, no concurrent resolver):
///        - a File handle at `U6BX_NOCAP_IDX` backed by a REAL descriptor but with ZERO rights — the
///          rights arm of the SYS_READ CHECK (a PRESENT File lacking `CAP_READ` must be `-EACCES`);
///        - a `Socket` handle at `U6BX_SOCK_IDX` carrying `CAP_READ` — the kind arm (a non-File object
///          with the right present must still be `-EACCES`);
///        - the staged prefix (first 16 bytes) at window page 2, through the slot's identity backing.
///      Then spawn `u6bx-file` on `demo_cpu`. From `u6bx_setup` on, EVERY path spawns the fixture (the
///      pi4 U6b discipline: there is no free-an-undispatched-slot primitive, so a bail after the alloc
///      would leak the slot — nothing after the alloc is allowed to fail out).
///   4. Verdict (folded): wait (bounded) for the fixture's exit (`U6BX_DONE == 1`), read its witness,
///      then wait (bounded) for its FILES row to clear — the file teardown-clear proof (the fixture exits
///      holding TWO live descriptors: its own open + the pre-endowed no-cap backing, so
///      `files_row_is_clear` transitions false->true when teardown runs). PASS iff witness ==
///      `U6BX_WITNESS_ALL` AND the file row cleared AND no U6bx kill. Prints ONE PASS line, then releases
///      the U7x gate (`U6BX_LAUNCH_DONE`) so the transfer demo orders after this one.
pub fn u6bx_launcher(demo_cpu: usize) {
    // 1. Gate on the U6x launcher (its verdict printed + its slot freed), bounded + yielding.
    let wdeadline = crate::arch::ticks() + 10_000;
    while !U6X_LAUNCH_DONE.load(Ordering::Acquire) && crate::arch::ticks() < wdeadline {
        crate::arch::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No block device, or nothing staged -> the fixture could neither open nor be denied against a
    //    real descriptor; skip silently (the probe already printed a skip line if staging failed).
    if crate::drivers::block::info().is_none() || !HELLO_STAGED.load(Ordering::Acquire) {
        U6BX_LAUNCH_DONE.store(true, Ordering::Release); // release the U7x gate (it also gates on storage)
        return;
    }

    // 3. Build the fixture slot (allocates it + prints the setup line). From here EVERY path spawns.
    let Some(u6b) = u6bx_setup() else {
        serial_println!(":: U6bx: no free address-space slot — File-handle demo skipped ::");
        U6BX_LAUNCH_DONE.store(true, Ordering::Release); // release the U7x gate even on the skip path
        return;
    };
    // 3a. The rights negative: a File handle backed by a REAL descriptor but carrying ZERO rights — a
    //     PRESENT File lacking CAP_READ is exactly the rights arm of the CHECK, distinct from an absent
    //     handle. `files_alloc` on the fixture's FRESH row cannot fail (NFILE free descriptors, cleared at
    //     the prior teardown of this slot); if it somehow did, leave the index Empty — the fixture's read
    //     still gets -EACCES (via NoHandle) and nothing leaks (the row rides teardown either way).
    let staged_sz = HELLO_LEN.load(Ordering::Acquire) as u32;
    if let Some(fid) = files_alloc(u6b.slot, 0, staged_sz) {
        install_cap(u6b.slot, U6BX_NOCAP_IDX, KIND_FILE, (fid + 1) as u64, 0);
    }
    // 3b. The kind negative: a Socket handle carrying CAP_READ — it HAS the right, so the read is denied
    //     purely on kind (SYS_READ serves File only). A scaffold id, no backing (never dereferenced).
    install_cap(u6b.slot, U6BX_SOCK_IDX, KIND_SOCKET, 0x200, CAP_READ);
    // 3c. Plant the expected prefix the fixture compares its read against — the first 16 staged bytes
    //     into window page 2, through the slot's identity backing (never USER_BASE; the ring-3 mappings
    //     stay exactly as built). The staged buffer is the arc's declared source of truth (the honest
    //     divergence), so the bytes-match proves the FULL capability path delivers the source byte-exact
    //     to ring 3; source->disk fidelity is U2's separately (metal-)proven FAT path — and sys_spawn's
    //     children RUN these same staged bytes, a behavioral proof they are the real program. These plain
    //     stores are ordered before the fixture can run by `spawn_user_in_space`'s run-queue publication
    //     (x86-TSO + the queue lock), the same pre-dispatch discipline as the endowments above.
    if let Some(src) = staged_bytes(0) {
        let plant = core::cmp::min(16, src.len());
        unsafe {
            let dst = crate::arch::memory::slot_backing_ptr(u6b.slot).add(2 * PAGE_SIZE as usize);
            core::ptr::copy_nonoverlapping(src.as_ptr(), dst, plant);
        }
    }
    crate::arch::sched::spawn_user_in_space("u6bx-file", u6b.file, u6b.sp, demo_cpu, u6b.cr3);

    // 4a. Wait (bounded, yielding) for the fixture to reach its exit, then snapshot the witness.
    let vdeadline = crate::arch::ticks() + 5000;
    while U6BX_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U6BX_WITNESS.load(Ordering::Acquire);
    let killed = U6BX_KILLED.load(Ordering::Acquire);

    // 4b. FILES-row teardown-clear proof: the fixture exited holding two live descriptors, so its exit
    //     path cleared the row (`clear_handle_row` -> `clear_files_row`). Poll (bounded) until it clears —
    //     false->true when teardown runs. Nothing reuses the slot after (U6bx is the last demo).
    let tdeadline = crate::arch::ticks() + 2000;
    while !files_row_is_clear(u6b.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let files_cleared = files_row_is_clear(u6b.slot);

    if witness == U6BX_WITNESS_ALL && files_cleared && killed == 0 {
        serial_println!(
            ":: U6bx: x86 real File handles — open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U6bx: x86 real File handles FAIL — witness={:#x} files_cleared={} killed={} done={} (want {:#x} / true / 0 / 1) ::",
            witness,
            files_cleared,
            killed,
            U6BX_DONE.load(Ordering::Acquire),
            U6BX_WITNESS_ALL
        );
    }
    // Release the U7x gate: the U6bx verdict has printed and (the fixture having exited) the U6bx slot
    // has freed, so the U7x launcher may build its two fixture slots and order its lines after ours.
    U6BX_LAUNCH_DONE.store(true, Ordering::Release);
}

/// U6bx one-shot, fired from the main loop after `u6x_probe_once` (gated on storage like U4x/U5x/U6x). It
/// spawns the U6bx launcher on a sibling AP; the launcher gates on `U6X_LAUNCH_DONE`, so ordering + the
/// free-slot precondition hold regardless of interleave. The staged set backs both the fixture's SYS_OPEN
/// and the no-CAP_READ negative's descriptor, so — like U4x/U6x — it needs HELLO.BIN staged:
/// `u4x_probe_once` normally staged it long before; this re-stages defensively. Without a FAT
/// volume/HELLO.BIN there is nothing to open, so the demo is skipped cleanly (no false FAIL).
pub fn u6bx_probe_once() {
    // U7x rides the SAME main-loop call sites as this probe (chained here, in-lane, rather than adding a
    // new hook in main.rs): each pass gives the U7x probe its own storage/AP-gated one-shot attempt; the
    // launchers' `U6BX_LAUNCH_DONE` gate orders the demos regardless of which probe fires first.
    u7x_probe_once();
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // retry next loop iteration until storage enumerates (mirrors u4x/u5x/u6x_probe_once)
    }
    let online = crate::arch::smp::online_aps();
    let Some(&cpu) = online.first() else {
        DONE.store(true, Ordering::Relaxed);
        serial_println!(":: U6bx: no application processor online — File-handle demo skipped ::");
        return;
    };
    DONE.store(true, Ordering::Relaxed); // one-shot from here regardless of outcome

    if !HELLO_STAGED.load(Ordering::Acquire) && !stage_hello() {
        serial_println!(":: U6bx: HELLO.BIN not available — File-handle demo skipped ::");
        return;
    }

    let vcpu = online.get(1).copied().unwrap_or(cpu);
    crate::arch::sched::spawn("u6bx-launch", u6bx_launcher, cpu, vcpu, crate::arch::sched::PRIO_NORMAL);
}

// =============================================================================================
// U7x: cross-process transfer — the two-fixture demo (parent delegates, child receives + uses,
// sender revokes) and the gated launcher/verdict. The aarch64 u7_launcher twin, with one structural
// x86 divergence: pi4 co-locates both fixtures on one core and sequences them with SYS_YIELD; x86
// ring 3 is IF-masked/cooperative with no yield syscall, so a polling fixture HOGS its core — each
// fixture therefore gets its OWN dedicated AP (the launcher on a third), and the polls are bounded
// spins (see the blob header). Needs 3 online APs; fewer skips cleanly.
// =============================================================================================

/// One U7x fixture's run parameters (the `U6bxDemo` shape, twice): the ring-3 entry VA, initial user rsp,
/// the slot CR3, and the SLOT id (the inbox key + handle row + the GO/USED word plant).
struct U7xFix {
    entry: u64,
    sp: u64,
    cr3: u64,
    slot: usize,
}

/// Build ONE U7x fixture slot: allocate, scrub the WHOLE window (the pi4 review-confirmed GO-word lesson:
/// slot backings survive teardown, and U6bx deterministically plants nonzero bytes in a prior tenant of
/// this slot — a stale nonzero GO would release a fixture early and turn the single-writer snapshot into
/// a race; x86 setups already scrub, kept explicit here), copy the (shared two-entry) U7x blob into its
/// code page through the identity alias (RX-RO from the start — W^X by construction), and return the run
/// params for the requested entry symbol. Does NOT pre-endow (the launcher does, per fixture, before
/// dispatch). `None` if slot allocation fails.
fn u7x_build(entry_sym: *const u8) -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u7x_blob_start as usize;
    let bend = &raw const unaos_user_u7x_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U7x blob does not fit in a code page");
    let off = (entry_sym as usize - bstart) as u64;
    let backing = crate::arch::memory::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, (USER_WINDOW_PAGES * PAGE_SIZE) as usize);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    Some(U7xFix {
        entry: USER_BASE + off,
        sp: USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16,
        cr3: crate::arch::memory::slot_cr3(slot),
        slot,
    })
}

/// Release a fixture's GO word (window +0x3000): the launcher-side half of the demo's sequencing. Written
/// through the slot's identity backing (the u6bx data-plant path); a volatile store suffices on x86-TSO —
/// the spinning fixture's next aliased load observes it.
fn u7x_release_go(slot: usize) {
    unsafe {
        let go = crate::arch::memory::slot_backing_ptr(slot).add(U7X_GO_OFF) as *mut u64;
        core::ptr::write_volatile(go, 1);
    }
}

/// U7x launcher + verdict (the `u6bx_launcher` shape: one gated kernel task on a scheduled sibling core).
/// `demo_cpu` (the task arg) is the CHILD's core; the PARENT runs on a third AP (see the section note).
/// Flow:
///   1. Wait (bounded) for `U6BX_LAUNCH_DONE`, so the U7x lines land after the U6bx verdict and its slot
///      has freed.
///   2. Skip silently if no block device (mirrors U4x..U6bx's control-path discipline — U7x itself needs
///      no disk, but the gate keeps the no-storage control path free of demo lines). Skip with a line if
///      fewer than 3 APs are online (the parent needs a core neither the child nor this launcher holds).
///   3. Claim a Proc entry (the child's pid->slot map for SYS_XFER), build the CHILD slot (row
///      deliberately EMPTY — the single-writer snapshot depends on it), spawn `u7x-child` (it spins on
///      its GO word), publish its slot+pid into the Proc entry, build + PRE-ENDOW the PARENT
///      (U7X_DEST_IDX = a Child handle naming the child; U7X_SRC_IDX = a full Console cap
///      `CAP_WRITE|CAP_GRANT`), print the setup line, spawn `u7x-parent`.
///   4. THE SINGLE-WRITER WITNESS: wait (bounded) until the parent's t1 deposit is LIVE in the child's
///      inbox, then — with the child still parked on its GO word, provably pre-RECV — verify the child's
///      handle row is still completely CLEAR (`handle_row_is_clear`): the deposit crossed processes
///      without one byte landing in the recipient's row. Only then release the child's GO.
///   5. Wait (bounded) for the child's USED word (its first write through the transferred cap landed),
///      then release the parent's GO — so the revoke is provably use-then-revoke.
///   6. Verdict (folded): wait (bounded) for both witness exits (`U7X_DONE == 2`), read both witnesses,
///      then wait (bounded) for the teardown proof — both handle rows clear, both inboxes clear, and the
///      transfer-record ledger fully FREE (every transfer's lifetime closed: t1 freed when the child's
///      revoked handle was torn down, t2 likewise, no pending residue). Free the planted Proc entry. PASS
///      iff both witnesses == `U7X_WITNESS_ALL` AND used AND the snapshot held AND everything cleared AND
///      no U7x kill. Prints ONE PASS line. U7x is the last demo, so it releases no further gate.
pub fn u7x_launcher(demo_cpu: usize) {
    // U8x rides the SAME kernel task, strictly after the whole U7x flow (every U7x exit path — PASS, FAIL,
    // or skip — falls through to it): the ordering gate the `*_LAUNCH_DONE` statics provide between
    // separately spawned launchers is here the program order of one task, and the U7x fixtures' slots have
    // torn down by the time `u7x_run` returns (its verdict waits on their exits + teardown). The aarch64
    // u7_launcher twin.
    u7x_run(demo_cpu);
    u8x_launcher(demo_cpu);
}

fn u7x_run(demo_cpu: usize) {
    // 1. Gate on the U6bx launcher (its verdict printed + its slot freed), bounded + yielding.
    let wdeadline = crate::arch::ticks() + 10_000;
    while !U6BX_LAUNCH_DONE.load(Ordering::Acquire) && crate::arch::ticks() < wdeadline {
        crate::arch::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No block device -> keep the no-storage control path free of demo lines (U7x needs no disk).
    if crate::drivers::block::info().is_none() {
        return;
    }
    // The parent's dedicated core: a third AP, distinct from the child's (`demo_cpu`) and this
    // launcher's. Cooperative ring 3 hogs its core while polling, so sharing either would deadlock the
    // sequencing (the launcher could never run to release a GO word).
    let online = crate::arch::smp::online_aps();
    let Some(&parent_cpu) = online.get(2) else {
        serial_println!(":: U7x: fewer than 3 application processors — transfer demo skipped ::");
        return;
    };

    // 3a. The child's Proc entry FIRST (nothing else claimed if the table is full).
    let Some(pi) = proc_reserve() else {
        serial_println!(":: U7x: no free process entry — transfer demo skipped ::");
        return;
    };
    // 3b. Build + spawn the CHILD (its handle row stays EMPTY — the single-writer snapshot depends on
    //     that; it parks on its GO word, so it makes no syscall that could populate anything).
    let Some(child) = u7x_build(&raw const unaos_user_u7x_child) else {
        serial_println!(":: U7x: no free address-space slot — transfer demo skipped ::");
        proc_free(pi);
        return;
    };
    let child_pid =
        crate::arch::sched::spawn_user_in_space("u7x-child", child.entry, child.sp, demo_cpu, child.cr3);
    // Publish the pid->slot map (slot first — the sys_spawn discipline — then the pid, the live key).
    PROCS[pi].slot.store(child.slot + 1, Ordering::Release);
    PROCS[pi].pid.store(child_pid, Ordering::Release);
    // 3c. Build + pre-endow + spawn the PARENT. If its slot alloc fails the child is already live: it
    //     parks to its GO budget, exits with its (empty) witness, and its slot tears down cleanly.
    let Some(parent) = u7x_build(&raw const unaos_user_u7x_parent) else {
        serial_println!(":: U7x: no free address-space slot — transfer demo skipped (child will park out) ::");
        proc_free(pi);
        return;
    };
    install_cap(parent.slot, U7X_DEST_IDX, KIND_CHILD, child_pid, CAP_READ);
    install_cap(parent.slot, U7X_SRC_IDX, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    serial_println!(
        ":: U7x: cross-process transfer — inbox-mediated SYS_XFER/SYS_RECV + sender revoke (single-writer preserved) ::"
    );
    crate::arch::sched::spawn_user_in_space("u7x-parent", parent.entry, parent.sp, parent_cpu, parent.cr3);

    // 4. The single-writer witness: t1 pending in the child's inbox + the child's row still untouched
    //    (the child is provably pre-RECV — it is parked on the GO word this launcher has not released).
    let ddeadline = crate::arch::ticks() + 5000;
    let mut deposit_seen = false;
    while !deposit_seen && crate::arch::ticks() < ddeadline {
        deposit_seen = (0..NXFER).any(|k| {
            let t = XFER_SLOT_TX[child.slot][k].load(Ordering::Acquire);
            t != 0 && t != HANDLE_RESERVING
        });
        if !deposit_seen {
            crate::arch::sched::yield_now();
        }
    }
    let snap_ok = deposit_seen && handle_row_is_clear(child.slot);
    u7x_release_go(child.slot);

    // 5. Use-then-revoke sequencing: wait for the child's first successful write through the cap (its
    //    USED word, read through the slot backing), then let the parent revoke.
    let used_ptr = unsafe {
        crate::arch::memory::slot_backing_ptr(child.slot).add(U7X_USED_OFF) as *const u64
    };
    let udeadline = crate::arch::ticks() + 5000;
    while unsafe { core::ptr::read_volatile(used_ptr) } == 0 && crate::arch::ticks() < udeadline {
        crate::arch::sched::yield_now();
    }
    let used = unsafe { core::ptr::read_volatile(used_ptr) };
    u7x_release_go(parent.slot);

    // 6a. Wait (bounded) for both witness exits, then snapshot the witnesses.
    let vdeadline = crate::arch::ticks() + 8000;
    while U7X_DONE.load(Ordering::Acquire) < 2 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let pw = U7X_PARENT_WITNESS.load(Ordering::Acquire);
    let cw = U7X_CHILD_WITNESS.load(Ordering::Acquire);
    let killed = U7X_KILLED.load(Ordering::Acquire);

    // 6b. Teardown/leak proof: both rows + both inboxes clear and the record ledger fully free (t1's
    //     record was released when the child's revoked handle tore down; t2's likewise — false->true as
    //     the exits' teardowns run).
    let all_clear = |ps: usize, cs: usize| {
        handle_row_is_clear(ps)
            && handle_row_is_clear(cs)
            && xfer_row_is_clear(ps)
            && xfer_row_is_clear(cs)
            && xfer_recs_all_free()
    };
    let tdeadline = crate::arch::ticks() + 2000;
    while !all_clear(parent.slot, child.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = all_clear(parent.slot, child.slot);
    proc_free(pi); // the planted pid->slot entry (the fixtures exited by name, never through the Proc path)

    if pw == U7X_WITNESS_ALL && cw == U7X_WITNESS_ALL && used != 0 && snap_ok && cleared && killed == 0 {
        serial_println!(
            ":: U7x: cross-process transfer — SYS_XFER attenuated, child received + used the cap, revoke enforced, single-writer intact -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U7x: cross-process transfer FAIL — parent={:#x} child={:#x} used={} snap={} cleared={} killed={} done={} (want {:#x}/{:#x}/1/true/true/0/2) ::",
            pw,
            cw,
            used,
            snap_ok,
            cleared,
            killed,
            U7X_DONE.load(Ordering::Acquire),
            U7X_WITNESS_ALL,
            U7X_WITNESS_ALL
        );
    }
}

// =============================================================================================
// U8x: revocation trees — the single-process ring-3 fixture (grant-chain kill + locality + errno
// negatives) and the kernel-side cross-process checks (re-transfer cascade + generation), folded
// into one launcher/verdict that rides the U7x launcher task. The aarch64 u8_launcher twin.
// =============================================================================================

/// Build the U8x fixture slot — the `u7x_build` shape for the U8x blob (allocate, scrub the WHOLE window,
/// copy the blob into its RX-RO code page through the identity alias, return the run params). `None` if slot
/// allocation fails. Does NOT pre-endow (the launcher does, before dispatch).
fn u8x_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u8x_blob_start as usize;
    let bend = &raw const unaos_user_u8x_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U8x blob does not fit in a code page");
    let off = (&raw const unaos_user_u8x_tree as usize - bstart) as u64;
    let backing = crate::arch::memory::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, (USER_WINDOW_PAGES * PAGE_SIZE) as usize);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    Some(U7xFix {
        entry: USER_BASE + off,
        sp: USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16,
        cr3: crate::arch::memory::slot_cr3(slot),
        slot,
    })
}

/// The U8x kernel-side checks — the cross-process halves the single-process fixture cannot stage. Drives
/// the REAL syscall bodies (`sys_xfer_from`/`sys_recv_for`/`sys_cap_grant`/`sys_cap_xrevoke`) over three
/// scratch rows (5/6/7 — every demo fixture has exited and torn down by the time this runs, so the rows are
/// provably clear and nothing else touches them; every planted resource is dropped again before returning,
/// verified by the final ledger-clean checks the verdict folds in). Rows 5/6/7 are all private (< USER_SLOTS
/// = 8), so none is the refused `SHARED_ROW`. Returns true iff ALL hold:
///
///  1. THE RE-TRANSFER CASCADE (U7x escape #2): S transfers a console cap to R1 (with CAP_GRANT); R1
///     re-transfers it onward to R2; S revokes the ROOT transfer -> R1's cap is stale (U7x semantics) AND
///     R2's re-transferred cap is stale TOO (the tree — U7x provably let this one escape), and a re-grant
///     from the dead cap is refused.
///  2. GENERATION-TAGGED INBOXES: a deposit stamped for R1's current tenant, followed by R1's teardown
///     generation bump (the `clear_handle_row` primitive — here invoked as the bare bump, the exact word
///     teardown writes), is NEVER delivered to the recycled row's next tenant (RECV discards it; its record
///     frees, so the sender's later XREVOKE honestly finds nothing).
///  3. LEDGER HYGIENE: after dropping every planted handle/Proc entry, the handle rows, inboxes, transfer
///     records AND the derivation ledger are all fully clear — no revoke/discard path leaked a node.
fn u8_kernel_check() -> bool {
    const S: usize = 5; // scratch "sender" row
    const R1: usize = 6; // scratch first recipient
    const R2: usize = 7; // scratch grand-recipient
    const PID1: u64 = 0xE1; // planted recipient pids (never collide: PROCS holds only planted entries now)
    const PID2: u64 = 0xE2;
    let mut ok = true;

    // Plant the two recipient Proc entries (the pid->slot maps sys_xfer resolves through).
    let Some(p1) = proc_reserve() else {
        return false;
    };
    let Some(p2) = proc_reserve() else {
        proc_free(p1);
        return false;
    };
    PROCS[p1].slot.store(R1 + 1, Ordering::Release); // +1-biased, like sys_spawn's pid->slot map
    PROCS[p1].pid.store(PID1, Ordering::Release);
    PROCS[p2].slot.store(R2 + 1, Ordering::Release);
    PROCS[p2].pid.store(PID2, Ordering::Release);
    // The sender's table: a delegable console cap at 0, a Child handle naming R1's tenant at 2 (in S's row)
    // and — in R1's own row — a Child handle naming R2's tenant at 2, for the onward hop.
    install_cap(S, 0, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    install_cap(S, 2, KIND_CHILD, PID1, 0);
    install_cap(R1, 2, KIND_CHILD, PID2, 0);

    // 1. The cascade. S -> R1 (keeping CAP_GRANT so R1 may delegate onward) -> R2; then revoke the root.
    let t1 = sys_xfer_from(S, 2, 0, (CAP_WRITE | CAP_GRANT) as u64);
    ok &= t1 > 0;
    let h1 = sys_recv_for(R1);
    ok &= h1 >= 0;
    // h2 must carry CAP_GRANT: the laundering assertion below has to reach the U8x tree-deep staleness check
    // in sys_cap_grant — without CAP_GRANT the earlier missing-right gate returns the same EACCES and the
    // assertion is vacuous (aarch64 U8 review note, carried into the twin).
    let t2 = if h1 >= 0 {
        sys_xfer_from(R1, 2, h1 as u64, (CAP_WRITE | CAP_GRANT) as u64)
    } else {
        -1
    };
    ok &= t2 > 0;
    let h2 = sys_recv_for(R2);
    ok &= h2 >= 0;
    // Pre-revoke, the grand-received cap carries real authority.
    ok &= matches!(handle_resolve(R2, h2 as u64, CAP_WRITE), Ok(HandleTarget::Console));
    // The sender revokes the ROOT transfer...
    ok &= t1 > 0 && sys_cap_xrevoke(S, t1 as u64) == 0;
    // ...the direct recipient's cap is stale (U7x's own guarantee, unchanged)...
    ok &= handle_resolve(R1, h1 as u64, CAP_WRITE).is_err();
    // ...AND the RE-TRANSFERRED cap is stale too — the U7x escape, closed by the derivation walk.
    ok &= handle_resolve(R2, h2 as u64, CAP_WRITE).is_err();
    // ...and the dead cap cannot be laundered by a fresh local mint either — h2 CARRIES CAP_GRANT, so this
    // EACCES provably comes from the tree-deep staleness check, not the missing-right gate.
    ok &= sys_cap_grant(R2, h2 as u64, CAP_WRITE as u64) == EACCES;

    // 2. Generations. Deposit for R1's CURRENT tenant, then that tenant tears down (the generation bump is
    //    the exact store `clear_handle_row` opens with) — the recycled row's next tenant RECVs nothing.
    let t3 = sys_xfer_from(S, 2, 0, CAP_WRITE as u64);
    ok &= t3 > 0;
    SLOT_GEN[R1].fetch_add(1, Ordering::AcqRel); // teardown + recycle: a NEW tenant generation
    ok &= sys_recv_for(R1) == EAGAIN; // the stale deposit is discarded, never delivered
    ok &= t3 > 0 && sys_cap_xrevoke(S, t3 as u64) == ENOENT; // its record already freed by the discard

    // 3. Drop everything planted/delivered, then demand every ledger fully clear (subtree tombstones
    //    drained, no node/record/slot leaked on any of the paths above).
    if h2 >= 0 {
        handle_clear(R2, h2 as usize);
    }
    if h1 >= 0 {
        handle_clear(R1, h1 as usize);
    }
    handle_clear(R1, 2);
    handle_clear(S, 2);
    handle_clear(S, 0);
    proc_free(p1);
    proc_free(p2);
    ok &= handle_row_is_clear(S) && handle_row_is_clear(R1) && handle_row_is_clear(R2);
    ok &= xfer_row_is_clear(R1) && xfer_row_is_clear(R2);
    ok &= xfer_recs_all_free() && deriv_all_free();
    ok
}

/// U8x launcher + verdict — called by `u7x_launcher` after the whole U7x flow (program-order gating; see
/// `u7x_launcher`). Flow: one-shot guard; skip silently with no block device (the control-path discipline —
/// U8x needs no disk); build + pre-endow + spawn the single fixture (`u8x-tree`: index 2 = a console cap
/// WITH `CAP_REVOKE`, index 3 = one WITHOUT); wait (bounded) for its witness exit; wait (bounded) for its
/// teardown (row clear + the derivation ledger drained — the tombstone-cascade proof); run the kernel-side
/// cross-process checks (which need the clear ledgers); PASS iff witness == `U8X_WITNESS_ALL` AND torn down
/// AND no kill AND the kernel checks held. U8x is the last demo — it releases no further gate.
fn u8x_launcher(demo_cpu: usize) {
    // One-shot (the U7x launcher is spawned once; guard defensively anyway).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // No block device -> keep the no-storage control path free of demo lines (mirrors the prior gates).
    if crate::drivers::block::info().is_none() {
        return;
    }
    let Some(fix) = u8x_build() else {
        serial_println!(":: U8x: no free address-space slot — revocation-tree demo skipped ::");
        return;
    };
    install_cap(fix.slot, U8X_SRC_IDX, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT | CAP_REVOKE);
    install_cap(fix.slot, U8X_SRC2_IDX, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    serial_println!(
        ":: U8x: revocation trees — derivation ledger + generation-tagged inboxes (revoke chases the subtree) ::"
    );
    crate::arch::sched::spawn_user_in_space("u8x-tree", fix.entry, fix.sp, demo_cpu, fix.cr3);

    // Wait (bounded, yielding) for the fixture's witness exit, then snapshot the witness.
    let vdeadline = crate::arch::ticks() + 5000;
    while U8X_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U8X_WITNESS.load(Ordering::Acquire);
    let killed = U8X_KILLED.load(Ordering::Acquire);

    // Teardown proof: the fixture exited holding live derived handles (g1/g2/g4 and the two endowed sources
    // — g1/g2 already stale, but their NODES persist as tombstones until the row clears), so its teardown
    // must drain BOTH the handle row and the derivation ledger. Poll bounded; false->true.
    let tdeadline = crate::arch::ticks() + 2000;
    while !(handle_row_is_clear(fix.slot) && deriv_all_free()) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = handle_row_is_clear(fix.slot) && deriv_all_free();

    // Kernel-side cross-process checks (they require the drained ledgers the wait above establishes).
    let ledger_ok = cleared && u8_kernel_check();

    if witness == U8X_WITNESS_ALL && cleared && ledger_ok && killed == 0 {
        serial_println!(
            ":: U8x: revocation trees — parent revoke kills re-grant + re-transfer, generation-tagged inbox, ledger clean -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U8x: revocation trees FAIL — witness={:#x} cleared={} ledger={} killed={} done={} (want {:#x}/true/true/0/1) ::",
            witness,
            cleared,
            ledger_ok,
            killed,
            U8X_DONE.load(Ordering::Acquire),
            U8X_WITNESS_ALL
        );
    }
}

/// U7x one-shot, chained off `u6bx_probe_once`'s main-loop call sites (gated on storage like the prior
/// probes, so the no-storage control path stays free of demo lines). It spawns the U7x launcher on a
/// sibling AP; the launcher gates on `U6BX_LAUNCH_DONE`, so ordering + the free-slot precondition hold
/// regardless of interleave. No FAT I/O — both U7x fixtures are inline blobs.
pub fn u7x_probe_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // retry next loop iteration until storage enumerates (mirrors the prior probes)
    }
    let online = crate::arch::smp::online_aps();
    let Some(&cpu) = online.first() else {
        DONE.store(true, Ordering::Relaxed);
        serial_println!(":: U7x: no application processor online — transfer demo skipped ::");
        return;
    };
    DONE.store(true, Ordering::Relaxed); // one-shot from here regardless of outcome

    let vcpu = online.get(1).copied().unwrap_or(cpu);
    crate::arch::sched::spawn("u7x-launch", u7x_launcher, cpu, vcpu, crate::arch::sched::PRIO_NORMAL);
}
