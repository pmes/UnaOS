#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// The EL0 "hello" user program, built as a SEPARATE link product (M6c) instead of living inline in
// the kernel's `.text`. `arroyo kernel8` objcopy's this crate to `target/user_blob.bin`; the kernel
// `include_bytes!`s that flat binary and copies it into the identity-mapped EL0 CODE page at boot
// (arch/aarch64/syscall.rs::setup), then flips the page to EL0-RX/EL1-RO and `eret`s a scheduled
// task into it. There is NO ELF loader — the VideoCore-ROM/objcopy flat-binary model: raw bytes,
// no relocation, no dynamic fixups.
//
// The routine is therefore fully POSITION-INDEPENDENT: `adr` is byte-granular PC-relative (±1 MiB)
// and the message bytes live inline in `.text` (no `.rodata`, no literal pool, no GOT), so it runs
// correctly at whatever address the kernel copies it to. The blob begins EXACTLY at `_start`'s first
// instruction — the routine is `#[naked]` (no compiler prologue) and the linker script places its
// section first — because the kernel enters at offset 0 of the copied image.
//
// ABI (Linux-aarch64, shared with x86_64 per the userspace docs): x8 = syscall number, args x0..x5,
// return in x0. Syscalls used: SYS_WRITE = 1 (fd, buf, len), SYS_EXIT = 2 (status). See syscall.rs.

use core::arch::naked_asm;

/// The user program: `sys_write(1, "hello from EL0\n", 15)` then `sys_exit(0)`. `#[naked]` so the
/// flat blob is exactly this instruction stream with no prologue/epilogue; `_start` is forced first
/// in the image by `#[link_section = ".text.entry"]` + the linker script's `KEEP(*(.text.entry))`.
/// The `adr` reaches the inline message (label `0:`) PC-relatively, so the buffer pointer handed to
/// SYS_WRITE is the message's run-time EL0 VA wherever the kernel loaded the blob.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    naked_asm!(
        "mov x8, #1",            // SYS_WRITE
        "mov x0, #1",            // fd = 1 (stdout)
        "adr x1, 0f",            // buf = the message (PC-relative -> its EL0 VA at run time)
        "mov x2, #(1f - 0f)",    // len = message length (assembled to an immediate)
        "svc #0",
        "mov x8, #2",            // SYS_EXIT
        "mov x0, #0",            // status = 0
        "svc #0",
        "2: b 2b",               // sys_exit never returns; spin as a belt-and-braces guard
        "0: .ascii \"hello from EL0\\n\"",
        "1:",
    );
}

/// A no_std binary must define a panic handler; this routine never panics (no Rust control flow —
/// the body is a single naked asm stream), so an empty spin suffices and never runs.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
