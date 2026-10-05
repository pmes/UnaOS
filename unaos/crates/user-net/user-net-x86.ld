/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   RING3ABI2 M4 (rmbp-ledger B333) link script for NET.ELF — the user-big shape (RING3WIN): linked at the
   ABSOLUTE ring-3 VA 0x10000200000 = una_abi::USER_XWIN_VA_X86, so the kernel places it in the ELF window
   (elf model: every segment mapped at its p_vaddr from heap frames, BSS zeroed, the stack at the window
   top sized by PT_GNU_STACK from `-z stack-size=`, SYS_SBRK from the page after the last segment). Was
   the NETRING3 (B306) two-PHDR fixed-window script linked at VA 0; NET.ELF reads its host from the args
   page (una_abi::args()), never from a `_start`-relative landmark, so the old `_start == 0` assert has
   nothing left to guard.

   NO PHDRS clause (with one, lld leaves PT_GNU_STACK's p_memsz 0 — measured by RING3WIN). lld lays out
   R / R+X / R+W PT_LOADs on separate pages itself, and emits a PT_NOTE for `.note.unaos.app` (EXECNAME's
   app note: a console program, flags 0), which is therefore KEPT, not discarded. */

ENTRY(_start)

SECTIONS
{
    . = 0x10000200000;
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }
    ASSERT(_start == 0x10000200000, "user-net-x86: _start must open the ELF window (the kernel's model test reads the lowest PT_LOAD)")

    . = ALIGN(0x1000);
    .rodata : {
        *(.rodata .rodata.*)
    }
    /* EXECNAME (B322, R82): the launch declaration (una_abi::AppNote), KEPT ahead of the DISCARD rule below
       (the first matching rule wins); lld gives an allocated SHT_NOTE output section its own PT_NOTE. */
    .note.unaos.app : { KEEP(*(.note.unaos.app)) }

    . = ALIGN(0x1000);
    .data : {
        *(.data .data.*)
    }
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.eh_frame*) *(.comment) *(.note*) *(.dynamic) *(.dynsym) *(.dynstr) *(.gnu.hash) *(.hash) *(.rela*) *(.got*) *(.plt*)
    }
}
