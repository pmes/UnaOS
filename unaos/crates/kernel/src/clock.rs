//! JD17 — the kernel WALL CLOCK: an operator-seeded, monotonically extended time-of-day service.
//!
//! The boards have NO RTC the kernel reads (§JD16 documented the consequence: every kernel-written
//! FAT entry carried an all-zero mtime). JD17 closes that gap honestly: the operator seeds the
//! clock once per boot with the `setdate` shell verb, and the free-running architectural counter
//! (aarch64 `CNTPCT_EL0` / `CNTFRQ_EL0` — the same JD3 timerless mechanism the BOT pump and the
//! JD4 screen-on-boot deadline ride) extends it forward from the moment of setting. UNSET is a
//! first-class state: `now()` is `None` and `fat_stamp()` is `(0, 0)` — exactly the all-zero
//! on-disk value §JD16's `ls -l` already renders as the dashed placeholder. The kernel never
//! fabricates a reading.
//!
//! CLOCK-X1 lights the x86_64 twin: the invariant TSC (`rdtsc`), calibrated once at boot against
//! the ACPI PM timer (`apic::calibrate` → `apic::tsc_hz`), is the free-running counter here, gated
//! on CPUID's invariant-TSC bit (`apic::tsc_invariant`, leaf 0x8000_0007 EDX[8]). Where the CPU
//! does NOT advertise an invariant TSC, or calibration never ran / was rejected, `monotonic()`
//! returns `None` and a set clock stays honestly FROZEN at its seeded value (the pre-CLOCK-X1
//! behaviour) — the kernel never serves a non-invariant or uncalibrated TSC as a clock.
//!
//! Range and resolution follow FAT's on-disk format (the consumer this arc serves): years
//! 1980..=2107, 2-second mtime resolution (the packing truncates the low second bit). Internally
//! the clock is whole seconds since 1980-01-01 00:00:00 (no timezone — FAT stores local wall
//! time with no offset, §JD16).

use spin::Mutex;

/// The anchor a `setdate` plants: the seeded wall-clock second paired with the counter reading at
/// the moment of seeding. `now()` = `base_secs` + (ticks since `anchor_ticks`) / freq. One small
/// lock (set is a rare operator action; reads are a couple of loads) keeps the pair consistent.
struct Anchor {
    /// Seconds since 1980-01-01 00:00:00 at the moment of `set`.
    base_secs: u64,
    /// The architectural counter at the moment of `set` (0 where no counter is available — a
    /// non-invariant or uncalibrated x86 TSC, where `monotonic()` is `None`).
    anchor_ticks: u64,
}

static ANCHOR: Mutex<Option<Anchor>> = Mutex::new(None);

/// The free-running monotonic counter and its frequency, where the architecture provides one.
/// aarch64: `CNTPCT_EL0`/`CNTFRQ_EL0` (EL-independent, never stops — the JD3 mechanism).
/// x86_64: the invariant TSC (`rdtsc`) at its boot-calibrated frequency, gated on the CPUID
/// invariant-TSC bit and a successful calibration; `None` otherwise (honest frozen clock).
#[cfg(target_arch = "aarch64")]
fn monotonic() -> Option<(u64, u64)> {
    let f = crate::arch::timer::cntfrq();
    if f == 0 {
        return None; // defensive: a zero CNTFRQ would make the division meaningless
    }
    Some((crate::arch::timer::cntpct(), f))
}

/// CLOCK-X1 — the x86 twin. `rdtsc` is only served as a clock when the CPU advertises an INVARIANT
/// TSC (constant rate across P-/C-/T-states, never stops — CPUID leaf 0x8000_0007 EDX[8]) AND the
/// boot calibration against the ACPI PM timer produced a frequency (`apic::tsc_hz() != 0`). Either
/// missing → `None`, i.e. the clock stays frozen at its seed rather than run on an untrustworthy or
/// unscaled counter. Cheap and lock-free (one CPUID + one atomic load + one `rdtsc`), so it is safe
/// on the shell-verb and FAT-stamp hot paths.
#[cfg(target_arch = "x86_64")]
fn monotonic() -> Option<(u64, u64)> {
    if !crate::arch::apic::tsc_invariant() {
        return None; // non-invariant TSC: never serve it as a clock
    }
    let hz = crate::arch::apic::tsc_hz();
    if hz == 0 {
        return None; // calibration never ran or was rejected — stay honestly frozen
    }
    Some((crate::arch::now_cycles(), hz))
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn monotonic() -> Option<(u64, u64)> {
    None
}

fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// A broken-down wall-clock moment. Field ranges mirror the FAT on-disk format's representable
/// span (year 1980..=2107); `sec` here is full-resolution 0..=59 — the 2-second truncation
/// happens only at `fat_stamp` packing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WallTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
}

impl WallTime {
    /// Validate ranges against the calendar and the FAT-representable span.
    pub fn is_valid(&self) -> bool {
        (1980..=2107).contains(&self.year)
            && (1..=12).contains(&self.month)
            && self.day >= 1
            && self.day <= days_in_month(self.year, self.month)
            && self.hour <= 23
            && self.min <= 59
            && self.sec <= 59
    }

    /// Whole seconds since 1980-01-01 00:00:00. The loops are bounded by the 128-year span.
    fn to_secs(&self) -> u64 {
        let mut days: u64 = 0;
        for y in 1980..self.year {
            days += if is_leap(y) { 366 } else { 365 };
        }
        for m in 1..self.month {
            days += days_in_month(self.year, m) as u64;
        }
        days += (self.day - 1) as u64;
        ((days * 24 + self.hour as u64) * 60 + self.min as u64) * 60 + self.sec as u64
    }

    /// Inverse of `to_secs`, saturating at the end of 2107 (the last FAT-representable moment) so
    /// a long-running clock degrades to a pinned honest maximum rather than wrapping or panicking.
    fn from_secs(mut secs: u64) -> WallTime {
        const MAX: u64 = (2108 - 1980) as u64 * 366 * 86400; // loose upper bound, then pin below
        if secs >= MAX {
            secs = MAX;
        }
        let mut days = secs / 86400;
        let rem = secs % 86400;
        let mut year: u32 = 1980;
        loop {
            let ylen: u64 = if is_leap(year) { 366 } else { 365 };
            if days < ylen || year == 2107 {
                break;
            }
            days -= ylen;
            year += 1;
        }
        let mut month: u32 = 1;
        loop {
            let mlen = days_in_month(year, month) as u64;
            if days < mlen || month == 12 {
                break;
            }
            days -= mlen;
            month += 1;
        }
        // Pin any residual overflow (the year-2107 saturation path) to the last valid day/second.
        let day = core::cmp::min(days as u32 + 1, days_in_month(year, month));
        let hour = core::cmp::min((rem / 3600) as u32, 23);
        let min = ((rem % 3600) / 60) as u32;
        let sec = (rem % 60) as u32;
        WallTime { year, month, day, hour, min, sec }
    }
}

/// Seed the clock. Returns `Err(())` for an out-of-range moment (caller shows the honest usage
/// message). Re-seeding simply replaces the anchor — the operator's correction wins.
pub fn set(t: WallTime) -> Result<(), ()> {
    if !t.is_valid() {
        return Err(());
    }
    let ticks = monotonic().map(|(p, _)| p).unwrap_or(0);
    *ANCHOR.lock() = Some(Anchor { base_secs: t.to_secs(), anchor_ticks: ticks });
    Ok(())
}

/// JD18: whole seconds since boot from the free-running architectural counter, INDEPENDENT of
/// whether the wall clock has been seeded. aarch64: `CNTPCT_EL0 / CNTFRQ_EL0` (the counter resets to
/// 0 at boot and never stops — the same JD3 mechanism `now()` extends from). x86_64 (CLOCK-X1): the
/// boot-calibrated invariant TSC / `tsc_hz` → `Some` once calibrated, else `None` and the `uptime`
/// verb prints an honest "no calibrated counter on this arch". Purely additive: it reads the same `monotonic()` source but
/// touches neither the seed anchor nor the `now()`/`fat_stamp()` logic.
pub fn uptime_secs() -> Option<u64> {
    monotonic().map(|(ticks, freq)| ticks / freq)
}

/// The current wall-clock moment: the seeded second plus counter-elapsed whole seconds, or `None`
/// while the clock has never been set this boot (the honest UNSET state).
pub fn now() -> Option<WallTime> {
    let guard = ANCHOR.lock();
    let a = guard.as_ref()?;
    let elapsed = match monotonic() {
        Some((p, f)) => p.wrapping_sub(a.anchor_ticks) / f,
        None => 0, // no invariant/calibrated counter: frozen at the seeded value, documented above
    };
    Some(WallTime::from_secs(a.base_secs.saturating_add(elapsed)))
}

/// The two packed FAT on-disk words `(time @0x16, date @0x18)` for "now" — the exact layout
/// §JD16's `DirEntry::mtime()` decodes (DATE: year-1980/month/day; TIME: hour/min/sec÷2).
/// `(0, 0)` while the clock is unset: byte-identical to the pre-JD17 zeroed field, which the
/// `ls -l` renderer already shows as the dashed placeholder.
pub fn fat_stamp() -> (u16, u16) {
    match now() {
        Some(t) => {
            let date = (((t.year - 1980) as u16) << 9) | ((t.month as u16) << 5) | t.day as u16;
            let time = ((t.hour as u16) << 11) | ((t.min as u16) << 5) | (t.sec as u16 / 2);
            (time, date)
        }
        None => (0, 0),
    }
}
