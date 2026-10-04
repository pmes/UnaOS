/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   RING3WIN M4 link script for BIG.ELF — the first ELF-WINDOW program. Unlike every user-*-x86.ld before
   it, this links at an ABSOLUTE ring-3 VA: 0x10000200000 = una_abi::USER_XWIN_VA_X86 (USER_BASE 1 TiB +
   USER_XWIN_OFF 2 MiB). The kernel reads "lowest PT_LOAD >= USER_XWIN_VA_X86" as the elf model: every
   segment mapped at exactly its p_vaddr from heap frames, BSS zeroed, the stack at the top of the 4 MiB
   window sized by PT_GNU_STACK's p_memsz (`-z stack-size=`), the heap (SYS_SBRK) from the page after the
   last segment. Because the link VA IS the run VA, absolute pointers in data (vtables, fmt) are correct
   with no relocation — the property a 78 KiB TLS client needs.

   NO PHDRS clause, deliberately: with one, lld emits only the headers named there and leaves
   PT_GNU_STACK's p_memsz 0 whatever `-z stack-size=` says (measured). Without it lld lays out R / R+X /
   R+W PT_LOADs itself on separate pages (W^X per page holds) and fills PT_GNU_STACK from the flag. */

ENTRY(_start)

SECTIONS
{
    . = 0x10000200000;
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }
    ASSERT(_start == 0x10000200000, "user-big-x86: _start must open the ELF window (the kernel's model test reads the lowest PT_LOAD)")

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
