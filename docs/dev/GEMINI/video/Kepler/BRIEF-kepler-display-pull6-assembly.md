# BRIEF — kepler-display pull 6: assembly-state hunt (read-only)

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #15 first.

## Where the lane stands

s15 refuted the bare-pointer write: **0x6101E0 is a read-only armed-state
readout** (write snaps back on the next read). EVO semantics on this class:
an ASSEMBLY state is written somewhere else, then an UPDATE latches it into
the armed state that 0x6101E0 reports. This pull finds the assembly side and
the latch's neighborhood — read-only; the latch write is pull 7.

## This pull — three read-only probes

**Milestone 1 — the gap region.** Dense dump of 0x610000–0x6103FC (the space
between PDISPLAY head words and the 0x610480 channel window), two passes,
bounded delay between. This region contains 0x6101E0 itself — its dense
neighborhood (±0x40) is the priority readout: armed-state blocks usually sit
as an array (per-channel or per-head strides); knowing 0x6101E0's neighbors
tells us the record shape.

**Milestone 2 — widened known-value scan.** Re-run the pull-4 scan predicate
(keys incl. 0x200/0x20000/0x90020000/pitch/fbsize/raster/2880/1800 shapes)
over 0x614000–0x61FFFC and 0x640000–0x647FFC (the DISP_USER region the old
refuted code guessed at — read it honestly this time). Same hit markers,
same 64-print cap with total count.

**Milestone 3 — armed-pair check.** Read 0x6101E0 and every hit from M2 once
more AFTER milestones 1–2 complete (a second sample later in the boot): a
word that TRACKS 0x6101E0's value is a mirror; a word holding the same value
that could diverge is the assembly candidate. Print all pairs.

## Exact serial markers (verbatim)

- `:: kdisp: gap pass<P> off=XXX val=XXXXXXXX ::` (off relative to 0x610000)
- `:: kdisp: gap pass<P> done rows=N ::`
- `:: kdisp: evo-scan2 hit off=XXXXXX val=XXXXXXXX key=<name> ::`
- `:: kdisp: evo-scan2 done ranges=614000-61FFFC,640000-647FFC hits=N capped=<t|f> ::`
- `:: kdisp: pair off=XXXXXX first=XXXXXXXX second=XXXXXXXX ::`
- keep all existing markers unchanged

## Gates (DONE = all of these)

Read-only: no register writes anywhere (the pull-5 repoint block is
removed/superseded — refuted code does not re-fly). Full-knob check both
arches + builder-path esp-x86 + `strings` proof of new markers in kernel.elf
+ default QEMU green both. Commit ALL docs+code; delete scratch; `git status`
clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull6.md`, STATUS: PROPOSED).
