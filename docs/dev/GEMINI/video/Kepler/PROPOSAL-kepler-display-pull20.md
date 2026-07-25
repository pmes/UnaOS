STATUS: APPROVED (2026-07-25, coordinator GR4) WITH ONE BINDING AMENDMENT.

**AMENDMENT (binding) — don't presume the number you're about to measure.**
The prose asserts as fact that GOP reports stride = 2880 px / 11520 B. We
have never read it; that is exactly what `fbcon-view` is for. Reword the
proposal's claim as a hypothesis and let the printed values settle it —
if GOP already reports 4096 px, the console's problem is elsewhere and
this pull's deliverable is that finding instead.

**Lane call — CORRECT, and thank you for stopping.** The fbcon-side fix is
outside your lane; the coordinator takes it. Your half is measure + visual
proof, exactly as proposed.

Verified for you: `crate::video::fbcon::current_info()` exists
(fbcon.rs:260, returns `unaos_boot_info::FrameBufferInfo`), so your read
compiles as written. Everything else as proposed.

# Proposal — kepler-display pull 20: fbcon on panel

## Objectives
S29 proved we can render a full-panel pattern by writing to the GOP framebuffer using a 16384-byte pitch. The kernel console currently shears off-screen, likely because `video::fbcon` is deriving its stride directly from the UEFI GOP mode info (which we hypothesize reports `stride = 2880` pixels, or `11520` bytes/row) while the hardware scanout uses 16384 bytes. We will measure and document this possible discrepancy, and then simulate a corrected console output to prove the fix visually.

## Measure and Print (Read-Only)
We will extract the `gop_info` structure directly from `crate::video::fbcon::current_info()` and print its values prior to our fill:
- `:: kdisp: fbcon-view base={:016X} stride_px={} bpp={} w={} h={} row_bytes={} ::`
- `:: kdisp: fbcon-vs-hw row_bytes={} hw_pitch=16384 match={} ::`

## Discrepancy & Routing
If our hypothesis is correct and the discrepancy exists, it is because the UEFI GOP reports a 2880-pixel stride, but the Apple Kepler firmware configures the display controller to scan out with a 4096-pixel (16384-byte) pitch. If GOP already reports 4096 px, the console's problem is elsewhere and that finding becomes our deliverable. 

**The correction genuinely belongs inside `video/fbcon.rs`** (e.g., patching the global `FRAMEBUFFER` info or adding a dynamic stride override mechanism). Because `video/fbcon.rs` is outside my lane, I will not reach into it. The coordinator must route this correction separately.

## Visual Proof
To prove that a corrected `fbcon` will render text properly, we will simulate it within our lane. After the calibration hold, we will draw three 8x8 WHITE glyph-sized blocks at a console-like origin (`y = 64..72`, `x = 64, 80, 96`) using the true hardware pitch of `16384`. This will leave text-shaped output perfectly placed at the top-left of the panel.
- Marker: `:: kdisp: fbcon-probe drawn rows=8 ::`

## No other changes
The full-panel calibration pattern (banded barcode, fiducials, diagonal) and its 5s hold will remain exactly as-is, serving as the reference frame before our probe.

Once approved, I will implement this change and commit it locally.
