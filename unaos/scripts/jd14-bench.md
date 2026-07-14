# JD14 bench card — `-f`/force + `-n`/no-clobber flags for `cp`/`mv`/`rm` (attended)

JD14 completes the file-manager ergonomics: `cp`/`mv` default to no-clobber (an existing destination is
`-EEXIST`), `-f` opts into overwrite, and `rm -f`/`rm -rf` delete quietly (POSIX). It is `shell.rs`-only (no
`fat.rs` change) — the flags only gate which existing primitive runs. This card confirms the flag behaviour on
silicon and — thanks to JD11 — leaves a durable serial transcript. Detail + rationale:
[`arch_arm64.md` §JD14](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from JD2–JD13. Pairs
cleanly with the JD13 `rm -r` bench in one attended session.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **109** (UNCHANGED from JD11–JD13 — JD14 adds no `tegra:` token; virt clobber ≈ 0/1). Copy `EFI` +
  `kernel.elf` to the boot stick, `dot_clean`, eject. (The ELF is ~762 KB — grown from the base's SOCK-3 /
  UNAFS-K3 merges, not JD14; validate by count, not size.)
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

## 2. No-clobber is the default (the behaviour change to confirm)
```
write A.TXT alpha
write B.TXT bravo
cp A.TXT B.TXT                # -> "cp: /B.TXT: file exists (-EEXIST); use cp -f to overwrite"
cat B.TXT                     # -> "bravo"   (UNTOUCHED — the copy was refused)
mv A.TXT B.TXT                # -> "mv: /B.TXT: file exists (-EEXIST); use mv -f to overwrite"
cat B.TXT                     # -> "bravo"   (still untouched)
```
Confirm the panel now REFUSES an overwriting `cp`/`mv` by default (pre-JD14 `cp` silently overwrote).

## 3. `-f`/force — opt into overwrite
```
cp -f A.TXT B.TXT             # -> "copied /A.TXT -> /B.TXT (5 bytes)"
cat B.TXT                     # -> "alpha"   (overwritten)
write C.TXT charlie
mv -f C.TXT B.TXT             # -> "moved /C.TXT -> /B.TXT"   (delete-dst-first, then relink)
cat B.TXT                     # -> "charlie"
ls                            # C.TXT gone (moved), A.TXT still present
```

## 4. A directory destination is never clobbered (even with `-f`)
```
mkdir DIR
mv -f A.TXT DIR              # -> lands as DIR/A.TXT (the SRC DIR/ idiom into an empty slot) — OK
write DIR/A.TXT one
write SOLO.TXT two
mv -f SOLO.TXT DIR/A.TXT     # DIR/A.TXT exists as a FILE -> overwrites it (force, file dest) -> "moved ..."
mkdir SUB
mv -f SUB DIR/A.TXT         # target DIR/A.TXT is a FILE, source is a dir -> honest error, not clobbered
# (cp -r onto an existing tree also stays -EEXIST even with -f — merge/replace is out of scope)
mkdir T1
mkdir T2
cp -rf T1 T2                 # T2 exists (dir) -> "cp: /T2/T1: ..." fresh-tree rule; use rm -r to replace
```
Confirm `-f` overwrites only a FILE destination; a directory target is refused honestly (no subtree is ever
silently destroyed by a single `cp`/`mv`).

## 5. `rm -f` / `rm -rf` — quiet on a missing target, bundled flags parse
```
rm -f NOSUCH.TXT             # (no output — POSIX rm -f is quiet on a missing target)
rm -rf NOSUCH*              # (no output — a no-match wildcard is quiet under -f)
mkdir GONE
write GONE/G.TXT g
rm -rf GONE                  # -> "removed /GONE/ (1 dir(s), 1 file(s))"  (bundled -rf parses; pre-JD14 bug)
ls                           # GONE gone
rm GONE                      # -> "rm: /GONE: not found (-ENOENT)"   (WITHOUT -f the error IS shown)
```
Confirm `rm -rf DIR` recurses (the bundled-flag fix), `-f` silences a missing target, and a plain `rm` still
reports `-ENOENT` honestly.

## 6. The two guards `-f` does NOT relax
```
mkdir KEEP
rm -f KEEP                    # -> "rm: /KEEP: is a directory (-EISDIR)"   (wrong-usage, shown even with -f)
rm -rf /                     # -> "rm: -r /: cannot remove the root directory (-EBUSY)"   (footgun, refused)
rmdir KEEP
```

## 7. `-n` reasserts / overrides `-f`
```
write P.TXT p
write Q.TXT q
cp -f -n P.TXT Q.TXT         # -n overrides -f -> "cp: /Q.TXT: file exists (-EEXIST) ..."
cat Q.TXT                     # -> "q"   (not overwritten — no-clobber won)
```

## 8. Durability across a power cycle
```
write R.TXT before
write S.TXT after
cp -f S.TXT R.TXT            # -> "copied ... " (R.TXT now holds "after")
```
Then POWER-CYCLE and confirm:
```
cat R.TXT                     # -> "after"   (the forced overwrite is durable / write-through)
```

## Pass criteria
`cp`/`mv` onto an existing FILE are refused `-EEXIST` by default; `-f` overwrites (cp truncate-in-place, mv
delete-dst-first); a DIRECTORY destination is never clobbered by `-f` (refused honestly), and `cp -r` onto an
existing tree stays `-EEXIST`. `rm -f`/`rm -rf` are quiet on a missing target and a no-match wildcard, `rm -rf
DIR` recurses (bundled flags parse), while a plain `rm NOSUCH` still reports `-ENOENT`. `rm -rf /` is still
`-EBUSY` and a `rm -f DIR` (no `-r`) still `-EISDIR`. `-n` overrides `-f`. After a power cycle a forced
overwrite is durable. No wedge, no wrong entry touched, no dropped lines. The serial transcript
(`awk '/:: tegra: JD2 —/' ~/unaos-bench/*.log`) is the durable evidence — a clean interleave of the typed keys
and each verb's output. This bench needs no pre-seeded layout: the panel builds every file above itself.
