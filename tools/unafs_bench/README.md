# unafs_bench

A single-shot stress benchmark for **UnaFS**, the UnaOS virtual filesystem.

## What it does

`unafs_bench` exercises the `unafs` library end to end against a real on-disk
image and prints a timing report. It runs a fixed scenario:

1. **Provision** — creates a fresh, pre-sized disk image
   (`100_000` blocks of `BLOCK_SIZE`) under the platform local-data directory
   (`<data_local>/unaos/bench_vault.img`), removing any prior image first, then
   formats a new UnaFS instance on it.
2. **High-volume metadata write** — creates `10_000` files in the root
   directory. Each file is given two attributes: an `embedding`
   (`AttributeValue::Vector` of 384 random `f32`s, simulating an AI embedding)
   and a `type` string cycling through `engram` / `directive` / `noise`. The
   wall-clock cost is recorded as *write latency*.
3. **Cold-boot recovery** — flushes metadata with `sync_metadata()`, drops the
   filesystem, re-opens the same image, and `mount()`s it. The mount cost is
   recorded as *recovery latency*, and the recovered root listing is asserted to
   contain exactly `10_000` entries.
4. **Compound query** — runs a semantic query that combines vector similarity
   with an attribute predicate
   (`similarity(embedding, <vec>) > -1.0 AND type == "engram"`) and records the
   *query latency*. Every returned inode is verified to actually carry
   `type == "engram"`.
5. **Telemetry report** — prints write latency, cold-boot recovery time,
   compound-query speed, and the number of matched inodes, then deletes the
   image.

Assertions make the binary a smoke test as well as a benchmark: a recovery
mismatch or a query that returns a wrong-typed or zero result aborts the run.

## Key types and entry point

This crate is a thin driver; it defines no public types of its own. `fn main()`
is the only entry point. It drives the following `unafs` API surface:

- `FileSystem` — the `unafs::UnaFS<FileDevice>` alias used throughout.
- `FileDevice::open(path)` — opens the backing disk image as a block device.
- `FileSystem::format(device, size_mb)` / `FileSystem::mount(device)` —
  initialize a new filesystem or remount an existing one.
- `create_file(parent_id, name)` and
  `set_attribute(inode_id, key, AttributeValue)` — populate metadata.
- `sync_metadata()` — persist in-memory metadata before the simulated reboot.
- `query(query_str)` — run a similarity-plus-predicate query, returning
  `(Inode, score)` pairs.
- `ls(inode_id)` — list a directory for cold-boot verification.
- Constants/types re-used from `unafs`: `BLOCK_SIZE`, `AttributeValue`.

## How it fits into UnaOS

`unafs_bench` is one of the command-line **vessels** under `tools/`
(alongside `unafs`, `vertex`, and `sentinel`). It depends only on the `unafs`
library and is the performance/regression harness for the storage layer rather
than part of the interactive userspace — it does not use Bandy, Synapse, or
Quartzite. See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../../docs/dev/USERLAND/ARCHITECTURE.md)
for the vessel/handler/library model and [`docs/CODEX.md`](../../../docs/CODEX.md)
for the system canon.

## Usage

```sh
cargo run -p unafs_bench --release
```

The scenario is hard-coded; there are currently no command-line arguments or
configuration knobs.

## Status

Implemented. The benchmark compiles and runs against the current `unafs`
library, with fixed workload parameters (`10_000` inodes, `100_000` blocks,
384-dimension embeddings). Parameterization (counts, dimensions, query
thresholds) is not yet exposed.
