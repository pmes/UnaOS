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
    let base_secs = t.to_secs();
    let ticks = monotonic().map(|(p, _)| p).unwrap_or(0);
    *ANCHOR.lock() = Some(Anchor { base_secs, anchor_ticks: ticks });
    // CLOCK-1: an operator seed is also a civil-clock anchor — plant a `Manual` Unix anchor so `time`
    // (and future log/mtime readers) reflect the setdate. Reads the SAME monotonic tick, so the two
    // anchors advance in lock-step. This does NOT touch the FAT anchor above, so `date`/`fat_stamp`
    // witnesses are byte-identical to JD17.
    set_anchor(base_secs.saturating_add(UNIX_1980), ticks, ClockSource::Manual);
    Ok(())
}

/// Whole seconds between the Unix epoch (1970-01-01) and the FAT epoch (1980-01-01): 10 years, two of
/// them leap (1972, 1976). Bridges JD17's FAT-epoch anchor to CLOCK-1's Unix-epoch civil clock.
const UNIX_1980: u64 = 315_532_800;

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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// CLOCK-1 — the shared kernel WALL-CLOCK SERVICE (Unix-epoch, source-tagged).
//
// JD17 above serves the FAT consumer in FAT's own epoch (whole seconds since 1980, operator-seeded).
// CLOCK-1 adds the arch-agnostic *civil* clock the rest of the kernel wants: a single Unix-second
// anchor plus the source that set it, so `time`, log timestamps, and fs mtimes can eventually all
// read one clock. It shares the SAME arch monotonic seam as JD17 (`monotonic()` above: aarch64
// CNTPCT/CNTFRQ, x86_64 the CLOCK-X1 invariant TSC), so it works on both arches with no new plumbing.
//
// This arc lands the SERVICE + the SNTP migration (pi/genet's PI-NET-16 wall-clock forwards here).
// Log-timestamp and fs-mtime adoption are each a named follow-up (see docs/dev/OS/02_KERNEL_CORE):
// this arc does NOT rewire `fat_stamp()`/`now()` onto the Unix anchor — the FAT anchor stays JD17's,
// so no fs-mtime behaviour changes here.

/// Where the current Unix anchor came from. `Unset` is first-class (no anchor yet — the honest state
/// the `time` verb prints as `unsynced`). `Sntp{stratum}` is a network sync (pi/genet PI-NET-16);
/// `Manual` is an operator seed (the `setdate` verb also plants one, so `time` reflects it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClockSource {
    Unset,
    Manual,
    Sntp { stratum: u8 },
}

/// The canonical civil-time anchor: a UTC Unix second paired with the monotonic counter reading taken
/// in the same breath, plus the source that planted it. `unix_now()` = `base_unix` + (monotonic
/// elapsed since `anchor_ticks`). One small lock (anchoring is rare; reads are a couple of loads).
struct UnixAnchor {
    /// UTC seconds since the Unix epoch (1970-01-01T00:00:00Z) at the moment of anchoring.
    base_unix: u64,
    /// The architectural monotonic counter at the moment of anchoring (from `monotonic()`; 0 where
    /// no counter is available, in which case the clock stays honestly frozen at `base_unix`).
    anchor_ticks: u64,
    /// What set this anchor.
    source: ClockSource,
}

static UNIX_ANCHOR: Mutex<Option<UnixAnchor>> = Mutex::new(None);

/// Read the shared arch monotonic counter (ticks only), for callers that want to capture the counter
/// "in the same breath" as a network/operator timestamp and pass it to `set_anchor`. `None` where the
/// arch provides no trustworthy counter (a non-invariant/uncalibrated x86 TSC, or CNTFRQ==0).
pub fn mono_ticks() -> Option<u64> {
    monotonic().map(|(ticks, _)| ticks)
}

/// CLOCK-2 — the lock-free-safe snapshot the serial log-timestamp prefix (`crate::logts`) reads on
/// every line. Returns `(monotonic milliseconds since boot, current UTC Unix seconds if anchored)`.
/// The monotonic part is fully lock-free (one counter read); the civil part uses `try_lock` and
/// yields `None` if the anchor lock is momentarily contended — so this NEVER blocks or panics and is
/// safe from early boot, IRQ-masked handlers, and any core. A `None` civil part simply keeps the
/// prefix in its monotonic form for that one line.
#[cfg(feature = "logts")]
pub fn logts_now() -> (Option<u64>, Option<u64>) {
    let mono_ms = monotonic().map(|(ticks, freq)| ticks.saturating_mul(1000) / freq);
    let unix = match UNIX_ANCHOR.try_lock() {
        Some(guard) => guard.as_ref().map(|a| {
            let elapsed = match monotonic() {
                Some((ticks, freq)) => ticks.wrapping_sub(a.anchor_ticks) / freq,
                None => 0,
            };
            a.base_unix.saturating_add(elapsed)
        }),
        None => None,
    };
    (mono_ms, unix)
}

/// Anchor the civil clock to `unix_secs` (UTC), pairing it with `mono_now` (a `mono_ticks()` reading
/// captured at the same instant) and tagging it with `source`. The ONLY writer of `UNIX_ANCHOR`.
/// Re-anchoring simply replaces the anchor — a fresh sync or operator correction wins.
pub fn set_anchor(unix_secs: u64, mono_now: u64, source: ClockSource) {
    *UNIX_ANCHOR.lock() = Some(UnixAnchor { base_unix: unix_secs, anchor_ticks: mono_now, source });
}

/// Current UTC Unix seconds: the anchored second plus counter-measured whole seconds elapsed since,
/// or `None` while the clock has never been anchored this boot (the honest UNSET state). Monotonic and
/// non-hanging (the arch counter is free-running); frozen at `base_unix` where no counter is available.
pub fn unix_now() -> Option<u64> {
    let guard = UNIX_ANCHOR.lock();
    let a = guard.as_ref()?;
    let elapsed = match monotonic() {
        Some((ticks, freq)) => ticks.wrapping_sub(a.anchor_ticks) / freq,
        None => 0, // no invariant/calibrated counter: frozen at the anchored value
    };
    Some(a.base_unix.saturating_add(elapsed))
}

/// The source of the current anchor, or `ClockSource::Unset` while never anchored.
pub fn source() -> ClockSource {
    (*UNIX_ANCHOR.lock()).as_ref().map(|a| a.source).unwrap_or(ClockSource::Unset)
}

/// Drop the civil (Unix) anchor, returning the clock to the honest UNSET state (`unix_now()` → `None`,
/// `source()` → `Unset`). Symmetric with `set_anchor`; used by the CLOCK-3 FAT-stamp witness to leave
/// the civil clock EXACTLY as it found it after a deterministic self-test anchor. Does NOT touch the
/// JD17 FAT anchor.
pub fn clear_anchor() {
    *UNIX_ANCHOR.lock() = None;
}

/// The raw, NON-extrapolated anchor pair `(base_unix, source)` — deterministic, does not race the
/// free-running counter. Used by the pi/genet NET16-GATE to assert an exact rendered ISO string.
pub fn raw_anchor() -> Option<(u64, ClockSource)> {
    (*UNIX_ANCHOR.lock()).as_ref().map(|a| (a.base_unix, a.source))
}

/// Civil date/time from UTC Unix seconds, no floats. Howard Hinnant's days→civil algorithm, exact for
/// the whole proleptic-Gregorian range. Returns `(year, month[1..12], day[1..31], h, m, s)`. Moved here
/// (CLOCK-1) from pi/genet PI-NET-16 so `time` and the SNTP client render civil time through one path.
pub fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let (hh, mm, ss) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    // Shift the epoch so era math has no negative-days special case for our band (days >= 0 here).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month, d, hh, mm, ss)
}

/// A fixed-width byte writer for the ISO renderer (no allocation, no heap).
struct IsoWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}
impl core::fmt::Write for IsoWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let end = core::cmp::min(self.buf.len(), self.len + b.len());
        let n = end - self.len;
        self.buf[self.len..end].copy_from_slice(&b[..n]);
        self.len = end;
        Ok(())
    }
}

/// Render `unix_secs` (UTC) as ISO-8601 `YYYY-MM-DDTHH:MM:SSZ` into `out`, returning its byte length
/// (20 for a 4-digit year). Fixed-width, no allocation. Moved here (CLOCK-1) from pi/genet PI-NET-16.
pub fn render_iso8601(unix_secs: u64, out: &mut [u8]) -> usize {
    use core::fmt::Write as _;
    let (y, mo, d, h, mi, s) = civil_from_unix(unix_secs);
    let mut w = IsoWriter { buf: out, len: 0 };
    let _ = write!(w, "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s);
    w.len
}

/// Render "now" as ISO-8601 UTC into `out`, returning `Some(len)`, or `None` while unset (the honest
/// state the `time` verb prints as `unsynced`). The one call the shell's `time` verb makes.
pub fn iso8601_now(out: &mut [u8]) -> Option<usize> {
    unix_now().map(|secs| render_iso8601(secs, out))
}

/// Pack a `WallTime` into the two FAT on-disk words `(time @0x16, date @0x18)` — the exact layout
/// §JD16's `DirEntry::mtime()` decodes (DATE: year-1980 in bits 15..9, month in 8..5, day in 4..0;
/// TIME: hour in 15..11, min in 10..5, seconds÷2 in 4..0 — FAT's 2-second granularity, the low
/// second bit is unrepresentable). Assumes the year is already within FAT's 1980..=2107 span
/// (callers clamp); the field widths silently wrap outside it, which is why the derivation clamps first.
fn fat_pack(t: &WallTime) -> (u16, u16) {
    let date = (((t.year - 1980) as u16) << 9) | ((t.month as u16) << 5) | t.day as u16;
    let time = ((t.hour as u16) << 11) | ((t.min as u16) << 5) | (t.sec as u16 / 2);
    (time, date)
}

/// CLOCK-3: convert UTC Unix seconds to a FAT-representable `WallTime`, CLAMPING to FAT's on-disk span
/// `[1980-01-01 00:00:00 .. 2107-12-31 23:59:58]` rather than wrapping or panicking — FAT simply cannot
/// represent a moment outside that band. Witnessable rule: a pre-1980 anchor pins to the epoch FLOOR, a
/// post-2107 anchor to the last representable 2-second tick (`23:59:58`). FAT stores wall-clock time with
/// no timezone; we stamp UTC (`civil_from_unix` is UTC), documented in clock.md.
fn wall_from_unix_clamped(unix: u64) -> WallTime {
    if unix < UNIX_1980 {
        return WallTime { year: 1980, month: 1, day: 1, hour: 0, min: 0, sec: 0 };
    }
    let (y, mo, d, h, mi, s) = civil_from_unix(unix);
    if y > 2107 {
        return WallTime { year: 2107, month: 12, day: 31, hour: 23, min: 59, sec: 58 };
    }
    // `unix >= UNIX_1980` ⇒ `y >= 1980`, and the `y > 2107` guard above bounds the top, so the cast is safe.
    WallTime { year: y as u32, month: mo, day: d, hour: h, min: mi, sec: s }
}

/// The two packed FAT on-disk words `(time @0x16, date @0x18)` for "now".
///
/// CLOCK-3 — the UNIFIED clock is the single source of truth. When a civil (Unix) anchor exists — an
/// SNTP sync (pi/genet PI-NET-16) or a `setdate` Manual seed — the FAT stamp is DERIVED from it
/// (`unix_now()` → `civil_from_unix` → FAT packing, clamped to the FAT span), so a networked board
/// stamps REAL last-write times with zero operator action. It falls back to the legacy JD17 FAT anchor
/// (`now()`) only when no Unix anchor is set, and to `(0, 0)` when neither is — byte-identical to the
/// pre-JD17 zeroed field, which the `ls -l` renderer already shows as the dashed placeholder. (Because
/// `setdate` plants BOTH anchors in the same breath from the same monotonic tick, the derivation
/// round-trips: `setdate D` → `fat_stamp()` yields `D`.)
pub fn fat_stamp() -> (u16, u16) {
    if let Some(unix) = unix_now() {
        return fat_pack(&wall_from_unix_clamped(unix));
    }
    match now() {
        Some(t) => fat_pack(&t),
        None => (0, 0),
    }
}
