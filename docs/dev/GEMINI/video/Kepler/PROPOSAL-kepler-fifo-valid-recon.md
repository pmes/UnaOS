# PROPOSAL: PFIFO Validate-Strip Reconnaissance

## Goal
Identify the exact unmet hardware precondition causing PFIFO to strip the `VALID` bit upon channel activation (at `kepler.rs:1512`), resulting in `err=0x2` (`CHAN_TABLE_ERROR` = NO_POLL). We will dump the surrounding PFIFO/channel state right before the `VALID` write to build a truth table of the hardware's view.

## 1. The Preconditions & Witnesses
A CCSR channel validation requires multiple moving parts to be correctly aligned and acknowledged by PFIFO. We will dump these as classified witnesses (using the `classify_fecs_word` alphabet: `VALUE`, `ZERO`, `POISON`, `ABSENT`).

1. **Instance Block & GPFIFO**
   - **Precondition**: The memory containing the channel instance and GPFIFO must remain allocated, intact, and visible to the host.
   - **Witnesses**: Read the intactness magic of the instance block (`bar1 + inst_off + 0x10`, written at `kepler.rs:839` as `0x0000face`) and the GPFIFO address (`bar1 + inst_off + 0x48`, per `gf100_pfifo.xml` offset `0x48`).

2. **Global Engine State (PMC & PFIFO)**
   - **Precondition**: The PFIFO unit and its sub-units (PBDMA) must be globally enabled and clocked. The engine mask must bind the PBDMA to an active engine (PGRAPH).
   - **Witnesses**:
     - `PMC_ENABLE` (`0x200` per `pmc.xml`)
     - `PMC_SUBFIFO_ENABLE` (`0x204` per `envytools/rnndb/bus/pmc.xml:193` with `variants="GF100-"`, written by `kepler.rs:797`. Note: `0x2204` is `gf100_pfifo.xml:42,46` `SUBFIFO_ENABLE` which has `variants="GF100:GK104"`, so it was removed on GK104+ and does not exist on our GK107.)
     - `SUBFIFO_ENG_MASK` (`0x2000 + 0x390` per `gf100_pfifo.xml`)

3. **Runlist / Playlist State**
   - **Precondition**: Though a channel can theoretically be validated without a runlist, PFIFO's scheduler state might influence the `NO_POLL` error. Note that these are read *before* the runlist is submitted at `:1729`, so their expected healthy state here is pre-submit (zero).
   - **Witnesses**:
     - `RUNLIST_BASE` (`0x2270` per `gf100_pfifo.xml`)
     - `PLAYLIST_RD` (`0x2280` per `gpu_spec.md §2.4.1` where it was proven on metal to be `0x2013` *after* submission. Note: `0x2284` was dropped because bit 20 is set 8/8 boots, so its `ZERO` arm cannot fire.)

4. **PFIFO Interrupts and Errors**
   - **Precondition**: PFIFO must not be in a fault state prior to the channel validation attempt.
   - **Witnesses**:
     - `PFIFO_INTR_0` (`0x2100` per `gf100_pfifo.xml`)
     - `CHAN_TABLE_ERROR` (`0x252c` per `gf100_pfifo.xml`)
     - `SCHED_STATUS` (`0x263c` per `gf100_pfifo.xml`)

## 2. Emitter Strings (The Prediction)
We will insert the following read-only block immediately before line 1512.

```text
:: kepler: recon inst_base_mem={:08X}({}) gpfifo_ptr={:08X}({}) ::
:: kepler: recon pmc_en={:08X}({}) subfifo_en={:08X}({}) eng_mask={:08X}({}) ::
:: kepler: recon playlist_base={:08X}({}) playlist_rd={:08X}({}) ::
:: kepler: recon pfifo_intr={:08X}({}) pfifo_err={:08X}({}) sched_stat={:08X}({}) ::
```

Every `{}` classification will state the raw class (`VALUE`, `ZERO`, `POISON`, `ABSENT`) and append an explicitly stated refutation or healthy value exactly matching the deliverable table (e.g., `ZERO,refutes-active`, `VALUE,alive`, `VALUE,NO_POLL`).

## 3. Write Experiment
**None proposed at this stage.**
Per the brief, we perform *at most one* write experiment *only if* the recon names a specific missing precondition. Since we do not yet know which precondition is unmet, we will execute this read-only recon first to collect the missing data. The output will yield the Deliverable table.

## 4. Deliverable Recon Table
The following table maps the falsifiable outcomes of our recon to the hardware precondition they refute. The seat will fill in the `Observed` column from the metal log.

| Precondition | Witness | Expected | Refutation State | Observed | Conclusion |
|--------------|---------|----------|------------------|----------|------------|
| Memory Intact | `inst_base_mem` | `VALUE` | `POISON` / `ZERO` | | |
| Memory Intact | `gpfifo_ptr` | `VALUE` | `POISON` / `ZERO` | | |
| PFIFO Clocked | `pmc_en` | `VALUE` (nonzero) | `ZERO` | | |
| PBDMA Clocked | `subfifo_en` | `VALUE` (nonzero) | `ZERO` | | |
| Engine Bound | `eng_mask` | `VALUE` (nonzero) | `ZERO` | | |
| Runlist Submitting | `playlist_base` | `ZERO` (pre-submit) | `VALUE` | | |
| Runlist Submitting | `playlist_rd` | `ZERO` (pre-submit) | `VALUE` | | |
| No Faults | `pfifo_intr` | `ZERO` | `VALUE` (nonzero) | | |
| No Faults | `pfifo_err` | `ZERO` | `VALUE` (nonzero) | | |
| Sched Healthy | `sched_stat` | `VALUE` (nonzero) | `ZERO` | | |

Whichever precondition lands in a Refutation State identifies the exact gate blocking `VALID`.
