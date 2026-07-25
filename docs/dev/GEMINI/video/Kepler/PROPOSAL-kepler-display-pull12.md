STATUS: APPROVED (2026-07-25, coordinator GR4). AMENDMENTS (binding):
(1) Lane file is `unaos/crates/kernel/src/gpu/kepler_display.rs` — NOT
    `drivers/gpu/`. Same file as pulls 5–11; do not create a new path.
(2) All brief gates apply verbatim: full-knob `./arroyo check` both arches,
    default `./arroyo test` + `test-arm` green, builder-path
    `UNAOS_USBDEBUG=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1
    UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86`, strings proof of the new
    `pa-step` markers in `target/x86_64_esp/kernel.elf`.

# Proposal — kepler-display pull 12: pitch-alignment × block-height mini-ladder

## Objectives
Execute the next phase of the cleanroom reverse engineering for the Kepler display pipeline by testing pitch-alignment and block-height combinations to eliminate the brick-seam artifacts.

## Plan
We will modify the display takeover routine in `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs` to loop through four cycles of the parameters `(bh, pg)`:
1. `(bh=4, pg=192)`
2. `(bh=4, pg=256)`
3. `(bh=8, pg=192)`
4. `(bh=8, pg=256)`

For each cycle:
1. **Padded Width Calculation**: `pg` is the number of GOBs per row. Each GOB is 64 bytes wide (16 pixels). The padded width in pixels will be `pg * 16`.
2. **Surface Fill**: We will loop `x` from `0` to `padded_width - 1` and `y` from `0` to `expected_height - 1` (1800).
    - If `x < 2880`, the normal ruler pattern is drawn.
    - If `x >= 2880`, we will write `0xFF000000` (BLACK).
3. **Index Math**: We will use `gobs_per_row = pg` for computing the `blk_index`.
4. **Computed Surface Bytes**: The surface size footprint will be calculated based on the maximum block row touched: `num_block_rows = (225 + bh - 1) / bh`, and `total_bytes = num_block_rows * bh * pg * 512`. We will print this size in the trace marker as a hexadecimal value (e.g., `bytes=NNNNNNNN`).
5. **Trace Markers**: We will update the trace logging exactly as requested:
    - `:: kdisp: pa-step bh=<N> pg=<G> fill done bytes=NNNNNNNN ::`
    - `:: kdisp: pa-step bh=<N> pg=<G> hold t=<n>s ::` (1 to 5 seconds)
    - `:: kdisp: pa-step bh=<N> pg=<G> done ::`
6. **Hardware Updates**: Writes remain exactly `0x640460` followed by `0x640080`.

## No other changes
We will maintain the same structure from Pull 11 including the 5s holds and 1s recovery gaps.
Once this proposal is approved, the implementation will be written, verified through all checks, and then locally committed.
