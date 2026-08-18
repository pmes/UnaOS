STATUS: APPROVED (2026-07-25, coordinator GR4) WITH ONE BINDING AMENDMENT.

I re-derived your byte stream and it packs to exactly the eight constants
you list (word 6 = f7 f8 02 + pad = 0002f8f7; the "30 bytes" in the prose
is 27 — cosmetic). `mov $r3,0` + `sethi $r3,0x50` → 0x500000 iterations is
right given sethi replaces the high half, and the branch target
0x0016 − 9 = 0x000d lands on the `add`, closing the loop correctly. Port
derivation for MAILBOX1 is right: (0x044 & 0xffc) << 6 = 0x1100.

**AMENDMENT (binding) — prove the bound actually bounded.** The whole
safety argument for this pull is that the loop terminates. Add a third
observation AFTER the witness block and a short settle:
`:: kepler: hb final mb1=XXXXXXXX cpuctl=XXXXXXXX ::`
cpuctl showing STOPPED (bit 4) with mb1 frozen = the bound held and the
engine parked cleanly. Still advancing = the loop is longer than the boot
window (report it; not a failure, but we must not claim a clean bound we
didn't observe). This costs two reads and closes the one claim the pull
makes about itself.

Everything else as proposed: image A first as the known-good witness, HB
started without polling, witness sequence byte-for-byte unchanged.

# PROPOSAL: kepler-fence pull 27 - Live Engine Witness

## Context
In sitting #29, our first microcode ran successfully, proving the execution capabilities of the Falcon on GK107 (FECS) and confirming the INDEXED IO scheme is correct (`port = (offset & 0xffc) << 6`).
The fence wall (PFIFO stripping VALID/POLL, err=2) has been tested under all conditions EXCEPT against an engine that is actively running code. This pull will start a bounded heartbeat microcode (`UCODE_HB`) that continuously increments `MAILBOX1` (at `base+0x044`) for a finite duration. We will read `MAILBOX1` before and after the existing PFIFO witness sequence to prove the engine was alive during the witness.

## IO Port Derivation
`MAILBOX1` is located at `base + 0x044`.
Using the proven INDEXED IO scheme:
`port = (0x044 & 0xffc) << 6`
`port = 0x044 << 6`
`port = 0x1100`

## Assembly Listing & Citations
We author a bounded heartbeat image (`UCODE_HB`) using `envytools` Falcon ISA v4 (`fuc4`). The loop counter is initialized to `0x500000` (5.2 million iterations, yielding sufficient runtime for the host witness without risking an unbounded wedge).

**Program bytes (30 bytes, padded to 8 `u32` words):**
```assembly
// Address  | Bytes       | Instruction           | Citations (envytools)
// ---------|-------------|-----------------------|---------------------------------------
// 0x0000   | f1 17 00 11 | mov $r1, 0x1100       | docs/hw/falcon/arith.rst (Loading immediates: mov, Form: R2, I16)
// 0x0004   | f0 37 00    | mov $r3, 0            | docs/hw/falcon/arith.rst (Loading immediates: mov, Form: R2, I8)
// 0x0007   | f0 33 50    | sethi $r3, 0x50       | docs/hw/falcon/arith.rst (Loading immediates: sethi, Form: R2, I8 - sets bits 16-23)
// 0x000a   | f0 27 00    | mov $r2, 0            | docs/hw/falcon/arith.rst (Loading immediates: mov, Form: R2, I8)
// 0x000d   | b6 20 01    | add b32 $r2, 1        | docs/hw/falcon/arith.rst (Addition: add, Form: R2, R1, I8)
// 0x0010   | d0 12 00    | iowr I[$r1], $r2      | docs/hw/falcon/io.rst (IO space writes: iowr, Form: R2, I8, R1)
// 0x0013   | b6 32 01    | sub b32 $r3, 1        | docs/hw/falcon/arith.rst (Subtraction: sub, Form: R2, R1, I8)
// 0x0016   | f4 1b f7    | bra nz, -9            | docs/hw/falcon/branch.rst (Conditional branch: bra nz, target = start of bra - 9 = 0x000d)
// 0x0019   | f8 02       | exit                  | docs/hw/falcon/proc.rst (Halting microcode execution: exit)
```
*(Compiled and verified with `envyas` / `envydis` using `-m falcon -V fuc4`.)*

Encoded in `kepler.rs` as:
```rust
const UCODE_HB: [u32; 8] = [
    0x110017f1,
    0xf00037f0,
    0x27f05033,
    0x0120b600,
    0xb60012d0,
    0x1bf40132,
    0x0002f8f7,
    0x00000000,
];
```

## Implementation Plan
Lane: `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.

1. **UCODE_HB Execution**:
   - Insert `UCODE_HB` execution logic prior to the PFIFO witness sequence, targeting FECS (`base=0x409000`).
   - Use the proven `image A` upload protocol:
     - Pre-seed `MAILBOX1` (`base + 0x044`) to `0xA5A50000` (or `0`).
     - Write `IMEMC` and `IMEMT`. Upload `UCODE_HB` with `IMEM_PAGE_WORDS` padding.
     - Perform TLB read attestation (`base + 0x140`, `0x144`).
     - Verify upload via `IMEMC` and `IMEMD` readback. If verification fails, `ABORT` without starting.
     - Clear `DMACTL` `REQUIRE_CTX` bit (read `base + 0x10C`, clear bit 0, verify it cleared).
     - Write `BOOTVEC=0`, `CPUCTL=2`.
   - **Crucially**: Print `:: kepler: hb start mb1=XXXXXXXX ::` and DO NOT POLL for completion. Let the heartbeat run in the background.
   - Wait: `Image A` (which we keep as a known-good execution witness) still runs *before* this as a baseline, ensuring the engine behaves predictably. Wait, the brief says: "keep image A as landed and still run it first — it is now our known-good execution witness", then "Start UCODE_HB...". So `UCODE_HB` will be uploaded and started *after* Image A finishes its clean halt.

2. **The PFIFO Witness Block**:
   - Print `:: kepler: hb pre-witness mb1=XXXXXXXX cpuctl=XXXXXXXX ::`.
   - Run the exact, byte-for-byte unchanged PFIFO witness sequence (the `PFIFO_CHAN[1]` register writes and `VALID/POLL` reads).
   - Print `:: kepler: hb post-witness mb1=XXXXXXXX cpuctl=XXXXXXXX ::`.
   - This proves the `MAILBOX1` value advanced and the engine was actively executing instructions while the wall was hit.

## Compliance Gates
* Finite iteration count in `UCODE_HB` (`0x500000` loops) ensures the engine doesn't spin forever, mitigating wedge risks.
* No polling loop after starting `UCODE_HB`—the host immediately proceeds to the witness.
* The historical witness sequence is left strictly untouched.
* Cleanroom notice strictly upheld (all assembly derived from `envytools` ISA specs).
* All docs+code committed, scratch deleted.
* No push. Report "PUSH OWED: 13".
