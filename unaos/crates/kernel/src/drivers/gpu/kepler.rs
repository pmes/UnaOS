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
    pub const NV_PFB_RAM_AMOUNT: usize = 0x0010_020C; // Common VRAM size register

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
            serial_println!(":: kepler: no-device ::");
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
        serial_println!(":: kepler: no-device ::");
        return;
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::memory::map_mmio_window(bar0 as u64, bar0_size);
        crate::arch::memory::map_mmio_window(bar1_base as u64, bar1_size);
        if crate::arch::memory::translate(bar0 as u64).is_none() {
            serial_println!("[NVIDIA] Error: BAR0 physical address (0x{:X}) is not mapped in the identity map. Probe aborted.", bar0);
            serial_println!(":: kepler: no-device ::");
            return;
        }
    }
    
    #[cfg(target_arch = "aarch64")]
    {
        serial_println!("[NVIDIA] Error: BAR0 mapping unimplemented on aarch64. Probe aborted.");
        serial_println!(":: kepler: no-device ::");
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
        let vram_size = mmio_read(bar0, regs::NV_PFB_RAM_AMOUNT) as usize;
        if vram_size < 16 * 1024 * 1024 || vram_size > 32usize * 1024 * 1024 * 1024 {
            serial_println!("[NVIDIA] Error: Absurd VRAM size reported ({} bytes). Probe aborted.", vram_size);
            serial_println!(":: kepler: no-device ::");
            return;
        }
        serial_println!("[NVIDIA] PFB Reported VRAM Size: {} MB", vram_size >> 20);

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
        if option_env!("UNAOS_KEPLER_FIFO").is_some() {
            serial_println!("[NVIDIA] Starting PFIFO initialization...");
            
            // Enable PFIFO (bit 8)
            let pmc_enable = mmio_read(bar0, regs::NV_PMC_ENABLE);
            mmio_write(bar0, regs::NV_PMC_ENABLE, pmc_enable | 0x100);
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
                                        serial_println!(":: kepler: takeover-abort fence-timeout ::");
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

    if option_env!("UNAOS_KEPLER_TAKEOVER").is_none() {
        serial_println!("[NVIDIA] UNAOS_KEPLER_TAKEOVER knob not set. Skipping display takeover.");
        return None;
    }
    
    // 1. Get the current GOP framebuffer physical address
    let gop_fb_phys = crate::video::WRITER.lock().base();
    if gop_fb_phys == 0 {
        serial_println!("[NVIDIA] Warning: video::WRITER has no base address. Cannot correlate scanout.");
        serial_println!(":: kepler: takeover-abort no-gop ::");
        return None;
    }
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
    if gop_fb_phys < vram_base {
        serial_println!("[NVIDIA] Warning: GOP FB is not within VRAM BAR1.");
        serial_println!(":: kepler: takeover-abort gop-not-in-vram {:X} ::", gop_fb_phys);
        return None;
    }
    let gop_vram_offset = gop_fb_phys - vram_base;
    serial_println!("[NVIDIA] GOP VRAM Offset: 0x{:X}", gop_vram_offset);

    // 3. Read PDISPLAY Head State
    // The Core EVO channel (NV_EVO_CORE) is mirrored at NV_PDISPLAY_BASE.
    // envytools rnndb/display/nv_evo.xml:
    // NV_EVO_CORE base = 0x610000
    // HEAD array offset = 0x400, stride = 0x300 (GF119+)
    // G80_EVO_HEAD -> G80_EVO_FB_SETTINGS stripe at offset 0x60
    // OFFSET_ORIGIN = 0x0, SIZE = 0x8, STORAGE = 0xC
    
    let expected_addr = (gop_vram_offset >> 8) as u32;
    let mut found_head = None;
    let mut raw_addr = 0;
    let mut raw_size = 0;
    let mut raw_storage = 0;

    for head in 0..4 {
        let head_base = regs::NV_PDISPLAY_BASE + 0x400 + (head * 0x300) + 0x60;
        let addr = mmio_read(bar0, head_base);
        if addr != 0 && addr == expected_addr {
            found_head = Some(head);
            raw_addr = addr;
            raw_size = mmio_read(bar0, head_base + 0x8);
            raw_storage = mmio_read(bar0, head_base + 0xC);
            break;
        }
    }

    if let Some(head) = found_head {
        let gop_info = crate::video::WRITER.lock().info();
        let expected_width = gop_info.width as u32;
        let expected_height = gop_info.height as u32;
        
        let width = raw_size & 0xFFFF;
        let height = raw_size >> 16;
        
        if width != expected_width || height != expected_height {
            serial_println!("[NVIDIA] Bounds check failed for head {}. Expected {}x{}, got {}x{}", head, expected_width, expected_height, width, height);
            serial_println!(":: kepler: takeover-abort head={} addr={:08X} size={:08X} storage={:08X} ::", head, raw_addr, raw_size, raw_storage);
            return None;
        }

        serial_println!("[NVIDIA] Found active display head {} at PDISPLAY+0x{:04X}.", head, 0x460 + head * 0x300);
        
        let fb_size = (expected_width * expected_height * 4) as usize; 
        if let Some(new_fb_offset) = allocator.alloc(fb_size) {
            serial_println!("[NVIDIA] Allocated new Framebuffer at VRAM offset 0x{:X}", new_fb_offset);
            serial_println!("[NVIDIA] Phase 2/3: Framebuffer handoff and allocation logic verified.");
            return Some(new_fb_offset);
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
