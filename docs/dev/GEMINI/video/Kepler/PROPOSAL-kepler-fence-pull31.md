STATUS: PROPOSED

# PROPOSAL: kepler-fence pull 31 - The First Context-Bind Experiment

## Context
In s34, our safest-first chain read clean zeroes for all 5 context/status offsets (`0x804`, `0xb00`, `0xb04`, `0xc00`, `0xc08`), convicting `0x409504` (`WRCMD_CMD`) by elimination as the sole cause of the PRING poison on GK107. We also confirmed that the context surface is reachable and empty. We can now safely test the first hypothesis of our FECS context study: writing directly to the context registers to satisfy PFIFO's validation check.

## Owed Citation: Decoding PBUS_INTR=0x0C
As requested, the `PBUS_INTR` reading of `0x0C` latches two distinct bits. According to `envytools` documentation (`docs/hw/bus/pbus.rst`, Section "PBUS interrupts", under the `pbus-intr` register breakdown):
*   **Bit 2 (`0x04`)**: `MMIO_RING_ERR` — "MMIO access from host failed due to some error in PRING [GF100-]"
*   **Bit 3 (`0x08`)**: `MMIO_FAULT` — "MMIO access from host failed due to other reasons [NV41-]"
This fully explains the `0x0C` we observed: a direct consequence of the `BADF1000` ("target refused transaction") PRING fault from our earlier offset reads.

## Candidate Selection
We will pursue **Direction (a): First context-bind write experiment**. 
Since we proved the context surface is clean and `0x409504` is the isolated poison trigger, there is no need to deliberately wedge and un-wedge the unit on this boot. The most direct path to K-GPU-4 Milestone 4 is to attempt the bind.

## Implementation Plan (Pull 31)
1. **The Binding Value (Citation)**: `envytools` (`rnndb/graph/gf100_pgraph/ctxctl.xml`) defines channel identifiers as `type="g80_channel"`, which universally corresponds to the Instance Block physical address shifted right by 12 (`inst_off >> 12`).
2. **The Sequence**:
    *   **Pre-Read**: Read and print `CHAN_CUR` (`0xb00`), `CHAN_NEXT` (`0xb04`), and `ENGINE_STATUS` (`0xc00`).
    *   **The Write**: Write the channel identifier (`(inst_off as u32) >> 12`) to `CHAN_CUR` (`0x409B00`). We will also mirror it to `CHAN_NEXT` (`0x409B04`) to simulate a completed transition.
    *   **Post-Read**: Re-read and print `CHAN_CUR`, `CHAN_NEXT`, and `ENGINE_STATUS`. 
3. **The Expected Behavior**:
    *   If the context-bind takes, `ENGINE_STATUS` (`0xc00`) should assert its `CHAN_VALID` bit (bit 1, value `0x00000002`) in our post-read.
4. **The Witness**:
    *   After the binding sequence, the boot will naturally proceed to the `witness-rematch` sequence. We will observe the PFIFO strip signature. If PFIFO sees the bound context, it should move off `err=2, stat=5, valid=2000` for the first time.

## Compliance Gates
*   Block placed securely after `hb final`.
*   Control-bracket discipline maintained (`cpuctl` before and after).
*   No `0x409504` reads.
*   Wait for approval before writing the code.
*   Report "PUSH OWED: 18".
