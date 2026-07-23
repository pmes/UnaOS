# PROPOSAL — Kepler wall-2 pull 8: runlist-entry and channel-bind derivation

STATUS: PROPOSED

## a. GK107 runlist entry format vs our channel id

The runlist entry for GK104-family GPUs is an 8-byte (64-bit) structure in memory. Our current implementation writes the channel ID `chan_id = 0` to the first 4 bytes, and `0` to the next 4 bytes:
```rust
core::ptr::write_volatile((bar1 + runlist_off) as *mut u32, chan_id);
core::ptr::write_volatile((bar1 + runlist_off + 4) as *mut u32, 0);
```
Per `rnndb/fifo/gf100_pfifo.xml` and related structures, the `CHID` is generally 12 bits on GK104 (`CHID` variants `GK104-`). No other bits are strictly required to be non-zero for a basic valid entry (there is no "valid" bit in the entry itself, its presence in the array defines its existence). Thus, `0x00000000_00000000` correctly specifies Channel 0. Our entry format is well-formed.

## b. RAMFC / instance-block fields the scheduler validates

The `inst-raw` values from sitting #6 map perfectly to the `PSUBFIFO` domain (which serves as the RAMFC instance block layout) in `envytools/rnndb/fifo/gf100_pfifo.xml`:

- `08=0x02002000`: Maps to `CTRL_ADDR_LOW`. Bits 0-1 are `TARGET` (0 = VRAM), bits 12-31 are `ADDR_LOW`. Matches our USERD offset `0x2002000`.
- `0C=0`: Maps to `CTRL_ADDR_HIGH`. Matches our USERD high bits.
- `48=0x02001000`: Maps to `IB_ADDRESS_LOW`. Matches our GPFIFO offset `0x2001000`.
- `4C=0x01FF0000`: Maps to `IB_CONFIG`. Bits 0-7 are `ADDRESS_HIGH` (0), bits 16-31 are `ORDER` (511). Matches our GPFIFO length config.

The scheduler validates these fields. A potential issue is `TARGET` mapping. In `CTRL_ADDR_LOW` (0x08), we wrote `0x02002000`. Bits 0-1 are `TARGET`. On GF100/GK104, a `TARGET` of `0` in this context may not map to VRAM depending on the GMMU setup, or it may require specific flags. However, assuming standard `g80_mem_target` mappings, 0 = VRAM.
The missing element is likely related to **engine binding/RAMFC alignment** or missing flags in the upper fields (which we populated from Kepler-specific offsets like 0x84, 0x94, etc., not fully detailed in the GF100 `PSUBFIFO` xml but valid for GK104).

## c. Runlist submit/commit ordering vs channel-enable

Our current sequence:
1. `mmio_write` to `0x800000` (PFIFO_CHAN_PTR)
2. `mmio_write` to `0x800004` (PFIFO_CHAN_ENABLE) -> **Channel Enabled**
3. Memory writes to construct the runlist.
4. `mmio_write` to `0x2270` (PLAYLIST_WR)
5. `mmio_write` to `0x2274` (PLAYLIST_WR_LEN) -> **Runlist Submitted**

**Correction required:** We must submit the runlist (and wait for it to be accepted/committed) *before* enabling the channel, or at least ensure the channel is disabled, the runlist is committed, and *then* the channel is enabled. If a channel is enabled but not in an active runlist, the scheduler may immediately fault it or hang the binding process.
We will flip the sequence: Write Runlist -> Commit (PLAYLIST_WR/LEN) -> Enable Channel.

## d. Which runlist?

On GK104, `PLAYLIST_WR_LEN` (0x2274) contains an `ENG` field at bits 20-23 (`variants="GK104-" type="gf100_pfifo_engine"`).
We are writing `1` to `PLAYLIST_WR_LEN`, which implies `LEN=1` and `ENG=0` (PGRAPH).
If the channel does not use PGRAPH (engine 0) or the PGRAPH engine is powered down or not fully initialized by our bare-bones driver, the scheduler will fail to bind the channel to the PGRAPH PBDMA. 
We should submit to the `NONE` engine (`ENG=0x1F`) or `PCOPY0` (`ENG=4`) if we only want PFIFO methods, or we must ensure we use an active engine's runlist. We will submit to engine `0` (PGRAPH) but ensure `PGRAPH` is powered/masked, or alternatively submit to the Copy Engine `PCOPY0` (4) or `NONE` (0x1F) to test if the scheduler binds it.
*Implementation Plan:* We will submit to `ENG=0x1F` (NONE) and `ENG=0` (PGRAPH) sequentially if needed, but first, we will fix the order. Let's write `(0x1F << 20) | 1` to `PLAYLIST_WR_LEN` to target the `NONE` / generic PFIFO runlist to bypass PGRAPH dependencies, or just `(0 << 20) | 1` but after fixing the submit order.

## Implementation Details (Pull 8)

1. **Reorder Initialization:**
   Construct Runlist -> Commit Runlist to `PLAYLIST_WR` -> Enable Channel `PFIFO_CHAN_ENABLE`.
2. **Dedupe Double Probe:**
   We will add a static `static ATTEMPTED: AtomicBool = AtomicBool::new(false);` in `kepler::init` to immediately return if already initialized, preventing the PCI walk from double-initializing the GPU.

## Verification
- We will arm the gate `UNAOS_IVB+UNAOS_KEPLER+UNAOS_KEPLER_TAKEOVER+UNAOS_KEPLER_FIFO`.
- A successful run will finally show `ib_get` advance from `ib_put`, meaning the PBDMA has bound the channel and processed the Host Semaphore Release.
