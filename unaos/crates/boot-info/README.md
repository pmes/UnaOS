# boot-info

The boot protocol shared between the bootloader and the kernel. A tiny,
dependency-free `#![no_std]` library defining the hand-off structure.

The bootloader fills in a `BootInfo` and passes a reference to the kernel's
`_start`. It carries:

- **Framebuffer** — `framebuffer_addr` / `framebuffer_size` plus `FrameBufferInfo`
  (width, height, stride, bytes-per-pixel, `PixelFormat` = `Rgb` / `Bgr` / `U8`),
  the result of the bootloader's GOP/EDID mode selection.
- **Memory map** — an array of `MemoryRegion { phys_start, page_count, kind }`
  (`MemoryRegionKind` = `Usable` / `Bootloader` / `Reserved`) built from the UEFI
  memory map, used by the kernel's frame allocator.
- **Platform tables** — the ACPI RSDP address (x86_64, for MADT/SMP discovery) and
  the device-tree blob address/size (aarch64).
- EDID/mode-selection diagnostics consumed by the boot-log build.

This crate has no dependencies and no logic — it is purely the data contract.
