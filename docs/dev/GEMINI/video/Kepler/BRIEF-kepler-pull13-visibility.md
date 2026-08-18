STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-kepler-pull13.md`, this directory)

# BRIEF — Kepler pull 13: does the scheduler SEE our instance bytes?

Coordinator-authored (2026-07-22, post sitting #11 boot 2).

## Facts (KEPLER-METAL-LOG.md #11)

- USERD_HI bit31 refuted as the poll enable (err=2 with the bit provably set
  in instance memory).
- Precision gained: instance-memory writes PERSIST as seen through BAR1; the
  strip is only ever on the PFIFO_CHAN MMIO word — the chip's documented
  NO_POLL refusal. But BAR1 readback proves only BAR1 self-coherence: whether
  the SCHEDULER's engine-side view of VRAM sees our bytes at validate time is
  unproven. If validate reads a stale/cached view of the instance block, the
  chip could be judging a zeroed entry no matter what we write.

## What the proposal must derive

1. **VRAM write → engine visibility** on GF100/GK104: the cited
   flush/serialization mechanism between CPU BAR1 stores and engine-side
   reads (PFB flush register, BAR flush doorbell, read-serialization via a
   PRI register — whatever the cleanroom sources actually document; rnndb
   bus/memory XMLs + envytools hwdocs in scope). Each candidate cited; if
   none exists in cleanroom sources, say so honestly and propose the
   cheapest empirical serialization (e.g. an MMIO read of a PFB register
   between write and validate) as a labeled experiment.
2. **Witness experiment** — insert the derived flush between instance-block
   writes and the validate; re-run the s10-era ladder unchanged otherwise
   (single variable). Print the flush action taken. Ladder and markers as
   before ("WITNESS PASSED - bits stuck!", "sched-status post-restore").
3. **Fallback framing** — if flush changes nothing, the poll area is
   genuinely elsewhere: the proposal should pre-commit the next derivation
   surface (core-channel/USERD config in the disp-era state tables? PBDMA
   CTRL_ADDR target bits audit — TARGET enum vals are marked XXX-unconfirmed
   in the XML) so pull 14 needs no fresh brief.

Exact grep-able markers listed in the proposal, per the new rule. Standing
rules unchanged. Metal owed: sitting #12 (rides with display pull 2).
