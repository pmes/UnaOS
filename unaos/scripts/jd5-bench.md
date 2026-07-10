# JD5 bench card — the panel write path (attended, on the Orin)

The money shot: create a file on the panel, **power-cycle**, `cat` it back. Detail + rationale:
[`arch_arm64.md` §JD5](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from JD3/JD4.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and size-check by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **108** (virt clobber ≈ 0). Copy `EFI` + `kernel.elf` to the boot stick, `dot_clean`, eject.
- **Data card:** a **separate** FAT card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. Use **FAT16** to match the JD4-confirmed path, with
  some free space (a few clusters). It may start empty — JD5 creates the file.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle. That graceful fallthrough is itself a JD5 pass check.

## 1. Connect the serial console
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
Screen-on-boot (JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. The write sequence (type on the panel; watch panel + serial)
```
diskinfo                      # geometry — confirms the data card enumerated (else re-seat)
ls                            # root listing
write NOTE.TXT hello from the orin
                              # -> "wrote 19 bytes to /NOTE.TXT (19 bytes)"
cat NOTE.TXT                  # -> "hello from the orin"
append NOTE.TXT and more text
                              # -> "appended 13 bytes to /NOTE.TXT (32 bytes)"
cat NOTE.TXT                  # -> "hello from the orinand more text" (exact bytes, no separator)
```
**Now POWER-CYCLE the board** (write-through durability is the point). Reconnect serial, then:
```
cat NOTE.TXT                  # -> the content SURVIVES the power cycle  ← THE money shot
rm NOTE.TXT                   # -> "removed /NOTE.TXT (K cluster(s) freed)"
cat NOTE.TXT                  # -> "cat: /NOTE.TXT: not found (-ENOENT)"
```

## 3. Honest-error probes (must NOT hang, must print the tag)
```
write DOCS/X.TXT hi           # -> "...writes are root-directory only this arc (-ENOTSUP)"
cat /                         # (sanity) still navigable
touch NOTE.TXT ; touch NOTE.TXT   # idempotent — second is a clean no-op
```
- A **stalled** USB write must surface `write: /NOTE.TXT: -EIO (Io)` — never a hang (the JD3 wall-clock
  BOT pump bounds it; the timerless EL1 core busy-spins to the deadline, then errors).
- `sync` prints "write-through storage — every write is already durable on the card".

## Pass criteria
File created + edited on the panel, **survives a power cycle**, deletes cleanly; subdir write is a
clean `-ENOTSUP`; a stalled write is `-EIO`, never a wedge. Capture serial to `~/unaos-bench/`.
