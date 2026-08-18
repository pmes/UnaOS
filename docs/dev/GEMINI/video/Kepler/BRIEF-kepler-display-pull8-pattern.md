# BRIEF — kepler-display pull 8: pattern-fill mapping decode

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #17 first.

## Facts this pull stands on (s17, capture-verified + panel-verified)

- Arm (0x640460) + UPDATE (0x640080) latch WORKS: solid-green surf2 produced
  a GREEN BAR at the BOTTOM of the panel; restore recovered the screen.
- 0x6101E0 never followed (not the live scanout tracker); asm slot holds
  writes; selfcheck showed no premature latch.
- Unknown: why a bottom BAND — the offset maps a sub-region (stride/tiling/
  window split undetermined).

## This pull — same writes, discriminating pattern

IDENTICAL write set and sequence to pull 7 (0x640460 arm → 0x640080 UPDATE →
hold → restore → UPDATE). The ONLY changes are VRAM fill content and richer
serial geometry:

1. **Pattern fill** replacing solid green, computed in gop_info geometry
   (pitch = width×4 assumed, that assumption is under test):
   - Vertical quarters by row: rows [0,h/4)=RED 0xFFFF0000,
     [h/4,h/2)=GREEN 0xFF00FF00, [h/2,3h/4)=BLUE 0xFF0000FF,
     [3h/4,h)=WHITE 0xFFFFFFFF.
   - Left 64 columns of every row overridden BLACK 0xFF000000 (column marker
     distinguishes horizontal wrap from vertical offset).
   Marker: `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN pattern=quarters+leftbar ::`
2. Print the geometry used:
   `:: kdisp: surf2 geom w=NNNN h=NNNN pitch=NNNN ::`
3. Same latch ladder and markers as pull 7 (`latch pre/asm-wrote/selfcheck/
   update-wrote/hold/restored/verdict`), hold extended to 8 s
   (`t=1..8`), so the bench can note WHICH colors appear WHERE.
4. NEW read during the hold (read-only): dump the head-0 timing cluster once,
   at t=4 — `:: kdisp: latch midhold 616340=XXXXXXXX 61634C=XXXXXXXX 6101E0=XXXXXXXX 61D1E0=XXXXXXXX 61D014=XXXXXXXX ::`
   (does ANY known state word move while the panel shows the pattern?)

Bench observation wanted (goes in the capture notes, not code): which colors
visible, where on the panel, in what vertical order, any repetition/tearing.
That observation + the pattern geometry decode the mapping arithmetically.

## Gates (DONE = all of these)

Writes remain exactly 0x640460 + 0x640080 (+ VRAM fill). Full-knob check
both arches + builder-path esp-x86 + `strings` proof of new/changed markers
in kernel.elf + default QEMU green both. Commit ALL docs+code; delete
scratch; `git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull8.md`, STATUS: PROPOSED).
