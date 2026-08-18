STATUS: LANDED (retroactive proposal — the implementation shipped first and was
landed 0e26447e against the RELAY invitation; this doc backfills the record and
delivers the owed PBUS_INTR decode. GR5 accepts the backfill; proposal-first
resumes from pull 31.)

# PROPOSAL: kepler-fence pull 30 - Safest-First Chain & Real Un-Wedge

## Context
In s33, the subunit-gating theory was cleanly refuted: `PIBUS_MMIO_HUB_ENABLE1` showed `CTXCTL` already enabled, and `CC_SCRATCH[0]` read a valid `00000000` when read first. This means the poison is **per-offset**, with `0x409504` (`WRCMD_CMD`) remaining the prime suspect. `PBUS_INTR` returned `0x0C` latched prior to our W1C.

## Decoding PBUS_INTR=0x0C
Per `envytools` documentation (`docs/hw/bus/pbus.rst` Section "PBUS interrupts"):
*   **Bit 2 (0x04)**: `MMIO_RING_ERR` — "MMIO access from host failed due to some error in PRING [GF100-]"
*   **Bit 3 (0x08)**: `MMIO_FAULT` — "MMIO access from host failed due to other reasons [NV41-]"

`0x0C` confirms the presence of PRING/MMIO faults (likely triggered by the BIOS or earlier OS state before our boot block runs) and validates that the host sees `badf1000` PRING refusals directly in this register.

## Implementation Plan (Pull 30)
We will probe the remaining 5 unknown offsets in a single boot, reading them in a "safest-first" chain to maximize clean data recovery before hitting a potential fault, followed immediately by the PRING un-wedge test if a fault occurs.

1.  **Safest-First Chain Order:**
    *   `0x804` (`CC_SCRATCH[1]`): Safest. Sibling to `CC_SCRATCH[0]` which proved clean in s33.
    *   `0xb00` (`CHAN_CUR`): Passive state register tracking current PFIFO context.
    *   `0xb04` (`CHAN_NEXT`): Passive state register for upcoming PFIFO context.
    *   `0xc00` (`ENGINE_STATUS`): Complex state register reporting unit readiness.
    *   `0xc08` (`ENGINE_TRIGGER`): Most dangerous. An active handshake/command register which could easily fault if the unit is in the wrong state.
    *   *Note: `0x504` (`WRCMD_CMD`) is deliberately excluded.*

2.  **The Chain Experiment:**
    *   Bracket the entire block with `cpuctl` reads (`recon-pre` / `recon-post`).
    *   Read each offset in sequence. If an offset reads a `BADF` family value, log it as `FAULT`.
    *   Any offset queued *after* the fault is skipped with a `SKIP (tainted)` marker.

3.  **The Real Un-Wedge:**
    *   Immediately upon detecting the first `BADF`, execute the observe-and-clear sequence: read `PIBUS INTR_ADDR` (`0x120120`), `PIBUS INTR_VALUE` (`0x120124`), `PIBUS INTR` (`0x120128`), and `PBUS_INTR` (`0x1100`).
    *   Print their latched values.
    *   Write back (W1C) exactly what was observed for each register (if non-zero).
    *   Perform a mid-block `cpuctl` read (`un-wedge-test`) to see if the unit recovers its real value.

## Compliance Gates
*   No execution logic introduced.
*   Zero writes to FECS offsets.
*   W1C restricted strictly to observed latched bits.
*   Block placed after `hb final`.
*   All scratch deleted, docs+code committed. No push. Report "PUSH OWED: 17".
