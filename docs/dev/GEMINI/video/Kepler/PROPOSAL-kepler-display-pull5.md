STATUS: APPROVED (2026-07-23 — clean match to brief, no amendments)

# PROPOSAL — kepler-display pull 5: repoint-the-surface

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull5-repoint.md`, this pull performs the **first Peter-approved display write** to test the hypothesis that `0x6101E0` is the live, bare scanout surface pointer. 

No implementation code will be written until this proposal is reviewed and approved (transitioning to `STATUS: APPROVED`).

## 2. Implementation Steps

This logic will be placed in the display takeover phase (behind `nvidia-kepler-takeover`), superseding the previous EVO core channel flip logic which did not succeed.

### Step 1: Prepare Second Surface
- Calculate the total framebuffer bytes (`width * height * 4`).
- Fill VRAM via BAR1 at the hardcoded offset `0x1600000` with the solid color `0xFF00FF00` (green).
- Emit: `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN fill=FF00FF00 ::`

### Step 2: Pre-Repoint State
- Read `0x6101E0` (the original pointer).
- Read `HEAD_STAT` (`vert` at `base+0x340`, `horz` at `base+0x344`).
- Emit: `:: kdisp: repoint pre 6101E0=XXXXXXXX stat vert=XXXXXXXX horz=XXXXXXXX ::`

### Step 3: Repoint
- Write `0x00016000` (which is `0x1600000 >> 8`) to `0x6101E0`.
- Read back the register to verify if it stuck.
- Emit: `:: kdisp: repoint wrote=00016000 rb=XXXXXXXX ::`

### Step 4: 5-Second Bounded Hold
- Spin-loop for approximately 5 seconds.
- Every ~1 second, read `HEAD_STAT` and emit: 
  `:: kdisp: repoint hold t=<n>s stat vert=XXXXXXXX horz=XXXXXXXX ::`

### Step 5: Restore
- Write the original value back to `0x6101E0`.
- Read back the register.
- Emit: `:: kdisp: repoint restored rb=XXXXXXXX ::`
- Spin-loop for approximately 2 seconds to allow the panel to visually recover.

### Step 6: Verdict
- Evaluate if the readback during Step 3 matched the written value.
- Emit: `:: kdisp: repoint verdict rb-stuck=<yes|no> ::`
- Explicit constraint: If the readback does not stick or the panel does not change, we do NOT improvise other register writes or doorbells. The test remains strictly limited to `0x6101E0`.

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Write constraints**: The ONLY new register written is `0x6101E0`. No improvised doorbells.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs cleanly on both arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all new `:: kdisp: repoint` markers in `kernel.elf`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: All docs and code will be committed. Scratch files will be deleted, and `git status` will be clean. **No push will be performed.**
