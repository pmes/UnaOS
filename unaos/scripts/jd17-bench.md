# JD17 bench card — the kernel wall clock: `date -s`-seeded mtime stamping (attended)

JD17 adds a kernel wall clock. §JD16 showed FAT mtime read-only; the kernel had no RTC, so every kernel-written
file showed a dashed placeholder. JD17 lets the operator **seed** wall time once per boot with `date -s`; the
free-running architectural counter (the JD3 mechanism) extends it forward, and the FAT create/write
**publication** paths stamp mtime from it. The contract is honest at both ends: **before** `date -s`, a
kernel-written file still shows the dash (the kernel never fabricates a reading); **after** `date -s`, a
newly-created/written file carries a real stamp that survives a power-cycle. This card confirms both halves on
silicon. Thanks to JD11 it leaves a durable serial transcript. Detail + rationale:
[`arch_arm64.md` §JD17](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Pairs cleanly with the JD16 `ls -l` bench
in one attended session.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **109** (UNCHANGED from JD11–JD16 — JD17 adds no `tegra:` token). Copy `EFI` + `kernel.elf` to the
  boot stick, `dot_clean`, eject. Validate by count, not size.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. **⚠ `dot_clean` the DATA card too** and strip `._*`
  AppleDouble sidecars (they are glob-visible on FAT 8.3 short names). No specific host file is required for
  this card — JD17 creates its own kernel-written files below.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ With JD11 the serial bridge is the **primary output-evidence channel** — confirm it is logging a full boot
to `~/unaos-bench/` BEFORE spending bench time (§JB1f: the round-6/8 host capture froze mid-bench). Screen-on-
boot (JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. UNSET is honest: `date` reports no clock, a fresh file shows the dash
```
date                          # before any date -s
touch PRECLOCK.TXT            # kernel create BEFORE the clock is set
ls -l PRECLOCK.TXT
```
- **PASS:** `date` prints `date: clock not set (date -s YYYY-MM-DD HH:MM:SS)`; `PRECLOCK.TXT` shows its size
  and a **dashed placeholder** where the timestamp would be. The kernel does NOT invent a clock reading.

## 3. `date -s` seeds the clock and `date` reads it back (counter-extended)
```
date -s 2026-07-15 14:30:00   # seed wall time
date                          # reads back ~14:30:00, ticking forward from the arch counter
date                          # a few seconds later — the seconds have advanced
```
- **PASS:** `date -s` prints `clock set: 2026-07-15 14:30:00`; the first `date` shows ~`2026-07-15 14:30:0x`,
  and a second `date` a few seconds later shows a **later** time (the free-running counter is extending it).
  A ±2 s granularity is expected (FAT/counter resolution), not a bug.
- **Range check (honest rejection):**
```
date -s 1979-01-01 00:00:00   # below the FAT epoch
date -s 2026-13-01 00:00:00   # impossible month
```
  Both print the usage line `date -s: usage: setdate YYYY-MM-DD HH:MM[:SS]  (year 1980-2107)` and leave the
  previously-set clock intact (`date` still reads the 14:30 seed forward).

## 4. A file created/written AFTER `date -s` carries a REAL stamp
```
touch KSTAMP.TXT              # kernel create — now stamped from the seeded clock
write KLINE.TXT kernel wrote this
ls -l                         # both new files show a real 2026-07-15 14:3x timestamp, NOT a dash
ls -l PRECLOCK.TXT            # the pre-clock file STILL shows the dash (created before the seed)
```
- **PASS:** `KSTAMP.TXT` and `KLINE.TXT` each show a real timestamp near the seeded moment; `PRECLOCK.TXT`
  (created in §2, before the seed) still shows the dashed placeholder. The stamp lands on the create/write
  **publication** path from the live clock.
- **Preserve-don't-zero note (optional):** re-writing a HOST-stamped file after boot but BEFORE any `date -s`
  keeps its original host stamp (the kernel leaves the on-disk words untouched when unset) — it is never
  zeroed. (Only exercisable if you stage a host file; not required for the headline verdict.)

## 5. A stamped mtime SURVIVES a power-cycle, and next boot is UNSET again
- Note the exact `ls -l KSTAMP.TXT` timestamp from §4. **Pull power** (genuine cold cut, the JD13–K4
  discipline), reboot to the prompt, then:
```
ls -l KSTAMP.TXT              # same 2026-07-15 14:3x timestamp — mtime is on-disk, durable
date                          # clock is UNSET again (no RTC — the seed does not persist across boots)
touch KFRESH2.TXT            # created next boot WITHOUT a date -s
ls -l KFRESH2.TXT            # shows the DASH — honest, because the clock was never set this boot
```
- **PASS:** `KSTAMP.TXT`'s stamp is identical across the power-cycle (it lives in the on-disk directory
  entry). `date` reports "clock not set" on the fresh boot, and a file touched before re-seeding shows the
  dash — the seed is per-boot, exactly as designed.

## Verdict
Record per section (PASS/FAIL + the observed lines from the serial transcript). The arc's headline claims:
(a) UNSET is honest — `date` says so and a pre-clock file shows the dash; (b) `date -s` seeds and `date` reads
it back, counter-extended; (c) out-of-range `date -s` is rejected without disturbing a set clock; (d) a file
created/written after the seed carries a real stamp while a pre-seed file keeps its dash; (e) a stamped mtime
survives a real power-cycle and the next boot comes up UNSET again. Note the serial log filename
(`~/unaos-bench/jetson-serial-<date>.log`). ⚠ `dot_clean` BOTH cards.
