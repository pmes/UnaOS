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

## ABI-LOCK — the layout is frozen and asserted

The loader and the kernel are separately compiled binaries, for *different targets*
(`{x86_64,aarch64}-unknown-uefi` vs the `*-unaos.json` kernel targets), and the hand-off
is a raw `transmute`d function pointer taking `&'static mut BootInfo`. Nothing checks at
run time that the two agree, so a layout disagreement is a wild pointer at the earliest
instant of boot — before serial exists on the Orin.

Every type here therefore carries an explicit `repr` (`#[repr(C)]` on the structs,
`#[repr(u8)]` on the fieldless enums), and the size, alignment and **every field offset**
are pinned by `const` assertions at the bottom of `src/lib.rs`. Because this crate is a
dependency of both sides, a change that moves a field fails the **build**, on whichever
side introduces it, rather than the boot.

Measured layout (identical on all four targets, rustc 1.98.0-nightly): `BootInfo` is 256
bytes / align 8 on a default build and 456 with `unaos_ivb`; `FrameBufferInfo` 40/8;
`MemoryRegion` 24/8 — that 24 is the array stride the kernel walks through
`memory_regions_addr`. **Add new fields at the end**; that is the one change that moves
no existing offset.

`unaos_ivb` is a cross-crate ABI feature: it appends fields to `BootInfo`, and
`builder/src/main.rs` and `arroyo` must arm it for the loader and the kernel together.
Under the previous `repr(Rust)` a one-sided arm scrambled *every* shared field
(`framebuffer_addr` 40 → 128, `edid_block` 104 → 0); under `repr(C)` the common 256-byte
prefix is identical in both feature legs, which is asserted here too.
