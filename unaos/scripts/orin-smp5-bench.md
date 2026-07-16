# ORIN-SMP-5 bench runbook — the RESIDUE legs (attended; one leg per boot)

This continues the ORIN-SMP-4 bisect. That sitting (2026-07-15 attended, serial
`~/unaos-bench/jetson-serial-2026-07-15-smp4bisect.log`) came back **7/7 legs survived**: legs 10..15
matched their predictions exactly (leg 15's `GICR_WAKER @ 0xf460014` read survived — prime suspect
INNOCENT) and **leg 16's full-path replica survived AGAINST its prediction** — checkpoint `0x53040010`,
`AP -> BSP SGI OK (BSP ipi 1 -> 2)`: the first live UnaOS AP on Orin silicon. So the SMP-3 RAS fault
(IOB SERR=0x12 / CBB-0x6 / ADDR `0x8000000000000200`, box reset) was **NOT reproduced** by the replica.

The trigger therefore lives in what leg 16 deliberately **omitted** vs the real
`__secondary_rust_virt` flow. ORIN-SMP-5 restores those omitted elements one at a time, each still
"leg-16 shape + exactly ONE variable," in the same self-contained `smpprobe.rs` machinery
(`UNAOS_SMPPROBE=17..20`; `arch/aarch64/smpprobe.rs`, §ORIN-SMP-5). One leg per boot.

## Hard rules for this bench

1. **RIDER 1 — a leg-16 re-confirm runs FIRST every sitting.** Re-flash `UNAOS_SMPPROBE=16` and confirm
   `CHECKPOINT REACHED (0x53040010)` + `AP -> BSP SGI OK` before adding any SMP-5 variable — proves the
   replica still goes online on tonight's firmware+silicon before the residue is tested.
2. **RIDER 2 — predictions are pre-registered (the table below, written BEFORE any boot) and exactly
   ONE variable is added per leg.** The FIRST leg that RAS-powers-off NAMES the residual trigger →
   **STOP THE SITTING THERE** (that fault IS the located wall; record it, run no further legs). A
   survival that contradicts a "fault expected" row is still informative — continue per the schedule
   (it narrows the residue), but a leg that FAULTS where survival was predicted = STOP + report.
3. **RIDER 3 — power-fault boots are DATA.** A RAS power-off IS the verdict for a faulting leg; recover
   with a full DC cut (per the SMP-2 runbook) and continue only per the pre-registered schedule.
4. **RIDER 4 — probe-only.** The woken core touches ONLY its own stack, the regime registers, its own
   GICR frame, the checkpoint, and (leg 17 only) the shared `SERIAL_PORT` console via the same bounded
   `serial_println!` path the BSP uses — no new UART code, no fuse/persistent writes.
5. **RIDER 5 — DTB-only presence.** Targets come from the DTB `/cpus` list, never `AFFINITY_INFO`/GICR
   walk. Leg 19 STOPs (no `CPU_ON`) if `/cpus` does not name the cluster-1 core `0x0001_0200`. Every
   address under test (leg 17 UARTC base, leg 19 cluster-1 GICR frame) is COMPUTED + PRINTED BSP-side
   before the `CPU_ON`.

## Firmware precondition (assert BEFORE any leg)

The first serial lines must show UEFI `t23x_general 39.2.0-gcid-45755727` (or newer, Peter-acknowledged).
A downgraded/different firmware = **STOP** — the SMP-3/SMP-4 discrimination was established on 39.2.0.

## The evidence channel

Same as SMP-4: the woken core raises a **CHECKPOINT** (`0x5304_00<leg>` + `DC CVAC` to PoC) that the BSP
polls (invalidate-then-read) under a bounded ~500 ms deadline. Leg 17 is the ONE exception to woken-core
silence — its whole point is the AP's own `serial_println!` (the residue variable). Read the BSP lines:

- `:: tegra: SMPPROBE-4 sel=<n> CHECKPOINT REACHED (val=0x5304_00<n>) …` → survived.
- box RAS power-off BEFORE that line → the leg FAULTED (its restored element is the residual trigger).
- `:: tegra: SMPPROBE-4 sel=<n> CHECKPOINT NOT reached in ~500ms …; box still up …` → wrong-EL park /
  hang (NOT the RAS reset).
- Leg 17 only: a `:: tegra: SMPPROBE-5 sel=17 [AP] woken core online — serial_println! from the
  SECONDARY … ::` line printed BY THE WOKEN CORE proves the secondary's console access itself survived.

## Pre-registered prediction table (RIDER 2 — verbatim, matches §ORIN-SMP-5)

| leg | `UNAOS_SMPPROBE=` | residue element restored over leg 16 | predicted BSP serial | predicted box behavior |
|---|---|---|---|---|
| **17** | 17 | +ONE `serial_println!` from the WOKEN CORE (UART MMIO + `SERIAL_PORT` console spinlock from a secondary) | BSP: `sel=17 the woken core will serial_println! through tegra UART base 0xc280000 …`; then EITHER the AP's own `sel=17 [AP] woken core online …` line + `CHECKPOINT REACHED (val=0x53040011)` + `AP -> BSP SGI OK`, OR nothing (box down) | **PRIME residue suspect — RAS power-off expected** if the secondary's console access (UART MMIO / spinlock) is the rejected one. Survival → the AP-print line + checkpoint `0x53040011` appear; the print is benign; continue to 18. |
| **18** | 18 | +the real **WFI** idle tail (replica parks WFE; real path parks WFI) | `sel=18 … CHECKPOINT REACHED (val=0x53040012)` + `AP -> BSP SGI OK` | **SURVIVES** → checkpoint `0x53040012`, box up (parks in WFI). WFI is a benign idle instruction and the checkpoint is raised before it. A fault here would be surprising → STOP + report. |
| **19** | 19 | leg-16 shape on the **CLUSTER-1** core (DTB aff `0x0001_0200`) | BSP: `sel=19 cluster-1 target aff=0x00010200 GICR frame=0x…; GICR_WAKER @ 0x…`; then either `CHECKPOINT REACHED (val=0x53040013)` + `AP -> BSP SGI OK`, or box down | **Fault CANDIDATE** — cluster-1 core-power crosses a CCPLEX cluster boundary (per-cluster MCE/BPMP coordination the SMP-3 5-core sequence exercised). RAS power-off → the cross-cluster bring-up is the trigger; survival → checkpoint `0x53040013`, continue to 20. STOP (no `CPU_ON`) if `/cpus` omits `0x00010200`. |
| **20** | 20 | the real **5-core wake SEQUENCE** (every non-BSP `/cpus` core, leg-16 shape, DTB order, one at a time) | `sel=20 … {n} non-BSP /cpus core(s) to wake in order`; per core `[i/n] … CHECKPOINT REACHED (0x53040014); AP -> BSP SGI OK`; then `SEQUENCE DONE — n/n cores reached checkpoint` | **RUNS LAST, only if 17..19 all survived.** Fault CANDIDATE — a RAS power-off MID-SEQUENCE (the `[i/n] issuing CPU_ON …` line names the core under the gun) = the fault is driven by multi-core concurrency (SMP-3 woke five; the single-core legs woke one). All survive → the full 5-core wake completes, box up. |

**Leg-17 note.** The AP print uses the SAME bounded-TXFF tegra writer as the BSP `serial_println!` (no
new UART code); its serial bytes may interleave with the BSP's (unarbitrated on metal — the pi
core3probe lesson), so read the AP line loosely (its presence, not clean framing, is the signal). The
BSP names the UARTC base `0x0C28_0000` before `CPU_ON` so the address under test is on the transcript
even if the box goes down.

## Reading the results (decision table)

- **The FIRST leg that RAS-faults NAMES the residual trigger.** Its restored element is the culprit;
  STOP the sitting there and report the leg + (leg 19) the printed cluster-1 GICR address / (leg 20)
  the `[i/n]` core under the gun.
- Expected leading hypothesis: **17 faults** → the secondary console access (UART MMIO / `SERIAL_PORT`
  spinlock) is the wall; the real path prints "AP online" exactly where SMP-3 faulted.
- If 17 SURVIVES → the print is benign; the residue is the cluster boundary (19) or the multi-core
  sequence (20). Continue 18 → 19 → 20.
- If **ALL FOUR survive** → the SMP-3 fault is not reproduced by any single restored element → the
  trigger is timing/ordering/concurrency (or the real `_secondary_start_virt` entry shape itself); a
  follow-up arc bisects that. This is a real, informative outcome, not a null.

## Schedule (one leg per boot)

1. Re-flash `UNAOS_SMPPROBE=16` (RIDER 1), boot, assert firmware precondition, confirm
   `CHECKPOINT REACHED (0x53040010)` + `AP -> BSP SGI OK`.
2. Flash 17, boot, record: AP-print line? checkpoint `0x53040011`? or box down.
3. If 17 survived, ascend 18 → 19 → 20, reflashing per leg, recording each checkpoint / fault.
4. Run 20 LAST **only if** 17..19 all survived. If any leg faulted, STOP at it — the wall is located.
5. Record the git7 + tar sha of every boot (the CORE3 build-size discipline).

## Recovery

A RAS power-off leaves the box off; do a full DC cut (unplug the barrel supply, wait, replug) before
the next boot — a warm reset can leave the CBB/MCE in a poisoned state that muddies the next leg.
Recovery is identical to the SMP-2 runbook.

## Staged media (flash ONLY from `~/unaos-bench/flash/orin/`, never `target/`)

Four armed tars `UnaOS-orin-esp-smpprobe{17..20}-<UTCstamp>-<git7>.tar` (EFI + kernel.elf), plus the
knob-off DEFAULT tar for the byte-identity fallback. Shas in the MANIFEST + the ORIN-SMP-5 landing
report. Each armed image validates by its distinct ELF hash + `strings | grep SMPPROBE-5` present +
confirm the LIVE `sel=<n>` on the first `SMPPROBE-4`/`SMPPROBE-5` serial line matches the leg you
flashed BEFORE trusting the boot (`UNAOS_SMPPROBE` is compile-time — one image per leg). The DEFAULT
image carries ZERO `SMPPROBE-5` strings and `tegra:` 109.
