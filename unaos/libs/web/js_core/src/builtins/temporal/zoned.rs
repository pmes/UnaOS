//! Temporal.ZonedDateTime (proposal-temporal 2025 §6): an exact time in a time zone, on the iso8601 calendar.

use super::plain::{date_getters, round_dt_options, time_getters};
use super::*;

pub fn init(vm: &mut Vm, d: &Def, ns: Obj) {
    let (c, p) = d.class(vm, ns, ZONED, 2, ctor);
    d.method(vm, c, "from", 1, from);
    d.method(vm, c, "compare", 2, compare);
    date_getters::<ZONED>(vm, d, p);
    time_getters::<ZONED>(vm, d, p);
    d.getter(vm, p, "timeZoneId", time_zone_id);
    d.getter(vm, p, "epochMilliseconds", epoch_ms);
    d.getter(vm, p, "epochNanoseconds", epoch_ns);
    d.getter(vm, p, "hoursInDay", hours_in_day);
    d.getter(vm, p, "offsetNanoseconds", offset_nanoseconds);
    d.getter(vm, p, "offset", offset);
    d.method(vm, p, "with", 1, with);
    d.method(vm, p, "withPlainTime", 0, with_plain_time);
    d.method(vm, p, "withTimeZone", 1, with_time_zone);
    d.method(vm, p, "withCalendar", 1, with_calendar);
    d.method(vm, p, "add", 1, add);
    d.method(vm, p, "subtract", 1, subtract);
    d.method(vm, p, "until", 1, until);
    d.method(vm, p, "since", 1, since);
    d.method(vm, p, "round", 1, round);
    d.method(vm, p, "equals", 1, equals);
    d.method(vm, p, "toString", 0, to_string);
    d.method(vm, p, "toLocaleString", 0, to_locale);
    d.method(vm, p, "toJSON", 0, to_json);
    d.method(vm, p, "valueOf", 0, value_of);
    d.method(vm, p, "startOfDay", 0, start_of_day);
    d.method(vm, p, "getTimeZoneTransition", 1, get_transition);
    d.method(vm, p, "toInstant", 0, to_instant_m);
    d.method(vm, p, "toPlainDate", 0, to_plain_date);
    d.method(vm, p, "toPlainTime", 0, to_plain_time);
    d.method(vm, p, "toPlainDateTime", 0, to_plain_date_time);
}

fn this_z(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<(i128, TimeZone)> {
    match this_of(vm, ctx, ZONED, m)? {
        TData::Zoned(n, tz) => Ok((n, tz)),
        _ => unreachable!(),
    }
}

fn zoned_value(vm: &mut Vm, ctx: &CallCtx, n: i128, tz: TimeZone) -> JsResult<Value> {
    if !valid_epoch_ns(n) {
        return vm.throw_range("instant outside the representable range");
    }
    Ok(make(vm, ctx, TData::Zoned(n, tz)))
}

fn ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Temporal.ZonedDateTime requires 'new'");
    }
    let a = vm.arg(ctx, 0);
    let b = vm.to_bigint(&a)?;
    let n = match i128_from_bigint(&b) {
        Some(n) if valid_epoch_ns(n) => n,
        _ => return vm.throw_range("epochNanoseconds outside the representable range"),
    };
    let tzv = vm.arg(ctx, 1);
    let s = match &tzv {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("time zone must be a string"),
    };
    let tz = match zone::parse_identifier(&s) {
        Some(z) => z,
        None => return vm.throw_range(&format!("unknown time zone: {s}")),
    };
    let c = vm.arg(ctx, 2);
    ctor_calendar(vm, &c)?;
    construct(vm, ctx, TData::Zoned(n, tz))
}

fn from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let (n, tz) = to_zoned(vm, &a, &o)?;
    Ok(make(vm, ctx, TData::Zoned(n, tz)))
}

fn compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    let (x, _) = to_zoned(vm, &a, &Value::Undefined)?;
    let (y, _) = to_zoned(vm, &b, &Value::Undefined)?;
    Ok(Value::Number(match x.cmp(&y) {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Greater => 1.0,
        _ => 0.0,
    }))
}

fn time_zone_id(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, tz) = this_z(vm, ctx, "timeZoneId")?;
    Ok(Value::str(&tz.id))
}

fn epoch_ms(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, _) = this_z(vm, ctx, "epochMilliseconds")?;
    Ok(Value::Number(n.div_euclid(1_000_000) as f64))
}

fn epoch_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, _) = this_z(vm, ctx, "epochNanoseconds")?;
    Ok(bigint_from_i128(n))
}

fn hours_in_day(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "hoursInDay")?;
    let (d, _) = zone::local(&tz, n);
    let a = zone::start_of_day(&tz, d);
    let a = rerr(vm, a)?;
    let b = zone::start_of_day(&tz, d.add_days(1));
    let b = rerr(vm, b)?;
    Ok(Value::Number(diff::total_time(b - a, Unit::Hour)))
}

fn offset_nanoseconds(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "offsetNanoseconds")?;
    Ok(Value::Number(zone::offset_ns(&tz, n) as f64))
}

fn offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "offset")?;
    Ok(Value::str(&offset_string(zone::offset_ns(&tz, n), false)))
}

fn with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "with")?;
    let a = vm.arg(ctx, 0);
    let o = reject_temporal_like(vm, &a)?;
    let (d, t) = zone::local(&tz, n);
    let mut base = date_fields(d);
    time_fields(&mut base, t);
    base.offset = Some(offset_string(zone::offset_ns(&tz, n), false));
    let partial = prepare_fields(vm, o, F_YEAR | F_MONTH | F_DAY | F_TIME | F_OFFSET, false, true)?;
    let f = merge_fields(&base, &partial);
    let ov = vm.arg(ctx, 1);
    let opts = get_options(vm, &ov)?;
    let dis = opt_disambiguation(vm, opts)?;
    let oopt = opt_offset(vm, opts, "prefer")?;
    let c = opt_overflow(vm, opts)?;
    let nd = date_from_fields(vm, &f, c)?;
    let nt = time_from_fields(vm, &f, c)?;
    let off = f.offset.as_deref().and_then(|s| parse::parse_offset_string(s).ok()).map(|o| o.ns).unwrap_or(0);
    let r = interpret_offset(vm, nd, Some(nt), OffsetBehaviour::Option, off, &tz, dis, oopt, false)?;
    zoned_value(vm, ctx, r, tz)
}

fn with_plain_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "withPlainTime")?;
    let a = vm.arg(ctx, 0);
    let (d, _) = zone::local(&tz, n);
    let r = if a.is_undefined() {
        zone::start_of_day(&tz, d)
    } else {
        let t = to_time(vm, &a, &Value::Undefined)?;
        zone::epoch_ns_for(&tz, d, t, Disambiguation::Compatible)
    };
    let r = rerr(vm, r)?;
    zoned_value(vm, ctx, r, tz)
}

fn with_time_zone(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, _) = this_z(vm, ctx, "withTimeZone")?;
    let a = vm.arg(ctx, 0);
    let tz = to_time_zone(vm, &a)?;
    zoned_value(vm, ctx, n, tz)
}

fn with_calendar(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "withCalendar")?;
    let a = vm.arg(ctx, 0);
    if a.is_undefined() {
        return vm.throw_type("withCalendar requires a calendar");
    }
    to_calendar(vm, &a)?;
    zoned_value(vm, ctx, n, tz)
}

fn add_sub(vm: &mut Vm, ctx: &CallCtx, neg: bool) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, if neg { "subtract" } else { "add" })?;
    let a = vm.arg(ctx, 0);
    let mut d = to_duration(vm, &a)?;
    if neg {
        d = d.negated();
    }
    let ov = vm.arg(ctx, 1);
    let o = get_options(vm, &ov)?;
    let c = opt_overflow(vm, o)?;
    let r = diff::add_zoned(n, &tz, &d.internal(), c);
    let r = rerr(vm, r)?;
    zoned_value(vm, ctx, r, tz)
}

fn add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    add_sub(vm, ctx, false)
}

fn subtract(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    add_sub(vm, ctx, true)
}

fn difference(vm: &mut Vm, ctx: &CallCtx, since: bool) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, if since { "since" } else { "until" })?;
    let a = vm.arg(ctx, 0);
    let (other, otz) = to_zoned(vm, &a, &Value::Undefined)?;
    let o = vm.arg(ctx, 1);
    let s = difference_settings(vm, since, &o, Group::DateTime, &[], Unit::Nanosecond, Unit::Hour)?;
    if !s.largest.is_date() {
        let r = diff::diff_instant(n, other, s.inc, s.smallest, s.mode);
        let i = rerr(vm, r)?;
        return duration_result(vm, ctx, i, s.largest, since);
    }
    if !tz.equals(&otz) {
        return vm.throw_range("a difference in calendar units needs both ZonedDateTimes in the same time zone");
    }
    if n == other {
        return Ok(make(vm, ctx, TData::Duration(Dur::default())));
    }
    let r = diff::diff_zoned_rounded(n, other, &tz, s.largest, s.inc, s.smallest, s.mode);
    let i = rerr(vm, r)?;
    duration_result(vm, ctx, i, Unit::Hour, since)
}

fn until(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    difference(vm, ctx, false)
}

fn since(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    difference(vm, ctx, true)
}

fn round(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "round")?;
    let a = vm.arg(ctx, 0);
    let (inc, mode, unit) = round_dt_options(vm, &a)?;
    if unit == Unit::Nanosecond && inc == 1 {
        return zoned_value(vm, ctx, n, tz);
    }
    let (d, t) = zone::local(&tz, n);
    let r = if unit == Unit::Day {
        let s = zone::start_of_day(&tz, d);
        let s = rerr(vm, s)?;
        let e = zone::start_of_day(&tz, d.add_days(1));
        let e = rerr(vm, e)?;
        let len = e - s;
        let prog = round_incr(n - s, len, mode);
        s + prog
    } else {
        let (rd, rt) = diff::round_datetime(d, t, inc, unit, mode);
        let off = zone::offset_ns(&tz, n);
        interpret_offset(vm, rd, Some(rt), OffsetBehaviour::Option, off, &tz, Disambiguation::Compatible, OffsetOpt::Prefer, false)?
    };
    zoned_value(vm, ctx, r, tz)
}

fn equals(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "equals")?;
    let a = vm.arg(ctx, 0);
    let (m, otz) = to_zoned(vm, &a, &Value::Undefined)?;
    Ok(Value::Bool(n == m && tz.equals(&otz)))
}

/// TemporalZonedDateTimeToString.
fn zdt_string(n: i128, tz: &TimeZone, p: Precision, show_cal: &str, show_tz: &str, show_off: &str) -> String {
    let off = zone::offset_ns(tz, n);
    let (d, t) = datetime_from_ns(n + off as i128);
    let mut s = datetime_string(d, t, p);
    if show_off != "never" {
        s.push_str(&offset_string(off, true));
    }
    match show_tz {
        "never" => {}
        "critical" => s.push_str(&format!("[!{}]", tz.id)),
        _ => s.push_str(&format!("[{}]", tz.id)),
    }
    s.push_str(calendar_annotation(show_cal));
    s
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "toString")?;
    let ov = vm.arg(ctx, 0);
    let o = get_options(vm, &ov)?;
    let show_cal = opt_calendar_name(vm, o)?;
    let fsd = opt_fsd(vm, o)?;
    let show_off = opt_str(vm, o, "offset", &["auto", "never"], "auto")?;
    let mode = opt_rounding_mode(vm, o, Mode::Trunc)?;
    let su = opt_unit(vm, o, "smallestUnit", Group::Time, false, false, false)?;
    if su == UnitOpt::U(Unit::Hour) {
        return vm.throw_range("smallestUnit hour is not allowed for toString");
    }
    let show_tz = opt_str(vm, o, "timeZoneName", &["auto", "never", "critical"], "auto")?;
    let (p, unit, inc) = seconds_precision(su, fsd);
    let r = round_incr_positive(n, unit.ns() * inc as i128, mode);
    if !valid_epoch_ns(r) {
        return vm.throw_range("rounded instant outside the representable range");
    }
    Ok(Value::str(&zdt_string(r, &tz, p, &show_cal, &show_tz, &show_off)))
}

fn to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "toJSON")?;
    Ok(Value::str(&zdt_string(n, &tz, None, "auto", "auto", "auto")))
}

fn to_locale(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "toLocaleString")?;
    let l = vm.arg(ctx, 0);
    let ov = vm.arg(ctx, 1);
    if let Value::Object(o) = &ov {
        let o = *o;
        let z = vm.get(o, &PropertyKey::from_str("timeZone"))?;
        if !z.is_undefined() {
            return vm.throw_type("ZonedDateTime.prototype.toLocaleString does not accept a timeZone option");
        }
    }
    let f = crate::builtins::intl::datetime::create(vm, &l, &ov, "any", "all")?;
    let mut g = f.clone();
    if let Some((rule, id)) = crate::builtins::intl::datetime::parse_zone(&tz.id) {
        g.zone = rule;
        g.zone_id = id;
    }
    if !f.explicit {
        g.weekday = None;
        g.era = None;
        g.year = Some("numeric");
        g.month = Some("numeric");
        g.day = Some("numeric");
        g.day_period = None;
        g.hour = Some("numeric");
        g.minute = Some("numeric");
        g.second = Some("numeric");
        g.fsd = None;
        g.tzn = Some("short");
        if g.hc.is_none() {
            g.hc = Some(if g.gb { "h23" } else { "h12" });
        }
    }
    let ms = n.div_euclid(1_000_000) as f64;
    let off = zone::offset_ns(&tz, n) as f64 / 1_000_000.0;
    let parts = crate::builtins::intl::datetime::format_parts(&g, ms, off);
    Ok(Value::str(&crate::builtins::intl::concat_parts(&parts)))
}

fn start_of_day(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "startOfDay")?;
    let (d, _) = zone::local(&tz, n);
    let r = zone::start_of_day(&tz, d);
    let r = rerr(vm, r)?;
    zoned_value(vm, ctx, r, tz)
}

fn get_transition(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "getTimeZoneTransition")?;
    let a = vm.arg(ctx, 0);
    let dir = match &a {
        Value::Undefined => return vm.throw_type("getTimeZoneTransition requires a direction"),
        Value::String(s) => s.to_rust(),
        _ => {
            let o = get_options(vm, &a)?;
            let v = opt_get(vm, o, "direction")?;
            if v.is_undefined() {
                return vm.throw_range("direction is required");
            }
            vm.to_string(&v)?.to_rust()
        }
    };
    let t = match dir.as_str() {
        "next" => zone::next_transition(&tz, n),
        "previous" => zone::previous_transition(&tz, n),
        _ => return vm.throw_range(&format!("{dir} is not a valid direction")),
    };
    match t {
        Some(t) => zoned_value(vm, ctx, t, tz),
        None => Ok(Value::Null),
    }
}

fn to_instant_m(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, _) = this_z(vm, ctx, "toInstant")?;
    Ok(make(vm, ctx, TData::Instant(n)))
}

fn to_plain_date(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "toPlainDate")?;
    Ok(make(vm, ctx, TData::Date(zone::local(&tz, n).0)))
}

fn to_plain_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "toPlainTime")?;
    Ok(make(vm, ctx, TData::Time(zone::local(&tz, n).1)))
}

fn to_plain_date_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (n, tz) = this_z(vm, ctx, "toPlainDateTime")?;
    let (d, t) = zone::local(&tz, n);
    Ok(make(vm, ctx, TData::DateTime(d, t)))
}
