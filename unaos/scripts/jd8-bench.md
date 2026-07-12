# JD8 bench card — panel `cp` (attended, on the Orin)

The money shot: **copy a file** on the panel into a subdirectory, `cat` the copy, power-cycle, `cat` it
again (the copy survives — write-through durability) with the **source left untouched**, then run the
honest-error probes. Proves the panel can now *duplicate* files, closing the file-manager verb set
(create / edit / delete / organize / **copy**). JD8 is pure `shell.rs` glue — `cp` composes the existing
`read_at` read primitive with the JD6 create-or-truncate write path, no `fat.rs` change. Detail +
rationale: [`arch_arm64.md` §JD8](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from
JD3/JD4/JD5/JD6/JD7.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **108** (virt clobber ≈ 0). Copy `EFI` + `kernel.elf` to the boot stick, `dot_clean`, eject.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. It needs at least one small 8.3-named text file at the
  root to copy (e.g. `README.TXT`) and a few free clusters (the copy allocates the destination chain).
  A subdir target is created below with `mkdir` (JD7), so the card need not already contain one; the pi4
  fixture `UNAOSRW` card works (it has `README.TXT` + `DOCS/`).
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle. That graceful fallthrough is itself a pass check.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ The round-6/8 host capture froze mid-bench (probe re-enumeration suspected) — **confirm the bridge is
logging a full boot to `~/unaos-bench/` before spending bench time.** Screen-on-boot (JD4) brings the
panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. The copy → cat → power-cycle → cat sequence (type on the panel; watch panel + serial)
```
ls                            # root listing (baseline — note README.TXT and DOCS/)
cat README.TXT                # the SOURCE content (remember it)
mkdir COPIES                  # a fresh target dir  -> "created directory /COPIES"  (skip if it exists → -EEXIST)
cp README.TXT COPIES/         # the `cp FILE DIR/` idiom -> "copied /README.TXT -> /COPIES/README.TXT (N bytes)"
ls COPIES                     # -> README.TXT listed (alongside . and ..) with the same size as the source
cat COPIES/README.TXT         # -> IDENTICAL to `cat README.TXT` above
cat README.TXT                # -> the SOURCE is UNCHANGED (cp does not disturb the original)
cp README.TXT COPIES/BACKUP.TXT   # explicit destination name (not the DIR/ idiom)
cat COPIES/BACKUP.TXT         # -> same content, different name
```
**Now POWER-CYCLE the board** (the copies must survive — write-through durability). Reconnect serial:
```
cat COPIES/README.TXT         # -> the copy SURVIVES the power cycle  ← THE money shot
cat COPIES/BACKUP.TXT         # -> the named copy survives too
cat README.TXT                # -> the source still intact
```

## 3. Honest-error probes (must NOT hang, must print the tag)
```
cp NOSUCH.TXT COPIES/         # source missing            -> "cp: ...NOSUCH.TXT: not found (-ENOENT)"
cp DOCS COPIES/               # source is a DIRECTORY       -> "cp: /DOCS: is a directory (-EISDIR)"  (no cp -r yet)
cp README.TXT README.TXT      # copy onto itself           -> "cp: /README.TXT and /README.TXT are the same file (-EINVAL)"
cp README.TXT NONE/X.TXT      # dst PARENT missing          -> "cp: ...: not found (-ENOENT)"
cp README.TXT README.TXT/X    # dst parent is a FILE        -> "cp: ...: not a directory (-ENOTDIR)"
cp                            # missing args                -> "usage: cp <src> <dst>"
cp README.TXT                 # missing dst                 -> "usage: cp <src> <dst>"
```
Then clean up whatever you created so the card returns to its prior shape:
```
rm COPIES/README.TXT
rm COPIES/BACKUP.TXT
rmdir COPIES
```
- A **stalled** USB read/write must surface `-EIO` (the JD3 wall-clock BOT pump bounds it) — never a hang.
- JD4 read-side navigation and JD5/JD6/JD7 write/shape commands stay unchanged.

## Pass criteria
A file **copied on the panel survives a power cycle** (you can `cat` the copy after re-boot), the **source
is left byte-for-byte untouched**, both the `cp FILE DIR/` idiom and an explicit destination name work, an
empty-file copy produces a 0-byte destination, and every error probe prints its honest errno tag with no
wedge (`-ENOENT`, `-EISDIR`, `-EINVAL`, `-ENOTDIR`). Capture serial to `~/unaos-bench/`.
