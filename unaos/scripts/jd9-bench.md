# JD9 bench card — panel `cp -r` (attended, on the Orin)

The money shot: **recursively copy a directory tree** on the panel into a fresh destination, `ls`/`cat`
the copy to prove the tree structure and file contents match, power-cycle, verify the copied tree
survives, then run the guard/error probes. Proves the panel can now *duplicate whole trees*, completing
the copy half of the file-manager verb set (`mv` is the only verb left, gated on a future FATMOVE seam).
JD9 is pure `shell.rs` glue — `cp -r` composes `read_dir` (JD4) + the FATDIRS `create_dir` seam (JD7
idiom) + the JD8 per-file streaming copy, **no `fat.rs` change**. Detail + rationale:
[`arch_arm64.md` §JD9](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from
JD3/JD4/JD5/JD6/JD7/JD8.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **108** (virt clobber ≈ 0). Copy `EFI` + `kernel.elf` to the boot stick, `dot_clean`, eject.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. It needs a few free clusters (the recursive copy allocates
  a destination cluster per directory + the file chains). The source tree is built on the card with
  `mkdir`/`write` below, so the card need not already contain one; the pi4 fixture `UNAOSRW` card works.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle. That graceful fallthrough is itself a pass check.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ The round-6/8 host capture froze mid-bench (probe re-enumeration suspected) — **confirm the bridge is
logging a full boot to `~/unaos-bench/` before spending bench time.** Screen-on-boot (JD4) brings the
panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. Build a small source tree (type on the panel)
```
mkdir SRC                     # -> "created directory /SRC"
write SRC/A.TXT hello alpha   # a file at the top of the tree
mkdir SRC/SUB                 # -> "created directory /SRC/SUB"
write SRC/SUB/B.TXT deep beta # a file one level down
ls SRC                        # -> A.TXT, SUB (plus . and ..)
ls SRC/SUB                    # -> B.TXT (plus . and ..)
```

## 3. The recursive copy → verify → power-cycle → verify sequence
```
cp -r SRC DST                 # DST does not exist -> DST becomes the tree
                              #   -> "copied /SRC/ -> /DST/ (2 dir(s), 2 file(s), N bytes)"
ls DST                        # -> A.TXT, SUB (structure matches SRC)
ls DST/SUB                    # -> B.TXT
cat DST/A.TXT                 # -> "hello alpha"   (matches SRC/A.TXT)
cat DST/SUB/B.TXT             # -> "deep beta"     (matches SRC/SUB/B.TXT)
cat SRC/A.TXT                 # -> the SOURCE tree is UNCHANGED
mkdir BACKUP                  # an existing destination directory
cp -r SRC BACKUP              # into an existing dir -> lands AS /BACKUP/SRC
                              #   -> "copied /SRC/ -> /BACKUP/SRC/ (2 dir(s), 2 file(s), N bytes)"
cat BACKUP/SRC/SUB/B.TXT      # -> "deep beta"     (the into-dir idiom worked)
```
**Now POWER-CYCLE the board** (the copied tree must survive — write-through durability). Reconnect serial:
```
ls DST                        # -> A.TXT, SUB   ← the copied tree SURVIVES  ← THE money shot
cat DST/SUB/B.TXT             # -> "deep beta"  (deep file survived the power cycle)
cat BACKUP/SRC/A.TXT          # -> "hello alpha" (the second copy survived too)
```

## 4. Guard + honest-error probes (must NOT hang, must print the tag)
```
cp -r SRC SRC/SUB             # into a DESCENDANT of itself -> "cp: cannot copy directory /SRC into itself
                              #                                 or its own subtree (/SRC/SUB/SRC) (-EINVAL)"
cp -r SRC SRC                 # into itself (SRC exists → /SRC/SRC, a descendant) -> (-EINVAL)
cp -r SRC BACKUP              # /BACKUP/SRC already exists (from step 3) -> "cp: /BACKUP/SRC: already exists (-EEXIST)"
cp -r NOSUCH DST2             # source missing              -> "cp: ...NOSUCH: not found (-ENOENT)"
cp -r /                       # missing dst                 -> "usage: cp [-r] <src> <dst>"
cp -r / DST3                  # copy the volume root        -> "cp: -r /: cannot copy the volume root (-EINVAL)"
cp -r SRC/A.TXT FILECOPY.TXT  # a FILE source with -r degrades to a plain file copy
                              #   -> "copied /SRC/A.TXT -> /FILECOPY.TXT (N bytes)"
cp -r SRC DST/A.TXT           # dst is a plain FILE          -> "cp: /DST/A.TXT: not a directory (-ENOTDIR)"
```
Then clean up whatever you created so the card returns to its prior shape (depth-first — `rmdir` only
removes empty directories):
```
rm FILECOPY.TXT
rm DST/A.TXT ; rm DST/SUB/B.TXT ; rmdir DST/SUB ; rmdir DST
rm BACKUP/SRC/A.TXT ; rm BACKUP/SRC/SUB/B.TXT ; rmdir BACKUP/SRC/SUB ; rmdir BACKUP/SRC ; rmdir BACKUP
rm SRC/A.TXT ; rm SRC/SUB/B.TXT ; rmdir SRC/SUB ; rmdir SRC
```
- A **stalled** USB read/write must surface `-EIO` (the JD3 wall-clock BOT pump bounds it) — never a hang.
- JD4 read-side navigation and JD5/JD6/JD7/JD8 write/shape/copy commands stay unchanged.

## Pass criteria
A **directory tree copied recursively on the panel survives a power cycle** (you can `ls`/`cat` the copy,
subdirectories and all, after re-boot), the **source tree is left byte-for-byte untouched**, both the
`cp -r DIR NEWDEST` (create) and `cp -r DIR EXISTINGDIR` (into-dir → `EXISTINGDIR/DIR`) idioms work, the
success line reports the honest `(dir(s), file(s), bytes)` tally, and every guard/error probe prints its
honest tag with no wedge (`-EINVAL` self/descendant + root, `-EEXIST` pre-existing target, `-ENOENT`
missing source, `-ENOTDIR` file destination). Capture serial to `~/unaos-bench/`.
