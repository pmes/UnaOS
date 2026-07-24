STATUS: APPROVED (2026-07-24 — clean match to brief, no amendments)

# PROPOSAL — kepler-display pull 9: ruler pattern (pitch + row-mapping solve)

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull9-ruler.md`, this pull runs the identical latch sequence as Pull 8 but introduces a new "ruler" test pattern in VRAM. This pattern is designed to precisely calculate the actual pitch of the frame buffer and exactly which scanlines are mapped to the display region (through cyclic color blocks and a wide high-contrast left-side marker).

No implementation code will be written until this proposal is reviewed and approved (transitioning to `STATUS: APPROVED`).

## 2. Implementation Steps

### Step 1: Prepare Surface with Ruler Pattern
- The VRAM fill pattern will be replaced with a `ruler64x8` pattern.
- Fill geometry:
  - Vertical color blocks cycling every 64 rows:
    - Block 0: RED (`0xFFFF0000`)
    - Block 1: GREEN (`0xFF00FF00`)
    - Block 2: BLUE (`0xFF0000FF`)
    - Block 3: YELLOW (`0xFFFFFF00`)
    - Block 4: CYAN (`0xFF00FFFF`)
    - Block 5: MAGENTA (`0xFFFF00FF`)
    - Block 6: WHITE (`0xFFFFFFFF`)
    - Block 7: GRAY (`0xFF404040`)
  - A thin tick line: Every row where `(row % 64) == 0` will be pure BLACK (`0xFF000000`).
  - Pitch probe override: For EVERY row, the left-most 256 pixels will be forced WHITE (`0xFFFFFFFF`), followed by 8 BLACK pixels (`0xFF000000`).
- Emit markers: 
  `:: kdisp: surf2 geom w=NNNN h=NNNN pitch=NNNN ::`
  `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN pattern=ruler64x8 ::`

### Step 2-7: Identical Latch Sequence
- Steps 2 through 7 from Pull 8 remain unchanged.
- Pre-state read and arming `0x640460`.
- Honesty rule (skip if write fails).
- `0x640080` UPDATE latch and 8-second hold.
- Mid-hold dump of `0x616340`, `0x61634C`, `0x6101E0`, `0x61D1E0`, and `0x61D014` at `t=4`.
- Restore `0x640460` and re-trigger `0x640080`.
- Final verdict read.

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Write constraints**: Writes remain strictly limited to `0x640460` and `0x640080` plus the VRAM fill.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs cleanly on both arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all changed markers in `kernel.elf`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: All docs and code committed locally. `git status` clean. **No push will be performed.**
