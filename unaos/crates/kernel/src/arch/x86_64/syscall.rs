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

use spin::Mutex as SpinMutex;
use x86_64::registers::control::Cr4;
use x86_64::registers::model_specific::{LStar, Msr};
use x86_64::VirtAddr;

use crate::arch::percpu::{KERNEL_RSP_OFFSET, USER_RSP_OFFSET};

// --- Syscall numbers (the tiny U1a subset; mirrors aarch64). ---
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
// WINX-1: the PROCESS verbs x86 was missing, at their SHARED numbers (the aarch64 twins are
// `sys_yield`/`sys_sleep_ms`/`sys_getinfo`). 3 (`SYS_REPORT`) and 6 (`SYS_GETPID`) stay unimplemented
// here: x86 routes fixture witnesses BY TASK NAME through the `SYS_EXIT` arm (the u5x/u6x idiom), so it
// has never needed `SYS_REPORT`, and nothing yet asks for a bare pid that `SYS_GETINFO` does not carry.
// Their NUMBERS are reserved by the shared law regardless — an x86 caller of 3 or 6 falls to the
// `-ENOSYS` default rather than colliding with something else.
const SYS_YIELD: u64 = 4;
const SYS_SLEEP_MS: u64 = 5;
const SYS_GETINFO: u64 = 7;
// WINX-1: the WINDOW verbs, at the shared numbers the aarch64 WC-B arc minted. 31 (`SYS_WIN_MOVE`) and
// 32 (`SYS_WIN_CLOSE`) are reserved but not implemented this arc: nothing x86 runs asks to reposition or
// explicitly retire a window, and a window is retired correctly at slot teardown (`win_close_slot`).
const SYS_WIN_CREATE: u64 = 29;
const SYS_WIN_PRESENT: u64 = 30;
// WINX-7: the THREAD verbs, at the shared numbers the aarch64 ELF-2 arc minted. Semantics are the
// aarch64 twins', verbatim:
//   SYS_THREAD_SPAWN(entry, sp, arg, place) -> thread handle >= 0 / -errno. A new ring-3 task under the
//     CALLER's own address space (same CR3), entered at `entry` on stack `sp` with `arg` in the SysV
//     first-argument register. `place`: 0 = the caller's core, 1 = a sibling online core.
//   SYS_THREAD_JOIN(handle) -> 0 / -ESRCH. Block until that thread finishes, then reap its handle.
//   SYS_THREAD_EXIT() — terminate the calling thread: post its completion (waking a joiner) and drop
//     this task's hold on the shared address space (teardown only on the LAST thread).
const SYS_THREAD_SPAWN: u64 = 21;
const SYS_THREAD_EXIT: u64 = 22;
const SYS_THREAD_JOIN: u64 = 23;
// WINX-7: SYS_FUTEX(uaddr, op, val) -> op-specific / -errno — the EL0 wait/wake a userspace mutex or
// frame barrier is built out of. `op`: 0 = WAIT (block iff `*uaddr == val`), 1 = WAKE (wake up to
// `val` waiters; returns the count woken). 24/25 (`SYS_FB_MAP`/`SYS_FB_PRESENT`) stay reserved and
// unimplemented on x86 — see the window-verb note above.
const SYS_FUTEX: u64 = 26;
/// WINX-7: `SYS_FUTEX` sub-ops (passed in the second argument). Shared with aarch64 by law.
const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
// WINX-7: SYS_INPUT_POLL() -> a packed input event (>= 0, bit 63 always clear) / -EAGAIN when the
// caller's ring is empty. The delivery half of "an EL0 app can be interactive": the kernel holds a
// small per-process ring, the router fills the FOCUSED process's ring, and EL0 drains its own
// nonblocking. 28 is unassigned on both arches.
const SYS_INPUT_POLL: u64 = 27;
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
// U9x: absolute seek on an open File descriptor (the aarch64 pi4 U9 twin; same number). SEEK(handle,
// offset) -> the new absolute offset, or a negative errno. The CHECK requires a File handle carrying ANY
// of `CAP_READ|CAP_WRITE`; an offset PAST the file's size is `-EINVAL` (seeking exactly TO size, the EOF
// position, is legal). A later SYS_READ / File SYS_WRITE resumes from the seeked offset. No I/O — a pure
// descriptor-state update, so it is IF-masked-handler-safe (the whole x86 staged-storage divergence).
const SYS_SEEK: u64 = 15;
// U11x: CLOSE an open File — SYS_CLOSE(handle) -> `0`, or a negative errno (the aarch64 pi4 U11 twin; same
// number). Frees the handle's open-file DESCRIPTOR (bumping its generation so a first-fit slot reuse can never
// re-bind a lingering sibling file-id to a different file — the U9x revoke+reopen aliasing gap) and clears the
// handle word. Close is not a mutation of the object, so it requires NO capability right; a non-File kind is
// `-EINVAL` (left intact), and an unresolvable / already-closed / stale-slot handle is `-EBADF` (a double-close
// returns cleanly; a use-after-close is denied). No I/O — the x86 staged write-back happens at whole-task
// teardown (`clear_files_row`), so like a revoke this drop DISCARDS any un-flushed dirty bytes (only teardown
// persists; a future arc could make an explicit close enqueue the flush).
const SYS_CLOSE: u64 = 17;
// U10 M3: DELETE (unlink) the runtime-created file an open File+CAP_WRITE handle names — SYS_UNLINK(handle) -> 0,
// or a negative errno (the aarch64 U10 twin; same number). Gated by the SAME single CAP_WRITE CHECK as write
// (delete is a mutation). Marks the name gone for the row (a re-open is -ENOENT), invalidates ALL of this
// process's descriptors for it (the U11x gen-tag mechanism — no stale reference), and enqueues the on-disk delete
// (create+grow+delete replayed at the launcher's IF=1 drain, since the fixture's create/grow never persisted).
const SYS_UNLINK: u64 = 16;
// U10: SYS_OPEN `mode` bit1 — create the file if it is absent from the "volume" (the aarch64 U10 O_CREAT twin;
// same encoding). bit0 = RW. `mode == 3` (O_CREAT | RW) is what the create/delete fixtures pass. A create is
// inherently RW (you create to write it); higher bits (O_TRUNC/O_EXCL/O_APPEND) stay reserved this arc.
const O_CREAT: u64 = 1 << 1;
// U6x: SYS_OPEN `mode` bit2 — opt an O_CREAT of a NEW name OUT of owned-by-default into world-access (the
// aarch64 U6 twin; same encoding). Ignored on an open of an existing file (ownership is fixed at create) and
// outside O_CREAT. Owned-by-default: a private create (no O_PUBLIC) records the creator as OWNER; O_PUBLIC
// keeps the pre-U6 open-by-anyone behaviour. See `open_create_new` and the owner/grants block.
const O_PUBLIC: u64 = 1 << 2;
// U6x: UnaFS owner/grants delegation — SYS_FGRANT(file_handle, child_handle, rights) -> 0, or a negative errno
// (the aarch64 U6 twin; same number). The OWNER of a private created file grants (a CAP_READ|CAP_WRITE subset)
// or revokes (rights == 0) access to another principal named OWNER-SCOPED by a `Child` handle the caller holds
// (the SYS_XFER idiom — no raw pid/slot from ring 3). The grant is an ACL edge on the FILE (nothing delivered to
// the grantee's table); the grantee opens the name and the SYS_OPEN ACL admits it. See `sys_fgrant`.
const SYS_FGRANT: u64 = 18;
// SOCK-2 (ROADMAP §1b): the UDP socket syscall family — the FIRST time ring 3 reaches the network.
// A socket is a new object-table kind (`KIND_SOCKET`, already scaffolded as the U6bx/U9x kind
// negative) whose value word is a persistent-`SocketSet` id; the handle is a capability exactly like a
// File (`CAP_READ` gates recv, `CAP_WRITE` gates send, so `SYS_CAP` GRANT attenuates a socket to
// send-only / recv-only). SOCKET() mints one carrying `CAP_READ|CAP_WRITE`; BIND(handle, port) names a
// local UDP port; SENDTO(handle, msg_ptr, msg_len) sends a datagram whose 8-byte header is
// `[dst_ip[4]][dst_port u16 LE][pad u16]` followed by the payload; RECVFROM(handle, buf_ptr, buf_len)
// writes that same header shape (source addr) + payload and returns the total, or `-EAGAIN` when empty
// (NON-BLOCKING — the IF-masked handler cannot block; smolnet drives a bounded poll pump). x86-only,
// `smolnet` feature (DEFAULT-ON since SMOLNET-DEFAULT; dropped under `UNAOS_NOSMOLNET=1`); aarch64 /
// opt-out never compile these arms (byte-identical).
//
// SOCKNUM (WINX-1): the family lives at 40..=48, NOT at the 19..=27 it originally claimed. The
// SHARED-NUMBER LAW is that a syscall NUMBER means the same verb on every arch — the ABI is described
// as Linux-style and cross-arch shared, and ring-3 programs above the per-arch asm stubs are meant to
// be arch-neutral Rust that names verbs by number. The original SOCK-2/3/6 numbering broke that law
// silently, because aarch64 had already spent 19..=27:
//     19 MSEND      20 MRECV       21 THREAD_SPAWN  22 THREAD_EXIT  23 THREAD_JOIN
//     24 FB_MAP     25 FB_PRESENT  26 FUTEX         27 INPUT_POLL
// Nothing caught it: the two families never had to coexist, since x86 compiled no window/thread verbs
// and aarch64 compiles no socket verbs. The collision only becomes load-bearing when x86 grows the
// WINDOW verbs (WINX-1) and, later, the thread/futex/input verbs — at which point `SYS_INPUT_POLL` and
// `SYS_ACCEPT` would both have to be 27 in the SAME dispatch. Moving the x86-only family (the arch that
// is alone in using these ids) to a free contiguous block at 40..=48 restores the law and leaves
// 19..=27 meaning on x86 exactly what it means on aarch64. RELATIVE ORDER is preserved, so the family
// reads the same and a reviewer can diff it by inspection.
//
// This is a ring-3 ABI BREAK by construction, and deliberately a cheap one: every caller of these
// numbers is IN-TREE (the inline-asm fixtures below), there is no out-of-tree x86 socket binary, and
// the SOCK-2/3/4/6 + zeolite legs of the headless suite are the completeness proof — a missed
// reference cannot pass, because a fixture that keeps the old immediate lands in an unrelated arm (or
// `-ENOSYS`) and fails its verdict.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_SOCKET: u64 = 40;
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_BIND: u64 = 41;
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_SENDTO: u64 = 42;
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_RECVFROM: u64 = 43;
// SOCK-3 (ROADMAP §1b): the TCP CLIENT socket syscalls — ring 3's first byte stream. A TCP socket is
// minted by `SYS_SOCKET` with type SOCK_STREAM(1) (the same `KIND_SOCKET` capability, gen-fenced value
// word). CONNECT(handle, msg_ptr, msg_len) active-opens to the peer in `msg`'s 8-byte
// `[ip[4]][port u16 LE][pad u16]` header (NON-BLOCKING: `0` established / `-EINPROGRESS` still handshaking
// — ring 3 polls by re-calling connect / `-ECONNREFUSED` reset); SEND(handle, buf, len) streams bytes
// (returns the count queued, or `-EAGAIN` tx-full / `-ENOTCONN`); RECV(handle, buf, len) reads stream
// bytes (bounded poll pump: the count, `-EAGAIN` when none yet, or `0` at clean end-of-stream). Send needs
// `CAP_WRITE`, recv needs `CAP_READ`, connect needs `CAP_WRITE` (a configuring authority, like bind).
// x86-only, knob-on; aarch64 / knob-off never compile these arms (byte-identical).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_CONNECT: u64 = 44;
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_SEND: u64 = 45;
// `SYS_SOCK_RECV` (not `SYS_RECV` — 14 is the capability-transfer inbox recv) — the stream recv.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_SOCK_RECV: u64 = 46;
// SOCK-6: TCP SERVER sockets — `SYS_LISTEN` arms a passive listener, `SYS_ACCEPT` polls for an inbound
// connection and mints a fresh `KIND_SOCKET` handle for it (the ring 3 now ACCEPTS inbound TCP).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_LISTEN: u64 = 47;
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SYS_ACCEPT: u64 = 48;

/// Base of the ring-3 window: 1 TiB — a FRESH top-level slot (PML4 index 2) above the firmware
/// identity map, so mapping it touches no kernel state. `setup` proves it unmapped before use.
pub const USER_BASE: u64 = 0x0000_0100_0000_0000;
/// Window size in 4 KiB pages: code, data, and two stack pages.
const USER_WINDOW_PAGES: u64 = 4;

/// WINX-2: the ring-3 window base, for the ELF loader (which lives in a sibling module and must not
/// duplicate the constant). The aarch64 twin reads the same pair from `boot::user_region()`.
pub fn user_base() -> u64 {
    USER_BASE
}

/// WINX-2: the ring-3 PROGRAM window size in bytes — the bound `validate_elf` fits every segment span
/// into, and the top of which is the initial ring-3 RSP. Deliberately the PROGRAM window only: the FB
/// region above it is mapped by `SYS_WIN_CREATE`, never by the loader, so an image can never place a
/// segment over a window surface.
pub fn user_window_size() -> usize {
    (USER_WINDOW_PAGES * PAGE_SIZE) as usize
}
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

// --- U9x ring-3 fixture (real File WRITES + SEEK — the aarch64 `__u9_prog_write` twin). ONE fixture
// (`u9x-write`) exercising the object table's FIRST mutating resource path: it opens a DEDICATED scratch file
// RW, seeks into it, overwrites a known 16-byte pattern IN PLACE (into its per-descriptor writable staging
// buffer — the x86 stand-in for pi4's in-place FAT write; see `sys_write_file`), seeks back and reads it
// through the SAME capability to witness the write landed, and proves the `sys_write` CHECK rejects both an
// RO-opened File (missing CAP_WRITE — the rights arm) and a non-File handle carrying CAP_WRITE (the kind arm).
// Register-only apart from the read-back dest: the dest is the DATA page (window page 1) and the write
// pattern a RO code-page `.ascii` constant, both fixed window VAs — it writes NO user stack, so it is safe on
// any slot under preemption. `u9x_launcher` pre-endows a `Socket` handle at index 2 carrying CAP_WRITE (the
// kind negative). The fixture's own RW `SYS_OPEN` first-free-claims index 0 (index 1 = the reserved
// CONSOLE_FD, never auto-allocated; 2 is pre-endowed); its RO open then claims index 3. It builds a 5-bit
// witness bitmask (see `U9X_WITNESS_ALL`) and conveys it as its `sys_exit` STATUS, routed BY NAME into
// `U9X_WITNESS` (x86 has no SYS_REPORT — the u5x/u6bx/u8x idiom). ABI (Linux-style): rax = number, args
// rdi/rsi/rdx, return in rax; rcx/r11 are the SYSCALL-clobbered pair, so state rides r12-r15/rbx.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u9x_blob_start
unaos_user_u9x_blob_start:
    .balign 16
    .globl unaos_user_u9x_write
unaos_user_u9x_write:
    xor r12d, r12d                          // witness bitmask = 0 (survives syscalls)
    lea r14, [rip + unaos_user_u9x_blob_start] // r14 = window base (blob runs at the code-page base)
    add r14, 0x1000                         // r14 -> read-back dest (writable DATA page, window page 1)
    lea r15, [rip + unaos_user_u9x_pattern] // r15 -> the 16-byte write pattern (RO code page; also compare src)

    // (0) SYS_OPEN("SCRATCH.BIN", RW) -> a File handle carrying CAP_READ|CAP_WRITE
    mov rax, 11                             // SYS_OPEN
    lea rdi, [rip + unaos_user_u9x_name]    // name ptr (RO code page — ring-3 readable)
    mov rsi, [rip + unaos_user_u9x_namelen] // name len ("SCRATCH.BIN" = 11)
    mov rdx, 1                              // mode = RW
    syscall
    mov rbx, rax                            // rbx = RW handle (>= 0) or -errno
    test rbx, rbx
    js 1f                                   // open failed (negative) -> skip bit0/1/2
    add r12, 1                              // bit0: open RW OK

    // (1) seek to the scratch offset (520), then overwrite the 16-byte pattern in place
    mov rax, 15                             // SYS_SEEK
    mov rdi, rbx
    mov rsi, 520                            // U9X_WRITE_OFFSET
    syscall
    cmp rax, 520                            // seek returns the new absolute offset
    jne 1f
    mov rax, 1                              // SYS_WRITE (File + CAP_WRITE -> in-place staged-buffer overwrite)
    mov rdi, rbx
    mov rsi, r15                            // src = the 16-byte pattern
    mov rdx, 16
    syscall
    cmp rax, 16                             // wrote exactly 16 bytes?
    jne 1f
    add r12, 2                              // bit1: seek + in-place write OK

    // (2) seek back to 520 and read the 16 bytes through the SAME cap; they must equal the pattern
    mov rax, 15                             // SYS_SEEK back to 520
    mov rdi, rbx
    mov rsi, 520
    syscall
    cmp rax, 520
    jne 1f
    mov rax, 12                             // SYS_READ
    mov rdi, rbx
    mov rsi, r14                            // dest buf (data page)
    mov rdx, 16
    syscall
    cmp rax, 16                             // exactly 16 bytes back?
    jne 1f
    mov rax, [r14]                          // two 8-byte compares: read-back == the pattern we wrote
    cmp rax, [r15]
    jne 1f
    mov rax, [r14 + 8]
    cmp rax, [r15 + 8]
    jne 1f
    add r12, 4                              // bit2: read-back matches the written pattern
1:
    // (3) an RO-opened File (mode=0, CAP_READ only) written to must be denied -> -EACCES (the rights CHECK)
    mov rax, 11                             // SYS_OPEN SCRATCH.BIN RO
    lea rdi, [rip + unaos_user_u9x_name]
    mov rsi, [rip + unaos_user_u9x_namelen]
    xor edx, edx                            // mode = RO
    syscall
    mov r13, rax                            // r13 = RO handle
    test r13, r13
    js 2f                                   // RO open failed -> skip bit3
    mov rax, 1                              // SYS_WRITE through the RO handle
    mov rdi, r13
    mov rsi, r15
    mov rdx, 16
    syscall
    cmp rax, -13                            // exactly -EACCES ?
    jne 2f
    add r12, 8                              // bit3: RO-open File write -> -EACCES
2:
    // (4) a non-File handle (a Socket carrying CAP_WRITE, pre-endowed at index 2) -> -EACCES (the kind CHECK)
    mov rax, 1                              // SYS_WRITE
    mov rdi, 2                              // U9X_SOCK_IDX
    mov rsi, r15
    mov rdx, 16
    syscall
    cmp rax, -13
    jne 3f
    add r12, 16                             // bit4: wrong-kind handle write -> -EACCES
3:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U9X_WITNESS
    mov rdi, r12
    syscall
4:  jmp 4b                                  // sys_exit never returns; belt-and-braces guard

    .balign 8
unaos_user_u9x_namelen:
    .quad unaos_user_u9x_name_end - unaos_user_u9x_name
unaos_user_u9x_name:
    .ascii "SCRATCH.BIN"
unaos_user_u9x_name_end:
    .balign 8
unaos_user_u9x_pattern:
    .ascii "U9x-WRITE-OK-123"
    .globl unaos_user_u9x_blob_end
unaos_user_u9x_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u9x_blob_start: u8;
    static unaos_user_u9x_blob_end: u8;
    static unaos_user_u9x_write: u8;
}

// --- U11x ring-3 fixture (open-file LIFECYCLE: SYS_CLOSE + generation-tagged file-ids). ONE fixture
// (`u11x-close`) exercising SYS_CLOSE end-to-end over the immutable staged SCRATCH.BIN (all 0xEE): open RO + read
// the 16-byte seed; SYS_CLOSE -> 0; double-close -> -EBADF; a read through the now-closed handle -> -EACCES; then
// REOPEN (a fresh handle first-fit-reusing the freed descriptor slot) + read the seed again. It builds a 5-bit
// witness bitmask (see `U11X_WITNESS_ALL`) and conveys it as its `sys_exit` STATUS, routed BY NAME into
// `U11X_WITNESS` (x86 has no SYS_REPORT — the u5x/u6bx/u8x/u9x idiom). Register-only apart from the read-back dest
// (the DATA page, window page 1) — writes NO user stack, so it is safe on any slot under preemption. The fixture
// reads offset 0 of SCRATCH.BIN's STAGED seed (always 0xEE, independent of any prior U9x disk write at offset
// 520). It cannot reach the gen-rebind gap from ring 3 (no way to hold a stale file-id across a free), so the
// no-rebind proof is kernel-side (`u11x_check_gen_rebind`). ABI (Linux-style): rax = number, args rdi/rsi/rdx,
// return in rax; rcx/r11 are the SYSCALL-clobbered pair, so state rides r12-r15/rbx.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u11x_blob_start
unaos_user_u11x_blob_start:
    .balign 16
    .globl unaos_user_u11x_close
unaos_user_u11x_close:
    xor r12d, r12d                          // witness bitmask = 0 (survives syscalls)
    lea r14, [rip + unaos_user_u11x_blob_start] // r14 = window base (blob runs at the code-page base)
    add r14, 0x1000                         // r14 -> read-back dest (writable DATA page, window page 1)
    lea r15, [rip + unaos_user_u11x_pattern] // r15 -> the 16-byte expected seed (0xEE x16; RO code page)

    // (0) open SCRATCH.BIN RO -> hA; read 16 bytes; they must equal the staged 0xEE seed
    mov rax, 11                             // SYS_OPEN
    lea rdi, [rip + unaos_user_u11x_name]   // name ptr (RO code page — ring-3 readable)
    mov rsi, [rip + unaos_user_u11x_namelen] // name len ("SCRATCH.BIN" = 11)
    xor edx, edx                            // mode = RO
    syscall
    mov rbx, rax                            // rbx = hA (>= 0) or -errno
    test rbx, rbx
    js 9f                                   // open failed -> exit with the witness so far
    mov rax, 12                             // SYS_READ(hA, dest, 16)
    mov rdi, rbx
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 1f                                  // short read -> skip bit0, still exercise the close path
    mov rax, [r14]                          // two 8-byte compares: read-back == the 0xEE seed
    cmp rax, [r15]
    jne 1f
    mov rax, [r14 + 8]
    cmp rax, [r15 + 8]
    jne 1f
    add r12, 1                              // bit0: open RO + read matches the seed
1:
    // (1) SYS_CLOSE(hA) -> 0
    mov rax, 17                             // SYS_CLOSE
    mov rdi, rbx
    syscall
    test rax, rax                           // == 0 ?
    jnz 2f
    add r12, 2                              // bit1: close OK
2:
    // (2) SYS_CLOSE(hA) AGAIN -> -EBADF (double-close must be clean)
    mov rax, 17
    mov rdi, rbx
    syscall
    cmp rax, -9                             // -EBADF ?
    jne 3f
    add r12, 4                              // bit2: double-close -> -EBADF
3:
    // (3) a SYS_READ through the now-closed hA -> -EACCES (use-after-close denied; the handle word was cleared)
    mov rax, 12
    mov rdi, rbx
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, -13                            // -EACCES ?
    jne 4f
    add r12, 8                              // bit3: use-after-close -> -EACCES
4:
    // (4) reopen SCRATCH.BIN RO -> hB (a fresh handle reusing the freed slot); read 16 -> equals the seed again
    mov rax, 11                             // SYS_OPEN SCRATCH.BIN RO
    lea rdi, [rip + unaos_user_u11x_name]
    mov rsi, [rip + unaos_user_u11x_namelen]
    xor edx, edx
    syscall
    mov r13, rax                            // r13 = hB
    test r13, r13
    js 9f
    mov rax, 12                             // SYS_READ(hB, dest, 16)
    mov rdi, r13
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 9f
    mov rax, [r14]
    cmp rax, [r15]
    jne 9f
    mov rax, [r14 + 8]
    cmp rax, [r15 + 8]
    jne 9f
    add r12, 16                             // bit4: reopen + read round-trip matches the seed
9:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U11X_WITNESS
    mov rdi, r12
    syscall
8:  jmp 8b                                  // sys_exit never returns; belt-and-braces guard

    .balign 8
unaos_user_u11x_namelen:
    .quad unaos_user_u11x_name_end - unaos_user_u11x_name
unaos_user_u11x_name:
    .ascii "SCRATCH.BIN"
unaos_user_u11x_name_end:
    .balign 8
unaos_user_u11x_pattern:
    .byte 0xEE, 0xEE, 0xEE, 0xEE, 0xEE, 0xEE, 0xEE, 0xEE
    .byte 0xEE, 0xEE, 0xEE, 0xEE, 0xEE, 0xEE, 0xEE, 0xEE
    .globl unaos_user_u11x_blob_end
unaos_user_u11x_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u11x_blob_start: u8;
    static unaos_user_u11x_blob_end: u8;
    static unaos_user_u11x_close: u8;
}

// --- U10 GROW ring-3 fixture (real file GROWTH — the aarch64 `__u10_prog_grow` twin). ONE fixture
// (`u10x-grow`): opens the planted GROW.BIN (512 × 0xC1, one cluster) RW, seeks to EOF (512 == the cluster
// boundary), appends a 16-byte pattern PAST EOF (a real grow — the growable descriptor's `sys_write_grow`
// extends its wstage in memory; the disk alloc + FAT chain + dir-size bump defer to the launcher's IF=1 drain),
// reads the appended bytes back through the SAME cap, re-reads offset 0 to prove the original cluster survived,
// and proves an RO-opened File write is `-EACCES` (growth rides the SAME single CAP_WRITE CHECK as in-place
// write). Register-only apart from the read-back dest (the DATA page, window page 1) — no user stack write, so
// it is preemption-safe on any slot. 5-bit witness (`U10X_WITNESS_ALL`) conveyed as its `sys_exit` status,
// routed BY NAME into `U10X_WITNESS`. ABI (Linux-style): rax = number, args rdi/rsi/rdx, return rax.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u10x_blob_start
unaos_user_u10x_blob_start:
    .balign 16
    .globl unaos_user_u10x_grow
unaos_user_u10x_grow:
    xor r12d, r12d                          // witness bitmask = 0 (survives syscalls)
    lea r14, [rip + unaos_user_u10x_blob_start]
    add r14, 0x1000                         // r14 -> read-back dest (writable DATA page, window page 1)
    lea r15, [rip + unaos_user_u10x_pattern] // r15 -> the 16-byte append pattern (also the compare source)

    // (0) SYS_OPEN("GROW.BIN", RW) -> a File handle carrying CAP_READ|CAP_WRITE
    mov rax, 11                             // SYS_OPEN
    lea rdi, [rip + unaos_user_u10x_name]
    mov rsi, [rip + unaos_user_u10x_namelen]
    mov rdx, 1                              // mode = RW
    syscall
    mov rbx, rax                            // rbx = RW handle (>= 0) or -errno
    test rbx, rbx
    js 1f
    add r12, 1                              // bit0: open RW OK

    // (1) seek to EOF (512) and append the 16-byte pattern PAST it -> a REAL grow returns 16 (not a clamp-to-0)
    mov rax, 15                             // SYS_SEEK
    mov rdi, rbx
    mov rsi, 512                            // U10_GROW_OFFSET (== planted EOF == cluster boundary)
    syscall
    cmp rax, 512
    jne 1f
    mov rax, 1                              // SYS_WRITE (File + CAP_WRITE, past EOF -> grow)
    mov rdi, rbx
    mov rsi, r15
    mov rdx, 16
    syscall
    cmp rax, 16                             // grew by exactly 16 bytes?
    jne 1f
    add r12, 2                              // bit1: grow write OK

    // (2) seek back to 512 and read the 16 appended bytes through the SAME cap -> must equal the pattern
    mov rax, 15
    mov rdi, rbx
    mov rsi, 512
    syscall
    cmp rax, 512
    jne 1f
    mov rax, 12                             // SYS_READ
    mov rdi, rbx
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 1f
    mov rax, [r14]                          // two 8-byte compares: read-back == the appended pattern
    cmp rax, [r15]
    jne 1f
    mov rax, [r14 + 8]
    cmp rax, [r15 + 8]
    jne 1f
    add r12, 4                              // bit2: appended bytes read back through the same cap
1:
    // (3) seek to 0 and read the ORIGINAL first cluster -> must still be 0xC1 filler (the grow didn't corrupt it)
    mov rax, 15
    mov rdi, rbx
    xor esi, esi                            // offset 0
    syscall
    cmp rax, 0
    jne 2f
    mov rax, 12                             // SYS_READ
    mov rdi, rbx
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 2f
    lea rcx, [rip + unaos_user_u10x_filler] // rcx loaded AFTER the syscall (syscall clobbers rcx)
    mov rax, [r14]
    cmp rax, [rcx]
    jne 2f
    mov rax, [r14 + 8]
    cmp rax, [rcx + 8]
    jne 2f
    add r12, 8                              // bit3: original first cluster intact (0xC1 filler)
2:
    // (4) an RO-opened File written to -> -EACCES (the CAP_WRITE rights CHECK; growth rides the SAME check)
    mov rax, 11                             // SYS_OPEN GROW.BIN RO
    lea rdi, [rip + unaos_user_u10x_name]
    mov rsi, [rip + unaos_user_u10x_namelen]
    xor edx, edx                            // mode = RO
    syscall
    mov r13, rax
    test r13, r13
    js 3f
    mov rax, 1                              // SYS_WRITE through the RO handle
    mov rdi, r13
    mov rsi, r15
    mov rdx, 16
    syscall
    cmp rax, -13                            // exactly -EACCES ?
    jne 3f
    add r12, 16                             // bit4: RO-open File write -> -EACCES
3:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U10X_WITNESS
    mov rdi, r12
    syscall
4:  jmp 4b                                  // sys_exit never returns; belt-and-braces guard

    .balign 8
unaos_user_u10x_namelen:
    .quad unaos_user_u10x_name_end - unaos_user_u10x_name
unaos_user_u10x_name:
    .ascii "GROW.BIN"
unaos_user_u10x_name_end:
    .balign 8
unaos_user_u10x_pattern:
    .ascii "U10x-GROW-OK-678"
    .balign 8
unaos_user_u10x_filler:
    .byte 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1
    .byte 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1
    .globl unaos_user_u10x_blob_end
unaos_user_u10x_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u10x_blob_start: u8;
    static unaos_user_u10x_blob_end: u8;
    static unaos_user_u10x_grow: u8;
}

// --- U10 CREATE ring-3 fixture (create-from-nothing — the aarch64 `__u10c_prog_create` twin). ONE fixture
// (`u10cx-create`): O_CREAT|RW-opens FRESH.BIN (absent from the staged set — the kernel creates an in-memory
// descriptor; the real dir entry + first-cluster alloc defer to the launcher's IF=1 drain), writes a 16-byte
// pattern at offset 0 (a grow-from-empty), reads it back through the SAME cap, and re-opens the same name
// O_CREAT|RW (idempotent create-if-present -> a second handle). 4-bit witness (`U10CX_WITNESS_ALL`) conveyed as
// its `sys_exit` status, routed BY NAME into `U10CX_WITNESS`. Register-only apart from the read-back dest.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u10cx_blob_start
unaos_user_u10cx_blob_start:
    .balign 16
    .globl unaos_user_u10cx_create
unaos_user_u10cx_create:
    xor r12d, r12d                          // witness bitmask = 0
    lea r14, [rip + unaos_user_u10cx_blob_start]
    add r14, 0x1000                         // r14 -> read-back dest (writable DATA page)
    lea r15, [rip + unaos_user_u10cx_pattern] // r15 -> the 16-byte pattern (also the compare source)

    // (0) SYS_OPEN("FRESH.BIN", O_CREAT|RW=3) -> creates the file, a File handle carrying CAP_READ|CAP_WRITE
    mov rax, 11                             // SYS_OPEN
    lea rdi, [rip + unaos_user_u10cx_name]
    mov rsi, [rip + unaos_user_u10cx_namelen]
    mov rdx, 3                              // mode = O_CREAT | RW
    syscall
    mov rbx, rax
    test rbx, rbx
    js 1f
    add r12, 1                              // bit0: O_CREAT|RW open OK (created)

    // (1) write the 16-byte pattern at offset 0 (past EOF=0) -> grow-from-empty returns 16
    mov rax, 1                              // SYS_WRITE
    mov rdi, rbx
    mov rsi, r15
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 1f
    add r12, 2                              // bit1: write-from-empty OK

    // (2) seek back to 0 and read the 16 bytes through the SAME cap -> must equal the pattern
    mov rax, 15                             // SYS_SEEK
    mov rdi, rbx
    xor esi, esi                            // offset 0
    syscall
    cmp rax, 0
    jne 1f
    mov rax, 12                             // SYS_READ
    mov rdi, rbx
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 1f
    mov rax, [r14]
    cmp rax, [r15]
    jne 1f
    mov rax, [r14 + 8]
    cmp rax, [r15 + 8]
    jne 1f
    add r12, 4                              // bit2: read-back matches the written pattern
1:
    // (3) a SECOND O_CREAT|RW open of the same name -> a handle (idempotent create-if-present, no duplicate)
    mov rax, 11
    lea rdi, [rip + unaos_user_u10cx_name]
    mov rsi, [rip + unaos_user_u10cx_namelen]
    mov rdx, 3                              // O_CREAT | RW
    syscall
    test rax, rax
    js 2f
    add r12, 8                              // bit3: idempotent second open OK
2:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U10CX_WITNESS
    mov rdi, r12
    syscall
3:  jmp 3b

    .balign 8
unaos_user_u10cx_namelen:
    .quad unaos_user_u10cx_name_end - unaos_user_u10cx_name
unaos_user_u10cx_name:
    .ascii "FRESH.BIN"
unaos_user_u10cx_name_end:
    .balign 8
unaos_user_u10cx_pattern:
    .ascii "U10x-CREATE-OK99"
    .globl unaos_user_u10cx_blob_end
unaos_user_u10cx_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u10cx_blob_start: u8;
    static unaos_user_u10cx_blob_end: u8;
    static unaos_user_u10cx_create: u8;
}

// --- U10 DELETE ring-3 fixture (create -> write -> unlink — the aarch64 `__u10d_prog_delete` twin). ONE fixture
// (`u10dx-delete`): O_CREAT|RW-opens DELME.BIN, writes a 16-byte pattern (so the file owns real data), opens a
// SECOND (sibling) RW handle, SYS_UNLINKs via the first (name gone + every descriptor invalidated + the on-disk
// delete enqueued), then proves the sibling read is `-EACCES` (invalidated) and a plain RO re-open is `-ENOENT`
// (gone). 5-bit witness (`U10DX_WITNESS_ALL`) as its `sys_exit` status, routed BY NAME into `U10DX_WITNESS`.
// Register-only apart from the read-back dest. Callee-saved regs (rbx/r13) hold the two handles across syscalls.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u10dx_blob_start
unaos_user_u10dx_blob_start:
    .balign 16
    .globl unaos_user_u10dx_delete
unaos_user_u10dx_delete:
    xor r12d, r12d                          // witness bitmask = 0
    lea r14, [rip + unaos_user_u10dx_blob_start]
    add r14, 0x1000                         // r14 -> read-back dest (writable DATA page)
    lea r15, [rip + unaos_user_u10dx_pattern] // r15 -> the 16-byte pattern

    // (0) SYS_OPEN("DELME.BIN", O_CREAT|RW=3) -> creates it, a File handle carrying CAP_READ|CAP_WRITE
    mov rax, 11
    lea rdi, [rip + unaos_user_u10dx_name]
    mov rsi, [rip + unaos_user_u10dx_namelen]
    mov rdx, 3                              // O_CREAT | RW
    syscall
    mov rbx, rax                            // rbx = primary handle (survives syscalls)
    test rbx, rbx
    js 3f
    add r12, 1                              // bit0: create+open OK

    // (1) write the 16-byte pattern -> grow-from-empty allocates the file's one data cluster
    mov rax, 1
    mov rdi, rbx
    mov rsi, r15
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 3f
    add r12, 2                              // bit1: write OK

    // sibling: a SECOND RW open (no O_CREAT — the file exists) -> h1, held in r13 across syscalls
    mov rax, 11
    lea rdi, [rip + unaos_user_u10dx_name]
    mov rsi, [rip + unaos_user_u10dx_namelen]
    mov rdx, 1                              // RW
    syscall
    mov r13, rax                            // r13 = sibling handle
    test r13, r13
    js 3f                                   // no sibling -> cannot prove bit3; bail

    // (2) SYS_UNLINK via the primary -> 0 (name gone + all descriptors invalidated + on-disk delete enqueued)
    mov rax, 16                             // SYS_UNLINK
    mov rdi, rbx
    syscall
    cmp rax, 0
    jne 3f
    add r12, 4                              // bit2: unlink OK

    // (3) a read through the now-invalidated SIBLING -> -EACCES (no stale reference to the file)
    mov rax, 12                             // SYS_READ
    mov rdi, r13
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, -13                            // -EACCES ?
    jne 3f
    add r12, 8                              // bit3: sibling invalidated

    // (4) a plain RO re-open of the deleted name -> -ENOENT (the file is gone)
    mov rax, 11
    lea rdi, [rip + unaos_user_u10dx_name]
    mov rsi, [rip + unaos_user_u10dx_namelen]
    xor edx, edx                            // RO, no O_CREAT
    syscall
    cmp rax, -2                             // -ENOENT ?
    jne 3f
    add r12, 16                             // bit4: re-open is gone
3:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U10DX_WITNESS
    mov rdi, r12
    syscall
4:  jmp 4b

    .balign 8
unaos_user_u10dx_namelen:
    .quad unaos_user_u10dx_name_end - unaos_user_u10dx_name
unaos_user_u10dx_name:
    .ascii "DELME.BIN"
unaos_user_u10dx_name_end:
    .balign 8
unaos_user_u10dx_pattern:
    .ascii "U10x-DELETE-OK42"
    .globl unaos_user_u10dx_blob_end
unaos_user_u10dx_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u10dx_blob_start: u8;
    static unaos_user_u10dx_blob_end: u8;
    static unaos_user_u10dx_delete: u8;
}

// --- U11x M2 ring-3 fixture (the aarch64 `el0-u11defer-b` twin): the OTHER-process actor against a file the
// LAUNCHER created + holds open on a scratch row. ONE fixture (`u11m2-unlink`): plain-RW-opens DEFER.BIN (a
// CROSS-PROCESS sibling open of another row's created file — the new U11x M2 capability), read-verifies the
// launcher's pattern (first 8 bytes), opens a second (sibling) handle, SYS_UNLINKs via the primary (the name
// vanishes GLOBALLY; the on-disk delete op is enqueued HELD — the launcher's row still holds the file open, so
// the free is DEFERRED), then proves: the sibling read is `-EACCES` (invalidated), a plain re-open is `-ENOENT`
// (gone for every process), and an O_CREAT re-create is `-EBUSY` (the delete has not completed). 6-bit witness
// (`U11M2_WITNESS_ALL`) as its `sys_exit` status, routed BY NAME into `U11M2_WITNESS`. Register-only apart from
// the read-back dest. Callee-saved regs (rbx/r13) hold the two handles across syscalls.
core::arch::global_asm!(
    r#"
    .globl unaos_user_u11m2_blob_start
unaos_user_u11m2_blob_start:
    .balign 16
    .globl unaos_user_u11m2_unlink
unaos_user_u11m2_unlink:
    xor r12d, r12d                          // witness bitmask = 0
    lea r14, [rip + unaos_user_u11m2_blob_start]
    add r14, 0x1000                         // r14 -> read-back dest (writable DATA page)

    // (0) SYS_OPEN("DEFER.BIN", RW=1, NO O_CREAT) -> a cross-process sibling of the launcher's created file
    mov rax, 11
    lea rdi, [rip + unaos_user_u11m2_name]
    mov rsi, [rip + unaos_user_u11m2_namelen]
    mov rdx, 1                              // RW, plain open (the file must already exist ACROSS rows)
    syscall
    mov rbx, rax                            // rbx = primary handle
    test rbx, rbx
    js 3f
    add r12, 1                              // bit0: cross-process open OK

    // (1) read 16 bytes and verify the first 8 against the launcher's pattern (content crossed processes)
    mov rax, 12                             // SYS_READ
    mov rdi, rbx
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 3f
    mov r8, [rip + unaos_user_u11m2_pattern]
    cmp r8, [r14]
    jne 3f
    add r12, 2                              // bit1: read-back matches the other process's bytes

    // sibling: a SECOND RW open -> r13 (for the invalidation negative after the unlink)
    mov rax, 11
    lea rdi, [rip + unaos_user_u11m2_name]
    mov rsi, [rip + unaos_user_u11m2_namelen]
    mov rdx, 1                              // RW
    syscall
    mov r13, rax
    test r13, r13
    js 3f                                   // no sibling -> cannot prove bit3; bail

    // (2) SYS_UNLINK via the primary -> 0 (name gone globally; the delete DEFERS — the launcher still holds it)
    mov rax, 16                             // SYS_UNLINK
    mov rdi, rbx
    syscall
    cmp rax, 0
    jne 3f
    add r12, 4                              // bit2: unlink OK

    // (3) a read through the now-invalidated SIBLING -> -EACCES (no stale reference)
    mov rax, 12                             // SYS_READ
    mov rdi, r13
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, -13                            // -EACCES ?
    jne 3f
    add r12, 8                              // bit3: sibling invalidated

    // (4) a plain RO re-open of the unlinked name -> -ENOENT (gone for EVERY process, launcher's open or not)
    mov rax, 11
    lea rdi, [rip + unaos_user_u11m2_name]
    mov rsi, [rip + unaos_user_u11m2_namelen]
    xor edx, edx                            // RO, no O_CREAT
    syscall
    cmp rax, -2                             // -ENOENT ?
    jne 3f
    add r12, 16                             // bit4: re-open is gone

    // (5) an O_CREAT|RW re-create of the unlinked name -> -EBUSY (its deferred delete has not completed)
    mov rax, 11
    lea rdi, [rip + unaos_user_u11m2_name]
    mov rsi, [rip + unaos_user_u11m2_namelen]
    mov rdx, 3                              // O_CREAT | RW
    syscall
    cmp rax, -16                            // -EBUSY ?
    jne 3f
    add r12, 32                             // bit5: re-create refused while delete-pending
3:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U11M2_WITNESS
    mov rdi, r12
    syscall
4:  jmp 4b

    .balign 8
unaos_user_u11m2_namelen:
    .quad unaos_user_u11m2_name_end - unaos_user_u11m2_name
unaos_user_u11m2_name:
    .ascii "DEFER.BIN"
unaos_user_u11m2_name_end:
    .balign 8
unaos_user_u11m2_pattern:
    .ascii "U11x-DEFER-OK-42"
    .globl unaos_user_u11m2_blob_end
unaos_user_u11m2_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u11m2_blob_start: u8;
    static unaos_user_u11m2_blob_end: u8;
    static unaos_user_u11m2_unlink: u8;
}

// --- U6x owner/grants ring-3 fixtures (the aarch64 `el0-uowner-a`/`el0-uowner-b` twins): TWO EL0 programs in
// ONE blob — process A (`u6gx-owner`) creates OWNED.BIN PRIVATE, and process B (`u6gx-grantee`, a DIFFERENT
// address space) is denied by default, granted, exercised, then revoked. The launcher (`u6gx_launcher`)
// choreographs them with a single GO word (launcher -> fixture: the next step the fixture may proceed to) and a
// single SIG word (fixture -> launcher: the last step it completed) planted per-slot at the fixed window
// offsets below; A is pre-endowed with a `Child` handle naming B so its SYS_FGRANT is owner-scoped (the SYS_XFER
// idiom — B is never named by a raw pid). Both convey a witness bitmask as their `sys_exit` status, routed BY
// NAME into `U6GX_OWNER_WITNESS` / `U6GX_GRANTEE_WITNESS` (x86 has no SYS_REPORT — the u5x/u7x idiom).
//
// A (`u6gx-owner`, U6GX_OWNER_ALL = 0x3F): bit0 create OWNED.BIN PRIVATE (O_CREAT|RW, no O_PUBLIC -> A owns it) +
// write the 16-byte pattern; bit1 SYS_FGRANT B read+write -> 0; bit2 SYS_FGRANT B revoke (rights 0) -> 0; bit3
// the owner RE-opens its own file after the revoke -> a handle (owner authority persists); bit4 the owner
// UNLINKs OWNED.BIN while B STILL HOLDS it open -> 0 (F1 admits the owner; the delete DEFERS — B holds a
// refcount); bit5 an O_CREAT re-create of the just-unlinked name -> -EBUSY (its deferred delete has not
// completed — the U11x M2 combined path). A exits; the kernel-side verdict proves ownership then dies at B's
// last close (re-creatable again).
//
// B (`u6gx-grantee`, U6GX_GRANTEE_ALL = 0x1F): bit0 open OWNED.BIN BEFORE any grant -> -EACCES (a non-owner is
// denied BY NAME — the gap U6x closes); bit1 open RW AFTER the grant -> a handle + the read-back matches A's
// pattern (content proves the grant admitted it); bit2 B (a non-owner) tries SYS_FGRANT -> -EACCES (only the
// owner may grant); bit3 B tries SYS_UNLINK via its CAP_WRITE handle -> -EACCES (F1 — a content grantee cannot
// delete/steal ownership); bit4 open AFTER the revoke -> -EACCES (re-denied). B KEEPS its granted handle open
// across A's revoke + unlink (a handle already held survives a revoke — the ACL gates ACQUISITION), then closes
// it at the final step (releasing A's deferred delete).
core::arch::global_asm!(
    r#"
    .globl unaos_user_u6gx_blob_start
unaos_user_u6gx_blob_start:
    .balign 16

    // ---- Process A: the OWNER ----
    .globl unaos_user_u6gx_owner
unaos_user_u6gx_owner:
    xor r12d, r12d                          // witness = 0
    lea r15, [rip + unaos_user_u6gx_blob_start] // r15 = window base (== USER_BASE; GO/SIG live in a data page)
    lea r14, [r15 + 0x1000]                 // r14 -> a scratch DATA page (unused by A; kept for symmetry)

    // (0) create OWNED.BIN PRIVATE (O_CREAT|RW = 3, NO O_PUBLIC -> A becomes owner) -> hA in rbx
    mov rax, 11                             // SYS_OPEN
    lea rdi, [rip + unaos_user_u6gx_name]
    mov rsi, [rip + unaos_user_u6gx_namelen]
    mov rdx, 3                              // O_CREAT | RW  (PRIVATE — A owns it)
    syscall
    mov rbx, rax
    test rbx, rbx
    js 9f
    // write the 16-byte pattern through hA (grows from empty)
    mov rax, 1                              // SYS_WRITE
    mov rdi, rbx
    lea rsi, [rip + unaos_user_u6gx_pattern]
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 9f
    add r12, 1                              // bit0: create PRIVATE + write OK (an owner open+write)
    mov qword ptr [r15 + 0x3808], 1         // SIG = 1 (A created the file — launcher releases B's pre-grant open)

    // park: wait GO >= 1 (released after B's pre-grant open was denied)
    mov rcx, 0x40000000
1:  cmp qword ptr [r15 + 0x3800], 1
    jae 2f
    pause
    dec rcx
    jnz 1b
    jmp 9f                                  // GO never released -> partial witness (verdict FAILs)

2:  // (1) grant B read+write: SYS_FGRANT(hA, child handle 2 -> B, CAP_READ|CAP_WRITE = 3) -> 0
    mov rax, 18                             // SYS_FGRANT
    mov rdi, rbx                            // the FILE handle (hA)
    mov rsi, 2                              // U6GX_CHILD_IDX — the Child handle naming B
    mov rdx, 3                              // CAP_READ | CAP_WRITE
    syscall
    cmp rax, 0
    jne 9f
    add r12, 2                              // bit1: grant returned 0
    mov qword ptr [r15 + 0x3808], 2         // SIG = 2 (launcher releases B's granted open)

    // park: wait GO >= 2
    mov rcx, 0x40000000
3:  cmp qword ptr [r15 + 0x3800], 2
    jae 4f
    pause
    dec rcx
    jnz 3b
    jmp 9f

4:  // (2) revoke B: SYS_FGRANT(hA, child 2, rights = 0) -> 0
    mov rax, 18
    mov rdi, rbx
    mov rsi, 2
    xor edx, edx                            // rights = 0 -> REVOKE
    syscall
    cmp rax, 0
    jne 9f
    add r12, 4                              // bit2: revoke returned 0
    mov qword ptr [r15 + 0x3808], 3         // SIG = 3 (launcher releases B's post-revoke denied open)

    // park: wait GO >= 3 (released after B's post-revoke open was denied; B still holds its granted handle)
    mov rcx, 0x40000000
5:  cmp qword ptr [r15 + 0x3800], 3
    jae 6f
    pause
    dec rcx
    jnz 5b
    jmp 9f

6:  // (3) the OWNER re-opens its OWN file after the revoke -> still admitted (ownership authority persists) -> r13
    mov rax, 11
    lea rdi, [rip + unaos_user_u6gx_name]
    mov rsi, [rip + unaos_user_u6gx_namelen]
    mov rdx, 1                              // RW
    syscall
    mov r13, rax
    test r13, r13
    js 9f
    add r12, 8                              // bit3: owner re-open OK

    // (4) the OWNER UNLINKs OWNED.BIN while B STILL HOLDS it open -> 0 (F1 admits the owner; the delete DEFERS)
    mov rax, 16                             // SYS_UNLINK
    mov rdi, rbx                            // via the primary owner handle
    syscall
    cmp rax, 0
    jne 9f
    add r12, 16                             // bit4: owner unlink OK (deferred — B holds a refcount)

    // (5) an O_CREAT re-create of the just-unlinked name -> -EBUSY (deferred delete not yet complete)
    mov rax, 11
    lea rdi, [rip + unaos_user_u6gx_name]
    mov rsi, [rip + unaos_user_u6gx_namelen]
    mov rdx, 3                              // O_CREAT | RW
    syscall
    cmp rax, -16                            // -EBUSY ?
    jne 9f
    add r12, 32                             // bit5: re-create refused while delete-pending
9:
    mov qword ptr [r15 + 0x3808], 4         // SIG = 4 (A is exiting — launcher releases B's final close)
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U6GX_OWNER_WITNESS
    mov rdi, r12
    syscall
99: jmp 99b

    // ---- Process B: the GRANTEE ----
    .balign 16
    .globl unaos_user_u6gx_grantee
unaos_user_u6gx_grantee:
    xor r12d, r12d                          // witness = 0
    lea r15, [rip + unaos_user_u6gx_blob_start] // r15 = window base (its OWN slot's window)
    lea r14, [r15 + 0x1000]                 // r14 -> read-back dest (writable DATA page)

    // park: wait GO >= 1 (released after A created OWNED.BIN — so the file exists before B opens it)
    mov rcx, 0x40000000
11: cmp qword ptr [r15 + 0x3800], 1
    jae 12f
    pause
    dec rcx
    jnz 11b
    jmp 19f

12: // (0) open OWNED.BIN RO BEFORE any grant -> -EACCES (a non-owner is denied BY NAME: the gap closed)
    mov rax, 11
    lea rdi, [rip + unaos_user_u6gx_name]
    mov rsi, [rip + unaos_user_u6gx_namelen]
    xor edx, edx                            // RO, no O_CREAT
    syscall
    cmp rax, -13                            // -EACCES ?
    jne 19f
    add r12, 1                              // bit0: non-owner open denied
    mov qword ptr [r15 + 0x3808], 1         // SIG = 1 (launcher tells A to grant)

    // park: wait GO >= 2 (released after A granted B)
    mov rcx, 0x40000000
13: cmp qword ptr [r15 + 0x3800], 2
    jae 14f
    pause
    dec rcx
    jnz 13b
    jmp 19f

14: // (1) open OWNED.BIN RW AFTER the grant -> a handle (>= 0) carrying CAP_READ|CAP_WRITE -> rbx (KEPT open)
    mov rax, 11
    lea rdi, [rip + unaos_user_u6gx_name]
    mov rsi, [rip + unaos_user_u6gx_namelen]
    mov rdx, 1                              // RW (subset of the granted R|W)
    syscall
    mov rbx, rax
    test rbx, rbx
    js 19f
    // read 16 bytes and verify the first 8 against A's pattern (content proves the grant admitted the open)
    mov rax, 12                             // SYS_READ
    mov rdi, rbx
    mov rsi, r14
    mov rdx, 16
    syscall
    cmp rax, 16
    jne 19f
    mov r8, [rip + unaos_user_u6gx_pattern]
    cmp r8, [r14]
    jne 19f
    add r12, 2                              // bit1: granted RW open + read-back matches A's bytes

    // (2) B (a non-owner) tries SYS_FGRANT -> -EACCES (only the owner may grant; no Child handle needed —
    //     ownership is checked FIRST, before the child argument is resolved)
    mov rax, 18                             // SYS_FGRANT
    mov rdi, rbx                            // its own File handle
    xor esi, esi                            // child handle 0 (irrelevant — the owner check fails first)
    mov rdx, 3
    syscall
    cmp rax, -13                            // -EACCES ?
    jne 19f
    add r12, 4                              // bit2: non-owner grant denied

    // (3) B tries SYS_UNLINK via its CAP_WRITE handle -> -EACCES (F1 — delete is owner-only; a content grantee
    //     must not be able to unlink + recreate to STEAL ownership)
    mov rax, 16                             // SYS_UNLINK
    mov rdi, rbx
    syscall
    cmp rax, -13                            // -EACCES ?
    jne 19f
    add r12, 8                              // bit3: grantee unlink denied (delete owner-only)
    mov qword ptr [r15 + 0x3808], 2         // SIG = 2 (launcher tells A to revoke) — B KEEPS rbx open

    // park: wait GO >= 3 (released after A revoked B)
    mov rcx, 0x40000000
15: cmp qword ptr [r15 + 0x3800], 3
    jae 16f
    pause
    dec rcx
    jnz 15b
    jmp 19f

16: // (4) open OWNED.BIN RW AFTER the revoke -> -EACCES (re-denied; the handle B ALREADY holds is unaffected)
    mov rax, 11
    lea rdi, [rip + unaos_user_u6gx_name]
    mov rsi, [rip + unaos_user_u6gx_namelen]
    mov rdx, 1                              // RW
    syscall
    cmp rax, -13                            // -EACCES ?
    jne 19f
    add r12, 16                             // bit4: post-revoke open re-denied
    mov qword ptr [r15 + 0x3808], 3         // SIG = 3 (B is done proving; launcher lets A unlink while B holds)

    // park: wait GO >= 4 (released after A unlinked OWNED.BIN — B still holds rbx, so the delete DEFERRED)
    mov rcx, 0x40000000
17: cmp qword ptr [r15 + 0x3800], 4
    jae 18f
    pause
    dec rcx
    jnz 17b
    jmp 19f

18: // close the held handle -> releases the LAST cross-process refcount, which drains A's deferred delete
    mov rax, 17                             // SYS_CLOSE
    mov rdi, rbx
    syscall
19:
    mov rax, 2                              // SYS_EXIT(witness) -> routed by name into U6GX_GRANTEE_WITNESS
    mov rdi, r12
    syscall
199: jmp 199b

    .balign 8
unaos_user_u6gx_namelen:
    .quad unaos_user_u6gx_name_end - unaos_user_u6gx_name
unaos_user_u6gx_name:
    .ascii "OWNED.BIN"
unaos_user_u6gx_name_end:
    .balign 8
unaos_user_u6gx_pattern:
    .ascii "U6x-OWNED-OK-777"
    .globl unaos_user_u6gx_blob_end
unaos_user_u6gx_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_u6gx_blob_start: u8;
    static unaos_user_u6gx_blob_end: u8;
    static unaos_user_u6gx_owner: u8;
    static unaos_user_u6gx_grantee: u8;
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
    // WINX-7: FIVE C arguments now, not four. `SYS_THREAD_SPAWN(entry, sp, arg, place)` is the first
    // four-argument verb on this arch, and the ring-3 side passes its fourth argument in `r10` —
    // which is what a userspace SysV caller would use for a fourth argument anyway, and is the
    // conventional choice precisely because `SYSCALL` itself destroys `rcx`. Move it to `r8` (the 5th
    // C argument register) BEFORE the shuffle below, so nothing in the shuffle can clobber it. `r8`
    // was previously untouched here and is scrubbed on the way out with the other caller-saved
    // registers, so the return half is unchanged.
    "mov r8, r10",                  // arg3 -> 5th C arg (SYS_THREAD_SPAWN's `place`; junk otherwise)
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
    // U9x: the File-WRITE fixture is register-only (its only user store is the read-back dest) and
    // well-behaved; a kill is a real U9x bug — its own counter, never the U1b `killed_unexpected` count. Not
    // in PROCS (the launcher spawned it, not sys_spawn), so no parent semaphore to post — the launcher times
    // out to FAIL on `U9X_DONE`.
    if name == "u9x-write" {
        U9X_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U11x: the open-file-lifecycle fixture is register-only (its only user store is the read-back dest) and
    // well-behaved; a kill is a real U11x bug — its own counter, never the U1b `killed_unexpected` count. Not in
    // PROCS (the launcher spawned it, not sys_spawn), so no parent semaphore to post — the launcher times out to
    // FAIL on `U11X_DONE`.
    if name == "u11x-close" {
        U11X_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U10 GROW: the growth fixture is register-only (its only user store is the read-back dest) and well-behaved;
    // a kill is a real U10 bug — its own counter, never the U1b `killed_unexpected` count. Not in PROCS, so no
    // parent semaphore to post — the launcher times out to FAIL on `U10X_DONE`.
    if name == "u10x-grow" {
        U10X_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U10 CREATE: the create fixture is register-only + well-behaved; a kill is a real U10 bug — its own counter.
    if name == "u10cx-create" {
        U10CX_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U10 DELETE: the delete fixture is register-only + well-behaved; a kill is a real U10 bug — its own counter.
    if name == "u10dx-delete" {
        U10DX_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U11x M2: the cross-process unlink fixture is register-only + well-behaved; a kill is a real bug — its own
    // counter (the launcher's unconditional release path still cleans up the held op).
    if name == "u11m2-unlink" {
        U11M2_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // WINX-2: an operator-launched `run`/`bg` program that faults. Unlike every fixture above, these run
    // ARBITRARY untrusted images, so a fault-kill here is NOT a kernel bug — it is the fault-kill net doing
    // its job on a bad program, and it must be reported to the operator, not counted as an unexpected kill.
    // The generic Proc short-circuit in the SYS_EXIT arm never runs for a killed task (it never reaches
    // SYS_EXIT), so record the kill status on its Proc row HERE and post `done`, so `run_user_image`'s wait
    // and `bg_poll`/`jobs` see a settled row instead of waiting out the full deadline on a dead task.
    if name == RUN_TASK_NAME || name == BG_TASK_NAME {
        let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
        if let Some(id) = crate::arch::sched::current_task_id(cpu) {
            if let Some(i) = proc_find_running(id) {
                PROCS[i].status.store(EXEC_KILLED_STATUS, Ordering::Release);
                PROCS[i].state.store(PEXITED, Ordering::Release);
                PROCS[i].done.post();
            }
        }
        return;
    }
    // WINX-1: the window fixture writes only its own data page and its own mapped surface, so a fault-kill
    // means a window verb mapped something wrong (or failed to map it) — a real WINX-1 bug, its own
    // counter, never the U1b `killed_unexpected` count. Not in PROCS, so the launcher times out to FAIL on
    // `WINX_DONE`.
    if name == "winx-app" {
        WINX_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // WINX-7: the threads/futex/input fixture and its worker threads all write only their own data
    // page and their own mapped surface, so a fault-kill means a thread verb handed ring 3 something
    // wrong — a real WINX-7 bug, on its own counter. The most likely shape it would take is the
    // refcount defect: an address space freed under a still-running sibling, which faults the survivor
    // rather than returning a wrong bit. Not in PROCS, so the launcher times out to FAIL.
    if name == EL0_THREAD_NAME || name == WINX7_TASK_NAME {
        WINX7_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // SOCK-2: the UDP round-trip fixture is register + inline-data only (its only user store is the recv
    // buffer in its own data page) and well-behaved; a kill is a real SOCK-2 bug — its own counter, never
    // the U1b `killed_unexpected` count. Not in PROCS, so the launcher times out to FAIL on `SOCK2_DONE`.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    if name == "sock2-udp" {
        SOCK2_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // SOCK-3: the TCP round-trip fixture (same well-behaved register + inline-data shape) — a kill is a
    // real SOCK-3 bug, its own counter. Not in PROCS, so the launcher times out to FAIL on `SOCK3_DONE`.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    if name == "sock3-tcp" {
        SOCK3_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // SOCK-4: the transferable-socket GRANTOR / GRANTEE fixtures are well-behaved (register + inline-data,
    // plus the grantee's recv-buffer + USED-word stores to its own RW pages) — a kill is a real SOCK-4 bug,
    // its own counter. Not in PROCS, so the launcher times out to FAIL on `SOCK4_DONE`.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    if name == "sock4-grantor" || name == "sock4-grantee" {
        SOCK4_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // SINKHOLE-1 (zeolite): the DNS resolver fixture is well-behaved (register + its own RW data pages —
    // the file/recv/response buffers). A kill is a real zeolite bug — its own counter, never the U1b
    // `killed_unexpected` count. Not in PROCS, so the launcher times out to FAIL on `ZEOLITE_DONE`.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    if name == "zeolite-resolver" {
        ZEOLITE_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U6x: the owner/grantee fixtures are well-behaved (register-only apart from the grantee's read-back store); a
    // kill is a real U6x bug — its own counter, never the U1b `killed_unexpected` count. The launcher's
    // unconditional cleanup still tears the fixtures' slots + Proc entries down.
    if name == "u6gx-owner" || name == "u6gx-grantee" {
        U6GX_KILLED.fetch_add(1, Ordering::AcqRel);
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
/// = number, rdi/rsi/rdx/r10 = args; the return value goes back in rax. A blocking/exiting syscall
/// may safely `switch_context` here — exactly like `timer_preempt` from the timer ISR.
///
/// WINX-7: `a3` (from ring-3 `r10`) is the fourth argument, used only by `SYS_THREAD_SPAWN`. Every
/// other arm ignores it, and a program that does not load `r10` simply passes junk to a verb that
/// does not read it.
#[unsafe(no_mangle)]
extern "C" fn syscall_dispatch(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    if !SYSCALL_LOGGED.swap(true, Ordering::Relaxed) {
        serial_println!(":: SYSCALL: nr={} — ring-3 -> ring-0 path live ::", nr);
    }
    let rc = syscall_dispatch_inner(nr, a0, a1, a2, a3);
    // TEARDOWN-1: the SYSCALL KILL BOUNDARY. A task whose `KillSwitch` is armed retires HERE, on the way
    // out, and this call does not return in that case — the scheduler's existing reap arm owns the
    // teardown (see `sched::kill_check_current`).
    //
    // This is the boundary that makes `bg_kill` honest for a real application. Before it, the only
    // delivery point was `run()`'s READY arm, i.e. a PREEMPTION — so a program whose every turn ends in a
    // blocking syscall (one `SYS_SLEEP_MS` per frame is the shape every windowed app has, and is exactly
    // what `STAT.ELF` does) went from dispatch straight back into a park without ever passing through it,
    // and its kill stayed armed until a timer preemption happened to land inside its short compute
    // window. The WINX-2 kill leg timed out on precisely that.
    //
    // Placed at the TAIL, after the handler has returned and every guard it took has been dropped: the
    // dispatcher's own contract already states that a syscall may `switch_context` from here, and the
    // abandoned syscall frame on the kernel stack is freed with the stack, exactly as the preemption
    // arm's abandoned interrupt frame is.
    crate::arch::sched::kill_check_current();
    rc
}

/// TEARDOWN-1: the dispatch table proper, split out of [`syscall_dispatch`] so the kill boundary above
/// runs on the way out of EVERY verb rather than being re-stated (and forgettable) at each arm.
fn syscall_dispatch_inner(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    match nr {
        SYS_WRITE => sys_write(a0, a1, a2),
        // WINX-1: the process verbs, at their cross-arch shared numbers. Unconditional (no feature
        // gate) — they are core process surface, like WRITE/EXIT, not an optional subsystem.
        SYS_YIELD => sys_yield(),
        SYS_SLEEP_MS => sys_sleep_ms(a0),
        SYS_GETINFO => sys_getinfo(a0),
        // WINX-1: the window verbs. Unconditional, like the process verbs — `video::wm` is arch-neutral
        // and always compiled; the `wc` feature only controls whether the compositor is ACTIVATED on the
        // x86 panel, and a refused `wm::create` degrades to a surface with no compositor row.
        SYS_WIN_CREATE => sys_win_create(a0, a1),
        SYS_WIN_PRESENT => sys_win_present(a0),
        // WINX-7: threads, futex and input, at their shared cross-arch numbers. Unconditional (no
        // feature gate), for the same reason the process and window verbs are: they are core EL0
        // surface that a windowed application is written against, not an optional subsystem. A
        // program that never calls them pays nothing — an unused match arm is not a cost.
        //
        // `SYS_THREAD_SPAWN` is the first FOUR-argument verb on this arch (entry, sp, arg, place),
        // which is why `unaos_syscall_entry` now shuffles a fourth register — see the stub. Every
        // other verb ignores `a3`.
        SYS_THREAD_SPAWN => sys_thread_spawn(a0, a1, a2, a3),
        SYS_THREAD_JOIN => sys_thread_join(a0),
        SYS_THREAD_EXIT => sys_thread_exit(), // never returns
        SYS_FUTEX => sys_futex(a0, a1, a2),
        SYS_INPUT_POLL => sys_input_poll(),
        SYS_SPAWN => sys_spawn(),
        SYS_WAIT => sys_wait(a0),
        SYS_CAP => sys_cap(a0, a1, a2),
        SYS_OPEN => sys_open(a0, a1, a2),
        SYS_READ => sys_read(a0, a1, a2),
        SYS_XFER => sys_xfer(a0, a1, a2),
        SYS_RECV => sys_recv(),
        SYS_SEEK => sys_seek(a0, a1),
        SYS_UNLINK => sys_unlink(a0),
        SYS_CLOSE => sys_close(a0),
        SYS_FGRANT => sys_fgrant(a0, a1, a2),
        // SOCK-2: the UDP socket family (x86-only, knob-on). Knob-off / aarch64 never emit these arms,
        // so the dispatch match is byte-identical there and an unknown number falls to the default.
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_SOCKET => sys_socket(a0, a1, a2),
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_BIND => sys_bind(a0, a1),
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_SENDTO => sys_sendto(a0, a1, a2),
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_RECVFROM => sys_recvfrom(a0, a1, a2),
        // SOCK-3: the TCP client socket family (x86-only, knob-on). Same gating as SOCK-2's arms.
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_CONNECT => sys_connect(a0, a1, a2),
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_SEND => sys_send(a0, a1, a2),
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_SOCK_RECV => sys_sock_recv(a0, a1, a2),
        // SOCK-6: the TCP server socket family (x86-only, knob-on). Same gating as the client arms.
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_LISTEN => sys_listen(a0, a1),
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        SYS_ACCEPT => sys_accept(a0),
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
                // U6x owner/grants: the GRANTEE has a launcher-PLANTED Proc entry (A's Child handle names it,
                // owner-scoped), so — exactly like the u7x child — its exit must be routed BY NAME BEFORE the
                // Proc reap short-circuit below, or its witness (and `U6GX_DONE`) would be swallowed by the
                // parent-reap path. The OWNER has no Proc entry but rides the same arm for symmetry. Mark the
                // planted entry EXITED (a late SYS_FGRANT to this recipient then fails the RUNNING check).
                if nm == Some("u6gx-owner") || nm == Some("u6gx-grantee") {
                    if nm == Some("u6gx-owner") {
                        U6GX_OWNER_WITNESS.store(a0 as u32, Ordering::Release);
                    } else {
                        U6GX_GRANTEE_WITNESS.store(a0 as u32, Ordering::Release);
                    }
                    U6GX_DONE.fetch_add(1, Ordering::AcqRel);
                    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
                    if let Some(id) = crate::arch::sched::current_task_id(cpu) {
                        if let Some(i) = proc_find_running(id) {
                            PROCS[i].state.store(PEXITED, Ordering::Release);
                        }
                    }
                    crate::arch::sched::exit(); // never returns
                }
                // SOCK-4: the transferable-socket GRANTEE has a launcher-PLANTED Proc entry (the grantor's
                // Child handle names it, owner-scoped, so SYS_XFER can resolve it), so — exactly like the
                // u7x child / u6gx grantee — its exit must be routed BY NAME BEFORE the Proc-reap
                // short-circuit below, or its witness (and SOCK4_DONE) would be swallowed by that path. The
                // GRANTOR has no Proc entry but rides the same arm for symmetry. Mark the planted entry
                // EXITED (a late SYS_XFER to this recipient then fails the RUNNING check). Knob-off /
                // aarch64 never emit this arm.
                #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
                if nm == Some("sock4-grantor") || nm == Some("sock4-grantee") {
                    if nm == Some("sock4-grantor") {
                        SOCK4_GRANTOR_WITNESS.store(a0 as u32, Ordering::Release);
                    } else {
                        SOCK4_GRANTEE_WITNESS.store(a0 as u32, Ordering::Release);
                    }
                    SOCK4_DONE.fetch_add(1, Ordering::AcqRel);
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
                Some("u9x-write") => {
                    // U9x: the File-WRITE fixture conveys its 5-bit witness bitmask as its exit STATUS (routed
                    // by name, the same u5x/u6bx/u8x idiom — x86 has no SYS_REPORT). No planted Proc entry (a
                    // single register-only fixture; the kernel-side revoke check plants its own scratch row),
                    // so it takes the ordinary by-name path. `U9X_DONE` gates the launcher's read.
                    U9X_WITNESS.store(a0 as u32, Ordering::Release);
                    U9X_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u11x-close") => {
                    // U11x: the open-file-lifecycle fixture conveys its 5-bit witness bitmask as its exit STATUS
                    // (routed by name, the same u5x/u6bx/u8x/u9x idiom — x86 has no SYS_REPORT). No planted Proc
                    // entry (a single register-only fixture; the kernel-side gen-rebind check plants its own
                    // scratch row), so it takes the ordinary by-name path. `U11X_DONE` gates the launcher's read.
                    U11X_WITNESS.store(a0 as u32, Ordering::Release);
                    U11X_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u10x-grow") => {
                    // U10 GROW: the growth fixture conveys its 5-bit witness bitmask as its exit STATUS (routed by
                    // name, the same u5x/u9x idiom). No planted Proc entry (a single register-only fixture), so it
                    // takes the ordinary by-name path. `U10X_DONE` gates the launcher's read.
                    U10X_WITNESS.store(a0 as u32, Ordering::Release);
                    U10X_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u10cx-create") => {
                    // U10 CREATE: the create fixture conveys its 4-bit witness bitmask as its exit STATUS (by name).
                    U10CX_WITNESS.store(a0 as u32, Ordering::Release);
                    U10CX_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u10dx-delete") => {
                    // U10 DELETE: the delete fixture conveys its 5-bit witness bitmask as its exit STATUS (by name).
                    U10DX_WITNESS.store(a0 as u32, Ordering::Release);
                    U10DX_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("u11m2-unlink") => {
                    // U11x M2: the cross-process unlink fixture conveys its 6-bit witness bitmask as its exit
                    // STATUS (by name). Spawned TWICE (one per phase) — `U11M2_DONE` counts exits; the launcher
                    // resets `U11M2_WITNESS` before each spawn and gates each read on the count.
                    U11M2_WITNESS.store(a0 as u32, Ordering::Release);
                    U11M2_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some("winx-app") => {
                    // WINX-1: the window fixture conveys its 7-bit witness bitmask as its exit STATUS
                    // (routed by name, the same u5x/u9x/sock2 idiom — x86 has no SYS_REPORT). No planted
                    // Proc entry (a register + inline-data fixture), so it takes the ordinary by-name
                    // path. `WINX_DONE` gates the launcher's read.
                    WINX_WITNESS.store(a0 as u32, Ordering::Release);
                    WINX_DONE.fetch_add(1, Ordering::AcqRel);
                }
                Some(WINX7_TASK_NAME) => {
                    // WINX-7: the threads/futex/input fixture conveys its 8-bit witness bitmask as its
                    // exit STATUS, by name, on the same idiom. Only the PARENT reaches here — its
                    // worker threads terminate through `SYS_THREAD_EXIT`, which never enters this arm.
                    WINX7_WITNESS.store(a0 as u32, Ordering::Release);
                    WINX7_DONE.fetch_add(1, Ordering::AcqRel);
                }
                #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
                Some("sock2-udp") => {
                    // SOCK-2: the UDP round-trip fixture conveys its 5-bit witness bitmask as its exit STATUS
                    // (routed by name, the same u5x/u9x/u11x idiom — x86 has no SYS_REPORT). No planted Proc
                    // entry (a single register + inline-data fixture), so it takes the ordinary by-name path.
                    // `SOCK2_DONE` gates the launcher's read. Knob-off / aarch64 never emit this arm.
                    SOCK2_WITNESS.store(a0 as u32, Ordering::Release);
                    SOCK2_DONE.fetch_add(1, Ordering::AcqRel);
                }
                #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
                Some("sock3-tcp") => {
                    // SOCK-3: the TCP round-trip fixture conveys its 5-bit witness bitmask as its exit STATUS
                    // (by name, the same idiom). `SOCK3_DONE` gates the launcher's read. Knob-off / aarch64
                    // never emit this arm.
                    SOCK3_WITNESS.store(a0 as u32, Ordering::Release);
                    SOCK3_DONE.fetch_add(1, Ordering::AcqRel);
                }
                #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
                Some("zeolite-resolver") => {
                    // SINKHOLE-1: the DNS resolver fixture conveys its 8-bit witness bitmask as its exit
                    // STATUS (by name, the same idiom). `ZEOLITE_DONE` gates the launcher's read. Knob-off /
                    // aarch64 never emit this arm.
                    ZEOLITE_WITNESS.store(a0 as u32, Ordering::Release);
                    ZEOLITE_DONE.fetch_add(1, Ordering::AcqRel);
                }
                // SOCK-4: the grantor + grantee are routed BY NAME in the pre-short-circuit block above
                // (the grantee is PLANTED in PROCS, so it must be caught before the Proc-reap short-circuit
                // — the u7x child / u6gx grantee discipline), so no arm is needed here.
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

// =============================================================================
// CFU-1: the SINGLE validated kernel/user copy seam (x86). Every syscall used to open-code the same
// ring-3 window predicate (`end < ptr || ptr < LO || end > USER_BASE + window`) and then read/write the
// memory RAW (`from_raw_parts` / `copy_nonoverlapping`), leaning on "ring-3 VA == kernel VA in the live
// CR3" (the identity alias). Correct-by-review, but with no single enforcement point: every new syscall
// re-open-coded it, and one future mistake would be a kernel read/write of arbitrary memory. This block
// UNIFIES that check + copy into three primitives; the syscalls below route through them. It does NOT
// change the semantics of the check — it is the EXACT current predicate, overflow-safe, with the same
// per-access lower bound. SMAP (`stac`/`clac`) is a NAMED FOLLOW-ON, NOT this arc: today the copy still
// goes through the identity-mapped window exactly as before (see docs/SECURITY.md CFU-1).
// =============================================================================

/// The access direction a ring-3 range is validated for. `Read` admits the WHOLE window including the
/// read-only code page (page 0 is ring3-RX — a legal read source, e.g. a fixture's RO pattern constant
/// or the DNS/console send buffer). `Write` requires the range to start PAST the code page
/// (`USER_BASE + PAGE_SIZE`): page 0 is RO, so a kernel store there would fault under CR0.WP or corrupt
/// W^X-protected code. This is the exact `sys_write`/`sys_sendto` (read) vs `sys_read`/`sys_recvfrom`
/// (write) lower-bound split, unified.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UserAccess {
    Read,
    Write,
}

/// The SINGLE ring-3 window predicate. Validate that `[ptr, ptr + len)` lies wholly inside the caller's
/// user window (overflow-safe) and — for `Write` — past the read-only code page. Returns `Ok(())` or
/// `Err(EFAULT)`. This is the exact predicate every site open-coded: `end = ptr.wrapping_add(len)`;
/// `end < ptr` IS the wrap check (a range that wraps past u64::MAX has `end < ptr`); `ptr < LO` and
/// `end > window_end` are the bounds. A `len == 0` range is in-bounds iff `ptr` itself is — matching the
/// historical per-site behavior (the zero-length callers that must no-op — `sys_send`/`sys_sock_recv` —
/// already `return 0` BEFORE reaching the check; the ones that don't — `sys_write` — validated a 0-len
/// range against the window exactly like this, so a 0-len write with a below-window pointer is `-EFAULT`
/// as before). `#[must_use]` so a caller cannot silently ignore the verdict.
#[must_use]
fn user_range_ok(ptr: u64, len: u64, access: UserAccess) -> Result<(), i64> {
    let lo = match access {
        UserAccess::Read => USER_BASE,                 // the RO code page is a legal read source
        UserAccess::Write => USER_BASE + PAGE_SIZE,    // past page 0 — a kernel store must hit writable RW pages
    };
    let window_end = USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE;
    let end = ptr.wrapping_add(len);
    if end < ptr || ptr < lo || end > window_end {
        return Err(EFAULT);
    }
    Ok(())
}

/// Validated copy FROM a ring-3 source INTO a kernel buffer: validate `[user_ptr, user_ptr + dst.len())`
/// as a READABLE user range (the code page is a legal source), then copy `dst.len()` bytes. `Err(EFAULT)`
/// leaves `dst` untouched and copies nothing (fail-closed). The single kernel/user READ seam — the
/// `sys_write`/`sys_write_file`/`sys_open` page-bounce sites route their raw `copy_nonoverlapping` here.
/// The copy itself is the historical identity-alias `copy_nonoverlapping` (no SMAP toggle this arc).
#[must_use]
fn copy_from_user(dst: &mut [u8], user_ptr: u64) -> Result<(), i64> {
    user_range_ok(user_ptr, dst.len() as u64, UserAccess::Read)?;
    // Ring-3 VA == kernel VA in the live CR3 (identity alias); the range is proven in-window above.
    unsafe {
        core::ptr::copy_nonoverlapping(user_ptr as *const u8, dst.as_mut_ptr(), dst.len());
    }
    Ok(())
}

/// Validated copy FROM a kernel buffer OUT TO a ring-3 destination: validate `[user_ptr, user_ptr +
/// src.len())` as a WRITABLE user range (inside the window AND past the read-only code page), then copy
/// `src.len()` bytes. `Err(EFAULT)` writes nothing (fail-closed). The single kernel/user WRITE seam — the
/// `sys_read`/`sys_recvfrom`/`sys_sock_recv` sites route their raw `copy_nonoverlapping` here.
#[must_use]
fn copy_to_user(user_ptr: u64, src: &[u8]) -> Result<(), i64> {
    user_range_ok(user_ptr, src.len() as u64, UserAccess::Write)?;
    // Ring-3 VA == kernel VA in the live CR3 (identity alias); the range is proven writable-in-window above.
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), user_ptr as *mut u8, src.len());
    }
    Ok(())
}

// =============================================================================================
// WINX-1 — the PROCESS verbs: SYS_YIELD(4), SYS_SLEEP_MS(5), SYS_GETINFO(7).
//
// The x86 twins of aarch64's `sys_yield`/`sys_sleep_ms`/`sys_getinfo`, at the SHARED numbers, with the
// same ring-3 contracts (yield/sleep return 0 unconditionally; getinfo returns 0 or `-EFAULT`). They
// exist because a windowed EL0 app paces itself: `user-stat` calls `SYS_SLEEP_MS` once per frame and
// `SYS_GETINFO` once at startup to learn the pid it paints, and neither had an x86 arm.
//
// ONE REAL ARCH DIVERGENCE, and it is a simplification. The aarch64 handlers must call `remask_irq()`
// before returning, because `sched::yield_now`/`sleep_ticks` there unmask unconditionally and the
// `__vec_svc` epilogue that follows restores per-core banked ELR/SPSR/SP_EL0 and MUST run I-masked.
// The x86 primitives instead SNAPSHOT the caller's IF (`are_enabled()`) and restore exactly that after
// the context switch, so a syscall handler entered IF-masked (SFMASK clears IF) is resumed IF-masked
// and needs no re-mask. Nothing about the switch itself differs — this is the same "a blocking syscall
// may safely `switch_context` here" property `syscall_dispatch` already documents for the storage
// syscalls.
//
// The other divergence is that x86 needs no dead-timer fallback. aarch64's `sys_sleep_ms` must test
// `timer::is_live()` and degrade to `yield_now`, because QEMU raspi4b delivers no Group-1 timer IRQ and
// `sleep_ticks` (whose only waker is the tick) would park the caller forever. The x86 local-APIC
// heartbeat is the scheduler's own tick and is always armed (`apic::init` arms it calibrated or at the
// fixed fallback), so `sleep_ticks` always has a waker. The honest residual is RESOLUTION, not
// liveness: before `apic::calibrate` runs, a tick is ~0.8 ms under QEMU, so a sleep runs proportionally
// short — the documented degradation `arch::ms_to_ticks` already carries.

/// WINX-1: `SYS_GETINFO`'s payload — the fixed `{pid, ticks}` struct copied out to ring 3. `#[repr(C)]`
/// plain-old-data, field-for-field identical to the aarch64 `UserInfo`, so one arch-neutral ring-3
/// program reads the same two little-endian u64s on either arch.
#[repr(C)]
struct UserInfo {
    pid: u64,
    ticks: u64,
}

/// WINX-1: `SYS_GETINFO(user_ptr)` — write `{pid, ticks}` to the caller's buffer through the validated
/// `copy_to_user` seam. Returns 0, or `-EFAULT` if the destination fails validation (outside the ring-3
/// window, wrapping, or aimed at the read-only code page — `UserAccess::Write` starts past page 0). An
/// error RETURN, never a task-kill: a bad pointer is a ring-3 mistake, not a kernel-integrity event.
///
/// `pid` is the SCHEDULER task id (`current_task_id` on this CPU), which is what the x86 `Proc` table
/// and the ring-3 fault-kill log already name a task by — the same identity aarch64 reports from
/// `sched::current_id()`. `ticks` is the global 1 kHz ms-since-boot timebase (`arch::ticks()`).
/// `None` from `current_task_id` (a syscall from an unscheduled context, which cannot normally happen —
/// ring 3 always runs inside a scheduled task) reports pid 0 rather than failing: the aarch64 twin's
/// `unwrap_or(0)`, kept identical so a program sees the same degenerate value on both arches.
fn sys_getinfo(user_ptr: u64) -> i64 {
    let info = UserInfo {
        pid: crate::arch::sched::current_task_id(crate::arch::sched::meter_current_cpu())
            .unwrap_or(0),
        ticks: crate::arch::ticks(),
    };
    // SAFETY: view `info` as its raw bytes for the copy; `UserInfo` is `#[repr(C)]` plain-old-data with
    // no padding (two u64s), so the byte view is exactly the two fields in declaration order.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &info as *const UserInfo as *const u8,
            core::mem::size_of::<UserInfo>(),
        )
    };
    match copy_to_user(user_ptr, bytes) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// WINX-1: `SYS_YIELD()` — cooperatively give up the CPU; thin over `sched::yield_now()`. Returns 0.
/// Safe from the IF-masked handler: `yield_now` disables IF for its own critical section, switches, and
/// restores the caller's IF snapshot on resume (see the block comment above) — so we return IF-masked,
/// exactly as the `unaos_syscall_entry` epilogue expects. A no-op (still 0) outside a scheduled task,
/// matching `yield_now`'s own contract.
fn sys_yield() -> i64 {
    crate::arch::sched::yield_now();
    0
}

/// WINX-1: `SYS_SLEEP_MS(ms)` — block the calling task ~`ms` milliseconds; thin over `sched::sleep_ms`,
/// which is `sleep_ticks(arch::ms_to_ticks(ms))` against the local-APIC heartbeat. Returns 0.
///
/// The ms→ticks conversion is deliberately NOT duplicated here (the aarch64 twin has to hardcode a
/// `TICK_HZ` because its `timer::TICK_HZ` is private): `arch::ms_to_ticks` is the one place the
/// ms<->tick relationship lives on x86, so a future retune of the tick rate cannot leave this verb
/// behind. Like `yield_now`, `sleep_ticks` restores the caller's IF snapshot, so this returns
/// IF-masked; and like it, it is a no-op outside a scheduled task (an immediate 0 rather than a hang).
fn sys_sleep_ms(ms: u64) -> i64 {
    crate::arch::sched::sleep_ms(ms);
    0
}

// =============================================================================================
// WINX-1 — the WINDOW SURFACE/VERB SEAM. The x86 twin of the aarch64 WC-B block, wired to the SAME
// arch-neutral compositor (`video::wm`), which has always been arch-neutral and until now had no x86
// caller other than `video/wcx.rs`'s kernel-drawn demo window.
//
// TWO INDICES, deliberately distinct (the aarch64 distinction, and it matters for the same reasons):
//   * the WINDOW ID (0..WIN_MAX) — GLOBAL, what EL0 passes to the verbs;
//   * the REGION SLOT (0..FB_WIN_SLOTS) — PER-ADDRESS-SPACE, which 64 KiB surface slot of the owner's
//     own FB region backs it. Region slots are allocated lowest-first per slot, so a process's FIRST
//     window always lands on region slot 0 — the VA (`base + 0x5000`) that `crates/user-stat` and
//     `crates/user-vug` compute. Collapsing the two would be wrong in both directions: a global surface
//     index would leak one process's surface VA layout into another's address space, and a per-process
//     window id would make ids ambiguous to the compositor.
//
// OWNERSHIP IS AUTHORITATIVE HERE, not in the compositor. Every verb resolves the caller's address-space
// SLOT from the live CR3 (`memory::current_slot` — the x86 stand-in for aarch64's ASID, the same read
// every handle gate uses) and refuses a window it does not own, errno-for-errno with the handle gates:
// `-EBADF` for an id out of range or free (the `sys_close` shape), `-EACCES` for a live window belonging
// to another slot (the rights-denial shape). Keeping the check on this side means the seam to `wm`
// carries no security weight.
//
// WHAT THIS ARC DOES NOT BRING OVER from aarch64, deliberately: the `SYS_FB_MAP`/`SYS_FB_PRESENT` compat
// pair (24/25) and their legacy info-page header. Those exist on aarch64 only to keep a pre-WC VUG.ELF
// binary byte-identical; x86 has no such binary to preserve, so the compat path would be dead weight
// carrying real complexity (`win_bind_compat`, the WIN_NONE present branch, `close_compat`). x86 gets
// the window verbs and only the window verbs. The numbers stay reserved.
//
// SLOT 0 IS NOT A WINDOW OWNER. `memory::current_slot()` returns `None` for the shared kernel window
// (the U1a/U1b/U2 tasks, which have no private address space and therefore no FB region); those callers
// get `-EINVAL`, the same refusal aarch64 gives ASID 0.

/// WINX-1: the fixed window count. Matches `memory::FB_WIN_SLOTS` and the compositor's fixed table.
/// STOP tripwire: a deliberate cap, like `USER_SLOTS` — do not raise it for a demo.
const WIN_MAX: usize = 8;
const _: () = assert!(WIN_MAX == crate::arch::x86_64::memory::FB_WIN_SLOTS);
const _: () = assert!(WIN_MAX <= crate::video::wm::MAX_WINDOWS);

/// WINX-1: one window table row. `owner == WIN_OWNER_FREE` means FREE. Unlike aarch64 — where ASID 0 is
/// the shared context and so doubles as the free marker — x86 slot 0 is a REAL address space, so the
/// table needs an explicit sentinel rather than reusing 0.
#[derive(Clone, Copy)]
struct WinEntry {
    owner: usize,
    rslot: u8,
    pages: u8,
    w: u16,
    h: u16,
    /// The compositor's OWN id for this window (`video::wm::WinId`), or `wm::WIN_NONE` when `wm::create`
    /// refused (table full, framebuffer not ready, geometry rejected). The two id spaces are separate:
    /// THIS table is authoritative for allocation and ownership, while `wm` mints its own id out of its
    /// own table. Storing wm's id here IS the binding — every later `wm` call goes through this field,
    /// never through a coincidence of the two indices lining up (they do not: wm ids are `1..=MAX_WINDOWS`).
    wm_id: crate::video::wm::WinId,
}

/// WINX-1: the free-row sentinel. `usize::MAX` is not a reachable slot index (`USER_SLOTS` is 8).
const WIN_OWNER_FREE: usize = usize::MAX;

impl WinEntry {
    const FREE: WinEntry = WinEntry {
        owner: WIN_OWNER_FREE,
        rslot: 0,
        pages: 0,
        w: 0,
        h: 0,
        wm_id: crate::video::wm::WIN_NONE,
    };
}

/// WINX-1: the window table. Taken IRQ-masked via `IrqGuard` on EVERY access — it is acquired from
/// syscall context (preemptible) AND from the teardown path (`free_user_space_by_cr3` ->
/// `win_close_slot`), the exact asymmetry `IrqGuard` exists to close. Held across the page-table
/// maintenance in `sys_win_create` so a create and a teardown on two cores cannot interleave their
/// leaf edits on the same slot.
static WINDOWS: SpinMutex<[WinEntry; WIN_MAX]> = SpinMutex::new([WinEntry::FREE; WIN_MAX]);

/// WINX-1: resolve the caller's address-space slot for a window verb. `-EINVAL` from the shared kernel
/// window (no private slot => no FB region), matching aarch64's refusal for ASID 0.
fn win_caller_slot() -> Result<usize, i64> {
    crate::arch::x86_64::memory::current_slot().ok_or(EINVAL)
}

/// WINX-1: pages needed for a `w` x `h` ARGB8888 surface — the negotiated PAGE-MULTIPLE size. `None` if
/// the geometry is out of range (0, or beyond `FB_WIN_MAX_W/H`), so every caller is fail-closed by shape.
fn win_pages_for(w: u32, h: u32) -> Option<usize> {
    use crate::arch::x86_64::memory::{FB_WIN_MAX_H, FB_WIN_MAX_W};
    if w == 0 || h == 0 || w > FB_WIN_MAX_W || h > FB_WIN_MAX_H {
        return None;
    }
    Some((w as usize) * 4 * (h as usize)).map(|b| b.div_ceil(0x1000))
}

/// WINX-1: publish window `id`'s geometry into the owner's RO info page, at the per-window entry for its
/// region slot. Layout of the page (all u32, little-endian) — the aarch64 layout, so one arch-neutral
/// program parses both:
///   `[0x40 + r*0x20]` per region slot `r`: magic, win_id, w, h, stride, size, surface-offset-from-info-base.
/// A region slot with no live window keeps a zeroed entry (magic 0), so EL0 can tell live from stale.
/// Written through the KERNEL identity pointer — the EL0 alias of this page is read-only, which is the
/// point: a program cannot forge the geometry the compositor was told about.
///
/// The `[0x00..0x1C]` legacy ELF-3 header and the `[0x20]` process-flags word are aarch64-only (they
/// serve `SYS_FB_MAP` and `bg`-detached launches, neither of which x86 has this arc) and stay zeroed.
fn fb_info_write_win(slot: usize, id: usize, e: &WinEntry) {
    use crate::arch::x86_64::memory as mem;
    let info = mem::slot_fb_info_ptr(slot) as *mut u32;
    let off = mem::FB_INFO_SIZE + (e.rslot as usize) * mem::FB_WIN_SLOT_SIZE;
    unsafe {
        let p = info.add(0x40 / 4 + (e.rslot as usize) * (0x20 / 4));
        p.add(0).write_volatile(FB_MAGIC);
        p.add(1).write_volatile(id as u32);
        p.add(2).write_volatile(e.w as u32);
        p.add(3).write_volatile(e.h as u32);
        p.add(4).write_volatile(e.w as u32 * 4);
        p.add(5).write_volatile((e.pages as usize * 0x1000) as u32);
        p.add(6).write_volatile(off as u32);
    }
}

/// WINX-1: the info-page entry magic — 'UWIN' little-endian. Same value the aarch64 header uses, so a
/// program can validate the entry it reads on either arch.
const FB_MAGIC: u32 = 0x4E49_5755;

/// WINX-7: which address-space slots were launched DETACHED (`bg`) rather than in the foreground
/// (`run`). Set by `spawn_user_image_bg` before the task is spawned; cleared on slot teardown.
///
/// This exists because a detached windowed app has no operator to press ESC and — being launched into
/// the background — may never receive input at all, so an app whose auto path ends after a fixed frame
/// count would simply vanish a second or two after `bg`. On the panel that reads as a crash, which is
/// exactly what it read as on the Pi before the aarch64 VUG-BG arc published the same bit. The remedy
/// there and here is the same: TELL the program, and let it decide (`user-vug` skips its frame cap and
/// tumbles until it is killed).
///
/// It is published in the RO info page's process-flags word rather than returned from a syscall so a
/// program can read it once, cheaply, with no new verb — and it is read-only to EL0, so a program can
/// learn how it was launched but cannot claim to have been launched some other way.
static SLOT_DETACHED: [AtomicBool; crate::arch::memory::USER_SLOTS] =
    [const { AtomicBool::new(false) }; crate::arch::memory::USER_SLOTS];

/// WINX-7: the process-flags word's DETACHED bit, at info-page byte offset `0x20`. Bit 0, the same
/// bit and the same offset the aarch64 info page uses, so one arch-neutral program reads it on both.
const FB_FLAG_DETACHED: u32 = 1 << 0;

/// WINX-7: publish slot `slot`'s process-flags word into its RO info page. Written through the KERNEL
/// identity pointer — EL0's alias of this page is read-only — and called from `sys_win_create`, which
/// is what maps the info page in the first place, so the word is present the moment a program can read
/// it and never before.
fn fb_info_write_flags(slot: usize) {
    let info = crate::arch::x86_64::memory::slot_fb_info_ptr(slot) as *mut u32;
    let flags = if SLOT_DETACHED[slot].load(Ordering::Acquire) { FB_FLAG_DETACHED } else { 0 };
    unsafe { info.add(0x20 / 4).write_volatile(flags) };
}

/// WINX-1: `SYS_WIN_CREATE(w, h)` -> window id (0..WIN_MAX), or a negative errno.
///
/// Allocates the lowest free REGION slot for this address space (so a process's first window is region
/// slot 0, the VA its program computes), claims a global window id, maps the info page plus EXACTLY the
/// negotiated pages of the surface slot, binds a compositor window, and publishes the geometry.
///
/// Errno shapes: `-EINVAL` from the shared kernel window or a degenerate/oversized geometry; `-EMFILE`
/// when this process already holds `FB_WIN_SLOTS` windows (the caller's own limit); `-ENFILE` when the
/// global table is full (a system limit). The split mirrors `SYS_OPEN`'s EMFILE/ENFILE distinction.
///
/// The out-of-range geometry test happens on the u64 arguments BEFORE any narrowing, so a huge value
/// cannot wrap into a legal one on the cast.
fn sys_win_create(w: u64, h: u64) -> i64 {
    use crate::arch::x86_64::memory as mem;
    let slot = match win_caller_slot() {
        Ok(v) => v,
        Err(e) => return e,
    };
    if w > mem::FB_WIN_MAX_W as u64 || h > mem::FB_WIN_MAX_H as u64 {
        return EINVAL;
    }
    let (w32, h32) = (w as u32, h as u32);
    let pages = match win_pages_for(w32, h32) {
        Some(p) => p,
        None => return EINVAL,
    };
    let _irq = IrqGuard::mask_save();
    let mut t = WINDOWS.lock();
    // Lowest-free REGION slot for this address space.
    let rslot = match (0..WIN_MAX)
        .find(|&r| !(0..WIN_MAX).any(|i| t[i].owner == slot && t[i].rslot as usize == r))
    {
        Some(r) => r,
        None => return EMFILE,
    };
    let id = match (0..WIN_MAX).find(|&i| t[i].owner == WIN_OWNER_FREE) {
        Some(i) => i,
        None => return ENFILE,
    };
    let mut e = WinEntry {
        owner: slot,
        rslot: rslot as u8,
        pages: pages as u8,
        w: w32 as u16,
        h: h32 as u16,
        wm_id: crate::video::wm::WIN_NONE,
    };
    // Map the info page (idempotent) and exactly the negotiated surface pages, under the table lock so
    // no concurrent teardown can edit the same leaves mid-sequence. The surface is ZEROED first, through
    // the kernel identity alias: a recycled REGION slot within a live address space (create, close,
    // create) must not hand the new window the old window's pixels, and the app has not drawn yet.
    unsafe {
        core::ptr::write_bytes(
            mem::slot_fb_win_surface_ptr(slot, rslot),
            0,
            pages * 0x1000,
        );
        mem::map_slot_fb_info(slot);
        mem::map_slot_fb_win(slot, rslot, pages);
    }
    // Bind the compositor window BEFORE publishing the row, so the row is never visible to another core
    // with a stale `wm_id`. The surface pointer is the kernel's identity view of the leaves just mapped,
    // and `surf_len` is the REAL byte length of that mapped slot (`pages * 0x1000`, the page-multiple
    // this verb negotiated) — never a recomputed `h * stride`. That distinction is wm's F1 extent
    // contract: `w`/`h`/`stride` are EL0-influenced, `pages` comes from the mapping code, and it is
    // `surf_len` that bounds every source read the compositor performs.
    // CLICK-X86 — the compositor owner is `slot + 1`, the SAME `+1` bias `EL0_INPUT_ACTIVE` carries,
    // and for the same reason stated there: x86 slot 0 is a REAL address space, so an unbiased owner
    // of 0 is indistinguishable from wm's "nobody owns this row". Unbiased, the first program to
    // launch got a window that `hit_test` skipped (`owner_asid == 0`) and `focus_ring` skipped, i.e.
    // it could be neither clicked nor tabbed to — silently, and only for slot 0. Biased, the value
    // `hit_test` returns is the value the router compares against `EL0_INPUT_ACTIVE` with no
    // conversion at the one seam that decides who receives a keystroke.
    e.wm_id = wc_shim::create(
        (slot as u64) + 1,
        mem::slot_fb_win_surface_ptr(slot, rslot) as usize,
        pages * 0x1000,
        w32,
        h32,
        w32 * 4,
        id,
    );
    t[id] = e;
    fb_info_write_win(slot, id, &e);
    // WINX-7: publish the process-flags word alongside the geometry. Idempotent across a process's
    // several `SYS_WIN_CREATE`s (it re-writes the same value), and it must be here rather than at
    // launch because `map_slot_fb_info` above is what makes the page exist.
    fb_info_write_flags(slot);
    drop(t);
    // WINX-7: if NOBODY currently has input focus, the process that just opened a window gets it.
    //
    // This is the minimum viable focus POLICY, and it is deliberately the weakest one that makes an
    // interactive EL0 app reachable at all. x86 has no focus-cycling key yet (the aarch64 WC-C TAB
    // ring lives in that arch's router seam, which this arc does not have a twin of), so without some
    // rule a program launched with `bg` would create a window, poll `SYS_INPUT_POLL` forever, and
    // never receive a single event — indistinguishable from broken input.
    //
    // What makes "only when nobody has focus" the safe version: it can never STEAL focus. A second
    // windowed app launched behind a focused one does not take the keyboard from it, so a background
    // program cannot arrange to receive keystrokes aimed at the window in front of it — which is the
    // property the producer-side focus gate exists to guarantee. Focus returns to the shell when the
    // holder's slot is torn down (`el0_input_revoke_slot`), so the next app launched after that one
    // exits is focusable in turn. A real focus RING — click-to-focus, a reserved cycling key — is the
    // follow-on arc, and it will replace this rule rather than build on it.
    if EL0_INPUT_ACTIVE
        .compare_exchange(0, (slot as u64) + 1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        clear_input_row(slot); // a fresh focus starts clean, exactly as `el0_input_set_active` does
        serial_println!(":: wc-x86: input focus -> slot {} (first window, shell was idle) ::", slot);
    }
    serial_println!(
        ":: wc-x86: SYS_WIN_CREATE slot={} win={} rslot={} {}x{} pages={} wm_id={} ::",
        slot, id, rslot, w32, h32, pages, e.wm_id
    );
    id as i64
}

/// WINX-1: `SYS_WIN_PRESENT(win)` -> 0, or a negative errno. Damage-mark + composite the caller's
/// window. Ownership-gated (`-EBADF` unresolvable, `-EACCES` another process's live window).
///
/// LOCK SPAN — the ownership check, the geometry snapshot AND the present run under ONE continuous hold
/// of the window table lock, for the reason the aarch64 twin documents: resolving the row, dropping the
/// lock, then presenting would leave a real window in which a close+create pair on other cores recycles
/// this id to a DIFFERENT process, and the composite would land the caller's pixels under the new
/// owner's window identity. Holding across the composite is what makes the id the compositor is handed
/// provably still the id we validated. The cost is a bounded blit inside an IRQ-masked spinlock, bounded
/// by the 64 KiB surface cap.
///
/// LOCK ORDER: `WINDOWS` is the OUTERMOST lock; `video::wm`'s own state is acquired strictly inside it
/// (`wc_shim::present` is called with `WINDOWS` held, never the reverse). The reverse edge does not
/// exist by construction — neither `video/wm.rs` nor `video/screen.rs` references the syscall layer, so
/// nothing under `wm` can call back into a window verb.
fn sys_win_present(win: u64) -> i64 {
    let slot = match win_caller_slot() {
        Ok(v) => v,
        Err(e) => return e,
    };
    if win >= WIN_MAX as u64 {
        return EBADF;
    }
    let id = win as usize;
    let _irq = IrqGuard::mask_save();
    let t = WINDOWS.lock();
    if t[id].owner == WIN_OWNER_FREE {
        return EBADF;
    }
    if t[id].owner != slot {
        return EACCES;
    }
    let e = t[id];
    FB_PRESENT_COUNT.fetch_add(1, Ordering::AcqRel);
    wc_shim::present(e.wm_id);
    drop(t);
    0
}

/// WINX-1: total `SYS_WIN_PRESENT` calls that reached the compositor — the headless witness's proof that
/// a present actually happened, independent of whether a panel was attached.
static FB_PRESENT_COUNT: AtomicU64 = AtomicU64::new(0);

/// WINX-1: read the present counter (the fixture verdict reads it).
pub fn fb_present_count() -> u64 {
    FB_PRESENT_COUNT.load(Ordering::Acquire)
}

/// WINX-1: retire every window owned by address-space slot `s`. Called from
/// `memory::free_user_space_by_cr3` before the FB leaves are dropped and the slot released, so a
/// recycled slot inherits no live compositor row pointing into backing about to be reused.
///
/// The rows are collected and the table lock RELEASED before `wm::close` runs: `close` executes a drain
/// barrier that spins on in-flight composites, and a `WINDOWS`-holding thread inside that barrier would
/// deadlock against a `sys_win_present` waiting for `WINDOWS` on another core. This is the same
/// lock-order rule the aarch64 twin states — `wm::close`/`close_owner` are the one pair that must NOT be
/// called with the window table held.
pub fn win_close_slot(s: usize) {
    let mut doomed = [crate::video::wm::WIN_NONE; WIN_MAX];
    {
        let _irq = IrqGuard::mask_save();
        let mut t = WINDOWS.lock();
        for i in 0..WIN_MAX {
            if t[i].owner == s {
                doomed[i] = t[i].wm_id;
                t[i] = WinEntry::FREE;
            }
        }
    }
    for id in doomed {
        wc_shim::destroy(id);
    }
}

/// WINX-1: the `video::wm` seam. Every compositor call the window verbs make goes through here, so the
/// coupling to the arch-neutral compositor is one small, reviewable surface rather than scattered calls.
mod wc_shim {
    use crate::video::wm::{self, WinId, WIN_NONE};

    /// `video::wm::create`. `surf_len` is the REAL mapped-slot byte length, the bound wm's F1 extent
    /// contract requires; `id` is our own row index, used only to build the KERNEL-OWNED title — an app
    /// never supplies its window's title, because chrome is kernel-drawn, always, so a program cannot
    /// paint something that looks like another window's frame. Returns wm's id, or `WIN_NONE` if the
    /// compositor refused (table full, framebuffer not ready, geometry rejected).
    ///
    /// A refusal is NOT an error for the syscall: the window still exists as far as this process is
    /// concerned (its surface is mapped and its own), it simply has no compositor row, so presents are
    /// accounted and dropped. That is the fail-closed direction — nothing extra is exposed to EL0 — and
    /// it keeps a headless run (no panel, `wm` never ready) from failing a program that only draws.
    /// CLICK-X86: `owner` is the `+1`-BIASED slot (`slot + 1`), matching `EL0_INPUT_ACTIVE`, so that
    /// `wm::hit_test`'s answer is directly comparable with the input focus and slot 0's windows are
    /// not mistaken for wm's ownerless rows.
    pub fn create(
        owner: u64,
        surf: usize,
        surf_len: usize,
        w: u32,
        h: u32,
        stride: u32,
        id: usize,
    ) -> WinId {
        let title = [b'e', b'l', b'0', b' ', b'w', b'i', b'n', b' ', b'0' + (id as u8 % 10)];
        wm::create(owner, surf, surf_len, w, h, stride, &title)
    }

    /// `video::wm::present` — damage-mark and run the compositor pass. Called with `WINDOWS` held.
    pub fn present(id: WinId) {
        if id != WIN_NONE {
            wm::present(id);
        }
    }

    /// `video::wm::close`. Runs a drain barrier, so every caller invokes it with `WINDOWS` RELEASED.
    pub fn destroy(id: WinId) {
        if id != WIN_NONE {
            wm::close(id);
        }
    }
}

// =============================================================================================
// WINX-7 — INPUT INTO EL0: `SYS_INPUT_POLL(27)`, the per-process input ring, the router seam, and
// the focus registration. The x86 twin of aarch64's ELF-5.
//
// An interactive EL0 app needs keys and mouse. It already has a surface (`SYS_WIN_CREATE`), pacing
// (`SYS_SLEEP_MS`) and, as of this arc, threads and a futex; this is the delivery half.
//
// SHAPE. The kernel holds a small per-SLOT ring of packed events. The router — the shell's own event
// drain, which is the single place every HID event on this arch passes through — offers each drained
// event to `el0_input_route`, which forwards it into the FOCUSED process's ring or hands it back for
// the shell to consume. EL0 drains its own ring nonblocking through `SYS_INPUT_POLL`. One producer
// (the single router drain) and one consumer (the owning EL0 task) per ring, so the ring is a
// lock-free SPSC: free-running head/tail, occupancy = `tail - head`.
//
// FOCUS GATING IS THE WHOLE SECURITY ARGUMENT, and it is why the ACTIVE slot is consulted on the
// PRODUCER side rather than the consumer side. Only one process can receive input at a time, and it
// is the one the window system says has focus; an unfocused program's ring simply never fills. A
// per-app opt-in would be no gate at all (any app could claim the keyboard by never opting out), and
// a consumer-side check would mean events for the focused app are queued into every process's ring
// and merely hidden — which is a disclosure, not a gate. Enqueue-into-the-focused-ring-only means a
// background program cannot observe a single keystroke aimed at another window, including keystrokes
// that are passwords.
//
// KEYED BY SLOT, NOT PID. `memory::current_slot()` is the x86 stand-in for aarch64's ASID and is what
// every other per-process gate in this file already uses. The consequence that matters: all THREADS
// of a process share one ring (they share a slot), which is correct — an app's worker threads and its
// parent are one input consumer, and `user-vug` drains from the parent only. The ring is reset on
// focus change and on slot teardown, so a recycled slot inherits nothing.
// =============================================================================================

/// WINX-7 packed-event type tags (bits [55:48] of the packed u64). Shared with aarch64 by law, so one
/// arch-neutral EL0 program decodes the same wire form on both.
const INPUT_EV_KEY_DOWN: u64 = 1; // a key PRESS   (payload[7:0] = ASCII / the C0 arrow codes)
const INPUT_EV_KEY_UP: u64 = 2; // a key RELEASE (payload[7:0] = same)
const INPUT_EV_MOUSE_REL: u64 = 3; // relative pointer motion  (payload[31:16] = dx, [15:0] = dy, i16)
const INPUT_EV_MOUSE_ABS: u64 = 4; // absolute pointer position(payload[31:16] = x,  [15:0] = y,  i16)
const INPUT_EV_BUTTON: u64 = 5; // a pointer button state    (payload[7:0] = button bitmask)

/// Per-process input ring capacity. A power of two, because occupancy is `tail.wrapping_sub(head)`
/// and the slot index is `& (CAP - 1)`.
const INPUT_RING_CAP: usize = 32;

/// The per-process input rings, keyed by address-space SLOT. One producer (the router) + one consumer
/// (the owning EL0 task) per ring => lock-free SPSC.
static EL0_INPUT_BUF: [[AtomicU64; INPUT_RING_CAP]; crate::arch::memory::USER_SLOTS] =
    [const { [const { AtomicU64::new(0) }; INPUT_RING_CAP] }; crate::arch::memory::USER_SLOTS];
/// Consumer index (free-running; advanced by `sys_input_poll`). Real slot = `head & (CAP - 1)`.
static EL0_INPUT_HEAD: [AtomicU32; crate::arch::memory::USER_SLOTS] =
    [const { AtomicU32::new(0) }; crate::arch::memory::USER_SLOTS];
/// Producer index (free-running; advanced by `el0_input_push`). Occupancy = `tail - head`.
static EL0_INPUT_TAIL: [AtomicU32; crate::arch::memory::USER_SLOTS] =
    [const { AtomicU32::new(0) }; crate::arch::memory::USER_SLOTS];

/// The slot currently designated to RECEIVE input, `+1`-BIASED: 0 means "no EL0 target — the shell
/// owns the keyboard", and `s + 1` means slot `s` has focus.
///
/// The bias is not cosmetic. aarch64 can use a bare ASID because ASID 0 is its shared/boot context
/// and so doubles as "nobody"; x86 slot 0 is a REAL address space that a real program can occupy
/// (the WINX-1 fixture routinely does), so an unbiased 0 would silently mean "the first program to
/// launch always has focus". This is the same reason `WinEntry` needs `WIN_OWNER_FREE` and `Proc`
/// stores `slot + 1`.
static EL0_INPUT_ACTIVE: AtomicU64 = AtomicU64::new(0);

/// How many times a slot TEARDOWN revoked the live input focus — i.e. the dying slot *was* the
/// focused one. Zero on a boot where no focused program ever exited; it climbs by one per focused
/// program that ends. Read by the witness to prove its measurement window was not crossed by a
/// revocation it did not cause.
static EL0_FOCUS_REVOKES: AtomicU64 = AtomicU64::new(0);

/// Router accounting: events actually DELIVERED into some process's ring, and events DROPPED because
/// the target ring was full. The drop counter is the honest half — a full ring is a real event loss
/// and must be countable, not silent.
static EL0_INPUT_DELIVERED: AtomicU64 = AtomicU64::new(0);
static EL0_INPUT_DROPPED: AtomicU64 = AtomicU64::new(0);

/// WINX-7: the slot currently receiving input, `+1`-biased (0 = the shell). The read side of
/// [`el0_input_set_active`], for the router and the witnesses.
pub fn el0_input_active() -> u64 {
    EL0_INPUT_ACTIVE.load(Ordering::Acquire)
}

/// WINX-7: `(delivered, dropped, focus_revokes)` — the router's own accounting, for the witness line.
pub fn el0_input_stats() -> (u64, u64, u64) {
    (
        EL0_INPUT_DELIVERED.load(Ordering::Acquire),
        EL0_INPUT_DROPPED.load(Ordering::Acquire),
        EL0_FOCUS_REVOKES.load(Ordering::Acquire),
    )
}

/// Pack a `pal::Event` into the WINX-7 wire form, or `None` for an event that carries nothing an EL0
/// app can use (`Timer`/`None`/`Unknown` are kernel-internal pacing and non-events).
///
/// BIT 63 IS ALWAYS CLEAR by construction — the type tag lives at [55:48] — which is what lets
/// `sys_input_poll` hand the packed value straight back as a NON-NEGATIVE `i64` that is unambiguously
/// distinguishable from `-EAGAIN`. EL0 tests `ev >> 63` and needs no separate out-parameter.
fn pack_input(ev: crate::pal::Event) -> Option<u64> {
    use crate::pal::Event;
    let pack_xy = |x: i32, y: i32| -> u64 { ((x as i16 as u16 as u64) << 16) | (y as i16 as u16 as u64) };
    let (ty, payload): (u64, u64) = match ev {
        Event::Key(b) => (INPUT_EV_KEY_DOWN, b as u64),
        Event::KeyUp(b) => (INPUT_EV_KEY_UP, b as u64),
        Event::Mouse { x, y } => (INPUT_EV_MOUSE_REL, pack_xy(x, y)),
        Event::MouseAbsolute { x, y } => (INPUT_EV_MOUSE_ABS, pack_xy(x, y)),
        Event::Button(mask) => (INPUT_EV_BUTTON, mask as u64),
        Event::Timer | Event::None | Event::Unknown => return None,
    };
    Some((ty << 48) | payload)
}

/// Push a pre-packed event into `slot`'s ring — the SPSC producer half. DROP-NEWEST on a full ring, so
/// a backlog can never clobber an event the EL0 consumer has not read yet: an app that falls behind
/// loses the events it could not have handled anyway, rather than having its next read silently
/// replaced by a later one (which would corrupt a press/release pairing). `slot` is validated by the
/// caller. Returns whether the event was queued.
fn el0_input_push(slot: usize, packed: u64) -> bool {
    let head = EL0_INPUT_HEAD[slot].load(Ordering::Acquire);
    let tail = EL0_INPUT_TAIL[slot].load(Ordering::Relaxed); // the router is the sole producer
    if tail.wrapping_sub(head) >= INPUT_RING_CAP as u32 {
        EL0_INPUT_DROPPED.fetch_add(1, Ordering::Relaxed);
        return false; // full — drop the newest
    }
    EL0_INPUT_BUF[slot][(tail as usize) & (INPUT_RING_CAP - 1)].store(packed, Ordering::Release);
    // Publish the tail AFTER the slot store: the consumer's Acquire load of the tail is what makes
    // the payload visible to it, so the two stores must not be reordered.
    EL0_INPUT_TAIL[slot].store(tail.wrapping_add(1), Ordering::Release);
    EL0_INPUT_DELIVERED.fetch_add(1, Ordering::Relaxed);
    true
}

/// WINX-7 ROUTER SEAM (public, in-lane): offer one input event to the FOCUSED process's ring.
///
/// Returns `true` if the event was queued for an EL0 app (the caller must NOT also give it to the
/// shell), `false` if there is no focused EL0 target, the event carries nothing deliverable, or the
/// target ring was full. Single producer: exactly one drain loop may call this.
pub fn el0_input_enqueue(ev: crate::pal::Event) -> bool {
    let active = EL0_INPUT_ACTIVE.load(Ordering::Acquire);
    if active == 0 {
        return false; // the shell owns the keyboard
    }
    let slot = (active - 1) as usize;
    if slot >= crate::arch::memory::USER_SLOTS {
        return false; // impossible unless the focus word was corrupted — fail to the shell
    }
    let Some(packed) = pack_input(ev) else {
        return false;
    };
    el0_input_push(slot, packed)
}

/// WINX-7 ROUTER FOLD POINT (public, in-lane) — the ONE line the shell's event drain needs.
///
/// Hands `ev` to [`el0_input_enqueue`] and reports the outcome AS AN EVENT, so a caller folds this in
/// without restructuring its drain: the event comes back UNCHANGED when no EL0 app took it (the shell
/// handles it exactly as before), and comes back as `Event::Unknown` when an EL0 ring consumed it.
///
/// `Event::Unknown` and not `Event::None` — deliberately, and it is the whole reason this wrapper
/// exists rather than a bare boolean. Every drain loop on this arch treats `Event::None` as
/// END-OF-QUEUE and breaks on it; returning `None` for a consumed event would end the drain at the
/// first keystroke routed to a window and strand everything queued behind it. `Event::Unknown`
/// already falls through every such loop's catch-all arm as a no-op, which is precisely the
/// "swallowed, keep draining" semantics required.
pub fn el0_input_route(ev: crate::pal::Event) -> crate::pal::Event {
    if el0_input_enqueue(ev) {
        crate::pal::Event::Unknown
    } else {
        ev
    }
}

/// Reset a slot's input ring (head == tail == 0 => empty). Called on a focus change (a freshly
/// focused app starts clean — no stale input aimed at whoever had focus before) and from
/// `clear_handle_row` on slot teardown (a reused slot inherits no stale input). Safe in both cases:
/// on teardown no producer runs for the dying slot, and on a focus change the router only ever
/// targets the newly-active slot.
fn clear_input_row(slot: usize) {
    if slot >= crate::arch::memory::USER_SLOTS {
        return;
    }
    EL0_INPUT_HEAD[slot].store(0, Ordering::Release);
    EL0_INPUT_TAIL[slot].store(0, Ordering::Release);
}

/// WINX-7 FOCUS REGISTRATION (public, in-lane): designate which process receives input.
/// `slot_plus1` is `slot + 1`, or 0 to hand the keyboard back to the shell.
///
/// A real focus (non-zero) RESETS the incoming ring first: the events queued while another window had
/// focus were aimed at that window, and delivering them to the new one is both wrong and, for a
/// button, actively harmful (a release with no matching press is a fabricated click). Clearing focus
/// leaves every ring alone — from then on events legitimately belong to the shell.
pub fn el0_input_set_active(slot_plus1: u64) {
    if slot_plus1 != 0 {
        let slot = (slot_plus1 - 1) as usize;
        if slot >= crate::arch::memory::USER_SLOTS {
            return; // not a real slot — refuse rather than publish a focus nobody can drain
        }
        clear_input_row(slot); // a fresh focus starts clean
    }
    EL0_INPUT_ACTIVE.store(slot_plus1, Ordering::Release);
}

/// WINX-7: revoke input focus if the dying `slot` holds it, and reset its ring. Called from
/// `clear_handle_row`, i.e. on the slot-teardown path both `exit` and the `KillSwitch` reap funnel
/// through.
///
/// The CAS is slot-exact, which is what makes this safe against a concurrent focus change: if focus
/// has already moved to another slot we must not clear it (that would silently deafen the app that
/// just gained focus), and if it has not, the exchange is the only writer. Without this a killed
/// windowed app would leave focus pointing at its own dead slot forever and every later keystroke
/// would be enqueued into a ring with no consumer — the keyboard would appear to stop working with
/// no message anywhere.
fn el0_input_revoke_slot(slot: usize) {
    let biased = (slot as u64) + 1;
    if EL0_INPUT_ACTIVE
        .compare_exchange(biased, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        EL0_FOCUS_REVOKES.fetch_add(1, Ordering::AcqRel);
        serial_println!(":: wc-x86: input focus revoked — slot {} torn down ::", slot);
    }
    clear_input_row(slot);
}

/// WINX-7: `SYS_INPUT_POLL()` — nonblocking dequeue of the next input event for the CALLING process.
/// Returns the packed event (>= 0, bit 63 clear) or `-EAGAIN` when the ring is empty or the caller has
/// no private slot (the shared kernel window, which is not an input target — the same refusal the
/// window verbs give it).
///
/// The SPSC consumer half: this process is the sole consumer of its own ring, so `head` is read
/// Relaxed and published Release AFTER the payload load — the mirror of the producer's ordering.
///
/// NOT GATED ON FOCUS, deliberately. An unfocused app may drain whatever was queued while it DID have
/// focus, which is what makes tabbing away and back lossless rather than a silent truncation; the gate
/// that matters is on the PRODUCER side, where an unfocused app's ring stops filling in the first
/// place. The distinction is that a program may always read what was already addressed to it, and may
/// never receive what was addressed to somebody else.
fn sys_input_poll() -> i64 {
    let Some(slot) = crate::arch::x86_64::memory::current_slot() else {
        return EAGAIN;
    };
    let head = EL0_INPUT_HEAD[slot].load(Ordering::Relaxed); // sole consumer
    let tail = EL0_INPUT_TAIL[slot].load(Ordering::Acquire);
    if head == tail {
        return EAGAIN; // empty
    }
    let packed = EL0_INPUT_BUF[slot][(head as usize) & (INPUT_RING_CAP - 1)].load(Ordering::Acquire);
    EL0_INPUT_HEAD[slot].store(head.wrapping_add(1), Ordering::Release); // consume AFTER the load
    packed as i64
}

/// CLICK-X86 witness read: how many events sit UNREAD in the ring of the `+1`-biased slot `active`.
/// `0` for the shell slot and for anything outside the private-slot range (they have no ring). The
/// selftest's only way to say "the press was DELIVERED" rather than "the router said it would be".
pub fn el0_input_depth(active: u64) -> u32 {
    if active == 0 {
        return 0;
    }
    let slot = (active - 1) as usize;
    if slot >= crate::arch::memory::USER_SLOTS {
        return 0;
    }
    EL0_INPUT_TAIL[slot]
        .load(Ordering::Acquire)
        .wrapping_sub(EL0_INPUT_HEAD[slot].load(Ordering::Acquire))
}

// =============================================================================================
// CLICK-X86 — A POINTER PRESS GOES TO THE WINDOW UNDER THE CURSOR.
//
// Built directly to the CLICK-PLAIN contract (the aarch64 seat's `475c51d3`); the superseded
// CLICK-SWALLOW shape (`1ed1c725`, "a focus-changing press is never also app input") is deliberately
// never passed through on this arch. The principle both arcs converged on: prefer a rule that is
// true UNCONDITIONALLY over one that is true given hidden state. A focus-changing press DELIVERS.
//
// ### What was here before: nothing
// The x86 event drain's `match` ended `_ => {}`. There was no `Event::Button` arm at all — HID pushed
// presses onto the queue and this drain popped and DISCARDED them. `wm::focus_changed` and
// `wm::hit_test` had no x86 callers; `FOCUS_ASID` was written only by wm's own selftests. So BOTH of
// the operator's standing complaints on x86 metal — that clicks get "eaten", and that out-of-focus
// clicks stop the focused app — named a mechanism that did not exist. Neither could ever have been
// right, and neither could be told apart on the wire. That is what this section supplies.
//
// ### The rule, entire
// **A press goes to the window under the cursor, and if that window was not focused, the focus goes
// there first.** Nothing about the press's DELIVERY is conditional on which window happened to hold
// focus when the hand moved; what a delivered click MEANS is the app's decision, made in the app.
// =============================================================================================

/// CLICK-X86: the pointer-button bitmask as the ROUTER last saw it, so this layer can tell a PRESS
/// edge (a bit going 0->1) from a RELEASE edge (1->0) from an unchanged held state. Its own tracker,
/// not shared with any drain: routing decisions are made upstream of every consumer, on events some
/// consumers never see.
static CLICK_PREV_MASK: AtomicU32 = AtomicU32::new(0);

/// CLICK-X86 sentinel: the outstanding press was NOT delivered to any ring, so its release must not be
/// either. `u64::MAX` is not a valid biased slot (`USER_SLOTS` is single digits) and sits far above
/// `wm::KERNEL_OWNER_BASE`, so it cannot collide with either population.
const CLICK_TARGET_DROP: u64 = u64::MAX;

/// CLICK-X86: where the outstanding PRESS was delivered, so the matching RELEASE follows it. A biased
/// slot (0 = the shell path), or [`CLICK_TARGET_DROP`] when the press was consumed here.
///
/// **A press/release pair must never be split across two apps.** A release delivered to an app that
/// never saw the press is a FABRICATED click — the receiving app sees a button go up that never went
/// down, which for a click-to-pause program is an invented click. So the release edge is never
/// re-hit-tested; it is compared against this and either follows the press or is dropped.
static CLICK_PRESS_TARGET: AtomicU64 = AtomicU64::new(CLICK_TARGET_DROP);

/// CLICK-X86 accounting, for the rollup: press edges seen, and press edges DELIVERED into some ring.
static CLICK_PRESSES: AtomicU64 = AtomicU64::new(0);
static CLICK_DELIVERED: AtomicU64 = AtomicU64::new(0);

/// CLICK-X86: `(presses, delivered)` — every press edge the router judged, and how many of them were
/// addressed to an EL0 ring. The difference is the count of presses that belonged to the shell.
pub fn click_stats() -> (u64, u64) {
    (
        CLICK_PRESSES.load(Ordering::Acquire),
        CLICK_DELIVERED.load(Ordering::Acquire),
    )
}

/// CLICK-X86: the current pointer position in PANEL pixels — the point a click is addressed to. Reads
/// the shared cursor state every other pointer consumer reads (the same state the compositor draws),
/// clamped by the live panel geometry.
///
/// Locks: the framebuffer info lock, then the cursor position lock, and both are released before
/// `wm::hit_test` takes the window TABLE lock. No nesting, so no new lock order.
fn click_pointer_pos() -> (i32, i32) {
    let (w, h) = {
        let info = crate::video::WRITER.lock().info();
        (info.width as i32, info.height as i32)
    };
    crate::pal::cursor::pos(w, h)
}

/// CLICK-X86 — **route a pointer button by POSITION rather than by focus.** The shell's event drain
/// calls this on every event, BEFORE `el0_input_route`; non-`Button` events return `false` untouched.
///
/// Returns `true` when the event was CONSUMED and the caller must not deliver or forward it. Under
/// CLICK-PLAIN that is only the arms with no app to deliver to (a press on kernel furniture or on the
/// bare desktop, and the release that follows one). `false` means "carry on with your normal path",
/// with whatever focus this call left in place — unchanged on a press to the already-focused window,
/// and the newly RAISED owner on a press that moved focus to a window.
///
/// See [`wc_click_route_at`] for the rule and its argument; this is that function at the live cursor.
///
/// Idempotent per edge: the mask tracker is swapped on entry, so a second call with the same mask
/// sees no edge and answers `false`.
pub fn wc_click_route(ev: crate::pal::Event) -> bool {
    let (x, y) = match ev {
        crate::pal::Event::Button(_) => click_pointer_pos(),
        _ => return false, // no position read for an event that cannot be a click
    };
    wc_click_route_at(ev, x, y)
}

/// CLICK-X86 — [`wc_click_route`] with the press POSITION supplied rather than read from the cursor.
///
/// The position is a parameter so that this decision is drivable with **no pointer at all**, which is
/// the only way it can be covered in QEMU (the gate runs headless and delivers no HID pointer). It is
/// the same idiom `wm::hittest_selftest` uses — assert against the window TABLE, not against the
/// panel — extended one layer up, from "which window owns this pixel" to "who receives this press".
///
/// ### The rule, on a PRESS edge
/// Hit-test the point (`wm::hit_test` — the topmost visible window whose outer box contains it):
///
///  * **a window owned by a DIFFERENT address space than the focused one** — raise it to focus
///    through the one focus primitive that exists (`el0_input_set_active` then `wm::focus_changed`,
///    in that order), and then **DELIVER the press to it**. The wake half runs FIRST and in full, so
///    by the time the caller pushes, `EL0_INPUT_ACTIVE` already names the raised owner and the press
///    lands in the ring of the window that was clicked. Click-to-focus, with no second focus path
///    invented for it.
///
///    This is the CLICK-PLAIN decision, and it is the one thing this arc most deliberately does not
///    inherit from CLICK-SWALLOW. Withholding the focus-changing press makes a click's effect depend
///    on invisible state — which window held focus, whether this was the first click or the second —
///    which is precisely what an operator cannot model. The router's job is ROUTING; the app decides
///    what a delivered click means.
///
///  * **the FOCUSED window** — deliver exactly as before. The no-change case, and the common one.
///
///  * **a KERNEL-OWNED window** (`wm::is_kernel_owner` — the panel console, the desktop demo) — the
///    click landed on the kernel's furniture, which owns no address space and has no input ring. Two
///    things happen and they are separate: the row is RAISED (`focus_changed(owner)`, so the click
///    has a visible effect — the console comes to the front under the hand), and the KEYBOARD goes
///    back to the shell (`el0_input_set_active(0)`), because the console is the shell's surface and
///    clicking it must be how the operator reaches it. The press itself is consumed: on x86 the shell
///    has no click consumer at all, so there is nothing to deliver it to, and re-addressing it after
///    the fact would split a pair whose press edge no consumer saw.
///
///    **`focus_changed(0)` is NOT called here, and the difference is x86-specific.** On aarch64 the
///    shell is the desktop layer BENEATH the window layer, so raising `SHELL_Z` reveals the console.
///    On x86 the console IS a window row (`fbcon::panel_console_window_open`), so raising `SHELL_Z`
///    above every window would push the console below the shell, stop it compositing and erase it to
///    the desktop colour — it would blank the console the operator just clicked. The kernel row's own
///    z-bump is the correct raise on this arch and `SHELL_Z` is left where it is.
///
///  * **no window** — the bare desktop. Not the focused app's click, so it is consumed rather than
///    delivered, and the keyboard goes back to the shell for the same reason the kernel-row arm gives
///    (clicking a window focuses it; clicking the desktop must focus the desktop). Two limits, the
///    same two CLICK-SHELL r2 settled: with focus already at the shell nothing is consumed and no
///    focus move is made, and a FULL-SCREEN app presenting through the compat row is exempt — a
///    compat row covers the panel but carries owner ASID 0, so it can never be hit, and a miss over a
///    full-screen app is a hit on that app.
///
/// ### The RELEASE edge follows the press, and is never re-routed
/// See [`CLICK_PRESS_TARGET`]. The release is delivered iff the focus is still the one the press was
/// delivered to; otherwise it is dropped. A focus change (or an app exit) between press and release
/// costs the release, never a fabricated one in a second app.
pub fn wc_click_route_at(ev: crate::pal::Event, x: i32, y: i32) -> bool {
    let crate::pal::Event::Button(mask) = ev else {
        return false;
    };
    let prev = CLICK_PREV_MASK.swap(mask as u32, Ordering::Relaxed) as u8;
    let cur = EL0_INPUT_ACTIVE.load(Ordering::Acquire);
    if mask & !prev != 0 {
        // PRESS edge.
        CLICK_PRESSES.fetch_add(1, Ordering::Relaxed);
        match crate::video::wm::hit_test(x, y) {
            // KERNEL FURNITURE — raise it, hand the keyboard to the shell, consume the press.
            Some((win, owner, _z)) if crate::video::wm::is_kernel_owner(owner) => {
                clickroute_witness(x, y, win, owner, cur, "consume", 0);
                el0_input_set_active(0);
                crate::video::wm::focus_changed(owner);
                CLICK_PRESS_TARGET.store(CLICK_TARGET_DROP, Ordering::Release);
                true
            }
            // A DIFFERENT app's window — raise it, then deliver the press into its ring.
            Some((win, owner, _z)) if owner != cur => {
                clickroute_witness(x, y, win, owner, cur, "raise+deliver", owner);
                // The wake half, in order: the focus arrival first, then the raise. Only then does
                // the caller push — and by then `EL0_INPUT_ACTIVE` is `owner`.
                el0_input_set_active(owner);
                crate::video::wm::focus_changed(owner);
                // The release must follow the press into the SAME ring: record the raised owner, not
                // the sentinel, so the pair is delivered whole.
                CLICK_PRESS_TARGET.store(owner, Ordering::Release);
                CLICK_DELIVERED.fetch_add(1, Ordering::Relaxed);
                false
            }
            // The ALREADY-FOCUSED window — the no-change case.
            Some((win, owner, _z)) => {
                clickroute_witness(x, y, win, owner, cur, "deliver", cur);
                CLICK_PRESS_TARGET.store(cur, Ordering::Release);
                CLICK_DELIVERED.fetch_add(1, Ordering::Relaxed);
                false
            }
            None => {
                // A full-screen app presenting through the compat row owns every pixel that
                // hit-tests to nothing. `hit_test` has already said no WINDOW owns this point; the
                // compat row is the only other thing that can.
                if cur != 0 && !crate::video::wm::compat_live() {
                    clickroute_witness(x, y, crate::video::wm::WIN_NONE, 0, cur, "consume", 0);
                    el0_input_set_active(0);
                    CLICK_PRESS_TARGET.store(CLICK_TARGET_DROP, Ordering::Release);
                    true
                } else {
                    let how = if cur == 0 { "shell" } else { "fullscreen" };
                    clickroute_witness(x, y, crate::video::wm::WIN_NONE, 0, cur, how, cur);
                    CLICK_PRESS_TARGET.store(cur, Ordering::Release);
                    if cur != 0 {
                        CLICK_DELIVERED.fetch_add(1, Ordering::Relaxed);
                    }
                    false
                }
            }
        }
    } else if prev & !mask != 0 {
        // RELEASE edge — follow the press, or drop. Never hit-tested: the release belongs to whoever
        // received the press, not to whatever the pointer has since been dragged over.
        let target = CLICK_PRESS_TARGET.load(Ordering::Acquire);
        target == CLICK_TARGET_DROP || target != cur
    } else {
        // Unchanged mask (a re-report of a held button, or an idempotent second call): no edge.
        false
    }
}

/// CLICK-X86 — the witness, and the ONE line the operator's two standing complaints are separated on.
///
/// It extends the existing `[clickroute]` vocabulary rather than inventing a second one: the same tag
/// `wm::hittest_selftest` already prints under, now carrying a per-press row. One line per press edge,
/// human-rate by construction (a hand cannot click faster than serial can print), so it needs no
/// throttle of its own.
///
/// **How it tells the two complaints apart, in one sitting.**
///  * *"the click was EATEN"* — press the pad and NO `[clickroute] press` line appears. The press
///    never reached the router: the defect is upstream, in HID or the queue, and no routing change
///    can fix it. (Before this arc every press on x86 was in exactly this state, silently.)
///  * *"the click was MIS-ROUTED"* — a line appears, and its `win=`/`owner=` name something other
///    than the window the hand was over, or `deliver=` names an asid other than that window's owner.
///    The press arrived and was addressed wrongly, and the defect is in the hit-test or in this
///    policy.
///  * *"the out-of-focus click stopped my app"* — a line whose `was=` names the focused app while
///    `deliver=` names 0 or another asid is the proof that this arc's rule held: the press went where
///    the hand pointed, not to whoever had the keyboard.
///
/// `deliver=0` means the press was NOT put in any app's ring (the shell's, the desktop's, or the
/// kernel furniture's arm); `win=` is `wm::WIN_NONE` when the point resolved to no window at all.
fn clickroute_witness(x: i32, y: i32, win: u32, owner: u64, was: u64, how: &str, deliver: u64) {
    serial_println!(
        "[clickroute] press at ({},{}) win={} owner={:#x} was={} -> {} deliver={}",
        x, y, win, owner, was, how, deliver
    );
}

/// The probe surface for [`clickroute_selftest`] — 8x8 ARGB8888, the `hittest_selftest` geometry.
#[cfg(feature = "witness")]
#[repr(align(4))]
struct ClickSurf([u32; 64]);
#[cfg(feature = "witness")]
static CLICK_SURF: ClickSurf = ClickSurf([0x0020_4060; 64]);

/// CLICK-X86 — the ROUTING witness: `wm::hittest_selftest` asserts *which window owns a pixel*; this
/// asserts *who receives the press*, which is the layer the operator's two complaints live in.
///
/// Headless-drivable, on the same idiom and for the same reason: QEMU delivers no pointer at all, so
/// the position is a PARAMETER (`wc_click_route_at`) rather than a read of the cursor, and every claim
/// is made against the window table and the input rings rather than against the panel.
///
/// ### The legs, each a distinct failure direction
///  1. **hit** — the probe point resolves to the topmost probe window (`B`). The address lookup is
///     `hittest_selftest`'s business; this only establishes the fixture.
///  2. **deliver** — with focus on `A`, a press over `B` is NOT consumed and moves focus to `B`. This
///     is the CLICK-PLAIN decision itself, and the direction CLICK-SWALLOW would fail: under that
///     superseded shape the focus-changing press is swallowed and this leg reads `false`.
///  3. **depth** — the press then actually LANDS in `B`'s ring (depth 1), and its RELEASE follows it
///     into the same ring (depth 2). A press/release pair delivered whole to one app, which is what
///     makes a fabricated half-click impossible.
///  4. **kernel** — a press over a KERNEL-owned row (the console, the demo — hittable since this arc)
///     is consumed and hands the keyboard back to the shell, and does NOT move `SHELL_Z` (which on
///     x86 would blank the console window it just raised).
///  5. **desktop** — a press over a point no window owns is consumed and hands the keyboard back to
///     the shell. Skipped, and said so, if the live panel leaves no unowned point to probe.
///  6. **nofab** — the release that follows a CONSUMED press is dropped rather than delivered. A
///     release in an app that never saw the press is a fabricated click; the sentinel forbids it.
///
/// Self-cleaning: the probe rows are closed, the input focus is restored to whatever held it, and
/// `wm::focus_reset` un-names the synthetic focus owner.
#[cfg(feature = "witness")]
pub fn clickroute_selftest() {
    use crate::pal::Event;
    use crate::video::wm;
    static DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    let (pw, ph) = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!("[clickroute] route -> SKIP (framebuffer not ready)");
            return;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    if pw < 256 || ph < 256 {
        serial_println!("[clickroute] route -> SKIP (panel {}x{} too small)", pw, ph);
        return;
    }

    // Biased slots, i.e. the values `SYS_WIN_CREATE` now hands the compositor and the values
    // `EL0_INPUT_ACTIVE` carries — the whole point of the bias is that these are the SAME numbers.
    const OWNER_A: u64 = 1; // slot 0
    const OWNER_B: u64 = 2; // slot 1
    // In the reserved kernel band, so `wm::is_kernel_owner` answers for it exactly as it does for the
    // real console row, without this witness disturbing the real console row.
    const OWNER_K: u64 = wm::KERNEL_OWNER_BASE + 0x7F;

    let s = &raw const CLICK_SURF as usize;
    let len = core::mem::size_of_val(&CLICK_SURF);
    let wa = wm::create(OWNER_A, s, len, 8, 8, 32, b"cr-a");
    let wb = wm::create(OWNER_B, s, len, 8, 8, 32, b"cr-b");
    let wk = wm::create(OWNER_K, s, len, 8, 8, 32, b"cr-k");
    if wa == wm::WIN_NONE || wb == wm::WIN_NONE || wk == wm::WIN_NONE {
        serial_println!(
            "[clickroute] route -> SKIP (window table full: a={} b={} k={})",
            wa, wb, wk
        );
        wm::close(wa);
        wm::close(wb);
        wm::close(wk);
        return;
    }

    // Three DISJOINT origins in a row, deliberately not the stacked pair `hittest_selftest` uses:
    // this witness raises windows as part of the decisions it is testing (`focus_changed` bumps the
    // focused owner's z), so overlapping probe boxes would let a raise silently change which window
    // owns a probe point and turn the "different window" leg into the "already focused" one. Pinned
    // with `move_to` against the tiler; the span is read back from the row the compositor actually
    // made, so the spacing tracks the panel's own scale rule rather than a number kept in sync here.
    let ox = pw / 3;
    let oy = ph / 4 + wm::TITLE_H + wm::BORDER;
    wm::move_to(wa, ox, oy);
    let scale = wm::info(wa).map(|i| i.scale).unwrap_or(1);
    let span = 8 * scale + 2 * wm::BORDER + wm::TITLE_H + 8;
    let (bxo, kxo) = (ox + span, ox + 2 * span);
    wm::move_to(wb, bxo, oy);
    wm::move_to(wk, kxo, oy);
    let (apx, apy) = ((ox + 2) as i32, (oy + 2) as i32);
    let (bpx, bpy) = ((bxo + 2) as i32, (oy + 2) as i32);
    let (kpx, kpy) = ((kxo + 2) as i32, (oy + 2) as i32);

    // Leg 1 — the fixture: three disjoint boxes, each owned by exactly the owner that made it.
    let hit_ok = wm::hit_test(apx, apy).map(|(_, a, _)| a) == Some(OWNER_A)
        && wm::hit_test(bpx, bpy).map(|(_, a, _)| a) == Some(OWNER_B)
        && wm::hit_test(kpx, kpy).map(|(_, a, _)| a) == Some(OWNER_K);
    let _ = (apx, apy);

    // The DESKTOP probe point, found rather than assumed: on x86 the console is itself a window and
    // now hittable, so most of the panel is owned. Report a skip rather than a false verdict if the
    // live panel leaves nothing unowned.
    let corners = [
        (2i32, 2i32),
        (pw as i32 - 3, 2),
        (2, ph as i32 - 3),
        (pw as i32 - 3, ph as i32 - 3),
    ];
    let desktop_pt = corners.iter().copied().find(|&(x, y)| wm::hit_test(x, y).is_none());

    let saved_focus = el0_input_active();

    // Leg 2/3 — focus on A, press over B. Delivered, not swallowed; the pair lands whole in B's ring.
    el0_input_set_active(OWNER_A);
    wm::focus_changed(OWNER_A);
    CLICK_PREV_MASK.store(0, Ordering::Relaxed);
    let press_consumed = wc_click_route_at(Event::Button(1), bpx, bpy);
    let raised_ok = el0_input_active() == OWNER_B;
    let deliver_ok = !press_consumed && raised_ok && el0_input_enqueue(Event::Button(1));
    let depth_press = el0_input_depth(OWNER_B);
    let rel_consumed = wc_click_route_at(Event::Button(0), bpx, bpy);
    let rel_ok = !rel_consumed && el0_input_enqueue(Event::Button(0));
    let depth_rel = el0_input_depth(OWNER_B);
    let depth_ok = depth_press == 1 && depth_rel == 2 && rel_ok;

    // Leg 4 — a press over KERNEL furniture, from a live app focus. Consumed; keyboard to the shell;
    // the raise is the row's own z-bump and `SHELL_Z` must not move.
    el0_input_set_active(OWNER_A);
    wm::focus_changed(OWNER_A);
    let shell_z_before = wm::shell_z();
    let kernel_consumed = wc_click_route_at(Event::Button(1), kpx, kpy);
    let kernel_ok =
        kernel_consumed && el0_input_active() == 0 && wm::shell_z() == shell_z_before;

    // Leg 6 — the release after a CONSUMED press is dropped, never delivered.
    let nofab_ok = wc_click_route_at(Event::Button(0), kpx, kpy);

    // Leg 5 — a press over the bare desktop, from a live app focus.
    let desktop_ok = match desktop_pt {
        Some((x, y)) => {
            el0_input_set_active(OWNER_A);
            wm::focus_changed(OWNER_A);
            let consumed = wc_click_route_at(Event::Button(1), x, y);
            let ok = consumed && el0_input_active() == 0;
            wc_click_route_at(Event::Button(0), x, y);
            Some(ok)
        }
        None => None,
    };

    let ok = hit_ok
        && deliver_ok
        && depth_ok
        && kernel_ok
        && nofab_ok
        && desktop_ok.unwrap_or(true);
    serial_println!(
        "[clickroute] route hit={} deliver={} depth={}/{} kernel={} desktop={} nofab={} -> {}",
        hit_ok,
        deliver_ok,
        depth_press,
        depth_rel,
        kernel_ok,
        match desktop_ok {
            Some(v) => if v { "true" } else { "false" },
            None => "skip",
        },
        nofab_ok,
        if ok { "PASS" } else { "FAIL" }
    );

    wm::close(wa);
    wm::close(wb);
    wm::close(wk);
    el0_input_set_active(saved_focus);
    CLICK_PREV_MASK.store(0, Ordering::Relaxed);
    CLICK_PRESS_TARGET.store(CLICK_TARGET_DROP, Ordering::Release);
    wm::focus_reset();
}

// =============================================================================================
// WINX-7 — EL0 THREADS: `SYS_THREAD_SPAWN(21)` / `SYS_THREAD_EXIT(22)` / `SYS_THREAD_JOIN(23)`.
//
// The syscall half; the lifetime machinery is `sched::spawn_user_thread` +
// `sched::user_space_retain`/`user_space_release` (see the section header there for why teardown had
// to become refcounted). What lives HERE is the part with security weight: validating that the entry
// and stack a program hands us are inside ITS OWN window, and making a thread handle name a thread
// that this exact tenant of this exact slot spawned.
//
// THE HANDLE TABLE IS GLOBAL AND FIXED, like every other table in this file. That has one consequence
// worth stating plainly, because aarch64 learned it the hard way (its `[killbound]` note): a row is
// released by the owner's own voluntary `SYS_THREAD_JOIN`, so a program KILLED before it joins leaks
// every row it holds — permanently, from a table shared by the whole machine. The remedy here is the
// same LAZY SCAVENGE, gated on the same kind of positive quiescence witness: `SLOT_GEN[slot]` is
// bumped by `clear_handle_row` on the slot's teardown edge, so `SLOT_GEN[owner] != rec.tenant_gen` is proof
// that the tenant which spawned the thread is entirely gone — not idle, gone — and its row is dead.
// =============================================================================================

/// How many concurrently-tracked joinable EL0 threads the kernel holds handles for. A small fixed
/// pool (`user-vug` uses 2 per process); `-EAGAIN` when exhausted, after the scavenge below has had
/// its chance. STOP tripwire: a deliberate cap, like `USER_SLOTS` — do not raise it for a demo.
const NTHREAD: usize = 8;

/// One live thread the kernel can be `SYS_THREAD_JOIN`ed on.
struct ThreadRec {
    /// The address-space slot that spawned it — only that process may join it.
    owner: usize,
    /// The owner slot's GENERATION at spawn time (`SLOT_GEN[owner]`). Slots are RECYCLED, so `owner`
    /// alone does not identify a TENANT; the generation is what stops a new tenant of the same slot
    /// from joining (and reaping) its predecessor's thread handle, and what makes the scavenge's
    /// "this row is dead" judgement a positive proof rather than a guess.
    tenant_gen: u64,
    /// The completion handle, posted by the thread's `exit()` whatever ends it.
    join: crate::arch::sched::JoinHandle,
}

/// The thread-handle table; the index IS the handle returned to EL0. A `SpinMutex` (not the per-slot
/// atomic sidecars the handle table uses) because a `JoinHandle` is a non-`Copy` owned value that
/// `join` must MOVE out. The lock is held only for the claim or the take — NEVER across the blocking
/// join, which would park a task holding a spinlock every other thread verb needs.
static THREAD_TABLE: SpinMutex<[Option<ThreadRec>; NTHREAD]> = SpinMutex::new([const { None }; NTHREAD]);

/// WINX-7: `SYS_THREAD_SPAWN(entry, sp, arg, place)` -> a thread handle (>= 0), or a negative errno.
///
/// Validates that `entry` and `sp` are inside the CALLER's own ring-3 window before anything else
/// happens — this is the gate. `entry` need only be in-window and readable: whether the target page is
/// actually executable is enforced by the page permissions, and a thread aimed at a non-exec page
/// simply takes a contained ring-3 fault and is killed by the existing net (there is no need, and no
/// way, to answer "is this an instruction?" here). `sp` must be 16-aligned (the SysV requirement the
/// first push depends on) and sit in the WRITABLE part of the window with headroom below it, which
/// excludes the read-only code page — so a stack aimed at page 0 is refused rather than faulting on
/// its first push.
///
/// `place`: 0 = the caller's core, 1 = a sibling core that is actually dispatching. Errno shapes:
/// `-EFAULT` bad entry/sp, `-EINVAL` from the shared kernel window (no private address space to
/// thread within), `-EAGAIN` the table is full.
fn sys_thread_spawn(entry: u64, sp: u64, arg: u64, place: u64) -> i64 {
    // entry: inside the window (the code page IS a legal target — that is where a worker lives).
    if user_range_ok(entry, 1, UserAccess::Read).is_err() {
        return EFAULT;
    }
    // sp: 16-aligned, and at least 16 WRITABLE bytes below it inside the window. `sp` is the stack
    // TOP the thread will push from, so the bytes that must be writable are the ones BELOW it.
    if sp & 0xF != 0 || sp < 16 || user_range_ok(sp - 16, 16, UserAccess::Write).is_err() {
        return EFAULT;
    }
    let Some(slot) = crate::arch::x86_64::memory::current_slot() else {
        return EINVAL; // the shared kernel window owns no private address space to thread within
    };
    let cr3 = crate::arch::x86_64::memory::slot_cr3(slot);
    let caller_cpu = crate::arch::sched::meter_current_cpu();
    let cpu = if place == 1 {
        crate::arch::sched::sibling_online_cpu(caller_cpu)
    } else {
        caller_cpu
    };
    let tenant_gen = SLOT_GEN[slot].load(Ordering::Acquire);

    let mut tab = THREAD_TABLE.lock();
    let idx = match tab.iter().position(|s| s.is_none()) {
        Some(i) => i,
        None => {
            // SCAVENGE — reclaim rows whose owning TENANT is provably gone. `SLOT_GEN[owner] != tenant_gen`
            // means that slot reached `clear_handle_row`, which happens only after the last live task
            // under it retired, so the thread this row tracks is not merely idle — it does not exist
            // on any core. Deliberately LAZY (under pressure) rather than eager at teardown: the
            // teardown path runs IRQ-masked, sometimes from the scheduler's own context, and taking
            // this `SpinMutex` there would add a lock-order hazard for nothing. Here the lock is
            // already held, in the one context that actually needs the rows.
            let mut freed = usize::MAX;
            for (i, row) in tab.iter_mut().enumerate() {
                let dead = match row {
                    Some(r) => SLOT_GEN[r.owner].load(Ordering::Acquire) != r.tenant_gen,
                    None => false,
                };
                if dead {
                    let owner = row.as_ref().map(|r| r.owner).unwrap_or(0);
                    *row = None; // drops the JoinHandle's Arc clone with the row
                    serial_println!(
                        ":: winx7: thread table full — reclaimed row {} from dead slot {} (its tenant reached teardown, so every task under it has retired) ::",
                        i, owner
                    );
                    if freed == usize::MAX {
                        freed = i;
                    }
                }
            }
            if freed == usize::MAX {
                return EAGAIN;
            }
            freed
        }
    };
    // RETAIN BEFORE SPAWN. The extra hold on the address space must be published before the task can
    // be dispatched, because a preemptible task can run on another core the instant it is enqueued —
    // and if the parent exited in that window an unretained slot would be freed under a live thread.
    crate::arch::sched::user_space_retain(cr3);
    let join = crate::arch::sched::spawn_user_thread(
        EL0_THREAD_NAME,
        entry,
        sp,
        arg as usize,
        cpu,
        cr3,
    );
    tab[idx] = Some(ThreadRec { owner: slot, tenant_gen, join });
    drop(tab);
    THREADS_SPAWNED.fetch_add(1, Ordering::AcqRel);
    idx as i64
}

/// WINX-7: `SYS_THREAD_JOIN(handle)` -> 0, or `-ESRCH`. Block until the named thread finishes, then
/// reap its handle (single-shot — the row is taken, so a second join on the same handle is `-ESRCH`
/// rather than a permanent park on a completion nobody will post twice).
///
/// The row is resolved against `(caller slot, slot generation)`, never the slot alone: slots are
/// recycled and a killed program's rows outlive it until the scavenge reclaims them, so slot-only
/// ownership would let a NEW tenant block on — and reap — its predecessor's thread. Fail-closed.
///
/// The `JoinHandle` is MOVED out UNDER the lock and the lock DROPPED BEFORE the blocking join. Joining
/// while holding `THREAD_TABLE` would park a task holding the spinlock that every spawn and join needs,
/// which is a system-wide wedge reachable from ring 3 by a program that simply joins a slow thread.
fn sys_thread_join(handle: u64) -> i64 {
    let Some(slot) = crate::arch::x86_64::memory::current_slot() else {
        return ESRCH;
    };
    let idx = handle as usize;
    if idx >= NTHREAD {
        return ESRCH;
    }
    let rec = {
        let mut tab = THREAD_TABLE.lock();
        let tenant_gen = SLOT_GEN[slot].load(Ordering::Acquire);
        if !tab[idx].as_ref().is_some_and(|r| r.owner == slot && r.tenant_gen == tenant_gen) {
            return ESRCH;
        }
        tab[idx].take().expect("thread row vanished under its own lock")
    };
    rec.join.join(); // blocks until the thread posts its completion (a scheduler wake)
    THREADS_JOINED.fetch_add(1, Ordering::AcqRel);
    0
}

/// WINX-7: `SYS_THREAD_EXIT()` — terminate the calling EL0 thread. Never returns.
///
/// `sched::exit()` does both halves: it posts this task's completion (waking a joiner) and drops its
/// hold on the shared address space, which tears the slot down only if this was the LAST thread. A
/// process's MAIN task uses `SYS_EXIT` instead; the two differ only in the accounting the `SYS_EXIT`
/// arm performs on the `Proc` row, which a worker thread does not own.
fn sys_thread_exit() -> i64 {
    THREADS_EXITED.fetch_add(1, Ordering::AcqRel);
    crate::arch::sched::exit() // diverges
}

/// The kernel-side name every EL0 worker thread carries. Fixed (not derived from the program) for the
/// same reason a window title is: it appears on the wire and in the fault-kill log, so it must not be
/// anything ring 3 chose.
const EL0_THREAD_NAME: &str = "el0-thread";

/// WINX-7 thread accounting — successful spawns, completed joins, and thread exits. Global rather than
/// per-slot: they exist to let a witness assert what happened on a boot, not to enforce anything.
static THREADS_SPAWNED: AtomicU64 = AtomicU64::new(0);
static THREADS_JOINED: AtomicU64 = AtomicU64::new(0);
static THREADS_EXITED: AtomicU64 = AtomicU64::new(0);

/// WINX-7: completed `FUTEX_WAIT`s that actually PARKED and were woken — the deterministic proof that
/// the futex is a real wait/wake and not a spin. See the `Woken` arm of `sys_futex` for why only that
/// outcome is counted.
static FUTEX_PARKS: AtomicU64 = AtomicU64::new(0);

/// WINX-7: the park counter's read side.
pub fn futex_park_count() -> u64 {
    FUTEX_PARKS.load(Ordering::Acquire)
}

/// WINX-7: `(spawned, joined, exited)` — read by the regression witness.
pub fn thread_stats() -> (u64, u64, u64) {
    (
        THREADS_SPAWNED.load(Ordering::Acquire),
        THREADS_JOINED.load(Ordering::Acquire),
        THREADS_EXITED.load(Ordering::Acquire),
    )
}

/// WINX-7: `SYS_FUTEX(uaddr, op, val)` -> op-specific, or a negative errno.
///
/// Validates `uaddr` as a 4-aligned, WRITABLE address inside the caller's own window — a futex word is
/// read AND written by ring 3, so the read-only code page is not a legal target and `UserAccess::Write`
/// is the right predicate — then derives the key from `(caller slot, uaddr)` and dispatches:
///   * `FUTEX_WAIT`: block iff `*uaddr == val`. Returns 0 when woken, `-EAGAIN` on a value mismatch
///     (the caller must re-check and loop — this is not an error, it is the compare-and-block contract)
///     or when the fixed bucket pool is exhausted, `-EINVAL` off a scheduled task.
///   * `FUTEX_WAKE`: wake up to `val` waiters on the key; returns the count woken.
///
/// `-EAGAIN` for BOTH the mismatch and the pool-exhausted cases is the aarch64 twin's choice and is
/// kept for wire compatibility: a correct futex user re-checks its condition and retries on `-EAGAIN`,
/// which is the right behaviour under either cause, and merging them keeps one arch-neutral EL0 loop.
fn sys_futex(uaddr: u64, op: u64, val: u64) -> i64 {
    if uaddr & 3 != 0 || user_range_ok(uaddr, 4, UserAccess::Write).is_err() {
        return EFAULT;
    }
    let Some(slot) = crate::arch::x86_64::memory::current_slot() else {
        return EINVAL; // no private address space => no futex domain
    };
    let key = crate::arch::sched::futex_key(slot, uaddr);
    match op {
        FUTEX_WAIT => match crate::arch::sched::futex_wait(key, uaddr, val as u32) {
            crate::arch::sched::FutexWait::Woken => {
                // WINX-7: count only the WOKEN outcome. This is the counter a witness can gate on,
                // and the distinction matters: `Mismatch` means the caller never slept (the value had
                // already moved), so counting it would let a pure spin masquerade as a working futex.
                // `Woken` is the one outcome that PROVES a task parked and a `FUTEX_WAKE` released it.
                //
                // It replaces sampling `sched::futex_parked_total()` from the launcher, which was the
                // first cut and was inherently racy: a park that begins and ends between two samples
                // is invisible, so a healthy run could report "no park observed" purely on timing.
                FUTEX_PARKS.fetch_add(1, Ordering::AcqRel);
                0
            }
            crate::arch::sched::FutexWait::Mismatch => EAGAIN,
            crate::arch::sched::FutexWait::TableFull => EAGAIN,
            crate::arch::sched::FutexWait::NoTask => EINVAL,
            // TEARDOWN-1: the caller has an armed kill, so the park was refused. Report SUCCESS rather
            // than an errno: a spurious wake is a legal futex outcome that every correct wait loop
            // already re-checks, whereas an errno could send a program down an error path. It does not
            // matter which the ring-3 loop does — the task retires at the kill boundary in
            // `syscall_dispatch` before it can execute another instruction at ring 3.
            crate::arch::sched::FutexWait::Killed => 0,
        },
        FUTEX_WAKE => crate::arch::sched::futex_wake(key, val as usize) as i64,
        _ => EINVAL,
    }
}

// =============================================================================
// SOCK-2: the UDP socket syscall family (x86-only, knob-on). Each is a thin, IF-masked-safe wrapper
// over the persistent smolnet stack (`crate::smolnet`) with the same object-table capability gate the
// File syscalls use: a socket handle is `KIND_SOCKET` carrying `CAP_READ|CAP_WRITE`; send needs
// `CAP_WRITE`, recv needs `CAP_READ`, so `SYS_CAP` GRANT can mint a send-only or recv-only socket cap.
// User buffers are bound-checked against the ring-3 window EXACTLY like `sys_write`/`sys_open` (no
// copy_from_user yet — the same cheap window check). The socket-id lives in the handle value word,
// `+1`-biased so it is never the 0(Empty)/u64::MAX(RESERVING) sentinel.
// =============================================================================

/// SYS_SOCKET(domain, type, proto) -> a socket HANDLE index, or a negative errno. UDP/IPv4 only this
/// arc: `domain` (AF_INET=2), `type` (SOCK_DGRAM=2), `proto` (0 or IPPROTO_UDP=17) are validated but
/// the only supported tuple is a UDP datagram socket. Allocates a socket in the persistent `SocketSet`
/// (owned by the caller's row for teardown), then mints a `KIND_SOCKET` handle carrying
/// `CAP_READ|CAP_WRITE`. `SHARED_ROW` (the kernel window) is refused — sockets are process-scoped,
/// freed at the owning slot's teardown (which `SHARED_ROW` never gets).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_socket(domain: u64, ty: u64, proto: u64) -> i64 {
    // AF_INET; SOCK_STREAM(1)=TCP (SOCK-3) or SOCK_DGRAM(2)=UDP (SOCK-2); proto 0 / IPPROTO_TCP(6) /
    // IPPROTO_UDP(17). The `type` drives the transport. Anything else is unsupported.
    if domain != 2 || (ty != 1 && ty != 2) || (proto != 0 && proto != 6 && proto != 17) {
        return EINVAL;
    }
    let row = caller_row();
    if row == SHARED_ROW {
        return EACCES; // no process-scoped socket in the shared kernel window (no teardown to free it)
    }
    let is_tcp = ty == 1;
    let opened = if is_tcp {
        crate::smolnet::stack_open_tcp(row)
    } else {
        crate::smolnet::stack_open(row)
    };
    let Some(sid) = opened else {
        return EMFILE; // the persistent socket set is full (too many concurrent sockets) / no NIC
    };
    let Some(h) = handle_install(row, HANDLE_RESERVING) else {
        crate::smolnet::stack_close(sid); // no handle slot — release the socket we just claimed (no leak)
        return EAGAIN;
    };
    // Publish kind + rights, then the live value LAST (Release) — a resolver that sees the live value
    // also sees Socket + its rights. Value = the GEN-FENCED socket-id `(gen << 32) | (sid + 1)` (never
    // the 0/RESERVING sentinels; the gen half rejects a stale handle to a reused registry slot).
    // SOCK-4: the mint now carries `CAP_GRANT` — the delegation right — so the OWNER may hand its own
    // socket to another principal via `SYS_XFER` / `SYS_CAP` GRANT (a socket is the process's own
    // resource; `CAP_GRANT` cannot be self-added later, so transferability must be endowed at mint).
    // The gen fence (SOCK-3) + owner migration (`sys_recv`) make the transfer safe by construction — a
    // stale cross-row handle can never rebind to a recycled slot. Send still needs `CAP_WRITE`, recv
    // `CAP_READ`, so a transfer/grant can still ATTENUATE to send-only / recv-only, and dropping
    // `CAP_GRANT` on transfer (single-level) keeps the grantee from re-delegating.
    handle_set_kind(row, h, KIND_SOCKET);
    handle_set_rights(row, h, CAP_READ | CAP_WRITE | CAP_GRANT);
    handle_set(row, h, sock_id_pack(sid));
    h as i64
}

/// SOCK-3: pack a socket registry slot into its handle value word `(gen << 32) | (sid + 1)` — the U11x
/// file-id discipline (the `+1` low half keeps the word clear of the 0/`u64::MAX` sentinels; the gen
/// high half is the recycled-slot fence). Read at `sys_socket` mint time; decoded + validated in
/// `socket_id_of`. A gen-0 socket packs to exactly `sid + 1` — byte-identical to the pre-SOCK-3 bare id.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sock_id_pack(sid: usize) -> u64 {
    ((crate::smolnet::sock_gen(sid) as u64) << 32) | ((sid + 1) as u64)
}

/// Decode a socket HANDLE carrying ALL of `req` into its persistent socket-id, or an errno. The single
/// enforcement point: a non-Socket kind / a missing right / no handle all fail closed as `-EACCES` (the
/// `sys_read`/`sys_write` idiom via `handle_resolve`). SOCK-3: after the kind+rights CHECK, the packed
/// `(gen, sid)` is validated against the LIVE registry (`smolnet::sock_valid`: present, owner-matched,
/// generation-matched) — so a stale handle to a freed+reused slot is rejected, no rebind (the U11x fence).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn socket_id_of(row: usize, handle: u64, req: u32) -> Result<usize, i64> {
    match handle_resolve(row, handle, req) {
        Ok(HandleTarget::Socket(raw)) => {
            let sid = ((raw & 0xFFFF_FFFF) as usize).checked_sub(1).ok_or(EACCES)?; // undo the +1 bias
            let generation = (raw >> 32) as u32;
            if crate::smolnet::sock_valid(row, sid, generation) {
                Ok(sid)
            } else {
                Err(EACCES) // stale (freed+reused), foreign, or free registry slot
            }
        }
        _ => Err(EACCES),
    }
}

/// SOCK-4: when `sys_recv` installs a received `KIND_SOCKET` cap, MOVE the persistent socket's registry
/// ownership to the receiving row so the moved cap resolves (`sock_valid` is owner-scoped). Decodes the
/// gen-fenced socket-id out of the handle value word `target` (the `socket_id_of` decode) and calls
/// `smolnet::reassign_owner`, which reassigns iff the slot is still present at the SAME generation AND
/// still owned by `from_row` — the transfer's SENDER (from the record; a disowned record's `u64::MAX`
/// sender matches no row). A non-Socket kind is a no-op; a stale deposit (socket freed+reused since the
/// transfer) fails the gen check, and a deposit whose sender no longer owns the socket (it already MOVED
/// to an earlier grantee — the sender's residual `CAP_GRANT` handle must not steal it back out from
/// under the current owner) fails the owner check: either way the received handle stays dead (fails
/// `sock_valid`) — never rebinding to a different tenant, never re-migrating a moved socket.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn xfer_socket_migrate(kind: u8, target: u64, from_row: u64, new_row: usize) {
    if kind != KIND_SOCKET {
        return;
    }
    let Ok(from_row) = usize::try_from(from_row) else {
        return; // a disowned record (u64::MAX sender) owns nothing on 32-bit-usize targets either
    };
    if let Some(sid) = ((target & 0xFFFF_FFFF) as usize).checked_sub(1) {
        let generation = (target >> 32) as u32;
        crate::smolnet::reassign_owner(sid, generation, from_row, new_row);
    }
}

/// SYS_BIND(handle, port) -> 0, or a negative errno. Binds the socket the handle names to a local UDP
/// `port`. Requires a Socket handle carrying `CAP_WRITE` (binding is a configuring authority). A port
/// of 0 or an already-bound/open socket is `-EINVAL` (smoltcp refuses it). No I/O — descriptor state
/// only, IF-masked-handler-safe.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_bind(handle: u64, port: u64) -> i64 {
    let row = caller_row();
    let sid = match socket_id_of(row, handle, CAP_WRITE) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if port == 0 || port > u16::MAX as u64 {
        return EINVAL;
    }
    match crate::smolnet::stack_bind(sid, port as u16) {
        Ok(()) => 0,
        Err(()) => EINVAL,
    }
}

/// SYS_SENDTO(handle, msg_ptr, msg_len) -> bytes sent, or a negative errno. `msg` is an 8-byte header
/// `[dst_ip[4]][dst_port u16 LE][pad u16]` followed by the payload. Requires a Socket handle carrying
/// `CAP_WRITE`. The WHOLE `msg` range is bound-checked inside the ring-3 window (the `sys_write`
/// pointer discipline) before any read; the payload is clamped to `UDP_MAX_PAYLOAD`. `-EAGAIN` if the
/// socket can't accept the datagram (unbound / TX buffer full). Non-blocking: smolnet pumps a bounded
/// egress loop, never blocks.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_sendto(handle: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let row = caller_row();
    let sid = match socket_id_of(row, handle, CAP_WRITE) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if msg_len < 8 {
        return EINVAL; // no room even for the address header
    }
    // CFU-1: validate the WHOLE `msg` range READABLE in the ring-3 window (the code page is a legal send
    // source) BEFORE any deref — the `sys_write` read-source discipline, now the unified seam.
    if let Err(e) = user_range_ok(msg_ptr, msg_len, UserAccess::Read) {
        return e;
    }
    let plen = (msg_len - 8) as usize;
    if plen > crate::smolnet::UDP_MAX_PAYLOAD {
        return EINVAL;
    }
    // Validate-only site (the seam borrows, it does not copy into a kernel buffer): the whole range is
    // proven in-window above, and ring-3 VA == kernel VA in the live CR3, so the header + payload are
    // read in place and handed to smolnet (which copies them into its tx buffers synchronously).
    let hdr = unsafe { core::slice::from_raw_parts(msg_ptr as *const u8, 8) };
    let ip = [hdr[0], hdr[1], hdr[2], hdr[3]];
    let port = u16::from_le_bytes([hdr[4], hdr[5]]);
    let payload = unsafe { core::slice::from_raw_parts((msg_ptr + 8) as *const u8, plen) };
    match crate::smolnet::stack_sendto(sid, ip, port, payload) {
        Ok(n) => n as i64,
        Err(()) => EAGAIN,
    }
}

/// SYS_RECVFROM(handle, buf_ptr, buf_len) -> total bytes written (header + payload), or a negative
/// errno. Writes the same 8-byte `[src_ip[4]][src_port u16 LE][pad u16]` header + payload into the
/// caller's buffer and returns the total. Requires a Socket handle carrying `CAP_READ`. `buf_len` must
/// be >= 8 (room for the header); the payload is truncated to fit. `-EAGAIN` when no datagram is
/// available (NON-BLOCKING — the IF-masked handler drives a bounded poll pump and returns rather than
/// waiting). The WHOLE dest range is bound-checked writable in the ring-3 window before any store.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_recvfrom(handle: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let row = caller_row();
    let sid = match socket_id_of(row, handle, CAP_READ) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if buf_len < 8 {
        return EINVAL;
    }
    // CFU-1: recvfrom is a WRITE path (it stores the source header + payload into `buf_ptr`), so validate
    // the WHOLE declared dest as WRITABLE user memory up front — inside the window AND past the read-only
    // code page (`UserAccess::Write` = the `USER_BASE + PAGE_SIZE` lower bound). Validating the full
    // `buf_len` (not just the 8 + n bytes actually written) keeps the -EFAULT surface byte-identical to
    // the open-coded predicate this replaces.
    if let Err(e) = user_range_ok(buf_ptr, buf_len, UserAccess::Write) {
        return e;
    }
    let cap = ((buf_len - 8) as usize).min(crate::smolnet::UDP_MAX_PAYLOAD);
    let mut kbuf = [0u8; crate::smolnet::UDP_MAX_PAYLOAD];
    let Some((ip, port, n)) = crate::smolnet::stack_recvfrom(sid, &mut kbuf[..cap]) else {
        return EAGAIN;
    };
    // Write the source-address header + payload out through the WRITE seam. The 8-byte header
    // `[src_ip[4]][src_port u16 LE][pad u16]` then the payload (`n <= cap <= buf_len - 8`, so both
    // subranges lie inside the buffer validated above — `copy_to_user` re-checks and cannot fail here).
    let pbytes = port.to_le_bytes();
    let hdr = [ip[0], ip[1], ip[2], ip[3], pbytes[0], pbytes[1], 0, 0];
    if let Err(e) = copy_to_user(buf_ptr, &hdr) {
        return e;
    }
    if let Err(e) = copy_to_user(buf_ptr + 8, &kbuf[..n]) {
        return e;
    }
    (8 + n) as i64
}

/// SYS_CONNECT(handle, msg_ptr, msg_len) -> `0` (ESTABLISHED), or a negative errno. `msg` is the 8-byte
/// header `[dst_ip[4]][dst_port u16 LE][pad u16]` naming the peer. Requires a TCP Socket handle carrying
/// `CAP_WRITE` (an active open is a configuring authority, like bind). NON-BLOCKING with a ring-3 poll
/// model: the first call issues the SYN and pumps a bounded loop chasing the handshake; a re-call while
/// SYN-SENT just pumps further. `-EINPROGRESS` = still handshaking (ring 3 re-invokes connect);
/// `-ECONNREFUSED` = the peer reset / refused; `-EACCES` = wrong kind (a UDP socket) / stale / no right.
/// The 8-byte `msg` is bound-checked in the ring-3 window (the `sys_sendto` pointer discipline) before
/// any deref. Never blocks the IF-masked handler.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_connect(handle: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let row = caller_row();
    let sid = match socket_id_of(row, handle, CAP_WRITE) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if msg_len < 8 {
        return EINVAL; // no room for the address header
    }
    // CFU-1: validate the 8-byte address header READABLE in the ring-3 window (as `sys_sendto`; the code
    // page is a legal source). Only the header is read here — the payload discipline is `sys_send`'s.
    if let Err(e) = user_range_ok(msg_ptr, 8, UserAccess::Read) {
        return e;
    }
    // Validate-only site: the 8-byte header is proven in-window above and ring-3 VA == kernel VA in the
    // live CR3, so it is read in place.
    let hdr = unsafe { core::slice::from_raw_parts(msg_ptr as *const u8, 8) };
    let ip = [hdr[0], hdr[1], hdr[2], hdr[3]];
    let port = u16::from_le_bytes([hdr[4], hdr[5]]);
    if port == 0 {
        return EINVAL;
    }
    match crate::smolnet::stack_connect(sid, ip, port) {
        crate::smolnet::ConnectOutcome::Established => 0,
        crate::smolnet::ConnectOutcome::InProgress => EINPROGRESS,
        crate::smolnet::ConnectOutcome::Refused => ECONNREFUSED,
    }
}

/// SYS_SEND(handle, buf_ptr, buf_len) -> bytes queued, or a negative errno. Streams `buf_len` bytes on a
/// connected TCP socket (no per-call address — a stream is connected). Requires a TCP Socket handle
/// carrying `CAP_WRITE`. Clamped to `TCP_MAX_CHUNK` (a stream is resumable — ring 3 loops for more).
/// `-EAGAIN` = the tx ring is momentarily full (retry); `-ENOTCONN` = not established / send half closed.
/// The whole `buf` range is bound-checked in the ring-3 window (the `sys_write`/`sys_sendto` read-source
/// discipline: `>= USER_BASE`, the RO code page is a legal send source) before any read. Non-blocking.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_send(handle: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let row = caller_row();
    let sid = match socket_id_of(row, handle, CAP_WRITE) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if buf_len == 0 {
        return 0;
    }
    // CFU-1: validate the WHOLE `buf` range READABLE in the ring-3 window (the code page is a legal send
    // source) — the `sys_write`/`sys_sendto` read-source discipline. The full `buf_len` is validated even
    // though only the clamped `len` is streamed, keeping the -EFAULT surface byte-identical.
    if let Err(e) = user_range_ok(buf_ptr, buf_len, UserAccess::Read) {
        return e;
    }
    let len = (buf_len as usize).min(crate::smolnet::TCP_MAX_CHUNK);
    // Validate-only site: the range is proven in-window above; smolnet copies the borrowed slice into its
    // tx ring synchronously.
    let data = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };
    match crate::smolnet::stack_send(sid, data) {
        Ok(n) => n as i64,
        Err(true) => EAGAIN,   // tx ring full right now
        Err(false) => ENOTCONN, // not connected / send half closed
    }
}

/// SYS_SOCK_RECV(handle, buf_ptr, buf_len) -> bytes read, `0` at end-of-stream, or a negative errno. Reads up
/// to `buf_len` stream bytes on a connected TCP socket (no address header — a stream is connected).
/// Requires a TCP Socket handle carrying `CAP_READ`. NON-BLOCKING: drives a BOUNDED poll pump and returns
/// the bytes read, `-EAGAIN` when none is available yet (connection still open), or `0` once the peer's
/// FIN is delivered and the rx ring is drained (clean end-of-stream). The whole dest range is
/// bound-checked WRITABLE (`>= USER_BASE + PAGE_SIZE`, past the RO code page — the `sys_read`/`sys_recvfrom`
/// write-dest discipline) before any store.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_sock_recv(handle: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let row = caller_row();
    let sid = match socket_id_of(row, handle, CAP_READ) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if buf_len == 0 {
        return 0;
    }
    // CFU-1: a WRITE path — validate the whole dest WRITABLE (inside the window AND past the RO code page,
    // `UserAccess::Write`'s `USER_BASE + PAGE_SIZE` lower bound) up front.
    if let Err(e) = user_range_ok(buf_ptr, buf_len, UserAccess::Write) {
        return e;
    }
    let cap = (buf_len as usize).min(crate::smolnet::TCP_MAX_CHUNK);
    let mut kbuf = [0u8; crate::smolnet::TCP_MAX_CHUNK];
    match crate::smolnet::stack_recv(sid, &mut kbuf[..cap]) {
        crate::smolnet::RecvOutcome::Data(n) => {
            // Copy the stream bytes out through the WRITE seam (`n <= cap <= buf_len`, a subrange of the
            // range validated above — `copy_to_user` re-checks and cannot fail here).
            if let Err(e) = copy_to_user(buf_ptr, &kbuf[..n]) {
                return e;
            }
            n as i64
        }
        crate::smolnet::RecvOutcome::WouldBlock => EAGAIN,
        crate::smolnet::RecvOutcome::Eof => 0,
    }
}

/// SYS_LISTEN(handle, port) -> `0`, or a negative errno. Arms a TCP Socket as a passive LISTENER on a
/// local `port` (the server side of the stack). Requires a Socket handle carrying `CAP_WRITE` (arming a
/// listener is a configuring authority, like `bind`/`connect`). `-EINVAL` if the port is 0/out of range,
/// or smoltcp refuses (the socket is already open / connected / not TCP). No I/O — descriptor state only,
/// IF-masked-handler-safe. NOTE: `sys_accept` is where the ring 3 first ACCEPTS inbound TCP.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_listen(handle: u64, port: u64) -> i64 {
    let row = caller_row();
    let sid = match socket_id_of(row, handle, CAP_WRITE) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if port == 0 || port > u16::MAX as u64 {
        return EINVAL;
    }
    match crate::smolnet::stack_listen(sid, port as u16) {
        Ok(()) => 0,
        Err(()) => EINVAL,
    }
}

/// SYS_ACCEPT(handle) -> a FRESH socket HANDLE for the accepted connection, or a negative errno. Requires
/// a listening Socket handle carrying `CAP_READ` (accepting an inbound connection is a receiving
/// authority). NON-BLOCKING with a ring-3 poll model like `sys_connect`: pumps a bounded loop chasing an
/// inbound handshake; `-EAGAIN` = none yet (ring 3 re-invokes accept); `-EINVAL` = the socket is not
/// armed for listen (never listened / already closed / wrong kind). SOCK-7 (PERSISTENT LISTENER): on
/// success the established connection is PEELED into a FRESH gen-fenced socket-id and the LISTENER is
/// re-armed in place, so the listener handle stays valid and can be `sys_accept`'d again — each call
/// mints a `KIND_SOCKET` handle for the NEW connection socket-id (not an alias of the listener). The
/// minted rights are the INTERSECTION of `CAP_READ|CAP_WRITE|CAP_GRANT` with the LISTENER handle's
/// current rights — accept is a derivation, not a mint-from-nothing, so an ATTENUATED listener (e.g. a
/// `CAP_READ`-only SOCK-4 grantee) cannot amplify itself a full-rights connection (the SOCK-4
/// attenuation boundary: any bit the holder does not have is an amplification). A full-rights owner
/// gets the POSIX-like full connection handle and may `SYS_XFER` it to a handler (inetd-style) while the
/// listener keeps accepting.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sys_accept(handle: u64) -> i64 {
    let row = caller_row();
    let sid = match socket_id_of(row, handle, CAP_READ) {
        Ok(s) => s,
        Err(e) => return e,
    };
    match crate::smolnet::stack_accept(sid) {
        crate::smolnet::AcceptOutcome::Connected(conn_sid) => {
            // Mint a fresh handle for the PEELED connection (its own gen-fenced socket-id; the listener
            // socket-id `sid` remains armed and its handle valid). The connection occupies its own reg
            // slot + stream buffers; the listener was re-armed on a fresh buffer set inside `stack_accept`.
            let Some(h) = handle_install(row, HANDLE_RESERVING) else {
                // No handle slot free — the peeled connection is owned by this row and reachable on the
                // next accept-drained slot; ring 3 retries for a handle slot. (The connection is not lost:
                // it lives in its reg slot until the row's teardown or an explicit close reaps it.)
                return EAGAIN;
            };
            handle_set_kind(row, h, KIND_SOCKET);
            // Derive, don't mint: intersect with the listener handle's CURRENT rights so an attenuated
            // listener (SOCK-4 reduced-rights transfer) cannot amplify into a full-rights connection.
            let listener_rights =
                HANDLE_RIGHTS[row][handle as usize].load(Ordering::Acquire);
            handle_set_rights(row, h, (CAP_READ | CAP_WRITE | CAP_GRANT) & listener_rights);
            handle_set(row, h, sock_id_pack(conn_sid));
            h as i64
        }
        crate::smolnet::AcceptOutcome::Pending => EAGAIN,
        crate::smolnet::AcceptOutcome::NotListening => EINVAL,
    }
}

/// SYS_WRITE(fd, buf, len): write `len` bytes from the ring-3 buffer, returning the count written or a
/// negative errno. U9x makes this KIND-DISPATCHED at the single CAP_WRITE CHECK (the aarch64 U9 twin): a
/// `Console` handle streams to the serial console (byte-identical to before); a `File` handle carrying
/// CAP_WRITE overwrites its per-descriptor writable staging buffer IN PLACE at the descriptor's offset via
/// `sys_write_file` — a pure memcpy, IF-masked-handler-safe (no disk I/O in the SYSCALL handler; see the
/// STAGED WRITE-BACK note above `sys_write_file`).
///
/// The console pointer is a ring-3 VA that (identity map) equals the kernel VA, so the kernel reads it
/// directly — BUT it is UNTRUSTED, so it is bound-checked against the user window before the deref:
/// a ring-3 caller must not be able to point `buf` at kernel RAM (exfiltration out the console) or
/// at unmapped memory (a ring-0 fault). Full copy_from_user is a later arc; this closes the hole
/// cheaply. Emitted through the standard console path (`serial_print!` -> UART **and** framebuffer
/// mirror) so the line is visible on a serial-less machine (fbcon) too, not only in QEMU's
/// serial.log — the rMBP has no 16550, so a UART-only write would vanish. The demo runs in a
/// BSP-quiet window (see `await_verdict`), so the best-effort console lock is uncontended here and
/// the line is not dropped.
fn sys_write(fd: u64, buf: u64, len: u64) -> i64 {
    // U5x/U9x: `fd` is a HANDLE INDEX into the caller's per-process table, not the ambient POSIX stdout. It
    // must resolve to a resource carrying CAP_WRITE. No such handle / a File LACKING CAP_WRITE (an RO open) /
    // a non-{Console,File} kind all yield -EACCES — the single enforcement point (subsuming U1a's `fd != 1 ->
    // -EBADF`; the U8x derivation/revocation walk rides inside `handle_resolve`, so a revoked File-write cap
    // is -EACCES here too). A `Console` is endowed at spawn/launch (`install_console_cap`) and streams below;
    // a `File` with CAP_WRITE routes to the in-place staged-buffer writer. A hostile pointer still -> -EFAULT.
    let row = caller_row();
    match handle_resolve(row, fd, CAP_WRITE) {
        Ok(HandleTarget::Console) => {} // fall through to the console-streaming path below
        Ok(HandleTarget::File(file_id)) => return sys_write_file(row, file_id, buf, len),
        _ => return EACCES,
    }
    // CFU-1: validate the whole console range READABLE in the user window (overflow-safe) BEFORE the
    // deref — a ring-3 caller must not point `buf` at kernel RAM (exfiltration out the console) or at
    // unmapped memory (a ring-0 fault). The seam borrows the bytes in place (the console sink consumes a
    // &str); it does not copy into a kernel buffer, so this is a validate-only site.
    if let Err(e) = user_range_ok(buf, len, UserAccess::Read) {
        return e;
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

/// U9x: the File half of `sys_write` — an IN-PLACE overwrite at the descriptor's offset, into the
/// descriptor's per-descriptor WRITABLE staging buffer (the write twin of `sys_read`'s staged-source read).
/// The CHECK already passed in `sys_write` (a `File` handle carrying CAP_WRITE, non-revoked), so this decodes
/// the file-id -> descriptor, clamps the write to the bytes left to EOF (NEVER grows the file), validates the
/// WHOLE source up front (a bad buffer is -EFAULT with no copy and NO offset move), and memcpys the bytes into
/// the writable buffer at `offset` — a pure in-memory copy, safe in the IF-masked SYSCALL handler (the whole
/// reason x86 STAGES the write instead of driving the xHCI BOT pump in-handler; see the STORAGE / IF NOTE at
/// the pre-stage buffer). M1 scope: purely in-memory — a read-back through the same cap witnesses it; there is
/// NO disk write-back here (that is M2, a BSP-side flush pump). The exact write twin of `sys_read`: same
/// decode, same up-front clamp/validate, same offset discipline. No create/grow/truncate — a write at/after
/// EOF writes 0 bytes (a later arc adds allocation).
fn sys_write_file(row: usize, file_id: u64, buf: u64, len: u64) -> i64 {
    // U11x: decode + validate the file-id through the ONE seam (undo the +1 bias, bounds + presence + generation
    // — a stale sibling to a reused slot is rejected). `None` -> -EACCES. Mirrors sys_read.
    let Some(idx) = file_desc_validate(row, file_id) else {
        return EACCES;
    };
    // STOR-1 S8/S9 (irqstorage): a DYNAMIC on-disk descriptor (`open_dynamic_ondisk` — a pre-existing file
    // neither staged nor a U10 name) opened RW writes THROUGH to the LIVE volume BY its stored NAME,
    // synchronously. Keyed on `FILE_DYNLEN != 0` (set ONLY knob-on, so a knob-off build never enters this
    // branch). A dynamic descriptor owns NO wstage (FILE_WSTAGE == 0), so it MUST resolve here — BEFORE the
    // `FILE_WSTAGE == 0 -> EIO` guard below, which would otherwise reject it. TWO regimes: an IN-EOF write
    // (`offset + len <= size`) is the S8 OVERWRITE path — `want` clamps to `[offset, EOF)` and `write_at`
    // (by contract no alloc, no directory touch) persists it in place; a write that extends PAST EOF is the
    // S9 GROW path (`dyn_write_grow`), which allocates + chains + bumps the on-disk size via `write_grow`,
    // bounded per-write (`DYN_GROW_MAX`) and per-file (`DYN_FILE_MAX`). No `ns_lock` EITHER way (a dynamic name
    // is outside the U10 mutation namespace — the S5 deadlock class stays closed).
    #[cfg(feature = "irqstorage")]
    if FILE_DYNLEN[row][idx].load(Ordering::Acquire) != 0 {
        // Only ever created knob-on with the service up; fail closed defensively if the service is somehow not
        // ready (never reached — the open refuses the dynamic path unless `s4_sync_storage()`).
        if !crate::drivers::xhci::irqstorage::service_ready() {
            return EIO;
        }
        let size = FILE_SIZE[row][idx].load(Ordering::Acquire);
        // STOR-1 S9: a write that extends PAST EOF now GROWS the file synchronously (S8 returned 0 here — the
        // overwrite-only constraint this arc retires). A dynamic RW descriptor lives only in a PRIVATE
        // single-writer slot (`open_dynamic_ondisk` refuses RW on SHARED_ROW, mirroring `sys_open_staged`), so
        // the offset load races nothing and the grow needs no CAS. `dyn_write_grow` bounds + persists it live
        // (whole-op-or-error); a NON-growing write (`offset + len <= size`) falls through to the UNCHANGED S8
        // overwrite path below. u64 math: `len` is the raw syscall arg; `offset`/`size` are u32.
        let cur = FILE_OFFSET[row][idx].load(Ordering::Acquire);
        if cur as u64 + len > size as u64 {
            return dyn_write_grow(row, idx, cur, size, buf, len);
        }
        // CAS-claim `[offset, offset+want)` exactly as `sys_read`/the in-place write below: clamp `want` to the
        // bytes left to EOF (overwrite-only — a past-EOF write is 0, never a grow), and validate the WHOLE
        // source range BEFORE the claim (a bad buffer is -EFAULT with NO claim, no copy, no offset move; the
        // code page is a legal RX source, so `UserAccess::Read` is the correct lower bound). CFU-1: the
        // validation is the unified `user_range_ok` seam; the per-page copies below route through `copy_from_user`.
        let (offset, want) = loop {
            let offset = FILE_OFFSET[row][idx].load(Ordering::Acquire);
            let want = core::cmp::min(len as usize, size.saturating_sub(offset) as usize);
            if want == 0 {
                return 0; // at/after EOF (never grows) or nothing requested — a clean no-op, no offset move
            }
            if let Err(e) = user_range_ok(buf, want as u64, UserAccess::Read) {
                return e;
            }
            if FILE_OFFSET[row][idx]
                .compare_exchange(offset, offset + want as u32, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break (offset, want); // [offset, offset+want) is now exclusively ours
            }
        };
        // Bounce page-at-a-time through a kernel-stack buffer (the service task's kernel CR3 cannot reach the
        // ring-3 window): copy user->kbuf, then write it live at `offset+done`. ANY submit error OR a short
        // chunk (`n < chunk`) fails the WHOLE write -EIO — NEVER a masked partial (the offset was CAS-advanced
        // by the full `want`, so returning `done` would silently skip `[done, want)` on the next sequential
        // write AND hide the error; EIO leaves the offset advanced — the ledgered N1 pattern, as the read
        // branch documents). A short write is a REAL error here: the on-disk size == captured FILE_SIZE (nothing
        // grows/unlinks a non-U10 name), so `write_at` delivers exactly `chunk` on success.
        let mut kbuf = [0u8; PAGE_SIZE as usize];
        let mut done = 0usize;
        while done < want {
            let chunk = core::cmp::min(PAGE_SIZE as usize, want - done);
            // CFU-1: pull this chunk in through the READ seam. `[buf+done, buf+done+chunk)` is a subrange of
            // the range validated before the CAS claim above, so this re-check cannot fail here.
            if let Err(e) = copy_from_user(&mut kbuf[..chunk], buf + done as u64) {
                return e;
            }
            let n = unsafe { dyn_write_live(row, idx, offset + done as u32, kbuf.as_mut_ptr(), chunk) };
            if n < 0 || (n as usize) < chunk {
                return EIO;
            }
            done += chunk;
        }
        return want as i64;
    }
    // A File+CAP_WRITE descriptor is always RW-opened, so it owns a writable staging slot (FILE_WSTAGE holds
    // the +1-biased pool index). A File+CAP_WRITE handle with NO writable buffer (FILE_WSTAGE == 0) is a kernel
    // setup bug (an RO descriptor endowed CAP_WRITE out of band) — fail closed, never fabricate a write target.
    let Some(widx) = (FILE_WSTAGE[row][idx].load(Ordering::Acquire) as usize).checked_sub(1) else {
        return EIO;
    };
    let size = FILE_SIZE[row][idx].load(Ordering::Acquire);
    // U10 M1: a GROWABLE descriptor (FILE_OPNAME set at open — GROW.BIN or a runtime-CREATED file; NEVER
    // SCRATCH.BIN, which has no opname) whose write runs PAST the current EOF grows the file. The extend lands
    // in memory here (bump FILE_SIZE + the wstage buffer); the disk alloc + FAT chain + dir-size bump DEFER to
    // the launcher's IF=1 drain (the IF-masked handler cannot drive the xHCI BOT pump). A NON-growable
    // descriptor keeps the U9x clamp-to-EOF path below UNCHANGED (a past-EOF write is a short/0 write, never a
    // grow) — so a RW SCRATCH.BIN holder can never mint a grow, and the deferred op always names THIS file.
    let offset0 = FILE_OFFSET[row][idx].load(Ordering::Acquire);
    let inplace_avail = size.saturating_sub(offset0) as usize;
    if FILE_OPNAME[row][idx].load(Ordering::Acquire) != 0 && (len as usize) > inplace_avail {
        return sys_write_grow(row, idx, widx, size, offset0, buf, len);
    }
    // U9x M2 (folding the M1 review's offset-CAS note): claim the write range with a tx-exact
    // `compare_exchange`, EXACTLY as `sys_read` claims its read range — so the write offset advance is
    // CAS-symmetric with the read path (closing the M1 load/store asymmetry before any shared writable
    // descriptor exists). In-place only: `want` clamps to the bytes left from `offset` to EOF (`offset <= size`
    // always — reads/writes clamp, seek rejects past size — so `size - offset` never underflows; a write
    // at/after EOF is 0 bytes, never grows). The WHOLE source range is validated BEFORE the claim (inside the
    // user window, readable — the code page is RX, so the fixture's RO pattern constant is a legal source; a
    // bad buffer is -EFAULT with NO claim, no copy, no offset move). Single-writer per private slot (one
    // IF-masked syscall at a time), so the CAS never retries here — the symmetry is the point. CFU-1: the
    // validation is the unified `user_range_ok` READ seam (the code page is a legal RX source).
    let (offset, want) = loop {
        let offset = FILE_OFFSET[row][idx].load(Ordering::Acquire);
        let want = core::cmp::min(len as usize, size.saturating_sub(offset) as usize);
        if want == 0 {
            return 0; // at/after EOF (never grows) or nothing requested — a clean no-op, no offset move
        }
        if let Err(e) = user_range_ok(buf, want as u64, UserAccess::Read) {
            return e;
        }
        if FILE_OFFSET[row][idx]
            .compare_exchange(offset, offset + want as u32, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break (offset, want); // [offset, offset+want) is now exclusively ours
        }
    };
    // S3 (irqstorage): a non-growable staged descriptor (SCRATCH.BIN, FILE_OPNAME == 0) writes THROUGH
    // to the live volume synchronously via the storage service task — in place, out of the IF-masked
    // handler — retiring the wstage buffer + the U9x FLUSH queue for this write. Nothing is staged or
    // marked dirty, so a SYS_CLOSE / teardown of this descriptor discards nothing (the close-discards-
    // dirty residual): the write is already on disk. Gated on a mounted FAT volume + the service task;
    // the no-FAT core and created/growable descriptors fall through to the staged write below.
    #[cfg(feature = "irqstorage")]
    if FILE_OPNAME[row][idx].load(Ordering::Acquire) == 0
        && HELLO_STAGED.load(Ordering::Acquire)
        && crate::drivers::xhci::irqstorage::service_ready()
    {
        let sidx = FILE_STAGED[row][idx].load(Ordering::Acquire) as usize;
        if let Some(name) = STAGED_NAMES.get(sidx) {
            // Bounce the ring-3 source (validated in the CAS claim above) into a kernel-stack buffer the
            // service task can reach; write_at persists it in place. `want <= PAGE_SIZE` (staged bound).
            // CFU-1: the bounce is the READ seam (`want` is a subrange of the validated range — cannot fail).
            let mut kbuf = [0u8; PAGE_SIZE as usize];
            if let Err(e) = copy_from_user(&mut kbuf[..want], buf) {
                return e;
            }
            let n = unsafe {
                crate::drivers::xhci::irqstorage::submit_write_file(
                    name.as_bytes(), offset, kbuf.as_mut_ptr(), want,
                )
            };
            if n < 0 {
                return EIO;
            }
            // LEDGERED (STOR-1 review note N1): as in sys_read, the offset was CAS-advanced by `want`; a
            // live short write / EIO returns `n <= want`. BENIGN today (in-place write, on-disk size ==
            // captured size, so `n == want`); the advance-by-actual clamp rides S4.
            return n as i64;
        }
    }
    // STOR-1 S4: an IN-PLACE write to a CREATED/growable file (`FILE_OPNAME != 0`, within EOF — the grow
    // branch at the top of this fn already handled a past-EOF write) ALSO writes THROUGH synchronously
    // knob-on: the file is on disk (S4a create + S4b grow), so persist the overwrite in place via `write_at`
    // and MIRROR it into wstage (reads still serve wstage this arc; S5 makes reads read shared backing).
    // WITHOUT this the write would land only in wstage + `mark_dirty`, and the created/grown teardown flush
    // is DISABLED knob-on (`clear_files_row`'s `!s4_sync_storage()` guard), silently DROPPING an
    // acknowledged write on remount — a knob-on durability regression the grow-only demos never exercise.
    // The name resolves via `U10_NAMES` (created/growable files are not in `STAGED_NAMES`).
    #[cfg(feature = "irqstorage")]
    if FILE_OPNAME[row][idx].load(Ordering::Acquire) != 0 && s4_sync_storage() {
        if let Some(nameid) = (FILE_OPNAME[row][idx].load(Ordering::Acquire) as usize).checked_sub(1) {
            if nameid < N_U10_NAMES {
                let name = U10_NAMES[nameid];
                // CFU-1: bounce the ring-3 source in through the READ seam (`want` is a subrange of the
                // range validated in the CAS claim above — cannot fail here).
                let mut kbuf = [0u8; PAGE_SIZE as usize];
                if let Err(e) = copy_from_user(&mut kbuf[..want], buf) {
                    return e;
                }
                let n = unsafe {
                    crate::drivers::xhci::irqstorage::submit_write_file(
                        name.as_bytes(), offset, kbuf.as_mut_ptr(), want,
                    )
                };
                if n < 0 {
                    return EIO;
                }
                let w = (n as usize).min(want);
                // CFU-1 NOTE-1: keep wstage coherent (reads serve it) through the READ seam (`w <= want`, a
                // subrange of the range copied above — cannot fail here).
                if let Err(e) = wstage_write_from_user(widx, offset as usize, buf, w) {
                    return e;
                }
                return w as i64; // no mark_dirty — already on disk
            }
        }
    }
    // Pure memcpy: user bytes -> the writable staging buffer at [offset, offset+want). The ring-3 VA equals the
    // kernel VA in the live CR3 (the sys_write/sys_read discipline), and offset+want <= size <= the buffer
    // length (WSTAGE is seeded to `size` at open), so the store stays inside the descriptor's own buffer.
    // CFU-1 NOTE-1: the ring-3 read is the unified `copy_from_user` seam (`want` is exactly the range the
    // CAS-claim `user_range_ok` validated above — cannot fail here).
    if let Err(e) = wstage_write_from_user(widx, offset as usize, buf, want) {
        return e;
    }
    // U9x M2: mark the descriptor dirty + cover [offset, offset+want) so the task's TEARDOWN flushes exactly
    // the touched bytes to disk (a REVOKE discards them instead — see `files_free`).
    mark_dirty(row, idx, offset, offset + want as u32);
    want as i64
}

/// U10 M1: the GROWTH half of a File write — a write past EOF on a GROWABLE descriptor (`FILE_OPNAME` set). The
/// aarch64 `sys_write_file` grow branch (`fat::write_grow` in-handler) done the x86 way: the extend lands in the
/// descriptor's one-page writable staging buffer HERE (bump FILE_SIZE + WSTAGE_LEN), and the real disk work
/// (`alloc_cluster` + zero-fill + chain, RMW the data, bump the directory size LAST) is DEFERRED to the
/// launcher's IF=1 drain (`u10_drain_grow`) — the IF-masked SYSCALL handler cannot drive the xHCI BOT pump.
/// Single-writer per PRIVATE slot (a growable descriptor is never on SHARED_ROW — RW opens are refused there),
/// so no CAS is needed. The grow stays within ONE page: `want` is bounded by `GROW_WRITE_MAX` and clamped so
/// `offset + want <= PAGE_SIZE`, keeping the load-bearing invariant `FILE_SIZE == WSTAGE_LEN <= PAGE_SIZE` (the
/// one the in-place read/write memcpys rely on). Publishes SIZE/WSTAGE_LEN BEFORE the offset (size-before-offset
/// — FILE_OFFSET <= FILE_SIZE is never even transiently violated). A bad source buffer is `-EFAULT` with no copy
/// / no offset move; the page-full case is `-ENOSPC` (never reached by the 16-byte demo). Returns the count written.
fn sys_write_grow(row: usize, idx: usize, widx: usize, size: u32, offset: u32, buf: u64, len: u64) -> i64 {
    let page = PAGE_SIZE as usize;
    let off = offset as usize;
    // Bound the grown span to GROW_WRITE_MAX and to the one-page staging buffer; RE-derive new_end from the
    // clamped want so FILE_SIZE and WSTAGE_LEN are driven by the SAME value (never desync past the page).
    let mut want = core::cmp::min(len as usize, GROW_WRITE_MAX);
    if off + want > page {
        want = page.saturating_sub(off);
    }
    if want == 0 {
        return ENOSPC; // the one-page staging buffer is full (offset at the page end) — never in the demo
    }
    // CFU-1: validate the (clamped) grow source READABLE in the window (a bad source buffer -> -EFAULT
    // with no copy, no size/offset move) — the unified READ seam.
    if let Err(e) = user_range_ok(buf, want as u64, UserAccess::Read) {
        return e;
    }
    let new_end = offset + want as u32; // <= PAGE_SIZE by the clamp above
    debug_assert!(new_end as usize <= page && new_end > offset, "grow: new_end out of range");
    // STOR-1 S4b: knob-on, GROW the file SYNCHRONOUSLY on the live volume via the storage service task —
    // out of the IF-masked handler (`fat::write_grow`: alloc + zero-fill + chain, RMW the data, bump the dir
    // size LAST) — retiring the deferred U10 Grow op + its causal-fidelity gap. Persist to DISK FIRST; only
    // on success mirror the extend into the descriptor's wstage + FILE_SIZE (reads still serve wstage this
    // arc, S5 makes them read shared backing), so a disk failure (`-EIO`) leaves the descriptor UNCHANGED
    // (never in-memory-ahead-of-disk). Nothing is marked dirty — the write is already durable, so the
    // teardown flush is a no-op (`clear_files_row` also skips the enqueue knob-on). The on-disk `de.size`
    // equals this descriptor's `size` (both created 0-length / staged then grown synchronously in lockstep),
    // so `write_grow` grows from the same base the syscall clamped against.
    #[cfg(feature = "irqstorage")]
    if s4_sync_storage() {
        if let Some(nameid) = (FILE_OPNAME[row][idx].load(Ordering::Acquire) as usize).checked_sub(1) {
            if nameid < N_U10_NAMES {
                let name = U10_NAMES[nameid];
                // CFU-1: bounce the ring-3 grow source in through the READ seam (`want` is a subrange of
                // the range validated above — cannot fail here).
                let mut kbuf = [0u8; PAGE_SIZE as usize];
                if let Err(e) = copy_from_user(&mut kbuf[..want], buf) {
                    return e;
                }
                let n = unsafe {
                    crate::drivers::xhci::irqstorage::submit_grow(name.as_bytes(), offset, kbuf.as_mut_ptr(), want)
                };
                if n < 0 {
                    return EIO; // disk grow failed -> descriptor untouched
                }
                let w = (n as usize).min(want); // bytes actually persisted (== want on a full write)
                let end2 = offset + w as u32;
                // CFU-1 NOTE-1: mirror the persisted grow into wstage through the READ seam (`w <= want`, a
                // subrange of the range copied above — cannot fail here).
                if let Err(e) = wstage_write_from_user(widx, off, buf, w) {
                    return e;
                }
                wstage_set_len_at_least(widx, end2);
                FILE_SIZE[row][idx].store(core::cmp::max(size, end2), Ordering::Release);
                FILE_GREW[row][idx].store(true, Ordering::Release);
                FILE_OFFSET[row][idx].store(end2, Ordering::Release); // offset LAST (size-before-offset)
                return w as i64; // no mark_dirty — already on disk
            }
        }
    }
    // Copy the bytes into the buffer, THEN publish the extended length + size (Release) — a reader sees the
    // appended tail only after it exists. Size before offset; mark_dirty covers exactly [offset, new_end).
    // CFU-1 NOTE-1: the ring-3 read is the unified `copy_from_user` seam (`want` is exactly the range the
    // `user_range_ok` above validated — cannot fail here).
    if let Err(e) = wstage_write_from_user(widx, off, buf, want) {
        return e;
    }
    wstage_set_len_at_least(widx, new_end);
    FILE_SIZE[row][idx].store(core::cmp::max(size, new_end), Ordering::Release);
    mark_dirty(row, idx, offset, new_end);
    FILE_GREW[row][idx].store(true, Ordering::Release);
    FILE_OFFSET[row][idx].store(new_end, Ordering::Release); // offset LAST (size-before-offset)
    want as i64
}

/// STOR-1 S9: the GROWTH half of a DYNAMIC on-disk write — a past-EOF write on a dynamic RW descriptor
/// (`FILE_DYNLEN != 0`) extends the file on the LIVE volume SYNCHRONOUSLY, through the storage service task's
/// `Grow` op (`submit_grow` -> `service_grow_file` -> `fat::write_grow`: alloc + zero-fill + chain new clusters,
/// RMW the data, bump the directory size LAST). This is the S4 `submit_grow` shape reused for a NON-staged file
/// — a dynamic descriptor owns no wstage, so unlike `sys_write_grow` (which extends the one-page staging buffer
/// and DEFERS the disk work) the extend lands straight on disk here.
///
/// GROWTH BOUNDS (both explicit + defended — unbounded growth off a syscall is a DoS surface):
///  * PER-WRITE: `want` clamps to `DYN_GROW_MAX` (one page). ONE bounce buffer, ONE `submit_grow` — no chunk
///    loop, so the persist is a single atomic op (see -EIO honesty below). A longer write returns the page-
///    clamped count; the caller continues sequentially.
///  * PER-FILE: the new EOF may not exceed `DYN_FILE_MAX` (64 KiB). An offset at/past the ceiling is `-ENOSPC`;
///    a write that would cross it is clamped so `offset + want <= DYN_FILE_MAX`. Absolute (not opened-size
///    relative), so a close+reopen cannot walk a file past the ceiling.
///
/// -EIO HONESTY (whole-op-or-error, mirroring S2/S3/S8): the grow is a SINGLE `submit_grow`; disk is written
/// FIRST and only on success (`n >= 0`) are `FILE_SIZE` then `FILE_OFFSET` published (size-before-offset, so
/// `FILE_OFFSET <= FILE_SIZE` is never even transiently violated). A failed grow returns `-EIO` with the
/// descriptor UNTOUCHED (offset/size unchanged, never CAS-advanced) and the on-disk size unchanged — `write_grow`
/// bumps the visible directory size LAST, so a mid-op failure leaves no half-acknowledged extend. A bad source
/// buffer is `-EFAULT` with no copy, no size/offset move. No `ns_lock` (the dynamic path stays outside the U10
/// namespace — the S5 deadlock class stays closed). Single-writer: a dynamic RW descriptor is never on
/// SHARED_ROW (the open refuses it), so the un-CAS'd offset load in the caller races nothing.
#[cfg(feature = "irqstorage")]
fn dyn_write_grow(row: usize, idx: usize, offset: u32, size: u32, buf: u64, len: u64) -> i64 {
    let off = offset as usize;
    // PER-FILE ceiling: refuse an offset already at/past the cap (no room to grow), else the room left below it.
    if off >= DYN_FILE_MAX {
        return ENOSPC;
    }
    // PER-WRITE cap (one page) AND per-file room — `want` is the smaller. `want == 0` only if `off == DYN_FILE_MAX`
    // (excluded above), so a legit grow always has room for at least one byte.
    let mut want = core::cmp::min(len as usize, DYN_GROW_MAX);
    want = core::cmp::min(want, DYN_FILE_MAX - off);
    if want == 0 {
        return ENOSPC;
    }
    // Validate the WHOLE (clamped) source READABLE in the window BEFORE any copy — a bad buffer is -EFAULT with
    // no copy, no size/offset move (the code page is a legal RX source, so `UserAccess::Read` is the bound).
    if let Err(e) = user_range_ok(buf, want as u64, UserAccess::Read) {
        return e;
    }
    // Bounce the ring-3 source into a kernel-stack buffer the service task can reach (`want <= PAGE_SIZE` by the
    // per-write cap), then GROW live. Persist to DISK FIRST; only on success mirror the new EOF into the
    // descriptor (never in-memory-ahead-of-disk).
    let mut kbuf = [0u8; PAGE_SIZE as usize];
    if let Err(e) = copy_from_user(&mut kbuf[..want], buf) {
        return e;
    }
    let n = unsafe { dyn_grow_live(row, idx, offset, kbuf.as_mut_ptr(), want) };
    if n < 0 {
        return EIO; // disk grow failed -> descriptor + on-disk size UNTOUCHED
    }
    let w = (n as usize).min(want); // bytes actually persisted (== want on a full write)
    let new_end = offset + w as u32;
    FILE_SIZE[row][idx].store(core::cmp::max(size, new_end), Ordering::Release);
    FILE_OFFSET[row][idx].store(new_end, Ordering::Release); // offset LAST (size-before-offset)
    w as i64
}

/// SYS_SEEK(handle, offset) -> the new absolute offset, or a negative errno (U9x; the aarch64 pi4 U9 twin).
/// Absolute seek on an open File descriptor: the CHECK requires a `File` handle carrying ANY of
/// CAP_READ|CAP_WRITE. `handle_resolve` requires ALL bits in `req`, so "any of" is expressed by resolving for
/// CAP_READ, else for CAP_WRITE — whichever right is present (and the File kind, and — via `handle_resolve` —
/// no revoked ancestor) admits the seek; a non-File kind / no handle / a revoked cap all give `-EACCES`. An
/// offset PAST `size` is `-EINVAL` (seeking exactly TO `size`, the EOF position, is legal). Sets FILE_OFFSET;
/// a later SYS_READ / File SYS_WRITE resumes from it. No I/O — a pure descriptor-state update (so it is safe in
/// the IF-masked SYSCALL handler).
fn sys_seek(handle: u64, offset: u64) -> i64 {
    let row = caller_row();
    // The CHECK: a File carrying CAP_READ OR CAP_WRITE (either admits a seek), non-revoked. The double resolve
    // expresses "any of" over `handle_resolve`'s all-bits-in-`req` semantics without reading the sidecars raw.
    let file_id = match handle_resolve(row, handle, CAP_READ) {
        Ok(HandleTarget::File(id)) => id,
        _ => match handle_resolve(row, handle, CAP_WRITE) {
            Ok(HandleTarget::File(id)) => id,
            _ => return EACCES,
        },
    };
    // U11x: decode + validate the file-id through the ONE seam (bounds + presence + generation — a stale sibling
    // to a reused slot is rejected). `None` -> -EACCES.
    let Some(idx) = file_desc_validate(row, file_id) else {
        return EACCES;
    };
    let size = FILE_SIZE[row][idx].load(Ordering::Acquire);
    // Absolute seek: an offset PAST the file's size is invalid; seeking exactly TO `size` (the EOF position, a
    // legal 0-byte read/write point) is allowed. Preserves the FILE_OFFSET <= FILE_SIZE invariant. `size` is a
    // u32, so `offset <= size` guarantees the cast is exact and the return fits a non-negative i64.
    if offset > size as u64 {
        return EINVAL;
    }
    FILE_OFFSET[row][idx].store(offset as u32, Ordering::Release);
    offset as i64
}

/// SYS_CLOSE(handle) -> `0`, or a negative errno (U11x; the aarch64 pi4 U11 twin). CLOSE an open `File`: free the
/// handle's descriptor slot (bumping its generation, so a first-fit reuse never re-binds a lingering sibling
/// file-id) and clear the handle word. A close is not a mutation of the underlying object, so it requires NO
/// capability right — `handle_resolve(row, handle, 0)` admits any live handle the caller holds (kind + descriptor
/// identity are still enforced). Semantics:
///   * a live `File` -> free descriptor + clear handle -> `0`;
///   * a `Console`/`Socket`/`Child` kind -> `-EINVAL`, object table UNTOUCHED (not closeable this arc — never
///     corrupt it by freeing a File slot it does not own);
///   * an unresolvable handle (Empty / out-of-range / RESERVING / revoked), or a `File` whose descriptor is
///     already stale/closed (`file_desc_validate` fails) -> `-EBADF` — so a double-close returns cleanly and a
///     use-after-close is denied.
/// Only the caller's OWN descriptor slot is freed. x86 divergence from pi4: `files_free` DISCARDS any un-flushed
/// dirty bytes (the staged write-back is a whole-task-teardown event, `clear_files_row`), so an explicit close of
/// a dirty RW descriptor drops the un-flushed write exactly as a revoke does — the demo closes RO handles.
fn sys_close(handle: u64) -> i64 {
    let row = caller_row();
    // Resolve for NO right (close is always permitted on a handle you hold). A non-File kind is refused without
    // being touched; anything that does not resolve falls through to -EBADF (already closed / never opened).
    let file_id = match handle_resolve(row, handle, 0) {
        Ok(HandleTarget::File(id)) => id,
        Ok(_) => return EINVAL, // Console/Socket/Child — not a closeable File this arc; leave it intact
        Err(_) => return EBADF, // no such handle (already closed / never opened / oob / revoked)
    };
    // The handle is a live File, but its descriptor may already be gone (a sibling revoke freed the slot, or this
    // is a stale handle to a reused slot). Validate before freeing so a double-close / stale-close is -EBADF, not
    // a free of someone else's current descriptor.
    let Some(idx) = file_desc_validate(row, file_id) else {
        return EBADF;
    };
    files_free(row, idx); // release the slot + its writable staging (if any) + bump the generation
    handle_clear(row, handle as usize);
    0
}

/// SYS_UNLINK(handle) -> `0`, or a negative errno (U10 M3; the aarch64 pi4 U10 twin). DELETE the runtime-CREATED
/// file an open File+`CAP_WRITE` handle names. Semantics:
///   * the CHECK is `sys_write`'s — `handle_resolve(row, handle, CAP_WRITE)` must yield a `File`; a missing right
///     (RO open) / a non-File kind / no handle / a revoked cap all -> `-EACCES` (delete is a mutation, gated by
///     the SAME single CAP_WRITE resolve as write), and a stale descriptor (`file_desc_validate`) -> `-EACCES`;
///   * SCAFFOLD GUARD — only a CREATED file is unlinkable this arc: an immutable STAGED file (HELLO.BIN is live
///     EL0 code; SCRATCH.BIN/GROW.BIN are demo fixtures) has `FILE_CREATED == false` -> `-EACCES`, so ring 3 can
///     never `0xE5` an immutable staged file;
///   * on success: CAPTURE the file's bytes + enqueue the on-disk delete (a `CreateGrowDelete` op — the fixture's
///     create/grow never persisted on x86, so the launcher create+grow+deletes to exercise `fat::delete_located`);
///     mark the name deleted for the row (`DYN_DELETED` — a subsequent plain re-open is `-ENOENT`); INVALIDATE
///     every one of this process's descriptors for the file (each `files_free` bumps the slot generation, so a
///     stale sibling handle's next read/write fails `file_desc_validate` -> `-EACCES`, no stale reference to the
///     freed chain — the U11x gen-tag); clear the caller's handle; return `0`.
/// x86 divergence from pi4: pi4 unlinks an independently-persisted file in-handler (0xE5 + free chain on disk);
/// x86 defers the disk delete to the launcher and REPLAYS create+grow+delete (a weaker causal exercise, but it
/// genuinely drives `fat::delete_located`). Bit3 here therefore proves gen-invalidation, not a freed-chain
/// aliasing fail-safe (there is no on-disk chain to alias pre-drain) — documented; the aliasing hazard is only
/// reproducible on the pi4 in-handler path.
fn sys_unlink(handle: u64) -> i64 {
    let row = caller_row();
    // The CHECK: a File carrying CAP_WRITE (the write gate), non-revoked.
    let file_id = match handle_resolve(row, handle, CAP_WRITE) {
        Ok(HandleTarget::File(id)) => id,
        _ => return EACCES,
    };
    let Some(idx) = file_desc_validate(row, file_id) else {
        return EACCES;
    };
    // Scaffold guard: only a runtime-CREATED file is unlinkable this arc.
    if !FILE_CREATED[row][idx].load(Ordering::Acquire) {
        return EACCES;
    }
    let opname = FILE_OPNAME[row][idx].load(Ordering::Acquire);
    let Some(nameid) = (opname as usize).checked_sub(1) else {
        return EACCES; // a created descriptor always carries a name-id; defensive
    };
    if nameid >= N_U10_NAMES {
        return EACCES;
    }
    // U6x F1 — DELETE is an OWNER-only authority. The CAP_WRITE CHECK above admits both the owner AND a
    // WRITE-GRANTEE (a grantee legitimately opened the file RW), but a content grantee must NOT be able to delete
    // — else it could `unlink` + `O_CREAT` the name to STEAL ownership and lock the real owner out. So an OWNED
    // file is unlinkable only by its current owner; a PUBLIC file (no owner row) keeps the prior CAP_WRITE-gated
    // behaviour. Checked BEFORE any state change, so a denied unlink mutates nothing.
    let caller_gen = SLOT_GEN[row].load(Ordering::Acquire);
    if !owned_unlink_permitted(nameid, row, caller_gen) {
        return EACCES;
    }
    // Capture the file's bytes BEFORE freeing descriptors (files_free discards the wstage), then enqueue the
    // on-disk delete — a self-contained COPY (u10_flush_enqueue), so it survives the frees below. U11x M2: the op
    // is enqueued HELD — not drainable until the LAST descriptor across ALL rows closes (`openf_decref`'s
    // release; teardown counts as close), the x86 unlink-defers-free. A SOLE opener releases it right here in the
    // sweep below (its own decrefs reach zero), so the U10 sole-process flow is unchanged. Enqueue only when a
    // FAT volume is present (HELLO_STAGED): the no-FAT in-memory core has nothing to delete on disk, so a queued
    // op would just strand (the launcher skips the drain). The in-memory delete semantics below run either way —
    // the syscall still returns 0 and invalidates the caller's descriptors.
    // STOR-1 S6a: the unlink name-state transitions (claim -> owned_clear -> pending marks -> invalidate THIS
    // row's descriptors) run under the NAMESPACE lock, MUTUALLY ATOMIC with sibling-open / create — so no open
    // can slip between the claim and the descriptor sweep. The last-close on-disk DELETE is lifted OUT of the
    // lock (below): never a service-task block under the spinlock (the S5 deadlock class). RAII-held to the
    // explicit `drop(ns)` after the sweep.
    let ns = ns_lock();
    // U11x M2: CLAIM the unlink atomically (swap) — a SECOND unlink of the same name (e.g. another row's live
    // sibling handle after a cross-row unlink) is -ENOENT (the name is already gone), never a double-enqueue of
    // the delete op (NU10 == 1) or a double pending-mark. The claimer proceeds; its stores below complete the
    // mark. (This also makes the flag-set the FIRST observable effect — no new increfs from here on.)
    if DYN_DELETED_G[nameid].swap(true, Ordering::AcqRel) {
        return ENOENT;
    }
    // U6x: the name is gone — drop its owner/grants row NOW. Ownership ends at unlink (the pi4 `owned_clear`
    // twin), so a later O_CREAT re-create of the name (once its deferred delete drains and clears DYN_DELETED_G)
    // establishes a FRESH owner rather than inheriting the deleted file's ACL. Placed right after the unlink is
    // claimed, so the ACL row dies exactly when the name does.
    owned_clear(nameid);
    let mut heldslot = 0u32; // +1-biased queue slot of the held op (0 == none)
    // Knob-off / no-FAT: enqueue the deferred CreateGrowDelete op HELD (a self-contained COPY of the file's
    // bytes; released at the LAST close). STOR-1 S4c: knob-on, the file is ALREADY ON DISK (S4a create + S4b
    // grow) and the DELETE runs SYNCHRONOUSLY at the last close (`openf_release`), so NOTHING is enqueued
    // (heldslot stays 0) — the U10 op-queue + its launcher-replay causal-fidelity gap are RETIRED when on.
    if !s4_sync_storage() && HELLO_STAGED.load(Ordering::Acquire) {
        let size = FILE_SIZE[row][idx].load(Ordering::Acquire) as usize;
        let slot = match (FILE_WSTAGE[row][idx].load(Ordering::Acquire) as usize).checked_sub(1) {
            Some(w) => {
                let all = wstage_bytes(w);
                let n = size.min(all.len());
                u10_flush_enqueue(U10OP_CREATE_GROW_DELETE, nameid as u32, 0, &all[..n], true)
            }
            None => u10_flush_enqueue(U10OP_CREATE_GROW_DELETE, nameid as u32, 0, &[], true),
        };
        if let Some(k) = slot {
            heldslot = (k + 1) as u32;
        }
    }
    // U11x M2 ordering (the pi4 mark-0xE5 -> mark-pending -> drop-descriptors rule): (1) the name vanishes
    // GLOBALLY (any row's plain re-open is now -ENOENT, an O_CREAT re-create -EBUSY — no new increfs possible
    // from here on); (2) the file goes unlink-PENDING with its held op recorded; (3) EVERY descriptor in THIS
    // row naming the file is invalidated (the primary + every sibling): each `files_free` bumps the slot
    // generation (a stale sibling handle is -EACCES on its next use) and decrefs the global count — if this row
    // held the last opens, the final decref performs the release. Descriptors in OTHER rows stay live (their
    // reads keep serving their own wstage copies — read-after-unlink, the deferral) and release at their own
    // close/teardown. (`DYN_DELETED_G` was already claimed by the swap above.)
    OPENF_PENDING[nameid].store(true, Ordering::Release);
    OPENF_HELDSLOT[nameid].store(heldslot, Ordering::Release);
    // S6a: sweep with `files_free_clear` (the atomic clears + gen bump; NO blocking release) and drop the global
    // open count via `openf_decref` (pure atomics). `openf_decref` returns true only when THIS row held the LAST
    // open across ALL rows AND the synchronous knob is on — i.e. the on-disk DELETE must now run; we perform it
    // AFTER releasing the lock. Knob-off / no-FAT: `openf_decref` does the in-memory / deferred-op release itself
    // and returns false (byte-identical to the pre-S6 `files_free` -> `openf_release` path, whose submit leg was
    // inert off-knob).
    let mut need_delete = false;
    for k in 0..NFILE {
        if FILE_USED[row][k].load(Ordering::Acquire)
            && FILE_CREATED[row][k].load(Ordering::Acquire)
            && FILE_OPNAME[row][k].load(Ordering::Acquire) == opname
        {
            if let Some(n) = files_free_clear(row, k) {
                debug_assert_eq!(n, nameid, "unlink sweep freed a descriptor of a different name");
                if openf_decref(n) {
                    need_delete = true;
                }
            }
        }
    }
    handle_clear(row, handle as usize);
    drop(ns); // release NAMESPACE before any service-task block
    // S6a: the deferred on-disk DELETE runs SYNCHRONOUSLY here (knob-on last close) — OUTSIDE the lock. Item-3
    // fix (`openf_perform_delete`): the DYN_DELETED_G clear is gated on delete success (a wedged -EIO leaves the
    // name blocked, fail-safe, rather than adopting a stale on-disk entry on re-create).
    #[cfg(feature = "irqstorage")]
    if need_delete {
        openf_perform_delete(nameid);
    }
    #[cfg(not(feature = "irqstorage"))]
    let _ = need_delete;
    0
}

/// SYS_FGRANT(file_handle, child_handle, rights) -> `0` or a negative errno (U6x, the aarch64 U6 twin). The
/// OWNER of a private created file grants (a CAP_READ|CAP_WRITE subset) or revokes (`rights == 0`) access to
/// another principal named OWNER-SCOPED by a `Child` handle it holds (the SYS_XFER idiom — ring 3 never supplies
/// a raw pid/slot). The grant is an ACL edge on the FILE: nothing is delivered to the grantee's table; the
/// grantee opens the name and the SYS_OPEN ACL admits it, and a handle it ALREADY holds survives a revoke (the
/// ACL gates ACQUISITION, not held caps). Ordering mirrors the pi4 twin: ownership is verified BEFORE the
/// grantee handle is resolved (a non-owner is `-EACCES` whatever it passes, and never learns whether that handle
/// was valid). Errnos: `-EACCES` (the caller owns no private row / the file is public/staged/nonexistent / the
/// caller is not its owner), `-ECHILD` (the child handle names no running child), `-EINVAL` (a nonzero rights
/// request naming only unsupported bits), `-ENOSPC` (the file's bounded grant list is full).
fn sys_fgrant(file_handle: u64, child_handle: u64, rights: u64) -> i64 {
    // The caller must own a PRIVATE row — the shared kernel window owns no created files (created opens are
    // refused on SHARED_ROW), so it can grant nothing.
    let Some(row) = crate::arch::memory::current_slot() else {
        return EACCES;
    };
    let caller_gen = SLOT_GEN[row].load(Ordering::Acquire);
    // The FILE: a live File handle the caller holds; decode + validate its descriptor (range/USED/generation),
    // then read the created-file NAME-ID the ACL is keyed by. A staged file (opname 0) owns nothing -> -EACCES.
    let file_id = match handle_resolve(row, file_handle, 0) {
        Ok(HandleTarget::File(id)) => id,
        _ => return EACCES,
    };
    let Some(idx) = file_desc_validate(row, file_id) else {
        return EACCES;
    };
    let Some(nameid) = (FILE_OPNAME[row][idx].load(Ordering::Acquire) as usize).checked_sub(1) else {
        return EACCES; // a staged (non-created) descriptor owns no created name
    };
    if nameid >= N_U10_NAMES {
        return EACCES;
    }
    // Only the file's OWNER may grant/revoke — checked FIRST, before resolving the grantee, so a non-owner is a
    // clean -EACCES whatever it passes as the child handle (and it never learns whether that handle was valid).
    if !owned_is_owner(nameid, row, caller_gen) {
        return EACCES;
    }
    // The GRANTEE: named owner-scoped by a Child handle the caller holds (the SYS_XFER idiom). Resolve child ->
    // pid -> the recipient's live SLOT + generation, exactly as sys_xfer does (must be RUNNING now; a
    // shared-window task with no private slot is not a grantee).
    let pid = match handle_resolve(row, child_handle, 0) {
        Ok(HandleTarget::Child(pid)) => pid,
        _ => return ECHILD,
    };
    let Some(pi) = proc_find_child(pid) else {
        return ECHILD;
    };
    if PROCS[pi].state.load(Ordering::Acquire) != PRUNNING {
        return ECHILD;
    }
    let Some(grantee_slot) = PROCS[pi].slot.load(Ordering::Acquire).checked_sub(1) else {
        return ECHILD; // no private slot recorded (a shared-window task is not a grant recipient)
    };
    if grantee_slot >= crate::arch::memory::USER_SLOTS {
        return ECHILD; // defensive: never the shared row (sys_spawn only records private slots)
    }
    let grantee_gen = SLOT_GEN[grantee_slot].load(Ordering::Acquire);
    // Only READ|WRITE are grantable file rights. `rights == 0` is an explicit REVOKE; a NONZERO request naming
    // ONLY unsupported bits is malformed (-EINVAL) rather than silently coerced to a revoke.
    let req = rights as u32;
    if req != 0 && req & (CAP_READ | CAP_WRITE) == 0 {
        return EINVAL;
    }
    let rights = req & (CAP_READ | CAP_WRITE);
    // F2-fold (SMP-hardening, the pi4 F2 M2 twin `4562501`): `grantee_slot` (from `PROCS[pi].slot`) and
    // `grantee_gen` (from `SLOT_GEN[grantee_slot]`) were captured as TWO separate atomic loads. On a multi-core
    // boot the named child could EXIT between them and its slot be recycled to a DIFFERENT process (a bumped
    // `SLOT_GEN`), so the grant would bind the stale `grantee_slot` to the RECYCLED incarnation's `grantee_gen` —
    // a misdelegation of the owner's file to whatever process now holds that slot (privilege escalation /
    // disclosure). Re-validate that `pi` is STILL the same running incarnation AFTER reading its gen: a slot's
    // gen bumps only at teardown (`clear_handle_row`, top), which drives `state` off `PRUNNING` and clears
    // `pid`/`slot` (`proc_free`), so a matching `state`/`pid`/`slot` proves the `(slot, gen)` pair is a
    // consistent snapshot of ONE incarnation. Any mismatch means the child recycled mid-resolution — refuse
    // (`-ECHILD`) rather than grant to the wrong principal. (Narrow, metal-only window; behaviourally
    // transparent single-core — nothing recycles mid-syscall on one core, so all witnesses stay byte-identical.)
    if PROCS[pi].state.load(Ordering::Acquire) != PRUNNING
        || PROCS[pi].pid.load(Ordering::Acquire) != pid
        || PROCS[pi].slot.load(Ordering::Acquire) != grantee_slot + 1
        || SLOT_GEN[grantee_slot].load(Ordering::Acquire) != grantee_gen
    {
        return ECHILD;
    }
    owned_grant(nameid, row, caller_gen, grantee_slot, grantee_gen, rights)
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

/// CLOCK-X1 (M3): a bounded, UNCOUNTED serial witness (`== witness ::`, not a `-> PASS` line, so it
/// never shifts a fixture COUNT) proving the x86 wall-clock timebase is live. It is SILENT where the
/// TSC path is honestly frozen — no invariant TSC or no calibration (`clock::uptime_secs()` is
/// `None`) — so a machine without an invariant TSC prints nothing rather than a spurious verdict.
/// When live it proves: (1) `monotonic()` returns `Some` (uptime is `Some`, freq nonzero);
/// (2) two `rdtsc` reads are monotone and advancing (second ≥ first, and strictly greater across a
/// bounded spin); (3) the wall-SECOND derivation `now()` uses to extend a seed advances — the exact
/// mechanism JD17 documented as FROZEN on x86 — observed directly as `uptime` crossing a second
/// within a bounded budget, or, if the run is too fast to cross one, reported as the raw cycle
/// advance with the current uptime (the brief's tick-monotonicity + nonzero-freq fallback). It never
/// seeds the global clock, so it leaves the operator's UNSET state untouched.
pub fn clock_x1_witness() {
    // Frozen path: no invariant/calibrated TSC. `uptime_secs()` is `None` exactly when
    // `clock::monotonic()` is `None`, so this is the honest silent gate.
    let u1 = match crate::clock::uptime_secs() {
        Some(u) => u,
        None => return,
    };
    let mhz = crate::arch::apic::tsc_hz() / 1_000_000;

    // Monotone + advancing: two rdtsc reads across a bounded spin. The spin is a fixed iteration
    // cap (not a wall-clock wait), so it can never hang the serial-less boot.
    let a = crate::arch::now_cycles();
    let mut spins = 0u64;
    // Spin until either a wall second elapses (uptime advances) or a bounded iteration cap is hit,
    // whichever comes first — deterministic and hang-proof under QEMU/TCG.
    let mut u2 = u1;
    while u2 == u1 && spins < 50_000_000 {
        core::hint::spin_loop();
        spins += 1;
        u2 = crate::clock::uptime_secs().unwrap_or(u1);
    }
    let b = crate::arch::now_cycles();
    let delta = b.wrapping_sub(a);

    if b < a {
        // A backwards rdtsc would break the clock's monotonicity contract — surface it (uncounted).
        serial_println!(":: CLOCK-X1: NON-MONOTONE rdtsc {} -> {} == witness ::", a, b);
        return;
    }

    if u2 > u1 {
        serial_println!(
            ":: CLOCK-X1: TSC invariant, ~{} MHz; monotone (rdtsc +{}); uptime {}->{} s (JD17 x86-frozen clock now advances) == witness ::",
            mhz, delta, u1, u2
        );
    } else {
        // Too fast to cross a wall second within the cap: fall back to tick-monotonicity + nonzero
        // freq (the brief's blessed fallback). A nonzero `delta` proves the counter advanced.
        serial_println!(
            ":: CLOCK-X1: TSC invariant, ~{} MHz; monotone (rdtsc +{} over {} spins, <1 s); uptime {} s == witness ::",
            mhz, delta, spins, u1
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
const EBADF: i64 = -9; // U11x SYS_CLOSE: no such handle (already closed / never opened / oob / stale-slot)
const ESRCH: i64 = -3; // WINX-7 SYS_THREAD_JOIN: no such thread handle (oob / already reaped / another tenant's)
const ENFILE: i64 = -23; // WINX-1 SYS_WIN_CREATE: the GLOBAL window table is full (vs EMFILE = this caller's)
const EBUSY: i64 = -16; // U11x M2: O_CREAT of a name whose deferred on-disk DELETE has not drained yet
const ENOSPC: i64 = -28; // U10: the FAT volume (or the one-page grow-staging buffer) is full
// SOCK-3 TCP-only errnos (cfg-gated so knob-off / aarch64 emit no unused-const warning).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const EINPROGRESS: i64 = -115; // SYS_CONNECT: the 3-way handshake is still in flight — ring 3 re-polls
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const ECONNREFUSED: i64 = -111; // SYS_CONNECT: the peer reset / refused the active open
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const ENOTCONN: i64 = -107; // SYS_SEND: the socket is not (yet) connected / the send half is closed

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

/// U9x: the DEDICATED writable scratch file the File-WRITE fixture opens + overwrites (NEVER `HELLO.BIN`,
/// which other fixtures load as EL0 code). The wstage seed is ALWAYS the in-memory const (below) — present
/// regardless of disk, so the in-memory core is self-contained. M2 KEEPS that seed (it equals the on-disk
/// plant byte-for-byte) and additionally, when a FAT volume is present, CAPTURES this file's on-disk chain head
/// (the launcher pre-flight -> `SCRATCH_CLUSTER`) so a dirty write is flushed to disk at teardown; the launcher
/// then raw-re-reads the sector. 11 chars (<= `MAX_NAME`); the fixture's namelen matches. The aarch64 twin's
/// `U9_SCRATCH_NAME`.
const U9X_SCRATCH_NAME: &str = "SCRATCH.BIN";
/// U9x: the in-memory scratch file's byte size (the EOF bound writes/seeks clamp against). 1024 bytes, so the
/// fixture's write offset (520) lands 8 bytes into the SECOND 512-byte sector — a partial-sector overwrite
/// (the interesting case for M2's disk write-back: the sector's other bytes must survive). The aarch64 twin's
/// `U9_SCRATCH_SIZE`.
const U9X_SCRATCH_SIZE: usize = 1024;
/// U9x: the filler byte the in-memory scratch seed carries — a distinctive non-pattern value, so a read-back
/// of the written region provably differs from the pre-image. Matches the pi4 image plant (`arroyo`).
const U9X_SCRATCH_FILL: u8 = 0xEE;
/// U9x: the in-memory scratch SEED — the read-only initial content of `SCRATCH.BIN` (`staged_bytes(1)`). A RW
/// open COPIES this into a per-descriptor writable staging buffer (writes land there, in place); an RO open
/// serves it directly. Const-initialized, so it is always "staged" in M1 with no disk dependency.
static U9X_SCRATCH_SEED: [u8; U9X_SCRATCH_SIZE] = [U9X_SCRATCH_FILL; U9X_SCRATCH_SIZE];
/// U9x M2: SCRATCH.BIN's on-disk FAT chain head, captured by the launcher's pre-flight (a fresh `mount` +
/// `find_in_root`, IF=1 on the demo AP) and published (Release) BEFORE the fixture is spawned, so a RW
/// `sys_open` can record it into the descriptor's `FILE_CLUSTER` (Acquire) — the flush target. `0` == NO disk
/// backing (no FAT volume, or SCRATCH.BIN absent / the wrong size): the fixture then runs the M1 IN-MEMORY
/// core (const seed above) and the flush is a no-op. x86 can't walk the FAT inside the IF-masked handler, so
/// the launcher pre-captures the chain head at IF=1 (the x86 stand-in for pi4 capturing FILE_CLUSTER at open).
static SCRATCH_CLUSTER: AtomicU32 = AtomicU32::new(0);

// --- U10 file GROWTH / CREATE / DELETE demo constants (the aarch64 U10/U10c/U10d twins). GROW.BIN is a staged
// file the growth fixture extends across a cluster boundary; FRESH.BIN / DELME.BIN are runtime-CREATED (never
// staged). Every disk mutation is DEFERRED from the IF-masked handler to the launcher's IF=1 drain. ---
/// U10 GROW: the dedicated file the growth fixture extends — planted 512 bytes of `0xC1` (one 512-B cluster on
/// the FAT32 layouts). 8 chars (<= `MAX_NAME`). The aarch64 twin's `U10_GROW_NAME`.
const U10_GROW_NAME: &str = "GROW.BIN";
/// U10 GROW: GROW.BIN's planted size (the "before" size). The fixture seeks HERE (== EOF == the 512-B cluster
/// boundary) and appends, so the write runs strictly PAST EOF (a real grow, never a U9x clamp-to-0).
const U10_GROW_PLANTED_SIZE: u32 = 512;
/// U10 GROW: the absolute offset the fixture seeks to and appends 16 bytes at (== the planted EOF).
const U10_GROW_OFFSET: u32 = 512;
/// U10 GROW: the 16-byte pattern appended at `U10_GROW_OFFSET` (the `.ascii` in the fixture MUST match). The
/// launcher's raw re-read of the grown region must find exactly these bytes. 16 chars.
const U10_GROW_PATTERN: [u8; 16] = *b"U10x-GROW-OK-678";
/// U10 GROW: the `0xC1` filler the planted cluster is full of; the fixture reads offset 0 back and the launcher
/// re-reads it to prove the ORIGINAL cluster survived the grow. Matches the make-fat-img.sh plant byte-for-byte.
const U10_GROW_FILLER: u8 = 0xC1;
/// U10 GROW: the size AFTER the grow (`512 + 16`) — the "size increased" invariant the launcher asserts.
const U10_GROW_NEW_SIZE: u32 = U10_GROW_PLANTED_SIZE + 16;
/// U10 GROW: GROW.BIN's in-memory SEED (`staged_bytes(GROW_STAGED_IDX)`) — 512 × `0xC1`, equal to the on-disk
/// plant byte-for-byte, so a RW open's wstage copy (and a read-back before any write) sees the original filler.
static U10_GROW_SEED: [u8; U10_GROW_PLANTED_SIZE as usize] =
    [U10_GROW_FILLER; U10_GROW_PLANTED_SIZE as usize];
/// U10 GROW: GROW.BIN's staged-set index (it is a staged file, like SCRATCH.BIN) — the value `sys_open` matches
/// to stamp the descriptor's `FILE_OPNAME` (marking it growable) and the launcher records its on-disk chain head.
const GROW_STAGED_IDX: u32 = 2;
/// U10 GROW: GROW.BIN's on-disk FAT chain head, captured by the launcher pre-flight (fresh mount + find, IF=1)
/// and published before the fixture opens — `0` == no disk backing (no FAT / absent / wrong size -> in-memory mode).
static GROW_CLUSTER: AtomicU32 = AtomicU32::new(0);
/// U10 GROW: the cap on a single growing write's in-memory extend (bounds the wstage span per call; the file
/// stays within the one-page staging buffer). A longer write returns a short count. The demo appends 16 bytes.
const GROW_WRITE_MAX: usize = 512;
/// STOR-1 S9: the PER-WRITE growth cap for a DYNAMIC on-disk descriptor (`open_dynamic_ondisk` RW, past-EOF).
/// A single `sys_write_file` grow persists at most one PAGE (4096 B) through ONE kernel bounce buffer and ONE
/// atomic `submit_grow` — no chunk loop, so a failed grow is whole-op `-EIO` (never a masked partial). A longer
/// write returns a short (page-clamped) count; the caller continues sequentially. Unlike the U10 `GROW_WRITE_MAX`
/// (bounded by the one-page WSTAGE buffer a created/staged descriptor extends into), a dynamic descriptor owns
/// NO wstage — its grow goes straight to the live volume — so the cap here bounds the disk work per syscall.
#[cfg(feature = "irqstorage")]
const DYN_GROW_MAX: usize = PAGE_SIZE as usize;
/// STOR-1 S9: the PER-FILE growth ceiling for the dynamic path — a dynamic descriptor may never grow a file's
/// EOF beyond this (64 KiB). A write whose target offset is at/past the ceiling is refused `-ENOSPC`; a write
/// that would cross it is clamped so `offset + want <= DYN_FILE_MAX`. This is the real DoS bound: unbounded
/// growth off a syscall would let any ring-3 task exhaust the volume, and because the check is absolute (not
/// relative to the opened size) a close+reopen cannot walk a file past the ceiling. Overwrite of a file already
/// larger than the ceiling is unaffected (S8's overwrite path never enters the grow branch).
#[cfg(feature = "irqstorage")]
const DYN_FILE_MAX: usize = 64 * 1024;
/// U10 CREATE / DELETE: the runtime-created file names (absent from the staged set; the fixtures O_CREAT them).
const U10C_NAME: &str = "FRESH.BIN";
const U10D_NAME: &str = "DELME.BIN";
/// U11x M2: the cross-process defer demo's runtime-created file (created by the launcher on a scratch row, opened
/// + unlinked by the EL0 fixture from ITS row — the pi4 `DEFER.BIN` twin). Absent from the staged set.
const U11M2_NAME: &str = "DEFER.BIN";
/// U11x M2: the 16-byte pattern the launcher writes into DEFER.BIN (the fixture read-verifies the first 8 bytes;
/// the `.ascii` in the fixture MUST match). 16 chars. The pi4 twin's `U11_DEFER_PATTERN`.
const U11M2_PATTERN: [u8; 16] = *b"U11x-DEFER-OK-42";
/// U6x: the owner/grants demo's runtime-created file — the OWNED file the A/B fixtures choreograph over (A
/// creates it PRIVATE; B is a different process, denied by default, granted, then revoked). A creatable
/// `U10_NAMES` member (index 4), absent from the staged set. The pi4 `OWNED.BIN` twin.
const U6GX_NAME: &str = "OWNED.BIN";
// U6gx: A writes the 16-byte pattern `U6x-OWNED-OK-777` into OWNED.BIN (defined inline in the fixture's `.ascii`;
// the granted B read-verifies the first 8 bytes). Kept in-blob (no kernel-side const) — B does the compare.
/// U10 CREATE: the 16-byte pattern the create fixture writes into FRESH.BIN (also its final size). The `.ascii`
/// in the fixture MUST match; the launcher re-reads it from disk. 16 chars. The aarch64 twin's `U10C_PATTERN`.
const U10C_PATTERN: [u8; 16] = *b"U10x-CREATE-OK99";
/// U10 CREATE: the created file's size after the write (== the pattern length) — the launcher's on-disk check.
const U10C_WRITTEN: u32 = 16;
/// U10 CREATE/DELETE: the `FILE_STAGED` sentinel a runtime-created descriptor carries — it is NOT backed by any
/// staged blob (it always owns a wstage buffer, so `sys_read` serves from that and never consults `FILE_STAGED`);
/// `staged_bytes(u32::MAX)` is `None`, so even a mis-read fails closed. Distinguishes a created descriptor's
/// (irrelevant) staged index from a real one at a glance.
const CREATED_STAGED_SENTINEL: u32 = u32::MAX;

/// The staged-file NAME table: index k names the source `staged_bytes(k)` serves. Index 0 = HELLO.BIN (the
/// buffer `stage_hello` fills for sys_spawn; shared, written once, then read-only). Index 1 = SCRATCH.BIN
/// (U9x's writable scratch). Index 2 = GROW.BIN (U10's growable file — a const `0xC1` seed + a disk chain head).
/// A future file rides by adding its name here + a stage buffer + a `staged_bytes` arm.
const STAGED_NAMES: [&str; 3] = ["HELLO.BIN", U9X_SCRATCH_NAME, U10_GROW_NAME];
/// Upper bound on a SYS_OPEN name (the aarch64 twin's `MAX_NAME`): a dotted 8.3 name is at most 12 bytes.
const MAX_NAME: usize = 12;

/// The staged bytes behind staged-file `idx`, or `None` if that stage has not published. Index 0 =
/// HELLO.BIN = `HELLO_BYTES[..HELLO_LEN]`, gated by `HELLO_STAGED` (Acquire, pairing with the
/// `stage_hello` Release) — written ONCE on the BSP before any consumer could hold a descriptor, then
/// read-only, so the returned slice is stable for the rest of the boot (the sys_spawn contract). Index 1 =
/// SCRATCH.BIN = the U9x const seed (always present in M1 — no disk dependency; it is the READ-ONLY seed a RW
/// open copies from, not the live writable buffer, which is per-descriptor via `wstage_bytes`).
fn staged_bytes(idx: u32) -> Option<&'static [u8]> {
    match idx {
        0 if HELLO_STAGED.load(Ordering::Acquire) => {
            let len = HELLO_LEN.load(Ordering::Acquire);
            Some(unsafe { core::slice::from_raw_parts((&raw const HELLO_BYTES).cast::<u8>(), len) })
        }
        1 => Some(&U9X_SCRATCH_SEED),
        2 => Some(&U10_GROW_SEED), // GROW.BIN — const 0xC1 seed (== the on-disk plant), always present
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

/// U9x M2: the on-disk FAT chain head for a staged file — the flush target a RW `sys_open` records into its
/// descriptor's `FILE_CLUSTER`. Only SCRATCH.BIN (index 1) is ever opened writable and flushed; its cluster is
/// captured by the launcher pre-flight into `SCRATCH_CLUSTER`. HELLO.BIN (index 0) is read-only EL0 code — never
/// written, so it has no flush target (`0`). `0` also means "no disk backing" (no FAT / not yet captured), which
/// drops a write to the M1 in-memory path (no flush). A future writable staged file adds its own arm here.
fn staged_cluster(idx: u32) -> u32 {
    match idx {
        1 => SCRATCH_CLUSTER.load(Ordering::Acquire),
        2 => GROW_CLUSTER.load(Ordering::Acquire), // GROW.BIN — the U10 growth flush target
        _ => 0,
    }
}

// --- Per-task open-file descriptors: parallel atomic sidecars keyed `[row][idx]` exactly like
// HANDLES/HANDLE_RIGHTS/HANDLE_KIND (`SHARED_ROW` included), so a File handle's value word carries only
// the +1-biased descriptor index (never the `0`/`u64::MAX` sentinels). Access is single-writer per row at
// any instant — a row is populated ONLY before its task is dispatched (`u6bx_launcher`'s pre-endow) or BY
// that one task mid-syscall (IF-masked), and cleared at teardown after the task exits — so the
// Release/Acquire discipline is belt-and-braces (the HANDLE_RIGHTS twin). Presence is a dedicated
// `FILE_USED` flag (NOT an overloaded index sentinel), so descriptor 0 is representable. U9x adds absolute
// SEEK + File WRITES (into a per-descriptor writable staging buffer), so these sidecars now back a mutable,
// randomly-addressable descriptor — the aarch64 U9 twin's model. ---
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
/// The sequential read/write offset — advanced by exactly the count each `sys_read`/`sys_write` delivers,
/// or set absolutely by `SYS_SEEK` (U9x). Meaningful only where `FILE_USED`. Always kept `<= FILE_SIZE`:
/// reads/writes clamp to the bytes remaining, and `sys_seek` rejects an offset past `size` with `-EINVAL`.
static FILE_OFFSET: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// U9x: the open file's WRITABLE staging-pool slot, `+1`-biased (`0` == a READ-ONLY descriptor with no
/// writable buffer). Set by `files_alloc` when a File is opened RW; a File `SYS_WRITE` overwrites
/// `WSTAGE_BUF[wstage-1]` in place, and `SYS_READ` serves from it (read-back witnesses writes). The x86
/// stand-in for pi4's on-disk backing — pi4 writes straight to FAT in-handler (PIO), x86 CANNOT (the
/// IF-masked handler / hlt-ing xHCI BOT pump), so the write lands in this in-memory buffer instead. M2 adds
/// the BSP flush pump that persists it to FAT. Meaningful only where `FILE_USED`.
static FILE_WSTAGE: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// U9x M2: the open file's on-disk FAT chain head (the flush target for a dirty RW descriptor), captured at
/// `sys_open` from `staged_cluster(sidx)`. `0` == no disk backing (in-memory mode / no FAT) — a dirty write
/// on a `0`-cluster descriptor is never flushed. The x86 twin of pi4's `FILE_CLUSTER`. Meaningful where `FILE_USED`.
static FILE_CLUSTER: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// U9x M2: the descriptor's DIRTY flag — set by `sys_write_file` when a File write lands in the writable
/// staging buffer. A dirty descriptor's bytes are flushed to disk at whole-task TEARDOWN (`clear_files_row`
/// enqueues them for the launcher's IF=1 flush pump); a REVOKE / open-unwind (`files_free`) DISCARDS them
/// (revoke repudiates the write — the brief's revoke ordering). Meaningful where `FILE_USED`.
static FILE_DIRTY: [[AtomicBool; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicBool::new(false) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// U9x M2: the dirty byte range [LO, HI) covering every byte the descriptor's writes touched — the exact span
/// the flush persists. `fat::write_at` read-modify-writes precisely the SECTORS this span overlaps; for the
/// demo's single contiguous write that is exactly one sector (ALL it dirtied, NONE it didn't). NOTE: two
/// DISJOINT writes widen [LO,HI) to their union, so the flush also RMWs any clean sector BETWEEN them — safe
/// here only because untouched staging bytes equal the on-disk content (the seed == the SCRATCH.BIN plant,
/// byte-for-byte); a future writable file whose staged image diverges from disk would need per-run tracking.
/// Set FRESH on the first write (NOT min/max'd against the 0 init — else the first write's range would start
/// at offset 0 and RMW an un-dirtied sector); widened on later writes. Meaningful where `FILE_USED && FILE_DIRTY`.
static FILE_DIRTY_LO: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
static FILE_DIRTY_HI: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// U11x: per-descriptor GENERATION counter. A `File` handle's value word packs `(gen << 32) | (idx + 1)`, and
/// `file_desc_validate` rejects a handle whose packed gen != the slot's CURRENT gen — so a stale sibling handle
/// to a slot that was freed (e.g. by a File revoke or SYS_CLOSE) and then FIRST-FIT-REUSED by a different file is
/// `-EACCES` (a gen mismatch), never a silent re-bind to that different file (closes the U9x revoke+reopen note).
/// Bumped LAST on EVERY free (`files_free` — the path SYS_CLOSE, `sys_cap_revoke`'s File-drop, and `sys_open`'s
/// unwind route through — and `clear_files_row` at teardown) so the very next reuse of the slot lands on a fresh
/// generation. Const-init `0`; monotone within a boot (a u32 wrap is ~4 billion frees away — unreachable for the
/// demo). Acquire/Release-paired with `FILE_USED` (published last on alloc, cleared on free) so a validator that
/// sees a live slot sees its gen. Meaningful for every slot (a fresh, never-freed slot reads gen 0). ---
static FILE_GEN: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];

/// U10: the descriptor's GROWABLE-file identity — a `+1`-biased index into `U10_NAMES` (`0` == NOT growable).
/// Set at `sys_open` for a RW open of a growable file (staged GROW.BIN, or a runtime-CREATED file); NEVER for
/// SCRATCH.BIN (in-place-only) or any RO descriptor. The `sys_write_file` grow branch fires ONLY when this is
/// non-zero, so a past-EOF write on a non-growable descriptor keeps the U9x clamp-to-EOF behaviour byte-for-byte
/// AND a RW holder of a non-growable file can never mint a deferred op targeting a DIFFERENT file (the deferred
/// Grow/CreateGrow/CreateGrowDelete op always names the descriptor's OWN file, resolved through THIS field — the
/// handle->file binding the single CAP_WRITE CHECK gives). Reset (to 0) on every alloc/free/teardown.
static FILE_OPNAME: [[AtomicU32; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// U10: the descriptor GREW past its original EOF (a `sys_write_file` grow branch fired). Routes the dirty
/// descriptor to a deferred `Grow` op (in-place `fat::write_grow`, allocating+chaining as needed) at teardown
/// instead of the U9x in-place `write_at` flush. Reset on every alloc/free/teardown. Meaningful where `FILE_USED`.
static FILE_GREW: [[AtomicBool; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicBool::new(false) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// U10 M2/M3: the descriptor names a runtime-CREATED file (O_CREAT of a name absent from the staged set), not a
/// staged-backed one. Routes teardown to a `CreateGrow` deferred op (create the dir entry + grow-from-empty on
/// disk). ALSO the ONLY thing that admits `sys_unlink` (a staged/immutable file — e.g. HELLO.BIN EL0 code — has
/// `FILE_CREATED == false` and is refused with `-EACCES`), so it MUST reset on slot reuse or a recycled slot
/// would let an unrelated staged RW open be unlinked. Reset on every alloc/free/teardown. Where `FILE_USED`.
static FILE_CREATED: [[AtomicBool; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicBool::new(false) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];

/// STOR-1 S7: the descriptor names a DYNAMIC on-disk file — a PRE-EXISTING file on the mounted FAT volume
/// that is neither in the staged set nor a U10 created name. Opened READ-ONLY (`open_dynamic_ondisk`), it
/// carries no staged blob and no wstage buffer; `sys_read` serves it from the LIVE volume BY NAME through
/// the storage service task (`submit_read_file`). `FILE_DYNLEN` is the stored name's length: `0` == NOT a
/// dynamic descriptor (the presence flag + the publish word — Release on set, Acquire on read); `> 0` ==
/// dynamic, with the name in `FILE_DYNNAME[row][idx][..len]`. Reset (to 0) on every alloc/free/teardown so
/// a first-fit-reused slot never inherits a stale dynamic name. Only ever set knob-on (open refuses the
/// dynamic path off), so the whole dynamic mechanism is `irqstorage`-gated — a knob-off build has neither
/// this nor `FILE_DYNNAME` (and never enters the dynamic branch). Meaningful where `FILE_USED`.
#[cfg(feature = "irqstorage")]
static FILE_DYNLEN: [[AtomicU8; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [const { [const { AtomicU8::new(0) }; NFILE] }; crate::arch::memory::USER_SLOTS + 1];
/// STOR-1 S7: the dynamic on-disk descriptor's 8.3 name (`[..FILE_DYNLEN]`), the read source `sys_read`
/// resolves live. Written ONCE at open by the descriptor's own task (before the handle publish — the
/// FILE_OPNAME/FILE_CREATED stamping discipline), then read-only for the descriptor's life; published via
/// the `FILE_DYNLEN` Release/Acquire pair. `static mut` (a byte matrix, like `HELLO_BYTES`/`U10_BUF`);
/// single-writer per `[row][idx]` at any instant (the FILES-row discipline), so raw access is sound.
#[cfg(feature = "irqstorage")]
static mut FILE_DYNNAME: [[[u8; MAX_NAME]; NFILE]; crate::arch::memory::USER_SLOTS + 1] =
    [[[0u8; MAX_NAME]; NFILE]; crate::arch::memory::USER_SLOTS + 1];

/// U11x M2 (was U10 M3's per-row overlay): GLOBAL per-U10-name "this created file was UNLINKED" flag — the x86
/// twin of pi4's 0xE5'd directory entry. `sys_unlink` sets it IN-HANDLER, so the name vanishes for EVERY row
/// immediately (a plain re-open is `-ENOENT` from any process, an O_CREAT re-create is `-EBUSY` — see
/// `sys_open_dynamic`). Cleared when the file's deferred on-disk DELETE op DRAINS (`u10_flush_drain_one`), or —
/// with no queued op (the no-FAT in-memory core) — at the LAST-close release (`openf_decref`), so the name
/// becomes re-creatable exactly when the old file's delete has fully completed. This CANNOT stay per-row: with
/// cross-row sibling opens (U11x M2) an unlink must hide the name from all rows at once. Global-per-boot; the
/// U10 launchers' pre-flight self-heal covers metal re-runs on a persistent card.
static DYN_DELETED_G: [AtomicBool; N_U10_NAMES] =
    [const { AtomicBool::new(false) }; N_U10_NAMES];

/// STOR-1 S4: is the interrupt-driven SYNCHRONOUS-storage path active for created-file mutations
/// (grow/create/delete)? True iff the `irqstorage` knob is on, a FAT volume is mounted (`HELLO_STAGED`),
/// AND the storage service task is up (`service_ready()`). When true, `open_create_new`/`sys_write_grow`/
/// the last-close delete drive `fat.rs` SYNCHRONOUSLY via the service task (out of the IF-masked handler),
/// and the U10x deferred op-queue is NOT used. When false — knob off, no FAT (the in-memory core), or the
/// service not yet up — every mutation takes the deferred op-queue / in-memory path BYTE-IDENTICALLY to
/// pre-S4. The service task provably starts (main loop) before any created-file fixture runs, so this is
/// stable (monotonic) across a fixture's create/grow/unlink and its launcher's later verdict.
#[cfg(feature = "irqstorage")]
fn s4_sync_storage() -> bool {
    HELLO_STAGED.load(Ordering::Acquire) && crate::drivers::xhci::irqstorage::service_ready()
}
#[cfg(not(feature = "irqstorage"))]
fn s4_sync_storage() -> bool {
    false
}

// --- U11x M2: the GLOBAL open-file refcount table — the x86 twin of pi4 U11 M2's `OPEN_FILES`
// (`SpinMutex<[OpenFileRow; 16]>` keyed by the on-disk `(dir_lba, dir_off)`). x86's created-file identity space
// is the STATIC `U10_NAMES` table, so the table is indexed DIRECTLY by name-id — no row allocation, no join, and
// (because a re-create while a delete is pending is refused with `-EBUSY`) none of the pi4 recycled-slot-key
// aliasing class (b863304) by construction. Pure atomics (no lock): a new incref of a name is impossible once
// its `DYN_DELETED_G` flag is set (set BEFORE `OPENF_PENDING`), so the decref-to-zero edge can never race a
// fresh open of the SAME file; the demo launchers sequence opens/closes, and the residual open-vs-unlink TOCTOU
// (an open of a healthy name racing a concurrent unlink on another core) is ledgered in SECURITY.md — the
// product fix is the pi4 SpinMutex table. ---
/// The number of live descriptors (across ALL rows) naming created file `nameid` — incremented by every
/// successful created-file open (`open_create_new` / `open_created_sibling`, strictly BEFORE
/// `install_file_handle`, so its EAGAIN unwind through `files_free` pairs the decrement exactly once), and
/// decremented by every descriptor release (`files_free`, and `clear_files_row` at teardown — teardown counts
/// as close, the pi4 M2b semantics).
static OPENF_REFS: [AtomicU32; N_U10_NAMES] = [const { AtomicU32::new(0) }; N_U10_NAMES];
/// The file was unlinked while descriptors were still live — its deferred DELETE waits for the last close.
/// Set by `sys_unlink` (after `DYN_DELETED_G`), consumed by the last `openf_decref`.
static OPENF_PENDING: [AtomicBool; N_U10_NAMES] = [const { AtomicBool::new(false) }; N_U10_NAMES];
/// The `+1`-biased U10-queue slot holding the file's HELD deferred-DELETE op (`0` == none — the no-FAT
/// in-memory core, or the sole-opener case where the op released inside `sys_unlink` itself).
static OPENF_HELDSLOT: [AtomicU32; N_U10_NAMES] = [const { AtomicU32::new(0) }; N_U10_NAMES];

/// U11x M2: increment created file `nameid`'s global open count. See `OPENF_REFS` for the pairing discipline.
fn openf_incref(nameid: usize) {
    debug_assert!(nameid < N_U10_NAMES, "openf_incref: bad name-id");
    OPENF_REFS[nameid].fetch_add(1, Ordering::AcqRel);
}

/// U11x M2: decrement created file `nameid`'s global open count — THE single release seam (the pi4
/// `openfile_decref_at` twin). The LAST decrement of an unlink-PENDING file performs the deferred-free release:
/// the file's HELD delete op (if any) becomes drainable (`U10_HELD` cleared — the launcher's IF=1 drain is the
/// x86 reaper), and with NO queued op (no-FAT mode) the in-memory delete completes here, clearing
/// `DYN_DELETED_G` so the name is re-creatable. Atomics only — safe from the IF-masked syscall handler AND the
/// IF=0 teardown path (`clear_files_row`), exactly the two callers. Defensive on underflow (a pairing bug):
/// restore 0 and return, never wrap (a wrapped count would strand a held op for the boot).
fn openf_decref(nameid: usize) -> bool {
    debug_assert!(nameid < N_U10_NAMES, "openf_decref: bad name-id");
    let prev = OPENF_REFS[nameid].fetch_sub(1, Ordering::AcqRel);
    if prev == 0 {
        // Defensive: unpaired decref — undo the wrap without clobbering a concurrent legitimate incref
        // (a plain store(0) could erase it; the CAS only repairs the exact wrapped value).
        let _ = OPENF_REFS[nameid].compare_exchange(u32::MAX, 0, Ordering::AcqRel, Ordering::Acquire);
        return false;
    }
    if prev == 1 && OPENF_PENDING[nameid].swap(false, Ordering::AcqRel) {
        // Last close of an unlinked file — release its deferred DELETE.
        let held = OPENF_HELDSLOT[nameid].swap(0, Ordering::AcqRel);
        // STOR-1 S4c: knob-on, the unlinked file is ALREADY ON DISK (S4a create + S4b grow) and `sys_unlink`
        // enqueued NO op (`held == 0`) — the deferred DELETE runs SYNCHRONOUSLY here at the last close. But
        // this function is pure atomics precisely because it is callable from an IF=0 self-teardown, where
        // blocking on the service task is unsafe; so it does NOT block itself — it SIGNALS the caller
        // (`openf_release`, which knows its context) to perform the on-disk delete, and LEAVES `DYN_DELETED_G`
        // SET meanwhile so no re-create of the name can alias the still-present on-disk file. Returns true ==
        // "the caller must now perform the on-disk delete and clear the flag".
        #[cfg(feature = "irqstorage")]
        if s4_sync_storage() {
            debug_assert!(held == 0, "S4c: synchronous unlink should not have enqueued a held op");
            return true;
        }
        // Knob-off / no-FAT: the existing in-memory / deferred-op release (byte-identical).
        match (held as usize).checked_sub(1) {
            Some(k) => U10_HELD[k].store(false, Ordering::Release), // op now drainable at the launcher's IF=1
            None => DYN_DELETED_G[nameid].store(false, Ordering::Release), // no queued op — delete completes now
        }
    }
    false
}

/// STOR-1 S4c/S6: run the deferred on-disk DELETE for created file `nameid` SYNCHRONOUSLY via the storage
/// service task, then clear its `DYN_DELETED_G` gate so the name is re-creatable. `can_block` MUST hold (the
/// caller blocks on the service task). Split out of `openf_release` so the S6 unlink sweep — which runs the
/// name-state transitions under the NAMESPACE lock — can perform the delete AFTER releasing the lock (never a
/// service-task block under a spinlock; the S5 deadlock class).
///
/// STOR-1 S6 carry-over (S4-review note 3): the flag-clear is now GATED on delete SUCCESS. Pre-S6, `-EIO` cleared
/// `DYN_DELETED_G` unconditionally (matching the deferred drain's unconditional clear) → after a storage error a
/// knob-on re-create idempotently ADOPTED the stale on-disk entry (a 0-length descriptor over an N-byte on-disk
/// file). Now a failed delete LEAVES the name blocked (fail-SAFE: `-EBUSY` re-create + a chkdsk-reclaimable orphan,
/// never a stale-entry adoption); a successful delete clears it (re-creatable, clean — the common path).
#[cfg(feature = "irqstorage")]
fn openf_perform_delete(nameid: usize) {
    let name = U10_NAMES[nameid];
    let rc = unsafe { crate::drivers::xhci::irqstorage::submit_delete(name.as_bytes()) };
    if rc >= 0 {
        // Delete succeeded (or the file was already absent — `submit_delete` is idempotent, returns 0): the name
        // is truly gone, so re-open it.
        DYN_DELETED_G[nameid].store(false, Ordering::Release);
    }
    // else -EIO: leave DYN_DELETED_G SET — the on-disk file may still be present, so a re-create must NOT adopt it.
}

/// STOR-1 S4c: the last-close release for created file `nameid` — decref, and if that was the last close of
/// an unlink-PENDING file whose deferred on-disk DELETE must now run SYNCHRONOUSLY (knob-on), perform it via
/// the storage service task. `can_block` MUST be true only where the caller can block on the service task:
/// `files_free` (always — a syscall handler, or the launcher's direct call), and `clear_files_row` only when
/// its teardown is NOT the current task's own IF=0 `exit`/reap (it proves this via `current_user_cr3`).
/// Knob-off / no-FAT: a pure-atomic decref (`openf_decref` returns false) — byte-identical to pre-S4.
fn openf_release(nameid: usize, can_block: bool) {
    let need_delete = openf_decref(nameid);
    #[cfg(feature = "irqstorage")]
    if need_delete {
        if can_block {
            // Blocking-safe context — run the deferred on-disk DELETE synchronously via the service task.
            openf_perform_delete(nameid);
        } else {
            // A last-close release in a NON-blocking teardown context (an IF=0 `exit`/reap of the current
            // task). REACHABLE — NOT by the unlinking process (its descriptors were swept at unlink), but by
            // ANOTHER process that opened the file cross-process (a U6 grantee / an O_PUBLIC sharer — the
            // u6gx/u11m2 pattern) and became the LAST holder, then EXITS or FAULTS without an explicit
            // SYS_CLOSE. Blocking on the service task here is unsafe (the task's CR3 is being freed / there is
            // no current task to switch away from). So DEFER the delete the IF=0-safe way — the knob-off
            // mechanism: enqueue a `U10OP_DELETE` op (the file is on disk; no bytes needed) that a launcher's
            // IF=1 drain completes, clearing `DYN_DELETED_G` then — and leave the flag SET meanwhile so no
            // re-create aliases the still-present on-disk file. Best-effort completion (if no drain runs before
            // boot end the op strands, leaving the name blocked + a chkdsk-reclaimable orphan — fail-safe, no
            // corruption); the clean fix (shared on-disk backing so reads/deletes need no per-holder blocking)
            // is S5. Pure atomics + a 0-byte memcpy — IF=0-safe.
            u10_flush_enqueue(U10OP_DELETE, nameid as u32, 0, &[], false);
        }
    }
    #[cfg(not(feature = "irqstorage"))]
    let _ = (need_delete, can_block);
}

// =============================================================================================
// STOR-1 S6a — the created-file NAMESPACE lock: mutual atomicity for the coupled name SEQUENCES (the x86 twin
// of the pi4 F3 `NAMESPACE`). S5 narrowed the recycle/open-vs-unlink windows to fail-SAFE (a sibling can only
// ever name the caller-verified name-id) but left the source-resolve + re-validate NON-atomic. S6 closes them
// airtight: one lock makes the three created-name sequences mutually atomic, so no unlink/create can interleave
// inside another sequence's resolve→claim.
//
//   * SIBLING OPEN (`sys_open_dynamic` created branch): DYN_DELETED_G check -> `created_desc_any_row` resolve ->
//     re-check -> ACL -> `open_created_sibling` (re-validate the source + claim a descriptor). No submit — the
//     whole sequence runs under the lock.
//   * FRESH CREATE (`open_create_new`): re-check DYN_DELETED_G -> idempotent `created_desc_any_row` (a racing
//     create won -> open a sibling instead, ACL-checked) -> claim the descriptor + record the owner. The
//     idempotent on-disk `submit_create` runs BEFORE the lock (see `open_create_new`).
//   * UNLINK (`sys_unlink`): claim the name (DYN_DELETED_G swap) -> owned_clear -> pending marks -> invalidate
//     THIS row's descriptors (the atomic clears). The last-close on-disk DELETE runs AFTER the lock drops.
//
// ⚠ SPAN — the S5 DEADLOCK LESSON BINDS: the lock is IRQ-masked and held for the O(1) IN-MEMORY namespace
// decision ONLY, NEVER across a `submit`/BOT pump. Routing a blocking service-task round-trip under this
// spinlock re-creates the S5 deadlock class (a holder blocked on the service task while other cores spin on the
// lock). So both blocking disk ops are lifted OUT of the locked region: the idempotent `submit_create` runs
// before the lock (in `open_create_new`), and the last-close `submit_delete` runs after it (in `sys_unlink` +
// `openf_release`). The disk create/delete are idempotent AND serialized by the single service-task BOT writer,
// so lifting them out re-introduces no disk-level race; the lock closes the IN-MEMORY namespace (identity / ACL
// / refcount / descriptor-slot) race, which is the actual S5 residual.
//
// WHY sys_close/`files_free` need NOT take the lock: completing the recycle requires a `files_alloc` REUSE of a
// freed slot, and every created-file `files_alloc` happens INSIDE a namespace sequence (the open paths) under
// this lock — so a reuse cannot slip into another sequence's resolve. A bare `files_free` (close) that lands
// mid-resolve only makes the re-validate fail closed (`-ENOENT`, fail-safe), it cannot rebind a slot to a live
// different-name descriptor without an alloc. And the unlink+re-create reincarnation leg cannot interleave
// because unlink and create are themselves namespace sequences (one lock).
//
// LOCK ORDER: `NAMESPACE` ⊃ { `OWNED_FILES`, the FILE_*/handle atomics }. Inner accesses (`owned_*`,
// `files_alloc`/`files_free` clears, `openf_incref`/`openf_decref`, `install_file_handle`) run freely while
// NAMESPACE is held; NAMESPACE is never re-acquired while held (spin::Mutex is NOT reentrant — the sequences
// above take it exactly once). x86-only file; no cfg gate needed on the lock itself.

/// STOR-1 S6a: an IRQ-mask RAII (the x86 `IrqGuard` — x86 has only the `without_interrupts` closure form, so
/// this gives the pi4 RAII shape the namespace sequences need across their early returns). Masks on construct,
/// restores the PRIOR IF on drop. In a syscall handler IF is already 0 (SFMASK) so this is inert (stays masked);
/// in a launcher/IF=1 kernel task it masks for the lock hold and restores on release.
struct IrqGuard {
    was_enabled: bool,
}

impl IrqGuard {
    fn mask_save() -> Self {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        IrqGuard { was_enabled }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        if self.was_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

/// STOR-1 S6a: the created-file namespace lock (a `()` mutex — it guards SEQUENCES, not data; the tables keep
/// their own atomics). One volume -> one static, mirroring the pi4 F3 `NAMESPACE`.
static NAMESPACE: SpinMutex<()> = SpinMutex::new(());

/// STOR-1 S6a: the RAII hold on [`NAMESPACE`]. Field order is load-bearing: `_lock` drops FIRST (release the
/// mutex), `_irq` LAST (restore IF) — the lock is never held with IRQs unmasked.
struct NsGuard {
    _lock: spin::mutex::MutexGuard<'static, (), spin::relax::Spin>,
    _irq: IrqGuard,
}

/// STOR-1 S6a: mask IRQs, then take the namespace lock. See the `NAMESPACE` section header for span + ordering.
fn ns_lock() -> NsGuard {
    let irq = IrqGuard::mask_save();
    let lock = NAMESPACE.lock();
    NsGuard { _lock: lock, _irq: irq }
}

// =============================================================================================
// U6x: UnaFS owner/grants — the by-NAME namespace ACL, the x86 twin of the reviewed aarch64 U6 (owner/grants
// enforced at SYS_OPEN + SYS_FGRANT delegation + F1 owner-only unlink). Closes the U11x M2 ledger anchor: on
// x86, cross-process open/unlink of a created file was GRANT-FREE — any process could open/unlink any created
// name. This makes the by-name namespace secure-by-DEFAULT.
//
// OWNED-BY-DEFAULT: an O_CREAT of a NEW name records the creating principal as the file's OWNER (the file is
// PRIVATE); the O_PUBLIC mode bit opts a create into world-access. An open of an EXISTING owned file is admitted
// only for the owner or a principal the owner GRANTED (SYS_FGRANT); everyone else is -EACCES. A file with NO
// owner row (a STAGED file — HELLO.BIN/SCRATCH.BIN/GROW.BIN have no created-name identity — or a created file
// opened O_PUBLIC, or one whose owner has torn down) is PUBLIC: byte-identical to the pre-U6x behaviour.
//
// IDENTITY — the KEY vs the PRINCIPAL. x86 keys the ACL by the created file's `U10_NAMES` NAME-ID (a direct
// index, 0..N_U10_NAMES), NOT pi4's on-disk `(dir_lba, dir_off)`: x86's created-file identity space IS the
// static name table (the same key `DYN_DELETED_G` / `OPENF_*` use), so there is no recycled-key aliasing class
// by construction (U11x M2's reasoning) and no bounded-table "full" case — every creatable name has exactly one
// owner row (`owned_set_owner` cannot fail, so a private create has no fail-closed -ENOSPC path, a clean x86
// divergence from pi4). The PRINCIPAL is the address-space SLOT fenced by its `SLOT_GEN` incarnation (the x86
// (ASID, ASID_GEN) analogue — `SLOT_GEN[slot]` is bumped at the TOP of `clear_handle_row`): a stale owner/grant
// whose gen no longer matches never authorizes a recycled slot's next tenant.
//
// LIFETIME (in-kernel, VOLATILE — the ENFORCEMENT SEAM the on-disk UnaFS owner/grants:* attributes will feed
// once persistent files land; there is no on-disk owner format yet, and `fat.rs` is shared/off-lane): a row is
// CLEARED at SYS_UNLINK (the name is gone) and at OWNER TEARDOWN (`clear_handle_row` -> the file reverts to
// PUBLIC — no persistent principal keeps owning it; this also keeps the table self-cleaning across create/exit).
//
// LOCKING: one `SpinMutex` (`OWNED_FILES`), a short I/O-free critical section taken IRQ-masked via
// `crate::arch::without_interrupts` on EVERY access — the SYSCALL handler already runs IF-masked, but the
// teardown path (`clear_handle_row`) does not necessarily, so the mask makes the two symmetric (the pi4 IrqGuard
// discipline). No block I/O and no other lock is ever held across it.

/// U6x: grantees per owned file. Bounded; a full grant list is `-ENOSPC` on SYS_FGRANT.
const NFGRANT: usize = 4;

/// U6x: one grant edge on an owned file — a principal `(slot, gen)` and the rights it was granted. An EMPTY
/// slot has `rights == 0` (a real grant always carries >= 1 of CAP_READ|CAP_WRITE), so `rights == 0` doubles as
/// the free marker and disambiguates the raw slot 0 (a valid slot).
#[derive(Clone, Copy)]
struct FileGrant {
    slot: u32,
    slot_gen: u64, // the grantee's SLOT_GEN captured at grant time — the recycle fence
    rights: u32,
}

impl FileGrant {
    const EMPTY: Self = FileGrant { slot: 0, slot_gen: 0, rights: 0 };
}

/// U6x: one created file's ACL, indexed DIRECTLY by name-id. `owner_slot` is `+1`-biased (`0` == no owner ==
/// PUBLIC; slot 0 is a valid slot, so the bias disambiguates it). Guarded by `OWNED_FILES` — plain `Copy`
/// integers, the lock provides mutual exclusion.
#[derive(Clone, Copy)]
struct OwnedFile {
    owner_slot: u32, // +1-biased: 0 == public (no owner), else owner is (owner_slot - 1)
    owner_gen: u64,
    grants: [FileGrant; NFGRANT],
}

impl OwnedFile {
    const EMPTY: Self = OwnedFile { owner_slot: 0, owner_gen: 0, grants: [FileGrant::EMPTY; NFGRANT] };
}

/// U6x: the owner/grants table, one row per creatable name (`N_U10_NAMES`). Indexed directly by name-id.
static OWNED_FILES: SpinMutex<[OwnedFile; N_U10_NAMES]> =
    SpinMutex::new([OwnedFile::EMPTY; N_U10_NAMES]);

/// U6x: record `(owner_slot, owner_gen)` as the OWNER of `nameid` — called at the O_CREAT of a NEW private name.
/// OVERWRITES any prior row (defensive against a recycled name whose old owner row was not yet cleared) and
/// resets the grant list. Infallible (a direct index — no bounded-table "full" case).
fn owned_set_owner(nameid: usize, owner_slot: usize, owner_gen: u64) {
    if nameid >= N_U10_NAMES {
        return;
    }
    crate::arch::without_interrupts(|| {
        let mut t = OWNED_FILES.lock();
        t[nameid] = OwnedFile {
            owner_slot: (owner_slot + 1) as u32,
            owner_gen,
            grants: [FileGrant::EMPTY; NFGRANT],
        };
    });
}

/// U6x: the ACL verdict for a caller opening an EXISTING created file. `true` = ALLOW: PUBLIC (no owner row),
/// the caller IS the owner (gen-matched — full authority), or it holds a grant whose rights COVER the requested
/// access (`requested ⊆ granted`, gen-fenced). `false` = DENY (-EACCES). `requested` is CAP_READ, or
/// CAP_READ|CAP_WRITE for an RW/O_CREAT open.
fn owned_access_ok(nameid: usize, slot: usize, caller_gen: u64, requested: u32) -> bool {
    if nameid >= N_U10_NAMES {
        return true; // unknown name-id -> no ACL (public)
    }
    crate::arch::without_interrupts(|| {
        let t = OWNED_FILES.lock();
        let row = t[nameid];
        let Some(owner) = (row.owner_slot as usize).checked_sub(1) else {
            return true; // no owner -> PUBLIC
        };
        if owner == slot && row.owner_gen == caller_gen {
            return true; // owner: full authority
        }
        for g in row.grants.iter() {
            if g.rights != 0 && g.slot as usize == slot && g.slot_gen == caller_gen {
                return (requested & !g.rights) == 0; // granted iff the request is a subset
            }
        }
        false // owned, and the caller is neither owner nor a sufficiently-granted principal
    })
}

/// U6x: is `(slot, gen)` the CURRENT owner of `nameid`? `false` for a public/unknown file. `sys_fgrant` refuses
/// a non-owner FAST via this — before resolving (and thus leaking the validity of) the named grantee handle.
fn owned_is_owner(nameid: usize, slot: usize, caller_gen: u64) -> bool {
    if nameid >= N_U10_NAMES {
        return false;
    }
    crate::arch::without_interrupts(|| {
        let t = OWNED_FILES.lock();
        let row = t[nameid];
        match (row.owner_slot as usize).checked_sub(1) {
            Some(owner) => owner == slot && row.owner_gen == caller_gen,
            None => false,
        }
    })
}

/// U6x: may `(slot, gen)` UNLINK `nameid`? DELETE is an OWNER-only authority, distinct from content write: an
/// OWNED file may be unlinked ONLY by its current owner — a CAP_WRITE grantee gets content read/write, NEVER
/// delete (else it could `unlink` + `O_CREAT` the name to STEAL ownership and lock the real owner out — the F1
/// class). A PUBLIC file (no owner row) keeps the pre-U6x behaviour: any CAP_WRITE handle may unlink it.
fn owned_unlink_permitted(nameid: usize, slot: usize, caller_gen: u64) -> bool {
    if nameid >= N_U10_NAMES {
        return true;
    }
    crate::arch::without_interrupts(|| {
        let t = OWNED_FILES.lock();
        let row = t[nameid];
        match (row.owner_slot as usize).checked_sub(1) {
            Some(owner) => owner == slot && row.owner_gen == caller_gen, // owned -> owner-only
            None => true,                                                // public -> pre-U6x CAP_WRITE gate
        }
    })
}

/// U6x: is `nameid` PUBLIC (no owner row)? The launcher's verdict uses it to prove ownership reverted after the
/// unlink + last close (the pi4 C2 twin).
fn owned_is_public(nameid: usize) -> bool {
    if nameid >= N_U10_NAMES {
        return true;
    }
    crate::arch::without_interrupts(|| OWNED_FILES.lock()[nameid].owner_slot == 0)
}

/// U6x: drop `nameid`'s owner/grants row — called at SYS_UNLINK (the name is gone; a later O_CREAT re-create
/// sets its OWN owner). Idempotent no-op if the file was public.
fn owned_clear(nameid: usize) {
    if nameid >= N_U10_NAMES {
        return;
    }
    crate::arch::without_interrupts(|| {
        OWNED_FILES.lock()[nameid] = OwnedFile::EMPTY;
    });
}

/// U6x: at the teardown of `slot`, drop every file it OWNS (revert to PUBLIC — no persistent principal keeps
/// owning it, and the slot's next tenant is a DIFFERENT process) and sweep any GRANT naming `slot` (a grantee
/// that exited). Matches on the slot irrespective of gen — the whole address space is being torn down. Called
/// from `clear_handle_row`; keeps the table self-cleaning across create/exit cycles. (The gen fence already
/// makes an unswept stale entry harmless — this is hygiene + the owner-exit-reverts-to-public rule.)
fn owned_clear_owner_slot(slot: usize) {
    crate::arch::without_interrupts(|| {
        let mut t = OWNED_FILES.lock();
        for row in t.iter_mut() {
            if (row.owner_slot as usize).checked_sub(1) == Some(slot) {
                *row = OwnedFile::EMPTY;
            } else {
                for g in row.grants.iter_mut() {
                    if g.rights != 0 && g.slot as usize == slot {
                        *g = FileGrant::EMPTY;
                    }
                }
            }
        }
    });
}

/// U6x: SYS_FGRANT's table half. Verify `nameid` is owned by `(owner_slot, owner_gen)`, then add/update a grant
/// for `(grantee_slot, grantee_gen)` with `rights` (a CAP_READ|CAP_WRITE subset), or REMOVE it when
/// `rights == 0`. Returns `0`, or `-EACCES` (public/nonexistent file, or the caller is not its current owner) /
/// `-ENOSPC` (the bounded grant list is full — add path only). Reclaims a gen-STALE grant slot when claiming.
/// Only the current owner may mutate the ACL — checked here, so a non-owner is refused BEFORE any effect.
fn owned_grant(
    nameid: usize,
    owner_slot: usize,
    owner_gen: u64,
    grantee_slot: usize,
    grantee_gen: u64,
    rights: u32,
) -> i64 {
    if nameid >= N_U10_NAMES {
        return EACCES;
    }
    crate::arch::without_interrupts(|| {
        let mut t = OWNED_FILES.lock();
        let row = &mut t[nameid];
        // Only the file's CURRENT owner (gen-matched) may grant or revoke.
        if (row.owner_slot as usize).checked_sub(1) != Some(owner_slot) || row.owner_gen != owner_gen {
            return EACCES;
        }
        // REVOKE (rights == 0): drop any existing grant for this grantee incarnation. Future opens deny; a
        // handle the grantee ALREADY holds is unaffected (the ACL gates ACQUISITION, not held caps).
        if rights == 0 {
            for g in row.grants.iter_mut() {
                if g.rights != 0 && g.slot as usize == grantee_slot && g.slot_gen == grantee_gen {
                    *g = FileGrant::EMPTY;
                }
            }
            return 0;
        }
        // GRANT/UPDATE: update an existing grant for this grantee in place.
        for g in row.grants.iter_mut() {
            if g.rights != 0 && g.slot as usize == grantee_slot && g.slot_gen == grantee_gen {
                g.rights = rights;
                return 0;
            }
        }
        // Otherwise claim a free slot — or reclaim a gen-stale one (a grantee whose slot was recycled).
        for g in row.grants.iter_mut() {
            let stale =
                g.rights != 0 && SLOT_GEN[g.slot as usize].load(Ordering::Acquire) != g.slot_gen;
            if g.rights == 0 || stale {
                *g = FileGrant { slot: grantee_slot as u32, slot_gen: grantee_gen, rights };
                return 0;
            }
        }
        ENOSPC // the file's grant list is full
    })
}

/// U9x M2: mark descriptor `[row][idx]` dirty and cover [lo, hi) in its dirty range. On the FIRST write
/// (`FILE_DIRTY` false->true) SET [LO,HI) fresh to exactly [lo,hi); on later writes WIDEN by min/max — so the
/// first write never starts the flushed span at offset 0 (which would RMW an un-dirtied sector). Single-writer
/// per descriptor: a writable buffer is written by exactly ONE task mid IF-masked syscall (a shared writable
/// open is refused at `sys_open`), so the swap + load/store is race-free without a lock.
fn mark_dirty(row: usize, idx: usize, lo: u32, hi: u32) {
    if FILE_DIRTY[row][idx].swap(true, Ordering::AcqRel) {
        // Already dirty — widen the covered range to the union with the prior [LO,HI).
        let cur_lo = FILE_DIRTY_LO[row][idx].load(Ordering::Acquire);
        let cur_hi = FILE_DIRTY_HI[row][idx].load(Ordering::Acquire);
        FILE_DIRTY_LO[row][idx].store(cur_lo.min(lo), Ordering::Release);
        FILE_DIRTY_HI[row][idx].store(cur_hi.max(hi), Ordering::Release);
    } else {
        // First write — SET the range exactly (never min/max against the 0 init).
        FILE_DIRTY_LO[row][idx].store(lo, Ordering::Release);
        FILE_DIRTY_HI[row][idx].store(hi, Ordering::Release);
    }
}

// --- U9x M2 flush queue: dirty writable buffers awaiting persistence to disk. THE crux resolution — a File
// write lands in an in-memory wstage buffer INSIDE the IF-masked SYSCALL handler (no disk I/O there), and
// that buffer is FREED at the owning task's teardown (`clear_files_row`, IF=0) — but the flush needs IF=1
// disk I/O. So teardown COPIES each dirty descriptor's bytes + its (cluster, size, [lo,hi)) into this static
// queue (surviving the teardown), and the demo launcher DRAINS it at IF=1 (the shell-proven
// `fat::write_at`->`block::write_block` path) and frees each entry. A COPY (not the wstage slot itself) makes
// every entry self-contained — a stranded entry can never point at a freed buffer. A REVOKE never enqueues
// (`files_free` discards dirty), so a revoked write is never flushed. Bounded by `NWSTAGE` (at most one dirty
// entry per writable buffer, and the demo opens one). Populated at IF=0 on the fixture's CPU; drained by the
// launcher only AFTER it observes full teardown (`wstage_all_free()` — the Acquire edge that makes the
// enqueue's stores visible, since teardown enqueues strictly BEFORE it frees the wstage slot). ---
const NFLUSH: usize = NWSTAGE;
/// Per-entry presence: `true` == a dirty flush is pending. Claimed (CAS false->true) in `flush_enqueue`,
/// cleared LAST (Release) in `flush_drain_one` after the write-back.
static FLUSH_USED: [AtomicBool; NFLUSH] = [const { AtomicBool::new(false) }; NFLUSH];
/// The flush target: the file's FAT chain head, its total size (the EOF bound `write_at` clamps against), and
/// the dirty byte range [START, START+LEN). Meaningful only where `FLUSH_USED`.
static FLUSH_CLUSTER: [AtomicU32; NFLUSH] = [const { AtomicU32::new(0) }; NFLUSH];
static FLUSH_SIZE: [AtomicU32; NFLUSH] = [const { AtomicU32::new(0) }; NFLUSH];
static FLUSH_START: [AtomicU32; NFLUSH] = [const { AtomicU32::new(0) }; NFLUSH];
static FLUSH_LEN: [AtomicU32; NFLUSH] = [const { AtomicU32::new(0) }; NFLUSH];
/// Sticky: set if a `flush_enqueue` was ever DROPPED because the queue was full (impossible in the demo —
/// `NFLUSH == NWSTAGE` bounds concurrent dirty buffers — but a silent drop would mean a lost acknowledged
/// write, so the launcher verdict reads this and FAILs loudly rather than treating it as a clean flush).
static FLUSH_OVERFLOW: AtomicBool = AtomicBool::new(false);
/// The dirty bytes themselves — a copy of `wstage[start..start+len]`, taken at teardown BEFORE the wstage slot
/// is freed. One page each (the staged size bound). Row-major like `WSTAGE_BUF`.
static mut FLUSH_BUF: [[u8; PAGE_SIZE as usize]; NFLUSH] = [[0; PAGE_SIZE as usize]; NFLUSH];

/// Copy a dirty descriptor's staged bytes into a free flush-queue entry, capturing the flush target (cluster,
/// size, [start, start+len)). Called from `clear_files_row` at teardown BEFORE the wstage slot is freed — `src`
/// is `wstage_bytes(widx)[start..]`, still live here; `src.len()` is the dirty span (<= PAGE_SIZE, the buffer
/// bound). Fields written first, `FLUSH_LEN` published LAST (Release) so a scanning drainer that sees the slot
/// used also sees its fields. On a full queue: set the sticky `FLUSH_OVERFLOW`, log, return false (never a
/// silent drop).
fn flush_enqueue(cluster: u32, size: u32, start: u32, src: &[u8]) -> bool {
    let len = src.len();
    debug_assert!(len <= PAGE_SIZE as usize, "flush_enqueue: dirty span exceeds a page");
    for k in 0..NFLUSH {
        if FLUSH_USED[k].compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            unsafe {
                let dst = (&raw mut FLUSH_BUF).cast::<u8>().add(k * PAGE_SIZE as usize);
                core::ptr::copy_nonoverlapping(src.as_ptr(), dst, len);
            }
            FLUSH_CLUSTER[k].store(cluster, Ordering::Relaxed);
            FLUSH_SIZE[k].store(size, Ordering::Relaxed);
            FLUSH_START[k].store(start, Ordering::Relaxed);
            FLUSH_LEN[k].store(len as u32, Ordering::Release); // publish LAST
            return true;
        }
    }
    FLUSH_OVERFLOW.store(true, Ordering::Release);
    serial_println!(
        ":: U9x: FLUSH QUEUE FULL — dropped a dirty write-back (cluster={} {} bytes @ {}) ::",
        cluster, len, start
    );
    false
}

/// Drain ONE pending flush entry to disk via `fat::write_at` (in place — never grows/allocs/writes-FAT/touches
/// a directory), then free the entry. Returns `Some(true)` on a full write-back, `Some(false)` on an I/O/short
/// write (entry still freed — a demo flush failure FAILs the verdict, it never strands the queue or retries),
/// `None` if no entry is pending. Called ONLY from the launcher at IF=1 (the shell-proven disk-write context).
fn flush_drain_one(fs: &crate::fs::fat::FatFs) -> Option<bool> {
    for k in 0..NFLUSH {
        if FLUSH_USED[k].load(Ordering::Acquire) {
            let cluster = FLUSH_CLUSTER[k].load(Ordering::Acquire);
            let size = FLUSH_SIZE[k].load(Ordering::Acquire);
            let start = FLUSH_START[k].load(Ordering::Acquire);
            let len = FLUSH_LEN[k].load(Ordering::Acquire) as usize;
            let bytes = unsafe {
                let base = (&raw const FLUSH_BUF).cast::<u8>().add(k * PAGE_SIZE as usize);
                core::slice::from_raw_parts(base, len)
            };
            let ok = fs.write_at(cluster, size, start, bytes).map(|w| w == len).unwrap_or(false);
            FLUSH_LEN[k].store(0, Ordering::Relaxed);
            FLUSH_USED[k].store(false, Ordering::Release); // free LAST
            return Some(ok);
        }
    }
    None
}

/// True iff the flush queue holds no pending entry — the launcher's post-drain leak check (no dirty write-back
/// stranded).
fn flush_all_free() -> bool {
    (0..NFLUSH).all(|k| !FLUSH_USED[k].load(Ordering::Acquire))
}

// --- U10 deferred-op queue: file GROW / CREATE / DELETE work that needs disk I/O, deferred out of the
// IF-masked SYSCALL handler to the launcher's IF=1 drain (the U9x FLUSH-queue-that-survives-teardown pattern,
// kept SEPARATE from that queue so the metal-confirmed U9x in-place write-back path is untouched). A U10 fixture
// mutates its file IN MEMORY in-handler (a growable/created wstage buffer + the DYN_DELETED overlay); teardown
// (`clear_files_row`, GROW/CREATE) or `sys_unlink` (DELETE) enqueues a self-contained op COPY (name-id +
// op-kind + the bytes to persist); the launcher drains it at IF=1 by calling the ready-made `fat.rs` primitive
// (`write_grow` / `create_in_root` / `delete_located`), re-resolving the on-disk directory location BY NAME via
// `find_located` (x86 has no in-handler dir walk). NU10 == 1: a U10 fixture opens exactly ONE growable/created
// file, and the launchers drain strictly sequentially (u10x -> u10cx -> u10dx, each draining before it chains),
// so one slot suffices; a second concurrent op would trip `U10_OVERFLOW` (a loud FAIL, never a silent drop). ---
const NU10: usize = 1;
/// U10 op-kinds (in `U10_OP`). GROW: extend an existing file (`write_grow`). CREATE_GROW: create a fresh entry
/// then grow-from-empty (`create_in_root` if absent + `write_grow`). CREATE_GROW_DELETE: create+grow+delete —
/// exercises the full on-disk delete path (`delete_located`) for a file the fixture created then unlinked.
/// DELETE (STOR-1 S4c fallback): delete an ALREADY-ON-DISK file by name — the IF=0-safe deferral for a
/// last-close release that lands in a NON-blocking teardown context (an `exit`/reap of a cross-process
/// last-holder; see `openf_release`), where the synchronous `submit_delete` cannot run.
const U10OP_GROW: u32 = 1;
const U10OP_CREATE_GROW: u32 = 2;
const U10OP_CREATE_GROW_DELETE: u32 = 3;
const U10OP_DELETE: u32 = 4;
/// The U10 demo file names — the single source of truth an op's `U10_NAMEID` indexes, so the drain re-resolves
/// the on-disk directory entry by the SAME name the fixture named (`find_located`). GROW.BIN is also a staged
/// file (idx `GROW_STAGED_IDX`); FRESH.BIN/DELME.BIN/DEFER.BIN are runtime-created (never staged).
const U10_NAMES: [&str; 5] = [U10_GROW_NAME, U10C_NAME, U10D_NAME, U11M2_NAME, U6GX_NAME];
/// The count of `U10_NAMES` — the width of the per-row `DYN_DELETED` overlay.
const N_U10_NAMES: usize = U10_NAMES.len();
static U10_USED: [AtomicBool; NU10] = [const { AtomicBool::new(false) }; NU10];
static U10_OP: [AtomicU32; NU10] = [const { AtomicU32::new(0) }; NU10];
static U10_NAMEID: [AtomicU32; NU10] = [const { AtomicU32::new(0) }; NU10];
static U10_START: [AtomicU32; NU10] = [const { AtomicU32::new(0) }; NU10];
static U10_LEN: [AtomicU32; NU10] = [const { AtomicU32::new(0) }; NU10];
/// Sticky overflow — a dropped U10 op (queue full) is a lost acknowledged mutation; the launcher reads this and
/// FAILs loudly rather than treating it as a clean drain (the U9x `FLUSH_OVERFLOW` discipline).
static U10_OVERFLOW: AtomicBool = AtomicBool::new(false);
/// U11x M2: the entry is HELD — enqueued by `sys_unlink` while OTHER processes still hold the file open, so the
/// drain must skip it until the LAST close releases it (`openf_decref` clears it — the deferred-free). Set
/// strictly BEFORE the `U10_LEN` Release publish in `u10_flush_enqueue`; the launchers drain only after
/// observing their fixture's teardown (the standing sequencing invariant), so a drain never races the publish.
static U10_HELD: [AtomicBool; NU10] = [const { AtomicBool::new(false) }; NU10];
/// The op's bytes — a COPY of the wstage span, taken BEFORE the wstage slot frees (self-contained; a stranded op
/// can never point at a freed buffer). One page each (the staged size bound).
static mut U10_BUF: [[u8; PAGE_SIZE as usize]; NU10] = [[0; PAGE_SIZE as usize]; NU10];

/// Enqueue a U10 deferred op — copy its bytes + (op, name-id, start) into a free slot. `nameid` indexes
/// `U10_NAMES`. Fields written first, `U10_LEN` published LAST (Release). `held` (U11x M2) enqueues the op HELD
/// (not drainable until `openf_decref`'s last-close release clears `U10_HELD`); a non-held enqueue clears the
/// flag so a reused slot never inherits a stale hold. On a full queue: set the sticky `U10_OVERFLOW`, log,
/// return `None` (never a silent drop). Returns the claimed slot index (the unlink path records it in
/// `OPENF_HELDSLOT`). Called at IF=0 (teardown / unlink) on the fixture's CPU; drained by the launcher at IF=1
/// only after it observes teardown.
fn u10_flush_enqueue(op: u32, nameid: u32, start: u32, src: &[u8], held: bool) -> Option<usize> {
    let len = src.len();
    debug_assert!(len <= PAGE_SIZE as usize, "u10_flush_enqueue: op span exceeds a page");
    debug_assert!((nameid as usize) < U10_NAMES.len(), "u10_flush_enqueue: bad name-id");
    for k in 0..NU10 {
        if U10_USED[k].compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            unsafe {
                let dst = (&raw mut U10_BUF).cast::<u8>().add(k * PAGE_SIZE as usize);
                core::ptr::copy_nonoverlapping(src.as_ptr(), dst, len);
            }
            U10_OP[k].store(op, Ordering::Relaxed);
            U10_NAMEID[k].store(nameid, Ordering::Relaxed);
            U10_START[k].store(start, Ordering::Relaxed);
            U10_HELD[k].store(held, Ordering::Release); // before the LEN publish (see U10_HELD)
            U10_LEN[k].store(len as u32, Ordering::Release); // publish LAST
            return Some(k);
        }
    }
    U10_OVERFLOW.store(true, Ordering::Release);
    serial_println!(":: U10: OP QUEUE FULL — dropped a deferred op (op={} name={} {} bytes) ::", op, nameid, len);
    None
}

/// Drain ONE pending U10 op to disk via the ready-made `fat.rs` primitive, then free the entry. Returns
/// `Some(true)` iff EVERY disk step of the op succeeded (so a launcher can require a real on-disk effect and not
/// be fooled by a no-op drain), `Some(false)` on any I/O/short-write, `None` if no op is pending. Called ONLY
/// from a U10 launcher at IF=1.
fn u10_flush_drain_one(fs: &crate::fs::fat::FatFs) -> Option<bool> {
    for k in 0..NU10 {
        if U10_USED[k].load(Ordering::Acquire) && !U10_HELD[k].load(Ordering::Acquire) {
            let op = U10_OP[k].load(Ordering::Acquire);
            let nameid = U10_NAMEID[k].load(Ordering::Acquire) as usize;
            let start = U10_START[k].load(Ordering::Acquire);
            let len = U10_LEN[k].load(Ordering::Acquire) as usize;
            let bytes = unsafe {
                let base = (&raw const U10_BUF).cast::<u8>().add(k * PAGE_SIZE as usize);
                core::slice::from_raw_parts(base, len)
            };
            let name = U10_NAMES[nameid];
            let ok = match op {
                U10OP_GROW => u10_drain_grow(fs, name, start, bytes),
                U10OP_CREATE_GROW => u10_drain_create_grow(fs, name, bytes),
                U10OP_CREATE_GROW_DELETE => u10_drain_create_grow_delete(fs, name, bytes),
                U10OP_DELETE => u10_drain_delete(fs, name),
                _ => false,
            };
            // U11x M2: a drained DELETE completes the file's lifecycle — the name is re-creatable again
            // (unconditional: on a failed drain the pre-flight self-heal owns the on-disk state anyway).
            // S4c: the U10OP_DELETE fallback likewise clears the flag its enqueuer left set.
            if op == U10OP_CREATE_GROW_DELETE || op == U10OP_DELETE {
                DYN_DELETED_G[nameid].store(false, Ordering::Release);
            }
            U10_LEN[k].store(0, Ordering::Relaxed);
            U10_USED[k].store(false, Ordering::Release); // free LAST
            return Some(ok);
        }
    }
    None
}

/// True iff the U10 op-queue holds no pending op — the launcher's post-drain leak check.
fn u10_flush_all_free() -> bool {
    (0..NU10).all(|k| !U10_USED[k].load(Ordering::Acquire))
}

/// STOR-1 S4d: drain any pending U10 ops and return `(ok, count)` for a launcher verdict. Knob-OFF
/// (deferred replay): the launcher requires EXACTLY ONE op drained, every disk step true, the queue then
/// empty, and no overflow — byte-identical to the pre-S4 inline check. Knob-ON (S4 synchronous): the
/// create/grow/delete ALREADY persisted in-syscall via the service task, so the op-queue is EMPTY — `ok`
/// requires `count == 0` (nothing enqueued: the deferred-replay causal-fidelity gap is closed) + no
/// overflow. `count` is returned so the caller can log the mode. Called ONLY from a U10 launcher at IF=1.
fn u10_drain_verdict(fs: &crate::fs::fat::FatFs) -> (bool, u32) {
    let mut all_ok = true;
    let mut count = 0u32;
    while let Some(one) = u10_flush_drain_one(fs) {
        all_ok &= one;
        count += 1;
    }
    let clean = u10_flush_all_free() && !U10_OVERFLOW.load(Ordering::Acquire);
    let ok = if s4_sync_storage() {
        count == 0 && clean // synchronous: nothing was ever enqueued
    } else {
        all_ok && count == 1 && clean // deferred: exactly one op replayed to disk
    };
    (ok, count)
}

/// U10 GROW drain: extend the existing on-disk file `name` — `find_located` (by name, the x86 stand-in for the
/// pi4 in-handler dir walk) then `fat::write_grow` (alloc + zero-fill + chain new clusters as needed, RMW the
/// data, bump the directory size LAST). `write_grow` publishes the new size + chain head to the directory, so no
/// descriptor republish is needed here (the fixture already tore down). True iff the whole span was written.
fn u10_drain_grow(fs: &crate::fs::fat::FatFs, name: &str, start: u32, bytes: &[u8]) -> bool {
    match fs.find_located(name) {
        Ok((de, lba, off)) if !de.is_dir => fs
            .write_grow(de.first_cluster(), de.size, lba, off, start, bytes)
            .map(|(w, _ns, _nf)| w == bytes.len())
            .unwrap_or(false),
        _ => false,
    }
}

/// U10 CREATE drain: persist a runtime-created file `name` carrying `bytes` — create the directory entry
/// (idempotent: `find_located` FIRST, `create_in_root` only when genuinely absent, so a re-drain never plants a
/// duplicate 8.3 entry — the `create_in_root` "caller must confirm absent" contract) then `write_grow` from
/// empty (allocates the first cluster + sets the dir `first_cluster`/`size`). True iff the entry exists after and
/// the whole content was written.
fn u10_drain_create_grow(fs: &crate::fs::fat::FatFs, name: &str, bytes: &[u8]) -> bool {
    let (de, lba, off) = match fs.find_located(name) {
        Ok(loc) => loc, // already present (idempotent re-drain) — grow in place, never a second create
        Err(crate::fs::fat::FatError::NotFound) => match fs.create_in_root(name, 0x20) {
            Ok(loc) => loc, // fresh 0-length/0-cluster entry
            Err(_) => return false,
        },
        Err(_) => return false, // a real I/O / mount error — do NOT create over it
    };
    if bytes.is_empty() {
        return true; // a 0-length created file: the directory entry alone is the persisted state
    }
    fs.write_grow(de.first_cluster(), de.size, lba, off, 0, bytes)
        .map(|(w, _ns, _nf)| w == bytes.len())
        .unwrap_or(false)
}

/// U10 DELETE drain: exercise the FULL on-disk delete path for a file the fixture created + unlinked. The
/// fixture's create/grow never persisted (deferred, IF-masked), so the drain first CREATES + GROWS the file on
/// disk (a real directory entry + a real allocated + chained cluster), then ASSERTS it is genuinely there — the
/// mid-op EXISTENCE WITNESS: without it a no-op drain would leave `gone`/`freed`/`reusable` all vacuously true,
/// so a broken delete could masquerade as a passing one — THEN deletes it (`delete_located`: dir byte0 -> 0xE5,
/// then free the whole chain in ALL FAT copies). True iff create + grow + the existence witness + delete ALL
/// succeeded. NOTE: this is a launcher-side REPLAY of the fixture's create+grow+unlink sequence — a weaker causal
/// exercise than the pi4 in-handler unlink of an independently-persisted file (documented in the launcher).
fn u10_drain_create_grow_delete(fs: &crate::fs::fat::FatFs, name: &str, bytes: &[u8]) -> bool {
    let (de0, lba, off) = match fs.find_located(name) {
        Ok(loc) => loc,
        Err(crate::fs::fat::FatError::NotFound) => match fs.create_in_root(name, 0x20) {
            Ok(loc) => loc,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    if !bytes.is_empty()
        && !fs
            .write_grow(de0.first_cluster(), de0.size, lba, off, 0, bytes)
            .map(|(w, _ns, _nf)| w == bytes.len())
            .unwrap_or(false)
    {
        return false;
    }
    // Mid-op existence witness: the file MUST be on disk now — non-dir, size == the bytes written, with a real
    // chain head when non-empty. This is what makes the launcher's gone/freed/reusable checks non-vacuous.
    let (de1, lba1, off1) = match fs.find_located(name) {
        Ok(loc) => loc,
        _ => return false,
    };
    if de1.is_dir || de1.size != bytes.len() as u32 {
        return false;
    }
    if !bytes.is_empty() && de1.first_cluster() < 2 {
        return false;
    }
    // Delete: dir entry 0xE5 FIRST, then free the chain (every FAT entry -> 0 in all copies) — crash-safe order.
    fs.delete_located(lba1, off1, de1.first_cluster()).is_ok()
}

/// STOR-1 S4c DELETE-fallback drain: delete an ALREADY-ON-DISK file `name` (created + grown synchronously by
/// S4a/S4b) — the IF=1 completion for a last-close release that landed in a NON-blocking teardown context and
/// deferred the delete (`openf_release` -> `U10OP_DELETE`). No create/grow (the file already exists with its
/// real content + chain, unlike the CREATE_GROW_DELETE replay): `find_located` then `delete_located`. A `name`
/// already absent returns `true` (idempotent — a racing drain / self-heal already removed it).
fn u10_drain_delete(fs: &crate::fs::fat::FatFs, name: &str) -> bool {
    match fs.find_located(name) {
        Ok((de, lba, off)) if !de.is_dir => fs.delete_located(lba, off, de.first_cluster()).is_ok(),
        Ok(_) => false, // a directory under this name — never delete it via the file path
        Err(crate::fs::fat::FatError::NotFound) => true, // already gone — idempotent
        Err(_) => false,
    }
}

// --- U9x writable staging pool: the write twin of the read-only staged set. A small fixed pool of
// per-descriptor writable buffers. A File opened RW (SYS_OPEN mode bit0) claims a slot SEEDED from the
// file's staged content; a File SYS_WRITE overwrites it IN PLACE at the descriptor's offset (a pure memcpy,
// IF-masked-handler-safe); a SYS_READ through the same descriptor serves from it, so a read-back through the
// SAME cap witnesses the write. Per-slot single-writer: a slot is claimed by ONE descriptor at a time
// (`WSTAGE_USED` CAS), SEEDED before its File handle installs (no concurrent reader yet), then read/written
// only by that descriptor's owning task mid-syscall (IF-masked). M1 scope: purely in-memory — no disk
// write-back (that is M2). One page max per buffer (the staged size bound); `NWSTAGE` bounds concurrent
// writable opens (the demo needs one). ---
const NWSTAGE: usize = 3; // U11x M2: launcher scratch-row buffer + fixture primary + fixture sibling, concurrent
/// Per-slot presence: `true` == the pool slot holds a live writable buffer. Claimed (CAS false->true) in
/// `wstage_alloc`, cleared in `wstage_free`. The single source of truth for "is this pool slot in use".
static WSTAGE_USED: [AtomicBool; NWSTAGE] = [const { AtomicBool::new(false) }; NWSTAGE];
/// Each writable buffer's live byte length (== the file's size; writes are in-place and never grow it, so
/// this is fixed for the buffer's lifetime). Meaningful only where `WSTAGE_USED`.
static WSTAGE_LEN: [AtomicU32; NWSTAGE] = [const { AtomicU32::new(0) }; NWSTAGE];
/// The writable buffers themselves — one page each. `static mut` + raw-pointer access mirrors `HELLO_BYTES`;
/// per-slot single-writer (above) makes the access race-free without a lock. Flat row-major layout: slot k
/// occupies bytes [k*PAGE_SIZE, (k+1)*PAGE_SIZE).
static mut WSTAGE_BUF: [[u8; PAGE_SIZE as usize]; NWSTAGE] = [[0; PAGE_SIZE as usize]; NWSTAGE];

/// Claim a free writable-staging slot and SEED it from `seed` (the file's staged content, capped at one
/// page — the staged size bound guarantees `seed.len() <= PAGE_SIZE`). Returns the pool index, or `None` if
/// the pool is full. Called from `sys_open` on a RW open, BEFORE the descriptor/handle are installed, so no
/// concurrent reader can observe a half-seeded buffer.
fn wstage_alloc(seed: &[u8]) -> Option<usize> {
    // A writable buffer is exactly one page; a seed larger than that cannot be represented — reject (the RW
    // open then returns `-EMFILE`) rather than silently truncate, which would leave `FILE_SIZE` (the full
    // staged size) > the buffer length and let a near-EOF write clamp against `FILE_SIZE` run PAST the slot's
    // page. Every staged file is <= PAGE_SIZE today (HELLO.BIN is one-page-bounded at stage time; SCRATCH.BIN
    // is 1 KiB), so this never fires in M1 — it is a defensive guard keeping `FILE_SIZE == WSTAGE_LEN <=
    // PAGE_SIZE` for every RW descriptor, the invariant `sys_write_file`'s in-bounds memcpy relies on.
    if seed.len() > PAGE_SIZE as usize {
        return None;
    }
    let n = seed.len();
    for k in 0..NWSTAGE {
        if WSTAGE_USED[k].compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            // Seed the buffer from the staged content, then publish the length (Release) after the copy.
            unsafe {
                let dst = (&raw mut WSTAGE_BUF).cast::<u8>().add(k * PAGE_SIZE as usize);
                core::ptr::copy_nonoverlapping(seed.as_ptr(), dst, n);
            }
            WSTAGE_LEN[k].store(n as u32, Ordering::Release);
            return Some(k);
        }
    }
    None
}

/// Release writable-staging slot `widx` — clears the length then drops `WSTAGE_USED` LAST (Release), so the
/// slot is never seen free with a stale length. Called from `files_free`/`clear_files_row` when the owning
/// descriptor is released or its slot is torn down (no writable buffer outlives its descriptor).
fn wstage_free(widx: usize) {
    debug_assert!(widx < NWSTAGE, "wstage_free: out of range");
    WSTAGE_LEN[widx].store(0, Ordering::Release);
    WSTAGE_USED[widx].store(false, Ordering::Release);
}

/// The live bytes behind writable-staging slot `widx` (its current content, `WSTAGE_LEN` bytes) — what a
/// SYS_READ serves for a RW descriptor. Stable-length (writes are in-place), single-writer per slot.
fn wstage_bytes(widx: usize) -> &'static [u8] {
    debug_assert!(widx < NWSTAGE, "wstage_bytes: out of range");
    let len = WSTAGE_LEN[widx].load(Ordering::Acquire) as usize;
    unsafe {
        let base = (&raw const WSTAGE_BUF).cast::<u8>().add(widx * PAGE_SIZE as usize);
        core::slice::from_raw_parts(base, len)
    }
}

/// Overwrite writable-staging slot `widx` IN PLACE: copy `len` bytes from user VA `buf` to the buffer at
/// `offset`. The caller (`sys_write_file`) has already clamped `offset + len <= WSTAGE_LEN <= PAGE_SIZE` and
/// validated `buf..buf+len` inside the user window, so the copy stays inside this slot's page and the ring-3
/// VA equals the kernel VA in the live CR3. A pure memcpy — no disk, IF-masked-handler-safe.
fn wstage_write_at(widx: usize, offset: usize, buf: u64, len: usize) {
    debug_assert!(widx < NWSTAGE && offset + len <= PAGE_SIZE as usize, "wstage_write_at: out of range");
    unsafe {
        let dst = (&raw mut WSTAGE_BUF).cast::<u8>().add(widx * PAGE_SIZE as usize).add(offset);
        core::ptr::copy_nonoverlapping(buf as *const u8, dst, len);
    }
}

/// CFU-1 NOTE-1 rider: the USER-pointer twin of `wstage_write_at`. Overwrite writable-staging slot `widx` in
/// place from a RING-3 source VA `buf`, routing the read through the unified `copy_from_user` seam so no raw
/// user-pointer deref survives outside the CFU-1 boundary (the plain `wstage_write_at` stays for the ONE
/// kernel-pointer caller — the `u11m2` in-kernel pattern seed). Every caller here has already validated
/// `buf..buf+len` (the CAS-claim `user_range_ok`, or a prior `copy_from_user` into a bounce buffer) for a
/// range that CONTAINS this one, so the seam's re-check cannot fail for a well-formed caller (byte-behavior
/// unchanged); a malformed range fails `Err(EFAULT)` fail-closed — nothing copied — exactly like the other
/// CFU-1 sites. `#[must_use]` so the verdict cannot be silently dropped.
#[must_use]
fn wstage_write_from_user(widx: usize, offset: usize, buf: u64, len: usize) -> Result<(), i64> {
    debug_assert!(widx < NWSTAGE && offset + len <= PAGE_SIZE as usize, "wstage_write_from_user: out of range");
    // A &mut view of exactly this slot's [offset, offset+len) — proven inside the slot's page by the bound
    // above — so `copy_from_user` validates the ring-3 source window and copies into the slot, nothing else.
    let dst = unsafe {
        core::slice::from_raw_parts_mut(
            (&raw mut WSTAGE_BUF).cast::<u8>().add(widx * PAGE_SIZE as usize).add(offset),
            len,
        )
    };
    copy_from_user(dst, buf)
}

/// U10: EXTEND writable-staging slot `widx`'s live length to at least `new_len` (never shrinks) — the grow twin
/// of the fixed length `wstage_alloc` sets. A grow writes the extended bytes with `wstage_write_at` FIRST, then
/// publishes the new length here (Release) so `sys_read` (which serves `WSTAGE_LEN` bytes) sees the appended tail
/// only after it is written. Caps at `PAGE_SIZE` (the one-page buffer bound) — the caller's grow branch already
/// clamps `new_len <= PAGE_SIZE`, so this preserves the `FILE_SIZE == WSTAGE_LEN <= PAGE_SIZE` invariant.
fn wstage_set_len_at_least(widx: usize, new_len: u32) {
    debug_assert!(widx < NWSTAGE && new_len <= PAGE_SIZE as u32, "wstage_set_len_at_least: out of range");
    if new_len > WSTAGE_LEN[widx].load(Ordering::Acquire) {
        WSTAGE_LEN[widx].store(new_len, Ordering::Release);
    }
}

/// True iff the entire writable-staging pool is free — the U9x teardown-clear verifier (the writable twin of
/// `files_row_is_clear`): read by `u9x_launcher` after the fixture exits and its slot retires, proving no
/// writable buffer leaked (every RW open's slot returned to the pool on teardown).
fn wstage_all_free() -> bool {
    (0..NWSTAGE).all(|k| !WSTAGE_USED[k].load(Ordering::Acquire))
}

/// U11x: pack a `(generation, descriptor index)` pair into a `File` handle's value word. Low 32 bits = `idx + 1`
/// (the +1 bias keeps the whole word clear of the value word's `0`=Empty / `u64::MAX`=RESERVING sentinels for ANY
/// index and generation — `idx + 1 >= 1` so the low half is nonzero, and `idx + 1 <= NFILE` so the word is never
/// all-ones); high 32 bits = the slot's generation at open time. `file_desc_validate` decodes + validates. The
/// gen-0 encoding of index `idx` is exactly `idx + 1` — byte-identical to the pre-U11x bare file-id, so a fresh
/// open on a never-freed slot is unchanged.
fn file_id_pack(g: u32, idx: usize) -> u64 {
    ((g as u64) << 32) | ((idx + 1) as u64)
}

/// U11x: THE single point that turns a `File` handle's value word into a live descriptor index. Decodes the
/// packed `(gen, idx)`, bounds-checks `idx`, requires the slot LIVE (`FILE_USED`), and requires the packed
/// generation to equal the slot's CURRENT generation. The gen check is what closes the U9x revoke+reopen note:
/// after a slot is freed (gen bumped) and first-fit-REUSED by a different file (`FILE_USED` true again), a
/// lingering handle carrying the OLD gen fails here — no silent re-bind. Every File consumer
/// (`sys_read`/`sys_write_file`/`sys_seek`/`sys_close`, and `sys_cap_revoke`'s File-drop) funnels its file-id
/// through this ONE helper, so the descriptor-identity check is inherited once (no per-syscall re-derivation that
/// could drift). Returns the validated index; `None` == invalid/stale (the caller maps to `-EACCES`, or
/// `sys_close` to `-EBADF`). Acquire loads pair with the Release stores in `files_alloc`/`files_free`
/// (belt-and-braces: a row has one writer at a time — its own task mid-syscall, or teardown after exit).
fn file_desc_validate(row: usize, file_id: u64) -> Option<usize> {
    let idx = ((file_id & 0xFFFF_FFFF) as usize).checked_sub(1)?;
    if idx >= NFILE {
        return None;
    }
    if !FILE_USED[row][idx].load(Ordering::Acquire) {
        return None; // free slot — a closed/revoked/unopened descriptor (the U6bx–U9x presence check)
    }
    let g = (file_id >> 32) as u32;
    if FILE_GEN[row][idx].load(Ordering::Acquire) != g {
        return None; // slot was freed + reused since this handle was minted — stale, no rebind (U11x)
    }
    Some(idx)
}

/// Claim the first free descriptor in the caller's FILES row for a freshly-opened file, returning its
/// index (the caller biases it to the file-id `idx + 1`). `wstage` is the `+1`-biased writable-staging slot
/// for a RW open (`0` for a RO open). Publishes staged-idx/size/offset/wstage after the `FILE_USED` CAS
/// claim — safe because a resolver only reaches a descriptor via a File HANDLE, which `sys_open` installs
/// strictly AFTER this returns (stored Release regardless, belt-and-braces — the pi4 `files_alloc`
/// discipline). `None` if the row is full (-> `-EMFILE`; never grown). U9x M2: `cluster` is the file's on-disk
/// FAT chain head (the flush target; `0` == no disk backing), and the descriptor starts CLEAN (`FILE_DIRTY`
/// false) with an empty dirty range.
fn files_alloc(row: usize, staged_idx: u32, size: u32, wstage: u32, cluster: u32) -> Option<usize> {
    debug_assert!(row < FILE_USED.len(), "files_alloc: row out of range");
    for k in 0..NFILE {
        if FILE_USED[row][k].compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
        {
            FILE_STAGED[row][k].store(staged_idx, Ordering::Release);
            FILE_SIZE[row][k].store(size, Ordering::Release);
            FILE_OFFSET[row][k].store(0, Ordering::Release);
            FILE_WSTAGE[row][k].store(wstage, Ordering::Release);
            FILE_CLUSTER[row][k].store(cluster, Ordering::Release);
            FILE_DIRTY[row][k].store(false, Ordering::Release);
            FILE_DIRTY_LO[row][k].store(0, Ordering::Release);
            FILE_DIRTY_HI[row][k].store(0, Ordering::Release);
            // U10: the growable-file identity + grow/create flags are SLOT-LIFETIME state — reset them on every
            // claim (before the `FILE_USED` publish) so a first-fit-reused slot never inherits a prior tenant's
            // FILE_CREATED (which would let an immutable STAGED file be unlinked) or FILE_OPNAME/FILE_GREW.
            FILE_OPNAME[row][k].store(0, Ordering::Release);
            FILE_GREW[row][k].store(false, Ordering::Release);
            FILE_CREATED[row][k].store(false, Ordering::Release);
            // S7: clear the dynamic on-disk name (slot-lifetime state) so a reused slot is never seen as a
            // dynamic descriptor with a stale name. The caller (`open_dynamic_ondisk`) stamps it AFTER, before
            // the handle publish (the FILE_OPNAME/FILE_CREATED discipline). `irqstorage`-gated with the field.
            #[cfg(feature = "irqstorage")]
            FILE_DYNLEN[row][k].store(0, Ordering::Release);
            return Some(k);
        }
    }
    None
}

/// Release descriptor `idx` in the caller's FILES row — the unwind for a `sys_open` that allocated a
/// descriptor but then failed to install its handle (the sys_spawn reserve/unwind discipline), and the
/// per-handle drop `sys_cap_revoke` performs on a File. Clears the fields then drops `FILE_USED` LAST
/// (Release), so the slot is never seen free with stale fields. U9x: a RW descriptor's writable staging slot
/// is freed too (no writable buffer outlives its descriptor — pi4's backing is the disk; x86's is the pool).
/// The FILE_* descriptor clear + generation bump for `[row][idx]` — everything `files_free` does EXCEPT the
/// final `openf_release`. Returns the created-file name-id (if this was a created descriptor) so the caller can
/// perform the global-refcount release in the RIGHT context. Pure atomics + a wstage free — no service-task
/// block. S6: the `sys_unlink` sweep runs THIS under the NAMESPACE lock (atomic slot clears) and defers the
/// last-close on-disk delete to AFTER the lock (never a submit under the spinlock — the S5 deadlock class);
/// every other caller uses `files_free` (clear + release together in one blocking-safe step).
fn files_free_clear(row: usize, idx: usize) -> Option<usize> {
    debug_assert!(row < FILE_USED.len() && idx < NFILE, "files_free_clear: out of range");
    // U11x M2: capture the created-file identity BEFORE the clears — the decref (below, after the descriptor is
    // fully released) needs it. Every created descriptor was incref'd exactly once at open (before its handle
    // installed), so every release path through here (SYS_CLOSE, revoke, sys_open unwind, the unlink sweep)
    // pairs it exactly once.
    let openf_nameid = if FILE_CREATED[row][idx].load(Ordering::Acquire) {
        (FILE_OPNAME[row][idx].load(Ordering::Acquire) as usize).checked_sub(1)
    } else {
        None
    };
    // U9x M2: a REVOKE (or a sys_open unwind) DISCARDS any dirty bytes — it frees the writable buffer WITHOUT
    // enqueuing a flush, so a revoked File-write cap never persists stale bytes (the brief's revoke ordering:
    // revoke drops the dirty flag). Only a whole-task TEARDOWN (`clear_files_row`) enqueues dirty bytes.
    if let Some(widx) = (FILE_WSTAGE[row][idx].load(Ordering::Acquire) as usize).checked_sub(1) {
        wstage_free(widx);
    }
    FILE_WSTAGE[row][idx].store(0, Ordering::Release);
    FILE_CLUSTER[row][idx].store(0, Ordering::Release);
    FILE_DIRTY[row][idx].store(false, Ordering::Release);
    FILE_DIRTY_LO[row][idx].store(0, Ordering::Release);
    FILE_DIRTY_HI[row][idx].store(0, Ordering::Release);
    FILE_STAGED[row][idx].store(0, Ordering::Release);
    FILE_SIZE[row][idx].store(0, Ordering::Release);
    FILE_OFFSET[row][idx].store(0, Ordering::Release);
    // U10: clear the growable-file identity + grow/create flags (slot-lifetime state — a revoke/close/unwind
    // frees the slot exactly like teardown; a reused slot must start clean).
    FILE_OPNAME[row][idx].store(0, Ordering::Release);
    FILE_GREW[row][idx].store(false, Ordering::Release);
    FILE_CREATED[row][idx].store(false, Ordering::Release);
    // S7: clear the dynamic on-disk name (slot-lifetime state — a close/revoke/unwind frees the slot exactly
    // like teardown; a reused slot must not be seen as dynamic with a stale name). Gated with the field.
    #[cfg(feature = "irqstorage")]
    FILE_DYNLEN[row][idx].store(0, Ordering::Release);
    FILE_USED[row][idx].store(false, Ordering::Release);
    // U11x: bump the slot's generation LAST — so the next `files_alloc` reuse lands on a fresh gen and any handle
    // still carrying the old (gen, idx) fails `file_desc_validate`'s gen check (no sibling rebind). Last because a
    // validator that observed the slot LIVE with the old gen resolved a genuinely-live descriptor (the same
    // file); the rebind window only opens once the slot is both free AND reused.
    FILE_GEN[row][idx].fetch_add(1, Ordering::AcqRel);
    // U11x M2: the descriptor is fully released — return the created-file name-id (bounded) so the caller drops
    // its global open count in the right context (S6 defers the sweep's release out of the namespace lock).
    openf_nameid.filter(|&n| n < N_U10_NAMES)
}

/// Release open-file descriptor `[row][idx]`: clear its state, then (created files) drop the global open count.
/// S4c: `files_free` is always a BLOCKING-SAFE context (a syscall handler — sys_close / revoke / open-unwind — or
/// a launcher's direct call; NEVER an IF=0 self-teardown, and NEVER under the NAMESPACE lock), so the last-close
/// synchronous on-disk delete may block here.
fn files_free(row: usize, idx: usize) {
    if let Some(nameid) = files_free_clear(row, idx) {
        openf_release(nameid, true);
    }
}

/// Clear an ENTIRE per-task open-file row at teardown — the file twin of `clear_handle_row`'s handle wipe
/// (which calls this). Presence dropped last per descriptor, so no torn intermediate looks live. U9x: each
/// descriptor's writable staging slot is freed too. `slot` is a PRIVATE slot (0..USER_SLOTS); `SHARED_ROW`
/// is never torn down (its opens persist, like its caps).
fn clear_files_row(slot: usize) {
    debug_assert!(slot < crate::arch::memory::USER_SLOTS, "clear_files_row: not a private slot");
    // STOR-1 S4c: may an S4 synchronous last-close DELETE block on the storage service task in THIS teardown?
    // Only when this is NOT the current task tearing down its OWN address space — `exit`/reap runs IF=0
    // mid-death (blocking there would resume a task whose CR3 is being freed: corruption) and the scheduler
    // reaper has no current task at all (an off-scheduler `submit` would fault). A LAUNCHER tearing down
    // ANOTHER slot is a live scheduled kernel task (its `user_cr3 == 0`, never equal to the slot's CR3), so
    // it may block. `exit` of a ring-3 task has `current.user_cr3 == slot_cr3(slot)`; the reaper has no
    // current → both resolve to `false`. (Computed once; the loop below hands it to `openf_release`.)
    #[cfg(feature = "irqstorage")]
    let teardown_can_block = match crate::arch::sched::current_user_cr3() {
        Some(ucr3) => ucr3 != crate::arch::memory::slot_cr3(slot),
        None => false,
    };
    #[cfg(not(feature = "irqstorage"))]
    let teardown_can_block = false;
    for k in 0..NFILE {
        // U11x M2: capture the created-file identity up front — teardown counts as CLOSE (the pi4 M2b
        // semantics), so each created descriptor decrefs the global open count after its fields clear; the last
        // decref of an unlink-pending file releases the deferred delete (atomics only — IF=0-safe).
        let openf_nameid = if FILE_CREATED[slot][k].load(Ordering::Acquire) {
            (FILE_OPNAME[slot][k].load(Ordering::Acquire) as usize).checked_sub(1)
        } else {
            None
        };
        // U9x M2: a DIRTY, disk-backed writable descriptor persists at teardown — COPY its dirty bytes +
        // (cluster, size, [lo,hi)) into the flush queue BEFORE freeing the wstage slot (the entry is a COPY, so
        // it survives the free and can never point at a freed buffer). The launcher drains it to disk at IF=1.
        // This is the whole-task teardown path (normal exit AND the fault-kill path, via `clear_handle_row`),
        // so a write acknowledged to ring 3 is persisted, not lost — and never stranded. A clean descriptor, or
        // one with no disk backing (`cluster == 0`: in-memory mode / no FAT), just frees. Enqueue BEFORE
        // `wstage_free`: the launcher drains only after observing `wstage_all_free()` (the Acquire edge pairing
        // with this `wstage_free`'s Release), so it always sees a fully-populated queue entry.
        if let Some(widx) = (FILE_WSTAGE[slot][k].load(Ordering::Acquire) as usize).checked_sub(1) {
            if FILE_DIRTY[slot][k].load(Ordering::Acquire) {
                let all = wstage_bytes(widx);
                // U10 op-routing precedence — a created file is ALSO a grown file (its first write grows from
                // empty), so CREATED wins: CreateGrow (persist the whole file) > Grow (extend a staged file) >
                // U9x in-place. Each names THIS descriptor's own file via `FILE_OPNAME` (never a hardcoded name),
                // so a deferred op can never target a different file than the checked handle wrote.
                let nameid = (FILE_OPNAME[slot][k].load(Ordering::Acquire) as usize).checked_sub(1);
                if FILE_CREATED[slot][k].load(Ordering::Acquire) {
                    // A runtime-CREATED file, still open at exit (a created file that was UNLINKED freed its
                    // descriptor at unlink — enqueuing a CreateGrowDelete there — so it never reaches here dirty).
                    // Enqueue only when a FAT volume is present (HELLO_STAGED — the launcher pre-flight signal):
                    // in the no-FAT in-memory core there is nothing to persist to, so a queued op would just
                    // strand (the launcher skips the drain) and trip a false overflow on the next fixture's op.
                    // U11x M2: SUPPRESSED for an unlink-pending name (`DYN_DELETED_G`) — a cross-row holder of
                    // an unlinked file exiting dirty must NOT re-persist it (its only remaining persistence is
                    // the HELD delete op; a CreateGrow here would both resurrect the file after its delete
                    // drains and overflow the NU10 == 1 queue).
                    // STOR-1 S4: knob-on, a created file was persisted SYNCHRONOUSLY (S4a create + S4b grow),
                    // so there is nothing to flush at teardown — skip the enqueue (the op-queue is retired
                    // when on). `!s4_sync_storage()` keeps the deferred CreateGrow byte-identical knob-off.
                    if let Some(nameid) = nameid {
                        let size = FILE_SIZE[slot][k].load(Ordering::Acquire) as usize;
                        if !s4_sync_storage()
                            && HELLO_STAGED.load(Ordering::Acquire)
                            && size <= all.len()
                            && !DYN_DELETED_G[nameid].load(Ordering::Acquire)
                        {
                            u10_flush_enqueue(U10OP_CREATE_GROW, nameid as u32, 0, &all[..size], false);
                        }
                    }
                } else if FILE_GREW[slot][k].load(Ordering::Acquire) {
                    // A GROWN staged file (GROW.BIN) — persist the extended dirty span via fat::write_grow. Only
                    // when disk-backed: FILE_CLUSTER (== GROW_CLUSTER) is `0` in the in-memory core (no FAT), so
                    // this both gates the enqueue and prevents an in-memory strand (mirrors the U9x cluster gate).
                    // STOR-1 S4b: knob-on the grow was already persisted synchronously — skip the enqueue.
                    if let Some(nameid) = nameid {
                        let cluster = FILE_CLUSTER[slot][k].load(Ordering::Acquire);
                        let lo = FILE_DIRTY_LO[slot][k].load(Ordering::Acquire);
                        let hi = FILE_DIRTY_HI[slot][k].load(Ordering::Acquire);
                        if !s4_sync_storage() && cluster != 0 && lo < hi && (hi as usize) <= all.len() {
                            u10_flush_enqueue(U10OP_GROW, nameid as u32, lo, &all[lo as usize..hi as usize], false);
                        }
                    }
                } else {
                    // U9x M2: an in-place write to a disk-backed file (SCRATCH.BIN) — the existing FLUSH queue.
                    let cluster = FILE_CLUSTER[slot][k].load(Ordering::Acquire);
                    if cluster != 0 {
                        let size = FILE_SIZE[slot][k].load(Ordering::Acquire);
                        let lo = FILE_DIRTY_LO[slot][k].load(Ordering::Acquire);
                        let hi = FILE_DIRTY_HI[slot][k].load(Ordering::Acquire);
                        if lo < hi && (hi as usize) <= all.len() {
                            flush_enqueue(cluster, size, lo, &all[lo as usize..hi as usize]);
                        }
                    }
                }
            }
            wstage_free(widx);
        }
        FILE_WSTAGE[slot][k].store(0, Ordering::Release);
        FILE_CLUSTER[slot][k].store(0, Ordering::Release);
        FILE_DIRTY[slot][k].store(false, Ordering::Release);
        FILE_DIRTY_LO[slot][k].store(0, Ordering::Release);
        FILE_DIRTY_HI[slot][k].store(0, Ordering::Release);
        FILE_STAGED[slot][k].store(0, Ordering::Release);
        FILE_SIZE[slot][k].store(0, Ordering::Release);
        FILE_OFFSET[slot][k].store(0, Ordering::Release);
        // U10: clear the growable-file identity + grow/create flags (slot-lifetime state).
        FILE_OPNAME[slot][k].store(0, Ordering::Release);
        FILE_GREW[slot][k].store(false, Ordering::Release);
        FILE_CREATED[slot][k].store(false, Ordering::Release);
        // S7: clear the dynamic on-disk name at whole-task teardown too (slot-lifetime state, same as
        // FILE_OPNAME/FILE_CREATED). Not strictly required for safety — a first-fit reuse resets it in
        // `files_alloc` before the slot goes live — but it honors the field's "reset on every alloc/free/
        // teardown" invariant and keeps a torn-down slot from carrying a stale dynamic name. Gated with the field.
        #[cfg(feature = "irqstorage")]
        FILE_DYNLEN[slot][k].store(0, Ordering::Release);
        FILE_USED[slot][k].store(false, Ordering::Release);
        // U11x: bump each slot's generation at teardown too (last, per slot) — so a recycled slot never hands its
        // next tenant a stale-gen descriptor that a lingering file-id could rebind to.
        FILE_GEN[slot][k].fetch_add(1, Ordering::AcqRel);
        // U11x M2: teardown counts as CLOSE — the global open-count decref (the pi4 M2b teardown-decrement twin;
        // the last decref of an unlink-pending file RELEASES its deferred delete). Knob-off: one atomic store,
        // legal on this IF=0 dying path. S4c knob-on: the delete runs SYNCHRONOUSLY, so it may block ONLY when
        // `teardown_can_block` proved this is a launcher's non-self teardown (never `exit`/reap self-death).
        if let Some(nameid) = openf_nameid {
            if nameid < N_U10_NAMES {
                openf_release(nameid, teardown_can_block);
            }
        }
    }
    // (U10 M3's per-row DYN_DELETED reset is gone — the overlay is GLOBAL now (U11x M2, `DYN_DELETED_G`) and
    // clears when the deferred delete completes: at the drain, or at the last-close release in no-FAT mode.)
}

/// True iff the entire FILES row for `row` is free — the U6bx teardown-clear verifier (the file twin of
/// `handle_row_is_clear`, the aarch64 `files_row_is_clear` twin). Read by `u6bx_launcher` after the
/// fixture exits and its slot retires: teardown clears the row, transitioning this false->true, proving
/// no open file outlives its owning slot.
fn files_row_is_clear(row: usize) -> bool {
    debug_assert!(row < FILE_USED.len(), "files_row_is_clear: row out of range");
    (0..NFILE).all(|k| !FILE_USED[row][k].load(Ordering::Acquire))
}

/// The `U10_NAMES` index of `name`, or `None` if it is not a U10 demo file. The single map from a name to the
/// `+1`-biased `FILE_OPNAME` a created/growable descriptor carries and the `U10_NAMEID` a deferred op indexes.
fn u10_name_id(name: &str) -> Option<u32> {
    U10_NAMES.iter().position(|n| *n == name).map(|i| i as u32)
}

/// The `U10_NAMES` index of a name that O_CREAT may CREATE — the runtime files (FRESH.BIN / DELME.BIN), NOT the
/// staged GROW.BIN (index 0, which is always resolved by the staged path). `None` for anything else — so O_CREAT
/// of an arbitrary name is `-ENOENT`, never a way to mint a file outside the demo set.
fn u10_creatable_nameid(name: &str) -> Option<u32> {
    match u10_name_id(name) {
        Some(id) if id != 0 => Some(id), // FRESH.BIN / DELME.BIN
        _ => None,
    }
}

/// The `(row, index)` of a LIVE runtime-created descriptor for `nameid` in ANY private row (U11x M2 — was
/// row-scoped `created_desc_in_row`), or `None`. A created file "exists" for a second open (idempotent
/// create-if-present) / a sibling open — from the SAME process or ANOTHER (the cross-process open the refcount
/// table counts) — exactly while one of its descriptors is live anywhere: the x86 in-memory stand-in for the
/// aarch64 on-disk `find_located` after an in-handler `create_in_root`. The caller's own row is scanned first
/// (prefer the local copy as the seed source). SHARED_ROW never holds created descriptors (refused at open).
fn created_desc_any_row(prefer_row: usize, nameid: u32) -> Option<(usize, usize)> {
    let find_in = |r: usize| {
        (0..NFILE).find(|&k| {
            FILE_USED[r][k].load(Ordering::Acquire)
                && FILE_CREATED[r][k].load(Ordering::Acquire)
                && FILE_OPNAME[r][k].load(Ordering::Acquire) == nameid + 1
        })
    };
    if prefer_row < crate::arch::memory::USER_SLOTS {
        if let Some(k) = find_in(prefer_row) {
            return Some((prefer_row, k));
        }
    }
    (0..crate::arch::memory::USER_SLOTS)
        .filter(|&r| r != prefer_row)
        .find_map(|r| find_in(r).map(|k| (r, k)))
}

/// Install a `File` handle over an already-allocated descriptor `fid` carrying `rights` — the shared tail of
/// every open path (staged, created, sibling): pack the slot's current generation into the file-id, reserve a
/// handle (unwinding the descriptor on a full handle table, `-EAGAIN`), then publish kind + rights + the live
/// file-id LAST (Release), so a resolver that sees the live value sees File + its rights. Returns the handle idx.
fn install_file_handle(row: usize, fid: usize, rights: u32) -> i64 {
    let file_id = file_id_pack(FILE_GEN[row][fid].load(Ordering::Acquire), fid);
    let Some(h) = handle_install(row, HANDLE_RESERVING) else {
        // S6 seat-fold 1: this unwind's `files_free` routes a CREATED descriptor through `openf_release`,
        // decrementing the global open count. That is safe against a concurrent last-close ONLY via the
        // non-local invariant pending ⟹ deleted ⟹ the open was already refused `-EBUSY`: a descriptor that
        // reached this install cannot have its name mid-delete (every create/sibling path re-checks
        // `DYN_DELETED_G` under the NAMESPACE lock before claiming), so this decref never races the last
        // close's release to zero. A dynamic on-disk descriptor (S7) has no name-id at all, so `files_free`
        // returns `None` here and skips `openf_release` entirely — the invariant is vacuous for it.
        files_free(row, fid); // no handle slot — release the descriptor (and its writable slot); no leak
        return EAGAIN;
    };
    handle_set_kind(row, h, KIND_FILE);
    handle_set_rights(row, h, rights);
    handle_set(row, h, file_id);
    h as i64
}

/// The DYNAMIC-open path (U10 M2/M3) — reached when a name is NOT in the staged set. Resolves, in order: a LIVE
/// runtime-created file in this row (idempotent create / sibling open); [M3: a created-then-deleted name -> gone];
/// an O_CREAT target (a fresh created file). Anything else -> `-ENOENT`. A created file is inherently RW, so this
/// refuses SHARED_ROW (the private-single-writer rule the U9x/M1 grow path relies on) up front.
fn sys_open_dynamic(row: usize, name: &str, mode: u64) -> i64 {
    let create = mode & O_CREAT != 0;
    if row == SHARED_ROW {
        return EACCES; // a created descriptor is RW; SHARED_ROW is refused (the writable-open discipline)
    }
    if let Some(nameid) = u10_name_id(name) {
        // STOR-1 S6a: the SIBLING-DECISION sequence runs under the NAMESPACE lock — deleted-check -> resolve ->
        // ACL -> open-sibling are now MUTUALLY ATOMIC against `sys_unlink` and `open_create_new`, so no unlink or
        // create can interleave inside the resolve->claim (retiring S5's non-atomic source-resolve + re-validate
        // residual). No `submit` runs in this region (the sibling path is pure atomics), so the lock is never
        // held across a service-task block — the S5 deadlock class stays closed. RELEASED before the create path
        // below (`open_create_new` takes NAMESPACE itself; spin::Mutex is not reentrant).
        let ns = ns_lock();
        // U11x M2: the DELETED check comes FIRST — before the any-row scan and before any resource claim. An
        // unlinked name is gone for EVERY row the moment `sys_unlink` sets the global flag, even while other
        // rows' descriptors keep the file's content alive (the deferral): a plain re-open is `-ENOENT`, and an
        // O_CREAT re-create is `-EBUSY` until the deferred on-disk delete completes. Under the S6a lock this and
        // the resolve below are atomic, so the S5b post-resolve re-check is subsumed (nothing can claim the name
        // mid-sequence) — the ordering that mattered pre-S6 is now enforced by exclusion.
        if DYN_DELETED_G[nameid as usize].load(Ordering::Acquire) {
            return if create { EBUSY } else { ENOENT };
        }
        // A live created file in ANY private row -> open ANOTHER descriptor to it (idempotent 2nd create / a
        // same-row sibling / the U11x M2 CROSS-PROCESS open). U6x: the by-NAME ACL gate — an open of an EXISTING
        // owned file is admitted only for the owner or a sufficiently-granted principal (public files pass). The
        // requested rights follow the open mode (RW or O_CREAT -> R|W; else R); a denial is a clean -EACCES with
        // nothing claimed. The gen fence is the caller's CURRENT SLOT_GEN.
        if let Some((srcrow, existing)) = created_desc_any_row(row, nameid) {
            let requested = if mode & 1 != 0 || create { CAP_READ | CAP_WRITE } else { CAP_READ };
            let caller_gen = SLOT_GEN[row].load(Ordering::Acquire);
            if !owned_access_ok(nameid as usize, row, caller_gen, requested) {
                return EACCES;
            }
            return open_created_sibling(row, srcrow, existing, nameid as usize, requested);
        }
        drop(ns); // not a live sibling — release the lock before the create path re-acquires it
    }
    // O_CREAT of a creatable name -> a fresh 0-length created file (the first write grows it). U6x: owned-by-
    // default unless O_PUBLIC (opt out into world-access).
    if create {
        if let Some(nameid) = u10_creatable_nameid(name) {
            return open_create_new(row, nameid, mode & O_PUBLIC != 0);
        }
    }
    // STOR-1 S7/S8: the name is neither staged nor a U10 name. Knob-on, fall through to DYNAMIC on-disk
    // resolution — an open of ANY pre-existing file on the mounted FAT volume resolves through the service task,
    // retiring the U6bx BSP-staged-set constraint. S7 opened it read-only; S8 honors mode bit0, so a RW open
    // gets CAP_WRITE and its writes route through `sys_write_file`'s overwrite-only dynamic branch. Knob-off /
    // no-FAT / pre-service -> ENOENT below (byte-identical to pre-S7).
    //
    // CANONICALIZE to the FAT 8.3 UPPERCASE form BEFORE deciding — this is a SECURITY-critical step. The
    // on-disk resolver the dynamic path submits to (`find_located` -> `DirEntry::eq_name`) matches
    // case-INSENSITIVELY, but `staged_lookup` / `u10_name_id` compare byte-EXACT against uppercase constant
    // tables. Without canonicalizing, a CASE VARIANT of an owned/created U10 name (e.g. "owned.bin") would miss
    // both exclusion checks yet resolve on disk to the OWNED file — bypassing the U6gx owner ACL and reading a
    // private file (a confidentiality break); a variant of a closed created name ("fresh.bin") would likewise
    // re-resolve it from disk as PUBLIC. Uppercasing makes the exclusion effectively case-insensitive (the
    // tables are uppercase): a variant of ANY staged/U10 name is EXCLUDED here and falls to `-ENOENT` (its
    // canonical form is handled by the staged/U10 paths above, owner ACL intact), so ONLY a genuinely
    // arbitrary on-disk file — with no U10 name-id in any casing, hence never ownable — reaches the dynamic
    // open. `open_dynamic_ondisk` then stores + resolves the canonical name (find_located matches it live).
    #[cfg(feature = "irqstorage")]
    if s4_sync_storage() {
        let mut canon = [0u8; MAX_NAME];
        let cn = name.len().min(MAX_NAME); // sys_open already bounded name.len() <= MAX_NAME
        canon[..cn].copy_from_slice(&name.as_bytes()[..cn]);
        canon[..cn].make_ascii_uppercase();
        // Uppercasing ASCII keeps valid UTF-8 (the source was already a validated &str); the Err arm is
        // unreachable but fails closed to -ENOENT.
        if let Ok(cname) = core::str::from_utf8(&canon[..cn]) {
            if staged_lookup(cname).is_none() && u10_name_id(cname).is_none() {
                return open_dynamic_ondisk(row, cname, mode);
            }
        }
    }
    ENOENT
}

/// U10 M2: open a FRESH runtime-created file — a 0-length RW descriptor backed by an EMPTY writable staging
/// buffer (the first write grows it from empty; the real dir-entry + first-cluster alloc DEFER to the launcher
/// drain). Marks the descriptor CREATED + stamps its `FILE_OPNAME` so the grow branch fires and teardown enqueues
/// a `CreateGrow` op naming THIS file. Rights `CAP_READ | CAP_WRITE` (O_CREAT implies write). Errnos as the
/// staged path: `-EMFILE` (no writable slot / FILES row full), `-EAGAIN` (handle table full).
fn open_create_new(row: usize, nameid: u32, public: bool) -> i64 {
    // U11x M2 defense in depth: `sys_open_dynamic` already refuses a delete-pending name up front, but this
    // function is also callable directly (the u11m2 / u6x launchers) — enforce the invariant AT the create so no
    // caller can mint a second live file under a name whose delete has not completed.
    if (nameid as usize) < N_U10_NAMES && DYN_DELETED_G[nameid as usize].load(Ordering::Acquire) {
        return EBUSY;
    }
    // STOR-1 S4a: knob-on, create the 0-length directory entry SYNCHRONOUSLY on the live volume via the
    // storage service task — so the file appears on disk IN-SYSCALL (retiring the U10 deferred CreateGrow
    // for the create half). Done FIRST, before any descriptor/wstage claim, so a create failure is a clean
    // -EIO with nothing to unwind (a create that SUCCEEDS but a later slot-alloc fails leaves a harmless
    // 0-length orphan the idempotent re-submit / the launcher pre-flight self-heal reconciles). The service
    // task's `Create` is idempotent (`find_located` first), so a re-open of the same created name never
    // plants a duplicate 8.3 slot. Off (knob off / no FAT / service not up) -> the deferred/in-memory path
    // below persists at teardown exactly as pre-S4.
    #[cfg(feature = "irqstorage")]
    if s4_sync_storage() {
        let name = U10_NAMES[nameid as usize];
        if unsafe { crate::drivers::xhci::irqstorage::submit_create(name.as_bytes()) } < 0 {
            return EIO;
        }
    }
    // STOR-1 S6a: the IN-MEMORY claim is atomic against a concurrent sibling-open / unlink under NAMESPACE. The
    // idempotent `submit_create` above ran BEFORE the lock (never a service-task block under the spinlock — the
    // S5 deadlock class). RAII-held to function end / every early return.
    let _ns = ns_lock();
    // Authoritative deleted re-check under the lock — a concurrent unlink could have claimed the name since the
    // racy pre-check above (or since this caller's `sys_open_dynamic` resolve found nothing).
    if (nameid as usize) < N_U10_NAMES && DYN_DELETED_G[nameid as usize].load(Ordering::Acquire) {
        return EBUSY;
    }
    // A racing O_CREAT on another core may have created this name since the caller decided to create (its
    // `sys_open_dynamic` resolve found nothing, then released the lock before calling here; a direct launcher
    // create can also target an already-live name). Open a SIBLING idempotently rather than minting a SECOND
    // descriptor + a second owner row (which would STEAL ownership from the winning creator). ACL-checked — the
    // winner may have created it private. The demo callers are single-threaded at setup, so this never fires for
    // them (byte-identical); it closes the create-races-create window on true SMP.
    if let Some((srcrow, existing)) = created_desc_any_row(row, nameid) {
        let caller_gen = SLOT_GEN[row].load(Ordering::Acquire);
        if !owned_access_ok(nameid as usize, row, caller_gen, CAP_READ | CAP_WRITE) {
            return EACCES;
        }
        return open_created_sibling(row, srcrow, existing, nameid as usize, CAP_READ | CAP_WRITE);
    }
    let Some(w) = wstage_alloc(&[]) else {
        return EMFILE; // the writable staging pool is full
    };
    let Some(fid) = files_alloc(row, CREATED_STAGED_SENTINEL, 0, (w + 1) as u32, 0) else {
        wstage_free(w);
        return EMFILE; // this task's open-file row is full
    };
    FILE_CREATED[row][fid].store(true, Ordering::Release);
    FILE_OPNAME[row][fid].store(nameid + 1, Ordering::Release);
    // U6x: owned-by-default — a PRIVATE create records the creator (this row, fenced by its current SLOT_GEN) as
    // the OWNER; O_PUBLIC keeps the pre-U6x open-by-anyone behaviour (no owner row). Recorded before the handle
    // install; direct-index, so it cannot fail (no fail-closed unwind). A created file is always on a private row
    // (SHARED_ROW is refused at `sys_open_dynamic`), so slot 0..USER_SLOTS is a valid gen-fenced owner.
    if !public {
        owned_set_owner(nameid as usize, row, SLOT_GEN[row].load(Ordering::Acquire));
    }
    // U11x M2: incref AFTER the created identity is stamped and BEFORE `install_file_handle` — its EAGAIN unwind
    // routes through `files_free`, which decrefs by that identity, so every failure path pairs exactly once (an
    // incref after the install would leave the unwind decrementing an un-incremented count: underflow).
    openf_incref(nameid as usize);
    install_file_handle(row, fid, CAP_READ | CAP_WRITE)
}

/// U10 M2/M3 + U11x M2: open ANOTHER descriptor to a live created file (`[srcrow][existing]`) — the idempotent
/// second O_CREAT open, the delete fixture's sibling handle, and (U11x M2) the CROSS-PROCESS open (`srcrow !=
/// row`). The sibling's identity is stamped from the CALLER-VERIFIED `nameid` (never a re-read of the source
/// slot), and it reads/writes through the U11x M2 global refcount (incref before `install_file_handle` — the
/// unwind-pairing rule). **STOR-1 S5a:** knob-on (`s4_sync_storage()`) a created-file descriptor READS the LIVE
/// shared on-disk backing (`sys_read` -> `created_read_live`), so the sibling seeds its wstage EMPTY — no snapshot
/// COPY of the source's private buffer — retiring the ledgered torn-copy / cross-file-disclosure residual (U11x
/// M2 residual 3) by construction. Knob-off / no-FAT / pre-service it keeps snapshot-copying the source wstage
/// (reads still serve wstage) — byte-identical to pre-S5. `sys_unlink` invalidates every sibling of a name in the
/// unlinking row via the shared `FILE_OPNAME` name-id.
fn open_created_sibling(row: usize, srcrow: usize, existing: usize, nameid: usize, rights: u32) -> i64 {
    // STOR-1 S6a: every caller of `open_created_sibling` now holds the NAMESPACE lock (the `sys_open_dynamic`
    // sibling branch + the `open_create_new` race fallback), so `created_desc_any_row`'s resolve and this
    // re-validation are ATOMIC against any close+first-fit-reuse of `[srcrow][existing]` — the recycle window S5
    // could only NARROW (SF-3) is now CLOSED. This source re-validation is kept as defense-in-depth (provably
    // passes under the lock): re-check the source STILL is a live created descriptor for the caller-verified
    // `nameid`; else fail closed (`-ENOENT`). Stamping identity from `nameid` (never a re-read of the source's
    // opname) keeps the sibling naming only the file the ACL admitted.
    if !(FILE_USED[srcrow][existing].load(Ordering::Acquire)
        && FILE_CREATED[srcrow][existing].load(Ordering::Acquire)
        && FILE_OPNAME[srcrow][existing].load(Ordering::Acquire) as usize == nameid + 1)
    {
        return ENOENT;
    }
    // S5a Change 2: seed EMPTY knob-on (reads go live, no snapshot copy — residual 3 closed by construction); keep
    // the snapshot seed knob-off (reads serve wstage). The sibling still owns a (0-length knob-on) writable buffer
    // so an RW sibling's in-place write-through can mirror into it; that mirror is dead weight for reads knob-on.
    let seed: &[u8] = if s4_sync_storage() {
        &[]
    } else {
        match (FILE_WSTAGE[srcrow][existing].load(Ordering::Acquire) as usize).checked_sub(1) {
            Some(ew) => wstage_bytes(ew),
            None => &[],
        }
    };
    let size = FILE_SIZE[srcrow][existing].load(Ordering::Acquire);
    let Some(w) = wstage_alloc(seed) else {
        return EMFILE;
    };
    let Some(fid) = files_alloc(row, CREATED_STAGED_SENTINEL, size, (w + 1) as u32, 0) else {
        wstage_free(w);
        return EMFILE;
    };
    FILE_CREATED[row][fid].store(true, Ordering::Release);
    FILE_OPNAME[row][fid].store((nameid + 1) as u32, Ordering::Release); // caller-verified identity
    openf_incref(nameid);
    // U6x: install exactly the ACL-vetted rights the open mode asked for — an RW open (owner, or a WRITE
    // grantee) gets CAP_READ|CAP_WRITE; an RO open (a READ-only grantee) gets CAP_READ. The `sys_open_dynamic`
    // ACL already proved `rights ⊆ granted`, so this never amplifies. (Pre-U6x callers all opened RW, so the
    // common path is byte-identical.)
    install_file_handle(row, fid, rights)
}

/// STOR-1 S7: stamp descriptor `[row][idx]`'s dynamic on-disk name — copy `name` into `FILE_DYNNAME` (bounded
/// to `MAX_NAME`; the caller already validated `name.len() <= MAX_NAME`) then PUBLISH the length via
/// `FILE_DYNLEN` (Release) so a reader that Acquires a non-zero length also sees the name bytes. Single-writer
/// per `[row][idx]` (the opening task, before the handle publish). Returns the stored length.
#[cfg(feature = "irqstorage")]
fn dyn_name_set(row: usize, idx: usize, name: &[u8]) -> usize {
    let n = name.len().min(MAX_NAME);
    // SAFETY: single-writer per slot (this task, pre-handle-publish); the slot is in range (`files_alloc`
    // returned `idx < NFILE`, `row` is the caller's row).
    unsafe {
        let dst = (&raw mut FILE_DYNNAME[row][idx]).cast::<u8>();
        core::ptr::copy_nonoverlapping(name.as_ptr(), dst, n);
    }
    FILE_DYNLEN[row][idx].store(n as u8, Ordering::Release); // publish LAST — the name bytes are now visible
    n
}

/// STOR-1 S7: read descriptor `[row][idx]`'s dynamic on-disk name into `out`, returning its length (`0` == not
/// a dynamic descriptor). Acquire-loads `FILE_DYNLEN` first (pairs with `dyn_name_set`'s Release), then copies
/// exactly that many name bytes — so a reader that observes a live length observes the matching bytes.
#[cfg(feature = "irqstorage")]
fn dyn_name_get(row: usize, idx: usize, out: &mut [u8; MAX_NAME]) -> usize {
    let n = (FILE_DYNLEN[row][idx].load(Ordering::Acquire) as usize).min(MAX_NAME);
    if n == 0 {
        return 0;
    }
    // SAFETY: `FILE_DYNNAME` is written once before the handle that reached this read was published, and the
    // name is stable for the descriptor's life (dynamic descriptors are read-only), so this read races nothing.
    unsafe {
        let src = (&raw const FILE_DYNNAME[row][idx]).cast::<u8>();
        core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), n);
    }
    n
}

/// STOR-1 S7: open a PRE-EXISTING arbitrary on-disk file — a name that is neither in the staged set nor a U10
/// created/ownable name — as a READ-ONLY descriptor backed by the LIVE volume. Reached from `sys_open_dynamic`
/// ONLY knob-on (`s4_sync_storage()`: `irqstorage` + a mounted FAT + the service task up) and ONLY for a
/// non-U10 name (U10 names keep their created-file semantics + owner ACL — a closed created file stays
/// `-ENOENT`, never re-resolved from disk as PUBLIC). This retires the U6bx BSP-staged-set constraint: an open
/// no longer requires the file to be pre-read into the staged buffer.
///
/// STOR-1 S8: mode bit0 now selects the descriptor's rights — RO (`CAP_READ`) or RW
/// (`CAP_READ | CAP_WRITE`). A RW dynamic descriptor's writes route through `sys_write_file`'s dynamic branch:
/// a synchronous, strictly OVERWRITE-ONLY live write-through BY NAME (`submit_write_file`), which by contract
/// never grows the file, allocs clusters, or touches the directory — the on-disk SIZE stays immutable for the
/// boot even though the CONTENT may now change. This IS a deliberate widening: any ring-3 task with `sys_open`
/// can now overwrite any genuinely-arbitrary (non-staged, non-U10) on-disk file. It is bounded three ways:
/// (1) MF2 stays closed by EXCLUSION — `sys_open_dynamic` canonicalizes to 8.3 UPPERCASE and drops every staged
/// name (HELLO.BIN EL0 code) and every U10 owned/created name in ANY casing BEFORE this path, so a writable
/// dynamic descriptor can only name a file that is neither code nor ownable; (2) the ACL surface is unchanged —
/// a dynamic on-disk file has no U10 name-id, so it can never enter `OWNED_FILES` and is PUBLIC-by-default for
/// writes exactly as S7 made it for reads (invariant 5: mirror created-file policy, don't invent new policy);
/// (3) overwrite-only — no growth, no allocation. The size comes from the service task (`submit_stat`, live
/// `find_located`), the one fact the IF-masked handler cannot resolve itself.
///
/// No NAMESPACE lock: a dynamic on-disk file is outside the U10 mutation namespace (it can't be created,
/// grown, or unlinked through the syscall API), so its resolve races no create/unlink — and neither the
/// `submit_stat` block nor the write-through submit runs under `ns_lock` (the S5 deadlock class stays closed).
/// Errnos: `-ENOENT` (absent / a directory), `-EIO` (an I/O error / a size that overflows the stat channel),
/// `-EMFILE` (the caller's FILES row is full), `-EAGAIN` (handle table full).
#[cfg(feature = "irqstorage")]
fn open_dynamic_ondisk(row: usize, name: &str, mode: u64) -> i64 {
    // S8: honor mode bit0. A RW open endows CAP_WRITE (writes route through the overwrite-only dynamic branch
    // in `sys_write_file`); RO keeps the S7 read-only cap. No wstage EITHER way — a dynamic descriptor never
    // stages (FILE_WSTAGE stays 0); its writes go straight to the live volume, its reads straight off it.
    let rw = mode & 1 != 0;
    // STOR-1 S9: a RW dynamic descriptor must be a PRIVATE single-writer slot (mirroring `sys_open_staged`'s
    // SHARED_ROW refusal) — the S9 grow path advances the offset un-CAS'd on the single-writer assumption, and
    // SHARED_ROW is the multi-tenant kernel window where two tasks could race one writable descriptor. No
    // fixture opens a dynamic file RW on SHARED_ROW; defensive, and it hardens the S8 overwrite path too (its
    // CAS claim was the only guard before). RO dynamic opens (S7) are unaffected.
    if rw && row == SHARED_ROW {
        return EACCES;
    }
    // Resolve on the LIVE volume via the service task (blocks — the caller is a scheduled AP task, exactly as
    // `sys_read`/`sys_write_file` already block). Returns the on-disk size (`>= 0`) or a negative errno.
    let rc = unsafe { crate::drivers::xhci::irqstorage::submit_stat(name.as_bytes()) };
    if rc < 0 {
        return if rc as i64 == ENOENT { ENOENT } else { EIO }; // ENOENT (absent/dir) vs any other -> EIO
    }
    let size = rc as u32;
    // A dynamic descriptor: NO staged blob (CREATED sentinel, so `staged_bytes`/`STAGED_NAMES.get` fail closed
    // if the dynamic branch is ever bypassed), NO wstage buffer (`0` — writes go live, never staged), NO flush
    // cluster (`0`). FILE_OPNAME stays 0 (not growable/created) and FILE_CREATED stays false (not unlinkable —
    // `sys_unlink` refuses it, exactly like a staged file), so this reuses the existing descriptor lifecycle
    // untouched — the ONLY change from a created/staged RW descriptor is the empty wstage + the FILE_DYNLEN key.
    let Some(fid) = files_alloc(row, CREATED_STAGED_SENTINEL, size, 0, 0) else {
        return EMFILE; // the caller's open-file row is full
    };
    // Stamp the dynamic identity (name + the FILE_DYNLEN publish) BEFORE the handle install — the FILE_OPNAME /
    // FILE_CREATED stamping order. `sys_read`/`sys_write_file` key their live-by-name branches on FILE_DYNLEN.
    dyn_name_set(row, fid, name.as_bytes());
    let rights = if rw { CAP_READ | CAP_WRITE } else { CAP_READ };
    install_file_handle(row, fid, rights)
}

/// SYS_OPEN(name_ptr, name_len, mode) -> a handle index, or a negative errno. The FIRST resource-OPEN through
/// the object table (the aarch64 U6b/U9 twin): validate + copy the name, look it up in the STAGED set, record
/// an open-file descriptor in the caller's FILES row, and install a `File` handle (first-free). U9x: `mode`
/// bit0 selects the rights the handle carries — `0` (RO, as U6bx) = `CAP_READ`; `1` (RW) =
/// `CAP_READ | CAP_WRITE`, the write cap a File `SYS_WRITE` presents, PLUS a per-descriptor WRITABLE staging
/// buffer (seeded from the file's staged content) that writes land in and reads serve from. (Higher `mode`
/// bits are reserved and ignored — no O_CREAT/O_TRUNC: create/grow are a later arc.)
///
/// Ordering mirrors the twin (and sys_spawn): every fallible READ-ONLY lookup first (name bound/copy, staged
/// lookup), so a failure there returns with nothing to unwind; RESOURCES claimed last (a RW open's writable
/// staging slot, then a descriptor, then a handle), each failure unwinding the prior claims — no leaked slot
/// on any path. Errnos: `-EINVAL` (bad name length), `-EFAULT` (name range outside the window), `-ENOENT`
/// (not in the staged set — non-UTF-8 names match nothing), `-EMFILE` (no writable staging slot, or the
/// FILES row full), `-EAGAIN` (handle table full).
fn sys_open(name_ptr: u64, name_len: u64, mode: u64) -> i64 {
    let row = caller_row();
    // 1. Bound + copy the name — the sys_write pointer discipline: the WHOLE range inside the user
    //    window (overflow rejected), then a bounded direct read (ring-3 VA == kernel VA in the live CR3).
    let n = name_len as usize;
    if n == 0 || n > MAX_NAME {
        return EINVAL;
    }
    // CFU-1: validate + copy the name through the READ seam in one call — `n == name_len`, so the
    // validated range is identical to the open-coded predicate this replaces (a bad name range is
    // -EFAULT with nothing claimed).
    let mut namebuf = [0u8; MAX_NAME];
    if let Err(e) = copy_from_user(&mut namebuf[..n], name_ptr) {
        return e;
    }
    let Ok(name) = core::str::from_utf8(&namebuf[..n]) else {
        return ENOENT; // a non-UTF-8 name matches no staged entry
    };
    // 2. Read-only lookup — nothing claimed yet, so a miss returns cleanly. A name NOT in the staged set may be a
    //    live runtime-CREATED file in this row (idempotent / sibling open) or an O_CREAT target (U10 M2/M3); the
    //    dynamic-open path handles those, and returns -ENOENT if the name is neither. The STAGED path below is
    //    UNCHANGED (U9x/U6bx byte-for-byte).
    let Some((sidx, size)) = staged_lookup(name) else {
        return sys_open_dynamic(row, name, mode);
    };
    sys_open_staged(row, sidx, size, mode)
}

/// The STAGED half of `sys_open` — factored out so the MF2 immutable-code guard is exercisable by a kernel-side
/// witness (`mf2_witness`) without a ring-3 name pointer. Reached with `sidx`/`size` from `staged_lookup`. Behaviour
/// is UNCHANGED (a pure extraction of the pre-S6 body): claim resources LAST — a RW open's writable staging slot,
/// then a descriptor, then a handle — each failure unwinding the prior claims.
fn sys_open_staged(row: usize, sidx: u32, size: u32, mode: u64) -> i64 {
    // 3. Claim resources LAST — for a RW open, a writable staging slot FIRST (seeded from the file's staged
    //    content, so a read before any write sees the original bytes), then a descriptor, then a handle. RO
    //    (bit0 clear) keeps the U6bx read-only path: no writable slot, `CAP_READ` only.
    let rw = mode & 1 != 0;
    // STOR-1 review (MF2): HELLO.BIN (staged index 0) is IMMUTABLE EL0 CODE — the program image the U2
    // loader ran and `sys_spawn` re-instantiates. Model it read-only AT THE SOURCE: refuse a writable
    // open. This is the ROOT-CAUSE close for the S3 live write-through — which resolves the target BY
    // NAME (`find_located` + `write_at`) and would otherwise overwrite the boot-critical executable on
    // disk, surviving reboot, defeating the default staged path's `staged_cluster(0) == 0` never-flush
    // protection — AND for the pre-S3 in-memory-dirty leg, in one place. SCRATCH.BIN/GROW.BIN keep their
    // write-through (demo scratch, not code). No fixture opens HELLO.BIN RW, so this regresses nothing.
    if rw && sidx == 0 {
        return EACCES;
    }
    // U9x M2 (folding the M1 review's SHARED_ROW writable-open note): a writable open needs a PRIVATE
    // single-writer descriptor row — its wstage buffer is written by exactly one task mid IF-masked syscall,
    // with no lock. SHARED_ROW is the multi-tenant kernel window (U1a/U1b/U2 run there with `user_cr3 == 0`, so
    // `caller_row()` falls back to it), where two tasks could race one writable descriptor + the unsynchronized
    // staging memcpy. Refuse it (mirroring SHARED_ROW's refusal as a transfer endpoint). Not reachable from the
    // demo (the fixture runs in a private slot; SHARED_ROW fixtures open no files) — defensive, like the
    // `FILE_WSTAGE == 0` EIO guard in `sys_write_file`.
    if rw && row == SHARED_ROW {
        return EACCES;
    }
    let wstage = if rw {
        // The staged content is the seed. staged_lookup already proved it Some, so this is Some too.
        let Some(seed) = staged_bytes(sidx) else {
            return ENOENT;
        };
        match wstage_alloc(seed) {
            Some(w) => (w + 1) as u32,
            None => return EMFILE, // the writable staging pool is full (too many concurrent RW opens)
        }
    } else {
        0
    };
    // U9x M2: the flush target — the file's on-disk chain head (SCRATCH.BIN's, captured by the launcher
    // pre-flight; `0` for HELLO.BIN or when no FAT backs it). A dirty write on a `0`-cluster descriptor is
    // never flushed (in-memory mode). Recorded for every open (harmless on RO / never-flushed descriptors).
    let cluster = staged_cluster(sidx);
    let Some(fid) = files_alloc(row, sidx, size, wstage, cluster) else {
        if let Some(w) = (wstage as usize).checked_sub(1) {
            wstage_free(w); // FILES row full — release the writable slot we just claimed (no leak)
        }
        return EMFILE; // this task's open-file row is full
    };
    // U11x: pack the slot's CURRENT generation into the file-id — `(gen << 32) | (idx + 1)`. The +1 low half
    // keeps the word clear of the 0 (Empty) / u64::MAX (RESERVING) sentinels; the gen high half lets
    // `file_desc_validate` reject a stale sibling handle after a free+reuse of this slot. `files_alloc` never
    // touches gen (it advances only on free), so this reads the gen this descriptor lives under.
    // U10: mark a RW open of a GROWABLE staged file with its `FILE_OPNAME` (+1-biased `U10_NAMES` index), so the
    // `sys_write_file` grow branch fires for it and any deferred op names THIS file. Only GROW.BIN in M1 (created
    // files set it on the O_CREAT path); SCRATCH.BIN (in-place-only) and every RO open keep opname 0 (not growable).
    if rw && sidx == GROW_STAGED_IDX {
        FILE_OPNAME[row][fid].store(1, Ordering::Release); // GROW.BIN == U10_NAMES[0] -> name-id 0 -> +1-biased 1
    }
    let file_id = file_id_pack(FILE_GEN[row][fid].load(Ordering::Acquire), fid);
    let Some(h) = handle_install(row, HANDLE_RESERVING) else {
        files_free(row, fid); // no handle slot — release the descriptor (and its writable slot); no leak
        return EAGAIN;
    };
    // U9x: RW mode (bit0) endows CAP_WRITE alongside CAP_READ, so a File `SYS_WRITE` through this handle passes
    // the CAP_WRITE CHECK; RO keeps the U6bx read-only cap. Publish the kind + rights, then the live file-id
    // LAST (Release) — a resolver that observes the live value also observes File + its rights. Single-writer
    // over this row (mid-syscall); belt-and-braces.
    let rights = if rw { CAP_READ | CAP_WRITE } else { CAP_READ };
    handle_set_kind(row, h, KIND_FILE);
    handle_set_rights(row, h, rights);
    handle_set(row, h, file_id);
    h as i64
}

/// STOR-1 S5a: read `want` bytes at byte `offset` of the CREATED/growable file behind descriptor `[row][idx]`
/// from the LIVE shared on-disk backing — resolve its name (`FILE_OPNAME` name-id -> `U10_NAMES`) and
/// `submit_read_file` into the KERNEL buffer `kbuf`. This is the created-file READ SOURCE `sys_read` routes to
/// knob-on (retiring the private wstage snapshot), and the exact function the S5 shared-backing witness exercises.
/// Returns the byte count read (`>= 0`) or `-EIO`. A descriptor with `FILE_OPNAME == 0` names no created file
/// (defensive — the `sys_read` caller already gates on `FILE_OPNAME != 0`), returning `-EIO` rather than indexing
/// `U10_NAMES` out of range. SAFETY: `kbuf` is a live kernel buffer of `>= want` bytes on the caller's stack; the
/// caller is a scheduled task (so `submit_read_file` can block on the service task).
#[cfg(feature = "irqstorage")]
unsafe fn created_read_live(row: usize, idx: usize, offset: u32, kbuf: *mut u8, want: usize) -> i32 {
    let Some(nameid) = (FILE_OPNAME[row][idx].load(Ordering::Acquire) as usize).checked_sub(1) else {
        return EIO as i32; // opname 0 -> not a created name; never index U10_NAMES
    };
    if nameid >= N_U10_NAMES {
        return EIO as i32;
    }
    let name = U10_NAMES[nameid];
    unsafe { crate::drivers::xhci::irqstorage::submit_read_file(name.as_bytes(), offset, kbuf, want) }
}

/// STOR-1 S8: the WRITE twin of `created_read_live` for a DYNAMIC on-disk descriptor — resolve its stored 8.3
/// name (`FILE_DYNLEN`/`FILE_DYNNAME` via `dyn_name_get`) and `submit_write_file` `[kbuf, kbuf+len)` at `offset`
/// on the LIVE volume. This is the created-file write SOURCE `sys_write_file` routes a RW dynamic descriptor to
/// knob-on, and the exact seam the S8 write witness drives with a kernel buffer. Returns the byte count written
/// (`>= 0`) or `-EIO`. An empty name (`FILE_DYNLEN == 0` — the caller already gated on it being non-zero) is a
/// kernel bug: fail closed `-EIO` rather than submitting an empty name. Strictly overwrite-only — the CLAMP that
/// keeps `offset + len <= FILE_SIZE` lives in the `sys_write_file` caller; `write_at` never grows by contract.
/// SAFETY: `kbuf` is a live kernel buffer of `>= len` bytes on the caller's stack; the caller is a scheduled
/// task (so `submit_write_file` can block on the service task).
#[cfg(feature = "irqstorage")]
unsafe fn dyn_write_live(row: usize, idx: usize, offset: u32, kbuf: *mut u8, len: usize) -> i32 {
    let mut namebuf = [0u8; MAX_NAME];
    let nl = dyn_name_get(row, idx, &mut namebuf);
    if nl == 0 {
        return EIO as i32; // FILE_DYNLEN said dynamic but the name is empty — a kernel bug; fail closed
    }
    unsafe { crate::drivers::xhci::irqstorage::submit_write_file(&namebuf[..nl], offset, kbuf, len) }
}

/// STOR-1 S9: the GROW twin of `dyn_write_live` for a DYNAMIC on-disk descriptor — resolve its stored 8.3 name
/// (`FILE_DYNLEN`/`FILE_DYNNAME` via `dyn_name_get`) and `submit_grow` `[kbuf, kbuf+len)` at `offset` on the LIVE
/// volume, EXTENDING the file (alloc + chain) when the write runs past EOF. This is the past-EOF write SOURCE
/// `sys_write_file` routes a RW dynamic descriptor to knob-on (via `dyn_write_grow`), and the exact seam the S9
/// grow witness drives with a kernel buffer. Returns the byte count written (`>= 0`) or `-EIO`. An empty name
/// (`FILE_DYNLEN == 0` — the caller already gated on it being non-zero) is a kernel bug: fail closed `-EIO`
/// rather than submitting an empty name. The PER-WRITE / PER-FILE growth CLAMPS live in the `dyn_write_grow`
/// caller; `write_grow` grows from the file's live on-disk size. SAFETY: `kbuf` is a live kernel buffer of `>=
/// len` bytes on the caller's stack; the caller is a scheduled task (so `submit_grow` can block on the service).
#[cfg(feature = "irqstorage")]
unsafe fn dyn_grow_live(row: usize, idx: usize, offset: u32, kbuf: *mut u8, len: usize) -> i32 {
    let mut namebuf = [0u8; MAX_NAME];
    let nl = dyn_name_get(row, idx, &mut namebuf);
    if nl == 0 {
        return EIO as i32; // FILE_DYNLEN said dynamic but the name is empty — a kernel bug; fail closed
    }
    unsafe { crate::drivers::xhci::irqstorage::submit_grow(&namebuf[..nl], offset, kbuf, len) }
}

/// SYS_READ(handle, buf, len) -> the byte count (`0` = EOF), or a negative errno. The object table's
/// first resource-read CHECK on a non-Console object: `handle_resolve(row, handle, CAP_READ)` must yield
/// a `File`. A missing right (`Denied`), a non-File kind (Console/Child/Socket), or no handle (Empty/oob)
/// ALL return `-EACCES` — the single enforcement point, the twin of `sys_write`'s Console+CAP_WRITE. Then
/// it clamps the request to the bytes left from the descriptor's offset, validates the WHOLE destination
/// up front (a bad buffer is `-EFAULT` with no copy and NO offset change), serves the bytes from the source
/// (U9x: a RW descriptor's per-descriptor writable staging buffer — so a read-back witnesses prior writes; a
/// RO descriptor's read-only staged source, the honest x86 divergence — see the section note), and advances
/// the offset by exactly the count delivered. The offset is set by `SYS_SEEK` (U9x) or advanced sequentially.
fn sys_read(handle: u64, buf: u64, len: u64) -> i64 {
    let row = caller_row();
    // The CHECK: File + CAP_READ, or -EACCES. Identical shape to sys_write's Console + CAP_WRITE resolve.
    let file_id = match handle_resolve(row, handle, CAP_READ) {
        Ok(HandleTarget::File(id)) => id,
        _ => return EACCES,
    };
    // U11x: decode + validate the file-id through the ONE seam — undo the +1 bias, bounds-check, re-check
    // presence (defense in depth) AND require the packed generation to match the slot's current gen (a stale
    // sibling to a reused slot is rejected, no rebind). `None` -> -EACCES.
    let Some(idx) = file_desc_validate(row, file_id) else {
        return EACCES;
    };
    let size = FILE_SIZE[row][idx].load(Ordering::Acquire);
    // U7x (folding the ledgered U6bx note): the offset advance is now a tx-exact CAS CLAIM, not a
    // load->store — two SHARED_ROW tasks racing one descriptor each claim a DISJOINT byte range (the
    // winner advances the offset before copying; the loser re-reads and claims the next range), so
    // concurrent reads are well-defined instead of double-delivering one range. Private slots keep their
    // single-writer discipline untouched (the CAS never retries there). The destination is validated
    // BEFORE the claim, so an -EFAULT still moves no offset and loses no bytes.
    let (offset, want) = loop {
        let offset = FILE_OFFSET[row][idx].load(Ordering::Acquire);
        // Bytes available from the current offset, clamped to the request. `offset` advances only by
        // claimed counts and never exceeds `size`, so the subtraction cannot underflow; 0 = clean EOF.
        let want = core::cmp::min(len as usize, size.saturating_sub(offset) as usize);
        if want == 0 {
            return 0; // EOF, or the caller requested nothing
        }
        // CFU-1: validate the WHOLE destination BEFORE any claim or copy — and it must be WRITABLE user
        // memory: inside the window AND past the read-only code page (page 0 is ring3-RX/RO; a kernel
        // store there would either fault under CR0.WP or corrupt W^X-protected code). This is now the
        // unified `user_range_ok(.., UserAccess::Write)` seam (the write-dest lower bound). A bad buffer
        // is -EFAULT with no copy and no offset move.
        if let Err(e) = user_range_ok(buf, want as u64, UserAccess::Write) {
            return e;
        }
        if FILE_OFFSET[row][idx]
            .compare_exchange(offset, offset + want as u32, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break (offset, want); // the range [offset, offset+want) is now exclusively ours
        }
    };
    // STOR-1 S7 (irqstorage): a DYNAMIC on-disk descriptor (`open_dynamic_ondisk` — a pre-existing file that
    // is neither staged nor a U10 name) reads the LIVE volume BY its stored NAME through the storage service
    // task. Keyed on `FILE_DYNLEN != 0` (set ONLY knob-on, so a knob-off build never enters this branch). A
    // dynamic descriptor owns no staged blob and no wstage, so it MUST resolve here — it never falls through
    // to the wstage/staged serve below (which would `-EIO`). Unlike the staged/created one-page bounce, an
    // arbitrary on-disk file may exceed a page, so fill `want` a page at a time (the whole `[buf, buf+want)`
    // dest was validated in the CAS claim above). STOR-1 S8: a dynamic file's CONTENT may now change (a RW
    // dynamic descriptor overwrites it live), but its SIZE is still immutable for the boot — no syscall
    // grows/allocates/unlinks a non-U10 name (S8 is strictly overwrite-only), so its on-disk size == the
    // descriptor's captured `FILE_SIZE`; the loop therefore delivers exactly `want` (each chunk full but the
    // last exact one), so the CAS offset advance-by-`want` matches bytes delivered (the S2/S3 N1 note holds:
    // no sequential-read gap).
    #[cfg(feature = "irqstorage")]
    if FILE_DYNLEN[row][idx].load(Ordering::Acquire) != 0 {
        // Only ever created knob-on with the service up; fail closed defensively if the service is somehow
        // not ready (never reached — open refuses the dynamic path unless `s4_sync_storage()`).
        if !crate::drivers::xhci::irqstorage::service_ready() {
            return EIO;
        }
        let mut namebuf = [0u8; MAX_NAME];
        let nl = dyn_name_get(row, idx, &mut namebuf);
        if nl == 0 {
            return EIO; // FILE_DYNLEN said dynamic but the name is empty — a kernel bug; fail closed
        }
        let name = &namebuf[..nl];
        // Bounce through a kernel-stack page buffer (as S2/S3/S5): the service task's kernel CR3 cannot reach
        // the ring-3 window. Fill `[buf, buf+want)` a page at a time from `offset` on the live volume.
        let mut kbuf = [0u8; PAGE_SIZE as usize];
        let mut done = 0usize;
        while done < want {
            let chunk = core::cmp::min(PAGE_SIZE as usize, want - done);
            let n = unsafe {
                crate::drivers::xhci::irqstorage::submit_read_file(
                    name, offset + done as u32, kbuf.as_mut_ptr(), chunk,
                )
            };
            if n < 0 {
                // Fail the WHOLE read closed on ANY live I/O error — the disk is the source of truth (matching
                // S2/S3 + service_read_file/created_read_live, which all EIO the whole read). NEVER report a
                // masked partial: `done` bytes may already be in `buf`, but the offset was CAS-advanced by the
                // full `want`, so returning `done` would silently skip `[done, want)` on the next sequential
                // read AND hide the error. EIO leaves the offset advanced (the ledgered N1 pattern, as S2/S3).
                return EIO;
            }
            let got = (n as usize).min(chunk);
            if got == 0 {
                break; // live EOF / short read — deliver what we have
            }
            // CFU-1: push this chunk out through the WRITE seam. `[buf+done, buf+done+got)` is a subrange
            // of the destination validated before the CAS claim above, so this re-check cannot fail here.
            if let Err(e) = copy_to_user(buf + done as u64, &kbuf[..got]) {
                return e;
            }
            done += got;
            if got < chunk {
                break; // short read — stop (the next SYS_READ resumes from the advanced offset)
            }
        }
        return done as i64;
    }
    // S2/S3 (irqstorage): a staged descriptor reads the LIVE volume through the storage service task,
    // retiring the in-memory staged source for reads. The gate is `FILE_OPNAME == 0` — i.e. an
    // IN-PLACE-only descriptor (a RO open, or a RW open of a non-growable staged file like SCRATCH.BIN,
    // whose writes S3 makes live too). A GROWABLE descriptor (GROW.BIN / a created file, FILE_OPNAME set)
    // keeps serving its wstage buffer until S4 makes its grow live (write-then-read-back coherence), and
    // `STAGED_NAMES.get` excludes created files (the CREATED sentinel). Gated on a mounted FAT volume
    // (HELLO_STAGED) + the service task being up; the no-FAT in-memory core and the pre-service window
    // both fall through to the staged serve below unchanged.
    #[cfg(feature = "irqstorage")]
    if FILE_OPNAME[row][idx].load(Ordering::Acquire) == 0
        && HELLO_STAGED.load(Ordering::Acquire)
        && crate::drivers::xhci::irqstorage::service_ready()
    {
        let sidx = FILE_STAGED[row][idx].load(Ordering::Acquire) as usize;
        if let Some(name) = STAGED_NAMES.get(sidx) {
            // Bounce through a kernel-stack buffer: the service task (kernel CR3) cannot reach the ring-3
            // window (PML4[2], the submitter's private CR3), but it can reach this stack buffer (shared
            // kernel half). `want <= size <= PAGE_SIZE` (the staged size bound), so one page suffices.
            let mut kbuf = [0u8; PAGE_SIZE as usize];
            let n = unsafe {
                crate::drivers::xhci::irqstorage::submit_read_file(
                    name.as_bytes(), offset, kbuf.as_mut_ptr(), want,
                )
            };
            if n < 0 {
                return EIO;
            }
            // LEDGERED (STOR-1 review note N1): the offset was CAS-advanced by `want` above, but a live
            // short read / EIO delivers `got <= want` — so a subsequent SEQUENTIAL read would skip the
            // undelivered tail. BENIGN today (the live on-disk size == the staged size the claim used, so
            // `got == want` always here); the real clamp (advance by bytes actually delivered) rides S4,
            // where a live grow makes the on-disk size diverge from the descriptor's captured size.
            let got = (n as usize).min(want);
            if got == 0 {
                return 0; // live EOF / short read
            }
            // CFU-1: copy the fetched bytes out through the WRITE seam — the submitter's CR3 is
            // re-installed on resume, so the user window is mapped again. `got <= want`, a subrange of the
            // destination validated before the CAS claim, so this re-check cannot fail here.
            if let Err(e) = copy_to_user(buf, &kbuf[..got]) {
                return e;
            }
            return got as i64;
        }
    }
    // STOR-1 S5a (irqstorage): a CREATED-file descriptor reads the LIVE SHARED on-disk backing through the
    // storage service task (`created_read_live` -> `submit_read_file` BY NAME), retiring its private wstage
    // snapshot as the read SOURCE. Gate: FILE_CREATED (so GROW.BIN — a growable STAGED file, FILE_CREATED ==
    // false — keeps its wstage serve, byte-identical) AND FILE_OPNAME != 0 (defensive: names a U10 file; also
    // keeps a recycled opname-0 created descriptor off `U10_NAMES`). A created file is on disk (S4a create + S4b
    // grow) and every created-file write writes THROUGH (S4), so the disk is the source of truth: a cross-process
    // sibling READS a peer's writes (shared backing) instead of a stale open-time snapshot. Same FAT + service
    // gate as S2/S3; no-FAT / pre-service / knob-off fall through to the wstage serve below unchanged.
    #[cfg(feature = "irqstorage")]
    if FILE_CREATED[row][idx].load(Ordering::Acquire)
        && FILE_OPNAME[row][idx].load(Ordering::Acquire) != 0
        && HELLO_STAGED.load(Ordering::Acquire)
        && crate::drivers::xhci::irqstorage::service_ready()
    {
        // Bounce through a kernel-stack buffer (as S2/S3): the service task's kernel CR3 cannot reach the ring-3
        // window. `want <= FILE_SIZE <= PAGE_SIZE` (created files are one-page-bounded by the grow clamp), so one
        // page suffices.
        let mut kbuf = [0u8; PAGE_SIZE as usize];
        let n = unsafe { created_read_live(row, idx, offset, kbuf.as_mut_ptr(), want) };
        if n < 0 {
            return EIO; // a live created-file read failure -> fail closed (the disk is the source of truth)
        }
        // As S2/S3 (N1): the offset was CAS-advanced by `want`; a live short read delivers `got <= want`. BENIGN
        // for created files — the descriptor's FILE_SIZE <= the on-disk size ALWAYS (grow persists to disk BEFORE
        // bumping FILE_SIZE; a sibling captures size <= the source's already-synced size), so `got == want` here.
        let got = (n as usize).min(want);
        if got == 0 {
            return 0; // live EOF / short read
        }
        // CFU-1: copy the created-file bytes out through the WRITE seam (`got <= want`, a subrange of the
        // destination validated before the CAS claim — cannot fail here).
        if let Err(e) = copy_to_user(buf, &kbuf[..got]) {
            return e;
        }
        return got as i64;
    }
    // U9x: serve from the descriptor's WRITABLE staging buffer if it has one (a RW open — so a read-back
    // witnesses prior writes through the same cap), else the read-only staged source (a RO open — written
    // once, stable across the whole boot). Both are stable-length: the writable buffer's length is fixed at
    // open (writes are in-place, never grow), the staged source never shrinks.
    let src: &[u8] = match (FILE_WSTAGE[row][idx].load(Ordering::Acquire) as usize).checked_sub(1) {
        Some(widx) => wstage_bytes(widx),
        None => match staged_bytes(FILE_STAGED[row][idx].load(Ordering::Acquire)) {
            Some(s) => s,
            None => return EIO, // a live RO descriptor over an unstaged source is a kernel bug; fail closed
        },
    };
    // offset..offset+want lies inside `src`: `size` was captured from this same source at open, the source
    // never shrinks, and offset only advances by claimed counts. The defensive clamp keeps even a violated
    // assumption from over-reading the source (the claim above already advanced the offset by `want`; a short
    // `got` here is an impossible-source-shrink fail-safe, not a real path).
    let got = core::cmp::min(want, src.len().saturating_sub(offset as usize));
    if got == 0 {
        return 0; // treat a (impossible) source shrink as EOF rather than over-read
    }
    // CFU-1: the dest range was validated before the CAS claim above; copy the staged/wstage bytes out
    // through the WRITE seam (`got <= want`, a subrange of that validated range — cannot fail here).
    if let Err(e) = copy_to_user(buf, &src[offset as usize..offset as usize + got]) {
        return e;
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
        // SOCK-4: a received Socket cap MOVES the persistent socket's registry ownership to THIS row, so
        // the moved cap actually resolves (`sock_valid` is owner-scoped). Done after the handle install
        // committed (a re-queue on a full table above never migrates), before the live value publish. A
        // no-op for non-Socket kinds; a stale deposit (socket freed+reused since XFER) fails the gen
        // check inside and the received handle stays dead — no cross-tenant rebind. The migration also
        // demands the transfer's SENDER (from the record) still OWNS the socket: a sender whose socket
        // already moved to an earlier grantee cannot use a second deposit to steal it back (its residual
        // CAP_GRANT handle is delegation-dead once ownership left it). The whole line is knob-gated, so
        // knob-off / aarch64 (where no socket is ever transferable) is byte-identical.
        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
        xfer_socket_migrate(
            kind,
            target,
            XFER_REC_SENDER[(rec - 1) as usize].load(Ordering::Acquire),
            row,
        );
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
    // WINX-7: revoke input focus if this dying slot held it, and reset its input ring — BEFORE the
    // rest of the teardown, and for the same reason the generation bump comes first: from this point
    // on nothing addressed to the old tenant may reach the slot's next one. Without the revoke, focus
    // would keep pointing at a dead slot and every later keystroke would be enqueued into a ring with
    // no consumer, which presents to the operator as the keyboard silently ceasing to work. The
    // generation bump above also retires this slot's thread-table rows for the lazy scavenge in
    // `sys_thread_spawn` — a killed threaded program leaks no rows permanently.
    el0_input_revoke_slot(slot);
    // WINX-7: the detached mark is per-TENANT, not per-slot, so it must die with the tenant — a
    // foreground `run` landing in a slot a `bg` job just vacated must not inherit "I was detached"
    // and skip its own frame cap.
    SLOT_DETACHED[slot].store(false, Ordering::Release);
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
    // U6x: revert every file this slot OWNS to PUBLIC and sweep any grant naming it (a grantee that exited) — an
    // owner's authority is boot- and lifetime-scoped (no persistent principal), so its files revert when it tears
    // down. SLOT_GEN was bumped at the top, so the gen fence already makes any unswept stale entry harmless; this
    // keeps the bounded table self-cleaning and enforces the owner-exit-reverts-to-public rule. Before
    // `clear_files_row` (the open-descriptor decrefs) — order-independent (a separate lock), placed here for
    // adjacency with the file teardown (the aarch64 `owned_clear_owner_asid` twin).
    owned_clear_owner_slot(slot);
    // U6bx: the slot's open-FILE row rides the same teardown (handles first, so no File handle can name a
    // descriptor this wipe has already freed) — covers both the exit and the fault-kill path, exactly like
    // the handles (the aarch64 `clear_handle_row` -> `clear_files_row` twin).
    clear_files_row(slot);
    // SOCK-2: free every persistent UDP socket this dying slot OWNED (the handle wipe above dropped its
    // Socket handles; this reclaims the smoltcp socket + its static buffers), so a reused slot inherits no
    // live socket. Knob-off / aarch64 have no persistent stack — the call vanishes.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    crate::smolnet::free_row_sockets(slot);
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
        // U11x: the value word is now a gen-tagged file-id `(gen << 32) | (idx + 1)`, so decode it through
        // `file_desc_validate` (which masks the low half, bounds-checks, and matches the generation) rather than a
        // bare `checked_sub(1)` — a bare subtract would leave the gen bits in the index. A stale/out-of-range id
        // (a kernel bug) validates to `None` and is simply skipped; the handle clear below still denies every use.
        if let Some(file_id) = handle_get(row, idx as usize).filter(|&v| v != HANDLE_RESERVING) {
            if let Some(fid) = file_desc_validate(row, file_id) {
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

// --- U9x demo accounting (real File writes + seek; the single-process fixture's witness + the kernel-side
// revoked-cap-write check, read by the launcher's verdict). The aarch64 U9 twin. ---
/// The handle index `u9x_launcher` pre-endows: a `Socket` carrying `CAP_WRITE` — the kind negative (a
/// non-File object WITH the write right is still `-EACCES`, denied purely on kind, since `sys_write` serves
/// Console/File only). Off index 0 (the fixture's own RW open first-free-claims it) and off `CONSOLE_FD`. The
/// `mov rdi, 2` operand in `unaos_user_u9x_write` MUST match.
const U9X_SOCK_IDX: usize = 2;
/// The full witness bitmask the File-WRITE fixture reports (as its exit status): bit0 open-RW OK
/// (`SYS_OPEN("SCRATCH.BIN", RW)` -> handle >= 0), bit1 seek+write OK (`SYS_SEEK` to the scratch offset then
/// `SYS_WRITE` -> the pattern length), bit2 read-back matches (seek back, `SYS_READ` -> the just-written
/// bytes, proving the in-place overwrite landed and is visible through the SAME cap), bit3 an RO-opened File
/// write -> `-EACCES` (the CAP_WRITE rights CHECK), bit4 a non-File handle (a `Socket` carrying `CAP_WRITE`)
/// write -> `-EACCES` (the kind CHECK). `u9x_launcher` PASSes iff it equals `U9X_WITNESS_ALL` AND the
/// kernel-side revoked-cap-write denial held. Must match the `add r12, {1,2,4,8,16}` steps in
/// `unaos_user_u9x_write`.
const U9X_WITNESS_ALL: u32 = 0x1F;
/// U9x M2: the byte offset the fixture seeks to and overwrites (the `mov rsi, 520` in `unaos_user_u9x_write`
/// MUST match). 520 lands 8 bytes into the SECOND 512-byte sector — a PARTIAL-sector overwrite, so the flush's
/// read-modify-write must preserve the sector's other bytes (the interesting on-disk case M2 proves). The
/// aarch64 twin's `U9_WRITE_OFFSET`.
const U9X_WRITE_OFFSET: u32 = 520;
/// U9x M2: the 16-byte pattern the fixture writes (the `.ascii "U9x-WRITE-OK-123"` in the blob MUST match).
/// The launcher's raw re-read of the flushed sector must find exactly these bytes at `U9X_WRITE_OFFSET`. The
/// aarch64 twin's `U9_PATTERN`.
const U9X_PATTERN: [u8; 16] = *b"U9x-WRITE-OK-123";
/// The U9x fixture's final witness bitmask (its `sys_exit` status, routed by name — the u5x/u6bx/u8x idiom).
static U9X_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The U9x fixture (`u9x-write`) reached its witness exit (want 1). Read by `u9x_launcher`'s bounded wait.
static U9X_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U9x fixture — a real bug (it is register-only apart from its read-back dest store). Off the U1b
/// counter (a kill here fails only the U9x verdict, never a phantom U1b regression).
static U9X_KILLED: AtomicU32 = AtomicU32::new(0);

/// The U11x open-file-lifecycle fixture (`u11x-close`) exercises SYS_CLOSE end-to-end over the immutable staged
/// SCRATCH.BIN: (bit0) open RO + read the 16-byte seed OK; (bit1) SYS_CLOSE -> `0`; (bit2) double-close ->
/// `-EBADF`; (bit3) a read through the closed handle -> `-EACCES` (use-after-close denied); (bit4) reopen (a fresh
/// handle reusing the freed slot) + read the seed again (round-trip). 5 bits — see `U11X_WITNESS_ALL`. The x86
/// gen-rebind gap is not ring-3-reachable (no way to hold a stale file-id across a free), so this fixture proves
/// SYS_CLOSE semantics while `u11x_check_gen_rebind` (kernel-side) is the airtight no-rebind proof.
const U11X_WITNESS_ALL: u32 = 0x1F;
/// The U11x fixture's final witness bitmask (its `sys_exit` status, routed by name — the u5x/u6bx/u8x/u9x idiom).
static U11X_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The U11x fixture (`u11x-close`) reached its witness exit (want 1). Read by `u11x_launcher`'s bounded wait.
static U11X_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U11x fixture — a real bug (register-only apart from its read-back dest store). Off the U1b counter (a
/// kill here fails only the U11x verdict, never a phantom U1b regression).
static U11X_KILLED: AtomicU32 = AtomicU32::new(0);

/// U10 GROW: the full witness bitmask the growth fixture (`u10x-grow`) reports as its exit status: bit0 open-RW
/// OK, bit1 seek-to-EOF + write-past-EOF -> 16 (a real grow, not a U9x clamp-to-0), bit2 seek-back + read -> the
/// appended pattern (through the SAME cap), bit3 read at offset 0 -> the original `0xC1` filler (the grow didn't
/// corrupt the pre-existing cluster), bit4 an RO-opened File write -> `-EACCES` (growth rides the SAME single
/// CAP_WRITE CHECK). `u10x_launcher` PASSes iff it equals `U10X_WITNESS_ALL` AND the on-disk grow proof held.
/// Must match the `add r12, {1,2,4,8,16}` steps in `unaos_user_u10x_grow`. The aarch64 twin's `U10_WITNESS_ALL`.
const U10X_WITNESS_ALL: u32 = 0x1F;
/// The U10 GROW fixture's final witness bitmask (its `sys_exit` status, routed by name — the u5x/u9x idiom).
static U10X_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The U10 GROW fixture (`u10x-grow`) reached its witness exit (want 1). Read by `u10x_launcher`'s bounded wait.
static U10X_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U10 GROW fixture — a real bug (register-only apart from its read-back dest store). Off the U1b counter.
static U10X_KILLED: AtomicU32 = AtomicU32::new(0);

/// U10 CREATE: the witness bitmask the create fixture (`u10cx-create`) reports: bit0 open O_CREAT|RW OK (the file
/// is created), bit1 write at offset 0 -> 16 (grow-from-empty allocates the first cluster), bit2 seek-0 + read ->
/// the pattern (through the SAME cap), bit3 a SECOND O_CREAT|RW open of the same name -> a handle (idempotent
/// create-if-present). `u10cx_launcher` PASSes iff it equals `U10CX_WITNESS_ALL` AND the on-disk create proof
/// held. Must match `add r12, {1,2,4,8}` in `unaos_user_u10cx_create`. The aarch64 twin's `U10C_WITNESS_ALL`.
const U10CX_WITNESS_ALL: u32 = 0xF;
/// The U10 CREATE fixture's final witness bitmask (its `sys_exit` status, routed by name — the u5x/u9x idiom).
static U10CX_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The U10 CREATE fixture (`u10cx-create`) reached its witness exit (want 1). Read by `u10cx_launcher`'s wait.
static U10CX_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U10 CREATE fixture — a real bug (register-only apart from its read-back dest store). Off the U1b counter.
static U10CX_KILLED: AtomicU32 = AtomicU32::new(0);

/// U10 DELETE: the witness bitmask the delete fixture (`u10dx-delete`) reports: bit0 create+open OK; bit1 write ->
/// 16 (grow-from-empty allocates the file's one data cluster); bit2 SYS_UNLINK -> 0 (name gone + all this proc's
/// descriptors invalidated + the on-disk delete enqueued); bit3 a read through a SIBLING handle -> `-EACCES` (the
/// sibling was invalidated — no stale reference; the U11x gen-tag); bit4 a plain RO re-open -> `-ENOENT` (the file
/// is gone). `u10dx_launcher` PASSes iff it equals `U10DX_WITNESS_ALL` AND the on-disk delete proof held. Must
/// match `add r12, {1,2,4,8,16}` in `unaos_user_u10dx_delete`. The aarch64 twin's `U10D_WITNESS_ALL`.
const U10DX_WITNESS_ALL: u32 = 0x1F;
/// The U10 DELETE fixture's final witness bitmask (its `sys_exit` status, routed by name — the u5x/u9x idiom).
static U10DX_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The U10 DELETE fixture (`u10dx-delete`) reached its witness exit (want 1). Read by `u10dx_launcher`'s wait.
static U10DX_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U10 DELETE fixture — a real bug (register-only apart from its read-back dest store). Off the U1b counter.
static U10DX_KILLED: AtomicU32 = AtomicU32::new(0);

/// U11x M2: the witness bitmask the cross-process unlink fixture (`u11m2-unlink`) reports: bit0 a PLAIN RW open
/// of ANOTHER process's created file -> a handle (the cross-row sibling open); bit1 read -> the launcher's
/// pattern (content crossed processes); bit2 SYS_UNLINK -> 0 (the deferred cross-process unlink); bit3 a read
/// through the invalidated sibling -> `-EACCES`; bit4 a plain re-open -> `-ENOENT` (gone globally, even while
/// the launcher still holds it open); bit5 an O_CREAT re-create -> `-EBUSY` (refused until the deferred delete
/// completes). Must match `add r12, {1,2,4,8,16,32}` in `unaos_user_u11m2_unlink`. The pi4 `el0-u11defer-b`
/// witness's x86 shape.
const U11M2_WITNESS_ALL: u32 = 0x3F;
/// The U11x M2 fixture's final witness bitmask (its `sys_exit` status, routed by name — the u5x/u9x idiom).
static U11M2_WITNESS: AtomicU32 = AtomicU32::new(0);
/// The U11x M2 fixture reached its witness exit — a COUNT (the fixture runs once per phase; the launcher gates
/// phase N's read on `>= N`).
static U11M2_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U11x M2 fixture — a real bug (register-only apart from its read-back dest store). Off the U1b counter.
static U11M2_KILLED: AtomicU32 = AtomicU32::new(0);

/// U6x: the OWNER fixture's full witness — bit0 create PRIVATE + write; bit1 grant B R|W -> 0; bit2 revoke -> 0;
/// bit3 owner re-open after revoke; bit4 owner unlink while B holds open -> 0 (deferred); bit5 O_CREAT re-create
/// -> -EBUSY. Must match `add r12, {1,2,4,8,16,32}` in `unaos_user_u6gx_owner`.
const U6GX_OWNER_ALL: u32 = 0x3F;
/// U6x: the GRANTEE fixture's full witness — bit0 pre-grant open -> -EACCES; bit1 granted RW open + read-back
/// matches; bit2 non-owner SYS_FGRANT -> -EACCES; bit3 grantee SYS_UNLINK -> -EACCES (F1); bit4 post-revoke open
/// -> -EACCES. Must match `add r12, {1,2,4,8,16}` in `unaos_user_u6gx_grantee`.
const U6GX_GRANTEE_ALL: u32 = 0x1F;
/// U6x: the OWNER / GRANTEE fixtures' final witness bitmasks (their `sys_exit` status, routed by name — the
/// u5x/u7x idiom; x86 has no SYS_REPORT).
static U6GX_OWNER_WITNESS: AtomicU32 = AtomicU32::new(0);
static U6GX_GRANTEE_WITNESS: AtomicU32 = AtomicU32::new(0);
/// U6x: a COUNT of the two fixtures' witness exits (the launcher gates its read on `>= 2`).
static U6GX_DONE: AtomicU32 = AtomicU32::new(0);
/// U6x: a killed owner/grantee fixture — a real bug (register-only apart from the grantee's read-back store).
static U6GX_KILLED: AtomicU32 = AtomicU32::new(0);
/// U6x: the pre-endowed handle index in A's table where the launcher plants a `Child` handle naming B — so A's
/// SYS_FGRANT is owner-scoped (A never names B by a raw pid). Distinct from `CONSOLE_FD` (1) and A's dynamic
/// File handles (0, then 3).
const U6GX_CHILD_IDX: usize = 2;
/// U6x: the per-slot GO word (launcher -> fixture: the next step it may proceed to) and SIG word (fixture ->
/// launcher: the last step it completed), planted at fixed offsets in each fixture's OWN window (a writable data
/// page, beyond the RX-RO code page). Read/written by the launcher through `slot_backing_ptr`.
const U6GX_GO_OFF: usize = 0x3800;
const U6GX_SIG_OFF: usize = 0x3808;

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

    // 2. U5x runs REGARDLESS of a block device (its fixture is an inline console-cap blob — no disk), so the
    //    capability chain is visible on the no-storage / metal path. `U5X_LAUNCH_DONE` is set at the end of the
    //    run path below (and on the no-free-slot skip), so the U6x gate is released either way.

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
    // U5x is an INLINE console-cap demo — it needs NO block device (its fixture is an inline blob; sys_write
    // targets the serial console, never disk). It runs REGARDLESS of storage so the capability chain is VISIBLE
    // on the no-storage / metal path (replayed over the FTDI console), not only when a block device enumerates.
    // The storage-GATED arcs (U2/U4x/U6x/U6bx/U9x/U11x) keep their gates. (Scoped relaxation — U5x/U7x/U8x only.)
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
    if let Some(fid) = files_alloc(u6b.slot, 0, staged_sz, 0, 0) {
        // cluster 0: HELLO.BIN is read-only (no writable staging, never dirtied) — no flush target (U9x M2).
        // U11x: pack the descriptor's generation into the file-id like a real `sys_open` (consistency — this cap
        // carries rights 0 so a read is denied on rights before `file_desc_validate`, but the value word stays a
        // valid gen-tagged file-id).
        let nocap_id = file_id_pack(FILE_GEN[u6b.slot][fid].load(Ordering::Acquire), fid);
        install_cap(u6b.slot, U6BX_NOCAP_IDX, KIND_FILE, nocap_id, 0);
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

    // 2. U7x runs REGARDLESS of a block device (both fixtures are inline console-cap blobs — no disk), so the
    //    cross-process-transfer rung is visible on the no-storage / metal path.
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

// =============================================================================================
// WINX-2 — the EL0 PROGRAM LIFECYCLE: `run` (synchronous) and `bg` (detached) on x86.
//
// The x86 twins of aarch64's EXEC-1 `run_user_image` and BGRUN-1 `spawn_user_image_bg` / `bg_poll` /
// `bg_kill`, with the same operator-visible contracts. `shell.rs` calls these through
// `crate::arch::syscall::*`, so the shell body is arch-neutral and the two arches diverge only here.
//
// PREEMPTIBLE, ALWAYS — the one substantive divergence from aarch64, and it is forced.
// aarch64 spawns a foreground `run` co-located and COOPERATIVE, and stops a runaway with its SKILL-1
// `sched::kill` primitive (a per-ASID kill ticket the SVC path polls). x86 has no such primitive; what it
// has is U3.5's `spawn_user_preemptible` + `KillSwitch`, where the SCHEDULER reaps the task at its next
// preemption boundary. So both `run` and `bg` spawn preemptible here.
//
// That is not a downgrade, it is the only correct choice: `STAT.ELF` has NO exit path by design ("runs
// until it is killed" — BGRUN-2's whole contract), so a cooperative x86 `run /fat/STAT.ELF` would wedge
// the shell task forever with no way back. Preemptible ring 3 (RFLAGS.IF set) is the proven U3.5 path,
// and the scheduler's reap tears the address space down through `free_user_space_by_cr3` — which, since
// WINX-1, also retires the task's compositor windows and drops its FB leaves. A killed windowed app
// therefore leaves nothing on the panel and nothing in the window table.
//
// EXIT ACCOUNTING rides the GENERIC Proc short-circuit already in the `SYS_EXIT` arm: we plant a Proc row
// and store the spawned pid, and the exiting task's own `proc_find_running` lookup records its status,
// marks `PEXITED`, and posts `done`. No dedicated exit arm is needed (and none would work — these
// programs run under an operator-chosen image, not a fixed fixture name). The KILL path has no such
// short-circuit, so `record_ring3_kill` gets an arm for these two task names below.

/// WINX-2: the Proc `status` a killed run/bg task is marked with, so `bg_poll` / `run_user_image` can
/// tell a fault-kill from an ordinary nonzero exit. Same sentinel value as the aarch64 twin.
const EXEC_KILLED_STATUS: i32 = i32::MIN;

/// WINX-2: how long `bg_kill` / the `run` deadline arm waits for the scheduler to confirm a reap before
/// reporting the kill as still-armed. The reap lands at the target's next preemption boundary, which is
/// one timer tick away; seconds of budget is generous by orders of magnitude.
const KILL_CONFIRM_MS: u64 = 2000;

/// WINX-2: the task name a foreground `run` program is spawned under.
const RUN_TASK_NAME: &str = "shell-run";
/// WINX-2: the task name a background `bg` program is spawned under.
const BG_TASK_NAME: &str = "bg-el0";

/// WINX-2: the outcome of [`run_user_image`] — the program exited with a status, was killed by the
/// fault-kill net (a CONTAINED fault), or overran its deadline (still running / stuck, and then killed).
pub enum RunOutcome {
    Exited(i32),
    Faulted,
    Timeout,
}

/// WINX-2: what [`bg_poll`] found for a pid.
pub enum BgPoll {
    /// The task is still running.
    Running,
    /// Exited with this status; if `reap` was set the row has been freed.
    Exited(i32),
    /// Killed by the fault-kill net (contained fault); if `reap` was set the row has been freed.
    Faulted,
    /// No row holds this pid (already reaped, or never existed).
    Gone,
}

/// WINX-2: one live background job's kill handle. The `Arc<KillSwitch>` clone is what keeps the switch
/// alive across the request/reap window after the scheduler has dropped the `Task` (the switch is shared
/// by `Arc` exactly so this is safe — see `sched::KillSwitch`).
struct BgKill {
    pid: u64,
    kill: alloc::sync::Arc<crate::arch::sched::KillSwitch>,
}

/// WINX-2: the background kill registry. Bounded by `MAX_PROCS` — a bg job always owns a Proc row, so a
/// row is the binding resource and this can never need to be larger. Guarded by a plain `SpinMutex`
/// (never taken from an IRQ-masked path: only the shell task and the `run` deadline arm touch it).
static BG_KILLS: SpinMutex<[Option<BgKill>; MAX_PROCS]> =
    SpinMutex::new([const { None }; MAX_PROCS]);

/// WINX-2: record a live job's kill handle. Drops the oldest completed entry if the table is full, which
/// cannot lose a live job (the table is sized to `MAX_PROCS` and every live job holds a Proc row).
fn bg_kill_register(pid: u64, kill: alloc::sync::Arc<crate::arch::sched::KillSwitch>) {
    let mut t = BG_KILLS.lock();
    if let Some(s) = t.iter_mut().find(|s| s.is_none()) {
        *s = Some(BgKill { pid, kill });
        return;
    }
    // Full: evict a reaped entry (its job is gone, so its switch is dead weight).
    if let Some(s) = t.iter_mut().find(|s| s.as_ref().is_some_and(|b| b.kill.is_reaped())) {
        *s = Some(BgKill { pid, kill });
    }
}

/// WINX-2: take a job's kill handle out of the registry (the kill consumes it — a second `kill` on the
/// same pid finds nothing and is told so, rather than re-arming a switch whose task is already gone).
fn bg_kill_take(pid: u64) -> Option<alloc::sync::Arc<crate::arch::sched::KillSwitch>> {
    let mut t = BG_KILLS.lock();
    let s = t.iter_mut().find(|s| s.as_ref().is_some_and(|b| b.pid == pid))?;
    s.take().map(|b| b.kill)
}

/// WINX-2: drop a pid's registry entry without arming it (the reap path, after an ordinary exit).
fn bg_kill_forget(pid: u64) {
    let mut t = BG_KILLS.lock();
    if let Some(s) = t.iter_mut().find(|s| s.as_ref().is_some_and(|b| b.pid == pid)) {
        *s = None;
    }
}

/// WINX-2: the shared front half of `run` and `bg` — bound the image, claim a Proc row, map the image
/// into a fresh slot, endow the console capability, and publish the slot. Returns the mapped image and
/// the Proc row index. On any failure the Proc row is released and NO slot is left allocated (the mapper
/// allocates its slot last, after all validation).
///
/// PUBLISH ORDER (the aarch64 EXEC1-M rule, and it matters for the same reason): the SLOT is published
/// into the Proc row BEFORE the task is spawned, because a preemptible child can be dispatched on
/// another core the instant it is enqueued — before the `pid` store below lands. Anything keyed off the
/// row must therefore be valid at spawn time, not at store time.
fn load_program_common(bytes: &[u8]) -> Result<(super::elf::Mapped, usize), &'static str> {
    if bytes.len() > user_window_size() {
        return Err("image larger than the 16 KiB user window");
    }
    let Some(pi) = proc_reserve() else {
        return Err("process table full (run `jobs` to reap exited jobs)");
    };
    let mapped = match super::elf::map_image_into_slot(bytes) {
        Ok(m) => m,
        Err(e) => {
            proc_free(pi);
            return Err(e.as_str());
        }
    };
    // Endow the console write capability, exactly as the fixtures' slots are endowed: a program's first
    // `SYS_WRITE(1, ...)` must resolve handle 1 to the Console cap or it could not say anything.
    install_console_cap(mapped.slot);
    PROCS[pi].slot.store(mapped.slot + 1, Ordering::Release); // +1-biased; 0 means "none"
    Ok((mapped, pi))
}

/// WINX-2: load an already-read program IMAGE (flat or ELF64) into a fresh ring-3 slot, run it, and
/// return its exit status — the synchronous shell `run <path>` entry. The shell reads the bytes off the
/// VFS at ring 0 and hands them here.
///
/// `deadline_ms` bounds the wait in milliseconds (the arch-neutral unit the shell now passes; aarch64's
/// twin takes a CNTPCT span and converts on its own side). On timeout the program is KILLED rather than
/// abandoned: an orphan that kept rendering would keep composing frames onto the panel forever and would
/// hold its address-space slot and window rows against every later launch. That is the one place this
/// diverges in EFFECT from the aarch64 twin, which leaves a documented orphan residue behind.
///
/// Returns `(outcome, entry)` where `entry` is the ring-3 entry VA the image was mapped at (for the
/// caller's witness line), or an operator string if the image could not be loaded.
pub fn run_user_image(
    name: &'static str,
    bytes: &[u8],
    deadline_ms: u64,
) -> Result<(RunOutcome, u64), &'static str> {
    let _ = name; // the task name is fixed (`RUN_TASK_NAME`) so the kill arm can match it
    let (mapped, pi) = load_program_common(bytes)?;
    let kill = alloc::sync::Arc::new(crate::arch::sched::KillSwitch::new());
    let cpu = crate::arch::sched::meter_current_cpu();
    let pid = crate::arch::sched::spawn_user_preemptible(
        RUN_TASK_NAME,
        mapped.entry,
        mapped.sp,
        cpu,
        mapped.cr3,
        kill.clone(),
    );
    PROCS[pi].pid.store(pid, Ordering::Release);

    // Deadline-bounded wait. Yielding (not sleeping) so this works before the timebase is calibrated and
    // so the shell task stays responsive to its own core's scheduler.
    let deadline = crate::arch::ticks() + deadline_ms;
    while PROCS[pi].state.load(Ordering::Acquire) == PRUNNING && crate::arch::ticks() < deadline {
        crate::arch::sched::yield_now();
    }

    if PROCS[pi].state.load(Ordering::Acquire) == PEXITED {
        let status = PROCS[pi].status.load(Ordering::Acquire);
        let _ = PROCS[pi].done.wait(); // already posted by the exiting task; balances the permit
        proc_free(pi);
        let outcome = if status == EXEC_KILLED_STATUS {
            RunOutcome::Faulted
        } else {
            RunOutcome::Exited(status)
        };
        return Ok((outcome, mapped.entry));
    }

    // TIMEOUT: arm the kill and wait (bounded) for the scheduler to confirm the reap.
    kill.request();
    let kdeadline = crate::arch::ticks() + KILL_CONFIRM_MS;
    while !kill.is_reaped() && crate::arch::ticks() < kdeadline {
        crate::arch::sched::yield_now();
        // A target that reached SYS_EXIT between the deadline test and the arm settles the row itself.
        if PROCS[pi].state.load(Ordering::Acquire) == PEXITED {
            break;
        }
    }
    if PROCS[pi].state.load(Ordering::Acquire) == PEXITED {
        // It exited on its own in the race window — reap it normally rather than reporting a timeout on
        // a program that actually finished.
        let status = PROCS[pi].status.load(Ordering::Acquire);
        let _ = PROCS[pi].done.wait();
        proc_free(pi);
        return Ok((RunOutcome::Exited(status), mapped.entry));
    }
    // Reaped by the scheduler (or the kill is still armed — either way the row is ours to release, and
    // the scheduler's reap has already torn the address space, windows and FB leaves down).
    proc_free(pi);
    Ok((RunOutcome::Timeout, mapped.entry))
}

/// WINX-2: load an EL0 image and spawn it WITHOUT waiting — the shell's `bg <path>` entry. Returns
/// `(pid, slot, entry)`; the caller records the pid and reaps it later via [`bg_poll`], or stops it with
/// [`bg_kill`]. Mirrors `run_user_image`'s front half exactly and diverges only in not waiting.
///
/// The Proc row stays claimed after exit (PEXITED, `done` posted) until `bg_poll(reap = true)` consumes
/// it — the `jobs` verb is the reaper. Rows are a bounded resource (`MAX_PROCS`), which is honest: a
/// shell that never runs `jobs` eventually gets "process table full", not silent loss.
pub fn spawn_user_image_bg(bytes: &[u8]) -> Result<(u64, u64, u64), &'static str> {
    let (mapped, pi) = load_program_common(bytes)?;
    // WINX-7: mark the slot DETACHED before the task can run, so its first `SYS_WIN_CREATE` publishes
    // the flag and the program never observes a stale `false`. See `SLOT_DETACHED` for why a windowed
    // app needs to know it was backgrounded.
    SLOT_DETACHED[mapped.slot].store(true, Ordering::Release);
    let kill = alloc::sync::Arc::new(crate::arch::sched::KillSwitch::new());
    // Place a bg job on a core chosen by the same round-robin the fixtures use rather than the shell's
    // own: every `bg` launch runs from the same shell context, so pinning to `this_cpu` would stack every
    // background program on one core (the aarch64 BG-SPREAD lesson). x86 has no CPU_AUTO, so this is the
    // simple honest version — spread across the online cores by job count.
    let cpu = bg_place_cpu();
    let pid = crate::arch::sched::spawn_user_preemptible(
        BG_TASK_NAME,
        mapped.entry,
        mapped.sp,
        cpu,
        mapped.cr3,
        kill.clone(),
    );
    PROCS[pi].pid.store(pid, Ordering::Release);
    bg_kill_register(pid, kill);
    Ok((pid, mapped.slot as u64, mapped.entry))
}

/// WINX-2: choose the core a background job starts on — the CALLER's core.
///
/// This started as a round-robin over `0..meter_cpu_count()`, mirroring aarch64's BG-SPREAD intent
/// (every `bg` runs from the same shell context, so pinning to one core piles every background program
/// onto it). That was WRONG and the WINX-3 witness caught it: `meter_cpu_count()` reports how many cores
/// the METER knows about, which is not the same as how many are released into `run()` and actually
/// dispatching. A job placed on a core that is online but not scheduling sits in that core's run queue
/// forever — spawned, never dispatched, never exiting, and invisible except as a job that never starts.
///
/// The caller's core is definitionally scheduling (we are running on it), so placement here is
/// always-correct if not always-optimal. Spreading properly needs an "is this core dispatching"
/// predicate that x86's scheduler does not expose yet (aarch64 gets this from `CPU_AUTO`, which picks by
/// ready-queue depth among cores it knows are live). That predicate is the honest prerequisite for
/// re-introducing spread, and it is a scheduler change, not a syscall-layer one.
fn bg_place_cpu() -> usize {
    crate::arch::sched::meter_current_cpu()
}

/// WINX-2: poll a background pid. With `reap` set, a `PEXITED` row is consumed here — the posted `done`
/// permit is awaited (it is already posted, so this does not block) and the row freed, exactly the reap
/// in `run_user_image`; the "reused entry starts at 0 permits" invariant holds. Scans by pid rather than
/// trusting a cached index: rows recycle, and a stale index could name a row that now belongs to someone
/// else — the pid is the key every other lookup uses.
pub fn bg_poll(pid: u64, reap: bool) -> BgPoll {
    for pi in 0..MAX_PROCS {
        if PROCS[pi].state.load(Ordering::Acquire) == PFREE {
            continue;
        }
        if PROCS[pi].pid.load(Ordering::Acquire) != pid {
            continue;
        }
        return match PROCS[pi].state.load(Ordering::Acquire) {
            PEXITED => {
                let status = PROCS[pi].status.load(Ordering::Acquire);
                if reap {
                    let _ = PROCS[pi].done.wait();
                    proc_free(pi);
                    bg_kill_forget(pid);
                }
                if status == EXEC_KILLED_STATUS {
                    BgPoll::Faulted
                } else {
                    BgPoll::Exited(status)
                }
            }
            _ => BgPoll::Running,
        };
    }
    BgPoll::Gone
}

/// WINX-2: kill a background pid through the U3.5 `KillSwitch` — request termination, then wait
/// (bounded) for the scheduler to confirm the reap at the target's next preemption boundary. The reap
/// tears the address space down via `free_user_space_by_cr3`, which since WINX-1 also retires the job's
/// compositor windows and drops its FB leaves, so a killed windowed app leaves nothing on the panel.
/// Returns an operator string for the shell.
pub fn bg_kill(pid: u64, _slot: u64) -> &'static str {
    let mut row: Option<usize> = None;
    for pi in 0..MAX_PROCS {
        if PROCS[pi].state.load(Ordering::Acquire) != PFREE
            && PROCS[pi].pid.load(Ordering::Acquire) == pid
        {
            row = Some(pi);
            break;
        }
    }
    let Some(pi) = row else {
        return "no such job (already reaped?)";
    };
    if PROCS[pi].state.load(Ordering::Acquire) == PEXITED {
        return "already exited — run `jobs` to reap it";
    }
    let Some(kill) = bg_kill_take(pid) else {
        return "already killed — the kill is still armed; the row frees itself when the task retires";
    };
    kill.request();
    let deadline = crate::arch::ticks() + KILL_CONFIRM_MS;
    while !kill.is_reaped() && crate::arch::ticks() < deadline {
        if PROCS[pi].state.load(Ordering::Acquire) == PEXITED {
            // It reached SYS_EXIT in the race window; the ordinary reap path owns it now.
            return "exited before the kill landed — run `jobs` to reap it";
        }
        crate::arch::sched::yield_now();
    }
    if !kill.is_reaped() {
        return "kill armed — the task retires at its next preemption";
    }
    // Confirmed reaped. The scheduler dropped the task without running the SYS_EXIT accounting, so mark
    // the row here — `jobs` must be able to reap it exactly like an ordinary exit.
    PROCS[pi].status.store(EXEC_KILLED_STATUS, Ordering::Release);
    PROCS[pi].state.store(PEXITED, Ordering::Release);
    PROCS[pi].done.post();
    "killed"
}

// =============================================================================================
// WINX-1: the ring-3 WINDOW fixture + its launcher. The x86 proof that an EL0 program can put a window
// on the compositor and present into it — the first one that ever could.
//
// It is an INLINE, position-independent blob (the sock2/u9x/u11x idiom — NOT an on-disk ELF), because
// x86 has no EL0 ELF loader yet: `run_user_image`/`spawn_user_image_bg` and the `run`/`bg` shell verbs
// are `#[cfg(feature = "baremetal")]`, i.e. aarch64 Pi-4-only, and building that loader is its own arc.
// The inline blob proves the SYSCALL SURFACE and the compositor binding, which is what this arc owes;
// running `crates/user-stat`'s actual ELF on x86 is the next arc's deliverable, and it will exercise
// exactly these verbs through exactly these paths.
//
// The fixture deliberately proves the FAIL-CLOSED direction too (bit6): a present of a window id it
// never created is `-EBADF`. A window verb suite that only proved the happy path would not have caught
// an ownership gate that admitted everything.
// =============================================================================================

// WINX-1 ring-3 fixture. Register + inline-data only. Runs correctly at any VA (RIP-relative). Witness
// bits (accumulated in r12, conveyed as the exit status):
//   bit0 SYS_GETINFO returned 0 with a non-zero pid · bit1 SYS_WIN_CREATE returned an id >= 0 ·
//   bit2 SYS_WIN_PRESENT returned 0 · bit3 the surface read back the pattern it wrote (the mapping is
//   really RW and really ours) · bit4 SYS_SLEEP_MS returned 0 · bit5 SYS_YIELD returned 0 ·
//   bit6 SYS_WIN_PRESENT of a never-created id is -EBADF. ALL = 0x7F.
// The callee-saved r12-r15 survive syscalls (the C dispatcher preserves them; the sysret tail scrubs
// only rdi/rsi/rdx/r8-r10). The surface store is `rep stosd` — 16384 dwords of a recognizable ARGB
// value — so a panel photograph and the kernel-side checksum see the same bytes.
core::arch::global_asm!(
    r#"
    .globl unaos_user_winx_blob_start
unaos_user_winx_blob_start:
    .balign 16
    .globl unaos_user_winx
unaos_user_winx:
    xor r12, r12                              // witness = 0
    lea r15, [rip + unaos_user_winx_blob_start]   // r15 = this program's window base (USER_BASE)

    // (0) SYS_GETINFO(&info) -> 0, into the DATA page (base + 0x1000). The RO code page would be
    //     -EFAULT by design (copy_to_user's write range starts past page 0), so this also proves the
    //     dest validation accepts a legitimate writable target.
    mov rax, 7                                // SYS_GETINFO
    lea rdi, [r15 + 0x1000]
    syscall
    test rax, rax
    jnz 1f                                    // non-zero -> no bit0
    cmp qword ptr [r15 + 0x1000], 0           // info.pid must be a real task id
    je 1f
    or r12, 1                                 // bit0: getinfo ok, pid non-zero
1:
    // (1) SYS_WIN_CREATE(128, 128) -> window id. Fail-closed: without a window there is no surface to
    //     write, so a negative return skips straight to the exit with the bits accumulated so far.
    mov rax, 29                               // SYS_WIN_CREATE
    mov rdi, 128
    mov rsi, 128
    syscall
    test rax, rax
    js 8f
    mov r13, rax                              // r13 = window id
    or r12, 2                                 // bit1: create ok

    // (2) Paint the surface. It lives at this program's own window base + 0x5000 (region slot 0 — the
    //     first window a process creates always lands there, which is the whole point of allocating
    //     region slots lowest-first). 128*128 = 16384 ARGB8888 pixels.
    lea r14, [r15 + 0x5000]                   // r14 = surface VA
    cld
    mov rdi, r14
    mov eax, 0xFF20C0FF                       // opaque cyan-ish — recognizable on glass and in a checksum
    mov rcx, 16384
    rep stosd

    // (3) Read the pattern back through the same mapping: proves the leaves really are EL0-RW and
    //     really are this process's own frames, not a stale or shared one.
    cmp dword ptr [r14], 0xFF20C0FF
    jne 2f
    cmp dword ptr [r14 + 65532], 0xFF20C0FF   // last pixel of the 64 KiB surface
    jne 2f
    or r12, 8                                 // bit3: surface readback matches
2:
    // (4) SYS_WIN_PRESENT(win) -> 0.
    mov rax, 30                               // SYS_WIN_PRESENT
    mov rdi, r13
    syscall
    test rax, rax
    jnz 3f
    or r12, 4                                 // bit2: present ok
3:
    // (5) SYS_SLEEP_MS(1) -> 0. The pacing verb a windowed app calls once per frame.
    mov rax, 5                                // SYS_SLEEP_MS
    mov rdi, 1
    syscall
    test rax, rax
    jnz 4f
    or r12, 16                                // bit4: sleep ok
4:
    // (6) SYS_YIELD -> 0.
    mov rax, 4                                // SYS_YIELD
    syscall
    test rax, rax
    jnz 5f
    or r12, 32                                // bit5: yield ok
5:
    // (7) A SECOND present, to prove the verb is repeatable across a sleep+yield (a one-shot present
    //     that wedged the compositor would still have set bit2).
    mov rax, 30                               // SYS_WIN_PRESENT
    mov rdi, r13
    syscall

    // (8) FAIL-CLOSED: present a window id this process never created. WIN_MAX-1 = 7 is in range but
    //     free, so the ownership gate must answer -EBADF (-9), not 0.
    mov rax, 30                               // SYS_WIN_PRESENT
    mov rdi, 7
    syscall
    cmp rax, -9                               // -EBADF
    jne 8f
    or r12, 64                                // bit6: unowned present refused -EBADF
8:  mov rax, 2                                // SYS_EXIT(witness)
    mov rdi, r12
    syscall
1:  jmp 1b                                    // sys_exit never returns; guard

    .globl unaos_user_winx_blob_end
unaos_user_winx_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_winx_blob_start: u8;
    static unaos_user_winx_blob_end: u8;
    static unaos_user_winx: u8;
}

/// WINX-1: the ring-3 fixture's witness bitmask (its exit status), routed by name in `SYS_EXIT`.
/// `WINX_DONE` gates the launcher's read; `WINX_KILLED` counts a (bug) fault-kill of the well-behaved
/// fixture — a non-zero value means the window verbs faulted a program that did nothing wrong.
static WINX_WITNESS: AtomicU32 = AtomicU32::new(0);
static WINX_DONE: AtomicU32 = AtomicU32::new(0);
static WINX_KILLED: AtomicU32 = AtomicU32::new(0);
/// All seven witness bits set = create + paint + readback + present + pace + fail-closed refusal.
const WINX_WITNESS_ALL: u32 = 0x7F;

/// WINX-1: the ARGB8888 value the fixture paints its whole surface with. The launcher re-reads the
/// KERNEL identity view of the surface after the fixture exits and checks this value, which is the
/// independent proof that ring-3's stores landed in the frames the compositor was handed — not merely
/// that the syscalls returned 0.
const WINX_FILL: u32 = 0xFF20_C0FF;

/// Build the WINX-1 fixture slot (the `sock2_build` shape): allocate a private slot, scrub the program
/// window, copy the blob into its RX-RO code page through the identity alias, return the run params.
/// `None` on slot-alloc failure. The FB region is NOT pre-mapped — mapping it is `SYS_WIN_CREATE`'s job,
/// and that it starts unmapped is part of what the fixture proves.
fn winx_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_winx_blob_start as usize;
    let bend = &raw const unaos_user_winx_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "WINX-1 blob does not fit in a code page");
    let off = (&raw const unaos_user_winx as usize - bstart) as u64;
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

/// WINX-1 launcher + verdict. Chained off `u8x_launcher` after the storage chain, BEFORE the
/// network-gated SOCK demos, so its line lands in a stable position whether or not a NIC is present and
/// whether or not `smolnet` is compiled.
///
/// Flow: one-shot; build + spawn the `winx-app` fixture; wait (bounded) for its witness exit; then run
/// the KERNEL-SIDE checks the fixture cannot run on itself — the surface really holds the pattern (read
/// through the kernel identity alias, not the ring-3 VA), the present counter really advanced, and the
/// slot tore down clean. PASS iff every witness bit is set AND the kernel-side checks agree AND no kill.
///
/// PANEL-INDEPENDENT by construction: a headless run has no ready framebuffer, so `wm::create` refuses
/// and `wm_id` is `WIN_NONE` — the syscalls still succeed, the surface is still mapped and painted, and
/// the present counter still advances. The verdict is therefore about the SYSCALL SURFACE, which is what
/// QEMU can honestly witness; the compositor binding is what `UNAOS_WC=1` on the bench proves.
fn winx_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let presents_before = fb_present_count();
    let Some(fix) = winx_build() else {
        serial_println!(":: WINX-1: no free address-space slot — window demo skipped ::");
        return;
    };
    serial_println!(
        ":: WINX-1: ring-3 windows — SYS_WIN_CREATE(29)/SYS_WIN_PRESENT(30) + SYS_GETINFO(7)/SYS_SLEEP_MS(5)/SYS_YIELD(4), an EL0 program paints a compositor window ::"
    );
    crate::arch::sched::spawn_user_in_space("winx-app", fix.entry, fix.sp, demo_cpu, fix.cr3);

    let vdeadline = crate::arch::ticks() + 10_000;
    while WINX_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = WINX_WITNESS.load(Ordering::Acquire);
    let killed = WINX_KILLED.load(Ordering::Acquire);

    // Kernel-side proof #1: the surface frames really hold what ring 3 wrote. Read through the KERNEL
    // identity pointer — the same pointer the compositor was handed — so this is independent of whether
    // the ring-3 mapping was ever correct in the direction the fixture tested.
    let surf = crate::arch::memory::slot_fb_win_surface_ptr(fix.slot, 0) as *const u32;
    let painted = unsafe { surf.read_volatile() == WINX_FILL && surf.add(16383).read_volatile() == WINX_FILL };

    // Kernel-side proof #2: presents actually reached the compositor seam (two of them — the fixture
    // presents once, sleeps/yields, and presents again).
    let presents = fb_present_count() - presents_before;

    // Teardown proof: the fixture's exit freed its slot, which retires its windows and drops its FB
    // leaves. Poll bounded; the window table row for this slot must go free.
    let tdeadline = crate::arch::ticks() + 2000;
    while winx_slot_has_window(fix.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = !winx_slot_has_window(fix.slot);

    // SERIAL SETTLE — not cosmetic, and the reason this line was intermittently missing.
    // `serial::_print` takes the UART with `try_lock` and DROPS the line on contention (by design: it
    // must never block an IRQ-masked or panicking context). This verdict is emitted immediately after
    // the compositor's create/present/close burst, which the render task prints from ANOTHER core — so
    // roughly one run in three the verdict lost the race and vanished, taking the suite from 32 PASS to
    // 31 with no FAIL to explain it. Everything this verdict reports is already latched in locals above,
    // and the fixture and its window are gone by now, so yielding here costs nothing and lets the
    // compositor's queued lines drain before we take the port.
    //
    // This is a MITIGATION, not a fix: the drop-on-contention writer makes ANY serial line racy, and the
    // suite's PASS-count gate inherits that. Flagged for the integrator — the real fix is a buffered or
    // blocking serial writer, which is not this arc's lane.
    for _ in 0..64 {
        crate::arch::sched::yield_now();
    }

    if witness == WINX_WITNESS_ALL && painted && presents >= 2 && cleared && killed == 0 {
        serial_println!(
            ":: WINX-1: ring-3 windows — create/paint/readback/present OK, sleep+yield OK, unowned present -EBADF, surface verified kernel-side, {} presents, teardown clean -> PASS ::",
            presents
        );
    } else {
        serial_println!(
            ":: WINX-1: ring-3 windows FAIL — witness={:#x} painted={} presents={} cleared={} killed={} done={} (want {:#x}/true/>=2/true/0/1) ::",
            witness,
            painted,
            presents,
            cleared,
            killed,
            WINX_DONE.load(Ordering::Acquire),
            WINX_WITNESS_ALL
        );
    }

    // CLICK-ROUTE — run the arch-neutral hit-test witness on the x86 panel, for the first time.
    //
    // `video::wm::hittest_selftest` has been in the tree since CLICK-ROUTE and has only ever been
    // driven from `arch::aarch64::syscall` (its sole call site, after that arch's `wci_rollup`), so
    // every claim it makes — that `hit_test` names the FRONTMOST window, that a raise moves who owns
    // a pixel, that a miss is a miss, that a window below `SHELL_Z` is unclickable — has been an
    // aarch64-only claim about arch-neutral code. Nothing in the function is arch-specific: it reads
    // the window TABLE rather than the panel (deliberately, so it is drivable with no pointer at
    // all), mints and closes its own two rows, and restores `SHELL_Z`/`FOCUS_ASID` before returning.
    // The x86 panel is a different geometry with a different upscale and a live console window in the
    // table, and the selftest derives its probe and miss points from the row it actually got — so
    // running it here asserts the same five legs against x86's real z-order.
    //
    // HERE, and not earlier: the selftest puts two rows of its own in the table and drives
    // `focus_changed(0)`, which pushes every live window below the shell. The WINX-1 fixture above
    // owns the one-shot per-window latches (`[wc-d] verify`, `[wc-g]`/`[wc-h]` rollups) and its
    // window must be gone — the `cleared` poll above is exactly that — before rows that would burn
    // them appear. That is the same ordering rule aarch64 states at its own call site.
    //
    // CLICK-X86 — the press path has since ARRIVED, so the two witnesses now run as a pair and this
    // paragraph records the division of labour rather than the old absence. `hittest_selftest` asserts
    // the ADDRESS LOOKUP (which window owns a pixel); `clickroute_selftest`, immediately below,
    // asserts the ROUTING built on it (who receives the press). Order matters for the same reason
    // stated above — both mint rows of their own, so they belong after every one-shot per-window latch
    // — and `hittest_selftest` goes first because its `focus_changed(0)` leg is the more disruptive of
    // the two and it restores `SHELL_Z`/`FOCUS_ASID` before returning.
    #[cfg(feature = "witness")]
    crate::video::wm::hittest_selftest();
    #[cfg(feature = "witness")]
    clickroute_selftest();
}

// =============================================================================================
// WINX-6: the END-TO-END witness — STAT.ELF off the boot volume, through the real loader, into a
// compositor window, then killed.
//
// Everything the previous WINX arcs proved in pieces, proved as ONE PATH and with the ACTUAL SHIPPING
// ARTIFACT rather than an inline fixture: the FAT read the shell's `bg` does, `spawn_user_image_bg`,
// `elf::validate_elf` + `map_image_into_slot` (a real two-segment ELF64 with per-segment W^X, not a
// one-page blob), ring 3, `SYS_GETINFO`/`SYS_WIN_CREATE`/`SYS_WIN_PRESENT`/`SYS_SLEEP_MS`, the
// compositor, and finally `bg_kill` + the reap.
//
// It uses the SAME BYTES the bench operator will run. `crates/user-stat` is built for x86 by arroyo's
// `build_user_stat_x86` and staged as `STAT.ELF` on the boot volume; this launcher reads that file, so a
// packaging break (wrong arch, truncated image, missing from the volume) fails HERE rather than at the
// bench. That is the whole point of witnessing the artifact instead of a fixture.
//
// STAT.ELF has NO exit path by design ("runs until it is killed"), so the kill is not cleanup bolted on
// the end — it is the last leg of the contract under test, and the only way this launcher can terminate.

/// WINX-6: the witness's own gate — how many presents the program must land before we accept that it is
/// really running its paint loop rather than having created a window and wedged. STAT.ELF presents once
/// per ~50 ms frame, so this is well under a second of its life.
const WINX2_MIN_PRESENTS: u64 = 3;

/// WINX-6: load `STAT.ELF` off the FAT boot volume and run the whole `bg` lifecycle on it.
/// Chained after `winx_launcher`, so the inline-fixture verdict (which proves the verbs in isolation)
/// lands first and this one proves the shipping path.
fn winx2_launcher(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // Read the image exactly as the shell's x86 `bg` does: FAT boot partition, root directory, 8.3 name.
    let Ok(fs) = crate::fs::fat::mount() else {
        serial_println!(":: WINX-2: no FAT boot volume — STAT.ELF end-to-end witness skipped ::");
        return;
    };
    let Ok(de) = fs.find_in_root("STAT.ELF") else {
        // WINX-7 PKG — the message names the VOLUME, because the previous wording ("the boot volume")
        // was not merely vague, it was WRONG in the case that actually fired. `fat::mount()` binds the
        // global block device, which on x86 is always the USB mass-storage device xHCI enumerated;
        // the volume UEFI booted from is a different thing and is unreachable after ExitBootServices.
        // An attended rMBP boot hit exactly that split — STAT.ELF was on the booted SD card, the mount
        // was the USB stick — and the old message sent the operator to check the card, which was fine.
        // Naming the mounted volume, and the tree that stages it, is the whole remedy from this side.
        serial_println!(
            ":: WINX-2: STAT.ELF absent from the mounted DATA volume (the USB mass-storage device on storage_slot — NOT the UEFI boot volume, which the kernel cannot read) — stage target/x86_64_data/ onto it; end-to-end witness skipped ::"
        );
        return;
    };
    let cap = user_window_size();
    if de.size == 0 || de.size as usize > cap {
        serial_println!(
            ":: WINX-2: STAT.ELF is {} bytes, outside the {}-byte EL0 window — witness skipped ::",
            de.size, cap
        );
        return;
    }
    let mut bytes = alloc::vec![0u8; de.size as usize];
    if fs.read_file(&de, &mut bytes, cap).is_err() {
        serial_println!(":: WINX-2: STAT.ELF read failed — end-to-end witness skipped ::");
        return;
    }
    serial_println!(
        ":: WINX-2: STAT.ELF end-to-end — {} bytes off the boot volume, through the ELF loader into a compositor window ::",
        bytes.len()
    );

    let presents_before = fb_present_count();
    let (pid, slot, entry) = match spawn_user_image_bg(&bytes) {
        Ok(v) => v,
        Err(why) => {
            serial_println!(":: WINX-2: STAT.ELF end-to-end FAIL — bg spawn rejected: {} ::", why);
            return;
        }
    };
    let slot = slot as usize;

    // Wait (bounded) for the program to prove it is alive: a window of its own AND presents landing.
    let deadline = crate::arch::ticks() + 5_000;
    let mut windowed = false;
    let mut presents = 0u64;
    while crate::arch::ticks() < deadline {
        windowed |= winx_slot_has_window(slot);
        presents = fb_present_count() - presents_before;
        if windowed && presents >= WINX2_MIN_PRESENTS {
            break;
        }
        crate::arch::sched::yield_now();
    }

    // The kill IS part of the contract: STAT.ELF has no exit path, so this is the only way it stops.
    let verdict = bg_kill(pid, slot as u64);
    let killed = verdict == "killed";
    // Reap the row through the same call `jobs` makes, then confirm the pid is gone.
    let reaped = matches!(bg_poll(pid, true), BgPoll::Faulted | BgPoll::Exited(_));
    let gone = matches!(bg_poll(pid, false), BgPoll::Gone);
    // Teardown: the kill's address-space free retires the window rows and drops the FB leaves.
    let tdeadline = crate::arch::ticks() + 2_000;
    while winx_slot_has_window(slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = !winx_slot_has_window(slot);

    // SERIAL SETTLE — see the note in `winx_launcher`: the UART writer drops lines on contention, and
    // this verdict follows the compositor's close/erase burst from another core.
    for _ in 0..64 {
        crate::arch::sched::yield_now();
    }

    if windowed && presents >= WINX2_MIN_PRESENTS && killed && reaped && gone && cleared {
        serial_println!(
            ":: WINX-2: STAT.ELF end-to-end — loaded (entry {:#x}) + windowed + {} presents, killed + reaped, teardown clean -> PASS ::",
            entry, presents
        );
    } else {
        serial_println!(
            ":: WINX-2: STAT.ELF end-to-end FAIL — windowed={} presents={} killed={} ({}) reaped={} gone={} cleared={} (want true/>={}/true/true/true/true) ::",
            windowed, presents, killed, verdict, reaped, gone, cleared, WINX2_MIN_PRESENTS
        );
    }
}

// =============================================================================================
// WINX-6b: the HEADLESS ELF-loader witness.
//
// WINX-2 above is the right BENCH witness — it runs the actual shipped `STAT.ELF` off the boot volume —
// but it cannot run in this repo's headless x86 CI: `./arroyo test` attaches NO block device (which is
// why the U9x/U10x verdicts all say "no FAT volume"), and the FAT image builder
// (`scripts/make-fat-img.sh`) is macOS-only. So WINX-2 skips with one honest line here and proves itself
// on the bench.
//
// That would leave the ELF LOADER — the whole point of this arc, and the most security-sensitive code in
// it — unproven in CI. This witness closes that: it SYNTHESIZES a real, valid, multi-segment ELF64 in
// memory around the existing `winx-app` ring-3 blob and pushes it through the SAME
// `spawn_user_image_bg` the shell's `bg` calls. Nothing about the loader is stubbed or bypassed —
// `validate_elf` parses this image field by field, `map_image_into_slot` walks its two `PT_LOAD`s,
// biases them, zeroes the `.bss` tail, and applies per-segment W^X, exactly as it would for STAT.ELF.
//
// The blob is position-independent and already computes everything from its own load address, so
// wrapping it in an ELF changes nothing about how it runs — but it now arrives through the ELF path
// rather than a raw copy, and its exit STATUS carries its 7-bit witness bitmask back out through the
// Proc table. A loader bug that mis-biased a segment, mis-set a permission, or mis-computed the entry
// shows up as a fault-kill or a wrong witness, not as a silent pass.

/// WINX-6b: build a valid two-segment ELF64 image around the `winx-app` blob.
///
/// Layout — deliberately simple, and deliberately NOT page-congruent between file offset and vaddr,
/// because the loader does not require congruence (it copies `p_filesz` bytes from `p_offset` to
/// `p_vaddr - min_vaddr`) and pinning that down is worth a witness:
///   file [0x00..0x40)      Ehdr
///   file [0x40..0xB0)      two Phdrs
///   file [0x1000..)        the blob bytes  (text segment's `p_offset`)
///   vaddr [0x0000..0x1000) text  PF_R|PF_X  — the blob, entry at its `unaos_user_winx` offset
///   vaddr [0x1000..0x2000) data  PF_R|PF_W  — filesz 0, memsz one page (exercises the zero-the-tail
///                                             path), and it is the page the blob writes its
///                                             `SYS_GETINFO` result into
fn winx_elf_image() -> alloc::vec::Vec<u8> {
    let bstart = &raw const unaos_user_winx_blob_start as usize;
    let bend = &raw const unaos_user_winx_blob_end as usize;
    let blen = bend - bstart;
    let entry_off = (&raw const unaos_user_winx as usize - bstart) as u64;
    const TEXT_FILE_OFF: usize = 0x1000;

    let mut img = alloc::vec![0u8; TEXT_FILE_OFF + blen];
    // --- Ehdr ---
    img[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    img[4] = 2; // EI_CLASS = ELFCLASS64
    img[5] = 1; // EI_DATA  = ELFDATA2LSB
    img[6] = 1; // EI_VERSION
    img[16..18].copy_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    img[18..20].copy_from_slice(&62u16.to_le_bytes()); // e_machine = EM_X86_64
    img[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    img[24..32].copy_from_slice(&entry_off.to_le_bytes()); // e_entry
    img[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    img[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    img[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    img[56..58].copy_from_slice(&2u16.to_le_bytes()); // e_phnum

    // --- Phdr 0: text (R+X) ---
    let p = 64;
    img[p..p + 4].copy_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    img[p + 4..p + 8].copy_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R|PF_X
    img[p + 8..p + 16].copy_from_slice(&(TEXT_FILE_OFF as u64).to_le_bytes()); // p_offset
    img[p + 16..p + 24].copy_from_slice(&0u64.to_le_bytes()); // p_vaddr
    img[p + 32..p + 40].copy_from_slice(&(blen as u64).to_le_bytes()); // p_filesz
    img[p + 40..p + 48].copy_from_slice(&0x1000u64.to_le_bytes()); // p_memsz
    img[p + 48..p + 56].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align

    // --- Phdr 1: data (R+W), filesz 0 / memsz one page — the zero-the-tail path ---
    let p = 64 + 56;
    img[p..p + 4].copy_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    img[p + 4..p + 8].copy_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R|PF_W
    img[p + 8..p + 16].copy_from_slice(&0u64.to_le_bytes()); // p_offset (filesz 0, so unused)
    img[p + 16..p + 24].copy_from_slice(&0x1000u64.to_le_bytes()); // p_vaddr
    img[p + 32..p + 40].copy_from_slice(&0u64.to_le_bytes()); // p_filesz
    img[p + 40..p + 48].copy_from_slice(&0x1000u64.to_le_bytes()); // p_memsz
    img[p + 48..p + 56].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align

    // --- the blob itself ---
    unsafe {
        core::ptr::copy_nonoverlapping(bstart as *const u8, img.as_mut_ptr().add(TEXT_FILE_OFF), blen);
    }
    img
}

/// WINX-6b: push the synthesized ELF through the real `bg` lifecycle and check what came out the other
/// side. PASS requires the loader to have accepted a genuine two-segment ELF64, the program to have run
/// correctly from its biased entry (its exit STATUS is the same 7-bit witness bitmask the inline fixture
/// reports — so every window verb was exercised through the ELF path), and the row to reap clean.
fn winx3_launcher(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let img = winx_elf_image();
    // Prove the VALIDATOR directly too, before the mapper ever sees it: this is the field-by-field
    // parse, and its plan is what the mapper trusts.
    let plan_ok = match super::elf::validate_elf(&img, user_window_size()) {
        Ok(p) => p.nsegs == 2 && p.min_vaddr == 0,
        Err(_) => false,
    };
    // And prove it REJECTS what it must — the same image with e_machine forged to aarch64's 183. A
    // validator that accepted this would map an aarch64 image into an x86 address space.
    let mut wrong_arch = img.clone();
    wrong_arch[18..20].copy_from_slice(&183u16.to_le_bytes());
    let rejects_wrong_arch = super::elf::validate_elf(&wrong_arch, user_window_size()).is_err();
    // And a W+X segment — the W^X refusal, which is the one validation failure with security weight.
    let mut wx = img.clone();
    wx[64 + 4..64 + 8].copy_from_slice(&7u32.to_le_bytes()); // PF_R|PF_W|PF_X
    let rejects_wx = super::elf::validate_elf(&wx, user_window_size()).is_err();

    serial_println!(
        ":: WINX-3: ELF loader — a synthesized two-segment ELF64 ({} bytes) through spawn_user_image_bg, the same path `bg` takes ::",
        img.len()
    );

    let presents_before = fb_present_count();
    let (pid, slot, entry) = match spawn_user_image_bg(&img) {
        Ok(v) => v,
        Err(why) => {
            serial_println!(":: WINX-3: ELF loader FAIL — bg spawn rejected: {} ::", why);
            return;
        }
    };
    let slot = slot as usize;

    // The blob exits on its own (unlike STAT.ELF), so wait for the row to settle rather than killing it.
    let deadline = crate::arch::ticks() + 10_000;
    let mut windowed = false;
    while crate::arch::ticks() < deadline {
        windowed |= winx_slot_has_window(slot);
        if matches!(bg_poll(pid, false), BgPoll::Exited(_) | BgPoll::Faulted) {
            break;
        }
        crate::arch::sched::yield_now();
    }
    let presents = fb_present_count() - presents_before;
    let status = match bg_poll(pid, true) {
        BgPoll::Exited(s) => s,
        _ => -1,
    };
    let gone = matches!(bg_poll(pid, false), BgPoll::Gone);
    let tdeadline = crate::arch::ticks() + 2_000;
    while winx_slot_has_window(slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = !winx_slot_has_window(slot);

    // SERIAL SETTLE — see `winx_launcher`.
    for _ in 0..64 {
        crate::arch::sched::yield_now();
    }

    let ok = plan_ok
        && rejects_wrong_arch
        && rejects_wx
        && status == WINX_WITNESS_ALL as i32
        && windowed
        && presents >= 2
        && gone
        && cleared;
    if ok {
        serial_println!(
            ":: WINX-3: ELF loader — 2 PT_LOADs mapped W^X, entry {:#x}, ring-3 witness {:#x} through the ELF path, {} presents, wrong-arch + W+X images refused, reap clean -> PASS ::",
            entry, status, presents
        );
    } else {
        serial_println!(
            ":: WINX-3: ELF loader FAIL — plan={} rej_arch={} rej_wx={} status={:#x} windowed={} presents={} gone={} cleared={} (want true/true/true/{:#x}/true/>=2/true/true) ::",
            plan_ok, rejects_wrong_arch, rejects_wx, status, windowed, presents, gone, cleared,
            WINX_WITNESS_ALL
        );
    }
}

// =============================================================================================
// WINX-7: the ring-3 THREADS + FUTEX + INPUT fixture and its launcher — the headless proof that the
// four verb families this arc adds work together, from ring 3, through the real syscall path.
//
// It is an INLINE, position-independent blob (the WINX-1 / sock2 / u9x idiom) rather than the shipped
// `VUG.ELF`, for the reason WINX-3's header already records: `./arroyo test` attaches no FAT volume,
// so an on-disk artifact cannot be read in CI. The two witnesses divide the work exactly as the WINX-1
// / WINX-2 pair does — this one proves the SYSCALL SURFACE in CI on every run, and WINX-8 below proves
// the SHIPPED ARTIFACT at the bench.
//
// What it proves that no earlier fixture could:
//   * Two ring-3 tasks really run under ONE CR3 — the workers write into the parent's data page and
//     the parent reads what they wrote.
//   * The thread ARGUMENT ABI works: each worker is passed a distinct index in rdi and writes its
//     magic at an offset computed from it, so a broken `arg` delivery shows up as a missing magic
//     rather than as a plausible-looking pass.
//   * The FUTEX is a real park, not a spin: the parent blocks in `FUTEX_WAIT` and is released by the
//     workers' `FUTEX_WAKE`. The launcher independently witnesses the park through
//     `futex_park_count()` — the count of `FUTEX_WAIT`s that actually blocked and were woken. That is
//     the difference between "the barrier completed" and "the barrier completed BECAUSE the futex
//     worked".
//   * `SYS_INPUT_POLL` delivers a routed event to the focused process and only to it.
//   * `SYS_THREAD_JOIN` returns after the workers' `SYS_THREAD_EXIT`, and the refcounted teardown
//     leaves the slot clean — a first-thread-frees bug would have torn the address space down under
//     the still-running parent and shown up as a fault-kill, not a wrong bit.
// =============================================================================================

// WINX-7 ring-3 fixture. Register + inline-data only; runs correctly at any VA (RIP-relative).
//
// Window layout it assumes (the 4-page ring-3 program window):
//   page 0 (+0x0000)  code, RX/RO          — this blob
//   page 1 (+0x1000)  data, RW/NX          — [+0x1000] done counter, [+0x1004] magic A, [+0x1008] magic B
//   page 2 (+0x2000)  RW/NX                — worker A's stack (grows down from +0x3000)
//   page 3 (+0x3000)  RW/NX                — worker B's stack (from +0x3800), parent's (from +0x4000)
// The same stack carve `user-vug` uses, so this fixture and the shipped program agree about the one
// piece of the ABI a program chooses for itself.
//
// Callee-saved registers survive syscalls (the C dispatcher preserves them; the sysret tail scrubs only
// rdi/rsi/rdx/r8/r9/r10), so the witness accumulator and the handles live in r12-r15/rbx/rbp. The poll
// budget in particular MUST be callee-saved — r8 would be scrubbed to 0 by the first syscall's return.
//
// Witness bits (accumulated in r12, conveyed as the exit status):
//   bit0 SYS_WIN_CREATE ok · bit1 thread A spawned · bit2 thread B spawned · bit3 the futex barrier
//   completed (both workers arrived) · bit4 BOTH workers wrote their magic at the offset their `arg`
//   selected (the thread-argument ABI) · bit5 SYS_INPUT_POLL delivered a routed event · bit6
//   SYS_WIN_PRESENT ok · bit7 both SYS_THREAD_JOINs returned 0. ALL = 0xFF.
core::arch::global_asm!(
    r#"
    .globl unaos_user_winx7_blob_start
unaos_user_winx7_blob_start:
    .balign 16
    .globl unaos_user_winx7
unaos_user_winx7:
    xor r12, r12                              // witness = 0
    lea r15, [rip + unaos_user_winx7_blob_start]  // r15 = this program's window base
    mov dword ptr [r15 + 0x1000], 0           // done   = 0
    mov dword ptr [r15 + 0x1004], 0           // magicA = 0
    mov dword ptr [r15 + 0x1008], 0           // magicB = 0

    // (0) SYS_WIN_CREATE(128, 128). Fail-closed: without a window there is nothing to present and no
    //     info page, so a negative return goes straight to the exit with the bits so far.
    mov rax, 29
    mov rdi, 128
    mov rsi, 128
    syscall
    test rax, rax
    js 90f
    mov r13, rax                              // r13 = window id
    or r12, 1

    // (1) SYS_THREAD_SPAWN(worker, sp = base+0x3000, arg = 0, place = 0 — this core).
    lea rdi, [rip + unaos_user_winx7_worker]
    lea rsi, [r15 + 0x3000]
    xor rdx, rdx
    xor r10, r10
    mov rax, 21
    syscall
    test rax, rax
    js 10f
    mov r14, rax                              // r14 = handle A
    or r12, 2
10:
    // (2) SYS_THREAD_SPAWN(worker, sp = base+0x3800, arg = 1, place = 1 — a sibling core).
    lea rdi, [rip + unaos_user_winx7_worker]
    lea rsi, [r15 + 0x3800]
    mov rdx, 1
    mov r10, 1
    mov rax, 21
    syscall
    test rax, rax
    js 20f
    mov rbx, rax                              // rbx = handle B
    or r12, 4
20:
    // (3) The FUTEX frame barrier: block until `done` reaches 2. Skipped unless BOTH spawns
    //     succeeded — a barrier whose target cannot be reached is the wedge itself, which is the
    //     VUGGUARD rule this fixture must not violate either.
    mov eax, r12d
    and eax, 6
    cmp eax, 6
    jne 40f
30: mov eax, dword ptr [r15 + 0x1000]
    cmp eax, 2
    jge 35f
    // SYS_FUTEX(&done, FUTEX_WAIT = 0, expected = the value we just read). The kernel re-compares
    // under the bucket lock, so a wake landing between the load and the call cannot be lost.
    mov edx, eax
    lea rdi, [r15 + 0x1000]
    xor rsi, rsi
    mov rax, 26
    syscall
    jmp 30b
35: or r12, 8                                 // bit3: both workers arrived
    // bit4: each worker wrote its magic at base+0x1004 + arg*4 — so this is a positive test of the
    // thread-argument register, not merely of "a worker ran".
    cmp dword ptr [r15 + 0x1004], -1517032219 // 0xA5A5A5A5 as a sign-extended imm32
    jne 40f
    cmp dword ptr [r15 + 0x1008], -1517032219
    jne 40f
    or r12, 16
40:
    // (4) SYS_INPUT_POLL until the launcher's injected event arrives, or the budget is spent. The
    //     budget lives in rbp because it must SURVIVE the syscall — the sysret tail scrubs r8-r10.
    mov rbp, 2000000
50: mov rax, 27
    syscall
    test rax, rax
    jns 55f                                   // a non-negative return IS a packed event
    dec rbp
    jnz 50b
    jmp 60f
55: or r12, 32                                // bit5: a routed input event reached this process
60:
    // (5) SYS_WIN_PRESENT(win).
    mov rax, 30
    mov rdi, r13
    syscall
    test rax, rax
    jnz 70f
    or r12, 64                                // bit6: present ok
70:
    // (6) Join both workers. Only joined if they were spawned — joining a value that is a negative
    //     errno is a bogus syscall, and joining a thread that never started would be a lie about
    //     what was reclaimed.
    mov eax, r12d
    and eax, 6
    cmp eax, 6
    jne 90f
    mov rax, 23
    mov rdi, r14
    syscall
    mov rbp, rax                              // accumulate both join returns
    mov rax, 23
    mov rdi, rbx
    syscall
    or rbp, rax
    test rbp, rbp
    jnz 90f
    or r12, 128                               // bit7: both joins returned 0
90: mov rax, 2                                // SYS_EXIT(witness)
    mov rdi, r12
    syscall
95: jmp 95b                                   // sys_exit never returns; guard

    // ---- the worker thread. Entered at ring 3 with rdi = the `arg` SYS_THREAD_SPAWN was given. ----
    .balign 16
    .globl unaos_user_winx7_worker
unaos_user_winx7_worker:
    mov rbx, rdi                              // stash `arg` — rdi is about to be a syscall argument
    lea r15, [rip + unaos_user_winx7_blob_start]
    // magic[arg] = 0xA5A5A5A5 — the store whose ADDRESS depends on `arg`, so a wrong argument writes
    // the wrong slot and the parent's bit4 check fails.
    lea rcx, [r15 + 0x1004]
    mov eax, -1517032219                      // 0xA5A5A5A5
    mov dword ptr [rcx + rbx*4], eax
    // Arrive: atomically bump `done`, then wake the parent. The lock-prefixed add is what makes two
    // workers on two cores both count.
    mov eax, 1
    lock xadd dword ptr [r15 + 0x1000], eax
    // SYS_FUTEX(&done, FUTEX_WAKE = 1, n = 1)
    lea rdi, [r15 + 0x1000]
    mov rsi, 1
    mov rdx, 1
    mov rax, 26
    syscall
    // SYS_THREAD_EXIT — posts this thread's completion (releasing the parent's join) and drops its
    // hold on the shared address space. Never returns.
    mov rax, 22
    syscall
1:  jmp 1b

    .globl unaos_user_winx7_blob_end
unaos_user_winx7_blob_end:
"#
);

unsafe extern "C" {
    static unaos_user_winx7_blob_start: u8;
    static unaos_user_winx7_blob_end: u8;
    static unaos_user_winx7: u8;
}

/// WINX-7: the fixture's witness bitmask (its exit status), routed by name in `SYS_EXIT`.
static WINX7_WITNESS: AtomicU32 = AtomicU32::new(0);
static WINX7_DONE: AtomicU32 = AtomicU32::new(0);
static WINX7_KILLED: AtomicU32 = AtomicU32::new(0);
/// All eight bits: window + two spawns + the futex barrier + the thread-argument ABI + a routed input
/// event + a present + two clean joins.
const WINX7_WITNESS_ALL: u32 = 0xFF;

/// WINX-7: the ring-3 task name, so the `SYS_EXIT` arm can route the witness by name (the u5x/u6x
/// idiom — x86 has no `SYS_REPORT`).
const WINX7_TASK_NAME: &str = "winx7-app";

/// WINX-7: build the fixture's slot — allocate a private address space, scrub the program window, and
/// copy the blob into its RX/RO code page through the kernel identity alias. The FB region is NOT
/// pre-mapped; mapping it is `SYS_WIN_CREATE`'s job, and that it starts unmapped is part of what the
/// fixture proves. `None` on slot-alloc failure.
fn winx7_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_winx7_blob_start as usize;
    let bend = &raw const unaos_user_winx7_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "WINX-7 blob does not fit in a code page");
    let off = (&raw const unaos_user_winx7 as usize - bstart) as u64;
    let backing = crate::arch::memory::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, (USER_WINDOW_PAGES * PAGE_SIZE) as usize);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    Some(U7xFix {
        entry: USER_BASE + off,
        // The parent's stack top is the window top; the workers' are carved BELOW it at +0x3800 and
        // +0x3000 by the blob, so the three never overlap.
        sp: USER_BASE + USER_WINDOW_PAGES * PAGE_SIZE - 16,
        cr3: crate::arch::memory::slot_cr3(slot),
        slot,
    })
}

/// WINX-7 launcher + verdict. Chained after the WINX-3 loader witness, so the arc's four verb families
/// are proved after the machinery they build on.
///
/// The launcher does three things the fixture cannot do for itself:
///   1. GRANTS IT FOCUS explicitly (`el0_input_set_active`), rather than relying on the create-time
///      auto-grant. Both paths exist and both are correct, but a witness that depended on the implicit
///      one would silently stop testing input the day the focus policy changed.
///   2. INJECTS an input event through the real `el0_input_enqueue` router seam — the same function
///      the shell's drain calls. QEMU delivers no USB HID at all, so a kernel-side injection is the
///      only way this leg can be witnessed headlessly, and routing it through the seam rather than
///      straight into the ring means the focus gate is on the tested path.
///   3. WITNESSES THE PARK independently, through `futex_park_count()` — the count of `FUTEX_WAIT`s
///      that actually blocked and were woken. Without it, a barrier that completed by luck (both
///      workers finishing before the parent ever reached the wait) would be indistinguishable from a
///      working futex. It is a COUNTER rather than a sample of the parked set, which the first cut
///      used and which was flaky by construction: a park beginning and ending between two samples is
///      invisible, so a perfectly healthy run could report "no park observed" purely on timing.
fn winx7_launcher(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let presents_before = fb_present_count();
    let (spawned_before, joined_before, exited_before) = thread_stats();
    let parks_before = futex_park_count();
    let Some(fix) = winx7_build() else {
        serial_println!(":: WINX-7: no free address-space slot — threads/futex/input demo skipped ::");
        return;
    };
    serial_println!(
        ":: WINX-7: ring-3 threads + futex + input — SYS_THREAD_SPAWN(21)/_EXIT(22)/_JOIN(23), SYS_FUTEX(26), SYS_INPUT_POLL(27), two EL0 threads under one CR3 ::"
    );
    crate::arch::sched::spawn_user_in_space(
        WINX7_TASK_NAME,
        fix.entry,
        fix.sp,
        crate::arch::sched::meter_current_cpu(),
        fix.cr3,
    );

    // Wait for the fixture, granting focus once its window appears and injecting input while it polls.
    // The futex park is counted kernel-side (`futex_park_count`), not sampled here.
    let deadline = crate::arch::ticks() + 10_000;
    let mut focused = false;
    let mut injected: u32 = 0;
    while WINX7_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < deadline {
        if !focused && winx_slot_has_window(fix.slot) {
            el0_input_set_active((fix.slot as u64) + 1);
            focused = true;
        }
        if focused && injected < 64 {
            // A plain printable key — the simplest event that survives `pack_input` and is trivially
            // recognisable if it ever needs to be read off the wire. Injected repeatedly because the
            // fixture reaches its poll only after the barrier, and a single early injection would be
            // consumed by nothing (drop-newest on a full ring makes repetition harmless).
            if el0_input_enqueue(crate::pal::Event::Key(b'k')) {
                injected += 1;
            }
        }
        crate::arch::sched::yield_now();
    }
    let witness = WINX7_WITNESS.load(Ordering::Acquire);
    let killed = WINX7_KILLED.load(Ordering::Acquire);
    let presents = fb_present_count() - presents_before;
    let (spawned, joined, exited) = thread_stats();
    // The PARK count, not a sample of the parked set: a `FUTEX_WAIT` that blocked and was woken is
    // recorded when it returns, so a park of any duration is caught and a run cannot fail on timing.
    let parks = futex_park_count() - parks_before;

    // Teardown proof: the fixture's exit freed its slot only after the LAST thread retired, which
    // retires its window rows and drops its FB leaves. A first-thread-frees bug shows up here as a
    // window row that went free while the parent was still presenting — or, far more likely, as a
    // fault-kill counted in `killed`.
    let tdeadline = crate::arch::ticks() + 2_000;
    while winx_slot_has_window(fix.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = !winx_slot_has_window(fix.slot);
    // Focus must have returned to the shell when the slot was torn down. If it had not, every later
    // keystroke on this boot would be enqueued into a ring with no consumer.
    let focus_released = el0_input_active() == 0;

    // SERIAL SETTLE — see the note in `winx_launcher`: the UART writer drops lines on contention and
    // this verdict follows the compositor's close/erase burst from another core.
    for _ in 0..64 {
        crate::arch::sched::yield_now();
    }

    // FUTEX-DUP: the double-claim witness, reported once per boot from the futex's own verdict site
    // rather than from `futex_wait`/`futex_wake` (both of which `user-vug` reaches once per frame).
    // `observed=0` is the healthy reading; nonzero says the race happened and the wake-side full scan
    // absorbed it — the verdict below deliberately does NOT gate on it, because absorbing the race is
    // correct behaviour and a boot that observes one is still a passing boot.
    crate::arch::sched::futex_dup_witness();

    let threads_ok = spawned - spawned_before == 2
        && joined - joined_before == 2
        && exited - exited_before == 2;
    if witness == WINX7_WITNESS_ALL
        && threads_ok
        && parks >= 1
        && presents >= 1
        && cleared
        && focus_released
        && killed == 0
    {
        serial_println!(
            ":: WINX-7: ring-3 threads + futex + input — 2 threads under one CR3 spawned/arrived/joined, {} futex park(s) witnessed, thread-arg ABI verified, {} injected event(s) polled, {} present(s), focus released, teardown clean -> PASS ::",
            parks, injected, presents
        );
    } else {
        serial_println!(
            ":: WINX-7: ring-3 threads + futex + input FAIL — witness={:#x} spawned={} joined={} exited={} parks={} presents={} injected={} cleared={} focus_released={} killed={} done={} (want {:#x}/2/2/2/>=1/>=1/>0/true/true/0/1) ::",
            witness,
            spawned - spawned_before,
            joined - joined_before,
            exited - exited_before,
            parks,
            presents,
            injected,
            cleared,
            focus_released,
            killed,
            WINX7_DONE.load(Ordering::Acquire),
            WINX7_WITNESS_ALL
        );
    }
}

// =============================================================================================
// WINX-8: the VUG.ELF end-to-end witness — the shipped 3D demo off the DATA volume, through the real
// loader, into a compositor window, then killed. The WINX-2 shape applied to the artifact this arc
// exists to deliver.
//
// Like WINX-2 it needs a mounted FAT volume carrying the artifact, which the headless `./arroyo test`
// does not attach — so in CI it skips with one honest line naming the volume it looked at, and it
// proves itself at the bench. What it adds over WINX-2 is that VUG.ELF exercises the WINX-7 verbs, so
// a successful run is the statement that the ring-3 stubs in `crates/user-vug` and the kernel handlers
// in this file agree about all ten numbers — the one thing the inline fixture above cannot say,
// because it and the kernel were written in the same file.
// =============================================================================================

/// WINX-8: how many presents VUG.ELF must land before we accept that it is really running its frame
/// loop rather than having created a window and wedged at its first barrier. A vug presents once per
/// frame and its frames are fast, so three is well under a second of its life — and, critically, a
/// program whose two thread spawns were refused would still reach a present (the VUGGUARD inline
/// raster), so this is a liveness gate rather than a threading one.
const WINX8_MIN_PRESENTS: u64 = 3;

/// WINX-8: load `VUG.ELF` off the mounted FAT volume and run the whole `bg` lifecycle on it.
fn winx8_launcher(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let Ok(fs) = crate::fs::fat::mount() else {
        serial_println!(":: WINX-8: no FAT volume on the block device — VUG.ELF end-to-end witness skipped ::");
        return;
    };
    let Ok(de) = fs.find_in_root("VUG.ELF") else {
        // The same volume-naming the WINX-2 skip carries, and for the same reason: on x86 the mounted
        // volume is the USB mass-storage device, never the UEFI boot volume, and staging into
        // `target/x86_64_esp/` alone puts the artifact where the running kernel has no path to it.
        serial_println!(
            ":: WINX-8: VUG.ELF absent from the mounted DATA volume (the USB mass-storage device on storage_slot — NOT the UEFI boot volume, which the kernel cannot read) — stage target/x86_64_data/ onto it; end-to-end witness skipped ::"
        );
        return;
    };
    let cap = user_window_size();
    if de.size == 0 || de.size as usize > cap {
        serial_println!(
            ":: WINX-8: VUG.ELF is {} bytes, outside the {}-byte EL0 window — witness skipped ::",
            de.size, cap
        );
        return;
    }
    let mut bytes = alloc::vec![0u8; de.size as usize];
    if fs.read_file(&de, &mut bytes, cap).is_err() {
        serial_println!(":: WINX-8: VUG.ELF read failed — end-to-end witness skipped ::");
        return;
    }
    serial_println!(
        ":: WINX-8: VUG.ELF end-to-end — {} bytes off the DATA volume, through the ELF loader into a compositor window ::",
        bytes.len()
    );

    let presents_before = fb_present_count();
    let (spawned_before, _, _) = thread_stats();
    let (pid, slot, entry) = match spawn_user_image_bg(&bytes) {
        Ok(v) => v,
        Err(why) => {
            serial_println!(":: WINX-8: VUG.ELF end-to-end FAIL — bg spawn rejected: {} ::", why);
            return;
        }
    };
    let slot = slot as usize;

    let deadline = crate::arch::ticks() + 5_000;
    let mut windowed = false;
    let mut presents = 0u64;
    while crate::arch::ticks() < deadline {
        windowed |= winx_slot_has_window(slot);
        presents = fb_present_count() - presents_before;
        if windowed && presents >= WINX8_MIN_PRESENTS {
            break;
        }
        crate::arch::sched::yield_now();
    }
    let threads = thread_stats().0 - spawned_before;

    // The kill IS part of the contract for a `bg` vug: it was launched DETACHED, so it skips its frame
    // cap and tumbles until it is killed — which is the whole point of publishing that flag.
    let verdict = bg_kill(pid, slot as u64);
    let killed = verdict == "killed";
    let reaped = matches!(bg_poll(pid, true), BgPoll::Faulted | BgPoll::Exited(_));
    let gone = matches!(bg_poll(pid, false), BgPoll::Gone);
    let tdeadline = crate::arch::ticks() + 2_000;
    while winx_slot_has_window(slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = !winx_slot_has_window(slot);

    for _ in 0..64 {
        crate::arch::sched::yield_now();
    }

    if windowed && presents >= WINX8_MIN_PRESENTS && killed && reaped && gone && cleared {
        serial_println!(
            ":: WINX-8: VUG.ELF end-to-end — loaded (entry {:#x}) + windowed + {} presents with {} EL0 thread(s), killed + reaped, teardown clean -> PASS ::",
            entry, presents, threads
        );
    } else {
        serial_println!(
            ":: WINX-8: VUG.ELF end-to-end FAIL — windowed={} presents={} threads={} killed={} ({}) reaped={} gone={} cleared={} (want true/>={}/-/true/true/true/true) ::",
            windowed, presents, threads, killed, verdict, reaped, gone, cleared, WINX8_MIN_PRESENTS
        );
    }
}

/// WINX-1: does address-space slot `s` still own a window table row? The teardown probe.
fn winx_slot_has_window(s: usize) -> bool {
    let _irq = IrqGuard::mask_save();
    let t = WINDOWS.lock();
    (0..WIN_MAX).any(|i| t[i].owner == s)
}

// =============================================================================
// SOCK-2: the ring-3 UDP round-trip fixture + its launcher (x86-only, knob-on). The fixture is an
// INLINE, position-independent blob (the u9x/u11x idiom — NOT an on-disk bin) that makes the four new
// socket syscalls from ring 3 and proves an end-to-end datagram round-trip: it opens a UDP socket,
// binds a local port, sends a DNS query to slirp's resolver (10.0.2.3:53), and recvfroms the reply,
// conveying a 5-bit witness bitmask as its exit status. The launcher gates on a NIC (network-, not
// storage-gated), spawns the fixture, awaits its witness + socket teardown, and prints the verdict.
// =============================================================================

// SOCK-2 ring-3 fixture. Register + inline-data only (its one user store is the recvfrom dest in its own
// data page). Runs correctly at any VA (RIP-relative). Witness bits (accumulated in r12, exit status):
//   bit0 socket ok · bit1 bind ok · bit2 sendto ok · bit3 recvfrom returned a datagram · bit4 it came
//   FROM 10.0.2.3:53. ALL = 0x1F. It LOOPS sendto+recvfrom (up to 16 rounds) so a reply stolen by the
//   BSP's hand-rolled `service_net` poll (the fixture runs on an AP) is retried — each recvfrom's own
//   IF-masked bounded pump is where a fresh reply is captured. The callee-saved r12-r15 survive syscalls
//   (the C dispatcher preserves them; the sysret tail scrubs only rdi/rsi/rdx/r8-r10).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
core::arch::global_asm!(
    r#"
    .globl unaos_user_sock2_blob_start
unaos_user_sock2_blob_start:
    .balign 16
    .globl unaos_user_sock2
unaos_user_sock2:
    xor r12, r12                              // witness = 0
    mov rax, 40                               // SYS_SOCKET(AF_INET=2, SOCK_DGRAM=2, proto=0)
    mov rdi, 2
    mov rsi, 2
    mov rdx, 0
    syscall
    test rax, rax
    js 8f                                     // socket failed (<0) -> exit witness=0
    mov r13, rax                              // r13 = socket handle
    or r12, 1                                 // bit0: socket ok
    mov rax, 41                               // SYS_BIND(handle, local port 49222)
    mov rdi, r13
    mov rsi, 49222
    syscall
    test rax, rax
    jnz 8f                                    // bind != 0 -> exit with current witness
    or r12, 2                                 // bit1: bind ok
    lea r14, [rip + unaos_user_sock2_blob_start]
    add r14, 0x1000                           // r14 -> this blob's data page (recv buffer)
    mov r15, 16                               // outer sendto+recvfrom retry budget
2:  mov rax, 42                               // SYS_SENDTO(handle, msg_ptr, 32)
    mov rdi, r13
    lea rsi, [rip + unaos_user_sock2_msg]
    mov rdx, 32
    syscall
    test rax, rax
    js 3f                                     // sendto -EAGAIN -> retry this round
    or r12, 4                                 // bit2: sendto succeeded
    mov rax, 43                               // SYS_RECVFROM(handle, buf, 64)
    mov rdi, r13
    mov rsi, r14
    mov rdx, 64
    syscall
    cmp rax, 8
    jge 4f                                    // >= header -> got a datagram
3:  dec r15
    jnz 2b
    jmp 8f                                    // budget exhausted -> exit (no bit3/4)
4:  or r12, 8                                 // bit3: recvfrom returned a datagram
    cmp byte ptr [r14 + 0], 10                // verify src ip 10.0.2.3 ...
    jne 8f
    cmp byte ptr [r14 + 1], 0
    jne 8f
    cmp byte ptr [r14 + 2], 2
    jne 8f
    cmp byte ptr [r14 + 3], 3
    jne 8f
    cmp byte ptr [r14 + 4], 53                // ... and src port 53 (LE low byte)
    jne 8f
    cmp byte ptr [r14 + 5], 0                 // (LE high byte)
    jne 8f
    or r12, 16                                // bit4: source is 10.0.2.3:53
8:  mov rax, 2                                // SYS_EXIT(witness)
    mov rdi, r12
    syscall
1:  jmp 1b                                    // sys_exit never returns; guard

    // SYS_SENDTO message: 8-byte addr header [dst ip 10.0.2.3][dst port 53 LE][pad u16], then the
    // 24-byte DNS A-query for "una.os" (txn id 0x5343). Inline in the RX code page (a legal SENDTO
    // source — the whole range is inside the ring-3 window). Total = 32 bytes (matches `mov rdx, 32`).
    .balign 8
    .globl unaos_user_sock2_msg
unaos_user_sock2_msg:
    .byte 10, 0, 2, 3
    .byte 53, 0
    .byte 0, 0
    .byte 0x53, 0x43, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    .byte 0x03, 0x75, 0x6e, 0x61, 0x02, 0x6f, 0x73, 0x00
    .byte 0x00, 0x01, 0x00, 0x01

    .globl unaos_user_sock2_blob_end
unaos_user_sock2_blob_end:
"#
);

#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
unsafe extern "C" {
    static unaos_user_sock2_blob_start: u8;
    static unaos_user_sock2_blob_end: u8;
    static unaos_user_sock2: u8;
}

/// The ring-3 fixture's witness bitmask (its exit status), routed by name in `SYS_EXIT`. `SOCK2_DONE`
/// gates the launcher's read; `SOCK2_KILLED` counts a (bug) fault-kill of the well-behaved fixture.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK2_WITNESS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK2_DONE: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK2_KILLED: AtomicU32 = AtomicU32::new(0);
/// All five witness bits set = the full ring-3 UDP round-trip landed.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SOCK2_WITNESS_ALL: u32 = 0x1F;

/// Build the SOCK-2 fixture slot (the `u10x_build` shape): allocate a private slot, scrub the whole
/// window, copy the blob into its RX-RO code page through the identity alias, return the run params.
/// `None` on slot-alloc failure. No pre-endowment (the fixture mints its own socket handle at runtime).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sock2_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_sock2_blob_start as usize;
    let bend = &raw const unaos_user_sock2_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "SOCK-2 blob does not fit in a code page");
    let off = (&raw const unaos_user_sock2 as usize - bstart) as u64;
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

/// SOCK-2 launcher + verdict — chained LAST off `u8x_launcher` (after the whole storage chain u9x drives),
/// so its line lands after every other demo in BOTH storage and no-storage modes. Flow: one-shot; skip
/// silently with no NIC (NETWORK-gated, not storage-gated); pre-build the persistent smolnet stack on THIS
/// task's stack (so the fixture's first `sys_socket` never triggers the ~4 KiB build on its own 16 KiB
/// syscall stack); build + spawn the `sock2-udp` fixture; wait (bounded) for its witness exit + socket
/// teardown (handle row clear ⇒ the persistent socket + its static buffers reclaimed). PASS iff witness ==
/// `SOCK2_WITNESS_ALL` (the full round-trip) AND torn down AND no kill.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sock2_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // NETWORK-gated: no NIC -> no smolnet stack -> skip cleanly (the no-NIC control path stays line-free).
    if crate::drivers::e1000::hw_addr().is_none() {
        return;
    }
    // Pre-build the persistent stack here (large-stack launcher task), so the ring-3 fixture's first
    // sys_socket finds it ready and never pays the construction transient on its own syscall stack.
    if !crate::smolnet::init() {
        return;
    }
    let Some(fix) = sock2_build() else {
        serial_println!(":: SOCK-2: no free address-space slot — udp round-trip demo skipped ::");
        return;
    };
    serial_println!(
        ":: SOCK-2: ring-3 udp sockets — sys_socket(40)/bind(41)/sendto(42)/recvfrom(43), a datagram round-trip over the persistent smoltcp stack ::"
    );
    crate::arch::sched::spawn_user_in_space("sock2-udp", fix.entry, fix.sp, demo_cpu, fix.cr3);

    // Wait (bounded, yielding) for the fixture's witness exit, then snapshot the witness + kill count.
    let vdeadline = crate::arch::ticks() + 10_000;
    while SOCK2_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = SOCK2_WITNESS.load(Ordering::Acquire);
    let killed = SOCK2_KILLED.load(Ordering::Acquire);

    // Teardown proof: the fixture exited holding one Socket handle, so its exit cleared the handle row —
    // and `clear_handle_row` freed the persistent socket + its static buffers. Poll bounded; false->true.
    let tdeadline = crate::arch::ticks() + 2000;
    while !handle_row_is_clear(fix.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = handle_row_is_clear(fix.slot);

    if witness == SOCK2_WITNESS_ALL && cleared && killed == 0 {
        serial_println!(
            ":: SOCK-2: ring-3 udp round-trip — socket/bind/sendto OK, recvfrom returned a datagram FROM 10.0.2.3:53, socket teardown clean -> PASS ::"
        );
    } else {
        serial_println!(
            ":: SOCK-2: ring-3 udp round-trip FAIL — witness={:#x} cleared={} killed={} done={} (want {:#x}/true/0/1) ::",
            witness,
            cleared,
            killed,
            SOCK2_DONE.load(Ordering::Acquire),
            SOCK2_WITNESS_ALL
        );
    }
}

// =============================================================================
// SOCK-3: the ring-3 TCP round-trip fixture + its launcher (x86-only, knob-on). The fixture is an
// INLINE, position-independent blob (the sock2 idiom — NOT an on-disk bin) that makes the three new
// TCP syscalls from ring 3 and proves an end-to-end BYTE-STREAM round-trip: it opens a TCP socket,
// active-opens to slirp's resolver (10.0.2.3:53) POLLING connect until ESTABLISHED, sends a
// DNS-over-TCP query, and recvs the reply, conveying a 5-bit witness bitmask as its exit status. The
// launcher gates on a NIC, spawns the fixture, awaits its witness + socket teardown, and prints the verdict.
// =============================================================================

// SOCK-3 ring-3 fixture. Register + inline-data only (its one user store is the recv dest in its own
// data page). Runs correctly at any VA (RIP-relative). Witness bits (accumulated in r12, exit status):
//   bit0 socket ok · bit1 connect ESTABLISHED · bit2 send ok · bit3 recv returned stream bytes ·
//   bit4 the reply is a real (>= 12-byte) DNS-over-TCP frame. ALL = 0x1F. connect POLLS on -EINPROGRESS
//   and recv POLLS on -EAGAIN (both bounded) — the non-blocking ring-3 model. Callee-saved r12-r15
//   survive syscalls (the C dispatcher preserves them; the sysret tail scrubs only rdi/rsi/rdx/r8-r10).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
core::arch::global_asm!(
    r#"
    .globl unaos_user_sock3_blob_start
unaos_user_sock3_blob_start:
    .balign 16
    .globl unaos_user_sock3
unaos_user_sock3:
    xor r12, r12                              // witness = 0
    mov rax, 40                               // SYS_SOCKET(AF_INET=2, SOCK_STREAM=1, proto=0)
    mov rdi, 2
    mov rsi, 1
    mov rdx, 0
    syscall
    test rax, rax
    js 8f                                     // socket failed (<0) -> exit witness=0
    mov r13, rax                              // r13 = socket handle
    or r12, 1                                 // bit0: socket ok
    lea r14, [rip + unaos_user_sock3_blob_start]
    add r14, 0x1000                           // r14 -> this blob's data page (recv buffer, writable)
    mov r15, 32                               // connect poll budget (-EINPROGRESS retries)
9:  mov rax, 44                               // SYS_CONNECT(handle, msg_ptr=[ip][port], 8)
    mov rdi, r13
    lea rsi, [rip + unaos_user_sock3_msg]
    mov rdx, 8
    syscall
    test rax, rax
    jz 5f                                     // 0 -> ESTABLISHED
    cmp rax, -115                             // -EINPROGRESS -> keep polling
    jne 8f                                    // any other negative -> exit (no bit1)
    dec r15
    jnz 9b
    jmp 8f                                    // connect budget exhausted -> exit
5:  or r12, 2                                 // bit1: connect established
    mov rax, 45                               // SYS_SEND(handle, query, 26)
    mov rdi, r13
    lea rsi, [rip + unaos_user_sock3_query]
    mov rdx, 26
    syscall
    cmp rax, 1
    jl 8f                                     // send <= 0 -> exit (no bit2)
    or r12, 4                                 // bit2: send ok
    mov r15, 64                               // recv poll budget (-EAGAIN retries)
6:  mov rax, 46                               // SYS_RECV(handle, buf, 64)
    mov rdi, r13
    mov rsi, r14
    mov rdx, 64
    syscall
    test rax, rax
    jg 7f                                     // > 0 -> got stream bytes
    cmp rax, -11                              // -EAGAIN -> keep polling
    jne 8f                                    // 0 (EOF) or other error -> exit (no bit3)
    dec r15
    jnz 6b
    jmp 8f                                    // recv budget exhausted -> exit
7:  or r12, 8                                 // bit3: recv returned stream bytes
    cmp rax, 12
    jl 8f                                     // < 12 bytes -> not a full DNS-over-TCP reply
    or r12, 16                                // bit4: a real DNS-over-TCP frame came back
8:  mov rax, 2                                // SYS_EXIT(witness)
    mov rdi, r12
    syscall
1:  jmp 1b                                    // sys_exit never returns; guard

    // SYS_CONNECT address header: [dst ip 10.0.2.3][dst port 53 LE][pad u16] = 8 bytes.
    .balign 8
    .globl unaos_user_sock3_msg
unaos_user_sock3_msg:
    .byte 10, 0, 2, 3
    .byte 53, 0
    .byte 0, 0
    // SYS_SEND payload: DNS-over-TCP query = [len BE u16 = 24][24-byte DNS A-query for "una.os"] = 26 bytes.
    .balign 8
    .globl unaos_user_sock3_query
unaos_user_sock3_query:
    .byte 0x00, 0x18
    .byte 0x53, 0x43, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    .byte 0x03, 0x75, 0x6e, 0x61, 0x02, 0x6f, 0x73, 0x00
    .byte 0x00, 0x01, 0x00, 0x01

    .globl unaos_user_sock3_blob_end
unaos_user_sock3_blob_end:
"#
);

#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
unsafe extern "C" {
    static unaos_user_sock3_blob_start: u8;
    static unaos_user_sock3_blob_end: u8;
    static unaos_user_sock3: u8;
}

/// The ring-3 fixture's witness bitmask (its exit status), routed by name in `SYS_EXIT`. `SOCK3_DONE`
/// gates the launcher's read; `SOCK3_KILLED` counts a (bug) fault-kill of the well-behaved fixture.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK3_WITNESS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK3_DONE: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK3_KILLED: AtomicU32 = AtomicU32::new(0);
/// All five witness bits set = the full ring-3 TCP round-trip landed.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SOCK3_WITNESS_ALL: u32 = 0x1F;

/// Build the SOCK-3 fixture slot (the `sock2_build` shape): allocate a private slot, scrub the whole
/// window, copy the blob into its RX-RO code page through the identity alias, return the run params.
/// `None` on slot-alloc failure. No pre-endowment (the fixture mints its own socket handle at runtime).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sock3_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_sock3_blob_start as usize;
    let bend = &raw const unaos_user_sock3_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "SOCK-3 blob does not fit in a code page");
    let off = (&raw const unaos_user_sock3 as usize - bstart) as u64;
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

/// SOCK-3 launcher + verdict — chained LAST off `sock2_launcher` (so its line lands after the whole
/// storage + UDP-socket chain). Flow: one-shot; skip silently with no NIC (NETWORK-gated); the persistent
/// smolnet stack is already built (SOCK-2's launcher pre-built it on this task's stack); build + spawn the
/// `sock3-tcp` fixture; wait (bounded) for its witness exit + socket teardown (handle row clear ⇒ the
/// persistent TCP socket + its static buffers reclaimed). PASS iff witness == `SOCK3_WITNESS_ALL` (the full
/// round-trip) AND torn down AND no kill.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sock3_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // NETWORK-gated: no NIC -> no smolnet stack -> skip cleanly (the no-NIC control path stays line-free).
    if crate::drivers::e1000::hw_addr().is_none() {
        return;
    }
    // The persistent stack is already built by SOCK-2's launcher (which ran just before this); ensure it
    // regardless so SOCK-3 is self-contained if the chain order ever changes.
    if !crate::smolnet::init() {
        return;
    }
    let Some(fix) = sock3_build() else {
        serial_println!(":: SOCK-3: no free address-space slot — tcp round-trip demo skipped ::");
        return;
    };
    serial_println!(
        ":: SOCK-3: ring-3 tcp sockets — sys_socket(SOCK_STREAM)/connect(44)/send(45)/recv(46), a byte-stream round-trip over the persistent smoltcp stack ::"
    );
    crate::arch::sched::spawn_user_in_space("sock3-tcp", fix.entry, fix.sp, demo_cpu, fix.cr3);

    // Wait (bounded, yielding) for the fixture's witness exit, then snapshot the witness + kill count.
    let vdeadline = crate::arch::ticks() + 10_000;
    while SOCK3_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = SOCK3_WITNESS.load(Ordering::Acquire);
    let killed = SOCK3_KILLED.load(Ordering::Acquire);

    // Teardown proof: the fixture exited holding one Socket handle, so its exit cleared the handle row —
    // and `clear_handle_row` freed the persistent TCP socket + its static buffers. Poll bounded; false->true.
    let tdeadline = crate::arch::ticks() + 2000;
    while !handle_row_is_clear(fix.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = handle_row_is_clear(fix.slot);

    if witness == SOCK3_WITNESS_ALL && cleared && killed == 0 {
        serial_println!(
            ":: SOCK-3: ring-3 tcp round-trip — socket/connect/send OK, recv returned a byte stream FROM 10.0.2.3:53, socket teardown clean -> PASS ::"
        );
    } else {
        serial_println!(
            ":: SOCK-3: ring-3 tcp round-trip FAIL — witness={:#x} cleared={} killed={} done={} (want {:#x}/true/0/1) ::",
            witness,
            cleared,
            killed,
            SOCK3_DONE.load(Ordering::Acquire),
            SOCK3_WITNESS_ALL
        );
    }
}

// =============================================================================
// SOCK-4 (scope B): TRANSFERABLE sockets — the two-fixture ring-3 demo + the kernel-side gen-rebind proof
// (x86-only, knob-on). A socket is now minted with `CAP_GRANT` (see `sys_socket`), so its OWNER may hand it
// to another principal via `SYS_XFER`; the receiving row RECVs it, and `sys_recv`'s `xfer_socket_migrate`
// MOVES the persistent socket's registry ownership to the grantee (so `sock_valid` — owner-scoped —
// resolves for it). The gen fence (SOCK-3) closes the recycled-slot UAF: a stale cross-row handle to a
// freed+reused slot is `-EACCES`, never a rebind.
//
// TWO fixtures in one blob (the U7x idiom), run in two separate slots (only the entry differs). Both are
// register + inline-data only, apart from the GRANTEE's stores to its OWN RW pages (the recvfrom buffer at
// window +0x1000 and the USED word at +0x3008). The GO words at +0x3000 in each slot are written ONLY by
// the launcher (through the slot backing); the fixtures poll them. SEQUENCING mirrors U7x (x86 ring 3 is
// IF-masked/cooperative, no yield syscall): each fixture runs on its OWN dedicated AP, the launcher on a
// third, and the polls are bounded spins (an exhausted budget falls through to the witness exit — the
// verdict FAILs honestly rather than wedging the core).
//
// GRANTOR (`sock4-grantor`, pre-endowed at idx 2 with a Child handle naming the grantee): (0) mint a UDP
// socket (carries CAP_READ|CAP_WRITE|CAP_GRANT); (1) an OVER-RIGHTS SYS_XFER (req = +CAP_EXEC, a bit the
// socket lacks) must be -EACCES — cross-process attenuation intact; (2) the REAL SYS_XFER (req =
// CAP_READ|CAP_WRITE, dropping CAP_GRANT — single-level) deposits the socket cap; then it spins on its GO
// (released only after the grantee has RECV'd + used the socket, so migration provably happened); (3) a
// SYS_SENDTO through its OWN handle must now be -EACCES — the socket MOVED to the grantee, so the grantor's
// handle is owner-mismatched (the cross-row stale-handle rejection). Witness (r12, exit status) ALL = 0xF.
// GRANTEE (`sock4-grantee`, row EMPTY at spawn — the single-writer snapshot depends on it): spins on its GO
// (released only after the launcher's pending-deposit/untouched-row snapshot), then (0) SYS_RECV -> the
// transferred socket handle; (1) SYS_BIND it (proves the moved cap RESOLVES under the grantee's row AND
// carries CAP_WRITE); (2) SYS_SENDTO a DNS query; (3) SYS_RECVFROM the reply; (4) the reply is FROM
// 10.0.2.3:53 — a full datagram round-trip on the RECEIVED socket. Sets the USED word (the launcher's cue).
// Witness (r12) ALL = 0x1F. ABI: rax=number, args rdi/rsi/rdx, return rax; r12-r15 + rbx callee-saved.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
core::arch::global_asm!(
    r#"
    .globl unaos_user_sock4_blob_start
unaos_user_sock4_blob_start:
    .balign 16
    .globl unaos_user_sock4_grantor
unaos_user_sock4_grantor:
    xor  r12d, r12d                           // witness = 0
    mov  rax, 40                              // SYS_SOCKET(AF_INET=2, SOCK_DGRAM=2, proto=0)
    mov  rdi, 2
    mov  rsi, 2
    mov  rdx, 0
    syscall
    test rax, rax
    js   8f                                    // mint failed (<0) -> exit witness=0
    mov  r13, rax                              // r13 = socket handle (CAP_READ|CAP_WRITE|CAP_GRANT)
    or   r12, 1                                // bit0: socket minted
    mov  rax, 13                               // SYS_XFER(dest=child idx 2, src=r13, req) — OVER-RIGHTS
    mov  rdi, 2
    mov  rsi, r13
    mov  rdx, 7                                // CAP_READ|CAP_WRITE|CAP_EXEC — socket lacks CAP_EXEC -> -EACCES
    syscall
    cmp  rax, -13                              // exactly -EACCES ?
    jne  8f
    or   r12, 2                                // bit1: cross-process attenuation held
    mov  rax, 13                               // SYS_XFER(dest=2, src=r13, req) — the REAL move
    mov  rdi, 2
    mov  rsi, r13
    mov  rdx, 3                                // CAP_READ|CAP_WRITE (drops CAP_GRANT — single-level)
    syscall
    test rax, rax
    js   8f                                    // deposit failed -> partial witness
    or   r12, 4                                // bit2: socket cap deposited (transfer id >= 1)
    lea  rbx, [rip + unaos_user_sock4_blob_start]
    add  rbx, 0x3000                           // rbx = GO VA (grantor's own slot +0x3000)
    mov  r14, 0x8000000                        // bounded GO poll budget (pure loads + pause)
2:  mov  rax, [rbx]
    test rax, rax
    jnz  3f
    pause
    dec  r14
    jnz  2b
    jmp  8f                                    // GO never released -> partial witness (verdict FAILs)
3:  mov  rax, 42                               // SYS_SENDTO(h, msg, 32) — the socket MOVED away at grantee RECV
    mov  rdi, r13
    lea  rsi, [rip + unaos_user_sock4_msg]
    mov  rdx, 32
    syscall
    cmp  rax, -13                              // exactly -EACCES (owner migrated to the grantee) ?
    jne  8f
    or   r12, 8                                // bit3: migrated-away handle rejected
8:  mov  rax, 2                                // SYS_EXIT(witness) -> routed by name into SOCK4_GRANTOR_WITNESS
    mov  rdi, r12
    syscall
1:  jmp  1b                                    // sys_exit never returns; guard

    .balign 16
    .globl unaos_user_sock4_grantee
unaos_user_sock4_grantee:
    xor  r12d, r12d                            // witness = 0
    lea  rbx, [rip + unaos_user_sock4_blob_start]
    add  rbx, 0x3000                           // rbx = GO VA (USED word = rbx+8), grantee's own slot
    mov  r15, 0x8000000                        // GO poll budget
11: mov  rax, [rbx]
    test rax, rax
    jnz  12f
    pause
    dec  r15
    jnz  11b
    jmp  19f                                   // GO never released -> exit (empty witness)
12: mov  r15, 0x100000                         // RECV poll budget
13: mov  rax, 14                               // SYS_RECV -> the transferred socket handle
    syscall
    test rax, rax
    jns  14f                                    // >= 0 -> received
    pause
    dec  r15
    jnz  13b
    jmp  19f                                   // nothing ever arrived -> partial witness
14: mov  r13, rax                              // r13 = socket handle (migrated to us at RECV)
    or   r12, 1                                // bit0: received
    mov  rax, 41                               // SYS_BIND(h, 49223) — proves the moved cap resolves + CAP_WRITE
    mov  rdi, r13
    mov  rsi, 49223
    syscall
    test rax, rax
    jnz  18f                                    // bind failed -> the moved cap did NOT work
    or   r12, 2                                // bit1: bind ok (moved cap WORKS)
    lea  r14, [rip + unaos_user_sock4_blob_start]
    add  r14, 0x1000                            // r14 -> recv buffer (grantee's own RW data page)
    mov  r15, 16                                // sendto+recvfrom retry budget
15: mov  rax, 42                                // SYS_SENDTO(h, msg, 32)
    mov  rdi, r13
    lea  rsi, [rip + unaos_user_sock4_msg]
    mov  rdx, 32
    syscall
    test rax, rax
    js   16f                                    // sendto -EAGAIN -> retry this round
    or   r12, 4                                 // bit2: sendto succeeded
    mov  rax, 43                                // SYS_RECVFROM(h, buf, 64)
    mov  rdi, r13
    mov  rsi, r14
    mov  rdx, 64
    syscall
    cmp  rax, 8
    jge  17f                                    // >= header -> got a datagram
16: dec  r15
    jnz  15b
    jmp  18f                                    // budget exhausted -> USED + exit
17: or   r12, 8                                 // bit3: recvfrom returned a datagram
    cmp  byte ptr [r14 + 0], 10                 // verify src ip 10.0.2.3 ...
    jne  18f
    cmp  byte ptr [r14 + 1], 0
    jne  18f
    cmp  byte ptr [r14 + 2], 2
    jne  18f
    cmp  byte ptr [r14 + 3], 3
    jne  18f
    cmp  byte ptr [r14 + 4], 53                 // ... and src port 53 (LE low byte)
    jne  18f
    cmp  byte ptr [r14 + 5], 0                  // (LE high byte)
    jne  18f
    or   r12, 16                                // bit4: source is 10.0.2.3:53 (round-trip on the RECEIVED socket)
18: mov  qword ptr [rbx + 8], 1                 // USED word — the launcher's cue that RECV + migration happened
19: mov  rax, 2                                 // SYS_EXIT(witness) -> routed by name into SOCK4_GRANTEE_WITNESS
    mov  rdi, r12
    syscall
20: jmp  20b                                    // sys_exit never returns; guard

    // Shared SYS_SENDTO message: 8-byte addr header [dst 10.0.2.3][port 53 LE][pad u16], then the 24-byte
    // DNS A-query for "una.os" (txn id 0x5343). Inline in the RX code page (a legal SENDTO source — the
    // whole range is inside the ring-3 window). Total = 32 bytes (matches `mov rdx, 32`).
    .balign 8
    .globl unaos_user_sock4_msg
unaos_user_sock4_msg:
    .byte 10, 0, 2, 3
    .byte 53, 0
    .byte 0, 0
    .byte 0x53, 0x43, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    .byte 0x03, 0x75, 0x6e, 0x61, 0x02, 0x6f, 0x73, 0x00
    .byte 0x00, 0x01, 0x00, 0x01

    .globl unaos_user_sock4_blob_end
unaos_user_sock4_blob_end:
"#
);

#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
unsafe extern "C" {
    static unaos_user_sock4_blob_start: u8;
    static unaos_user_sock4_blob_end: u8;
    static unaos_user_sock4_grantor: u8;
    static unaos_user_sock4_grantee: u8;
}

/// The two SOCK-4 fixtures' witness bitmasks (their exit statuses), routed by name in `SYS_EXIT`.
/// `SOCK4_DONE` counts BOTH exits (want 2); `SOCK4_KILLED` counts a (bug) fault-kill of either.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK4_GRANTOR_WITNESS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK4_GRANTEE_WITNESS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK4_DONE: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static SOCK4_KILLED: AtomicU32 = AtomicU32::new(0);
/// Grantor: socket/attenuation/xfer/rejected (4 bits). Grantee: received/bind/sendto/recvfrom/source (5 bits).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SOCK4_GRANTOR_WITNESS_ALL: u32 = 0x0F;
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
const SOCK4_GRANTEE_WITNESS_ALL: u32 = 0x1F;

/// Build a SOCK-4 fixture slot at `entry_sym` (the `sock3_build`/`u7x_build` shape): allocate a private
/// slot, scrub the whole window, copy the blob into its RX-RO code page through the identity alias, return
/// the run params. `None` on slot-alloc failure. The grantor is pre-endowed (its Child handle) by the
/// launcher after build; the grantee is left EMPTY (the single-writer snapshot depends on it).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sock4_build(entry_sym: *const u8) -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_sock4_blob_start as usize;
    let bend = &raw const unaos_user_sock4_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "SOCK-4 blob does not fit in a code page");
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

/// SOCK-4 M1 — the kernel-side transferable-socket proof the two-fixture demo cannot fully stage: the
/// U11x-style stale-cross-row GEN-REBIND proof (socket edition). Drives the REAL syscall bodies
/// (`sys_xfer_from`/`sys_recv_for`) + the smolnet registry over two scratch rows (5 = grantor A, 6 = grantee
/// B — every demo fixture has exited and torn down by the time this runs, so the rows are provably clear;
/// both < USER_SLOTS, so neither is the refused `SHARED_ROW`). Returns true iff ALL hold:
///
///   1. A mints a UDP socket (owner = A) carrying CAP_READ|CAP_WRITE|CAP_GRANT; A resolves it.
///   2. A transfers it to B (attenuated to CAP_READ|CAP_WRITE); B RECVs it -> `xfer_socket_migrate` moves
///      the registry ownership to B.
///   3. THE MOVED CAP WORKS: B now resolves the socket (owner-matched under B).
///   4. A's original handle is DEAD (owner migrated to B) — the cross-row stale handle is `-EACCES`.
///   4b. THE STEAL FENCE (review fix): A's residual `CAP_GRANT` handle deposits AGAIN — to C — and the
///       deposit itself lands (handle-level rights are all `sys_xfer` checks), but the migration at C's
///       RECV is REFUSED (the sender A no longer owns the socket): C's received handle is dead and B's
///       ownership is undisturbed — a second transfer can never yank a moved socket back.
///   5. THE GEN-REBIND FENCE: B frees the socket (gen bumps), then a fresh socket REUSES the same slot at
///      the NEW generation — B's old handle (old gen) stays `-EACCES`, provably no rebind to the new tenant.
///   6. LEDGER HYGIENE: after dropping every planted handle/Proc entry, the handle rows, inboxes, transfer
///      records AND the derivation ledger are all fully clear.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sock4_kernel_check() -> bool {
    const A: usize = 5; // scratch grantor row
    const B: usize = 6; // scratch grantee row
    const PIDB: u64 = 0xE4; // planted grantee pid (never collides: PROCS holds only planted entries now)
    let mut ok = true;

    if !crate::smolnet::init() {
        return false; // no NIC / stack — the demo already skipped, so this never runs then
    }
    // Plant B's Proc entry (the pid->slot map `sys_xfer` resolves through; `proc_reserve` marks it RUNNING).
    let Some(pb) = proc_reserve() else {
        return false;
    };
    PROCS[pb].slot.store(B + 1, Ordering::Release); // +1-biased, like sys_spawn's pid->slot map
    PROCS[pb].pid.store(PIDB, Ordering::Release);

    // 1. A mints a UDP socket owned by row A, carrying the full CAP_READ|CAP_WRITE|CAP_GRANT (the sys_socket
    //    mint), and holds a Child handle naming B for the transfer.
    let Some(sid) = crate::smolnet::stack_open(A) else {
        proc_free(pb);
        return false;
    };
    let gen0 = crate::smolnet::sock_gen(sid);
    let val = ((gen0 as u64) << 32) | ((sid as u64) + 1); // the sock_id_pack value word
    install_cap(A, 2, KIND_SOCKET, val, CAP_READ | CAP_WRITE | CAP_GRANT);
    install_cap(A, 3, KIND_CHILD, PIDB, CAP_READ);
    ok &= socket_id_of(A, 2, CAP_WRITE) == Ok(sid); // A owns + resolves its socket

    // 2. Transfer to B (attenuate to CAP_READ|CAP_WRITE); B RECVs -> ownership migrates to B.
    let t = sys_xfer_from(A, 3, 2, (CAP_READ | CAP_WRITE) as u64);
    ok &= t > 0;
    let hb = sys_recv_for(B);
    ok &= hb >= 0;

    // 3. THE MOVED CAP WORKS: B resolves the socket now (owner-matched under B after migration).
    ok &= hb >= 0 && socket_id_of(B, hb as u64, CAP_WRITE) == Ok(sid);
    // 4. A's original handle is DEAD: its owner is now B, so the cross-row handle is -EACCES.
    ok &= socket_id_of(A, 2, CAP_WRITE) == Err(EACCES);

    // 4b. THE STEAL FENCE (review fix): A's handle still carries CAP_GRANT (rights are handle-local), so a
    //     SECOND deposit — to C — lands; but `xfer_socket_migrate` at C's RECV demands the sender still OWN
    //     the socket, and A doesn't (it moved to B at step 2). C's received handle must be dead and B must
    //     still resolve — the residual grantor handle can never steal the socket back from its new owner.
    const C: usize = 7; // scratch second-grantee row (< USER_SLOTS; clear like A/B — see the doc comment)
    const PIDC: u64 = 0xE5; // planted second-grantee pid (same never-collides argument as PIDB)
    let Some(pc) = proc_reserve() else {
        if hb >= 0 {
            handle_clear(B, hb as usize);
        }
        handle_clear(A, 2);
        handle_clear(A, 3);
        crate::smolnet::stack_close(sid);
        proc_free(pb);
        return false;
    };
    PROCS[pc].slot.store(C + 1, Ordering::Release);
    PROCS[pc].pid.store(PIDC, Ordering::Release);
    install_cap(A, 4, KIND_CHILD, PIDC, CAP_READ);
    let t2 = sys_xfer_from(A, 4, 2, (CAP_READ | CAP_WRITE) as u64);
    ok &= t2 > 0; // the deposit lands — sys_xfer's checks are handle-level by design
    let hc = sys_recv_for(C);
    ok &= hc >= 0; // the cap is delivered ...
    ok &= hc >= 0 && socket_id_of(C, hc as u64, CAP_WRITE) == Err(EACCES); // ... but DEAD: migration refused
    ok &= hb >= 0 && socket_id_of(B, hb as u64, CAP_WRITE) == Ok(sid); // B undisturbed — still the owner
    if hc >= 0 {
        handle_clear(C, hc as usize); // frees the second transfer's record + node before the gen-fence step
    }
    handle_clear(A, 4);
    proc_free(pc);

    // 5. THE GEN-REBIND FENCE. B frees the socket (gen bumps) — then a fresh socket first-fit-REUSES the same
    //    slot at the NEW gen. B's old handle carries (sid, gen0), so it must stay -EACCES against BOTH the
    //    freed slot and the new tenant — no rebind (the U11x fd discipline, socket edition).
    crate::smolnet::stack_close(sid);
    ok &= crate::smolnet::sock_gen(sid) != gen0; // the generation advanced on free
    ok &= hb >= 0 && socket_id_of(B, hb as u64, CAP_WRITE) == Err(EACCES); // stale against the freed slot
    let Some(sid2) = crate::smolnet::stack_open(A) else {
        // cleanup on the unlikely alloc failure, then fail
        if hb >= 0 {
            handle_clear(B, hb as usize);
        }
        handle_clear(A, 2);
        handle_clear(A, 3);
        proc_free(pb);
        return false;
    };
    ok &= sid2 == sid; // first-fit reused the freed slot — the rebind hazard is now live
    ok &= hb >= 0 && socket_id_of(B, hb as u64, CAP_WRITE) == Err(EACCES); // still -EACCES vs the NEW tenant
    crate::smolnet::stack_close(sid2);

    // 6. Drop everything planted, then demand every ledger fully clear (no record/node/slot leaked).
    if hb >= 0 {
        handle_clear(B, hb as usize); // frees the transfer record + drops the delivered cap's node
    }
    handle_clear(A, 2); // drops the transfer source's derivation root node
    handle_clear(A, 3);
    proc_free(pb);
    ok &= handle_row_is_clear(A) && handle_row_is_clear(B) && handle_row_is_clear(C);
    ok &= xfer_row_is_clear(A) && xfer_row_is_clear(B) && xfer_row_is_clear(C);
    ok &= xfer_recs_all_free() && deriv_all_free();
    ok
}

/// SOCK-4 launcher + verdict — chained LAST off `sock3_launcher` (so its line lands after the whole storage
/// + UDP/TCP-socket chain). The transferable-socket two-fixture demo (the U7x idiom) + the M1 kernel-side
/// gen-rebind proof. `demo_cpu` (the task arg) is the GRANTEE's core; the GRANTOR runs on a third AP, the
/// launcher on its own (cooperative ring 3 hogs its core while polling). Flow:
///   1. One-shot; skip silently with no NIC (NETWORK-gated). Skip with a line if fewer than 3 APs.
///   2. Plant the grantee Proc entry; build + spawn the GRANTEE (row EMPTY — it parks on its GO word);
///      publish its pid->slot map; build + pre-endow the GRANTOR (Child handle naming the grantee); spawn it.
///   3. Single-writer witness: wait for the grantor's deposit to land in the grantee's inbox, then — grantee
///      provably pre-RECV (parked on its GO) — verify its handle row is still CLEAR. Release the grantee GO.
///   4. Wait for the grantee's USED word (RECV + migration + round-trip landed), then release the grantor GO
///      — so its post-transfer SYS_SENDTO rejection is provably AFTER the ownership migration.
///   5. Verdict: wait for both witness exits, the teardown proof (both rows + inboxes clear, records free),
///      then run `sock4_kernel_check` (the M1 gen-rebind proof). PASS iff both witnesses full AND used AND
///      the snapshot held AND cleared AND the kernel check held AND no kill.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn sock4_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // NETWORK-gated: no NIC -> no smolnet stack -> skip cleanly (the no-NIC control path stays line-free).
    if crate::drivers::e1000::hw_addr().is_none() {
        return;
    }
    if !crate::smolnet::init() {
        return;
    }
    // The grantor's dedicated core: a third AP, distinct from the grantee's (`demo_cpu`) and this launcher's
    // (cooperative ring 3 hogs its core while polling, so sharing either would deadlock the sequencing).
    let online = crate::arch::smp::online_aps();
    let Some(&grantor_cpu) = online.get(2) else {
        serial_println!(":: SOCK-4: fewer than 3 application processors — transferable-socket demo skipped ::");
        return;
    };
    // Plant the grantee's Proc entry FIRST (nothing else claimed if the table is full).
    let Some(pi) = proc_reserve() else {
        serial_println!(":: SOCK-4: no free process entry — transferable-socket demo skipped ::");
        return;
    };
    // Build + spawn the GRANTEE (its handle row stays EMPTY — the single-writer snapshot depends on that; it
    // parks on its GO word, making no syscall that could populate anything).
    let Some(grantee) = sock4_build(&raw const unaos_user_sock4_grantee) else {
        serial_println!(":: SOCK-4: no free address-space slot — transferable-socket demo skipped ::");
        proc_free(pi);
        return;
    };
    let grantee_pid = crate::arch::sched::spawn_user_in_space(
        "sock4-grantee",
        grantee.entry,
        grantee.sp,
        demo_cpu,
        grantee.cr3,
    );
    PROCS[pi].slot.store(grantee.slot + 1, Ordering::Release); // slot first (the sys_spawn discipline)
    PROCS[pi].pid.store(grantee_pid, Ordering::Release); // then the pid, the live key
    // Build + pre-endow + spawn the GRANTOR (idx 2 = a Child handle naming the grantee, for SYS_XFER).
    let Some(grantor) = sock4_build(&raw const unaos_user_sock4_grantor) else {
        serial_println!(":: SOCK-4: no free address-space slot — transferable-socket demo skipped (grantee parks out) ::");
        proc_free(pi);
        return;
    };
    install_cap(grantor.slot, 2, KIND_CHILD, grantee_pid, CAP_READ);
    serial_println!(
        ":: SOCK-4: transferable sockets — SYS_XFER moves a KIND_SOCKET cap cross-row (owner migrates), the grantee round-trips it, the grantor's stale handle is rejected ::"
    );
    crate::arch::sched::spawn_user_in_space(
        "sock4-grantor",
        grantor.entry,
        grantor.sp,
        grantor_cpu,
        grantor.cr3,
    );

    // 3. Single-writer witness: the grantor's deposit is live in the grantee's inbox + the grantee's row is
    //    still untouched (it is parked on the GO word this launcher has not released).
    let ddeadline = crate::arch::ticks() + 5000;
    let mut deposit_seen = false;
    while !deposit_seen && crate::arch::ticks() < ddeadline {
        deposit_seen = (0..NXFER).any(|k| {
            let t = XFER_SLOT_TX[grantee.slot][k].load(Ordering::Acquire);
            t != 0 && t != HANDLE_RESERVING
        });
        if !deposit_seen {
            crate::arch::sched::yield_now();
        }
    }
    let snap_ok = deposit_seen && handle_row_is_clear(grantee.slot);
    u7x_release_go(grantee.slot);

    // 4. Use-then-reject sequencing: wait for the grantee's USED word (RECV + migration + round-trip done),
    //    then release the grantor GO so its post-transfer SYS_SENDTO is provably after the ownership move.
    let used_ptr =
        unsafe { crate::arch::memory::slot_backing_ptr(grantee.slot).add(U7X_USED_OFF) as *const u64 };
    let udeadline = crate::arch::ticks() + 8000;
    while unsafe { core::ptr::read_volatile(used_ptr) } == 0 && crate::arch::ticks() < udeadline {
        crate::arch::sched::yield_now();
    }
    let used = unsafe { core::ptr::read_volatile(used_ptr) };
    u7x_release_go(grantor.slot);

    // 5a. Wait (bounded) for both witness exits, then snapshot both witnesses + the kill count.
    let vdeadline = crate::arch::ticks() + 10_000;
    while SOCK4_DONE.load(Ordering::Acquire) < 2 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let gw = SOCK4_GRANTOR_WITNESS.load(Ordering::Acquire);
    let ew = SOCK4_GRANTEE_WITNESS.load(Ordering::Acquire);
    let killed = SOCK4_KILLED.load(Ordering::Acquire);

    // 5b. Teardown/leak proof: both rows + both inboxes clear and the transfer-record ledger fully FREE (the
    //     record was released when the grantee's received handle tore down). Poll bounded; false->true.
    let all_clear = |gs: usize, es: usize| {
        handle_row_is_clear(gs)
            && handle_row_is_clear(es)
            && xfer_row_is_clear(gs)
            && xfer_row_is_clear(es)
            && xfer_recs_all_free()
    };
    let tdeadline = crate::arch::ticks() + 2000;
    while !all_clear(grantor.slot, grantee.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = all_clear(grantor.slot, grantee.slot);
    proc_free(pi); // the planted pid->slot entry (the fixtures exited by name, never through the Proc path)

    // 5c. The M1 kernel-side gen-rebind proof (needs the drained ledgers the wait above establishes).
    let kernel_ok = cleared && sock4_kernel_check();

    if gw == SOCK4_GRANTOR_WITNESS_ALL
        && ew == SOCK4_GRANTEE_WITNESS_ALL
        && used != 0
        && snap_ok
        && cleared
        && kernel_ok
        && killed == 0
    {
        serial_println!(
            ":: SOCK-4: transferable sockets — grantee received + round-tripped the moved socket, grantor's migrated-away handle -EACCES, gen-rebind rejected, teardown clean -> PASS ::"
        );
    } else {
        serial_println!(
            ":: SOCK-4: transferable sockets FAIL — grantor={:#x} grantee={:#x} used={} snap={} cleared={} kernel={} killed={} done={} (want {:#x}/{:#x}/1/true/true/true/0/2) ::",
            gw,
            ew,
            used,
            snap_ok,
            cleared,
            kernel_ok,
            killed,
            SOCK4_DONE.load(Ordering::Acquire),
            SOCK4_GRANTOR_WITNESS_ALL,
            SOCK4_GRANTEE_WITNESS_ALL
        );
    }
}

// =============================================================================
// SINKHOLE-1 (zeolite): the ring-3 DNS resolver fixture + its launcher (x86-only, knob-on). The DNS
// SINKHOLE proof — "the Pi-hole concept, done the UnaOS way." The fixture is an INLINE,
// position-independent blob (the sock2 idiom — NOT an on-disk bin) that:
//   (0) loads a BLOCKLIST from BLOCK.TXT via the S7 dynamic-open path (ring-3 SYS_OPEN RO + SYS_READ —
//       a genuine STOR-feeds-NET composition witness), falling back to a builtin list (+ honest marker)
//       when no FAT volume is mounted;
//   (1) SELF-TEST #1 (hermetic): parses an inline DNS query for ADS.EXAMPLE, matches it against the
//       blocklist (BLOCKED), and BUILDS a well-formed 0.0.0.0 A-answer — the sinkhole DECISION + response
//       construction, proven without a peer;
//   (2) SELF-TEST #2 / FORWARD (hermetic under slirp): parses an inline query for una.os, confirms it is
//       NOT blocked, and FORWARDS it to the upstream resolver (10.0.2.3:53), relaying the real answer —
//       the forward leg;
//   (3) SERVE (needs the UNAOS_NET=socket `dns` injector): binds UDP :53, and for a bounded window
//       recvfroms an inbound query; a blocked name is sinkholed to 0.0.0.0 over the wire to the client.
// The two legs have conflicting media (slirp answers upstream but cannot inject inbound; the socket
// injector injects inbound but has no upstream resolver), so — like SOCK-6/7 — the SERVE leg reads
// PENDING under hermetic slirp and OK under the injector, while the FORWARD + composition legs prove
// hermetically. The hostile-payload DNS name parser (`zdns_parse_and_match`) is strictly bounded:
// every field access is length-checked, labels are capped, and ANY label-length byte with a high bit
// set (compression pointers 0b11 AND the reserved 0b01/0b10 forms) is REFUSED — no pointer is ever
// followed, so no compression loop is possible; any malformed packet is rejected without a crash.
// Witness bits (accumulated in r12, exit status):
//   bit0 list loaded · bit1 list from FAT (else builtin) · bit2 ADS.EXAMPLE matched blocklist ·
//   bit3 built a well-formed 0.0.0.0 answer · bit4 una.os NOT blocked (forward decision) ·
//   bit5 forwarded una.os -> got a real answer from 10.0.2.3:53 · bit6 served an inbound query on :53 ·
//   bit7 sinkholed the served query to 0.0.0.0 over the wire ·
//   bit8 (M2) a SUBDOMAIN of a blocked base name was sinkholed (label-boundary suffix match) ·
//   bit9 (M2) a near-miss (string suffix, NOT a label boundary) was correctly NOT blocked.
// The blocklist parser (M1) reads real hosts-file format (IP + domain, '#'/';' comments, blank lines);
// the match (M2) is label-boundary suffix (a blocked base domain sinkholes its subdomains).
// =============================================================================

#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
core::arch::global_asm!(
    r#"
    .globl unaos_user_zeolite_blob_start
unaos_user_zeolite_blob_start:
    .balign 16
    .globl unaos_user_zeolite
unaos_user_zeolite:
    cld
    xor r12, r12                              // witness = 0
    lea r14, [rip + unaos_user_zeolite_blob_start]
    add r14, 0x1000                           // r14 -> data page base (window page 1, RW)

    // --- (0) load the blocklist: SYS_OPEN("BLOCK.TXT", RO) + SYS_READ (the S7 dynamic-open path) ---
    mov rax, 11                               // SYS_OPEN
    lea rdi, [rip + zdns_blockname]
    mov rsi, [rip + zdns_blocknamelen]
    xor rdx, rdx                              // mode 0 = read-only
    syscall
    test rax, rax
    js zdns_builtin                           // open failed (no FAT / absent) -> builtin list
    mov r13, rax                              // r13 = file handle
    mov rax, 12                               // SYS_READ(handle, FILEBUF, 2048)
    mov rdi, r13
    lea rsi, [r14 + 0x000]                    // FILEBUF (data page)
    mov rdx, 2048
    syscall
    test rax, rax
    jle zdns_close_builtin                    // empty/failed read -> close + builtin
    mov [r14 + 0xD00], rax                    // file_len = bytes read
    mov rax, 17                               // SYS_CLOSE(handle) — resolver keeps only its sockets
    mov rdi, r13
    syscall
    or r12, 1                                 // bit0: list loaded
    or r12, 2                                 // bit1: from the FAT BLOCK.TXT
    jmp zdns_have_list
zdns_close_builtin:
    mov rax, 17                               // SYS_CLOSE(handle)
    mov rdi, r13
    syscall
zdns_builtin:
    // Plant the builtin fallback blocklist into FILEBUF (uppercase, LF-separated).
    lea rsi, [rip + zdns_builtinlist]
    mov rdx, [rip + zdns_builtinlen]
    lea rdi, [r14 + 0x000]
    mov rcx, rdx
    rep movsb
    mov [r14 + 0xD00], rdx                    // file_len = builtin length
    or r12, 1                                 // bit0: list loaded (builtin; bit1 stays clear)
zdns_have_list:

    // --- (1) SELF-TEST #1: parse+match the inline ADS.EXAMPLE query -> BLOCKED + build 0.0.0.0 answer ---
    lea rsi, [rip + zdns_blockedquery]
    mov rcx, [rip + zdns_blockedquerylen]
    call zdns_parse_and_match                 // rax: 0 ok/not-blocked, 1 blocked, 2 malformed
    inc qword ptr [r14 + 0xD10]               // metric: queries seen++ (ADS.EXAMPLE)
    cmp rax, 1
    jne zdns_st2                              // not blocked (unexpected) -> skip bit2/3
    or r12, 4                                 // bit2: ADS.EXAMPLE matched the blocklist
    inc qword ptr [r14 + 0xD18]               // metric: blocked (sinkholed)++
    lea rsi, [rip + zdns_blockedquery]
    mov rcx, [rip + zdns_blockedquerylen]
    lea rdi, [r14 + 0x1008]                   // RESPBUF DNS area (dst-addr hdr occupies +0x1000..+0x1008)
    call zdns_build_sinkhole                  // rax = response DNS length
    // Self-check the built answer: QR bit set + RDATA (last 4 bytes) == 0.0.0.0.
    test byte ptr [r14 + 0x100A], 0x80        // DNS flags byte (offset 2 of the response) — QR
    jz zdns_st2
    lea r9, [r14 + 0x1008]
    add r9, rax
    sub r9, 4                                 // -> RDATA
    cmp dword ptr [r9], 0
    jne zdns_st2
    or r12, 8                                 // bit3: built a well-formed 0.0.0.0 answer

zdns_m2:
    // --- (1b) M2 SELF-TESTS: label-boundary suffix matching ---
    // A subdomain of a blocked base name MUST be sinkholed; a mere-string near-miss MUST NOT.
    lea rsi, [rip + zdns_subquery]            // WWW.ADS.EXAMPLE -> expect BLOCKED (subdomain)
    mov rcx, [rip + zdns_subquerylen]
    call zdns_parse_and_match
    inc qword ptr [r14 + 0xD10]               // metric: queries seen++ (WWW.ADS.EXAMPLE)
    cmp rax, 1
    jne zdns_m2_near
    or r12, 256                               // bit8: subdomain of a blocked base was sinkholed
    inc qword ptr [r14 + 0xD18]               // metric: blocked (sinkholed)++
zdns_m2_near:
    lea rsi, [rip + zdns_nearquery]           // NOTADS.EXAMPLE -> expect NOT blocked (not a boundary)
    mov rcx, [rip + zdns_nearquerylen]
    call zdns_parse_and_match
    inc qword ptr [r14 + 0xD10]               // metric: queries seen++ (NOTADS.EXAMPLE, allowed)
    test rax, rax                             // 0 = not blocked (the correct, safe answer)
    jnz zdns_st2                              // blocked/malformed -> over-block bug, skip bit9
    or r12, 512                               // bit9: near-miss correctly NOT blocked (no over-block)

zdns_st2:
    // --- (2) SELF-TEST #2 / FORWARD: parse+match una.os -> NOT blocked, forward upstream 10.0.2.3:53 ---
    lea rsi, [rip + zdns_realquery]
    mov rcx, [rip + zdns_realquerylen]
    call zdns_parse_and_match
    inc qword ptr [r14 + 0xD10]               // metric: queries seen++ (una.os)
    test rax, rax
    jnz zdns_serve                            // blocked(1) / malformed(2) -> unexpected, skip forward
    or r12, 16                                // bit4: una.os NOT blocked (forward decision)
    mov rax, 40                               // SYS_SOCKET(AF_INET=2, SOCK_DGRAM=2, 0)
    mov rdi, 2
    mov rsi, 2
    xor rdx, rdx
    syscall
    test rax, rax
    js zdns_serve
    mov r13, rax                              // r13 = upstream socket
    mov rax, 41                               // SYS_BIND(handle, 49260)
    mov rdi, r13
    mov rsi, 49260
    syscall
    mov r15, 6                                // forward retry budget (slirp answers round 1; keep short so
                                              // a medium with no upstream — the socket injector — fails fast
                                              // and the SERVE leg starts promptly, overlapping the injector)
zdns_fwd_loop:
    mov rax, 42                               // SYS_SENDTO(handle, msg, len)
    mov rdi, r13
    lea rsi, [rip + zdns_fwdmsg]              // [10.0.2.3][53 LE][pad] + una.os DNS payload
    mov rdx, [rip + zdns_fwdmsglen]
    syscall
    test rax, rax
    js zdns_fwd_retry
    mov rax, 43                               // SYS_RECVFROM(handle, UPRECV, 512)
    mov rdi, r13
    lea rsi, [r14 + 0x1400]                   // UPRECV
    mov rdx, 512
    syscall
    cmp rax, 8                                // >= 8-byte src header -> a datagram landed
    jl zdns_fwd_retry
    cmp byte ptr [r14 + 0x1400], 10           // src ip 10.0.2.3 ...
    jne zdns_fwd_retry
    cmp byte ptr [r14 + 0x1401], 0
    jne zdns_fwd_retry
    cmp byte ptr [r14 + 0x1402], 2
    jne zdns_fwd_retry
    cmp byte ptr [r14 + 0x1403], 3
    jne zdns_fwd_retry
    cmp byte ptr [r14 + 0x1404], 53           // ... src port 53 (LE low byte)
    jne zdns_fwd_retry
    cmp byte ptr [r14 + 0x1405], 0
    jne zdns_fwd_retry
    or r12, 32                                // bit5: forwarded + real answer relayed from upstream
    inc qword ptr [r14 + 0xD20]               // metric: forwarded upstream++ (relayed)
    jmp zdns_fwd_done
zdns_fwd_retry:
    dec r15
    jnz zdns_fwd_loop
zdns_fwd_done:
    mov rax, 17                               // SYS_CLOSE(upstream socket)
    mov rdi, r13
    syscall

zdns_serve:
    // --- (3) SERVE: bind UDP :53, recvfrom a bounded window; a blocked name -> 0.0.0.0 to the client ---
    mov rax, 40                               // SYS_SOCKET(AF_INET, SOCK_DGRAM, 0)
    mov rdi, 2
    mov rsi, 2
    xor rdx, rdx
    syscall
    test rax, rax
    js zdns_exit
    mov r13, rax                              // r13 = serve socket
    mov rax, 41                               // SYS_BIND(handle, 53)
    mov rdi, r13
    mov rsi, 53
    syscall
    test rax, rax
    jnz zdns_exit                             // bind :53 failed
    mov r15, 24                               // serve round budget: a bounded window for an inbound query.
                                              // Under the injector a query is caught + sinkholed on the
                                              // first round (fast exit); hermetically all rounds run to their
                                              // -EAGAIN budget then the fixture exits (the PENDING path).
zdns_serve_loop:
    mov rax, 43                               // SYS_RECVFROM(serve, RECVBUF, 512)
    mov rdi, r13
    lea rsi, [r14 + 0x800]                    // RECVBUF = [8-byte src hdr][DNS query]
    mov rdx, 512
    syscall
    cmp rax, 20                               // >= 8 hdr + 12 DNS header -> a plausible query
    jl zdns_serve_next
    or r12, 64                                // bit6: served an inbound datagram on :53
    inc qword ptr [r14 + 0xD10]               // metric: queries seen++ (an over-the-wire query)
    mov rcx, rax
    sub rcx, 8                                // DNS message length
    mov [r14 + 0xD08], rcx                    // stash it (parse clobbers rcx)
    lea rsi, [r14 + 0x808]                    // DNS message start (past the 8-byte src header)
    call zdns_parse_and_match
    cmp rax, 1
    jne zdns_serve_next                       // not blocked / malformed -> keep serving (no sinkhole)
    // Blocked: build the 0.0.0.0 response and send it to the client (client addr = RECVBUF[0..8]).
    mov rax, [r14 + 0x800]                    // copy the 8-byte client src header -> RESPBUF dst header
    mov [r14 + 0x1000], rax
    lea rsi, [r14 + 0x808]                    // query DNS
    mov rcx, [r14 + 0xD08]                    // its length
    lea rdi, [r14 + 0x1008]                   // response DNS area
    call zdns_build_sinkhole                  // rax = response DNS length
    add rax, 8                                // + the 8-byte dst header
    mov rdx, rax
    mov rax, 42                               // SYS_SENDTO(serve, RESPBUF, 8+resp)
    mov rdi, r13
    lea rsi, [r14 + 0x1000]
    syscall
    or r12, 128                               // bit7: sinkholed the served query over the wire (latched)
    inc qword ptr [r14 + 0xD18]               // metric: blocked (sinkholed)++
    jmp zdns_serve_next                        // keep serving across the window (answer every blocked query),
                                              // so an over-the-wire injector reliably rendezvouses with an answer
zdns_serve_next:
    dec r15
    jnz zdns_serve_loop
zdns_exit:
    // --- (4) METRICS (M3): pack the three counters into the spare high bits of the witness word so the
    // launcher can print them (the honest source a future stats view reads) — no new syscall. Each count
    // saturates at 63: seen -> bits[10:16], blocked -> bits[16:22], forwarded -> bits[22:28].
    mov rax, [r14 + 0xD10]                    // queries seen
    cmp rax, 63
    jbe zdns_pk_seen
    mov rax, 63
zdns_pk_seen:
    shl rax, 10
    or r12, rax
    mov rax, [r14 + 0xD18]                    // blocked (sinkholed)
    cmp rax, 63
    jbe zdns_pk_blk
    mov rax, 63
zdns_pk_blk:
    shl rax, 16
    or r12, rax
    mov rax, [r14 + 0xD20]                    // forwarded upstream
    cmp rax, 63
    jbe zdns_pk_fwd
    mov rax, 63
zdns_pk_fwd:
    shl rax, 22
    or r12, rax
    mov rax, 2                                // SYS_EXIT(witness) -> routed by name into ZEOLITE_WITNESS
    mov rdi, r12
    syscall
zdns_hang:
    jmp zdns_hang                             // sys_exit never returns; guard

// --- zdns_parse_and_match: hostile-payload-hardened DNS question-name parse + blocklist match ---
//   in:  rsi = DNS message start, rcx = message length; r14 = data base
//   out: rax = 1 blocked, 0 valid/not-blocked, 2 malformed. Fills NAMEBUF (r14+0xC00) with the
//        uppercase dotted name, length in r11. Preserves r12/r13/r14/r15.
//   Every field access is bounds-checked against the packet end; a label-length byte with EITHER
//   high bit set is refused (compression pointers AND reserved forms) so no pointer is followed and
//   no compression loop can exist; the assembled name is capped well under NAMEBUF.
zdns_parse_and_match:
    cmp rcx, 17                               // header(12) + root(1) + qtype(2) + qclass(2) minimum
    jb zdns_malformed
    lea rdi, [rsi + 12]                       // cursor = past the 12-byte header
    lea r9, [rsi + rcx]                       // r9 = packet end
    lea r10, [r14 + 0xC00]                    // NAMEBUF
    xor r11, r11                              // assembled name length = 0
zdns_lbl_loop:
    cmp rdi, r9
    jae zdns_malformed                        // no room to read a length byte
    movzx rax, byte ptr [rdi]
    inc rdi
    test al, al
    jz zdns_name_done                         // 0x00 -> end of name
    test al, 0xC0                             // high bit(s) set -> pointer/reserved -> REFUSE
    jnz zdns_malformed
    // valid label length 1..63 in al. Cap the assembled name (with a dot) well under NAMEBUF.
    mov r8, r11
    add r8, rax
    cmp r8, 250
    ja zdns_malformed
    mov r8, rdi
    add r8, rax                               // label must fit inside the packet
    cmp r8, r9
    ja zdns_malformed
    test r11, r11
    jz zdns_no_dot
    mov byte ptr [r10 + r11], 0x2E            // '.' separator between labels
    inc r11
zdns_no_dot:
    mov rcx, rax                              // copy `al` bytes, uppercased
zdns_copy_lbl:
    mov dl, byte ptr [rdi]
    cmp dl, 0x61                              // 'a'
    jb zdns_noup
    cmp dl, 0x7A                              // 'z'
    ja zdns_noup
    sub dl, 0x20
zdns_noup:
    mov byte ptr [r10 + r11], dl
    inc rdi
    inc r11
    dec rcx
    jnz zdns_copy_lbl
    jmp zdns_lbl_loop
zdns_name_done:
    // Match NAMEBUF[0..r11] (uppercase) against a hosts-format blocklist in FILEBUF (ZEOLITE-2 M1).
    // Real sinkhole lists ship in hosts form: "<ip> <domain>  [# comment]", plus '#'/';' comment lines
    // and blank lines. Per line: skip leading whitespace (SP/TAB); drop blank + '#'/';' comment lines;
    // the DOMAIN is field-2 when a second whitespace-delimited field exists (the "0.0.0.0 domain" form),
    // else field-1 (bare "domain"); the domain is compared to NAMEBUF case-insensitively. Every byte
    // access is bounds-checked against fend (r9) — a hostile BLOCK.TXT can never read past file_len or
    // crash; a malformed line simply matches nothing and is skipped. Preserves r10 (NAMEBUF), r11 (len).
    test r11, r11
    jz zdns_not_blocked                       // empty (root) name matches nothing
    lea r8, [r14 + 0x000]                     // r8 = line cursor, = FILEBUF
    mov rax, [r14 + 0xD00]                    // file_len
    lea r9, [r14 + rax]                       // r9 = fend = FILEBUF + file_len
zdns_line_start:
    cmp r8, r9
    jae zdns_not_blocked
zdns_skip_ws1:                                // skip leading whitespace (SP/TAB)
    cmp r8, r9
    jae zdns_not_blocked
    mov al, byte ptr [r8]
    cmp al, 0x20
    je zdns_ws1_inc
    cmp al, 0x09
    je zdns_ws1_inc
    jmp zdns_after_ws1
zdns_ws1_inc:
    inc r8
    jmp zdns_skip_ws1
zdns_after_ws1:
    cmp al, 0x0A                              // blank line (EOL right away) -> next line
    je zdns_eol_adv
    cmp al, 0x0D
    je zdns_eol_adv
    cmp al, 0x23                              // '#' comment line
    je zdns_toeol
    cmp al, 0x3B                              // ';' comment line
    je zdns_toeol
    // FIELD1: [rsi, rdi) = a run of non-whitespace, non-comment, non-EOL bytes.
    mov rsi, r8                               // field1 start
    mov rdi, r8                               // scan cursor
zdns_f1_scan:
    cmp rdi, r9
    jae zdns_f1_end
    mov al, byte ptr [rdi]
    cmp al, 0x20
    je zdns_f1_end
    cmp al, 0x09
    je zdns_f1_end
    cmp al, 0x0A
    je zdns_f1_end
    cmp al, 0x0D
    je zdns_f1_end
    cmp al, 0x23
    je zdns_f1_end
    cmp al, 0x3B
    je zdns_f1_end
    inc rdi
    jmp zdns_f1_scan
zdns_f1_end:
    // Default domain = field1 [rsi, rdi). Look for a second field after whitespace.
    mov rdx, rsi                              // rdx = domain start (default field1)
    mov rcx, rdi                              // rcx = domain end   (default field1)
    mov r8, rdi                               // advance line cursor past field1
zdns_skip_ws2:
    cmp r8, r9
    jae zdns_have_domain                      // EOF after field1 -> bare-name form
    mov al, byte ptr [r8]
    cmp al, 0x20
    je zdns_ws2_inc
    cmp al, 0x09
    je zdns_ws2_inc
    jmp zdns_after_ws2
zdns_ws2_inc:
    inc r8
    jmp zdns_skip_ws2
zdns_after_ws2:
    cmp al, 0x0A                              // EOL/comment after field1 -> no field2 (bare-name form)
    je zdns_have_domain
    cmp al, 0x0D
    je zdns_have_domain
    cmp al, 0x23
    je zdns_have_domain
    cmp al, 0x3B
    je zdns_have_domain
    // There IS a field2 -> the DOMAIN is field2 (the "0.0.0.0 domain" hosts form).
    mov rdx, r8                               // domain start = field2 start
    mov rdi, r8
zdns_f2_scan:
    cmp rdi, r9
    jae zdns_f2_end
    mov al, byte ptr [rdi]
    cmp al, 0x20
    je zdns_f2_end
    cmp al, 0x09
    je zdns_f2_end
    cmp al, 0x0A
    je zdns_f2_end
    cmp al, 0x0D
    je zdns_f2_end
    cmp al, 0x23
    je zdns_f2_end
    cmp al, 0x3B
    je zdns_f2_end
    inc rdi
    jmp zdns_f2_scan
zdns_f2_end:
    mov rcx, rdi                              // domain end = field2 end
    mov r8, rdi                               // line cursor past field2
zdns_have_domain:
    // Domain field = [rdx, rcx). BLOCK if it equals NAMEBUF, OR is a label-boundary SUFFIX of it —
    // i.e. NAMEBUF ends with "." + domain (ZEOLITE-2 M2). So a blocked base domain (ads.example)
    // sinkholes its subdomains (www.ads.example) but NOT a mere string suffix (notads.example — the
    // char before the tail must be a dot). Compare is against the TAIL of NAMEBUF, case-insensitive.
    mov rax, rcx
    sub rax, rdx                              // rax = domain length L
    test rax, rax
    jz zdns_toeol                             // empty domain field -> matches nothing
    cmp rax, r11
    ja zdns_toeol                             // domain longer than the queried name -> cannot match
    mov rdi, r11
    sub rdi, rax                              // rdi = offset = r11 - L (tail start in NAMEBUF)
    test rdi, rdi
    jz zdns_dom_cmp                           // offset 0 -> exact-length case (no boundary dot needed)
    mov sil, byte ptr [r10 + rdi - 1]         // label-boundary guard: char before the tail must be '.'
    cmp sil, 0x2E
    jne zdns_toeol                            // suffix not on a label boundary -> no match
zdns_dom_cmp:
    cmp rdx, rcx
    jae zdns_blocked                          // whole domain matched at the tail -> BLOCKED
    mov al, byte ptr [rdx]
    cmp al, 0x61                              // upper-case the file side
    jb zdns_dcu
    cmp al, 0x7A
    ja zdns_dcu
    sub al, 0x20
zdns_dcu:
    cmp al, byte ptr [r10 + rdi]
    jne zdns_toeol                            // mismatch -> skip to EOL, next line
    inc rdx
    inc rdi
    jmp zdns_dom_cmp
zdns_toeol:                                   // scan r8 to end-of-line, then advance past the EOL bytes
    cmp r8, r9
    jae zdns_not_blocked
    mov al, byte ptr [r8]
    cmp al, 0x0A
    je zdns_eol_adv
    cmp al, 0x0D
    je zdns_eol_adv
    inc r8
    jmp zdns_toeol
zdns_eol_adv:
    cmp r8, r9
    jae zdns_not_blocked
    mov al, byte ptr [r8]
    cmp al, 0x0A
    je zdns_eol_adv_inc
    cmp al, 0x0D
    je zdns_eol_adv_inc
    jmp zdns_line_start
zdns_eol_adv_inc:
    inc r8
    jmp zdns_eol_adv
zdns_blocked:
    mov rax, 1
    ret
zdns_not_blocked:
    xor rax, rax
    ret
zdns_malformed:
    mov rax, 2
    ret

// --- zdns_build_sinkhole: build a DNS response answering the query with a single A-record 0.0.0.0 ---
//   in:  rsi = query DNS start, rcx = query DNS length, rdi = destination DNS buffer
//   out: rax = response DNS length. Preserves r12/r13/r14/r15.
//   The response = the query bytes (header + question) with QR/RA set, ANCOUNT=1, plus a 16-byte
//   answer (name pointer 0xC00C, TYPE A, CLASS IN, TTL 0, RDLENGTH 4, RDATA 0.0.0.0).
zdns_build_sinkhole:
    mov r8, rdi                               // dest base
    mov r9, rcx                               // query length
    rep movsb                                 // copy the query (header+question) verbatim
    or byte ptr [r8 + 2], 0x80                // QR = 1 (response)
    mov byte ptr [r8 + 3], 0x80               // RA = 1, RCODE = 0
    mov byte ptr [r8 + 6], 0x00               // ANCOUNT hi
    mov byte ptr [r8 + 7], 0x01               // ANCOUNT lo = 1
    mov rax, r8
    add rax, r9                               // -> the appended answer
    mov byte ptr [rax + 0], 0xC0              // name = pointer to offset 12 (the qname)
    mov byte ptr [rax + 1], 0x0C
    mov byte ptr [rax + 2], 0x00              // TYPE = A
    mov byte ptr [rax + 3], 0x01
    mov byte ptr [rax + 4], 0x00              // CLASS = IN
    mov byte ptr [rax + 5], 0x01
    mov dword ptr [rax + 6], 0x00000000       // TTL = 0
    mov byte ptr [rax + 10], 0x00             // RDLENGTH hi
    mov byte ptr [rax + 11], 0x04             // RDLENGTH lo = 4
    mov dword ptr [rax + 12], 0x00000000      // RDATA = 0.0.0.0
    mov rax, r9
    add rax, 16                               // response length = query length + 16-byte answer
    ret

    .balign 8
zdns_blocknamelen:
    .quad zdns_blockname_end - zdns_blockname
zdns_blockname:
    .ascii "BLOCK.TXT"
zdns_blockname_end:
    .balign 8
zdns_builtinlen:
    .quad zdns_builtinlist_end - zdns_builtinlist
zdns_builtinlist:
    // Builtin fallback in the SAME hosts-file format as BLOCK.TXT (lowercase domains — the parser
    // upper-cases the file side, so this also exercises case-insensitive ingest).
    // (Leading newline is deliberate: a hash immediately after the opening quote would read as the
    // enclosing Rust raw-string terminator. The blank first line is skipped by the parser.)
    .ascii "\n# zeolite builtin blocklist (hosts format)\n0.0.0.0 ads.example\n0.0.0.0 track.example\n"
zdns_builtinlist_end:
    // Inline DNS query for ADS.EXAMPLE (header + QNAME + QTYPE A + QCLASS IN). txn id 0x5A44 = "ZD".
    .balign 8
zdns_blockedquerylen:
    .quad zdns_blockedquery_end - zdns_blockedquery
zdns_blockedquery:
    .byte 0x5A, 0x44, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    .byte 0x03, 0x41, 0x44, 0x53, 0x07, 0x45, 0x58, 0x41, 0x4D, 0x50, 0x4C, 0x45, 0x00
    .byte 0x00, 0x01, 0x00, 0x01
zdns_blockedquery_end:
    // Inline DNS query for "una.os" (the forward self-test name — deliberately NOT in the blocklist).
    .balign 8
zdns_realquerylen:
    .quad zdns_realquery_end - zdns_realquery
zdns_realquery:
    .byte 0x5A, 0x45, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    .byte 0x03, 0x75, 0x6E, 0x61, 0x02, 0x6F, 0x73, 0x00
    .byte 0x00, 0x01, 0x00, 0x01
zdns_realquery_end:
    // M2 self-test A: inline query for "www.ads.example" — a SUBDOMAIN of the blocked base ads.example;
    // must be sinkholed by the label-boundary suffix rule. (labels: www / ads / example)
    .balign 8
zdns_subquerylen:
    .quad zdns_subquery_end - zdns_subquery
zdns_subquery:
    .byte 0x5A, 0x53, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    .byte 0x03, 0x77, 0x77, 0x77, 0x03, 0x61, 0x64, 0x73, 0x07, 0x65, 0x78, 0x41, 0x4D, 0x50, 0x4C, 0x45, 0x00
    .byte 0x00, 0x01, 0x00, 0x01
zdns_subquery_end:
    // M2 self-test B: inline query for "notads.example" — shares the string suffix "ads.example" but NOT
    // on a label boundary; must NOT be blocked (guards the suffix rule against a naive substring bug).
    .balign 8
zdns_nearquerylen:
    .quad zdns_nearquery_end - zdns_nearquery
zdns_nearquery:
    .byte 0x5A, 0x4E, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    .byte 0x06, 0x6E, 0x6F, 0x74, 0x61, 0x64, 0x73, 0x07, 0x65, 0x78, 0x61, 0x6D, 0x70, 0x6C, 0x65, 0x00
    .byte 0x00, 0x01, 0x00, 0x01
zdns_nearquery_end:
    // SYS_SENDTO message for the upstream forward: 8-byte addr header [10.0.2.3][53 LE][pad] + the
    // una.os DNS payload (the same 24 bytes as zdns_realquery).
    .balign 8
zdns_fwdmsglen:
    .quad zdns_fwdmsg_end - zdns_fwdmsg
zdns_fwdmsg:
    .byte 10, 0, 2, 3
    .byte 53, 0
    .byte 0, 0
    .byte 0x5A, 0x45, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    .byte 0x03, 0x75, 0x6E, 0x61, 0x02, 0x6F, 0x73, 0x00
    .byte 0x00, 0x01, 0x00, 0x01
zdns_fwdmsg_end:
    .globl unaos_user_zeolite_blob_end
unaos_user_zeolite_blob_end:
"#
);

#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
unsafe extern "C" {
    static unaos_user_zeolite_blob_start: u8;
    static unaos_user_zeolite_blob_end: u8;
    static unaos_user_zeolite: u8;
}

/// The zeolite resolver fixture's witness bitmask (its exit status), routed by name in `SYS_EXIT`.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static ZEOLITE_WITNESS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static ZEOLITE_DONE: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
static ZEOLITE_KILLED: AtomicU32 = AtomicU32::new(0);

/// Build the zeolite fixture slot (the sock2 shape): allocate a private slot, scrub the whole window,
/// copy the blob into its RX-RO code page. `None` on slot-alloc failure.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn zeolite_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_zeolite_blob_start as usize;
    let bend = &raw const unaos_user_zeolite_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "zeolite blob does not fit in a code page");
    let off = (&raw const unaos_user_zeolite as usize - bstart) as u64;
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

/// SINKHOLE-1 (zeolite) launcher + verdict — chained LAST off `sock4_launcher`, so its lines land after
/// every other demo. Flow: one-shot; skip silently with no NIC; pre-build the persistent smolnet stack
/// on this large-stack task; build + spawn the `zeolite-resolver` fixture; wait (bounded) for its witness
/// exit + socket teardown; print two verdict lines — the composition/forward leg (proves hermetically)
/// and the sinkhole-serve leg (OK under the `dns` injector, PENDING under hermetic slirp).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn zeolite_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::e1000::hw_addr().is_none() {
        return; // NETWORK-gated: no NIC -> skip cleanly, line-free
    }
    if !crate::smolnet::init() {
        return;
    }
    let Some(fix) = zeolite_build() else {
        serial_println!(":: zeolite: no free address-space slot — DNS sinkhole demo skipped ::");
        return;
    };
    serial_println!(
        ":: zeolite: DNS sinkhole (the Pi-hole concept, UnaOS way) — ring-3 resolver binds :53, blocks from a list, forwards the rest ::"
    );
    crate::arch::sched::spawn_user_in_space("zeolite-resolver", fix.entry, fix.sp, demo_cpu, fix.cr3);

    let vdeadline = crate::arch::ticks() + 40_000;
    while ZEOLITE_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let w = ZEOLITE_WITNESS.load(Ordering::Acquire);
    let killed = ZEOLITE_KILLED.load(Ordering::Acquire);
    let tdeadline = crate::arch::ticks() + 2000;
    while !handle_row_is_clear(fix.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = handle_row_is_clear(fix.slot);

    let list_loaded = w & 1 != 0;
    let from_fat = w & 2 != 0;
    let blocked_ok = w & 4 != 0 && w & 8 != 0;
    let fwd_decided = w & 16 != 0;
    let fwd_relayed = w & 32 != 0;
    let served = w & 64 != 0 && w & 128 != 0;
    let subdomain_ok = w & 256 != 0; // M2: a subdomain of a blocked base was sinkholed
    let nearmiss_ok = w & 512 != 0; // M2: a near-miss (not a label boundary) was NOT over-blocked
    let suffix_ok = subdomain_ok && nearmiss_ok;
    // M3 metrics: the resolver's own counters, packed into the witness word's spare high bits (saturating
    // 63). The honest data source a future stats view/widget reads — query counts, blocked counts.
    let seen_ct = (w >> 10) & 0x3F;
    let blocked_ct = (w >> 16) & 0x3F;
    let forwarded_ct = (w >> 22) & 0x3F;
    let list_src = if from_fat { "BLOCK.TXT via S7 dynamic-open" } else { "builtin list (no FAT)" };

    // Leg 1: the STOR-feeds-NET composition + hosts-format ingest + suffix-match + hostile-parse + forward.
    if list_loaded && blocked_ok && suffix_ok && fwd_decided && fwd_relayed && killed == 0 {
        serial_println!(
            ":: zeolite: hosts-format blocklist from {}, blocked ADS.EXAMPLE -> 0.0.0.0 (answer built), subdomain WWW.ADS.EXAMPLE sinkholed + NOTADS.EXAMPLE not over-blocked, forwarded una.os -> 10.0.2.3:53 real answer relayed — witness OK ::",
            list_src
        );
    } else if list_loaded && blocked_ok && suffix_ok && fwd_decided && !fwd_relayed && killed == 0 {
        // The forward leg's upstream is unreachable on this medium (the UNAOS_NET=socket injector has
        // no slirp resolver) — the sinkhole DECISION + build proved; the upstream relay is INCOMPLETE here.
        serial_println!(
            ":: zeolite: blocklist from {}, blocked ADS.EXAMPLE -> 0.0.0.0 (answer built), una.os NOT blocked -> forwarded to 10.0.2.3:53 (no upstream on this medium) — witness INCOMPLETE ::",
            list_src
        );
    } else {
        serial_println!(
            ":: zeolite: DNS sinkhole FAIL — witness={:#x} (list={} fat={} blocked+built={} subdomain={} nearmiss_ok={} fwd_decided={} fwd_relayed={}) cleared={} killed={} done={} ::",
            w, list_loaded, from_fat, blocked_ok, subdomain_ok, nearmiss_ok, fwd_decided, fwd_relayed, cleared, killed,
            ZEOLITE_DONE.load(Ordering::Acquire)
        );
    }

    // Metrics (M3): the resolver's own tally — the honest source a future stats view reads.
    serial_println!(
        ":: zeolite: metrics — {} queries seen, {} blocked (sinkholed), {} forwarded upstream ::",
        seen_ct, blocked_ct, forwarded_ct
    );

    // Leg 2: the over-the-wire sinkhole serve — needs the UNAOS_NET=socket `dns` injector.
    if served {
        serial_println!(
            ":: zeolite: served an inbound query on :53 — blocked name sinkholed to 0.0.0.0 over the wire — witness OK ::"
        );
    } else {
        serial_println!(
            ":: zeolite: resolver bound :53 — awaiting an inbound query (UNAOS_NET=socket net-inject dns) — witness PENDING ::"
        );
    }
}

/// U8x launcher + verdict — called by `u7x_launcher` after the whole U7x flow (program-order gating; see
/// `u7x_launcher`). Flow: one-shot guard; skip silently with no block device (the control-path discipline —
/// U8x needs no disk); build + pre-endow + spawn the single fixture (`u8x-tree`: index 2 = a console cap
/// WITH `CAP_REVOKE`, index 3 = one WITHOUT); wait (bounded) for its witness exit; wait (bounded) for its
/// teardown (row clear + the derivation ledger drained — the tombstone-cascade proof); run the kernel-side
/// cross-process checks (which need the clear ledgers); PASS iff witness == `U8X_WITNESS_ALL` AND torn down
/// AND no kill AND the kernel checks held. Then it chains `u9x_launcher` (the File-WRITE demo) in program
/// order — the same one-task chaining `u7x_launcher` uses for `u8x_launcher`, so U9x's lines land after the
/// U8x verdict and its slot has freed.
fn u8x_launcher(demo_cpu: usize) {
    // One-shot (the U7x launcher is spawned once; guard defensively anyway).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // U8x runs REGARDLESS of a block device (its fixture is an inline console-cap blob — no disk), so the
    // revocation-tree rung is visible on the no-storage / metal path. (Scoped relaxation — U5x/U7x/U8x only.)
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

    // Chain the U9x File-WRITE demo in program order (the `u7x_launcher` -> `u8x_launcher` idiom): every U8x
    // exit path above falls through to here, and the U8x fixture's slot has torn down (its verdict waited on
    // teardown), so a slot is free for U9x. U9x gates on the block device itself, so the no-storage control
    // path stays free of U9x lines too.
    u9x_launcher(demo_cpu);

    // WINX-1: chain the ring-3 WINDOW demo after the storage chain and BEFORE the network-gated SOCK
    // demos, so its line lands in a stable position whether or not a NIC is present and whether or not
    // `smolnet` is compiled. Unconditional (no feature gate): the window verbs are core process surface,
    // and `video::wm` is arch-neutral and always compiled. aarch64 never reaches this launcher at all.
    winx_launcher(demo_cpu);

    // WINX-6: chain the STAT.ELF end-to-end witness right after the inline-fixture one, so the isolated
    // verb proof lands first and the shipping-artifact proof second. It gates on the boot volume
    // internally, so a run with no FAT volume (or no staged STAT.ELF) skips cleanly with one honest line.
    winx2_launcher(demo_cpu);

    // WINX-6b: the headless ELF-loader witness. WINX-2 above needs a block device the headless x86 run
    // does not have, so this one synthesizes a real multi-segment ELF64 in memory and pushes it through
    // the same `spawn_user_image_bg`, keeping the loader proven in CI rather than only at the bench.
    winx3_launcher(demo_cpu);

    // WINX-7: the threads + futex + input fixture, after the loader witness so the machinery it
    // builds on is proved first. Unconditional and headless-complete — it needs no block device and
    // no panel, only the scheduler.
    winx7_launcher(demo_cpu);

    // WINX-8: the VUG.ELF end-to-end witness. Gates on the mounted volume internally, so a run with no
    // FAT volume (or no staged VUG.ELF) skips cleanly with one honest line naming the volume.
    winx8_launcher(demo_cpu);

    // SOCK-2 (knob-on, x86-only): chain the ring-3 UDP round-trip demo LAST — after the whole storage
    // chain u9x drives (so its line lands after every other demo, in both storage and no-storage modes,
    // since U8x is storage-independent). It gates on a NIC internally, so no-NIC configs skip cleanly.
    // Knob-off / aarch64 never emit this line, so U8x is byte-identical there.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    sock2_launcher(demo_cpu);
    // SOCK-3 (knob-on, x86-only): chain the ring-3 TCP round-trip demo after SOCK-2, so its line lands
    // last. It gates on a NIC internally, so no-NIC configs skip cleanly. Knob-off / aarch64 never emit it.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    sock3_launcher(demo_cpu);
    // SOCK-4 (knob-on, x86-only): chain the transferable-socket two-fixture demo + the M1 kernel-side
    // gen-rebind proof after SOCK-3, so its line lands last. It gates on a NIC + a third AP internally, so
    // no-NIC / low-AP configs skip cleanly. Knob-off / aarch64 never emit it.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    sock4_launcher(demo_cpu);
    // SINKHOLE-1 (zeolite, knob-on, x86-only): chain the ring-3 DNS resolver / sinkhole demo LAST, so its
    // lines land after every other demo. It gates on a NIC internally, so no-NIC configs skip cleanly.
    // Knob-off / aarch64 never emit its lines.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    zeolite_launcher(demo_cpu);
}

/// Build the U9x fixture slot — the `u8x_build` shape for the U9x blob (allocate, scrub the WHOLE window, copy
/// the blob into its RX-RO code page through the identity alias, return the run params). `None` if slot
/// allocation fails. Does NOT pre-endow (the launcher does, before dispatch).
fn u9x_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u9x_blob_start as usize;
    let bend = &raw const unaos_user_u9x_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U9x blob does not fit in a code page");
    let off = (&raw const unaos_user_u9x_write as usize - bstart) as u64;
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

/// The U9x kernel-side revocation + revoke-DISCARD check — staged over a scratch row (5 — every demo fixture
/// has exited and torn down by the time this runs, and `u8_kernel_check` dropped its plants, so the row is
/// provably clear). Proves TWO things:
///   (1) a U8x-revoked File-write cap is `-EACCES` — `sys_write`'s File gate is `handle_resolve(row, fd,
///       CAP_WRITE)`, so an `Err` resolve after the ancestor is revoked IS the write denial (the derivation walk);
///   (2) M2's revoke ordering — a REVOKE of a DIRTY File cap DISCARDS the staged write (never persists stale
///       bytes). The negative alone ("the flush queue is empty after the revoke") would be VACUOUS: the queue is
///       empty after ANY revoke, since only TEARDOWN (`clear_files_row`) enqueues. So a POSITIVE control first
///       proves that a dirty descriptor run through `clear_files_row` DOES register a pending flush; the
///       contrast (teardown persists, revoke discards) is the real proof.
/// Backs REAL writable descriptors (wstage-seeded, exactly what a RW `SYS_OPEN` mints). Drops everything planted
/// and demands the handle / file / writable-pool / derivation / flush ledgers all clear (no leak). The aarch64
/// `u9_check_revoked_write` twin, plus the x86-specific flush-queue discard proof (pi4 writes in-handler, no queue).
fn u9x_check_revoked_write() -> bool {
    const A: usize = 5; // scratch row (provably clear here — the fixture uses a freshly-alloc'd slot; u8_kernel_check dropped its plants)
    let mut ok = true;
    ok &= flush_all_free(); // a clean starting queue (the launcher already drained the fixture's flush, if any)
    // Back a REAL writable File descriptor: seed a writable staging slot from SCRATCH.BIN's staged content
    // (what a RW `SYS_OPEN` does), so the ROOT cap names a genuine open file a write through WOULD land in.
    let Some(seed) = staged_bytes(1) else {
        return false; // SCRATCH.BIN not staged — cannot stage a writable descriptor (an M1 kernel bug)
    };
    let dirty_lo = U9X_WRITE_OFFSET;
    let dirty_hi = U9X_WRITE_OFFSET + U9X_PATTERN.len() as u32;
    let scratch_cluster = SCRATCH_CLUSTER.load(Ordering::Acquire);

    // --- POSITIVE control (disk-backed mode only): a DIRTY descriptor run through the whole-task TEARDOWN
    //     (`clear_files_row`) DOES enqueue a flush — so the negative below discriminates instead of being
    //     vacuous. Uses SCRATCH.BIN's REAL chain head (harmless even if some path drained the probe — it would
    //     re-write the seed bytes it copied); we RESET the queue WITHOUT a disk write (drop the entry, no `write_at`).
    if scratch_cluster != 0 {
        let Some(w) = wstage_alloc(seed) else {
            return false;
        };
        let Some(fid) = files_alloc(A, 1, seed.len() as u32, (w + 1) as u32, scratch_cluster) else {
            wstage_free(w);
            return false;
        };
        mark_dirty(A, fid, dirty_lo, dirty_hi);
        clear_files_row(A); // the whole-task teardown path — enqueues the dirty descriptor + frees its wstage
        ok &= !flush_all_free(); // PROVES teardown registered a pending flush (so the negative below is meaningful)
        // Reset the queue WITHOUT persisting (no write_at to the probe cluster): drop every entry. A leftover
        // entry transitions the final `flush_all_free()` below to false -> a loud verdict FAIL, never a silent drain.
        for k in 0..NFLUSH {
            FLUSH_LEN[k].store(0, Ordering::Relaxed);
            FLUSH_USED[k].store(false, Ordering::Release);
        }
        ok &= flush_all_free() && files_row_is_clear(A) && wstage_all_free();
    }

    // --- NEGATIVE: a REVOKE of a DIRTY File cap DISCARDS the write (no enqueue) AND the derived cap goes stale.
    let Some(w) = wstage_alloc(seed) else {
        return false;
    };
    let Some(fid) = files_alloc(A, 1, seed.len() as u32, (w + 1) as u32, scratch_cluster) else {
        wstage_free(w);
        return false;
    };
    // U11x: pack the descriptor's CURRENT generation (the positive control above ran `clear_files_row(A)`, which
    // bumps every slot's gen, so slot 0's gen is non-zero here) — the revoke below decodes this file-id through
    // `file_desc_validate`, which now checks the gen, so a bare `(fid + 1)` would fail to match and the descriptor
    // would never be freed (leaking the row, failing the verdict).
    let file_id = file_id_pack(FILE_GEN[A][fid].load(Ordering::Acquire), fid);
    mark_dirty(A, fid, dirty_lo, dirty_hi); // dirty — the write the revoke must repudiate (never flush)
    // A ROOT File cap carrying CAP_WRITE|CAP_GRANT|CAP_REVOKE at index 2 (off index 0/CONSOLE_FD, the U8x idiom).
    install_cap(A, 2, KIND_FILE, file_id, CAP_WRITE | CAP_GRANT | CAP_REVOKE);
    // Pre-revoke: the root File+CAP_WRITE cap resolves — a `sys_write` through it WOULD pass the CHECK.
    ok &= matches!(handle_resolve(A, 2, CAP_WRITE), Ok(HandleTarget::File(id)) if id == file_id);
    // Derive a child File+CAP_WRITE (records the derivation edge + back-fills the root's node).
    let g = sys_cap_grant(A, 2, CAP_WRITE as u64);
    ok &= g >= 0;
    // Pre-revoke: the DERIVED File+CAP_WRITE cap also resolves — a write through it WOULD land too.
    if g >= 0 {
        ok &= matches!(handle_resolve(A, g as u64, CAP_WRITE), Ok(HandleTarget::File(_)));
    }
    // Revoke the ROOT (index 2 carries CAP_REVOKE) -> kills the derivation subtree; the revoke clears index 2
    // AND — because it is a File handle — frees the shared (DIRTY) descriptor + its writable staging slot via
    // `files_free`, which DISCARDS the dirty bytes (no flush enqueued).
    ok &= sys_cap_revoke(A, 2) == 0;
    // THE DISCARD PROOF: the revoke did NOT enqueue the dirty write — a revoked File-write cap never persists.
    ok &= flush_all_free();
    // THE DENIAL: the derived File+CAP_WRITE cap is now stale — its next CAP_WRITE resolve (exactly the CHECK
    // `sys_write` performs) is `-EACCES`. This is the "a U8x-revoked File cap write -> -EACCES" proof.
    if g >= 0 {
        ok &= handle_resolve(A, g as u64, CAP_WRITE).is_err();
    }
    // Drop everything planted; index 2 was already cleared by the revoke (freeing the descriptor + writable
    // slot), so clearing the derived handle drops the last node (the root's tombstone cascades free). Then
    // demand every ledger clear — no handle / descriptor / writable-pool / derivation node / flush entry leaked.
    if g >= 0 {
        handle_clear(A, g as usize);
    }
    ok &= handle_row_is_clear(A)
        && files_row_is_clear(A)
        && wstage_all_free()
        && deriv_all_free()
        && flush_all_free();
    ok
}

/// U9x kernel-side helper: read 16 bytes at absolute file offset `off` from a FRESH mount, via the read-only
/// offset-aware FAT reader (`read_at`, which WALKS the cluster chain — it never assumes the file's clusters are
/// contiguous). The "raw re-read" the launcher uses to prove the flushed bytes landed on disk. `None` on any
/// mount/read failure or a short read. Independent of any FILE_OFFSET sidecar (re-derives everything from a
/// fresh mount). The aarch64 `u9_read16` twin.
fn u9x_read16(fc: u32, size: u32, off: u32) -> Option<[u8; 16]> {
    let fs = crate::fs::fat::mount().ok()?;
    let mut v: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    fs.read_at(fc, size, off, &mut v, 16).ok()?;
    if v.len() < 16 {
        return None;
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(&v[..16]);
    Some(out)
}

/// U9x launcher + verdict (M2 — real disk write-back) — chained off `u8x_launcher` in program order. Flow:
/// one-shot; skip silently with no block device (the control-path discipline). PRE-FLIGHT (gated on
/// `HELLO_STAGED`, the BSP's FAT-present signal) captures SCRATCH.BIN's chain head + size + the pre-image bytes
/// at the write offset and publishes the cluster BEFORE the fixture opens; build + pre-endow the kind negative
/// (index 2 = a `Socket` carrying `CAP_WRITE`) + spawn `u9x-write`; wait (bounded) for its witness exit and
/// teardown (FILES row + writable pool + handle row clear — the teardown that ENQUEUES the dirty write). Then,
/// disk-backed and only once teardown is observed, DRAIN the flush queue to disk (`fat::write_at`, in place) and
/// RAW-RE-READ the sector to prove the bytes landed + size unchanged. Finally the kernel-side
/// revoked-cap-write denial + revoke-discard proof. PASS iff witness == `U9X_WITNESS_ALL` AND torn down AND no
/// kill AND the revoke check held AND (disk-backed) the on-disk write-back proof held.
///
/// TWO MODES: disk-backed (a FAT volume backs SCRATCH.BIN — `test-fat sf`) requires the on-disk proof; in-memory
/// (no FAT — plain `./arroyo test` attaches a non-FAT usb.img) runs the M1 core with the flush a no-op and does
/// ZERO AP disk I/O (the pre-flight is skipped, bounding the no-FAT run). METAL-CONFIRMED on the rMBP
/// (2026-07-08 bench, post xHCI-enumeration fix): on-disk write-back PASSes off a FAT16 SD card; the pre-flight
/// self-heals a prior boot's persisted pattern (see below) so re-runs stay honest. NOTE: the disk-backed flush
/// is the FIRST concurrent AP-side xHCI BOT I/O in the tree; the pump is BOUNDED (a 2000-iter timeout -> `Io`),
/// so a failure is a LOUD verdict FAIL (`flushed=false`), never a hang — `test-fat sf` is its empirical proof.
fn u9x_launcher(demo_cpu: usize) {
    // One-shot (reached once via the U8x chain; guard defensively anyway).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // No block device -> keep the no-storage control path free of demo lines (mirrors every prior gate).
    if crate::drivers::block::info().is_none() {
        return;
    }
    // U9x M2: pre-flight SCRATCH.BIN's chain head + size + the pre-image bytes at the write offset from a FRESH
    // mount at IF=1, and publish the cluster (Release) BEFORE spawning the fixture so its RW `sys_open` records
    // it as the flush target. GATED on `HELLO_STAGED` — the BSP's "a FAT volume mounted" signal (set by
    // `stage_hello` before this chain runs): no FAT -> false -> ZERO AP disk I/O, U9x runs its in-memory core.
    // The pre-flight also VALIDATES SCRATCH.BIN (a regular file, non-zero chain head, size == U9X_SCRATCH_SIZE ==
    // the staged/wstage length the flush drives `write_at` with); any mismatch drops to in-memory mode rather
    // than flushing against an inconsistent size. Best-effort — never fail the demo for a missing on-disk file.
    let (fc, pre_size, pre) = if HELLO_STAGED.load(Ordering::Acquire) {
        match crate::fs::fat::mount().ok().and_then(|fs| fs.find_in_root(U9X_SCRATCH_NAME).ok()) {
            Some(de)
                if !de.is_dir && de.first_cluster() != 0 && de.size == U9X_SCRATCH_SIZE as u32 =>
            {
                let fc = de.first_cluster();
                SCRATCH_CLUSTER.store(fc, Ordering::Release); // publish the flush target before the fixture opens
                let mut pre = u9x_read16(fc, de.size, U9X_WRITE_OFFSET);
                // Re-run self-heal (metal): a PRIOR boot's flush already persisted U9X_PATTERN to
                // this card, so `pre == pattern` and the strict "sector CHANGED" proof below could
                // never pass again (first seen at the 2026-07-08 rMBP bench, boot #3). Restore the
                // 0xEE seed in place — same write_at path, in-place, never grows — and re-read it
                // as the pre-image, making the demo idempotent across reboots on the same medium.
                // QEMU never hits this (each run starts from a fresh image). Best-effort: a failed
                // restore leaves `pre` as-is and the verdict stays honest (FAILs on sector_changed).
                if pre == Some(U9X_PATTERN) {
                    if let Ok(fs) = crate::fs::fat::mount() {
                        let seed = [U9X_SCRATCH_FILL; 16];
                        let restored = fs
                            .write_at(fc, de.size, U9X_WRITE_OFFSET, &seed)
                            .map(|w| w == seed.len())
                            .unwrap_or(false);
                        serial_println!(
                            ":: U9x: pre-flight found a prior boot's pattern on disk; seed restore {} ::",
                            if restored { "OK" } else { "FAILED" }
                        );
                        if restored {
                            pre = u9x_read16(fc, de.size, U9X_WRITE_OFFSET);
                        }
                    }
                }
                (fc, de.size, pre)
            }
            _ => (0, 0, None), // absent / a directory / the wrong size -> in-memory mode
        }
    } else {
        (0, 0, None) // no FAT volume mounted (the BSP's stage_hello failed) -> in-memory mode, no AP disk I/O
    };
    let disk_backed = fc != 0;

    let Some(fix) = u9x_build() else {
        serial_println!(":: U9x: no free address-space slot — File-write demo skipped ::");
        return;
    };
    // The kind negative: a Socket carrying CAP_WRITE at U9X_SOCK_IDX. It HAS the right, so a File `sys_write`
    // is denied purely on kind (write serves Console/File only) — the kind arm, not the rights arm. A scaffold
    // id, never dereferenced.
    install_cap(fix.slot, U9X_SOCK_IDX, KIND_SOCKET, 0x200, CAP_WRITE);
    serial_println!(
        ":: U9x: real File writes — SYS_SEEK + File+CAP_WRITE, staged in-place then flushed to FAT (out of the IF-masked handler) ::"
    );
    crate::arch::sched::spawn_user_in_space("u9x-write", fix.entry, fix.sp, demo_cpu, fix.cr3);

    // Wait (bounded, yielding) for the fixture's witness exit, then snapshot the witness.
    let vdeadline = crate::arch::ticks() + 5000;
    while U9X_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U9X_WITNESS.load(Ordering::Acquire);
    let killed = U9X_KILLED.load(Ordering::Acquire);

    // Teardown proof: the fixture exited holding two live descriptors (its RW + RO opens; the RW one owns a
    // writable staging slot) and the pre-endowed Socket handle, so its exit cleared BOTH the FILES row (with
    // its writable slot) and the handle row. The RW descriptor was DIRTY, so its teardown also ENQUEUED a flush
    // (drained below). Poll bounded; false->true.
    let tdeadline = crate::arch::ticks() + 2000;
    while !(files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot) && wstage_all_free())
        && crate::arch::ticks() < tdeadline
    {
        crate::arch::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot) && wstage_all_free();

    // U9x M2: FLUSH the staged write to disk + prove it LANDED — disk-backed mode, and ONLY once teardown is
    // observed (`cleared` is the Acquire edge that makes the enqueue's stores visible; draining before it could
    // race a late enqueue and strand it — so on a teardown timeout we do NOT drain, and the verdict FAILs on
    // `cleared`). Drain every pending entry via `fat::write_at` (in place, never grows), then raw-re-read the
    // sector: it must now equal the pattern, DIFFER from the pre-image (a real change), and the directory size
    // must be UNCHANGED. A drain I/O timeout (the AP could not drive the xHCI BOT pump) fails `flushed` — bounded,
    // never a hang. `flushed` also demands the queue drained empty and no entry was dropped on a full queue.
    let (flushed, sector_changed, size_unchanged) = if disk_backed && cleared {
        match crate::fs::fat::mount() {
            Ok(fs) => {
                // S3 (irqstorage): with the write routed THROUGH the storage service task, the fixture's
                // SYS_WRITE persisted SYNCHRONOUSLY in-syscall — so the disk ALREADY holds the pattern
                // BEFORE any drain, and nothing was ever enqueued (FILE_DIRTY never set). This is the
                // close-discards-dirty residual RETIRED: a SYS_CLOSE / teardown of the written descriptor
                // loses nothing, because the write is on the volume, not in a staged buffer awaiting flush.
                #[cfg(feature = "irqstorage")]
                {
                    let pre_drain = u9x_read16(fc, pre_size, U9X_WRITE_OFFSET);
                    let no_defer = flush_all_free() && !FLUSH_OVERFLOW.load(Ordering::Acquire);
                    let sync_ok = pre_drain == Some(U9X_PATTERN) && pre != Some(U9X_PATTERN) && no_defer;
                    serial_println!(
                        ":: S3: synchronous write-through — SYS_WRITE persisted to FAT in-syscall (disk held the write pre-drain: {}, no deferred flush: {}), close-discards-dirty residual retired -> {} ::",
                        pre_drain == Some(U9X_PATTERN),
                        no_defer,
                        if sync_ok { "PASS" } else { "FAIL" }
                    );
                }
                let mut flushed = true;
                while let Some(one) = flush_drain_one(&fs) {
                    flushed &= one;
                }
                flushed &= flush_all_free() && !FLUSH_OVERFLOW.load(Ordering::Acquire);
                let now = u9x_read16(fc, pre_size, U9X_WRITE_OFFSET);
                let post_size = fs.find_in_root(U9X_SCRATCH_NAME).ok().map(|de| de.size);
                let sector_changed = now == Some(U9X_PATTERN) && pre != Some(U9X_PATTERN);
                let size_unchanged =
                    post_size == Some(pre_size) && post_size == Some(U9X_SCRATCH_SIZE as u32);
                (flushed, sector_changed, size_unchanged)
            }
            Err(_) => (false, false, false),
        }
    } else {
        (false, false, false) // in-memory mode (flush a no-op) OR !cleared (the verdict fails on `cleared`)
    };

    // Kernel-side revoked-File-write denial + revoke-discard proof (needs a clear scratch row — every fixture
    // has torn down by here, and the launcher's own flush drained above, so the flush queue starts clean).
    let revoke_ok = u9x_check_revoked_write();

    // Verdict: the M1 CORE (witness + torn down + no kill + revoke check) in every mode; PLUS, in disk-backed
    // mode, the on-disk write-back proof (flushed + sector changed + size unchanged). In-memory mode states so.
    let core_ok = witness == U9X_WITNESS_ALL && cleared && killed == 0 && revoke_ok;
    let pass = core_ok && (!disk_backed || (flushed && sector_changed && size_unchanged));
    if pass {
        if disk_backed {
            serial_println!(
                ":: U9x: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES, staged write FLUSHED to FAT (on-disk sector changed + size unchanged) -> PASS ::"
            );
        } else {
            serial_println!(
                ":: U9x: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES (in-memory core; no FAT volume, flush is a no-op) -> PASS ::"
            );
        }
    } else {
        serial_println!(
            ":: U9x: real File writes FAIL — disk_backed={} witness={:#x} cleared={} killed={} revoke={} flushed={} sector_changed={} size_unchanged={} done={} (want disk_backed?ALL/true/0/true/true/true/true : ALL/true/0/true) ::",
            disk_backed,
            witness,
            cleared,
            killed,
            revoke_ok,
            flushed,
            sector_changed,
            size_unchanged,
            U9X_DONE.load(Ordering::Acquire),
        );
    }

    // Chain the U10 file-growth demo in program order (the u7x -> u8x -> u9x idiom): every U9x exit path above
    // falls through to here, and the U9x fixture's slot has torn down (its verdict waited on teardown), so a slot
    // is free for U10. The U10 chain (grow -> create -> delete) ends by chaining U11x, and EVERY U10 launcher
    // chains the next on ALL paths (skip/stale/no-slot included), so a U10 skip can never strand U11x.
    u10x_launcher(demo_cpu);
}

/// Build the U10 GROW fixture slot — the `u9x_build` shape for the `u10x-grow` blob (allocate, scrub the WHOLE
/// window, copy the blob into its RX-RO code page through the identity alias, return the run params). `None` on
/// slot-alloc failure. Does NOT pre-endow (the fixture's only negative, bit4, is an RO open of the same file).
fn u10x_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u10x_blob_start as usize;
    let bend = &raw const unaos_user_u10x_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U10x blob does not fit in a code page");
    let off = (&raw const unaos_user_u10x_grow as usize - bstart) as u64;
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

/// U10 GROW launcher — chained off `u9x_launcher` in program order. A THIN WRAPPER over `u10x_run` so that EVERY
/// exit path of the run (skip / stale / no-slot / verdict) still chains the next launcher: a U10 skip can NEVER
/// strand the downstream U10c/U10d demos or the already-landed U11x (the fall-through discipline the x86 chain
/// relies on, made structural).
fn u10x_launcher(demo_cpu: usize) {
    u10x_run(demo_cpu);
    u10cx_launcher(demo_cpu); // chain CREATE (which chains DELETE, then U11x) on ALL paths
}

/// U10 GROW run + verdict (real on-disk file growth). Flow (the `u9x_launcher` shape): one-shot; skip silently
/// with no block device (the control-path discipline). PRE-FLIGHT at IF=1 (gated on `HELLO_STAGED`, the BSP's
/// FAT-present signal): SELF-HEAL a persistent metal card (if GROW.BIN is absent or NOT exactly the planted
/// 512×0xC1 — e.g. a prior boot grew it to 528 — delete + recreate it, via pub `delete_located`/`create_in_root`/
/// `write_grow`, so re-runs stay honest, the U9x seed-restore idiom), then capture GROW.BIN's chain head into
/// `GROW_CLUSTER` (published BEFORE the fixture opens, so its RW open marks the descriptor growable + disk-backed).
/// Build + spawn `u10x-grow`; wait (bounded) for its witness exit + teardown (its two descriptors clear the FILES
/// + handle rows, and the RW one's teardown ENQUEUES the deferred Grow op). Then, disk-backed and only once
/// teardown is observed, DRAIN the U10 op to disk (`fat::write_grow`) and RAW-RE-READ from a fresh mount: the
/// directory size GREW to `U10_GROW_NEW_SIZE`, the appended bytes are on disk, the original first cluster still
/// holds `0xC1`, and the chain is the cluster-size-appropriate length with all FAT copies agreeing. PASS iff
/// witness == `U10X_WITNESS_ALL` AND torn down AND no kill AND (disk-backed) the drain succeeded AND every
/// on-disk check held. TWO MODES like U9x: disk-backed (a FAT volume) requires the on-disk proof; in-memory (no
/// FAT) runs the witness core with the drain a no-op and ZERO AP disk I/O.
fn u10x_run(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no block device -> keep the no-storage control path free of U10 lines
    }
    // Pre-flight at IF=1: ensure GROW.BIN is the pristine 512×0xC1 plant (self-heal a persistent card), then
    // capture its chain head. GATED on HELLO_STAGED (a FAT volume mounted); no FAT -> in-memory mode, no AP I/O.
    let fc = if HELLO_STAGED.load(Ordering::Acquire) {
        u10x_preflight_grow_file()
    } else {
        0
    };
    GROW_CLUSTER.store(fc, Ordering::Release); // publish the flush target (0 == in-memory mode) before the fixture
    let disk_backed = fc != 0;

    let Some(fix) = u10x_build() else {
        serial_println!(":: U10: no free address-space slot — file-growth demo skipped ::");
        return;
    };
    serial_println!(
        ":: U10: file growth — File+CAP_WRITE past EOF, staged in-place then GROWN on FAT via fat::write_grow (alloc + zero + chain, dir size last, out of the IF-masked handler) ::"
    );
    crate::arch::sched::spawn_user_in_space("u10x-grow", fix.entry, fix.sp, demo_cpu, fix.cr3);

    let vdeadline = crate::arch::ticks() + 5000;
    while U10X_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U10X_WITNESS.load(Ordering::Acquire);
    let killed = U10X_KILLED.load(Ordering::Acquire);

    let tdeadline = crate::arch::ticks() + 2000;
    while !(files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot) && wstage_all_free())
        && crate::arch::ticks() < tdeadline
    {
        crate::arch::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot) && wstage_all_free();

    // Drain the deferred Grow op to disk + prove it landed — disk-backed, and ONLY once teardown is observed
    // (`cleared` is the Acquire edge making the enqueue's stores visible; draining earlier could race a late
    // enqueue). The drain must have run exactly ONE op that returned true, and the fresh-mount re-read must show
    // the grow: size 528, the appended pattern, the original 0xC1 cluster intact, the chain the right length with
    // all FAT copies agreeing. A drain I/O timeout fails `drained` — bounded, never a hang.
    let (drained, grew_ok) = if disk_backed && cleared {
        match crate::fs::fat::mount() {
            Ok(fs) => {
                // S4d: knob-off drains exactly one deferred Grow op; knob-on the grow already persisted
                // in-syscall, so the queue is EMPTY (count 0) and `u10x_ondisk_grow_ok` reads the state the
                // synchronous grow already wrote.
                let (drained, _count) = u10_drain_verdict(&fs);
                (drained, u10x_ondisk_grow_ok(&fs))
            }
            Err(_) => (false, false),
        }
    } else {
        (false, false) // in-memory mode (drain a no-op) OR !cleared (verdict fails on `cleared`)
    };
    // STOR-1 S4 witness: knob-on, grow/create/delete run SYNCHRONOUSLY in-syscall via the storage service
    // task, so the U10 op-queue drained NOTHING (the U10x deferred-replay causal-fidelity gap is closed).
    // Emitted once here (the first U10 launcher, the S3-witness pattern); each U10 launcher independently
    // requires `count == 0` knob-on, so this headline is backed by every op in the chain.
    #[cfg(feature = "irqstorage")]
    if s4_sync_storage() && disk_backed {
        serial_println!(
            ":: S4: grow/create/delete SYNCHRONOUS in-syscall via the storage service task — U10 op-queue drained NOTHING (deferred-replay causal-fidelity gap closed) -> {} ::",
            if drained && grew_ok { "PASS" } else { "FAIL" }
        );
    }

    let core_ok = witness == U10X_WITNESS_ALL && cleared && killed == 0;
    let pass = core_ok && (!disk_backed || (drained && grew_ok));
    if pass {
        if disk_backed {
            serial_println!(
                ":: U10: file growth — open-RW+grow-write+readback OK, original cluster intact, RO-write -EACCES, staged grow FLUSHED to FAT (on-disk size grew + appended data present + FAT copies consistent) -> PASS ::"
            );
        } else {
            serial_println!(
                ":: U10: file growth — open-RW+grow-write+readback OK, original cluster intact, RO-write -EACCES (in-memory core; no FAT volume, grow-flush is a no-op) -> PASS ::"
            );
        }
    } else {
        serial_println!(
            ":: U10: file growth FAIL — disk_backed={} witness={:#x} cleared={} killed={} drained={} grew_ok={} done={} (want disk_backed?ALL/true/0/true/true : ALL/true/0) ::",
            disk_backed, witness, cleared, killed, drained, grew_ok, U10X_DONE.load(Ordering::Acquire),
        );
    }
}

/// U10 GROW pre-flight (IF=1): make GROW.BIN the pristine planted state (512 × `0xC1`, one cluster) and return
/// its chain head — `0` on any failure (drops the launcher to in-memory mode, never fails the demo). SELF-HEAL a
/// persistent metal card, the U9x seed-restore idiom: if GROW.BIN is absent, a directory, or NOT exactly 512
/// bytes (a prior boot grew it to 528), delete + recreate it as a fresh 512×0xC1 file so the strict "grew to 528"
/// proof can pass again across reboots. QEMU always starts from a fresh image, so the heal path only runs on
/// metal re-runs. Uses only pub `fat.rs` primitives (`find_located`/`delete_located`/`create_in_root`/`write_grow`).
fn u10x_preflight_grow_file() -> u32 {
    let Ok(fs) = crate::fs::fat::mount() else {
        return 0;
    };
    // If GROW.BIN is already the pristine 512-byte plant (fresh QEMU image, or an already-healed card), use it.
    if let Ok((de, _lba, _off)) = fs.find_located(U10_GROW_NAME) {
        if !de.is_dir && de.size == U10_GROW_PLANTED_SIZE && de.first_cluster() >= 2 {
            return de.first_cluster();
        }
        // Present but wrong (a prior boot's grown 528-byte copy, a directory, or a 0-length entry) — delete it so
        // the re-plant below starts from a clean absent state.
        if let Ok((de2, lba, off)) = fs.find_located(U10_GROW_NAME) {
            let _ = fs.delete_located(lba, off, de2.first_cluster());
        }
    }
    // GROW.BIN must be ABSENT now to re-plant it (create_in_root does NOT de-duplicate). If a delete above failed
    // and it is still present, bail to in-memory mode rather than risk corrupting it.
    if fs.find_located(U10_GROW_NAME).is_ok() {
        return 0;
    }
    // (Re)create a fresh 512×0xC1 GROW.BIN: a 0-length entry, then grow from empty with 512 filler bytes
    // (allocates + zero-fills + chains one cluster, RMWs the 0xC1 data, sets the dir size to 512).
    let (_de, lba, off) = match fs.create_in_root(U10_GROW_NAME, 0x20) {
        Ok(loc) => loc,
        Err(_) => return 0,
    };
    let filler = [U10_GROW_FILLER; U10_GROW_PLANTED_SIZE as usize];
    match fs.write_grow(0, 0, lba, off, 0, &filler) {
        Ok((w, _ns, new_fc)) if w == filler.len() => new_fc,
        _ => 0,
    }
}

/// U10 GROW on-disk proof (fresh re-read via `fs`): GROW.BIN's directory size is now `U10_GROW_NEW_SIZE`, the
/// appended pattern is at `U10_GROW_OFFSET`, the original first cluster still holds `0xC1`, and the cluster chain
/// is the CLUSTER-SIZE-APPROPRIATE length (`new_size.div_ceil(cluster_size)` — 2 on 512-B FAT32, 1 on the 2048-B
/// FAT16 fixed-root image) with every FAT copy agreeing. Cluster-size-aware so the proof is correct on all
/// layouts (a fixed `len == 2` would spuriously fail the FAT16 image where 528 bytes stay in one 2048-B cluster).
fn u10x_ondisk_grow_ok(fs: &crate::fs::fat::FatFs) -> bool {
    let Ok((de, _lba, _off)) = fs.find_located(U10_GROW_NAME) else {
        return false;
    };
    if de.is_dir || de.size != U10_GROW_NEW_SIZE {
        return false;
    }
    let fc = de.first_cluster();
    if fc < 2 {
        return false;
    }
    if u9x_read16(fc, de.size, U10_GROW_OFFSET) != Some(U10_GROW_PATTERN) {
        return false; // appended bytes not on disk
    }
    if u9x_read16(fc, de.size, 0) != Some([U10_GROW_FILLER; 16]) {
        return false; // original cluster corrupted by the grow
    }
    // Chain length must match the cluster geometry, and every FAT copy must agree along it (no torn/one-FAT write).
    let clus = fs.cluster_size();
    if clus == 0 {
        return false;
    }
    let expect = (de.size + clus - 1) / clus; // ceil — the codebase's manual idiom (fat.rs write_grow)
    let Ok(chain) = fs.chain_clusters(fc) else {
        return false;
    };
    if chain.len() as u32 != expect {
        return false;
    }
    let nf = fs.num_fats();
    for &c in &chain {
        let Ok(e0) = fs.fat_entry_copy(c, 0) else {
            return false;
        };
        for f in 1..nf {
            if fs.fat_entry_copy(c, f) != Ok(e0) {
                return false; // a copy disagrees (or read failed) -> a torn / one-FAT write
            }
        }
    }
    true
}

/// Build the U10 CREATE fixture slot — the `u10x_build` shape for the `u10cx-create` blob.
fn u10cx_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u10cx_blob_start as usize;
    let bend = &raw const unaos_user_u10cx_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U10cx blob does not fit in a code page");
    let off = (&raw const unaos_user_u10cx_create as usize - bstart) as u64;
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

/// U10 CREATE launcher — the thin wrapper (chains the next launcher on ALL paths of the run).
fn u10cx_launcher(demo_cpu: usize) {
    u10cx_run(demo_cpu);
    u10dx_launcher(demo_cpu); // chain DELETE (which chains U11x) on ALL paths
}

/// U10 CREATE run + verdict (real on-disk file creation). Flow (the `u10x_run` shape): one-shot; skip silently
/// with no block device. PRE-FLIGHT at IF=1 (gated on `HELLO_STAGED`): SELF-HEAL a persistent metal card — if
/// FRESH.BIN already exists (a prior boot created it), DELETE it so the ABSENT precondition holds and the demo
/// creates it afresh (the U9x seed-restore idiom; QEMU always starts clean). Build + spawn `u10cx-create`; wait
/// (bounded) for its witness exit + teardown (its two descriptors — the primary + the idempotent sibling — clear
/// the FILES + handle rows, and the DIRTY primary's teardown ENQUEUES the `CreateGrow` op). Then, disk-backed and
/// only once teardown is observed, DRAIN the op to disk (`create_in_root` + `write_grow`) and re-read from a fresh
/// mount: FRESH.BIN exists, size 16, the pattern is on disk, first cluster >= 2, and EXACTLY ONE root entry names
/// it (no duplicate). PASS iff witness == `U10CX_WITNESS_ALL` AND torn down AND no kill AND (disk-backed) the
/// drain succeeded AND every on-disk check held. In-memory mode (no FAT) runs the witness core, drain a no-op.
fn u10cx_run(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return;
    }
    // Pre-flight at IF=1: FRESH.BIN must be ABSENT to prove the demo CREATED it — self-heal a persistent card by
    // deleting a stale copy. `ready` == a FAT volume is present AND FRESH.BIN is now absent (disk-backed proof on).
    let ready = HELLO_STAGED.load(Ordering::Acquire) && u10_preflight_absent(U10C_NAME);

    let Some(fix) = u10cx_build() else {
        serial_println!(":: U10c: no free address-space slot — file-create demo skipped ::");
        return;
    };
    serial_println!(
        ":: U10c: file create — O_CREAT|RW of an absent name, written then CREATED on FAT via fat::create_in_root + write_grow (deferred to IF=1) ::"
    );
    crate::arch::sched::spawn_user_in_space("u10cx-create", fix.entry, fix.sp, demo_cpu, fix.cr3);

    let vdeadline = crate::arch::ticks() + 5000;
    while U10CX_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U10CX_WITNESS.load(Ordering::Acquire);
    let killed = U10CX_KILLED.load(Ordering::Acquire);

    let tdeadline = crate::arch::ticks() + 2000;
    while !(files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot) && wstage_all_free())
        && crate::arch::ticks() < tdeadline
    {
        crate::arch::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot) && wstage_all_free();

    // Gate the DRAIN on the SAME signal as the enqueue (HELLO_STAGED — a FAT volume is present), not the
    // stricter `ready`, so a FAT-present boot where the pre-flight failed (`ready` false) still DRAINS the op the
    // fixture enqueued (no stranded slot) and cannot masquerade as a false in-memory PASS. `ready` (the ABSENT
    // pre-flight held) is folded into the pass instead: a FAT-present demo must actually prove on disk.
    let disk_present = HELLO_STAGED.load(Ordering::Acquire);
    let (drained, created_ok) = if disk_present && cleared {
        match crate::fs::fat::mount() {
            Ok(fs) => {
                // S4d: knob-off drains exactly one deferred CreateGrow op; knob-on the create+grow already
                // persisted in-syscall (S4a/S4b), so the queue is EMPTY (count 0) and `u10cx_ondisk_create_ok`
                // reads the state the synchronous create+grow already wrote.
                let (drained, _count) = u10_drain_verdict(&fs);
                (drained, u10cx_ondisk_create_ok(&fs))
            }
            Err(_) => (false, false),
        }
    } else {
        (false, false)
    };

    let core_ok = witness == U10CX_WITNESS_ALL && cleared && killed == 0;
    let pass = core_ok && (!disk_present || (ready && drained && created_ok));
    if pass {
        if disk_present {
            serial_println!(
                ":: U10c: file create — O_CREAT|RW+write+readback+idempotent-reopen OK, CREATED on FAT (on-disk entry present + content + exactly one dir entry, no duplicate) -> PASS ::"
            );
        } else {
            serial_println!(
                ":: U10c: file create — O_CREAT|RW+write+readback+idempotent-reopen OK (in-memory core; no FAT volume, create-flush is a no-op) -> PASS ::"
            );
        }
    } else {
        serial_println!(
            ":: U10c: file create FAIL — disk_present={} ready={} witness={:#x} cleared={} killed={} drained={} created_ok={} done={} (want disk?ready/ALL/true/0/true/true : ALL/true/0) ::",
            disk_present, ready, witness, cleared, killed, drained, created_ok, U10CX_DONE.load(Ordering::Acquire),
        );
    }
}

/// U10 pre-flight helper (IF=1): ensure a runtime-created file `name` is ABSENT on disk (delete a stale copy from
/// a persistent metal card so the create/delete demo's ABSENT precondition holds across reboots), returning true
/// iff a FAT volume mounted AND the name is now absent. Uses only pub `fat.rs` primitives; QEMU always starts
/// from a fresh image, so the delete only runs on metal re-runs.
fn u10_preflight_absent(name: &str) -> bool {
    let Ok(fs) = crate::fs::fat::mount() else {
        return false;
    };
    if let Ok((de, lba, off)) = fs.find_located(name) {
        // A stale copy from a prior boot — delete it so the demo recreates it afresh.
        let _ = fs.delete_located(lba, off, de.first_cluster());
    }
    fs.find_located(name).is_err() // now absent?
}

/// U10 CREATE on-disk proof (fresh re-read via `fs`): FRESH.BIN exists, is non-dir, size `U10C_WRITTEN`, holds
/// the pattern, has a real first cluster, and appears EXACTLY ONCE in the root (no duplicate). Then re-runs the
/// create drain to exercise the idempotent create-if-present dedup branch (`find_located` hits -> `create_in_root`
/// is SKIPPED) — the deferred model creates only once, so this is what proves the no-duplicate guarantee the
/// aarch64 twin proves via its second in-handler open.
fn u10cx_ondisk_create_ok(fs: &crate::fs::fat::FatFs) -> bool {
    let Ok((de, _lba, _off)) = fs.find_located(U10C_NAME) else {
        return false;
    };
    if de.is_dir || de.size != U10C_WRITTEN || de.first_cluster() < 2 {
        return false;
    }
    if u9x_read16(de.first_cluster(), de.size, 0) != Some(U10C_PATTERN) {
        return false;
    }
    let Ok(root) = fs.read_root() else {
        return false;
    };
    if root.iter().filter(|d| !d.is_dir && d.name() == U10C_NAME).count() != 1 {
        return false;
    }
    // Idempotency (create-if-present): the drain finds FRESH.BIN and SKIPS create_in_root — a re-drain must not
    // duplicate. This exercises the find-first-then-create dedup branch the single-op deferred path can't otherwise.
    if !u10_drain_create_grow(fs, U10C_NAME, &U10C_PATTERN) {
        return false;
    }
    let Ok(root2) = fs.read_root() else {
        return false;
    };
    root2.iter().filter(|d| !d.is_dir && d.name() == U10C_NAME).count() == 1
}

/// Build the U10 DELETE fixture slot — the `u10x_build` shape for the `u10dx-delete` blob.
fn u10dx_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u10dx_blob_start as usize;
    let bend = &raw const unaos_user_u10dx_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U10dx blob does not fit in a code page");
    let off = (&raw const unaos_user_u10dx_delete as usize - bstart) as u64;
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

/// U10 DELETE launcher — the thin wrapper (chains U11x on ALL paths of the run, so a DELETE skip never strands
/// the already-landed U11x regression).
fn u10dx_launcher(demo_cpu: usize) {
    u10dx_run(demo_cpu);
    u11x_launcher(demo_cpu);
}

/// U10 DELETE run + verdict (real on-disk file delete). Flow (the `u10cx_run` shape): one-shot; skip silently
/// with no block device. PRE-FLIGHT at IF=1 (gated on `HELLO_STAGED`): SELF-HEAL a persistent card — DELME.BIN
/// must be ABSENT (delete a stale copy) — then SNAPSHOT `f0 = first_free_cluster()` (the cluster the drain's
/// create+grow will deterministically allocate, this being a single sequential demo). Build + spawn
/// `u10dx-delete`; wait (bounded) for its witness exit + teardown (unlink freed its descriptors + enqueued the
/// `CreateGrowDelete` op; the fixture holds nothing at exit). Then, disk-backed and only once teardown is
/// observed, DRAIN the op (create + grow -> allocates f0, the mid-op existence witness, then delete_located frees
/// it) and prove on disk: DELME.BIN GONE, the cluster f0 FREE in every FAT copy, and first-free is f0 again
/// (re-allocatable). PASS iff witness == `U10DX_WITNESS_ALL` AND torn down AND no kill AND (disk-backed) the drain
/// returned true AND gone+freed+reusable held. In-memory mode (no FAT) runs the witness core, drain a no-op.
fn u10dx_run(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return;
    }
    // Pre-flight at IF=1: DELME.BIN absent (self-heal), then snapshot the first free cluster the drain will use.
    let (ready, f0) = if HELLO_STAGED.load(Ordering::Acquire) && u10_preflight_absent(U10D_NAME) {
        match crate::fs::fat::mount().ok().and_then(|fs| fs.first_free_cluster().ok()) {
            Some(c) => (true, c),
            None => (false, 0),
        }
    } else {
        (false, 0)
    };

    let Some(fix) = u10dx_build() else {
        serial_println!(":: U10d: no free address-space slot — file-delete demo skipped ::");
        return;
    };
    serial_println!(
        ":: U10d: file delete — SYS_UNLINK a created file (name gone + all descriptors invalidated), DELETED on FAT via fat::delete_located (0xE5 + free chain, all copies; deferred to IF=1) ::"
    );
    crate::arch::sched::spawn_user_in_space("u10dx-delete", fix.entry, fix.sp, demo_cpu, fix.cr3);

    let vdeadline = crate::arch::ticks() + 5000;
    while U10DX_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U10DX_WITNESS.load(Ordering::Acquire);
    let killed = U10DX_KILLED.load(Ordering::Acquire);

    let tdeadline = crate::arch::ticks() + 2000;
    while !(files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot) && wstage_all_free())
        && crate::arch::ticks() < tdeadline
    {
        crate::arch::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot) && wstage_all_free();

    // Gate the DRAIN on the SAME signal as the enqueue (HELLO_STAGED), not the stricter `ready`, so a FAT-present
    // boot with a failed pre-flight still drains the fixture's op (no stranded slot) and cannot report a false
    // in-memory PASS; `ready` (absent pre-flight + a captured f0) is folded into the pass — a FAT-present demo
    // must prove the delete on disk.
    let disk_present = HELLO_STAGED.load(Ordering::Acquire);
    let (drained, deleted_ok) = if disk_present && cleared {
        match crate::fs::fat::mount() {
            Ok(fs) => {
                // S4d: knob-off drains exactly one deferred CreateGrowDelete op; knob-on the create+grow
                // persisted in-syscall (S4a/S4b) and the DELETE ran synchronously at the fixture's last close
                // (S4c: the unlink sweep freed both its descriptors, the last decref submitting the delete),
                // so the queue is EMPTY (count 0) and DELME.BIN is already gone on disk with its cluster freed.
                let (drained, _count) = u10_drain_verdict(&fs);
                (drained, u10dx_ondisk_delete_ok(&fs, U10D_NAME, f0))
            }
            Err(_) => (false, false),
        }
    } else {
        (false, false)
    };

    let core_ok = witness == U10DX_WITNESS_ALL && cleared && killed == 0;
    let pass = core_ok && (!disk_present || (ready && drained && deleted_ok));
    if pass {
        if disk_present {
            serial_println!(
                ":: U10d: file delete — create+write+unlink OK, sibling read -EACCES, re-open -ENOENT, DELETED on FAT (dir gone + chain freed in all FAT copies + cluster re-allocatable) -> PASS ::"
            );
        } else {
            serial_println!(
                ":: U10d: file delete — create+write+unlink OK, sibling read -EACCES, re-open -ENOENT (in-memory core; no FAT volume, delete-flush is a no-op) -> PASS ::"
            );
        }
    } else {
        serial_println!(
            ":: U10d: file delete FAIL — disk_present={} ready={} witness={:#x} cleared={} killed={} drained={} deleted_ok={} done={} (want disk?ready/ALL/true/0/true/true : ALL/true/0) ::",
            disk_present, ready, witness, cleared, killed, drained, deleted_ok, U10DX_DONE.load(Ordering::Acquire),
        );
    }
}

/// U10/U11x-M2 DELETE on-disk proof (fresh re-read via `fs`): `name` is GONE from the directory, the cluster
/// `f0` the create+grow drain allocated is FREE (`0`) in EVERY FAT copy, and `first_free_cluster` is `f0` again
/// (the freed cluster is re-allocatable). The drain having returned true already proves the file genuinely
/// EXISTED at `f0` with the written size mid-op (the existence witness in `u10_drain_create_grow_delete`), so
/// these three checks are non-vacuous. Shared by `u10dx_run` (DELME.BIN) and `u11m2_phase` (DEFER.BIN).
fn u10dx_ondisk_delete_ok(fs: &crate::fs::fat::FatFs, name: &str, f0: u32) -> bool {
    if fs.find_located(name).is_ok() {
        return false; // not gone
    }
    let nf = fs.num_fats();
    for f in 0..nf {
        match fs.fat_entry_copy(f0, f) {
            Ok(0) => {}
            _ => return false, // the freed cluster is not free in some FAT copy (or a read failed)
        }
    }
    fs.first_free_cluster().ok() == Some(f0) // re-allocatable
}

/// Build the U11x fixture slot — the `u9x_build` shape for the U11x blob (allocate, scrub the WHOLE window, copy
/// the blob into its RX-RO code page through the identity alias, return the run params). `None` if slot
/// allocation fails. Does NOT pre-endow (the fixture opens its own files; no negatives to plant).
fn u11x_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u11x_blob_start as usize;
    let bend = &raw const unaos_user_u11x_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U11x blob does not fit in a code page");
    let off = (&raw const unaos_user_u11x_close as usize - bstart) as u64;
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

/// U11x kernel-side proof of the generation-tag rebind fix, staged on a scratch row (6 — clear by this point in
/// the demo chain, since every fixture has exited + torn down and `u9x_check_revoked_write` used row 5; re-cleared
/// defensively first). It reproduces the U9x revoke+reopen aliasing hole MECHANICALLY: claim a descriptor slot and
/// mint its `(gen, idx)` file-id; free it (bumping the gen); then RE-claim the SAME slot (first-fit) for a
/// "different file" — and prove the OLD file-id is now rejected by `file_desc_validate` (a generation mismatch)
/// EVEN THOUGH the slot is live again (`FILE_USED` true), while a FRESH file-id minted against the reused slot
/// resolves. That is exactly "no silent rebind on slot reuse", isolated from disk/EL0 timing. Leaves the row clean
/// (no descriptor leaked). The aarch64 `u11_check_gen_rebind` twin (x86 `files_alloc`/`row` shape).
fn u11x_check_gen_rebind() -> bool {
    const A: usize = 6; // scratch private row (clear here — every fixture has torn down; u9x used row 5)
    clear_files_row(A); // defensive: start from a provably clear row
    let mut ok = true;
    // 1. Claim a slot; mint its file-id at the current generation. A live descriptor resolves.
    let Some(fid0) = files_alloc(A, 1 /*staged idx*/, 16 /*size*/, 0 /*wstage: RO*/, 0 /*cluster*/) else {
        return false;
    };
    let g0 = FILE_GEN[A][fid0].load(Ordering::Acquire);
    let id0 = file_id_pack(g0, fid0);
    ok &= file_desc_validate(A, id0) == Some(fid0);
    // 2. Free the slot (bumps the generation). The old file-id no longer resolves (slot is free).
    files_free(A, fid0);
    ok &= file_desc_validate(A, id0).is_none();
    // 3. Re-claim: first-fit reuses the SAME slot at the bumped generation — a "different file".
    let Some(fid1) = files_alloc(A, 1, 32 /*different size*/, 0, 0) else {
        return false;
    };
    ok &= fid1 == fid0; // first-fit really reclaimed the slot
    let g1 = FILE_GEN[A][fid1].load(Ordering::Acquire);
    ok &= g1 != g0; // the generation advanced on free
    let id1 = file_id_pack(g1, fid1);
    // 4. THE PROOF: the slot is LIVE again, yet the STALE file-id is rejected (gen mismatch — no rebind); the
    //    FRESH file-id resolves.
    ok &= FILE_USED[A][fid1].load(Ordering::Acquire);
    ok &= file_desc_validate(A, id0).is_none();
    ok &= file_desc_validate(A, id1) == Some(fid1);
    // Cleanup: drop the descriptor and demand the row clean (no leak).
    files_free(A, fid1);
    ok &= files_row_is_clear(A);
    ok
}

/// U11x launcher + verdict — the LAST demo in the chain, chained off `u9x_launcher` in program order. Flow:
/// one-shot; skip silently with no block device (the control-path discipline, chain-inherited); build + spawn the
/// `u11x-close` fixture; wait (bounded) for its witness exit + teardown (FILES row + handle row clear); then run
/// the kernel-side gen-rebind proof. PASS iff witness == `U11X_WITNESS_ALL` AND torn down AND no kill AND the
/// gen-rebind proof held. Releases no further gate. No disk I/O (the fixture reads the static staged seed and the
/// gen-rebind proof is pure descriptor bookkeeping) — so, unlike U9x, this runs identically in both FAT and
/// non-FAT block-device modes and needs no metal storage (the xHCI enumeration blocker still keeps it off metal,
/// like the rest of the storage-gated chain).
fn u11x_launcher(demo_cpu: usize) {
    // One-shot (reached once via the U9x chain; guard defensively anyway).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // No block device -> keep the no-storage control path free of demo lines (mirrors every prior gate; also
    // chain-inherited, since `u9x_launcher` already returned before reaching here in that case).
    if crate::drivers::block::info().is_none() {
        return;
    }
    let Some(fix) = u11x_build() else {
        serial_println!(":: U11x: no free address-space slot — open-file-lifecycle demo skipped ::");
        return;
    };
    serial_println!(
        ":: U11x: open-file lifecycle — SYS_CLOSE + generation-tagged file-ids (stale sibling to a reused slot -> -EACCES, no rebind) ::"
    );
    crate::arch::sched::spawn_user_in_space("u11x-close", fix.entry, fix.sp, demo_cpu, fix.cr3);

    // Wait (bounded, yielding) for the fixture's witness exit, then snapshot the witness + kill count.
    let vdeadline = crate::arch::ticks() + 5000;
    while U11X_DONE.load(Ordering::Acquire) < 1 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U11X_WITNESS.load(Ordering::Acquire);
    let killed = U11X_KILLED.load(Ordering::Acquire);

    // Teardown proof: the fixture exited holding one live descriptor (its reopened hB) and no pre-endowed handles,
    // so its exit cleared BOTH the FILES row and the handle row. Poll bounded; false->true.
    let tdeadline = crate::arch::ticks() + 2000;
    while !(files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot))
        && crate::arch::ticks() < tdeadline
    {
        crate::arch::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot);

    // Kernel-side mechanistic proof of the gen-tag (isolated from EL0 timing) — runs AFTER teardown so nothing
    // else touches the scratch row.
    let gen_ok = u11x_check_gen_rebind();

    if witness == U11X_WITNESS_ALL && cleared && killed == 0 && gen_ok {
        serial_println!(
            ":: U11x: open-file lifecycle — open+read/close/double-close(-EBADF)/use-after-close(-EACCES)/reopen+read OK, gen-tagged file-id rejects a stale sibling to a reused slot -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U11x: open-file lifecycle FAIL — witness={:#x} cleared={} killed={} gen_ok={} done={} (want {:#x}/true/0/true/1) ::",
            witness,
            cleared,
            killed,
            gen_ok,
            U11X_DONE.load(Ordering::Acquire),
            U11X_WITNESS_ALL,
        );
    }
    // U11x M2: chain the cross-process unlink-defers-free demo (program order, the u9x->u10x->..->u11x idiom).
    u11m2_launcher(demo_cpu);
}

/// Build a U11x M2 fixture slot — the `u11x_build` shape for the `u11m2-unlink` blob.
fn u11m2_build() -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u11m2_blob_start as usize;
    let bend = &raw const unaos_user_u11m2_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U11m2 blob does not fit in a code page");
    let off = (&raw const unaos_user_u11m2_unlink as usize - bstart) as u64;
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

/// One U11x M2 phase: the launcher plays "process S" on the held scratch row `srow` — CREATE DEFER.BIN through
/// the PRODUCT open path (`open_create_new`), write the pattern into its wstage, then spawn the EL0
/// `u11m2-unlink` fixture (its OWN slot/row — the allocator cannot hand it `srow`, which is held) to open the
/// file CROSS-PROCESS and unlink it. After the fixture exits + tears down, prove the DEFER (C1): the delete op
/// is still HELD (disk mode), the file is unlink-PENDING at refcount 1 (only `srow`'s open left), the name is
/// globally deleted — and `srow`'s descriptor still reads the ORIGINAL bytes (cross-process read-after-unlink).
/// Then RELEASE via the product path this phase exercises — `via_teardown == false`: the SYS_CLOSE core
/// (`files_free` + `handle_clear`), the explicit last-close; `via_teardown == true`: `free_user_space_by_cr3`,
/// the REAL whole-task teardown funnel (`clear_handle_row` -> `clear_files_row` -> the teardown decref — the pi4
/// M2b exit-without-close path; this also frees `srow` itself, so it must be the LAST phase). Prove the release
/// (C2: op drainable, refcount 0, pending cleared), then drain at IF=1 (count == 1) and prove on disk: name
/// GONE, chain freed in every FAT copy, cluster re-allocatable, and the name re-creatable again
/// (`DYN_DELETED_G` cleared at the drain). In-memory mode (no FAT): the fixture witness + C1/C2 run identically
/// minus the held-op/disk checks (the release clears `DYN_DELETED_G` instead — the no-queued-op arm). Returns
/// PASS; prints its own FAIL diagnostics. `want_done` gates the fixture-exit wait (the fixture runs once per
/// phase; `U11M2_DONE` is a cumulative count).
fn u11m2_phase(srow: usize, demo_cpu: usize, via_teardown: bool, want_done: u32) -> bool {
    let phase = if via_teardown { "teardown-release" } else { "close-release" };
    let nameid = match u10_name_id(U11M2_NAME) {
        Some(id) => id as usize,
        None => return false,
    };
    let disk_present = HELLO_STAGED.load(Ordering::Acquire);
    // Pre-flight at IF=1 (disk mode): DEFER.BIN absent (self-heal a persistent card) + snapshot the first free
    // cluster the drain's create+grow will deterministically allocate (the u10dx_run discipline).
    let (ready, f0) = if disk_present && u10_preflight_absent(U11M2_NAME) {
        match crate::fs::fat::mount().ok().and_then(|fs| fs.first_free_cluster().ok()) {
            Some(c) => (true, c),
            None => (false, 0),
        }
    } else {
        (false, 0)
    };
    // "Process S": create DEFER.BIN on the scratch row through the PRODUCT open path, then write the pattern
    // into its wstage (the launcher stands in for the pi4 fixture A; opens/writes complete strictly BEFORE the
    // fixture spawns, so the cross-row snapshot-copy hazard cannot fire in the demo).
    // U6x: DEFER.BIN is created PUBLIC — the cross-process `u11m2-unlink` fixture (a DIFFERENT row) opens it by
    // name, so it must be world-accessible (the pi4 `DEFER.BIN` O_PUBLIC twin). This keeps U11x M2 byte-identical.
    let h = open_create_new(srow, nameid as u32, true);
    if h < 0 {
        serial_println!(":: U11m2({}): scratch-row create failed ({}) ::", phase, h);
        return false;
    }
    let Some((_, fid)) = created_desc_any_row(srow, nameid as u32) else {
        serial_println!(":: U11m2({}): created descriptor not found on the scratch row ::", phase);
        // Defensive leg ("can't happen" — open_create_new just created it): sweep the whole scratch row so the
        // unfindable descriptor + its refcount cannot strand, then drop the handle.
        clear_files_row(srow);
        handle_clear(srow, h as usize);
        return false;
    };
    let Some(w) = (FILE_WSTAGE[srow][fid].load(Ordering::Acquire) as usize).checked_sub(1) else {
        serial_println!(":: U11m2({}): created descriptor has no wstage ::", phase);
        files_free(srow, fid); // defensive leg: release before returning (no stranded refcount)
        handle_clear(srow, h as usize);
        return false;
    };
    wstage_write_at(w, 0, U11M2_PATTERN.as_ptr() as u64, U11M2_PATTERN.len());
    wstage_set_len_at_least(w, U11M2_PATTERN.len() as u32);
    FILE_SIZE[srow][fid].store(U11M2_PATTERN.len() as u32, Ordering::Release);
    // STOR-1 S4: `open_create_new` above created DEFER.BIN on disk (S4a) as a 0-length entry, but the
    // launcher populates its wstage DIRECTLY (not through a grow syscall), so knob-on GROW it on disk too —
    // so the on-disk file owns a REAL cluster (`f0`) that the last-close DELETE frees, keeping the phase's
    // `u10dx_ondisk_delete_ok(.., f0)` proof NON-vacuous (a 0-length file would leave `f0` untouched, making
    // the freed/re-allocatable checks vacuously pass). Blocking-safe (the launcher's own scheduled task).
    #[cfg(feature = "irqstorage")]
    if s4_sync_storage() {
        let mut kbuf = U11M2_PATTERN; // a mutable copy the service task can read via *mut u8
        let _ = unsafe {
            crate::drivers::xhci::irqstorage::submit_grow(
                U11M2_NAME.as_bytes(),
                0,
                kbuf.as_mut_ptr(),
                U11M2_PATTERN.len(),
            )
        };
    }

    // Spawn the cross-process actor. `srow` is HELD (allocated), so the fixture's slot is necessarily distinct.
    let fix = match u11m2_build() {
        Some(f) => f,
        None => {
            serial_println!(":: U11m2({}): no free address-space slot — releasing + skipping ::", phase);
            files_free(srow, fid);
            handle_clear(srow, h as usize);
            return false;
        }
    };
    debug_assert!(fix.slot != srow, "u11m2: fixture landed on the held scratch row");
    U11M2_WITNESS.store(0, Ordering::Release);
    crate::arch::sched::spawn_user_in_space("u11m2-unlink", fix.entry, fix.sp, demo_cpu, fix.cr3);

    // Wait (bounded) for the fixture's witness exit, then for its teardown. NOTE: no `wstage_all_free()` in the
    // predicate — the launcher's own `srow` wstage is legitimately live across this whole phase.
    let vdeadline = crate::arch::ticks() + 5000;
    while U11M2_DONE.load(Ordering::Acquire) < want_done && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let witness = U11M2_WITNESS.load(Ordering::Acquire);
    let killed = U11M2_KILLED.load(Ordering::Acquire);
    let tdeadline = crate::arch::ticks() + 2000;
    while !(files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot)) && crate::arch::ticks() < tdeadline
    {
        crate::arch::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.slot) && handle_row_is_clear(fix.slot);

    // C1 — the DEFER is observable: the file is unlink-PENDING at refcount 1 (only srow's open left), the name
    // is globally gone, the delete op sits HELD (disk mode; NU10 == 1 -> entry 0), and srow's descriptor still
    // serves the ORIGINAL bytes (cross-process read-after-unlink — the pi4 A-side witness).
    // S4c: knob-on there is NO held op (the delete runs synchronously at the last close), so the deferred
    // state is just PENDING + DYN_DELETED_G — bypass the U10_USED/U10_HELD leg (`s4_sync_storage()` first).
    let c1 = OPENF_REFS[nameid].load(Ordering::Acquire) == 1
        && OPENF_PENDING[nameid].load(Ordering::Acquire)
        && DYN_DELETED_G[nameid].load(Ordering::Acquire)
        && (s4_sync_storage()
            || !disk_present
            || (U10_USED[0].load(Ordering::Acquire) && U10_HELD[0].load(Ordering::Acquire)))
        && wstage_bytes(w).get(..U11M2_PATTERN.len()) == Some(&U11M2_PATTERN[..]);

    // RELEASE — the phase's product path (see the doc comment). Both funnel into the same `openf_decref` seam.
    if via_teardown {
        crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(srow));
    } else {
        files_free(srow, fid);
        handle_clear(srow, h as usize);
    }

    // C2 — the release happened: refcount 0, pending consumed. S4c knob-on: the DELETE ran SYNCHRONOUSLY at
    // the release above (`openf_release` -> `submit_delete` + clears DYN_DELETED_G), so the name is already
    // re-creatable. Knob-off disk mode: the op is now DRAINABLE (still queued, hold dropped); no-FAT mode: the
    // in-memory delete completed. `s4_sync_storage()` first so the (empty) U10 queue is not consulted knob-on.
    let c2 = OPENF_REFS[nameid].load(Ordering::Acquire) == 0
        && !OPENF_PENDING[nameid].load(Ordering::Acquire)
        && if s4_sync_storage() {
            !DYN_DELETED_G[nameid].load(Ordering::Acquire)
        } else if disk_present {
            U10_USED[0].load(Ordering::Acquire) && !U10_HELD[0].load(Ordering::Acquire)
        } else {
            !DYN_DELETED_G[nameid].load(Ordering::Acquire)
        };

    // STOR-1 S6 carry-over (seat fold-in 2): witness WHICH last-close release path fired, so the metal-only
    // interleaving is mbench-VISIBLE rather than attended-eyeball. Knob-on the SYNCHRONOUS in-syscall delete won
    // (no held op — `openf_release`/the S6 unlink sweep ran `submit_delete`); the knob-off/deferred held-op
    // outcome is already implied by the phase PASS line, so only the knob-on distinction is emitted. Uncounted
    // idiom (no ` PASS`/`FAIL ::`).
    #[cfg(feature = "irqstorage")]
    if s4_sync_storage() {
        serial_println!(
            ":: S4-race({}): last close took the SYNCHRONOUS in-syscall delete path (no held op) — c2={} — witness OK ::",
            phase, c2
        );
    }
    // Drain at IF=1: knob-off replays exactly the ONE released delete op, every disk step true; knob-on the
    // delete already ran synchronously at the release, so the queue is EMPTY (count 0). Either way prove on
    // disk: DEFER.BIN GONE, its cluster `f0` freed in every FAT copy + re-allocatable, and the name
    // re-creatable (`DYN_DELETED_G` clear).
    let (drained, deleted_ok) = if disk_present {
        match crate::fs::fat::mount() {
            Ok(fs) => {
                let (drained, _count) = u10_drain_verdict(&fs);
                let ok = u10dx_ondisk_delete_ok(&fs, U11M2_NAME, f0)
                    && !DYN_DELETED_G[nameid].load(Ordering::Acquire);
                (drained, ok)
            }
            Err(_) => (false, false),
        }
    } else {
        (false, false)
    };

    let core_ok = witness == U11M2_WITNESS_ALL && cleared && killed == 0 && c1 && c2;
    let pass = core_ok && (!disk_present || (ready && drained && deleted_ok));
    if !pass {
        serial_println!(
            ":: U11m2({}) FAIL — disk_present={} ready={} witness={:#x} cleared={} killed={} c1={} c2={} drained={} deleted_ok={} done={} (want disk?ready/{:#x}/true/0/true/true/true/true : {:#x}/…) ::",
            phase, disk_present, ready, witness, cleared, killed, c1, c2, drained, deleted_ok,
            U11M2_DONE.load(Ordering::Acquire), U11M2_WITNESS_ALL, U11M2_WITNESS_ALL,
        );
    }
    pass
}

/// STOR-1 S5 witness core (run under `s5_shared_backing_witness`'s row alloc + cleanup): create DEFER.BIN on the
/// creator row `r1`, grow the disk to P1, open a CROSS-ROW sibling on `r2` through the PRODUCT open path, then
/// prove — NON-VACUOUSLY — that the sibling reads LIVE SHARED backing (not a private snapshot). Returns `None` for
/// a clean resource SKIP (no descriptor/wstage slot — no serial line), `Some(true)` PASS, `Some(false)` FAIL
/// (having printed its own diagnostic). The proof is a CONJUNCTION the old snapshot model fails: (Change 2) the
/// sibling's wstage is EMPTY (no snapshot copy) AND (Change 1) its read SOURCE (`created_read_live`, the exact fn
/// `sys_read` routes a created descriptor through) returns a peer's POST-OPEN overwrite — a value that cannot come
/// from the empty private buffer. `FILE_SIZE == 16` on the sibling proves the production `sys_read` size clamp is
/// exercised faithfully (a stale-0 size would make the real read EOF). No concurrent stress: read/write file ops
/// serialize through the SINGLE storage service task, so tearing is impossible by construction (not a metal race).
#[cfg(feature = "irqstorage")]
fn s5_witness_run(r1: usize, r2: usize, nameid: usize) -> Option<bool> {
    const P1: [u8; 16] = *b"S5-SHARED-BEF-01";
    const P2: [u8; 16] = *b"S5-SHARED-AFT-02";
    // Create DEFER.BIN PUBLIC on r1 (so the cross-row sibling may open it by name), then GROW the disk to P1.
    let h1 = open_create_new(r1, nameid as u32, true);
    if h1 < 0 {
        return None; // EMFILE/EAGAIN — a resource skip, not a failure
    }
    let Some((_, fid1)) = created_desc_any_row(r1, nameid as u32) else {
        return Some(false); // just created it — a miss is a real bug
    };
    let mut p1 = P1;
    if unsafe { crate::drivers::xhci::irqstorage::submit_grow(U11M2_NAME.as_bytes(), 0, p1.as_mut_ptr(), 16) } < 0 {
        return Some(false); // disk grow failed
    }
    // SF-2 (production-faithful): set the creator's FILE_SIZE so the sibling captures a non-zero size (else the
    // real `sys_read` clamps `want` to 0 -> EOF while `created_read_live(want=16)` would still read 16 and mask it).
    FILE_SIZE[r1][fid1].store(16, Ordering::Release);
    // Open the CROSS-ROW sibling through the product path (`sys_open_dynamic` -> the S5b re-check -> owned_access_ok
    // -> `open_created_sibling`, which knob-on seeds an EMPTY wstage — no snapshot copy).
    let h2 = sys_open_dynamic(r2, U11M2_NAME, 1); // RW
    if h2 < 0 {
        return if h2 == EMFILE { None } else { Some(false) }; // EMFILE (wstage/rows) -> skip; else fail
    }
    let Some((_, fid2)) = created_desc_any_row(r2, nameid as u32) else {
        return Some(false);
    };
    let Some(w2) = (FILE_WSTAGE[r2][fid2].load(Ordering::Acquire) as usize).checked_sub(1) else {
        return Some(false); // a sibling always owns a wstage
    };
    // Change 2 (residual 3): the sibling holds NO snapshot — its wstage is EMPTY. SF-2: it captured size 16.
    let no_snapshot = WSTAGE_LEN[w2].load(Ordering::Acquire) == 0;
    let size_ok = FILE_SIZE[r2][fid2].load(Ordering::Acquire) == 16;
    // Sanity: the sibling's LIVE read source returns the on-disk P1.
    let mut kbuf = [0u8; 16];
    let n1 = unsafe { created_read_live(r2, fid2, 0, kbuf.as_mut_ptr(), 16) };
    let read_p1 = n1 == 16 && kbuf == P1;
    // A PEER overwrites the disk in place AFTER the sibling opened (a different holder's write). Bytes never touch
    // the sibling's private wstage.
    let mut p2 = P2;
    let wr = unsafe {
        crate::drivers::xhci::irqstorage::submit_write_file(U11M2_NAME.as_bytes(), 0, p2.as_mut_ptr(), 16)
    };
    // Change 1 (residual 3, the payoff): the sibling now READS P2 — the post-open peer overwrite — from the shared
    // live backing, while its private wstage is STILL EMPTY. The conjunction is the discriminator: under the old
    // snapshot model `no_snapshot`/`still_empty` would be false (wstage held P1) and this could not observe P2.
    let mut kbuf2 = [0u8; 16];
    let n2 = unsafe { created_read_live(r2, fid2, 0, kbuf2.as_mut_ptr(), 16) };
    let read_p2 = n2 == 16 && kbuf2 == P2;
    let still_empty = WSTAGE_LEN[w2].load(Ordering::Acquire) == 0;
    let pass = no_snapshot && size_ok && read_p1 && wr == 16 && read_p2 && still_empty;
    if !pass {
        serial_println!(
            ":: S5 FAIL — no_snapshot={} size_ok={} read_p1={} wr={} read_p2={} still_empty={} (n1={} n2={}) ::",
            no_snapshot, size_ok, read_p1, wr, read_p2, still_empty, n1, n2
        );
    }
    Some(pass)
}

/// STOR-1 S5 witness — cross-process reads serve LIVE shared on-disk backing (not a private snapshot). A
/// deterministic, kernel-side, cross-row proof folded into the u11m2 launcher chain (runs FIRST, so DEFER.BIN and
/// all three wstage slots are free; it peaks at two and releases before the phases). REUSES DEFER.BIN — no new U10
/// name, so ZERO knob-off footprint. Requires knob-on + FAT + service (created reads route live); SKIPS SILENTLY
/// otherwise (no serial line — the DONE gate's `test 40` knob-on stays 18 PASS + MISSION + bx-blockreq). The
/// u11m2 / u6gx EL0 fixtures supply the production-faithful DISPATCH proof (a real SYS_READ of a cross-process
/// sibling returns the correct pattern from an EMPTY-wstage sibling -> the read can only be live). Every leg tears
/// the scratch rows down via the real teardown funnel (decrefs OPENF[DEFER] to 0) + deletes the on-disk file +
/// asserts the name is left idle — never a force-store of OPENF/DYN_DELETED_G, never a stranded slot/refcount.
/// STOR-1 S6 carry-over (seat fold-in 1): witness MF2 — a WRITABLE open of HELLO.BIN (staged index 0, immutable
/// EL0 code) is refused `-EACCES`. Kernel-side: resolve HELLO.BIN's staged index, then drive a RW open through the
/// REAL staged-open guard (`sys_open_staged`) on a scratch PRIVATE row. A private row proves MF2 in ISOLATION —
/// NOT SHARED_ROW (whose own RW refusal would mask MF2; MF2 is checked FIRST, so this is faithful). Uncounted idiom
/// (no ` PASS`/`FAIL ::`). Gated on the knob-on FAT path (bench media is knob-on and HELLO.BIN is staged there);
/// MF2 itself is knob-INDEPENDENT (immutable code is read-only in both states) — this witness just makes it
/// mbench-visible on the knob-on bench.
/// CFU-1 negative witness (uncounted, kernel-side): drive the REAL dispatcher (`syscall_dispatch`) with a
/// `SYS_OPEN` whose name pointer takes each of the three out-of-window shapes the unified `user_range_ok`
/// seam must reject — (a) a WRAPPING range (`ptr + len` overflows u64), (b) a pointer BELOW `USER_BASE`,
/// (c) a range whose END exceeds the window — and prove each returns `-EFAULT` with NO side effect. SYS_OPEN
/// is the vehicle because its `copy_from_user(name)` is the FIRST thing it does after the length check, so it
/// reaches the seam with no handle prerequisite: the `-EFAULT` is the seam's, not a capability denial, and
/// because `user_range_ok` rejects all three BEFORE any deref, the witness never touches the bad memory (no
/// ring-0 fault) and SYS_OPEN claims nothing (no file opened, no descriptor allocated). Two safe positive
/// controls (pure `user_range_ok`, no deref) prove the seam is a real gate, not an always-fail: a valid
/// in-window range validates `Ok`, and the READ bound admits the read-only code page (page 0) that the WRITE
/// bound rejects — the one semantic difference between the access modes. Uncounted idiom (a bare
/// `witness OK` / `FAIL` line, no ` PASS`/` FAIL` token the mbench count asserts). Runs on any block-present
/// path, knob-on and knob-off (a bad pointer needs no storage), so it exercises the seam on every gate.
fn cfu_efault_witness() {
    const NLEN: u64 = 8; // 0 < 8 <= MAX_NAME, so the length check passes and the name copy (the seam) runs
    let window = USER_WINDOW_PAGES * PAGE_SIZE;
    // The three rejected shapes, each driven through the real dispatcher. None dereferences the pointer —
    // `user_range_ok` returns `Err(EFAULT)` before `copy_from_user` copies, so this is safe in any CR3.
    let wrap = syscall_dispatch(SYS_OPEN, u64::MAX - 2, NLEN, 0, 0); // ptr + NLEN overflows (end < ptr)
    let below = syscall_dispatch(SYS_OPEN, USER_BASE - PAGE_SIZE, NLEN, 0, 0); // ptr < USER_BASE
    let above = syscall_dispatch(SYS_OPEN, USER_BASE + window - 4, NLEN, 0, 0); // end past the window
    // Positive controls (validate-only, no deref): a valid in-window READ range is accepted, and the READ
    // bound admits page 0 (a legal read source) while the WRITE bound rejects it (page 0 is RO/RX).
    let inwin_ok = user_range_ok(USER_BASE + PAGE_SIZE, NLEN, UserAccess::Read).is_ok();
    let read_page0 = user_range_ok(USER_BASE, NLEN, UserAccess::Read).is_ok();
    let write_page0 = user_range_ok(USER_BASE, NLEN, UserAccess::Write).is_err();
    let ok = wrap == EFAULT
        && below == EFAULT
        && above == EFAULT
        && inwin_ok
        && read_page0
        && write_page0;
    if ok {
        serial_println!(
            ":: CFU: SYS_OPEN wrap/below/above ranges each -EFAULT, no side effect, in-window accepted, write-bound rejects the RO code page — witness OK ::"
        );
    } else {
        serial_println!(
            ":: CFU FAIL — wrap={} below={} above={} inwin_ok={} rd0={} wr0={} (want -14,-14,-14,true,true,true) ::",
            wrap, below, above, inwin_ok, read_page0, write_page0
        );
    }
}

#[cfg(feature = "irqstorage")]
fn mf2_witness() {
    if !s4_sync_storage() {
        return;
    }
    let Some((sidx, size)) = staged_lookup("HELLO.BIN") else {
        return; // HELLO.BIN not staged — nothing to witness
    };
    if sidx != 0 {
        return; // HELLO.BIN must be staged index 0 (the immutable-code slot MF2 guards)
    }
    let Some(r) = crate::arch::memory::alloc_user_space() else {
        return; // no scratch address space — silent skip
    };
    let rc = sys_open_staged(r, sidx, size, 1); // RW (mode bit0) open of immutable staged code
    // Reclaim the whole scratch row (nothing was installed on the -EACCES path; belt-and-braces if it ever were).
    crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(r));
    if rc == EACCES {
        serial_println!(
            ":: S4-mf2: RW open of staged code (HELLO.BIN) refused -EACCES == expected — witness OK ::"
        );
    } else {
        serial_println!(":: S4-mf2 FAIL — RW open of HELLO.BIN returned {} (want -EACCES) ::", rc);
    }
}

/// STOR-1 S7 witness — an open of an ARBITRARY pre-existing on-disk file (NOT in the pre-stage set) resolves +
/// reads through the storage service task, retiring the U6bx BSP-staged-set constraint. Kernel-side + knob-on
/// (uncounted idiom, no ` PASS`/` FAIL` token). Target: README.TXT — a file every FAT test image carries
/// (make-fat-img.sh) that is NEITHER staged (STAGED_NAMES = HELLO.BIN/SCRATCH.BIN/GROW.BIN) NOR a U10 name, so
/// pre-S7 this exact open was `-ENOENT`. It drives the REAL dynamic dispatcher (`sys_open_dynamic`, what
/// `sys_open` calls once `staged_lookup` misses) on a scratch PRIVATE row, then proves a CONJUNCTION: the open
/// succeeded, minted a DYNAMIC descriptor (`FILE_DYNLEN != 0` — not a staged/created one), stamped with the
/// resolved NAME, sized from the LIVE volume (`FILE_SIZE > 0`, from `submit_stat`), and a live read BY THAT
/// STORED NAME returns the file's known content prefix. Gated on the knob-on FAT path; SILENT skip otherwise
/// (no serial line — the knob-off / no-FAT chains stay byte-identical). Tears the scratch row down through the
/// real teardown funnel; the dynamic descriptor owns no wstage/openf, so cleanup is a plain row free.
#[cfg(feature = "irqstorage")]
fn s7_openany_witness() {
    // Gate: dynamic on-disk resolution exists ONLY knob-on with a mounted FAT + the service task up. Off -> skip.
    if !s4_sync_storage() {
        return;
    }
    const NAME: &str = "README.TXT";
    const PREFIX: [u8; 16] = *b"UnaOS read-only "; // README.TXT begins with this on every make-fat-img.sh layout
    // Defensive: prove the target really exercises the DYNAMIC path (not a staged/U10 collision — it never is).
    if staged_lookup(NAME).is_some() || u10_name_id(NAME).is_some() {
        return;
    }
    let Some(r) = crate::arch::memory::alloc_user_space() else {
        return; // no scratch address space — silent skip
    };
    // Drive the REAL dispatcher, RO (mode 0). Pre-S7 this returned -ENOENT; S7 resolves it dynamically.
    let h = sys_open_dynamic(r, NAME, 0);
    let mut is_dyn = false;
    let mut name_ok = false;
    let mut size: u32 = 0;
    let mut read_n: i32 = -1;
    let mut read_ok = false;
    if h >= 0 {
        // Resolve the handle -> descriptor through the production seam (handle_resolve + file_desc_validate).
        if let Ok(HandleTarget::File(file_id)) = handle_resolve(r, h as u64, CAP_READ) {
            if let Some(fid) = file_desc_validate(r, file_id) {
                is_dyn = FILE_DYNLEN[r][fid].load(Ordering::Acquire) != 0;
                size = FILE_SIZE[r][fid].load(Ordering::Acquire);
                let mut nb = [0u8; MAX_NAME];
                let nl = dyn_name_get(r, fid, &mut nb);
                name_ok = nl == NAME.len() && &nb[..nl] == NAME.as_bytes();
                // Read the live source BY THE DESCRIPTOR'S STORED NAME — the same route `sys_read`'s dynamic
                // branch takes (`submit_read_file`) — and match the file's known content prefix.
                let mut kbuf = [0u8; 16];
                read_n = unsafe {
                    crate::drivers::xhci::irqstorage::submit_read_file(&nb[..nl], 0, kbuf.as_mut_ptr(), 16)
                };
                read_ok = read_n == 16 && kbuf == PREFIX;
            }
        }
    }
    // SECURITY REGRESSION LOCK (review CONFIRMED-critical): the dynamic path canonicalizes the name to 8.3
    // UPPERCASE and EXCLUDES any case-variant of a staged/U10 name — because `find_located` resolves
    // case-INSENSITIVELY, a lowercase "owned.bin" must NOT reach the dynamic open (which would read the U6gx
    // owner-private OWNED.BIN with no ACL check). This is a NAME-TABLE decision (u10_name_id("OWNED.BIN") is
    // Some), so it fires BEFORE any disk resolution — deterministic regardless of whether OWNED.BIN is on disk.
    // Must be refused (`< 0`); pre-fix it returned a handle (the bypass). No cleanup: a refusal installs nothing.
    let deny = sys_open_dynamic(r, "owned.bin", 0);
    let excl_ok = deny < 0;
    // Teardown: release the whole scratch row through the real funnel (clears the dynamic descriptor + handle).
    crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(r));
    let pass = h >= 0 && is_dyn && name_ok && size >= PREFIX.len() as u32 && read_ok && excl_ok;
    if pass {
        serial_println!(
            ":: S7-openany: a non-staged on-disk file (README.TXT, {} bytes) resolved dynamically + read its live content off the pre-stage set, and a case-variant of an owned name (\"owned.bin\") was refused (owner ACL not bypassed) == expected — witness OK ::",
            size
        );
    } else {
        serial_println!(
            ":: S7-openany FAIL — h={} is_dyn={} name_ok={} size={} read_n={} read_ok={} excl_ok={} deny={} (want h>=0 dyn name size>={} readPREFIX deny<0) ::",
            h, is_dyn, name_ok, size, read_n, read_ok, excl_ok, deny, PREFIX.len()
        );
    }
}

/// STOR-1 S8 witness — a RW open of an ARBITRARY pre-existing on-disk file (NOT staged, NOT a U10 name) writes
/// THROUGH to the LIVE volume, strictly overwrite-only, off the pre-stage set. Kernel-side + knob-on (uncounted
/// idiom, no ` PASS`/` FAIL` token, so the fixture PASS count stays byte-equivalent). Target: S8W.BIN — 64 bytes
/// of 0xA5 planted by make-fat-img.sh, NEITHER staged (HELLO/SCRATCH/GROW.BIN) NOR a U10 name, and NEVER
/// README.TXT (S7 checks that file's prefix). It drives the REAL seams: `sys_open_dynamic(.., 1)` mints a RW
/// dynamic descriptor (CAP_WRITE, FILE_DYNLEN != 0, size 64); the kernel-buffer helper `dyn_write_live` lands a
/// distinct 16-byte pattern at offset 8; `submit_read_file` reads it back == pattern; then it RESTORES the 0xA5
/// seed bytes and re-verifies — so the image/card is left pristine and the witness is IDEMPOTENT across boots +
/// power-cuts (it never depends on prior content). Negative leg: `sys_open_dynamic(.., "hello.bin", 1)` — a
/// case-variant of staged EL0 code — MUST be refused `< 0` (the MF2-under-S8 regression lock). (STOR-1 S9
/// retired the former past-EOF-returns-0 leg: a past-EOF write now GROWS, witnessed by `s9_grow_witness` on a
/// throwaway file — S8W.BIN must NOT be grown here or it would lose idempotency. This witness is now purely
/// in-EOF overwrite.) Gated on the knob-on FAT path; SILENT skip otherwise (no serial line — knob-off / no-FAT
/// chains stay byte-identical). Tears the scratch row down through the real funnel; the dynamic descriptor owns
/// no wstage/openf, so cleanup is a plain row free.
#[cfg(feature = "irqstorage")]
fn s8_write_witness() {
    if !s4_sync_storage() {
        return;
    }
    const NAME: &str = "S8W.BIN";
    const SEED: u8 = 0xA5; // make-fat-img.sh plants 64 bytes of this
    const SIZE: u32 = 64;
    const OFF: u32 = 8;
    const PAT: [u8; 16] = *b"S8-WRITE-WITNES!"; // a distinct 16-byte pattern (never 0xA5)
    // Defensive: prove the target really exercises the DYNAMIC path (not a staged/U10 collision — it never is).
    if staged_lookup(NAME).is_some() || u10_name_id(NAME).is_some() {
        return;
    }
    let Some(r) = crate::arch::memory::alloc_user_space() else {
        return; // no scratch address space — silent skip
    };
    // Drive the REAL dispatcher, RW (mode bit0). S8 mints a writable dynamic descriptor for a non-staged/non-U10
    // on-disk file; pre-S8 this same open was -EACCES.
    let h = sys_open_dynamic(r, NAME, 1);
    let mut is_dyn = false;
    let mut cap_write = false;
    let mut size: u32 = 0;
    let mut wrote: i32 = -1;
    let mut readback_ok = false;
    let mut restored_ok = false;
    if h >= 0 {
        // Resolve the handle -> descriptor through the production seam, REQUIRING CAP_WRITE (S8's new right).
        if let Ok(HandleTarget::File(file_id)) = handle_resolve(r, h as u64, CAP_WRITE) {
            cap_write = true;
            if let Some(fid) = file_desc_validate(r, file_id) {
                is_dyn = FILE_DYNLEN[r][fid].load(Ordering::Acquire) != 0;
                size = FILE_SIZE[r][fid].load(Ordering::Acquire);
                // (1) write the distinct pattern at OFF via the kernel-buffer helper (the live write seam).
                let mut wbuf = PAT;
                wrote = unsafe { dyn_write_live(r, fid, OFF, wbuf.as_mut_ptr(), PAT.len()) };
                // (2) read it back live BY THE STORED NAME and match.
                let mut rbuf = [0u8; 16];
                let rn = unsafe {
                    crate::drivers::xhci::irqstorage::submit_read_file(
                        NAME.as_bytes(), OFF, rbuf.as_mut_ptr(), 16,
                    )
                };
                readback_ok = wrote == PAT.len() as i32 && rn == 16 && rbuf == PAT;
                // (3) RESTORE the 0xA5 seed so the image stays pristine + the witness is idempotent, re-verify.
                let mut sbuf = [SEED; 16];
                let sw = unsafe { dyn_write_live(r, fid, OFF, sbuf.as_mut_ptr(), 16) };
                let mut vbuf = [0u8; 16];
                let vn = unsafe {
                    crate::drivers::xhci::irqstorage::submit_read_file(
                        NAME.as_bytes(), OFF, vbuf.as_mut_ptr(), 16,
                    )
                };
                restored_ok = sw == 16 && vn == 16 && vbuf == [SEED; 16];
                // (STOR-1 S9 retired the past-EOF-returns-0 leg — a past-EOF write now GROWS, so driving one on
                // S8W.BIN would grow it permanently and break idempotency. Growth is witnessed by s9_grow_witness
                // on a throwaway file. This witness stays purely in-EOF overwrite, leaving S8W.BIN pristine.)
            }
        }
    }
    // (a) MF2-UNDER-S8 regression lock: a case-variant of staged EL0 code ("hello.bin") RW MUST be refused — the
    // canonicalize-to-UPPERCASE exclusion drops it before any disk resolution. Must be `< 0` (installs nothing).
    let deny = sys_open_dynamic(r, "hello.bin", 1);
    let excl_ok = deny < 0;
    // Teardown: release the whole scratch row through the real funnel (clears the dynamic descriptor + handle).
    crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(r));
    let pass = h >= 0
        && is_dyn
        && cap_write
        && size == SIZE
        && readback_ok
        && restored_ok
        && excl_ok;
    if pass {
        serial_println!(
            ":: S8-write: a non-staged on-disk file (S8W.BIN, {} bytes) opened RW + overwrote a live 16-byte pattern in place off the pre-stage set, read it back, RESTORED the seed (pristine + idempotent); a case-variant of staged code (\"hello.bin\") RW was refused (MF2 intact) == expected — witness OK ::",
            size
        );
    } else {
        serial_println!(
            ":: S8-write FAIL — h={} is_dyn={} cap_write={} size={} readback_ok={} restored_ok={} excl_ok={} deny={} (want h>=0 dyn capW size={} readback restore deny<0) ::",
            h, is_dyn, cap_write, size, readback_ok, restored_ok, excl_ok, deny, SIZE
        );
    }
}

/// STOR-1 S9 witness — a RW open of a DYNAMIC on-disk file, a past-EOF write GROWS it on disk (the S8-overwrite
/// successor). Kernel-side + knob-on (uncounted idiom, no ` PASS`/` FAIL` token, so the fixture PASS count stays
/// byte-equivalent). The witness OWNS the file's whole lifecycle so it is idempotent by construction across boots
/// + power-cuts (self-heal delete FIRST, delete LAST): it deletes any prior S9G.BIN, CREATEs a fresh 0-length one
/// on the live volume, opens it RW dynamically (size 0, NOT staged, NOT a U10 name), then GROWs it in two steps
/// via the kernel-buffer live seam `dyn_grow_live` (0->48->96) — the same route `sys_write_file`'s S9 grow branch
/// takes (`submit_grow`) — and proves through the service task that (a) `submit_stat` reports the file GREW to 96
/// (real on-disk alloc + chain + dir-size bump), and (b) `submit_read_file` returns the appended bytes. Bound
/// legs: the PER-FILE ceiling is witnessed through the REAL `sys_write_file` — an offset at `DYN_FILE_MAX` is
/// refused `-ENOSPC` (the cap fires BEFORE any user deref, so the buf is never touched, exactly as S8's eof-zero
/// leg). The PER-WRITE page cap is a short-count clamp verified by review (like S8's user-pointer clamp NOTE — no
/// user memory is mappable from the launcher context to drive it). Refusal leg: `sys_open_dynamic("hello.bin", 1)`
/// — staged EL0 code — MUST still be refused `< 0` (the MF2-under-S8/S9 lock is untouched). Gated on the knob-on
/// FAT path; SILENT skip otherwise. Tears the scratch row down through the real funnel + deletes S9G.BIN.
#[cfg(feature = "irqstorage")]
fn s9_grow_witness() {
    if !s4_sync_storage() {
        return;
    }
    const NAME: &str = "S9G.BIN";
    const G1: usize = 48; // first grow: 0 -> 48 (allocates the file's first cluster from empty)
    const G2: usize = 48; // second grow: 48 -> 96 (incremental extend)
    const FINAL: u32 = (G1 + G2) as u32;
    const B1: u8 = 0xE1; // first-grow filler
    const B2: u8 = 0xE2; // second-grow filler
    // Defensive: prove the target really exercises the DYNAMIC path (not a staged/U10 collision — it never is).
    if staged_lookup(NAME).is_some() || u10_name_id(NAME).is_some() {
        return;
    }
    let Some(r) = crate::arch::memory::alloc_user_space() else {
        return; // no scratch address space — silent skip
    };
    // Self-heal any prior boot's grown copy, then CREATE a fresh 0-length file on the live volume.
    let _ = unsafe { crate::drivers::xhci::irqstorage::submit_delete(NAME.as_bytes()) };
    let created = unsafe { crate::drivers::xhci::irqstorage::submit_create(NAME.as_bytes()) };
    // Open it RW dynamically (mode bit0). S9 mints a writable dynamic descriptor; size starts at 0 (fresh file).
    let h = sys_open_dynamic(r, NAME, 1);
    let mut is_dyn = false;
    let mut cap_write = false;
    let mut size0: u32 = u32::MAX;
    let mut grew1: i32 = -1;
    let mut grew2: i32 = -1;
    let mut stat_grew: i32 = -1;
    let mut readback_ok = false;
    let mut cap_enospc = false;
    if created == 0 && h >= 0 {
        if let Ok(HandleTarget::File(file_id)) = handle_resolve(r, h as u64, CAP_WRITE) {
            cap_write = true;
            if let Some(fid) = file_desc_validate(r, file_id) {
                is_dyn = FILE_DYNLEN[r][fid].load(Ordering::Acquire) != 0;
                size0 = FILE_SIZE[r][fid].load(Ordering::Acquire);
                // (1) GROW past EOF twice via the kernel-buffer live seam (the S9 grow SOURCE) — 0->48->96.
                let mut w1 = [B1; G1];
                grew1 = unsafe { dyn_grow_live(r, fid, 0, w1.as_mut_ptr(), G1) };
                let mut w2 = [B2; G2];
                grew2 = unsafe { dyn_grow_live(r, fid, G1 as u32, w2.as_mut_ptr(), G2) };
                // (2) prove the on-disk size GREW (Stat through the service task) — a real alloc + chain + dir bump.
                stat_grew = unsafe { crate::drivers::xhci::irqstorage::submit_stat(NAME.as_bytes()) };
                // (3) read the whole grown file back BY NAME and match the appended bytes.
                let mut rbuf = [0u8; G1 + G2];
                let rn = unsafe {
                    crate::drivers::xhci::irqstorage::submit_read_file(
                        NAME.as_bytes(), 0, rbuf.as_mut_ptr(), G1 + G2,
                    )
                };
                readback_ok = grew1 == G1 as i32
                    && grew2 == G2 as i32
                    && stat_grew == FINAL as i32
                    && rn == (G1 + G2) as i32
                    && rbuf[..G1].iter().all(|&b| b == B1)
                    && rbuf[G1..].iter().all(|&b| b == B2);
                // (4) PER-FILE ceiling: drive the REAL sys_write_file with the offset AT DYN_FILE_MAX — the grow
                // branch fires (offset far past size) and `dyn_write_grow` refuses `-ENOSPC` BEFORE any user
                // deref (so USER_BASE is never touched, exactly as S8's eof-zero leg). Witnesses the DoS bound.
                FILE_OFFSET[r][fid].store(DYN_FILE_MAX as u32, Ordering::Release);
                cap_enospc = sys_write_file(r, file_id, USER_BASE, 16) == ENOSPC;
            }
        }
    }
    // Refusal leg: a case-variant of staged EL0 code ("hello.bin") RW MUST still be refused (MF2 lock intact).
    let deny = sys_open_dynamic(r, "hello.bin", 1);
    let excl_ok = deny < 0;
    // Teardown: release the scratch row through the real funnel, THEN delete the on-disk file (idempotent cleanup).
    crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(r));
    let _ = unsafe { crate::drivers::xhci::irqstorage::submit_delete(NAME.as_bytes()) };
    let pass = created == 0
        && h >= 0
        && is_dyn
        && cap_write
        && size0 == 0
        && readback_ok
        && cap_enospc
        && excl_ok;
    if pass {
        serial_println!(
            ":: S9-grow: a dynamic on-disk file (S9G.BIN) opened RW GREW past EOF live off the pre-stage set — Stat confirms it extended 0 -> {} bytes (real alloc + chain), the appended bytes read back, a write at the {}-byte per-file ceiling was refused -ENOSPC, and staged code (\"hello.bin\") RW stayed refused (MF2 intact) == expected — witness OK ::",
            FINAL, DYN_FILE_MAX
        );
    } else {
        serial_println!(
            ":: S9-grow FAIL — created={} h={} is_dyn={} cap_write={} size0={} grew1={} grew2={} stat_grew={} readback_ok={} cap_enospc={} excl_ok={} deny={} (want created=0 h>=0 dyn capW size0=0 grew1={} grew2={} stat={} readback enospc deny<0) ::",
            created, h, is_dyn, cap_write, size0, grew1, grew2, stat_grew, readback_ok, cap_enospc, excl_ok, deny, G1, G2, FINAL
        );
    }
}

#[cfg(feature = "irqstorage")]
fn s5_shared_backing_witness() {
    // Gate: created reads route live ONLY knob-on with a mounted FAT + the service task up. Off -> SILENT skip.
    if !s4_sync_storage() {
        return;
    }
    let Some(nameid) = u10_name_id(U11M2_NAME).map(|i| i as usize) else {
        return;
    };
    // DEFER.BIN must be idle before the phases; if not (can't happen here — the witness is FIRST), don't interfere.
    if OPENF_REFS[nameid].load(Ordering::Acquire) != 0 || DYN_DELETED_G[nameid].load(Ordering::Acquire) {
        return;
    }
    // Self-heal a persistent card: remove any stale on-disk DEFER.BIN so the create starts clean.
    let _ = unsafe { crate::drivers::xhci::irqstorage::submit_delete(U11M2_NAME.as_bytes()) };
    let Some(r1) = crate::arch::memory::alloc_user_space() else {
        return; // no scratch address space — silent skip
    };
    let Some(r2) = crate::arch::memory::alloc_user_space() else {
        crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(r1));
        return;
    };
    let outcome = s5_witness_run(r1, r2, nameid);
    // Cleanup — release BOTH rows via the real teardown funnel (each created descriptor's `openf_release` decrefs
    // OPENF[DEFER]; the name was never unlinked, so no last-close delete fires), THEN delete the on-disk file.
    crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(r2));
    crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(r1));
    let _ = unsafe { crate::drivers::xhci::irqstorage::submit_delete(U11M2_NAME.as_bytes()) };
    let cleaned =
        OPENF_REFS[nameid].load(Ordering::Acquire) == 0 && !DYN_DELETED_G[nameid].load(Ordering::Acquire);
    match outcome {
        None => {} // silent resource skip (no line)
        Some(true) if cleaned => serial_println!(
            ":: S5: cross-process read serves LIVE shared backing — a created-file sibling holds NO private snapshot (empty wstage) yet its read SOURCE returns a peer's POST-OPEN overwrite (P2); torn-copy (residual 3) closed by construction, open-vs-unlink (residual 4) re-checked; read/write serialize through the single service task -> PASS ::"
        ),
        Some(true) => serial_println!(
            ":: S5 FAIL — witness passed but cleanup left DEFER.BIN state (OPENF={} DYN_DELETED={}) ::",
            OPENF_REFS[nameid].load(Ordering::Acquire),
            DYN_DELETED_G[nameid].load(Ordering::Acquire)
        ),
        Some(false) => {} // FAIL diagnostic already printed by s5_witness_run
    }
}

// =============================================================================================
// STOR-1 S6b — the NAMESPACE-lock cross-core witness (the pi4 F3 `f3_witness` twin). Proves the S6a lock HOLDS:
// two cores drive a non-atomic RMW of a scratch counter with a WIDE read->write window, once serialized through
// the REAL `ns_lock` and once un-serialized. LOCKED must reach `2*N` (no lost update -> the lock serializes);
// the UNLOCKED control loses increments UNDER TRUE PARALLELISM. In-RAM only (no service task, no disk) — never a
// submit, so it cannot re-enter the S5 deadlock class, and it runs BEFORE u6gx spawns its cooperative spinners.
// **QEMU-green ≠ correct (design risk 3):** under RR-TCG the cores rarely interleave, so the unlocked control
// shows ZERO loss here — the negative is metal-latent; the POSITIVE (the lock engaged + the serialized path
// reaches `2*N`) is what this proves in QEMU. Emits one `S6-witness:` line — NOT a `-> PASS`/`FAIL ::` line, so
// the fixture PASS count stays byte-equivalent. Knob-on FAT only (keeps knob-off byte-identical).
#[cfg(feature = "irqstorage")]
static S6_WITNESS_COUNTER: AtomicU32 = AtomicU32::new(0);

/// S6b witness — one non-atomic RMW of the scratch counter with a wide read->write window (the F2/F3 step shape).
#[cfg(feature = "irqstorage")]
#[inline(never)]
fn s6_witness_step() {
    let v = S6_WITNESS_COUNTER.load(Ordering::Relaxed);
    for _ in 0..48 {
        core::hint::spin_loop();
    }
    S6_WITNESS_COUNTER.store(v.wrapping_add(1), Ordering::Relaxed);
}

/// S6b witness — drive `iters` steps, optionally serialized through the REAL namespace lock (`ns_lock`).
#[cfg(feature = "irqstorage")]
fn s6_witness_rmw(iters: u32, locked: bool) {
    for _ in 0..iters {
        if locked {
            let _ns = ns_lock();
            s6_witness_step();
        } else {
            s6_witness_step();
        }
    }
}

#[cfg(feature = "irqstorage")]
fn s6_witness_worker_locked(iters: usize) {
    s6_witness_rmw(iters as u32, true);
}

#[cfg(feature = "irqstorage")]
fn s6_witness_worker_unlocked(iters: usize) {
    s6_witness_rmw(iters as u32, false);
}

/// S6b — the cross-core witness for the S6a NAMESPACE serialization. This task drives its half inline on its own
/// core (`vcpu`); a joinable worker runs on `demo_cpu` (a distinct schedulable AP), so the two halves execute on
/// two cores. Gated on the knob-on FAT path (byte-identical knob-off). Runs from `u11m2_launcher` BEFORE u6gx
/// spawns its cooperative spinners.
#[cfg(feature = "irqstorage")]
fn s6_witness_launcher(demo_cpu: usize) {
    if !s4_sync_storage() {
        return;
    }
    const N: u32 = 120_000;
    let want = 2 * N;

    S6_WITNESS_COUNTER.store(0, Ordering::SeqCst);
    let h = crate::arch::sched::spawn_joinable(
        "s6-witness-lk",
        s6_witness_worker_locked,
        N as usize,
        demo_cpu,
        crate::arch::sched::PRIO_NORMAL,
    );
    s6_witness_rmw(N, true);
    h.join();
    let locked_got = S6_WITNESS_COUNTER.load(Ordering::SeqCst);

    S6_WITNESS_COUNTER.store(0, Ordering::SeqCst);
    let h2 = crate::arch::sched::spawn_joinable(
        "s6-witness-ul",
        s6_witness_worker_unlocked,
        N as usize,
        demo_cpu,
        crate::arch::sched::PRIO_NORMAL,
    );
    s6_witness_rmw(N, false);
    h2.join();
    let unlocked_got = S6_WITNESS_COUNTER.load(Ordering::SeqCst);
    let unlocked_lost = want.saturating_sub(unlocked_got);

    if locked_got == want {
        if unlocked_lost > 0 {
            serial_println!(
                ":: S6-witness: NAMESPACE cross-core RMW (worker on cpu {}) — locked {}/{} intact (0 lost); unlocked lost {}/{} -> the created-file open/create/unlink sequence lock serializes under real contention -> witness OK ::",
                demo_cpu, locked_got, want, unlocked_lost, want
            );
        } else {
            serial_println!(
                ":: S6-witness: NAMESPACE cross-core RMW (worker on cpu {}) — locked {}/{} intact; unlocked also 0 lost (QEMU RR-TCG did not interleave — cross-core contention is metal-only), lock engaged + serialized path intact -> witness OK ::",
                demo_cpu, locked_got, want
            );
        }
    } else {
        serial_println!(
            ":: S6-witness FAIL — NAMESPACE cross-core RMW (worker on cpu {}) — locked {}/{}, LOST {} increments UNDER the lock -> SERIALIZATION REGRESSION ::",
            demo_cpu, locked_got, want, want - locked_got
        );
    }
}

/// U11x M2 launcher + verdict — cross-process unlink-defers-free, the x86 twin of pi4 U11 M2/M2b (aarch64
/// `b88d2ba`/`303e271`): a file's deferred on-disk DELETE fires at the LAST close ACROSS PROCESSES, not at
/// unlink — the launcher's IF=1 drain playing the pi4 reaper. Two phases over the same machinery, one per
/// release path: (1) explicit last CLOSE (the SYS_CLOSE core), (2) whole-task TEARDOWN (`free_user_space_by_cr3`
/// — exit-without-close, the pi4 M2b orphan). Chained off `u11x_launcher` (LAST in the demo chain). The scratch
/// row is a REAL allocated slot held across both phases (never a hardcoded row — the fixture's allocator must
/// not be able to claim it); phase 2's teardown-release frees it, and every failure path releases before
/// returning, so no held op or pending name can strand for the boot.
fn u11m2_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return;
    }
    // CFU-1: negative witness for the unified kernel/user copy seam — three out-of-window SYS_OPEN name
    // ranges each -EFAULT through the REAL dispatcher, no side effect (uncounted; knob-on AND knob-off).
    cfu_efault_witness();
    // STOR-1 S5: prove cross-process created-file reads serve LIVE shared backing (knob-on + FAT only). Runs
    // FIRST — DEFER.BIN and all wstage slots are free; it reuses DEFER.BIN and cleans up fully before phase 1
    // (idempotent self-heal + a post-cleanup OPENF/DYN_DELETED_G assert). SILENT skip off the knob-on FAT path.
    #[cfg(feature = "irqstorage")]
    s5_shared_backing_witness();
    // STOR-1 S6 carry-over (seat fold-in 1): witness MF2's immutable-code RW refusal (knob-on FAT, uncounted).
    #[cfg(feature = "irqstorage")]
    mf2_witness();
    // STOR-1 S7: witness an ARBITRARY on-disk file (README.TXT) opens + reads off the pre-stage set (knob-on FAT).
    #[cfg(feature = "irqstorage")]
    s7_openany_witness();
    // STOR-1 S8: witness a RW open of an arbitrary on-disk file (S8W.BIN) overwriting live off the pre-stage set.
    #[cfg(feature = "irqstorage")]
    s8_write_witness();
    // STOR-1 S9: witness a RW dynamic on-disk file (S9G.BIN) GROWING past EOF live (create -> grow -> stat/read).
    #[cfg(feature = "irqstorage")]
    s9_grow_witness();
    let Some(srow) = crate::arch::memory::alloc_user_space() else {
        serial_println!(":: U11m2: no free scratch slot — cross-process unlink demo skipped ::");
        return;
    };
    serial_println!(
        ":: U11m2: cross-process unlink-defers-free — a file another process holds open survives its unlink until the LAST close/teardown releases the deferred delete (pi4 U11 M2/M2b twin) ::"
    );
    let p1 = u11m2_phase(srow, demo_cpu, false, 1);
    // Phase 2 re-creates DEFER.BIN (legal again — phase 1's drain/release cleared the deleted flag) and releases
    // via the REAL teardown funnel, which also frees `srow` itself. On a phase-1 failure, still run it: it
    // release-cleans the scratch row either way (the unconditional-cleanup discipline).
    let p2 = u11m2_phase(srow, demo_cpu, true, 2);
    if !p2 {
        // A phase-2 failure may have exited on a leg BEFORE its teardown-release — tear the scratch row down
        // unconditionally so neither the slot nor a lingering descriptor/refcount strands for the boot
        // (idempotent if phase 2 already released: the row and handle row are simply already clear).
        crate::arch::memory::free_user_space_by_cr3(crate::arch::memory::slot_cr3(srow));
    }
    if p1 && p2 {
        if HELLO_STAGED.load(Ordering::Acquire) {
            serial_println!(
                ":: U11m2: cross-process unlink-defers-free — open-across-rows + read-after-unlink OK, delete op HELD past unlink, released at last CLOSE and at TEARDOWN, re-create -EBUSY while pending, DELETED on FAT (gone + chain freed all copies + re-allocatable + name re-creatable) -> PASS ::"
            );
        } else {
            serial_println!(
                ":: U11m2: cross-process unlink-defers-free — open-across-rows + read-after-unlink OK, released at last CLOSE and at TEARDOWN, re-create -EBUSY while pending (in-memory core; no FAT volume, delete-flush is a no-op) -> PASS ::"
            );
        }
    } else {
        serial_println!(":: U11m2: cross-process unlink-defers-free FAIL — p1={} p2={} ::", p1, p2);
    }
    // STOR-1 S6b: prove the NAMESPACE lock HOLDS (cross-core, in-RAM, no submit). Runs BEFORE u6gx spawns its
    // cooperative spinners (no IF=0 spinner co-located → no deadlock class). Knob-on FAT only (byte-identical off).
    #[cfg(feature = "irqstorage")]
    s6_witness_launcher(demo_cpu);
    // U6x: chain the owner/grants ACL demo (program order, the u9x->..->u11m2 idiom; the LAST demo in the chain).
    u6gx_launcher(demo_cpu);
}

/// Build a U6x fixture slot at a given entry symbol — the `u7x_build`/`u11m2_build` shape (allocate a private
/// address space, scrub the WHOLE window, copy the shared blob into the RX-RO code page through the identity
/// alias, and return the run params for `entry_sym`). `None` if slot allocation fails.
fn u6gx_build(entry_sym: *const u8) -> Option<U7xFix> {
    let slot = crate::arch::memory::alloc_user_space()?;
    let bstart = &raw const unaos_user_u6gx_blob_start as usize;
    let bend = &raw const unaos_user_u6gx_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen as u64 <= PAGE_SIZE, "U6x blob does not fit in a code page");
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

/// U6x: set fixture `slot`'s GO word (launcher -> fixture: the next step it may proceed past) through the slot
/// backing (the fixture polls the same VA in its window).
fn u6gx_set_go(slot: usize, step: u64) {
    let p = unsafe { crate::arch::memory::slot_backing_ptr(slot).add(U6GX_GO_OFF) as *mut u64 };
    unsafe { core::ptr::write_volatile(p, step) };
}

/// U6x: read fixture `slot`'s SIG word (fixture -> launcher: the last step it completed).
fn u6gx_get_sig(slot: usize) -> u64 {
    let p = unsafe { crate::arch::memory::slot_backing_ptr(slot).add(U6GX_SIG_OFF) as *const u64 };
    unsafe { core::ptr::read_volatile(p) }
}

/// U6x: wait (bounded, yielding) until fixture `slot`'s SIG word reaches `step`. Returns false on timeout (a
/// wedged fixture — the verdict then FAILs honestly on the witness), so the launcher never hangs the boot.
fn u6gx_wait_sig(slot: usize, step: u64) -> bool {
    let deadline = crate::arch::ticks() + 5000;
    while u6gx_get_sig(slot) < step && crate::arch::ticks() < deadline {
        crate::arch::sched::yield_now();
    }
    u6gx_get_sig(slot) >= step
}

/// U6x launcher + verdict — UnaFS owner/grants, the x86 twin of the reviewed aarch64 U6 (owned-by-default at
/// SYS_OPEN + SYS_FGRANT delegation + F1 owner-only unlink), folding pi4's post-hoc F1 fix from the start and
/// asserting the U11x M2 combined path (owner unlink while a grantee holds open -> deferred; re-create -EBUSY;
/// ownership dies at the last close). Two EL0 processes A (owner) + B (grantee) on their own cores (cooperative
/// ring 3 hogs its core, so — like U7x — each needs a dedicated AP; the launcher on a third), choreographed with
/// per-slot GO/SIG words. Chained off `u11m2_launcher` (LAST in the demo chain). Needs 3 online APs + storage;
/// fewer skips cleanly. Every path releases both slots + B's Proc entry before returning (no strand).
fn u6gx_launcher(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // keep the no-storage control path free of demo lines (mirrors every prior gate)
    }
    let nameid = match u10_name_id(U6GX_NAME) {
        Some(id) => id as usize,
        None => return,
    };
    // A dedicated core each for A and B (distinct from `demo_cpu` and this launcher): a polling ring-3 fixture
    // hogs its core, so co-locating would deadlock the GO/SIG sequencing (the u7x structural divergence).
    let online = crate::arch::smp::online_aps();
    let (Some(&cpu_a), Some(&cpu_b)) = (online.first(), online.get(2)) else {
        serial_println!(":: U6gx: fewer than 3 application processors — owner/grants demo skipped ::");
        return;
    };
    // B's Proc entry FIRST (A's Child handle -> B resolves pid->slot in SYS_FGRANT); nothing else claimed on fail.
    let Some(pi) = proc_reserve() else {
        serial_println!(":: U6gx: no free process entry — owner/grants demo skipped ::");
        return;
    };
    // Build + spawn B (the grantee); it parks on its GO word immediately, so it populates nothing pre-grant.
    let Some(b) = u6gx_build(&raw const unaos_user_u6gx_grantee) else {
        serial_println!(":: U6gx: no free address-space slot (B) — owner/grants demo skipped ::");
        proc_free(pi);
        return;
    };
    U6GX_OWNER_WITNESS.store(0, Ordering::Release);
    U6GX_GRANTEE_WITNESS.store(0, Ordering::Release);
    // STOR-1 S5: knob-on, u6gx's created READS route through the storage service task (C2). Owner A busy-spins
    // on its GO word on the SERVICE TASK'S CORE (both take `online.first()`); a NON-preemptible spin (IF=0,
    // `spawn_user_in_space`) would then STARVE the service task, so grantee B's cross-core granted read — which
    // blocks on it — never completes (a deadlock: launcher waits B, B waits the service task, the service task
    // waits behind A's IF=0 spin; the PRIO_HIGH service task cannot preempt an IF=0 ring-3 task). So spawn the
    // fixtures PREEMPTIBLE (RFLAGS.IF set — the timer evicts the spinner so the PRIO_HIGH service task runs).
    // Gated on `s4_sync_storage()` == "reads route live" (knob-on + FAT + service): knob-off / no-FAT reads serve
    // wstage (no service task, no deadlock) and keep the BYTE-IDENTICAL non-preemptible spawn. u6gx is
    // register-only (no FP/SIMD across a preemptible switch — the ledgered unsaved-FP gap is not reachable here).
    // The KillSwitch is a watchdog safety net; the fixtures exit normally (SYS_EXIT), so it is unused.
    let preempt = s4_sync_storage();
    let b_pid = if preempt {
        crate::arch::sched::spawn_user_preemptible(
            "u6gx-grantee", b.entry, b.sp, cpu_b, b.cr3,
            alloc::sync::Arc::new(crate::arch::sched::KillSwitch::new()),
        )
    } else {
        crate::arch::sched::spawn_user_in_space("u6gx-grantee", b.entry, b.sp, cpu_b, b.cr3)
    };
    PROCS[pi].slot.store(b.slot + 1, Ordering::Release); // slot first, then the live pid (the sys_spawn discipline)
    PROCS[pi].pid.store(b_pid, Ordering::Release);
    // Build + pre-endow + spawn A (the owner). Pre-endow A with a Child handle naming B (owner-scoped SYS_FGRANT).
    let Some(a) = u6gx_build(&raw const unaos_user_u6gx_owner) else {
        serial_println!(":: U6gx: no free address-space slot (A) — owner/grants demo skipped (B parks out) ::");
        proc_free(pi);
        return;
    };
    debug_assert!(a.slot != b.slot, "u6x: A and B landed on the same slot");
    install_cap(a.slot, U6GX_CHILD_IDX, KIND_CHILD, b_pid, CAP_READ);
    serial_println!(
        ":: U6gx: UnaFS owner/grants — the by-NAME ACL at SYS_OPEN (owned-by-default) + SYS_FGRANT delegation + F1 owner-only unlink (aarch64 U6 twin) ::"
    );
    // A rides the same `preempt` gate as B (above) — knob-on it must be preemptible so the service task can run
    // while A spins on the service core (A is `online.first()`, == the service task's core).
    if preempt {
        crate::arch::sched::spawn_user_preemptible(
            "u6gx-owner", a.entry, a.sp, cpu_a, a.cr3,
            alloc::sync::Arc::new(crate::arch::sched::KillSwitch::new()),
        );
    } else {
        crate::arch::sched::spawn_user_in_space("u6gx-owner", a.entry, a.sp, cpu_a, a.cr3);
    }

    // The choreography (single GO/SIG per slot): A creates -> B pre-grant deny -> A grants -> B granted open +
    // negatives (keeps its handle open) -> A revokes -> B post-revoke deny -> A reopen+unlink(deferred)+EBUSY ->
    // B closes (releases). Each `wait_sig` returns false on timeout, short-circuiting to the verdict (which FAILs).
    let seq_ok = u6gx_wait_sig(a.slot, 1) && {
        u6gx_set_go(b.slot, 1);
        u6gx_wait_sig(b.slot, 1)
    } && {
        u6gx_set_go(a.slot, 1);
        u6gx_wait_sig(a.slot, 2)
    } && {
        u6gx_set_go(b.slot, 2);
        u6gx_wait_sig(b.slot, 2)
    } && {
        u6gx_set_go(a.slot, 2);
        u6gx_wait_sig(a.slot, 3)
    } && {
        u6gx_set_go(b.slot, 3);
        u6gx_wait_sig(b.slot, 3)
    } && {
        u6gx_set_go(a.slot, 3);
        u6gx_wait_sig(a.slot, 4) // A finished (reopen + deferred unlink + EBUSY), about to exit
    } && {
        u6gx_set_go(b.slot, 4); // release B's final close (which releases A's deferred delete)
        true
    };

    // Wait (bounded) for both witness exits, then snapshot the witnesses + kill count.
    let vdeadline = crate::arch::ticks() + 8000;
    while U6GX_DONE.load(Ordering::Acquire) < 2 && crate::arch::ticks() < vdeadline {
        crate::arch::sched::yield_now();
    }
    let ow = U6GX_OWNER_WITNESS.load(Ordering::Acquire);
    let gw = U6GX_GRANTEE_WITNESS.load(Ordering::Acquire);
    let killed = U6GX_KILLED.load(Ordering::Acquire);

    // Teardown proof: both fixtures exited holding no live descriptors (A unlinked; B closed), so both rows +
    // handle rows cleared. Poll bounded; false->true.
    let tdeadline = crate::arch::ticks() + 3000;
    let both_clear = |a: usize, b: usize| {
        files_row_is_clear(a) && handle_row_is_clear(a) && files_row_is_clear(b) && handle_row_is_clear(b)
    };
    while !both_clear(a.slot, b.slot) && crate::arch::ticks() < tdeadline {
        crate::arch::sched::yield_now();
    }
    let cleared = both_clear(a.slot, b.slot);

    // C — ownership DIED at B's last close: the global open count is 0 and the file is unlink-PENDING no more. On
    // the disk path the released delete op is now DRAINABLE — drain it at IF=1 (the queue holds exactly A's
    // OWNED.BIN delete) so nothing strands; on the no-FAT path the last close already cleared DYN_DELETED_G.
    // Either way the name is re-creatable again and the owner/grants row is EMPTY (public).
    let disk_present = HELLO_STAGED.load(Ordering::Acquire);
    // STOR-1 S6 carry-over (S4-review note 4): tighten the u6gx drain to a VERDICT (u11m2 already asserts it) —
    // knob-on the delete ran SYNCHRONOUSLY at B's last close, so `count == 0` (nothing enqueued); knob-off exactly
    // A's OWNED.BIN delete op replayed. Either way `released` now requires the drain verdict, not just a lenient sweep.
    let drain_ok = if disk_present {
        match crate::fs::fat::mount() {
            Ok(fs) => u10_drain_verdict(&fs).0,
            Err(_) => false,
        }
    } else {
        true // no-FAT: the last close already cleared DYN_DELETED_G; nothing to drain
    };
    let released = drain_ok
        && OPENF_REFS[nameid].load(Ordering::Acquire) == 0
        && !OPENF_PENDING[nameid].load(Ordering::Acquire)
        && !DYN_DELETED_G[nameid].load(Ordering::Acquire)
        && owned_is_public(nameid);

    proc_free(pi); // drop the planted pid->slot entry (the fixtures exited by name, not via the Proc path)

    if seq_ok && ow == U6GX_OWNER_ALL && gw == U6GX_GRANTEE_ALL && killed == 0 && cleared && released {
        serial_println!(
            ":: U6gx: UnaFS owner/grants — non-owner open -EACCES, owner grant admits R|W (content crossed), non-owner grant -EACCES, grantee unlink -EACCES (delete owner-only), revoke re-denies, owner unlink defers while grantee holds + re-create -EBUSY, ownership dies at last close -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U6gx: UnaFS owner/grants FAIL — seq={} owner={:#x} grantee={:#x} killed={} cleared={} released={} done={} (want true/{:#x}/{:#x}/0/true/true/2) ::",
            seq_ok, ow, gw, killed, cleared, released, U6GX_DONE.load(Ordering::Acquire),
            U6GX_OWNER_ALL, U6GX_GRANTEE_ALL,
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
    // U7x is an INLINE demo — both fixtures are inline blobs transferring a console cap, no disk. It runs
    // REGARDLESS of storage so the cross-process-transfer rung shows on the no-storage / metal path (it needs 3
    // online APs; fewer skips cleanly in u7x_run). (Scoped relaxation — U5x/U7x/U8x only.)
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
