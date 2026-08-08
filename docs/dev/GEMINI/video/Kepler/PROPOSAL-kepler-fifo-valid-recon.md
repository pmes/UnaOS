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

The `ENGINE_TRIGGER` (`0x409c08`) host handshake (Hypothesis 3) is actually NOT untried; `kepler.rs:1372-1380` already writes `1` to it (pull 35's H2/H3 arm) during the ucode-echo loop, which fires on every FIFO boot. Sitting #37 shows that this initial write succeeds (`img=A`), and no PMC reset intervenes before our validation logic at `:1576`.

However, sitting #35 showed that host pokes to CTXCTL registers took but built no state. The genuinely new variable we are testing is **PLACEMENT**: we will execute the write post-ucode, immediately pre-VALID. We also note that sitting #37 retired `NO_POLL`: writing `VALID` without `POLL_ENABLE` gives byte-identical `err=0x2`, so we must drop the `NO_POLL` framing.

We will add a write experiment immediately after the recon reads and before the `VALID` write at `:1576`.
1. **Justifying Read (Pre-Image):** We will read `ENGINE_TRIGGER` (`0x409c08`) via `fecs_read` to capture the handshake state before intervention.
2. **Write:** We will write `1` (the value pull 34/35 already writes) to `ENGINE_TRIGGER` (`0x409c08`) via `fecs_write` to complete the host handshake.
3. **Restoration:** Following the BCMA-S1 shape, we will read back the `ENGINE_TRIGGER` state to verify the write. On every exit path (whether the witness passes, strips, or wedges), we will restore `ENGINE_TRIGGER` to its exact pre-image value so that we leave a defined state (e.g. not leaving `1` latched through the runlist submit if it was `0`).

### Emitter Strings (The Prediction)

We predict that completing the host handshake at this new placement will satisfy PFIFO's validation gate. When the fence works (the validation succeeds):
- The readback at `:1525` (now the readback after the write) will retain the `VALID` bit: it will read `0xC0000000 | (inst_off >> 12)` (or similar, depending on the exact written instance offset).
- `pfifo_err` (`CHAN_TABLE_ERROR`) will remain `0x0` (quiet), rather than stripping to `0x00002000` with `err=0x2`.

**Null-Result Flag:** If `eng_trig_pre` reads `1`, our write of `1` is a no-op. The boot proves nothing about placement, and this shape will be logged as a null result.

**Refutation Shapes:**
If the write experiment fails (a refute):
- **Stripped:** The channel readback still strips `VALID` (`0x00002000`), and `pfifo_err` reports `0x2`.
- **Wedged (Made it worse):** `pfifo_err` is altered to an unhandled or poisoned state (`POISON`, `ABSENT`, or unnamed values). This will be distinctly printed by routing `err` through `classify_fecs_word`.

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
| Host Handshake | `engine_trigger` | `VALUE` | `ZERO` | | |
