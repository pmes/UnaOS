//! Temporal (TC39 proposal-temporal, Stage 3, the 2025 specification text): `Temporal.Instant`,
//! `ZonedDateTime`, `PlainDate`, `PlainTime`, `PlainDateTime`, `PlainYearMonth`, `PlainMonthDay`, `Duration`
//! and `Temporal.Now`, on the ISO 8601 calendar only (`iso8601`; every other calendar identifier is refused
//! by name with a RangeError). Time zones are UTC, offset zones and the 84 IANA zones of the Intl data with
//! their historical transitions (`crate::tzdata`, generated from tzdata 2025b by INTLCLDR SR137). `Intl.DateTimeFormat` formats
//! Temporal objects through `format_hook` (ECMA-402 Temporal integration, §11.5.x HandleDateTimeValue).
//!
//! Internal slots ride the engine's Intl object kind (`IntlData::Temporal`), and each realm's Temporal
//! prototypes live in one internal registry record that every Temporal built-in function carries in its
//! first native slot — so creating a result object never consults a user-mutable global.

pub mod diff;
pub mod iso;
pub mod parse;
pub mod zone;

mod duration;
mod instant;
mod plain;
mod zoned;

use super::*;
use crate::builtins::intl::datetime::DtFmt;
use crate::builtins::intl::{alloc_intl, IntlData};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use iso::*;
use zone::{Disambiguation, TimeZone};

/// The internal slots of a Temporal object.
#[derive(Clone, Debug)]
pub enum TData {
    Instant(i128),
    Date(IsoDate),
    Time(Time),
    DateTime(IsoDate, Time),
    /// [[ISODate]] with its reference day.
    YearMonth(IsoDate),
    /// [[ISODate]] with its reference year (1972).
    MonthDay(IsoDate),
    Zoned(i128, TimeZone),
    Duration(Dur),
}

pub const INSTANT: usize = 0;
pub const DATE: usize = 1;
pub const TIME: usize = 2;
pub const DATETIME: usize = 3;
pub const YEARMONTH: usize = 4;
pub const MONTHDAY: usize = 5;
pub const ZONED: usize = 6;
pub const DURATION: usize = 7;
const NTYPES: usize = 8;

pub const NAMES: [&str; NTYPES] = ["Instant", "PlainDate", "PlainTime", "PlainDateTime", "PlainYearMonth", "PlainMonthDay", "ZonedDateTime", "Duration"];

impl TData {
    pub fn index(&self) -> usize {
        match self {
            TData::Instant(_) => INSTANT,
            TData::Date(_) => DATE,
            TData::Time(_) => TIME,
            TData::DateTime(..) => DATETIME,
            TData::YearMonth(_) => YEARMONTH,
            TData::MonthDay(_) => MONTHDAY,
            TData::Zoned(..) => ZONED,
            TData::Duration(_) => DURATION,
        }
    }
}

// ============================================================================================ registry

fn reg_of(vm: &Vm, ctx: &CallCtx) -> Obj {
    vm.native_slot(ctx.callee, 0).as_object().expect("Temporal registry slot")
}

fn reg_get(vm: &Vm, reg: Obj, i: usize) -> Obj {
    match &vm.heap.get(reg).kind {
        Kind::Internal(v) => v[i].as_object().expect("Temporal registry entry"),
        _ => unreachable!("Temporal registry"),
    }
}

/// The realm's prototype of type `i`, from the registry the running built-in carries.
pub fn proto(vm: &Vm, ctx: &CallCtx, i: usize) -> Obj {
    let r = reg_of(vm, ctx);
    reg_get(vm, r, i)
}

/// A new Temporal object of the data's type with the intrinsic prototype.
pub fn make(vm: &mut Vm, ctx: &CallCtx, d: TData) -> Value {
    let p = proto(vm, ctx, d.index());
    Value::Object(alloc_intl(vm, p, IntlData::Temporal(Box::new(d))))
}

/// A constructor's result: `new` required, prototype from NewTarget (GetPrototypeFromConstructor).
pub fn construct(vm: &mut Vm, ctx: &CallCtx, d: TData) -> JsResult<Value> {
    let p = match &ctx.new_target {
        Value::Object(nt) => {
            let nt = *nt;
            match vm.get(nt, &PropertyKey::from_str("prototype"))? {
                Value::Object(p) => p,
                _ => proto(vm, ctx, d.index()),
            }
        }
        _ => return vm.throw_type(&format!("Constructor Temporal.{} requires 'new'", NAMES[d.index()])),
    };
    Ok(Value::Object(alloc_intl(vm, p, IntlData::Temporal(Box::new(d)))))
}

pub fn tdata(vm: &Vm, v: &Value) -> Option<TData> {
    if let Value::Object(o) = v {
        if let Kind::Intl(i) = &vm.heap.get(*o).kind {
            if let IntlData::Temporal(t) = &i.data {
                return Some((**t).clone());
            }
        }
    }
    None
}

pub fn is_type(vm: &Vm, v: &Value, i: usize) -> bool {
    tdata(vm, v).map(|d| d.index() == i).unwrap_or(false)
}

/// The receiver's slots, or a TypeError naming the method.
pub fn this_of(vm: &mut Vm, ctx: &CallCtx, i: usize, method: &str) -> JsResult<TData> {
    match tdata(vm, &ctx.this) {
        Some(d) if d.index() == i => Ok(d),
        _ => vm.throw_type(&format!("Temporal.{}.prototype.{} called on incompatible receiver", NAMES[i], method)),
    }
}

/// Turn a pure-layer error into a RangeError.
pub fn rerr<T>(vm: &mut Vm, r: Result<T, &'static str>) -> JsResult<T> {
    match r {
        Ok(v) => Ok(v),
        Err(m) => vm.throw_range(m),
    }
}

// ============================================================================================ definitions

pub struct Def {
    pub reg: Obj,
}

impl Def {
    fn native(&self, vm: &mut Vm, name: &str, len: u32, f: NativeFn, ctor: bool) -> Obj {
        let fp = vm.intr().function_proto;
        vm.make_native_with(name, len, f, ctor, Some(fp), vec![Value::Object(self.reg)])
    }
    pub fn method(&self, vm: &mut Vm, o: Obj, name: &str, len: u32, f: NativeFn) {
        let fo = self.native(vm, name, len, f, false);
        vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    pub fn getter(&self, vm: &mut Vm, o: Obj, name: &str, f: NativeFn) {
        let g = self.native(vm, &format!("get {name}"), 0, f, false);
        vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop { slot: Slot::Accessor(Some(g), None), flags: C });
    }
    /// A constructor and its prototype (with @@toStringTag), registered at index `i` of the registry.
    pub fn class(&self, vm: &mut Vm, ns: Obj, i: usize, len: u32, f: NativeFn) -> (Obj, Obj) {
        let op = vm.intr().object_proto;
        let p = vm.new_object(Some(op));
        let c = self.native(vm, NAMES[i], len, f, true);
        vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(p), 0));
        vm.heap.get_mut(p).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(c), WC));
        to_str_tag(vm, p, &format!("Temporal.{}", NAMES[i]));
        if let Kind::Internal(v) = &mut vm.heap.get_mut(self.reg).kind {
            v[i] = Value::Object(p);
            v[NTYPES + i] = Value::Object(c);
        }
        vm.heap.get_mut(ns).props.insert(PropertyKey::from_str(NAMES[i]), Prop::data(Value::Object(c), WC));
        (c, p)
    }
}

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let ns = vm.new_object(Some(op));
    to_str_tag(vm, ns, "Temporal");
    let reg = vm.alloc(ObjectData::new(None, Kind::Internal(vec![Value::Undefined; 2 * NTYPES])));
    let d = Def { reg };
    instant::init(vm, &d, ns);
    plain::init(vm, &d, ns);
    zoned::init(vm, &d, ns);
    duration::init(vm, &d, ns);
    let now = vm.new_object(Some(op));
    to_str_tag(vm, now, "Temporal.Now");
    d.method(vm, now, "instant", 0, now_instant);
    d.method(vm, now, "timeZoneId", 0, now_time_zone_id);
    d.method(vm, now, "zonedDateTimeISO", 0, now_zoned);
    d.method(vm, now, "plainDateTimeISO", 0, now_plain_date_time);
    d.method(vm, now, "plainDateISO", 0, now_plain_date);
    d.method(vm, now, "plainTimeISO", 0, now_plain_time);
    vm.heap.get_mut(ns).props.insert(PropertyKey::from_str("Now"), Prop::data(Value::Object(now), WC));
    global(vm, "Temporal", Value::Object(ns));
}

// ============================================================================================ Temporal.Now

fn now_ns(vm: &mut Vm) -> i128 {
    let ms = vm.host.now_ms();
    (crate::numconv::libm_floor(ms) as i128) * 1_000_000
}

/// SystemTimeZoneIdentifier: the host's zone when the data knows it, otherwise UTC or the local offset.
fn system_zone(vm: &mut Vm) -> TimeZone {
    let (_, id) = crate::builtins::intl::datetime::default_zone(vm);
    zone::parse_identifier(&id).unwrap_or_else(TimeZone::utc)
}

fn now_tz_arg(vm: &mut Vm, ctx: &CallCtx) -> JsResult<TimeZone> {
    let v = vm.arg(ctx, 0);
    if v.is_undefined() { Ok(system_zone(vm)) } else { to_time_zone(vm, &v) }
}

fn now_instant(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = now_ns(vm);
    Ok(make(vm, ctx, TData::Instant(n)))
}

fn now_time_zone_id(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let z = system_zone(vm);
    Ok(Value::str(&z.id))
}

fn now_zoned(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let tz = now_tz_arg(vm, ctx)?;
    let n = now_ns(vm);
    Ok(make(vm, ctx, TData::Zoned(n, tz)))
}

fn now_local(vm: &mut Vm, ctx: &CallCtx) -> JsResult<(IsoDate, Time)> {
    let tz = now_tz_arg(vm, ctx)?;
    let n = now_ns(vm);
    Ok(zone::local(&tz, n))
}

fn now_plain_date_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, t) = now_local(vm, ctx)?;
    Ok(make(vm, ctx, TData::DateTime(d, t)))
}

fn now_plain_date(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (d, _) = now_local(vm, ctx)?;
    Ok(make(vm, ctx, TData::Date(d)))
}

fn now_plain_time(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t) = now_local(vm, ctx)?;
    Ok(make(vm, ctx, TData::Time(t)))
}

// ============================================================================================ abstract ops

/// ToIntegerWithTruncation.
pub fn to_int(vm: &mut Vm, v: &Value) -> JsResult<f64> {
    let n = vm.to_number(v)?;
    if !n.is_finite() {
        return vm.throw_range("value must be a finite number");
    }
    let t = trunc(n);
    Ok(if t == 0.0 { 0.0 } else { t })
}

/// ToPositiveIntegerWithTruncation.
pub fn to_pos_int(vm: &mut Vm, v: &Value) -> JsResult<f64> {
    let n = to_int(vm, v)?;
    if n <= 0.0 {
        return vm.throw_range("value must be a positive integer");
    }
    Ok(n)
}

/// ToIntegerIfIntegral.
pub fn to_int_if_integral(vm: &mut Vm, v: &Value) -> JsResult<f64> {
    let n = vm.to_number(v)?;
    if !n.is_finite() || trunc(n) != n {
        return vm.throw_range("value must be an integer");
    }
    Ok(if n == 0.0 { 0.0 } else { n })
}

/// GetOptionsObject.
pub fn get_options(vm: &mut Vm, v: &Value) -> JsResult<Option<Obj>> {
    match v {
        Value::Undefined => Ok(None),
        Value::Object(o) => Ok(Some(*o)),
        _ => vm.throw_type("options must be an object"),
    }
}

pub fn opt_get(vm: &mut Vm, o: Option<Obj>, name: &str) -> JsResult<Value> {
    match o {
        Some(o) => vm.get(o, &PropertyKey::from_str(name)),
        None => Ok(Value::Undefined),
    }
}

/// GetOption for a string-valued option with an allowed list.
pub fn opt_str(vm: &mut Vm, o: Option<Obj>, name: &str, allowed: &[&str], default: &str) -> JsResult<String> {
    let v = opt_get(vm, o, name)?;
    if v.is_undefined() {
        return Ok(String::from(default));
    }
    let s = vm.to_string(&v)?.to_rust();
    if !allowed.contains(&s.as_str()) {
        return vm.throw_range(&format!("{s} is not a valid value for {name}"));
    }
    Ok(s)
}

/// GetTemporalOverflowOption: true = constrain.
pub fn opt_overflow(vm: &mut Vm, o: Option<Obj>) -> JsResult<bool> {
    Ok(opt_str(vm, o, "overflow", &["constrain", "reject"], "constrain")? == "constrain")
}

pub fn opt_disambiguation(vm: &mut Vm, o: Option<Obj>) -> JsResult<Disambiguation> {
    Ok(match opt_str(vm, o, "disambiguation", &["compatible", "earlier", "later", "reject"], "compatible")?.as_str() {
        "earlier" => Disambiguation::Earlier,
        "later" => Disambiguation::Later,
        "reject" => Disambiguation::Reject,
        _ => Disambiguation::Compatible,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OffsetOpt {
    Prefer,
    Use,
    Ignore,
    Reject,
}

pub fn opt_offset(vm: &mut Vm, o: Option<Obj>, default: &str) -> JsResult<OffsetOpt> {
    Ok(match opt_str(vm, o, "offset", &["prefer", "use", "ignore", "reject"], default)?.as_str() {
        "prefer" => OffsetOpt::Prefer,
        "use" => OffsetOpt::Use,
        "ignore" => OffsetOpt::Ignore,
        _ => OffsetOpt::Reject,
    })
}

pub fn opt_rounding_mode(vm: &mut Vm, o: Option<Obj>, default: Mode) -> JsResult<Mode> {
    let v = opt_get(vm, o, "roundingMode")?;
    if v.is_undefined() {
        return Ok(default);
    }
    let s = vm.to_string(&v)?.to_rust();
    match Mode::parse(&s) {
        Some(m) => Ok(m),
        None => vm.throw_range(&format!("{s} is not a valid value for roundingMode")),
    }
}

/// GetRoundingIncrementOption.
pub fn opt_increment(vm: &mut Vm, o: Option<Obj>) -> JsResult<i64> {
    let v = opt_get(vm, o, "roundingIncrement")?;
    if v.is_undefined() {
        return Ok(1);
    }
    let n = vm.to_number(&v)?;
    if !n.is_finite() {
        return vm.throw_range("roundingIncrement must be finite");
    }
    let i = trunc(n);
    if !(1.0..=1e9).contains(&i) {
        return vm.throw_range("roundingIncrement out of range");
    }
    Ok(i as i64)
}

/// ValidateTemporalRoundingIncrement.
pub fn validate_increment(vm: &mut Vm, inc: i64, dividend: i128, inclusive: bool) -> JsResult<()> {
    let max = if inclusive { dividend } else { dividend - 1 };
    if inc as i128 > max || dividend % inc as i128 != 0 {
        return vm.throw_range("roundingIncrement does not divide the unit");
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnitOpt {
    Unset,
    Auto,
    U(Unit),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Date,
    Time,
    DateTime,
}

/// GetTemporalUnitValuedOption: `day_extra` admits "day" in the time group (round), `auto` admits "auto".
pub fn opt_unit(vm: &mut Vm, o: Option<Obj>, key: &str, group: Group, required: bool, auto: bool, day_extra: bool) -> JsResult<UnitOpt> {
    let v = opt_get(vm, o, key)?;
    if v.is_undefined() {
        if required {
            return vm.throw_range(&format!("{key} is required"));
        }
        return Ok(UnitOpt::Unset);
    }
    let s = vm.to_string(&v)?.to_rust();
    if auto && s == "auto" {
        return Ok(UnitOpt::Auto);
    }
    let u = match Unit::parse(&s) {
        Some(u) => u,
        None => return vm.throw_range(&format!("{s} is not a valid value for {key}")),
    };
    let ok = match group {
        Group::Date => u.is_date(),
        Group::Time => !u.is_date() || (day_extra && u == Unit::Day),
        Group::DateTime => true,
    };
    if !ok {
        return vm.throw_range(&format!("{s} is not a valid value for {key}"));
    }
    Ok(UnitOpt::U(u))
}

/// GetTemporalFractionalSecondDigitsOption: `None` = auto.
pub fn opt_fsd(vm: &mut Vm, o: Option<Obj>) -> JsResult<Option<u8>> {
    let v = opt_get(vm, o, "fractionalSecondDigits")?;
    if v.is_undefined() {
        return Ok(None);
    }
    if let Value::Number(n) = v {
        if !n.is_finite() {
            return vm.throw_range("fractionalSecondDigits out of range");
        }
        let d = crate::numconv::libm_floor(n);
        if !(0.0..=9.0).contains(&d) {
            return vm.throw_range("fractionalSecondDigits out of range");
        }
        return Ok(Some(d as u8));
    }
    let s = vm.to_string(&v)?.to_rust();
    if s != "auto" {
        return vm.throw_range(&format!("{s} is not a valid value for fractionalSecondDigits"));
    }
    Ok(None)
}

/// ToSecondsStringPrecisionRecord: (precision, unit, increment).
pub fn seconds_precision(smallest: UnitOpt, fsd: Option<u8>) -> (Precision, Unit, i64) {
    match smallest {
        UnitOpt::U(Unit::Minute) => (Some(PREC_MINUTE), Unit::Minute, 1),
        UnitOpt::U(Unit::Second) => (Some(0), Unit::Second, 1),
        UnitOpt::U(Unit::Millisecond) => (Some(3), Unit::Millisecond, 1),
        UnitOpt::U(Unit::Microsecond) => (Some(6), Unit::Microsecond, 1),
        UnitOpt::U(Unit::Nanosecond) => (Some(9), Unit::Nanosecond, 1),
        _ => match fsd {
            None => (None, Unit::Nanosecond, 1),
            Some(0) => (Some(0), Unit::Second, 1),
            Some(d @ 1..=3) => (Some(d), Unit::Millisecond, 10i64.pow(3 - d as u32)),
            Some(d @ 4..=6) => (Some(d), Unit::Microsecond, 10i64.pow(6 - d as u32)),
            Some(d) => (Some(d), Unit::Nanosecond, 10i64.pow(9 - d as u32)),
        },
    }
}

/// The toString options shared by PlainTime / PlainDateTime / Instant / ZonedDateTime:
/// fractionalSecondDigits, roundingMode, smallestUnit (alphabetical reads), hour refused.
pub fn to_string_rounding(vm: &mut Vm, o: Option<Obj>) -> JsResult<(Precision, Unit, i64, Mode)> {
    let fsd = opt_fsd(vm, o)?;
    let mode = opt_rounding_mode(vm, o, Mode::Trunc)?;
    let su = opt_unit(vm, o, "smallestUnit", Group::Time, false, false, false)?;
    if su == UnitOpt::U(Unit::Hour) {
        return vm.throw_range("smallestUnit hour is not allowed for toString");
    }
    let (p, u, inc) = seconds_precision(su, fsd);
    Ok((p, u, inc, mode))
}

/// The calendarName option (auto / always / never / critical).
pub fn opt_calendar_name(vm: &mut Vm, o: Option<Obj>) -> JsResult<String> {
    opt_str(vm, o, "calendarName", &["auto", "always", "never", "critical"], "auto")
}

/// FormatCalendarAnnotation for iso8601.
pub fn calendar_annotation(show: &str) -> &'static str {
    match show {
        "always" => "[u-ca=iso8601]",
        "critical" => "[!u-ca=iso8601]",
        _ => "",
    }
}

// ------------------------------------------------------------------------------------------- calendars

const KNOWN_CALENDARS: &[&str] = &[
    "buddhist", "chinese", "coptic", "dangi", "ethioaa", "ethiopic", "ethiopic-amete-alem", "gregory", "hebrew", "indian", "islamic", "islamic-civil", "islamic-rgsa",
    "islamic-tbla", "islamic-umalqura", "islamicc", "japanese", "persian", "roc",
];

/// CanonicalizeCalendar: iso8601 only; other calendars are refused by name.
pub fn canonicalize_calendar(vm: &mut Vm, s: &str) -> JsResult<()> {
    let l = s.to_ascii_lowercase();
    if l == "iso8601" {
        return Ok(());
    }
    if KNOWN_CALENDARS.contains(&l.as_str()) {
        return vm.throw_range(&format!("calendar {s} is not supported: this Temporal implements the iso8601 calendar only"));
    }
    vm.throw_range(&format!("invalid calendar identifier: {s}"))
}

/// ToTemporalCalendarIdentifier.
pub fn to_calendar(vm: &mut Vm, v: &Value) -> JsResult<()> {
    if let Some(d) = tdata(vm, v) {
        if !matches!(d, TData::Instant(_) | TData::Duration(_) | TData::Time(_)) {
            return Ok(());
        }
    }
    let s = match v {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("calendar must be a string"),
    };
    // ParseTemporalCalendarString: an ISO string's annotation, or the identifier itself.
    if let Ok(p) = parse::parse_date_time(&s, parse::Form::Relative) {
        if let Some(c) = p.calendar {
            return canonicalize_calendar(vm, &c);
        }
        return Ok(());
    }
    for f in [parse::parse_time as fn(&str) -> parse::PResult<parse::Parsed>, parse::parse_year_month, parse::parse_month_day] {
        if let Ok(p) = f(&s) {
            if let Some(c) = p.calendar {
                return canonicalize_calendar(vm, &c);
            }
            return Ok(());
        }
    }
    canonicalize_calendar(vm, &s)
}

/// A constructor's calendar argument: undefined → iso8601, otherwise a String canonicalized.
pub fn ctor_calendar(vm: &mut Vm, v: &Value) -> JsResult<()> {
    match v {
        Value::Undefined => Ok(()),
        Value::String(s) => {
            let s = s.to_rust();
            canonicalize_calendar(vm, &s)
        }
        _ => vm.throw_type("calendar must be a string"),
    }
}

/// GetTemporalCalendarIdentifierWithISODefault (property bags).
pub fn bag_calendar(vm: &mut Vm, o: Obj) -> JsResult<()> {
    let v = vm.get(o, &PropertyKey::from_str("calendar"))?;
    if v.is_undefined() {
        return Ok(());
    }
    to_calendar(vm, &v)
}

pub fn parsed_calendar(vm: &mut Vm, p: &parse::Parsed) -> JsResult<()> {
    if let Some(c) = &p.calendar {
        let c = c.clone();
        canonicalize_calendar(vm, &c)?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------- time zones

/// ToTemporalTimeZoneIdentifier.
pub fn to_time_zone(vm: &mut Vm, v: &Value) -> JsResult<TimeZone> {
    if let Some(TData::Zoned(_, tz)) = tdata(vm, v) {
        return Ok(tz);
    }
    let s = match v {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("time zone must be a string"),
    };
    match parse::parse_time_zone_string(&s) {
        Ok(parse::TzString::Id(id)) => match zone::parse_identifier(&id) {
            Some(z) => Ok(z),
            None => vm.throw_range(&format!("unknown time zone: {id}")),
        },
        Ok(parse::TzString::Offset(m)) => Ok(TimeZone { id: zone::offset_minutes_id(m), kind: zone::TzKind::Fixed(m * 60_000_000_000), is_offset: true }),
        Err(e) => vm.throw_range(&format!("{e}: {s}")),
    }
}

// ------------------------------------------------------------------------------------------- property bags

#[derive(Default, Clone)]
pub struct Fields {
    pub year: Option<f64>,
    pub month: Option<f64>,
    pub month_code: Option<u8>,
    pub day: Option<f64>,
    /// hour, minute, second, millisecond, microsecond, nanosecond.
    pub time: [Option<f64>; 6],
    pub offset: Option<String>,
    pub tz: Option<TimeZone>,
}

pub const F_YEAR: u32 = 1;
pub const F_MONTH: u32 = 2;
pub const F_DAY: u32 = 4;
pub const F_TIME: u32 = 8;
pub const F_OFFSET: u32 = 16;
pub const F_TZ: u32 = 32;

impl Fields {
    pub fn any(&self) -> bool {
        self.year.is_some() || self.month.is_some() || self.month_code.is_some() || self.day.is_some() || self.time.iter().any(|t| t.is_some()) || self.offset.is_some() || self.tz.is_some()
    }
}

fn month_code(vm: &mut Vm, v: &Value) -> JsResult<u8> {
    let p = vm.to_primitive(v, 2)?;
    let s = match p {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("monthCode must be a string"),
    };
    let b = s.as_bytes();
    if !(b.len() == 3 || (b.len() == 4 && b[3] == b'L')) || b[0] != b'M' || !b[1].is_ascii_digit() || !b[2].is_ascii_digit() {
        return vm.throw_range(&format!("invalid monthCode: {s}"));
    }
    let m = (b[1] - b'0') * 10 + (b[2] - b'0');
    if b.len() == 4 || !(1..=12).contains(&m) {
        return vm.throw_range(&format!("monthCode {s} is not valid in the iso8601 calendar"));
    }
    Ok(m)
}

/// PrepareCalendarFields: reads the requested fields in alphabetical order, converting each as it is read.
/// `partial`: at least one field must be present (TypeError otherwise).
pub fn prepare_fields(vm: &mut Vm, o: Obj, which: u32, required_tz: bool, partial: bool) -> JsResult<Fields> {
    let mut f = Fields::default();
    let get = |vm: &mut Vm, n: &str| vm.get(o, &PropertyKey::from_str(n));
    if which & F_DAY != 0 {
        let v = get(vm, "day")?;
        if !v.is_undefined() {
            f.day = Some(to_pos_int(vm, &v)?);
        }
    }
    let tnames = [("hour", 0usize), ("microsecond", 4), ("millisecond", 3), ("minute", 1)];
    if which & F_TIME != 0 {
        for (n, i) in tnames {
            let v = get(vm, n)?;
            if !v.is_undefined() {
                f.time[i] = Some(to_int(vm, &v)?);
            }
        }
    }
    if which & F_MONTH != 0 {
        let v = get(vm, "month")?;
        if !v.is_undefined() {
            f.month = Some(to_pos_int(vm, &v)?);
        }
        let v = get(vm, "monthCode")?;
        if !v.is_undefined() {
            f.month_code = Some(month_code(vm, &v)?);
        }
    }
    if which & F_TIME != 0 {
        let v = get(vm, "nanosecond")?;
        if !v.is_undefined() {
            f.time[5] = Some(to_int(vm, &v)?);
        }
    }
    if which & F_OFFSET != 0 {
        let v = get(vm, "offset")?;
        if !v.is_undefined() {
            let p = vm.to_primitive(&v, 2)?;
            let s = match p {
                Value::String(s) => s.to_rust(),
                _ => return vm.throw_type("offset must be a string"),
            };
            if parse::parse_offset_string(&s).is_err() {
                return vm.throw_range(&format!("invalid offset: {s}"));
            }
            f.offset = Some(s);
        }
    }
    if which & F_TIME != 0 {
        let v = get(vm, "second")?;
        if !v.is_undefined() {
            f.time[2] = Some(to_int(vm, &v)?);
        }
    }
    if which & F_TZ != 0 {
        let v = get(vm, "timeZone")?;
        if !v.is_undefined() {
            f.tz = Some(to_time_zone(vm, &v)?);
        } else if required_tz {
            return vm.throw_type("timeZone is required");
        }
    }
    if which & F_YEAR != 0 {
        let v = get(vm, "year")?;
        if !v.is_undefined() {
            f.year = Some(to_int(vm, &v)?);
        }
    }
    if partial && !f.any() {
        return vm.throw_type("at least one recognized property is required");
    }
    Ok(f)
}

/// The month of a field set (CalendarResolveFields): monthCode and month must agree; one is required.
pub fn resolve_month(vm: &mut Vm, f: &Fields) -> JsResult<i64> {
    match (f.month, f.month_code) {
        (Some(m), Some(c)) => {
            if m as i64 != c as i64 {
                return vm.throw_range("month and monthCode disagree");
            }
            Ok(m as i64)
        }
        (Some(m), None) => Ok(m as i64),
        (None, Some(c)) => Ok(c as i64),
        (None, None) => vm.throw_type("month or monthCode is required"),
    }
}

/// CalendarDateFromFields for iso8601.
pub fn date_from_fields(vm: &mut Vm, f: &Fields, constrain: bool) -> JsResult<IsoDate> {
    let y = match f.year {
        Some(y) => y as i64,
        None => return vm.throw_type("year is required"),
    };
    let m = resolve_month(vm, f)?;
    let d = match f.day {
        Some(d) => d as i64,
        None => return vm.throw_type("day is required"),
    };
    let y = y.clamp(-1_000_000, 1_000_000);
    let m = m.min(1_000_000);
    let d = d.min(1_000_000);
    let r = match regulate_date(y, m, d, constrain) {
        Some(r) => r,
        None => return vm.throw_range("date out of range (overflow: reject)"),
    };
    if !date_within_limits(r) {
        return vm.throw_range("date outside the representable range");
    }
    Ok(r)
}

/// The time of a field set (absent fields are zero), regulated.
pub fn time_from_fields(vm: &mut Vm, f: &Fields, constrain: bool) -> JsResult<Time> {
    let v = f.time.map(|x| x.unwrap_or(0.0).clamp(-1e15, 1e15) as i64);
    match regulate_time(v, constrain) {
        Some(t) => Ok(t),
        None => vm.throw_range("time out of range (overflow: reject)"),
    }
}

/// ToTemporalTimeRecord (partial or complete) for PlainTime bags.
pub fn time_bag(vm: &mut Vm, o: Obj, partial: bool) -> JsResult<[Option<f64>; 6]> {
    let names = [("hour", 0usize), ("microsecond", 4), ("millisecond", 3), ("minute", 1), ("nanosecond", 5), ("second", 2)];
    let mut out = [None; 6];
    let mut any = false;
    for (n, i) in names {
        let v = vm.get(o, &PropertyKey::from_str(n))?;
        if !v.is_undefined() {
            out[i] = Some(to_int(vm, &v)?);
            any = true;
        }
    }
    if !any {
        let _ = partial;
        return vm.throw_type("at least one time property is required");
    }
    Ok(out)
}

/// RejectTemporalLikeObject (the `with` methods).
pub fn reject_temporal_like(vm: &mut Vm, v: &Value) -> JsResult<Obj> {
    let o = match v {
        Value::Object(o) => *o,
        _ => return vm.throw_type("argument must be an object"),
    };
    if let Some(d) = tdata(vm, v) {
        if !matches!(d, TData::Instant(_) | TData::Duration(_)) {
            return vm.throw_type("a Temporal object is not a valid property bag here");
        }
    }
    for k in ["calendar", "timeZone"] {
        let x = vm.get(o, &PropertyKey::from_str(k))?;
        if !x.is_undefined() {
            return vm.throw_type(&format!("{k} is not allowed in a with() property bag"));
        }
    }
    Ok(o)
}

/// CalendarMergeFields for iso8601: a new month or monthCode replaces both of the receiver's.
pub fn merge_fields(base: &Fields, partial: &Fields) -> Fields {
    let mut f = base.clone();
    if partial.month.is_some() || partial.month_code.is_some() {
        f.month = partial.month;
        f.month_code = partial.month_code;
    }
    if partial.year.is_some() {
        f.year = partial.year;
    }
    if partial.day.is_some() {
        f.day = partial.day;
    }
    for i in 0..6 {
        if partial.time[i].is_some() {
            f.time[i] = partial.time[i];
        }
    }
    if partial.offset.is_some() {
        f.offset = partial.offset.clone();
    }
    f
}

pub fn date_fields(d: IsoDate) -> Fields {
    Fields { year: Some(d.year as f64), month: Some(d.month as f64), month_code: Some(d.month), day: Some(d.day as f64), ..Default::default() }
}

pub fn time_fields(f: &mut Fields, t: Time) {
    let v = t.fields();
    for i in 0..6 {
        f.time[i] = Some(v[i] as f64);
    }
}

pub fn month_code_str(m: u8) -> String {
    format!("M{m:02}")
}

// ------------------------------------------------------------------------------------------- conversions

/// ToTemporalDate.
pub fn to_date(vm: &mut Vm, item: &Value, options: &Value) -> JsResult<IsoDate> {
    if let Value::Object(o) = item {
        let o = *o;
        if let Some(d) = tdata(vm, item) {
            let r = match d {
                TData::Date(d) => Some(d),
                TData::DateTime(d, _) => Some(d),
                TData::Zoned(n, tz) => Some(zone::local(&tz, n).0),
                _ => None,
            };
            if let Some(r) = r {
                let opts = get_options(vm, options)?;
                opt_overflow(vm, opts)?;
                return Ok(r);
            }
        }
        bag_calendar(vm, o)?;
        let f = prepare_fields(vm, o, F_YEAR | F_MONTH | F_DAY, false, false)?;
        let opts = get_options(vm, options)?;
        let c = opt_overflow(vm, opts)?;
        return date_from_fields(vm, &f, c);
    }
    let s = match item {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("cannot convert to a PlainDate"),
    };
    let p = rerr(vm, parse::parse_date_time(&s, parse::Form::DateTime))?;
    parsed_calendar(vm, &p)?;
    let opts = get_options(vm, options)?;
    opt_overflow(vm, opts)?;
    let d = IsoDate::new(p.year, p.month, p.day);
    if !date_within_limits(d) {
        return vm.throw_range("date outside the representable range");
    }
    Ok(d)
}

/// ToTemporalDateTime.
pub fn to_date_time(vm: &mut Vm, item: &Value, options: &Value) -> JsResult<(IsoDate, Time)> {
    if let Value::Object(o) = item {
        let o = *o;
        if let Some(d) = tdata(vm, item) {
            let r = match d {
                TData::DateTime(d, t) => Some((d, t)),
                TData::Date(d) => Some((d, Time::MIDNIGHT)),
                TData::Zoned(n, tz) => Some(zone::local(&tz, n)),
                _ => None,
            };
            if let Some(r) = r {
                let opts = get_options(vm, options)?;
                opt_overflow(vm, opts)?;
                return Ok(r);
            }
        }
        bag_calendar(vm, o)?;
        let f = prepare_fields(vm, o, F_YEAR | F_MONTH | F_DAY | F_TIME, false, false)?;
        let opts = get_options(vm, options)?;
        let c = opt_overflow(vm, opts)?;
        let d = date_from_fields(vm, &f, c)?;
        let t = time_from_fields(vm, &f, c)?;
        if !datetime_within_limits(d, t) {
            return vm.throw_range("date-time outside the representable range");
        }
        return Ok((d, t));
    }
    let s = match item {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("cannot convert to a PlainDateTime"),
    };
    let p = rerr(vm, parse::parse_date_time(&s, parse::Form::DateTime))?;
    parsed_calendar(vm, &p)?;
    let opts = get_options(vm, options)?;
    opt_overflow(vm, opts)?;
    let d = IsoDate::new(p.year, p.month, p.day);
    let t = p.time.unwrap_or(Time::MIDNIGHT);
    if !datetime_within_limits(d, t) {
        return vm.throw_range("date-time outside the representable range");
    }
    Ok((d, t))
}

/// ToTemporalTime.
pub fn to_time(vm: &mut Vm, item: &Value, options: &Value) -> JsResult<Time> {
    if let Value::Object(o) = item {
        let o = *o;
        if let Some(d) = tdata(vm, item) {
            let r = match d {
                TData::Time(t) => Some(t),
                TData::DateTime(_, t) => Some(t),
                TData::Zoned(n, tz) => Some(zone::local(&tz, n).1),
                _ => None,
            };
            if let Some(r) = r {
                let opts = get_options(vm, options)?;
                opt_overflow(vm, opts)?;
                return Ok(r);
            }
        }
        let tb = time_bag(vm, o, false)?;
        let opts = get_options(vm, options)?;
        let c = opt_overflow(vm, opts)?;
        let f = Fields { time: tb, ..Default::default() };
        return time_from_fields(vm, &f, c);
    }
    let s = match item {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("cannot convert to a PlainTime"),
    };
    let p = rerr(vm, parse::parse_time(&s))?;
    parsed_calendar(vm, &p)?;
    let opts = get_options(vm, options)?;
    opt_overflow(vm, opts)?;
    Ok(p.time.unwrap_or(Time::MIDNIGHT))
}

/// ToTemporalInstant.
pub fn to_instant(vm: &mut Vm, item: &Value) -> JsResult<i128> {
    if let Some(d) = tdata(vm, item) {
        match d {
            TData::Instant(n) => return Ok(n),
            TData::Zoned(n, _) => return Ok(n),
            _ => {}
        }
    }
    let p = vm.to_primitive(item, 2)?;
    let s = match p {
        Value::String(s) => s.to_rust(),
        _ => return vm.throw_type("cannot convert to a Temporal.Instant"),
    };
    let r = rerr(vm, parse::parse_date_time(&s, parse::Form::Instant))?;
    let off = if r.z { 0 } else { r.offset.map(|o| o.ns).unwrap_or(0) };
    let d = IsoDate::new(r.year, r.month, r.day);
    let t = r.time.unwrap_or(Time::MIDNIGHT);
    if d.year.abs() > 300_000 {
        return vm.throw_range("instant outside the representable range");
    }
    let n = utc_ns(d, t) - off as i128;
    if !valid_epoch_ns(n) {
        return vm.throw_range("instant outside the representable range");
    }
    Ok(n)
}

pub fn bigint_from_i128(n: i128) -> Value {
    Value::BigInt(Rc::new(BigInt::from_mag(n < 0, crate::bignum::BigUint::from_u128(n.unsigned_abs()))))
}

pub fn i128_from_bigint(b: &BigInt) -> Option<i128> {
    if b.mag.limbs.len() > 3 {
        return None;
    }
    let mut v: u128 = 0;
    for (i, l) in b.mag.limbs.iter().enumerate() {
        v |= (*l as u128) << (32 * i);
    }
    let v = v as i128;
    Some(if b.neg { -v } else { v })
}

/// InterpretISODateTimeOffset (§6.5.x). `time: None` = start of day.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OffsetBehaviour {
    Option,
    Exact,
    Wall,
}

#[allow(clippy::too_many_arguments)]
pub fn interpret_offset(
    vm: &mut Vm,
    d: IsoDate,
    time: Option<Time>,
    beh: OffsetBehaviour,
    off_ns: i64,
    tz: &TimeZone,
    dis: Disambiguation,
    opt: OffsetOpt,
    match_minutes: bool,
) -> JsResult<i128> {
    if !date_within_limits(d) && d.year.abs() > 300_000 {
        return vm.throw_range("date outside the representable range");
    }
    let t = match time {
        None => {
            let r = zone::start_of_day(tz, d);
            return rerr(vm, r);
        }
        Some(t) => t,
    };
    if beh == OffsetBehaviour::Wall || (beh == OffsetBehaviour::Option && opt == OffsetOpt::Ignore) {
        if !datetime_within_limits(d, t) {
            return vm.throw_range("date-time outside the representable range");
        }
        let r = zone::epoch_ns_for(tz, d, t, dis);
        return rerr(vm, r);
    }
    if beh == OffsetBehaviour::Exact || opt == OffsetOpt::Use {
        let n = utc_ns(d, t) - off_ns as i128;
        if !valid_epoch_ns(n) {
            return vm.throw_range("instant outside the representable range");
        }
        return Ok(n);
    }
    if !datetime_within_limits(d, t) {
        return vm.throw_range("date-time outside the representable range");
    }
    let utc = utc_ns(d, t);
    let cands = zone::possible(tz, d, t);
    let cands = rerr(vm, cands)?;
    for c in &cands {
        let co = utc - c;
        if co == off_ns as i128 {
            return Ok(*c);
        }
        if match_minutes && round_incr(co, 60_000_000_000, Mode::HalfExpand) == off_ns as i128 {
            return Ok(*c);
        }
    }
    if opt == OffsetOpt::Reject {
        return vm.throw_range(&format!("offset {} is invalid for {} in {}", offset_string(off_ns, false), date_string(d), tz.id));
    }
    let r = zone::disambiguate(&cands, tz, d, t, dis);
    rerr(vm, r)
}

/// ToTemporalZonedDateTime.
pub fn to_zoned(vm: &mut Vm, item: &Value, options: &Value) -> JsResult<(i128, TimeZone)> {
    let (d, time, beh, off_ns, tz, match_minutes);
    let (dis, oopt);
    if let Value::Object(o) = item {
        let o = *o;
        if let Some(TData::Zoned(n, z)) = tdata(vm, item) {
            let opts = get_options(vm, options)?;
            opt_disambiguation(vm, opts)?;
            opt_offset(vm, opts, "reject")?;
            opt_overflow(vm, opts)?;
            return Ok((n, z));
        }
        bag_calendar(vm, o)?;
        let f = prepare_fields(vm, o, F_YEAR | F_MONTH | F_DAY | F_TIME | F_OFFSET | F_TZ, true, false)?;
        let opts = get_options(vm, options)?;
        dis = opt_disambiguation(vm, opts)?;
        oopt = opt_offset(vm, opts, "reject")?;
        let c = opt_overflow(vm, opts)?;
        d = date_from_fields(vm, &f, c)?;
        time = Some(time_from_fields(vm, &f, c)?);
        tz = f.tz.clone().unwrap();
        match &f.offset {
            Some(s) => {
                beh = OffsetBehaviour::Option;
                off_ns = parse::parse_offset_string(s).map(|o| o.ns).unwrap_or(0);
            }
            None => {
                beh = OffsetBehaviour::Wall;
                off_ns = 0;
            }
        }
        match_minutes = false;
    } else {
        let s = match item {
            Value::String(s) => s.to_rust(),
            _ => return vm.throw_type("cannot convert to a ZonedDateTime"),
        };
        let p = rerr(vm, parse::parse_date_time(&s, parse::Form::Zoned))?;
        let tzs = p.tz.clone().unwrap();
        tz = match zone::parse_identifier(&tzs) {
            Some(z) => z,
            None => return vm.throw_range(&format!("unknown time zone: {tzs}")),
        };
        parsed_calendar(vm, &p)?;
        let opts = get_options(vm, options)?;
        dis = opt_disambiguation(vm, opts)?;
        oopt = opt_offset(vm, opts, "reject")?;
        opt_overflow(vm, opts)?;
        d = IsoDate::new(p.year, p.month, p.day);
        time = p.time;
        if p.z {
            beh = OffsetBehaviour::Exact;
            off_ns = 0;
        } else if let Some(o) = p.offset {
            beh = OffsetBehaviour::Option;
            off_ns = o.ns;
        } else {
            beh = OffsetBehaviour::Wall;
            off_ns = 0;
        }
        match_minutes = !p.offset.map(|o| o.sub_minute).unwrap_or(false);
    }
    let n = interpret_offset(vm, d, time, beh, off_ns, &tz, dis, oopt, match_minutes)?;
    Ok((n, tz))
}

/// ToTemporalDuration.
pub fn to_duration(vm: &mut Vm, item: &Value) -> JsResult<Dur> {
    if let Some(TData::Duration(d)) = tdata(vm, item) {
        return Ok(d);
    }
    let o = match item {
        Value::Object(o) => *o,
        Value::String(s) => {
            let s = s.to_rust();
            return rerr(vm, parse::parse_duration(&s));
        }
        _ => return vm.throw_type("cannot convert to a Temporal.Duration"),
    };
    let p = partial_duration(vm, o)?;
    let mut d = Dur::default();
    for i in 0..10 {
        d.f[i] = p[i].unwrap_or(0.0);
    }
    if !d.is_valid() {
        return vm.throw_range("invalid duration");
    }
    Ok(d)
}

/// ToTemporalPartialDurationRecord: alphabetical reads, at least one field.
pub fn partial_duration(vm: &mut Vm, o: Obj) -> JsResult<[Option<f64>; 10]> {
    let names = [
        ("days", 3usize),
        ("hours", 4),
        ("microseconds", 8),
        ("milliseconds", 7),
        ("minutes", 5),
        ("months", 1),
        ("nanoseconds", 9),
        ("seconds", 6),
        ("weeks", 2),
        ("years", 0),
    ];
    let mut out = [None; 10];
    let mut any = false;
    for (n, i) in names {
        let v = vm.get(o, &PropertyKey::from_str(n))?;
        if !v.is_undefined() {
            out[i] = Some(to_int_if_integral(vm, &v)?);
            any = true;
        }
    }
    if !any {
        return vm.throw_type("a duration-like object needs at least one duration property");
    }
    Ok(out)
}

/// GetTemporalRelativeToOption: (plain date, zoned) — at most one is set.
pub fn relative_to(vm: &mut Vm, o: Option<Obj>) -> JsResult<(Option<IsoDate>, Option<(i128, TimeZone)>)> {
    let v = opt_get(vm, o, "relativeTo")?;
    if v.is_undefined() {
        return Ok((None, None));
    }
    let (d, time, tz, beh, off_ns, match_minutes);
    if let Value::Object(ro) = &v {
        let ro = *ro;
        if let Some(t) = tdata(vm, &v) {
            match t {
                TData::Zoned(n, z) => return Ok((None, Some((n, z)))),
                TData::Date(d) => return Ok((Some(d), None)),
                TData::DateTime(d, _) => return Ok((Some(d), None)),
                _ => {}
            }
        }
        bag_calendar(vm, ro)?;
        let f = prepare_fields(vm, ro, F_YEAR | F_MONTH | F_DAY | F_TIME | F_OFFSET | F_TZ, false, false)?;
        d = date_from_fields(vm, &f, true)?;
        time = Some(time_from_fields(vm, &f, true)?);
        tz = f.tz.clone();
        match &f.offset {
            Some(s) => {
                beh = OffsetBehaviour::Option;
                off_ns = parse::parse_offset_string(s).map(|o| o.ns).unwrap_or(0);
            }
            None => {
                beh = OffsetBehaviour::Wall;
                off_ns = 0;
            }
        }
        match_minutes = false;
    } else {
        let s = match &v {
            Value::String(s) => s.to_rust(),
            _ => return vm.throw_type("relativeTo must be a Temporal object, a property bag or a string"),
        };
        let p = rerr(vm, parse::parse_date_time(&s, parse::Form::Relative))?;
        tz = match &p.tz {
            Some(id) => match zone::parse_identifier(id) {
                Some(z) => Some(z),
                None => return vm.throw_range(&format!("unknown time zone: {id}")),
            },
            None => None,
        };
        parsed_calendar(vm, &p)?;
        d = IsoDate::new(p.year, p.month, p.day);
        time = if p.time.is_none() && tz.is_some() { None } else { Some(p.time.unwrap_or(Time::MIDNIGHT)) };
        if p.z {
            beh = OffsetBehaviour::Exact;
            off_ns = 0;
        } else if let Some(o) = p.offset {
            beh = OffsetBehaviour::Option;
            off_ns = o.ns;
        } else {
            beh = OffsetBehaviour::Wall;
            off_ns = 0;
        }
        match_minutes = true;
    }
    match tz {
        None => {
            if !date_within_limits(d) {
                return vm.throw_range("relativeTo outside the representable range");
            }
            Ok((Some(d), None))
        }
        Some(z) => {
            let n = interpret_offset(vm, d, time, beh, off_ns, &z, Disambiguation::Compatible, OffsetOpt::Reject, match_minutes)?;
            Ok((None, Some((n, z))))
        }
    }
}

// ============================================================================================ Intl hook

/// Intl.DateTimeFormat over a Temporal object (ECMA-402 Temporal §11.5.x HandleDateTimeValue): plain
/// types format their wall-clock fields with no time zone, Instant in the formatter's zone. `None` when `v`
/// is not a Temporal object. A ZonedDateTime is refused (its own toLocaleString formats it).
pub fn format_hook(vm: &mut Vm, f: &DtFmt, v: &Value) -> JsResult<Option<Vec<(String, String)>>> {
    let d = match tdata(vm, v) {
        Some(d) => d,
        None => return Ok(None),
    };
    let (g, ms, off) = match prepare_format(vm, f, &d)? {
        Some(x) => x,
        None => return vm.throw_type("Intl.DateTimeFormat cannot format a Temporal.ZonedDateTime; use toLocaleString"),
    };
    Ok(Some(crate::builtins::intl::datetime::format_parts(&g, ms, off)))
}

fn floor_div(a: i128, b: i128) -> i128 {
    a.div_euclid(b)
}

/// The derived format (the type's own field defaults), the epoch milliseconds and the offset to show.
pub fn prepare_format(vm: &mut Vm, f: &DtFmt, d: &TData) -> JsResult<Option<(DtFmt, f64, f64)>> {
    let mut g = f.clone();
    let has_date = |g: &DtFmt| g.weekday.is_some() || g.era.is_some() || g.year.is_some() || g.month.is_some() || g.day.is_some();
    let has_time = |g: &DtFmt| g.day_period.is_some() || g.hour.is_some() || g.minute.is_some() || g.second.is_some() || g.fsd.is_some();
    let clear_time = |g: &mut DtFmt| {
        g.day_period = None;
        g.hour = None;
        g.minute = None;
        g.second = None;
        g.fsd = None;
        g.time_style = None;
    };
    let clear_date = |g: &mut DtFmt| {
        g.weekday = None;
        g.era = None;
        g.year = None;
        g.month = None;
        g.day = None;
        g.date_style = None;
    };
    let date_defaults = |g: &mut DtFmt| {
        g.year = Some("numeric");
        g.month = Some("numeric");
        g.day = Some("numeric");
    };
    let time_defaults = |g: &mut DtFmt| {
        g.hour = Some("numeric");
        g.minute = Some("numeric");
        g.second = Some("numeric");
    };
    let explicit = f.explicit;
    let wall = |dd: IsoDate, tt: Time| floor_div(utc_ns(dd, tt), 1_000_000) as f64;
    let (ms, off) = match d {
        TData::Date(dd) => {
            if f.time_style.is_some() && f.date_style.is_none() {
                return vm.throw_type("timeStyle cannot format a Temporal.PlainDate");
            }
            clear_time(&mut g);
            g.tzn = None;
            if !explicit {
                clear_date(&mut g);
                date_defaults(&mut g);
            } else if !has_date(&g) && g.date_style.is_none() {
                return vm.throw_type("the format has no date fields for a Temporal.PlainDate");
            }
            (wall(*dd, Time::NOON), 0.0)
        }
        TData::Time(tt) => {
            if f.date_style.is_some() && f.time_style.is_none() {
                return vm.throw_type("dateStyle cannot format a Temporal.PlainTime");
            }
            clear_date(&mut g);
            g.tzn = None;
            if !explicit {
                clear_time(&mut g);
                time_defaults(&mut g);
            } else if !has_time(&g) && g.time_style.is_none() {
                return vm.throw_type("the format has no time fields for a Temporal.PlainTime");
            }
            (wall(IsoDate::new(1970, 1, 1), *tt), 0.0)
        }
        TData::DateTime(dd, tt) => {
            g.tzn = None;
            if !explicit {
                clear_date(&mut g);
                clear_time(&mut g);
                date_defaults(&mut g);
                time_defaults(&mut g);
            }
            (wall(*dd, *tt), 0.0)
        }
        TData::YearMonth(dd) => {
            if f.time_style.is_some() && f.date_style.is_none() {
                return vm.throw_type("timeStyle cannot format a Temporal.PlainYearMonth");
            }
            clear_time(&mut g);
            g.tzn = None;
            g.weekday = None;
            g.day = None;
            if !explicit {
                clear_date(&mut g);
                g.year = Some("numeric");
                g.month = Some("numeric");
            } else if g.year.is_none() && g.month.is_none() && g.era.is_none() && g.date_style.is_none() {
                return vm.throw_type("the format has no year or month for a Temporal.PlainYearMonth");
            }
            (wall(*dd, Time::NOON), 0.0)
        }
        TData::MonthDay(dd) => {
            if f.time_style.is_some() && f.date_style.is_none() {
                return vm.throw_type("timeStyle cannot format a Temporal.PlainMonthDay");
            }
            clear_time(&mut g);
            g.tzn = None;
            g.weekday = None;
            g.year = None;
            g.era = None;
            if !explicit {
                clear_date(&mut g);
                g.month = Some("numeric");
                g.day = Some("numeric");
            } else if g.month.is_none() && g.day.is_none() && g.date_style.is_none() {
                return vm.throw_type("the format has no month or day for a Temporal.PlainMonthDay");
            }
            (wall(*dd, Time::NOON), 0.0)
        }
        TData::Instant(n) => {
            if !explicit {
                clear_date(&mut g);
                clear_time(&mut g);
                date_defaults(&mut g);
                time_defaults(&mut g);
            }
            let ms = floor_div(*n, 1_000_000) as f64;
            let off = crate::builtins::intl::datetime::zone_offset_of(vm, f, ms);
            (ms, off)
        }
        TData::Zoned(..) => return Ok(None),
        TData::Duration(_) => return vm.throw_type("Intl.DateTimeFormat cannot format a Temporal.Duration"),
    };
    if g.hour.is_some() && g.hc.is_none() {
        g.hc = Some(if g.gb { "h23" } else { "h12" });
    }
    if matches!(d, TData::Date(_) | TData::Time(_) | TData::DateTime(..) | TData::YearMonth(_) | TData::MonthDay(_)) {
        g.zone = crate::builtins::intl::datetime::ZoneRule::Fixed(0.0);
        g.zone_id = String::from("UTC");
    }
    Ok(Some((g, ms, off)))
}

/// `toLocaleString(locales, options)` of every Temporal type but ZonedDateTime.
pub fn to_locale_string(vm: &mut Vm, ctx: &CallCtx, d: &TData) -> JsResult<Value> {
    let l = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let f = crate::builtins::intl::datetime::create(vm, &l, &o, "any", "all")?;
    let (g, ms, off) = prepare_format(vm, &f, d)?.expect("not zoned");
    let parts = crate::builtins::intl::datetime::format_parts(&g, ms, off);
    Ok(Value::str(&crate::builtins::intl::concat_parts(&parts)))
}

pub fn value_of(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    vm.throw_type("Temporal objects do not convert to primitives with valueOf; use compare() or equals()")
}

pub struct DiffSettings {
    pub largest: Unit,
    pub smallest: Unit,
    pub inc: i64,
    pub mode: Mode,
}

/// GetDifferenceSettings (§13.x): largestUnit, roundingIncrement, roundingMode, smallestUnit (alphabetical).
pub fn difference_settings(vm: &mut Vm, since: bool, options: &Value, group: Group, disallowed: &[Unit], fallback_smallest: Unit, default_largest: Unit) -> JsResult<DiffSettings> {
    let o = get_options(vm, options)?;
    let lu = opt_unit(vm, o, "largestUnit", group, false, true, false)?;
    if let UnitOpt::U(u) = lu {
        if disallowed.contains(&u) {
            return vm.throw_range(&format!("largestUnit {} is not allowed here", u.singular()));
        }
    }
    let inc = opt_increment(vm, o)?;
    let mut mode = opt_rounding_mode(vm, o, Mode::Trunc)?;
    if since {
        mode = mode.negate();
    }
    let su = opt_unit(vm, o, "smallestUnit", group, false, false, false)?;
    let smallest = match su {
        UnitOpt::U(u) => u,
        _ => fallback_smallest,
    };
    if disallowed.contains(&smallest) {
        return vm.throw_range(&format!("smallestUnit {} is not allowed here", smallest.singular()));
    }
    let dl = larger(default_largest, smallest);
    let largest = match lu {
        UnitOpt::U(u) => u,
        _ => dl,
    };
    if larger(largest, smallest) != largest {
        return vm.throw_range("largestUnit must be at least as large as smallestUnit");
    }
    if let Some(max) = max_increment(smallest) {
        validate_increment(vm, inc, max, false)?;
    }
    Ok(DiffSettings { largest, smallest, inc, mode })
}

/// A Duration result of a difference: TemporalDurationFromInternal, negated for `since`.
pub fn duration_result(vm: &mut Vm, ctx: &CallCtx, d: IDur, largest: Unit, since: bool) -> JsResult<Value> {
    let r = match from_internal(d, largest) {
        Some(r) => r,
        None => return vm.throw_range("duration out of range"),
    };
    let r = if since { r.negated() } else { r };
    Ok(make(vm, ctx, TData::Duration(r)))
}

/// The round() parameter: a string is `{ smallestUnit }`; undefined is a TypeError.
pub fn round_options(vm: &mut Vm, v: &Value) -> JsResult<(Option<Obj>, Option<String>)> {
    match v {
        Value::Undefined => vm.throw_type("round() requires an options object or a unit string"),
        Value::String(s) => Ok((None, Some(s.to_rust()))),
        _ => Ok((get_options(vm, v)?, None)),
    }
}

/// smallestUnit of round(): from the string shorthand or the options bag.
pub fn round_smallest(vm: &mut Vm, o: Option<Obj>, shorthand: &Option<String>, group: Group, day_extra: bool) -> JsResult<Unit> {
    match shorthand {
        Some(s) => {
            let u = match Unit::parse(s) {
                Some(u) => u,
                None => return vm.throw_range(&format!("{s} is not a valid unit")),
            };
            let ok = match group {
                Group::Date => u.is_date(),
                Group::Time => !u.is_date() || (day_extra && u == Unit::Day),
                Group::DateTime => true,
            };
            if !ok {
                return vm.throw_range(&format!("{s} is not a valid unit here"));
            }
            Ok(u)
        }
        None => match opt_unit(vm, o, "smallestUnit", group, true, false, day_extra)? {
            UnitOpt::U(u) => Ok(u),
            _ => vm.throw_range("smallestUnit is required"),
        },
    }
}

/// ISODateTimeToString.
pub fn datetime_string(d: IsoDate, t: Time, p: Precision) -> String {
    format!("{}T{}", date_string(d), time_string(t, p))
}
