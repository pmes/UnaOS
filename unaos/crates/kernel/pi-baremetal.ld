/* SPDX-License-Identifier: GPL-3.0-or-later
   Bare-metal Raspberry Pi 4 (aarch64) link script.

   The VideoCore GPU ROM loads kernel8.img RAW (no ELF parsing, no relocation) to the load address
   and jumps to its first byte. So we link NON-position-independent at a fixed address, with `_start`
   placed first, and expose __bss_start/__bss_end (for the entry to zero BSS), __stack_top, and
   __rx_end (the page-aligned end of text+rodata, for the W^X carve). */

ENTRY(_start)

/* arm_64bit=1 default load address for kernel8.img. */
. = DEFINED(KERNEL_LOAD) ? KERNEL_LOAD : 0x80000;

SECTIONS
{
    .text.boot : { KEEP(*(.text.boot)) }   /* the raw entry MUST be first in the image */
    .text      : { *(.text .text.*) }
    .rodata    : { *(.rodata .rodata.*) }

    /* WXN64 (rmbp-ledger B685): the end of the read-only image, page-aligned, so the W^X carve
       (arch/aarch64/mmu_wxn.rs) can map [_start, __rx_end) EL1 read-only + executable and everything
       from .data up writable + execute-never. Nothing writable may be placed before this point. */
    . = ALIGN(4096);
    __rx_end = .;

    .data      : { *(.data .data.*) }

    . = ALIGN(16);
    __bss_start = .;
    .bss (NOLOAD) : { *(.bss .bss.*) *(COMMON) }
    . = ALIGN(16);
    __bss_end = .;

    /* Initial stack: reserved ABOVE __bss_end so the BSS clear (which covers only
       __bss_start..__bss_end) never wipes the stack we set SP to. 512 KiB, grows down. */
    . = ALIGN(4096);
    . = . + 0x80000;
    __stack_top = .;

    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) *(.eh_frame_hdr) *(.interp) *(.dynamic) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash) }
}
