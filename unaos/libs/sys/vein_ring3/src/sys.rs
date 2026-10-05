// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Syscall stubs. x86_64: the kernel's `sysretq` tail scrubs rdi/rsi/rdx/r8/r9/r10 on EVERY return, so
//! all six are declared as not surviving (U1b B1; user-pulse has the long form). One 4-argument stub
//! serves every call; unused argument registers are passed as 0.

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

/// Write bytes to the console (fd 1 → the serial wire).
pub fn write(b: &[u8]) {
    sys(una_abi::SYS_WRITE, 1, b.as_ptr() as u64, b.len() as u64, 0);
}

/// Milliseconds since boot (SYS_GETINFO's ticks, converted by the per-arch rate).
pub fn now_ms() -> u64 {
    let mut info = [0u64; 2];
    if sys(una_abi::SYS_GETINFO, info.as_mut_ptr() as u64, 0, 0, 0) < 0 {
        return 0;
    }
    una_abi::getinfo_ticks_to_ms(info[1])
}

/// Fill `buf` from the kernel DRBG (SYS_GETRANDOM loops: at most GETRANDOM_MAX bytes per call).
pub fn getrandom(buf: &mut [u8]) -> Result<(), i64> {
    let mut off = 0;
    while off < buf.len() {
        let n = sys(una_abi::SYS_GETRANDOM, buf[off..].as_mut_ptr() as u64, (buf.len() - off) as u64, 0, 0);
        if n <= 0 {
            return Err(if n < 0 { n } else { una_abi::EIO });
        }
        off += n as usize;
    }
    Ok(())
}

/// VEINTLS (SR36): the kernel's civil clock, UTC Unix seconds (SYS_TIME). `Err(-EAGAIN)` while the
/// kernel's clock has never been anchored this boot (no RTC read, no SNTP answer).
pub fn unix_time() -> Result<i64, i64> {
    let r = sys(una_abi::SYS_TIME, 0, 0, 0, 0);
    if r < 0 { Err(r) } else { Ok(r) }
}
