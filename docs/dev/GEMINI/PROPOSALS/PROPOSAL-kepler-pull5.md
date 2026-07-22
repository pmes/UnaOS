# PROPOSAL — Kepler pull 5: Derivation of GK107 Head Scanout and PBDMA Config

## Overview
This proposal details the cleanroom derivation of the correct register bases and initialization sequences required to resolve the two walls encountered in Pull 4:
1. **Wall 1: Head Scanout Base.** The PDISPLAY MMIO (0x610000+) previously used reads zeros because firmware-configured heads on GF119+ hold their state in a separate "armed vs assembly" model.
2. **Wall 2: PBDMA Status Poison.** The PBDMA status read returned `0xBAD0011F` due to using the legacy GF100 `PBDMA_STATUS` register (0x6c0) which does not apply to GK104+. 

## Derivation 1: Head Scanout Base (ARMED state)
**Source:** `envytools/rnndb/display/g80_pdisplay.xml`, `envytools/rnndb/display/nv_evo.xml`

On GF119+ architectures (which includes GK107), the EVO channel uses an assembly/pending state and an active/armed state. The assembly state is written via DMA or the EVO core channel (0x610400). The **ARMED** state (the actual live hardware state) is reflected in the `NV_HEAD_STAT` block.

1. **HEAD_STAT Base:** According to `g80_pdisplay.xml`, `HEAD_STAT` begins at offset `0x6000` from `NV_MMIO` (which is `0x610000` for PDISPLAY). For GK104+, it is an array with `stride="0x800"` and `length="4"`.
   Base: `0x610000 + 0x6000 = 0x616000`
2. **FB_SETTINGS Offset:** The EVO framebuffer settings are defined in `nv_evo.xml` under `G80_EVO_FB_SETTINGS`. The hardware mirrors this group inside the `HEAD_STAT` array at offset `0x100`. 
   Armed Base: `0x616100 + (head * 0x800)`
3. **Fields:**
   - `OFFSET_ORIGIN`: `0x616100` (contains the VRAM scanout address shifted by 8)
   - `SIZE`: `0x616108`
   - `STORAGE`: `0x61610C` (PITCH is bits 8-20 shifted right by 4)

## Derivation 2: PBDMA Base, Count, and Enables
**Source:** `envytools/rnndb/fifo/gf100_pfifo.xml`, `envytools/rnndb/bus/pmc.xml`

The legacy GF100 PBDMA status register (0x6c0) was removed in GK104. GK104+ moves PBDMAs (called `PSUBFIFO`) to a new offset and requires specific enablement.

1. **Register Base:** In `gf100_pfifo.xml`, the `PSUBFIFO` array for `GF100-` is at offset `0x40000` (relative to PFIFO base `0x2000`? No, relative to NV_MMIO base `0x0`). 
   - Base: `0x40000`
   - Stride: `0x2000`
   - Length: `3` (for the family)
2. **Unit Count (PBDMA_COUNT):** The number of valid PBDMAs on the specific chip (e.g., 1 for GK107) can be derived from the `SUBFIFO_ENABLE` register in the PMC block (`pmc.xml`). 
   - Offset: `0x204`
   - Writing `0xFFFFFFFF` and reading it back yields the bitmask of physically present PBDMAs. We will witness this count.
3. **PFIFO Subunit Enables & Clock Gating:** 
   - Enable PBDMAs via `PMC_SUBFIFO_ENABLE` (`0x204`).
   - Prevent the PFIFO from timing out on framebuffer fetches by configuring the `FB_TIMEOUT` register at PFIFO offset `0xa04` (`0x2000 + 0xa04 = 0x2a04`). According to `gf100_pfifo.xml`, this should be set to `0xbfffffff` for GK104 to disable the timeout/configure it appropriately.

## Proposed Changes
1. **kepler.rs (Wall 1):** Update the scanout loop to read the ARMED state at `0x616100 + (head * 0x800)`. Witness the `OFFSET_ORIGIN`, `SIZE`, and `STORAGE` values.
2. **kepler.rs (Wall 2):** 
   - Initialize PBDMAs by setting `0x000204` to the enabled mask.
   - Configure `FB_TIMEOUT` at `0x002a04` to `0xbfffffff`.
   - Update the PBDMA diagnostic witness to read from the correct `PSUBFIFO` base (`0x40000`), specifically checking `0x40108` (INTR) or `0x4002c` (ACTIVE_CYCLES) to confirm PBDMA clocking/status.
