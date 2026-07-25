# BRIEF — kepler-display pull 18: placement-model probe (you design it)

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #27 first.

## The s27 facts this pull acts on

Row-cal photo: ONLY the white stripe (rows 0–7) visible, one line at
~62–66% of panel height. Red@448 / green@896 / blue@1344 / magenta@1792
ALL absent. Two hard deductions already made:
1. Simple 1:1-with-offset is REFUTED — at 1:1, red@448 fits on-screen
   below the white line and it isn't there.
2. Arithmetic bound: our pointer (VRAM +0x1600000) is only 352 rows
   (×16384 B) above VRAM 0, so 1:1 scan placing row 0 at panel ~1190 is
   impossible anyway.

Open hypotheses, strongest first: (a) vertical scaling — the fw scan mode
is smaller than native and lines are doubled/scaled (note: mirror size
07080B40 says 1800×2880, but scaling may live in a viewport/raster
cluster — recall 07080B40 repeats at 0x4B8–0x4C8); (b) the scan window
covers fewer memory rows than the panel shows; (c) pointer-latch
granularity/truncation (our 0x016000 in the 0x460 slot — check which
bits actually arm by comparing the armed readback).

## This pull — YOU design the discriminating pattern

One latch cycle, restore-paired, writes remain exactly 0x640460 +
0x640080. Design a fill pattern (linear, pitch 16384) that discriminates
(a)/(b)/(c) in a SINGLE photo — e.g. per-row-index encodings: thin
stripes at power-of-two rows, distinct colors every 64 rows with a
binary-coded left-edge key, or a gradient with landmark rows. Your
proposal must state, per hypothesis, what the photo would show if that
hypothesis is true (a falsification table). Markers: `:: kdisp: pm-step
... fill done bytes=NNNNNNNN ::` / hold t=1..5 / done, exact format your
choice within the `pm-step` prefix.

Optional second evidence channel (read-only, allowed): re-read the
0x640400–0x6405FC window AFTER your latch while holding (the recon code
is already in the file, gated) and print the 0x460/0x468/0x46C/0x470 +
0x4B8–0x4C8 cluster — armed-vs-written comparison feeds hypothesis (c).

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates at land-review and delivers the sitting ESP.

Proposal first (`PROPOSAL-kepler-display-pull18.md`, STATUS: PROPOSED) —
must include the falsification table.
