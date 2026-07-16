# ORIN-SMP-8 bench runbook — the tegrasmp RELINK (attended; the layout-axis close-out)

ORIN-SMP-7's attended sitting (2026-07-16) exonerated the wake POSITION axis end-to-end: leg 24
(post-xHCI-takeover) and leg 25 (pre-xHCI-takeover) both put 5/5 real-path cores online, and with
legs 21–25 all innocent, **the SMP-3 residual trigger reduces to IMAGE LAYOUT** — the XCARVE
through-line (three distinct layouts of one leg-23 knob-set sampled 4/4, ~50%, 0/4 carveout-fault
rates, the fault ADDRESS itself moving with layout). Every code-shape, concurrency, conjunction, and
position suspect is acquitted on silicon. This arc tests the last axis DIRECTLY on the real
`UNAOS_TEGRASMP=1` image — the exact build that RAS-faulted 2/2 on 2026-07-15 — by rebuilding it with
the inert relink pad composed in (`UNAOS_XCARVE_RELINK=1`), which shifts the whole image +0x4000 with
ZERO semantic change.

See `arch/aarch64/xusb_tegra.rs` (`XCARVE_RELINK_PAD`) + arch_arm64.md §ORIN-SMP-8 for the compose
evidence and layout deltas.

## The two images (one variable = image LAYOUT; the SMP code is byte-for-byte identical)

Both images carry the IDENTICAL real `start_secondaries_tegra` 6-core kick-off (`UNAOS_TEGRASMP=1`).
They differ ONLY by the presence of the inert `.xcarve_relink_pad` (16 KiB of `0xA5`, `#[used]`,
never read), which relocates every downstream section by +0x4000:

- **tegrasmp-original (`UNAOS_TEGRASMP=1`)** — the 2026-07-15 fault-repro CONTROL. This is the exact
  SMP-3 configuration that RAS-faulted 2/2 (IOB `SERR=0x12` / CBB-`0x6` / ADDR
  `0x8000000000000200`) before the first `CPU_ON` result printed. **EXPECTED to fault** — it is the
  historical faulter, staged so the wall's reproducibility is re-confirmed on tonight's silicon.
- **tegrasmp-relinked (`UNAOS_TEGRASMP=1 UNAOS_XCARVE_RELINK=1`)** — the SAME SMP kick-off at a
  shifted layout (`.text 0x2c000 → 0x30000`, `.bss 0xd1000 → 0xd5000`, whole image +0x4000; the pad
  sits between `.rodata` and `.text`). The layout-axis test image.

## Hard rules for this bench

1. **RIDER 1 — a real-entry rapid re-confirm anchors the sitting.** If a leg-23 (or leg-16) armed tar
   is still staged, boot it FIRST and confirm `RAPID REAL-ENTRY SEQUENCE DONE — 5/5` (or leg-16
   `CHECKPOINT REACHED (0x53040010)`) — proves the real bring-up still goes online on tonight's
   firmware + silicon before the tegrasmp images run. (Optional if the operator prefers to spend the
   boots on the two SMP-8 images directly; the tegrasmp-relinked clean boot is itself a real-path ×5
   witness.)
2. **RIDER 2 — predictions are pre-registered (the table below, written BEFORE any boot); exactly ONE
   variable across the pair (image layout).** A result that contradicts a pre-registered prediction
   NAMES the finding and is recorded exactly — both the CONFIRM and the REFUTE branch are decisive
   (see the two-signature discrimination below).
3. **RIDER 3 — power-fault boots are DATA.** Recover with a FULL DC CUT (unplug the barrel supply,
   wait ~10 s, replug — a warm reset can leave the CBB/MCE poisoned) and continue per the schedule.
4. **RIDER 4 — DISTINGUISH THE TWO FAULT SIGNATURES (this is the crux of this sitting).** Two
   independent walls can take a boot on these images; the runbook demands the operator read the RAS
   ADDR to tell them apart:
   - **SMP-3 fault (the axis under test):** IOB `SERR=0x12` / CBB-`0x6`, ADDR ending
     **`…0200`** (`0x8000000000000200`), fired from the `start_secondaries_tegra` kick-off BEFORE the
     first `CPU_ON` result prints. This is the signature that MATTERS for SMP-8.
   - **SNOC-Carveout / xHCI-takeover wall (independent, unrelated to SMP):** RAS SNOC `SERR=0xd`
     "Illegal address" + IERR Carveout Uncorrectable `0x3`, paired ACI `SERR=0x4` / FillWrite `0x9`,
     ADDR ending **`…7767dcXX`** (`0x800000027767dc40` / `…dc80`), fired at the xHCI `JB9i`
     inherited-slot eviction (`DISABLE_SLOT 1..8 issued + drained` is the last line before it). This
     wall may independently take ANY boot pre-probe; it is WALL DATA, not SMP data — **retry** (a full
     DC cut, re-boot the same image). The relinked image is a fresh layout and may sample this wall at
     any rate; only the `…0200` signature (or its clean absence through the kick-off) answers SMP-8.
5. **RIDER 5 — DTB-only presence.** The real kick-off enumerates its targets from the DTB `/cpus`
   list; every computed address is printed BSP-side before the first `CPU_ON` on both images.

## Firmware precondition (assert BEFORE any boot)

The first serial lines must show UEFI `t23x_general 39.2.0-gcid-45755727` (or newer,
Peter-acknowledged). A downgraded/different firmware = **STOP**.

## The evidence channels

- **Clean SMP path (either image):** `:: AARCH64 SMP: ORIN-SMP-3 …` plan lines, then per-core
  `CPU_ON AP <n> … ret=0`, five `:: AARCH64 SMP: AP <n> online … ::`, the AP→BSP SGI, then the BSP
  proceeds to the JM6 EL1 drop + CAPSTONE 6/6 with the woken APs parked in WFI.
- **SMP-3 fault:** box RAS power-off with ADDR **`…0200`** BEFORE the first `CPU_ON` result printed
  (the panel freezes at whatever was last scanned out — a RAS power-off presents as a "lockup").
- **Carveout wall:** box RAS power-off with ADDR **`…7767dcXX`** right after
  `JB9i — inherited-slot eviction: DISABLE_SLOT 1..8 issued + drained` — this is the OTHER wall;
  retry the same image.

## Pre-registered prediction table (RIDER 2 — verbatim; written BEFORE any boot)

| image | knobs | the ONE variable | predicted serial (SMP path) | predicted box behavior | what it MEANS |
|---|---|---|---|---|---|
| **tegrasmp-relinked** | `UNAOS_TEGRASMP=1 UNAOS_XCARVE_RELINK=1` | shifted layout (+0x4000) | full `ORIN-SMP-3` plan → 5 `AP <n> online` → CAPSTONE 6/6, panel live | **SURVIVE the kick-off** ×2–3 (no `…0200` fault) | **SMP-3 trigger = LAYOUT, CONFIRMED.** The production SMP path is CODE-COMPLETE; the SMP arc closes pending the carveout wall's real fix. A `…0200` fault here instead **REFUTES layout for this image** — the first refutation of the layout axis, equally decisive: back to one anomaly, the SMP-3 trigger is something the relink did not perturb. Record exactly. |
| **tegrasmp-original** (optional Boot B) | `UNAOS_TEGRASMP=1` | the historical fault build | may fault before `CPU_ON` | **FAULT** (`…0200`) — the historical SMP-3 signature | Control that the wall still reproduces on tonight's silicon at the original layout. A SURVIVE here is also data (the fault was never deterministic 2/2 → it was a sample of a probabilistic layout-modulated wall, exactly the XCARVE pattern). |

Note both images can ALSO sample the carveout wall (`…7767dcXX` at JB9i) pre-probe on any boot —
that is wall data (RIDER 4), retry; only the `…0200` signature at the kick-off answers SMP-8.

## Reading the results (decision table)

- **tegrasmp-relinked SURVIVES the kick-off (×2–3, no `…0200`)** → **layout CONFIRMED as the SMP-3
  trigger.** Every axis is now closed: entry shape, concurrency, conjunction, position, layout. The
  production 6-core SMP path is code-complete; SMP-3 is not a wake-semantics bug but the same
  layout-modulated fabric exposure the carveout wall exhibits. SMP arc CLOSES pending the wall fix.
  STOP the SMP investigation; the remaining work is the (separately-tracked) carveout-wall fix.
- **tegrasmp-relinked FAULTS with `…0200` at the kick-off** → **layout REFUTED for this image** (first
  such refutation). The SMP-3 trigger survives a +0x4000 relink, so it is not purely layout — record
  the exact ADDR, boot count, and firmware, and re-open the residual (the follow-up is a proposal, not
  a probe leg spawned here). STOP and report.
- **tegrasmp-relinked FAULTS with `…7767dcXX` at JB9i** → that is the carveout wall, NOT SMP-3
  (RIDER 4). Retry the image (full DC cut). It does not answer SMP-8 either way.

## Schedule (2–3 boots; can ride ANY future Orin window)

1. (RIDER 1, optional) Boot a staged leg-23/leg-16 armed tar; confirm real-path ×5 / checkpoint.
2. Boot **tegrasmp-relinked** ×2–3. Read the RAS ADDR on any fault (`…0200` = SMP-3 answer;
   `…7767dcXX` = wall, retry). Record: clean SMP path + CAPSTONE, or the fault signature.
3. (optional) Boot **tegrasmp-original** ×1 as the historical-fault control. Record signature.
4. Record the git7 + tar sha of every boot; restore the metal-validated DEFAULT (`cad623af…`,
   `d3ecf48` era) to the stick at close — per the standing rule, NEITHER SMP-8 layout is a
   stick-default candidate (no clean multi-boot metal record; both are bench-only).

## Recovery

A RAS power-off leaves the box off; full DC cut (unplug barrel supply, wait ~10 s, replug) before the
next boot. Identical to the SMP-2/4/5/6/7 + XCARVE runbooks.

## Staged media (flash ONLY from `~/unaos-bench/flash/orin/`, never `target/`)

- `UnaOS-orin-esp-tegrasmp-relinked-<UTCstamp>-<git7>.tar` — the layout-axis test image
  (`UNAOS_TEGRASMP=1 UNAOS_XCARVE_RELINK=1`).
- `UnaOS-orin-esp-tegrasmp-original-<UTCstamp>-<git7>.tar` — the 2026-07-15 fault-repro CONTROL
  (`UNAOS_TEGRASMP=1`); **EXPECTED to RAS at the SMP kick-off** (`…0200`), or to sample the carveout
  wall (`…7767dcXX`) pre-probe — both are pre-registered.
- the knob-off DEFAULT tar (byte-identity fallback / end-restore reference) — hash only; **NOT a
  stick-default candidate from this arc** (standing rule: defaults need a clean multi-boot metal
  record; the standing default stays `cad623af…` / `d3ecf48`).

Shas in the MANIFEST + the ORIN-SMP-8 landing report. Each tegrasmp image validates by its distinct
ELF hash + `strings | grep -a "AARCH64 SMP: ORIN-SMP-3"` present; the relinked image additionally by
`llvm-objdump -h kernel.elf | grep xcarve_relink_pad` present (`.text` VMA `0x30000`), the original by
its ABSENCE (`.text` VMA `0x2c000`). The default validates by hash + ZERO `AARCH64 SMP: ORIN-SMP-3`
strings (`tegra:` count 109).
