# PROPOSAL — Kepler Pull 9: Runlist-Entry & Channel-Bind Derivation (Round 2)

STATUS: LANDED 861b116c + 80ede042 (2026-07-22 — fuzz implemented per A1/A3:
distinct CHIDs 1/2/3(/7 both-decodes), all initialized in PFIFO_CHAN,
DISCRIMINATOR readback per PBDMA, PFIFO_CHAN pre/post dumps, RAMFC post-submit
dump. Gates green, strings-proven. Metal owed: sitting #8.)
Prior: APPROVED WITH AMENDMENTS (2026-07-22 — reviewer verified PFIFO_CHAN
against envytools master: 0x800000 stride 8 length 0x1000 GK104-, CHAN
INST 0-29 + UNK31, STATE ENABLED/UNK9/ENABLE_TRIGGER/ENGINE 16-19 — the §2
audit is exact. The honest "rnndb has no runlist-entry layout" + empirical
fuzz plan is the right cleanroom move. Amendments:
A1 — the fuzz needs a DISCRIMINATOR: after submitting the 3-entry runlist,
read back each PBDMA's CH register (CHID field) and print it — the CHID that
appears identifies WHICH encoding won without a second boot. Entries must use
distinct CHIDs for that to work; ensure the three encodings resolve to
distinguishable CHID values, and print the entry table as written.
A2 — §1's exploratory prose (the "but wait—" chain) must not reach the
implementation: the code implements exactly the §5 plan; any deviation goes
in the REPORT, per the standing flow.
A3 — configure PFIFO_CHAN for EVERY chid the fuzz entries can select (at
minimum chids 1 and 3), so a "wrong" decode still lands on an initialized
channel and the discriminator stays meaningful rather than scheduling an
uninitialized slot.
Full-knob land-review law. Metal owed: sitting #8.)

This proposal derives the exact structures required to bridge the remaining gap in the Kepler channel-bind sequence, following the successful repair of the `ORDER` validation in pull 8.

## 1. Runlist Entry Encoding on GK104-family
**Derivation & Audit:**
- `rnndb` (`gf100_pfifo.xml`, `gk104_copy.xml`, etc.) completely lacks a structural `<bitset>` or `<array>` definition for the VRAM-resident GK104 runlist entry layout (an exhaustive cleanroom search of `envytools` yields no match).
- **Structural Deduction:** We know from `PFIFO_CHAN` (`0x800000`, `stride="8"`, `length="0x1000"`) that GK104 supports 4096 channels, meaning `CHID` is exactly 12 bits. We also know the runlist entry stride is 8 bytes (2 dwords).
- **Current Write:** In `kepler.rs`, we simply wrote `chan_id` (1) to dword 0, and `0` to dword 1.
- **The Null/Skip Entry Hypothesis:** If the entry requires a `VALID` bit (e.g., bit 0, or bit 31), writing just `1` means either (a) we invoked `CHID=1` but `VALID=0`, or (b) we invoked `CHID=0, VALID=1` (if bit 0 is valid and bits 1..12 are `CHID`). Since we configured `PFIFO_CHAN[1]`, a lookup of `CHID=0` would hit an uninitialized, disabled channel, causing the scheduler to skip it silently!
- **Fix/Test:** We will instrument the runlist entry. We will explicitly test bit 0 and bit 31 as `VALID` bits, or write a dense block of entries trying different alignments, but a simpler structural approach is to look at standard NVIDIA layouts: typically, a valid bit is at bit 0 or bit 31, or the entry might require the `ENGINE` to be replicated. We will propose writing `(chan_id | 1)` or `(chan_id | (1 << 31))` if a valid bit is required. Actually, we will write `chan_id` and test for a valid bit, but wait—what if `PLAYLIST_WR_LEN` provides the engine?

## 2. Channel Table / RAMFC Validation
**Derivation & Audit:**
- The channel table at `0x800000` is `PFIFO_CHAN` (stride 8).
- `offset="0" (CHAN)`: Bits 0..29 are `INST` (`type="g80_channel"`), bit 31 is `UNK31`.
  - `g80_channel` dictates bits 0..27 are `ADDRESS` (`shr="12"`) and 28..29 are `TARGET` (0 = VRAM).
  - **Audit:** We wrote `0x80000000 | (inst_off >> 12)`. This correctly populates `ADDRESS` (inst_off/4096) and `TARGET` (0), and sets `UNK31=1`. `UNK31` is likely the channel `VALID` bit in the table.
- `offset="4" (STATE)`: Bit 0 `ENABLED`, Bit 10 `ENABLE_TRIGGER`, Bits 16..19 `ENGINE`.
  - **Audit:** We wrote `0x00000400` (`ENABLE_TRIGGER`). The hardware responded (`ch_stat=0x11000001`), confirming `ENABLED=1` and RO bits set. However, we left `ENGINE` (bits 16..19) as `0`. Since we want PGRAPH (Engine 0), this coincidentally perfectly matches!
- **RAMFC Validation (`PSUBFIFO` mapped to VRAM):**
  - We populated `CTRL_ADDR_LOW/HIGH`, `SIG`, `SEMAPHORE_CONFIG`, `IB_ADDRESS_LOW`, `IB_CONFIG` (now fixed to `ORDER=9`).
  - Fields we never touch but exist in `PSUBFIFO`: `IB_POS_UNK` (0x50), `IB_ENTRY_LOW/HIGH` (0x54/0x64), `DMA_PUT/GET` (0x5c, etc). These might need to be 0 or explicitly initialized, but our zeroed VRAM allocation likely handles this.

## 3. Submit/Enable Ordering
**Derivation:**
- Current sequence: Write `PFIFO_CHAN` -> Write `ENABLE_TRIGGER` -> Write Runlist VRAM -> Write `PLAYLIST_WR/WR_LEN`.
- **Symptom:** The playlist read advances (`playlist_rd=0x2013`), but nothing schedules.
- **Root Cause Hypothesis:** If the scheduler evaluates the runlist and finds an entry, but the channel's `ENABLED` bit isn't fully propagated, it might skip it. However, the hardware clearly acknowledges the runlist read. The more likely reason the scheduler ignores the channel is that the runlist entry *itself* is malformed (missing a valid bit) or the `PFIFO_CHAN` state is missing a critical engine mask bit.
- **Proposed Order:** 
  1. Write Runlist VRAM
  2. Write `PFIFO_CHAN`
  3. Write `ENABLE_TRIGGER`
  4. Write `PLAYLIST_WR` / `PLAYLIST_WR_LEN` (Submit Runlist)
  This ensures the channel is unconditionally fully armed before the scheduler is instructed to fetch the runlist.

## 4. Which Runlist?
**Derivation:**
- In GK104, `PLAYLIST_WR_LEN` (`0x2274`) has `ENG` at bits 20..23 (`type="gf100_pfifo_engine"`).
- We wrote `1` (which means `LEN=1`, `ENG=0`).
- Engine 0 is PGRAPH. This explicitly targets the PGRAPH runlist.
- Since we bound PBDMA 0 to Engine 0 (`SUBFIFO_ENG_MASK[0]`), this is structurally correct.

## 5. Instrumentation Delta
To definitively isolate whether the entry is skipped due to runlist format or channel table validation, we will add:
1. **RAMFC Dump:** A readback of the first 0x80 bytes of the `inst-raw` AFTER the runlist submission, to see if the scheduler wrote back any state (like `DMA_GET` advancing or `IB_POS_UNK` updating).
2. **Channel Table Dump:** A readback of `PFIFO_CHAN` (`0x800000` and `0x800004`) immediately before and after the runlist submission to verify `ENABLED` and `ENGINE` persist as expected.
3. **Runlist Entry Fuzzing:** We will write a multi-entry runlist:
   - Entry 0: `chan_id` (raw)
   - Entry 1: `chan_id | (1 << 31)` (Testing Bit 31 VALID)
   - Entry 2: `(chan_id << 1) | 1` (Testing Bit 0 VALID)
   By submitting `LEN=3`, if *any* format is correct, the scheduler will bind the channel, breaking the wall.
