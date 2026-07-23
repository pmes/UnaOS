STATUS: APPROVED (2026-07-22 — reviewer verified every rnndb citation against
envytools master: PSUBFIFO 0x40000/stride 0x2000/len 3 ✓, SUBFIFO_ENG_MASK
0x390 len 3 GK104- ✓, +0x108=INTR ✓, +0x120=CH with GK104 CHID 0-11/ACTIVE
bit 13 ✓, IB_PUT +0x00 / IB_GET +0x14 ✓, PMC.ENABLE bit 8 PFIFO ✓,
PMC.SUBFIFO_ENABLE 0x204 ✓, PFIFO-internal 0x2200/0x2204 GF100:GK104-only ✓.
The 40108=INTR reframe — "bad-read zero" was a naturally-zero interrupt
register, not a wrong base — is the best kind of wall demolition: the base was
right all along and we were reading the wrong register. Proceed exactly as
written; instrumentation commit first, sitting #6 falsifies.)

# PROPOSAL — Kepler wall-2 (PFIFO/PBDMA fence), pull 7 arc intro

## 1. GK107 PBDMA register base and per-unit stride
According to the rnndb facts, PBDMA units (referred to as `PSUBFIFO` in the XML) live at base `0x40000` with a per-unit stride of `0x2000`. 
Citation: `envytools/rnndb/fifo/gf100_pfifo.xml:308`: `<array name="PSUBFIFO" offset="0x40000" stride="0x2000" length="3" variants="GF100-">`.
This means PBDMA 0 is at `0x40000`, PBDMA 1 at `0x42000`, and PBDMA 2 at `0x44000`.

The count of 3 is real. It is reported by `NV_PMC_SUBFIFO_ENABLE` (`0x204` in PMC). 
Citation: `envytools/rnndb/bus/pmc.xml:193`: `<reg32 offset="0x204" name="SUBFIFO_ENABLE" variants="GF100-">` with bits 0, 1, and 2 defined. The mask `0x7` indicates 3 present units on this silicon, explaining why the count register decode evaluates to 3.

## 2. Unit start/clock sequence
The PBDMA unit enablement relies on PMC. On GK104+, the PFIFO-internal enables (`0x2200` `CTRL.ENABLE` and `0x2204` `SUBFIFO_ENABLE`) were removed (marked `variants="GF100:GK104"` in `gf100_pfifo.xml:42,46`). 

The correct start sequence is strictly within PMC, which must be executed BEFORE runlist submission:
1. **PMC engine-enable for PFIFO**: Write bit 8 (`PFIFO`) to `PMC.ENABLE` (`0x200`). Citation: `envytools/rnndb/bus/pmc.xml:143`.
2. **Per-PBDMA (SUBFIFO) enable**: Write `0x7` (bits 0, 1, 2) or `0xFFFFFFFF` to `PMC.SUBFIFO_ENABLE` (`0x204`) to clock/enable the PBDMAs. Citation: `envytools/rnndb/bus/pmc.xml:193`.

*(Note: The current code in `kepler.rs` correctly performs both of these writes, so the units are likely clocked correctly, pointing to the binding or instrumentation as the culprit for zero reads).*

## 3. Runlist↔PBDMA binding
The binding that determines which PBDMA services a runlist is controlled by the `SUBFIFO_ENG_MASK` array in PFIFO.
Citation: `envytools/rnndb/fifo/gf100_pfifo.xml:104`: `<reg32 offset="0x390" name="SUBFIFO_ENG_MASK" length="3" variants="GK104-">`.
This is an array of 3 masks (one for each PBDMA). To bind the PGRAPH engine (Engine `0`, `gf100_pfifo.xml:14`) to PBDMA 0, we write `1 << 0` to `0x2390`. To bind it to PBDMA 1, we would write `1 << 0` to `0x2394`. We can read these masks back to prove which unit is serving the PGRAPH runlist.

## 4. Instrumentation-first milestone
The prior "wall" assumption that PBDMA reads clean zero is based on reading `0x40108`. However, `0x108` in `PSUBFIFO` is the `INTR` (Interrupt) register, which naturally reads zero unless an interrupt is pending. Citation: `envytools/rnndb/fifo/gf100_pfifo.xml:483`. The legacy GF100 `PBDMA_STATUS` (`0x26c0`) does not exist on Kepler.

To properly falsify the base/clock hypothesis and prove we're watching the right unit, pull 7's first commit will instrument ALL 3 PBDMAs (`0x40000`, `0x42000`, `0x44000`), reading:
- `CH` register at `+0x120`: Shows the currently active channel and `ACTIVE` bit (bit 13 on GK104). Citation: `envytools/rnndb/fifo/gf100_pfifo.xml:490`.
- `IB_PUT` at `+0x00` and `IB_GET` at `+0x14`.
- The `SUBFIFO_ENG_MASK` array at `0x2390`, `0x2394`, and `0x2398` (to confirm binding).
- `PMC.ENABLE` (`0x200`) and `PMC.SUBFIFO_ENABLE` (`0x204`) (to prove clock state).

### Citation cleanup note
The remaining nouveau attribution near line 465 in `kepler.rs` will be removed/replaced with an empirical note as requested in the standing rules.
