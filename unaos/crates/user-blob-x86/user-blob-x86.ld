/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   Link script for the x86_64 ring-3 "hello from disk" user blob (U2). `llvm-objcopy -O binary
   --only-section=.text` flattens the single output section to target/hello.bin; the kernel's U2
   loader copies those raw bytes into the ring-3 code page at USER_BASE (4 KiB-aligned) and enters at
   offset 0. The routine is position-independent (RIP-relative references + an inline message, no
   .rodata/GOT), so the LINK address is immaterial to correctness — we link at 0 and keep `.text`
   (entry first) plus any stray `.rodata` contiguous, so the flat image is exactly the routine and
   its bytes with nothing before `_start`. Mirrors crates/user-blob/user-blob.ld (aarch64 M6c). */

ENTRY(_start)

. = 0;

SECTIONS
{
    .text : {
        KEEP(*(.text.entry))     /* _start MUST be the first byte of the flat image */
        *(.text .text.*)
        *(.rodata .rodata.*)     /* none expected (message is inline) — keep contiguous if any */
    }

    /* Nothing else belongs in a flat ring-3 blob; drop metadata/unwind/dynamic/PLT/GOT so objcopy
       sees only .text. Any leftover would either bloat the copy or (a GOT/PLT) imply an
       unsatisfiable relocation for a flat, loader-less binary. */
    /DISCARD/ : {
        *(.comment) *(.note*) *(.eh_frame*) *(.eh_frame_hdr)
        *(.interp) *(.dynamic) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash)
        *(.got) *(.got.plt) *(.plt) *(.igot*) *(.iplt*)
    }
}
