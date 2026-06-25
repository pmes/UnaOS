#![no_std]
#![no_main]

extern crate alloc;

use uefi::prelude::*;
use uefi::boot::{self, MemoryType};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::GraphicsOutput;
use unaos_boot_info::{BootInfo, FrameBufferInfo, PixelFormat, MemoryRegion, MemoryRegionKind};

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
        let gop_handle = match boot::get_handle_for_protocol::<GraphicsOutput>() {
            Ok(handle) => handle,
            Err(e) => {
                log::error!("Failed to get GraphicsOutput handle: {:?}", e);
                return Status::UNSUPPORTED;
            }
        };
        let mut gop = match boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle) {
            Ok(gop) => gop,
            Err(e) => {
                log::error!("Failed to open GraphicsOutput protocol: {:?}", e);
                return Status::UNSUPPORTED;
            }
        };
        
        // The display resolution is auto-detected from the firmware's GOP, not hard-coded — and the
        // firmware's *default* mode is usually not the panel's native/maximum. So enumerate the
        // modes and select the largest *linear-framebuffer* (Rgb/Bgr) one we can drive. The full
        // list is logged because on real hardware (no serial) it's the only way to see what the
        // panel offers.
        //
        // The pick is capped to what the kernel can back-buffer: the double-buffer back buffer is
        // stride*height*4 bytes and must fit the kernel heap alongside its other allocations.
        // MAX_BACKBUF_BYTES is tied to crates/kernel allocator::HEAP_SIZE (32 MiB) — keep in sync;
        // 24 MiB leaves headroom for the xHCI/console/block allocations.
        const MAX_BACKBUF_BYTES: usize = 24 * 1024 * 1024;

        let cur = gop.current_mode_info().resolution();
        log::info!("GOP: {} modes (firmware default {}x{}):", gop.modes().len(), cur.0, cur.1);
        for mode in gop.modes() {
            let mi = mode.info();
            let (w, h) = mi.resolution();
            log::info!("  GOP mode: {}x{} stride={} fmt={:?}", w, h, mi.stride(), mi.pixel_format());
        }

        // Linear-framebuffer modes within the back-buffer budget, largest area first. (BltOnly /
        // Bitmask modes have no usable linear framebuffer, so they're excluded.)
        let mut candidates: alloc::vec::Vec<uefi::proto::console::gop::Mode> = gop
            .modes()
            .filter(|m| {
                matches!(
                    m.info().pixel_format(),
                    uefi::proto::console::gop::PixelFormat::Rgb
                        | uefi::proto::console::gop::PixelFormat::Bgr
                )
            })
            .filter(|m| {
                let mi = m.info();
                mi.stride() * mi.resolution().1 * 4 <= MAX_BACKBUF_BYTES
            })
            .collect();
        candidates.sort_unstable_by_key(|m| {
            let (w, h) = m.info().resolution();
            core::cmp::Reverse(w * h)
        });

        // Set the largest mode the firmware accepts (fall through to smaller ones if set_mode
        // fails, e.g. not enough video memory). If none work, the current mode is left untouched.
        for mode in &candidates {
            let (w, h) = mode.info().resolution();
            match gop.set_mode(mode) {
                Ok(()) => {
                    log::info!("GOP: selected {}x{}", w, h);
                    break;
                }
                Err(e) => log::warn!("GOP: set_mode {}x{} failed ({:?}); trying smaller", w, h, e),
            }
        }

        // Read the active mode + framebuffer *after* set_mode (it invalidates the old framebuffer).
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

    let sfs_handle = match boot::get_handle_for_protocol::<SimpleFileSystem>() {
        Ok(handle) => handle,
        Err(e) => {
            log::error!("Failed to get SimpleFileSystem handle: {:?}", e);
            return Status::LOAD_ERROR;
        }
    };
    let mut sfs = match boot::open_protocol_exclusive::<SimpleFileSystem>(sfs_handle) {
        Ok(sfs) => sfs,
        Err(e) => {
            log::error!("Failed to open SimpleFileSystem protocol: {:?}", e);
            return Status::LOAD_ERROR;
        }
    };
    let mut root = match sfs.open_volume() {
        Ok(root) => root,
        Err(e) => {
            log::error!("Failed to open root volume: {:?}", e);
            return Status::LOAD_ERROR;
        }
    };

    let mut kernel_file = match root.open(cstr16!("kernel.elf"), FileMode::Read, FileAttribute::empty()) {
        Ok(file) => match file.into_regular_file() {
            Some(f) => f,
            None => {
                log::error!("kernel.elf is not a regular file");
                return Status::LOAD_ERROR;
            }
        },
        Err(e) => {
            log::error!("Failed to open kernel.elf: {:?}", e);
            return Status::NOT_FOUND;
        }
    };

    let mut file_info_buf = [0u8; 128];
    let file_info = match kernel_file.get_info::<uefi::proto::media::file::FileInfo>(&mut file_info_buf) {
        Ok(info) => info,
        Err(e) => {
            log::error!("Failed to get kernel.elf file info: {:?}", e);
            return Status::LOAD_ERROR;
        }
    };
    let kernel_size = file_info.file_size() as usize;

    let kernel_buffer_ptr = match boot::allocate_pages(
        boot::AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        (kernel_size + 4095) / 4096,
    ) {
        Ok(ptr) => ptr,
        Err(e) => {
            log::error!("Failed to allocate pages for kernel file: {:?}", e);
            return Status::OUT_OF_RESOURCES;
        }
    };

    let kernel_buffer = unsafe { core::slice::from_raw_parts_mut(kernel_buffer_ptr.as_ptr(), kernel_size) };
    if let Err(e) = kernel_file.read(kernel_buffer) {
        log::error!("Failed to read kernel.elf: {:?}", e);
        return Status::LOAD_ERROR;
    }

    let elf = match xmas_elf::ElfFile::new(kernel_buffer) {
        Ok(elf) => elf,
        Err(e) => {
            log::error!("Failed to parse ELF: {:?}", e);
            return Status::LOAD_ERROR;
        }
    };

    // ELF Validation
    if elf.header.pt1.magic != [0x7f, b'E', b'L', b'F'] {
        log::error!("Invalid ELF magic");
        return Status::LOAD_ERROR;
    }

    #[cfg(target_arch = "x86_64")]
    if elf.header.pt2.machine().as_machine() != xmas_elf::header::Machine::X86_64 {
        log::error!("Invalid ELF machine type, expected X86_64");
        return Status::LOAD_ERROR;
    }

    #[cfg(target_arch = "aarch64")]
    if elf.header.pt2.machine().as_machine() != xmas_elf::header::Machine::AArch64 {
        log::error!("Invalid ELF machine type, expected AArch64");
        return Status::LOAD_ERROR;
    }

    let mut max_vaddr = 0;
    let mut min_vaddr = u64::MAX;
    for ph in elf.program_iter() {
        if ph.get_type() == Ok(xmas_elf::program::Type::Load) {
            if ph.virtual_addr() < min_vaddr { min_vaddr = ph.virtual_addr(); }
            if ph.virtual_addr() + ph.mem_size() > max_vaddr { max_vaddr = ph.virtual_addr() + ph.mem_size(); }
        }
    }
    
    let kernel_pages = ((max_vaddr - min_vaddr + 4095) / 4096) as usize;
    log::info!("Kernel ELF: min_vaddr={:#x}, max_vaddr={:#x}, pages={}", min_vaddr, max_vaddr, kernel_pages);
    
    let load_base = match boot::allocate_pages(
        boot::AllocateType::AnyPages,
        MemoryType::LOADER_CODE,
        kernel_pages,
    ) {
        Ok(ptr) => ptr.as_ptr() as u64,
        Err(e) => {
            log::error!("Failed to allocate pages for kernel load: {:?}", e);
            return Status::OUT_OF_RESOURCES;
        }
    };
    log::info!("Allocated kernel at {:#x}", load_base);

    let mut dynamic_vaddr = 0;

    for ph in elf.program_iter() {
        if ph.get_type() == Ok(xmas_elf::program::Type::Load) {
            let offset = ph.virtual_addr() - min_vaddr;
            let dest_ptr = load_base + offset;
            
            let dest_slice = unsafe { core::slice::from_raw_parts_mut(dest_ptr as *mut u8, ph.mem_size() as usize) };
            let src_slice = &kernel_buffer[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize];
            dest_slice[..ph.file_size() as usize].copy_from_slice(src_slice);
            dest_slice[ph.file_size() as usize..].fill(0);
        } else if ph.get_type() == Ok(xmas_elf::program::Type::Dynamic) {
            dynamic_vaddr = ph.virtual_addr();
        }
    }

    #[repr(C)]
    struct Elf64_Dyn {
        d_tag: i64,
        d_val: u64,
    }

    #[repr(C)]
    struct Elf64_Rela {
        r_offset: u64,
        r_info: u64,
        r_addend: i64,
    }

    if dynamic_vaddr != 0 {
        let dyn_ptr = (load_base + (dynamic_vaddr - min_vaddr)) as *const Elf64_Dyn;
        let mut rela_vaddr = 0;
        let mut rela_sz = 0;
        let mut rela_ent = 0;
        
        unsafe {
            let mut curr = dyn_ptr;
            while (*curr).d_tag != 0 {
                match (*curr).d_tag {
                    7 => rela_vaddr = (*curr).d_val,
                    8 => rela_sz = (*curr).d_val,
                    9 => rela_ent = (*curr).d_val,
                    _ => {}
                }
                curr = curr.add(1);
            }
            
            if rela_vaddr != 0 && rela_ent == core::mem::size_of::<Elf64_Rela>() as u64 {
                let num_relocs = rela_sz / rela_ent;
                let rela_ptr = (load_base + (rela_vaddr - min_vaddr)) as *const Elf64_Rela;
                for i in 0..num_relocs {
                    let reloc = &*rela_ptr.add(i as usize);
                    let r_type = reloc.r_info & 0xffffffff;
                    
                    // 8 = R_X86_64_RELATIVE, 1027 = R_AARCH64_RELATIVE
                    if r_type == 8 || r_type == 1027 {
                        let target = (load_base + (reloc.r_offset - min_vaddr)) as *mut u64;
                        *target = (load_base as i64 + reloc.r_addend - min_vaddr as i64) as u64;
                    }
                }
            }
        }
    }

    let entry_point = elf.header.pt2.entry_point() - min_vaddr + load_base;

    #[allow(unused_mut)]
    let mut dtb_addr = 0;
    #[allow(unused_mut)]
    let mut dtb_size = 0;

    // AArch64 DTB logic
    #[cfg(target_arch = "aarch64")]
    {
        // uefi::table::cfg::DEVICE_TREE_GUID
        let dtb_guid = uefi::Guid::from_bytes([
            0xb1, 0xb6, 0x21, 0xb1, 0x59, 0xc1, 0x4a, 0x4f, 0x93, 0x20, 0xd0, 0x04, 0x67, 0xe4, 0x8a, 0xe9,
        ]);
        
        uefi::system::with_config_table(|config_table| {
            for config_entry in config_table {
                if config_entry.guid == dtb_guid {
                    dtb_addr = config_entry.address as u64;
                    // We'll read the size from the FDT header if we need to, but the kernel can do it
                    // FDT header contains totalsize at offset 4 (big endian)
                    unsafe {
                        let ptr = dtb_addr as *const u8;
                        // Check magic (0xd00dfeed)
                        if ptr.read() == 0xd0 && ptr.add(1).read() == 0x0d && ptr.add(2).read() == 0xfe && ptr.add(3).read() == 0xed {
                            let size_bytes = [ptr.add(4).read(), ptr.add(5).read(), ptr.add(6).read(), ptr.add(7).read()];
                            dtb_size = u32::from_be_bytes(size_bytes) as usize;
                            log::info!("Found DTB at {:#x}, size: {} bytes", dtb_addr, dtb_size);
                        }
                    }
                    break;
                }
            }
        });
    }

    let boot_info = alloc::boxed::Box::new(BootInfo {
        framebuffer_addr: fb_addr,
        framebuffer_size: fb_size,
        framebuffer_info: fb_info,
        physical_memory_offset: 0,
        dtb_addr,
        dtb_size,
        memory_regions_addr: 0,
        memory_regions_len: 0,
    });
    let boot_info_static = alloc::boxed::Box::leak(boot_info);

    let map_entries = match boot::memory_map(MemoryType::LOADER_DATA) {
        Ok(map) => map.entries().count(),
        Err(_) => 512, // fallback
    };
    let max_regions = map_entries + 128; // conservative estimate
    
    let regions_ptr = match boot::allocate_pages(
        boot::AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        ((max_regions * core::mem::size_of::<MemoryRegion>()) + 4095) / 4096,
    ) {
        Ok(ptr) => ptr.as_ptr() as *mut MemoryRegion,
        Err(e) => {
            log::error!("Failed to allocate pages for memory map regions: {:?}", e);
            return Status::OUT_OF_RESOURCES;
        }
    };
    let regions = unsafe { core::slice::from_raw_parts_mut(regions_ptr, max_regions) };

    let memory_map = unsafe { boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };
    
    let mut regions_len = 0;
    for desc in memory_map.entries() {
        if regions_len >= max_regions {
            break; // Better than panicking after boot services are gone
        }
        let kind = match desc.ty {
            MemoryType::CONVENTIONAL => MemoryRegionKind::Usable,
            MemoryType::LOADER_DATA | MemoryType::LOADER_CODE => MemoryRegionKind::Bootloader,
            _ => MemoryRegionKind::Reserved,
        };
        regions[regions_len] = MemoryRegion {
            phys_start: desc.phys_start,
            page_count: desc.page_count,
            kind,
        };
        regions_len += 1;
    }

    boot_info_static.memory_regions_addr = regions_ptr as u64;
    boot_info_static.memory_regions_len = regions_len;

    #[cfg(target_arch = "x86_64")]
    let kernel_entry: extern "sysv64" fn(&'static mut BootInfo) -> ! = unsafe {
        core::mem::transmute(entry_point as usize)
    };

    #[cfg(target_arch = "aarch64")]
    let kernel_entry: extern "C" fn(&'static mut BootInfo) -> ! = unsafe {
        core::mem::transmute(entry_point as usize)
    };

    kernel_entry(boot_info_static);
}
