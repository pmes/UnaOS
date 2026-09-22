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


    // DPA AUX Channel
    pub const DPA_AUX_CH_CTL: usize = 0x64010;
    pub const DPA_AUX_CH_DATA1: usize = 0x64014;
    pub const DPA_AUX_CH_DATA2: usize = 0x64018;
    pub const DPA_AUX_CH_DATA3: usize = 0x6401C;
    pub const DPA_AUX_CH_DATA4: usize = 0x64020;
    pub const DPA_AUX_CH_DATA5: usize = 0x64024;

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
    pub const BLT_RING_ACTHD: usize = 0x22074;

    // ═══════════════════════════════════════════════════════════════════════════════════════
    // GMUX8 — PIPE TIMING AND PIPE M/N, EVERY ROW PINNED TO THE PUBLIC IVB PRM.
    //
    // Source, and it is the ONLY source: Intel® OpenSource HD Graphics PRM
    // **Volume 3 Part 3: North Display Engine Registers (Ivy Bridge)**, Doc Ref
    // `IHD-OS-V3 Pt 3 – 05 12`, May 2012 Rev 1.0, downloaded from intel.com
    // (`cdrdv2-public.intel.com/690915/ivb-ihd-os-vol3-part3.pdf`). Section and PAGE are on
    // every family below, and the same rows are tabulated in
    // `docs/dev/OS/08_VIDEO/gpu_spec.md` §6. No i915 / i965 naming was opened for any of it.
    //
    // The three addresses per family are the PRM's own three printed rows (Pipe A / B / C),
    // not a stride this file derived — see §4.1.1 p.66, which prints `60000h`, `61000h` and
    // `62000h` as three separate `Address:` lines. `regs::PIPEASRC`/`PIPEACONF` above are the
    // same registers the PRM calls `PIPE_SRCSZ_A` (§4.1.7 p.72) and `PIPE_CONF_A` (§5.1.3
    // p.100), and this pass PINS those two offsets without moving them.
    //
    // ⚠ THREE REGISTERS A MODE-SET WANTS ARE NOT HERE AND CANNOT BE: `DP_TP_CTL`,
    // `DP_TP_STATUS` and `TRANS_DDI_FUNC_CTL` occur **zero** times in either IVB display
    // volume (Vol3 Pt3 North, Vol3 Pt4 South) — they are Haswell DDI-era registers. On this
    // part eDP link training is `DP_CTL_A` bits 9:8 (§4.4.1 pp.85–86) and the eDP port is
    // CPU-attached, so it is driven straight off the pipe timing block below with no PCH
    // transcoder in the path. That is a FINDING, not a gap: see rung 08b's NOT-IN-IVB-PRM line.

    // HTOTAL — §4.1.1 p.66. [28:16] Horizontal Total (pixels−1), [11:0] Horizontal Active (−1).
    pub const PIPE_HTOTAL_A: usize = 0x60000;
    pub const PIPE_HTOTAL_B: usize = 0x61000;
    pub const PIPE_HTOTAL_C: usize = 0x62000;

    // HBLANK — §4.1.2 p.67. [28:16] Horizontal Blank End, [12:0] Horizontal Blank Start.
    pub const PIPE_HBLANK_A: usize = 0x60004;
    pub const PIPE_HBLANK_B: usize = 0x61004;
    pub const PIPE_HBLANK_C: usize = 0x62004;

    // HSYNC — §4.1.3 p.68. [28:16] Horizontal Sync End, [12:0] Horizontal Sync Start.
    pub const PIPE_HSYNC_A: usize = 0x60008;
    pub const PIPE_HSYNC_B: usize = 0x61008;
    pub const PIPE_HSYNC_C: usize = 0x62008;

    // VTOTAL — §4.1.4 p.69. [28:16] Vertical Total (lines−1 progressive), [11:0] Vertical Active.
    pub const PIPE_VTOTAL_A: usize = 0x6000C;
    pub const PIPE_VTOTAL_B: usize = 0x6100C;
    pub const PIPE_VTOTAL_C: usize = 0x6200C;

    // VBLANK — §4.1.5 p.70. [28:16] Vertical Blank End, [12:0] Vertical Blank Start.
    pub const PIPE_VBLANK_A: usize = 0x60010;
    pub const PIPE_VBLANK_B: usize = 0x61010;
    pub const PIPE_VBLANK_C: usize = 0x62010;

    // VSYNC — §4.1.6 p.71. [28:16] Vertical Sync End, [12:0] Vertical Sync Start.
    pub const PIPE_VSYNC_A: usize = 0x60014;
    pub const PIPE_VSYNC_B: usize = 0x61014;
    pub const PIPE_VSYNC_C: usize = 0x62014;

    // DATAM — §4.2.1 pp.74–75. [30:25] TU Size (−1), [23:0] Data M. M1 = normal refresh,
    // M2 = the low-power set PIPE_CONF bit 20 selects (§5.1.3 p.101).
    pub const PIPE_DATAM1_A: usize = 0x60030;
    pub const PIPE_DATAM1_B: usize = 0x61030;
    pub const PIPE_DATAM1_C: usize = 0x62030;
    pub const PIPE_DATAM2_A: usize = 0x60038;
    pub const PIPE_DATAM2_B: usize = 0x61038;
    pub const PIPE_DATAM2_C: usize = 0x62038;

    // DATAN — §4.2.2 pp.75–76. [23:0] Data N.
    pub const PIPE_DATAN1_A: usize = 0x60034;
    pub const PIPE_DATAN1_B: usize = 0x61034;
    pub const PIPE_DATAN1_C: usize = 0x62034;
    pub const PIPE_DATAN2_A: usize = 0x6003C;
    pub const PIPE_DATAN2_B: usize = 0x6103C;
    pub const PIPE_DATAN2_C: usize = 0x6203C;

    // LINKM — §4.2.3 p.76. [23:0] Link M, the m sent in the Main Stream Attributes.
    pub const PIPE_LINKM1_A: usize = 0x60040;
    pub const PIPE_LINKM1_B: usize = 0x61040;
    pub const PIPE_LINKM1_C: usize = 0x62040;
    pub const PIPE_LINKM2_A: usize = 0x60048;
    pub const PIPE_LINKM2_B: usize = 0x61048;
    pub const PIPE_LINKM2_C: usize = 0x62048;

    // LINKN — §4.2.4 p.77. [23:0] Link N. "Writes to this register arm M/N registers for this
    // pipe" — the double-buffer arm point for the whole M/N set, which the write rung will need.
    pub const PIPE_LINKN1_A: usize = 0x60044;
    pub const PIPE_LINKN1_B: usize = 0x61044;
    pub const PIPE_LINKN1_C: usize = 0x62044;
    pub const PIPE_LINKN2_A: usize = 0x6004C;
    pub const PIPE_LINKN2_B: usize = 0x6104C;
    pub const PIPE_LINKN2_C: usize = 0x6204C;
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

#[cfg(target_arch = "x86_64")]
static IGPU_BAR0: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);


use core::sync::atomic::AtomicU32;
pub static ACTIVE_SURF: AtomicU32 = AtomicU32::new(0);

static PROBED: AtomicBool = AtomicBool::new(false);
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
pub static PROTOCOL_PROVEN: AtomicBool = AtomicBool::new(false);
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

// ═══════════════════════════════════════════════════════════════════════════════════════════
// GMUX-IGD — point the display mux at the integrated GPU, prove the write landed, get back.
//
// EVERYTHING BELOW THIS LINE IS COMPILED ONLY WHEN `gmux_igd` IS ON.
//
// That split is deliberate. `read_gmux_trace()` above is left exactly as baseline has it —
// inline closures, a hard iteration cap, no `arch::ms()` anywhere — so the knob-off build is
// behaviourally identical to trunk. An earlier attempt replaced those closures with
// `arch::ms()`-deadline helpers gated on `target_arch` only, so EVERY `unaos_ivb` build (armed
// or not) picked up a wait whose bound depends on the BSP timer ISR still running. The old
// bound could not hang; that one could. The armed gmux helpers here carry an unconditional
// iteration cap, so even on the armed build a stopped clock cannot hang them
// (though `dp_aux_transfer`'s inner wait remains rdtsc-deadline-only).
//
// The panel WILL go black between the switch and the revert. That is the EXPECTED result, not
// the experiment failing: the census in this same function reads every pipe, every plane and
// `DPLL_A` as zero, so nothing on the integrated side is driving the panel. The deliverable is
// the READ-BACK proving the mux write landed.
//
// See docs/dev/GEMINI/video/iGUI/PROPOSAL-igpu-gmux-igd.md and RUNBOOK-gmux-igd.md.
// ═══════════════════════════════════════════════════════════════════════════════════════════

// Port map and register/value encodings, from Linux `drivers/platform/x86/apple-gmux.c` (the
// classic port-I/O backend, which is the one this 2012 Retina MacBookPro uses):
//   GMUX_PORT_VALUE 0x7C2 · GMUX_PORT_READ 0x7D0 · GMUX_PORT_WRITE 0x7D4
//   GMUX_PORT_SWITCH_DISPLAY 0x10 · GMUX_PORT_SWITCH_DDC 0x28 · GMUX_PORT_SWITCH_EXTERNAL 0x40
//   GMUX_SWITCH_DDC_IGD 0x1 / _DIS 0x2 · GMUX_SWITCH_DISPLAY_IGD 0x2 / _DIS 0x3
//   (EXTERNAL shares the DISPLAY encoding).
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_PORT_VALUE: u16 = 0x7C2;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_PORT_READ: u16 = 0x7D0;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_PORT_WRITE: u16 = 0x7D4;

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_SWITCH_DDC: u8 = 0x28;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_SWITCH_DISPLAY: u8 = 0x10;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_READ_DISPLAY: u8 = 0x11;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_READ_EXTERNAL: u8 = 0x41;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_DDC_DIS: u8 = 0x02;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_DISPLAY_DIS: u8 = 0x03;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_EXTERNAL_DIS: u8 = 0x03;
/// A GATE-ACCEPTED EXTERNAL pre-state: the EXT port's value on a Kepler-owned boot. Metal fact,
/// Boot AK: `pre-switch state DDC=0x02 DISP=0x03 EXT=0x21`. This is the NORM on the 2012 rMBP —
/// what the firmware actually leaves behind — not an anomaly, which is why round 11 first admitted
/// it and why round 13 continues to.
///
/// The pre-switch gate accepts EXTERNAL in `{ GMUX_EXTERNAL_DIS, GMUX_EXTERNAL_KEPLER_OWNED }` and
/// the unwind restores **the member it validated**, never a blanket `GMUX_EXTERNAL_DIS`: forcing a
/// Kepler-owned port to DIS is not a restore, it is a silent state change. Anything outside the set
/// — including the 0xFFFFFFFF gmux-timeout sentinel — refuses before any mux is touched.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_EXTERNAL_KEPLER_OWNED: u8 = 0x21;

/// Name an EXTERNAL register value for the witness lines, so a reader never has to guess whether
/// an `EXT=0x21` in a capture is the correct Kepler-owned pre-state or a failed restore.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn ext_state_name(v: u32) -> &'static str {
    if v == GMUX_EXTERNAL_DIS as u32 {
        "DIS"
    } else if v == GMUX_EXTERNAL_KEPLER_OWNED as u32 {
        "kepler-owned"
    } else {
        "UNACCEPTED"
    }
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_SWITCH_EXTERNAL: u8 = 0x40;

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_DISPLAY_IGD: u8 = 0x02;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_EXTERNAL_IGD: u8 = 0x02;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_DDC_IGD: u8 = 0x01;

/// Baseline's iteration bound, kept UNCONDITIONALLY.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_WAIT_ITERS: u32 = 5000;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn gmux_outb(port: u16, val: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags)); }
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn gmux_inb(port: u16) -> u8 {
    let mut val: u8;
    unsafe { core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack, preserves_flags)); }
    val
}

/// Wait for the gmux to be ready to accept an index byte. Bounded by an iteration count that cannot depend on any clock.
/// Returns false on timeout —
/// and no caller here swallows a timeout silently.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn gmux_wait_ready() -> bool {
    let mut iters = GMUX_WAIT_ITERS;
    loop {
        if (unsafe { gmux_inb(GMUX_PORT_WRITE) } & 0x01) == 0 {
            return true;
        }
        // apple-gmux.c drains the stale reply byte here before retrying.
        let _ = unsafe { gmux_inb(GMUX_PORT_READ) };
        if iters == 0 {
            return false;
        }
        iters -= 1;
        for _ in 0..1000 { core::hint::spin_loop(); }
    }
}

/// Wait for the gmux to signal that the transaction completed, then consume the reply byte.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn gmux_wait_complete() -> bool {
    let mut iters = GMUX_WAIT_ITERS;
    loop {
        if (unsafe { gmux_inb(GMUX_PORT_WRITE) } & 0x01) != 0 {
            let _ = unsafe { gmux_inb(GMUX_PORT_READ) };
            return true;
        }
        if iters == 0 {
            return false;
        }
        iters -= 1;
        for _ in 0..1000 { core::hint::spin_loop(); }
    }
}

/// Read one gmux register. Returns `0xFFFFFFFF` on timeout — a value no 8-bit register can
/// produce, which is what makes the refuse-to-arm sentinel unambiguous.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn gmux_index_read(reg: u8) -> u32 {
    if !unsafe { gmux_wait_ready() } { return 0xFFFFFFFF; }
    unsafe { gmux_outb(GMUX_PORT_READ, reg) };
    if !unsafe { gmux_wait_complete() } { return 0xFFFFFFFF; }
    (unsafe { gmux_inb(GMUX_PORT_VALUE) }) as u32
}

/// Write one gmux register. Upstream order, reproduced exactly: **value byte first, then
/// `gmux_index_wait_ready()`, then the index byte**, then wait for completion.
///
/// The wait BETWEEN the value write and the index write is upstream's — see
/// `gmux_index_write8()` in `drivers/platform/x86/apple-gmux.c`. Two separate reviews asked
/// for it to be removed on the theory that it belongs only before the value byte; both were
/// wrong and the instruction is retracted. Cited here so it is not raised a third time.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn gmux_index_write(reg: u8, val: u8) -> bool {
    unsafe { gmux_outb(GMUX_PORT_VALUE, val) };
    if !unsafe { gmux_wait_ready() } { return false; }
    unsafe { gmux_outb(GMUX_PORT_WRITE, reg) };
    unsafe { gmux_wait_complete() }
}

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
        IGPU_BAR0.store(bar0, Ordering::SeqCst);
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
                #[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
                PROTOCOL_PROVEN.store(true, Ordering::SeqCst);
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

                // GMUX-IGD: the arm block lives INSIDE the `PROTOCOL PROVEN` arm, not after the
                // if/else. An earlier version computed `boot_ver_ok`/`kern_ver_ok`, closed the
                // if/else, and then opened the arm block outside both branches — so a gmux that
                // answered the handshake but reported an implausible version tuple passed the
                // 0xFFFFFFFF sentinel and got its display mux written anyway. The version check
                // is only a gate if something is actually gated on it.

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

        // GEN7-3D rung R1 — read-only render-engine reconnaissance (UNAOS_IVB3D).
        // Placed HERE, above `bring_up_blt_ring`, deliberately: the rung's GGTT census must
        // see FIRMWARE's page tables, not ours. `bring_up_blt_ring` writes a GGTT PTE when
        // it does not refuse, and a census taken after it would be reading our own footprint
        // and calling it a finding. Read-only, and it cannot black the panel: it writes
        // nothing. (Precisely: reads only. It does touch ONE display-block offset —
        // PCH_PP_CONTROL, read as a control-frame witness because the PPS sits outside the
        // GT power well — and it never writes it.)
        //
        // The cfg is `all(target_arch = "x86_64", feature = "gen7")`, not `feature` alone:
        // that pair is what makes "gen7 emits not one byte of aarch64 code" a property of
        // the SOURCE, and it is what `arroyo`'s `arm_features` strip comment asserts.
        #[cfg(all(target_arch = "x86_64", feature = "gen7"))]
        super::gen7::recon(bar0, bar0_size, gpu.bus, gpu.slot, gpu.func);

        // GEN7-3D rung R2 — the wake (UNAOS_IVB3D). The FIRST write in the ladder: three MMIO
        // writes to the IVB Sync-Flush workaround path (INSTPM 0x2050, RCS_WAKE 0x2700), a
        // bounded poll of 0x22AC, then re-park INSTPM to 0x00010000 on every exit path. Placed
        // AFTER `recon` — so the R1 GGTT census still read firmware's page tables, not a
        // post-wake state — and BEFORE `bring_up_blt_ring`. It writes no GGTT entry, no ring
        // register and no display register; the panel is the Kepler's, and the wake write is
        // reversed in-rung, so R2 cannot black Peter's screen. Its proof is the same
        // 17-register GT battery R1 read dark reading structured/varying afterward.
        #[cfg(all(target_arch = "x86_64", feature = "gen7"))]
        super::gen7::wake(bar0, bar0_size, gpu.bus, gpu.slot, gpu.func);

        // GEN7-3D rung R3 — the forcewake acquire (UNAOS_IVB3D). R2 flew on Boot D and came
        // back `gt-still-dark`: its poll passed on iteration zero because a power-gated window
        // reads zero and the pass condition was `== 0`, and not one of the fourteen ring
        // registers moved. So R3 goes at the power well itself — the two forcewake
        // request/ack pairs Intel actually published (0x0A188/0x130044 [BDW], 0x1300B0/0x1300B4
        // [CHV]) — one candidate at a time, each request released in-rung AND the release
        // verified against the register's entry dword, with the same 17-register battery read
        // under each hold. Placed AFTER `wake` (which re-parks INSTPM on every exit path, so R3
        // starts from firmware's state) and still BEFORE `bring_up_blt_ring`. It writes at most
        // two GT power-management registers, no ring register, no GGTT entry and no display
        // register; the panel is the Kepler's, so R3 cannot black Peter's screen. R3 returns
        // its wake verdict as a `GtWake` so R4 branches on real evidence.
        #[cfg(all(target_arch = "x86_64", feature = "gen7"))]
        let gt_wake = super::gen7::forcewake(bar0, bar0_size, gpu.bus, gpu.slot, gpu.func);

        // GEN7-3D rung R4 — the GGTT claim (UNAOS_IVB3D). It reads R3's `GtWake` verdict and
        // branches on TWO gates. The read-only recon (RCS ring registers + GGTT window census)
        // runs on any reachable wake. The ONE reversible PTE round-trip (write, read back the
        // 0->pte transition, verify no neighbour smeared, restore the whole neighbourhood to zero
        // and re-read) is attempted ONLY on a CONFIRMED wake (`Woke`/`LiveAlready`): an ack-less
        // `WokeNoAck` gets the recon but the write is withheld (`claim-gated-on-ack`), and a
        // `Dark` GT gets the recon and writes nothing (`gated-on-wake`) — the outcome Boot D's
        // `gt-still-dark` makes most likely. Placed AFTER `forcewake` and still BEFORE
        // `bring_up_blt_ring`. On the confirmed-wake branch it touches at most three GGTT PTE
        // slots, all in a window it first read as zero and all restored to zero, and frees its
        // scratch page only once that reversal verifies; it writes no ring register and no
        // display register, so R4 cannot black Peter's screen either.
        #[cfg(all(target_arch = "x86_64", feature = "gen7"))]
        super::gen7::claim(bar0, bar0_size, gpu.bus, gpu.slot, gpu.func, gt_wake);

        // GEN7-3D rung R5 — the first EXECUTED command (UNAOS_IVB3D). It reads R3's `GtWake`
        // verdict and self-gates on the same `write_ok()` evidence R4's PTE round-trip uses. On a
        // CONFIRMED wake it claims TWO GGTT slots via the R4b path (a ring page + a target page),
        // maps a minimal RCS ring, submits one MI_STORE_DATA_IMM (IVB-V1P3 §1.2.17 p.186) that
        // stores a sentinel to the target page's GGTT address, and proves execution by reading the
        // sentinel back through the target page's own CPU mapping. Then it disables the ring
        // (proven by CTL readback), restores both PTEs and their neighbours to their entry images
        // and re-reads, and LEAKS both pages (no GGTT TLB-invalidation rung exists yet — the R4b
        // rule). On a `Dark`/`WokeNoAck` wake it runs the read-only recon and writes nothing
        // (`gated-on-wake` / `exec-gated-on-ack`) — the outcome Boot D's `gt-still-dark` makes most
        // likely. Placed AFTER `claim` and still BEFORE `bring_up_blt_ring`. It writes at most two
        // GGTT PTEs (pinned uncached-coherent encoding, clflushed) and the four RCS ring registers
        // (drained to idle, disabled with the disable proven by readback, then restored to their
        // captured entry images — never unmapped under a live ring), no display register, so R5
        // cannot black Peter's screen; a GT fault parks the CS, it does not touch scanout.
        #[cfg(all(target_arch = "x86_64", feature = "gen7"))]
        super::gen7::execute(bar0, bar0_size, gpu.bus, gpu.slot, gpu.func, gt_wake);

        // GEN7-3D rung R6 — the wake that makes RING_CTL latch (UNAOS_IVB3D). R5 flew three boot
        // legs and came back `enable-void`: the GGTT PTEs landed, the four RCS submission
        // registers were programmed, RING_CTL was written 0x00000001 and read back 0x00000000.
        // R3 releases its forcewake acquire inside its own rung, so nothing was held when R5
        // armed the ring — which R5's own `next=` named as the suspect. R6 is that experiment:
        // it acquires a forcewake candidate (MT 0x0A188/0x130044 [BDW-ONLY] first — the only
        // [PINNED]-adjacent pair, and the one R3's now-retired preheld guard skipped on metal —
        // then RENFW [CHV-ONLY], then GTFORCEAWAKE), KEEPS the hold across the ring arm, the
        // MI_STORE_DATA_IMM submit, the drain, the disable and the register restore, and only
        // then releases it. It stops at the first candidate whose RING_CTL enable reads back
        // set. It also closes R5's two-page-per-run leak: a GGTT TLB-invalidation rung with its
        // own verdict, and a reclaim gated on that verdict OR on the proof that no engine access
        // was ever issued through either GGTT address. Placed AFTER `execute` and still BEFORE
        // `bring_up_blt_ring`. Same write envelope as R5, not one inch wider — the Dark branch
        // writes nothing, every write is captured/restored/re-read on every exit path, the GGTT
        // claim only enters a proven-unowned window, and no display register is touched — so R6
        // cannot black Peter's screen either.
        #[cfg(all(target_arch = "x86_64", feature = "gen7"))]
        super::gen7::rearm(bar0, bar0_size, gpu.bus, gpu.slot, gpu.func, gt_wake);

        // GEN7-3D rung R7 — the BCS blitter ring (UNAOS_IVB3D). R6 asked whether ANY ring's
        // RING_CTL latches under a held wake and whether the RCS retires a bare store; R7 keeps
        // that whole envelope (same R6_CANDS holds, same fw_acquire-across-the-arm, same
        // proven-unowned GGTT window, full capture/restore/re-read) and moves it to the BCS with a
        // real XY_SRC_COPY_BLT — a 16x16x32bpp pixel copy plus an MI_STORE_DATA_IMM sentinel, so
        // the copy and the retirement are two independent witnesses. Placed AFTER `rearm` and
        // still BEFORE `bring_up_blt_ring`. Same write envelope as R6 plus one GGTT slot: the Dark
        // branch writes nothing, every write is captured/restored/re-read on every exit path, the
        // claim only enters a proven-unowned window, the PTEs are never unmapped under a live
        // ring, and no display register is touched — so R7 cannot black Peter's screen either.
        #[cfg(all(target_arch = "x86_64", feature = "gen7"))]
        super::gen7::blit(bar0, bar0_size, gpu.bus, gpu.slot, gpu.func, gt_wake);

        // BLT ring bring-up. SEAT FIXUP (review round 2): an ACCELERATOR must degrade, never kill
        // the boot — every refusal below breaks out of this block, the ring simply never comes up,
        // `blitter_*` return false, and the CPU path carries the console exactly as before this
        // module existed. Each refusal names itself on an `igpu-blt: ring=absent` line.

        bring_up_blt_ring(bar0, active_surf);

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

        let start_raw_head = unsafe { core::ptr::read_volatile((self.bar0 + regs::BLT_RING_HEAD) as *const u32) };

        unsafe {
            core::ptr::write_volatile((self.bar0 + regs::BLT_RING_TAIL) as *mut u32, tail);
        }

        let mut spins = 0;
        let max_spins = 1_000_000; // bounded cycle budget timeout (approx 1M pause cycles)

        loop {
            let current_raw_head = unsafe { core::ptr::read_volatile((self.bar0 + regs::BLT_RING_HEAD) as *const u32) };
            let current_head = current_raw_head & 0x1FFFFC;
            if current_head == tail {
                break;
            }
            core::hint::spin_loop();
            spins += 1;
            if spins > max_spins {
                self.dead = true;
                self.fallbacks += 1;

                let ctl = unsafe { core::ptr::read_volatile((self.bar0 + regs::BLT_RING_CTL) as *const u32) };
                let acthd = unsafe { core::ptr::read_volatile((self.bar0 + regs::BLT_RING_ACTHD) as *const u32) };
                let hw_tail = unsafe { core::ptr::read_volatile((self.bar0 + regs::BLT_RING_TAIL) as *const u32) };

                let verdict = if (ctl & 1) == 0 {
                    "ring-disabled"
                } else if current_raw_head == start_raw_head {
                    "head-never-moved"
                } else if (current_raw_head >> 21) != (start_raw_head >> 21) {
                    "head-wrapped"
                } else {
                    "head-stalled-mid-run"
                };

                serial_println!(":: igpu: STOP-NOTE blitter wedged, ring marked dead ({}) ::", verdict);
                serial_println!(":: igpu: [BLT] Snapshot: HEAD=0x{:08X} HW_TAIL=0x{:08X} CTL=0x{:08X} ACTHD=0x{:08X} ::", current_raw_head, hw_tail, ctl, acthd);
                serial_println!(":: igpu: [BLT] Refutation: start_raw_head=0x{:08X} current_raw_head=0x{:08X} tail=0x{:08X} hw_tail=0x{:08X} ::", start_raw_head, current_raw_head, tail, hw_tail);

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

#[cfg(target_arch = "x86_64")]
unsafe fn bring_up_blt_ring(bar0: usize, active_surf: Option<u32>) {
    if BLT_RING.lock().is_some() { return; }
    'ring: {
        // The outermost refusal arm — SEAT FIXUP round 3, and the census's own first metal boot
        // (Boot X) is why it exists: on the dual-GPU rMBP the gmux routes the panel to the KEPLER,
        // every iGPU plane reads zero, `active_surf` is None — and the census printed NOTHING,
        // the one outcome the playbook called worst. The fb the console draws into is Kepler
        // VRAM, which this blitter cannot reach through the iGPU GGTT: the acceleration is
        // structurally confined to boots where the iGPU owns a scanout (gmux switched, or
        // iGPU-only machines). This line makes that fact one awk away instead of an absence.
        let Some(surf) = active_surf else {
            serial_println!(":: igpu-blt: ring=absent why=no-active-surface — every iGPU display plane is off (gmux routes the panel elsewhere); CPU path carries the console ::");
            break 'ring;
        };
        {
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
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
#[derive(Clone, Copy)]
enum UnwindEntry {
    Mmio { off: u32, pre: u32 },
    Gmux { reg: u8, pre: u8 },
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
struct DisplayUnwind {
    bar0: usize,
    entries: [UnwindEntry; 32],
    len: usize,
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
impl DisplayUnwind {
    fn new(bar0: usize) -> Self {
        Self { bar0, entries: [UnwindEntry::Mmio { off: 0, pre: 0 }; 32], len: 0 }
    }

    unsafe fn execute(&mut self) -> bool {
        let mut all_ok = true;
        while self.len > 0 {
            self.len -= 1;
            let entry = &self.entries[self.len];
            match entry {
                UnwindEntry::Mmio { off, pre } => {
                    core::ptr::write_volatile((self.bar0 + *off as usize) as *mut u32, *pre);
                }
                UnwindEntry::Gmux { reg, pre } => {
                    let ok = gmux_index_write(*reg, *pre);
                    if !ok {
                        all_ok = false;
                    }
                }
            }
        }
        all_ok
    }

    unsafe fn push_mmio(&mut self, off: usize, pre: u32) {
        if self.len < 32 {
            self.entries[self.len] = UnwindEntry::Mmio { off: off as u32, pre };
            self.len += 1;
        }
    }

    unsafe fn push_gmux(&mut self, reg: u8, pre: u8) {
        if self.len < 32 {
            self.entries[self.len] = UnwindEntry::Gmux { reg, pre };
            self.len += 1;
        }
    }
}


#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_NATIVE_READ: u32 = 0x9;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_I2C_WRITE: u32 = 0x0;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_I2C_READ: u32 = 0x1;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_I2C_MOT: u32 = 0x4;

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_CH_CTL_SEND_BUSY: u32 = 1 << 31;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_CH_CTL_DONE: u32 = 1 << 30;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_CH_CTL_TIME_OUT_ERROR: u32 = 1 << 28;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_CH_CTL_TIME_OUT_1600US: u32 = 3 << 26;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DP_AUX_CH_CTL_RECEIVE_ERROR: u32 = 1 << 25;

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn dp_aux_transfer(bar0: usize, clock_divider: u32, cmd: u32, addr: u32, tx_len: u32, tx_data: &[u8], rx_data: &mut [u8], window_deadline: u64) -> Result<(), (&'static str, u32)> {
    let mut retry_count = 0;

    let is_write = (cmd & 0x3) == 0;
    let send_bytes = if is_write { 4 + tx_len } else { 4 };

    loop {
        if crate::arch::now_cycles() >= window_deadline || retry_count >= 7 {
            return Err(("aux-defer-exhausted", 0));
        }

        let mut status = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *const u32);
        if (status & DP_AUX_CH_CTL_SEND_BUSY) != 0 {
            core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *mut u32, (status & !DP_AUX_CH_CTL_SEND_BUSY) | DP_AUX_CH_CTL_DONE | DP_AUX_CH_CTL_TIME_OUT_ERROR | DP_AUX_CH_CTL_RECEIVE_ERROR);
            status = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *const u32);
            if (status & DP_AUX_CH_CTL_SEND_BUSY) != 0 {
                return Err(("aux-defer-busy", status));
            }
        }

        let data1 = (cmd << 28) | ((addr & 0xFFFFF) << 8) | tx_len.saturating_sub(1);
        core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *mut u32, data1);

        if is_write && tx_len > 0 {
            let mut w_idx = 0;
            let data_regs = [regs::DPA_AUX_CH_DATA2, regs::DPA_AUX_CH_DATA3, regs::DPA_AUX_CH_DATA4, regs::DPA_AUX_CH_DATA5];
            for reg_offset in &data_regs {
                if w_idx >= tx_len { break; }
                let mut val = 0u32;
                for b in 0..4 {
                    if w_idx < tx_len {
                        val |= (tx_data[w_idx as usize] as u32) << (24 - (b * 8));
                        w_idx += 1;
                    }
                }
                core::ptr::write_volatile((bar0 + *reg_offset) as *mut u32, val);
            }
        }

        let ctl = DP_AUX_CH_CTL_SEND_BUSY | DP_AUX_CH_CTL_DONE | DP_AUX_CH_CTL_TIME_OUT_ERROR | DP_AUX_CH_CTL_RECEIVE_ERROR | DP_AUX_CH_CTL_TIME_OUT_1600US | (send_bytes << 20) | (clock_divider & 0x7FF);
        core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *mut u32, ctl);

        let wait_deadline = crate::arch::now_cycles() + (crate::arch::hw_wait_budget() / 100);
        loop {
            status = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *const u32);
            if (status & DP_AUX_CH_CTL_SEND_BUSY) == 0 { break; }
            if crate::arch::now_cycles() >= wait_deadline { break; }
            core::hint::spin_loop();
        }

        if (status & DP_AUX_CH_CTL_SEND_BUSY) != 0 {
            return Err(("aux-timeout-busy", status));
        }

        let status_clean = status & !DP_AUX_CH_CTL_SEND_BUSY;
        core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *mut u32, status_clean | DP_AUX_CH_CTL_DONE | DP_AUX_CH_CTL_TIME_OUT_ERROR | DP_AUX_CH_CTL_RECEIVE_ERROR);

        if (status & DP_AUX_CH_CTL_TIME_OUT_ERROR) != 0 {
            return Err(("aux-timeout-error", status));
        }
        if (status & DP_AUX_CH_CTL_RECEIVE_ERROR) != 0 {
            return Err(("aux-receive-error", status));
        }

        let rx_data1 = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *const u32);
        let reply_nibble = (rx_data1 >> 28) & 0xF;
        let native_reply = reply_nibble & 0x3;
        let i2c_reply = (reply_nibble >> 2) & 0x3;

        let is_i2c = (cmd & 0x8) == 0;
        let reply_status = if is_i2c { i2c_reply } else { native_reply };

        if reply_status == 3 {
            return Err(("aux-reserved-reply", status));
        }

        if reply_status == 2 {
            retry_count += 1;
            continue;
        }

        if reply_status == 1 {
            return Err(("aux-nack", status));
        }

        let total_rx = (status >> 20) & 0x1F;
        let payload_rx = total_rx.saturating_sub(1) as usize;

        if !is_write && payload_rx != rx_data.len() {
            return Err(("aux-short-read", status));
        }

        if payload_rx > 0 && !is_write {
            let to_copy = core::cmp::min(payload_rx, rx_data.len());
            let mut r_idx = 0;

            for b in 1..4 {
                if r_idx < to_copy {
                    rx_data[r_idx] = ((rx_data1 >> (24 - (b * 8))) & 0xFF) as u8;
                    r_idx += 1;
                }
            }
            let data_regs = [regs::DPA_AUX_CH_DATA2, regs::DPA_AUX_CH_DATA3, regs::DPA_AUX_CH_DATA4, regs::DPA_AUX_CH_DATA5];
            for reg_offset in &data_regs {
                if r_idx >= to_copy { break; }
                let val = core::ptr::read_volatile((bar0 + *reg_offset) as *const u32);
                for b in 0..4 {
                    if r_idx < to_copy {
                        rx_data[r_idx] = ((val >> (24 - (b * 8))) & 0xFF) as u8;
                        r_idx += 1;
                    }
                }
            }
        }

        return Ok(());
    }
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
pub unsafe fn gmux_igd_switch() {
    let ladder_start = crate::arch::now_cycles();
    let get_elapsed_ms = || (crate::arch::now_cycles().wrapping_sub(ladder_start)) / (crate::arch::hw_wait_budget() / 2000);

    let mut highest = 0;
    let mut ok_flag = 0;
    let mut why_str = "none";
    let mut rung_name = "harness";
    let mut unwind = DisplayUnwind::new(0);

    let mut out_dpcd: Option<u8> = None;
    let mut out_edid: Option<[u8; 128]> = None;
    let mut out_pre_dpcd: Option<u8> = None;
    let mut out_pre_dpcd_err: Option<&'static str> = None;
    let mut out_pre_dpcd_status = 0;
    let mut pp_before: Option<(u32, u32)> = None;
    let mut pp_after: Option<(u32, u32)> = None;
    let mut mux_reads = (0, 0, 0);
    // GMUXDPCD: the three things flight 10's `why=aux-timeout-error` could not tell an operator
    // apart, hoisted out of the harness closure so the LADDER line carries them on EVERY exit
    // path. `aux_div` is the divider the rung actually programmed into `DPA_AUX_CH_CTL[10:0]`
    // (inherited from firmware's own `aux_ctl`, never invented here) and, with the `aux_port=`
    // token, says WHICH channel was driven; `dpcd_tries` is how many AUX attempts rung 4 spent;
    // `pp_settle_ms` is how long rung 3 waited between the mux write and the first attempt. A
    // capture carrying only `why=aux-timeout-error` cannot separate "wrong channel", "sink
    // unpowered" and "asked too soon" — these three tokens can.
    let mut aux_div = 0u32;
    let mut dpcd_tries = 0u32;
    let mut pp_settle_ms = 0u64;
    // GMUX7: rung 07's verdict, hoisted out of the harness closure for the same reason GMUXDPCD
    // hoisted the three above — the rollup must carry it on EVERY exit path. It is the token that
    // says the ladder STOPPED SHORT OF PANEL POWER and why, so a capture that reaches
    // `name=end ok=1` can never be misread as "the panel was powered and nothing happened".
    let mut pps_verdict = "not-reached";

    // Will be captured dynamically based on live pre-image
    let mut pre_ddc: Option<u32> = None;
    let mut pre_disp: Option<u32> = None;
    // The EXTERNAL pre-image, split: `pre_ext` is the SWITCH_EXTERNAL target register (0x40) and
    // `pre_ext_status` is the READ_EXTERNAL status register (0x41). GMUX-2: only the STATUS half is
    // a state read, so only the STATUS half is gated and only the STATUS half is compared by the
    // MATCH verdict. `pre_ext` is captured to be PRINTED — the value a write-target port returns on
    // a Kepler-owned boot is a metal fact worth a witness — and is never written back.
    let mut pre_ext: Option<u32> = None;
    let mut pre_ext_status: Option<u32> = None;
    // GMUX-2 restore policy for SWITCH_EXTERNAL (0x40), decided by `ext_restore_target` from the
    // 0x41 STATUS read: `Some(v)` means the status→write encoding map is known and cited for that
    // value and the unwind pushes `v`; `None` means SKIPPED — the pre-switch read of 0x40 is not a
    // state, so there is nothing this code may honestly write back there. `None` is also the value
    // when the gate refused and the unwind never carried an EXTERNAL entry at all.
    let mut ext_restore: Option<u8> = None;
    let mut mux_touched = false;

    let mut execute_harness = || -> Result<(), (&'static str, u32)> {
        if !PROTOCOL_PROVEN.load(Ordering::SeqCst) { return Err(("protocol-unproven", 0)); }
        let bar0 = IGPU_BAR0.load(Ordering::SeqCst);
        if bar0 == 0 { return Err(("bar0-unmapped", 0)); }
        unwind.bar0 = bar0;

        let p_ddc = gmux_index_read(GMUX_SWITCH_DDC);
        let p_disp = gmux_index_read(GMUX_SWITCH_DISPLAY);
        let p_ext = gmux_index_read(GMUX_SWITCH_EXTERNAL);
        let disp = gmux_index_read(GMUX_READ_DISPLAY);
        let ext = gmux_index_read(GMUX_READ_EXTERNAL);
        // GMUX-2: the census reads BOTH halves of the EXTERNAL pair and prints both. `SW_EXT` is
        // the write-target port 0x40; `SW_EXT_ST` is the status port 0x41 — the same register the
        // pre-existing `EXT=` field carries, repeated under the name that says which port it is,
        // because `EXT=` alone never told a reader that `SW_EXT`'s pair-mate was already on the
        // wire. Every pre-existing field keeps its name, its value and its order: flight 5's
        // capture and the register's G6 row both key on this line. `sw_ext_state=` likewise stays
        // verbatim — it is `ext_state_name` applied to the write-target port, which is exactly the
        // category error this commit removes from the GATE, kept only so old and new captures
        // compare field for field. `gate=` is the verdict that now decides.
        let gate = gmux_preswitch_decode(p_ddc, disp, ext);
        serial_println!(":: igpu-dpy: pre-switch state DDC=0x{:02X} SW_DISP=0x{:02X} SW_EXT=0x{:02X} SW_EXT_ST=0x{:02X} DISP=0x{:02X} EXT=0x{:02X} sw_ext_state={} ext_state={} gate={} ::",
            p_ddc, p_disp, p_ext, ext, disp, ext, ext_state_name(p_ext), ext_state_name(ext), gate.token());

        // THE GATE SCORES STATE READS ONLY, AND THE UNWIND RESTORES ONLY WHAT IT READ AS STATE.
        // The two halves are one property and must be read together. See `gmux_preswitch_decode`
        // and `ext_restore_target` at the foot of this file for the register-level reasoning and
        // the port-pair citations; the decode is a pure function and is pinned by `const _` there.
        //
        // What flight 5 cost, in one line: the term that refused was `SW_EXT` — the read of the
        // WRITE-TARGET port 0x40 — scored against the STATUS encodings. 0x40 is not a state read,
        // so `0x01` was never a mux state to begin with, and `gmux_preswitch_decode` cannot repeat
        // the error because 0x40 IS NOT ONE OF ITS ARGUMENTS.
        //
        // SWITCH_DISPLAY (`p_disp`) stays printed and ungated for the reason the review condition
        // already gave: it is a write-target port too, it has never been captured on this machine,
        // and it is not a restore value (the unwind writes the constant DIS).
        if !gate.accepted() {
            // Buffer the REFUSED print; the outer error handler will print it. The `why` token is
            // unchanged — the register's G6 row and every existing capture key on
            // `pre-switch-not-accepted` — and the discrimination the operator needs (WHICH port,
            // and REFUSE versus UNREADABLE) is on the witness line above as `gate=`. UNREADABLE is
            // a refusal, not a pass: a port that did not answer has not said the mux is safe to
            // move.
            return Err(("pre-switch-not-accepted", 0));
        }
        pre_ddc = Some(p_ddc);
        pre_disp = Some(p_disp);
        pre_ext = Some(p_ext);
        pre_ext_status = Some(ext);
        ext_restore = ext_restore_target(ext);
        if ext_restore.is_none() {
            serial_println!(":: igpu-dpy: restore ext=SKIPPED (write-target port, no state read) SW_EXT=0x{:02X} SW_EXT_ST=0x{:02X} ext_state={} ::",
                p_ext, ext, ext_state_name(ext));
        }

        rung_name = "census";
        let ggc = crate::arch::pci::read_config_32(0, 0, 0, 0x50);
        let bdsm = crate::arch::pci::read_config_32(0, 0, 0, 0xB0);
        let ggtt0 = mmio_read(bar0, regs::GTT_BASE);
        let ggtt1 = mmio_read(bar0, regs::GTT_BASE + 4);
        let aux_ctl = mmio_read(bar0, regs::DPA_AUX_CH_CTL);
        let frmcnt = mmio_read(bar0, 0x70040);

        if (aux_ctl & DP_AUX_CH_CTL_SEND_BUSY) != 0 {
            serial_println!(":: igpu: [AUX] REFUSED: aux_ctl=0x{:08X} SEND_BUSY is set at boot bdsm=0x{:08X} ggc=0x{:08X} ggtt0=0x{:08X} ggtt1=0x{:08X} frmcnt=0x{:08X} ::", aux_ctl, bdsm, ggc, ggtt0, ggtt1, frmcnt);
            return Err(("aux-busy-at-boot", 0));
        }

        let clock_divider = aux_ctl & 0x7FF;
        if clock_divider == 0 {
            serial_println!(":: igpu: [AUX] REFUSED: aux_ctl=0x{:08X} clock divider is 0 bdsm=0x{:08X} ggc=0x{:08X} ggtt0=0x{:08X} ggtt1=0x{:08X} frmcnt=0x{:08X} ::", aux_ctl, bdsm, ggc, ggtt0, ggtt1, frmcnt);
            return Err(("aux-divider-unusable", 0));
        }

        // GMUXDPCD: the divider is INHERITED, and that is the point — `clock_divider` is
        // firmware's own `aux_ctl[10:0]`, so it is whatever the part was left programmed with for
        // the channel at `regs::DPA_AUX_CH_CTL`. Hoisting it (and the offset, on the LADDER line's
        // `aux_port=` token) is what turns "AUX timed out" into a statement about a NAMED channel.
        aux_div = clock_divider;
        serial_println!(":: igpu-dpy: rung=00 name=census ok=1 bdsm=0x{:08X} ggc=0x{:08X} ggtt0=0x{:08X} ggtt1=0x{:08X} aux_ctl=0x{:08X} frmcnt=0x{:08X} ::",
            bdsm, ggc, ggtt0, ggtt1, aux_ctl, frmcnt);

        highest = 1;
        rung_name = "selftest";
        // Note: the MMIO self-test can pass vacuously if the register is Read-As-Zero (RAZ).
        serial_println!(":: igpu: [GMUX] running Unwind stack self-test ::");

        let test_val = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *const u32);
        unwind.push_mmio(regs::DPA_AUX_CH_DATA1, test_val);
        core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *mut u32, !test_val);

        // Push order EXTERNAL, DISPLAY, DDC so the LIFO unwind restores DDC, DISPLAY, EXTERNAL —
        // the SAME order as the forward writes (review condition: an earlier comment called this
        // "the reverse", which is false — upstream apple-gmux uses DDC→DISPLAY→EXTERNAL in both
        // directions, and so does this). GMUX-2: EXTERNAL is pushed ONLY when `ext_restore_target`
        // mapped the 0x41 STATUS read back to a named write encoding; otherwise no EXTERNAL entry
        // exists at all and the SKIPPED line above says so. It is NEVER restored from the 0x40
        // read — that is a write-target port, `0x01` there is not a state, and pushing it would be
        // the silent state change the EXTERNAL doc-comment forbids. DDC and DISPLAY are restored
        // to DIS — for DDC the only value its gate admits, for DISPLAY the constant upstream
        // restores.
        if let Some(v) = ext_restore { unwind.push_gmux(GMUX_SWITCH_EXTERNAL, v); }
        unwind.push_gmux(GMUX_SWITCH_DISPLAY, GMUX_DISPLAY_DIS);
        unwind.push_gmux(GMUX_SWITCH_DDC, GMUX_DDC_DIS);

        let _ = unwind.execute();

        let test_val_after = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *const u32);
        if test_val_after != test_val {
            serial_println!(":: igpu: [GMUX] Unwind stack MMIO self-test FAILED (expected 0x{:08X}, got 0x{:08X}) ::", test_val, test_val_after);
            return Err(("unwind-mmio-failed", 0));
        }
        serial_println!(":: igpu: [GMUX] Unwind stack MMIO self-test passed ::");
        serial_println!(":: igpu: [GMUX] Unwind stack gmux-dispatch=REACHED (Gmux restore path executed without faulting, not implying restore verified) ::");

        // Push order EXTERNAL, DISPLAY, DDC so the LIFO unwind restores DDC, DISPLAY, EXTERNAL —
        // the SAME order as the forward writes (review condition: an earlier comment called this
        // "the reverse", which is false — upstream apple-gmux uses DDC→DISPLAY→EXTERNAL in both
        // directions, and so does this). GMUX-2: EXTERNAL is pushed ONLY when `ext_restore_target`
        // mapped the 0x41 STATUS read back to a named write encoding; otherwise no EXTERNAL entry
        // exists at all and the SKIPPED line above says so. It is NEVER restored from the 0x40
        // read — that is a write-target port, `0x01` there is not a state, and pushing it would be
        // the silent state change the EXTERNAL doc-comment forbids. DDC and DISPLAY are restored
        // to DIS — for DDC the only value its gate admits, for DISPLAY the constant upstream
        // restores.
        if let Some(v) = ext_restore { unwind.push_gmux(GMUX_SWITCH_EXTERNAL, v); }
        unwind.push_gmux(GMUX_SWITCH_DISPLAY, GMUX_DISPLAY_DIS);
        unwind.push_gmux(GMUX_SWITCH_DDC, GMUX_DDC_DIS);

        // F1: ASSIGN out_pre_dpcd_err, out_pre_dpcd_status, out_pre_dpcd before the first gmux write.
        let mut pre_dpcd_rev = [0u8; 1];
        let pre_deadline = crate::arch::now_cycles() + (crate::arch::hw_wait_budget() * 1);
        if let Err((e, stat)) = dp_aux_transfer(bar0, clock_divider, DP_AUX_NATIVE_READ, 0x00000, 1, &[], &mut pre_dpcd_rev, pre_deadline) {
            out_pre_dpcd_err = Some(e);
            out_pre_dpcd_status = stat;
        } else {
            out_pre_dpcd = Some(pre_dpcd_rev[0]);
        }

        // F2: ASSIGN pp_before before the first gmux write.
        pp_before = Some((mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS)));

        highest = 2;
        rung_name = "switch";
        mux_touched = true;

        // F6: Forward writes to DDC, DISPLAY, EXTERNAL.
        gmux_index_write(GMUX_SWITCH_DDC, GMUX_DDC_IGD);
        gmux_index_write(GMUX_SWITCH_DISPLAY, GMUX_DISPLAY_IGD);
        gmux_index_write(GMUX_SWITCH_EXTERNAL, GMUX_EXTERNAL_IGD);

        mux_reads.0 = gmux_index_read(GMUX_SWITCH_DDC);
        mux_reads.1 = gmux_index_read(GMUX_SWITCH_DISPLAY);
        mux_reads.2 = gmux_index_read(GMUX_SWITCH_EXTERNAL);

        // Review condition: only DDC's read-back may abort the flight. SWITCH_DISPLAY and
        // SWITCH_EXTERNAL have never been read back on this machine, and write-side switch ports
        // that do not echo their value would fail this comparison on a switch that WORKED —
        // blanking the panel and aborting before the first AUX transaction, the whole round spent
        // and nothing learned. DDC's echo is metal-proven and stays strict; the other two are
        // advisory (the [GMUX] witness prints all three), and the AUX/EDID rungs below are the
        // actual test of whether the mux moved.
        if mux_reads.0 != GMUX_DDC_IGD as u32 {
            return Err(("mux-switch-failed", 0));
        }

        // ═══════════════════════════════════════════════════════════════════════════════════
        // RUNG 3 — `pp`: THE PANEL-POWER FRAME, AND THE SETTLE THE SWITCH OWES THE SINK.
        //
        // It writes NOTHING. That is a finding, not an omission, and both halves of the reason
        // are on the wire below.
        //
        // WHY NO PPS WRITE (half one — the citation is missing). Raising panel VDD means setting
        // a named bit in `PCH_PP_CONTROL` (0xC7204). This tree does not carry a legal bit map for
        // that register. What it carries is: the LOCATION (`igpu.rs` census, "[CITATION: Intel PRM
        // Vol 3, South Display Engine Registers] GMBUS and Panel Power Sequencer (PPS) are
        // PCH-attached"), and the 0xABCD unlock KEY as a Wall-D entropy pattern (`gen7.rs`
        // CONTROL_FRAME row `PCH_PP_CONTROL_KEY`, the one CRITICAL MMIO row of that frame).
        // `docs/dev/OS/08_VIDEO/gen7.md` names 0xC7204 once and only to say it is READ ONLY; the
        // cleanroom `gpu_spec.md` does not name it at all. The one place in tree that names bit 0
        // / bit 2 / bit 3 — `docs/dev/GEMINI/video/iGUI/LADDER-igpu-bringup.md` rung 2 — marks the
        // whole map **TBV** and sources it to i915 `intel_pps.c` NAMING, then names the document
        // still needed: PRM Vol 3 Part 4 "Panel Power Sequencing". A guessed bit in a panel-power
        // sequencer is the one guess on this machine that can end at damaged hardware, and the
        // same doc says why in one line: `PP_ON_DELAYS`/`PP_OFF_DELAYS` read 0 on this part, so
        // the panel's T1..T12 are NOT programmed, and "firing the PPS with zero delays is the
        // single most likely way to damage or hard-hang the panel". So the write is declined and
        // the declension is printed with its reason token.
        //
        // WHY NO PPS WRITE (half two — METAL SAYS IT IS NOT NEEDED). Flight 8 (2026-09-16) ran
        // this ladder to `LADDER highest=05/10 name=end ok=1`, which this file only reaches after
        // the DPCD native read, the I2C-over-AUX EDID address write, eight 16-byte EDID reads, the
        // EDID header compare and the checksum — i.e. **the eDP sink answered AUX**, with the PPS
        // in exactly the state flight 10 timed out under (`PP_CONTROL_PCH=0xABCD0008`,
        // `PP_STATUS=0x00000000`). A sink that answers with the sequencer untouched is not a sink
        // waiting on VDD. The 0xABCD0008 = forced-VDD reading is therefore CORROBORATED by flight
        // 8, not falsified — and the flight 9/10 timeouts are an INTERMITTENT fault, which "VDD is
        // off" cannot be.
        //
        // WHAT IS LEFT, AND IS BUILT HERE. Flight 10's own timestamps: the gmux switch and the
        // first DPCD attempt are stamped in the SAME millisecond (`[26522ms]` for both the
        // `[GMUX] switched` line and the AUX verdict), and the whole ladder is `elapsed_ms=9`. The
        // ladder re-routes the panel's AUX pair electrically and then interrogates the sink with
        // no settle at all, once, aborting on the first 1600 µs hardware timeout. So rung 3 spends
        // a bounded settle — writing nothing, reading the PPS across it so the wait is itself a
        // measurement — and rung 4 retries instead of asking once.
        highest = 3;
        rung_name = "pp";
        let cycles_per_ms = crate::arch::hw_wait_budget() / 2000;

        let pp_ctl_entry = mmio_read(bar0, regs::PCH_PP_CONTROL);
        let pp_sts_entry = mmio_read(bar0, regs::PCH_PP_STATUS);
        let pp_on_entry = mmio_read(bar0, regs::PCH_PP_ON_DELAYS);
        let pp_off_entry = mmio_read(bar0, regs::PCH_PP_OFF_DELAYS);
        let pp_div_entry = mmio_read(bar0, regs::PCH_PP_DIVISOR);

        // Wall D for this rung, borrowed whole from `gen7.rs`'s CONTROL_FRAME: the 0xABCD unlock
        // key in 31:16 is sixteen bits of entropy that no dead bus, floating line or zero-filled
        // window can produce by accident, and it does not move with panel power state. Without it
        // every other pp_* number on this line is noise and must not be read as a panel state.
        // Note the asymmetry that keeps it honest: the KEY is scored, the LOW byte is not —
        // `gen7.rs` learned that the hard way (review caught that a legitimate 0xABCD0008 ->
        // 0xABCD0009 would have voided its control frame and burned a boot), and this inherits it.
        //
        // ⚠ IT IS A TOKEN, NOT A REFUSAL, AND THAT IS DELIBERATE (R19). The PPS is in the SOUTH
        // display engine; `DPA_AUX_CH_CTL` (0x64010) is in the NORTH one. A south window that is
        // not decoding says nothing about whether the AUX channel decodes, so refusing here would
        // shut out rungs 4 and 5 — which flight 8 proves can pass — on the strength of a register
        // neither of them uses. This rung writes nothing and so cannot fail: it reports.
        let pp_window = if (pp_ctl_entry & 0xFFFF_0000) == 0xABCD_0000 { "KEYED" } else { "DEAD" };

        // `delays_programmed` is the second, independent blocker on any future PPS write, and it
        // is measured rather than remembered: a boot whose firmware DOES program the T-delays
        // would print 1 here and that is the day this rung's write half becomes designable.
        let delays_programmed = if pp_on_entry != 0 && pp_off_entry != 0 { 1 } else { 0 };
        serial_println!(":: igpu-dpy: rung=03 name=pp ok=1 pp_write=DECLINED why=pp-bits-uncited pp_window={} delays_programmed={} pp_unwind=0 pp_ctl=0x{:08X} pp_sts=0x{:08X} on_delays=0x{:08X} off_delays=0x{:08X} div=0x{:08X} ::",
            pp_window, delays_programmed, pp_ctl_entry, pp_sts_entry, pp_on_entry, pp_off_entry, pp_div_entry);

        // The settle. TSC-bounded, never `arch::ms()`-bounded: `now_cycles()` advances regardless
        // of EFLAGS.IF or whether the APIC-timer ISR runs, and a panel delay measured on a stopped
        // clock is either instantaneous or infinite. `PP_SETTLE_MS` is a BUDGET, not a cited T3 —
        // it is named that way at its definition and on the wire, because this tree cannot cite a
        // T3 either. Reading the PPS across the wait makes the wait falsifiable: `moved=1` would
        // say the sequencer is live and sequencing on its own, `moved=0` that it is static and the
        // settle is buying the SINK time, not the sequencer.
        let settle_deadline = crate::arch::now_cycles() + cycles_per_ms.saturating_mul(PP_SETTLE_MS);
        while crate::arch::now_cycles() < settle_deadline {
            core::hint::spin_loop();
        }
        pp_settle_ms = PP_SETTLE_MS;
        let pp_ctl_settled = mmio_read(bar0, regs::PCH_PP_CONTROL);
        let pp_sts_settled = mmio_read(bar0, regs::PCH_PP_STATUS);
        let pp_moved = if pp_ctl_settled != pp_ctl_entry || pp_sts_settled != pp_sts_entry { 1 } else { 0 };
        serial_println!(":: igpu-dpy: rung=03 name=pp SETTLE ms={} budget=not-a-cited-T3 moved={} pp_ctl=0x{:08X}->0x{:08X} pp_sts=0x{:08X}->0x{:08X} elapsed_ms={} ::",
            PP_SETTLE_MS, pp_moved, pp_ctl_entry, pp_ctl_settled, pp_sts_entry, pp_sts_settled, get_elapsed_ms());

        highest = 4;
        rung_name = "dpcd";
        let window_deadline = crate::arch::now_cycles() + (crate::arch::hw_wait_budget() * 1);

        // GMUXDPCD: the software retry window. The HARDWARE timeout is already at its maximum
        // encoding and has been since before flight 8 — `DP_AUX_CH_CTL_TIME_OUT_1600US` is
        // `3 << 26`, and flight 10's own failing status `0x5D4000C8` carries bits 27:26 = 0b11,
        // so the 1600 µs arm of the 400/600/800/1600 µs field was the one in force. There is
        // nothing left to lengthen on the hardware side; the only remaining lever is to ASK
        // AGAIN. `dp_aux_transfer` already retries seven deep, but only on an AUX DEFER reply —
        // a TIME_OUT_ERROR returns to the caller on the first occurrence, and flight 10's dpcd
        // rung (numbered 3 then, 4 now) called it exactly ONCE — which is why its whole ladder is
        // `elapsed_ms=9`. `DPCD_TRIES` mirrors that same in-file seven (it is inherited
        // from this file's own defer loop, NOT cited to the DP spec, and is named that way at its
        // definition). Every attempt prints its own verdict WITH the PPS read at that instant, so
        // a capture where attempt 1 fails and attempt 3 succeeds settles the intermittency
        // question by itself, and one where all seven fail with byte-identical status settles it
        // the other way.
        let mut dpcd_rev = [0u8; 1];
        let mut dpcd_err: Option<(&'static str, u32)> = None;
        for attempt in 1..=DPCD_TRIES {
            dpcd_tries = attempt;
            match dp_aux_transfer(bar0, clock_divider, DP_AUX_NATIVE_READ, 0x00000, 1, &[], &mut dpcd_rev, window_deadline) {
                Ok(()) => {
                    dpcd_err = None;
                    break;
                }
                Err((e, stat)) => {
                    dpcd_err = Some((e, stat));
                    serial_println!(":: igpu-dpy: rung=04 name=dpcd try={}/{} ok=0 why={} status=0x{:08X} pp_ctl=0x{:08X} pp_sts=0x{:08X} elapsed_ms={} ::",
                        attempt, DPCD_TRIES, e, stat,
                        mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS),
                        get_elapsed_ms());
                    if attempt >= DPCD_TRIES || crate::arch::now_cycles() >= window_deadline {
                        break;
                    }
                    let gap_deadline = crate::arch::now_cycles() + cycles_per_ms.saturating_mul(DPCD_RETRY_GAP_MS);
                    while crate::arch::now_cycles() < gap_deadline {
                        core::hint::spin_loop();
                    }
                }
            }
        }
        if let Some(e) = dpcd_err {
            pp_after = Some((mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS)));
            return Err(e);
        }
        serial_println!(":: igpu-dpy: rung=04 name=dpcd try={}/{} ok=1 dpcd_rev=0x{:02X} elapsed_ms={} ::",
            dpcd_tries, DPCD_TRIES, dpcd_rev[0], get_elapsed_ms());
        out_dpcd = Some(dpcd_rev[0]);

        highest = 5;
        rung_name = "edid";
        if let Err(e) = dp_aux_transfer(bar0, clock_divider, DP_AUX_I2C_WRITE | DP_AUX_I2C_MOT, 0x50, 1, &[0x00], &mut [], window_deadline) {
            pp_after = Some((mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS)));
            return Err(e);
        }

        let mut edid = [0u8; 128];
        for chunk in 0..8 {
            let offset = chunk * 16;
            let cmd = if chunk == 7 { DP_AUX_I2C_READ } else { DP_AUX_I2C_READ | DP_AUX_I2C_MOT };
            if let Err(e) = dp_aux_transfer(bar0, clock_divider, cmd, 0x50, 16, &[], &mut edid[offset..offset + 16], window_deadline) {
                pp_after = Some((mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS)));
                return Err(e);
            }
        }

        pp_after = Some((mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS)));
        out_edid = Some(edid);

        let header: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
        if &edid[0..8] != &header {
            return Err(("edid-header-corrupt", 0));
        }

        let checksum: u8 = edid.iter().fold(0, |acc, &x| acc.wrapping_add(x));
        if checksum != 0 {
            return Err(("edid-checksum-bad", 0));
        }

        // ═══════════════════════════════════════════════════════════════════════════════════
        // RUNG 06 — `link`: WHAT THE SINK SAYS IT CAN DO, READ AND PRINTED DECODED.
        //
        // Flight 11 (2026-09-22) is why this rung exists and why it is FIRST of the three. It ran
        // `rung=03 name=pp … SETTLE ms=210 … moved=0` → `rung=04 name=dpcd try=1/7 ok=1
        // dpcd_rev=0x11 elapsed_ms=226` → `LADDER highest=06/10 name=end ok=1 pending=2
        // gmux=MATCH why=none elapsed_ms=231`. GMUXDPCD's timing theory is CONFIRMED: after the
        // settle the FIRST AUX try answered, the EDID read, and the ladder ended at its last built
        // rung for the first time on this machine. The ladder was out of rungs, not out of sink.
        //
        // WHAT IT READS, AND WHERE EVERY ADDRESS COMES FROM. Two native AUX reads, no writes to
        // anything but the AUX transaction registers rungs 04 and 05 already write:
        //
        //   * DPCD `0x00000..0x0000F` — the cap window this tree already names, verbatim, at
        //     `docs/dev/GEMINI/video/iGUI/LADDER-igpu-bringup.md:318-319` ("Native AUX read of DPCD
        //     `0x00000..0x0000F` → `DPCD_REV`, `MAX_LINK_RATE`, `MAX_LANE_COUNT`, `MAX_DOWNSPREAD`,
        //     `eDP_CONFIGURATION_CAP`"). The brief asked for `0x000-0x00B`; the window is the
        //     tree-cited 16 bytes because `eDP_CONFIGURATION_CAP` — which the brief names in the
        //     same breath — is at `0x00D`, OUTSIDE `0x000-0x00B`. One transfer, not two, is also
        //     one failure point rather than two.
        //   * DPCD `0x00202..0x00207` — `LANE0_1_STATUS`, `LANE2_3_STATUS`,
        //     `LANE_ALIGN_STATUS_UPDATED`, `SINK_STATUS`, `ADJUST_REQUEST_LANE0_1`,
        //     `ADJUST_REQUEST_LANE2_3`, cited to the same document at :420-422, which marks the
        //     whole set **NEEDS-VERIFICATION against the DP 1.1/1.2 spec**. So the BYTES are
        //     printed and the BIT meanings are not claimed: `bits=uncited` is on the wire.
        //
        // ⚠ THE ONE DECODE THIS RUNG REFUSES TO MAKE, and it is a finding. The only MAX_LINK_RATE
        // map in this tree (`LADDER-igpu-bringup.md:318-319`) reads `0x0A`=1.62, `0x14`=2.7 Gbps —
        // and that same document marks its DP-side facts NEEDS-VERIFICATION at :420-422. It names
        // no encoding at all for `0x06`. A link rate is the number that sets the PLL frequency
        // (:177 of that doc), so converting the raw byte to MHz on an unverified table is exactly
        // the "silent wrongness downstream" that document's own rung-3 section warns about. The
        // rung therefore prints `rate_raw=` verbatim and `rate_decode=TBV-tree-table`, and the
        // NEXT executor's first job is one PRM/DP-spec line, not one more boot.
        //
        // It cannot black the panel: it writes no display register, no PPS register, no gmux
        // register, and adds no unwind entry. Its write envelope is exactly rung 04's.
        highest = 6;
        rung_name = "link";

        let mut dpcd_cap = [0u8; 16];
        if let Err(e) = dp_aux_transfer(bar0, clock_divider, DP_AUX_NATIVE_READ, 0x00000, 16, &[], &mut dpcd_cap, window_deadline) {
            pp_after = Some((mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS)));
            return Err(e);
        }

        let mut link_status = [0u8; 6];
        if let Err(e) = dp_aux_transfer(bar0, clock_divider, DP_AUX_NATIVE_READ, 0x00202, 6, &[], &mut link_status, window_deadline) {
            pp_after = Some((mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS)));
            return Err(e);
        }
        pp_after = Some((mmio_read(bar0, regs::PCH_PP_CONTROL), mmio_read(bar0, regs::PCH_PP_STATUS)));

        let cap_rev = dpcd_cap[0x0];
        let cap_rate = dpcd_cap[0x1];
        let cap_lane_byte = dpcd_cap[0x2];
        let cap_lanes = cap_lane_byte & 0x1F;
        let cap_enhanced = (cap_lane_byte >> 7) & 0x1;
        let cap_tps3 = (cap_lane_byte >> 6) & 0x1;
        let cap_spread_byte = dpcd_cap[0x3];
        let cap_downspread = cap_spread_byte & 0x1;
        let cap_no_aux_tl = (cap_spread_byte >> 6) & 0x1;
        let cap_edp_cfg = dpcd_cap[0xD];
        // `MAX_LANE_COUNT & 0x1F ∈ {1,2,4}` is the structural-plausibility predicate the same
        // design document states at :326-327. Scored, printed, and NOT made a refusal: this rung
        // reports, and an implausible lane count is a fact rung 08 must carry, not a reason to
        // throw away a capture that already cost a dark window.
        let cap_lanes_plausible = if cap_lanes == 1 || cap_lanes == 2 || cap_lanes == 4 { 1 } else { 0 };

        serial_println!(":: igpu-dpy: rung=06 name=link ok=1 window=DPCD(0x000-0x00F) dpcd_rev=0x{:02X} rate_raw=0x{:02X} rate_decode=TBV-tree-table lanes={} lanes_plausible={} enhanced_frame={} tps3={} downspread={} no_aux_tl={} edp_cfg_cap=0x{:02X} elapsed_ms={} ::",
            cap_rev, cap_rate, cap_lanes, cap_lanes_plausible, cap_enhanced, cap_tps3, cap_downspread, cap_no_aux_tl, cap_edp_cfg, get_elapsed_ms());
        serial_print!(":: igpu-dpy: rung=06 name=link CAP 000:");
        for b in dpcd_cap.iter() { serial_print!(" {:02X}", b); }
        serial_println!(" ::");
        serial_println!(":: igpu-dpy: rung=06 name=link STATUS addr=DPCD(0x202-0x207) bits=uncited lane01=0x{:02X} lane23=0x{:02X} align=0x{:02X} sink=0x{:02X} adj01=0x{:02X} adj23=0x{:02X} ::",
            link_status[0], link_status[1], link_status[2], link_status[3], link_status[4], link_status[5]);

        // ═══════════════════════════════════════════════════════════════════════════════════
        // RUNG 07 — `pps-on`: THE PANEL-POWER WRITE, DECLINED AGAIN, WITH THE DOCUMENT NAMED.
        //
        // The brief built this rung CONDITIONALLY: raise VDD **only if** this tree carries a cited
        // bit map for `PCH_PP_CONTROL` (0xC7204) bits 0 and 3 and a cited T1..T5 field layout for
        // `PCH_PP_ON_DELAYS` (0xC7208) / `PCH_PP_OFF_DELAYS` (0xC720C), from Intel PRM Vol 3
        // Part 4 "Panel Power Sequencing"; else decline again and stop the ladder there.
        //
        // IT DOES NOT, AND THIS EXECUTOR COULD NOT SUPPLY IT. `gen7.md` names 0xC7204 once and
        // only to say it is READ ONLY; `gpu_spec.md` does not name it at all; `gen7.rs`'s
        // `PCH_PP_CONTROL_KEY` cites `0xABCD` as an entropy PATTERN, not a semantic; the one place
        // in tree that names bit 0 / bit 2 / bit 3 — `LADDER-igpu-bringup.md` rung 2 — marks the
        // whole map **TBV** and sources it to i915 `intel_pps.c` NAMING, which the clean-room rule
        // forbids this rung from promoting to a citation. So the write is declined, again, with
        // the same token flight 11 printed at rung 03, and NOTHING is written.
        //
        // AND THE T VALUES THE BRIEF WOULD HAVE DERIVED DO NOT EXIST EITHER, which is the new
        // finding rather than a repeat. The brief said "program the delays from the DPCD/EDID-
        // derived T values (never zero)". Rung 06 has just read the whole DPCD cap window this
        // tree cites (0x000-0x00F) and rung 05 has just read the 128-byte EDID base block, and
        // neither carries a panel-power T value: the cap window is link capability, the EDID base
        // block is timing and identification. The panel's T1..T12 are therefore unavailable from
        // EVERYTHING THIS LADDER HAS EVER READ, and `PCH_PP_ON_DELAYS`/`PP_OFF_DELAYS` read
        // 0x00000000 on this part (flights 8/9/10/11). That is three independent legs under one
        // decline, and the third one is the dangerous one: firing the PPS on zero delays is the
        // documented panel-damage path.
        //
        // ⚠ THE DECLINE IS A TOKEN, NOT AN ABORT — deliberately, and the file already decided this
        // one register along. Rung 03's own doc-comment states the rule (R19): "This rung writes
        // nothing and so cannot fail: it reports." The brief's "stop the ladder here with the
        // reason" is honoured as **the ladder stops WRITING here** — no PPS write, no panel power,
        // no link write, ever, on this build — while rung 08, which writes nothing at all, still
        // flies. Returning `Err` instead would have made rung 08 structurally unreachable on every
        // boot, which is precisely the defect `docs/dev/QUEUE.md` §5 BRANCHCENSUS spent a whole
        // commit convicting (206 executor branches that reached no track; 23 LOST). The reason is
        // not hidden by that choice: it rides the rung line AND the rollup as `pps=`.
        highest = 7;
        rung_name = "pps-on";
        pps_verdict = "DECLINED:pp-bits-uncited";

        let pps_ctl = mmio_read(bar0, regs::PCH_PP_CONTROL);
        let pps_sts = mmio_read(bar0, regs::PCH_PP_STATUS);
        let pps_on = mmio_read(bar0, regs::PCH_PP_ON_DELAYS);
        let pps_off = mmio_read(bar0, regs::PCH_PP_OFF_DELAYS);
        let pps_div = mmio_read(bar0, regs::PCH_PP_DIVISOR);
        serial_println!(":: igpu-dpy: rung=07 name=pps-on ok=1 pp_write=DECLINED why=pp-bits-uncited writes=0 pp_unwind=0 t_source=NONE t_dpcd=absent t_edid=absent pp_ctl=0x{:08X} pp_sts=0x{:08X} on_delays=0x{:08X} off_delays=0x{:08X} div=0x{:08X} elapsed_ms={} ::",
            pps_ctl, pps_sts, pps_on, pps_off, pps_div, get_elapsed_ms());
        serial_println!(":: igpu-dpy: rung=07 name=pps-on MISSING doc=PRM-Vol3-Part4-Panel-Power-Sequencing pp_control=0xC7204:bit0=power-state-target:UNCITED,bit3=vdd-override:UNCITED on_delays=0xC7208:T1-T5:UNCITED off_delays=0xC720C:T:UNCITED — no PPS write is attempted and this ladder raises no panel power on any boot of this build ::");

        // ═══════════════════════════════════════════════════════════════════════════════════
        // RUNG 07b — `pps-read`: THE CITATION RUNG 07 COULD NOT MAKE, MADE — AND THE PANEL'S
        // ACTUAL POWER STATE DECODED FROM IT.
        //
        // ⚠ THIS RUNG DOES NOT UNDO RUNG 07'S DECLINE AND MUST NOT BE READ AS DOING SO. Rung 07
        // still writes nothing, still prints `pp_write=DECLINED`, and this build still raises no
        // panel power on any boot. What changed is only the FIRST of the three legs rung 07 stood
        // on: "no citation exists for the bit". It exists now, and it is in this rung's `cite=`
        // fields. The other two legs are untouched and are why the write is still declined —
        // `PP_ON_DELAYS`/`PP_OFF_DELAYS` read `0x00000000` on this part, so the panel's T values
        // are unprogrammed and firing the PPS on zero delays is the documented panel-damage path;
        // and flight 8 says the write is not needed to reach the sink at all.
        //
        // WHERE THE BIT MAP CAME FROM, and the clean-room line held: Intel® OpenSource HD
        // Graphics PRM **Volume 3 Part 4: South Display Engine Registers (Ivy Bridge)**, Doc Ref
        // `IHD-OS-V3 Pt 4 – 05 12`, May 2012 Rev 1.0, fetched from intel.com
        // (`cdrdv2-public.intel.com/690917/ivb-ihd-os-vol3-part4.pdf`) — §2.4 "Panel Power
        // Sequencing", pp.38–43, one sub-section per register. `i915`/`intel_pps.c` was not
        // opened; `docs/dev/GEMINI/video/iGUI/LADDER-igpu-bringup.md`'s TBV map, which was sourced
        // to i915 NAMING and is why GMUX7 refused, is not this rung's source and is not consulted.
        // Every row is re-stated with its section and page in `docs/dev/OS/08_VIDEO/gpu_spec.md` §6.
        //
        // ⚠ THE BRIEF ASKED FOR "T1/T2/T3/T4/T5" AND THE PRM DOES NOT NAME THOSE ON DISPLAYPORT —
        // that lettering is the LVDS/SPWG column. On an eDP panel the same four fields are eDP
        // **T3** (power up, `PP_ON_DELAYS` [28:16]), **T9** (backlight-off to video-off,
        // `PP_OFF_DELAYS` [12:0]), **T10** (video-off to power-off, `PP_OFF_DELAYS` [28:16]) and
        // **T12** (the power-cycle floor, `PP_DIVISOR` [4:0]). Both letterings are printed so a
        // capture is not silently re-mapped, and the eDP names are the load-bearing ones.
        //
        // IT WRITES NOTHING. It re-uses rung 07's five reads rather than taking a second sample,
        // deliberately: two samples a few microseconds apart would invite a reader to diff them
        // as if the rung had measured motion, and it has not. `sample=rung07` says so on the wire.
        highest = 8;
        rung_name = "pps-read";

        let pp_power_on = (pps_sts >> 31) & 1;
        let pp_assets = (pps_sts >> 30) & 1;
        let pp_seq = (pps_sts >> 28) & 0x3;
        let pp_cycle_active = (pps_sts >> 27) & 1;
        let pp_wp_key = (pps_ctl >> 16) & 0xFFFF;
        let pp_vdd_force = (pps_ctl >> 3) & 1;
        let pp_backlight = (pps_ctl >> 2) & 1;
        let pp_pd_on_reset = (pps_ctl >> 1) & 1;
        let pp_target = pps_ctl & 1;
        let pp_port_sel = (pps_on >> 30) & 0x3;
        let pp_t3_raw = (pps_on >> 16) & 0x1FFF;
        let pp_bl_on_raw = pps_on & 0x1FFF;
        let pp_t10_raw = (pps_off >> 16) & 0x1FFF;
        let pp_t9_raw = pps_off & 0x1FFF;
        let pp_refdiv = (pps_div >> 8) & 0xFFFFFF;
        let pp_t12_raw = pps_div & 0x1F;
        // §2.4.5 p.43 states the divider law in words — "the value should be (100 * Ref clock
        // frequency in MHz / 2) - 1" — and gives one worked row, 125 MHz → 1869h. Inverted here,
        // and printed beside the raw field so a reader can re-check the arithmetic on the wire.
        let pp_refclk_mhz = if pp_refdiv == 0xFFFFFF { 0 } else { (2 * (pp_refdiv + 1)) / 100 };
        // §2.4.5 p.43 gives TWO data points for the power-cycle field and no formula: default
        // 4h = 300 ms, and "to achieve 400 ms, program a value of 5". Both fit (raw−1)·100 ms,
        // and a written 0 is documented as "no delay". The derivation is named on the wire as
        // `t12_law=prm-two-points` so it is never mistaken for a quoted formula.
        let pp_t12_ms = if pp_t12_raw == 0 { 0 } else { (pp_t12_raw - 1) * 100 };
        let pp_delays_programmed =
            (pps_on != 0) as u32 + (pps_off != 0) as u32 + (pps_div != 0) as u32;

        serial_println!(":: igpu-dpy: rung=07b name=pps-read ok=1 writes=0 sample=rung07 doc=IHD-OS-V3-Pt-4-05-12 cite=sec2.4-pp38-43 elapsed_ms={} ::",
            get_elapsed_ms());
        serial_println!(":: igpu-dpy: rung=07b name=pps-read REG pp_status@0x{:05X}=0x{:08X} power_on={} assets={}(ignored-on-DPA-p38) seq={} cycle_delay_t4={} cite=sec2.4.1-pp38-39 ::",
            regs::PCH_PP_STATUS, pps_sts,
            if pp_power_on == 1 { "ON" } else { "OFF" },
            if pp_assets == 1 { "ready" } else { "not-ready" },
            pp_seq_name(pp_seq),
            if pp_cycle_active == 1 { "active" } else { "not-active" });
        serial_println!(":: igpu-dpy: rung=07b name=pps-read REG pp_control@0x{:05X}=0x{:08X} wp_key=0x{:04X} write_protect={} vdd_override={} backlight={} pd_on_reset={} power_target={} cite=sec2.4.2-pp39-41 ::",
            regs::PCH_PP_CONTROL, pps_ctl, pp_wp_key,
            if pp_wp_key == 0xABCD { "DISABLED-key-ABCD" } else { "ENABLED" },
            if pp_vdd_force == 1 { "FORCE" } else { "not-force" },
            if pp_backlight == 1 { "enable" } else { "disable" },
            if pp_pd_on_reset == 1 { "run" } else { "do-not-run" },
            if pp_target == 1 { "ON" } else { "OFF" });
        serial_println!(":: igpu-dpy: rung=07b name=pps-read REG pp_on_delays@0x{:05X}=0x{:08X} port_sel={} t3_powerup_raw={} t3_us={} pwron_to_bl_raw={} pwron_to_bl_us={} unit=100us cite=sec2.4.3-pp41-42 ::",
            regs::PCH_PP_ON_DELAYS, pps_on, pp_port_name(pp_port_sel),
            pp_t3_raw, pp_t3_raw * 100, pp_bl_on_raw, pp_bl_on_raw * 100);
        serial_println!(":: igpu-dpy: rung=07b name=pps-read REG pp_off_delays@0x{:05X}=0x{:08X} t10_video_off_to_power_off_raw={} t10_us={} t9_bl_off_to_video_off_raw={} t9_us={} unit=100us cite=sec2.4.4-p42 ::",
            regs::PCH_PP_OFF_DELAYS, pps_off, pp_t10_raw, pp_t10_raw * 100, pp_t9_raw, pp_t9_raw * 100);
        serial_println!(":: igpu-dpy: rung=07b name=pps-read REG pp_divisor@0x{:05X}=0x{:08X} refdiv=0x{:06X} refclk_mhz={} t12_power_cycle_raw=0x{:02X} t12_ms={} t12_law=prm-two-points unit=100ms cite=sec2.4.5-p43 ::",
            regs::PCH_PP_DIVISOR, pps_div, pp_refdiv, pp_refclk_mhz, pp_t12_raw, pp_t12_ms);
        // THE ROLLUP LINE, and it is the one a write rung has to obey. `panel_powered_by_fw=` is
        // the single fact the brief asked this rung to establish: what a later `pps-on` must
        // PRESERVE rather than re-do. `vdd_forced_by_fw=` is the second, and on this machine it
        // is the one that explains flight 11 — §2.4.2 p.40 says bit 3 exists precisely "to force
        // on VDD for the embedded DisplayPort panel so AUX transactions can occur WITHOUT
        // enabling the panel power sequence", which is exactly the state every AUX rung of this
        // ladder has been reading the sink in. `port_sel_conflict=` is the third and it is a
        // TRAP for the write rung: §2.4.3 p.41's own workaround note ties the `0xABCD` key to
        // `PP_ON_DELAYS[31:30] = 01b DisplayPort A`, so a key of ABCD sitting above a port select
        // of LVDS is a half-configured sequencer, and the write rung must program the port select
        // (and real T values) before it touches the power-state target.
        serial_println!(":: igpu-dpy: rung=07b name=pps-read ROLLUP panel_powered_by_fw={} vdd_forced_by_fw={} wp_window={} delays_programmed={}/3 t_source=PRM-fields-not-firmware-values port_sel_conflict={} preserve=pp_control=0x{:08X} ::",
            if pp_power_on == 1 { "yes" } else { "no" },
            if pp_vdd_force == 1 { "yes" } else { "no" },
            if pp_wp_key == 0xABCD { "KEYED" } else { "PROTECTED" },
            pp_delays_programmed,
            if pp_wp_key == 0xABCD && pp_port_sel != 0b01 { "key-ABCD-but-port-sel-not-DPA" } else { "none" },
            pps_ctl);

        // ═══════════════════════════════════════════════════════════════════════════════════
        // RUNG 08 — `link-train-dry`: THE TRANSCRIPTION, SO THE NEXT MODE-SET IS A COPY.
        //
        // A DRY RUN. It writes nothing — not one AUX byte, not one MMIO dword — and its whole
        // product is five printed lines: the link parameters it WOULD set (from rung 06's DPCD,
        // never invented), the DPCD addresses it WOULD write, the display registers it WOULD touch
        // WITH THEIR CURRENT VALUES, and — the line that matters most — the registers a real
        // mode-set needs that THIS TREE HAS NO OFFSET FOR. The last one turns the next executor's
        // first hour from a hunt into a shopping list with a citation requirement attached.
        //
        // WHY THE "WOULD-TOUCH" SET IS EXACTLY THE `regs` BLOCK AND NOT ONE REGISTER WIDER. The
        // clean-room rule for this seat is "every register cited". Each offset printed below is
        // already a named constant in this file's own `regs` module, which is the tree carrying
        // it, and inventing one from memory is the exact move this ladder has refused at the PPS
        // for four flights running.
        //
        // ⚠ GMUX8 CHANGED THIS RUNG'S LAST LINE, and only that line, because leaving it would have
        // put a FALSEHOOD on the wire. GMUX7 wrote `NOT-IN-TREE htotal hblank hsync vtotal vblank
        // vsync transcoder dp_tp_ctl dp_tp_status link_m link_n — population=igpu.rs-regs-module
        // (igpu.rs:5-93), hits=0`. Eight of those eleven names are now cited constants in that
        // same module (GMUX8 added them from IVB PRM Vol3 Pt3), so `hits=0` is no longer true and
        // the `igpu.rs:5-93` span no longer bounds the module. The three that remain are not
        // "still missing" either — they are absent FROM THE SILICON'S OWN PUBLIC SPEC, which is a
        // stronger statement and a different one, so the line now says which of the two it means.
        // Every other token of rung 08 — its id, its name, `DRY`, `writes=0`, `WOULD-SET`,
        // `WOULD-WRITE-DPCD`, `WOULD-TOUCH`, `NOT-IN-TREE` — is byte-identical to GMUX7's.
        //
        // `highest` moves 8 → 9 here because rung 07b took slot 8. The number is not a comparator
        // and this file has said so since GMUXDPCD: captures compare on `name=`.
        //
        // It cannot black the panel, and the statement is structural rather than a promise:
        // there is no `write_volatile` and no `dp_aux_transfer` call in the rung at all.
        highest = 9;
        rung_name = "link-train-dry";

        serial_println!(":: igpu-dpy: rung=08 name=link-train-dry ok=1 DRY writes=0 aux_writes=0 pp_unwind=0 gated_on=pps={} elapsed_ms={} ::",
            pps_verdict, get_elapsed_ms());
        serial_println!(":: igpu-dpy: rung=08 name=link-train-dry WOULD-SET rate=0x{:02X}(DPCD 0x001 verbatim; no MHz conversion, decode TBV) lanes={} enhanced_frame={} tps3={} downspread={} seq=TPS1-CR/TPS2-EQ/pattern-off cite=LADDER-igpu-bringup.md-rung5-NEEDS-VERIFICATION ::",
            cap_rate, cap_lanes, cap_enhanced, cap_tps3, cap_downspread);
        serial_println!(":: igpu-dpy: rung=08 name=link-train-dry WOULD-WRITE-DPCD link_bw_set=0x100 lane_count_set=0x101 training_pattern_set=0x102 training_lane0_3_set=0x103-0x106 cite=LADDER-igpu-bringup.md-rung5-NEEDS-VERIFICATION ::");
        serial_println!(":: igpu-dpy: rung=08 name=link-train-dry WOULD-TOUCH dp_a@0x{:05X}=0x{:08X} dplla@0x{:05X}=0x{:08X} fpa0@0x{:05X}=0x{:08X} fpa1@0x{:05X}=0x{:08X} pipeaconf@0x{:05X}=0x{:08X} pipeasrc@0x{:05X}=0x{:08X} ::",
            regs::DP_A, mmio_read(bar0, regs::DP_A),
            regs::DPLL_A_CTRL, mmio_read(bar0, regs::DPLL_A_CTRL),
            regs::FPA0, mmio_read(bar0, regs::FPA0),
            regs::FPA1, mmio_read(bar0, regs::FPA1),
            regs::PIPEACONF, mmio_read(bar0, regs::PIPEACONF),
            regs::PIPEASRC, mmio_read(bar0, regs::PIPEASRC));
        serial_println!(":: igpu-dpy: rung=08 name=link-train-dry WOULD-TOUCH dspacntr@0x{:05X}=0x{:08X} dspastride@0x{:05X}=0x{:08X} dspalinoff@0x{:05X}=0x{:08X} dspatileoff@0x{:05X}=0x{:08X} dspasurf@0x{:05X}=0x{:08X} ::",
            regs::DSPACNTR, mmio_read(bar0, regs::DSPACNTR),
            regs::DSPASTRIDE, mmio_read(bar0, regs::DSPASTRIDE),
            regs::DSPALINOFF, mmio_read(bar0, regs::DSPALINOFF),
            regs::DSPATILEOFF, mmio_read(bar0, regs::DSPATILEOFF),
            regs::DSPASURF, mmio_read(bar0, regs::DSPASURF));
        serial_println!(":: igpu-dpy: rung=08 name=link-train-dry NOT-IN-TREE (none) LANDED-BY-GMUX8 htotal hblank hsync vtotal vblank vsync link_m link_n — population=igpu.rs-regs-module, read decoded at rung=08b; NOT-IN-IVB-PRM transcoder dp_tp_ctl dp_tp_status — population=IVB-PRM-V3Pt3+V3Pt4, hits=0, so they are absent from the SILICON's public spec and not merely from this tree ::");

        // ═══════════════════════════════════════════════════════════════════════════════════
        // RUNG 08b — `modeset-read`: THE DRY MODE-SET, READ OFF THE RUNNING MACHINE.
        //
        // Rung 08 says what a mode-set WOULD write and had to name eleven registers it could not
        // reach. This rung READS eight of those eleven — plus the port and pipe control words —
        // and prints the mode THE FIRMWARE IS ALREADY DRIVING. That is the whole point: the write
        // rung's job is to REPRODUCE these numbers, so they have to be on a wire first, decoded,
        // with the section and page beside each one.
        //
        // IT WRITES NOTHING. There is no `write_volatile` and no `dp_aux_transfer` call in this
        // rung; every access is `mmio_read`. It cannot black the panel.
        //
        // SOURCE, and again it is one document: Intel® OpenSource HD Graphics PRM **Volume 3
        // Part 3: North Display Engine Registers (Ivy Bridge)**, Doc Ref `IHD-OS-V3 Pt 3 – 05 12`,
        // May 2012 Rev 1.0, from intel.com (`cdrdv2-public.intel.com/690915/`). Offsets and bit
        // fields are in `regs` with per-family section/page comments and in `gpu_spec.md` §6.
        //
        // WHICH PIPE — READ, NOT ASSUMED. `DP_CTL_A` bits [30:29] (§4.4.1 p.82) name the pipe the
        // eDP port takes its data from, so the rung reads the port word FIRST and selects the
        // timing block from it. Every earlier rung of this ladder that touched a pipe touched
        // pipe A by construction; this one would print `pipe=B` if the firmware said B.
        //
        // ⚠ THE ONE NUMBER THIS RUNG DERIVES, and it is derived from two CITED fields rather than
        // guessed. Refresh is not a register. §4.2 p.74 states the link law in the PRM's own
        // words — "Link M/N = dot clock / ls_clk" — and §4.4.1 p.85 gives `ls_clk` a cited
        // encoding on the SOURCE side: `DP_CTL_A` [17:16], `00b` = 270 MHz, `01b` = 162 MHz. So
        // `dot_clock = ls_clk · LinkM / LinkN`, and refresh = dot clock / (htotal · vtotal) with
        // the PRM's own minus-one convention undone. THIS IS THE FIRST CITED LINK RATE THIS
        // LADDER HAS HAD: rung 06 still prints `rate_decode=TBV-tree-table` for the DPCD byte at
        // `0x001`, because that is a SINK capability in a DP-spec encoding and this rung's number
        // is a SOURCE PLL setting in an Intel encoding — they are two different facts and the
        // rung prints both side by side rather than letting one stand in for the other.
        //
        // ⚠ AND WHICH M/N SET IS LIVE IS ALSO READ. `PIPE_CONF` bit 20 (§5.1.3 p.101) selects the
        // normal (M/N 1) or low-power (M/N 2) set for software-controlled DRRS. Both sets are
        // printed in full and `mn_set=` says which one the `MODE` line used, because a dry
        // mode-set that silently read M1 while the panel was running on M2 would hand the write
        // rung the wrong refresh with no way to notice.
        //
        // `end` LANDS AT 10 HERE — the ladder's full nominal height, so the rollup's `/10`
        // denominator is exact for the first time. The `end` step below changes `rung_name` only:
        // this rung already carried `highest` to the top, and a completed run is distinguished by
        // `name=end ok=1`, never by the number.
        highest = 10;
        rung_name = "modeset-read";

        let dp_a_val = mmio_read(bar0, regs::DP_A);
        let dp_enable = (dp_a_val >> 31) & 1;
        let dp_pipe_sel = (dp_a_val >> 29) & 0x3;
        let dp_width = (dp_a_val >> 19) & 0x7;
        let dp_enh_frame = (dp_a_val >> 18) & 1;
        let dp_pll_freq = (dp_a_val >> 16) & 0x3;
        let dp_reversed = (dp_a_val >> 15) & 1;
        let dp_pll_en = (dp_a_val >> 14) & 1;
        let dp_train_pat = (dp_a_val >> 8) & 0x3;
        let dp_sync_pol = (dp_a_val >> 3) & 0x3;
        let dp_detected = (dp_a_val >> 2) & 1;
        let p = pipe_timing_regs(dp_pipe_sel);

        serial_println!(":: igpu-dpy: rung=08b name=modeset-read ok=1 writes=0 doc=IHD-OS-V3-Pt-3-05-12 pipe={} pipe_sel_raw={} elapsed_ms={} ::",
            p.name, dp_pipe_sel, get_elapsed_ms());
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read PORT dp_a@0x{:05X}=0x{:08X} enable={} pipe={} lanes={} enhanced_frame={} pll_freq={} pll_enable={} reversed={} train_pattern={} sync_pol=0x{:X} detected={} cite=sec4.4.1-pp82-86 ::",
            regs::DP_A, dp_a_val,
            if dp_enable == 1 { "ENABLED" } else { "disabled" },
            p.name, dp_width_name(dp_width),
            if dp_enh_frame == 1 { "enable" } else { "disable" },
            dp_pll_freq_name(dp_pll_freq),
            if dp_pll_en == 1 { "enable" } else { "disable" },
            if dp_reversed == 1 { "reversed" } else { "not-reversed" },
            dp_train_pat_name(dp_train_pat), dp_sync_pol,
            if dp_detected == 1 { "detected" } else { "not-detected" });

        let conf = mmio_read(bar0, p.conf);
        let conf_enable = (conf >> 31) & 1;
        let conf_state = (conf >> 30) & 1;
        let conf_fsd = (conf >> 27) & 0x3;
        let conf_interlace = (conf >> 21) & 0x7;
        let conf_drrs = (conf >> 20) & 1;
        let conf_bpc = (conf >> 5) & 0x7;
        let mn_set = if conf_drrs == 1 { 2 } else { 1 };
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read PIPECONF conf@0x{:05X}=0x{:08X} pipe_enable={} pipe_state={} bpc={} interlace=0x{:X} drrs_power_mode={} mn_set={} frame_start_delay={} cite=sec5.1.3-pp100-103 ::",
            p.conf, conf,
            if conf_enable == 1 { "ENABLED" } else { "disabled" },
            if conf_state == 1 { "ENABLED" } else { "disabled" },
            pipe_bpc_name(conf_bpc), conf_interlace,
            if conf_drrs == 1 { "low-power" } else { "normal" },
            mn_set, conf_fsd);

        // §4.1.1–§4.1.3 pp.66–68: the PRM programs Horizontal Total and Horizontal Active as
        // "the number of pixels desired MINUS ONE", so every count below is the field plus one
        // and every POSITION (blank/sync start and end) is printed as the field reads, since the
        // PRM defines those relative to active-display start and not as a count.
        let htot = mmio_read(bar0, p.htotal);
        let hbl = mmio_read(bar0, p.hblank);
        let hsy = mmio_read(bar0, p.hsync);
        let h_active = (htot & 0xFFF) + 1;
        let h_total = ((htot >> 16) & 0x1FFF) + 1;
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read TIMING-H htotal@0x{:05X}=0x{:08X} hblank@0x{:05X}=0x{:08X} hsync@0x{:05X}=0x{:08X} active={} total={} blank_start={} blank_end={} sync_start={} sync_end={} minus_one_undone=yes cite=sec4.1.1-4.1.3-pp66-68 ::",
            p.htotal, htot, p.hblank, hbl, p.hsync, hsy,
            h_active, h_total, hbl & 0x1FFF, (hbl >> 16) & 0x1FFF, hsy & 0x1FFF, (hsy >> 16) & 0x1FFF);

        let vtot = mmio_read(bar0, p.vtotal);
        let vbl = mmio_read(bar0, p.vblank);
        let vsy = mmio_read(bar0, p.vsync);
        let v_active = (vtot & 0xFFF) + 1;
        let v_total = ((vtot >> 16) & 0x1FFF) + 1;
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read TIMING-V vtotal@0x{:05X}=0x{:08X} vblank@0x{:05X}=0x{:08X} vsync@0x{:05X}=0x{:08X} active={} total={} blank_start={} blank_end={} sync_start={} sync_end={} minus_one_undone=yes cite=sec4.1.4-4.1.6-pp69-71 ::",
            p.vtotal, vtot, p.vblank, vbl, p.vsync, vsy,
            v_active, v_total, vbl & 0x1FFF, (vbl >> 16) & 0x1FFF, vsy & 0x1FFF, (vsy >> 16) & 0x1FFF);

        let srcsz = mmio_read(bar0, p.srcsz);
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read SRCSZ srcsz@0x{:05X}=0x{:08X} h={} v={} minus_one_undone=yes cite=sec4.1.7-p72 ::",
            p.srcsz, srcsz, ((srcsz >> 16) & 0xFFF) + 1, (srcsz & 0xFFF) + 1);

        let dm1 = mmio_read(bar0, p.datam1);
        let dn1 = mmio_read(bar0, p.datan1);
        let lm1 = mmio_read(bar0, p.linkm1);
        let ln1 = mmio_read(bar0, p.linkn1);
        let dm2 = mmio_read(bar0, p.datam2);
        let dn2 = mmio_read(bar0, p.datan2);
        let lm2 = mmio_read(bar0, p.linkm2);
        let ln2 = mmio_read(bar0, p.linkn2);
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read MN1 datam1@0x{:05X}=0x{:08X} tu_size={} data_m={} datan1@0x{:05X}=0x{:08X} data_n={} linkm1@0x{:05X}=0x{:08X} link_m={} linkn1@0x{:05X}=0x{:08X} link_n={} cite=sec4.2.1-4.2.4-pp74-77 ::",
            p.datam1, dm1, ((dm1 >> 25) & 0x3F) + 1, dm1 & 0xFFFFFF,
            p.datan1, dn1, dn1 & 0xFFFFFF,
            p.linkm1, lm1, lm1 & 0xFFFFFF,
            p.linkn1, ln1, ln1 & 0xFFFFFF);
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read MN2 datam2@0x{:05X}=0x{:08X} tu_size={} data_m={} datan2@0x{:05X}=0x{:08X} data_n={} linkm2@0x{:05X}=0x{:08X} link_m={} linkn2@0x{:05X}=0x{:08X} link_n={} cite=sec4.2.1-4.2.4-pp74-77 ::",
            p.datam2, dm2, ((dm2 >> 25) & 0x3F) + 1, dm2 & 0xFFFFFF,
            p.datan2, dn2, dn2 & 0xFFFFFF,
            p.linkm2, lm2, lm2 & 0xFFFFFF,
            p.linkn2, ln2, ln2 & 0xFFFFFF);

        let (live_lm, live_ln) = if mn_set == 2 { (lm2 & 0xFFFFFF, ln2 & 0xFFFFFF) } else { (lm1 & 0xFFFFFF, ln1 & 0xFFFFFF) };
        let ls_clk_khz: u32 = match dp_pll_freq { 0 => 270_000, 1 => 162_000, _ => 0 };
        let dot_khz: u32 = if live_ln != 0 && ls_clk_khz != 0 {
            ((ls_clk_khz as u64 * live_lm as u64) / live_ln as u64) as u32
        } else { 0 };
        let px_per_frame = h_total as u64 * v_total as u64;
        let refresh_mhz: u32 = if dot_khz != 0 && px_per_frame != 0 {
            ((dot_khz as u64 * 1_000_000) / px_per_frame) as u32
        } else { 0 };
        // A zero here is a REFUSAL, not a reading, and it says which input was missing — an
        // unpowered pipe reads M/N as zero and this ladder has never yet seen the pipe live.
        let mode_why = if ls_clk_khz == 0 { "pll-freq-reserved" }
            else if live_ln == 0 { "link_n=0" }
            else if px_per_frame == 0 { "htotal-or-vtotal=0" }
            else { "none" };
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read MODE active={}x{} total={}x{} mn_set={} ls_clk_khz={} dot_clock_khz={} refresh_mhz={} undefined_why={} law=LinkM/N=dotclock/ls_clk(sec4.2-p74)+DP_CTL[17:16](sec4.4.1-p85) dpcd_rate_raw=0x{:02X}(rate_decode=TBV-tree-table) dpcd_lanes={} ::",
            h_active, v_active, h_total, v_total, mn_set, ls_clk_khz, dot_khz, refresh_mhz, mode_why, cap_rate, cap_lanes);
        serial_println!(":: igpu-dpy: rung=08b name=modeset-read NOT-IN-IVB-PRM dp_tp_ctl dp_tp_status trans_ddi_func_ctl — population=IVB-PRM-V3Pt3(North)+V3Pt4(South), hits=0; those are Haswell DDI registers, and on Ivy Bridge eDP link training is DP_CTL_A[9:8] (sec4.4.1 pp.85-86) with the port CPU-attached, so no PCH transcoder is in the eDP path and the write rung must not look for one ::");

        highest = 10;
        rung_name = "end";
        Ok(())
    };

    let mut why_status = 0;
    match execute_harness() {
        Ok(_) => {
            ok_flag = 1;
        }
        Err((e, stat)) => {
            why_str = e;
            why_status = stat;
        }
    }

    // GMUX7: `pending=` has been a BARE COUNT since the rollup existed, and flight 11 printed
    // `pending=2` with nothing on the wire saying WHICH two. The entries are snapshotted HERE —
    // before `execute()` drains the stack to zero — so the rollup can name them. `entries` is
    // `[UnwindEntry; 32]` and `UnwindEntry` is `Copy`, so this is a copy of the array, not a
    // borrow that would outlive the drain.
    let pending_entries = unwind.entries;
    let unwound_count = unwind.len;
    let revert_ok = unwind.execute();

    if let Some(e) = out_pre_dpcd_err {
        serial_println!(":: igpu: [AUX] PRE-SWITCH DPCD Read Failed: {} (status: 0x{:08X}) ::", e, out_pre_dpcd_status);
    } else if let Some(rev) = out_pre_dpcd {
        serial_println!(":: igpu: [AUX] PRE-SWITCH DPCD REV: 0x{:02X} ::", rev);
    } else {
        serial_println!(":: igpu: [AUX] PRE-SWITCH DPCD Read: (n/a - path never reached) ::");
    }

    if mux_touched {
        serial_println!(":: igpu: [GMUX] switched DISPLAY, EXTERNAL, and DDC to IGD (panel BLANKED/FLICKERED) ::");
        if mux_reads.0 != GMUX_DDC_IGD as u32 || mux_reads.1 != GMUX_DISPLAY_IGD as u32 || mux_reads.2 != GMUX_EXTERNAL_IGD as u32 {
            serial_println!(":: igpu: [GMUX] switch read-back mismatch (DDC is the only echo proven on this machine; DISP/EXT are advisory — see the AUX rungs for whether the mux moved): DDC=0x{:02X} DISP=0x{:02X} EXT=0x{:02X} ::", mux_reads.0, mux_reads.1, mux_reads.2);
        }
        if let Some(pb) = pp_before {
            serial_println!(":: igpu: [AUX] PCH_PP_STATUS/CONTROL Before AUX: STATUS=0x{:08X} CONTROL=0x{:08X} ::", pb.1, pb.0);
        } else {
            serial_println!(":: igpu: [AUX] PCH_PP_STATUS/CONTROL Before AUX: (n/a - path never reached) ::");
        }
        if let Some(pa) = pp_after {
            serial_println!(":: igpu: [AUX] PCH_PP_STATUS/CONTROL After AUX:  STATUS=0x{:08X} CONTROL=0x{:08X} ::", pa.1, pa.0);
        } else {
            serial_println!(":: igpu: [AUX] PCH_PP_STATUS/CONTROL After AUX:  (n/a - path never reached) ::");
        }
    }

    if let Some(rev) = out_dpcd {
        serial_println!(":: igpu: [AUX] DPCD REV: 0x{:02X} ::", rev);
    } else {
        serial_println!(":: igpu: [AUX] DPCD REV: (n/a) ::");
    }

    if let Some(edid) = out_edid {
        serial_println!(":: igpu: [AUX] EDID Dump ::");
        for i in 0..8 {
            let row = &edid[(i*16)..((i+1)*16)];
            serial_print!(":: igpu: [AUX] {:02X}0: ", i);
            for b in row { serial_print!("{:02X} ", b); }
            serial_println!("::");
        }
    } else {
        serial_println!(":: igpu: [AUX] EDID Dump: (n/a) ::");
    }

    if ok_flag == 0 {
        serial_println!(":: igpu: [GMUX] REFUSED: {} (status: 0x{:08X}) ::", why_str, why_status);
    }

    // Read back to decide gmux= verdict. Only DDC is proven, others are TBV.
    let gmux_verdict = if let (Some(intent_ddc), Some(_intent_disp), Some(intent_ext), Some(intent_ext_status)) = (pre_ddc, pre_disp, pre_ext, pre_ext_status) {
        if mux_touched {
            let post_ddc = gmux_index_read(GMUX_SWITCH_DDC);
            let post_disp_target = gmux_index_read(GMUX_SWITCH_DISPLAY);
            let post_disp_status = gmux_index_read(GMUX_READ_DISPLAY);
            let post_ext_target = gmux_index_read(GMUX_SWITCH_EXTERNAL);
            let post_ext_status = gmux_index_read(GMUX_READ_EXTERNAL);

            // EXTERNAL is compared against the VALIDATED PRE-IMAGE, not against DIS. On a
            // Kepler-owned machine the correct restore lands 0x21, and comparing that to DIS would
            // report FAILED for a restore that was exactly right.
            //
            // Review condition: the verdict rests ONLY on registers proven readable on this
            // machine — DDC's echo and the two READ_* status ports. SWITCH_DISPLAY and
            // SWITCH_EXTERNAL read-backs are printed as TBV but do not vote: a write-side port
            // that does not echo would otherwise flip a correct restore to FAILED, and the
            // RUNBOOK's FAILED row tells the operator to power-cycle a healthy machine.
            //
            // GMUX-2: EXTERNAL's status port votes only when EXTERNAL was actually restored. When
            // `ext_restore` is None the unwind deliberately did not write 0x40, so 0x40 is still
            // carrying the forward `GMUX_EXTERNAL_IGD` and 0x41 has no reason to have come back to
            // its pre-image — scoring it would report FAILED for a flight that did exactly what
            // this commit told it to do, and would send the operator to power-cycle a healthy
            // machine for the second time. The skipped restore is not hidden: it is printed on its
            // own `restore ext=SKIPPED` line at the gate and named again below.
            let verdict = if post_ddc == intent_ddc
                && post_disp_status == GMUX_DISPLAY_DIS as u32
                && (ext_restore.is_none() || post_ext_status == intent_ext_status)
                && revert_ok {
                "MATCH"
            } else {
                "FAILED"
            };

            serial_println!(":: igpu: [GMUX] revert read-back: DDC=0x{:02X} SWITCH_DISP=0x{:02X} READ_DISP=0x{:02X} SWITCH_EXT=0x{:02X} READ_EXT=0x{:02X} (TBV) ::", post_ddc, post_disp_target, post_disp_status, post_ext_target, post_ext_status);
            match ext_restore {
                Some(v) => serial_println!(":: igpu: [GMUX] EXTERNAL restore=0x{:02X} from the READ_EXT(0x41) state {} (SWITCH_EXT 0x{:02X}->0x{:02X}, READ_EXT 0x{:02X}->0x{:02X}) — mapped from the status read, never from the write-target port ::",
                    v, ext_state_name(intent_ext_status), intent_ext, post_ext_target, intent_ext_status, post_ext_status),
                None => serial_println!(":: igpu: [GMUX] EXTERNAL restore=SKIPPED (write-target port, no state read) — SWITCH_EXT(0x40) 0x{:02X}->0x{:02X} was NOT written back and does not vote; READ_EXT(0x41) 0x{:02X}->0x{:02X} state {}->{} ::",
                    intent_ext, post_ext_target, intent_ext_status, post_ext_status, ext_state_name(intent_ext_status), ext_state_name(post_ext_status)),
            }
            verdict
        } else {
            "UNTOUCHED"
        }
    } else {
        "UNTOUCHED"
    };

    // GMUXDPCD: the new tokens are APPENDED after `elapsed_ms=`, which was the line's last field.
    // Every pre-existing field keeps its name, its value and its ORDER — flights 5 and 8-10 and
    // the G8 row all key on this line, and a capture-to-capture diff has to stay a field-for-field
    // diff. `pp_seen=` says which of the two PPS samples the LADDER's `pp=` pair actually holds
    // (`both` on any path that reached an AUX transaction, `before` where the harness died between
    // the switch and the first transaction, `none` where it never switched), so a 0x00000000 in
    // `pp=` is never mistaken for a read that happened.
    let (ppb_ctl, ppb_sts) = pp_before.unwrap_or((0, 0));
    let (ppa_ctl, ppa_sts) = pp_after.unwrap_or((0, 0));
    let pp_seen = match (pp_before.is_some(), pp_after.is_some()) {
        (true, true) => "both",
        (true, false) => "before",
        _ => "none",
    };
    // GMUX7 appends `pps=` and `pending_items=` after `dpcd_tries=`, which GMUXDPCD left as the
    // line's last field — the same discipline GMUXDPCD itself followed: every pre-existing field
    // keeps its NAME, its VALUE and its ORDER, so a flight-5/8/9/10/11-to-flight-12 diff stays a
    // field-for-field diff and the register's G8 row keeps its key. The line is emitted in three
    // calls because `pending_items=` is a variable-length list, but `serial_print!` appends no
    // newline, so it is still ONE physical line and one `awk` match.
    serial_print!(":: igpu-dpy: LADDER highest={:02}/10 name={} ok={} pending={} gmux={} why={} elapsed_ms={} pp_seen={} pp=0x{:08X}/0x{:08X}->0x{:08X}/0x{:08X} pp_settle_ms={} aux_port=DPA(0x{:05X}) aux_div=0x{:03X} dpcd_tries={} pps={} pending_items=",
        highest, rung_name, ok_flag, unwound_count, gmux_verdict, why_str, get_elapsed_ms(),
        pp_seen, ppb_ctl, ppb_sts, ppa_ctl, ppa_sts, pp_settle_ms,
        regs::DPA_AUX_CH_CTL, aux_div, dpcd_tries, pps_verdict);
    print_pending_items(&pending_entries, unwound_count);
    serial_println!(" ::");
}

// ═══════════════════════════════════════════════════════════════════════════════════════════
// GMUX-2 — THE PRE-SWITCH GATE'S DECODE, AS A PURE FUNCTION OVER THE STATE READS
//
// Placed at the FOOT of the file on purpose. Everything below the gmux harness is nothing, so
// appending here moves no line number in this file, and `panic::Location` in the knob-OFF build
// cannot shift under a `gmux_igd` change (QUEUE B94). Every item carries the same
// `all(target_arch = "x86_64", feature = "gmux_igd")` cfg as the rest of the ladder, so with the
// knob off this whole block compiles to nothing at all.
//
// THE PORT PAIRS, read off this file's own constants block at :244-300 and the port map comment
// at :238-243 (upstream `drivers/platform/x86/apple-gmux.c`, port-I/O backend):
//
//   * DISPLAY  0x10/0x11 — `GMUX_SWITCH_DISPLAY` (target, :254) / `GMUX_READ_DISPLAY` (status,
//     :256). THE FILE ALREADY TREATS THIS PAIR AS TARGET/STATUS: the gate scores `GMUX_READ_DISPLAY`
//     and the unwind writes `GMUX_SWITCH_DISPLAY`, and the review condition that ungated
//     SWITCH_DISPLAY said so in as many words.
//   * EXTERNAL 0x40/0x41 — `GMUX_SWITCH_EXTERNAL` (target, :291) / `GMUX_READ_EXTERNAL` (status,
//     :258). THE PAIR IS COMPLETE IN THE FILE, AND ONLY THE VERDICT WAS WRONG: until this commit
//     the gate scored BOTH halves against the STATUS encodings
//     `{GMUX_EXTERNAL_DIS 0x03, GMUX_EXTERNAL_KEPLER_OWNED 0x21}`. The POST-switch read-back in
//     this same function had already been relaxed on this very reasoning, one step later — "write-side
//     switch ports that do not echo their value would fail this comparison on a switch that
//     WORKED" — and the two were never reconciled. Flight 5 (2026-08-28) is the bill:
//     `SW_EXT=0x01` on the write-target port refused the switch at `gmux=UNTOUCHED elapsed_ms=1`
//     while DDC=0x02, DISP=0x03 and EXT=0x21 — every register that REPORTS mux state — each read
//     its accepted value. See docs/dev/evidence/rmbp-0915/GMUX-1-PRESWITCH.md §3-§5.
//   * DDC      0x28 ONLY — `GMUX_SWITCH_DDC` (:252). THERE IS NO 0x29 CONSTANT IN THIS FILE, and
//     that is not an omission: DDC is the one port whose ECHO is metal-proven on this machine, and
//     it is the only read-back allowed to abort the flight at the switch rung. DDC is therefore
//     scored on 0x28 BY MEASUREMENT, not by the pair convention — the single exception, and the
//     reason this decode takes DDC's own port rather than a status twin it does not have.
//
// A ZERO-COMPARE IS NEVER A VERDICT. `gmux_index_read` returns 0xFFFFFFFF on timeout, and a gmux
// that is dead, not ready, or answering a wrong index returns 0x00 or 0xFF. None of those is a mux
// state, and folding them into "not accepted" would let a boot that learned NOTHING look like a
// boot that learned the mux was in the wrong place. They decode to UNREADABLE, are printed as
// such on the pre-switch witness line, and refuse.
// ═══════════════════════════════════════════════════════════════════════════════════════════

/// The pre-switch gate's verdict. `Accept` is the only variant that may touch a mux.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
#[derive(Clone, Copy)]
enum PreSwitchGate {
    Accept,
    UnreadableDdc,
    UnreadableDisp,
    UnreadableExt,
    RefuseDdc,
    RefuseDisp,
    RefuseExt,
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
impl PreSwitchGate {
    /// The `gate=` field on the pre-switch witness line: the verdict AND the port that produced
    /// it, so a capture never needs the source to say which read decided.
    const fn token(self) -> &'static str {
        match self {
            PreSwitchGate::Accept => "ACCEPT",
            PreSwitchGate::UnreadableDdc => "UNREADABLE:ddc@0x28",
            PreSwitchGate::UnreadableDisp => "UNREADABLE:read_display@0x11",
            PreSwitchGate::UnreadableExt => "UNREADABLE:read_external@0x41",
            PreSwitchGate::RefuseDdc => "REFUSE:ddc@0x28",
            PreSwitchGate::RefuseDisp => "REFUSE:read_display@0x11",
            PreSwitchGate::RefuseExt => "REFUSE:read_external@0x41",
        }
    }

    const fn accepted(self) -> bool {
        matches!(self, PreSwitchGate::Accept)
    }

    /// A stable small code, so the `const _` pins below can assert on the exact variant.
    const fn code(self) -> u8 {
        match self {
            PreSwitchGate::Accept => 0,
            PreSwitchGate::UnreadableDdc => 1,
            PreSwitchGate::UnreadableDisp => 2,
            PreSwitchGate::UnreadableExt => 3,
            PreSwitchGate::RefuseDdc => 4,
            PreSwitchGate::RefuseDisp => 5,
            PreSwitchGate::RefuseExt => 6,
        }
    }
}

/// Not a mux state: the `gmux_index_read` timeout sentinel and the two all-bits answers a gmux
/// gives when it is dead, not ready, or was handed an index it does not implement.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const fn gmux_unreadable(v: u32) -> bool {
    v == 0x00 || v == 0xFF || v == 0xFFFF_FFFF
}

/// THE GATE'S DECODE. Pure, total, and — the whole point of GMUX-2 — structurally unable to score
/// a write-target port, because `GMUX_SWITCH_EXTERNAL` (0x40) and `GMUX_SWITCH_DISPLAY` (0x10) are
/// not parameters. The three arguments are the three reads that report mux STATE on this machine:
/// DDC's proven echo at 0x28, `GMUX_READ_DISPLAY` 0x11, and `GMUX_READ_EXTERNAL` 0x41.
///
/// Order matters for the witness, not for the result: UNREADABLE is tested before REFUSE on every
/// port so that a dead gmux is never reported as a mux in an unexpected place.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const fn gmux_preswitch_decode(ddc: u32, disp_status: u32, ext_status: u32) -> PreSwitchGate {
    if gmux_unreadable(ddc) {
        return PreSwitchGate::UnreadableDdc;
    }
    if gmux_unreadable(disp_status) {
        return PreSwitchGate::UnreadableDisp;
    }
    if gmux_unreadable(ext_status) {
        return PreSwitchGate::UnreadableExt;
    }
    // DDC: strict DIS — every pre-switch capture reads 0x02 and there is no second legitimate
    // member to admit. READ_DISPLAY: strict DIS — every recorded DISP=0x03 is THIS register.
    if ddc != GMUX_DDC_DIS as u32 {
        return PreSwitchGate::RefuseDdc;
    }
    if disp_status != GMUX_DISPLAY_DIS as u32 {
        return PreSwitchGate::RefuseDisp;
    }
    // READ_EXTERNAL: DIS *or* `GMUX_EXTERNAL_KEPLER_OWNED` (0x21). 0x21 is the Boot AK metal NORM —
    // what this machine reads when the firmware leaves the port Kepler-owned — which is why round
    // 11 relaxed this term. Demanding DIS would refuse on the only machine the flight was written
    // for.
    if ext_status != GMUX_EXTERNAL_DIS as u32 && ext_status != GMUX_EXTERNAL_KEPLER_OWNED as u32 {
        return PreSwitchGate::RefuseExt;
    }
    PreSwitchGate::Accept
}

/// THE RESTORE HALF. Given the ACCEPTED `GMUX_READ_EXTERNAL` (0x41) STATE read, what — if
/// anything — may honestly be written back to `GMUX_SWITCH_EXTERNAL` (0x40) on the unwind?
///
/// * `GMUX_EXTERNAL_DIS` (0x03) → `Some(GMUX_EXTERNAL_DIS)`. This is the ONE value whose
///   status→write map is known and cited: the port map comment at :238-243 gives EXTERNAL the
///   DISPLAY write encoding (`_IGD 0x2 / _DIS 0x3`), and 0x03 is the same named constant on both
///   sides. Writing it back is a restore, not a guess.
/// * `GMUX_EXTERNAL_KEPLER_OWNED` (0x21) → `None`. 0x21 is a STATUS encoding only. Nothing in this
///   file, and nothing in upstream apple-gmux as this file quotes it, names a value you WRITE to
///   0x40 to put the port back to Kepler-owned. An uncertain map is not a map, so the unwind
///   writes nothing there and says `restore ext=SKIPPED` on the wire. This is the branch the 2012
///   rMBP takes: flight 5 read `EXT=0x21`.
/// * anything else → `None`, unreachable in practice because the gate already refused it.
///
/// The old code wrote the 0x40 READ back to 0x40. On flight 5 that read was `0x01` — a value no
/// encoding in this file names — and it would have gone into the external display mux on EVERY
/// exit path, including the self-test's immediate `unwind.execute()`. That is the silent state
/// change the `GMUX_EXTERNAL_KEPLER_OWNED` doc-comment at :265-273 forbids, and it is why
/// deleting the gate term alone was never the fix.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const fn ext_restore_target(ext_status: u32) -> Option<u8> {
    if ext_status == GMUX_EXTERNAL_DIS as u32 {
        Some(GMUX_EXTERNAL_DIS)
    } else {
        None
    }
}

// THE PINS. `cargo test` never runs on this `no_std` kernel crate — `./arroyo check` is the gate
// and `#[cfg(test)]` code is invisible to it, which is exactly why the `mod tests` at the foot of
// `kepler.rs` was deleted rather than repaired ("a test that cannot run is a comment that lies
// about being a test", kepler.rs:3906-3918). `const _` blocks ARE evaluated by the gate, so the
// decode's behaviour is pinned here, where a wrong answer is a BUILD FAILURE on both arches'
// gmux_igd legs rather than a test nobody runs.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const _: () = {
    // FLIGHT 5's OWN CENSUS, byte for byte (GMUX-1-PRESWITCH.md §3): DDC=0x02, DISP=0x03,
    // EXT=0x21. This is the vector that refused. It must ACCEPT.
    assert!(gmux_preswitch_decode(0x02, 0x03, 0x21).code() == 0);
    // And it accepts REGARDLESS of what the write-target port 0x40 read, because 0x40 is not an
    // argument — the `0x01` that stopped flight 5 has nowhere to enter this decision.
    assert!(gmux_preswitch_decode(0x02, 0x03, 0x21).accepted());
    // The other accepted EXTERNAL member: a DIS-owned external port.
    assert!(gmux_preswitch_decode(0x02, 0x03, 0x03).code() == 0);

    // UNREADABLE beats REFUSE on every port. A zero-compare is never a verdict.
    assert!(gmux_preswitch_decode(0x00, 0x03, 0x21).code() == 1);
    assert!(gmux_preswitch_decode(0x02, 0xFF, 0x21).code() == 2);
    assert!(gmux_preswitch_decode(0x02, 0x03, 0x00).code() == 3);
    assert!(gmux_preswitch_decode(0x02, 0x03, 0xFF).code() == 3);
    // The gmux timeout sentinel, on each port in turn.
    assert!(gmux_preswitch_decode(0xFFFF_FFFF, 0x03, 0x21).code() == 1);
    assert!(gmux_preswitch_decode(0x02, 0xFFFF_FFFF, 0x21).code() == 2);
    assert!(gmux_preswitch_decode(0x02, 0x03, 0xFFFF_FFFF).code() == 3);
    // ...and NONE of them is an Accept.
    assert!(!gmux_preswitch_decode(0x02, 0x03, 0x00).accepted());
    assert!(!gmux_preswitch_decode(0x02, 0x03, 0xFFFF_FFFF).accepted());

    // The gate is still honest about real states it was not written for.
    assert!(gmux_preswitch_decode(0x01, 0x03, 0x21).code() == 4); // DDC routed to the iGPU
    assert!(gmux_preswitch_decode(0x02, 0x02, 0x21).code() == 5); // panel already IGD
    assert!(gmux_preswitch_decode(0x02, 0x03, 0x02).code() == 6); // EXTERNAL already IGD

    // The restore map: exactly one member is certain, and it is not the one this machine reads.
    assert!(matches!(ext_restore_target(0x03), Some(GMUX_EXTERNAL_DIS)));
    assert!(ext_restore_target(0x21).is_none()); // the 2012 rMBP's branch: SKIPPED
    assert!(ext_restore_target(0x01).is_none()); // flight 5's 0x40 read, had it ever been offered
    assert!(ext_restore_target(0xFFFF_FFFF).is_none());
};

// ═══════════════════════════════════════════════════════════════════════════════════════════
// GMUXDPCD — THE TWO NUMBERS RUNG 3 AND RUNG 4 SPEND, AND WHERE EACH ONE COMES FROM
//
// Appended at the FOOT for the reason the GMUX-2 block above already gives: everything below the
// gmux harness is nothing in a knob-OFF build, so an append here moves no knob-OFF line number and
// `panic::Location` cannot shift under a `gmux_igd` change (QUEUE B94). Both items carry the same
// `all(target_arch = "x86_64", feature = "gmux_igd")` cfg as the rest of the ladder.
//
// NEITHER OF THESE IS A SPEC CITATION, and each says so in its own name and on the wire. That is
// deliberate: this rung's whole finding is that the tree cannot cite the PPS bit map, and a
// budget dressed up as a cited T3 would be the same error one register along.

/// The settle rung 3 spends between the gmux write and the first AUX transaction, in ms.
///
/// **A BUDGET, NOT A CITED T3.** The eDP panel-power-on delay for this panel is not in this tree:
/// `PCH_PP_ON_DELAYS` reads `0x00000000` on this part (firmware never programmed T1..T8), the DPCD
/// that would carry the panel's own figure is what rung 4 is trying to read, and the field layout
/// that would decode `PP_ON_DELAYS` is marked TBV against PRM Vol 3 Part 4 "Panel Power
/// Sequencing" in `docs/dev/OS/08_VIDEO/` — a document this tree does not hold. So 210 is chosen,
/// not derived, on the one principle that design doc does give for panel timings: **bias long** —
/// a panel given too much time works, one given too little does not. It is printed on the wire as
/// `budget=not-a-cited-T3` so no later reader can promote it to a measurement, and it costs the
/// boot a fifth of a second against a ladder that took 9 ms and learned nothing.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const PP_SETTLE_MS: u64 = 210;

/// How many times rung 4 asks the sink for DPCD 0x00000 before it gives up.
///
/// **INHERITED FROM THIS FILE, NOT FROM THE DP SPEC.** `dp_aux_transfer` already gives up after
/// seven AUX DEFERs (`retry_count >= 7`); seven is reused here so one number governs "how many
/// times will this ladder ask" rather than two that can drift apart. The hardware timeout needs no
/// such choice — it is already pinned at the maximum the field encodes
/// (`DP_AUX_CH_CTL_TIME_OUT_1600US`, `3 << 26`, confirmed on the wire by flight 10's failing
/// status `0x5D4000C8`, bits 27:26 = 0b11).
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DPCD_TRIES: u32 = 7;

/// The gap rung 4 leaves between two AUX attempts, in ms. Same standing as `PP_SETTLE_MS`: a
/// budget. Seven attempts at this spacing put the last attempt roughly 120 ms past the first,
/// inside the 2 s `window_deadline` the rung already carried, so the retry window can never
/// outrun the bound that was already there.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const DPCD_RETRY_GAP_MS: u64 = 20;

// ═══════════════════════════════════════════════════════════════════════════════════════════
// GMUX7 — NAMING THE UNWIND ENTRIES THE ROLLUP HAS ONLY EVER COUNTED
//
// Appended at the FOOT for the reason the two blocks above already give: everything below the
// gmux harness is nothing in a knob-OFF build, so an append here moves no knob-OFF line number and
// `panic::Location` cannot shift under a `gmux_igd` change (QUEUE B94). Every item carries the
// same `all(target_arch = "x86_64", feature = "gmux_igd")` cfg as the rest of the ladder.
//
// WHAT `pending=2` MEANS ON THIS MACHINE, which is the question flight 11 left on the table. The
// number is `DisplayUnwind::len` sampled immediately before the revert, and the stack at that
// instant is the SECOND push set (the first is drained by the self-test's own `execute()` at the
// `selftest` rung). That set is pushed EXTERNAL → DISPLAY → DDC and is LIFO, so:
//
//   * `GMUX_SWITCH_EXTERNAL` (0x40) is pushed ONLY when `ext_restore_target` mapped the 0x41
//     STATUS read back to a named write encoding. On the 2012 rMBP it reads `0x21`
//     (kepler-owned), for which this tree has no cited status→write map, so it is NOT pushed —
//     `restore ext=SKIPPED` says so on its own line. THAT ABSENCE IS THE WHOLE REASON THE COUNT
//     IS 2 AND NOT 3.
//   * `GMUX_SWITCH_DISPLAY` (0x10) ← `GMUX_DISPLAY_DIS` (0x03).
//   * `GMUX_SWITCH_DDC` (0x28) ← `GMUX_DDC_DIS` (0x02).
//
// So `pending=2` has always meant "the two gmux restores, DDC first then DISPLAY, and EXTERNAL
// deliberately absent" — and from this commit the line SAYS that instead of leaving a reader to
// re-derive it from three push sites and a `None`.
// ═══════════════════════════════════════════════════════════════════════════════════════════

/// The gmux index port, by the name this file's own constant block gives it.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const fn gmux_reg_name(reg: u8) -> &'static str {
    match reg {
        GMUX_SWITCH_DDC => "SWITCH_DDC",
        GMUX_SWITCH_DISPLAY => "SWITCH_DISPLAY",
        GMUX_SWITCH_EXTERNAL => "SWITCH_EXTERNAL",
        _ => "UNNAMED",
    }
}

/// The MMIO offsets the unwind can carry. Only the self-test ever pushes one (`DPA_AUX_CH_DATA1`),
/// and it drains its own entry, so `UNNAMED` here is itself a finding rather than a formatting
/// gap: it would mean an MMIO entry survived to the revert, which no path in this file writes.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const fn unwind_mmio_name(off: u32) -> &'static str {
    if off as usize == regs::DPA_AUX_CH_DATA1 {
        "DPA_AUX_CH_DATA1"
    } else {
        "UNNAMED"
    }
}

/// Print the entries still on the unwind stack in the order `DisplayUnwind::execute` will replay
/// them — LIFO, i.e. the order the forward writes went out — as a comma-separated list with no
/// spaces, so the rollup stays one `awk`-able line. `none` when the stack is empty, which is what
/// a `pending=0` flight (the pre-switch refusal of flight 5) prints.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn print_pending_items(entries: &[UnwindEntry; 32], len: usize) {
    if len == 0 {
        serial_print!("none");
        return;
    }
    let mut i = len;
    while i > 0 {
        i -= 1;
        if i + 1 != len {
            serial_print!(",");
        }
        match entries[i] {
            UnwindEntry::Gmux { reg, pre } => {
                serial_print!("gmux:{}@0x{:02X}<-0x{:02X}", gmux_reg_name(reg), reg, pre);
            }
            UnwindEntry::Mmio { off, pre } => {
                serial_print!("mmio:{}@0x{:05X}<-0x{:08X}", unwind_mmio_name(off), off, pre);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════
// GMUX8 — THE FIELD DECODES FOR RUNGS 07b AND 08b, EACH ONE A PRM TABLE AND NOTHING ELSE
//
// Appended at the FOOT for the reason the GMUX-2 and GMUXDPCD blocks above already give:
// everything below the gmux harness is nothing in a knob-OFF build, so an append here moves no
// knob-OFF line number and `panic::Location` cannot shift under a `gmux_igd` change (QUEUE B94).
// Every item carries the same `all(target_arch = "x86_64", feature = "gmux_igd")` cfg.
//
// ⚠ UNLIKE THE GMUXDPCD BLOCK BELOW WHICH IT SITS BESIDE, EVERY VALUE HERE *IS* A SPEC CITATION.
// Each function below transcribes one `Value / Name / Description` table out of a public Intel
// document, named in its doc-comment with section and page, and contains no judgement of its own:
// an encoding the PRM marks Reserved returns the string `reserved`, never a guess at what the
// silicon might do with it. Nothing here was taken from `i915`, `i965` or any driver source.
//
// The one thing these functions deliberately do NOT do is convert. `dp_pll_freq_name` returns
// "270mhz" because that is the PRM's own spelling of the encoding at §4.4.1 p.85; the kHz number
// the MODE line divides by is formed at the call site, where the law it is used under
// (`Link M/N = dot clock / ls_clk`, §4.2 p.74) is written down next to it.

/// `PP_STATUS` [29:28] Power Sequence Progress — IVB PRM Vol3 Pt4 (`IHD-OS-V3 Pt 4 – 05 12`)
/// §2.4.1 p.39: `00b` None, `01b` Power Up, `10b` Power Down, `11b` Reserved.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn pp_seq_name(v: u32) -> &'static str {
    match v {
        0b00 => "none",
        0b01 => "power-up",
        0b10 => "power-down",
        _ => "reserved",
    }
}

/// `PP_ON_DELAYS` [31:30] Panel control port select — IVB PRM Vol3 Pt4 §2.4.3 p.41: `00b` LVDS,
/// `01b` DisplayPort A, `10b` DisplayPort C, `11b` DisplayPort D. The same page carries the
/// workaround that ties `01b` to a `PP_CONTROL` write-protect key of `0xABCD`, which is why rung
/// 07b scores the two against each other.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn pp_port_name(v: u32) -> &'static str {
    match v {
        0b00 => "LVDS",
        0b01 => "DP-A",
        0b10 => "DP-C",
        _ => "DP-D",
    }
}

/// `DP_CTL` [21:19] Port Width Selection — IVB PRM Vol3 Pt3 (`IHD-OS-V3 Pt 3 – 05 12`) §4.4.1
/// p.85: `000b` x1, `001b` x2, `011b` x4, others Reserved. Note the gap at `010b`: the PRM lists
/// no x3 and this returns `reserved` rather than inventing one.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn dp_width_name(v: u32) -> &'static str {
    match v {
        0b000 => "x1",
        0b001 => "x2",
        0b011 => "x4",
        _ => "reserved",
    }
}

/// `DP_CTL` [17:16] DP PLL Frequency — IVB PRM Vol3 Pt3 §4.4.1 p.85: `00b` 270mhz, `01b` 162mhz,
/// others Reserved. This is the SOURCE-side link symbol clock and is the only cited link rate
/// this ladder has; the DPCD `MAX_LINK_RATE` byte rung 06 reads is a SINK capability in a DP-spec
/// encoding and stays `rate_decode=TBV-tree-table` until that encoding is cited from the DP spec.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn dp_pll_freq_name(v: u32) -> &'static str {
    match v {
        0b00 => "270mhz",
        0b01 => "162mhz",
        _ => "reserved",
    }
}

/// `DP_CTL` [9:8] Link training pattern enable — IVB PRM Vol3 Pt3 §4.4.1 pp.85–86: `00b`
/// Pattern 1, `01b` Pattern 2, `10b` Idle, `11b` Normal (send normal pixels). This is Ivy
/// Bridge's whole `DP_TP_CTL`: there is no such register on this part.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn dp_train_pat_name(v: u32) -> &'static str {
    match v {
        0b00 => "pattern1",
        0b01 => "pattern2",
        0b10 => "idle",
        _ => "normal",
    }
}

/// `PIPE_CONF` [7:5] Bits Per Color — IVB PRM Vol3 Pt3 §5.1.3 p.102: `000b` 8bpc, `001b` 10bpc,
/// `010b` 6bpc, `011b` 12bpc, others Reserved. The ordering is the PRM's and is not monotonic.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn pipe_bpc_name(v: u32) -> &'static str {
    match v {
        0b000 => "8bpc",
        0b001 => "10bpc",
        0b010 => "6bpc",
        0b011 => "12bpc",
        _ => "reserved",
    }
}

/// The per-pipe timing/M-N offsets rung 08b reads, chosen by `DP_CTL_A` [30:29] Pipe Select
/// (IVB PRM Vol3 Pt3 §4.4.1 p.82: `00b` Pipe A, `01b` Pipe B, `10b` Pipe C, `11b` Reserved).
///
/// `11b` is Reserved and there is no fourth pipe to read, so it falls back to pipe A and the
/// `name` field says `A(pipe-sel-reserved)` — the fallback is never silent, and rung 08b also
/// prints `pipe_sel_raw=` so the encoding is on the wire whatever this returns.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
#[derive(Clone, Copy)]
struct PipeTimingRegs {
    name: &'static str,
    conf: usize,
    srcsz: usize,
    htotal: usize,
    hblank: usize,
    hsync: usize,
    vtotal: usize,
    vblank: usize,
    vsync: usize,
    datam1: usize,
    datan1: usize,
    linkm1: usize,
    linkn1: usize,
    datam2: usize,
    datan2: usize,
    linkm2: usize,
    linkn2: usize,
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const fn pipe_timing_regs(sel: u32) -> PipeTimingRegs {
    match sel {
        0b01 => PipeTimingRegs {
            name: "B",
            conf: regs::PIPEBCONF,
            srcsz: regs::PIPEBSRC,
            htotal: regs::PIPE_HTOTAL_B,
            hblank: regs::PIPE_HBLANK_B,
            hsync: regs::PIPE_HSYNC_B,
            vtotal: regs::PIPE_VTOTAL_B,
            vblank: regs::PIPE_VBLANK_B,
            vsync: regs::PIPE_VSYNC_B,
            datam1: regs::PIPE_DATAM1_B,
            datan1: regs::PIPE_DATAN1_B,
            linkm1: regs::PIPE_LINKM1_B,
            linkn1: regs::PIPE_LINKN1_B,
            datam2: regs::PIPE_DATAM2_B,
            datan2: regs::PIPE_DATAN2_B,
            linkm2: regs::PIPE_LINKM2_B,
            linkn2: regs::PIPE_LINKN2_B,
        },
        0b10 => PipeTimingRegs {
            name: "C",
            conf: regs::PIPECCONF,
            srcsz: regs::PIPECSRC,
            htotal: regs::PIPE_HTOTAL_C,
            hblank: regs::PIPE_HBLANK_C,
            hsync: regs::PIPE_HSYNC_C,
            vtotal: regs::PIPE_VTOTAL_C,
            vblank: regs::PIPE_VBLANK_C,
            vsync: regs::PIPE_VSYNC_C,
            datam1: regs::PIPE_DATAM1_C,
            datan1: regs::PIPE_DATAN1_C,
            linkm1: regs::PIPE_LINKM1_C,
            linkn1: regs::PIPE_LINKN1_C,
            datam2: regs::PIPE_DATAM2_C,
            datan2: regs::PIPE_DATAN2_C,
            linkm2: regs::PIPE_LINKM2_C,
            linkn2: regs::PIPE_LINKN2_C,
        },
        _ => PipeTimingRegs {
            name: if sel == 0b00 { "A" } else { "A(pipe-sel-reserved)" },
            conf: regs::PIPEACONF,
            srcsz: regs::PIPEASRC,
            htotal: regs::PIPE_HTOTAL_A,
            hblank: regs::PIPE_HBLANK_A,
            hsync: regs::PIPE_HSYNC_A,
            vtotal: regs::PIPE_VTOTAL_A,
            vblank: regs::PIPE_VBLANK_A,
            vsync: regs::PIPE_VSYNC_A,
            datam1: regs::PIPE_DATAM1_A,
            datan1: regs::PIPE_DATAN1_A,
            linkm1: regs::PIPE_LINKM1_A,
            linkn1: regs::PIPE_LINKN1_A,
            datam2: regs::PIPE_DATAM2_A,
            datan2: regs::PIPE_DATAN2_A,
            linkm2: regs::PIPE_LINKM2_A,
            linkn2: regs::PIPE_LINKN2_A,
        },
    }
}
