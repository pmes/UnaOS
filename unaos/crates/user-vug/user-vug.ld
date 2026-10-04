/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   UVUG-1 link script for the static ELF64 EL0 mini-vug program (crates/user-vug). Same shape as the
   ELFHELLO link (user-elf.ld): the link product is kept AS AN ELF and handed to the kernel's aarch64 ELF
   loader. We declare TWO PT_LOAD program headers via PHDRS so the loader has distinct segments to map
   with distinct permissions:

     text  PT_LOAD FLAGS(5)  =  PF_R | PF_X  -> mapped to an EL0-RX / EL1-RO code page
     data  PT_LOAD FLAGS(6)  =  PF_R | PF_W  -> mapped to an EL0+EL1-RW, never-executable data page

   Link at VA 0: the loader computes a load bias = (slot user-window base) - (min PT_LOAD p_vaddr), so
   p_vaddr are treated as offsets from the window base. .data is forced onto its own 4 KiB page so the
   two segments never share a page (per-page leaf permissions are the granularity the kernel enforces).
   The routine is position-independent (byte-granular `adr`), so the link VA is immaterial to
   correctness — the bias shifts both segments uniformly and preserves their relative distance. */

ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);   /* R + X */
    data PT_LOAD FLAGS(6);   /* R + W */
    note PT_NOTE FLAGS(4);   /* R — EXECNAME: the .note.unaos.app launch declaration */
}

SECTIONS
{
    . = 0x0;

    /* Segment 1 (R+X): the entry routine. `_start` is forced first so e_entry points at its first
       instruction (the loader enters at bias + e_entry). */
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
        *(.rodata .rodata.*)
    } :text

    /* EXECNAME (B322, R82): the program's launch declaration (una_abi::AppNote), KEPT here — ahead of the
       /DISCARD/ *(.note*) line below, and the first matching rule wins — inside the text segment and under
       its own PT_NOTE header, so the shell finds it by program header after --strip-all. */
    .note.unaos.app : { KEEP(*(.note.unaos.app)) } :text :note

    /* Force the data segment onto its own page so text/data never share a leaf. */
    . = ALIGN(0x1000);

    /* Segment 2 (R+W): the witness message + any .bss. A non-empty .bss (p_memsz > p_filesz) exercises
       the loader's zero-the-tail path. */
    .data : {
        *(.data .data.*)
    } :data
    .bss : {
        *(.bss .bss.*)
    } :data

    /* Drop metadata/unwind/dynamic/PLT/GOT — a static, loader-less program needs none, and a stray
       GOT/PLT would imply an unsatisfiable relocation. */
    /DISCARD/ : {
        *(.comment) *(.note*) *(.eh_frame*) *(.eh_frame_hdr)
        *(.interp) *(.dynamic) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash)
        *(.got) *(.got.plt) *(.plt) *(.igot*) *(.iplt*)
    }
}
