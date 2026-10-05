# UnaFS on-disk records: the byte-level specification (§R)

**Status:** normative. UNAFSCODEC (SR53) wrote this down and derived `unafs::codec` from it; before
SR53 the record bytes were whatever `bincode` (1.3.3, then 2.x `legacy()`) produced, so the format
the kernel, the installer, Amber Bytes and the host tools share was defined by a third-party crate's
rules. Now this page defines them and the crate implements this page. Every record below is held to
golden vectors (`unaos/libs/fs/unafs/tests/kat_vectors.rs`, `btree_kats.rs`), to a property oracle
whose digests were cut against bincode 2.0.1 (`tests/codec_oracle.rs`), and to whole-volume images
cut under bincode (`tests/codec_volume.rs`). `tools/unafs dump --records` prints a volume field by
field from this page (§R15).

Companion documents: the B+tree node (`btree.md`, referenced from §R11a), the paged maps
(`docs/dev/evidence/rmbp-1005/UNAFSMAP.md`), the grow flag (`docs/dev/evidence/rmbp-1005/UNAFSGROW.md`).

## §R0 Volume geometry

* Block size 4096 B, fixed (`Superblock.block_size` must be 4096). Block `n` lives at byte
  `n × 4096` of the volume (the partition, or the whole disk on a superfloppy; a partition adapter
  remaps, it never re-encodes).
* Block 0 — the superblock (§R2) at byte 0, zero padding to 4096.
* Block 1 — the two root slots: slot A = sector 0 (bytes 0..512), slot B = sector 1 (512..1024),
  each a root record (§R12) at the sector's head, zero padding. Bytes 1024..4096 are zero.
* Every other block is reached from the active root: map leaves and index nodes (§R13), inode
  blocks (§R3), data/directory/system-object extents, indirect blocks (§R8), B+tree nodes (§R11a).
* Versions: 3 (K8 CoW), 4 (+extent spill §R8), 5 (+two-level legacy refmap), 6 (+B+tree catalog
  §R11, +inode meta trailer §R7), 7 (+paged maps §R13). This build mounts 3..=7 and formats 7; v2 is
  read-only (§R14). A version outside the range is refused, never guessed at.

## §R1 Encoding rules (the "record codec")

Applied to the records in §R2–§R10 and §R14. All multi-byte integers are **little-endian**.
**No varints anywhere**; every length is a fixed `u64`.

| Type | Encoding | Size |
| :--- | :--- | :--- |
| `u32` | 4 bytes LE | 4 |
| `u64` | 8 bytes LE | 8 |
| `i64` | 8 bytes LE two's complement | 8 |
| `f32` | IEEE-754 binary32 bit pattern as `u32` LE (NaN payload and sign of zero preserved) | 4 |
| `f64` | IEEE-754 binary64 bit pattern as `u64` LE (same) | 8 |
| `[u8; N]` | the N bytes, no length | N |
| `len` | `u64` LE count | 8 |
| `String` | `len` = byte count, then that many bytes of UTF-8 (no terminator) | 8 + n |
| `Vec<T>` | `len` = element count, then each element | 8 + Σ |
| `Map<String, V>` | `len` = pair count, then `String` key ‖ `V` per pair, keys in ascending byte order | 8 + Σ |
| enum | discriminant `u32` LE (0-based, in the order this page lists), then the variant's payload | 4 + payload |
| struct | its fields in the order this page lists, nothing between them, no length, no tag | Σ |

Versioning: the record codec carries no per-record version; the **superblock version** (§R0)
decides which records and trailers a volume holds. A field is never added to a §R1 record in
place — new data rides a magic-discriminated hand-packed trailer (§R7, §R8) or a new object, and an
incompatible change bumps the volume version.

**Decoding.** A record is decoded from a PREFIX of its byte source: bytes after the record are not
part of it (records sit in zero-padded blocks; an inode is followed by its trailers). A decoder:

1. refuses a length prefix whose minimum payload (`len × min-encoded-size(T)`) exceeds the record
   budget (`LimitExceeded`) or the bytes the source still holds (`Truncated`) — **before any
   allocation**;
2. refuses an enum discriminant ≥ the variant count (`BadVariant`) and a `String` that is not
   UTF-8 (`BadUtf8`);
3. on a repeated map key keeps the LAST value (writers never repeat a key; this is the rule every
   existing volume was read under);
4. never panics; every refusal names the record, the field (`Type.field`) and its byte offset.

Budgets (total bytes one decode may consume): **8192 B** for block records (superblock, inode,
indirect trailer — a block record that claims more is corrupt by definition) and **4 MiB** for
extent-backed records (directory lists, flat catalog, spilled attribute values, overflow extent
list, snapshot index, reclaim queue), well under the kernel's free heap.

Minimum encoded sizes (used by rule 1): `FileKind` 4, `Extent` 24, `AttributeValue` 12, `Inode` 44,
`IndirectTrailer` 32, `DirEntry` 20, `CatalogEntry` 24, `SnapshotEntry` 48, `ReclaimEntry` 16,
`u64` 8, `f32` 4, `u8` 1, a map pair 8 + min(V).

## §R2 Superblock (block 0, 37 bytes)

| off | len | field | notes |
| ---: | ---: | :--- | :--- |
| 0 | 5 | `magic` `[u8;5]` | `"UNAFS"` |
| 5 | 4 | `version` `u32` | 3..=7 |
| 9 | 4 | `block_size` `u32` | 4096 |
| 13 | 8 | `block_count` `u64` | volume span in blocks; ≥ 3; ≤ 2^31 on v7, ≤ 2^28 on v5/v6, ≤ 2^19 before v5 |
| 21 | 8 | `root_inode` `u64` | always 1 |
| 29 | 8 | `catalog_inode` `u64` | always 2 |

Static after format except: a GROW rewrites `block_count` (with root flag GROW, §R12), the v6 → v7
migration rewrites `version` (with root flag MIGRATE). The superblock is all fixed-width, so a decode
of any 37 bytes succeeds — validity is the magic + version + geometry check, never the decode.
KAT: `Superblock::new(4096)` v7 = `554e414653 07000000 00100000 0010000000000000 0100000000000000
0200000000000000`.

## §R3 Inode block (one 4096 B block per inode)

```
 [ Inode record §R6 ][ meta trailer §R7 (v6+) ][ spill trailer §R8 (v4+, only if spilled) ][ zeros ]
```

The inode map (§R13) names each inode's current block by its LOGICAL id. The trailers are
discriminated by their magic at the exact end of the previous part; zero padding can never spell a
magic. A v3–v5 block has no meta trailer; a block without a spill trailer is byte-identical to the
pre-v4 format.

## §R4 `FileKind` (enum, 4 bytes)

| discriminant | variant |
| ---: | :--- |
| 0 | `File` |
| 1 | `Directory` |
| 2 | `Symlink` |
| 3 | `System` (attribute catalog, snapshot index, reclaim queue) |

## §R5 `Extent` (24 bytes) and `AttributeValue` (enum)

`Extent`: `logical_offset u64` (byte offset in the object) ‖ `physical_block u64` ‖ `length u64`
(bytes). An extent list is `Vec<Extent>`.

`AttributeValue`:

| discriminant | variant | payload |
| ---: | :--- | :--- |
| 0 | `Int` | `i64` |
| 1 | `Float` | `f64` |
| 2 | `String` | `String` |
| 3 | `Blob` | `len` ‖ bytes |
| 4 | `Vector` | `len` (element count) ‖ `f32` × len |

KATs: `Int(-42)` = `00000000 d6ffffffffffffff`; `String("hello")` = `02000000 0500000000000000
68656c6c6f`; `Vector([0.1, 0.2, 0.9])` = `04000000 0300000000000000 cdcccc3d cdcc4c3e 6666663f`.

## §R6 `Inode` record

| # | field | type |
| ---: | :--- | :--- |
| 1 | `id` | `u64` logical inode id |
| 2 | `kind` | `FileKind` §R4 |
| 3 | `size` | `u64` logical byte size |
| 4 | `chunks` | `Vec<Extent>` (inline part only when spilled, §R8) |
| 5 | `attributes` | `Map<String, AttributeValue>` — small values |
| 6 | `large_attributes` | `Map<String, Vec<Extent>>` — the extents holding an encoded `AttributeValue` (§R10) |

`parent`, `name`, `ctime`, `mtime`, `atime` are NOT part of this record (they ride §R7). Small vs
large: a `Vector` over 64 elements, a `Blob` or `String` over 256 bytes goes to `large_attributes`.
KAT: `Inode::new(101, File)` = `6500000000000000 00000000 0000000000000000 0000000000000000
0000000000000000 0000000000000000` (44 bytes).

## §R7 Inode meta trailer (v6+, hand-packed, 42 + n bytes)

| off | len | field |
| ---: | ---: | :--- |
| 0 | 8 | magic `"UNAFSMT1"` (`u64` LE `0x31544d5346414e55`) |
| 8 | 8 | `parent` `u64` (0 = none) |
| 16 | 8 | `ctime` `u64` unix seconds |
| 24 | 8 | `mtime` `u64` |
| 32 | 8 | `atime` `u64` (stamped at create and write only — noatime) |
| 40 | 2 | `name_len` `u16`; `0xFFFF` = name not stored |
| 42 | n | name, UTF-8, n ≤ 255 |

Refused: magic present but fewer than 42 bytes; `name_len` > 255 (and ≠ 0xFFFF) or past the block;
a name that is not UTF-8.

## §R8 Spill trailer `IndirectTrailer` (v4+, record codec)

| # | field | type |
| ---: | :--- | :--- |
| 1 | `magic` | `u64` = `0x554E414653585054` ("UNAFSXPT" read as a big-endian number; bytes on disk `54 50 58 53 46 41 4e 55`) |
| 2 | `total_extents` | `u64` — inline + overflow |
| 3 | `overflow_len` | `u64` — byte length of the overflow list |
| 4 | `index` | `Vec<Extent>` — the indirect blocks holding the overflow |

The overflow is a `Vec<Extent>` (§R1) stored across `index` (extent-backed budget); the reader
appends it to `chunks` and checks the total. A writer keeps as many leading extents inline as fit
`4096 − 1024 (trailer reserve) − |inode without chunks| − |meta trailer|`, 24 B each.

## §R9 Directory data: `Vec<DirEntry>`

A `Directory` inode's data (its extents) is one `Vec<DirEntry>`; an empty directory holds the
8-byte empty list. `DirEntry`: `name String` ‖ `inode_id u64` (logical) ‖ `kind FileKind`.

## §R10 Spilled attribute value, flat catalog

* A large attribute's extents hold one encoded `AttributeValue` (§R5).
* v3–v5 attribute catalog (inode 2's data): `Vec<CatalogEntry>`, `CatalogEntry` = `key_hash u64` ‖
  `val_hash u64` ‖ `inode_id u64` (FNV-1a 64 of the key bytes; the value hash per `catalog::hash_value`).

## §R11 v6+ catalog record (hand-packed, 40 bytes) and §R11a B+tree nodes

Inode 2's data on v6+: `"UNAFSCX1"` ‖ `eq_root u64` ‖ `ord_root u64` ‖ `entries u64` ‖ FNV-1a 64 of
bytes 0..32 (`u64` LE). The roots name `UNAFSBT1` B+tree nodes; the node layout and the key
encodings are specified in `btree.md` §2 and `index.rs` (equality key `key_hash ‖ val_hash ‖
inode_id`, ordered key `key_hash ‖ tag ‖ value ‖ inode_id`, all big-endian for byte order).

Snapshot index (inode 3's data): `Vec<SnapshotEntry>`, `SnapshotEntry` = `generation u64` ‖
`imap_block u64` ‖ `imap_leaves u64` ‖ `name String` ‖ `creator String` ‖ `timestamp u64`.
Reclaim queue (inode 4's data): `Vec<ReclaimEntry>`, `ReclaimEntry` = `generation u64` ‖
`blocks Vec<u64>`. Both exist (empty, 8 bytes) from format.

## §R12 Root record (hand-packed, 80 bytes, one per 512 B slot of block 1)

| off | len | field |
| ---: | ---: | :--- |
| 0 | 8 | magic `"UNAFSRT1"` |
| 8 | 8 | `generation` `u64` (0 = invalid slot) |
| 16 | 8 | `imap_block` — top node of the inode map |
| 24 | 8 | `imap_leaves` |
| 32 | 8 | `next_inode` |
| 40 | 8 | `refmap_block` — top node of the refcount map |
| 48 | 8 | `refmap_leaves` |
| 56 | 8 | `free_blocks` |
| 64 | 8 | `flags`: bit 0 `GROW` (superblock rewrite may lag a grow), bit 1 `MIGRATE` (maps paged though block 0 may still say v6); other bits 0 |
| 72 | 8 | FNV-1a 64 of bytes 0..72 |

Mount takes the valid slot (magic, checksum, generation ≠ 0) with the higher generation; a commit
writes the other slot as ONE sector — the format's only in-place write.

## §R13 Maps (hand-packed raw arrays)

* Refcount-map leaf: 1024 × `u32` LE — the reference count of blocks `leaf × 1024 + i`.
* Inode-map leaf: 512 × `u64` LE — slot `leaf × 512 + i` holds logical inode `i`'s current block,
  0 = unallocated.
* v7 paged index node: 255 × 16 B entries `(block u64, sum u32, used u32)` (`block` 0 = hole: an
  all-zero subtree with no block; `sum` = `h ^ (h >> 32)` truncated to `u32`, `h` = FNV-1a 64 of the
  child's 4096 B; `used` = non-zero entries under the child), then `"UNAFSMN1"` at 4080 and FNV-1a
  64 of bytes 0..4088 at 4088. Levels are a pure function of the leaf count (fan-out 255).
* v3–v6 legacy index: 512 × `u64` LE raw child pointers; one level up to 512 leaves, two past it
  (v5+ only). No sums, no holes.

## §R14 Version-2 superblock (read-only migration source, 77 bytes)

`magic [u8;5]` ‖ `version u32` (2) ‖ `block_size u32` ‖ `block_count`, `root_inode`, `free_blocks`,
`bitmap_start`, `bitmap_blocks`, `journal_start`, `journal_blocks`, `catalog_inode` (8 × `u64`).
v2 inode ids are physical block ids; `legacy::LegacyVolume` reads it, nothing writes it.

## §R15 Implementation map and proofs

* `unafs::codec` — §R1 (`Writer`, `Reader`, `Encode`, `Decode`, the budgets, `DecodeError {record,
  field, offset, kind}`); the per-record impls sit beside each type (`inode.rs` §R4–§R6/§R8, `fs.rs`
  §R9/§R11 lists, `catalog.rs` §R10, `superblock.rs` §R2, `legacy.rs` §R14). Hand-packed records:
  `root.rs` §R12, `inode.rs::meta_bytes/apply_meta` §R7, `index.rs` §R11, `btree.rs` §R11a,
  `maptree.rs`/`refmap.rs` §R13.
* Proofs: `tests/kat_vectors.rs` (golden bytes, unchanged by SR53), `tests/codec_oracle.rs`
  (2,000 drawn values per record type × 12 types; digests cut against bincode 2.0.1 at the M1
  commit), `tests/codec_volume.rs` (v3/v5/v6/v7 whole-volume images cut under bincode, byte-equal),
  `tests/codec_fuzz.rs` (mutated records: never a panic, every refusal typed and field-named).
* `tools/unafs dump --records <image>` prints every record of a volume, field by field, in this
  page's names and order.
