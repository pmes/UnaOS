STATUS: PROPOSED

# PROPOSAL: kepler-fence pull 25 - K-GPU-4 Milestone 2 (First Ucode)

## Context
The previous pull verified that the Falcon memory ports at the correct bases (`0x409000` for FECS and `0x41A000` for GPCCS) are live and the upload path is intact, with all 16 sentinels verified via readback. Milestone 2 executes the first UnaOS-authored code on the GPU (FECS only) to prove the execution path. We will write `0xF00DFACE` to `FALCON_MAILBOX0` and exit cleanly.

## Assembly Listing & Citations
We author the microcode from scratch using the `envytools` Falcon ISA v4 (`fuc4`) specifications.
`FALCON_MAILBOX0` is mapped at offset `0x040` in the host MMIO window, which corresponds to IO port `0x10` in the Falcon's internal IO space (`0x040 / 4 = 0x10`). 

**Program bytes (16 bytes = 4 words):**
```assembly
// Address  | Bytes       | Instruction        | Citations (envytools)
// ---------|-------------|--------------------|---------------------------------------
// 0x0000   | f0 17 10    | mov $r1, 0x10      | docs/hw/falcon/arith.rst ("Loading immediates: mov, sethi", Form: R2, I8)
// 0x0003   | f1 27 ce fa | mov $r2, -0x532    | docs/hw/falcon/arith.rst ("Loading immediates: mov, sethi", Form: R2, I16 sign-extended, -0x532 = 0xface)
// 0x0007   | f1 23 0d f0 | sethi $r2, 0xf00d  | docs/hw/falcon/arith.rst ("Loading immediates: mov, sethi", Form: R2, I16 zero-extended)
// 0x000b   | d0 12 00    | iowr I[$r1], $r2   | docs/hw/falcon/io.rst ("IO space writes: iowr", Form: R2, I8, R1)
// 0x000e   | f8 02       | exit               | docs/hw/falcon/proc.rst ("Halting microcode execution: exit")
```
*(Compiled with `envyas` using `-m falcon -V fuc4`.)*

These 16 bytes will be encoded into `u32` words in `kepler.rs` as:
```rust
const UCODE_M2: [u32; 4] = [
    0xf11017f0, 
    0xf1face27, 
    0xd0f00d23, 
    0x02f80012, 
];
```

## Implementation Plan
Lane: `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.

1. **Host Target**: 
   - FECS ONLY (`base = 0x409000`). GPCCS is skipped for this execution test.

2. **Upload Protocol**:
   - Pre-state: Print `:: kepler: ucode pre mailbox0=XXXXXXXX cpuctl=XXXXXXXX ::` (reading `base+0x040` and `base+0x100`).
   - Write `base+0x180` (IMEMC) = `0 | (1 << 24)` (offset 0, AINCW).
   - Write `base+0x188` (IMEMT) = `0` (Tag discipline: tag 0 per 256B block).
   - Write the 4 words of `UCODE_M2` to `base+0x184` (IMEMD).
   - Print `:: kepler: ucode uploaded words=4 ::`.

3. **Readback Verify**:
   - Write `base+0x180` (IMEMC) = `0 | (1 << 25)` (offset 0, AINCR).
   - Read 4 words from `base+0x184` (IMEMD) and compare against `UCODE_M2`.
   - Print `:: kepler: ucode verify ok=Y/N w0=XXXXXXXX ::` (showing the first word).
   - If verify fails: STOP, emit honest null `:: kepler: ucode end cpuctl=... mailbox0=... ::` and break.

4. **Execution**:
   - Write `base+0x104` (BOOTVEC) = `0`.
   - Write `base+0x100` (CPUCTL) = `2` (`STARTCPU`).
   - Print `:: kepler: ucode start cpuctl-wr=00000002 ::`.
   - **Bounded poll**: Loop for roughly ~100 ms checking `base+0x100` (CPUCTL). Wait until `(cpuctl & 0x10) != 0` (HALT bit set).
   - Read `FALCON_MAILBOX0` (`base+0x040`).
   - Print post-state: `:: kepler: ucode end cpuctl=XXXXXXXX mailbox0=XXXXXXXX ::`.

5. **Legacy State**:
   - All old probes, pulses, and dumps stay unchanged/gated as landed.

## Compliance Gates
* Cleanroom notice strictly upheld: Encoded from ISA docs.
* Only FECS targeted for execution.
* Readback verification blocks execution if failed.
* No retries/restore on failure.
* All docs+code committed, scratch deleted.
* No push. Report "PUSH OWED: 11".
