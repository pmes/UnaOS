//! The ISO 8601 calendar and the exact arithmetic under every Temporal type (proposal-temporal 2025, §12–§13):
//! dates as (year, month, day) with epoch-day conversion (proleptic Gregorian, Hinnant's civil algorithms),
//! wall-clock times as nanoseconds since midnight, durations as an internal record (date units as integers,
//! the time part as one i128 nanosecond count — the spec's "time duration"), and the nine rounding modes of
//! ECMA-402 §15.5.12 applied to i128 quantities. Nothing here touches the VM.

use alloc::string::String;
use alloc::format;

pub const NS_PER_DAY: i128 = 86_400_000_000_000;
/// nsMaxInstant (§8.1): 10^8 days in nanoseconds.
pub const NS_MAX_INSTANT: i128 = 8_640_000_000_000_000_000_000;
/// maxTimeDuration (§7.5.1): 2^53 × 10^9 − 1 nanoseconds.
pub const MAX_TIME_DURATION: i128 = 9_007_199_254_740_992 * 1_000_000_000 - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IsoDate {
    pub year: i64,
    pub month: u8,
    pub day: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Time {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub ms: u16,
    pub us: u16,
    pub ns: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unit {
    Year,
    Month,
    Week,
    Day,
    Hour,
    Minute,
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
}

pub const UNITS: [Unit; 10] =
    [Unit::Year, Unit::Month, Unit::Week, Unit::Day, Unit::Hour, Unit::Minute, Unit::Second, Unit::Millisecond, Unit::Microsecond, Unit::Nanosecond];

impl Unit {
    pub fn singular(self) -> &'static str {
        match self {
            Unit::Year => "year",
            Unit::Month => "month",
            Unit::Week => "week",
            Unit::Day => "day",
            Unit::Hour => "hour",
            Unit::Minute => "minute",
            Unit::Second => "second",
            Unit::Millisecond => "millisecond",
            Unit::Microsecond => "microsecond",
            Unit::Nanosecond => "nanosecond",
        }
    }
    /// Table 21: a unit name, singular or plural.
    pub fn parse(s: &str) -> Option<Unit> {
        let t = s.strip_suffix('s').unwrap_or(s);
        UNITS.iter().copied().find(|u| u.singular() == t)
    }
    pub fn is_calendar(self) -> bool {
        matches!(self, Unit::Year | Unit::Month | Unit::Week)
    }
    pub fn is_date(self) -> bool {
        self <= Unit::Day
    }
    /// The length of a time unit (or a 24-hour day) in nanoseconds.
    pub fn ns(self) -> i128 {
        match self {
            Unit::Day => NS_PER_DAY,
            Unit::Hour => 3_600_000_000_000,
            Unit::Minute => 60_000_000_000,
            Unit::Second => 1_000_000_000,
            Unit::Millisecond => 1_000_000,
            Unit::Microsecond => 1_000,
            Unit::Nanosecond => 1,
            _ => 0,
        }
    }
    pub fn index(self) -> usize {
        self as usize
    }
}

/// LargerOfTwoTemporalUnits.
pub fn larger(a: Unit, b: Unit) -> Unit {
    if a <= b { a } else { b }
}

/// MaximumTemporalDurationRoundingIncrement (§13.21).
pub fn max_increment(u: Unit) -> Option<i128> {
    match u {
        Unit::Year | Unit::Month | Unit::Week | Unit::Day => None,
        Unit::Hour => Some(24),
        Unit::Minute | Unit::Second => Some(60),
        _ => Some(1000),
    }
}

// ============================================================================================ rounding

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Ceil,
    Floor,
    Expand,
    Trunc,
    HalfCeil,
    HalfFloor,
    HalfExpand,
    HalfTrunc,
    HalfEven,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s {
            "ceil" => Mode::Ceil,
            "floor" => Mode::Floor,
            "expand" => Mode::Expand,
            "trunc" => Mode::Trunc,
            "halfCeil" => Mode::HalfCeil,
            "halfFloor" => Mode::HalfFloor,
            "halfExpand" => Mode::HalfExpand,
            "halfTrunc" => Mode::HalfTrunc,
            "halfEven" => Mode::HalfEven,
            _ => return None,
        })
    }
    /// NegateRoundingMode (§13.6).
    pub fn negate(self) -> Mode {
        match self {
            Mode::Ceil => Mode::Floor,
            Mode::Floor => Mode::Ceil,
            Mode::HalfCeil => Mode::HalfFloor,
            Mode::HalfFloor => Mode::HalfCeil,
            m => m,
        }
    }
}

pub const MODE_NAMES: &[&str] = &["ceil", "floor", "expand", "trunc", "halfCeil", "halfFloor", "halfExpand", "halfTrunc", "halfEven"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UMode {
    Zero,
    Infinity,
    HalfZero,
    HalfInfinity,
    HalfEven,
}

/// GetUnsignedRoundingMode (ECMA-402 §15.5.11).
pub fn unsigned_mode(m: Mode, negative: bool) -> UMode {
    match (m, negative) {
        (Mode::Ceil, false) | (Mode::Floor, true) | (Mode::Expand, _) => UMode::Infinity,
        (Mode::Ceil, true) | (Mode::Floor, false) | (Mode::Trunc, _) => UMode::Zero,
        (Mode::HalfCeil, false) | (Mode::HalfFloor, true) | (Mode::HalfExpand, _) => UMode::HalfInfinity,
        (Mode::HalfCeil, true) | (Mode::HalfFloor, false) | (Mode::HalfTrunc, _) => UMode::HalfZero,
        (Mode::HalfEven, _) => UMode::HalfEven,
    }
}

/// ApplyUnsignedRoundingMode for x = r1 + num/den with 0 ≤ num < den: whether the result is r1 + 1 (r2).
pub fn round_up(um: UMode, r1: i128, num: i128, den: i128) -> bool {
    if num == 0 {
        return false;
    }
    match um {
        UMode::Zero => false,
        UMode::Infinity => true,
        _ => {
            let twice = num * 2;
            if twice < den {
                false
            } else if twice > den {
                true
            } else {
                match um {
                    UMode::HalfZero => false,
                    UMode::HalfInfinity => true,
                    _ => r1 % 2 != 0,
                }
            }
        }
    }
}

/// RoundNumberToIncrement (§13.25) on an exact integer quantity.
pub fn round_incr(x: i128, inc: i128, mode: Mode) -> i128 {
    let neg = x < 0;
    let a = x.abs();
    let r1 = a / inc;
    let rem = a % inc;
    let r = if round_up(unsigned_mode(mode, neg), r1, rem, inc) { r1 + 1 } else { r1 };
    (if neg { -r } else { r }) * inc
}

/// RoundNumberToIncrementAsIfPositive (§13.26).
pub fn round_incr_positive(x: i128, inc: i128, mode: Mode) -> i128 {
    let r1 = x.div_euclid(inc);
    let rem = x.rem_euclid(inc);
    let r = if round_up(unsigned_mode(mode, false), r1.abs(), rem, inc) { r1 + 1 } else { r1 };
    r * inc
}

// ============================================================================================ calendar

pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i64, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

pub fn days_in_year(y: i64) -> u16 {
    if is_leap(y) { 366 } else { 365 }
}

/// Days from 1970-01-01 (Hinnant's days_from_civil) for a valid month.
pub fn epoch_days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn date_from_epoch_days(z: i64) -> IsoDate {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    IsoDate { year: if m <= 2 { y + 1 } else { y }, month: m as u8, day: d as u8 }
}

impl IsoDate {
    pub fn new(year: i64, month: u8, day: u8) -> IsoDate {
        IsoDate { year, month, day }
    }
    pub fn epoch_days(&self) -> i64 {
        epoch_days(self.year, self.month as i64, self.day as i64)
    }
    /// 1 = Monday … 7 = Sunday.
    pub fn day_of_week(&self) -> u8 {
        ((self.epoch_days() + 3).rem_euclid(7) + 1) as u8
    }
    pub fn day_of_year(&self) -> u16 {
        (self.epoch_days() - epoch_days(self.year, 1, 1) + 1) as u16
    }
    /// The ISO week number and its week-numbering year.
    pub fn week_of_year(&self) -> (u8, i64) {
        let doy = self.day_of_year() as i64;
        let dow = self.day_of_week() as i64;
        let w = (doy - dow + 10).div_euclid(7);
        if w < 1 {
            (weeks_in_year(self.year - 1), self.year - 1)
        } else if w > weeks_in_year(self.year) as i64 {
            (1, self.year + 1)
        } else {
            (w as u8, self.year)
        }
    }
    pub fn add_days(&self, d: i64) -> IsoDate {
        date_from_epoch_days(self.epoch_days() + d)
    }
}

fn weeks_in_year(y: i64) -> u8 {
    let p = |y: i64| (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400)).rem_euclid(7);
    if p(y) == 4 || p(y - 1) == 3 { 53 } else { 52 }
}

/// BalanceISOYearMonth.
pub fn balance_year_month(y: i64, m: i64) -> (i64, i64) {
    (y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) + 1)
}

/// BalanceISODate: any day offset from the first of a (balanced) month.
pub fn balance_date(y: i64, m: i64, d: i64) -> IsoDate {
    let (y, m) = balance_year_month(y, m);
    date_from_epoch_days(epoch_days(y, m, 1) + d - 1)
}

pub fn is_valid_date(y: i64, m: i64, d: i64) -> bool {
    (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m as u8) as i64
}

/// RegulateISODate: constrain clamps, reject refuses (`None`).
pub fn regulate_date(y: i64, m: i64, d: i64, constrain: bool) -> Option<IsoDate> {
    if constrain {
        let m = m.clamp(1, 12);
        let d = d.clamp(1, days_in_month(y, m as u8) as i64);
        Some(IsoDate::new(y, m as u8, d as u8))
    } else if is_valid_date(y, m, d) {
        Some(IsoDate::new(y, m as u8, d as u8))
    } else {
        None
    }
}

/// RegulateTime.
pub fn regulate_time(v: [i64; 6], constrain: bool) -> Option<Time> {
    let max = [23, 59, 59, 999, 999, 999];
    let mut out = [0i64; 6];
    for i in 0..6 {
        if constrain {
            out[i] = v[i].clamp(0, max[i]);
        } else if v[i] < 0 || v[i] > max[i] {
            return None;
        } else {
            out[i] = v[i];
        }
    }
    Some(Time { hour: out[0] as u8, minute: out[1] as u8, second: out[2] as u8, ms: out[3] as u16, us: out[4] as u16, ns: out[5] as u16 })
}

impl Time {
    pub const MIDNIGHT: Time = Time { hour: 0, minute: 0, second: 0, ms: 0, us: 0, ns: 0 };
    pub const NOON: Time = Time { hour: 12, minute: 0, second: 0, ms: 0, us: 0, ns: 0 };
    pub fn to_ns(&self) -> i128 {
        ((((self.hour as i128 * 60 + self.minute as i128) * 60 + self.second as i128) * 1000 + self.ms as i128) * 1000 + self.us as i128) * 1000
            + self.ns as i128
    }
    /// A time of day from nanoseconds in [0, NS_PER_DAY).
    pub fn from_ns(n: i128) -> Time {
        let mut n = n;
        let ns = (n % 1000) as u16;
        n /= 1000;
        let us = (n % 1000) as u16;
        n /= 1000;
        let ms = (n % 1000) as u16;
        n /= 1000;
        let second = (n % 60) as u8;
        n /= 60;
        let minute = (n % 60) as u8;
        n /= 60;
        Time { hour: n as u8, minute, second, ms, us, ns }
    }
    pub fn fields(&self) -> [i64; 6] {
        [self.hour as i64, self.minute as i64, self.second as i64, self.ms as i64, self.us as i64, self.ns as i64]
    }
}

/// AddTime / BalanceTime: (whole days carried, time of day).
pub fn add_time(t: Time, ns: i128) -> (i64, Time) {
    let total = t.to_ns() + ns;
    (total.div_euclid(NS_PER_DAY) as i64, Time::from_ns(total.rem_euclid(NS_PER_DAY)))
}

/// GetUTCEpochNanoseconds.
pub fn utc_ns(d: IsoDate, t: Time) -> i128 {
    d.epoch_days() as i128 * NS_PER_DAY + t.to_ns()
}

/// The UTC date and time of an epoch nanosecond count shifted by an offset.
pub fn datetime_from_ns(ns: i128) -> (IsoDate, Time) {
    let days = ns.div_euclid(NS_PER_DAY);
    (date_from_epoch_days(days as i64), Time::from_ns(ns.rem_euclid(NS_PER_DAY)))
}

pub fn valid_epoch_ns(ns: i128) -> bool {
    ns.abs() <= NS_MAX_INSTANT
}

/// ISODateTimeWithinLimits (§5.5.5).
pub fn datetime_within_limits(d: IsoDate, t: Time) -> bool {
    if (d.epoch_days()).abs() > 100_000_001 {
        return false;
    }
    let ns = utc_ns(d, t);
    ns > -NS_MAX_INSTANT - NS_PER_DAY && ns < NS_MAX_INSTANT + NS_PER_DAY
}

/// ISODateWithinLimits.
pub fn date_within_limits(d: IsoDate) -> bool {
    d.year.abs() < 300_000 && datetime_within_limits(d, Time::NOON)
}

/// ISOYearMonthWithinLimits (§9.5.4): April -271821 through September 275760.
pub fn year_month_within_limits(y: i64, m: u8) -> bool {
    !(y < -271_821 || y > 275_760 || (y == -271_821 && m < 4) || (y == 275_760 && m > 9))
}

/// CalendarDateAdd for iso8601 (§12.3.x): years and months on the year-month, the day regulated, then weeks
/// and days on the epoch-day line. `None` when the day is rejected or the result is outside the limits.
pub fn add_date(d: IsoDate, years: i64, months: i64, weeks: i64, days: i64, constrain: bool) -> Option<IsoDate> {
    if years.abs() > 1_000_000 || months.abs() > 12_000_000 || weeks.abs() > 20_000_000 || days.abs() > 200_000_000 {
        return None;
    }
    let (y, m) = balance_year_month(d.year + years, d.month as i64 + months);
    let r = regulate_date(y, m, d.day as i64, constrain)?;
    let out = r.add_days(weeks * 7 + days);
    if date_within_limits(out) { Some(out) } else { None }
}

/// ISODateSurpasses (§12.2.x).
fn surpasses(sign: i64, y1: i64, m1: i64, d1: i64, two: IsoDate) -> bool {
    if y1 != two.year {
        return sign * (y1 - two.year) > 0;
    }
    if m1 != two.month as i64 {
        return sign * (m1 - two.month as i64) > 0;
    }
    if d1 != two.day as i64 {
        return sign * (d1 - two.day as i64) > 0;
    }
    false
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DateDur {
    pub years: i64,
    pub months: i64,
    pub weeks: i64,
    pub days: i64,
}

impl DateDur {
    pub fn sign(&self) -> i64 {
        for v in [self.years, self.months, self.weeks, self.days] {
            if v != 0 {
                return v.signum();
            }
        }
        0
    }
}

/// CalendarDateUntil for iso8601, with closed forms for the step loops of the spec.
pub fn date_until(one: IsoDate, two: IsoDate, largest: Unit) -> DateDur {
    let sign = match two.cmp(&one) {
        core::cmp::Ordering::Equal => return DateDur::default(),
        core::cmp::Ordering::Greater => 1i64,
        core::cmp::Ordering::Less => -1,
    };
    let mut years = 0i64;
    if largest == Unit::Year {
        let mut cand = two.year - one.year;
        while cand != 0 && surpasses(sign, one.year + cand, one.month as i64, one.day as i64, two) {
            cand -= sign;
        }
        years = cand;
    }
    let mut months = 0i64;
    if largest == Unit::Year || largest == Unit::Month {
        let mut cand = (two.year - (one.year + years)) * 12 + (two.month as i64 - one.month as i64);
        loop {
            if cand == 0 {
                break;
            }
            let (y, m) = balance_year_month(one.year + years, one.month as i64 + cand);
            if !surpasses(sign, y, m, one.day as i64, two) {
                break;
            }
            cand -= sign;
        }
        months = cand;
    }
    let (y, m) = balance_year_month(one.year + years, one.month as i64 + months);
    let constrained = regulate_date(y, m, one.day as i64, true).unwrap();
    let span = two.epoch_days() - constrained.epoch_days();
    let mut weeks = 0i64;
    if largest == Unit::Week {
        weeks = span / 7;
    }
    let days = span - weeks * 7;
    DateDur { years, months, weeks, days }
}

// ============================================================================================ durations

/// A Temporal.Duration's ten fields (each an integral Number).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dur {
    pub f: [f64; 10],
}

/// The internal duration record (§7.5.x): date units plus one time duration in nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IDur {
    pub date: DateDur,
    pub time: i128,
}

pub fn trunc(x: f64) -> f64 {
    if x < 0.0 { -crate::numconv::libm_floor(-x) } else { crate::numconv::libm_floor(x) }
}

pub fn f2i(x: f64) -> i128 {
    x as i128
}

pub fn sign_i128(x: i128) -> i64 {
    if x < 0 {
        -1
    } else if x > 0 {
        1
    } else {
        0
    }
}

impl Dur {
    pub fn sign(&self) -> i64 {
        for v in self.f {
            if v < 0.0 {
                return -1;
            }
            if v > 0.0 {
                return 1;
            }
        }
        0
    }
    /// IsValidDuration (§7.5.x).
    pub fn is_valid(&self) -> bool {
        let mut sign = 0.0f64;
        for v in self.f {
            if !v.is_finite() || trunc(v) != v {
                return false;
            }
            if v != 0.0 {
                if sign != 0.0 && (v > 0.0) != (sign > 0.0) {
                    return false;
                }
                sign = v;
            }
        }
        for v in &self.f[0..3] {
            if v.abs() >= 4_294_967_296.0 {
                return false;
            }
        }
        for v in &self.f[3..] {
            if v.abs() >= 1e30 {
                return false;
            }
        }
        let secs = f2i(self.f[3]) * 86_400_000_000_000
            + f2i(self.f[4]) * 3_600_000_000_000
            + f2i(self.f[5]) * 60_000_000_000
            + f2i(self.f[6]) * 1_000_000_000
            + f2i(self.f[7]) * 1_000_000
            + f2i(self.f[8]) * 1_000
            + f2i(self.f[9]);
        secs.abs() <= MAX_TIME_DURATION
    }
    /// DefaultTemporalLargestUnit: the largest non-zero unit (nanosecond when blank).
    pub fn default_largest(&self) -> Unit {
        for (i, v) in self.f.iter().enumerate() {
            if *v != 0.0 {
                return UNITS[i];
            }
        }
        Unit::Nanosecond
    }
    pub fn time_ns(&self) -> i128 {
        f2i(self.f[4]) * 3_600_000_000_000 + f2i(self.f[5]) * 60_000_000_000 + f2i(self.f[6]) * 1_000_000_000 + f2i(self.f[7]) * 1_000_000 + f2i(self.f[8]) * 1_000 + f2i(self.f[9])
    }
    /// ToInternalDurationRecord.
    pub fn internal(&self) -> IDur {
        IDur { date: DateDur { years: self.f[0] as i64, months: self.f[1] as i64, weeks: self.f[2] as i64, days: self.f[3] as i64 }, time: self.time_ns() }
    }
    /// ToInternalDurationRecordWith24HourDays.
    pub fn internal_24h(&self) -> IDur {
        IDur { date: DateDur { years: self.f[0] as i64, months: self.f[1] as i64, weeks: self.f[2] as i64, days: 0 }, time: self.time_ns() + f2i(self.f[3]) * NS_PER_DAY }
    }
    pub fn negated(&self) -> Dur {
        let mut d = *self;
        for v in d.f.iter_mut() {
            *v = if *v == 0.0 { 0.0 } else { -*v };
        }
        d
    }
}

impl IDur {
    pub fn sign(&self) -> i64 {
        let s = self.date.sign();
        if s != 0 { s } else { sign_i128(self.time) }
    }
}

/// TemporalDurationFromInternal (§7.5.x): the time duration balanced up to `largest`; `None` when invalid.
pub fn from_internal(d: IDur, largest: Unit) -> Option<Dur> {
    let sign = sign_i128(d.time) as i128;
    let mut ns = d.time.abs();
    let (mut days, mut hours, mut minutes, mut seconds, mut ms, mut us) = (0i128, 0i128, 0i128, 0i128, 0i128, 0i128);
    match largest {
        Unit::Year | Unit::Month | Unit::Week | Unit::Day | Unit::Hour | Unit::Minute | Unit::Second | Unit::Millisecond | Unit::Microsecond => {
            us = ns / 1000;
            ns %= 1000;
            if largest != Unit::Microsecond {
                ms = us / 1000;
                us %= 1000;
                if largest != Unit::Millisecond {
                    seconds = ms / 1000;
                    ms %= 1000;
                    if largest != Unit::Second {
                        minutes = seconds / 60;
                        seconds %= 60;
                        if largest != Unit::Minute {
                            hours = minutes / 60;
                            minutes %= 60;
                            if largest.is_date() {
                                days = hours / 24;
                                hours %= 24;
                            }
                        }
                    }
                }
            }
        }
        Unit::Nanosecond => {}
    }
    let z = |x: f64| if x == 0.0 { 0.0 } else { x };
    let out = Dur {
        f: [
            z(d.date.years as f64),
            z(d.date.months as f64),
            z(d.date.weeks as f64),
            z((d.date.days as i128 + days * sign) as f64),
            z((hours * sign) as f64),
            z((minutes * sign) as f64),
            z((seconds * sign) as f64),
            z((ms * sign) as f64),
            z((us * sign) as f64),
            z((ns * sign) as f64),
        ],
    };
    if out.is_valid() { Some(out) } else { None }
}

// ============================================================================================ strings

/// PadISOYear.
pub fn pad_year(y: i64) -> String {
    if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else {
        format!("{}{:06}", if y < 0 { '-' } else { '+' }, y.unsigned_abs())
    }
}

/// Seconds-string precision: `None` = auto, `Some(10)` = minute, `Some(0..=9)` digits.
pub type Precision = Option<u8>;
pub const PREC_MINUTE: u8 = 10;

/// FormatFractionalSeconds.
pub fn fraction(sub_ns: u32, p: Precision) -> String {
    match p {
        None => {
            if sub_ns == 0 {
                return String::new();
            }
            let s = format!("{sub_ns:09}");
            format!(".{}", s.trim_end_matches('0'))
        }
        Some(0) => String::new(),
        Some(n) => {
            let s = format!("{sub_ns:09}");
            format!(".{}", &s[..n as usize])
        }
    }
}

/// FormatTimeString.
pub fn time_string(t: Time, p: Precision) -> String {
    if p == Some(PREC_MINUTE) {
        return format!("{:02}:{:02}", t.hour, t.minute);
    }
    let sub = t.ms as u32 * 1_000_000 + t.us as u32 * 1000 + t.ns as u32;
    format!("{:02}:{:02}:{:02}{}", t.hour, t.minute, t.second, fraction(sub, p))
}

pub fn date_string(d: IsoDate) -> String {
    format!("{}-{:02}-{:02}", pad_year(d.year), d.month, d.day)
}

/// FormatUTCOffsetNanoseconds / FormatDateTimeUTCOffsetRounded's tail.
pub fn offset_string(ns: i64, round_to_minutes: bool) -> String {
    let ns = if round_to_minutes { round_incr(ns as i128, 60_000_000_000, Mode::HalfExpand) as i64 } else { ns };
    let sign = if ns < 0 { '-' } else { '+' };
    let a = ns.unsigned_abs();
    let h = a / 3_600_000_000_000;
    let m = (a / 60_000_000_000) % 60;
    let s = (a / 1_000_000_000) % 60;
    let sub = (a % 1_000_000_000) as u32;
    if s == 0 && sub == 0 {
        format!("{sign}{h:02}:{m:02}")
    } else {
        format!("{sign}{h:02}:{m:02}:{s:02}{}", fraction(sub, None))
    }
}

/// TemporalDurationToString (§7.5.x).
pub fn duration_string(d: &Dur, p: Precision) -> String {
    let sign = d.sign();
    let abs = |x: f64| -> String {
        let a = x.abs();
        if a < 1e21 { format!("{}", a as u128) } else { crate::vm::ops::number_to_jsstr(a).to_rust() }
    };
    let mut date = String::new();
    for (i, l) in ["Y", "M", "W", "D"].iter().enumerate() {
        if d.f[i] != 0.0 {
            date.push_str(&abs(d.f[i]));
            date.push_str(l);
        }
    }
    let mut time = String::new();
    if d.f[4] != 0.0 {
        time.push_str(&abs(d.f[4]));
        time.push('H');
    }
    if d.f[5] != 0.0 {
        time.push_str(&abs(d.f[5]));
        time.push('M');
    }
    let secs = f2i(d.f[6]) * 1_000_000_000 + f2i(d.f[7]) * 1_000_000 + f2i(d.f[8]) * 1000 + f2i(d.f[9]);
    let zero_min_up = d.f[..6].iter().all(|v| *v == 0.0);
    if secs != 0 || zero_min_up || p.is_some() {
        let a = secs.abs();
        time.push_str(&format!("{}", a / 1_000_000_000));
        time.push_str(&fraction((a % 1_000_000_000) as u32, p));
        time.push('S');
    }
    let mut out = String::new();
    if sign < 0 {
        out.push('-');
    }
    out.push('P');
    out.push_str(&date);
    if !time.is_empty() {
        out.push('T');
        out.push_str(&time);
    }
    out
}
