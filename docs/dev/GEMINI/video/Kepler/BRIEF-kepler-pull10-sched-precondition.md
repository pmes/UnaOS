STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-kepler-pull10.md`, this directory)

# BRIEF — Kepler pull 10: read the chip's own reject reason, then satisfy the scheduling precondition

Coordinator-authored (2026-07-22, post sitting #8 boot 2).

## Where the wall is now (KEPLER-METAL-LOG.md #8)

Runlist parse proven fine (all 3 entries read and counted); all three entry
encodings refuted as sufficient; no hardware writeback to PFIFO_CHAN or
RAMFC. The gate is a per-channel scheduling precondition our setup fails.

## Reviewer-verified leads (gf100_pfifo.xml — cite and extend)

1. **CHAN_TABLE_ERROR, PFIFO+0x52c** — a readable reject-reason register:
   `1 CHANNEL_IN_USE / 2 NO_POLL ("validated a channel with POLL_ENABLE, but
   poll area is disabled") / 5 NO_ENGINE / 6 INVALID_TARGET /
   0xb CHANNEL_RUNNABLE`, etc. We have NEVER read it. Instrumentation first:
   dump it (and `SCHED_STATUS`, +0x63c RO) after channel-table write and
   after runlist submit.
2. **The GF100 CHAN_TABLE decode** (0x1000-era array): CHAN word = INST +
   bit30 `POLL_ENABLE` + bit31 `VALID`; STATE bit0 = `RUNNABLE`. GK104's
   PFIFO_CHAN names bit31 `UNK31` — derivation task: establish (or fuzz) the
   GK104 analogs. We currently write bit31 but never bit30; the NO_POLL
   error code suggests exactly our symptom class.
3. **"Poll area" enablement** — what the NO_POLL code calls "poll area"
   (USERD polling config) and how it is enabled on GK104: derive the
   register(s), or the honest empirical plan if rnndb lacks it.
4. **Ordering per CHAN_TABLE_ERROR semantics** — several codes fire on
   modify-while-validated; derive the write order that avoids IN_USE/ACTIVE
   violations (invalidate → modify → validate?).

## Shape

Instrumentation (error/status readbacks) FIRST and unconditional — one boot
of reject-reason evidence outranks another blind fix. Precondition writes
(bit30, poll-area enable, revised order) in the same pull, each printed
before/after with the error register re-read, so the sitting reads as
cause→effect per write. Bounded polls; `:: kepler:` prefixes; full-knob
land-review law (strings-proof both artifacts; keep the main.rs arch gate).

Metal owed: sitting #9 (rides with igpu pull 4).
