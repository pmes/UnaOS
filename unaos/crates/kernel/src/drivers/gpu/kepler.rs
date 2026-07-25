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

        // 6. Display Engine — read-only trace + optional takeover
        let mut kdisp_trace = [0u32; 7];
        
        // Milestone 1: Method-Mirror Backing-Store Beacon Test - Pre-Takeover Dump
        let mut mirror_hdr_pre = [0u32; 256];
        for (i, offset) in (0..=0x3FC).step_by(4).enumerate() {
            let val = mmio_read(bar0, 0x640000 + offset);
            mirror_hdr_pre[i] = val;
            serial_println!(":: kepler: mirror-hdr pre off={:03X} val={:08X} ::", offset, val);
        }
        serial_println!(":: kepler: mirror-hdr pre done rows=256 ::");

        let _fb_offset = crate::drivers::gpu::kepler_display::takeover_display(
            gpu, bar0, &mut vram_allocator, &mut kdisp_trace,
        );
        serial_println!(":: kdisp: landed trace [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
            kdisp_trace[0], kdisp_trace[1], kdisp_trace[2], kdisp_trace[3],
            kdisp_trace[4], kdisp_trace[5], kdisp_trace[6]);

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

                                    let chan_id = 1;

                                    // Setup Channel Instance Block
                                    unsafe {
                                        core::ptr::write_volatile((bar1 + inst_off + 0x08) as *mut u32, (userd_off & 0xFFFFFFFF) as u32);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x0C) as *mut u32, ((userd_off >> 32) as u32) | 0x80000000);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x10) as *mut u32, 0x0000face);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x30) as *mut u32, 0xfffff902);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x48) as *mut u32, (gpfifo_off & 0xFFFFFFFF) as u32);
                                        // limit2 = ORDER 9 (512 entries)
                                        core::ptr::write_volatile((bar1 + inst_off + 0x4C) as *mut u32, ((gpfifo_off >> 32) as u32) | (9 << 16));
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

                                    let chid_0 = 1;
                                    let chid_1 = 2;
                                    let chid_2 = 3;
                                    let entry_0 = chid_0;
                                    let entry_1 = chid_1 | (1 << 31);
                                    let entry_2 = (chid_2 << 1) | 1;

                                    // 1. Write Runlist VRAM FIRST
                                    unsafe {
                                        core::ptr::write_volatile((bar1 + runlist_off) as *mut u32, entry_0);
                                        core::ptr::write_volatile((bar1 + runlist_off + 4) as *mut u32, 0);
                                        core::ptr::write_volatile((bar1 + runlist_off + 8) as *mut u32, entry_1);
                                        core::ptr::write_volatile((bar1 + runlist_off + 12) as *mut u32, 0);
                                        core::ptr::write_volatile((bar1 + runlist_off + 16) as *mut u32, entry_2);
                                        core::ptr::write_volatile((bar1 + runlist_off + 20) as *mut u32, 0);
                                    }

                                    let _read_sched_status = |label: &str| {
                                        let err = mmio_read(bar0, 0x252c);
                                        let stat = mmio_read(bar0, 0x263c);
                                        let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                        serial_println!(":: kepler: sched-status {} err={:08X} ({}) stat={:08X} ::", label, err, err_str, stat);
                                    };

                                    // Milestone 1: Method-Mirror Backing-Store Beacon Test
                                    // Pass 0: Baseline dump
                                    let mut rows = 0;
                                    let mut diff_found = false;
                                    for (i, offset) in (0..=0x3FC).step_by(4).enumerate() {
                                        let val = mmio_read(bar0, 0x640000 + offset);
                                        let pre_val = mirror_hdr_pre[i];
                                        serial_println!(":: kepler: mirror-hdr pass0 off={:03X} val={:08X} ::", offset, val);
                                        if val != pre_val {
                                            serial_println!(":: kepler: latch-delta off={:03X} pre={:08X} post={:08X} ::", offset, pre_val, val);
                                            diff_found = true;
                                        }
                                        rows += 1;
                                    }
                                    serial_println!(":: kepler: mirror-hdr pass0 done rows={} ::", rows);
                                    if !diff_found {
                                        serial_println!(":: kepler: latch-delta none ::");
                                    }

                                    // Plant Beacons
                                    let pattern = [
                                        0xBEAC0001, 0xBEAC0002, 0xBEAC0003, 0xBEAC0004,
                                        0xBEAC0005, 0xBEAC0006, 0xBEAC0007, 0xBEAC0008,
                                    ];
                                    
                                    unsafe {
                                        // userd
                                        for (i, val) in pattern.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + userd_off + i * 4) as *mut u32, *val);
                                        }
                                        serial_println!(":: kepler: beacon planted at=userd off={:08X} ::", userd_off);
                                        
                                        // pushbuffer
                                        for (i, val) in pattern.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + pb_off + i * 4) as *mut u32, *val);
                                        }
                                        serial_println!(":: kepler: beacon planted at=pb off={:08X} ::", pb_off);
                                        
                                        // runlist
                                        for (i, val) in pattern.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + runlist_off + i * 4) as *mut u32, *val);
                                        }
                                        serial_println!(":: kepler: beacon planted at=runlist off={:08X} ::", runlist_off);
                                    }

                                    // Pass 1: Post-Plant Dump & Scan
                                    let mut rows_pass1 = 0;
                                    let mut beacons_seen = 0;
                                    for offset in (0..=0x3FC).step_by(4) {
                                        let val = mmio_read(bar0, 0x640000 + offset);
                                        serial_println!(":: kepler: mirror-hdr pass1 off={:03X} val={:08X} ::", offset, val);
                                        if val >= 0xBEAC0001 && val <= 0xBEAC0008 {
                                            serial_println!(":: kepler: beacon SEEN off={:03X} val={:08X} ::", offset, val);
                                            beacons_seen += 1;
                                        }
                                        rows_pass1 += 1;
                                    }
                                    serial_println!(":: kepler: mirror-hdr pass1 done rows={} ::", rows_pass1);
                                    if beacons_seen == 0 {
                                        serial_println!(":: kepler: beacon none-seen ::");
                                    }

                                    // Delay
                                    for _ in 0..2_000_000 { core::hint::spin_loop(); }

                                    // Pass 2: Volatility Re-Check
                                    let mut rows_pass2 = 0;
                                    for offset in (0..=0x3FC).step_by(4) {
                                        let val = mmio_read(bar0, 0x640000 + offset);
                                        serial_println!(":: kepler: mirror-hdr pass2 off={:03X} val={:08X} ::", offset, val);
                                        rows_pass2 += 1;
                                    }
                                    serial_println!(":: kepler: mirror-hdr pass2 done rows={} ::", rows_pass2);
                                    
                                    // M2: Disp-Era USERD Reconnaissance (Read-Only)
                                    let disp_base = 0x610000;
                                    let pdisplay_0 = mmio_read(bar0, disp_base);
                                    let pdisplay_1 = mmio_read(bar0, disp_base + 0x40);
                                    let evo_core = mmio_read(bar0, disp_base + 0x490);
                                    let evo_userd_ptr = mmio_read(bar0, disp_base + 0x494);
                                    serial_println!(":: kepler: disp-userd-recon pdisplay_0={:08X} +40={:08X} evo_0x490={:08X} evo_0x494={:08X} ::", pdisplay_0, pdisplay_1, evo_core, evo_userd_ptr);

                                    // Milestone 2: PGRAPH Falcon Reconnaissance (Pull 18 + Pull 19)
                                    let pmc_en_pre = mmio_read(bar0, regs::NV_PMC_ENABLE);
                                    serial_println!(":: kepler: pgraph-enable pre={:08X} ::", pmc_en_pre);

                                    mmio_write(bar0, regs::NV_PMC_ENABLE, pmc_en_pre | (1 << 12));
                                    let pmc_en_rb = mmio_read(bar0, regs::NV_PMC_ENABLE);
                                    serial_println!(":: kepler: pgraph-enable wrote={:08X} rb={:08X} ::", pmc_en_pre | (1 << 12), pmc_en_rb);

                                    if (pmc_en_rb & (1 << 12)) == 0 {
                                        serial_println!(":: kepler: pgraph-enable REFUSED ::");
                                    } else {
                                        for _ in 0..2_000_000 { core::hint::spin_loop(); }

                                        for pass in 0..2 {
                                            if pass == 1 {
                                                for _ in 0..2_000_000 { core::hint::spin_loop(); }
                                            }

                                            let cpuctl = mmio_read(bar0, 0x400100);
                                            let bootvec = mmio_read(bar0, 0x400104);
                                            serial_println!(":: kepler: falcon pass{} cpuctl={:08X} bootvec={:08X} ::", pass, cpuctl, bootvec);

                                            let mut falcon_rows = 0;
                                            for offset in (0..=0x1C).step_by(4) {
                                                let val = mmio_read(bar0, 0x400100 + offset);
                                                let abs = if val == 0xFFFFFFFF || val == 0xBAD0BA20 { " ABSENT?" } else { "" };
                                                serial_println!(":: kepler: falcon core off={:03X} val={:08X}{} ::", 0x100 + offset, val, abs);
                                                falcon_rows += 1;
                                            }
                                            serial_println!(":: kepler: falcon core done rows={} ::", falcon_rows);

                                            let imemc = mmio_read(bar0, 0x400180);
                                            let dmemc = mmio_read(bar0, 0x4001C0);
                                            let abs_i = if imemc == 0xFFFFFFFF || imemc == 0xBAD0BA20 { " ABSENT?" } else { "" };
                                            let abs_d = if dmemc == 0xFFFFFFFF || dmemc == 0xBAD0BA20 { " ABSENT?" } else { "" };
                                            serial_println!(":: kepler: falcon mem imemc={:08X}{} dmemc={:08X}{} ::", imemc, abs_i, dmemc, abs_d);

                                            let mut pgraph_rows = 0;
                                            for offset in (0..=0x7C).step_by(4) {
                                                let val = mmio_read(bar0, 0x400000 + offset);
                                                let abs = if val == 0xFFFFFFFF || val == 0xBAD0BA20 { " ABSENT?" } else { "" };
                                                serial_println!(":: kepler: pgraph stat off={:03X} val={:08X}{} ::", offset, val, abs);
                                                pgraph_rows += 1;
                                            }
                                            serial_println!(":: kepler: pgraph stat done rows={} ::", pgraph_rows);
                                        }
                                    }

                                    // --- Witness Rematch ---
                                    serial_println!(":: kepler: witness-rematch begin (pgraph on) ::");

                                    // 2. Bind and Enable PFIFO_CHAN for channel 1
                                    mmio_write(bar0, 0x800000 + (1 * 8), 0); 
                                    mmio_write(bar0, 0x800004 + (1 * 8), 0x00000400); 
                                    mmio_write(bar0, 0x800000 + (1 * 8), 0xC0000000 | ((inst_off as u32) >> 12)); 

                                    let err = mmio_read(bar0, 0x252c);
                                    let stat = mmio_read(bar0, 0x263c);
                                    let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                    serial_println!(":: kepler: sched-status post-init err={:08X} ({}) stat={:08X} ::", err, err_str, stat);

                                    let ch_1_0_pre = mmio_read(bar0, 0x800000 + (1 * 8));
                                    let ch_1_4_pre = mmio_read(bar0, 0x800004 + (1 * 8));
                                    serial_println!(":: kepler: PFIFO_CHAN[1] pre-submit: 00={:08X} 04={:08X} ::", ch_1_0_pre, ch_1_4_pre);
                                    
                                    // Witness check
                                    if (ch_1_0_pre & 0xC0000000) != 0xC0000000 {
                                        serial_println!(":: kepler: WITNESS FAILED - bits stripped. Restoring inst_off+0x0C ::");
                                        core::ptr::write_volatile((bar1 + inst_off + 0x0C) as *mut u32, (userd_off >> 32) as u32);
                                        // Re-test PFIFO_CHAN[1] to clear state
                                        mmio_write(bar0, 0x800000 + (1 * 8), 0);
                                        mmio_write(bar0, 0x800004 + (1 * 8), 0x00000400);
                                        mmio_write(bar0, 0x800000 + (1 * 8), 0xC0000000 | ((inst_off as u32) >> 12));
                                        
                                        let err = mmio_read(bar0, 0x252c);
                                        let stat = mmio_read(bar0, 0x263c);
                                        let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                        serial_println!(":: kepler: sched-status post-restore err={:08X} ({}) stat={:08X} ::", err, err_str, stat);
                                    } else {
                                        serial_println!(":: kepler: WITNESS PASSED - bits stuck! ::");
                                    }

                                    // 3. Submit Runlist
                                    mmio_write(bar0, 0x2270, (runlist_off as u32) >> 12); // target=0 (VRAM), addr
                                    mmio_write(bar0, 0x2274, 3); // LEN=3, ENG=0
                                    serial_println!("[NVIDIA] Configured Runlist and bound channel.");

                                    // Wait for PLAYLIST_RD to accept the runlist
                                    let mut pl_rd = 0;
                                    let mut pl_rd_len = 0;
                                    for _ in 0..100_000 {
                                        pl_rd = mmio_read(bar0, 0x2280);
                                        pl_rd_len = mmio_read(bar0, 0x2284);
                                        if pl_rd == ((runlist_off as u32) >> 12) && (pl_rd_len & 0xFFF) == 1 {
                                            break;
                                        }
                                    }
                                    serial_println!(":: kepler: post-bind playlist_rd={:08X} playlist_rd_len={:08X} ::", pl_rd, pl_rd_len);
                                    
                                    let err = mmio_read(bar0, 0x252c);
                                    let stat = mmio_read(bar0, 0x263c);
                                    let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                    serial_println!(":: kepler: sched-status post-submit err={:08X} ({}) stat={:08X} ::", err, err_str, stat);

                                    let ch_1_0_post = mmio_read(bar0, 0x800000 + (1 * 8));
                                    let ch_1_4_post = mmio_read(bar0, 0x800004 + (1 * 8));
                                    serial_println!(":: kepler: PFIFO_CHAN[1] post-submit: 00={:08X} 04={:08X} ::", ch_1_0_post, ch_1_4_post);

                                    // Discriminator readback
                                    for i in 0..3 {
                                        let pbdma_base_i = 0x40000 + (i * 0x2000);
                                        let ch = mmio_read(bar0, pbdma_base_i + 0x120);
                                        let chid_active = ch & 0xFFF;
                                        let is_active = (ch >> 13) & 1;
                                        serial_println!(":: kepler: DISCRIMINATOR pbdma{} ch={:08X} (CHID={} ACTIVE={}) ::", i, ch, chid_active, is_active);
                                    }

                                    let final_err = mmio_read(bar0, 0x252c);
                                    let final_stat = mmio_read(bar0, 0x263c);
                                    serial_println!(":: kepler: witness-rematch end err={:08X} stat={:08X} valid={:08X} ::", final_err, final_stat, ch_1_0_post);

                                    // --- K-GPU-4 Milestone 1: Falcon IMEM/DMEM Probe ---
                                    // 1. IMEM probe
                                    mmio_write(bar0, 0x400180, 1 << 24); // IMEMC offset=0, auto-increment
                                    let imemc_rb = mmio_read(bar0, 0x400180);
                                    serial_println!(":: kepler: falcon imemc wr=01000000 rb={:08X} ::", imemc_rb);
                                    
                                    mmio_write(bar0, 0x400184, 0xDEADBEEF);
                                    mmio_write(bar0, 0x400184, 0xCAFEF00D);
                                    mmio_write(bar0, 0x400184, 0x12345678);
                                    mmio_write(bar0, 0x400184, 0xA5A55A5A);
                                    
                                    mmio_write(bar0, 0x400180, 1 << 25); // reset offset, AINCR (bit25 = read auto-increment; bit24 only increments on writes)
                                    let imem_w0 = mmio_read(bar0, 0x400184);
                                    let imem_w1 = mmio_read(bar0, 0x400184);
                                    let imem_w2 = mmio_read(bar0, 0x400184);
                                    let imem_w3 = mmio_read(bar0, 0x400184);
                                    serial_println!(":: kepler: falcon imem rb w0={:08X} w1={:08X} w2={:08X} w3={:08X} ::", imem_w0, imem_w1, imem_w2, imem_w3);
                                    
                                    // 2. DMEM probe
                                    mmio_write(bar0, 0x4001C0, 1 << 24); // DMEMC offset=0, auto-increment
                                    let dmemc_rb = mmio_read(bar0, 0x4001C0);
                                    serial_println!(":: kepler: falcon dmemc wr=01000000 rb={:08X} ::", dmemc_rb);
                                    
                                    mmio_write(bar0, 0x4001C4, 0xDEADBEEF);
                                    mmio_write(bar0, 0x4001C4, 0xCAFEF00D);
                                    mmio_write(bar0, 0x4001C4, 0x12345678);
                                    mmio_write(bar0, 0x4001C4, 0xA5A55A5A);
                                    
                                    mmio_write(bar0, 0x4001C0, 1 << 25); // reset offset, AINCR (read auto-increment)
                                    let dmem_w0 = mmio_read(bar0, 0x4001C4);
                                    let dmem_w1 = mmio_read(bar0, 0x4001C4);
                                    let dmem_w2 = mmio_read(bar0, 0x4001C4);
                                    let dmem_w3 = mmio_read(bar0, 0x4001C4);
                                    serial_println!(":: kepler: falcon dmem rb w0={:08X} w1={:08X} w2={:08X} w3={:08X} ::", dmem_w0, dmem_w1, dmem_w2, dmem_w3);
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
    pub fn push(&mut self, _word: u32) {
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

pub unsafe fn mmio_read(base: usize, offset: usize) -> u32 {
    core::ptr::read_volatile((base + offset) as *const u32)
}

pub unsafe fn mmio_write(base: usize, offset: usize, val: u32) {
    core::ptr::write_volatile((base + offset) as *mut u32, val)
}
