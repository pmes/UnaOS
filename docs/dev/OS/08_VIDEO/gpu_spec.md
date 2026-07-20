# Cleanroom GPU Specification: 2012 Retina MacBook Pro

This document specifies the hardware interface for the dual-GPU setup in the 2012 Retina MacBook Pro (board-ID: `Mac-6F01561E16C75D06`). It is intended to serve as the sole reference for implementing native driver support in UnaOS, ensuring a cleanroom design without referencing proprietary driver code.

Data in this document is sourced from:
1. Public hardware documentation (Intel PRMs, NVIDIA open-gpu-doc).
2. Open-source driver code (Nouveau, i915).
3. Metadata extracted from macOS 10.15 driver `Info.plist` files (device IDs, power thresholds).

---

## 1. Device Identification

### NVIDIA GeForce GT 650M (Kepler / GK107)
- **PCI Class**: `0x03` (Display Controller), Subclass `0x00` (VGA Compatible) or `0x80` (Other/3D)
- **Vendor ID**: `0x10DE` (NVIDIA)
- **Device ID**: `0x0FD5`
- **Architecture**: Kepler (GK1xx)

### Intel HD Graphics 4000 (Ivy Bridge / Gen7)
- **PCI Class**: `0x03` (Display Controller), Subclass `0x00` (VGA Compatible)
- **Vendor ID**: `0x8086` (Intel)
- **Device ID**: `0x0166`
- **Architecture**: Ivy Bridge (Gen7)

---

## 2. NVIDIA Kepler (GK107) Register Map

NVIDIA uses a single large MMIO region (BAR0) for all registers, typically 16MB or 32MB in size. Registers are accessed via 32-bit read/writes.

### 2.1 Master Control (PMC) - Base `0x000000`
- `0x000000` **NV_PMC_BOOT_0**: Chip identification and stepping.
  - Bits [27:20]: Chipset ID (GK107 = `0xE7`)
  - Bits [19:16]: Major revision
  - Bits [15:0]: Minor revision
- `0x000004` **NV_PMC_BOOT_1**: Additional revision info.
- `0x000200` **NV_PMC_ENABLE**: Master engine enable mask. Indicates if the GPU is initialized/POST'd.
- `0x000100` **NV_PMC_INTR_0**: Global interrupt status.
- `0x000140` **NV_PMC_INTR_EN**: Global interrupt enable. Write `0` to disable all interrupts during init.

### 2.2 Bus Control (PBUS) - Base `0x001000`
- `0x001800` **NV_PBUS_PCI_NV_0**: Mirror of PCI config space `0x00` (Vendor/Device ID).
- `0x001804` **NV_PBUS_PCI_NV_1**: Mirror of PCI config space `0x04` (Command/Status).

### 2.3 Display Engine (PDISPLAY) - Base `0x610000`
*(To be detailed in Phase 2 for Modesetting)*
- Kepler uses a sophisticated display engine supporting multiple CRTCs (heads) and output resources (SORs).
- Display heads control timings, while SORs control the physical encoders (eDP, HDMI).

---

## 3. Power Management Profiles (from AGPM)

AppleGraphicsPowerManagement dictates specific heuristics for the GT 650M (`0x0fd5`) on this board:

- **Power States**: Typically 4 states (0-3), with 0 being highest performance and 3 being deepest sleep/idle.
- **Thresholds**: State transitions are based on core/memory clock thresholds and utilization percentages.
- **Heuristic ID**: `-1` (custom Apple heuristic logic).

---

## 4. Hardware Performance Counters

GPU profiling uses specific performance counters to measure utilization and identify bottlenecks.

### Key Metrics:
- **SM Utilization (%)**: Percentage of time Streaming Multiprocessors are active.
- **TEX Utilization (%)**: Texture unit utilization. High values (>25% stalls, >128 bytes/thread) indicate texture-bound workloads.
- **ROP Utilization (%)**: Raster Operations Pipeline activity (ZROP, CROP).
- **Cache Hit Rates**: L1 and L2 cache efficiency.

---

## 5. UnaOS Driver Architecture Mapping

Based on the capabilities and requirements:

1. **Detection**: `PciScanner` matches Vendor/Device IDs during boot.
2. **Initialization**: The driver maps BAR0 (MMIO), verifies chip ID (`NV_PMC_BOOT_0`), checks POST status, and disables interrupts (`NV_PMC_INTR_EN = 0`).
3. **Display Takeover**: The driver queries the current scanout address programmed by the GOP, allocates its own `FrameBuffer`, programs the display head to use the new buffer, and enables the display.
4. **Integration**: The new `FrameBuffer` is passed to the `video` subsystem, replacing the GOP's buffer.
