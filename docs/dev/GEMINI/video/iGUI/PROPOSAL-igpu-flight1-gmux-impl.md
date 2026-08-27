# Goal Description

Implement Flight 1 of the iGPU blitter arc: a bounded, knob-gated gmux switch to route the internal panel to the iGPU (Ivy Bridge) while leaving Kepler boot unchanged. This will allow the blitter machinery to arm and prove itself on the physical hardware.

## User Review Required

Please review the bounding logic and fallback strategy. If the gmux switch fails (the iGPU doesn't produce an advancing scanout within budget), it will automatically revert to the Discrete GPU (Kepler) to prevent a black-forever boot.

## Proposed Changes

### Build System & Flags

#### [MODIFY] [arroyo](file:///home/pmes/src/github.com/pmes/UnaOS-gemini/unaos/arroyo)
Add support for the `UNAOS_GMUX_SWITCH=1` environment variable.

#### [MODIFY] [main.rs](file:///home/pmes/src/github.com/pmes/UnaOS-gemini/unaos/builder/src/main.rs)
Translate the `UNAOS_GMUX_SWITCH` environment variable into the `gmux_switch` cargo feature.

#### [MODIFY] [Cargo.toml](file:///home/pmes/src/github.com/pmes/UnaOS-gemini/unaos/crates/kernel/Cargo.toml)
Declare the `gmux_switch` feature flag.

---

### GPU Drivers

#### [MODIFY] [igpu.rs](file:///home/pmes/src/github.com/pmes/UnaOS-gemini/unaos/crates/kernel/src/drivers/gpu/igpu.rs)
- **Implement `index_write`:** Add a helper for GMUX port writes (writing data to `0x7C2`, then the index to `0x7D0`).
- **GMUX Switch Sequence (Gated by `gmux_switch`):**
  - Switch DDC to IGD (index `0x28`, value `1`).
  - Switch DISPLAY to IGD (index `0x10`, value `1`).
  - *Serial-First Witnessing:* Emit a readback-verified witness line before and after each GMUX write.
- **Bounded Fallback:**
  - After switching, wait briefly and poll `PIPE_FRMCOUNT_A` (0x70040) to verify it is advancing, and verify the plane is enabled.
  - If the scanout fails to advance within a tight cycle budget (e.g., 50ms equivalent), immediately revert the gmux to DIS (value `2`) and emit a one-line failure witness: `:: gmux: switch failed, reverted to DIS ::`.
  - The switch occurs *before* the pull-7 census so that `active_surf` accurately reflects the new state and arms the blitter.

## Verification Plan

### Automated Tests
- Run `./arroyo check` for both `x86_64` and `aarch64` to verify no regressions in default builds.

### Manual Verification
- Provide a test boot using `UNAOS_GMUX_SWITCH=1 UNAOS_IVB=1 ./arroyo esp-x86`.
- Verify the serial log demonstrates:
  1. The step-by-step gmux write witness lines.
  2. The frame counter advancing, confirming the switch.
  3. `active_surf` going non-None and the blitter arming (`igpu-blt: ring=up`).
  4. The WXPROBE FBWC lines matching the new expected framebuffer base (iGPU stolen memory).

---

## Flight 2 Proposal

For Flight 2 (the no-Kepler boot, bypassing the 397 ms initialization entirely), we must solve the ignition gap: `desktop_uefi::activate()` is currently solely called at the end of Kepler takeover. I propose moving the `desktop_uefi::activate()` ignition into a generalized video handoff seam, or calling it directly at the end of `igpu::init()` when we detect a proven, live iGPU scanout and `UNAOS_KEPLER=0` (or when Kepler is bypassed). This decouples the window compositor from Kepler, allowing the machine to boot directly to the iGPU's framebuffer and reclaim the 397 ms.
