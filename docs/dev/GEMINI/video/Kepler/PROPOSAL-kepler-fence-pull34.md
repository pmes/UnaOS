# PROPOSAL: Kepler Fence Pull 34 - Milestone 6 (Ctx State Assertion)

## 1. Goal
Fulfill the priorities set out for pull 34 (Milestone 6):
- **Bound the echo:** The ucode will loop waiting for commands from `CC_SCRATCH[0]`, but the loop is strictly bounded by a decrementing counter (`$r5`) initialized to `0x10` (Image A) or incrementing from `0xFFF0` (Image B). If the host wedges and the bound expires, the ucode writes a phase stamp of `0xBD` (or `0xBE`) to the ACK register and cleanly exits (`exit` instruction). When functioning normally, the host sends command `1` to trigger a register read, and then sends command `2` (the exit sentinel) to cleanly bound and terminate the ucode before expiry. The host will verify `CPUCTL` reaches the STOPPED state. This must-fix has been owed since pull 33.
- **Recon probe before any write:** Before touching any engine state, the host will read and print the raw FECS handshake surface: `CHAN_CUR` (0xb00), `CHAN_NEXT` (0xb04), `ENGINE_STATUS` (0xc00), `ENGINE_TRIGGER` (0xc08), and `WRCMD_DATA/CMD` (0x500/0x504) via the `0x409000` base.
- **H2/H3 with readback:** Following the read-only recon probe, the host will write `2` (CHAN_VALID) to `ENGINE_STATUS` and read it back. If it sticks, it will write `1` to `ENGINE_TRIGGER` and read it back. This tests if `ENGINE_STATUS` is host-writable or falcon-owned.
- **Falcon-side read:** The ucode will read the `0x409504` register from inside the Falcon. We derive the port by `(0x504 & 0xffc) << 6 = 0x14100`. The ucode will use `iord` to read this port, then write the result to `CC_SCRATCH[1]` (the ACK register) for the host to observe. We will ship an A/B pair for this.

## 2. Microcode Changes & Branch Mathematics
The microcode is expanded to 128 bytes (32 words) to accommodate the new logic. The executing code stays within the first 111 bytes. Because the padding shifted the relative offsets, the branches are recalculated.

**Branch Offset Base:** In the Falcon ISA, the PC-relative branch offset is calculated from the start address of the branch instruction itself. The equation used is `offset = target - branch_address`.

`poll` block starts at `0x2C` (44).
1. `bra eq, cmd2_exit` is located at `0x3B` (59).
   Target `cmd2_exit` is at `0x67` (103).
   Offset = `103 - 59 = 44 = 0x2C`.
   Instruction bytes: `0xf4, 0x2b, 0x2c`.

2. `bra ne, dec` is located at `0x41` (65).
   Target `dec` is at `0x56` (86).
   Offset = `86 - 65 = 21 = 0x15`.
   Instruction bytes: `0xf4, 0x1b, 0x15`.

3. `bra ne, poll` (after checking `r5` != 0 from `cmd1`) is at `0x53` (83).
   Target `poll` is at `0x2C` (44).
   Offset = `44 - 83 = -39`.
   Two's complement = `256 - 39 = 217 = 0xD9`.
   Instruction bytes: `0xf4, 0x1b, 0xd9`.

4. `bra ne, poll` (after decrementing `r5`) is at `0x5C` (92).
   Target `poll` is at `0x2C` (44).
   Offset = `44 - 92 = -48`.
   Two's complement = `256 - 48 = 208 = 0xD0`.
   Instruction bytes: `0xf4, 0x1b, 0xd0`.

This explicitly proves where every relative branch lands after the 128-byte padding.

## 3. Host Driver Changes (`kepler.rs`)
- Expand `pack92` to `pack128` (32 words) to handle the 128-byte ucode images.
- Adjust the compile-time `assert!`s for the new port locations and phase magics:
  - The counter seed shifted from `0x1d/0x1e` to `0x24/0x25` (36/37).
  - The phase magics shifted to `0x28, 0x34, 0x46, 0x61, 0x69`.
  - The add/sub operation shifted to `0x57`.
- Insert the read-only Recon Probe at the start of the GPU initialization, before any engine writes.
- Add the H2/H3 sequence (with readback checks) after the recon probe.
- Update the `ucode-echo` host loop:
  - Send the command `3` to `CC_SCRATCH[0]` to trigger the `0x14100` Falcon-side read.
  - Wait for `ack != 0`.
  - Send the exit sentinel `2` to `CC_SCRATCH[0]`.
  - Spin for `CPUCTL` to show the STOPPED bit (`0x10`).

## 4. Post-Implementation Verification
After implementation, we will verify that the feature's strings (e.g. `:: kepler: recon CHAN_CUR=...`) are present inside `kernel.elf`. This check will be run directly against the artifact produced by `unaos/builder/` (the `esp-x86` media artifact), not against the `.rlib`, to prove the feature knob survives builder's feature mapping and is shipped enabled on metal.
