# MUTATIONS.md — the can-fire proof for `scorer11.sh`

Produced by `bash mutate11.sh` (2026-09-08, orin 22 / STAGE11). Every row is a MUTATION OF DATA —
a copy of the render9 wire (`~/unaos-bench/scratch/orin19/pointerlag/boot-render9.log`, NUL-stripped,
last boot = lines 19900+) edited into the render11 contract's shape, and/or one of four artifact
fixtures built from real kernel builds. **The scorer is never edited between rows.** Fixtures in
`fix/`, per-row scorer output in `out/M<NN>.out`, machine-readable table in `MUTATIONS.tsv`.

Base fixture `W0.log` = that boot with the retired `[sdmmc] root` lines DELETED and the contract's
bind line `[vfs] root = boot volume serial=0xde001a13 source=usb ::` APPENDED — `0xde001a13` is the
serial the loader actually printed on that wire, so the base is a true PASS and not a rigged one.

Artifact fixtures: `elf-armed.bin` = `unaos/target/aarch64_esp/kernel.elf` + the arming literal
appended · `elf-unarmed.bin` = that kernel.elf untouched (control 32, arming 0) ·
`elf-oldfamily.bin` = armed + the retired `[sdmmc] root` literal · `elf-nocontrol.bin` = a 61-byte
text file carrying the arming literal and no control · `elf-missing.bin` = a path that does not exist.

| id | mutation | exit | ARMING | LOADER-SERIAL | ROOT-BIND | ROOT-NONE | OLD-BIND-ABSENT | SOURCE-VOCAB |
|---|---|---|---|---|---|---|---|---|
| M00 | BASE (contract-shaped PASS wire, armed artifact) | **0** | PASS | PASS | PASS | PASS | PASS | PASS |
| M01 | loader serial line DELETED (loader still speaks) | **1** | PASS | ABSENT | NOT-SCORED | PASS | PASS | PASS |
| M02 | ALL loader lines DELETED (loader control zero) | **1** | PASS | NOT-SCORED | NOT-SCORED | PASS | PASS | PASS |
| M03 | bind serial ALTERED to 0x0badcafe (mismatch) | **1** | PASS | PASS | FAIL | PASS | PASS | PASS |
| M04 | bind line DELETED, no NONE line | **1** | PASS | PASS | ABSENT | PASS | PASS | NOT-EXERCISED |
| M05 | NONE inserted, reason=loader-named-no-volume | **1** | PASS | PASS | NOTE | FAIL | PASS | NOT-EXERCISED |
| M06 | NONE inserted, reason=boot-volume-not-among-disks | **1** | PASS | PASS | NOTE | FAIL | PASS | NOT-EXERCISED |
| M07 | retired [sdmmc] root line INSERTED on the wire | **1** | PASS | PASS | PASS | PASS | WRONG-IMAGE | PASS |
| M08 | second loader serial 0xfeed0001 INSERTED (two boots) | **1** | PASS | FAIL | FAIL | PASS | PASS | PASS |
| M09 | bind source ALTERED to source=potato | **0** | PASS | PASS | PASS | PASS | PASS | NOTE |
| M10 | bind line with NO serial= field | **1** | PASS | PASS | FAIL | PASS | PASS | PASS |
| M11 | loader-only wire (kernel control zero) | **1** | PASS | PASS | NOT-SCORED | NOT-SCORED | NOT-SCORED | NOT-EXERCISED |
| M12 | NONE with an UNDOCUMENTED reason | **1** | PASS | PASS | NOTE | FAIL | PASS | NOT-EXERCISED |
| M13 | bind line with NO source= field | **0** | PASS | PASS | PASS | PASS | PASS | NOTE |
| M14 | second bind line, DIFFERENT serial | **1** | PASS | PASS | FAIL | PASS | PASS | NOTE |
| M15 | EMPTY wire, armed artifact | **1** | PASS | NOT-SCORED | NOT-SCORED | NOT-SCORED | NOT-SCORED | NOT-EXERCISED |
| M16 | UNARMED artifact + wire with no [vfs] lines | **3** | NOT-EXERCISED | PASS | NOT-EXERCISED | NOT-EXERCISED | PASS | NOT-EXERCISED |
| M17 | UNARMED artifact + wire that HAS [vfs] lines | **1** | WRONG-IMAGE | PASS | NOT-EXERCISED | NOT-EXERCISED | PASS | NOT-EXERCISED |
| M18 | artifact with arming literal but NO positive control | **2** | - | - | - | - | - | - |
| M19 | artifact path does not exist | **2** | - | - | - | - | - | - |
| M20 | clean wire, artifact still carries [sdmmc] root | **1** | PASS | PASS | PASS | PASS | WRONG-IMAGE | PASS |

## Outcome coverage — every leg produced more than one verdict, and every exit code was reached

| leg | outcomes produced | rows |
|---|---|---|
| ARMING | PASS · NOT-EXERCISED · WRONG-IMAGE · (refuse, exit 2) | M00 · M16 · M17 · M18, M19 |
| LOADER-SERIAL | PASS · ABSENT · NOT-SCORED · FAIL | M00 · M01 · M02, M15 · M08 |
| ROOT-BIND | PASS · FAIL · ABSENT · NOTE · NOT-SCORED · NOT-EXERCISED | M00 · M03, M08, M10, M14 · M04 · M05, M06, M12 · M01, M02, M11, M15 · M16, M17 |
| ROOT-NONE | PASS · FAIL · NOT-SCORED · NOT-EXERCISED | M00 · M05, M06, M12 · M11, M15 · M16, M17 |
| OLD-BIND-ABSENT | PASS · WRONG-IMAGE (wire) · WRONG-IMAGE (artifact) · NOT-SCORED | M00 · M07 · M20 · M11, M15 |
| SOURCE-VOCAB | PASS · NOTE (unknown token) · NOTE (field absent) · NOTE (two tokens) · NOT-EXERCISED | M00 · M09 · M13 · M14 · M04 |
| exit code | 0 · 1 · 2 · 3 | M00, M09, M13 · 15 rows · M18, M19 · M16 |

## The three rows that matter most

* **M03** — the same wire as the PASS base with ONE hex digit group changed. ROOT-BIND FAILs and
  names both operands. This is the leg's whole reason to exist: a kernel that roots on a volume it
  was not loaded from.
* **M16 vs M17** — the same UNARMED artifact against two wires. With no `[vfs] root` lines the family
  is NOT-EXERCISED and the run exits 3 with ZERO reds (the emitter is not in the image; that is not a
  failure). With `[vfs] root` lines on the wire it is WRONG-IMAGE: the kernel.elf handed to the scorer
  is not the kernel that booted. scorer10 had no arm for either case.
* **M20** — a perfectly clean wire and a PASSing bind, but the flashed artifact still carries the
  retired `[sdmmc] root` literal. Red. A boot that dies above the mount table cannot hide a
  pre-BOOTROOT image from this leg, because the artifact half does not depend on the wire.

## Known, deliberate: SOURCE-VOCAB never reds (M09 exits 0)

An unrecognised `source=` token is a signal that the scorer is behind the tree, not that the flight
failed — root still bound to the boot volume. It is a NOTE, and the run exits 0. If a future round
wants the vocabulary to be binding, promote the NOTE to FAIL in that one leg; the fixture (`W9.log`)
already exists to prove both polarities.
