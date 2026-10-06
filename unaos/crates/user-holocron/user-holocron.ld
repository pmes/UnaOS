/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   HOLOCRONARM (rmbp-ledger B484) link script for the aarch64 HOLOCRON.ELF — the x86 script's shape
   (user-holocron-x86.ld) on the aarch64 ELF window (RING3ABI2 M5, arch/aarch64/xwin.rs): linked at the
   ABSOLUTE EL0 VA 0x7840200000 = una_abi::USER_XWIN_VA_ARM (the slot's extension GiB, L1 entry 481, +
   USER_XWIN_OFF 2 MiB), exactly as user-lumen.ld. The loader reads "lowest PT_LOAD >= USER_XWIN_VA_ARM" as
   the elf model: every segment mapped at its p_vaddr from heap frames, BSS zeroed, the stack at the window
   top sized by PT_GNU_STACK from `-z stack-size=`, SYS_SBRK from the page after the last segment (the ring
   KDF's Argon2id blocks). An elf-model image has no 16 KiB slot and no page cliff; its cap is
   xwin::IMAGE_CAP (12 MiB on the 48 MiB heap).

   NO PHDRS clause (with one, lld leaves PT_GNU_STACK's p_memsz 0 — measured by RING3WIN). lld lays out
   R / R+X / R+W PT_LOADs on separate pages itself, and emits a PT_NOTE for `.note.unaos.app` (EXECNAME's
   app note: flags 2 = resident), which is therefore KEPT, not discarded. */

ENTRY(_start)

SECTIONS
{
    . = 0x7840200000;
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }
    ASSERT(_start == 0x7840200000, "user-holocron (aarch64): _start must open the ELF window (the kernel's model test reads the lowest PT_LOAD)")

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
