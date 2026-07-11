# JD6 bench card — subdirectory writes (attended, on the Orin)

The money shot: `cd` into a subdirectory, **write a file there**, power-cycle, `cd` back, `cat` it —
proving the write path now reaches the whole tree, not just the root (JD5). Detail + rationale:
[`arch_arm64.md` §JD6](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from JD3/JD4/JD5.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and size-check by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **108** (virt clobber ≈ 0). Copy `EFI` + `kernel.elf` to the boot stick, `dot_clean`, eject.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. **It MUST already contain at least one subdirectory**
  (e.g. `DOCS/`) — the shell has no `mkdir`/`rmdir` this arc, so the money-shot writes INTO a directory
  that already exists. The pi4 fixture `UNAOSRW` card already has a `DOCS` subdir (JD4/JD5 `ls docs`).
  Leave a few free clusters (the write allocates).
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle. That graceful fallthrough is itself a pass check.

## 1. Connect the serial console
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
Screen-on-boot (JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. The subdirectory write sequence (type on the panel; watch panel + serial)
```
ls                            # root listing — confirms DOCS/ is present (else pick an existing subdir)
cd DOCS                       # descend (JD4 navigation)
pwd                           # -> /DOCS
write NOTE.TXT hello from a subdir on the orin
                              # -> "wrote 30 bytes to /DOCS/NOTE.TXT (30 bytes)"   ← canonical path
cat NOTE.TXT                  # -> "hello from a subdir on the orin"
append NOTE.TXT and more
                              # -> "appended 9 bytes to /DOCS/NOTE.TXT (39 bytes)"
cat NOTE.TXT                  # -> "hello from a subdir on the orinand more"  (exact bytes)
```
**Now POWER-CYCLE the board** (write-through durability across a reboot, in a subdir). Reconnect serial:
```
cd DOCS
cat NOTE.TXT                  # -> the content SURVIVES the power cycle  ← THE money shot
rm NOTE.TXT                   # -> "removed /DOCS/NOTE.TXT (K cluster(s) freed)"
cat NOTE.TXT                  # -> "cat: /DOCS/NOTE.TXT: not found (-ENOENT)"
```
Also prove the absolute-path form (no `cd` needed) still reaches the subdir:
```
write DOCS/A.TXT direct       # -> "wrote 6 bytes to /DOCS/A.TXT (6 bytes)"
cat DOCS/A.TXT                # -> "direct"
rm DOCS/A.TXT                 # -> "removed /DOCS/A.TXT (…)"
```

## 3. Honest-error probes (must NOT hang, must print the tag)
```
write DOCS hi                 # DOCS is a directory        -> "...is a directory (-EISDIR)"
rm DOCS                       # directory removal          -> "...is a directory (-EISDIR)"  (rmdir out of scope)
write NONESUCH/X.TXT hi       # parent dir missing         -> "...: not found (-ENOENT)"
write NOTE.TXT/X.TXT hi       # parent is a FILE           -> "...: not a directory (-ENOTDIR)"   (after re-creating NOTE.TXT)
write /                       # root as a target           -> "/: is a directory (-EISDIR)"
```
- A **stalled** USB write must surface `-EIO` (the JD3 wall-clock BOT pump bounds it) — never a hang.
- `sync` prints its write-through confirmation (every write is already durable — no cache).
- JD4 read-side navigation (`ls DIR` / `cd` / `pwd` / `cat PATH`) must be unchanged.

## Pass criteria
A file created **inside a subdirectory** on the panel **survives a power cycle** and deletes cleanly;
both the `cd`-relative and absolute (`DOCS/NAME`) forms reach it; the error probes each print their
honest errno tag with no wedge; `rmdir` stays refused (`-EISDIR`). Capture serial to `~/unaos-bench/`.
