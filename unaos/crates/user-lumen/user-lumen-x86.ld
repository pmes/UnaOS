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

    /* EXECNAME (B322, R82): the program's launch declaration (una_abi::AppNote), KEPT here — ahead of the
       /DISCARD/ *(.note*) line below, and the first matching rule wins — inside the text segment and under
       its own PT_NOTE header, so the shell finds it by program header after --strip-all. */
    .note.unaos.app : { KEEP(*(.note.unaos.app)) }

    /* LDPIN (WINX-8 class, commit 442f1cfa; provenance: the sibling linker-script audit,
       ~/.claude/plans/unaos/review/pi4-sibling-ld-audit.md): the sibling user-pulse main.rs derives the window base as
       `_start`'s own address, and every fixed landmark (info page +0x4000, surface +0x5000) hangs
       off that base — correct only while e_entry stays 0. WINX-8 proved that a layout change which
       moves e_entry (user-vug-x86's FILEHDR PHDRS moved it to 0xb0) skews every landmark silently.
       This ASSERT allocates nothing and turns that silent skew into a link failure. */
    ASSERT(_start == 0, "user-lumen-x86: e_entry moved off 0 -- the main.rs base derivation assumes _start IS the window base (WINX-8 class)")

    /* Force the data segment onto its own page so text/data never share a leaf. */
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
