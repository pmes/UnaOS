#![no_std]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! # ABI-LOCK — the loader↔kernel hand-off is a **frozen binary layout**
//!
//! Everything in this crate is written by one binary (the UEFI bootloader, built for
//! `x86_64-unknown-uefi` / `aarch64-unknown-uefi`) and read by a *different, separately compiled*
//! binary (the kernel, built for `x86_64-unaos.json` / `aarch64-unaos.json`). Nothing checks the
//! two agree at run time: the kernel is entered through a raw `transmute`d function pointer with
//! `&'static mut BootInfo` as its only argument, so a layout disagreement is not a type error —
//! it is the kernel reading a framebuffer pointer out of the bytes the loader wrote as a length,
//! at the earliest instant of boot, before serial exists. On the Jetson Orin that lands before the
//! DARKWIN UARTC latch is armed, i.e. a silent hang with no output at all.
//!
//! Therefore **every type in this file carries an explicit `repr`**, and the layout is pinned by
//! the `const` assertions at the bottom of the file:
//!
//! * structs are `#[repr(C)]` — fields sit at their declared offsets, in declaration order. Under
//!   the default `repr(Rust)` the compiler is free to reorder, and it *does*: measured on
//!   2026-08-22 (rustc 1.98.0-nightly), enabling the `unaos_ivb` feature — which only *appends*
//!   fields at the end of `BootInfo` — moved `framebuffer_addr` from offset 40 to 128 and
//!   `edid_block` from 104 to 0. That is why `unaos_ivb` must be armed on both sides
//!   (`builder/src/main.rs`, `arroyo`) and, with `repr(C)`, why a one-sided arm no longer
//!   scrambles the fields both sides already share.
//! * fieldless enums are `#[repr(u8)]`, not `#[repr(C)]`: a `repr(C)` enum takes the target C
//!   ABI's `int` width, which is a property of the target rather than of this contract. `u8` is
//!   the width `repr(Rust)` happens to pick today, so pinning it here changed no offsets.
//!
//! `MemoryRegion` is **not** reached by value — the loader writes an array of them and passes only
//! `memory_regions_addr`/`memory_regions_len`, which the kernel turns back into a slice with
//! `slice::from_raw_parts` (`arch/{x86_64,aarch64}/memory.rs`, `aarch64/{boot_virt,mmu_tegra}.rs`).
//! A pointer hides a layout disagreement completely, so it is pinned exactly like the rest.

/// A simple framebuffer definition
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct FrameBufferInfo {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub pixel_format: PixelFormat,
}

/// ABI-LOCK: `repr(u8)` — see the crate-level note. Never carried as `Option<PixelFormat>`
/// anywhere, so no niche optimisation is load-bearing and pinning the width costs nothing.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
pub enum PixelFormat {
    Rgb,
    Bgr,
    U8,
    Unknown,
}

/// ABI-LOCK: `repr(u8)` — see the crate-level note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MemoryRegionKind {
    Usable,
    Bootloader,
    Reserved,
}

/// ABI-LOCK: written by the loader as an ARRAY and read back through a raw pointer
/// (`BootInfo::memory_regions_addr`), so nothing but this `repr(C)` keeps the two sides agreeing.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct MemoryRegion {
    pub phys_start: u64,
    pub page_count: u64,
    pub kind: MemoryRegionKind,
}

/// The information passed from the UEFI bootloader to the Kernel.
///
/// ABI-LOCK: `#[repr(C)]`, layout pinned by the `const` assertions at the bottom of this file.
/// Add new fields **at the end** — that keeps every existing offset, which is the whole point of
/// the `repr(C)`; then add the new offset to the assertion block and bump `SIZE`.
#[repr(C)]
pub struct BootInfo {
    /// The physical address where the framebuffer is mapped
    pub framebuffer_addr: u64,
    pub framebuffer_size: usize,
    pub framebuffer_info: FrameBufferInfo,

    /// Offset where physical memory is mapped in the virtual address space
    pub physical_memory_offset: u64,

    pub dtb_addr: u64,
    pub dtb_size: usize,

    /// Physical address of the ACPI RSDP (Root System Description Pointer), found by the
    /// bootloader in the UEFI configuration table (x86_64). 0 if not present. The kernel walks
    /// it (RSDP -> XSDT -> MADT) to discover the CPU topology for SMP bring-up. On aarch64 the
    /// equivalent discovery comes from the DTB, so this stays 0 there.
    pub rsdp_addr: u64,

    /// Pointer to the array of memory regions
    pub memory_regions_addr: u64,
    pub memory_regions_len: usize,

    /// Display mode-selection diagnostics, filled by the bootloader (for the `bootlog` readout):
    /// the monitor's EDID-native resolution it parsed (0 if no EDID was readable), which EDID
    /// protocol provided it, and which mode-selection branch ran.
    pub edid_native_width: u32,
    pub edid_native_height: u32,
    /// Which protocol the carried EDID came from: 0 = no EDID read,
    /// 1 = EFI_EDID_ACTIVE_PROTOCOL, 2 = EFI_EDID_DISCOVERED_PROTOCOL.
    pub edid_source: u32,
    /// 0 = kept firmware current mode, 1 = set the EDID-native mode, 2 = set a fallback linear
    /// mode (current was BltOnly), 3 = headless (no linear framebuffer available).
    pub mode_action: u32,

    /// EDID-CARRY: the panel's raw EDID **base block** — the first 128 bytes, copied verbatim by the
    /// bootloader out of the UEFI EDID protocol while boot services were still live.
    ///
    /// Until this field existed the bootloader read the whole block, kept only the native
    /// width/height (`edid_native_width`/`edid_native_height`) and dropped the bytes. Width and
    /// height are not enough to program a display pipe: the pixel clock, the horizontal/vertical
    /// blanking and sync numbers, and the panel's own feature/colour bits all live in the block and
    /// were being discarded. This field is the transport for them.
    ///
    /// All-zero when no EDID was readable — check [`BootInfo::edid_block_valid`] first, and prefer
    /// the kernel-side accessor (`video::edid_block()`), which also enforces header + checksum.
    /// Only the BASE block is carried: byte 126 of it is the EDID extension-block count and any
    /// extension blocks the firmware reported (`edid_total_len > 128`) are NOT copied.
    pub edid_block: [u8; 128],
    /// True when `edid_block` holds 128 bytes actually copied from firmware. False = no EDID
    /// protocol on the GOP handle, a null firmware buffer, a firmware-reported size below one base
    /// block, or a boot path that never runs the UEFI bootloader (aarch64 bare-metal). The array is
    /// then all zeroes and must not be parsed. **This flag says the bytes were copied, not that
    /// they are a valid EDID** — the header and checksum are checked kernel-side.
    pub edid_block_valid: bool,
    /// The firmware-reported size of the WHOLE EDID in bytes (0 = none). Greater than 128 means
    /// extension blocks exist and were dropped, which the kernel's witness line reports.
    pub edid_total_len: u16,

    /// INSTALL-SELF: the FAT `BS_VolID` (volume serial) of the volume this kernel was loaded FROM —
    /// read by the bootloader off LBA 0 of its own loaded-image device handle, i.e. the very ESP that
    /// carried `kernel.elf`. This is the only thing in `BootInfo` that names the boot *storage*, and
    /// the installer's boot-device guard is built on it: a candidate target disk carrying a FAT volume
    /// with this serial is the device we booted from (or a byte clone of it) and is never offered and
    /// never erased.
    ///
    /// **0 is the absent sentinel** — no readable FAT volume on the boot device, a non-FAT boot path,
    /// a formatter that left `BS_VolID` unstamped, or an aarch64 boot (its `build_boot_info` fills 0).
    /// The guard DISARMS on 0 with a witness line rather than guessing: an installer that cannot
    /// identify its boot device must still be usable.
    pub boot_volume_serial: u32,

    #[cfg(feature = "unaos_ivb")]
    pub igpu_trace_0: [u32; 11],
    #[cfg(feature = "unaos_ivb")]
    pub igpu_trace_1: [u32; 11],
    #[cfg(feature = "unaos_ivb")]
    pub igpu_trace_2: [u32; 11],
    #[cfg(feature = "unaos_ivb")]
    pub gmux_trace_0: [u32; 7],
    #[cfg(feature = "unaos_ivb")]
    pub igpu_trace_valid: bool,
    #[cfg(feature = "unaos_ivb")]
    pub kdisp_trace_0: [u32; 7],
    #[cfg(feature = "unaos_ivb")]
    pub kdisp_trace_valid: bool,
}

// ============================================================================================
// ABI-LOCK — the loader↔kernel layout, pinned.
// ============================================================================================
//
// These assertions are compiled by BOTH sides of the hand-off (this crate is a dependency of
// `crates/bootloader` and of `crates/kernel`, and is built separately for each of the four
// targets in play: `x86_64-unknown-uefi` / `aarch64-unknown-uefi` for the loader,
// `x86_64-unaos.json` / `aarch64-unaos.json` for the kernel). Any change that moves a field
// therefore fails the BUILD — on whichever side it is introduced — instead of failing the BOOT,
// which on the Orin means a silent hang with no serial output at all.
//
// Every number below was MEASURED, not derived: `offset_of!`/`size_of!` compiled for all four
// targets, both `unaos_ivb` legs (2026-08-22, rustc 1.98.0-nightly). All four targets agree.
//
// WHEN ONE OF THESE FIRES, the fix is never to edit the number to match. It is:
//   * added a field?  Put it at the END of `BootInfo` (or of `FrameBufferInfo`/`MemoryRegion`),
//     which leaves every existing offset alone, then ADD its offset assertion here and bump the
//     SIZE constant. Appending is the only free change.
//   * changed a field's type, order, or a `repr`?  You have altered the boot protocol. Both
//     binaries must be rebuilt from the same tree — and any prebuilt `bootloader.efi` sitting on
//     existing boot media is now incompatible with a fresh `kernel.elf`. Re-pack the whole ESP.
//   * moved a field to "tidy up"?  Don't. Declaration order IS the wire order under `repr(C)`.

const _: () = assert!(
    core::mem::align_of::<BootInfo>() == 8,
    "ABI-LOCK: BootInfo alignment changed — the loader↔kernel hand-off layout moved. See the \
     ABI-LOCK note at the top of crates/boot-info/src/lib.rs before touching anything."
);

// --- BootInfo: the fields present on EVERY build. Under `repr(C)` these offsets are identical
// --- with and without `unaos_ivb`, which is what makes a one-sided feature arm survivable.
const _: () = {
    use core::mem::offset_of;
    assert!(offset_of!(BootInfo, framebuffer_addr) == 0, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, framebuffer_size) == 8, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, framebuffer_info) == 16, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, physical_memory_offset) == 56, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, dtb_addr) == 64, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, dtb_size) == 72, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, rsdp_addr) == 80, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, memory_regions_addr) == 88, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, memory_regions_len) == 96, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, edid_native_width) == 104, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, edid_native_height) == 108, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, edid_source) == 112, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, mode_action) == 116, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, edid_block) == 120, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, edid_block_valid) == 248, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, edid_total_len) == 250, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
    assert!(offset_of!(BootInfo, boot_volume_serial) == 252, "ABI-LOCK: BootInfo field offset moved — see the ABI-LOCK note at the top of this file. Append new fields at the END; never reorder.");
};

/// The size of the common prefix — everything above the `unaos_ivb` fields. `boot_volume_serial`
/// is the last common field and it ends at 256, so this is also `size_of::<BootInfo>()` on a
/// default build.
pub const BOOT_INFO_COMMON_LEN: usize = 256;

#[cfg(not(feature = "unaos_ivb"))]
const _: () = assert!(
    core::mem::size_of::<BootInfo>() == BOOT_INFO_COMMON_LEN,
    "ABI-LOCK: BootInfo size changed on a default (no unaos_ivb) build. If you appended a field, \
     update BOOT_INFO_COMMON_LEN and add its offset assertion. See the ABI-LOCK note at the top \
     of crates/boot-info/src/lib.rs."
);

// --- BootInfo: the `unaos_ivb` tail. This feature is a CROSS-CRATE ABI knob — `builder/src/main.rs`
// --- and `arroyo` must arm it for the loader and the kernel together. `repr(C)` is what demotes a
// --- one-sided arm from "every shared field is scrambled" to "the tail is absent"; these
// --- assertions keep it that way.
#[cfg(feature = "unaos_ivb")]
const _: () = {
    use core::mem::offset_of;
    // The tail starts exactly where the common prefix ends — this is the property that makes a
    // one-sided `unaos_ivb` arm non-catastrophic. If it ever fails, the two feature legs have
    // diverged in their shared region and the loader and kernel no longer see the same fields.
    assert!(offset_of!(BootInfo, igpu_trace_0) == BOOT_INFO_COMMON_LEN, "ABI-LOCK: the unaos_ivb tail no longer starts at the end of the common prefix — the two feature legs have diverged. See the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(BootInfo, igpu_trace_1) == 300, "ABI-LOCK: BootInfo unaos_ivb field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(BootInfo, igpu_trace_2) == 344, "ABI-LOCK: BootInfo unaos_ivb field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(BootInfo, gmux_trace_0) == 388, "ABI-LOCK: BootInfo unaos_ivb field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(BootInfo, igpu_trace_valid) == 416, "ABI-LOCK: BootInfo unaos_ivb field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(BootInfo, kdisp_trace_0) == 420, "ABI-LOCK: BootInfo unaos_ivb field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(BootInfo, kdisp_trace_valid) == 448, "ABI-LOCK: BootInfo unaos_ivb field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(core::mem::size_of::<BootInfo>() == 456, "ABI-LOCK: BootInfo size changed on an unaos_ivb build — see the ABI-LOCK note at the top of this file.");
};

// --- FrameBufferInfo: carried BY VALUE inside BootInfo, so its layout is part of BootInfo's.
const _: () = {
    use core::mem::offset_of;
    assert!(core::mem::size_of::<FrameBufferInfo>() == 40, "ABI-LOCK: FrameBufferInfo size changed — it sits by value inside BootInfo, so this moves every field after it. See the ABI-LOCK note at the top of this file.");
    assert!(core::mem::align_of::<FrameBufferInfo>() == 8, "ABI-LOCK: FrameBufferInfo alignment changed — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(FrameBufferInfo, width) == 0, "ABI-LOCK: FrameBufferInfo field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(FrameBufferInfo, height) == 8, "ABI-LOCK: FrameBufferInfo field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(FrameBufferInfo, stride) == 16, "ABI-LOCK: FrameBufferInfo field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(FrameBufferInfo, bytes_per_pixel) == 24, "ABI-LOCK: FrameBufferInfo field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(FrameBufferInfo, pixel_format) == 32, "ABI-LOCK: FrameBufferInfo field offset moved — see the ABI-LOCK note at the top of this file.");
};

// --- MemoryRegion: reached only through `BootInfo::memory_regions_addr`, as a raw array the kernel
// --- rebuilds with `slice::from_raw_parts`. A pointer hides a layout disagreement completely — a
// --- wrong stride here silently mis-parses the entire physical memory map.
const _: () = {
    use core::mem::offset_of;
    assert!(core::mem::size_of::<MemoryRegion>() == 24, "ABI-LOCK: MemoryRegion size changed — this is the ARRAY STRIDE the kernel walks via slice::from_raw_parts(memory_regions_addr, ...). See the ABI-LOCK note at the top of this file.");
    assert!(core::mem::align_of::<MemoryRegion>() == 8, "ABI-LOCK: MemoryRegion alignment changed — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(MemoryRegion, phys_start) == 0, "ABI-LOCK: MemoryRegion field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(MemoryRegion, page_count) == 8, "ABI-LOCK: MemoryRegion field offset moved — see the ABI-LOCK note at the top of this file.");
    assert!(offset_of!(MemoryRegion, kind) == 16, "ABI-LOCK: MemoryRegion field offset moved — see the ABI-LOCK note at the top of this file.");
};

// --- The fieldless enums. `repr(u8)` pins the discriminant width; these assertions are what stop a
// --- future `#[repr(C)]` (4 bytes on both targets) or an added 257th variant from silently
// --- resizing the structs that embed them.
const _: () = {
    assert!(core::mem::size_of::<PixelFormat>() == 1, "ABI-LOCK: PixelFormat is no longer one byte — it is embedded in FrameBufferInfo, which is embedded in BootInfo. See the ABI-LOCK note at the top of this file.");
    assert!(core::mem::align_of::<PixelFormat>() == 1, "ABI-LOCK: PixelFormat alignment changed — see the ABI-LOCK note at the top of this file.");
    assert!(core::mem::size_of::<MemoryRegionKind>() == 1, "ABI-LOCK: MemoryRegionKind is no longer one byte — it is embedded in MemoryRegion, whose size is the array stride the kernel walks. See the ABI-LOCK note at the top of this file.");
    assert!(core::mem::align_of::<MemoryRegionKind>() == 1, "ABI-LOCK: MemoryRegionKind alignment changed — see the ABI-LOCK note at the top of this file.");
};
