# ORIN-SMP-3 bench card — the real 6-core Orin bring-up (`UNAOS_TEGRASMP`, attended)

This is a **verification** bench: QEMU cannot model the Tegra machine, so the metal Orin boot is the
only place the tegra SMP kick-off runs for real. ORIN-SMP-2's bench already proved `CPU_ON` works on
this firmware (the JM5 wall is gone); this card confirms the born-fixed §ORIN-SMP bring-up brings
every fused core online and each secondary re-derives its own linear index from live MPIDR.

Attended (LC-orin + Peter). Drive the shell by Peter-types / session-verifies off the JD11 serial
mirror — the Orin shell input is USB keyboard ONLY (serial RX is not an input path).

## ⛔ RIDER 2 — FIRMWARE PRECONDITION (assert BEFORE any boot)

The kick-off's first serial line restates it; assert it here too. The UEFI banner MUST read
`t23x_general 39.2.0-gcid-45755727` (2026-06-01) or a newer Peter-acknowledged build — the firmware
under which `CPU_ON` is known-good (ORIN-SMP-2 verdict). **A downgraded/different firmware = STOP
before trusting the run** — the JM5 RAS wall may still stand there, so a `CPU_ON` could power the box
off. Record the observed UEFI build line in the bench log.

## ⛔ RIDER 1 — the DTB `/cpus` is the ONLY presence oracle

The kernel targets exactly the cores the DTB `/cpus` node names — never `AFFINITY_INFO` (12
false-valid slots on the 6-core Nano) nor the GIC redistributor walk (8 frames). If the enumeration
prints fewer/more than the expected 6 cores, that is DATA about the firmware DTB, not a bug to patch
on-metal — record it and STOP. Do NOT improvise a probe of a non-`/cpus` core (only Peter adds that
as its own pre-registered leg).

## 0. Build + stage (Peter builds; flash ONLY from the staged tar)

Build the tegra ESP LAST (any `test-arm`/`test` clobbers `target/aarch64_esp`). Validate the armed
image by the distinct ELF hash + `ORIN-SMP-3` string presence — NOT by `tegra:` count (the kick-off's
records use the `AARCH64 SMP:` family, so the armed `tegra:` count is 109, same as baseline):

```
UNAOS_TEGRA=1 UNAOS_TEGRASMP=1 ./arroyo esp-jetson
strings target/aarch64_esp/kernel.elf | grep -c 'ORIN-SMP-3'   # > 0 confirms the kick-off is armed
```

**⛔ Flash ONLY from the staged artifact, never from `target/`** (`~/unaos-bench/flash/README.md`).
This arc pre-stages the armed + the default (knob-off) ESP tars under
`~/unaos-bench/flash/orin/UnaOS-orin-esp[-tegrasmp]-<UTCstampZ>-<git7>.tar`, each with a MANIFEST
sha256 line. Verify the sha256, untar onto the boot stick, `dot_clean` BOTH cards, eject. The armed
tar is the SMP boot; the default tar is the byte-identical-off fallback (proves the knob-off image).

**Serial-verify before trusting the boot:** the first SMP line is
`:: AARCH64 SMP: ORIN-SMP-3 kick-off — PRECONDITION UEFI t23x_general 39.2.0-gcid-45755727 … ::`.

## 1. Connect the serial console — VERIFY CAPTURE FIRST

```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```

Confirm a FULL boot is captured from byte 0 before bench time (the round-6/8 lesson: host capture can
freeze mid-boot). With JD11 the serial mirror is the primary output channel.

## 2. The money shot — 6 cores online

Boot the armed image. Expect on serial (reconstruct with `awk '/AARCH64 SMP: ORIN-SMP-3/'`):

1. The PRECONDITION line (RIDER 2).
2. `ORIN-SMP-3 enumerated core <k> aff=… (source=DTB /cpus)` for k = 0..5 — **6 cores**, core 0 the
   BSP. (If `bsp aff not present in DTB /cpus` prints, note it — a firmware/enumeration inconsistency,
   not a bring-up failure.)
3. `ORIN-SMP-3 CPU_ON AP <n> (aff=…) -> SUCCESS` for each of the **5** secondaries (ret=0). A
   `-> ERROR <r>` on any core = record the errno + which affinity; the box should NOT power off on
   this firmware (contrast the JM5 wall).
4. `AARCH64 SMP: AP <n> online (aff=…)` for each secondary that checked in — **expect ×5**. A
   `WARNING AP <n> … did not come online` = a bounded-timeout miss (recorded, boot continues).
5. `ORIN-SMP-3 BSP -> AP <n> SGI OK (count 0 -> 1)` ×5 and `ORIN-SMP-3 AP -> BSP SGI OK (… delivered)`
   — the cross-core IPI proof in both directions.
6. `ORIN-SMP-3 5/5 secondaries online via PSCI CPU_ON (DTB /cpus oracle)`.
7. Then the boot core proceeds to the **JM6** EL1 drop and **CAPSTONE 6/6** (the existing single-core
   scheduler run — unchanged; the secondaries park in WFI at EL2).

## 3. Power-cycle durability + the knob-off fallback

- Genuine power-cut, re-boot the armed image: the 6-core enumeration + 5/5 online repeat every boot
  (the bring-up is stateless — no persistence expected, just deterministic re-enumeration).
- Boot the **default (knob-off) tar** once: NO `ORIN-SMP-3` lines, single-core JM6 → CAPSTONE 6/6
  (the byte-identical-off image; proves the knob gates cleanly).

## 4. STOP tripwires (record exactly, do not improvise)

- The UEFI build line differs from RIDER 2's precondition → STOP before the SMP boot.
- Any `CPU_ON` powers the box off / RAS-faults (the JM5 wall reappearing) → STOP; record the firmware
  build + which affinity + the last serial line.
- The `/cpus` enumeration names a core count other than 6 → record the full enumeration + STOP.
- A secondary comes online but CAPSTONE/JM6 regresses → STOP (the drop path is unchanged; a regression
  there is unexpected).

Verdict = the attended observation + the serial capture (`~/unaos-bench/jetson-serial-…-smp3.log`).
Fold ✅/notes into `arch_arm64.md §ORIN-SMP-3` + MILESTONES (⏳ → ✅ METAL-CONFIRMED) at the next
seat/Maestro pass.

---
## ⛔ BENCH STOP VERDICT (2026-07-15 night; serial `~/unaos-bench/jetson-serial-2026-07-15-smp3bench.log`)

Firmware precondition MATCHED (39.2.0 banner). Enumeration 6/6 (real fused topology: cluster0 `0x0..0x300`,
cluster1 `0x10200/0x10300`). Then, BEFORE any `CPU_ON AP ->` result line, ×2 reproducible: `Exception
reason=1 syndrome=0x82000010` + RAS Uncorrectable **IOB** (base `0xe010000`, Status `0xe4000612`,
SERR=0x12 slave-error, IERR=CBB Interface 0x6, **ADDR=`0x8000000000000200`**) + **ACI** (base `0xe01a000`)
→ box reset. STOP after the second fault; no improvised legs.

**Discrimination vs SMP-2 exp5 (same firmware, same target aff `0x00000100`):** exp5 entry =
`_smpprobe_park` → SURVIVED; SMP-3 entry = `_secondary_start_virt` (real path) → FAULTS.
**Firmware `CPU_ON` works; the woken core's early execution drives the CBB-rejected access.**
Next: ORIN-SMP-4 pre-registered execution bisect (park control → +SP → +regime → +MMU → +exceptions →
+GICR → full; GICR = prime suspect — 8 frames on a 6-core part; fault ADDR smells like an MMIO window).
