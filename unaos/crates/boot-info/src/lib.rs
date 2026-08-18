#![no_std]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

/// A simple framebuffer definition
#[derive(Debug, Clone, Copy)]
pub struct FrameBufferInfo {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub pixel_format: PixelFormat,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PixelFormat {
    Rgb,
    Bgr,
    U8,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionKind {
    Usable,
    Bootloader,
    Reserved,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub phys_start: u64,
    pub page_count: u64,
    pub kind: MemoryRegionKind,
}

/// The information passed from the UEFI bootloader to the Kernel
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
