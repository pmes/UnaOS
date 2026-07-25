STATUS: PROPOSED

# Proposal — kepler-display pull 15: mirror surface params

## Objectives
Since the parameter matrix failed to eliminate the seams, we will read the surface configuration (pitch, WxH, block-mode) directly from the hardware. We will perform a read-only dense dump of the EVO core method mirror `0x640400–0x6405FC`, flagging candidates that match the expected dimensions and block-mode patterns.

## Plan
We will update `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`. We will disable all writes (both filling and latching) for this pull using a boolean gate (`let do_takeover = false;`), keeping the code path intact for pull 16.

1. **Pass 1: Dense Dump**
   We will iterate over offsets `0x400` to `0x5FC` (step 4). For each offset:
   - Read `val = mmio_read(bar0, 0x640000 + offset)`.
   - Print `:: kdisp: mirror-sp off=XXX val=XXXXXXXX ::`.
   - Append ` ABSENT?` if the value is `0xFFFFFFFF` or matches `0xBAD0xxxx`.

2. **Wait / Settle**
   Spin loop for ~100ms (`for _ in 0..1_500_000 { core::hint::spin_loop(); }`) to check for volatility.

3. **Pass 2: Volatility Check**
   Repeat the exact same loop as Pass 1, but prefix the marker with `mirror-sp2`.

4. **Pass 3: Cross-Check Candidates**
   - Read `0x640460` and print: 
     `:: kdisp: mirror-sp ptr-slot val=XXXXXXXX expect=00090000-ish (fw surface ptr>>8?) ::`
   - Iterate over the offsets again, looking for specific patterns (skipping absent or 0 values):
     - **Pitch candidates**: Check if `val` equals `11520` (0x2D00), `46080` (0xB400), `720` (0x2D0), `180` (0xB4), `192`, `256`, or their `<< 8` shifted equivalents.
     - **WxH candidates**: Check if the low or high 16 bits equal `2880` (0xB40) or `1800` (0x708).
     - **Block-mode candidates**: Check if `val < 0x100` (indicating only low nibbles are set).
   - For any match, print:
     `:: kdisp: mirror-sp cand off=XXX val=XXXXXXXX kind=<pitch|wh|blockmode> ::`

## No other changes
Once approved, I will implement this recon dump in `kepler_display.rs`, run all the testing gates, and commit the changes without pushing.
