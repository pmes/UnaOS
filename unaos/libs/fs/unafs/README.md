# unafs

The UnaOS virtual filesystem client: a metadata-rich, block-based store that
treats file attributes — including AI embedding vectors — as first-class,
queryable data.

## What it does

`unafs` is a self-contained filesystem implementation that lays out an entire
on-disk volume (superblock, A/B root sectors, inode map, refcount map, inodes,
and data) over a
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
- **Crash-consistency by construction (K8a, copy-on-write).** Every mutation
  allocates fresh blocks — nothing reachable from the last committed root is
  ever overwritten. Each public operation is one transaction, committed by an
  atomic 512 B root-sector flip (A/B generation-stamped, checksummed slots in
  block 1; mount picks the newer valid one). A power cut anywhere — including
  a tear inside the root write itself — yields the old committed tree or the
  new one, never a hybrid. There is no journal: atomicity is structural. An
  **fsck** (`fsck.rs`) recomputes reachability and diffs it against the
  persisted refcount map (media-corruption defense; `UnaFS::recover` repairs).
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
- **`Superblock`** (`superblock.rs`) — the STATIC volume identity at block 0
  (magic, version, geometry, the reserved logical inode ids); written once at
  format, never rewritten.
- **`RootRecord` / `RootSlot`** (`root.rs`) — the 512 B root record and the A/B
  slot discipline: the single atomically-flipped sector the live tree hangs
  from (`ROOT_RECORD_SIZE` ≤ 512 is compile-time asserted and KAT-pinned).
- **`RefMap`** (`refmap.rs`) — the refcount allocator (current/frozen views).
- **`SnapshotEntry` / `ReclaimEntry`** (`fs.rs`) — the snapshot-index and
  reclaim-queue object payloads; `SnapshotEntry::drop_permitted` is the
  owner-or-kernel drop-authority decision. Retention verbs live on `UnaFS`:
  `snapshot_create` / `snapshot_drop` / `snapshot_index`.
- **`legacy`** (`legacy.rs`) — the read-only pre-K8 (v2) walker +
  `migrate_into`, feeding `tools/unafs migrate`.
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
handler and the `tools/unafs` vessel build on it, and attribute changes are
surfaced to other handlers through Bandy (`SMessage` / `Synapse`).

## Status

Implemented and functional: format, mount, directory and file operations,
copy-on-write transactional writes, inline and spilled attributes, the
mutation set (`unlink`, `rename`, `remove_attribute` — each with full
attribute-catalog cleanup, so a deleted file or attribute can never be
returned by a query), retained roots / snapshots with eager crash-safe
reclamation (K8b), refcount-consistency fsck (multi-root aware), a one-way
pre-K8 migration reader, and the attribute/similarity query engine, over
`FileDevice` and `MemDevice`, host and kernel (`no_std`) alike. The attribute catalog is still
append-only on `set_attribute` (mutations scrub it) — the F4 index arc's
target.

### The K8a copy-on-write core (format v3)

Everything mutable hangs from a single **root record** that fits ONE 512 B
sector (a compile-time assert and a golden KAT both enforce it): commit =
write all fresh blocks → barrier → write the *inactive* of two A/B
generation-stamped, checksummed root slots (block 1) → barrier. Mount reads
both slots and follows the newer valid one, so even a torn root write costs
only the crashed commit. Key pieces:

- **Stable logical inode ids** (Fork-1 verdict): `DirEntry.inode_id`,
  `CatalogEntry.inode_id`, and the kernel's ACL keys never change across
  mutations. The **inode map** (`imap`: id → current physical block, itself
  CoW'd — raw u64 leaves + one index block) is the pointer graph's spine;
  ids are never recycled, so a stale id fails `NotFound` instead of aliasing
  a new file.
- **Refcount allocator** (`refmap.rs`): free ⇔ reachable from no retained
  root. Two views — `current` (in-flight transaction) and `frozen` (as of the
  last commit); allocation requires BOTH zero, which structurally forbids
  overwriting any block the on-disk tree still reaches. Counts persist CoW
  (raw u32 leaves + index block) each commit.
- **Retained roots / snapshots (K8b)**: the root reaches a **snapshot index**
  and a **persistent reclaim queue**, both ordinary UnaFS objects (`System`
  inodes at reserved ids 3 and 4). A snapshot is a retained root — see the
  dedicated section below.
- **Bench counters** (`CommitStats`): commits, blocks written, blocks per
  last commit — read via `commit_stats()`; the kernel witness pairs them with
  CNTPCT ticks.
- **Crash-simulation seam**: `set_autocommit(false)` performs mutations
  without ever flipping the root — dropping the instance then models a power
  cut mid-commit exactly (the recovery suite and the kernel `K8a-cow` witness
  are built on it).

Open-handle semantics stay honest: there is no open-file table, but under
stable logical ids a caller that keeps an id across `unlink` now gets
`NotFound` — never another file's data.

### Snapshots: retained roots + reclamation (K8b)

A snapshot is a **retained root** — an O(1), block-sharing point-in-time image
of the whole volume, the substrate for UnaFS-native versioning and the two-root
diff (K8c).

- **`snapshot_create(name, creator, timestamp)`** retains the current committed
  tree: it records a generation-stamped `SnapshotEntry` (the `name`, `creator`
  principal, and `timestamp` are K6 typed attributes) in the on-disk snapshot
  index, and increfs **every block the retained root reaches** through its inode
  map. This is the security core of retention: the allocator invariant is
  *free ⇔ reachable from no retained root*, so a block a snapshot lives on
  carries one refcount per referencing root and can never be reallocated while
  the snapshot lives. Nothing is copied — the snapshot and the live tree share
  every block until a mutation forces the live tree onto fresh blocks (the
  ordinary CoW write path). Returns the snapshot's **generation stamp** (its
  unique, never-recycled id). v1 policy caps retention at `SNAPSHOT_CAP` (16),
  refused cleanly with `SnapshotCapReached` — the index is an unbounded growable
  object, so lifting the cap is a constant change, not a format migration.
- **`snapshot_drop(generation)`** never frees blocks directly: it removes the
  index entry and enqueues the snapshot's block set on the persistent reclaim
  queue in **one atomic commit**, then drains eagerly — decrefing each block,
  which frees it iff no live or retained root still reaches it. A power cut
  mid-drain is crash-safe: the entry stays on the queue and the next mount's
  eager drain resumes and converges (`snapshot_drop_enqueue` exposes the
  enqueue-only half for exercising exactly that seam). Drop is destructive and
  authorized owner-or-kernel (`SnapshotEntry::drop_permitted`).
- **`snapshot_index()`** lists the retained roots; **`reclaim_queue()`** the
  pending drops. `fsck` walks the live root *and* every retained root, and its
  repair rebuilds true multi-root refcounts.
- **Bench counters** (`CommitStats`): `snapshots_created` / `snapshots_dropped`.

The shell verbs `usnap` / `usnaps` / `usnapdrop` and the host `unafs snap` /
`snaps` / `snapdrop` subcommands drive this surface directly.

### Snapshot reads: the read path under current-ACL (K8c)

`open_snapshot(generation) -> SnapshotView` opens a retained root for **reading**.
The view resolves paths, lists directories, reads data, and reads attributes **as
they were at snapshot time** — `resolve_path` / `ls` / `read_data` / `read_inode`
/ `get_attribute`, keyed through the snapshot's frozen inode map. It is
**read-only by construction**: `SnapshotView` exposes no mutating method, so "a
snapshot cannot be written" is a property of the type, not a runtime policy check.
Reads share the *same* bounded primitives the live mount uses (the crate's
`*_via` read functions), and they touch **nothing** mutable — no refcount, no
reclaim queue, no root flip; the view holds its own map and issues only block
reads. A view of a dropped or unknown generation fails closed
(`SnapshotNotFound`), so a dangling read handle is unrepresentable.

**Authority is not in the crate.** The `SnapshotView` is pure bytes; it applies
no access control. The *governing rule* (Peter, 2026-07-16, "we want high
security") is that a snapshot read is authorized by the **live object's CURRENT
ACL**, re-evaluated at read time — enforced one layer up, at the kernel verb /
ACL seam ([`fs/unafs.rs`](../../../crates/kernel/src/fs/unafs.rs), `read_authz` /
`snapshot_read`). Every snapshot-read surface (`usnapcat`, `usnapls`, the host
mirror) defers to that ONE evaluator. Honest scoping: `read_authz` enforces the
same *semantics* as the live syscall path — current-ACL, CAP_READ-equivalent
grant rights, fail-closed on a deleted object — but it is a kernel-verb-layer
evaluator distinct from the syscall layer's OwnedFile/FileGrant machinery;
unifying the two evaluators is a ledgered follow-up (SECURITY.md K8c entry).

- **Revocation is total, and it reaches the past.** A principal that cannot read
  the live object cannot read *any* snapshot of it. Snapshots preserve bytes,
  never authority — dropping a grant retroactively closes every retained copy to
  that principal.
- **Grant rights are honored, not just grant presence.** A `grants:<principal>`
  row admits a snapshot read only if its `rw`/`r`/`w` rights value carries the
  READ right — decoded by the syscall layer's own `rights_from_native` and tested
  against its `CAP_READ` bit, so a write-only grantee reads neither the live
  object nor any snapshot of it.
- **Deleted-from-live fails closed (documented consequence).** An object deleted
  from the live tree still has its bytes on disk (a retained root pins them), but
  it has **no live ACL row** — so the current-ACL check refuses it for *every*
  principal, the owner and kernel authority included. Deletion is the ultimate
  revocation; the refusal is traced honestly (`DenyNoLiveObject`), never silent.
  (This is the strict reading of the high-security ruling: the retained bytes are
  unreadable through the enforced path once the live object is gone.)

The kernel verbs `usnapcat <gen> <path>` (read a file under current-ACL) and
`usnapls <gen> [path]` (list a snapshot directory), and the host
`unafs snapcat <gen> <path> --as <principal>`, drive this surface.

### The mutation set: `unlink` / `rename` / `remove_attribute` (F2)

Each mutation is **one transaction** — every rewrite it makes becomes visible at
a single root flip, or none does. The pre-K8 crash windows (a name indexed but
not present, an entry in neither directory, extents leaked by a half-finished
delete) are gone by construction, not by recovery.

- **`unlink(parent_id, name)`** removes the directory entry, **every** catalog
  index entry for the inode, the inode's data and spilled-attribute extents,
  its extent-index (indirection) blocks, the inode block, and its inode-map
  slot. Returns the freed logical id. Only `File` and `Symlink` are accepted;
  a directory is refused with `IsADirectory` (there is **no `rmdir`** — the
  crate does not remove directories, and that is outside F2's scope).
- **`rename(parent, old, new_parent, new)`** rekeys a **name**, never bytes:
  the inode, its data, and the catalog (which keys on the stable logical id)
  are untouched, so a cross-directory move copies nothing. Both directory
  rewrites land in the same transaction. An existing destination is
  **refused** (`FileExists` — a deliberate divergence from POSIX's silent
  overwrite); a directory moved into itself or a descendant is `DirectoryLoop`;
  renaming an entry to its own name is a no-op `Ok`.
- **`remove_attribute(inode_id, key)`** drops the inline **or** spilled value
  and every catalog entry for that (inode, key) pair, so a removed attribute
  can never be returned by a later query — including a removed `Vector`, which
  simply drops out of the similarity candidate set rather than scoring against
  a stale index row. A key that is absent is `AttributeNotFound`, never a
  silent no-op.

**Why an unlink cannot destroy a snapshot's bytes.** `unlink` never *frees*
blocks; it **decrefs** them. The allocator invariant is *free ⇔ reachable from
no retained root*, and `snapshot_create` increfs every block its retained root
reaches — so a block that a snapshot and the live tree share carries a count
per referencing root. Unlinking the live name drops exactly the live tree's
reference; while a snapshot still holds one the count stays above zero, the
block is never reallocated, and the snapshot keeps reading the unlinked bytes.
Only when the last root that reaches a block lets go does the count hit zero
and the block become allocatable. This is the same one-count-per-root
accounting `fsck` recomputes across the live root *and* every retained root, so
a mutated volume's refcounts converge and verify clean.

A power cut mid-`rename` is covered by the same structural atomicity as every
other commit: the crate's cut-mid-commit seam (`set_autocommit(false)`, drop,
remount) is exercised against a cross-directory rename in
`tests/mutation_logic.rs` and converges to the old name or the new one — never
both, never neither.

The kernel verbs `urm` / `umv` / `urmattr` drive this surface through the
single IRQ-masked mount, and the uncounted `F2-mutations` witness proves each
mutation durable across a genuine remount on the live card.

### The bulk create+write path (UNAFS-BATCH)

**`create_files_batch(parent_id, Vec<BatchFile>) -> Result<Vec<u64>>`** is the
vectored create/write API. It stages many new files under one parent directory
and lands them together, because the per-op path (`create_file` + `write_data`
+ N × `set_attribute`, each its own root flip) made a whole-tree sync pay one
root flip *per file*: the VAIRE-2 baseline measured the cold native sync at
~11.3 s with **97 % of the wall in the commit phase** across 242 root flips.
Each `BatchFile` carries a `name`, its `data`, and a typed-attribute map; every
child is created as a `File` and the returned ids are in supplied order.

For the whole set the batch reads the parent directory **once** and rewrites it
**once** (vs once per file), folds every file's attributes into its **single
creation inode write** (small inline, large spilled — the same split
`set_attribute` makes), appends every attribute's catalog entry in one pass and
rewrites the catalog **once** (so batched attributes are query-indexed
identically), and commits **once**.

- **Transaction shape.** With autocommit on the whole batch is one root flip.
  With autocommit **off** the batch stages into the caller's larger transaction
  and does not commit — the whole-tree single-transaction shape: a sync drives
  `set_autocommit(false)`, then many `mkdir` + `create_files_batch` (one per
  directory), then a single `commit()`, so the **entire tree lands in one
  flip**. An empty batch is a true no-op (no flip).
- **Unwind (fail closed).** On *any* error mid-batch — a name collision (an
  existing entry or a duplicate *within* the batch, both `FileExists`), a full
  volume (`NoSpace`), an oversized inode — the whole batch unwinds via
  `txn_unwind`, reloading ground truth from the committed root: no partial
  file, name, catalog entry, or leaked block survives, and the allocator is
  poison-closed if even the reload fails (the K8b thaw-unwind precedent). Under
  autocommit-off composition the unwind reloads the committed root, so a
  failure anywhere in a multi-batch whole-tree transaction discards the *entire*
  staged tree — the safe outcome.
- **Snapshot composition.** One batch is one commit, so a `snapshot_create`
  taken *after* a batch retains the whole batch or nothing; refcount/reclaim
  accounting is identical to the per-op path (same CoW primitives — only the
  transaction boundary and metadata churn are batched), proven block-for-block
  by `batched_reclaim_matches_the_per_op_path_block_for_block`.

**Measured before/after.** `tools/unafs bench-batch <dir>` syncs a real
directory tree two ways into fresh throwaway v3 images (the per-op regime — the
control — and the batch path) from one shared scan, isolating the single
variable. Run on the tree the VAIRE-2 baseline used (`~/.claude/plans/unaos`),
**245 files / 11 dirs / 4,212,921 B**, 512 MB images. Host: **MacBookPro16,1**,
**macOS 26.5.2 (25F84)**, internal **APFS** on a **PCI-Express SSD**; release;
**solo**. Per-phase totals in ms; `flips` = root flips since format (the
batch's `2` = the format commit + the one whole-tree commit).

| Run | mode | files | dirs | scan | build | commit | flips | blocks | wall |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **COLD** | per-op (control) | 245 | 11 | 16.14 | 161.90 | 11779.62 | 257 | 41172 | 11941.74 |
| **COLD** | batch | 245 | 11 | 16.14 | 20.07 | 49.90 | 2 | 2018 | 72.48 |
| **WARM** | per-op (245 skip) | 245 | 0 | — | 11.56 | 44.05 | 1 | 131 | 55.77 |
| **WARM** | batch (245 skip) | 245 | 0 | — | 11.33 | 43.32 | 1 | 131 | 54.81 |

The cold commit phase — 98.6 % of the per-op wall — collapses from **~11.8 s
across 257 flips to ~50 ms across 2**: a **165× wall speedup**, exactly the
VAIRE-2 prediction. The `blocks` column shows the second win — 41,172 blocks
written by the per-op path (each attr rewrites the full inode and
re-serializes the parent directory + catalog) drop to 2,018 for the same tree.
The warm all-skip run is lookup-bound and identical for both paths. This path
is the concrete substrate for closing the ledgered ~0.7 s `with_unafs` IRQ mask
([`docs/SECURITY.md`](../../docs/SECURITY.md)); the kernel-side adoption is a
future arc.

### Migration from the pre-K8 format (v2)

Per the do-it-right principle there is **no runtime compatibility** with our
own old format: `UnaFS::mount` refuses a v2 volume. `legacy.rs` is a read-only
v2 walker, and `tools/unafs migrate --from old.img --to new.img` replays the
whole tree (names, data, inline + spilled attributes) into a freshly formatted
v3 image, verifying the result with a post-migration fsck.

## no_std and the feature matrix

The crate is `#![no_std]` + `alloc` by construction, with a single default-on
`std` feature that re-enables the host-native surface. Downstream consumers get
`std` by default and build unchanged; the kernel adapter (a later arc) will
depend on it with `default-features = false`.

| Surface | `std` (default) | `no_std` (`--no-default-features`) |
| :--- | :---: | :---: |
| On-disk types (`Superblock`, `RootRecord`, `Inode`, `Extent`, `AttributeValue`, `FileKind`, `DirEntry`, `CatalogEntry`, `SnapshotEntry`, `ReclaimEntry`) | ✅ | ✅ |
| `codec` (bincode 2.x `legacy()` serialization seam) | ✅ | ✅ |
| `BlockDevice` trait + `MemDevice` | ✅ | ✅ |
| `adapter` (512↔4096 `BlockAdapter` over `SectorDevice`; GPT/MBR parse; `MemSectorDevice`) | ✅ | ✅ |
| `UnaFS` core ops (`format`/`mount`/`read`/`write`/`ls`/`mkdir`/`set_attribute`/`get_attribute`) | ✅ | ✅ |
| `query` engine (`Query` parsing, `UnaFS::query`, `cosine_similarity`) | ✅ | ✅ |
| Mutations: `unlink` (full catalog scrub + extent frees) | ✅ | ✅ |
| Mutations: `rename` (same-dir and cross-dir; refuses overwrite and directory loops) | ✅ | ✅ |
| Mutations: `remove_attribute` (inline and spilled; index entries scrubbed) | ✅ | ✅ |
| Recovery: `fsck` + `recover` (refcount-consistency check/rebuild, stale-index scrub) | ✅ | ✅ |
| `legacy` (read-only pre-K8 v2 walker + `migrate_into`) | ✅ | ✅ |
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

The on-disk byte layout (format **v3**, the K8 CoW format) is **frozen**. The
K8 design pass lifted the pre-K8 freeze once — deliberately, with new goldens
cut the same arc — and re-froze: the superblock (static identity), the
hand-packed 512 B root record (the root-fits-one-sector KAT), and the
snapshot/reclaim entry lists are pinned alongside the record encodings the
format KEPT byte-identical (`Inode`, `Extent`, `AttributeValue`, `FileKind`,
`DirEntry`, `CatalogEntry` retain their ORIGINAL bincode-1.3.3-frozen
goldens). Serialization is bincode 2.x in its `legacy()` configuration, routed
through the single `codec` seam so every write path agrees; the inode-map and
refcount-map leaves are raw little-endian arrays.

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
  entire catalog** — O(n), a scaling cliff rather than an index. (The bulk
  create+write path amortizes this across a batch — one catalog rewrite for the
  whole set — but the per-op single-attribute cost is still O(n) until F4.)
- Directories are flat serialized vectors; data blocks are unchecksummed (the
  root record is checksummed); extents are a flat inline list (large-file
  depth limit).

Planned arcs (sequencing in [`docs/ROADMAP.md`](../../docs/ROADMAP.md) §2):

| Arc | Content |
| :--- | :--- |
| F1 | ~~Journal rollback/replay~~ — **superseded by K8a** (commit is one atomic root flip; there is no torn state to roll back) |
| F2 | `unlink` / `rename` / `remove_attribute` + catalog removal — **✅ landed** (each a single atomic CoW transaction; kernel verbs `urm`/`umv`/`urmattr` + the `F2-mutations` witness complete the surface) |
| F3 | Generic on-disk B+tree (shared by indexes and directories, as BeFS did) |
| F4 | Per-attribute B+tree indexes: log-time equality, true range queries |
| F5 | **Live queries** — delta-emitting persistent queries published over bandy (the query-driven spatial UI, now including similarity) |
| F6–F8 | B+tree directories; metadata checksums; extent trees |
| K1–K4 | Kernel convergence: **`no_std` core (K1, ✅)** → **512↔4096 block adapter + partitions (K2, ✅)** → **read-only kernel mount (K3, ✅)** → **kernel writes (K4, ✅)** |
| K8a–K8c | **Copy-on-write (K8a, ✅ landed — this format)** → **retained roots/snapshots + reclamation (K8b, ✅ landed)** → the two-root diff walk + vaire/Bolt surface (K8c) |

The capability model (see [`docs/SECURITY.md`](../../docs/SECURITY.md)) stores
principals and grants as ordinary typed attributes (`owner`, `grants:*`), so
security metadata is queryable and needs no format change.
