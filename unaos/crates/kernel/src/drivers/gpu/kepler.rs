use super::detect::GpuInfo;
use crate::drivers::pci::PciScanner;

pub mod regs {
    // PMC — Master Control
    pub const NV_PMC_BOOT_0: usize = 0x0000_0000;   // chip ID, stepping
    pub const NV_PMC_BOOT_1: usize = 0x0000_0004;   // revision
    pub const NV_PMC_ENABLE: usize = 0x0000_0200;   // engine enable mask
    pub const NV_PMC_INTR_0: usize = 0x0000_0100;   // interrupt status
    pub const NV_PMC_INTR_EN: usize = 0x0000_0140;  // interrupt enable

    // PBUS — Bus Control
    pub const NV_PBUS_PCI_NV_0: usize = 0x0000_1800; // PCI vendor/device mirror
    pub const NV_PBUS_PCI_NV_1: usize = 0x0000_1804; // PCI status/command mirror

    // PFB — Framebuffer (VRAM) Controller
    pub const NV_PFB_BASE: usize = 0x0010_0000;
    pub const NV_PFB_RAM_AMOUNT: usize = 0x0010_F20C; // Kepler VRAM size register in MB (PBFB_BROADCAST + MEM_AMOUNT)

    // PFIFO — Command Submission / Pushbuffer
    pub const NV_PFIFO_BASE: usize = 0x0000_2000;

    // PGRAPH — 2D/3D Graphics Engine
    pub const NV_PGRAPH_BASE: usize = 0x0040_0000;

    // PDISPLAY — Display Engine
    pub const NV_PDISPLAY_BASE: usize = 0x0061_0000;
    pub const NV_PDISPLAY_SIZE: usize = 0x0001_0000; // Scan 64KB of display engine regs
}

pub fn init(gpu: &GpuInfo) {
    serial_println!("[NVIDIA] Initializing Kepler GPU at BDF {}:{}:{}", gpu.bus, gpu.slot, gpu.func);

    // 1. Enable Bus Master and Memory Space
    PciScanner::enable_bus_master(gpu.bus, gpu.slot, gpu.func);

    let bar0 = gpu.bar0_phys as usize;

    let mut bar0_size = 0;
    let mut bar1_base = 0;
    let mut bar1_size = 0;

    unsafe {
        let cmd = crate::arch::pci::read_config_16(gpu.bus as u8, gpu.slot, gpu.func, 0x04);
        crate::arch::pci::write_config_16(gpu.bus as u8, gpu.slot, gpu.func, 0x04, cmd & !0x02);

        let bar0_orig = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x10);
        crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x10, 0xFFFFFFFF);
        let bar0_val = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x10);
        crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x10, bar0_orig);
        if bar0_val != 0 && bar0_val != 0xFFFFFFFF {
            bar0_size = (!(bar0_val & !0xF)).wrapping_add(1) as usize;
        }

        let bar1_orig_lo = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
        
        if (bar1_orig_lo & 0x1) != 0 {
            serial_println!("[NVIDIA] Error: BAR1 is I/O space. Probe aborted.");
            serial_println!(":: kepler: probe-abort bar0-unmapped ::");
            return;
        }

        let is_64bit = ((bar1_orig_lo >> 1) & 0x3) == 0x2;
        
        if is_64bit {
            let bar1_orig_hi = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18);
            bar1_base = (bar1_orig_lo & 0xFFFFFFF0) as usize | ((bar1_orig_hi as usize) << 32);

            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14, 0xFFFFFFFF);
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18, 0xFFFFFFFF);
            let bar1_val_lo = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
            let bar1_val_hi = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18);
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14, bar1_orig_lo);
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18, bar1_orig_hi);
            
            let bar1_val = (bar1_val_lo & 0xFFFFFFF0) as u64 | ((bar1_val_hi as u64) << 32);
            if bar1_val != 0 {
                bar1_size = (!bar1_val).wrapping_add(1) as usize;
            }
        } else {
            bar1_base = (bar1_orig_lo & 0xFFFFFFF0) as usize;
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14, 0xFFFFFFFF);
            let bar1_val_lo = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14, bar1_orig_lo);
            
            if bar1_val_lo != 0 && bar1_val_lo != 0xFFFFFFFF {
                bar1_size = (!(bar1_val_lo & 0xFFFFFFF0)).wrapping_add(1) as usize;
            }
        }

        crate::arch::pci::write_config_16(gpu.bus as u8, gpu.slot, gpu.func, 0x04, cmd);
    }

    if bar0_size == 0 || bar1_size == 0 {
        serial_println!("[NVIDIA] Error: Invalid BAR sizes (BAR0: {} bytes, BAR1: {} bytes). Probe aborted.", bar0_size, bar1_size);
        serial_println!(":: kepler: probe-abort bar1-unmapped ::");
        return;
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::memory::map_mmio_window(bar0 as u64, bar0_size);
        crate::arch::memory::map_mmio_window(bar1_base as u64, bar1_size);
        if crate::arch::memory::translate(bar0 as u64).is_none() {
            serial_println!("[NVIDIA] Error: BAR0 physical address (0x{:X}) is not mapped in the identity map. Probe aborted.", bar0);
            serial_println!(":: kepler: probe-abort bar1-not-64bit ::");
            return;
        }
    }
    
    #[cfg(target_arch = "aarch64")]
    {
        serial_println!("[NVIDIA] Error: BAR0 mapping unimplemented on aarch64. Probe aborted.");
        serial_println!(":: kepler: probe-abort bar0-unmapped ::");
        return;
    }


    unsafe {
        // 2. Read NV_PMC_BOOT_0 to identify chip
        let boot_0 = mmio_read(bar0, regs::NV_PMC_BOOT_0);
        let chipset = (boot_0 >> 20) & 0xFF;
        let major = (boot_0 >> 16) & 0xF;
        let minor = boot_0 & 0xFFFF;
        serial_println!("[NVIDIA] Chipset: 0x{:02X}, Stepping: {}.{}", chipset, major, minor);

        if chipset != 0xE7 {
            serial_println!("[NVIDIA] Warning: Expected GK107 (0xE7), found 0x{:02X}", chipset);
        }

        // 3. Verify POST
        let pmc_enable = mmio_read(bar0, regs::NV_PMC_ENABLE);
        serial_println!("[NVIDIA] PMC Enable: 0x{:08X}", pmc_enable);
        if pmc_enable == 0 {
            serial_println!("[NVIDIA] Warning: GPU does not appear to be POST'd (PMC_ENABLE is 0)");
        }

        // 4. Disable Interrupts
        mmio_write(bar0, regs::NV_PMC_INTR_EN, 0);
        serial_println!("[NVIDIA] Disabled interrupts via PMC_INTR_EN");

        // 5. VRAM Detection & Initialization
        let vram_size_mb = mmio_read(bar0, regs::NV_PFB_RAM_AMOUNT) as usize;
        let vram_size = vram_size_mb * 1024 * 1024;
        
        let is_power_of_two = vram_size_mb.is_power_of_two();
        let is_3n_over_4 = (vram_size_mb % 3 == 0) && (vram_size_mb / 3 * 4).is_power_of_two();

        if vram_size < 16 * 1024 * 1024 || vram_size > 32usize * 1024 * 1024 * 1024 || (!is_power_of_two && !is_3n_over_4) {
            serial_println!("[NVIDIA] Error: Absurd VRAM size reported ({} MB). Probe aborted.", vram_size_mb);
            serial_println!(":: kepler: probe-abort vram-size-invalid ::");
            return;
        }
        serial_println!("[NVIDIA] PFB Reported VRAM Size: {} MB", vram_size_mb);

        let mut vram_allocator = VramAllocator::new(bar1_base, bar1_size, vram_size);
        serial_println!("[NVIDIA] Initialized VRAM bump allocator. Total BAR1 visible: {} MB", vram_allocator.total_size >> 20);

        // 6. Display Engine Takeover
        let _fb_offset = takeover_display(gpu, bar0, &mut vram_allocator);

        // 7. PGRAPH 2D/3D Engine Init (Placeholder)
        // Kepler requires Falcon microcode to fully initialize PGRAPH.
        // We log its presence but leave it disabled to prevent hangs.
        let pgraph_status = mmio_read(bar0, regs::NV_PGRAPH_BASE);
        serial_println!("[NVIDIA] PGRAPH Engine Status (0x400000): 0x{:08X}. Requires firmware for full 2D/3D.", pgraph_status);

        // 8. Phase 4: 3D Foundation - PFIFO and Pushbuffer setup
        if cfg!(feature = "nvidia-kepler-fifo") {
            serial_println!("[NVIDIA] Starting PFIFO initialization...");
            
            // Enable PFIFO and SUBFIFO (PBDMA) in PMC
            let pmc_enable = mmio_read(bar0, regs::NV_PMC_ENABLE);
            mmio_write(bar0, regs::NV_PMC_ENABLE, pmc_enable | 0x100);
            
            // GK104 PBDMA enable (NV_PMC_SUBFIFO_ENABLE at 0x204 in pmc.xml)
            mmio_write(bar0, 0x000204, 0xFFFFFFFF);
            let pbdma_count_mask = mmio_read(bar0, 0x000204);
            let pbdma_count = pbdma_count_mask.count_ones();
            serial_println!(":: kepler: pbdma-count {} ::", pbdma_count);

            // Bind PBDMA 0 to Engine 0 (PGRAPH) by writing mask `1` to SUBFIFO_ENG_MASK[0]
            // According to gf100_pfifo.xml, SUBFIFO_ENG_MASK is at offset 0x390 relative to PFIFO (0x2000).
            let pfifo_base = 0x2000;
            mmio_write(bar0, pfifo_base + 0x390, 1 << 0);
            serial_println!(":: kepler: pbdma-eng-mask set ::");

            let check = mmio_read(bar0, regs::NV_PMC_ENABLE);
            serial_println!("[NVIDIA] NV_PMC_ENABLE after bit 8 set: 0x{:08X}", check);

            if let Some(inst_off) = vram_allocator.alloc(0x1000) {
                if let Some(gpfifo_off) = vram_allocator.alloc(0x1000) {
                    if let Some(userd_off) = vram_allocator.alloc(0x1000) {
                        if let Some(pb_off) = vram_allocator.alloc(64 * 1024) {
                            if let Some(runlist_off) = vram_allocator.alloc(0x1000) {
                                if let Some(fence_off) = vram_allocator.alloc(0x1000) {
                                    serial_println!("[NVIDIA] Allocated Channel Instance, GPFIFO, USERD, PushBuffer, Runlist, Fence.");

                                    let bar1 = vram_allocator.base_phys;
                                    
                                    // Zero memory
                                    for i in 0..(0x1000 / 4) {
                                        unsafe {
                                            core::ptr::write_volatile((bar1 + inst_off + i * 4) as *mut u32, 0);
                                            core::ptr::write_volatile((bar1 + gpfifo_off + i * 4) as *mut u32, 0);
                                            core::ptr::write_volatile((bar1 + userd_off + i * 4) as *mut u32, 0);
                                            core::ptr::write_volatile((bar1 + runlist_off + i * 4) as *mut u32, 0);
                                            core::ptr::write_volatile((bar1 + fence_off + i * 4) as *mut u32, 0);
                                        }
                                    }

                                    let chan_id = 0;

                                    // Setup Channel Instance Block
                                    unsafe {
                                        core::ptr::write_volatile((bar1 + inst_off + 0x08) as *mut u32, (userd_off & 0xFFFFFFFF) as u32);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x0C) as *mut u32, (userd_off >> 32) as u32);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x10) as *mut u32, 0x0000face);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x30) as *mut u32, 0xfffff902);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x48) as *mut u32, (gpfifo_off & 0xFFFFFFFF) as u32);
                                        // limit2 = (0x1000 / 8) - 1 = 511
                                        core::ptr::write_volatile((bar1 + inst_off + 0x4C) as *mut u32, ((gpfifo_off >> 32) as u32) | (511 << 16));
                                        core::ptr::write_volatile((bar1 + inst_off + 0x84) as *mut u32, 0x20400000);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x94) as *mut u32, 0x30000000); // VRAM devm=0
                                        core::ptr::write_volatile((bar1 + inst_off + 0x9C) as *mut u32, 0x00000100);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xAC) as *mut u32, 0x0000001f);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xE4) as *mut u32, 0x00000000);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xE8) as *mut u32, chan_id);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xB8) as *mut u32, 0xf8000000);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xF8) as *mut u32, 0x10003080); // 0x002310
                                        core::ptr::write_volatile((bar1 + inst_off + 0xFC) as *mut u32, 0x10000010); // 0x002350
                                    }

                                    // Witness instance block raws
                                    let ib_08 = unsafe { core::ptr::read_volatile((bar1 + inst_off + 0x08) as *const u32) };
                                    let ib_0c = unsafe { core::ptr::read_volatile((bar1 + inst_off + 0x0C) as *const u32) };
                                    let ib_48 = unsafe { core::ptr::read_volatile((bar1 + inst_off + 0x48) as *const u32) };
                                    let ib_4c = unsafe { core::ptr::read_volatile((bar1 + inst_off + 0x4C) as *const u32) };
                                    serial_println!(":: kepler: inst-raw 08={:08X} 0C={:08X} 48={:08X} 4C={:08X} ::", ib_08, ib_0c, ib_48, ib_4c);

                                    // Bind Channel to PFIFO_CHAN (GK104)
                                    // inst_off >> 12 | 0x80000000
                                    mmio_write(bar0, 0x800000 + (chan_id as usize * 8), 0x80000000 | ((inst_off as u32) >> 12));
                                    // Enable Channel
                                    mmio_write(bar0, 0x800004 + (chan_id as usize * 8), 0x00000400);

                                    // Add to Runlist
                                    unsafe {
                                        core::ptr::write_volatile((bar1 + runlist_off) as *mut u32, chan_id);
                                        core::ptr::write_volatile((bar1 + runlist_off + 4) as *mut u32, 0);
                                    }
                                    mmio_write(bar0, 0x2270, (runlist_off as u32) >> 12); // target=0 (VRAM), addr
                                    mmio_write(bar0, 0x2274, 1); // count=1, runl=0
                                    serial_println!("[NVIDIA] Configured Runlist and bound channel.");

                                    // Write Pushbuffer payload (A06F GPFIFO class host semaphore release)
                                    let pb_base = bar1 + pb_off;
                                    let fence_val = 0xdeadbeef;
                                    unsafe {
                                        // SetObject (method 0x0000), class A06F
                                        core::ptr::write_volatile((pb_base + 0) as *mut u32, 0x20010000); // INCR, 1 words, MTHD 0x00
                                        core::ptr::write_volatile((pb_base + 4) as *mut u32, 0x0000A06F);
                                        // Host semaphore release: INCR, 4 words, MTHD 0x10 (SEMAPHOREA)
                                        core::ptr::write_volatile((pb_base + 8) as *mut u32, 0x20040004);
                                        core::ptr::write_volatile((pb_base + 12) as *mut u32, (fence_off >> 32) as u32);
                                        core::ptr::write_volatile((pb_base + 16) as *mut u32, (fence_off & 0xFFFFFFFF) as u32);
                                        core::ptr::write_volatile((pb_base + 20) as *mut u32, fence_val);
                                        core::ptr::write_volatile((pb_base + 24) as *mut u32, 0x2); // RELEASE (2)
                                    }

                                    // Write GPFIFO entry (1 entry pointing to the pushbuffer)
                                    let gpfifo_base = bar1 + gpfifo_off;
                                    unsafe {
                                        core::ptr::write_volatile((gpfifo_base + 0) as *mut u32, (pb_off & 0xFFFFFFFF) as u32);
                                        // len in bytes / 4 = 7 words. 7 << 10. Opcode 2 (0x20000000)
                                        core::ptr::write_volatile((gpfifo_base + 4) as *mut u32, ((pb_off >> 32) as u32) | (7 << 10) | (2 << 28));
                                    }

                                    serial_println!("[NVIDIA] Ringing GP_PUT doorbell...");
                                    // Submit via GP_PUT to USERD (offset 0x90)
                                    unsafe {
                                        core::ptr::write_volatile((bar1 + userd_off + 0x90) as *mut u32, 1); // increment GP_PUT to 1
                                    }

                                    let gp_get = unsafe { core::ptr::read_volatile((bar1 + userd_off + 0x8c) as *const u32) };
                                    let gp_put = unsafe { core::ptr::read_volatile((bar1 + userd_off + 0x90) as *const u32) };
                                    serial_println!(":: kepler: fifo-layout userd={:X} fence={:X} gp={}/{} ::", userd_off, fence_off, gp_put, gp_get);

                                    // Poll VRAM for fence value
                                    serial_println!("[NVIDIA] Polling for fence value 0x{:08X} at VRAM offset 0x{:X}", fence_val, fence_off);
                                    let mut found = false;
                                    for _ in 0..10_000_000 {
                                        let val = unsafe { core::ptr::read_volatile((bar1 + fence_off) as *const u32) };
                                        if val == fence_val {
                                            found = true;
                                            break;
                                        }
                                    }

                                    if found {
                                        serial_println!(":: kepler: fence {:08X} ::", fence_val);
                                    } else {
                                        let gp_get = unsafe { core::ptr::read_volatile((bar1 + userd_off + 0x8c) as *const u32) };
                                        let ch_stat = mmio_read(bar0, 0x800004 + (chan_id as usize * 8));
                                        
                                        let pbdma_base = 0x40000; // PSUBFIFO base
                                        let pbdma_stat = mmio_read(bar0, pbdma_base + 0x108); // PSUBFIFO INTR (gf100_pfifo.xml)
                                        
                                        if pbdma_stat == 0 || (pbdma_stat & 0xFFF00000) == 0xBAD00000 {
                                            serial_println!(":: kepler: bad-read pbdma {:X} {:08X} ::", pbdma_base + 0x108, pbdma_stat);
                                        }

                                        let playlist_rd = mmio_read(bar0, 0x2280); // PLAYLIST_RD (gf100_pfifo.xml)
                                        let playlist_rd_len = mmio_read(bar0, 0x2284);
                                        
                                        serial_println!(":: kepler: fifo-front pbdma_stat={:08X} playlist_rd={:08X} playlist_rd_len={:08X} ::", 
                                            pbdma_stat, playlist_rd, playlist_rd_len);
                                            
                                        serial_println!(":: kepler: takeover-abort fence-timeout gp_get={} ch_stat={:08X} (ENABLED={} UNK24_RO={} UNK28_RO={}) ::", 
                                            gp_get, ch_stat, ch_stat & 1, (ch_stat >> 24) & 7, (ch_stat >> 28) & 1);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    serial_println!("[NVIDIA] Initialization complete (Phases 1-4)");
}

unsafe fn takeover_display(gpu: &GpuInfo, bar0: usize, allocator: &mut VramAllocator) -> Option<usize> {
    serial_println!("[NVIDIA] Starting PDISPLAY takeover sequence...");

    if !cfg!(feature = "nvidia-kepler-takeover") {
        serial_println!("[NVIDIA] UNAOS_KEPLER_TAKEOVER feature not set. Skipping display takeover.");
        return None;
    }
    
    // 1. Get current framebuffer base address
    let gop_fb_phys = match crate::video::fbcon::current_base() {
        Some(base) => base,
        None => {
            serial_println!("[NVIDIA] Warning: video::fbcon has no base address.");
            serial_println!(":: kepler: takeover-abort no-gop ::");
            return None;
        }
    };
    serial_println!("[NVIDIA] GOP Framebuffer Physical Base: 0x{:X}", gop_fb_phys);

    // 2. Determine VRAM aperture base (BAR1 or BAR2 depending on 64-bit BAR0)
    let bar1_reg = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
    let mut vram_base = (bar1_reg & 0xFFFFFFF0) as usize;
    if (bar1_reg & 0x04) != 0 {
        let bar1_high = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18);
        vram_base |= (bar1_high as usize) << 32;
    }
    serial_println!("[NVIDIA] VRAM Base (BAR1): 0x{:X}", vram_base);

    // Calculate the VRAM offset of the GOP framebuffer
    if gop_fb_phys < vram_base as u64 {
        serial_println!("[NVIDIA] Warning: GOP FB is not within VRAM BAR1.");
        serial_println!(":: kepler: takeover-abort gop-not-in-vram {:X} ::", gop_fb_phys);
        return None;
    }
    let gop_vram_offset = (gop_fb_phys - vram_base as u64) as usize;
    serial_println!("[NVIDIA] GOP VRAM Offset: 0x{:X}", gop_vram_offset);

    // 3. Read PDISPLAY Head State
    // The Core EVO channel (NV_EVO_CORE) is mirrored at NV_PDISPLAY_BASE.
    // envytools rnndb/display/nv_evo.xml:
    // NV_EVO_CORE base = 0x610000
    // HEAD array offset = 0x400, stride = 0x300 (GF119+)
    // G80_EVO_HEAD -> G80_EVO_FB_SETTINGS stripe at offset 0x60
    // OFFSET_ORIGIN = 0x0, SIZE = 0x8, STORAGE = 0xC
    
    let expected_addr = (gop_vram_offset >> 8) as u32;
    let expected_phys = (gop_fb_phys >> 8) as u32;
    let mut found_head = None;
    let mut raw_addr = 0;
    let mut raw_size = 0;
    let mut raw_storage = 0;

    for head in 0..4 {
        // Candidate 1: NV_EVO_CORE (may be zero if EFI bypassed EVO)
        let head_evo = regs::NV_PDISPLAY_BASE + 0x400 + (head * 0x300) + 0x60;
        let addr_evo = mmio_read(bar0, head_evo + 0x0); // OFFSET_ORIGIN
        
        // Candidate 2: Direct CRTC (HEAD_VAL)
        let head_crtc = regs::NV_PDISPLAY_BASE + 0xA00 + (head * 0x540) + 0x128;
        let addr_crtc = mmio_read(bar0, head_crtc); // FB_POS

        serial_println!(":: kepler: head-raw head={} evo={:08X} crtc={:08X} ::", head, addr_evo, addr_crtc);

        let addr;
        let size;
        let storage;

        if addr_evo != 0 && (addr_evo & 0xFFF00000) != 0xBAD00000 {
            addr = addr_evo;
            size = mmio_read(bar0, head_evo + 0x8);
            storage = mmio_read(bar0, head_evo + 0xC);
            serial_println!(":: kepler: head-selected evo head={} ::", head);
        } else if addr_crtc != 0 && (addr_crtc & 0xFFF00000) != 0xBAD00000 {
            addr = addr_crtc;
            size = mmio_read(bar0, regs::NV_PDISPLAY_BASE + 0xA00 + (head * 0x540) + 0x118); // FB_SIZE
            storage = 0;
            serial_println!(":: kepler: head-selected crtc head={} ::", head);
        } else {
            serial_println!(":: kepler: bad-read head {} no valid candidates ::", head);
            continue;
        }

        if size == 0 || (size & 0xFFF00000) == 0xBAD00000 {
            serial_println!(":: kepler: bad-read head-size {:08X} ::", size);
            continue;
        }

        if addr == expected_addr || addr == expected_phys {
            found_head = Some(head);
            raw_addr = addr;
            raw_size = size;
            raw_storage = storage;
            // No break! Continue to dump all heads.
        }
    }

    if let Some(head) = found_head {
        let gop_info = crate::video::fbcon::current_info().unwrap();
        let expected_width = gop_info.width as u32;
        let expected_height = gop_info.height as u32;
        
        let width = raw_size & 0xFFFF;
        let height = raw_size >> 16;
        
        if width != expected_width || height != expected_height {
            serial_println!("[NVIDIA] Bounds check failed for head {}. Expected {}x{}, got {}x{}", head, expected_width, expected_height, width, height);
            serial_println!(":: kepler: takeover-abort head={} addr={:08X} size={:08X} storage={:08X} ::", head, raw_addr, raw_size, raw_storage);
            return None;
        }

        let bar1 = vram_base; // Already defined earlier
        let expected_height = (raw_size >> 16) & 0xFFFF;
        serial_println!("[NVIDIA] Head {} is active. Address: 0x{:X}, Size: {}x{}",
            found_head.unwrap(), raw_addr, expected_width, expected_height);
            
        // 4. Double buffer: Copy GOP contents to new surface before the flip
        let fb_size = (expected_width * expected_height * 4) as usize; 
        if let Some(new_fb_offset) = allocator.alloc(fb_size) {
            serial_println!("[NVIDIA] Allocated new Framebuffer at VRAM offset 0x{:X}", new_fb_offset);
            
            // Perform the double-buffer copy
            // The GOP surface is at `gop_vram_offset`, the new surface is at `new_fb_offset`
            // Map both to copy. Since we already have VRAM mapped linearly at `bar1`, we can do it directly.
            serial_println!("[NVIDIA] Copying GOP contents to new surface...");
            let src = (bar1 + gop_vram_offset) as *const u8;
            let dst = (bar1 + new_fb_offset) as *mut u8;
            core::ptr::copy_nonoverlapping(src, dst, fb_size);
            
            // 5. EVO core-channel push to flip the surface address
            // Allocate a 4KB pushbuffer for the EVO core channel
            if let Some(evo_pb_off) = allocator.alloc(4096) {
                serial_println!("[NVIDIA] Allocated EVO Pushbuffer at VRAM offset 0x{:X}", evo_pb_off);
                
                let evo_pb = (bar1 + evo_pb_off) as *mut u32;
                let head = found_head.unwrap();
                
                // Write the methods to the pushbuffer
                // Envytools: NV_EVO_CORE methods (size << 18) | method
                // OFFSET_ORIGIN (0x460 for head 0, +0x300 per head) (envytools: nv_evo.xml)
                let offset_origin_method = 0x400 + (head * 0x300) + 0x60; 
                // UPDATE (0x80) (envytools: nv_evo.xml)
                let update_method = 0x80;
                
                let new_addr = (new_fb_offset >> 8) as u32;
                
                // 1 dword to OFFSET_ORIGIN
                core::ptr::write_volatile(evo_pb.add(0), (1 << 18) | (offset_origin_method as u32));
                core::ptr::write_volatile(evo_pb.add(1), new_addr);
                
                // 1 dword to UPDATE
                core::ptr::write_volatile(evo_pb.add(2), (1 << 18) | (update_method as u32));
                core::ptr::write_volatile(evo_pb.add(3), 0x00000000);
                
                // Initialize the GF119+ EVO core channel (NV_PDISPLAY + 0x490)
                // Empirically probed on GK107, unverified against public docs.
                let core_ctrl = regs::NV_PDISPLAY_BASE + 0x490;
                
                // Read behind a bad-read guard so incorrect probing will self-identify
                let core_ctrl_val = mmio_read(bar0, core_ctrl);
                if core_ctrl_val == 0 || (core_ctrl_val & 0xFFF00000) == 0xBAD00000 {
                    serial_println!(":: kepler: bad-read core_ctrl {:X} {:08X} ::", core_ctrl, core_ctrl_val);
                    serial_println!(":: kepler: takeover-abort bad-core-ctrl ::");
                    return None;
                }
                
                // Deactivate channel first
                mmio_write(bar0, core_ctrl, core_ctrl_val & !0x10);
                mmio_write(bar0, core_ctrl, mmio_read(bar0, core_ctrl) & !0x03);
                
                // Delay briefly for inactivation (blind)
                for _ in 0..100000 { core::hint::spin_loop(); }
                
                // Bind our new pushbuffer
                let push_handle = (evo_pb_off >> 8) as u32 | 0x1; // VRAM target
                mmio_write(bar0, core_ctrl + 0x4, push_handle); // PB address
                mmio_write(bar0, core_ctrl + 0x8, 0x00010000);
                mmio_write(bar0, core_ctrl + 0xC, 0x00000001);
                
                // Set DISP_USER PUT to 0
                // DISP_USER array offset 0x640000 (g80_pdisplay.xml)
                mmio_write(bar0, 0x640000, 0);
                
                // Activate channel
                mmio_write(bar0, core_ctrl, mmio_read(bar0, core_ctrl) | 0x10);
                mmio_write(bar0, core_ctrl, mmio_read(bar0, core_ctrl) | 0x01000013);
                
                // Submit the pushbuffer by writing the new PUT value (4 dwords = 16 bytes)
                mmio_write(bar0, 0x640000, 16);
                
                // Wait for latch by reading back OFFSET_ORIGIN via MMIO shadow
                let head_base = regs::NV_PDISPLAY_BASE + 0x400 + (head * 0x300) + 0x60;
                let mut latched = false;
                for _ in 0..1000000 {
                    if mmio_read(bar0, head_base) == new_addr {
                        latched = true;
                        break;
                    }
                    core::hint::spin_loop();
                }
                
                if latched {
                    serial_println!("[NVIDIA] EVO flip successful! Head {} latched to 0x{:X}", head, new_addr);
                    return Some(new_fb_offset);
                } else {
                    serial_println!("[NVIDIA] EVO flip timeout! OFFSET_ORIGIN readback did not match.");
                    serial_println!(":: kepler: takeover-abort evo-flip-timeout ::");
                    return None;
                }
            }
        } else {
            serial_println!("[NVIDIA] Error: Failed to allocate VRAM for new framebuffer.");
        }
    } else {
        serial_println!("[NVIDIA] Failed to find the active display head scanout register.");
        serial_println!(":: kepler: takeover-abort no-match ::");
    }
    
    None
}

/// A simple bump allocator for VRAM (CPU-visible via BAR1).
pub struct VramAllocator {
    pub base_phys: usize,
    pub total_size: usize,
    pub current_offset: usize,
}

impl VramAllocator {
    pub fn new(bar1_base: usize, bar1_size: usize, vram_size: usize) -> Self {
        let total_size = if vram_size < bar1_size { vram_size } else { bar1_size };
        
        Self {
            base_phys: bar1_base,
            total_size,
            // Skip the first 32MB to avoid stepping on the firmware's GOP framebuffer
            current_offset: 32 * 1024 * 1024,
        }
    }

    /// Allocates a block of VRAM and returns the byte offset from the start of VRAM.
    pub fn alloc(&mut self, size: usize) -> Option<usize> {
        // Align to 4KB (page boundary)
        let aligned_offset = (self.current_offset + 0xFFF) & !0xFFF;
        
        if aligned_offset + size > self.total_size {
            return None; // Out of memory
        }
        
        self.current_offset = aligned_offset + size;
        Some(aligned_offset)
    }
}

/// A GPU Command PushBuffer for PFIFO command submission.
/// Commands are written as 32-bit words (methods and data) and the hardware fetches them via DMA.
pub struct PushBuffer {
    pub vram_phys: usize,
    pub size: usize,
    pub capacity: usize,
    pub write_ptr: usize,
}

impl PushBuffer {
    pub fn new(vram_phys: usize, size: usize) -> Self {
        Self {
            vram_phys,
            size,
            capacity: size / 4, // 32-bit command words
            write_ptr: 0,
        }
    }

    /// Appends a 32-bit command word to the pushbuffer.
    pub fn push(&mut self, word: u32) {
        if self.write_ptr < self.capacity {
            // In a real implementation, we would write to the CPU-mapped virtual address of VRAM.
            // unsafe { core::ptr::write_volatile((self.vram_virt + self.write_ptr * 4) as *mut u32, word); }
            self.write_ptr += 1;
        }
    }

    /// Generates an NVIDIA "Set Object" command for a specific GPU class (e.g., Kepler 2D or 3D engine).
    pub fn push_set_object(&mut self, class_id: u32) {
        // Method 0x0000 is typically SetObject
        // Format: [Size: 13 bits] [Subchannel: 3 bits] [Method: 16 bits]
        let header = (1 << 16) | (0 << 13) | 0x0000;
        self.push(header);
        self.push(class_id);
    }
}

unsafe fn mmio_read(base: usize, offset: usize) -> u32 {
    core::ptr::read_volatile((base + offset) as *const u32)
}

unsafe fn mmio_write(base: usize, offset: usize, val: u32) {
    core::ptr::write_volatile((base + offset) as *mut u32, val)
}
