# PROPOSAL — iGPU Pull 2: Scanout Teardown Hunt

STATUS: APPROVED WITH AMENDMENTS (2026-07-22 — trace design, register list,
and suspect list are all good. Two binding amendments:
A1 — OUTPUT PATH IS WRONG FOR THIS MACHINE: the 2012 rMBP has no legacy
16550 at 0x3F8 — our own kernel serial.rs probes it with a loopback self-test
and gets `None` on this Mac; bench serial is the usbdebug path, which isn't up
in the bootloader. A raw outb writer at point 2 prints into the void, and
pre-EBS `log::info!` may never reach the bench capture either. Replace BOTH
bootloader output paths: stash the 8 dwords per trace point in new
`unaos-boot-info` fields (point-1 and point-2 arrays + a validity flag) and
have igpu.rs print all three points together at probe time. Zero new output
channels, and the comparison arrives on serial as one block.
A2 — TOUCHED-FILES LIST GROWS: A1 adds `unaos-boot-info` (shared crate) to
the lane touch alongside bootloader main.rs — both flagged to the integrator.
Gate every bootloader/boot-info addition behind the same `UNAOS_IVB` feature
so default builds carry zero delta.
Full-knob land-review law applies, extended per the brief: strings-proof in
BOTH kernel.elf AND BOOTX64.EFI. Metal owed: sitting #7.)
Prior: STATUS: PROPOSED

## The Problem
As identified in sitting #6, GOP sets up a working scanout, but by the time the kernel's `igpu::init` probe runs, all pipes and planes are dead (`CONF=0`, `CNTR=0`, `SURF=0`), resulting in a persistent black panel. To localize the teardown (firmware at ExitBootServices vs. our early kernel boot), we will execute a three-point trace of the minimal scanout state.

## Three-Point Trace Implementation

We will trace the exact state of the display block at three stages of the boot pipeline:

### 1. Bootloader (Pre-ExitBootServices)
- **Location:** `unaos/crates/bootloader/src/main.rs`, immediately before the `boot::exit_boot_services` call.
- **Method:** Since UEFI services are still alive, we will locate the GPU via PCI port I/O (0xCF8/0xCFC, B:0 D:2 F:0), read BAR0, and read the MMIO registers directly from the identity-mapped physical address.
- **Output:** Output will use the existing UEFI logging system (e.g., `log::info!`). Expectation: Scanout is live.

### 2. Bootloader (Post-ExitBootServices)
- **Location:** `unaos/crates/bootloader/src/main.rs`, immediately after `boot::exit_boot_services` returns.
- **Method:** We will re-read the exact same MMIO physical addresses.
- **Output:** Since UEFI services (and thus `log::info!`) are dead, we will emit the output using a minimal raw x86 UART writer (raw `outb` instructions to COM1 at port `0x3F8`, polling the line status register at `0x3FD`).
- **Significance:** If the scanout is dead here, the UEFI firmware (GOP/ExitBootServices) killed it.

### 3. Kernel (Probe)
- **Location:** `igpu.rs` (already implemented).
- **Significance:** If the scanout was alive at Point 2 but dead here, our kernel boot chain killed it.

## Precise Register List (IVB PRM Citations)
The dump will read the following MMIO offsets relative to BAR0:
- `PIPEACONF` (0x70008) — Pipe A Configuration
- `PIPEBCONF` (0x71008) — Pipe B Configuration
- `PIPECCONF` (0x72008) — Pipe C Configuration
- `DSPACNTR` (0x70180) — Display Plane A Control
- `DSPBCNTR` (0x71180) — Display Plane B Control
- `DSPCCNTR` (0x72180) — Display Plane C Control
- `DSPASURF` (0x7019C) — Display Plane A Surface Base Address
- `DP_A` (0x64000) — DisplayPort A Control

## Suspects (If Killed by Kernel)
If the killer is isolated to the delta between Point 2 and Point 3, the prime suspects to bisect are:
1. Framebuffer console initial writes (writing pixels or clearing the screen at `0x90020000`).
2. PCI enumeration and command-register re-writes on device `00:02.0`.
3. Early page table setup / MMIO window mappings that might unmap or overwrite the GTT area.

## Standing Rules Compliance
- **Read-Only:** This pull is strictly for localization. We will perform zero state modifications or writes to the GPU; we are only reading MMIO state.
- **Touched Files:** The bootloader integration will only modify `unaos/crates/bootloader/src/main.rs`. The integrator must clear this lane before we commit.
- **Gate:** Will be gated with `UNAOS_IVB`.
