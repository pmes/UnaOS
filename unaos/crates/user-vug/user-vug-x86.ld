/* SPDX-License-Identifier: GPL-3.0-or-later
   Copyright (C) 2026 The Architect & Una

   WINX-7 link script for the x86_64 build of the static ELF64 EL0 mini-vug program
   (crates/user-vug). The x86 twin of user-vug.ld, and deliberately the SAME SHAPE — the two loaders
   were written against the same checklist, so the same segment layout satisfies both:

     text  PT_LOAD FLAGS(5)  =  PF_R | PF_X  -> mapped to a ring3-RX / read-only code page
     data  PT_LOAD FLAGS(6)  =  PF_R | PF_W  -> mapped to a ring3-RW, never-executable (NX) data page

   Link at VA 0: the loader computes a load bias = (slot user-window base) - (min PT_LOAD p_vaddr), so
   p_vaddr are treated as offsets from the window base. .data is forced onto its own 4 KiB page so the
   two segments never share a page — per-page leaf permissions are the granularity the kernel enforces,
   and a shared page would have to be granted the union of both segments' rights, breaking W^X.

   It is a separate file from user-vug.ld rather than a shared one because the two are free to diverge
   (and the comments should not lie about which loader they serve); the content being identical was a
   fact about the two loaders agreeing, not a coincidence worth collapsing — and as of VUGSHRINK the
   two HAVE diverged, in the one clause below.

   VUGSHRINK — WHY THE TEXT SEGMENT CARRIES `FILEHDR PHDRS`, AND WHAT IT IS WORTH.

   Without them, nothing in the image claims file offsets [0, 0xb0), so lld cannot start the text
   segment there: the ELF header and phdr table sit in a no-man's-land ahead of it, and the first
   PT_LOAD is pushed to the next max-page-size boundary — file offset 0x1000 against p_vaddr 0. That
   0x1000 SKEW is then carried by every later offset, because a PT_LOAD's file offset must stay
   congruent to its vaddr modulo the page size. `.bss` at vaddr 0x3000 therefore lands at file offset
   0x4000, the file is padded out to reach it, and the image is 0x4000 + the section-header trailer =
   16664 bytes — 280 over the 16384-byte EL0 user window `arroyo` enforces — while carrying only
   12415 bytes of anything. The other 4249 were hole.

   Worse than the waste was the CLIFF it built. With the skew in place the whole budget was `.text`
   <= 0x2000: one byte past it moved `.bss` a page and the FILE grew by 4096 in a single step, from
   12568 straight through the ceiling to 16664. That is the step the WHEELZOOM M1 commit (35f49101)
   fell off — it grew `.text` from 0x1f9b to 0x207f, 127 bytes over the line, and paid 4096 for them.
   Three separate notes in the crate (Cargo.toml's `opt-level = "z"` requirement, and the SIZE notes on
   `input_wait` and `wheel_zoom` in main.rs) exist because authors kept having to buy code back at that
   rate.

   Declaring the headers as part of the segment claims those bytes, so the segment starts at file
   offset 0, the skew is zero, and offsets equal vaddrs throughout: `.text` at 0xb0, `.bss` at 0x3000,
   file = 0x3000 + trailer = 12568 bytes. Nothing about the program changed — same code, same data,
   same two segments, same flags. The budget is now `0xb0 + .text <= 0x3000`, i.e. `.text` <= 0x2f50,
   which turns a 127-byte deficit into 3793 bytes of headroom.

   THE LOADER IS UNAFFECTED, and that is checkable rather than hoped: `arch/x86_64/elf.rs` maps by
   p_offset/p_vaddr/p_filesz per PT_LOAD with no assumption that the first segment begins past the
   headers, and `validate_elf`'s constraints all still hold — both segments are page-aligned in vaddr
   (0 and 0x3000), so per-page W^X leaf permissions are unchanged; the entry (0xb0) lies inside the
   executable segment; no segment is both W and X. The only observable difference is that the RX page
   now also holds the image's own 0xb0 bytes of ELF header, which is the ordinary layout of every
   static ELF on every system.

   aarch64 is deliberately NOT changed here: user-vug.ld keeps its shape, and VUG.ELF stays
   byte-for-byte what it was. That arch has no size problem to solve — its `.text` sits under 0x2000
   with room — and changing its link layout to fix a bug it does not have would put the aarch64 loader
   on trial for nothing.

   The program is position-independent (RIP-relative references), so the link VA is immaterial to
   correctness — the bias shifts both segments uniformly and preserves their relative distance.

   WINX-8 — THE BUDGET ABOVE WAS FILE-MATH TRUE AND RUNTIME FALSE, AND WHAT NOW HOLDS IT UP.

   The `0xb0 + .text <= 0x3000` budget was derived from the file/window arithmetic alone. It omitted a
   second consumer of vaddr page 2 (0x2000..0x3000): worker A's stack, which main.rs (UVUG-1) topped at
   `base + 0x3000` and descended through that page. The two never collided before VUGSHRINK only
   because text past 0x2000 ALSO blew the 16384-byte file window — the build gate failed before any
   boot could. VUGSHRINK removed the file cliff and let text legally claim page 2 (it stands at memsz
   0x212f as of this note), so the first x86 run killed worker A on its first frame write
   (cr2 = base+0x2f88, err=0x7) — found by the rmbp seat's WINX-8 leg pre-validating the trunk fold.

   And "nothing about the program changed" above was ALSO one clause too strong: e_entry moved from 0
   to 0xb0, and main.rs derived the window base as `_start`'s own address — correct for e_entry 0,
   now +0xb0-skewed. That skew is what killed the PARENT in the same WINX-8 report: its surface blit
   started at window+0x50b0 and crossed the mapping end at exactly window+0x56000. main.rs now
   subtracts the header size on x86, and an ASSERT below pins e_entry == SIZEOF_HEADERS.

   The fix is ownership, not retreat: page 2 belongs to text (the budget stands), worker A's stack
   moved to the top half of page 3 (main.rs carve note at the spawn site), the base derivation is
   de-skewed, and the ASSERTs below turn every contract violation this file can see into a LINK
   error instead of a runtime kill. */

ENTRY(_start)

PHDRS
{
    /* VUGSHRINK: `FILEHDR PHDRS` puts the ELF header and the program-header table INSIDE the text
       segment, which is what lets that segment start at file offset 0. See the note above. */
    text PT_LOAD FILEHDR PHDRS FLAGS(5);   /* R + X */
    data PT_LOAD FLAGS(6);   /* R + W */
}

SECTIONS
{
    /* VUGSHRINK: `+ SIZEOF_HEADERS` is the other half of `FILEHDR PHDRS` — the headers occupy
       [0, 0xb0) of both the file and the address space, and `.text` begins after them. */
    . = 0x0 + SIZEOF_HEADERS;

    /* Segment 1 (R+X): the entry routine. `_start` is forced first so e_entry points at its first
       instruction (the loader enters at bias + e_entry, and validate_elf requires the entry to land
       inside an executable segment). */
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
        *(.rodata .rodata.*)
    } :text

    /* WINX-8: page 3 (0x3000..0x4000) must stay a data/stack page — text ending past 0x3000 would
       turn the image's ONLY RW page RO+X and take both worker stacks with it. This is the same line
       as the VUGSHRINK budget above, now enforced rather than narrated. */
    ASSERT(. <= 0x3000, "user-vug-x86: text segment (headers + .text + .rodata) spills past 0x3000 into the data/stack page")

    /* WINX-8: main.rs derives the window base as `_start - 0xb0` on x86 (FILEHDR PHDRS puts `_start`
       at e_entry SIZEOF_HEADERS, not 0 — deriving it as `_start` bare was the +0xb0 skew that walked
       the parent's surface blit off the end of its mapping). Pin the constant: if the header count
       ever changes SIZEOF_HEADERS, this trips instead of the skew coming back at a new width. */
    ASSERT(_start == SIZEOF_HEADERS, "user-vug-x86: e_entry moved off SIZEOF_HEADERS — update the 0xb0 base correction in main.rs `_start`")

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

    /* WINX-8: when .data lands at 0x3000 (text using page 2), page 3 is shared three ways —
       .data/.bss at its head, worker B's stack descending from +0x3800, worker A's from +0x4000
       (the main.rs carve). A .bss that grows past 0x3600 would silently eat worker B's floor;
       0x3600 leaves B no less than the smallest region it has already proven on metal. When text is
       small and .data lands at 0x2000 instead, this assert is trivially true and the stacks own
       page 3 outright. */
    ASSERT(!(. > 0x3600 && ADDR(.data) >= 0x3000), "user-vug-x86: .data/.bss spills past 0x3600 into worker B's stack region")

    /* Drop metadata/unwind/dynamic/PLT/GOT — a static, loader-less program needs none, and a stray
       GOT/PLT would imply an unsatisfiable relocation. */
    /DISCARD/ : {
        *(.comment) *(.note*) *(.eh_frame*) *(.eh_frame_hdr)
        *(.interp) *(.dynamic) *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash)
        *(.got) *(.got.plt) *(.plt) *(.igot*) *(.iplt*)
    }
}
