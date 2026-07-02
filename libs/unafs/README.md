# unafs

The UnaOS virtual filesystem client: a metadata-rich, block-based store that
treats file attributes — including AI embedding vectors — as first-class,
queryable data.

## What it does

`unafs` is a self-contained filesystem implementation that lays out an entire
on-disk volume (superblock, journal, free-space bitmap, inodes, and data) over a
simple block-device abstraction. Beyond plain file storage it provides:

- **Semantic metadata.** Every inode carries a typed attribute map
  (`Int`, `Float`, `String`, `Blob`, and `Vector` for embeddings). Small values
  are stored inline in the inode; values over the inline threshold spill to
  external data extents.
- **An attribute query engine.** A small query language supports equality,
  inequality, ordering, and vector cosine-similarity searches
  (e.g. `similarity(embedding, [..]) > 0.8`), with optional secondary `AND`
  filters. A hash-indexed attribute catalog narrows the candidate set before
  full evaluation.
- **Crash-consistency.** A write-ahead journal records the begin/end of each
  mutating operation; on mount, an unterminated transaction is detected and
  reported as a dirty volume.
- **Extent-based data layout.** File data is mapped through extents (logical
  offset to physical block runs), with contiguous appends coalesced into a
  single extent.

## Key public types and entry points

The crate root (`lib.rs`) re-exports the primary surface:

- **`UnaFS<D: BlockDevice>`** (`fs.rs`) — the filesystem itself, generic over a
  storage backend. Lifecycle: `format(device, size_mb)` and `mount(device)`.
  Operations include `mkdir`, `create_file`, `ls`, `resolve_path`, `read_data`,
  `write_data`, `set_attribute` / `get_attribute`, and `query`. The convenience
  alias `FileSystem = UnaFS<FileDevice>` binds it to a host file.
- **`BlockDevice`** (`storage.rs`) — the trait every backend implements
  (`read_block`, `write_block`, `block_count`, `flush`), with `BLOCK_SIZE` of
  4096 bytes. Two backends ship: `FileDevice` (host file) and `MemDevice`
  (in-memory, for tests).
- **`Inode`, `FileKind`, `AttributeValue`, `Extent`, `ExtentList`**
  (`inode.rs`) — the on-disk metadata record and its attribute/data types.
- **`Superblock`** (`superblock.rs`) — volume header at block 0, describing the
  layout (journal, bitmap, root inode, attribute catalog).
- **`Journal` / `JournalOp`** (`wal.rs`) — the write-ahead log and its operation
  records.
- **`Query` / `QueryOp` / `parse_value`** (`query.rs`) — the query parser and
  operators, plus `cosine_similarity` (in `fs.rs`) for vector scoring.
- **`CatalogEntry`, `serialize_catalog`, `deserialize_catalog`** (`catalog.rs`)
  — the FNV-1a-hashed attribute index used to accelerate queries.
- **`MappedFile`** (`io/mmap.rs`) — a zero-copy memory-mapped file that
  implements `gneiss_pal::io::MemoryMappedRegion`, letting other crates consume
  mapped data without depending on the mapping implementation.

`UnaFS` implements `bandy::BandyMember`: after an attribute change it publishes
an `SMessage::FileEvent` describing the change, so other userspace components can
observe filesystem activity over the message bus.

## How it fits into UnaOS

`unafs` is one of the shared libraries under `libs/` in the UnaOS userspace (see
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)).
It is the storage and indexing foundation referenced as the "UnaFS client" in
the system canon ([`docs/CODEX.md`](../../docs/CODEX.md)); the `amber_bytes`
handler and the `apps/cli/unafs` vessel build on it, and attribute changes are
surfaced to other handlers through Bandy (`SMessage` / `Synapse`).

## Status

Implemented and functional as a host-native library: format, mount, directory
and file operations, journaled writes, inline and spilled attributes, and the
attribute/similarity query engine all work over `FileDevice` and `MemDevice`.
Some areas are early-stage: journal recovery currently detects and reports a
dirty mount rather than rolling transactions back, and the attribute catalog is
append-only. The crate runs as an ordinary host process today and is intended to
converge onto the UnaOS kernel as it matures.

## Direction: meeting and surpassing BeFS

UnaFS is a modernized take on the Be File System — BeFS's celebrated ideas
were typed extended attributes, per-attribute B+tree indexes, and *live
queries* (persistent queries whose results update as files change, the basis
of BeOS's query-driven UI). UnaFS already goes beyond BeFS on one axis:
attributes include `Vector` embeddings, and the query engine does cosine
similarity — semantic search as a filesystem primitive.

Known honest caveats in the current implementation, which the arcs below
address:

- The attribute catalog is a flat, hash-bucketed list, (de)serialized whole:
  every non-equality query scans it, and **every `set_attribute` rewrites the
  entire catalog** — O(n), a scaling cliff rather than an index.
- Journal recovery is **detect-only** (reports a dirty mount; no rollback).
- There is **no `unlink`/`rename`/`remove_attribute`** yet; once deletion
  exists, stale catalog entries would break queries — the index arc fixes
  both together.
- Directories are flat serialized vectors; no checksums anywhere; extents are
  a flat inline list (large-file depth limit).

Planned arcs (sequencing in [`docs/ROADMAP.md`](../../docs/ROADMAP.md) §2):

| Arc | Content |
| :--- | :--- |
| F1 | Journal rollback/replay on dirty mount |
| F2 | `unlink` / `rename` / `remove_attribute` + catalog removal |
| F3 | Generic on-disk B+tree (shared by indexes and directories, as BeFS did) |
| F4 | Per-attribute B+tree indexes: log-time equality, true range queries |
| F5 | **Live queries** — delta-emitting persistent queries published over bandy (the query-driven spatial UI, now including similarity) |
| F6–F8 | B+tree directories; metadata checksums; extent trees |
| K1–K4 | Kernel convergence: `no_std` core → 512↔4096 block adapter + partitions → read-only kernel mount of a real volume → journaled kernel writes |

The capability model (see [`docs/SECURITY.md`](../../docs/SECURITY.md)) stores
principals and grants as ordinary typed attributes (`owner`, `grants:*`), so
security metadata is queryable and needs no format change.
