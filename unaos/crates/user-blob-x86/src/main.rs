#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// The x86_64 ring-3 "hello from disk" user program (U2), built as a SEPARATE flat link product (out
// of the kernel's .text) so the kernel loads it FROM DISK instead of running a baked-in blob. This
// is the x86 twin of the aarch64 M6c `crates/user-blob` path, adapted from `include_bytes!` to a
// FAT-hosted artifact: `arroyo`/`builder` build this for a bare x86_64 target and
// `llvm-objcopy --only-section=.text -O binary` it to `target/hello.bin`, which lands on the FAT
// image (and the ESP) as `HELLO.BIN`. The kernel's U2 loader (arch/x86_64/syscall.rs::u2_probe_once)
// reads it off the volume, copies it into the ring-3 code page at USER_BASE through the identity
// alias (RO-from-start), and drops a scheduled task into it. There is NO ELF loader — the
// objcopy flat-binary model: raw bytes, no relocation, entry at byte 0.
//
// The routine is fully POSITION-INDEPENDENT: every memory reference is RIP-relative and the message
// bytes live inline in `.text` (no `.rodata`, no GOT/literal pool), so it runs correctly at whatever
// VA the kernel copies it to. The blob begins EXACTLY at `_start`'s first instruction — `#[naked]`
// (no compiler prologue) + the linker script's `KEEP(*(.text.entry))` first — because the kernel
// enters the copied image at offset 0.
//
// ABI (Linux-style, shared x86_64/aarch64 per the userspace docs): rax = syscall number, args in
// rdi/rsi/rdx, return in rax. Syscalls used: SYS_WRITE (fd, buf, len), SYS_EXIT (status).
//
// ABIFREEZE: both numbers are `const` OPERANDS out of `una_abi` — the same declaration
// `arch/x86_64/syscall.rs` dispatches from — rather than immediates typed into the asm string.
// Byte-identical output, and the entry stays at offset 0 where the kernel enters the copied image.

use core::arch::naked_asm;

/// `sys_write(1, "hello from disk\n", 16)` then `sys_exit(0)`. `#[naked]` so the flat blob is exactly
/// this instruction stream with no prologue/epilogue; forced first in the image by `.text.entry` +
/// the linker script. The message length is stored as a `.quad` and loaded RIP-relative (LLVM Intel
/// reads a `mov reg, sym-sym` immediate as a memory operand, so the running blob loads it
/// RIP-relative instead — the exact shape the kernel's U1a inline blob uses). Distinct numeric labels
/// keep the spin guard (`1:`) separate from the message start/end (`3:`/`4:`) and the length (`2:`).
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    naked_asm!(
        "mov rax, {sys_write}",    // SYS_WRITE
        "mov rdi, 1",              // fd = 1 (stdout)
        "lea rsi, [rip + 3f]",     // buf -> the message's ring-3 VA at run time (RIP-relative)
        "mov rdx, [rip + 2f]",     // len (from the stored length word; RIP-relative)
        "syscall",
        "mov rax, {sys_exit}",     // SYS_EXIT
        "mov rdi, 0",              // status = 0
        "syscall",
        "1: jmp 1b",               // sys_exit never returns; spin as a belt-and-braces guard
        ".balign 8",
        "2: .quad 4f - 3f",        // message length (assemble-time label difference)
        "3: .ascii \"hello from disk\\n\"",
        "4:",
        sys_write = const una_abi::SYS_WRITE,
        sys_exit = const una_abi::SYS_EXIT,
    );
}

/// A no_std binary must define a panic handler; this routine never panics (a single naked asm
/// stream, no Rust control flow), so an empty spin suffices and never runs.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
