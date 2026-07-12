# JD10 bench card — panel `mv` (attended, on the Orin)

The money shot: **move/rename a file on the panel, then power-cycle and read it back intact**. A move is
O(1) by reference (only the directory entry is relinked — the data never moves), so this also proves the
pi4-lane **FATMOVE** `move_entry` crash-ordering (destination published before the source is `0xE5`'d) on
real silicon for the first time. JD10 closes the classic file-manager verb set: navigate (JD4), write
(JD5/JD6), shape (JD7), copy (JD8/JD9), **move/rename (JD10)**. JD10 is pure `shell.rs` glue — `mv` composes
the FATMOVE `rename_entry`/`move_entry` seam + the JD6 path idioms + the JD9 `is_descendant` guard,
**no `fat.rs` change** (call-never-edit). Detail + rationale:
[`arch_arm64.md` §JD10](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from JD3–JD9.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **108** (virt clobber ≈ 0). Copy `EFI` + `kernel.elf` to the boot stick, `dot_clean`, eject.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. A move needs no free clusters for data (it relinks the
  entry), but the destination directory needs a free slot; the pi4 fixture `UNAOSRW` card works. The source
  file/tree is built on the card with `write`/`mkdir` below, so the card need not already contain one.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle. That graceful fallthrough is itself a pass check.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ The round-6/8 host capture froze mid-bench (probe re-enumeration suspected) — **confirm the bridge is
logging a full boot to `~/unaos-bench/` before spending bench time.** Screen-on-boot (JD4) brings the
panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. Build a small source set (type on the panel)
```
write A.TXT hello alpha       # a file in the root -> "/A.TXT"
mkdir DOCS                    # -> "created directory /DOCS"
mkdir DOCS/SUB                # -> "created directory /DOCS/SUB"
ls                            # -> A.TXT, DOCS
```

## 3. Rename in place → move into a dir → power-cycle → read back (THE money shot)
```
mv A.TXT B.TXT                # rename in the SAME dir (rename_entry, O(1))
                             #   -> "renamed /A.TXT -> /B.TXT"
cat B.TXT                     # -> "hello alpha"   (same data, new name)
ls                            # -> B.TXT, DOCS     (A.TXT is gone)
mv B.TXT DOCS/                # move into a directory (move_entry, by reference) -> lands as /DOCS/B.TXT
                             #   -> "moved /B.TXT -> /DOCS/B.TXT"
cat DOCS/B.TXT                # -> "hello alpha"   (moved, data intact)
ls                            # -> DOCS            (/B.TXT is gone from the root)
```
**Now POWER-CYCLE the board** (the moved file must survive — write-through durability + FATMOVE crash
ordering). Reconnect serial:
```
cat DOCS/B.TXT                # -> "hello alpha"   ← the moved file SURVIVES  ← THE money shot
ls DOCS                       # -> B.TXT, SUB (plus . and ..)   (the destination is intact)
```

## 4. Directory rename (O(1) subtree move) → verify children follow
```
mv DOCS NOTES                 # rename a DIRECTORY in place (rename_entry allows dirs) -> "renamed /DOCS -> /NOTES"
ls NOTES                      # -> B.TXT, SUB   ← the whole subtree moved with one entry relink (no copy)
cat NOTES/B.TXT               # -> "hello alpha"   (the child file came along)
```

## 5. Guard + honest-error probes (must NOT hang, must print the tag)
```
mv NOTES NOTES/SUB            # a dir into its own DESCENDANT -> "mv: cannot move directory /NOTES into
                             #                                   itself or its own subtree (/NOTES/SUB/NOTES) (-EINVAL)"
mkdir ARCHIVE                 # an existing destination directory
mv NOTES ARCHIVE/             # move a DIRECTORY across parents -> "mv: /NOTES: cannot move a directory
                             #                     across directories (-EISDIR); rename it in place or use cp -r + rm -r"
mv NOSUCH X                   # source missing              -> "mv: ...NOSUCH: not found (-ENOENT)"
mv NOTES                      # missing dst                 -> "usage: mv <src> <dst>"
mv / X                        # move the volume root        -> "mv: /: cannot move the volume root (-EBUSY)"
write C.TXT gamma             # a distinct existing file at the destination name
mv NOTES/B.TXT C.TXT          # dst file already exists (no-clobber) -> "mv: /C.TXT: file exists (-EEXIST)"
```
Then clean up whatever you created so the card returns to its prior shape (depth-first — `rmdir` only
removes empty directories):
```
rm C.TXT
rm NOTES/B.TXT ; rmdir NOTES/SUB ; rmdir NOTES ; rmdir ARCHIVE
```
- A **stalled** USB read/write must surface `-EIO` (the JD3 wall-clock BOT pump bounds it) — never a hang.
- JD4 read-side navigation and JD5–JD9 write/shape/copy commands stay unchanged.

## Pass criteria
A **file moved/renamed on the panel survives a power cycle** (you can `cat` the moved file, at its new
name/location, after re-boot), an in-place **directory rename** carries the whole subtree with one relink
(O(1), no copy — `ls NOTES` shows the former `DOCS` children), both `mv A B` (rename) and `mv A DIR/`
(move-into-dir) idioms work and echo `renamed`/`moved`, and every guard/error probe prints its honest tag
with no wedge (`-EINVAL` dir self/descendant, `-EISDIR` cross-parent dir move, `-ENOENT` missing source,
`-EEXIST` no-clobber, `-EBUSY` root). This bench also flips the FATMOVE seam's own metal verdict — its
`move_entry` crash-ordering runs on silicon here for the first time. Capture serial to `~/unaos-bench/`.
