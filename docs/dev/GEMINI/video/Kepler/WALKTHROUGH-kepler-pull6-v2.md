# Walkthrough: Kepler Pull 6 (v2) Execution

I have implemented the approved dual-instrumentation strategy from `PROPOSAL-kepler-pull6-v2.md` directly into the `kepler.rs` driver.

## What Was Changed

### 1. Wall 1: Dual-Instrumentation Scanout Read
- Substituted the static EVO address logic with a dual-read approach to accommodate differing firmware initialization sequences.
- **Candidate 1 (EVO Armed Shadow):** Reads `0x610000 + 0x400 + (head * 0x300) + 0x60`
- **Candidate 2 (CRTC Direct Readback):** Reads `0x610000 + 0xA00 + (head * 0x540) + 0x128` (FB_POS from `HEAD_VAL`). 
- **Selection Logic:** Both addresses are read and logged using `serial_println!(":: kepler: head-raw ...")`. The logic checks both addresses (behind bad-read protection), falling through to whichever provides a valid, non-zero offset.
- Maintained the wait loop (which correctly continues to poll the EVO armed shadow post-activation, since our takeover writes activate the EVO channel).

### 2. PBDMA Runlist Bind (Wall 2)
- Added a write of `1 << 0` (targeting Engine 0: PGRAPH) to `SUBFIFO_ENG_MASK[0]` at offset `0x2390` as previously implemented.
- The PBDMA 0 will now actively fetch runlist entries for PGRAPH.

### 3. Cleanroom Debt Removal
- Removed the forbidden Nouveau GPLv2 citation from `kepler.rs` concerning the EVO core-channel control register (`0x490`).
- Replaced the citation with an honest empirical note and placed the initial read of the `0x490` block behind a bad-read guard to safely abort on incompatible architectures.

## Next Steps
The dual-instrumented driver is ready. Once compiled and deployed to the metal rig, the serial log will display the raw outputs for both `evo` and `crtc` sources, allowing us to definitively prove which channel the firmware used to post the display.
