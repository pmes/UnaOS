# BRIEF — kepler-display pull 20: put the kernel console on the panel

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY (read `video/fbcon.rs` for facts, but do not edit outside your lane —
if the fix genuinely belongs in fbcon, say so in the proposal and STOP;
the coordinator will route it).

## What s29 gave us

`fb-draw base=00020000 pitch=16384 rows=1800 bytes=01C20000`,
**cover=exact**, and a full-panel photo. UnaOS can put arbitrary pixels on
the rMBP panel: linear, 16384 B/row, at VRAM 0x20000 (phys 0x90020000).

## The open discrepancy this pull resolves

`kepler_display` has carried `expected_pitch = 11520` (2880 × 4) since the
early pulls, while the hardware's real stride — proven by the mirror
storage word and by your own correct-geometry photo — is **16384**.
Meanwhile `video::fbcon` derives its stride from the GOP mode info
(`info.stride * info.bytes_per_pixel`, fbcon.rs:157) and the kernel
console is NOT visibly rendering on this panel. Those facts need
reconciling before the console can work: if fbcon believes the stride is
2880 px while the scanout uses 4096 px, every console row lands 1216 px
short and the text shears off-screen.

## This pull — measure first, then one narrow change

1. **Measure and print, read-only**, right where the fb-draw runs:
   `:: kdisp: fbcon-view base={:016X} stride_px={} bpp={} w={} h={} row_bytes={} ::`
   taking every value from fbcon's own info struct (not from our
   assumptions), plus
   `:: kdisp: fbcon-vs-hw row_bytes={} hw_pitch=16384 match={} ::`
2. **If they disagree**, the fix is to make our side use the hardware
   truth — state in the proposal exactly which value is wrong and where
   the corrected one must come from. If the correction belongs inside
   `video::fbcon` (outside your lane), say so and stop; do not reach.
3. **Then prove it visually**: after the calibration hold, draw a small
   number of legible glyph-sized blocks (or reuse your existing pattern
   scaled down) at a known console-like origin, so the photo shows
   text-shaped output landing where a console would start. Marker
   `:: kdisp: fbcon-probe drawn rows={} ::`.
4. Keep the full-panel calibration draw and its hold as-is before this —
   it is now our known-good reference frame.

Verdict key: `match=true` plus text-shaped output at the top-left of the
panel ⇒ the console path is ready to be wired for real. `match=false` ⇒
the printed numbers name the exact fix, and that is the whole deliverable.

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates and delivers the sitting ESP.

Proposal first (`PROPOSAL-kepler-display-pull20.md`, STATUS: PROPOSED).
