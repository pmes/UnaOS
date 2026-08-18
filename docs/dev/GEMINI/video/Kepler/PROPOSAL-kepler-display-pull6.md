STATUS: APPROVED (2026-07-23 — clean match to brief, no amendments)

# PROPOSAL — kepler-display pull 6: assembly-state hunt

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull6-assembly.md`, this pull removes the refuted pull 5 repoint logic (read-only execution only) and searches for the assembly state that ultimately latches into `0x6101E0`. 

No implementation code will be written until this proposal is reviewed and approved (transitioning to `STATUS: APPROVED`).

## 2. Implementation Steps

### Step 1: Remove Refuted Code
- Remove the Phase 2 `takeover_display` block from pull 5 (the surface prep, `0x6101E0` repoint, holds, and verdict).
- This ensures we remain strictly read-only for pull 6.

### Step 2: Milestone 1 — The Gap Region
- Read the window `0x610000–0x6103FC` (which includes `0x6101E0`).
- Execute two passes separated by a bounded delay (`core::hint::spin_loop` for approx. 2 frames).
- We will store the first read of `0x6101E0` during Pass 0 for the Milestone 3 pair check.
- Emitted markers (`off` relative to `0x610000`):
  - `:: kdisp: gap pass<P> off=XXX val=XXXXXXXX ::`
  - `:: kdisp: gap pass<P> done rows=N ::`

### Step 3: Milestone 2 — Widened Known-Value Scan
- Re-run the pull 4 scan predicate (checking for `0x00000200`, `0x00020000`, `0x90020000`, pitch `0x00002D00`, fb size `0x013C6800`, raster totals, and BAR/geometry shapes).
- Scan ranges: `0x614000–0x61FFFC` and `0x640000–0x647FFC`.
- Track up to 64 hits in an array (storing the offset and the M2 value).
- Emitted markers:
  - `:: kdisp: evo-scan2 hit off=XXXXXX val=XXXXXXXX key=<name> ::`
  - `:: kdisp: evo-scan2 done ranges=614000-61FFFC,640000-647FFC hits=N capped=<t|f> ::`

### Step 4: Milestone 3 — Armed-Pair Check
- After M1 and M2 have completed (thus separated by time), re-read `0x6101E0`.
- Iterate through the saved hits from M2 and re-read their offsets.
- Emitted markers:
  - `:: kdisp: pair off=6101E0 first=XXXXXXXX second=XXXXXXXX ::`
  - `:: kdisp: pair off=XXXXXX first=XXXXXXXX second=XXXXXXXX ::` (for each M2 hit)

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Read-only execution**: No register writes will be performed; previous repoint code is removed.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs cleanly on both arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all new `:: kdisp:` markers in `kernel.elf`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: All docs and code will be committed. Scratch files will be deleted, and `git status` will be clean. **No push will be performed.**
