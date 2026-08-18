STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — index math,
byte math, and the cited x-fastest within-block ordering all check out.
Implement exactly as written; all brief gates apply.

# Proposal — kepler-display pull 13: block-width ladder (block > 1 GOB wide)

## Objectives
Following the S22 verdict refuting pitch-alignment as the secondary shear parameter, we test the surviving suspect: `block width > 1 GOB`. We will run a mini-ladder across block widths `bw ∈ {2, 4}` and block heights `bh ∈ {4, 8}` to find the matching configuration.

## Plan
We will update the `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs` takeover routine to loop through the four configurations:
1. `(bw=2, bh=4)`
2. `(bw=2, bh=8)`
3. `(bw=4, bh=4)`
4. `(bw=4, bh=8)`

For each cycle:
1. **Padded Pitch Calculation**: The natural pitch is 180 GOBs per row. We calculate the padded GOBs per row as `let pg = ((180 + bw - 1) / bw) * bw;` (which evaluates to 180 for both `bw=2` and `bw=4`, meaning no actual padding is added beyond the natural 2880 px width, but the logic remains robust).
2. **Index Math**:
   - `px_byte_x = x * 4`
   - `gob_x = px_byte_x / 64` (gob_width_bytes)
   - `inner_x = px_byte_x % 64`
   - `gob_y = y / 8` (gob_height)
   - `inner_y = y % 8`
   
   - `blk_col = gob_x / bw`
   - `gob_inner_x = gob_x % bw`
   - `blk_y = gob_y / bh`
   - `gob_inner_y = gob_y % bh`
   
   - `blocks_per_row = pg / bw`
   - `blk_index = (blk_y * blocks_per_row) + blk_col`
   
   Within a block, GOBs are ordered **x-fastest then y**. This is explicitly cited from `envytools/docs/hw/memory/g80-surface.rst` (lines 200-205): "gobs inside a block are stored ordered first by x coord, then by y coord...".
   - `gob_inner_index = (gob_inner_y * bw) + gob_inner_x`
   
   The final byte address is:
   - `target_byte_addr = (blk_index * bw * bh * 512) + (gob_inner_index * 512) + (inner_y * 64) + inner_x`

3. **Surface Size Calculation**:
   `let gob_rows = (expected_height + 8 - 1) / 8;`
   `let num_block_rows = (gob_rows + bh - 1) / bh;`
   `let total_bytes = num_block_rows * bw * bh * blocks_per_row * 512;`

4. **Trace Markers**:
   - `:: kdisp: bw-step bw=<W> bh=<N> pg=<G> fill done bytes=NNNNNNNN ::`
   - `:: kdisp: bw-step bw=<W> bh=<N> pg=<G> hold t=<n>s ::` (t=1..5)
   - `:: kdisp: bw-step bw=<W> bh=<N> pg=<G> done ::`

5. **Hardware Updates**: Writes remain exactly `0x640460` followed by `0x640080`. The 5s holds and 1s recovery gap remain identical.

## No other changes
Once approved, I will implement this in `kepler_display.rs`, run the required gates, and commit the changes without pushing.
