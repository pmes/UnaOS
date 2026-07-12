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
append-only. The crate runs as an ordinary host process today and is converging
onto the UnaOS kernel: the **`no_std` core port (BeFS-K1) has landed** (see
below).

## no_std and the feature matrix

The crate is `#![no_std]` + `alloc` by construction, with a single default-on
`std` feature that re-enables the host-native surface. Downstream consumers get
`std` by default and build unchanged; the kernel adapter (a later arc) will
depend on it with `default-features = false`.

| Surface | `std` (default) | `no_std` (`--no-default-features`) |
| :--- | :---: | :---: |
| On-disk types (`Superblock`, `Inode`, `Extent`, `AttributeValue`, `FileKind`, `DirEntry`, `CatalogEntry`, `JournalOp`) | ✅ | ✅ |
| `codec` (bincode 2.x `legacy()` serialization seam) | ✅ | ✅ |
| `BlockDevice` trait + `MemDevice` | ✅ | ✅ |
| `adapter` (512↔4096 `BlockAdapter` over `SectorDevice`; GPT/MBR parse; `MemSectorDevice`) | ✅ | ✅ |
| `UnaFS` core ops (`format`/`mount`/`read`/`write`/`ls`/`mkdir`/`set_attribute`/`get_attribute`) | ✅ | ✅ |
| `FileDevice` (host file backend) | ✅ | — |
| `io::MappedFile` (memmap reader) | ✅ | — |
| `query` engine + `cosine_similarity` (needs FP `sqrt`, absent in `core`) | ✅ | — |
| bandy `BandyMember` events on attribute change | ✅ | — |

The `no_std` build is verified against the kernel's own bare target
(`aarch64-unknown-none-softfloat`), where `std` is genuinely absent from the
sysroot.

## The kernel block adapter (BeFS-K2)

`adapter.rs` bridges the kernel's storage primitive to the UnaFS block seam.
The kernel exposes 512 B logical sectors (USB/SD), possibly at a partition
offset; UnaFS speaks 4096 B blocks.

- **`SectorDevice`** — the generic 512 B-sector trait the kernel's block driver
  implements (`read_sector` / `write_sector` / `sector_count` / `flush`). Host
  tests use `MemSectorDevice`, the 512 B twin of `MemDevice`; the kernel wires
  its real driver in the K3 mount arc. `&mut S` is itself a `SectorDevice`, so a
  device can be borrowed for a probe and handed back.
- **`BlockAdapter<S>`** — implements `BlockDevice` over a `SectorDevice`. Block
  `id` maps to the eight contiguous sectors `[base_lba + id*8, +8)`, where
  `base_lba` is the partition offset. `block_count` bounds the exposed volume;
  reads/writes at or beyond it fail `OutOfBounds` before touching the device,
  and every `base_lba + id*8 + i` uses `checked_*` so a crafted or corrupt span
  can never wrap into an in-bounds sector.
- **`parse_partitions`** — reads the MBR at LBA 0; on a protective GPT entry
  (type `0xEE`) it parses the GPT header at LBA 1 and its entry array, otherwise
  the four MBR primaries. It validates the boot/GPT signatures, entry size,
  entry-count range, and LBA ordering, and rejects any partition whose extent
  runs past the backing device.
- **`locate_unafs`** — parses the table, then probes each partition's block 0
  for the `UNAFS` superblock magic, returning the first match as a
  `PartitionSpan { base_lba, block_count }` ready for
  `BlockAdapter::for_partition`. The volume is identified by its on-disk
  signature, not a reserved partition type, so any partitioning tool works.

The adapter is a pure block-level remap: it never touches serialization, so the
frozen on-disk format (and its KATs) are unaffected.

## On-disk format and the KAT contract

The on-disk byte layout is **frozen**. Serialization is bincode 2.x in its
`legacy()` configuration (little-endian, fixed-int width, no length limit),
which reproduces the historical bincode 1.3.3 encoding byte-for-byte, routed
through the single `codec` seam so every write path agrees.

`tests/kat_vectors.rs` holds golden-vector known-answer tests: every struct that
reaches disk is serialized with representative and boundary values and asserted
byte-for-byte against baked-in hex literals, in both directions (serialize ==
golden, and deserialize(golden) == value). Any layout drift — a field reorder, a
type change, a codec-config regression — fails a KAT immediately. **These vectors
must never be edited to make a change pass; they are the format contract.**

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
| K1–K4 | Kernel convergence: **`no_std` core (K1, ✅ landed)** → **512↔4096 block adapter + partitions (K2, ✅ landed)** → read-only kernel mount of a real volume → journaled kernel writes |

The capability model (see [`docs/SECURITY.md`](../../docs/SECURITY.md)) stores
principals and grants as ordinary typed attributes (`owner`, `grants:*`), so
security metadata is queryable and needs no format change.
