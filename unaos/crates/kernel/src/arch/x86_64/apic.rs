// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Local APIC driver — xAPIC (MMIO) and x2APIC (MSR), chosen at runtime.
//
// Modern CPUs expose the local APIC two ways. The legacy xAPIC interface is a 4 KiB MMIO window
// at the architectural default 0xFEE00000 (identity-mapped by UEFI, like the xHCI BAR and the
// framebuffer — physical_memory_offset 0). The x2APIC interface replaces that window with a bank
// of MSRs: no MMIO, 32-bit APIC ids (so >255 CPUs), and a single-write ICR (no hi/lo split, no
// delivery-status poll) — what a real multi-core machine wants. We detect x2APIC via CPUID and
// drive whichever the CPU has behind one API (`init`/`eoi`/`apic_id`/`init_timer`/`send_ipi`),
// keeping the xAPIC path as a fallback for older hardware.
//
// Register *layouts* (SVR, the LVT entries, the timer) are identical across the two; only the
// access *method* differs, so `lapic_read`/`lapic_write` take both the MMIO offset and the MSR
// index and pick at runtime. EOI and the ICR are the only places the semantics differ.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::registers::model_specific::Msr;

/// Physical (== virtual, identity-mapped) base of the xAPIC register block.
const LAPIC_BASE: usize = 0xFEE0_0000;

// xAPIC MMIO register offsets (Intel SDM Vol.3, "Local APIC Register Address Map").
const REG_ID: usize = 0x020; // Local APIC ID (id in bits 31:24)
const REG_EOI: usize = 0x0B0; // End Of Interrupt (write 0)
const REG_SVR: usize = 0x0F0; // Spurious Interrupt Vector Register
const REG_ICR_LOW: usize = 0x300; // Interrupt Command Register, low dword (writing it sends)
const REG_ICR_HIGH: usize = 0x310; // Interrupt Command Register, high dword (destination)
const REG_LVT_TIMER: usize = 0x320; // LVT Timer entry
const REG_LVT_LINT0: usize = 0x350; // LVT LINT0 entry
const REG_LVT_LINT1: usize = 0x360; // LVT LINT1 entry
const REG_TIMER_INITCNT: usize = 0x380; // Timer Initial Count
const REG_TIMER_CURRCNT: usize = 0x390; // Timer Current Count (read-only, counts down)
const REG_TIMER_DIVIDE: usize = 0x3E0; // Timer Divide Configuration

// x2APIC MSR indices (the same registers, addressed as MSRs).
const IA32_APIC_BASE: u32 = 0x1B; // bit 10 = x2APIC enable (EXTD), bit 11 = global enable (EN)
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_X2APIC: u64 = 1 << 10;
const X2_ID: u32 = 0x802; // full 32-bit APIC id (no >>24)
const X2_EOI: u32 = 0x80B;
const X2_SVR: u32 = 0x80F;
const X2_ICR: u32 = 0x830; // single 64-bit write: dest in bits 63:32, command in bits 31:0
const X2_LVT_TIMER: u32 = 0x832;
const X2_LVT_LINT0: u32 = 0x835;
const X2_LVT_LINT1: u32 = 0x836;
const X2_TIMER_INITCNT: u32 = 0x838;
const X2_TIMER_CURRCNT: u32 = 0x839;
const X2_TIMER_DIVIDE: u32 = 0x83E;

/// True once any CPU has switched to x2APIC. Set on the BSP in `init`; the APs read it (in their
/// own `init`) to enable x2APIC in their own MSR. A `Relaxed` `AtomicBool` is lock-free and safe
/// from interrupt context (the hot path `eoi` reads it).
static X2APIC: AtomicBool = AtomicBool::new(false);

/// Monotonic count of local-APIC timer ticks since boot. A periodic heartbeat / hlt wake source
/// (and the basis for a future scheduler tick / uptime).
pub static APIC_TICKS: AtomicU64 = AtomicU64::new(0);

/// Calibrated invariant-TSC frequency in Hz, or 0 until `calibrate` succeeds. rdtsc is invariant
/// on Nehalem+ (incl. the Ivy Bridge rMBP), so this is a stable per-machine constant once measured.
static TSC_HZ: AtomicU64 = AtomicU64::new(0);

/// Calibrated local-APIC timer rate in Hz *after* the ÷16 divider — i.e. how fast the LVT-timer
/// down-counter actually decrements — or 0 until `calibrate` succeeds. The initial count for an
/// N-Hz periodic tick is `APIC_TIMER_HZ / N`.
static APIC_TIMER_HZ: AtomicU64 = AtomicU64::new(0);

#[inline]
fn x2apic() -> bool {
    X2APIC.load(Ordering::Relaxed)
}

#[inline]
unsafe fn mmio_read(reg: usize) -> u32 {
    core::ptr::read_volatile((LAPIC_BASE + reg) as *const u32)
}

#[inline]
unsafe fn mmio_write(reg: usize, val: u32) {
    core::ptr::write_volatile((LAPIC_BASE + reg) as *mut u32, val);
}

/// Read a local-APIC register through whichever interface is active.
#[inline]
unsafe fn lapic_read(mmio_off: usize, msr: u32) -> u32 {
    if x2apic() {
        Msr::new(msr).read() as u32
    } else {
        mmio_read(mmio_off)
    }
}

/// Write a local-APIC register through whichever interface is active.
#[inline]
unsafe fn lapic_write(mmio_off: usize, msr: u32, val: u32) {
    if x2apic() {
        Msr::new(msr).write(val as u64);
    } else {
        mmio_write(mmio_off, val);
    }
}

/// Detect x2APIC (CPUID.01H:ECX bit 21) and, if present, switch this CPU's local APIC into
/// x2APIC mode via IA32_APIC_BASE. Sets the global `X2APIC` flag. Idempotent and per-CPU: the
/// BSP calls it first (latching the mode), each AP calls it to flip its own MSR.
fn enable_x2apic_if_supported() {
    let supported = core::arch::x86_64::__cpuid(1).ecx & (1 << 21) != 0;
    if !supported {
        return; // old hardware: stay on the xAPIC MMIO path
    }
    unsafe {
        // Legal transition is xAPIC(EN=1) -> x2APIC(EN=1,EXTD=1); you cannot jump straight from
        // APIC-disabled to x2APIC. QEMU boots with EN=1, but enable it first to be safe.
        let mut base = Msr::new(IA32_APIC_BASE).read();
        if base & APIC_BASE_ENABLE == 0 {
            base |= APIC_BASE_ENABLE;
            Msr::new(IA32_APIC_BASE).write(base);
        }
        base |= APIC_BASE_X2APIC;
        Msr::new(IA32_APIC_BASE).write(base);
    }
    X2APIC.store(true, Ordering::Relaxed);
}

/// Software-enable this CPU's local APIC and configure its LVT entries, then arm the timer.
///
/// Detect/enable x2APIC first (so every subsequent access uses the right interface). SVR: set
/// bit 8 (APIC Software Enable) and the spurious vector to 0xFF (matches the `spurious_handler`
/// in the IDT). Pure local-APIC system, no 8259, so LINT0 is masked (no legacy ExtINT virtual
/// wire); LINT1 stays wired as NMI (a real hardware signal, not legacy ISA). Then the timer.
///
/// Callable by both the BSP (early `arch::init`) and each AP (in `ap_main`) — each CPU has its
/// own local APIC, so this configures the calling CPU's.
pub fn init() {
    enable_x2apic_if_supported();
    unsafe {
        let svr = lapic_read(REG_SVR, X2_SVR);
        lapic_write(REG_SVR, X2_SVR, svr | (1 << 8) | 0xFF);
        lapic_write(REG_LVT_LINT0, X2_LVT_LINT0, 1 << 16); // masked
        lapic_write(REG_LVT_LINT1, X2_LVT_LINT1, 0x400); // delivery mode 100b = NMI, unmasked

        serial_println!(
            "APIC: {} software-enabled (id={}, SVR={:#x}, LINT0=masked, LINT1=NMI).",
            if x2apic() { "x2APIC" } else { "xAPIC" },
            apic_id_u32(),
            lapic_read(REG_SVR, X2_SVR)
        );
    }

    init_timer();
}

/// Start the local APIC timer in periodic mode at the IDT timer vector, as the system heartbeat
/// / hlt wake source (replacing the retired 8254 PIT).
///
/// The initial count is a fixed empirical value: the APIC timer counts at the (unknown,
/// per-machine) core-crystal frequency, so this gives some convenient rate on QEMU but NOT a
/// precise tick. Exact timing is not needed yet (the tick is only a wake source). Real hardware
/// will need calibration (CPUID leaf 0x15 / TSC-deadline) — deferred.
pub fn init_timer() {
    let vector = crate::arch::interrupts::TIMER_VECTOR as u32;
    unsafe {
        // Divide the input clock by 16 (encoding 0b0011).
        lapic_write(REG_TIMER_DIVIDE, X2_TIMER_DIVIDE, 0x3);
        // LVT Timer: vector, bit 17 = periodic mode, bit 16 (mask) = 0.
        lapic_write(REG_LVT_TIMER, X2_LVT_TIMER, vector | (1 << 17));
        // Writing the initial count (last) arms the countdown; it reloads each period.
        lapic_write(REG_TIMER_INITCNT, X2_TIMER_INITCNT, 50_000);
    }
    serial_println!("APIC: timer armed (vector {:#x}, periodic, ÷16).", vector);
}

/// Calibrate the invariant TSC and the local-APIC timer against the fixed-frequency ACPI PM timer.
///
/// Ivy Bridge predates CPUID leaf 0x15/0x16, so neither the TSC nor the APIC-timer crystal rate is
/// discoverable — we *measure* them. Over one known PM-timer window (`CALIB_MS`) we count both how
/// many rdtsc cycles and how many APIC-timer counts elapse, then scale by the PM timer's spec
/// frequency to get each in Hz. rdtsc is invariant (constant rate across P-/C-/T-states), so its
/// measured Hz is a durable machine constant; the APIC rate lets us later pick an initial count
/// for an exact tick (commit 3 wires both into the timebase).
///
/// Runs with interrupts masked: the APIC timer is briefly repurposed as a free-running one-shot
/// down-counter (masked, so it never raises its IRQ) to be sampled, then the periodic heartbeat is
/// re-armed via `init_timer` before returning. On any failure (no PM advance, insane result) it
/// re-arms the timer and leaves the calibrated values at 0 so callers keep the fixed fallbacks.
pub fn calibrate(pm: &crate::arch::acpi::PmTimer) {
    /// Measurement window. 100 ms is long enough to average out I/O-read jitter yet only ~2% of a
    /// 24-bit PM timer's ~4.687 s wrap period, so the single-wrap `delta` stays valid.
    const CALIB_MS: u64 = 100;
    /// Hard ceiling on the calibration spin in rdtsc cycles (~4–20 s across 1–5 GHz): a backstop so
    /// a wedged/absent PM timer aborts instead of hanging the serial-less boot.
    const CALIB_MAX_TSC: u64 = 20_000_000_000;

    let pm_hz = crate::arch::acpi::PM_TIMER_HZ;
    let target_pm = (pm_hz * CALIB_MS / 1000) as u32; // ~357_954 ticks, << 2^24

    let (tsc_delta, apic_counts, elapsed_pm) = crate::arch::without_interrupts(|| unsafe {
        // Repurpose the APIC timer as a masked one-shot down-counter from the max initial count.
        // Masking (bit 16) stops IRQ delivery but NOT the counting, so we sample it freely; one-shot
        // (bits 18:17 = 00) means it counts toward 0 and stops — and 0xFFFF_FFFF counts at the ÷16
        // rate take many seconds to reach 0, far longer than the window, so it never underflows.
        let vector = crate::arch::interrupts::TIMER_VECTOR as u32;
        lapic_write(REG_TIMER_DIVIDE, X2_TIMER_DIVIDE, 0x3); // ÷16, same divider as the heartbeat
        lapic_write(REG_LVT_TIMER, X2_LVT_TIMER, vector | (1 << 16)); // masked, one-shot
        lapic_write(REG_TIMER_INITCNT, X2_TIMER_INITCNT, 0xFFFF_FFFF); // arm; starts counting down

        let pm_start = pm.read();
        let tsc_start = crate::arch::now_cycles();

        // Spin until the PM timer has advanced the target, bailing if it never does.
        let mut aborted = false;
        loop {
            if pm.delta(pm_start, pm.read()) >= target_pm {
                break;
            }
            if crate::arch::now_cycles().wrapping_sub(tsc_start) > CALIB_MAX_TSC {
                aborted = true;
                break;
            }
            core::hint::spin_loop();
        }

        // Sample all three clocks as close together as possible at the window's end.
        let apic_curr = lapic_read(REG_TIMER_CURRCNT, X2_TIMER_CURRCNT);
        let tsc_end = crate::arch::now_cycles();
        let pm_end = pm.read();

        if aborted {
            (0u64, 0u64, 0u64)
        } else {
            (
                tsc_end.wrapping_sub(tsc_start),
                0xFFFF_FFFFu32.wrapping_sub(apic_curr) as u64,
                pm.delta(pm_start, pm_end) as u64,
            )
        }
    });

    // Re-arm the periodic heartbeat we borrowed for the measurement, regardless of outcome.
    init_timer();

    if elapsed_pm == 0 {
        serial_println!("APIC: calibration ABORTED (PM timer did not advance) — timebase stays uncalibrated.");
        return;
    }

    // Scale each count to Hz by the PM timer's fixed frequency. u128 intermediates: at 5 GHz a
    // 100 ms window is ~5e8 cycles, and 5e8 * 3.58e6 ≈ 1.8e15 — fine for u128, tight for u64 only
    // if the window grew, so keep the widening explicit.
    let tsc_hz = ((tsc_delta as u128) * (pm_hz as u128) / (elapsed_pm as u128)) as u64;
    let apic_hz = ((apic_counts as u128) * (pm_hz as u128) / (elapsed_pm as u128)) as u64;

    // Sanity gate: a plausible TSC is 0.2–20 GHz and the ÷16 APIC rate is a few MHz to ~1 GHz.
    // Anything outside that is a bad measurement (SMI storm, wrong port) — discard and stay fixed.
    let sane = (200_000_000..=20_000_000_000).contains(&tsc_hz) && (100_000..=2_000_000_000).contains(&apic_hz);
    if !sane {
        serial_println!(
            "APIC: calibration REJECTED (implausible: TSC {} Hz, APIC ÷16 {} Hz over {} PM ticks) — staying uncalibrated.",
            tsc_hz, apic_hz, elapsed_pm
        );
        return;
    }

    TSC_HZ.store(tsc_hz, Ordering::Relaxed);
    APIC_TIMER_HZ.store(apic_hz, Ordering::Relaxed);
    serial_println!(
        "APIC: calibrated over {} PM ticks ({} ms) — TSC {}.{:03} GHz, APIC timer {}.{:03} MHz (÷16); 1 kHz tick => initcnt {}.",
        elapsed_pm,
        elapsed_pm * 1000 / pm_hz,
        tsc_hz / 1_000_000_000,
        (tsc_hz % 1_000_000_000) / 1_000_000,
        apic_hz / 1_000_000,
        (apic_hz % 1_000_000) / 1_000,
        apic_hz / 1_000
    );
}

/// Calibrated invariant-TSC frequency in Hz, or 0 if calibration has not run / failed.
#[inline]
pub fn tsc_hz() -> u64 {
    TSC_HZ.load(Ordering::Relaxed)
}

/// Calibrated post-÷16 APIC-timer rate in Hz, or 0 if calibration has not run / failed.
#[inline]
pub fn apic_timer_hz() -> u64 {
    APIC_TIMER_HZ.load(Ordering::Relaxed)
}

/// Number of local-APIC timer ticks since boot.
#[inline]
pub fn ticks() -> u64 {
    APIC_TICKS.load(Ordering::Relaxed)
}

/// Signal End-Of-Interrupt to the local APIC. Called from every APIC-delivered handler (the
/// timer and the xHCI MSI-X interrupter) — but NOT from the spurious handler. Lock-free: a
/// single MMIO/MSR write, safe from interrupt context.
#[inline]
pub fn eoi() {
    unsafe { lapic_write(REG_EOI, X2_EOI, 0) };
}

/// Local APIC ID of the current CPU, truncated to 8 bits. Used as the MSI-X destination field
/// (the standard MSI address format only carries an 8-bit id, and we target the BSP, id 0).
pub fn apic_id() -> u8 {
    apic_id_u32() as u8
}

/// Full 32-bit local APIC ID of the current CPU (x2APIC ids can exceed 8 bits). Used for IPI
/// destinations and per-CPU identification.
pub fn apic_id_u32() -> u32 {
    unsafe {
        if x2apic() {
            Msr::new(X2_ID).read() as u32
        } else {
            mmio_read(REG_ID) >> 24
        }
    }
}

/// Send an inter-processor interrupt. `dest` is the destination APIC id; `icr_low` is the ICR
/// low-dword command (delivery mode, level, trigger, vector). Used for AP startup
/// (INIT-SIPI-SIPI) and, later, scheduler/TLB-shootdown IPIs.
///
/// x2APIC issues the whole thing as one 64-bit MSR write (no delivery-status poll exists). xAPIC
/// writes the destination into ICR-high, then the command into ICR-low (which triggers the
/// send), then spins on the delivery-status bit.
pub fn send_ipi(dest: u32, icr_low: u32) {
    unsafe {
        if x2apic() {
            Msr::new(X2_ICR).write(((dest as u64) << 32) | icr_low as u64);
        } else {
            mmio_write(REG_ICR_HIGH, dest << 24);
            mmio_write(REG_ICR_LOW, icr_low);
            while mmio_read(REG_ICR_LOW) & (1 << 12) != 0 {
                core::hint::spin_loop();
            }
        }
    }
}
