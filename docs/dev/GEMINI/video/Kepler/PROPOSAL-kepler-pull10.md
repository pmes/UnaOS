# PROPOSAL — Kepler Pull 10: Sched-Precondition & Reject Reason

STATUS: LANDED 2650a38b (2026-07-22 — error-readback at pre-init/post-init/
post-submit with "absent?" honesty per A2, invalidate→modify→validate order,
bit30 POLL_ENABLE asserted, no fuzz rider per A1. Gates green, strings-proven.
Metal owed: sitting #9.)
Prior: APPROVED WITH AMENDMENTS (2026-07-22 — error-readback-first shape,
invalidate→modify→validate ordering, and the bit30 hypothesis all accepted;
the honest "no USERD-enable register in rnndb" is noted. Amendments:
A1 — NO undocumented-register write fuzzing in this pull: if the readback
says NO_POLL, print it and STOP; the 0x2200-0x22FF fuzz idea is a separate
proposal needing its own approval (blind writes to undocumented PFIFO config
on the only bench GPU is not a rider).
A2 — CHAN_TABLE_ERROR/SCHED_STATUS carry no GK104- variant tag in the XML and
sit beside GF100:GK104-only registers; they may not exist on GK107. Treat a
0/poison readback as "register absent" and SAY so in the serial output —
don't canonize a zero as "no error".
Full-knob land-review law; keep the main.rs arch gate. Metal owed: sitting #9.)

This proposal addresses the final Kepler scheduling precondition gate. As verified in Sitting #8, the runlist is successfully read (`playlist_rd=0x2013`), but the hardware refuses to schedule the channel. We will implement strict `CHAN_TABLE_ERROR` instrumentation to ask the hardware directly *why* it rejects the channel, while simultaneously correcting the `PFIFO_CHAN` enablement ordering to satisfy known hardware semantics.

## 1. Instrumentation: CHAN_TABLE_ERROR & SCHED_STATUS
**Derivation:**
- `gf100_pfifo.xml` defines `CHAN_TABLE_ERROR` at offset `0x52c` (mapped to `0x252c` in BAR0) and `SCHED_STATUS` at offset `0x63c` (`0x263c`).
- `CHAN_TABLE_ERROR` holds discrete failure codes (`1 CHANNEL_IN_USE`, `2 NO_POLL`, `5 NO_ENGINE`, `6 INVALID_TARGET`, `0xb CHANNEL_RUNNABLE`).
- **Plan:** We will read `0x252c` and `0x263c` at three points: (1) before `PFIFO_CHAN` initialization, (2) immediately after, and (3) after the runlist submission/PLAYLIST_RD poll. This provides absolute proof of which precondition fails.

## 2. Ordering per CHAN_TABLE_ERROR Semantics
**Derivation:**
- Error code `1 CHANNEL_IN_USE` fires when the host "modified a validated channel".
- In our current `kepler.rs`, we write `0x800000` (`CHAN`, setting `UNK31` which maps to `VALID`), and *then* we write `0x800004` (`STATE`, setting `ENABLE_TRIGGER`).
- This order explicitly violates the `CHANNEL_IN_USE` semantic. We are modifying the state of a channel that is already flagged as valid.
- **Fix:** We will invert the initialization order. 
  1. Invalidate: `mmio_write(bar0, 0x800000 + ch, 0)`
  2. Modify: `mmio_write(bar0, 0x800004 + ch, 0x00000400)`
  3. Validate: `mmio_write(bar0, 0x800000 + ch, VALID | INST)`

## 3. CHAN_TABLE Decode & POLL_ENABLE (Bit 30)
**Derivation:**
- GF100 `CHAN_TABLE` offset 0 defines bit 31 as `VALID` and bit 30 as `POLL_ENABLE`.
- GK104 `PFIFO_CHAN` offset 0 names bit 31 `UNK31` (which we currently write, confirming it is `VALID`), but leaves bit 30 unlabelled.
- The `gf100_pfifo.xml` comments state: `POLL_ENABLE /* XXX what good is a channel without this? */`.
- We currently write `0x80000000` (bit 31 only). Without `POLL_ENABLE`, the scheduler may silently ignore the channel (as it has no valid poll area config).
- **Fix:** We will write `0xC0000000 | (inst_off >> 12)` to assert both `VALID` (bit 31) and `POLL_ENABLE` (bit 30). 

## 4. Poll Area Enablement
**Derivation & Empirical Plan:**
- If a channel is validated with `POLL_ENABLE`, but the global "poll area" (`USERD`) is disabled, the hardware throws `CHAN_TABLE_ERROR` = `2` (`NO_POLL`).
- An exhaustive cleanroom search of `rnndb` (`gf100_pfifo.xml`, `g80_pfifo.xml`, etc.) reveals NO documentation for a global `USERD` enable register on GK104. It is entirely absent from the `rnndb` facts.
- **Honest Empirical Plan:** We will assert `POLL_ENABLE` (bit 30) on our channels and observe the `CHAN_TABLE_ERROR` readback. 
  - If we get `NO_POLL` (2), it confirms the global poll area is disabled. We will then need a follow-up test to fuzz the PFIFO `0x2200`-`0x22FF` space for the undocumented enable register (e.g., `0x2258`).
  - If we get `0` (Success), it means the poll area is already enabled by default or the GOP driver, and the scheduler will proceed.

## Implementation Shape
All reads will be printed with `:: kepler: sched-status ::` prefixes. The implementation will fully replace the `PFIFO_CHAN` initialization loop and add the bounded error readbacks, changing no other logic.
