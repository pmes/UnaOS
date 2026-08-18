STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-kepler-pull9.md`, this directory)

# BRIEF — Kepler wall-2 pull 9: runlist entry / channel-bind, round two

Coordinator-authored (2026-07-22, post sitting #7). Rules unchanged:
derivation proposal first, every offset cited to rnndb/envytools XML,
no code before approval. Full-knob land-review law applies.

## What sitting #7 settled (KEPLER-METAL-LOG.md)

- Pull-8's ORDER fix TOOK on silicon (`inst-raw 4C=0x00090000`) and is
  **refuted as the bind wall**: runlist is read (`playlist_rd=0x2013
  len=0x100001`), channel ENABLED, yet all three PBDMAs stay `ch=0 ACTIVE=0`,
  `gp_get=0`, fence-timeout. The channel is never scheduled onto any PBDMA.
- Clocks/enables/eng-masks all proven fine (#6). PBDMA base/stride proven
  (#6-#7). The remaining unknowns are exactly the runlist-entry and bind
  mechanics.

## What the proposal must derive (each with citations)

1. **Runlist entry encoding on GK104-family** — full 2-dword (or however
   many) entry layout: what goes in each field for a channel entry, how the
   channel ID is encoded, what a scheduler treats as a null/skip entry. Then
   audit our actual entry write in kepler.rs against it dword by dword.
2. **Channel table / RAMFC validation** — what the scheduler checks in the
   channel's state before it will schedule it (channel-table entry at
   0x800000+, instance pointer/target bits, any required "bound to engine"
   or context-valid bits we never set). Audit every field we DO set against
   the layout; list fields present in the layout that we never touch.
3. **Submit/enable ordering** — the canonical sequence (channel-table write →
   enable → runlist submit vs other orders), and whether a runlist submitted
   BEFORE some prerequisite is read-but-ignored (matches our exact symptom:
   playlist_rd advances, nothing scheduled).
4. **Which-runlist** — engine runlist vs runlist 0 on GK107, and how the
   entry/register choice differs.
5. **Instrumentation delta** — whatever new readbacks distinguish the
   hypotheses in one boot (e.g. channel-table entry readback after enable,
   scheduler status regs beyond playlist_rd).

## Notes

- The dedupe + prefix + poll-bound hygiene is done (94b0ed0c + follow-up);
  don't re-ship it.
- Takeover stays armed at sitting #8 — its polls are now bounded; no display
  work in this pull beyond leaving takeover untouched.

Metal owed: sitting #8.
