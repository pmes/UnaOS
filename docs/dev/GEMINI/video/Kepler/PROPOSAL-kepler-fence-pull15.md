STATUS: APPROVED (2026-07-23 — clean match to brief; removal (not just quieting) of the refuted CTRL_ADDR ladder is the preferred form)

# Proposal: kepler-fence pull 15 — method-mirror header recon (read-only)

## Context & Objectives
Sitting #17 proved the EVO method-mirror write + UPDATE path is live. The disp-era USERD enablement fallback now has a working mechanism to ride. Before we attempt to write to any USERD/channel-control state, we need to map the method-mirror header region.
The region `0x640000–0x6403FC` (256 words) has never been dumped and is expected to contain channel-control, USERD-linkage, and interrupt/notify slots.

## Implementation Plan
1. **Disable Refuted Logic:**
   The `CTRL_ADDR` TARGET audit from pull 14 has been cleanly refuted. We will remove or quiet its execution so it does not clutter the log.

2. **Read-Only Reconnaissance:**
   In the existing disp-recon section of `unaos/crates/kernel/src/drivers/gpu/kepler.rs`, we will implement a read-only dense dump of the `0x640000–0x6403FC` region.
   
   - Base address: `0x640000`
   - We will run 2 passes (`pass0` and `pass1`).
   - For each pass, we will read 256 32-bit words (offsets `0x0` to `0x3FC`, step `4`).
   - Every word will be printed (zeros printed as zeros).
   - Marker format (relative offset `XXX`): `:: kepler: mirror-hdr pass<P> off=XXX val=XXXXXXXX ::`
   - At the end of each pass, we will print: `:: kepler: mirror-hdr pass<P> done rows=256 ::`
   - Between passes (i.e. after pass 0), we will execute a bounded delay (using the `core::hint::spin_loop` idiom with an appropriate iteration count like 2_000_000, similar to the display pull 3/4 idiom).

3. **Gates & Compliance:**
   - No new writes will be introduced to the Kepler MMIO space in this pull.
   - We will run the full-knob gate (`UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check`).
   - We will run `arroyo build esp-x86` and provide `strings` proof of the new markers in `kernel.elf`.
   - Ensure `git status` is clean and all scratch files are removed.
   - Per the brief, we will commit but NOT push the implementation, reporting `PUSH OWED: n`.
