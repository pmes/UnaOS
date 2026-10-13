//! Temporal.Instant (proposal-temporal 2025 §8): an exact time as epoch nanoseconds.

use super::*;

pub fn init(vm: &mut Vm, d: &Def, ns: Obj) {
    let (c, p) = d.class(vm, ns, INSTANT, 1, ctor);
    d.method(vm, c, "from", 1, from);
    d.method(vm, c, "fromEpochMilliseconds", 1, from_epoch_ms);
    d.method(vm, c, "fromEpochNanoseconds", 1, from_epoch_ns);
    d.method(vm, c, "compare", 2, compare);
    d.getter(vm, p, "epochMilliseconds", epoch_ms);
    d.getter(vm, p, "epochNanoseconds", epoch_ns);
    d.method(vm, p, "add", 1, add);
    d.method(vm, p, "subtract", 1, subtract);
    d.method(vm, p, "until", 1, until);
    d.method(vm, p, "since", 1, since);
    d.method(vm, p, "round", 1, round);
    d.method(vm, p, "equals", 1, equals);
    d.method(vm, p, "toString", 0, to_string);
    d.method(vm, p, "toJSON", 0, to_json);
    d.method(vm, p, "toLocaleString", 0, to_locale);
    d.method(vm, p, "valueOf", 0, value_of);
    d.method(vm, p, "toZonedDateTimeISO", 1, to_zoned_iso);
}

fn this_ns(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<i128> {
    match this_of(vm, ctx, INSTANT, m)? {
        TData::Instant(n) => Ok(n),
        _ => unreachable!(),
    }
}

fn checked(vm: &mut Vm, ctx: &CallCtx, n: i128) -> JsResult<Value> {
    if !valid_epoch_ns(n) {
        return vm.throw_range("instant outside the representable range");
    }
    Ok(make(vm, ctx, TData::Instant(n)))
}

fn ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Temporal.Instant requires 'new'");
    }
    let a = vm.arg(ctx, 0);
    let b = vm.to_bigint(&a)?;
    let n = match i128_from_bigint(&b) {
        Some(n) if valid_epoch_ns(n) => n,
        _ => return vm.throw_range("epochNanoseconds outside the representable range"),
    };
    construct(vm, ctx, TData::Instant(n))
}

fn from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let n = to_instant(vm, &a)?;
    Ok(make(vm, ctx, TData::Instant(n)))
}

fn from_epoch_ms(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let ms = vm.to_number(&a)?;
    if !ms.is_finite() || trunc(ms) != ms {
        return vm.throw_range("epochMilliseconds must be an integer");
    }
    if ms.abs() > 8.64e15 {
        return vm.throw_range("instant outside the representable range");
    }
    checked(vm, ctx, ms as i128 * 1_000_000)
}

fn from_epoch_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let b = vm.to_bigint(&a)?;
    match i128_from_bigint(&b) {
        Some(n) => checked(vm, ctx, n),
        None => vm.throw_range("instant outside the representable range"),
    }
}

fn compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    let x = to_instant(vm, &a)?;
    let y = to_instant(vm, &b)?;
    Ok(Value::Number(match x.cmp(&y) {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Greater => 1.0,
        _ => 0.0,
    }))
}

fn epoch_ms(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_ns(vm, ctx, "epochMilliseconds")?;
    Ok(Value::Number(n.div_euclid(1_000_000) as f64))
}

fn epoch_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_ns(vm, ctx, "epochNanoseconds")?;
    Ok(bigint_from_i128(n))
}

/// AddDurationToInstant (§8.5.x).
fn add_sub(vm: &mut Vm, ctx: &CallCtx, sign: f64, m: &str) -> JsResult<Value> {
    let n = this_ns(vm, ctx, m)?;
    let a = vm.arg(ctx, 0);
    let mut d = to_duration(vm, &a)?;
    if sign < 0.0 {
        d = d.negated();
    }
    if d.default_largest().is_date() {
        return vm.throw_range("an Instant can add only hours and smaller units (no years, months, weeks or days)");
    }
    let t = d.internal_24h().time;
    checked(vm, ctx, n + t)
}

fn add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    add_sub(vm, ctx, 1.0, "add")
}

fn subtract(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    add_sub(vm, ctx, -1.0, "subtract")
}

fn difference(vm: &mut Vm, ctx: &CallCtx, since: bool) -> JsResult<Value> {
    let n = this_ns(vm, ctx, if since { "since" } else { "until" })?;
    let a = vm.arg(ctx, 0);
    let other = to_instant(vm, &a)?;
    let o = vm.arg(ctx, 1);
    let s = difference_settings(vm, since, &o, Group::Time, &[], Unit::Nanosecond, Unit::Second)?;
    let r = diff::diff_instant(n, other, s.inc, s.smallest, s.mode);
    let d = rerr(vm, r)?;
    duration_result(vm, ctx, d, s.largest, since)
}

fn until(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    difference(vm, ctx, false)
}

fn since(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    difference(vm, ctx, true)
}

fn round(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_ns(vm, ctx, "round")?;
    let a = vm.arg(ctx, 0);
    let (o, sh) = round_options(vm, &a)?;
    let inc = opt_increment(vm, o)?;
    let mode = opt_rounding_mode(vm, o, Mode::HalfExpand)?;
    let unit = round_smallest(vm, o, &sh, Group::Time, false)?;
    let max = NS_PER_DAY / unit.ns();
    validate_increment(vm, inc, max, true)?;
    let r = round_incr_positive(n, unit.ns() * inc as i128, mode);
    checked(vm, ctx, r)
}

fn equals(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_ns(vm, ctx, "equals")?;
    let a = vm.arg(ctx, 0);
    let o = to_instant(vm, &a)?;
    Ok(Value::Bool(n == o))
}

/// TemporalInstantToString.
pub fn instant_string(n: i128, tz: Option<&TimeZone>, p: Precision) -> String {
    let utc = TimeZone::utc();
    let z = tz.unwrap_or(&utc);
    let off = zone::offset_ns(z, n);
    let (d, t) = datetime_from_ns(n + off as i128);
    let mut s = datetime_string(d, t, p);
    match tz {
        None => s.push('Z'),
        Some(_) => s.push_str(&offset_string(off, true)),
    }
    s
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_ns(vm, ctx, "toString")?;
    let ov = vm.arg(ctx, 0);
    let o = get_options(vm, &ov)?;
    let (p, unit, inc, mode) = to_string_rounding(vm, o)?;
    let tzv = opt_get(vm, o, "timeZone")?;
    let tz = if tzv.is_undefined() { None } else { Some(to_time_zone(vm, &tzv)?) };
    let r = round_incr_positive(n, unit.ns() * inc as i128, mode);
    if !valid_epoch_ns(r) {
        return vm.throw_range("rounded instant outside the representable range");
    }
    Ok(Value::str(&instant_string(r, tz.as_ref(), p)))
}

fn to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_ns(vm, ctx, "toJSON")?;
    Ok(Value::str(&instant_string(n, None, None)))
}

fn to_locale(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_of(vm, ctx, INSTANT, "toLocaleString")?;
    to_locale_string(vm, ctx, &d)
}

fn to_zoned_iso(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_ns(vm, ctx, "toZonedDateTimeISO")?;
    let a = vm.arg(ctx, 0);
    let tz = to_time_zone(vm, &a)?;
    Ok(make(vm, ctx, TData::Zoned(n, tz)))
}
