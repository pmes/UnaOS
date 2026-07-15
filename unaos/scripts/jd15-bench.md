# JD15 bench card — `-f` tree-replace for `cp -r`/`mv` (attended)

JD15 closes the last flag-family gap: `cp -rf` and `mv -f` now REPLACE an existing directory-TREE
destination (JD14 bounded `-f` to a single FILE dest — a tree stayed `-EEXIST`/`-EISDIR`). It is
`shell.rs`-only (no `fat.rs` change) — a new `force_remove_existing` helper deletes the destination first
(delete-dst-first) via the JD13 `rm_tree` + `remove_dir` primitives, then the fresh copy/move proceeds. This
card confirms the tree-replace on silicon, checks the crash-safe-PARTIAL property across a real power cut, and
— thanks to JD11 — leaves a durable serial transcript. Detail + rationale:
[`arch_arm64.md` §JD15](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from JD2–JD14. Pairs
cleanly with the JD13/JD14 benches in one attended session.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **109** (UNCHANGED from JD11–JD14 — JD15 adds no `tegra:` token; virt clobber ≈ 0/1). Copy `EFI` +
  `kernel.elf` to the boot stick, `dot_clean`, eject. (The ELF is ~762 KB+ — grown from the base's SOCK-3 /
  UNAFS-K3 merges, not JD15; validate by count, not size.)
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. The pi4 fixture `UNAOSRW` card is fine; the panel itself
  creates every file below. ⚠ If the card was Mac-touched, `dot_clean` it too and strip `._*` (AppleDouble
  sidecars are glob-visible on FAT 8.3 short names — a `rm -rf *` would match them).
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ With JD11 the serial bridge is the **primary output-evidence channel** — confirm it is logging a full boot
to `~/unaos-bench/` BEFORE spending bench time (§JB1f: the round-6/8 host capture froze mid-bench). Screen-on-
boot (JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. `cp -rf` replaces an existing directory tree
```
mkdir SRC
write SRC/A.TXT alpha
write SRC/B.TXT bravo
cp -r SRC DST                 # -> "copied /SRC/ -> /DST/ (1 dir(s), 2 file(s), 10 bytes)"  (fresh tree)
cp -r SRC DST                 # DST/SRC path: DST is a dir -> copy-INTO idiom -> "copied /SRC/ -> /DST/SRC/ ..."
cp -r SRC DST                 # NOW /DST/SRC pre-exists -> "cp: /DST/SRC: already exists (-EEXIST); use
                              #  cp -rf to replace it, or rm -r it first"   (no-clobber default, unbypassed)
cp -rf SRC DST                # -f -> tree-replace: deletes /DST/SRC first, then rebuilds -> "copied ..."
cat DST/SRC/A.TXT             # -> "alpha"   (the replaced tree is present + correct)
```
Confirm the DEFAULT still refuses (`-EEXIST`), and `-rf` deletes-then-rebuilds the existing tree destination.

## 3. `cp -rf` also replaces a stale/differing tree (content actually changes)
```
mkdir OLD
write OLD/K.TXT old-keep
mkdir NEW
write NEW/K.TXT new-content
write NEW/EXTRA.TXT extra
cp -rf NEW OLD/NEW           # OLD/NEW does not exist yet -> plain fresh copy (nest under OLD) — OK, "copied ..."
cp -rf NEW OLD/NEW           # ⚠ ERRATA (2026-07-15 bench): OLD/NEW is an existing DIR -> the copy-INTO
                             #  idiom takes precedence (consistent with §2 and POSIX) -> nests /OLD/NEW/NEW/.
cp -rf NEW OLD               # THE true differing-tree replace: target = OLD/NEW (exists) -> delete-then-
                             #  rebuild -> "copied /NEW/ -> /OLD/NEW/ ..." (prior nested content GONE)
cat OLD/NEW/EXTRA.TXT        # -> "extra"   (the fresh tree, not a merge of a prior one)
```

## 4. `mv -f` replaces an existing directory-tree destination
```
mkdir MSRC
write MSRC/M.TXT moved-in
mkdir MDST
mkdir MDST/MSRC              # pre-seed a directory named like MSRC's leaf, INSIDE MDST
write MDST/MSRC/STALE.TXT stale
mv MSRC MDST                 # SRC DIR/ idiom: target = MDST/MSRC which EXISTS -> "-EISDIR"/"-EINVAL" family?
                             #  NOTE a DIRECTORY source crossing parents is refused first (-EISDIR) — see §4b.
```
### 4b. mv tree-replace applies to a FILE source landing over an existing subdir (dir-src-across-parents stays refused)
```
write LEAF.TXT leaf
mkdir MDST/LEAF.TXT          # an (odd but legal) directory whose name collides with the file's leaf
mv -f LEAF.TXT MDST          # target = MDST/LEAF.TXT which is a DIR -> -f tree-replace: rm_tree it, then
                             #  relink the file -> "moved /LEAF.TXT -> /MDST/LEAF.TXT"
cat MDST/LEAF.TXT            # -> "leaf"   (now a FILE, the stale subdir was replaced)
```
Confirm `mv -f` tree-deletes an existing directory destination before relinking. (A DIRECTORY source moved
across parents remains `-EISDIR` — that guard fires BEFORE any delete-dst-first, so a doomed move never
destroys its destination: try `mv -f MSRC MDST2` after `mkdir MDST2` — refused, MDST2 untouched.)

## 5. The guards `-f` tree-replace does NOT relax
```
mkdir SELF
write SELF/X.TXT x
cp -rf SELF SELF/SUB         # copying a dir into its own subtree -> "-EINVAL" (self/subtree guard stands)
cp -rf SELF /                # target = /SELF == source -> "cannot copy directory into itself" (-EINVAL)
rm -rf /                     # -> "rm: -r /: cannot remove the root directory (-EBUSY)"  (footgun, refused)
```

## 6. Durability + crash-safe-PARTIAL across a power cycle
```
mkdir PRE
write PRE/D.TXT before
mkdir POST
write POST/D.TXT after
write POST/NEW.TXT fresh
cp -rf POST PRE/POST        # PRE/POST absent -> fresh copy first pass; run twice so the 2nd is a real replace
cp -rf POST PRE/POST        # -> tree-replace (delete /PRE/POST, rebuild) -> "copied ..."
```
Then POWER-CYCLE and confirm the completed replace is durable:
```
cat PRE/POST/NEW.TXT         # -> "fresh"   (the replaced tree is durable / write-through)
```
⚠ Crash-safe-PARTIAL note (informational — do NOT try to force a mid-op power cut): `-f` tree-replace deletes
the destination BEFORE rebuilding, so a power cut in the window leaves the destination ABSENT (never
half-merged). If a `cat` after a boot shows the destination missing, that is the honest partial state — re-run
`cp -rf`/`mv -f` to complete it, NOT a corruption.

## Pass criteria
`cp -r`/`mv` onto an existing directory-tree destination are refused by default (`-EEXIST` / copy-into idiom),
and `-rf`/`-f` REPLACE it — the destination tree is deleted first, then the fresh copy/move lands (confirmed
by reading the new tree's content, including files the prior tree lacked). The JD9 self/subtree `-EINVAL`, the
`mv` directory-across-parents `-EISDIR` (fired before any delete-dst-first — a doomed move leaves its
destination intact), and the `rm -rf /` `-EBUSY` root footgun all still refuse under `-f`. After a power cycle
a completed tree-replace is durable; a mid-op cut leaves the destination absent, not merged. No wedge, no wrong
entry touched, no dropped lines. The serial transcript (`awk '/:: tegra: JD2 —/' ~/unaos-bench/*.log`) is the
durable evidence — a clean interleave of the typed keys and each verb's output. This bench needs no pre-seeded
layout: the panel builds every tree above itself.
