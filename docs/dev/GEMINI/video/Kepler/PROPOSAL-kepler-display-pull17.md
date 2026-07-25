STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — single
linear cycle, five marker stripes at the briefed rows, recon stays gated.
(Note: "stay as landed" for the old-base probes means gated-off, per the
s26 ring trim.)

# Proposal — kepler-display pull 17: row offset calibration

## Objectives
The mapping is verified as linear with a 16384-byte pitch. Now we must calibrate the vertical placement (scan start row offset) by drawing a series of distinctively colored horizontal stripes at known rows. 

## Plan
We will update `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`. The code remains a single linear cycle with `pitch = 16384` and `expected_height = 1800`.

1. **Linear Fill Math (Row Cal)**:
   For each row `y` in `0..1800`:
     - Determine the `row_color` based on `y`:
       - `0..=7`: `0xFFFFFFFF` (WHITE)
       - `448..=455`: `0xFFFF0000` (RED)
       - `896..=903`: `0xFF00FF00` (GREEN)
       - `1344..=1351`: `0xFF0000FF` (BLUE)
       - `1792..=1799`: `0xFFFF00FF` (MAGENTA)
       - Everything else: `0xFF000000` (BLACK)
     
     - For each column `x` in `0..4096` (the 16384-byte row width):
       - If `x >= 2880`, `color = 0xFF000000` (BLACK padding bytes)
       - Else, `color = row_color`
       - Write `color` to `dst.add((y * 16384) + (x * 4))`

2. **Trace Markers**:
   - `:: kdisp: row-cal fill done bytes=01C20000 ::`
   - `:: kdisp: row-cal hold t=<n>s ::` (for t in 1..=5)
   - `:: kdisp: row-cal done ::`

3. **Hardware Updates**: Writes remain exactly `0x640460` followed by `0x640080`. The 5s holds and 1s recovery gap remain identical. ONE cycle only. The pull 15 recon dump remains gated off.

## No other changes
Once approved, I will implement this calibration cycle in `kepler_display.rs` and commit the changes without pushing. The coordinator will run all builds and gates at land-review.
