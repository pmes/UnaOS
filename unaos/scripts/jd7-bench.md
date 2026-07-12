# JD7 bench card — panel `mkdir` / `rmdir` (attended, on the Orin)

The money shot: **create a directory** on the panel, `cd` into it, write a file there, power-cycle, `cd`
back, `cat` (the created tree survives), then clean up — `rm` the file, `cd ..`, **`rmdir` the directory**.
Proves the panel can now *shape* the tree, not just navigate/write it. This bench also flips the FATDIRS
seam's metal verdict (its `create_dir`/`remove_dir` run end-to-end here for the first time on silicon).
Detail + rationale: [`arch_arm64.md` §JD7](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused
from JD3/JD4/JD5/JD6.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **108** (virt clobber ≈ 0). Copy `EFI` + `kernel.elf` to the boot stick, `dot_clean`, eject.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. Unlike JD6, the card need NOT already contain a subdir —
  **JD7 creates its own.** It DOES need a few free clusters (each `mkdir` allocates one, plus the file).
  The pi4 fixture `UNAOSRW` card works. ⚠ FATDIRS cleanup leaves `0xE5` tombstone slots — repeated
  `mkdir`/`rmdir` runs across boots accumulate tombstones in the parent dir (harmless; NOT corruption; a
  card re-prep clears them). If the parent dir fills with tombstones after many runs, re-prep the card.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle. That graceful fallthrough is itself a pass check.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ The round-6 host capture froze mid-bench (probe re-enumeration suspected) — **confirm the bridge is
logging a full boot to `~/unaos-bench/` before spending bench time.** Screen-on-boot (JD4) brings the panel
to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. The mkdir → write → power-cycle → rmdir sequence (type on the panel; watch panel + serial)
```
ls                            # root listing (baseline)
mkdir DOCS                    # -> "created directory /DOCS"           (skip if DOCS already exists → -EEXIST)
mkdir DOCS/DRAFTS             # -> "created directory /DOCS/DRAFTS"    ← nested create, absolute-path form
ls DOCS                       # -> DRAFTS listed as <DIR> (alongside . and .. — subdir listings show them,
                              #    the DOS `dir` idiom; the root has none)
ls DOCS/DRAFTS                # -> just "." and ".." as <DIR>  ← proves create_dir wrote a well-formed
                              #    empty directory cluster that JD4's read_dir can walk
cd DOCS/DRAFTS                # descend (JD4)
pwd                           # -> /DOCS/DRAFTS
write NOTE.TXT hello from a fresh directory on the orin
                              # -> "wrote 40 bytes to /DOCS/DRAFTS/NOTE.TXT (40 bytes)"   ← canonical path
cat NOTE.TXT                  # -> "hello from a fresh directory on the orin"
```
**Now POWER-CYCLE the board** (the created tree + its file must survive — write-through durability).
Reconnect serial:
```
cd DOCS/DRAFTS                # the directory we created is still there  ← THE money shot (tree survived)
cat NOTE.TXT                  # -> the content SURVIVES the power cycle
rm NOTE.TXT                   # -> "removed /DOCS/DRAFTS/NOTE.TXT (K cluster(s) freed)"
cd ..                         # back to /DOCS
rmdir DRAFTS                  # -> "removed directory /DOCS/DRAFTS (1 cluster(s) freed)"   ← empty rmdir
ls                            # -> DRAFTS is gone
```

## 3. Honest-error probes (must NOT hang, must print the tag)
```
mkdir DOCS/DRAFTS             # after re-creating it: name exists    -> "...: file exists (-EEXIST)"
mkdir NONESUCH/X              # parent dir missing                   -> "...: not found (-ENOENT)"
write DOCS/AFILE.TXT hi       # make a plain file to probe against
mkdir DOCS/AFILE.TXT/Y        # parent is a FILE                     -> "...: not a directory (-ENOTDIR)"
rmdir DOCS/AFILE.TXT          # target is a FILE                     -> "...: not a directory (-ENOTDIR)"
rm DOCS/DRAFTS                # rm on a directory                    -> "...: is a directory (-EISDIR)"  (use rmdir)
rmdir DOCS                    # DOCS is NON-EMPTY (holds DRAFTS/AFILE.TXT) -> "...: directory not empty (-ENOTEMPTY)"
rmdir /                       # the root                             -> "/: cannot remove the root directory (-EBUSY)"
mkdir /                       # root as a create target              -> "/: is a directory (-EISDIR)"
```
Then clean up whatever you created (`rm DOCS/AFILE.TXT`, `rmdir DOCS/DRAFTS`, `rmdir DOCS`) so the card
returns to its prior shape.
- A **stalled** USB write must surface `-EIO` (the JD3 wall-clock BOT pump bounds it) — never a hang.
- JD4 read-side navigation (`ls DIR` / `cd` / `pwd` / `cat PATH`) and JD5/JD6 file writes stay unchanged.

## Pass criteria
A directory **created on the panel survives a power cycle** (you can `cd` back into it and `cat` a file
written inside it); an EMPTY directory `rmdir`s cleanly and frees its cluster; both the `cd`-relative and
absolute (`DOCS/NAME`) forms reach it; every error probe prints its honest errno tag with no wedge
(`-EEXIST`, `-ENOENT`, `-ENOTDIR`, `-ENOTEMPTY`, `-EISDIR`, `-EBUSY`). Capture serial to `~/unaos-bench/`.
