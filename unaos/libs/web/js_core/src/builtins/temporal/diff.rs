//! Differences, additions and relative rounding (proposal-temporal 2025 §7.5): DifferenceISODateTime,
//! DifferenceZonedDateTime, AddZonedDateTime, RoundRelativeDuration with its three nudges
//! (NudgeToCalendarUnit, NudgeToZonedTime, NudgeToDayOrTime) and BubbleRelativeDuration, and the "total"
//! variants that Duration.prototype.total reads. All exact: date units as i64, time as i128 nanoseconds.

use super::iso::*;
use super::zone::{self, Disambiguation, TimeZone};

pub type DResult<T> = Result<T, &'static str>;

const RANGE: &str = "date-time outside the representable range";

/// RoundTimeDurationToIncrement.
pub fn round_time_incr(d: i128, inc: i128, mode: Mode) -> DResult<i128> {
    let r = round_incr(d, inc, mode);
    if r.abs() > MAX_TIME_DURATION { Err("time duration out of range") } else { Ok(r) }
}

/// RoundTimeDuration.
pub fn round_time(d: i128, inc: i128, unit: Unit, mode: Mode) -> DResult<i128> {
    round_time_incr(d, unit.ns() * inc, mode)
}

/// DifferenceISODateTime (§5.5.x).
pub fn diff_datetime(d1: IsoDate, t1: Time, d2: IsoDate, t2: Time, largest: Unit) -> IDur {
    let mut time = t2.to_ns() - t1.to_ns();
    let time_sign = sign_i128(time);
    let date_sign = match d2.cmp(&d1) {
        core::cmp::Ordering::Greater => 1,
        core::cmp::Ordering::Less => -1,
        _ => 0,
    };
    let mut adjusted = d2;
    if time_sign == -date_sign {
        adjusted = d2.add_days(time_sign);
        time -= time_sign as i128 * NS_PER_DAY;
    }
    let date_largest = larger(Unit::Day, largest);
    let mut date = date_until(d1, adjusted, date_largest);
    if largest != date_largest {
        time += date.days as i128 * NS_PER_DAY;
        date.days = 0;
    }
    IDur { date, time }
}

/// DifferenceZonedDateTime (§6.5.x), for a date `largest` unit.
pub fn diff_zoned(ns1: i128, ns2: i128, tz: &TimeZone, largest: Unit) -> DResult<IDur> {
    if ns1 == ns2 {
        return Ok(IDur::default());
    }
    let (sd, st) = zone::local(tz, ns1);
    let (ed, et) = zone::local(tz, ns2);
    if sd == ed {
        return Ok(IDur { date: DateDur::default(), time: ns2 - ns1 });
    }
    let sign: i64 = if ns2 - ns1 < 0 { -1 } else { 1 };
    let max_corr = if sign == 1 { 2 } else { 1 };
    let mut corr = 0i64;
    let mut time = et.to_ns() - st.to_ns();
    if sign_i128(time) == -sign {
        corr += 1;
    }
    let mut inter = ed;
    let mut ok = false;
    while corr <= max_corr && !ok {
        inter = ed.add_days(-corr * sign);
        let ins = zone::epoch_ns_for(tz, inter, st, Disambiguation::Compatible)?;
        time = ns2 - ins;
        if sign != -sign_i128(time) {
            ok = true;
        }
        corr += 1;
    }
    if !ok {
        return Err("zoned difference did not converge");
    }
    let date = date_until(sd, inter, larger(largest, Unit::Day));
    Ok(IDur { date, time })
}

/// AddZonedDateTime (§6.5.x).
pub fn add_zoned(ns: i128, tz: &TimeZone, d: &IDur, constrain: bool) -> DResult<i128> {
    if d.date.sign() == 0 {
        let r = ns + d.time;
        return if valid_epoch_ns(r) { Ok(r) } else { Err(RANGE) };
    }
    let (ld, lt) = zone::local(tz, ns);
    let added = add_date(ld, d.date.years, d.date.months, d.date.weeks, d.date.days, constrain).ok_or("date arithmetic out of range or rejected")?;
    if !datetime_within_limits(added, lt) {
        return Err(RANGE);
    }
    let inter = zone::epoch_ns_for(tz, added, lt, Disambiguation::Compatible)?;
    let r = inter + d.time;
    if valid_epoch_ns(r) { Ok(r) } else { Err(RANGE) }
}

/// Wall-clock date-time → epoch ns, as UTC (plain) or in a zone (compatible).
fn epoch_of(tz: Option<&TimeZone>, d: IsoDate, t: Time) -> DResult<i128> {
    match tz {
        None => Ok(utc_ns(d, t)),
        Some(z) => zone::epoch_ns_for(z, d, t, Disambiguation::Compatible),
    }
}

pub struct Nudge {
    pub dur: IDur,
    pub epoch: i128,
    pub expanded: bool,
}

/// NudgeToCalendarUnit (§7.5.x): also returns the fractional total.
#[allow(clippy::too_many_arguments)]
pub fn nudge_calendar(sign: i64, d: &IDur, dest: i128, od: IsoDate, ot: Time, tz: Option<&TimeZone>, inc: i64, unit: Unit, mode: Mode) -> DResult<(Nudge, f64)> {
    let trunc_to = |v: i64| -> i64 { (round_incr(v as i128, inc as i128, Mode::Trunc)) as i64 };
    let (r1, _r2, start, end) = match unit {
        Unit::Year => {
            let y = trunc_to(d.date.years);
            let r2 = y + inc * sign;
            (y, r2, DateDur { years: y, ..Default::default() }, DateDur { years: r2, ..Default::default() })
        }
        Unit::Month => {
            let m = trunc_to(d.date.months);
            let r2 = m + inc * sign;
            (m, r2, DateDur { years: d.date.years, months: m, ..Default::default() }, DateDur { years: d.date.years, months: r2, ..Default::default() })
        }
        Unit::Week => {
            let ym = DateDur { years: d.date.years, months: d.date.months, ..Default::default() };
            let ws = add_date(od, ym.years, ym.months, 0, 0, true).ok_or(RANGE)?;
            let we = ws.add_days(d.date.days);
            let u = date_until(ws, we, Unit::Week);
            let w = trunc_to(d.date.weeks + u.weeks);
            let r2 = w + inc * sign;
            (
                w,
                r2,
                DateDur { years: d.date.years, months: d.date.months, weeks: w, days: 0 },
                DateDur { years: d.date.years, months: d.date.months, weeks: r2, days: 0 },
            )
        }
        _ => {
            let dd = trunc_to(d.date.days);
            let r2 = dd + inc * sign;
            (dd, r2, DateDur { days: dd, ..d.date }, DateDur { days: r2, ..d.date })
        }
    };
    let sdate = add_date(od, start.years, start.months, start.weeks, start.days, true).ok_or(RANGE)?;
    let edate = add_date(od, end.years, end.months, end.weeks, end.days, true).ok_or(RANGE)?;
    let s_ns = epoch_of(tz, sdate, ot)?;
    let e_ns = epoch_of(tz, edate, ot)?;
    if s_ns == e_ns {
        return Err("rounding interval is empty");
    }
    let num = dest - s_ns;
    let den = e_ns - s_ns;
    let total = r1 as f64 + (num as f64 / den as f64) * inc as f64 * sign as f64;
    let um = unsigned_mode(mode, sign < 0);
    let (an, ad) = (num.abs(), den.abs());
    let up = if an >= ad { true } else { round_up(um, r1.abs() as i128 / inc as i128, an, ad) };
    let (dur, epoch, expanded) = if up { (end, e_ns, true) } else { (start, s_ns, false) };
    Ok((Nudge { dur: IDur { date: dur, time: 0 }, epoch, expanded }, total))
}

/// NudgeToZonedTime (§7.5.x).
#[allow(clippy::too_many_arguments)]
pub fn nudge_zoned(sign: i64, d: &IDur, od: IsoDate, ot: Time, tz: &TimeZone, inc: i64, unit: Unit, mode: Mode) -> DResult<Nudge> {
    let start = add_date(od, d.date.years, d.date.months, d.date.weeks, d.date.days, true).ok_or(RANGE)?;
    let end = start.add_days(sign);
    let s_ns = zone::epoch_ns_for(tz, start, ot, Disambiguation::Compatible)?;
    let e_ns = zone::epoch_ns_for(tz, end, ot, Disambiguation::Compatible)?;
    let span = e_ns - s_ns;
    let step = inc as i128 * unit.ns();
    let mut rounded = round_time_incr(d.time, step, mode)?;
    let beyond = rounded - span;
    let (epoch, delta, expanded) = if sign_i128(beyond) != -sign {
        rounded = round_time_incr(beyond, step, mode)?;
        (e_ns + rounded, sign, true)
    } else {
        (s_ns + rounded, 0, false)
    };
    let date = DateDur { days: d.date.days + delta, ..d.date };
    Ok(Nudge { dur: IDur { date, time: rounded }, epoch, expanded })
}

/// NudgeToDayOrTime (§7.5.x).
pub fn nudge_day_time(d: &IDur, dest: i128, largest: Unit, inc: i64, smallest: Unit, mode: Mode) -> DResult<Nudge> {
    let time = d.time + d.date.days as i128 * NS_PER_DAY;
    let rounded = round_time_incr(time, smallest.ns() * inc as i128, mode)?;
    let diff = rounded - time;
    let whole = time / NS_PER_DAY;
    let rwhole = rounded / NS_PER_DAY;
    let delta = rwhole - whole;
    let expanded = sign_i128(delta) == sign_i128(time);
    let epoch = dest + diff;
    let (days, rem) = if larger(largest, Unit::Day) == largest { (rwhole, rounded - rwhole * NS_PER_DAY) } else { (0, rounded) };
    Ok(Nudge { dur: IDur { date: DateDur { days: days as i64, ..d.date }, time: rem }, epoch, expanded })
}

/// BubbleRelativeDuration (§7.5.x).
#[allow(clippy::too_many_arguments)]
pub fn bubble(sign: i64, d: IDur, nudged: i128, od: IsoDate, ot: Time, tz: Option<&TimeZone>, largest: Unit, smallest: Unit) -> DResult<IDur> {
    let mut d = d;
    if smallest == largest {
        return Ok(d);
    }
    let li = largest.index() as i64;
    let mut ui = smallest.index() as i64 - 1;
    while ui >= li {
        let unit = UNITS[ui as usize];
        if unit != Unit::Week || largest == Unit::Week {
            let end = match unit {
                Unit::Year => DateDur { years: d.date.years + sign, ..Default::default() },
                Unit::Month => DateDur { years: d.date.years, months: d.date.months + sign, ..Default::default() },
                Unit::Week => DateDur { years: d.date.years, months: d.date.months, weeks: d.date.weeks + sign, days: 0 },
                _ => {
                    ui -= 1;
                    continue;
                }
            };
            let edate = add_date(od, end.years, end.months, end.weeks, end.days, true).ok_or(RANGE)?;
            let e_ns = epoch_of(tz, edate, ot)?;
            let beyond = nudged - e_ns;
            if sign_i128(beyond) != -sign {
                d = IDur { date: end, time: 0 };
            } else {
                break;
            }
        }
        ui -= 1;
    }
    Ok(d)
}

/// RoundRelativeDuration (§7.5.x).
#[allow(clippy::too_many_arguments)]
pub fn round_relative(d: IDur, dest: i128, od: IsoDate, ot: Time, tz: Option<&TimeZone>, largest: Unit, inc: i64, smallest: Unit, mode: Mode) -> DResult<IDur> {
    let irregular = smallest.is_calendar() || (tz.is_some() && smallest == Unit::Day);
    let sign = if d.sign() < 0 { -1 } else { 1 };
    let n = if irregular {
        nudge_calendar(sign, &d, dest, od, ot, tz, inc, smallest, mode)?.0
    } else if let Some(z) = tz {
        nudge_zoned(sign, &d, od, ot, z, inc, smallest, mode)?
    } else {
        nudge_day_time(&d, dest, largest, inc, smallest, mode)?
    };
    let mut out = n.dur;
    if n.expanded && smallest != Unit::Week {
        let start_unit = larger(smallest, Unit::Day);
        out = bubble(sign, out, n.epoch, od, ot, tz, largest, start_unit)?;
    }
    Ok(out)
}

/// TotalRelativeDuration (§7.5.x).
pub fn total_relative(d: IDur, dest: i128, od: IsoDate, ot: Time, tz: Option<&TimeZone>, unit: Unit) -> DResult<f64> {
    if unit.is_calendar() || (tz.is_some() && unit == Unit::Day) {
        let sign = if d.sign() < 0 { -1 } else { 1 };
        return Ok(nudge_calendar(sign, &d, dest, od, ot, tz, 1, unit, Mode::Trunc)?.1);
    }
    let t = d.time + d.date.days as i128 * NS_PER_DAY;
    Ok(total_time(t, unit))
}

/// TotalTimeDuration: an exact quotient rendered as the nearest Number.
pub fn total_time(t: i128, unit: Unit) -> f64 {
    let div = unit.ns();
    let q = t / div;
    let r = t % div;
    q as f64 + r as f64 / div as f64
}

/// DifferencePlainDateTimeWithRounding (§5.5.x).
#[allow(clippy::too_many_arguments)]
pub fn diff_plain_rounded(d1: IsoDate, t1: Time, d2: IsoDate, t2: Time, largest: Unit, inc: i64, smallest: Unit, mode: Mode) -> DResult<IDur> {
    if d1 == d2 && t1 == t2 {
        return Ok(IDur::default());
    }
    if !datetime_within_limits(d1, t1) || !datetime_within_limits(d2, t2) {
        return Err(RANGE);
    }
    let diff = diff_datetime(d1, t1, d2, t2, largest);
    if smallest == Unit::Nanosecond && inc == 1 {
        return Ok(diff);
    }
    let dest = utc_ns(d2, t2);
    round_relative(diff, dest, d1, t1, None, largest, inc, smallest, mode)
}

pub fn diff_plain_total(d1: IsoDate, t1: Time, d2: IsoDate, t2: Time, unit: Unit) -> DResult<f64> {
    if d1 == d2 && t1 == t2 {
        return Ok(0.0);
    }
    if !datetime_within_limits(d1, t1) || !datetime_within_limits(d2, t2) {
        return Err(RANGE);
    }
    let diff = diff_datetime(d1, t1, d2, t2, unit);
    if unit == Unit::Nanosecond {
        return Ok(diff.time as f64);
    }
    total_relative(diff, utc_ns(d2, t2), d1, t1, None, unit)
}

/// DifferenceInstant (§8.5.x) as an internal duration (time only).
pub fn diff_instant(ns1: i128, ns2: i128, inc: i64, smallest: Unit, mode: Mode) -> DResult<IDur> {
    let t = round_time(ns2 - ns1, inc as i128, smallest, mode)?;
    Ok(IDur { date: DateDur::default(), time: t })
}

/// DifferenceZonedDateTimeWithRounding (§6.5.x).
#[allow(clippy::too_many_arguments)]
pub fn diff_zoned_rounded(ns1: i128, ns2: i128, tz: &TimeZone, largest: Unit, inc: i64, smallest: Unit, mode: Mode) -> DResult<IDur> {
    if !largest.is_date() {
        return diff_instant(ns1, ns2, inc, smallest, mode);
    }
    let d = diff_zoned(ns1, ns2, tz, largest)?;
    if smallest == Unit::Nanosecond && inc == 1 {
        return Ok(d);
    }
    let (od, ot) = zone::local(tz, ns1);
    round_relative(d, ns2, od, ot, Some(tz), largest, inc, smallest, mode)
}

pub fn diff_zoned_total(ns1: i128, ns2: i128, tz: &TimeZone, unit: Unit) -> DResult<f64> {
    if !unit.is_date() {
        return Ok(total_time(ns2 - ns1, unit));
    }
    let d = diff_zoned(ns1, ns2, tz, unit)?;
    let (od, ot) = zone::local(tz, ns1);
    total_relative(d, ns2, od, ot, Some(tz), unit)
}

/// RoundTime (§4.5.x): (days carried, rounded time).
pub fn round_wall_time(t: Time, inc: i64, unit: Unit, mode: Mode) -> (i64, Time) {
    let full = t.to_ns();
    let q = match unit {
        Unit::Day | Unit::Hour => full,
        Unit::Minute => full - t.hour as i128 * 3_600_000_000_000,
        Unit::Second => full - (t.hour as i128 * 60 + t.minute as i128) * 60_000_000_000,
        Unit::Millisecond => t.ms as i128 * 1_000_000 + t.us as i128 * 1000 + t.ns as i128,
        Unit::Microsecond => t.us as i128 * 1000 + t.ns as i128,
        _ => t.ns as i128,
    };
    let base = full - q;
    let len = unit.ns();
    let r = round_incr(q, inc as i128 * len, mode);
    if unit == Unit::Day {
        return ((r / NS_PER_DAY) as i64, Time::MIDNIGHT);
    }
    add_time(Time::MIDNIGHT, base + r)
}

/// RoundISODateTime.
pub fn round_datetime(d: IsoDate, t: Time, inc: i64, unit: Unit, mode: Mode) -> (IsoDate, Time) {
    let (days, tt) = round_wall_time(t, inc, unit, mode);
    (d.add_days(days), tt)
}
