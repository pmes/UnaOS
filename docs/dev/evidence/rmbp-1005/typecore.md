# TYPECORE (rmbp-ledger B450) — one MIME table, one ELF parse, both rings

**Finding (ARCH-2026-10-06 F6, MED).** The kernel's `fs/filetype.rs` owns the extension table (`EXT_TABLE`), the
built-in magics (PNG, RIFF/WAVE, ELF, gzip, ustar, GIF, the ISO/EBML gate) and its own PT_LOAD walk
(`elf_flavour`) beside `elf_core::read_phdrs`; `libs/bandy` (`FacetCommand::image_mime_for`) keeps a host twin of
the image rows. Two tables drift (bandy knew `jpe jfif dib`, the kernel did not); two ELF parses can disagree.

**The seam (R79): a shared core.** `unaos/libs/sys/type_core` — `#![no_std]`, `#![forbid(unsafe_code)]`, no
allocator, one dependency (`elf_core`). It owns the MIME strings, THE extension table, `ext_of`/`by_extension`,
`extensions_of` (the `una:extensions` list FILETYPES lists), `ext_in_list` (a user-added `una:extensions` value),
the built-in magic sniff and its order, the text/JSON/Markdown heads, the ELF split (through
`elf_core::read_phdrs`, the loaders' parse), and the fixture heads the kernel's `tests filetype` writes. The
kernel's filetype re-exports it and keeps only what needs a volume (attributes, the ISO walk, the stamp);
bandy's `image_mime_for` is the table filtered to the types Facet claims. Host KATs: `cargo test -p type_core -p bandy`
(the strings are checked equal to pixel_core's/audio_core's/demux_core's own).

**Milestones.** M1 type_core + KATs. M2 kernel filetype on type_core (EXT_TABLE/sniff/elf_flavour gone from the
kernel; the GATE-ARCH `parser|fs/filetype.rs|{elf,gif,isobmff,png,riff}` baseline rows are deleted at the fold —
arch.baseline is not on this cut). M3 bandy on type_core. M4 the owed B423 leg: a name the table does not type is
looked up in the FILETYPES registry's user-added `una:extensions` (`/system/filetypes/*`), before `unknown`.

**Witness.** `file x.foo` (an EMPTY file, or bytes no sniff recognises — content still beats the name) after `setfattr /system/filetypes/text-plain una:extensions=txt,foo` prints
`/home/x.foo: text/plain extension`, and the serial wire carries `[filetype] ext=user foo -> text/plain (from /system/filetypes/text-plain)`. On the metal boot nothing prints (R80: it is read on a table miss only).

**Owed.** FILETYPES (merge18) `assoc::extensions_of` swaps its EXT_TABLE walk for `type_core::extensions_of` at the
fold (its file is not on this cut).
