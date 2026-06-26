# amber_bytes

Persistent storage handler for UnaOS userspace. It owns a single UnaFS vault and
serves the system's durable memory — saving, querying, and paging chat/engram
records — and additionally ships a standalone forensic byte-level CLI.

## Responsibility

`amber_bytes` is the storage domain service. It holds an exclusive lock on one
UnaFS volume (the "vault") and is the only component allowed to perform blocking
filesystem I/O against it. All other components reach the vault indirectly, by
publishing storage requests on the message bus.

The crate has two faces:

1. **Handler (`src/lib.rs`)** — the async entry point that integrates with the
   bus. This is the part the rest of UnaOS depends on.
2. **CLI (`src/main.rs`)** — `amber_bytes`, an independent forensic tool for
   raw byte operations on files and block devices. It does not touch the bus.

## How it plugs into the Synapse

Per the userspace convention (see
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)),
the handler exposes:

```rust
pub async fn ignite(vault_path: PathBuf, synapse: Synapse)
```

On startup `ignite` mounts (or formats, on first run / mount failure) the UnaFS
vault on a blocking thread, then runs an actor loop over `synapse.subscribe()`.
Because UnaFS I/O is synchronous and blocking, every request is dispatched to
`tokio::task::spawn_blocking` and the owned `DiskManager` is moved in and out of
the blocking task — it is never driven on the async reactor thread.

### Messages consumed → emitted

| Consumes (`SMessage`) | Action | Emits (`SMessage`) |
| --- | --- | --- |
| `StorageSave` | Create an inode, write content, store the embedding and `type` attribute | `StorageSaveResult { success, error }` |
| `StorageQuery` | Vector-similarity search (threshold 0.45) over `chat`, `directive`, `engram` types plus the 2 latest engrams; top-3 per type by cosine score | `StorageQueryResult { memories, directives, engrams, chrono }` |
| `StorageLoadPaged` | Page stored `chat` records newest-first, returned in chronological order as `DispatchRecord`s | `StorageLoadPagedResult { records }` |

All other `SMessage` variants are ignored. Each request/response pair is matched
by a caller-supplied `receipt_id`. Replies are published with `fire_async`.

### Key types

- `DiskManager` (`src/lib.rs`) — synchronous owner of the UnaFS `FileSystem`;
  provides `save_memory`, `search_memories`, `get_latest_engrams`, and
  `load_paged_memories`. Documented as strictly synchronous and never to be
  called on the reactor thread.
- Records carry `sender`, `timestamp`, `type`, and an embedding `Vector`
  attribute; queries use UnaFS's `similarity(embedding, …)` predicate.

## The CLI (`amber_bytes`)

A separate forensic binary built from the same crate. Subcommands:

- `inspect` — read-only hex/ASCII dump of the first 128 bytes (memory-mapped).
- `image` — bit-exact copy with a live progress bar and a SHA-256 of the source.
- `search` — scan for a `--text` or `--hex-pattern` needle (memchr), with
  context windows around each match.
- `extract` — copy a byte range (`--offset`/`--length`) to an output file.
- `wipe` — destructively overwrite with zeros or random data; requires `--force`.

## Status

- **Handler (storage service): implemented.** `ignite` plus the three
  request/response flows are complete and wired to the live `SMessage`
  variants in `bandy`.
- **CLI (forensic tool): implemented.** All five subcommands function.
- **Block-device / partition management** (GPT/MBR editing, partition recovery,
  mount policy) described in earlier design notes is **not implemented** in this
  crate.

Dependencies: `unafs`, `bandy`, `tokio`, plus `memmap2` / `sha2` / `memchr` /
`indicatif` / `clap` / `rand` for the CLI.
