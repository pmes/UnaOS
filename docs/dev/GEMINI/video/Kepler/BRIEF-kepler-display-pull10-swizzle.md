# BRIEF — kepler-display pull 10: pre-swizzled ruler (block-linear proof)

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #19 first.

## The hypothesis this pull proves or refutes (s19 decode)

The scanout window reads the surface **block-linear** — NVIDIA GOB tiling,
GOB = 64 bytes wide × 8 rows — while we fill linear. It explains all four
s19 panel facts (8× vertical compression, 16-px dash checkerboard,
vanished white column, s17 solid-color immunity).

## This pull — same writes, pre-swizzled fill

IDENTICAL write set and latch sequence (0x640460 arm → 0x640080 UPDATE →
8 s hold → restore). The fill is the SAME ruler64x8 pattern as pull 9, but
written through a **linear→block-linear address transform** before storing:

1. Compute each pixel's linear position (x, y) and its target byte address
   under GOB tiling with GOB 64 B × 8 rows, block height = 1 GOB (start
   simple: no higher-order block stacking — if the photo comes back
   partially descrambled, block height is the next knob and pull 11 steps
   it). Document the exact transform as a comment with the arithmetic —
   this is address math, not borrowed semantics.
2. Markers:
   `:: kdisp: surf2 geom w=NNNN h=NNNN pitch=NNNN ::`
   `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN pattern=ruler64x8-gob64x8 ::`
   plus the unchanged latch ladder.
3. Bench deliverable: one photo during the hold.
   - CLEAN 64-row color stripes + solid white left column = block-linear
     PROVEN with GOB 64×8, block-height 1 — and the ruler then reads out
     the band's row-mapping directly.
   - Partially descrambled = tiling confirmed, block-height wrong (pull 11
     steps it: 2, 4, 8, 16 GOBs).
   - Identical scramble to s19 = hypothesis refuted, honest null.

## Gates (DONE = all of these)

Writes remain exactly 0x640460 + 0x640080 (+ VRAM fill). Full-knob check
both arches + builder-path esp-x86 + `strings` proof of the changed pattern
marker in kernel.elf + default QEMU green both. Commit ALL docs+code;
delete scratch; `git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull10.md`, STATUS: PROPOSED).
