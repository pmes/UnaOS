# ORIN-SMP-7 bench runbook — the BOOT-STATE-CONTEXT bisect (attended; one leg per boot)

The XCARVE leg-23 close-out (2026-07-16 attended) acquitted the last SMP-3 code-shape suspect: the
conjunction (real entry × rapid 5-core) is INNOCENT on silicon, replicated ×5. Every single-variable
and conjunction suspect is now acquitted. What remains for the original SMP-3 fault (IOB `SERR=0x12`
/ CBB-`0x6` / ADDR `0x8000000000000200` — a bit-63 address, echoing the XCARVE carveout wall's bit-63
`0x800000027767dc80`) is BOOT-STATE CONTEXT: the fault fired from the real `UNAOS_TEGRASMP=1` kick-off
at its production position, while every surviving probe ran from the `smpprobe` dispatch point.

ORIN-SMP-7 bisects WHERE in the boot the wake fires, relative to the JB2b `jb2b_attach` xHCI takeover
/ JB9i inherited-slot eviction (the XCARVE-suspect step, and the interplay the bit-63 echo suggests).
See `arch/aarch64/smpprobe.rs` §ORIN-SMP-7 + arch_arm64.md §ORIN-SMP-7 for the boot-ordering audit.

## The bisect (one variable = dispatch POSITION)

Legs 24 and 25 run the IDENTICAL wake code (`run_real_entry_rapid`, the leg-23 real-entry × rapid
5-core path — publication via the SMP-6-granted `smp_virt::probe_publish_real_path`, real
`_secondary_start_virt`, `CORE_READY` online signal). They differ ONLY in dispatch POSITION, so the
pair isolates exactly one variable — the xHCI takeover/eviction fabric state at wake time:

- **Leg 24 (`UNAOS_SMPPROBE=24`)** — the REPRO CONTROL: the wake at the **POST-xHCI-takeover** site
  (`smpprobe::run`, after `jb2b_attach`), where leg 23 already survived ×5.
- **Leg 25 (`UNAOS_SMPPROBE=25`)** — the SAME wake at the **PRE-xHCI-takeover** site
  (`smpprobe::run_pre_xhci`, after JM4 + heap, BEFORE `jb2b_attach`).

Legs **26/27 are FOLDED** (not built): 26 ("immediately after JB9i eviction, before the rest of
`jb2b_attach`") needs an in-`jb2b_attach` hook in `xusb_tegra.rs` (another executor's file) → OUT OF
LANE, flagged to LC-orin; leg 24 (post-FULL-takeover) already brackets the post-eviction fabric. 27
("full production post-wake path") is degenerate — after any probe leg the BSP already runs the JM6
drop + CAPSTONE with the APs parked in WFI (the real SMP-3 post-wake path). If 26/27 are armed the
image prints a self-documenting fold line and continues single-track.

## Hard rules for this bench

1. **RIDER 1 — a leg-23 (or leg-16) re-confirm runs FIRST every sitting.** Re-flash
   `UNAOS_SMPPROBE=23` and confirm `RAPID REAL-ENTRY SEQUENCE DONE — 5/5` + `AP -> BSP SGI OK` (or, if
   preferred, leg 16 → `CHECKPOINT REACHED (0x53040010)`) before any SMP-7 leg — proves the real-entry
   rapid path still goes online on tonight's firmware + silicon and anchors the control.
2. **RIDER 2 — predictions are pre-registered (the table below, written BEFORE any boot); exactly ONE
   variable per leg (position).** A leg that FAULTS where survival was predicted — or the FIRST leg to
   RAS-power-off — NAMES the trigger and **STOPs the sitting there**.
3. **RIDER 3 — power-fault boots are DATA.** Recover with a FULL DC CUT (unplug the barrel supply,
   wait ~10 s, replug — a warm reset can leave the CBB/MCE poisoned) and continue only per the schedule.
4. **RIDER 4 — probe-only, with the documented exception.** Legs 24/25's woken cores run the REAL
   (probe-independent) bring-up — exactly what every `tegrasmp` boot runs — so their
   `:: AARCH64 SMP: AP <n> online … ::` prints are EXPECTED, not a rider breach. Nothing writes
   persistent state. The shared `smp_virt.rs` path is byte-untouched — the arc only READS via the
   SMP-6-granted `probe_publish_real_path` / `probe_core_online`.
5. **RIDER 5 — DTB-only presence.** All targets come from the DTB `/cpus` list. Every computed address
   (real entry PA, every target affinity, ctxids) is PRINTED BSP-side BEFORE the first `CPU_ON`.

## Firmware precondition (assert BEFORE any leg)

The first serial lines must show UEFI `t23x_general 39.2.0-gcid-45755727` (or newer,
Peter-acknowledged). A downgraded/different firmware = **STOP**.

## The evidence channels

- **Leg 24 (post-takeover):** `:: tegra: SMPPROBE-7 sel=24 — REAL-ENTRY × RAPID 5-core …`, then the
  plan lines, `burst COMPLETE`, per-core `CPU_ON ret=0` + up to five `:: AARCH64 SMP: AP <i> online
  (aff=…) ::` + `CORE_READY[i] SET — online via the REAL path` + `AP -> BSP SGI OK` +
  `RAPID REAL-ENTRY SEQUENCE DONE — 5/5`. This leg fires at the SAME site leg 23 did.
- **Leg 25 (pre-takeover):** an EARLY banner `:: tegra: SMPPROBE-7 ARMED sel=25 (PRE-xHCI-takeover
  site — before jb2b_attach/JB9i eviction) …` appears BEFORE the JB2b attach lines in the log, then
  the same `SMPPROBE-7 sel=25 …` plan/burst/online sequence. At the post-takeover `smpprobe::run`
  site later, sel=25 prints `the wake already fired at the early dispatch site … boot continues`.
- Box RAS power-off before the survival lines → the leg FAULTED. For 24/25 the burst is print-free by
  design, so a mid-burst fault localizes to the burst as the pre-registered unit under test; the
  printed PLAN names every core.
- `… CORE_READY[i] NOT set in ~500ms; box up …` → wrong-EL park or hang (NOT the RAS reset).

## Pre-registered prediction table (RIDER 2 — verbatim, matches §ORIN-SMP-7)

| leg | `UNAOS_SMPPROBE=` | the ONE variable (position) | predicted BSP serial | predicted box behavior |
|---|---|---|---|---|
| **23** | 23 | RIDER-1 control re-confirm (post-takeover, real entry × rapid) | `RAPID REAL-ENTRY SEQUENCE DONE — 5/5` + `AP -> BSP SGI OK` | **SURVIVE** (×5 on silicon already). If it faults tonight, STOP — the silicon/firmware baseline moved. |
| **24** | 24 | REAL entry × rapid 5-core at the **POST-xHCI-takeover** site — the REPRO CONTROL | `SMPPROBE-7 sel=24 … burst COMPLETE … CORE_READY[i] SET ×5 … SEQUENCE DONE — 5/5` | Per the boot-state hypothesis: **FAULT** (IOB `…0200`). Given leg 23's ×5 innocence at this exact site, the likely ACTUAL is **SURVIVE** → the bisect INVERTS: the finding is the leg-24-vs-real-SMP-3 delta = the build FEATURE / image LAYOUT (`tegrasmp` vs `smpprobe`), the XCARVE layout-correlation through-line. |
| **25** | 25 | the SAME wake at the **PRE-xHCI-takeover** site — before `jb2b_attach`/JB9i eviction | early `SMPPROBE-7 ARMED sel=25 (PRE-xHCI-takeover site)` banner, then `… SEQUENCE DONE — 5/5` | If 24 FAULTED and 25 SURVIVES → **the xHCI takeover/eviction fabric state IS the trigger** (the wall is created by the takeover; a wake into the pre-takeover fabric is clean). If BOTH survive → the takeover-state axis is ALSO acquitted; the residual is the build-layout delta (a non-probe follow-up, e.g. a `tegrasmp`-layout relink experiment cf. XCARVE M1). |

## Reading the results (decision table)

- **24 faults (IOB `…0200`)** → boot-state context IS reproducible AND post-takeover. Proceed to 25 to
  test whether the takeover created it.
  - then **25 survives** → the xHCI takeover/eviction fabric state is the SMP-3 trigger. STOP; the fix
    arc targets the takeover-created fabric state (or wakes the secondaries BEFORE the takeover — a
    boot-ordering fix in `main.rs`).
  - then **25 also faults** → the trigger is present even pre-takeover; boot-state context is NOT the
    xHCI takeover. Narrow to what else differs pre-takeover (a follow-up).
- **24 survives** (the likely branch, given leg 23 ×5) → the bisect INVERTS. Leg 24 = the real
  SMP-3 code at the real SMP-3 position, yet it survives where the original `tegrasmp` run faulted.
  Every position/code variable is now exhausted; the enumerated residual is the build FEATURE / image
  LAYOUT (`smpprobe` vs `tegrasmp`), which the XCARVE arc proved decides fabric exposure. **The SMP-3
  fault is then attributable to image layout, not to any wake semantic** — the follow-up is a
  `tegrasmp`-image relink experiment (XCARVE M1 idiom), NOT another probe leg. Run leg 25 anyway to
  confirm the pre-takeover fabric is equally clean (it should be) and close the position axis.

## Schedule (one leg per boot; 3 boots)

1. Flash `UNAOS_SMPPROBE=23` (RIDER 1), boot, assert firmware precondition, confirm
   `RAPID REAL-ENTRY SEQUENCE DONE — 5/5` + `AP -> BSP SGI OK`.
2. Flash 24, boot, record: AP-online prints + `CORE_READY[i] SET` ×5 / `SEQUENCE DONE — 5/5`, or box
   down (DC-cut recovery). This is the control at the SMP-3 position.
3. Flash 25, boot (even if 24 faulted — 25 discriminates whether the takeover created the fault),
   record: the EARLY `ARMED sel=25 (PRE-xHCI-takeover site)` banner, then online ×5, or box down.
4. Record the git7 + tar sha of every boot; restore the DEFAULT image to the stick at close.

## Recovery

A RAS power-off leaves the box off; full DC cut (unplug barrel supply, wait ~10 s, replug) before the
next boot. Identical to the SMP-2/4/5/6 runbooks.

## Staged media (flash ONLY from `~/unaos-bench/flash/orin/`, never `target/`)

Two armed tars `UnaOS-orin-esp-smpprobe{24,25}-<UTCstamp>-<git7>.tar` (EFI + kernel.elf) + the knob-off
DEFAULT tar (byte-identity fallback). RIDER 1 uses the SMP-6-staged `smpprobe23` tar if still present;
if absent, rebuild leg 23 from this tree (the leg-23 code is unchanged — `run_real_entry_rapid` was
only parameterized by `sel`, leaving the sel-23 record grammar byte-for-byte identical). Shas in the
MANIFEST + the ORIN-SMP-7 landing report. Each armed image validates by its distinct ELF hash +
`strings | grep -a SMPPROBE-7` present + the LIVE `sel=<n>` echoed on the first probe serial line. The
default validates by hash + ZERO `SMPPROBE-7` strings (`tegra:` count 109).
