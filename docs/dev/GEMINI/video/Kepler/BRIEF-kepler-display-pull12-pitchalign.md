# BRIEF — kepler-display pull 12: pitch-alignment × block-height mini-ladder

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #21 first.

## The s21 read this pull tests

GOB 64B×8 stands; block stacking is real; but no bh rung was clean and the
seam geometry scaled monotonically with bh → a SECOND parameter is wrong.
Prime suspect: **pitch alignment** — the hardware treats the surface as
having a blocks-per-row derived from an ALIGNED pitch, and our 180
GOBs/row (2880×4/64, unaligned) shears each block row. Candidates: 192
GOBs (align 64) and 256 GOBs (pow2).

## This pull — four cycles, two axes

Same latch/restore discipline, 5 s holds (standing length). Cycles in
order: (bh=4,pg=192), (bh=4,pg=256), (bh=8,pg=192), (bh=8,pg=256).

Per cycle:
1. Fill with the ruler through the block-linear transform using
   `gobs_per_row = pg` (the ALIGNED value) in the block-index math, while
   the visible image remains 2880 px wide (pixels beyond x=2880 within the
   padded pitch are filled BLACK — they are real bytes the hw will scan).
   Surface allocation grows accordingly (pg×64 bytes per 8-row GOB row) —
   still far inside the 0x1600000+ scratch region; print the computed
   surface bytes.
2. Markers:
   `:: kdisp: pa-step bh=<N> pg=<G> fill done bytes=NNNNNNNN ::`
   `:: kdisp: pa-step bh=<N> pg=<G> hold t=<n>s ::` (t=1..5)
   `:: kdisp: pa-step bh=<N> pg=<G> done ::`
3. Bench: one photo per hold, tagged from serial.

Verdict key: the (bh,pg) whose photo has ZERO brick seams and a SOLID white
left column is the real pair — mapping solved. If 192 and 256 both fail
with identical seam counts, pitch-alignment is refuted and the next suspect
(block-width > 1 GOB) gets its own pull.

## Gates (DONE = all of these)

Writes remain exactly 0x640460 + 0x640080 (×4 cycles, restore-paired).
Full-knob check both arches + builder-path esp-x86 + `strings` proof of new
markers in kernel.elf + default QEMU green both. Commit ALL docs+code;
delete scratch; `git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull12.md`, STATUS: PROPOSED).
