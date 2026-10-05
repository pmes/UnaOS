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
- `gpt show <image>` — read-only: validate and print the GPT (via `amber_core`).

All inspection and search paths open their target read-only. The `image` source
is read-only; only its destination is written. `wipe` is the sole path that
opens a target for writing, and it refuses to run without `--force`.

## The layout CLI (`amber`) and the bus (AMBER1, SR34)

The disk-layout half of The Block, as a library (`src/lib.rs`) with a bus surface and a second
binary. Underneath it is `unaos/libs/sys/amber_core` (GPT, MBR, plan to disk, whole-table verify,
backup-header recovery, the complete FAT32 format) and `unafs::format` (UnaFS). Nothing in this
crate encodes an on-disk structure itself.

```text
amber list                                        DiskList: sysfs + mounts, opens no device node
amber plan    <target> --layout L | --part ...    PlanLayout: the exact writes + sha256; writes nothing
amber apply   <target> --layout L --sha256 HEX    Apply: refused unless HEX is the plan's for THIS target
              [--dry-run] [--create] [--yes-i-mean-it --allow DEV]
amber verify  <target>                            Verify: table + every partition probed; exit 1 on FAIL
amber recover <target> [--write]                  Recover: the lost GPT copy from the good one
amber bus                                         the JSON wire on stdio (one request per line)
```

Layout file (`amber-layout v1`; the compact `--part kind:size:name[:format]` form is equivalent):

```text
amber-layout v1
disk-seed UNAOS-X86-CARD
part esp 262144s UNAOS-ESP seed=UNAOS-X86-ESP format=fat32 label=UNAOS
part unafs 131072s UNAOS-UNAFS seed=UNAOS-X86-UFS format=unafs
```

Bus verbs (`src/bus.rs`; JSON, `{"verb": ...}`): `DiskList`, `PlanLayout`, `Apply`, `Verify`,
`Recover`. Topics are `amber/disk/list`, `amber/plan/layout`, `amber/apply`, `amber/verify`,
`amber/recover`. The full table, the signed-plan rule and the write policy are in
`docs/dev/OS/10_INSTALL/ENSEMBLE.md` §2. The short version:

- **Signed plan.** `Apply` derives the plan again for the target it opens, and refuses unless the
  presented signature verifies over that plan's canonical bytes. The scheme today is `sha256`.
  `trait Signer` is where a keyed HMAC scheme plugs in.
- **Write policy.** An image file is always writable. A block device needs `--yes-i-mean-it` AND an
  allowlist entry (`--allow` / `AMBER_ALLOW`), and is never written while it is mounted.
- **Read back.** After a write, the table is re-verified against the plan, every FAT32 write is
  re-read, and every UnaFS volume is mounted and fscked.

## Status

- **Forensic CLI: implemented.** All five subcommands function.
- **GPT: read side live, over the shared core.** `amber_bytes gpt show <image>`
  validates and prints a GUID Partition Table (protective MBR, primary header and
  entry array with CRCs, backup header) using `unaos/libs/sys/amber_core` — the
  `no_std` disk-layout core the kernel installer (`install/gpt.rs`,
  `install ssd`) and `tools/una-card` (the x86 card image) link too, so the table
  this tool reads and the one the kernel lays are one code path (SELFINSTALL2,
  rmbp-ledger B310).
- **Layout: live (AMBER1).** `amber plan/apply/verify/recover` and the bus verbs
  above, end to end on image files. Real disks are behind the two keys and are
  not exercised by the tests.
- **Not yet here:** clone on the host (`amber_core::ClonePlan` exists; the kernel
  runs it), a keyed `Signer` (HMAC under a Holocron key), carrying the JSON wire
  on bandy (a shared `SMessage` variant, added at the fold), MBR-only (legacy)
  layouts from the CLI (`amber_core::mbr` encodes them).

Dependencies: `amber_core` and `unafs` (in-tree cores, which do the work), then utilities:
`memmap2`, `sha2` (the plan digest), `memchr`, `indicatif`, `clap`, `hex`, `rand`, `serde` and
`serde_json` (the bus wire). There is no async runtime.
