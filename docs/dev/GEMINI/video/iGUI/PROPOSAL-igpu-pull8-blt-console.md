# Pull 8: Window Truth, Panel Census, and IVB BLT Console Acceleration

This plan folds the remaining tasks of Pull 7 (SMC power measurement and read-only panel census) into the start of Pull 8, and introduces the IVB (HD 4000) blitter to accelerate console fill and scroll.

## User Review Required

> [!IMPORTANT]
> **FB WC-Typing Guarantee**: The `WXPROBE map: at=fb … pat=1 pcd=0 pwt=0` output will remain strictly untouched. The framebuffer is already mapped into GGTT by the GOP firmware (which we probe via the `DSPASURF` / `DSPBSURF` pointers). The BLT ring will submit commands using these existing GGTT addresses rather than altering the CPU's memory mappings, preserving the WC-typed CPU side perfectly.

## Proposed Changes

### 1. `drivers/smc.rs` (Pull 7 M1)
- **[MODIFY]** `drivers/smc.rs`
  - Track and print the exact elapsed window time (`window_ms=...`).
  - Add boot-cumulative tracking (`total_ms`, `total_samples`, `total_sum`) to allow direct comparison of boots.
  - Explicitly declare the inherited assumption of the sign convention and `ac_derived` state on the 2012 rMBP (which lacks an AC key).

### 2. `drivers/gpu/igpu.rs` (Pull 7 M2 + Pull 8 Init)
- **[MODIFY]** `drivers/gpu/igpu.rs`
  - **Panel Census:** Add PCI config space probe for D-State/Memory Space Enable. Probe the correct PCH offsets for Gen7 South Display Engine (GMBUS at `0xC5100..0xC5110` and PPS at `0xC7200..0xC7210`).
  - **BLT Ring Setup:**
    - Allocate 4KB physical memory for the BLT ring buffer.
    - Write a GGTT PTE for the ring buffer.
    - Initialize the Gen7 Blitter ring (`BLT_RING_START` at `0x22038`, `BLT_RING_CTL` at `0x2203C`).
  - **Blitter Interface:** Expose functions `igpu::blitter_fill_rect` (using `XY_COLOR_BLT`) and `igpu::blitter_copy_rect` (using `XY_SRC_COPY_BLT`).

### 3. `video/fbcon.rs` & `video/framebuffer.rs` (Pull 8 Integration)
- **[MODIFY]** `video/fbcon.rs` and `video/framebuffer.rs`
  - Hook `scroll_up` and `fill_rows` to the IVB blitter if `UNAOS_IVB=1` and the ring is initialized.
  - Fall back cleanly to the existing CPU paths if the blitter is not ready.

## Verification Plan

### Automated Tests
- Gate: `./arroyo check` for both `x86_64` and `aarch64`. No QEMU tests required.
- `strings`-verify the generated `target/x86_64_esp/kernel.elf` to ensure the symbols (`blitter_fill_rect`, `XY_COLOR_BLT`, SMC log formats) are genuinely present in the artifact before declaring it shipped.

### Manual Verification (Metal s59)
1. **SMC Sign Convention Bench:** Observe the `:: PWR: ::` output with N=3 unplugged, N=3 plugged, N=3 unplugged, checking if the sum reverses properly.
2. **Panel Probe Dump:** Ensure the serial log prints the new PCH offsets and PCI reachability.
3. **Blitter Speedup:** Compare `igpu=1ms` budget and GPACE profiling before and after. Console scrolling (usually a massive CPU stall reading from WC memory) should see an enormous latency drop since `memmove` is replaced by an asynchronous ring submission.
