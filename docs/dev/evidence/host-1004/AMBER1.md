# AMBER1 — Amber Bytes becomes The Block (ledger SR34)

Branch `exec-host-amber1`. Milestones: M1 `526bca81`, M2 `ad65630c`, M3 `fd78c8d1`, M4 (this doc
plus `docs/dev/OS/10_INSTALL/ENSEMBLE.md`).

## Finding

Amber Bytes' own README said GPT/MBR editing was not implemented. The GPT logic lived only in
`unaos/libs/sys/amber_core`, which had the read half and an encoder, plus six hand-rolled readers in
the kernel. UnaFS could be formatted only by `tools/unafs init`, inline in a CLI. The installer
ensemble (B296, R79) had no host disk leg and no bus surface.

## What exists now

| Layer | Code | What it does |
|---|---|---|
| `amber_core` (no_std, zero deps) | `block.rs`, `mbr.rs`, `plan_apply.rs`, `verify.rs`, `recover.rs`, `fat32_format.rs`, `file_block.rs` (std only), `kat_write.rs` | the `Block` seam (`MemBlock`, `SparseBlock`, `Window`, `Recorder`, `FileBlock`); legacy and protective MBR encode, decode and classify; plan → disk with an exact dry run (the same write list is shown and written); a whole-table verify; backup-header recovery (both directions, plus relocation on a grown disk); a complete FAT32 format |
| `unafs` | `src/format.rs` | `unafs::format(&mut block, &FormatParams)` and `format_partition(&mut sectors, base, len, ..)`; `tools/unafs init` calls it |
| handler | `handlers/amber_bytes/src/{bus,ops,layout,policy,signer,disks}.rs`, `src/bin/amber.rs` | bus verbs `DiskList`, `PlanLayout`, `Apply`, `Verify`, `Recover` (JSON); the `amber` CLI; the signed plan; the two-key write policy |

## Spec sections covered

- UEFI 2.x §5.2.3 (protective MBR), §5.3.2 (GPT header: primary, backup, AlternateLBA, CRC),
  §5.3.3 (entry array, CRC, extents). Verify checks every field. Recover rebuilds whichever copy is
  lost.
- The classic MBR partition record (CHS clamp, 0xEE, hybrid detection). A hybrid MBR is reported and
  never overwritten.
- fatgen103 §3 (BPB), §3.3 (FAT32 EBPB), §3.5 (FAT size and the cluster-size table), §4 (reserved
  FAT entries), §5 (FSInfo with a real free count and next-free hint), §6 (the volume-label entry).
  Both FATs are written whole, plus the backup boot sector and the backup FSInfo.
- UnaFS on-disk format v3..v6, through the crate's own `format_with_version`.

## Oracles (every one third-party except UnaFS's)

| What | Oracle | Result |
|---|---|---|
| GPT bytes | the PARTINSTALL Python writer `unaos/scripts/make-gpt-fixture.py` | primary and backup regions byte-identical (M1, `amber_core/tests/oracle.rs`) |
| GPT read back | util-linux `partx` + `blkid` | every extent, name, GUID and type matches; `amber` e2e: `1 2048 264191 UNAOS-ESP / 2 264192 395263 UNAOS-UNAFS` |
| card table | `tools/una-card`'s real output | byte-identical to the `amber plan` dry run (`tools/una-card/tests/card_table.rs`), and the golden MBR CRC `F2DAC7B7` |
| FAT32 | dosfstools `fsck.fat -n -v`, `mkfs.vfat` geometry, mtools `minfo` | clean: `1 files, 1/258048 clusters` on the 128 MiB card ESP; `minfo` reads the label and FAT32 in place in the image |
| MBR | libblkid DOS reader (`partx`) | legacy layouts read back (M1) |
| UnaFS | none outside UnaOS. The oracle is the unafs crate's own `mount` + `fsck(false)` on the bytes as written, through `BlockAdapter` at the partition's LBA | clean, generation 1 |

## KATs and tests

- `cargo test -p amber_core`: 8 tests. `kat::run` and `kat_write::run` carry the known-answer suite
  and run in-kernel too. `tests/oracle.rs` is 4 oracle tests.
- `cargo test -p unafs --test format_entry`: 5. Byte-identical to `format_with_version` for every
  version under a pinned clock; mount + fsck clean; a narrowed volume leaves the rest untouched; a
  partition format touches nothing outside the partition; every refusal writes nothing. The full
  unafs suite is 185 passed, 0 failed.
- `cargo test -p amber_bytes`: 12 tests.
  - 6 unit tests: the sha256 FIPS vectors; the layout is the golden card plan, with an emit/parse
    round trip; size parsing and refusals; a synthetic sysfs tree; the policy on images and non-images;
    the policy on a real block node (refused without the flag, refused when not listed, refused when
    mounted, never opened).
  - 1 for `gpt show`.
  - 5 end to end on images: the card plan → apply → verify path with partx, fsck.fat and minfo; dry
    run, plus a signature refused after the medium grows; recover from a lost primary, a lost backup
    and a grown disk, each byte-identical to the original; the JSON bus over `amber bus`, where bus
    and CLI give the same table and ESP bytes; the Python fixture verifies and recover finds it
    healthy without writing.

## The signed plan

The signature covers the canonical bytes: `amber-plan v1`, the table's render (every write's LBA,
length and CRC-32), then a format section. That section holds every FAT32 write at its absolute LBA
with its CRC, and each UnaFS volume's version and block count. UnaFS's bytes carry the format clock,
so they are not in the digest. `Apply` re-plans for the medium it opens and refuses on any mismatch.
The scheme today is `sha256`, an unkeyed digest: it proves the caller saw this plan. `trait Signer`
takes HMAC later.

## Third-party crates

| Crate | Version | Role |
|---|---|---|
| `sha2` | 0.11.0 (latest) | the plan digest. **Utility.** The in-tree hash belongs to CRYPTOCORE (SR27) and can replace it when it lands |
| `serde`, `serde_json` | 1.0.229, 1.0.151 (latest, added by this arc) | the bus wire. **Utility** |
| `clap`, `memmap2`, `memchr`, `indicatif`, `hex`, `rand` | already in the crate | the forensic CLI. **Utility** |

No crate encodes a table or a filesystem. `amber_core` has zero dependencies. The `unafs` core is
linked with `default-features = false`.

## Honest ceiling

- Real disks: the policy gates writes to them and they are enumerated read-only, but no test writes
  a real disk.
- No clone verb on the host (`ClonePlan` exists; the kernel runs it).
- MBR-only layouts are not offered by the CLI (the core encodes them).
- No keyed `Signer`.
- The JSON wire is dispatched in-process and over `amber bus` stdio. It is not yet carried on bandy:
  that needs a shared `SMessage` variant, and the seat adds it at the fold.
- No GPT editing in place (add, delete or resize one partition on an existing table). A layout is a
  whole table.
- 512-byte logical sectors only. A 4Kn disk is listed with its `logical_block_size` but not planned.

## Owed

The kernel's six GPT readers onto `amber_core` (AHCIROOT's kernel sibling); host clone; HMAC
`Signer` under Holocron; the bandy carriage; in-place table edits; 4Kn. The CODEX entry stays as it
is: Amber Bytes is already chartered for "Partitioning (GPT), Formatting, Block-level recovery", so
no new handler is proposed.

## How to continue

Run `cargo test --release -p amber_core -p amber_bytes -p unafs`. The oracles skip when absent.
Start from `handlers/amber_bytes/src/ops.rs`: every verb is one function there over `&mut dyn Block`.
