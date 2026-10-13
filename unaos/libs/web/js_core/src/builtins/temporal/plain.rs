//! The wall-clock types (proposal-temporal 2025 §3, §4, §5, §9, §10): Temporal.PlainDate, PlainTime,
//! PlainDateTime, PlainYearMonth and PlainMonthDay on the ISO 8601 calendar. The calendar getters are
//! generic over the receiver's brand (`K`), so each prototype's accessor checks its own internal slots.

use super::*;

pub fn init(vm: &mut Vm, d: &Def, ns: Obj) {
    // ---------------------------------------------------------------- PlainDate
    let (c, p) = d.class(vm, ns, DATE, 3, date_ctor);
    d.method(vm, c, "from", 1, date_from);
    d.method(vm, c, "compare", 2, date_compare);
    date_getters::<DATE>(vm, d, p);
    d.method(vm, p, "toPlainYearMonth", 0, date_to_ym);
    d.method(vm, p, "toPlainMonthDay", 0, date_to_md);
    d.method(vm, p, "add", 1, date_add);
    d.method(vm, p, "subtract", 1, date_subtract);
    d.method(vm, p, "with", 1, date_with);
    d.method(vm, p, "withCalendar", 1, with_calendar::<DATE>);
    d.method(vm, p, "until", 1, date_until_m);
    d.method(vm, p, "since", 1, date_since);
    d.method(vm, p, "equals", 1, date_equals);
    d.method(vm, p, "toPlainDateTime", 0, date_to_pdt);
    d.method(vm, p, "toZonedDateTime", 1, date_to_zoned);
    d.method(vm, p, "toString", 0, date_to_string);
    d.method(vm, p, "toLocaleString", 0, locale::<DATE>);
    d.method(vm, p, "toJSON", 0, date_to_json);
    d.method(vm, p, "valueOf", 0, value_of);
    // ---------------------------------------------------------------- PlainTime
    let (c, p) = d.class(vm, ns, TIME, 0, time_ctor);
    d.method(vm, c, "from", 1, time_from);
    d.method(vm, c, "compare", 2, time_compare);
    time_getters::<TIME>(vm, d, p);
    d.method(vm, p, "add", 1, time_add);
    d.method(vm, p, "subtract", 1, time_subtract);
    d.method(vm, p, "with", 1, time_with);
    d.method(vm, p, "until", 1, time_until);
    d.method(vm, p, "since", 1, time_since);
    d.method(vm, p, "round", 1, time_round);
    d.method(vm, p, "equals", 1, time_equals);
    d.method(vm, p, "toString", 0, time_to_string);
    d.method(vm, p, "toLocaleString", 0, locale::<TIME>);
    d.method(vm, p, "toJSON", 0, time_to_json);
    d.method(vm, p, "valueOf", 0, value_of);
    // ---------------------------------------------------------------- PlainDateTime
    let (c, p) = d.class(vm, ns, DATETIME, 3, pdt_ctor);
    d.method(vm, c, "from", 1, pdt_from);
    d.method(vm, c, "compare", 2, pdt_compare);
    date_getters::<DATETIME>(vm, d, p);
    time_getters::<DATETIME>(vm, d, p);
    d.method(vm, p, "with", 1, pdt_with);
    d.method(vm, p, "withPlainTime", 0, pdt_with_time);
    d.method(vm, p, "withCalendar", 1, with_calendar::<DATETIME>);
    d.method(vm, p, "add", 1, pdt_add);
    d.method(vm, p, "subtract", 1, pdt_subtract);
    d.method(vm, p, "until", 1, pdt_until);
    d.method(vm, p, "since", 1, pdt_since);
    d.method(vm, p, "round", 1, pdt_round);
    d.method(vm, p, "equals", 1, pdt_equals);
    d.method(vm, p, "toString", 0, pdt_to_string);
    d.method(vm, p, "toLocaleString", 0, locale::<DATETIME>);
    d.method(vm, p, "toJSON", 0, pdt_to_json);
    d.method(vm, p, "valueOf", 0, value_of);
    d.method(vm, p, "toZonedDateTime", 1, pdt_to_zoned);
    d.method(vm, p, "toPlainDate", 0, pdt_to_date);
    d.method(vm, p, "toPlainTime", 0, pdt_to_time);
    // ---------------------------------------------------------------- PlainYearMonth
    let (c, p) = d.class(vm, ns, YEARMONTH, 2, ym_ctor);
    d.method(vm, c, "from", 1, ym_from);
    d.method(vm, c, "compare", 2, ym_compare);
    d.getter(vm, p, "calendarId", calendar_id::<YEARMONTH>);
    d.getter(vm, p, "era", era::<YEARMONTH>);
    d.getter(vm, p, "eraYear", era::<YEARMONTH>);
    d.getter(vm, p, "year", g_year::<YEARMONTH>);
    d.getter(vm, p, "month", g_month::<YEARMONTH>);
    d.getter(vm, p, "monthCode", g_month_code::<YEARMONTH>);
    d.getter(vm, p, "daysInYear", g_days_in_year::<YEARMONTH>);
    d.getter(vm, p, "daysInMonth", g_days_in_month::<YEARMONTH>);
    d.getter(vm, p, "monthsInYear", g_months_in_year::<YEARMONTH>);
    d.getter(vm, p, "inLeapYear", g_in_leap_year::<YEARMONTH>);
    d.method(vm, p, "with", 1, ym_with);
    d.method(vm, p, "add", 1, ym_add);
    d.method(vm, p, "subtract", 1, ym_subtract);
    d.method(vm, p, "until", 1, ym_until);
    d.method(vm, p, "since", 1, ym_since);
    d.method(vm, p, "equals", 1, ym_equals);
    d.method(vm, p, "toString", 0, ym_to_string);
    d.method(vm, p, "toLocaleString", 0, locale::<YEARMONTH>);
    d.method(vm, p, "toJSON", 0, ym_to_json);
    d.method(vm, p, "valueOf", 0, value_of);
    d.method(vm, p, "toPlainDate", 1, ym_to_date);
    // ---------------------------------------------------------------- PlainMonthDay
    let (c, p) = d.class(vm, ns, MONTHDAY, 2, md_ctor);
    d.method(vm, c, "from", 1, md_from);
    d.getter(vm, p, "calendarId", calendar_id::<MONTHDAY>);
    d.getter(vm, p, "monthCode", g_month_code::<MONTHDAY>);
    d.getter(vm, p, "day", g_day::<MONTHDAY>);
    d.method(vm, p, "with", 1, md_with);
    d.method(vm, p, "equals", 1, md_equals);
    d.method(vm, p, "toString", 0, md_to_string);
    d.method(vm, p, "toLocaleString", 0, locale::<MONTHDAY>);
    d.method(vm, p, "toJSON", 0, md_to_json);
    d.method(vm, p, "valueOf", 0, value_of);
    d.method(vm, p, "toPlainDate", 1, md_to_date);
}

/// The calendar getters shared by PlainDate, PlainDateTime and ZonedDateTime.
pub fn date_getters<const K: usize>(vm: &mut Vm, d: &Def, p: Obj) {
    d.getter(vm, p, "calendarId", calendar_id::<K>);
    d.getter(vm, p, "era", era::<K>);
    d.getter(vm, p, "eraYear", era::<K>);
    d.getter(vm, p, "year", g_year::<K>);
    d.getter(vm, p, "month", g_month::<K>);
    d.getter(vm, p, "monthCode", g_month_code::<K>);
    d.getter(vm, p, "day", g_day::<K>);
    d.getter(vm, p, "dayOfWeek", g_day_of_week::<K>);
    d.getter(vm, p, "dayOfYear", g_day_of_year::<K>);
    d.getter(vm, p, "weekOfYear", g_week_of_year::<K>);
    d.getter(vm, p, "yearOfWeek", g_year_of_week::<K>);
    d.getter(vm, p, "daysInWeek", g_days_in_week::<K>);
    d.getter(vm, p, "daysInMonth", g_days_in_month::<K>);
    d.getter(vm, p, "daysInYear", g_days_in_year::<K>);
    d.getter(vm, p, "monthsInYear", g_months_in_year::<K>);
    d.getter(vm, p, "inLeapYear", g_in_leap_year::<K>);
}

pub fn time_getters<const K: usize>(vm: &mut Vm, d: &Def, p: Obj) {
    d.getter(vm, p, "hour", g_time::<K, 0>);
    d.getter(vm, p, "minute", g_time::<K, 1>);
    d.getter(vm, p, "second", g_time::<K, 2>);
    d.getter(vm, p, "millisecond", g_time::<K, 3>);
    d.getter(vm, p, "microsecond", g_time::<K, 4>);
    d.getter(vm, p, "nanosecond", g_time::<K, 5>);
}

/// The receiver's wall-clock (date, time) for brand `K`.
pub fn parts<const K: usize>(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<(IsoDate, Time)> {
    Ok(match this_of(vm, ctx, K, m)? {
        TData::Date(d) | TData::YearMonth(d) | TData::MonthDay(d) => (d, Time::MIDNIGHT),
        TData::DateTime(d, t) => (d, t),
        TData::Time(t) => (IsoDate::new(1970, 1, 1), t),
        TData::Zoned(n, tz) => zone::local(&tz, n),
        _ => unreachable!(),
    })
}

fn calendar_id<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    parts::<K>(vm, ctx, "calendarId")?;
    Ok(Value::str("iso8601"))
}
fn era<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    parts::<K>(vm, ctx, "era")?;
    Ok(Value::Undefined)
}
fn g_year<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(parts::<K>(vm, ctx, "year")?.0.year as f64))
}
fn g_month<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(parts::<K>(vm, ctx, "month")?.0.month as f64))
}
fn g_month_code<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::str(&month_code_str(parts::<K>(vm, ctx, "monthCode")?.0.month)))
}
fn g_day<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(parts::<K>(vm, ctx, "day")?.0.day as f64))
}
fn g_day_of_week<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(parts::<K>(vm, ctx, "dayOfWeek")?.0.day_of_week() as f64))
}
fn g_day_of_year<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(parts::<K>(vm, ctx, "dayOfYear")?.0.day_of_year() as f64))
}
fn g_week_of_year<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(parts::<K>(vm, ctx, "weekOfYear")?.0.week_of_year().0 as f64))
}
fn g_year_of_week<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(parts::<K>(vm, ctx, "yearOfWeek")?.0.week_of_year().1 as f64))
}
fn g_days_in_week<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    parts::<K>(vm, ctx, "daysInWeek")?;
    Ok(num(7.0))
}
fn g_days_in_month<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = parts::<K>(vm, ctx, "daysInMonth")?.0;
    Ok(num(days_in_month(d.year, d.month) as f64))
}
fn g_days_in_year<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(days_in_year(parts::<K>(vm, ctx, "daysInYear")?.0.year) as f64))
}
fn g_months_in_year<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    parts::<K>(vm, ctx, "monthsInYear")?;
    Ok(num(12.0))
}
fn g_in_leap_year<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(is_leap(parts::<K>(vm, ctx, "inLeapYear")?.0.year)))
}
fn g_time<const K: usize, const F: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let names = ["hour", "minute", "second", "millisecond", "microsecond", "nanosecond"];
    let t = parts::<K>(vm, ctx, names[F])?.1;
    Ok(num(t.fields()[F] as f64))
}

pub fn num(v: f64) -> Value {
    Value::Number(v)
}

fn with_calendar<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_of(vm, ctx, K, "withCalendar")?;
    let c = vm.arg(ctx, 0);
    if c.is_undefined() {
        return vm.throw_type("withCalendar requires a calendar");
    }
    to_calendar(vm, &c)?;
    Ok(make(vm, ctx, d))
}

fn locale<const K: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_of(vm, ctx, K, "toLocaleString")?;
    to_locale_string(vm, ctx, &d)
}

fn int_arg(vm: &mut Vm, ctx: &CallCtx, i: usize, default: f64) -> JsResult<f64> {
    let v = vm.arg(ctx, i);
    if v.is_undefined() {
        return Ok(default);
    }
    to_int(vm, &v)
}

fn cmp_value<T: Ord>(a: T, b: T) -> Value {
    Value::Number(match a.cmp(&b) {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Greater => 1.0,
        _ => 0.0,
    })
}

/// ToDateDurationRecordWithoutTime: the date units plus the whole days of the time part.
fn date_part(d: &Dur) -> DateDur {
    let i = d.internal_24h();
    DateDur { days: (i.time / NS_PER_DAY) as i64, ..i.date }
}

fn ok_date(vm: &mut Vm, ctx: &CallCtx, d: Option<IsoDate>) -> JsResult<Value> {
    match d {
        Some(d) => Ok(make(vm, ctx, TData::Date(d))),
        None => vm.throw_range("date out of range or rejected (overflow: reject)"),
    }
}

fn this_date(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<IsoDate> {
    match this_of(vm, ctx, DATE, m)? {
        TData::Date(d) => Ok(d),
        _ => unreachable!(),
    }
}

// ============================================================================================ PlainDate

fn date_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Temporal.PlainDate requires 'new'");
    }
    let y = int_arg(vm, ctx, 0, f64::NAN)?;
    let m = int_arg(vm, ctx, 1, f64::NAN)?;
    let d = int_arg(vm, ctx, 2, f64::NAN)?;
    let c = vm.arg(ctx, 3);
    ctor_calendar(vm, &c)?;
    if y.abs() > 1e6 || !is_valid_date(y as i64, m as i64, d as i64) {
        return vm.throw_range("invalid ISO date");
    }
    let date = IsoDate::new(y as i64, m as u8, d as u8);
    if !date_within_limits(date) {
        return vm.throw_range("date outside the representable range");
    }
    construct(vm, ctx, TData::Date(date))
}

fn date_from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let d = to_date(vm, &a, &o)?;
    Ok(make(vm, ctx, TData::Date(d)))
}

fn date_compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    let x = to_date(vm, &a, &Value::Undefined)?;
    let y = to_date(vm, &b, &Value::Undefined)?;
    Ok(cmp_value(x, y))
}

fn date_to_ym(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_date(vm, ctx, "toPlainYearMonth")?;
    if !year_month_within_limits(d.year, d.month) {
        return vm.throw_range("year-month outside the representable range");
    }
    Ok(make(vm, ctx, TData::YearMonth(IsoDate::new(d.year, d.month, 1))))
}

fn date_to_md(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_date(vm, ctx, "toPlainMonthDay")?;
    Ok(make(vm, ctx, TData::MonthDay(IsoDate::new(1972, d.month, d.day))))
}

fn date_add_sub(vm: &mut Vm, ctx: &CallCtx, neg: bool) -> JsResult<Value> {
    let d = this_date(vm, ctx, if neg { "subtract" } else { "add" })?;
    let a = vm.arg(ctx, 0);
    let mut dur = to_duration(vm, &a)?;
    if neg {
        dur = dur.negated();
    }
    let dd = date_part(&dur);
    let ov = vm.arg(ctx, 1);
    let o = get_options(vm, &ov)?;
    let c = opt_overflow(vm, o)?;
    ok_date(vm, ctx, add_date(d, dd.years, dd.months, dd.weeks, dd.days, c))
}

fn date_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    date_add_sub(vm, ctx, false)
}

fn date_subtract(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    date_add_sub(vm, ctx, true)
}

fn date_with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_date(vm, ctx, "with")?;
    let a = vm.arg(ctx, 0);
    let o = reject_temporal_like(vm, &a)?;
    let partial = prepare_fields(vm, o, F_YEAR | F_MONTH | F_DAY, false, true)?;
    let f = merge_fields(&date_fields(d), &partial);
    let ov = vm.arg(ctx, 1);
    let opts = get_options(vm, &ov)?;
    let c = opt_overflow(vm, opts)?;
    let r = date_from_fields(vm, &f, c)?;
    Ok(make(vm, ctx, TData::Date(r)))
}

fn date_difference(vm: &mut Vm, ctx: &CallCtx, since: bool) -> JsResult<Value> {
    let d = this_date(vm, ctx, if since { "since" } else { "until" })?;
    let a = vm.arg(ctx, 0);
    let other = to_date(vm, &a, &Value::Undefined)?;
    let o = vm.arg(ctx, 1);
    let s = difference_settings(vm, since, &o, Group::Date, &[], Unit::Day, Unit::Day)?;
    if d == other {
        return Ok(make(vm, ctx, TData::Duration(Dur::default())));
    }
    let mut dur = IDur { date: date_until(d, other, s.largest), time: 0 };
    if !(s.smallest == Unit::Day && s.inc == 1) {
        let dest = utc_ns(other, Time::MIDNIGHT);
        let r = diff::round_relative(dur, dest, d, Time::MIDNIGHT, None, s.largest, s.inc, s.smallest, s.mode);
        dur = rerr(vm, r)?;
    }
    duration_result(vm, ctx, dur, Unit::Day, since)
}

fn date_until_m(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    date_difference(vm, ctx, false)
}

fn date_since(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    date_difference(vm, ctx, true)
}

fn date_equals(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_date(vm, ctx, "equals")?;
    let a = vm.arg(ctx, 0);
    let o = to_date(vm, &a, &Value::Undefined)?;
    Ok(Value::Bool(d == o))
}

fn date_to_pdt(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_date(vm, ctx, "toPlainDateTime")?;
    let a = vm.arg(ctx, 0);
    let t = if a.is_undefined() { Time::MIDNIGHT } else { to_time(vm, &a, &Value::Undefined)? };
    if !datetime_within_limits(d, t) {
        return vm.throw_range("date-time outside the representable range");
    }
    Ok(make(vm, ctx, TData::DateTime(d, t)))
}

fn date_to_zoned(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_date(vm, ctx, "toZonedDateTime")?;
    let a = vm.arg(ctx, 0);
    let (tz, tv) = if let Value::Object(o) = &a {
        let o = *o;
        let tzl = vm.get(o, &PropertyKey::from_str("timeZone"))?;
        if tzl.is_undefined() {
            (to_time_zone(vm, &a)?, Value::Undefined)
        } else {
            let tz = to_time_zone(vm, &tzl)?;
            let pt = vm.get(o, &PropertyKey::from_str("plainTime"))?;
            (tz, pt)
        }
    } else {
        (to_time_zone(vm, &a)?, Value::Undefined)
    };
    let n = if tv.is_undefined() {
        let r = zone::start_of_day(&tz, d);
        rerr(vm, r)?
    } else {
        let t = to_time(vm, &tv, &Value::Undefined)?;
        if !datetime_within_limits(d, t) {
            return vm.throw_range("date-time outside the representable range");
        }
        let r = zone::epoch_ns_for(&tz, d, t, Disambiguation::Compatible);
        rerr(vm, r)?
    };
    Ok(make(vm, ctx, TData::Zoned(n, tz)))
}

fn date_to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_date(vm, ctx, "toString")?;
    let ov = vm.arg(ctx, 0);
    let o = get_options(vm, &ov)?;
    let show = opt_calendar_name(vm, o)?;
    Ok(Value::str(&format!("{}{}", date_string(d), calendar_annotation(&show))))
}

fn date_to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_date(vm, ctx, "toJSON")?;
    Ok(Value::str(&date_string(d)))
}

// ============================================================================================ PlainTime

fn this_time(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<Time> {
    match this_of(vm, ctx, TIME, m)? {
        TData::Time(t) => Ok(t),
        _ => unreachable!(),
    }
}

fn time_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Temporal.PlainTime requires 'new'");
    }
    let mut v = [0i64; 6];
    for (i, x) in v.iter_mut().enumerate() {
        *x = int_arg(vm, ctx, i, 0.0)?.clamp(-1e15, 1e15) as i64;
    }
    match regulate_time(v, false) {
        Some(t) => construct(vm, ctx, TData::Time(t)),
        None => vm.throw_range("invalid time"),
    }
}

fn time_from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let t = to_time(vm, &a, &o)?;
    Ok(make(vm, ctx, TData::Time(t)))
}

fn time_compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    let x = to_time(vm, &a, &Value::Undefined)?;
    let y = to_time(vm, &b, &Value::Undefined)?;
    Ok(cmp_value(x, y))
}

fn time_add_sub(vm: &mut Vm, ctx: &CallCtx, neg: bool) -> JsResult<Value> {
    let t = this_time(vm, ctx, if neg { "subtract" } else { "add" })?;
    let a = vm.arg(ctx, 0);
    let mut d = to_duration(vm, &a)?;
    if neg {
        d = d.negated();
    }
    let (_, r) = add_time(t, d.internal().time);
    Ok(make(vm, ctx, TData::Time(r)))
}

fn time_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    time_add_sub(vm, ctx, false)
}

fn time_subtract(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    time_add_sub(vm, ctx, true)
}

fn time_with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_time(vm, ctx, "with")?;
    let a = vm.arg(ctx, 0);
    let o = reject_temporal_like(vm, &a)?;
    let partial = time_bag(vm, o, true)?;
    let mut f = Fields::default();
    time_fields(&mut f, t);
    for i in 0..6 {
        if partial[i].is_some() {
            f.time[i] = partial[i];
        }
    }
    let ov = vm.arg(ctx, 1);
    let opts = get_options(vm, &ov)?;
    let c = opt_overflow(vm, opts)?;
    let r = time_from_fields(vm, &f, c)?;
    Ok(make(vm, ctx, TData::Time(r)))
}

fn time_difference(vm: &mut Vm, ctx: &CallCtx, since: bool) -> JsResult<Value> {
    let t = this_time(vm, ctx, if since { "since" } else { "until" })?;
    let a = vm.arg(ctx, 0);
    let other = to_time(vm, &a, &Value::Undefined)?;
    let o = vm.arg(ctx, 1);
    let s = difference_settings(vm, since, &o, Group::Time, &[], Unit::Nanosecond, Unit::Hour)?;
    let r = diff::round_time(other.to_ns() - t.to_ns(), s.inc as i128, s.smallest, s.mode);
    let td = rerr(vm, r)?;
    duration_result(vm, ctx, IDur { date: DateDur::default(), time: td }, s.largest, since)
}

fn time_until(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    time_difference(vm, ctx, false)
}

fn time_since(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    time_difference(vm, ctx, true)
}

fn time_round(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_time(vm, ctx, "round")?;
    let a = vm.arg(ctx, 0);
    let (o, sh) = round_options(vm, &a)?;
    let inc = opt_increment(vm, o)?;
    let mode = opt_rounding_mode(vm, o, Mode::HalfExpand)?;
    let unit = round_smallest(vm, o, &sh, Group::Time, false)?;
    let max = max_increment(unit).unwrap_or(1);
    validate_increment(vm, inc, max, false)?;
    let (_, r) = diff::round_wall_time(t, inc, unit, mode);
    Ok(make(vm, ctx, TData::Time(r)))
}

fn time_equals(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_time(vm, ctx, "equals")?;
    let a = vm.arg(ctx, 0);
    let o = to_time(vm, &a, &Value::Undefined)?;
    Ok(Value::Bool(t == o))
}

fn time_to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_time(vm, ctx, "toString")?;
    let ov = vm.arg(ctx, 0);
    let o = get_options(vm, &ov)?;
    let (p, unit, inc, mode) = to_string_rounding(vm, o)?;
    let (_, r) = diff::round_wall_time(t, inc, unit, mode);
    Ok(Value::str(&time_string(r, p)))
}

fn time_to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_time(vm, ctx, "toJSON")?;
    Ok(Value::str(&time_string(t, None)))
}

// ============================================================================================ PlainDateTime

fn this_pdt(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<(IsoDate, Time)> {
    match this_of(vm, ctx, DATETIME, m)? {
        TData::DateTime(d, t) => Ok((d, t)),
        _ => unreachable!(),
    }
}

fn pdt_value(vm: &mut Vm, ctx: &CallCtx, d: IsoDate, t: Time) -> JsResult<Value> {
    if !datetime_within_limits(d, t) {
        return vm.throw_range("date-time outside the representable range");
    }
    Ok(make(vm, ctx, TData::DateTime(d, t)))
}

fn pdt_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Temporal.PlainDateTime requires 'new'");
    }
    let y = int_arg(vm, ctx, 0, f64::NAN)?;
    let m = int_arg(vm, ctx, 1, f64::NAN)?;
    let dd = int_arg(vm, ctx, 2, f64::NAN)?;
    let mut v = [0i64; 6];
    for (i, x) in v.iter_mut().enumerate() {
        *x = int_arg(vm, ctx, 3 + i, 0.0)?.clamp(-1e15, 1e15) as i64;
    }
    let c = vm.arg(ctx, 9);
    ctor_calendar(vm, &c)?;
    if y.abs() > 1e6 || !is_valid_date(y as i64, m as i64, dd as i64) {
        return vm.throw_range("invalid ISO date");
    }
    let t = match regulate_time(v, false) {
        Some(t) => t,
        None => return vm.throw_range("invalid time"),
    };
    let d = IsoDate::new(y as i64, m as u8, dd as u8);
    if !datetime_within_limits(d, t) {
        return vm.throw_range("date-time outside the representable range");
    }
    construct(vm, ctx, TData::DateTime(d, t))
}

fn pdt_from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let (d, t) = to_date_time(vm, &a, &o)?;
    Ok(make(vm, ctx, TData::DateTime(d, t)))
}

fn pdt_compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    let x = to_date_time(vm, &a, &Value::Undefined)?;
    let y = to_date_time(vm, &b, &Value::Undefined)?;
    Ok(cmp_value(x, y))
}

fn pdt_with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, t) = this_pdt(vm, ctx, "with")?;
    let a = vm.arg(ctx, 0);
    let o = reject_temporal_like(vm, &a)?;
    let partial = prepare_fields(vm, o, F_YEAR | F_MONTH | F_DAY | F_TIME, false, true)?;
    let mut base = date_fields(d);
    time_fields(&mut base, t);
    let f = merge_fields(&base, &partial);
    let ov = vm.arg(ctx, 1);
    let opts = get_options(vm, &ov)?;
    let c = opt_overflow(vm, opts)?;
    let nd = date_from_fields(vm, &f, c)?;
    let nt = time_from_fields(vm, &f, c)?;
    pdt_value(vm, ctx, nd, nt)
}

fn pdt_with_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, _) = this_pdt(vm, ctx, "withPlainTime")?;
    let a = vm.arg(ctx, 0);
    let t = if a.is_undefined() { Time::MIDNIGHT } else { to_time(vm, &a, &Value::Undefined)? };
    pdt_value(vm, ctx, d, t)
}

/// AddDurationToDateTime (§5.5.x).
fn pdt_add_sub(vm: &mut Vm, ctx: &CallCtx, neg: bool) -> JsResult<Value> {
    let (d, t) = this_pdt(vm, ctx, if neg { "subtract" } else { "add" })?;
    let a = vm.arg(ctx, 0);
    let mut dur = to_duration(vm, &a)?;
    if neg {
        dur = dur.negated();
    }
    let ov = vm.arg(ctx, 1);
    let o = get_options(vm, &ov)?;
    let c = opt_overflow(vm, o)?;
    let i = dur.internal_24h();
    let (days, nt) = add_time(t, i.time);
    let nd = match add_date(d, i.date.years, i.date.months, i.date.weeks, days, c) {
        Some(x) => x,
        None => return vm.throw_range("date-time out of range or rejected (overflow: reject)"),
    };
    pdt_value(vm, ctx, nd, nt)
}

fn pdt_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    pdt_add_sub(vm, ctx, false)
}

fn pdt_subtract(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    pdt_add_sub(vm, ctx, true)
}

fn pdt_difference(vm: &mut Vm, ctx: &CallCtx, since: bool) -> JsResult<Value> {
    let (d, t) = this_pdt(vm, ctx, if since { "since" } else { "until" })?;
    let a = vm.arg(ctx, 0);
    let (od, ot) = to_date_time(vm, &a, &Value::Undefined)?;
    let o = vm.arg(ctx, 1);
    let s = difference_settings(vm, since, &o, Group::DateTime, &[], Unit::Nanosecond, Unit::Day)?;
    let r = diff::diff_plain_rounded(d, t, od, ot, s.largest, s.inc, s.smallest, s.mode);
    let i = rerr(vm, r)?;
    duration_result(vm, ctx, i, s.largest, since)
}

fn pdt_until(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    pdt_difference(vm, ctx, false)
}

fn pdt_since(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    pdt_difference(vm, ctx, true)
}

/// The round() options of PlainDateTime and ZonedDateTime: (increment, mode, unit).
pub fn round_dt_options(vm: &mut Vm, a: &Value) -> JsResult<(i64, Mode, Unit)> {
    let (o, sh) = round_options(vm, a)?;
    let inc = opt_increment(vm, o)?;
    let mode = opt_rounding_mode(vm, o, Mode::HalfExpand)?;
    let unit = round_smallest(vm, o, &sh, Group::Time, true)?;
    if unit == Unit::Day {
        validate_increment(vm, inc, 1, true)?;
    } else {
        let max = max_increment(unit).unwrap_or(1);
        validate_increment(vm, inc, max, false)?;
    }
    Ok((inc, mode, unit))
}

fn pdt_round(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, t) = this_pdt(vm, ctx, "round")?;
    let a = vm.arg(ctx, 0);
    let (inc, mode, unit) = round_dt_options(vm, &a)?;
    let (nd, nt) = diff::round_datetime(d, t, inc, unit, mode);
    pdt_value(vm, ctx, nd, nt)
}

fn pdt_equals(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let x = this_pdt(vm, ctx, "equals")?;
    let a = vm.arg(ctx, 0);
    let y = to_date_time(vm, &a, &Value::Undefined)?;
    Ok(Value::Bool(x == y))
}

fn pdt_to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, t) = this_pdt(vm, ctx, "toString")?;
    let ov = vm.arg(ctx, 0);
    let o = get_options(vm, &ov)?;
    let show = opt_calendar_name(vm, o)?;
    let (p, unit, inc, mode) = to_string_rounding(vm, o)?;
    let (nd, nt) = diff::round_datetime(d, t, inc, unit, mode);
    if !datetime_within_limits(nd, nt) {
        return vm.throw_range("rounded date-time outside the representable range");
    }
    Ok(Value::str(&format!("{}{}", datetime_string(nd, nt, p), calendar_annotation(&show))))
}

fn pdt_to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, t) = this_pdt(vm, ctx, "toJSON")?;
    Ok(Value::str(&datetime_string(d, t, None)))
}

fn pdt_to_zoned(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, t) = this_pdt(vm, ctx, "toZonedDateTime")?;
    let a = vm.arg(ctx, 0);
    let tz = to_time_zone(vm, &a)?;
    let ov = vm.arg(ctx, 1);
    let o = get_options(vm, &ov)?;
    let dis = opt_disambiguation(vm, o)?;
    let r = zone::epoch_ns_for(&tz, d, t, dis);
    let n = rerr(vm, r)?;
    Ok(make(vm, ctx, TData::Zoned(n, tz)))
}

fn pdt_to_date(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, _) = this_pdt(vm, ctx, "toPlainDate")?;
    Ok(make(vm, ctx, TData::Date(d)))
}

fn pdt_to_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t) = this_pdt(vm, ctx, "toPlainTime")?;
    Ok(make(vm, ctx, TData::Time(t)))
}

// ============================================================================================ PlainYearMonth

fn this_ym(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<IsoDate> {
    match this_of(vm, ctx, YEARMONTH, m)? {
        TData::YearMonth(d) => Ok(d),
        _ => unreachable!(),
    }
}

/// CalendarYearMonthFromFields for iso8601 (reference day 1).
fn ym_from_fields(vm: &mut Vm, f: &Fields, constrain: bool) -> JsResult<IsoDate> {
    let y = match f.year {
        Some(y) => y as i64,
        None => return vm.throw_type("year is required"),
    };
    let m = resolve_month(vm, f)?;
    let y = y.clamp(-1_000_000, 1_000_000);
    let r = match regulate_date(y, m.min(1_000_000), 1, constrain) {
        Some(r) => r,
        None => return vm.throw_range("month out of range (overflow: reject)"),
    };
    if !year_month_within_limits(r.year, r.month) {
        return vm.throw_range("year-month outside the representable range");
    }
    Ok(r)
}

/// ToTemporalYearMonth.
fn to_year_month(vm: &mut Vm, item: &Value, options: &Value) -> JsResult<IsoDate> {
    if let Value::Object(o) = item {
        let o = *o;
        if let Some(TData::YearMonth(d)) = tdata(vm, item) {
            let opts = get_options(vm, options)?;
            opt_overflow(vm, opts)?;
            return Ok(d);
        }
        bag_calendar(vm, o)?;
        let f = prepare_fields(vm, o, F_YEAR | F_MONTH, false, false)?;
        let opts = get_options(vm, options)?;
        let c = opt_overflow(vm, opts)?;
        return ym_from_fields(vm, &f, c);
    }
    let s = match item {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("cannot convert to a PlainYearMonth"),
    };
    let p = rerr(vm, parse::parse_year_month(&s))?;
    if p.z {
        return vm.throw_range("a UTC designator is not allowed for a plain year-month");
    }
    parsed_calendar(vm, &p)?;
    let opts = get_options(vm, options)?;
    opt_overflow(vm, opts)?;
    if !year_month_within_limits(p.year, p.month) {
        return vm.throw_range("year-month outside the representable range");
    }
    Ok(IsoDate::new(p.year, p.month, 1))
}

fn ym_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Temporal.PlainYearMonth requires 'new'");
    }
    let y = int_arg(vm, ctx, 0, f64::NAN)?;
    let m = int_arg(vm, ctx, 1, f64::NAN)?;
    let c = vm.arg(ctx, 2);
    ctor_calendar(vm, &c)?;
    let rd = int_arg(vm, ctx, 3, 1.0)?;
    if y.abs() > 1e6 || !is_valid_date(y as i64, m as i64, rd as i64) {
        return vm.throw_range("invalid ISO year-month");
    }
    if !year_month_within_limits(y as i64, m as u8) {
        return vm.throw_range("year-month outside the representable range");
    }
    construct(vm, ctx, TData::YearMonth(IsoDate::new(y as i64, m as u8, rd as u8)))
}

fn ym_from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let d = to_year_month(vm, &a, &o)?;
    Ok(make(vm, ctx, TData::YearMonth(d)))
}

fn ym_compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    let x = to_year_month(vm, &a, &Value::Undefined)?;
    let y = to_year_month(vm, &b, &Value::Undefined)?;
    Ok(cmp_value(x, y))
}

fn ym_with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_ym(vm, ctx, "with")?;
    let a = vm.arg(ctx, 0);
    let o = reject_temporal_like(vm, &a)?;
    let partial = prepare_fields(vm, o, F_YEAR | F_MONTH, false, true)?;
    let mut base = date_fields(d);
    base.day = None;
    let f = merge_fields(&base, &partial);
    let ov = vm.arg(ctx, 1);
    let opts = get_options(vm, &ov)?;
    let c = opt_overflow(vm, opts)?;
    let r = ym_from_fields(vm, &f, c)?;
    Ok(make(vm, ctx, TData::YearMonth(r)))
}

/// AddDurationToYearMonth (§9.5.x).
fn ym_add_sub(vm: &mut Vm, ctx: &CallCtx, neg: bool) -> JsResult<Value> {
    let d = this_ym(vm, ctx, if neg { "subtract" } else { "add" })?;
    let a = vm.arg(ctx, 0);
    let mut dur = to_duration(vm, &a)?;
    if neg {
        dur = dur.negated();
    }
    let ov = vm.arg(ctx, 1);
    let o = get_options(vm, &ov)?;
    let c = opt_overflow(vm, o)?;
    let sign = dur.sign();
    let first = IsoDate::new(d.year, d.month, 1);
    let start = if sign < 0 {
        match add_date(first, 0, 1, 0, 0, true) {
            Some(n) => n.add_days(-1),
            None => return vm.throw_range("year-month outside the representable range"),
        }
    } else {
        first
    };
    let dd = date_part(&dur);
    let added = match add_date(start, dd.years, dd.months, dd.weeks, dd.days, c) {
        Some(x) => x,
        None => return vm.throw_range("year-month out of range or rejected (overflow: reject)"),
    };
    let f = Fields { year: Some(added.year as f64), month: Some(added.month as f64), ..Default::default() };
    let r = ym_from_fields(vm, &f, c)?;
    Ok(make(vm, ctx, TData::YearMonth(r)))
}

fn ym_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ym_add_sub(vm, ctx, false)
}

fn ym_subtract(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ym_add_sub(vm, ctx, true)
}

fn ym_difference(vm: &mut Vm, ctx: &CallCtx, since: bool) -> JsResult<Value> {
    let d = this_ym(vm, ctx, if since { "since" } else { "until" })?;
    let a = vm.arg(ctx, 0);
    let other = to_year_month(vm, &a, &Value::Undefined)?;
    let o = vm.arg(ctx, 1);
    let s = difference_settings(vm, since, &o, Group::Date, &[Unit::Week, Unit::Day], Unit::Month, Unit::Year)?;
    let x = IsoDate::new(d.year, d.month, 1);
    let y = IsoDate::new(other.year, other.month, 1);
    if x == y {
        return Ok(make(vm, ctx, TData::Duration(Dur::default())));
    }
    let du = date_until(x, y, s.largest);
    let mut dur = IDur { date: DateDur { years: du.years, months: du.months, weeks: 0, days: 0 }, time: 0 };
    if !(s.smallest == Unit::Month && s.inc == 1) {
        let dest = utc_ns(y, Time::MIDNIGHT);
        let r = diff::round_relative(dur, dest, x, Time::MIDNIGHT, None, s.largest, s.inc, s.smallest, s.mode);
        dur = rerr(vm, r)?;
    }
    duration_result(vm, ctx, dur, Unit::Day, since)
}

fn ym_until(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ym_difference(vm, ctx, false)
}

fn ym_since(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ym_difference(vm, ctx, true)
}

fn ym_equals(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_ym(vm, ctx, "equals")?;
    let a = vm.arg(ctx, 0);
    let o = to_year_month(vm, &a, &Value::Undefined)?;
    Ok(Value::Bool(d == o))
}

fn ym_string(d: IsoDate, show: &str) -> String {
    let mut s = format!("{}-{:02}", pad_year(d.year), d.month);
    if show == "always" || show == "critical" {
        s.push_str(&format!("-{:02}", d.day));
    }
    s.push_str(calendar_annotation(show));
    s
}

fn ym_to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_ym(vm, ctx, "toString")?;
    let ov = vm.arg(ctx, 0);
    let o = get_options(vm, &ov)?;
    let show = opt_calendar_name(vm, o)?;
    Ok(Value::str(&ym_string(d, &show)))
}

fn ym_to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_ym(vm, ctx, "toJSON")?;
    Ok(Value::str(&ym_string(d, "auto")))
}

fn ym_to_date(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_ym(vm, ctx, "toPlainDate")?;
    let a = vm.arg(ctx, 0);
    let o = match a {
        Value::Object(o) => o,
        _ => return vm.throw_type("toPlainDate requires an object with a day"),
    };
    let extra = prepare_fields(vm, o, F_DAY, false, false)?;
    let mut f = date_fields(d);
    f.day = extra.day;
    let r = date_from_fields(vm, &f, true)?;
    Ok(make(vm, ctx, TData::Date(r)))
}

// ============================================================================================ PlainMonthDay

fn this_md(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<IsoDate> {
    match this_of(vm, ctx, MONTHDAY, m)? {
        TData::MonthDay(d) => Ok(d),
        _ => unreachable!(),
    }
}

/// CalendarMonthDayFromFields for iso8601 (reference year 1972).
fn md_from_fields(vm: &mut Vm, f: &Fields, constrain: bool) -> JsResult<IsoDate> {
    if f.month_code.is_none() && f.month.is_some() && f.year.is_none() {
        return vm.throw_type("year is required when month is given without monthCode");
    }
    let m = resolve_month(vm, f)?;
    let d = match f.day {
        Some(d) => d as i64,
        None => return vm.throw_type("day is required"),
    };
    let y = f.year.map(|y| y as i64).unwrap_or(1972).clamp(-1_000_000, 1_000_000);
    let r = match regulate_date(y, m.min(1_000_000), d.min(1_000_000), constrain) {
        Some(r) => r,
        None => return vm.throw_range("month-day out of range (overflow: reject)"),
    };
    Ok(IsoDate::new(1972, r.month, r.day))
}

fn to_month_day(vm: &mut Vm, item: &Value, options: &Value) -> JsResult<IsoDate> {
    if let Value::Object(o) = item {
        let o = *o;
        if let Some(TData::MonthDay(d)) = tdata(vm, item) {
            let opts = get_options(vm, options)?;
            opt_overflow(vm, opts)?;
            return Ok(d);
        }
        bag_calendar(vm, o)?;
        let f = prepare_fields(vm, o, F_YEAR | F_MONTH | F_DAY, false, false)?;
        let opts = get_options(vm, options)?;
        let c = opt_overflow(vm, opts)?;
        return md_from_fields(vm, &f, c);
    }
    let s = match item {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("cannot convert to a PlainMonthDay"),
    };
    let p = rerr(vm, parse::parse_month_day(&s))?;
    if p.z {
        return vm.throw_range("a UTC designator is not allowed for a plain month-day");
    }
    parsed_calendar(vm, &p)?;
    let opts = get_options(vm, options)?;
    opt_overflow(vm, opts)?;
    Ok(IsoDate::new(1972, p.month, p.day))
}

fn md_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Temporal.PlainMonthDay requires 'new'");
    }
    let m = int_arg(vm, ctx, 0, f64::NAN)?;
    let d = int_arg(vm, ctx, 1, f64::NAN)?;
    let c = vm.arg(ctx, 2);
    ctor_calendar(vm, &c)?;
    let ry = int_arg(vm, ctx, 3, 1972.0)?;
    if ry.abs() > 1e6 || !is_valid_date(ry as i64, m as i64, d as i64) {
        return vm.throw_range("invalid ISO month-day");
    }
    let date = IsoDate::new(ry as i64, m as u8, d as u8);
    if !date_within_limits(date) {
        return vm.throw_range("month-day outside the representable range");
    }
    construct(vm, ctx, TData::MonthDay(date))
}

fn md_from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let d = to_month_day(vm, &a, &o)?;
    Ok(make(vm, ctx, TData::MonthDay(d)))
}

fn md_with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_md(vm, ctx, "with")?;
    let a = vm.arg(ctx, 0);
    let o = reject_temporal_like(vm, &a)?;
    let partial = prepare_fields(vm, o, F_YEAR | F_MONTH | F_DAY, false, true)?;
    let base = Fields { month_code: Some(d.month), day: Some(d.day as f64), ..Default::default() };
    let f = merge_fields(&base, &partial);
    let ov = vm.arg(ctx, 1);
    let opts = get_options(vm, &ov)?;
    let c = opt_overflow(vm, opts)?;
    let r = md_from_fields(vm, &f, c)?;
    Ok(make(vm, ctx, TData::MonthDay(r)))
}

fn md_equals(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_md(vm, ctx, "equals")?;
    let a = vm.arg(ctx, 0);
    let o = to_month_day(vm, &a, &Value::Undefined)?;
    Ok(Value::Bool(d == o))
}

fn md_string(d: IsoDate, show: &str) -> String {
    let mut s = String::new();
    if show == "always" || show == "critical" {
        s.push_str(&pad_year(d.year));
        s.push('-');
    }
    s.push_str(&format!("{:02}-{:02}", d.month, d.day));
    s.push_str(calendar_annotation(show));
    s
}

fn md_to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_md(vm, ctx, "toString")?;
    let ov = vm.arg(ctx, 0);
    let o = get_options(vm, &ov)?;
    let show = opt_calendar_name(vm, o)?;
    Ok(Value::str(&md_string(d, &show)))
}

fn md_to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_md(vm, ctx, "toJSON")?;
    Ok(Value::str(&md_string(d, "auto")))
}

fn md_to_date(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_md(vm, ctx, "toPlainDate")?;
    let a = vm.arg(ctx, 0);
    let o = match a {
        Value::Object(o) => o,
        _ => return vm.throw_type("toPlainDate requires an object with a year"),
    };
    let extra = prepare_fields(vm, o, F_YEAR, false, false)?;
    let f = Fields { year: extra.year, month_code: Some(d.month), day: Some(d.day as f64), ..Default::default() };
    let r = date_from_fields(vm, &f, true)?;
    Ok(make(vm, ctx, TData::Date(r)))
}
