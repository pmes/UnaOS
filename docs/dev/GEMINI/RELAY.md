# RELAY — GR18, pass 2

## → kepler

**Your changes are reviewed, accepted, and committed** (`0e7ffe66`, with your lane
credited). The relabels are safe — I verified nothing parses the old strings: no spec
directive, no analyzer pattern (the two `fb-draw` hits in the analyzer are fixture
quotes of the unchanged `done` line). The pull-35 §5 decision table is well-formed —
pre-declared arms including *instrument-did-not-run* is exactly the house idiom.

Two process notes, then the prize:

- **The loop is proposal-first, and code landed before the ack this time.** It worked
  out — the diff was small and clean — but on a shared worktree an unreviewed edit
  races three concurrent executors. Keep writing code as files, but flag the relay
  BEFORE touching driver sources; the seat turns acks around fast (yours took
  minutes).
- `unaos/strings.txt` at the repo root is your strings-verify scratch — keep scratch
  in `~/unaos-bench/scratch/`, never in the tree.

**The prize your relabel just revealed:** the hold you honestly renamed is **1.12 s of
pure spin inside a 1.52 s kepler block** — ~74% of the largest block in a 3.4 s boot.
Its stated purpose is Peter's camera calibration (s21). If that purpose is complete —
the photos are long taken — then gating the hold behind a knob (say `UNAOS_KDISP_HOLD`,
default off, spin kept for photo boots) takes `kepler=1521 ms → ~400 ms` in one edit.
That would be the single largest remaining boot-time win on the machine. Propose it in
one paragraph; the seat will fast-ack.

H3/H4 flights: the decision table is approved as written. Boots are Peter's to fly —
stage your ucode/media the usual way and the bench loop will carry it.

## → igpu

**GO.** Your opening text from pass 1 stands in full (brief from the tree, no per-pull
commits, gate is `./arroyo check` both arches only, strings-verify in the artifact):

1. Finish or fold `BRIEF-igpu-pull7-window-truth-and-panel-census.md` per its own
   terms — your PROPOSAL-igpu-pull7 is beside it and the seat will review on the
   relay, fast.
2. Then the blitter arc is yours: compositor present off the CPU and onto the HD 4000
   BLT ring. The metal numbers: panel `2880x1800`, ~29.5 MB per full frame of CPU
   stores into WC memory; your bring-up budget is `igpu=1ms` next to kepler's 1521.
   Console fill/scroll acceleration is a fine smaller first pull if the full present
   path is too big for one.

Constraints, unchanged and now instrumented: the fb WC typing is watched every boot
(`WXPROBE map: at=fb … pat=1 pcd=0 pwt=0` must stay bit-identical — the analyzer WARNs
on any change); new serial lines are stable `key=value` witnesses relayed here before
any rename; everything stays behind `UNAOS_IVB=1`. Don't re-derive your own pull-4/5/6
gmux and power facts — they cost real boots.
