# BRIEF — kepler-display pull 14: block-width × aligned-pitch matrix

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #23 first.

## The s23 verdict this pull acts on

Block width is REAL: at bw>1 the uniform periodic brick seams vanished —
photos show long clean band runs with seams clustered in narrow x-regions.
The mapping is close; one interaction is still wrong. Prime suspect:
pitch padding × bw. Pitch alignment was refuted ONLY at bw=1 (s22); at
bw=2/4 the natural 180 GOBs/row gives 90/45 blocks per row (45 is odd),
and the hardware may pad the surface to an aligned blocks-per-row. The
two hypotheses were never tested together. Cleanest s23 config was
(bw=2, bh=4) — bh=4 is fixed for this matrix.

## This pull — four cycles, bh=4 throughout

Same latch/restore discipline, 5 s holds, 1 s gaps. Cycles in order:
(bw=2,pg=192), (bw=2,pg=256), (bw=4,pg=192), (bw=4,pg=256).

Per cycle: identical index math to pull 13 (x-fastest within-block,
cited ordering stands) with `pg` the PADDED GOBs/row: blocks_per_row =
pg/bw (both pgs divide by both bws), padded pixels x>=2880 filled BLACK
(real bytes, as in pull 12), visible image 2880 px. Print computed bytes.

Markers:
`:: kdisp: bwpg-step bw=<W> bh=4 pg=<G> fill done bytes=NNNNNNNN ::`
`:: kdisp: bwpg-step bw=<W> bh=4 pg=<G> hold t=<n>s ::` (t=1..5)
`:: kdisp: bwpg-step bw=<W> bh=4 pg=<G> done ::`

Bench: one photo per hold, tagged from serial. Verdict key: ZERO seams +
solid white left column names (bw,pg) — with bh=4 that completes the
mapping. If all four still cluster, record cluster positions per photo;
the fallback suspect is a non-power-of-2 blocks-per-row rule (e.g. hw
rounds blocks_per_row itself, not gobs_per_row).

## Gates (DONE = all of these)

Writes remain exactly 0x640460 + 0x640080 (×4 cycles, restore-paired).
Full-knob `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1
UNAOS_KEPLER_FIFO=1 ./arroyo check` both arches; default `./arroyo test`
+ `./arroyo test-arm` green; builder-path `UNAOS_USBDEBUG=1 <same knobs>
./arroyo esp-x86`; strings-proof the new `bwpg-step` markers in
`target/x86_64_esp/kernel.elf`. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull14.md`, STATUS: PROPOSED).
