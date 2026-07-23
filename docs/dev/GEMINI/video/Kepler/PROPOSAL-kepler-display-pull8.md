STATUS: PROPOSED

# PROPOSAL — kepler-display pull 8: pattern-fill mapping decode

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull8-pattern.md`, this pull runs the identical latch sequence as Pull 7 but uses a specific test pattern in the VRAM surface to help visually decode how the hardware maps the surface to the screen (stride, tiling, or window bounds). It also adds a mid-hold dump of timing and shadow registers to search for state changes while the panel displays the new surface.

No implementation code will be written until this proposal is reviewed and approved (transitioning to `STATUS: APPROVED`).

## 2. Implementation Steps

### Step 1: Prepare Surface with Pattern
- The solid green fill will be replaced with a pattern calculated based on GOP width and height (`pitch = width * 4`).
- Fill geometry:
  - Rows `[0, h/4)`: RED `0xFFFF0000`
  - Rows `[h/4, h/2)`: GREEN `0xFF00FF00`
  - Rows `[h/2, 3h/4)`: BLUE `0xFF0000FF`
  - Rows `[3h/4, h)`: WHITE `0xFFFFFFFF`
- Override: The left 64 columns of every row will be filled with BLACK `0xFF000000`.
- Emit markers: 
  `:: kdisp: surf2 geom w=NNNN h=NNNN pitch=NNNN ::`
  `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN pattern=quarters+leftbar ::`

### Step 2: Pre-State Read
- Same as Pull 7. Read `0x640460`, `0x6101E0`, `0x61D1E0`.
- Emit: `:: kdisp: latch pre asm=XXXXXXXX armed=XXXXXXXX shadow=XXXXXXXX ::`

### Step 3: Arm Assembly
- Same as Pull 7. Write `0x00016000` to `0x640460` and read back.
- Emit: `:: kdisp: latch asm-wrote=00016000 rb=XXXXXXXX ::`

### Step 4: Self-Check Hold
- Same as Pull 7. Spin-loop for ~2 seconds.
- Every ~1 second, read `0x6101E0`.
- Emit: `:: kdisp: latch selfcheck t=<n>s armed=XXXXXXXX ::`
- Honesty Rule applies as before: Skip UPDATE write if the readback from Step 3 didn't match.

### Step 5: UPDATE Latch and Extended Hold
- Write `0x00000000` to `0x640080`.
- Read back `0x640080`.
- Emit: `:: kdisp: latch update-wrote rb0080=XXXXXXXX ::`
- Spin-loop for **8 seconds**.
- Every ~1 second, read `0x6101E0` and `HEAD_STAT` vertical.
- Emit: `:: kdisp: latch hold t=<n>s armed=XXXXXXXX stat vert=XXXXXXXX ::`
- **NEW**: At `t=4`, perform a one-time read-only dump of timing and shadow registers: `0x616340`, `0x61634C`, `0x6101E0`, `0x61D1E0`, and `0x61D014`.
- Emit: `:: kdisp: latch midhold 616340=XXXXXXXX 61634C=XXXXXXXX 6101E0=XXXXXXXX 61D1E0=XXXXXXXX 61D014=XXXXXXXX ::`

### Step 6: Restore
- Same as Pull 7. Restore `0x640460`, rewrite `0x640080`, wait ~2s, and read final state.
- Emit: `:: kdisp: latch restored asm=XXXXXXXX armed=XXXXXXXX shadow=XXXXXXXX ::`

### Step 7: Verdict
- Same as Pull 7. 
- Emit: `:: kdisp: latch verdict asm-stuck=<y|n> armed-followed=<y|n> ::`

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Write constraints**: Writes remain strictly limited to `0x640460` and `0x640080` plus the VRAM fill.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs cleanly on both arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all changed markers in `kernel.elf`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: All docs and code committed locally. `git status` clean. **No push will be performed.**
