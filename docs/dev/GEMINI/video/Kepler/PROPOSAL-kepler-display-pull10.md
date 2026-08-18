STATUS: APPROVED (2026-07-24 — clean match to brief; transform arithmetic verified: 180 GOBs/row, block-height 1)

# PROPOSAL — kepler-display pull 10: pre-swizzled ruler (block-linear proof)

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull10-swizzle.md`, this pull runs the identical latch sequence and test pattern (ruler64x8) as Pull 9. The only change is that the test pattern is written into VRAM using a linear-to-block-linear address transformation. This will prove or refute whether the NVIDIA display hardware reads the scanout buffer as a tiled (block-linear) surface rather than a pitch-linear surface.

No implementation code will be written until this proposal is reviewed and approved (transitioning to `STATUS: APPROVED`).

## 2. Implementation Steps

### Step 1: Prepare Surface with Pre-Swizzled Ruler Pattern
- The VRAM fill pattern is exactly `ruler64x8` (from Pull 9).
- **Address Transform**: Instead of `(y * pitch) + (x * 4)`, the target byte address for each pixel at `(x, y)` will be calculated assuming a GOB (Group of Bytes) size of 64 bytes wide by 8 rows high, with a block-height of 1 GOB.
- The arithmetic transform will be:
  ```rust
  let gob_width_bytes = 64;
  let gob_height = 8;
  let gobs_per_row = (expected_width * 4) / gob_width_bytes;
  let gob_size_bytes = gob_width_bytes * gob_height;

  // For a pixel at (x, y) with 4 bytes per pixel:
  let px_byte_x = x * 4;
  let gob_x = px_byte_x / gob_width_bytes;
  let gob_y = y / gob_height;
  let inner_x = px_byte_x % gob_width_bytes;
  let inner_y = y % gob_height;

  let gob_index = (gob_y * gobs_per_row) + gob_x;
  let target_byte_addr = (gob_index * gob_size_bytes) + (inner_y * gob_width_bytes) + inner_x;
  ```
- The calculated `target_byte_addr` will be used as the offset from the base of the surface to write the pixel's `final_color`.
- Emit markers: 
  `:: kdisp: surf2 geom w=NNNN h=NNNN pitch=NNNN ::`
  `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN pattern=ruler64x8-gob64x8 ::`

### Step 2-7: Identical Latch Sequence
- Steps 2 through 7 from Pull 9 remain completely unchanged.
- The same 8-second hold and mid-hold dump at `t=4` will execute, allowing the bench to photograph the panel and evaluate if the block-linear layout decodes into clean 64-row color stripes.

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Write constraints**: Writes remain strictly limited to `0x640460` and `0x640080` plus the VRAM fill.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs cleanly on both arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all changed markers in `kernel.elf`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: All docs and code committed locally. `git status` clean. **No push will be performed.**
