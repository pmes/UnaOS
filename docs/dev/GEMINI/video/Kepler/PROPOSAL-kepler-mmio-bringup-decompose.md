# PROPOSAL: Decompose mmio_bringup (331 ms)

## Goal
The `mmio_bringup` phase takes 331 ms, which is 84% of the `kepler` boot block (396 ms). We need to decompose this block into sub-phases to identify the exact source of this delay, using the same `phase!` macro instrumentation shape. 

## Falsifiable Predictions

Currently, the `mmio_bringup` phase spans from the start of `kepler::init` up to `mirror_hdr pass 1 done`. This includes:
1. Basic chip identification and VRAM detection (`pmc_and_vram_init`).
2. `kepler_display::takeover_display`, which draws a 29MB surface to BAR1 linearly pixel-by-pixel (`kdisp_takeover`).
3. PFIFO, PBDMA, and channel instance allocation/zeroing (5 pages = 5120 `write_volatile` calls to BAR1) (`pfifo_alloc_zero`).
4. `mirror_hdr pass 0` readback (`mirror_pass0`).
5. Beacon planting (`plant_beacons`).
6. `mirror_hdr pass 1` readback (`mirror_pass1`).

**Prediction**: 
The `takeover_display` call (`kdisp_takeover`) is responsible for ~315-325 ms of the 331 ms delay. 
*Why?* The function iterates over 1800 rows of 16384 bytes, performing 32-bit `write_volatile` stores. That equals ~7.37 million PCIe writes. Even if Write-Combining (WC) is successfully gathering these into 64-byte PCIe transactions, the sheer volume of MMIO stores executed by the CPU will dominate this block. 
The 5120 zeroing writes in PFIFO setup will likely account for ~1-2 ms. The mirror reads and beacon writes will account for < 1 ms.

## Proposed Changes

We will replace the single `phase!("mmio_bringup");` at the end of the block with granular phase boundaries immediately following each major component:

### `unaos/crates/kernel/src/drivers/gpu/kepler.rs`
#### [MODIFY] `kepler.rs`
- Add `phase!("pmc_vram_init");` right before `takeover_display`.
- Add `phase!("kdisp_takeover");` right after `takeover_display`.
- Add `phase!("pfifo_alloc_zero");` right after zeroing the 5 PFIFO pages.
- Add `phase!("runlist_and_pass0");` right after `mirror-hdr pass0 done`.
- Rename the final `phase!("mmio_bringup");` to `phase!("plant_and_pass1");`.

This decomposes the 331 ms into five numbers, exactly locating the cost without altering driver behavior.
