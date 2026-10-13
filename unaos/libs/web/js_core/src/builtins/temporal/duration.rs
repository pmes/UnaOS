//! Temporal.Duration (proposal-temporal 2025 §7): ten integral fields, one sign, balanced and rounded
//! exactly (`iso::IDur`), relative to a PlainDate or ZonedDateTime for the calendar units.

use super::*;

const FIELDS: [&str; 10] = ["years", "months", "weeks", "days", "hours", "minutes", "seconds", "milliseconds", "microseconds", "nanoseconds"];

pub fn init(vm: &mut Vm, d: &Def, ns: Obj) {
    let (c, p) = d.class(vm, ns, DURATION, 0, ctor);
    d.method(vm, c, "from", 1, from);
    d.method(vm, c, "compare", 2, compare);
    let getters: [NativeFn; 10] = [field::<0>, field::<1>, field::<2>, field::<3>, field::<4>, field::<5>, field::<6>, field::<7>, field::<8>, field::<9>];
    for (i, g) in getters.iter().enumerate() {
        d.getter(vm, p, FIELDS[i], *g);
    }
    d.getter(vm, p, "sign", sign);
    d.getter(vm, p, "blank", blank);
    d.method(vm, p, "with", 1, with);
    d.method(vm, p, "negated", 0, negated);
    d.method(vm, p, "abs", 0, abs);
    d.method(vm, p, "add", 1, add);
    d.method(vm, p, "subtract", 1, subtract);
    d.method(vm, p, "round", 1, round);
    d.method(vm, p, "total", 1, total);
    d.method(vm, p, "toString", 0, to_string);
    d.method(vm, p, "toJSON", 0, to_json);
    d.method(vm, p, "toLocaleString", 0, to_locale);
    d.method(vm, p, "valueOf", 0, value_of);
}

fn this_d(vm: &mut Vm, ctx: &CallCtx, m: &str) -> JsResult<Dur> {
    match this_of(vm, ctx, DURATION, m)? {
        TData::Duration(d) => Ok(d),
        _ => unreachable!(),
    }
}

fn result(vm: &mut Vm, ctx: &CallCtx, d: Dur) -> JsResult<Value> {
    if !d.is_valid() {
        return vm.throw_range("invalid duration (mixed signs or out of range)");
    }
    Ok(make(vm, ctx, TData::Duration(d)))
}

fn ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Temporal.Duration requires 'new'");
    }
    let mut d = Dur::default();
    for i in 0..10 {
        let v = vm.arg(ctx, i);
        d.f[i] = if v.is_undefined() { 0.0 } else { to_int_if_integral(vm, &v)? };
    }
    if !d.is_valid() {
        return vm.throw_range("invalid duration (mixed signs or out of range)");
    }
    construct(vm, ctx, TData::Duration(d))
}

fn from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let d = to_duration(vm, &a)?;
    Ok(make(vm, ctx, TData::Duration(d)))
}

/// DateDurationDays: the calendar units of `d` as days from a plain date.
fn date_duration_days(vm: &mut Vm, d: &DateDur, rel: IsoDate) -> JsResult<i128> {
    if d.years == 0 && d.months == 0 && d.weeks == 0 {
        return Ok(d.days as i128);
    }
    let later = match add_date(rel, d.years, d.months, d.weeks, 0, true) {
        Some(x) => x,
        None => return vm.throw_range("relativeTo arithmetic out of range"),
    };
    Ok((later.epoch_days() - rel.epoch_days()) as i128 + d.days as i128)
}

fn compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    let one = to_duration(vm, &a)?;
    let two = to_duration(vm, &b)?;
    let ov = vm.arg(ctx, 2);
    let o = get_options(vm, &ov)?;
    let (plain, zoned) = relative_to(vm, o)?;
    if one == two {
        return Ok(Value::Number(0.0));
    }
    let (l1, l2) = (one.default_largest(), two.default_largest());
    let (d1, d2) = (one.internal(), two.internal());
    let cmp = |x: i128, y: i128| Value::Number(if x < y { -1.0 } else if x > y { 1.0 } else { 0.0 });
    if let Some((n, tz)) = &zoned {
        if l1.is_date() || l2.is_date() {
            let r1 = diff::add_zoned(*n, tz, &d1, true);
            let a1 = rerr(vm, r1)?;
            let r2 = diff::add_zoned(*n, tz, &d2, true);
            let a2 = rerr(vm, r2)?;
            return Ok(cmp(a1, a2));
        }
    }
    let (days1, days2) = if l1.is_calendar() || l2.is_calendar() {
        let rel = match plain {
            Some(p) => p,
            None => match &zoned {
                Some((n, tz)) => zone::local(tz, *n).0,
                None => return vm.throw_range("comparing durations with years, months or weeks needs relativeTo"),
            },
        };
        (date_duration_days(vm, &d1.date, rel)?, date_duration_days(vm, &d2.date, rel)?)
    } else {
        (one.f[3] as i128, two.f[3] as i128)
    };
    let t1 = d1.time + days1 * NS_PER_DAY;
    let t2 = d2.time + days2 * NS_PER_DAY;
    Ok(cmp(t1, t2))
}

fn field<const I: usize>(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, FIELDS[I])?;
    Ok(Value::Number(d.f[I]))
}

fn sign(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "sign")?;
    Ok(Value::Number(d.sign() as f64))
}

fn blank(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "blank")?;
    Ok(Value::Bool(d.sign() == 0))
}

fn with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "with")?;
    let a = vm.arg(ctx, 0);
    let o = match a {
        Value::Object(o) => o,
        _ => return vm.throw_type("with() requires a duration-like object"),
    };
    let p = partial_duration(vm, o)?;
    let mut r = d;
    for i in 0..10 {
        if let Some(v) = p[i] {
            r.f[i] = v;
        }
    }
    result(vm, ctx, r)
}

fn negated(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "negated")?;
    Ok(make(vm, ctx, TData::Duration(d.negated())))
}

fn abs(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "abs")?;
    let mut r = d;
    for v in r.f.iter_mut() {
        *v = v.abs();
    }
    Ok(make(vm, ctx, TData::Duration(r)))
}

/// AddDurations (§7.5.x).
fn add_sub(vm: &mut Vm, ctx: &CallCtx, neg: bool) -> JsResult<Value> {
    let d = this_d(vm, ctx, if neg { "subtract" } else { "add" })?;
    let a = vm.arg(ctx, 0);
    let mut o = to_duration(vm, &a)?;
    if neg {
        o = o.negated();
    }
    let largest = larger(d.default_largest(), o.default_largest());
    if largest.is_calendar() {
        return vm.throw_range("adding durations with years, months or weeks needs a relative date; use add() on a date");
    }
    let t = d.internal_24h().time + o.internal_24h().time;
    if t.abs() > MAX_TIME_DURATION {
        return vm.throw_range("duration out of range");
    }
    let r = from_internal(IDur { date: DateDur::default(), time: t }, largest);
    match r {
        Some(r) => Ok(make(vm, ctx, TData::Duration(r))),
        None => vm.throw_range("duration out of range"),
    }
}

fn add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    add_sub(vm, ctx, false)
}

fn subtract(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    add_sub(vm, ctx, true)
}

/// Temporal.Duration.prototype.round (§7.3.x).
fn round(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "round")?;
    let a = vm.arg(ctx, 0);
    let (o, sh) = round_options(vm, &a)?;
    let lu = if sh.is_some() { UnitOpt::Unset } else { opt_unit(vm, o, "largestUnit", Group::DateTime, false, true, false)? };
    let (plain, zoned) = if sh.is_some() { (None, None) } else { relative_to(vm, o)? };
    let inc = opt_increment(vm, o)?;
    let mode = opt_rounding_mode(vm, o, Mode::HalfExpand)?;
    let su = match &sh {
        Some(s) => match Unit::parse(s) {
            Some(u) => UnitOpt::U(u),
            None => return vm.throw_range(&format!("{s} is not a valid unit")),
        },
        None => opt_unit(vm, o, "smallestUnit", Group::DateTime, false, false, false)?,
    };
    let (smallest, smallest_present) = match su {
        UnitOpt::U(u) => (u, true),
        _ => (Unit::Nanosecond, false),
    };
    let existing = d.default_largest();
    let default_largest = larger(existing, smallest);
    let (largest, largest_present) = match lu {
        UnitOpt::U(u) => (u, true),
        UnitOpt::Auto => (default_largest, true),
        UnitOpt::Unset => (default_largest, false),
    };
    if !smallest_present && !largest_present {
        return vm.throw_range("round() needs smallestUnit or largestUnit");
    }
    if larger(largest, smallest) != largest {
        return vm.throw_range("largestUnit must be at least as large as smallestUnit");
    }
    if let Some(max) = max_increment(smallest) {
        validate_increment(vm, inc, max, false)?;
    }
    if inc > 1 && largest != smallest && smallest.is_date() {
        return vm.throw_range("a roundingIncrement above 1 on a date unit needs largestUnit equal to smallestUnit");
    }
    if let Some((n, tz)) = zoned {
        let i = d.internal();
        let r = diff::add_zoned(n, &tz, &i, true);
        let target = rerr(vm, r)?;
        let r = diff::diff_zoned_rounded(n, target, &tz, largest, inc, smallest, mode);
        let i = rerr(vm, r)?;
        let lu2 = if largest.is_date() { Unit::Hour } else { largest };
        return duration_result(vm, ctx, i, lu2, false);
    }
    if let Some(rel) = plain {
        let i = d.internal_24h();
        let (days, tt) = add_time(Time::MIDNIGHT, i.time);
        let target = match add_date(rel, i.date.years, i.date.months, i.date.weeks, i.date.days + days, true) {
            Some(x) => x,
            None => return vm.throw_range("relativeTo arithmetic out of range"),
        };
        let r = diff::diff_plain_rounded(rel, Time::MIDNIGHT, target, tt, largest, inc, smallest, mode);
        let i = rerr(vm, r)?;
        return duration_result(vm, ctx, i, largest, false);
    }
    if existing.is_calendar() || largest.is_calendar() {
        return vm.throw_range("rounding a duration with years, months or weeks needs relativeTo");
    }
    let i = d.internal_24h();
    let out = if smallest == Unit::Day {
        let r = diff::round_time_incr(i.time, inc as i128 * NS_PER_DAY, mode);
        let t = rerr(vm, r)?;
        IDur { date: DateDur { days: (t / NS_PER_DAY) as i64, ..Default::default() }, time: 0 }
    } else {
        let r = diff::round_time(i.time, inc as i128, smallest, mode);
        let t = rerr(vm, r)?;
        IDur { date: DateDur::default(), time: t }
    };
    duration_result(vm, ctx, out, largest, false)
}

/// Temporal.Duration.prototype.total (§7.3.x).
fn total(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "total")?;
    let a = vm.arg(ctx, 0);
    let (o, sh) = match &a {
        Value::Undefined => return vm.throw_type("total() requires an options object or a unit string"),
        Value::String(s) => (None, Some(s.to_rust())),
        _ => (get_options(vm, &a)?, None),
    };
    let (plain, zoned) = if sh.is_some() { (None, None) } else { relative_to(vm, o)? };
    let unit = match &sh {
        Some(s) => match Unit::parse(s) {
            Some(u) => u,
            None => return vm.throw_range(&format!("{s} is not a valid unit")),
        },
        None => match opt_unit(vm, o, "unit", Group::DateTime, true, false, false)? {
            UnitOpt::U(u) => u,
            _ => return vm.throw_range("unit is required"),
        },
    };
    let r = if let Some((n, tz)) = zoned {
        let i = d.internal();
        let t = diff::add_zoned(n, &tz, &i, true);
        let target = rerr(vm, t)?;
        diff::diff_zoned_total(n, target, &tz, unit)
    } else if let Some(rel) = plain {
        let i = d.internal_24h();
        let (days, tt) = add_time(Time::MIDNIGHT, i.time);
        let target = match add_date(rel, i.date.years, i.date.months, i.date.weeks, i.date.days + days, true) {
            Some(x) => x,
            None => return vm.throw_range("relativeTo arithmetic out of range"),
        };
        diff::diff_plain_total(rel, Time::MIDNIGHT, target, tt, unit)
    } else {
        if d.default_largest().is_calendar() || unit.is_calendar() {
            return vm.throw_range("a total in or of years, months or weeks needs relativeTo");
        }
        Ok(diff::total_time(d.internal_24h().time, unit))
    };
    let v = rerr(vm, r)?;
    Ok(Value::Number(v))
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "toString")?;
    let ov = vm.arg(ctx, 0);
    let o = get_options(vm, &ov)?;
    let fsd = opt_fsd(vm, o)?;
    let mode = opt_rounding_mode(vm, o, Mode::Trunc)?;
    let su = opt_unit(vm, o, "smallestUnit", Group::Time, false, false, false)?;
    if matches!(su, UnitOpt::U(Unit::Hour) | UnitOpt::U(Unit::Minute)) {
        return vm.throw_range("smallestUnit must be seconds or smaller for Duration.toString");
    }
    let (p, unit, inc) = seconds_precision(su, fsd);
    if unit == Unit::Nanosecond && inc == 1 {
        return Ok(Value::str(&duration_string(&d, p)));
    }
    let largest = d.default_largest();
    let i = d.internal();
    let r = diff::round_time(i.time, inc as i128, unit, mode);
    let t = rerr(vm, r)?;
    let rl = larger(largest, Unit::Second);
    match from_internal(IDur { date: i.date, time: t }, rl) {
        Some(r) => Ok(Value::str(&duration_string(&r, p))),
        None => vm.throw_range("rounded duration out of range"),
    }
}

fn to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "toJSON")?;
    Ok(Value::str(&duration_string(&d, None)))
}

fn to_locale(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_d(vm, ctx, "toLocaleString")?;
    let l = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let f = crate::builtins::intl::duration::create(vm, &l, &o)?;
    let (parts, _) = crate::builtins::intl::duration::parts(&f, &d.f);
    Ok(Value::str(&crate::builtins::intl::concat_parts(&parts)))
}
