# Kepler Pull 3 - Implementation Plan

This plan addresses the bugs and features requested in `PLAN-GEMINI-kepler-pull3.md`.

## Proposed Changes

### 1. Phase 1 — Bug Fixes

#### [MODIFY] `kepler.rs` (VRAM Size Decode)
- **Fix:** Update `NV_PFB_RAM_AMOUNT` from `0x10020c` to `0x10f20c` for Kepler (which represents `PBFB_BROADCAST + MEM_AMOUNT`).
- **Fix:** Read the register, treating the raw value as MBs (per `shr="20"` in `envytools`). Convert it to bytes by shifting left by 20.
- **Validation:** Tighten the 16MB..32GB sanity check to also ensure the size in MB is a power of 2, or `3n/4` for asymmetric configs.

#### [MODIFY] `video/fbcon.rs` (Framebuffer Base Getter)
- **Fix:** Add an out-of-lane getter `pub fn current_base() -> Option<u64>` to query the active FBCON base. `FBCON` is initialized with the bootloader's true frame buffer base during `fbcon::init()`.

#### [MODIFY] `kepler.rs` (GOP Base Fetch)
- **Fix:** Change `takeover_display` to read the framebuffer base via `crate::video::fbcon::current_base()` instead of `crate::video::WRITER`.

---

### 2. Phase 2 — Witness and Plumbing

#### [MODIFY] `kepler.rs` & `pci.rs` (Witness Split)
- **Fix:** Modify `kepler::init` to return `Result<(), &'static str>`. If it fails (e.g. invalid VRAM size, missing BARs), it prints `:: kepler: probe-abort <reason> ::`.
- **Fix:** Ensure `pci.rs` only prints `:: kepler: no-device ::` when no Kepler device is discovered during scan.

#### [MODIFY] `builder/src/main.rs`, `arroyo`, `Cargo.toml` (Knob Plumbing)
- **Fix:** Wire `UNAOS_KEPLER_TAKEOVER` and `UNAOS_KEPLER_FIFO` to Cargo features `nvidia-kepler-takeover` and `nvidia-kepler-fifo`. Update `kepler.rs` to use `cfg!(feature = "...")` instead of `option_env!`.

---

### 3. Phase 3 — EVO display flip

#### [MODIFY] `kepler.rs` (EVO Flip)
- **Feature:** Implement the descoped EVO flip in `takeover_display`.
- Allocate an EVO pushbuffer, submit the surface-address methods + `UPDATE` method (using envytools disp method IDs), and verify via readback that the head latched.
- Copy existing GOP contents to the new surface *before* the flip so it is visually seamless.
- Keep this behind `nvidia-kepler-takeover`.

---

### 4. Phase 4 — Fence re-run readiness

#### [MODIFY] `kepler.rs` (Diagnostic witnesses)
- **Feature:** Re-audit USERD/semaphore VRAM offsets based on the fixed VRAM size logic.
- Add `:: kepler: fifo-layout userd=<off> fence=<off> gp=<put>/<get> ::` right before the poll.
- If the fence times out, read back `GP_GET` and channel status raws into the abort witness to localize stalls.
