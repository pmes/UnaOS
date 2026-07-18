# V3D-3 — landing report (hw-pi4)

**Arc:** PI-V3D-3 — the PM/ASB enable step, the enable-sequence refinement after V3D-2's metal
refutation. Branch `hw-pi4`.

## The ground truth being refined (PI-V3D-2 metal verdict, 2026-07-18, non-relitigable)

Firmware power domain 10 ACKed ON, clock id 5 rate 500 MHz ACKed, clock GATE ACKed ENABLED/active —
and the V3D hub STILL read `0xdeadbeef` (probe correctly fail-closed `BUS-POISON`). The poison-honest
probe leg is CONFIRMED and is **not touched** by this arc. Conclusion of record: **the RPi firmware
property-channel power+clock path is NOT sufficient to decode the V3D block on BCM2711.**

## The adjudication — is ASB the missing piece? YES.

Established from Linux `drivers/soc/bcm/bcm2835-power.c` and `arch/arm/boot/dts/bcm2711.dtsi` (rpi-6.1.y;
rpi-6.6.y / master 404'd, 6.1.y is current for these files):

- On BCM2711 the V3D power domain (`BCM2835_POWER_DOMAIN_GRAFX_V3D`) is brought up by
  `bcm2835_asb_power_on(pd, PM_GRAFX, ASB_V3D_M_CTRL, ASB_V3D_S_CTRL, PM_V3DRSTN)`.
- The PM_POWUP / inrush / memory-repair core sequence (`bcm2835_power_power_on`) is **skipped on
  BCM2711** — `if (power->rpivid_asb) return 0`. The firmware already runs it; that is exactly why our
  mailbox `SET_DOMAIN_STATE` domain 10 ACKs ON. So the firmware power path is real and necessary, but it
  is only *part* of bring-up.
- What `bcm2835_asb_power_on` **still runs on BCM2711**, and the firmware property path does **not** do,
  is the two-step piece:
  1. **Deassert the V3D reset**: set `PM_V3DRSTN` (bit 6) in `PM_GRAFX` (PM block offset `0x10c`), written
     with the PM password `0x5A000000`.
  2. **Release the two async AXI bridges**: clear `ASB_REQ_STOP` (bit 0) in `ASB_V3D_M_CTRL` (offset
     `0x0c`) then `ASB_V3D_S_CTRL` (offset `0x08`), each with the PM password, waiting for `ASB_ACK`
     (bit 1) to clear.
- **Base disambiguation (the load-bearing detail):** the V3D ASB registers are in the `rpivid_asb`
  block, **not** the legacy `asb` block. `bcm2835_asb_control` routes `ASB_V3D_{S,M}_CTRL` to
  `power->rpivid_asb` when present (always, on BCM2711). The DT `pm` node's three reg ranges confirm the
  bases:
  - `pm`         `<0x7e100000 0x114>`  → ARM PA `0xFE10_0000`  (holds `PM_GRAFX` @ `0x10c`)
  - `asb`        `<0x7e00a000 0x24>`   → ARM PA `0xFE00_A000`  (legacy — NOT used for V3D)
  - `rpivid_asb` `<0x7ec11000 0x20>`   → ARM PA `0xFEC1_1000`  (holds `ASB_V3D_{S,M}_CTRL`)
  V3D hub `<0x7ec00000 0x4000>` = ARM `0xFEC0_0000` and core0 `0xFEC0_4000` match the existing constants.

Both new bases sit inside the `boot.rs` L1[3] Device-nGnRnE window (`0xC000_0000–0xFFFF_FFFF`) — **no new
MMU mapping**. Firmware power/rate/gate steps are **kept** (they stand in for the skipped PM_POWUP
sequence); the PM/ASB step is sequenced **after** them, before the probe.

## The exact new sequence (added to `v3d.rs::bringup`, between gate-enable and settle+probe)

`enable_pm_asb()`:
1. `PM_WRITE(PM_GRAFX, PM_READ(PM_GRAFX) | PM_V3DRSTN)`  (password in top byte) — deassert reset.
2. `asb_release(ASB_V3D_M_CTRL)` — read, clear `ASB_REQ_STOP`, write with password, bounded wait for
   `ASB_ACK` to clear.
3. `asb_release(ASB_V3D_S_CTRL)` — same, slave bridge.

Discipline: announced-before-issue writes; PM-password on every PM/ASB write; poison-honest readback +
log at each stage; each `ACK`-clear wait is a finite CNTPCT backstop (reuses `wait_bit_clear`, ~500 ms
cap). Best-effort — a bridge that never ACKs or reads poison is logged and bring-up proceeds; the
`IDENT0` probe (unchanged, V3D-2 semantics) is the real verdict gate. Nothing can fault or hang, so QEMU
stays on the honest BLOCK-DOWN.

The probe's three-verdict semantics, the MMU backstop, sched, smp, and the mbench battery are untouched.

## Expected metal chain (the discriminator)

QEMU: honest **BLOCK-DOWN** (models neither `rpivid_asb` nor V3D — ASB reads 0, ACK already clear, no
wait, no fault). Metal: **BLOCK-UP** — a live V3D identity after the corrected sequence → M1 probe PASS →
M2 MMU PASS → M3 clear-job. If it still reads poison the probe fail-closes `BUS-POISON` with the raw
IDENT word — honest data for the next refinement, not a STOP.

## Gate results (verbatim)

- `./arroyo check` (both arches): `✅ aarch64 OK` (x86 + aarch64 type-check green; pre-existing warnings
  only).
- `UNAOS_V3D=1 ./arroyo kernel8-test 35` → `mbench.py --spec scripts/specs/pi4-regression.spec --replay`:
  `✅ MBENCH PASS — 46/46 required witnesses, 0 forbidden hit(s), 200 lines scanned`. Forbidden scan of
  the serial log (`AARCH64 EXCEPTION|PANIC|-> FAIL|Serror`): **0 hits**. New PM/ASB chain present, all
  readbacks `0x00000000`, both bridges "ACK clear (bridge released)", probe verdict **BLOCK-DOWN** — no
  exception, no fault.
- Knob-off `./arroyo kernel8-test 35` → replay: `✅ MBENCH PASS — 46/46 required witnesses, 0 forbidden
  hit(s), 189 lines scanned`, 0 FAIL. (All edits are inside `#[cfg(feature = "v3d")]`-gated code, so
  knob-off `kernel8.img` is byte-identical to baseline by construction.)
- `./arroyo test-arm 22`: `✅ aarch64 test complete`; forbidden scan clean.
- `UNAOS_GICV3=1 ./arroyo test-arm 40`: `✅ aarch64 test complete`, `CAPSTONE COMPLETE — all 6 sync
  primitives verified`; forbidden scan clean.
- `./arroyo test 22`: `✅ Test run complete`; forbidden scan clean.

## Files touched (in lane)

- `unaos/crates/kernel/src/arch/aarch64/v3d.rs` — PM/ASB constants, `enable_pm_asb()`, `asb_release()`,
  the call in `bringup()`. All within the `v3d`-gated module.
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` §PI-V3D — new `### PI-V3D-3` subsection (verdict verbatim +
  ASB adjudication + new expected metal chain) + a `bcm2835-power.c`/`bcm2711.dtsi` reference line.
- `review/unaos-v3d3-LANDING.md` — this report.
- `mailbox.rs` — **not needed** (the firmware power/rate/gate tags already exist from V3D-2; the PM/ASB
  writes are direct MMIO, not mailbox tags).

## Flagged

- Nothing out-of-lane; no protection weakened. Probe verdict semantics, MMU backstop, sched, smp, mbench
  battery untouched.
- The ASB `ACK`-clear wait reuses the shared `wait_bit_clear` (~500 ms cap). On a metal BUS-POISON re-run
  where ACK never clears, this adds ~1 s (two waits) to boot before the honest verdict — bounded and
  acceptable; not a hang. Metal verification (BLOCK-UP vs BUS-POISON) is the next attended Pi sitting.
