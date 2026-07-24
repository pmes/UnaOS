STATUS: PROPOSED

# Proposal: kepler-fence pull 17 — window-vs-latch correlation (read-only)

## Context & Objectives
Sitting #19 proved that the `0x640000` window is stable within a boot (our beacons were not seen and pass1→pass2 showed zero changed words) but varies across boots. Since the display lane's latch (`0x640460` + `0x640080` UPDATE) drives this engine, we want to know if the latch UPDATE perturbs the window. If it does, the window is core-channel processing state, and the display lane's write path is our way into it.

## Implementation Plan
1. **Pre-Takeover Dump (pass pre):**
   Before calling `crate::drivers::gpu::kepler_display::takeover_display(...)`, perform a dense dump of the `0x640000–0x6403FC` window and store it in a local array (256 words).
   Print the read values with the marker `:: kepler: mirror-hdr pre off={:03X} val={:08X} ::`.
   Print `:: kepler: mirror-hdr pre done rows=256 ::`.

2. **Run `takeover_display`:**
   (Already in the code, untouched). This will execute the display lane's pull-10 latch logic.

3. **Post-Takeover Comparison (during pass 0):**
   Keep the existing `pass0` dump, beacon planting, `pass1` (with beacon scan), delay, and `pass2` exactly as they are.
   During `pass0`, compare each read value against the stored `pre` value:
   - If they differ, print: `:: kepler: latch-delta off={:03X} pre={:08X} post={:08X} ::`
   - Track if any differences were found. If identical across all 256 words, print: `:: kepler: latch-delta none ::`.

## Gates & Compliance
- **Read-Only MMIO**: Only read operations for the new pre-dump. Beacon BAR1 writes remain unchanged.
- **Full-knob Check**: Run `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` for both architectures.
- **Strings Proof**: Run `arroyo build esp-x86` and confirm `strings` proof of the new markers (`mirror-hdr pre`, `latch-delta`) in `kernel.elf`.
- **Clean Tree**: Remove scratch files, ensure `git status` is clean, and commit all code and docs.
- **No Push**: Report `PUSH OWED: n`.
