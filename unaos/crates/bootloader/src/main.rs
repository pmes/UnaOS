#![no_std]
#![no_main]

extern crate alloc;

use uefi::prelude::*;
use uefi::boot::{self, MemoryType};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::GraphicsOutput;
use unaos_boot_info::{BootInfo, FrameBufferInfo, PixelFormat, MemoryRegion, MemoryRegionKind};
use alloc::vec::Vec;
use uefi::proto::media::file::{File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    log::info!("UnaOS UEFI Bootloader Started");

    let fb_addr: u64;
    let fb_size: usize;
    let fb_info: FrameBufferInfo;
    
    {
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
            bytes_per_pixel: 4,
            pixel_format,
        };
        
        fb_addr = fb.as_mut_ptr() as u64;
        fb_size = fb.size();
    }

    let sfs_handle = boot::get_handle_for_protocol::<SimpleFileSystem>().unwrap();
    let mut sfs = boot::open_protocol_exclusive::<SimpleFileSystem>(sfs_handle).unwrap();
    let mut root = sfs.open_volume().unwrap();

    let mut kernel_file = root.open(cstr16!("kernel.elf"), FileMode::Read, FileAttribute::empty())
        .unwrap().into_regular_file().unwrap();

    let mut file_info_buf = [0u8; 128];
    let file_info = kernel_file.get_info::<uefi::proto::media::file::FileInfo>(&mut file_info_buf).unwrap();
    let kernel_size = file_info.file_size() as usize;

    let kernel_buffer_ptr = boot::allocate_pages(
        boot::AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        (kernel_size + 4095) / 4096,
    ).unwrap();

    let kernel_buffer = unsafe { core::slice::from_raw_parts_mut(kernel_buffer_ptr.as_ptr(), kernel_size) };
    kernel_file.read(kernel_buffer).unwrap();

    let elf = xmas_elf::ElfFile::new(kernel_buffer).unwrap();

    let mut base_addr = 0;
    
    let mut max_vaddr = 0;
    let mut min_vaddr = u64::MAX;
    for ph in elf.program_iter() {
        if ph.get_type() == Ok(xmas_elf::program::Type::Load) {
            if ph.virtual_addr() < min_vaddr { min_vaddr = ph.virtual_addr(); }
            if ph.virtual_addr() + ph.mem_size() > max_vaddr { max_vaddr = ph.virtual_addr() + ph.mem_size(); }
        }
    }
    
    let kernel_pages = ((max_vaddr - min_vaddr + 4095) / 4096) as usize;
    let load_base = boot::allocate_pages(
        boot::AllocateType::AnyPages,
        MemoryType::LOADER_CODE,
        kernel_pages,
    ).unwrap().as_ptr() as u64;

    for ph in elf.program_iter() {
        if ph.get_type() == Ok(xmas_elf::program::Type::Load) {
            let offset = ph.virtual_addr() - min_vaddr;
            let dest_ptr = load_base + offset;
            
            let dest_slice = unsafe { core::slice::from_raw_parts_mut(dest_ptr as *mut u8, ph.mem_size() as usize) };
            let src_slice = &kernel_buffer[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize];
            dest_slice[..ph.file_size() as usize].copy_from_slice(src_slice);
            dest_slice[ph.file_size() as usize..].fill(0);
        }
    }

    let entry_point = elf.header.pt2.entry_point() - min_vaddr + load_base;

    let boot_info = alloc::boxed::Box::new(BootInfo {
        framebuffer_addr: fb_addr,
        framebuffer_size: fb_size,
        framebuffer_info: fb_info,
        physical_memory_offset: 0,
        memory_regions_addr: 0,
        memory_regions_len: 0,
    });
    let boot_info_static = alloc::boxed::Box::leak(boot_info);

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

    let kernel_entry: extern "C" fn(&'static mut BootInfo) -> ! = unsafe {
        core::mem::transmute(entry_point as usize)
    };

    kernel_entry(boot_info_static);
}
