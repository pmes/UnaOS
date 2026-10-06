//! CHARTER: Kernel — kernel-by-ruling (B397 CLOCKCORE: the seat's sched ruling on the global clock)
//!
//! CLOCKCORE — the machine's millisecond clock no longer IS cpu 0's timer interrupt.
//!
//! Before: `arch::ms()` read `APIC_TICKS`, advanced only by the BSP's LAPIC-timer ISR, so any masked
//! span on cpu 0 (every UnaFS transaction runs under `without_interrupts`) froze time for every core —
//! and with it every timeout, guard and stall detector (flights 24/25: the FLAC wedge went SILENT).
//! Now: once `apic::calibrate` has measured the TSC and CPUID says it is INVARIANT, `ms()` is the TSC
//! scaled by that rate, continuous with the tick count at the switch, monotonic across cores (one
//! `fetch_max`), with a per-core offset measured at AP bring-up and applied only if an AP is out of
//! sync. The per-core LAPIC tick stays — for scheduling — but it no longer is the clock.
//!
//! Masked sections are NAMED: the outermost `IrqMask`/`without_interrupts` records (cpu, t0, site);
//! a span over 10 ms prints `[irq] masked section fn=<name> ms=<n> cpu=<c> live=0` once per site, and
//! any OTHER core's tick names a section still masked past 10 ms (`live=1`) — the wedge seen from
//! outside the wedged core. `tests clock` is the witness (registered, never run at boot — R80).

use core::panic::Location;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU32, AtomicU64, Ordering::*};

use crate::arch::gdt::MAX_CPUS;

/// The TSC clock is live (`start` ran with a calibrated, invariant TSC).
static TSC_CLOCK: AtomicBool = AtomicBool::new(false);
/// TSC at the switch, and the ms value at that instant (the tick count it continues from).
static BASE_TSC: AtomicU64 = AtomicU64::new(0);
static BASE_MS: AtomicU64 = AtomicU64::new(0);
/// ms per cycle as a 64.64 fixed-point multiplier: `(1000 << 64) / hz`.
static MUL: AtomicU64 = AtomicU64::new(0);
/// Largest ms value handed out — the cross-core monotonic floor.
static LAST_MS: AtomicU64 = AtomicU64::new(0);
/// Per-core TSC offset vs the BSP (cycles; this core's TSC minus the BSP's). 0 = in sync.
static TSC_OFF: [AtomicI64; MAX_CPUS] = [const { AtomicI64::new(0) }; MAX_CPUS];
/// Set after SMP bring-up only when some AP measured out of sync: `ms()` then subtracts its offset.
static OFFS_APPLY: AtomicBool = AtomicBool::new(false);
/// Largest |offset| measured at bring-up (cycles), and APs whose sync never saw the BSP publish.
static SKEW_MAX: AtomicU64 = AtomicU64::new(0);
static SYNC_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
/// The BSP's published TSC while it waits for an AP (`bsp_publish`).
static BSP_PUB: AtomicU64 = AtomicU64::new(0);

/// Masked-section watch armed (after SMP: every core's GS-based per-CPU block exists).
static ARMED: AtomicBool = AtomicBool::new(false);
/// 10 ms in TSC cycles (the naming threshold).
static THRESH: AtomicU64 = AtomicU64::new(u64::MAX);
/// Per-core: TSC (BSP-corrected) at the outermost mask, 0 = not masked; and the site.
static MASK_T0: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static MASK_SITE: [AtomicPtr<Location<'static>>; MAX_CPUS] = [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_CPUS];
/// Sites already named (once per site), and the largest span seen (cycles).
const SEEN_CAP: usize = 48;
static SEEN: [AtomicPtr<Location<'static>>; SEEN_CAP] = [const { AtomicPtr::new(core::ptr::null_mut()) }; SEEN_CAP];
static SEEN_N: AtomicU32 = AtomicU32::new(0);
static MASKED_MAX: AtomicU64 = AtomicU64::new(0);
/// Next ms at which a ticking core scans the others for a live masked section.
static NEXT_SCAN: AtomicU64 = AtomicU64::new(0);

#[inline]
fn rdtsc() -> u64 {
    crate::arch::now_cycles()
}

/// This core's TSC on the BSP's timeline. Uncorrected unless an AP measured out of sync (and then
/// only after every core's per-CPU block exists — `OFFS_APPLY` is set after SMP bring-up).
#[inline]
fn tsc_global() -> u64 {
    let t = rdtsc();
    if OFFS_APPLY.load(Relaxed) {
        let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
        if cpu < MAX_CPUS {
            return t.wrapping_sub(TSC_OFF[cpu].load(Relaxed) as u64);
        }
    }
    t
}

/// M1 — switch the global clock to the TSC. Called once by `apic::calibrate` (BSP, before the APs)
/// after it stored `hz`. A non-invariant TSC keeps the tick clock (it would drift with P-states).
pub fn start(hz: u64) {
    if hz == 0 || !crate::arch::apic::tsc_invariant() {
        return;
    }
    let mul = ((1000u128 << 64) / hz as u128) as u64;
    let now_ms = crate::arch::apic::APIC_TICKS.load(Relaxed);
    BASE_MS.store(now_ms, Relaxed);
    LAST_MS.store(now_ms, Relaxed);
    BASE_TSC.store(rdtsc(), Relaxed);
    MUL.store(mul, Relaxed);
    THRESH.store(hz / 100, Relaxed);
    TSC_CLOCK.store(true, Release);
}

/// Whether the global clock is the TSC.
#[inline]
pub fn tsc_clock() -> bool {
    TSC_CLOCK.load(Relaxed)
}

/// Milliseconds since boot: the TSC clock when live, else the BSP's tick count.
#[inline]
pub fn ms() -> u64 {
    if !TSC_CLOCK.load(Acquire) {
        return crate::arch::apic::APIC_TICKS.load(Relaxed);
    }
    let d = tsc_global().saturating_sub(BASE_TSC.load(Relaxed));
    let v = BASE_MS.load(Relaxed) + ((d as u128 * MUL.load(Relaxed) as u128) >> 64) as u64;
    let last = LAST_MS.load(Relaxed);
    if v > last {
        LAST_MS.fetch_max(v, Relaxed);
        v
    } else {
        last
    }
}

/// BSP, inside `smp::start_aps`'s wait-for-AP spin: publish the TSC for the AP's offset sample.
#[inline]
pub fn bsp_publish() {
    BSP_PUB.store(rdtsc(), Release);
}

/// AP `idx`, in `ap_entry` before the `AP_ONLINE` handshake (no GS yet, no interrupts): measure this
/// core's TSC against the BSP's published one. `own - published` over-reads the true offset by the
/// publish→read latency, so the minimum of 64 fresh samples is the bound; within 10 us is in sync.
pub fn ap_sync(idx: usize) {
    if idx >= MAX_CPUS || !TSC_CLOCK.load(Acquire) {
        return;
    }
    let hz = crate::arch::apic::tsc_hz().max(1);
    let give_up = rdtsc().wrapping_add(hz / 20); // 50 ms of this core's own TSC
    let mut prev = BSP_PUB.load(Acquire);
    let mut best = i64::MAX;
    let mut n = 0u32;
    while n < 64 && rdtsc() < give_up {
        let p = BSP_PUB.load(Acquire);
        let t = rdtsc();
        if p != prev && p != 0 {
            prev = p;
            best = best.min(t.wrapping_sub(p) as i64);
            n += 1;
        }
        core::hint::spin_loop();
    }
    if n == 0 {
        SYNC_TIMEOUTS.fetch_add(1, Relaxed);
        return;
    }
    let eps = (hz / 100_000) as i64; // 10 us
    let off = if best.unsigned_abs() as i64 <= eps { 0 } else { best };
    TSC_OFF[idx].store(off, Relaxed);
    SKEW_MAX.fetch_max(best.unsigned_abs(), Relaxed);
}

/// M2 — the outermost mask on this core begins (`IrqMask::new`, interrupts already off). Returns the
/// (t0, cpu) token `mask_exit` takes; t0 == 0 = not tracked (before SMP / no TSC clock).
#[inline]
pub fn mask_enter(site: &'static Location<'static>) -> (u64, u32) {
    if !ARMED.load(Relaxed) {
        return (0, 0);
    }
    let cpu = crate::arch::percpu::this_cpu().cpu_index;
    let t0 = tsc_global().max(1);
    if (cpu as usize) < MAX_CPUS {
        MASK_SITE[cpu as usize].store(site as *const _ as *mut _, Relaxed);
        MASK_T0[cpu as usize].store(t0, Release);
    }
    (t0, cpu)
}

/// The outermost mask ends (still masked; the caller re-enables right after). Names a span over
/// 10 ms once per site.
#[inline]
pub fn mask_exit(tok: (u64, u32), site: &'static Location<'static>) {
    let (t0, cpu) = tok;
    if t0 == 0 {
        return;
    }
    if (cpu as usize) < MAX_CPUS {
        MASK_T0[cpu as usize].store(0, Release);
    }
    let span = tsc_global().saturating_sub(t0);
    if span > THRESH.load(Relaxed) {
        MASKED_MAX.fetch_max(span, Relaxed);
        name_once(site, span, cpu, false);
    }
}

/// `fn=` for a site: `<file>:<line>`, prefixed `with_unafs@` for the UnaFS transaction's file.
fn site_name(site: &Location<'static>) -> (&'static str, &'static str, u32) {
    let f: &'static str = site.file();
    let short = f.rsplit('/').next().unwrap_or(f);
    let pre = if f.ends_with("fs/unafs.rs") { "with_unafs@" } else { "" };
    (pre, short, site.line())
}

fn name_once(site: &'static Location<'static>, span: u64, cpu: u32, live: bool) {
    let p = site as *const _ as *mut Location<'static>;
    for slot in SEEN.iter() {
        let cur = slot.load(Acquire);
        if cur == p {
            return;
        }
        if cur.is_null() {
            if slot.compare_exchange(core::ptr::null_mut(), p, AcqRel, Acquire).is_ok() {
                SEEN_N.fetch_add(1, Relaxed);
                let hz = crate::arch::apic::tsc_hz().max(1);
                let (pre, file, line) = site_name(site);
                // `serial_println!` takes the sink with try_lock only (deadman's contract): safe from
                // an IRQ context and under any caller's lock.
                serial_println!("[irq] masked section fn={}{}:{} ms={} cpu={} live={}", pre, file, line, span * 1000 / hz, cpu, live as u8);
                return;
            }
            if slot.load(Acquire) == p {
                return;
            }
        }
    }
}

/// Timer ISR, every core (after `note_tick`): every 50 ms one ticking core scans the OTHERS for a
/// section still masked past 10 ms and names it `live=1` — the core that is stuck cannot say so.
#[inline]
pub fn isr_tick() {
    if !ARMED.load(Relaxed) {
        return;
    }
    let now = ms();
    let due = NEXT_SCAN.load(Relaxed);
    if now < due || NEXT_SCAN.compare_exchange(due, now + 50, Relaxed, Relaxed).is_err() {
        return;
    }
    let here = crate::arch::percpu::this_cpu().cpu_index as usize;
    let t = tsc_global();
    let th = THRESH.load(Relaxed);
    for c in 0..MAX_CPUS {
        if c == here {
            continue;
        }
        let t0 = MASK_T0[c].load(Acquire);
        if t0 == 0 || t.saturating_sub(t0) <= th {
            continue;
        }
        let site = MASK_SITE[c].load(Relaxed);
        if site.is_null() || MASK_T0[c].load(Acquire) != t0 {
            continue;
        }
        // SAFETY: every stored site is a `&'static Location` (from `#[track_caller]`).
        let site: &'static Location<'static> = unsafe { &*site };
        MASKED_MAX.fetch_max(t.saturating_sub(t0), Relaxed);
        name_once(site, t.saturating_sub(t0), c as u32, true);
    }
}

/// M3 — the clock duties (deadman's once-a-second line) run on ANY ticking core once the clock is the
/// TSC; on the tick clock only the BSP's reading is the wall clock, so only it may.
#[inline]
pub fn duty_core() -> bool {
    TSC_CLOCK.load(Relaxed) || crate::arch::percpu::this_cpu().cpu_index == 0
}

/// Online cores (BSP + the APs that reported in).
fn online() -> alloc::vec::Vec<usize> {
    let mut v = alloc::vec![0usize];
    for c in crate::arch::smp::online_aps() {
        if c != 0 && !v.contains(&c) {
            v.push(c);
        }
    }
    v
}

/// M4 — after SMP bring-up (`apic::report_tick_rate`'s tail): apply offsets if any AP is out of
/// sync, arm the masked-section watch, print the boot witness.
pub fn boot_witness() {
    let offs = TSC_OFF.iter().any(|o| o.load(Relaxed) != 0);
    if offs {
        OFFS_APPLY.store(true, Release);
    }
    if TSC_CLOCK.load(Acquire) {
        ARMED.store(true, Release);
    }
    let cores = online().len();
    serial_println!(
        "[clock] source={} hz={} invariant={} per_core_tick=1 cores={} skew_max_cycles={} sync_timeouts={} offsets={} (CLOCKCORE)",
        if tsc_clock() { "tsc" } else { "apic" },
        crate::arch::apic::tsc_hz(),
        crate::arch::apic::tsc_invariant() as u8,
        cores,
        SKEW_MAX.load(Relaxed),
        SYNC_TIMEOUTS.load(Relaxed),
        if offs { "applied" } else { "none" }
    );
    ensure_tests();
}

/// `tests clock` registration, once (never run at boot, R80).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, AcqRel) {
        crate::tests::register("clock", selftest);
    }
}

/// `tests clock`: (1) the TSC ms clock against the ACPI PM timer over 250 ms (no HPET driver in this
/// tree — `ref=pm`); (2) a deliberate 20 ms masked spin on THIS core must still see `ms()` advance
/// 20 ms, and the watch must have named it; (3) every online core's LAPIC tick advanced in the window.
pub fn selftest() {
    const WIN_MS: u64 = 250;
    let hz = crate::arch::apic::tsc_hz();
    let cores = online();
    let snap: alloc::vec::Vec<u64> = cores
        .iter()
        .map(|&c| crate::arch::percpu::cpu(c).map(|p| p.ticks.load(Relaxed)).unwrap_or(0))
        .collect();
    let mut drift: i64 = -1;
    if let (Some(pm), true) = (crate::arch::acpi::pm_timer(crate::arch::acpi::rsdp_addr()), hz != 0) {
        let pm_hz = crate::arch::acpi::PM_TIMER_HZ;
        let target = (pm_hz * WIN_MS / 1000) as u32;
        let m0 = ms();
        let p0 = pm.read();
        let give_up = rdtsc().wrapping_add(hz * 2);
        let mut e = 0u32;
        while rdtsc() < give_up {
            e = pm.delta(p0, pm.read());
            if e >= target {
                break;
            }
            core::hint::spin_loop();
        }
        let m1 = ms();
        let pm_ms = e as u64 * 1000 / pm_hz;
        drift = ((m1 - m0) as i64 - pm_ms as i64).abs();
    }
    // (2) the masked spin — the property the arc exists for.
    let mut masked_ms = 0u64;
    if hz != 0 {
        masked_ms = crate::arch::without_interrupts(|| {
            let a = ms();
            let end = rdtsc().wrapping_add(hz / 50); // 20 ms
            while rdtsc() < end {
                core::hint::spin_loop();
            }
            ms() - a
        });
    }
    let ticking = cores
        .iter()
        .zip(snap.iter())
        .filter(|&(&c, &s)| crate::arch::percpu::cpu(c).map(|p| p.ticks.load(Relaxed) > s).unwrap_or(false))
        .count();
    let mmax = MASKED_MAX.load(Relaxed) * 1000 / hz.max(1);
    let pass = tsc_clock() && (0..=2).contains(&drift) && masked_ms >= 19 && mmax >= 19 && ticking == cores.len();
    serial_println!(
        ":: CLOCKCORE: source={} tsc_ms_vs_hpet_ms={} ref=pm window_ms={} masked_clock_ms={}/20 masked_max_ms={} masked_sites={} cores_ticking={}/{} skew_max_cycles={} -> {} ::",
        if tsc_clock() { "tsc" } else { "apic" },
        drift,
        WIN_MS,
        masked_ms,
        mmax,
        SEEN_N.load(Relaxed),
        ticking,
        cores.len(),
        SKEW_MAX.load(Relaxed),
        if pass { "PASS" } else { "FAIL" }
    );
    crate::tests::tally(pass);
}
