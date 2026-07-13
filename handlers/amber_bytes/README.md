# amber_bytes — "The Block"

Forensic disk and partition recovery for UnaOS. A **forensic tool first,
formatter second**: it inspects, images, searches, and surgically extracts raw
bytes from files and block devices, and destroys data only behind an explicit
safety interlock. It is deliberately **not** a file manager and **not** a
durable-memory service.

## Charter

Per the `docs/CODEX.md` Handler Manifest, amber_bytes is "The Block" — the
forensic recovery handler. Its job is to let an operator reason about, preserve,
and recover raw storage: bit-exact imaging with cryptographic proof, pattern
hunting across whole devices, and precise byte extraction, plus a guarded
destructive wipe. Read-only by default; every destructive path requires an
explicit `--force`.

> Provenance note: from March–July 2026 this crate also carried a durable-memory
> "vault" actor (a UnaFS `DiskManager` engram store) that had been extracted out
> of `vein` and bolted on here (Jules commit `3839cff`). The AMBER-CHARTER arc
> returned that actor to its home in `vein` (`vein::vault`); amber_bytes is once
> again purely The Block.

## The CLI (`amber_bytes`)

A single forensic binary (`src/main.rs`). Subcommands:

- `inspect` — read-only hex/ASCII dump of the first 128 bytes (memory-mapped).
- `image` — bit-exact copy with a live progress bar and a SHA-256 of the source.
- `search` — scan for a `--text` or `--hex-pattern` needle (memchr), with
  context windows around each match.
- `extract` — copy a byte range (`--offset`/`--length`) to an output file.
- `wipe` — destructively overwrite with zeros or random data; requires `--force`.

All inspection and search paths open their target read-only. The `image` source
is read-only; only its destination is written. `wipe` is the sole path that
opens a target for writing, and it refuses to run without `--force`.

## Status

- **Forensic CLI: implemented.** All five subcommands function.
- **Block-device / partition management** (GPT/MBR editing, partition-table
  recovery, mount policy, the Two-Key destructive turn) described in the charter
  design notes is **not yet implemented** in this crate.

Dependencies: `memmap2`, `sha2`, `memchr`, `indicatif`, `clap`, `hex`, `rand`.
No bus, no async runtime, no filesystem library — The Block is a standalone tool.
