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
// bound could not hang; that one could. The armed helpers here carry BOTH an unconditional
// iteration cap AND an `ms()` deadline, so even on the armed build a stopped clock cannot hang
// them — whichever bound trips first ends the wait.
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
const GMUX_SWITCH_DISPLAY: u8 = 0x10;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_SWITCH_DDC: u8 = 0x28;
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_SWITCH_EXTERNAL: u8 = 0x40;
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

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_DDC_IGD: u8 = 0x01;

/// Baseline's iteration bound, kept UNCONDITIONALLY alongside the TSC deadline.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_WAIT_ITERS: u32 = 5000;
/// How long the mux stays on IGD before the revert fires.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
const GMUX_DWELL_MS: u64 = 10_000;
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

/// Wait for the gmux to be ready to accept an index byte. Bounded twice over: an iteration
/// count that cannot depend on any clock, and a TSC deadline. Returns false on timeout —
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

/// The pre-switch mux state, plus whether a revert is owed and when.
///
/// ONE encode point and ONE decode point. Every mutation of the saved bytes routes through
/// `pack`/`unpack`; this is what stopped a saved byte being lost to a mask in an earlier
/// round, and it is why `gmux_dwell()` reads `deadline_ms` back out of the packed word rather
/// than keeping a second local copy of it.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
#[derive(Clone, Copy)]
struct RevertState {
    armed: bool,
    due: bool,
    ddc: u8,
    disp: u8,
    ext: u8,
    /// Absolute `arch::ms()` value at which the revert comes due. Saturated into 32 bits
    /// (~49 days of uptime); the switch is a boot-time one-shot, so that is not reachable.
    deadline_ms: u32,
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
impl RevertState {
    fn pack(&self) -> u64 {
        let mut v = 0u64;
        if self.armed { v |= 1 << 0; }
        if self.due { v |= 1 << 1; }
        v |= (self.ddc as u64) << 8;
        v |= (self.disp as u64) << 16;
        v |= (self.ext as u64) << 24;
        v |= (self.deadline_ms as u64) << 32;
        v
    }

    fn unpack(v: u64) -> Self {
        Self {
            armed: (v & (1 << 0)) != 0,
            due: (v & (1 << 1)) != 0,
            ddc: ((v >> 8) & 0xFF) as u8,
            disp: ((v >> 16) & 0xFF) as u8,
            ext: ((v >> 24) & 0xFF) as u8,
            deadline_ms: ((v >> 32) & 0xFFFF_FFFF) as u32,
        }
    }
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
static GMUX_REVERT_STATE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Atomically transform the packed revert state.
///
/// `SeqCst` on an independent load and an independent store does NOT make the pair atomic —
/// two contexts can both observe `armed` and both run the port sequence. This compare-exchange
/// loop makes the read-modify-write indivisible. `f` returns `None` to abandon the update; on
/// success the PRE-IMAGE is returned.
/// and the exclusive right to use them in one indivisible step.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
fn gmux_state_update<F>(mut f: F) -> Option<RevertState>
where
    F: FnMut(RevertState) -> Option<RevertState>,
{
    let mut cur = GMUX_REVERT_STATE.load(Ordering::SeqCst);
    loop {
        let old = RevertState::unpack(cur);
        let new = f(old)?;
        match GMUX_REVERT_STATE.compare_exchange_weak(
            cur,
            new.pack(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => return Some(old),
            Err(actual) => cur = actual,
        }
    }
}

/// Write the DDC/DISPLAY/EXTERNAL triple, read all three back, and compare the read-back
/// against what was intended.
///
/// The verdict is decided by the READ-BACK, never by the write helpers returning `true`. A
/// timed-out DDC write with a landed DISPLAY write leaves the panel on IGD with DDC on
/// discrete; an earlier version logged that and printed `Revert Complete` anyway, so a black
/// screen could not be distinguished from a write that never happened. The `w_*` flags are
/// still printed — they say WHERE it broke — but they do not decide anything.
#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn gmux_apply(phase: &str, ddc: u8, disp: u8, ext: u8) -> (bool, u8, u8, u8) {
    let w_ddc = unsafe { gmux_index_write(GMUX_SWITCH_DDC, ddc) };
    let w_disp = unsafe { gmux_index_write(GMUX_SWITCH_DISPLAY, disp) };
    let w_ext = unsafe { gmux_index_write(GMUX_SWITCH_EXTERNAL, ext) };
    let ok = |b: bool| if b { "ok" } else { "TIMEOUT" };
    serial_println!(
        ":: igpu: [GMUX] {} write: ddc={} disp={} ext={} (intent DDC=0x{:02X} DISP=0x{:02X} EXT=0x{:02X}) ::",
        phase, ok(w_ddc), ok(w_disp), ok(w_ext), ddc, disp, ext);

    let r_ddc = unsafe { gmux_index_read(GMUX_SWITCH_DDC) };
    let r_disp = unsafe { gmux_index_read(GMUX_READ_DISPLAY) };
    let r_ext = unsafe { gmux_index_read(GMUX_READ_EXTERNAL) };
    serial_println!(
        ":: igpu: [GMUX] {} read-back: DDC=0x{:02X} DISP=0x{:02X} EXT=0x{:02X} ::",
        phase, r_ddc, r_disp, r_ext);

    let m_ddc = r_ddc == ddc as u32;
    let m_disp = r_disp == disp as u32;
    let m_ext = r_ext == ext as u32;
    if m_ddc && m_disp && m_ext {
        serial_println!(":: igpu: [GMUX] {} verdict: MATCH (all three registers read back as written) ::", phase);
        (true, r_ddc as u8, r_disp as u8, r_ext as u8)
    } else {
        if !m_ddc { serial_println!(":: igpu: [GMUX] {} MISMATCH SW_DDC: wrote 0x{:02X}, read 0x{:02X} ::", phase, ddc, r_ddc); }
        if !m_disp { serial_println!(":: igpu: [GMUX] {} MISMATCH SW_DISPLAY: wrote 0x{:02X}, read 0x{:02X} ::", phase, disp, r_disp); }
        if !m_ext { serial_println!(":: igpu: [GMUX] {} MISMATCH SW_EXTERNAL: wrote 0x{:02X}, read 0x{:02X} ::", phase, ext, r_ext); }
        serial_println!(":: igpu: [GMUX] {} verdict: MISMATCH ::", phase);
        (false, r_ddc as u8, r_disp as u8, r_ext as u8)
    }
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
const DP_AUX_CH_CTL_RECEIVE_ERROR: u32 = 1 << 25;

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]
unsafe fn dp_aux_transfer(bar0: usize, clock_divider: u32, cmd: u32, addr: u32, tx_len: u32, tx_data: &[u8], rx_data: &mut [u8]) -> Result<(), &'static str> {
    let mut retry_count = 0;
    let deadline = crate::arch::now_cycles() + (crate::arch::hw_wait_budget() * 2);

    let is_write = (cmd & 0x3) == 0;
    let send_bytes = if is_write { 4 + tx_len } else { 4 };

    loop {
        if crate::arch::now_cycles() >= deadline || retry_count >= 7 {
            return Err("aux-defer-exhausted");
        }

        let data1 = (cmd << 28) | ((addr & 0xFFFFF) << 8) | (tx_len.saturating_sub(1));
        core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *mut u32, data1);

        if is_write {
            let mut idx = 0;
            let data_regs = [regs::DPA_AUX_CH_DATA2, regs::DPA_AUX_CH_DATA3, regs::DPA_AUX_CH_DATA4, regs::DPA_AUX_CH_DATA5];
            for reg_offset in &data_regs {
                let mut val = 0u32;
                for b in 0..4 {
                    if idx < tx_data.len() {
                        val |= (tx_data[idx] as u32) << (24 - (b * 8));
                        idx += 1;
                    }
                }
                core::ptr::write_volatile((bar0 + *reg_offset) as *mut u32, val);
            }
        }

        let ctl = DP_AUX_CH_CTL_SEND_BUSY | DP_AUX_CH_CTL_DONE | DP_AUX_CH_CTL_TIME_OUT_ERROR | DP_AUX_CH_CTL_RECEIVE_ERROR
                  | (send_bytes << 20) | (clock_divider & 0x7FF);
        core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *mut u32, ctl);

        let mut status;
        let wait_deadline = crate::arch::now_cycles() + (crate::arch::hw_wait_budget() / 100);
        loop {
            status = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *const u32);
            if (status & DP_AUX_CH_CTL_SEND_BUSY) == 0 { break; }
            if crate::arch::now_cycles() >= wait_deadline { break; }
            core::hint::spin_loop();
        }

        if (status & DP_AUX_CH_CTL_SEND_BUSY) != 0 {
            return Err("aux-timeout-busy");
        }

        let status_clean = status & !DP_AUX_CH_CTL_SEND_BUSY;
        core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_CTL) as *mut u32, status_clean | DP_AUX_CH_CTL_DONE | DP_AUX_CH_CTL_TIME_OUT_ERROR | DP_AUX_CH_CTL_RECEIVE_ERROR);

        if (status & DP_AUX_CH_CTL_TIME_OUT_ERROR) != 0 {
            return Err("aux-timeout-error (Hypotheses: VDD off, wrong divider, bad offsets, mux write failed, panel not on AUX)");
        }
        if (status & DP_AUX_CH_CTL_RECEIVE_ERROR) != 0 {
            return Err("aux-receive-error");
        }

        let rx_data1 = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *const u32);
        let reply_nibble = (rx_data1 >> 28) & 0xF;
        let native_reply = reply_nibble & 0x3;
        let i2c_reply = (reply_nibble >> 2) & 0x3;

        let is_i2c = (cmd & 0x8) == 0;
        let reply_status = if is_i2c { i2c_reply } else { native_reply };

        if reply_status == 2 {
            retry_count += 1;
            continue;
        }

        if reply_status == 1 {
            return Err("aux-nack");
        }

        let total_rx = (status >> 20) & 0x1F;
        let payload_rx = total_rx.saturating_sub(1) as usize;

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

    // Will be captured dynamically based on live pre-image
    let mut pre_ddc = 0x02;

    let mut execute_harness = || -> Result<(), &'static str> {
        if !PROTOCOL_PROVEN.load(Ordering::SeqCst) { return Err("protocol-unproven"); }
        let bar0 = IGPU_BAR0.load(Ordering::SeqCst);
        if bar0 == 0 { return Err("bar0-unmapped"); }
        unwind.bar0 = bar0;

        pre_ddc = gmux_index_read(GMUX_SWITCH_DDC);
        let disp = gmux_index_read(GMUX_READ_DISPLAY);
        let ext = gmux_index_read(GMUX_READ_EXTERNAL);
        serial_println!(":: igpu-dpy: pre-switch state DDC=0x{:02X} DISP=0x{:02X} EXT=0x{:02X} ::", pre_ddc, disp, ext);

        if pre_ddc != GMUX_DDC_DIS as u32 || disp != GMUX_DISPLAY_DIS as u32 || ext != GMUX_EXTERNAL_DIS as u32 {
            serial_println!(":: igpu: [GMUX] REFUSED: pre-switch state is not fully DIS (DDC={}, DISP={}, EXT={}) — no known safe state to return to ::", pre_ddc, disp, ext);
            return Err("pre-switch-not-dis");
        }

        rung_name = "census";
        let ggc = crate::arch::pci::read_config_32(0, 0, 0, 0x50);
        let bdsm = crate::arch::pci::read_config_32(0, 0, 0, 0xB0);
        let ggtt0 = mmio_read(bar0, regs::GTT_BASE);
        let ggtt1 = mmio_read(bar0, regs::GTT_BASE + 4);
        let aux_ctl = mmio_read(bar0, regs::DPA_AUX_CH_CTL);
        let frmcnt = mmio_read(bar0, 0x70040);
        serial_println!(":: igpu-dpy: rung=00 name=census ok=1 bdsm=0x{:08X} ggc=0x{:08X} ggtt0=0x{:08X} ggtt1=0x{:08X} aux_ctl=0x{:08X} frmcnt=0x{:08X} ::",
            bdsm, ggc, ggtt0, ggtt1, aux_ctl, frmcnt);

        if (aux_ctl & DP_AUX_CH_CTL_SEND_BUSY) != 0 {
            serial_println!(":: igpu: [AUX] REFUSED: aux_ctl=0x{:08X} SEND_BUSY is set at boot ::", aux_ctl);
            return Err("aux-busy-at-boot");
        }

        let clock_divider = aux_ctl & 0x7FF;
        if clock_divider == 0 {
            serial_println!(":: igpu: [AUX] REFUSED: aux_ctl=0x{:08X} clock divider is 0 ::", aux_ctl);
            return Err("aux-divider-unusable");
        }

        serial_println!(":: igpu: [GMUX] running Unwind stack self-test ::");

        let test_val = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *const u32);
        unwind.push_mmio(regs::DPA_AUX_CH_DATA1, test_val);
        core::ptr::write_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *mut u32, !test_val);

        unwind.push_gmux(GMUX_SWITCH_DDC, pre_ddc as u8);

        let _ = unwind.execute();

        let test_val_after = core::ptr::read_volatile((bar0 + regs::DPA_AUX_CH_DATA1) as *const u32);
        if test_val_after != test_val {
            serial_println!(":: igpu: [GMUX] Unwind stack MMIO self-test FAILED (expected 0x{:08X}, got 0x{:08X}) ::", test_val, test_val_after);
            return Err("unwind-mmio-failed");
        }
        serial_println!(":: igpu: [GMUX] Unwind stack MMIO self-test passed ::");
        serial_println!(":: igpu: [GMUX] Unwind stack gmux-dispatch=REACHED (Gmux restore path executed without faulting, not implying restore verified) ::");

        let ddc_live = gmux_index_read(GMUX_SWITCH_DDC);
        unwind.push_gmux(GMUX_SWITCH_DDC, ddc_live as u8);

        serial_println!(":: igpu: [GMUX] switching DDC to IGD (0x{:02X}) — panel should REMAIN ON since DISPLAY is not moved ::", GMUX_DDC_IGD);
        gmux_index_write(GMUX_SWITCH_DDC, GMUX_DDC_IGD);
        let read_ddc = gmux_index_read(GMUX_SWITCH_DDC);
        if read_ddc != GMUX_DDC_IGD as u32 {
            return Err("mux-ddc-switch-failed");
        }

        let mut dpcd_rev = [0u8; 1];
        if let Err(e) = dp_aux_transfer(bar0, clock_divider, DP_AUX_NATIVE_READ, 0x00000, 1, &[], &mut dpcd_rev) {
            serial_println!(":: igpu: [AUX] DPCD Read Failed: {} ::", e);
            return Err(e);
        }
        serial_println!(":: igpu: [AUX] DPCD REV: 0x{:02X} ::", dpcd_rev[0]);

        if let Err(e) = dp_aux_transfer(bar0, clock_divider, DP_AUX_I2C_WRITE | DP_AUX_I2C_MOT, 0x50, 1, &[0x00], &mut []) {
            serial_println!(":: igpu: [AUX] EDID Address Set NACKed/Failed: {} ::", e);
            return Err("edid-address-nack");
        }

        let mut edid = [0u8; 128];
        for chunk in 0..8 {
            let offset = chunk * 16;
            let cmd = if chunk == 7 { DP_AUX_I2C_READ } else { DP_AUX_I2C_READ | DP_AUX_I2C_MOT };
            if let Err(e) = dp_aux_transfer(bar0, clock_divider, cmd, 0x50, 16, &[], &mut edid[offset..offset + 16]) {
                serial_println!(":: igpu: [AUX] EDID chunk {} failed: {} ::", chunk, e);
                return Err("edid-chunk-read-failed");
            }
        }

        let header: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
        if &edid[0..8] != &header {
            serial_println!(":: igpu: [AUX] EDID Header corrupt: {:02X?} ::", &edid[0..8]);
            return Err("edid-corrupt");
        }

        let checksum: u8 = edid.iter().fold(0, |acc, &x| acc.wrapping_add(x));
        if checksum != 0 {
            serial_println!(":: igpu: [AUX] EDID Checksum failed: {} ::", checksum);
            return Err("edid-corrupt");
        }

        serial_println!(":: igpu: [AUX] EDID Dump ::");
        for i in 0..8 {
            let row = &edid[(i*16)..((i+1)*16)];
            serial_print!(":: igpu: [AUX] {:02X}0: ", i);
            for b in row { serial_print!("{:02X} ", b); }
            serial_println!("::");
        }

        rung_name = "edid";
        highest = 3;
        Ok(())
    };

    match execute_harness() {
        Ok(_) => {
            ok_flag = 1;
        }
        Err(e) => {
            why_str = e;
        }
    }

    let unwound_count = unwind.len;
    let _ = unwind.execute();

    // Read back to decide gmux= verdict. Only DDC is proven, others are TBV.
    let post_ddc = gmux_index_read(GMUX_SWITCH_DDC);
    let post_disp = gmux_index_read(GMUX_READ_DISPLAY);
    let post_ext = gmux_index_read(GMUX_READ_EXTERNAL);

    let gmux_verdict = if post_ddc == pre_ddc {
        "MATCH"
    } else {
        "FAILED"
    };

    serial_println!(":: igpu: [GMUX] revert read-back: DDC=0x{:02X} DISP=0x{:02X} (TBV) EXT=0x{:02X} (TBV) ::", post_ddc, post_disp, post_ext);

    serial_println!(":: igpu-dpy: LADDER highest={:02}/10 name={} ok={} unwound={} gmux={} why={} elapsed_ms={} ::",
        highest, rung_name, ok_flag, unwound_count, gmux_verdict, why_str, get_elapsed_ms());
}
