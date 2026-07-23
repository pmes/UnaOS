use super::detect::GpuInfo;

pub mod regs {
    // Pipe Configuration
    pub const PIPEACONF: usize = 0x70008;
    pub const PIPEBCONF: usize = 0x71008;
    pub const PIPECCONF: usize = 0x72008;

    // Pipe Source (Width/Height)
    pub const PIPEASRC: usize = 0x6001C;
    pub const PIPEBSRC: usize = 0x6101C;
    pub const PIPECSRC: usize = 0x6201C;

    // Display Plane Control
    pub const DSPACNTR: usize = 0x70180;
    pub const DSPBCNTR: usize = 0x71180;
    pub const DSPCCNTR: usize = 0x72180;

    // Display Plane Surface Base
    pub const DSPASURF: usize = 0x7019C;
    pub const DSPBSURF: usize = 0x7119C;
    pub const DSPCSURF: usize = 0x7219C;

    // Display Plane Stride
    pub const DSPASTRIDE: usize = 0x70188;
    pub const DSPBSTRIDE: usize = 0x71188;
    pub const DSPCSTRIDE: usize = 0x72188;

    // Display Plane Panning Offsets
    pub const DSPALINOFF: usize = 0x70184;
    pub const DSPATILEOFF: usize = 0x701A4;
    pub const DSPBLINOFF: usize = 0x71184;
    pub const DSPBTILEOFF: usize = 0x711A4;
    pub const DSPCLINOFF: usize = 0x72184;
    pub const DSPCTILEOFF: usize = 0x721A4;

    // DP_A (eDP Port)
    pub const DP_A: usize = 0x64000;

    pub const PP_STATUS: usize = 0x61200;
    pub const PP_CONTROL: usize = 0x61204;
    pub const DPLL_A_CTRL: usize = 0x06014;

    // GTT Window (starts at 2MB offset in BAR0)
    pub const GTT_BASE: usize = 0x200000;
}

use core::sync::atomic::{AtomicBool, Ordering};

static PROBED: AtomicBool = AtomicBool::new(false);
static mut TRACE_0: [u32; 11] = [0; 11];
static mut TRACE_1: [u32; 11] = [0; 11];
static mut TRACE_2: [u32; 11] = [0; 11];
static mut GMUX_0: [u32; 6] = [0; 6];
static mut TRACES_VALID: bool = false;

pub fn set_boot_traces(t0: [u32; 11], t1: [u32; 11], t2: [u32; 11], g0: [u32; 6]) {
    unsafe {
        TRACE_0 = t0;
        TRACE_1 = t1;
        TRACE_2 = t2;
        GMUX_0 = g0;
        TRACES_VALID = true;
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn read_gmux_trace() -> [u32; 6] {
    use core::arch::asm;
    let outb = |port: u16, val: u8| {
        unsafe { asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags)); }
    };
    let inb = |port: u16| -> u8 {
        let mut val: u8;
        unsafe { asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack, preserves_flags)); }
        val
    };

    let wait_ready = || {
        let mut i = 200;
        let mut gwr = inb(0x7D4);
        while i > 0 && (gwr & 0x01) != 0 {
            inb(0x7D0);
            gwr = inb(0x7D4);
            for _ in 0..1000 { unsafe { asm!("pause", options(nomem, nostack, preserves_flags)); } }
            i -= 1;
        }
    };

    let wait_complete = || {
        let mut i = 200;
        let mut gwr = inb(0x7D4);
        while i > 0 && (gwr & 0x01) == 0 {
            gwr = inb(0x7D4);
            for _ in 0..1000 { unsafe { asm!("pause", options(nomem, nostack, preserves_flags)); } }
            i -= 1;
        }
        if (gwr & 0x01) != 0 {
            inb(0x7D0);
        }
    };

    let index_read = |reg: u8| -> u32 {
        wait_ready();
        outb(0x7D0, reg);
        wait_complete();
        let val = inb(0x7C2);
        val as u32
    };

    [
        index_read(0x04), // VERSION_MAJOR
        index_read(0x05), // VERSION_MINOR
        index_read(0x06), // VERSION_RELEASE
        index_read(0x10), // SWITCH_DISPLAY
        index_read(0x28), // SWITCH_DDC
        index_read(0x50), // DISCRETE_POWER
    ]
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn read_gmux_trace() -> [u32; 6] { [0; 6] }

pub fn init(gpu: &GpuInfo) {
    if PROBED.swap(true, Ordering::SeqCst) {
        return;
    }
    serial_println!("[Intel iGPU] Initializing Ivy Bridge GT2 at BDF {}:{}:{}", gpu.bus, gpu.slot, gpu.func);

    let bar0 = gpu.bar0_phys as usize;
    let bar0_size = gpu.bar0_size as usize;

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::memory::map_mmio_window(bar0 as u64, bar0_size);
        if crate::arch::memory::translate(bar0 as u64).is_none() {
            serial_println!("[Intel iGPU] Error: BAR0 physical address (0x{:X}) is not mapped. Probe aborted.", bar0);
            return;
        }
    }
    
    #[cfg(target_arch = "aarch64")]
    {
        serial_println!("[Intel iGPU] Error: BAR0 mapping unimplemented on aarch64. Probe aborted.");
        return;
    }

    // MILESTONE 1: Read-only instrumentation
    serial_println!("[Intel iGPU] Milestone 1: Read-only probe (instrumentation phase)");

    unsafe {
        if TRACES_VALID {
            let gmux3 = read_gmux_trace();
            serial_println!(":: igpu: TEARDOWN HUNT TRACE ::");
            serial_println!(":: igpu: Reg          | Point 0 (Boot)    | Point 1 (Pre-EBS) | Point 2 (Post-EBS)| Point 3 (Kernel) ::");
            let trace3 = [
                mmio_read(bar0, regs::PIPEACONF),
                mmio_read(bar0, regs::PIPEBCONF),
                mmio_read(bar0, regs::PIPECCONF),
                mmio_read(bar0, regs::DSPACNTR),
                mmio_read(bar0, regs::DSPBCNTR),
                mmio_read(bar0, regs::DSPCCNTR),
                mmio_read(bar0, regs::DSPASURF),
                mmio_read(bar0, regs::DP_A),
                mmio_read(bar0, regs::PP_STATUS),
                mmio_read(bar0, regs::PP_CONTROL),
                mmio_read(bar0, regs::DPLL_A_CTRL),
            ];
            let names = ["PIPEACONF", "PIPEBCONF", "PIPECCONF", "DSPACNTR", "DSPBCNTR", "DSPCCNTR", "DSPASURF", "DP_A", "PP_STATUS", "PP_CTRL", "DPLL_A"];
            for i in 0..11 {
                serial_println!(":: igpu: {:<12} | 0x{:08X}        | 0x{:08X}        | 0x{:08X}        | 0x{:08X} ::", 
                    names[i], TRACE_0[i], TRACE_1[i], TRACE_2[i], trace3[i]);
            }
            serial_println!(":: igpu: GMUX TRACE ::");
            
            let boot_ver_ok = !(GMUX_0[0] == 0x00 && GMUX_0[1] == 0x00 && GMUX_0[2] == 0x00) &&
                              !(GMUX_0[0] == 0xFF && GMUX_0[1] == 0xFF && GMUX_0[2] == 0xFF) &&
                              !(GMUX_0[0] == GMUX_0[1] && GMUX_0[1] == GMUX_0[2]);
            let kern_ver_ok = !(gmux3[0] == 0x00 && gmux3[1] == 0x00 && gmux3[2] == 0x00) &&
                              !(gmux3[0] == 0xFF && gmux3[1] == 0xFF && gmux3[2] == 0xFF) &&
                              !(gmux3[0] == gmux3[1] && gmux3[1] == gmux3[2]);

            if !boot_ver_ok || !kern_ver_ok {
                serial_println!(":: igpu: PROTOCOL UNPROVEN (implausible version tuples)");
                serial_println!(":: igpu: Boot Version: {}.{}.{} | Kernel Version: {}.{}.{}", 
                    GMUX_0[0], GMUX_0[1], GMUX_0[2], gmux3[0], gmux3[1], gmux3[2]);
                serial_println!(":: igpu: Raw SW_DISP: Boot=0x{:02X}, Kern=0x{:02X}", GMUX_0[3], gmux3[3]);
                serial_println!(":: igpu: Raw SW_DDC : Boot=0x{:02X}, Kern=0x{:02X}", GMUX_0[4], gmux3[4]);
                serial_println!(":: igpu: Raw POWER  : Boot=0x{:02X}, Kern=0x{:02X}", GMUX_0[5], gmux3[5]);
            } else {
                serial_println!(":: igpu: Version (Maj,Min,Rel) | {}.{}.{}             |                   |                   | {}.{}.{} ::", 
                    GMUX_0[0], GMUX_0[1], GMUX_0[2], gmux3[0], gmux3[1], gmux3[2]);
                let gnames = ["SW_DISPLAY", "SW_DDC", "DISC_POWER"];
                for i in 0..3 {
                    serial_println!(":: igpu: {:<19} | 0x{:08X}        |                   |                   | 0x{:08X} ::", 
                        gnames[i], GMUX_0[i+3], gmux3[i+3]);
                }
            }
            serial_println!(":: igpu: TRACE END ::");
        }

        let dp_a = mmio_read(bar0, regs::DP_A);
        serial_println!("[Intel iGPU] DP_A: 0x{:08X} (Port A / eDP)", dp_a);

        // Check Pipes
        dump_pipe(bar0, 'A', regs::PIPEACONF, regs::PIPEASRC);
        dump_pipe(bar0, 'B', regs::PIPEBCONF, regs::PIPEBSRC);
        dump_pipe(bar0, 'C', regs::PIPECCONF, regs::PIPECSRC);

        // Check Planes
        let surf_a = dump_plane(bar0, 'A', regs::DSPACNTR, regs::DSPASURF, regs::DSPASTRIDE, regs::DSPALINOFF, regs::DSPATILEOFF);
        let surf_b = dump_plane(bar0, 'B', regs::DSPBCNTR, regs::DSPBSURF, regs::DSPBSTRIDE, regs::DSPBLINOFF, regs::DSPBTILEOFF);
        let surf_c = dump_plane(bar0, 'C', regs::DSPCCNTR, regs::DSPCSURF, regs::DSPCSTRIDE, regs::DSPCLINOFF, regs::DSPCTILEOFF);

        if let Some(surf) = surf_a.or(surf_b).or(surf_c) {
            // Read GGTT entries around the surface base
            let page_number = (surf >> 12) as usize;
            let gtt_offset = regs::GTT_BASE + (page_number * 4);
            
            serial_println!("[Intel iGPU] GGTT Inspection for surface at 0x{:X}:", surf);
            for i in 0..4 {
                let pte_offset = gtt_offset + (i * 4);
                let pte = mmio_read(bar0, pte_offset);
                serial_println!("[Intel iGPU] GGTT PTE[{}] (offset 0x{:X}): 0x{:08X}", page_number + i, pte_offset, pte);
            }
        }
    }
    
    serial_println!(":: igpu: probe-complete ::");
}

unsafe fn dump_pipe(bar0: usize, name: char, conf_reg: usize, src_reg: usize) {
    let conf = mmio_read(bar0, conf_reg);
    let src = mmio_read(bar0, src_reg);
    let enabled = (conf & (1 << 31)) != 0;
    
    serial_println!("[Intel iGPU] Pipe {}: CONF=0x{:08X} (Enabled: {}), SRC=0x{:08X}", name, conf, enabled, src);
}

unsafe fn dump_plane(bar0: usize, name: char, cntr_reg: usize, surf_reg: usize, stride_reg: usize, linoff_reg: usize, tileoff_reg: usize) -> Option<u32> {
    let cntr = mmio_read(bar0, cntr_reg);
    let enabled = (cntr & (1 << 31)) != 0;
    let format = (cntr >> 26) & 0xF;
    let tiled = (cntr & (1 << 10)) != 0;
    let surf = mmio_read(bar0, surf_reg);
    let stride = mmio_read(bar0, stride_reg);
    let linoff = mmio_read(bar0, linoff_reg);
    let tileoff = mmio_read(bar0, tileoff_reg);

    serial_println!("[Intel iGPU] Plane {}: CNTR=0x{:08X} (Enabled: {}, Format: 0x{:X}, Tiled: {})", name, cntr, enabled, format, tiled);
    serial_println!("[Intel iGPU] Plane {}: SURF=0x{:08X}, STRIDE=0x{:08X}, LINOFF=0x{:08X}, TILEOFF=0x{:08X}", 
        name, surf, stride, linoff, tileoff);
    
    if enabled {
        serial_println!(":: igpu: FOX CROSS-CHECK - If Plane {} is enabled here but panel goes black, handoff/bootchain is the cause, not hardware! ::", name);
        Some(surf)
    } else {
        None
    }
}

unsafe fn mmio_read(base: usize, offset: usize) -> u32 {
    core::ptr::read_volatile((base + offset) as *const u32)
}
