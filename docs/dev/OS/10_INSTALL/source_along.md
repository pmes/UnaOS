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

## Rung 2 — the shard reads its own source (`UNAOS_SELFHOST=1`)

Everything above is rung 1: the code is *on* the machine. But the verification recipe in the previous
section runs on a **host** — UnaOS itself had no way to check the payload it was carrying. Rung 2
closes that: the kernel opens `SRC.TGZ` off the program-source volume, verifies it against `SRC.SHA`,
decompresses it, and enumerates the tree, with no host in the loop.

`crates/kernel/src/selfhost/` — three files, all `no_std + alloc` and arch-neutral:

| File | Contents |
|------|----------|
| `mod.rs` | mount, stamp parse, the one-pass read/hash driver, the witness |
| `inflate.rs` | streaming gzip (RFC 1952) + DEFLATE (RFC 1951) decoder |
| `tar.rs` | streaming POSIX ustar member walker (a census, never an extraction) |

**Read-only, and streaming at both ends.** Nothing here writes to media — the FAT volume is reached
through the ordinary `fs::fat` reader and only `read_at` is ever called. Neither end of the pipe is
buffered whole: the compressed file is pulled in 512 KiB chunks that feed the SHA-256 *and* the
inflater in one pass, and the decompressed tar is walked 512 bytes at a time and discarded. Against a
48 MiB kernel heap and a tree that only grows, a `Vec<u8>` of the whole archive was never an option.

**Four independent claims**, so a PASS is not one number vouching for itself:

1. `sha256(SRC.TGZ)` equals the digest on line 1 of `SRC.SHA` — the payload is the one that was packed;
2. the gzip trailer's CRC-32 and ISIZE match what the decoder produced — the **decoder** is right, not
   merely self-consistent (a broken inflater cannot forge the packer's CRC over its own garbage);
3. the tar walk reaches the two-zero-block end-of-archive marker — the tree is **complete**;
4. zero members under `target/` — this is source, not build output (the same check the shell recipe
   above spells as `tar -tzf SRC.TGZ | awk '/^target\//' | wc -l`).

Witness, on success:

```
:: SELFHOST: SRC.TGZ on <volume> tgz=… raw=… crc=0x… commit=… describe=… first=… ::
:: SELFHOST: src verified sha=<64 hex> files=… dirs=… bytes=… target=0 crc=ok -> PASS ::
```

A medium that legitimately carries no pair (any `UNAOS_NOSRC=1` build — the FAT fixtures, `vm-image`,
the Pi card) gets an honest line and no verdict, never a fault:

```
:: SELFHOST: no SRC.TGZ/SRC.SHA on <volume> — SOURCE-ALONG not packed on this medium ::
```

Every other outcome — a stamp without a payload, a sha mismatch, a decode or walk failure, a member
under `target/` — ends `-> FAIL ::` and names which claim broke.

**Gate:** `./arroyo test-selfhost [part|gpt|p16|sf] [secs]`. It is the one lane that packs
SOURCE-ALONG *onto* a FAT fixture, and it has to be: `test-fat` sets `UNAOS_NOSRC=1` precisely so its
image stays enumerable byte for byte, so a gate riding `test-fat` would look for a payload the fixture
had just been told not to carry — vacuous, and green while vacuous. `test-selfhost` leaves
SOURCE-ALONG on for `esp-x86` and passes `UNAOS_SRCFIXTURE=1` to `make-fat-img.sh`, which stages the
pair onto the image only under that knob (so every other fixture's root directory stays byte-stable).
The lane asserts the `-> PASS ::` line positively rather than trusting the generic fault scan — a boot
that never reached the witness at all would pass a scan for the *absence* of faults.

The lane re-`exec`s itself with `UNAOS_SELFHOST=1` already in the environment. That is not tidiness:
`arroyo`'s knob→feature mapping is evaluated once at script load, before dispatch, so a knob exported
from inside a lane function never reaches `arroyo`'s own `cargo build` — which is the build that runs
last and produces the kernel that boots.

**Verified at landing** (2026-08-11, base `3bc0ead0`): `./arroyo check` green both arches, plain and
under `UNAOS_SELFHOST=1` and `UNAOS_WC=1` (12 cfg-coverage legs each, zero new warnings). Reachability
proven on the built kernel rather than claimed from the banner — an armed
`target/x86_64-unaos/release/unaos-kernel` carries 14 `SELFHOST` strings including
`:: SELFHOST: src verified sha=`, i.e. the linker kept the module, so the call site is live. That
check earned its place immediately: the first draft put the call at only ONE of `main.rs`'s three
storage-ready passes — the `usbdebug` one — so the module linked out of every ordinary boot while the
feature banner read `selfhost`. It now sits at all three, the way `probe_once` and the FATVERB witness
already do, with the latch inside making it speak once.

Knob-off identity: the loaded image is byte-identical to baseline (both strip to
`fd27cd58ca3af83f1f5317b3add4bdfddbabc2a03326d3979a0ec3b10d9a2811`). The unstripped ELFs differ by 66
bytes, all of it in `.strtab` — LLVM's `.llvm.<hash>` internal-symbol disambiguator moves whenever a
compilation unit's text changes at all, including by a comment or a `cfg`-disabled line. No allocated
section differs in size or content.

**Not in this rung, deliberately:** extraction. Materialising members onto a volume is a write path
with a genuinely different blast radius, and it is a separate rung. What this one establishes is the
precondition for every rung after it — a future in-tree build tool cannot compile a source tree it
cannot verify, decompress, or enumerate.

## Pre-registered follow-on

The installer engine does **not** yet propagate `SRC.TGZ` to installed systems: an installed UnaOS
should land the same pair on its root volume so an installed machine is as self-carrying as the
stick it came from. That is media-independent work in the installer payload path and is
deliberately out of the packaging arc that introduced this file.
