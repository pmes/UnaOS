# BRIEF — kepler-display pull 11: block-height step ladder

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #20 first.

## Facts this pull stands on (s20, photo + serial)

GOB 64B×8 is CONFIRMED (checkerboard gone, bands continuous, cycle order
right). Remaining artifact: periodic brick-seam x-steps → block-height
(GOBs stacked per block before x advances) is wrong; we assumed 1.

## This pull — four holds, one boot, block-height stepped

Same two registers, same restore discipline. The boot runs FOUR sequential
latch cycles, one per block-height bh ∈ {2, 4, 8, 16}:

For each bh, in order:
1. Fill surf2 with the ruler through the FULL block-linear transform:
   - `blk_y = gob_y / bh`, `blk_inner = gob_y % bh`
   - `blk_index = blk_y * gobs_per_row + gob_x`
   - `addr = blk_index * (bh * 512) + blk_inner * 512 + inner_y * 64 + inner_x`
   (512 = GOB bytes; document the arithmetic in a comment.)
2. `:: kdisp: bh-step bh=<N> fill done ::`
3. Arm 0x640460=0x00016000, UPDATE 0x640080=0, hold 4 s
   (`:: kdisp: bh-step bh=<N> hold t=<n>s ::` once/s),
   restore + UPDATE, 1 s recovery gap.
4. `:: kdisp: bh-step bh=<N> done ::`

Bench deliverable: one photo PER HOLD (four total), each labeled by the
serial bh in flight. The bh whose photo shows NO brick-seams + a SOLID
white left column is the real block height — and with it the mapping is
fully solved (band placement gets re-read from that clean photo).

Keep the standard geom/prep markers with `pattern=ruler64x8-gob64x8-bh<N>`.

## Gates (DONE = all of these)

Writes remain exactly 0x640460 + 0x640080 (×4 cycles, restore-paired each).
Full-knob check both arches + builder-path esp-x86 + `strings` proof of the
new markers in kernel.elf + default QEMU green both. Commit ALL docs+code;
delete scratch; `git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull11.md`, STATUS: PROPOSED).
