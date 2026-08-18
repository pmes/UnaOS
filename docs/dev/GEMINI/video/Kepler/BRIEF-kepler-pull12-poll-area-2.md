STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-kepler-pull12.md`, this directory)

# BRIEF — Kepler pull 12: NO_POLL round two — the poll area is probably per-channel state, not a global knob

Coordinator-authored (2026-07-22, post sitting #10 boot 2).

## Facts (KEPLER-METAL-LOG.md #10)

- USERD_SNOOP (candidate A) refuted: orig=0, write-1 reads back 0, witness
  failed, err stays 2. Restored, no residue.
- Refined write-behavior split: eng-masks and playlist writes STICK;
  PFIFO_CHAN VALID/POLL strip is the chip's documented NO_POLL refusal;
  USERD_SNOOP write-void is unexplained (possibly absent on GK107 — rnndb is
  silent on variants).

## Derivation priorities (strongest hypothesis first)

1. **Poll area as per-channel INSTANCE state.** On GK104+ USERD lives in
   VRAM pointed to by the channel's instance block. Derive the complete
   GK104 instance-block/RAMFC field list relevant to USERD/polling: is there
   a USERD-valid/poll-enable field IN the inst block (or in the channel
   table's INST word beyond ADDRESS/TARGET) that we never set? Audit every
   inst-block dword we write against the derived layout, and list fields the
   layout defines that we leave zero. Primary sources: rnndb memory/inst
   XMLs + envytools hwdocs fifo documentation (hwdocs is an allowed
   cleanroom source — cite file+section).
2. **PFIFO reset/unlock handshake** — is there an engine-reset-done or priv
   config-unlock sequence for the scheduler config block beyond
   PMC_ENABLE.PFIFO? Cite or mark honestly absent.
3. **USERD_SNOOP existence test (instrumentation only)** — read it on GK107
   before any write in the new pull; if it reads back written values under a
   hypothesis-2 unlock, that's a signal; otherwise record "absent/inert on
   this part" and stop touching it.

## Shape

Same disciplines as pulls 10/11: every candidate write individually listed in
the proposal with citation, printed before/after with CHAN_TABLE_ERROR +
witness readback, restore-original on failure, no fuzz. Success path ends at
the 0xdeadbeef fence via the existing machinery. Full-knob land-review; arch
gate stays. Metal owed: sitting #11 (rides with display pull 6).
