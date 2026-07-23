STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-kepler-pull8.md`, this directory)

# BRIEF — Kepler wall-2 pull 8: the channel is never scheduled onto a PBDMA

Coordinator-authored (2026-07-22, post sitting #6). Same rules as pull 7:
derivation proposal first with rnndb/envytools XML citations for every offset,
STATUS: PROPOSED, no code before approval. Pull 7's citation discipline was
exemplary — keep it.

## What sitting #6 proved (KEPLER-METAL-LOG.md — trust, don't re-derive)

Pull 7's instrumentation answered everything it was built to ask:
- All three PBDMAs: `ch=0 ACTIVE=0, ib_put=ib_get=0` — **no PBDMA ever binds
  our channel.**
- Clock/enable theory DEAD: `PMC_ENABLE=0xE011216D` (PFIFO=1),
  `SUBFIFO_ENABLE=0x7`, eng-masks set (pbdma0=0x01, pbdma1=0x6E, pbdma2=0x10).
- Scheduler reads the runlist (`playlist_rd=0x2013 len=0x100001`), channel is
  ENABLED (`ch_stat=0x11000001`), yet `gp_get=0` and the fence times out.

The wall is upstream of PBDMA: the runlist-entry/channel-bind step.

## What the proposal must derive (candidates, from the sitting synthesis)

a. **GK107 runlist entry format** — exact encoding vs our channel id; is our
   entry well-formed?
b. **RAMFC/instance-block fields the scheduler validates before binding** —
   decode the observed `inst-raw 08=0x02002000 0C=0 48=0x02001000
   4C=0x01FF0000` against the rnndb layout and identify missing/wrong fields.
c. **Runlist submit/commit ordering vs channel-enable** — required sequence.
d. **Which runlist** — does the channel need the ENGINE's runlist rather than
   runlist 0 on GK104-family?

Also fold in: dedupe the double igpu/kepler probe run (PCI walk revisits the
device — guard init against re-entry; observed on serial both boots).

## Standing rules

Cleanroom rnndb facts only; no nouveau code/names/masks. No re-proposal of
refuted addresses. Full-knob law applies to the land-review: gate with
`UNAOS_IVB+UNAOS_KEPLER+UNAOS_KEPLER_TAKEOVER+UNAOS_KEPLER_FIFO` armed AND
strings-proof in the builder-path kernel.elf. Metal owed: sitting #7.
