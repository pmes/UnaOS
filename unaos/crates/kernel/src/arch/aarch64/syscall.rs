// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 EL0 userspace + the SVC syscall interface (M6a — the first privilege boundary).
//
// The kernel runs at EL1 (see boot::drop_to_el1). A user task drops to EL0 (sched::spawn_user) and
// calls back in with `svc #0`; because the kernel is at EL1 and HCR_EL2.TGE=0, that SVC is taken to
// EL1 at VBAR_EL1 + 0x400, where the `__vec_svc` stub (exceptions.rs) saves the frame, checks
// ESR_EL1.EC==0x15 (SVC from AArch64), and calls `aarch64_svc_handler` here — on the faulting task's
// own kernel stack, IRQ-masked. The ABI is the Linux-aarch64 one: x8 = syscall number, args in x0–x5,
// return in x0.
//
// M6a is deliberately tiny: one baked-in position-independent user program does sys_write then
// sys_exit, proving the whole EL0→EL1→EL0 round trip end to end. M6b hardens the fault path (task-kill
// instead of halt) and per-page permissions; M6f adds a real copy_from_user and a wider surface.

use core::sync::atomic::{AtomicBool, Ordering};

// --- Syscall numbers (a tiny subset for M6a). ---
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;

// --- The baked-in M6a user program: sys_write(1, "hello from EL0\n") then sys_exit(0). Fully
// position-independent — `adr` is PC-relative and there are only svc + mov-immediate — so it runs
// correctly wherever it is copied. `__user_blob_{start,end}` bound the copy. ---
core::arch::global_asm!(
    r#"
    .globl __user_blob_start
__user_blob_start:
    mov x8, #1                              // SYS_WRITE
    mov x0, #1                              // fd = 1 (stdout)
    adr x1, __user_msg                      // buf (PC-relative -> the EL0 VA at run time)
    mov x2, #(__user_msg_end - __user_msg)  // len
    svc #0
    mov x8, #2                              // SYS_EXIT
    mov x0, #0                              // status = 0
    svc #0
1:  b 1b                                    // sys_exit never returns; spin as a belt-and-braces guard
__user_msg:
    .ascii "hello from EL0\n"
__user_msg_end:
    .balign 4
    .globl __user_blob_end
__user_blob_end:
"#
);

unsafe extern "C" {
    static __user_blob_start: u8;
    static __user_blob_end: u8;
}

/// Copy the M6a user program into the EL0 window (`boot::user_region`), make it executable at EL0 (the
/// I-cache maintenance), and return `(entry VA, initial SP_EL0)` for `sched::spawn_user`. Call once,
/// after `mmu_init` (the user pages are mapped and EL1-writable — AP=0b01). The window is identity-
/// mapped, so the entry VA equals the region base PA and the blob's PC-relative `adr` resolves in it.
pub fn setup() -> (u64, u64) {
    let (base, size) = super::boot::user_region();
    let start = &raw const __user_blob_start as usize;
    let end = &raw const __user_blob_end as usize;
    let blob_len = end - start;
    assert!(blob_len <= size, "M6a user blob does not fit in USER_REGION");
    unsafe {
        core::ptr::copy_nonoverlapping(start as *const u8, base as *mut u8, blob_len);
    }
    // Freshly-written code: clean D to the PoU + invalidate the I-cache so the EL0 fetch (possibly on
    // another core — IC IVAU broadcasts Inner-Shareable) sees the new bytes. Metal-only; QEMU no-op.
    super::cache::icache_sync_range(base as usize, blob_len);
    let entry = base;
    let sp = (base + size as u64) & !0xF; // 16-aligned top of the window = initial user stack pointer
    (entry, sp)
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
        SYS_EXIT => super::sched::exit(), // never returns; the __vec_svc eret tail is not reached
        _ => -38,                         // -ENOSYS
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
/// holes cheaply. NOTE (A72/Armv8.0): the direct EL1 read of an AP=0b01 EL0 page is permitted because
/// this core lacks FEAT_PAN; on a PAN-capable core (the Jetson A78 port) this must become an
/// unprivileged load (LDTR) or a validated copy first, else it Permission-faults (EC 0x25).
fn sys_write(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != 1 {
        return -9; // -EBADF (only stdout for M6a)
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
