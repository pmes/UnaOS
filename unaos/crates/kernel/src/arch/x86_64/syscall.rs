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

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use x86_64::registers::control::Cr4;
use x86_64::registers::model_specific::{LStar, Msr};
use x86_64::VirtAddr;

use crate::arch::percpu::{KERNEL_RSP_OFFSET, USER_RSP_OFFSET};

// --- Syscall numbers (the tiny U1a subset; mirrors aarch64). ---
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;

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
        SYS_EXIT => {
            // Accounting BEFORE the no-return exit, routed by task so the U1a/U1b, Part-0a, and U2
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
    if fd != 1 {
        return -9; // -EBADF (only stdout for U1a)
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
}
