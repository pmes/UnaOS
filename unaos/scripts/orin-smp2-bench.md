# ORIN-SMP-2 bench runbook — the JM5 `CPU_ON` firmware-wall INVESTIGATION (attended; A-B-A)

This is an **investigation**, not a verification. JM5 proved the PSCI/GICv3 SMP mechanism correct on
QEMU `virt` (3/3 secondaries), but the FIRST `CPU_ON` on real Orin raises a fatal Tegra CBB-fabric
RAS Uncorrectable Error inside BL31/MCE and powers the box off, while every PSCI *query*
(`AFFINITY_INFO`) works on silicon (§JM5-result). This runbook drives the `UNAOS_SMPPROBE` probe
(`arch/aarch64/smpprobe.rs`, §ORIN-SMP-2) to discriminate the four ranked hypotheses.

**Two hard rules for this bench:**
1. **RIDER (a) — predictions are pre-registered.** The prediction table below is written BEFORE any
   boot. A boot whose outcome was not pre-registered is spent evidence. Do NOT improvise a new
   experiment on-metal — if a result is surprising, record it and STOP.
2. **RIDER (b) — probe-only.** Every experiment is read/query/one-shot-volatile. NONE writes fuses or
   any persistent firmware state. The `CPU_ON` experiments (3, 5) power a core (what JetPack does every
   boot) and write nothing persistent. **Power-fault boots are DATA, not failures.**

## Pre-registered prediction table (RIDER (a) — verbatim, matches §ORIN-SMP-2)

| knob | hypothesis / role | `CPU_ON`? | predicted serial record | predicted box behavior |
|---|---|---|---|---|
| **0** | CONTROL — `AFFINITY_INFO` sweep (Aff2 0–3 × Aff1 0–3) + redistributor walk | no | `sel=0 slot aff=… AFFINITY_INFO=<0/1/2 or −>`; fused cores valid, unpopulated slots −INVALID_PARAMS; `present=<k>` | clean boot → CAPSTONE |
| **1** | **H1** MCE/BPMP — PSCI capability census | no | `PSCI_VERSION`; `FEATURES(CPU_ON)=<r>`; `FEATURES(AFFINITY_INFO)`; `MIGRATE_INFO_TYPE` | clean boot → CAPSTONE. `CPU_ON` advertised yet faulting ⇒ fault is in Tegra's `CPU_ON` impl |
| **2** | **H2** latent RAS — read error records before any `CPU_ON` | no | `ID_AA64PFR0.RAS`; per record `ERXSTATUS … V=<b> UE=<b> ERXADDR ERXMISC0` | clean boot → CAPSTONE. A pre-existing `V=1`/`UE=1` supports H2 |
| **3** | **H3** entry-high — `CPU_ON`, LOW (2 GiB) sentinel entry | **yes** | `sel=3 target aff=… entry=0x80000000 … issuing CPU_ON` then RAS (no more) or `RETURNED ret=<r> — SURVIVED` | **RAS + power-OFF** if H3 false; SURVIVAL ⇒ H3 candidate |
| **4** | **H4** caller-EL — **BLOCKED-BY-DESIGN** | no | `sel=4 exp=el1-caller BLOCKED-BY-DESIGN` + reason | clean boot → CAPSTONE |
| **5** | **H3 reference / JM5-wall reproduction** — `CPU_ON`, HIGH (~9.5 GiB kernel) entry | **yes** | `sel=5 target aff=… entry=0x25e……(HIGH) … issuing CPU_ON` then RAS (no more) or `RETURNED — SURVIVED` | **RAS + power-OFF** (the wall). exp3 vs exp5 differ ONLY in entry PA: same fault ⇒ H3 refuted |

**Reading the results (decision table):**
- exp3 and exp5 both **RAS-fault identically** → H3 refuted; the fault precedes the woken core's
  fetch (H1/H2 territory). This is the expected outcome.
- exp3 **survives** while exp5 faults → H3 candidate (entry-PA-sensitive); warrants a proper
  low-trampoline follow-up arc.
- exp1 shows `FEATURES(CPU_ON) ≥ 0` (advertised) → BL31 recognizes the call; the fault is inside its
  Tegra `CPU_ON`/MCE path, consistent with H1's "needs MCE/BPMP coordination".
- exp2 shows a valid/uncorrectable record BEFORE any `CPU_ON` → H2 (pre-poisoned RAS) supported;
  all-clean → H2 weakened.

## 0. Build + stage the probe images (Peter builds; flash ONLY from the staged tar)

Each armed value is a **distinct image** (`option_env!("UNAOS_SMPPROBE")` const), so rebuild+restage
per experiment. Build the tegra ESP LAST (any `test-arm` clobbers `target/aarch64_esp`), validate by
COUNT not size:
```
UNAOS_TEGRA=1 UNAOS_SMPPROBE=<n> ./arroyo esp-jetson
strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'    # must be 142 for any armed value
```
**⛔ Flash ONLY from the staged artifact, never from `target/`** (`~/unaos-bench/flash/README.md`).
This arc pre-stages all six images (one per experiment) as
`~/unaos-bench/flash/orin/UnaOS-orin-esp-smpprobe<n>-<UTCstampZ>-<git7>.tar`, each with a `MANIFEST`
sha256 line noting its `sel` + `tegra:142`. **Verify the sha256** before flashing, untar onto the
boot stick, `dot_clean`, eject. (The paths + shas are in the ORIN-SMP-2 landing report.)

**Serial-verify before trusting any boot:** the first probe line is
`:: tegra: SMPPROBE ARMED sel=<n> … ::` — CONFIRM `<n>` matches the image you flashed. A mismatch
means a stale build; rebuild that image.

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ Serial (JD11 output-mirror) is the **only** evidence channel here — a `CPU_ON` boot may power the
box off, so the record must already be captured when it does. CONFIRM the bridge is logging a full
boot to `~/unaos-bench/` BEFORE spending a boot. The probe re-enumerates across power-cycles (the
bridge auto-reopens); confirm the dated log GROWS on each boot. If it froze mid-bench (§JB1f hazard),
the boot's evidence does NOT count — re-power and re-verify capture.

## 2. The A-B-A schedule — same silicon, same card, swap only the kernel image, minutes apart

Run in this order. Boots 1, 3, 5, 7, 9 are the **A control** (knob 0) re-run around each experiment so
a power-fault is provably the experiment's, not drift. `awk`, never plain `grep`, on the serial log
(control bytes). Grammar: `awk '/SMPPROBE/' ~/unaos-bench/jetson-serial-<date>.log`.

| boot | image | expect | after |
|---|---|---|---|
| 1 | `smpprobe0` (A) | control sweep → CAPSTONE; box stays up | note the enumerated affinities + `present=k` |
| 2 | `smpprobe1` (H1) | census lines → CAPSTONE; box stays up | record `FEATURES(CPU_ON)`, `MIGRATE_INFO_TYPE` |
| 3 | `smpprobe0` (A) | control again → CAPSTONE | confirms boot 2 left the box healthy |
| 4 | `smpprobe2` (H2) | RAS records → CAPSTONE; box stays up | record any `V=1`/`UE=1` BEFORE `CPU_ON` |
| 5 | `smpprobe0` (A) | control → CAPSTONE | health check |
| 6 | `smpprobe5` (H3-ref / wall) | `issuing CPU_ON` … **RAS + power-OFF** | the box powers off — EXPECTED. Recover (§3) |
| 7 | `smpprobe0` (A) | control → CAPSTONE | proves the box recovers cleanly after the fault |
| 8 | `smpprobe3` (H3) | `issuing CPU_ON` (LOW entry) … RAS + power-OFF **or** SURVIVED | compare the RAS `ADDR`/`IERR` to boot 6 |
| 9 | `smpprobe0` (A) | control → CAPSTONE | final health check |
| — | `smpprobe4` (H4) | BLOCKED record → CAPSTONE (optional; documents the block) | no `CPU_ON` |

Per boot, record: the `SMPPROBE ARMED sel=<n>` line (verify it matches), every `sel=<n>` record line,
whether the box stayed up or powered off, and the serial log filename.

## 3. Recovery after a power-fault boot (boots 6, 8 if it faults)

The RAS Uncorrectable Error powers the box off (0 post-fault heartbeats — the JM5 syndrome). This is
the pre-registered outcome, not a bench failure. To recover:
1. **Full re-power** the Orin (cut the DC barrel supply, wait ~5 s, restore) — a soft reset is not
   enough after a CBB-fabric RAS.
2. Re-seat nothing else; keep the SAME card + serial rig (A-B-A needs same silicon/same card).
3. **Re-verify serial capture** (§1) — the bridge re-enumerates; confirm the dated log is growing on
   the recovery boot BEFORE it counts as evidence.
4. Flash the next image (the following A control) and continue the schedule.

## 4. Verdict + hand-off

Fill the decision table (§ prediction table "Reading the results"). Headline questions to answer:
- Does `CPU_ON` fault regardless of entry PA (exp3 ≡ exp5)? → H3 refuted, fault precedes fetch.
- Does BL31 advertise `CPU_ON` (exp1)? → H1 (needs Tegra MCE/BPMP path) vs unrecognized call.
- Any pre-existing RAS record (exp2)? → H2 supported/weakened.
- H4 is BLOCKED-BY-DESIGN (recorded, not benched).

Note each experiment's serial transcript filename (`~/unaos-bench/jetson-serial-<date>.log`) and the
exact RAS lines (`ERROR: RAS Uncorrectable Error …`, `ADDR = …`, `IERR = …`) from any fault boot —
run-to-run `ADDR` variation is itself H2 evidence. Feed the verdict to the next-arc decision (a fix
arc, or the NVIDIA-collaboration angle §JM5-result names). ⚠ `dot_clean` the boot stick.

---
## ✅ BENCH VERDICT (2026-07-15 attended; serial `~/unaos-bench/jetson-serial-2026-07-15-smp2bench.log`)

**7 boots (schedule 1–7), 7 CAPSTONEs, 0 RAS faults, 0 power-offs. THE WALL DID NOT REPRODUCE.**

| boot | image | outcome |
|---|---|---|
| 1 | A (sel=0) | PASS — sweep: 8 GICR frames, `present=12` (3 clusters ALL answer OFF on a 6-core part; Aff2=3 −INVALID) |
| 2 | H1 (sel=1) | PASS — PSCI 1.1; `FEATURES(CPU_ON)=0` ADVERTISED; `MIGRATE_INFO_TYPE=2` |
| 3 | A | PASS — `present=12` identical |
| 4 | H2 (sel=2) | PASS — RAS implemented, 2 records, BOTH clean (`V=0 UE=0`) → H2 weakened |
| 5 | A | PASS |
| 6 | **wall (sel=5)** | **SURPRISE — `CPU_ON RETURNED ret=0 — SURVIVED`** (aff `0x00000100`, HIGH entry `0x25b42135c`); boot continued to CAPSTONE + shell |
| 7 | A | PASS — box healthy post-CPU_ON; bracket closed |

Boot 8 (exp3) **SKIPPED** under the pre-registered surprise-STOP rule — exp5's survival mooted the
entry-PA discrimination. **Verdict: the JM5 `CPU_ON` wall is firmware-era, fixed upstream — NOT
reproducible on UEFI `t23x_general 39.2.0-gcid-45755727 (2026-06-01)`.** Consequences: (a) the
born-fixed `smp_virt` bring-up is UNBLOCKED on Orin silicon — the 6-core bring-up is a proper next
arc; (b) presence MUST gate on the DTB `/cpus` (6 real cores), NOT `AFFINITY_INFO` (answers OFF for
fuse-disabled slots — 12 "valid" on a 6-core part) and NOT the GICR walk alone (8 frames); (c) the
UEFI firmware build line is a recorded PRECONDITION of every future SMP bench.
