#![no_std]
#![no_main]

extern crate alloc;

use uefi::prelude::*;
use uefi::boot::{self, MemoryType};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::GraphicsOutput;
use unaos_boot_info::{BootInfo, FrameBufferInfo, PixelFormat, MemoryRegion, MemoryRegionKind};
use alloc::vec::Vec;

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    log::info!("UnaOS UEFI Bootloader Started");

    let fb_addr: u64;
    let fb_size: usize;
    let fb_info: FrameBufferInfo;
    
    {
        // 1. Setup Framebuffer
        let gop_handle = boot::get_handle_for_protocol::<GraphicsOutput>().unwrap();
        let mut gop = boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle).unwrap();
        
        let mode_info = gop.current_mode_info();
        let mut fb = gop.frame_buffer();
        
        let pixel_format = match mode_info.pixel_format() {
            uefi::proto::console::gop::PixelFormat::Rgb => PixelFormat::Rgb,
            uefi::proto::console::gop::PixelFormat::Bgr => PixelFormat::Bgr,
            _ => PixelFormat::Unknown,
        };

        fb_info = FrameBufferInfo {
            width: mode_info.resolution().0,
            height: mode_info.resolution().1,
            stride: mode_info.stride(),
            bytes_per_pixel: 4, // Assume 32-bit for now
            pixel_format,
        };
        
        fb_addr = fb.as_mut_ptr() as u64;
        fb_size = fb.size();
    }

    // 2. Construct BootInfo
    let boot_info = alloc::boxed::Box::new(BootInfo {
        framebuffer_addr: fb_addr,
        framebuffer_size: fb_size,
        framebuffer_info: fb_info,
        physical_memory_offset: 0,
        memory_regions_addr: 0,
        memory_regions_len: 0,
    });
    let boot_info_static = alloc::boxed::Box::leak(boot_info);

    // 3. Exit Boot Services
    let memory_map = unsafe { boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };
    
    let mut regions = Vec::new();
    for desc in memory_map.entries() {
        let kind = match desc.ty {
            MemoryType::CONVENTIONAL => MemoryRegionKind::Usable,
            MemoryType::LOADER_DATA | MemoryType::LOADER_CODE => MemoryRegionKind::Bootloader,
            _ => MemoryRegionKind::Reserved,
        };
        regions.push(MemoryRegion {
            phys_start: desc.phys_start,
            page_count: desc.page_count,
            kind,
        });
    }

    let regions_slice = regions.leak();
    boot_info_static.memory_regions_addr = regions_slice.as_ptr() as u64;
    boot_info_static.memory_regions_len = regions_slice.len();

    // 4. Load the Kernel ELF
    // TODO: Load kernel.elf from the EFI file system and parse it with xmas-elf.
    // For now, we will halt since we haven't implemented the ELF loader yet.
    loop {
        core::hint::spin_loop();
    }
}
