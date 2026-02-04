# Bootloader Specification & Handoff

## 1. The UEFI Contract
unaOS requires a UEFI 2.x compliant environment.
* **Entry Point:** The kernel expects to be loaded as an EFI executable (`BOOTX64.EFI`).
* **FrameBuffer:** The bootloader MUST set up the Graphics Output Protocol (GOP) before handing off control. The kernel does not perform mode-setting during early boot (to prevent flickering).

## 2. OpenCore & Mac Hardware Compatibility
To support Apple hardware (specifically MacBookPro10,1 and newer) and patched environments (OpenCore):
* **ACPI Tables:** The kernel will respect ACPI patches applied by OpenCore. We do not re-enumerate the raw hardware; we trust the tables provided by the bootloader.
* **System Integrity:** Secure Boot signatures, if present, are verified against the `unaOS_Root_CA` key.

## 3. Memory Map Handoff
Upon exit of Boot Services:
1.  The bootloader provides a memory map of type `EfiLoaderData`.
2.  The kernel immediately claims all "Conventional Memory" for the `PhysMem` manager.
3.  The kernel engages the "Virtual Memory Air-Gap" (see `docs/02_KERNEL_CORE/memory_model.md`).
