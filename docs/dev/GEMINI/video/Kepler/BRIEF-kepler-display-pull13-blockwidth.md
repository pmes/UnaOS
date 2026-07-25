# BRIEF — kepler-display pull 13: block-width ladder (block > 1 GOB wide)

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #22 first.

## The s22 verdict this pull acts on

Pitch alignment is REFUTED: at fixed bh, pg=192 and pg=256 produced
IDENTICAL seam geometry (same count, same x positions). Seam count still
scales with bh (~6–7 @bh4, 2–3 @bh8). GOB 64B×8 stands (s20); block
stacking is real (s21). The surviving suspect for the second parameter is
**block width > 1 GOB**: hardware fetches blocks that are bw GOBs wide ×
bh GOBs tall, so our bw=1 index math shears columns at block granularity.

## This pull — block-width × block-height, four cycles

Same latch/restore discipline, 5 s holds, 1 s gaps. Revert pitch to the
natural 180 GOBs/row unless the bw math needs padding to a multiple of bw
(pad to the next multiple of bw with BLACK, as in pull 12; print the padded
gobs_per_row in the marker).

Cycles in order: (bw=2,bh=4), (bw=2,bh=8), (bw=4,bh=4), (bw=4,bh=8).

Index math per pixel (propose your exact formulation in the proposal):
- gob_x, inner as before (GOB 64B×8 unchanged);
- block column = gob_x / bw, gob-within-block x = gob_x % bw;
- blocks_per_row = padded_gobs_per_row / bw;
- block index = blk_y * blocks_per_row + blk_col;
- within a block, GOBs order x-fastest then y (state this explicitly in
  the proposal; if envytools hwdocs — allowed source — says Kepler orders
  differently, follow the doc and cite it).

Markers:
`:: kdisp: bw-step bw=<W> bh=<N> pg=<G> fill done bytes=NNNNNNNN ::`
`:: kdisp: bw-step bw=<W> bh=<N> pg=<G> hold t=<n>s ::` (t=1..5)
`:: kdisp: bw-step bw=<W> bh=<N> pg=<G> done ::`

Bench: one photo per hold, tagged from serial. Verdict key: zero seams +
solid white left column names (bw,bh). If all four still seam, the ladder
report (seam count/positions per cycle) feeds the next derivation — record
them precisely.

## Gates (DONE = all of these)

Writes remain exactly 0x640460 + 0x640080 (×4 cycles, restore-paired).
Full-knob `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1
UNAOS_KEPLER_FIFO=1 ./arroyo check` both arches; default `./arroyo test` +
`./arroyo test-arm` green; builder-path `UNAOS_USBDEBUG=1 <same knobs>
./arroyo esp-x86`; strings-proof the new `bw-step` markers in
`target/x86_64_esp/kernel.elf`. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull13.md`, STATUS: PROPOSED).
