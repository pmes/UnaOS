#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ELF-1: the minimal static ELF64 (aarch64) EL0 "hello" user program. UNLIKE the flat `user-blob`
// (objcopy'd to raw bytes and copied to one code page), this crate's link product is kept AS AN ELF.
// The kernel's aarch64 loader (arch/aarch64/syscall.rs::load_elf_into_slot) validates the ident
// (EI_CLASS=2, little-endian, EM_AARCH64, ET_EXEC), walks the PT_LOAD program headers, and maps each
// segment into a fresh slot with per-segment permissions.
//
// Two PT_LOAD segments (see user-elf.ld's PHDRS):
//   * text (R+X): this routine — mapped to an EL0-RX / EL1-RO code page.
//   * data (R+W): the witness message + a .bss tail — mapped to an EL0+EL1-RW, never-exec data page.
// The routine `adr`-loads the message out of the DATA segment (a different page from its own code),
// so a successful run PROVES the two segments landed at their linked relative offsets with the right
// permissions: the code page executes, and the read of the data page's message succeeds.
//
// ABI (Linux-aarch64): x8 = syscall number, args x0..x5, return in x0. Syscalls used:
//   SYS_WRITE = 1 (fd, buf, len), SYS_REPORT = 3 (value), SYS_EXIT = 2 (status). See syscall.rs.

use core::arch::global_asm;

// The whole program as explicit-section asm so the linker script's PHDRS assigns .text.entry to the
// R+X segment and .data/.bss to the R+W segment. `_start` writes the witness message (read out of the
// DATA segment via `adr`), reports the ELF-1 witness token 0x1E, then exits 0.
//
// The message length is a fixed immediate (19 = len("elf hello from EL0\n")); the string below MUST
// match. A `.space` in .bss gives the data segment a non-empty zero tail (p_memsz > p_filesz) so the
// loader's zero-the-bss path is exercised (the kernel witness checks those bytes read back as zero).
global_asm!(
    r#"
.section .text.entry,"ax",@progbits
.globl _start
_start:
    mov  x8, #1                 // SYS_WRITE
    mov  x0, #1                 // fd = 1 (stdout)
    adr  x1, elf_msg            // buf = the message in the DATA segment (PC-relative)
    mov  x2, #19                // len = strlen("elf hello from EL0\n")
    svc  #0
    adr  x3, elf_bss            // reference the .bss tail so it is not GC'd (kept NOBITS => p_memsz>p_filesz)
    mov  x8, #3                 // SYS_REPORT
    movz x0, #0x1E              // witness token 0x1E
    svc  #0
    mov  x8, #2                 // SYS_EXIT
    mov  x0, #0                 // status = 0
    svc  #0
0:  b 0b                        // sys_exit never returns; spin as a guard

.section .data,"aw",@progbits
.globl elf_msg
elf_msg:
    .ascii "elf hello from EL0\n"

.section .bss,"aw",@nobits
.globl elf_bss
elf_bss:
    .space 16                   // non-empty zero tail: exercises the loader's p_memsz>p_filesz zeroing
"#
);

// A no_std binary must define a panic handler; this routine never panics (the body is a single asm
// stream, no Rust control flow), so an empty spin suffices and never runs.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
