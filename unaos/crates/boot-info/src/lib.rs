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

    /// Pointer to the array of memory regions
    pub memory_regions_addr: u64,
    pub memory_regions_len: usize,
}
