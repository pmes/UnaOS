# TOOLING1 — run record (2026-10-13)

Executor branch `exec-rmbp-local-tooling1`, based at `2c538013f`. Bench scripts only: no QEMU, no boot, no
real device. Every medium below is a plain file. `$S` = `~/unaos-bench/scratch/rmbp-0915/local1/tooling1`
(the fixture scripts `mkloop.sh`, `runloop.sh`, `runscorer.sh` and `mut/runmut.sh` live there, with the full
outputs). Commands run from the worktree root unless shown from `unaos/`.

## 1. media-writer.sh — versioned, C14 TREE-FITS, release-on-failure

| step | command | rc | result |
|---|---|---|---|
| import | `cp -p ~/unaos-bench/tools/media-writer.sh unaos/scripts/media-writer.sh` | — | sha256 `8fbd618e…50cd8a` both sides (commit `b53615584`) |
| baseline selftest | `./unaos/scripts/media-writer.sh --selftest $S/st-baseline` | 0 | `SELFTEST: 45 passed, 0 failed` |
| final selftest (x2) | `./unaos/scripts/media-writer.sh --selftest $S/st-final` (and `st-final2`) | 0, 0 | `SELFTEST: 56 passed, 0 failed` both runs |
| version | `media-writer.sh --version` | 0 | `media-writer 2026-10-13.c14 sha256=3f32bc9a660dc1caf4ea3d9090aec5aee9f74cdf30ba39529368d786551f53be path=…` |

### The loop file (`bash $S/mkloop.sh`)

`truncate -s 64M fat64.img; mkfs.vfat -F 32 -s 1 -n UNAOS-ORIN -i 12345678 fat64.img` — a 64 MiB FAT32 volume
(512 B clusters, 66,058,752 B free). `tree-ok` = kernel.elf + EFI/BOOT/BOOTAA64.EFI + MANIFEST; `tree-big` =
the same plus an 80 MiB `UnaOS-orin-card.img` (orin 27's shape: a card image left inside the staged tree).
Every run passes `--geom-from $L/no-geom-record.md --allow-geom-absent` (a bare volume has no MBR geometry).

| run (`bash $S/runloop.sh <repo>/unaos/scripts/media-writer.sh`) | rc | result |
|---|---|---|
| `--src tree-big --target fat64.img` (dry) | 1 | `C14-TREE-FITS: … needs 83888640 B … has 66058752 B free + 0 B … SHORT BY 17829888 B` |
| `--src tree-big --target fat64.img --write` | 1 | same refusal; image sha256 identical before and after (`33f8e3fa…`), so nothing was copied |
| `--src tree-ok --target fat64.img` (dry) | 0 | 11/11; `C14: tree needs 2560 B; volume has 66058752 B free + 0 B reclaimable (512 B clusters)` |
| `--src tree-ok --target fat64.img --write` | 0 | `2/2 match`, `RELEASED … (file-backed: nothing was mounted) safe to pull.` |
| same, again (re-stage) | 0 | `C14: tree needs 1536 B; … 66056192 B free + 1536 B reclaimable` (EFI/ exists now; 3 files overwritten) |
| `MEDIAWRITER_FAIL_COPY=1 … --target fat64-copyfail.img --write` | 1 | `FATAL: the copy … FAILED (rc=1) … MEDIUM IS NOT LOADED`, then `RELEASED … after a FAILED write` |

Cross-check against mtools: after the first write `mdir` reports 66,056,192 B free — 2,560 B less than C14's
pre-copy count, which is exactly the 2,560 B C14 said the tree needed.

### Go-red by mutation (`bash $S/mut/runmut.sh <repo>`; each mutant is a copy in `$S/mut`, never the tree)

| mutant | selftest rc | arms that went red |
|---|---|---|
| C14 compare replaced by `false` | 1 | `C14 RED (image left in tree)`, `C14 RED leaves target untouched` (the mutant really copied part of the tree — the orin 27 failure, reproduced), `C14 RED (new path, no room)` |
| reclaim credit removed | 1 | `C14 green (same path reclaimed)` |
| copy-failure branch without `release_medium` | 1 | `COPY RED releases the medium` |
| `To load:` hint without `--geom-from` | 1 | `HINT (re-run as printed)` |

Not exercised: the BLOCK-DEVICE half of `release_medium` (`umount` of a root mount, `udisksctl unmount`). It
needs a real or loop block device, which this executor may not create; the file-backed arms prove every failure
path reaches the function, and the block branch is the pre-existing unmount code moved into it.

## 2. scorer11 STAMP-MATCH (`docs/dev/evidence/orin22/stage11/scorer11.sh`)

Real wire `docs/dev/evidence/orin28/render15-5s-boot1-safety-scheduler.log` (`[vfs] root = boot volume …
sha=2d694c8f-d9796c4`). Card ELFs read (not written) from `~/unaos-bench/flash/orin/`: render15-5s
(`UNAOS-BUILD-STAMP-1:2d694c8f-d9796c4`) and the sibling render15-5a (`…:352d6fed-d1e204a`). The other fixtures
are DATA mutations of those two (`bash $S/runscorer.sh <repo>`):

| run | rc | STAMP-MATCH | rest of the table |
|---|---|---|---|
| wire vs its own ELF | 0 | PASS (`2d694c8f-d9796c4 == 2d694c8f-d9796c4`) | 7/7 green |
| wire vs sibling ELF | 1 | WRONG-IMAGE | the six older legs ALL PASS — the wrong-artifact score this leg exists to stop |
| wire vs ELF with the stamp magic overwritten | 1 | WRONG-IMAGE (sha on wire, no stamp in ELF) | |
| wire with ` sha=` removed vs own ELF | 1 | WRONG-IMAGE (ELF stamped, booted kernel printed none) | |
| both stripped | 3 | NOT-EXERCISED | |
| wire + a second root line `sha=deadbeef` | 1 | FAIL (2 distinct sha=, two boots) | |

## 3. orin-specscore --self-test and the two gates rows

| command | rc | result |
|---|---|---|
| `TMPDIR=$S SPECSCORE_SELFTEST_VERBOSE=1 python3 unaos/scripts/orin-specscore.py --self-test` | 0 | `SPECSCORE SELF-TEST PASS — 41/41` (23 specs explained) |
| `python3 unaos/scripts/orin-specscore.py --self-test --spec x` | 2 | `--self-test takes no other argument; got --spec` |
| `python3 unaos/scripts/mbench.py --self-test` | 0 | `SELF-TEST PASS — 48/48`, 4.0 s wall |
| (from `unaos/`) `./arroyo gates --list` | 0 | ends `mbench-selftest  unaos/scripts/mbench.py` / `specscore  unaos/scripts/orin-specscore.py` |
| (from `unaos/`) `./arroyo gates` | 1 | `GATES: 15 run 14 green -> FAIL`; `gate=mbench-selftest rc=0 … 48/48`, `gate=specscore rc=0 … 41/41`; the red is `gate=charter rc=1` on `crates/kernel/src/video/cursorshape.rs` (no CHARTER header) — not touched by this branch |
| (from `unaos/`) `TMPDIR=$S ./arroyo gates --selftest` (first plant set) | 1 | `plants=15 caught=14` — the mbench plant `RC_TRUNCATED = 1` MISSED (the self-test compares against the constant) |
| same, after the plant became "drop `-> FLICKER` from DEFAULT_FORBIDS" | 0 | `GATEPATH selftest: plants=15 caught=15 -> PASS` |
