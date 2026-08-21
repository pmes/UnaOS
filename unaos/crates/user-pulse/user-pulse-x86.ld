/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   PULSE-1 link script for the x86_64 build of the static ELF64 EL0 CPU-pulse monitor
   (crates/user-pulse). The x86 twin of user-pulse.ld, and deliberately the SAME SHAPE — the two loaders
   were written against the same checklist, so the same segment layout satisfies both:

     text  PT_LOAD FLAGS(5)  =  PF_R | PF_X  -> mapped to a ring3-RX / read-only code page
     data  PT_LOAD FLAGS(6)  =  PF_R | PF_W  -> mapped to a ring3-RW, never-executable (NX) data page

   Link at VA 0: the loader computes a load bias = (slot user-window base) - (min PT_LOAD p_vaddr), so
   p_vaddr are treated as offsets from the window base. .data is forced onto its own 4 KiB page so the
   two segments never share a page — per-page leaf permissions are the granularity the kernel enforces,
   and a shared page would have to be granted the union of both segments' rights, breaking W^X.

   It is a separate file from user-pulse.ld rather than a shared one because the two are free to diverge
   (and the comments should not lie about which loader they serve); the CONTENT being identical today is
   a fact about the two loaders agreeing, not a coincidence worth collapsing.

   The program is position-independent (RIP-relative references), so the link VA is immaterial to
   correctness — the bias shifts both segments uniformly and preserves their relative distance. */

ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);   /* R + X */
    data PT_LOAD FLAGS(6);   /* R + W */
}

SECTIONS
{
    . = 0x0;

    /* Segment 1 (R+X): the entry routine. `_start` is forced first so e_entry points at its first
       instruction (the loader enters at bias + e_entry, and validate_elf requires the entry to land
       inside an executable segment). */
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
        *(.rodata .rodata.*)
    } :text

    /* LDPIN (WINX-8 class, commit 442f1cfa; provenance: the sibling linker-script audit,
       ~/.claude/plans/unaos/review/pi4-sibling-ld-audit.md): main.rs:803 derives the window base as
       `_start`'s own address, and every fixed landmark (info page +0x4000, surface +0x5000) hangs
       off that base — correct only while e_entry stays 0. WINX-8 proved that a layout change which
       moves e_entry (user-vug-x86's FILEHDR PHDRS moved it to 0xb0) skews every landmark silently.
       This ASSERT allocates nothing and turns that silent skew into a link failure. */
    ASSERT(_start == 0, "user-pulse-x86: e_entry moved off 0 -- the main.rs base derivation assumes _start IS the window base (WINX-8 class)")

    /* Force the data segment onto its own page so text/data never share a leaf. */
    . = ALIGN(0x1000);

    /* Segment 2 (R+W): the mutable state + any .bss. A non-empty .bss (p_memsz > p_filesz) exercises
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
