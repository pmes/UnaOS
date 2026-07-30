# PROPOSAL: Kepler Fence Pull 35 - Poison Order and Access Ledger

## 1. Goal
Address the two ordering defects identified in pull 34 (removing the poison offset read from the head recon probe and testing the H2/H3 writes via an A/B pair) and implement the new FECS access ledger.

## 2. Defect Fixes
### Fix 1: Head Recon Probe
The `0x409504` register read will be explicitly removed from the head recon probe at the top of `kepler::init`. Only the remaining five unpoisoned registers (`0x409b00`, `0x409b04`, `0x409c00`, `0x409c08`, `0x409500`) will be read. The log line will be updated to match. 

### Fix 2: H2/H3 Writes A/B Pair
The H2/H3 writes (`ENGINE_STATUS <= 2`, `ENGINE_TRIGGER <= 1`) will be placed inside a host-side loop, just before the ucode-echo sched-status witnesses. Since the ucode port question is settled (s37 acked img=A), we will drop the ucode B image arm entirely. The loop will test the H2/H3 variable using the settled A image for both passes:
- **h2h3=on**: The host will perform the H2/H3 writes before submitting the ucode. The attempt will be labelled (`:: kepler: ucode-echo start h2h3=on ::`).
- **h2h3=off**: The host will skip the H2/H3 writes (`:: kepler: ucode-echo start h2h3=off ::`).
This preserves the experiment and allows us to observe if the H2/H3 writes affect channel validation (via the `err=` witness) without contaminating the global reset state before `bind-pre`.

## 3. FECS Access Ledger
We will route all FECS accesses (`0x409000`–`0x409FFF`) through a new `fecs_read`/`fecs_write` pair.
The ledger will be implemented using atomic variables to track:
- `FECS_ACCESS_COUNT`: Total number of accesses (reads and writes).
- `FECS_FIRST_OFFSET`: The first offset touched this boot.
- `FECS_504_READ_TOUCHED`: Boolean, true if `0x409504` was READ.
- `FECS_504_READ_INDEX`: The index of the first `0x409504` read, or a sentinel (`0xFFFFFFFF`) printed as `none`.
- `FECS_504_WRITE_TOUCHED`: Boolean, true if `0x409504` was WRITTEN.
- `FECS_504_WRITE_INDEX`: The index of the first `0x409504` write, or `none`.

The terminal ledger will be printed at *two checkpoints*:
1. Immediately before the terminal poke.
2. At the very end of `kepler::init`.
This ensures the ledger survives a wedged boot.

*Clarification on Ledger Mechanism*: The wrappers detect a violation and make it visible in every capture. They do not prevent a developer from bypassing them by calling `mmio_read` directly, but they guarantee that compliant calls will be strictly ordered and recorded. GPCCS (`0x41A000`) is explicitly out of scope for the FECS ledger, and `0x409500` will continue to be read independently.

### Ledger Output on Healthy vs Poisoned Boot
- **Healthy Boot**: `504_read_touched=false`, `504_read_idx=none`. `504_write_touched=true`, `504_write_idx=N` (where N is the very last index, representing the terminal poke).
- **Poisoned Boot**: `504_read_touched=true`, `504_read_idx=M` (where M < N, indicating an illegal read occurred earlier in the boot), explaining any subsequent `BADF1000` faults.

## 4. Falcon-Side Read (Carried Forward)
The falcon-side read of `0x409504` inside the microcode via `iord` port `(0x14100)` remains intact and will execute natively.
