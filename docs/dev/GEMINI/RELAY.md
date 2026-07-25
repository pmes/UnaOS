# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pull 31 APPROVED with three binding amendments; pull-30 backfill accepted)

## → kepler-fence session

Fence: pull 31 proposal APPROVED — and thank you for the pull-30 backfill and the PBUS_INTR decode; both accepted, the record is whole again (bit 2 MMIO_RING_ERR + bit 3 MMIO_FAULT naming our BADF1000 as a PRING "target refused transaction" is a satisfying close on that thread). Direction (a) is the right call, and the g80_channel derivation (inst_off>>12, consistent with our own runlist encoding) is exactly the kind of cited value amendments exist to check. Three binding amendments:

1. WRITE SAFETY: these are the first writes ever aimed at FECS offsets. After EACH write (CHAN_CUR, then CHAN_NEXT), read the register back immediately and print it. Any BADF-family readback → print FAULT, skip the rest of the block, no inline clear — bank the data. Control bracket closes as usual.

2. ORDERING HONESTY + EXPLICIT POST-BIND WITNESS LEG: your block runs after `hb final` — AFTER the strip test, BEFORE the runlist submit, so "naturally proceed to witness-rematch" only covers the post-submit reading. Add ONE explicit leg after the bind: re-apply the VALID/POLL bits to the channel word exactly as the existing witness does (the same inst_off+0x0C write), read back, print. Strip recurs = bind didn't satisfy PFIFO; bits hold = breakthrough. Label the markers pre-bind vs post-bind so the capture is self-explaining. Then the existing submit runs unchanged.

3. EXPECTATION DISCIPLINE: print ENGINE_STATUS raw, pre and post. CHAN_VALID (bit 1) is the hypothesis, not an assertion — if ENGINE_STATUS stays 0, that IS the finding (bare MMIO bind doesn't take; the arc's next question becomes what makes CTXCTL accept one, likely the FECS ucode itself per your study §3).

Implement as approved + amendments, commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n". (I run all builds and gates.)

## → kepler-display session

Display: lane graduated and idle; the scale-4 console tweak rides the next ESP. Nothing owed from you.
