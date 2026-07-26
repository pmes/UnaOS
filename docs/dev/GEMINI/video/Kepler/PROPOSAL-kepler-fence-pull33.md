STATUS: APPROVED (2026-07-25, coordinator GR4) WITH 3 BINDING AMENDMENTS.

# PROPOSAL: kepler-fence pull 33 - K-GPU-4 Milestone 5 (FECS Command Loop Skeleton)

## Amendments (Binding)
1. **The IO Ports are wrong**: The scheme is `host register X -> falcon (X & 0xffc) << 6`. `CC_SCRATCH[0]` (host `0x800`) is `I[0x20000]`. `CC_SCRATCH[1]` (host `0x804`) is `I[0x20100]`.
2. **A/B Fallback**: Ship image A with derived indexed ports, and image B with flat ports (`0x800`/`0x804`). Run A first, fall back to B if no ack, label the attempt.
3. **Drop Gating Premise**: CTXCTL subunit gating was refuted. The poison is per-offset, and `0x409504` is convicted. `CC_SCRATCH` is host-readable because it is just a working offset.

## Context
Ten sittings of elimination have proven that PFIFO channel validation is solely dependent on state built by the FECS context-switch microcode. We are now ready to build that gatekeeper. 

In our `docs/dev/GEMINI/video/Kepler/STUDY-fecs-ctx-init.md`, we identified the `CC_SCRATCH` / `WRCMD` surface as the host↔FECS communication channel. The poison trigger at `0x409504` (WRCMD_CMD) prevents host usage. However, `CC_SCRATCH` is host-readable/writable and unpoisoned. We will use the `CC_SCRATCH` registers as our command proxy. 

## Implementation Plan (Pull 33)

We will author the first minimal host↔FECS command loop echo test.

### 1. The Microcode (FECS Command Echo)
The microcode will loop polling `CC_SCRATCH[0]`. If it reads `0x1`, it writes `0x1` to `CC_SCRATCH[1]` and loops.

**Image A: Derived Ports (`0x20000`, `0x20100`) - 32 bytes (8 words)**
```assembly
// Address  | Bytes       | Instruction        | Citations (envytools)
// ---------|-------------|--------------------|---------------------------------------
// 0x0000   | f0 17 00    | mov $r1, 0x0       | docs/hw/falcon/arith.rst ("Loading immediates", Form: R2, I8)
// 0x0003   | f0 13 02    | sethi $r1, 0x20000 | docs/hw/falcon/arith.rst ("Loading immediates", Form: R2, I16) [sets high 16b to 0x0002]
// 0x0006   | f1 27 00 01 | mov $r2, 0x100     | docs/hw/falcon/arith.rst ("Loading immediates", Form: R2, I16)
// 0x000a   | f0 23 02    | sethi $r2, 0x20000 | docs/hw/falcon/arith.rst ("Loading immediates", Form: R2, I16) [sets high 16b to 0x0002]
// 0x000d   | f0 37 01    | mov $r3, 0x1       | docs/hw/falcon/arith.rst ("Loading immediates", Form: R2, I8)
// 0x0010   | cf 14 00    | iord $r4, I[$r1]   | docs/hw/falcon/io.rst ("IO space reads: iord")
// 0x0013   | b0 44 01    | cmpu b32 $r4, 0x1  | docs/hw/falcon/arith.rst ("Comparison")
// 0x0016   | f4 1b fa    | bra ne, -6         | docs/hw/falcon/isa.rst ("Branches" - loops to 0x10)
// 0x0019   | d0 23 00    | iowr I[$r2], $r3   | docs/hw/falcon/io.rst ("IO space writes: iowr")
// 0x001c   | f4 0e f4    | bra -12            | docs/hw/falcon/isa.rst ("Branches" - loops to 0x10)
// 0x001f   | 00          | (padding)          | Align to 4-byte boundary
```

**Image B: Flat Ports (`0x800`, `0x804`) - 28 bytes (7 words)**
```assembly
// Address  | Bytes       | Instruction        | Citations (envytools)
// ---------|-------------|--------------------|---------------------------------------
// 0x0000   | f1 17 00 08 | mov $r1, 0x800     | docs/hw/falcon/arith.rst ("Loading immediates", Form: R2, I16)
// 0x0004   | f1 27 04 08 | mov $r2, 0x804     | docs/hw/falcon/arith.rst ("Loading immediates", Form: R2, I16)
// 0x0008   | f0 37 01    | mov $r3, 0x1       | docs/hw/falcon/arith.rst ("Loading immediates", Form: R2, I8)
// 0x000b   | cf 14 00    | iord $r4, I[$r1]   | docs/hw/falcon/io.rst ("IO space reads: iord")
// 0x000e   | b0 44 01    | cmpu b32 $r4, 0x1  | docs/hw/falcon/arith.rst ("Comparison")
// 0x0011   | f4 1b fa    | bra ne, -6         | docs/hw/falcon/isa.rst ("Branches" - loops to 0x0b)
// 0x0014   | d0 23 00    | iowr I[$r2], $r3   | docs/hw/falcon/io.rst ("IO space writes: iowr")
// 0x0017   | f4 0e f4    | bra -12            | docs/hw/falcon/isa.rst ("Branches" - loops to 0x0b)
// 0x001a   | 00 00       | (padding)          | Align to 4-byte boundary
```

### 2. The Host Sequence
We implement an A/B fallback sequence, uploading and executing Image A first. If no acknowledgment is received, we halt, upload Image B, and execute it. 

*   **Execute Image A Witness**: The baseline Pull 25/27 witness program (heartbeat `0xF00DFACE`) remains running first to verify engine liveness as established.
*   **A/B Fallback Loop**:
    *   Upload Image A (Derived Ports). Start Falcon.
    *   Write `0x1` to `CC_SCRATCH[0]`.
    *   Poll `CC_SCRATCH[1]` for `0x1`. If acked: mark success and break.
    *   If no ack: print failure, Halt Falcon, upload Image B (Flat Ports). Start Falcon.
    *   Write `0x1` to `CC_SCRATCH[0]`.
    *   Poll `CC_SCRATCH[1]` for `0x1`. Print outcome.

## Compliance Gates
*   Cleanroom notice strictly upheld: Encoded from ISA docs.
*   Only FECS targeted for execution.
*   A/B Fallback methodology correctly handles the unknown port mapping.
*   All docs+code committed, scratch deleted.
*   No push.
