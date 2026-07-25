STATUS: PROPOSED

# Proposal — kepler-display pull 20: fbcon on panel

## Objectives
S29 proved we can render a full-panel pattern by writing to the GOP framebuffer using a 16384-byte pitch. However, `video::fbcon` is deriving its stride directly from the UEFI GOP mode info (which reports `stride = 2880` pixels, or `11520` bytes/row). This discrepancy means the kernel console currently shears off-screen because it is rendering with a row width of 11520 bytes, while the hardware scanout uses 16384 bytes. We will measure and document this discrepancy, and then simulate a corrected console output to prove the fix visually.

## Measure and Print (Read-Only)
We will extract the `gop_info` structure directly from `crate::video::fbcon::current_info()` and print its values prior to our fill:
- `:: kdisp: fbcon-view base={:016X} stride_px={} bpp={} w={} h={} row_bytes={} ::`
- `:: kdisp: fbcon-vs-hw row_bytes={} hw_pitch=16384 match={} ::`

## Discrepancy & Routing
The discrepancy exists because the UEFI GOP reports a 2880-pixel stride, but the Apple Kepler firmware configures the display controller to scan out with a 4096-pixel (16384-byte) pitch. 

**The correction genuinely belongs inside `video/fbcon.rs`** (e.g., patching the global `FRAMEBUFFER` info or adding a dynamic stride override mechanism). Because `video/fbcon.rs` is outside my lane, I will not reach into it. The coordinator must route this correction separately.

## Visual Proof
To prove that a corrected `fbcon` will render text properly, we will simulate it within our lane. After the calibration hold, we will draw three 8x8 WHITE glyph-sized blocks at a console-like origin (`y = 64..72`, `x = 64, 80, 96`) using the true hardware pitch of `16384`. This will leave text-shaped output perfectly placed at the top-left of the panel.
- Marker: `:: kdisp: fbcon-probe drawn rows=8 ::`

## No other changes
The full-panel calibration pattern (banded barcode, fiducials, diagonal) and its 5s hold will remain exactly as-is, serving as the reference frame before our probe.

Once approved, I will implement this change and commit it locally.
