STATUS: PROPOSED

# PROPOSAL: kepler-fence pull 28 - FECS Context/Init Study & Recon

## Context
In sitting #30, our heartbeat microcode proved that the fence wall (PFIFO stripped channel with `err=2, stat=5, valid=00002000`) remains even when the PGRAPH engine is actively running code. The wall is definitively **not** engine liveness. The next lead is `DMACTL` bit 0: `REQUIRE_CTX`. This pull conducts a cleanroom study of the real FECS context switch init process to determine the minimal register changes needed to satisfy PFIFO's "valid context" check, and introduces a read-only probe to gather ground-truth reset values.

## Implementation Plan
1. **STUDY Deliverable**: Created `docs/dev/GEMINI/video/Kepler/STUDY-fecs-ctx-init.md` answering the three prompt questions with citations from `envytools` (specifically `docs/hw/graph/fermi/ctxctl/intro.rst` and `rnndb/graph/gf100_pgraph/ctxctl.xml`).
    * Identifies phases (Self-init, PGRAPH strand init, Host handshake, Load/Save loop).
    * Outlines the handshake surface (`CC_SCRATCH` mailboxes, `ENGINE_STATUS`/`ENGINE_TRIGGER` synchronization).
    * Defines what "a context exists" means (the `CHAN_CUR` register at `0xb00` holds a channel, and `ENGINE_STATUS` `CHAN_VALID` bit is high).
    * Forms a set of minimal hypotheses to test in future pulls.

2. **PROBE Deliverable**: Inserted a read-only reconnaissance block in `kepler.rs` at the start of the `FECS` ucode sequence (`base = 0x409000`). It dumps the following registers one per line using the `serial_println!` format `:: kepler: recon [reg_name]=[value] ::`:
    * `0x409504` `WRCMD_CMD`
    * `0x409800` `CC_SCRATCH[0]`
    * `0x409804` `CC_SCRATCH[1]`
    * `0x409B00` `CHAN_CUR`
    * `0x409B04` `CHAN_NEXT`
    * `0x409C00` `ENGINE_STATUS`
    * `0x409C08` `ENGINE_TRIGGER`

## Compliance Gates
* No new execution logic introduced (`Image A` and `UCODE_HB` preserved as landed).
* `FTDI` ring budget respected (only 7 lines printed, no dense sweeps).
* Zero code copied verbatim for the study.
* Scratch deleted, all docs+code committed.
* No push. Report "PUSH OWED: 14".
