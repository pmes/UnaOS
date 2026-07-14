# Sentinel

A command-line self-verification tool for the UnaOS repository.

## Overview

Sentinel is a CLI vessel (`tools/sentinel`) that audits the integrity of a
UnaOS checkout in a single pass. It confirms that the working tree is in fact a
UnaOS workspace, that the project manifest matches what is on disk, that the
on-disk filesystem image is well-formed, and that the full source tree can be
reduced to a single reproducible fingerprint. It is intended to be run from the
repository root as a fast pre-flight / CI sanity check, and it exits non-zero
when any verification fails.

## Responsibilities

Sentinel runs three sequential phases from `main`:

1. **Structural verification.** It reads the project manifest (`MEMORIA.md`,
   falling back to `MEMORIA.md`) and parses every declared `[CRATE]` / `[BIN]`
   artifact entry with a `regex`, then checks that each referenced path exists.
   Manifest entries that point at missing files (recorded as failures) and
   `[SHELL]` entries (skipped) are handled distinctly. This catches manifests
   that have drifted from the actual repository layout.

2. **Vault integrity.** It opens the primary UnaFS vault image as a read-only
   block device, reads block 0, and decodes the superblock. On success it
   reports the UnaFS format version and total/free capacity; a missing image is
   reported as a warning (not an error), and a corrupt superblock is a failure.

3. **Cryptographic seal.** It walks the working tree (excluding `target/` and
   `.git/`), hashes every file with BLAKE3, and folds the per-file
   `(path, hash)` pairs into one master digest — the "system state hash." File
   hashing runs in parallel via `rayon`, using BLAKE3's memory-mapped,
   Rayon-backed `update_mmap_rayon` for throughput.

On completion Sentinel prints the elapsed time and the system state hash, or, if
any phase recorded an error, the error count. The process exits with status `1`
on failure and `0` on success.

## Key types and entry points

Sentinel is a single-binary crate; its logic lives in `src/main.rs` and it
exposes no public library API. Its behavior is driven by types from the three
internal libraries it depends on:

- `elessar::Context` / `elessar::Spline` — workspace/context detection.
  Sentinel builds a `Context` for the current directory and aborts unless its
  `spline` is `Spline::UnaOS`, ensuring it only runs against a UnaOS tree.
- `unafs::{BlockDevice, FileDevice, Superblock, BLOCK_SIZE}` — the UnaFS client.
  `FileDevice::open_read_only` plus `BlockDevice::read_block` and
  `Superblock::from_bytes` drive the vault check; `Superblock::version`,
  `block_count`, and `free_blocks` are surfaced in the report.
- `gneiss_pal::paths::UnaPaths::primary_vault()` — resolves the canonical vault
  location on the host.

External dependencies: `blake3` (hashing), `rayon` (parallelism), `ignore`
(`WalkBuilder` tree walk), `regex` (manifest parsing), `anyhow` (error
handling), and `colored` / `directories`.

## How it fits into UnaOS

Sentinel is one of the command-line vessels under `tools/` described in the
[userspace architecture](../../../docs/dev/USERLAND/ARCHITECTURE.md). Unlike GUI
vessels such as Lumen, it does not start a Tokio runtime, the Bandy message bus
(`SMessage` / `Synapse`), or a Quartzite window; it composes only the storage
and context libraries (Elessar, UnaFS, Gneiss PAL) to perform a one-shot
integrity audit and report to stdout.

## Status

Implemented. All three phases run today against a host checkout. The vault phase
is gated on the presence of a UnaFS image and treats its absence as a non-fatal
warning.
