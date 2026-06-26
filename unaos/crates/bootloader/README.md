# bootloader

The UEFI bootloader. It runs as a UEFI application, loads the kernel, and hands
control over with a populated [`BootInfo`](../boot-info).

## Responsibilities
1. **Graphics** — open the UEFI GOP, enumerate modes, and select the display mode
   (EDID-native when an EDID is readable, otherwise the firmware-current mode);
   record the chosen framebuffer geometry and pixel format.
2. **Load the kernel** — read `kernel.elf` from the EFI System Partition, parse it
   (`xmas-elf`), load its `PT_LOAD` segments at an allocated physical address, and
   apply ELF relocations (`R_X86_64_RELATIVE` / `R_AARCH64_RELATIVE`) for the PIE
   kernel.
3. **Platform tables** — locate the ACPI RSDP (x86_64) and, on aarch64, the
   device-tree blob from the UEFI configuration table.
4. **Memory map** — exit boot services and build the `MemoryRegion` array.
5. **Hand off** — jump to the kernel entry point (`extern "sysv64"` on x86_64,
   `extern "C"` on aarch64), passing the `BootInfo` reference.

## Targets
Built for `x86_64-unknown-uefi` and the aarch64 UEFI target. See
[`docs/dev/OS/01_BOOT_HAL/`](../../../docs/dev/OS/01_BOOT_HAL) for the boot/HAL
specifications.
