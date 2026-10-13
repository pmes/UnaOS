// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// STRIAMETAL (SR139): the syscall stubs and the file read — user-aether's sys.rs (AETHERMETAL SR66) shape, with the
// file sized by SYS_STAT first so a media file is read into ONE allocation (a growing Vec would hold two size classes
// of a large file at once). x86_64: the kernel's `sysretq` tail scrubs rdi/rsi/rdx/r8/r9/r10 on every return, so all
// are declared clobbered.
//
// FILES. `SYS_PATH_READ` (59, absolute paths through the kernel VFS; `stria` implies `selfdiag`, which serves it on
// both arches), and, when that verb is absent (ENOSYS), the classic `SYS_OPEN` + `SYS_READ`.

use alloc::vec::Vec;
use una_abi::{ENOSYS, PATH_IO_HDR_LEN, PATH_IO_MAX, PATH_IO_PATH_MAX, SYS_CLOSE, SYS_OPEN, SYS_PATH_READ, SYS_READ, SYS_STAT, USER_STAT_LEN};

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn sys4(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let mut r: u64;
    unsafe { core::arch::asm!("svc #0", inout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x8") n, options(nostack)) };
    r
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn sys4(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let mut r: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") n => r,
            inlateout("rdi") a0 => _,
            inlateout("rsi") a1 => _,
            inlateout("rdx") a2 => _,
            inlateout("r10") a3 => _,
            lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _,
            options(nostack),
        )
    };
    r
}

/// Raw syscall: the kernel's answer as a signed value (negative = errno).
#[inline(never)]
pub fn sys(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    unsafe { sys4(n, a0, a1, a2, a3) as i64 }
}

pub fn sleep_ms(ms: u64) {
    sys(una_abi::SYS_SLEEP_MS, ms, 0, 0, 0);
}

/// Bytes to the console (fd 1 → the serial wire).
pub fn write(b: &[u8]) {
    sys(una_abi::SYS_WRITE, 1, b.as_ptr() as u64, b.len() as u64, 0);
}

/// Milliseconds since boot (SYS_GETINFO's ticks through the per-arch rate) — the wall clock.
pub fn now_ms() -> u64 {
    let mut info = [0u64; 2];
    if sys(una_abi::SYS_GETINFO, info.as_mut_ptr() as u64, 0, 0, 0) < 0 {
        return 0;
    }
    una_abi::getinfo_ticks_to_ms(info[1])
}

pub fn exit(code: u64) -> ! {
    sys(una_abi::SYS_EXIT, code, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// `SYS_STAT(path)`: the file's size, or -errno (a directory is -EISDIR).
pub fn stat_size(path: &[u8]) -> Result<u64, i64> {
    let mut st = [0u8; USER_STAT_LEN];
    let r = sys(SYS_STAT, path.as_ptr() as u64, path.len() as u64, st.as_mut_ptr() as u64, 0);
    if r < 0 {
        return Err(r);
    }
    if u32::from_le_bytes([st[0], st[1], st[2], st[3]]) == 1 {
        return Err(una_abi::EISDIR);
    }
    Ok(u64::from_le_bytes([st[8], st[9], st[10], st[11], st[12], st[13], st[14], st[15]]))
}

/// `SYS_PATH_READ` once: bytes read (0 = EOF) or -errno.
fn path_read(path: &[u8], offset: u64, out: &mut [u8]) -> i64 {
    if path.is_empty() || path.len() > PATH_IO_PATH_MAX {
        return una_abi::EINVAL;
    }
    let mut req = [0u8; PATH_IO_HDR_LEN + PATH_IO_PATH_MAX];
    req[0..2].copy_from_slice(&(path.len() as u16).to_le_bytes());
    req[8..16].copy_from_slice(&offset.to_le_bytes());
    req[16..16 + path.len()].copy_from_slice(path);
    let n = PATH_IO_HDR_LEN + path.len();
    let cap = out.len().min(PATH_IO_MAX);
    sys(SYS_PATH_READ, req.as_ptr() as u64, n as u64, out.as_mut_ptr() as u64, cap as u64)
}

/// Read a whole file of at most `max` bytes. `Err(code)`: the first refusal (-E2BIG when it is over `max`).
pub fn read_file(path: &[u8], max: usize) -> Result<Vec<u8>, i64> {
    let size = match stat_size(path) {
        Ok(s) if s > max as u64 => return Err(una_abi::E2BIG),
        Ok(s) => s as usize,
        Err(e) if e == ENOSYS => max.min(PATH_IO_MAX * 64),
        Err(e) => return Err(e),
    };
    let mut v: Vec<u8> = Vec::new();
    if v.try_reserve_exact(size).is_err() {
        return Err(una_abi::ENOMEM);
    }
    v.resize(size, 0);
    let mut n = 0usize;
    while n < size {
        let step = PATH_IO_MAX.min(size - n);
        let r = path_read(path, n as u64, &mut v[n..n + step]);
        if r < 0 {
            if n == 0 && r == ENOSYS {
                return read_open(path, max);
            }
            if n == 0 {
                return Err(r);
            }
            break;
        }
        n += r as usize;
        if r == 0 || (r as usize) < step {
            break;
        }
    }
    v.truncate(n);
    Ok(v)
}

/// The classic handle verbs: SYS_OPEN (read-only) + SYS_READ until short.
fn read_open(path: &[u8], max: usize) -> Result<Vec<u8>, i64> {
    let h = sys(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if h < 0 {
        return Err(h);
    }
    let mut v: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    while v.len() < max {
        let k = sys(SYS_READ, h as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0);
        if k <= 0 {
            break;
        }
        let k = (k as usize).min(max - v.len());
        if v.try_reserve(k).is_err() {
            break;
        }
        v.extend_from_slice(&buf[..k]);
    }
    sys(SYS_CLOSE, h as u64, 0, 0, 0);
    Ok(v)
}
