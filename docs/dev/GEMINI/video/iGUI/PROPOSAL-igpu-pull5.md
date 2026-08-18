# PROPOSAL — iGPU Pull 5: gmux Protocol Prove

STATUS: LANDED 9d904c96 (2026-07-22 — 32-bit version + required MAX_BRIGHTNESS row, ABI 7-wide both sides, gate logic intact. Gates green, strings-proven both artifacts. Metal owed: sitting #10.)
Prior: APPROVED WITH AMENDMENTS (2026-07-22 — the 8-bit-vs-32-bit version
read diagnosis is credible and cleanly explains the #9 gate failure; the
variant table matches known gmux hardware facts (facts with attribution are
this lane's standing rule; no GPL code bodies). Amendments:
A1 — MAX_BRIGHTNESS (0x70, 32-bit) is REQUIRED as the secondary proof row,
not optional: two independent known-shape reads make the protocol proof
robust against one register coincidentally looking sane.
A2 — decode stays gated exactly as in pull 4 (raw-only on unproven), and if
the gate PASSES, print both the decoded ownership verdict AND the raw bytes
so the log carries the evidence, not just the conclusion.
ABI unchanged as stated; land-review law; arch gate stays. Metal owed:
sitting #10.)

## The Hardware Facts: gmux Variants & Version Read
Research into the Linux `apple-gmux.c` driver reveals why the version self-test failed in Sitting #9 despite the handshake yielding stable switch bytes: **the version tuple is read differently depending on the gmux variant**.

1. **Classic PIO gmux** (Pre-Retina): Version is read as three separate 8-bit values at indices `0x04`, `0x05`, and `0x06`.
2. **Indexed gmux** (Pre-T2 Retina): Version is read as a **single 32-bit value** (`inl`) from index `0x04` (`GMUX_PORT_VERSION_MAJOR`). The byte breakdown is:
   - Major: `(version32 >> 24) & 0xFF`
   - Minor: `(version32 >> 16) & 0xFF`
   - Release: `(version32 >> 8) & 0xFF`
3. **T2 MMIO gmux**: Also uses a 32-bit read.

In Pull 4, we faithfully implemented the Indexed wait loops but mistakenly issued three separate 8-bit `index_read` operations for the version, which the Indexed microcontroller rejects or returns garbage for, causing our version gate to fail and quarantine the decode.

## Alternate Self-Tests
The `GMUX_PORT_MAX_BRIGHTNESS` register (`0x70`) is another known 32-bit register. If it returns a bounded sane value (like `0x03FF` or `0xFFFF`), it serves as a secondary protocol proof. However, correcting the version fetch to a 32-bit read should allow the primary gate to pass honestly.

## Strategic Implication
If correcting the 32-bit read causes the version tuple to pass the plausibility gate, we will un-quarantine the decode. If `SWITCH_DISPLAY` evaluates to `0x03` (Discrete GPU) and `DISCRETE_POWER` evaluates to `0x03` (Powered On), then **the iGPU is structurally divorced from the panel**. 

This completely explains why the iGPU `PP_STATUS` and `PP_CONTROL` were dead/zero. The "dead iGPU paradox" becomes the expected, correct hardware state, and the display bring-up arc must immediately pivot to the Kepler dGPU.

## Implementation Plan (ABI Law)
1. **`bootloader/src/main.rs` & `igpu.rs`**: 
   - Introduce an `index_read32(reg: u8) -> u32` helper that writes the index to `0x7D0` but reads the value via `inl(0x7C2)`.
   - Update the version fetch to perform a single `index_read32(0x04)` and split the result into Major, Minor, and Release bytes to populate the first 3 indices of the `[u32; 6]` array.
   - The switch/power registers (`0x10`, `0x28`, `0x50`) remain 8-bit `index_read` operations.
2. **ABI Unchanged**: The `[u32; 6]` trace structure remains exactly the same. The version gate logic in `igpu.rs` remains exactly the same. Only the *method* of fetching the version at Point-0 and Point-3 changes.
3. **Verification**: Build with `UNAOS_IVB=1` and ensure the binary string-proves the gate (which remains intact).
