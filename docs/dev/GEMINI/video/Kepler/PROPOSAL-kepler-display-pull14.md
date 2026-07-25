STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — matrix,
index math, and byte math all check out (worst case 0x1C80000, inside
scratch). Implement exactly as written; all brief gates apply.

# Proposal — kepler-display pull 14: block-width × aligned-pitch matrix

## Objectives
Following the S23 verdict, the uniform periodic brick seams were eliminated by setting block width (`bw`) > 1. However, seams clustered in narrow regions, suggesting a secondary shear variable interacting with block width. We will test block width (`bw`) against aligned pitch (`pg`) directly, while fixing block height (`bh`) at 4 (the cleanest S23 configuration). 

## Plan
We will update the `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs` takeover loop to iterate over the matrix `(bw, pg)`:
1. `(bw=2, pg=192)`
2. `(bw=2, pg=256)`
3. `(bw=4, pg=192)`
4. `(bw=4, pg=256)`

For each cycle, `bh` is fixed at 4.

1. **Padded Pitch Calculation**:
   `padded_width_px = pg * 16`
   `blocks_per_row = pg / bw` (all combinations evenly divide since 192 and 256 are multiples of 4).

2. **Index Math**:
   - `px_byte_x = x * 4`
   - `gob_x = px_byte_x / 64`
   - `inner_x = px_byte_x % 64`
   - `gob_y = y / 8`
   - `inner_y = y % 8`
   
   - `blk_col = gob_x / bw`
   - `gob_inner_x = gob_x % bw`
   - `blk_y = gob_y / 4` (bh = 4)
   - `gob_inner_y = gob_y % 4`
   
   - `blk_index = (blk_y * blocks_per_row) + blk_col`
   - `gob_inner_index = (gob_inner_y * bw) + gob_inner_x` (x-fastest)
   
   The byte address:
   - `target_byte_addr = (blk_index * bw * 4 * 512) + (gob_inner_index * 512) + (inner_y * 64) + inner_x`

3. **Surface Fill Constraints**:
   - The loop over `x` goes up to `padded_width_px`.
   - If `x >= 2880`, the pixel is painted BLACK (`0xFF000000`).

4. **Surface Size Calculation**:
   `let gob_rows = (expected_height + 8 - 1) / 8;`
   `let num_block_rows = (gob_rows + 4 - 1) / 4;`
   `let total_bytes = num_block_rows * bw * 4 * blocks_per_row * 512;`

5. **Trace Markers**:
   - `:: kdisp: bwpg-step bw=<W> bh=4 pg=<G> fill done bytes=NNNNNNNN ::`
   - `:: kdisp: bwpg-step bw=<W> bh=4 pg=<G> hold t=<n>s ::` (t=1..5)
   - `:: kdisp: bwpg-step bw=<W> bh=4 pg=<G> done ::`

6. **Hardware Updates**: Writes remain exactly `0x640460` followed by `0x640080`. The 5s holds and 1s recovery gap remain identical.

## No other changes
Once approved, I will implement this matrix in `kepler_display.rs`, run all the testing gates, and commit the changes without pushing.
