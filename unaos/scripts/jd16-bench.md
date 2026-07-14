# JD16 bench card — `ls -l` long listing with real FAT timestamps (attended)

JD16 adds `ls -l`: a long listing that shows each entry's size, its **real FAT last-write timestamp**, and its
name. It is the first arc to touch `fat.rs` — a **read-side-only** addition (`DirEntry` now parses the packed
timestamp words at offsets 0x16/0x18) — plus the `ls` arm in `shell.rs`. Plain `ls` is unchanged. This card
confirms the timestamp column on silicon: a **host-written** file shows its real host timestamp, a
**kernel-written** file shows the honest dashed placeholder (the kernel has NO RTC — it stamps zero), and an
mtime survives a real power-cycle. Thanks to JD11 it leaves a durable serial transcript. Detail + rationale:
[`arch_arm64.md` §JD16](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Pairs cleanly with the JD13–JD15 benches
in one attended session.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **109** (UNCHANGED from JD11–JD15 — JD16 adds no `tegra:` token). Copy `EFI` + `kernel.elf` to the
  boot stick, `dot_clean`, eject. Validate by count, not size.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. **⚠ For the host-timestamp check (§2), the card's staged
  files matter:** on your build/bench Mac, freshly write a file onto the card from the host so it carries a
  known real timestamp — e.g. `echo "host wrote this" > /Volumes/<CARD>/HOSTFILE.TXT`, note the date/time,
  then `dot_clean` the card and strip `._*` AppleDouble sidecars (they are glob-visible on FAT 8.3 short
  names). The panel creates the kernel-written files below itself.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ With JD11 the serial bridge is the **primary output-evidence channel** — confirm it is logging a full boot
to `~/unaos-bench/` BEFORE spending bench time (§JB1f: the round-6/8 host capture froze mid-bench). Screen-on-
boot (JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. A HOST-written file shows its REAL timestamp
```
ls                            # plain ls unchanged — short table, no timestamp column
ls -l                         # long format: size + "YYYY-MM-DD HH:MM:SS" + name
ls -l HOSTFILE.TXT            # the file you wrote from the host in §0
```
- **PASS:** `ls -l HOSTFILE.TXT` prints the file's size, then a real timestamp matching (within the FAT
  2-second resolution, and modulo any host/board timezone offset — FAT stores local wall-clock with no zone)
  the date/time you noted when you wrote it on the Mac, then the name. Plain `ls` output is byte-identical to
  every prior bench (no timestamp column).
- **Honest note:** the timestamp is what the HOST tool stamped; JD16 only displays it. A ±(timezone) or
  ±(2 s) difference is expected and correct, not a bug.

## 3. A KERNEL-written file shows the honest dashed placeholder
```
touch KFRESH.TXT              # kernel create path — no RTC, stamps zero
write KLINE.TXT kernel wrote this
ls -l                         # both new files listed
```
- **PASS:** `KFRESH.TXT` and `KLINE.TXT` each show their size (0 and the byte count) but a **dashed
  placeholder** where the timestamp would be — because the kernel has no clock and writes a zero timestamp.
  This is the correct, honest verdict for this arc: the OS does NOT fabricate a clock reading. (A real
  on-write clock is a named FUTURE arc.)
- Directories show `<DIR>` and a trailing `/`:
```
mkdir KDIR
ls -l                         # KDIR shows "<DIR> ... KDIR/" with the dashed placeholder (kernel-made)
```

## 4. `ls -l` on a wildcard and on a single file
```
ls -l *.TXT                   # the timestamp column applies to glob matches too (JD12 path)
ls -l HOSTFILE.TXT            # single-file long line: size + timestamp + name
```
- **PASS:** every match prints its own size + timestamp/placeholder + name, then the file/dir tally; the
  host-written match shows a real time, the kernel-written matches show the placeholder.

## 5. An mtime SURVIVES a power-cycle (durability)
- Note the exact `ls -l HOSTFILE.TXT` timestamp from §2. **Pull power** (genuine cold cut, the JD13–K4
  discipline), reboot to the prompt, then:
```
ls -l HOSTFILE.TXT            # same real timestamp as before the cut — mtime is on-disk, durable
```
- **PASS:** the host file's timestamp is identical across the power-cycle (it lives in the on-disk directory
  entry; JD16 reads, never rewrites it). The kernel-written files still show the dashed placeholder.

## Verdict
Record per section (PASS/FAIL + the observed lines from the serial transcript). The arc's headline claims:
(a) plain `ls` unchanged; (b) `ls -l` shows a host file's real FAT timestamp; (c) a kernel-written file shows
the honest dashed placeholder (no invented clock); (d) the column applies to globs and single files; (e) an
mtime survives a real power-cycle. Note the serial log filename (`~/unaos-bench/jetson-serial-<date>.log`).
