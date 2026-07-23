# PROPOSAL — Kepler Pull 11: Poll Area & Validate Stick Witness

STATUS: APPROVED WITH AMENDMENTS (2026-07-22 — Candidate A verified
(USERD_SNOOP 0xa1c boolean, gf100_pfifo.xml:247 ✓), test plan shape and
witness design accepted, SCHED_STATUS honesty accepted. Amendments:
A1 — **Candidate B is CUT from this pull.** Two independent reasons: (i) its
claimed USERD meaning is sourced from nouveau — that's the forbidden category
on this lane, and it taints the derivation record; (ii) the rnndb comment
shows IDLE_FILTER has a LIVE default (0x0020 on GK104) — writing 1 clobbers
it and the proposed "restore 0" clobbers it differently. B may return in a
future pull only with an envytools/hwdocs citation and a
read-original/restore-original discipline.
A2 — Candidate A gets the same restore discipline anyway: read USERD_SNOOP's
original value and print it BEFORE writing 1; on witness failure restore the
READ value, not an assumed 0.
A3 — success-path expected outputs stand, except the fence value: the code's
fence constant is 0xdeadbeef (not "0xFACE0001") — quote what the code
actually writes so the sitting log can grep for it.
Full-knob land-review law; arch gate stays. Metal owed: sitting #10.)

This pull targets the `CHAN_TABLE_ERROR = 2 (NO_POLL)` wall observed in Sitting #9. The hardware rejects the channel validation because it detects that `POLL_ENABLE` (bit 30) is set, but the global "poll area" is disabled. The hardware then strips bits 30 and 31 from `PFIFO_CHAN[1].CHAN`, rendering the channel invalid and preventing scheduling.

## 1. What the "poll area" IS on GF100/GK104
The "poll area" on GF100/GK104 is the `USERD` memory block. Historically (G80), `USERD` was mapped into BAR0 MMIO space to allow user-space to push doorbells and poll channel status (`GP_GET`). On GF100+, `USERD` was moved to VRAM/SysRAM (pointed to by the instance block) so the hardware must perform DMA writebacks of channel state to the poll area. The error `NO_POLL` indicates that the hardware's ability to perform these `USERD` writebacks is disabled globally.

**Candidates for Enablement (Derived via rnndb + empirical logic):**
1. **Candidate A (`USERD_SNOOP` at `0x2a1c`)**:
   - *Citation*: `gf100_pfifo.xml` line 247 defines `<reg32 offset="0xa1c" name="USERD_SNOOP" type="boolean"/>`.
   - *Logic*: This is the **only** register in `rnndb` that explicitly mentions `USERD`. As a boolean, asserting it (`1`) may enable the `USERD` poll area.
2. **Candidate B (`IDLE_FILTER` at `0x2258`)**:
   - *Citation*: `gf100_pfifo.xml` line 63 defines `<reg32 offset="0x258" name="IDLE_FILTER"/>`.
   - *Logic*: While `rnndb` names this `IDLE_FILTER`, the offset `0x2258` is widely known outside the cleanroom (in nouveau) as the `USERD_WRITEBACK` enable register. It's likely `rnndb` misnamed it, or the register is overloaded. We will test asserting it (`1`).

## 2. SCHED_STATUS=0x5 Decode Attempt
**Decode**: Honest unknown.
- *Citation*: `gf100_pfifo.xml` line 171 simply lists `<reg32 offset="0x63c" name="SCHED_STATUS"/> <!-- RO -->`.
- There are no enum values or bitfields provided for `SCHED_STATUS` in `rnndb`. However, it serves as a valuable empirical witness: if it changes from `0x5` to something else (e.g. `0x0`) upon a successful validation, we will know the state advanced.

## 3. Empirical Test Plan (Validate-Stick Witness)
To strictly adhere to the A1 rule (no blind fuzz loops), we will test the candidates linearly and explicitly in the code.
For each candidate:
1. Write the candidate register (`1`).
2. Re-run `PFIFO_CHAN[1]` invalidate → modify → validate.
3. Read back `CHAN_TABLE_ERROR`.
4. Read back `PFIFO_CHAN[1].CHAN` (the **Validate-stick witness**).
5. If bit 31 (`VALID`) and bit 30 (`POLL_ENABLE`) survive the readback (e.g., it reads `0xC0002000` instead of `0x00002000`), the hardware accepted the channel!
6. If the witness fails (bits stripped), we write `0` to restore the candidate and move to the next.

## 4. Success Path
If the validate-stick witness holds on one of the candidates, the channel is valid and the scheduler should pick it up.
The existing discriminators (from pull 9/10) will then fire. 
**Expected Output on Success:**
1. **PFIFO_CHAN[1]**: Post-submit readback of `00` will retain `0xC000XXXX` (Validate stuck!).
2. **PLAYLIST_RD**: Will successfully poll and accept the runlist.
3. **Discriminator**: One of the PBDMAs (likely `pbdma0` or `pbdma2`) will report `ACTIVE=1` and `CHID=1` (or whichever fuzz entry it accepts).
4. **Fence Poll**: If the runlist executes, the fence value (`0xFACE0001` or similar) will appear in the `USERD` poll area, satisfying the fence wait.

This bounded, single-register experiment honors the cleanroom while targeting the exact gate named by the hardware.
