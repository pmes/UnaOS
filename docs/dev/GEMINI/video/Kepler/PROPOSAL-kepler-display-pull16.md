STATUS: PROPOSED

# Proposal — kepler-display pull 16: linear 16k fill

## Objectives
Following the S25 verdict, the hardware scanout is configured as LINEAR with a 16384-byte pitch (0x4000), not block-linear. We will test this by rendering a single linear cycle matching this exact configuration to eliminate all remaining seams.

## Plan
We will update `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`. The previous pull 15 recon dump will be gated behind a `let run_recon = false;` bool. We will set `let do_takeover = true;` and replace the previous `(bw, pg)` matrix loop with a single linear pass.

1. **Linear Fill Math**:
   - Total rows: 1800
   - Row stride (pitch): 16384 bytes
   - Total columns (px): `16384 / 4 = 4096`
   
   For each row `y` in `0..1800`:
     - Determine the `row_color` based on `(y / 64) % 8` and `y % 64 == 0` (black separator), identical to previous pulls.
     - For each column `x` in `0..4096`:
       - If `x >= 2880`, `color = 0xFF000000` (BLACK padding bytes from 11520 to 16384)
       - Else if `x < 256`, `color = 0xFFFFFFFF` (WHITE column)
       - Else if `x < 264`, `color = 0xFF000000` (BLACK line)
       - Else, `color = row_color`
       - `target_byte_addr = (y * 16384) + (x * 4)`
       - Write `color` to `dst.add(target_byte_addr)`

2. **Trace Markers**:
   - `:: kdisp: lin-step pitch=4000 fill done bytes=NNNNNNNN ::`
   - `:: kdisp: lin-step pitch=4000 hold t=<n>s ::` (for t in 1..=5)
   - `:: kdisp: lin-step pitch=4000 done ::`

3. **Hardware Updates**: Writes remain exactly `0x640460` followed by `0x640080`. The 5s holds and 1s recovery gap remain identical. ONE cycle only.

4. **Preservation**: The recon-dump code will remain compiled but gated off (`if false`). We will NOT touch 0x640468/46C/470.

## No other changes
Once approved, I will implement this linear cycle in `kepler_display.rs`, run all the testing gates, and commit the changes without pushing.
