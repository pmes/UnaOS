# SRCEXTRACT — SELFHOST rung 3: the verified source tree is materialised under `/SRC/`

**Finding.** `selfhost/mod.rs` (SELFHOST-2) verifies `SRC.TGZ` against `SRC.SHA`, inflates it and WALKS the tar read-only
(`tar.rs` keeps a census, never contents). A build tool still has no files to read: ROADMAP §1c SH-5's precondition is a
tree on the volume. No extraction code exists (`grep -n create_dir selfhost/` is empty). Not a boot-log finding; a direction arc.

**Mechanism.** New `selfhost/extract.rs` (`pub mod extract;` in `selfhost/mod.rs:43`).
- `Stream<H: Handler>` is a `Sink` (plugs into `inflate::gunzip` exactly as `TarWalk` does) and turns the byte stream into
  `begin/data*/end` member events; PAX `path=` and GNU `L` names honoured; `target/`, `..` paths and links skipped.
- `Extractor` writes: `FatFs::create_dir` / `create_in_dir` (8.3 or LFN, the writer chooses) / `write_grow` once per
  64 KiB flush. `write_grow` is CALLED, not touched (SDHCMULTI2). No whole-file buffering. Honest gap: the brief's "claim the
  cluster run once" needs a FAT-writer entry this arc does not own; `write_grow` re-collects the chain per chunk.
- Where: `/SRC/` at the volume root of `fat::mount_program_source()` (the volume SRC.TGZ is on), written as the kernel
  principal (the `src` shell verb is the console of the machine, `SHELL_PRINCIPAL == KERNEL_PRINCIPAL`); DIRNS: a system
  tree is a top-level directory, not under a home. The `MountTable` is NOT used: the extractor needs directory clusters and the
  FAT writer's `(lba, off)` slots, which the VFS hides (same refusal DIRNS recorded, vfs.md §13.9).
- Names: `fat_name` — illegal/non-ASCII chars to `_`, trailing dot/space to `_`, > 200 chars truncated, and a `~hhhh` tag
  (FNV-1a/16 of the original component) before the extension on any change. Pure, so `src verify` recomputes it. A
  case-insensitive collision takes `fat_name_alt`. Logged `[src] renamed a/b/c.rs -> SRC/a/b/c~1f2e.rs` (first 64, then counted).
- Resumable: a file already on disk with the member's size is kept (size is published last per chunk), anything else is
  deleted and rewritten.
- Verify (M1): a 32-byte digest per member in a RING OF THE LAST 64 files (no re-inflate); up to 8 random ring entries are
  re-read from the volume and compared (sha + size) → `verify=ok/n`. It is the archive's tail, a spot check of the write path.

**Milestones.** M1 `src extract [--dry-run]` · M2 `src status` (present? SRC.SHA commit/describe, file count) · M3 `src verify`
(second inflate pass: every file present with the right size, 1-in-8 sha sample, disk file count == tar file count).
`tests srcextract` = the dry run, registered at the end of `verify_source_once`'s PASS path (`selfhost/mod.rs`).

**Knob.** `UNAOS_SELFHOST=1` already gates everything — three places confirmed: `unaos/arroyo:243`, `unaos/builder/src/main.rs:61`,
`unaos/scripts/banner-cert.sh:381`. No new knob. `src` verb arm is `#[cfg(feature = "selfhost")]` in `shell.rs`; `("src", Always)` in
`midden_core::HOST_VERBS` like `fetch`.

**Witness.** `:: SRCEXTRACT: files= dirs= bytes= renamed= ms= verify= -> PASS|FAIL ::` · `:: SRCVERIFY: tar_files= disk_files= missing=
size_bad= hashed= hash_bad= sha_match= ms= -> PASS|FAIL ::` · progress `[src] files= dirs= bytes= renamed= skipped=` every 256 members.

**Pins.** `./arroyo test-selfhost` carries SRC.TGZ in QEMU and boots with `tests-at-boot`, so the dry run fires there; the lane
has no spec (arroyo NO SPEC note), so the pin is an awk check in `test_selfhost` for `verify=dry -> PASS` (unflown — R76/R78).
The real extraction is NOT pinned in a lane (writes the volume; metal verb only).

## Written
Boot 17 should show (desktop shell, after `tests srcextract`, and on a selfhost image):
`:: SRCEXTRACT: files=<n> dirs=<n> bytes=<n> renamed=<n> ms=<n> verify=dry -> PASS ::`; then operator `src extract` →
`... verify=8/8 -> PASS ::`, `src status` → `[src] /SRC present on … files=<n> stamp-commit=…`, `src verify` →
`:: SRCVERIFY: … missing=0 size_bad=0 hash_bad=0 sha_match=1 -> PASS ::`. Nothing compiled or run (R76/R78).
