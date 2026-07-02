/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   Link script for the EL0 "hello" user blob (M6c). `llvm-objcopy -O binary` flattens the single
   output section to target/user_blob.bin; the kernel copies those raw bytes into the identity-mapped
   EL0 CODE page (VA == PA, 4 KiB-aligned) and enters at offset 0. The routine is position-independent
   (byte-granular `adr` + an inline message, no .rodata/GOT), so the LINK address is immaterial to
   correctness — we link at 0 and keep `.text` (entry first) plus any stray `.rodata` contiguous, so
   the flat image is exactly the routine and its bytes with nothing before `_start`. */

ENTRY(_start)

. = 0;

SECTIONS
{
    .text : {
        KEEP(*(.text.entry))     /* _start MUST be the first byte of the flat image */
        *(.text .text.*)
        *(.rodata .rodata.*)     /* none expected (message is inline) — keep contiguous if any */
    }

    /* Nothing else belongs in a flat EL0 blob; drop metadata/unwind/dynamic/PLT/GOT so objcopy sees
       only .text. Any leftover would either bloat the copy or (a GOT/PLT) imply an unsatisfiable
       relocation for a flat, loader-less binary. */
    /DISCARD/ : {
        *(.comment) *(.note*) *(.eh_frame*) *(.eh_frame_hdr)
        *(.interp) *(.dynamic) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash)
        *(.got) *(.got.plt) *(.plt) *(.igot*) *(.iplt*)
    }
}
