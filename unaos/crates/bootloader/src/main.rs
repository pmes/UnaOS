#![no_std]
#![no_main]

extern crate alloc;

use uefi::prelude::*;
use uefi::boot::{self, MemoryType};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::GraphicsOutput;
use unaos_boot_info::{BootInfo, FrameBufferInfo, PixelFormat, MemoryRegion, MemoryRegionKind};

use uefi::proto::media::file::{File, FileAttribute, FileMode};
use uefi::proto::unsafe_protocol;

// EDID — how a monitor reports its own native/preferred resolution. The first Detailed Timing
// Descriptor in the EDID block is the preferred (native) mode. UEFI exposes EDID via two protocols
// (UEFI 2.x spec 12.9), neither in the `uefi` crate, so we declare both. Both have the same layout:
// { UINT32 SizeOfEdid; UINT8 *Edid; }. We try ACTIVE (the EDID the firmware is actually using)
// first, then DISCOVERED (the raw block read from the monitor) — different firmware publishes one,
// the other, or neither.
#[repr(C)]
#[unsafe_protocol("bd8c1056-9f36-44ec-92a8-a6337f817986")]
struct EdidActiveProtocol {
    size_of_edid: u32,
    edid: *const u8,
}

#[repr(C)]
#[unsafe_protocol("1c0c34f6-d380-41fa-a049-8ad06c1a66aa")]
struct EdidDiscoveredProtocol {
    size_of_edid: u32,
    edid: *const u8,
}

/// Parse the monitor's native (preferred) resolution from a raw EDID block: the first Detailed
/// Timing Descriptor (offset 54). `None` if the block is too short/malformed or that descriptor is
/// actually a monitor descriptor (zero pixel clock) rather than a timing.
fn parse_edid_native(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.len() < 128 {
        return None;
    }
    // EDID 1.x fixed header pattern.
    if bytes[0..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }
    let d = &bytes[54..72];
    if d[0] == 0 && d[1] == 0 {
        return None; // monitor descriptor, not a timing
    }
    // Horizontal/vertical active pixels: low 8 bits + high 4 bits packed in the upper nibble.
    let w = (d[2] as usize) | (((d[4] as usize) >> 4) << 8);
    let h = (d[5] as usize) | (((d[7] as usize) >> 4) << 8);
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

/// Read the monitor's native resolution from EDID on `handle`, trying the ACTIVE then DISCOVERED
/// protocol. `None` if no EDID is exposed — callers then fall back to the firmware's current mode.
/// Returns `(width, height, source)` where source is 1 = ACTIVE protocol, 2 = DISCOVERED protocol.
fn edid_native_resolution(handle: uefi::Handle) -> Option<(usize, usize, u32)> {
    // Safety: the firmware owns the EDID buffer; it's valid while boot services are live (we read
    // it now, before exit_boot_services). `size_of_edid` is the firmware-reported length.
    if let Ok(p) = boot::open_protocol_exclusive::<EdidActiveProtocol>(handle) {
        if !p.edid.is_null() && p.size_of_edid >= 128 {
            let bytes = unsafe { core::slice::from_raw_parts(p.edid, p.size_of_edid as usize) };
            if let Some((w, h)) = parse_edid_native(bytes) {
                return Some((w, h, 1));
            }
        }
    }
    if let Ok(p) = boot::open_protocol_exclusive::<EdidDiscoveredProtocol>(handle) {
        if !p.edid.is_null() && p.size_of_edid >= 128 {
            let bytes = unsafe { core::slice::from_raw_parts(p.edid, p.size_of_edid as usize) };
            if let Some((w, h)) = parse_edid_native(bytes) {
                return Some((w, h, 2));
            }
        }
    }
    None
}

/// Surface a fatal boot error on a serial-less machine. A returned error Status just bounces the
/// firmware back to its boot picker — invisible (the "select EFI Boot -> black flash -> back to the
/// picker" symptom). If a linear framebuffer is already up, paint it solid white (0xFF in every
/// channel, so format-independent) and pause ~30s so the failure can be seen/photographed, then
/// return the Status for the caller to propagate.
fn boot_fail(fb_addr: u64, fb_size: usize, status: Status, what: &str) -> Status {
    log::error!("BOOT FATAL: {} ({:?})", what, status);
    if fb_addr != 0 && fb_size != 0 {
        unsafe { core::ptr::write_bytes(fb_addr as *mut u8, 0xFF, fb_size); }
    }
    boot::stall(core::time::Duration::from_secs(30));
    status
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    log::info!("UnaOS UEFI Bootloader Started");

    let fb_addr: u64;
    let fb_size: usize;
    let fb_info: FrameBufferInfo;
    // Display mode-selection diagnostics, surfaced to the kernel via BootInfo (bootlog readout).
    let mut edid_native_w: u32 = 0;
    let mut edid_native_h: u32 = 0;
    let mut edid_source: u32 = 0;
    let mut mode_action: u32 = 0;

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
        
        // Choose the display mode. Resolution is auto-detected (never hard-coded). We target the
        // monitor's NATIVE resolution — read from its EDID — not the GPU's largest advertised mode:
        // a non-native mode on a real panel means scaling/blur, or one the monitor can't display.
        // The fallback is the firmware's current mode (which firmware usually already sets from
        // EDID). The full mode list is logged because on serial-less hardware it's the only way to
        // see what the panel offers.
        //
        // Capped to what the kernel can back-buffer: the double-buffer back buffer is
        // stride*height*4 bytes and must fit the kernel heap. MAX_BACKBUF_BYTES is tied to
        // crates/kernel allocator::HEAP_SIZE (48 MiB) — keep in sync; 40 MiB leaves headroom.
        const MAX_BACKBUF_BYTES: usize = 40 * 1024 * 1024;

        let cur_info = gop.current_mode_info();
        let cur = cur_info.resolution();
        let cur_linear = matches!(
            cur_info.pixel_format(),
            uefi::proto::console::gop::PixelFormat::Rgb | uefi::proto::console::gop::PixelFormat::Bgr
        );
        log::info!("GOP: {} modes (firmware current {}x{}):", gop.modes().len(), cur.0, cur.1);
        for mode in gop.modes() {
            let mi = mode.info();
            let (w, h) = mi.resolution();
            log::info!("  GOP mode: {}x{} stride={} fmt={:?}", w, h, mi.stride(), mi.pixel_format());
        }

        // The monitor's native resolution from EDID, if the firmware exposes it.
        let edid = edid_native_resolution(gop_handle);
        let native = edid.map(|(w, h, _)| (w, h));
        match edid {
            Some((w, h, s)) => {
                edid_native_w = w as u32;
                edid_native_h = h as u32;
                edid_source = s;
                log::info!("EDID: monitor native resolution {}x{} (source {})", w, h, s);
            }
            None => log::info!("EDID: not available; using the firmware's current mode"),
        }

        // A "usable" mode has a linear (Rgb/Bgr) framebuffer within the back-buffer budget. BltOnly
        // and Bitmask modes have no directly-addressable framebuffer, so they're never usable here.
        let usable = |mi: &uefi::proto::console::gop::ModeInfo| -> bool {
            matches!(
                mi.pixel_format(),
                uefi::proto::console::gop::PixelFormat::Rgb | uefi::proto::console::gop::PixelFormat::Bgr
            ) && mi.stride() * mi.resolution().1 * 4 <= MAX_BACKBUF_BYTES
        };

        // Decide the target mode (None = the current mode is already what we want):
        //   1. the EDID-native resolution as a linear mode — drive the panel at native res; else
        //   2. if the current mode is already linear, keep it (firmware set it, usually from EDID); else
        //   3. the current mode is BltOnly/Bitmask (no linear framebuffer) so we MUST switch: prefer a
        //      linear mode at the firmware's chosen resolution, else the largest linear within budget.
        let by_native = native
            .and_then(|res| gop.modes().find(|m| m.info().resolution() == res && usable(m.info())));
        let target_is_native = by_native.is_some();
        let target: Option<uefi::proto::console::gop::Mode> = if by_native.is_some() {
            by_native
        } else if cur_linear {
            None
        } else {
            gop.modes()
                .find(|m| m.info().resolution() == cur && usable(m.info()))
                .or_else(|| {
                    gop.modes().filter(|m| usable(m.info())).max_by_key(|m| {
                        let (w, h) = m.info().resolution();
                        w * h
                    })
                })
        };

        match target {
            Some(mode) => {
                let (w, h) = mode.info().resolution();
                // Switch when the resolution changes or the current mode lacks a linear framebuffer.
                if (w, h) != cur || !cur_linear {
                    match gop.set_mode(&mode) {
                        Ok(()) => {
                            // 1 = drove the panel to its EDID-native res; 2 = fallback linear mode.
                            mode_action = if target_is_native { 1 } else { 2 };
                            log::info!("GOP: set linear mode {}x{}", w, h);
                        }
                        Err(e) => log::warn!(
                            "GOP: set_mode {}x{} failed ({:?}); current {}x{}", w, h, e, cur.0, cur.1
                        ),
                    }
                } else {
                    log::info!("GOP: current mode {}x{} already native + linear", w, h);
                }
            }
            None => log::info!("GOP: keeping firmware current mode {}x{}", cur.0, cur.1),
        }

        // Read the active mode *after* any set_mode (it invalidates the old framebuffer).
        let mode_info = gop.current_mode_info();
        let pixel_format = match mode_info.pixel_format() {
            uefi::proto::console::gop::PixelFormat::Rgb => PixelFormat::Rgb,
            uefi::proto::console::gop::PixelFormat::Bgr => PixelFormat::Bgr,
            _ => PixelFormat::Unknown,
        };

        // Only access the linear framebuffer if the active mode actually has one. A BltOnly/Bitmask
        // mode has no direct framebuffer (gop.frame_buffer() would panic); rather than die in the
        // bootloader we boot without a display (the kernel falls back to serial-only).
        let (fb_ptr, fb_bytes) = if matches!(pixel_format, PixelFormat::Rgb | PixelFormat::Bgr) {
            let mut fb = gop.frame_buffer();
            (fb.as_mut_ptr() as u64, fb.size())
        } else {
            mode_action = 3; // headless: no linear framebuffer available
            log::warn!("GOP: active mode has no linear framebuffer; booting without a display");
            (0u64, 0usize)
        };

        fb_info = FrameBufferInfo {
            width: mode_info.resolution().0,
            height: mode_info.resolution().1,
            stride: mode_info.stride(),
            bytes_per_pixel: 4,
            pixel_format,
        };
        fb_addr = fb_ptr;
        fb_size = fb_bytes;
    }

    // Open the filesystem on the SAME device this bootloader image was loaded from. A real machine
    // exposes many SimpleFileSystem volumes (internal ESP, recovery, our USB stick, ...), and the
    // old `get_handle_for_protocol::<SimpleFileSystem>()` returned an arbitrary one — on the Mac it
    // picked the wrong volume, kernel.elf wasn't there, and we returned NOT_FOUND so the firmware
    // silently bounced back to the boot picker. get_image_file_system resolves the loaded-image
    // device handle, so we always read kernel.elf from our own ESP (QEMU has one volume, unaffected).
    let mut sfs = match boot::get_image_file_system(boot::image_handle()) {
        Ok(sfs) => sfs,
        Err(e) => return boot_fail(fb_addr, fb_size, e.status(), "open boot-volume filesystem"),
    };
    let mut root = match sfs.open_volume() {
        Ok(root) => root,
        Err(e) => return boot_fail(fb_addr, fb_size, e.status(), "open root volume"),
    };

    let mut kernel_file = match root.open(cstr16!("kernel.elf"), FileMode::Read, FileAttribute::empty()) {
        Ok(file) => match file.into_regular_file() {
            Some(f) => f,
            None => return boot_fail(fb_addr, fb_size, Status::LOAD_ERROR, "kernel.elf is not a regular file"),
        },
        Err(e) => return boot_fail(fb_addr, fb_size, e.status(), "open kernel.elf (missing on our volume?)"),
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

    // ACPI RSDP (x86_64): the firmware hands us the Root System Description Pointer via a UEFI
    // configuration-table entry. Prefer the ACPI 2.0 entry (XSDT, 64-bit pointers); fall back to
    // the legacy ACPI 1.0 entry (RSDT). The kernel walks it to find the MADT (CPU topology) for
    // SMP bring-up. (aarch64 uses the DTB instead, handled above.)
    #[allow(unused_mut)]
    let mut rsdp_addr = 0u64;
    #[cfg(target_arch = "x86_64")]
    {
        use uefi::table::cfg::ConfigTableEntry;
        uefi::system::with_config_table(|config_table| {
            for entry in config_table {
                if entry.guid == ConfigTableEntry::ACPI2_GUID {
                    rsdp_addr = entry.address as u64;
                    break; // ACPI 2.0 preferred; stop on first match
                }
                if entry.guid == ConfigTableEntry::ACPI_GUID && rsdp_addr == 0 {
                    rsdp_addr = entry.address as u64; // 1.0 fallback, keep scanning for 2.0
                }
            }
        });
        log::info!("ACPI RSDP at {:#x}", rsdp_addr);
    }

    let boot_info = alloc::boxed::Box::new(BootInfo {
        framebuffer_addr: fb_addr,
        framebuffer_size: fb_size,
        framebuffer_info: fb_info,
        physical_memory_offset: 0,
        dtb_addr,
        dtb_size,
        rsdp_addr,
        memory_regions_addr: 0,
        memory_regions_len: 0,
        edid_native_width: edid_native_w,
        edid_native_height: edid_native_h,
        edid_source,
        mode_action,
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
