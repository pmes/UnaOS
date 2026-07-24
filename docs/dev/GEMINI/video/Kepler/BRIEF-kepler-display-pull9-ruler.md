# BRIEF — kepler-display pull 9: ruler pattern (pitch + row-mapping solve)

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #18 first.

## Facts this pull stands on (s18 photo + serial, capture-verified)

- The latch scans EARLY surface rows (red quarter + part of green) into a
  fixed bottom band; no blue/white ever visible.
- The 64-px black left bar showed as staggered drifting dashes, not a
  column → working hypothesis: hardware pitch ≠ w×4=11520. The ruler below
  measures the real pitch and the row mapping from one photo.
- Latch mechanism unchanged and proven (s17/s18): 0x640460 arm + 0x640080
  UPDATE, restore-paired. 0x61634C mutates under latch (logged; read it
  this pull too).

## This pull — same writes, ruler fill

IDENTICAL write set and latch sequence to pull 8 (arm → UPDATE → 8 s hold →
restore → UPDATE). Only the fill changes:

1. **Row ruler (vertical solve):** row color cycles every 64 rows through 8
   maximally-distinct colors in fixed order: RED, GREEN, BLUE, YELLOW
   (0xFFFFFF00), CYAN (0xFF00FFFF), MAGENTA (0xFFFF00FF), WHITE, GRAY
   (0xFF404040). Sequence restarts every 512 rows. Additionally every row
   where (row % 64) == 0 is pure BLACK (thin tick line between color
   blocks).
2. **Pitch probe (horizontal solve):** the LEFT 256 pixels of every row are
   forced WHITE, then 8 BLACK pixels — a wide high-contrast marker whose
   drift per scanline gives the pitch delta exactly.
3. Markers:
   `:: kdisp: surf2 geom w=NNNN h=NNNN pitch=NNNN ::`
   `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN pattern=ruler64x8 ::`
   plus the unchanged latch ladder (pre/asm-wrote/selfcheck/update-wrote/
   hold t=1..8 with midhold at t=4/restored/verdict).

Bench deliverable: one photo during the hold. The color order visible +
count of 64-row blocks + the white marker's drift angle solve row-offset,
scale, and true pitch arithmetically. (Coordinator does that decode.)

## Gates (DONE = all of these)

Writes remain exactly 0x640460 + 0x640080 (+ VRAM fill). Full-knob check
both arches + builder-path esp-x86 + `strings` proof of changed markers in
kernel.elf + default QEMU green both. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull9.md`, STATUS: PROPOSED).
