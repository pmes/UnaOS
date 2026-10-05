// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD2 M3 (B349) — Linux SIGNALS for the Linux ABI shim: a per-process handler table, a blocked mask, a pending set,
//! delivery of a caught signal at the SYSCALL boundary, and a real `rt_sigreturn`.
//!
//! Where SELFBUILD1 accepted `rt_sigaction` and never delivered, a caught signal now runs its handler ONCE:
//! - `kill`/`tgkill` to a process whose action is a handler sets the pending bit (SIG_IGN drops it, SIG_DFL keeps the old
//!   fatal-or-ignore path); a child's exit posts SIGCHLD to a parent that catches it.
//! - On the way out of any syscall (`dispatch`), the lowest pending, unblocked signal is delivered: an `rt_sigframe` (Linux's
//!   layout: pretcode, `ucontext` with the full `sigcontext`, `siginfo`, a 512-byte FXSAVE image) is written below the user
//!   stack's red zone, and the SYSCALL return frame is rewritten to enter the handler. The SYSCALL stub scrubs rdi/rsi/rdx on
//!   the way out (U1b B1), so the handler is entered through a 21-byte TRAMPOLINE page this layer maps at [`TRAMP_VA`]
//!   (`mov rdi,rbx; mov rsi,r12; mov rdx,r13; jmp r14` — the arguments ride the callee-saved registers the stub restores),
//!   followed by a default restorer (`mov eax,15; syscall`) for a handler installed without SA_RESTORER.
//! - `rt_sigreturn` reads the `ucontext` back (rip, rsp, rflags, rbx/rbp/r12-r15, rax, the mask, the FXSAVE image) and returns
//!   to the interrupted point. A blocking syscall is interrupted with `-EINTR` unless the handler has SA_RESTART (then it keeps
//!   blocking and the handler runs when it completes); `rt_sigsuspend`/`pause` always end in `-EINTR`.
//!
//! Limits (stated, not hidden): the mask is per PROCESS (not per thread); a blocked SIG_DFL signal is not queued (its default
//! action applies at once); no alternate signal stack; no real-time queueing (one pending bit per signal); delivery happens at
//! a syscall boundary only (a thread spinning in ring 3 sees its signal at its next syscall).

use super::proc::ProcInfo;
use super::{fd, sys, LinuxProc};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

const EINTR: i64 = 4;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;
const SA_RESTORER: u64 = 0x0400_0000;
const SA_RESTART: u64 = 0x1000_0000;
const SA_NODEFER: u64 = 0x4000_0000;
const SA_RESETHAND: u64 = 0x8000_0000;
/// SIGKILL and SIGSTOP can be neither caught nor blocked.
const UNBLOCKABLE: u64 = (1 << 8) | (1 << 18);
/// The signal trampoline page: just above the stack top, inside PML4[2] (the process-private window).
pub const TRAMP_VA: u64 = super::STACK_TOP + super::PAGE;
const TRAMP: [u8; 21] = [
    0x48, 0x89, 0xdf, // mov rdi, rbx   (signal number)
    0x4c, 0x89, 0xe6, // mov rsi, r12   (&siginfo)
    0x4c, 0x89, 0xea, // mov rdx, r13   (&ucontext)
    0x41, 0xff, 0xe6, // jmp r14        (the handler; its return address is the frame's pretcode)
    0xb8, 0x0f, 0x00, 0x00, 0x00, // TRAMP_VA+12: the default restorer — mov eax, 15 (rt_sigreturn)
    0x0f, 0x05, // syscall
    0x0f, 0x0b, // ud2
];
const UC_SIZE: usize = 304;
const FRAME_SIZE: u64 = 8 + UC_SIZE as u64 + 128;

#[derive(Clone, Copy, Default)]
struct Act {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
}

struct SigProc {
    pid: u32,
    acts: [Act; 64],
    mask: u64,
    pending: u64,
    /// `rt_sigsuspend`'s saved mask while it waits (restored through the frame's `uc_sigmask`).
    suspend_saved: Option<u64>,
}

static SIGS: spin::Mutex<Vec<SigProc>> = spin::Mutex::new(Vec::new());
pub static DELIVERED: AtomicU64 = AtomicU64::new(0);
pub static RETURNS: AtomicU64 = AtomicU64::new(0);

fn with<R>(pid: u32, f: impl FnOnce(&mut SigProc) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = SIGS.lock();
        let i = match g.iter().position(|s| s.pid == pid) {
            Some(i) => i,
            None => {
                g.push(SigProc { pid, acts: [Act::default(); 64], mask: 0, pending: 0, suspend_saved: None });
                g.len() - 1
            }
        };
        f(&mut g[i])
    })
}

fn peek<R>(pid: u32, f: impl FnOnce(Option<&SigProc>) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let g = SIGS.lock();
        f(g.iter().find(|s| s.pid == pid))
    })
}

fn bit(sig: u64) -> u64 {
    1u64 << (sig - 1)
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap_or([0; 8]))
}

/// `rt_sigaction(sig, act, oact, sigsetsize)` (13): the per-process table. Old action written back before the new is stored.
pub fn sigaction(p: &mut LinuxProc, info: &ProcInfo, a: [u64; 6]) -> i64 {
    if a[3] != 8 {
        return -EINVAL; // the kernel's sigset_t is 8 bytes on x86_64
    }
    let sig = a[0];
    if sig == 0 || sig > 64 || (a[1] != 0 && (sig == 9 || sig == 19)) {
        return -EINVAL; // SIGKILL/SIGSTOP cannot be caught
    }
    let mut new = None;
    if a[1] != 0 {
        let mut b = [0u8; 32];
        if !p.asp.copy_in(a[1], &mut b) {
            return -EFAULT;
        }
        new = Some(Act { handler: u64_at(&b, 0), flags: u64_at(&b, 8), restorer: u64_at(&b, 16), mask: u64_at(&b, 24) & !UNBLOCKABLE });
    }
    let old = peek(info.pid, |s| s.map(|s| s.acts[(sig - 1) as usize]).unwrap_or_default());
    if a[2] != 0 {
        let mut o = [0u8; 32];
        o[0..8].copy_from_slice(&old.handler.to_le_bytes());
        o[8..16].copy_from_slice(&old.flags.to_le_bytes());
        o[16..24].copy_from_slice(&old.restorer.to_le_bytes());
        o[24..32].copy_from_slice(&old.mask.to_le_bytes());
        if !p.asp.copy_out(a[2], &o, false) {
            return -EFAULT;
        }
    }
    if let Some(n) = new {
        with(info.pid, |s| {
            s.acts[(sig - 1) as usize] = n;
            if n.handler == 1 {
                s.pending &= !bit(sig); // SIG_IGN discards a pending one
            }
        });
    }
    0
}

/// `rt_sigprocmask(how, set, oset, sigsetsize)` (14): a real blocked mask (per process).
pub fn sigprocmask(p: &mut LinuxProc, info: &ProcInfo, a: [u64; 6]) -> i64 {
    if a[3] != 8 {
        return -EINVAL;
    }
    let mut set = None;
    if a[1] != 0 {
        if a[0] > 2 {
            return -EINVAL; // how: SIG_BLOCK / SIG_UNBLOCK / SIG_SETMASK
        }
        let mut b = [0u8; 8];
        if !p.asp.copy_in(a[1], &mut b) {
            return -EFAULT;
        }
        set = Some(u64::from_le_bytes(b));
    }
    let old = peek(info.pid, |s| s.map_or(0, |s| s.mask));
    if a[2] != 0 && !p.asp.copy_out(a[2], &old.to_le_bytes(), false) {
        return -EFAULT;
    }
    if let Some(v) = set {
        let how = a[0];
        with(info.pid, |s| {
            s.mask = match how {
                0 => s.mask | v,
                1 => s.mask & !v,
                _ => v,
            } & !UNBLOCKABLE;
        });
    }
    0
}

/// `rt_sigsuspend(mask, sigsetsize)` (130): swap the mask in and wait; the dispatch loop ends it with `-EINTR` at the first
/// deliverable signal, and the delivery restores the old mask through the frame.
pub fn sigsuspend(p: &mut LinuxProc, info: &ProcInfo, a: [u64; 6]) -> i64 {
    if peek(info.pid, |s| s.is_some_and(|s| s.suspend_saved.is_some())) {
        return sys::RETRY;
    }
    if a[1] != 8 {
        return -EINVAL;
    }
    let mut b = [0u8; 8];
    if !p.asp.copy_in(a[0], &mut b) {
        return -EFAULT;
    }
    let m = u64::from_le_bytes(b) & !UNBLOCKABLE;
    with(info.pid, |s| {
        s.suspend_saved = Some(s.mask);
        s.mask = m;
    });
    sys::RETRY
}

/// `kill`/`tgkill`: `true` = the signal was taken by this table (queued for a handler, or dropped by SIG_IGN) and must not
/// take the default (fatal) path. A process that never touched its handlers has no entry: default action.
pub fn post(pid: u32, sig: u64) -> bool {
    if sig == 0 || sig > 64 || pid == 0 {
        return false;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = SIGS.lock();
        let Some(s) = g.iter_mut().find(|s| s.pid == pid) else { return false };
        match s.acts[(sig - 1) as usize].handler {
            0 => false,
            1 => true,
            _ => {
                s.pending |= bit(sig);
                true
            }
        }
    })
}

/// The lowest pending unblocked signal of `pid` and whether its handler asked SA_RESTART.
fn deliverable(pid: u32) -> Option<(u64, bool)> {
    peek(pid, |s| {
        let s = s?;
        let ready = s.pending & !s.mask;
        if ready == 0 {
            return None;
        }
        let sig = ready.trailing_zeros() as u64 + 1;
        Some((sig, s.acts[(sig - 1) as usize].flags & SA_RESTART != 0))
    })
}

/// The dispatch RETRY loop asks this between tries: does a pending signal end this blocking syscall (`-EINTR`)?
pub fn interrupts(info: &ProcInfo, nr: u64) -> bool {
    match deliverable(info.pid) {
        Some((_, restart)) => nr == 130 || nr == 34 || !restart,
        None => false,
    }
}

pub const fn eintr() -> i64 {
    -EINTR
}

fn fw(ktop: u64, off: u64) -> u64 {
    unsafe { *((ktop - off) as *const u64) }
}

fn fset(ktop: u64, off: u64, v: u64) {
    unsafe { *((ktop - off) as *mut u64) = v };
}

fn ensure_tramp(p: &mut LinuxProc) -> bool {
    if p.asp.is_mapped(TRAMP_VA) {
        let mut b = [0u8; 21];
        return p.asp.copy_in(TRAMP_VA, &mut b) && b == TRAMP;
    }
    if !p.asp.map_new(TRAMP_VA, false, true) {
        return false;
    }
    let ok = p.asp.copy_out(TRAMP_VA, &TRAMP, true);
    unsafe { super::memory::load_cr3(super::memory::current_cr3()) };
    ok
}

/// On the way out of syscall `a` (result `rc`): deliver the lowest pending unblocked caught signal, if any. Returns what rax
/// must carry (0 into a handler, else `rc` unchanged).
pub fn deliver(info: &ProcInfo, ktop: u64, a: [u64; 6], rc: i64) -> i64 {
    let Some((sig, _)) = deliverable(info.pid) else { return rc };
    let (act, oldmask) = with(info.pid, |s| {
        s.pending &= !bit(sig);
        let act = s.acts[(sig - 1) as usize];
        let old = s.suspend_saved.take().unwrap_or(s.mask);
        if act.handler > 1 {
            s.mask = (old | act.mask | if act.flags & SA_NODEFER != 0 { 0 } else { bit(sig) }) & !UNBLOCKABLE;
            if act.flags & SA_RESETHAND != 0 {
                s.acts[(sig - 1) as usize] = Act::default();
            }
        } else {
            s.mask = old;
        }
        (act, old)
    });
    if act.handler < 2 {
        return rc; // reset to SIG_DFL / SIG_IGN since it was posted
    }
    let mut p = fd::lk(&info.lp);
    let usp = fw(ktop, 16);
    let fp = usp.wrapping_sub(128).wrapping_sub(512) & !63;
    let frame = (fp.wrapping_sub(FRAME_SIZE) & !15).wrapping_sub(8);
    let (uc_va, si_va) = (frame + 8, frame + 8 + UC_SIZE as u64);
    let mut img = [0u8; 512];
    let has_fp = super::fpu::save_live(&mut img);
    let rflags = fw(ktop, 24);
    let rip = fw(ktop, 32);
    let mut f = [0u8; FRAME_SIZE as usize];
    let restorer = if act.flags & SA_RESTORER != 0 && act.restorer != 0 { act.restorer } else { TRAMP_VA + 12 };
    f[0..8].copy_from_slice(&restorer.to_le_bytes());
    let uc = 8usize;
    f[uc + 24..uc + 28].copy_from_slice(&2u32.to_le_bytes()); // uc_stack.ss_flags = SS_DISABLE
    let mc = uc + 40;
    let sels = (crate::arch::gdt::USER_CODE_SEL as u64) | ((crate::arch::gdt::USER_DATA_SEL as u64) << 48);
    let regs: [u64; 24] = [
        a[4], a[5], a[3], rflags, // r8 r9 r10 r11
        fw(ktop, 56), fw(ktop, 64), fw(ktop, 72), fw(ktop, 80), // r12 r13 r14 r15
        a[0], a[1], fw(ktop, 48), fw(ktop, 40), a[2], rc as u64, rip, usp, // rdi rsi rbp rbx rdx rax rcx rsp
        rip, rflags, sels, 0, 0, oldmask, 0, if has_fp { fp } else { 0 }, // rip eflags cs..ss err trapno oldmask cr2 fpstate
    ];
    for (i, r) in regs.iter().enumerate() {
        f[mc + i * 8..mc + i * 8 + 8].copy_from_slice(&r.to_le_bytes());
    }
    f[uc + 296..uc + 304].copy_from_slice(&oldmask.to_le_bytes());
    let si = 8 + UC_SIZE;
    f[si..si + 4].copy_from_slice(&(sig as i32).to_le_bytes()); // si_signo; si_errno 0; si_code 0 = SI_USER
    f[si + 16..si + 20].copy_from_slice(&info.pid.to_le_bytes()); // si_pid (sender: this layer does not track it — the receiver)
    let ok = ensure_tramp(&mut p) && (!has_fp || p.asp.copy_out(fp, &img, false)) && p.asp.copy_out(frame, &f, false);
    drop(p);
    if !ok {
        // No room for the frame (or no trampoline): Linux forces SIGSEGV.
        serial_println!("[linuxabi] signal {} pid={}: frame at {:#x} unwritable -> SIGSEGV", sig, info.pid, frame);
        super::thread::kill_group(info.pid);
        info.killsig.store(11, Ordering::Release);
        info.kill.request();
        return rc;
    }
    fset(ktop, 8, frame);
    fset(ktop, 16, frame);
    fset(ktop, 24, rflags & !(0x100 | 0x400)); // TF, DF clear on handler entry
    fset(ktop, 32, TRAMP_VA);
    fset(ktop, 40, sig); // rbx -> rdi
    fset(ktop, 56, si_va); // r12 -> rsi (Linux passes &siginfo whether or not SA_SIGINFO asked)
    fset(ktop, 64, uc_va); // r13 -> rdx
    fset(ktop, 72, act.handler); // r14 -> jmp
    DELIVERED.fetch_add(1, Ordering::AcqRel);
    serial_println!("[linuxabi] signal {} pid={} -> handler {:#x} frame={:#x}", sig, info.pid, act.handler, frame);
    0
}

/// `rt_sigreturn` (15): restore the context the delivery saved (the frame's `ucontext` sits at the user rsp: the handler's
/// `ret` popped the pretcode). Returns the interrupted syscall's rax.
pub fn sigreturn(p: &mut LinuxProc, info: &ProcInfo, ktop: u64) -> i64 {
    let usp = fw(ktop, 16);
    let mut uc = [0u8; UC_SIZE];
    if !p.asp.copy_in(usp, &mut uc) {
        super::thread::kill_group(info.pid);
        info.killsig.store(11, Ordering::Release);
        info.kill.request();
        return -EFAULT;
    }
    let mc = |i: usize| u64_at(&uc, 40 + i * 8);
    let fpst = mc(23);
    let mut img = [0u8; 512];
    let fp_ok = fpst != 0 && p.asp.copy_in(fpst, &mut img);
    fset(ktop, 40, mc(11)); // rbx
    fset(ktop, 48, mc(10)); // rbp
    fset(ktop, 56, mc(4)); // r12
    fset(ktop, 64, mc(5)); // r13
    fset(ktop, 72, mc(6)); // r14
    fset(ktop, 80, mc(7)); // r15
    fset(ktop, 32, mc(16)); // rip (the stub refuses a non-canonical one)
    fset(ktop, 8, mc(15)); // rsp
    fset(ktop, 16, mc(15));
    fset(ktop, 24, (mc(17) & 0xCD5) | 0x202); // CF PF AF ZF SF DF OF from the frame; IF on
    let mask = u64_at(&uc, 296) & !UNBLOCKABLE;
    with(info.pid, |s| s.mask = mask);
    if fp_ok {
        super::fpu::restore_live(&img);
    }
    RETURNS.fetch_add(1, Ordering::AcqRel);
    let rax = mc(13) as i64;
    if rax == sys::RETRY { 0 } else { rax }
}

/// fork: the child inherits the handler table and the mask (not the pending set).
pub fn fork_copy(parent: u32, child: u32) {
    let st = peek(parent, |s| s.map(|s| (s.acts, s.mask)));
    if let Some((acts, mask)) = st {
        with(child, |s| {
            s.acts = acts;
            s.mask = mask;
        });
    }
}

/// execve: caught signals return to SIG_DFL (SIG_IGN and the mask survive, as on Linux).
pub fn exec_reset(pid: u32) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = SIGS.lock();
        if let Some(s) = g.iter_mut().find(|s| s.pid == pid) {
            for a in s.acts.iter_mut() {
                if a.handler > 1 {
                    *a = Act::default();
                }
            }
            s.suspend_saved = None;
        }
    });
}

pub fn forget(pid: u32) {
    x86_64::instructions::interrupts::without_interrupts(|| SIGS.lock().retain(|s| s.pid != pid));
}

pub fn reset() {
    x86_64::instructions::interrupts::without_interrupts(|| SIGS.lock().clear());
    DELIVERED.store(0, Ordering::Release);
    RETURNS.store(0, Ordering::Release);
}
