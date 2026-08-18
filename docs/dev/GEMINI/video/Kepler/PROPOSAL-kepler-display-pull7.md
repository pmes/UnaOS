STATUS: APPROVED WITH ONE AMENDMENT (2026-07-23): on the Step-4 skip path (asm rb unchanged), Step 6 performs NO writes — the original value is still in place and an UPDATE write on unarmed state would be an uncontrolled first doorbell. Skip path = re-read all three registers, report, verdict. Otherwise clean match.

# PROPOSAL — kepler-display pull 7: assembly write + UPDATE latch

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull7-latch.md`, this pull tests the armed-vs-assembly hypothesis by writing the assembly surface pointer (`0x640460`) and triggering the UPDATE latch (`0x640080`).

No implementation code will be written until this proposal is reviewed and approved (transitioning to `STATUS: APPROVED`).

## 2. Implementation Steps

### Step 1: Remove Pull 6 Code and Prepare Surface
- Remove the Phase 2 `takeover_display` block from pull 6 (the M1/M2/M3 read-only scans).
- Prepare a secondary surface at VRAM offset `0x1600000`, filling it with `0xFF00FF00` (green).
- Emit: `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN fill=FF00FF00 ::`

### Step 2: Pre-State Read
- Read `0x640460` (assembly), `0x6101E0` (armed), and `0x61D1E0` (shadow).
- Emit: `:: kdisp: latch pre asm=XXXXXXXX armed=XXXXXXXX shadow=XXXXXXXX ::`

### Step 3: Arm Assembly
- Write `0x00016000` to `0x640460`.
- Read back `0x640460`.
- Emit: `:: kdisp: latch asm-wrote=00016000 rb=XXXXXXXX ::`

### Step 4: Self-Check Hold
- Spin-loop for approximately 2 seconds.
- Every ~1 second, read `0x6101E0` (armed).
- Emit: `:: kdisp: latch selfcheck t=<n>s armed=XXXXXXXX ::`
- **Honesty Rule**: If the readback in Step 3 did not match `0x00016000`, we skip Step 5 (the UPDATE latch), emit `:: kdisp: latch skip — asm rb unchanged ::`, and proceed directly to Step 6.

### Step 5: UPDATE Latch (conditional)
- If the assembly register took the write, trigger the latch by writing `0x00000000` to `0x640080`.
- Read back `0x640080`.
- Emit: `:: kdisp: latch update-wrote rb0080=XXXXXXXX ::`
- Spin-loop for approximately 5 seconds.
- Every ~1 second, read `0x6101E0` (armed) and `HEAD_STAT` vertical.
- Emit: `:: kdisp: latch hold t=<n>s armed=XXXXXXXX stat vert=XXXXXXXX ::`

### Step 6: Restore
- Restore the original value to `0x640460`.
- Trigger the latch again by writing `0x00000000` to `0x640080`.
- Spin-loop for approximately 2 seconds to allow the panel to visually recover.
- Re-read all three registers (`0x640460`, `0x6101E0`, `0x61D1E0`).
- Emit: `:: kdisp: latch restored asm=XXXXXXXX armed=XXXXXXXX shadow=XXXXXXXX ::`

### Step 7: Verdict
- Determine if the assembly write stuck (from Step 3) and if the armed read followed it (from Step 5).
- Emit: `:: kdisp: latch verdict asm-stuck=<y|n> armed-followed=<y|n> ::`

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Write constraints**: The ONLY new registers written are `0x640460` and `0x640080`. No other registers or doorbells.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs cleanly on both arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all new `:: kdisp: latch` markers in `kernel.elf`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: All docs and code will be committed. Scratch files will be deleted, and `git status` will be clean. **No push will be performed.**
