# UNAFSCODEC — UnaFS's on-disk records get UnaFS's own codec (SR53)

Branch `exec-fs-unafscodec`, cut at 22178aeb; `exec-rmbp-merge13` (boot-23 integration: UNAFSMAP v7,
BOOT80, UNAFSGROW, AMBER1 `unafs::format`, DEPS) merged first, clean (723cabf1). No new crate, no knob.

## Finding

Every bincode-shaped UnaFS record (superblock, inode, extent, attribute value, indirect trailer,
directory list, flat catalog, snapshot index, reclaim queue, v2 superblock) was encoded by
`bincode 2.0.1` `legacy()` through `serde` derives. The format the kernel, installer, Amber Bytes and
the host tools share was therefore defined by a third-party crate (DEPS SR31: OWED, pinned because
bincode 3.0.0 is a `compile_error!` tombstone). The hand-packed records (root, meta trailer, catalog
record, B+tree node, map nodes/leaves) were already UnaFS's own but were specified only in code
comments scattered across five files.

## What was built

* **Spec** — `docs/dev/OS/09_FILESYSTEM/unafs-records.md` (§R0–§R15): geometry, the §R1 encoding rules
  (fixed-width LE, `u64` lengths, `u32` enum discriminants, NO varints, struct = fields in order),
  decode rules (prefix decode, claims checked before allocation, last-wins map keys, typed refusals,
  8 KiB / 4 MiB budgets, per-type minimum sizes), then every on-disk record with offsets/widths:
  superblock, inode block layout, `FileKind`, `Extent`, `AttributeValue`, `Inode`, meta trailer,
  spill trailer + overflow, `DirEntry` lists, large attributes, flat catalog, catalog record (+ the
  B+tree pointer to `btree.md`), snapshot index, reclaim queue, root record (incl. GROW/MIGRATE
  flags), refcount/inode map leaves, v7 paged and legacy index nodes, the v2 superblock. Versioning:
  no per-record versions; the superblock version gates records/trailers.
* **Codec** — `unafs::codec`: `Writer`/`Reader` (the §R1 primitives), `Encode`/`Decode` traits
  (hand impls beside each type, no macro needed), `DecodeError { record, field, offset, kind }` with
  `Truncated` / `LimitExceeded` / `BadVariant` / `BadUtf8`. Same seam signatures as before
  (`serialize`, `deserialize`, `deserialize_block`, `deserialize_block_prefix`) plus `decode_prefix`
  and `encoded_len`. Hardening is stricter than bincode's: a length claim must fit both the budget
  AND the bytes present before anything is allocated (bincode allocated any claim under budget).
* **Deps** — `bincode` and `serde` out of `unafs/Cargo.toml`; the `bincode` pin out of
  `tools/deps-audit/pins.toml` and its OWED verdict out of `classify.toml`; `docs/dev/DEPS.md`
  regenerated. Root and `unaos/` lockfiles pruned: the kernel workspace no longer pulls `serde`,
  `serde_derive`, `bincode`, `unty`. (`unaos/builder/Cargo.lock` still has bincode 1.3.3 — that is
  `mbrman`'s, unrelated to UnaFS.)
* **Tool** — `tools/unafs dump --records --img X`: superblock, both root slots (active named),
  every inode (record, meta trailer, spill trailer, overflow list), every large attribute value,
  directory lists, flat catalog / catalog record, snapshot index, reclaim queue — one line per field,
  `<record> @<off> <Type.field> <type> = <value>`, walked with the §R1 primitives, NOT the library's
  decoders (a second reading of the spec).

## Oracle

1. **Per-record property oracle** (`tests/codec_oracle.rs`): 12 record types × 2,000 seeded draws
   (boundaries: 0/MAX/powers of two, NaN with payload, ±inf, −0.0, subnormals, multi-byte UTF-8 and
   NUL, empty/long lists, multi-key maps, the in-RAM-only inode fields set). At M1 (9fe7260f, bincode
   still a dependency) every encoding was asserted **byte-equal to bincode 2.0.1 `legacy()`**, and on
   4 mutations per draw (8,000 per type, 96,000 total) the two **decoders agreed** on accept/refuse
   and on the value — zero disagreements. The FNV-1a digests of bincode's bytes are pinned; from M2
   the same draws must reproduce them without bincode.
2. **Whole-volume images** (`tests/codec_volume.rs`): one deterministic script (fixed clock) builds
   v7, v6, v5 (flat catalog) and v3 volumes reaching every record (all attribute variants, large
   attributes, a 220-extent spilled inode, batch path, snapshots, rename/unlink, a NON-empty reclaim
   queue). Image digests were cut at 723cabf1 under bincode; with the hand codec the images are
   **byte-identical** (v7 `0xe605fd3f4d00e53b`, v6 `0x9fd7ba0da19302b1`, v5 `0x02e4528e3cbcdf0f`,
   v3 `0x13b9449adaad13a3`) and read back to the same digests.
3. **KAT vectors** (`tests/kat_vectors.rs`): unchanged hex goldens; only the helper's trait bounds
   changed (`serde` → `codec::{Encode, Decode}`).

## Fuzz (M3, `tests/codec_fuzz.rs`)

20,004 mutated records (12 types × 1,667; 1–3 stacked mutations: bit flip, truncation, overwrite,
hostile u64 splice, appended garbage), each decoded under both budgets = 40,008 decodes under
`catch_unwind`: **0 panics**; 18,942 accepted, 21,066 refused (truncated 12,750, limit 4,254, variant
2,690, utf8 1,372), every refusal naming record + `Type.field` + offset in range. A counting global
allocator bounds each decode's peak heap by 64× input + 64 KiB: max seen **10.9×**. Plus 4,000
mutated whole inode blocks through `Inode::decode_block` (record + meta + spill trailers) and 4,000
mutated sectors through the root / catalog-record / superblock parsers: no panic.

## Results

Commits: merge 723cabf1 · M1 9fe7260f (spec + codec, bincode-equal) · M2 1bffcf68 (bincode/serde out,
pin dies) · M3 2605bdde (fuzz) · M4 on the tip (dump --records, this doc).

- `cargo test --release -p unafs -p unafs-cli` (repo root): exit 0, 33 targets, **233 passed**, 0 failed,
  1 ignored (the `map_bench` row bench). The 209 pre-existing tests are unchanged and green; new:
  `codec_oracle` 13, `codec_volume` 4, `codec_fuzz` 4, `unafs-cli dump_records` 3.
- `cargo tree -p unafs -e normal`: no `bincode`. `--no-default-features` (the kernel's shape):
  `libm`, `thiserror` only.
- `tools/deps-audit/run.py --check`: 108 crates, **0 failing** (5 behind latest stable, all pinned;
  the `bincode` pin is gone).
- `unafs dump --records` cross-check (`tools/unafs/tests/dump_records.rs`) on library-written v7, v6
  and v5 volumes: 8 inodes each, 80 / 80 / 62 field groups agree with the library (superblock, active
  root generation, every inode's id/size/kind, inline + overflow extents of a spilled inode, small and
  large attribute values, meta trailer parent/name, directory lists, snapshot index, a non-empty
  reclaim queue, v6+ catalog record / v5 flat catalog). The dump never mounts (a mount would drain
  the reclaim queue): it walks the inode map from the root record (§R13, checking paged child sums).
- Kernel: x86 `cargo +nightly check --release` from `unaos/crates/kernel` with
  `--features unafs,ahci,ahciroot`: exit 0, no warning in a touched file; aarch64 default leg (unafs
  linked unconditionally there): exit 0. Kernel source changes are comment-only (two references to
  bincode now cite §R2).

## Third-party crates this arc leans on

None for the codec. `unafs` normal deps after SR53: `thiserror` 2.0 (utility: error derive), `libm`
0.2 (utility: float math), and under `std` only: `anyhow`, `memmap2`, `bandy`/`gneiss_pal` (in-tree).
`serde` remains in `unafs`'s std tree solely through `bandy` (the host bus's message JSON) — not
used by any UnaFS record; under `no_std` (the kernel) the tree is `thiserror` + `libm` only.

## Ceiling / owed

* No kernel-written volume (the boot-21 card's p2) was available in this container; the dump's
  cross-check ran on volumes the unafs library writes (the kernel links the same library, so the
  bytes are the same code's, but a flight capture is still owed: `unafs dump --records --img p2.img`).
* `dump --records` takes a bare volume image; a partitioned card needs the partition extracted
  first (a `--partition` flag via `locate_unafs` is a small follow-up).
* The B+tree nodes and map index nodes are specified (§R11a/§R13) but not printed by the dump.
* Unflown (R78): host-proven only; the kernel legs are type-checks.
