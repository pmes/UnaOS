# SOURCE-ALONG — the media carries the source that built it

Every piece of UnaOS **boot media** ships with the source tree that produced it, at the volume
root:

| File      | Contents                                                                    |
|-----------|-----------------------------------------------------------------------------|
| `SRC.TGZ` | `git archive HEAD \| gzip -9` of the repository — **tracked files only** |
| `SRC.SHA` | sha256 of `SRC.TGZ`, plus the exact `commit` / `branch` / `describe` provenance |

Both names are 8.3-clean, so they survive a FAT32 root directory unmangled and are readable by
the kernel's own FAT reader.

## Why

1. **Self-hosting, rung 1.** The direction is a machine that can rebuild itself. The first
   prerequisite is that the code is *on* the machine, not on some other machine. Media that
   carries its own source is the cheapest possible version of that rung, and it exists before any
   in-tree compiler does.
2. **GPL mechanics by construction.** The repository is GPL-3.0-or-later (per-file `SPDX-License-Identifier`
   governs; see `docs/SECURITY.md` and the repo `LICENSE`). GPLv3 §6 lets a binary distributor
   satisfy the source-availability obligation by conveying the corresponding source *on the same
   medium*. Packing `SRC.TGZ` beside `kernel.elf` means every stick we hand anyone is already
   compliant — no written offer, no separate download, nothing to remember at hand-off time.

## What packs it, and when

`unaos/arroyo` → `pack_source_along <media-dir>`, one implementation, called from the media
commands:

- `./arroyo esp-x86`   → `target/x86_64_esp/{SRC.TGZ,SRC.SHA}`
- `./arroyo esp-arm`   → `target/aarch64_esp/{SRC.TGZ,SRC.SHA}`
- `./arroyo esp-jetson`→ `target/aarch64_esp/{SRC.TGZ,SRC.SHA}`

Default **ON** for those three. `UNAOS_NOSRC=1` skips the step (and removes any stale pair from
the media directory) for tight media and CI speed.

Deliberately **not** carried, because these are size-budgeted images whose contents are enumerated
or byte-verified by tests:

- `fat-img` / `test-fat` — 96 MiB FAT32 / 32 MiB FAT16 kernel-FAT-reader fixtures (they set
  `UNAOS_NOSRC=1` themselves and strip a pair left by an earlier `esp-x86`);
- `vm-image` — fixed 64 MiB ESP (`builder/src/vm_image.rs`, `ESP_BYTES`); strips the pair so the
  image stays byte-comparable to the pre-SOURCE-ALONG one;
- `kernel8` — the Pi 4 bare-metal card image and its installer clone-verify enumerate the staged
  boot directory file by file.

Size today: ~5.6 MiB compressed. Irrelevant on a 64 GB stick; the reason for the exclusions above
is fixture discipline, not bytes.

## Manifest discipline

`esp-x86` and `esp-arm` print, at build end and beside the `kernel.elf` sha256, the size and
sha256 of `SRC.TGZ`. Those lines land in a bench MANIFEST by the ordinary copy-the-tail habit, so
a flashed stick's source payload is identified by hash in the same record as its kernel.

## Verifying a stick

From the mounted volume:

```sh
cat SRC.SHA                                  # sha + commit/branch/describe
shasum -a 256 SRC.TGZ                        # must equal SRC.SHA's first line
tar -tzf SRC.TGZ | head                      # sane tree: unaos/arroyo, crates/kernel/src/...
tar -tzf SRC.TGZ | awk '/^target\//' | wc -l # must be 0 — build output is never archived
```

To rebuild from it: `tar -xzf SRC.TGZ -C <dir> && cd <dir>/unaos && ./arroyo esp-x86`.

## Dirty trees

`git archive HEAD` archives **committed state**. If the working tree was dirty at build time, the
binaries on the media may not correspond exactly to `SRC.TGZ`. That case is *stamped, not hidden*:
`SRC.SHA`'s `describe` line comes from `git describe --always --dirty` and ends in `-dirty`. A
`-dirty` stamp means "this media is not reproducible from its own source payload" — fine for a
bench iteration, never acceptable for media handed to anyone else.

## Pre-registered follow-on

The installer engine does **not** yet propagate `SRC.TGZ` to installed systems: an installed UnaOS
should land the same pair on its root volume so an installed machine is as self-carrying as the
stick it came from. That is media-independent work in the installer payload path and is
deliberately out of the packaging arc that introduced this file.
