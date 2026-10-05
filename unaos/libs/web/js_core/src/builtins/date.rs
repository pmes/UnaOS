//! Date (§21.4): time values, local time through a POSIX TZ rule (or the host), the Date Time String Format
//! parser plus the engine's own toString/toUTCString forms, and Annex B getYear/setYear/toGMTString.

use super::*;
use crate::tz::{day_from_year, is_leap, year_of};

const MS_DAY: f64 = 86_400_000.0;
const MS_HOUR: f64 = 3_600_000.0;
const MS_MIN: f64 = 60_000.0;
const MS_SEC: f64 = 1000.0;

fn floor(x: f64) -> f64 {
    crate::numconv::libm_floor(x)
}
fn trunc(x: f64) -> f64 {
    crate::builtins::math::trunc(x)
}
fn pmod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r < 0.0 { r + b } else { r + 0.0 }
}

pub fn day(t: f64) -> f64 {
    floor(t / MS_DAY)
}
fn time_within_day(t: f64) -> f64 {
    pmod(t, MS_DAY)
}
fn in_leap(t: f64) -> bool {
    is_leap(year_of(t))
}
fn day_within_year(t: f64) -> f64 {
    day(t) - day_from_year(year_of(t))
}
const CUM: [f64; 13] = [0.0, 31.0, 59.0, 90.0, 120.0, 151.0, 181.0, 212.0, 243.0, 273.0, 304.0, 334.0, 365.0];
fn month_start(m: usize, leap: bool) -> f64 {
    CUM[m] + if leap && m >= 2 { 1.0 } else { 0.0 }
}
pub fn month_from_time(t: f64) -> f64 {
    let d = day_within_year(t);
    let leap = in_leap(t);
    let mut m = 0;
    while m < 11 && d >= month_start(m + 1, leap) {
        m += 1;
    }
    m as f64
}
pub fn date_from_time(t: f64) -> f64 {
    let m = month_from_time(t) as usize;
    day_within_year(t) - month_start(m, in_leap(t)) + 1.0
}
pub fn week_day(t: f64) -> f64 {
    pmod(day(t) + 4.0, 7.0)
}
fn hour_from_time(t: f64) -> f64 {
    pmod(floor(t / MS_HOUR), 24.0)
}
fn min_from_time(t: f64) -> f64 {
    pmod(floor(t / MS_MIN), 60.0)
}
fn sec_from_time(t: f64) -> f64 {
    pmod(floor(t / MS_SEC), 60.0)
}
fn ms_from_time(t: f64) -> f64 {
    pmod(t, MS_SEC)
}

fn to_int(x: f64) -> f64 {
    trunc(x) + 0.0
}

pub fn make_time(h: f64, m: f64, s: f64, ms: f64) -> f64 {
    if !h.is_finite() || !m.is_finite() || !s.is_finite() || !ms.is_finite() {
        return f64::NAN;
    }
    to_int(h) * MS_HOUR + to_int(m) * MS_MIN + to_int(s) * MS_SEC + to_int(ms)
}

pub fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !year.is_finite() || !month.is_finite() || !date.is_finite() {
        return f64::NAN;
    }
    let (y, m, dt) = (to_int(year), to_int(month), to_int(date));
    let ym = y + floor(m / 12.0);
    if !ym.is_finite() || ym.abs() > 400_000.0 {
        return f64::NAN;
    }
    let mn = pmod(m, 12.0) as usize;
    let d = day_from_year(ym) + month_start(mn, is_leap(ym));
    d + dt - 1.0
}

pub fn make_date(day: f64, time: f64) -> f64 {
    if !day.is_finite() || !time.is_finite() {
        return f64::NAN;
    }
    let tv = day * MS_DAY + time;
    if !tv.is_finite() { f64::NAN } else { tv }
}

pub fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > 8.64e15 {
        return f64::NAN;
    }
    to_int(t)
}

fn make_full_year(y: f64) -> f64 {
    if y.is_nan() {
        return f64::NAN;
    }
    let t = to_int(y);
    if (0.0..=99.0).contains(&t) { 1900.0 + t } else { t }
}

/// Offset of local time from UTC at UTC time `t` (LocalTZA(t, true)).
fn tz_utc(vm: &mut Vm, t: f64) -> f64 {
    if !t.is_finite() {
        return 0.0;
    }
    match &vm.tz {
        Some(z) => z.offset_at_utc(t),
        None => vm.host.tz_offset_ms(t, true),
    }
}

pub fn local_time(vm: &mut Vm, t: f64) -> f64 {
    t + tz_utc(vm, t)
}

pub fn utc(vm: &mut Vm, t: f64) -> f64 {
    if !t.is_finite() {
        return f64::NAN;
    }
    let off = match &vm.tz {
        Some(z) => z.offset_for_local(t),
        None => vm.host.tz_offset_ms(t, false),
    };
    t - off
}

// ================================================================================================ init

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.new_object(Some(op));
    let c = ctor(vm, "Date", 7, date_ctor, proto);
    method(vm, c, "now", 0, date_now);
    method(vm, c, "parse", 1, date_parse);
    method(vm, c, "UTC", 7, date_utc);
    let getters: [(&str, NativeFn); 20] = [
        ("getDate", get_date),
        ("getDay", get_day),
        ("getFullYear", get_full_year),
        ("getHours", get_hours),
        ("getMilliseconds", get_milliseconds),
        ("getMinutes", get_minutes),
        ("getMonth", get_month),
        ("getSeconds", get_seconds),
        ("getTime", get_time),
        ("getTimezoneOffset", get_timezone_offset),
        ("getUTCDate", get_utc_date),
        ("getUTCDay", get_utc_day),
        ("getUTCFullYear", get_utc_full_year),
        ("getUTCHours", get_utc_hours),
        ("getUTCMilliseconds", get_utc_milliseconds),
        ("getUTCMinutes", get_utc_minutes),
        ("getUTCMonth", get_utc_month),
        ("getUTCSeconds", get_utc_seconds),
        ("getYear", get_year),
        ("valueOf", get_time),
    ];
    for (n, f) in getters {
        method(vm, proto, n, 0, f);
    }
    let setters: [(&str, u32, NativeFn); 16] = [
        ("setDate", 1, set_date),
        ("setFullYear", 3, set_full_year),
        ("setHours", 4, set_hours),
        ("setMilliseconds", 1, set_milliseconds),
        ("setMinutes", 3, set_minutes),
        ("setMonth", 2, set_month),
        ("setSeconds", 2, set_seconds),
        ("setTime", 1, set_time),
        ("setUTCDate", 1, set_utc_date),
        ("setUTCFullYear", 3, set_utc_full_year),
        ("setUTCHours", 4, set_utc_hours),
        ("setUTCMilliseconds", 1, set_utc_milliseconds),
        ("setUTCMinutes", 3, set_utc_minutes),
        ("setUTCMonth", 2, set_utc_month),
        ("setUTCSeconds", 2, set_utc_seconds),
        ("setYear", 1, set_year),
    ];
    for (n, l, f) in setters {
        method(vm, proto, n, l, f);
    }
    for (n, l, f) in [
        ("toDateString", 0, to_date_string as NativeFn),
        ("toISOString", 0, to_iso_string),
        ("toJSON", 1, to_json),
        ("toLocaleDateString", 0, to_date_string),
        ("toLocaleString", 0, to_string),
        ("toLocaleTimeString", 0, to_time_string),
        ("toString", 0, to_string),
        ("toTimeString", 0, to_time_string),
    ] {
        method(vm, proto, n, l, f);
    }
    let utc_fn = method(vm, proto, "toUTCString", 0, to_utc_string);
    vm.heap.get_mut(proto).props.insert(PropertyKey::from_str("toGMTString"), Prop::data(Value::Object(utc_fn), WC));
    let tp = vm.wk.to_primitive.clone();
    method_sym(vm, proto, tp, "[Symbol.toPrimitive]", 1, to_primitive, C);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.date_proto = proto;
    vm.realms[r].intrinsics.date_ctor = c;
    global(vm, "Date", Value::Object(c));
}

fn now(vm: &mut Vm) -> f64 {
    time_clip(vm.host.now_ms())
}

fn date_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        let t = now(vm);
        return Ok(Value::String(JsStr::from_str(&full_string(vm, t))));
    }
    let tv = match ctx.argc {
        0 => now(vm),
        1 => {
            let v = vm.arg(ctx, 0);
            let dv = match &v {
                Value::Object(o) => match vm.heap.get(*o).kind {
                    Kind::Date(t) => Some(t),
                    _ => None,
                },
                _ => None,
            };
            match dv {
                Some(t) => time_clip(t),
                None => {
                    let p = vm.to_primitive(&v, 0)?;
                    match &p {
                        Value::String(s) => parse_date(vm, s),
                        _ => {
                            let n = vm.to_number(&p)?;
                            time_clip(n)
                        }
                    }
                }
            }
        }
        n => {
            let mut nums = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
            for (i, slot) in nums.iter_mut().enumerate().take(n.min(7)) {
                let a = vm.arg(ctx, i);
                *slot = vm.to_number(&a)?;
            }
            let yr = make_full_year(nums[0]);
            let fd = make_date(make_day(yr, nums[1], nums[2]), make_time(nums[3], nums[4], nums[5], nums[6]));
            let u = utc(vm, fd);
            time_clip(u)
        }
    };
    let nt = ctx.new_target.clone();
    let proto = vm.get_prototype_from_ctor(&nt, |i| i.date_proto)?;
    Ok(Value::Object(vm.alloc(ObjectData::new(Some(proto), Kind::Date(tv)))))
}

fn date_now(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Number(now(vm)))
}

fn date_parse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    Ok(Value::Number(parse_date(vm, &s)))
}

fn date_utc(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let mut nums = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    for (i, slot) in nums.iter_mut().enumerate().take(ctx.argc.min(7)) {
        let a = vm.arg(ctx, i);
        *slot = vm.to_number(&a)?;
    }
    let yr = make_full_year(nums[0]);
    Ok(Value::Number(time_clip(make_date(make_day(yr, nums[1], nums[2]), make_time(nums[3], nums[4], nums[5], nums[6])))))
}

// ================================================================================================ getters

fn this_tv(vm: &mut Vm, ctx: &CallCtx) -> JsResult<f64> {
    if let Value::Object(o) = &ctx.this {
        if let Kind::Date(t) = vm.heap.get(*o).kind {
            return Ok(t);
        }
    }
    vm.throw_type("this is not a Date object.")
}

fn set_tv(vm: &mut Vm, ctx: &CallCtx, t: f64) -> Value {
    if let Value::Object(o) = &ctx.this {
        if let Kind::Date(v) = &mut vm.heap.get_mut(*o).kind {
            *v = t;
        }
    }
    Value::Number(t)
}

fn getter(vm: &mut Vm, ctx: &CallCtx, local: bool, f: fn(f64) -> f64) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(vm, t) } else { t };
    Ok(Value::Number(f(t)))
}

macro_rules! getters {
    ($($name:ident, $local:expr, $f:expr;)*) => {
        $(fn $name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> { getter(vm, ctx, $local, $f) })*
    };
}

getters! {
    get_date, true, date_from_time;
    get_day, true, week_day;
    get_full_year, true, year_of;
    get_hours, true, hour_from_time;
    get_milliseconds, true, ms_from_time;
    get_minutes, true, min_from_time;
    get_month, true, month_from_time;
    get_seconds, true, sec_from_time;
    get_utc_date, false, date_from_time;
    get_utc_day, false, week_day;
    get_utc_full_year, false, year_of;
    get_utc_hours, false, hour_from_time;
    get_utc_milliseconds, false, ms_from_time;
    get_utc_minutes, false, min_from_time;
    get_utc_month, false, month_from_time;
    get_utc_seconds, false, sec_from_time;
    get_year, true, |t| year_of(t) - 1900.0;
}

fn get_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Number(this_tv(vm, ctx)?))
}

fn get_timezone_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let lt = local_time(vm, t);
    Ok(Value::Number((t - lt) / MS_MIN))
}

// ================================================================================================ setters

/// Arguments i.. as numbers (only those present), converted in order.
fn nums(vm: &mut Vm, ctx: &CallCtx, max: usize) -> JsResult<[Option<f64>; 4]> {
    let mut out = [None; 4];
    for (i, slot) in out.iter_mut().enumerate().take(max) {
        if i < ctx.argc {
            let a = vm.arg(ctx, i);
            *slot = Some(vm.to_number(&a)?);
        }
    }
    // A missing first argument still converts `undefined` (NaN).
    if ctx.argc == 0 {
        out[0] = Some(f64::NAN);
    }
    Ok(out)
}

fn finish(vm: &mut Vm, ctx: &CallCtx, local: bool, date: f64) -> JsResult<Value> {
    let u = if local { utc(vm, date) } else { date };
    Ok(set_tv(vm, ctx, time_clip(u)))
}

fn set_ms_impl(vm: &mut Vm, ctx: &CallCtx, local: bool) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    let a = nums(vm, ctx, 1)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(vm, t) } else { t };
    let time = make_time(hour_from_time(t), min_from_time(t), sec_from_time(t), a[0].unwrap());
    finish(vm, ctx, local, make_date(day(t), time))
}
fn set_seconds_impl(vm: &mut Vm, ctx: &CallCtx, local: bool) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    let a = nums(vm, ctx, 2)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(vm, t) } else { t };
    let ms = a[1].unwrap_or(ms_from_time(t));
    let time = make_time(hour_from_time(t), min_from_time(t), a[0].unwrap(), ms);
    finish(vm, ctx, local, make_date(day(t), time))
}
fn set_minutes_impl(vm: &mut Vm, ctx: &CallCtx, local: bool) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    let a = nums(vm, ctx, 3)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(vm, t) } else { t };
    let s = a[1].unwrap_or(sec_from_time(t));
    let ms = a[2].unwrap_or(ms_from_time(t));
    let time = make_time(hour_from_time(t), a[0].unwrap(), s, ms);
    finish(vm, ctx, local, make_date(day(t), time))
}
fn set_hours_impl(vm: &mut Vm, ctx: &CallCtx, local: bool) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    let a = nums(vm, ctx, 4)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(vm, t) } else { t };
    let m = a[1].unwrap_or(min_from_time(t));
    let s = a[2].unwrap_or(sec_from_time(t));
    let ms = a[3].unwrap_or(ms_from_time(t));
    let time = make_time(a[0].unwrap(), m, s, ms);
    finish(vm, ctx, local, make_date(day(t), time))
}
fn set_date_impl(vm: &mut Vm, ctx: &CallCtx, local: bool) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    let a = nums(vm, ctx, 1)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(vm, t) } else { t };
    let nd = make_date(make_day(year_of(t), month_from_time(t), a[0].unwrap()), time_within_day(t));
    finish(vm, ctx, local, nd)
}
fn set_month_impl(vm: &mut Vm, ctx: &CallCtx, local: bool) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    let a = nums(vm, ctx, 2)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(vm, t) } else { t };
    let dt = a[1].unwrap_or(date_from_time(t));
    let nd = make_date(make_day(year_of(t), a[0].unwrap(), dt), time_within_day(t));
    finish(vm, ctx, local, nd)
}
fn set_full_year_impl(vm: &mut Vm, ctx: &CallCtx, local: bool) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    let a = nums(vm, ctx, 3)?;
    let t = if t.is_nan() { 0.0 } else if local { local_time(vm, t) } else { t };
    let m = a[1].unwrap_or(month_from_time(t));
    let dt = a[2].unwrap_or(date_from_time(t));
    let nd = make_date(make_day(a[0].unwrap(), m, dt), time_within_day(t));
    finish(vm, ctx, local, nd)
}

macro_rules! setters {
    ($($name:ident, $imp:ident, $local:expr;)*) => {
        $(fn $name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> { $imp(vm, ctx, $local) })*
    };
}
setters! {
    set_milliseconds, set_ms_impl, true;
    set_utc_milliseconds, set_ms_impl, false;
    set_seconds, set_seconds_impl, true;
    set_utc_seconds, set_seconds_impl, false;
    set_minutes, set_minutes_impl, true;
    set_utc_minutes, set_minutes_impl, false;
    set_hours, set_hours_impl, true;
    set_utc_hours, set_hours_impl, false;
    set_date, set_date_impl, true;
    set_utc_date, set_date_impl, false;
    set_month, set_month_impl, true;
    set_utc_month, set_month_impl, false;
    set_full_year, set_full_year_impl, true;
    set_utc_full_year, set_full_year_impl, false;
}

fn set_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_tv(vm, ctx)?;
    let a = vm.arg(ctx, 0);
    let t = vm.to_number(&a)?;
    Ok(set_tv(vm, ctx, time_clip(t)))
}

/// Annex B.2.3.2 Date.prototype.setYear
fn set_year(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_tv(vm, ctx)?;
    let a = vm.arg(ctx, 0);
    let y = vm.to_number(&a)?;
    let t = if t.is_nan() { 0.0 } else { local_time(vm, t) };
    let yyyy = make_full_year(y);
    let d = make_day(yyyy, month_from_time(t), date_from_time(t));
    let date = make_date(d, time_within_day(t));
    let u = utc(vm, date);
    Ok(set_tv(vm, ctx, time_clip(u)))
}

// ================================================================================================ formatting

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn year_str(y: f64) -> String {
    let yi = y as i64;
    if yi >= 0 { alloc::format!("{:04}", yi) } else { alloc::format!("-{:04}", -yi) }
}

fn date_part(t: f64) -> String {
    alloc::format!("{} {} {:02} {}", DAYS[week_day(t) as usize], MONTHS[month_from_time(t) as usize], date_from_time(t) as u32, year_str(year_of(t)))
}

fn time_part(t: f64) -> String {
    alloc::format!("{:02}:{:02}:{:02} GMT", hour_from_time(t) as u32, min_from_time(t) as u32, sec_from_time(t) as u32)
}

fn tz_part(vm: &mut Vm, tv: f64) -> String {
    let off = tz_utc(vm, tv);
    let sign = if off >= 0.0 { '+' } else { '-' };
    let a = off.abs();
    let h = floor(a / MS_HOUR) as u32;
    let m = (floor(a / MS_MIN) as u32) % 60;
    let name = match &vm.tz {
        Some(z) => alloc::string::String::from(z.name_at_utc(tv)),
        None if off == 0.0 => alloc::string::String::from("Coordinated Universal Time"),
        None => alloc::string::String::new(),
    };
    if name.is_empty() { alloc::format!("{}{:02}{:02}", sign, h, m) } else { alloc::format!("{}{:02}{:02} ({})", sign, h, m, name) }
}

fn full_string(vm: &mut Vm, tv: f64) -> String {
    if tv.is_nan() {
        return alloc::string::String::from("Invalid Date");
    }
    let t = local_time(vm, tv);
    alloc::format!("{} {}{}", date_part(t), time_part(t), tz_part(vm, tv))
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let tv = this_tv(vm, ctx)?;
    Ok(Value::String(JsStr::from_str(&full_string(vm, tv))))
}

fn to_date_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let tv = this_tv(vm, ctx)?;
    if tv.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    let t = local_time(vm, tv);
    Ok(Value::String(JsStr::from_str(&date_part(t))))
}

fn to_time_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let tv = this_tv(vm, ctx)?;
    if tv.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    let t = local_time(vm, tv);
    let s = alloc::format!("{}{}", time_part(t), tz_part(vm, tv));
    Ok(Value::String(JsStr::from_str(&s)))
}

fn to_utc_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let tv = this_tv(vm, ctx)?;
    if tv.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    let s = alloc::format!(
        "{}, {:02} {} {} {}",
        DAYS[week_day(tv) as usize],
        date_from_time(tv) as u32,
        MONTHS[month_from_time(tv) as usize],
        year_str(year_of(tv)),
        time_part(tv)
    );
    Ok(Value::String(JsStr::from_str(&s)))
}

fn iso_string(tv: f64) -> String {
    let y = year_of(tv) as i64;
    let ys = if (0..=9999).contains(&y) {
        alloc::format!("{:04}", y)
    } else if y < 0 {
        alloc::format!("-{:06}", -y)
    } else {
        alloc::format!("+{:06}", y)
    };
    alloc::format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        ys,
        month_from_time(tv) as u32 + 1,
        date_from_time(tv) as u32,
        hour_from_time(tv) as u32,
        min_from_time(tv) as u32,
        sec_from_time(tv) as u32,
        ms_from_time(tv) as u32
    )
}

fn to_iso_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let tv = this_tv(vm, ctx)?;
    if !tv.is_finite() {
        return vm.throw_range("Invalid time value");
    }
    Ok(Value::String(JsStr::from_str(&iso_string(tv))))
}

fn to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = vm.to_object(&ctx.this)?;
    let tv = vm.to_primitive(&o, 1)?;
    if let Value::Number(n) = tv {
        if !n.is_finite() {
            return Ok(Value::Null);
        }
    }
    vm.invoke(&o, &PropertyKey::from_str("toISOString"), &[])
}

fn to_primitive(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("Date.prototype[Symbol.toPrimitive] called on non-object"),
    };
    let h = vm.arg(ctx, 0);
    let hint = match &h {
        Value::String(s) if s.eq_str("string") || s.eq_str("default") => 2,
        Value::String(s) if s.eq_str("number") => 1,
        _ => return vm.throw_type("Invalid hint"),
    };
    vm.ordinary_to_primitive(o, hint)
}

// ================================================================================================ parsing

struct Sc<'a> {
    s: &'a [u16],
    i: usize,
}

impl Sc<'_> {
    fn peek(&self) -> Option<u16> {
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c as u16) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn digits(&mut self, n: usize) -> Option<f64> {
        let mut v = 0.0;
        for _ in 0..n {
            let c = self.peek()?;
            if !(0x30..=0x39).contains(&c) {
                return None;
            }
            v = v * 10.0 + (c - 0x30) as f64;
            self.i += 1;
        }
        Some(v)
    }
    fn number(&mut self) -> Option<f64> {
        let start = self.i;
        let mut v = 0.0;
        while let Some(c) = self.peek() {
            if !(0x30..=0x39).contains(&c) {
                break;
            }
            v = v * 10.0 + (c - 0x30) as f64;
            self.i += 1;
        }
        if self.i == start { None } else { Some(v) }
    }
    fn skip_ws(&mut self) {
        while self.peek() == Some(b' ' as u16) {
            self.i += 1;
        }
    }
    fn word(&mut self) -> alloc::string::String {
        let mut w = alloc::string::String::new();
        while let Some(c) = self.peek() {
            if (c as u8 as u16 == c) && (c as u8).is_ascii_alphabetic() {
                w.push(c as u8 as char);
                self.i += 1;
            } else {
                break;
            }
        }
        w
    }
}

/// Date Time String Format (§21.4.1.32): returns NaN when `s` does not conform.
fn parse_iso(vm: &mut Vm, s: &[u16]) -> Option<f64> {
    let mut p = Sc { s, i: 0 };
    let year = match p.peek()? {
        0x2B | 0x2D => {
            let neg = p.peek() == Some(0x2D);
            p.i += 1;
            let y = p.digits(6)?;
            if neg && y == 0.0 {
                return None;
            }
            if neg { -y } else { y }
        }
        _ => p.digits(4)?,
    };
    let (mut month, mut dayv) = (1.0, 1.0);
    if p.eat('-') {
        month = p.digits(2)?;
        if p.eat('-') {
            dayv = p.digits(2)?;
        }
    }
    let (mut h, mut m, mut sec, mut ms) = (0.0, 0.0, 0.0, 0.0);
    let mut has_time = false;
    let mut offset: Option<f64> = None;
    if p.eat('T') {
        has_time = true;
        h = p.digits(2)?;
        if !p.eat(':') {
            return None;
        }
        m = p.digits(2)?;
        if p.eat(':') {
            sec = p.digits(2)?;
            if p.eat('.') {
                let start = p.i;
                let mut frac = 0.0;
                let mut scale = 100.0;
                while let Some(c) = p.peek() {
                    if !(0x30..=0x39).contains(&c) {
                        break;
                    }
                    frac += (c - 0x30) as f64 * scale;
                    scale /= 10.0;
                    p.i += 1;
                }
                if p.i == start {
                    return None;
                }
                ms = floor(frac);
            }
        }
        if p.eat('Z') {
            offset = Some(0.0);
        } else if matches!(p.peek(), Some(0x2B) | Some(0x2D)) {
            let neg = p.peek() == Some(0x2D);
            p.i += 1;
            let oh = p.digits(2)?;
            if !p.eat(':') {
                return None;
            }
            let om = p.digits(2)?;
            if oh > 23.0 || om > 59.0 {
                return None;
            }
            let o = oh * MS_HOUR + om * MS_MIN;
            offset = Some(if neg { -o } else { o });
        }
    } else if p.eat('Z') {
        offset = Some(0.0);
    }
    if p.i != s.len() {
        return None;
    }
    if !(1.0..=12.0).contains(&month) || dayv < 1.0 {
        return None;
    }
    let dim = [31.0, if is_leap(year) { 29.0 } else { 28.0 }, 31.0, 30.0, 31.0, 30.0, 31.0, 31.0, 30.0, 31.0, 30.0, 31.0][month as usize - 1];
    if dayv > dim || h > 24.0 || m > 59.0 || sec > 59.0 || (h == 24.0 && (m > 0.0 || sec > 0.0 || ms > 0.0)) {
        return None;
    }
    let t = make_date(make_day(year, month - 1.0, dayv), make_time(h, m, sec, ms));
    let u = match offset {
        Some(o) => t - o,
        // Date-only forms are UTC; date-time forms without an offset are local time.
        None if !has_time => t,
        None => utc(vm, t),
    };
    Some(time_clip(u))
}

/// The engine's own toString / toUTCString output, plus "Mon DD YYYY [HH:mm[:ss]] [GMT±hhmm]" variants.
fn parse_fallback(vm: &mut Vm, s: &[u16]) -> Option<f64> {
    let mut p = Sc { s, i: 0 };
    p.skip_ws();
    let w = p.word();
    let mut month: Option<f64> = None;
    let dayv;
    if DAYS.iter().any(|d| d.eq_ignore_ascii_case(&w)) {
        p.eat(',');
        p.skip_ws();
    } else if let Some(mi) = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(&w)) {
        month = Some(mi as f64);
        p.skip_ws();
    } else if !w.is_empty() {
        return None;
    }
    if month.is_none() {
        // "Mon DD YYYY" or "DD Mon YYYY"
        if let Some(d) = p.number() {
            dayv = d;
            p.skip_ws();
            let mw = p.word();
            month = Some(MONTHS.iter().position(|m| m.eq_ignore_ascii_case(&mw))? as f64);
        } else {
            let mw = p.word();
            month = Some(MONTHS.iter().position(|m| m.eq_ignore_ascii_case(&mw))? as f64);
            p.skip_ws();
            dayv = p.number()?;
        }
    } else {
        dayv = p.number()?;
    }
    p.skip_ws();
    let neg = p.eat('-');
    let y = p.number()?;
    let year = if neg { -y } else { y };
    p.skip_ws();
    let (mut h, mut mi, mut sec) = (0.0, 0.0, 0.0);
    if let Some(hh) = p.number() {
        h = hh;
        if !p.eat(':') {
            return None;
        }
        mi = p.number()?;
        if p.eat(':') {
            sec = p.number()?;
        }
    }
    p.skip_ws();
    let mut offset = None;
    let save = p.i;
    let z = p.word();
    if z == "GMT" || z == "UTC" || z == "Z" {
        offset = Some(0.0);
        if matches!(p.peek(), Some(0x2B) | Some(0x2D)) {
            let neg = p.peek() == Some(0x2D);
            p.i += 1;
            let hh = p.digits(2)?;
            let mm = p.digits(2)?;
            let o = hh * MS_HOUR + mm * MS_MIN;
            offset = Some(if neg { -o } else { o });
        }
    } else {
        p.i = save;
    }
    p.skip_ws();
    if p.eat('(') {
        while let Some(c) = p.peek() {
            p.i += 1;
            if c == b')' as u16 {
                break;
            }
        }
    }
    p.skip_ws();
    if p.i != s.len() {
        return None;
    }
    let t = make_date(make_day(year, month?, dayv), make_time(h, mi, sec, 0.0));
    let u = match offset {
        Some(o) => t - o,
        None => utc(vm, t),
    };
    Some(time_clip(u))
}

pub fn parse_date(vm: &mut Vm, s: &JsStr) -> f64 {
    let u = s.units();
    if let Some(t) = parse_iso(vm, u) {
        return t;
    }
    parse_fallback(vm, u).unwrap_or(f64::NAN)
}

use alloc::string::String;
