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

    // Additional DP/DDI Ports (CPU block)
    pub const DP_B: usize = 0x64100;
    pub const DP_C: usize = 0x64200;
    pub const DP_D: usize = 0x64300;

    // Additional DP/DDI Ports (PCH block)
    pub const PCH_DP_B: usize = 0xE4100;
    pub const PCH_DP_C: usize = 0xE4200;
    pub const PCH_DP_D: usize = 0xE4300;

    // FDI links (CPU-to-PCH)
    pub const FDI_RXA_CTL: usize = 0xF000C;
    pub const FDI_TXA_CTL: usize = 0x60100;

    // DPLL divisors
    pub const FPA0: usize = 0x06040;
    pub const FPA1: usize = 0x06044;

    // PCH-based Panel Power Sequencer (South Display Engine)
    pub const PCH_PP_STATUS: usize = 0xC7200;
    pub const PCH_PP_CONTROL: usize = 0xC7204;
    pub const PCH_PP_ON_DELAYS: usize = 0xC7208;
    pub const PCH_PP_OFF_DELAYS: usize = 0xC720C;
    pub const PCH_PP_DIVISOR: usize = 0xC7210;

    // PCH-based GMBUS (South Display Engine)
    pub const PCH_GMBUS0: usize = 0xC5100;
    pub const PCH_GMBUS1: usize = 0xC5104;
    pub const PCH_GMBUS2: usize = 0xC5108;
    pub const PCH_GMBUS3: usize = 0xC510C;
    pub const PCH_GMBUS4: usize = 0xC5110;

    // GTT Window (starts at 2MB offset in BAR0)
    pub const GTT_BASE: usize = 0x200000;

    // BLT Ring
    pub const BLT_RING_TAIL: usize = 0x22030;
    pub const BLT_RING_HEAD: usize = 0x22034;
    pub const BLT_RING_START: usize = 0x22038;
    pub const BLT_RING_CTL: usize = 0x2203C;
}

#[cfg(target_arch = "x86_64")]
use alloc::alloc::{alloc_zeroed, Layout};
#[cfg(target_arch = "x86_64")]
use spin::Mutex;

use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_arch = "x86_64")]
struct BltRing {
    bar0: usize,
    ring_ptr: *mut u8,
    gtt_offset: u32,
    tail: u32,
    fills: u32,
    scrolls: u32,
    fallbacks: u32,
    spins_max: u32,
    dead: bool,
}
#[cfg(target_arch = "x86_64")]
unsafe impl Send for BltRing {}

#[cfg(target_arch = "x86_64")]
static BLT_RING: Mutex<Option<BltRing>> = Mutex::new(None);

use core::sync::atomic::AtomicU32;
pub static ACTIVE_SURF: AtomicU32 = AtomicU32::new(0);

static PROBED: AtomicBool = AtomicBool::new(false);
static mut TRACE_0: [u32; 11] = [0; 11];
static mut TRACE_1: [u32; 11] = [0; 11];
static mut TRACE_2: [u32; 11] = [0; 11];
static mut GMUX_0: [u32; 7] = [0; 7];
static mut TRACES_VALID: bool = false;

pub fn set_boot_traces(t0: [u32; 11], t1: [u32; 11], t2: [u32; 11], g0: [u32; 7]) {
    unsafe {
        TRACE_0 = t0;
        TRACE_1 = t1;
        TRACE_2 = t2;
        GMUX_0 = g0;
        TRACES_VALID = true;
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn read_gmux_trace() -> [u32; 7] {
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

    let index_read32 = |reg: u8| -> u32 {
        wait_ready();
        outb(0x7D0, reg);
        wait_complete();
        let mut val: u32;
        unsafe { asm!("in eax, dx", out("eax") val, in("dx") 0x7C2u16, options(nomem, nostack, preserves_flags)); }
        val
    };

    let version32 = index_read32(0x04);

    [
        (version32 >> 24) & 0xFF, // VERSION_MAJOR
        (version32 >> 16) & 0xFF, // VERSION_MINOR
        (version32 >> 8) & 0xFF,  // VERSION_RELEASE
        index_read(0x10),         // SWITCH_DISPLAY
        index_read(0x28),         // SWITCH_DDC
        index_read(0x50),         // DISCRETE_POWER
        index_read32(0x70),       // MAX_BRIGHTNESS
    ]
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn read_gmux_trace() -> [u32; 7] { [0; 7] }

pub fn init(gpu: &GpuInfo) {
    if PROBED.swap(true, Ordering::SeqCst) {
        return;
    }
    serial_println!("[Intel iGPU] Initializing Ivy Bridge GT2 at BDF {}:{}:{}", gpu.bus, gpu.slot, gpu.func);

    #[cfg(target_arch = "x86_64")]
    {
        // Reachability check via PCI config space
        let vid_did = unsafe { crate::arch::pci::read_config_32(gpu.bus, gpu.slot, gpu.func, 0x00) };
        let cmd = unsafe { crate::arch::pci::read_config_16(gpu.bus, gpu.slot, gpu.func, 0x04) };
        
        let cap_ptr = unsafe { crate::arch::pci::read_config_32(gpu.bus, gpu.slot, gpu.func, 0x34) } & 0xFF;
        let mut d_state = "Unknown";
        if cap_ptr != 0 && cap_ptr != 0xFF {
            let mut ptr = cap_ptr as u8;
            let mut cap_iters = 0;
            while ptr != 0 {
                if cap_iters >= 48 {
                    serial_println!(":: igpu: CAPABILITY LIST BOUND HIT (48 iterations, aborting walk)");
                    break;
                }
                cap_iters += 1;
                let cap = unsafe { crate::arch::pci::read_config_32(gpu.bus, gpu.slot, gpu.func, ptr) };
                if (cap & 0xFF) == 0x01 { // Power Management
                    let pmcsr = unsafe { crate::arch::pci::read_config_16(gpu.bus, gpu.slot, gpu.func, ptr + 4) };
                    d_state = match pmcsr & 0x3 {
                        0 => "D0",
                        1 => "D1",
                        2 => "D2",
                        3 => "D3hot",
                        _ => "??",
                    };
                    break;
                }
                ptr = ((cap >> 8) & 0xFF) as u8;
            }
        }
        
        serial_println!(":: igpu: REACHABILITY CENSUS (PCI Config Space) ::");
        serial_println!(":: igpu: RAW: VID:DID=0x{:08X}, CMD=0x{:04X}, D-State={} ::", vid_did, cmd, d_state);
        
        if vid_did == 0xFFFFFFFF {
            serial_println!(":: igpu: VERDICT: Device not present (Vendor/Device ID read 0xFFFFFFFF)");
        } else {
            if (cmd & 0x02) == 0 {
                serial_println!(":: igpu: VERDICT: Present but BAR not decoding (Memory Space Enable = 0)");
            }
            if d_state == "D3hot" {
                serial_println!(":: igpu: VERDICT: Present, decoding, but in D3hot (PM state = D3hot)");
            }
            if (cmd & 0x02) != 0 && d_state != "D3hot" {
                serial_println!(":: igpu: VERDICT: Device reachable and powered (D-State: {})", d_state);
            }
        }
    }

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
                serial_println!(":: igpu: Boot MAX_BRT: 0x{:08X} | Kernel MAX_BRT: 0x{:08X}", GMUX_0[6], gmux3[6]);
                serial_println!(":: igpu: Raw SW_DISP: Boot=0x{:02X}, Kern=0x{:02X}", GMUX_0[3], gmux3[3]);
                serial_println!(":: igpu: Raw SW_DDC : Boot=0x{:02X}, Kern=0x{:02X}", GMUX_0[4], gmux3[4]);
                serial_println!(":: igpu: Raw POWER  : Boot=0x{:02X}, Kern=0x{:02X}", GMUX_0[5], gmux3[5]);
            } else {
                serial_println!(":: igpu: PROTOCOL PROVEN (version plausible)");
                serial_println!(":: igpu: Version (Maj,Min,Rel) | {}.{}.{}             |                   |                   | {}.{}.{} ::", 
                    GMUX_0[0], GMUX_0[1], GMUX_0[2], gmux3[0], gmux3[1], gmux3[2]);
                serial_println!(":: igpu: MAX_BRIGHTNESS        | 0x{:08X}        |                   |                   | 0x{:08X} ::", 
                    GMUX_0[6], gmux3[6]);
                
                let decode_disp = |val: u32| match val { 2 => "IGD", 3 => "DIS", _ => "???" };
                let decode_ddc = |val: u32| match val { 1 => "IGD", 2 => "DIS", _ => "???" };
                let decode_pwr = |val: u32| if val != 0 { "ON " } else { "OFF" };

                serial_println!(":: igpu: SW_DISPLAY            | 0x{:02X} ({:<3})          |                   |                   | 0x{:02X} ({:<3}) ::", 
                    GMUX_0[3], decode_disp(GMUX_0[3]), gmux3[3], decode_disp(gmux3[3]));
                serial_println!(":: igpu: SW_DDC                | 0x{:02X} ({:<3})          |                   |                   | 0x{:02X} ({:<3}) ::", 
                    GMUX_0[4], decode_ddc(GMUX_0[4]), gmux3[4], decode_ddc(gmux3[4]));
                serial_println!(":: igpu: DISC_POWER            | 0x{:02X} ({:<3})          |                   |                   | 0x{:02X} ({:<3}) ::", 
                    GMUX_0[5], decode_pwr(GMUX_0[5]), gmux3[5], decode_pwr(gmux3[5]));
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

        let mut active_surf = None;
        if let Some(surf) = surf_a.or(surf_b).or(surf_c) {
            active_surf = Some(surf);
            ACTIVE_SURF.store(surf, Ordering::SeqCst);
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
        
        // BLT ring bring-up. SEAT FIXUP (review round 2): an ACCELERATOR must degrade, never kill
        // the boot — every refusal below breaks out of this block, the ring simply never comes up,
        // `blitter_*` return false, and the CPU path carries the console exactly as before this
        // module existed. Each refusal names itself on an `igpu-blt: ring=absent` line.
        'ring: {
        if let Some(surf) = active_surf {
            let layout = Layout::from_size_align(4096, 4096).unwrap();
            let ring_ptr = alloc_zeroed(layout);

            // Translate the ring's heap VIRTUAL address to physical via the kernel's own walk —
            // the first cut programmed the virtual address into the PTE and worked only by
            // identity-map luck.
            let Some(phys_addr64) = crate::arch::memory::translate(ring_ptr as u64) else {
                serial_println!(":: igpu-blt: ring=absent why=ring-virt-unmapped va=0x{:X} — CPU path carries the console ::", ring_ptr as usize);
                break 'ring;
            };
            let phys_addr = phys_addr64 as usize;
            // Gen7 GGTT PTEs carry extended address bits 39:32 in PTE bits 7:4, which this
            // bring-up does not program — refuse (not panic) anything above 4 GiB.
            if phys_addr >= 0x1_0000_0000 {
                serial_println!(":: igpu-blt: ring=absent why=phys-above-4g phys=0x{:X} — extended PTE bits not programmed; CPU path carries the console ::", phys_addr);
                break 'ring;
            }

            // SEAT FIXUP: the scanout extent comes from the live panel info, not a hardcoded
            // 2880x1800 — a different panel (UNAOS_FBW/FBH override, another machine) would
            // otherwise put the ring PTE INSIDE the scanout and black the panel silently. If the
            // WRITER lock is contended at init time, refuse: a guess here is exactly the defect
            // the review bounced.
            let fb_bytes = match crate::video::WRITER.try_lock().map(|w| {
                let i = w.info();
                (i.height * i.stride * i.bytes_per_pixel) as u32
            }) {
                Some(b) => b,
                None => {
                    serial_println!(":: igpu-blt: ring=absent why=writer-locked — cannot prove scanout extent; CPU path carries the console ::");
                    break 'ring;
                }
            };
            let extent = surf + fb_bytes;
            let gtt_page = ((extent + 4095) / 4096) as u32; // provably beyond the scanout surface

            let ring_gtt_addr = gtt_page * 4096;
            let gtt_offset = regs::GTT_BASE + (gtt_page as usize * 4);
            let pte = (phys_addr as u32) | 1; // Valid bit

            // Neighbouring PTEs read BEFORE the write; re-read and compared AFTER it below — a
            // store that smears past its slot is a silent black panel, so "unchanged" is verified,
            // not asserted in prose.
            let pte_prev = mmio_read(bar0, gtt_offset - 4);
            let pte_next = mmio_read(bar0, gtt_offset + 4);
            serial_println!(":: igpu: GGTT slot constraint - writing ring PTE at 0x{:X} (slot offset 0x{:X}, scanout extent 0x{:X}) ::", ring_gtt_addr, gtt_offset, extent);

            core::ptr::write_volatile((bar0 + gtt_offset) as *mut u32, pte);
            let pte_prev_after = mmio_read(bar0, gtt_offset - 4);
            let pte_next_after = mmio_read(bar0, gtt_offset + 4);
            if pte_prev_after != pte_prev || pte_next_after != pte_next {
                serial_println!(":: igpu-blt: ring=absent why=neighbour-pte-changed prev 0x{:08X}->0x{:08X} next 0x{:08X}->0x{:08X} — PTE write smeared; ring NOT enabled ::",
                    pte_prev, pte_prev_after, pte_next, pte_next_after);
                core::ptr::write_volatile((bar0 + gtt_offset) as *mut u32, 0);
                break 'ring;
            }
            serial_println!(":: igpu: GGTT PTE prev: 0x{:08X}, PTE next: 0x{:08X} (verified unchanged after write) ::", pte_prev, pte_next);
            
            core::ptr::write_volatile((bar0 + regs::BLT_RING_CTL) as *mut u32, 0);
            core::ptr::write_volatile((bar0 + regs::BLT_RING_START) as *mut u32, ring_gtt_addr);
            core::ptr::write_volatile((bar0 + regs::BLT_RING_HEAD) as *mut u32, 0);
            core::ptr::write_volatile((bar0 + regs::BLT_RING_TAIL) as *mut u32, 0);
            core::ptr::write_volatile((bar0 + regs::BLT_RING_CTL) as *mut u32, 1); // 4KB length, enable
            
            *BLT_RING.lock() = Some(BltRing {
                bar0,
                ring_ptr,
                gtt_offset: ring_gtt_addr,
                tail: 0,
                fills: 0,
                scrolls: 0,
                fallbacks: 0,
                spins_max: 0,
                dead: false,
            });
            serial_println!(":: igpu: BLT Ring initialized at GGTT 0x{:08X} (Phys 0x{:08X}) ::", ring_gtt_addr, phys_addr);
        }
        } // 'ring

        serial_println!(":: igpu: [CITATION: Intel PRM Vol 3, Display Registers] On Ivy Bridge (Gen7 / Panther Point 7-Series PCH), the Display Engine is split.");
        serial_println!(":: igpu: [CITATION: Intel PRM Vol 3, Display Registers, Section 1.1.2] eDP on Port A (DP_A) is CPU-attached (North Display Engine).");
        serial_println!(":: igpu: [CITATION: Intel PRM Vol 3, South Display Engine Registers] GMBUS and Panel Power Sequencer (PPS) are PCH-attached (South Display Engine).");
        serial_println!(":: igpu: [CITATION: Intel PRM Vol 3, South Display Engine Registers] Therefore, GMBUS is at PCH base 0xC5100 and PPS is at PCH base 0xC7200.");
        serial_println!(":: igpu: [CITATION: Intel PRM Vol 3, Display Registers] Because eDP is CPU-attached, the FDI link (CPU-to-PCH) is bypassed for the internal panel.");

        serial_println!(":: igpu: --- ADDITIONAL CENSUS GAPS --- ::");
        serial_println!(":: igpu: PP_STATUS_CPU:  0x{:08X} | PP_STATUS_PCH:  0x{:08X}", mmio_read(bar0, regs::PP_STATUS), mmio_read(bar0, regs::PCH_PP_STATUS));
        serial_println!(":: igpu: PP_CONTROL_CPU: 0x{:08X} | PP_CONTROL_PCH: 0x{:08X}", mmio_read(bar0, regs::PP_CONTROL), mmio_read(bar0, regs::PCH_PP_CONTROL));
        serial_println!(":: igpu: DP_B_CPU: 0x{:08X} | DP_B_PCH: 0x{:08X}", mmio_read(bar0, regs::DP_B), mmio_read(bar0, regs::PCH_DP_B));
        serial_println!(":: igpu: DP_C_CPU: 0x{:08X} | DP_C_PCH: 0x{:08X}", mmio_read(bar0, regs::DP_C), mmio_read(bar0, regs::PCH_DP_C));
        serial_println!(":: igpu: DP_D_CPU: 0x{:08X} | DP_D_PCH: 0x{:08X}", mmio_read(bar0, regs::DP_D), mmio_read(bar0, regs::PCH_DP_D));
        serial_println!(":: igpu: FDI_RXA_CTL: 0x{:08X}", mmio_read(bar0, regs::FDI_RXA_CTL));
        serial_println!(":: igpu: FDI_TXA_CTL: 0x{:08X}", mmio_read(bar0, regs::FDI_TXA_CTL));
        serial_println!(":: igpu: FPA0: 0x{:08X}", mmio_read(bar0, regs::FPA0));
        serial_println!(":: igpu: FPA1: 0x{:08X}", mmio_read(bar0, regs::FPA1));
        
        serial_println!(":: igpu: PCH_PP_ON_DELAYS: 0x{:08X}", mmio_read(bar0, regs::PCH_PP_ON_DELAYS));
        serial_println!(":: igpu: PCH_PP_OFF_DELAYS: 0x{:08X}", mmio_read(bar0, regs::PCH_PP_OFF_DELAYS));
        serial_println!(":: igpu: PCH_PP_DIVISOR: 0x{:08X}", mmio_read(bar0, regs::PCH_PP_DIVISOR));

        serial_println!(":: igpu: PCH_GMBUS0: 0x{:08X}", mmio_read(bar0, regs::PCH_GMBUS0));
        serial_println!(":: igpu: PCH_GMBUS1: 0x{:08X}", mmio_read(bar0, regs::PCH_GMBUS1));
        serial_println!(":: igpu: PCH_GMBUS2: 0x{:08X}", mmio_read(bar0, regs::PCH_GMBUS2));
        serial_println!(":: igpu: PCH_GMBUS3: 0x{:08X}", mmio_read(bar0, regs::PCH_GMBUS3));
        serial_println!(":: igpu: PCH_GMBUS4: 0x{:08X}", mmio_read(bar0, regs::PCH_GMBUS4));
        serial_println!(":: igpu: --- END CENSUS --- ::");
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

#[cfg(target_arch = "x86_64")]
impl BltRing {
    fn submit(&mut self, dwords: &[u32]) -> bool {
        if self.dead {
            return false;
        }

        let mut tail = self.tail;
        
        for &dw in dwords {
            unsafe {
                core::ptr::write_volatile(
                    (self.ring_ptr as *mut u32).add(tail as usize / 4),
                    dw
                );
            }
            tail += 4;
            if tail >= 4096 {
                tail = 0;
            }
        }
        
        if (tail / 4) % 2 != 0 {
            unsafe {
                core::ptr::write_volatile(
                    (self.ring_ptr as *mut u32).add(tail as usize / 4),
                    0
                );
            }
            tail += 4;
            if tail >= 4096 {
                tail = 0;
            }
        }
        
        self.tail = tail;
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        
        unsafe {
            core::ptr::write_volatile((self.bar0 + regs::BLT_RING_TAIL) as *mut u32, tail);
        }
        
        let mut spins = 0;
        let max_spins = 1_000_000; // bounded cycle budget timeout (approx 1M pause cycles)
        
        loop {
            let current_head = unsafe { core::ptr::read_volatile((self.bar0 + regs::BLT_RING_HEAD) as *const u32) } & 0x1FFFFC;
            if current_head == tail {
                break;
            }
            core::hint::spin_loop();
            spins += 1;
            if spins > max_spins {
                self.dead = true;
                self.fallbacks += 1;
                serial_println!(":: igpu: STOP-NOTE blitter wedged, ring marked dead ::");
                return false; // Return and let the CPU fallback path take over
            }
        }
        
        if spins > self.spins_max {
            self.spins_max = spins;
        }
        
        true
    }
}

pub fn blitter_fill_rect(dst_gtt: u32, x: u16, y: u16, w: u16, h: u16, color: u32, pitch: u32) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let mut ring_lock = BLT_RING.lock();
        if let Some(ring) = ring_lock.as_mut() {
            let dw0 = (0x50 << 22) | 4;
            let dw1 = (3 << 24) | (0xF0 << 16) | pitch;
            let dw2 = (y as u32) << 16 | (x as u32);
            let dw3 = ((y + h) as u32) << 16 | ((x + w) as u32);
            let dw4 = dst_gtt;
            let dw5 = color;
            
            if ring.submit(&[dw0, dw1, dw2, dw3, dw4, dw5]) {
                ring.fills += 1;
                return true;
            }
        }
    }
    false
}

pub fn blitter_copy_rect(dst_gtt: u32, src_gtt: u32, dst_x: u16, dst_y: u16, src_x: u16, src_y: u16, w: u16, h: u16, pitch: u32) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let mut ring_lock = BLT_RING.lock();
        if let Some(ring) = ring_lock.as_mut() {
            let dw0 = (0x53 << 22) | 6;
            let dw1 = (3 << 24) | (0xCC << 16) | pitch;
            let dw2 = (dst_y as u32) << 16 | (dst_x as u32);
            let dw3 = ((dst_y + h) as u32) << 16 | ((dst_x + w) as u32);
            let dw4 = dst_gtt;
            let dw5 = (src_y as u32) << 16 | (src_x as u32);
            let dw6 = pitch;
            let dw7 = src_gtt;
            
            if ring.submit(&[dw0, dw1, dw2, dw3, dw4, dw5, dw6, dw7]) {
                ring.scrolls += 1;
                return true;
            }
        }
    }
    false
}

pub fn print_blt_stats() {
    #[cfg(target_arch = "x86_64")]
    {
        if let Some(ring) = BLT_RING.lock().as_ref() {
            serial_println!(":: igpu-blt: ring={} fills={} scrolls={} fallbacks={} spins_max={} ::", 
                if ring.dead { "dead" } else { "up" },
                ring.fills, ring.scrolls, ring.fallbacks, ring.spins_max);
        }
    }
}
