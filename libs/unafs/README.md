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
  reported as a dirty volume. A mark-and-sweep **fsck-scavenger** (`fsck.rs`)
  reclaims the crash-window residue the crash-ordered mutation engine leaves —
  leaked blocks and query-orphaned inodes — and `UnaFS::recover` runs it on a
  dirty host mount and clears the dirty flag.
- **Extent-based data layout.** File data is mapped through extents (logical
  offset to physical block runs), with contiguous appends coalesced into a
  single extent.

## Key public types and entry points

The crate root (`lib.rs`) re-exports the primary surface:

- **`UnaFS<D: BlockDevice>`** (`fs.rs`) — the filesystem itself, generic over a
  storage backend. Lifecycle: `format(device, size_mb)` and `mount(device)`.
  Operations include `mkdir`, `create_file`, `ls`, `resolve_path`, `read_data`,
  `write_data`, `set_attribute` / `get_attribute` / `remove_attribute`,
  `unlink`, `rename`, and `query`. The convenience alias
  `FileSystem = UnaFS<FileDevice>` binds it to a host file.
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
  operators, plus `cosine_similarity` (in `fs.rs`, re-exported at the crate
  root) for vector scoring.
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
and file operations, journaled writes, inline and spilled attributes, the
mutation set (`unlink`, `rename`, `remove_attribute` — each with full
attribute-catalog cleanup, so a deleted file or attribute can never be
returned by a query), crash-recovery (the **fsck-scavenger** and the
dirty-mount `recover` path), and the attribute/similarity query engine all
work over `FileDevice` and `MemDevice`. Some areas are early-stage: the WAL is
a dirty-detector, not a redo/undo log — recovery reconciles the volume rather
than replaying logged block images (see below) — and the attribute catalog is
append-only on `set_attribute` (mutations scrub it). The crate runs as an
ordinary host process today and is converging onto the UnaOS kernel: the
**`no_std` core port (BeFS-K1) has landed** (see below).

### Mutation crash windows and recovery (honest note)

The mutation operations are crash-**ordered**, not crash-**atomic**. The WAL
detects a torn operation on the next mount and reports a dirty volume; it does
**not** carry redo/undo block images, so a classic log replay is not
expressible in the current on-disk format (growing that format is a separate,
KAT-recutting arc). Instead, every mutation is sequenced so that no
intermediate on-disk state ever references a freed block — catalog and
directory rewrites go new-extents-first with a single-block inode swap last
(each rewrite is old-or-new, never torn), and block frees come last of all.
The only residue an ill-timed power cut can leave is therefore bounded and
structurally sound: a **leak** (allocated-but-unreachable blocks), plus in one
cross-directory `rename` window a **query-orphan** (a file reachable by query
but by no name) — never a dangling reference and never a torn structure. The
concrete per-operation windows are documented on `unlink`, `rename`, and
`remove_attribute` in `fs.rs`.

**Recovery (F1).** The fsck-scavenger (`fsck.rs`, `UnaFS::fsck`) walks the
volume from its roots (system blocks, the name tree, the catalog), diffs the
reachable-block set against the allocation bitmap, and — in repair mode —
heals query-orphans (scrubbing their catalog entries via the same crash-ordered
rewrite path, so nothing dangles) and returns every leaked block to the free
pool. `UnaFS::recover` is the host-side dirty-mount entry point: it runs the
scavenger in repair mode and, if the journal was dirty, resets it, clearing the
flag so the next mount is clean. Both are exposed on the `unafs` CLI as
`fsck [--repair]`. The kernel's K3 mount is read-only and never calls them. A
run on a healthy volume reclaims nothing and leaves free-space accounting
untouched (a KAT pins this). This recovery is **best-effort under program-order
writes**: unafs issues no write barriers, so a reordering write-back cache can
still dangle a pointer the ordered design never would — the scavenger is not a
general fsck for arbitrary corruption (the on-disk parser is hardened
separately), and the barrier question is future work.

Open-handle semantics are equally honest: there is no open-file table, so a
caller that keeps a raw inode id across `unlink` touches freed (possibly
reallocated) blocks — the caller's problem, by design.

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
| `query` engine (`Query` parsing, `UnaFS::query`, `cosine_similarity`) | ✅ | ✅ |
| Mutations: `unlink` (full catalog scrub + extent frees) | ✅ | ✅ |
| Mutations: `rename` (same-dir and cross-dir; refuses overwrite and directory loops) | ✅ | ✅ |
| Mutations: `remove_attribute` (inline and spilled; index entries scrubbed) | ✅ | ✅ |
| Recovery: `fsck` scavenger + `recover` (leak reclamation, query-orphan heal, dirty-flag clear) | ✅ | ✅ |
| `FileDevice` (host file backend) | ✅ | — |
| `io::MappedFile` (memmap reader) | ✅ | — |
| bandy `BandyMember` events on attribute change | ✅ | — |

The `no_std` build is verified against the kernel's own bare target
(`aarch64-unknown-none-softfloat`), where `std` is genuinely absent from the
sysroot.

The query engine's floating-point square roots go through `libm` (pure-Rust,
`no_std`) on **every** build — the `std` build does not fall back to
`f32::sqrt` — so there is exactly one scoring path: a similarity query
answered by the kernel over a mounted volume and the same query answered by a
host tool produce **bit-identical scores**. `tests/query_kats.rs` pins this
contract with golden cosine-similarity vectors (bit-exact known answers), a
std-math-vs-libm identity sweep, and end-to-end `UnaFS::query` score
assertions, including the extent-spilled vector path. Wiring the engine into
the kernel's read-only mount (in-kernel similarity queries over the K3 chain)
is a follow-on kernel arc; this crate side is ready.

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
- Journal recovery **reconciles, it does not replay**: the WAL detects a dirty
  mount, and the fsck-scavenger (F1, landed) reclaims the resulting leaks and
  heals query-orphans — but there is no redo/undo of logged block images, so
  recovery is best-effort under program-order writes (see the crash-window and
  recovery note above), not crash-atomic mutation.
- Directories are flat serialized vectors; no checksums anywhere; extents are
  a flat inline list (large-file depth limit).

Planned arcs (sequencing in [`docs/ROADMAP.md`](../../docs/ROADMAP.md) §2):

| Arc | Content |
| :--- | :--- |
| F1 | Dirty-mount recovery: fsck-scavenger + `recover` — **✅ landed** (mark-and-sweep leak reclamation + query-orphan heal; reconciliation, not redo/undo replay — see the recovery note) |
| F2 | `unlink` / `rename` / `remove_attribute` + catalog removal — **✅ landed** (crash-ordered, no replay; see the crash-window note) |
| F3 | Generic on-disk B+tree (shared by indexes and directories, as BeFS did) |
| F4 | Per-attribute B+tree indexes: log-time equality, true range queries |
| F5 | **Live queries** — delta-emitting persistent queries published over bandy (the query-driven spatial UI, now including similarity) |
| F6–F8 | B+tree directories; metadata checksums; extent trees |
| K1–K4 | Kernel convergence: **`no_std` core (K1, ✅ landed)** → **512↔4096 block adapter + partitions (K2, ✅ landed)** → **read-only kernel mount of a real volume (K3, ✅ landed)** → journaled kernel writes |

The capability model (see [`docs/SECURITY.md`](../../docs/SECURITY.md)) stores
principals and grants as ordinary typed attributes (`owner`, `grants:*`), so
security metadata is queryable and needs no format change.
