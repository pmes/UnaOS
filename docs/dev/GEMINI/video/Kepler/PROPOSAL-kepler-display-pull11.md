STATUS: APPROVED (2026-07-24 — clean match; dropping the midhold dump is fine, the brief didn't ask for it)

# PROPOSAL — kepler-display pull 11: block-height step ladder

## 1. Intent & Scope
Following `BRIEF-kepler-display-pull11-blockheight.md`, this pull executes a sequence of four display takeover cycles in one boot, testing block heights `bh` ∈ {2, 4, 8, 16}. For each cycle, it populates VRAM with the `ruler64x8` pattern mapped through the full GOB block-linear transform, pushes the pointer to the hardware, latches it, holds for 4 seconds, and restores it. This sweep will visually identify the exact GOB stacking configuration expected by the Kepler display controller.

No implementation code will be written until this proposal is reviewed and approved (transitioning to `STATUS: APPROVED`).

## 2. Implementation Steps

### Loop over `bh` in `[2, 4, 8, 16]`
The phase 2 takeover logic will be restructured into a loop for the four block heights. Before the loop begins, the pre-state (`0x640460`, `0x6101E0`, `0x61D1E0`) will be recorded.

Inside the loop, for each `bh`:

#### Step 1: Pre-Swizzled Ruler Pattern Fill
- Clear/overwrite VRAM using the `bh` parameter.
- The address arithmetic is extended for block stacking:
  ```rust
  // GOB dimensions (fixed 64x8 bytes)
  let gob_width_bytes = 64;
  let gob_height = 8;
  let gob_size_bytes = 512;
  let gobs_per_row = expected_pitch / gob_width_bytes;
  
  // Transform logic per pixel
  let gob_y = y / gob_height;
  let inner_y = y % gob_height;
  
  let px_byte_x = x * 4;
  let gob_x = px_byte_x / gob_width_bytes;
  let inner_x = px_byte_x % gob_width_bytes;
  
  let blk_y = gob_y / bh;
  let blk_inner = gob_y % bh;
  let blk_index = (blk_y * gobs_per_row) + gob_x;
  
  let target_byte_addr = (blk_index * (bh * gob_size_bytes)) 
                       + (blk_inner * gob_size_bytes) 
                       + (inner_y * gob_width_bytes) 
                       + inner_x;
  ```
- Markers emitted:
  `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN pattern=ruler64x8-gob64x8-bh<N> ::`
  `:: kdisp: bh-step bh=<N> fill done ::`

#### Step 2: Latch Sequence
- Write `0x00016000` to `0x640460` (assembly pointer).
- Check if assembly stuck (honesty rule: if not, skip `UPDATE` latch and restore, then continue loop).
- If it stuck, write `0` to `0x640080` (UPDATE doorbell).
- Hold for 4 seconds. Output `:: kdisp: bh-step bh=<N> hold t=<t>s ::` every second.
- Mid-hold dump removed for this run to keep the loop output tidy (or can be retained if preferred, but not strictly requested by brief).

#### Step 3: Restore
- Write `pre_asm` back to `0x640460`.
- Write `0` to `0x640080`.
- Wait 1 second (recovery gap).
- Output `:: kdisp: bh-step bh=<N> done ::`

## 3. Gates
Before concluding this pull, I will ensure the following gates are passed:
- **Write constraints**: Writes remain strictly limited to `0x640460` and `0x640080` plus the VRAM fill, executed identically four times.
- **Full-knob check**: `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` runs cleanly on both arches.
- **Builder-path build**: `esp-x86` builds properly.
- **Strings proof**: `strings` shows all changed markers in `kernel.elf`.
- **QEMU Regression**: Default QEMU regression runs green.
- **Hygiene**: All docs and code committed locally. `git status` clean. **No push will be performed.**
