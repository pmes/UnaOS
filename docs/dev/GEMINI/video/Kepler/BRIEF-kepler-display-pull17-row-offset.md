# BRIEF — kepler-display pull 17: vertical placement calibration (the last variable)

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #26 first.

## The s26 verdict this pull acts on

⭐ MAPPING SOLVED — your linear/pitch-16384 cycle photographed seam-free
with a solid white column. One variable remains: vertical placement. Only
~a band-cycle of our rows displayed, at the bottom ~quarter of the panel;
horizontal is perfect, so the scan start sits at a fixed, measurable row
offset from our pointer.

## This pull — one cycle, marker-row ruler, same latch

1. Fill the surface LINEAR (pitch 16384, 1800 rows) with BLACK, then
   paint distinctive marker rows (full 2880-px wide):
   - rows 0–7: WHITE
   - rows 448–455: RED
   - rows 896–903: GREEN
   - rows 1344–1351: BLUE
   - rows 1792–1799: MAGENTA
   (8-px-tall stripes; the photo tells us which markers are visible and
   where on the panel — that names the offset and the visible row window.)
2. Markers:
   `:: kdisp: row-cal fill done bytes=01C20000 ::`
   `:: kdisp: row-cal hold t=<n>s ::` (t=1..5)
   `:: kdisp: row-cal done ::`
3. Latch/restore exactly as always (0x640460 → 0x640080), ONE cycle.

Deliverable: photo → (which colors visible, vertical position of each).
Pull 18 then adjusts the surface pointer by the measured offset and
re-runs the full ruler — full-panel, correctly placed, mapping closed
end-to-end; after that the lane graduates to the real framebuffer console.

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates at land-review.

Proposal first (`PROPOSAL-kepler-display-pull17.md`, STATUS: PROPOSED).
