# PROPOSAL — Kepler Pull 12: Poll Area 2 (Instance Block)

STATUS: APPROVED WITH AMENDMENTS (2026-07-22 — the honest "no cleanroom
RAMFC layout exists for GF100/GK104" finding is accepted and recorded; the
single logically-derived bit test (USERD_HI bit31) is exactly the right size,
and the restore+re-test discipline is right. Amendments:
A1 — after the failure-path restore and re-test, read and print
CHAN_TABLE_ERROR one more time so "no residue" is evidenced, not asserted.
A2 — USERD_SNOOP scrub approved; keep one comment line in kepler.rs noting
0x2a1c was tested inert on GK107 (sitting #10) so it never gets re-proposed.
Full-knob land-review; arch gate stays. Metal owed: sitting #11.)

This pull addresses the `NO_POLL` (2) rejection observed in Sitting #10. With global MMIO candidates refuted or undocumented, we pivot to the hypothesis that the "poll area" (USERD) is enabled via per-channel state within the instance block.

## 1. Poll area as per-channel INSTANCE state
**Derivation effort**: Exhaustive searches of `rnndb` (`fifo`, `memory`, `graph`) and `envytools` `hwdocs` reveal that the GF100/GK104 channel instance block and RAMFC layout are **not documented in cleanroom sources.** 
- `rnndb` contains a `G80_RAMFC` domain, but no `GF100_RAMFC` or `GK104_RAMFC`. 
- The `g80_channel` bitset used by `PFIFO_CHAN.CHAN` only defines `ADDRESS` (28 bits) and `TARGET` (2 bits), leaving bits 30 and 31 unnamed (which we know are `POLL_ENABLE` and `VALID`).
- We write to offsets `0x08`, `0x0C`, `0x10`, `0x30`, `0x48`, `0x4C`, `0x84`, `0x94`, `0x9C`, `0xAC`, `0xE4`, `0xE8`, `0xB8`, `0xF8`, `0xFC` in the instance block, but cannot audit them against a cleanroom layout because none exists.

**Proposed Bounded Test**: Since we cannot blindly fuzz, we will perform a single, logically-derived test. We currently write the `USERD` pointer to the instance block at `0x08` (low) and `0x0C` (high). The upper word `0x0C` only requires bits 0-7 to store a 40-bit address; bits 8-31 are zeroed.
A common Nvidia idiom is embedding a `VALID` or `ENABLE` bit in the high word of a pointer. We will test setting **bit 31 of `inst+0x0C`** (`USERD_HI`).

## 2. PFIFO reset/unlock handshake
**Derivation effort**: Honestly absent. A review of `rnndb/bus/pmc.xml` and `rnndb/bus/pbus.xml` shows no undocumented unlock sequences or privileged handshake registers for `PFIFO`. The only reset mechanism is the standard `PMC_ENABLE.PFIFO` toggle, which we already perform correctly.

## 3. USERD_SNOOP (0x2a1c)
**Action**: Inert and abandoned. Sitting #10 proved that `USERD_SNOOP` reads as `0`, ignores writes of `1`, and leaves no residue. With no unlock handshake found to gate it, it is functionally absent or inert on GK107. The codebase will be scrubbed of `0x2a1c` entirely.

## 4. Empirical Test Plan (Validate-Stick Witness)
This pull will adapt the Pull 11 witness machinery to test the instance block:
1. Remove `USERD_SNOOP` (0x2a1c) reads/writes.
2. Modify the `USERD_HI` write at `inst_off + 0x0C` to set bit 31: `(userd_off >> 32) as u32 | 0x80000000`.
3. Submit the channel to `PFIFO_CHAN[1]`.
4. Read back `PFIFO_CHAN[1].CHAN` (Validate-stick witness).
5. If bits 31 and 30 strip (witness fails), we will immediately restore `inst+0x0C` to its original value `(userd_off >> 32)` and re-test `PFIFO_CHAN[1]` to clear the state, ensuring no residue.
6. If the bits stick, we proceed to the fence poll machinery expecting `0xdeadbeef`.
