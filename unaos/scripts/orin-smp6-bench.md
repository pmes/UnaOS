# ORIN-SMP-6 bench runbook — the LAST-DIFFERENCES legs (attended; one leg per boot)

This closes the SMP-3 discrimination space. The ORIN-SMP-5 sitting (2026-07-16 attended, serial
`~/unaos-bench/jetson-serial-2026-07-16-smp5sitting.log`) acquitted EVERY residue element — AP serial
print (17), WFI tail (18), cluster-1 (19), and the serialized 5-core sequence (20; **5/5 online, every
core on the part has run UnaOS**) — while SMP-3 (the real `tegrasmp` bring-up) still RAS-faulted ×2
(IOB SERR=0x12 / CBB-0x6 / ADDR `0x8000000000000200`) BEFORE the first `CPU_ON` result printed.

Exactly TWO differences remain between everything acquitted and the faulting SMP-3 flow:

1. the **REAL `_secondary_start_virt` entry** (real stub code + real per-CPU `SECONDARY_STACKS` +
   the real `__secondary_rust_virt`) vs the probe's replica stub;
2. **RAPID-FIRE wake concurrency** (SMP-3's back-to-back `CPU_ON` loop) vs leg-20's
   park-before-next serialization.

ORIN-SMP-6 takes them ONE VARIABLE PER LEG (`UNAOS_SMPPROBE=21..23`; `arch/aarch64/smpprobe.rs`
§ORIN-SMP-6; the granted `smp_virt.rs` publish-only API feeds the real path). One leg per boot.
Together with leg 20 (stub × serialized, SURVIVED) the legs cover the full 2×2 of
{entry shape} × {concurrency}.

## Hard rules for this bench

1. **RIDER 1 — a leg-16 re-confirm runs FIRST every sitting.** Re-flash `UNAOS_SMPPROBE=16` and confirm
   `CHECKPOINT REACHED (0x53040010)` + `AP -> BSP SGI OK` before any SMP-6 leg — proves the replica
   still goes online on tonight's firmware+silicon.
2. **RIDER 2 — predictions are pre-registered (the table below, written BEFORE any boot) and exactly
   ONE variable per leg.** The FIRST leg that RAS-powers-off NAMES the trigger axis → STOP THE SITTING
   THERE. A leg that FAULTS where survival was predicted = STOP + report.
3. **RIDER 3 — power-fault boots are DATA.** Recover with a FULL DC CUT (unplug the barrel supply,
   wait, replug — a warm reset can leave the CBB/MCE poisoned) and continue only per the schedule.
4. **RIDER 4 — probe-only, with the documented exception.** Legs 21/23's woken cores run the REAL
   (probe-independent) bring-up — exactly what every `tegrasmp` boot runs — so their
   `:: AARCH64 SMP: AP <n> online … ::` prints are EXPECTED, not a rider breach. Leg 22 stays
   probe-silent (checkpoint slots only). Nothing writes persistent state.
5. **RIDER 5 — DTB-only presence.** All targets come from the DTB `/cpus` list. Every computed address
   (real entry PA, every target affinity, ctxids, leg-22 slot values) is PRINTED BSP-side BEFORE the
   first `CPU_ON`.
6. **Leg 23 runs LAST and ONLY if legs 21 AND 22 both survived** (the image is staged regardless; this
   gate is operational, not build-time).

## Firmware precondition (assert BEFORE any leg)

The first serial lines must show UEFI `t23x_general 39.2.0-gcid-45755727` (or newer,
Peter-acknowledged). A downgraded/different firmware = **STOP**.

## The evidence channels

- **Leg 21/23 (real entry):** the REAL path's own signals — the AP's
  `:: AARCH64 SMP: AP <idx> online (aff=…) ::` print, then BSP-side
  `SMPPROBE-6 sel=21 CORE_READY[1] SET — leg SURVIVED …` /
  `sel=23 [i/5] … CORE_READY[i] SET — online via the REAL path`, plus `AP -> BSP SGI OK`.
  AP serial bytes may interleave with the BSP's (unarbitrated on metal — presence, not framing, is
  the signal).
- **Leg 22 (stub, rapid):** per-core checkpoint slots, value `0x5304_01xx16` where `xx` = the core
  index in byte 1 (`0x53040116`, `0x53040216`, `0x53040316`, `0x53040416`, `0x53040516`), polled
  AFTER the print-free burst. `burst COMPLETE` on serial = the box survived all five back-to-back
  `CPU_ON`s.
- Box RAS power-off before the survival lines → the leg FAULTED (mid-burst for 22/23: the printed
  PLAN names every core; the burst is print-free by design, so the fault localizes to the burst as a
  whole, the pre-registered unit under test).
- `… NOT reached/NOT set in ~500ms; box still up …` → wrong-EL park or hang (NOT the RAS reset).

## Pre-registered prediction table (RIDER 2 — verbatim, matches §ORIN-SMP-6)

| leg | `UNAOS_SMPPROBE=` | the ONE variable | predicted BSP serial | predicted box behavior |
|---|---|---|---|---|
| **21** | 21 | the REAL `_secondary_start_virt` entry, ONE core (`0x00000100`, ctxid 1) — real stub + real `SECONDARY_STACKS[1]` + real `__secondary_rust_virt` | `sel=21 real path published … REAL entry _secondary_start_virt=0x…`; `issuing CPU_ON target aff=0x00000100 entry=0x… ctxid=1`; then EITHER the AP's own `:: AARCH64 SMP: AP 1 online (aff=0x00000100) ::` + `CORE_READY[1] SET — leg SURVIVED` + `AP -> BSP SGI OK`, OR nothing (box down) | **PRIME suspect — RAS power-off is the leading prediction** if the entry shape is the wall (SMP-3 died before core 1's result printed, and this is the first-ever single-core real-entry wake). SURVIVAL is fully plausible (then the wall is concurrency) → continue to 22. Either outcome is informative; neither STOPs the sitting by itself. |
| **22** | 22 | RAPID 5-core burst on the ACQUITTED replica stub — all five `CPU_ON`s back-to-back, poll after (per-core stacks + slots) | `sel=22 … plan [1/5]..[5/5] …` (every target/ctxid/slot value), then `burst COMPLETE`, then per-core `CPU_ON ret=0` ×5 and `CHECKPOINT REACHED (0x5304_0116..0x5304_0516)` ×5 + `AP -> BSP SGI OK` + `RAPID SEQUENCE DONE — 5/5` | **Fault CANDIDATE** — if wake concurrency (overlapping MCE/BPMP core-power transitions on the fabric) is the wall, the box RAS-powers-off mid-burst (after the plan, before/around `burst COMPLETE`). Survival → concurrency alone is innocent on stub code; continue per rule 6. |
| **23** | 23 | REAL entry × RAPID 5-core — SMP-3 replayed under instrumentation (print-free burst, faster than SMP-3's own loop) | `sel=23 real path published … plan [1/5]..[5/5] …`, `burst COMPLETE`, per-core `ret=0` + up to five `AP <i> online` prints + `CORE_READY[i] SET` ×5 + `RAPID REAL-ENTRY SEQUENCE DONE — 5/5` | **The SMP-3 replay.** If 21 AND 22 both survived, a fault HERE = the wall is the CONJUNCTION (real entry × concurrency). Survival of all three = SMP-3's fault is NOT reproduced under instrumentation → the trigger is a boot-state/ordering delta vs the `tegrasmp` flow (follow-up arc). RUNS LAST, only if 21+22 survived. |

## Reading the results (decision table)

- **21 faults** → the REAL entry shape is the wall, single-core. STOP; the fix arc targets
  `_secondary_start_virt`/`__secondary_rust_virt` early execution (codegen/disassembly burden, the
  CORE3-FIX idiom).
- **21 survives, 22 faults** → concurrency is the wall, on any code. STOP; the fix arc serializes the
  real bring-up (park-before-next, leg-20 shape) — a straightforward `smp_virt.rs` fix arc.
- **21+22 survive, 23 faults** → the conjunction is the wall; fix = serialize the real bring-up
  (same fix as above, with the added knowledge that either factor alone is innocent).
- **All three survive** → SMP-3's fault is not reproduced under instrumentation at all; the delta is
  in the surrounding `tegrasmp` boot context (what runs before/after the kick-off), a follow-up
  bisect. A real, informative outcome — not a null.

## Schedule (one leg per boot; 2–5 boots)

1. Flash `UNAOS_SMPPROBE=16` (RIDER 1), boot, assert firmware precondition, confirm
   `CHECKPOINT REACHED (0x53040010)` + `AP -> BSP SGI OK`.
2. Flash 21, boot, record: AP-1-online print? `CORE_READY[1] SET`? or box down (DC-cut recovery).
3. Flash 22, boot (even if 21 faulted — 22 discriminates the OTHER axis on safe code), record:
   `burst COMPLETE`? slots ×5? or box down mid-burst.
4. Flash 23 ONLY if 21 AND 22 both survived. Record per-core online / fault.
5. Record the git7 + tar sha of every boot; restore the DEFAULT image to the stick at close.

## Recovery

A RAS power-off leaves the box off; full DC cut (unplug barrel supply, wait ~10 s, replug) before the
next boot. Identical to the SMP-2/4/5 runbooks.

## Staged media (flash ONLY from `~/unaos-bench/flash/orin/`, never `target/`)

Three armed tars `UnaOS-orin-esp-smpprobe{21..23}-<UTCstamp>-<git7>.tar` (EFI + kernel.elf) + the
knob-off DEFAULT tar (byte-identity fallback) + the SMP-5-staged `smpprobe16` tar for RIDER 1 (already
in the MANIFEST from the SMP-5 arc; if absent, rebuild leg 16 from this tree — the leg-16 code is
untouched by SMP-6). Shas in the MANIFEST + the ORIN-SMP-6 landing report. Each armed image validates
by its distinct ELF hash + `strings | grep -a SMPPROBE-6` present + the LIVE `sel=<n>` echoed on the
first probe serial line. The default validates by hash + ZERO `SMPPROBE-6` strings.
