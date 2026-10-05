// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD1 M2 — the Linux syscalls a STATIC TOOLCHAIN reaches beyond the LINUXABI1–3 table (`sys.rs`).
//!
//! Measured, not guessed: `strace` of the staged binaries on the host (tcc 0.9.28rc on glibc 2.39 static, busybox 1.37 on
//! glibc 2.39 static) named the surface; this file answers what `sys.rs` did not. `sys::handle`'s fall-through arm asks
//! [`handle`] before it says `-ENOSYS`, and its `rt_sigaction`/`rt_sigprocmask`/`readlink*` arms route here.
//! Signals are ACCEPTED, never delivered (no signal frames in this layer): the old values written back say "default action,
//! nothing blocked, no alternate stack", the truth for a process that is never signalled. `rseq` and `clone3` answer `-ENOSYS`
//! ON PURPOSE (the kernels before 4.18 / 5.3 did; glibc then falls back to no-rseq / `clone`), so they are not noted as gaps.

use super::proc::ProcInfo;
use super::LinuxProc;
use alloc::sync::Arc;

const ENOENT: i64 = 2;
const EBADF: i64 = 9;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;
const ENOSYS: i64 = 38;
const ENOTSOCK: i64 = 88;
const SELF_EXE: &[u8] = b"/proc/self/exe";

/// `true` when the NUL-terminated user string at `va` is `/proc/self/exe` (readlink and execve answer it with this image).
pub fn is_self_exe(p: &LinuxProc, va: u64) -> bool {
    p.asp.read_cstr(va, 64).is_some_and(|s| s == SELF_EXE)
}

/// `readlink(path, buf, sz)` (89) / `readlinkat(dirfd, path, buf, sz)` (267). `/proc/self/exe` = the image path; every other
/// existing path is "not a symlink" (`-EINVAL`, the VFS has no symlinks), a missing one `-ENOENT`.
pub fn readlink(p: &mut LinuxProc, nr: u64, a: [u64; 6]) -> i64 {
    let (dirfd, path, buf, sz) = if nr == 89 { (super::sys::AT_FDCWD, a[0], a[1], a[2]) } else { (a[0] as i64, a[1], a[2], a[3]) };
    if (sz as i64) <= 0 {
        return -EINVAL;
    }
    if is_self_exe(p, path) {
        let e = p.exe.clone();
        let n = e.len().min(sz.min(4096) as usize);
        return if p.asp.copy_out(buf, &e.as_bytes()[..n], false) { n as i64 } else { -EFAULT };
    }
    match super::sys::resolve(p, dirfd, path) {
        Ok(full) => {
            if crate::shell::vfs_mount_table().stat(&full).is_ok() || full == "/" {
                -EINVAL
            } else {
                -ENOENT
            }
        }
        Err(e) => e,
    }
}

/// `rt_sigaction(sig, act, oact, sz)` (13) / `rt_sigprocmask(how, set, oset, sz)` (14): accepted, never delivered.
pub fn sigs(p: &mut LinuxProc, nr: u64, a: [u64; 6]) -> i64 {
    if a[3] != 8 {
        return -EINVAL; // the kernel's sigset_t is 8 bytes on x86_64
    }
    if nr == 13 {
        let sig = a[0];
        if sig == 0 || sig > 64 || (a[1] != 0 && (sig == 9 || sig == 19)) {
            return -EINVAL; // SIGKILL/SIGSTOP cannot be caught
        }
        // struct kernel_sigaction { handler, flags, restorer, mask } = 32 bytes: SIG_DFL, no flags.
        if a[2] != 0 && !p.asp.copy_out(a[2], &[0u8; 32], false) {
            return -EFAULT;
        }
        return 0;
    }
    if a[1] != 0 && a[0] > 2 {
        return -EINVAL; // how: SIG_BLOCK / SIG_UNBLOCK / SIG_SETMASK
    }
    if a[2] != 0 && !p.asp.copy_out(a[2], &[0u8; 8], false) {
        return -EFAULT;
    }
    0
}

fn put(p: &LinuxProc, va: u64, b: &[u8]) -> i64 {
    if p.asp.copy_out(va, b, false) { 0 } else { -EFAULT }
}

/// `clone` (fork-shaped) tid stores: CLONE_PARENT_SETTID writes the child's tid at `a[2]` in the parent, CLONE_CHILD_SETTID at
/// `a[3]` in the child (the child is pinned to this core and has not run yet: the forking syscall does not yield).
pub fn clone_settid(p: &mut LinuxProc, flags: u64, a: [u64; 6], pid: i64) {
    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
    if pid <= 0 {
        return;
    }
    let tid = (pid as u32).to_le_bytes();
    if flags & CLONE_PARENT_SETTID != 0 && a[2] != 0 {
        let _ = p.asp.copy_out(a[2], &tid, false);
    }
    if flags & CLONE_CHILD_SETTID != 0 && a[3] != 0 {
        if let Some(c) = super::proc::snapshot().into_iter().find(|i| i.pid as i64 == pid) {
            let _ = super::fd::lk(&c.lp).asp.copy_out(a[3], &tid, false);
        }
    }
}

/// The SELFBUILD1 additions. `None` = not answered here either (the caller notes `-ENOSYS`).
pub fn handle(p: &mut LinuxProc, info: &Arc<ProcInfo>, nr: u64, a: [u64; 6]) -> Option<i64> {
    let _ = info;
    Some(match nr {
        // sigaltstack(ss, oss): stack_t { sp, flags:i32, size } = 24 bytes; old = SS_DISABLE (2).
        131 => {
            if a[1] != 0 {
                let mut o = [0u8; 24];
                o[8..12].copy_from_slice(&2u32.to_le_bytes());
                if put(p, a[1], &o) != 0 {
                    return Some(-EFAULT);
                }
            }
            if a[0] != 0 {
                let mut s = [0u8; 24];
                if !p.asp.copy_in(a[0], &mut s) {
                    return Some(-EFAULT);
                }
            }
            0
        }
        334 | 435 => -ENOSYS, // rseq / clone3: deliberate (see the module note), never noted as a gap
        // sched_getaffinity(pid, len, mask): one CPU — every Linux process is pinned to the verb's core.
        204 => {
            if a[1] < 8 || a[1] & 7 != 0 {
                return Some(-EINVAL);
            }
            let r = put(p, a[2], &1u64.to_le_bytes());
            if r != 0 { r } else { 8 }
        }
        203 => 0,                             // sched_setaffinity: accepted (pinned anyway)
        135 => 0,                             // personality: PER_LINUX
        160 => 0,                             // setrlimit: accepted
        90 | 268 => {
            // chmod / fchmodat: the VFS keeps no mode bits — accepted when the path exists
            let (dirfd, path) = if nr == 90 { (super::sys::AT_FDCWD, a[0]) } else { (a[0] as i64, a[1]) };
            match super::sys::resolve(p, dirfd, path) {
                Ok(full) if crate::shell::vfs_mount_table().stat(&full).is_ok() => 0,
                Ok(_) => -ENOENT,
                Err(e) => e,
            }
        }
        91 | 93 => {
            // fchmod / fchown: accepted on an open descriptor
            if p.fds.get(a[0] as usize).is_some_and(|s| s.is_some()) { 0 } else { -EBADF }
        }
        92 | 94 | 260 => 0, // chown / lchown / fchownat: one user, accepted
        157 => match a[0] {
            15 => 0, // PR_SET_NAME
            16 => put(p, a[1], b"linuxabi\0\0\0\0\0\0\0\0"),
            _ => -EINVAL,
        },
        98 => put(p, a[1], &[0u8; 144]), // getrusage: zero
        100 => {
            // times(buf): zero CPU times; returns clock ticks (CLK_TCK 100)
            if a[0] != 0 && put(p, a[0], &[0u8; 32]) != 0 {
                return Some(-EFAULT);
            }
            (crate::arch::ms() / 10) as i64
        }
        201 => {
            let s = (crate::arch::ms() / 1000) as i64;
            if a[0] != 0 && put(p, a[0], &s.to_le_bytes()) != 0 {
                return Some(-EFAULT);
            }
            s
        }
        229 => {
            // clock_getres: 1 ms (arch::ms)
            if a[1] != 0 {
                let mut t = [0u8; 16];
                t[8..16].copy_from_slice(&1_000_000u64.to_le_bytes());
                if put(p, a[1], &t) != 0 {
                    return Some(-EFAULT);
                }
            }
            0
        }
        51 | 52 => {
            // getsockname / getpeername: no descriptor of this layer is a socket
            if p.fds.get(a[0] as usize).is_some_and(|s| s.is_some()) { -ENOTSOCK } else { -EBADF }
        }
        _ => return None,
    })
}
