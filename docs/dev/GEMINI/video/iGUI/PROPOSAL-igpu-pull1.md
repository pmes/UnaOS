STATUS: LANDED f73c85eb+4f819d8a (2026-07-22 — both amendments honored: GGTT
treated as offset with PTE-window inspection, full geometry/tiling/PIPESRC
dumps, DP_A readback, and the black-panel Fox cross-check callout. Read-only
throughout. `./arroyo check` green both arches + QEMU suite green. Two
non-blocking nits in REVIEW-igpu-pull1.md §Landing. Metal owed: sitting #6.)
Prior: APPROVED WITH AMENDMENTS (2026-07-22 — see REVIEW-igpu-pull1.md in
this directory; the amendments are binding: DSPxSURF holds a GGTT offset, not
a CPU physical address, and milestone 1 must also dump stride/tiling/PIPESRC.
Milestone 1 read-only pass may proceed as amended; the repoint milestone needs
the GGTT answer from metal first.)

# PROPOSAL — iGPU (Intel HD 4000) Pull 1: Read-only Probe & Framebuffer Repoint

## 1. Probe Plan

- **PCI Location**: Locate the iGPU at PCI address `00:02.0`.
- **Identifiers**: Match Vendor ID `0x8086` and Device ID `0x0166` (Ivy Bridge GT2).
- **BAR Enumeration**:
  - **BAR 0 (`GTTMMADR`)**: Memory-Mapped I/O and Global GTT. Typically a 4MB region, where the first 2MB provides access to MMIO registers and the remaining 2MB maps the Global Graphics Translation Table (GGTT) aperture.
  - **BAR 2 (`GMADR`)**: Graphics Memory Address Range aperture (usually 256MB) for CPU access to GPU memory.

## 2. Scanout Derivation & Instrumentation

Our goal is to transition from "GOP left the panel lit" to pointing the display at our own framebuffer without touching the gmux or modesetting. We will achieve this in two steps: a read-only instrumentation pass, followed by an inherit-and-repoint pass.

**Read-Only Instrumentation (Milestone 1)**:
We will read the following registers to determine the active display pipe, plane, and transcoder left active by GOP:
- **Plane Control (`DSPxCNTR`)**: `DSPACNTR` (0x70180), `DSPBCNTR` (0x71180), `DSPCCNTR` (0x72180). We check bit 31 (Enable) to identify which plane is active.
- **Pipe Configuration (`PIPExCONF`)**: `PIPEACONF` (0x70008), `PIPEBCONF` (0x71008), `PIPECCONF` (0x72008). We check bit 31 (Enable) to verify the pipe status.
- **Plane Surface Base (`DSPxSURF`)**: `DSPASURF` (0x7019C), `DSPBSURF` (0x7119C), `DSPCSURF` (0x7219C). This points to the physical address of the current GOP framebuffer (expected `0x90020000`).

**Inherit and Repoint**:
Once the active plane (e.g., Plane A) is confirmed via instrumentation, we will:
1. Retain the existing GOP mode and pipe/plane configuration.
2. Write our new UnaOS framebuffer physical address into the active `DSPxSURF` register.
3. The hardware will seamlessly scan out from the new address on the next VBLANK.

## 3. Citations

Cleanroom source of record: **Intel Open Source HD Graphics Programmer's Reference Manual (PRM) - Volume 3 Part 1: Display Registers (Ivy Bridge)**.

- `PIPEACONF` - Pipe A Configuration Register: Offset `0x70008`
- `DSPACNTR` - Display Plane A Control: Offset `0x70180`
- `DSPASURF` - Display Plane A Surface Base Address: Offset `0x7019C`

*(Linux i915 source has strictly been avoided in the preparation of this proposal.)*

## 4. Scope Fence

- **NO** gmux control.
- **NO** Kepler (dGPU) interaction.
- **NO** mode-setting (clock, timing, PLL programming). We rely purely on the mode pre-programmed by GOP.

## 5. Honesty Lines

- **"NOT COMPILED HERE — Mac owed"**: As the physical hardware is remote, all metal behavior and exact register states left by GOP are unverified locally. The initial read-only instrumentation is strictly required to establish ground truth on the active pipe and plane.
- **"metal owed"**: The final repoint logic will be blind until tested on metal by Fox during sitting #6.

## Proposed Module Lane

A new kernel module will be introduced under the video subsystem at `igpu.rs` (or similar namespace like `intel/hd4000.rs`).
The builder wiring will map to an `UNAOS_IGPU` feature flag to ensure isolation from `kepler.rs`. Kepler files will remain completely untouched.
