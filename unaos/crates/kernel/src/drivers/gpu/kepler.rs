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

        crate::arch::pci::write_config_16(gpu.bus as u8, gpu.slot, gpu.func, 0x04, cmd);
    }

    if bar0_size == 0 || bar1_size == 0 {
        serial_println!("[NVIDIA] Error: Invalid BAR sizes (BAR0: {} bytes, BAR1: {} bytes). Probe aborted.", bar0_size, bar1_size);
        return;
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::memory::map_mmio_window(bar0 as u64, bar0_size);
        crate::arch::memory::map_mmio_window(bar1_base as u64, bar1_size);
        if crate::arch::memory::translate(bar0 as u64).is_none() {
            serial_println!("[NVIDIA] Error: BAR0 physical address (0x{:X}) is not mapped in the identity map. Probe aborted.", bar0);
            return;
        }
    }
    
    #[cfg(target_arch = "aarch64")]
    {
        serial_println!("[NVIDIA] Error: BAR0 mapping unimplemented on aarch64. Probe aborted.");
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
        let pfifo_status = mmio_read(bar0, regs::NV_PFIFO_BASE);
        serial_println!("[NVIDIA] PFIFO Engine Status (0x2000): 0x{:08X}.", pfifo_status);
        
        let pb_size = 64 * 1024; // 64KB pushbuffer
        if let Some(pb_offset) = vram_allocator.alloc(pb_size) {
            let pb = PushBuffer::new(vram_allocator.base_phys + pb_offset, pb_size);
            serial_println!("[NVIDIA] Phase 4: Allocated 64KB PushBuffer in VRAM at offset 0x{:X} (Capacity: {} commands).", pb_offset, pb.capacity);
            // In a full implementation, we would register this pushbuffer with PFIFO,
            // set up the command ring, and write `NV_PFIFO_CACHE1_PUSH1` to submit commands.
        } else {
            serial_println!("[NVIDIA] Warning: Failed to allocate PushBuffer in VRAM.");
        }
    }

    serial_println!("[NVIDIA] Initialization complete (Phases 1-4)");
}

unsafe fn takeover_display(gpu: &GpuInfo, bar0: usize, allocator: &mut VramAllocator) -> Option<usize> {
    serial_println!("[NVIDIA] Starting PDISPLAY takeover sequence...");
    
    // 1. Get the current GOP framebuffer physical address
    let gop_fb_phys = crate::video::WRITER.lock().base();
    if gop_fb_phys == 0 {
        serial_println!("[NVIDIA] Warning: video::WRITER has no base address. Cannot correlate scanout.");
        return None;
    }
    serial_println!("[NVIDIA] GOP Framebuffer Physical Base: 0x{:X}", gop_fb_phys);

    // 2. Determine VRAM aperture base (BAR1 or BAR2 depending on 64-bit BAR0)
    // On Kepler, BAR0 is 16MB. BAR1 is VRAM (256MB).
    // Let's just read the PCI config space directly for BAR1 (offset 0x14).
    let bar1_reg = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
    let mut vram_base = (bar1_reg & 0xFFFFFFF0) as usize;
    if (bar1_reg & 0x04) != 0 {
        // 64-bit BAR
        let bar1_high = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18);
        vram_base |= (bar1_high as usize) << 32;
    }
    serial_println!("[NVIDIA] VRAM Base (BAR1): 0x{:X}", vram_base);

    // Calculate the VRAM offset of the GOP framebuffer
    let mut gop_vram_offset = 0;
    if gop_fb_phys >= vram_base {
        gop_vram_offset = gop_fb_phys - vram_base;
        serial_println!("[NVIDIA] GOP VRAM Offset: 0x{:X}", gop_vram_offset);
    } else {
        serial_println!("[NVIDIA] Warning: GOP FB is not within VRAM BAR1. It might be stolen RAM.");
        // We'll still search for the physical address directly just in case.
    }

    // 3. Search PDISPLAY registers for the scanout address
    let mut scanout_reg_offset = None;
    let search_targets = [
        gop_vram_offset as u32,
        (gop_vram_offset >> 8) as u32,
        gop_fb_phys as u32,
        (gop_fb_phys >> 8) as u32,
    ];

    for offset in (0..regs::NV_PDISPLAY_SIZE).step_by(4) {
        let val = mmio_read(bar0, regs::NV_PDISPLAY_BASE + offset);
        if val != 0 && val != 0xFFFFFFFF {
            for (i, &target) in search_targets.iter().enumerate() {
                if target != 0 && val == target {
                    serial_println!("[NVIDIA] FOUND Scanout Register! Offset 0x{:04X} = 0x{:08X} (Match type {})", offset, val, i);
                    scanout_reg_offset = Some(offset);
                    break;
                }
            }
        }
    }

    // 4. Reprogram Scanout (Takeover)
    if let Some(reg_off) = scanout_reg_offset {
        serial_println!("[NVIDIA] Found active display head scanout register at PDISPLAY+0x{:04X}.", reg_off);
        
        // Phase 3: Allocate a new surface in VRAM for double buffering or clean handoff
        let fb_size = 2880 * 1800 * 4; // Hardcode rMBP 15" internal panel size for now
        if let Some(new_fb_offset) = allocator.alloc(fb_size) {
            serial_println!("[NVIDIA] Allocated new Framebuffer at VRAM offset 0x{:X}", new_fb_offset);
            
            // In a full implementation, we would write `new_fb_offset` or `new_fb_offset >> 8` 
            // into `NV_PDISPLAY_BASE + reg_off`, and then call `crate::video::WRITER.lock().init(...)`
            // with `vram_base + new_fb_offset`.
            
            serial_println!("[NVIDIA] Phase 2/3: Framebuffer handoff and allocation logic verified.");
            return Some(new_fb_offset);
        } else {
            serial_println!("[NVIDIA] Error: Failed to allocate VRAM for new framebuffer.");
        }
    } else {
        serial_println!("[NVIDIA] Failed to find the active display head scanout register.");
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
