STATUS: APPROVED (2026-07-25, coordinator GR4) WITH ONE BINDING AMENDMENT.

**AMENDMENT (binding) — diagonal width.** 4 px is not photographable on
this panel (2880×1800 on 15", ~0.35 mm; the s28 diagonal that read so well
was 16 px). Keep the new corner-to-corner slope `diag_x = y * 2880 / 1800`
but restore the width to **16 px** (`x >= diag_x && x < diag_x + 16`).

**Expected and accepted, not a defect:** this draw destroys the firmware
console for the rest of the boot — we are writing on the surface being
scanned and there is deliberately no restore. Later kernel console output
will scribble over parts of the pattern after the hold ends; the photo is
taken during the hold, so it is unaffected. Say so in your report.

Everything else as proposed: dst at `bar1 + gop_vram_offset`, 1800 rows,
pitch 16384, fiducials, barcode, banding, latch and restore removed,
fb-draw markers, one hold.

# Proposal — kepler-display pull 19: relocate decisive

## Objectives
S28 demonstrated that the EVO arm+UPDATE path never repointed the scanout, and our rendering results were actually writes directly overlapping the active firmware GOP framebuffer. We will make this deliberate: we will render our full-panel calibration pattern (with the barcode) directly into the GOP framebuffer (`gop_vram_offset`) and remove the EVO latch code entirely.

## Plan
We will update `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`.

1. **Target the GOP Framebuffer**:
   - Change `dst` to point directly to `bar1 + gop_vram_offset` instead of the scratch surface `0x1600000`.

2. **Full-Panel Calibration Pattern**:
   - Retain the linear layout: 1800 rows, pitch `16384`.
   - Add fiducials at the top and bottom (`y < 4` and `y >= 1796` rendered as solid WHITE).
   - Add a diagonal line from top-left to bottom-right (`x >= diag_x && x < diag_x + 4` where `diag_x = (y * 2880) / 1800`, rendered as WHITE).
   - Retain the 16-row color banding and the left-aligned 7-bit barcode of `band_idx`.

3. **Remove the EVO Latch**:
   - Remove the writes to `asm_reg` (`0x640460`) and `update_reg` (`0x640080`).
   - Remove the restore code.
   - We will keep the pre-state reads and the post-fill reg-dump of the 0x460/0x4B8 clusters during the hold loop, as requested.

4. **Trace Markers**:
   - Use `:: kdisp: fb-draw base={:08X} pitch={} rows={} bytes={:08X} ::` using `gop_vram_offset`.
   - Log `hold t=1..5` and `done` (one hold, one photo).

## Expected Result
The monitor will display the full-panel pattern correctly aligned to the screen, with fiducials visible on the extreme top and bottom, the barcode rendering perfectly from `band_idx=0` to `band_idx=112`, and a continuous diagonal from corner to corner. This proves UnaOS has complete working access to the linear GOP framebuffer.

Once approved, I will implement this change and commit it locally.
