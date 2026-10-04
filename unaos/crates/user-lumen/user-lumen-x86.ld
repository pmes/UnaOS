/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   LUMENAPP (rmbp-ledger B323) link script for LUMEN.ELF — the user-big shape (RING3WIN): linked at the
   ABSOLUTE ring-3 VA 0x10000200000 = una_abi::USER_XWIN_VA_X86, so the kernel places it in the ELF window
   (elf model: every segment mapped at its p_vaddr from heap frames, BSS zeroed, the stack at the window
   top sized by PT_GNU_STACK from `-z stack-size=`). This is the BIGAPPS relink RING3WIN owed for LUMEN: a
   TLS 1.3 client (~80 KiB of code) and ~90 KiB of buffers cannot live in the 16 KiB fixed window.

   NO PHDRS clause (with one, lld leaves PT_GNU_STACK's p_memsz 0 — measured by RING3WIN). lld lays out
   R / R+X / R+W PT_LOADs on separate pages itself, and emits a PT_NOTE for `.note.unaos.app` (EXECNAME's
   app note: name "UnaOS", type 1, flags bit0 = windowed), which is therefore KEPT, not discarded. */

ENTRY(_start)

SECTIONS
{
    . = 0x10000200000;
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }
    ASSERT(_start == 0x10000200000, "user-lumen-x86: _start must open the ELF window (the kernel's model test reads the lowest PT_LOAD)")

    . = ALIGN(0x1000);
    .rodata : {
        *(.rodata .rodata.*)
    }
    .note.unaos.app : ALIGN(4) {
        KEEP(*(.note.unaos.app))
    }

    . = ALIGN(0x1000);
    .data : {
        *(.data .data.*)
    }
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.eh_frame*) *(.comment) *(.note.gnu*) *(.note.GNU*) *(.dynamic) *(.dynsym) *(.dynstr) *(.gnu.hash) *(.hash) *(.rela*) *(.got*) *(.plt*)
    }
}
