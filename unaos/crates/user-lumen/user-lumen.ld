/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   RING3ABI2 M5 (rmbp-ledger B333) link script for the aarch64 LUMEN.ELF — the user-big shape on the
   aarch64 ELF window: linked at the ABSOLUTE EL0 VA 0x7840200000 = una_abi::USER_XWIN_VA_ARM (the slot's
   extension GiB, L1 entry 481, + USER_XWIN_OFF 2 MiB). The aarch64 loader reads "lowest PT_LOAD >=
   USER_XWIN_VA_ARM" as the elf model (arch/aarch64/xwin.rs): every segment mapped at its p_vaddr from heap
   frames, BSS zeroed, the stack at the window top sized by PT_GNU_STACK from `-z stack-size=`, SYS_SBRK
   from the page after the last segment. Was the LUMENBIN two-PHDR script linked at VA 0 in the 16 KiB
   classic window, whose `data+bss <= 0x3800` assert the vein_core/vein_ring3 build no longer met.

   The FB landmarks (info page +0x4000, surface +0x5000) hang off the CLASSIC window base, which an
   elf-model program cannot derive from `_start`: main.rs reads it from the args page
   (`una_abi::args().window_base()`), so the old `_start == 0` assert has nothing left to guard.

   NO PHDRS clause (with one, lld leaves PT_GNU_STACK's p_memsz 0 — measured by RING3WIN). lld lays out
   R / R+X / R+W PT_LOADs on separate pages itself, and emits a PT_NOTE for `.note.unaos.app` (EXECNAME's
   app note, flags bit0 = windowed), which is therefore KEPT, not discarded. */

ENTRY(_start)

SECTIONS
{
    . = 0x7840200000;
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }
    ASSERT(_start == 0x7840200000, "user-lumen (aarch64): _start must open the ELF window (the kernel's model test reads the lowest PT_LOAD)")

    /* EXECNAME (B322, R82): the launch declaration (una_abi::AppNote), KEPT ahead of the DISCARD rule below
       (the first matching rule wins); lld gives an allocated SHT_NOTE output section its own PT_NOTE. */
    .note.unaos.app : { KEEP(*(.note.unaos.app)) }

    . = ALIGN(0x1000);
    .rodata : {
        *(.rodata .rodata.*)
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
