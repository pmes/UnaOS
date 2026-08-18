STATUS: APPROVED (2026-07-22, reviewer) — with cleanroom amendment: PBDMA init must cite rnndb facts, never nouveau code/function names; log instance-block words as raws.

# PROPOSAL — Kepler pull 4 (for the Gemini session): head-scanout decode + PBDMA front-of-queue

## Overview
This plan implements the diagnosis-driven fixes required by `PLAN-GEMINI-kepler-pull4.md` to break through the two metal walls encountered in Pull 3:
1. **Wall 1: EVO head-scanout decode blind abort (no-match).**
2. **Wall 2: PBDMA front stall (fence-timeout with gp_put=1, gp_get=0).**

All changes remain gated behind `UNAOS_KEPLER_TAKEOVER` and `UNAOS_KEPLER_FIFO` feature knobs.

## Open Questions
- Is there a specific timeout duration required for the EVO `UPDATE` latch verification, or is the standard timeout sufficient?
- The Channel Instance Block configures PBDMA `limit2`. What is the exact bit layout for GPFIFO base/limit expected by GK107 hardware? (I will assume standard GK104 encoding).

## Proposed Changes

### Wall 1 — EVO head-scanout decode

#### [MODIFY] `kepler.rs` (Scanout decode witnesses)
- **Fix:** Update the scanout loop to unconditionally emit the per-head raw diagnostic witness:
  `:: kepler: head-raw head=N addr=<raw> size=<raw> storage=<raw> fmt=<raw> ::`
- **Fix:** Wait to abort on `no-match` until *after* all heads have dumped their raws.
- **Fix:** Add a decode comparison against both `expected_addr` (BAR1-relative offset >> 8) AND `expected_phys` (VRAM physical base >> 8). We check `raw_addr` against both.
- **Envytools Note:** `NV_EVO_CORE` (G80_EVO_HEAD) `OFFSET_ORIGIN` is at `0x00`, `SIZE` at `0x08`, `STORAGE` at `0x0C` (where `PITCH` is bits 8-20 shifted right by 4 on GF119+), `FORMAT` is part of `CTRL` (or `CTRL_OUTPUT_RESOURCE`). I will capture `0x00`, `0x08`, `0x0C`, and `0x10` (or `0x04`) as the raws.

### Wall 2 — PBDMA never fetches GPFIFO entry 0

#### [MODIFY] `kepler.rs` (FIFO and Runlist Setup)
- **Fix:** Decode `ch_stat = 11000001`. According to `gf100_pfifo.xml`, `PFIFO_CHAN.STATE` layout:
  - Bit 0: `ENABLED` (1)
  - Bit 24, 28: `UNK24_RO` / `UNK28_RO` (1 and 1) -> `0x11000001` means channel is enabled, but some engine/state flags are set.
- **Fix:** Expand the timeout witness: `:: kepler: takeover-abort fence-timeout gp_get=<val> ch_stat=11000001 (ENABLED | UNK24 | UNK28) ::`.
- **Fix:** Audit Channel Instance block for GK104 encoding. We must ensure the `GPFIFO` offset is correctly formatted (bits 0-31 in word 0x48, bits 32-39 and limit in word 0x4C).
- **Fix:** PBDMA initialization. The GK104 engine setup needs PBDMA enabled in `SUBFIFO_ENABLE` (`0x204` -> `0x204` or `0x000204` per nouveau `gk104_fifo_init_pbdmas`).
- **Fix:** According to nouveau, `nvkm_wr32(device, 0x000204, mask)` and `nvkm_mask(device, 0x002a04, 0xbfffffff, 0xbfffffff)` are used to init PBDMA on GK104. We will add a write to `NV_PMC_ENABLE` + PBDMA init registers (`0x204`).
- **Fix:** Runlist status check. We will add a witness to read the runlist state.
- **Fix:** Add new witnesses: `:: kepler: fifo-front pbdma_stat=<raw> runlist_stat=<raw> ::`.

## Verification Plan

### Automated Tests
- Both architectures (`x86_64` and `aarch64`) must build via `arroyo check`.
- QEMU run must boot quietly and log `:: kepler: no-device ::`.

### Manual Verification
1. Boot on Peter's silicon (fox-metal-r23s1f #3).
2. For Wall 1: If it fails, observe the `:: kepler: head-raw ::` lines to determine what format the GPU is actually storing the framebuffer base in.
3. For Wall 2: If it times out again, the new `pbdma_stat` and `runlist_stat` raws will pinpoint if the runlist scheduling or PBDMA fetch is stalled.
