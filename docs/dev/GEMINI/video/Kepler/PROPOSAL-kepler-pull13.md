# PROPOSAL — Kepler Pull 13: Instance Bytes Visibility (Flush)

STATUS: PROPOSED

This pull tests the hypothesis that the `NO_POLL` rejection occurs because the PFIFO engine reads a stale/cached view of the instance block during channel validation, failing to see our `USERD` pointer and flags written via BAR1.

## 1. VRAM write → Engine visibility on GF100/GK104
Cleanroom sources identify specific BAR/VRAM flush candidates:

* **BAR Flush Doorbell (`PFLUSH`)**: 
  * `envytools/docs/hw/memory/gf100-host-mem.rst` explicitly documents `gf100-pflush 0x1000 used to flush BAR writes`.
  * `envytools/docs/hw/mmio.rst` places `PFLUSH` at `0x070000`.
  * `envytools/rnndb/fifo/g80_pfifo.xml` maps `PFIFO_FLUSH` at `0x70000` (valid for `G84-`), defining a `FLUSH_CTRL` register where bit 0 is `TRIGGER`.
* **PFB Flush Register (TLB Flush)**: 
  * `envytools/docs/hw/memory/g80-vm.rst` and `gf100_pffb.xml` document a `TLB_FLUSH` mechanism (`0x100c80`). However, this explicitly targets page tables, not raw BAR1 payload caches.
* **PRI Read-Serialization**: 
  * If the flush register alone is insufficient, reading back the flush register over the PRI (PCIe register interface) forces the CPU to serialize the MMIO write stream.

**Selection**: The `0x70000` (`PFLUSH` / `PFIFO_FLUSH`) register is the canonical "flush BAR writes" mechanism in `envytools`. We will use it.

## 2. Witness Experiment
We will introduce a single variable: a BAR1 flush inserted between the instance block writes and the `PFIFO_CHAN[1]` validation ladder. 
1. Write our instance block (`USERD_HI` bit 31 remains as the probe).
2. **Execute Flush**: Write `1` (`TRIGGER`) to `0x70000`, then read it back to force PRI serialization.
3. Print `:: kepler: flush-executed 0x70000 ::`.
4. Run the exact Sitting #10 validation ladder unchanged.
5. Print standard markers: `WITNESS PASSED - bits stuck!` or `sched-status post-restore`.

## 3. Fallback Framing
If the flush changes nothing (bits still strip and `NO_POLL` persists), the cache-visibility hypothesis is definitively refuted. 
The poll area must genuinely be configured elsewhere. If this fails, Pull 14 will target:
1. **PBDMA Engine State**: Auditing `PBDMA` engine `CTRL_ADDR` target bits (the `TARGET` enum values are marked `XXX-unconfirmed` in `rnndb` and may dictate USERD enablement).
2. **Display-era State Tables**: Investigating if the Core-channel/USERD config requires enablement via `PDISPLAY` or `NV_EVO_CORE` structures.
