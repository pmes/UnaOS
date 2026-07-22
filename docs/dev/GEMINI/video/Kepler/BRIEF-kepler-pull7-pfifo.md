STATUS: BRIEF — awaiting Gemini intro proposal (new arc, new session)

# BRIEF — Kepler wall-2 (PFIFO/PBDMA fence), pull 7 arc intro

Coordinator-authored brief (2026-07-22, post sitting #5). The assigned Gemini
specialist answers this with `PROPOSAL-kepler-pull7.md` (STATUS: PROPOSED)
saved in THIS directory (`docs/dev/GEMINI/video/Kepler/`), following the
review flow in `docs/dev/GEMINI/README.md` — **no implementation commits
before approval**.
This is a **derivation arc**: the proposal must contain the derived offsets
with rnndb/envytools XML citations so facts are checkable before code.

## Where wall 2 stands (sittings #4–#5, KEPLER-METAL-LOG.md — trust, don't re-derive)

Kepler display is PARKED (panel is iGPU-owned); this arc is pure
compute-plumbing: get the first fence write back from the GK107. Real progress
on silicon, one unit still asleep:

Working / confirmed:
- Channel instance block decodes sanely:
  `inst-raw 08=02002000 0C=00000000 48=02001000 4C=01FF0000`;
  `fifo-layout userd=2002000 fence=2014000 gp=1/0`.
- `pbdma-eng-mask set` — the engine-mask write took (new in #5).
- `ch_stat=11000001` — channel ENABLED (+ two RO unknowns at bits 24/28).
- `playlist_rd=00002013` (stable across #4 and #5, a real breadcrumb),
  `playlist_rd_len=00100001` — the scheduler SEES our playlist.

The wall:
- `gp_get=0` always — the PBDMA never fetches GP entry 0.
- `bad-read pbdma 40108 00000000` — PBDMA status at base 0x40000 reads
  clean zero (was 0xBAD0011F poison at the legacy 0x6c0 base in #3, so 0x40000
  is closer, but zero = wrong base OR unit unclocked/never started).
- `pbdma_stat=00000000`.
- `pbdma-count 3` — persists across sittings; GK107 was expected to report 1.
  Either our count-register decode is wrong, or 3 is real and the per-PBDMA
  stride decides which unit serves our runlist.

Diagnosis to beat: the scheduler sees the channel and playlist, but the PBDMA
unit that should service them is either being read at the wrong base or has
never been clocked/started.

## What the intro proposal must derive (with citations)

1. **GK107 PBDMA register base and per-unit stride** — the authoritative
   rnndb answer for where PBDMA unit registers live on GK104-family, and how
   `NV_PFIFO` reports the PBDMA count (explain the observed 3).
2. **Unit start/clock sequence** — what enables the PBDMA: PMC engine-enable
   bits, PFIFO unit enable, per-PBDMA enable/init registers, and the ordering
   relative to runlist submit. The proposal states the exact writes, in order,
   each cited.
3. **Runlist↔PBDMA binding** — which PBDMA serves our runlist, and how to read
   that binding back, so instrumentation can prove we're watching the right
   unit before we conclude anything from its status.
4. **Instrumentation-first milestone** — pull 7's first commit extends the
   existing dual-instrument pattern: read PBDMA status/get-put at the derived
   base for ALL reported units, plus the enable/clock state, so sitting #6
   falsifies the base/clock hypothesis in one boot before any enable writes.

## Standing rules (these have been violated before — they will be checked)

- **Cleanroom:** rnndb/envytools XML facts only. Nouveau GPLv2 code, function
  names, and magic masks are forbidden (the 0x2a04=0xbfffffff and
  "derived from nouveau/gf119.c" incidents are on the record). While in the
  neighborhood: the `kepler.rs` ~465 EVO-offset comment still carries nouveau
  attribution — replace with an rnndb citation or an honest empirical note as
  part of this pull's cleanup, not new derivation.
- **No circular derivations** — do not re-propose addresses already refuted on
  metal (legacy 0x6c0 PBDMA base; both wall-1 scanout candidates).
- **CHANGES REQUESTED means changed** — reshipping a rejected item unchanged
  ends the review.
- **Honesty lines** — "NOT COMPILED HERE — Mac owed" for anything not built
  locally; the reviewer builds on the Mac, Fox flies it at sitting #6.

## Lane

`kepler.rs` + its existing instrumentation/builder wiring only. No display
(PDISPLAY/EVO) work beyond the citation cleanup above; no iGPU files (that is
the other new arc).
