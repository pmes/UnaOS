STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-kepler-pull11.md`, this directory)

# BRIEF — Kepler pull 11: enable the poll area, make VALIDATE stick, watch the fence

Coordinator-authored (2026-07-22, post sitting #9). The chip itself named the
wall: CHAN_TABLE_ERROR=2 NO_POLL at channel-validate time.

## Facts to build on (KEPLER-METAL-LOG.md #9)

- NO_POLL fires at VALIDATE (post-init, before submit). The hardware REJECTS
  the validate and strips the bits: we write `0x80002000`/`0xC0002000` to
  PFIFO_CHAN[1].CHAN and read back `0x00002000` — bit31/30 cleared by the
  chip, not lost by us. Cause: "poll area is disabled".
- CHAN_TABLE_ERROR (0x252c) and SCHED_STATUS (0x263c) both live on GK107;
  post-submit stat=0x00000005 is undecoded.
- gf100_pfifo.xml contains NO poll-area config register — the only "poll"
  mentions are the error code and the entry bit.

## What the proposal must derive

1. **What the "poll area" IS on GF100/GK104** — the USERD/BAR1 polling
   machinery: where user-space (or host) polling of channel doorbells is
   configured, and which register(s) enable it. Search wider than
   gf100_pfifo.xml: BAR/PBUS/PFIFO-adjacent files in envytools, hwdocs
   (`envytools/hwdocs/fifo/*`), and the G80-era poll-area concept it
   descends from. Citations for every candidate; empirical fallback plan
   (readback-verified single-register experiments, NOT blind fuzz — same
   A1 rule as pull 10: each candidate write individually approved in the
   proposal, printed before/after with CHAN_TABLE_ERROR re-read).
2. **SCHED_STATUS=0x5 decode attempt** — bits, if derivable; honest unknown
   otherwise.
3. **Validate-stick witness** — after each candidate poll-area enable, re-run
   invalidate→modify→validate and print the CHAN word readback: the moment
   bit31 (and 30) SURVIVE readback, the precondition is satisfied; then the
   existing discriminators/fence machinery answers the rest in the same boot.
4. **Success path** — if validate sticks and a discriminator shows a CHID,
   the fence poll is already in place. State expected outputs for the full
   win so the sitting log can quote them.

Bounded polls, `:: kepler:` prefixes, full-knob land-review law, arch gate
stays. Metal owed: sitting #10 (rides with igpu pull 5).
