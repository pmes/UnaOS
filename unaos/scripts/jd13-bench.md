# JD13 bench card — recursive `rm -r <dir>` on the Orin panel (attended)

JD13 closes the destructive side of the file-manager verb set: `rm -r <dir>` deletes a whole subtree (files
then directories, depth-first), and it composes with the JD12 glob (`rm -r OLD*/`). It is `shell.rs`-only (no
`fat.rs` change) — it reuses `read_dir`, the `fs_rm` delete pair, and the `rmdir` primitive. This card confirms
it on silicon and — thanks to JD11 — leaves a durable serial transcript proving exactly which entries were
removed. Detail + rationale: [`arch_arm64.md` §JD13](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics
reused from JD2–JD12.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **109** (UNCHANGED from JD11/JD12 — JD13 adds no `tegra:` token; virt clobber ≈ 0/1). Copy `EFI` +
  `kernel.elf` to the boot stick, `dot_clean`, eject. (The ELF is ~727 KB — grown from the base's SOCK-3 /
  UNAFS-K3 merges, not JD13; validate by count, not size.)
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. The pi4 fixture `UNAOSRW` card is fine; the panel itself
  creates the bench tree below (no pre-seeded layout needed). ⚠ If the card was Mac-touched, `dot_clean` it too
  and strip `._*` (AppleDouble sidecars are glob-visible on FAT 8.3 short names — a `rm -r *` would match them).
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ With JD11 the serial bridge is the **primary output-evidence channel** — confirm it is logging a full boot
to `~/unaos-bench/` BEFORE spending bench time (§JB1f: the round-6/8 host capture froze mid-bench). Screen-on-
boot (JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. Build a small nested tree (on the panel)
```
mkdir DOCS
mkdir DOCS/SUB
write DOCS/A.TXT alpha
write DOCS/B.TXT bravo
write DOCS/SUB/C.TXT charlie
ls DOCS                       # -> A.TXT, B.TXT, SUB
ls DOCS/SUB                   # -> ., .., C.TXT
```
Every line echoes to serial as `:: tegra: JD2 — OUT | … ::` (JD11), so the whole session is replayable.

## 3. The money shot — recursive delete
```
rm DOCS                       # -> "rm: /DOCS: is a directory (-EISDIR)"   (no -r: refused, file-only default)
rm -r DOCS                    # -> "removed /DOCS/ (2 dir(s), 3 file(s))"  (top DOCS + SUB = 2 dirs; A/B/C = 3)
ls                            # root: DOCS gone entirely
ls DOCS                       # -> "ls: /DOCS: not found (-ENOENT)"        (the whole subtree is gone)
```
Confirm the single summary line (not a per-file flood), the correct dir/file counts, and that the subtree is
fully gone — no orphaned SUB, no leftover files.

## 4. Guards / error probes
```
rm -r /                       # -> "rm: -r /: cannot remove the root directory (-EBUSY)"
rm -r NOSUCH                  # -> "rm: /NOSUCH: not found (-ENOENT)"
write SOLO.TXT solo
rm -r SOLO.TXT                # -> "removed /SOLO.TXT (…)"  (a FILE target degrades to a plain rm)
ls                            # SOLO.TXT gone
```

## 5. Glob form — remove several trees at once (composes with JD12)
```
mkdir OLD1
mkdir OLD2
write OLD1/X.TXT x
write OLD2/Y.TXT y
rm -r OLD*                    # removes OLD1 + OLD2 (each a "removed /OLDn/ (…)" summary), sorted
ls                            # both OLD trees gone
rm -r NONE*                   # -> "rm: NONE*: no match"   (honest, no wedge)
```

## 6. Durability across a power cycle (freed clusters genuinely reused)
```
mkdir TREE
write TREE/D.TXT durable
rm -r TREE                    # -> "removed /TREE/ (1 dir(s), 1 file(s))"
```
Then POWER-CYCLE and confirm:
```
ls                            # TREE is still gone (the delete was write-through / durable)
mkdir TREE                    # re-create the same name — succeeds (the freed clusters were reclaimable)
write TREE/D.TXT again
cat TREE/D.TXT                # -> "again"   (a fresh chain in the reused space, not stale bytes)
```

## Pass criteria
`rm DIR` without `-r` is refused `-EISDIR` (the default stays file-only); `rm -r DIR` deletes the whole subtree
in one command and prints ONE summary with the correct dir/file counts (the top dir counted); the subtree is
fully gone (`ls` on it → `-ENOENT`, no orphaned children). The guards fire honestly: `rm -r /` → `-EBUSY`,
`rm -r NOSUCH` → `-ENOENT`, `rm -r FILE` → a plain delete. `rm -r OLD*` expands and removes each matching tree
in sorted order; a no-match pattern reports `no match`. After a power cycle the deletion is durable and the same
name re-creates cleanly (freed clusters reused, no stale data). No wedge, no wrong entry touched, no dropped
lines. The serial transcript (`awk '/:: tegra: JD2 —/' ~/unaos-bench/*.log`) is the durable evidence — a clean
interleave of the typed keys and each verb's output. This bench needs no pre-seeded layout: the panel builds
every tree above itself.
