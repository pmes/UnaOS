STATUS: APPROVED (2026-07-24 — clean match to brief; beacons overwrite only OUR dead-channel structures, acceptable)

# Proposal: kepler-fence pull 16 — mirror-window backing-store beacon test

## Context & Objectives
Sitting #18 revealed that the `mirror-hdr` window (`0x640000–0x6403FC`) is highly volatile without us writing any MMIO state, suggesting it might be an aperture onto live memory (like the core-channel pushbuffer or USERD) rather than a config register file. To test this hypothesis, we will plant recognizable "beacon" patterns in the memory regions we own (via BAR1) and check if they surface in the `mirror-hdr` window.

## Implementation Plan
1. **Baseline Dump (pass 0):**
   Dump the `0x640000–0x6403FC` window using the existing `mirror-hdr` marker format (pass 0).

2. **Plant Beacons (BAR1 writes only):**
   Write an 8-word pattern `0xBEAC0001` through `0xBEAC0008` to each of our three owned channel structures via BAR1:
   - `userd_off`
   - `pb_off`
   - `runlist_off` (using a safe offset within the runlist, e.g. base + 0x100 if we have space, or simply overwriting unused space in the structure). *Note: We will write these starting exactly at the respective base offsets to ensure visibility.*
   Print `:: kepler: beacon planted at=<name> off=XXXXXXXX ::` for each planting.
   **Crucially: Zero MMIO register writes will occur in this pull.**

3. **Post-Plant Dump (pass 1):**
   Dump the window again as pass 1. 

4. **In-Code Comparison Scan:**
   During pass 1, check if the read value matches any of `0xBEAC0001..0xBEAC0008`. 
   - If a match is found: print `:: kepler: beacon SEEN off=XXX val=XXXXXXXX ::`.
   - Track if any were seen. If none were seen across the entire pass 1 dump, print `:: kepler: beacon none-seen ::`.

5. **Delay & Volatility Re-Check (pass 2):**
   Wait for a bounded ~2s delay (`core::hint::spin_loop` x 2_000_000).
   Perform a third dump (pass 2) to check volatility with the beacons in place.

## Gates & Compliance
- **Read-Only MMIO**: Only BAR1 VRAM writes are used to plant the beacons. No MMIO writes.
- **Full-knob Check**: Run `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` for both architectures.
- **Strings Proof**: Run `arroyo build esp-x86` and confirm `strings` proof of `beacon planted` and `beacon SEEN` markers in `kernel.elf`.
- **Clean Tree**: Remove scratch files, ensure `git status` is clean, and commit all code and docs.
- **No Push**: Report `PUSH OWED: n`.
