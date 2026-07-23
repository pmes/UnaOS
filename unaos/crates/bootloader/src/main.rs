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

/// UEFI boot-time diagnostics (`bootdiag` feature; enabled via `UNAOS_BOOTDIAG=1`). For the Jetson
/// Orin headless bring-up (JM2): surface the firmware identity, whether the platform publishes ANY
/// GraphicsOutput protocol, ConOut's device path, and the device-tree console/UART truth. Best-effort:
/// absent nodes/properties and unavailable protocols degrade to a note rather than aborting boot (the
/// DTB parse additionally assumes a well-formed firmware device tree). Called while boot services are
/// live, before the GOP lookup.
#[cfg(feature = "bootdiag")]
fn bootdiag() {
    use uefi::boot::{self, SearchType};
    use uefi::proto::console::gop::GraphicsOutput;

    log::info!("BOOTDIAG: ===== UnaOS boot diagnostics (bootdiag) =====");

    // Firmware identity + the UEFI spec revision it implements.
    log::info!(
        "BOOTDIAG: firmware vendor='{}' fw-revision={:#010x} uefi-revision={}",
        uefi::system::firmware_vendor(),
        uefi::system::firmware_revision(),
        uefi::system::uefi_revision(),
    );

    // How many handles publish GraphicsOutput? JM1's `get_handle_for_protocol` returned NOT_FOUND on
    // the Orin; count them explicitly so a truly headless SoC reads 0 (LocateHandleBuffer → NOT_FOUND)
    // rather than leaving it ambiguous.
    match boot::locate_handle_buffer(SearchType::from_proto::<GraphicsOutput>()) {
        Ok(buf) => log::info!("BOOTDIAG: GraphicsOutput handles = {}", buf.len()),
        Err(e) => log::info!("BOOTDIAG: GraphicsOutput handles = 0 ({:?})", e.status()),
    }

    // ConOut's device path — what device the firmware console output targets (a UEFI-side cross-check
    // on the UART truth). The DTB stdout-path below is the canonical answer.
    bootdiag_conout();

    // The device tree's console truth (aarch64 only): /chosen stdout-path names the console UART, and
    // /aliases resolves serial* to the physical UART node (its name carries the MMIO base). This is
    // exactly what JM3 needs to confirm the tegra driver's UARTC base.
    #[cfg(target_arch = "aarch64")]
    bootdiag_dtb();

    log::info!("BOOTDIAG: ===== end diagnostics =====");
}

/// Best-effort dump of ConOut's device path. The `uefi` crate has no safe getter for the ConsoleOut
/// handle, so read it from the raw system table; every step degrades to an honest note on failure
/// (no panic). Uses `to_string16` (not the `Display` impl, which `.unwrap()`s the text protocol).
#[cfg(feature = "bootdiag")]
fn bootdiag_conout() {
    use uefi::boot;
    use uefi::proto::device_path::{
        text::{AllowShortcuts, DisplayOnly},
        DevicePath,
    };

    let Some(st) = uefi::table::system_table_raw() else {
        log::info!("BOOTDIAG: ConOut: system table unavailable");
        return;
    };
    // SAFETY: the system table is valid while boot services are live.
    let conout_ptr = unsafe { st.as_ref() }.stdout_handle;
    // SAFETY: firmware-provided ConOut handle; `from_ptr` returns None if it is null.
    let Some(handle) = (unsafe { uefi::Handle::from_ptr(conout_ptr) }) else {
        log::info!("BOOTDIAG: ConOut: handle is null");
        return;
    };
    match boot::open_protocol_exclusive::<DevicePath>(handle) {
        Ok(dp) => match dp.to_string16(DisplayOnly(true), AllowShortcuts(true)) {
            Ok(s) => log::info!("BOOTDIAG: ConOut device path: {}", s),
            Err(e) => log::info!("BOOTDIAG: ConOut device path unrenderable ({:?})", e),
        },
        Err(e) => log::info!("BOOTDIAG: ConOut has no DevicePath ({:?})", e.status()),
    }
}

/// Best-effort dump of the device-tree console/UART truth: the DTB model, the raw `/chosen`
/// `stdout-path` (carries baud/format), `bootargs`, and the `serial*`/`uart*` `/aliases` (whose node
/// names carry the physical MMIO base). Parses the DTB the firmware published as a UEFI configuration
/// table. Any absent node/property degrades to a note; the header + block bounds are validated so a
/// truncated DTB is skipped rather than faulting (a well-formed firmware DTB is otherwise assumed).
#[cfg(all(feature = "bootdiag", target_arch = "aarch64"))]
fn bootdiag_dtb() {
    // EFI_DTB_TABLE_GUID (UEFI spec / EDK2 gFdtTableGuid), the same GUID the kernel handoff scan
    // uses. The uefi 0.38 crate ships no named constant for it (ConfigTableEntry exposes only
    // ACPI/SMBIOS/... GUIDs), so define it by string via the mistake-proof `guid!` macro rather
    // than a hand-laid mixed-endian byte array.
    let dtb_guid = uefi::guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");
    let mut dtb_addr: u64 = 0;
    uefi::system::with_config_table(|entries| {
        for entry in entries {
            if entry.guid == dtb_guid {
                dtb_addr = entry.address as u64;
                break;
            }
        }
    });
    if dtb_addr == 0 {
        log::info!("BOOTDIAG: no DTB configuration table published by firmware");
        return;
    }

    // Read the FDT magic + total size from the header (both big-endian, at offsets 0 and 4).
    let hdr = dtb_addr as *const u8;
    // SAFETY: firmware DTB base; the 8-byte header is valid while boot services are live.
    let (magic, total) = unsafe {
        let magic = u32::from_be_bytes([hdr.read(), hdr.add(1).read(), hdr.add(2).read(), hdr.add(3).read()]);
        let total = u32::from_be_bytes([hdr.add(4).read(), hdr.add(5).read(), hdr.add(6).read(), hdr.add(7).read()]);
        (magic, total)
    };
    // fdt 0.1.5's `Fdt::new` validates only magic + totalsize, not the internal block offsets, and
    // several accessors PANIC on absent/empty data (`chosen()` → `.expect("/chosen is required")`,
    // `root().model()` → `.unwrap()`, `Chosen::stdout/bootargs` underflow on an empty value).
    // Firmware may legitimately omit any of these. So: require a full v17 header, bound the struct +
    // strings blocks against the buffer (else the first traversal indexes out of bounds), and read
    // every field via `find_node`/`property` (which return `Option`) — never via the panicking
    // helpers. A sparse firmware DTB then degrades to a note instead of aborting boot.
    if magic != 0xd00d_feed || total < 40 || total > 4 * 1024 * 1024 {
        log::info!("BOOTDIAG: DTB @ {:#x}: bad magic/size (magic={:#010x} size={})", dtb_addr, magic, total);
        return;
    }
    // SAFETY: total >= 40 confirms the full v17 fdt_header is present at hdr; these fields all lie
    // within its first 40 bytes (off_dt_struct@8, off_dt_strings@12, size_dt_strings@32,
    // size_dt_struct@36 — all big-endian).
    let (off_struct, size_struct, off_strings, size_strings) = unsafe {
        let off_struct = u32::from_be_bytes([hdr.add(8).read(), hdr.add(9).read(), hdr.add(10).read(), hdr.add(11).read()]) as u64;
        let off_strings = u32::from_be_bytes([hdr.add(12).read(), hdr.add(13).read(), hdr.add(14).read(), hdr.add(15).read()]) as u64;
        let size_strings = u32::from_be_bytes([hdr.add(32).read(), hdr.add(33).read(), hdr.add(34).read(), hdr.add(35).read()]) as u64;
        let size_struct = u32::from_be_bytes([hdr.add(36).read(), hdr.add(37).read(), hdr.add(38).read(), hdr.add(39).read()]) as u64;
        (off_struct, size_struct, off_strings, size_strings)
    };
    if off_struct + size_struct > total as u64 || off_strings + size_strings > total as u64 {
        log::info!("BOOTDIAG: DTB @ {:#x}: struct/strings block out of bounds — skipping parse", dtb_addr);
        return;
    }
    // SAFETY: `total` is the FDT-declared length, bounded to 4 MiB above.
    let slice = unsafe { core::slice::from_raw_parts(hdr, total as usize) };
    let fdt = match fdt::Fdt::new(slice) {
        Ok(f) => f,
        Err(e) => {
            log::info!("BOOTDIAG: DTB @ {:#x}: parse failed ({:?})", dtb_addr, e);
            return;
        }
    };
    // Board model via the root node's `model` property (NOT `root().model()`, which unwraps).
    let model = fdt
        .find_node("/")
        .and_then(|r| r.property("model"))
        .and_then(|p| p.as_str())
        .unwrap_or("(no model property)");
    log::info!("BOOTDIAG: DTB @ {:#x} size={} model='{}'", dtb_addr, total, model);

    // /chosen: the console UART truth. Read properties directly off the node — never via
    // `fdt.chosen()` (whose helpers panic when /chosen is absent or a property value is empty). The
    // raw stdout-path string carries the console (often an alias like `serial0`, with a baud suffix);
    // the /aliases dump below resolves that alias to the physical UART node.
    if let Some(chosen) = fdt.find_node("/chosen") {
        match chosen.property("stdout-path").and_then(|p| p.as_str()) {
            Some(sp) => log::info!("BOOTDIAG: /chosen stdout-path = '{}'", sp),
            None => log::info!("BOOTDIAG: /chosen has no stdout-path property"),
        }
        if let Some(args) = chosen.property("bootargs").and_then(|p| p.as_str()) {
            log::info!("BOOTDIAG: /chosen bootargs = '{}'", args);
        }
    } else {
        log::info!("BOOTDIAG: no /chosen node");
    }

    // /aliases: the serial*/uart* → UART node mapping. The node name (e.g. "serial@0c280000")
    // carries the physical MMIO base — directly comparable to the tegra driver's UARTC 0x0C28_0000.
    match fdt.aliases() {
        Some(aliases) => {
            for (name, path) in aliases.all() {
                if name.contains("serial") || name.contains("uart") {
                    log::info!("BOOTDIAG: alias {} -> {}", name, path);
                }
            }
        }
        None => log::info!("BOOTDIAG: no /aliases node"),
    }
}

#[entry]
fn main() -> Status {
    #[cfg(feature = "unaos_ivb")]
    let (t0, g0) = unsafe { (read_igpu_trace(), read_gmux_trace()) };

    uefi::helpers::init().unwrap();
    log::info!("UnaOS UEFI Bootloader Started");

    // Boot-time diagnostics (UNAOS_BOOTDIAG=1 → `bootdiag` feature). Additive and OFF by default
    // (byte-identical boot logs when off). Runs here — while boot services are live and before any
    // GOP access — so a headless SoC (Jetson Orin) still surfaces the firmware identity, the GOP
    // handle count, ConOut's device path, and the DTB console/UART truth. Best-effort: absent data
    // degrades to a note rather than aborting boot.
    #[cfg(feature = "bootdiag")]
    bootdiag();

    let fb_addr: u64;
    let fb_size: usize;
    let fb_info: FrameBufferInfo;
    // Display mode-selection diagnostics, surfaced to the kernel via BootInfo (bootlog readout).
    let mut edid_native_w: u32 = 0;
    let mut edid_native_h: u32 = 0;
    let mut edid_source: u32 = 0;
    let mut mode_action: u32 = 0;

    'gop: {
        // Acquire the GraphicsOutput protocol. On firmware that publishes no GOP at all — a headless
        // server SoC like the Jetson Orin, which drives only a serial console — one of these lookups
        // fails (JM1 saw `get_handle_for_protocol` → NOT_FOUND). Rather than bounce back to the
        // firmware boot picker (the old `return Status::UNSUPPORTED`, invisible on a serial-less box),
        // both Err arms now BOOT HEADLESS: zero the framebuffer info so `fb_addr == 0` tells the kernel
        // to fall back to a serial-only console, and record `mode_action = 4` ("headless, no GOP
        // protocol") for the boot-log readout. `break 'gop` skips the whole display-mode-selection body.
        let gop_handle = match boot::get_handle_for_protocol::<GraphicsOutput>() {
            Ok(handle) => handle,
            Err(e) => {
                log::warn!("No GraphicsOutput handle ({:?}); booting headless (serial only)", e);
                fb_info = FrameBufferInfo {
                    width: 0,
                    height: 0,
                    stride: 0,
                    bytes_per_pixel: 4,
                    pixel_format: PixelFormat::Unknown,
                };
                fb_addr = 0;
                fb_size = 0;
                mode_action = 4;
                break 'gop;
            }
        };
        let mut gop = match boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle) {
            Ok(gop) => gop,
            Err(e) => {
                log::warn!("GraphicsOutput handle present but open failed ({:?}); booting headless", e);
                fb_info = FrameBufferInfo {
                    width: 0,
                    height: 0,
                    stride: 0,
                    bytes_per_pixel: 4,
                    pixel_format: PixelFormat::Unknown,
                };
                fb_addr = 0;
                fb_size = 0;
                mode_action = 4;
                break 'gop;
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

    // aarch64: make the just-written (and just-relocated) kernel image coherent with the
    // INSTRUCTION side before jumping into it. ARMv8 I-caches are not coherent with stores: the
    // image bytes sit in the D-cache, while the I-cache (and unified L2 as fetched) may hold
    // STALE lines from whatever previously occupied these physical pages — across repeated boot
    // cycles UEFI's allocator hands back the same base, so that is the PREVIOUS kernel build's
    // code. Executing a stale line raises an EC=0 "unknown reason" exception at an address whose
    // CURRENT bytes disassemble innocently (the JB1b metal lesson on the Orin A78AE, 2026-07-06 —
    // nondeterministic, and invisible in QEMU, which models no I-cache). The ARMv8
    // code-modification sequence: clean every image line to the Point of Unification, then
    // invalidate the whole I-cache, with barriers.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let mut addr = load_base & !63;
        let end = load_base + (max_vaddr - min_vaddr);
        while addr < end {
            core::arch::asm!("dc cvau, {}", in(reg) addr, options(nostack, preserves_flags));
            addr += 64;
        }
        core::arch::asm!(
            "dsb ish",
            "ic iallu",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags)
        );
    }

    let entry_point = elf.header.pt2.entry_point() - min_vaddr + load_base;

    #[allow(unused_mut)]
    let mut dtb_addr = 0;
    #[allow(unused_mut)]
    let mut dtb_size = 0;

    // AArch64 DTB logic
    #[cfg(target_arch = "aarch64")]
    {
        // EFI_DTB_TABLE_GUID (UEFI spec / EDK2 gFdtTableGuid). The uefi 0.38 crate ships no named
        // constant for it (ConfigTableEntry exposes only ACPI/SMBIOS/... GUIDs), so define it by
        // string via the mistake-proof `guid!` macro rather than a hand-laid mixed-endian byte array.
        let dtb_guid = uefi::guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");
        
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
        #[cfg(feature = "unaos_ivb")]
        igpu_trace_0: t0,
        #[cfg(feature = "unaos_ivb")]
        gmux_trace_0: g0,
        #[cfg(feature = "unaos_ivb")]
        igpu_trace_1: [0; 11],
        #[cfg(feature = "unaos_ivb")]
        igpu_trace_2: [0; 11],
        #[cfg(feature = "unaos_ivb")]
        igpu_trace_valid: false,
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

    // JB6 (Jetson Orin, attended-bench arc): NVIDIA's UEFI `XhciControllerDxe.OnExitBootServices`
    // tears the XUSB Falcon down on a Device-Tree boot (USBCMD<-0, padctl DeInitHw, power-gate
    // Assert), but SELF-SKIPS the whole teardown when an ACPI table is present
    // (`EfiGetSystemConfigurationTable(&gEfiAcpiTableGuid)` succeeds -> immediate `return`;
    // source-verified in edk2-nvidia). Installing a minimal, well-formed dummy ACPI 2.0 RSDP/XSDT
    // into the UEFI config table right before ExitBootServices flips that branch, so UnaOS inherits
    // a LIVE Falcon (the Linux-style "inherit, don't revive"). Runtime-gated on a tegra234 DTB
    // sniff, so QEMU `esp-arm` (virt DTB, never contains "tegra234") stays byte-identical.
    #[cfg(target_arch = "aarch64")]
    {
        // JB8 (Jetson Orin, bench arc): the JB6 shim below skips the EBS *teardown*, yet the kernel
        // still inherits a HALTED Falcon (JB7: CPUCTL=0xffffffff, core reset-held, no NS lever). So
        // the halt happens earlier — inside UEFI's own driver lifetime. This witness reads the
        // Falcon's CPUCTL/BOOTVEC + firmware header from HERE, pre-ExitBootServices, while
        // XhciControllerDxe still owns a live block. One bit of output decides the whole arc:
        //   CPUCTL != 0xffffffff pre-EBS  -> the kill is in the EBS window (huntable from the loader);
        //   CPUCTL == 0xffffffff pre-EBS  -> Start (or MB2/secure) never left the core running, and
        //                                    the lever is a forced driver reconnect (jb8lever below).
        if dtb_is_tegra234(dtb_addr, dtb_size) {
            jb8_falcon_witness("pre-EBS");
            #[cfg(feature = "jb8lever")]
            jb8_disconnect_lever();
            install_tegra_acpi_shim();
        }
    }

    #[cfg(feature = "unaos_ivb")]
    {
        boot_info_static.igpu_trace_1 = unsafe { read_igpu_trace() };
        log::info!("iGPU trace 1 (pre-EBS) collected.");
    }

    let memory_map = unsafe { boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };

    #[cfg(feature = "unaos_ivb")]
    {
        boot_info_static.igpu_trace_2 = unsafe { read_igpu_trace() };
        boot_info_static.igpu_trace_valid = true;
    }
    
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

/// JB6 (Jetson Orin attended-bench arc): install a minimal, spec-correct dummy ACPI 2.0 RSDP+XSDT
/// into the UEFI configuration table immediately before ExitBootServices, so NVIDIA's
/// `XhciControllerDxe.OnExitBootServices` (and `XusbControllerDxe`) see an ACPI boot and SKIP the
/// XUSB Falcon teardown — UnaOS then inherits a live Falcon instead of a power-gated dead block.
///
/// Blast radius (source-verified against edk2-nvidia, 2026-07-08): the DXE core signals the
/// `gEfiAcpiTableGuid` event group from inside `InstallConfigurationTable`, so our install fires,
/// synchronously and before ExitBootServices, exactly one NVIDIA callback — EqosDeviceDxe's
/// `UpdateACPIMacAddress`. That callback presence-checks the table then walks the ACPI *SDT
/// protocol* registry (`gEfiAcpiSdtProtocolGuid`), never our RSDP; with no SDT protocol / no DSDT
/// it early-returns EFI_SUCCESS. So our RSDP/XSDT bytes are never dereferenced on the DT-boot path
/// and a zero-entry XSDT is safe. The tables are made fully valid anyway (both RSDP checksums, a
/// valid empty XSDT) as cheap defensive insurance.
///
/// Runtime-gated (at the call site, via `dtb_is_tegra234`) so QEMU `esp-arm` (virt DTB, which
/// never contains "tegra234") never reaches this: on non-Tegra firmware the whole JB6/JB8 block
/// is a no-op.
#[cfg(target_arch = "aarch64")]
fn install_tegra_acpi_shim() {
    // One page of ACPI_RECLAIM memory survives ExitBootServices and is mapped Reserved by the
    // kernel's memory-map pass (never handed to the allocator), so the tables persist untouched.
    let page = match boot::allocate_pages(boot::AllocateType::AnyPages, MemoryType::ACPI_RECLAIM, 1)
    {
        Ok(p) => p.as_ptr(),
        Err(e) => {
            log::error!("JB6: ACPI shim page alloc failed: {:?} — teardown will NOT skip", e);
            return;
        }
    };
    unsafe { core::ptr::write_bytes(page, 0, 4096) };
    let base = page as u64;
    let xsdt_addr = base + 64; // RSDP at [0..36), XSDT at [64..100) — both inside one 4 KiB page

    // XSDT: a standard 36-byte ACPI System Description Table header with ZERO entry pointers
    // (Length == header size). An empty XSDT is legal and the safest possible payload.
    let mut xsdt = [0u8; 36];
    xsdt[0..4].copy_from_slice(b"XSDT");
    xsdt[4..8].copy_from_slice(&36u32.to_le_bytes()); // Length (header only, no entries)
    xsdt[8] = 1; // Revision
    xsdt[10..16].copy_from_slice(b"UNAOS "); // OEMID (6)
    xsdt[16..24].copy_from_slice(b"UNAOSTBL"); // OEM Table ID (8)
    xsdt[24..28].copy_from_slice(&1u32.to_le_bytes()); // OEM Revision
    xsdt[28..32].copy_from_slice(b"UNAO"); // Creator ID (4)
    xsdt[32..36].copy_from_slice(&1u32.to_le_bytes()); // Creator Revision
    xsdt[9] = acpi_checksum(&xsdt); // whole-table byte sum == 0 (mod 256)

    // RSDP: ACPI 2.0 (Revision 2), 36 bytes. RsdtAddress left 0 (XSDT-only, legal at rev 2).
    let mut rsdp = [0u8; 36];
    rsdp[0..8].copy_from_slice(b"RSD PTR "); // signature (the trailing space is required)
    rsdp[9..15].copy_from_slice(b"UNAOS "); // OEMID (6)
    rsdp[15] = 2; // Revision = 2 (ACPI 2.0+ — what gEfiAcpiTableGuid denotes)
    rsdp[20..24].copy_from_slice(&36u32.to_le_bytes()); // Length (whole RSDP)
    rsdp[24..32].copy_from_slice(&xsdt_addr.to_le_bytes()); // XsdtAddress (64-bit)
    rsdp[8] = acpi_checksum(&rsdp[0..20]); // v1 checksum: first 20 bytes sum == 0
    rsdp[32] = acpi_checksum(&rsdp); // extended checksum: all 36 bytes sum == 0

    unsafe {
        core::ptr::copy_nonoverlapping(rsdp.as_ptr(), page, 36);
        core::ptr::copy_nonoverlapping(xsdt.as_ptr(), xsdt_addr as *mut u8, 36);
    }

    // gEfiAcpiTableGuid (ACPI 2.0+). Installing it under this GUID is what flips
    // XhciControllerDxe's teardown to its no-op ACPI branch. `&'static Guid` is required by the
    // boot service, hence the `static`.
    static ACPI2_TABLE_GUID: uefi::Guid = uefi::guid!("8868e871-e4f1-11d3-bc22-0080c73c8881");
    match unsafe {
        boot::install_configuration_table(&ACPI2_TABLE_GUID, base as *const core::ffi::c_void)
    } {
        Ok(()) => log::info!(
            "JB6: dummy ACPI 2.0 table installed (RSDP @{:#x}, XSDT @{:#x}) — XUSB Falcon teardown will self-skip",
            base,
            xsdt_addr
        ),
        Err(e) => log::error!("JB6: install_configuration_table failed: {:?} — teardown will NOT skip", e),
    }
}

/// Gate for the JB6 shim + JB8 probes: only the real Orin firmware DTB. QEMU virt's root
/// `compatible` is "linux,dummy-virt" and never contains "tegra234"; a raw substring scan of the
/// bounded blob is decisive and needs no FDT parser in the loader.
#[cfg(target_arch = "aarch64")]
fn dtb_is_tegra234(dtb_addr: u64, dtb_size: usize) -> bool {
    if dtb_addr == 0 || dtb_size < 8 || dtb_size > 4 * 1024 * 1024 {
        return false;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let tegra = blob.windows(8).any(|w| w == b"tegra234");
    if !tegra {
        log::info!("JB6/JB8: firmware DTB is not tegra234 — tegra shims skipped (QEMU-safe no-op)");
    }
    tegra
}

/// JB8: pre-ExitBootServices Falcon witness. Metal boots 2–3 showed UEFI never programs the FPCI
/// CFG BARs at all (CFG_7 stays raw garbage while USB-reader boots demonstrably work) — the NVIDIA
/// driver drives the block through the DT-fixed addresses (`FalconSetHostBase2Addr(0x3650000)`),
/// not FPCI routing. So the CFG header is dumped for the record only, and the CSB read uses the
/// same DT-fixed windows UEFI's own FalconLib uses — gated NOT on CFG_7 but on "a Usb2Hc handle
/// exists" (= generic XhciDxe bound on top of the NVIDIA stack = XhciControllerDxe.Start ran = the
/// block is powered and its MMIO live). CSB recipe is byte-for-byte the kernel's JB3 path
/// (page-select @BAR2+0x9c, window @+0x2000; FW_SCRATCH ioctl @+0x1000, reply @+0x1c) — the only
/// safe CSB aperture (the FPCI/CFG-path one is EL3-FATAL; never resurrect it).
#[cfg(target_arch = "aarch64")]
fn jb8_falcon_witness(tag: &str) {
    const XUSB_FPCI: u64 = 0x0361_0000; // FPCI CFG header (== DT xHC base; cap regs live here too)
    const XUSB_BAR2: u64 = 0x0365_0000; // ARU/CSB window, DT-fixed (UEFI FalconLib's Base2Addr)
    let cfg = |off: u64| unsafe { core::ptr::read_volatile((XUSB_FPCI + off) as *const u32) };
    let cfg0 = cfg(0x0); // vendor/device id — liveness canary
    let cfg1 = cfg(0x4); // io/mem/busmaster enables
    let cfg4 = cfg(0x10); // BAR0 (UEFI leaves it unprogrammed; recorded, not trusted)
    let cfg7 = cfg(0x1c); // BAR2 routing (same)
    log::info!(
        "JB8[{tag}]: FPCI CFG_0={cfg0:#010x} CFG_1={cfg1:#010x} (mem={} busmaster={}) CFG_4={cfg4:#010x} CFG_7={cfg7:#010x}",
        (cfg1 >> 1) & 1,
        (cfg1 >> 2) & 1
    );
    static USB2_HC_GUID: uefi::Guid = uefi::guid!("3e745226-9818-45b6-a2ac-d7cd0e8ba2bc");
    let started = boot::locate_handle_buffer(boot::SearchType::ByProtocol(&USB2_HC_GUID))
        .map(|h| !h.is_empty())
        .unwrap_or(false);
    if cfg0 == 0xffff_ffff || !started {
        log::info!("JB8[{tag}]: no Usb2Hc handle (XhciControllerDxe not Started) — CSB witness skipped");
        return;
    }
    let (bar0, bar2) = (XUSB_FPCI, XUSB_BAR2);
    let r = |off: u64| unsafe { core::ptr::read_volatile((bar2 + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe { core::ptr::write_volatile((bar2 + off) as *mut u32, v) };
    let csb_r = |addr: u32| {
        w(0x9c, (addr >> 9) & 0x7f_ffff);
        r(0x2000 + (addr & 0x1ff) as u64)
    };
    let fw_hdr = |word_off: u32| {
        w(0x1000, (17u32 << 24) | word_off);
        r(0x1c)
    };
    let cpuctl = csb_r(0x100);
    let bootvec = csb_r(0x104);
    let (codetag, codesize) = (fw_hdr(0x8), fw_hdr(0xc));
    log::info!(
        "JB8[{tag}]: FALCON CPUCTL={cpuctl:#010x} (halted={} stopped={}) BOOTVEC={bootvec:#010x} fw codetag={codetag:#010x} codesize={codesize:#010x}",
        (cpuctl >> 4) & 1,
        (cpuctl >> 5) & 1
    );
    // USBSTS.CNR (bit 11): edk2-nvidia's T234 Start skips the firmware load entirely when CNR
    // clears within 200 ms (MB2-resident FW), else IFR-DMA-autoboots it. CNR here pre-EBS says
    // which of those worlds this boot lives in (remember CNR is a stale latch post-halt — it is
    // only trustworthy as a "was ready at some point" witness, never as "running now").
    if bar0 != 0 {
        let cap = unsafe { core::ptr::read_volatile(bar0 as *const u32) };
        let usbsts =
            unsafe { core::ptr::read_volatile((bar0 + (cap & 0xff) as u64 + 0x4) as *const u32) };
        log::info!(
            "JB8[{tag}]: xHC cap={cap:#010x} USBSTS={usbsts:#010x} CNR={}",
            (usbsts >> 11) & 1
        );
    }
    // JB9d (bench 2026-07-08): the pre-EBS MAILBOX liveness bit — the one datum the whole JB9
    // kernel campaign is missing. Post-EBS the FW is mailbox-dead at every point (raw-handoff /
    // post-restart / post-PG-cycle, 5-attempt ladders), and the only actor between "FW provably
    // servicing pre-EBS" (JB8: HCH=0, enumerating) and the dead kernel handoff is generic
    // XhciDxe's XhcHaltHC (USBCMD.RS=0) + PciIo attribute restore. This send brackets the kill:
    //   SERVICED here + dead at kernel raw-handoff  -> the EBS halt parks the FW service loop
    //     (kernel counter-lever: attach WITHOUT HCRST / replay the halted state);
    //   silent here too (while USB demonstrably enumerates) -> the mailbox was never
    //     NS-serviceable on this firmware, and mailbox silence stops meaning "FW idle".
    // Same MSG_ENABLED words the kernel's jb3_aru_probe writes (owner-claim -> DATA_IN ->
    // CMD|=INT_EN|DEST_FALC); 3 short attempts (~15 ms total) to keep the pre-EBS dwell small —
    // the NVIDIA driver still owns the block, so a held owner semaphore is itself a datum.
    let own0 = r(0x010);
    let mut serviced = false;
    let mut attempts = 0u32;
    while attempts < 3 && !serviced {
        attempts += 1;
        w(0x010, 2); // OWNER_SW
        if r(0x010) != 2 {
            boot::stall(core::time::Duration::from_micros(5_000));
            continue;
        }
        w(0x008, 5u32 << 24); // MBOX_CMD_MSG_ENABLED
        w(0x004, r(0x004) | (1 << 31) | (1 << 27)); // INT_EN | DEST_FALC
        let mut polls = 0u32;
        while polls < 50 {
            if r(0x00c) != 0 || r(0x010) != 2 {
                serviced = true;
                break;
            }
            boot::stall(core::time::Duration::from_micros(100));
            polls += 1;
        }
        if r(0x010) == 2 {
            w(0x010, 0); // release
        }
    }
    log::info!(
        "JB9d[{tag}]: MBOX MSG_ENABLED {} (owner pre={own0:#x} now={:#x} data_out={:#010x}, attempts={attempts})",
        if serviced { "SERVICED" } else { "SILENT" },
        r(0x010),
        r(0x00c)
    );
}

/// JB8 lever (feature `jb8lever`, arroyo `UNAOS_JB8_LEVER=1` — separate media, only flashed when
/// the plain witness shows the Falcon already dead pre-EBS): force a full driver-model
/// disconnect + reconnect of every USB2 host-controller handle, so NVIDIA's
/// `XhciControllerDxe.Start` runs AGAIN right before handoff — a fresh Falcon firmware load /
/// core start at the last possible moment, with the JB6 shim then skipping the teardown. Runs
/// after the kernel + DTB are fully read into memory, so tearing down a USB boot volume's stack
/// is safe. A witness re-read reports whether the reconnect left the core running.
#[cfg(all(target_arch = "aarch64", feature = "jb8lever"))]
fn jb8_disconnect_lever() {
    // gEfiUsb2HcProtocolGuid — the handle XhciControllerDxe installs its host-controller I/O on.
    // It exists only after Start has run; on a plain auto-boot BDS never connects the XHCI driver
    // at all (metal JB8: BARs unprogrammed, mem decode off), so when no Usb2Hc handle exists the
    // lever is a loader-side `connect -r`: ConnectController(recursive) over every handle, which
    // is what makes XhciControllerDxe.Start run (PG cycle + IFR firmware load) for the first time.
    static USB2_HC_GUID: uefi::Guid = uefi::guid!("3e745226-9818-45b6-a2ac-d7cd0e8ba2bc");
    // Boot-3 taught: on a USB-reader boot XhciControllerDxe IS Started (Usb2Hc exists), the Falcon
    // runs during UEFI, and the un-gated killer candidate at EBS is GENERIC edk2 XhciDxe's
    // XhcExitBootService (unconditional XhcHaltHC + OriginalPciAttributes restore — no ACPI gate;
    // source-verified). Its DriverBindingStop CLOSES that EBS event. So the lever is now
    // disconnect-and-STAY-disconnected: Stop halts the xHC (same HCH=1 either way) but nothing
    // touches the block at EBS afterwards, and the kernel inherits whatever the Falcon state
    // truly is. If Usb2Hc is absent (native-slot boot, driver never Started), connect-all first
    // so there is a Started stack to witness + disconnect.
    let mut handles = boot::locate_handle_buffer(boot::SearchType::ByProtocol(&USB2_HC_GUID));
    if !matches!(&handles, Ok(h) if !h.is_empty()) {
        log::info!("JB8-lever: no Usb2Hc handles — connect-all pass to Start XhciControllerDxe");
        if let Ok(all) = boot::locate_handle_buffer(boot::SearchType::AllHandles) {
            let mut ok = 0u32;
            for &h in all.iter() {
                if boot::connect_controller(h, &[], None, true).is_ok() {
                    ok += 1;
                }
            }
            log::info!("JB8-lever: connect-all over {} handles (connected={ok})", all.len());
        }
        handles = boot::locate_handle_buffer(boot::SearchType::ByProtocol(&USB2_HC_GUID));
    }
    jb8_falcon_witness("pre-disconnect");
    match handles {
        Ok(h) if !h.is_empty() => {
            for &hc in h.iter() {
                let d = boot::disconnect_controller(hc, None, None);
                log::info!("JB8-lever: handle {:#x} disconnect(no reconnect)={d:?}", hc.as_ptr() as usize);
            }
        }
        _ => log::info!("JB8-lever: still no Usb2Hc handle — nothing to disconnect"),
    }
    jb8_falcon_witness("post-disconnect");
}

/// 8-bit ACPI checksum: the byte value that makes the sum of every byte in `bytes` zero (mod 256).
#[cfg(target_arch = "aarch64")]
fn acpi_checksum(bytes: &[u8]) -> u8 {
    let sum = bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    0u8.wrapping_sub(sum)
}

#[cfg(feature = "unaos_ivb")]
unsafe fn read_igpu_trace() -> [u32; 11] {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use core::arch::asm;
        
        let outl = |port: u16, val: u32| {
            unsafe { asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags)); }
        };
        let inl = |port: u16| -> u32 {
            let mut val: u32;
            unsafe { asm!("in eax, dx", out("eax") val, in("dx") port, options(nomem, nostack, preserves_flags)); }
            val
        };
        
        // 0x80000000 | (bus << 16) | (dev << 11) | (func << 8) | offset
        // Bus 0, Dev 2, Func 0. 0x80001000.
        outl(0xCF8, 0x80001010);
        let bar0 = inl(0xCFC) & 0xFFFFFFF0;
        if bar0 == 0 || bar0 == 0xFFFFFFF0 {
            // Sentinel, NOT zeros: an all-zero trace is also a legitimate all-dark reading,
            // so a failed BAR0 read must be unmistakable in the serial table.
            return [0xBAD0BA20; 11];
        }
        
        let read_mmio = |offset: u32| -> u32 {
            unsafe { core::ptr::read_volatile((bar0 as usize + offset as usize) as *const u32) }
        };
        
        [
            read_mmio(0x70008), // PIPEACONF
            read_mmio(0x71008), // PIPEBCONF
            read_mmio(0x72008), // PIPECCONF
            read_mmio(0x70180), // DSPACNTR
            read_mmio(0x71180), // DSPBCNTR
            read_mmio(0x72180), // DSPCCNTR
            read_mmio(0x7019C), // DSPASURF
            read_mmio(0x64000), // DP_A
            read_mmio(0x61200), // PP_STATUS
            read_mmio(0x61204), // PP_CONTROL
            read_mmio(0x06014), // DPLL_A_CTRL
        ]
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        [0; 11]
    }
}

#[cfg(feature = "unaos_ivb")]
unsafe fn read_gmux_trace() -> [u32; 4] {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use core::arch::asm;
        
        let outb = |port: u16, val: u8| {
            unsafe { asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags)); }
        };
        let inb = |port: u16| -> u8 {
            let mut val: u8;
            unsafe { asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack, preserves_flags)); }
            val
        };

        // Indexed read: index -> 0x7D0, value <- 0x7C2
        let index_read = |reg: u8| -> u32 {
            outb(0x7D0, reg);
            let val = inb(0x7C2);
            if val == 0 || val == 0xFF {
                0xBAD0BA20
            } else {
                val as u32
            }
        };

        // Classic read: value <- 0x700 + reg
        let pio_read = |port: u16| -> u32 {
            let val = inb(port);
            if val == 0 || val == 0xFF {
                0xBAD0BA20
            } else {
                val as u32
            }
        };

        [
            index_read(0x10), // Indexed SWITCH_DISPLAY
            index_read(0x50), // Indexed DISCRETE_POWER
            pio_read(0x710),  // Classic SWITCH_DISPLAY
            pio_read(0x750),  // Classic DISCRETE_POWER
        ]
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        [0; 4]
    }
}
