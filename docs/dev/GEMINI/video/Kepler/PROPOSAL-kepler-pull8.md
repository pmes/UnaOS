# PROPOSAL — Kepler Pull 8: Runlist-Entry & Channel Bind Derivation

This proposal addresses the wall upstream of the PBDMA from sitting #6, where the channel is never scheduled onto a PBDMA. Using `envytools/rnndb/fifo/gf100_pfifo.xml`, we decode the failed `inst-raw` and the runlist mechanics to break the wall.

## a. GK107 Runlist Entry Format
**Diagnosis**: In `kepler.rs`, we populate the runlist entry as just `chan_id` (0).
- In `gf100_pfifo.xml`, there is no explicit VRAM struct for the playlist entry. However, the channel ID on GK104 is definitively 12 bits (`CHID` field in `PSUBFIFO.CH` at `0x120`).
- While writing `0` is structurally an integer channel ID, channel `0` might be treated as a null/invalid entry by the hardware scheduler.
- **Fix**: We will allocate `chan_id = 1` instead of `0` to ensure the entry isn't ignored as an empty slot, and we will write it as a 32-bit word (`chan_id`).

## b. RAMFC/Instance-Block Fields Validation
**Diagnosis**: The scheduler DMAs the RAMFC (Instance Block) in VRAM directly into the `PSUBFIFO` MMIO registers (base `0x40000`). Our `inst-raw` writes matched `PSUBFIFO` offsets, but with a fatal formatting error in `IB_CONFIG`.

**Decode against rnndb (`gf100_pfifo.xml`, `array name="PSUBFIFO"`):**
- `0x08`: `CTRL_ADDR_LOW` (Target/Addr for `USERD`). We wrote `userd_off & 0xFFFFFFFF`. Since `ADDR_LOW` is `shr="12"` and `TARGET` is bits 0-1 (0 = VRAM), writing the raw page-aligned address coincidentally sets the right bits.
- `0x0C`: `CTRL_ADDR_HIGH`. We wrote `userd_off >> 32` (0). Correct.
- `0x48`: `IB_ADDRESS_LOW` (GPFIFO base). We wrote `gpfifo_off & 0xFFFFFFFF`. Correct.
- `0x4C`: `IB_CONFIG`. **FATAL ERROR.**
  - `gf100_pfifo.xml` defines `IB_CONFIG` as: bits 0..7 `ADDRESS_HIGH`, bits 16..31 `ORDER`.
  - We wrote `(gpfifo_off >> 32) | (511 << 16)`.
  - We treated bits 16..31 as a length/limit (511 for a 512-entry ring). BUT `ORDER` is a logarithm (`log2(entries)`).
  - Writing `511` to `ORDER` means $2^{511}$ entries! The scheduler sees an invalid GPFIFO size and refuses to bind the channel.
  - **Fix**: `ORDER` for 512 entries must be `9` (`log2(512)`). Write `(gpfifo_off >> 32) | (9 << 16)`.

## c. Runlist Submit/Commit Ordering vs Channel-Enable
**Diagnosis**: In `kepler.rs`, we:
1. Bound channel to engine `0x80000000 | inst_off >> 12`
2. Enabled channel (`0x800004` = `0x00000400`)
3. Submitted Runlist (`PLAYLIST_WR` and `PLAYLIST_WR_LEN`)
- According to standard scheduling flow, the channel must be enabled *before* or *during* the runlist update so the scheduler sees it as runnable when evaluating the new runlist. The order used in `kepler.rs` is generally correct, but we must ensure we wait for `PLAYLIST_RD` to acknowledge the new runlist.

## d. Which Runlist?
**Diagnosis**: On GK104, there is a per-engine runlist.
- In `kepler.rs`, we wrote `1` to `PLAYLIST_WR_LEN` (`0x2274`).
- Per `gf100_pfifo.xml`, `PLAYLIST_WR_LEN` bits 20..23 are the `ENG` field (`type="gf100_pfifo_engine"`). Writing `1` sets `LEN=1` and `ENG=0`.
- Engine `0` is PGRAPH. This is correct because we bound PBDMA 0 to Engine 0 (PGRAPH) in `SUBFIFO_ENG_MASK`.
- **Fix**: The runlist targeting is correct (`ENG=0`). We will keep using this runlist.

## e. Deduplication
- We will deduplicate the `igpu.rs` probe run by adding a guard against re-entry (using an `AtomicBool` or static flag) to ensure the PCI walk doesn't double-probe the Ivy Bridge iGPU.

## Cleanroom Debt
We will remove the `GF119` EVO comment at line `~465` of `kepler.rs` and replace it with an honest empirically probed note or an rnndb citation if applicable.
